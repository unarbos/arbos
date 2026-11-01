#!/usr/bin/env python3
"""Main-thread CPU in a fixed window around each push, anchored on UIKit's `NavigationTransition`
signpost instead of the app's `open-chat`. The app's signposts can be missing for the start of a
trace when the Mac is loaded; UIKit's and the CPU samples are not. Pushes are every other
transition (push, pop, push, ...).
Usage: openprof_push_window.py PRE_MS POST_MS <tag>... (reads <tag>.tp.xml and <tag>.sp.xml)"""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import openprof_analyze as oa


def push_spans(sp, pre, post):
    pushes = [a for a, _, _ in oa.intervals_of(sp)["NavigationTransition"]][0::2]
    return [(p - pre * 1_000_000, p + post * 1_000_000, "") for p in pushes]


def main():
    pre, post = int(sys.argv[1]), int(sys.argv[2])
    for tag in sys.argv[3:]:
        ivs = oa.intervals_of(tag + ".sp.xml")
        spans = push_spans(tag + ".sp.xml", pre, post)
        opens = [a for a, _, _ in ivs.get("open-chat", [])]
        pushes = [a + pre * 1_000_000 for a, _, _ in spans]
        lead = [round((min(pushes, key=lambda p: abs(p - o)) - o) / 1e6) for o in opens]
        cpu = [0.0] * len(spans)
        for k, _, w, _ in oa.samples_of(tag + ".tp.xml", spans):
            cpu[k] += w
        print(f"{tag:9s} pushes={len(spans)} open-chat={len(opens)} open->push ms={lead}"
              f"  CPU[-{pre},+{post}] per push: {[round(c) for c in cpu]}")


if __name__ == "__main__":
    main()
