#!/usr/bin/env python3
"""Do the four new checkpoint rules fire, and stay quiet on a healthy place?

A rule nobody has seen fire is a rule nobody should trust. Four staged faults, each the shape the
rule names, plus a clean arm that must produce nothing.
"""
import json
import pathlib
import shutil
import sys
import tempfile

sys.path.insert(0, "/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa")
import consistency


def place(tmp, transcript_lines=6, sidecars=None):
    p = pathlib.Path(tmp)
    root = p / ".arbos" / "agents" / "root"
    (root / "checkpoints.d").mkdir(parents=True)
    (p / ".arbos" / "runtime").mkdir(parents=True, exist_ok=True)
    (root / "agent.md").write_text("# root\n")
    rows = [json.dumps({"ts": 1, "kind": "wake", "wake": "kickoff", "text": "x"})]
    rows += [json.dumps({"ts": 1, "kind": "assistant", "text": "x"}) for _ in range(transcript_lines - 2)]
    rows += [json.dumps({"ts": 1, "kind": "turn_complete"})]
    (root / "transcript.jsonl").write_text("\n".join(rows) + "\n")
    for name, body in (sidecars or {}).items():
        (root / "checkpoints.d" / name).write_text(body)
    return p


def rules_for(**kw):
    tmp = tempfile.mkdtemp(prefix="cpcheck-")
    try:
        p = place(tmp, **kw)
        found = consistency.check_place(p)
        names = sorted({f["finding"] if isinstance(f, dict) and "finding" in f else f.get("rule", str(f)) for f in found})
        return [n for n in names if n.startswith("checkpoint")]
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


good = json.dumps({"line": 3, "ts": 1, "head": "a" * 40, "work": "b" * 40})
arms = {
    "clean (must be silent)": {"sidecars": {"3.json": good}},
    "unreadable": {"sidecars": {"3.json": "{not json"}},
    "clean-shaped (must be silent)": {"sidecars": {"3.json": json.dumps({"line": 3, "ts": 1, "head": "a" * 40, "clean": True})}},
    "incomplete (neither work nor clean)": {"sidecars": {"3.json": json.dumps({"line": 3, "ts": 1, "head": "a" * 40})}},
    "name/body mismatch": {"sidecars": {"9.json": good}},
    "past the transcript": {"sidecars": {"99.json": json.dumps({"line": 99, "ts": 1, "head": "a" * 40, "work": "b" * 40})}},
}
for label, kw in arms.items():
    print(f"  {label:<26} -> {rules_for(**kw) or 'nothing'}")
