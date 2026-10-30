"""Ship checks of feedback_poller.py against a small simulated GitHub repository.

    python3 -m unittest discover -s scripts/feedback -p 'test_*.py' -v

The fake serves the REST shapes the poller reads (commits, compare, contents, releases, workflow runs) from an
in-memory commit graph, with unified diffs from difflib and real git blob shas, and records every write.
"""

from __future__ import annotations

import base64
import difflib
import hashlib
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


def blob_sha(text: str) -> str:
    data = text.encode()
    return hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()


def file_diff(path: str, old: str | None, new: str | None) -> dict | None:
    if old == new:
        return None
    status = "added" if old is None else "removed" if new is None else "modified"
    lines = list(difflib.unified_diff((old or "").splitlines(), (new or "").splitlines(), lineterm="", n=3))[2:]
    return {"filename": path, "status": status, "sha": blob_sha(new) if new is not None else None, "patch": "\n".join(lines)}


class FakeRepo:
    def __init__(self) -> None:
        self.commits: dict[str, dict] = {}
        self.refs: dict[str, str] = {}
        self.releases: list[dict] = []
        self.runs: list[dict] = []
        self.writes: list[tuple[str, str, object]] = []

    def commit(self, parents: list[str], message: str, tree: dict[str, str], date: str) -> str:
        sha = hashlib.sha1(json.dumps([parents, message, tree, date]).encode()).hexdigest()
        self.commits[sha] = {"parents": parents, "message": message, "tree": dict(tree), "date": date}
        return sha

    def resolve(self, ref: str) -> str:
        return self.refs.get(ref, ref)

    def ancestors(self, sha: str) -> set[str]:
        seen, todo = set(), [sha]
        while todo:
            s = todo.pop()
            if s not in seen:
                seen.add(s)
                todo += self.commits[s]["parents"]
        return seen

    def diff(self, base_tree: dict[str, str], head_tree: dict[str, str]) -> list[dict]:
        return [d for p in sorted(set(base_tree) | set(head_tree)) for d in [file_diff(p, base_tree.get(p), head_tree.get(p))] if d]

    def commit_doc(self, sha: str) -> dict:
        c = self.commits[sha]
        parent_tree = self.commits[c["parents"][0]]["tree"] if c["parents"] else {}
        return {"sha": sha, "html_url": f"https://github.com/{REPO}/commit/{sha}", "parents": [{"sha": p} for p in c["parents"]],
                "commit": {"message": c["message"], "committer": {"date": c["date"]}}, "files": self.diff(parent_tree, c["tree"])}

    def compare(self, base: str, head: str) -> dict:
        b, h = self.resolve(base), self.resolve(head)
        if b == h:
            status = "identical"
        elif b in self.ancestors(h):
            status = "ahead"
        elif h in self.ancestors(b):
            status = "behind"
        else:
            status = "diverged"
        return {"status": status, "files": self.diff(self.commits[b]["tree"], self.commits[h]["tree"]) if status == "ahead" else []}

    def api(self, method: str, path: str, json_body: dict | None = None, **_kw: object) -> object:
        url = urllib.parse.urlsplit(path)
        q = dict(urllib.parse.parse_qsl(url.query))
        p = url.path.removeprefix(f"/repos/{REPO}")
        if method != "GET":
            self.writes.append((method, p, json_body))
            return {}
        parts = p.strip("/").split("/")
        if parts[0] == "commits" and len(parts) == 2:
            return self.commit_doc(self.resolve(parts[1]))
        if parts[0] == "compare":
            base, head = urllib.parse.unquote(parts[1]).split("...")
            return self.compare(base, head)
        if parts[0] == "contents":
            tree = self.commits[self.resolve(q["ref"])]["tree"]
            path_in_repo = urllib.parse.unquote("/".join(parts[1:]))
            if path_in_repo not in tree:
                raise fp.HttpError(404, "Not Found", path)
            text = tree[path_in_repo]
            return {"type": "file", "sha": blob_sha(text), "encoding": "base64", "content": base64.b64encode(text.encode()).decode()}
        if parts[0] == "releases":
            return self.releases
        if parts[0] == "labels":
            return {}
        if parts[:2] == ["actions", "runs"] and len(parts) == 2:
            return {"workflow_runs": [r for r in self.runs if r["head_sha"] == q.get("head_sha")]}
        if parts[:2] == ["actions", "runs"] and parts[-1] == "artifacts":
            run = next(r for r in self.runs if str(r["id"]) == parts[2])
            return {"artifacts": [{"name": f"xcodebuild-logs-{run['build_prefix']}"}] if run.get("build_prefix") else []}
        if parts[:2] == ["actions", "workflows"]:
            wf = parts[2]
            if q.get("status") != "success":
                return {"workflow_runs": []}
            return {"workflow_runs": [r for r in self.runs if r["workflow"] == wf and r["conclusion"] == "success"]}
        raise AssertionError(f"unexpected GET {path}")

    def comments(self) -> list[str]:
        return [body["body"] for m, p, body in self.writes if m == "POST" and p.endswith("/comments")]

    def closed(self) -> bool:
        return any(m == "PATCH" and (body or {}).get("state") == "closed" for m, p, body in self.writes)


class FakeGitHub(fp.GitHub):
    def __init__(self, cfg: object, repo: FakeRepo) -> None:
        super().__init__(cfg)
        self.fake = repo

    def api(self, method: str, path: str, **kw: object) -> object:
        return self.fake.api(method, path, **kw)


class FakeAsc:
    def build_state(self, version: str) -> dict:
        return {"version": version, "processingState": "VALID"}


HEADER_BEFORE = """\
pub fn header(cx: &mut Cx) -> Div {
    let pill = pill(cx);
    let avatar = avatar(cx, 22.0);
    let name = name_label(cx, 13.0);
    div()
        .child(avatar)
        .child(presence_dot(cx))
        .child(name)
        .child(pill)
}

fn presence_dot(cx: &mut Cx) -> Div {
    dot(cx, GREEN)
}

fn name_label(cx: &mut Cx, size: f32) -> Div {
    label(cx, size)
}
"""

HEADER_FIXED = HEADER_BEFORE.replace("""        .child(avatar)
        .child(presence_dot(cx))
""", """        .child(avatar.with_badge(badge::ring(cx)))
""").replace("""fn presence_dot(cx: &mut Cx) -> Div {
    dot(cx, GREEN)
}
""", """fn presence_dot(cx: &mut Cx) -> Div {
    badge::dot(cx, badge::size_for(22.0))
}
""")

BADGE = """\
pub fn ring(cx: &mut Cx) -> Ring {
    Ring::cut_out(cx, theme::surface(cx))
}

pub fn size_for(avatar: f32) -> f32 {
    (avatar / 4.0).clamp(6.0, 12.0)
}
"""

SWIFT_BEFORE = """\
struct AgentAvatar: View {
    var body: some View {
        ZStack(alignment: .bottomTrailing) {
            Circle().fill(color)
            Text(initials)
            Circle().fill(.green).frame(width: 8)
        }
    }
}
"""

SWIFT_FIXED = SWIFT_BEFORE.replace("            Circle().fill(.green).frame(width: 8)\n",
                                   "            PresenceBadge(size: size)\n                .zIndex(1)\n")


class ShipCheckTest(unittest.TestCase):
    """One feedback item per test, already `pushed`: the fix merged to main as `Merge feedback/<id>`."""

    FID = "presence-dot-2026-10-30"

    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        state_dir = Path(self.tmp.name)
        self.cfg = types.SimpleNamespace(state_dir=state_dir, github_repo=REPO, github_token="t", ship_timeout_min=90,
                                         relay_url="", relay_admin_token="")
        self.repo = FakeRepo()
        self.loop = fp.Loop.__new__(fp.Loop)
        self.loop.cfg = self.cfg
        self.loop.store = fp.Store(self.cfg)
        self.loop.gh = FakeGitHub(self.cfg, self.repo)
        self.loop.asc = FakeAsc()
        cur = self.loop.store.cursor()
        cur["deploy_graph"] = {"at": fp.iso(), "dirs": {"agentd": ["crates/arbos-agentd/"], "relay": ["crates/arbos-relay/"]}}
        self.loop.store.save_cursor(cur)
        (state_dir / self.FID).mkdir()
        (state_dir / self.FID / "submission.json").write_text(json.dumps({"created": "2026-10-30T09:00:00Z", "comment": "dot sits behind the picture"}))

    def tearDown(self) -> None:
        self.tmp.cleanup()

    # ---- history builders

    def land(self, path: str, before: str, fixed: str, extra: dict[str, str] | None = None) -> tuple[str, str, str]:
        """main: base → merge of the fix branch. Returns (base, fix commit, merge)."""
        base = self.repo.commit([], "base", {path: before, "README.md": "arbos\n"}, "2026-10-30T08:00:00Z")
        tree = {path: fixed, "README.md": "arbos\n", **(extra or {})}
        fix = self.repo.commit([base], f"fix(feedback): presence badge in front of the picture [feedback {self.FID}]", tree, "2026-10-30T09:30:00Z")
        merge = self.repo.commit([base, fix], f"Merge feedback/{self.FID}: handed-off fix by bc-1 [feedback {self.FID}]", tree, "2026-10-30T10:00:00Z")
        return base, fix, merge

    def then(self, parent: str, message: str, tree: dict[str, str], date: str) -> str:
        return self.repo.commit([parent], message, tree, date)

    def release(self, sha: str, tag: str, published: str) -> None:
        self.repo.refs[tag] = sha
        self.repo.releases.insert(0, {"tag_name": tag, "draft": False, "prerelease": False, "published_at": published,
                                      "html_url": f"https://github.com/{REPO}/releases/tag/{tag}", "target_commitish": "main"})

    def add_run(self, workflow: str, name: str, head: str, event: str = "push", **extra: object) -> None:
        rid = len(self.repo.runs) + 1000
        self.repo.runs.append({"id": rid, "run_number": rid, "workflow": workflow, "name": name, "head_sha": head,
                               "head_branch": "main", "event": event, "status": "completed", "conclusion": "success",
                               "run_started_at": "2026-10-30T11:00:00Z", "updated_at": "2026-10-30T11:30:00Z",
                               "html_url": f"https://github.com/{REPO}/actions/runs/{rid}", **extra})

    def pushed(self, fix: str, merge: str, files: list[str], **extra: object) -> dict:
        st = {"status": "pushed", "issue": 2098, "issue_url": f"https://github.com/{REPO}/issues/2098", "commit": fix,
              "commit_url": f"https://github.com/{REPO}/commit/{fix}", "commit_message": "fix(feedback): presence badge in front of the picture",
              "merge_sha": merge, "pushed_at": "2026-10-30T10:00:00Z", "commit_date": "2026-10-30T10:00:00Z", "files": files,
              "expects_ios": fp.touches_ios(files), "expects_ci": any(not f.startswith("apps/ios/") for f in files),
              "created": "2026-10-30T09:00:00Z", **extra}
        self.loop.store.save_state(self.FID, st)
        return st

    def tick(self) -> dict:
        self.loop.gh._releases = None
        self.loop.check_ship(self.FID, self.loop.store.state(self.FID))
        return self.loop.store.state(self.FID)

    # ---- Mac: a release tag carries the commit

    def mac_history(self, reverted: bool) -> tuple[str, str]:
        path = "crates/arbos-mac/src/ui/header.rs"
        _, fix, merge = self.land(path, HEADER_BEFORE, HEADER_FIXED, {"crates/arbos-mac/src/ui/badge.rs": BADGE})
        head = merge
        if reverted:
            # The #2098 shape: a free-text revert message, no "This reverts commit <sha>" line.
            head = self.then(head, f'Revert "Merge feedback/{self.FID}" [hotfix]\n\nReverts merge {merge[:9]} (#2098): '
                             "Jacob's no-dots rule.", {path: HEADER_BEFORE, "README.md": "arbos\n"}, "2026-10-30T10:20:00Z")
        else:
            # A later fix elsewhere in the same file, and one that rewrites a line next to the fix.
            later = self.repo.commits[head]["tree"][path].replace("label(cx, size)", "label(cx, size).truncate()")
            later = later.replace("    let pill = pill(cx);", "    let pill = pill(cx).centred();")
            head = self.then(head, "fix(feedback): name truncates [feedback other]", {**self.repo.commits[head]["tree"], path: later},
                             "2026-10-30T10:20:00Z")
        bump = self.then(head, "release: 0.6.366 — …", self.repo.commits[head]["tree"], "2026-10-30T10:40:00Z")
        self.add_run("ci.yml", "CI", bump)
        self.release(bump, "v0.6.366", "2026-10-30T11:40:00Z")
        self.pushed(fix, merge, ["crates/arbos-mac/src/ui/header.rs", "crates/arbos-mac/src/ui/badge.rs"])
        return fix, bump

    def test_mac_reverted_fix_is_not_shipped(self) -> None:
        fix, _ = self.mac_history(reverted=True)
        self.assertEqual(self.loop.gh.release_carrying(fix, "2026-10-30T10:00:00Z")["tag_name"], "v0.6.366",
                         "precondition: the tag carries the fix commit by ancestry")
        st = self.tick()
        self.assertEqual(st["status"], "pushed")
        self.assertFalse(st.get("release_done"))
        self.assertEqual(st["fix_held"]["verdict"], fp.FIX_REVERTED)
        self.assertEqual(st["fix_held"]["ref"], "v0.6.366")
        self.assertEqual(self.repo.comments(), [])
        self.assertFalse(self.repo.closed())

    def test_mac_reverted_fix_stays_held_past_the_ship_timeout(self) -> None:
        self.mac_history(reverted=True)
        self.tick()
        st = self.loop.store.state(self.FID)
        st["pushed_at"] = "2026-10-29T00:00:00Z"
        self.loop.store.save_state(self.FID, st)
        st = self.tick()
        self.assertEqual(st["status"], "pushed")
        self.assertEqual(self.repo.comments(), [])
        self.assertEqual(self.repo.writes, [])

    def test_mac_genuine_fix_ships_and_closes(self) -> None:
        self.mac_history(reverted=False)
        st = self.tick()
        self.assertEqual(st["status"], "shipped")
        self.assertEqual(st["edge_release"], "v0.6.366")
        self.assertNotIn("fix_held", st)
        self.assertTrue(self.repo.closed())
        self.assertTrue(any(c.startswith("Fixed in Mac v0.6.366") for c in self.repo.comments()), self.repo.comments())

    def test_mac_old_ancestry_logic_closes_the_reverted_fix(self) -> None:
        """Control: with the files check switched off the same history is closed as fixed — the bug this guards."""
        self.mac_history(reverted=True)
        with mock.patch.object(fp.Loop, "build_has_fix", return_value=True):
            st = self.tick()
        self.assertEqual(st["status"], "shipped")
        self.assertTrue(any(c.startswith("Fixed in Mac v0.6.366") for c in self.repo.comments()))

    def test_reverted_list_is_honoured(self) -> None:
        fix, _ = self.mac_history(reverted=False)
        st = self.loop.store.state(self.FID)
        st["reverted"] = [fix]
        self.loop.store.save_state(self.FID, st)
        st = self.tick()
        self.assertEqual(st["status"], "pushed")
        self.assertEqual(st["fix_held"]["verdict"], fp.FIX_REVERTED)
        self.assertFalse(self.repo.closed())

    # ---- iPhone: a TestFlight build's commit carries the fix

    def ios_history(self, reverted: bool) -> str:
        path = "apps/ios/Arbos/Arbos/UI/Avatar.swift"
        _, fix, merge = self.land(path, SWIFT_BEFORE, SWIFT_FIXED)
        head = merge
        if reverted:
            head = self.then(head, f'Revert "Merge feedback/{self.FID}" [hotfix]\n\nReverts merge {merge[:9]} (#2100).',
                             {path: SWIFT_BEFORE, "README.md": "arbos\n"}, "2026-10-30T10:20:00Z")
        else:
            later = self.repo.commits[head]["tree"][path].replace("Text(initials)", "Text(initials).font(.caption)")
            head = self.then(head, "fix(feedback): initials font [feedback other]", {**self.repo.commits[head]["tree"], path: later},
                             "2026-10-30T10:20:00Z")
        self.add_run("ios.yml", "iOS", head, build_prefix="202610301120")
        self.pushed(fix, merge, [path])
        return fix

    def test_ios_reverted_fix_is_not_shipped(self) -> None:
        fix = self.ios_history(reverted=True)
        later = self.loop.gh.later_ios_build(fix, "2026-10-30T10:00:00Z")
        self.assertIsNotNone(later, "precondition: the TestFlight build carries the fix commit by ancestry")
        st = self.tick()
        self.assertEqual(st["status"], "pushed")
        self.assertFalse(st.get("ios_done"))
        self.assertIsNone(st.get("testflight_build"))
        self.assertEqual(st["fix_held"]["verdict"], fp.FIX_REVERTED)
        self.assertEqual(self.repo.comments(), [])
        self.assertFalse(self.repo.closed())

    def test_ios_genuine_fix_ships_and_closes(self) -> None:
        self.ios_history(reverted=False)
        st = self.tick()
        self.assertEqual(st["status"], "shipped")
        self.assertTrue(st["ios_done"])
        self.assertTrue(st["testflight_build"].startswith("202610301120"))
        self.assertTrue(self.repo.closed())
        self.assertTrue(any(c.startswith(f"Fixed in TestFlight {st['testflight_build']}") for c in self.repo.comments()), self.repo.comments())

    def test_ios_old_ancestry_logic_closes_the_reverted_fix(self) -> None:
        self.ios_history(reverted=True)
        with mock.patch.object(fp.Loop, "build_has_fix", return_value=True):
            st = self.tick()
        self.assertEqual(st["status"], "shipped")
        self.assertTrue(any(c.startswith("Fixed in TestFlight") for c in self.repo.comments()))


def change(path: str, old: str | None, new: str | None) -> dict:
    d = file_diff(path, old, new)
    assert d is not None
    return d


def at(text: str | None) -> dict | None:
    return None if text is None else {"sha": blob_sha(text), "text": text}


BODY = "\n".join(f"line {i}" for i in range(1, 41)) + "\n"


def edit(text: str, old: str, new: str) -> str:
    assert old in text, old
    return text.replace(old, new, 1)


class FixPresenceTest(unittest.TestCase):
    """The tree check on its own: what a build's files say about one fix."""

    def verdict(self, changes: list[dict], build: dict[str, str]) -> str:
        return fp.fix_presence(changes, lambda path: at(build.get(path)))[0]

    def test_untouched_since_the_fix(self) -> None:
        fixed = edit(BODY, "line 20\n", "line 20 fixed\n")
        self.assertEqual(self.verdict([change("a.rs", BODY, fixed)], {"a.rs": fixed}), fp.FIX_PRESENT)

    def test_full_revert(self) -> None:
        fixed = edit(edit(BODY, "line 5\n", "line 5 fixed\n"), "line 30\n", "line 30 fixed\n")
        self.assertEqual(self.verdict([change("a.rs", BODY, fixed)], {"a.rs": BODY}), fp.FIX_REVERTED)

    def test_later_edits_elsewhere_and_next_to_the_fix(self) -> None:
        fixed = edit(BODY, "line 20\n", "line 20 fixed\n")
        build = edit(edit(fixed, "line 19\n", "line 19 renamed\n"), "line 2\n", "line 2 other fix\n")
        self.assertEqual(self.verdict([change("a.rs", BODY, fixed)], {"a.rs": build}), fp.FIX_PRESENT)

    def test_one_of_three_hunks_rewritten_by_a_follow_up(self) -> None:
        fixed = edit(edit(edit(BODY, "line 5\n", "line 5 fixed\n"), "line 20\n", "line 20 fixed\n"), "line 35\n", "line 35 fixed\n")
        build = edit(fixed, "line 35 fixed\n", "line 35 fixed better\n")
        self.assertEqual(self.verdict([change("a.rs", BODY, fixed)], {"a.rs": build}), fp.FIX_PRESENT)

    def test_merge_resolution_undid_one_hunk(self) -> None:
        fixed = edit(edit(BODY, "line 5\n", "line 5 fixed\n"), "line 30\n", "line 30 fixed\n")
        build = edit(fixed, "line 30 fixed\n", "line 30\n")
        self.assertEqual(self.verdict([change("a.rs", BODY, fixed)], {"a.rs": build}), fp.FIX_REVERTED)

    def test_most_of_the_fix_rewritten_is_not_present(self) -> None:
        fixed = edit(edit(edit(BODY, "line 5\n", "line 5 fixed\n"), "line 20\n", "line 20 fixed\n"), "line 35\n", "line 35 fixed\n")
        build = edit(edit(fixed, "line 5 fixed\n", "line 5 other\n"), "line 20 fixed\n", "line 20 other\n")
        self.assertEqual(self.verdict([change("a.rs", BODY, fixed)], {"a.rs": build}), fp.FIX_UNKNOWN)

    def test_added_file_deleted_again(self) -> None:
        self.assertEqual(self.verdict([change("badge.rs", None, BADGE)], {}), fp.FIX_REVERTED)

    def test_deleted_file_back(self) -> None:
        self.assertEqual(self.verdict([change("old.rs", BADGE, None)], {"old.rs": BADGE}), fp.FIX_REVERTED)
        self.assertEqual(self.verdict([change("old.rs", BADGE, None)], {}), fp.FIX_PRESENT)

    def test_append_at_end_of_file_removed_again(self) -> None:
        fixed = BODY + "line 41 added\n"
        self.assertEqual(self.verdict([change("a.rs", BODY, fixed)], {"a.rs": fixed}), fp.FIX_PRESENT)
        self.assertNotEqual(self.verdict([change("a.rs", BODY, fixed)], {"a.rs": BODY}), fp.FIX_PRESENT)

    def test_pure_deletion_reverted(self) -> None:
        fixed = edit(BODY, "line 20\nline 21\n", "")
        self.assertEqual(self.verdict([change("a.rs", BODY, fixed)], {"a.rs": fixed}), fp.FIX_PRESENT)
        self.assertEqual(self.verdict([change("a.rs", BODY, fixed)], {"a.rs": BODY}), fp.FIX_REVERTED)

    def test_binary_file_needs_the_identical_blob(self) -> None:
        png = {"filename": "icon.png", "status": "modified", "sha": "a" * 40}
        self.assertEqual(fp.fix_presence([png], lambda _p: {"sha": "a" * 40, "text": None})[0], fp.FIX_PRESENT)
        self.assertEqual(fp.fix_presence([png], lambda _p: {"sha": "b" * 40, "text": None})[0], fp.FIX_UNKNOWN)

    def test_a_fix_with_no_files_of_its_own(self) -> None:
        self.assertEqual(fp.fix_presence([], lambda _p: None)[0], fp.FIX_PRESENT)


if __name__ == "__main__":
    unittest.main()
