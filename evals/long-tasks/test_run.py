#!/usr/bin/env python3
"""The run leaves the lane projects named as it found them (AF-142).

`ensure_project` matches a lane on its *name*, so a rename the run never puts back makes the next run
create a second project for that lane. Both then cost the box an idle agent: a parked `pi` of its own
(#2302) and a room reopened at boot for every chat it holds (#2367) — load the suite measures the agent
against. Run 16 counted 15 projects on the eval user for the five lanes `run.py` names.

Nothing here talks to the relay: `lib.relay`'s calls are stubbed over one in-memory project list.

    python3 -m pytest evals/long-tasks/test_run.py
"""
import contextlib
import io
import sys
import tempfile
import threading
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import run  # noqa: E402
from lib import relay as rl  # noqa: E402


class FakeRelay:
    """One test user's projects by id, every `project create` the harness asked for, and a PATCH that
    can be made to refuse (a rename lost to a failed call, which the old code could not tell from one
    that worked)."""

    def __init__(self, names):
        self.names = dict(names)
        self.created = []
        self.patch_refused = set()

    def projects(self):
        return [{"id": pid, "name": name} for pid, name in self.names.items()]

    def project_name(self, pid):
        return self.names.get(pid)

    def patch_project(self, pid, body):
        if pid in self.patch_refused:
            return 503, {"error": "relay refused"}
        self.names[pid] = body["name"]
        return 204, None

    def ctl(self, *args, **kw):
        if args[:2] == ("project", "create"):
            name = args[args.index("--name") + 1]
            self.names[f"pid-{name}"] = name
            self.created.append(name)
        return 0, "", ""


class LaneNameTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.fake = FakeRelay({"pid-lane-a": "lane-a"})
        self.rs = {"run_id": "T", "budget": 8.0, "spent_at_start": 0.0, "lock": threading.Lock(),
                   "renames": {}, "projects": {"a": "pid-lane-a"}}
        self.lane = run.Lane("a", "pid-lane-a", [{"id": "i1", "category": "identity", "lane": "a"}],
                             None, self.rs, Path(self.tmp.name), io.StringIO())

    @contextlib.contextmanager
    def stubbed(self):
        with mock.patch.multiple(rl, projects=self.fake.projects, project_name=self.fake.project_name,
                                 patch_project=self.fake.patch_project, ctl=self.fake.ctl,
                                 spent_today=lambda: (0.0, 10.0), routines=lambda pid: []):
            yield

    def rename(self, st):
        self.lane.driver_action({"kind": "rename_project", "name": "Quill", "settle_s": 0}, st)

    def test_a_task_that_throws_after_the_rename_leaves_the_lane_findable(self):
        def throws(task):
            self.rename({})
            raise RuntimeError("chat open: 502")  # anything between the rename and its cleanup action

        self.lane.run_task = throws
        with self.stubbed():
            self.lane.run()
            self.assertEqual(self.fake.names["pid-lane-a"], "lane-a")
            self.assertEqual(rl.ensure_project("lane-a"), "pid-lane-a")
        self.assertEqual(self.fake.created, [], "the next run created a second project for lane a")
        self.assertEqual(self.rs["renames"], {})

    def test_a_restore_the_relay_refused_is_swept_at_the_end_of_the_run(self):
        def renames_then_fails_to_restore(task):
            st = {}
            self.rename(st)
            self.fake.patch_refused.add("pid-lane-a")
            self.lane.driver_action({"kind": "restore_name"}, st)
            return {"task": "i1", "success": True}

        self.lane.run_task = renames_then_fails_to_restore
        with self.stubbed():
            self.lane.run()
            self.assertEqual(self.fake.names["pid-lane-a"], "Quill")
            self.assertEqual(self.rs["renames"], {"pid-lane-a": "lane-a"})
            self.fake.patch_refused.clear()
            run.end_of_run_sweep(self.rs)
            self.assertEqual(self.fake.names["pid-lane-a"], "lane-a")
            self.assertEqual(rl.ensure_project("lane-a"), "pid-lane-a")
        self.assertEqual(self.fake.created, [])

    def test_a_restore_that_landed_owes_nothing(self):
        def renames_and_restores(task):
            st = {}
            self.rename(st)
            self.lane.driver_action({"kind": "restore_name"}, st)
            return {"task": "i1", "success": True}

        self.lane.run_task = renames_and_restores
        with self.stubbed():
            self.lane.run()
            self.assertEqual(self.rs["renames"], {})
            run.end_of_run_sweep(self.rs)
        self.assertEqual(self.fake.names["pid-lane-a"], "lane-a")


if __name__ == "__main__":
    unittest.main()
