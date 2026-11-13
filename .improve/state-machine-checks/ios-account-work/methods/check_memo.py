"""Exact sendMemo callback with real transcribe helper; no audio device/UI SDK."""
from pathlib import Path
import argparse,hashlib,importlib.util,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
spec=importlib.util.spec_from_file_location('media',HERE/'check_media.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);u=m.u
V=r'''
extension AppModel {typealias PendingAttachment=Memo}
typealias Memo=PendingAttachment
struct Mention{let value:String}
enum MentionDraft{static func tokens(_ mentions:[Mention])->[String:String]{[:]}}
struct Composer{var text="typed-A";var mentions:[Mention]=[]}
struct Project{let id:String}
struct Reply{let id:String}
@MainActor final class MemoView{
 let model:AppModel;var composerDraft=Composer();var pending:[PendingAttachment]=[];var replyTo:Reply?;var replyRef:ReplyRef?;var forwardedFrom:String?="forward";var editing:String?;var mentionQuery:String?;var sentOnce=false;var transcribing=false
 var project:Project?=Project(id:"project");var isPeople=false;var isChannel=false;var needsAnswer=false;let peopleSlot="people";let chatId="same-chat"
 init(_ model:AppModel){self.model=model}
 func sendAddress(for text:String)->String?{nil};func chipMentions(for text:String)->[String]{[]};func clearDraft(){composerDraft.text=""};func pinTalkTo(afterSending:String?,chipMentions:[String]){}
 func start(){sendMemo(PendingAttachment(name:"memo",mime:"audio/aac",data:Data([8])))}
'''
T=r'''
}
@main struct Checks{
 @MainActor static func spin(_ p:()->Bool)async->Bool{for _ in 0..<10000{if p(){return true};await Task.yield()};return p()}
 @MainActor static func main()async{
 var cases:[String:Bool]=[:]
 do{let a=Session("A");let model=AppModel(a);let view=MemoView(model);view.start();let b=Session("B");model.replace(b);for _ in 0..<100{await Task.yield()};cases["queued-A-memo-no-B-authority-or-send"]=b.authorizations.isEmpty && model.commits.isEmpty}
 for failed in [false,true]{let a=Session("A");a.pauseTranscribe=true;let model=AppModel(a);let view=MemoView(model);view.start();let began=await spin{a.transcribeWaiter != nil};model.replace(Session("B"));view.transcribing=true;a.finishTranscribe(failed);for _ in 0..<100{await Task.yield()};cases[failed ? "late-memo-failure-no-typed-A-send":"late-memo-success-no-typed-A-send"]=began && model.commits.isEmpty && view.transcribing && model.errors.isEmpty}
 for people in [false,true]{let a=Session("A");let model=AppModel(a);let view=MemoView(model);view.isPeople=people;view.start();let done=await spin{!model.commits.isEmpty};cases[people ? "current-people-memo-control":"current-project-memo-control"]=done && model.texts==["typed-A\n\nA-transcript"] && !view.transcribing && model.commits.count==1}
 let failed=cases.values.filter{!$0}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":cases.count-failed,"failed":failed,"total":cases.count,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--tree',type=Path,required=True);ap.add_argument('--out',type=Path,required=True);a=ap.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(exist_ok=False)
 paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift','UI/ChatView.swift']];app,api,helper,view=[p.read_text()for p in paths];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};fixture=m.F+u.method(api,'bound')+'\n}\n'+helper
 if 'struct AccountWork' in app:
  k=re.search(r'^struct AccountWork \{',app,re.M);fixture+='\n'+app[k.start():app.index('\n}',k.start())+2]
 else:fixture+='\nstruct AccountWork{let owner:WorkLifetime.Ticket;let api:RelayAPI}\n'
 models=m.M.replace('var marked:[String]=[]','var marked:[String]=[];var texts:[String]=[]')
 models=models.replace(' {commits.append(Commit(', ' {texts.append(text);commits.append(Commit(')
 models+=''' func send(chatId:String,text:String,attachments:[PendingAttachment]=[],replyTo:String?=nil,forwardedFrom:String?=nil){write(chatId:chatId,text:text,command:"send",attachments:[])}
 func sendPeople(chatId:String,text:String,replyTo:String?,address:String?,chipMentions:[String],attachments:[PendingAttachment],forwardedFrom:String?,chipTokens:[String:String],replyRef:ReplyRef?){send(chatId:chatId,text:text)}
 func sendChannel(chatId:String,text:String){send(chatId:chatId,text:text)};func answer(chatId:String,text:String){send(chatId:chatId,text:text)}
'''
 fixture+=models
 for name in ['accountWork','boundCallAPI']:
  if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=u.method(app,name)+'\n'
 fixture+=u.method(app,'transcribe')+'\n}\n'+V+u.method(view,'sendMemo')+T;start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-ios-memo-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'sources':sources,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  record['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
