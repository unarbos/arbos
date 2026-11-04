#!/usr/bin/env python3
"""Report what the UI bundle's first app launch cost, and fail if the simulator was cloned.

The Swift gate's intermittent red was never a launch failure: `app.launch()` spawned a process
every time (four failing cases, four distinct pids), and what timed out was the handshake after
the spawn. It happened because the scheme marks `ArbosTests` parallelizable, so `xcodebuild test`
cloned the simulator and ran the unit bundle beside the UI bundle. The UI bundle's first launch
had to finish XCUITest's fixed waits on a cold clone while the other clone ran 530 unit tests.

`-parallel-testing-enabled NO` in the `test` lane removes that. This checks it stayed removed:
a clone in the log means the flag was dropped or overridden, and the flake is back. Nothing here
inspects test outcomes, so it cannot turn a real failure green or a real pass red.
"""
import os
import re
import sys

CLONE = re.compile(r"Clone \d+ of ")
FIRST_UI = re.compile(r"Test Case '-\[(ArbosUITests)\.(\w+) (\w+)\]' started")
# XCUITest's three harness-handshake timeouts, as observed across 248 gate attempts. All three
# are "a harness operation timed out", which is what distinguishes them from an app that will not
# start: `Failed to launch <app>: <anything else>` is a real failure and is deliberately not here.
HANDSHAKE_TIMEOUT = re.compile(
    r"error: -\[[\w.]+ \w+\] : Failed to (?:get background assertion|get launch progress|launch)\b"
    r"[^\n]*?Timed out while [^\n]*")


def case_result(log, cls, meth):
    m = re.search(r"Test Case '-\[ArbosUITests\.%s %s\]' (passed|failed) \((\d+\.\d+) seconds\)"
                  % (re.escape(cls), re.escape(meth)), log)
    return (m.group(1), float(m.group(2))) if m else (None, None)


def main(path):
    if not os.path.exists(path):
        print(f"{path}: no log, nothing to report")
        return 0
    log = open(path, errors="replace").read()
    first = FIRST_UI.search(log)
    if not first:
        print(f"{path}: the UI bundle did not run, nothing to report")
        return 0

    cls, meth = first.group(2), first.group(3)
    result, secs = case_result(log, cls, meth)
    clones = sorted(set(CLONE.findall(log)))
    timeouts = HANDSHAKE_TIMEOUT.findall(log)

    lines = [f"First UI case: `{cls}.{meth}` — {result or 'no verdict'}"
             + (f", {secs:.1f} s" if secs else ""),
             f"Simulator clones in the log: {len(clones)}",
             f"Launch-handshake timeouts: {len(timeouts)}"]
    if secs and secs > 30:
        lines.append(f"Note: the first UI launch cost {secs:.1f} s. Serial runs measure 13-21 s; "
                     "above ~60 s it starts to overrun XCUITest's fixed handshake waits.")
    for t in timeouts:
        lines.append(f"  {t.strip()}")

    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    body = "\n".join(f"- {line}" for line in lines)
    print(body)
    if summary:
        with open(summary, "a") as fh:
            fh.write("## UI first launch\n" + body + "\n")

    if clones:
        print("\nERROR: xcodebuild cloned the simulator, so the unit and UI bundles ran "
              "concurrently. docs/process-rules.md requires -parallel-testing-enabled NO, and the "
              "fastlane `test` lane passes it. Something dropped or overrode it; the first UI "
              "launch flake comes back with it.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "build/xcodebuild-test.log"))
