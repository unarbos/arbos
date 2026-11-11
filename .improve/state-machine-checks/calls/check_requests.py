"""Run actual native call reservation tests without GPUI/LiveKit or services."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import time


def repository():
    for root in Path(__file__).resolve().parents:
        if (root/'Cargo.lock').is_file() and (root/'crates/arbos-mac').is_dir(): return root
    raise RuntimeError('Repository root unavailable')


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tree',type=Path,default=repository())
    parser.add_argument('--record-dir',type=Path)
    args=parser.parse_args();root=args.tree.resolve();source=root/'crates/arbos-mac/src/sync/call_requests.rs'
    before=hashlib.sha256(source.read_bytes()).hexdigest();lock=(repository()/'Cargo.lock').read_text()
    def version(name):
        values=re.findall(r'\[\[package\]\]\nname = "'+re.escape(name)+r'"\nversion = "([^"]+)"',lock)
        if len(values)!=1:raise RuntimeError('Ambiguous locked dependency '+name)
        return values[0]
    start=time.monotonic()
    with tempfile.TemporaryDirectory(prefix='arbos-call-order-') as directory:
        work=Path(directory)
        manifest='[workspace]\n[package]\nname="call-request-order-check"\nversion="0.0.0"\nedition="2021"\n[lib]\npath='+json.dumps(str(source))+'\n[dependencies]\nfutures="='+version('futures')+'"\ntokio={version="='+version('tokio')+'",features=["sync","rt","macros"]}\n[profile.dev]\ndebug=0\nincremental=false\n'
        (work/'Cargo.toml').write_text(manifest)
        env={'PATH':os.environ['PATH'],'HOME':str(work),'TMPDIR':str(work),'CARGO_HOME':str(Path.home()/'.cargo'),'RUSTUP_HOME':str(Path.home()/'.rustup'),'CARGO_NET_OFFLINE':'true','CARGO_TARGET_DIR':str(work/'target'),'CARGO_BUILD_JOBS':'2'}
        command=['cargo','test','--offline','--manifest-path',str(work/'Cargo.toml'),'--lib','--','--test-threads=1']
        guarded=(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* (subpath "/Users/Shared"))'] if sys.platform=='darwin' else [])+command
        run=subprocess.run(guarded,cwd=work,env=env,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=90)
        after=hashlib.sha256(source.read_bytes()).hexdigest()
        row={'exit_code':run.returncode,'seconds':time.monotonic()-start,'source_sha256':before,'source_unchanged':before==after,'tests':3,'output':run.stdout}
        if args.record_dir:
            args.record_dir.mkdir(parents=True,exist_ok=True)
            (args.record_dir/'requests.json').write_text(json.dumps(row,indent=2)+'\n');(args.record_dir/'requests.log').write_text(run.stdout)
        print(json.dumps(row))
        return 0 if run.returncode==0 and before==after and '3 passed; 0 failed' in run.stdout else 1

if __name__=='__main__':raise SystemExit(main())
