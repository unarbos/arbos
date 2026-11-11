"""Actual raw upload/read/transcription methods + bound API over intercept-all URLProtocol."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
def block(s,marker):
 a=s.index(marker);return s[a:s.index('\n}',a)+2]
def method(s,name):
 m=re.search(r'^    (?:private )?func '+name+r'\(',s,re.M);assert m,name;a=m.start();a=a-len('    @MainActor\n')if s[:a].endswith('    @MainActor\n')else a;return s[a:s.index('\n    }',m.end())+6]
F=r'''
typealias Id=String
enum PictureMaster{static func mime(_ data:Data)->String{"image/png"}}
struct Hit{let method:String;let path:String;let bearer:String;let mime:String;let body:Data;let timeout:Double}
final class Wire:URLProtocol,@unchecked Sendable{
 static let lock=NSLock();static var hits:[Hit]=[];static var status=200
 static func reset(_ value:Int=200){lock.lock();status=value;hits=[];lock.unlock()}
 static func read()->[Hit]{lock.lock();defer{lock.unlock()};return hits}
 override class func canInit(with request:URLRequest)->Bool{true}
 override class func canonicalRequest(for request:URLRequest)->URLRequest{request}
 override func startLoading(){
  var body=request.httpBody ?? Data()
  if body.isEmpty,let stream=request.httpBodyStream{stream.open();defer{stream.close()};var bytes=[UInt8](repeating:0,count:4096);while true{let n=stream.read(&bytes,maxLength:bytes.count);if n<=0{break};body.append(contentsOf:bytes.prefix(n));if body.count>65536{fatalError("Synthetic request exceeds test cap")}}}
  Self.lock.lock();let status=Self.status;Self.hits.append(Hit(method:request.httpMethod ?? "GET",path:request.url!.path,bearer:request.value(forHTTPHeaderField:"Authorization") ?? "",mime:request.value(forHTTPHeaderField:"Content-Type") ?? "",body:body,timeout:request.timeoutInterval));Self.lock.unlock()
  let response=HTTPURLResponse(url:request.url!,statusCode:status,httpVersion:nil,headerFields:nil)!
  let payload:Data
  if request.url!.path.contains("transcriptions"){payload=Data("{\"text\":\" synthetic transcript \"}".utf8)}
  else if request.url!.path=="/me/avatar"{payload=Data("{\"user\":{\"id\":\"synthetic\"},\"avatar_url\":\"http://synthetic.invalid/avatar\"}".utf8)}
  else{payload=Data([1,2,3])}
  client?.urlProtocol(self,didReceive:response,cacheStoragePolicy:.notAllowed);client?.urlProtocol(self,didLoad:payload);client?.urlProtocolDidFinishLoading(self)
 }
 override func stopLoading(){}
}
@MainActor final class Box{var token="A";var current=true;var refreshes=0}
final class RelayAPI{
 let endpoint=RelayEndpoint("http://synthetic.invalid");var token:()->String;var refresher:(()async->Bool)?;private let session:URLSession;private var authorization:(@MainActor ()->String?)?;private var cleanupAuthority:CallAuthority?
 init(_ session:URLSession,token:@escaping()->String){self.session=session;self.token=token}
 private init(endpoint:RelayEndpoint,token:@escaping()->String,session:URLSession){self.session=session;self.token=token}
'''
T=r'''
}
@main struct Checks{
 static let bytes=Data([1,2,3])
 static func invoke(_ api:RelayAPI,_ kind:String)async throws->Bool{
  switch kind{
   case "put":return try await api.putBlob(projectId:"synthetic",key:"key",mime:"application/test",data:bytes)=="key"
   case "avatar":return try await api.putMeAvatar(jpeg:bytes)=="http://synthetic.invalid/avatar"
   case "transcribe":return try await api.transcribe(projectId:"synthetic",data:bytes,mime:"audio/test")=="synthetic transcript"
   default:return try await api.blobData(projectId:"synthetic",key:"key")==bytes
  }
 }
 static func metadata(_ hit:Hit,_ kind:String)->Bool{
  switch kind{
   case "put":return hit.method=="PUT" && hit.path=="/blob/synthetic/key" && hit.body==bytes && hit.mime=="application/test"
   case "avatar":return hit.method=="PUT" && hit.path=="/me/avatar" && hit.body==bytes && hit.mime=="image/png"
   case "transcribe":return hit.method=="POST" && hit.path=="/proxy/v1/audio/transcriptions" && hit.mime.hasPrefix("multipart/form-data; boundary=arbos-") && hit.timeout==60 && hit.body.range(of:Data("gpt-4o-mini-transcribe".utf8)) != nil && hit.body.range(of:bytes) != nil
   default:return hit.method=="GET" && hit.path=="/blob/synthetic/key" && hit.body.isEmpty
  }
 }
 @MainActor static func main()async{
  var cases:[String:Bool]=[:]
  for kind in ["put","avatar","transcribe","read"]{
   let config=URLSessionConfiguration.ephemeral;config.protocolClasses=[Wire.self];let session=URLSession(configuration:config);defer{session.invalidateAndCancel()};let box=Box();let api=RelayAPI(session,token:{box.token});let old=api.bound(to:{box.current})
   box.current=false;box.token="B";Wire.reset();do{_ = try await invoke(old,kind)}catch{};cases[kind+"-retired-bound-no-B-dispatch"]=Wire.read().isEmpty
   box.current=true;box.token="A2";Wire.reset();var success=false;do{success=try await invoke(old,kind)}catch{};let hits=Wire.read();cases[kind+"-current-authority-bytes-mime-policy"]=success && hits.count==1 && hits[0].bearer=="Bearer A2" && metadata(hits[0],kind)
   Wire.reset();do{success=try await invoke(api,kind)}catch{success=false};cases[kind+"-unbound-compatible-control"]=success && Wire.read().count==1 && Wire.read()[0].bearer=="Bearer A2"
   box.refreshes=0;old.refresher={box.refreshes+=1;return true};Wire.reset(401);var status=0;do{_ = try await invoke(old,kind)}catch let e as RelayError{status=e.status}catch{};cases[kind+"-no-new-401-retry"]=status==401 && Wire.read().count==1 && box.refreshes==0
  }
  let failed=cases.values.filter{!$0}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":cases.count-failed,"failed":failed,"total":cases.count,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--dto',type=Path);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();a.dto=a.dto or tree/'packages/ArbosFFI/Sources/ArbosFFI/RelayTypes.swift';out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);path=tree/(P+'Sync/RelayAPI.swift');s=path.read_text();dto=a.dto.read_text();hashes={P+'Sync/RelayAPI.swift':hashlib.sha256(path.read_bytes()).hexdigest(),'RelayTypes.swift':hashlib.sha256(a.dto.read_bytes()).hexdigest()}
 fixture='import Foundation\n'+block(s,'struct RelayError:')+'\n'+block(s,'struct RelayEndpoint:')+'\n'+s[s.index('@MainActor\nprivate final class CallAuthority {'):s.index('\n}',s.index('@MainActor\nprivate final class CallAuthority {'))+2]+'\n'
 for name in ['AccountUserDto','ApiErrorBodyDto','ApiErrorDto','MeUserResponseDto','TranscriptionDto']:fixture+=block(dto,'public struct '+name+':')+'\n'
 fixture+=F+method(s,'bound')+'\n'
 if 'func requestBearer('in s:fixture+=method(s,'requestBearer')+'\n'
 fixture+='\n'.join(method(s,name)for name in ['putBlob','putMeAvatar','transcribe','blobData'])+T;tick=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-raw-api-')as raw:
  t=Path(raw);(t/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(t/'main.swift'),'-o',str(t/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);row={'sources':hashes,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(t/'check')],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout)if r.stdout else{});(out/'tests.log').write_text(r.stdout+r.stderr)
  row['seconds']=time.monotonic()-tick;assert s==path.read_text()and dto==a.dto.read_text();(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==16;exit(row['results']['failed']!=0)
if __name__=='__main__':main()
