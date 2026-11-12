from pathlib import Path
import hashlib,json,subprocess,tempfile,time,argparse
from check_profile import P,VIEW,plain
F=r'''
import Foundation
struct AccountWork{let owner:WorkLifetime.Ticket}
ORIGIN
@MainActor final class AppModel{
 var accountLifetime=WorkLifetime();var isFixture=false;var relayUrl="https://example.invalid";var credentials:Bool?=true
 func accountWork()->AccountWork?{guard credentials != nil else{return nil};return accountLifetime.ticket.map{AccountWork(owner:$0)}}
 func replace(){accountLifetime.close();accountLifetime.reopen()}
 ACCEPTS
}
@main struct Checks{
 @MainActor static func main()async{var cases:[String:Bool]=[:]
 do{let m=AppModel();let a=SettingsOrigin(m);cases["current-account-admitted"]=a.accepts(m);m.replace();cases["replacement-account-refused"] = !a.accepts(m)}
 do{let m=AppModel();let a=SettingsOrigin(m);cases["ordinary-same-owner-refresh-admitted"]=a.accepts(m);m.accountLifetime.close();cases["closed-account-refused"] = !a.accepts(m);m.accountLifetime.reopen();cases["same-credentials-readoption-refused"] = !a.accepts(m)}
 do{let m=AppModel();m.credentials=nil;let a=SettingsOrigin(m);cases["unpaired-is-not-account-permission"] = !a.accepts(m);cases["explicit-local-unpaired-policy-control"]=a.accepts(m,allowUnpaired:true);m.relayUrl="https://other.invalid";cases["unpaired-relay-replacement-refused"] = !a.accepts(m,allowUnpaired:true)}
 do{let m=AppModel();m.credentials=nil;let a=SettingsOrigin(m);m.credentials=true;m.replace();cases["unpaired-cannot-adopt-authenticated-account"] = !a.accepts(m,allowUnpaired:true);m.credentials=nil;m.accountLifetime.close();cases["unpaired-ABA-after-account-retirement-refused"] = !a.accepts(m,allowUnpaired:true)}
 do{let m=AppModel();m.isFixture=true;m.credentials=nil;let a=SettingsOrigin(m);cases["fixture-local-policy-control"]=a.accepts(m);m.isFixture=false;m.credentials=true;cases["old-fixture-cannot-adopt-real-account"] = !a.accepts(m)}
 do{let m=AppModel();let task=Task{SettingsOrigin(m)};task.cancel();let origin=await task.value;cases["cancelled-capture-remains-unavailable-after-task"] = !origin.accepts(m,allowUnpaired:true)}
 do{let m=AppModel();let origin=SettingsOrigin(m);let task=Task{origin.accepts(m,allowUnpaired:true)};task.cancel();cases["cancelled-callback-refused"] = !(await task.value)}
 let failed=cases.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["total":cases.count,"passed":cases.count-failed,"failed":failed,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('label');a=ap.parse_args();s=VIEW.read_text();x=s.index('@MainActor\nprivate enum SettingsOrigin {');origin=s[x:s.index('\n}',x)+2];helper=(P/'context/WorkLifetime.swift').read_text();accept=plain((P/'context/AppModel.swift').read_text(),'acceptsAccountWork');fixture=helper+F.replace('ORIGIN',origin).replace('ACCEPTS',accept);out=P/a.label;out.mkdir(exist_ok=False);(out/'fixture.swift').write_text(fixture);start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='settings-origin-')as raw:
  tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'view_sha256':hashlib.sha256(s.encode()).hexdigest(),'origin_sha256':hashlib.sha256(origin.encode()).hexdigest(),'compile_exit':c.returncode}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  assert s==VIEW.read_text();record['seconds']=time.monotonic()-start;(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
