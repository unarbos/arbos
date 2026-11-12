#!/usr/bin/env python3
"""Render activity-index reads and write/storage tradeoffs from retained samples."""
from pathlib import Path
from io import StringIO
import json
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from render_people_dto import table
HERE=Path(__file__).resolve().parent

def render():
    plt.rcParams.update({"svg.fonttype":"none", "svg.hashsalt":"arbos-scale-v1"})
    data=json.loads((HERE/'ACTIVITY_RESULTS.json').read_text());experiments=data['experiments'];x=[r['fixture']['rows_per_room']for r in experiments]
    fig,axes=plt.subplots(1,3,figsize=(15,5),layout='constrained');colors={'before':'#b45f24','after':'#197f77'}
    for arm,color in colors.items():
        p50=[r['summary'][arm]['p50_ns']/1e6 for r in experiments];p95=[r['summary'][arm]['p95_ns']/1e6 for r in experiments]
        axes[0].plot(x,p50,marker='o',label=arm,color=color);axes[0].fill_between(x,p50,p95,color=color,alpha=.15)
        axes[1].plot(x,[r['counters'][arm]['visits']for r in experiments],marker='o',label=arm,color=color)
    axes[0].set(ylabel='Milliseconds per two-SELECT activity read',title='Read latency · p50 line / p50–p95 band')
    axes[1].set(ylabel='SQLite logical row visits (NVISIT)',title='Retained-history work · head + timestamp max')
    for ax in axes[:2]:
        ax.set(xscale='log',yscale='log',xlabel='Retained rows per room');ax.set_xticks(x,[f'{n:,}'for n in x]);ax.grid(alpha=.2,which='both');ax.legend()
    write=data['write_cost']['summary'];median=[write[a]['p50_ns']/1000 for a in colors];upper=[write[a]['p95_ns']/1000-write[a]['p50_ns']/1000 for a in colors]
    axes[2].bar(list(colors),median,color=list(colors.values()),yerr=[[0,0],upper],capsize=5)
    axes[2].set(ylabel='Microseconds per synthetic SQL row append',title='Write cost · p50 bars / p95 whiskers');axes[2].grid(axis='y',alpha=.2)
    fig.suptitle('Room activity index: faster history reads, added writes/storage\n31 randomized read pairs/scale ·21 write pairs ×500 row transactions',fontsize=13)
    text=StringIO();fig.savefig(text,format='svg',metadata={'Date':None},bbox_inches='tight',pad_inches=.15);fig.savefig(HERE/'activity-index.png',dpi=160,bbox_inches='tight',pad_inches=.15);plt.close(fig)
    svg=text.getvalue();svg=svg[svg.index('<svg'):].replace('<svg ','<svg role="img" aria-label="Activity read latency and logical visits fall with the index; SQL append cost rises" ',1)
    rows=[]
    for r in experiments:
        s=r['summary'];cells=[f"{r['fixture']['rows_per_room']:,}"]
        for arm in colors:cells.extend(f"{s[arm][k+'_ns']/1e6:.3f}"for k in ['p50','p95','p99'])
        cells.extend(['–'.join(f'{v/1e6:.3f}'for v in s['paired_median_saved_95_ci_ns']),f"{s['two_sided_sign_test_p']:.3g}",f"{r['counters']['before']['visits']:,}→{r['counters']['after']['visits']:,}",f"{r['index_extra_bytes']:,}",f"{r['schema_replay_seconds_python_sqlite']*1000:.1f}"]);rows.append(cells)
    page='''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Arbos room activity index</title><style>body{font:16px/1.5 system-ui,sans-serif;color:#24332f;max-width:1320px;margin:2rem auto;padding:0 1rem}svg{width:100%;height:auto}.table{overflow:auto}table{border-collapse:collapse;font-size:13px;width:100%;font-variant-numeric:tabular-nums}th,td{padding:.6rem;border-bottom:1px solid #ccd6d0;text-align:right}th:first-child,td:first-child{text-align:left}.note{background:#eef5f1;padding:1rem;border-left:4px solid #197f77}</style><h1>Last activity without scanning retained history</h1><p class="note">The actual room-activity query still returns head and the maximum timestamp. Adding rows(room,created_at) lets SQLite find the maximum in its index. At100,000 retained rows per room, SQL p50 falls5.503→0.041ms and logical visits100,001→2. This is a query/index measurement, not a screen or send-latency result.</p>'''+svg
    page+=table(['Rows/room','Before p50ms','Before p95ms','Before p99ms','After p50ms','After p95ms','After p99ms','Saved median95%CI ms','Sign-test p','Logical visits','Extra index bytes','Schema replay ms'],rows)
    cost=-write['median_saved_ns']/1000;ci=[-v/1000 for v in reversed(write['paired_median_saved_95_ci_ns'])]
    page+=f'''<h2>Explicit costs</h2><p>The generated million-row database grows by27,045,888 bytes (about25.8MiB); one-time schema replay/index creation takes0.280s using Python SQLite. The separate root-lock SQLite3.46 write sample increases SQL-append p50 from{write['before']['p50_ns']/1000:.3f} to{write['after']['p50_ns']/1000:.3f}µs. The paired median added cost is{cost:.3f}µs (95% interval{ci[0]:.3f}–{ci[1]:.3f}µs), with exact sign-test p={write['two_sided_sign_test_p']:.3g}. This deliberately records a write regression. The transaction loop includes Python dispatch and SQL preparation; it approximates one-row append work and does not measure real Db.append, valid Loro decoding or end-to-end sending.</p>'''
    page+='''<p>The index is chosen to avoid recurring history scans on home/group reads. Read/write frequency and total history size in production are unmeasured; review storage, startup migration and write budget before applying the additive schema change. No shared database was opened or migrated.</p><p>Four generated local datasets contain10 rooms with100/1,000/10,000/100,000 retained65-byte opaque rows each. SQL and full head/default/timestamp semantics are unchanged. Six suite checks preserve missing/empty defaults, out-of-order maxima, negative timestamps, row bytes and idempotent schema replay. A separate actual native regression failed its VM-step budget before the index, then passed with the other nine roster/profile checks. No test was removed.</p><p>31 randomized read pairs per scale,3 warmups;21 randomized write pairs of500 transactions per arm. Intervals use2,000 seeded paired-median bootstrap resamples; exact two-sided sign tests exclude ties. All four read-saving intervals are positive (Bonferroni-adjusted p≤3.73e-9 across four read comparisons). Root-lock SQLite3.46 uses scanstatus; logical visits are not physical reads. Write pragmas match actual Db.open: WAL, NORMAL, foreign_keys and secure_delete. Warm single-reader/writer operation, no CPU affinity, concurrency or production-performance claim. Full samples, query plans, actual SQL, source hashes and write costs are in <a href="ACTIVITY_RESULTS.json">ACTIVITY_RESULTS.json</a>.</p></html>'''
    (HERE/'activity-index.html').write_text('\n'.join(line.rstrip()for line in page.splitlines())+'\n')

if __name__=='__main__':render()
