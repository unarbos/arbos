#!/usr/bin/env python3
"""Local synthetic SQLite profiling. No DB paths, DSNs, services or SQL are accepted."""
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
from source import ROOT, extract

HERE=Path(__file__).resolve().parent


def read_position(c,queries,room):
    results=[c.execute(sql,(room,)).fetchall() for sql in queries]
    return tuple(results[0][0]) if results[0] else (0,0)


def verify(path,before,after,fixture):
    # Read-only Python oracle compares values/defaults separately from timing engine.
    c=sqlite3.connect(f'file:{path}?mode=ro',uri=True)
    try:
        for room in ('missing','people/c_00000','people/c_00001'):
            assert read_position(c,before['room_state'],room)==read_position(c,[after['room_position']],room)
        expected=(fixture['rows_per_room']*5//4,fixture['rows_per_room']//4)
        assert read_position(c,[after['room_position']],'people/c_00001')==expected
        assert c.execute('SELECT MIN(seq)>? FROM rows WHERE room=?',(expected[1],'people/c_00001')).fetchone()==(1,)
        assert c.execute("SELECT COUNT(*) FROM people_members m JOIN people_chats c ON c.id=m.chat_id WHERE c.kind='dm' AND m.member NOT LIKE 'user:%'").fetchone()==(0,)
        assert c.execute('PRAGMA integrity_check').fetchone()==('ok',)
    finally:
        c.close()


def run(repetitions=101, control_repetitions=21):
    before=json.loads((HERE/'before-source.json').read_text()); after=extract()
    if not after['room_position'] or before['room_state'][0]!=after['room_position']:
        raise ValueError('Actual new position SQL must equal the baseline position query')
    for k in ('schema','people_schema','migrations','project_name_index','members_of','chats_of_member'):
        if before[k]!=after[k]: raise ValueError(f'Unreviewed source fixture change: {k}')
    results={'protocol':1,'seed':20261109,'repetitions':repetitions,'control_repetitions':control_repetitions,
             'python_sqlite_seed_and_oracle':sqlite3.sqlite_version,'platform':platform.platform(),
             'source_before':before['source_sha256'],'source_after':after['source_sha256'],
             'assumptions': {'axes':'History fixes10rooms and scales retainedrows/room100×scale; registry scales10rooms×scale with100retainedrows/room. Each has1000×scale total retainedrows.',
              'rosters':'Half DM two human owners; half groups owner+six human peers+one agent. One synthetic owner belongs to every room (intentional unbounded registry stress),7users total.',
              'payload':'65-byte opaque synthetic blobs and synthetic checkpoints, not valid Loro. SQL-only, not CRDT or HTTP/native integration.',
              'proportions':'Declared deterministic assumptions; no production measurements available. Project/chat registries are related by owner but independent chat IDs.',
              'cache':'Warm single-reader local SQLite; no writers, network, relay mutex wait or device rendering.',
              'rows_read':'Actual summed SQLite NVISIT loop counters across executed source SELECTs; logical visits, not physical disk reads. VM steps separately reported.'},
             'experiments':[]}
    rng=random.Random(results['seed'])
    for experiment in ('history','registry'):
        for scale in SCALES:
            with tempfile.TemporaryDirectory(prefix='arbos-synthetic-scale-') as tmp:
                path=Path(tmp)/'synthetic.db'; start=time.perf_counter()
                fixture=seed(path,before,experiment,scale); verify(path,before,after,fixture)
                e=Engine(path); results['measurement_engine']=e.metadata
                try:
                    record={'fixture':fixture,'fixture_seconds':round(time.perf_counter()-start,3),'equivalence':'PASS: missing default and both DM/group positions; integrity,compactedrow,DMroster checks','queries':{}}
                    for kind,room in (('dm','people/c_00000'),('group','people/c_00001')):
                        arms={'before':before['room_state'],'after':[after['room_position']]}
                        for arm,queries in arms.items():
                            for _ in range(5): e.run(queries,room)
                        samples={'before':[],'after':[]}; orders=[]
                        for _ in range(repetitions):
                            order=['before','after']; rng.shuffle(order); orders.append(order)
                            for arm in order: samples[arm].append(e.run(arms[arm],room)['elapsed_ns'])
                        record['queries']['room_position_'+kind]={'samples_ns':samples,'order':orders,'summary':paired(**samples),
                            'counters':{arm:e.run(queries,room,profile=True) for arm,queries in arms.items()},
                            'plans':{arm:[e.plan(q,room) for q in queries] for arm,queries in arms.items()}}
                    control_queries=[before['chats_of_member'],before['members_of']]
                    for _ in range(3): e.run(control_queries,'user:u_owner',registry=True)
                    samples={'before':[],'after':[]}; orders=[]
                    for _ in range(control_repetitions):
                        order=['before','after']; rng.shuffle(order); orders.append(order)
                        for arm in order: samples[arm].append(e.run(control_queries,'user:u_owner',registry=True)['elapsed_ns'])
                    record['queries']['registry_n_plus_one_unchanged']={'samples_ns':samples,'order':orders,'summary':paired(**samples),'unchanged_control':True,
                        'counters':e.run(control_queries,'user:u_owner',registry=True,profile=True),
                        'plans':[e.plan(control_queries[0],'user:u_owner'),e.plan(control_queries[1],'c_00001')]}
                    results['experiments'].append(record)
                    (HERE/'RESULTS.json').write_text(json.dumps(results,indent=2)+'\n')
                    print(json.dumps({'experiment':experiment,'scale':scale,'bytes':fixture['database_bytes'],'rows':fixture['counts']['rows'], 'room_group_p50_ns':record['queries']['room_position_group']['summary']['before']['p50_ns'],'registry_p50_ns':record['queries']['registry_n_plus_one_unchanged']['summary']['before']['p50_ns']}),flush=True)
                finally:
                    e.close()
    return results


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--repetitions',type=int,default=101)
    p.add_argument('--control-repetitions',type=int,default=21)
    args=p.parse_args()
    if args.repetitions<21 or args.control_repetitions<5: p.error('At least21paired samples and5control pairs required')
    run(args.repetitions,args.control_repetitions)
