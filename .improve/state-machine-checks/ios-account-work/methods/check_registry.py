"""Actual registry refresh/controller helpers with paused local API/machine edges."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
P='apps/ios/Arbos/Arbos/';paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};app,api,helper=[p.read_text()for p in paths]
def method(text,name):
 m=list(re.finditer(r'^    (?:private )?(?:nonisolated )?(?:static )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1,(name,len(m));start=m[0].start();start=start-len('    @MainActor\n')if text[:start].endswith('    @MainActor\n')else start;return text[start:text.index('\n    }',m[0].end())+6]
# Shared collaborators only, never a copied registry implementation.
fakes=(HERE/'check_directory.py').read_text();start=fakes.index("fixture=r'''")+len("fixture=r'''");end=fakes.index("'''+method(api,'bound')",start);fixture=fakes[start:end]
fixture=fixture.replace('var authorizations:[String]=[]','var authorizations:[String]=[];var pauseRegistry=false;var registryWaiter:CheckedContinuation<RegistryResult,Error>?;var registryResult=RegistryResult(projects:[],skippedIds:[],incomplete:false)')
fixture=fixture.replace(' func finish(_ value:[Person]?)',' func registry()async throws->RegistryResult{if pauseRegistry{return try await withCheckedThrowingContinuation{registryWaiter=$0}};return registryResult}\n func finishRegistry(_ error:Bool=false){let saved=registryWaiter;registryWaiter=nil;if error{saved?.resume(throwing:RelayError(status:401))}else{saved?.resume(returning:registryResult)}}\n func finish(_ value:[Person]?)')
fixture=fixture.replace(' func users()async throws->[Person]{if let authorization',' func registry()async throws->RegistryResult{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};return try await session.registry()}\n func users()async throws->[Person]{if let authorization')
fixture+=method(api,'bound')+'\n}\n'+helper
if 'struct AccountWork' in app:
 m=re.search(r'^(?:private )?struct AccountWork \{',app,re.M);assert m;fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
fixture+=r'''
typealias Id=String
struct Project:Equatable {var id:String;var name:String;var state="awake";var stateDetail:String?;var modelNote="";var workspace="unknown"}
struct Chat:Equatable {var id:String;var projectId:String;var unread=false;var lastMessageAt:Int64=0;var title="New chat"}
struct RegistryProject {var project:Project;var chats:[Chat]}
struct RegistryResult {var projects:[RegistryProject];var skippedIds:Set<String>;var incomplete:Bool}
struct Room {let projectId:String}
enum LinkState:Equatable {case connecting,live,reconnecting,offline,relayDown;static func fault(_:Error)->Self{.offline}}
struct RelayError:Error {let status:Int;var isUnauthorized:Bool{status==401}}
struct UserError:Equatable {let tag:String;static let authExpired=Self(tag:"auth");static func of(_ verb:String,_ type:Self,_ detail:String)->Self{type};static func classify(_ verb:String,_ error:Error)->Self{Self(tag:verb)}}
let peopleSlot="people";let projectDocChatId="project"
func projectDocKey(_ id:String)->String{"proj:\(id)"}
@MainActor final class AppModel {
 var credentials:Bool?=true;var isFixture=false;var api:RelayAPI;var accountLifetime=WorkLifetime();var projects:[Project]=[];var chats:[Id:[Chat]]=[:];var registryError:UserError?;var pairingError:UserError?;var link=LinkState.live;var rooms:[Id:Room]=[:];var docs:[Id:Int]=[:];var roomCreatedAt:[Id:Date]=[:];var peopleChats:[Project]=[]
 var saved=0;var ensured:[String]=[];var wiped:[String]=[];var firstLoginCalls=0;var unpairCount=0;var pauseMachine=false;var machineWaiter:CheckedContinuation<Void,Never>?
 nonisolated static let roomRecentGraceSeconds:TimeInterval=60
 init(_ session:Session){api=RelayAPI(session)}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{expectedOwner.map{accountLifetime.accepts($0)} ?? true}
 func replace(_ session:Session){accountLifetime.close();accountLifetime.reopen();api=RelayAPI(session);projects=[Project(id:"B",name:"B")];chats=[:];saved=0;ensured=[];wiped=[];firstLoginCalls=0;registryError=UserError(tag:"B-marker");rooms=["B":Room(projectId:"B")];docs=["B":1]}
 func reconcileAgentAvatars(){};func reconcileArchivedProjects(_ values:[Project]){};func saveRegistryCache(){saved+=1};func setRelayLink(_ value:LinkState){link=value}
 func refreshMachine(expectedOwner:WorkLifetime.Ticket?=nil)async{if pauseMachine{await withCheckedContinuation{machineWaiter=$0}}}
 func ensureRoom(projectId:String,key:String,roomChat:String){ensured.append(key)};func wipeRoom(_ key:String){wiped.append(key)};func firstLoginRegistryUpdated(){firstLoginCalls+=1}
 func unpair(forget:Bool){unpairCount+=1;credentials=nil};static func sorted(_ values:[Chat])->[Chat]{values.sorted{$0.id<$1.id}}
'''
for name in ['accountWork','boundCallAPI']:
 if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=method(app,name)+'\n'
for name in ['refreshRegistry','mergeSkippedProjects','shouldPrune','roomsToPrune']:fixture+=method(app,name)+'\n'
fixture+='}\n'+r'''
@main struct Checks {
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func result(_ id:String)->RegistryResult{RegistryResult(projects:[RegistryProject(project:Project(id:id,name:id),chats:[Chat(id:"chat-\(id)",projectId:id)])],skippedIds:[],incomplete:false)}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{let a=Session("A");a.pauseRegistry=true;a.registryResult=result("A");let m=AppModel(a);let old=Task{await m.refreshRegistry()};let began=await spin{a.registryWaiter != nil};m.replace(Session("B"));a.finishRegistry();await old.value;record("late-registry-success-cannot-paint-save-or-create-B-rooms",began && m.projects.map(\.id)==["B"] && m.saved==0 && m.ensured.isEmpty && m.wiped.isEmpty && m.registryError?.tag=="B-marker")}
  do{let a=Session("A");a.pauseRegistry=true;let m=AppModel(a);let old=Task{await m.refreshRegistry()};let began=await spin{a.registryWaiter != nil};m.replace(Session("B"));a.finishRegistry(true);await old.value;record("late-registry-refusal-cannot-unpair-B",began && m.credentials != nil && m.unpairCount==0 && m.pairingError==nil)}
  do{let a=Session("A");a.registryResult=result("A");let m=AppModel(a);m.pauseMachine=true;let old=Task{await m.refreshRegistry()};let began=await spin{m.machineWaiter != nil};m.replace(Session("B"));let saved=m.machineWaiter;m.machineWaiter=nil;saved?.resume();await old.value;record("late-machine-followup-cannot-create-or-prune-B-rooms",began && m.ensured.isEmpty && m.wiped.isEmpty && m.firstLoginCalls==0)}
  do{let a=Session("A");let m=AppModel(a);let owner=m.accountLifetime.ticket!;let b=Session("B");m.replace(b);await m.refreshRegistry(EXPECTED);record("late-registry-poll-entry-cannot-admit-B",b.authorizations.isEmpty && m.saved==0 && m.registryError?.tag=="B-marker")}
  do{let a=Session("A");a.registryResult=result("A");let m=AppModel(a);await m.refreshRegistry();record("current-registry-saves-and-creates-rooms",m.projects.map(\.id)==["A"] && m.saved==1 && Set(m.ensured)==Set(["proj:A","chat-A"]) && m.firstLoginCalls==1)}
  do{let a=Session("A");a.registryResult=RegistryResult(projects:[],skippedIds:["cached"],incomplete:true);let m=AppModel(a);m.projects=[Project(id:"cached",name:"cached")];m.chats=["cached":[Chat(id:"saved",projectId:"cached",unread:true)]];m.rooms=["keep":Room(projectId:"unlisted")];await m.refreshRegistry();record("skipped-incomplete-cache-fallback-control",m.projects.map(\.id)==["cached"] && m.chats["cached"]?.first?.unread==true && m.wiped.isEmpty)}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
fixture=fixture.replace('refreshRegistry(EXPECTED)','refreshRegistry(expectedOwner:owner)'if re.search(r'func refreshRegistry\([^\n]*expectedOwner',app)else'refreshRegistry()')
start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-ios-registry-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':sources,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},'Sources changed';(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==6 and row['results']['failed']==0,'Expected6 registry ownership controls'
