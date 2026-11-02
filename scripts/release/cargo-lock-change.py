#!/usr/bin/env python3
"""Does a Cargo.lock change mean anything to the Mac build?

    scripts/release/cargo-lock-change.py <before.lock> <after.lock>

Exit 0: the two lockfiles are the same, or they differ only in the version numbers of our own
        workspace crates. Nothing the Mac binary links against moved.
Exit 1: something else differs — a dependency arrived or left, a third-party version or checksum
        moved, a dependency list changed. The Mac app must be built and released.

The exit codes follow `git diff --quiet`: 0 means "nothing to see here".

Why this exists: a release bump commit writes the new version into Cargo.toml and Cargo.lock, and
that commit fast-forwards onto main. Every commit after it therefore carries the bump's version
lines in Cargo.lock. Cargo.lock is one of the paths the Mac app is built from, so without this
check every merge looked like a Mac change and cut a Mac release — v0.6.368 was a relay-only fix
whose whole Mac-path difference was twelve lines of `version = "0.6.366"` becoming
`version = "0.6.367"`, and it still cost a full 32-minute release cycle.

How "our own crate" is decided: in a lockfile, a package Cargo resolved from a path inside the
workspace has no `source` key, and a package from a registry or a git remote always has one. The
packages with no `source` are therefore exactly the workspace members — the `arbos-*` crates. Only
their `version` lines are ignored, and only their `version` lines: a changed dependency list, a new
workspace member, a removed one, or any edit at all to a package that has a `source` still counts.
The `version = 4` at the top of the file is the lockfile format number, not a package, and is never
ignored.
"""

from __future__ import annotations

import sys
from pathlib import Path

USAGE = "usage: cargo-lock-change.py <before.lock> <after.lock>"
IGNORED = 'version = "<ignored: our own workspace crate>"\n'


def normalise(text: str) -> str:
    """Return `text` with the version line of every workspace package blanked out.

    Every other byte is left as it was, so two lockfiles whose normalised forms are equal differ
    in nothing but our own crates' version numbers.
    """
    out: list[str] = []
    # The lines of the [[package]] block being read, or None while we are outside one: the file
    # header, or a section such as [metadata].
    block: list[str] | None = None

    def flush() -> None:
        nonlocal block
        if block is None:
            return
        ours = not any(line.startswith("source = ") for line in block)
        out.extend(
            IGNORED if ours and line.startswith("version = ") else line for line in block
        )
        block = None

    for line in text.splitlines(keepends=True):
        # A key inside a block is either unindented (`name = "…"`) or indented by one space (an
        # entry of a dependency list), so a line that starts with "[" always opens a new section.
        if line.startswith("["):
            flush()
            out.append(line)
            block = [] if line.rstrip("\n") == "[[package]]" else None
        elif block is None:
            out.append(line)
        else:
            block.append(line)
    flush()
    return "".join(out)


def report(before: str, after: str, limit: int = 6) -> None:
    """Print the first few lines that really differ, so the workflow log says why it released."""
    was, now = set(before.splitlines()), set(after.splitlines())
    for mark, lines, kept in (("-", before, now), ("+", after, was)):
        shown = 0
        for line in lines.splitlines():
            if shown == limit:
                break
            if line not in kept:
                print(f"{mark}{line}", file=sys.stderr)
                shown += 1


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print(USAGE, file=sys.stderr)
        return 2
    before, after = (normalise(Path(p).read_text(encoding="utf-8")) for p in argv[1:3])
    if before == after:
        return 0
    report(before, after)
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
