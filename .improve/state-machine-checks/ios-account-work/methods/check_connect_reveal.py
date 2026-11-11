"""Actual ConnectReveal .task body over paused account-bound collaborator; no UI/HTTP."""
from pathlib import Path
import argparse, hashlib, importlib.util, json, re, subprocess, tempfile, time
HERE=Path(__file__).resolve().parent; P='apps/ios/Arbos/Arbos/'
spec=importlib.util.spec_from_file_location('a',HERE/'check_actions.py');a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a);u=a.u

def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);args=p.parse_args();tree=args.tree.resolve();out=args.out.resolve();out.mkdir(exist_ok=False)
 paths=[tree/(P+q)for q in ['UI/PeopleSheets.swift','Store/AppModel.swift','Sync/WorkLifetime.swift']];src,app,helper=[p.read_text()for p in paths];hashes={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths}
 block=src[src.index('struct ConnectRevealView: View {'):src.index('struct GroupDraft:')]
 marker='.task(id: reveal.projectId) {';start=block.index(marker)+len(marker);end=block.index('\n        }\n    }',start);task=block[start:end]
 fixture='import Foundation\n'+helper+r'''
struct AccountWork{let owner:WorkLifetime.Ticket}
struct OutsideAgentInfo{let connected:Bool;let marker:String}
struct ConnectReveal{let projectId:String}
@MainActor final class Model{
 var accountLifetime=WorkLifetime();var requests:[String]=[];var pause=false;var waiter:CheckedContinuation<Void,Never>?
 func replace(){accountLifetime.close();accountLifetime.reopen()}
 func refreshOutsideInfo(_ id:String,expectedOwner:WorkLifetime.Ticket?=nil)async->OutsideAgentInfo?{guard expectedOwner==nil || accountLifetime.accepts(expectedOwner!)else{return nil};requests.append(id);if pause{await withCheckedContinuation{waiter=$0}};return OutsideAgentInfo(connected:true,marker:"A")}
'''+u.method(app,'acceptsAccountWork')+'\n}\n'+r'''
@MainActor final class RevealView{
 let model:Model;let reveal=ConnectReveal(projectId:"synthetic");var info:OutsideAgentInfo?
 var connected:Bool{info?.connected==true};init(_ model:Model){self.model=model}
 func run(work:AccountWork?)async{
'''+task+r'''
 }
}
@main struct Checks{
 @MainActor static func spin(_ f:()->Bool)async->Bool{for _ in 0..<10000{if f(){return true};await Task.yield()};return f()}
 @MainActor static func main()async{
 var cases:[String:Bool]=[:]
 do{let m=Model();m.pause=true;let v=RevealView(m);let w=AccountWork(owner:m.accountLifetime.ticket!);let old=Task{await v.run(work:w)};let began=await spin{m.waiter != nil};m.replace();v.info=OutsideAgentInfo(connected:true,marker:"B");m.waiter?.resume();m.waiter=nil;await old.value;cases["late-status-no-B-view-paint"]=began && v.info?.marker=="B"}
 do{let m=Model();let v=RevealView(m);let w=AccountWork(owner:m.accountLifetime.ticket!);m.replace();await v.run(work:w);cases["retired-render-no-fresh-query"]=m.requests.isEmpty && v.info==nil}
 do{let m=Model();let v=RevealView(m);await v.run(work:AccountWork(owner:m.accountLifetime.ticket!));cases["current-first-heartbeat-control"]=m.requests==["synthetic"] && v.info?.marker=="A" && v.connected}
 let failed=cases.values.filter{!$0}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":cases.count-failed,"failed":failed,"total":cases.count,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
 start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-reveal-check-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);row={'sources':hashes,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  row['seconds']=time.monotonic()-start;assert hashes=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==3;exit(row['results']['failed']!=0)
if __name__=='__main__':main()
