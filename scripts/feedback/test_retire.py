"""`retire`: a handed-off item whose owner is gone ends, its solo slot is released, and neither the issue nor the
branch is touched. Run: python3 -m unittest discover -s scripts/feedback -p 'test_*.py'"""

import contextlib
import io
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import feedback_poller as fp  # noqa: E402

FID = "app-01M5N5WPHBR935T6SXFE1F31E1"
HEAD = "6ce4198f" + "0" * 32
NEW_HEAD = "7d0e2a11" + "0" * 32


def handed_off(fid: str = FID, **over) -> dict:
    """The shape the relay host reported for #1547 and #1529: with its owner, solo slot reserved, branch pushed."""
    st = {"id": fid, "created": "2026-10-23T12:00:00Z", "status": "handed_off", "handed_off_to": "bc-c7641a47",
          "handed_off_at": "2026-10-23T12:05:00Z", "issue": 1547, "branch": f"feedback/{fid}", "branch_head": HEAD,
          "gated_head": HEAD, "files": ["crates/arbos-mac/src/app.rs"], "expects_rust": True, "follow_pushes": True,
          "track_commit": True, "solo_next": True, "sibling_conflicts": 1, "main_conflicts": 2, "main_moved_noted": "b025353b"}
    st.update(over)
    return st


def no_network(*_a, **_k):
    raise AssertionError("retire must not build a GitHub / Cursor / App Store client or open a connection")


class RetireTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        env = mock.patch.dict(os.environ, {"FEEDBACK_STATE_DIR": self.tmp.name}, clear=False)
        env.start()
        self.addCleanup(env.stop)
        self.cfg = fp.Config.from_env()
        self.store = fp.Store(self.cfg)

    def put(self, st: dict) -> None:
        self.store.item_dir(st["id"]).mkdir(parents=True, exist_ok=True)
        self.store.save_state(st["id"], st)

    def retire(self, fid: str, note: str = "owner bc-c7641a47 is gone") -> tuple[int, str]:
        out = io.StringIO()
        with contextlib.ExitStack() as stack:
            for name in ("GitHub", "Cursor", "Asc", "Loop"):
                stack.enter_context(mock.patch.object(fp, name, side_effect=no_network))
            stack.enter_context(mock.patch.object(fp.urllib.request, "urlopen", side_effect=no_network))
            stack.enter_context(contextlib.redirect_stdout(out))
            code = fp.main(["feedback_poller.py", "retire", fid, *note.split()])
        return code, out.getvalue()

    def loop(self) -> fp.Loop:
        loop = fp.Loop.__new__(fp.Loop)
        loop.cfg, loop.store, loop.gh = self.cfg, self.store, mock.Mock()
        return loop

    def events(self) -> list[dict]:
        p = Path(self.tmp.name) / "log.jsonl"
        return [json.loads(l) for l in p.read_text().splitlines()] if p.is_file() else []

    def test_handed_off_item_ends_and_its_slot_is_freed(self):
        self.put(handed_off(gate_pinned=True, pinned_by="jacob", pinned_at="2026-10-29T10:00:00Z"))
        queue = io.StringIO()
        with contextlib.redirect_stdout(queue):
            self.loop().print_gate_queue(self.store.cursor())
        self.assertIn("solo slot reserved", queue.getvalue())

        code, out = self.retire(FID)

        self.assertEqual(code, 0, out)
        st = self.store.state(FID)
        self.assertEqual(st["status"], "retired")
        self.assertIn("retired", fp.TERMINAL)
        for k in ("solo_next", "gate_pinned", "pinned_by", "pinned_at", "main_moved_noted", "sibling_conflicts", "main_conflicts"):
            self.assertNotIn(k, st)
        r = st["retired"]
        self.assertEqual((r["owner"], r["note"], r["branch_head"]), ("bc-c7641a47", "owner bc-c7641a47 is gone", HEAD))
        self.assertTrue(r["solo_next"] and r["gate_pinned"])
        self.assertEqual((r["sibling_conflicts"], r["main_conflicts"]), (1, 2))
        self.assertEqual([e["event"] for e in self.events()], ["retired"])
        self.assertIn("solo slot released", out)
        queue = io.StringIO()
        with contextlib.redirect_stdout(queue):
            self.loop().print_gate_queue(self.store.cursor())
        self.assertNotIn("solo slot reserved", queue.getvalue())
        self.assertNotIn(FID, queue.getvalue())

    def test_issue_and_branch_are_untouched(self):
        self.put(handed_off())
        code, out = self.retire(FID)  # every GitHub client and urlopen raise inside retire()
        self.assertEqual(code, 0, out)
        st = self.store.state(FID)
        self.assertEqual((st["issue"], st["branch"], st["branch_head"]), (1547, f"feedback/{FID}", HEAD))
        self.assertIn(f"branch feedback/{FID} kept at {HEAD[:9]}", out)
        self.assertIn("issue #1547 untouched", out)

    def test_later_push_to_the_dead_branch_does_not_resurrect_a_solo_slot(self):
        ctl = "app-control-still-owned"
        self.put(handed_off())
        self.put(handed_off(ctl, created="2026-10-24T12:00:00Z", issue=1529))
        self.assertEqual(self.retire(FID)[0], 0)
        loop = self.loop()
        gh = loop.gh
        gh.repo_api.return_value = []
        gh.branch_head.return_value = "c0ffee00" + "0" * 32
        gh.commits_since.return_value = []
        gh.feedback_refs.return_value = {f"feedback/{FID}": NEW_HEAD, f"feedback/{ctl}": NEW_HEAD}
        gh.compare.return_value = {"status": "ahead", "files": [{"filename": "crates/arbos-mac/src/app.rs"}],
                                   "commits": [{"sha": NEW_HEAD, "commit": {"message": f"merge main [feedback {FID}] [feedback {ctl}]"}}]}

        loop.watch_handoffs()

        self.assertEqual(self.store.state(ctl)["status"], "gating")  # the same push on a live item does re-gate
        st = self.store.state(FID)
        self.assertEqual(st["status"], "retired")
        self.assertEqual(st["branch_head"], HEAD)
        self.assertNotIn("solo_next", st)
        self.assertEqual([c.args[1] for c in gh.compare.call_args_list], [NEW_HEAD])
        self.assertNotIn(FID, [f for f, _ in loop.rust_gate_ready()])
        loop.advance = mock.Mock()
        loop.advance_all()
        self.assertNotIn(FID, [c.args[0] for c in loop.advance.call_args_list])

    def test_main_moved_note_stops(self):
        self.put(handed_off())
        loop = self.loop()
        loop.announce_main_moved("aaaa1111")
        self.assertEqual(loop.gh.comment.call_count, 1)
        self.assertEqual(self.retire(FID)[0], 0)
        loop.announce_main_moved("bbbb2222")
        self.assertEqual(loop.gh.comment.call_count, 1)

    def test_gating_item_is_refused_and_left_as_it_is(self):
        self.put(handed_off(status="gating"))
        before = (self.store.item_dir(FID) / "state.json").read_text()
        code, out = self.retire(FID)
        self.assertEqual(code, 2)
        self.assertIn("refused — gating", out)
        self.assertIn(f"park {FID}", out)
        self.assertEqual((self.store.item_dir(FID) / "state.json").read_text(), before)
        self.assertEqual(self.events(), [])

    def test_member_of_the_active_gate_run_is_refused(self):
        self.put(handed_off())
        cur = self.store.cursor()
        cur["rust_gate"] = {"id": "tree-20261030T170000-2", "members": {FID: HEAD}}
        self.store.save_cursor(cur)
        code, out = self.retire(FID)
        self.assertEqual(code, 2)
        self.assertIn("active gate run tree-20261030T170000-2", out)
        self.assertEqual(self.store.state(FID)["status"], "handed_off")

    def test_other_states_are_refused(self):
        for status in ("pushed", "shipped", "queued", "running", "awaiting_jacob", "retired"):
            with self.subTest(status=status):
                self.put(handed_off(status=status))
                code, out = self.retire(FID)
                self.assertEqual(code, 2)
                self.assertIn(f"status {status}", out)
                self.assertEqual(self.store.state(FID)["status"], status)
        self.assertEqual(self.retire("no-such-item")[0], 2)

    def test_a_note_is_required(self):
        self.put(handed_off())
        self.assertEqual(self.retire(FID, note="")[0], 2)
        self.assertEqual(self.store.state(FID)["status"], "handed_off")

    def test_owner_field_holding_a_label_is_recorded_not_taken_for_an_owner(self):
        # #959: `handoff <id>` without an owner left its default label in handed_off_to; no branch was pushed.
        st = handed_off("AMS42DpgMCxxMVQf7AqDfIM", handed_off_to="in-progress", issue=959, solo_next=False)
        for k in ("branch", "branch_head", "gated_head"):
            st.pop(k)
        self.put(st)
        code, out = self.retire("AMS42DpgMCxxMVQf7AqDfIM", note="no live owner")
        self.assertEqual(code, 0, out)
        r = self.store.state("AMS42DpgMCxxMVQf7AqDfIM")["retired"]
        self.assertIsNone(r["owner"])
        self.assertEqual(r["owner_field"], "in-progress")
        self.assertIn("handed_off_to is 'in-progress', not a bc- id", out)
        self.assertNotIn("kept at", out)


if __name__ == "__main__":
    unittest.main()
