"""The rig's own liveness: is the X display answering?

Display ``:1`` hung for about ten minutes mid-journey on 2026-09-16 (X
clients could not connect; it freed when Arbos and Cursor were killed).
A hung display makes every check downstream meaningless while the run
looks healthy — `scrot` blocks, the driver's state goes stale, a still
never lands. So the loops pulse the display and fail loudly the moment
it stops answering, the way QA's liveness pulse does for the window.

    from rig import DisplayHung, pulse, still

Both ``pulse`` and ``still`` raise ``DisplayHung``; the loops let that
end the run with a row that says so.
"""
from __future__ import annotations

import os
import subprocess
import time
from pathlib import Path

#: How long the display gets to answer a trivial request before the run
#: calls it hung. `xdotool getactivewindow` on a healthy Xvfb answers in
#: milliseconds; the hang seen on the rig answered nothing for minutes.
PULSE_TIMEOUT_S = 8.0

#: A still that takes longer than this is a hung display, not a slow one.
STILL_TIMEOUT_S = 15.0


class DisplayHung(RuntimeError):
    """The X display did not answer within the pulse timeout."""


def pulse(display: str | None = None) -> float:
    """Ask the display for its active window and return how long it took.

    Raises :class:`DisplayHung` when nothing comes back in time. The
    active window may legitimately be none (returns non-zero); only a
    timeout counts as hung.
    """
    env = {**os.environ, "DISPLAY": display or os.environ.get("DISPLAY", ":1")}
    t0 = time.monotonic()
    try:
        subprocess.run(
            ["xdotool", "getactivewindow"],
            env=env,
            capture_output=True,
            timeout=PULSE_TIMEOUT_S,
            check=False,
        )
    except subprocess.TimeoutExpired as e:
        raise DisplayHung(
            f"display {env['DISPLAY']} did not answer `xdotool getactivewindow` in {PULSE_TIMEOUT_S:.0f}s"
        ) from e
    return time.monotonic() - t0


def still(path: Path | str, display: str | None = None) -> None:
    """`scrot -o path`, bounded: a hung display raises instead of blocking."""
    env = {**os.environ, "DISPLAY": display or os.environ.get("DISPLAY", ":1")}
    try:
        subprocess.run(["scrot", "-o", str(path)], env=env, timeout=STILL_TIMEOUT_S, check=False)
    except subprocess.TimeoutExpired as e:
        raise DisplayHung(
            f"display {env['DISPLAY']} did not deliver a still in {STILL_TIMEOUT_S:.0f}s"
        ) from e


def desktop_build(desktop_bin: str | Path) -> str:
    """The desktop's own word on its build — `arbos-desktop --version`,
    e.g. `0.2.0 1462 42cb975-dirty` (version, build number, short sha,
    `-dirty` when the tree had uncommitted changes). Read from the binary
    the run will launch. A build that failed leaves the previous binary in
    `target/`, and a gate that never reads the binary's sha runs green on
    a build that is not the PR's (rig audit R21)."""
    try:
        out = subprocess.run([str(desktop_bin), "--version"], capture_output=True, text=True, timeout=10)
        line = (out.stdout or out.stderr).strip().splitlines()
        return line[0] if line else f"{desktop_bin}: no version line"
    except Exception as e:  # noqa: BLE001
        return f"{desktop_bin}: --version failed ({e})"


def tree_sha(src: str | Path) -> str:
    """Short sha of the checkout the binary should have been built from."""
    try:
        out = subprocess.run(["git", "-C", str(src), "rev-parse", "--short=7", "HEAD"], capture_output=True, text=True, timeout=10)
        return out.stdout.strip()
    except Exception:  # noqa: BLE001
        return ""


def binary_matches_tree(build_line: str, sha: str) -> bool:
    """`0.2.0 1462 42cb975-dirty` matches tree `42cb975`; a `-dirty` build
    still counts as the tree's (the rig builds uncommitted work to drive it)."""
    if not sha:
        return False
    parts = build_line.split()
    return any(p.split("-")[0].startswith(sha) or sha.startswith(p.split("-")[0]) for p in parts if len(p.split("-")[0]) >= 7)


def kernel_build(kernel_bin: str | Path) -> str:
    """The kernel's own word on its build — `arbos-kernel --version`, e.g.
    `arbos-kernel 0.2.0 d73a25aea876 protocol 1` — read from the binary the
    run will launch, never from a roster (the hub reports whichever process
    registered last) and never assumed from the branch (a kernel serving a
    deleted binary looked current for two and a half days, rig audit R11).
    """
    try:
        out = subprocess.run([str(kernel_bin), "--version"], capture_output=True, text=True, timeout=10)
        line = (out.stdout or out.stderr).strip().splitlines()
        return line[0] if line else f"{kernel_bin}: no version line"
    except Exception as e:  # noqa: BLE001
        return f"{kernel_bin}: --version failed ({e})"
