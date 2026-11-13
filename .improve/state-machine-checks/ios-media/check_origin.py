"""Actual existing receipt + media-origin predicates; no HTTP, files, SDK or views."""
from pathlib import Path
import argparse,hashlib,json,runpy,subprocess,tempfile,time
H=runpy.run_path(str(Path(__file__).with_name('check_copy.py')));block=H['block'];P='apps/ios/Arbos/Arbos/'
F=r'''
import Foundation
struct RelayAPI{}
struct AccountWork{let owner:WorkLifetime.Ticket;let api:RelayAPI}
struct MediaItem{var pictureURL:String?=nil}
@MainActor final class AppModel{
 var accountLifetime=WorkLifetime();var credentials:Bool?=true;var isFixture=false;var factories=0
 func accountWork()->AccountWork?{factories+=1;guard let owner=accountLifetime.ticket else{return nil};return AccountWork(owner:owner,api:RelayAPI())}
 func acceptsAccountWork(_ w:AccountWork?)->Bool{guard let w,!Task.isCancelled else{return false};return accountLifetime.accepts(w.owner)}
 func replace(){accountLifetime.close();accountLifetime.reopen()}
}
@main struct Checks{
 @MainActor static func main()async{var c:[String:Bool]=[:];let blob=MediaItem();let picture=MediaItem(pictureURL:"https://public.invalid/picture")
 do{let m=AppModel();let o=MediaOrigin(m);c["current-private-read-and-actions"]=o.allowsPrivate(m) && o.allowsReading(blob,m) && o.allowsReading(picture,m)}
 do{let m=AppModel();let owner=m.accountLifetime.ticket;let o=MediaOrigin(m);let before=m.factories;let again=MediaOrigin(m);c["same-owner-receipt-stable-no-API-factory"]=owner==m.accountLifetime.ticket && o.allowsPrivate(m) && again.allowsPrivate(m) && m.factories==before}
 do{let m=AppModel();let o=MediaOrigin(m);m.replace();c["old-account-ticket-cannot-read-or-act"] = !o.allowsReading(blob,m) && !o.allowsPrivate(m)}
 do{let m=AppModel();let ctx=ChatAccountContext(m);m.replace();let o=MediaOrigin(m,account:ctx);c["supplied-old-chat-never-falls-back-to-B"] = !o.allowsReading(blob,m) && !o.allowsPrivate(m)}
 do{let m=AppModel();let o=MediaOrigin(m,account:.unavailable);c["explicit-unavailable-chat-never-falls-back"] = !o.allowsReading(blob,m) && !o.allowsPrivate(m)}
 do{let m=AppModel();m.credentials=nil;let o=MediaOrigin(m);c["anonymous-public-read-zero-private-authority"]=o.allowsReading(picture,m) && !o.allowsReading(blob,m) && !o.allowsPrivate(m)}
 do{let m=AppModel();m.credentials=nil;let o=MediaOrigin(m);m.replace();c["anonymous-origin-retires-with-lifetime"] = !o.allowsReading(picture,m)}
 do{let m=AppModel();m.credentials=nil;let o=MediaOrigin(m);let other=AppModel();other.credentials=nil;c["public-origin-bound-to-model-instance"] = !o.allowsReading(picture,other)}
 do{let m=AppModel();m.credentials=nil;let o=MediaOrigin(m);m.credentials=true;c["public-origin-not-promoted-by-credential-install"] = !o.allowsReading(picture,m)}
 do{let m=AppModel();m.isFixture=true;let o=MediaOrigin(m);c["fixture-private-control"]=o.allowsPrivate(m) && o.allowsReading(blob,m);m.isFixture=false;c["fixture-mode-exit-revokes-receipt"] = !o.allowsPrivate(m)}
 do{let m=AppModel();let o=MediaOrigin(m);let t=Task{while !Task.isCancelled{await Task.yield()};return o.allowsReading(blob,m) || o.allowsPrivate(m)};t.cancel();c["cancelled-task-cannot-read-or-act"] = !(await t.value)}
 let failed=c.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["cases":c,"total":c.count,"passed":c.count-failed,"failed":failed],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);paths=[tree/(P+x)for x in ['UI/MediaViewer.swift','UI/ChatView.swift','Sync/WorkLifetime.swift']];media,chat,life=[p.read_text()for p in paths];hashes={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};fixture=life+'\n'+block(chat,'enum ChatAccountContext:')+'\n'+block(media,'enum MediaOrigin {')+'\n'+F;t=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-media-origin-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};r=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=90);(out/'compile.log').write_text(r.stdout+r.stderr);row={'sources':hashes,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':r.returncode}
  if r.returncode==0:
   v=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);row.update(exit_code=v.returncode,results=json.loads(v.stdout)if v.stdout else{});(out/'tests.log').write_text(v.stdout+v.stderr)
  row['seconds']=time.monotonic()-t;(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert hashes=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};assert r.returncode==0 and row['results']['total']==12;exit(row['results']['failed']!=0)
if __name__=='__main__':main()
