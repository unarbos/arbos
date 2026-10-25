#!/usr/bin/env python3
"""Render the scoreboard: SUMMARY.md across runs (in the repo) and a per-run markdown report.

  report.py --run <run-id> [--prev <run-id>] [--issues issues.json] [--report-out <file>] [--fixed "<text>"]
"""

import argparse
import json
import sys
import time
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
SB = HERE / "scoreboard"


def load(run_id):
    return json.loads((SB / f"{run_id}.json").read_text())


def all_runs():
    runs = []
    for p in sorted(SB.glob("*.json")):
        try:
            runs.append(json.loads(p.read_text()))
        except Exception:
            pass
    return runs


def median(xs):
    xs = sorted(x for x in xs if x is not None)
    if not xs:
        return 0.0
    n = len(xs)
    return xs[n // 2] if n % 2 else (xs[n // 2 - 1] + xs[n // 2]) / 2


def fmt(v, suffix=""):
    return "—" if v is None else f"{v}{suffix}"


def summary_md(runs):
    lines = [
        "# Long-task eval — scoreboard",
        "",
        "One row per run, newest last. `pass` counts tasks whose programmatic checks and rubric all passed. `ttfr` = time to first reply (median over tasks). Per-task records: `<run>.json`.",
        "",
        "| run | image | tasks | pass | pass % | ttfr p50 | total p50 | tool errors | stalls | cost (relay) | judge |",
        "|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for b in runs:
        m, s = b["meta"], b["summary"]
        lines.append(
            f"| {m['run_id']} | {m.get('image')} | {s['run']}/{s['tasks']} | {s['passed']} | {fmt(s['pass_pct'], ' %')} | {fmt(s['ttfr_p50_s'], ' s')} | {fmt(s['total_p50_s'], ' s')} | {s['tool_errors']} | {s['stalls']} | ${s.get('run_cost_usd') if s.get('run_cost_usd') is not None else s['cost_usd']:.2f} | ${s.get('judge_cost_usd') or 0:.2f} |"
        )
    conv = [b for b in runs if (b["summary"].get("convergence") or {}).get("tasks")]
    if conv:
        lines += ["", "## Grok convergence (docs/grok-convergence-loop.md)", "", "| run | image | pass | largest gap | $ / turn | traits (Arbos % · gap pp) |", "|---|---|---|---|---|---|"]
        for b in conv:
            c = b["summary"]["convergence"]
            tr = ", ".join(f"{t} {round(100 * v['arbos_share'])}%·{v['gap_pp']}" for t, v in c["traits"].items() if v.get("arbos_share") is not None)
            lg = f"{c['largest_gap']['trait']} {c['largest_gap']['gap_pp']} pp" if c.get("largest_gap") else "—"
            lines.append(f"| {b['meta']['run_id']} | {b['meta'].get('image')} | {c['passed']}/{c['tasks']} | {lg} | ${c['cost_per_turn_usd']:.4f} | {tr} |")
    par = [b for b in runs if (b["summary"].get("parity") or {}).get("tasks")]
    if par:
        lines += ["", "## Grok parity (docs/duplex-agent-design.md §4)", "", "| run | image | parity pass | first bubble p50 / p95 | judge mean | traits (pass/total) |", "|---|---|---|---|---|---|"]
        for b in par:
            p = b["summary"]["parity"]
            tr = ", ".join(f"{t} {v['pass']}/{v['total']}" for t, v in sorted(p["per_trait"].items()))
            lines.append(f"| {b['meta']['run_id']} | {b['meta'].get('image')} | {p['passed']}/{p['tasks']} | {fmt(p['first_bubble_p50_s'], ' s')} / {fmt(p['first_bubble_p95_s'], ' s')} | {fmt(p['judge_overall'])} | {tr} |")
    if runs:
        last = runs[-1]
        lines += ["", f"## Per category — {last['meta']['run_id']}", "", "| category | tasks | passed | % |", "|---|---|---|---|"]
        for c, v in sorted(last["summary"]["categories"].items()):
            lines.append(f"| {c} | {v['tasks']} | {v['passed']} | {fmt(v['pass_pct'])} |")
        lines += ["", "## Per task — pass/fail across runs", ""]
        ids = sorted({r["task"] for b in runs for r in b["results"]})
        hdr = "| task | " + " | ".join(b["meta"]["run_id"] for b in runs) + " |"
        lines += [hdr, "|---|" + "---|" * len(runs)]
        for tid in ids:
            cells = []
            for b in runs:
                r = next((x for x in b["results"] if x["task"] == tid), None)
                cells.append("—" if r is None else ("skip" if r.get("skipped") else ("✓" if r["success"] else "✗")))
            lines.append(f"| {tid} | " + " | ".join(cells) + " |")
    return "\n".join(lines) + "\n"


def weaknesses(board, n=5):
    """Top weaknesses: group failures by the first words of their reason."""
    c = Counter()
    examples = {}
    for r in board["results"]:
        if r.get("success") or r.get("skipped"):
            continue
        for reason in r.get("reasons") or []:
            key = reason.split(":")[0]
            key = key.replace("t1", "").replace("t2", "").replace("t3", "").replace("t4", "").strip() or reason[:40]
            if "first reply took" in reason:
                key = "slow first reply"
            elif "rubric" in reason:
                key = "weak answer quality (rubric)"
            elif "routine" in reason.lower():
                key = "scheduling (routines)"
            elif "did not finish" in reason or "timed out" in reason:
                key = "turn did not finish"
            elif "question" in reason:
                key = "no confirmation before a risky action"
            c[key] += 1
            examples.setdefault(key, []).append(r["task"])
    return [(k, v, sorted(set(examples[k]))) for k, v in c.most_common(n)]


def parity_section(board, prev):
    p = (board["summary"].get("parity") or {})
    if not p.get("tasks"):
        return []
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from lib import parity as py
    lines = ["", "## Grok parity", "", f"**{p['passed']}/{p['tasks']} parity tasks passed**; first bubble p50 {fmt(p['first_bubble_p50_s'], ' s')} / p95 {fmt(p['first_bubble_p95_s'], ' s')}; judge mean {fmt(p['judge_overall'])} over {p['with_baseline']} task(s) with a Grok baseline.", "", "| trait | pass / total |", "|---|---|"]
    for t, v in sorted(p["per_trait"].items()):
        lines.append(f"| {t} | {v['pass']} / {v['total']} |")
    if p.get("judge_mean"):
        lines += ["", "| judge trait | mean 0–5 |", "|---|---|"]
        for t, v in sorted(p["judge_mean"].items()):
            lines.append(f"| {t} | {v} |")
    pp = (prev or {}).get("summary", {}).get("parity") if prev else None
    if pp:
        problems = py.gate(p, pp)
        lines += ["", f"Gate vs `{prev['meta']['run_id']}`: " + ("**held** (no trait dropped, judge within 0.2)" if not problems else "**FAILED** — " + "; ".join(problems))]
    worst = sorted((r for r in board["results"] if r.get("parity")), key=lambda r: (r["success"], -(len(r.get("reasons") or []))))[:3]
    if worst:
        lines += ["", "Worst parity tasks: " + "; ".join(f"`{r['task']}` ({'; '.join(r.get('reasons') or ['pass'])[:120]})" for r in worst)]
    return lines


def run_report(board, prev, issues, fixed_text):
    m, s = board["meta"], board["summary"]
    date = m["started_at"][:10]
    lines = [f"# Agent eval run {m['run_id']} — {date}", ""]
    lines.append(f"*Image {m.get('image')}, agentd `{m.get('agentd')}`, {m.get('provider')} sandbox of test user `{m.get('user')}`; {s['run']} of {s['tasks']} tasks ran ({s['skipped']} skipped), {m['started_at']} → {m.get('ended_at')}. Judge: {m.get('judge_model')}.*")
    lines.append("")
    lines.append("## Pass rate vs last run")
    lines.append("")
    if prev:
        ps = prev["summary"]
        delta = (s["pass_pct"] or 0) - (ps["pass_pct"] or 0)
        lines.append(f"**{s['passed']}/{s['run']} passed ({fmt(s['pass_pct'], ' %')})** vs {ps['passed']}/{ps['run']} ({fmt(ps['pass_pct'], ' %')}) in `{prev['meta']['run_id']}` — {'+' if delta >= 0 else ''}{round(delta, 1)} points.")
        prev_map = {r["task"]: r for r in prev["results"]}
        newly_fixed = [r["task"] for r in board["results"] if r["success"] and r["task"] in prev_map and not prev_map[r["task"]]["success"]]
        newly_broken = [r["task"] for r in board["results"] if not r["success"] and not r.get("skipped") and r["task"] in prev_map and prev_map[r["task"]]["success"]]
        lines.append("")
        lines.append(f"- newly passing: {', '.join(newly_fixed) or 'none'}")
        lines.append(f"- newly failing: {', '.join(newly_broken) or 'none'}")
    else:
        lines.append(f"**{s['passed']}/{s['run']} passed ({fmt(s['pass_pct'], ' %')})** — first run, no comparison.")
    lines += [
        "",
        f"Metrics: first reply p50 {fmt(s['ttfr_p50_s'], ' s')} (all turns {fmt(s['ttfr_all_p50_s'], ' s')}), total per task p50 {fmt(s['total_p50_s'], ' s')}, tool errors {s['tool_errors']}, stalls {s['stalls']}, cost ${s.get('run_cost_usd') if s.get('run_cost_usd') is not None else s['cost_usd']:.2f} relay-metered + ${s.get('judge_cost_usd') or 0:.2f} judge.",
        "",
        "| category | passed / tasks |",
        "|---|---|",
    ]
    for c, v in sorted(s["categories"].items()):
        lines.append(f"| {c} | {v['passed']} / {v['tasks']} |")
    lines += parity_section(board, prev)
    if board["summary"].get("convergence"):
        from lib import convergence as cv
        lines += cv.section(board["summary"]["convergence"], (prev or {}).get("summary", {}).get("convergence") if prev else None)
    lines += ["", "## Top weaknesses", ""]
    ws = weaknesses(board)
    if not ws:
        lines.append("None — every task passed.")
    for k, v, ex in ws:
        lines.append(f"- **{k}** ({v}): {', '.join(ex)}")
    lines += ["", "## Failures and issues", ""]
    issue_map = {t: u for t, u in (issues or [])}
    fails = [r for r in board["results"] if not r.get("success")]
    if not fails:
        lines.append("None.")
    for r in fails:
        link = issue_map.get(r["task"], "")
        link = f" → {link}" if link else ""
        lines.append(f"- `{r['task']}` — {'; '.join(r.get('reasons') or [])[:220]}{link}")
    lines += ["", "## What was fixed since the last run", "", fixed_text or ("— (first run)" if not prev else "— nothing landed between the runs"), "", "## What's next", ""]
    nxt = []
    for k, v, ex in ws[:3]:
        nxt.append(f"- fix the root cause behind **{k}** ({', '.join(ex[:3])}); the tasks stay as regression cases")
    nxt.append("- add tasks where this run was weakest; keep the per-run cost under the line")
    lines += nxt
    by_route = {}
    for r in board["results"]:
        if not r.get("skipped"):
            by_route.setdefault(r.get("route_t1") or "?", []).append(r.get("cost_usd") or 0)
    if by_route:
        lines += ["", "Cost by first-turn route (a `new` route spawns a Sonnet helper and costs ≈ 5× an `act` turn on Haiku, so a task's cost is bimodal): "
                  + ", ".join(f"`{k}` × {len(v)} → median ${median(v):.3f}" for k, v in sorted(by_route.items()))]
    lines += ["", "## Per task", "", "| task | result | route (t1) | first reply | total | tool err | stalls | cost | rubric | reasons |", "|---|---|---|---|---|---|---|---|---|---|"]
    for r in board["results"]:
        res = "skip" if r.get("skipped") else ("pass" if r["success"] else "FAIL")
        lines.append(f"| `{r['task']}` | {res} | {r.get('route_t1') or '—'} | {fmt(r.get('ttfr_s'), ' s')} | {fmt(r.get('total_s'), ' s')} | {r.get('tool_errors', '—')} | {r.get('stalls', '—')} | ${(r.get('cost_usd') or 0):.3f} | {fmt(r.get('rubric_min'))} | {'; '.join(r.get('reasons') or [])[:160]} |")
    return "\n".join(lines) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True)
    ap.add_argument("--prev", default="")
    ap.add_argument("--issues", default="")
    ap.add_argument("--report-out", default="")
    ap.add_argument("--fixed", default="")
    ap.add_argument("--one-line", action="store_true", help="print the run's one-line summary and exit")
    args = ap.parse_args()
    runs = all_runs()
    board = load(args.run)
    if args.one_line:
        s = board["summary"]
        cost = s.get("run_cost_usd") if s.get("run_cost_usd") is not None else s["cost_usd"]
        print(f"{s['passed']}/{s['run']} passed ({s['pass_pct']} %), first reply p50 {s['ttfr_p50_s']} s, tool errors {s['tool_errors']}, cost ${cost:.2f}")
        return
    prev = load(args.prev) if args.prev else next((b for b in reversed(runs) if b["meta"]["run_id"] < args.run), None)
    issues = json.loads(Path(args.issues).read_text()) if args.issues and Path(args.issues).exists() else []
    (SB / "SUMMARY.md").write_text(summary_md(runs))
    rep = run_report(board, prev, issues, args.fixed)
    out = Path(args.report_out) if args.report_out else SB / f"{args.run}.md"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(rep)
    print(out)


if __name__ == "__main__":
    main()
