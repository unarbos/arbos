from pathlib import Path
import argparse,hashlib,json,subprocess,tempfile,time
P=Path(__file__).resolve().parent;ROOT=P.parents[2];UI=Path('apps/ios/Arbos/Arbos/UI')
F=r'''
import Foundation
@propertyWrapper struct Published<T>{var wrappedValue:T;init(wrappedValue:T){self.wrappedValue=wrappedValue}}
protocol ObservableObject{}
final class AVAudioPlayer{let duration=2.0;var isPlaying=false;var currentTime=0.0;static var plays=0;init(data:Data)throws{};func play()->Bool{Self.plays+=1;isPlaying=true;return true};func pause(){isPlaying=false};func stop(){isPlaying=false};func prepareToPlay(){}}
final class AVAudioSession{static let shared=AVAudioSession();static func sharedInstance()->AVAudioSession{shared};enum Category{case playback};enum Mode{case spokenAudio};struct Options:OptionSet{let rawValue:Int;static let duckOthers=Options(rawValue:1)};func setCategory(_ c:Category,mode:Mode,options:Options)throws{};func setActive(_ x:Bool)throws{}}
@MainActor final class URLSession{static let shared=URLSession();static var pending:CheckedContinuation<Void,Never>?;static var started=false;func data(for req:URLRequest)async throws->(Data,URLResponse){Self.started=true;await withCheckedContinuation{Self.pending=$0};return(Data([1]),HTTPURLResponse(url:req.url!,statusCode:200,httpVersion:nil,headerFields:nil)!)} }
PLAYER
@MainActor final class Owner{var current=true}
@main struct Checks{
 @MainActor static func reset(){URLSession.started=false;AVAudioPlayer.plays=0}
 @MainActor static func main()async{var cases:[String:Bool]=[:];let url=URL(string:"https://fixture.invalid/audio")!
 for play in [false,true]{reset();let owner=Owner();let p=MemoPlayer();if play{TOGGLE}else{DURATION};while !URLSession.started{await Task.yield()};owner.current=false;URLSession.pending?.resume();URLSession.pending=nil;for _ in 0..<10{await Task.yield()};cases[play ? "retired-pending-toggle-does-not-play":"retired-duration-does-not-publish"]=p.durationLabel=="0:00" && AVAudioPlayer.plays==0 && !p.playing}
 for play in [false,true]{reset();let owner=Owner();owner.current=false;let p=MemoPlayer();if play{TOGGLE}else{DURATION};for _ in 0..<5{await Task.yield()};cases[play ? "retired-toggle-refuses-fetch":"retired-duration-refuses-fetch"] = !URLSession.started;if URLSession.started{URLSession.pending?.resume();URLSession.pending=nil;for _ in 0..<5{await Task.yield()}}}
 do{reset();let owner=Owner();let p=MemoPlayer();DURATION;while !URLSession.started{await Task.yield()};URLSession.pending?.resume();URLSession.pending=nil;for _ in 0..<10{await Task.yield()};cases["current-duration-control"]=p.durationLabel=="0:02" && AVAudioPlayer.plays==0}
 do{reset();let owner=Owner();let p=MemoPlayer();TOGGLE;while !URLSession.started{await Task.yield()};URLSession.pending?.resume();URLSession.pending=nil;for _ in 0..<10{await Task.yield()};cases["current-play-control"]=p.playing && AVAudioPlayer.plays==1;TOGGLE;cases["current-pause-control"] = !p.playing && AVAudioPlayer.plays==1}
 let failed=cases.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["total":cases.count,"passed":cases.count-failed,"failed":failed,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('arm',choices=['before','after']);ap.add_argument('label');a=ap.parse_args();src=P/'base/Transcript.swift'if a.arm=='before'else ROOT/UI/'Transcript.swift';s=src.read_text();x=s.index('@MainActor\nfinal class MemoPlayer:');part=s[x:s.index('\n/// Streaming fade',x)];owned='isCurrent:'in part;toggle='p.toggle(url: url, headers: [:]'+(', isCurrent: { owner.current }'if owned else'')+')';duration=toggle.replace('toggle','loadDuration');fixture=F.replace('PLAYER',part).replace('TOGGLE',toggle).replace('DURATION',duration);out=P/a.label;out.mkdir(exist_ok=False);(out/'fixture.swift').write_text(fixture);start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='chat-player-')as raw:
  tmp=Path(raw);f=tmp/'main.swift';f.write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(f),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);j={'source_sha256':hashlib.sha256(s.encode()).hexdigest(),'class_sha256':hashlib.sha256(part.encode()).hexdigest(),'compile_exit':c.returncode}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);j.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  assert s==src.read_text();j['seconds']=time.monotonic()-start;(out/'GATES.json').write_text(json.dumps(j,indent=2)+'\n');print(json.dumps(j))
if __name__=='__main__':main()
