// Synthetic URLProtocol and effect collaborators only.

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
