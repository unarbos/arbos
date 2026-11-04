#!/usr/bin/env python3
"""One fix per release: refuse a release-path merge while main still owes a release.

    scripts/release/release-gate.py --pr-base <commit> --pr-head <commit> [options]

Exit 0: merging this pull request publishes nothing that is already owed, so merge away.
Exit 1: blocked — main carries a release-path commit that has not been released yet.
Exit 2: this check could not decide, so it fails closed. Not a verdict about the pull request.

Why this exists
---------------
A merge to main publishes to users. A merge whose diff touches the trigger paths of
.github/workflows/ios.yml cuts a public TestFlight build and submits it for Beta App Review. A
merge the Mac release predicates call a Mac change cuts a signed, notarized stable release, marks
it latest, and pings every installed Mac app. The owner's rule is one fix per release: one fix per
TestFlight build, one fix per `vX.Y.Z` tag.

Nothing enforced that rule. .github/workflows/release.yml releases everything since the last tag
in one batch, on a 15-minute schedule, so two unreleased fixes sitting on main at the same time
are silently folded into one release and one of them never gets a build of its own. This check
makes the rule structural: while an artifact is outstanding, the next pull request on that same
path waits.

The two sides are not symmetric, and reproducing either one wrongly is the failure mode
-----------------------------------------------------------------------------------------
Mac: whether a difference cuts a Mac release is decided by scripts/release/mac-change.sh and the
lockfile reader it leans on, scripts/release/cargo-lock-change.py. A plain diff of the Mac paths
is not the same predicate and gets a different answer: the release bump commit's twelve lines of
`version = "0.6.379"` becoming `version = "0.6.380"` look like a Mac change to a path diff and are
correctly not one. This file never decides that question itself; it runs the script.

iOS: ios.yml has no such intelligence. It is a plain GitHub path filter, and it lists Cargo.lock
outright, so the bump commit that is not a Mac change IS a TestFlight build. The filter is read
out of the workflow file at the commit being judged, so this check cannot drift away from it.

What counts as released
-----------------------
Mac: the last stable `v*` tag, against main. auto-stable.sh pushes the tag before it builds, so a
tag is the moment the release is claimed; scripts/release/stable-tag-base.sh gives both the tag
and the commit the app was built from, and is the same code the release itself uses.

iOS: the newest push run of ios.yml on main that concluded success. That run is the upload, so a
build that is still archiving, still processing, or red has not reached a tester and the commit
that caused it is still owed.

Blocking is per artifact. A Mac-only pull request waits for a Mac release and ignores TestFlight;
a pull request on both paths (crates/arbos-core is in both sets) waits while either is
outstanding; a pull request on neither always passes without reading anything.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tomllib
import urllib.error
import urllib.request
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

import yaml

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
IOS_WORKFLOW = ".github/workflows/ios.yml"
GATE_WORKFLOW = ".github/workflows/release-gate.yml"
PROBE = "__release_gate_coverage_probe__"

ALLOW, BLOCK, UNDETERMINED = 0, 1, 2

# How many commits of main to walk when naming the one that made main dirty. The verdict never
# depends on this: it is already known by then, and a longer history only costs the message its
# precision.
NAME_SCAN = 60
# How many ios.yml runs to read before admitting that no published build was found. "Nothing is
# there" must not stand in for "the window was too small", so running out is UNDETERMINED.
RUN_WINDOW = 60

MAC_ARTIFACT = "a stable Mac release (signed, notarized, marked latest, pushed to every installed Mac app)"
IOS_ARTIFACT = "a TestFlight build (uploaded to the public group and submitted for Beta App Review)"


class Undetermined(Exception):
    """The gate could not answer its own question. Never a verdict about the pull request."""


# ── the repository, read rather than reproduced ─────────────────────────────────────────────────


def git(*args: str) -> str:
    done = subprocess.run(["git", *args], capture_output=True, text=True, check=False)
    if done.returncode != 0:
        raise Undetermined(f"`git {' '.join(args)}` failed: {done.stderr.strip()[:300]}")
    return done.stdout


def commit(ref: str) -> str:
    """`ref` as a full sha, or UNDETERMINED. An abbreviated or missing ref is never guessed at."""
    sha = git("rev-parse", "--verify", f"{ref}^{{commit}}").strip()
    if len(sha) != 40:
        raise Undetermined(f"{ref!r} does not name a commit in this checkout")
    return sha


def mac_change(base: str, target: str) -> tuple[bool, str]:
    """The repository's own answer to "does this difference cut a Mac release?", and its reason.

    scripts/release/mac-change.sh follows `git diff --quiet`: 0 is no change, 1 is a change.
    Anything else is the script failing, not a verdict, so it is UNDETERMINED.
    """
    done = subprocess.run(
        [str(HERE / "mac-change.sh"), base, target], capture_output=True, text=True, check=False
    )
    reason = done.stdout.strip()
    if done.returncode in (0, 1):
        return done.returncode == 1, reason
    detail = (reason or done.stderr.strip() or "no output")[:300]
    raise Undetermined(
        f"mac-change.sh exited {done.returncode} for {base[:8]}..{target[:8]}: {detail}"
    )


def trigger_paths(text: str, workflow: str, event: str, where: str) -> list[str]:
    """A workflow's literal `on.<event>.paths` list, parsed rather than grepped."""
    try:
        doc = yaml.safe_load(text)
    except yaml.YAMLError as err:
        raise Undetermined(f"{workflow} at {where} is not readable YAML: {err}") from err
    # YAML 1.1 reads a bare `on` key as the boolean true, which is what `on:` becomes here.
    triggers = doc.get("on", doc.get(True)) if isinstance(doc, dict) else None
    trigger = triggers.get(event) if isinstance(triggers, dict) else None
    paths = trigger.get("paths") if isinstance(trigger, dict) else None
    if not isinstance(paths, list) or not paths:
        raise Undetermined(f"{workflow} at {where} has no `on.{event}.paths` list to read")
    patterns = [str(p) for p in paths]
    unreadable = [p for p in patterns if p.startswith(("!", "+"))]
    if unreadable:
        raise Undetermined(
            f"{workflow} at {where} has pattern(s) this gate cannot evaluate: "
            f"{', '.join(unreadable)}. Teach it the syntax rather than letting it guess."
        )
    return patterns


def ios_trigger_paths(ref: str) -> list[str]:
    """The literal path filter that decides whether a push to main cuts a TestFlight build."""
    return trigger_paths(git("show", f"{ref}:{IOS_WORKFLOW}"), IOS_WORKFLOW, "push", ref[:8])


def mac_paths() -> list[str]:
    """Everything the Mac app is built from, from the one file that owns the list."""
    done = subprocess.run(
        [str(HERE / "mac-paths.sh")], capture_output=True, text=True, check=False
    )
    if done.returncode != 0:
        raise Undetermined(f"mac-paths.sh failed: {done.stderr.strip()[:200]}")
    return done.stdout.split()


def mac_binary_closure() -> set[str]:
    """Every workspace crate the Mac binary links, read off the manifests.

    `cargo tree -p arbos-mac` is the authority, but it needs a resolved workspace and about
    fifteen seconds. The manifests give the same closure for the question being asked — which of
    our own crates end up in Arbos.app — because a path dependency is a path dependency. Dev
    dependencies are left out: they build tests, not the shipped binary.
    """
    manifests: dict[str, set[str]] = {}
    for path in sorted((ROOT / "crates").glob("*/Cargo.toml")):
        try:
            data = tomllib.loads(path.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError) as err:
            raise Undetermined(f"{path} is not readable: {err}") from err
        name = data.get("package", {}).get("name")
        if not name:
            continue
        tables = [data, *data.get("target", {}).values()]
        manifests[name] = {
            dep
            for table in tables
            for section in ("dependencies", "build-dependencies")
            for dep in table.get(section, {})
            if dep.startswith("arbos-")
        }
    if "arbos-mac" not in manifests:
        raise Undetermined("crates/arbos-mac/Cargo.toml does not name the package arbos-mac")
    linked: set[str] = set()
    pending = ["arbos-mac"]
    while pending:
        crate = pending.pop()
        if crate in linked or crate not in manifests:
            continue
        linked.add(crate)
        pending += sorted(manifests[crate])
    return linked


def assert_mac_paths_is_the_binary_closure() -> None:
    """What ships in the Mac binary must be what cuts a Mac release.

    MAC_PATHS was a trigger list that had drifted from the binary: arbos-client and arbos-matrix
    were linked into Arbos.app and absent from it, so a fix in either shipped to Mac users with no
    release cut for it and no release notes naming it. That is a silent ship, not a late one. This
    makes the list answerable to the manifests instead of to memory.
    """
    watched = {p.split("/", 2)[1] for p in mac_paths() if p.startswith("crates/")}
    missing = sorted(mac_binary_closure() - watched)
    if missing:
        raise Undetermined(
            f"the Mac binary links {', '.join(missing)}, which scripts/release/mac-paths.sh does "
            "not watch, so a fix in any of them would ship to Mac users with no release cut for "
            "it. Add them to mac-paths.sh."
        )


def assert_trigger_covers_both_predicates() -> None:
    """This workflow must run whenever either predicate could say yes.

    The gate decides with the repository's own predicates, but it only decides at all when GitHub
    starts it, and that is a static `paths:` list in a third file. A path the predicates watch and
    the trigger does not is a pull request the gate never sees — a silent miss, which is the whole
    failure this gate exists to end. So the trigger list is checked against both predicates on
    every run, out of the working tree, and a gap fails the gate rather than passing quietly.

    The trigger list also names ios.yml, mac-paths.sh and this workflow, so any pull request that
    moves either predicate runs this check and cannot widen one without widening the other.
    """
    gate = (ROOT / GATE_WORKFLOW).read_text(encoding="utf-8")
    covered = [matcher(p) for p in trigger_paths(gate, GATE_WORKFLOW, "pull_request", "this tree")]
    ios = (ROOT / IOS_WORKFLOW).read_text(encoding="utf-8")
    watched = [(IOS_WORKFLOW, p) for p in trigger_paths(ios, IOS_WORKFLOW, "push", "this tree")]
    watched += [("mac-paths.sh", p) for p in mac_paths()]

    gaps: list[str] = []
    for source, entry in watched:
        if entry.endswith("/**"):
            probe = f"{entry[:-2]}{PROBE}"
        elif "*" in entry:
            gaps.append(f"{entry} (from {source}) is a shape this check cannot probe")
            continue
        elif (ROOT / entry).is_dir():
            probe = f"{entry}/{PROBE}"
        else:
            probe = entry
        if not any(m.match(probe) for m in covered):
            gaps.append(f"{entry} (from {source})")
    if gaps:
        raise Undetermined(
            f"{GATE_WORKFLOW}'s own `paths:` trigger does not cover what the release predicates "
            f"watch, so a pull request touching {'; '.join(gaps)} would never start this gate. "
            "Add those paths to the trigger."
        )


def matcher(pattern: str) -> re.Pattern[str]:
    """A GitHub path filter pattern as a regex: `**` crosses directories, `*` and `?` do not."""
    out: list[str] = []
    i = 0
    while i < len(pattern):
        if pattern.startswith("**", i):
            out.append(".*")
            i += 2
        elif pattern[i] == "*":
            out.append("[^/]*")
            i += 1
        elif pattern[i] == "?":
            out.append("[^/]")
            i += 1
        else:
            out.append(re.escape(pattern[i]))
            i += 1
    return re.compile("".join(out) + r"\Z")


def changed_files(base: str, target: str) -> list[str]:
    return [line for line in git("diff", "--name-only", base, target).splitlines() if line]


def ios_hits(patterns: list[str], files: list[str]) -> list[str]:
    matchers = [matcher(p) for p in patterns]
    return [f for f in files if any(m.match(f) for m in matchers)]


def subject(sha: str) -> str:
    return git("log", "-1", "--format=%h %s", sha).strip()


# ── what main already owes ──────────────────────────────────────────────────────────────────────


@dataclass
class Published:
    """The head of the last artifact that actually reached users, and what came after it."""

    sha: str
    detail: str
    unfinished: list[str] = field(default_factory=list)


def last_stable_release() -> Published:
    lines = dict(
        line.split("=", 1)
        for line in subprocess.run(
            [str(HERE / "stable-tag-base.sh")], capture_output=True, text=True, check=False
        ).stdout.splitlines()
        if "=" in line
    )
    tag, base = lines.get("tag", ""), lines.get("base", "")
    if not tag or not base:
        raise Undetermined(
            "no stable v*.*.* tag is readable in this checkout, so what main owes cannot be "
            "measured. Fetch the tags."
        )
    return Published(sha=commit(base), detail=f"{tag}, built from {base[:8]}")


def last_testflight_build(repo: str, token: str) -> Published:
    """The newest ios.yml push run on main that concluded success — the last build testers saw."""
    url = (
        f"https://api.github.com/repos/{repo}/actions/workflows/ios.yml/runs"
        f"?branch=main&event=push&per_page={RUN_WINDOW}"
    )
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "arbos-release-gate",
            **({"Authorization": f"Bearer {token}"} if token else {}),
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310 - fixed host
            runs = json.loads(response.read()).get("workflow_runs")
    except (urllib.error.URLError, OSError, ValueError) as err:
        raise Undetermined(f"could not read ios.yml's runs from the GitHub API: {err}") from err
    if not isinstance(runs, list) or not runs:
        raise Undetermined(
            "the GitHub API reported no ios.yml push runs on main at all, which cannot be true; "
            "the query or the token is wrong."
        )
    runs.sort(key=lambda run: run.get("run_number", 0), reverse=True)
    unfinished: list[str] = []
    for run in runs:
        head = str(run.get("head_sha", ""))
        if run.get("status") == "completed" and run.get("conclusion") == "success":
            return Published(
                sha=head, detail=f"built from {head[:8]} by ios.yml run {run.get('id')}",
                unfinished=unfinished,
            )
        state = run.get("conclusion") or run.get("status") or "unknown"
        unfinished.append(f"run {run.get('id')} on {head[:8]} ({state})")
    raise Undetermined(
        f"none of the last {len(runs)} ios.yml push runs on main uploaded a build, so the last "
        "one that reached TestFlight is outside the window this check reads. Widen RUN_WINDOW or "
        "get ios.yml green on main."
    )


def names_the_commit(published: str, main: str, dirty: Callable[[str], bool]) -> str:
    """The first commit after `published` that made main dirty, in main's first-parent order.

    `dirty(sha)` answers "is main dirty as of this commit?". The earliest commit for which it is
    true is the one the next release is waiting for.
    """
    try:
        shas = git("rev-list", "--reverse", "--first-parent", f"{published}..{main}").split()
    except Undetermined:
        return "a commit on main since then"
    for sha in shas[:NAME_SCAN]:
        if dirty(sha):
            return subject(sha)
    if len(shas) > NAME_SCAN:
        return f"one of the {len(shas)} commits on main since then (looked at the first {NAME_SCAN})"
    return "a commit on main since then"


# ── the verdict ─────────────────────────────────────────────────────────────────────────────────


@dataclass
class Side:
    """One artifact's answer: does the pull request publish it, and does main already owe one?"""

    artifact: str
    on_path: bool
    why: str
    outstanding: bool = False
    published: str = ""
    unreleased: str = ""
    reason: str = ""
    note: str = ""


def mac_side(pr_base: str, pr_head: str, main: str, override: Published | None) -> Side:
    changes, why = mac_change(pr_base, pr_head)
    side = Side(artifact=MAC_ARTIFACT, on_path=changes, why=why)
    if not changes:
        return side
    released = override or last_stable_release()
    side.published = released.detail
    side.outstanding, side.reason = mac_change(released.sha, main)
    if side.outstanding:
        side.unreleased = names_the_commit(
            released.sha, main, lambda sha: mac_change(released.sha, sha)[0]
        )
    return side


def ios_side(
    pr_base: str, pr_head: str, main: str, repo: str, token: str, override: Published | None
) -> Side:
    patterns = ios_trigger_paths(pr_head)
    hits = ios_hits(patterns, changed_files(pr_base, pr_head))
    why = (
        f"matches ios.yml's trigger paths: {', '.join(hits[:3])}"
        if hits
        else f"none of ios.yml's {len(patterns)} trigger paths"
    )
    side = Side(artifact=IOS_ARTIFACT, on_path=bool(hits), why=why)
    if not hits:
        return side
    published = override or last_testflight_build(repo, token)
    side.published = published.detail
    if published.unfinished:
        side.note = "ios.yml runs since then that published nothing: " + ", ".join(
            published.unfinished[:3]
        )
    released_sha = commit(published.sha)
    if not git_contains(released_sha, main):
        raise Undetermined(
            f"the last published TestFlight build was made from {released_sha[:8]}, which is not "
            f"an ancestor of {main[:8]}. This checkout and that build do not describe the same "
            "main, so nothing read from either can be trusted."
        )
    main_patterns = ios_trigger_paths(main)
    dirty = ios_hits(main_patterns, changed_files(released_sha, main))
    side.outstanding = bool(dirty)
    side.reason = (
        f"unreleased on ios.yml's paths: {', '.join(dirty[:3])}"
        if dirty
        else "nothing on ios.yml's paths has changed since that build"
    )
    if dirty:
        side.unreleased = names_the_commit(
            released_sha,
            main,
            lambda sha: bool(ios_hits(main_patterns, changed_files(released_sha, sha))),
        )
    return side


def git_contains(ancestor: str, descendant: str) -> bool:
    done = subprocess.run(
        ["git", "merge-base", "--is-ancestor", ancestor, descendant],
        capture_output=True,
        text=True,
        check=False,
    )
    if done.returncode not in (0, 1):
        raise Undetermined(f"`git merge-base --is-ancestor` failed: {done.stderr.strip()[:200]}")
    return done.returncode == 0


def report(pr_base: str, pr_head: str, main: str, sides: list[Side]) -> tuple[int, list[str]]:
    out = [
        "Release gate — would merging this collapse two fixes into one release?",
        "",
        f"  pull request   {pr_base[:8]}..{pr_head[:8]}",
        f"  main           {main[:8]}",
        "",
    ]
    for side in sides:
        label = "YES" if side.on_path else "no"
        out.append(f"  publishes {side.artifact.split(' (')[0]}: {label} — {side.why}")
        if side.on_path:
            state = "OUTSTANDING" if side.outstanding else "clear"
            out.append(f"      last published: {side.published}")
            out.append(f"      main now:       {state} — {side.reason}")
            if side.note:
                out.append(f"      note:           {side.note}")
    out.append("")

    blocking = [side for side in sides if side.on_path and side.outstanding]
    if not blocking:
        out += [
            "PASS. Nothing this pull request publishes is already owed by main.",
            "",
        ]
        return ALLOW, out

    out.append("BLOCKED. main already owes a release on a path this pull request is also on.")
    out.append("")
    for side in blocking:
        out += [
            f"  outstanding artifact  {side.artifact}",
            f"  last published        {side.published}",
            f"  unreleased on main    {side.unreleased}",
            f"  why it is unreleased  {side.reason}",
            "",
        ]
    out += [
        "The owner's rule is one fix per release: one fix per TestFlight build, one fix per",
        "vX.Y.Z tag. release.yml batches everything since the last release into one artifact, so",
        "merging now would ship both fixes as one and neither would get a build of its own.",
        "",
        "Nothing about this pull request needs to change. Wait for the outstanding release to",
        "publish — a Mac release is claimed the moment release.yml pushes its vX.Y.Z tag, and a",
        "TestFlight build the moment ios.yml's run on main concludes success. Re-run this check",
        "once that has happened and it passes.",
        "",
    ]
    return BLOCK, out


def emit(lines: list[str], annotation: str, level: str) -> None:
    """The whole finding in the log and the job summary, and one line of it as an annotation."""
    text = "\n".join(lines)
    print(text)
    print(f"::{level} title=Release gate::{annotation}")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as handle:
            handle.write(f"### Release gate\n\n```\n{text}\n```\n")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--pr-base", required=True, help="the merge base of the pull request")
    parser.add_argument("--pr-head", required=True, help="the head commit of the pull request")
    parser.add_argument("--main", default="origin/main", help="the branch being merged into")
    parser.add_argument(
        "--repo", default=os.environ.get("GITHUB_REPOSITORY", "unarbos/arbos"), help="owner/name"
    )
    parser.add_argument(
        "--mac-released-sha",
        help="the commit the last stable Mac release was built from; default: the last v* tag",
    )
    parser.add_argument(
        "--ios-released-sha",
        help="the commit of the last TestFlight build; default: ios.yml's runs on main",
    )
    args = parser.parse_args(argv)
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN") or ""

    try:
        assert_mac_paths_is_the_binary_closure()
        assert_trigger_covers_both_predicates()
        pr_base, pr_head, main_sha = (
            commit(args.pr_base),
            commit(args.pr_head),
            commit(args.main),
        )
        mac_override = (
            Published(sha=commit(args.mac_released_sha), detail=f"{args.mac_released_sha[:8]}")
            if args.mac_released_sha
            else None
        )
        ios_override = (
            Published(sha=commit(args.ios_released_sha), detail=f"{args.ios_released_sha[:8]}")
            if args.ios_released_sha
            else None
        )
        sides = [
            mac_side(pr_base, pr_head, main_sha, mac_override),
            ios_side(pr_base, pr_head, main_sha, args.repo, token, ios_override),
        ]
        verdict, lines = report(pr_base, pr_head, main_sha, sides)
    except Undetermined as err:
        emit(
            [
                "Release gate — could not decide, so it fails closed.",
                "",
                f"  what failed   {err}",
                "",
                "This is this check's own problem. It is not a verdict about the pull request,",
                "and it is not another job flaking: nothing here reads another job's result.",
                "Re-run this check. If it fails the same way again, the gate itself is wrong —",
                "read scripts/release/release-gate.py rather than changing the pull request.",
                "",
            ],
            f"could not determine whether main owes a release: {err}",
            "error",
        )
        return UNDETERMINED

    if verdict == BLOCK:
        blocked = [side for side in sides if side.on_path and side.outstanding]
        emit(
            lines,
            "main owes "
            + " and ".join(side.artifact.split(" (")[0] for side in blocked)
            + "; "
            + "; ".join(f"{side.unreleased} is unreleased" for side in blocked)
            + ". The block clears when that release publishes.",
            "error",
        )
        return BLOCK
    emit(lines, "nothing this pull request publishes is already owed by main.", "notice")
    return ALLOW


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
