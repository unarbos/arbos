#!/usr/bin/env python3
"""A guard for one of Jacob's asks does not come off quietly.

`jacob-asks-check.py` keeps the register honest about the tree as it stands. This one compares
the branch with its base and fails when an ask comes out of the register, loses the test that
guards it, or has that test switched off — unless the pull request says so on purpose:

    Guard-change: <ask-id> — <why, and who agreed>

The line is the point. Removing a guard is a decision about whether Jacob still gets what he
asked for, and it should be made by someone willing to write down why.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from jacob_asks import find_guard, read_registry, test_cases  # noqa: E402

SIGN_OFF_RE = re.compile(r"^\s*Guard-change:\s*([a-z0-9][a-z0-9-]*)\s*[-—:]\s*\S", re.MULTILINE)
TRACKED = ("apps/ios/Arbos/Arbos", "apps/ios/Arbos/ArbosTests", "apps/ios/Arbos/ArbosUITests",
           "docs/product/jacob-asks.md")


def checkout(ref: str, into: Path) -> Path:
    """The base's own copy of the files the register covers, as far as it has them."""
    into.mkdir(parents=True, exist_ok=True)
    present = [
        path
        for path in TRACKED
        if subprocess.run(["git", "cat-file", "-e", f"{ref}:{path}"],
                          capture_output=True).returncode == 0
    ]
    if present:
        archive = subprocess.run(
            ["git", "archive", ref, *present], check=True, stdout=subprocess.PIPE
        ).stdout
        subprocess.run(["tar", "-x", "-C", str(into)], input=archive, check=True)
    return into


def guard_state(root: Path) -> tuple[dict[str, str], dict[str, bool]]:
    """What the register promises, and whether each named guard is live."""
    guarded, _, _ = read_registry(root)
    cases = test_cases(root)
    # Every case a guard names is part of it, so one switched-off case is one hole.
    live = {
        ask: (found := find_guard(guard, cases)) and not any(c.skipped for c in found)
        for ask, guard in guarded.items()
    }
    return guarded, live


def check(base_ref: str, body: str) -> list[str]:
    with tempfile.TemporaryDirectory() as tmp:
        was_guarded, was_live = guard_state(checkout(base_ref, Path(tmp) / "base"))
    now_guarded, now_live = guard_state(Path.cwd())

    signed_off = set(SIGN_OFF_RE.findall(body or ""))
    losses: list[str] = []

    for ask, guard in sorted(was_guarded.items()):
        if ask not in now_guarded:
            losses.append(f"{ask}: taken out of the register (was guarded by `{guard}`)")
        elif now_guarded[ask] != guard and was_live.get(ask) and not now_live.get(ask):
            losses.append(f"{ask}: moved from `{guard}` to `{now_guarded[ask]}`, which does not guard it")
        elif was_live.get(ask) and not now_live.get(ask):
            losses.append(f"{ask}: `{now_guarded[ask]}` no longer runs — deleted, renamed or skipped")

    return [loss for loss in losses if loss.split(":")[0] not in signed_off]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-ref", required=True, help="what this branch merges into")
    parser.add_argument("--body", default="", help="the pull request description")
    parser.add_argument("--body-file", type=Path, help="the description, from a file")
    args = parser.parse_args()

    body = args.body_file.read_text(encoding="utf-8") if args.body_file else args.body
    losses = check(args.base_ref, body)
    if losses:
        print("This branch takes a guard off one of Jacob's asks:\n", file=sys.stderr)
        for loss in losses:
            print(f"  - {loss}", file=sys.stderr)
        print(
            "\nIf that is intended, say so in the pull request description, one line per ask:\n"
            "\n    Guard-change: <ask-id> — <why, and who agreed>\n"
            "\nIf it is not, restore the test. docs/product/jacob-asks.md says what each ask is.",
            file=sys.stderr,
        )
        return 1

    print("No guard comes off in this branch.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
