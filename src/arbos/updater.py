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

# Substrings that, when present in a git remote URL, identify it as the
# arbos repo. We accept any of these so https/ssh, mirror forks, and
# username variants all match -- the important property is that NO
# unrelated repo (e.g. a user's templar / project checkout that happens
# to live near the install) can be mistaken for arbos. Override via the
# ``ARBOS_UPSTREAM_REMOTES`` env var (comma-separated substrings).
_DEFAULT_ARBOS_REMOTE_NEEDLES = ("/arbos.git", "/arbos ", "unarbos/arbos")


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

def _arbos_remote_needles() -> tuple[str, ...]:
    """Substrings that identify a git remote URL as the arbos repo.

    Override via ``ARBOS_UPSTREAM_REMOTES`` (comma-separated substrings)
    if a fork / mirror needs to be recognised.
    """
    raw = (os.environ.get("ARBOS_UPSTREAM_REMOTES") or "").strip()
    if raw:
        extra = tuple(s.strip() for s in raw.split(",") if s.strip())
        if extra:
            return extra
    return _DEFAULT_ARBOS_REMOTE_NEEDLES


def _is_arbos_repo(candidate: Path) -> bool:
    """True iff ``candidate`` is a git repo whose ``origin`` URL looks like arbos.

    The check is intentionally a substring match against ``origin``'s
    fetch URL: we want to ALLOW https/ssh variants and forks named
    ``arbos`` (set ARBOS_UPSTREAM_REMOTES to broaden), but REFUSE any
    unrelated repo so we can never ``git reset --hard`` something that
    isn't ours. Returns False on any git error.
    """
    if not (candidate / ".git").exists():
        return False
    try:
        url = _run(
            ["git", "remote", "get-url", "origin"], cwd=candidate
        ).strip()
    except UpdateError:
        return False
    if not url:
        return False
    # Pad with a trailing space so substrings like "/arbos " match a URL
    # whose path component ends exactly with "/arbos" (no .git suffix).
    haystack = url + " "
    return any(needle in haystack for needle in _arbos_remote_needles())


def resolve_src_dir(paths: InstallPaths) -> Path:
    """Find the arbos git checkout to update -- and ONLY that.

    Lookup order:

    1. ``~/.arbos/src`` (the canonical curl-install location). Must
       verify as an arbos repo via :func:`_is_arbos_repo` -- we will
       NEVER ``git reset --hard`` a directory whose ``origin`` doesn't
       look like arbos.
    2. Walk up from this module's path (editable-install dev loop), but
       again only return a candidate that verifies as arbos. This stops
       the previous behaviour where the walk-up could land in an
       unrelated parent git repo (e.g. a project the user happened to
       install arbos inside) and silently destroy their work.

    Anything else raises :class:`UpdateError` so ``/update`` aborts
    instead of touching the wrong tree.
    """
    if (paths.src_dir / ".git").exists():
        if _is_arbos_repo(paths.src_dir):
            return paths.src_dir
        raise UpdateError(
            f"{paths.src_dir} is a git repo but its `origin` URL does not "
            "look like arbos. Refusing to /update -- this looks like a "
            "different project's checkout. Set ARBOS_UPSTREAM_REMOTES if "
            "you intend to update from a fork."
        )
    # Editable-install dev loop: arbos/updater.py lives at <repo>/src/arbos/.
    here = Path(__file__).resolve()
    if len(here.parents) >= 3:
        candidate = here.parents[2]
        if _is_arbos_repo(candidate):
            return candidate
        # Found a git repo but it's NOT arbos -- this is exactly the
        # scenario that destroyed user work on remote machines (the
        # walk-up landed in a project repo, /update ran git reset --hard
        # against it). Refuse loudly.
        if (candidate / ".git").exists():
            raise UpdateError(
                f"refusing to /update {candidate}: that's a git repo but "
                "its `origin` does not look like arbos. /update only "
                "touches arbos's own checkout (see ARBOS_UPSTREAM_REMOTES "
                "for fork support)."
            )
    raise UpdateError(
        f"no arbos git checkout at {paths.src_dir} or in this module's "
        "source tree; nothing to update"
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


def _dirty_tracked_files(src: Path) -> list[str]:
    """List TRACKED files with uncommitted changes -- the only files that
    ``git reset --hard origin/main`` actually overwrites.

    Untracked files (``??`` in ``git status --porcelain``) survive a
    hard reset, so listing them as "wiped" was misleading and led the
    previous /update output to scare users into thinking we'd deleted
    work we hadn't. We exclude them here so the post-update warning
    only mentions files that genuinely got rolled back.
    """
    out = _run(["git", "status", "--porcelain"], cwd=src)
    files: list[str] = []
    for line in out.splitlines():
        if not line.strip():
            continue
        # `XY <path>` (XY is two-char status). `??` means untracked.
        status = line[:2]
        if status == "??":
            continue
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

    # Only TRACKED-modified files are actually thrown away by
    # `git reset --hard`; untracked files survive. Listing untracked
    # files as "wiped" was misleading and scared users into thinking
    # /update had destroyed work it hadn't.
    dirty = _dirty_tracked_files(src)
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
