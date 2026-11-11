"""Actual feedback send helper/UI controller; fake image encoder, no screen capture."""
from pathlib import Path
import argparse,hashlib,importlib.util,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/Arbos/'
spec=importlib.util.spec_from_file_location('a',HERE/'check_actions.py');a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a);u=a.u
F=a.F.replace(' func setModel(',''' func sendFeedback(note:String,image:Data?,mime:String?,width:Int,height:Int,appVersion:String,os:String,projectId:String?)async throws->String?{session.notes.append(note);session.feedback.append((image,mime,width,height,projectId));try await request("feedback");return session.label+"-feedback"}
 func setModel(''').replace('var pause=false;var waiter:', 'var notes:[String]=[];var feedback:[(Data?,String?,Int,Int,String?)]=[];var pause=false;var waiter:')
E=r'''
typealias CGFloat=Double
struct UIImage{let size=(width:2.0,height:3.0);let scale=2.0;func jpegData(compressionQuality:CGFloat)->Data?{Data([9,8])}}
final class UIDevice{static let current=UIDevice();let systemVersion="synthetic"}
@MainActor final class FeedbackCenter{static let shared=FeedbackCenter();static let appVersion="synthetic";struct Context{var projectId:String?="project-A"};var context=Context();var screenshot:UIImage?=UIImage()}
'''
V=r'''
}
@MainActor final class Sheet{
 let model:AppModel;let center=FeedbackCenter.shared;var note=" A note ";var sending=false;var error:UserError?;var noteFocused=true;var sent:String?;var canSend=true
 init(_ model:AppModel){self.model=model}
 func trigger(_ work:AccountWork?){send(CALL)}
'''
T=r'''
}
@main struct Checks{
 @MainActor static func spin(_ p:()->Bool)async->Bool{for _ in 0..<10000{if p(){return true};await Task.yield()};return p()}
 @MainActor static func main()async{
 var cases:[String:Bool]=[:]
 do{let a=Session("A");let model=AppModel(a);let view=Sheet(model);let old=model.accountWork();let b=Session("B");model.replace(b);view.trigger(old);for _ in 0..<100{await Task.yield()};cases["retired-feedback-render-cannot-post-as-B"]=b.requests.isEmpty && view.sent==nil}
 do{let a=Session("A");let model=AppModel(a);let view=Sheet(model);view.trigger(model.accountWork());let b=Session("B");model.replace(b);for _ in 0..<100{await Task.yield()};cases["queued-feedback-cannot-post-as-B"]=b.requests.isEmpty && view.sent==nil}
 for failed in [false,true]{let a=Session("A");a.pause=true;let model=AppModel(a);let view=Sheet(model);view.trigger(model.accountWork());let began=await spin{a.waiter != nil};model.replace(Session("B"));view.sending=true;a.finish(failed);for _ in 0..<100{await Task.yield()};cases[failed ? "late-feedback-error-no-B-paint":"late-feedback-result-no-B-paint"]=began && view.sent==nil && view.error==nil && view.sending}
 do{let a=Session("A");let model=AppModel(a);let view=Sheet(model);view.trigger(model.accountWork());let done=await spin{view.sent != nil};cases["current-feedback-body-image-control"]=done && view.sent=="A-feedback" && !view.sending && a.notes==["A note"] && a.feedback.count==1 && a.feedback[0].0==Data([9,8]) && a.feedback[0].1=="image/jpeg" && a.feedback[0].2==4 && a.feedback[0].3==6 && a.feedback[0].4=="project-A"}
 let failed=cases.values.filter{!$0}.count;print(String(data:try! JSONSerialization.data(withJSONObject:["passed":cases.count-failed,"failed":failed,"total":cases.count,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--tree',type=Path,required=True);ap.add_argument('--out',type=Path,required=True);arg=ap.parse_args();tree=arg.tree.resolve();out=arg.out.resolve();out.mkdir(exist_ok=False)
 paths=[tree/(P+q)for q in ['Store/AppModel.swift','Sync/RelayAPI.swift','Sync/WorkLifetime.swift','UI/FeedbackSheet.swift']];app,api,helper,view=[p.read_text()for p in paths];sources={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};fixture=F+u.method(api,'bound')+'\n}\n'+helper
 if 'struct AccountWork' in app:
  k=re.search(r'^struct AccountWork \{',app,re.M);fixture+='\n'+app[k.start():app.index('\n}',k.start())+2]
 else:fixture+='\nstruct AccountWork{let owner:WorkLifetime.Ticket;let api:RelayAPI}\n'
 fixture+='\n'+E+a.M
 if 'struct AccountWork' not in app:fixture+=' func accountWork()->AccountWork?{AccountWork(owner:accountLifetime.ticket!,api:api)}\n'
 for name in ['accountWork','acceptsAccountWork','boundCallAPI','sendFeedback']:
  if re.search(r'^    (?:private )?func '+name+r'\(',app,re.M):fixture+=u.method(app,name)+'\n'
 method=u.method(view,'send');fixture+=V.replace('CALL','work:work'if'func send(work:'in method else'')+method+T;start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-ios-feedback-send-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'sources':sources,'compile_exit':c.returncode,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest()}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,text=True,capture_output=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  record['seconds']=time.monotonic()-start;assert sources=={str(p.relative_to(tree)):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths};(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
