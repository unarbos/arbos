"""Actual off-main cache import/adoption with synthetic saved bytes and paused core."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
P='apps/ios/Arbos/Arbos/';paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};app,api,helper=[p.read_text()for p in paths]
def method(text,name):
 m=list(re.finditer(r'^    (?:private )?(?:nonisolated )?(?:static )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1;start=m[0].start();start=start-len('    @MainActor\n')if text[:start].endswith('    @MainActor\n')else start;return text[start:text.index('\n    }',m[0].end())+6]
fakes=(HERE/'check_directory.py').read_text();start=fakes.index("fixture=r'''")+len("fixture=r'''");end=fakes.index("'''+method(api,'bound')",start);fixture=fakes[start:end]+method(api,'bound')+'\n}\n'+helper
if 'struct AccountWork' in app:
 m=re.search(r'^(?:private )?struct AccountWork \{',app,re.M);fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
fixture+=r'''
struct Message{let id:String}
final class ImportGate:@unchecked Sendable{let began=DispatchSemaphore(value:0);let release=DispatchSemaphore(value:0);func waitUntilBegan()->Bool{began.wait(timeout:.now()+2) == .success}}
final class CoreChatDoc:@unchecked Sendable{
 static let lock=NSLock();static var gate:ImportGate?
 let key:String;var count:Int{1};init(key:String){self.key=key}
 static func pause(_ value:ImportGate?){lock.lock();gate=value;lock.unlock()}
 func importUpdate(_ data:Data)throws{Self.lock.lock();let gate=Self.gate;Self.lock.unlock();gate?.began.signal();if let gate{_ = gate.release.wait(timeout:.now()+5)}}
 func readAll()->[Message]{[Message(id:"saved")]}
}
final class DeviceStore {
 struct SavedDoc{let key:String;let projectId:String;let roomChat:String;let cursor:Int64;let snapshot:Data}
 var saved:[SavedDoc]=[SavedDoc(key:"saved-chat",projectId:"saved-project",roomChat:"saved-chat",cursor:9,snapshot:Data("synthetic-opaque".utf8))]
 func loadDocs()->[SavedDoc]{saved};func loadPending()->[String:[(String,Data)]]{["saved-chat":[("batch",Data([1]))]]}
}
let projectDocChatId="project"
enum LaunchClock{static func mark(_ name:String,detail:String){}}
@MainActor final class AppModel {
 var credentials:Bool?=true;var api:RelayAPI;var accountLifetime=WorkLifetime();var store:DeviceStore?=DeviceStore();var ensured:[String]=[];var restoredCursor:Int64?;var restoredPending=0;var restoredRows=0
 init(_ session:Session){api=RelayAPI(session)}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{expectedOwner.map{accountLifetime.accepts($0)} ?? true}
 func replace(){accountLifetime.close();accountLifetime.reopen();store=DeviceStore();ensured=[];restoredCursor=nil}
 func loadDocsForCheck(_ store:DeviceStore)async{await loadStoreDocs(store)}
 func ensureRoom(projectId:String,key:String,roomChat:String,restored:(CoreChatDoc,Int64,[(String,Data)],[Message]?)){ensured.append(key);restoredCursor=restored.1;restoredPending=restored.2.count;restoredRows=restored.3?.count ?? 0}
'''
for name in ['accountWork','boundCallAPI']:
 if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=method(app,name)+'\n'
for name in ['loadStoreDocs','storedCopyIsBlank']:fixture+=method(app,name)+'\n'
fixture+='}\n'+r'''
@main struct Checks {
 @MainActor static func began(_ gate:ImportGate)async->Bool{await Task.detached{gate.waitUntilBegan()}.value}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{let m=AppModel(Session("A"));let store=m.store!;let gate=ImportGate();CoreChatDoc.pause(gate);let old=Task{await m.loadDocsForCheck(store)};let started=await began(gate);m.replace();gate.release.signal();await old.value;record("late-saved-doc-import-cannot-adopt-B-rooms",started && m.ensured.isEmpty)}
  do{let m=AppModel(Session("A"));let store=m.store!;let gate=ImportGate();CoreChatDoc.pause(gate);let old=Task{await m.loadDocsForCheck(store)};let started=await began(gate);old.cancel();gate.release.signal();await old.value;record("cancelled-saved-import-cannot-adopt-rooms",started && m.ensured.isEmpty)}
  do{CoreChatDoc.pause(nil);let m=AppModel(Session("A"));let old=m.store!;m.store=DeviceStore();await m.loadDocsForCheck(old);record("foreign-cache-instance-cannot-adopt-rooms",m.ensured.isEmpty)}
  do{CoreChatDoc.pause(nil);let m=AppModel(Session("A"));await m.loadDocsForCheck(m.store!);record("current-saved-doc-cursor-pending-rows-control",m.ensured==["saved-chat"] && m.restoredCursor==9 && m.restoredPending==1 && m.restoredRows==1)}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-ios-store-docs-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':sources,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},'Sources changed';(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==4 and row['results']['failed']==0,'Expected4 saved-doc ownership controls'
