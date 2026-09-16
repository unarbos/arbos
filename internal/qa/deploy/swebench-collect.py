#!/usr/bin/env python3
"""Turn a nightly SWE-bench run into one history line, failing bundles, and
regression drafts.

  swebench-collect.py --cost-only <outputs/<half>>          print USD spent so far
  swebench-collect.py <run-dir> <sha> <branch> <model> <history.jsonl> <bundles-out> <bugs-out>

Reads every traces.jsonl under <run-dir>/outputs (verifiers writes one per
half). Solved = rewards.solved.score == 1.0. Cost = sum of per-call usage.cost.
A history line: {ts, sha, branch, model, instances, solved, failed, errored,
cost_usd, per_instance: {name: {solved, cost, tool_calls}}}. Regressions =
instances solved in the previous line and not now; each gets a bug draft.
"""

import datetime as dt
import glob
import hashlib
import json
import os
import shutil
import sys
from pathlib import Path


def traces(root):
    for path in glob.glob(os.path.join(root, "**", "traces.jsonl"), recursive=True):
        for line in open(path, errors="replace"):
            if not line.strip():
                continue
            try:
                yield json.loads(line)
            except json.JSONDecodeError:
                continue


def rows_from(outputs_dir):
    rows = {}
    for ep in traces(outputs_dir):
        name = (ep.get("task", {}).get("data", {}).get("name") or "").split("/")[-1]
        for tr in ep.get("traces", []):
            cost = sum((c.get("usage", {}) or {}).get("cost", 0.0) or 0.0 for c in tr.get("calls", []))
            solved = ((tr.get("rewards") or {}).get("solved") or {}).get("score")
            m = tr.get("metrics") or {}
            rows[name] = {
                "solved": solved,
                "cost": round(cost, 4),
                "model_calls": len(tr.get("calls", [])),
                "tool_calls": m.get("arbos_tool_calls"),
                "patch_bytes": m.get("arbos_patch_bytes"),
                "stop": tr.get("stop_condition"),
                "errors": tr.get("errors"),
                "trace_id": tr.get("id"),
            }
    return rows


def main(argv):
    if argv[1] == "--cost-only":
        print(round(sum(r["cost"] for r in rows_from(argv[2]).values()), 4))
        return 0
    run_dir, sha, branch, model, history, bundles_out, bugs_out = argv[1:8]
    run_dir = Path(run_dir)
    rows = rows_from(run_dir / "outputs")
    solved = sorted(n for n, r in rows.items() if r["solved"] == 1.0)
    failed = sorted(n for n, r in rows.items() if r["solved"] == 0.0)
    errored = sorted(n for n, r in rows.items() if r["solved"] is None)
    line = {
        "ts": int(dt.datetime.now(dt.timezone.utc).timestamp() * 1000),
        "run": run_dir.name,
        "sha": sha,
        "branch": branch,
        "model": model,
        "instances": len(rows),
        "solved": len(solved),
        "failed": len(failed),
        "errored": len(errored),
        "cost_usd": round(sum(r["cost"] for r in rows.values()), 4),
        "per_instance": rows,
    }
    # previous run on the same branch, for the diff
    prev = None
    hp = Path(history)
    if hp.exists():
        for l in hp.read_text().splitlines():
            try:
                cand = json.loads(l)
            except json.JSONDecodeError:
                continue
            if cand.get("branch") == branch and cand.get("instances"):
                prev = cand
    regressions, fixed = [], []
    if prev:
        for n, r in rows.items():
            was = (prev.get("per_instance") or {}).get(n, {}).get("solved")
            if was == 1.0 and r["solved"] != 1.0:
                regressions.append(n)
            if was == 0.0 and r["solved"] == 1.0:
                fixed.append(n)
    line["regressions"] = regressions
    line["newly_fixed"] = fixed
    line["previous"] = {"run": prev["run"], "sha": prev["sha"], "solved": prev["solved"]} if prev else None
    with open(hp, "a") as f:
        f.write(json.dumps(line) + "\n")
    print(f"== swebench {line['solved']}/{line['instances']} solved, {line['errored']} errored, ${line['cost_usd']} on {sha}" + (f"; regressions: {regressions}" if regressions else "") + (f"; newly fixed: {fixed}" if fixed else ""))

    # failing bundles: the artifact folder's small files, never the tarball twice
    out = Path(bundles_out)
    out.mkdir(parents=True, exist_ok=True)
    for name in failed + errored:
        r = rows[name]
        arts = sorted(glob.glob(str(run_dir / "artifacts" / f"*{name}*")))
        dest = out / f"{name}--{run_dir.name}"
        dest.mkdir(parents=True, exist_ok=True)
        for a in arts:
            for f in ("result.json", "kernel.log", "patch.diff", "run.jsonl", "run.stderr", "task.json", "rollout.tar.gz"):
                p = Path(a) / f
                if p.exists() and p.stat().st_size < 20 * 1024 * 1024:
                    shutil.copyfile(p, dest / f)
        (dest / "summary.json").write_text(json.dumps({"instance": name, "run": run_dir.name, "sha": sha, "branch": branch, **r}, indent=2))

    # regressions become bug drafts the triage pass reads
    bugs = Path(bugs_out)
    bugs.mkdir(parents=True, exist_ok=True)
    for name in regressions:
        fp = hashlib.sha1(f"swebench|{name}".encode()).hexdigest()[:10]
        path = bugs / f"{fp}.md"
        if path.exists():
            continue
        path.write_text(
            f"# {fp}: swebench regression {name}\n\nstatus: draft (auto: solved on {prev['sha']}, failed on {sha})\n"
            f"scenario: swebench-nightly\nbranch: {branch}\nrun: {run_dir.name}\nbundle: {out / f'{name}--{run_dir.name}'}\n"
            f"fingerprints: {fp}\n\n## Detail\n\n{json.dumps(rows[name], indent=2)}\n\n## Repro\n\n"
            f"`arbos-kernel rollout replay <bundle>/rollout` (no model), or rerun the instance with `swebench-nightly.sh`.\n"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
