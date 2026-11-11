#!/usr/bin/env python3
"""Selected actual Settings/confirmation controllers with local collaborators, no SwiftUI runtime."""
from pathlib import Path
import argparse,hashlib,importlib.util,json,shutil,sys,tempfile,time
HERE=Path(__file__).resolve().parent

def load(name):
 spec=importlib.util.spec_from_file_location(name,HERE/(name+'.py'));module=importlib.util.module_from_spec(spec);sys.modules[name]=module;spec.loader.exec_module(module);return module

def main():
 ap=argparse.ArgumentParser();ap.add_argument('--source-root',type=Path,default=HERE.parents[2]);ap.add_argument('--output',type=Path);a=ap.parse_args();root=a.source_root.resolve();holder=None
 if a.output:out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 else:holder=tempfile.TemporaryDirectory(prefix='arbos-settings-');out=Path(holder.name)
 prefix=Path('apps/ios/Arbos/Arbos');view=root/prefix/'UI/SettingsView.swift';context=out/'context';context.mkdir();paths=[view,root/prefix/'Store/AppModel.swift',root/prefix/'Sync/WorkLifetime.swift',root/prefix/'UI/Confirm.swift'];before={str(x.relative_to(root)):hashlib.sha256(x.read_bytes()).hexdigest()for x in paths};shutil.copyfile(paths[1],context/'AppModel.swift');shutil.copyfile(paths[2],context/'WorkLifetime.swift')
 start=time.monotonic();groups=[]
 for name in ['check_profile','check_open','check_token','check_origin','check_disconnect','check_confirm']:
  m=load(name);m.P=out;m.VIEW=view;m.SOURCE=root/prefix/'UI/Confirm.swift';sys.argv=[name,*([]if name=='check_origin'else['after']),name];m.main();record=json.loads((out/name/'GATES.json').read_text());assert record['compile_exit']==0 and record['exit_code']==0,record;groups.append(record)
 after={str(x.relative_to(root)):hashlib.sha256(x.read_bytes()).hexdigest()for x in paths};assert before==after,'source changed during selected-source checks';total=sum(g['results']['total']for g in groups);record={'passed':total,'failed':0,'groups':len(groups),'seconds':time.monotonic()-start,'source_before':before,'source_after':after,'source_unchanged':True,'scope':'Actual selected save/open/token/anchored-dialog bodies plus real SettingsOrigin/WorkLifetime/account acceptance; fake profile/connector collaborators, no UI rendering or SDK proof. All execution uses clean environment and macOS sandbox deny network*.'};(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record,indent=2))
 if holder:holder.cleanup()
if __name__=='__main__':main()
