"""Actual private link-card load body: fake byte/decode effects, no URLs or network."""
from pathlib import Path
import argparse,hashlib,json,re,runpy,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
def block(s,needle):
 at=s.index(needle);return s[at:s.index('\n}',at)+2]
def method(s,name):
 m=list(re.finditer(r'^    (?:private )?func '+name+r'\(',s,re.M));assert len(m)==1;return s[m[0].start():s.index('\n    }',m[0].end())+6]
F=r'''
import Foundation
struct RelayAPI{}
struct AccountWork{let owner:WorkLifetime.Ticket;let api:RelayAPI}
@MainActor final class AppModel{var accountLifetime=WorkLifetime();var isFixture=false;var credentials:Bool?=true;var label="A";func replace(){accountLifetime.close();accountLifetime.reopen();label="B"};func accountWork()->AccountWork?{guard let owner=accountLifetime.ticket else{return nil};return AccountWork(owner:owner,api:RelayAPI())};func acceptsAccountWork(_ w:AccountWork?)->Bool{guard let w,!Task.isCancelled else{return false};return accountLifetime.accepts(w.owner)}}
struct UIImage{var value:String;init?(data:Data){value=String(data:data,encoding:.utf8)!}}
struct MediaItem{enum Kind{case image};var id:String;var name:String;var mime:String;var kind:Kind;var blobRef:String?;var slot:String;var by:String;var at:Int;var pictureURL:String?=nil}
struct LinkPreview{struct Picture{var ref:String;var mime="image/png"};var siteIcon:Picture?=Picture(ref:"icon");var image:Picture?=Picture(ref:"picture");var slot="same-slot"}
enum LinkBrand{static func of(host:String)->Bool?{host=="branded.invalid" ? true:nil}}
@MainActor enum MediaFetch{
 static var calls:[String]=[];static var pauseAt:Int?=nil;static var waiter:CheckedContinuation<Void,Never>?;static var failures:Set<String>=[]
 static func reset(){calls=[];pauseAt=nil;waiter=nil;failures=[]}
 static func bytes(for item:MediaItem,model:AppModel,isCurrent:@MainActor()->Bool={true})async->(Data,String)?{guard isCurrent()else{return nil};let captured=model.label;calls.append(captured+":"+item.name);if pauseAt==calls.count{await withCheckedContinuation{waiter=$0}};if failures.contains(item.name){return nil};return(Data((captured+":"+item.name).utf8),"image/png")}
 static func finish(){pauseAt=nil;let w=waiter;waiter=nil;w?.resume()}
}
@MainActor final class Card{
 let model:AppModel;let origin:MediaOrigin;var preview=LinkPreview();let messageId="same-message";var host="plain.invalid";var siteIcon:UIImage?;var picture:UIImage?
 init(_ model:AppModel){self.model=model;origin=MediaOrigin(model)}
 func run()async{await CALL}
'''
T=r'''
}
@main struct Checks{
 @MainActor static func spin(_ p:()->Bool)async->Bool{for _ in 0..<10000{if p(){return true};await Task.yield()};return p()}
 @MainActor static func main()async{var c:[String:Bool]=[:]
 do{MediaFetch.reset();let m=AppModel();let x=Card(m);m.replace();await x.run();c["queued-old-card-cannot-admit-B-icon-or-image"]=MediaFetch.calls.isEmpty && x.siteIcon==nil && x.picture==nil}
 do{MediaFetch.reset();MediaFetch.pauseAt=1;let m=AppModel();let x=Card(m);let task=Task{await x.run()};let began=await spin{MediaFetch.waiter != nil};m.replace();MediaFetch.finish();await task.value;c["retired-icon-no-paint-or-second-B-image"]=began && MediaFetch.calls==["A:icon"] && x.siteIcon==nil && x.picture==nil}
 do{MediaFetch.reset();MediaFetch.pauseAt=2;let m=AppModel();let x=Card(m);let task=Task{await x.run()};let began=await spin{MediaFetch.waiter != nil};m.replace();MediaFetch.finish();await task.value;c["retired-image-no-late-paint"]=began && MediaFetch.calls==["A:icon","A:picture"] && x.siteIcon?.value=="A:icon" && x.picture==nil}
 do{MediaFetch.reset();let m=AppModel();let x=Card(m);await x.run();c["current-sequential-icon-and-image-control"]=MediaFetch.calls==["A:icon","A:picture"] && x.siteIcon?.value=="A:icon" && x.picture?.value=="A:picture"}
 do{MediaFetch.reset();MediaFetch.failures=["icon"];let m=AppModel();let x=Card(m);await x.run();c["current-icon-failure-still-loads-image"]=MediaFetch.calls==["A:icon","A:picture"] && x.siteIcon==nil && x.picture?.value=="A:picture"}
 do{MediaFetch.reset();let m=AppModel();let x=Card(m);x.host="branded.invalid";await x.run();c["brand-skips-icon-control"]=MediaFetch.calls==["A:picture"] && x.siteIcon==nil && x.picture != nil}
 do{MediaFetch.reset();let m=AppModel();let x=Card(m);x.preview.slot="";await x.run();c["empty-slot-no-private-fetch-control"]=MediaFetch.calls.isEmpty && x.siteIcon==nil && x.picture==nil}
 do{MediaFetch.reset();let m=AppModel();let x=Card(m);let task=Task{await x.run()};task.cancel();await task.value;c["cancelled-before-admission-no-fetch"]=MediaFetch.calls.isEmpty && x.siteIcon==nil && x.picture==nil}
 let failed=c.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["cases":c,"total":c.count,"passed":c.count-failed,"failed":failed],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);names=['UI/LinkPreviewCard.swift','UI/MediaViewer.swift','UI/ChatView.swift','Sync/WorkLifetime.swift'];paths=[tree/(P+x)for x in names];card,media,chat,life=[p.read_text()for p in paths];hashes={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};fixture=life+'\n'+block(chat,'enum ChatAccountContext:')+'\n'+block(media,'enum MediaOrigin {')+'\n'+F+method(card,'load')+T;fixture=fixture.replace('CALL','load(origin:origin)'if 'func load(origin:'in card else'load()');tick=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-link-origin-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};r=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=90);(out/'compile.log').write_text(r.stdout+r.stderr);record={'sources':hashes,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':r.returncode}
  if r.returncode==0:
   v=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);record.update(exit_code=v.returncode,results=json.loads(v.stdout)if v.stdout else{});(out/'tests.log').write_text(v.stdout+v.stderr)
  record['seconds']=time.monotonic()-tick;(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record));assert hashes=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};assert r.returncode==0 and record['results']['total']==8;exit(record['results']['failed']!=0)
if __name__=='__main__':main()
