"""Actual CallManager method bodies with synthetic REST/Room collaborators only."""
from pathlib import Path
import argparse,hashlib,json,os,re,subprocess,tempfile,time
ROOT=Path(__file__).resolve().parent
from support import repository, dependency_version, fence, record
REPO=repository(ROOT)
SOURCE=Path('apps/ios/Arbos/Arbos/Store/CallManager.swift')

def method(text,name):
    matches=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+(r'\(chatId' if name=='start' else r'\('),text,re.M))
    assert len(matches)==1,(name,len(matches))
    start=matches[0].start();end=text.index('\n    }',matches[0].end())+6
    return text[start:end]

PRELUDE=r'''
import Foundation
import os
let log=Logger(subsystem:"arbos.synthetic",category:"calls")
let audioLog=log
struct UserError {let sentence:String;static func refused(_ a:String,_ b:String)->Self{Self(sentence:b)}}
struct RelayError:Error {let status:Int;let message:String}
struct RemoteParticipant {var attributes:[String:String]=[:]}
enum CallKind {case voice,video}
struct CallPerson {var id="person";var displayName="Person"}
struct Call {var callId:String;var chatId:String;var projectId="project";var kind=CallKind.voice;var token:String?="synthetic";var livekitUrl="http://synthetic.invalid";var canSpeak:Bool?;var isPeople=true;var agents:[CallPerson]=[]}
enum CXCallEndedReason {case remoteEnded,failed,unanswered}
struct CameraCaptureOptions {enum Position {case front,back};enum Dimensions{case h720_169};init(position:Position,dimensions:Dimensions,fps:Int){}}
struct AudioCaptureOptions {}
struct RoomOptions {init(defaultCameraCaptureOptions:CameraCaptureOptions,defaultAudioCaptureOptions:AudioCaptureOptions,adaptiveStream:Bool,dynacast:Bool){}}
struct ConnectOptions {init(autoSubscribe:Bool){}}
enum AVCaptureDevice {enum Status{case authorized};static func authorizationStatus(for:Media)->Status{.authorized};enum Media{case video}}
@MainActor final class Gate<T> {var waiters:[CheckedContinuation<T,Error>]=[];var waiter:CheckedContinuation<T,Error>?{waiters.first};var began=false;func wait()async throws ->T{began=true;return try await withCheckedThrowingContinuation{waiters.append($0)}};func finish(_ v:T){if !waiters.isEmpty{waiters.removeFirst().resume(returning:v)}};func fail(){if !waiters.isEmpty{waiters.removeFirst().resume(throwing:RelayError(status:500,message:"synthetic"))}}}
@MainActor final class RelayAPI {
 var endpoint="http://synthetic.invalid";func bound(to: @escaping ()->Bool)->RelayAPI{self};func frozenAuthority()->RelayAPI{self}
 let gate=Gate<Call>();var leaveGate:Gate<Bool>?;var starts=0;var joins=0;var leaves:[String]=[];var lookups=0
 func startCall(chatId:String,projectId:String?,kind:CallKind,ring:[String]?)async throws ->(Call,Bool){starts+=1;return(try await gate.wait(),true)}
 func joinCall(_ id:String)async throws ->Call{joins+=1;return try await gate.wait()};func activeCall(chatId:String,projectId:String?)async throws ->Call?{lookups+=1;return nil}
 func leaveCall(_ id:String)async throws ->(ended:Bool,durationS:Int){leaves.append(id);if let leaveGate{_ = try await leaveGate.wait()};return(true,10)}
}
@MainActor enum LiveKit {
 @MainActor final class Room {
  let localParticipant=Participant();var remoteParticipants:[String:Participant]=[:];var connections=0;var disconnects=0;var pause=false;let gate=Gate<Bool>()
  var delegates:Room{self};func add(delegate:CallManager){};func remove(delegate:CallManager){}
  func connect(url:String,token:String,connectOptions:ConnectOptions,roomOptions:RoomOptions)async throws{connections+=1;if pause{_ = try await gate.wait()}}
  func disconnect()async{disconnects+=1}
 }
 @MainActor final class Participant {var attributes:[String:String]=[:];var microphones:[Bool]=[];func setMicrophone(enabled:Bool)async throws{microphones.append(enabled)};func setCamera(enabled:Bool)async throws{}}
}
@MainActor final class Provider {func reportCall(with:UUID,endedAt:Date,reason:CXCallEndedReason){};func reportOutgoingCall(with:UUID,connectedAt:Date){}}
@MainActor final class CallManager {
 enum Phase:Equatable {case idle,ringing,connecting,connected,reconnecting}
 static var permission:Gate<Bool>?
 static func micGranted()async ->Bool{if let permission{return (try? await permission.wait()) ?? false};return true}
 static func cameraGranted()async ->Bool{true}
 var phase=Phase.idle;var call:Call?;var error:UserError?;var api:RelayAPI?;var room=LiveKit.Room()
 var isPresented=false;var ringbackWanted=false;var ringIds:Set<String>?;var muted=false;var cameraOn=false;var screenShareOn=false;var speakerOn=false;var frontCamera=true
 var callKitOwnsAudio=false;var cxUUID:UUID?;var isTearingDown=false;var anyoneJoined=false;var outgoingTimeout:Task<Void,Never>?
 var connectedAt:Date?;var tiles:[String]=[];var agentSeeing=false;var seeingAgentName:String?;var isFixture=false
 var incoming:(call:Call,from:CallPerson?)?;var ringTimeout:Task<Void,Never>?;var callbacks=0;var activeByChat:[String:Call]=[:];var writes:[String]=[];let provider=Provider()
 var me:()->CallPerson?={CallPerson()};var writeSystemLine:((String,String,String,String)->Void)?
 var isActive:Bool{phase == .connecting || phase == .connected || phase == .reconnecting}
 init(_ api:RelayAPI){self.api=api;writeSystemLine={ [weak self] _,_,_,id in self?.writes.append(id)}}
 static func describe(_ s:String,_ e:Error)->UserError{UserError(sentence:s)}
 func stopIncomingRing(){};func prepareCallAudioSession(video:Bool){};func syncRingback(){};func stopRingback(){};func reportOutgoing(title:String,video:Bool){}
 func armOutgoingTimeout(){};func handleAttributes(_ v:[String:String],of:LiveKit.Participant){};func handleAttributes(_ v:[String:String],of:RemoteParticipant){callbacks+=1};func applySpeakerRoute(){};func enableCallAudioEngine(video:Bool){};func disableCallAudioEngine(){};func refreshTiles(){callbacks+=1}
 func noteActive(_ c:Call?,chatId:String){activeByChat[chatId]=c};func callEndedLine(durationS:Int)->String{"Ended"}
'''
TESTS=r'''
}
@main struct Checks {
 @MainActor static func spin(_ f:()->Bool)async ->Bool{for _ in 0..<10000{if f(){return true};await Task.yield()};return f()}
 @MainActor static func start(_ m:CallManager,_ chat:String)->Task<Void,Never>{Task{await m.start(chatId:chat,projectId:nil,kind:.voice,title:chat)}}
 @MainActor static func lateREST()async ->Bool{
  let a=RelayAPI();let m=CallManager(a);let t=start(m,"old");guard await spin({a.gate.began})else{return false};await m.hangUp();a.gate.finish(Call(callId:"old-call",chatId:"old"));await t.value
  return m.phase == .idle && m.call == nil && m.room.connections==0 && a.leaves==["old-call"] && m.writes.isEmpty
 }
 @MainActor static func lateRESTError()async ->Bool{
  let a=RelayAPI();let b=RelayAPI();let m=CallManager(a);let t=start(m,"old");guard await spin({a.gate.began})else{return false};await m.hangUp();m.api=b;let u=start(m,"new");guard await spin({b.gate.began})else{return false};a.gate.fail();await t.value
  let preserved=m.phase == .connecting && m.error == nil;b.gate.finish(Call(callId:"new-call",chatId:"new"));await u.value
  return preserved && m.call?.callId=="new-call" && m.phase == .connected && b.leaves.isEmpty
 }
 @MainActor static func duplicatePermission()async ->Bool{
  let permission=Gate<Bool>();CallManager.permission=permission;let a=RelayAPI();let m=CallManager(a);let t=start(m,"one");guard await spin({permission.began})else{return false}
  let u=start(m,"two");for _ in 0..<50{await Task.yield()};let count=permission.waiters.count
  while !permission.waiters.isEmpty{permission.finish(true)};CallManager.permission=nil
  guard await spin({a.gate.began})else{return false};for _ in 0..<50{await Task.yield()};while !a.gate.waiters.isEmpty{a.gate.finish(Call(callId:"call",chatId:"one"))};await t.value;await u.value
  return count==1 && a.starts==1
 }
 @MainActor static func sameCallReplacement()async ->Bool{
  let a=RelayAPI();let b=RelayAPI();let m=CallManager(a);let t=start(m,"same");guard await spin({a.gate.began})else{return false};await m.hangUp();INVALIDATE_CALL for _ in 0..<50{await Task.yield()};m.api=b
  let u=Task{await m.join(Call(callId:"shared-call",chatId:"same"),title:"same")};guard await spin({b.gate.began})else{return false}
  a.gate.finish(Call(callId:"shared-call",chatId:"same"));await t.value;let kept=a.leaves.isEmpty && b.leaves.isEmpty && m.call==nil
  b.gate.finish(Call(callId:"shared-call",chatId:"same"));await u.value;return kept && m.call?.callId=="shared-call" && m.phase == .connected
 }
 @MainActor static func capturedAuthority()async ->Bool{
  let a=RelayAPI();let b=RelayAPI();let m=CallManager(a);let t=start(m,"old");guard await spin({a.gate.began})else{return false};await m.hangUp();m.api=b;let u=start(m,"new");guard await spin({b.gate.began})else{return false}
  a.gate.finish(Call(callId:"old-call",chatId:"old"));await t.value;let kept=m.phase == .connecting && m.call==nil
  b.gate.finish(Call(callId:"new-call",chatId:"new"));await u.value;return kept && a.leaves==["old-call"] && b.leaves.isEmpty && m.call?.callId=="new-call"
 }
 @MainActor static func mediaReplacement()async ->Bool{
  let a=RelayAPI();let b=RelayAPI();let m=CallManager(a);let t=start(m,"old");guard await spin({a.gate.began})else{return false};m.room.pause=true;a.gate.finish(Call(callId:"old-call",chatId:"old"));guard await spin({m.room.gate.began})else{return false};let old=m.room
  await m.hangUp();m.api=b;let u=start(m,"new");guard await spin({b.gate.began})else{return false};old.gate.finish(true);await t.value
  b.gate.finish(Call(callId:"new-call",chatId:"new"));for _ in 0..<50{await Task.yield()};while !m.room.gate.waiters.isEmpty{m.room.gate.finish(true)};await u.value;return old !== m.room && old.localParticipant.microphones.isEmpty && m.call?.callId=="new-call" && m.phase == .connected
 }
 @MainActor static func permissionAfterHangup()async ->Bool{
  let permission=Gate<Bool>();CallManager.permission=permission;let a=RelayAPI();let m=CallManager(a);let t=start(m,"old");guard await spin({permission.began})else{return false};await m.hangUp();permission.finish(true);CallManager.permission=nil
  for _ in 0..<30{await Task.yield()};if a.gate.began{a.gate.finish(Call(callId:"late",chatId:"old"))};await t.value
  return a.starts==0 && m.phase == .idle
 }
 @MainActor static func lateMedia()async ->Bool{
  let a=RelayAPI();let m=CallManager(a);let t=start(m,"old");guard await spin({a.gate.began})else{return false};m.room.pause=true
  a.gate.finish(Call(callId:"old-call",chatId:"old"));guard await spin({m.room.gate.began})else{return false};let old=m.room;await m.hangUp();old.gate.finish(true);await t.value
  return m.phase == .idle && m.call == nil && old.localParticipant.microphones.isEmpty
 }
 @MainActor static func ordinary()async ->Bool{
  let a=RelayAPI();let m=CallManager(a);let t=start(m,"ok");guard await spin({a.gate.began})else{return false};a.gate.finish(Call(callId:"ok-call",chatId:"ok"));await t.value
  return m.phase == .connected && m.call?.callId=="ok-call" && m.writes==["ok-call"] && m.room.localParticipant.microphones==[true]
 }
 @MainActor static func joinAfterHangup()async ->Bool{
  let a=RelayAPI();let m=CallManager(a);let t=Task{await m.join(Call(callId:"old-call",chatId:"old"),title:"old")};guard await spin({a.gate.began})else{return false};await m.hangUp();a.gate.finish(Call(callId:"old-call",chatId:"old"));await t.value
  return m.phase == .idle && m.call == nil && a.leaves==["old-call"]
 }
 @MainActor static func leaveAfterAccountReplacement()async ->Bool{
  let a=RelayAPI();let b=RelayAPI();let m=CallManager(a);let t=start(m,"old");guard await spin({a.gate.began})else{return false};a.gate.finish(Call(callId:"old-call",chatId:"old"));await t.value
  let paused=Gate<Bool>();a.leaveGate=paused;let hang=Task{await m.hangUp()};guard await spin({paused.began})else{return false};INVALIDATE_CALL m.api=b;m.writes=[];m.activeByChat["old"]=Call(callId:"B-call",chatId:"old");paused.finish(true);await hang.value
  return m.writes.isEmpty && b.lookups==0 && m.activeByChat["old"]?.callId=="B-call"
 }
 @MainActor static func oldRoomCallback()async ->Bool{
  let a=RelayAPI();let b=RelayAPI();let m=CallManager(a);let t=start(m,"old");guard await spin({a.gate.began})else{return false};a.gate.finish(Call(callId:"old-call",chatId:"old"));await t.value;let old=m.room;await m.hangUp();m.api=b;let u=start(m,"new");guard await spin({b.gate.began})else{return false};m.callbacks=0;m.room(old,participantDidConnect:RemoteParticipant());for _ in 0..<50{await Task.yield()};let rejected=m.callbacks==0;b.gate.finish(Call(callId:"new-call",chatId:"new"));await u.value;m.callbacks=0;m.room(m.room,participantDidConnect:RemoteParticipant());for _ in 0..<50{await Task.yield()};return rejected && m.callbacks>0
 }
 @MainActor static func differentRemoteMember()async ->Bool{
  let a=RelayAPI();let b=RelayAPI();let m=CallManager(a);m.me={CallPerson(id:"A")};let t=start(m,"same");guard await spin({a.gate.began})else{return false};await m.hangUp();INVALIDATE_CALL for _ in 0..<50{await Task.yield()};m.api=b;m.me={CallPerson(id:"B")};let u=Task{await m.join(Call(callId:"shared-call",chatId:"same"),title:"same")};guard await spin({b.gate.began})else{return false};a.gate.finish(Call(callId:"shared-call",chatId:"same"));await t.value;let cleaned=a.leaves==["shared-call"]&&b.leaves.isEmpty;b.gate.finish(Call(callId:"shared-call",chatId:"same"));await u.value;return cleaned&&m.call?.callId=="shared-call"&&m.phase == .connected
 }
 @MainActor static func differentRelaySameIds()async ->Bool{
  let a=RelayAPI();let b=RelayAPI();b.endpoint="http://other.synthetic.invalid";let m=CallManager(a);let t=start(m,"same");guard await spin({a.gate.began})else{return false};await m.hangUp();m.api=b;let u=Task{await m.join(Call(callId:"shared-call",chatId:"same"),title:"same")};guard await spin({b.gate.began})else{return false};a.gate.finish(Call(callId:"shared-call",chatId:"same"));await t.value;let cleaned=a.leaves==["shared-call"]&&b.leaves.isEmpty;b.gate.finish(Call(callId:"shared-call",chatId:"same"));await u.value;return cleaned&&m.phase == .connected
 }
 @MainActor static func main()async {
  var failures=0
  let cases: [(String, @MainActor () async -> Bool)] = [("late REST cannot resurrect",lateREST),("late error cannot clear replacement",lateRESTError),("permission completion after hangup",permissionAfterHangup),("late media cannot activate",lateMedia),("ordinary start control",ordinary),("late join cleanup",joinAfterHangup),("permission reservation prevents duplicate starts",duplicatePermission),("same-call replacement is never left",sameCallReplacement),("cleanup retains original API authority",capturedAuthority),("late media cannot affect replacement room",mediaReplacement),("old leave cannot mutate replacement account",leaveAfterAccountReplacement),("queued old-room delegate rejected/current control",oldRoomCallback),("different remote user membership gets old cleanup",differentRemoteMember),("different relay scope gets old cleanup",differentRelaySameIds)]
  for(name,run) in cases {
   let ok=await run();print("\(ok ? "PASS":"FAIL") \(name)");if !ok{failures+=1}
  }
  print("RESULT tests=14 failed=\(failures)");exit(failures==0 ? 0:1)
 }
}
'''

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--tree',type=Path,default=REPO);parser.add_argument('--label',default='swift');parser.add_argument('--record-dir',type=Path);args=parser.parse_args()
    source=(args.tree/SOURCE).read_text()
    methods=['start','join','connect','teardown','hangUp','syncActiveBar']
    extra=''
    # New ownership helpers are actual methods; the original fixture has none.
    for name in ['beginAttempt','ownsAttempt','releaseStaleCall','invalidateForSignOut']:
        if re.search(r'^    (?:private )?func '+name+r'\(',source,re.M):methods.append(name)
    lifetime=args.tree/SOURCE.parent/'CallAttempt.swift'
    if lifetime.exists():extra=lifetime.read_text()
    if 'private struct CallAttempt {' in source:
        at=source.index('private struct CallAttempt {');end=source.index('\n}',at)+2;extra=source[at:end]
    fields=''
    if 'private var callAttempt:' in source:
        fields=re.search(r'^    private var callAttempt:.*$',source,re.M).group(0)+'\n'
        if 'private var accountLifetime' in source:fields+=re.search(r'^    private var accountLifetime.*$',source,re.M).group(0)+'\n'
        if 'private var staleCallCleanup:' in source:fields+=re.search(r'^    private var staleCallCleanup:.*$',source,re.M).group(0)+'\n'
    at=source.index('    nonisolated func room(_ room: LiveKit.Room, participantDidConnect');delegate=source[at:source.index('\n    }',at)+6]
    fixture=PRELUDE+fields+'\n'.join(method(source,n) for n in methods)+'\n'+delegate+TESTS+'\n'+extra
    fixture=fixture.replace('INVALIDATE_CALL', 'm.invalidateForSignOut();' if 'func invalidateForSignOut' in source else '')
    start=time.monotonic()
    with tempfile.TemporaryDirectory(prefix='arbos-call-check-') as temp:
        temp=Path(temp);swift=temp/'main.swift';swift.write_text(fixture);binary=temp/'checks';home=temp/'home';home.mkdir()
        env={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(home),'TMPDIR':str(temp)}
        compiled=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(swift),'-o',str(binary)],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=30)
        if compiled.returncode:output=compiled.stdout;code=compiled.returncode
        else:
            policy='(version 1)(allow default)(deny network*)(deny file-read* (subpath "/Users/Shared"))'
            p=subprocess.run(['/usr/bin/sandbox-exec','-p',policy,str(binary)],env=env,cwd=temp,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=15);output=p.stdout;code=p.returncode
    result={'exit_code':code,'compile_exit':compiled.returncode,'wall_seconds':time.monotonic()-start,'source_sha256':hashlib.sha256(source.encode()).hexdigest(),'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'methods':methods,'output':output}
    record(args.record_dir,args.label,result,output);print(json.dumps(result));return code

if __name__=='__main__':raise SystemExit(main())
