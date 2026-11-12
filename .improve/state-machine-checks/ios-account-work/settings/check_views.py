"""Run unchanged Swift view controller bodies without SwiftUI/network collaborators."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--actions',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
view=tree/'apps/ios/Arbos/Arbos/UI/SecretsViews.swift';src=view.read_text();sourcehash=hashlib.sha256(src.encode()).hexdigest();meta=json.loads((a.actions/'GATES.json').read_text())
for rel,digest in meta['sources'].items():assert hashlib.sha256((tree/rel).read_bytes()).hexdigest()==digest,'Controller fixture does not match tree'
fixture=(a.actions/'fixture.swift').read_text().split('@main struct Checks{')[0]
def block(name):
 start=src.index('struct '+name+': View {');end=src.find('\nstruct ',start+1);return src[start:end if end!=-1 else len(src)]
def method(name,fn):
 text=block(name);m=re.search(r'^    private func '+fn+r'\(',text,re.M);assert m,(name,fn);return text[m.start():text.index('\n    }',m.end())+6].replace('private func','func',1)
fixture+=r'''
struct SecretActivity:Equatable{let id:String}
struct ActivityReply{let items:[SecretActivity];let nextBefore:Int64?}
extension RelayAPI{
 func createSecret(_ body:[String:Any])async throws{try await call("createSecret")}
 func secretsActivity(secretId:String?,projectId:String?,vaultId:String?,before:Int64?,limit:Int)async throws->ActivityReply{try await call("activity");return ActivityReply(items:[SecretActivity(id:session.label)],nextBefore:nil)}
 func setVaultItemEnv(_ id:String,ref:String,on:Bool)async throws->[[String:Any]]{try await call("itemEnv");return [["marker":session.label]]}
}
@MainActor final class NewSecretSheet{
 let model:AppModel;var busy=false;var error:UserError?;var name="KEY";var kind="key";var note="";var hosts="";var picked:Set<String>=[];var injectEnv=false;var url="";var username="";var password="";var value="synthetic-secret";var canSave=true;var dismissed=false
 init(_ model:AppModel){self.model=model}
 func dismiss(){dismissed=true}
'''+method('NewSecretSheet','save')+'\n}\n'+r'''
@MainActor final class GiveAccessSheet{
 let model:AppModel;var secret:SecretItem;var picked:Set<String>=["one","two"];var note="";var busy=false;var dismissed=false
 init(_ model:AppModel){self.model=model;secret=SecretItem(id:"A")}
 func dismiss(){dismissed=true}
'''+method('GiveAccessSheet','save')+'\n}\n'+r'''
@MainActor final class SecretsActivityPage{
 let model:AppModel;var loading=false;var secretId:String?="s";var projectId:String?;var vaultId:String?;var nextBefore:Int64?;var items:[SecretActivity]=[]
 init(_ model:AppModel){self.model=model}
'''+method('SecretsActivityPage','load')+'\n}\n'+r'''
@MainActor final class VaultPage{
 let model:AppModel;var busy=false;var vaultId="v";var items:[[String:Any]]=[]
 init(_ model:AppModel){self.model=model}
'''+method('VaultPage','setItemEnv')+'\n}\n'
# The scheduler supplies a captured capability to the successor; the before arm has no such parameter.
newcall='await v.save(work:work)'if 'func save(work:'in block('NewSecretSheet')else'await v.save()'
givecall='await v.save(work:work)'if 'func save(work:'in block('GiveAccessSheet')else'await v.save()'
loadcall='await v.load(more:false,work:work)'if 'work: AccountWork'in method('SecretsActivityPage','load')else'await v.load(more:false)'
fixture+=r'''
@main struct ViewChecks{
 @MainActor static func spin(_ predicate:()->Bool)async->Bool{for _ in 0..<10000{if predicate(){return true};await Task.yield()};return predicate()}
 @MainActor static func main()async{
  var results:[[String:Any]]=[];func record(_ name:String,_ pass:Bool){results.append(["name":name,"pass":pass])}
  do{let a=Session("A");let m=AppModel(a);let v=NewSecretSheet(m);let work=m.accountWork()!;let old=Task{NEW};let b=Session("B");m.replace(b);await old.value;record("queued-secret-form-refuses-B-authority",b.calls.isEmpty && !v.dismissed)}
  do{let a=Session("A");a.pauses=["createSecret"];let m=AppModel(a);let v=NewSecretSheet(m);let work=m.accountWork()!;let old=Task{NEW};let began=await spin{a.waiters["createSecret"] != nil};let b=Session("B");m.replace(b);a.finish("createSecret");await old.value;record("retired-secret-form-no-refresh-dismiss-cleanup",began && b.calls.isEmpty && !v.dismissed && v.busy && v.value=="synthetic-secret")}
  do{let a=Session("A");a.pauses=["createSecret"];let m=AppModel(a);let v=NewSecretSheet(m);let work=m.accountWork()!;let old=Task{NEW};let began=await spin{a.waiters["createSecret"] != nil};m.replace(Session("B"));a.finish("createSecret",500);await old.value;record("retired-secret-form-no-error",began && v.error==nil)}
  do{let a=Session("A");let m=AppModel(a);let v=NewSecretSheet(m);let work=m.accountWork()!;NEW;record("current-secret-form-control",v.dismissed && !v.busy && v.value.isEmpty && a.calls==["createSecret","secrets"])}
  do{let a=Session("A");a.pauses=["access"];let m=AppModel(a);let v=GiveAccessSheet(m);let work=m.accountWork()!;let old=Task{GIVE};let began=await spin{a.waiters["access"] != nil};let b=Session("B");m.replace(b);a.pauses=[];a.finish("access");await old.value;record("multi-access-loop-stops-at-owner-edge",began && a.calls==["access"] && b.calls.isEmpty && !v.dismissed && v.busy)}
  do{let a=Session("A");let m=AppModel(a);let v=GiveAccessSheet(m);let work=m.accountWork()!;GIVE;record("current-multi-access-control",a.calls==["access","access","secrets"] && v.dismissed && !v.busy)}
  do{let a=Session("A");a.pauses=["activity"];let m=AppModel(a);let v=SecretsActivityPage(m);let work=m.accountWork()!;let old=Task{LOAD};let began=await spin{a.waiters["activity"] != nil};m.replace(Session("B"));a.finish("activity");await old.value;record("late-activity-no-publication-or-busy-release",began && v.items.isEmpty && v.loading)}
  do{let a=Session("A");let m=AppModel(a);let v=SecretsActivityPage(m);let work=m.accountWork()!;LOAD;record("current-activity-control",v.items==[SecretActivity(id:"A")] && !v.loading)}
  do{let a=Session("A");let m=AppModel(a);let v=VaultPage(m);v.setItemEnv(["ref":"fixture"],on:true);let b=Session("B");m.replace(b);for _ in 0..<100{await Task.yield()};record("queued-vault-item-refuses-B-authority",b.calls.isEmpty && v.items.isEmpty)}
  do{let a=Session("A");a.pauses=["itemEnv"];let m=AppModel(a);let v=VaultPage(m);v.setItemEnv(["ref":"fixture"],on:true);let began=await spin{a.waiters["itemEnv"] != nil};let b=Session("B");m.replace(b);a.finish("itemEnv");for _ in 0..<100{await Task.yield()};record("late-vault-item-no-B-write-or-refresh",began && v.items.isEmpty && b.calls.isEmpty && v.busy)}
  let failed=results.filter{!($0["pass"]as!Bool)}.count;let value:[String:Any]=["passed":results.count-failed,"failed":failed,"total":results.count,"results":results];print(String(data:try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
fixture=fixture.replace('NEW',newcall).replace('GIVE',givecall).replace('LOAD',loadcall)
(out/'fixture.swift').write_text(fixture);start=time.monotonic()
with tempfile.TemporaryDirectory(prefix='arbos-secret-views-')as raw:
 tmp=Path(raw);srcfile=tmp/'main.swift';srcfile.write_text(fixture);binary=tmp/'checks';env={'PATH':'/usr/bin:/bin','HOME':str(tmp),'TMPDIR':str(tmp)}
 c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(srcfile),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text((c.stdout+c.stderr).replace(raw,'<fresh-build>'));row={'sources':{**meta['sources'],str(view.relative_to(tree)):sourcehash},'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
 if c.returncode==0:
  r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* file-write* (subpath "/Users/Shared"))',str(binary)],env=env,text=True,capture_output=True,timeout=25);row.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'tests.log').write_text(r.stdout+r.stderr)
 row['seconds']=time.monotonic()-start;assert hashlib.sha256(view.read_bytes()).hexdigest()==sourcehash;(out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps(row));assert c.returncode==0 and row['results']['total']==10,'Compile/count guard';exit(row['results']['failed']!=0)
