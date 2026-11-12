"""Actual credential adoption identity checks, local collaborators only."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
def method(text,name):
 pattern=r'^    (?:private )?func '+name+(r'\(tokens' if name=='adopt' else r'\(')
 m=list(re.finditer(pattern,text,re.M));assert len(m)==1
 return text[m[0].start():text.index('\n    }',m[0].end())+6]
F=r'''
import Foundation
struct AuthTokens{var relayUrl="http://synthetic.invalid";var deviceId="same-device";var accessToken="synthetic"}
struct AuthUser{let id:String}
struct DeviceCredentials{let relayUrl:String;let deviceId:String;let deviceToken:String}
enum Keychain{static func storeAuth(_ v:AuthTokens)throws{}}
enum DebugHooks{static let skipFirstLogin=true}
@MainActor final class CallManager{static let shared=CallManager();var api:Bool?}
@MainActor final class AppModel{
 var credentials:DeviceCredentials?;var tokens:AuthTokens?;var user:AuthUser?;var credits:Int?;var needsProfileSetup=false;var accountLifetime=WorkLifetime();var privateMarker:String?;var starts=0;var resets=0
 func boundCallAPI()->Bool?{true};func beginFirstLogin(){};func startAfterPairing(){starts+=1};func stopMatrix(){};func fail(_ label:String,_ error:Error){}
 func unpair(forget:Bool){accountLifetime.close();credentials=nil;privateMarker=nil;resets+=1}
 func install(_ user:AuthUser?,_ tokens:AuthTokens=AuthTokens()){adopt(tokens:tokens,user:user,persist:false)}
'''
T=r'''
}
@main struct Checks{
 @MainActor static func main(){
 var cases:[String:Bool]=[:]
 do{let m=AppModel();m.install(AuthUser(id:"A"));let old=m.accountLifetime.ticket!;m.privateMarker="A-private";m.install(AuthUser(id:"B"));cases["known-user-replacement-retires-same-device"]=(!m.accountLifetime.accepts(old) && m.privateMarker==nil && m.user?.id=="B" && m.starts==2)}
 do{let m=AppModel();m.install(AuthUser(id:"A"));let old=m.accountLifetime.ticket!;m.privateMarker="A-private";m.install(AuthUser(id:"A"));cases["same-known-user-refresh-preserves"]=m.accountLifetime.accepts(old) && m.privateMarker=="A-private" && m.starts==1}
 do{let m=AppModel();m.install(nil);let old=m.accountLifetime.ticket!;m.privateMarker="legacy";m.install(AuthUser(id:"A"));cases["legacy-unknown-upgrade-preserves"]=m.accountLifetime.accepts(old) && m.privateMarker=="legacy"}
 do{let m=AppModel();m.install(AuthUser(id:"A"));let old=m.accountLifetime.ticket!;m.install(nil);cases["missing-user-refresh-preserves-known"]=m.accountLifetime.accepts(old) && m.user?.id=="A"}
 do{let m=AppModel();m.install(AuthUser(id:"A"));let old=m.accountLifetime.ticket!;var next=AuthTokens();next.deviceId="other-device";m.install(AuthUser(id:"A"),next);cases["different-device-retires-control"] = !m.accountLifetime.accepts(old) && m.starts==2}
 do{let m=AppModel();m.install(AuthUser(id:"A"));let old=m.accountLifetime.ticket!;var next=AuthTokens();next.relayUrl="http://other.synthetic.invalid";m.install(AuthUser(id:"A"),next);cases["different-relay-retires-control"] = !m.accountLifetime.accepts(old) && m.starts==2}
 let failed=cases.values.filter{!$0}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":cases.count-failed,"failed":failed,"total":cases.count,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--tree',type=Path,required=True);ap.add_argument('--out',type=Path,required=True);a=ap.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(exist_ok=False)
 paths=[tree/(P+x) for x in ['Store/AppModel.swift','Sync/WorkLifetime.swift']];app,helper=[p.read_text()for p in paths];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};fixture=helper+F+method(app,'prepareCredentialOwner')+'\n'+method(app,'adopt')+T;start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-ios-identity-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'sources':sources,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  record['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
