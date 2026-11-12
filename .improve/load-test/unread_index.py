#!/usr/bin/env python3
"""Fresh-local unread index tradeoff probes; no existing DB/SQL/DSN is accepted."""
import argparse
import ctypes as C
import json
from pathlib import Path
import random
import re
import sqlite3
import tempfile
import time

from analysis_stats import paired
from coverage import BoundQueries, seed_relay, sources
from engine import Engine

HERE = Path(__file__).resolve().parent
SCALES = (1, 10, 100, 1000)


def inputs():
    before = json.loads((HERE / 'UNREAD_BEFORE_SOURCE.json').read_text())
    # The candidate was measured but withheld: reproduce it only on fresh fixtures.
    after = json.loads((HERE / 'UNREAD_CANDIDATE_SOURCE.json').read_text())
    current = sources()
    if current['queries']['relay_unread'] != before['queries']['relay_unread']:
        raise ValueError('Current source unread query changed; review the experiment')
    if current['push_schema'] not in (before['push_schema'], after['push_schema']):
        raise ValueError('Current source schema differs from the reviewed experiment')
    if before['queries']['relay_unread'] != after['queries']['relay_unread']:
        raise ValueError('Unread SELECT must remain byte-identical')
    found = re.findall(r'CREATE INDEX IF NOT EXISTS chat_events_project[^;]+;', after['push_schema'])
    if len(found) != 1 or after['push_schema'].replace(found[0] + '\n', '') != before['push_schema']:
        raise ValueError('Expected exactly the reviewed additive source index')
    for key in ('schema', 'people_schema', 'migrations', 'project_name_index'):
        if before['relay'][key] != after['relay'][key]:
            raise ValueError('Unreviewed fixture schema change')
    return before, after, found[0]


def decorate(path, scale, legacy, broad):
    with sqlite3.connect(path) as connection:
        connection.execute('PRAGMA synchronous=OFF')
        for i in range(10 * scale):
            group, owner, project = f'g_{i}', f'u_{i}', f'group_agent_{i}'
            if broad:
                connection.execute("INSERT OR IGNORE INTO people_members(chat_id,member,role,added_by,joined_at) VALUES (?,'user:u_target','member',?,1)", (group, owner))
            if legacy:
                chat = f'legacy_{i}'
                connection.execute('INSERT INTO chats(id,project_id,created_at) VALUES (?,?,1)', (chat, project))
                for guest in ['u_peer', 'u_blocked'] + (['u_target'] if broad else []):
                    connection.execute("INSERT INTO project_members(project_id,user_id,role,added_by,joined_at) VALUES (?,?,'member',?,1)", (project, guest, owner))
                for n in range(20):
                    sender = (owner, 'u_blocked', None, 'u_peer')[n % 4]
                    connection.execute('INSERT INTO chat_events(chat_id,project_id,message_id,sender_user,created_at) VALUES (?,?,?,?,?)', (chat, project, f'legacy_m_{n}', sender, 2_000_000 + i * 60_000 + n * 1000))
        connection.commit()
        return {name: connection.execute(f'SELECT COUNT(*) FROM {name}').fetchone()[0]
                for name in ('chat_events', 'people_chats', 'people_members', 'projects', 'chats', 'project_members')}


def rows(engine, sql, values):
    BoundQueries(engine)  # installs inspected parameter/step/finalize prototypes
    lib = engine.lib
    lib.sqlite3_column_int64.argtypes = [C.c_void_p, C.c_int]
    lib.sqlite3_column_int64.restype = C.c_int64
    statement, tail = C.c_void_p(), C.c_char_p()
    rc = lib.sqlite3_prepare_v2(engine.db, sql.encode(), -1, C.byref(statement), C.byref(tail))
    if rc or not statement:
        raise RuntimeError('Full-row oracle prepare failed')
    try:
        for i, value in enumerate(values, 1):
            if lib.sqlite3_bind_text(statement, i, value.encode(), -1, C.c_void_p(-1)):
                raise RuntimeError('Full-row oracle bind failed')
        out = []
        while (rc := lib.sqlite3_step(statement)) == 100:
            chat, project = lib.sqlite3_column_text(statement, 0), lib.sqlite3_column_text(statement, 1)
            out.append((chat.decode(), project.decode() if project is not None else None,
                        lib.sqlite3_column_int64(statement, 2)))
        if rc != 101:
            raise RuntimeError('Full-row oracle step failed')
        return out
    finally:
        lib.sqlite3_finalize(statement)


def fixture_pair(temporary, before, index, scale, legacy, broad):
    paths, metadata = {}, {}
    for arm in ('before', 'after'):
        path = Path(temporary) / (arm + '.db')
        seeded = seed_relay(path, before, scale)
        counts = decorate(path, scale, legacy, broad)
        old_bytes = path.stat().st_size
        build_ns = 0
        if arm == 'after':
            with sqlite3.connect(path) as connection:
                start = time.perf_counter_ns()
                connection.execute(index)
                connection.commit()
                build_ns = time.perf_counter_ns() - start
        paths[arm] = path
        metadata[arm] = {'counts': counts, 'database_bytes': path.stat().st_size,
                         'index_added_bytes': path.stat().st_size - old_bytes,
                         'index_build_ns_single_observation': build_ns,
                         'seed_counts': seeded['counts']}
    if sum(path.stat().st_size for path in paths.values()) > 300 * 1024**2:
        raise RuntimeError('Fixture pair exceeds 300 MiB budget')
    return paths, metadata


def run(repetitions=101, check=False):
    before, after, index = inputs()
    query = before['queries']['relay_unread']
    args = ('u_target', 'user:u_target')
    rng = random.Random(20261109)
    result = {'seed': 20261109, 'repetitions': repetitions, 'query': query, 'index': index,
              'source_before': before['source_sha256'], 'source_after': after['source_sha256'],
              'seed_and_index_creation_sqlite': sqlite3.sqlite_version, 'experiments': [],
              'assumptions': {'visibility': 'Sparse sees two people groups (one archived) and two project chats. Broad additionally joins every group and, for mixed history, every legacy project.',
                              'mix': 'People history20events/group; mixed adds one valid legacy project chat with20events per group-agent project, making about half events project-tagged.',
                              'read_filtering': 'Shared original read marks, own/blocked/NULL senders, archived room and owner/guest OR scopes; timestamps span under seven days. Proportions are declared synthetic, not observed production.',
                              'measurement': 'Warm readonly single-reader bundled SQLite with scanstatus. Python ctypes preparation/binding/column consumption included; native mapping/mutex/writers/network/UI excluded. Two fresh identical fixtures per arm; all deleted afterward.'}}
    for legacy, broad in ((False, False), (False, True), (True, False), (True, True)):
        case = ('mixed' if legacy else 'people') + ('_broad' if broad else '_sparse')
        for scale in ((1,) if check else SCALES):
            with tempfile.TemporaryDirectory(prefix='arbos-unread-scale-') as temporary:
                paths, metadata = fixture_pair(temporary, before, index, scale, legacy, broad)
                engines = {arm: Engine(path) for arm, path in paths.items()}
                try:
                    expected = rows(engines['before'], query, args)
                    if expected != rows(engines['after'], query, args):
                        raise AssertionError('Bundled SQLite full ordered rows changed')
                    for arm, path in paths.items():
                        with sqlite3.connect(f'file:{path}?mode=ro', uri=True) as connection:
                            if expected != connection.execute(query, args).fetchall():
                                raise AssertionError('Independent seed/oracle SQLite rows differ')
                            if connection.execute('PRAGMA integrity_check').fetchone() != ('ok',):
                                raise AssertionError('Fixture integrity failed')
                    if not broad and [(row[0], row[2]) for row in expected] != [('pc_1', 1), ('pc_0', 1), ('g_0', 6)]:
                        raise AssertionError('Sparse ACL/read/blocked/archive controls differ')
                    if broad and len(expected) != (20 if legacy else 10) * scale + 1:
                        raise AssertionError('Broad target did not see every declared live group/project')
                    if check:
                        continue
                    bound = {arm: BoundQueries(engine) for arm, engine in engines.items()}
                    for arm in bound:
                        for _ in range(5):
                            bound[arm].run(query, args)
                    samples, order = {'before': [], 'after': []}, []
                    for _ in range(repetitions):
                        arms = ['before', 'after']; rng.shuffle(arms); order.append(arms)
                        for arm in arms:
                            samples[arm].append(bound[arm].run(query, args)['elapsed_ns'])
                    record = {'case': case, 'scale': scale, 'fixtures': metadata, 'full_ordered_rows': 'PASS both SQLite versions',
                              'result_rows': len(expected), 'samples_ns': samples, 'order': order,
                              'summary': paired(**samples),
                              'counters': {arm: runner.run(query, args, profile=True) for arm, runner in bound.items()},
                              'plans': {arm: runner.run(query, args, plan=True) for arm, runner in bound.items()}}
                    result['measurement_engine'] = engines['before'].metadata
                    result['experiments'].append(record)
                    (HERE / 'UNREAD_INDEX_RESULTS.json').write_text(json.dumps(result, indent=2) + '\n')
                    print(json.dumps({'case': case, 'scale': scale, 'summary': record['summary'], 'rows': len(expected)}), flush=True)
                finally:
                    for engine in engines.values():
                        engine.close()
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--repetitions', type=int, default=101)
    options = parser.parse_args()
    if not 21 <= options.repetitions <= 1001:
        parser.error('Use 21–1001 paired samples')
    run(options.repetitions, options.check)
