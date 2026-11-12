#!/usr/bin/env python3
"""boxd-migrate.py: what it decides, and that a disk never loses its last name.

The decisions are tested as the pure functions they are. The moves run the real script against
real files in a temp directory (real hard links, a real ext4 superblock's worth of bytes) with
the relay, the daemon and losetup stood in for by `FakeWorld`; no network, no root.

    python3 -m unittest discover -s scripts/deploy -p 'test_boxd_migrate.py'
"""
import contextlib
import importlib.util
import io
import os
import tempfile
import unittest
import urllib.error
import uuid

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("boxd_migrate", os.path.join(HERE, "boxd-migrate.py"))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)

U = "u_alice"
GIB = m.GIB


def seen(**over):
    """An untouched, movable gvisor box, unless told otherwise."""
    base = dict(provider="gvisor", state="awake", old=(1, 10), new=None, size=8 * GIB,
                same_fs=True, installed=True, record=None)
    return m.Seen(**{**base, **over})


class Decide(unittest.TestCase):
    def step(self, **over):
        return m.decide(seen(**over))[0]

    def test_a_box_goes_through_the_steps_in_order(self):
        rec = {"user": U}
        self.assertEqual(self.step(), "link")
        self.assertEqual(self.step(new=(1, 10), record=rec), "switch")
        self.assertEqual(self.step(provider=None, new=(1, 10), record=rec), "start")
        self.assertEqual(self.step(provider="boxd", new=(1, 10), record=rec), "verify")
        self.assertEqual(self.step(provider="boxd", old=None, new=(1, 10), record=rec), "done")

    def test_a_box_that_cannot_be_moved_is_left_alone_and_told_why(self):
        for over, word in [
            (dict(size=20 * GIB), "20 GiB"),
            (dict(same_fs=False), "different filesystems"),
            (dict(installed=False), "not installed"),
            (dict(state="asleep"), "asleep"),
            (dict(old=None), "no work.img"),
            (dict(provider="process"), "process"),
            (dict(provider=None), "no box"),
            # Off Docker and linked, but by hand: nothing says what the disk was before.
            (dict(provider=None, new=(1, 10)), "no box"),
            (dict(provider="boxd", old=None, new=None), "no disk on this host"),
            (dict(provider="boxd", new=(1, 11)), "a disk of its own"),
        ]:
            step, why = m.decide(seen(**over))
            self.assertEqual(step, "skip", over)
            self.assertIn(word, why)

    def test_two_different_disks_for_one_user_stop_the_tool(self):
        self.assertEqual(self.step(new=(1, 11)), "stuck")

    def test_a_moved_box_is_the_same_box_or_it_is_not_verified(self):
        before = {"user": U, "file": [1, 10], "fs_uuid": "a"}
        good = dict(state="awake", provider="boxd", node="node-0", answers=True, box_user=U,
                    running=True, file=[1, 10], fs_uuid="a")
        self.assertIsNone(m.checks(before, good))
        for over in [dict(state="waking"), dict(provider="gvisor"), dict(node=None),
                     dict(answers=False), dict(box_user="u_bob"), dict(running=False),
                     dict(file=[1, 11]), dict(file=None), dict(fs_uuid="b")]:
            self.assertIsNotNone(m.checks(before, {**good, **over}), over)

    def test_idle_is_awake_answering_and_nothing_running_or_waiting(self):
        def box(turn=None, jobs=0, **relay):
            ws = [{"project_id": "p", "turn": turn or {"running": False, "sends_waiting": 0},
                   "jobs_running": jobs}]
            return {"relay": {"state": "awake", "turn_running_since": None, **relay},
                    "box": {"reachable": True, "state": {"workspaces": ws}}}
        self.assertIsNone(m.idle(box()))
        self.assertIsNotNone(m.idle(box(turn={"running": True, "sends_waiting": 0})))
        self.assertIsNotNone(m.idle(box(turn={"running": False, "sends_waiting": 1})))
        self.assertIsNotNone(m.idle(box(jobs=1)))
        self.assertIsNotNone(m.idle(box(turn_running_since=5)))
        self.assertIsNotNone(m.idle(box(state="asleep")))
        self.assertIsNotNone(m.idle({"relay": {"state": "awake"}, "box": {"reachable": False}}))
        self.assertIsNotNone(m.idle({}))


class FakeWorld(m.World):
    """The real `World` over real files, with a relay, a daemon and losetup that are dicts."""

    box = {"exists": True, "provider": "gvisor", "state": "awake", "snapshot": "arbos-agent-1.0"}
    setting = "gvisor"       # users.box_provider
    attached_old = True      # Docker's loop device on the relay's name
    running = False          # the daemon's box
    busy = False
    answers_as = U           # whose agentd answers once the box is on the daemon
    refuse_start = False

    def __init__(self, args, admin):
        super().__init__(args, admin)
        self.calls = []

    def relay(self, method, path, body=None, token=None, timeout=60):
        if method != "GET":
            self.calls.append(f"{method} {path.split('?')[0]}")
        if path == "/admin/users":
            return [{"id": U, "machine": dict(self.box)}]
        if path.startswith("/machine/debug"):
            turn = {"running": self.busy, "sends_waiting": 0}
            on_daemon = self.box["provider"] == "boxd"
            return {
                "relay": {"state": self.box["state"], "turn_running_since": None,
                          "provider": {"provider": self.box["provider"],
                                       "node_id": "node-0" if on_daemon else None}},
                "box": {"reachable": self.box["exists"], "state": {
                    "user_id": self.answers_as if on_daemon else U,
                    "workspaces": [{"project_id": "p", "turn": turn, "jobs_running": 0}]}},
            }
        if method == "PUT":
            self.setting = body["provider"]
        elif method == "DELETE" and path.startswith("/admin/users/"):
            self.box = {**self.box, "exists": False, "state": "asleep"}
            self.attached_old = self.running = False
        elif path.endswith("/devices"):
            return {"access_token": "dev-token", "device_id": "dev1"}
        elif path == "/machine/start":
            if self.refuse_start:
                raise urllib.error.HTTPError(path, 502, "box create: no", None, None)
            self.box = {**self.box, "exists": True, "provider": self.setting, "state": "awake"}
            self.running = self.setting == "boxd"
            self.attached_old = self.setting == "gvisor"
        return None

    def daemon_boxes(self):
        return {U: {"user_id": U, "running": self.running}}

    def attached(self, path):
        return self.attached_old if path == self.old(U) else self.running


class Moves(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = self.tmp.name
        self.data, self.boxd = os.path.join(root, "relay"), os.path.join(root, "boxd")
        os.makedirs(os.path.join(self.data, "boxes", U))
        os.makedirs(os.path.join(self.boxd, "images", "1.0", "rootfs"))
        self.old = os.path.join(self.data, "boxes", U, "work.img")
        self.new = os.path.join(self.boxd, "boxes", U, "work.img")
        # A sparse 8 GiB file with an ext4 superblock's magic and a filesystem id in it.
        with open(self.old, "wb") as fh:
            fh.truncate(8 * GIB)
            fh.seek(1024 + 0x38)
            fh.write(b"\x53\xef")
            fh.seek(1024 + 0x68)
            fh.write(uuid.uuid4().bytes)
        self.disk = os.stat(self.old).st_ino
        self.token = os.path.join(root, "token")
        with open(self.token, "w", opener=lambda p, f: os.open(p, f, 0o600)) as fh:
            fh.write("t" * 32)
        os.environ["ARBOS_RELAY_ADMIN_TOKEN"] = "admin"
        self.world = None

    def run_tool(self, *flags, **fake):
        def world(args, admin):
            self.world = self.world or FakeWorld(args, admin)
            for key, value in fake.items():
                setattr(self.world, key, value)
            return self.world
        argv = ["--user", U, "--data", self.data, "--boxd-root", self.boxd,
                "--boxd-token-file", self.token, "--state-dir", os.path.join(self.boxd, "migrate"),
                "--idle-seconds", "0", "--idle-wait", "0", "--wake-seconds", "0", "--poll", "0"]
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = m.main([*flags, *argv], world=world)
        return code, out.getvalue()

    def ino(self, path):
        return os.stat(path).st_ino if os.path.exists(path) else None

    def test_a_box_is_moved_and_the_old_name_goes_last(self):
        code, out = self.run_tool("--yes")
        self.assertEqual(code, 0, out)
        self.assertIn("moved", out)
        w = self.world
        self.assertEqual((self.ino(self.old), self.ino(self.new)), (None, self.disk))
        self.assertEqual((w.box["provider"], w.box["state"], w.setting), ("boxd", "awake", "boxd"))
        self.assertEqual(w.calls, [
            f"PUT /admin/users/{U}/box", f"DELETE /admin/users/{U}/box",
            f"POST /admin/users/{U}/devices", "POST /machine/start", "DELETE /auth/devices/dev1",
        ])
        self.assertEqual(w.record(U)["phase"], "done")
        # Again: nothing to do, and nothing is asked of the relay but what it has.
        w.calls.clear()
        code, out = self.run_tool("--yes")
        self.assertEqual((code, w.calls), (0, []), out)
        self.assertIn("done", out)

    def test_a_box_that_does_not_verify_is_put_back_on_its_disk(self):
        code, out = self.run_tool("--yes", answers_as="u_bob")
        self.assertEqual(code, 1, out)
        self.assertIn("rolled_back", out)
        self.assertIn("another user's", out)
        w = self.world
        self.assertEqual(self.ino(self.old), self.disk, "the disk kept its name")
        self.assertFalse(os.path.exists(os.path.dirname(self.new)), "the daemon's link is gone")
        self.assertEqual((w.box["provider"], w.box["state"], w.setting), ("gvisor", "awake", "gvisor"))
        self.assertEqual(w.record(U)["phase"], "rolled_back")
        # It can be tried again, and with nothing wrong it goes through.
        code, out = self.run_tool("--yes", answers_as=U)
        self.assertEqual((code, self.ino(self.old), self.ino(self.new)), (0, None, self.disk), out)

    def test_a_refused_start_is_rolled_back_too(self):
        code, out = self.run_tool("--yes", refuse_start=True)
        self.assertEqual(code, 1, out)
        self.assertEqual(self.ino(self.old), self.disk)
        # The start back on gvisor was refused as well: the disk is under its old name alone,
        # and the tool says a person has to look.
        self.assertIn("stuck", out)
        self.assertFalse(os.path.exists(self.new))

    def test_a_run_cut_short_is_picked_up_where_it_stopped(self):
        # After the link.
        self.run_tool("--dry-run")
        w = self.world
        os.makedirs(os.path.join(self.boxd, "migrate"))
        w.write_record(U, phase="linking", file=list(w.file_id(self.old)[0]),
                       fs_uuid=w.fs_uuid(self.old), size=8 * GIB)
        w.link(U)
        code, out = self.run_tool("--yes")
        self.assertEqual((code, self.ino(self.old), self.ino(self.new)), (0, None, self.disk), out)
        self.assertNotIn("link\n", out)

    def test_a_run_cut_short_after_the_docker_box_went_starts_the_new_one(self):
        self.run_tool("--dry-run")
        w = self.world
        os.makedirs(os.path.join(self.boxd, "migrate"))
        w.write_record(U, phase="switching", file=list(w.file_id(self.old)[0]),
                       fs_uuid=w.fs_uuid(self.old), size=8 * GIB)
        w.link(U)
        w.set_provider(U, "boxd")
        w.delete_box(U)
        w.calls.clear()
        code, out = self.run_tool("--yes")
        self.assertEqual((code, self.ino(self.old), self.ino(self.new)), (0, None, self.disk), out)
        self.assertEqual(w.calls[0], f"POST /admin/users/{U}/devices")

    def test_a_busy_box_is_not_touched(self):
        code, out = self.run_tool("--yes", busy=True)
        self.assertEqual(code, 0, out)
        self.assertIn("not idle", out)
        self.assertEqual((self.ino(self.old), self.ino(self.new), self.world.calls), (self.disk, None, []))

    def test_the_daemons_link_is_never_removed_when_it_is_the_disks_last_name(self):
        self.run_tool("--dry-run")
        w = self.world
        os.makedirs(os.path.join(self.boxd, "migrate"))
        w.write_record(U, phase="starting")
        w.link(U)
        os.unlink(self.old)
        outcome, why = m.rollback(w, U, "a test")
        self.assertEqual(outcome, "stuck", why)
        self.assertEqual(self.ino(self.new), self.disk)

    def test_it_does_nothing_without_yes_and_a_dry_run_changes_nothing(self):
        code, out = self.run_tool()
        self.assertEqual(code, 2, out)
        self.assertIsNone(self.world, "nothing was even looked at")
        code, out = self.run_tool("--dry-run", "--yes")
        self.assertEqual(code, 2, out)

        code, out = self.run_tool("--dry-run")
        self.assertEqual(code, 0, out)
        for would in ["would wait until idle", f"ln {self.old} {self.new}", '{"provider": "boxd"}',
                      "DELETE", "POST", "would verify", f"would rm {self.old}"]:
            self.assertIn(would, out)
        self.assertEqual(self.world.calls, [])
        self.assertEqual((self.ino(self.old), self.ino(self.new)), (self.disk, None))
        self.assertFalse(os.path.exists(os.path.join(self.boxd, "migrate")))

    def test_the_tokens_go_to_this_host_only_and_a_user_id_is_one(self):
        for flags in [("--relay", "https://relay.example"), ("--boxd-url", "http://10.0.0.5:7790"),
                      ("--user", "../etc"), ("--all",)]:
            code, out = self.run_tool("--yes", *flags)
            self.assertEqual(code, 2, (flags, out))
        os.chmod(self.token, 0o644)
        with self.assertRaises(SystemExit):
            self.run_tool("--yes")


if __name__ == "__main__":
    unittest.main()
