"""Actual announcement and two-await private reveal completion edges."""
from pathlib import Path
import argparse,hashlib,importlib.util,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
spec=importlib.util.spec_from_file_location('a',HERE/'check_actions.py');a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a);u=a.u
F=a.F.replace(' func setModel(',''' func notifyMessage(chatId:String,projectId:String?,messageId:String,text:String,replyRef:ReplyRef?)async throws{try await request("notify")}
 func connectAgent(name:String,transport:String)async throws->ConnectReveal{try await request("connect");return ConnectReveal(projectId:session.label,info:OutsideAgentInfo(marker:session.label))}
 func setModel(''')
E=r'''
struct ReplyRef{}
struct OutsideAgentInfo{let marker:String}
struct ConnectReveal{let projectId:String;let info:OutsideAgentInfo?}
'''
C=r'''
 for kind in ["current","stale-entry","late-refresh","late-error"]{
  let a=Session("A");let model=AppModel(a);let work=model.accountWork()
  if kind=="late-refresh"{model.pauseRegistry=true}
  if kind=="late-error"{a.pause=true}
  if kind=="stale-entry"{let b=Session("B");model.replace(b);let value=await model.connectAgent(name:"synthetic",transport:"synthetic"OWNER);cases["stale-connect-entry-refuses-B"]=value==nil && b.requests.isEmpty;continue}
  let old=Task{await model.connectAgent(name:"synthetic",transport:"synthetic"OWNER)}
  if kind=="late-refresh"{let began=await spin{model.registryWaiter != nil};model.replace(Session("B"));model.registryWaiter?.resume();model.registryWaiter=nil;let value=await old.value;cases["reveal-retired-during-refresh-never-returned"]=began && value==nil}
  else if kind=="late-error"{let began=await spin{a.waiter != nil};model.replace(Session("B"));a.finish(true);let value=await old.value;cases["late-connect-error-no-B-notice"]=began && value==nil && model.errors.isEmpty}
  else{let value=await old.value;cases["current-reveal-control"]=value?.projectId=="A" && model.outsideInfo["A"]?.marker=="A" && model.registryCalls==1}
 }
'''
A=r'''
 do{let a=Session("A");let model=AppModel(a);model.triggerAnnounce();model.replace(Session("B"));for _ in 0..<100{await Task.yield()};cases["retired-queued-announcement-no-effect"]=a.requests.isEmpty && model.api.session.requests.isEmpty}
 do{let a=Session("A");let model=AppModel(a);model.triggerAnnounce();let done=await spin{!a.requests.isEmpty};cases["current-announcement-control"]=done && a.requests==["A:notify"]}
 do{let a=Session("A");let model=AppModel(a);model.isFixture=true;model.triggerAnnounce();for _ in 0..<100{await Task.yield()};cases["fixture-announcement-no-network-control"]=a.requests.isEmpty}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--tree',type=Path,required=True);ap.add_argument('--stage',choices=['connect','announce','all'],required=True);ap.add_argument('--out',type=Path,required=True);arg=ap.parse_args();tree=arg.tree.resolve();out=arg.out.resolve();out.mkdir(exist_ok=False)
 paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];app,api,helper=[p.read_text()for p in paths];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};fixture=F+u.method(api,'bound')+'\n}\n'+helper
 if 'struct AccountWork' in app:
  k=re.search(r'^struct AccountWork \{',app,re.M);fixture+='\n'+app[k.start():app.index('\n}',k.start())+2]
 else:fixture+='\nstruct AccountWork{let owner:WorkLifetime.Ticket;let api:RelayAPI}\n'
 fixture+='\n'+E+a.M.replace('var user=','var outsideInfo:[String:OutsideAgentInfo]=[:];var pauseRegistry=false;var registryWaiter:CheckedContinuation<Void,Never>?;var registryCalls=0\n var user=').replace('user=AuthUser(id:s.label);','user=AuthUser(id:s.label);outsideInfo=[:];registryCalls=0;')
 fixture+='func refreshRegistry(expectedOwner:WorkLifetime.Ticket?=nil)async{if let expectedOwner,!accountLifetime.accepts(expectedOwner){return};registryCalls+=1;if pauseRegistry{await withCheckedContinuation{registryWaiter=$0}}}\n'
 if 'struct AccountWork' not in app:fixture+=' func accountWork()->AccountWork?{AccountWork(owner:accountLifetime.ticket!,api:api)}\n'
 for name in ['accountWork','boundCallAPI']:
  if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=u.method(app,name)+'\n'
 if arg.stage in ['connect','all']:fixture+=u.method(app,'connectAgent')+'\n'
 if arg.stage in ['announce','all']:fixture+=u.method(app,'announce')+'\n func triggerAnnounce(){announce(chatId:"same",projectId:"project",messageId:"synthetic",text:"synthetic",replyRef:nil)}\n'
 fixture+='\n}\n@main struct Checks{\n @MainActor static func spin(_ p:()->Bool)async->Bool{for _ in 0..<10000{if p(){return true};await Task.yield()};return p()}\n @MainActor static func main()async{var cases:[String:Bool]=[:]\n'
 if arg.stage in ['connect','all']:fixture+=C.replace('OWNER',',expectedOwner:work?.owner'if re.search(r'func connectAgent\([^\n]*expectedOwner',app)else'')
 if arg.stage in ['announce','all']:fixture+=A
 fixture+='''let failed=cases.values.filter{!$0}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":cases.count-failed,"failed":failed,"total":cases.count,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)\n}\n}\n''';start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-ios-final-edges-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'sources':sources,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  record['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
