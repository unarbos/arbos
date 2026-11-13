"""Actual member-picker add/create controllers over an owner-aware fake model edge."""
from pathlib import Path
import argparse,hashlib,importlib.util,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
spec=importlib.util.spec_from_file_location('u',HERE/'check_uploads.py');u=importlib.util.module_from_spec(spec);spec.loader.exec_module(u)
F=r'''
import Foundation
typealias Id=String
struct AccountWork{let owner:WorkLifetime.Ticket}
struct Person{let id:String;let handle:String}
struct Project{let id:String}
struct Server{let id:String}
struct Secret{let id:String}
struct Chat{let id:String}
struct User{let id:String}
struct Draft{var title:String?=nil;var discColor:UInt8?=nil;var picture:String?=nil}
struct UserError{let sentence:String;static func refused(_ action:String,_ sentence:String)->Self{Self(sentence:sentence)}}
struct AddMembersPlan{let chatId:String;let handles:[String];let agentIds:[String];var isEmpty:Bool{handles.isEmpty && agentIds.isEmpty};static func make(chat:Chat,meId:String?,picked:[Person],pickedAgents:[Project])->Self{Self(chatId:chat.id,handles:picked.map(\.handle),agentIds:pickedAgents.map(\.id))}}
@MainActor final class AppModel{
 var accountLifetime=WorkLifetime();var user:User?=User(id:"A");var requests:[String]=[];var pause=false;var waiter:CheckedContinuation<Bool,Never>?
 func accountWork()->AccountWork?{accountLifetime.ticket.map{AccountWork(owner:$0)}}
 func acceptsAccountWork(_ work:AccountWork?)->Bool{work.map{accountLifetime.accepts($0.owner)} ?? false}
 func replace(){accountLifetime.close();accountLifetime.reopen();user=User(id:"B")}
 func request(_ name:String,_ owner:WorkLifetime.Ticket?)async->Bool{if let owner,!accountLifetime.accepts(owner){return false};requests.append((user?.id ?? "")+":"+name);if pause{return await withCheckedContinuation{waiter=$0}};return true}
 func addSecretPeople(secretId:String,handles:[String],expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{await request("secret",expectedOwner)}
 func addServerMembers(serverId:String,userIds:[String],expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{await request("server",expectedOwner)}
 func addMembers(chatId:String,handles:[String],agentIds:[String],expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{await request("group",expectedOwner)}
 func createGroup(handles:[String],agentIds:[String],title:String?,discColor:UInt8?,picture:String?,expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{await request("create",expectedOwner)}
}
@MainActor final class Picker{
 let model:AppModel;var secret:Secret?;var server:Server?;var picked=[Person(id:"P",handle:"synthetic")];var pickedAgents:[Project]=[];var creating=false;var localError:UserError?;var canSubmit=true;var addToChat:Chat?=Chat(id:"same");var draft:Draft?;var done=0
 init(_ model:AppModel,_ kind:String){self.model=model;if kind=="secret"{secret=Secret(id:"same")};if kind=="server"{server=Server(id:"same")}}
 func isIn(_ person:Person)->Bool{false};func onDone(){done+=1}
'''
T=r'''
}
@main struct Checks{
 @MainActor static func spin(_ p:()->Bool)async->Bool{for _ in 0..<10000{if p(){return true};await Task.yield()};return p()}
 @MainActor static func main()async{
 var cases:[String:Bool]=[:]
 for kind in ["secret","server","group","create"]{
  func send(_ view:Picker,_ work:AccountWork?){view.trigger(kind,work)}
  do{let model=AppModel();let view=Picker(model,kind);let old=model.accountWork();model.replace();send(view,old);for _ in 0..<100{await Task.yield()};cases[kind+"-retired-render-no-B-action"]=model.requests.isEmpty && view.done==0}
  do{let model=AppModel();model.pause=true;let view=Picker(model,kind);send(view,model.accountWork());let began=await spin{model.waiter != nil};model.replace();view.creating=true;model.waiter?.resume(returning:true);model.waiter=nil;for _ in 0..<100{await Task.yield()};cases[kind+"-late-result-no-B-dismiss-or-cleanup"]=began && view.done==0 && view.creating}
  do{let model=AppModel();let view=Picker(model,kind);send(view,model.accountWork());let done=await spin{view.done==1};cases[kind+"-current-control"]=done && !view.creating && model.requests==["A:"+kind]}
 }
 let failed=cases.values.filter{!$0}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":cases.count-failed,"failed":failed,"total":cases.count,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--tree',type=Path,required=True);ap.add_argument('--out',type=Path,required=True);a=ap.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(exist_ok=False)
 paths=[tree/(P+q)for q in ['UI/PeopleSheets.swift','Sync/WorkLifetime.swift']];source,helper=[p.read_text()for p in paths];source=source[source.index('struct NewGroupView:'):source.index('struct ProjectMembersSheet:')];methods=u.method(source,'add')+'\n'+u.method(source,'create');test=T.replace('CREATE','work:work'if 'func create(work:'in methods else'').replace('ADD','work:work'if 'func add(work:'in methods else'');facade='\n func trigger(_ kind:String,_ work:AccountWork?){if kind=="create"{create('+('work:work'if'func create(work:'in methods else'')+')}else{add('+('work:work'if'func add(work:'in methods else'')+')}}\n';fixture=helper+F+methods+facade+test;sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-ios-people-actions-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'sources':sources,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  record['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
