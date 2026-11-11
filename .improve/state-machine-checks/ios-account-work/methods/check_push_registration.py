"""Actual machine refresh with synthetic paused API results, no machine/provider use."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
P='apps/ios/Arbos/Arbos/';paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};app,api,helper=[p.read_text()for p in paths]
def method(text,name):
 m=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1;start=m[0].start();start=start-len('    @MainActor\n')if text[:start].endswith('    @MainActor\n')else start;return text[start:text.index('\n    }',m[0].end())+6]
fakes=(HERE/'check_directory.py').read_text();start=fakes.index("fixture=r'''")+len("fixture=r'''");end=fakes.index("'''+method(api,'bound')",start);fixture=fakes[start:end]
fixture=fixture.replace('var authorizations:[String]=[]','var authorizations:[String]=[];var pauseRegister=false;var registerWaiter:CheckedContinuation<Bool,Error>?;var registrations=0')
fixture=fixture.replace(' func finish(_ value:[Person]?)'," func registerPush()async throws->Bool{registrations+=1;if pauseRegister{return try await withCheckedThrowingContinuation{registerWaiter=$0}};return false}\n func finishRegister(_ failure:Bool=false){let saved=registerWaiter;registerWaiter=nil;if failure{saved?.resume(throwing:URLError(.badServerResponse))}else{saved?.resume(returning:false)}}\n func finish(_ value:[Person]?)")
fixture=fixture.replace(' func users()async throws->[Person]{if let authorization'," func registerPush(deviceId:String,token:String,environment:String,bundleId:String,appVersion:String)async throws->Bool{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(self.token())};return try await session.registerPush()}\n func users()async throws->[Person]{if let authorization")
fixture+=method(api,'bound')+'\n}\n'+helper
if 'struct AccountWork' in app:
 m=re.search(r'^(?:private )?struct AccountWork \{',app,re.M);fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
fixture+=r'''
struct DeviceCredentials{let deviceId:String}
@MainActor final class PushManager {
 static let shared=PushManager();var register:((String,String)async->Void)?;var relayPushEnabled=true;var forgets=0;var sends=0;var pauseStatus=false;var statusWaiter:CheckedContinuation<Void,Never>?
 func reset(){register=nil;relayPushEnabled=true;forgets=0;sends=0;pauseStatus=false;statusWaiter=nil}
 func forgetRegistration(){forgets+=1};func refreshStatus()async{if pauseStatus{await withCheckedContinuation{statusWaiter=$0}}};func sendTokenIfNeeded(force:Bool)async{sends+=1}
}
@MainActor final class AppModel {
 var credentials:DeviceCredentials?=DeviceCredentials(deviceId:"A");var isFixture=false;var api:RelayAPI;var accountLifetime=WorkLifetime();var callHooks=0
 init(_ session:Session){api=RelayAPI(session)}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{expectedOwner.map{accountLifetime.accepts($0)} ?? true}
 func replace(_ session:Session){accountLifetime.close();accountLifetime.reopen();api=RelayAPI(session);credentials=DeviceCredentials(deviceId:"B");PushManager.shared.relayPushEnabled=true;PushManager.shared.forgets=0;PushManager.shared.sends=0}
 func hookForCheck(){hookPush()}
 func hookCalls(){callHooks+=1}
'''
for name in ['accountWork','boundCallAPI']:
 if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=method(app,name)+'\n'
fixture+=method(app,'hookPush')+'\n}\n'+r'''
@main struct Checks {
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func settle()async{for _ in 0..<100{await Task.yield()}}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{PushManager.shared.reset();let a=Session("A");a.pauseRegister=true;let m=AppModel(a);m.hookForCheck();let callback=PushManager.shared.register!;let old=Task{await callback("synthetic","development")};let began=await spin{a.registerWaiter != nil};m.replace(Session("B"));a.finishRegister();await old.value;record("late-register-success-cannot-replace-B-status",began && PushManager.shared.relayPushEnabled)}
  do{PushManager.shared.reset();let a=Session("A");a.pauseRegister=true;let m=AppModel(a);m.hookForCheck();let callback=PushManager.shared.register!;let old=Task{await callback("synthetic","development")};let began=await spin{a.registerWaiter != nil};m.replace(Session("B"));a.finishRegister(true);await old.value;record("late-register-failure-cannot-forget-B",began && PushManager.shared.forgets==0)}
  do{PushManager.shared.reset();let a=Session("A");let m=AppModel(a);m.hookForCheck();let callback=PushManager.shared.register!;m.replace(Session("B"));await callback("synthetic","development");record("retired-register-callback-cannot-start-request",a.registrations==0 && PushManager.shared.forgets==0)}
  do{PushManager.shared.reset();PushManager.shared.pauseStatus=true;let a=Session("A");let m=AppModel(a);m.hookForCheck();let began=await spin{PushManager.shared.statusWaiter != nil};m.replace(Session("B"));let saved=PushManager.shared.statusWaiter;PushManager.shared.statusWaiter=nil;saved?.resume();await settle();record("retired-bootstrap-cannot-start-token-step",began && PushManager.shared.sends==0)}
  do{PushManager.shared.reset();let a=Session("A");let m=AppModel(a);m.hookForCheck();await PushManager.shared.register?("synthetic","development");await settle();record("current-register-and-bootstrap-control",a.registrations==1 && !PushManager.shared.relayPushEnabled && PushManager.shared.forgets==0 && PushManager.shared.sends==1)}
  do{PushManager.shared.reset();let a=Session("A");let m=AppModel(a);m.isFixture=true;m.hookForCheck();await settle();record("fixture-call-hook-without-push-request-control",m.callHooks==1 && PushManager.shared.register==nil && a.registrations==0 && PushManager.shared.sends==0)}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-ios-push-registration-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':sources,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},'Sources changed';(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==6 and row['results']['failed']==0,'Expected6 push-registration ownership controls'
