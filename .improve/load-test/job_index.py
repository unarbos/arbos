#!/usr/bin/env python3
"""Compare the actual box-worker query on retained old and current source DDL."""
import argparse
import ctypes as C
import hashlib
import json
from pathlib import Path
import random
import sqlite3
import tempfile
import time

from analysis_stats import paired
from coverage import BoundQueries, seed_box, sources
from engine import Engine

HERE = Path(__file__).resolve().parent
BEFORE = HERE / 'JOB_INDEX_BEFORE.json'
INDEX = 'CREATE INDEX IF NOT EXISTS jobs_subagent_created ON jobs(subagent_id, created_at DESC);'


def bundled_rows(engine, query, worker):
    """Read the complete projected job row through production-version SQLite."""
    BoundQueries(engine)  # Configure the shared library's parameterized SELECT API.
    lib, statement, tail = engine.lib, C.c_void_p(), C.c_char_p()
    lib.sqlite3_column_type.argtypes = [C.c_void_p, C.c_int]
    lib.sqlite3_column_int64.argtypes = [C.c_void_p, C.c_int]
    lib.sqlite3_column_int64.restype = C.c_int64
    rc = lib.sqlite3_prepare_v2(engine.db, query.encode(), -1, C.byref(statement), C.byref(tail))
    if rc:
        raise RuntimeError(f'Bundled full-row prepare failed: {rc}')
    try:
        rc = lib.sqlite3_bind_null(statement, 1) if worker is None else lib.sqlite3_bind_text(statement, 1, worker.encode(), -1, C.c_void_p(-1))
        if rc:
            raise RuntimeError(f'Bundled full-row bind failed: {rc}')
        out = []
        while (rc := lib.sqlite3_step(statement)) == 100:
            row = []
            for col in range(lib.sqlite3_column_count(statement)):
                kind = lib.sqlite3_column_type(statement, col)
                if kind == 1:
                    row.append(lib.sqlite3_column_int64(statement, col))
                elif kind == 3:
                    row.append(lib.sqlite3_column_text(statement, col).decode())
                elif kind == 5:
                    row.append(None)
                else:
                    raise AssertionError(f'Unexpected projected job SQL type: {kind}')
            out.append(tuple(row))
        if rc != 101:
            raise RuntimeError(f'Bundled full-row step failed: {rc}')
        return out
    finally:
        lib.sqlite3_finalize(statement)


def rows(path, query, worker):
    c = sqlite3.connect(f'file:{path}?mode=ro', uri=True)
    try:
        return c.execute(query, (worker,)).fetchall()
    finally:
        c.close()


def regression(schema, query):
    """Pin full-row winner semantics, index replay, NULL and absent workers."""
    with tempfile.TemporaryDirectory(prefix='arbos-job-index-check-') as tmp:
        path = Path(tmp) / 'fresh.db'
        c = sqlite3.connect(path)
        try:
            c.executescript(schema)
            for job, worker, at, title in (
                    ('older', 'worker', 1, 'Old'), ('latest', 'worker', 10, 'Latest'),
                    ('tie-first', 'tied', 20, 'First insertion'),
                    ('tie-second', 'tied', 20, 'Second insertion'),
                    ('null-worker', None, 100, 'Unassigned')):
                c.execute("INSERT INTO jobs(id,chat_id,ask_message_id,subagent_id,title,kind,brief,state,created_at) VALUES (?, 'chat', 'ask', ?, ?, 'task', 'Synthetic regression', 'done', ?)",
                          (job, worker, title, at))
            initial = {worker: c.execute(query, (worker,)).fetchall() for worker in ('worker', 'tied', 'missing', None)}
            assert initial['worker'][0][0] == 'latest'
            assert initial['tied'][0][0] == 'tie-first'
            assert initial['missing'] == initial[None] == []
            # Replaying actual SCHEMA is how Store::open upgrades an existing store.
            c.executescript(schema)
            assert initial == {worker: c.execute(query, (worker,)).fetchall() for worker in initial}
            c.commit()
            assert c.execute('PRAGMA integrity_check').fetchone() == ('ok',)
            engine = Engine(path)
            try:
                assert initial == {worker: bundled_rows(engine, query, worker) for worker in initial}
            finally:
                engine.close()
            return initial
        finally:
            c.close()


def capture():
    if BEFORE.exists():
        raise ValueError('Refusing to overwrite retained pre-edit source')
    source = sources()
    schema, query = source['box_schema'], source['queries']['box_job_for_worker']
    if INDEX in schema:
        raise ValueError('Index already exists; before capture must precede source edit')
    pinned = regression(schema, query)
    result = {'schema': schema, 'query': query,
              'source_sha256': source['source_sha256'],
              'before_checks': {'fresh_and_existing_replay': 'PASS', 'latest': pinned['worker'],
                                'equal_timestamp': pinned['tied'], 'missing_and_null': 'empty'}}
    BEFORE.write_text(json.dumps(result, indent=2) + '\n')
    print('Captured actual old source and passing full-row winner checks before edit')


def inputs():
    old = json.loads(BEFORE.read_text())
    current = sources()
    if current['queries']['box_job_for_worker'] != old['query']:
        raise ValueError('Worker query changed; this comparison permits an index only')
    schema = current['box_schema']
    if schema.count(INDEX) != 1 or schema.replace(INDEX + '\n', '') != old['schema']:
        raise ValueError('Expected exactly the one source-derived additive index')
    return old, current


def check():
    old, current = inputs()
    before = regression(old['schema'], old['query'])
    after = regression(current['box_schema'], old['query'])
    assert before == after, 'Index changed full-row winner semantics'
    with tempfile.TemporaryDirectory(prefix='arbos-job-existing-check-') as tmp:
        c = sqlite3.connect(Path(tmp) / 'existing.db')
        try:
            c.executescript(old['schema'])
            c.execute("INSERT INTO jobs(id,chat_id,ask_message_id,title,kind,brief,state,created_at) VALUES ('old', 'c', 'a', 'Title', 'task', 'Brief', 'done', 1)")
            c.commit()
            prior = c.execute('SELECT * FROM jobs').fetchall()
            c.executescript(current['box_schema'])
            c.executescript(current['box_schema'])
            assert c.execute('SELECT * FROM jobs').fetchall() == prior
            assert c.execute("SELECT sql FROM sqlite_master WHERE name='jobs_subagent_created'").fetchone()
        finally:
            c.close()
    print(json.dumps({'checks_passed': 6, 'checks': ['old fresh and replay', 'new fresh and replay',
          'exact full-row latest equivalence', 'equal timestamp first-insertion equivalence',
          'NULL and missing worker equivalence', 'existing old data upgrade and idempotent index replay']}))


def measure(repetitions):
    old, current = inputs()
    check()
    rng = random.Random(20261109)
    result = {'kind': 'paired exact worker SELECT, old schema versus actual new index schema',
              'source_before': old['source_sha256'], 'source_after': current['source_sha256'],
              'driver_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'index': INDEX, 'query': old['query'], 'seed': 20261109, 'repetitions': repetitions,
              'limits': 'Warm instrumented SQLite SQL-only comparison; no Rust/native integration or user-visible latency claim. Only index changes; ties/NULL/missing/full-row checks retained.',
              'experiments': []}
    for scale in (1, 10, 100, 1000):
        with tempfile.TemporaryDirectory(prefix='arbos-job-index-scale-') as tmp:
            old_path, new_path = Path(tmp) / 'old.db', Path(tmp) / 'new.db'
            old_source = dict(current, box_schema=old['schema'])
            fixture_before = seed_box(old_path, old_source, scale)
            fixture_after = seed_box(new_path, current, scale)
            assert fixture_before['counts'] == fixture_after['counts']
            assert old_path.stat().st_size + new_path.stat().st_size < 300 * 1024**2
            for worker in ('w_00000', f'w_{10 * scale - 1:05}', 'missing', None):
                assert rows(old_path, old['query'], worker) == rows(new_path, old['query'], worker)
            engines = {'before': Engine(old_path), 'after': Engine(new_path)}
            try:
                for worker in ('w_00000', f'w_{10 * scale - 1:05}', 'missing', None):
                    assert bundled_rows(engines['before'], old['query'], worker) == bundled_rows(engines['after'], old['query'], worker)
                samples, orders = {'before': [], 'after': []}, []
                for engine in engines.values():
                    for _ in range(5):
                        engine.run([old['query']], 'w_00000')
                for _ in range(repetitions):
                    order = ['before', 'after']
                    rng.shuffle(order)
                    orders.append(order)
                    for arm in order:
                        samples[arm].append(engines[arm].run([old['query']], 'w_00000')['elapsed_ns'])
                record = {'scale': scale, 'fixture_before': fixture_before, 'fixture_after': fixture_after,
                          'samples_ns': samples, 'order': orders, 'summary': paired(**samples),
                          'counters': {arm: engine.run([old['query']], 'w_00000', profile=True) for arm, engine in engines.items()},
                          'plans': {arm: engine.plan(old['query'], 'w_00000') for arm, engine in engines.items()}}
                # After all baseline reads, time the actual Store::open schema replay
                # on the populated old fixture, including the one-time index build.
                prior_bytes = old_path.stat().st_size
                upgrade = sqlite3.connect(old_path)
                try:
                    started = time.perf_counter()
                    upgrade.executescript(current['box_schema'])
                    record['existing_schema_upgrade_seconds'] = time.perf_counter() - started
                    assert upgrade.execute("SELECT name FROM sqlite_master WHERE name='jobs_subagent_created'").fetchone()
                finally:
                    upgrade.close()
                record['existing_index_file_growth_bytes'] = old_path.stat().st_size - prior_bytes
                result['measurement_engine'] = engines['before'].metadata
                result['experiments'].append(record)
                (HERE / 'JOB_INDEX_RESULTS.json').write_text(json.dumps(result, indent=2) + '\n')
                print(json.dumps({'scale': scale, 'jobs': fixture_before['counts']['jobs'],
                                 'before_p50_us': record['summary']['before']['p50_ns'] / 1000,
                                 'after_p50_us': record['summary']['after']['p50_ns'] / 1000}), flush=True)
            finally:
                for engine in engines.values():
                    engine.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--capture-before', action='store_true')
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--repetitions', type=int, default=101)
    args = parser.parse_args()
    if args.repetitions < 21:
        parser.error('At least 21 randomized pairs are required')
    capture() if args.capture_before else check() if args.check else measure(args.repetitions)
