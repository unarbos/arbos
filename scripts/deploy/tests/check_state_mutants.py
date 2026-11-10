#!/usr/bin/env python3
"""Negative controls against isolated copies of actual pure shell functions."""
import hashlib
import argparse
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

TREE = Path(__file__).resolve().parents[3]
REPLAY = {'outcome':'Unchanged','terminal':'Released','tail':[]}
PUBLICATION_REPLAY = {'kind':'relay','revision':0,'queued':'-','installed':'-','currentDefault':'-'}
MUTANTS = [
    ('superseded-request-acknowledged', 89701,
     '    Observed:Changed) echo Superseded ;;',
     '    Observed:Changed) echo Acknowledged ;;'),
    ('busy-lane-granted', 89702,
     '    Checking:Busy|Inspecting:Blocked) echo Deferred ;;',
     '    Checking:Busy|Inspecting:Blocked) echo Granted ;;'),
    ('terminal-lane-reopened', 89703,
     '    Deferred:*|Released:*) echo "$1" ;;',
     '    Deferred:*|Released:*) echo Granted ;;'),
    ('changed-publication-receipt-granted', 89704,
     '  if [[ "$1" == "$2" ]]; then echo Publish; else echo Superseded; fi',
     '  echo Publish'),
]

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--record-dir', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='arbos-deploy-mutant-results-') as directory:
        return check(args.record_dir or Path(directory))


def check(output):
    output.mkdir(parents=True, exist_ok=True)
    source_path = TREE/'scripts/deploy/deploy-state.sh'
    original = digest(source_path)
    control_start = time.monotonic()
    control = subprocess.run(['node', '--test', str(TREE/'harness/arbos-pi-ext/deploy-state-laws.test.ts')],
                             cwd=TREE, env={'PATH':os.environ.get('PATH','/usr/bin:/bin')},
                             stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=45)
    (output/'control.log').write_text(control.stdout)
    assert control.returncode == 0 and '# pass 5' in control.stdout, 'Original laws must pass before mutations'
    control_record = {'exit_code':control.returncode,'tests':5,'laws':4,'cases':1024,
                      'seconds':time.monotonic()-control_start,'log':'control.log'}
    results = []
    for name, seed, before, after in MUTANTS:
        replay = PUBLICATION_REPLAY if seed == 89704 else REPLAY
        with tempfile.TemporaryDirectory(prefix='arbos-lease-mutant-') as temp:
            tree = Path(temp)
            (tree/'harness/arbos-pi-ext').mkdir(parents=True)
            (tree/'scripts/deploy/tests').mkdir(parents=True)
            shutil.copy2(TREE/'scripts/deploy/deploy-state.sh', tree/'scripts/deploy/deploy-state.sh')
            shutil.copy2(TREE/'scripts/deploy/tests/deploy_state_bridge.py', tree/'scripts/deploy/tests/deploy_state_bridge.py')
            shutil.copy2(TREE/'harness/arbos-pi-ext/deploy-state-laws.test.ts', tree/'harness/arbos-pi-ext')
            (tree/'harness/arbos-pi-ext/package.json').write_text('{"type":"module"}\n')
            (tree/'harness/arbos-pi-ext/node_modules').symlink_to((TREE/'harness/arbos-pi-ext/node_modules').resolve(), target_is_directory=True)
            source = tree/'scripts/deploy/deploy-state.sh'
            text = source.read_text()
            assert text.count(before) == 1
            source.write_text(text.replace(before, after))
            record = {'name':name,'seed':seed,'original_sha256':original,
                      'mutant_sha256':digest(source),'replace':{'before':before,'after':after}}
            for mode in ('generated','replay'):
                env = {'PATH':os.environ.get('PATH','/usr/bin:/bin'), 'ARBOS_DEPLOY_LAW':str(seed)}
                if mode == 'replay': env['ARBOS_DEPLOY_REPLAY'] = json.dumps(replay,separators=(',',':'))
                start = time.monotonic()
                process = subprocess.run(['node','--test',str(tree/'harness/arbos-pi-ext/deploy-state-laws.test.ts')],
                                         cwd=tree,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=30)
                (output/f'{name}-{mode}.log').write_text(process.stdout)
                assert process.returncode != 0, 'Mutant survived'
                if mode == 'generated':
                    assert 'ReturnedFalse' in process.stdout, process.stdout
                    match = re.search(r'and ([01]) shrink\(s\)', process.stdout)
                    assert match, process.stdout
                    record['accepted_shrinks'] = int(match.group(1))
                    counterexample = re.search(r'Shrunk input:\s*(\{[\s\S]*?\})\s*Failure:', process.stdout)
                    assert counterexample and json.loads(counterexample.group(1)) == replay, process.stdout
                else:
                    assert 'canonical replay=' in process.stdout, process.stdout
                record[mode] = {'exit_code':process.returncode,'wall_seconds':time.monotonic()-start,
                                'log':f'mutants/{name}-{mode}.log'}
            record['counterexample'] = replay
            results.append(record)
    assert original == digest(source_path)
    final = {'mutations_are_isolated':True,'actual_functions':True,'original_sha256':original,'control':control_record,'results':results}
    (output/'RESULTS.json').write_text(json.dumps(final,indent=2)+'\n')
    print(json.dumps({'negative_controls':len(results),'each_generated_and_replayed':True}))

if __name__ == '__main__':
    raise SystemExit(main())
