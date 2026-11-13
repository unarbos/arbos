"""Actual machine refresh with synthetic paused API results, no machine/provider use."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
P='apps/ios/Arbos/Arbos/';paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};app,api,helper=[p.read_text()for p in paths]
def method(text,name):
 m=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1;start=m[0].start();start=start-len('    @MainActor\n')if text[:start].endswith('    @MainActor\n')else start;return text[start:text.index('\n    }',m[0].end())+6]
fakes=(HERE/'check_directory.py').read_text();start=fakes.index("fixture=r'''")+len("fixture=r'''");end=fakes.index("'''+method(api,'bound')",start);fixture=fakes[start:end]
fixture=fixture.replace('var authorizations:[String]=[]','var authorizations:[String]=[];var pausePeople=false;var peopleWaiter:CheckedContinuation<[Any],Error>?;var peopleResult:[Any]=[]')
fixture=fixture.replace(' func finish(_ value:[Person]?)'," func peopleChatsJSON()async throws->[Any]{if pausePeople{return try await withCheckedThrowingContinuation{peopleWaiter=$0}};return peopleResult}\n func finishPeople(){let saved=peopleWaiter;peopleWaiter=nil;saved?.resume(returning:peopleResult)}\n func finish(_ value:[Person]?)")
fixture=fixture.replace(' func users()async throws->[Person]{if let authorization'," func peopleChatsJSON()async throws->[Any]{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};return try await session.peopleChatsJSON()}\n func users()async throws->[Person]{if let authorization")
fixture+=method(api,'bound')+'\n}\n'+helper
if 'struct AccountWork' in app:
 m=re.search(r'^(?:private )?struct AccountWork \{',app,re.M);fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
fixture+=r'''
typealias Id=String
struct PeopleChat:Equatable {let id:String;let unread:Bool;static func parse(_ value:Any)->Self?{value as? Self}}
struct Room {let projectId:String}
enum Role{case system,user}
struct Message{var pending=false;var role=Role.user;var sender:String?="peer"}
let peopleSlot="people"
@MainActor final class AppModel {
 var credentials:Bool?=true;var isFixture=false;var api:RelayAPI;var accountLifetime=WorkLifetime();var peopleChats:[PeopleChat]=[];var peopleUnread:Set<String>=[];var visibleChatId:Id?;var myUserId:Id?="me";var messages:[Id:[Message]]=[:];var rooms:[Id:Room]=[:];var docs:[Id:Int]=[:];var roomCreatedAt:[Id:Date]=[:];var saved=0;var ensured:[String]=[];var wiped:[String]=[]
 nonisolated static let roomRecentGraceSeconds:TimeInterval=60
 init(_ session:Session){api=RelayAPI(session)}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{expectedOwner.map{accountLifetime.accepts($0)} ?? true}
 func replace(_ session:Session){accountLifetime.close();accountLifetime.reopen();api=RelayAPI(session);peopleChats=[PeopleChat(id:"B-marker",unread:false)];peopleUnread=["B-unread"];rooms=["B-marker":Room(projectId:peopleSlot)];docs=["B-marker":1];saved=0;ensured=[];wiped=[]}
 func reconcileAgentAvatars(){};func reconcileArchivedPeople(_ list:[PeopleChat]){};func savePeopleCache(_ raw:[Any]){saved+=1};func ensureRoom(projectId:String,key:String,roomChat:String){ensured.append(key)};func wipeRoom(_ key:String){wiped.append(key)}
'''
for name in ['accountWork','boundCallAPI']:
 if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=method(app,name)+'\n'
fixture+=method(app,'refreshPeople')+'\n}\n'+r'''
@main struct Checks {
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{let a=Session("A");a.pausePeople=true;a.peopleResult=[PeopleChat(id:"A",unread:true)];let m=AppModel(a);let old=Task{await m.refreshPeople()};let began=await spin{a.peopleWaiter != nil};m.replace(Session("B"));a.finishPeople();await old.value;record("late-people-list-cannot-paint-prune-or-save-B",began && m.peopleChats==[PeopleChat(id:"B-marker",unread:false)] && m.peopleUnread==["B-unread"] && m.saved==0 && m.ensured.isEmpty && m.wiped.isEmpty)}
  do{let a=Session("A");let m=AppModel(a);let owner=m.accountLifetime.ticket!;let b=Session("B");m.replace(b);await m.refreshPeople(EXPECTED);record("late-people-entry-refuses-B",b.authorizations.isEmpty && m.saved==0)}
  do{let a=Session("A");a.pausePeople=true;a.peopleResult=[PeopleChat(id:"A",unread:true)];let m=AppModel(a);let old=Task{await m.refreshPeople()};let began=await spin{a.peopleWaiter != nil};old.cancel();a.finishPeople();await old.value;record("cancelled-people-work-cannot-publish",began && m.peopleChats.isEmpty && m.saved==0)}
  do{let a=Session("A");a.peopleResult=[PeopleChat(id:"new",unread:true),PeopleChat(id:"incoming",unread:false),PeopleChat(id:"read",unread:false)];let m=AppModel(a);m.peopleUnread=["incoming","read"];m.messages=["incoming":[Message()]];m.rooms=["old":Room(projectId:peopleSlot),"recent":Room(projectId:peopleSlot),"project":Room(projectId:"project")];m.roomCreatedAt=["recent":Date()];await m.refreshPeople();record("current-unread-recent-room-prune-control",m.peopleUnread==["new","incoming"] && m.saved==1 && Set(m.ensured)==["new","incoming","read"] && m.wiped==["old"])}
  do{let a=Session("A");let m=AppModel(a);m.isFixture=true;await m.refreshPeople();record("fixture-people-no-request-control",a.authorizations.isEmpty && m.saved==0)}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
fixture=fixture.replace('refreshPeople(EXPECTED)','refreshPeople(expectedOwner:owner)'if re.search(r'func refreshPeople\([^\n]*expectedOwner',app)else'refreshPeople()')
start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-ios-people-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':sources,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},'Sources changed';(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==5 and row['results']['failed']==0,'Expected5 people ownership controls'
