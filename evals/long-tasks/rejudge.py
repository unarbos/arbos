#!/usr/bin/env python3
"""Re-run the LLM rubric for turns of a scoreboard whose rubric was skipped (judge error), from the
stored reply and verify output; recompute pass/fail and the aggregates.

  rejudge.py --run <run-id> [--tasks a,b]
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from lib import grade as gr  # noqa: E402
from run import load_tasks, summarize  # noqa: E402

SB = Path(__file__).resolve().parent / "scoreboard"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True)
    ap.add_argument("--tasks", default="", help="also re-judge these tasks' scored rubric turns (comma list); the first verdict is kept as `first_rubric`")
    args = ap.parse_args()
    board = json.loads((SB / f"{args.run}.json").read_text())
    tasks = {t["id"]: t for t in load_tasks()}
    again = set(filter(None, args.tasks.split(",")))
    judged = 0
    for r in board["results"]:
        task = tasks.get(r["task"])
        if not task:
            continue
        for t in r.get("turns") or []:
            spec = task["turns"][t["turn"] - 1] if t["turn"] - 1 < len(task["turns"]) else {}
            rub = spec.get("rubric")
            scored = t.get("rubric") and t["rubric"].get("score") is not None
            if not rub or (scored and r["task"] not in again):
                continue
            if scored:
                t["first_rubric"] = t["rubric"]
            produced = t.get("produced") or "\n".join(l[5:] for l in (t.get("verify_out") or "").splitlines() if l.startswith("SHOW "))
            res = gr.judge(t["prompt"], rub["criteria"], t.get("reply") or "", produced, t.get("verify_out") or "")
            t["rubric"] = res
            judged += 1
            t["reasons"] = [x for x in t.get("reasons") or [] if not x.startswith("rubric ")]
            if res.get("score") is not None and res["score"] < rub.get("min", 3):
                t["reasons"].append(f"rubric {res['score']}/5: {res['reason']}")
            t["ok"] = not t["reasons"]
        vw = task.get("verify_wait") or {}
        bg = r.get("background")
        if vw.get("rubric") and bg and (not bg.get("rubric") or bg["rubric"].get("score") is None):
            text = bg.get("all_text") or "\n".join(t.get("reply") or "" for t in r.get("turns") or [])
            res = gr.judge(task["turns"][0]["prompt"], vw["rubric"]["criteria"], text, bg.get("verify_out") or "", bg.get("verify_out") or "")
            bg["rubric"] = res
            judged += 1
        turn_reasons = [f"t{t['turn']}: {x}" for t in r.get("turns") or [] for x in t.get("reasons") or []]
        other = [x for x in r.get("reasons") or [] if not x.startswith("t") or ":" not in x[:4]]
        r["reasons"] = turn_reasons + [x for x in other if not x.startswith("rubric ")]
        if bg and bg.get("rubric") and bg["rubric"].get("score") is not None and bg["rubric"]["score"] < vw["rubric"].get("min", 3):
            r["reasons"].append(f"rubric {bg['rubric']['score']}/5: {bg['rubric']['reason']}")
        r["success"] = not r["reasons"] and not r.get("skipped")
        scores = [t["rubric"]["score"] for t in r.get("turns") or [] if t.get("rubric") and t["rubric"].get("score") is not None]
        r["rubric_min"] = min(scores) if scores else None
        r["judge_usd"] = round(sum(((t.get("rubric") or {}).get("usd") or 0) for t in r.get("turns") or []) + (((bg or {}).get("rubric") or {}).get("usd") or 0), 5)
    meta = board["meta"]
    meta["judge_cost_usd"] = round(sum(r.get("judge_usd") or 0 for r in board["results"]), 4)
    new = summarize(meta, board["results"])
    (SB / f"{args.run}.json").write_text(json.dumps(new, indent=1))
    print(f"re-judged {judged} turns; pass {new['summary']['passed']}/{new['summary']['run']}; judge ${meta['judge_cost_usd']}")


if __name__ == "__main__":
    main()
