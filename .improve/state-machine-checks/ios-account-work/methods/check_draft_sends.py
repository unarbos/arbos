"""Actual draft-DM creation/routing with local paused requests and synthetic sends."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
P='apps/ios/Arbos/Arbos/';paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];queue=tree/(P+'Sync/DraftSendQueue.swift')
if queue.exists():paths.append(queue)
sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};app,api,helper=[p.read_text()for p in paths[:3]]
def method(text,name):
 m=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1,(name,len(m));start=m[0].start();start=start-len('    @MainActor\n')if text[:start].endswith('    @MainActor\n')else start;return text[start:text.index('\n    }',m[0].end())+6]
fakes=(HERE/'check_directory.py').read_text();start=fakes.index("fixture=r'''")+len("fixture=r'''");end=fakes.index("'''+method(api,'bound')",start);fixture=fakes[start:end]
fixture=fixture.replace('var authorizations:[String]=[]','var authorizations:[String]=[];var requests:[String]=[];var waiters:[String:CheckedContinuation<PeopleChat,Error>]=[:]')
fixture=fixture.replace(' func finish(_ value:[Person]?)',' func openDM(_ handle:String)async throws->PeopleChat{requests.append(handle);return try await withCheckedThrowingContinuation{waiters[handle]=$0}}\n func finishDM(_ handle:String,failure:Bool=false){let saved=waiters.removeValue(forKey:handle);if failure{saved?.resume(throwing:RelayError())}else{saved?.resume(returning:PeopleChat(id:"dm-"+handle+"-"+label))}}\n func finish(_ value:[Person]?)')
fixture=fixture.replace(' func users()async throws->[Person]{if let authorization',' func openDM(handle:String)async throws->PeopleChat{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};return try await session.openDM(handle)}\n func users()async throws->[Person]{if let authorization')
fixture+=method(api,'bound')+'\n}\n'+helper
if 'struct AccountWork' in app:
 m=re.search(r'^(?:private )?struct AccountWork \{',app,re.M);fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
if queue.exists():
 fixture+='\n'+queue.read_text()
 a=app.index('    private var draftSends =');b=app.index('\n',a);state=app[a:b]+'\n';a=app.index('    var draftDMSending: Bool {');state+=app[a:app.index('\n    }',a)+6]
else:
 state='    private(set) var draftDMSending = false\n    private var draftDMSends: [(Id) -> Void] = []\n'
fixture+=r'''
typealias Id=String
struct PeopleChat:Equatable{let id:String}
enum Route:Equatable{case chat(chatId:String,projectId:String)}
struct RelayError:Error{let type="not_available"}
struct UserError{let tag:String;static func refused(_ verb:String,_ detail:String)->Self{Self(tag:"refused")};static func classify(_ verb:String,_ error:Error)->Self{Self(tag:verb)}}
let peopleSlot="people"
@MainActor final class AppModel {
 var credentials:Bool?=true;var isFixture=false;var api:RelayAPI;var accountLifetime=WorkLifetime();var peopleChats:[PeopleChat]=[];var ensured:[String]=[];var failures:[String]=[];var delivered:[String]=[];var draftDM:PeopleChat?;var messages:[Id:Int]=[:];var caughtUp:Set<Id>=[];var path:[Route]=[];var clearedDrafts:[String]=[];var trackedTasks:[Task<Void,Never>]=[]
 static let draftDMPrefix="draft-dm:"
'''+state+r'''
 init(_ session:Session){api=RelayAPI(session)}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{expectedOwner.map{accountLifetime.accepts($0)} ?? true}
 func replace(_ session:Session){accountLifetime.close();RESET_DRAFT;accountLifetime.reopen();api=RelayAPI(session);peopleChats=[];ensured=[];failures=[];delivered=[];draftDM=PeopleChat(id:"draft-dm:bob");messages=[:];caughtUp=[];path=[.chat(chatId:"draft-dm:bob",projectId:peopleSlot)]}
 func submit(_ handle:String,_ label:String){let draft=PeopleChat(id:Self.draftDMPrefix+handle);draftDM=draft;messages[draft.id]=0;caughtUp.insert(draft.id);path=[.chat(chatId:draft.id,projectId:peopleSlot)];sendDraftDM(draft){[weak self] id in self?.delivered.append(label+"->"+id)}}
 func trackTask(_ body:@escaping @MainActor ()async->Void){trackedTasks.append(Task{@MainActor in await body()})}
 func ensureRoom(projectId:String,key:String,roomChat:String){ensured.append(key)};func fail(_ value:UserError){failures.append(value.tag)};func setComposerDraft(_ value:String,for id:String){clearedDrafts.append(id)}
'''
for name in ['accountWork','boundCallAPI','retireDraftSends']:
 if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=method(app,name)+'\n'
for name in ['createDM','sendDraftDM']:
 body=method(app,name)
 if name=='sendDraftDM':body=body.replace('Task { @MainActor in','trackTask { @MainActor [self] in',1)
 fixture+=body+'\n'
fixture+='}\n'+r'''
@main struct Checks {
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func settle()async{for _ in 0..<100{await Task.yield()}}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{let a=Session("A");let m=AppModel(a);m.submit("alice","one");m.submit("alice","two");let began=await spin{a.waiters["alice"] != nil};a.finishDM("alice");let done=await spin{m.delivered.count==2};record("same-draft-single-create-FIFO-control",began && done && a.requests==["alice"] && m.delivered==["one->dm-alice-A","two->dm-alice-A"] && m.draftDM==nil && !m.draftDMSending && m.path==[.chat(chatId:"dm-alice-A",projectId:peopleSlot)])}
  do{let a=Session("A");let m=AppModel(a);m.submit("alice","A");let first=await spin{a.waiters["alice"] != nil};m.submit("bob","B");await settle();let second=a.waiters["bob"] != nil;a.finishDM("bob");a.finishDM("alice");let done=await spin{m.delivered.count==2};record("different-drafts-route-only-to-own-recipients",first && second && done && Set(m.delivered)==["A->dm-alice-A","B->dm-bob-A"])}
  do{let a=Session("A");let m=AppModel(a);m.submit("alice","A");let first=await spin{a.waiters["alice"] != nil};m.submit("bob","B");await settle();a.finishDM("alice");let done=await spin{!m.delivered.isEmpty};let kept=m.draftDM?.id=="draft-dm:bob" && m.draftDMSending && m.path==[.chat(chatId:"draft-dm:bob",projectId:peopleSlot)];a.finishDM("bob");await settle();record("A-completion-preserves-B-current-draft-and-pending-state",first && done && kept)}
  do{let a=Session("A");let m=AppModel(a);m.submit("alice","failed");let first=await spin{a.waiters["alice"] != nil};a.finishDM("alice",failure:true);let failed=await spin{!m.draftDMSending};let kept=m.draftDM?.id=="draft-dm:alice" && m.delivered.isEmpty && m.failures==["refused"];m.submit("alice","retry");let retry=await spin{a.waiters["alice"] != nil};a.finishDM("alice");let done=await spin{m.delivered.count==1};record("current-failure-keeps-draft-and-retry-policy",first && failed && kept && retry && done && a.requests==["alice","alice"] && m.delivered==["retry->dm-alice-A"])}
  do{let a=Session("A");let m=AppModel(a);m.submit("alice","old");let began=await spin{a.waiters["alice"] != nil};let b=Session("B");m.replace(b);m.submit("bob","new");await settle();a.finishDM("alice");await settle();let kept=m.delivered.isEmpty && m.peopleChats.isEmpty && m.ensured.isEmpty && m.draftDM?.id=="draft-dm:bob" && m.draftDMSending;b.finishDM("bob");await settle();record("retired-A-success-cannot-drain-or-clear-B",began && kept && m.delivered==["new->dm-bob-B"])}
  do{let a=Session("A");let m=AppModel(a);m.submit("alice","old");let began=await spin{a.waiters["alice"] != nil};let b=Session("B");m.replace(b);m.submit("bob","new");await settle();a.finishDM("alice",failure:true);await settle();let kept=m.failures.isEmpty && m.draftDM?.id=="draft-dm:bob" && m.draftDMSending;b.finishDM("bob");await settle();record("retired-A-error-cannot-fail-or-clear-B",began && kept && m.delivered==["new->dm-bob-B"])}
  do{let a=Session("A");let m=AppModel(a);m.submit("alice","old");let b=Session("B");m.replace(b);await settle();let untouched=b.requests.isEmpty && m.delivered.isEmpty;b.finishDM("alice");a.finishDM("alice");await settle();record("queued-old-draft-task-cannot-originate-as-B",untouched)}
  do{let a=Session("A");let m=AppModel(a);m.submit("alice","old");let began=await spin{a.waiters["alice"] != nil};let b=Session("B");m.replace(b);m.submit("alice","new");await settle();let own=b.waiters["alice"] != nil;b.finishDM("alice");await settle();a.finishDM("alice");await settle();record("same-draft-new-account-starts-independent-current-request",began && own && m.delivered==["new->dm-alice-B"] && m.peopleChats==[PeopleChat(id:"dm-alice-B")])}
  do{let a=Session("A");let m=AppModel(a);m.submit("alice","admitted");let began=await spin{a.waiters["alice"] != nil};m.trackedTasks.last?.cancel();a.finishDM("alice");await settle();record("admitted-same-account-send-finishes-after-caller-cancellation",began && !m.draftDMSending && m.delivered==["admitted->dm-alice-A"])}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
if queue.exists():assert 'retireDraftSends()'in method(app,'unpair'),'The real account teardown must retire the queue'
fixture=fixture.replace('RESET_DRAFT;','retireDraftSends();'if queue.exists()else'')
start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-ios-draft-sends-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':sources,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},'Sources changed';(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==9 and row['results']['failed']==0,'Expected9 draft routing/ownership controls'
