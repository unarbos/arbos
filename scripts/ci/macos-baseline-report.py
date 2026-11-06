#!/usr/bin/env python3
"""What the nightly macOS baseline actually measured, read out of xcodebuild's own log.

    scripts/ci/macos-baseline-report.py report --log PATH --tests DIR [DIR ...] [--post]
    scripts/ci/macos-baseline-report.py selftest

WHY THE RUN CONCLUSION IS NOT THE ANSWER.

`** TEST SUCCEEDED **` means every case xcodebuild RAN passed. It says nothing about how many ran.
A suite that is in the tree, declares an `XCTestCase` subclass and `test` methods, and is listed in
`project.pbxproj`, can still be compiled into the bundle zero times, and then its absence looks
exactly like success from outside the log.

That is not a hypothetical. In run 42095537743 — the one real macOS execution main got in its last
forty `swift-core-check` runs — the string `TranscriptSpacing` does not occur anywhere in thirteen
thousand lines of `xcodebuild-test.log`, and the run printed `** TEST SUCCEEDED **`. The cause is in
`apps/ios/Arbos/Arbos.xcodeproj/project.pbxproj`, which defines two object ids twice:
`A1BB0000000000000000003C` names both `TranscriptSpacingUITests.swift` and
`AvatarBackSwipeUITests.swift` as a PBXFileReference, and `A1BB0000000000000000003D` names both as
a PBXBuildFile. Xcode resolves an object by its id and ignores the decorative comment beside it, so
the group entry and the Sources entry written for `TranscriptSpacingUITests.swift` both resolve to
`AvatarBackSwipeUITests.swift` instead — which is also why Xcode warns that
`AvatarBackSwipeUITests.swift` is a member of two groups both named `ArbosUITests`.

So a baseline that reports the job's conclusion reports nothing a lane can rely on. This reads the
log and answers three separate questions, because they have three different remedies:

  SOUND?   did a recognisable test run happen at all, and did every suite the tree declares appear
           in it. An unsound measurement is not a red suite and must not be read as one.
  RED?     which cases reported `failed`, by name.
  SKIPPED? which cases reported `skipped`. XCTSkip is neither a pass nor a failure, and counting it
           as a pass is how a suite quietly stops testing anything. Run 42095537743 had thirteen.

Exit status: 0 sound and green, 1 sound and red, 2 unsound — the measurement cannot be trusted and
nothing should be concluded from it in either direction.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

# The two shapes one `xcodebuild test` log carries at once. The UI bundle streams the legacy
# XCTest form while it runs; the unit bundle is reported in the newer per-clone form. Both appear
# in every log this repo produces, so a parser that knows only one silently halves the measurement.
CASE_LEGACY = re.compile(
    r"^Test Case '-\[(?:[A-Za-z0-9_]+\.)?([A-Za-z0-9_]+) ([A-Za-z0-9_:]+)\]' (passed|failed|skipped)"
)
CASE_MODERN = re.compile(
    r"^Test case '([A-Za-z0-9_]+)\.([A-Za-z0-9_]+)\(\)' (passed|failed|skipped)"
)
# A second, independent witness that a suite ran. xcodebuild occasionally interleaves its own
# timestamped chatter into the middle of a case line, which costs that line; losing every line of a
# one-case suite would otherwise read as a registration fault. Requiring either witness to be absent
# before accusing the project file keeps that misreading out.
SUITE_ANY = re.compile(r"^Test [Ss]uite '([A-Za-z0-9_]+)'")

DECLARED_CLASS = re.compile(r"^\s*(?:final\s+)?class\s+([A-Za-z0-9_]+)\s*:\s*XCTestCase\b", re.M)
DECLARED_METHOD = re.compile(r"\bfunc\s+test[A-Za-z0-9_]*\s*\(")

ISSUE_TITLE = "Nightly macOS baseline: the iOS/Mac suite's health on main"


def parse_log(text):
    """-> (cases, suites_seen). cases is [(suite, case, verdict)] in the order xcodebuild said it."""
    cases, suites_seen = [], set()
    for line in text.splitlines():
        m = CASE_LEGACY.match(line) or CASE_MODERN.match(line)
        if m:
            cases.append((m.group(1), m.group(2), m.group(3)))
            suites_seen.add(m.group(1))
            continue
        m = SUITE_ANY.match(line)
        if m:
            suites_seen.add(m.group(1))
    return cases, suites_seen


def declared_suites(dirs):
    """Every XCTestCase subclass that declares at least one `test` method, by suite name.

    The method requirement is not decoration: a base class or a helper that subclasses XCTestCase
    without test methods is never reported by xcodebuild, so demanding it in the log would accuse
    the project file of a fault that is not there.
    """
    found = {}
    for d in dirs:
        for path in sorted(Path(d).rglob("*.swift")):
            src = path.read_text(errors="replace")
            for name in DECLARED_CLASS.findall(src):
                # The methods of one file's classes are not told apart here. A file holding two
                # classes where only one has test methods would be over-required; this repo has no
                # such file, and over-requiring fails loudly rather than silently.
                if DECLARED_METHOD.search(src):
                    found[name] = str(path)
    return found


def measure(log_text, tests_dirs):
    """Everything the report needs, with no I/O and no opinions about exit status."""
    cases, suites_seen = parse_log(log_text)
    declared = declared_suites(tests_dirs)
    unsound = []

    if not cases:
        if "** BUILD FAILED **" in log_text or "error:" in log_text:
            unsound.append(
                "no test case ran: the log carries build errors, so this run measured the build "
                "and not the suite"
            )
        else:
            unsound.append(
                "no test case ran and the log reports no build error either, so what this run "
                "measured is unknown"
            )

    missing = sorted(n for n in declared if n not in suites_seen)
    if cases and missing:
        unsound.append(
            f"{len(missing)} suite(s) the tree declares never appear in the log, so the run passed "
            "over tests that did not execute: " + ", ".join(missing)
        )

    return {
        "cases": cases,
        "passed": [c for c in cases if c[2] == "passed"],
        "failed": [c for c in cases if c[2] == "failed"],
        "skipped": [c for c in cases if c[2] == "skipped"],
        "suites_seen": sorted(suites_seen),
        "declared": declared,
        "missing": missing,
        "unsound": unsound,
    }


def names(cases):
    return [f"{suite}.{case}" for suite, case, _ in cases]


def render(m, ctx):
    """The one body that goes to the step summary and, when there is something to say, the issue."""
    out = []
    head = ctx["sha"][:8]
    if m["unsound"]:
        out.append(f"## Nightly macOS baseline on `{ctx['ref']}` at `{head}`: UNSOUND")
        out.append("")
        out.append(
            "This run measured something other than the suite, so it is evidence of nothing — "
            "neither a pass nor a failure. Read it before reading the job's colour."
        )
        for reason in m["unsound"]:
            out.append(f"- {reason}")
    elif m["failed"]:
        out.append(f"## Nightly macOS baseline on `{ctx['ref']}` at `{head}`: RED")
    else:
        out.append(f"## Nightly macOS baseline on `{ctx['ref']}` at `{head}`: GREEN")
    out.append("")
    out.append(
        f"{len(m['cases'])} case(s) ran across {len(m['suites_seen'])} suite(s): "
        f"{len(m['passed'])} passed, {len(m['failed'])} failed, {len(m['skipped'])} skipped. "
        f"The tree declares {len(m['declared'])} suite(s)."
    )
    if m["failed"]:
        out += ["", "### Failed, by name", ""] + [f"- `{n}`" for n in names(m["failed"])]
    if m["skipped"]:
        out += [
            "",
            "### Skipped, by name",
            "",
            "XCTSkip is not a pass. These cases asserted nothing on this run.",
            "",
        ] + [f"- `{n}`" for n in names(m["skipped"])]
    if m["missing"]:
        out += [
            "",
            "### Declared but never executed",
            "",
            "Each of these is an `XCTestCase` subclass with `test` methods that xcodebuild never "
            "named. That is a `project.pbxproj` registration fault, not a test failure, and the "
            "run's own conclusion cannot see it.",
            "",
        ] + [f"- `{n}` — `{m['declared'][n]}`" for n in m["missing"]]
    out += ["", f"Run: {ctx['run_url']}", f"Triggered by: `{ctx['event']}`"]
    return "\n".join(out) + "\n"


def post_issue(body, repo):
    """Comment on one rolling issue, creating it the first time.

    ONE issue, not one per night, and it is never closed automatically. What a lane needs at
    02:00 is not a notification, it is a lookup: has this suite been failing on main without me.
    That answer is a searchable thread of dated comments naming the cases. A fresh issue per red
    night buries it, and closing on a green night throws away the history on a suite with three
    known flake families — a green night after a red one is not evidence the flake is gone.
    """
    found = subprocess.run(
        ["gh", "issue", "list", "--repo", repo, "--state", "open", "--search",
         f'"{ISSUE_TITLE}" in:title', "--json", "number,title", "--limit", "20"],
        capture_output=True, text=True, check=True,
    )
    number = next(
        (i["number"] for i in json.loads(found.stdout or "[]") if i["title"] == ISSUE_TITLE), None
    )
    if number is None:
        subprocess.run(
            ["gh", "issue", "create", "--repo", repo, "--title", ISSUE_TITLE, "--body", body],
            check=True,
        )
        return "created"
    subprocess.run(
        ["gh", "issue", "comment", str(number), "--repo", repo, "--body", body], check=True
    )
    return f"commented on #{number}"


def cmd_report(args):
    ctx = {
        "sha": os.environ.get("GITHUB_SHA", "0" * 40),
        "ref": os.environ.get("GITHUB_REF_NAME", "?"),
        "event": os.environ.get("GITHUB_EVENT_NAME", "?"),
        "run_url": "{}/{}/actions/runs/{}".format(
            os.environ.get("GITHUB_SERVER_URL", "https://github.com"),
            os.environ.get("GITHUB_REPOSITORY", "?"),
            os.environ.get("GITHUB_RUN_ID", "?"),
        ),
    }
    log = Path(args.log)
    if not log.is_file():
        print(f"{log} is not there, so the run produced no log to read.", file=sys.stderr)
        return 2
    m = measure(log.read_text(errors="replace"), args.tests)
    body = render(m, ctx)
    print(body)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as fh:
            fh.write(body)

    worth_saying = bool(m["unsound"] or m["failed"])
    if worth_saying and args.post:
        try:
            print(f"issue: {post_issue(body, args.repo)}")
        except (subprocess.CalledProcessError, OSError) as exc:
            # The measurement is already in the step summary and the artifact, so losing the issue
            # loses reach, not evidence. Say so loudly and let the suite's own verdict stand.
            print(f"::error::could not reach the tracking issue: {exc}")
    elif worth_saying:
        print("=== DRY RUN: this run does not post. The body above is what it would post, to "
              f'the open issue titled "{ISSUE_TITLE}". ===')
    else:
        print("green and sound, so nothing to post.")

    if m["unsound"]:
        return 2
    return 1 if m["failed"] else 0


# --------------------------------------------------------------------------------------------
# The selftest, which exists because this repo has shipped a lint that passed by reading nothing.
# Every case below feeds the scanner a body it should fire on AND one it should not.
# --------------------------------------------------------------------------------------------

LEGACY_LINES = """\
Test Suite 'AlphaUITests' started at 2026-11-05 21:36:59.438.
Test Case '-[ArbosUITests.AlphaUITests testOne]' started.
Test Case '-[ArbosUITests.AlphaUITests testOne]' passed (1.000 seconds).
Test Case '-[ArbosUITests.AlphaUITests testTwo]' started.
Test Case '-[ArbosUITests.AlphaUITests testTwo]' failed (2.000 seconds).
Test Case '-[ArbosUITests.AlphaUITests testThree]' skipped (0.100 seconds).
"""
MODERN_LINES = """\
Test suite 'BetaTests' started on 'Clone 1 of iPhone 17 - Arbos (1)'
Test case 'BetaTests.testFour()' passed on 'Clone 1 of iPhone 17 - Arbos (1)' (0.010 seconds)
** TEST SUCCEEDED **
"""


def _sources(tmp, **files):
    """A fresh source tree per call. Sharing one would let an earlier case's suite leak into a
    later one, and every later case would then be testing a tree it did not describe."""
    d = Path(tempfile.mkdtemp(dir=tmp))
    for name, body in files.items():
        (d / f"{name}.swift").write_text(body)
    return [str(d)]


def _suite(name, methods=("testOne",)):
    body = "\n".join(f"    func {m}() {{}}" for m in methods)
    return f"import XCTest\nfinal class {name}: XCTestCase {{\n{body}\n}}\n"


def cmd_selftest(_args):
    fails = []

    def check(label, condition):
        print(f"  {'ok   ' if condition else 'BAD  '} {label}")
        if not condition:
            fails.append(label)

    tmp = tempfile.mkdtemp()
    try:
        log = LEGACY_LINES + MODERN_LINES

        # 1. Both log shapes are read, and all three verdicts are told apart. A parser that knows
        #    only the legacy form loses the whole unit bundle and still reports a cheerful total.
        m = measure(log, _sources(tmp, Alpha=_suite("AlphaUITests"), Beta=_suite("BetaTests")))
        check("both log shapes parse", len(m["cases"]) == 4)
        check("passed, failed and skipped are three answers, not two",
              (len(m["passed"]), len(m["failed"]), len(m["skipped"])) == (2, 1, 1))
        check("the failing case is named", names(m["failed"]) == ["AlphaUITests.testTwo"])
        check("the skipped case is named", names(m["skipped"]) == ["AlphaUITests.testThree"])
        check("a complete, green-enough run is sound", not m["unsound"])

        # 2. THE POSITIVE CONTROL for the registration scanner: a suite the tree declares and the
        #    log never names must be caught. This is the TranscriptSpacingUITests shape.
        m = measure(log, _sources(
            tmp, Alpha=_suite("AlphaUITests"), Beta=_suite("BetaTests"),
            Gamma=_suite("GammaUITests"),
        ))
        check("a declared suite missing from the log is caught", m["missing"] == ["GammaUITests"])
        check("and it makes the measurement unsound rather than red", bool(m["unsound"]))

        # 3. The scanner must not fire on things xcodebuild never reports anyway, or every nightly
        #    is unsound and the signal is worth nothing.
        m = measure(log, _sources(
            tmp, Alpha=_suite("AlphaUITests"), Beta=_suite("BetaTests"),
            Helper="import XCTest\nclass RigBase: XCTestCase {\n    func helper() {}\n}\n",
        ))
        check("an XCTestCase with no test methods is not required to appear", not m["missing"])

        # 4. A suite seen only through its `Test Suite ... started` line still counts as executed.
        #    xcodebuild sometimes interleaves its own chatter through a case line and eats it;
        #    accusing the project file over that would be a false alarm against a real file.
        m = measure("Test Suite 'AlphaUITests' started at x\n" + MODERN_LINES,
                    _sources(tmp, Alpha=_suite("AlphaUITests"), Beta=_suite("BetaTests")))
        check("a suite witnessed only by its suite line is not called missing",
              "AlphaUITests" not in m["missing"])

        # 5. An empty log is unsound, never green. This is the shape a silent registration fault
        #    across the whole bundle would take.
        m = measure("", _sources(tmp, Alpha=_suite("AlphaUITests")))
        check("an empty log is unsound, not green", bool(m["unsound"]) and not m["cases"])
        m = measure("/x.swift:1: error: no\n** BUILD FAILED **\n", _sources(tmp, Alpha=_suite("A")))
        check("a failed build says so rather than blaming the project file",
              any("build" in u for u in m["unsound"]))

        # 6. The rendered body must carry the names, because a report that only carries counts
        #    cannot answer the question a lane actually has at 02:00.
        m = measure(log, _sources(tmp, Alpha=_suite("AlphaUITests"), Beta=_suite("BetaTests")))
        body = render(m, {"sha": "abcdef1234", "ref": "main", "event": "schedule",
                          "run_url": "http://x/1"})
        check("the body names the failing case", "AlphaUITests.testTwo" in body)
        check("the body names the skipped case", "AlphaUITests.testThree" in body)
        check("the body says RED when a case failed", ": RED" in body)
        green = measure(MODERN_LINES, _sources(tmp, Beta=_suite("BetaTests")))
        check("the body says GREEN when nothing failed",
              ": GREEN" in render(green, {"sha": "abcdef1234", "ref": "main",
                                          "event": "schedule", "run_url": "http://x/1"}))

        # 7. Exit status must tell the three answers apart. A caller that cannot separate "red"
        #    from "measured nothing" will eventually read one as the other.
        check("unsound exits 2, red exits 1, green exits 0",
              _exit_for(measure("", _sources(tmp, A=_suite("A")))) == 2
              and _exit_for(measure(log, _sources(tmp, Alpha=_suite("AlphaUITests"),
                                                  Beta=_suite("BetaTests")))) == 1
              and _exit_for(green) == 0)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    print()
    print(f"macos-baseline-report selftest: {'PASS' if not fails else 'FAIL'}")
    return 1 if fails else 0


def _exit_for(m):
    if m["unsound"]:
        return 2
    return 1 if m["failed"] else 0


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("report")
    r.add_argument("--log", required=True)
    r.add_argument("--tests", nargs="+", required=True)
    r.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", ""))
    r.add_argument("--post", action="store_true",
                   help="comment on the tracking issue; without it the body is printed only")
    r.set_defaults(func=cmd_report)
    s = sub.add_parser("selftest")
    s.set_defaults(func=cmd_selftest)
    args = ap.parse_args()
    sys.exit(args.func(args))


if __name__ == "__main__":
    main()
