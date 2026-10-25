"""Grok convergence block for the scoreboard (docs/grok-convergence-loop.md §1 stage 2).

Folds the parity trait results of the convergence subset into one table: per trait, Arbos's pass
share on this run next to Grok's share on the same prompts (from evals/convergence/corpus/index.json,
tagged by the same rules at ingest), and the gap in percentage points. Traits where Grok's turn *is*
the target (cadence, formatting, offer, threading, when_to_delegate) show Grok as `reference` (100 %).
The largest gap is the next cycle's target. Nothing here talks to the box or a model.
"""

import json
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
CORPUS_INDEX = HERE.parent / "convergence" / "corpus" / "index.json"
TRAITS = ["speed", "ack_first", "cadence", "formatting", "offer", "threading", "busy_reply", "status_line", "worker_hidden", "stop", "when_to_delegate", "no_handle"]


def load_index():
    try:
        return json.loads(CORPUS_INDEX.read_text())
    except (OSError, json.JSONDecodeError):
        return {"conversations": {}, "grok_share": {}}


def subset_ids():
    """Task ids of the convergence subset: corpus-derived tasks + the parity replays that cover the
    traits the corpus under-covers (steer, stop, parallel asks, busy)."""
    corpus_tasks = [p.stem for p in sorted((HERE / "tasks" / "convergence").glob("*.json"))]
    replays = ["pj1-time-difference", "pj2-packing-list", "pj3-surf-schools", "py-15-steer-mid-task", "py-16-stop", "py-17-stop-one-of-two",
               "py-20-parallel-two", "py-21-parallel-three", "py-22-status-while-busy", "py-23-other-chat-while-busy"]
    return corpus_tasks + replays


def grok_tags_for(task_id, index):
    """The corpus prompt tags linked to a task: a corpus task carries them in `convergence.grok`; a
    seed baseline links by `task`."""
    for cid, conv in index.get("conversations", {}).items():
        if conv.get("task") == task_id or f"cv-{cid}" == task_id:
            return conv.get("prompts") or []
    return []


def arbos_delegated(rec_turn_msgs):
    """Did Arbos start background work for this turn? A hidden job row (`worker: true`) or a
    subagent row in the turn says yes."""
    return any(m.get("worker_flag") or m.get("is_worker_row") for m in rec_turn_msgs)


def summarize(results, index=None):
    index = index if index is not None else load_index()
    ids = set(subset_ids())
    recs = [r for r in results if r.get("parity") and r["task"] in ids]
    if not recs:
        return None
    arbos = {t: {"pass": 0, "total": 0} for t in TRAITS}
    for r in recs:
        for t, v in r["parity"]["traits"].items():
            if t in arbos:
                arbos[t]["pass"] += v["pass"]
                arbos[t]["total"] += v["total"]
        # when_to_delegate: compare per turn with the corpus tag (or the task's own ack_first shape when no corpus record)
        tags = grok_tags_for(r["task"], index)
        msgs = r["parity"].get("messages") or []
        for turn in r.get("turns") or []:
            if turn.get("kind") not in (None, "prompt"):
                continue
            i = turn["turn"]
            tag = tags[i - 1] if i - 1 < len(tags) else None
            # no corpus record (a scripted replay): the shape's ack_first says whether the prompt is long work
            expected = tag["delegated"] if tag is not None else ("ack_first" in (turn.get("traits") or {}))
            did = arbos_delegated([m for m in msgs if m.get("turn") == i])
            arbos["when_to_delegate"]["total"] += 1
            arbos["when_to_delegate"]["pass"] += 1 if did == expected else 0
    grok = index.get("grok_share") or {}
    rows = {}
    for t in TRAITS:
        a = arbos[t]
        a_share = (a["pass"] / a["total"]) if a["total"] else None
        g = grok.get(t) or {}
        if g.get("reference"):
            g_share, g_label = 1.0, "reference"
        elif g.get("total"):
            g_share, g_label = g["pass"] / g["total"], f"{g['pass']}/{g['total']}"
        else:
            # no timed Grok sample yet: the parity thresholds (≤ 5 s first bubble, ≤ 3 s stop, …) are the
            # Grok-level targets from the duplex design §4.5, so Grok counts as the reference until a drop
            # with timestamps measures it
            g_share, g_label = 1.0, "threshold"
        gap = round(100 * (g_share - a_share), 1) if (g_share is not None and a_share is not None) else None
        rows[t] = {"arbos": a, "arbos_share": round(a_share, 3) if a_share is not None else None, "grok": g_label, "grok_share": round(g_share, 3) if g_share is not None else None, "gap_pp": gap}
    ranked = sorted((t for t in rows if rows[t]["gap_pp"] is not None), key=lambda t: (-rows[t]["gap_pp"], -rows[t]["arbos"]["total"]))
    return {
        "tasks": len(recs),
        "passed": sum(1 for r in recs if r["success"]),
        "traits": rows,
        "largest_gap": {"trait": ranked[0], "gap_pp": rows[ranked[0]]["gap_pp"]} if ranked else None,
        "cost_usd": round(sum(r.get("cost_usd") or 0 for r in recs), 4),
        "cost_per_turn_usd": round(sum(r.get("cost_usd") or 0 for r in recs) / max(1, sum(len(r.get("turns") or []) for r in recs)), 4),
        "judge_mean": _judge_mean(recs),
        "corpus_conversations": len(index.get("conversations", {})),
    }


def _judge_mean(recs):
    out = {}
    keys = sorted({k for r in recs for k in (r["parity"].get("judge") or {})})
    for k in keys:
        xs = [r["parity"]["judge"][k] for r in recs if r["parity"].get("judge", {}).get(k) is not None]
        if xs:
            out[k] = round(sum(xs) / len(xs), 2)
    return out


def section(block, prev_block=None):
    """Markdown lines for the run report."""
    if not block:
        return []
    lines = ["", "## Grok convergence", "", f"**{block['passed']}/{block['tasks']} convergence tasks passed**; cost ${block['cost_usd']:.2f} (${block['cost_per_turn_usd']:.4f} per turn); corpus {block['corpus_conversations']} conversation(s).", "",
             "| trait | Arbos pass / total | Arbos % | Grok | gap (pp) |", "|---|---|---|---|---|"]
    for t, v in block["traits"].items():
        a = v["arbos"]
        pct = f"{100 * v['arbos_share']:.0f} %" if v["arbos_share"] is not None else "—"
        prev = ""
        if prev_block and (prev_block.get("traits") or {}).get(t, {}).get("arbos_share") is not None and v["arbos_share"] is not None:
            d = 100 * (v["arbos_share"] - prev_block["traits"][t]["arbos_share"])
            prev = f" ({'+' if d >= 0 else ''}{d:.0f})"
        lines.append(f"| {t} | {a['pass']} / {a['total']} | {pct}{prev} | {v['grok']} | {v['gap_pp'] if v['gap_pp'] is not None else '—'} |")
    if block.get("largest_gap"):
        lines += ["", f"Largest gap: **{block['largest_gap']['trait']}** ({block['largest_gap']['gap_pp']} pp) — the next cycle's target (ledger: project store `docs/grok-convergence-ledger.md`)."]
    return lines
