"""Actual send branches+raw PUT via intercept-all URLProtocol, no real HTTP."""
from pathlib import Path
import argparse,hashlib,importlib.util,json,re,subprocess,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
import support as u
import fixture
PROTOCOL=r'''
@MainActor final class WeakControl{weak var value:Session?;init(_ value:Session){self.value=value}}
final class LocalProtocol:URLProtocol,@unchecked Sendable{
 private let lock=NSLock();private var pending:Task<Void,Never>?;private var stopped=false
 override class func canInit(with request:URLRequest)->Bool{true}
 override class func canonicalRequest(for request:URLRequest)->URLRequest{request}
 override func startLoading(){
  let operation=Task{ @MainActor [self] in
   guard let id=request.value(forHTTPHeaderField:"X-Synthetic-Control"),let s=Session.controls[id]?.value else{client?.urlProtocol(self,didFailWithError:URLError(.badServerResponse));return}
   var body=request.httpBody ?? Data()
   if body.isEmpty,let stream=request.httpBodyStream{stream.open();defer{stream.close()};var bytes=[UInt8](repeating:0,count:65536);while true{let n=stream.read(&bytes,maxLength:bytes.count);if n<=0{break};body.append(contentsOf:bytes.prefix(n));if body.count>32*1024*1024{fatalError("Synthetic body exceeds cap")}}}
   s.authorizations.append(request.value(forHTTPHeaderField:"Authorization") ?? "")
   var status=200
   do{_ = try await s.put(request.url!.lastPathComponent,request.value(forHTTPHeaderField:"Content-Type") ?? "",body)}catch{status=500}
   guard !isStopped else{return}
   let reply=HTTPURLResponse(url:request.url!,statusCode:status,httpVersion:nil,headerFields:nil)!
   client?.urlProtocol(self,didReceive:reply,cacheStoragePolicy:.notAllowed);client?.urlProtocol(self,didLoad:Data());client?.urlProtocolDidFinishLoading(self)
  }
  lock.lock();pending=operation;let cancel=stopped;lock.unlock();if cancel{operation.cancel()}
 }
 var isStopped:Bool{lock.lock();defer{lock.unlock()};return stopped}
 override func stopLoading(){lock.lock();stopped=true;let operation=pending;pending=nil;lock.unlock();operation?.cancel()}
}
'''
API=r'''
final class RelayAPI{
 let endpoint=RelayEndpoint();var token:()->String;var refresher:(()async->Bool)?;private let session:URLSession
 private var authorization:(@MainActor ()->String?)?;private var cleanupAuthority:CallAuthority?
 @MainActor init(_ control:Session){session=control.transport;let label=control.label;token={label}}
 private init(endpoint:RelayEndpoint,token:@escaping()->String,session:URLSession){self.session=session;self.token=token}
'''
BENCH=r'''
}
@main struct Profile{
 @MainActor static func main()async{
  let size=Int(CommandLine.arguments[1])!;let people=CommandLine.arguments[2]=="people"
  let items=(0..<4).map{PendingAttachment(name:String($0),mime:"application/test",data:Data(repeating:UInt8($0+1),count:size))}
  let a=Session("A");a.delay=8_000_000;let m=AppModel(a)
  let tick=DispatchTime.now().uptimeNanoseconds
  await withCheckedContinuation{(done:CheckedContinuation<Void,Never>)in
   m.onCommit={done.resume()}
   if people{m.peopleUpload(attachments:items)}else{m.send(chatId:"chat",text:"synthetic",attachments:items,replyTo:"reply",forwardedFrom:"forward",extraParts:[["kind":"chart"]])}
  }
  let elapsed=Double(DispatchTime.now().uptimeNanoseconds-tick)/1_000_000
  let encoded=Dictionary(uniqueKeysWithValues:zip(a.names,a.bytes));let mimes=Dictionary(uniqueKeysWithValues:zip(a.names,a.mimes));let expected=Dictionary(uniqueKeysWithValues:items.map{($0.name,$0.data)})
  let c=m.commits.first!;let valid=m.commits.count==1 && c.attachments.map(\.name)==["0","1","2","3"] && c.attachments.allSatisfy{$0.blobRef==$0.name} && encoded==expected && mimes.values.allSatisfy{$0=="application/test"} && a.authorizations==Array(repeating:"Bearer A",count:4) && a.active==0
  let result:[String:Any]=["milliseconds":elapsed,"bytes_per_attachment":size,"peak":a.peak,"active_after":a.active,"put_requests":a.names.count,"valid":valid,"people":people]
  print(String(data:try! JSONSerialization.data(withJSONObject:result,options:[.sortedKeys]),encoding:.utf8)!);exit(valid ? 0:1)
 }
}
'''

def prepare(tree,api_path,out,baseline=False):
 ctx=fixture.context(tree,baseline,api_path);app=ctx['app'];api=ctx['api'];hashes=ctx['hashes']
 f=u.F[:u.F.index('@MainActor final class RelayAPI{')]
 f=f.replace('struct RelayError:Error{let status:Int}','struct RelayError:Error{let status:Int;var message:String="";static func from(status:Int,body:Data)->Self{Self(status:status)}}')
 f=f.replace(' let label:String;',r''' static var controls:[String:WeakControl]=[:]
 let wireId=UUID().uuidString
 lazy var transport:URLSession={
  let config=URLSessionConfiguration.ephemeral;config.protocolClasses=[LocalProtocol.self];config.httpAdditionalHeaders=["X-Synthetic-Control":wireId]
  Self.controls[wireId]=WeakControl(self);return URLSession(configuration:config)
 }()
 let label:String;''')
 text=f+PROTOCOL+API
 if 'func bound(' in api:text+='@MainActor\n'+u.method(api,'bound')+'\n'
 if 'func requestBearer('in api:text+=u.method(api,'requestBearer')+'\n'
 text+=u.method(api,'putBlob')+'\n}\n'+fixture.declarations(ctx)+fixture.model(ctx,capture_commit=True)+fixture.branches(ctx)+BENCH
 fixture_text=text
 (out/'fixture.swift').write_text(fixture_text);binary=out/'upload-profile';env={'PATH':'/usr/bin:/bin','HOME':str(out),'TMPDIR':str(out)};tick=time.monotonic();c=subprocess.run(['swiftc','-O','-swift-version','5','-parse-as-library',str(out/'fixture.swift'),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=180);(out/'compile.log').write_text(c.stdout+c.stderr);value={'sources':hashes,'compile_exit':c.returncode,'seconds':time.monotonic()-tick,'fixture_sha256':hashlib.sha256(fixture_text.encode()).hexdigest()};(out/'GATES.json').write_text(json.dumps(value,indent=2)+'\n');print(json.dumps(value),flush=True);assert c.returncode==0;assert fixture.stable(ctx)
 for size,kind in [(64,'people'),(1024*1024,'project')]:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(binary),str(size),kind],env=env,text=True,capture_output=True,timeout=30);(out/(kind+'.log')).write_text(r.stdout+r.stderr);print(r.stdout.strip(),flush=True);assert r.returncode==0 and json.loads(r.stdout)['valid']

def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--api',type=Path);p.add_argument('--out',type=Path,required=True);p.add_argument('--baseline',action='store_true');a=p.parse_args();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);prepare(a.tree.resolve(),a.api.resolve()if a.api else None,out,a.baseline)
if __name__=='__main__':main()
