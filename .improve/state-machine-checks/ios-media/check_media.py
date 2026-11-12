"""Actual viewer save/page methods with deterministic synthetic permission/media effects."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
import runpy
_helper=runpy.run_path(str(Path(__file__).with_name("check_copy.py")))
method,block=_helper["method"],_helper["block"]
P='apps/ios/Arbos/Arbos/'
F=r'''
import Foundation
struct RelayAPI{}
struct AccountWork{let owner:WorkLifetime.Ticket;let api:RelayAPI}
struct MediaItem{enum Kind{case image,video,file};var name="synthetic.png";var kind=Kind.image;var blobRef:String?="same-ref";var pictureURL:String?=nil}
@MainActor final class AppModel{
 var accountLifetime=WorkLifetime();var isFixture=false;var credentials:Bool?=true;var current="A"
 func accountWork()->AccountWork?{guard credentials != nil,let owner=accountLifetime.ticket else{return nil};return AccountWork(owner:owner,api:RelayAPI())}
 func replace(){accountLifetime.close();accountLifetime.reopen();current="B"}
 func acceptsAccountWork(_ work:AccountWork?)->Bool{guard let work,!Task.isCancelled else{return false};return accountLifetime.accepts(work.owner)}
}
@MainActor enum MediaFetch{
 static var calls:[String]=[];static var temp=0;static var pause=false;static var fail=false;static var waiter:CheckedContinuation<Void,Never>?
 static func reset(){calls=[];temp=0;pause=false;fail=false;waiter=nil;PHPhotoLibrary.reset()}
 static func bytes(for item:MediaItem,model:AppModel,isCurrent:@MainActor()->Bool={true})async->(Data,String)?{guard isCurrent()else{return nil};calls.append(item.pictureURL != nil ? "public":model.current);if pause{await withCheckedContinuation{waiter=$0}};return fail ? nil:(Data([1]),"image/png")}
 static func tempFile(_ data:Data,name:String)->URL?{temp+=1;return URL(fileURLWithPath:"/synthetic/"+name)}
 static func finish(){pause=false;let saved=waiter;waiter=nil;saved?.resume()}
}
struct UIImage{init?(data:Data){}}
@MainActor final class AVPlayer{init(url:URL){}}
struct ShareFile{var url:URL;init(_ url:URL){self.url=url}}
enum Access{case addOnly};enum Status{case authorized,limited,denied};enum Resource{case photo,video}
@MainActor final class PHAssetCreationRequest{static var resources=0;static func forAsset()->PHAssetCreationRequest{Self()};func addResource(with:Resource,data:Data,options:Int?){Self.resources+=1};func addResource(with:Resource,fileURL:URL,options:Int?){Self.resources+=1}}
@MainActor final class PHPhotoLibrary{
 static var requests=0;static var changes=0;static var pause=false;static var pauseChange=false;static var status=Status.authorized;static var waiter:CheckedContinuation<Void,Never>?;static var changeWaiter:CheckedContinuation<Void,Never>?
 static func reset(){requests=0;changes=0;pause=false;pauseChange=false;status = .authorized;waiter=nil;changeWaiter=nil;PHAssetCreationRequest.resources=0}
 static func requestAuthorization(for:Access)async->Status{requests+=1;if pause{await withCheckedContinuation{waiter=$0}};return status}
 static func shared()->PHPhotoLibrary{Self()};func performChanges(_ action:()->Void)async throws{Self.changes+=1;action();if Self.pauseChange{await withCheckedContinuation{Self.changeWaiter=$0}}}
 static func finish(){pause=false;let saved=waiter;waiter=nil;saved?.resume()}
 static func finishChange(){pauseChange=false;let saved=changeWaiter;changeWaiter=nil;saved?.resume()}
}
@MainActor final class Viewer{
 let model:AppModel;var origin:MediaOrigin;var item:MediaItem?=MediaItem();var canAct=true;var busy=false;var shareURL:ShareFile?;var flashes:[String]=[]
 init(_ model:AppModel){self.model=model;origin=MediaOrigin(model)}
 func flash(_ text:String){flashes.append(text)};func flash(_ text:String,origin:MediaOrigin){flashes.append(text)}
 func run()async{await DOWNLOAD}
'''
C=r'''
}
@MainActor final class Video{
 let model:AppModel;var origin:MediaOrigin;var item=MediaItem();var loading=false;var failed=false;var player:AVPlayer?
 init(_ m:AppModel){model=m;origin=MediaOrigin(m)};func run()async{await load()}
'''
D=r'''
}
@MainActor final class File{
 let model:AppModel;var origin:MediaOrigin;var item=MediaItem();var loading=false;var failed=false;var previewURL:URL?
 init(_ m:AppModel){model=m;origin=MediaOrigin(m)};func run()async{await open()}
'''
E=r'''
}
@MainActor final class Picture{
 let model:AppModel;var origin:MediaOrigin;var item=MediaItem();var image:UIImage?;var failed=false
 init(_ m:AppModel){model=m;origin=MediaOrigin(m)};func run()async{await load()}
'''
T=r'''
}
@main struct Checks{
 @MainActor static func spin(_ pred:()->Bool)async->Bool{for _ in 0..<10000{if pred(){return true};await Task.yield()};return pred()}
 @MainActor static func settle()async{for _ in 0..<100{await Task.yield()}}
 @MainActor static func main()async{var c:[String:Bool]=[:]
 do{MediaFetch.reset();let m=AppModel();let v=Viewer(m);m.replace();await v.run();c["queued-save-refuses-replacement"]=MediaFetch.calls.isEmpty && PHPhotoLibrary.requests==0 && v.shareURL==nil && v.flashes.isEmpty}
 do{MediaFetch.reset();MediaFetch.pause=true;let m=AppModel();let v=Viewer(m);let t=Task{await v.run()};let began=await spin{MediaFetch.waiter != nil};m.replace();MediaFetch.finish();await t.value;c["retired-fetch-cannot-request-photos-or-share"]=began && MediaFetch.calls==["A"] && PHPhotoLibrary.requests==0 && MediaFetch.temp==0 && v.shareURL==nil && v.flashes.isEmpty}
 do{MediaFetch.reset();PHPhotoLibrary.pause=true;let m=AppModel();let v=Viewer(m);let t=Task{await v.run()};let began=await spin{PHPhotoLibrary.waiter != nil};m.replace();PHPhotoLibrary.finish();await t.value;c["retired-permission-cannot-save-or-share"]=began && PHPhotoLibrary.changes==0 && MediaFetch.temp==0 && v.shareURL==nil && v.flashes.isEmpty}
 do{MediaFetch.reset();PHPhotoLibrary.pauseChange=true;let m=AppModel();let v=Viewer(m);let t=Task{await v.run()};let began=await spin{PHPhotoLibrary.changeWaiter != nil};m.replace();PHPhotoLibrary.finishChange();await t.value;c["issued-photos-effect-not-recalled-but-no-stale-toast"]=began && PHPhotoLibrary.changes==1 && v.flashes.isEmpty}
 do{MediaFetch.reset();let m=AppModel();let v=Viewer(m);await v.run();c["current-photo-save-control"]=PHPhotoLibrary.changes==1 && PHAssetCreationRequest.resources==1 && v.flashes==["Saved to Photos"] && !v.busy}
 do{MediaFetch.reset();PHPhotoLibrary.status = .denied;let m=AppModel();let v=Viewer(m);await v.run();c["current-denied-photos-share-control"]=PHPhotoLibrary.changes==0 && v.shareURL != nil && MediaFetch.temp==1}
 do{MediaFetch.reset();let m=AppModel();let v=Viewer(m);v.item?.kind = .file;await v.run();c["current-file-share-control"]=PHPhotoLibrary.requests==0 && v.shareURL != nil && MediaFetch.temp==1}
 do{MediaFetch.reset();let m=AppModel();let v=Video(m);m.replace();await v.run();c["queued-video-refuses-replacement"]=MediaFetch.calls.isEmpty && v.player==nil && !v.failed}
 do{MediaFetch.reset();MediaFetch.pause=true;let m=AppModel();let v=Video(m);let t=Task{await v.run()};let began=await spin{MediaFetch.waiter != nil};m.replace();MediaFetch.finish();await t.value;c["retired-video-no-temp-player-publication"]=began && MediaFetch.temp==0 && v.player==nil && !v.failed}
 do{MediaFetch.reset();let m=AppModel();let v=Video(m);await v.run();c["current-video-control"]=v.player != nil && MediaFetch.temp==1 && !v.loading}
 do{MediaFetch.reset();let m=AppModel();let v=File(m);m.replace();await v.run();c["queued-file-refuses-replacement"]=MediaFetch.calls.isEmpty && v.previewURL==nil && !v.failed}
 do{MediaFetch.reset();MediaFetch.pause=true;let m=AppModel();let v=File(m);let t=Task{await v.run()};let began=await spin{MediaFetch.waiter != nil};m.replace();MediaFetch.finish();await t.value;c["retired-file-no-temp-preview-publication"]=began && MediaFetch.temp==0 && v.previewURL==nil && !v.failed}
 do{MediaFetch.reset();let m=AppModel();let v=File(m);v.previewURL=URL(fileURLWithPath:"/synthetic/existing");await v.run();m.replace();await settle();c["queued-cached-preview-does-not-restore-after-retire"]=v.previewURL==nil}
 do{MediaFetch.reset();let m=AppModel();let v=File(m);await v.run();c["current-file-open-control"]=v.previewURL != nil && MediaFetch.temp==1 && !v.loading}
 do{MediaFetch.reset();let m=AppModel();let v=Picture(m);m.replace();await v.run();c["queued-picture-refuses-replacement"]=MediaFetch.calls.isEmpty && v.image==nil && !v.failed}
 do{MediaFetch.reset();MediaFetch.pause=true;let m=AppModel();let v=Picture(m);let t=Task{await v.run()};let began=await spin{MediaFetch.waiter != nil};m.replace();MediaFetch.finish();await t.value;c["retired-picture-no-image-publication"]=began && v.image==nil && !v.failed}
 do{MediaFetch.reset();let m=AppModel();let v=Picture(m);await v.run();c["current-picture-control"]=v.image != nil && !v.failed}
 do{MediaFetch.reset();let m=AppModel();m.credentials=nil;let v=Picture(m);v.item.pictureURL="https://public.invalid/picture";await v.run();c["anonymous-public-picture-control"]=MediaFetch.calls==["public"] && v.image != nil}
 do{MediaFetch.reset();let m=AppModel();m.credentials=nil;let v=Picture(m);await v.run();c["anonymous-private-blob-denied"]=MediaFetch.calls.isEmpty && v.image==nil && !v.failed}
 do{MediaFetch.reset();let m=AppModel();m.isFixture=true;let v=Picture(m);await v.run();c["current-fixture-picture-control"]=MediaFetch.calls==["A"] && v.image != nil}
 let failures=c.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["cases":c,"total":c.count,"passed":c.count-failures,"failed":failures],options:[.sortedKeys]),encoding:.utf8)!);exit(failures==0 ? 0:1)
 }
}
'''
def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
 names=['UI/MediaViewer.swift','UI/ChatView.swift','Sync/WorkLifetime.swift'];paths=[tree/(P+n)for n in names];media,chat,helper=[x.read_text()for x in paths];hashes={str(x.relative_to(tree)):hashlib.sha256(x.read_bytes()).hexdigest()for x in paths}
 origin=block(media,'enum MediaOrigin {')if 'enum MediaOrigin {'in media else 'typealias MediaOrigin=ChatAccountContext'
 image=media[media.index('private struct ZoomableImagePage:'):media.index('private struct VideoPage:')]
 if 'private func load(origin:'in image: picture=method(image,'load').replace('load(origin: MediaOrigin)', 'load()')
 elif 'private func load()'in image: picture=method(image,'load')
 else:
  start=image.index('            if let (data, _) = await MediaFetch.bytes');end=image.index('\n        }\n    }',start);picture='    private func load() async {\n'+image[start:end]+'\n    }'
 video=media[media.index('private struct VideoPage:'):media.index('private struct FilePage:')];file=media[media.index('private struct FilePage:'):media.index('// MARK: - Forward')]
 fixture=helper+'\n'+F+block(chat,'enum ChatAccountContext:')+'\n'+origin+'\n'+method(media,'download')+C+method(video,'load')+D+method(file,'open')+E+picture+T
 # ChatAccountContext/MediaOrigin are top-level, not members of Viewer.
 top=block(chat,'enum ChatAccountContext:')+'\n'+origin;fixture=fixture.replace(F+top,F).replace('import Foundation\nstruct RelayAPI{}',top+'\nimport Foundation\nstruct RelayAPI{}',1)
 fixture=fixture.replace('DOWNLOAD','download(origin:origin)'if 'func download(origin:'in media else'download()')
 tick=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-media-effects-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);(out/'fixture.swift').write_text(fixture);row={'sources':hashes,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout)if r.stdout else{});(out/'tests.log').write_text(r.stdout+r.stderr)
  row['seconds']=time.monotonic()-tick;assert hashes=={str(x.relative_to(tree)):hashlib.sha256(x.read_bytes()).hexdigest()for x in paths};(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==20;exit(row['results']['failed']!=0)
if __name__=='__main__':main()
