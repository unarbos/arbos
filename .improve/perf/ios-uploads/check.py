"""Performance contracts (11), or explicit P4 ownership composition contracts (17)."""
from pathlib import Path
import argparse
import hashlib
import json
import subprocess
import tempfile
import time
import fixture
import support
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def prepare(tree, out, baseline=False, composition=False):
    ctx = fixture.context(tree, baseline)
    if composition and not ctx['owned']:
        raise ValueError('Composition checks require the Phase4 AccountWork source')
    text = support.F
    if ctx['owned']:
        text += support.method(ctx['api'], 'bound')+'\n'
    text += '\n}\n'+fixture.declarations(ctx)+fixture.model(ctx)+fixture.branches(ctx)
    tests = support.T
    if not composition:
        # Performance command never runs/claims the cross-account fixture class.
        # Those six assertions remain mandatory in the separate --composition gate.
        tests = '\n'.join(line for line in tests.splitlines()
                          if 'queued-A-never-uses-B' not in line and 'for failed in [false,true]{' not in line)+'\n'
    text += tests.replace('PEAK', '2' if 'func uploadAttachments(' in ctx['selected'] else '1')
    tick = time.monotonic()
    with tempfile.TemporaryDirectory(prefix='arbos-upload-check-') as raw:
        tmp = Path(raw)
        (tmp/'main.swift').write_text(text)
        env = {'PATH': '/usr/bin:/bin', 'HOME': raw, 'TMPDIR': raw}
        proc = subprocess.run(['swiftc', '-swift-version', '5', '-parse-as-library', str(tmp/'main.swift'), '-o', str(tmp/'check')],
                              env=env, text=True, capture_output=True, timeout=120)
        (out/'compile.log').write_text(proc.stdout+proc.stderr)
        row = {'sources': ctx['hashes'], 'fixture_sha256': hashlib.sha256(text.encode()).hexdigest(),
               'compile_exit': proc.returncode, 'scope': 'composition' if composition else 'performance'}
        if proc.returncode == 0:
            proc = subprocess.run(['/usr/bin/sandbox-exec', '-p', '(version 1)(allow default)(deny network*)', str(tmp/'check')],
                                  env=env, text=True, capture_output=True, timeout=20)
            (out/'tests.log').write_text(proc.stdout+proc.stderr)
            row.update(exit_code=proc.returncode, results=json.loads(proc.stdout) if proc.stdout else {})
        row['seconds'] = time.monotonic()-tick
        assert fixture.stable(ctx)
        (out/'GATES.json').write_text(json.dumps(row, indent=2)+'\n')
        print(json.dumps(row), flush=True)
        expected = 17 if composition else 11
        assert row['compile_exit'] == 0 and row['exit_code'] == 0
        assert row['results']['total'] == expected and row['results']['failed'] == 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tree', type=Path, default=ROOT)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--baseline', action='store_true')
    parser.add_argument('--composition', action='store_true')
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    prepare(args.tree.resolve(), out, args.baseline, args.composition)


if __name__ == '__main__':
    main()
