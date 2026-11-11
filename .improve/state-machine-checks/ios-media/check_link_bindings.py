"""Exact link/markdown constructor receipt propagation; no SwiftUI or API effects."""
from pathlib import Path
import argparse,hashlib,json,runpy,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;HELPER=HERE/'check_bindings.py'
if not HELPER.exists():HELPER=HERE.parent/'check_bindings.py'
H=runpy.run_path(str(HELPER));expr,block=H['expr'],H['block'];P='apps/ios/Arbos/Arbos/'
F=r'''
import Foundation
struct RelayAPI{}
struct AccountWork{let owner:WorkLifetime.Ticket;let api:RelayAPI}
@MainActor final class AppModel{var accountLifetime=WorkLifetime();var credentials:Bool?=true;var isFixture=false;func replace(){accountLifetime.close();accountLifetime.reopen()};func accountWork()->AccountWork?{guard let owner=accountLifetime.ticket else{return nil};return AccountWork(owner:owner,api:RelayAPI())};func acceptsAccountWork(_ w:AccountWork?)->Bool{guard let w,!Task.isCancelled else{return false};return accountLifetime.accepts(w.owner)}}
struct MediaItem{var pictureURL:String?=nil}
struct Color{}
struct MentionTable{}
struct Block{}
struct Preview{}
struct Row{var messageId="message"}
struct Theme{var userText=Color();var userCodeChip=Color();var text=Color();var codeChip=Color()}
struct LinkPreviewCard{var preview:Preview;var chatId:String;var messageId:String;var own:Bool;var origin:MediaOrigin?=nil}
struct BlockView{var block:Block;var live:Bool;var mentions:MentionTable;var chatId="";var messageId="";var ink:Color?=nil;var chip:Color?=nil;var mediaOrigin:MediaOrigin?=nil}
struct TypedBody{var text:String;var ink:Color;var chip:Color;var mentions:MentionTable;var mediaOrigin:MediaOrigin?=nil}
@MainActor final class Host{
 let model:AppModel;let account:ChatAccountContext;let mediaOrigin:MediaOrigin?;let preview=Preview();let chatId="chat";let row=Row();let messageId="message";let theme=Theme();let mentions=MentionTable();let block=Block();let b=Block();let blocks=[Block()];let live=false;let i=0;let ink=Color();let chip=Color();let muted=Color();let text=""
 init(_ model:AppModel){self.model=model;account=ChatAccountContext(model);mediaOrigin=MediaOrigin(model,account:account)}
'''
T=r'''
}
@main struct Checks{
 @MainActor static func main(){var c:[String:Bool]=[:]
 for(n,make)in MAKERS{let m=AppModel();let h=Host(m);let o=make(h);c[n+"-current"]=o?.allowsPrivate(m) ?? false;m.replace();c[n+"-retired"]=o.map{!$0.allowsPrivate(m)} ?? false}
 let failed=c.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["cases":c,"total":c.count,"passed":c.count-failed,"failed":failed],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);names=['UI/Transcript.swift','UI/MediaViewer.swift','UI/ChatView.swift','Sync/WorkLifetime.swift'];paths=[tree/(P+x)for x in names];trans,media,chat,life=[p.read_text()for p in paths];hashes={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};funcs=[];names=[]
 for needle,count,member in [('LinkPreviewCard(',3,'origin'),('BlockView(',4,'mediaOrigin'),('TypedBody(',2,'mediaOrigin')]:
  assert trans.count(needle)==count
  for i in range(count):
   n=needle[:-1]+str(i);names.append(n);funcs.append('func '+n+'()->MediaOrigin?{'+expr(trans,needle,i)+'.'+member+'}')
 makers='['+','.join('("'+n+'",{(h:Host)in h.'+n+'()})'for n in names)+']';fixture=life+'\n'+block(chat,'enum ChatAccountContext:')+'\n'+block(media,'enum MediaOrigin {')+'\n'+F+'\n'.join(funcs)+T.replace('MAKERS',makers);tick=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-link-bindings-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};r=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=90);(out/'compile.log').write_text(r.stdout+r.stderr);record={'sources':hashes,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':r.returncode}
  if r.returncode==0:
   v=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);record.update(exit_code=v.returncode,results=json.loads(v.stdout)if v.stdout else{});(out/'tests.log').write_text(v.stdout+v.stderr)
  record['seconds']=time.monotonic()-tick;(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record));assert hashes=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};assert r.returncode==0 and record['results']['total']==18;exit(record['results']['failed']!=0)
if __name__=='__main__':main()
