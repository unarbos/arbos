#!/usr/bin/env python3
"""Pick up in-app feedback from the Arbos desktop app, and answer it.

Jacob reports a problem from the app. The app writes the report into a
store it can reach, and a loop reads it here, records it where the loop's
agents can see it, and later writes back which build carries the fix.

Four verbs:

    poll    find reports nobody has taken yet; copy each to the rig and to
            the Project store; append a row to the ledger
    filed   record that a report was handed to another inbox (a kernel bug
            is still this loop's to answer)
    fixed   verify a pull request really merged green, then write the
            `fixed` marker back into the report's own folder
    show    what is known about one report, or all of them

This is the desktop app's side. `asc-feedback.py` beside it is the phone's,
reading App Store Connect. The two share this shape on purpose — a timer, a
dedupe list, a folder per report, a ledger, quiet when nothing is new — and
share nothing else.

Why a script in the repository rather than on a rig: a poller that lives only
on a rented host goes away with the host. This one is reviewed, versioned, and
runs anywhere the loop happens to be.

It has no dependencies beyond Python 3.9 and, for the two verbs that need
them, `arbos-kernel` and `gh` on PATH.

    desktop-feedback.py poll   --source arbos://arboslife/feedback/internal/feedback
    desktop-feedback.py fixed  2026-09-16-1 --pr 331 --what "the composer kept focus"
"""

from __future__ import annotations

import argparse
import base64
import binascii
import datetime as dt
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Iterable, Optional

# The kernel refuses to serve more of one file than this, and pages the rest
# through `tail` (crates/arbos-kernel/src/files.rs READ_CAP). A report whose
# screenshot crosses it would arrive as a broken image, so the poller says so
# loudly instead of saving half a PNG.
READ_CAP = 1024 * 1024

REPORT_NAME = "report.json"
SCREENSHOT_NAME = "screenshot.b64"
FIXED_NAME = "fixed.json"
FILED_NAME = "filed.json"

LEDGER_MARK = "<!-- poll.py appends rows above this line; do not remove -->"

# How many polls to let a picture arrive before taking the report without it.
PICTURE_PATIENCE = 2


class Incomplete(Exception):
    """The folder is still arriving. Not a fault: leave it for the next poll."""


# --------------------------------------------------------------------------
# Transport
#
# Two ways to reach the reports, because the loop and the reports are not
# always on the same machine:
#
#   a store address   `arbos://<machine>/<project>/<path>`, through the hub,
#                     using the `arbos-kernel store` verbs — authenticated by
#                     the hub token the kernel already reads, so this script
#                     holds no credential of its own
#   a directory       a plain path, for the machine that holds the reports
#                     itself and for tests
# --------------------------------------------------------------------------


class Transport:
    def list(self, sub: str = "") -> list[str]:
        raise NotImplementedError

    def read(self, rel: str) -> Optional[str]:
        raise NotImplementedError

    def write(self, rel: str, text: str) -> None:
        raise NotImplementedError

    def size(self, rel: str) -> Optional[int]:
        raise NotImplementedError


class DirTransport(Transport):
    def __init__(self, root: Path):
        self.root = root

    def _at(self, rel: str) -> Path:
        return self.root / rel if rel else self.root

    def list(self, sub: str = "") -> list[str]:
        at = self._at(sub)
        if not at.is_dir():
            return []
        return sorted(p.name + ("/" if p.is_dir() else "") for p in at.iterdir())

    def read(self, rel: str) -> Optional[str]:
        at = self._at(rel)
        try:
            return at.read_text()
        except OSError:
            return None

    def write(self, rel: str, text: str) -> None:
        at = self._at(rel)
        at.parent.mkdir(parents=True, exist_ok=True)
        at.write_text(text)

    def size(self, rel: str) -> Optional[int]:
        try:
            return self._at(rel).stat().st_size
        except OSError:
            return None


class StoreTransport(Transport):
    """`arbos-kernel store ls|read|put arbos://…`, through the hub."""

    def __init__(self, address: str, kernel: str = "arbos-kernel"):
        self.address = address.rstrip("/")
        self.kernel = kernel

    def _addr(self, rel: str) -> str:
        return f"{self.address}/{rel}" if rel else self.address

    def _run(self, verb: str, *rest: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            [self.kernel, "store", verb, *rest],
            capture_output=True,
            text=True,
            timeout=120,
        )

    def list(self, sub: str = "") -> list[str]:
        done = self._run("ls", self._addr(sub))
        if done.returncode != 0:
            # An absent folder is not an error: no reports yet is the normal
            # state, and a loop that treats it as a fault goes loud every
            # fifteen minutes for nothing.
            if "no such file" in done.stderr.lower() or "not found" in done.stderr.lower():
                return []
            raise RuntimeError(f"store ls {self._addr(sub)}: {done.stderr.strip()}")
        return [line for line in done.stdout.splitlines() if line.strip()]

    def read(self, rel: str) -> Optional[str]:
        done = self._run("read", self._addr(rel))
        if done.returncode != 0:
            return None
        if "bytes;" in done.stderr and "only" in done.stderr:
            raise RuntimeError(
                f"{rel} is past the {READ_CAP} byte read cap and came back cut: {done.stderr.strip()}"
            )
        return done.stdout

    def write(self, rel: str, text: str) -> None:
        # `store put` takes a file, so the text goes through one.
        import tempfile

        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
            f.write(text)
            tmp = f.name
        try:
            done = self._run("put", self._addr(rel), tmp)
            if done.returncode != 0:
                raise RuntimeError(f"store put {self._addr(rel)}: {done.stderr.strip()}")
        finally:
            os.unlink(tmp)

    def size(self, rel: str) -> Optional[int]:
        text = self.read(rel)
        return None if text is None else len(text)


def transport_for(source: str) -> Transport:
    if source.startswith("arbos://"):
        return StoreTransport(source)
    return DirTransport(Path(source).expanduser())


# --------------------------------------------------------------------------
# State
#
# `seen.json` lives on the rig, not in the Project store. The store has
# dropped directories twice today; losing the ledger costs a rewrite, but
# losing the dedupe list would replay every report Jacob has ever sent.
# --------------------------------------------------------------------------


def load_seen(rig: Path) -> dict:
    path = rig / "seen.json"
    try:
        state = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return {"ids": {}, "counts": {}, "waiting": {}}
    state.setdefault("ids", {})
    state.setdefault("counts", {})
    state.setdefault("waiting", {})
    return state


def save_seen(rig: Path, state: dict) -> None:
    rig.mkdir(parents=True, exist_ok=True)
    path = rig / "seen.json"
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(state, indent=1, sort_keys=True) + "\n")
    tmp.replace(path)


def next_name(state: dict, day: str) -> str:
    n = state["counts"].get(day, 0) + 1
    state["counts"][day] = n
    return f"{day}-{n}"


# --------------------------------------------------------------------------
# poll
# --------------------------------------------------------------------------


def utc_day(ms: Optional[int]) -> str:
    when = dt.datetime.fromtimestamp((ms or 0) / 1000, dt.timezone.utc) if ms else dt.datetime.now(dt.timezone.utc)
    return when.strftime("%Y-%m-%d")


def human_time(ms: Optional[int]) -> str:
    if not ms:
        return "unknown"
    return dt.datetime.fromtimestamp(ms / 1000, dt.timezone.utc).strftime("%Y-%m-%d %H:%M UTC")


def cmd_poll(args: argparse.Namespace) -> int:
    src = transport_for(args.source)
    rig = Path(args.rig).expanduser()
    state = load_seen(rig)

    folders = [e.rstrip("/") for e in src.list() if e.endswith("/")]
    fresh = [f for f in folders if f not in state["ids"]]

    if not fresh:
        # Quiet when nothing is new, so a fifteen-minute timer does not
        # spend a turn saying so.
        print("nothing new")
        return 0

    taken: list[dict] = []
    still_arriving: list[str] = []
    for report_id in sorted(fresh):
        try:
            record = take_one(src, report_id, rig, args, state)
        except Incomplete as e:
            # Expected, and quiet: it will be here next time.
            still_arriving.append(f"{report_id}: {e}")
            continue
        except Exception as e:  # noqa: BLE001 - one bad report must not stop the rest
            print(f"SKIPPED {report_id}: {e}", file=sys.stderr)
            continue
        taken.append(record)

    if taken or still_arriving:
        save_seen(rig, state)
    if taken and args.ledger:
        append_ledger(Path(args.ledger).expanduser(), taken)

    if not taken:
        for line in still_arriving:
            print(f"still arriving — {line}")
        return 0

    print(f"NEW {len(taken)} report(s)")
    for r in taken:
        mark = "" if r["real"] else "  [FIXTURE — not from the app, events may be invented]"
        print(f"  {r['name']}  build {r['build']}  {human_time(r['sent_ms'])}{mark}")
        print(f"    words: {r['note'] or '(none)'}")
        print(f"    kept: {r['kept']}")
        if r["missing"]:
            print(f"    he removed: {r['missing']}")
        if r["faults"]:
            print(f"    MISSING, not removed (a fault to chase): {r['faults']}")
        print(f"    rig:   {r['rig_path']}")
        if r["store_path"]:
            print(f"    store: {r['store_path']}")
    print()
    print(
        "Take these as the next thing in the cycle's plan. Do not abandon a run in\n"
        "flight: let it reach its gate and its pull request first, or half-gated work\n"
        "ships under his name."
    )
    return 0


def take_one(
    src: Transport,
    report_id: str,
    rig: Path,
    args: argparse.Namespace,
    state: dict,
) -> dict:
    raw = src.read(f"{report_id}/{REPORT_NAME}")
    if raw is None:
        raise RuntimeError(f"no {REPORT_NAME}")
    report = json.loads(raw)

    # The picture is fetched before a name is spent on the report.
    #
    # The app claims its folder with `report.json` and writes the picture after
    # it, so a poll can land between the two. A report that says it carries a
    # picture and has none has probably not finished arriving, so it is left for
    # the next poll rather than recorded as a report whose screenshot was lost.
    # `PICTURE_PATIENCE` polls later it is taken anyway, with the loss written
    # down: a report is worth more than its picture and must not be stuck
    # behind one for ever.
    #
    # And the wait happens *here*, before `next_name`, because a human number
    # must not be spent on a report that was not taken — waiting twice used to
    # make the first report `2026-09-16-3`.
    shot = None
    if report.get("included", {}).get("screenshot"):
        shot = fetch_screenshot(src, report_id)
        if shot is None:
            waited = state["waiting"].get(report_id, 0) + 1
            state["waiting"][report_id] = waited
            if waited <= PICTURE_PATIENCE:
                raise Incomplete(
                    f"the picture it says it carries has not arrived yet "
                    f"(poll {waited} of {PICTURE_PATIENCE}); leaving it"
                )
            report["screenshot_lost_in_transit"] = True
    state["waiting"].pop(report_id, None)

    sent_ms = report.get("sent_ms")
    name = next_name(state, utc_day(sent_ms))

    rig_dir = rig / name
    rig_dir.mkdir(parents=True, exist_ok=True)
    (rig_dir / REPORT_NAME).write_text(json.dumps(report, indent=1) + "\n")

    shot_written = False
    if shot is not None:
        raw, suffix = shot
        (rig_dir / f"screenshot.{suffix}").write_bytes(raw)
        shot_written = True

    summary = summarise(report, name, report_id, shot_written)
    (rig_dir / "feedback.md").write_text(summary)

    # The rig copy is the authoritative one. The store copy is for the
    # agents that read the Project store, and it is a copy on purpose: the
    # store has dropped directories twice today.
    store_path = None
    if args.store:
        store_dir = Path(args.store).expanduser() / name
        try:
            store_dir.mkdir(parents=True, exist_ok=True)
            for f in sorted(rig_dir.iterdir()):
                if f.is_file():
                    shutil.copy2(f, store_dir / f.name)
            store_path = str(store_dir)
        except OSError as e:
            print(f"WARNING {name}: the store copy failed ({e}); the rig copy stands", file=sys.stderr)

    state["ids"][report_id] = {
        "name": name,
        "taken_ms": int(dt.datetime.now(dt.timezone.utc).timestamp() * 1000),
        "sent_ms": sent_ms,
    }

    included = report.get("included", {})
    chosen = report.get("chose", included)
    kept = ", ".join(k for k, v in sorted(included.items()) if v) or "words only"
    # What he took out, and what went missing although he kept it. The second
    # is a fault worth chasing; the first is none of the loop's business.
    missing = ", ".join(k for k, v in sorted(chosen.items()) if not v)
    faults = ", ".join(
        k
        for k, v in sorted(included.items())
        if not v and chosen.get(k, v) is not False
    )

    return {
        "id": report_id,
        "name": name,
        "sent_ms": sent_ms,
        "note": (report.get("note") or "").strip().replace("\n", " ")[:200],
        "real": is_real(report),
        "build": build_label(report),
        "kept": kept,
        "missing": missing,
        "faults": faults,
        "rig_path": str(rig_dir),
        "store_path": store_path,
    }


def fetch_screenshot(src: Transport, report_id: str) -> Optional[tuple[bytes, str]]:
    """The picture and its kind, or `None` when there is not a usable one yet.

    It travels as base64 text, because a store read serves text. A file past
    the read cap comes back cut, and a cut base64 string decodes to a broken
    image — better to say so than to file a corrupt picture as evidence.

    `None` here means "not yet": the caller waits a couple of polls before
    deciding the picture is really lost.
    """
    try:
        b64 = src.read(f"{report_id}/{SCREENSHOT_NAME}")
    except RuntimeError as e:
        print(f"WARNING {report_id}: {e}", file=sys.stderr)
        return None
    if not b64:
        return None
    try:
        raw = base64.b64decode(b64.strip(), validate=True)
    except (binascii.Error, ValueError) as e:
        print(f"WARNING {report_id}: the screenshot did not decode ({e})", file=sys.stderr)
        return None
    if not raw.startswith(b"\x89PNG") and not raw.startswith(b"\xff\xd8\xff"):
        print(f"WARNING {report_id}: the screenshot is neither PNG nor JPEG", file=sys.stderr)
        return None
    return raw, "png" if raw.startswith(b"\x89PNG") else "jpg"


def build_label(report: dict) -> str:
    app = report.get("app", {})
    version = app.get("version", "?")
    build = app.get("build", "?")
    return f"{version} ({build})"


def is_real(report: dict) -> bool:
    """Whether the Arbos app wrote this report.

    A hand-made fixture is invaluable for exercising the loop and poisonous
    left lying beside real reports: its events are invented, and the first
    person to read one cold cannot tell. The app stamps `written_by`, so
    anything without it is labelled rather than trusted.
    """
    return report.get("written_by") == "arbos-desktop"


def plural(n: int, one: str, many: str = "") -> str:
    """`1 line`, `2 lines`. A report is read by a person and quoted by an agent,
    and "1 tool calls" reads as carelessness in both."""
    return f"{n} {one if n == 1 else (many or one + 's')}"


def summarise(report: dict, name: str, report_id: str, shot: bool) -> str:
    """What a person reads first. The machine-readable form is beside it."""
    app = report.get("app", {})
    kernel = report.get("kernel", {})
    turn = report.get("turn", {})
    red = report.get("redacted", {})
    inc = report.get("included", {})
    chosen = report.get("chose", {})
    events = report.get("events") or []
    tools = [e for e in events if e.get("kind") == "tool"]
    failed = [e for e in tools if e.get("error")]

    lines = [
        f"# {name} — {report.get('note') or '(no words)'}",
        "",
    ]
    if not is_real(report):
        lines += [
            "> **Not a real report.** Nothing in this file says the Arbos app wrote"
            " it (`written_by`), so it is a fixture someone made by hand. Its"
            " events, its log and its timings may be invented. Do not diagnose"
            " from it and do not quote it as something Jacob saw.",
            "",
        ]
    lines += [
        f"- Sent: {human_time(report.get('sent_ms'))}",
        f"- App: {build_label(report)} commit `{app.get('commit', '?')}`",
        f"- Kernel: {kernel.get('version', '?')} `{kernel.get('git_sha', '?')}` built {kernel.get('built_at', '?')}",
        f"- Machine: {kernel.get('os', '?')}/{kernel.get('arch', '?')}, project `{kernel.get('project', '?')}`",
        f"- Model: {kernel.get('provider', '?')} {kernel.get('model', '?')}",
        f"- Report id: `{report_id}`",
        "",
        "## What he wrote",
        "",
        (report.get("note") or "_nothing typed_").strip(),
        "",
        "## What came with it",
        "",
    ]

    def row(label: str, key: str, detail: str) -> str:
        # A part he removed and a part that failed to arrive are opposite
        # facts, and telling them apart is the whole reason the sheet shows
        # him the list. `included` alone cannot: it is false either way. So
        # `chose` says what he asked for, and the two together say which
        # happened. A report without `chose` is an early one; fall back.
        chose = chosen.get(key, inc.get(key))
        if chose is False:
            return f"- {label}: **he removed it**"
        if inc.get(key) is False:
            why = report.get("screenshot_error") if key == "screenshot" else None
            return (
                f"- {label}: **missing, and he did not remove it** — a fault"
                + (f": {why}" if why else ", cause unrecorded")
            )
        return f"- {label}: {detail}"

    lines += [
        row(
            "Screenshot",
            "screenshot",
            "saved beside this file"
            if shot
            else "**lost in transit** — the report says it carries one and it never arrived",
        ),
        row(
            "Trajectory",
            "trajectory",
            f"{plural(len(events), 'line')}, {plural(len(tools), 'tool call')}, {len(failed)} failed"
            f" (turn {turn.get('from', '?')}–{turn.get('to', '?')}"
            f"{', cut' if report.get('truncated') else ''})",
        ),
        row("Kernel log", "log", plural(len(report.get("log") or []), "line")),
        row("Transcript tail", "tail", f"{len(report.get('tail') or [])} lines"),
        row("The app's own view", "session", "included"),
        row("Tool arguments and outputs", "tool_io", "included"),
    ]

    # One number, not two. The kernel counts what it took before handing the
    # bundle over, and the app counts what it took from its own additions on the
    # way out; a reader should not have to add them up to know whether anything
    # was in this report that should not have been.
    out = report.get("redacted_on_the_way_out", {})
    kinds = ("secrets", "tokens", "values", "blocks")
    total = sum(int(red.get(k) or 0) for k in kinds) + sum(int(out.get(k) or 0) for k in kinds)
    if total:
        lines += [
            "",
            f"**{plural(total, 'credential')} removed** — "
            f"kernel `{json.dumps(red)}`, app `{json.dumps(out)}`.",
        ]

    if failed:
        lines += ["", "## The calls that failed", ""]
        for e in failed[:5]:
            lines.append(f"- `{e.get('name')}` — {str(e.get('error'))[:300]}")

    lines += [
        "",
        "## What it was",
        "",
        "_to fill in: the cause, not the symptom_",
        "",
        "## Which build carries the fix",
        "",
        "_written by `desktop-feedback.py fixed` once the pull request has merged green_",
        "",
    ]
    return "\n".join(lines)


def append_ledger(ledger: Path, taken: Iterable[dict]) -> None:
    """One row per report, above the marker, so the newest is at the bottom."""
    rows = [
        "| {name} | {build} | {note} | {what} | — | — |".format(
            name=r["name"],
            build=r["build"],
            note=(r["note"] or "(no words)").replace("|", "\\|"),
            what="_reading_" if r["real"] else "**fixture, not a real report**",
        )
        for r in taken
    ]
    if not ledger.exists():
        ledger.parent.mkdir(parents=True, exist_ok=True)
        ledger.write_text(
            "# Jacob's desktop feedback — what he said, what the loop did, which build has it\n"
            "\n"
            "Written by `deploy/feedback/poll.py`. The rig holds the authoritative copy of\n"
            "every report; `media/desktop-feedback/<date>-<n>/` is the copy agents read.\n"
            "\n"
            "| # | build | what Jacob wrote | what it was | done | reaches him in |\n"
            "| --- | --- | --- | --- | --- | --- |\n"
            f"{LEDGER_MARK}\n"
        )
    text = ledger.read_text()
    if LEDGER_MARK not in text:
        text = text.rstrip("\n") + f"\n{LEDGER_MARK}\n"
    ledger.write_text(text.replace(LEDGER_MARK, "\n".join(rows) + f"\n{LEDGER_MARK}"))


# --------------------------------------------------------------------------
# filed — a kernel bug is still this loop's to answer
# --------------------------------------------------------------------------


def cmd_filed(args: argparse.Namespace) -> int:
    src = transport_for(args.source)
    report_id = resolve_id(args.report, Path(args.rig).expanduser())
    marker = {
        "report": report_id,
        "to": args.to,
        "note": args.note,
        "at_ms": int(dt.datetime.now(dt.timezone.utc).timestamp() * 1000),
    }
    src.write(f"{report_id}/{FILED_NAME}", json.dumps(marker, indent=1) + "\n")
    local = Path(args.rig).expanduser() / args.report / FILED_NAME
    local.parent.mkdir(parents=True, exist_ok=True)
    local.write_text(json.dumps(marker, indent=1) + "\n")
    print(f"{args.report}: filed to {args.to}")
    print(
        "This loop still owes him the answer. Watch that inbox's pull request and run\n"
        "`desktop-feedback.py fixed` yourself when it merges — he talks to one place."
    )
    return 0


# --------------------------------------------------------------------------
# fixed — verified, not asserted
# --------------------------------------------------------------------------


def git(*args: str) -> Optional[str]:
    done = subprocess.run(["git", *args], capture_output=True, text=True)
    return done.stdout.strip() if done.returncode == 0 else None


def cmd_fixed(args: argparse.Namespace) -> int:
    """Write the marker only when the pull request really merged, green.

    The whole value of telling Jacob a build number is that the build
    carries the fix. So this checks rather than trusts: merged, on the
    branch it claims, and with a passing gate on the merge commit.
    """
    done = subprocess.run(
        [
            "gh", "pr", "view", str(args.pr),
            "--json", "state,mergedAt,mergeCommit,headRefOid,baseRefName,url,title",
        ],
        capture_output=True,
        text=True,
    )
    if done.returncode != 0:
        print(f"cannot read pull request {args.pr}: {done.stderr.strip()}", file=sys.stderr)
        return 2
    pr = json.loads(done.stdout)

    if pr.get("state") != "MERGED":
        print(
            f"REFUSED: pull request {args.pr} is {pr.get('state')}, not MERGED.\n"
            "The marker says which build carries the fix; writing it before the merge\n"
            "would name a build that does not.",
            file=sys.stderr,
        )
        return 1

    sha = (pr.get("mergeCommit") or {}).get("oid")
    if not sha:
        print(f"REFUSED: pull request {args.pr} has no merge commit.", file=sys.stderr)
        return 1

    base = pr.get("baseRefName") or args.base
    git("fetch", "-q", "origin", base)
    if subprocess.run(
        ["git", "merge-base", "--is-ancestor", sha, f"origin/{base}"],
        capture_output=True,
    ).returncode != 0:
        print(
            f"REFUSED: {sha[:12]} is not on origin/{base}. It may have been reverted.",
            file=sys.stderr,
        )
        return 1

    # The gate ran on the pull request's own head, which is what the steward
    # gated the merge on. The merge commit itself carries no checks in this
    # repository, so asking it would always read "unknown" and refuse
    # everything.
    head = pr.get("headRefOid") or sha
    gate = commit_gate(head)
    if gate != "success" and not args.allow_ungated:
        print(
            f"REFUSED: the gate on {head[:12]} (the pull request's head) reads {gate!r}, not success.\n"
            "A build he is told about should be one whose checks passed. Pass\n"
            "--allow-ungated only if you know why the gate is missing, and say so\n"
            "in --what.",
            file=sys.stderr,
        )
        return 1

    # The build number the desktop stamps is the commit count behind HEAD
    # (desktop/build.rs), and the dev channel publishes one build per green
    # commit on main. So the count at the merge commit is the build he can
    # install.
    count = git("rev-list", "--count", sha)
    if not count:
        print(f"cannot count commits at {sha[:12]}", file=sys.stderr)
        return 1

    marker = {
        "report": resolve_id(args.report, Path(args.rig).expanduser()),
        "pr": int(args.pr),
        "pr_url": pr.get("url"),
        "pr_title": pr.get("title"),
        "merged_commit": sha,
        "merged_at": pr.get("mergedAt"),
        "base": base,
        "gate": gate,
        "gated_commit": head,
        "build": count,
        "what": args.what,
        "at_ms": int(dt.datetime.now(dt.timezone.utc).timestamp() * 1000),
    }

    src = transport_for(args.source)
    src.write(f"{marker['report']}/{FIXED_NAME}", json.dumps(marker, indent=1) + "\n")

    rig_dir = Path(args.rig).expanduser() / args.report
    if rig_dir.is_dir():
        (rig_dir / FIXED_NAME).write_text(json.dumps(marker, indent=1) + "\n")
        note = (
            f"\nFixed in build **{count}** by [#{args.pr}]({pr.get('url')}) "
            f"(`{sha[:12]}` on {base}, gate {gate}): {args.what}\n"
        )
        summary = rig_dir / "feedback.md"
        if summary.exists():
            summary.write_text(
                summary.read_text().replace(
                    "_written by `desktop-feedback.py fixed` once the pull request has merged green_",
                    note.strip(),
                )
            )

    print(f"{args.report}: fixed in build {count} by #{args.pr} ({sha[:12]}, gate {gate})")
    print("The app reads this marker on its next attach and tells him the build.")
    print(f"Ledger: put `**{count}** (#{args.pr})` in the 'reaches him in' column.")
    return 0


def commit_gate(sha: str) -> str:
    """The combined check state of a commit, as GitHub reports it."""
    done = subprocess.run(
        ["gh", "api", f"repos/{{owner}}/{{repo}}/commits/{sha}/status", "--jq", ".state"],
        capture_output=True,
        text=True,
    )
    if done.returncode == 0 and done.stdout.strip():
        state = done.stdout.strip()
        if state != "pending":
            return state
    # A repository whose gates are all GitHub Actions reports no legacy
    # status at all, so fall back to the checks the run itself recorded.
    done = subprocess.run(
        [
            "gh", "api", f"repos/{{owner}}/{{repo}}/commits/{sha}/check-runs",
            "--jq", "[.check_runs[].conclusion] | unique | join(\",\")",
        ],
        capture_output=True,
        text=True,
    )
    if done.returncode != 0 or not done.stdout.strip():
        return "unknown"
    seen = [c for c in done.stdout.strip().split(",") if c]
    if not seen:
        return "unknown"
    if all(c in ("success", "skipped", "neutral") for c in seen):
        return "success"
    return ",".join(seen)


# --------------------------------------------------------------------------
# show
# --------------------------------------------------------------------------


def resolve_id(name: str, rig: Path) -> str:
    """Accept either the app's report id or the loop's `<date>-<n>` name."""
    state = load_seen(rig)
    if name in state["ids"]:
        return name
    for report_id, rec in state["ids"].items():
        if rec.get("name") == name:
            return report_id
    return name


def cmd_show(args: argparse.Namespace) -> int:
    rig = Path(args.rig).expanduser()
    state = load_seen(rig)
    rows = sorted(state["ids"].items(), key=lambda kv: kv[1].get("taken_ms") or 0)
    if args.report:
        rows = [(k, v) for k, v in rows if args.report in (k, v.get("name"))]
        if not rows:
            print(f"no report called {args.report}", file=sys.stderr)
            return 1
    if not rows:
        print("no reports taken yet")
        return 0
    for report_id, rec in rows:
        name = rec.get("name", "?")
        d = rig / name
        marks = [m for m in (FIXED_NAME, FILED_NAME) if (d / m).exists()]
        state_word = "fixed" if (d / FIXED_NAME).exists() else ("filed" if (d / FILED_NAME).exists() else "open")
        print(f"{name}  {state_word:6}  taken {human_time(rec.get('taken_ms'))}  id {report_id}")
        if args.verbose:
            for m in marks:
                print(f"    {m}: {json.loads((d / m).read_text())}")
    return 0


# --------------------------------------------------------------------------


def main(argv: Optional[list[str]] = None) -> int:
    p = argparse.ArgumentParser(
        prog="desktop-feedback.py",
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument(
        "--source",
        default=os.environ.get("ARBOS_FEEDBACK_SOURCE", ""),
        help="where reports land: a store address `arbos://machine/project/path`, or a directory",
    )
    p.add_argument(
        "--rig",
        default=os.environ.get("ARBOS_FEEDBACK_RIG", "~/arbos-desktop-feedback"),
        help="the loop's own copy, which outlives the Project store (default ~/arbos-desktop-feedback)",
    )
    sub = p.add_subparsers(dest="verb", required=True)

    q = sub.add_parser("poll", help="take reports nobody has taken yet")
    q.add_argument("--store", default=os.environ.get("ARBOS_FEEDBACK_STORE", ""), help="the Project store's media/desktop-feedback directory")
    q.add_argument("--ledger", default=os.environ.get("ARBOS_FEEDBACK_LEDGER", ""), help="the ledger markdown file to append a row to")
    q.set_defaults(fn=cmd_poll)

    f = sub.add_parser("filed", help="record that a report went to another inbox")
    f.add_argument("report")
    f.add_argument("--to", required=True, help="which inbox, e.g. features")
    f.add_argument("--note", default="", help="one line on what was asked for")
    f.set_defaults(fn=cmd_filed)

    x = sub.add_parser("fixed", help="verify a merged pull request and write the marker back")
    x.add_argument("report")
    x.add_argument("--pr", required=True, help="the pull request number")
    x.add_argument("--what", required=True, help="one sentence: what it was")
    x.add_argument("--base", default="main")
    x.add_argument("--allow-ungated", action="store_true", help="write the marker although the gate did not read success")
    x.set_defaults(fn=cmd_fixed)

    s = sub.add_parser("show", help="what is known about a report")
    s.add_argument("report", nargs="?")
    s.add_argument("-v", "--verbose", action="store_true")
    s.set_defaults(fn=cmd_show)

    args = p.parse_args(argv)
    if args.verb in ("poll", "filed", "fixed") and not args.source:
        p.error("--source is required (or set ARBOS_FEEDBACK_SOURCE)")
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
