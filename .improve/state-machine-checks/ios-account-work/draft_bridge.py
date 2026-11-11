"""Bounded SDK-free observations of the actual Swift/Rust lifetime modules."""
from pathlib import Path
import argparse,hashlib,json,os,shutil,subprocess,sys,tempfile
HERE=Path(__file__).resolve().parent
ROOT=HERE.parents[2]
MAX=65536

def cache_path(raw):
    p=Path(raw).resolve()
    if p.parent != Path('/tmp').resolve() or not p.name.startswith('arbos-draft-laws-') or not p.is_dir() or p.stat().st_uid!=os.getuid():
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
    sources=[ROOT/'apps/ios/Arbos/Arbos/Sync/WorkLifetime.swift',ROOT/'apps/ios/Arbos/Arbos/Sync/DraftSendQueue.swift',HERE/'draft_oracle.swift']
    hashes={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sources}
    env={'PATH':os.environ.get('PATH','/usr/bin:/bin'),'HOME':str(cache)}
    command=['swiftc','-swift-version','5','-parse-as-library',*[str(p) for p in sources],'-o',str(cache/'swift-oracle')]
    r=subprocess.run(sandbox(command),env=env,cwd=cache,text=True,capture_output=True,timeout=120)
    (cache/'compiler.log').write_text(r.stdout+r.stderr)
    if r.returncode:raise RuntimeError('Draft oracle compilation failed')
    if hashes!={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sources}:raise RuntimeError('Draft sources changed')
    print(json.dumps({'compiled':True,'sources':hashes}))

def valid(value,language):
    if not isinstance(value,dict) or set(value)!={'id','actions'} or type(value.get('id')) is not int or not 0<=value['id']<=9007199254740991:return False
    if not isinstance(value['actions'],list) or len(value['actions'])>64:return False
    for action in value['actions']:
        if not isinstance(action,dict):return False
        kind=action.get('kind')
        if kind=='enqueue':
            if set(action)!={'kind','account','draft','value'} or action['draft'] not in ['a','b',''] or type(action['account']) is not int or action['account'] not in [0,1] or type(action['value']) is not int or not -1000<=action['value']<=1000:return False
        elif kind=='finish':
            if set(action)!={'kind','index'} or type(action['index']) is not int or not 0<=action['index']<64:return False
        elif kind=='fresh':
            if set(action)!={'kind','account'} or type(action['account']) is not int or action['account'] not in [0,1]:return False
        elif kind=='retire':
            if set(action)!={'kind'}:return False
        else:return False
    return True

def serve(cache,language):
    binary=cache/'swift-oracle'
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
    ap=argparse.ArgumentParser();ap.add_argument('--prepare',action='store_true');ap.add_argument('--cache',required=True);ap.add_argument('--language',choices=['swift']);a=ap.parse_args();cache=cache_path(a.cache)
    if a.prepare:prepare(cache)
    elif a.language:serve(cache,a.language)
    else:raise ValueError('Missing operation')
if __name__=='__main__':main()
