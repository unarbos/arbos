"""Actual socket callback/lifetime bodies with a local socket/core-action observer."""
from pathlib import Path
import argparse,hashlib,json,os,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;ROOT=HERE.parents[2]
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,default=ROOT);p.add_argument('--output',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
path=tree/'apps/ios/Arbos/Arbos/Sync/SessionRoom.swift';original=path.read_bytes();s=original.decode()
def method(name):
 matches=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',s,re.M));assert len(matches)==1,(name,'Expected one actual method');start=matches[0].start();return s[start:s.index('\n    }',start)+6]
a=s.index('protocol RoomSocket:');protocol=s[a:s.index('\n}',a)+2]
fixture=r'''
import Foundation
typealias Id=String
enum LinkState {case syncing,live,reconnecting,relayDown;static func fault(_:Error)->Self{.reconnecting}}
func roomName(_ a:String,_ b:String)->String{"\(a)/\(b)"}
@MainActor final class ObservedCore {
 var opens=0;var closes=0;var frames=0;var ticks=0
 func onOpen(nowMs:UInt64){opens+=1};func onClose(nowMs:UInt64){closes+=1};func onFrame(bytes:Data){frames+=1};func onTick(nowMs:UInt64){ticks+=1};func isLive()->Bool{false}
}
'''+protocol+r'''
@MainActor final class FakeSocket:RoomSocket {
 var onOpen:(()->Void)?;var httpStatus:Int?;var closeCode:Int?;var waiter:CheckedContinuation<Data,Error>?;var resumes=0;var cancellations=0
 func resume(){resumes+=1};func send(_:Data)async throws{}
 func receive()async throws->Data{try await withCheckedThrowingContinuation{waiter=$0}}
 func cancel(){cancellations+=1;let saved=waiter;waiter=nil;saved?.resume(throwing:URLError(.cancelled))}
 func fail(_ status:Int){httpStatus=status;let saved=waiter;waiter=nil;saved?.resume(throwing:URLError(.badServerResponse))}
}
@MainActor final class SessionRoom {
 let projectId="synthetic-project";let roomChat="synthetic-chat";let session=ObservedCore()
 var socket:RoomSocket?;var generation=0;var started=false;var stopped=false;var sawOpen=false;var connectStartedAt:Date?;var pendingFault:LinkState?;var cleanClose=false;var lastError:String?
 var receiveTask:Task<Void,Never>?;var tickTask:Task<Void,Never>?;var presenceTask:Task<Void,Never>?;var reconnectTask:Task<Void,Never>?;var checkpointTask:Task<Void,Never>?;var sendChain:Task<Void,Never>?;var urlSession:URLSession?
 var onLinkState:((LinkState)->Void)?;var onUnavailable:(()->Void)?;var presence=0;var drains=0;var sockets:[FakeSocket]=[]
 static let helloDeadlineNs:UInt64=15_000_000_000;static let tickEveryNs:UInt64=1_000_000_000
 func roomURL(_:String)->URL?{URL(string:"wss://synthetic.invalid/local-only")}
 func makeSocket(_:URL)->RoomSocket{let value=FakeSocket();sockets.append(value);return value}
 func sendPresence(){presence+=1};func drain(){drains+=1}
'''+ '\n'.join(method(n) for n in ['start','stop','connect','handleOpen','startTick','startReceive','cancelTasks','sessionNowMs'])+'\n'+re.search(r'^    static func isRestartClose[^\n]+',s,re.M)[0]+r'''
 func reconnectForCheck(){connect()}
}
@main struct Checks {
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func settle()async{for _ in 0..<100{await Task.yield()}}
 @MainActor static func main()async{
  var results:[[String:Any]]=[]
  func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  for status in [403,404]{
   let room=SessionRoom();var unavailable=0;room.onUnavailable={unavailable+=1};room.start();let socket=room.sockets[0];let callback=socket.onOpen!;let receiving=await spin{socket.waiter != nil};socket.fail(status);let terminal=await spin{room.lastError != nil};callback();await settle();room.start();record("terminal-\(status)-rejects-queued-open",receiving && terminal && unavailable==1 && room.session.opens==0 && room.presence==0 && room.presenceTask==nil && room.sockets.count==1);room.stop()
  }
  do{let room=SessionRoom();room.start();let socket=room.sockets[0];socket.onOpen?();let opened=await spin{room.session.opens==1};record("current-open-still-reaches-core-and-presence",opened && room.presence==1 && room.presenceTask != nil);room.stop()}
  do{let room=SessionRoom();room.start();let callback=room.sockets[0].onOpen!;room.stop();callback();await settle();record("explicit-stop-rejects-queued-open",room.session.opens==0 && room.presence==0)}
  do{let room=SessionRoom();room.start();let callback=room.sockets[0].onOpen!;room.reconnectForCheck();callback();await settle();let newer=room.sockets[1];newer.onOpen?();let opened=await spin{room.session.opens==1};record("reconnect-rejects-old-and-accepts-current-open",opened && room.presence==1);room.stop()}
  let failed=results.filter{!($0["pass"]as!Bool)}.count
  let value:[String:Any]=["total":results.count,"passed":results.count-failed,"failed":failed,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
t=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-room-callback-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'source_sha256':hashlib.sha256(original).hexdigest(),'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-t;assert path.read_bytes()==original,'Source changed during check';(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==5 and row['results']['failed']==0,'Expected5 terminal/current callback checks'
