"""Exact media forward task/fetch helper replay; synthetic fetched bytes only."""
from pathlib import Path
import argparse,hashlib,importlib.util,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
spec=importlib.util.spec_from_file_location('media',HERE/'check_media.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);u=m.u
E=r'''
struct Notice{let text:String}
struct Chart{let fallback:Attachment?;func forwardParts(fallbackRef:String?)->[[String:Any]]{[["kind":"chart","fallback":fallbackRef as Any]]}}
struct Payload{let text:String;let attachments:[Attachment];let charts:[Chart]}
struct TargetChat{let id:String}
enum TargetKind{case people(TargetChat);case agent}
struct ForwardTarget{let kind:TargetKind}
@MainActor enum MediaFetch{
 static var pause=false;static var waiter:CheckedContinuation<(Data,Int)?,Never>?;static var requests:[String]=[];static var failures:Set<String>=[]
 static func reset(){pause=false;waiter=nil;requests=[];failures=[]}
 static func data(_ url:URL,headers:[String:String])async->(Data,Int)?{requests.append((headers["Authorization"] ?? "")+":"+url.lastPathComponent);if pause{return await withCheckedContinuation{waiter=$0}};if failures.contains(url.lastPathComponent){return nil};return(Data([9]),200)}
 static func finish(){let w=waiter;waiter=nil;pause=false;w?.resume(returning:(Data([9]),200))}
}
'''
T=r'''
}
@main struct Checks{
 @MainActor static func spin(_ p:()->Bool)async->Bool{for _ in 0..<10000{if p(){return true};await Task.yield()};return p()}
 @MainActor static func main()async{
 var cases:[String:Bool]=[:];let att=Attachment(name:"a",mime:"image/png",blobRef:"a",kind:"image");let pic=Attachment(name:"chart",mime:"image/png",blobRef:"chart",kind:"image")
 let payload=Payload(text:"A-text",attachments:[att],charts:[Chart(fallback:pic)])
 do{MediaFetch.reset();let a=Session("A");let model=AppModel(a);model.forwardTask(payload);model.replace(Session("B"));for _ in 0..<100{await Task.yield()};cases["queued-forward-no-B-fetch-or-send"]=MediaFetch.requests.isEmpty && model.commits.isEmpty}
 do{MediaFetch.reset();MediaFetch.pause=true;let a=Session("A");let model=AppModel(a);model.forwardTask(payload);let began=await spin{MediaFetch.waiter != nil};model.replace(Session("B"));MediaFetch.finish();for _ in 0..<100{await Task.yield()};cases["late-forward-fetch-cannot-copy-as-B"]=began && model.commits.isEmpty && MediaFetch.requests==["Bearer A:a"] && model.errors.isEmpty}
 for failed in [false,true]{MediaFetch.reset();let a=Session("A");a.pause=true;let model=AppModel(a);model.forwardTask(payload);let began=await spin{a.waiter != nil};model.replace(Session("B"));a.finish(failed);for _ in 0..<100{await Task.yield()};cases[failed ? "late-chart-upload-no-B-error":"late-chart-upload-no-B-send"]=began && model.commits.isEmpty && model.errors.isEmpty}
 do{MediaFetch.reset();let a=Session("A");let model=AppModel(a);model.forwardTask(payload);let done=await spin{!model.commits.isEmpty};cases["current-forward-order-chart-control"]=done && model.commits.count==1 && MediaFetch.requests==["Bearer A:a","Bearer A:chart"] && model.commits[0].attachments.map(\.name)==["a"] && model.commits[0].forwarded=="Alice" && model.commits[0].extraParts.first?["kind"] as? String=="chart"}
 do{MediaFetch.reset();MediaFetch.failures=["a"];let a=Session("A");a.failNames=["chart"];let model=AppModel(a);model.forwardTask(payload);let done=await spin{!model.commits.isEmpty};cases["current-partial-failure-chart-preserved"]=done && model.commits.count==1 && model.commits[0].attachments.isEmpty && model.commits[0].extraParts.count==1 && model.errors.count==1 && model.notice != nil}
 let failed=cases.values.filter{!$0}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":cases.count-failed,"failed":failed,"total":cases.count,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--tree',type=Path,required=True);ap.add_argument('--out',type=Path,required=True);a=ap.parse_args();tree=a.tree.resolve();out=a.out.resolve();out.mkdir(exist_ok=False)
 paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift','UI/MediaViewer.swift']];app,api,helper,view=[p.read_text()for p in paths];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};fixture=m.F+u.method(api,'bound')+'\n}\n'+helper
 if 'struct AccountWork' in app:
  k=re.search(r'^struct AccountWork \{',app,re.M);fixture+='\n'+app[k.start():app.index('\n}',k.start())+2]
 fixture+=E
 models=m.M.replace('var visibleChatId:String?="chat"','var visibleChatId:String?="chat";var notice:Notice?')
 models+=''' var authHeaders:[String:String]{["Authorization":"Bearer \\(api.session.label)"]}
 func blobURL(projectId:String,key:String)->URL?{URL(string:"http://synthetic.invalid/"+key)}
 func fail(_ error:String){errors.append(error)}
 func send(chatId:String,text:String,attachments:[PendingAttachment],forwardedFrom:String?,extraParts:[[String:Any]]){write(chatId:chatId,text:text,command:"send",attachments:attachments.map{Attachment(name:$0.name,mime:$0.mime,blobRef:"copied",kind:nil)},forwardedFrom:forwardedFrom,extraParts:extraParts)}
 func sendPeople(chatId:String,text:String,replyTo:String?,attachments:[PendingAttachment],forwardedFrom:String?,extraParts:[[String:Any]]){send(chatId:chatId,text:text,attachments:attachments,forwardedFrom:forwardedFrom,extraParts:extraParts)}
'''
 fixture+=models
 for name in ['accountWork','boundCallAPI']:
  if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=u.method(app,name)+'\n'
 fixture+=u.method(app,'uploadBlob')+'\n'+u.method(view,'fetchForForward')+'\n'
 forward=u.method(view,'forwardMessage');start=forward.index('        Task { @MainActor in')
 if 'let work = accountWork()' in forward[:start]:start=forward.rindex('        guard let work = accountWork()',0,start)
 fixture+='''    func forwardTask(_ payload:Payload){
        let chatId="chat";let targetSlot="project";let slot="source";let target=ForwardTarget(kind:.agent);let forwardedFrom:String?="Alice"
'''+forward[start:]+T
 fixture=fixture.replace('struct UserError{','struct UserError{static func fromText(_ label:String,_ error:String)->String{label}\n')
 start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-ios-forward-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'sources':sources,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  record['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
