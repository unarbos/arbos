"""Reproduce local Swift registry correctness and paired synthetic latency/RSS."""
from pathlib import Path
import argparse
import hashlib
import json
import math
import random
import re
import statistics
import subprocess
import sys
import time
import check

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def quantile(values, probability):
    values = sorted(values)
    index = (len(values) - 1) * probability
    low, high = math.floor(index), math.ceil(index)
    return values[low] if low == high else values[low] * (high-index) + values[high] * (index-low)


def summarize(rows):
    old = [r['before']['milliseconds'] for r in rows]
    new = [r['after']['milliseconds'] for r in rows]
    differences = [a-b for a, b in zip(old, new)]
    randomizer = random.Random(91207)
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
                  bootstrap_seed=91207, bootstrap_resamples=10000, sign_test_two_sided_p=p,
                  p50_ratio=statistics.median(old)/statistics.median(new),
                  p50_percent_saved=(1-statistics.median(new)/statistics.median(old))*100)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tree', type=Path, default=ROOT)
    parser.add_argument('--out', type=Path, required=True, help='New output directory')
    parser.add_argument('--check-only', action='store_true')
    parser.add_argument('--samples', type=int, choices=[21, 41, 101], default=21)
    args = parser.parse_args()
    if sys.platform != 'darwin':
        raise SystemExit('Requires Apple Swift, macOS sandbox-exec and time -l')
    tree, out = args.tree.resolve(), args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    gates = {}
    for name in ('before', 'after'):
        target = out/name
        target.mkdir()
        check.prepare(tree, target, baseline=name == 'before')
        gates[name] = json.loads((target/'GATES.json').read_text())
    assert gates['before']['results']['outputs'] == gates['after']['results']['outputs'], 'Typed normal/mixed outputs changed'
    result = {'scenario': '12 independent detail GETs, each with requested 8ms synthetic async delay; no HTTP/service',
              'before_sources': gates['before']['sources'], 'after_sources': gates['after']['sources'],
              'before_fixture_sha256': gates['before']['fixture_sha256'],
              'after_fixture_sha256': gates['after']['fixture_sha256'],
              'normal_output_sha256': hashlib.sha256(gates['before']['results']['outputs']['normal'].encode()).hexdigest(),
              'mixed_output_sha256': hashlib.sha256(gates['before']['results']['outputs']['mixed'].encode()).hexdigest(),
              'seed': 91206, 'samples': []}
    if not args.check_only:
        env = {'PATH': '/usr/bin:/bin', 'HOME': str(out), 'TMPDIR': str(out)}
        randomizer = random.Random(91206)
        tick = time.monotonic()
        for index in range(args.samples):
            order = ['before', 'after']
            randomizer.shuffle(order)
            pair = {'index': index, 'order': order}
            for label in order:
                command = ['/usr/bin/time', '-l', '/usr/bin/sandbox-exec', '-p',
                           '(version 1)(allow default)(deny network*)', str(out/label/'registry-check'), '--bench']
                proc = subprocess.run(command, env=env, capture_output=True, text=True, timeout=10)
                assert proc.returncode == 0, 'Registry process failed'
                value = json.loads(proc.stdout)
                assert value['output'] == gates['before']['results']['outputs']['normal'], 'Typed benchmark output changed'
                assert value['active_after'] == 0 and value['peak'] == (1 if label == 'before' else 4)
                assert len(value['requests']) == 13
                rss = re.search(r'^\s*(\d+)\s+maximum resident set size\s*$', proc.stderr, re.M)
                assert rss, 'macOS RSS counter unavailable'
                pair[label] = {'milliseconds': value['milliseconds'], 'max_rss_bytes': int(rss[1]),
                               'peak': value['peak'], 'requests': len(value['requests'])}
            result['samples'].append(pair)
        result['seconds'] = time.monotonic()-tick
        result['statistics'] = summarize(result['samples'])
    (out/'RESULTS.json').write_text(json.dumps(result, indent=2)+'\n')
    print(json.dumps({'correctness_cases_per_version': 9, 'pairs': len(result['samples']),
                      'statistics': result.get('statistics')}, indent=2))


if __name__ == '__main__':
    main()
