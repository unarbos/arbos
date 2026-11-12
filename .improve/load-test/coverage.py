#!/usr/bin/env python3
"""Supplementary generated-local SELECT probes; no DB path or service is accepted."""
import argparse
import ctypes as C
import hashlib
import json
from pathlib import Path
import random
import re
import sqlite3
import tempfile
import time

from analysis_stats import describe
from engine import Engine
from source import ROOT, extract, raw_constant

HERE = Path(__file__).resolve().parent
SCALES = (1, 10, 100, 1000)


def function(text, name):
    match = re.search(r'\bfn ' + re.escape(name) + r'\(', text)
    if not match:
        raise ValueError(f'Missing source function {name}')
    end = re.search(r'\n(?:    )?(?:pub(?:\([^)]*\))? )?fn ', text[match.end():])
    return text[match.start():match.end() + end.start() if end else len(text)]


def select(text, name):
    queries = re.findall(r'"(SELECT [^"\\]*)"', function(text, name), re.S)
    if len(queries) != 1:
        raise ValueError(f'Expected exactly one unescaped SELECT in {name}')
    return queries[0]


def sources():
    paths = {'push': ROOT / 'crates/arbos-relay/src/db_push.rs',
             'box': ROOT / 'crates/arbos-agentd/src/store.rs'}
    texts = {key: path.read_text() for key, path in paths.items()}
    relay = extract()
    cols = re.search(r'const JOB_COLS:\s*&\x27static str\s*=\s*"([^"\\]+)";', texts['box'])
    if not cols:
        raise ValueError('Missing exact job column source constant')
    queries = {'relay_unread': select(texts['push'], 'unread_query'),
               'box_fts': select(texts['box'], 't3_search_fts'),
               'box_job_for_worker': select(texts['box'], 'job_for_subagent').replace('{}', cols[1]),
               'box_open_jobs': select(texts['box'], 'open_jobs').replace('{}', cols[1])}
    if any('{' in q or not q.startswith('SELECT ') for q in queries.values()):
        raise ValueError('Unexpanded or unsupported SELECT')
    return {'relay': relay, 'push_schema': raw_constant(texts['push'], 'PUSH_SCHEMA'),
            'box_schema': raw_constant(texts['box'], 'SCHEMA'), 'queries': queries,
            'source_sha256': {**relay['source_sha256'], **{str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in paths.values()}}}


def fresh(path):
    if path.exists():
        raise ValueError('Generated fixture requires a fresh path')
    connection = sqlite3.connect(path)
    connection.execute('PRAGMA journal_mode=OFF')
    connection.execute('PRAGMA synchronous=OFF')
    return connection


def finish(connection, path, tables):
    connection.commit()
    counts = {table: connection.execute(f'SELECT COUNT(*) FROM {table}').fetchone()[0] for table in tables}
    assert connection.execute('PRAGMA integrity_check').fetchone() == ('ok',)
    connection.close()
    if path.stat().st_size > 300 * 1024**2:
        raise RuntimeError('Generated fixture exceeds 300MiB budget')
    return {'counts': counts, 'database_bytes': path.stat().st_size}


def seed_relay(path, source, scale):
    c = fresh(path)
    relay = source['relay']
    c.executescript(relay['schema'] + relay['people_schema'] + source['push_schema'])
    for migration in relay['migrations']:
        c.execute(migration)
    c.execute(relay['project_name_index'])
    # Two active target groups, one archived; unrelated rooms grow with scale.
    users = ['u_target', 'u_blocked', 'u_peer'] + [f'u_{i}' for i in range(10 * scale)]
    c.executemany('INSERT INTO users(id,created_at) VALUES (?,1)', ((u,) for u in users))
    c.execute("INSERT INTO blocks(blocker,blocked,created_at) VALUES ('u_target','u_blocked',1)")
    c.execute("INSERT INTO chat_user_archive(user_id,chat_id,archived_at) VALUES ('u_target','g_1',1)")
    for i in range(10 * scale):
        chat, owner = f'g_{i}', f'u_{i}'
        c.execute('INSERT INTO people_chats(id,kind,created_by,created_at) VALUES (?,\x27group\x27,?,1)', (chat, owner))
        project = f'group_agent_{i}'
        c.execute('INSERT INTO projects(id,name,user_id,created_at,last_active_at) VALUES (?,?,?,1,1)', (project, f'Group agent {i}', owner))
        members = [(f'user:{owner}', 'owner'), ('user:u_peer', 'member'), ('user:u_blocked', 'member'), (f'agent:{project}', 'agent')]
        if i < 2:
            members.append(('user:u_target', 'member'))
        c.executemany('INSERT INTO people_members(chat_id,member,role,added_by,joined_at) VALUES (?,?,?,?,1)',
                      ((chat, member, role, owner) for member, role in members))
        for n in range(20):
            sender = ('u_target' if i < 2 else owner, 'u_blocked', None, 'u_peer')[n % 4]
            c.execute('INSERT INTO chat_events(chat_id,message_id,sender_user,created_at) VALUES (?,?,?,?)', (chat, f'm_{n}', sender, 1_000_000 + i * 60_000 + n * 1000))
        if i < 2:
            read = c.execute('SELECT id FROM chat_events WHERE chat_id=? AND message_id=\x27m_9\x27', (chat,)).fetchone()[0]
            c.execute('INSERT INTO chat_user_reads(user_id,chat_id,event_id,at) VALUES (\x27u_target\x27,?,?,1)', (chat, read))
    # Exercise each other ACL OR branch with two independently owned project chats.
    for i, owner in enumerate(('u_target', 'u_peer')):
        pid, chat = f'p_{i}', f'pc_{i}'
        c.execute('INSERT INTO projects(id,name,user_id,created_at,last_active_at) VALUES (?,?,?,1,1)', (pid, f'Project {i}', owner))
        c.execute('INSERT INTO chats(id,project_id,created_at) VALUES (?,?,1)', (chat, pid))
        if owner != 'u_target':
            c.execute('INSERT INTO project_members(project_id,user_id,added_by,joined_at) VALUES (?,\x27u_target\x27,?,1)', (pid, owner))
        c.execute('INSERT INTO chat_events(chat_id,project_id,message_id,sender_user,created_at) VALUES (?,?,\x27m_0\x27,\x27u_peer\x27,1)', (chat, pid))
    return finish(c, path, ('users', 'people_chats', 'people_members', 'projects', 'chats', 'project_members', 'chat_events', 'chat_user_reads', 'blocks', 'chat_user_archive'))


def seed_box(path, source, scale):
    c = fresh(path)
    c.executescript(source['box_schema'])
    # Each thread has one chat, worker, session, ten history entries and ten jobs.
    for i in range(10 * scale):
        chat, worker, session = f't_{i:05}', f'w_{i:05}', f's_{i:05}'
        c.execute('INSERT INTO chats(id,created_at) VALUES (?,1)', (chat,))
        c.execute('INSERT INTO subagents(id,name,purpose,harness,chat_id,created_at) VALUES (?,?,\x27Synthetic work\x27,\x27pi\x27,?,1)', (worker, worker, chat))
        c.execute('INSERT INTO sessions(id,subagent_id,harness,role,created_at) VALUES (?,?,\x27pi\x27,\x27worker\x27,1)', (session, worker))
        for n in range(10):
            row_id = f'm_{i:05}_{n:02}'
            kind = 'user_refused' if n == 9 else 'user' if n % 2 == 0 else 'assistant'
            text = f'Shared synthetic context for thread {i} turn {n}; history payload only.'
            c.execute('INSERT INTO t3(id,thread_id,subagent_id,session_id,kind,role,at,text,source,size) VALUES (?,?,?,?,?,?,?,? ,\x27{}\x27,?)',
                      (row_id, chat, worker, session, kind, 'user' if kind.startswith('user') else 'assistant', i * 10 + n, text, len(text)))
            c.execute('INSERT INTO t3_fts(text,id,thread_id,kind) VALUES (?,?,?,?)', (text, row_id, chat, kind))
            job = f'j_{i:05}_{n:02}'
            c.execute('INSERT INTO jobs(id,chat_id,ask_message_id,subagent_id,title,kind,brief,state,created_at) VALUES (?,?,?,?,\x27Synthetic job\x27,\x27task\x27,\x27Declared workload\x27,?,?)',
                      (job, chat, f'm_{i:05}_00', worker, 'running' if n == 9 else 'done', i * 10 + n))
            c.execute('INSERT INTO job_workers(job_id,subagent_id,at) VALUES (?,?,?)', (job, worker, i * 10 + n))
    return finish(c, path, ('chats', 'subagents', 'sessions', 't3', 't3_fts', 'jobs', 'job_workers'))


class BoundQueries:
    """Multi-parameter SELECT consumer, using the main suite's readonly engine."""
    def __init__(self, engine):
        self.e = engine
        self.lib = engine.lib
        api = {'sqlite3_prepare_v2': [C.c_void_p, C.c_char_p, C.c_int, C.POINTER(C.c_void_p), C.POINTER(C.c_char_p)],
               'sqlite3_bind_null': [C.c_void_p, C.c_int], 'sqlite3_bind_int64': [C.c_void_p, C.c_int, C.c_int64],
               'sqlite3_bind_text': [C.c_void_p, C.c_int, C.c_char_p, C.c_int, C.c_void_p],
               'sqlite3_step': [C.c_void_p], 'sqlite3_column_count': [C.c_void_p],
               'sqlite3_column_bytes': [C.c_void_p, C.c_int], 'sqlite3_finalize': [C.c_void_p],
               'sqlite3_stmt_readonly': [C.c_void_p], 'sqlite3_stmt_status': [C.c_void_p, C.c_int, C.c_int],
               'sqlite3_stmt_scanstatus': [C.c_void_p, C.c_int, C.c_int, C.c_void_p]}
        for name, args in api.items():
            getattr(self.lib, name).argtypes = args
        self.lib.sqlite3_column_text.argtypes = [C.c_void_p, C.c_int]
        self.lib.sqlite3_column_text.restype = C.c_char_p

    def run(self, sql, values, *, plan=False, profile=False):
        if not sql.startswith('SELECT ') or ';' in sql or '\x00' in sql:
            raise ValueError('Only extracted single SELECT queries are allowed')
        statement, tail = C.c_void_p(), C.c_char_p()
        started = time.perf_counter_ns()
        rc = self.lib.sqlite3_prepare_v2(self.e.db, (('EXPLAIN QUERY PLAN ' if plan else '') + sql).encode(), -1, C.byref(statement), C.byref(tail))
        if rc or not statement:
            raise RuntimeError(f'Prepare failed: {rc}')
        try:
            if not self.lib.sqlite3_stmt_readonly(statement) or tail.value:
                raise ValueError('Statement must be readonly and singular')
            for i, value in enumerate(values, 1):
                if value is None:
                    rc = self.lib.sqlite3_bind_null(statement, i)
                elif isinstance(value, int):
                    rc = self.lib.sqlite3_bind_int64(statement, i, value)
                else:
                    rc = self.lib.sqlite3_bind_text(statement, i, value.encode(), -1, C.c_void_p(-1))
                if rc:
                    raise RuntimeError(f'Bind failed: {rc}')
            rows, plans = 0, []
            while (rc := self.lib.sqlite3_step(statement)) == 100:
                rows += 1
                if plan:
                    plans.append(self.lib.sqlite3_column_text(statement, 3).decode())
                else:
                    for col in range(self.lib.sqlite3_column_count(statement)):
                        self.lib.sqlite3_column_bytes(statement, col)
            if rc != 101:
                raise RuntimeError(f'Step failed: {rc}')
            result = {'elapsed_ns': time.perf_counter_ns() - started, 'returned': rows}
            if profile:
                visits, index = 0, 0
                while True:
                    current = C.c_int64()
                    if self.lib.sqlite3_stmt_scanstatus(statement, index, 1, C.byref(current)):
                        break
                    visits += current.value
                    index += 1
                result.update(logical_row_visits=visits, vm_steps=self.lib.sqlite3_stmt_status(statement, 4, 0))
            return plans if plan else result
        finally:
            self.lib.sqlite3_finalize(statement)


def probes(source, family):
    q = source['queries']
    if family == 'relay':
        return [('relay_unread', q['relay_unread'], ('u_target', 'user:u_target'))]
    return [('box_fts_global', q['box_fts'], ('"Shared"', None, None, 30, 0, 'user_refused')),
            ('box_fts_one_thread', q['box_fts'], ('"Shared"', 't_00000', 'user', 30, 0, 'user_refused')),
            ('box_job_for_worker', q['box_job_for_worker'], ('w_00000',)),
            ('box_open_jobs', q['box_open_jobs'], ())]


def verify(path, source, family, scale):
    c = sqlite3.connect(f'file:{path}?mode=ro', uri=True)
    try:
        rows = {name: c.execute(sql, values).fetchall() for name, sql, values in probes(source, family)}
        if family == 'relay':
            assert {(row[0], row[2]) for row in rows['relay_unread']} == {('g_0', 6), ('pc_0', 1), ('pc_1', 1)}
        else:
            assert len(rows['box_fts_global']) == 30
            assert len(rows['box_fts_one_thread']) == 6
            assert all(row[1] == 't_00000' and row[4] in ('user', 'user_refused') for row in rows['box_fts_one_thread'])
            assert rows['box_job_for_worker'][0][0] == 'j_00000_09'
            assert len(rows['box_open_jobs']) == 10 * scale
        return {name: len(value) for name, value in rows.items()}
    finally:
        c.close()


def run(repetitions):
    source = sources()
    result = {'protocol': 1, 'kind': 'unchanged supplementary SELECT coverage; no before/after optimization claim',
              'repetitions': repetitions, 'seed': 20261109, 'source_sha256': source['source_sha256'],
              'driver_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'queries': source['queries'], 'experiments': [],
              'limits': 'Synthetic declared proportions; warm single-reader instrumented SQLite. Python ctypes consumes rows; absolute times exclude Rust mapping, mutex waits, native/network/CRDT. NVISIT is logical visits, not disk reads.'}
    rng = random.Random(result['seed'])
    for family, seed in (('relay', seed_relay), ('box', seed_box)):
        for scale in SCALES:
            with tempfile.TemporaryDirectory(prefix='arbos-extra-sql-') as tmp:
                path = Path(tmp) / 'generated.db'
                fixture = seed(path, source, scale)
                expected = verify(path, source, family, scale)
                engine = Engine(path)
                try:
                    consumer = BoundQueries(engine)
                    entries = probes(source, family)
                    samples = {name: [] for name, _, _ in entries}
                    for _ in range(3):
                        for _, sql, values in entries:
                            consumer.run(sql, values)
                    for _ in range(repetitions):
                        rng.shuffle(entries)
                        for name, sql, values in entries:
                            samples[name].append(consumer.run(sql, values)['elapsed_ns'])
                    record = {'family': family, 'scale': scale, 'fixture': fixture, 'verified_result_counts': expected, 'probes': {}}
                    for name, sql, values in entries:
                        counters = consumer.run(sql, values, profile=True)
                        assert counters['returned'] == expected[name]
                        record['probes'][name] = {'samples_ns': samples[name], 'summary': describe(samples[name]),
                                                 'counters': counters, 'plan': consumer.run(sql, values, plan=True)}
                    result['measurement_engine'] = engine.metadata
                    result['experiments'].append(record)
                    (HERE / 'COVERAGE_RESULTS.json').write_text(json.dumps(result, indent=2) + '\n')
                    print(json.dumps({'family': family, 'scale': scale, 'bytes': fixture['database_bytes'], 'p50_us': {name: round(data['summary']['p50_ns'] / 1000, 3) for name, data in record['probes'].items()}}), flush=True)
                finally:
                    engine.close()
    return result


def check():
    source = sources()
    checks = []
    for family, seed in (('relay', seed_relay), ('box', seed_box)):
        with tempfile.TemporaryDirectory(prefix='arbos-extra-check-') as tmp:
            path = Path(tmp) / 'generated.db'
            seed(path, source, 1)
            verify(path, source, family, 1)
            checks.append(f'{family} extracted queries and relationship/results')
            try:
                seed(path, source, 1)
            except ValueError:
                checks.append(f'{family} existing path rejected')
            else:
                raise AssertionError('Existing database path accepted')
            engine = Engine(path)
            try:
                consumer = BoundQueries(engine)
                for bad in ('DELETE FROM chats', 'SELECT 1; SELECT 2'):
                    try:
                        consumer.run(bad, ())
                    except ValueError:
                        continue
                    raise AssertionError('Non-SELECT or multiple statements accepted')
                for name, sql, values in probes(source, family):
                    actual = consumer.run(sql, values, profile=True)
                    assert actual['returned'] == verify(path, source, family, 1)[name]
                    assert consumer.run(sql, values, plan=True)
                checks.append(f'{family} readonly bound queries/plans/counters')
            finally:
                engine.close()
    print(json.dumps({'checks_passed': len(checks), 'checks': checks}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repetitions', type=int, default=41)
    parser.add_argument('--check', action='store_true', help='run small fixture/semantic/readonly regression checks only')
    args = parser.parse_args()
    if args.repetitions < 21:
        parser.error('At least 21 warm repeated samples are required')
    check() if args.check else run(args.repetitions)
