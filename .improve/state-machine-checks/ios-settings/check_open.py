from pathlib import Path
import argparse,hashlib,json,subprocess,tempfile,time
from check_profile import P,VIEW,method,plain
F=r'''
import Foundation
ORIGIN
struct Project{let id:String}
struct AccountWork {let owner:WorkLifetime.Ticket}
@MainActor final class AppModel {
 var accountLifetime=WorkLifetime();var relayUrl="https://example.invalid";var credentials:Bool?{authenticated ? true:nil};var current="A";var isFixture=false;var authenticated=true;var opens:[String]=[]
 func capture()->AccountWork?{guard authenticated,!isFixture else{return nil};return accountLifetime.ticket.map{AccountWork(owner:$0)}}
 func accountWork()->AccountWork?{capture()}
 func replace(){accountLifetime.close();accountLifetime.reopen();current="B"}
 func open(project:Project){opens.append(current+"/"+project.id)}
 ACCEPTS
}
@MainActor final class Counts{var dismissals=0}
@MainActor struct Settings {
 let model:AppModel;let dismiss:()->Void
 METHOD
 func act(_ work:AccountWork?,fixture:Bool=false){CALL}
}
@main struct Checks {
 @MainActor static func main()async{
 var snapshots:[(String,AppModel,Counts,[String],Int)]=[]
 do{let m=AppModel();let c=Counts();let v=Settings(model:m,dismiss:{c.dismissals+=1});let a=m.capture();m.replace();v.act(a);snapshots.append(("queued-old-agent-row-does-not-open-or-dismiss-B",m,c,[],0))}
 do{let m=AppModel();let c=Counts();let v=Settings(model:m,dismiss:{c.dismissals+=1});v.act(m.capture());m.replace();snapshots.append(("scheduled-old-agent-open-refuses-B",m,c,[],1))}
 do{let m=AppModel();let c=Counts();let v=Settings(model:m,dismiss:{c.dismissals+=1});v.act(m.capture());snapshots.append(("current-agent-open-control",m,c,["A/old-project"],1))}
 do{let m=AppModel();m.isFixture=true;m.authenticated=false;let c=Counts();let v=Settings(model:m,dismiss:{c.dismissals+=1});v.act(nil,fixture:true);snapshots.append(("fixture-agent-open-local-policy-control",m,c,["A/old-project"],1))}
 do{let m=AppModel();m.authenticated=false;let c=Counts();let v=Settings(model:m,dismiss:{c.dismissals+=1});v.act(nil);snapshots.append(("missing-owner-does-not-open-account-project",m,c,[],0))}
 do{let m=AppModel();let c=Counts();let v=Settings(model:m,dismiss:{c.dismissals+=1});let a=m.capture();let task=Task{v.act(a)};task.cancel();await task.value;snapshots.append(("cancelled-agent-open-does-not-dismiss-or-schedule",m,c,[],0))}
 try? await Task.sleep(nanoseconds:650_000_000)
 var cases:[String:Bool]=[:];for(name,m,c,opens,dismissals)in snapshots{cases[name]=m.opens==opens && c.dismissals==dismissals};let failed=cases.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["total":cases.count,"passed":cases.count-failed,"failed":failed,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('arm',choices=['before','after']);ap.add_argument('label');a=ap.parse_args();view=P/'base/SettingsView.swift'if a.arm=='before'else VIEW;s=view.read_text();body=method(s,'openAgentFromSettings');ctx=(P/'context/AppModel.swift').read_text();accept=plain(ctx,'acceptsAccountWork');helper=(P/'context/WorkLifetime.swift').read_text();call='openAgentFromSettings(Project(id:"old-project"),origin:work.map(SettingsOrigin.account) ?? (fixture ? .fixture(model.accountLifetime.ticket,model.accountLifetime.state):.unavailable))'if'origin:'in body else'openAgentFromSettings(Project(id:"old-project"))';origin='';
 if 'private enum SettingsOrigin {'in s:
  start_origin=s.index('@MainActor\nprivate enum SettingsOrigin {');origin=s[start_origin:s.index('\n}',start_origin)+2]
 fixture=helper+F.replace('ORIGIN',origin).replace('ACCEPTS',accept).replace('METHOD',body).replace('CALL',call);out=P/a.label;out.mkdir(exist_ok=False);(out/'fixture.swift').write_text(fixture);start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='settings-open-')as raw:
  tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'view_sha256':hashlib.sha256(s.encode()).hexdigest(),'method_sha256':hashlib.sha256(body.encode()).hexdigest(),'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  assert s==view.read_text();record['seconds']=time.monotonic()-start;(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
