from pathlib import Path
import argparse,json,hashlib,subprocess,tempfile,time
F=r'''
@main struct Checks {
 @MainActor static func main() {
  func matchesSize(_ size:CGSize?,_ expected:CGSize)->Bool{size?.width==expected.width && size?.height==expected.height}
  var a=WorkLifetime();let b=WorkLifetime();let owner=a.ticket!;let other=b.ticket!
  let imgA=UIImage(CGSize(width:10,height:20));let imgB=UIImage(CGSize(width:30,height:40));let imgC=UIImage(CGSize(width:50,height:60))
  let da=ChatImageCache.Decoded(image:imgA,stillFrame:nil,animated:false,naturalSize:imgA.size)
  let db=ChatImageCache.Decoded(image:imgB,stillFrame:nil,animated:false,naturalSize:imgB.size)
  let dc=ChatImageCache.Decoded(image:imgC,stillFrame:nil,animated:false,naturalSize:imgC.size)
  var cases:[String:Bool]=[:]
  ChatImageCache.store(da,for:key(owner,"project-a","same-ref"))
  cases["current-decoded-hit"]=ChatImageCache.decoded(for:key(owner,"project-a","same-ref"))?.image === imgA
  cases["current-size-hit"]=matchesSize(ChatImageCache.naturalSize(for:key(owner,"project-a","same-ref")),imgA.size)
  cases["foreign-account-decoded-miss"]=ChatImageCache.decoded(for:key(other,"project-a","same-ref"))==nil
  cases["foreign-account-size-miss"]=ChatImageCache.naturalSize(for:key(other,"project-a","same-ref"))==nil
  cases["foreign-slot-decoded-miss"]=ChatImageCache.decoded(for:key(owner,"people:chat-b","same-ref"))==nil
  cases["foreign-slot-size-miss"]=ChatImageCache.naturalSize(for:key(owner,"people:chat-b","same-ref"))==nil
  ChatImageCache.store(db,for:key(other,"project-a","same-ref"))
  cases["current-B-progress"]=ChatImageCache.decoded(for:key(other,"project-a","same-ref"))?.image === imgB
  cases["B-store-preserves-A"]=ChatImageCache.decoded(for:key(owner,"project-a","same-ref"))?.image === imgA
  ChatImageCache.store(dc,for:key(owner,"people:chat-b","same-ref"))
  cases["current-other-slot-progress"]=ChatImageCache.decoded(for:key(owner,"people:chat-b","same-ref"))?.image === imgC
  cases["slot-store-preserves-B"]=ChatImageCache.decoded(for:key(other,"project-a","same-ref"))?.image === imgB
  cases["slot-store-preserves-A"]=ChatImageCache.decoded(for:key(owner,"project-a","same-ref"))?.image === imgA
  ChatImageCache.evictBitmapsForFixture()
  cases["size-survives-bitmap-eviction"]=matchesSize(ChatImageCache.naturalSize(for:key(owner,"project-a","same-ref")),imgA.size)
  cases["bitmap-eviction-control"]=ChatImageCache.decoded(for:key(owner,"project-a","same-ref"))==nil
  cases["same-owner-refresh-key-control"]=key(owner,"project-a","same-ref")==key(a.ticket!,"project-a","same-ref")
  a.close();a.reopen()
  cases["readoption-does-not-reuse-private-size"]=ChatImageCache.naturalSize(for:key(a.ticket!,"project-a","same-ref"))==nil
  cases["new-generation-does-not-reuse-other-slot-size"]=ChatImageCache.naturalSize(for:key(a.ticket!,"people:chat-b","same-ref"))==nil
  let failed=cases.values.filter{!$0}.count
  print(String(data:try!JSONSerialization.data(withJSONObject:["cases":cases,"total":cases.count,"passed":cases.count-failed,"failed":failed],options:[.sortedKeys]),encoding:.utf8)!)
  exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--source-root',type=Path,required=True);ap.add_argument('--out',type=Path,required=True);a=ap.parse_args();r=a.source_root.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);rel='apps/ios/Arbos/Arbos/UI/ChatImageCache.swift';src=(r/rel).read_text();start=time.monotonic();body=src[src.index('enum ChatImageCache {'):src.index('    /// Decode the bytes away')]+"}\n";lifetime=(r/'apps/ios/Arbos/Arbos/Sync/WorkLifetime.swift').read_text();typed='struct Key:'in body
 adapter='func key(_ owner:WorkLifetime.Ticket,_ slot:String,_ ref:String)->ChatImageCache.Key{.init(owner:owner,slot:slot,ref:ref)}\n'if typed else'func key(_ owner:WorkLifetime.Ticket,_ slot:String,_ ref:String)->String{ref}\n'
 fixture=lifetime+'\nfinal class UIImage:@unchecked Sendable{let size:CGSize;let scale:CGFloat=1;var images:[UIImage]?{nil};init(_ size:CGSize){self.size=size}}\n'+body+'\nextension ChatImageCache{@MainActor static func evictBitmapsForFixture(){images.removeAllObjects()}}\n'+adapter+F;(out/'fixture.swift').write_text(fixture)
 with tempfile.TemporaryDirectory(prefix='arbos-image-cache-')as raw:
  tmp=Path(raw);f=tmp/'main.swift';f.write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['/usr/bin/xcrun','swiftc','-parse-as-library','-swift-version','5',str(f),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=90);(out/'compile.log').write_text(c.stdout+c.stderr);record={'source_sha256':hashlib.sha256(src.encode()).hexdigest(),'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode,'scope':'Actual Foundation NSCache and size methods +actual WorkLifetime; UIImage size/identity collaborator; excludes decoder/UIKit/UI runtime, no model/network/userdata'}
  if c.returncode==0:
   x=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=15);record.update(exit_code=x.returncode,results=json.loads(x.stdout));(out/'test.log').write_text(x.stdout+x.stderr)
 record['seconds']=time.monotonic()-start;record['source_unchanged']=src==(r/rel).read_text();(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record));assert c.returncode==0 and record['source_unchanged'];raise SystemExit(record['exit_code'])
if __name__=='__main__':main()
