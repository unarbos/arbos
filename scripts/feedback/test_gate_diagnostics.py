"""A red gate must reach its owner with the real error lines, or with an explicit "could not retrieve" — never
an empty block. Run: python3 -m unittest discover -s scripts/feedback -p 'test_*.py'"""

import http.server
import os
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import feedback_poller as fp  # noqa: E402

E, B, R, BL, G, Y, C = "\x1b[0m", "\x1b[1m", "\x1b[91m", "\x1b[94m", "\x1b[92m", "\x1b[33m", "\x1b[36;1m"
TS = "2026-10-28T21:00:21.0175340Z "

# GitHub run 40767863812 (#2232's Linux half), as the job log has it: cargo's colours, the runner's timestamps.
CLIPPY_2232 = "\n".join(TS + l for l in [
    "##[group]Run cargo clippy --workspace --all-targets --locked -- -D warnings",
    f"{C}cargo clippy --workspace --all-targets --locked -- -D warnings{E}",
    f"{C}curl --proto '=https' --tlsv1.2 --retry 10 --location --silent --show-error --fail https://sh.rustup.rs{E}",
    "##[endgroup]",
    f"{B}{G}   Compiling{E} arbos-mac v0.6.338 (/home/runner/work/arbos/arbos/crates/arbos-mac)",
    f"{B}{R}error[E0004]{E}{B}: non-exhaustive patterns: `sync::ClientCommand::OutsideAgentLog {{ .. }}` not covered{E}",
    f"    {B}{BL}--> {E}crates/arbos-mac/src/sync/fixture.rs:1120:11",
    f"     {B}{BL}|{E}",
    f"{B}{BL}1120{E} {B}{BL}|{E}     match cmd {{",
    f"{B}{G}note{E}: `sync::ClientCommand` defined here",
    f"    {B}{BL}--> {E}crates/arbos-mac/src/sync/mod.rs:607:10",
    "",
    f"{B}{R}error[E0004]{E}{B}: non-exhaustive patterns: `Some(&app::ProfilePage::AgentLog {{ .. }})` not covered{E}",
    f"  {B}{BL}--> {E}crates/arbos-mac/src/ui/header.rs:57:27",
    f"{B}For more information about this error, try `rustc --explain E0004`.{E}",
    f"{B}{R}error{E}: could not compile `arbos-mac` (bin \"arbos-mac\") due to 2 previous errors",
    f"{B}{Y}warning{E}: build failed, waiting for other jobs to finish...",
    "##[error]Process completed with exit code 101.",
])

# Run 40338870911 (#1890's Linux half): rustfmt's diff.
FMT_1890 = "\n".join(TS + l for l in [
    "Diff in /home/runner/work/arbos/arbos/crates/arbos-relay/src/api.rs:82:",
    "     let low = disk_low();",
    '-    let path = std::env::var("ARBOS_DISK_WATCH_PATH").unwrap_or_else(|_| "/var/lib/arbos-relay".to_owned());',
    '+    let path = std::env::var("ARBOS_DISK_WATCH_PATH")',
    '+        .unwrap_or_else(|_| "/var/lib/arbos-relay".to_owned());',
    "##[error]Process completed with exit code 1.",
])

SHELLCHECK = "\n".join(TS + l for l in [
    "In scripts/evals/long-tasks/run-suite.sh line 32:",
    '  export ARBOS_RELAY_ADMIN_TOKEN="$(op item get x --fields credential --reveal)"',
    "         ^---------------------^ SC2155 (warning): Declare and assign separately to avoid masking return values.",
    "##[error]Process completed with exit code 1.",
])

# Run 41081417967: a failed cargo test with its panic.
TEST_FAIL = "\n".join(TS + l for l in [
    "test ui::picture_fit::tests::non_square_pictures_fit_their_shorter_side ... FAILED",
    "failures:",
    "---- ui::picture_fit::tests::non_square_pictures_fit_their_shorter_side stdout ----",
    "thread 'ui::picture_fit::tests::non_square_pictures_fit_their_shorter_side' (6826) panicked at crates/arbos-mac/src/ui/picture_fit.rs:208:9:",
    "assertion `left == right` failed",
    "  left: (80, 40)",
    " right: (160, 80)",
    "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace",
    "error: test failed, to rerun pass `-p arbos-mac --bin arbos-mac`",
    "##[error]Process completed with exit code 101.",
])

NOTHING_MATCHES = "\n".join(TS + l for l in [
    "##[group]Run scripts/check-typeshare-fresh.sh",
    "generated types are stale:",
    "  apps/ios/Arbos/Generated/Types.swift",
    "run scripts/typeshare.sh and commit the result",
    "##[error]Process completed with exit code 1.",
    "Post job cleanup.",
])


class LogErrorLinesTest(unittest.TestCase):
    def test_coloured_rustc_errors_come_with_their_location(self):
        lines = fp.log_error_lines(CLIPPY_2232)
        self.assertEqual(lines[:4], [
            "error[E0004]: non-exhaustive patterns: `sync::ClientCommand::OutsideAgentLog { .. }` not covered",
            "    --> crates/arbos-mac/src/sync/fixture.rs:1120:11",
            "error[E0004]: non-exhaustive patterns: `Some(&app::ProfilePage::AgentLog { .. })` not covered",
            "  --> crates/arbos-mac/src/ui/header.rs:57:27",
        ])

    def test_the_old_word_match_misses_the_coloured_error(self):
        coloured = f"{B}{R}error[E0004]{E}{B}: non-exhaustive patterns"
        self.assertIsNone(fp.re.search(r"\berror\b", coloured))
        self.assertIn("error[E0004]: non-exhaustive patterns", fp.log_error_lines(TS + coloured))

    def test_echoed_commands_and_rustc_boilerplate_are_not_errors(self):
        lines = fp.log_error_lines(CLIPPY_2232)
        self.assertFalse([l for l in lines if "show-error" in l or "For more information" in l
                          or "Process completed" in l or l.startswith("warning")], lines)

    def test_fmt_diff_with_repo_relative_path(self):
        lines = fp.log_error_lines(FMT_1890)
        self.assertEqual(lines[0], "Diff in crates/arbos-relay/src/api.rs:82:")
        self.assertIn('+    let path = std::env::var("ARBOS_DISK_WATCH_PATH")', lines)

    def test_shellcheck_finding(self):
        lines = fp.log_error_lines(SHELLCHECK)
        self.assertEqual(lines[0], "In scripts/evals/long-tasks/run-suite.sh line 32:")
        self.assertTrue(any("SC2155" in l for l in lines))

    def test_failed_test_with_its_panic_and_assertion(self):
        lines = fp.log_error_lines(TEST_FAIL)
        self.assertIn("test ui::picture_fit::tests::non_square_pictures_fit_their_shorter_side ... FAILED", lines)
        self.assertTrue(any("panicked at crates/arbos-mac/src/ui/picture_fit.rs:208:9" in l for l in lines))
        self.assertIn("  left: (80, 40)", lines)

    def test_no_pattern_gives_the_failed_steps_last_lines(self):
        lines = fp.log_error_lines(NOTHING_MATCHES)
        self.assertEqual(lines[0], "(no error pattern matched; the failed step's last lines:)")
        self.assertIn("generated types are stale:", lines)
        self.assertNotIn("Post job cleanup.", lines)

    def test_empty_log_says_so(self):
        self.assertEqual(fp.log_error_lines(""), ["(the job's log has no output before the failure; open the run)"])


class _Blob(http.server.BaseHTTPRequestHandler):
    """Stands in for GitHub's API (302 to the log) and the blob store behind it (refuses any Authorization)."""
    log_text = CLIPPY_2232

    def do_GET(self):
        if self.path.startswith("/repos/"):
            self.send_response(302)
            self.send_header("Location", f"http://127.0.0.1:{self.server.server_port}/blob/log.txt?sig=x")
            self.end_headers()
        elif self.headers.get("Authorization"):
            body = b"<Error><Code>InvalidAuthenticationInfo</Code></Error>"
            self.send_response(401)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            body = self.log_text.encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    def log_message(self, *args):
        pass


class JobLogTest(unittest.TestCase):
    def setUp(self):
        self.server = http.server.HTTPServer(("127.0.0.1", 0), _Blob)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        api = f"http://127.0.0.1:{self.server.server_port}"
        patcher = mock.patch.object(fp, "GITHUB_API", api)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.gh = fp.GitHub.__new__(fp.GitHub)
        self.gh.repo = "unarbos/arbos"
        self.gh.h = {"Authorization": "Bearer t", "Accept": "application/vnd.github+json"}
        self.url = f"{api}/repos/unarbos/arbos/actions/jobs/1/logs"

    def test_the_old_fetch_carries_the_token_into_the_blob_store_and_is_refused(self):
        with self.assertRaises(fp.HttpError) as caught:
            fp.http("GET", self.url, headers=self.gh.h)
        self.assertEqual(caught.exception.status, 401)

    def test_job_log_follows_the_redirect_without_the_token(self):
        self.assertEqual(self.gh.job_log(1), CLIPPY_2232)


JOB_2232 = {"id": 123096780164, "name": "rust (fmt, clippy, test)", "conclusion": "failure",
            "html_url": "https://github.com/unarbos/arbos/actions/runs/40767863812/job/123096780164",
            "steps": [{"name": "Run cargo clippy --workspace --all-targets --locked -- -D warnings", "conclusion": "failure"}]}


def fake_gh(jobs, log=None, error=None):
    gh = fp.GitHub.__new__(fp.GitHub)
    gh.repo_api = lambda method, path, **kw: {"jobs": jobs}
    gh.job_log = mock.Mock(return_value=log, side_effect=error)
    return gh


class RunErrorLinesTest(unittest.TestCase):

    def test_2232_shape_carries_both_errors(self):
        lines = fake_gh([{"name": "Layout", "conclusion": "success"}, JOB_2232], CLIPPY_2232).run_error_lines(1, limit=60)
        self.assertEqual(lines[0], "step failed: Run cargo clippy --workspace --all-targets --locked -- -D warnings")
        self.assertEqual(sum(l.startswith("error[E0004]") for l in lines), 2)
        self.assertIn("  --> crates/arbos-mac/src/ui/header.rs:57:27", lines)

    def test_an_unreadable_log_says_so(self):
        err = fp.HttpError(401, "InvalidAuthenticationInfo", "https://blob")
        lines = fake_gh([JOB_2232], error=err).run_error_lines(1)
        self.assertTrue(lines)
        self.assertTrue(any(l.startswith("(could not retrieve the log of job `rust (fmt, clippy, test)`") and "401" in l
                            and JOB_2232["html_url"] in l for l in lines), lines)

    def test_a_network_failure_says_so(self):
        lines = fake_gh([JOB_2232], error=TimeoutError("timed out")).run_error_lines(1)
        self.assertTrue(any("could not retrieve" in l and "timed out" in l for l in lines), lines)

    def test_a_run_without_a_failed_job_says_so(self):
        lines = fake_gh([{"name": "rust", "conclusion": "cancelled"}]).run_error_lines(1)
        self.assertEqual(lines, ["(no job of the run failed — job conclusions: cancelled; open the run)"])


class GateDiagnosticsTest(unittest.TestCase):
    RUN = "https://github.com/unarbos/arbos/actions/runs/40767863812"

    def test_linux_errors_are_shown(self):
        out = fp.gate_diagnostics({"linux": "fail", "mac": "skipped", "linux_run": self.RUN,
                                   "linux_errors": ["error[E0004]: non-exhaustive patterns", "  --> crates/x.rs:57:27"]})
        self.assertIn("error[E0004]: non-exhaustive patterns\n  --> crates/x.rs:57:27", out)
        self.assertNotIn("could be retrieved", out)

    def test_an_empty_linux_half_says_so(self):
        out = fp.gate_diagnostics({"linux": "fail", "mac": "ok", "linux_run": self.RUN, "linux_errors": []})
        self.assertIn(f"(no error lines could be retrieved from {self.RUN}; open it for the failure)", out)

    def test_2301_shape_ios_half_with_only_step_summaries_says_so(self):
        tail = ("== ios-gate gate/20261029T220718-1 ccdaa652\n== Release: compiles\n== Debug: compiles\n"
                "== tests: FAIL to compile (ArbosTests)\n== tests: errors NOT on main (0be7d85f) — this branch's regression:\n"
                "== done rc=1\n")
        out = fp.gate_diagnostics({"linux": "ok", "mac": "skipped", "ios": "fail", "ios_tail": tail})
        self.assertIn("== tests: FAIL to compile (ArbosTests)", out)
        self.assertIn("(no error lines could be retrieved from the gate Mac's log of this run; open it for the failure)", out)

    def test_an_ios_half_with_errors_is_shown_as_is(self):
        tail = "== tests: FAIL to compile (ArbosTests)\nerror: apps/ios/Arbos/ArbosTests/A.swift: cannot find 'X' in scope\n"
        out = fp.gate_diagnostics({"linux": "ok", "mac": "skipped", "ios": "fail", "ios_tail": tail})
        self.assertIn("error: apps/ios/Arbos/ArbosTests/A.swift: cannot find 'X' in scope", out)
        self.assertNotIn("could be retrieved", out)


class GateFollowupCommentTest(unittest.TestCase):
    """The handed-off owner's comment (#2232's path) from a red Linux half to the posted body."""

    def post(self, linux_errors):
        comments = []
        loop = fp.Loop.__new__(fp.Loop)
        loop.store = SimpleNamespace(save_state=lambda fid, st: None)
        loop.gh = SimpleNamespace(comment=lambda issue, body: comments.append(body))
        st = {"branch_head": "a216eb0d" + "0" * 32, "branch": "feedback/sidecar-sessions-mac-2026-10-28",
              "handed_off_to": "bc-49e6e7e3", "issue": 2232}
        active = {"linux": "fail", "mac": "skipped", "linux_run": GateDiagnosticsTest.RUN, "linux_errors": linux_errors}
        loop.gate_followup("sidecar-sessions-mac-2026-10-28", st, "rust", fp.gate_diagnostics(active))
        self.assertEqual(len(comments), 1)
        return comments[0].split("```")[1]

    def test_the_comment_carries_the_errors(self):
        gh = fake_gh([JOB_2232], CLIPPY_2232)
        block = self.post(gh.run_error_lines(1, limit=60))
        self.assertIn("error[E0004]: non-exhaustive patterns: `sync::ClientCommand::OutsideAgentLog { .. }` not covered", block)
        self.assertIn("    --> crates/arbos-mac/src/sync/fixture.rs:1120:11", block)

    def test_the_comment_admits_missing_errors(self):
        block = self.post([])
        self.assertIn("no error lines could be retrieved", block)

    def test_the_first_errors_survive_a_long_list(self):
        block = self.post([f"error[E0{i:03}]: e{i}" for i in range(60)])
        self.assertIn("Linux (CI on the integration tree", block)
        self.assertIn("error[E0000]: e0", block)


class IosGateErrorsReachThePollerTest(unittest.TestCase):
    """ios-gate.sh prints the test-target errors; rust-gate.sh `status` filters the run log for the poller."""

    XCODE = "/Users/ec2-user/arbos-gate/apps/ios/Arbos/ArbosTests/MentionChipTests.swift:41:9: error: cannot find 'MentionChip' in scope"

    def status_of(self, printer: str) -> str:
        with tempfile.TemporaryDirectory() as home:
            runs = Path(home) / "arbos-rust-gate-runs"
            runs.mkdir()
            script = (f'eval "$(sed -n \'/^test_error_set()/,/^}}/p;/^new_test_errors()/,/^}}/p;/^print_errors()/,/^}}/p\' {HERE / "ios-gate.sh"})"\n'
                      f'x=$(mktemp); printf "%s\\n" "$XCODE" > "$x"\n'
                      'errs=$(test_error_set "$x"); new=$(new_test_errors "$errs" "")\n'
                      'echo "== tests: FAIL to compile (ArbosTests)"\n'
                      'echo "== tests: errors NOT on main (0be7d85f) — this branch\'s regression:"\n'
                      f'{printer}\n'
                      'echo "== done rc=1"\n')
            log = subprocess.run(["bash", "-c", script], env={**os.environ, "XCODE": self.XCODE}, capture_output=True, text=True, check=True).stdout
            (runs / "g1-ios.log").write_text(log)
            (runs / "g1-ios.rc").write_text("1\n")
            r = subprocess.run(["bash", str(HERE / "rust-gate.sh"), "status", "g1-ios"], capture_output=True, text=True,
                               env={**os.environ, "HOME": home, "ARBOS_SCRATCH": home})
            self.assertEqual(r.returncode, 0, r.stderr)
            return r.stdout

    def test_the_old_bare_lines_are_filtered_out(self):
        out = self.status_of('printf "%s\\n" "$new" | head -20')
        self.assertNotIn("MentionChip", out)

    def test_failing_test_names_reach_the_poller(self):
        sed = next(l for l in (HERE / "ios-gate.sh").read_text().splitlines() if "... FAILED/'" in l).strip()
        out = self.status_of(sed.replace('"$new"', '"MentionChipTests/testTieBreak"'))
        self.assertIn("test MentionChipTests/testTieBreak ... FAILED", out)

    def test_print_errors_lines_reach_the_poller(self):
        out = self.status_of('print_errors "$new"')
        self.assertIn("error: apps/ios/Arbos/ArbosTests/MentionChipTests.swift: cannot find 'MentionChip' in scope", out)
        self.assertTrue(out.startswith("fail\n"))


if __name__ == "__main__":
    unittest.main()
