"""Click every reachable control once and see what breaks.

For each element that a real click would reach: click it, check the app still
answers, note what changed, take a screenshot, then press Escape and click an
empty spot to get back to a resting state. Findings go to a JSON report.

Run from the repo root::

    source .venv/bin/activate
    python desktop/driver/examples/sweep.py [--max N] [--include PATTERN]

Controls that leave the window (file dialogs, the settings window) or that
destroy things (archive, close, delete) are skipped by default; pass
``--include`` to widen the net once a workspace you can throw away is open.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from arbosdriver import Arbos, DriverError, _matches  # noqa: E402

OUT = Path("/tmp/arbos-driver/sweep")

# Things a blind click must not do to a real workspace.
SKIP = [
    "open-project*",  # native folder picker: blocks until a person answers
    "settings",  # opens a second window the driver does not see
    "project-archive-*",
    "surface-close-*",
    "surface-pane-close-*",
    "*-delete*",
    "composer-send",  # sends whatever the field holds to the agent
    "composer-attach",  # native file picker
    "composer-voice*",  # starts the microphone
    "sidebar-split",  # a drag handle, not a button
    "*-drop",  # drop zones
]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--max", type=int, default=200, help="stop after this many clicks")
    parser.add_argument("--include", action="append", default=[], help="pattern to click even if skipped by default")
    args = parser.parse_args()

    stamp = time.strftime("%H%M%S")
    run = OUT / stamp
    run.mkdir(parents=True, exist_ok=True)
    report: list[dict] = []
    seen: set[str] = set()

    with Arbos.launch(log=run / "app.log") as app:
        base = app.state()
        print(f"resting state: pane={base['showing']} projects={len(base['projects'])}")
        clicks = 0
        # The tree changes as we click (menus open, panes switch), so re-read
        # it after every click and pick the next unseen reachable element.
        while clicks < args.max:
            candidates = [
                el
                for el in app.snapshot()["elements"]
                if el["reachable"] and el["path"] not in seen and not skipped(el["path"], args.include)
            ]
            if not candidates:
                break
            el = candidates[0]
            seen.add(el["path"])
            clicks += 1
            entry = {"path": el["path"], "at": [el["cx"], el["cy"]]}
            try:
                before = app.state()
                app.click(el["path"])
                after = app.state()
                entry["changed"] = diff_keys(before, after)
                shot = run / f"{clicks:03d}-{el['id']}.png"
                app.screenshot(shot)
                entry["screenshot"] = str(shot)
                # Back to rest: Escape closes popovers; a click on the detail
                # column's top edge closes a menu that Escape did not.
                app.key("escape")
                rest = app.state()
                if rest["menu_open"]:
                    entry.setdefault("notes", []).append("menu did not close on escape")
                    app.click(x=after["sidebar_width"] + 40, y=200)
                if rest["sidebar_open"] != base["sidebar_open"]:
                    app.action("cydonia::ToggleSidebar")
                entry["ok"] = True
            except (DriverError, ConnectionError, OSError, TimeoutError) as err:
                entry["ok"] = False
                entry["error"] = str(err)
                print(f"!! {el['path']}: {err}")
                try:
                    app.hello()
                except Exception as dead:  # the app is gone: stop here
                    entry["fatal"] = str(dead)
                    report.append(entry)
                    break
            report.append(entry)
            mark = "ok" if entry["ok"] else "ERR"
            print(f"[{clicks:03d}] {mark} {el['id']:<32} changed={entry.get('changed', [])}")

    (run / "report.json").write_text(json.dumps(report, indent=2))
    bad = [entry for entry in report if not entry["ok"]]
    notes = [entry for entry in report if entry.get("notes")]
    print(f"\n{len(report)} clicks, {len(bad)} errors, {len(notes)} notes -> {run / 'report.json'}")
    return 1 if bad else 0


def skipped(path: str, include: list[str]) -> bool:
    if any(_matches(pattern, path) for pattern in include):
        return False
    return any(_matches(pattern, path) for pattern in SKIP)


def diff_keys(before: dict, after: dict) -> list[str]:
    """Top-level state keys whose value changed, ``projects`` compared as JSON."""
    changed = []
    for key in sorted(set(before) | set(after)):
        if json.dumps(before.get(key), sort_keys=True) != json.dumps(after.get(key), sort_keys=True):
            changed.append(key)
    return changed


if __name__ == "__main__":
    sys.exit(main())
