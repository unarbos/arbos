"""Actual RelayAPI request/binding bodies over an intercept-all URLProtocol."""
from pathlib import Path
import argparse,hashlib,json,os,re,subprocess,tempfile,time
ROOT=Path(__file__).resolve().parent
from support import repository, dependency_version, fence, record
REPO=repository(ROOT);SOURCE=Path('apps/ios/Arbos/Arbos/Sync/RelayAPI.swift')
def method(s,name):
 a=list(re.finditer(r'^    (?:private )?func '+name+r'\(',s,re.M));assert len(a)==1;return s[(a[0].start()-len('    @MainActor\n') if s[:a[0].start()].endswith('    @MainActor\n') else a[0].start()):s.index('\n    }',a[0].end())+6]
PRELUDE=r'''
import Foundation
struct RelayEndpoint{var httpUrl="http://synthetic.invalid"}
struct RelayError:Error{let status:Int;let message:String;var isUnauthorized:Bool{status==401};static func from(status:Int,body:Data)->Self{Self(status:status,message:"synthetic")}}
final class Box:@unchecked Sendable{let lock=NSLock();var rawToken="A";var rawCurrent=true;var token:String{get{lock.lock();defer{lock.unlock()};return rawToken}set{lock.lock();rawToken=newValue;lock.unlock()}};var current:Bool{get{lock.lock();defer{lock.unlock()};return rawCurrent}set{lock.lock();rawCurrent=newValue;lock.unlock()}}}
final class FakeProtocol:URLProtocol,@unchecked Sendable{
 static let lock=NSLock();static var statuses:[Int]=[];static var bearers:[String]=[]
 static func reset(_ values:[Int]){lock.lock();statuses=values;bearers=[];lock.unlock()}
 static func tokens()->[String]{lock.lock();defer{lock.unlock()};return bearers}
 override class func canInit(with request:URLRequest)->Bool{true};override class func canonicalRequest(for request:URLRequest)->URLRequest{request}
 override func startLoading(){Self.lock.lock();let status=Self.statuses.isEmpty ? 200:Self.statuses.removeFirst();Self.bearers.append(request.value(forHTTPHeaderField:"Authorization") ?? "");Self.lock.unlock();let response=HTTPURLResponse(url:request.url!,statusCode:status,httpVersion:nil,headerFields:nil)!;client?.urlProtocol(self,didReceive:response,cacheStoragePolicy:.notAllowed);client?.urlProtocol(self,didLoad:Data("{}".utf8));client?.urlProtocolDidFinishLoading(self)}
 override func stopLoading(){}
}
final class RelayAPI{
 let endpoint:RelayEndpoint;var token:()->String;var refresher:(()async->Bool)?;private let session:URLSession;private var authorization:(@MainActor ()->String?)?;private var cleanupAuthority:CallAuthority?
 init(_ session:URLSession,_ token:@escaping()->String){self.endpoint=RelayEndpoint();self.session=session;self.token=token}
 static var clientHeader:String{"synthetic"}
'''
TESTS=r'''
}
@MainActor final class Gate{var waiter:CheckedContinuation<Bool,Never>?;func wait()async->Bool{await withCheckedContinuation{waiter=$0}};func finish(){waiter?.resume(returning:true);waiter=nil}}
@main struct Checks{
 @MainActor static func spin(_ f:()->Bool)async->Bool{for _ in 0..<10000{if f(){return true};await Task.yield()};return f()}
 static func api(_ b:Box)->RelayAPI{let c=URLSessionConfiguration.ephemeral;c.protocolClasses=[FakeProtocol.self];return RelayAPI(URLSession(configuration:c),{b.token})}
 @MainActor static func main()async{
  var failed=0
  let b=Box();let a=api(b);a.refresher={b.token="A2";return true};FakeProtocol.reset([401,200]);let bound=BIND_A;let result=try? await bound.request("POST","/calls/current/leave");var ok=result != nil && FakeProtocol.tokens()==["Bearer A","Bearer A2"];print("\(ok ? "PASS":"FAIL") same-account refresh control");if !ok{failed+=1}
  b.token="A";b.current=true;let g=Gate();a.refresher={await g.wait()};FakeProtocol.reset([401,200]);let old=BIND_A;let task=Task{try? await old.request("POST","/calls/old/leave")};if await spin({g.waiter != nil}){b.current=false;b.token="B";g.finish()};_ = await task.value;ok=FakeProtocol.tokens()==["Bearer A"];print("\(ok ? "PASS":"FAIL") old refresh cannot retry as replacement");if !ok{failed+=1}
  b.current=true;b.token="A";FakeProtocol.reset([200]);let frozen=FROZEN_A;b.current=false;b.token="B";_ = try? await frozen.request("POST","/calls/old/leave");ok=FakeProtocol.tokens()==["Bearer A"];print("\(ok ? "PASS":"FAIL") captured cleanup freezes original bearer");if !ok{failed+=1}
  b.current=true;b.token="A";let blocked=BIND_A;b.current=false;b.token="B";FakeProtocol.reset([200]);_ = try? await blocked.request("POST","/calls/old/join");ok=FakeProtocol.tokens().isEmpty;print("\(ok ? "PASS":"FAIL") invalid account cannot begin request");if !ok{failed+=1}
  b.current=true;b.token="A";a.refresher={b.token="A2";return true};let owned=BIND_A;let refreshedCleanup=FROZEN_OWNED;FakeProtocol.reset([401,200]);_ = try? await owned.request("POST","/calls/old/join");b.current=false;b.token="B";FakeProtocol.reset([200]);_ = try? await refreshedCleanup.request("POST","/calls/old/leave");ok=FakeProtocol.tokens()==["Bearer A2"];print("\(ok ? "PASS":"FAIL") refreshed original bearer survives retired-account cleanup");if !ok{failed+=1}
  print("RESULT tests=5 failed=\(failed)");exit(failed==0 ? 0:1)
 }
}
'''
def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,default=REPO);p.add_argument('--label',default='api');p.add_argument('--record-dir',type=Path);args=p.parse_args();source=(args.tree/SOURCE).read_text();after='func frozenAuthority()' in source
 methods=(['requestBearer'] if 'func requestBearer(' in source else [])+['request','requestOnce'];extra=''
 if after:
  methods+=['bound','frozenAuthority'];at=source.index('    private init(endpoint:');extra=source[at:source.index('\n    }',at)+6]
 capture=''
 if 'private final class CallAuthority {' in source:
  at=source.index('@MainActor\nprivate final class CallAuthority {');capture=source[at:source.index('\n}',at)+2]+'\n'
 else:capture='@MainActor private final class CallAuthority {}\n'
 fixture=capture+PRELUDE+extra+'\n'+'\n'.join(method(source,n)for n in methods)+TESTS
 fixture=fixture.replace('BIND_A','a.bound(to: {b.current})' if after else 'a').replace('FROZEN_A','a.frozenAuthority()' if after else 'a').replace('FROZEN_OWNED','owned.frozenAuthority()' if after else 'owned')
 start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-call-api-')as temp:
  temp=Path(temp);(temp/'main.swift').write_text(fixture);(temp/'home').mkdir();env={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(temp/'home'),'TMPDIR':str(temp)}
  compile=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(temp/'main.swift'),'-o',str(temp/'check')],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=30)
  if compile.returncode:output=compile.stdout;code=compile.returncode
  else:
   p=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* (subpath "/Users/Shared"))',str(temp/'check')],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=15);output=p.stdout;code=p.returncode
 result={'exit_code':code,'compile_exit':compile.returncode,'wall_seconds':time.monotonic()-start,'source_sha256':hashlib.sha256(source.encode()).hexdigest(),'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'output':output}
 record(args.record_dir,args.label,result,output);print(json.dumps(result));return code
if __name__=='__main__':raise SystemExit(main())
