"""Actual machine refresh with synthetic paused API results, no machine/provider use."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
P='apps/ios/Arbos/Arbos/';paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift','Store/ServerStore.swift']];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};app,api,helper,server=[p.read_text()for p in paths]
def method(text,name):
 m=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1;start=m[0].start();start=start-len('    @MainActor\n')if text[:start].endswith('    @MainActor\n')else start;return text[start:text.index('\n    }',m[0].end())+6]
fakes=(HERE/'check_directory.py').read_text();start=fakes.index("fixture=r'''")+len("fixture=r'''");end=fakes.index("'''+method(api,'bound')",start);fixture=fakes[start:end]
fixture=fixture.replace('var authorizations:[String]=[]','var authorizations:[String]=[];var pauseServers=false;var serversWaiter:CheckedContinuation<[Server],Error>?;var serversResult:[Server]=[];var serversFailure:Int?;var pauseServer=false;var serverWaiter:CheckedContinuation<Server,Error>?')
fixture=fixture.replace(' func finish(_ value:[Person]?)'," func servers()async throws->[Server]{if pauseServers{return try await withCheckedThrowingContinuation{serversWaiter=$0}};if let status=serversFailure{throw RelayError(status:status)};return serversResult}\n func server(_ id:String)async throws->Server{if pauseServer{return try await withCheckedThrowingContinuation{serverWaiter=$0}};return Server(id:id,members:[label])}\n func finishServers(_ failure:Int?=nil){let saved=serversWaiter;serversWaiter=nil;if let failure{saved?.resume(throwing:RelayError(status:failure))}else{saved?.resume(returning:serversResult)}}\n func finishServer(){let saved=serverWaiter;serverWaiter=nil;saved?.resume(returning:Server(id:\"same\",members:[label]))}\n func finish(_ value:[Person]?)")
fixture=fixture.replace(' func users()async throws->[Person]{if let authorization'," func servers()async throws->[Server]{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};return try await session.servers()}\n func server(_ id:String)async throws->Server{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};return try await session.server(id)}\n func users()async throws->[Person]{if let authorization")
fixture+=method(api,'bound')+'\n}\n'+helper
if 'struct AccountWork' in app:
 m=re.search(r'^(?:private )?struct AccountWork \{',app,re.M);fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
fixture+=r'''
typealias Id=String
struct Server:Equatable {let id:String;var members:[String]=[]}
struct RelayError:Error{let status:Int}
@MainActor final class AppModel {
 var credentials:Bool?=true;var isFixture=false;var api:RelayAPI;var relay:RelayAPI{api};var accountLifetime=WorkLifetime();var servers:[Server]=[];var serversAvailable=false;var remembered:[Bool]=[];var serverRostersRequested:Set<String>=[];var senderRefreshes=0;var hookCalls=0
 init(_ session:Session){api=RelayAPI(session)}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{expectedOwner.map{accountLifetime.accepts($0)} ?? true}
 func replace(_ session:Session){accountLifetime.close();accountLifetime.reopen();api=RelayAPI(session);servers=[Server(id:"B-marker",members:["B"])];serversAvailable=true;remembered=[];serverRostersRequested=[];senderRefreshes=0;hookCalls=0}
 func server(_ id:String)->Server?{servers.first{$0.id==id}};func reconcileArchivedServers(_ values:[Server]){};func rememberServersAvailable(_ value:Bool){remembered.append(value)};func refreshChannelSenders(){senderRefreshes+=1};func applyOpenServerHook(){hookCalls+=1}
'''
for name in ['accountWork','boundCallAPI']:
 if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=method(app,name)+'\n'
for name in ['refreshServers','refreshServer','wantServerRosters','upsert']:fixture+=method(server,name)+'\n'
fixture+='}\n'+r'''
@main struct Checks {
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func settle()async{for _ in 0..<100{await Task.yield()}}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{let a=Session("A");a.pauseServers=true;a.serversResult=[Server(id:"A",members:["A"])];let m=AppModel(a);let old=Task{await m.refreshServers()};let began=await spin{a.serversWaiter != nil};m.replace(Session("B"));a.finishServers();await old.value;record("late-server-list-cannot-paint-save-or-hook-B",began && m.servers==[Server(id:"B-marker",members:["B"])] && m.remembered.isEmpty && m.senderRefreshes==0 && m.hookCalls==0)}
  do{let a=Session("A");a.pauseServers=true;let m=AppModel(a);let old=Task{await m.refreshServers()};let began=await spin{a.serversWaiter != nil};m.replace(Session("B"));a.finishServers(503);await old.value;record("late-server-offline-cannot-clear-B",began && m.servers==[Server(id:"B-marker",members:["B"])] && m.serversAvailable && m.remembered.isEmpty)}
  do{let a=Session("A");a.pauseServer=true;let m=AppModel(a);let old=Task{await m.refreshServer("same")};let began=await spin{a.serverWaiter != nil};m.replace(Session("B"));a.finishServer();let value=await old.value;record("late-server-detail-cannot-upsert-B",began && value==nil && m.servers==[Server(id:"B-marker",members:["B"])] && m.senderRefreshes==0)}
  do{let a=Session("A");let m=AppModel(a);m.servers=[Server(id:"same")];m.wantServerRosters();let b=Session("B");m.replace(b);await settle();record("queued-A-roster-task-cannot-request-B",b.authorizations.isEmpty && m.servers==[Server(id:"B-marker",members:["B"])])}
  do{let a=Session("A");let m=AppModel(a);let owner=m.accountLifetime.ticket!;let b=Session("B");m.replace(b);await m.refreshServers(EXPECTED);record("late-server-poll-entry-refuses-B",b.authorizations.isEmpty && m.remembered.isEmpty)}
  do{let a=Session("A");a.serversResult=[Server(id:"same")];let m=AppModel(a);m.servers=[Server(id:"same",members:["cached"])];await m.refreshServers();await settle();record("current-server-preserves-roster-control",m.servers==[Server(id:"same",members:["cached"])] && m.remembered==[true] && m.serversAvailable && a.authorizations==["A"])}
  do{let a=Session("A");a.serversFailure=404;let m=AppModel(a);m.servers=[Server(id:"old",members:["cached"])];m.serversAvailable=true;await m.refreshServers();record("current-server404-control",m.servers.isEmpty && !m.serversAvailable && m.remembered==[false])}
  do{let a=Session("A");let m=AppModel(a);m.isFixture=true;await m.refreshServers();record("fixture-server-no-request-control",a.authorizations.isEmpty && m.remembered.isEmpty)}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
fixture=fixture.replace('refreshServers(EXPECTED)','refreshServers(expectedOwner:owner)'if re.search(r'func refreshServers\([^\n]*expectedOwner',server)else'refreshServers()')
start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-ios-servers-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':sources,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},'Sources changed';(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==8 and row['results']['failed']==0,'Expected8 server ownership controls'
