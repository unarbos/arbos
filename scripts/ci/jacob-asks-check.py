#!/usr/bin/env python3
"""Every one of Jacob's asks has a test that fails when the app stops doing it.

Checks `docs/product/jacob-asks.md` against the Swift sources:

  * every `[feedback …]` tag in the app is registered,
  * every registered ask names a guard that exists,
  * that guard names the ask, so the pairing is checked rather than claimed,
  * that guard is not switched off.

Runs on Linux in seconds, so it sits in the fast `Layout` job rather than spending a macOS slot.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from jacob_asks import REGISTRY, app_tags, find_guard, read_registry, test_cases  # noqa: E402


def check(root: Path) -> list[str]:
    guarded, superseded, problems = read_registry(root)
    cases = test_cases(root)
    tagged = app_tags(root)

    for ask in sorted(set(tagged) - set(guarded) - superseded):
        where = ", ".join(str(p) for p in tagged[ask][:2])
        problems.append(
            f"{ask}: tagged in {where} but not in {REGISTRY}. "
            f"Add a row naming the test that guards it."
        )

    for ask, guard in sorted(guarded.items()):
        found = find_guard(guard, cases)
        if not found:
            problems.append(
                f"{ask}: names `{guard}`, which is not in the tree. Restore it, point the row "
                f"at the test that guards this ask now, or — if that test is landing in another "
                f"PR — merge that one first."
            )
            continue
        if naming := [c for c in found if ask in c.tags]:
            found = naming
        else:
            problems.append(
                f"{ask}: `{guard}` exists but does not name this ask. "
                f"Put `[feedback {ask}]` in its doc comment (or its suite's) so the pairing holds."
            )
        if live := [c for c in found if not c.skipped]:
            found = live
        else:
            problems.append(f"{ask}: `{guard}` always skips, so nothing guards this ask.")

    return problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd(), help="repository root")
    args = parser.parse_args()

    problems = check(args.root)
    if problems:
        print(f"{len(problems)} problem(s) with Jacob's ask register:\n", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        print(
            f"\n{REGISTRY} is the register; see its 'Changing a guard' section before "
            f"removing anything.",
            file=sys.stderr,
        )
        return 1

    guarded, superseded, _ = read_registry(args.root)
    print(f"{len(guarded)} of Jacob's asks are registered and guarded "
          f"({len(superseded)} superseded).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
