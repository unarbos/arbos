"""Fail and replay actual DraftSendQueue mutations in disposable source trees."""
from pathlib import Path
import hashlib,json,os,shutil,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
ROOT=HERE.parents[2]
FILES=['apps/ios/Arbos/Arbos/Sync/WorkLifetime.swift','apps/ios/Arbos/Arbos/Sync/DraftSendQueue.swift',
       'harness/arbos-pi-ext/draft-send.laws.ts','.improve/state-machine-checks/ios-account-work/draft_bridge.py',
       '.improve/state-machine-checks/ios-account-work/draft_oracle.swift']
MUTANTS=[('stale-attempt',90009,'current.ticket == ticket','true'),
         ('closed-reopens',90010,'return (.closed, .enqueued(.refused))','return transition(.open(accountFrom(event), [:]), event)')]

def main():
    # The second mutation changes only the real closed enqueue transition, not a
    # copied model. Destructure the event to expose the supplied account target.
    started=time.monotonic();records=[]
    source=ROOT/'apps/ios/Arbos/Arbos/Sync/DraftSendQueue.swift'
    original=source.read_text()
    for name,seed,before,after in MUTANTS:
        with tempfile.TemporaryDirectory(prefix='arbos-draft-mutant-') as raw:
            tree=Path(raw)
            for relative in FILES:
                dest=tree/relative;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(ROOT/relative,dest)
            package=tree/'harness/arbos-pi-ext'
            (package/'package.json').write_text('{"type":"module"}\n')
            (package/'node_modules').symlink_to(ROOT/'harness/arbos-pi-ext/node_modules',target_is_directory=True)
            text=original
            if name=='closed-reopens':
                old='case (.closed, .enqueue):\n            return (.closed, .enqueued(.refused))'
                new='case (.closed, .enqueue(_, let account, _, _)):\n            return transition(.open(account, [:]), event)'
            else:old,new=before,after
            assert text.count(old)==1
            (tree/'apps/ios/Arbos/Arbos/Sync/DraftSendQueue.swift').write_text(text.replace(old,new))
            result=tree/'result.json'
            env={'PATH':os.environ.get('PATH','/usr/bin:/bin'),'ARBOS_DRAFT_LAW':str(seed),'ARBOS_DRAFT_RESULT':str(result)}
            command=['node','--test',str(package/'draft-send.laws.ts')]
            run=subprocess.run(command,env=env,text=True,capture_output=True,timeout=120)
            observed=json.loads(result.read_text())
            assert run.returncode!=0 and observed['_tag']=='Falsified',name
            shrunk=observed['shrunkInput']
            assert observed['shrinks']>0,name
            env['ARBOS_DRAFT_REPLAY']=json.dumps(shrunk,separators=(',',':'))
            replay=subprocess.run(command,env=env,text=True,capture_output=True,timeout=120)
            assert replay.returncode!=0 and 'replay='+env['ARBOS_DRAFT_REPLAY'] in replay.stdout,name
            records.append({'name':name,'seed':seed,'result':observed,'replay_failed':True,'mutation':{'before':old,'after':new}})
    out={'sources':{p:hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in FILES},'mutants':records,'duration_seconds':time.monotonic()-started}
    print(json.dumps(out,indent=2))
if __name__=='__main__':main()
