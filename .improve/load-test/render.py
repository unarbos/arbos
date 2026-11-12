#!/usr/bin/env python3
"""Render saved local measurements, with no network or browser dependencies."""
import html
import io
import json
from pathlib import Path

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

HERE=Path(__file__).resolve().parent


def render():
    data=json.loads((HERE/'RESULTS.json').read_text())
    plt.rcParams.update({'font.family':'DejaVu Sans','font.size':10,'svg.fonttype':'none'})
    fig,axes=plt.subplots(2,2,figsize=(12,8),layout='constrained')
    colors={'before':'#c36a37','after':'#247b73'}
    for col,experiment in enumerate(('history','registry')):
        records=[r for r in data['experiments'] if r['fixture']['experiment']==experiment]
        x=[r['fixture']['scale'] for r in records]
        for row,name in enumerate(('room_position_group','registry_n_plus_one_unchanged')):
            ax=axes[row,col]
            for arm in ('before','after'):
                summaries=[r['queries'][name]['summary'][arm] for r in records]
                med=[s['p50_ns']/1e6 for s in summaries]; p95=[s['p95_ns']/1e6 for s in summaries]
                label=arm if row==0 else f'{arm} label (same SQL)'
                ax.plot(x,med,marker='o',color=colors[arm],label=label)
                ax.fill_between(x,med,p95,color=colors[arm],alpha=.15)
            ax.set_xscale('log'); ax.set_yscale('log'); ax.set_xticks(x,labels=['1×','10×','100×','1000×'])
            ax.set_xlabel('Data scale (1000–1,000,000 retained rows)'); ax.set_ylabel('Milliseconds per call (log scale)')
            ax.set_title(('History growth · 10 rooms' if col==0 else 'Registry growth · 10–10,000 rooms')+'\n'+('Cache position query: full state → position only' if row==0 else 'Unchanged registry N+1 control'))
            ax.grid(alpha=.2,which='both'); ax.legend(fontsize=8)
    fig.suptitle('Local synthetic SQLite: remove history scan from cache-position checks\nAll panels in milliseconds · lines p50 · shading p50–p95',fontsize=15)
    fig.savefig(HERE/'latency.png',dpi=150,bbox_inches='tight',pad_inches=.15)
    out=io.StringIO(); fig.savefig(out,format='svg',bbox_inches='tight',pad_inches=.15); plt.close(fig)
    svg=out.getvalue().split('<svg',1)[1]; svg='<svg'+svg
    changed=[q['summary'] for r in data['experiments'] for q in r['queries'].values() if not q.get('unchanged_control')]
    assert all(s['paired_median_saved_95_ci_ns'][0]>0 for s in changed)
    max_p=max(s['two_sided_sign_test_p'] for s in changed)
    significance=f'All {len(changed)} changed-query comparisons have positive paired-median 95% intervals; maximum unadjusted p={max_p:.3g}, Bonferroni-adjusted across {len(changed)} comparisons ≤{max_p*len(changed):.3g}.'
    rows=[]
    for r in data['experiments']:
        for name,q in r['queries'].items():
            s=q['summary']; b=s['before']; a=s['after']; counts=q['counters']
            visits=(str(counts['visits'])+' (same SQL)') if q.get('unchanged_control') else f"{counts['before']['visits']} → {counts['after']['visits']}"
            ci=s['paired_median_saved_95_ci_ns']
            cells=[r['fixture']['experiment'],str(r['fixture']['scale'])+'×',name,
                   '/'.join(f'{b[k]/1e6:.3f}' for k in ('p50_ns','p95_ns','p99_ns')),
                   '/'.join(f'{a[k]/1e6:.3f}' for k in ('p50_ns','p95_ns','p99_ns')),
                   f'{ci[0]/1e6:.3f}–{ci[1]/1e6:.3f}',f"{s['two_sided_sign_test_p']:.2g}",visits]
            rows.append('<tr>'+''.join('<td>'+html.escape(c)+'</td>' for c in cells)+'</tr>')
    page='''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Arbos SQLite scale profile</title><style>body{font:16px/1.5 system-ui;color:#20352f;background:#fafbf8;max-width:1250px;margin:35px auto;padding:0 20px}h1{line-height:1.15}p{max-width:1000px}svg{width:100%;height:auto}.table{overflow:auto}table{border-collapse:collapse;font-size:12px;width:100%}td,th{text-align:left;padding:8px;border-bottom:1px solid #ccd7d0}th{background:#e9eee9}.note{border-left:4px solid #247b73;padding:12px;background:#edf5f0}</style><h1>SQLite at 1×, 10×, 100× and 1000×</h1><p class="note">The cache check needs head and floor. Reusing full room statistics scans every retained history row. Position-only SQL removes that scan. The separate unbounded registry still issues one roster query per chat and remains unchanged.</p>'''+svg+'''<p>Lines show p50; shading extends to p95. Group rooms are plotted; both DM and group queries appear in the table. Two independent synthetic axes each contain 1000–1,000,000 retained opaque rows. Half of rooms are two-human DMs; half are eight-member groups including an agent. The same owner belongs to every room, an intentional registry stress case. These are declared assumptions, not production proportions.</p><p>101 randomized before/after pairs per changed query; 21 pairs for the identical-SQL registry control. Seeded 2000-resample paired-median bootstrap 95% intervals and exact two-sided sign tests. '''+html.escape(significance)+''' No registry optimization is claimed.</p><div class="table"><table><thead><tr><th>Axis</th><th>Scale</th><th>Query</th><th>Before p50/p95/p99 ms</th><th>After p50/p95/p99 ms</th><th>Saved median95%CI ms</th><th>Sign-test p</th><th>Logical visits</th></tr></thead><tbody>'''+''.join(rows)+'''</tbody></table></div><p>Timings include preparing/binding/stepping/finalizing actual source SELECTs in C, using cached root-lock SQLite3.46.0 compiled with scanstatus+FTS5. NVISIT counts logical rows visited across loops, not physical reads. VM steps and exact query plans are in RESULTS.json. Counter calls run separately from latency samples, but scanstatus instrumentation remains compiled in. C consumes SQL columns; Rust row allocation/JSON mapping is not reproduced. PythonSQLite3.53.4 creates fixtures and checks returned values. Warm single-reader measurements omit concurrency, relay lock waits, valid Loro decoding, native/HTTP behavior and network latency. Observed timer granularity is1µs, so very small CIs are quantized. Maximum temporary database178MB; fixtures are deleted between scales. This is no whole-app/chat-latency claim.</p></html>'''
    (HERE/'latency.html').write_text('\n'.join(line.rstrip() for line in page.splitlines()) + '\n')
    print('Rendered latency.html and latency.png')


if __name__=='__main__': render()
