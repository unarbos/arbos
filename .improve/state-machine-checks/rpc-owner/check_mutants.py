"""Negative controls for actual pure PiSlot functions; temporary trees only."""
from pathlib import Path
import argparse,hashlib,json,os,re,shutil,subprocess,tempfile,time,base64
HERE=Path(__file__).resolve().parent;ROOT=HERE.parents[2]
PRODUCT='crates/arbos-agentd/src/pi_slot.rs';LAW='harness/arbos-pi-ext/rpc-owner-laws.test.ts';BRIDGE='.improve/state-machine-checks/pi_slot_bridge.rs'
CASES=[
 ('stale-abort-retires-replacement',91001,'''if !matches!(self, Self::Starting { ticket: current } | Self::Checking { ticket: current, .. } if *current == ticket)\n                {\n                    return Action::Rejected(None);\n                }''','''if false { return Action::Rejected(None); }'''),
 ('shutdown-can-reopen',91002,'if self.is_stopped() {','if false {'),
 ('ready-resource-is-lost',91003,'''Self::Checking { child, .. }\n            | Self::Ready { child, .. }\n            | Self::Exited { child, .. } => Some(child),''','''Self::Checking { child, .. } | Self::Exited { child, .. } => Some(child),\n            Self::Ready { .. } => None,'''),
]
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--output',type=Path,required=True);a=ap.parse_args();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 files=[PRODUCT,LAW,BRIDGE,'harness/arbos-pi-ext/package.json'];hashes={f:hashlib.sha256((ROOT/f).read_bytes()).hexdigest()for f in files};rows=[]
 # The unmodified controller must pass before any negative control is meaningful.
 control=subprocess.run(['node','--test',str(ROOT/LAW)],cwd=ROOT,capture_output=True,text=True,timeout=60)
 (out/'control.log').write_text(control.stdout+control.stderr)
 assert control.returncode==0 and '# tests 4' in control.stdout and '# pass 4' in control.stdout
 for name,seed,before,after in CASES:
  with tempfile.TemporaryDirectory(prefix='arbos-pi-slot-mutant-')as raw:
   tree=Path(raw)
   for f in files:
    target=tree/f;target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(ROOT/f,target)
   (tree/'harness/arbos-pi-ext/node_modules').symlink_to((ROOT/'harness/arbos-pi-ext/node_modules').resolve())
   product=tree/PRODUCT;s=product.read_text();assert s.count(before)==1,(name,'stale anchor');product.write_text(s.replace(before,after,1))
   row={'name':name,'seed':seed,'original_sha256':hashes[PRODUCT],'mutant_sha256':hashlib.sha256(product.read_bytes()).hexdigest()}
   for replay in [False,True]:
    env={'PATH':os.environ.get('PATH','/usr/bin:/bin'),'ARBOS_RPC_LAW':str(seed)}
    if replay:env['ARBOS_RPC_REPLAY']=json.dumps(row['shrunk_input'],separators=(',',':'))
    start=time.monotonic();r=subprocess.run(['node','--test',str(tree/LAW)],cwd=tree,env=env,capture_output=True,text=True,timeout=60)
    log=(r.stdout+r.stderr).replace(str(tree),'<isolated-tree>');key='replay'if replay else'generated';(out/f'{name}-{key}.log').write_text(log)
    detected=r.returncode!=0 and ('Exact counterexample replay'in log if replay else'pi-slot-counterexample:'in log and'Property falsified'in log)
    row[key]={'exit_code':r.returncode,'seconds':time.monotonic()-start,'detected':detected}
    assert detected,(name,key,log[-4000:])
    if not replay:
     encoded=re.search(r'pi-slot-counterexample:([A-Za-z0-9+/=]+)',log).group(1);e=json.loads(base64.b64decode(encoded));row['shrunk_input']=e['result']['shrunkInput'];row['generated']['accepted_shrinks']=1 if '1 shrink(s)'in log else 0
     assert row['shrunk_input']=={'tail':[]},e
   rows.append(row)
 assert hashes=={f:hashlib.sha256((ROOT/f).read_bytes()).hexdigest()for f in files}
 record={'unmodified_tests_passed':4,'generated_law_cases':768,'protocol_test':1,'original_sources':hashes,'source_unchanged':True,'results':rows};(out/'RESULTS.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps({'mutants':len(rows),'generated_detected':len(rows),'replay_detected':len(rows),'source_unchanged':True}))
if __name__=='__main__':main()
