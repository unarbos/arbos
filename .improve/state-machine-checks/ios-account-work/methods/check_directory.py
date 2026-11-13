"""Actual directory controller+API binding over synthetic paused request collaborators."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
P='apps/ios/Arbos/Arbos/';paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};app,api,helper=[p.read_text()for p in paths]
def method(text,name):
 m=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1,(name,len(m));start=m[0].start();start=start-len('    @MainActor\n')if text[:start].endswith('    @MainActor\n')else start;return text[start:text.index('\n    }',m[0].end())+6]
fixture=r'''
import Foundation
struct Person:Equatable {let id:String;let handle:String;var status:String="member"}
struct User{let id:String}
@MainActor private final class CallAuthority{var bearer:String;init(_ value:String){bearer=value}}
@MainActor final class Session {
 let label:String;var pause=false;var waiter:CheckedContinuation<[Person],Error>?;var usersValue:[Person]=[];var usersFails=false;var searches:[String]=[];var authorizations:[String]=[]
 init(_ label:String){self.label=label}
 func users()async throws->[Person]{if pause{return try await withCheckedThrowingContinuation{waiter=$0}};if usersFails{throw URLError(.badServerResponse)};return usersValue}
 func finish(_ value:[Person]?){let saved=waiter;waiter=nil;if let value{saved?.resume(returning:value)}else{saved?.resume(throwing:URLError(.badServerResponse))}}
}
@MainActor final class RelayAPI {
 let endpoint:String;var token:()->String;var refresher:(()async->Bool)?;private let session:Session;private var authorization:(@MainActor ()->String?)?;private var cleanupAuthority:CallAuthority?
 init(_ session:Session){self.session=session;endpoint="http://synthetic.invalid";token={session.label}}
 private init(endpoint:String,token:@escaping()->String,session:Session){self.endpoint=endpoint;self.token=token;self.session=session}
 func users()async throws->[Person]{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};return try await session.users()}
 func searchUsers(_ query:String)async throws->[Person]{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};session.searches.append(query);return [Person(id:"p1",handle:"Zed"),Person(id:"p2",handle:"alpha"),Person(id:"self",handle:"me"),Person(id:"github",handle:"provider",status:"github")]}
'''+method(api,'bound')+'\n}\n'+helper
if 'struct AccountWork' in app:
 m=re.search(r'^(?:private )?struct AccountWork \{',app,re.M);assert m;fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
fixture+=r'''
@MainActor final class AppModel {
 var credentials:Bool?=true;var isFixture=false;var directory:[Person]=[];var directoryLoadedAt:Int64=0;var user:User?=User(id:"self");var api:RelayAPI;var accountLifetime=WorkLifetime();var commits:[[Person]]=[]
 nonisolated static let directoryTtlMs:Int64=600_000
 init(_ session:Session){api=RelayAPI(session)}
 func setDirectory(_ values:[Person]){directory=values;commits.append(values)}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{expectedOwner.map{accountLifetime.accepts($0)} ?? true}
 func replace(_ session:Session){accountLifetime.close();accountLifetime.reopen();api=RelayAPI(session);directory=[Person(id:"B-marker",handle:"B")];directoryLoadedAt=0;commits=[]}
'''
for name in ['accountWork','boundCallAPI']:
 if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=method(app,name)+'\n'
fixture+=method(app,'loadDirectory')+'\n'
m=re.search(r'^    nonisolated static func directoryNeedsRefresh\(',app,re.M);assert m;fixture+=app[m.start():app.index('\n    }',m.start())+6]+'\n}\n'
fixture+=r'''
func nowMs()->Int64{1_000}
@main struct Checks {
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func main()async{
  var results:[[String:Any]]=[]
  func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{let a=Session("A");a.pause=true;let m=AppModel(a);let old=Task{await m.loadDirectory(force:true)};let began=await spin{a.waiter != nil};let b=Session("B");m.replace(b);a.finish([Person(id:"A-private",handle:"A")]);await old.value;record("late-users-response-cannot-publish-into-B",began && m.directory==[Person(id:"B-marker",handle:"B")] && m.commits.isEmpty && m.directoryLoadedAt==0)}
  do{let a=Session("A");a.pause=true;let m=AppModel(a);let old=Task{await m.loadDirectory(force:true)};let began=await spin{a.waiter != nil};let b=Session("B");m.replace(b);a.finish(nil);await old.value;record("late-users-error-cannot-start-B-fallback",began && b.searches.isEmpty && m.commits.isEmpty && m.directory==[Person(id:"B-marker",handle:"B")])}
  do{let a=Session("A");a.usersValue=[Person(id:"self",handle:"me"),Person(id:"peer",handle:"peer")];let m=AppModel(a);await m.loadDirectory(force:true);record("current-users-control",m.directory==[Person(id:"peer",handle:"peer")] && m.commits.count==1 && a.searches.isEmpty)}
  do{let a=Session("A");a.usersFails=true;let m=AppModel(a);await m.loadDirectory(force:true);record("current-fallback-filter-dedup-sort-control",m.directory==[Person(id:"p2",handle:"alpha"),Person(id:"p1",handle:"Zed")] && Set(a.searches)==Set("abcdefghijklmnopqrstuvwxyz0123456789".map(String.init)) && a.searches.count==36)}
  do{let a=Session("A");let m=AppModel(a);let owner=m.accountLifetime.ticket!;let b=Session("B");m.replace(b);await m.loadDirectory(force:trueEXPECTED);record("late-poll-entry-refuses-new-account-work",b.authorizations.isEmpty && m.commits.isEmpty && m.directoryLoadedAt==0)}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
fixture=fixture.replace('force:trueEXPECTED','force:true,expectedOwner:owner'if re.search(r'func loadDirectory\([^\n]*expectedOwner',app)else'force:true')
start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-ios-directory-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':sources,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},'Sources changed';(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==5 and row['results']['failed']==0,'Expected5 directory controls'
