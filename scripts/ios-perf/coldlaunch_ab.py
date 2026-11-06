#!/usr/bin/env python3
# Cold-launch A/B from `perf-rig.sh` coldlaunch: coldlaunch_ab.py <cl-launch.app.log> <cl-pids.txt>
# Per launch: process start → AppModel init (`app-init`), → root view on screen (frame probe
# start, `probe on t=`), → populated chat list (`first-content`), and the late frames after the
# root appears, split at +400 ms (the warm-up sleeps 500 ms before it starts).
import re
import statistics
import sys
from datetime import datetime

log, pids = sys.argv[1], sys.argv[2]
arm_of = {}
for line in open(pids):
    p = line.split()
    if len(p) == 3:
        arm_of[p[2]] = (p[0], int(p[1]))
runs = {}
for line in open(log, errors="replace"):
    m = re.match(r"(\S+ \S+) \S+ \S+\[(\d+):[0-9a-f]+\] \[com\.unarbos\.arbos\.ios:(launch|perf-frames)\] (.*)", line)
    if not m or m.group(2) not in arm_of:
        continue
    ts = datetime.strptime(m.group(1), "%Y-%m-%d %H:%M:%S.%f").timestamp()
    r = runs.setdefault(m.group(2), {"late": []})
    msg = m.group(4)
    mm = re.match(r"(app-init|first-content) (\d+) ms", msg)
    if mm:
        r[mm.group(1)] = int(mm.group(2))
    mm = re.match(r"probe on .* t=(\d+)ms", msg)
    if mm:
        r["root"], r["root_ts"] = int(mm.group(1)), ts
    mm = re.match(r"late ms=([\d.]+) missed=(\d+)", msg)
    if mm and "root_ts" in r:
        r["late"].append((1000 * (ts - r["root_ts"]), float(mm.group(1)), int(mm.group(2))))


def med(xs):
    return statistics.median(xs) if xs else None


for arm in ("on", "off"):
    rows = sorted(((arm_of[p][1], p, r) for p, r in runs.items() if arm_of[p][0] == arm))
    print(f"== warm-up {arm}")
    for i, p, r in rows:
        early = [x for x in r["late"] if x[0] < 400]
        later = [x for x in r["late"] if 400 <= x[0] < 6000]
        print(f"  #{i} pid={p} app-init={r.get('app-init')} root={r.get('root')} first-content={r.get('first-content')} "
              f"late<+400ms={[round(x[1]) for x in early]} late+400ms..6s={[(round(x[0]), round(x[1])) for x in later]}")
    for key in ("app-init", "root", "first-content"):
        v = [r[key] for _, _, r in rows if key in r]
        if v:
            print(f"  {key}: n={len(v)} median={med(v)} min={min(v)} max={max(v)}")
    lt = [len([x for x in r["late"] if 400 <= x[0] < 6000]) for _, _, r in rows]
    dr = [sum(x[2] for x in r["late"] if 400 <= x[0] < 6000) for _, _, r in rows]
    print(f"  late frames +400ms..6s per launch: {lt} (dropped frames {dr})")
