"""Synthetic effect collaborators and strict source extractors; no live endpoints."""
import re

def method(text,name):
 m=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',text,re.M));assert len(m)==1,(name,len(m))
 return text[m[0].start():text.index('\n    }',m[0].end())+6]

F=r'''
import Foundation
struct RelayError:Error{let status:Int}
final class CallAuthority{var bearer:String;init(_ token:String){bearer=token}}
struct RelayEndpoint{let httpUrl="http://synthetic.invalid"}
@MainActor final class Session{
 let label:String;var authorizations:[String]=[];var names:[String]=[];var bytes:[Data]=[];var mimes:[String]=[];var pause=false;var delay:UInt64=0;var delays:[String:UInt64]=[:];var active=0;var peak=0;var failNames:Set<String>=[];var waiters:[CheckedContinuation<String,Error>]=[];var waiter:CheckedContinuation<String,Error>?{waiters.first}
 init(_ label:String){self.label=label}
 func put(_ key:String,_ mime:String,_ data:Data)async throws->String{names.append(key);bytes.append(data);mimes.append(mime);active+=1;peak=max(peak,active);defer{active-=1};let wait=delays[key] ?? delay;if wait>0{try await Task.sleep(nanoseconds:wait)};if pause{return try await withCheckedThrowingContinuation{waiters.append($0)}};if failNames.contains(key){throw RelayError(status:500)};return key}
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
   func send(_ m:AppModel,_ override:[PendingAttachment]?=nil){let attachments=override ?? attachments;if people{m.peopleUpload(attachments:attachments)}else{m.send(chatId:"chat",text:"synthetic",attachments:attachments,replyTo:"reply",forwardedFrom:"forward",extraParts:[["kind":"chart"]])}}
   do{let a=Session("A");let m=AppModel(a);let b=Session("B");send(m);m.replace(b);for _ in 0..<100{await Task.yield()};record(prefix+"-queued-A-never-uses-B",b.authorizations.isEmpty && m.commits.isEmpty)}
   for failed in [false,true]{let a=Session("A");a.pause=true;let m=AppModel(a);send(m,attachments+[PendingAttachment(name:"c",mime:"text/plain",data:Data([3])),PendingAttachment(name:"d",mime:"text/plain",data:Data([4]))]);let began=await spin{a.waiter != nil};let admitted=a.names;let b=Session("B");m.replace(b);a.finish(failed);for _ in 0..<100{await Task.yield()};record(prefix+(failed ? "-stale-error-no-paint":"-stale-success-no-commit"),began && b.authorizations.isEmpty && m.commits.isEmpty && m.errors.isEmpty && a.names==admitted && !admitted.isEmpty)}
   do{let a=Session("A");a.failNames=["a"];let m=AppModel(a);send(m);let done=await spin{!m.commits.isEmpty};let c=m.commits.first;record(prefix+"-current-partial-failure-policy",done && m.commits.count==1 && Set(a.names)==Set(["a","b"]) && a.names.count==2 && m.errors.count==1 && (people ? c?.attachments.map(\.name)==["b"] : c?.attachments.map(\.name)==["a","b"] && c?.attachments.first?.blobRef==nil) && c?.attachments.last?.blobRef=="b" && c?.forwarded=="forward" && c?.extraParts.first?["kind"] as? String=="chart" && c?.reply=="reply")}
   do{let a=Session("A");let m=AppModel(a);send(m);let done=await spin{!m.commits.isEmpty};record(prefix+"-current-order-bytes-single-commit",done && m.commits.count==1 && m.commits[0].attachments.map(\.name)==["a","b"] && Dictionary(uniqueKeysWithValues:zip(a.names,a.bytes))==["a":Data([1]),"b":Data([2])] && Dictionary(uniqueKeysWithValues:zip(a.names,a.mimes))==["a":"image/png","b":"text/plain"] && m.errors.isEmpty)}
  }
  for people in [true,false]{
   let prefix=people ? "people":"project";let many=(0..<4).map{PendingAttachment(name:String($0),mime:"application/test",data:Data([UInt8($0)]))}
   func send(_ m:AppModel){if people{m.peopleUpload(attachments:many)}else{m.send(chatId:"chat",text:"synthetic",attachments:many,replyTo:"reply",forwardedFrom:"forward",extraParts:[["kind":"chart"]])}}
   do{let a=Session("A");a.delay=2_000_000;a.delays=["0":10_000_000,"1":1_000_000];let m=AppModel(a);send(m);let done=await spin{!m.commits.isEmpty};if !done{try? await Task.sleep(nanoseconds:30_000_000)};let c=m.commits.first;record(prefix+"-bounded-out-of-order-single-commit",c?.attachments.map(\.name)==["0","1","2","3"] && m.commits.count==1 && a.peak==PEAK && a.active==0 && Set(a.names)==Set(["0","1","2","3"]))}
   do{let a=Session("A");a.failNames=Set(["0","2"]);a.delays=["0":10_000_000];let m=AppModel(a);send(m);try? await Task.sleep(nanoseconds:30_000_000);let c=m.commits.first;record(prefix+"-multiple-failures-original-row-policy",m.commits.count==1 && m.errors.count==2 && (people ? c?.attachments.map(\.name)==["1","3"] : c?.attachments.map(\.name)==["0","1","2","3"] && c?.attachments[0].blobRef==nil && c?.attachments[2].blobRef==nil))}
   do{let a=Session("A");a.failNames=Set(["0","1","2","3"]);let m=AppModel(a);send(m);let done=await spin{!m.commits.isEmpty};let c=m.commits.first;record(prefix+"-all-failures-still-one-original-commit",done && m.commits.count==1 && m.errors.count==4 && (people ? c?.attachments.isEmpty==true : c?.attachments.count==4 && c?.attachments.allSatisfy{$0.blobRef==nil}==true))}
  }
  do{let a=Session("A");let m=AppModel(a);m.isFixture=true;m.send(chatId:"chat",text:"synthetic",attachments:attachments);record("project-fixture-no-network-control",a.authorizations.isEmpty && m.commits.count==1 && m.commits[0].attachments.allSatisfy{$0.blobRef==nil})}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":results.count-failed,"failed":failed,"total":results.count,"results":results],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
