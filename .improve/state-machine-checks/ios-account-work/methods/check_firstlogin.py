"""Actual first-login retry/poll loops with synthetic suspended effects."""
from pathlib import Path
import argparse,hashlib,importlib.util,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
spec=importlib.util.spec_from_file_location('u',HERE/'check_uploads.py');u=importlib.util.module_from_spec(spec);spec.loader.exec_module(u)
F=r'''
import Foundation
struct Stage{static let computer=Stage()}
struct FirstLogin{var stage=Stage.computer;var stageSince=Date();var startedAt=Date();var error:String?;var detail:String?;var awakeSince:Date?}
@MainActor final class AppModel{
 var accountLifetime=WorkLifetime();var firstLogin:FirstLogin?=FirstLogin();var firstLoginTask:Task<Void,Never>?;var firstLoginIsDemo=false
 var requests:[String]=[];var current="A";var pauseMachine=false;var machineWaiter:CheckedContinuation<Void,Never>?;var pauseRegistry=false;var registryWaiter:CheckedContinuation<Void,Never>?;var ticks=0
 func replace(){accountLifetime.close();accountLifetime.reopen();firstLoginTask?.cancel();firstLoginTask=nil;current="B";firstLogin=FirstLogin();ticks=0}
 func firstLoginTick(){ticks+=1}
 func startMachineQuietly(expectedOwner:WorkLifetime.Ticket?=nil)async{if let expectedOwner,!accountLifetime.accepts(expectedOwner){return};requests.append(current+":machine");if pauseMachine{await withCheckedContinuation{machineWaiter=$0}}}
 func refreshRegistry(expectedOwner:WorkLifetime.Ticket?=nil)async{if let expectedOwner,!accountLifetime.accepts(expectedOwner){return};requests.append(current+":registry");if pauseRegistry{await withCheckedContinuation{registryWaiter=$0}}}
'''
T=r'''
}
@main struct Checks{
 @MainActor static func spin(_ p:()->Bool)async->Bool{for _ in 0..<10000{if p(){return true};await Task.yield()};return p()}
 @MainActor static func main()async{
 var cases:[String:Bool]=[:]
 do{let m=AppModel();m.firstLoginTask=Task{};m.retryFirstLogin();m.replace();for _ in 0..<100{await Task.yield()};cases["queued-first-login-retry-no-B-work"]=m.requests.isEmpty && m.ticks==0}
 do{let m=AppModel();m.firstLoginTask=Task{};m.pauseMachine=true;m.retryFirstLogin();let began=await spin{m.machineWaiter != nil};m.replace();m.machineWaiter?.resume();m.machineWaiter=nil;for _ in 0..<100{await Task.yield()};cases["late-retry-cannot-refresh-or-tick-B"]=began && m.requests==["A:machine"] && m.ticks==0}
 do{let m=AppModel();m.firstLoginTask=Task{};m.retryFirstLogin();let done=await spin{m.ticks==1};cases["current-retry-order-control"]=done && m.requests==["A:machine","A:registry"]}
 do{let m=AppModel();m.pauseRegistry=true;m.startFirstLoginPolling();let began=await spin{m.registryWaiter != nil};m.replace();m.registryWaiter?.resume();m.registryWaiter=nil;for _ in 0..<100{await Task.yield()};cases["late-poll-cannot-tick-B"]=began && m.ticks==0 && m.requests==["A:registry"]}
 do{let m=AppModel();m.startFirstLoginPolling();let done=await spin{m.ticks==1};m.firstLoginTask?.cancel();cases["current-poll-control"]=done && m.requests==["A:registry"]}
 let failed=cases.values.filter{!$0}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":cases.count-failed,"failed":failed,"total":cases.count,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--tree',type=Path,required=True);ap.add_argument('--out',type=Path,required=True);a=ap.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(exist_ok=False)
 paths=[tree/(P+q)for q in ['Store/FirstLogin.swift','Sync/WorkLifetime.swift']];source,helper=[p.read_text()for p in paths];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};fixture=helper+F+u.method(source,'retryFirstLogin')+'\n'+u.method(source,'startFirstLoginPolling').replace('private func startFirstLoginPolling','func startFirstLoginPolling')+T;start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-ios-first-login-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'sources':sources,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  record['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
