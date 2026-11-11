#!/usr/bin/env python3
"""Render retained worker-index samples as standalone HTML and an export PNG."""
from html import escape
from io import StringIO
import json
from pathlib import Path

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

HERE = Path(__file__).resolve().parent


def render():
    result = json.loads((HERE / 'JOB_INDEX_RESULTS.json').read_text())
    experiments = result['experiments']
    fig, ax = plt.subplots(figsize=(9, 5), layout='constrained')
    for arm, color in (('before', '#b85e26'), ('after', '#187f7a')):
        x = [e['scale'] for e in experiments]
        median = [e['summary'][arm]['p50_ns'] / 1e6 for e in experiments]
        upper = [e['summary'][arm]['p95_ns'] / 1e6 for e in experiments]
        ax.plot(x, median, marker='o', label=arm, color=color)
        ax.fill_between(x, median, upper, alpha=.15, color=color)
    ax.set(xscale='log', yscale='log', xlabel='Data scale (100–100,000 jobs)',
           ylabel='Milliseconds per lookup (log scale)',
           title='Latest job for one worker · actual SQLite SELECT\n'
                 'Lines p50 · shading p50–p95 · 101 randomized pairs per scale')
    ax.set_xticks([1, 10, 100, 1000], ['1×', '10×', '100×', '1000×'])
    ax.grid(True, which='both', alpha=.2)
    ax.legend()
    svg = StringIO()
    fig.savefig(svg, format='svg', bbox_inches='tight', pad_inches=.15)
    fig.savefig(HERE / 'job-index.png', dpi=160, bbox_inches='tight', pad_inches=.15)
    plt.close(fig)
    rows = []
    for e in experiments:
        s = e['summary']
        cells = [f"{e['scale']}×", f"{e['fixture_before']['counts']['jobs']:,}"]
        for arm in ('before', 'after'):
            cells.extend(f"{s[arm][p + '_ns'] / 1e6:.3f}" for p in ('p50', 'p95', 'p99'))
        cells.extend([f"{s['two_sided_sign_test_p']:.3g}",
                      '–'.join(f'{n / 1e6:.3f}' for n in s['paired_median_saved_95_ci_ns'])])
        rows.append('<tr>' + ''.join(f'<td>{escape(c)}</td>' for c in cells) + '</tr>')
    headings = ['Scale', 'Jobs', 'Before p50 ms', 'Before p95 ms', 'Before p99 ms',
                'After p50 ms', 'After p95 ms', 'After p99 ms', 'Sign-test p',
                'Median saving 95% CI ms']
    page = '''<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Arbos worker lookup scale comparison</title>
<style>body{font:16px system-ui,sans-serif;max-width:1100px;margin:2rem auto;padding:0 1rem;color:#202a30}
svg{width:100%;height:auto}table{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}
th,td{padding:.6rem;border-bottom:1px solid #ccd2d6;text-align:right}th:first-child,td:first-child{text-align:left}
pre{white-space:pre-wrap;overflow-wrap:anywhere;background:#f1f5f6;padding:1rem}.table{overflow-x:auto}</style>
<h1>Latest job for one worker</h1><p>Generated local databases, bundled SQLite 3.46.0,
100–100,000 jobs; identical source SELECT, additive composite index only.
Warm single reader, with scanstatus instrumentation. Includes statement preparation
and Python column consumption; excludes Rust mapping, lock waits and native UI.
This does not measure app latency. Lines are p50; shading ends at p95.</p>'''
    page += svg.getvalue()[svg.getvalue().index('<svg'):]
    page += '<div class="table"><table><thead><tr>' + ''.join(f'<th>{h}</th>' for h in headings)
    page += '</tr></thead><tbody>' + ''.join(rows) + '</tbody></table></div>'
    page += '''<p>Intervals: seeded 2,000-resample paired median bootstrap. P-values:
exact two-sided sign test, excluding ties. One local timing run, not production
confidence bounds. Full raw pairs, logical row visits, plans, fixture sizes and
source hashes are retained in JOB_INDEX_RESULTS.json. Fresh and existing-store
DDL replay plus full-row equality (including timestamp ties, NULL and missing
worker) are checked separately; these are SQL checks, not native integration.</p>'''
    page += '<h2>Actual query and index</h2><pre>' + escape(result['query'] + '\n' + result['index']) + '</pre></html>'
    (HERE / 'job-index.html').write_text('\n'.join(line.rstrip() for line in page.splitlines()) + '\n')


if __name__ == '__main__':
    render()
