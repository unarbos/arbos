#!/usr/bin/env python3
"""Turn one run of the voice gateway harness (voice-server/tests/out/report.json) into
QA-loop files: a history line and one bug draft per red scenario (deduped by scenario).

    call-mode-collect.py <report.json> <loop-dir> <branch> <sha> [<out-dir>]

Writes:
  <loop>/call-mode-history.jsonl                 one line per run: pass/fail counts, branch, sha
  <loop>/bugs/call-<scenario>.md                 draft bug for a red scenario (appended to when seen again)
"""

import json
import os
import shutil
import sys
import time


def main() -> int:
    report, loop, branch, sha = sys.argv[1:5]
    out_dir = sys.argv[5] if len(sys.argv) > 5 else os.path.join(os.path.dirname(report))
    try:
        results = json.load(open(report))
    except Exception as e:  # noqa: BLE001
        print(f"-- call-mode: no report ({e})")
        return 1
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    red = [r for r in results if not r.get("ok")]
    line = {
        "ts": stamp, "branch": branch, "sha": sha[:12], "scenarios": len(results), "pass": len(results) - len(red),
        "fail": [r["name"] for r in red],
    }
    with open(os.path.join(loop, "call-mode-history.jsonl"), "a") as f:
        f.write(json.dumps(line) + "\n")
    print(f"-- call-mode: {line['pass']}/{line['scenarios']} green on {branch}@{sha[:12]}" + (f"; red: {', '.join(line['fail'])}" if red else ""))

    bugs = os.path.join(loop, "bugs")
    os.makedirs(bugs, exist_ok=True)
    keep = os.path.join(loop, "rollouts")
    for r in red:
        name = r["name"]
        bundle = os.path.join(out_dir, name)
        dest = os.path.join(keep, f"{stamp}-call-{name}")
        if os.path.isdir(bundle):
            shutil.copytree(bundle, dest, ignore=shutil.ignore_patterns("place"), dirs_exist_ok=True)
        failed = [c["what"] for c in r.get("checks", []) if not c.get("ok")]
        path = os.path.join(bugs, f"call-{name}.md")
        if os.path.exists(path):
            with open(path, "a") as f:
                f.write(f"\n- seen again {stamp} on `{branch}` @ `{sha[:12]}`; bundle `{dest}`\n")
            continue
        body = [
            f"# call-{name}: {r.get('title') or name} (red in the voice harness)",
            "",
            f"- Feature: call mode (voice-server gateway), branch `{branch}` @ `{sha[:12]}`",
            f"- Scenario: `voice-server/tests/scenarios/{name}.toml`",
            f"- First seen: {stamp}",
            f"- Bundle: `{dest}` (gateway.log, frames.jsonl, audio.jsonl, inbox.json)",
            "",
            "## Repro",
            "",
            f"`cd voice-server && source .venv/bin/activate && python -m tests.run {name}`",
            "",
            "## Failed checks",
            "",
        ]
        body += [f"- {w}" for w in failed] or [f"- {r.get('error') or 'scenario error'}"]
        if r.get("error"):
            body += ["", f"Error: `{r['error']}`"]
        body += ["", "## Spoken", ""] + [f"- {s}" for s in r.get("spoken", [])[:20]]
        with open(path, "w") as f:
            f.write("\n".join(body) + "\n")
        print(f"-- call-mode: filed {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
