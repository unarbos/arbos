"""New actual upload helper cancellation/empty/result contract, synthetic local edges."""
from pathlib import Path
import argparse,hashlib,importlib.util,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
import support as u
import fixture
T=r'''
 func batch(_ input:[PendingAttachment])async->[String]?{
  guard let values=await uploadAttachments(input,slot:"synthetic",api:api)else{return nil}
  return values.map{switch $0{case .uploaded(let a):return "ok:"+a.name;case .failed(let a,_):return "failed:"+a.name}}
 }
}
@main struct Checks{
 @MainActor static func spin(_ p:()->Bool)async->Bool{for _ in 0..<10000{if p(){return true};await Task.yield()};return p()}
 @MainActor static func main()async{
 var cases:[String:Bool]=[:];let items=(0..<4).map{PendingAttachment(name:String($0),mime:"text/plain",data:Data([UInt8($0)]))}
 do{let a=Session("A");let m=AppModel(a);let task=Task{withUnsafeCurrentTask{$0?.cancel()};return await m.batch(items)};let result=await task.value;cases["already-cancelled-no-key-or-request-admission"]=result==nil && a.names.isEmpty}
 do{let a=Session("A");a.delay=1_000_000_000;let m=AppModel(a);let task=Task{await m.batch(items)};let began=await spin{a.active>0};task.cancel();let result=await task.value;cases["cancel-pending-bounded-no-result-or-active-leak"]=began && result==nil && a.active==0 && a.names.count<=2 && a.peak<=2}
 do{let a=Session("A");let m=AppModel(a);let value=await m.batch([]);cases["empty-batch-no-requests"]=value==[] && a.names.isEmpty}
 do{let a=Session("A");a.cancelNames=["0"];let m=AppModel(a);let value=await m.batch(items);cases["individual-cancellation-error-preserves-other-items"]=value==["failed:0","ok:1","ok:2","ok:3"] && a.names.count==4 && !Task.isCancelled}
 let failed=cases.values.filter{!$0}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":cases.count-failed,"failed":failed,"total":cases.count,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,default=HERE.parents[2]);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);ctx=fixture.context(tree);app=ctx['app'];api=ctx['api'];hashes=ctx['hashes']
 f=u.F.replace('var failNames:Set<String>=[]','var cancelNames:Set<String>=[];var failNames:Set<String>=[]').replace('if failNames.contains(key)','if cancelNames.contains(key){throw CancellationError()};if failNames.contains(key)')
 text=f+(u.method(api,'bound')if ctx['owned']else'')+'\n}\n'+fixture.declarations(ctx)+fixture.model(ctx)
 at=app.index('    private enum AttachmentUpload {');text+=app[at:app.index('\n    }',at)+6]+'\n'+u.method(app,'uploadAttachments')+T;tick=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-upload-contract-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(text);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);row={'sources':hashes,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(text.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);row.update(exit_code=r.returncode,results=json.loads(r.stdout)if r.stdout else{});(out/'tests.log').write_text(r.stdout+r.stderr)
  row['seconds']=time.monotonic()-tick;assert fixture.stable(ctx);(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==4;exit(row['results']['failed']!=0)
if __name__=='__main__':main()
