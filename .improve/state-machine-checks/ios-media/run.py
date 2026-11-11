"""Run 80 deterministic selected-source media checks; fresh output, no network or SDK effects."""
from pathlib import Path
import argparse,hashlib,json,subprocess,sys,time
HERE=Path(__file__).resolve().parent
CHECKS=[('check_copy.py',8),('check_media.py',20),('check_bindings.py',8),('check_origin.py',12),('check_link.py',8),('check_link_bindings.py',18),('check_link_actions.py',6)]
def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,default=HERE.parents[2]);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);names=['UI/MediaViewer.swift','UI/MessagePasteboard.swift','UI/LibraryList.swift','UI/ChatView.swift','UI/Transcript.swift','UI/FunPicker.swift','UI/LinkPreviewCard.swift','Sync/WorkLifetime.swift','Store/AppModel.swift'];prefix='apps/ios/Arbos/Arbos/';before={prefix+n:hashlib.sha256((tree/(prefix+n)).read_bytes()).hexdigest()for n in names};rows=[];tick=time.monotonic()
 for file,count in CHECKS:
  target=out/Path(file).stem;r=subprocess.run([sys.executable,'-I','-B',str(HERE/file),'--tree',str(tree),'--out',str(target)],capture_output=True,text=True,timeout=150);(out/(Path(file).stem+'.log')).write_text(r.stdout+r.stderr);gate=json.loads((target/'GATES.json').read_text());assert r.returncode==0 and gate['compile_exit']==0 and gate['exit_code']==0 and gate['results']['total']==count and gate['results']['passed']==count and gate['results']['failed']==0,gate;rows.append({'file':file,'expected':count,'seconds':gate['seconds'],'fixture_sha256':gate['fixture_sha256']})
 after={prefix+n:hashlib.sha256((tree/(prefix+n)).read_bytes()).hexdigest()for n in names};assert before==after;data={'passed':sum(x['expected']for x in rows),'failed':0,'groups':rows,'seconds':time.monotonic()-tick,'sources':before,'source_unchanged':True,'scope':'Actual selected production methods, constructors and existing receipt predicates with inert collaborators. No real HTTP, Photos, media playback, clipboard, app runtime or device effects.'};(out/'GATES.json').write_text(json.dumps(data,indent=2)+'\n');print(json.dumps(data,indent=2))
if __name__=='__main__':main()
