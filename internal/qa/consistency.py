#!/usr/bin/env python3
"""State-consistency checker for an Arbos place (a folder with `.arbos/`).

Every rule below names the files that must agree and the code that owns them
on branch `rust`. A "finding" is one broken rule with the file it points at.

Usage:
    python3 consistency.py <place> [--kernel-running]

Exit code 0 = consistent, 1 = findings.
"""

import json
import os
import re
import sys
from pathlib import Path

TRANSCRIPT_END_KINDS = {"turn_complete", "interrupted"}
TERMINAL_STATUSES = {"done", "cancelled", "failed"}
ID_RE = re.compile(r"^[A-Za-z0-9_-]{1,64}$")


def read_jsonl(path):
    """Return (parsed_lines, bad_line_numbers). Blank lines are skipped."""
    parsed, bad = [], []
    if not path.exists():
        return parsed, bad
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        for i, line in enumerate(f, start=1):
            if not line.strip():
                continue
            try:
                parsed.append((i, json.loads(line)))
            except json.JSONDecodeError:
                bad.append(i)
    return parsed, bad


def parse_agent_md(text):
    fields = {}
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or ":" not in line:
            continue
        k, v = line.split(":", 1)
        fields[k.strip()] = v.strip()
    return fields


def pid_alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def fold_last(rows, key):
    latest = {}
    for _, row in rows:
        try:
            latest[key(row)] = row
        except (KeyError, TypeError):
            continue
    return latest


def check_place(place, kernel_running=False):
    """Return a list of findings. Each is a dict: rule, path, detail."""
    place = Path(place)
    arbos = place / ".arbos"
    findings = []

    def add(rule, path, detail):
        findings.append({"rule": rule, "path": str(path), "detail": detail})

    if not arbos.is_dir():
        add("arbos-dir", arbos, "missing .arbos/ (bootstrap in arbos-core/src/files.rs never ran)")
        return findings

    agents_dir = arbos / "agents"
    agent_ids = set()
    if agents_dir.is_dir():
        agent_ids = {p.name for p in agents_dir.iterdir() if p.is_dir()}

    # Newer kernels keep lock/kernel.json/focus under .arbos/runtime/.
    rt = arbos / "runtime" if (arbos / "runtime").is_dir() else arbos
    # focus -> an existing agent folder (files.rs bootstrap, serve.rs snapshot)
    focus = rt / "focus"
    if focus.exists():
        target = focus.read_text(errors="replace").strip()
        if target and not (place / target).is_dir():
            add("focus-target", focus, f"focus points at {target!r}, which is not a folder")
    else:
        add("focus-missing", focus, "no focus file")

    # kernel.json / lock agree with whether a kernel runs (serve.rs write_kernel_json, lock.rs)
    kjson = rt / "kernel.json"
    lock = rt / "lock"
    if kjson.exists():
        try:
            info = json.loads(kjson.read_text())
            alive = pid_alive(int(info.get("pid", -1)))
            if kernel_running and not alive:
                add("kernel-json-dead-pid", kjson, f"pid {info.get('pid')} is not alive but a kernel should run")
            if not kernel_running and alive:
                add("kernel-json-live-pid", kjson, f"pid {info.get('pid')} is alive but no kernel should run")
        except (ValueError, json.JSONDecodeError) as e:
            add("kernel-json-parse", kjson, f"unparseable: {e}")
    if lock.exists() and not kernel_running:
        add("lock-leftover", lock, "lock file left behind after the kernel stopped (lock.rs Drop did not run)")
    if kernel_running and not lock.exists():
        add("lock-missing", lock, "kernel runs but no lock file")

    # agent folders
    for aid in sorted(agent_ids):
        adir = agents_dir / aid
        if not ID_RE.match(aid):
            add("agent-id-invalid", adir, "folder name fails validate_id (agent.rs)")
        md = adir / "agent.md"
        if not md.exists():
            add("agent-md-missing", md, "agent folder without agent.md; list_agents skips it silently")
            continue
        fields = parse_agent_md(md.read_text(errors="replace"))
        parent = fields.get("parent", "")
        if parent and parent not in agent_ids:
            add("agent-parent-dangling", md, f"parent {parent!r} has no folder")
        for tmp in adir.glob(".agent.md.*.tmp"):
            add("agent-md-tmp-leftover", tmp, "atomic-save temp file left behind (agent.rs save)")

        # transcript
        tpath = adir / "transcript.jsonl"
        events, bad = read_jsonl(tpath)
        if bad:
            add("transcript-bad-lines", tpath, f"{len(bad)} unparseable line(s): {bad[:5]}")
        last_wake = last_end = None
        for i, ev in events:
            kind = ev.get("kind")
            if kind == "wake":
                last_wake = i
            elif kind in TRANSCRIPT_END_KINDS:
                last_end = i
        if last_wake is not None and (last_end is None or last_end < last_wake) and not kernel_running:
            add(
                "transcript-unended-turn",
                tpath,
                f"wake at line {last_wake} has no later turn_complete/interrupted; needs_serve() will refire it on every kernel start",
            )
        wakes = [i for i, ev in events if ev.get("kind") == "wake"]
        ends = [i for i, ev in events if ev.get("kind") in TRANSCRIPT_END_KINDS]
        if len(wakes) > len(ends) + 1:
            add(
                "transcript-wake-pileup",
                tpath,
                f"{len(wakes)} wakes but {len(ends)} turn ends: turns are starting without finishing",
            )

        # plan + attempts
        ppath = adir / "plan.jsonl"
        apath = adir / "attempts.jsonl"
        nodes_rows, bad_n = read_jsonl(ppath)
        att_rows, bad_a = read_jsonl(apath)
        if bad_n:
            add("plan-bad-lines", ppath, f"{len(bad_n)} unparseable line(s): {bad_n[:5]}")
        if bad_a:
            add("attempts-bad-lines", apath, f"{len(bad_a)} unparseable line(s): {bad_a[:5]}")
        nodes = fold_last(nodes_rows, lambda n: n["id"])
        attempts = fold_last(att_rows, lambda a: a["id"])
        for nid, n in nodes.items():
            if n.get("parent", 0) and n["parent"] not in nodes:
                add("plan-parent-dangling", ppath, f"node #{nid} has parent #{n['parent']} that does not exist")
            att = n.get("attempt")
            if att and att not in attempts:
                add("plan-attempt-dangling", ppath, f"node #{nid} points at attempt {att!r} that is not in attempts.jsonl")
            if n.get("status") == "active" and not kernel_running:
                add("plan-active-after-stop", ppath, f"node #{nid} is active with no kernel; reclaim() should settle it on next start")
            if n.get("status") in TERMINAL_STATUSES and att:
                add("plan-terminal-with-attempt", ppath, f"node #{nid} is {n['status']} but still holds attempt {att!r}")
        for aid_, a in attempts.items():
            if a.get("node") not in nodes:
                add("attempt-node-dangling", apath, f"attempt {aid_} refers to node #{a.get('node')} that does not exist")
            if a.get("ended_ms") is None and not kernel_running:
                add("attempt-running-after-stop", apath, f"attempt {aid_} never ended")
        # a done inbox node must have been served: its attempt must exist and end
        for nid, n in nodes.items():
            if n.get("status") == "done" and n.get("origin") == "user":
                served = any(a.get("node") == nid and a.get("ended_ms") for a in attempts.values())
                if not served:
                    add("plan-done-unattempted", ppath, f"user node #{nid} is done but no ended attempt records the turn")
                if n.get("outcome") == "(no reply)":
                    add(
                        "plan-done-no-reply",
                        ppath,
                        f"user node #{nid} is done with outcome '(no reply)': the turn produced nothing but was counted as success",
                    )

        # plan.md must match plan.jsonl existence
        pmd = adir / "plan.md"
        if nodes and not pmd.exists():
            add("plan-md-missing", pmd, "plan.jsonl has nodes but plan.md was never rendered")

        # jobs
        jobs = adir / "jobs"
        if jobs.is_dir():
            for j in jobs.iterdir():
                meta = j / "meta.json"
                if j.is_dir() and not meta.exists():
                    add("job-meta-missing", j, "job folder without meta.json")

    return findings


def main(argv):
    if len(argv) < 2:
        print(__doc__)
        return 2
    kernel_running = "--kernel-running" in argv
    findings = check_place(argv[1], kernel_running=kernel_running)
    print(json.dumps(findings, indent=2))
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
