#!/usr/bin/env python3
"""Negative controls against isolated copies of actual pure shell functions."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

REPLAY = {'ask':'Ask','phase':'Claiming','abort':'Cancelled','badPid':'1',
          'badSince':'invalid','snapshot':'Retired','scope':'Explicit','tail':[]}
MUTANTS = [
    ('invalid-lease-not-counted', 89003, 'arbos-heavy-slot.sh',
     '    InvalidHeld|CappedHeld|ExemptHeld|HeldCleanupFailed) echo 1 ;;',
     '    InvalidHeld) echo 0 ;;\n    CappedHeld|ExemptHeld|HeldCleanupFailed) echo 1 ;;'),
    ('retired-lease-authorizes-boot', 89004, 'xcrun-slot-guard.sh',
     '    Ancestor:Owned) echo OwnedLease ;;',
     '    Explicit:Retired) echo ExplicitLiveLease ;;\n    Ancestor:Owned) echo OwnedLease ;;'),
]

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    root, output = args.root.resolve(), args.out.resolve()
    output.mkdir(parents=True, exist_ok=False)
    sources = ['arbos-heavy-slot.sh', 'gui-lock.sh', 'xcrun-slot-guard.sh']
    original = {name: digest(root/'scripts/runner'/name) for name in sources}
    control_start = time.monotonic()
    control = subprocess.run(['node', '--test', str(root/'harness/arbos-pi-ext/lease-state-laws.test.ts')],
        cwd=root, env={'PATH':os.environ.get('PATH','/usr/bin:/bin')},
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=90)
    (output/'control.log').write_text(control.stdout)
    assert control.returncode == 0 and '# pass 5' in control.stdout and '# fail 0' in control.stdout, 'Unmodified law control failed'
    control_result = {'exit_code':control.returncode,'wall_seconds':time.monotonic()-control_start,
                      'generated_laws':4,'generated_cases':1024,'protocol_tests':1}
    results = []
    for name, seed, filename, before, after in MUTANTS:
        with tempfile.TemporaryDirectory(prefix='arbos-lease-mutant-') as temp:
            tree = Path(temp)
            (tree/'harness/arbos-pi-ext').mkdir(parents=True)
            (tree/'scripts/runner/tests').mkdir(parents=True)
            for source in sources:
                shutil.copy2(root/'scripts/runner'/source, tree/'scripts/runner'/source)
            shutil.copy2(root/'scripts/runner/tests/lease_state_bridge.py', tree/'scripts/runner/tests')
            shutil.copy2(root/'harness/arbos-pi-ext/lease-state-laws.test.ts', tree/'harness/arbos-pi-ext')
            (tree/'harness/arbos-pi-ext/package.json').write_text('{"type":"module"}\n')
            (tree/'harness/arbos-pi-ext/node_modules').symlink_to((root/'harness/arbos-pi-ext/node_modules').resolve(), target_is_directory=True)
            source = tree/'scripts/runner'/filename
            text = source.read_text()
            assert text.count(before) == 1
            source.write_text(text.replace(before, after))
            record = {'name':name,'seed':seed,'original_sha256':original[filename],
                      'mutant_sha256':digest(source),'replace':{'before':before,'after':after}}
            for mode in ('generated','replay'):
                env = {'PATH':os.environ.get('PATH','/usr/bin:/bin'), 'ARBOS_LEASE_LAW':str(seed)}
                if mode == 'replay': env['ARBOS_LEASE_REPLAY'] = json.dumps(REPLAY,separators=(',',':'))
                start = time.monotonic()
                process = subprocess.run(['node','--test',str(tree/'harness/arbos-pi-ext/lease-state-laws.test.ts')],
                                         cwd=tree,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=90)
                (output/f'{name}-{mode}.log').write_text(process.stdout)
                assert process.returncode != 0, 'Mutant survived'
                if mode == 'generated':
                    assert 'ReturnedFalse' in process.stdout and 'and 1 shrink(s)' in process.stdout, process.stdout
                else:
                    assert 'canonical replay=' in process.stdout, process.stdout
                record[mode] = {'exit_code':process.returncode,'wall_seconds':time.monotonic()-start,
                                'log':f'{name}-{mode}.log'}
            record['counterexample'] = REPLAY
            results.append(record)
    assert original == {name:digest(root/'scripts/runner'/name) for name in sources}
    final = {'mutations_are_isolated':True,'actual_functions':True,'control':control_result,'results':results}
    (output/'RESULTS.json').write_text(json.dumps(final,indent=2)+'\n')
    print(json.dumps({'negative_controls':len(results),'each_generated_and_replayed':True}))

if __name__ == '__main__':
    main()
