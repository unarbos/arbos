"""Local contracts only: no project build, network, commits, refs, or live index changes."""
import copy
import difflib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("assembler", Path(__file__).with_name("assemble.py"))
assembler = importlib.util.module_from_spec(spec)
spec.loader.exec_module(assembler)


class FakeGit:
    def __init__(self):
        self.calls = []

    def text(self, *args):
        self.calls.append(args)
        return args[2][:-9]  # exact <commit>^{commit}

    def run(self, *args):
        self.calls.append(args)
        return b""


class InputContracts(unittest.TestCase):
    def pins(self):
        names = ["structure", "state-machines", "observability", "performance", "database-scale", "laws", "finish"]
        return {"version": 1, "base": assembler.BASE, "phases": [
            {"phase": n, "ref": "refs/heads/improve/" + name, "commit": f"{n:040x}"}
            for n, name in zip(range(3, 10), names)]}

    def load(self, value, git=None):
        with tempfile.TemporaryDirectory(prefix="arbos-assembly-contract-") as directory:
            path = Path(directory) / "pins.json"
            path.write_text(json.dumps(value))
            return assembler.load_pins(path, git or FakeGit())

    def test_safe_relative_asset_and_spaces(self):
        self.assertEqual(str(assembler.safe_path("assets/a file.svg")), "assets/a file.svg")

    def test_unsafe_paths_are_rejected(self):
        for name in ("/absolute", "../escape", "ok/../escape", ".git/config", ".env", "a/.ENV.local", "a/.environment"):
            with self.subTest(name=name), self.assertRaises(ValueError):
                assembler.safe_path(name)

    def test_immutable_pins_do_not_require_latest_label_identity(self):
        git = FakeGit()
        value = self.pins()
        self.assertEqual(self.load(value, git), value)
        self.assertTrue(all(args[2].endswith("^{commit}") and not args[2].startswith("refs/")
                            for args in git.calls if args[0] == "rev-parse"))
        self.assertEqual(sum(args[0] == "merge-base" for args in git.calls), 7)

    def test_missing_pin_and_invalid_ref_are_rejected(self):
        value = self.pins()
        value["phases"][1]["commit"] = None
        with self.assertRaisesRegex(ValueError, "Phase 4 still needs"):
            self.load(value)
        value = self.pins()
        value["phases"][0]["ref"] = "refs/heads/main"
        with self.assertRaisesRegex(ValueError, "explicit local improve"):
            self.load(value)

    def test_phase_order_and_unknown_commit_are_rejected(self):
        value = self.pins()
        value["phases"].reverse()
        with self.assertRaisesRegex(ValueError, "phases 3 through 9"):
            self.load(value)
        git = FakeGit()
        git.text = lambda *args: "0" * 40
        with self.assertRaisesRegex(ValueError, "immutable commit"):
            self.load(self.pins(), git)

    def test_snapshot_labels_are_optional_exact_names_and_observed(self):
        value = self.pins()
        git = FakeGit()
        first = value["phases"][0]["ref"]
        git.run = lambda *args: (first + " " + "a" * 40 + "\n" + first + "/extra " + "b" * 40 + "\n").encode()
        actual = assembler.snapshot_refs(git, value)
        self.assertEqual(actual[first], "a" * 40)
        self.assertTrue(all(actual[item["ref"]] is None for item in value["phases"][1:]))
        self.assertNotIn(first + "/extra", actual)

    def test_carried_source_and_diff_guards_are_both_enforced(self):
        before, after, name = b"first\n", b"first\nsecond\n", "src/file.rs"
        git = FakeGit()
        git.run = lambda op, key: before if key.startswith("p3:") else after
        delta = "".join(difflib.unified_diff(before.decode().splitlines(True), after.decode().splitlines(True),
                                            fromfile="phase3/" + name, tofile="phase4/" + name)).encode()
        guard = {"phase3_sha256": assembler.digest(before), "phase4_sha256": assembler.digest(after),
                 "review_diff_algorithm": "stdlib-difflib-unified-context3-phase3-phase4-v1",
                 "review_diff_sha256": assembler.digest(delta)}
        assembler.verify_carried(git, "p3", "p4", name, guard)
        for field in ("phase3_sha256", "phase4_sha256", "review_diff_sha256"):
            invalid = copy.deepcopy(guard)
            invalid[field] = "0" * 64
            with self.subTest(field=field), self.assertRaises(ValueError):
                assembler.verify_carried(git, "p3", "p4", name, invalid)

    def test_changed_local_input_or_assembler_refuses_close(self):
        with tempfile.TemporaryDirectory(prefix="arbos-assembly-input-") as directory:
            script = Path(directory) / "assemble.py"
            guard = Path(directory) / "guard.json"
            script.write_bytes(b"reviewed script")
            guard.write_bytes(b"reviewed guard")
            expected_script = assembler.digest(script.read_bytes())
            expected_inputs = {guard.name: assembler.digest(guard.read_bytes())}
            assembler.verify_input_bytes(script, expected_script, expected_inputs)
            guard.write_bytes(b"changed guard")
            with self.assertRaisesRegex(ValueError, "Local input bytes changed"):
                assembler.verify_input_bytes(script, expected_script, expected_inputs)
            guard.write_bytes(b"reviewed guard")
            script.write_bytes(b"changed script")
            with self.assertRaisesRegex(ValueError, "Assembler bytes changed"):
                assembler.verify_input_bytes(script, expected_script, expected_inputs)

    def test_nonregular_tree_entries_are_rejected_before_materialization(self):
        git = FakeGit()
        git.run = lambda *args: b"120000 blob " + b"a" * 40 + b"\tlink\0"
        with self.assertRaisesRegex(ValueError, "nonregular"):
            assembler.Git.tree(git, "tree")

    def test_output_outside_private_evidence_is_refused_before_creation(self):
        with tempfile.TemporaryDirectory(prefix="arbos-assembly-contract-") as directory:
            repo = Path(directory) / "repo"
            repo.mkdir()
            output = Path(directory) / "outside"
            with patch.object(assembler, "load_pins", return_value=self.pins()):
                with self.assertRaisesRegex(ValueError, "Output must be a new child"):
                    assembler.assemble(repo, repo / "pins.json", output)
            self.assertFalse(output.exists())


class ActualGitContracts(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="arbos-assembly-git-")
        self.addCleanup(self.directory.cleanup)
        self.repo = Path(self.directory.name) / "repo"
        self.repo.mkdir()
        self.live = assembler.Git(self.repo)
        self.live.run("-c", "init.defaultBranch=main", "init", "--quiet")
        self.live.run("read-tree", "--empty")
        self.blob = self.live.run("hash-object", "-w", "--stdin", data=b"original\n").decode().strip()
        self.live.run("update-index", "--add", "--cacheinfo", "100644", self.blob, "assets/a file.txt")
        self.tree = self.live.text("write-tree")
        self.private = assembler.Git(self.repo, Path(self.directory.name) / "private.index")
        self.private.run("read-tree", self.tree)

    def test_actual_patch_conflict_refuses_and_live_index_is_unchanged(self):
        live_index = self.repo / ".git/index"
        before = live_index.read_bytes()
        delta = b"--- a/assets/a file.txt\n+++ b/assets/a file.txt\n@@ -1 +1 @@\n-original\n+changed\n"
        assembler.apply(self.private, delta)
        self.assertEqual(self.private.run("show", ":assets/a file.txt"), b"changed\n")
        private_before = Path(self.private.env["GIT_INDEX_FILE"]).read_bytes()
        with self.assertRaises(RuntimeError):
            assembler.apply(self.private, delta)
        self.assertEqual(Path(self.private.env["GIT_INDEX_FILE"]).read_bytes(), private_before)
        self.assertEqual(live_index.read_bytes(), before)
        self.assertEqual(self.live.run("show", ":assets/a file.txt"), b"original\n")

    def test_exact_blob_materialization_keeps_bytes_modes_and_manifest(self):
        executable = self.live.run("hash-object", "-w", "--stdin", data=b"#!/bin/sh\nexit 0\n").decode().strip()
        self.private.run("update-index", "--add", "--cacheinfo", "100755", executable, "scripts/inert.sh")
        entries = self.private.tree(self.private.text("write-tree"))
        output = Path(self.directory.name) / "materialized"
        manifest, size = assembler.materialize(self.private, entries, output)
        self.assertEqual((output / "assets/a file.txt").read_bytes(), b"original\n")
        self.assertEqual((output / "scripts/inert.sh").stat().st_mode & 0o777, 0o755)
        self.assertEqual(size, len(b"original\n#!/bin/sh\nexit 0\n"))
        self.assertEqual([item["path"] for item in manifest], sorted(entries))
        self.assertTrue(all(item["sha256"] == assembler.digest((output / item["path"]).read_bytes()) for item in manifest))


if __name__ == "__main__":
    unittest.main()
