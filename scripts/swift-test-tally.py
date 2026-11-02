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

   Where the cut lands decides whether anything is lost, and only that. The same
   observer message hit the same case in two runs on consecutive commits: at 9818583d1
   it cut mid-name and the record needed recovering, and at d6b35db36 it cut after
   `' passed`, leaving the record readable and taking only the trailing clone stamp.
   So a truncated `passed on 'Clone N …'` is not a missing case — but it does make the
   clone stamps one short of the records, which is a real trap for anyone counting
   those to check how work was spread across clones.

4. RERUNS. A case can appear more than once, so unique names are what count.

Declared cases are counted as `func test[A-Z][A-Za-z0-9_]*\\(\\)`. A bare `func test`
also matches helpers whose names merely begin with "test" — `private func
testingGroup()` in AddMembersPlanTests.swift is one — and inflates the target.

A fifth error is not in the log at all, and no amount of care with the log finds it.
`Arbos.xcodeproj` is NOT filesystem-synchronized, so a new test file compiles into
nothing until it is registered in the project file by hand. Its cases are declared in
the tree and absent from the log, and the two agree with each other while both are
wrong about reality: the gate goes green covering none of them. So the project file is
read here too, every test file is checked in all four places it has to appear, and an
unregistered file is its own exit code (3) — "never built" is a different failure from
"built and failing", and a caller should not have to guess which it has.
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

TEST_DIRS = ("apps/ios/Arbos/ArbosTests", "apps/ios/Arbos/ArbosUITests")
PBXPROJ = "apps/ios/Arbos/Arbos.xcodeproj/project.pbxproj"

# Object ids are matched structurally, never by width. This project holds 795 ids of 24
# characters and 7 hand-written ones of 22, and a fixed `{24}` silently skips the short
# ones — which reads as "this file is unregistered" for a file that is registered fine.
ID = r"[A-Za-z0-9]+"
BUILD_FILE = re.compile(rf"^\t+({ID}) /\* .*? \*/ = \{{isa = PBXBuildFile; fileRef = ({ID})", re.M)
FILE_REF = re.compile(rf"^\t+({ID}) /\* .*? \*/ = \{{isa = PBXFileReference;([^}}]*)\}}", re.M)
PATH_IN = re.compile(r"path = (\"?)([^\";]+)\1;")
BLOCK = r"^\t+({id}) /\* (.*?) \*/ = \{{\s*isa = {isa};(.*?)\n\t\t\}};"
GROUP = re.compile(BLOCK.format(id=ID, isa="PBXGroup"), re.S | re.M)
SOURCES = re.compile(BLOCK.format(id=ID, isa="PBXSourcesBuildPhase"), re.S | re.M)
TARGET = re.compile(BLOCK.format(id=ID, isa="PBXNativeTarget"), re.S | re.M)
LISTED = re.compile(rf"^\t+({ID})[ ,]", re.M)

DECLARED = re.compile(r"func (test[A-Z][A-Za-z0-9_]*)\(\)")
SUITE = re.compile(r"\b(?:final\s+)?class\s+([A-Za-z0-9_]+)\s*:\s*XCTestCase")
PARALLEL = re.compile(r"Test case '([A-Za-z0-9_]+)\.(test[A-Za-z0-9_]*)\(\)' (passed|failed)")
SERIAL = re.compile(r"Test Case '-\[[A-Za-z0-9_]+\.([A-Za-z0-9_]+) (test[A-Za-z0-9_]*)\]' (passed|failed)")
DAMAGED = re.compile(r"Test case '([A-Za-z0-9_]+)\.(test[A-Za-z0-9_]*)")


def git(*args: str) -> str:
    return subprocess.run(["git", *args], capture_output=True, text=True, check=True).stdout


def cases_in(text: str) -> list[str]:
    """Every `Suite.testName` declared in one file's `text`."""
    found, suite = [], None
    for line in text.splitlines():
        m = SUITE.search(line)
        if m:
            suite = m.group(1)
        m = DECLARED.search(line)
        if m:
            found.append(f"{suite}.{m.group(1)}")
    return found


def section(text: str, name: str) -> str:
    m = re.search(rf"/\* Begin {name} section \*/(.*?)/\* End {name} section \*/", text, re.S)
    return m.group(1) if m else ""


def unregistered(pbxproj: str, names_by_dir: dict[str, list[str]]) -> dict[str, list[str]]:
    """Test files that will not compile into their bundle, and why, keyed by target.

    A test file has to appear in four places, and three of them looking right is the
    same as none: a PBXFileReference, a PBXBuildFile pointing at it, the group's
    children, and the Sources phase **of the target that should own it**. A file in the
    app target's Sources phase instead looks registered and still never runs in the test
    bundle.
    """
    refs = {i: PATH_IN.search(body) for i, body in FILE_REF.findall(section(pbxproj, "PBXFileReference"))}
    ref_path = {i: m.group(2) for i, m in refs.items() if m}
    build_of: dict[str, list[str]] = {}
    for build_id, ref_id in BUILD_FILE.findall(section(pbxproj, "PBXBuildFile")):
        build_of.setdefault(ref_id, []).append(build_id)

    group_kids: dict[str, set[str]] = {}
    for _, label, body in GROUP.findall(section(pbxproj, "PBXGroup")):
        kids = re.search(r"children = \((.*?)\);", body, re.S)
        group_kids.setdefault(label, set()).update(LISTED.findall(kids.group(1)) if kids else [])

    phase_files = {}
    for phase_id, _, body in SOURCES.findall(section(pbxproj, "PBXSourcesBuildPhase")):
        files = re.search(r"files = \((.*?)\);", body, re.S)
        phase_files[phase_id] = set(LISTED.findall(files.group(1)) if files else [])

    # Which target's Sources phase each build file ends up in.
    owner: dict[str, str] = {}
    for _, label, body in TARGET.findall(section(pbxproj, "PBXNativeTarget")):
        phases = re.search(r"buildPhases = \((.*?)\);", body, re.S)
        for phase_id in LISTED.findall(phases.group(1)) if phases else []:
            for build_id in phase_files.get(phase_id, ()):
                owner[build_id] = label

    problems: dict[str, list[str]] = {}
    for d, names in names_by_dir.items():
        target = Path(d).name
        for name in names:
            ids = [i for i, p in ref_path.items() if Path(p).name == name]
            if not ids:
                problems.setdefault(target, []).append(f"{name}: no PBXFileReference")
                continue
            ref_id = ids[0]
            builds = build_of.get(ref_id, [])
            missing = []
            if not builds:
                missing.append("no PBXBuildFile")
            if ref_id not in group_kids.get(target, set()):
                missing.append(f"not in the {target} group's children")
            owners = {owner[b] for b in builds if b in owner}
            if not owners:
                missing.append("in no target's Sources phase")
            elif target not in owners:
                missing.append(f"in the Sources phase of {', '.join(sorted(owners))}, not {target}")
            if missing:
                problems.setdefault(target, []).append(f"{name}: " + "; ".join(missing))
    return problems


def read_at_ref(ref: str) -> tuple[dict[str, dict[str, list[str]]], str, str]:
    found: dict[str, dict[str, list[str]]] = {d: {} for d in TEST_DIRS}
    for d in TEST_DIRS:
        for path in git("ls-tree", "--name-only", "-r", ref, f"{d}/").split():
            if path.endswith(".swift"):
                found[d][Path(path).name] = cases_in(git("show", f"{ref}:{path}"))
    sha = git("rev-parse", ref).strip()
    subject = git("log", "-1", "--format=%s", ref).strip()
    return found, git("show", f"{ref}:{PBXPROJ}"), f"{sha} ({subject})"


def read_worktree(root: Path) -> tuple[dict[str, dict[str, list[str]]], str, str]:
    found: dict[str, dict[str, list[str]]] = {d: {} for d in TEST_DIRS}
    for d in TEST_DIRS:
        for path in sorted((root / d).glob("*.swift")):
            found[d][path.name] = cases_in(path.read_text(errors="replace"))
    pbx = (root / PBXPROJ).read_text(errors="replace")
    return found, pbx, f"worktree {root.resolve()} (files on disk, not a commit)"


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
        by_file, pbxproj, counted = read_worktree(args.worktree)
    else:
        by_file, pbxproj, counted = read_at_ref(args.ref or "HEAD")

    problems = unregistered(pbxproj, {d: sorted(files) for d, files in by_file.items()})
    broken = {p.split(":", 1)[0] for names in problems.values() for p in names}

    # Declarations come from registered files only: a file that compiles into nothing
    # would otherwise be counted as expected and then never accounted for, which reads
    # as a mystery gap rather than as the real error.
    per_target = {
        d: [c for name, cases in files.items() if name not in broken for c in cases]
        for d, files in by_file.items()
    }
    skipped = {
        d: {name: cases for name, cases in files.items() if name in broken}
        for d, files in by_file.items()
    }
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
        excluded = sum(len(c) for c in skipped[d].values())
        note = f"  (+{excluded} in unregistered files, excluded)" if excluded else ""
        print(f"  {Path(d).name:<16} {len(cases)} declared{note}")
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

    if problems:
        total = sum(len(c) for d in skipped for c in skipped[d].values())
        print(f"\nUNREGISTERED TEST FILES ({len(broken)}, holding {total} case(s) that never build):")
        for target, names in sorted(problems.items()):
            for note in sorted(names):
                print(f"    {target}: {note}")
        for d in skipped:
            for name, cases in sorted(skipped[d].items()):
                print(f"    {name} declares {len(cases)} case(s): {', '.join(sorted(cases))}")
        print("    A test file needs all four: PBXBuildFile, PBXFileReference, the group's")
        print("    children, and the Sources phase of its own target. Three of four is none.")
        return 3

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
