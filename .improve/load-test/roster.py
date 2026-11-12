#!/usr/bin/env python3
"""Compare actual old/new roster SQL on fresh synthetic local databases only."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import random
import sqlite3
import tempfile
import time

from analysis_stats import paired
from engine import Engine
from fixture import SCALES, seed
from source import extract

HERE = Path(__file__).resolve().parent


def sources():
    before = json.loads((HERE / 'ROSTER_BEFORE_SOURCE.json').read_text())
    after = extract()
    for key in ('schema', 'people_schema', 'migrations', 'project_name_index',
                'members_of', 'chats_of_member'):
        if before[key] != after[key]:
            raise ValueError(f'Unreviewed fixture/source change: {key}')
    if not after['batched_rosters']:
        raise ValueError('Actual source must contain the reviewed batched roster SELECT')
    return before, after


def roster_rows(connection, source, member, batched):
    chats = connection.execute(source['chats_of_member'], (member,)).fetchall()
    if not batched:
        return [(chat, connection.execute(source['members_of'], (chat[0],)).fetchall()) for chat in chats]
    rows = connection.execute(source['batched_rosters'], (member,)).fetchall()
    rosters = {chat[0]: [] for chat in chats}
    for row in rows:
        if row[0] not in rosters:
            raise AssertionError('Batched roster included a chat outside the selected scope')
        rosters[row[0]].append(row)
    return [(chat, rosters[chat[0]]) for chat in chats]


def verify(path, before, after):
    connection = sqlite3.connect(f'file:{path}?mode=ro', uri=True)
    digests = {}
    try:
        for member in ('user:u_owner', 'user:u_peer_0', 'agent:p_00001', 'user:absent'):
            old = roster_rows(connection, before, member, False)
            new = roster_rows(connection, after, member, True)
            if old != new:
                raise AssertionError('Complete ordered chat/roster SQL rows differ')
            digests[member] = hashlib.sha256(json.dumps(old, ensure_ascii=False).encode()).hexdigest()
        if connection.execute('PRAGMA integrity_check').fetchone() != ('ok',):
            raise AssertionError('Fixture integrity check failed')
    finally:
        connection.close()
    return digests


def run(repetitions=101):
    before, after = sources()
    rng = random.Random(20261109)
    queries = {'before': [before['chats_of_member'], before['members_of']],
               'after': [after['chats_of_member'], after['batched_rosters']]}
    result = {
        'protocol': 1, 'seed': 20261109, 'repetitions': repetitions,
        'platform': platform.platform(), 'python_sqlite_seed_oracle': sqlite3.sqlite_version,
        'source_before': before['source_sha256'], 'source_after': after['source_sha256'],
        'queries': queries, 'experiments': [],
        'assumptions': {
            'data': 'The original registry axis:10rooms×scale,100retained opaque rows/room, half DM2human owners/half group7humans+1agent; one declared synthetic owner sees every room.',
            'relationships': 'Original source DDL/ownership migrations/unique project names and independent project-chat IDs; SQL-only blobs/checkpoints are not valid Loro.',
            'scope': 'Warm readonly single-reader SQLite; no writers, native relay mutex wait, Rust HashMap/DTO mapping or rendering. No production proportions or app-latency claim.',
            'instrumentation': 'Same cached bundled SQLite3.46 scanstatus driver for both arms. Actual SELECT preparation, execution and column consumption included. Baseline Cdriver copies chat IDs to issue its N queries; new arm consumes2SELECTs without native mapping.',
            'rows_read': 'Logical summed SQLite NVISIT loop visits, not physical disk reads.',
        },
    }
    for scale in SCALES:
        with tempfile.TemporaryDirectory(prefix='arbos-roster-scale-') as temporary:
            path = Path(temporary) / 'synthetic.db'
            start = time.monotonic()
            fixture = seed(path, before, 'registry', scale)
            if fixture['database_bytes'] > 300 * 1024**2:
                raise RuntimeError('Fixture exceeds 300 MiB budget')
            digests = verify(path, before, after)
            fixture_seconds = time.monotonic() - start
            engine = Engine(path)
            result['measurement_engine'] = engine.metadata
            try:
                for arm in ('before', 'after'):
                    for _ in range(5):
                        engine.run(queries[arm], 'user:u_owner', registry=arm == 'before')
                samples = {'before': [], 'after': []}
                orders = []
                for _ in range(repetitions):
                    order = ['before', 'after']
                    rng.shuffle(order)
                    orders.append(order)
                    for arm in order:
                        samples[arm].append(engine.run(queries[arm], 'user:u_owner', registry=arm == 'before')['elapsed_ns'])
                counters = {arm: engine.run(sql, 'user:u_owner', registry=arm == 'before', profile=True)
                            for arm, sql in queries.items()}
                expected_rows = fixture['rooms'] + fixture['counts']['people_members']
                if counters['before']['returned'] != expected_rows or counters['after']['returned'] != expected_rows:
                    raise AssertionError('Query chain did not consume every selected chat and membership')
                if counters['before']['statements'] != 1 + fixture['rooms'] or counters['after']['statements'] != 2:
                    raise AssertionError('Statement counts differ from the actual source query budget')
                plans = {'before': [engine.plan(queries['before'][0], 'user:u_owner'), engine.plan(queries['before'][1], 'c_00001')],
                         'after': [engine.plan(query, 'user:u_owner') for query in queries['after']]}
                record = {'scale': scale, 'fixture': fixture, 'fixture_seconds': fixture_seconds,
                          'equivalence': {'complete_ordered_rows': 'PASS', 'sha256_per_member': digests},
                          'samples_ns': samples, 'order': orders, 'summary': paired(**samples),
                          'counters': counters, 'plans': plans}
                result['experiments'].append(record)
                (HERE / 'ROSTER_RESULTS.json').write_text(json.dumps(result, indent=2) + '\n')
                print(json.dumps({'scale': scale, 'rooms': fixture['rooms'], 'database_bytes': fixture['database_bytes'],
                                  'summary': record['summary'], 'counters': counters}), flush=True)
            finally:
                engine.close()
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repetitions', type=int, default=101)
    args = parser.parse_args()
    if not 21 <= args.repetitions <= 1001:
        parser.error('Use 21–1001 paired samples')
    run(args.repetitions)
