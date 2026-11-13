"""Compile actual settings controllers against paused synthetic collaborators, no network."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
P='apps/ios/Arbos/Arbos/'
paths=[tree/(P+s)for s in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift','Store/SecretsStore.swift','Store/DevicesStore.swift']]
app,api,helper,secrets,devices=[x.read_text()for x in paths];hashes={str(x.relative_to(tree)):hashlib.sha256(x.read_bytes()).hexdigest()for x in paths}
def method(text,name):
 m=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1,(name,len(m));return text[m[0].start():text.index('\n    }',m[0].end())+6]
fixture=r'''
import Foundation
typealias Id=String
struct UserError:Error {let sentence:String;static func classify(_ verb:String,_ error:Error)->UserError{UserError(sentence:verb)} }
struct RelayError:Error{let status:Int}
struct SecretAgentAccess{let projectId:String;var enabled:Bool}
struct SecretDirector{let userId:String;var enabled:Bool;var disabledAt:Int64?}
struct SecretItem{let id:String;var agents:[SecretAgentAccess]=[];var directors:[SecretDirector]=[];var injectEnv=false;var askEveryTime=false;var othersAsk="card"}
struct ConnectedVault{let id:String;var agents:[SecretAgentAccess]=[]}
struct SecretsOverview{var paused:Bool;let pausedAt:Int64?;var vaults:[ConnectedVault];var secrets:[SecretItem];let requestsPending:Int;let activity:[String];var rowValue:String?{nil};var vaultConnect:Bool{false};var providers:[String]{[]}}
struct ProjectSecretAccess{var secret:SecretItem?;var vault:ConnectedVault?;var enabled:Bool}
struct ProjectSecrets{var items:[ProjectSecretAccess]}
struct GoalItem{let id:String;let projectId:String;var status:String="active";var title:String="";var nextStep:String="";var autonomy:String="";var checkInCron:String?;var isOpen:Bool{status=="active"}}
func overview(_ label:String)->SecretsOverview{SecretsOverview(paused:false,pausedAt:nil,vaults:[],secrets:[SecretItem(id:label)],requestsPending:0,activity:[])}
@MainActor private final class CallAuthority{var bearer:String;init(_ value:String){bearer=value}}
@MainActor final class Session{
 let label:String;var calls:[String]=[];var authorizations:[String]=[];var pauses:Set<String>=[];var failures:Set<String>=[];var waiters:[String:CheckedContinuation<Void,Error>]=[:]
 init(_ label:String){self.label=label}
 func call(_ op:String)async throws{calls.append(op);if pauses.contains(op){try await withCheckedThrowingContinuation{waiters[op]=$0}}else if failures.contains(op){throw RelayError(status:500)}}
 func finish(_ op:String,_ status:Int?=nil){let c=waiters.removeValue(forKey:op);if let status{c?.resume(throwing:RelayError(status:status))}else{c?.resume()}}
}
@MainActor final class RelayAPI{
 let endpoint:String;var token:()->String;var refresher:(()async->Bool)?;let session:Session;private var authorization:(@MainActor ()->String?)?;private var cleanupAuthority:CallAuthority?
 init(_ session:Session){self.session=session;endpoint="http://synthetic.invalid";token={session.label}}
 private init(endpoint:String,token:@escaping()->String,session:Session){self.endpoint=endpoint;self.token=token;self.session=session}
 func call(_ op:String)async throws{if let authorization{guard let value=authorization()else{throw CancellationError()};session.authorizations.append(value)}else{session.authorizations.append(token())};try await session.call(op)}
'''+method(api,'bound')+'\n'
methods=[
('secrets()','SecretsOverview','secrets','overview(session.label)'),('projectSecrets(_ id:String)','ProjectSecrets','projectSecrets','ProjectSecrets(items:[])'),('goals()','[GoalItem]','goals','[GoalItem(id:session.label,projectId:"p")]'),('devices()','[AuthDevice]','devices','[AuthDevice(id:session.label,name:"",kind:"",createdAt:0,lastSeenAt:nil,current:false)]'),
('setSecretsPaused(_ value:Bool)','Void','pause','()'),('setSecretAccess(_ id:String,projectId:String,enabled:Bool,note:String?=nil,hosts:[String]?=nil,scope:[String:Any]?=nil)','Void','access','()'),('deleteSecretAccess(_ id:String,projectId:String)','Void','deleteAccess','()'),('revokeSecretEverywhere(_ id:String)','Void','revokeEverywhere','()'),('scrubSecret(_ id:String)','Void','scrub','()'),('deleteSecret(_ id:String)','Void','deleteSecret','()'),('patchSecret(_ id:String,_ body:[String:Any])','Void','patchSecret','()'),
('setSecretDirector(_ id:String,userId:String,note:String?,enabled:Bool?=nil)','SecretItem','director','SecretItem(id:session.label)'),('addSecretDirector(_ id:String,handle:String,note:String?)','SecretItem','addDirector:'+ '"+handle+"','SecretItem(id:session.label)'),('deleteSecretDirector(_ id:String,userId:String)','SecretItem','deleteDirector','SecretItem(id:session.label)'),('decideSecretRequest(_ id:String,decision:String)','Void','decision','()'),('setVaultAccess(_ id:String,projectId:String,enabled:Bool,note:String?)','Void','vaultAccess','()'),('deleteVault(_ id:String)','Void','deleteVault','()'),
('createGoal(projectId:String,_ body:[String:Any])','Void','createGoal','()'),('patchGoal(projectId:String,goalId:String,_ body:[String:Any])','Void','patchGoal','()'),('deleteGoal(projectId:String,goalId:String)','Void','deleteGoal','()'),('revokeDevice(_ id:String)','Void','revokeDevice','()')]
for signature,ret,op,val in methods:fixture+=f' func {signature}async throws->{ret}{{try await call("{op}");return {val}}}\n'
fixture+='}\n'+helper+'\n'+devices[devices.index('struct AuthDevice:'):devices.index('\nextension RelayAPI')]
m=re.search(r'^struct AccountWork \{',app,re.M);fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
fixture+=r'''
@MainActor final class AppModel{
 var credentials:Bool?=true;var isFixture=false;var api:RelayAPI;var relay:RelayAPI{api};var accountLifetime=WorkLifetime();var secrets:SecretsOverview?;var projectSecrets:[Id:ProjectSecrets]=[:];var secretsBusy=false;var goalsBusy=false;var goals:[GoalItem]=[];var devices:[AuthDevice]=[];var devicesLoaded=false;var devicesError:UserError?;var notices:[String]=[]
 init(_ session:Session){api=RelayAPI(session);secrets=overview(session.label)}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{expectedOwner.map{accountLifetime.accepts($0)} ?? true}
 func fail(_ error:UserError){notices.append(error.sentence)}
 func fail(_ verb:String,_ error:Error){notices.append(verb)}
 func replace(_ session:Session){accountLifetime.close();accountLifetime.reopen();api=RelayAPI(session);secrets=overview("B-marker");goals=[GoalItem(id:"B-marker",projectId:"p")];devices=[AuthDevice(id:"B-marker",name:"",kind:"",createdAt:0,lastSeenAt:nil,current:false)];devicesLoaded=false;devicesError=nil;notices=[];secretsBusy=false;goalsBusy=false}
'''+method(app,'accountWork')+'\n}\n'+secrets+'\n'+devices[devices.index('\nextension AppModel'):]
fixture+=r'''
@main struct Checks{
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{let a=Session("A");let m=AppModel(a);m.setSecretsPaused(true);let b=Session("B");m.replace(b);for _ in 0..<100{await Task.yield()};record("secret-task-cannot-mint-B-authority",b.calls.isEmpty && b.authorizations.isEmpty)}
  do{let a=Session("A");a.pauses=["director"];let m=AppModel(a);m.setSecretPerson(secretId:"A",userId:"u",enabled:true);let began=await spin{a.waiters["director"] != nil};let b=Session("B");m.replace(b);m.secretsBusy=true;a.finish("director");for _ in 0..<100{await Task.yield()};record("secret-reply-no-B-write-refresh-or-busy-release",began && m.secrets?.secrets.first?.id=="B-marker" && b.calls.isEmpty && m.secretsBusy)}
  do{let a=Session("A");a.pauses=["director"];let m=AppModel(a);m.setSecretPerson(secretId:"A",userId:"u",enabled:true);let began=await spin{a.waiters["director"] != nil};m.replace(Session("B"));a.finish("director",500);for _ in 0..<100{await Task.yield()};record("late-secret-error-cannot-notify-B",began && m.notices.isEmpty)}
  do{let a=Session("A");let m=AppModel(a);m.setSecretsPaused(true);let ended=await spin{a.calls.contains("secrets") && !m.secretsBusy};record("current-secret-mutation-refresh-control",ended && a.calls==["pause","secrets"] && Set(a.authorizations)==["A"])}
  do{let a=Session("A");a.pauses=["addDirector:one"];let m=AppModel(a);let old=Task{await m.addSecretPeople(secretId:"A",handles:["one","two"])};let began=await spin{a.waiters["addDirector:one"] != nil};let b=Session("B");m.replace(b);a.finish("addDirector:one");let ok=await old.value;record("multi-handle-retirement-stops-next-effect",began && !ok && b.calls.isEmpty && a.calls==["addDirector:one"])}
  do{let a=Session("A");let m=AppModel(a);let ok=await m.addSecretPeople(secretId:"A",handles:["one","two"]);record("current-multi-handle-FIFO-control",ok && a.calls==["addDirector:one","secrets","addDirector:two","secrets"])}
  do{let a=Session("A");a.failures=["addDirector:one"];let m=AppModel(a);let ok=await m.addSecretPeople(secretId:"A",handles:["one","two"]);record("per-handle-error-continues-control",!ok && a.calls==["addDirector:one","addDirector:two","secrets"] && m.notices.count==1)}
  do{let a=Session("A");let m=AppModel(a);m.createGoal(projectId:"p",title:"t",outcome:"o",nextStep:"n",checkInCron:nil,autonomy:"ask");let b=Session("B");m.replace(b);for _ in 0..<100{await Task.yield()};record("goal-task-cannot-mint-B-authority",b.calls.isEmpty)}
  do{let a=Session("A");a.pauses=["goals"];let m=AppModel(a);let old=Task{await m.refreshGoals()};let began=await spin{a.waiters["goals"] != nil};m.replace(Session("B"));a.finish("goals");await old.value;record("late-goals-cannot-paint-B",began && m.goals.first?.id=="B-marker")}
  do{let a=Session("A");a.pauses=["goals"];let m=AppModel(a);let old=Task{await m.refreshGoals()};let began=await spin{a.waiters["goals"] != nil};m.replace(Session("B"));a.finish("goals",404);await old.value;record("late-goals404-cannot-clear-B",began && m.goals.first?.id=="B-marker")}
  do{let a=Session("A");let m=AppModel(a);m.createGoal(projectId:"p",title:"t",outcome:"o",nextStep:"n",checkInCron:nil,autonomy:"ask");let ended=await spin{a.calls.contains("goals") && !m.goalsBusy};record("current-goal-control",ended && a.calls==["createGoal","goals"] && m.goals.first?.id=="A")}
  do{let a=Session("A");a.pauses=["createGoal"];let m=AppModel(a);m.createGoal(projectId:"p",title:"t",outcome:"o",nextStep:"n",checkInCron:nil,autonomy:"ask");let began=await spin{a.waiters["createGoal"] != nil};let b=Session("B");m.replace(b);m.goalsBusy=true;a.finish("createGoal",500);for _ in 0..<100{await Task.yield()};record("late-goal-error-busy-owner",began && m.notices.isEmpty && m.goalsBusy && b.calls.isEmpty)}
  do{let a=Session("A");a.pauses=["devices"];let m=AppModel(a);let old=Task{await m.refreshDevices()};let began=await spin{a.waiters["devices"] != nil};m.replace(Session("B"));a.finish("devices");await old.value;record("late-devices-cannot-paint-B",began && m.devices.first?.id=="B-marker" && !m.devicesLoaded)}
  do{let a=Session("A");a.pauses=["devices"];let m=AppModel(a);let old=Task{await m.refreshDevices()};let began=await spin{a.waiters["devices"] != nil};m.replace(Session("B"));a.finish("devices",500);await old.value;record("late-device-error-cannot-paint-B",began && m.devicesError==nil && !m.devicesLoaded)}
  do{let a=Session("A");let m=AppModel(a);m.devices=AuthDevice.fixture();m.revokeDevice("dev-fx-mac");let b=Session("B");m.replace(b);for _ in 0..<100{await Task.yield()};record("revoke-task-cannot-mint-B-authority",b.calls.isEmpty)}
  do{let a=Session("A");a.pauses=["revokeDevice"];let m=AppModel(a);m.devices=AuthDevice.fixture();m.revokeDevice("dev-fx-mac");let began=await spin{a.waiters["revokeDevice"] != nil};let b=Session("B");m.replace(b);a.finish("revokeDevice",500);for _ in 0..<100{await Task.yield()};record("retired-revoke-no-B-error-or-refresh",began && b.calls.isEmpty && m.notices.isEmpty && m.devices.first?.id=="B-marker")}
  do{let a=Session("A");let m=AppModel(a);m.devices=AuthDevice.fixture();m.revokeDevice("dev-fx-iphone");for _ in 0..<20{await Task.yield()};record("current-device-refused-control",m.devices.count==3 && a.calls.isEmpty)}
  do{let a=Session("A");let m=AppModel(a);m.isFixture=true;await m.refreshDevices();m.revokeDevice("dev-fx-mac");record("fixture-local-device-control",m.devices.count==2 && m.devicesLoaded && a.calls.isEmpty)}
  do{let a=Session("A");let m=AppModel(a);await m.refreshDevices();record("current-device-load-control",m.devices.first?.id=="A" && m.devicesLoaded && m.devicesError==nil)}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
(out/'fixture.swift').write_text(fixture)
start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-settings-actions-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':hashes,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=25);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert hashes=={str(x.relative_to(tree)):hashlib.sha256(x.read_bytes()).hexdigest()for x in paths};(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row))
 assert c.returncode==0 and row['results']['total']==19,'Compile and count guard';exit(row['results']['failed']!=0)
