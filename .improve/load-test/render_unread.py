#!/usr/bin/env python3
"""Render every retained unread-index tradeoff; the candidate was not adopted."""
from html import escape
from io import StringIO
import json
from pathlib import Path

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

HERE = Path(__file__).resolve().parent
CASES = ('people_sparse', 'people_broad', 'mixed_sparse', 'mixed_broad')
LABELS = ('People history · sparse ACL', 'People history · broad ACL',
          'Mixed people/legacy · sparse ACL', 'Mixed people/legacy · broad ACL')


def table(headings, rows):
    return '<div class="table"><table><thead><tr>' + ''.join('<th>' + escape(h) + '</th>' for h in headings) + '</tr></thead><tbody>' + ''.join('<tr>' + ''.join('<td>' + escape(str(c)) + '</td>' for c in row) + '</tr>' for row in rows) + '</tbody></table></div>'


def render():
    data = json.loads((HERE / 'UNREAD_INDEX_RESULTS.json').read_text())
    fig, axes = plt.subplots(2, 2, figsize=(12, 8), layout='constrained')
    for ax, case, title in zip(axes.flat, CASES, LABELS):
        records = [e for e in data['experiments'] if e['case'] == case]
        for arm, label, color in (('before', 'original', '#405d78'), ('after', 'candidate', '#b85e26')):
            x = [e['scale'] for e in records]
            median = [e['summary'][arm]['p50_ns'] / 1e6 for e in records]
            p95 = [e['summary'][arm]['p95_ns'] / 1e6 for e in records]
            ax.plot(x, median, 'o-', label=label, color=color)
            ax.fill_between(x, median, p95, color=color, alpha=.16)
        ax.set(title=title, xscale='log', yscale='log', xlabel='Declared data scale', ylabel='Milliseconds (log scale)')
        ax.set_xticks([1, 10, 100, 1000], ['1×', '10×', '100×', '1000×'])
        ax.grid(True, which='both', alpha=.2)
        ax.legend()
    fig.suptitle('Unread partial-index experiment · candidate withheld\nLines p50 · shading p50–p95 · 101 randomized pairs per case/scale', fontsize=14)
    svg = StringIO()
    fig.savefig(svg, format='svg', bbox_inches='tight', pad_inches=.15)
    fig.savefig(HERE / 'unread-index.png', dpi=150, bbox_inches='tight', pad_inches=.15)
    plt.close(fig)
    timings, work = [], []
    for e in data['experiments']:
        summary = e['summary']
        cells = [e['case'], f"{e['scale']}×"]
        for arm in ('before', 'after'):
            cells.extend(f"{summary[arm][p + '_ns'] / 1e6:.3f}" for p in ('p50', 'p95', 'p99'))
        cells += ['–'.join(f'{v / 1e6:.3f}' for v in summary['paired_median_saved_95_ci_ns']),
                  f"{summary['two_sided_sign_test_p']:.3g}"]
        timings.append(cells)
        a, b = e['counters']['before'], e['counters']['after']
        fixture = e['fixtures']['after']
        work.append([e['case'], f"{e['scale']}×", f"{e['result_rows']:,}",
                     f"{a['logical_row_visits']:,}", f"{b['logical_row_visits']:,}",
                     f"{a['vm_steps']:,}", f"{b['vm_steps']:,}",
                     f"{fixture['index_added_bytes']:,}", f"{fixture['index_build_ns_single_observation'] / 1e6:.3f}"])
    page = '''<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Arbos unread index tradeoff: withheld</title>
<style>body{font:16px system-ui,sans-serif;max-width:1200px;margin:2rem auto;padding:0 1rem;color:#202a30;line-height:1.5}svg{width:100%;height:auto}table{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}th,td{padding:.6rem;border-bottom:1px solid #ccd2d6;text-align:right}th:first-child,td:first-child{text-align:left}.table{overflow-x:auto}pre{white-space:pre-wrap;overflow-wrap:anywhere;background:#f1f5f6;padding:1rem}</style>
<h1>Unread index candidate withheld</h1><p>A partial index on project-tagged events helps
sparse accounts but regresses every broad-access case. The product schema and unread SELECT
stay unchanged. At 1000× mixed history, sparse p50 improves 50.542→0.110 ms; broad p50
regresses 144.332→158.781 ms. A sparse-only chart would conceal the tradeoff.</p>
<p>All 16 cases use 101 randomized pairs, 5 warmups per arm, fresh identical local databases and
the exact unchanged source SELECT. Full ordered rows agree through bundled SQLite 3.46.0
and independent seed SQLite. Python ctypes binding/column consumption and scanstatus overhead
are included; Rust mapping, mutex contention, writers, network and UI are excluded.
These are declared synthetic distributions, not observed production.</p>'''
    image = svg.getvalue()[svg.getvalue().index('<svg'):]
    page += image.replace('<svg ', '<svg role="img" aria-label="Four panels show sparse improvements and broad regressions for the unread index candidate" ', 1)
    page += table(['Case', 'Scale', 'Original p50 ms', 'Original p95 ms', 'Original p99 ms', 'Candidate p50 ms', 'Candidate p95 ms', 'Candidate p99 ms', 'Saved median 95% CI ms', 'Sign-test p'], timings)
    page += '''<p>Negative saving intervals mean the candidate is slower. Intervals use a seeded
2,000-resample paired-median bootstrap; p-values use an exact two-sided sign test excluding ties.
This is one coordinated local warm single-reader run, not production confidence bounds.</p>
<h2>Logical work and index cost</h2>'''
    page += table(['Case', 'Scale', 'Returned rows', 'Original logical visits', 'Candidate logical visits', 'Original VM steps', 'Candidate VM steps', 'Added index bytes', 'Single creation observation ms'], work)
    page += '''<p>The index enables MULTI-INDEX OR for sparse scope. Broad scope visits the same
rows and needs temporary grouping, causing a regression. Mixed history adds a valid legacy
project chat with 20 events per group-agent project. Broad accounts join every people group
and every mixed legacy project; sparse accounts retain the original two-group/two-project
scope. NULL, own/blocked senders, read marks and archive predicates are preserved.</p>
<p>The largest two-fixture pair is 108,072,960 bytes, below 300 MiB. Index creation uses seed
SQLite and is one observation per fixture, rather than a production startup or write benchmark.
The candidate's native fresh/reopen/full-row checks passed privately. Only two useful native
ACL/full-row characterization tests are retained for the unchanged product query; index-specific
existence tests are excluded. The reusable profiler applies the captured candidate only to new
temporary files. No production database, provider or credentials were used.</p>
<h2>Experimental index and unchanged query</h2>'''
    page += '<pre>' + escape(data['index'] + '\n\n' + data['query']) + '</pre></html>'
    (HERE / 'unread-index.html').write_text('\n'.join(line.rstrip() for line in page.splitlines()) + '\n')


if __name__ == '__main__':
    render()
