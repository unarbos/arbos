from pathlib import Path
import argparse,hashlib,json,subprocess,tempfile,time
from check_profile import P,VIEW,method,plain
F=r'''
import Foundation
struct AccountWork{let owner:WorkLifetime.Ticket}
struct ConnectorProvider{let provider:String}
ORIGIN
TOKEN
@MainActor final class AppModel{
 var accountLifetime=WorkLifetime();var current="A";var isFixture=false;var relayUrl="https://example.invalid";var credentials:Bool?=true;var calls:[String]=[]
 func accountWork()->AccountWork?{guard credentials != nil else{return nil};return accountLifetime.ticket.map{AccountWork(owner:$0)}}
 func replace(){accountLifetime.close();accountLifetime.reopen();current="B"}
 func connectConnector(provider:String,token:String?=nil){guard credentials != nil,!isFixture else{return};calls.append(current+"/"+provider+"/"+(token ?? ""))}
 ACCEPTS
}
@MainActor private final class Form{
 let model:AppModel;STATE
 init(_ model:AppModel){self.model=model}
 OPEN
 SAVE
 func stage(_ origin:SettingsOrigin){STAGE}
 func text(_ value:String){TEXT}
 func act(_ origin:SettingsOrigin,provider:String="google"){ACT}
}
@main struct Checks{
 @MainActor static func main(){var cases:[String:Bool]=[:]
 do{let m=AppModel();let v=Form(m);let a=SettingsOrigin(m);v.stage(a);v.text("synthetic-A");m.replace();v.act(a);cases["old-rendered-save-cannot-send-A-token-under-B"]=m.calls.isEmpty}
 do{let m=AppModel();let v=Form(m);let a=SettingsOrigin(m);v.stage(a);v.text("synthetic-A");m.replace();v.act(SettingsOrigin(m));cases["new-row-cannot-adopt-retired-token-form"]=m.calls.isEmpty}
 do{let m=AppModel();let v=Form(m);let a=SettingsOrigin(m);v.stage(a);v.text("synthetic-A");m.replace();let b=SettingsOrigin(m);v.stage(b);v.text("synthetic-B");v.act(a);cases["old-save-cannot-adopt-replacement-B-form"]=m.calls.isEmpty}
 do{let m=AppModel();let v=Form(m);let a=SettingsOrigin(m);v.stage(a);v.text("synthetic-A");v.act(a,provider:"slack");cases["token-stays-bound-to-selected-provider"]=m.calls.isEmpty}
 do{let m=AppModel();let v=Form(m);let a=SettingsOrigin(m);v.stage(a);v.text("  synthetic-A  ");v.act(a);cases["current-token-trim-and-send-control"]=m.calls==["A/google/synthetic-A"]}
 do{let m=AppModel();let v=Form(m);let a=SettingsOrigin(m);v.stage(a);v.text("  ");v.act(a);cases["empty-token-no-api-control"]=m.calls.isEmpty}
 do{let m=AppModel();m.credentials=nil;let v=Form(m);let a=SettingsOrigin(m);v.stage(a);v.text("synthetic-local");v.act(a);cases["unpaired-token-no-api-control"]=m.calls.isEmpty}
 let failed=cases.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["total":cases.count,"passed":cases.count-failed,"failed":failed,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('arm',choices=['before','after']);ap.add_argument('label');a=ap.parse_args();view=P/'base/SettingsView.swift'if a.arm=='before'else VIEW;s=view.read_text();current=VIEW.read_text();x=current.index('@MainActor\nprivate enum SettingsOrigin {');origin=current[x:current.index('\n}',x)+2];x=current.index('private struct SettingsConnectorToken {');token=current[x:current.index('\n}',x)+2];op=method(s,'openToken');save=method(s,'saveToken');after='SettingsConnectorToken?'in s;state='var tokenDraft:SettingsConnectorToken?'if after else'var tokenFor:String?;var tokenDraft=""';stage='openToken(ConnectorProvider(provider:"google"),origin:origin)'if'origin:'in op else'openToken(ConnectorProvider(provider:"google"))';text='tokenDraft?.value=value'if after else'tokenDraft=value';act='saveToken(ConnectorProvider(provider:provider),origin:origin)'if'origin:'in save.split('\n')[0] else'saveToken(ConnectorProvider(provider:provider))';helper=(P/'context/WorkLifetime.swift').read_text();accept=plain((P/'context/AppModel.swift').read_text(),'acceptsAccountWork');fixture=helper+F.replace('ORIGIN',origin).replace('TOKEN',token).replace('ACCEPTS',accept).replace('STATE',state).replace('OPEN',op).replace('SAVE',save).replace('STAGE',stage).replace('TEXT',text).replace('ACT',act);out=P/a.label;out.mkdir(exist_ok=False);(out/'fixture.swift').write_text(fixture);start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='settings-token-')as raw:
  tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'view_sha256':hashlib.sha256(s.encode()).hexdigest(),'origin_and_token_source_sha256':hashlib.sha256((origin+token).encode()).hexdigest(),'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  assert s==view.read_text();record['seconds']=time.monotonic()-start;(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
