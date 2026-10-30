"""The release-queue stall guard: watch_release_queue_stall() in feedback_poller.py.

    python3 -m unittest discover -s scripts/feedback -p 'test_*.py' -v

The merge hold (/usr/local/bin/arbos-release-serialize, on the relay host and not in this repository) has no
timeout, so two things freeze every merge in the project: a release that fails or is cancelled after its tag is
cut, and a released item whose gate stays red. These tests cover both shapes, a healthy queue that must stay
silent, and the repeat suppression that keeps one stall from shouting once per 2-minute tick.

The fake serves the REST shapes the guard reads (release.yml runs, releases, /releases/latest, run jobs) and
records every write, so "posted nothing" is an assertion and not a hope.
"""

from __future__ import annotations

import datetime as dt
import json
import sys
import tempfile
import types
import unittest
import urllib.parse
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import feedback_poller as fp  # noqa: E402

REPO = "unarbos/arbos"


def ago(minutes: int) -> str:
    return fp.iso(fp.utcnow() - dt.timedelta(minutes=minutes))


class FakeRepo:
    """The repository the guard reads: tag releases, their assets, and release.yml's runs."""

    def __init__(self) -> None:
        self.releases: list[dict] = []
        self.runs: list[dict] = []
        self.latest: str | None = None  # the tag GitHub serves at /releases/latest
        self.jobs: dict[int, list[dict]] = {}
        self.writes: list[tuple[str, str, object]] = []
        self.issue_seq = 7000

    # -- building the world ------------------------------------------------------------------------
    def release_run(self, tag: str, conclusion: str | None, *, concluded_min_ago: int = 30,
                    run_id: int = 0, status: str = "completed") -> dict:
        run = {"id": run_id or (900 + len(self.runs)), "workflow": "release.yml", "event": "push",
               "head_branch": tag, "head_sha": "a" * 40, "status": status, "conclusion": conclusion,
               "run_started_at": ago(concluded_min_ago + 20), "updated_at": ago(concluded_min_ago),
               "html_url": f"https://github.com/{REPO}/actions/runs/{run_id or (900 + len(self.runs))}"}
        self.runs.append(run)
        return run

    def published(self, tag: str) -> None:
        """A tag published the way scripts/release/publish-release.sh leaves it."""
        self.releases.append({"tag_name": tag, "draft": False, "prerelease": False,
                              "assets": [{"name": f"Arbos-{tag[1:]}-macos-universal.dmg"}, {"name": "appcast.xml"}]})
        self.latest = tag

    # -- the API ----------------------------------------------------------------------------------
    def api(self, method: str, path: str, json_body: dict | None = None, **_kw: object) -> object:
        url = urllib.parse.urlsplit(path)
        q = dict(urllib.parse.parse_qsl(url.query))
        p = url.path.removeprefix(f"/repos/{REPO}")
        if method != "GET":
            self.writes.append((method, p, json_body))
            if p == "/issues":
                self.issue_seq += 1
                return {"number": self.issue_seq}
            return {}
        parts = p.strip("/").split("/")
        if p == "/releases/latest":
            if not self.latest:
                raise fp.HttpError(404, "Not Found", path)
            return {"tag_name": self.latest}
        if parts == ["releases"]:
            return self.releases
        if parts[:2] == ["actions", "workflows"] and parts[3:] == ["runs"]:
            return {"workflow_runs": [r for r in self.runs
                                      if r["workflow"] == parts[2] and r["event"] == q.get("event", r["event"])]}
        if parts[:2] == ["actions", "runs"] and parts[3:] == ["jobs"]:
            return {"jobs": self.jobs.get(int(parts[2]), [])}
        if parts[0] == "labels":
            return {}
        raise AssertionError(f"unexpected GET {path}")

    # -- what the guard did -----------------------------------------------------------------------
    def issues_filed(self) -> list[dict]:
        return [body for m, p, body in self.writes if m == "POST" and p == "/issues"]

    def comments(self) -> list[str]:
        return [body["body"] for m, p, body in self.writes if m == "POST" and p.endswith("/comments")]

    def closes(self) -> list[str]:
        return [p for m, p, body in self.writes if m == "PATCH" and (body or {}).get("state") == "closed"]


class FakeGitHub(fp.GitHub):
    def __init__(self, cfg: object, repo: FakeRepo) -> None:
        super().__init__(cfg)
        self.fake = repo

    def api(self, method: str, path: str, **kw: object) -> object:
        return self.fake.api(method, path, **kw)


class StallGuardTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.state_dir = Path(self.tmp.name)
        self.cfg = types.SimpleNamespace(state_dir=self.state_dir, github_repo=REPO, github_token="t",
                                         stall_channel="issue", stall_release_after_min=5, stall_gate_after_min=20)
        self.repo = FakeRepo()
        self.loop = fp.Loop.__new__(fp.Loop)
        self.loop.cfg = self.cfg
        self.loop.store = fp.Store(self.cfg)
        self.loop.gh = FakeGitHub(self.cfg, self.repo)

    # -- the queue ------------------------------------------------------------------------------------
    def gating(self, fid: str, *, head: str, do_not_merge: str | None = None, red_min_ago: int | None = None,
               green: bool = False, issue: int | None = None) -> None:
        (self.state_dir / fid).mkdir(exist_ok=True)
        st: dict = {"id": fid, "status": "gating", "branch": f"feedback/{fid}", "branch_head": head,
                    "created": ago(600)}
        if do_not_merge:
            st["do_not_merge"] = do_not_merge
        if red_min_ago is not None:
            st["rust_gate"] = {"head": head, "ok": False, "verdict": "fail", "at": ago(red_min_ago)}
        if green:
            st["rust_gate"] = {"head": head, "ok": True, "verdict": "ok", "at": ago(5)}
        if issue:
            st["issue"] = issue
        self.loop.store.save_state(fid, st)

    def held_queue(self) -> None:
        """Three items the hold has marked do-not-merge, waiting their turn."""
        for n, fid in enumerate(("behind-one", "behind-two", "behind-three")):
            self.gating(fid, head=f"{n + 1}" * 40, do_not_merge="release serialization")

    def events(self, kind: str) -> list[dict]:
        p = self.state_dir / "log.jsonl"
        if not p.is_file():
            return []
        return [r for r in (json.loads(l) for l in p.read_text().splitlines()) if r["event"] == kind]

    def tick(self) -> None:
        self.loop.watch_release_queue_stall()

    # -- shape one: the release died after the tag was cut ------------------------------------------
    def test_failed_release_after_the_tag_was_cut_is_flagged(self) -> None:
        self.repo.published("v0.6.367")
        run = self.repo.release_run("v0.6.368", "failure", run_id=4242)
        self.repo.jobs[4242] = [{"id": 1, "conclusion": "failure", "steps": [{"name": "Publish", "conclusion": "failure"}]}]
        self.held_queue()
        with mock.patch.object(fp, "http", return_value=(200, {}, b"2026-10-30T16:20:01Z error: notarization failed\n")):
            self.tick()
        filed = self.repo.issues_filed()
        self.assertEqual(len(filed), 1)
        self.assertIn("v0.6.368", filed[0]["title"])
        self.assertEqual(filed[0]["labels"], ["release-queue-stalled"])
        self.assertIn("there is no release object for the tag", filed[0]["body"])
        self.assertIn(run["html_url"], filed[0]["body"])
        self.assertIn("error: notarization failed", filed[0]["body"], "the issue carries the lines from the run's log")
        self.assertIn("3 item(s) are marked do-not-merge", filed[0]["body"])
        self.assertEqual(len(self.events("release_queue_stalled")), 1)

    def test_cancelled_release_is_flagged_and_says_there_are_no_error_lines(self) -> None:
        self.repo.published("v0.6.367")
        self.repo.release_run("v0.6.368", "cancelled")
        self.tick()
        filed = self.repo.issues_filed()
        self.assertEqual(len(filed), 1)
        self.assertIn("cancelled", filed[0]["title"])
        self.assertIn("logged no error lines", filed[0]["body"])

    def test_a_draft_release_with_no_disk_image_is_a_stall(self) -> None:
        self.repo.published("v0.6.367")
        self.repo.releases.append({"tag_name": "v0.6.368", "draft": True, "assets": []})
        self.repo.release_run("v0.6.368", "failure")
        self.tick()
        body = self.repo.issues_filed()[0]["body"]
        self.assertIn("the release is still a draft", body)
        self.assertIn("no disk image (`.dmg`) is uploaded", body)
        self.assertIn("the stable appcast still names `v0.6.367`", body)

    def test_a_release_that_failed_after_publishing_everything_is_not_a_stall(self) -> None:
        """The hold can move on by itself: the tag is published, whatever the run's last step did."""
        self.repo.published("v0.6.368")
        self.repo.release_run("v0.6.368", "failure")
        self.held_queue()
        self.tick()
        self.assertEqual(self.repo.writes, [])

    def test_a_retry_still_running_is_not_a_stall(self) -> None:
        """release.yml's own */15 cron backstop, or a hand re-run: the queue is moving, so say nothing."""
        self.repo.published("v0.6.367")
        self.repo.release_run("v0.6.368", "failure")
        self.repo.release_run("v0.6.368", None, status="in_progress")
        self.tick()
        self.assertEqual(self.repo.writes, [])

    def test_an_older_tags_run_stuck_queued_does_not_blind_the_guard(self) -> None:
        """Only the head of the queue is judged, so a run left queued on an old tag cannot hide a live stall."""
        self.repo.published("v0.6.367")
        self.repo.release_run("v0.6.300", None, status="queued", concluded_min_ago=4000)
        self.repo.release_run("v0.6.368", "failure")
        self.held_queue()
        self.tick()
        self.assertEqual(len(self.repo.issues_filed()), 1)
        self.assertIn("v0.6.368", self.repo.issues_filed()[0]["title"])

    def test_a_release_that_failed_a_minute_ago_waits_out_its_grace_period(self) -> None:
        self.repo.published("v0.6.367")
        self.repo.release_run("v0.6.368", "failure", concluded_min_ago=1)
        self.tick()
        self.assertEqual(self.repo.writes, [])
        self.assertEqual(self.events("release_queue_stalled"), [])

    # -- shape two: the released item's gate stays red ----------------------------------------------
    def test_a_released_item_with_a_red_gate_is_flagged(self) -> None:
        self.held_queue()
        self.gating("stuck-item", head="f" * 40, red_min_ago=40, issue=2100)
        self.tick()
        filed = self.repo.issues_filed()
        self.assertEqual(len(filed), 1)
        self.assertIn("stuck-item", filed[0]["title"])
        self.assertIn("3 item(s) held behind it", filed[0]["title"])
        self.assertIn("#2100", filed[0]["body"])
        self.assertIn("A red gate never merges", filed[0]["body"])
        self.assertEqual(len(self.events("release_queue_stalled")), 1)

    def test_a_batch_red_recorded_against_the_judged_head_counts(self) -> None:
        """A bisect records `last_red` rather than the item's own verdict; the queue is just as stuck."""
        self.held_queue()
        (self.state_dir / "bisected").mkdir()
        self.loop.store.save_state("bisected", {"id": "bisected", "status": "gating", "created": ago(600),
                                                "branch_head": "e" * 40,
                                                "last_red": {"head": "e" * 40, "run": "batch-9", "at": ago(45)}})
        self.tick()
        self.assertEqual(len(self.repo.issues_filed()), 1)

    def test_a_red_gate_inside_its_grace_period_is_not_flagged_yet(self) -> None:
        """A red gate may be a flake somebody is already re-running: 20 min before anyone is told."""
        self.held_queue()
        self.gating("maybe-flake", head="f" * 40, red_min_ago=3)
        self.tick()
        self.assertEqual(self.repo.writes, [])

    def test_a_red_item_nobody_is_held_behind_is_not_a_stall(self) -> None:
        """No hold in force means no queue to freeze — the red item is its owner's business, not an alert."""
        self.gating("lonely-red", head="f" * 40, red_min_ago=99)
        self.tick()
        self.assertEqual(self.repo.writes, [])

    def test_a_hold_that_has_released_nobody_yet_is_left_to_shape_one(self) -> None:
        self.held_queue()
        self.tick()
        self.assertEqual(self.repo.writes, [])

    # -- a healthy queue must stay silent -----------------------------------------------------------
    def test_a_healthy_queue_is_never_flagged(self) -> None:
        self.repo.published("v0.6.368")
        self.repo.release_run("v0.6.368", "success")
        self.held_queue()
        self.gating("moving-along", head="f" * 40, green=True)
        for _ in range(5):
            self.tick()
        self.assertEqual(self.repo.writes, [], "a healthy queue posts nothing")
        self.assertEqual(self.events("release_queue_stalled"), [])

    def test_an_empty_queue_with_a_published_tag_is_never_flagged(self) -> None:
        self.repo.published("v0.6.368")
        self.repo.release_run("v0.6.368", "success")
        self.tick()
        self.assertEqual(self.repo.writes, [])

    # -- one stall, one flag ------------------------------------------------------------------------
    def test_one_stall_is_flagged_once_however_many_ticks_run(self) -> None:
        """The script ticks every 2 min. A stall that lasts three hours is 90 ticks and must stay one flag."""
        self.repo.published("v0.6.367")
        self.repo.release_run("v0.6.368", "failure")
        self.held_queue()
        for _ in range(90):
            self.tick()
        self.assertEqual(len(self.repo.issues_filed()), 1, "90 ticks, one issue")
        self.assertEqual(self.repo.comments(), [], "and not one comment in between")
        self.assertEqual(len(self.events("release_queue_stalled")), 1)

    def test_both_shapes_at_once_are_one_flag_each(self) -> None:
        self.repo.published("v0.6.367")
        self.repo.release_run("v0.6.368", "failure")
        self.held_queue()
        self.gating("stuck-item", head="f" * 40, red_min_ago=40)
        for _ in range(10):
            self.tick()
        self.assertEqual(len(self.repo.issues_filed()), 2)
        self.assertEqual(self.repo.comments(), [])

    def test_a_new_tag_under_an_open_flag_adds_one_comment_not_a_second_issue(self) -> None:
        self.repo.published("v0.6.367")
        self.repo.release_run("v0.6.368", "failure")
        self.tick()
        self.repo.release_run("v0.6.369", "failure")
        for _ in range(10):
            self.tick()
        self.assertEqual(len(self.repo.issues_filed()), 1)
        self.assertEqual(len(self.repo.comments()), 1)
        self.assertIn("v0.6.369", self.repo.comments()[0])

    def test_the_flag_closes_itself_when_the_queue_moves_again(self) -> None:
        self.repo.published("v0.6.367")
        self.repo.release_run("v0.6.368", "failure")
        self.held_queue()
        self.tick()
        issue = self.repo.issue_seq
        self.repo.published("v0.6.368")
        for _ in range(5):
            self.tick()
        self.assertEqual(self.repo.closes(), [f"/issues/{issue}"])
        self.assertIn("can advance again", self.repo.comments()[0])
        self.assertIn("nothing was released, bypassed or merged", self.repo.comments()[0].lower())
        self.assertEqual(len(self.events("release_queue_moving")), 1)

    def test_github_falling_over_mid_stall_does_not_close_the_flag(self) -> None:
        """An unreadable check is not a recovery: the flag stays up and nothing is re-filed."""
        self.repo.published("v0.6.367")
        self.repo.release_run("v0.6.368", "failure")
        self.tick()
        with mock.patch.object(FakeGitHub, "api", side_effect=fp.HttpError(502, "Bad Gateway", "/x")):
            for _ in range(5):
                self.tick()
        self.assertEqual(len(self.repo.issues_filed()), 1)
        self.assertEqual(self.repo.closes(), [])

    # -- the channel --------------------------------------------------------------------------------
    def test_the_default_channel_records_the_stall_and_posts_nothing(self) -> None:
        """Shipped default: `log`. Landing the guard cannot comment, file or close anything."""
        self.cfg.stall_channel = "log"
        self.repo.published("v0.6.367")
        self.repo.release_run("v0.6.368", "failure")
        self.held_queue()
        for _ in range(30):
            self.tick()
        self.assertEqual(self.repo.writes, [], "the log channel posts nothing at all")
        self.assertEqual(len(self.events("release_queue_stalled")), 1, "but the stall is in the audit once")

    def test_off_detects_nothing(self) -> None:
        self.cfg.stall_channel = "off"
        self.repo.published("v0.6.367")
        self.repo.release_run("v0.6.368", "failure")
        self.tick()
        self.assertEqual(self.repo.writes, [])
        self.assertEqual(self.events("release_queue_stalled"), [])

    # -- the hold is never touched ------------------------------------------------------------------
    def test_the_guard_never_writes_to_the_queue_it_watches(self) -> None:
        """Jacob's limit: flag only. No merge, no release, no do-not-merge marker moved."""
        self.repo.published("v0.6.367")
        self.repo.release_run("v0.6.368", "failure")
        self.held_queue()
        self.gating("stuck-item", head="f" * 40, red_min_ago=40)
        before = {fid: self.loop.store.state(fid) for fid in self.loop.store.items()}
        self.tick()
        self.assertEqual({fid: self.loop.store.state(fid) for fid in self.loop.store.items()}, before,
                         "no item state changed")
        self.assertEqual([p for m, p, _ in self.repo.writes if m != "POST" or not p.startswith("/issues")], [],
                         "the only writes are the ops issues")


if __name__ == "__main__":
    unittest.main()
