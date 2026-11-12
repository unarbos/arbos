"""Actual opaque ticket identity plus the source SwiftUI consumer; no render claim."""
from pathlib import Path
import argparse,hashlib,json,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
T=r'''
@main struct Checks{
 static func main(){
 var a=WorkLifetime();let first=a.ticket!;let unchanged=a.ticket!;let other=WorkLifetime().ticket!
 var cases:[String:Bool]=[:]
 cases["ordinary-refresh-identity-stable"]=first==unchanged
 a.close();let retired=a.ticket;a.reopen();cases["retirement-and-foreign-owner-identity-distinct"]=retired==nil && first != a.ticket && first != other
 cases["ticket-valid-SwiftUI-hashable-identity"]=(first as Any) is AnyHashable
 cases["production-shell-bound-to-account-owner"]=CONSUMER
 let failed=cases.values.filter{!$0}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":cases.count-failed,"failed":failed,"total":cases.count,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--tree',type=Path,required=True);ap.add_argument('--out',type=Path,required=True);a=ap.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(exist_ok=False)
 paths=[tree/(P+q)for q in ['Sync/WorkLifetime.swift','App/ArbosApp.swift']];helper,app=[p.read_text()for p in paths];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};consumer='MainShellView()\n                    .id(model.accountLifetime.ticket)'in app;fixture=helper+T.replace('CONSUMER',str(consumer).lower());start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-ios-ui-identity-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'sources':sources,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  record['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
