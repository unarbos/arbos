"""Actual machine refresh with synthetic paused API results, no machine/provider use."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
P='apps/ios/Arbos/Arbos/';paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift','Store/SecretsStore.swift']];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};app,api,helper,secrets=[p.read_text()for p in paths]
def method(text,name):
 m=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1;start=m[0].start();start=start-len('    @MainActor\n')if text[:start].endswith('    @MainActor\n')else start;return text[start:text.index('\n    }',m[0].end())+6]
fakes=(HERE/'check_directory.py').read_text();start=fakes.index("fixture=r'''")+len("fixture=r'''");end=fakes.index("'''+method(api,'bound')",start);fixture=fakes[start:end]
fixture=fixture.replace('var authorizations:[String]=[]','var authorizations:[String]=[];var pauseSecrets=false;var secretsWaiter:CheckedContinuation<SecretsOverview,Error>?;var pauseProjectSecrets=false;var projectSecretsWaiter:CheckedContinuation<ProjectSecrets,Error>?')
fixture=fixture.replace(' func finish(_ value:[Person]?)'," func secrets()async throws->SecretsOverview{if pauseSecrets{return try await withCheckedThrowingContinuation{secretsWaiter=$0}};return overview(label)}\n func projectSecrets(_ id:String)async throws->ProjectSecrets{if pauseProjectSecrets{return try await withCheckedThrowingContinuation{projectSecretsWaiter=$0}};return ProjectSecrets(marker:label)}\n func finishSecrets(_ status:Int?=nil){let saved=secretsWaiter;secretsWaiter=nil;if let status{saved?.resume(throwing:RelayError(status:status))}else{saved?.resume(returning:overview(label))}}\n func finishProjectSecrets(){let saved=projectSecretsWaiter;projectSecretsWaiter=nil;saved?.resume(returning:ProjectSecrets(marker:label))}\n func finish(_ value:[Person]?)")
fixture=fixture.replace(' func users()async throws->[Person]{if let authorization'," func secrets()async throws->SecretsOverview{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};return try await session.secrets()}\n func projectSecrets(_ id:String)async throws->ProjectSecrets{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};return try await session.projectSecrets(id)}\n func users()async throws->[Person]{if let authorization")
fixture+=method(api,'bound')+'\n}\n'+helper
if 'struct AccountWork' in app:
 m=re.search(r'^(?:private )?struct AccountWork \{',app,re.M);fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
fixture+=r'''
typealias Id=String
struct SecretsOverview{var paused:Bool;let pausedAt:Int64?;let vaults:[String];let secrets:[String];let requestsPending:Int;let activity:[String]}
struct ProjectSecrets{let marker:String}
func overview(_ label:String)->SecretsOverview{SecretsOverview(paused:false,pausedAt:nil,vaults:[],secrets:[label],requestsPending:0,activity:[])}
struct RelayError:Error{let status:Int}
@MainActor final class AppModel {
 var credentials:Bool?=true;var isFixture=false;var api:RelayAPI;var relay:RelayAPI{api};var accountLifetime=WorkLifetime();var secrets:SecretsOverview?;var projectSecrets:[Id:ProjectSecrets]=[:]
 init(_ session:Session){api=RelayAPI(session)}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{expectedOwner.map{accountLifetime.accepts($0)} ?? true}
 func replace(_ session:Session){accountLifetime.close();accountLifetime.reopen();api=RelayAPI(session);secrets=overview("B-marker");projectSecrets=["same":ProjectSecrets(marker:"B-marker")]}
'''
for name in ['accountWork','boundCallAPI']:
 if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=method(app,name)+'\n'
fixture+=method(secrets,'refreshSecrets')+'\n'+method(secrets,'refreshProjectSecrets')+'\n}\n'+r'''
@main struct Checks {
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{let a=Session("A");a.pauseSecrets=true;let m=AppModel(a);let old=Task{await m.refreshSecrets()};let began=await spin{a.secretsWaiter != nil};m.replace(Session("B"));a.finishSecrets();await old.value;record("late-secret-overview-cannot-paint-B",began && m.secrets?.secrets==["B-marker"])}
  do{let a=Session("A");a.pauseSecrets=true;let m=AppModel(a);let old=Task{await m.refreshSecrets()};let began=await spin{a.secretsWaiter != nil};m.replace(Session("B"));a.finishSecrets(404);await old.value;record("late-secret404-cannot-clear-B",began && m.secrets?.secrets==["B-marker"])}
  do{let a=Session("A");let m=AppModel(a);let owner=m.accountLifetime.ticket!;let b=Session("B");m.replace(b);await m.refreshSecrets(EXPECTED);record("late-secret-entry-refuses-B",b.authorizations.isEmpty && m.secrets?.secrets==["B-marker"])}
  do{let a=Session("A");let m=AppModel(a);await m.refreshSecrets();record("current-overview-control",m.secrets?.secrets==["A"])}
  do{let a=Session("A");let m=AppModel(a);m.isFixture=true;await m.refreshSecrets();record("fixture-overview-no-request-control",a.authorizations.isEmpty && m.secrets==nil)}
  do{let a=Session("A");a.pauseProjectSecrets=true;let m=AppModel(a);let old=Task{await m.refreshProjectSecrets("same")};let began=await spin{a.projectSecretsWaiter != nil};m.replace(Session("B"));a.finishProjectSecrets();await old.value;record("late-project-secret-metadata-cannot-paint-B",began && m.projectSecrets["same"]?.marker=="B-marker")}
  do{let a=Session("A");let m=AppModel(a);let owner=m.accountLifetime.ticket!;let b=Session("B");m.replace(b);await m.refreshProjectSecrets("same"PROJECT_EXPECTED);record("late-project-secret-entry-refuses-B",b.authorizations.isEmpty && m.projectSecrets["same"]?.marker=="B-marker")}
  do{let a=Session("A");let m=AppModel(a);await m.refreshProjectSecrets("same");record("current-project-metadata-control",m.projectSecrets["same"]?.marker=="A")}
  do{let a=Session("A");let m=AppModel(a);m.isFixture=true;await m.refreshProjectSecrets("same");record("fixture-project-metadata-no-request-control",a.authorizations.isEmpty && m.projectSecrets.isEmpty)}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
fixture=fixture.replace('refreshSecrets(EXPECTED)','refreshSecrets(expectedOwner:owner)'if re.search(r'func refreshSecrets\([^\n]*expectedOwner',secrets)else'refreshSecrets()').replace('PROJECT_EXPECTED',',expectedOwner:owner'if re.search(r'func refreshProjectSecrets\([^\n]*expectedOwner',secrets)else'')
start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-ios-secrets-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':sources,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},'Sources changed';(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==9 and row['results']['failed']==0,'Expected9 secret metadata ownership controls'
