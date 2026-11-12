"""Actual viewer/library copy callbacks and shared copy methods; synthetic effects only."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
def method(s,name):
 matches=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',s,re.M));assert len(matches)==1,(name,len(matches));at=matches[0].start();return s[at:s.index('\n    }',matches[0].end())+6]
def block(s,needle):
 at=s.index(needle);return s[at:s.index('\n}',at)+2]
F=r'''
import Foundation
struct RelayAPI{}
struct AccountWork{let owner:WorkLifetime.Ticket;let api:RelayAPI}
struct Notice{let text:String}
struct Attachment{var name:String;var mime:String;var blobRef:String?;var kind:String?;var isOnRelay:Bool{blobRef.map{!$0.hasPrefix("local:")} ?? false}}
struct MediaItem{enum Kind{case image,video,file};var id="same";var name="synthetic.png";var mime="image/png";var kind=Kind.image;var blobRef:String?="ref";var slot="same-slot";var by="";var at=0;var pictureURL:String?=nil}
struct ChatItem{var media:MediaItem?=MediaItem()}
enum MediaAlbum{static func kind(of a:Attachment)->MediaItem.Kind?{.image}}
@MainActor enum MessagePasteboard{struct Fetched{let name:String;let mime:String;let data:Data};static var writes=0;static func items(text:String,fetched:[Fetched])->[Int]{text.isEmpty && fetched.isEmpty ? []:[1]};static func write(_ values:[Int]){writes+=1}}
@MainActor enum MediaFetch{
 static var calls:[String]=[];static var pause=false;static var fail=false;static var waiter:CheckedContinuation<Void,Never>?
 static func reset(){calls=[];pause=false;fail=false;waiter=nil;MessagePasteboard.writes=0}
 static func bytes(for item:MediaItem,model:AppModel,isCurrent:@MainActor()->Bool={true})async->(Data,String)?{
  guard isCurrent()else{return nil};let captured=model.current;calls.append(captured);if pause{await withCheckedContinuation{waiter=$0}};return fail ? nil:(Data(captured.utf8),"image/png")
 }
 static func finish(){pause=false;let saved=waiter;waiter=nil;saved?.resume()}
}
@MainActor final class AppModel{
 var accountLifetime=WorkLifetime();var current="A";var isFixture=false;var hasAccount=true;var credentials:Bool?{hasAccount ? true:nil};var notice:Notice?
 func accountWork()->AccountWork?{guard hasAccount,let owner=accountLifetime.ticket else{return nil};return AccountWork(owner:owner,api:RelayAPI())}
 func replace(){accountLifetime.close();accountLifetime.reopen();current="B";notice=nil}
'''
V=r'''
@MainActor final class Viewer{
 let model:AppModel;var item:MediaItem?=MediaItem();var canAct=true;var busy=false;var flashes:[String]=[]
 init(_ model:AppModel){self.model=model}
 func flash(_ s:String){flashes.append(s)}
 func flash(_ s:String,origin:MediaOrigin){flashes.append(s)}
 func queued(_ origin:MediaOrigin){Task{await COPY}}
 func run(_ origin:MediaOrigin)async{await COPY}
'''
L=r'''
}
@MainActor final class Library{
 let model:AppModel;var item=ChatItem();init(_ model:AppModel){self.model=model}
 func invoke(_ origin:MediaOrigin){LIBRARY}
'''
T=r'''
}
@main struct Checks{
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func settle()async{for _ in 0..<100{await Task.yield()}}
 @MainActor static func main()async{
 var cases:[String:Bool]=[:]
 do{MediaFetch.reset();let m=AppModel();let v=Viewer(m);let origin=MediaOrigin(m);v.queued(origin);m.replace();await settle();cases["queued-viewer-A-never-fetches-or-pastes-B"]=MediaFetch.calls.isEmpty && MessagePasteboard.writes==0 && m.notice==nil && v.flashes.isEmpty}
 do{MediaFetch.reset();MediaFetch.pause=true;let m=AppModel();let v=Viewer(m);let origin=MediaOrigin(m);let task=Task{await v.run(origin)};let began=await spin{MediaFetch.waiter != nil};m.replace();MediaFetch.finish();await task.value;cases["paused-viewer-A-no-post-retire-clipboard-or-notice"]=began && MediaFetch.calls==["A"] && MessagePasteboard.writes==0 && m.notice==nil && v.flashes.isEmpty}
 do{MediaFetch.reset();let m=AppModel();let v=Library(m);let origin=MediaOrigin(m);m.replace();v.invoke(origin);await settle();cases["queued-library-render-A-never-admits-B"]=MediaFetch.calls.isEmpty && MessagePasteboard.writes==0 && m.notice==nil}
 do{MediaFetch.reset();MediaFetch.pause=true;let m=AppModel();let v=Library(m);let origin=MediaOrigin(m);v.invoke(origin);let began=await spin{MediaFetch.waiter != nil};m.replace();MediaFetch.finish();await settle();cases["paused-library-A-no-post-retire-clipboard-or-notice"]=began && MediaFetch.calls==["A"] && MessagePasteboard.writes==0 && m.notice==nil}
 do{MediaFetch.reset();let m=AppModel();let v=Viewer(m);await v.run(MediaOrigin(m));cases["current-viewer-copy-control"]=MediaFetch.calls==["A"] && MessagePasteboard.writes==1 && m.notice?.text=="Copied the picture" && v.flashes==["Copied the picture"] && !v.busy}
 do{MediaFetch.reset();let m=AppModel();let v=Library(m);v.invoke(MediaOrigin(m));let done=await spin{MessagePasteboard.writes==1};cases["current-library-copy-control"]=done && MediaFetch.calls==["A"] && m.notice?.text=="Copied the picture"}
 do{MediaFetch.reset();MediaFetch.fail=true;let m=AppModel();let v=Viewer(m);await v.run(MediaOrigin(m));cases["current-download-failure-note-control"]=MediaFetch.calls==["A"] && MessagePasteboard.writes==0 && m.notice != nil && v.flashes.isEmpty && !v.busy}
 do{MediaFetch.reset();let m=AppModel();let origin=MediaOrigin(m);m.replace();let a=Attachment(name:"local",mime:"image/png",blobRef:"local:pending",kind:"image");_ = await DIRECT;cases["retired-local-attachment-no-B-notice"]=MediaFetch.calls.isEmpty && MessagePasteboard.writes==0 && m.notice==nil}
 let failed=cases.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["passed":cases.count-failed,"failed":failed,"total":cases.count,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
 rels=['UI/MediaViewer.swift','UI/MessagePasteboard.swift','UI/LibraryList.swift','UI/ChatView.swift','Sync/WorkLifetime.swift','Store/AppModel.swift'];paths=[tree/(P+name)for name in rels];media,board,library,chat,helper,app=[p.read_text()for p in paths];hashes={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths}
 origin=block(media,'enum MediaOrigin {')if 'enum MediaOrigin {'in media else 'typealias MediaOrigin=ChatAccountContext\nextension ChatAccountContext{@MainActor func allowsPrivate(_ model:AppModel)->Bool{accepts(model)}}'
 # Only test-collaborator constructors are simplified; production methods are verbatim.
 fixture=helper+'\n'+F+method(app,'acceptsAccountWork')+'\n'+method(board,'copyAttachment')+'\n'+method(board,'copy')+'\n}\n'+block(chat,'enum ChatAccountContext:')+'\n'+origin+'\n'+V+method(media,'copy')+'\n'+L+method(library,'copy')+T
 viewer_call='copy(origin:origin)'if 'func copy(origin:'in media else'copy()';library_call='copy(origin:origin)'if 'func copy(origin:'in library else'copy()';direct='m.copyAttachment(a,slot:"slot",isCurrent:{origin.allowsPrivate(m)})'if 'func copyAttachment(_ attachment: Attachment, slot: String, isCurrent:'in board else'm.copyAttachment(a,slot:"slot")'
 fixture=fixture.replace('COPY',viewer_call).replace('LIBRARY',library_call).replace('DIRECT',direct)
 # MediaItem's production memberwise labels are kept by the simplified collaborator.
 fixture=fixture.replace('struct MediaItem{enum Kind','struct MediaItem{enum Kind')
 tick=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-media-owner-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);row={'sources':hashes,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout)if r.stdout else{});(out/'tests.log').write_text(r.stdout+r.stderr)
  row['seconds']=time.monotonic()-tick;assert hashes=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==8;exit(row['results']['failed']!=0)
if __name__=='__main__':main()
