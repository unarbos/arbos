"""Actual pins refresh with synthetic paused API results, no machine/provider use."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
P='apps/ios/Arbos/Arbos/';paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};app,api,helper=[p.read_text()for p in paths]
def method(text,name):
 m=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1;start=m[0].start();start=start-len('    @MainActor\n')if text[:start].endswith('    @MainActor\n')else start;return text[start:text.index('\n    }',m[0].end())+6]
fakes=(HERE/'check_directory.py').read_text();start=fakes.index("fixture=r'''")+len("fixture=r'''");end=fakes.index("'''+method(api,'bound')",start);fixture=fakes[start:end]
fixture=fixture.replace('var authorizations:[String]=[]','var authorizations:[String]=[];var pausePins=false;var pauseSetPins=false;var pinsWaiter:CheckedContinuation<PinsDto,Error>?;var setPinsWaiter:CheckedContinuation<PinsDto,Error>?;var pinsResult=PinsDto(pins:["current"],cap:5);var putPins:[[String]]=[]')
fixture=fixture.replace(' func finish(_ value:[Person]?)'," func pins()async throws->PinsDto{if pausePins{return try await withCheckedThrowingContinuation{pinsWaiter=$0}};return pinsResult}\n func setPins(_ pins:[String])async throws->PinsDto{putPins.append(pins);if pauseSetPins{return try await withCheckedThrowingContinuation{setPinsWaiter=$0}};return PinsDto(pins:pins,cap:pinsResult.cap)}\n func finishPins(){let saved=pinsWaiter;pinsWaiter=nil;saved?.resume(returning:pinsResult)}\n func finishSetPins(){let saved=setPinsWaiter;setPinsWaiter=nil;saved?.resume(returning:PinsDto(pins:putPins.last ?? [],cap:pinsResult.cap))}\n func finish(_ value:[Person]?)")
fixture=fixture.replace(' func users()async throws->[Person]{if let authorization'," func pins()async throws->PinsDto{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};return try await session.pins()}\n func setPins(_ pins:[String])async throws->PinsDto{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};return try await session.setPins(pins)}\n func users()async throws->[Person]{if let authorization")
fixture+=method(api,'bound')+'\n}\n'+helper
if 'struct AccountWork' in app:
 m=re.search(r'^(?:private )?struct AccountWork \{',app,re.M);fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
fixture+=r'''
struct PinsDto {let pins:[String];let cap:Int64}
struct RelayError:Error{let status:Int}
enum HomePins {static var legacy:[String]=[];static var marks=0;static func legacyLocal()->[String]{legacy};static func migrated()->Bool{marks>0};static func migrateLegacy(_ input:[String],cap:Int)->[String]{Array(input.prefix(cap))};static func markMigrated(){marks+=1}}
@MainActor final class AppModel {
 var credentials:Bool?=true;var isFixture=false;var api:RelayAPI;var accountLifetime=WorkLifetime();var pinnedHomeIds=["initial"];var pinCap=5;var applies=0
 init(_ session:Session){api=RelayAPI(session);HomePins.legacy=[];HomePins.marks=0}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{expectedOwner.map{accountLifetime.accepts($0)} ?? true}
 func replace(_ session:Session){accountLifetime.close();accountLifetime.reopen();api=RelayAPI(session);pinnedHomeIds=["B-marker"];pinCap=7;applies=0}
 func applyPins(_ value:PinsDto){pinCap=max(1,Int(value.cap));pinnedHomeIds=value.pins;applies+=1}
'''
for name in ['accountWork','boundCallAPI']:
 if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=method(app,name)+'\n'
fixture+=method(app,'refreshPins')+'\n}\n'+r'''
@main struct Checks {
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{let a=Session("A");a.pausePins=true;let m=AppModel(a);let old=Task{await m.refreshPins()};let began=await spin{a.pinsWaiter != nil};m.replace(Session("B"));a.finishPins();await old.value;record("late-pin-list-cannot-paint-B",began && m.pinnedHomeIds==["B-marker"] && m.pinCap==7 && m.applies==0)}
  do{let a=Session("A");a.pausePins=true;a.pinsResult=PinsDto(pins:[],cap:2);let m=AppModel(a);HomePins.legacy=["legacy-A"];let old=Task{await m.refreshPins()};let began=await spin{a.pinsWaiter != nil};let b=Session("B");m.replace(b);a.finishPins();await old.value;record("late-empty-pins-cannot-migrate-into-B",began && b.putPins.isEmpty && HomePins.marks==0 && m.applies==0)}
  do{let a=Session("A");a.pinsResult=PinsDto(pins:[],cap:2);a.pauseSetPins=true;let m=AppModel(a);HomePins.legacy=["legacy-A"];let old=Task{await m.refreshPins()};let began=await spin{a.setPinsWaiter != nil};m.replace(Session("B"));a.finishSetPins();await old.value;record("late-pin-migration-cannot-mark-or-apply-B",began && HomePins.marks==0 && m.pinnedHomeIds==["B-marker"] && m.applies==0)}
  do{let a=Session("A");let m=AppModel(a);let owner=m.accountLifetime.ticket!;let b=Session("B");m.replace(b);await m.refreshPins(EXPECTED);record("late-pin-entry-refuses-B",b.authorizations.isEmpty && m.applies==0)}
  do{let a=Session("A");let m=AppModel(a);await m.refreshPins();record("current-pin-list-control",m.pinnedHomeIds==["current"] && m.applies==1 && a.putPins.isEmpty)}
  do{let a=Session("A");a.pinsResult=PinsDto(pins:[],cap:2);let m=AppModel(a);HomePins.legacy=["one","two","three"];await m.refreshPins();record("current-pin-migration-control",a.putPins==[["one","two"]] && m.pinnedHomeIds==["one","two"] && m.pinCap==2 && HomePins.marks==1)}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
fixture=fixture.replace('refreshPins(EXPECTED)','refreshPins(expectedOwner:owner)'if re.search(r'func refreshPins\([^\n]*expectedOwner',app)else'refreshPins()')
start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-ios-pins-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':sources,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},'Sources changed';(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==6 and row['results']['failed']==0,'Expected6 pin ownership controls'
