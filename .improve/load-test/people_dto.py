#!/usr/bin/env python3
"""Profile actual chats_list + JSON over generated local data; no service/DSN input."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import queue
import random
import re
import shutil
import signal
import subprocess
import sys
import threading
import time

HERE=Path(__file__).resolve().parent
ROOT=HERE.parents[1]
SOURCES=[f'crates/arbos-relay/src/{name}.rs'for name in ['people','db_people','db','db_accounts','api','notify','avatar']]
SCALES=[1,10,100,1000]
BASE='d499090b71f01795e1f2f99ee1ffe56db0aaa23f'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def snapshot(revision,directory):
    commit=subprocess.check_output(['git','rev-parse','--verify',revision+'^{commit}'],cwd=ROOT,text=True).strip()
    items=subprocess.check_output(['git','ls-tree','-rz','--full-tree',commit],cwd=ROOT)
    paths=[];total=0
    for item in items.split(b'\0'):
        if not item:continue
        metadata,raw=item.split(b'\t',1);mode,kind,oid=metadata.decode().split();name=raw.decode();path=Path(name)
        if path.is_absolute()or'..'in path.parts:raise RuntimeError('Unsafe tracked path')
        if any(part.startswith('.env')for part in path.parts):continue
        if kind!='blob'or mode not in ['100644','100755']:raise RuntimeError('Nonregular source entry')
        paths.append((name,mode,oid))
    directory.mkdir()
    child=subprocess.Popen(['git','cat-file','--batch'],cwd=ROOT,stdin=subprocess.PIPE,stdout=subprocess.PIPE)
    try:
        for name,mode,oid in paths:
            child.stdin.write((oid+'\n').encode());child.stdin.flush();header=child.stdout.readline().decode().split()
            if len(header)!=3 or header[0]!=oid or header[1]!='blob':raise RuntimeError('Unexpected git object')
            size=int(header[2]);total+=size
            if size>128*1024**2 or total>512*1024**2:raise RuntimeError('Source snapshot exceeds budget')
            data=child.stdout.read(size)
            if len(data)!=size or child.stdout.read(1)!=b'\n':raise RuntimeError('Truncated git blob')
            dest=directory/name;dest.parent.mkdir(parents=True,exist_ok=True);dest.write_bytes(data);dest.chmod(int(mode[-3:],8))
    finally:
        child.stdin.close();child.stdout.close()
        if child.wait(timeout=10):raise RuntimeError('Git source extraction failed')
    return commit


def fence():
    if sys.platform=='darwin':return ['/usr/bin/sandbox-exec','-p','(version 1) (allow default) (deny network*)']
    if sys.platform.startswith('linux')and shutil.which('unshare'):return ['unshare','--user','--map-root-user','--net','--']
    raise RuntimeError('A macOS Seatbelt or Linux unprivileged network namespace is required; no network-enabled fallback')


def execute(command,directory,environment,log,timeout=1800):
    start=time.monotonic()
    with log.open('w')as output:
        result=subprocess.run([*fence(),*command],cwd=directory,env=environment,stdout=output,stderr=subprocess.STDOUT,timeout=timeout)
    record={'command':command,'exit_code':result.returncode,'seconds':time.monotonic()-start}
    if result.returncode:raise RuntimeError(f'Command failed; inspect {log}')
    return record


def prepare(arm,revision,out,seed):
    directory=out/arm;directory.mkdir();tree=directory/'tree';commit=snapshot(revision,tree)
    for name in ['home','tmp','export']:(directory/name).mkdir()
    target=directory/'target'
    if seed:
        if not(seed/'debug/deps').is_dir():raise RuntimeError('Dependency seed is not a completed Cargo target')
        if sys.platform=='darwin':subprocess.run(['cp','-cR',str(seed),str(target)],check=True)
        else:shutil.copytree(seed,target)
    env={'PATH':os.environ.get('PATH','/usr/bin:/bin'),'HOME':str(directory/'home'),'TMPDIR':str(directory/'tmp'),
         'CARGO_HOME':str(Path.home()/'.cargo'),'RUSTUP_HOME':str(Path.home()/'.rustup'),
         'CARGO_TARGET_DIR':str(target),'CARGO_NET_OFFLINE':'true','CARGO_BUILD_JOBS':'2',
         'CARGO_PROFILE_DEV_DEBUG':'0','CARGO_PROFILE_TEST_DEBUG':'0','CARGO_INCREMENTAL':'0'}
    sources={name:digest(tree/name)for name in SOURCES};fragment=HERE/'people_http_profile.rs';people=tree/SOURCES[0]
    people.write_bytes(people.read_bytes()+b'\n'+fragment.read_bytes())
    compiled={name:digest(tree/name)for name in SOURCES}
    meta_command=['cargo','metadata','--locked','--offline','--no-deps','--format-version','1']
    meta=json.loads(subprocess.check_output([*fence(),*meta_command],cwd=tree,env=env))
    clean=['cargo','clean','--locked','--offline']
    for package in meta['packages']:clean+=['-p',package['name']]
    execute(clean,tree,env,directory/'clean.log')
    command=['cargo','test','--locked','--offline','-p','arbos-relay','--no-run','--message-format=json']
    build=execute(command,tree,env,directory/'build.log');binaries=[]
    for line in(directory/'build.log').read_text().splitlines():
        try:item=json.loads(line)
        except json.JSONDecodeError:continue
        if item.get('reason')=='compiler-artifact'and item.get('profile',{}).get('test')and item.get('executable')and item.get('target',{}).get('name')=='arbos-relay':binaries.append(item['executable'])
    if len(binaries)!=1 or compiled!={name:digest(tree/name)for name in SOURCES}:raise RuntimeError('Compiler source/binary provenance mismatch')
    result={'commit':commit,'sources':sources,'compiled_sources':compiled,'fragment_sha256':digest(fragment),
            'lock_sha256':digest(tree/'Cargo.lock'),'workspace_packages_cleaned':len(meta['packages']),
            'build':build,'binary':binaries[0],'source_unchanged':True}
    (directory/'BUILD.json').write_text(json.dumps(result,indent=2)+'\n')
    return directory,result


class Worker:
    def __init__(self,directory,build):
        self.directory=directory;self.build=build;self.lines=[];self.queue=queue.Queue()
        environment={'PATH':os.environ.get('PATH','/usr/bin:/bin'),'HOME':str(directory/'home'),
                     'TMPDIR':str(directory/'tmp'),'ARBOS_PEOPLE_PROFILE_EXPORT':str(directory/'export')}
        command=[*fence(),build['binary'],'--exact','people::people_http_profile::whole_chat_list','--ignored','--nocapture','--test-threads=1']
        self.child=subprocess.Popen(command,cwd=directory/'tree',env=environment,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,bufsize=1,start_new_session=True)
        def collect():
            for line in self.child.stdout:self.lines.append(line);self.queue.put(line)
            self.queue.put(None)
        self.reader=threading.Thread(target=collect,daemon=True);self.reader.start()
        try:self.until('PEOPLE_READY',120)
        except BaseException:self.close();raise

    def until(self,marker,timeout=120):
        end=time.monotonic()+timeout
        while True:
            line=self.queue.get(timeout=max(.01,end-time.monotonic()))
            if line is None:raise RuntimeError('Native fixture ended before its response; inspect runtime.log')
            if marker in line:return line.split(marker,1)[1].strip()

    def sample(self,number,scale):
        self.child.stdin.write(f'{number},{scale}\n');self.child.stdin.flush()
        result=json.loads(self.until('PEOPLE_BENCH:'))
        if result['sample']!=number or result['scale']!=scale:raise RuntimeError('Wrong native response identity')
        if result['output_bytes']>128*1024**2 or not re.fullmatch('[0-9a-f]{64}',result['output_sha256']):raise RuntimeError('Invalid native response evidence')
        return result

    def close(self):
        try:
            if self.child.poll()is None:self.child.stdin.close();self.child.wait(timeout=15)
        finally:
            if self.child.poll()is None:os.killpg(self.child.pid,signal.SIGKILL);self.child.wait(timeout=10)
            self.reader.join(timeout=5);(self.directory/'runtime.log').write_text(''.join(self.lines))
        if self.child.returncode or'test result: ok. 1 passed; 0 failed; 0 ignored'not in''.join(self.lines):raise RuntimeError('Native profiling case failed or count mismatch')
        if self.build['compiled_sources']!={name:digest(self.directory/'tree'/name)for name in SOURCES}:raise RuntimeError('Source changed during profiling')


def sql_profile(workers):
    from engine import Engine
    result=[]
    for scale in SCALES:
        row={'scale':scale}
        for arm,worker in workers.items():
            export=worker.directory/'export';queries=json.loads((export/f'queries-{scale}.json').read_text())
            if not 0<len(queries)<200000 or sum(len(q)for q in queries)>128*1024**2:raise RuntimeError('SQL capture exceeds budget')
            engine=Engine(export/f'fixture-{scale}.sqlite')
            try:
                counters=engine.run(queries,'',profile=True)
                plans={}
                for sql in queries:
                    key=re.sub(r"'(?:''|[^'])*'",'?value',sql)
                    key=re.sub(r'\b[0-9]+\b','?number',key)
                    if key not in plans:plans[key]=engine.plan(sql,'')
                row[arm]={'counters':counters,'plans':[{'query':q,'plan':plan}for q,plan in plans.items()],
                          'queries_sha256':digest(export/f'queries-{scale}.json'),'fixture_sha256':digest(export/f'fixture-{scale}.sqlite'),
                          'engine':engine.metadata}
            finally:engine.close()
        result.append(row)
    return result


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,required=True,help='fresh owned directory; databases are generated here')
    parser.add_argument('--before',default=BASE);parser.add_argument('--after',default='HEAD')
    parser.add_argument('--pairs',type=int,default=21);parser.add_argument('--dependency-seed',type=Path,help='optional inactive owned Cargo target; all workspace packages are cleaned in each clone')
    args=parser.parse_args()
    if not 21<=args.pairs<=63:parser.error('Use21–63 paired samples (the bounded native protocol permits256 requests)')
    out=args.output.resolve();out.mkdir(parents=True,exist_ok=False)
    seed=args.dependency_seed.resolve()if args.dependency_seed else None
    builds={arm:prepare(arm,revision,out,seed)for arm,revision in [('before',args.before),('after',args.after)]}
    workers={};profiles=[];samples=[]
    try:
        for arm,(directory,build)in builds.items():workers[arm]=Worker(directory,build)
        for scale in SCALES:
            pair={arm:worker.sample(-1,scale)for arm,worker in workers.items()}
            if pair['before']['output_sha256']!=pair['after']['output_sha256']:raise RuntimeError(f'Whole JSON output differs at{scale} rooms')
            profiles.append({'scale':scale,**pair})
        rng=random.Random(73011)
        for number in range(args.pairs):
            scales=SCALES.copy();rng.shuffle(scales)
            for scale in scales:
                order=['before','after'];rng.shuffle(order);pair={arm:workers[arm].sample(number,scale)for arm in order}
                expected=next(p['before']['output_sha256']for p in profiles if p['scale']==scale)
                if any(v['output_sha256']!=expected for v in pair.values()):raise RuntimeError('Whole output changed during timing')
                samples.append({'pair':number,'scale':scale,'order':order,**pair})
            print(f'pair{number+1}/{args.pairs} complete',flush=True)
        raw={'schema':1,'seed':73011,'pairs':args.pairs,'profiles':profiles,'samples':samples,
             'scope':'Actual Rust handler+JSON body; generated local relational SQLite, no HTTP transport/UI/network. Workspace opt0/dependencies opt2, warm persistent workers, trace disabled for timings.'}
        (out/'RAW.json').write_text(json.dumps(raw,indent=2)+'\n')
        (out/'SQL.json').write_text(json.dumps(sql_profile(workers),indent=2)+'\n')
        from analysis_stats import paired
        summary=[{'scale':scale,**paired([x['before']['latency_ns']for x in samples if x['scale']==scale],[x['after']['latency_ns']for x in samples if x['scale']==scale])}for scale in SCALES]
        (out/'SUMMARY.json').write_text(json.dumps(summary,indent=2)+'\n')
    finally:
        for worker in workers.values():worker.close()


if __name__=='__main__':main()
