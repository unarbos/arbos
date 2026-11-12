from pathlib import Path
import argparse,hashlib,json,subprocess,tempfile,time
from check_cards import P,ROOT,PREFIX,method
F=r'''
import Foundation
typealias Id=String
final class RelayAPI{}
struct AccountWork{let owner:WorkLifetime.Ticket;let api:RelayAPI}
CONTEXT
struct UserError{static func refused(_ verb:String,_ message:String)->UserError{UserError()}}
enum CallKind{case voice,video}
enum Haptics{static func send(){}}
@MainActor final class AppModel{
 struct PendingAttachment{let name:String;let mime:String;let data:Data}
 var accountLifetime=WorkLifetime();var isFixture=false;var authenticated=true;var current="A";var effects:[String]=[];var failures=0
 static func isDraftDM(_ id:String)->Bool{id=="draft"}
 func accountWork()->AccountWork?{guard authenticated,let owner=accountLifetime.ticket else{return nil};return AccountWork(owner:owner,api:RelayAPI())}
 ACCEPTS
 func replace(){accountLifetime.close();accountLifetime.reopen();current="B"}
 func stop(chatId:String){effects.append(current+"/stop/"+chatId)}
 func fail(_ error:UserError){failures+=1}
}
@MainActor final class Calls{let model:AppModel;init(_ m:AppModel){model=m};func start(chatId:String,projectId:String?,kind:CallKind,title:String,ring:[String]?)async{model.effects.append(model.current+"/call/"+chatId)}}
@MainActor final class Recorder{var recording=true;var stops=0;func stop()->(name:String,data:Data)?{stops+=1;recording=false;return("Memo",Data([1,2]))}}
@MainActor private final class Session{
 let model:AppModel;let account:ChatAccountContext;let calls:Calls;let recorder=Recorder();var chatId="chat";var projectId="project";var isPeople=false;var callTitle="Call";var memos:[String]=[];var handed:AppModel.PendingAttachment?
 init(_ model:AppModel){self.model=model;account=ChatAccountContext(model);calls=Calls(model)}
 STOP
 CALL
 FINISH
 func sendMemo(_ memo:AppModel.PendingAttachment){memos.append(model.current);handed=memo}
 func act(_ which:String){if which=="stop"{stop()}else if which=="call"{startCall(.voice)}else{finishMemoHold()}}
}
@main struct Checks{
 @MainActor static func settle()async{for _ in 0..<100{await Task.yield()}}
 @MainActor static func main()async{var cases:[String:Bool]=[:]
 for action in ["stop","call","finish"]{let m=AppModel();let s=Session(m);m.replace();s.act(action);await settle();cases["old-chat-"+action+"-cannot-adopt-B"]=m.effects.isEmpty && s.memos.isEmpty && s.recorder.stops==0}
 for action in ["stop","call","finish"]{let m=AppModel();m.authenticated=false;let s=Session(m);s.act(action);await settle();cases["unavailable-chat-"+action+"-cannot-mutate"]=m.effects.isEmpty && s.memos.isEmpty && s.recorder.stops==0}
 for action in ["stop","call","finish"]{let m=AppModel();let s=Session(m);let t=Task{s.act(action)};t.cancel();await t.value;await settle();cases["cancelled-chat-"+action+"-cannot-mutate"]=m.effects.isEmpty && s.memos.isEmpty && s.recorder.stops==0}
 do{let m=AppModel();let s=Session(m);s.act("call");m.replace();await settle();cases["queued-call-task-retirement-refused"]=m.effects.isEmpty}
 do{let m=AppModel();let s=Session(m);s.act("stop");cases["current-stop-control"]=m.effects==["A/stop/chat"]}
 do{let m=AppModel();let s=Session(m);s.act("call");await settle();cases["current-call-control"]=m.effects==["A/call/chat"]}
 do{let m=AppModel();let s=Session(m);s.act("finish");cases["current-memo-caller-handoff-control"]=s.recorder.stops==1 && s.memos==["A"] && s.handed?.name=="Memo" && s.handed?.mime=="audio/m4a" && s.handed?.data==Data([1,2])}
 do{let m=AppModel();let s=Session(m);s.recorder.recording=false;s.act("finish");cases["not-recording-memo-noop-control"]=s.recorder.stops==0 && s.memos.isEmpty}
 do{let m=AppModel();m.isFixture=true;m.authenticated=false;let s=Session(m);s.act("stop");cases["fixture-local-stop-control"]=m.effects==["A/stop/chat"]}
 do{let m=AppModel();let s=Session(m);s.chatId="draft";s.act("call");await settle();cases["current-draft-call-refusal-preserved-control"]=m.effects.isEmpty && m.failures==1}
 let failed=cases.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["total":cases.count,"passed":cases.count-failed,"failed":failed,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('arm',choices=['before','after']);ap.add_argument('label');a=ap.parse_args();src=P/'base/ChatView.swift'if a.arm=='before'else ROOT/PREFIX/'ChatView.swift';s=src.read_text();pieces=[method(s,n)for n in ['stop','startCall','finishMemoHold']];helper=(P/'context/WorkLifetime.swift').read_text();ctx=(P/'context/ChatAccountContext.swift').read_text();model=(P/'context/AppModel.swift').read_text();x=model.index('    func acceptsAccountWork(');accept=model[x:model.index('\n    }',x)+6];fixture=helper+F.replace('CONTEXT',ctx).replace('ACCEPTS',accept).replace('STOP',pieces[0]).replace('CALL',pieces[1]).replace('FINISH',pieces[2]);out=P/a.label;out.mkdir(exist_ok=False);(out/'fixture.swift').write_text(fixture);start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='chat-session-')as raw:
  tmp=Path(raw);f=tmp/'main.swift';f.write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(f),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);j={'source_sha256':hashlib.sha256(s.encode()).hexdigest(),'method_sha256':hashlib.sha256(''.join(pieces).encode()).hexdigest(),'context_sha256':hashlib.sha256(ctx.encode()).hexdigest(),'compile_exit':c.returncode}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);j.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  assert s==src.read_text();j['seconds']=time.monotonic()-start;(out/'GATES.json').write_text(json.dumps(j,indent=2)+'\n');print(json.dumps(j))
if __name__=='__main__':main()
