"""`requeue` must not delete a branch that has work on it unless told to, and `handoff` must not run without an
owner. Run: python3 -m unittest discover -s scripts/feedback -p 'test_*.py'"""

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

ORPHANS = {  # handed-off items whose owner is gone, with live branches (#1547, #1529, #1890, #2126; 2026-10-30)
    "app-01M5N5WPHBR935T6SXFE1F31E1": "6ce4198f" + "0" * 32,
    "call-strip-in-chat-column-2026-10-23": "c298bff5" + "0" * 32,
    "relay-host-disk-guard-2026-10-26": "5e0ca586" + "0" * 32,
    "link-wrap-hints-revert-2026-10-27": "09b0929d" + "0" * 32,
}


def no_client(*_a, **_k):
    raise AssertionError("a refused handoff must not build the loop's clients")


class GuardTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        env = mock.patch.dict(os.environ, {"FEEDBACK_STATE_DIR": self.tmp.name}, clear=False)
        env.start()
        self.addCleanup(env.stop)
        self.cfg = fp.Config.from_env()
        self.store = fp.Store(self.cfg)
        self.live: dict[str, str] = {}
        loop = fp.Loop.__new__(fp.Loop)
        loop.cfg, loop.store, loop.gh = self.cfg, self.store, mock.Mock()
        loop.gh.branch_head.side_effect = lambda b: self.live.get(b)
        loop.gh.issue.return_value = {"state": "open"}
        loop.tier = lambda info: "normal"
        loop.tester_label = lambda info, tier: "tester"
        loop.tester_key = lambda info: "tester"
        loop.pick_target = lambda info: "mac"
        self.loop = loop

    def put(self, fid: str, **st) -> Path:
        d = self.store.item_dir(fid)
        d.mkdir(parents=True, exist_ok=True)
        (d / "submission.json").write_text(json.dumps({"comment": "x"}))
        self.store.save_state(fid, {"id": fid, "created": "2026-10-23T12:00:00Z", "issue": 1529, **st})
        return d / "state.json"

    def run_cli(self, *args: str, loop_factory=None) -> tuple[int, str]:
        out = io.StringIO()
        with mock.patch.object(fp, "Loop", side_effect=loop_factory or (lambda cfg: self.loop)), contextlib.redirect_stdout(out):
            code = fp.main(["feedback_poller.py", *args])
        return code, out.getvalue()

    def log(self) -> Path:
        return Path(self.tmp.name) / "log.jsonl"

    def assert_no_github_writes(self):
        gh = self.loop.gh
        for m in (gh.delete_branch, gh.comment, gh.reopen_issue, gh.set_labels):
            m.assert_not_called()

    # ---- requeue ------------------------------------------------------------------------------

    def test_requeue_refuses_each_branch_with_work_and_names_its_head(self):
        before = {}
        for fid, head in ORPHANS.items():
            self.live[f"feedback/{fid}"] = head
            path = self.put(fid, status="handed_off", handed_off_to="bc-c7641a47", branch=f"feedback/{fid}", branch_head=head)
            before[path] = path.read_text()

        code, out = self.run_cli("requeue", *ORPHANS)

        self.assertEqual(code, 2)
        for fid, head in ORPHANS.items():
            self.assertIn(f"{fid}: refused — requeue would delete feedback/{fid} at {head[:9]} (handed_off, owner bc-c7641a47)", out)
            self.assertIn(f"`retire {fid} <note>`", out)
        self.assertIn("--discard-branch", out)
        self.assertIn("nothing requeued", out)
        for path, text in before.items():
            self.assertEqual(path.read_text(), text)
        self.assertFalse(self.log().exists())
        self.assert_no_github_writes()

    def test_one_refusal_leaves_the_whole_batch_untouched(self):
        fid, head = "call-strip-in-chat-column-2026-10-23", ORPHANS["call-strip-in-chat-column-2026-10-23"]
        self.live[f"feedback/{fid}"] = head
        guarded = self.put(fid, status="handed_off", branch_head=head)
        clean = self.put("no-branch-item", status="failed")
        texts = (guarded.read_text(), clean.read_text())

        code, out = self.run_cli("requeue", "no-branch-item", fid)

        self.assertEqual(code, 2)
        self.assertEqual((guarded.read_text(), clean.read_text()), texts)
        self.assertNotIn("no-branch-item: queued", out)
        self.assert_no_github_writes()

    def test_lookup_failure_is_a_refusal(self):
        self.put("x", status="failed")
        self.loop.gh.branch_head.side_effect = fp.HttpError(502, "bad gateway", "https://api.github.com/x")
        code, out = self.run_cli("requeue", "x", "--discard-branch")
        self.assertEqual(code, 2)
        self.assertIn("could not check whether feedback/x exists (HTTP 502); nothing deleted", out)
        self.assertEqual(self.store.state("x")["status"], "failed")
        self.assert_no_github_writes()

    def test_discard_branch_is_explicit_and_records_the_head(self):
        fid, head = "relay-host-disk-guard-2026-10-26", ORPHANS["relay-host-disk-guard-2026-10-26"]
        self.live[f"feedback/{fid}"] = head
        self.put(fid, status="handed_off", branch_head=head)

        code, out = self.run_cli("requeue", fid, "--discard-branch")

        self.assertEqual(code, 0, out)
        self.assertEqual(self.store.state(fid)["status"], "queued")
        self.loop.gh.delete_branch.assert_called_once_with(f"feedback/{fid}")
        self.assertIn(f"branch feedback/{fid} at {head[:9]} discarded", out)
        ev = [json.loads(l) for l in self.log().read_text().splitlines()]
        self.assertEqual(ev[-1]["event"], "requeued")
        self.assertEqual(ev[-1]["discarded_branch_head"], head)

    def test_regressed_fix_whose_branch_is_gone_still_requeues(self):
        # A merged item keeps branch_head after the merge deleted the branch: nothing is left to lose.
        self.put("shipped-then-regressed", status="shipped", branch_head="aaaa1111" + "0" * 32, commit="bbbb2222" + "0" * 32)
        self.loop.gh.issue.return_value = {"state": "closed"}

        code, out = self.run_cli("requeue", "shipped-then-regressed")

        self.assertEqual(code, 0, out)
        self.assertEqual(self.store.state("shipped-then-regressed")["status"], "queued")
        self.assertIn("shipped-then-regressed: queued (normal, tester)\n", out)
        self.assertIsNone(json.loads(self.log().read_text().splitlines()[-1])["discarded_branch_head"])
        self.loop.gh.reopen_issue.assert_called_once()

    # ---- handoff ------------------------------------------------------------------------------

    def test_handoff_without_an_owner_is_refused(self):
        path = self.put("AMS42DpgMCxxMVQf7AqDfIM", status="failed", issue=959)
        before = path.read_text()
        for args in (("handoff", "AMS42DpgMCxxMVQf7AqDfIM"), ("handoff", "AMS42DpgMCxxMVQf7AqDfIM", "  ")):
            with self.subTest(args=args):
                code, out = self.run_cli(*args, loop_factory=no_client)
                self.assertEqual(code, 2)
                self.assertIn("AMS42DpgMCxxMVQf7AqDfIM: refused — handoff needs an owner: `handoff <id> bc-<agent id>`", out)
                self.assertIn("the item stays failed", out)
                self.assertEqual(path.read_text(), before)
        self.assertFalse(self.log().exists())

    def test_handoff_refusal_names_the_owner_it_protected(self):
        path = self.put("owned", status="handed_off", handed_off_to="bc-c7641a47")
        before = path.read_text()
        code, out = self.run_cli("handoff", "owned", loop_factory=no_client)
        self.assertEqual(code, 2)
        self.assertIn("the item stays handed_off, owner bc-c7641a47", out)
        self.assertEqual(path.read_text(), before)

    def test_handoff_with_an_owner_still_hands_off(self):
        self.put("owned", status="failed")
        self.loop.hand_off = mock.Mock()
        code, out = self.run_cli("handoff", "owned", "bc-12345678")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.loop.hand_off.call_args.args[2], "bc-12345678")


if __name__ == "__main__":
    unittest.main()
