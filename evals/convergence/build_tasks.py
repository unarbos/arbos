#!/usr/bin/env python3
"""Turn the clean corpus into replay tasks for the long-task harness (parity-style scripted chats).

  build_tasks.py [--force]

For every conversation in corpus/index.json that is not already covered by an existing task
(`task` set — the three seed baselines are pj1–pj3), write
  evals/long-tasks/tasks/convergence/cv-<id>.json   the scripted conversation with per-turn shapes
  evals/long-tasks/baselines/grok/cv-<id>.md        Grok's bubbles, the judge's reference
The shapes come from Grok's own turn (cadence ± 1, table/bullets/links as Grok used them, offer,
ack-first and status line when Grok delegated); the task carries `convergence.grok` so the
scoreboard can compare side by side. Prompts that need a file the user had, or that would send a
real message outside, are written with `replayable: false` and skipped by the runner.
"""

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import traits  # noqa: E402

HERE = Path(__file__).resolve().parent
LT = HERE.parent / "long-tasks"
TASKS = LT / "tasks" / "convergence"
BASELINES = LT / "baselines" / "grok"
INDEX = HERE / "corpus" / "index.json"

NOT_REPLAYABLE_RX = re.compile(r"\b(send (it|that|this|an? (email|message|text|dm))|text (him|her|them)|post (it|this) to|email (him|her|them|ana|the team)|attached|this file|the attached|my (file|photo|screenshot))\b", re.I)
JUDGE_BY_CLASS = {
    "quick-fact": "One message for a quick ask, speaks to the user in the second person, states the fact for the user's moment, no filler, no handle.",
    "one-liner-list": "One message: a short lead line, real sections with bullets, ends with one offer. Compare with the Grok baseline's structure.",
    "research": "Acknowledgement first, then the result with sources and what could not be checked, then one next-step offer; several short messages, not one dense paragraph.",
    "research-compare": "A one-line acknowledgement first, then the conclusion in one or two sentences, a markdown table with one row per item and a link per row, then details, caveats and sources, ending with one concrete next-step offer.",
    "routine-or-reminder": "One short confirmation naming the time and what will happen; no unnecessary question.",
    "draft": "The draft itself as a message or card; nothing sent; one short line asking whether to send or change.",
    "stop": "One short line confirming the stop; no result after it.",
}


def shape_for(t, cls):
    n = max(1, t["bubbles"])
    shape = {
        "messages_min": max(1, n - 1),
        "messages_max": n + 1,
        "ack_first": bool(t["delegated"]),
        "table": bool(t["table"]),
        "bullets": bool(t["bullets"]),
        "links_min": min(int(t["links"]), 2),
        "offer": bool(t["offer"]),
        "first_bubble_max_s": 5,
        "no_handle": True,
        "worker_rows": 0,
        "reply_to_ask": bool(t["delegated"]),
    }
    if t["delegated"]:
        shape["status_line_max_s"] = 2
        shape["sources"] = True
    return shape


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--force", action="store_true", help="rewrite tasks that already exist")
    args = ap.parse_args()
    index = json.loads(INDEX.read_text())
    TASKS.mkdir(parents=True, exist_ok=True)
    written = 0
    for cid, conv in sorted(index["conversations"].items()):
        if conv.get("task"):
            continue  # covered by an existing parity task (seed baselines)
        tid = f"cv-{cid}"
        out = TASKS / f"{tid}.json"
        if out.exists() and not args.force:
            continue
        msgs = [json.loads(l) for l in (HERE / conv["file"]).read_text().splitlines() if l.strip()]
        wins = traits.windows(msgs)
        turns = []
        replayable = True
        delegated_any = False
        prev_ts = None
        for i, (w, t) in enumerate(zip(wins, conv["prompts"]), 1):
            cls = t.get("class", "quick-fact")
            prompt = t["prompt"]
            if t.get("attachments") or NOT_REPLAYABLE_RX.search(prompt):
                replayable = False
            if t.get("is_stop"):
                turn = {"kind": "stop", "shape": {"stop_within_s": 3, "no_result_after": True}}
            else:
                turn = {"prompt": prompt, "shape": shape_for(t, cls), "judge": {"criteria": JUDGE_BY_CLASS.get(cls, JUDGE_BY_CLASS["quick-fact"])}}
            ts = w.get("ts")
            if i > 1 and t.get("busy") and ts is not None and prev_ts is not None:
                turn["at_s"] = max(1, round(ts - prev_ts))
            prev_ts = ts
            delegated_any |= bool(t.get("delegated"))
            turns.append(turn)
        lane = "ab"[int(hashlib.sha256(tid.encode()).hexdigest(), 16) % 2]
        task = {
            "id": tid,
            "lane": lane,
            "category": "convergence",
            "categories": ["convergence", conv["prompts"][0].get("class", "quick-fact")],
            "title": f"Grok corpus {cid}: {conv['prompts'][0]['prompt'][:70]}",
            "parity": True,
            "replayable": replayable,
            "timeout_s": 420 if delegated_any else 180,
            "settle_s": 20 if delegated_any else 12,
            "turns": turns,
            "baseline": f"baselines/grok/{tid}.md",
            "convergence": {"conversation": cid, "source": conv.get("source"), "grok": conv["prompts"]},
        }
        out.write_text(json.dumps(task, indent=2, ensure_ascii=False) + "\n")
        grok_bubbles = [m for m in msgs if m.get("role") == "grok" and (m.get("text") or "").strip()]
        fm = [f"task: {tid}", f"prompt: {json.dumps(conv['prompts'][0]['prompt'])}", "bot: Grok Bot (corpus drop, PII-scrubbed)",
              f"captured_at: {conv.get('captured_at') or 'unknown'}", "source: corpus", f"message_count: {len(grok_bubbles)}",
              f"status_line: {json.dumps(conv['prompts'][0].get('status_line_texts') or [])}"]
        body = "\n\n".join(f"--- {i} ---\n{m['text']}" for i, m in enumerate(grok_bubbles, 1))
        (BASELINES / f"{tid}.md").write_text("---\n" + "\n".join(fm) + "\n---\n\n" + body + "\n")
        written += 1
        print(f"{tid}: lane {lane}, {len(turns)} turn(s), replayable={replayable}")
    print(f"wrote {written} task(s)")


if __name__ == "__main__":
    main()
