#!/usr/bin/env python3
"""A/B table of the work-kind split from openprof_analyze.py reports, median per arm.
Usage: openprof_ab.py <group> <tag>.report.txt...   (the arm is `-on-` / `-off-` in the file name)."""
import re, sys, statistics, collections
grp_want=sys.argv[1]; files=sys.argv[2:]
d=collections.defaultdict(lambda: {'on':{}, 'off':{}})
arms={'on':[], 'off':[]}
for f in files:
    arm='on' if '-on-' in f else 'off'; arms[arm].append(f)
    grp=None;inwk=False
    for l in open(f):
        m=re.match(r"=== \S+ / (\S+):",l)
        if m: grp=m.group(1);inwk=False;continue
        if grp!=grp_want: continue
        m=re.search(r"main-thread CPU \(sampled\) per open: ([\d.]+)",l)
        if m: d['== main-thread CPU total'][arm][f]=float(m.group(1))
        if '-- work kind' in l: inwk=True;continue
        if l.strip().startswith('--'): inwk=False
        if inwk:
            m=re.match(r"\s*([\d.]+) ms\s+[\d.]+%\s+(.*)",l)
            if m: d[m.group(2).strip()][arm][f]=float(m.group(1))
rows=[]
for k,v in d.items():
    on=statistics.median([v['on'].get(f,0.0) for f in arms['on']]); off=statistics.median([v['off'].get(f,0.0) for f in arms['off']])
    rows.append((off,k,on))
rows.sort(reverse=True)
print(f"{'off':>7} {'on':>7} {'diff':>7}  kind   ({grp_want}; medians, off n={len(arms['off'])}, on n={len(arms['on'])})")
for off,k,on in rows: print(f"{off:7.1f} {on:7.1f} {on-off:+7.1f}  {k}")
