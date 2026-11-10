#!/usr/bin/env python3
"""Run bounded actual-source ownership regressions; no real SDK/provider calls."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
import time
from support import repository

HERE=Path(__file__).resolve().parent
CASES=[('check_swift.py',14),('check_api.py',5),('check_mac.py',6),
       ('check_engine.py',6),('check_relay.py',17),('check_owned_http.py',4),
       ('check_requests.py',3),('check_store_owner.py',11)]

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--tree',type=Path,default=repository(HERE))
    parser.add_argument('--record-dir',type=Path)
    parser.add_argument('--rust-only',action='store_true')
    args=parser.parse_args()
    if not args.rust_only and sys.platform!='darwin':
        parser.error('Full suite requires macOS Swift/Foundation; use --rust-only on Linux')
    results=[]
    for filename,count in CASES[2:] if args.rust_only else CASES:
        command=[sys.executable,str(HERE/filename),'--tree',str(args.tree.resolve())]
        if args.record_dir:
            command += ['--record-dir',str(args.record_dir.resolve())]
            if filename not in ['check_requests.py','check_store_owner.py']:command += ['--label',Path(filename).stem]
        start=time.monotonic()
        process=subprocess.run(command,capture_output=True,text=True,timeout=120)
        if process.returncode:
            print(process.stdout+process.stderr,file=sys.stderr)
            return 1
        result=json.loads(process.stdout)
        if result.get('compile_exit',0)!=0 or not (result.get('tests')==count if filename in ['check_requests.py','check_store_owner.py'] else re.search(rf'(?m)^RESULT tests={count} failed=0$',result['output'])):
            raise RuntimeError('Wrong or missing exact test count: '+filename)
        results.append({'file':filename,'tests':count,'wall_seconds':time.monotonic()-start,**result})
        print(f'PASS {filename}: {count} checks',flush=True)
    summary={'tests':sum(r['tests']for r in results),'failed':0,'fresh_targets':True,'results':results}
    if args.record_dir:
        args.record_dir.mkdir(parents=True,exist_ok=True)
        (args.record_dir/'SUMMARY.json').write_text(json.dumps(summary,indent=2)+'\n')
    print(f"RESULT tests={summary['tests']} failed=0")
    return 0

if __name__=='__main__':raise SystemExit(main())
