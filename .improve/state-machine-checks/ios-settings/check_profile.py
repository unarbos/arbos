from pathlib import Path
import argparse,hashlib,json,subprocess,tempfile,time
P=Path(__file__).resolve().parent
VIEW=P.parents[2]/'apps/ios/Arbos/Arbos/UI/SettingsView.swift'
def method(s,name):
 a=s.index('    private func '+name+'(');return s[a:s.index('\n    }',a)+6]
def plain(s,name):
 a=s.index('    func '+name+'(');return s[a:s.index('\n    }',a)+6]
F=r'''
import Foundation
struct RelayAPI{}
struct AccountWork { let owner: WorkLifetime.Ticket;let api=RelayAPI() }
struct UserError { let message:String;static func refused(_ verb:String,_ message:String)->UserError{UserError(message:message)} }
@MainActor final class AppModel {
 var accountLifetime=WorkLifetime();var current="A";var requested:[String]=[];var waiter:CheckedContinuation<String?,Never>?
 func capture()->AccountWork?{accountLifetime.ticket.map{AccountWork(owner:$0)}}
 func replace(){accountLifetime.close();accountLifetime.reopen();current="B"}
 func updateProfile(name:String,handle:String,expectedOwner:WorkLifetime.Ticket?=nil)async->String?{
  guard !Task.isCancelled,let owner=accountLifetime.ticket,expectedOwner==nil||expectedOwner==owner else{return nil}
  requested.append(current);let result=await withCheckedContinuation{waiter=$0};guard accountLifetime.accepts(owner)else{return nil};return result
 }
 func finish(_ result:String?){let w=waiter;waiter=nil;w?.resume(returning:result)}
 ACCEPTS
}
@MainActor final class Profile {
 let model:AppModel;var name="old name";var handle="old-handle";var canSave=true;var busy=false;var error:UserError?;var dismissals=0
 init(_ model:AppModel){self.model=model};func dismiss(){dismissals+=1}
 METHOD
 func act(_ work:AccountWork?)async{CALL}
}
@main struct Tests {
 @MainActor static func spin(_ p:()->Bool)async->Bool{for _ in 0..<5000{if p(){return true};await Task.yield()};return p()}
 @MainActor static func main()async{
 var cases:[String:Bool]=[:]
 do{let m=AppModel();let v=Profile(m);let a=m.capture();m.replace();let task=Task{await v.act(a)};if await spin({m.waiter != nil}){m.finish(nil)};await task.value;cases["queued-old-profile-refuses-new-account-rpc-and-dismiss"]=m.requested.isEmpty && v.dismissals==0 && !v.busy}
 do{let m=AppModel();let v=Profile(m);let a=m.capture();let task=Task{await v.act(a)};let began=await spin{m.waiter != nil};m.replace();v.busy=true;v.error=UserError(message:"B error");m.finish(nil);await task.value;cases["retired-profile-completion-cannot-dismiss-or-clear-B-busy"]=began && m.requested==["A"] && v.dismissals==0 && v.busy && v.error?.message=="B error"}
 do{let m=AppModel();let v=Profile(m);let a=m.capture();let task=Task{await v.act(a)};task.cancel();if await spin({m.waiter != nil}){m.finish(nil)};await task.value;cases["cancelled-profile-task-does-not-dismiss-or-start-rpc"]=m.requested.isEmpty && v.dismissals==0 && !v.busy}
 do{let m=AppModel();let v=Profile(m);let a=m.capture();let task=Task{await v.act(a)};let began=await spin{m.waiter != nil};task.cancel();m.finish(nil);await task.value;cases["cancelled-current-save-releases-own-busy-without-dismiss"]=began && m.requested==["A"] && !v.busy && v.dismissals==0 && v.error==nil}
 for result:String? in [nil,"name refused"]{let m=AppModel();let v=Profile(m);let a=m.capture();let task=Task{await v.act(a)};let began=await spin{m.waiter != nil};m.finish(result);await task.value;cases[result==nil ? "current-profile-save-success-control":"current-profile-save-error-control"]=began && m.requested==["A"] && !v.busy && (result==nil ? v.dismissals==1 && v.error==nil:v.dismissals==0 && v.error?.message==result)}
 do{let m=AppModel();let v=Profile(m);v.canSave=false;await v.act(m.capture());cases["invalid-form-no-rpc-control"]=m.requested.isEmpty && v.dismissals==0 && !v.busy}
 let failed=cases.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["total":cases.count,"passed":cases.count-failed,"failed":failed,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('arm',choices=['before','after']);ap.add_argument('label');a=ap.parse_args();view=P/'base/SettingsView.swift'if a.arm=='before'else VIEW;s=view.read_text();body=method(s,'save');ctx=(P/'context/AppModel.swift').read_text();accept=plain(ctx,'acceptsAccountWork');helper=(P/'context/WorkLifetime.swift').read_text();call='await save(work:work)'if 'save(work:'in body else'await save()';fixture=helper+F.replace('ACCEPTS',accept).replace('METHOD',body).replace('CALL',call);out=P/a.label;out.mkdir(exist_ok=False);(out/'fixture.swift').write_text(fixture);start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='settings-owner-')as raw:
  tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'view_sha256':hashlib.sha256(s.encode()).hexdigest(),'accepts_method_sha256':hashlib.sha256(accept.encode()).hexdigest(),'helper_sha256':hashlib.sha256(helper.encode()).hexdigest(),'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=30);(out/'test.log').write_text(r.stdout+r.stderr);record.update(exit_code=r.returncode,results=json.loads(r.stdout))
  assert s==view.read_text();record['seconds']=time.monotonic()-start;(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
