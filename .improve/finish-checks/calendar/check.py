"""Actual Foundation date methods/tests; no app, network, clocks or host timezone mutations."""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile,time
HERE=Path(__file__).resolve().parent;P='apps/ios/Arbos/'
BASELINE_METHOD_HASHES={'dayLabel': '45da4a005309bc764d7d1316c058d691fc39a13426e2e273a572f6ddb549a0ba', 'stamp': 'e0033958e8a858feb9929f17ac782d5caf8f7b503986b91f2865a9ec2c85516c'}
def method(text,name,static=True):
 prefix=r'^    (?:private )?'+('static 'if static else'')+r'func '+name+r'\(';matches=list(re.finditer(prefix,text,re.M));assert len(matches)==1,(name,len(matches));at=matches[0].start();return text[at:text.index('\n    }',matches[0].end())+6]
def main():
 p=argparse.ArgumentParser();p.add_argument('--tree',type=Path,required=True);p.add_argument('--tests',type=Path);p.add_argument('--out',type=Path,required=True);a=p.parse_args();tree=a.tree.resolve();tests=a.tests.resolve()if a.tests else tree;out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);paths=[tree/(P+'Arbos/UI/LibraryList.swift'),tree/(P+'Arbos/UI/BlockedState.swift'),tests/(P+'ArbosTests/LibraryListTests.swift'),tests/(P+'ArbosTests/BlockedStateTests.swift')];lib,blocked,libtests,blockedtests=[p.read_text()for p in paths];hashes={str(i)+':'+str(p.relative_to(tree if i<2 else tests)):hashlib.sha256(p.read_bytes()).hexdigest()for i,p in enumerate(paths)};old=(HERE/'baseline-date-methods.swift').read_text();assert {name:hashlib.sha256(method(old,name).encode()).hexdigest() for name in BASELINE_METHOD_HASHES} == BASELINE_METHOD_HASHES, 'baseline method fixture changed';current='\n'.join(method(lib,n)for n in ['dayLabel','stamp']);baseline='\n'.join(method(old,n)for n in ['dayLabel','stamp']);blocked_code='\n'.join(method(blocked,n)for n in ['who','mutedUntilLabel','refusalSentence']);clock=method(blockedtests,'testTheWords',False);clock=clock[clock.index('        // Mute-until label'):];clock='    func clockWords() {\n'+clock;cal=libtests[libtests.index('    private var cal:'):libtests.index('    private func item(')];lib_cases=method(libtests,'testDayLabelsAreTheHomeListsWords',False)+'\n'+method(libtests,'testInjectedCalendarOwnsDayBoundariesAndClock',False);support=r'''
import Foundation
var failures:[[String:String]]=[];var assertions=0;var group=""
func XCTAssertEqual<T:Equatable>(_ actual:@autoclosure()->T,_ expected:@autoclosure()->T,_ message:String="") {assertions+=1;let a=actual(),b=expected();if a != b{failures.append(["group":group,"actual":String(describing:a),"expected":String(describing:b)])}}
''';tail=r'''
struct Checks {
CAL
CASES
CLOCK
}
let checks=Checks()
group="existing-library-words";checks.testDayLabelsAreTheHomeListsWords()
group="opposing-zone-boundaries";checks.testInjectedCalendarOwnsDayBoundariesAndClock()
group="blocked-sentence-fixture";checks.clockWords()
group="unchanged-current-zone-control"
let now=Date(timeIntervalSince1970:1_793_188_800)
for days in [0.0,1.0,3.0,400.0]{let date=now.addingTimeInterval(-days*86_400);XCTAssertEqual(LibraryList.dayLabel(date,today:now,calendar:.current),BaselineLibrary.dayLabel(date,today:now,calendar:.current));XCTAssertEqual(LibraryList.stamp(Int64(date.timeIntervalSince1970*1000),now:now,calendar:.current),BaselineLibrary.stamp(Int64(date.timeIntervalSince1970*1000),now:now,calendar:.current))}
print(String(data:try!JSONSerialization.data(withJSONObject:["assertions":assertions,"passed":assertions-failures.count,"failed":failures.count,"failures":failures,"current_zone":TimeZone.current.identifier],options:[.sortedKeys]),encoding:.utf8)!);exit(failures.isEmpty ? 0:1)
''';fixture=support+'\nenum LibraryList{\n'+current+'\n}\nenum BaselineLibrary{\n'+baseline+'\n}\nenum BlockedState{\n'+blocked_code+'\n}\n'+tail.replace('CAL',cal).replace('CASES',lib_cases).replace('CLOCK',clock);tick=time.monotonic()
 with tempfile.TemporaryDirectory(prefix='arbos-calendar-')as raw:
  tmp=Path(raw);(tmp/'main.swift').write_text(fixture);env={'PATH':'/usr/bin:/bin','HOME':raw,'TMPDIR':raw};c=subprocess.run(['swiftc',str(tmp/'main.swift'),'-o',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=90);(out/'compile.log').write_text(c.stdout+c.stderr);record={'sources':hashes,'fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'compile_exit':c.returncode}
  if c.returncode==0:
   p=subprocess.run(['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)',str(tmp/'check')],env=env,capture_output=True,text=True,timeout=10);record.update(exit_code=p.returncode,results=json.loads(p.stdout));(out/'test.log').write_text(p.stdout+p.stderr)
  record['seconds']=time.monotonic()-tick;(out/'GATES.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record));assert c.returncode==0 and record['results']['assertions']==29;exit(record['results']['failed']!=0)
if __name__=='__main__':main()
