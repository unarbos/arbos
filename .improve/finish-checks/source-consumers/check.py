"""Compile actual view callbacks/call expressions with inert local collaborators."""
from pathlib import Path
import argparse
import hashlib
import json
import re
import subprocess
import tempfile
import time

PREFIX = Path('apps/ios/Arbos/Arbos')


def enclosed(text, start, left='{', right='}'):
    """Swift scopes in these selected methods have balanced literal braces."""
    opening = text.index(left, start)
    depth = 1
    at = opening + 1
    while depth:
        depth += (text[at] == left) - (text[at] == right)
        at += 1
    return text[start:at]


def method(text, name):
    found = list(re.finditer(r'^    (?:private )?func ' + name + r'\(', text, re.M))
    assert len(found) == 1, (name, len(found))
    return enclosed(text, found[0].start())


FIXTURE = r'''
import Foundation
typealias Id=String
final class RelayAPI {}
struct AccountWork { let owner:WorkLifetime.Ticket;let api:RelayAPI }
CONTEXT
POLICY
struct ProfileTarget:Equatable { let id:String }
@MainActor final class AppModel {
 var accountLifetime=WorkLifetime();var isFixture=false;var authenticated=true;var toggles:[ProfileTarget]=[]
 func accountWork()->AccountWork? { guard authenticated,let owner=accountLifetime.ticket else{return nil};return AccountWork(owner:owner,api:RelayAPI()) }
 ACCEPTS
 func replace(){ accountLifetime.close();accountLifetime.reopen() }
 func toggleProfile(_ target:ProfileTarget){ toggles.append(target) }
}
struct Transaction { var disablesAnimations=false;init(){};init(animation:Int?){} }
func withTransaction(_ transaction:Transaction,_ body:()->Void){body()}
struct Deadline { let seconds:Double;static func now()->Self{Self(seconds:0)};static func +(lhs:Self,rhs:Double)->Self{Self(seconds:lhs.seconds+rhs)} }
@MainActor enum DispatchQueue {
 static let main=Queue()
 final class Queue {
  var pending:[()->Void]=[];var delays:[Double]=[]
  func asyncAfter(deadline:Deadline,execute:@escaping ()->Void){delays.append(deadline.seconds);pending.append(execute)}
  func drain(){let ready=pending;pending=[];for work in ready{work()}}
  func reset(){pending=[];delays=[]}
 }
}
enum Anchor {case bottom}
final class Scroller {func scrollTo(_ id:String,anchor:Anchor){}}
struct Probe {let last=0.0}
@propertyWrapper struct Ref<Value> {
 final class Box {var value:Value;init(_ value:Value){self.value=value}}
 let box:Box
 init(wrappedValue:Value){box=Box(wrappedValue)}
 var wrappedValue:Value {get{box.value}nonmutating set{box.value=newValue}}
}
@MainActor struct Actions {
 let model:AppModel;let account:ChatAccountContext;@Ref var searchHit=false
 @Ref var showSearch=true;@Ref var searchText="keyword";@Ref var searchFocused=true
 @Ref var windowSkipEnd=20;@Ref var windowCount=3;@Ref var pinnedToBottom=false;var scroller:Scroller?=nil
 var visibleRows=[1,2];var scrollProbe=Probe();@Ref var traceOffsetsUntil=Date();@Ref var searchResultTapped:String?=nil
 @Ref var revealed:[String]=[];@Ref var revealSawClosed=false
 static let windowInitial=80;static let windowMax=360;static let revealTraceWindow=1.0
 static func offsetText(_ value:Double)->String{"offset"}
 init(_ model:AppModel){self.model=model;account=ChatAccountContext(model)}
 func revealTrace(_ event:String,_ text:String){}
 func revealQuoted(_ id:String){revealed.append(id);revealSawClosed = !showSearch && searchText.isEmpty && !searchFocused && windowSkipEnd==0 && windowCount==Self.windowInitial}
 CLOSE
 OPEN
 PROFILE
 func profile(_ target:ProfileTarget)->()->Void{openProfile(target)}
 func tap(){openSearchResult("hit")}
 func picture()->(ProfileTarget)->Void{{PICTURE}}
}
enum Transcript {
 static func rows(_ messages:[String],byId:[String:String],projectId:String,me:String?,people:Bool,agentNames:[String:String],multiParty:Bool,memberNames:[String:String],hiddenSenders:Set<String>,linkPreviews:Bool=true)->Set<String>{hiddenSenders}
}
func rows(_ index:Int,_ hiddenSenders:Set<String>)->Set<String>{
 let slice=["slice"],list=["list"],byId:[String:String]=[:],slot="slot",me:String?="me",peopleFlag=true,agentNames:[String:String]=[:],multiParty=true,memberNames:[String:String]=[:],linkPreviews=true
 switch index{CALLS default:preconditionFailure("unknown call")}
}
@main struct Checks {
 @MainActor static func main()async{
  var checks:[String:Bool]=[:]
  for i in 0..<3 { checks["rows-\(i)-hidden-input"]=rows(i,["user:hidden"])==["user:hidden"];checks["rows-\(i)-empty-input"]=rows(i,[]).isEmpty }
  for mode in ["current","fixture","retired","unavailable","cancelled"] {
   for hit in [false,true] {
    let m=AppModel();if mode=="fixture"{m.isFixture=true;m.authenticated=false};if mode=="unavailable"{m.authenticated=false};let a=Actions(m);a.searchHit=hit;let callback=a.profile(ProfileTarget(id:"profile"));if mode=="retired"{m.replace()}
    if mode=="cancelled"{let t=Task{callback()};t.cancel();await t.value}else{callback()}
    checks["profile-\(mode)-result-\(hit)"]=m.toggles.count == ((!hit && (mode=="current" || mode=="fixture")) ? 1:0)
   }
   let m=AppModel();if mode=="fixture"{m.isFixture=true;m.authenticated=false};if mode=="unavailable"{m.authenticated=false};let a=Actions(m);let callback=a.picture();if mode=="retired"{m.replace()}
   if mode=="cancelled"{let t=Task{callback(ProfileTarget(id:"picture"))};t.cancel();await t.value}else{callback(ProfileTarget(id:"picture"))}
   checks["presence-picture-\(mode)"]=m.toggles.count == ((mode=="current" || mode=="fixture") ? 1:0)
  }
  for mode in ["current","fixture","empty","closed","retired","unavailable","cancelled","late-retirement"] {
   DispatchQueue.main.reset();let m=AppModel();if mode=="fixture"{m.isFixture=true;m.authenticated=false};if mode=="unavailable"{m.authenticated=false};let a=Actions(m)
   if mode=="empty"{a.searchText=""};if mode=="closed"{a.showSearch=false};if mode=="retired"{m.replace()}
   if mode=="cancelled"{let t=Task{a.tap()};t.cancel();await t.value}else{a.tap()}
   if mode=="current" || mode=="fixture" || mode=="late-retirement" {
    let queued = !a.showSearch && a.searchText.isEmpty && !a.searchFocused && a.windowSkipEnd==0 && a.windowCount==80 && !a.pinnedToBottom && a.revealed.isEmpty && DispatchQueue.main.delays==[0.05]
    if mode=="late-retirement"{m.replace()};DispatchQueue.main.drain()
    checks["search-\(mode)"]=queued && (mode=="late-retirement" ? a.revealed.isEmpty : a.revealed==["hit"] && a.revealSawClosed)
   }else{checks["search-\(mode)"]=a.searchResultTapped==nil && a.revealed.isEmpty && DispatchQueue.main.pending.isEmpty}
  }
  let failures=checks.filter{!$0.value}.map(\.key).sorted();print(String(data:try!JSONSerialization.data(withJSONObject:["total":checks.count,"passed":checks.count-failures.count,"failed":failures.count,"failures":failures],options:[.sortedKeys]),encoding:.utf8)!);exit(failures.isEmpty ? 0:1)
 }
}
'''


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--tree',type=Path,required=True)
    parser.add_argument('--out',type=Path,required=True)
    parser.add_argument('--scope',choices=['rows','all'],default='all')
    parser.add_argument('--mutant',choices=['omit-hidden','ignore-result','late-reveal','unguarded-picture','skip-close'])
    args=parser.parse_args();tree=args.tree.resolve();out=args.out.resolve();out.mkdir(parents=True,exist_ok=False)
    names=['UI/ChatView.swift','UI/Transcript.swift']
    if args.scope=='all':names+=['Model/ChatSearchShortcut.swift','Sync/WorkLifetime.swift','Store/AppModel.swift']
    sources={n:(tree/PREFIX/n).read_text()for n in names};chat=sources[names[0]];transcript=sources[names[1]]
    calls=[]
    for m in re.finditer(r'Transcript\.rows\(',chat):calls.append(enclosed(chat,m.start(),'(',')'))
    assert len(calls)==3,len(calls)
    profile=method(transcript,'openProfile')if args.scope=='all'else''
    opened=method(chat,'openSearchResult')if args.scope=='all'else''
    picture=enclosed(chat,chat.index('onPicture: {')).split('{',1)[1][:-1]if args.scope=='all'else''
    before_mutant=(tuple(calls),profile,opened,picture)
    if args.mutant=='omit-hidden':calls[0]=calls[0].replace(', hiddenSenders: hiddenSenders','')
    if args.mutant=='ignore-result':profile=profile.replace(', !ChatSearchClose.rowOwnsInnerTap(resultRow: searchHit)','')
    if args.mutant=='late-reveal':opened=opened.replace('{ guard account.accepts(model) else { return }; revealQuoted','{ revealQuoted')
    if args.mutant=='unguarded-picture':picture=picture.replace('guard account.accepts(model) else { return };','')
    if args.mutant=='skip-close':opened=opened.replace('        closeSearch(scrollToTail: false)\n','')
    if args.mutant:assert before_mutant!=(tuple(calls),profile,opened,picture),'mutation had no effect'
    call_code=''.join('case '+str(i)+':return '+c+'\n'for i,c in enumerate(calls))
    if args.scope=='all':
        context=enclosed(chat,chat.index('enum ChatAccountContext:'))
        policy=enclosed(sources[names[2]],sources[names[2]].index('enum ChatSearchClose'))
        fixture=sources[names[3]]+FIXTURE.replace('CONTEXT',context).replace('POLICY',policy).replace('ACCEPTS',method(sources[names[4]],'acceptsAccountWork')).replace('CLOSE',method(chat,'closeSearch')).replace('OPEN',opened).replace('PROFILE',profile).replace('PICTURE',picture).replace('CALLS',call_code)
    else:
        fixture='import Foundation\n'+FIXTURE[FIXTURE.index('enum Transcript {'):FIXTURE.index('@main struct Checks')].replace('CALLS',call_code)+r'''
@main struct RowChecks { static func main(){var checks:[Bool]=[];for i in 0..<3{checks.append(rows(i,["user:hidden"])==["user:hidden"]);checks.append(rows(i,[]).isEmpty)};let failed=checks.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["total":checks.count,"passed":checks.count-failed,"failed":failed]),encoding:.utf8)!);exit(failed==0 ? 0:1)} }
'''
    start=time.monotonic();record={'scope':args.scope,'source_sha256':{str(PREFIX/n):hashlib.sha256(s.encode()).hexdigest()for n,s in sources.items()},'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'mutant':args.mutant}
    with tempfile.TemporaryDirectory(prefix='arbos-consumer-')as raw:
        tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw}
        compiled=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=90)
        (out/'compile.log').write_text(compiled.stdout+compiled.stderr);record['compile_exit']=compiled.returncode
        if compiled.returncode==0:
            result=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=10)
            (out/'test.log').write_text(result.stdout+result.stderr);record.update(exit_code=result.returncode,results=json.loads(result.stdout))
    assert all(s==(tree/PREFIX/n).read_text()for n,s in sources.items()),'source changed'
    record['seconds']=time.monotonic()-start;(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
    if args.mutant:
        assert record['compile_exit']!=0 or record['results']['failed']>0,'mutant escaped detection'
        return
    assert record['compile_exit']==0 and record['exit_code']==0 and record['results']['total']==(29 if args.scope=='all'else 6)


if __name__=='__main__':main()
