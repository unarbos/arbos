#!/usr/bin/env python3
"""Render retained roster-query samples as standalone HTML and a shareable PNG."""
from html import escape
from io import StringIO
import json
from pathlib import Path

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

HERE = Path(__file__).resolve().parent


def table(headings, rows):
    return '<div class="table"><table><thead><tr>' + ''.join(f'<th>{escape(h)}</th>' for h in headings) + '</tr></thead><tbody>' + ''.join('<tr>' + ''.join(f'<td>{escape(str(c))}</td>' for c in row) + '</tr>' for row in rows) + '</tbody></table></div>'


def render():
    result = json.loads((HERE / 'ROSTER_RESULTS.json').read_text())
    experiments = result['experiments']
    fig, ax = plt.subplots(figsize=(9, 5), layout='constrained')
    for arm, color in (('before', '#b85e26'), ('after', '#187f7a')):
        rooms = [e['fixture']['rooms'] for e in experiments]
        median = [e['summary'][arm]['p50_ns'] / 1e6 for e in experiments]
        upper = [e['summary'][arm]['p95_ns'] / 1e6 for e in experiments]
        ax.plot(rooms, median, marker='o', color=color, label=arm)
        ax.fill_between(rooms, median, upper, color=color, alpha=.15)
    ax.set(xscale='log', yscale='log', xlabel='Rooms visible to one synthetic owner (log scale)',
           ylabel='Milliseconds per roster query chain (log scale)',
           title='Chat roster batching · actual SQLite SELECTs\nLines p50 · shading p50–p95 ·101 randomized pairs per scale')
    ax.set_xticks([10, 100, 1000, 10000], ['10', '100', '1,000', '10,000'])
    ax.grid(True, which='both', alpha=.2)
    ax.legend()
    svg = StringIO()
    fig.savefig(svg, format='svg', bbox_inches='tight', pad_inches=.15)
    fig.savefig(HERE / 'roster.png', dpi=160, bbox_inches='tight', pad_inches=.15)
    plt.close(fig)
    timings, counters = [], []
    for e in experiments:
        summary = e['summary']
        cells = [f"{e['scale']}×", f"{e['fixture']['rooms']:,}"]
        for arm in ('before', 'after'):
            cells.extend(f"{summary[arm][p + '_ns'] / 1e6:.3f}" for p in ('p50', 'p95', 'p99'))
        cells.extend([f"{summary['before']['p50_ns'] / summary['after']['p50_ns']:.2f}×",
                      '–'.join(f'{v / 1e6:.3f}' for v in summary['paired_median_saved_95_ci_ns']),
                      f"{summary['two_sided_sign_test_p']:.3g}"])
        timings.append(cells)
        before, after = e['counters']['before'], e['counters']['after']
        counters.append([f"{e['fixture']['rooms']:,}", f"{before['statements']:,}", f"{after['statements']:,}",
                         f"{before['visits']:,}", f"{after['visits']:,}",
                         f"{before['vm_steps']:,}", f"{after['vm_steps']:,}", f"{after['returned']:,}"])
    page = '''<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Arbos chat roster batching scale comparison</title>
<style>body{font:16px system-ui,sans-serif;max-width:1120px;margin:2rem auto;padding:0 1rem;color:#202a30;line-height:1.5}svg{width:100%;height:auto}table{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}th,td{padding:.6rem;border-bottom:1px solid #ccd2d6;text-align:right}th:first-child,td:first-child{text-align:left}pre{white-space:pre-wrap;overflow-wrap:anywhere;background:#f1f5f6;padding:1rem}.table{overflow-x:auto}</style>
<h1>Chat roster batching</h1><p>Fresh generated local databases and bundled SQLite 3.46.0.
The original chat SELECT stays unchanged; one live-roster SELECT replaces one SELECT per chat.
At 10,000 rooms, warm SQL p50 falls 84.308→27.615 ms, with 10,001→2 statements.
Scope validation repeats in the batch query, increasing logical row visits 70,000→90,000.
Both arms return the same 60,000 chat/member rows.</p>
<p>These are declared synthetic relationships: half DM rooms with 2 human owners, half groups
with 7 humans and 1 agent,100 retained opaque rows per room, and one owner in every room.
The largest database is 175.94 MB. These are SQL workloads; Rust roster allocation/DTO mapping,
relay mutex waits, network and rendering are excluded. Complete list/detail DTO parity is
checked separately by actual native regressions. No app-latency claim is made.</p>'''
    content = svg.getvalue()[svg.getvalue().index('<svg'):]
    page += content.replace('<svg ', '<svg role="img" aria-label="Before and after SQL roster latency at 10 to 10,000 rooms" ', 1)
    page += table(['Scale', 'Rooms', 'Before p50 ms', 'Before p95 ms', 'Before p99 ms', 'After p50 ms', 'After p95 ms', 'After p99 ms', 'p50 ratio', 'Paired median saving 95% CI ms', 'Sign-test p'], timings)
    page += '<h2>Work and returned data</h2>' + table(['Rooms', 'Before statements', 'After statements', 'Before logical visits', 'After logical visits', 'Before VM steps', 'After VM steps', 'Rows returned by each arm'], counters)
    page += '''<p>Each scale uses 101 randomized before/after pairs and 5 warmups per arm.
Intervals are seeded 2,000-resample paired-median bootstrap intervals; p-values use an exact
two-sided sign test excluding ties. One coordinated local run measures warm, readonly,
single-reader behavior with scanstatus instrumentation. The baseline Cdriver copies chat IDs
to execute its per-room queries; the batched arm consumes 2 SELECTs. Native mapping and
concurrent writer latency are unmeasured. The new native method holds one connection lock
through its two queries; the previous method reacquired it between per-room queries.</p>
<p>Full raw samples, fixture counts, exact SQL, source hashes and query plans are retained
in ROSTER_RESULTS.json. Fresh databases are deleted after every scale; no existing database,
hosted service or credentials are used.</p><h2>Actual source queries</h2>'''
    page += '<pre>' + escape('\n\n'.join(result['queries']['before'] + [result['queries']['after'][1]])) + '</pre></html>'
    (HERE / 'roster.html').write_text('\n'.join(line.rstrip() for line in page.splitlines()) + '\n')


if __name__ == '__main__':
    render()
