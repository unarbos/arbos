from pathlib import Path
import argparse,hashlib,json,subprocess,tempfile,time
P=Path(__file__).resolve().parent
SOURCE=P.parents[2]/'apps/ios/Arbos/Arbos/UI/Confirm.swift'
F=r'''
import Foundation
SPEC
struct Binding<T>{let get:()->T;let set:(T)->Void;var wrappedValue:T{get{get()}nonmutating set{set(newValue)}}}
@MainActor protocol View{var recorder:Recorder{get}}
@MainActor struct BasicView:View{let recorder:Recorder}
@MainActor final class Recorder{var presentations:[Presentation]=[]}
@MainActor struct Presentation{let spec:ConfirmSpec;let shown:Binding<Bool>;let action:()->Void;let cancel:()->Void}
@MainActor final class Box<T>{var value:T;init(_ v:T){value=v}}
struct Item:Identifiable,Equatable{let id:String;let payload:String}
extension View{
 func confirm(_ spec:ConfirmSpec,isPresented:Binding<Bool>,onCancel:@escaping()->Void={},perform:@escaping()->Void)->some View{
  recorder.presentations.append(Presentation(spec:spec,shown:isPresented,action:perform,cancel:onCancel));return BasicView(recorder:recorder)
 }
 METHOD
}
@MainActor func present(_ box:Box<Item?>,_ allowed:@escaping()->Bool={true},perform:@escaping(Item)->Void)->Presentation{
 let recorder=Recorder();_=BasicView(recorder:recorder).confirm(item:Binding(get:{box.value},set:{box.value=$0}),spec:{i in ConfirmSpec(title:"Delete "+i.payload,message:"Message "+i.payload,action:"Remove",destructive:false,cancel:"Keep")}CURRENTARG,perform:perform);return recorder.presentations.last!
}
@main struct Checks{
 @MainActor static func main()async{
 var cases:[String:Bool]=[:]
 do{let box=Box<Item?>(Item(id:"a",payload:"A"));var effects:[String]=[];let old=present(box){effects.append($0.payload)};box.value=Item(id:"b",payload:"B");old.action();cases["old-action-cannot-adopt-replacement-entity"]=effects.isEmpty && box.value?.payload=="B"}
 do{let box=Box<Item?>(Item(id:"same",payload:"A"));var effects:[String]=[];let old=present(box){effects.append($0.payload)};box.value=Item(id:"same",payload:"updated");old.action();cases["same-identified-entity-uses-observed-payload"]=effects==["A"] && box.value==nil}
 for edge in ["action","cancel","setter"]{let box=Box<Item?>(Item(id:"same",payload:"A"));var scope=0;var effects:[String]=[];let old=present(box,{scope==0}){if scope==0{effects.append($0.payload)}};scope=1;box.value=Item(id:"same",payload:"B");if edge=="action"{old.action()}else if edge=="cancel"{old.cancel()}else{old.shown.wrappedValue=false};cases["old-account-"+edge+"-cannot-clear-same-id-B-item"]=effects.isEmpty && box.value?.payload=="B"}
 for edge in ["cancel","setter"]{let box=Box<Item?>(Item(id:"a",payload:"A"));let old=present(box){_ in};box.value=Item(id:"b",payload:"B");if edge=="cancel"{old.cancel()}else{old.shown.wrappedValue=false};cases["old-"+edge+"-preserves-different-current-item"]=box.value?.id=="b"}
 do{let box=Box<Item?>(Item(id:"a",payload:"A"));var effects:[String]=[];let old=present(box){i in effects.append(i.payload);box.value=Item(id:"b",payload:"new")};old.action();cases["action-preserves-reentrant-new-selection"]=effects==["A"] && box.value?.id=="b"}
 do{let box=Box<Item?>(Item(id:"same",payload:"A"));var scope=0;let old=present(box,{scope==0}){_ in scope=1;box.value=Item(id:"same",payload:"B")};old.action();cases["action-preserves-reentrant-account-same-id-selection"]=scope==1 && box.value?.payload=="B"}
 do{let box=Box<Item?>(nil);var effects:[String]=[];let old=present(box){effects.append($0.payload)};let initiallyHidden = !old.shown.wrappedValue;box.value=Item(id:"b",payload:"B");let stillHidden = !old.shown.wrappedValue;old.action();cases["nil-render-does-not-adopt-later-item"]=initiallyHidden && stillHidden && effects.isEmpty && box.value?.id=="b"}
 do{let box=Box<Item?>(Item(id:"a",payload:"A"));var effects:[String]=[];let old=present(box){effects.append($0.payload)};box.value=nil;old.action();cases["removed-item-action-is-noop-control"]=effects.isEmpty && box.value==nil}
 do{let box=Box<Item?>(Item(id:"a",payload:"A"));var effects:[String]=[];let old=present(box){effects.append($0.payload)};old.action();old.action();cases["current-confirm-once-control"]=effects==["A"] && box.value==nil;cases["confirm-text-action-style-preserved-control"]=old.spec.title=="Delete A" && old.spec.message=="Message A" && old.spec.buttons==["Remove","Keep"] && !old.spec.destructive}
 do{let box=Box<Item?>(Item(id:"a",payload:"A"));var effects:[String]=[];let old=present(box){effects.append($0.payload)};old.cancel();cases["current-cancel-control"]=effects.isEmpty && box.value==nil}
 do{let box=Box<Item?>(Item(id:"a",payload:"A"));let old=present(box){_ in};cases["current-visibility-control"]=old.shown.wrappedValue;box.value=Item(id:"b",payload:"B");cases["old-visibility-does-not-follow-new-entity"]=!old.shown.wrappedValue}
 do{let box=Box<Item?>(Item(id:"a",payload:"A"));var effects:[String]=[];let old=present(box,{!Task.isCancelled}){if !Task.isCancelled{effects.append($0.payload)}};let task=Task{old.action()};task.cancel();await task.value;cases["cancelled-admission-does-not-clear-pending-item"]=effects.isEmpty && box.value?.id=="a"}
 let failed=cases.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["total":cases.count,"passed":cases.count-failed,"failed":failed,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('arm',choices=['before','after']);ap.add_argument('label');a=ap.parse_args();path=P/'base/Confirm.swift'if a.arm=='before'else SOURCE;source=path.read_text();start=source.index('struct ConfirmSpec:');spec=source[start:source.index('\n}',start)+2];start=source.index('    func confirm<T');method=source[start:source.index('\n    }',start)+6];arg=',isCurrent:allowed'if'isCurrent:'in method else'';fixture=F.replace('SPEC',spec).replace('METHOD',method).replace('CURRENTARG',arg).replace(']=!','] = !');out=P/a.label;out.mkdir(exist_ok=False);(out/'fixture.swift').write_text(fixture);started=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-confirm-')as raw:
  tmp=Path(raw);src=tmp/'main.swift';src.write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(src),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);record={'source_sha256':hashlib.sha256(source.encode()).hexdigest(),'method_sha256':hashlib.sha256(method.encode()).hexdigest(),'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);record.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  assert source==path.read_text();record['seconds']=time.monotonic()-started;(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
if __name__=='__main__':main()
