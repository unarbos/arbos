"""Actual machine refresh with synthetic paused API results, no machine/provider use."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
P='apps/ios/Arbos/Arbos/';paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};app,api,helper=[p.read_text()for p in paths]
def method(text,name):
 m=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1;start=m[0].start();start=start-len('    @MainActor\n')if text[:start].endswith('    @MainActor\n')else start;return text[start:text.index('\n    }',m[0].end())+6]
fakes=(HERE/'check_directory.py').read_text();start=fakes.index("fixture=r'''")+len("fixture=r'''");end=fakes.index("'''+method(api,'bound')",start);fixture=fakes[start:end]
fixture=fixture.replace('var authorizations:[String]=[]','var authorizations:[String]=[];var pauseModels=false;var modelsWaiter:CheckedContinuation<[String],Error>?')
fixture=fixture.replace(' func finish(_ value:[Person]?)'," func models()async throws->[String]{if pauseModels{return try await withCheckedThrowingContinuation{modelsWaiter=$0}};return [label]}\n func finishModels(){let saved=modelsWaiter;modelsWaiter=nil;saved?.resume(returning:[label])}\n func finish(_ value:[Person]?)")
fixture=fixture.replace(' func users()async throws->[Person]{if let authorization'," func models()async throws->[String]{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};return try await session.models()}\n func users()async throws->[Person]{if let authorization")
fixture+=method(api,'bound')+'\n}\n'+helper
if 'struct AccountWork' in app:
 m=re.search(r'^(?:private )?struct AccountWork \{',app,re.M);fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
fixture+=r'''
enum Link{case connecting,live}
final class DeviceStore{}
@MainActor final class AppModel {
 var credentials:Bool?=true;var api:RelayAPI;var accountLifetime=WorkLifetime();var link=Link.live;var relayLink=Link.live;var roomLinks:[String:Int]=[:];var roomLinkSince:[String:Date]=[:];var store:DeviceStore?=DeviceStore();var projects:[String]=[];var peopleChats:[String]=[];var models:[String]=[];var registryTask:Task<Void,Never>?;var storeDocsLoaded=false;var pinsLoaded=false;var calls:[String]=[];var retries=0;var pauseStore=false;var storeWaiter:CheckedContinuation<Void,Never>?;var pauseRegistry=false;var registryWaiter:CheckedContinuation<Void,Never>?;var pausePins=false;var pinsWaiter:CheckedContinuation<Void,Never>?
 init(_ session:Session){api=RelayAPI(session)}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{expectedOwner.map{accountLifetime.accepts($0)} ?? true}
 func replace(_ session:Session){accountLifetime.close();accountLifetime.reopen();registryTask?.cancel();api=RelayAPI(session);models=["B-marker"];storeDocsLoaded=false;pinsLoaded=false;calls=[];retries=0;store=DeviceStore()}
 func startForCheck(){startAfterPairing()}
 func openStore(){};func loadStoreKV(_ store:DeviceStore){};func resumeFirstLoginIfPending(){};func hookPush(){}
 func loadStoreDocs(_ store:DeviceStore,expectedOwner:WorkLifetime.Ticket?=nil)async{if pauseStore{await withCheckedContinuation{storeWaiter=$0}}}
 func retryPendingOpen(){retries+=1}
 func refreshRegistry(expectedOwner:WorkLifetime.Ticket?=nil)async{calls.append("registry");if pauseRegistry{await withCheckedContinuation{registryWaiter=$0}}}
 func refreshAccount(expectedOwner:WorkLifetime.Ticket?=nil)async{calls.append("account")}
 func refreshPeople(expectedOwner:WorkLifetime.Ticket?=nil)async{calls.append("people")}
 func loadDirectory(force:Bool=false,expectedOwner:WorkLifetime.Ticket?=nil)async{}
 func refreshServers(expectedOwner:WorkLifetime.Ticket?=nil)async{calls.append("servers")}
 func refreshPins(expectedOwner:WorkLifetime.Ticket?=nil)async{calls.append("pins");if pausePins{await withCheckedContinuation{pinsWaiter=$0}}}
 func startMatrixIfNeeded(expectedOwner:WorkLifetime.Ticket?=nil)async{calls.append("matrix")}
 func drainPushes(expectedOwner:WorkLifetime.Ticket?=nil)async{calls.append("pushes")}
 func refreshUnread(expectedOwner:WorkLifetime.Ticket?=nil)async{calls.append("unread")}
'''
for name in ['accountWork','boundCallAPI']:
 if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=method(app,name)+'\n'
fixture+=method(app,'startAfterPairing')+'\n}\n'+r'''
@main struct Checks {
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func settle()async{for _ in 0..<100{await Task.yield()}}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{let a=Session("A");a.pauseModels=true;let m=AppModel(a);m.startForCheck();let old=m.registryTask;let began=await spin{a.modelsWaiter != nil};m.replace(Session("B"));a.finishModels();await settle();await old?.value;record("late-model-list-cannot-paint-B",began && m.models==["B-marker"])}
  do{let a=Session("A");let m=AppModel(a);m.pauseStore=true;m.startForCheck();let old=m.registryTask;let began=await spin{m.storeWaiter != nil};m.replace(Session("B"));let saved=m.storeWaiter;m.storeWaiter=nil;saved?.resume();await old?.value;record("late-store-startup-cannot-mark-loaded-or-retry-B",began && !m.storeDocsLoaded && m.retries==0 && m.calls.isEmpty)}
  do{let a=Session("A");let m=AppModel(a);m.pauseRegistry=true;m.startForCheck();let old=m.registryTask;let began=await spin{m.registryWaiter != nil};m.replace(Session("B"));let saved=m.registryWaiter;m.registryWaiter=nil;saved?.resume();await old?.value;record("late-poll-step-cannot-continue-B-pipeline",began && m.calls.isEmpty && m.retries==0 && !m.pinsLoaded)}
  do{let a=Session("A");let m=AppModel(a);m.pausePins=true;m.startForCheck();let old=m.registryTask;let began=await spin{m.pinsWaiter != nil};m.replace(Session("B"));let saved=m.pinsWaiter;m.pinsWaiter=nil;saved?.resume();await old?.value;record("late-pin-step-cannot-mark-B-pins-loaded",began && !m.pinsLoaded && m.calls.isEmpty && m.retries==0)}
  do{let a=Session("A");let m=AppModel(a);m.startForCheck();let old=m.registryTask;let b=Session("B");m.replace(b);await settle();await old?.value;record("queued-startup-model-work-cannot-originate-as-B",b.authorizations.isEmpty && m.models==["B-marker"])}
  do{let a=Session("A");let m=AppModel(a);m.startForCheck();let old=m.registryTask;let completed=await spin{m.calls.contains("unread") && m.models==["A"]};m.registryTask?.cancel();await old?.value;record("current-startup-pipeline-control",completed && m.storeDocsLoaded && m.pinsLoaded && m.calls==["registry","account","people","servers","pins","matrix","pushes","unread"] && m.retries==2)}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-ios-startup-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':sources,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},'Sources changed';(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==6 and row['results']['failed']==0,'Expected6 startup ownership controls'
