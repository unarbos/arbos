#!/usr/bin/env python3
"""The gate's lane hold (#1899): `lane-hold <lane> on` is refused for a lane the gate has no footprint
for, a `mac` hold still keeps unpinned Mac items out of the gate, and a hold left in the cursor for an
unenforceable lane reads as NOT ENFORCED instead of listing the Mac items as that lane's.

State lives in a temp dir; no network, no App Store Connect, no GitHub.

    python3 -m unittest discover -s scripts/feedback -p 'test_lane_hold.py'
"""
import contextlib
import io
import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import feedback_poller as fp  # noqa: E402 - the poller lives beside this test

# `lane-hold` runs behind main()'s credential check; the values are never used (no call leaves the box).
ENV = {"ASC_KEY_ID": "key", "ASC_ISSUER_ID": "issuer", "ASC_KEY_P8_B64": "eA==",
       "CURSOR_API_KEY": "cursor", "GITHUB_TOKEN": "gh"}


class LaneHoldTest(unittest.TestCase):
    def setUp(self) -> None:
        state = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, state, True)
        env = mock.patch.dict(os.environ, {**ENV, "FEEDBACK_STATE_DIR": str(state)})
        env.start()
        self.addCleanup(env.stop)
        self.loop = fp.Loop(fp.Config.from_env())

    def gating(self, fid: str, files: list[str], created: str, **extra: object) -> None:
        self.loop.store.item_dir(fid).mkdir(parents=True, exist_ok=True)
        self.loop.store.save_state(fid, {"id": fid, "created": created, "status": "gating",
                                         "expects_rust": True, "branch_head": f"head-{fid}",
                                         "files": files, **extra})

    @staticmethod
    def run_cli(*args: str) -> tuple[int, str]:
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rc = fp.main(["feedback_poller.py", *args])
        return rc, out.getvalue()

    def test_hold_for_an_unenforceable_lane_is_refused(self):
        rc, out = self.run_cli("lane-hold", "ios", "on", "iOS release in review")
        self.assertEqual(rc, 2)
        self.assertIn("nothing in the gate enforces a 'ios' hold", out)
        self.assertNotIn("lane_hold", self.loop.store.cursor())

    def test_mac_hold_keeps_unpinned_mac_items_out_of_the_gate(self):
        self.gating("mac-item", ["crates/arbos-mac/src/ui/chat.rs"], "2026-11-01T00:00:00Z")
        self.gating("mac-manifest", ["Cargo.lock"], "2026-11-01T00:01:00Z")
        self.gating("mac-pinned", ["crates/arbos-mac/src/main.rs"], "2026-11-01T00:02:00Z", gate_pinned=True)
        self.gating("ios-item", ["apps/ios/Arbos/ChatView.swift"], "2026-11-01T00:03:00Z")
        rc, out = self.run_cli("lane-hold", "mac", "on", "0.6.370 hotfix")
        self.assertEqual(rc, 0)
        self.assertIn("lane-hold mac: ON", out)
        self.assertEqual([fid for fid, _ in self.loop.rust_gate_ready()], ["mac-pinned", "ios-item"])

    def test_a_hold_written_before_the_check_holds_nothing_and_says_so(self):
        cur = self.loop.store.cursor()
        cur["lane_hold"] = {"ios": {"since": "2026-10-26T15:27:00Z", "reason": "iOS release in review"}}
        self.loop.store.save_cursor(cur)
        self.gating("mac-item", ["crates/arbos-mac/src/ui/chat.rs"], "2026-11-01T00:00:00Z")
        self.gating("ios-item", ["apps/ios/Arbos/ChatView.swift"], "2026-11-01T00:01:00Z")
        self.assertEqual([fid for fid, _ in self.loop.rust_gate_ready()], ["mac-item", "ios-item"])
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.loop.print_gate_queue(self.loop.store.cursor())
        self.assertIn("lane hold ios since 2026-10-26T15:27:00Z (iOS release in review): NOT ENFORCED", out.getvalue())
        self.assertNotIn("item(s) wait", out.getvalue())
        self.assertIn("NOT ENFORCED", self.run_cli("lane-hold", "ios", "status")[1])


if __name__ == "__main__":
    unittest.main()
