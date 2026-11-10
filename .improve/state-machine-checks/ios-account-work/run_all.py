"""Compile and run selected real iOS methods against synthetic local effects."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
METHODS = {
    'directory': 5, 'registry': 6, 'machine': 6, 'pins': 6,
    'poll_data': 9, 'people': 5, 'servers': 8, 'store_docs': 4,
    'matrix_entry': 4, 'secrets': 9, 'push_admission': 5,
    'feedback_capture': 5, 'startup': 6, 'notification': 6,
    'push_registration': 6, 'draft_sends': 9, 'reset': 9,
    'uploads': 11, 'identity': 6, 'media': 8, 'memo': 5,
    'forward': 6, 'actions': 19, 'connectors': 6, 'ui_identity': 4,
    'firstlogin': 5, 'people_actions': 12, 'avatar_state': 5,
    'feedback_send': 5, 'fun_picker': 7, 'final_edges': 7,
    'connect_reveal': 3, 'raw_api': 16,
}


def hashes(tree):
    paths = sorted((tree / 'apps/ios/Arbos').rglob('*.swift'))
    paths += [tree / 'apps/ios/Arbos/Arbos.xcodeproj/project.pbxproj']
    return {str(p.relative_to(tree)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in paths}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tree', type=Path, default=ROOT)
    parser.add_argument('--out', type=Path, required=True,
                        help='Fresh output directory, never overwritten')
    parser.add_argument('--only', choices=[*METHODS, 'settings', 'servers', 'profile'])
    args = parser.parse_args()
    if sys.platform != 'darwin':
        raise SystemExit('These source adapters require macOS, Swift and sandbox-exec')
    tree, out = args.tree.resolve(), args.out.resolve()
    before = hashes(tree)
    out.mkdir(parents=True, exist_ok=False)
    started = time.monotonic()
    rows = []
    env = {'PATH': os.environ.get('PATH', '/usr/bin:/bin'),
           'PYTHONDONTWRITEBYTECODE': '1'}

    def run(name, count, command, target):
        tick = time.monotonic()
        result = subprocess.run([sys.executable, '-I', '-B', *map(str, command)],
                                env=env, capture_output=True, text=True, timeout=180)
        (out / (name + '.driver.log')).write_text(result.stdout + result.stderr)
        value = json.loads((target / 'GATES.json').read_text()) if (target / 'GATES.json').exists() else {}
        counts = value.get('results', value.get('counts')) or {}
        passed, failed = counts.get('passed', 0), counts.get('failed', 0)
        ok = (result.returncode == 0 and value.get('compile_exit') == 0
              and value.get('exit_code') == 0 and passed == count and failed == 0)
        row = {'name': name, 'passed': passed, 'failed': failed, 'expected': count,
               'ok': ok, 'seconds': time.monotonic() - tick,
               'evidence': str(target.relative_to(out) / 'GATES.json')}
        rows.append(row)
        print(json.dumps(row), flush=True)
        if not ok:
            raise RuntimeError('Selected-source gate failed: ' + name)

    try:
        for name, count in METHODS.items():
            if args.only and args.only != name:
                continue
            target = out / name
            command = [HERE / 'methods' / ('check_' + name + '.py'),
                       '--tree', tree, '--out', target]
            if name == 'final_edges':
                command += ['--stage', 'all']
            run(name, count, command, target)

        # The server fixture reuses actual settings helpers. Running --only servers
        # therefore includes its settings-action prerequisite explicitly.
        if args.only in (None, 'settings', 'servers'):
            actions = out / 'settings-actions'
            run('settings-actions', 19, [HERE / 'settings/check_actions.py', '--tree', tree,
                                         '--out', actions], actions)
            if args.only != 'servers':
                views = out / 'settings-views'
                run('settings-views', 10, [HERE / 'settings/check_views.py', '--tree', tree,
                                          '--actions', actions, '--out', views], views)
            if args.only != 'settings':
                server = out / 'server-actions'
                run('server-actions', 18, [HERE / 'servers/check_mutations.py', '--tree', tree,
                                           '--helpers', actions / 'fixture.swift', '--out', server], server)
                views = out / 'server-views'
                run('server-views', 9, [HERE / 'servers/check_views.py', '--tree', tree,
                                       '--controllers', server, '--out', views], views)
        if args.only in (None, 'profile'):
            target = out / 'profile'
            run('profile', 7, [HERE / 'profile/check_controllers.py', '--tree', tree,
                               '--model', tree / 'apps/ios/Arbos/Arbos/Store/AppModel.swift',
                               '--lifetime', tree / 'apps/ios/Arbos/Arbos/Sync/WorkLifetime.swift',
                               '--out', target], target)
    finally:
        stable = before == hashes(tree)
        summary = {'sources': before, 'source_stable': stable, 'gates': rows,
                   'passed': sum(row['passed'] for row in rows),
                   'failed': sum(row['failed'] for row in rows),
                   'seconds': time.monotonic() - started}
        (out / 'GATES.json').write_text(json.dumps(summary, indent=2) + '\n')
        if not stable:
            raise RuntimeError('iOS source changed during gate')


if __name__ == '__main__':
    main()
