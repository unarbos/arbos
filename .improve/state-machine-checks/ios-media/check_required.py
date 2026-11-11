"""Compiler contract for the four actual production signatures; no method bodies execute."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
P='apps/ios/Arbos/Arbos/'
CALLS={
 'bytes':'_ = await MediaFetch.bytes(for: MediaItem(), model: self, isCurrent: { true })',
 'copyMessage':'await copyMessage(Message(), slot: "s", isCurrent: { true })',
 'copyAttachment':'_ = await copyAttachment(Attachment(), slot: "s", isCurrent: { true })',
 'copy':'_ = await copy(text: "", attachments: [], slot: "s", isCurrent: { true })',
}
def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);p.add_argument('--expect-required',action='store_true');a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);paths=[tree/(P+x)for x in ['UI/MediaViewer.swift','UI/MessagePasteboard.swift']];media,board=[p.read_text()for p in paths];hashes={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};headers={}
 for name,source in [('bytes',media),('copyMessage',board),('copyAttachment',board),('copy',board)]:
  found=re.findall(r'^    (?:private |static )?func '+name+(r'\(for item:[^\n]+$'if name=='bytes'else r'\([^\n]+$'),source,re.M);assert len(found)==1;(header,sep,end)=found[0].rpartition('{');assert sep and not end.strip();headers[name]=header+'{ fatalError() }'
 rows=[];tick=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-required-origin-')as raw:
  tmp=Path(raw);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw}
  for case in ['explicit',*CALLS]:
   calls=[value.replace(', isCurrent: { true }','')if case==name else value for name,value in CALLS.items()];fixture='import Foundation\nstruct Message{};struct Attachment{};struct MediaItem{}\n@MainActor enum MediaFetch{\n'+headers['bytes']+'\n}\n@MainActor final class AppModel{\n'+'\n'.join(headers[n]for n in ['copyMessage','copyAttachment','copy'])+'\nfunc probe() async{\n'+'\n'.join(calls)+'\n}\n}\n';file=tmp/'fixture.swift';file.write_text(fixture);r=subprocess.run(['swiftc','-swift-version','5','-typecheck',str(file)],env=env,capture_output=True,text=True,timeout=30);expected_failure=a.expect_required and case!='explicit';assert (r.returncode!=0)==expected_failure,(case,r.stderr);assert not expected_failure or "missing argument for parameter 'isCurrent'"in r.stderr;(out/(case+'.log')).write_text(r.stdout+r.stderr);rows.append({'case':case,'compile_exit':r.returncode,'expected_missing_argument':expected_failure,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()})
 assert hashes=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};record={'sources':hashes,'passed':len(rows),'failed':0,'expect_required':a.expect_required,'cases':rows,'seconds':time.monotonic()-tick,'scope':'Exact production signature headers with inert fatalError bodies: compiler contract only, no runtime policy implementation or effects.'};(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
