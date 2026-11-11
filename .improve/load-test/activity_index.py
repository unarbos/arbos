#!/usr/bin/env python3
"""Profile actual room-activity SELECTs and explicit index costs on generated data."""
from pathlib import Path
import argparse
import ctypes as C
import hashlib
import json
import random
import shutil
import sqlite3
import tempfile
import time
from analysis_stats import paired
from engine import Engine
from fixture import seed
from source import ROOT,body,extract,raw_constant,selects
HERE=Path(__file__).resolve().parent
INDEX='CREATE INDEX IF NOT EXISTS rows_room_activity ON rows(room, created_at);'
APPEND="BEGIN IMMEDIATE; UPDATE room_seq SET head=head+1 WHERE room='people/c_00001'; INSERT INTO rows(room,seq,device,batch_id,bytes,created_at) SELECT room,head,'d_0','extra_'||head,zeroblob(65),head FROM room_seq WHERE room='people/c_00001'; COMMIT;"


def inputs():
    before=json.loads((HERE/'ACTIVITY_BEFORE_SOURCE.json').read_text());current=extract()
    if current['schema'].count(INDEX)!=1 or current['schema'].replace(INDEX+'\n','')!=before['schema']:
        raise ValueError('Expected only the reviewed additive activity index in SCHEMA')
    queries=selects(body((ROOT/'crates/arbos-relay/src/db_people.rs').read_text(),'room_activity'))
    if queries!=before['queries']or len(queries)!=2:raise ValueError('Actual room activity query changed; review its baseline explicitly')
    return before,current,queries


def activity(connection,queries,room):
    head=connection.execute(queries[0],(room,)).fetchone()
    return(head[0]if head else 0,connection.execute(queries[1],(room,)).fetchone()[0])


def check():
    before,current,queries=inputs()
    with tempfile.TemporaryDirectory(prefix='arbos-activity-check-')as raw:
        path=Path(raw)/'generated.sqlite';seed(path,dict(current,schema=before['schema']),'history',1)
        c=sqlite3.connect(path)
        try:
            c.execute("UPDATE rows SET created_at=-seq WHERE room='people/c_00001'")
            c.execute("UPDATE rows SET created_at=999999 WHERE room='people/c_00001' AND seq=26")
            expected={room:activity(c,queries,room)for room in ['missing','people/c_00000','people/c_00001']}
            assert expected['missing']==(0,0)and expected['people/c_00001']==(125,999999)
            prior=list(c.execute('SELECT room,seq,device,batch_id,bytes,created_at FROM rows ORDER BY room,seq'))
            c.executescript(current['schema']);c.executescript(current['schema'])
            assert prior==list(c.execute('SELECT room,seq,device,batch_id,bytes,created_at FROM rows ORDER BY room,seq'))
            assert expected=={room:activity(c,queries,room)for room in expected}
            c.execute("DELETE FROM rows WHERE room='people/c_00001' AND seq=26")
            assert activity(c,queries,'people/c_00001')==(125,-27)
            c.execute("DELETE FROM rows WHERE room='people/c_00001'")
            assert activity(c,queries,'people/c_00001')==(125,0)
            c.commit();assert c.execute('PRAGMA integrity_check').fetchone()==('ok',)
        finally:c.close()
    return {'passed':6,'checks':['missing defaults','nonmonotonic timestamp maximum','exact row preservation through schema replay','idempotent index replay','maximum deletion and negative timestamp','empty retained history with preserved head/integrity']}


class Writer:
    """Only called on freshly generated owned paths, through root-lock SQLite."""
    def __init__(self,path):
        engine=Engine(path);self.lib=engine.lib;self.metadata=engine.metadata;engine.close();self.db=C.c_void_p()
        self.lib.sqlite3_open_v2.argtypes=[C.c_char_p,C.POINTER(C.c_void_p),C.c_int,C.c_char_p]
        self.lib.sqlite3_exec.argtypes=[C.c_void_p,C.c_char_p,C.c_void_p,C.c_void_p,C.c_void_p]
        if self.lib.sqlite3_open_v2(str(path).encode(),C.byref(self.db),2,None):raise RuntimeError('Generated writer open failed')
        self.execute('PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA secure_delete=ON;')
    def execute(self,sql):
        code=self.lib.sqlite3_exec(self.db,sql.encode(),None,None,None)
        if code:raise RuntimeError(f'Generated SQLite write failed:{code}')
    def close(self):
        if self.db:
            code=self.lib.sqlite3_close(self.db);self.db=None
            if code:raise RuntimeError('Generated SQLite writer close failed')


def write_cost(before,current,pairs):
    with tempfile.TemporaryDirectory(prefix='arbos-activity-writes-')as raw:
        d=Path(raw);a=d/'before.sqlite';b=d/'after.sqlite';fixture=seed(a,dict(current,schema=before['schema']),'history',1000);shutil.copyfile(a,b)
        c=sqlite3.connect(b);c.execute(INDEX);c.commit();c.close()
        writers={};samples={'before':[],'after':[]};orders=[];rng=random.Random(73102)
        try:
            for arm,path in [('before',a),('after',b)]:writers[arm]=Writer(path)
            for _ in range(pairs):
                order=list(writers);rng.shuffle(order);orders.append(order)
                for arm in order:
                    start=time.perf_counter_ns()
                    for _ in range(500):writers[arm].execute(APPEND)
                    samples[arm].append((time.perf_counter_ns()-start)/500)
            return {'fixture':fixture,'samples_ns':samples,'order':orders,'summary':paired(**samples),'engine':writers['before'].metadata,
                    'sql':APPEND,'scope':'Approximate SQL append transaction cost:500one-rowWAL/NORMAL/secure_delete transactions per pair arm; Python dispatch+SQLprepare included. Not real Db.append/Loro/HTTP/application latency.'}
        finally:
            for writer in writers.values():writer.close()


def measure(pairs=31,write_pairs=21):
    before,current,queries=inputs();regression=check();records=[];rng=random.Random(73101)
    for scale in [1,10,100,1000]:
        with tempfile.TemporaryDirectory(prefix='arbos-activity-scale-')as raw:
            d=Path(raw);a=d/'before.sqlite';b=d/'after.sqlite';fixture=seed(a,dict(current,schema=before['schema']),'history',scale);shutil.copyfile(a,b)
            c=sqlite3.connect(b);start=time.perf_counter();c.executescript(current['schema']);create=time.perf_counter()-start;c.close()
            engines={'before':Engine(a),'after':Engine(b)};samples={'before':[],'after':[]};orders=[]
            try:
                for engine in engines.values():
                    for _ in range(3):engine.run(queries,'people/c_00001')
                for _ in range(pairs):
                    order=list(engines);rng.shuffle(order);orders.append(order)
                    for arm in order:samples[arm].append(engines[arm].run(queries,'people/c_00001')['elapsed_ns'])
                record={'scale':scale,'fixture':fixture,'samples_ns':samples,'order':orders,'summary':paired(**samples),
                        'counters':{arm:e.run(queries,'people/c_00001',profile=True)for arm,e in engines.items()},
                        'plans':{arm:[e.plan(q,'people/c_00001')for q in queries]for arm,e in engines.items()},
                        'schema_replay_seconds_python_sqlite':create,'index_extra_bytes':b.stat().st_size-a.stat().st_size}
                records.append(record);print(json.dumps({'scale':scale,'before_p50_ns':record['summary']['before']['p50_ns'],'after_p50_ns':record['summary']['after']['p50_ns']}),flush=True)
            finally:
                for engine in engines.values():engine.close()
    result={'query':queries,'index':INDEX,'before_source':before['source_sha256'],'after_source':current['source_sha256'],
            'driver_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'checks':regression,'pairs':pairs,'write_pairs':write_pairs,
            'sqlite_seed_and_schema_replay_version':sqlite3.sqlite_version,'experiments':records,'write_cost':write_cost(before,current,write_pairs),
            'limits':'SQL-only warm instrumented root-lock SQLite3.46; real NVISIT and plans, no Rust/HTTP/Loro/network/UI claim. Every path is newly generated, no DSN accepted. Write cost is approximate SQL, not native append. One-time schema replay/index growth is measured separately with Python SQLite.'}
    (HERE/'ACTIVITY_RESULTS.json').write_text(json.dumps(result,indent=2)+'\n');return result


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--check',action='store_true');parser.add_argument('--pairs',type=int,default=31);parser.add_argument('--write-pairs',type=int,default=21);args=parser.parse_args()
    if args.pairs<21 or args.write_pairs<21:parser.error('At least21 pairs per comparison are required')
    if args.check:print(json.dumps(check()))
    else:measure(args.pairs,args.write_pairs)
