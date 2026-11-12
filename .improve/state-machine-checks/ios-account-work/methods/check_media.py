"""Actual sticker task/upload/transcription methods with synthetic collaborators."""
from pathlib import Path
import argparse,hashlib,importlib.util,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
spec=importlib.util.spec_from_file_location('upload',HERE/'check_uploads.py');u=importlib.util.module_from_spec(spec);spec.loader.exec_module(u)
F=u.F.replace('var pause=false','var transcribeWaiter:CheckedContinuation<String,Error>?;var pauseTranscribe=false;var pause=false').replace(' func finish(_ failed:Bool=false)',r''' func transcribe()async throws->String{if pauseTranscribe{return try await withCheckedThrowingContinuation{transcribeWaiter=$0}};return label+"-transcript"}
 func finishTranscribe(_ failed:Bool=false){let w=transcribeWaiter;transcribeWaiter=nil;if failed{w?.resume(throwing:RelayError(status:500))}else{w?.resume(returning:label+"-transcript")}}
 func finish(_ failed:Bool=false)''')
F=F.replace(' func putBlob(projectId:String',r''' func transcribe(projectId:String?,data:Data,mime:String)async throws->String{if let sourceAuthorization{guard let bearer=sourceAuthorization()else{throw CancellationError()};session.authorizations.append(bearer)}else{session.authorizations.append(token())};return try await session.transcribe()}
 func putBlob(projectId:String''')
M=u.M.replace('struct Attachment:Equatable{','struct AttachmentSource:Equatable{let provider:String;let id:String;let pageUrl:String?}\nstruct Pack{let id:String;let name:String;let pageUrl:String?}\nstruct Item{let id:String;let mime:String}\nstruct Attachment:Equatable{').replace('var kind:String?}','var kind:String?;var bytes:Int64?=nil;var source:AttachmentSource?=nil}')
M=M.replace('var marked:[String]=[]','var marked:[String]=[];var visibleChatId:String?="chat"')
M=M.replace(' func clearSendError(_ id:String){}',' func fail(_ what:String,_ error:Error){errors.append(what)}\n func sendPeopleAttachments(chatId:String,attachments:[Attachment]){write(chatId:chatId,text:"",command:"send",attachments:attachments)}\n func clearSendError(_ id:String){}')
T=r'''
}
@main struct Checks{
 @MainActor static func spin(_ p:()->Bool)async->Bool{for _ in 0..<10000{if p(){return true};await Task.yield()};return p()}
 @MainActor static func main()async{
 var cases:[String:Bool]=[:];let pack=Pack(id:"pack",name:"Synthetic",pageUrl:nil);let item=Item(id:"item",mime:"image/png")
 do{let a=Session("A");let m=AppModel(a);let b=Session("B");m.sticker(pack:pack,item:item,data:Data([7]));m.replace(b);for _ in 0..<100{await Task.yield()};cases["queued-sticker-never-uses-B"]=b.authorizations.isEmpty && m.commits.isEmpty}
 for failed in [false,true]{let a=Session("A");a.pause=true;let m=AppModel(a);m.sticker(pack:pack,item:item,data:Data([7]));let began=await spin{a.waiter != nil};let b=Session("B");m.replace(b);a.finish(failed);for _ in 0..<100{await Task.yield()};cases[failed ? "stale-sticker-error-no-paint":"stale-sticker-success-no-commit"]=began && b.authorizations.isEmpty && m.commits.isEmpty && m.errors.isEmpty}
 do{let a=Session("A");let m=AppModel(a);m.sticker(pack:pack,item:item,data:Data([7]));let done=await spin{!m.commits.isEmpty};let value=m.commits.first?.attachments.first;cases["current-sticker-source-bytes-control"]=done && m.commits.count==1 && value?.kind=="sticker" && value?.bytes==1 && value?.source?.provider=="Synthetic" && a.bytes==[Data([7])]}
 for failed in [false,true]{let a=Session("A");a.pauseTranscribe=true;let m=AppModel(a);let work=m.accountWork();let t=Task{await m.transcribe(projectId:"project",memo:PendingAttachment(name:"memo",mime:"audio/aac",data:Data([8]))WORK)};let began=await spin{a.transcribeWaiter != nil};m.replace(Session("B"));a.finishTranscribe(failed);let value=await t.value;cases[failed ? "stale-transcribe-error-no-paint":"stale-transcript-no-private-return"]=began && value==nil && m.errors.isEmpty}
 do{let a=Session("A");let m=AppModel(a);let work=m.accountWork();let value=await m.transcribe(projectId:"project",memo:PendingAttachment(name:"memo",mime:"audio/aac",data:Data([8]))WORK);cases["current-transcript-control"]=value=="A-transcript"}
 do{let a=Session("A");let m=AppModel(a);m.isFixture=true;let work=m.accountWork();let value=await m.transcribe(projectId:"project",memo:PendingAttachment(name:"memo",mime:"audio/aac",data:Data([8]))WORK);cases["fixture-transcribe-no-request"]=value==nil && a.authorizations.isEmpty}
 let failed=cases.values.filter{!$0}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":cases.count-failed,"failed":failed,"total":cases.count,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--tree',type=Path,required=True);ap.add_argument('--out',type=Path,required=True);a=ap.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(exist_ok=False)
 paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];app,api,helper=[p.read_text()for p in paths];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths}
 fixture=F+u.method(api,'bound')+'\n}\n'+helper
 if 'struct AccountWork' in app:
  m=re.search(r'^struct AccountWork \{',app,re.M);fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
 else:
  fixture+='\nstruct AccountWork{let owner:WorkLifetime.Ticket;let api:RelayAPI}\n'
 fixture+=M
 if 'struct AccountWork' not in app:fixture+=' func accountWork()->AccountWork?{AccountWork(owner:accountLifetime.ticket!,api:api)}\n'
 for name in ['accountWork','boundCallAPI']:
  if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=u.method(app,name)+'\n'
 fixture+=u.method(app,'uploadBlob')+'\n'+u.method(app,'transcribe')+'\n'
 media=u.method(app,'sendMedia');start=media.index('            let slot = people ?');end=media.rindex('\n        }');branch=media[start:end]
 fixture+='''    func sticker(pack:Pack,item:Item,data:Data) {
        let people=false;let chatId="chat";let name="item.png";let source=AttachmentSource(provider:pack.name,id:"pack/item",pageUrl:pack.pageUrl)
'''+branch+'\n    }\n'+T.replace('WORK',',work:work!' if 'memo: PendingAttachment, work: AccountWork' in app else '')
 start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-ios-media-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'sources':sources,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  record['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
