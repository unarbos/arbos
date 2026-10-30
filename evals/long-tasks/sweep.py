#!/usr/bin/env python3
"""Delete every routine on the eval lanes and report the box's spend.

Run after a crashed or budget-stopped run (and before any scheduled run): a routine an eval task created
keeps firing model turns after the run ends. `run.py` sweeps at its own end; this is the stand-alone copy.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from lib import relay as rl  # noqa: E402


def main():
    swept = 0
    for p in rl.projects():
        if not (p.get("name") or "").startswith("lane-"):
            continue
        for r in rl.routines(p["id"]):
            if r.get("id"):
                print(f"{p['name']}: deleting routine {r['id']} {r.get('name')!r} {r.get('trigger')}")
                rl.delete_routine(p["id"], r["id"])
                swept += 1
    spent, cap = rl.spent_today()
    print(f"swept {swept} routine(s); lt-eval spent today ${spent:.2f} of ${cap}")


if __name__ == "__main__":
    main()
