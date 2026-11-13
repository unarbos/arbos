from pathlib import Path
import argparse,json,hashlib,subprocess,tempfile,time
FAKES=r'''
import Combine
@MainActor enum Probe {static var finished=0;static var starts=0;static var stops=0;static var fail=false;static var recordOK=true;static var urls:[URL]=[];static var settings:[String:Any]=[:];static var intervals:[Double]=[]}
let AVFormatIDKey="format",AVSampleRateKey="sample-rate",AVNumberOfChannelsKey="channels",AVEncoderAudioQualityKey="quality"
let kAudioFormatMPEG4AAC:UInt32=1633772320
enum AVAudioQuality:Int{case medium=64}
@MainActor final class AVAudioSession {
 enum Category{case playAndRecord};enum Mode{case `default`};struct Options:OptionSet{let rawValue:Int;static let defaultToSpeaker=Options(rawValue:1)}
 static let shared=AVAudioSession();var callbacks:[(Bool)->Void]=[]
 static func sharedInstance()->AVAudioSession{shared}
 func requestRecordPermission(_ reply:@escaping(Bool)->Void){callbacks.append(reply)}
 func setCategory(_ category:Category,mode:Mode,options:Options)throws{}
 func setActive(_ value:Bool)throws{}
}
@MainActor final class AVAudioRecorder {
 var isMeteringEnabled=false
 init(url:URL,settings:[String:Any])throws{Probe.urls.append(url);Probe.settings=settings;try Data("synthetic-memo".utf8).write(to:url);if Probe.fail{throw NSError(domain:"fixture",code:1)}}
 @discardableResult func record()->Bool{if Probe.recordOK{Probe.starts+=1};return Probe.recordOK};func stop(){Probe.stops+=1};func updateMeters(){};func averagePower(forChannel:Int)->Float{-25}
}
@MainActor final class FixtureTimer {
 func invalidate(){}
 static func scheduledTimer(withTimeInterval interval:Double,repeats:Bool,block:@escaping(FixtureTimer)->Void)->FixtureTimer{Probe.intervals.append(interval);return FixtureTimer()}
}
typealias Timer=FixtureTimer
extension MemoRecorder{@MainActor func setElapsedForFixture(_ s:Double){seconds=s};@MainActor func tickForFixture(){tick()}}
@main struct Checks {
 @MainActor static func settle(_ expected:Int)async{let end=Date().addingTimeInterval(3);while Probe.finished<expected && Date()<end{await Task.yield()};precondition(Probe.finished==expected,"permission callback did not settle")}
 @MainActor static func fresh()->MemoRecorder{Probe.finished=0;Probe.starts=0;Probe.stops=0;Probe.fail=false;Probe.recordOK=true;Probe.urls=[];Probe.intervals=[];Probe.settings=[:];AVAudioSession.shared.callbacks=[];return MemoRecorder()}
 @MainActor static func main()async{
  var cases:[String:Bool]=[:]
  do{let r=fresh();r.start();AVAudioSession.shared.callbacks[0](true);await settle(1);cases["current-grant-starts-once"]=r.recording && Probe.starts==1;r.cancel()}
  do{let r=fresh();r.start();AVAudioSession.shared.callbacks[0](false);await settle(1);cases["current-denial-published"]=r.permissionDenied && !r.recording && Probe.starts==0;r.cancel()}
  do{let r=fresh();r.start();r.cancel();AVAudioSession.shared.callbacks[0](true);await settle(1);cases["cancel-revokes-pending-grant"] = !r.recording && Probe.starts==0;r.cancel()}
  do{let r=fresh();r.start();_=r.stop();AVAudioSession.shared.callbacks[0](true);await settle(1);cases["stop-revokes-pending-grant"] = !r.recording && Probe.starts==0;r.cancel()}
  do{let r=fresh();r.start();r.cancel();AVAudioSession.shared.callbacks[0](false);await settle(1);cases["cancel-revokes-late-denial"] = !r.permissionDenied;r.cancel()}
  do{let r=fresh();r.start();_=r.stop();AVAudioSession.shared.callbacks[0](false);await settle(1);cases["stop-revokes-late-denial"] = !r.permissionDenied;r.cancel()}
  do{let r=fresh();var allowed=true;start(r,isCurrent:{allowed});allowed=false;AVAudioSession.shared.callbacks[0](true);await settle(1);cases["retired-context-cannot-start-microphone"] = !r.recording && Probe.starts==0;r.cancel()}
  do{let r=fresh();var allowed=true;start(r,isCurrent:{allowed});allowed=false;AVAudioSession.shared.callbacks[0](false);await settle(1);cases["retired-context-cannot-publish-denial"] = !r.permissionDenied;r.cancel()}
  do{let r=fresh();r.start();r.start();AVAudioSession.shared.callbacks[0](true);await settle(1);let stale=Probe.starts;AVAudioSession.shared.callbacks[1](true);await settle(2);cases["only-latest-pending-request-starts"]=stale==0 && Probe.starts==1;r.cancel()}
  do{let r=fresh();r.start();r.start();AVAudioSession.shared.callbacks[0](false);await settle(1);let stale=r.permissionDenied;AVAudioSession.shared.callbacks[1](true);await settle(2);cases["old-denial-does-not-mark-current-request"] = !stale && r.recording && !r.permissionDenied;r.cancel()}
  do{let r=fresh();r.start();AVAudioSession.shared.callbacks[0](true);await settle(1);cases["format-and-meter-calibration-control"]=(Probe.settings[AVSampleRateKey]as?Int)==16000 && (Probe.settings[AVNumberOfChannelsKey]as?Int)==1 && (Probe.settings[AVEncoderAudioQualityKey]as?Int)==64 && (Probe.settings[AVFormatIDKey]as?Int)==Int(kAudioFormatMPEG4AAC) && Probe.intervals==[0.08];r.cancel()}
  do{let r=fresh();r.start();AVAudioSession.shared.callbacks[0](true);await settle(1);r.tickForFixture();cases["meter-shape-and-formula-control"]=r.levels.count==28 && r.levels.last==0.5 && r.seconds>=0;r.cancel()}
  do{let r=fresh();r.start();AVAudioSession.shared.callbacks[0](true);await settle(1);r.setElapsedForFixture(0.999);cases["short-memo-remains-discarded"]=r.stop()==nil && !r.recording;r.cancel()}
  do{let r=fresh();r.start();AVAudioSession.shared.callbacks[0](true);await settle(1);r.setElapsedForFixture(1);let result=r.stop();cases["one-second-memo-bytes-and-name-control"]=result?.data==Data("synthetic-memo".utf8) && result?.name.hasPrefix("Voice memo ")==true && result?.name.hasSuffix(".m4a")==true && !r.recording;r.cancel()}
  do{let r=fresh();Probe.fail=true;r.start();AVAudioSession.shared.callbacks[0](true);await settle(1);cases["recorder-construction-failure-control"] = !r.recording && Probe.starts==0;r.cancel()}
  do{let a=fresh();let b=MemoRecorder();a.start();AVAudioSession.shared.callbacks[0](true);await settle(1);b.start();AVAudioSession.shared.callbacks[1](true);await settle(2);let au=Probe.urls[0],bu=Probe.urls[1];cases["each-capture-owns-a-distinct-file"]=au != bu;a.cancel();cases["old-cancel-preserves-current-file"]=FileManager.default.fileExists(atPath:bu.path);b.setElapsedForFixture(1);cases["other-capture-bytes-survive-old-cancel"]=b.stop()?.data==Data("synthetic-memo".utf8);b.cancel()}
  do{let r=fresh();r.start();AVAudioSession.shared.callbacks[0](true);await settle(1);let url=Probe.urls[0];r.setElapsedForFixture(1);let result=r.stop();cases["successful-stop-releases-private-temp-file"]=result != nil && !FileManager.default.fileExists(atPath:url.path);r.cancel()}
  do{let r=fresh();r.start();AVAudioSession.shared.callbacks[0](true);await settle(1);let url=Probe.urls[0];r.setElapsedForFixture(0.999);_=r.stop();cases["discarded-short-memo-releases-temp-file"] = !FileManager.default.fileExists(atPath:url.path);r.cancel()}
  do{let r=fresh();Probe.fail=true;r.start();AVAudioSession.shared.callbacks[0](true);await settle(1);cases["failed-construction-releases-partial-file"] = !FileManager.default.fileExists(atPath:Probe.urls[0].path);r.cancel()}
  do{let r=fresh();Probe.recordOK=false;r.start();AVAudioSession.shared.callbacks[0](true);await settle(1);cases["failed-record-effect-cannot-claim-recording"] = !r.recording && Probe.starts==0;cases["failed-record-releases-file-and-installs-no-meter"] = !FileManager.default.fileExists(atPath:Probe.urls[0].path) && Probe.intervals.isEmpty;r.cancel()}
  let failed=cases.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["cases":cases,"total":cases.count,"passed":cases.count-failed,"failed":failed],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--source-root',type=Path,required=True);ap.add_argument('--out',type=Path,required=True);a=ap.parse_args();r=a.source_root.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);rel='apps/ios/Arbos/Arbos/UI/Recorder.swift';src=(r/rel).read_text();start=time.monotonic();body=src[src.index('@MainActor\nfinal class MemoRecorder'):];assert body.count('Task { @MainActor in')==2;body=body.replace('Task { @MainActor in','Task { @MainActor in\n                defer { Probe.finished += 1 }',1);body=body.replace('Int(Date().timeIntervalSince1970)','1000');typed='func start(isCurrent:'in src
 adapter='@MainActor func start(_ r:MemoRecorder,isCurrent:@escaping @MainActor()->Bool){r.start(isCurrent:isCurrent)}\n'if typed else'@MainActor func start(_ r:MemoRecorder,isCurrent:@escaping @MainActor()->Bool){r.start()}\n'
 helper=(r/'apps/ios/Arbos/Arbos/Sync/WorkLifetime.swift').read_text();fixture=helper+'\n'+FAKES.replace('@main struct Checks',body+'\n'+adapter+'\n@main struct Checks');(out/'fixture.swift').write_text(fixture)
 with tempfile.TemporaryDirectory(prefix='arbos-recorder-')as raw:
  tmp=Path(raw);f=tmp/'main.swift';f.write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['/usr/bin/xcrun','swiftc','-parse-as-library','-swift-version','5',str(f),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=90);(out/'compile.log').write_text(c.stdout+c.stderr);record={'source_sha256':hashlib.sha256(src.encode()).hexdigest(),'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode,'scope':'Actual complete MemoRecorder +WorkLifetime with fake AV session/recorder/timer, synthetic owned tempbytes; only injecteddefer observespermissioncallbacksettlement andprivatefilenameclockfixed; no UIKit/audio/mic/device/userdata'}
  if c.returncode==0:
   x=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);record.update(exit_code=x.returncode,results=json.loads(x.stdout));(out/'test.log').write_text(x.stdout+x.stderr)
 record['seconds']=time.monotonic()-start;record['source_unchanged']=src==(r/rel).read_text();(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record));assert c.returncode==0 and record['source_unchanged'];raise SystemExit(record['exit_code'])
if __name__=='__main__':main()
