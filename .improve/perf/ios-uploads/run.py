"""Reproduce bounded upload contracts and local Foundation transport latency/RSS."""
from pathlib import Path
import argparse
import json
import math
import random
import re
import statistics
import subprocess
import sys
import time
import check
import profile
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]

def quantile(values, probability):
    values = sorted(values)
    index = (len(values) - 1) * probability
    low, high = math.floor(index), math.ceil(index)
    return values[low] if low == high else values[low] * (high-index) + values[high] * (index-low)


def summarize(rows, randomizer):
    old = [r['before']['milliseconds'] for r in rows]
    new = [r['after']['milliseconds'] for r in rows]
    differences = [a-b for a, b in zip(old, new)]
    bootstraps = [statistics.mean(randomizer.choices(differences, k=len(rows))) for _ in range(10000)]
    n = sum(d != 0 for d in differences)
    k = min(sum(d > 0 for d in differences), sum(d < 0 for d in differences))
    p = min(1, 2*sum(math.comb(n, i) for i in range(k+1))/2**n)
    result = {label: {'p50_ms': quantile(values, .5), 'p95_ms': quantile(values, .95),
                      'p99_ms': quantile(values, .99), 'range_ms': [min(values), max(values)],
                      'mean_ms': statistics.mean(values),
                      'median_max_rss_bytes': statistics.median(r[label]['max_rss_bytes'] for r in rows)}
              for label, values in [('before', old), ('after', new)]}
    result.update(pairs=len(rows), mean_saved_ms=statistics.mean(differences),
                  mean_saved_bootstrap95_ms=[quantile(bootstraps, .025), quantile(bootstraps, .975)],
                  bootstrap_resamples=10000, sign_test_two_sided_p=p,
                  p50_ratio=statistics.median(old)/statistics.median(new),
                  p50_percent_saved=(1-statistics.median(new)/statistics.median(old))*100)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tree', type=Path, default=ROOT)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--samples', type=int, choices=[21, 41, 101], default=21)
    parser.add_argument('--check-only', action='store_true')
    parser.add_argument('--composition', action='store_true', help='Also require the six Phase4 ownership contracts')
    args = parser.parse_args()
    if sys.platform != 'darwin':
        raise SystemExit('Requires macOS, Apple Swift, sandbox-exec and time -l')
    tree, out = args.tree.resolve(), args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    for label in ('before', 'after'):
        target = out/(label+'-checks')
        target.mkdir()
        check.prepare(tree, target, baseline=label == 'before', composition=args.composition)
    contract = out/'contract'
    command = [sys.executable, '-B', str(HERE/'check_contract.py'), '--tree', str(tree), '--out', str(contract)]
    proc = subprocess.run(command, text=True, capture_output=True, timeout=150)
    (out/'contract.driver.log').write_text(proc.stdout+proc.stderr)
    assert proc.returncode == 0
    gate = json.loads((contract/'GATES.json').read_text())
    assert gate['results']['passed'] == 4 and gate['results']['failed'] == 0
    manifests = {}
    for label in ('before', 'after'):
        target = out/(label+'-transport')
        target.mkdir()
        profile.prepare(tree, None, target, baseline=label == 'before')
        manifests[label] = json.loads((target/'GATES.json').read_text())
    result = {'sources': manifests, 'seed': 91208, 'samples': [],
              'scope': '17 composition cases per version' if args.composition else '11 performance cases per version',
              'helper_contracts': 4,
              'scenario': '4 synthetic attachments; actual selected send branches and raw PUT; Foundation URLSession; intercept-all URLProtocol requested 8ms reply delay; network denied; oracle retains body copies for byte verification'}
    if not args.check_only:
        randomizer = random.Random(91208)
        env = {'PATH': '/usr/bin:/bin', 'HOME': str(out), 'TMPDIR': str(out)}
        tick = time.monotonic()
        scenarios = [('people-small', 64, 'people'), ('project-1MiB', 1048576, 'project'), ('project-8MiB', 8*1048576, 'project')]
        for name, size, kind in scenarios:
            for index in range(args.samples):
                order = ['before', 'after']
                randomizer.shuffle(order)
                pair = {'scenario': name, 'bytes_per_attachment': size, 'people': kind == 'people', 'index': index, 'order': order}
                for label in order:
                    command = ['/usr/bin/time', '-l', '/usr/bin/sandbox-exec', '-p', '(version 1)(allow default)(deny network*)',
                               str(out/(label+'-transport')/'upload-profile'), str(size), kind]
                    proc = subprocess.run(command, env=env, text=True, capture_output=True, timeout=20)
                    assert proc.returncode == 0, 'Synthetic upload process failed'
                    value = json.loads(proc.stdout)
                    assert value['valid'] and value['put_requests'] == 4 and value['active_after'] == 0
                    assert value['peak'] == (1 if label == 'before' else 2)
                    rss = re.search(r'^\s*(\d+)\s+maximum resident set size\s*$', proc.stderr, re.M)
                    assert rss, 'macOS RSS counter unavailable'
                    pair[label] = {'milliseconds': value['milliseconds'], 'max_rss_bytes': int(rss[1]),
                                   'peak': value['peak'], 'put_requests': value['put_requests']}
                result['samples'].append(pair)
        result['seconds'] = time.monotonic()-tick
        bootstrap = random.Random(91209)
        result['statistics'] = {name: summarize([r for r in result['samples'] if r['scenario'] == name], bootstrap)
                                for name in sorted({r['scenario'] for r in result['samples']})}
        result['bootstrap'] = {'seed': 91209, 'resamples': 10000, 'scenario_order': sorted(result['statistics'])}
    (out/'RESULTS.json').write_text(json.dumps(result, indent=2)+'\n')
    print(json.dumps({'pairs': len(result['samples']), 'statistics': result.get('statistics')}, indent=2))


if __name__ == '__main__':
    main()
