"""Negative controls for actual pure worker registry functions; temporary trees only."""
from pathlib import Path
import argparse,hashlib,json,os,re,shutil,subprocess,tempfile,time,base64
HERE=Path(__file__).resolve().parent;ROOT=HERE.parents[1]
PRODUCT='crates/arbos-agentd/src/worker_lifetime.rs';LAW='harness/arbos-pi-ext/worker-lifetime-laws.test.ts';BRIDGE='.improve/state-machine-checks/worker_lifetime_bridge.rs'
CASES=[('shutdown-reopens', 92001, '        let Self::Accepting { next, entries } = self else {', '        if matches!(self, Self::Closed) && matches!(&event, Event::Begin(_)) { *self = Self::default(); }\n        let Self::Accepting { next, entries } = self else {'), ('stale-abort-retires-successor', 92002, 'Some(Entry::Starting { ticket: current, .. }) if *current == ticket)', 'Some(Entry::Starting { .. }))'), ('stale-stop-completion-releases-successor', 92003, 'Some(Entry::Stopping { ticket: current, .. }) if *current == ticket)', 'Some(Entry::Stopping { .. }))'), ('captured-unpublished-resource-lost', 92004, 'Self::Starting { resource, .. } | Self::Stopping { resource, .. } => resource,', 'Self::Starting { .. } => None,\n            Self::Stopping { resource, .. } => resource,')]
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--output',type=Path,required=True);a=ap.parse_args();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 files=[PRODUCT,LAW,BRIDGE,'harness/arbos-pi-ext/package.json'];hashes={f:hashlib.sha256((ROOT/f).read_bytes()).hexdigest()for f in files};rows=[]
 # The unmodified controller must pass before any negative control is meaningful.
 control=subprocess.run(['node','--test',str(ROOT/LAW)],cwd=ROOT,env={'PATH':os.environ.get('PATH','/usr/bin:/bin')},capture_output=True,text=True,timeout=60)
 (out/'control.log').write_text(control.stdout+control.stderr)
 assert control.returncode==0 and '# tests 5' in control.stdout and '# pass 5' in control.stdout
 for name,seed,before,after in CASES:
  with tempfile.TemporaryDirectory(prefix='arbos-worker-mutant-')as raw:
   tree=Path(raw)
   for f in files:
    target=tree/f;target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(ROOT/f,target)
   (tree/'harness/arbos-pi-ext/node_modules').symlink_to((ROOT/'harness/arbos-pi-ext/node_modules').resolve())
   product=tree/PRODUCT;s=product.read_text();assert s.count(before)==1,(name,'stale anchor');product.write_text(s.replace(before,after,1))
   row={'name':name,'seed':seed,'original_sha256':hashes[PRODUCT],'mutant_sha256':hashlib.sha256(product.read_bytes()).hexdigest()}
   for replay in [False,True]:
    env={'PATH':os.environ.get('PATH','/usr/bin:/bin'),'ARBOS_WORKER_LAW':str(seed)}
    if replay:env['ARBOS_WORKER_REPLAY']=json.dumps(row['shrunk_input'],separators=(',',':'))
    start=time.monotonic();r=subprocess.run(['node','--test',str(tree/LAW)],cwd=tree,env=env,capture_output=True,text=True,timeout=60)
    log=(r.stdout+r.stderr).replace(str(tree),'<isolated-tree>');key='replay'if replay else'generated';(out/f'{name}-{key}.log').write_text(log)
    detected=r.returncode!=0 and ('Exact counterexample replay'in log if replay else'worker-lifetime-counterexample:'in log and'Property falsified'in log)
    row[key]={'exit_code':r.returncode,'seconds':time.monotonic()-start,'detected':detected}
    assert detected,(name,key,log[-4000:])
    if not replay:
     encoded=re.search(r'worker-lifetime-counterexample:([A-Za-z0-9+/=]+)',log).group(1);e=json.loads(base64.b64decode(encoded));row['shrunk_input']=e['result']['shrunkInput'];row['generated']['accepted_shrinks']=1 if '1 shrink(s)'in log else 0
     assert row['shrunk_input']=={'tail':[]},e
   rows.append(row)
 assert hashes=={f:hashlib.sha256((ROOT/f).read_bytes()).hexdigest()for f in files}
 record={'unmodified_tests_passed':5,'generated_law_cases':1024,'protocol_test':1,'original_sources':hashes,'source_unchanged':True,'results':rows};(out/'RESULTS.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps({'mutants':len(rows),'generated_detected':len(rows),'replay_detected':len(rows),'source_unchanged':True}))
if __name__=='__main__':main()
