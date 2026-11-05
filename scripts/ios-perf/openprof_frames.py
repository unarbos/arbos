#!/usr/bin/env python3
"""Where a frame's cost comes from, for main-thread samples inside chosen opens.
Frames are leaf first; "outermost match" is the match nearest the root.
  callers: the DEPTH frames above the outermost match, plus the deepest Arbos frame.
  below:   the three frames just below the outermost match (what it spends time in).
  stacks:  the TOP most common full stacks above the outermost match, DEPTH frames each.
OPENS uses openprof_analyze.py's group syntax (for example `0` or `0,2-4`). SYMS is a
sim_symbol_addrs.py symbol file, or `-` when the trace is already symbolicated.
Usage: openprof_frames.py callers|below|stacks <tag> SYMS OPENS REGEX [DEPTH] [TOP]
(reads <tag>.tp.xml and <tag>.sp.xml)"""
import collections
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import openprof_analyze as oa


def main():
    mode, tag, syms, opens, target = sys.argv[1:6]
    depth = int(sys.argv[6]) if len(sys.argv) > 6 else (4 if mode == "callers" else 30)
    top = int(sys.argv[7]) if len(sys.argv) > 7 else 3
    if syms != "-":
        oa.load_syms(syms)
    spans = oa.intervals_of(tag + ".sp.xml").get("open-chat", [])
    sel = set(oa.parse_groups("g=" + opens, len(spans))[0][1])
    rx = re.compile(target)
    by_stack, by_app, total = collections.Counter(), collections.Counter(), 0.0
    for k, _, w, fr in oa.samples_of(tag + ".tp.xml", spans):
        if k not in sel:
            continue
        idx = [i for i, (nm, _) in enumerate(fr) if rx.search(nm)]
        if not idx:
            continue
        i = max(idx)
        total += w
        if mode == "callers":
            by_stack[" < ".join(oa.short(nm, 70) for nm, _ in fr[i + 1:i + 1 + depth])] += w
            by_app[oa.short(next((nm for nm, b in fr if b in oa.APP_BINARIES), "(none)"), 90)] += w
        elif mode == "below":
            by_stack[" < ".join(oa.short(nm, 70) for nm, _ in reversed(fr[max(0, i - 3):i]))] += w
        elif mode == "stacks":
            by_stack[tuple(oa.short(nm, 110) for nm, _ in fr[i:i + depth])] += w
        else:
            sys.exit(f"unknown mode {mode}")
    print("total", total)
    if mode == "stacks":
        for st, w in by_stack.most_common(top):
            print(f"==== {w:.1f} ms")
            print("\n".join("   " + s for s in st))
        return
    for c, v in by_stack.most_common(12):
        print(f"{v:6.1f}  {c}")
    if mode == "callers":
        print("-- deepest Arbos frame")
        for c, v in by_app.most_common(15):
            print(f"{v:6.1f}  {c}")


if __name__ == "__main__":
    main()
