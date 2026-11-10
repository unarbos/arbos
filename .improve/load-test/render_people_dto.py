#!/usr/bin/env python3
"""Render retained whole-handler measurements as standalone HTML and PNG."""
from pathlib import Path
from html import escape
from io import StringIO
import json
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
HERE=Path(__file__).resolve().parent

def table(headings,rows):
    return '<div class="table"><table><thead><tr>'+''.join('<th>'+escape(v)+'</th>'for v in headings)+'</tr></thead><tbody>'+''.join('<tr>'+''.join('<td>'+escape(str(v))+'</td>'for v in row)+'</tr>'for row in rows)+'</tbody></table></div>'

def render():
    plt.rcParams.update({"svg.fonttype":"none", "svg.hashsalt":"arbos-scale-v1"})
    data=json.loads((HERE/'PEOPLE_DTO_RESULTS.json').read_text());stats=data['summary'];sql=data['sql'];profiles=data['raw']['profiles'];x=[s['scale']for s in stats]
    fig,axes=plt.subplots(1,2,figsize=(12,5),layout='constrained')
    for arm,color in [('before','#b45f24'),('after','#197f77')]:
        median=[s[arm]['p50_ns']/1e6 for s in stats];p95=[s[arm]['p95_ns']/1e6 for s in stats]
        axes[0].plot(x,median,marker='o',color=color,label=arm);axes[0].fill_between(x,median,p95,color=color,alpha=.16)
        axes[1].plot(x,[s[arm]['counters']['visits']for s in sql],marker='o',color=color,label=arm)
    axes[0].set(ylabel='Milliseconds per handler + JSON body',title='Actual Rust handler · p50 lines, p50–p95 shading')
    axes[1].set(ylabel='SQLite logical row visits (NVISIT)',title='Actual captured SELECTs · separate instrumented run')
    for ax in axes:
        ax.set(xscale='log',yscale='log',xlabel='Generated group chats (log scale)');ax.set_xticks(x,[f'{n:,}'for n in x]);ax.grid(alpha=.2,which='both');ax.legend()
    fig.suptitle('People chat list: batch profiles + index live-device owner\n21 randomized pairs/scale · workspace opt0 · warm local workers',fontsize=13)
    text=StringIO();fig.savefig(text,format='svg',metadata={'Date':None},bbox_inches='tight',pad_inches=.15);fig.savefig(HERE/'people-dto.png',dpi=160,bbox_inches='tight',pad_inches=.15);plt.close(fig)
    timings=[];work=[]
    for s,c,p in zip(stats,sql,profiles):
        timing=[f"{s['scale']:,}"]
        for arm in ['before','after']:timing.extend(f"{s[arm][key+'_ns']/1e6:.3f}"for key in ['p50','p95','p99'])
        timing.extend([f"{s['before']['p50_ns']/s['after']['p50_ns']:.2f}×",'–'.join(f'{v/1e6:.3f}'for v in s['paired_median_saved_95_ci_ns']),f"{s['two_sided_sign_test_p']:.3g}"]);timings.append(timing)
        work.append([f"{s['scale']:,}",*[f"{c[arm]['counters'][key]:,}"for key in ['statements','visits','vm_steps']for arm in ['before','after']],f"{p['after']['output_bytes']:,}",p['after']['output_sha256'][:12]+'…'])
    svg=text.getvalue();svg=svg[svg.index('<svg'):].replace('<svg ','<svg role="img" aria-label="Before and after whole native handler latency and SQLite logical visits, 1 to 1000 group chats" ',1)
    page='''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Arbos whole chat-list profile</title><style>body{font:16px/1.5 system-ui,sans-serif;color:#24332f;max-width:1240px;margin:2rem auto;padding:0 1rem}svg{width:100%;height:auto}.table{overflow:auto}table{border-collapse:collapse;width:100%;font-size:13px;font-variant-numeric:tabular-nums}th,td{padding:.6rem;text-align:right;border-bottom:1px solid #ccd6d0}th:first-child,td:first-child{text-align:left}.note{padding:1rem;background:#eef5f1;border-left:4px solid #197f77}</style><h1>Whole people chat-list profile</h1><p class="note">The real Rust handler now loads public profiles once per request. A live-device user-id index avoids scanning other accounts. Complete JSON bytes match at all four scales. Median local work falls0.945→0.749ms for one group and1832.479→321.528ms for1,000 groups. These are database/JSON measurements, not screen or network latency.</p>'''+svg
    page+=table(['Rooms','Before p50ms','Before p95ms','Before p99ms','After p50ms','After p95ms','After p99ms','p50 ratio','Paired saving95%CI ms','Sign-test p'],timings)
    page+='<h2>Work and complete output</h2>'+table(['Rooms','Before SQL','After SQL','Before visits','After visits','Before VMsteps','After VMsteps','Body bytes (same)','Same body SHA256 prefix'],work)
    page+='''<p>Each group has24 humans and2 agents; groups share three non-viewer humans and20 more within cohorts of10 rooms. Agent owners include a person absent from the roster. Each room has20 retained opaque rows and5 announced events; legacy/invited people, device presence, directed blocks, read cursors and archive marks remain in the generated relationships. Payloads are SQL fixtures, not valid Loro updates. The largest body is9.46MB; the complete-list API necessarily retains output work proportional to room count.</p><p>21 randomized pairs per scale, seed73011. Intervals use2,000 seeded paired-median bootstrap resamples; exact two-sided sign tests exclude ties. All four saving intervals are positive; Bonferroni-adjusted p across four comparisons is≤3.82e-6. Timings include the actual Rust handler and JSON body, with tracing disabled, workspace opt0/dependencies opt2 and warm persistent workers. There is no CPU affinity/exclusive-host, HTTP transport, GUI or production-data claim. SQL plans and real summed NVISIT are recorded separately through cached root-lock SQLite3.46; logical visits are not physical disk reads.</p><p>Sources are pinned to the pre-change database phase d499090b and batching/device-index commit b96ef1f90. A separately evaluated room-activity index is not included in these measurements. Per-room metadata queries remain visible: at1,000 rooms the chain uses8,005 statements, down from93,822. No claim of a constant-query complete list is made.</p><p>Re-run people_dto.py using a fresh output directory and cached toolchains. Both source snapshots have fresh HOME, separate cleaned targets and OS-enforced network denial. Full samples, compiled source hashes, exact output hashes and query plans are in <a href="PEOPLE_DTO_RESULTS.json">PEOPLE_DTO_RESULTS.json</a>.</p></html>'''
    (HERE/'people-dto.html').write_text('\n'.join(line.rstrip()for line in page.splitlines())+'\n')

if __name__=='__main__':render()
