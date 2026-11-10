from pathlib import Path
import argparse,hashlib,json,subprocess,tempfile,time
from check_cards import P,ROOT,PREFIX,closure
F=r'''
import Foundation
final class RelayAPI{}
struct AccountWork{let owner:WorkLifetime.Ticket;let api:RelayAPI}
CONTEXT
@MainActor final class AppModel{var accountLifetime=WorkLifetime();var isFixture=false;var authenticated=true;func accountWork()->AccountWork?{guard authenticated,let owner=accountLifetime.ticket else{return nil};return AccountWork(owner:owner,api:RelayAPI())};ACCEPTS
func replace(){accountLifetime.close();accountLifetime.reopen()}}
@MainActor final class Player{var retired=false;func retire(){retired=true}}
@MainActor final class Bubble{let model:AppModel;let account:ChatAccountContext;let player=Player();init(_ m:AppModel){model=m;account=ChatAccountContext(m)};func disappear(){DISAPPEAR}}
@main struct Checks{
 @MainActor static func main()async{var cases:[String:Bool]=[:]
 do{let m=AppModel();let b=Bubble(m);b.disappear();cases["normal-current-scroll-keeps-existing-playback-policy"] = !b.player.retired}
 do{let m=AppModel();let b=Bubble(m);let t=Task{b.disappear()};t.cancel();await t.value;cases["cancelled-current-scroll-is-not-account-retirement"] = !b.player.retired}
 do{let m=AppModel();let b=Bubble(m);m.replace();b.disappear();cases["old-account-disappearance-retires-owned-player"] = b.player.retired}
 do{let m=AppModel();m.isFixture=true;m.authenticated=false;let b=Bubble(m);m.isFixture=false;b.disappear();cases["fixture-to-authenticated-disappearance-retires-owned-player"] = b.player.retired}
 do{let m=AppModel();m.authenticated=false;let b=Bubble(m);b.disappear();cases["unavailable-player-is-retired"] = b.player.retired}
 let failed=cases.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["total":cases.count,"passed":cases.count-failed,"failed":failed,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('label');a=ap.parse_args();src=ROOT/PREFIX/'Transcript.swift';s=src.read_text();part=s[s.index('struct VoiceMemoBubble:'):s.index('@MainActor\nfinal class MemoPlayer:')];body=closure(part,'.onDisappear {');ctx=(P/'context/ChatAccountContext.swift').read_text();model=(P/'context/AppModel.swift').read_text();x=model.index('    func acceptsAccountWork(');accept=model[x:model.index('\n    }',x)+6];fixture=(P/'context/WorkLifetime.swift').read_text()+F.replace('CONTEXT',ctx).replace('ACCEPTS',accept).replace('DISAPPEAR',body);out=P/a.label;out.mkdir(exist_ok=False);(out/'fixture.swift').write_text(fixture);start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='chat-retire-')as raw:
  tmp=Path(raw);f=tmp/'main.swift';f.write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(f),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);j={'source_sha256':hashlib.sha256(s.encode()).hexdigest(),'closure_sha256':hashlib.sha256(body.encode()).hexdigest(),'compile_exit':c.returncode}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);j.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  assert s==src.read_text();j['seconds']=time.monotonic()-start;(out/'GATES.json').write_text(json.dumps(j,indent=2)+'\n');print(json.dumps(j))
if __name__=='__main__':main()
