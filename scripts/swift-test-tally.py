#!/usr/bin/env python3
"""Count the Swift cases an `xcodebuild test` log actually ran, case by case, and
reconcile them against the cases declared in the tree.

    scripts/swift-test-tally.py <xcodebuild-test.log> --ref <commit>
    scripts/swift-test-tally.py <xcodebuild-test.log> --worktree <path>

Exits 0 only when every declared case is accounted for, nothing ran that is not
declared, and no case failed. Anything else is a non-zero exit and a named list,
because a counter that can only report is a counter people talk themselves past.

THE EXPECTED TOTAL IS COMPUTED HERE, NOT PASSED IN, and the commit it was computed
from is printed with its subject so a report says what was counted. Main moves
under you: on 2026-10-31 `9818583d1` declared 510 unit + 39 UI = 549 and
`482bd9103` declared 512 + 44 = 556 the same evening. A tool that takes the
expected number as an argument gets fed a stale one, and a branch's own base is
the wrong ref for a merge — count the commit the log was produced from.

The log comes from the `Swift core check` workflow's artifact
(`swift-core-check-logs-<sha>`), or from `apps/ios/Arbos/build/xcodebuild-test.log`
after a local `fastlane test`.

Four things make a naive grep wrong. Each was found in a real artifact, and three
separate lanes rediscovered them from scratch on 2026-10-31.

1. A FAILED BUILD IS NOT A TEST RESULT. If the test bundle does not compile, the log
   holds `** TEST FAILED **` and `error:` lines and *no* case records at all. Counted
   naively that reads as a tally, not as "nothing ran". This script says so first.

2. TWO RECORD FORMATS, because the Arbos scheme mixes parallel and serial bundles.
   ArbosTests is `parallelizable = YES`:

       Test case 'Suite.testName()' passed on 'Clone 1 of iPhone 17 - Arbos (14211)' (0.020 seconds)

   ArbosUITests is `parallelizable = NO`:

       Test Case '-[ArbosUITests.Suite testName]' passed (54.386 seconds).

   Note the lower-case "case" in the first and the capital "Case" in the second.
   Counting either alone silently drops the other bundle: on main at 9818583d1 the
   capital-C form alone gives 39 of 549.

3. SHREDDED RECORDS. The log is a `tee` of one stream that several writers share, so
   an observer line can interleave into a case record and cut it mid-word:

       Test case 'MessageMenuTests.testOwnMe2026-10-31 19:56:34.160 xcodebuild[19432:55719] [MT] IDE…

   That case ran. Only its record is damaged. THE TELL, for a lane that is one or two
   cases short with no failing case named, is a line that contains `Test case '` and
   never reaches `' passed`. Those are recovered here by shortening the captured name
   from the right until exactly one otherwise-unaccounted declaration still matches,
   so a recovery is unambiguous or it is reported as unresolved.

4. RERUNS. A case can appear more than once, so unique names are what count.

Declared cases are counted as `func test[A-Z][A-Za-z0-9_]*\\(\\)`. A bare `func test`
also matches helpers whose names merely begin with "test" — `private func
testingGroup()` in AddMembersPlanTests.swift is one — and inflates the target.
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

TEST_DIRS = ("apps/ios/Arbos/ArbosTests", "apps/ios/Arbos/ArbosUITests")

DECLARED = re.compile(r"func (test[A-Z][A-Za-z0-9_]*)\(\)")
SUITE = re.compile(r"\b(?:final\s+)?class\s+([A-Za-z0-9_]+)\s*:\s*XCTestCase")
PARALLEL = re.compile(r"Test case '([A-Za-z0-9_]+)\.(test[A-Za-z0-9_]*)\(\)' (passed|failed)")
SERIAL = re.compile(r"Test Case '-\[[A-Za-z0-9_]+\.([A-Za-z0-9_]+) (test[A-Za-z0-9_]*)\]' (passed|failed)")
DAMAGED = re.compile(r"Test case '([A-Za-z0-9_]+)\.(test[A-Za-z0-9_]*)")


def git(*args: str) -> str:
    return subprocess.run(["git", *args], capture_output=True, text=True, check=True).stdout


def cases_in(source: str, text: str, found: dict[str, list[str]]) -> None:
    """Add every `Suite.testName` declared in one file's `text` to `found[source]`."""
    suite = None
    for line in text.splitlines():
        m = SUITE.search(line)
        if m:
            suite = m.group(1)
        m = DECLARED.search(line)
        if m:
            found[source].append(f"{suite}.{m.group(1)}")


def declared_at_ref(ref: str) -> tuple[dict[str, list[str]], str]:
    found: dict[str, list[str]] = {d: [] for d in TEST_DIRS}
    for d in TEST_DIRS:
        for path in git("ls-tree", "--name-only", "-r", ref, f"{d}/").split():
            if path.endswith(".swift"):
                cases_in(d, git("show", f"{ref}:{path}"), found)
    sha = git("rev-parse", ref).strip()
    subject = git("log", "-1", "--format=%s", ref).strip()
    return found, f"{sha} ({subject})"


def declared_in_worktree(root: Path) -> tuple[dict[str, list[str]], str]:
    found: dict[str, list[str]] = {d: [] for d in TEST_DIRS}
    for d in TEST_DIRS:
        for path in sorted((root / d).glob("*.swift")):
            cases_in(d, path.read_text(errors="replace"), found)
    return found, f"worktree {root.resolve()} (files on disk, not a commit)"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("log", type=Path)
    src = ap.add_mutually_exclusive_group()
    src.add_argument("--ref", help="the commit the log was produced from (not the branch's base)")
    src.add_argument("--worktree", type=Path, help="count the files on disk under this repo root")
    args = ap.parse_args()

    text = args.log.read_text(errors="replace")
    lines = text.splitlines()

    passed: dict[str, None] = {}
    failed: dict[str, None] = {}
    damaged: list[tuple[str, str]] = []
    for line in lines:
        clean = False
        for pat in (PARALLEL, SERIAL):
            for m in pat.finditer(line):
                clean = True
                (passed if m.group(3) == "passed" else failed)[f"{m.group(1)}.{m.group(2)}"] = None
        if not clean and "Test case '" in line:
            m = DAMAGED.search(line)
            if m:
                damaged.append((f"{m.group(1)}.{m.group(2)}", line.strip()[:140]))

    if args.worktree:
        per_target, counted = declared_in_worktree(args.worktree)
    else:
        per_target, counted = declared_at_ref(args.ref or "HEAD")
    want = sorted(c for cases in per_target.values() for c in cases)
    wantset = set(want)

    recovered: dict[str, str] = {}
    for capture, raw in damaged:
        floor = capture.index(".test") + len(".test") + 4
        hit = None
        for cut in range(len(capture), floor - 1, -1):
            stem = capture[:cut]
            candidates = [w for w in wantset if w.startswith(stem)
                          and w not in passed and w not in failed and w not in recovered.values()]
            if len(candidates) == 1:
                hit = candidates[0]
                break
        recovered[hit or f"UNRESOLVED<{capture}>"] = raw

    resolved = {k for k in recovered if not k.startswith("UNRESOLVED<")}
    accounted = set(passed) | set(failed) | resolved
    missing = sorted(wantset - accounted)
    extra = sorted(accounted - wantset)

    build_failed = "** TEST FAILED **" in text
    errors = [l for l in lines if "error:" in l]

    print(f"log            {args.log}")
    print(f"counted at     {counted}")
    for d, cases in per_target.items():
        print(f"  {Path(d).name:<16} {len(cases)} declared")
    print(f"declared       {len(wantset)}")
    print(f"passed         {len(passed)}")
    print(f"failed         {len(failed)}")
    print(f"recovered      {len(resolved)} of {len(damaged)} damaged record(s)")
    for name, raw in recovered.items():
        print(f"    {name}\n      from: {raw}")
    print(f"accounted      {len(accounted)}")

    if not accounted and (build_failed or errors):
        print("\nNOTHING RAN. This log is a build failure, not a test result:")
        for e in dict.fromkeys(errors[:10]):
            print(f"    {e.strip()}")
        return 2

    if failed:
        print(f"\nFAILED CASES ({len(failed)}):")
        for name in sorted(failed):
            print(f"    {name}")
    if missing:
        print(f"\nDECLARED BUT UNACCOUNTED ({len(missing)}):")
        for name in missing:
            print(f"    {name}")
        print("    If a line in the log contains \"Test case '\" and never reaches \"' passed\",")
        print("    the record was shredded by an interleaving writer and the case did run.")
    if extra:
        print(f"\nRAN BUT NOT DECLARED ({len(extra)}):")
        for name in extra:
            print(f"    {name}")

    ok = not missing and not extra and not failed and not build_failed
    print(f"\n{'OK' if ok else 'MISMATCH'}: {len(accounted)} accounted of {len(wantset)} declared")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
