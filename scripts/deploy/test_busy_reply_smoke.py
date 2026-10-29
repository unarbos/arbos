#!/usr/bin/env python3
"""busy-reply-smoke.py under an outer `timeout`: the log keeps every line while the run is
still going, and a SIGTERM still prints the JSON report (with the verdict once both probes
are in), kills the arbosctl clients and revokes the devices.

Runs the real script against a fake relay (admin device mint, /projects, device revoke) and a
fake `arbosctl` on PATH — no network, no relay rows.

    python3 -m unittest discover -s scripts/deploy -p 'test_busy_reply_smoke.py'
"""
import json
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
SMOKE = os.path.join(HERE, "busy-reply-smoke.py")
BUSY_MSG = "01BUSYMSG0000000000000000A"

FAKE_ARBOSCTL = r'''#!{python}
import json, os, sys, time, zlib

args = sys.argv[1:]
pids = os.environ["FAKE_PIDS"]
if args[:2] == ["chat", "open"]:
    title = args[args.index("--title") + 1]
    print("01CHAT" + format(zlib.crc32(title.encode()), "020d"))
elif args[:2] == ["chat", "dump"]:
    print(json.dumps({{"messages": [{{"id": "busyrow", "role": "assistant", "inReplyTo": "{busy}",
                                      "status": "streaming"}}]}}))
elif args[:2] == ["chat", "send"]:
    open(os.path.join(pids, str(os.getpid())), "w").close()
    text = args[4]
    if text.startswith("Use the bash tool"):
        sys.stderr.write("[sent message {busy} (box=awake)]\n"); sys.stderr.flush()
        while True:
            time.sleep(1)
    sys.stderr.write("[sent message 01PROBEMSG0000000000000000 (box=awake)]\n"); sys.stderr.flush()
    time.sleep(0.3)
    sys.stdout.write("[assistant ab%04d] Yes, I'm here.\n" % (os.getpid() % 10000)); sys.stdout.flush()
    if os.environ.get("FAKE_PROBE") == "hang":
        while True:
            time.sleep(1)
'''


class FakeRelay(BaseHTTPRequestHandler):
    revoked = []
    minted = 0

    def _send(self, code, body=None):
        raw = b"" if body is None else json.dumps(body).encode()
        self.send_response(code)
        self.send_header("content-length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length") or 0))
        FakeRelay.minted += 1
        self._send(201, {"access_token": f"tok{FakeRelay.minted}", "device_id": f"dev{FakeRelay.minted}"})

    def do_GET(self):
        self._send(200, [{"id": "01PROJECT00000000000000000"}])

    def do_DELETE(self):
        FakeRelay.revoked.append(self.path)
        self._send(204)

    def log_message(self, *_):
        pass


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    try:
        with open(f"/proc/{pid}/stat") as fh:
            return fh.read().split(")")[-1].split()[0] != "Z"
    except OSError:
        return True


class SmokeUnderSigterm(unittest.TestCase):
    def setUp(self):
        FakeRelay.revoked, FakeRelay.minted = [], 0
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), FakeRelay)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.tmp = tempfile.TemporaryDirectory()
        bindir = os.path.join(self.tmp.name, "bin")
        self.pids = os.path.join(self.tmp.name, "pids")
        os.makedirs(bindir)
        os.makedirs(self.pids)
        fake = os.path.join(bindir, "arbosctl")
        with open(fake, "w") as fh:
            fh.write(FAKE_ARBOSCTL.format(python=sys.executable, busy=BUSY_MSG))
        os.chmod(fake, 0o755)
        self.log = os.path.join(self.tmp.name, "smoke.log")
        self.env = dict(os.environ, PATH=bindir + os.pathsep + os.environ["PATH"],
                        ARBOS_RELAY_ADMIN_TOKEN="admin", FAKE_PIDS=self.pids)

    def tearDown(self):
        for name in os.listdir(self.pids):
            try:
                os.kill(int(name), signal.SIGKILL)
            except ProcessLookupError:
                pass
        self.server.shutdown()
        self.server.server_close()
        self.tmp.cleanup()

    def start(self, probe):
        relay = f"http://127.0.0.1:{self.server.server_address[1]}"
        return subprocess.Popen(
            [sys.executable, SMOKE, "--relay", relay, "--user", "test_fake", "--probe-delay", "0",
             "--busy-seconds", "600", "--log", self.log],
            env=dict(self.env, FAKE_PROBE=probe), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

    def wait_for_log(self, proc, needle, count, timeout=40):
        """Polls the log while the smoke is still running — a block-buffered log stays empty here."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            self.assertIsNone(proc.poll(), "the smoke exited before it was signalled")
            if os.path.exists(self.log):
                with open(self.log) as fh:
                    if fh.read().count(needle) >= count:
                        return
            time.sleep(0.2)
        proc.kill()
        self.fail(f"{needle!r} x{count} never reached the log while the smoke was running")

    def finish(self, proc):
        proc.send_signal(signal.SIGTERM)
        out, err = proc.communicate(timeout=30)
        lines = [l for l in out.splitlines() if l.strip()]
        self.assertEqual(len(lines), 1, f"stdout={out!r} stderr={err!r}")
        return proc.returncode, json.loads(lines[0]), err

    def assert_cleaned_up(self):
        self.assertEqual(sorted(FakeRelay.revoked),
                         ["/auth/devices/dev1", "/auth/devices/dev2", "/auth/devices/dev3"])
        clients = [int(n) for n in os.listdir(self.pids)]
        self.assertEqual(len(clients), 3, "busy + two probes")
        time.sleep(0.5)
        self.assertEqual([p for p in clients if alive(p)], [], "arbosctl clients left running")
        with open(self.log) as fh:
            tail = fh.read().splitlines()[-1]
        self.assertIn(" report {", tail)

    def test_sigterm_after_both_probes_keeps_the_verdict(self):
        proc = self.start("reply")
        self.wait_for_log(proc, "Yes, I'm here.", 2)
        time.sleep(1)
        rc, report, err = self.finish(proc)
        self.assertEqual(rc, 0, err)
        self.assertIs(report["ok"], True)
        self.assertTrue(report["same_chat_ok"] and report["other_chat_ok"])
        self.assertIn("verdict stands", report["terminated"])
        self.assertEqual(report["other_chat"]["exit"], 0)
        self.assert_cleaned_up()

    def test_sigterm_mid_probe_reports_partial_timings(self):
        proc = self.start("hang")
        self.wait_for_log(proc, "Yes, I'm here.", 2)
        rc, report, err = self.finish(proc)
        self.assertEqual(rc, 2, err)
        self.assertIsNone(report["ok"])
        self.assertEqual(report["terminated"], "SIGTERM before the gate verdict")
        for name in ("same_chat", "other_chat"):
            self.assertIsNotNone(report[name]["first_text_s"], report)
            self.assertIsNone(report[name]["exit"], report)
        self.assertNotIn("Traceback", err)
        self.assert_cleaned_up()


if __name__ == "__main__":
    unittest.main()
