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
# The activity trace, which is where the handshake is readable. Case duration is not a usable
# stand-in: most of a slow first case is the suite's own `Waiting 90.0s for ... to exist`, which
# moves for reasons unrelated to the launch. The budget is spent between these two events.
TRACE = re.compile(r"^\s+t = +([\d.]+)s +(.*)$", re.M)
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


def handshake(block):
    """Seconds from `Launch com.unarbos…` to `Setting up automation session`, or None if the
    case never got there — which is what a wedged launch looks like."""
    launch = None
    for m in TRACE.finditer(block):
        t, label = float(m.group(1)), m.group(2)
        if launch is None:
            if label.startswith("Launch com.unarbos"):
                launch = t
        elif "Setting up automation session" in label:
            return round(t - launch, 2)
    return None


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
    nxt = FIRST_UI.search(log, first.end())
    shake = handshake(log[first.start():nxt.start() if nxt else len(log)])

    lines = [f"First UI case: `{cls}.{meth}` — {result or 'no verdict'}"
             + (f", {secs:.1f} s" if secs else ""),
             f"First-case launch handshake: "
             + (f"{shake:.2f} s" if shake is not None else "never completed"),
             f"Simulator clones in the log: {len(clones)}",
             f"Launch-handshake timeouts: {len(timeouts)}"]
    # Measured over 1,622 UI cases: the handshake is 1.08-3.31 s on the first case of a serial
    # run and 2.14 s median on every later case, against 37.25 s median and a 99.26 s maximum on
    # the first case of a cloned run. The background-assertion budget is about 90 s.
    if shake is not None and shake > 5:
        lines.append(f"Note: the first-case handshake took {shake:.2f} s against a ~90 s budget. "
                     "Serial runs measure 1-4 s; a cloned run medians 37 s and has been seen at "
                     "99 s, past the budget.")
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
