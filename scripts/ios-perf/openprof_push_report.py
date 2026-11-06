#!/usr/bin/env python3
"""openprof_analyze.py's full report over push-anchored windows (see openprof_push_window.py), so
openprof_ab.py can compare the work-kind split when `open-chat` signposts are missing.
Usage: openprof_push_report.py <tag> PRE_MS POST_MS GROUPS > <tag>.push.report.txt
  GROUPS as in openprof_analyze.py, over push indices (e.g. dm-first=0;group-first=1;dm-repeat=2-4:2;group-repeat=3-5:2)"""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import openprof_analyze as oa
from openprof_push_window import push_spans


def main():
    tag, pre, post, spec = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
    spans = push_spans(tag + ".sp.xml", pre, post)
    samples = oa.samples_of(tag + ".tp.xml", spans)
    oa.report(tag, spans, samples, oa.parse_groups(spec, len(spans)))


if __name__ == "__main__":
    main()
