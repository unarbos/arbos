from pathlib import Path
import argparse,hashlib,json,subprocess,tempfile,time
P=Path(__file__).resolve().parent;ROOT=P.parents[2];UI=Path('apps/ios/Arbos/Arbos/UI')
def method(s,name):
 needle=('    private func 'if name=='copy'else'    func ')+name+'(';a=s.index(needle);return s[a:s.index('\n    }',a)+6]
F=r'''
import Foundation
struct Notice{let text:String}
struct Attachment{var isOnRelay=true;var blobRef:String?="ref";var name="file";var mime="image/png"}
struct Body{var text="text";var attachments:[Attachment]=[Attachment(),Attachment()]}
struct Message{var body:Body?=Body();var hasUploadingAttachment=false}
enum Kind{case file}
enum MediaAlbum{static func kind(of a:Attachment)->Kind?{.file}}
struct MediaItem{var id:String;var name:String;var mime:String;var kind:Kind;var blobRef:String?;var slot:String;var by:String;var at:Int}
@MainActor enum MessagePasteboard{struct Fetched{let name:String;let mime:String;let data:Data};static var writes=0;static func items(text:String,fetched:[Fetched])->[Int]{text.isEmpty && fetched.isEmpty ? []:[1]};static func write(_ items:[Int]){writes+=1}}
@MainActor enum MediaFetch{static var pending:CheckedContinuation<Void,Never>?;static var calls:[String]=[];static var pause=false;static var fail=false;static func bytes(for item:MediaItem,model:AppModel,isCurrent:@MainActor()->Bool={true})async->(Data,String)?{guard isCurrent()else{return nil};calls.append(model.current);if pause && calls.count==1{await withCheckedContinuation{pending=$0}};return fail ? nil:(Data([1]),"image/png")}}
@MainActor final class AppModel{var notice:Notice?;var current="A";var admitted=true
 METHODS
}
@main struct Checks{
 @MainActor static func reset(){MediaFetch.calls=[];MediaFetch.pause=false;MediaFetch.fail=false;MessagePasteboard.writes=0}
 @MainActor static func main()async{var cases:[String:Bool]=[:]
 do{reset();let m=AppModel();m.admitted=false;await CALL;cases["stale-copy-refuses-entry"] = MediaFetch.calls.isEmpty && MessagePasteboard.writes==0 && m.notice==nil}
 for fail in [false,true]{reset();let m=AppModel();MediaFetch.pause=true;MediaFetch.fail=fail;let t=Task{await CALL};while MediaFetch.calls.isEmpty{await Task.yield()};m.admitted=false;m.current="B";MediaFetch.pending?.resume();MediaFetch.pending=nil;await t.value;cases[fail ? "late-copy-error-no-B-notice":"late-copy-no-next-B-fetch-or-clipboard"] = MediaFetch.calls==["A"] && MessagePasteboard.writes==0 && m.notice==nil}
 do{reset();let m=AppModel();await CALL;cases["current-copy-preserves-two-fetch-control"] = MediaFetch.calls==["A","A"] && MessagePasteboard.writes==1 && m.notice?.text=="Copied 2 pictures and the text"}
 do{reset();let m=AppModel();MediaFetch.fail=true;await CALL;cases["current-copy-error-control"] = MessagePasteboard.writes==1 && m.notice?.text=="Copied — 2 pictures didn't download"}
 do{reset();let m=AppModel();var message=Message();message.body=Body(text:"text",attachments:[]);await TEXT;cases["text-only-copy-control"] = MediaFetch.calls.isEmpty && MessagePasteboard.writes==1}
 let failed=cases.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["total":cases.count,"passed":cases.count-failed,"failed":failed,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('arm',choices=['before','after']);ap.add_argument('label');a=ap.parse_args();src=P/'base/MessagePasteboard.swift'if a.arm=='before'else ROOT/UI/'MessagePasteboard.swift';s=src.read_text();parts=[method(s,n)for n in ['copyMessage','copy']];owned='isCurrent:'in parts[0];call='m.copyMessage(Message(), slot: "slot"'+(', isCurrent: { m.admitted && !Task.isCancelled }'if owned else'')+')';text=call.replace('Message()','message');fixture=F.replace('METHODS','\n'.join(parts)).replace('CALL',call).replace('TEXT',text);out=P/a.label;out.mkdir(exist_ok=False);(out/'fixture.swift').write_text(fixture);start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='chat-copy-')as raw:
  tmp=Path(raw);f=tmp/'main.swift';f.write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(f),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);j={'source_sha256':hashlib.sha256(s.encode()).hexdigest(),'methods_sha256':hashlib.sha256(''.join(parts).encode()).hexdigest(),'compile_exit':c.returncode}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);j.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  assert s==src.read_text();j['seconds']=time.monotonic()-start;(out/'GATES.json').write_text(json.dumps(j,indent=2)+'\n');print(json.dumps(j))
if __name__=='__main__':main()
