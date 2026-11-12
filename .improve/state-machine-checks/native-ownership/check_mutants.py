"""Detect actual native-function mutations; isolate sources and preserve shrink/replay evidence."""
from pathlib import Path
import argparse, hashlib, json, os, shutil, subprocess, tempfile, time
HERE=Path(__file__).resolve().parent
ROOT=HERE.parents[2]
SWIFT='apps/ios/Arbos/Arbos/Sync/WorkLifetime.swift'
RUST='crates/arbos-mac/src/sync/work_lifetime.rs'
UI='crates/arbos-mac/src/sync/session_scope.rs'
TASK='crates/arbos-mac/src/sync/account_tasks.rs'
LAW='harness/arbos-pi-ext/native-ownership.laws.ts'
CHECK='.improve/state-machine-checks/native-ownership'
CASES=[
 ('ui-control-only-admits-old-account',90006,UI,'self.control.accepts(control) && self.accepts_account(account)','self.control.accepts(control)'),
 ('ui-stale-controls-admitted',90006,UI,'if !self.control.accepts(control) {\n            return false;\n        }','if false { return false; }'),
 ('task-retirement-reopens',90007,TASK,'std::mem::replace(&mut *state, Tasks::Closed)','std::mem::replace(&mut *state, Tasks::Open(JoinSet::new()))'),
 ('swift-accepts-any-owner',90001,SWIFT,'func accepts(_ ticket: Ticket) -> Bool { self.ticket == ticket }','func accepts(_ ticket: Ticket) -> Bool { true }'),
 ('swift-stale-matrix-failure',90002,SWIFT,'mutating func failed(_ ticket: Ticket) { state = Self.transition(state, .failed(ticket)) }','mutating func failed(_ ticket: Ticket) { state = .idle(ticket.generation) }'),
 ('swift-empty-paint-bypasses-owner',90003,SWIFT,'guard account.accepts(ticket.account), case .active(let doc, let revision) = target,','if empty { return true }; guard account.accepts(ticket.account), case .active(let doc, let revision) = target,'),
 ('rust-accepts-any-owner',90004,RUST,'self.ticket() == Some(ticket)','true'),
 ('rust-stale-matrix-failure',90005,RUST,'self.state = Self::transition(self.state, MatrixEvent::Failed(ticket));','self.state = MatrixState::Idle;'),
]
CANONICAL={'noise':[],'late':[],'closed':True,'foreign':False,'disposed':False,'sameDoc':True,'empty':True,'revision':0,'latest':1}

def main():
 ap=argparse.ArgumentParser();ap.add_argument('--output',type=Path,required=True);a=ap.parse_args();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 files=[SWIFT,RUST,UI,TASK,LAW,*[str(p.relative_to(ROOT)) for p in HERE.rglob('*') if p.is_file() and ('native' in p.parts and p.name in ['Cargo.toml','Cargo.lock','main.rs'] or p.name in ['bridge.py','oracle.swift'])]]
 files=list(dict.fromkeys(files));hashes={p:hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in files};results=[]
 for name,seed,path,before,after in CASES:
  with tempfile.TemporaryDirectory(prefix='arbos-native-mutant-') as raw:
   tree=Path(raw)
   for p in files:
    dest=tree/p;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(ROOT/p,dest)
   ext=tree/'harness/arbos-pi-ext';(ext/'node_modules').symlink_to((ROOT/'harness/arbos-pi-ext/node_modules').resolve());shutil.copyfile(ROOT/'harness/arbos-pi-ext/package.json',ext/'package.json')
   source=tree/path;s=source.read_text();assert s.count(before)==1,(name,'stale mutation anchor');source.write_text(s.replace(before,after,1))
   row={'name':name,'seed':seed,'source':path,'original_sha256':hashes[path],'mutant_sha256':hashlib.sha256(source.read_bytes()).hexdigest()}
   for replay in [False,True]:
    env={'PATH':os.environ.get('PATH','/usr/bin:/bin'),'ARBOS_NATIVE_LAW':str(seed)}
    if replay:env['ARBOS_NATIVE_REPLAY']=json.dumps(CANONICAL,separators=(',',':'))
    t=time.monotonic();r=subprocess.run(['node','--test',str(tree/LAW)],env=env,cwd=tree,text=True,capture_output=True,timeout=280)
    log=(r.stdout+r.stderr).replace(str(tree),'<isolated-tree>');key='replay' if replay else 'generated';(out/f'{name}-{key}.log').write_text(log)
    detected=r.returncode!=0 and ('Property falsified' in log if not replay else f'seed={seed} replay=' in log)
    row[key]={'exit_code':r.returncode,'seconds':time.monotonic()-t,'detected':detected}
    if not replay:row[key]['accepted_shrinks']=1 if '1 shrink(s)' in log else 0
    if not detected or not replay and row[key]['accepted_shrinks']!=1:raise AssertionError(f'Mutant was not detected/shrunk: {name}/{key}')
   row['shrunk_input']=CANONICAL;results.append(row)
 assert hashes=={p:hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in files},'Original sources changed'
 (out/'RESULTS.json').write_text(json.dumps({'controls':'Run the unmodified seven-law suite separately; this command runs eight isolated actual-function mutants.','sources':hashes,'results':results},indent=2)+'\n');print(json.dumps({'mutants':len(results),'generated_detected':len(results),'replay_detected':len(results),'source_unchanged':True}))
if __name__=='__main__':main()
