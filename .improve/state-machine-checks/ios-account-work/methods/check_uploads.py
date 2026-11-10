"""Actual upload task branches; synthetic API/commits, no blobs or network."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent
P='apps/ios/Arbos/Arbos/'
def method(text,name):
 m=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1,(name,len(m))
 return text[m[0].start():text.index('\n    }',m[0].end())+6]
F=r'''
import Foundation
struct RelayError:Error{let status:Int}
final class CallAuthority{var bearer:String;init(_ token:String){bearer=token}}
struct RelayEndpoint{let httpUrl="http://synthetic.invalid"}
@MainActor final class Session{
 let label:String;var authorizations:[String]=[];var names:[String]=[];var bytes:[Data]=[];var mimes:[String]=[];var pause=false;var failNames:Set<String>=[];var waiters:[CheckedContinuation<String,Error>]=[];var waiter:CheckedContinuation<String,Error>?{waiters.first}
 init(_ label:String){self.label=label}
 func put(_ key:String,_ mime:String,_ data:Data)async throws->String{names.append(key);bytes.append(data);mimes.append(mime);if pause{return try await withCheckedThrowingContinuation{waiters.append($0)}};if failNames.contains(key){throw RelayError(status:500)};return key}
 func finish(_ failed:Bool=false){let saved=waiters;waiters=[];pause=false;for w in saved{if failed{w.resume(throwing:RelayError(status:500))}else{w.resume(returning:"opaque")}}}
}
@MainActor final class RelayAPI{
 let endpoint=RelayEndpoint();let session:Session;var token:()->String;var refresher:(()async->Bool)?;var sourceAuthorization:(@MainActor ()->String?)?;var cleanupAuthority:CallAuthority?
 var authorization:(@MainActor ()->String?)?{get{sourceAuthorization}set{sourceAuthorization=newValue}}
 init(_ s:Session){session=s;token={s.label}}
 private init(endpoint:RelayEndpoint,token:@escaping ()->String,session:Session){self.session=session;self.token=token}
 func putBlob(projectId:String,key:String,mime:String,data:Data)async throws->String{if let sourceAuthorization{guard let bearer=sourceAuthorization()else{throw CancellationError()};session.authorizations.append(bearer)}else{session.authorizations.append(token())};return try await session.put(key,mime,data)}
'''
M=r'''
typealias Id=String
struct UserError{static func classify(_ label:String,_ error:Error)->String{label}}
struct Attachment:Equatable{var name:String;var mime:String;var blobRef:String?;var kind:String?}
struct PendingAttachment{let name:String;let mime:String;let data:Data}
struct ReplyRef{let id:String}
struct Chat{let projectId:String}
struct CoreChatDoc{struct Author{let id:String}}
enum JobTabs{static func isJobChat(_ id:String)->Bool{false}}
@MainActor final class AppModel{
 var credentials:Bool?=true;var accountLifetime=WorkLifetime();var api:RelayAPI;var isFixture=false;let peopleSlot="people";var commits:[Commit]=[];var errors:[String]=[];var marked:[String]=[]
 struct Commit{let attachments:[Attachment];let forwarded:String?;let extraParts:[[String:Any]];let reply:String?;let messageId:String?}
 init(_ s:Session){api=RelayAPI(s)}
 func replace(_ s:Session){accountLifetime.close();accountLifetime.reopen();api=RelayAPI(s);commits=[];errors=[]}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{expectedOwner.map{accountLifetime.accepts($0)} ?? true}
 static func blobKey(for name:String)->String{name}
 static func attachmentKind(mime:String,name:String)->String?{mime.hasPrefix("image/") ? "image":nil}
 func clearSendError(_ id:String){}
 func steerFromJobTab(chatId:String,text:String){}
 func chat(_ id:String)->Chat?{Chat(projectId:"project")}
 func setSendError(_ id:String,_ error:String){errors.append(error)}
 func markUnacked(chatId:String,messageId:String){marked.append(messageId)}
 func write(chatId:String,text:String,command:String,attachments:[Attachment],replyTo:String?=nil,forwardedFrom:String?=nil,extraParts:[[String:Any]]=[]) {commits.append(Commit(attachments:attachments,forwarded:forwardedFrom,extraParts:extraParts,reply:replyTo,messageId:nil))}
 func commitPeople(chatId:String,doc:CoreChatDoc,messageId:String,text:String,author:CoreChatDoc.Author,mentions:[String],reply:String?,attachments:[Attachment],alreadyUnacked:Bool,forwardedFrom:String?,replyRef:ReplyRef?,extraParts:[[String:Any]]){commits.append(Commit(attachments:attachments,forwarded:forwardedFrom,extraParts:extraParts,reply:reply,messageId:messageId))}
'''
T=r'''
}
@main struct Checks{
 @MainActor static func spin(_ p:()->Bool)async->Bool{for _ in 0..<10000{if p(){return true};await Task.yield()};return p()}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  let attachments=[PendingAttachment(name:"a",mime:"image/png",data:Data([1])),PendingAttachment(name:"b",mime:"text/plain",data:Data([2]))]
  for people in [true,false]{
   let prefix=people ? "people":"project"
   func send(_ m:AppModel){if people{m.peopleUpload(attachments:attachments)}else{m.send(chatId:"chat",text:"synthetic",attachments:attachments,replyTo:"reply",forwardedFrom:"forward",extraParts:[["kind":"chart"]])}}
   do{let a=Session("A");let m=AppModel(a);let b=Session("B");send(m);m.replace(b);for _ in 0..<100{await Task.yield()};record(prefix+"-queued-A-never-uses-B",b.authorizations.isEmpty && m.commits.isEmpty)}
   for failed in [false,true]{let a=Session("A");a.pause=true;let m=AppModel(a);send(m);let began=await spin{a.waiter != nil};let admitted=a.names;let b=Session("B");m.replace(b);a.finish(failed);for _ in 0..<100{await Task.yield()};record(prefix+(failed ? "-stale-error-no-paint":"-stale-success-no-commit"),began && b.authorizations.isEmpty && m.commits.isEmpty && m.errors.isEmpty && a.names==admitted && !admitted.isEmpty)}
   do{let a=Session("A");a.failNames=["a"];let m=AppModel(a);send(m);let done=await spin{!m.commits.isEmpty};let c=m.commits.first;record(prefix+"-current-partial-failure-policy",done && m.commits.count==1 && Set(a.names)==Set(["a","b"]) && a.names.count==2 && m.errors.count==1 && (people ? c?.attachments.map(\.name)==["b"] : c?.attachments.map(\.name)==["a","b"] && c?.attachments.first?.blobRef==nil) && c?.attachments.last?.blobRef=="b" && c?.forwarded=="forward" && c?.extraParts.first?["kind"] as? String=="chart" && c?.reply=="reply")}
   do{let a=Session("A");let m=AppModel(a);send(m);let done=await spin{!m.commits.isEmpty};record(prefix+"-current-order-bytes-single-commit",done && m.commits.count==1 && m.commits[0].attachments.map(\.name)==["a","b"] && Dictionary(uniqueKeysWithValues:zip(a.names,a.bytes))==["a":Data([1]),"b":Data([2])] && Dictionary(uniqueKeysWithValues:zip(a.names,a.mimes))==["a":"image/png","b":"text/plain"] && m.errors.isEmpty)}
  }
  do{let a=Session("A");let m=AppModel(a);m.isFixture=true;m.send(chatId:"chat",text:"synthetic",attachments:attachments);record("project-fixture-no-network-control",a.authorizations.isEmpty && m.commits.count==1 && m.commits[0].attachments.allSatisfy{$0.blobRef==nil})}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":results.count-failed,"failed":failed,"total":results.count,"results":results],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--tree',type=Path,required=True);ap.add_argument('--out',type=Path,required=True);a=ap.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(exist_ok=False)
 paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];app,api,helper=[p.read_text()for p in paths];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths}
 fixture=F+method(api,'bound')+'\n}\n'+helper
 if 'struct AccountWork' in app:
  m=re.search(r'^struct AccountWork \{',app,re.M);fixture+='\n'+app[m.start():app.index('\n}',m.start())+2]
 fixture+=M
 for name in ['accountWork','boundCallAPI']:
  if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=method(app,name)+'\n'
 if 'func uploadAttachments('in app:
  at=app.index('    private enum AttachmentUpload {');fixture+=app[at:app.index('\n    }',at)+6]+'\n'+method(app,'uploadAttachments')+'\n'
 fixture+=method(app,'send')+'\n'
 people=method(app,'sendPeople');begin=people.index('        markUnacked(chatId: chatId, messageId: messageId)')
 if 'let work = accountWork()' in people[:begin]:begin=people.rindex('        guard let work = accountWork()',0,begin)
 fixture+='''    func peopleUpload(chatId:String="chat",attachments:[PendingAttachment],forwardedFrom:String?="forward",replyRef:ReplyRef?=nil,extraParts:[[String:Any]]=[["kind":"chart"]]) {
        let doc=CoreChatDoc();let messageId="message";let text="synthetic";let author=CoreChatDoc.Author(id:"A");let mentions=["user:synthetic"];let reply:String?="reply"
'''+people[begin:]+T
 start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-ios-upload-')as raw:
  tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'sources':sources,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  record['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
