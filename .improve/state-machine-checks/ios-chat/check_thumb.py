from pathlib import Path
import argparse,hashlib,json,subprocess,tempfile,time
from check_cards import P,ROOT,PREFIX,method
F=r'''
import Foundation
final class RelayAPI{}
struct AccountWork{let owner:WorkLifetime.Ticket;let api:RelayAPI}
CONTEXT
struct UIImage:Equatable{let data:Data;init?(data:Data){self.data=data}}
@MainActor final class URLSession{static let shared=URLSession();static var pending:CheckedContinuation<Void,Never>?;static var started=false;static var pause=false;func data(for req:URLRequest)async throws->(Data,URLResponse){Self.started=true;if Self.pause{await withCheckedContinuation{Self.pending=$0}};return(Data([1]),HTTPURLResponse(url:req.url!,statusCode:200,httpVersion:nil,headerFields:nil)!)} }
@MainActor final class AppModel{var accountLifetime=WorkLifetime();var isFixture=false;var authenticated=true;var reads=0;func accountWork()->AccountWork?{guard authenticated,let owner=accountLifetime.ticket else{return nil};return AccountWork(owner:owner,api:RelayAPI())};ACCEPTS
 func replace(){accountLifetime.close();accountLifetime.reopen()};func blobURL(projectId:String,key:String)->URL?{reads+=1;return URL(string:"https://fixture.invalid/blob")};var authHeaders:[String:String]{reads+=1;return["Authorization":"synthetic-A"]}}
@MainActor final class Thumb{let model:AppModel;let account:ChatAccountContext;let projectId="A-slot";let blobRef="ref";var image:UIImage?;init(_ m:AppModel){model=m;account=ChatAccountContext(m)}
 LOAD
 func run()async{await load()}}
@main struct Checks{
 @MainActor static func reset(){URLSession.started=false;URLSession.pause=false}
 @MainActor static func main()async{var cases:[String:Bool]=[:]
 do{reset();let m=AppModel();let t=Thumb(m);m.replace();await t.run();cases["old-thumb-refuses-B-authority"] = m.reads==0 && !URLSession.started && t.image==nil}
 do{reset();let m=AppModel();m.authenticated=false;let t=Thumb(m);await t.run();cases["unavailable-thumb-refuses-fetch"] = m.reads==0 && !URLSession.started}
 do{reset();let m=AppModel();let t=Thumb(m);let job=Task{await t.run()};job.cancel();await job.value;cases["cancelled-thumb-refuses-fetch"] = m.reads==0 && !URLSession.started}
 do{reset();let m=AppModel();let t=Thumb(m);URLSession.pause=true;let job=Task{await t.run()};while !URLSession.started{await Task.yield()};m.replace();URLSession.pending?.resume();URLSession.pending=nil;await job.value;cases["late-thumb-does-not-publish"] = t.image==nil}
 do{reset();let m=AppModel();let t=Thumb(m);await t.run();cases["current-thumb-load-control"] = t.image?.data==Data([1]) && m.reads==2}
 let failed=cases.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["total":cases.count,"passed":cases.count-failed,"failed":failed,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('arm',choices=['before','after']);ap.add_argument('label');a=ap.parse_args();src=P/'base/Transcript.swift'if a.arm=='before'else ROOT/PREFIX/'Transcript.swift';s=src.read_text();part=method(s[s.index('struct ReplyThumb:'):],'load');ctx=(P/'context/ChatAccountContext.swift').read_text();model=(P/'context/AppModel.swift').read_text();x=model.index('    func acceptsAccountWork(');accept=model[x:model.index('\n    }',x)+6];fixture=(P/'context/WorkLifetime.swift').read_text()+F.replace('CONTEXT',ctx).replace('ACCEPTS',accept).replace('LOAD',part);out=P/a.label;out.mkdir(exist_ok=False);(out/'fixture.swift').write_text(fixture);start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='chat-thumb-')as raw:
  tmp=Path(raw);f=tmp/'main.swift';f.write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(f),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);j={'source_sha256':hashlib.sha256(s.encode()).hexdigest(),'method_sha256':hashlib.sha256(part.encode()).hexdigest(),'compile_exit':c.returncode}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);j.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  assert s==src.read_text();j['seconds']=time.monotonic()-start;(out/'GATES.json').write_text(json.dumps(j,indent=2)+'\n');print(json.dumps(j))
if __name__=='__main__':main()
