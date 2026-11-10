"""Actual group edit/share controller bodies with paused account-bound collaborators."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--model',type=Path,required=True);p.add_argument('--lifetime',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);rel='apps/ios/Arbos/Arbos/UI/ProfilePages.swift';src=(tree/rel).read_text();model=a.model.read_text();helper=a.lifetime.read_text()
def method(text,name):
 m=re.search(r'^    (?:private )?func '+name+r'\(',text,re.M);assert m,name;return text[m.start():text.index('\n    }',m.end())+6].replace('private func','func',1)
def block(name):
 start=src.index('struct '+name+': View {');end=src.find('\nstruct ',start+1);return src[start:end if end!=-1 else len(src)]
fixture='import Foundation\n'+helper+r'''
struct AccountWork{let owner:WorkLifetime.Ticket}
struct ServerInviteLink{let id:String;let url:String;var usable=true}
@MainActor final class Session{
 let label:String;var calls:[String]=[];var pause=false;var waiter:CheckedContinuation<Void,Never>?;init(_ label:String){self.label=label}
 func call(_ op:String)async{calls.append(op);if pause{await withCheckedContinuation{waiter=$0}}}
 func finish(){let saved=waiter;waiter=nil;saved?.resume()}
}
@MainActor final class AppModel{
 var session:Session;var accountLifetime=WorkLifetime();var sentBodies:[[String]]=[]
 init(_ s:Session){session=s}
 func replace(_ s:Session){accountLifetime.close();accountLifetime.reopen();session=s}
 func accountWork()->AccountWork?{accountLifetime.ticket.map{AccountWork(owner:$0)}}
 func patchPeopleChat(_ id:String,title:String,description:String,expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{guard let owner=accountLifetime.ticket,expectedOwner==nil || expectedOwner==owner else{return false};let api=session;sentBodies.append([title,description]);await api.call("patch");return accountLifetime.accepts(owner)}
 func createGroupInvite(_ id:String,expires:Bool,singleUse:Bool,expectedOwner:WorkLifetime.Ticket?=nil)async->ServerInviteLink?{guard let owner=accountLifetime.ticket,expectedOwner==nil || expectedOwner==owner else{return nil};let api=session;await api.call("invite");guard accountLifetime.accepts(owner)else{return nil};return ServerInviteLink(id:api.label,url:"https://synthetic.invalid/g/"+api.label)}
'''
if 'model.acceptsAccountWork(work)'in src:fixture+=method(model,'acceptsAccountWork')+'\n'
fixture+='}\n'
# Compile the actual rendered-capability binding lines as well as controller bodies.
# This catches accidental self-reference or an unavailable helper before a full SwiftUI gate.
for i,line in enumerate(re.findall(r'^        let work = model\.accountWork[^\n]+',src,re.M)):
 fixture+='@MainActor func renderCapture'+str(i)+'(_ model:AppModel){\n'+line+'\n _ = work\n}\n'
fixture+=r'''
@MainActor final class GroupEditSheet{
 let model:AppModel;var saving=false;var name="  title  ";var descriptionText=String(repeating:"x",count:240);var chatId="same";var dismissed=false;init(_ model:AppModel){self.model=model};func dismiss(){dismissed=true}
'''+method(block('GroupEditSheet'),'save')+'\n}\n'+r'''
@MainActor final class GroupProfileView{
 let model:AppModel;var sharingInvite=false;var groupInvites:[ServerInviteLink]=[];var chatId="same";var inviteExpires=false;var inviteSingleUse=false;var shares:[String]=[];init(_ model:AppModel){self.model=model};func presentShareSheet(items:[String]){shares+=items}
'''+method(block('GroupProfileView'),'shareGroupInvite')+'\n}\n'
edit='await v.save(work:work)'if 'func save(work:'in block('GroupEditSheet')else'await v.save()';share='v.shareGroupInvite(work:work)'if 'func shareGroupInvite(work:'in block('GroupProfileView')else'v.shareGroupInvite()'
fixture+=r'''
@main struct Checks{
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func settle()async{for _ in 0..<100{await Task.yield()}}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{let a=Session("A");let m=AppModel(a);let v=GroupEditSheet(m);let work=m.accountWork()!;let old=Task{EDIT};let b=Session("B");m.replace(b);await old.value;record("queued-group-edit-refuses-B",b.calls.isEmpty && !v.dismissed)}
  do{let a=Session("A");a.pause=true;let m=AppModel(a);let v=GroupEditSheet(m);let work=m.accountWork()!;let old=Task{EDIT};let began=await spin{a.waiter != nil};m.replace(Session("B"));a.finish();await old.value;record("late-group-edit-no-dismiss-or-busy-release",began && !v.dismissed && v.saving)}
  do{let a=Session("A");let m=AppModel(a);let v=GroupEditSheet(m);let work=m.accountWork()!;EDIT;record("current-group-edit-fields-control",v.dismissed && !v.saving && m.sentBodies==[["title",String(repeating:"x",count:200)]] && a.calls==["patch"])}
  do{let a=Session("A");let m=AppModel(a);let v=GroupProfileView(m);let work=m.accountWork()!;SHARE;let b=Session("B");m.replace(b);await settle();record("queued-invite-share-refuses-B",b.calls.isEmpty && v.shares.isEmpty)}
  do{let a=Session("A");a.pause=true;let m=AppModel(a);let v=GroupProfileView(m);let work=m.accountWork()!;SHARE;let began=await spin{a.waiter != nil};m.replace(Session("B"));a.finish();await settle();record("late-invite-no-share-or-busy-release",began && v.shares.isEmpty && v.groupInvites.isEmpty && v.sharingInvite)}
  do{let a=Session("A");let m=AppModel(a);let v=GroupProfileView(m);v.groupInvites=[ServerInviteLink(id:"old",url:"https://synthetic.invalid/g/old")];let work=m.accountWork()!;SHARE;let ended=await spin{!v.sharingInvite};record("current-cached-share-control",ended && v.shares==["https://synthetic.invalid/g/old"] && a.calls.isEmpty)}
  do{let a=Session("A");let m=AppModel(a);let v=GroupProfileView(m);let work=m.accountWork()!;SHARE;let ended=await spin{!v.sharingInvite};record("current-created-share-control",ended && v.shares==["https://synthetic.invalid/g/A"] && v.groupInvites.first?.id=="A" && a.calls==["invite"])}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
fixture=fixture.replace('EDIT',edit).replace('SHARE',share);hashes={rel:hashlib.sha256(src.encode()).hexdigest(),'AppModel.swift':hashlib.sha256(model.encode()).hexdigest(),'WorkLifetime.swift':hashlib.sha256(helper.encode()).hexdigest()};(out/'fixture.swift').write_text(fixture);start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-profile-controls-')as raw:
 tmp=Path(raw);source=tmp/'main.swift';source.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(source),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':hashes,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=25);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert src==(tree/rel).read_text() and model==a.model.read_text() and helper==a.lifetime.read_text();(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==7,'Compile/count guard';exit(row['results']['failed']!=0)
