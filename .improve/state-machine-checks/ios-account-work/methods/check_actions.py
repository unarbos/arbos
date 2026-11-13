"""Representative actual account mutation controllers and failure cleanup paths."""
from pathlib import Path
import argparse,hashlib,importlib.util,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
spec=importlib.util.spec_from_file_location('upload',HERE/'check_uploads.py');u=importlib.util.module_from_spec(spec);spec.loader.exec_module(u)
F=r'''
import Foundation
struct RelayError:Error{let status:Int;var type:String?=nil;var message=""}
struct RelayEndpoint{let httpUrl="http://synthetic.invalid"}
final class CallAuthority{var bearer:String;init(_ token:String){bearer=token}}
@MainActor final class Session{
 let label:String;var requests:[String]=[];var pause=false;var waiter:CheckedContinuation<Void,Error>?
 init(_ label:String){self.label=label}
 func request(_ name:String)async throws{requests.append(name);if pause{try await withCheckedThrowingContinuation{waiter=$0}}}
 func finish(_ failed:Bool=false){let w=waiter;waiter=nil;pause=false;if failed{w?.resume(throwing:RelayError(status:500))}else{w?.resume()}}
}
@MainActor final class RelayAPI{
 let endpoint=RelayEndpoint();let session:Session;var token:()->String;var refresher:(()async->Bool)?;var authorization:(@MainActor ()->String?)?;var cleanupAuthority:CallAuthority?
 init(_ s:Session){session=s;token={s.label}}
 private init(endpoint:RelayEndpoint,token:@escaping ()->String,session:Session){self.session=session;self.token=token}
 func request(_ name:String)async throws{if let authorization{guard let bearer=authorization()else{throw CancellationError()};try await session.request(bearer+":"+name)}else{try await session.request(token()+":"+name)}}
 func setModel(projectId:String,model:String,effort:Effort)async throws{try await request("model")}
 func addMember(chatId:String,handle:String)async throws->PeopleChat{try await request("member");return PeopleChat(id:chatId,marker:session.label)}
 func patchMe(name:String,login:String)async throws->AuthUser{try await request("profile");return AuthUser(id:session.label)}
 func startMachine()async throws{try await request("machine")}
 func chatSettings(chatId:String,muted:Bool)async throws{try await request("mute")}
'''
M=r'''
typealias Id=String
struct AuthUser{let id:String}
struct UserError{let sentence:String;static func classify(_ label:String,_ error:Error)->Self{Self(sentence:label)};static func refused(_ label:String,_ text:String)->Self{Self(sentence:text)}}
enum AuthError{static func humanize(_ type:String,_ message:String)->String{message}}
enum Effort{case medium}
struct Project{let id:String;var model:String;var effort:Effort}
struct PeopleChat{let id:String;let marker:String;var muted=false}
struct Machine{enum State{case waking,error};var state=State.waking;var since:Int64=0;func coerced(_ state:State)->State{state}}
final class Doc{func setModelSettings(model:String,effort:Effort)throws{};func commit(){}}
@MainActor final class AppModel{
 var credentials:Bool?=true;var accountLifetime=WorkLifetime();var api:RelayAPI;var isFixture=false
 var user=AuthUser(id:"initial");var needsProfileSetup=true;var cacheWrites=0;var machineBusy=false;var machine=Machine();var machineError:String?;var refreshed=0
 var projects=[Project(id:"same",model:"old",effort:.medium)];var peopleChats=[PeopleChat(id:"same",marker:"initial")];var docs:[String:Doc]=[:];var errors:[String]=[]
 init(_ s:Session){api=RelayAPI(s)}
 func replace(_ s:Session){accountLifetime.close();accountLifetime.reopen();api=RelayAPI(s);user=AuthUser(id:s.label);peopleChats=[PeopleChat(id:"same",marker:s.label)];machineBusy=true;machineError=nil;errors=[];cacheWrites=0;refreshed=0}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket?=nil)async->Bool{expectedOwner.map{accountLifetime.accepts($0)} ?? true}
 func refreshMachine(expectedOwner:WorkLifetime.Ticket?=nil)async{if expectedOwner.map({accountLifetime.accepts($0)}) ?? true{refreshed+=1}}
 func saveAccountCache(){cacheWrites+=1}
 func fail(_ label:String,_ error:Error){errors.append(label)};func fail(_ e:UserError){errors.append(e.sentence)}
 func projectDocKey(_ id:String)->String{id};func nowMs()->Int64{1}
'''
T=r'''
}
@main struct Checks{
 @MainActor static func spin(_ p:()->Bool)async->Bool{for _ in 0..<10000{if p(){return true};await Task.yield()};return p()}
 @MainActor static func main()async{
 var cases:[String:Bool]=[:]
 for kind in ["model","member","machine","mute"]{
  func send(_ m:AppModel){switch kind{case "model":m.setModel(projectId:"same",model:"new",effort:.medium);case "member":m.addPerson(chatId:"same",handle:"synthetic");case "machine":m.startMachine();default:m.setMuted(chatId:"same",muted:true)}}
  do{let a=Session("A");let m=AppModel(a);send(m);let b=Session("B");m.replace(b);for _ in 0..<100{await Task.yield()};cases[kind+"-queued-A-no-B-authority"]=b.requests.isEmpty && m.errors.isEmpty}
  for failed in [false,true]{let a=Session("A");a.pause=true;let m=AppModel(a);send(m);let began=await spin{a.waiter != nil};m.replace(Session("B"));a.finish(failed);for _ in 0..<100{await Task.yield()};cases[kind+(failed ? "-late-error":"-late-success")]=began && m.errors.isEmpty && m.peopleChats.first?.marker=="B" && m.machineBusy && m.refreshed==0}
  do{let a=Session("A");let m=AppModel(a);send(m);let began=await spin{!a.requests.isEmpty};for _ in 0..<100{await Task.yield()};cases[kind+"-current-control"]=began && a.requests==["A:"+kind] && m.errors.isEmpty && (kind != "member" || m.peopleChats.first?.marker=="A") && (kind != "machine" || !m.machineBusy && m.refreshed==1)}
 }
 for failed in [false,true]{let a=Session("A");a.pause=true;let m=AppModel(a);let work=m.accountWork();let old=Task{await m.updateProfile(name:" A ",handle:" @synthetic "PROFILE)};let began=await spin{a.waiter != nil};m.replace(Session("B"));a.finish(failed);let result=await old.value;cases[failed ? "profile-late-error-not-returned":"profile-late-success-no-B-paint"]=began && result==nil && m.user.id=="B" && m.cacheWrites==0}
 do{let a=Session("A");let m=AppModel(a);let work=m.accountWork();let result=await m.updateProfile(name:" A ",handle:" @synthetic "PROFILE);cases["profile-current-control"]=result==nil && m.user.id=="A" && m.cacheWrites==1 && !m.needsProfileSetup}
 let failed=cases.values.filter{!$0}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":cases.count-failed,"failed":failed,"total":cases.count,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--tree',type=Path,required=True);ap.add_argument('--out',type=Path,required=True);a=ap.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(exist_ok=False)
 paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];app,api,helper=[p.read_text()for p in paths];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};fixture=F+u.method(api,'bound')+'\n}\n'+helper
 if 'struct AccountWork' in app:
  k=re.search(r'^struct AccountWork \{',app,re.M);fixture+='\n'+app[k.start():app.index('\n}',k.start())+2]
 else:fixture+='\nstruct AccountWork{let owner:WorkLifetime.Ticket;let api:RelayAPI}\n'
 fixture+=M
 if 'struct AccountWork' not in app:fixture+=' func accountWork()->AccountWork?{AccountWork(owner:accountLifetime.ticket!,api:api)}\n'
 for name in ['accountWork','boundCallAPI','setModel','addPerson','startMachine','setMuted','updateProfile']:
  if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=u.method(app,name)+'\n'
 fixture+=T.replace('PROFILE',',expectedOwner:work!.owner' if re.search(r'func updateProfile\([^\n]*expectedOwner',app)else'');start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-ios-actions-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'sources':sources,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  record['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
