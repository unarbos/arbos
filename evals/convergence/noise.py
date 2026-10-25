#!/usr/bin/env python3
"""Noise bands per trait (docs/grok-convergence-loop.md §4).

  noise.py [--runs id1,id2,…] [--ab RUN-ID] [--write]

Sources, in order of preference:
  --ab RUN-ID   an A/A run from ab.py: band = |share_A − share_B|
  --runs …      repeat runs of the long-task parity subset on near-identical images (the four
                existing parity scorecards are such a series: 0.1.118 / 0.1.121 / 0.1.125 / 0.1.131
                differ by one change each) — band = standard deviation of the pass share per trait
Both get the floor 1 / n_t (one turn's worth), so a trait scored on one turn can never be "won" by
one lucky sample. --write stores evals/convergence/noise.json; decide.py reads it.
"""

import argparse
import json
import statistics
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
LT_SB = HERE.parent / "long-tasks" / "scoreboard"
CV_SB = HERE / "scoreboard"
OUT = HERE / "noise.json"
sys.path.insert(0, str(HERE.parent / "long-tasks"))
from lib import convergence as cv  # noqa: E402

DEFAULT_RUNS = ["20261024T2330-parity-before", "20261025T0210-parity-after-step1", "20261025T0420-parity-after-steps123", "20261025T1320-parity-after-1710"]


def shares_from_board(path):
    b = json.loads(path.read_text())
    blk = cv.summarize(b["results"])
    return {t: v for t, v in (blk or {}).get("traits", {}).items()}, (blk or {}).get("judge_mean") or {}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", default=",".join(DEFAULT_RUNS))
    ap.add_argument("--ab", default="")
    ap.add_argument("--write", action="store_true")
    args = ap.parse_args()
    bands, detail = {}, {}
    if args.ab:
        b = json.loads((CV_SB / f"{args.ab}.json").read_text())
        ta, tb = b["arms"]["A"]["convergence"]["traits"], b["arms"]["B"]["convergence"]["traits"]
        for t in cv.TRAITS:
            a, c = ta.get(t), tb.get(t)
            if a and c and a["arbos_share"] is not None and c["arbos_share"] is not None:
                n = max(1, a["arbos"]["total"])
                bands[t] = round(max(abs(a["arbos_share"] - c["arbos_share"]), 1.0 / n), 3)
                detail[t] = {"source": f"A/A {args.ab}", "n": n, "A": a["arbos_share"], "B": c["arbos_share"]}
        src = {"kind": "A/A", "run": args.ab}
    else:
        runs = [r for r in args.runs.split(",") if r]
        series = {}
        judges = {}
        for r in runs:
            p = LT_SB / f"{r}.json"
            if not p.exists():
                print(f"skip {r}: no scoreboard", file=sys.stderr)
                continue
            tr, jm = shares_from_board(p)
            for t, v in tr.items():
                if v["arbos_share"] is not None:
                    series.setdefault(t, []).append((v["arbos_share"], v["arbos"]["total"]))
            for t, v in jm.items():
                judges.setdefault(t, []).append(v)
        for t, xs in series.items():
            shares = [s for s, _ in xs]
            n = max(1, round(sum(n for _, n in xs) / len(xs)))
            sd = statistics.pstdev(shares) if len(shares) > 1 else 0.0
            bands[t] = round(max(sd, 1.0 / n), 3)
            detail[t] = {"source": "repeat runs", "runs": len(shares), "n_per_run": n, "shares": [round(s, 3) for s in shares], "sd": round(sd, 3)}
        bands["judge"] = round(max([statistics.pstdev(v) for v in judges.values() if len(v) > 1] or [0.2]), 2)
        detail["judge"] = {k: [round(x, 2) for x in v] for k, v in judges.items()}
        src = {"kind": "repeat runs", "runs": runs}
    out = {"bands": bands, "detail": detail, "source": src, "built_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
           "rule": "keep a change only if the target trait's share rises by more than its band and no other trait's share falls by more than its band; judge means may not fall by more than max(0.2, judge band)"}
    print(json.dumps(out, indent=1))
    if args.write:
        OUT.write_text(json.dumps(out, indent=1) + "\n")
        print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
