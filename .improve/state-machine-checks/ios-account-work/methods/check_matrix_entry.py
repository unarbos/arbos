"""Actual Matrix startup + RelayAPI HTTP over intercept-all URLProtocol; no SDK/network."""
from pathlib import Path
import importlib.util,hashlib,json,os,subprocess,sys,tempfile,time
import argparse,re
HERE=Path(__file__).resolve().parent
ap=argparse.ArgumentParser();ap.add_argument('--tree',type=Path,required=True);ap.add_argument('--out',type=Path,required=True);args=ap.parse_args();TREE=args.tree.resolve();OUTPUT=args.out.resolve();OUTPUT.mkdir(parents=True,exist_ok=False)
P='apps/ios/Arbos/Arbos/'
inputs=['apps/ios/Arbos/Arbos/Sync/RelayAPI.swift',P+'Store/ServerStore.swift',P+'Sync/WorkLifetime.swift',P+'Store/AppModel.swift'];sources={p:hashlib.sha256((TREE/p).read_bytes()).hexdigest() for p in inputs}
api=(TREE/inputs[0]).read_text();server=(TREE/inputs[1]).read_text();helper=(TREE/inputs[2]).read_text();app=(TREE/inputs[3]).read_text()
def method(text,name):
 matches=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',text,re.M));assert len(matches)==1,(name,'Expected one actual method');a=matches[0].start();a=a-len('    @MainActor\n') if text[:a].endswith('    @MainActor\n') else a;return text[a:text.index('\n    }',a)+6]
at=api.index('@MainActor\nprivate final class CallAuthority {');capture=api[at:api.index('\n}',at)+2]
at=api.index('    private init(endpoint:');extra=api[at:api.index('\n    }',at)+6]
fixture=capture+'\n'+(HERE/'matrix_fakes.swift').read_text().replace('var isUnauthorized:Bool','var type:String{""};var isUnauthorized:Bool')+extra+'\n'+'\n'.join(method(api,n) for n in (['requestBearer']if 'func requestBearer('in api else[])+['request','requestOnce','bound','frozenAuthority','json','matrixSession'])+'\n}\n'+helper
extra_helper=''
if 'struct AccountWork' in app:
 a=app.index('struct AccountWork {');extra_helper=app[a:app.index('\n}',a)+2]+'\n'
fixture+=extra_helper
fixture+='''
import os
struct MatrixCredentials {let homeserver="http://matrix.synthetic.invalid";let userId="synthetic";let accessToken="synthetic";let deviceId="synthetic";static func parse(_ v:Any)->Self?{Self()}}
enum ArbosFFI {
 struct MatrixSession {let homeserver:String;let userId:String;let accessToken:String;let deviceId:String}
 struct MatrixSnapshot {var spaces:[String]=[];var channels:[String]=[]}
 @MainActor final class MatrixClient {func stop(){};func snapshot()async->MatrixSnapshot{MatrixSnapshot()}}
}
final class MatrixBridgeListener {init(model:AppModel,owner:MatrixLifetime.Ticket){}}
@MainActor var starts=0
@MainActor func matrixStart(session:ArbosFFI.MatrixSession,listener:MatrixBridgeListener)async throws->ArbosFFI.MatrixClient{starts+=1;return ArbosFFI.MatrixClient()}
let matrixLog=Logger(subsystem:"synthetic",category:"matrix")
@MainActor final class AppModel {
 var credentials:Bool?=true;var isFixture=false;var matrixClient:ArbosFFI.MatrixClient?;var accountLifetime=WorkLifetime();var matrixLifetime=MatrixLifetime();var matrixListener:MatrixBridgeListener?;var matrixConnected=false;let relay:RelayAPI;var api:RelayAPI{relay}
 init(_ api:RelayAPI){relay=api}
 func applyMatrixSnapshot(_ v:ArbosFFI.MatrixSnapshot){}
 func refreshTokens(expectedOwner:WorkLifetime.Ticket)async->Bool{guard accountLifetime.accepts(expectedOwner)else{return false};return await relay.refresher?() ?? false}
'''+(method(app,'accountWork') if 'func accountWork(' in app else '')+'\n'+method(server,'startMatrixIfNeeded')+'\n'+method(server,'acceptsMatrix')+'''
}
@MainActor final class Gate{var waiter:CheckedContinuation<Bool,Never>?;func wait()async->Bool{await withCheckedContinuation{waiter=$0}};func finish(){waiter?.resume(returning:true);waiter=nil}}
@main struct Checks {
 @MainActor static func spin(_ f:()->Bool)async->Bool{for _ in 0..<10000{if f(){return true};await Task.yield()};return f()}
 @MainActor static func main()async{
  var failures=0
  let box=Box();let config=URLSessionConfiguration.ephemeral;config.protocolClasses=[FakeProtocol.self];let api=RelayAPI(URLSession(configuration:config),{box.token});let model=AppModel(api);let gate=Gate();api.refresher={await gate.wait()};FakeProtocol.reset([401,200]);starts=0
  let old=Task{await model.startMatrixIfNeeded()};let began=await spin({gate.waiter != nil});model.accountLifetime.close();model.matrixLifetime.stop();box.current=false;box.token="B";gate.finish();await old.value
  var ok=began && FakeProtocol.tokens()==["Bearer A"] && starts==0;print("\\(ok ? "PASS":"FAIL") Matrix retired credential fetch cannot retry as B");if !ok{failures+=1}
  box.token="A";box.current=true;model.accountLifetime.reopen();api.refresher={box.token="A2";return true};FakeProtocol.reset([401,200]);starts=0;await model.startMatrixIfNeeded();ok=FakeProtocol.tokens()==["Bearer A","Bearer A2"] && starts==1;print("\\(ok ? "PASS":"FAIL") Matrix same-account refresh and SDK start control");if !ok{failures+=1}
  model.matrixClient=nil;model.matrixLifetime.stop();let staleOwner=model.accountLifetime.ticket!;model.accountLifetime.close();model.accountLifetime.reopen();box.token="B";FakeProtocol.reset([200]);starts=0;await model.startMatrixIfNeeded(EXPECTED_OLD);ok=FakeProtocol.tokens().isEmpty && starts==0;print("\\(ok ? "PASS":"FAIL") Matrix late entry refuses replacement account");if !ok{failures+=1}
  model.matrixClient=nil;model.matrixLifetime.stop();let currentOwner=model.accountLifetime.ticket!;FakeProtocol.reset([200]);starts=0;await model.startMatrixIfNeeded(EXPECTED_CURRENT);ok=FakeProtocol.tokens()==["Bearer B"] && starts==1;print("\\(ok ? "PASS":"FAIL") Matrix current expected-owner control");if !ok{failures+=1}
  print("RESULT tests=4 failed=\\(failures)");exit(failures==0 ? 0:1)
 }
}
'''

owned=bool(re.search(r'func startMatrixIfNeeded\([^\n]*expectedOwner',server))
fixture=fixture.replace('EXPECTED_OLD','expectedOwner:staleOwner'if owned else'').replace('EXPECTED_CURRENT','expectedOwner:currentOwner'if owned else'')
start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-matrix-session-pin-')as raw:
 tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);binary=tmp/'check';env={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120)
 if c.returncode:output=c.stdout+c.stderr;code=c.returncode
 else:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(binary)],env=env,text=True,capture_output=True,timeout=15);output=r.stdout+r.stderr;code=r.returncode
assert sources=={p:hashlib.sha256((TREE/p).read_bytes()).hexdigest() for p in inputs},'Sources changed during Matrix gate'
match=re.search(r'RESULT tests=(\d+) failed=(\d+)',output);counts={'passed':int(match[1])-int(match[2]),'failed':int(match[2])}if match else None
row={'sources':sources,'compile_exit':c.returncode,'exit_code':code,'seconds':time.monotonic()-start,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'counts':counts};(OUTPUT/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');(OUTPUT/'tests.log').write_text(output);print(json.dumps(row));assert code==0 and counts=={'passed':4,'failed':0},'Expected4 Matrix authority/entry checks; inspect tests.log'
