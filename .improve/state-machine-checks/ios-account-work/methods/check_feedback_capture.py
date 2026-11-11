"""Actual machine refresh with synthetic paused API results, no machine/provider use."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
P='apps/ios/Arbos/Arbos/';paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};app,api,helper=[p.read_text()for p in paths]
def method(text,name):
 m=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1;start=m[0].start();start=start-len('    @MainActor\n')if text[:start].endswith('    @MainActor\n')else start;return text[start:text.index('\n    }',m[0].end())+6]
fakes=(HERE/'check_directory.py').read_text();start=fakes.index("fixture=r'''")+len("fixture=r'''");end=fakes.index("'''+method(api,'bound')",start);fixture=fakes[start:end]
fixture=fixture.replace('var authorizations:[String]=[]','var authorizations:[String]=[];var pauseCapture=false;var captureWaiter:CheckedContinuation<Void,Error>?;var captures:[String]=[];var details:[String]=[]')
fixture=fixture.replace(' func finish(_ value:[Person]?)'," func capture()async throws{captures.append(label);if pauseCapture{try await withCheckedThrowingContinuation{captureWaiter=$0}}}\n func finishCapture(){let saved=captureWaiter;captureWaiter=nil;saved?.resume()}\n func finish(_ value:[Person]?)")
fixture=fixture.replace(' func users()async throws->[Person]{if let authorization'," func sendFeedbackCapture(requestId:String,image:Data?,mime:String,width:Int,height:Int,appVersion:String,os:String,device:String,screen:String,skipped:String?)async throws{if let authorization{guard let token=authorization()else{throw CancellationError()};session.authorizations.append(token)}else{session.authorizations.append(token())};session.details=[mime,String(width),String(height),screen,skipped ?? \"\"];try await session.capture()}\n func users()async throws->[Person]{if let authorization")
fixture+=method(api,'bound')+'\n}\n'+helper
if 'struct AccountWork' in app:
 m=re.search(r'^(?:private )?struct AccountWork \{',app,re.M);fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
fixture+=r'''
import os
struct Payload{let request_id:String;let expires_at:Int64=0}
enum PushEvent{case feedbackCapture(Payload),other}
struct PushItem{let event:PushEvent}
struct Notice{let text:String}
struct Shot{let size=(width:2.0,height:3.0);let scale=2.0;func jpegData(compressionQuality:Double)->Data?{FeedbackCenter.qualities.append(compressionQuality);return Data([1,2])}}
enum FeedbackCenter{static var calls=0;static var qualities:[Double]=[];static let appVersion="synthetic";static let deviceLine="synthetic";static func captureScreen()->Shot?{calls+=1;return Shot()}}
struct UIDevice{static let current=Self();let systemVersion="synthetic"}
let pushLog=Logger(subsystem:"synthetic",category:"capture")
@MainActor final class AppModel {
 var credentials:Bool?=true;var api:RelayAPI;var accountLifetime=WorkLifetime();var visibleChatId:String?="A-chat";var appActive=true;var notice:Notice?
 init(_ session:Session){api=RelayAPI(session);FeedbackCenter.calls=0;FeedbackCenter.qualities=[]}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{expectedOwner.map{accountLifetime.accepts($0)} ?? true}
 func replace(_ session:Session){accountLifetime.close();accountLifetime.reopen();api=RelayAPI(session);visibleChatId="B-chat";notice=nil}
 func captureForCheck(_ item:PushItem,expectedOwner:WorkLifetime.Ticket?=nil)async{await answerFeedbackCapture(item PASS_EXPECTED)}
'''
for name in ['accountWork','boundCallAPI']:
 if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=method(app,name)+'\n'
fixture+=method(app,'answerFeedbackCapture')+'\n}\n'+r'''
@main struct Checks {
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func item()->PushItem{PushItem(event:.feedbackCapture(Payload(request_id:"synthetic-request")))}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{let a=Session("A");a.pauseCapture=true;let m=AppModel(a);let old=Task{await m.captureForCheck(item())};let began=await spin{a.captureWaiter != nil};m.replace(Session("B"));a.finishCapture();await old.value;record("late-capture-upload-cannot-notice-B",began && m.notice==nil)}
  do{let a=Session("A");let m=AppModel(a);let owner=m.accountLifetime.ticket!;let b=Session("B");m.replace(b);await m.captureForCheck(item(),expectedOwner:owner);record("late-capture-entry-cannot-capture-or-upload-B",FeedbackCenter.calls==0 && b.authorizations.isEmpty && b.captures.isEmpty && m.notice==nil)}
  do{let a=Session("A");let m=AppModel(a);await m.captureForCheck(item());record("current-capture-quality-size-receipt-control",FeedbackCenter.qualities==[0.85] && a.details==["image/jpeg","4","6","chat:A-chat",""] && a.captures==["A"] && m.notice?.text=="Screenshot added to your feedback")}
  do{let a=Session("A");let m=AppModel(a);m.appActive=false;await m.captureForCheck(item());record("background-capture-skip-control",FeedbackCenter.calls==0 && a.details.last=="background" && a.captures==["A"] && m.notice==nil)}
  do{let a=Session("A");let m=AppModel(a);await m.captureForCheck(PushItem(event:.other));record("other-push-no-capture-control",FeedbackCenter.calls==0 && a.captures.isEmpty)}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
fixture=fixture.replace('PASS_EXPECTED',',expectedOwner:expectedOwner'if re.search(r'func answerFeedbackCapture\([^\n]*expectedOwner',app)else'')
start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-ios-feedback-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':sources,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},'Sources changed';(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==5 and row['results']['failed']==0,'Expected5 feedback ownership controls'
