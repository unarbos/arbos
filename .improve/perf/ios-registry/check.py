"""Actual Swift registry/controller+DTO/model source with synthetic async JSON edges."""
from pathlib import Path
import argparse, hashlib, json, os, re, subprocess, tempfile, time
HERE=Path(__file__).resolve().parent
P='apps/ios/Arbos/Arbos/'

def block(src,marker):
    start=src.index(marker)
    return src[start:src.index('\n}',start)+2]

def method(src,name):
    starts=list(re.finditer(r'^    (?:private )?func '+re.escape(name)+r'\(',src,re.M))
    assert len(starts)==1,(name,len(starts))
    m=starts[0];return src[m.start():src.index('\n    }',m.end())+6]

F=r'''
struct RelayError:Error{let status:Int;let message:String}
struct RelayEndpoint{let httpUrl="http://synthetic.invalid"}
actor Backend{
 let mode:String;let delay:UInt64;var active=0;var peak=0;var requests:[String]=[]
 init(_ mode:String="normal",delay:UInt64=0){self.mode=mode;self.delay=delay}
 func snapshot()->(active:Int,peak:Int,requests:[String]){(active,peak,requests)}
 func json(_ path:String)async throws->Any{
  try Task.checkCancellation();requests.append(path)
  if path=="/projects"{
   if mode=="invalid-list"{return ["not":"an array"]}
   var list:[Any]=(0..<12).map{index in var p=project("p\(index)");if index==0{p["owner"]="bare-list-owner"};if index==1{p["archived_at"]=Int64(101)};return p}
   if mode=="mixed"{list += ["invalid-item",["name":"missing-id"],["id":4],project("p0")]}
   return list
  }
  active+=1;peak=max(peak,active);defer{active-=1}
  let id=String(path.dropFirst("/projects/".count))
  let wait:UInt64=mode=="errors" ? (id=="p1" ? 20_000_000:1_000_000):delay
  if wait>0{try await Task.sleep(nanoseconds:wait)}
  try Task.checkCancellation()
  if mode=="errors",id=="p1" || id=="p2"{throw RelayError(status:id=="p1" ? 503:502,message:id)}
  if mode=="mixed"{
   if id=="p4" || id=="p7"{throw RelayError(status:id=="p4" ? 403:404,message:id)}
   if id=="p2"{return "invalid-detail"}
   if id=="p8"{return ["project":project(id)]}
  }
  return detail(id)
 }
 func project(_ id:String)->[String:Any]{
  ["id":id,"name":"","icon":"","color":"","instructions":"private synthetic","avatar":"tree","avatar_color":"mono","state":"unknown","box":"awake","box_since":Int64(11),"spend_today_usd":1.25,"spend_cap_usd":4.5,"created_at":Int64(7),"last_active_at":Int64(8),"description":"synthetic description","vision_enabled":true,"avatar_color_index_effective":Int64(2),"image_key":"picture.png","avatar_version":Int64(42),"owner":["kind":"user","id":"synthetic-owner","name":"Synthetic","handle":"synthetic","avatar_color_index_effective":Int64(2),"online":true],"machine":["state":"awake","since":Int64(10),"class":"test","image_outdated":false,"always_on":false],"models":["talking_model":["id":"model/actual","effort":"max"]]]
 }
 func detail(_ id:String)->[String:Any]{
  ["project":project(id),"chats":[["id":"chat-"+id,"project_id":id,"title":"","is_pinned":true,"is_system":false,"kind":"group","participants":["other"],"position":Int64(0),"created_at":Int64(22),"doc_epoch":Int64(0),"room":"synthetic"]],"box":"waking","box_since":Int64(99),"skills_rev":"synthetic"]
 }
}
final class RelayAPI{
 let endpoint=RelayEndpoint();let backend:Backend
 init(_ backend:Backend){self.backend=backend}
 func json(_ method:String,_ path:String)async throws->Any{try await backend.json(path)}
'''
T=r'''
}
struct EncodedRegistry:Codable{let projects:[Project];let chats:[[Chat]];let skipped:[String];let incomplete:Bool}
func dump(_ value:RegistryResult)->String{let e=JSONEncoder();e.outputFormatting=[.sortedKeys];return String(data:try! e.encode(EncodedRegistry(projects:value.projects.map(\.project),chats:value.projects.map(\.chats),skipped:value.skippedIds.sorted(),incomplete:value.incomplete)),encoding:.utf8)!}
@main struct Checks{
 static func main()async{
  if CommandLine.arguments.contains("--bench"){
   let b=Backend(delay:8_000_000);let tick=DispatchTime.now().uptimeNanoseconds;let result=try! await RelayAPI(b).registry();let elapsed=Double(DispatchTime.now().uptimeNanoseconds-tick)/1_000_000;let state=await b.snapshot();let v:[String:Any]=["milliseconds":elapsed,"peak":state.peak,"active_after":state.active,"requests":state.requests,"output":dump(result)];print(String(data:try! JSONSerialization.data(withJSONObject:v,options:[.sortedKeys]),encoding:.utf8)!);return
  }
  var cases:[String:Bool]=[:];var outputs:[String:String]=[:];var peaks:[String:Int]=[:]
  do{let b=Backend();let result=try await RelayAPI(b).registry();let s=await b.snapshot();outputs["normal"]=dump(result);peaks["normal"]=s.peak
   cases["input-order-and-request-multiset"]=result.projects.map{$0.project.id}==(0..<12).map{"p\($0)"} && Set(s.requests)==Set(["/projects"]+(0..<12).map{"/projects/p\($0)"}) && s.requests.count==13
   guard let first=result.projects.first,let c=first.chats.first else{throw RelayError(status:0,message:"fixture produced no project/chat")};let p=first.project
   cases["actual-generated-DTO-and-model-fields"]=p.name=="Project" && p.icon=="Ω" && p.color=="#8b7cf6" && p.model=="model/actual" && p.effort == .high && p.state == .waking && p.stateSince==99 && p.imageURL=="http://synthetic.invalid/projects/p0/avatar.jpg?v=42" && p.ownerId=="synthetic-owner" && p.instructions=="private synthetic" && p.spendTodayUsd==1.25 && p.visionEnabled && c.title=="New chat" && c.kind=="group" && c.participants==["other"] && !c.unread && c.lastMessageAt==22
   cases["list-only-archive-and-complete-status"]=result.projects[1].project.archivedAt==101 && result.projects[0].project.archivedAt==nil && result.skippedIds.isEmpty && !result.incomplete
  }catch{cases["normal-unexpected-error"]=false}
  do{let b=Backend("mixed");let r=try await RelayAPI(b).registry();outputs["mixed"]=dump(r);cases["partial-invalid-and-access-answers"]=r.incomplete && r.skippedIds==Set(["p2","p8"]) && r.projects.map{$0.project.id}==["p0","p1","p3","p5","p6","p9","p10","p11","p0"];cases["duplicate-list-row-retained"]=r.projects.filter{$0.project.id=="p0"}.count==2}catch{cases["mixed-unexpected-error"]=false}
  do{let r=try await RelayAPI(Backend("invalid-list")).registry();cases["nonarray-list-empty-control"]=r.projects.isEmpty && r.skippedIds.isEmpty && !r.incomplete}catch{cases["nonarray-list-unexpected-error"]=false}
  do{_ = try await RelayAPI(Backend("errors")).registry();cases["first-input-fatal-error"]=false}catch let e as RelayError{cases["first-input-fatal-error"]=e.status==503 && e.message=="p1"}catch{cases["first-input-fatal-error"]=false}
  do{let b=Backend();let task=Task{withUnsafeCurrentTask{$0?.cancel()};return try await RelayAPI(b).registry()};do{_ = try await task.value;cases["pre-cancel-no-requests"]=false}catch{let s=await b.snapshot();cases["pre-cancel-no-requests"]=s.requests.isEmpty && s.active==0}}
  do{let b=Backend(delay:1_000_000_000);let task=Task{try await RelayAPI(b).registry()};var began=false;for _ in 0..<10000{if await b.snapshot().active>0{began=true;break};await Task.yield()};task.cancel();do{_ = try await task.value;cases["cancel-no-result-active-leak"]=false}catch{let s=await b.snapshot();cases["cancel-no-result-active-leak"]=began && s.active==0;peaks["cancel"]=s.peak}}
  let failed=cases.values.filter{!$0}.count;let result:[String:Any]=["cases":cases,"passed":cases.count-failed,"failed":failed,"total":cases.count,"outputs":outputs,"peaks":peaks];print(String(data:try! JSONSerialization.data(withJSONObject:result,options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''

def prepare(tree,out,baseline=False):
    inputs=[P+'Sync/RelayAPI.swift',P+'Model/Model.swift','apps/ios/Arbos/Shared/AvatarPalette.swift','packages/ArbosFFI/Sources/ArbosFFI/RelayTypes.swift']
    paths={rel:tree/rel for rel in inputs}
    if baseline:
        paths.pop(inputs[0])
        paths['.improve/perf/ios-registry/registry-before.swift']=HERE/'registry-before.swift'
    sources={rel:hashlib.sha256(p.read_bytes()).hexdigest()for rel,p in paths.items()}
    api=(HERE/'registry-before.swift').read_text()if baseline else(tree/inputs[0]).read_text()
    model,palette,dto=[(tree/rel).read_text()for rel in inputs[1:]]
    definitions=['typealias Id = String']
    for marker in ['enum BoxState:','enum Effort:','enum WorkspaceState:','struct Project:','extension Project {\n    /// Maps','struct MessageGrant:','struct Chat:','extension Chat {\n    /// Maps']:
        definitions.append(block(model,marker))
    definitions+=['extension Project {\n'+model[model.index('    static func pictureURL('):model.index('\n    }',model.index('    static func pictureURL('))+6]+'\n}',
                  'enum AvatarPalette {\n'+re.search(r'    static let count = \d+',palette).group(0)+'\n'+palette[palette.index('    static func clamp('):palette.index('\n    }',palette.index('    static func clamp('))+6]+'\n}']
    fixture='import Foundation\n'+dto+'\n'+'\n'.join(definitions)+'\n'
    fixture+='\n'.join(block(api,x)for x in ['struct RegistryProject {','struct RegistryResult {','private struct ProjectListId:'])+'\n'
    if 'private enum RegistryDetail' in api:fixture+=block(api,'private enum RegistryDetail')+'\n'
    fixture+=F+method(api,'registry')+'\n'
    if 'func registryDetail(' in api:fixture+=method(api,'registryDetail')+'\n'
    fixture+=T
    source=out/'fixture.swift';source.write_text(fixture);binary=out/'registry-check';env={'PATH':'/usr/bin:/bin','HOME':str(out),'TMPDIR':str(out)};tick=time.monotonic()
    c=subprocess.run(['swiftc','-O','-swift-version','5','-module-name','Arbos','-parse-as-library',str(source),'-o',str(binary)],env=env,text=True,capture_output=True,timeout=180);(out/'compile.log').write_text(c.stdout+c.stderr)
    row={'sources':sources,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode,'compile_seconds':time.monotonic()-tick}
    if c.returncode==0:
        tick=time.monotonic();r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(binary)],env=env,text=True,capture_output=True,timeout=20);(out/'tests.log').write_text(r.stdout+r.stderr);row.update(exit_code=r.returncode,seconds=time.monotonic()-tick,results=json.loads(r.stdout)if r.stdout.strip()else{})
    assert sources=={rel:hashlib.sha256(p.read_bytes()).hexdigest()for rel,p in paths.items()}
    (out/'GATES.json').write_text(json.dumps(row,indent=2)+'\n');print(json.dumps({k:v for k,v in row.items()if k!='results'}),flush=True)
    assert c.returncode==0 and row['exit_code']==0 and row['results']['failed']==0 and row['results']['total']==9,'Registry source/semantic gate'

def main():
    p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--out',type=Path,required=True);p.add_argument('--baseline',action='store_true');a=p.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);prepare(tree,out,a.baseline)
if __name__=='__main__':main()
