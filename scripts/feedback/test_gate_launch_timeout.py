#!/usr/bin/env python3
"""A launch that timed out is no verdict: not the branch's failure, and not a pass either.

Drives the real functions out of `ios-gate.sh` (sliced from the file, never retyped), the real
`rust-gate.sh status` transport filter, and the real `Loop.note_ios_no_verdict` from the poller, over
synthesised xcodebuild logs in the wordings this project has measured on its own runs.

    python3 scripts/feedback/test_gate_launch_timeout.py
"""

import importlib.util
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
IOS_GATE = HERE / "ios-gate.sh"
RUST_GATE = HERE / "rust-gate.sh"

# The cold-start wording (sighting 4, run 41239073951: 80.4 s, no gesture sent).
LAUNCH_PROGRESS_LOG = """\
Test Suite 'All tests' started at 2026-10-31 22:04:11.221
    t =     1.44s Launch com.unarbos.arbos.ios
2026-10-31 22:05:31.638 xcodebuild[2411:9912] Failed to get launch progress for com.unarbos.arbos.ios: \
Error Domain=IDELaunchErrorDomain Code=11 "Timed out while requesting launch progress"
Testing failed:
	Arbos.app encountered an error (Failed to get launch progress)
** TEST EXECUTE FAILED **
"""

# Xcode 27 / iOS 27 wording (sighting 3, run 41213344893).
XCODE_LAUNCH_LOG = """\
Test Suite 'All tests' started at 2026-10-31 15:20:54.306
Failed to launch <XCUIApplicationImpl: 0x600003a1c000 com.unarbos.arbos.ios> via Xcode: \
Timed out while launching application via Xcode.
** TEST EXECUTE FAILED **
"""

# iOS 26.2 wording (sighting 1, run 41193500492 attempt 1).
BACKGROUND_ASSERTION_LOG = """\
Test Suite 'All tests' started at 2026-10-31 10:55:43.009
Failed to get background assertion for target app with pid 4471: \
Timed out while acquiring background assertion
** TEST EXECUTE FAILED **
"""

GREEN_LOG = """\
Test Suite 'All tests' started at 2026-11-01 02:10:00.000
Test Case '-[ArbosTests.AgentIdentityTests testSilencedDefaultsOff]' passed (0.002 seconds).
Executed 510 tests, with 0 failures (0 unexpected) in 4.113 (4.221) seconds
Test Suite 'All tests' passed at 2026-11-01 02:11:04.000
** TEST SUCCEEDED **
"""

TEST_FAILURE_LOG = """\
Test Suite 'All tests' started at 2026-11-01 02:10:00.000
/Users/ec2-user/arbos-gate/apps/ios/Arbos/ArbosTests/MentionDraftTests.swift:88: \
error: -[ArbosTests.MentionDraftTests testAChipKeepsItsTrailingSpace] : XCTAssertEqual failed
Test Case '-[ArbosTests.MentionDraftTests testAChipKeepsItsTrailingSpace]' failed (0.114 seconds).
Executed 510 tests, with 1 failure (0 unexpected) in 4.503 (4.610) seconds
Test Suite 'All tests' failed at 2026-11-01 02:11:04.000
** TEST FAILED **
"""

# A launch that timed out while a test case of its own also failed: the suite did run, so the failure
# still belongs to whoever owns it. This must not be reclassified.
MIXED_LOG = TEST_FAILURE_LOG + "Failed to get launch progress for com.unarbos.arbos.ios\n"


def slice_shell(path: Path, names) -> str:
    """The named top-level functions and `NAME=` assignments, lifted verbatim out of a shell script."""
    text = path.read_text()
    out = []
    for name in names:
        m = re.search(rf"^{re.escape(name)}\(\) \{{\n.*?^\}}$", text, re.M | re.S)
        if m is None:
            m = re.search(rf"^{re.escape(name)}=.*$", text, re.M)
        if m is None:
            raise AssertionError(f"{name} not found in {path}")
        out.append(m.group(0))
    return "\n".join(out)


GATE_FUNCS = slice_shell(IOS_GATE, ["LAUNCH_TIMEOUT_RE", "failing_tests", "launch_timed_out",
                                    "launch_timeout_lines", "run_unit_tests"])

# What `rust-gate.sh status` forwards to the poller, taken from the file so it cannot drift.
STATUS_FILTER = re.search(r"grep -E \"\^== \".*?\n.*?grep -E \"\^\(error\|warning\).*?head -n 150",
                          RUST_GATE.read_text(), re.S).group(0)


def run_gate(log_text: str, baseline: str = "", disable_fix: bool = False):
    """(exit code, what reaches the poller) of the real run_unit_tests over `log_text`."""
    with tempfile.TemporaryDirectory(prefix="gate-launch-") as tmp:
        Path(tmp, "suite.log").write_text(log_text)
        script = f"""
set -uo pipefail
ROOT={tmp}
GATE_BASE_SHA=0123456789abcdef0123456789abcdef01234567
{GATE_FUNCS}
# BSD `mktemp -t <prefix>` on the gate Mac; GNU mktemp wants the X's.
mktemp() {{ command mktemp "{tmp}/$2.XXXXXX"; }}
run_suite() {{ cp {tmp}/suite.log "$2"; grep -q 'TEST SUCCEEDED' "$2" && return 0; return 65; }}
baseline_for() {{ printf '%s' {baseline!r}; }}
{'launch_timed_out() { return 1; }' if disable_fix else ''}
run_unit_tests
"""
        r = subprocess.run(["bash", "-c", script], capture_output=True, text=True)
        forwarded = subprocess.run(["bash", "-c", f"log=/dev/stdin\nrepo={tmp}\n{STATUS_FILTER}"],
                                   input=r.stdout, capture_output=True, text=True)
        return r.returncode, forwarded.stdout


def load_poller():
    spec = importlib.util.spec_from_file_location("feedback_poller", HERE / "feedback_poller.py")
    mod = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = mod  # @dataclass resolves its annotations through sys.modules
    spec.loader.exec_module(mod)
    return mod


class LaunchTimeoutIsNoVerdict(unittest.TestCase):
    """Exit 2 is the gate Mac's infrastructure code: `rust-gate.sh status` reports it as `infra`, which the
    poller re-runs without a verdict. 0 would merge the branch and 1 would blame its author."""

    def test_every_measured_wording_is_no_verdict(self):
        for name, log in (("launch progress", LAUNCH_PROGRESS_LOG),
                          ("via Xcode", XCODE_LAUNCH_LOG),
                          ("background assertion", BACKGROUND_ASSERTION_LOG)):
            with self.subTest(name):
                rc, out = run_gate(log)
                self.assertEqual(rc, 2, out)
                self.assertIn("NO VERDICT", out)
                self.assertIn("did not launch the app", out)
                self.assertIn("neither a pass nor a failure of the branch", out)

    def test_the_reason_survives_the_transport(self):
        _rc, out = run_gate(LAUNCH_PROGRESS_LOG)
        self.assertIn("Timed out while requesting launch progress", out)

    def test_the_defect_without_the_fix(self):
        """Control: with the classifier disabled the same log exits 0 — the gate reads it as a pass."""
        rc, out = run_gate(LAUNCH_PROGRESS_LOG, disable_fix=True)
        self.assertEqual(rc, 0, out)
        self.assertNotIn("NO VERDICT", out)


class RealResultsAreUntouched(unittest.TestCase):
    def test_a_passing_suite_still_passes(self):
        rc, out = run_gate(GREEN_LOG)
        self.assertEqual(rc, 0, out)
        self.assertIn("Executed 510 tests pass", out)

    def test_a_failing_test_is_still_the_branch_s(self):
        rc, out = run_gate(TEST_FAILURE_LOG)
        self.assertEqual(rc, 1, out)
        self.assertIn("FAIL — failing and NOT red on main", out)

    def test_a_launch_line_beside_a_real_failure_is_still_a_failure(self):
        rc, out = run_gate(MIXED_LOG)
        self.assertEqual(rc, 1, out)
        self.assertNotIn("NO VERDICT", out)

    def test_a_failure_red_on_main_is_still_not_the_branch_s(self):
        rc, out = run_gate(TEST_FAILURE_LOG, baseline="MentionDraftTests/testAChipKeepsItsTrailingSpace")
        self.assertEqual(rc, 0, out)
        self.assertIn("red on main", out)


class BaselineThatNeverRan(unittest.TestCase):
    """A launch that timed out while measuring MAIN is the blame path: an empty failing set cached for that
    head turns every test red on main into the next branch's regression."""

    def test_an_unmeasurable_baseline_blames_nobody_and_passes_nothing(self):
        rc, out = run_gate(TEST_FAILURE_LOG, baseline="__noverdict__")
        self.assertEqual(rc, 2, out)
        self.assertIn("could not be measured", out)
        self.assertIn("the branch is not blamed", out)

    def test_the_defect_an_empty_baseline_blames_the_branch(self):
        """Control: the same red with the empty set the old code would have cached."""
        rc, out = run_gate(TEST_FAILURE_LOG, baseline="")
        self.assertEqual(rc, 1, out)
        self.assertIn("NOT red on main", out)

    def test_baseline_for_caches_nothing_when_main_never_launched(self):
        """The real `baseline_for`, over a stub git and xcodebuild, must not write a file for that head."""
        with tempfile.TemporaryDirectory(prefix="gate-baseline-") as tmp:
            bin_dir = Path(tmp, "bin")
            bin_dir.mkdir()
            for tool in ("git", "xcodebuild"):
                p = bin_dir / tool
                p.write_text("#!/bin/bash\n[[ \"$*\" == *rev-parse* ]] && echo deadbeef\nexit 0\n")
                p.chmod(0o755)
            Path(tmp, "apps/ios/Arbos").mkdir(parents=True)
            Path(tmp, "suite.log").write_text(LAUNCH_PROGRESS_LOG)
            script = f"""
set -uo pipefail
export PATH={bin_dir}:$PATH
ROOT={tmp}
BASELINE_DIR={tmp}/.test-baseline
{slice_shell(IOS_GATE, ["LAUNCH_TIMEOUT_RE", "failing_tests", "launch_timed_out", "baseline_for"])}
mktemp() {{ command mktemp "{tmp}/$2.XXXXXX"; }}
run_suite() {{ cp {tmp}/suite.log "$2"; return 65; }}
baseline_for feedcafefeedcafefeedcafefeedcafefeedcafe
"""
            r = subprocess.run(["bash", "-c", script], capture_output=True, text=True)
            self.assertEqual(r.stdout.strip(), "__noverdict__", r.stderr)
            self.assertFalse(Path(tmp, ".test-baseline/feedcafefeedcafefeedcafefeedcafefeedcafe").exists(),
                             "a run that tested nothing was cached as main's failing set")


class TheItemIsTold(unittest.TestCase):
    def setUp(self):
        self.mod = load_poller()

    def _run(self, body, state=None):
        posted, events, saved = [], [], {}
        loop = type("Stub", (), {})()
        loop.IOS_NO_VERDICT_MARK = self.mod.Loop.IOS_NO_VERDICT_MARK
        loop.store = type("S", (), {
            "state": lambda _s, fid: dict(state if state is not None else {"issue": 42, "branch": "feedback/9"}),
            "save_state": lambda _s, fid, st: saved.update(st),
            "event": lambda _s, fid, kind, **kw: events.append(kind),
        })()
        loop.gh = type("G", (), {"comment": lambda _s, n, b: posted.append((n, b))})()
        self.mod.Loop.note_ios_no_verdict(loop, {"id": "r1", "members": {"9": "a" * 40}}, body)
        return posted, events, saved

    def test_the_comment_says_nothing_ran_and_asks_for_no_commit(self):
        posted, events, saved = self._run("== tests: NO VERDICT — the simulator did not launch the app, so no test case ran (rc=2).")
        self.assertEqual(len(posted), 1)
        number, body = posted[0]
        self.assertEqual(number, 42)
        self.assertIn("no verdict", body)
        self.assertIn("not one test case ran", body)
        self.assertIn("not a pass", body)
        self.assertIn("nothing is merged", body)
        self.assertIn("no new commit", body)
        self.assertEqual(events, ["ios_no_verdict"])
        self.assertEqual(saved.get("ios_no_verdict_noted"), "a" * 40)

    def test_it_is_said_once_per_head(self):
        posted, _e, _s = self._run("== tests: NO VERDICT — nope", state={"issue": 42, "branch": "feedback/9",
                                                                         "ios_no_verdict_noted": "a" * 40})
        self.assertEqual(posted, [])

    def test_other_infrastructure_errors_are_not_claimed(self):
        posted, events, _s = self._run("fetch of feedback/9 failed")
        self.assertEqual((posted, events), ([], []))

    def test_no_retry_is_counted_and_no_agent_is_woken(self):
        _p, _e, saved = self._run("== tests: NO VERDICT — the simulator did not launch the app")
        self.assertNotIn("gate_retries", saved)
        self.assertNotIn("status", saved)


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
