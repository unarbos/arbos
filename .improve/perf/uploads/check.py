#!/usr/bin/env python3
"""Compile actual selected source against local collaborators; no app or real relay."""
from pathlib import Path
import argparse,hashlib,json,os,platform,subprocess,tempfile,time
from extract import fixture
P=Path(__file__).resolve().parent

def main():
 ap=argparse.ArgumentParser();ap.add_argument('--source-root',type=Path,default=P.parents[2]);ap.add_argument('--work',type=Path);args=ap.parse_args()
 if platform.system()!='Darwin':raise SystemExit('This Mac regression runner requires macOS sandbox-exec for the loopback-only fence.')
 holder=None
 if args.work is None:holder=tempfile.TemporaryDirectory(prefix='arbos-upload-check-');work=Path(holder.name)
 else:
  work=args.work.resolve();work.mkdir(parents=True,exist_ok=True)
  if (work/'source').exists() or (work/'target').exists():raise SystemExit('--work must not already contain source/target; choose a fresh directory')
 source=args.source_root.resolve();fixture(source,work/'source');(work/'home').mkdir(exist_ok=True);(work/'tmp').mkdir(exist_ok=True)
 env={'PATH':os.environ['PATH'],'HOME':str(work/'home'),'CARGO_HOME':os.environ.get('CARGO_HOME',str(Path.home()/'.cargo')),'RUSTUP_HOME':os.environ.get('RUSTUP_HOME',str(Path.home()/'.rustup')),'CARGO_BUILD_JOBS':'2','CARGO_PROFILE_DEV_DEBUG':'0','FIXTURE_TMP':str(work/'tmp')}
 # A unique per-source target avoids Cargo relative-path artifact aliases between snapshots.
 cmd=['/usr/bin/sandbox-exec','-p','(version 1) (allow default) (deny network*)','cargo','build','--locked','--offline','--manifest-path',str(work/'source/Cargo.toml'),'--target-dir',str(work/'target')]
 start=time.monotonic();r=subprocess.run(cmd,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=600);(work/'build.log').write_text(r.stdout)
 if r.returncode:raise SystemExit('Offline fixture build failed; see '+str(work/'build.log'))
 results=[]
 for case in [['controls'],['redirect','path'],['redirect','copy'],['redirect','sticker'],['bounds-check','4','65536','path'],['bounds-check','4','65536','copy'],['bounds-check','4','65536','shared-sticker'],['allocation-check','1','4194304','copy']]:
  command=['/usr/bin/sandbox-exec','-f',str(P/'loopback-only.sb'),str(work/'target/debug/upload-flow-check'),*case];run=subprocess.run(command,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,timeout=15)
  if run.returncode:raise SystemExit('Upload regression failed for '+repr(case)+': '+run.stderr)
  value=json.loads(run.stdout);results.append({'arguments':case,'result':value})
 source_record=json.loads((work/'source/SOURCE.json').read_text());source_after={name:hashlib.sha256((source/name).read_bytes()).hexdigest() for name in source_record['source_files']};assert source_after==source_record['source_files'],'production source changed during regression run'
 record={'source_unchanged':True,'source_after':source_after,'seconds':time.monotonic()-start,'controls':11,'redirect_cases':3,'serial_bound_cases':3,'allocation_cases':1,'source':json.loads((work/'source/SOURCE.json').read_text()),'results':results,'scope':'Selected actual upload/fetch/send tranche plus actual classifier/ChatSendQueue; generated files and loopback-only HTTP; no SDK/media/CRDT or whole-app performance inference.'};(work/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps({k:v for k,v in record.items() if k not in ('source','results')},indent=2))
 if holder:holder.cleanup()
if __name__=='__main__':main()
