#!/usr/bin/env python3
"""Signpost intervals in a trace: name, count, and the first ten start times in seconds.
Use it to see whether the app's `open-chat` signposts survived before trusting an analysis that
anchors on them (when they did not, use openprof_push_window.py).
Usage: openprof_signposts.py <tag>.sp.xml ..."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import openprof_analyze as oa


def main():
    for f in sys.argv[1:]:
        print(f)
        for name, v in sorted(oa.intervals_of(f).items()):
            print("   %-28s %3d  starts s: %s" % (name, len(v), [round(b / 1e9, 1) for b, _, _ in v][:10]))


if __name__ == "__main__":
    main()
