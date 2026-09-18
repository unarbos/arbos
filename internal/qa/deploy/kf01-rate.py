#!/usr/bin/env python3
"""`qal-j43` as a rate, read from the rollouts rather than from verdict lines.

The fault is a race — 5 of 6 at `2ea8d565`, 0 of 6 at its parent — so a single run says little and
one green does not close it. The verdict line cannot carry that: a run where the kickoff turn had
already finished at mint time proves nothing about the bug, and `kf-01` reports it as a pass
because nothing broke. That inflates any rate counted from `[pass ]`/`[break]` lines.

Three outcomes matter, and they are all in the notes:

  lost         `line_landed` false while the kickoff turn was running  — the bug
  kept         `line_landed` true  while the kickoff turn was running  — a real pass
  no window    the kickoff turn was not running at mint time           — proves nothing

Read it **within one build**. The totals below mix every app the rollouts were run against, and
this bug moves with the app — 5 of 6 at `2ea8d565`, 0 of 6 at its parent — so an aggregate rate
across builds is meaningless. The per-run lines are in time order; group them by when the loop
rebuilt its app (`grep "desktop main: building" logs/vm-loop.log`) before reading a rate off them.

Usage: kf01-rate.py [rollouts-dir ...]
"""
import json
import pathlib
import sys

DIRS = sys.argv[1:] or ["/home/ubuntu/arbos-qa/loop/rollouts", "/tmp/j42/rollouts"]


def read(d):
    try:
        return json.load(open(d / "result.json")).get("notes", {})
    except (OSError, ValueError):
        return None


rows = []
for base in DIRS:
    p = pathlib.Path(base)
    if not p.is_dir():
        continue
    for d in sorted(p.glob("*kf-01-a-chat-opened-during-kickoff*")):
        n = read(d)
        if n is None:
            continue
        if n.get("inconclusive") or not n.get("kickoff_running_when_minted"):
            outcome = "no window"
        elif n.get("line_landed"):
            outcome = "kept"
        else:
            outcome = "LOST"
        rows.append((d.name[:15], outcome, str(n.get("agent", ""))[-8:]))

if not rows:
    print("no kf-01 rollouts found in " + ", ".join(DIRS))
    sys.exit(0)

for stamp, outcome, agent in rows[-25:]:
    print(f"  {stamp}  {outcome:<9} {agent}")

lost = sum(1 for _, o, _ in rows if o == "LOST")
kept = sum(1 for _, o, _ in rows if o == "kept")
none = sum(1 for _, o, _ in rows if o == "no window")
conclusive = lost + kept
print(f"\n  {len(rows)} run(s): {lost} lost, {kept} kept, {none} with no window")
if conclusive:
    print(f"  rate over conclusive runs: {lost}/{conclusive} lost")
    print("  => qal-j43 is live" if lost else "  => no loss seen; needs several more before that means anything")
else:
    print("  no conclusive run yet — every one missed the kickoff window")
