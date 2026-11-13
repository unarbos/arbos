"""macOS host checks of actual iOS preview builder/cache/helper; no simulator or app effects."""
from pathlib import Path
import argparse,hashlib,json,os,re,runpy,shutil,subprocess,sys,tempfile,time
HERE=Path(__file__).resolve().parent;ROOT=HERE.parents[2]
SOURCES=['apps/ios/Arbos/Arbos/UI/Transcript.swift','apps/ios/Arbos/Arbos/UI/ChatView.swift','apps/ios/Arbos/Arbos/UI/MediaViewer.swift','apps/ios/Arbos/Arbos/Model/Model.swift','apps/ios/Arbos/Arbos/Store/AppModel.swift','apps/ios/Arbos/ArbosTests/LinkPreviewCardTests.swift']
READER=runpy.run_path(str(HERE/'extract.py'));block=READER['block'];prepare=READER['prepare']
POLICY='(version 1) (allow default) (deny network*)'
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def run(command,env,output,timeout=120):
 start=time.monotonic();r=subprocess.run(['/usr/bin/sandbox-exec','-p',POLICY]+command,env=env,capture_output=True,timeout=timeout);output.write_bytes(r.stdout+r.stderr)
 if r.returncode:raise RuntimeError('Local Swift check command failed; see '+str(output))
 return r,time.monotonic()-start
def check(tree,reference,output):
 output.mkdir(parents=True,exist_ok=True);hashes={p:sha(tree/p)for p in SOURCES}
 with tempfile.TemporaryDirectory(prefix='arbos-ios-preview-')as raw:
  work=Path(raw);env={'PATH':os.environ.get('PATH','/usr/bin:/bin'),'HOME':str(work),'TMPDIR':str(work)};generated=work/'leaf.swift';binary=work/'leaf'
  prepare(tree,generated);r,compile_seconds=run(['swiftc','-swift-version','5',str(generated),'-o',str(binary)],env,output/'compile.log')
  r,run_seconds=run([str(binary)],env,output/'run.log',10);rows=[json.loads(line)for line in r.stdout.decode().splitlines()];observations={row['case']:row for row in rows};assert len(observations)==12
  expected={'default-user':{'preview':True},'off-user':{'preview':False,'text':'https://synthetic.test/path'},'on-assistant':{'preview':True},'off-assistant':{'preview':False},'sender-removed':{'preview':False,'text':'https://synthetic.test/path'},'off-caller-survives-on':{'preview':False,'other':True},'on-caller-survives-off':{'preview':True,'other':False},'unchanged-positive':{'preview':True,'reused':True,'path':'unchanged'},'policy-unchanged-messages':{'preview':False},'policy-plus-append':{'preview':False},'policy-plus-tail':{'preview':False},'same-policy-tail-positive':{'preview':True,'reused':True,'path':'tail'}}
  checks=[{'case':name,'pass':all(observations[name].get(key)==value for key,value in fields.items()),'expected':fields,'actual':observations[name]}for name,fields in expected.items()]
  tests=(tree/SOURCES[-1]).read_text();reference_tests=(reference/SOURCES[-1]).read_text();helper=block(tests,'private func userRow(');negative=block(reference_tests,'func testMissingUserRowIsAnAssertionFailure()')
  body=generated.read_text();body=body[:body.index('let preview=')]
  body+='\nimport XCTest\n@objcMembers final class MandatoryUserRowProbe: XCTestCase {\n'+helper+'\n'+negative+'\nfunc testPresentUserRowPasses() throws {let row=Row(id:"synthetic",messageId:"synthetic",role:.user,kind:.user(text:"synthetic text",delivery:.delivered,quote:nil));let value=try userRow([row]);XCTAssertEqual(value.text,"synthetic text")}\n}\nlet suite=XCTestSuite(forTestCaseClass:MandatoryUserRowProbe.self);suite.run();let run=suite.testRun!;print("mandatory-helper-summary:"+String(data:try!JSONSerialization.data(withJSONObject:["tests":run.testCaseCount,"failures":run.totalFailureCount,"succeeded":run.hasSucceeded],options:[.sortedKeys]),encoding:.utf8)!)\n'
  helper_source=work/'helper.swift';helper_source.write_text(body);helper_binary=work/'helper'
  found=subprocess.run(['xcrun','--sdk','macosx','--show-sdk-platform-path'],env=env,capture_output=True,text=True,timeout=10);assert found.returncode==0
  developer=Path(found.stdout.strip())/'Developer';frameworks=str(developer/'Library/Frameworks');libraries=str(developer/'usr/lib')
  command=['swiftc','-swift-version','5','-I',libraries,'-L',libraries,'-F',frameworks,'-lXCTestSwiftSupport','-framework','XCTest','-Xlinker','-rpath','-Xlinker',frameworks,'-Xlinker','-rpath','-Xlinker',libraries,str(helper_source),'-o',str(helper_binary)]
  r,helper_compile_seconds=run(command,env,output/'helper-compile.log');r,helper_run_seconds=run([str(helper_binary)],env,output/'helper-run.log',15)
  match=re.search(rb'mandatory-helper-summary:(\{[^\n]+\})',r.stdout);assert match,'Missing real XCTest summary';helper_result=json.loads(match.group(1));assert helper_result['tests']==2
  assert hashes=={p:sha(tree/p)for p in SOURCES},'Source changed during check'
  record={'source_hashes':hashes,'selected_items':json.loads(generated.with_suffix('.provenance.json').read_text())['selected_items'],'compile_seconds':compile_seconds,'run_seconds':run_seconds,'checks':checks,'pass':sum(row['pass']for row in checks),'fail':sum(not row['pass']for row in checks),'helper':helper_result,'helper_compile_seconds':helper_compile_seconds,'helper_run_seconds':helper_run_seconds,'scope':'actual Foundation DTOs, Transcript.rows, ChatView.refreshRows and real XCTest helper; observer/other-domain collaborators only; no UI layout, FFI date/markdown, full app or simulator claim'}
  (output/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');return record
MUTANTS=[('ignore-user-policy',SOURCES[0],'var preview: LinkPreview? = linkPreviews ?','var preview: LinkPreview? = true ?'),('ignore-cache-policy',SOURCES[1],'if !policyChanged,','if '),('mandatory-row-skips',SOURCES[-1],'return try XCTUnwrap(values.first, "Expected a user row")','if let value = values.first { return value }; throw XCTSkip("no user row")')]
def main():
 ap=argparse.ArgumentParser(description=__doc__);ap.add_argument('--root',type=Path,default=ROOT);ap.add_argument('--reference-root',type=Path);ap.add_argument('--output',type=Path,default=HERE/'generated');ap.add_argument('--mutants',action='store_true');args=ap.parse_args()
 if sys.platform!='darwin':raise SystemExit('Requires macOS/Xcode host Swift/XCTest; no skipped runtime check is reported as a pass')
 tree=args.root.resolve();reference=(args.reference_root or tree).resolve();output=args.output.resolve();normal=check(tree,reference,output/'normal');assert normal['pass']==12 and normal['fail']==0 and normal['helper']=={'tests':2,'failures':0,'succeeded':True},'Actual preview/helper regression detected'
 results=[]
 if args.mutants:
  for name,relative,before,after in MUTANTS:
   with tempfile.TemporaryDirectory(prefix='arbos-preview-mutant-')as raw:
    temporary=Path(raw)
    for p in SOURCES:dest=temporary/p;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(tree/p,dest)
    source=temporary/relative;text=source.read_text();expected=2 if name=='ignore-cache-policy'else 1;assert text.count(before)==expected;source.write_text(text.replace(before,after))
    record=check(temporary,reference,output/name);detected=record['fail']>0 or record['helper']['failures']>0;assert detected,name;results.append({'name':name,'detected':detected,'row_failures':record['fail'],'helper_failures':record['helper']['failures']})
 print(json.dumps({'row_checks':12,'helper_tests':2,'passed':14,'failed':0,'mutants':results}))
if __name__=='__main__':main()
