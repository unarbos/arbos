"""Exercise the real local harness admission scripts without keys or services."""

import hashlib
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
PRIME = ROOT / "image/arbos-shell"
CLAUDE = ROOT / "image/hooks/claude/pre-tool-use.sh"
DENIED = {
    "daytona-api-key": "printf '%s' 'DAYTONA_API_KEY'",
    "relay-admin": "printf '%s' '/admin '",
    "arbos-state-tamper": "printf '%s' '/work/.arbos/agentd'",
}


class ToolAuditTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="arbos-tool-audit-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.cwd = self.root / "cwd"
        self.cwd.mkdir()
        self.audit = self.root / "audit.jsonl"
        # In particular, do not inherit BASH_ENV or a real workspace's credentials.
        self.env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "ARBOS_AUDIT_PATH": str(self.audit),
            "ARBOS_STATE_DIR": str(self.root / "state"),
            "ARBOS_SESSION_ID": "synthetic-session",
            "ARBOS_TRACE_ID": "synthetic-trace",
        }

    def prime(self, *args, input_text=None):
        return subprocess.run(
            ["/bin/bash", str(PRIME), *args], input=input_text, env=self.env,
            cwd=self.cwd, capture_output=True, text=True, timeout=5,
        )

    def claude(self, tool="Bash", arguments=None, session="synthetic-session"):
        payload = {"tool_name": tool, "tool_input": arguments or {}, "session_id": session}
        return subprocess.run(
            ["/bin/bash", str(CLAUDE)], input=json.dumps(payload), env=self.env,
            cwd=self.cwd, capture_output=True, text=True, timeout=5,
        )

    def events(self):
        return [json.loads(line) for line in self.audit.read_text().splitlines()]

    def test_prime_preserves_command_arguments_and_exit_code(self):
        result = self.prime("-c", 'printf "%s|%s" "$0" "$1"; exit 7', "tool-name", "argument with spaces")
        self.assertEqual(result.returncode, 7)
        self.assertEqual(result.stdout, "tool-name|argument with spaces")
        self.assertEqual(len(self.audit.read_text().splitlines()), 1)

    def test_prime_exec_preserves_the_wrapper_pid(self):
        proc = subprocess.Popen(
            ["/bin/bash", str(PRIME), "-c", 'printf "%s" "$$"'], env=self.env,
            cwd=self.cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        stdout, _ = proc.communicate(timeout=5)
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(stdout, str(proc.pid))

    def test_prime_exec_preserves_signal_delivery(self):
        proc = subprocess.Popen(
            ["/bin/bash", str(PRIME), "-c", "printf 'ready\\n'; exec sleep 60"],
            env=self.env, cwd=self.cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        try:
            self.assertTrue(select.select([proc.stdout], [], [], 5)[0], "shell did not start")
            self.assertEqual(proc.stdout.readline(), "ready\n")
            proc.send_signal(signal.SIGTERM)
            proc.communicate(timeout=5)
            self.assertEqual(proc.returncode, -signal.SIGTERM)
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.communicate()

    def test_prime_keeps_stdin_available_for_non_c_invocations(self):
        result = self.prime("-s", input_text="printf 'stdin preserved'")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "stdin preserved")
        self.assertFalse(self.audit.exists(), "non-tool shell invocations keep their passthrough behavior")

    def test_prime_denies_each_rule_before_execution(self):
        for rule, command in DENIED.items():
            with self.subTest(rule=rule):
                sentinel = self.root / f"must-not-run-{rule}"
                result = self.prime("-c", f"{command}; touch '{sentinel}'")
                self.assertEqual(result.returncode, 126)
                self.assertIn(rule, result.stderr)
                self.assertEqual(result.stdout, "")
                self.assertFalse(sentinel.exists())
        self.assertEqual(len(self.audit.read_text().splitlines()), 3)

    def test_claude_allows_benign_tools_without_executing_them(self):
        sentinel = self.root / "hook-must-not-execute"
        result = self.claude(arguments={"command": f"touch '{sentinel}'"})
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertFalse(sentinel.exists())
        self.assertEqual(len(self.audit.read_text().splitlines()), 1)

    def test_claude_denies_each_rule(self):
        for rule, command in DENIED.items():
            with self.subTest(rule=rule):
                result = self.claude(arguments={"command": command})
                self.assertEqual(result.returncode, 2)
                self.assertIn(rule, result.stderr)
        self.assertEqual(len(self.audit.read_text().splitlines()), 3)

    def test_claude_state_tamper_rule_keeps_its_existing_tool_scope(self):
        for tool in ["Bash", "Write", "Edit", "MultiEdit", "Read"]:
            with self.subTest(tool=tool):
                result = self.claude(tool=tool, arguments={"path": "/work/.arbos/agentd"})
                self.assertEqual(result.returncode, 0 if tool == "Read" else 2)

    def test_audit_write_failure_does_not_change_admission_or_execution(self):
        parent = self.root / "not-a-directory"
        parent.write_text("test")
        self.env["ARBOS_AUDIT_PATH"] = str(parent / "audit.jsonl")
        self.assertEqual(self.prime("-c", "printf allowed").stdout, "allowed")
        self.assertEqual(self.prime("-c", DENIED["relay-admin"]).returncode, 126)
        self.assertEqual(self.claude(arguments={"command": "printf allowed"}).returncode, 0)
        self.assertEqual(self.claude(arguments={"command": DENIED["relay-admin"]}).returncode, 2)

    def test_prime_audit_hashes_the_full_command_without_its_private_text(self):
        marker = "SYNTHETIC_PRIVATE_PRIME_VALUE"
        command = f"printf '%s' '{marker}{'x' * 300}'"
        result = self.prime("-c", command)
        self.assertEqual(result.returncode, 0)
        self.assertNotIn(marker, self.audit.read_text())
        events = self.events()
        self.assertEqual(len(events), 1)
        self.assertNotIn("input_preview", events[0])
        self.assertEqual(events[0]["input_hash"], hashlib.sha256(command.encode()).hexdigest())
        self.assertIsInstance(events[0]["ts"], int)

    def test_claude_audit_hashes_the_full_tool_input_without_its_private_text(self):
        marker = "SYNTHETIC_PRIVATE_CLAUDE_VALUE"
        arguments = {"command": f"printf '{marker}'", "metadata": "α" + "x" * 300}
        self.assertEqual(self.claude(arguments=arguments).returncode, 0)
        self.assertNotIn(marker, self.audit.read_text())
        events = self.events()
        self.assertEqual(len(events), 1)
        self.assertNotIn("input_preview", events[0])
        encoded = json.dumps(arguments, ensure_ascii=False, separators=(",", ":")).encode()
        self.assertEqual(events[0]["input_hash"], hashlib.sha256(encoded).hexdigest())
        self.assertIsInstance(events[0]["ts"], int)

    def test_prime_metadata_with_quotes_and_newlines_remains_one_json_event(self):
        self.cwd = self.root / 'working"directory\ncontinued'
        self.cwd.mkdir()
        self.env["ARBOS_SESSION_ID"] = 'session"quoted\ncontinued'
        self.env["ARBOS_TRACE_ID"] = 'trace\\quoted"\ncontinued'
        self.assertEqual(self.prime("-c", "true").returncode, 0)
        events = self.events()
        self.assertEqual(len(events), 1)
        self.assertEqual(events[0]["session"], self.env["ARBOS_SESSION_ID"])
        self.assertEqual(events[0]["trace_id"], self.env["ARBOS_TRACE_ID"])
        self.assertEqual(events[0]["cwd"], str(self.cwd.resolve()))
        self.assertIsNone(events[0]["denied"])
        self.assertIsInstance(events[0]["ts"], int)

    def test_claude_metadata_with_quotes_and_newlines_remains_one_json_event(self):
        session = 'session"quoted\ncontinued'
        tool = 'Custom"tool\ncontinued'
        self.assertEqual(self.claude(tool=tool, session=session).returncode, 0)
        events = self.events()
        self.assertEqual(len(events), 1)
        self.assertEqual(events[0]["session"], session)
        self.assertEqual(events[0]["tool"], tool)
        self.assertIsNone(events[0]["denied"])
        self.assertIsInstance(events[0]["ts"], int)

    def test_prime_denial_creates_a_fresh_audit_directory_and_redacts_input(self):
        self.audit = self.root / "fresh/nested/prime.jsonl"
        self.env["ARBOS_AUDIT_PATH"] = str(self.audit)
        command = "printf '%s' 'DAYTONA_API_KEY SYNTHETIC_PRIVATE_DENIAL'"
        self.assertEqual(self.prime("-c", command).returncode, 126)
        events = self.events()
        self.assertEqual(len(events), 1)
        self.assertEqual(events[0]["denied"], "daytona-api-key")
        self.assertEqual(events[0]["input_hash"], hashlib.sha256(command.encode()).hexdigest())
        self.assertNotIn("SYNTHETIC_PRIVATE_DENIAL", self.audit.read_text())

    def test_claude_denial_creates_a_fresh_audit_directory_and_redacts_input(self):
        self.audit = self.root / "fresh/nested/claude.jsonl"
        self.env["ARBOS_AUDIT_PATH"] = str(self.audit)
        arguments = {"command": "printf '%s' 'DAYTONA_API_KEY SYNTHETIC_PRIVATE_DENIAL'"}
        self.assertEqual(self.claude(arguments=arguments).returncode, 2)
        events = self.events()
        self.assertEqual(len(events), 1)
        self.assertEqual(events[0]["denied"], "daytona-api-key")
        encoded = json.dumps(arguments, separators=(",", ":")).encode()
        self.assertEqual(events[0]["input_hash"], hashlib.sha256(encoded).hexdigest())
        self.assertNotIn("SYNTHETIC_PRIVATE_DENIAL", self.audit.read_text())


if __name__ == "__main__":
    unittest.main()
