"""Three actual-function negative controls; disposable sources and deny-network Swift checks."""
from pathlib import Path
import argparse,difflib,hashlib,json,shutil,subprocess,sys,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
MUTANTS=[
 ('copy-predicate-discarded','UI/MessagePasteboard.swift','return await copy(text: "", attachments: [attachment], slot: slot, isCurrent: isCurrent)','return await copy(text: "", attachments: [attachment], slot: slot, isCurrent: { true })','check_copy.py',8),
 ('photos-permission-owner-check-removed','UI/MediaViewer.swift','let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)\n            guard origin.allowsPrivate(model) else { return }','let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)','check_media.py',20),
 ('link-icon-publication-check-removed','UI/LinkPreviewCard.swift','let bytes = await MediaFetch.bytes(for: item, model: model, isCurrent: { origin.allowsPrivate(model) })\n            guard origin.allowsPrivate(model) else { return }','let bytes = await MediaFetch.bytes(for: item, model: model, isCurrent: { origin.allowsPrivate(model) })','check_link.py',8),
]
def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);names=['UI/MediaViewer.swift','UI/MessagePasteboard.swift','UI/LibraryList.swift','UI/ChatView.swift','UI/LinkPreviewCard.swift','Sync/WorkLifetime.swift','Store/AppModel.swift'];hashes={P+n:hashlib.sha256((tree/(P+n)).read_bytes()).hexdigest()for n in names};rows=[];tick=time.monotonic()
 for name,file,old,new,driver,total in MUTANTS:
  with tempfile.TemporaryDirectory(prefix='arbos-media-mutant-')as raw:
   tmp=Path(raw)
   for n in names:
    dest=tmp/(P+n);dest.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(tree/(P+n),dest)
   target=tmp/(P+file);before=target.read_text();assert before.count(old)==1;after=before.replace(old,new,1);target.write_text(after);(out/(name+'.diff')).write_text(''.join(difflib.unified_diff(before.splitlines(True),after.splitlines(True),fromfile='a/'+P+file,tofile='b/'+P+file)))
   proofs=[]
   for suffix in ['first','replay']:
    folder=out/(name+'-'+suffix);r=subprocess.run([sys.executable,'-I','-B',str(HERE/driver),'--tree',str(tmp),'--out',str(folder)],capture_output=True,text=True,timeout=140);row=json.loads((folder/'GATES.json').read_text());assert row['compile_exit']==0 and row['results']['total']==total and row['results']['failed']>0 and r.returncode!=0;proofs.append(row['results'])
   assert proofs[0]==proofs[1];rows.append({'name':name,'mutation_count':1,'detected':True,'same_deterministic_replay':True,'results':proofs[0]})
 assert hashes=={P+n:hashlib.sha256((tree/(P+n)).read_bytes()).hexdigest()for n in names};data={'sources':hashes,'mutants':rows,'detected':len(rows),'total':len(MUTANTS),'seconds':time.monotonic()-tick};(out/'RESULTS.json').write_text(json.dumps(data,indent=2)+'\n');print(json.dumps(data))
if __name__=='__main__':main()
