#!/usr/bin/env python3
"""Prepare a private validation tree; never commit, update a ref, merge, or push.

Requires explicit frozen checkpoint pins; final publication still needs final pins. Git writes immutable blob/tree objects, but
all mutable index/materialized state is confined to a new private output folder.
"""
import argparse
import difflib
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import time

BASE = "2865cb29b806121d4a90f55ba6ce151a150279a0"
REDACT = "crates/arbos-core/src/redact.rs"
RESOLUTION = ".improve/integration/redaction-after-performance.patch"
REVIEWED_REDACTION = {
    6: "aeebd4d52c2cec5fc7cd32f23e5cf51a371e011caf735b8462282aa98f48b568",
    8: "51053e0dd677a570466b66431d8863519dbd9628175f8556cfd0589db4e8117c",
}
CANONICAL = "1919913639b72ba43ffcff9a51bd29fe3038f08701de5e390463613b2dbde3da"
RESOLUTION_SHA256 = "a138335631889a984955e26ddf190e86b5ec6932cc9d67e782f05340471cb413"
SWIFT_DUPLICATES = (
    "apps/ios/Arbos/fastlane/Fastfile",
    "scripts/ci/xctestrun-env.py",
    "scripts/ci/test_xctestrun_env.py",
)

RUST_FENCES = (
    "scripts/ci/run-tests-no-egress.sh",
    "scripts/ci/netns-smoke.py",
    "scripts/ci/test_no_egress_runner.py",
    "scripts/ci/test_netns_smoke.py",
)
IDENTICAL_FENCES = SWIFT_DUPLICATES + RUST_FENCES


def digest(data):
    return hashlib.sha256(data).hexdigest()


def safe_path(name):
    path = PurePosixPath(name)
    if path.is_absolute() or ".." in path.parts or ".git" in path.parts:
        raise ValueError("Unsafe tracked path")
    if any(part.lower().startswith(".env") for part in path.parts):
        raise ValueError("Unexpected tracked environment path")
    return path


class Git:
    def __init__(self, repo, index=None):
        self.repo = repo
        self.command = [shutil.which("git") or "git", "-C", str(repo),
                        "-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false"]
        self.env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
                    "LANG": "C", "LC_ALL": "C", "GIT_CONFIG_NOSYSTEM": "1",
                    "GIT_CONFIG_GLOBAL": os.devnull, "GIT_TERMINAL_PROMPT": "0",
                    "GIT_OPTIONAL_LOCKS": "0"}
        if index is not None:
            self.env["GIT_INDEX_FILE"] = str(index)

    def run(self, *arguments, data=None):
        result = subprocess.run([*self.command, *arguments], env=self.env,
                                input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if result.returncode:
            raise RuntimeError(f"git {arguments[0]} failed: " + result.stderr.decode(errors="replace"))
        return result.stdout

    def text(self, *arguments):
        return self.run(*arguments).decode().strip()

    def tree(self, treeish):
        result = {}
        for record in self.run("ls-tree", "-rz", treeish).split(b"\0"):
            if not record:
                continue
            header, name = record.split(b"\t", 1)
            mode, kind, oid = header.decode().split()
            name = name.decode()
            safe_path(name)
            if kind != "blob" or mode not in ("100644", "100755"):
                raise ValueError(f"Unexpected nonregular tracked entry: {name}")
            result[name] = (mode, oid)
        return result


def load_pins(path, git):
    pins = json.loads(path.read_text())
    if pins.get("version") != 1 or pins.get("base") != BASE:
        raise ValueError("Require the reviewed manifest version and exact upstream baseline")
    phases = pins.get("phases")
    if not isinstance(phases, list) or [p.get("phase") for p in phases] != list(range(3, 10)):
        raise ValueError("Require phases 3 through 9 exactly once, in order")
    for phase in phases:
        commit, ref = phase.get("commit"), phase.get("ref")
        if not isinstance(commit, str) or not re.fullmatch(r"[0-9a-f]{40}", commit):
            raise ValueError(f"Phase {phase['phase']} still needs a reviewed final commit pin")
        if not isinstance(ref, str) or not ref.startswith("refs/heads/improve/"):
            raise ValueError("Require an explicit local improve branch for each pin")
        if git.text("rev-parse", "--verify", commit + "^{commit}") != commit:
            raise ValueError(f"Phase {phase['phase']} immutable commit is unavailable or changed")
        git.run("merge-base", "--is-ancestor", BASE, commit)  # Read-only ancestry check.
    return pins


def snapshot_refs(git, pins):
    """Labels are movement sentinels, never substitutes for immutable input pins."""
    names = [phase["ref"] for phase in pins["phases"]]
    rows = git.run("for-each-ref", "--format=%(refname) %(objectname)", *names)
    found = dict(line.split() for line in rows.decode().splitlines())
    return {name: found.get(name) for name in names}


def verify_carried(git, phase3, phase4, name, reviewed):
    safe_path(name)
    phase3_data = git.run("show", phase3 + ":" + name)
    phase4_data = git.run("show", phase4 + ":" + name)
    if (digest(phase3_data) != reviewed["phase3_sha256"]
            or digest(phase4_data) != reviewed["phase4_sha256"]):
        raise ValueError("Reviewed carried-structure source changed")
    reviewed_diff = "".join(difflib.unified_diff(
        phase3_data.decode().splitlines(True), phase4_data.decode().splitlines(True),
        fromfile="phase3/" + name, tofile="phase4/" + name)).encode()
    if (reviewed["review_diff_algorithm"] != "stdlib-difflib-unified-context3-phase3-phase4-v1"
            or digest(reviewed_diff) != reviewed["review_diff_sha256"]):
        raise ValueError("Reviewed carried-structure diff changed")


def verify_input_bytes(script, expected_script, expected_inputs):
    if digest(script.read_bytes()) != expected_script:
        raise ValueError("Assembler bytes changed during assembly")
    for name, expected in expected_inputs.items():
        if digest((script.parent / name).read_bytes()) != expected:
            raise ValueError("Local input bytes changed during assembly: " + name)


def live_fingerprint(git):
    path = Path(git.text("rev-parse", "--git-path", "index"))
    if not path.is_absolute():
        path = git.repo / path
    return {"head": git.text("rev-parse", "HEAD"),
            "index_sha256": digest(path.read_bytes()) if path.exists() else None}


def diff(git, commit, exclude=()):
    return git.run("diff", "--binary", "--full-index", "--no-renames",
                   "--no-ext-diff", "--no-textconv", BASE, commit, "--", ".",
                   *(":(exclude)" + name for name in exclude))


def apply(git, patch):
    git.run("apply", "--cached", "--check", "--whitespace=nowarn", "-", data=patch)
    git.run("apply", "--cached", "--whitespace=nowarn", "-", data=patch)


def materialize(git, entries, destination):
    """Read exact blobs, bypassing checkout filters, archive hooks and executable code."""
    destination.mkdir()
    process = subprocess.Popen([*git.command, "cat-file", "--batch"], env=git.env,
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    manifest, total = [], 0
    try:
        for name, (mode, oid) in sorted(entries.items()):
            process.stdin.write((oid + "\n").encode())
            process.stdin.flush()
            header = process.stdout.readline().decode().split()
            if len(header) != 3 or header[:2] != [oid, "blob"]:
                raise RuntimeError("Unexpected cat-file response")
            length = int(header[2])
            if length > 128 * 1024**2 or total + length > 512 * 1024**2:
                raise ValueError("Unexpected materialized tree size; review before increasing limits")
            data = process.stdout.read(length)
            if len(data) != length or process.stdout.read(1) != b"\n":
                raise RuntimeError("Truncated Git blob")
            path = destination.joinpath(*safe_path(name).parts)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            path.chmod(0o755 if mode == "100755" else 0o644)
            manifest.append({"path": name, "mode": mode, "git_blob": oid,
                             "sha256": digest(data), "bytes": length})
            total += length
        process.stdin.close()
        if process.wait() != 0:
            raise RuntimeError("cat-file failed")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        if not process.stdin.closed:
            process.stdin.close()
        process.stdout.close()
    return manifest, total


def assemble(repo, pins_path, output):
    live = Git(repo)
    pins = load_pins(pins_path, live)  # Null/floating pins fail before creating any output.
    allowed = (repo / ".improve/evidence/finish-validation").resolve()
    if not allowed.is_relative_to(repo) or not output.is_relative_to(allowed) or output == allowed:
        raise ValueError("Output must be a new child of this checkout's private .improve/evidence/finish-validation folder")
    output.mkdir(parents=True, exist_ok=False)
    git = Git(repo, output / "assembly.index")
    before = live_fingerprint(live)
    labels_before = snapshot_refs(live, pins)
    input_files = ("MAC-CARRIED-RESOLUTION.json", "IOS-CARRIED-RESOLUTION.json",
                       "P6-IOS-UPLOADS-RESOLUTION.json", "P6-IOS-API-GUARD.json",
                       "P6-RELAY-RESOLUTION.json", "P7-PEOPLE-RESOLUTION.json",
                       "p6-relay.patch", "p6-ios-uploads.patch", "p7-people.patch",
                       "CI-RESOLUTION.json", "p9-ci.patch")
    report = {"base": BASE, "pins": pins, "phases": [], "status": "assembling",
              "assembler_sha256": digest(Path(__file__).read_bytes()),
              "input_policy": "Immutable pinned commits; branch names are snapshot labels only",
              "observed_snapshot_refs_before": labels_before,
              "inputs_sha256": {name: digest((Path(__file__).parent / name).read_bytes())
                                for name in input_files}}
    try:
        baseline = live.tree(BASE)
        phase_trees = {p["phase"]: live.tree(p["commit"]) for p in pins["phases"]}
        for phase, expected in REVIEWED_REDACTION.items():
            commit = pins["phases"][phase - 3]["commit"]
            if digest(live.run("show", commit + ":" + REDACT)) != expected:
                raise ValueError("A reviewed redaction input changed; review a new resolution")
        ci_resolution = json.loads((Path(__file__).parent / "CI-RESOLUTION.json").read_text())
        ci_name = ci_resolution["path"]
        if ci_name != ".github/workflows/ci.yml" or ci_resolution["mode"] != "100644":
            raise ValueError("Unexpected reviewed CI resolution path/mode")
        if any(ci_resolution[f"phase{number}_sha256"] != ci_resolution["phase3_sha256"]
               for number in (6, 7, 8)):
            raise ValueError("Standalone Phase6/7/8 CI must equal reviewed Phase3 safety")
        if set(ci_resolution["common_fences"]) != set(RUST_FENCES):
            raise ValueError("Unexpected common Rust fence paths")
        for name, reviewed in ci_resolution["common_fences"].items():
            for number in range(3, 10):
                entry = phase_trees[number].get(name)
                if (entry is None or entry != phase_trees[3].get(name)
                        or entry[0] != reviewed["mode"]):
                    raise ValueError("Common Rust fence blob/mode differs: " + name)
            source = live.run("show", pins["phases"][0]["commit"] + ":" + name)
            if digest(source) != reviewed["sha256"]:
                raise ValueError("Reviewed common Rust fence bytes differ: " + name)
        for name in SWIFT_DUPLICATES:
            if (name not in phase_trees[3]
                    or any(phase_trees[number].get(name) != phase_trees[3][name]
                           for number in (4, 6, 7, 8, 9))):
                raise ValueError(f"Phase 3/4/6/7/8/9 Swift fence differs: {name}")
        resolution = live.run("show", pins["phases"][-1]["commit"] + ":" + RESOLUTION)
        if digest(resolution) != RESOLUTION_SHA256:
            raise ValueError("Public canonical redaction resolution patch changed")
        git.run("read-tree", "--no-sparse-checkout", BASE)
        touched = set()
        carried = json.loads((Path(__file__).parent / "MAC-CARRIED-RESOLUTION.json").read_text())
        ios_carried = json.loads((Path(__file__).parent / "IOS-CARRIED-RESOLUTION.json").read_text())
        if (set(carried) - {"scope"}) & (set(ios_carried) - {"scope"}):
            raise ValueError("Duplicate explicit carried resolution path")
        carried.update({name: value for name, value in ios_carried.items() if name != "scope"})
        if ci_name in carried:
            raise ValueError("Duplicate explicit carried CI resolution")
        carried[ci_name] = ci_resolution
        carried_paths = [name for name in carried if name != "scope"]
        uploads_resolution = json.loads((Path(__file__).parent / "P6-IOS-UPLOADS-RESOLUTION.json").read_text())
        uploads_name = uploads_resolution["path"]
        api_guard = json.loads((Path(__file__).parent / "P6-IOS-API-GUARD.json").read_text())
        api_name = api_guard["path"]
        relay_resolution = json.loads((Path(__file__).parent / "P6-RELAY-RESOLUTION.json").read_text())
        relay_name = relay_resolution["relay_path"]
        for phase in pins["phases"]:
            number, commit = phase["phase"], phase["commit"]
            current_tree = git.tree(git.text("write-tree"))
            excluded = []
            if number == 8:
                excluded.append(REDACT)
            if number > 3:
                for name in IDENTICAL_FENCES:
                    if name in phase_trees[number] and phase_trees[number].get(name) != baseline.get(name):
                        if phase_trees[number][name] != phase_trees[3][name] or current_tree.get(name) != phase_trees[3][name]:
                            raise ValueError("Changed test fence, not an identical duplicate: " + name)
                        excluded.append(name)
            if number == 4:
                # Only byte-and-mode identical Phase3/4 changes are carried once, across
                # all paths. Differing overlaps still require explicit reviewed resolutions.
                for name in baseline.keys() | phase_trees[3].keys():
                    if name in excluded:
                        continue
                    if baseline.get(name) != phase_trees[3].get(name) and phase_trees[3].get(name) == phase_trees[4].get(name):
                        if current_tree.get(name) != phase_trees[4].get(name):
                            raise ValueError("Expected exact carried Phase3 blob is absent")
                        excluded.append(name)
            if number == 4:
                for name in carried_paths:
                    verify_carried(live, pins["phases"][0]["commit"], commit, name, carried[name])
                    if current_tree.get(name) != phase_trees[3].get(name):
                        raise ValueError("Carried-structure resolution preimage is not Phase3")
                    if "mode" in carried[name] and any(phase_trees[n][name][0] != carried[name]["mode"] for n in (3, 4)):
                        raise ValueError("Reviewed carried-structure mode changed")
                    excluded.append(name)
            if number in (5, 6, 7, 8, 9):
                expected = ci_resolution[f"phase{number}_sha256"]
                if (current_tree.get(ci_name, (None,))[0] != "100644"
                        or phase_trees[number].get(ci_name, (None,))[0] != "100644"
                        or digest(git.run("show", ":" + ci_name)) != ci_resolution["phase4_sha256"]
                        or digest(live.run("show", commit + ":" + ci_name)) != expected):
                    raise ValueError("Reviewed CI safety/free-fix input changed")
                excluded.append(ci_name)
            if number == 6:
                if digest(git.run("show", ":" + relay_name)) != relay_resolution["phase4_sha256"] or digest(live.run("show", commit + ":" + relay_name)) != relay_resolution["phase6_sha256"]:
                    raise ValueError("Reviewed P6 relay resolution preimage changed")
                if current_tree[relay_name][0] != "100644" or phase_trees[6][relay_name][0] != "100644":
                    raise ValueError("Reviewed P6 relay mode changed")
                excluded.append(relay_name)
                if digest(git.run("show", ":" + uploads_name)) != uploads_resolution["phase4_sha256"] or digest(live.run("show", commit + ":" + uploads_name)) != uploads_resolution["phase6_sha256"]:
                    raise ValueError("Reviewed P6 upload resolution preimage changed")
                if current_tree[uploads_name][0] != "100644" or phase_trees[6][uploads_name][0] != "100644":
                    raise ValueError("Reviewed P6 upload mode changed")
                excluded.append(uploads_name)
                if digest(git.run("show", ":" + api_name)) != api_guard["phase4_sha256"] or digest(live.run("show", commit + ":" + api_name)) != api_guard["phase6_sha256"]:
                    raise ValueError("Reviewed P6 API input changed")
                if current_tree[api_name][0] != api_guard["mode"] or phase_trees[6][api_name][0] != api_guard["mode"]:
                    raise ValueError("Reviewed P6 API mode changed")
            if number == 7:
                people_resolution = json.loads((Path(__file__).parent / "P7-PEOPLE-RESOLUTION.json").read_text())
                people_name = people_resolution["path"]
                if digest(git.run("show", ":" + people_name)) != people_resolution["preimage_sha256"] or digest(live.run("show", commit + ":" + people_name)) != people_resolution["phase7_sha256"]:
                    raise ValueError("Reviewed P7 people resolution preimage changed")
                if current_tree[people_name][0] != "100644" or phase_trees[7][people_name][0] != "100644":
                    raise ValueError("Reviewed P7 people mode changed")
                excluded.append(people_name)
            excluded = tuple(sorted(excluded))
            changed = sorted(p for p in baseline.keys() | phase_trees[number].keys()
                             if baseline.get(p) != phase_trees[number].get(p))
            touched.update(changed)
            for name in excluded:
                if (name != REDACT
                        and not (number == 4 and name in carried_paths)
                        and not (number == 6 and name in (relay_name, uploads_name))
                        and not (number == 7 and name == people_name)
                        and not (number in (5, 6, 7, 8, 9) and name == ci_name)
                        and (current_tree.get(name) != phase_trees[number].get(name) or name not in changed)):
                    raise ValueError("Expected identical carried source is absent or modified")
            full, effective = diff(live, commit), diff(live, commit, excluded)
            (output / f"phase-{number}-full.patch").write_bytes(full)
            (output / f"phase-{number}-apply.patch").write_bytes(effective)
            apply(git, effective)  # Any other conflict stops; no three-way/reject/ours/theirs fallback.
            if number == 4:
                for name in carried_paths:
                    mode, oid = phase_trees[4][name]
                    git.run("update-index", "--add", "--cacheinfo", mode, oid, name)
            if number == 6:
                for name, reviewed, patch_name in ((relay_name, relay_resolution, "p6-relay.patch"),
                                                   (uploads_name, uploads_resolution, "p6-ios-uploads.patch")):
                    if reviewed["patch"] != patch_name:
                        raise ValueError("Unexpected P6 resolution asset name")
                    resolution_patch = (Path(__file__).parent / patch_name).read_bytes()
                    if digest(resolution_patch) != reviewed["patch_sha256"]:
                        raise ValueError("Reviewed P6 resolution patch changed")
                    apply(git, resolution_patch)
                    if digest(git.run("show", ":" + name)) != reviewed["output_sha256"]:
                        raise ValueError("Reviewed P6 resolution output changed")
                if digest(git.run("show", ":" + api_name)) != api_guard["output_sha256"]:
                    raise ValueError("Normal P6 API application differs from reviewed compiled result")
            if number == 7:
                if people_resolution["patch"] != "p7-people.patch":
                    raise ValueError("Unexpected P7 resolution asset name")
                resolution_patch = (Path(__file__).parent / "p7-people.patch").read_bytes()
                if digest(resolution_patch) != people_resolution["patch_sha256"]:
                    raise ValueError("Reviewed P7 people patch changed")
                apply(git, resolution_patch)
                if digest(git.run("show", ":" + people_name)) != people_resolution["output_sha256"]:
                    raise ValueError("Reviewed P7 people output changed")
            if number == 9:
                if ci_resolution["patch"] != "p9-ci.patch":
                    raise ValueError("Unexpected Phase9 CI delta asset name")
                ci_patch = (Path(__file__).parent / "p9-ci.patch").read_bytes()
                if digest(ci_patch) != ci_resolution["patch_sha256"]:
                    raise ValueError("Reviewed Phase9 CI free-fix delta changed")
                apply(git, ci_patch)
                if digest(git.run("show", ":" + ci_name)) != ci_resolution["output_sha256"]:
                    raise ValueError("Combined fenced CI/prefetch/free-fix output changed")
            item = {"phase": number, "commit": commit, "changed_paths": changed,
                    "full_patch_sha256": digest(full), "apply_patch_sha256": digest(effective),
                    "excluded_paths": list(excluded)}
            if number == 4:
                item["exact_identical_carried_paths"] = [name for name in excluded if name not in carried_paths]
                item["explicit_superset_paths"] = {name: carried[name] for name in carried_paths}
            if number == 6:
                item["reviewed_resolutions"] = {"relay": relay_resolution, "uploads": uploads_resolution, "api": api_guard}
            if number == 7:
                item["reviewed_resolution"] = people_resolution
            if number == 9:
                item["reviewed_ci_resolution"] = ci_resolution
            if number == 8:
                current_redact = git.run("show", ":" + REDACT)
                if digest(current_redact) != REVIEWED_REDACTION[6]:
                    raise ValueError("Canonical patch no longer applies to the reviewed cached input")
                (output / "reviewed-redaction-resolution.patch").write_bytes(resolution)
                apply(git, resolution)
                if digest(git.run("show", ":" + REDACT)) != CANONICAL:
                    raise ValueError("Canonical redaction result differs from reviewed bytes")
                item["resolution_sha256"] = digest(resolution)
                item["resolved_redact_sha256"] = CANONICAL
            item["tree"] = git.text("write-tree")
            report["phases"].append(item)
        tree = git.text("write-tree")
        final = git.tree(tree)
        for name, entry in baseline.items():
            if name not in touched and final.get(name) != entry:
                raise ValueError("Untouched baseline file changed or disappeared: " + name)
            if name.startswith("assets/") and final.get(name) != entry:
                raise ValueError("A baseline asset changed or disappeared: " + name)
        if final.get("Cargo.lock") != baseline.get("Cargo.lock"):
            raise ValueError("Unexpected root Cargo.lock change")
        if digest(git.run("show", tree + ":" + REDACT)) != CANONICAL:
            raise ValueError("A later phase altered the reviewed redaction result")
        reviewed_outputs = {name: value["phase4_sha256"] for name, value in carried.items() if name != "scope"}
        reviewed_outputs.update({relay_name: relay_resolution["output_sha256"],
                                 uploads_name: uploads_resolution["output_sha256"],
                                 api_name: api_guard["output_sha256"],
                                 people_name: people_resolution["output_sha256"],
                                 ci_name: ci_resolution["output_sha256"], REDACT: CANONICAL})
        for name, expected in reviewed_outputs.items():
            if final[name][0] != "100644" or digest(git.run("show", tree + ":" + name)) != expected:
                raise ValueError("A later phase changed a reviewed resolution: " + name)
        report["reviewed_final_outputs"] = reviewed_outputs
        manifest, size = materialize(git, final, output / "workspace")
        manifest_bytes = (json.dumps(manifest, indent=2) + "\n").encode()
        (output / "source-manifest.json").write_bytes(manifest_bytes)
        if load_pins(pins_path, live) != pins:
            raise ValueError("Input pins changed during assembly")
        labels_after = snapshot_refs(live, pins)
        if labels_after != labels_before:
            raise ValueError("Snapshot label moved during assembly")
        report["observed_snapshot_refs_after"] = labels_after
        if live_fingerprint(live) != before:
            raise ValueError("Live HEAD/index changed concurrently; inspect and assemble from a frozen state")
        verify_input_bytes(Path(__file__), report["assembler_sha256"], report["inputs_sha256"])
        report["local_input_bytes_unchanged"] = True
        report.update(status="assembled; not tested or published", tree=tree,
                      source_files=len(final), source_bytes=size,
                      source_manifest_sha256=digest(manifest_bytes),
                      baseline_files=len(baseline), untouched_baseline_files=len(baseline.keys() - touched),
                      canonical_redact_sha256=CANONICAL, live_head_index_unchanged=True,
                      baseline_assets_preserved=True, intended_validation_parent=BASE,
                      intended_validation_branch="improve/finish-validation")
    except Exception as error:
        report.update(status="FAILED; do not publish", error=str(error))
        raise
    finally:
        (output / "PROVENANCE.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--pins", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    start = time.monotonic()
    result = assemble(args.repo.resolve(), args.pins.resolve(), args.output.resolve())
    print(json.dumps({"tree": result["tree"], "files": result["source_files"],
                      "bytes": result["source_bytes"], "seconds": time.monotonic() - start,
                      "status": result["status"]}))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        print("Assembly refused: " + str(error), file=sys.stderr)
        sys.exit(1)
