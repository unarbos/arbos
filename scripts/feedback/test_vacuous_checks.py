#!/usr/bin/env python3
"""Three poller checks that some input could satisfy while measuring nothing, each pinned to the producer it
reads so the producer cannot move without something failing:

  * `note_main_tests_broken` reads the wording `ios-gate.sh` prints today (AF-645: it read a wording the gate
    stopped printing on 2026-10-25 and raised nothing for eight days), and says so out loud when the gate
    reports a FAIL-to-compile in words it does not read.
  * `SHELLCHECK_GLOBS` covers every glob on CI's `shellcheck` line — a path CI lints and this list does not
    reach is a path a scripts-only branch merges unlinted. This one holds on main too: it is the alarm for the
    next time CI's line grows, not a repair.
  * `shellcheck_ok` treats a CI-linted file it could not read as unchecked — not as clean (a `/contents` 403 on
    every file left no paths to lint and `not paths` read as a pass) and not as the branch's fault.

The producers are read from the repo (`.github/workflows/ci.yml`, `scripts/feedback/ios-gate.sh`); state lives
in a temp dir; no network, no App Store Connect, no GitHub.

    python3 -m unittest discover -s scripts/feedback -p 'test_vacuous_checks.py'
"""
import base64
import contextlib
import io
import os
import re
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import feedback_poller as fp  # noqa: E402 - the poller lives beside this test

REPO = Path(__file__).resolve().parents[2]
CI_YML = REPO / ".github/workflows/ci.yml"
IOS_GATE = REPO / "scripts/feedback/ios-gate.sh"
ENV = {"ASC_KEY_ID": "key", "ASC_ISSUER_ID": "issuer", "ASC_KEY_P8_B64": "eA==",
       "CURSOR_API_KEY": "cursor", "GITHUB_TOKEN": "gh"}


def ci_shellcheck_globs() -> list[str]:
    """The paths CI's Layout job actually runs shellcheck over — the authority SHELLCHECK_GLOBS copies."""
    m = re.search(r"^\s*shellcheck -S warning (.+)$", CI_YML.read_text(), re.M)
    assert m, "ci.yml no longer runs `shellcheck -S warning <paths>`; SHELLCHECK_GLOBS has no authority to copy"
    return m.group(1).split()


class PollerTest(unittest.TestCase):
    def setUp(self) -> None:
        state = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, state, True)
        env = mock.patch.dict(os.environ, {**ENV, "FEEDBACK_STATE_DIR": str(state)})
        env.start()
        self.addCleanup(env.stop)
        self.loop = fp.Loop(fp.Config.from_env())


class MainTestsBrokenTest(PollerTest):
    """The gate's own words, read out of ios-gate.sh rather than retyped here."""

    def gate_line(self, needle: str) -> str:
        """The `== tests:` line ios-gate.sh prints today, with its shell expansions filled in."""
        lines = [l for l in IOS_GATE.read_text().splitlines() if needle in l and "echo " in l]
        self.assertEqual(len(lines), 1, f"{needle!r} is not one echo in ios-gate.sh: {lines}")
        return lines[0].split("echo ", 1)[1].strip().strip('"').replace("${GATE_BASE_SHA:0:8}", "4a9de3cc")

    def body(self, *gate_lines: str) -> str:
        return "\n".join([*gate_lines,
                          "apps/ios/Arbos/ArbosTests/MentionDraftTests.swift:31:9: error: cannot find 'draft' in scope"])

    def capture(self, body: str) -> str:
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.loop.note_main_tests_broken({"id": "g-1"}, body)
        return out.getvalue()

    def test_the_wording_the_gate_prints_today_raises_the_alert(self):
        out = self.capture(self.body(self.gate_line("FAIL to compile"),
                                     self.gate_line("every error exists on main too")))
        self.assertIn("ALERT main's iOS unit-test target does not compile at 4a9de3cc", out)
        self.assertEqual(self.loop.store.cursor()["main_ios_tests_broken"]["main"], "4a9de3cc")

    def test_the_wording_the_gate_printed_before_755fd2ee9_still_raises_it(self):
        out = self.capture(self.body(
            self.gate_line("FAIL to compile"),
            "== tests: the same failure exists on main (4a9de3cc); not this branch's regression — reported, not blamed"))
        self.assertIn("ALERT main's iOS unit-test target does not compile at 4a9de3cc", out)

    def test_a_reworded_verdict_is_announced_instead_of_passing_in_silence(self):
        """What AF-645 did for eight days: the report arrives, no pattern matches, nothing at all is raised."""
        out = self.capture(self.body(
            self.gate_line("FAIL to compile"),
            "== tests: main has all of these errors as well (4a9de3cc) — reported, not blamed"))
        self.assertIn("ios-gate.sh's wording has changed", out)
        self.assertNotIn("main_ios_tests_broken", self.loop.store.cursor())

    def test_a_regression_the_gate_blames_on_the_branch_raises_nothing(self):
        out = self.capture(self.body(self.gate_line("FAIL to compile"),
                                     self.gate_line("errors NOT on main")))
        self.assertEqual(out, "")

    def test_a_compiling_test_target_clears_the_record(self):
        cur = self.loop.store.cursor()
        cur["main_ios_tests_broken"] = {"main": "4a9de3cc", "since": "2026-11-01T00:00:00Z", "run": "g-0", "errors": []}
        self.loop.store.save_cursor(cur)
        out = self.capture("== tests: compile\n== tests: Executed 508 tests pass (211 s)")
        self.assertIn("compiles again", out)
        self.assertNotIn("main_ios_tests_broken", self.loop.store.cursor())


class ShellcheckGlobsTest(PollerTest):
    """CI's Layout job is the authority on what gets linted; SHELLCHECK_GLOBS is a copy of its argument list,
    and a scripts-only branch is merged without the Rust gate, so a path CI lints and this does not reach is a
    shell warning landing on main unannounced. Covered today, including scripts/merge/*.sh, only because
    `fnmatch` lets `*` cross `/` — nothing in the list says so, and the next path CI adds may be elsewhere."""

    @unittest.skipUnless(shutil.which("shellcheck"), "shellcheck is not installed here")
    def test_every_path_ci_lints_is_one_the_gate_fetches_and_lints(self):
        for glob in ci_shellcheck_globs():
            touched = glob.replace("*", "some-script")
            with mock.patch.object(self.loop.gh, "repo_api", side_effect=AssertionError("fetched")):
                with self.assertRaises(AssertionError, msg=f"CI lints {touched}; the gate merges it with no lint"):
                    self.loop.shellcheck_ok("x", {"files": [touched], "branch": "feedback/x"}, "0" * 40)


@unittest.skipUnless(shutil.which("shellcheck"), "shellcheck is not installed here")
class ShellcheckUnreadableTest(PollerTest):
    """A file the loop could not read is not a file the loop checked."""

    FILES = ["scripts/feedback/ios-gate.sh", "scripts/merge/merge.sh"]

    def shellcheck(self, repo_api) -> tuple[bool, str]:
        st = {"files": self.FILES, "branch": "feedback/x", "issue": 1}
        out = io.StringIO()
        with contextlib.redirect_stdout(out), mock.patch.object(self.loop.gh, "repo_api", side_effect=repo_api):
            ok = self.loop.shellcheck_ok("x", st, "0" * 40)
        return ok, out.getvalue()

    @staticmethod
    def blob(text: str) -> dict:
        return {"content": base64.b64encode(text.encode()).decode(), "encoding": "base64", "size": len(text)}

    def test_a_clean_branch_passes(self):
        ok, _ = self.shellcheck(lambda *a, **k: self.blob('#!/bin/sh\nset -eu\necho "hi"\n'))
        self.assertTrue(ok)

    def test_a_shell_warning_fails(self):
        with mock.patch.object(self.loop, "gate_followup") as followup:
            ok, out = self.shellcheck(lambda *a, **k: self.blob('#!/bin/sh\nrm -rf "$DIR/"\n'))
        self.assertFalse(ok)
        self.assertIn("shellcheck red", out)
        followup.assert_called_once()

    def test_an_unfetchable_file_is_not_a_clean_lint(self):
        def refuse(_method, path, **_kw):
            raise fp.HttpError(403, '{"message":"API rate limit exceeded"}', path, {})
        ok, out = self.shellcheck(refuse)
        self.assertFalse(ok)
        self.assertIn("is not merged unchecked", out)

    def test_a_blob_with_no_content_is_not_blamed_on_the_branch(self):
        """`/contents` answers a blob over 1 MB with encoding "none" and no body. The empty file that used to
        leave reds with SC2148 (no shebang), so the owner was sent a followup about a file nobody read."""
        with mock.patch.object(self.loop, "gate_followup") as followup:
            ok, out = self.shellcheck(lambda *a, **k: {"content": "", "encoding": "none", "size": 1_400_000})
        self.assertFalse(ok)
        self.assertIn("came back without content", out)
        self.assertNotIn("shellcheck red", out)
        followup.assert_not_called()

    def test_a_file_the_branch_deletes_is_not_held(self):
        """A 404 at the branch head means the branch removed the file: there is nothing to lint, so holding the
        item in gating forever would be wrong. Only a readable-but-unread file is held."""
        def gone(_method, path, **_kw):
            raise fp.HttpError(404, '{"message":"Not Found"}', path, {})
        ok, out = self.shellcheck(gone)
        self.assertTrue(ok)
        self.assertNotIn("not merged unchecked", out)


if __name__ == "__main__":
    unittest.main()
