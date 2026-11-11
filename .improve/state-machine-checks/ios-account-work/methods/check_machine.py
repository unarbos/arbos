"""Actual machine refresh with synthetic paused API results, no machine/provider use."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
P='apps/ios/Arbos/Arbos/';paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};app,api,helper=[p.read_text()for p in paths]
def method(text,name):
 m=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1;start=m[0].start();start=start-len('    @MainActor\n')if text[:start].endswith('    @MainActor\n')else start;return text[start:text.index('\n    }',m[0].end())+6]
fakes=(HERE/'check_directory.py').read_text();start=fakes.index("fixture=r'''")+len("fixture=r'''");end=fakes.index("'''+method(api,'bound')",start);fixture=fakes[start:end]
fixture=fixture.replace('var authorizations:[String]=[]','var authorizations:[String]=[];var pauseMachine=false;var machineWaiter:CheckedContinuation<Machine,Error>?;var machineResult=Machine(label:"A-result");var machineFailure:Int?')
fixture=fixture.replace(' func finish(_ value:[Person]?)',' func machine()async throws->Machine{if pauseMachine{return try await withCheckedThrowingContinuation{machineWaiter=$0}};if let status=machineFailure{throw RelayError(status:status)};return machineResult}\n func finishMachine(_ failure:Int?=nil){let saved=machineWaiter;machineWaiter=nil;if let failure{saved?.resume(throwing:RelayError(status:failure))}else{saved?.resume(returning:machineResult)}}\n func finish(_ value:[Person]?)')
fixture=fixture.replace(' func users()async throws->[Person]{if let authorization',' func machine()async throws->Machine{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};return try await session.machine()}\n func users()async throws->[Person]{if let authorization')
fixture+=method(api,'bound')+'\n}\n'+helper
if 'struct AccountWork' in app:
 m=re.search(r'^(?:private )?struct AccountWork \{',app,re.M);fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
fixture+=r'''
struct Project {var id:String;var workspace:String="old"}
enum State:String {case awake,waking,asleep}
struct Machine:Equatable {var label:String;var alwaysOn=false;var state=State.awake;var workspaces:[String:String]=["same":"new"];func coerced(_ state:State)->State{state}}
struct RelayError:Error{let status:Int}
struct UserError{let sentence:String;static func classify(_ verb:String,_ error:Error)->Self{Self(sentence:verb)}}
@MainActor final class AppModel {
 var credentials:Bool?=true;var api:RelayAPI;var accountLifetime=WorkLifetime();var machine=Machine(label:"initial");var machineError:String?="initial-error";var meAlwaysOn:Bool?;var projects=[Project(id:"same")]
 init(_ session:Session){api=RelayAPI(session)}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{expectedOwner.map{accountLifetime.accepts($0)} ?? true}
 func replace(_ session:Session){accountLifetime.close();accountLifetime.reopen();api=RelayAPI(session);machine=Machine(label:"B-marker");machineError="B-error";projects=[Project(id:"same",workspace:"B-workspace")]}
'''
for name in ['accountWork','boundCallAPI']:
 if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=method(app,name)+'\n'
fixture+=method(app,'refreshMachine')+'\n}\n'+r'''
@main struct Checks {
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{let a=Session("A");a.pauseMachine=true;let m=AppModel(a);let old=Task{await m.refreshMachine()};let began=await spin{a.machineWaiter != nil};m.replace(Session("B"));a.finishMachine();await old.value;record("late-machine-success-cannot-paint-B",began && m.machine.label=="B-marker" && m.machineError=="B-error" && m.projects.first?.workspace=="B-workspace")}
  do{let a=Session("A");a.pauseMachine=true;let m=AppModel(a);let old=Task{await m.refreshMachine()};let began=await spin{a.machineWaiter != nil};m.replace(Session("B"));a.finishMachine(404);await old.value;record("late-machine-error-cannot-replace-B-error",began && m.machineError=="B-error")}
  do{let a=Session("A");a.pauseMachine=true;let m=AppModel(a);let old=Task{await m.refreshMachine()};let began=await spin{a.machineWaiter != nil};old.cancel();a.finishMachine();await old.value;record("cancelled-machine-work-cannot-publish",began && m.machine.label=="initial" && m.machineError=="initial-error")}
  do{let a=Session("A");let m=AppModel(a);let owner=m.accountLifetime.ticket!;let b=Session("B");m.replace(b);await m.refreshMachine(EXPECTED);record("late-machine-entry-refuses-B",b.authorizations.isEmpty && m.machine.label=="B-marker" && m.machineError=="B-error")}
  do{let a=Session("A");let m=AppModel(a);m.meAlwaysOn=true;await m.refreshMachine();record("current-machine-success-control",m.machine.label=="A-result" && m.machine.alwaysOn && m.machineError==nil && m.projects.first?.workspace=="new")}
  do{let a=Session("A");a.machineFailure=404;let m=AppModel(a);await m.refreshMachine();record("current-machine404-control",m.machineError=="This relay has no machine API yet")}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
fixture=fixture.replace('refreshMachine(EXPECTED)','refreshMachine(expectedOwner:owner)'if re.search(r'func refreshMachine\([^\n]*expectedOwner',app)else'refreshMachine()')
start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-ios-machine-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':sources,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},'Sources changed';(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==6 and row['results']['failed']==0,'Expected6 machine ownership controls'
