"""Compile every actual chat row rebuild against a required hidden-sender input."""
from pathlib import Path
import argparse
import hashlib
import json
import re
import subprocess
import tempfile
import time

PREFIX = Path('apps/ios/Arbos/Arbos')
FIXTURE = r'''
import Foundation
enum Transcript {
 static func rows(_ messages:[String],byId:[String:String],projectId:String,me:String?,people:Bool,agentNames:[String:String],multiParty:Bool,memberNames:[String:String],hiddenSenders:Set<String>,linkPreviews:Bool=true)->Set<String>{hiddenSenders}
}
func rows(_ index:Int,_ hiddenSenders:Set<String>)->Set<String>{
 let slice=["slice"],list=["list"],byId:[String:String]=[:],slot="slot",me:String?="me",peopleFlag=true,agentNames:[String:String]=[:],multiParty=true,memberNames:[String:String]=[:],linkPreviews=true
 switch index{CALLS default:preconditionFailure("unknown call")}
}
@main struct Checks {
 static func main(){var checks:[Bool]=[];for i in 0..<3{checks.append(rows(i,["user:hidden"])==["user:hidden"]);checks.append(rows(i,[]).isEmpty)};let failed=checks.filter{!$0}.count;print(String(data:try!JSONSerialization.data(withJSONObject:["total":checks.count,"passed":checks.count-failed,"failed":failed]),encoding:.utf8)!);exit(failed==0 ? 0:1)}
}
'''


def calls(text):
    found=[]
    for match in re.finditer(r'Transcript\.rows\(',text):
        at=match.end();depth=1
        while depth:
            depth+=(text[at]=='(')-(text[at]==')');at+=1
        found.append(text[match.start():at])
    assert len(found)==3, ('expected three chat row rebuilds',len(found))
    return found


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--tree',type=Path,required=True)
    parser.add_argument('--out',type=Path,required=True)
    parser.add_argument('--mutant',choices=['omit-hidden'])
    args=parser.parse_args();tree=args.tree.resolve();out=args.out.resolve();out.mkdir(parents=True,exist_ok=False)
    source=tree/PREFIX/'UI/ChatView.swift';text=source.read_text();expressions=calls(text)
    if args.mutant:
        changed=expressions[0].replace(', hiddenSenders: hiddenSenders','');assert changed!=expressions[0];expressions[0]=changed
    fixture=FIXTURE.replace('CALLS',''.join('case '+str(i)+':return '+c+'\n'for i,c in enumerate(expressions)))
    record={'scope':'rows','source_sha256':{str(source.relative_to(tree)):hashlib.sha256(text.encode()).hexdigest()},'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'mutant':args.mutant};start=time.monotonic()
    with tempfile.TemporaryDirectory(prefix='arbos-row-contract-')as raw:
        tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw}
        compile_result=subprocess.run(['swiftc','-swift-version','5','-parse-as-library',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=90)
        (out/'compile.log').write_text(compile_result.stdout+compile_result.stderr);record['compile_exit']=compile_result.returncode
        if compile_result.returncode==0:
            result=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=10)
            (out/'test.log').write_text(result.stdout+result.stderr);record.update(exit_code=result.returncode,results=json.loads(result.stdout))
    assert text==source.read_text(),'source changed'
    record['seconds']=time.monotonic()-start;(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
    if args.mutant:
        assert record['compile_exit']!=0,'missing required input compiled'
    else:
        assert record['compile_exit']==0 and record['exit_code']==0 and record['results']=={'total':6,'passed':6,'failed':0}


if __name__=='__main__':main()
