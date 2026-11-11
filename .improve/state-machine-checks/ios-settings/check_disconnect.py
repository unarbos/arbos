from pathlib import Path
import argparse,hashlib,json,subprocess,tempfile,time
from check_profile import P,VIEW,plain

def closure(s,anchor):
 a=s.index(anchor)+len(anchor);depth=1;i=a
 while depth:
  if s[i]=='{':depth+=1
  if s[i]=='}':depth-=1
  i+=1
 return s[a:i-1]
F=r'''
import Foundation
struct AccountWork{let owner:WorkLifetime.Ticket}
ORIGIN
SELECTION
@MainActor final class AppModel{
 var accountLifetime=WorkLifetime();var isFixture=false;var credentials:Bool?=true;var relayUrl="https://example.invalid";var current="A";var calls:[String]=[];var onDisconnect:(()->Void)?
 func accountWork()->AccountWork?{accountLifetime.ticket.map{AccountWork(owner:$0)}}
 func replace(){accountLifetime.close();accountLifetime.reopen();current="B"}
 func disconnectConnector(id:String){calls.append(current+"/"+id);onDisconnect?()}
 ACCEPTS
}
@MainActor private final class Dialog{
 let model:AppModel;STATE
 init(_ m:AppModel){model=m}
 func stage(_ selection:SettingsConnectorDisconnect){STAGE}
 var pending:Bool{disconnecting != nil}
 func perform(_ origin:SettingsOrigin,_ confirmation:SettingsConnectorDisconnect?){ACTION}
 func cancel(_ confirmation:SettingsConnectorDisconnect?){CANCEL}
 func presented(_ shown:Bool,_ confirmation:SettingsConnectorDisconnect?){SETTER}
}
@main struct Tests{
 @MainActor static func main(){var cases:[String:Bool]=[:]
 do{let m=AppModel();let d=Dialog(m);let a=SettingsOrigin(m);let first=SettingsConnectorDisconnect(connectorId:"same",label:"App",origin:a);d.stage(first);m.replace();let second=SettingsConnectorDisconnect(connectorId:"same",label:"App",origin:SettingsOrigin(m));d.stage(second);d.perform(a,first);cases["old-A-confirm-cannot-act-on-same-id-B-selection"]=m.calls.isEmpty && d.pending}
 do{let m=AppModel();let d=Dialog(m);let a=SettingsOrigin(m);let first=SettingsConnectorDisconnect(connectorId:"same",label:"App",origin:a);d.stage(first);let second=SettingsConnectorDisconnect(connectorId:"same",label:"App",origin:a);d.stage(second);d.perform(a,first);cases["old-confirm-cannot-adopt-new-same-account-selection"]=m.calls.isEmpty && d.pending}
 do{let m=AppModel();let d=Dialog(m);let a=SettingsOrigin(m);let first=SettingsConnectorDisconnect(connectorId:"first",label:"App",origin:a);d.stage(first);d.stage(SettingsConnectorDisconnect(connectorId:"next",label:"App",origin:a));d.cancel(first);cases["old-cancel-preserves-replacement-selection"]=d.pending && m.calls.isEmpty}
 do{let m=AppModel();let d=Dialog(m);let a=SettingsOrigin(m);let first=SettingsConnectorDisconnect(connectorId:"first",label:"App",origin:a);d.stage(first);d.stage(SettingsConnectorDisconnect(connectorId:"next",label:"App",origin:a));d.presented(false,first);cases["old-dismiss-binding-preserves-replacement-selection"]=d.pending && m.calls.isEmpty}
 do{let m=AppModel();let d=Dialog(m);let a=SettingsOrigin(m);let first=SettingsConnectorDisconnect(connectorId:"first",label:"App",origin:a);d.stage(first);m.onDisconnect={d.stage(SettingsConnectorDisconnect(connectorId:"next",label:"App",origin:a))};d.perform(a,first);cases["confirm-effect-keeps-reentrant-new-selection"]=m.calls==["A/first"] && d.pending}
 do{let m=AppModel();let d=Dialog(m);let a=SettingsOrigin(m);let first=SettingsConnectorDisconnect(connectorId:"first",label:"App",origin:a);d.stage(first);d.perform(a,first);cases["current-confirm-control"]=m.calls==["A/first"] && !d.pending}
 do{let m=AppModel();let d=Dialog(m);let a=SettingsOrigin(m);let first=SettingsConnectorDisconnect(connectorId:"first",label:"App",origin:a);d.stage(first);d.cancel(first);cases["current-cancel-control"]=m.calls.isEmpty && !d.pending}
 let failed=cases.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["total":cases.count,"passed":cases.count-failed,"failed":failed,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('arm',choices=['before','after']);ap.add_argument('label');a=ap.parse_args();view=P/'base/SettingsView.swift'if a.arm=='before'else VIEW;s=view.read_text();current=VIEW.read_text();x=current.index('@MainActor\nprivate enum SettingsOrigin {');origin=current[x:current.index('\n}',x)+2];x=current.index('private struct SettingsConnectorDisconnect:');selection=current[x:current.index('\n}',x)+2];start=s.index('struct ConnectorsPage:');end=s.index('struct DevicesPage:',start);part=s[start:end];dialog=part[part.index('.confirmationDialog('):];action=closure(dialog,'Button("Disconnect", role: .destructive) {');cancel=closure(dialog,'Button("Keep", role: .cancel) {');setter=closure(dialog,'set: {');after='SettingsConnectorDisconnect?'in part;state='var disconnecting:SettingsConnectorDisconnect?'if after else'var disconnecting:(String,String)?';stage='disconnecting=selection'if after else'disconnecting=(selection.connectorId,selection.label)';helper=(P/'context/WorkLifetime.swift').read_text();accept=plain((P/'context/AppModel.swift').read_text(),'acceptsAccountWork');fixture=helper+F.replace('ORIGIN',origin).replace('SELECTION',selection).replace('ACCEPTS',accept).replace('STATE',state).replace('STAGE',stage).replace('ACTION',action).replace('CANCEL',cancel).replace('SETTER',setter.replace('$0','shown'));out=P/a.label;out.mkdir(exist_ok=False);(out/'fixture.swift').write_text(fixture);start_time=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='settings-dialog-')as raw:
  tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'view_sha256':hashlib.sha256(s.encode()).hexdigest(),'closures_sha256':hashlib.sha256((action+cancel+setter).encode()).hexdigest(),'compile_exit':c.returncode}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  assert s==view.read_text();record['seconds']=time.monotonic()-start_time;(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
