"""Actual machine refresh with synthetic paused API results, no machine/provider use."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
P='apps/ios/Arbos/Arbos/';paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};app,api,helper=[p.read_text()for p in paths]
def method(text,name):
 m=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1;start=m[0].start();start=start-len('    @MainActor\n')if text[:start].endswith('    @MainActor\n')else start;return text[start:text.index('\n    }',m[0].end())+6]
fakes=(HERE/'check_directory.py').read_text();start=fakes.index("fixture=r'''")+len("fixture=r'''");end=fakes.index("'''+method(api,'bound')",start);fixture=fakes[start:end]
fixture=fixture.replace('struct User{let id:String}','struct User:Equatable{let id:String}')
fixture=fixture.replace('var authorizations:[String]=[]','var authorizations:[String]=[];var pauseMe=false;var pauseUnread=false;var meWaiter:CheckedContinuation<MeResponse,Error>?;var unreadWaiter:CheckedContinuation<Unread?,Error>?')
fixture=fixture.replace(' func finish(_ value:[Person]?)'," func me()async throws->MeResponse{if pauseMe{return try await withCheckedThrowingContinuation{meWaiter=$0}};return response()}\n func unread()async throws->Unread?{if pauseUnread{return try await withCheckedThrowingContinuation{unreadWaiter=$0}};return (1,[label:1])}\n func finishMe(){let saved=meWaiter;meWaiter=nil;saved?.resume(returning:response())}\n func finishUnread(){let saved=unreadWaiter;unreadWaiter=nil;saved?.resume(returning:(1,[label:1]))}\n func response()->MeResponse{(User(id:label),9,Summary(waitlisted:true,waitlist_position:3,always_on:true,held:2),Features(voice_mode:true),Privacy(group_add:\"nobody\"))}\n func finish(_ value:[Person]?)")
fixture=fixture.replace(' func users()async throws->[Person]{if let authorization'," func me()async throws->MeResponse{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};return try await session.me()}\n func unread()async throws->Unread?{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};return try await session.unread()}\n func users()async throws->[Person]{if let authorization")
fixture+=method(api,'bound')+'\n}\n'+helper
if 'struct AccountWork' in app:
 m=re.search(r'^(?:private )?struct AccountWork \{',app,re.M);fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
fixture+=r'''
struct Summary{let waitlisted:Bool;let waitlist_position:Int64?;let always_on:Bool;let held:Int}
struct Features{let voice_mode:Bool}
struct Privacy{let group_add:String?}
typealias MeResponse=(User,Int,Summary,Features?,Privacy?)
typealias Unread=(total:Int,chats:[String:Int])
enum GroupAddPolicy:String{case anyone,nobody}
@MainActor final class VoiceModeFlag{static let shared=VoiceModeFlag();var value=false;func set(_ value:Bool){self.value=value}}
struct Tokens{var isExpiringSoon=true}
enum State{case awake}
struct Machine:Equatable{var held=0;var alwaysOn=false;var state=State.awake;func coerced(_ state:State)->State{state}}
@MainActor final class AppModel {
 var credentials:Bool?=true;var isFixture=false;var api:RelayAPI;var accountLifetime=WorkLifetime();var tokens:Tokens?;var user:User?;var credits:Int?;var meAlwaysOn:Bool?;var waitlistPosition:Int?;var groupAddPolicy="anyone";var machine=Machine();var accountSaves=0;var unreadCounts:[String:Int]=[:];var unreadSaves=0;var badges=0;var pauseRefresh=false;var refreshWaiter:CheckedContinuation<Bool,Never>?
 init(_ session:Session){api=RelayAPI(session);VoiceModeFlag.shared.value=false}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{if let expectedOwner,!accountLifetime.accepts(expectedOwner){return false};if pauseRefresh{return await withCheckedContinuation{refreshWaiter=$0}};return true}
 func replace(_ session:Session){accountLifetime.close();accountLifetime.reopen();api=RelayAPI(session);user=User(id:"B-marker");credits=7;meAlwaysOn=false;waitlistPosition=nil;machine=Machine();accountSaves=0;unreadCounts=["B-marker":7];unreadSaves=0;badges=0;tokens=nil}
 func saveAccountCache(){accountSaves+=1};func saveUnreadCache(){unreadSaves+=1};func applyBadge(){badges+=1}
'''
for name in ['accountWork','boundCallAPI']:
 if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=method(app,name)+'\n'
fixture+=method(app,'refreshAccount')+'\n'+method(app,'refreshUnread')+'\n}\n'+r'''
@main struct Checks {
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{let a=Session("A");a.pauseUnread=true;let m=AppModel(a);let old=Task{await m.refreshUnread()};let began=await spin{a.unreadWaiter != nil};m.replace(Session("B"));a.finishUnread();await old.value;record("late-unread-cannot-save-or-badge-B",began && m.unreadCounts==["B-marker":7] && m.unreadSaves==0 && m.badges==0)}
  do{let a=Session("A");let m=AppModel(a);let owner=m.accountLifetime.ticket!;let b=Session("B");m.replace(b);await m.refreshUnread(EXPECTED);record("late-unread-entry-refuses-B",b.authorizations.isEmpty && m.unreadSaves==0 && m.badges==0)}
  do{let a=Session("A");let m=AppModel(a);await m.refreshUnread();record("current-unread-control",m.unreadCounts==["A":1] && m.unreadSaves==1 && m.badges==1)}
  do{let a=Session("A");let m=AppModel(a);m.isFixture=true;await m.refreshUnread();record("fixture-unread-still-badges-without-request",a.authorizations.isEmpty && m.badges==1)}
  do{let a=Session("A");a.pauseMe=true;let m=AppModel(a);let old=Task{await m.refreshAccount()};let began=await spin{a.meWaiter != nil};m.replace(Session("B"));a.finishMe();await old.value;record("late-me-cannot-paint-cache-or-feature-B",began && m.user?.id=="B-marker" && m.credits==7 && m.accountSaves==0 && !VoiceModeFlag.shared.value && m.groupAddPolicy=="anyone")}
  do{let a=Session("A");let m=AppModel(a);m.tokens=Tokens();m.pauseRefresh=true;let old=Task{await m.refreshAccount()};let began=await spin{m.refreshWaiter != nil};let b=Session("B");m.replace(b);let saved=m.refreshWaiter;m.refreshWaiter=nil;saved?.resume(returning:true);await old.value;record("late-refresh-followup-cannot-fetch-me-as-B",began && b.authorizations.isEmpty && m.accountSaves==0)}
  do{let a=Session("A");let m=AppModel(a);let owner=m.accountLifetime.ticket!;let b=Session("B");m.replace(b);await m.refreshAccount(EXPECTED);record("late-account-entry-refuses-B",b.authorizations.isEmpty && m.accountSaves==0)}
  do{let a=Session("A");let m=AppModel(a);await m.refreshAccount();record("current-account-control",m.user?.id=="A" && m.credits==9 && m.accountSaves==1 && VoiceModeFlag.shared.value && m.groupAddPolicy=="nobody" && m.waitlistPosition==3 && m.machine.alwaysOn && m.machine.held==2)}
  do{let a=Session("A");let m=AppModel(a);m.tokens=Tokens();await m.refreshAccount();record("same-account-refresh-followup-control",m.user?.id=="A" && m.accountSaves==1 && a.authorizations==["A"])}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
for name in ['refreshUnread','refreshAccount']:fixture=fixture.replace(name+'(EXPECTED)',name+'(expectedOwner:owner)'if re.search(r'func '+name+r'\([^\n]*expectedOwner',app)else name+'()')
start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-ios-poll-data-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':sources,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},'Sources changed';(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==9 and row['results']['failed']==0,'Expected9 account/unread ownership controls'
