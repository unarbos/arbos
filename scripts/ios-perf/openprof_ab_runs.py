#!/usr/bin/env python3
"""Per-run values for a warm-up on/off A/B, from openprof_analyze.py reports.
Lists every run's value (not only the median) for each open group, so one noisy run is visible.
A file whose name contains `-on-` is the warm-up arm; any other file is the off arm. A work kind
missing from a group counts as 0.
Usage: openprof_ab_runs.py wu-on-2.report.txt wu-off-1.report.txt ..."""
import collections
import re
import statistics
import sys

KEYS = [("interval", "interval"), ("cpu", "main CPU"),
        ("Swift runtime type lookup", "type lookup"),
        ("DynamicPropertyCache.fields", "DPC.fields"),
        ("AttributeScope.scopeDescription", "scopeDescription"),
        ("Metal device first use", "Metal first use")]
GROUPS = ["dm-first", "group-first", "dm-repeat", "group-repeat"]


def main():
    data = collections.defaultdict(lambda: collections.defaultdict(list))
    for f in sys.argv[1:]:
        arm = "on" if "-on-" in f else "off"
        grp, in_kinds, seen = None, False, set()

        def close():
            for k, _ in KEYS:
                if k not in seen:
                    data[(grp, k)][arm].append(0.0)

        for line in open(f):
            m = re.match(r"=== \S+ / (\S+):", line)
            if m:
                if grp:
                    close()
                grp, seen, in_kinds = m.group(1), set(), False
                continue
            if grp is None:
                continue
            m = re.search(r"interval ms: median ([\d.]+)", line)
            if m:
                data[(grp, "interval")][arm].append(float(m.group(1)))
                seen.add("interval")
            m = re.search(r"main-thread CPU \(sampled\) per open: ([\d.]+)", line)
            if m:
                data[(grp, "cpu")][arm].append(float(m.group(1)))
                seen.add("cpu")
            if "-- work kind" in line:
                in_kinds = True
                continue
            if line.strip().startswith("--"):
                in_kinds = False
            if in_kinds:
                for k, _ in KEYS[2:]:
                    if k in line and k not in seen:
                        data[(grp, k)][arm].append(float(line.split()[0]))
                        seen.add(k)
        if grp:
            close()
    for grp in GROUPS:
        print(f"## {grp}")
        for k, label in KEYS:
            d = data[(grp, k)]
            med = {a: statistics.median(d[a]) if d[a] else "-" for a in ("on", "off")}
            print(f"  {label:18s} on {d['on']} med {med['on']} | off {d['off']} med {med['off']}")


if __name__ == "__main__":
    main()
