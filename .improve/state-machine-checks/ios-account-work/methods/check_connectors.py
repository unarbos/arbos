"""Actual provider flow with fake auth sheet/browser; no OAuth/browser/network."""
from pathlib import Path
import argparse,hashlib,importlib.util,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
spec=importlib.util.spec_from_file_location('actions',HERE/'check_actions.py');a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a);u=a.u
F=a.F.replace(' func setModel(',''' func connectConnector(provider:String,token:String?=nil)async throws->String?{try await request("connect");return token==nil ? "http://synthetic.invalid/start":nil}
 func connectors()async throws->ConnectorsView{try await request("catalog");return ConnectorsView(loaded:true)}
 func disconnectConnector(id:String)async throws{try await request("disconnect")}
 func setModel(''')
M=a.M.replace('struct UserError{','struct UserError{static func fromText(_ label:String,_ error:String)->Self{Self(sentence:label)};')
M=M.replace('enum AuthError{','struct AuthError:Error{var message:String;')
M=M.replace(' var projects=', ' var connectorsBusy=false;var connectors=ConnectorsView()\n var projects=')
M=M.replace('machineBusy=true;machineError=nil;', 'machineBusy=true;connectorsBusy=true;machineError=nil;')
M='''enum ConnectionStatus{case connected}
struct Connector{let status=ConnectionStatus.connected;let lastError:String?=nil}
struct ConnectorsView{var loaded=false;var items:[String]=[];func connector(for provider:String)->Connector?{loaded ? Connector():nil}}
@MainActor final class WebAuth{static var pause=false;static var waiter:CheckedContinuation<URL,Error>?;func open(_ url:URL)async throws->URL{if Self.pause{return try await withCheckedThrowingContinuation{Self.waiter=$0}};return URL(string:"http://synthetic.invalid/done?ok=1")!};static func finish(){let w=waiter;waiter=nil;pause=false;w?.resume(returning:URL(string:"http://synthetic.invalid/done?ok=1")!)}}
@MainActor final class UIApplication{static let shared=UIApplication();func open(_ url:URL)async->Bool{true}}
'''+M
T=r'''
}
@main struct Checks{
 @MainActor static func spin(_ p:()->Bool)async->Bool{for _ in 0..<10000{if p(){return true};await Task.yield()};return p()}
 @MainActor static func main()async{
 var cases:[String:Bool]=[:]
 do{let a=Session("A");let m=AppModel(a);m.connectConnector(provider:"synthetic",token:"synthetic-token");let b=Session("B");m.replace(b);for _ in 0..<100{await Task.yield()};cases["queued-connect-never-uses-B"]=b.requests.isEmpty && m.connectorsBusy && m.errors.isEmpty}
 for failed in [false,true]{let a=Session("A");a.pause=true;let m=AppModel(a);m.connectConnector(provider:"synthetic",token:"synthetic-token");let began=await spin{a.waiter != nil};let b=Session("B");m.replace(b);a.finish(failed);for _ in 0..<100{await Task.yield()};cases[failed ? "late-provider-error-does-not-paint-B":"late-token-return-does-not-refresh-B"]=began && b.requests.isEmpty && m.connectorsBusy && m.errors.isEmpty}
 do{WebAuth.pause=true;let a=Session("A");let m=AppModel(a);m.connectConnector(provider:"synthetic");let began=await spin{WebAuth.waiter != nil};let b=Session("B");m.replace(b);WebAuth.finish();for _ in 0..<100{await Task.yield()};cases["late-auth-callback-does-not-refresh-B"]=began && b.requests.isEmpty && m.connectorsBusy && m.errors.isEmpty}
 for token in [true,false]{let a=Session("A");let m=AppModel(a);m.connectConnector(provider:"synthetic",token:token ? "synthetic-token":nil);let done=await spin{m.connectors.loaded};cases[token ? "current-token-control":"current-web-control"]=done && a.requests==["A:connect","A:catalog"] && !m.connectorsBusy && m.errors.isEmpty}
 let failed=cases.values.filter{!$0}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":cases.count-failed,"failed":failed,"total":cases.count,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--tree',type=Path,required=True);ap.add_argument('--out',type=Path,required=True);arg=ap.parse_args();tree=arg.tree.resolve();out=arg.out.resolve();out.mkdir(exist_ok=False)
 paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift']];app,api,helper=[p.read_text()for p in paths];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};fixture=F+u.method(api,'bound')+'\n}\n'+helper
 if 'struct AccountWork' in app:
  k=re.search(r'^struct AccountWork \{',app,re.M);fixture+='\n'+app[k.start():app.index('\n}',k.start())+2]
 fixture+='\n'+M
 for name in ['accountWork','boundCallAPI','connectConnector','refreshConnectorsOnce']:
  if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=u.method(app,name)+'\n'
 k=re.search(r'^    private static func connectError\(',app,re.M);fixture+=app[k.start():app.index('\n    }',k.end())+6]+T;start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-ios-connect-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'sources':sources,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  record['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
