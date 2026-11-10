#!/usr/bin/env python3
"""The release object's publication lifecycle; signing and channel feeds stay in Bash."""
from __future__ import annotations

import argparse
from dataclasses import dataclass
from enum import Enum
import json
import os
from pathlib import Path
import re
import selectors
import shlex
import signal
import subprocess
import sys
import time
from typing import ClassVar, Union
from urllib.parse import quote


class Intent(Enum):
    NO_PROMOTE = "no-promote"
    PUBLISH = "publish"


class Target(Enum):
    DRAFT = "draft"
    CANDIDATE = "candidate"
    EDGE = "edge"
    STABLE = "stable"


class Error(Enum):
    INPUT = "invalid_publication_input"
    OBSERVATION = "release_observation_failed"
    RESPONSE = "invalid_release_response"
    TIMEOUT = "release_command_timeout"
    TOO_LARGE = "release_response_too_large"
    COMMAND = "release_command_failed"
    TRANSITION = "invalid_publication_transition"


class PublicationError(Exception):
    def __init__(self, code: Error):
        self.code = code
        super().__init__(code.value)


@dataclass(frozen=True)
class Missing:
    tag: ClassVar[str] = "missing"


@dataclass(frozen=True)
class Draft:
    tag: ClassVar[str] = "draft"


@dataclass(frozen=True)
class Prerelease:
    tag: ClassVar[str] = "prerelease"


@dataclass(frozen=True)
class Stable:
    tag: ClassVar[str] = "stable"


@dataclass(frozen=True)
class Creating:
    target: Target
    tag: ClassVar[str] = "creating"

    def __post_init__(self):
        if self.target is Target.DRAFT:
            raise ValueError("Missing releases are created with the existing public/prerelease policy")


@dataclass(frozen=True)
class Uploading:
    target: Target
    tag: ClassVar[str] = "uploading"


@dataclass(frozen=True)
class Finalizing:
    target: Target
    tag: ClassVar[str] = "finalizing"


@dataclass(frozen=True)
class Complete:
    target: Target
    tag: ClassVar[str] = "complete"


@dataclass(frozen=True)
class Refused:
    tag: ClassVar[str] = "refused"


@dataclass(frozen=True)
class Failed:
    error: Error
    tag: ClassVar[str] = "failed"


State = Union[Missing, Draft, Prerelease, Stable, Creating, Uploading, Finalizing, Complete, Refused, Failed]


@dataclass(frozen=True)
class Request:
    intent: Intent
    channel: str


@dataclass(frozen=True)
class Failure:
    error: Error


class Event(Enum):
    CREATED = "created"
    UPLOADED = "uploaded"
    FINALIZED = "finalized"


def transition(state: State, event: Request | Event | Failure) -> State:
    if isinstance(state, (Complete, Refused, Failed)):
        return state
    if isinstance(event, Failure):
        return Failed(event.error)
    if isinstance(event, Request) and isinstance(state, (Missing, Draft, Prerelease, Stable)):
        if event.channel not in ("stable", "edge") or not isinstance(event.intent, Intent):
            return Failed(Error.INPUT)
        if event.intent is Intent.NO_PROMOTE:
            if isinstance(state, Stable):
                return Refused()
            target = Target.DRAFT if isinstance(state, Draft) else Target.CANDIDATE
        else:
            target = Target.STABLE if event.channel == "stable" else Target.EDGE
        return Creating(target) if isinstance(state, Missing) else Uploading(target)
    if event is Event.CREATED and isinstance(state, Creating):
        return Complete(state.target)
    if event is Event.UPLOADED and isinstance(state, Uploading):
        return Finalizing(state.target)
    if event is Event.FINALIZED and isinstance(state, Finalizing):
        return Complete(state.target)
    return Failed(Error.TRANSITION)


def run_gh(arguments: list[str], *, timeout: float = 60, limit: int = 65536) -> tuple[int, bytes]:
    """Bound the one CLI response before parsing; discard raw stderr, including credentials."""
    process = None
    try:
        process = subprocess.Popen(["gh", *arguments], stdout=subprocess.PIPE,
                                   stderr=subprocess.DEVNULL)
        deadline = time.monotonic() + timeout
        output = bytearray()
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not selector.select(remaining):
                    raise PublicationError(Error.TIMEOUT)
                chunk = os.read(process.stdout.fileno(), min(8192, limit + 1 - len(output)))
                if not chunk:
                    break
                output.extend(chunk)
                if len(output) > limit:
                    raise PublicationError(Error.TOO_LARGE)
        code = process.wait(timeout=max(0, deadline - time.monotonic()))
        return code, bytes(output)
    except subprocess.TimeoutExpired as error:
        raise PublicationError(Error.TIMEOUT) from error
    except OSError as error:
        raise PublicationError(Error.COMMAND) from error
    finally:
        if process is not None:
            if process.poll() is None:
                process.kill()
                process.wait()
            if process.stdout is not None:
                process.stdout.close()


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate response field")
        result[key] = value
    return result


def observe(repo: str, tag: str) -> State:
    code, response = run_gh([
        "api", f"repos/{repo}/releases/tags/{quote(tag, safe='')}", "--method", "GET", "--include",
        "--jq", "{tag_name,draft,prerelease}",
    ])
    try:
        header, body = re.split(rb"\r?\n\r?\n", response, maxsplit=1)
        status = re.fullmatch(rb"HTTP/[0-9.]+ ([0-9]{3})(?: [^\r\n]*)?", header.splitlines()[0])
        if status is None:
            raise ValueError("missing HTTP status")
        status = int(status[1])
        if status == 404:
            return Missing()
        if status != 200 or code != 0:
            raise PublicationError(Error.OBSERVATION)
        item = json.loads(body, object_pairs_hook=unique_object)
        if not isinstance(item, dict) or set(item) != {"tag_name", "draft", "prerelease"}:
            raise ValueError("unexpected release fields")
        if item["tag_name"] != tag or type(item["draft"]) is not bool or type(item["prerelease"]) is not bool:
            raise ValueError("invalid release fields")
        return Draft() if item["draft"] else Prerelease() if item["prerelease"] else Stable()
    except (ValueError, IndexError, RecursionError) as error:
        raise PublicationError(Error.RESPONSE) from error


def publish(args) -> State:
    intent = Intent.PUBLISH if args.promote == "true" else Intent.NO_PROMOTE
    observed = Missing() if args.dry_run else observe(args.repo, args.tag)
    state = transition(observed, Request(intent, args.channel))
    while isinstance(state, (Creating, Uploading, Finalizing)):
        if isinstance(state, Creating):
            flags = ["--latest"] if state.target is Target.STABLE else ["--prerelease"]
            command = ["release", "create", args.tag, "--repo", args.repo, "--target", args.sha,
                       "--title", args.title, "--notes-file", args.notes_file, *flags, "--", *args.assets]
            event = Event.CREATED
        elif isinstance(state, Uploading):
            command = ["release", "upload", args.tag, "--repo", args.repo, "--clobber", "--", *args.assets]
            event = Event.UPLOADED
        else:
            flags = ["--draft=true"] if state.target is Target.DRAFT else [
                "--prerelease=" + str(state.target is not Target.STABLE).lower(),
                "--latest=" + str(state.target is Target.STABLE).lower(),
            ]
            if state.target in (Target.STABLE, Target.EDGE):
                flags.append("--draft=false")
            command = ["release", "edit", args.tag, "--repo", args.repo, "--title", args.title,
                       "--notes-file", args.notes_file, *flags]
            event = Event.FINALIZED
        if args.dry_run:
            print("[dry-run] " + shlex.join(["gh", *command]), file=sys.stderr)
            return Complete(state.target)
        try:
            code, _ = run_gh(command)
            state = transition(state, event if code == 0 else Failure(Error.COMMAND))
        except PublicationError as error:
            state = transition(state, Failure(error.code))
    return state


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("repo", "tag", "sha", "title", "notes-file"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--channel", choices=("stable", "edge"), required=True)
    parser.add_argument("--promote", choices=("true", "false"), required=True)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("assets", nargs="+")
    args = parser.parse_args()
    if (not re.fullmatch(r"[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}", args.repo)
            or any(part in (".", "..") for part in args.repo.split("/"))
            or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", args.tag)
            or ".." in args.tag or args.tag.endswith((".", ".lock"))
            or not args.tag.startswith("v" if args.channel == "stable" else "edge-")
            or not re.fullmatch(r"[0-9a-f]{40}", args.sha)
            or len(args.title) > 512 or not Path(args.notes_file).is_file()
            or any(not Path(asset).is_file() or "\n" in asset or "\r" in asset for asset in args.assets)):
        raise PublicationError(Error.INPUT)
    state = publish(args)
    if isinstance(state, Refused):
        print("release publication refused: --no-promote cannot replace a public stable release", file=sys.stderr)
        return 1
    if isinstance(state, Failed):
        raise PublicationError(state.error)
    if not isinstance(state, Complete):
        raise PublicationError(Error.TRANSITION)
    print("preview" if args.dry_run else "draft" if state.target is Target.DRAFT else "published")
    return 0


if __name__ == "__main__":
    def terminate(signum, _frame):
        # Unwind run_gh's finally block before this CLI attempt exits.
        raise SystemExit(128 + signum)

    signal.signal(signal.SIGTERM, terminate)
    try:
        sys.exit(main())
    except PublicationError as error:
        print("release publication failed: " + error.code.value, file=sys.stderr)
        sys.exit(1)
