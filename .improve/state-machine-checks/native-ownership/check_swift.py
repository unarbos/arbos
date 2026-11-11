"""Compile exact controller methods with local collaborators; no SDK/keychain/network."""
from pathlib import Path
import argparse, hashlib, json, os, re, subprocess, tempfile, time
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
P = 'apps/ios/Arbos/Arbos/'

def method(text, name):
    starts = list(re.finditer(r'^    (?:private )?func ' + re.escape(name) + (r'\(tokens' if name == 'adopt' else r'\('), text, re.M))
    if len(starts) != 1:
        raise AssertionError(f'Expected one actual {name} method, found {len(starts)}')
    start = starts[0].start()
    end = text.index('\n    }', starts[0].end()) + len('\n    }')
    return text[start:end]

PRELUDE = r'''
import Foundation
import os

typealias Id = String
struct AuthTokens: Equatable { var relayUrl = "http://synthetic.invalid"; var deviceId: String; var accessToken: String; var refreshToken = "synthetic-refresh" }
struct AuthUser: Equatable { var id = "synthetic" }
struct DeviceCredentials: Equatable { var relayUrl: String; var deviceId: String; var deviceToken: String }
struct RelayError: Error { let status: Int }
struct UserError { static func of(_ a:String,_ b:Self,_ c:String)->Self{Self()}; static let authExpired = Self() }
enum Keychain { static var writes:[AuthTokens] = []; static func storeAuth(_ v:AuthTokens)throws {writes.append(v)}; static func deleteAuth(relayUrl:String){}; static func delete(relayUrl:String){} }
struct DeviceStore { struct Usage {} }
struct ConnectorsView {}
enum Link { case connecting, live }
final class FakeStore {func close(wipe:Bool){}}
final class FakeRoom {func stop(){} }
struct Message: Equatable { let id:String }
final class CoreChatDoc: @unchecked Sendable {
    var count = 10
    var value:[Message]
    let gate: DispatchSemaphore?
    let began: DispatchSemaphore?
    init(_ id:String, pause:Bool=false){value=[Message(id:id)];gate=pause ? DispatchSemaphore(value:0):nil;began=pause ? DispatchSemaphore(value:0):nil}
    func readAll()->[Message]{began?.signal();gate?.wait();return value}
}
struct FakeEndpoint {}
typealias RelayAPI = FakeAPI
@MainActor final class FakeAPI {
    static var cleanupTokens:[String]=[]
    let endpoint:FakeEndpoint
    var token:()->String
    func frozenAuthority()->FakeAPI{let bearer=token();return FakeAPI(endpoint:endpoint,token:{bearer})}
    var refresher:(() async->Bool)?
    func bound(to current:@escaping ()->Bool)->FakeAPI{let result=FakeAPI(endpoint:endpoint,token:token);result.refresher=refresher;return result}
    init(endpoint:FakeEndpoint=FakeEndpoint(),token:@escaping ()->String={""}){self.endpoint=endpoint;self.token=token}
    var calls=0
    var waiters:[CheckedContinuation<(AuthTokens,AuthUser?),Error>?]=[]
    func refresh(_ v:AuthTokens)async throws ->(AuthTokens,AuthUser?){calls += 1;return try await withCheckedThrowingContinuation{waiters.append($0)}}
    func finish(_ i:Int,_ value:AuthTokens?=nil, failure:Bool=false){let w=waiters[i];waiters[i]=nil;if failure{w?.resume(throwing:RelayError(status:401))}else{w?.resume(returning:(value!,nil))}}
    func unregisterPush(deviceId:String)async throws{Self.cleanupTokens.append(token())}
    func unregisterVoip(deviceId:String)async throws{Self.cleanupTokens.append(token())}
    func logout()async{Self.cleanupTokens.append(token())}
    func matrixSession()async throws -> MatrixCredentials? {MatrixCredentials()}
}
struct MatrixCredentials {let homeserver="http://matrix.synthetic.invalid";let userId="synthetic";let accessToken="synthetic";let deviceId="synthetic"}
enum ArbosFFI {
 struct MatrixSession {let homeserver:String;let userId:String;let accessToken:String;let deviceId:String}
 struct MatrixSnapshot {var spaces:[String];var channels:[String]}
 struct MatrixMessageRow {}
 protocol MatrixListener: AnyObject {func onRooms(snapshot:MatrixSnapshot);func onTimeline(roomId:String,rows:[MatrixMessageRow]);func onConnection(connected:Bool,error:String?)}
 @MainActor final class MatrixClient {
  let name:String;var stops=0;var paused=false;var snapshotWaiter:CheckedContinuation<MatrixSnapshot,Never>?
  init(_ name:String){self.name=name}
  func stop(){stops += 1}
  func snapshot()async -> MatrixSnapshot{if paused{return await withCheckedContinuation{snapshotWaiter=$0}};return MatrixSnapshot(spaces:[],channels:[name])}
 }
}
@MainActor enum MatrixSDK {
 static var listeners:[MatrixBridgeListener]=[]
 static var waiters:[CheckedContinuation<ArbosFFI.MatrixClient,Error>?]=[]
 static func start(_ listener:MatrixBridgeListener)async throws ->ArbosFFI.MatrixClient {listeners.append(listener);return try await withCheckedThrowingContinuation{waiters.append($0)}}
 static func finish(_ i:Int,_ client:ArbosFFI.MatrixClient){let w=waiters[i];waiters[i]=nil;w?.resume(returning:client)}
 static func reset(){listeners=[];waiters=[]}
}
@MainActor func matrixStart(session:ArbosFFI.MatrixSession,listener:MatrixBridgeListener)async throws ->ArbosFFI.MatrixClient{try await MatrixSDK.start(listener)}
let matrixLog = Logger(subsystem:"arbos.synthetic.test",category:"matrix")
@MainActor final class CallManager {static let shared=CallManager();var api:FakeAPI?;func forgetVoipRegistration(){};func invalidateForSignOut(){}}
@MainActor final class PushManager {static let shared=PushManager();func forgetRegistration(){};func setBadge(_ n:Int){}}

@MainActor final class AppModel {
    var api=FakeAPI()
    var relay:FakeAPI {api}
    var credentials:DeviceCredentials?
    var tokens:AuthTokens?
    var user:AuthUser?
    var credits:Int?
    var needsProfileSetup=false
    var signInError:UserError?
    var signingIn=false;var pairingBusy=false
    var refreshTask: Task<Bool,Never>?
    var isFixture=false
    var matrixClient:ArbosFFI.MatrixClient?
    var matrixListener:MatrixBridgeListener?
    var matrixStarting=false
    var matrixSnapshot=ArbosFFI.MatrixSnapshot(spaces:[],channels:[])
    var matrixConnected=true
    var matrixDisconnectedSince:Date?
    var channelRows:[Id:[Message]]=[:]
    var channelAtStart:Set<Id>=[]
    var channelLoading:Set<Id>=[]
    var caughtUp:Set<Id>=[]
    var registryTask:Task<Void,Never>?
    var rooms:[Id:FakeRoom]=[:]
    var docs:[Id:CoreChatDoc]=[:]
    var messages:[Id:[Message]]=[:]
    var projects:[Int]=[];var chats:[Id:[Int]]=[:]
    var unacked:[Id:Set<Id>]=[:];var peopleChats:[Int]=[];var peopleUnread:[Int]=[];var unreadCounts:[Id:Int]=[:]
    var servers:[Int]=[];var serversAvailable=false;var directory:[Int]=[];var directoryLoadedAt=0
    var dirtyDocs:Set<Id>=[];var dirtySince:Int?;var flushTask:Task<Void,Never>?;var storeDocsLoaded=false
    var readPutTasks:[Id:Task<Void,Never>]=[:];var lastReadPutValue:[Id:Int]=[:];var lastReadPutAt:[Id:Int]=[:];var pendingReadPutValue:[Id:Int]=[:]
    var roomUnavailableFailures:[Id:Int]=[:];var roomUnavailableAt:[Id:Date]=[:];var roomCreatedAt:[Id:Date]=[:];var roomRestoredCursors:[Id:Int]=[:]
    var store:FakeStore?;var storeUsage=DeviceStore.Usage();var link=Link.connecting;var roomLinks:[Id:Int]=[:];var roomLinkSince:[Id:Int]=[:];var relayLink=Link.live
    var connectors=ConnectorsView();var connectorsBusy=false
    var pushRefused:Set<Id>=[];var pushRefusalText:[Id:String]=[:];var pushRefusalReason:[Id:String]=[:]
    var snapshotPending:Set<Id>=[];var snapshotLastAt:[Id:Double]=[:];var snapshotGeneration:[Id:Int]=[:]
    static let asyncSnapshotMinRows=2
    var snapshots:[String]=[];var connections:[Bool]=[];var timelines=0
    func fail(_ s:String,_ e:Error){}
    func beginFirstLogin(){};func endFirstLogin(){};func startAfterPairing(){}
    func forgetDoc(_ key:Id){}
    func applySnapshot(_ chatId:Id,list:[Message]){messages[chatId]=list}
    func applyMatrixSnapshot(_ s:ArbosFFI.MatrixSnapshot){matrixSnapshot=s;snapshots += s.channels}
    func applyMatrixConnection(connected:Bool,error:String?){connections.append(connected)}
    func applyMatrixTimeline(roomId:String,rows:[ArbosFFI.MatrixMessageRow]){timelines += 1}
    func install(_ t:AuthTokens){adopt(tokens:t,user:nil,persist:false)}
    func requestRead(_ key:Id){emitSnapshot(key)}
    func removeRoom(_ key:Id){wipeRoom(key)}
'''
TESTS = r'''
}
enum DebugHooks {static let skipFirstLogin=true}
@main struct Checks {
 @MainActor static func spin(_ predicate:()->Bool)async ->Bool {for _ in 0..<10000 {if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func token(_ id:String,_ access:String?=nil)->AuthTokens{AuthTokens(deviceId:id,accessToken:access ?? id)}
 @MainActor static func checkRefreshAfterSignOut()async ->Bool{
  Keychain.writes=[];let m=AppModel();m.install(token("A"));let t=Task{await m.refreshTokens()};guard await spin({m.api.calls==1})else{return false}
  m.signOut(remote:false);m.api.finish(0,token("A","A-new"));let result=await t.value
  return !result && m.credentials==nil && Keychain.writes.isEmpty
 }
 @MainActor static func checkStaleRefusal()async ->Bool{
  let m=AppModel();m.install(token("A"));let t=Task{await m.refreshTokens()};guard await spin({m.api.calls==1})else{return false}
  m.signOut(remote:false);m.install(token("B"));m.api.finish(0,failure:true);_ = await t.value
  return m.credentials?.deviceId=="B"
 }
 @MainActor static func checkCurrentRefreshControl()async ->Bool{
  let m=AppModel();m.install(token("A"));let a=Task{await m.refreshTokens()};let b=Task{await m.refreshTokens()};guard await spin({m.api.calls==1})else{return false}
  m.api.finish(0,token("A","A-next"));let x=await a.value;let y=await b.value
  return x && y && m.api.calls==1 && m.tokens?.accessToken=="A-next"
 }
 @MainActor static func checkStaleSingleFlightCleanup()async ->Bool{
  let m=AppModel();m.install(token("A"));let a=Task{await m.refreshTokens()};guard await spin({m.api.calls==1})else{return false}
  m.signOut(remote:false);m.install(token("B"));let b=Task{await m.refreshTokens()}
  guard await spin({m.api.calls==2})else{m.api.finish(0,token("A","old"));_ = await a.value;_ = await b.value;return false}
  m.api.finish(0,token("A","old"));_ = await a.value;let c=Task{await m.refreshTokens()};for _ in 0..<100{await Task.yield()}
  let count=m.api.calls;m.api.finish(1,token("B","new"));if count>2{for i in 2..<count{m.api.finish(i,token("B","extra"))}}
  _ = await b.value;_ = await c.value
  return count==2 && m.tokens?.accessToken=="new"
 }
 @MainActor static func checkSnapshotAfterUnpair()async ->Bool{
  let m=AppModel();m.install(token("A"));let d=CoreChatDoc("private",pause:true);m.docs["room"]=d;m.requestRead("room")
  guard await spin({d.began!.wait(timeout:.now()) == .success})else{d.gate?.signal();return false}
  m.unpair();d.gate?.signal();for _ in 0..<100{await Task.yield()}
  return m.messages["room"]==nil
 }
 @MainActor static func checkMatrixSnapshotAfterStop()async ->Bool{
  MatrixSDK.reset();let m=AppModel();m.install(token("A"));let t=Task{await m.startMatrixIfNeeded()};guard await spin({MatrixSDK.waiters.count==1})else{return false}
  let old=ArbosFFI.MatrixClient("obsolete");old.paused=true;MatrixSDK.finish(0,old)
  guard await spin({old.snapshotWaiter != nil})else{return false}
  m.stopMatrix();old.snapshotWaiter?.resume(returning:ArbosFFI.MatrixSnapshot(spaces:[],channels:["obsolete"]));old.snapshotWaiter=nil;await t.value
  return m.snapshots.isEmpty && m.matrixClient==nil && old.stops==1
 }
 @MainActor static func checkDirectAccountReplacement()async ->Bool{
  let m=AppModel();m.install(token("A"));m.docs["private-A"]=CoreChatDoc("A");m.messages["private-A"]=[Message(id:"A")]
  m.install(token("B"))
  return m.credentials?.deviceId=="B" && m.docs.isEmpty && m.messages.isEmpty
 }
 @MainActor static func checkCurrentRefusalControl()async ->Bool{
  let m=AppModel();m.install(token("A"));let t=Task{await m.refreshTokens()};guard await spin({m.api.calls==1})else{return false}
  m.api.finish(0,failure:true);let result=await t.value
  return !result && m.credentials==nil && m.signInError != nil
 }
 @MainActor static func checkLogoutKeepsOriginalBearer()async ->Bool{
  FakeAPI.cleanupTokens=[];let m=AppModel();m.install(token("A"));m.api.token={ [weak m] in m?.credentials?.deviceToken ?? "" }
  m.signOut(remote:true);m.install(token("B"));guard await spin({FakeAPI.cleanupTokens.count==3})else{return false}
  return FakeAPI.cleanupTokens.allSatisfy{$0=="A"}
 }
 @MainActor static func checkSnapshotAfterWipe()async ->Bool{
  let m=AppModel();m.install(token("A"));let d=CoreChatDoc("old",pause:true);m.docs["room"]=d;m.requestRead("room")
  guard await spin({d.began!.wait(timeout:.now()) == .success})else{d.gate?.signal();return false}
  m.removeRoom("room");d.gate?.signal();for _ in 0..<100{await Task.yield()}
  return m.messages["room"]==nil
 }
 @MainActor static func checkSnapshotAfterReplace()async ->Bool{
  let m=AppModel();m.install(token("A"));let d=CoreChatDoc("old",pause:true);m.docs["room"]=d;m.requestRead("room")
  guard await spin({d.began!.wait(timeout:.now()) == .success})else{d.gate?.signal();return false}
  m.removeRoom("room");m.docs["room"]=CoreChatDoc("new");d.gate?.signal();for _ in 0..<100{await Task.yield()}
  return m.messages["room"]==nil
 }
 @MainActor static func checkSameDocFallback()async ->Bool{
  let m=AppModel();m.install(token("A"));let d=CoreChatDoc("same",pause:true);m.docs["room"]=d;m.requestRead("room")
  guard await spin({d.began!.wait(timeout:.now()) == .success})else{d.gate?.signal();return false}
  m.snapshotGeneration["room",default:0] += 1;d.gate?.signal();guard await spin({m.messages["room"] != nil})else{return false}
  return m.messages["room"]==[Message(id:"same")]
 }
 @MainActor static func checkMatrixStartAfterReplacement()async ->Bool{
  MatrixSDK.reset();let m=AppModel();m.install(token("A"));let t=Task{await m.startMatrixIfNeeded()};guard await spin({MatrixSDK.waiters.count==1})else{return false}
  m.signOut(remote:false);m.install(token("B"));let old=ArbosFFI.MatrixClient("A");MatrixSDK.finish(0,old);await t.value
  return m.matrixClient==nil && old.stops==1 && m.snapshots.isEmpty
 }
 @MainActor static func checkQueuedMatrixCallback()async ->Bool{
  MatrixSDK.reset();let m=AppModel();m.install(token("A"));let t=Task{await m.startMatrixIfNeeded()};guard await spin({MatrixSDK.waiters.count==1})else{return false}
  let listener=MatrixSDK.listeners[0];let old=ArbosFFI.MatrixClient("A");MatrixSDK.finish(0,old);await t.value;m.snapshots=[]
  listener.onRooms(snapshot:ArbosFFI.MatrixSnapshot(spaces:[],channels:["obsolete"]));listener.onConnection(connected:false,error:nil);listener.onTimeline(roomId:"old",rows:[])
  m.stopMatrix();for _ in 0..<100{await Task.yield()}
  return m.snapshots.isEmpty && m.connections.isEmpty && m.timelines==0
 }
 @MainActor static func checkMatrixCurrentControl()async ->Bool{
  MatrixSDK.reset();let m=AppModel();m.install(token("A"));let t=Task{await m.startMatrixIfNeeded()};guard await spin({MatrixSDK.waiters.count==1})else{return false}
  let current=ArbosFFI.MatrixClient("A");MatrixSDK.finish(0,current);await t.value
  MatrixSDK.listeners[0].onRooms(snapshot:ArbosFFI.MatrixSnapshot(spaces:[],channels:["current"]));guard await spin({m.snapshots.contains("current")})else{return false}
  return m.matrixClient===current && current.stops==0
 }
 @MainActor static func main()async{
  var results:[[String:Any]]=[]
  let cases:[(String,()async->Bool)]=[("refresh-after-signout",checkRefreshAfterSignOut),("stale-refusal",checkStaleRefusal),("single-flight-control",checkCurrentRefreshControl),("stale-single-flight-cleanup",checkStaleSingleFlightCleanup),("direct-account-replacement",checkDirectAccountReplacement),("current-refusal-control",checkCurrentRefusalControl),("logout-keeps-original-bearer",checkLogoutKeepsOriginalBearer),("snapshot-after-unpair",checkSnapshotAfterUnpair),("matrix-snapshot-after-stop",checkMatrixSnapshotAfterStop),("snapshot-after-wipe",checkSnapshotAfterWipe),("snapshot-after-replace",checkSnapshotAfterReplace),("same-doc-starvation-control",checkSameDocFallback),("matrix-start-after-replacement",checkMatrixStartAfterReplacement),("queued-matrix-callback",checkQueuedMatrixCallback),("current-matrix-control",checkMatrixCurrentControl)]
  for (name,run) in cases {results.append(["name":name,"pass":await run()])}
  let failed=results.filter{!($0["pass"] as! Bool)}.count
  let data=try! JSONSerialization.data(withJSONObject:["results":results,"passed":results.count-failed,"failed":failed,"total":results.count],options:[.sortedKeys])
  print(String(data:data,encoding:.utf8)!);if failed>0{exit(1)}
 }
}
'''

def build(source):
    app=(source/P/'Store/AppModel.swift').read_text()
    server=(source/P/'Store/ServerStore.swift').read_text()
    listener=server[server.index('final class MatrixBridgeListener:'):server.index('\nextension AppModel {')]
    methods=['adopt','refreshTokens','signOut','unpair','wipeRoom','emitSnapshot']
    prelude = PRELUDE
    helper = source/P/'Sync/WorkLifetime.swift'
    if helper.exists():
        start = app.index('    var accountLifetime = WorkLifetime()')
        end = app.index('\n    /// Outside agents', start)
        prelude = helper.read_text() + '\n' + prelude.replace('    var refreshTask: Task<Bool,Never>?', app[start:end]).replace('    var matrixStarting=false', '    var matrixLifetime = MatrixLifetime()')
        methods_extra = method(app, 'prepareCredentialOwner')+'\n'+method(app,'boundCallAPI')+'\n'+method(server, 'acceptsMatrix')
        extra = r"""
 @MainActor static func checkExpectedOwnerEntry()async ->Bool {
  let m=AppModel();m.install(token("A"));let owner=m.accountLifetime.ticket!;m.signOut(remote:false);m.install(token("B"))
  let result=await m.refreshTokens(expectedOwner:owner)
  return !result && m.api.calls==0 && m.credentials?.deviceId=="B"
 }
"""
        final_tests = TESTS.replace(' @MainActor static func main()async{', extra+' @MainActor static func main()async{').replace('("current-refusal-control",checkCurrentRefusalControl),','("current-refusal-control",checkCurrentRefusalControl),("expected-owner-entry",checkExpectedOwnerEntry),')

    else: methods_extra = ''; final_tests = TESTS
    return prelude+'\n'.join(method(app,n) for n in methods)+'\n'+method(server,'startMatrixIfNeeded')+'\n'+method(server,'stopMatrix')+'\n'+methods_extra+final_tests+'\n'+listener

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--source-root',type=Path,default=ROOT);ap.add_argument('--output',type=Path,required=True);args=ap.parse_args()
    source=args.source_root.resolve();output=args.output.resolve();output.mkdir(parents=True,exist_ok=False)
    inputs=[P+'Store/AppModel.swift',P+'Store/ServerStore.swift',P+'Sync/WorkLifetime.swift'];sources={p:hashlib.sha256((source/p).read_bytes()).hexdigest() for p in inputs}
    with tempfile.TemporaryDirectory(prefix='arbos-native-ownership-',dir='/tmp') as tmp:
        tmp=Path(tmp);swift=tmp/'main.swift';swift.write_text(build(source));binary=tmp/'checks'
        start=time.monotonic();cmd=['swiftc','-swift-version','5','-parse-as-library',str(swift),'-o',str(binary)]
        c=subprocess.run(cmd,env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)},text=True,capture_output=True,timeout=120);compile_s=time.monotonic()-start
        record={'sources':sources,'source':'checkout','compile_seconds':compile_s,'compile_exit':c.returncode,'extracted_fixture_sha256':hashlib.sha256(swift.read_bytes()).hexdigest()}
        (output/'compile.log').write_text(c.stdout+c.stderr)
        if c.returncode==0:
            start=time.monotonic();r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)},text=True,capture_output=True,timeout=30)
            record.update({'test_seconds':time.monotonic()-start,'exit_code':r.returncode,'results':json.loads(r.stdout)})
            (output/'tests.log').write_text(r.stdout+r.stderr)
        assert sources=={p:hashlib.sha256((source/p).read_bytes()).hexdigest() for p in inputs},'Sources changed during Swift gate'
        (output/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
        assert record.get('compile_exit')==0 and record.get('exit_code')==0 and record.get('results',{}).get('passed')==16, 'Controller checks failed or changed count'
if __name__=='__main__':main()
