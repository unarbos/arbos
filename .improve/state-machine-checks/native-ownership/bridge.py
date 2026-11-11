"""Bounded SDK-free observations of the actual Swift/Rust lifetime modules."""
from pathlib import Path
import argparse,hashlib,json,os,shutil,subprocess,sys,tempfile
HERE=Path(__file__).resolve().parent
ROOT=HERE.parents[2]
MAX=65536

def cache_path(raw):
    p=Path(raw).resolve()
    if p.parent != Path('/tmp').resolve() or not p.name.startswith('arbos-native-laws-') or not p.is_dir() or p.stat().st_uid!=os.getuid():
        raise ValueError('Invalid scratch directory')
    return p

def profile():
    home=Path.home()
    literals=[home/'.cargo/credentials',home/'.cargo/credentials.toml',home/'.git-credentials']
    dirs=[Path('/Users/Shared'),home/'.ssh',home/'.aws',home/'.config/gcloud']
    def q(p):return json.dumps(str(p))
    return '(version 1)(allow default)(deny network*)'+''.join('(deny file-read* (literal '+q(p)+'))' for p in literals)+''.join('(deny file-read* file-write* (subpath '+q(p)+'))' for p in dirs)

def sandbox(cmd):
    return ['/usr/bin/sandbox-exec','-p',profile(),*cmd] if sys.platform=='darwin' else cmd

def prepare(cache):
    swift=ROOT/'apps/ios/Arbos/Arbos/Sync/WorkLifetime.swift'
    rust=ROOT/'crates/arbos-mac/src/sync/work_lifetime.rs'
    inputs=[swift,rust,ROOT/'crates/arbos-mac/src/sync/session_scope.rs',ROOT/'crates/arbos-mac/src/sync/account_tasks.rs',HERE/'oracle.swift',HERE/'native/src/main.rs',HERE/'native/Cargo.toml',HERE/'native/Cargo.lock']
    source={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs}
    env={'PATH':os.environ.get('PATH','/usr/bin:/bin'),'HOME':str(cache),'RUSTUP_HOME':str(Path.home()/'.rustup'),'CARGO_HOME':str(Path.home()/'.cargo'),'CARGO_BUILD_JOBS':'2','CARGO_INCREMENTAL':'0'}
    commands=[['swiftc','-swift-version','5','-parse-as-library',str(swift),str(HERE/'oracle.swift'),'-o',str(cache/'swift-oracle')],['cargo','build','--offline','--locked','--manifest-path',str(HERE/'native/Cargo.toml'),'--target-dir',str(cache/'target')]]
    for i,cmd in enumerate(commands):
        r=subprocess.run(sandbox(cmd),env=env,cwd=cache,text=True,capture_output=True,timeout=120)
        (cache/f'compiler-{i}.log').write_text(r.stdout+r.stderr)
        if r.returncode:raise RuntimeError('Native oracle compilation failed')
    after={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs}
    if source!=after:raise RuntimeError('Native sources changed during compilation')
    (cache/'sources.json').write_text(json.dumps(source,sort_keys=True))
    print(json.dumps({'compiled':True,'sources':source}))

def valid(value,language):
    if not isinstance(value,dict) or type(value.get('id')) is not int or not 0<=value['id']<=9007199254740991:return False
    machine=value.get('machine')
    if machine=='snapshot':
        if language!='swift' or set(value)!={'id','machine','snapshot'}:return False
        p=value['snapshot']
        if not isinstance(p,dict) or set(p)!={'closed','foreign','disposed','sameDoc','empty','revision','latest'}:return False
        return all(type(p[k]) is bool for k in ['closed','foreign','disposed','sameDoc','empty']) and all(type(p[k]) is int and 0<=p[k]<=16 for k in ['revision','latest'])
    if machine not in ['work','matrix','ui','tasks'] or machine in ['ui','tasks'] and language!='rust' or set(value)!={'id','machine','actions'}:return False
    actions=value['actions']
    if not isinstance(actions,list) or len(actions)>64:return False
    kinds={'close','reopen','capture'} if machine=='work' else {'start','stop','ready','failed'} if machine=='matrix' else {'adopt_a','adopt_b','adopt_old','adopt_foreign','renew','clear','unbind'} if machine=='ui' else {'capture','close'}
    for a in actions:
        if not isinstance(a,dict) or a.get('kind') not in kinds:return False
        if a['kind'] in ['ready','failed']:
            if set(a)!={'kind','index'} or type(a['index']) is not int or not 0<=a['index']<64:return False
        elif set(a)!={'kind'}:return False
    return True

def serve(cache,language):
    binary=cache/'swift-oracle' if language=='swift' else cache/'target/debug/arbos-native-ownership-oracle'
    env={'PATH':'/usr/bin:/bin','HOME':str(cache)}
    child=subprocess.Popen(sandbox([str(binary)]),env=env,cwd=cache,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
    try:
        while True:
            line=sys.stdin.buffer.readline(MAX+2)
            if not line:break
            if len(line)>MAX+1 or (len(line)==MAX+1 and not line.endswith(b'\n')):
                while line and not line.endswith(b'\n'):line=sys.stdin.buffer.readline(MAX+2)
                out={'id':None,'error':'Invalid trace'}
            else:
                try:value=json.loads(line)
                except (ValueError,UnicodeDecodeError):value=None
                if not valid(value,language):out={'id':value.get('id') if isinstance(value,dict) and type(value.get('id')) is int else None,'error':'Invalid trace'}
                else:
                    child.stdin.write(json.dumps(value,separators=(',',':')).encode()+b'\n');child.stdin.flush()
                    raw=child.stdout.readline(262146)
                    if not raw or len(raw)>262145 or not raw.endswith(b'\n'):raise RuntimeError('Native oracle stopped')
                    out=json.loads(raw)
                    if out.get('id')!=value['id']:raise RuntimeError('Invalid native reply')
            print(json.dumps(out,separators=(',',':')),flush=True)
    finally:
        child.stdin.close()
        try:child.wait(timeout=1)
        except subprocess.TimeoutExpired:child.kill();child.wait(timeout=1)

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--prepare',action='store_true');ap.add_argument('--cache',required=True);ap.add_argument('--language',choices=['swift','rust']);a=ap.parse_args();cache=cache_path(a.cache)
    if a.prepare:prepare(cache)
    elif a.language:serve(cache,a.language)
    else:raise ValueError('Missing operation')
if __name__=='__main__':main()
