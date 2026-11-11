"""Hermetic regressions for the actual feedback controller; no real service/config/keys."""
import copy
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import patch


def guard(event, args):
    if event.startswith("socket.") or event in {"subprocess.Popen", "os.system"}:
        raise RuntimeError("External effects are forbidden in feedback state tests")
    if event == "open" and args and isinstance(args[0], (str, bytes)):
        path = Path(args[0].decode() if isinstance(args[0], bytes) else args[0])
        if (any(part.lower().startswith(".env") or part in {".aws", ".ssh"} for part in path.parts)
                or str(path).startswith(("/etc/arbos", "/var/lib/arbos-"))):
            raise RuntimeError("Live configuration/credential/state access is forbidden")


sys.addaudithook(guard)


def forbidden_crypto(*_args, **_kwargs):
    raise RuntimeError("Cryptographic key/auth operations are outside this pure-controller fixture")


# This script's controllers do not use JWT construction. Keep that unavailable
# dependency explicit and fail if a test accidentally enters the auth boundary.
for name in ("cryptography", "cryptography.hazmat", "cryptography.hazmat.primitives",
             "cryptography.hazmat.primitives.hashes", "cryptography.hazmat.primitives.serialization",
             "cryptography.hazmat.primitives.asymmetric", "cryptography.hazmat.primitives.asymmetric.ec",
             "cryptography.hazmat.primitives.asymmetric.utils"):
    module = ModuleType(name)
    module.__path__ = []
    sys.modules[name] = module
    if "." in name:
        parent, child = name.rsplit(".", 1)
        setattr(sys.modules[parent], child, module)
sys.modules["cryptography.hazmat.primitives.asymmetric.utils"].decode_dss_signature = forbidden_crypto

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
spec = importlib.util.spec_from_file_location("feedback_poller_under_test", HERE / "feedback_poller.py")
poller = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = poller
spec.loader.exec_module(poller)

HEAD, NEW, BASE, TREE, MERGE = (value * 40 for value in "abcde")
FID = "synthetic-outside"
WHEN = "2026-11-09T00:00:00Z"


class FakeGitHub:
    def __init__(self, states):
        self.states = states
        self.merges, self.comments, self.labels, self.deleted = [], [], [], []
        self.tree_contains_members = True
        self.already_carried = False
        self.heads = {"main": BASE, **{st["branch"]: st["branch_head"] for st in states.values()}}

    def branch_head(self, branch):
        return self.heads.get(branch)

    def contains(self, ref, sha):
        return self.tree_contains_members

    def compare(self, base, head):
        if self.already_carried and base == "main":
            return {"status": "behind", "commits": []}
        return {"status": "ahead", "commits": [self.commit(st["branch_head"], fid)
                for fid, st in self.states.items()]}

    def repo_api(self, method, path):
        if method == "GET" and path.startswith("/commits?sha="):
            return [self.commit(HEAD)]
        raise AssertionError("Unexpected synthetic API observation")

    def commits_since(self, since):
        return [{**self.commit(MERGE), "parents": [{"sha": BASE}, {"sha": HEAD}]}]

    def commit(self, sha, fid=FID):
        return {"sha": sha, "html_url": "https://example.invalid/commit/" + sha,
                "commit": {"message": "fix [feedback " + fid + "]", "committer": {"date": WHEN}},
                "files": [{"filename": "docs/synthetic.md"}], "parents": []}

    def merge_into_main(self, head, message):
        self.merges.append((head, message))
        if self.already_carried:
            return {}  # GitHub's existing 204 response: this head is already on main.
        return {**self.commit(MERGE), "parents": [{"sha": BASE}, {"sha": head}]}

    def ensure_label(self, *args):
        self.labels.append(args)

    def set_labels(self, *args):
        self.labels.append(args)

    def comment(self, *args):
        self.comments.append(args)

    def delete_branch(self, branch):
        self.deleted.append(branch)


class InterruptedAttempt(Exception):
    pass


class FeedbackFixture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="arbos-feedback-state-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.cfg = SimpleNamespace(state_dir=self.root, github_repo="fixture/fixture", tester_max_wait_min=30,
                                   rust_gate_ssh="fixture-never-used", relay_url="", relay_admin_token="")
        self.loop = poller.Loop.__new__(poller.Loop)
        self.loop.cfg = self.cfg
        self.loop.store = poller.Store(self.cfg)
        self.states = {}
        self.loop.gh = FakeGitHub(self.states)
        self.closed, self.requests = [], []
        self.loop.close_fixed = lambda fid, st: self.closed.append((fid, copy.deepcopy(st)))
        self.loop.request_deploys = lambda fid, kinds, sha: self.requests.append((fid, list(kinds), sha))
        # Graph observation only; use the actual deploy_kinds decision with synthetic manifests.
        self.loop.deploy_crate_dirs = lambda refresh=False: {"agentd": ["crates/arbos-agentd/"], "relay": ["crates/arbos-relay/"]}

    def item(self, fid=FID, **overrides):
        directory = self.loop.store.item_dir(fid)
        directory.mkdir()
        (directory / "submission.json").write_text(json.dumps({"source": "app", "handle": "outside",
                                                               "created": WHEN, "comment": "button alignment"}))
        st = {"id": fid, "status": "gating", "branch": "feedback/" + fid, "branch_head": HEAD,
              "tester_key": "outside", "tier": "tester", "issue": 1, "issue_url": "https://example.invalid/issue/1",
              "files": ["docs/synthetic.md"], "created": WHEN}
        st.update(overrides)
        self.loop.store.save_state(fid, st)
        self.states[fid] = copy.deepcopy(st)
        self.loop.gh.heads[st["branch"]] = st["branch_head"]
        return st

    def active(self):
        return {"id": "synthetic-tree", "base": BASE, "tree": TREE,
                "members": {fid: st["branch_head"] for fid, st in self.states.items()},
                "linux": "ok", "mac": "ok", "ios": "ok", "linux_run": "https://example.invalid/run/1"}

    def stop_after_first_save(self):
        save = self.loop.store.save_state

        def interrupted(fid, st):
            save(fid, st)
            raise InterruptedAttempt()

        self.loop.store.save_state = interrupted
        return save


class MergeAdmissionTests(FeedbackFixture):
    def test_prepared_tree_cannot_merge_an_unapproved_outside_member(self):
        self.item()
        self.loop.land_tree(self.active(), True)
        self.assertEqual(self.loop.gh.merges, [], "prepared tree bypassed the per-item approval gate")

    def test_prepared_tree_cannot_merge_an_operator_held_member(self):
        self.item(approved_by_jacob=WHEN, do_not_merge="operator hold")
        self.loop.land_tree(self.active(), True)
        self.assertEqual(self.loop.gh.merges, [], "prepared tree bypassed the operator hold")

    def test_prepared_tree_cannot_merge_after_a_member_branch_moves(self):
        st = self.item(approved_by_jacob=WHEN)
        active = self.active()
        self.loop.gh.heads[st["branch"]] = NEW
        self.loop.land_tree(active, True)
        self.assertEqual(self.loop.gh.merges, [], "prepared tree merged a covered head after its branch moved")

    def test_all_members_are_checked_before_an_atomic_tree_merge(self):
        self.item(approved_by_jacob=WHEN)
        self.item("synthetic-second")
        self.loop.land_tree(self.active(), True)
        self.assertEqual(self.loop.gh.merges, [])

    def test_approved_current_prepared_tree_still_lands_the_covered_commit(self):
        self.item(approved_by_jacob=WHEN)
        self.loop.land_tree(self.active(), True)
        self.assertEqual([head for head, _ in self.loop.gh.merges], [TREE])
        saved = self.loop.store.state(FID)
        self.assertEqual(saved["status"], "pushed")
        self.assertEqual(saved["commit"], HEAD)
        self.assertEqual(saved["rust_gate"]["head"], HEAD)

    def test_red_prepared_tree_never_attempts_a_merge(self):
        self.item(approved_by_jacob=WHEN)
        active = self.active()
        active["linux"] = "fail"
        self.loop.land_tree(active, False)
        self.assertEqual(self.loop.gh.merges, [])

    def test_lifting_the_global_gate_does_not_approve_an_already_held_item(self):
        st = self.item(awaiting={"ready": True}, expects_rust=False, expects_ios=False)
        (self.root / self.loop.APPROVAL_LIFTED_FILE).write_text('{"reason":"synthetic operator action"}')
        self.loop.gate(FID, st)
        self.assertEqual(self.loop.gh.merges, [], "a global lift released an already held item without per-item approval")

    def test_a_missing_live_branch_does_not_merge_an_old_recorded_head(self):
        st = self.item(approved_by_jacob=WHEN, expects_rust=False, expects_ios=False)
        self.loop.gh.heads[st["branch"]] = None
        self.loop.gate(FID, st)
        self.assertEqual(self.loop.gh.merges, [])

    def test_truthy_nonboolean_gate_results_are_not_a_green_verdict(self):
        st = self.item(approved_by_jacob=WHEN, expects_rust=True, expects_ios=False,
                       files=["crates/arbos-core/src/synthetic.rs"], rust_gate={"head": HEAD, "ok": "false"})
        self.loop.gate(FID, st)
        self.assertEqual(self.loop.gh.merges, [])

    def test_prepared_tree_must_actually_contain_the_claimed_member_head(self):
        self.item(approved_by_jacob=WHEN)
        self.loop.gh.tree_contains_members = False
        self.loop.land_tree(self.active(), True)
        self.assertEqual(self.loop.gh.merges, [], "gate metadata alone cannot claim an unrelated SHA was covered")

    def test_normal_approved_current_head_still_merges(self):
        st = self.item(approved_by_jacob=WHEN, expects_rust=False, expects_ios=False)
        self.loop.gate(FID, st)
        self.assertEqual([head for head, _ in self.loop.gh.merges], [HEAD])

    def test_normal_outside_head_still_holds_for_per_item_approval(self):
        st = self.item(expects_rust=False, expects_ios=False)
        self.loop.gate(FID, st)
        self.assertEqual(self.loop.gh.merges, [])
        self.assertEqual(self.loop.store.state(FID)["status"], "awaiting_jacob")

    def test_deleted_shared_branch_already_on_main_is_still_tracked(self):
        st = self.item(approved_by_jacob=WHEN, expects_rust=False, expects_ios=False)
        self.loop.gh.heads[st["branch"]] = None
        self.loop.gh.already_carried = True
        self.loop.gate(FID, st)
        self.assertEqual(self.loop.store.state(FID)["status"], "pushed")
        self.assertEqual(self.loop.store.state(FID)["commit"], HEAD)


class DeliveryPersistenceTests(FeedbackFixture):
    def test_first_shipping_save_retains_pending_deploy_and_device_evidence(self):
        st = self.item(status="pushed", commit=HEAD, files=["voice/worker.py"], release_done=True,
                       expects_ios=False, expects_ci=True, release_at=WHEN)
        self.stop_after_first_save()
        with self.assertRaises(InterruptedAttempt):
            self.loop.ship(FID, st)
        saved = poller.Store(self.cfg).state(FID)
        self.assertEqual(saved.get("pending_deploy"), ["voice"], "terminal state persisted without its deployment intent")
        self.assertEqual(saved.get("device_evidence"), "not needed")

    def test_first_shipping_save_keeps_unprocessed_testflight_nonterminal(self):
        st = self.item(status="pushed", commit=HEAD, files=["apps/ios/ChatView.swift"], expects_ios=True,
                       expects_ci=False, ios_done=True, testflight_build="123456789012", ios_uploaded_at=WHEN)
        self.stop_after_first_save()
        with self.assertRaises(InterruptedAttempt):
            self.loop.ship(FID, st, partial=True)
        saved = poller.Store(self.cfg).state(FID)
        self.assertEqual(saved["status"], "pushed", "unprocessed iOS artifact became terminal before restart")

    def test_partial_ios_and_deploy_both_remain_required(self):
        st = self.item(status="pushed", commit=HEAD, files=["voice/worker.py", "apps/ios/CallView.swift"],
                       expects_ios=True, expects_ci=True, release_done=True, ios_done=True,
                       testflight_build="123456789012", ios_uploaded_at=WHEN)
        self.loop.ship(FID, st, partial=True)
        saved = self.loop.store.state(FID)
        self.assertEqual(saved["status"], "pushed")
        self.assertEqual(saved["pending_deploy"], ["voice"])
        self.loop.mark_deployed(FID, saved, "synthetic deployment proof")
        self.assertEqual(self.loop.store.state(FID)["status"], "pushed")
        self.assertEqual(self.closed, [], "deploy completion cannot substitute for an installable iOS build")

    def test_installable_nondeploy_artifact_still_completes(self):
        st = self.item(status="pushed", commit=HEAD, files=["apps/ios/ChatView.swift"], expects_ios=True,
                       expects_ci=False, ios_done=True, testflight_build="123456789012", testflight_ready_at=WHEN)
        self.loop.ship(FID, st)
        saved = self.loop.store.state(FID)
        self.assertEqual(saved["status"], "shipped")
        self.assertEqual(saved["device_evidence"], "not needed")
        self.assertEqual(len(self.closed), 1)

    def test_retry_after_interrupted_record_keeps_deploy_intent_and_can_complete(self):
        st = self.item(status="pushed", commit=HEAD, files=["voice/worker.py"], expects_ios=False,
                       expects_ci=True, release_done=True, release_at=WHEN)
        save = self.stop_after_first_save()
        with self.assertRaises(InterruptedAttempt):
            self.loop.ship(FID, st)
        self.loop.store.save_state = save
        saved = self.loop.store.state(FID)
        self.assertEqual(saved.get("pending_deploy"), ["voice"])
        self.loop.ship(FID, saved)
        self.assertEqual(self.requests[-1], (FID, ["voice"], HEAD))
        self.loop.mark_deployed(FID, self.loop.store.state(FID), "synthetic same-commit deployment proof")
        complete = self.loop.store.state(FID)
        self.assertNotIn("pending_deploy", complete)
        self.assertEqual(complete["deployed_commit"], HEAD)
        self.assertEqual(len(self.closed), 1)

    def test_a_processed_partial_build_does_not_requeue_an_already_proven_deploy(self):
        st = self.item(status="pushed", commit=HEAD, files=["voice/worker.py", "apps/ios/CallView.swift"],
                       expects_ios=True, expects_ci=True, release_done=True, ios_done=True,
                       testflight_build="123456789012", ios_uploaded_at=WHEN)
        self.loop.ship(FID, st, partial=True)
        self.loop.mark_deployed(FID, self.loop.store.state(FID), "synthetic same-commit deployment proof")
        ready = self.loop.store.state(FID)
        ready["testflight_ready_at"] = WHEN
        self.loop.ship(FID, ready)
        self.assertEqual(self.loop.store.state(FID)["status"], "shipped")
        self.assertNotIn("pending_deploy", self.loop.store.state(FID))
        self.assertEqual(len(self.requests), 1)
        self.assertEqual(len(self.closed), 1)

    def test_deploy_completion_first_save_contains_its_proof_not_an_empty_queue(self):
        self.item(status="shipped", commit=HEAD, pending_deploy=["voice"], device_evidence="not needed",
                  expects_ios=False, expects_ci=True, release_done=True)
        self.cfg.relay_url, self.cfg.relay_admin_token = "https://example.invalid", "synthetic-unusable-token"
        self.loop.gh.repo_api = lambda method, path: {"status": "identical"}
        self.stop_after_first_save()
        with patch.object(poller, "http_json", return_value={"voice": {"worker_sha": HEAD}}), \
             patch.object(poller, "AGENTD_DEPLOYED_SHA_FILE", str(self.root / "absent-agentd")), \
             patch.object(poller, "AGENTD_SKIP_FILE", str(self.root / "absent-skip")), \
             patch.object(poller, "RELAY_DEPLOYED_SHA_FILE", str(self.root / "absent-relay")):
            with self.assertRaises(InterruptedAttempt):
                self.loop.watch_deploys()
        saved = self.loop.store.state(FID)
        self.assertEqual(saved.get("deployed_commit"), HEAD)
        self.assertTrue(saved.get("deployed_note"))
        self.assertNotIn("pending_deploy", saved)


if __name__ == "__main__":
    unittest.main()
