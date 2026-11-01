#!/usr/bin/env python3
"""Rebuild opens whose `open-chat` signposts the trace lost (the os_signpost track can start late
when the Mac is loaded) from the in-app driver's fixed cycle, then break down fixed windows.
The driver (`ARBOS_PERF_OPENS`) opens on a steady period, so open k sits at
first_recorded - (LOST - k) * median_period.
Usage: openprof_synth_opens.py tp.xml sp.xml syms.tsv LABEL LOST WINDOW_MS GROUPS
  GROUPS as in openprof_analyze.py, over open indices 0.. (the rebuilt ones first)."""
import os, sys, statistics, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import openprof_analyze as oa


def main():
    tp, sp, syms, label, lost, win, spec = sys.argv[1:8]
    lost, win = int(lost), int(win)
    oa.load_syms(syms)
    rec = oa.intervals_of(sp)["open-chat"]
    starts = [a for a, _, _ in rec]
    gaps = [(b - a) / 1e9 for a, b in zip(starts, starts[1:])]
    period = statistics.median(gaps)
    print(f"recorded opens {len(rec)}; period s median {period:.4f} min {min(gaps):.4f} max {max(gaps):.4f}; rebuilt {lost}")
    syn = [(int(starts[0] - k * period * 1e9), 0, "rebuilt") for k in range(lost, 0, -1)]
    spans = [(a, a + win * 1_000_000, m) for a, _, m in syn + rec]
    samples = oa.samples_of(tp, spans)
    print("   main-thread CPU per 100 ms bin from each rebuilt start (checks the alignment):")
    for i in range(lost + 1):
        bins = collections.Counter()
        for k, dt, w, fr in samples:
            if k == i:
                bins[int(dt / 1e8)] += w
        print(f"   open {i:2d}: " + " ".join(f"{bins[b]:3.0f}" for b in range(win // 100)))
    stats = oa.report(label, spans, samples, oa.parse_groups(spec, len(spans)))
    oa.first_minus_repeat(label, stats)


if __name__ == "__main__":
    main()
