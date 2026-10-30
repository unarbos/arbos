#!/usr/bin/env python3
"""Keep-or-drop verdict for one A/B run, and the ledger row (docs/grok-convergence-loop.md §1 stage 5).

  decide.py --ab RUN-ID --target TRAIT --hypothesis "text (lever)" --change "what · kill switch" [--write-ledger] [--version V]

Rule: keep when the target trait's share rises by more than its noise band, no other trait's share
falls by more than its band, and no judge mean falls by more than max(0.2, judge band). Cost per
turn for both arms is printed next to the verdict; the ledger carries it either way.
"""

import argparse
import json
import os
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "long-tasks"))
from lib import convergence as cv  # noqa: E402

NOISE = HERE / "noise.json"
STORE = Path(os.environ.get("LT_STORE", "/cursor/stores/bc-055bfdfb-336d-43af-bd4b-a794e153944c"))
LEDGER = STORE / "docs" / "grok-convergence-ledger.md"


def verdict(board, target, noise):
    bands = noise["bands"]
    a, b = board["arms"]["A"], board["arms"]["B"]
    ta, tb = a["convergence"]["traits"], b["convergence"]["traits"]
    deltas = {}
    for t in cv.TRAITS:
        if ta.get(t) and tb.get(t) and ta[t]["arbos_share"] is not None and tb[t]["arbos_share"] is not None:
            deltas[t] = round(tb[t]["arbos_share"] - ta[t]["arbos_share"], 3)
    problems, notes = [], []
    tgt = deltas.get(target)
    band_t = bands.get(target, 0.1)
    if tgt is None:
        problems.append(f"target {target} not scored on both arms")
    elif tgt <= band_t:
        problems.append(f"target {target} moved {tgt:+.3f}, not above its band {band_t}")
    else:
        notes.append(f"target {target} {tgt:+.3f} > band {band_t}")
    for t, d in deltas.items():
        if t != target and d < -bands.get(t, 0.1):
            problems.append(f"{t} fell {d:+.3f} beyond its band {bands.get(t, 0.1)}")
    ja, jb = (a.get("parity") or {}).get("judge_mean") or {}, (b.get("parity") or {}).get("judge_mean") or {}
    jband = max(0.2, bands.get("judge", 0.2))
    for t in ja:
        if t in jb and jb[t] < ja[t] - jband:
            problems.append(f"judge {t} fell {jb[t] - ja[t]:+.2f} beyond {jband}")
    cost = f"${a['cost_per_turn_usd']:.4f} → ${b['cost_per_turn_usd']:.4f} per turn (×{(b['cost_per_turn_usd'] / a['cost_per_turn_usd']) if a['cost_per_turn_usd'] else 0:.1f})"
    worst = min(((t, d) for t, d in deltas.items() if t != target), key=lambda x: x[1], default=None)
    return {"keep": not problems, "problems": problems, "notes": notes, "deltas": deltas, "target_delta": tgt, "worst_other": worst, "cost": cost,
            "judge": {t: (ja.get(t), jb.get(t)) for t in sorted(set(ja) | set(jb))}}


def ledger_row(board, args, v):
    m = board["meta"]
    a, b = board["arms"]["A"], board["arms"]["B"]
    ta, tb = a["convergence"]["traits"].get(args.target, {}), b["convergence"]["traits"].get(args.target, {})
    tgt = f"{args.target} {ta.get('arbos', {}).get('pass')}/{ta.get('arbos', {}).get('total')} → {tb.get('arbos', {}).get('pass')}/{tb.get('arbos', {}).get('total')}"
    worst = f"worst other {v['worst_other'][0]} {100 * v['worst_other'][1]:+.0f} pp" if v["worst_other"] else "no other trait scored"
    ja = ", ".join(f"{t} {x[0]}→{x[1]}" for t, x in v["judge"].items() if x[0] is not None and x[1] is not None) or "—"
    gap = a["convergence"]["traits"].get(args.target, {}).get("gap_pp")
    return (f"| {m['cycle']} | {m['started_at'][:16].replace('T', ' ')} | {args.target} ({gap} pp) | {args.hypothesis} | {args.change} | "
            f"{tgt}; {worst}; judge {ja}; {v['cost']}; run `{m['run_id']}` | {'**kept**' if v['keep'] else 'dropped — ' + '; '.join(v['problems'])[:200]} | {args.version or '—'} |")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ab", required=True)
    ap.add_argument("--target", required=True)
    ap.add_argument("--hypothesis", required=True)
    ap.add_argument("--change", required=True)
    ap.add_argument("--version", default="")
    ap.add_argument("--write-ledger", action="store_true")
    args = ap.parse_args()
    board = json.loads((HERE / "scoreboard" / f"{args.ab}.json").read_text())
    noise = json.loads(NOISE.read_text()) if NOISE.exists() else {"bands": {}}
    v = verdict(board, args.target, noise)
    print(json.dumps(v, indent=1))
    row = ledger_row(board, args, v)
    print(row)
    if args.write_ledger and LEDGER.exists():
        text = LEDGER.read_text()
        lines = text.rstrip("\n").split("\n")
        # append under the table: the last table row is the last line starting with '|' before the noise section
        idx = max(i for i, l in enumerate(lines) if l.startswith("|"))
        lines.insert(idx + 1, row)
        LEDGER.write_text("\n".join(lines) + "\n")
        print(f"ledger row added: {LEDGER}")
    print("VERDICT:", "KEEP" if v["keep"] else "DROP", "|", v["cost"], "|", time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()))


if __name__ == "__main__":
    main()
