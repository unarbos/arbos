#!/usr/bin/env python3
"""Execute selected production chat controllers with local collaborators on macOS."""
from pathlib import Path
import argparse,hashlib,importlib.util,json,shutil,sys,tempfile,time
HERE=Path(__file__).resolve().parent

def load(name):
 spec=importlib.util.spec_from_file_location(name,HERE/(name+'.py'));m=importlib.util.module_from_spec(spec);sys.modules[name]=m;spec.loader.exec_module(m);return m

def main():
 ap=argparse.ArgumentParser();ap.add_argument('--source-root',type=Path,default=HERE.parents[2]);ap.add_argument('--output',type=Path);a=ap.parse_args();root=a.source_root.resolve();holder=None
 if a.output:out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 else:holder=tempfile.TemporaryDirectory(prefix='arbos-chat-');out=Path(holder.name)
 prefix=Path('apps/ios/Arbos/Arbos');paths=[root/prefix/'UI'/n for n in ['ChatView.swift','Transcript.swift','QuestionCard.swift','FunPicker.swift','MediaViewer.swift','MessagePasteboard.swift']]+[root/prefix/'Store/AppModel.swift',root/prefix/'Sync/WorkLifetime.swift'];before={str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};context=out/'context';context.mkdir();shutil.copyfile(paths[-2],context/'AppModel.swift');shutil.copyfile(paths[-1],context/'WorkLifetime.swift');chat=paths[0].read_text();start=chat.index('enum ChatAccountContext:');end=chat.index('\n}\n',start)+2;(context/'ChatAccountContext.swift').write_text(chat[start:end]);sys.path.insert(0,str(HERE));start=time.monotonic();groups=[]
 for name,extra in [('check_cards',[]),('check_eq',[]),('check_session',[]),('check_actions',[]),('check_media',['image']),('check_media',['sticker']),('check_player',[]),('check_copy',[]),('check_fetch',[]),('check_thumb',[]),('check_retire',[]),('check_recorder_caller',[])]:
  m=load(name);m.P=out;m.ROOT=root;label=name+('-'+extra[0]if extra else'');sys.argv=[name,*([]if name in ['check_retire','check_recorder_caller']else['after']),*extra,label];m.main();record=json.loads((out/label/'GATES.json').read_text());assert record['compile_exit']==0 and record.get('exit_code')==0,record;expected={'check_cards':16,'check_eq':11,'check_session':16,'check_actions':20,'check_media-image':11,'check_media-sticker':11,'check_player':7,'check_copy':6,'check_fetch':8,'check_thumb':5,'check_retire':5,'check_recorder_caller':6};assert record['results']['total']==expected[label] and record['results']['passed']==expected[label] and record['results']['failed']==0,record;groups.append(record)
 after={str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};assert before==after,'source changed during selected-source checks';j={'passed':sum(g['results']['total']for g in groups),'failed':0,'groups':len(groups),'seconds':time.monotonic()-start,'source_before':before,'source_after':after,'source_unchanged':True,'scope':'Selected actual chat/card/equality/media/player/pasteboard controller bodies and real context/lifetime helpers; fake API, network, codec, cache and clipboard collaborators. No SwiftUI rendering, physical device, or runtime SDK proof. Clean environment and macOS deny-network sandbox.'};(out/'GATES.json').write_text(json.dumps(j,indent=2)+'\n');print(json.dumps(j,indent=2))
 if holder:holder.cleanup()
if __name__=='__main__':main()
