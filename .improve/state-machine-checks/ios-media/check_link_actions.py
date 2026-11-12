"""Actual rendered link-card tap closure; synthetic URL-opening effect only."""
from pathlib import Path
import argparse,hashlib,json,runpy,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;H=runpy.run_path(str(HERE/'check_link.py'));block=H['block'];P=H['P']
T=r'''
}
@main struct Checks{
 @MainActor static func main()async{var c:[String:Bool]=[:]
 for mode in ["current","retired","fixture","cancelled","anonymous","missing-url"]{let m=AppModel();if mode=="fixture"{m.isFixture=true};if mode=="anonymous"{m.credentials=nil};let card=Card(m);if mode=="retired"{m.replace()};if mode=="missing-url"{card.preview.openURL=nil};if mode=="cancelled"{let task=Task{card.invoke()};task.cancel();await task.value}else{card.invoke()};c["link-tap-"+mode] = ["current","fixture"].contains(mode) ? card.opens==["https://synthetic.invalid/link"]:card.opens.isEmpty}
 let failed=c.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["cases":c,"total":c.count,"passed":c.count-failed,"failed":failed],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);names=['UI/LinkPreviewCard.swift','UI/MediaViewer.swift','UI/ChatView.swift','Sync/WorkLifetime.swift'];paths=[tree/(P+x)for x in names];card,media,chat,life=[p.read_text()for p in paths];hashes={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};start=card.index('        Button {')+len('        Button {');end=card.index('\n        } label: {',start);body=card[start:end];f=H['F'].replace('var slot="same-slot"','var openURL:URL?=URL(string:"https://synthetic.invalid/link");var slot="same-slot"').replace(' func run()async{await CALL}',' var opens:[String]=[];func openURL(_ url:URL){opens.append(url.absoluteString)}');fixture=life+'\n'+block(chat,'enum ChatAccountContext:')+'\n'+block(media,'enum MediaOrigin {')+'\n'+f+'func invoke(){'+body+'\n}'+T;tick=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-link-tap-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};r=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=90);(out/'compile.log').write_text(r.stdout+r.stderr);record={'sources':hashes,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'callback_sha256':hashlib.sha256(body.encode()).hexdigest(),'compile_exit':r.returncode}
  if r.returncode==0:
   v=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);record.update(exit_code=v.returncode,results=json.loads(v.stdout)if v.stdout else{});(out/'tests.log').write_text(v.stdout+v.stderr)
  record['seconds']=time.monotonic()-tick;(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record));assert hashes=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};assert r.returncode==0 and record['results']['total']==6;exit(record['results']['failed']!=0)
if __name__=='__main__':main()
