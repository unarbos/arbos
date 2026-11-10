from pathlib import Path
import argparse,hashlib,json,subprocess,tempfile,time
P=Path(__file__).resolve().parent;ROOT=P.parents[2];UI=Path('apps/ios/Arbos/Arbos/UI')
def method(s,name):
 a=s.index('    static func '+name+'(');return s[a:s.index('\n    }',a)+6]
F=r'''
import Foundation
struct MediaItem{var pictureURL:String?;var blobRef:String?="ref";var remoteURL:String?;var slot="slot"}
@MainActor final class AppModel{var current="A";var admitted=true;var reads=0;var isFixture=false;func gifMediaURL(_ remote:String)->URL?{reads+=1;return URL(string:"https://fixture.invalid/"+current+"/gif")};func blobURL(projectId:String,key:String)->URL?{reads+=1;return URL(string:"https://fixture.invalid/"+current+"/blob")};var authHeaders:[String:String]{reads+=1;return["Authorization":current]}}
@MainActor enum MediaFetch{static var requests:[(String,[String:String])]=[];static var started=false;static var pause=false;static var pending:CheckedContinuation<Void,Never>?;static func data(_ url:URL,headers:[String:String])async->(Data,String)?{started=true;requests.append((url.absoluteString,headers));if pause{await withCheckedContinuation{pending=$0}};return(Data([1]),"image/png")}
 BYTES
}
@main struct Checks{
 @MainActor static func reset(){MediaFetch.requests=[];MediaFetch.started=false;MediaFetch.pause=false}
 @MainActor static func main()async{var cases:[String:Bool]=[:]
 do{reset();let m=AppModel();m.admitted=false;_ = await CALL;cases["stale-fetch-boundary-does-not-acquire-current-authority"] = m.reads==0 && !MediaFetch.started}
 do{reset();let m=AppModel();let t=Task{_ = await CALL};m.admitted=false;m.current="B";await t.value;cases["queued-fetch-boundary-does-not-acquire-B"] = m.reads==0 && !MediaFetch.started}
 do{reset();let m=AppModel();let t=Task{_ = await CALL};t.cancel();await t.value;cases["cancelled-fetch-boundary-refuses-authority"] = m.reads==0 && !MediaFetch.started}
 do{reset();let m=AppModel();_ = await CALL;cases["current-blob-URL-and-bearer-control"] = MediaFetch.requests.first?.0=="https://fixture.invalid/A/blob" && MediaFetch.requests.first?.1==["Authorization":"A"]}
 do{reset();let m=AppModel();MediaFetch.pause=true;let t=Task{_ = await CALL};while !MediaFetch.started{await Task.yield()};m.current="B";m.admitted=false;MediaFetch.pending?.resume();MediaFetch.pending=nil;await t.value;cases["issued-request-keeps-acquired-A-bytes-control"] = MediaFetch.requests.count==1 && MediaFetch.requests[0].1==["Authorization":"A"]}
 do{reset();let m=AppModel();let item=MediaItem(pictureURL:"https://public.invalid/image");_ = await ITEM;cases["public-picture-never-sends-device-bearer-control"] = m.reads==0 && MediaFetch.requests.first?.1==[:]}
 do{reset();let m=AppModel();let item=MediaItem(blobRef:nil,remoteURL:"https://gif.invalid/image");_ = await ITEM;cases["current-GIF-relay-pass-through-control"] = MediaFetch.requests.first?.0=="https://fixture.invalid/A/gif" && MediaFetch.requests.first?.1==["Authorization":"A"]}
 do{reset();let m=AppModel();let item=MediaItem(blobRef:"local:pending");_ = await ITEM;cases["local-pending-no-fetch-control"] = m.reads==0 && !MediaFetch.started}
 let failed=cases.values.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["total":cases.count,"passed":cases.count-failed,"failed":failed,"cases":cases],options:[.sortedKeys]),encoding:.utf8)!);exit(failed==0 ? 0:1)
 }
}
'''
def main():
 ap=argparse.ArgumentParser();ap.add_argument('arm',choices=['before','after']);ap.add_argument('label');a=ap.parse_args();src=P/'base/MediaViewer.swift'if a.arm=='before'else ROOT/UI/'MediaViewer.swift';s=src.read_text();part=method(s[s.index('enum MediaFetch {'):],'bytes');owned='isCurrent:'in part;call='MediaFetch.bytes(for: MediaItem(), model: m'+(', isCurrent: { m.admitted && !Task.isCancelled }'if owned else'')+')';fixture=F.replace('BYTES',part).replace('CALL',call).replace('ITEM',call.replace('MediaItem()','item'));out=P/a.label;out.mkdir(exist_ok=False);(out/'fixture.swift').write_text(fixture);start=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='chat-fetch-')as raw:
  tmp=Path(raw);f=tmp/'main.swift';f.write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(f),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=120);(out/'compile.log').write_text(c.stdout+c.stderr);j={'source_sha256':hashlib.sha256(s.encode()).hexdigest(),'method_sha256':hashlib.sha256(part.encode()).hexdigest(),'compile_exit':c.returncode}
  if c.returncode==0:
   r=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=20);j.update(exit_code=r.returncode,results=json.loads(r.stdout));(out/'test.log').write_text(r.stdout+r.stderr)
  assert s==src.read_text();j['seconds']=time.monotonic()-start;(out/'GATES.json').write_text(json.dumps(j,indent=2)+'\n');print(json.dumps(j))
if __name__=='__main__':main()
