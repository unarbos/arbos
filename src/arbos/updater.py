"""``/update`` implementation: hard-reset arbos source to ``origin/main``,
reinstall the editable package if deps changed, then trigger a detached
``pm2 reload`` so the agent restarts on the new code.

All git/uv/pip/pm2 work is done via plain ``subprocess.run``; no new deps.
The whole thing typically completes in 1-3 seconds, so the agent layer drives
each step from the event loop and edits the bubble between them rather than
shoving the lot into a worker thread.
"""

from __future__ import annotations

import logging
import os
import shutil
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

from .workspace import InstallPaths

logger = logging.getLogger(__name__)


UPDATE_BRANCH = "main"
_GIT_TIMEOUT = 30.0
_INSTALL_TIMEOUT = 120.0


class UpdateError(RuntimeError):
    """User-facing update failure (network, missing git, etc.)."""


@dataclass
class UpdateResult:
    src_dir: Path
    old_sha: str
    new_sha: str
    old_subject: str
    new_subject: str
    reinstalled: bool
    dirty_wiped: list[str] = field(default_factory=list)

    @property
    def changed(self) -> bool:
        return self.old_sha != self.new_sha


# --- subprocess helpers -----------------------------------------------------

def _run(
    cmd: list[str],
    *,
    cwd: Path,
    timeout: float = _GIT_TIMEOUT,
    env: Optional[dict[str, str]] = None,
) -> str:
    """Run a sync subprocess, returning stdout. Raises ``UpdateError`` on any
    failure with a short, user-presentable message."""
    try:
        proc = subprocess.run(
            cmd,
            cwd=str(cwd),
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
            env=env,
        )
    except FileNotFoundError as exc:
        raise UpdateError(f"`{cmd[0]}` not found on PATH") from exc
    except subprocess.TimeoutExpired as exc:
        raise UpdateError(
            f"`{cmd[0]} {' '.join(cmd[1:3])}…` timed out after {timeout:.0f}s"
        ) from exc
    if proc.returncode != 0:
        tail = (proc.stderr or proc.stdout or "").strip()[-400:]
        raise UpdateError(
            f"`{' '.join(cmd[:3])}…` exited {proc.returncode}: {tail}"
        )
    return proc.stdout


# --- public helpers ---------------------------------------------------------

def resolve_src_dir(paths: InstallPaths) -> Path:
    """Find the arbos git checkout to update.

    Curl-installed machines clone to ``~/.arbos/src``; dev machines run
    against an in-place editable install -- in that case we walk up from
    this module to find the source repo root.
    """
    if (paths.src_dir / ".git").exists():
        return paths.src_dir
    # Editable-install dev loop: arbos/updater.py lives at <repo>/src/arbos/.
    here = Path(__file__).resolve()
    if len(here.parents) >= 3:
        candidate = here.parents[2]
        if (candidate / ".git").exists():
            return candidate
    raise UpdateError(
        f"no git checkout at {paths.src_dir} or in this module's source tree; "
        "nothing to update"
    )


def _head_info(src: Path, ref: str = "HEAD") -> tuple[str, str]:
    """Return ``(short_sha, subject)`` for ``ref``."""
    out = _run(
        ["git", "log", "-1", "--format=%h%x1f%s", ref],
        cwd=src,
    ).strip()
    if "\x1f" in out:
        sha, _, subject = out.partition("\x1f")
        return sha.strip(), subject.strip()
    return out, ""


def _dirty_files(src: Path) -> list[str]:
    out = _run(["git", "status", "--porcelain"], cwd=src)
    files: list[str] = []
    for line in out.splitlines():
        # `XY <path>` (XY is two-char status)
        path = line[3:].strip() if len(line) > 3 else line.strip()
        if path:
            files.append(path)
    return files


def _changed_paths(src: Path, old_sha: str, new_sha: str) -> set[str]:
    if old_sha == new_sha:
        return set()
    out = _run(
        ["git", "diff", "--name-only", old_sha, new_sha],
        cwd=src,
    )
    return {line.strip() for line in out.splitlines() if line.strip()}


def _needs_reinstall(changed: set[str]) -> bool:
    """True if any path that affects the installed package or runner changed."""
    triggers = {"pyproject.toml", "run.sh", "uv.lock", "setup.py", "setup.cfg"}
    return bool(changed & triggers)


def _reinstall(src: Path) -> None:
    """Reinstall the editable arbos package against ``src``.

    Prefer the system ``uv`` (matches what ``run.sh`` uses everywhere);
    fall back to the venv's ``pip`` if uv isn't available.
    """
    if shutil.which("uv"):
        _run(
            ["uv", "pip", "install", "-e", str(src)],
            cwd=src,
            timeout=_INSTALL_TIMEOUT,
            env=os.environ.copy(),
        )
        return

    venv_pip = src / ".venv" / "bin" / "pip"
    if venv_pip.exists():
        _run(
            [str(venv_pip), "install", "-e", str(src)],
            cwd=src,
            timeout=_INSTALL_TIMEOUT,
        )
        return

    raise UpdateError(
        "neither `uv` nor `<src>/.venv/bin/pip` available; cannot reinstall"
    )


def fetch_and_reset(src: Path) -> tuple[str, str, str, str, list[str]]:
    """Fetch ``origin/main`` and hard-reset ``src`` to it -- but only if the
    remote actually moved.

    Returns ``(old_sha, old_subject, new_sha, new_subject, dirty_wiped)``.
    ``dirty_wiped`` is the list of files that were thrown away by the reset,
    so it is only populated when we actually reset (i.e. origin moved). When
    ``old == new`` we skip the reset entirely; an "already up to date" run
    must not mutate the working tree.
    """
    old_sha, old_subject = _head_info(src)

    _run(
        ["git", "fetch", "--depth", "1", "origin", UPDATE_BRANCH],
        cwd=src,
    )

    target_sha, _ = _head_info(src, ref=f"origin/{UPDATE_BRANCH}")
    if target_sha == old_sha:
        return old_sha, old_subject, old_sha, old_subject, []

    dirty = _dirty_files(src)
    _run(
        ["git", "reset", "--hard", f"origin/{UPDATE_BRANCH}"],
        cwd=src,
    )
    new_sha, new_subject = _head_info(src)
    return old_sha, old_subject, new_sha, new_subject, dirty


def spawn_pm2_reload(pm2_name: str) -> None:
    """Fire-and-forget ``pm2 reload <pm2_name>`` in a detached child so we
    survive long enough to flush the bubble before pm2 SIGINTs us.

    ``pm2_name`` is the full pm2 entry name (e.g. ``arbos-constmac``), not
    just the machine suffix; the caller is responsible for picking the
    right one (see ``CursorAgent.pm2_name``).
    """
    if shutil.which("pm2") is None:
        raise UpdateError("`pm2` not on PATH; cannot trigger restart")
    try:
        subprocess.Popen(
            ["pm2", "reload", pm2_name],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
    except OSError as exc:
        raise UpdateError(f"failed to spawn `pm2 reload {pm2_name}`: {exc}") from exc


def render_summary(result: UpdateResult, *, pm2_name: str, restarting: bool) -> str:
    """Format the final bubble text for an update run."""
    lines: list[str] = []
    if not result.changed:
        lines.append(f"already up to date at {result.new_sha}")
        if result.old_subject:
            lines.append(f"  ({result.old_subject})")
    else:
        lines.append(f"old: {result.old_sha} — {result.old_subject or '?'}")
        lines.append(f"new: {result.new_sha} — {result.new_subject or '?'}")
        lines.append("")
        lines.append(f"reinstalled deps: {'yes' if result.reinstalled else 'no'}")
    if result.dirty_wiped:
        shown = result.dirty_wiped[:8]
        lines.append("")
        lines.append(
            f"warning: wiped {len(result.dirty_wiped)} uncommitted file(s):"
        )
        for f in shown:
            lines.append(f"  {f}")
        if len(result.dirty_wiped) > len(shown):
            lines.append(f"  …and {len(result.dirty_wiped) - len(shown)} more")
    if restarting:
        lines.append("")
        lines.append(f"restarting {pm2_name}…")
    return "\n".join(lines)
