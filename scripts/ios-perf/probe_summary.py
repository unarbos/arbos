#!/usr/bin/env python3
# Summarise a `perf-rig.sh` proberun: probe_summary.py <tag.probe.log> <tag.files.txt>
import re
import sys

log, files = sys.argv[1], sys.argv[2]
phases, counts, own, summary, launch = {}, [], [], None, {}
for line in open(log, errors="replace"):
    if "perfprobe:probe" in line:
        m = re.search(r"probe phase=(\d) tid=(\d+) main=(\d) wall=([\d.]+)", line)
        if m:
            phases[int(m.group(1))] = (float(m.group(4)), m.group(3) == "1")
        if "probe count when=phase4+3s" in line or ("probe count when=t+14s" in line and 4 not in phases):
            counts.append(line.split("probe count ", 1)[1].strip())
        if "probe OWN hit" in line:
            own.append(line.split("probe OWN hit ", 1)[1].strip())
        if "probe summary" in line:
            summary = line.split("probe summary ", 1)[1].strip()
        if "probe loaded" in line:
            print("loaded:", line.split("probe loaded ", 1)[1].strip())
    m = re.search(r"\[com\.unarbos\.arbos\.ios:launch\] (app-init|first-content) (\d+) ms", line)
    if m:
        launch[m.group(1)] = int(m.group(2))
print("launch:", launch)
if phases:
    w1, w4 = phases[1][0], phases[4][0]
    print(f"window: phase1→2 {1000 * (phases[2][0] - w1):.2f} ms (main={phases[1][1]}), "
          f"phase3→4 {1000 * (w4 - phases[3][0]):.1f} ms (main={phases[3][1]}), total {1000 * (w4 - w1):.1f} ms")
    touched = []
    for line in open(files, errors="replace"):
        p = line.split(" ", 2)
        if len(p) == 3 and w1 - 0.05 <= float(p[0]) <= w4 + 0.05:
            touched.append(line.strip())
    print(f"files modified inside the window (±50 ms): {len(touched)}")
    for t in touched:
        print("  ", t)
else:
    print("no warm-up window (off arm)")
print("summary:", summary)
print("OWN hits:", len(own))
for o in own:
    print("  ", o)
for c in counts:
    print("  ", c)
