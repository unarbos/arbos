#!/usr/bin/env python3
"""Who asks for a costly frame: for samples whose stack matches TARGET inside the chosen opens,
charge them to the nearest caller outside the runtime (the frame just above the outermost
TARGET match that is not in libswiftCore / libobjc / dyld / libsystem_*), and to the deepest
Arbos frame. Usage: openprof_callers.py tp.xml sp.xml syms.tsv OPENS TARGET_REGEX"""
import os, sys, re, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import openprof_analyze as oa

RUNTIME = re.compile(r"^(libswiftCore|libobjc|dyld|libsystem_|libdyld|libc\+\+)")


def main():
    tp, sp, syms, opens, target = sys.argv[1:6]
    oa.load_syms(syms)
    spans = oa.intervals_of(sp).get("open-chat", [])
    groups = oa.parse_groups("g=" + opens, len(spans))
    sel = set(groups[0][1])
    n = len(sel)
    rx = re.compile(target)
    samples = [s for s in oa.samples_of(tp, spans) if s[0] in sel]
    total = sum(s[2] for s in samples)
    callers, app, both = collections.Counter(), collections.Counter(), collections.Counter()
    hit = 0.0
    for k, dt, w, fr in samples:
        idx = [i for i, (nm, _) in enumerate(fr) if rx.search(nm)]
        if not idx:
            continue
        hit += w
        caller = "?"
        for nm, b in fr[max(idx) + 1:]:
            if not RUNTIME.match(b) and not rx.search(nm):
                caller = f"{nm}  [{b}]"
                break
        callers[caller] += w
        a = oa.deepest_app(fr) or "(no Arbos frame)"
        app[a] += w
        both[(caller, a)] += w
    print(f"opens {sorted(sel)[:5]}… n={n}; target /{target}/: {hit / n:.1f} ms per open of {total / n:.1f} ms main-thread CPU")
    print("-- nearest non-runtime caller")
    for k, v in callers.most_common(25):
        print(f"  {v / n:7.2f} ms {100 * v / max(hit, 1e-9):5.1f}%  {oa.short(k, 170)}")
    print("-- deepest Arbos frame")
    for k, v in app.most_common(15):
        print(f"  {v / n:7.2f} ms {100 * v / max(hit, 1e-9):5.1f}%  {oa.short(k, 170)}")


if __name__ == "__main__":
    main()
