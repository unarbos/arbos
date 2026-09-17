#!/usr/bin/env python3
"""journey-record.py <run-dir> <target> <app-build> [notes...]

Turns a finished run's score.txt into the one line that goes into
internal/mobile-journey-runs.md's machine twin,
internal/mobile-journey-history.jsonl. Writes <run-dir>/record.json and
prints the line.

Why this exists: the line used to be typed out by hand from score.txt, and
what got typed was the app build. QA imports these verdicts and could not
say which kernel a pass was measured against, so it printed "kernel commit:
not recorded by the phone loop" beside every one of them. The commit is part
of the finding, not context around it, so it is written here by the runner
rather than remembered afterwards.

Two fields carry it:

  kernel_version      the kernel's own `--version` line, read from the
                      attach socket of the kernel this run talked to
                      (`kernel.py <target> hello`) — never the hub's
                      `/list`, whose git_sha is whichever process
                      registered last and can name a build the process you
                      are talking to is not running.
  kernel_version_end  the same line read again after the last step. A
                      kernel has been replaced under a run here before; when
                      the two disagree the run measured two builds and the
                      record says so instead of picking one.
  kernel_binary_gone  the kernel was running a file that had been deleted
                      (#385). It serves in that state and refuses every
                      spawn, which is JB-6, so a J2 failure against such a
                      kernel is the machine's fault and not the app's.
"""
import json
import pathlib
import sys
import time

VERDICTS = {"PASS": "pass", "FAIL": "fail", "U": "unverified", "EYE": "eye"}


def version_line(path):
    """First line of a kernel.py hello dump, or None when it did not answer."""
    try:
        line = path.read_text().splitlines()[0].strip()
    except (OSError, IndexError):
        return None
    return line if line.startswith("arbos-kernel ") else None


def git_sha(line):
    parts = (line or "").split()
    return parts[2] if len(parts) > 2 and parts[2] != "unknown" else None


def main():
    if len(sys.argv) < 4:
        sys.exit(__doc__)
    run = pathlib.Path(sys.argv[1])
    target, build = sys.argv[2], sys.argv[3]
    notes = " ".join(sys.argv[4:])

    steps, scored_twice = {}, {}
    for raw in (run / "score.txt").read_text().splitlines():
        parts = raw.split(None, 2)
        if len(parts) < 2 or parts[1] not in VERDICTS:
            continue
        step = parts[0]
        scored_twice[step] = scored_twice.get(step, 0) + 1
        steps[step] = VERDICTS[parts[1]]  # a step scored again keeps its last word

    start = version_line(run / "kernel-version.txt")
    end = version_line(run / "kernel-version-end.txt")
    gone = bool(start and "BINARY-GONE" in start) or bool(end and "BINARY-GONE" in end)

    record = {
        "ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "loop": "iphone",
        "target": target,
        "branch": build,
        "kernel_version": start,
        "kernel_git_sha": git_sha(start),
        "steps": steps,
        "evidence": f"media/mobile/journey/{run.name}/",
    }
    if gone:
        record["kernel_binary_gone"] = True
    if end and end != start:
        record["kernel_version_end"] = end
        record["kernel_changed_mid_run"] = True
    again = sorted(s for s, n in scored_twice.items() if n > 1)
    if again:
        record["scored_more_than_once"] = again
    if notes:
        record["notes"] = notes

    (run / "record.json").write_text(json.dumps(record, indent=1) + "\n")
    print(json.dumps(record, sort_keys=True))
    if not start:
        print("WARNING: the kernel never said hello; this run has no commit", file=sys.stderr)
    if record.get("kernel_changed_mid_run"):
        print("WARNING: the kernel changed under this run; both lines are in the record", file=sys.stderr)


if __name__ == "__main__":
    main()
