#!/usr/bin/env python3
"""When the first-open warm-up ran: the count of CPU samples with a FirstOpenWarmup frame and the
times of the first and last one. Shows whether the warm-up finished before the first push.
Usage: openprof_warmup_span.py <tag>... (reads <tag>.tp.xml)"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import openprof_analyze as oa


def main():
    for tag in sys.argv[1:]:
        ts = []
        for row, res in oa.load(tag + ".tp.xml"):
            bt = row.get("backtrace")
            bt = bt if bt is not None else row.get("tagged-backtrace")
            if bt is None:
                continue
            if any("FirstOpenWarmup" in n or "warmFirstOpen" in n for n, _ in oa.frames(bt, res)):
                ts.append(int(row["sample-time"].text))
        span = "first %.2f s last %.2f s" % (min(ts) / 1e9, max(ts) / 1e9) if ts else "none"
        print(tag, len(ts), span)


if __name__ == "__main__":
    main()
