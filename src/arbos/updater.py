"""``/update`` implementation: hard-reset arbos source to ``origin/main``,
reinstall the editable package if deps changed, then trigger a detached
``pm2 reload`` so the agent restarts on the new code.

All git/uv/pip/pm2 work is done via plain ``subprocess.run``; no new deps.
The whole thing typically completes in 1-3 seconds, so the agent layer drives
each step from the event loop and edits the bubble between them rather than
shoving the lot into a worker thread.
"""

from __future__ import annotations

import json
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
    """Return the canonical arbos source dir, or raise :class:`UpdateError`.

    Strict policy: ``/update`` operates on **exactly one** location --
    ``~/.arbos/src`` -- and nowhere else. There is no fallback that
    walks up looking for an arbitrary git repo; that fallback used to
    land on a user's unrelated project checkout and silently
    ``git reset --hard`` it (see the templar incident).

    Two checks must both pass:

    1. ``paths.src_dir`` (i.e. ``~/.arbos/src``) exists and contains a
       ``.git`` directory.
    2. Its ``origin`` remote URL substring-matches an arbos remote (see
       :func:`_is_arbos_repo`; ``ARBOS_UPSTREAM_REMOTES`` widens the
       allowlist for forks).

    Failures raise :class:`UpdateError` with a clear remediation hint:
    re-run ``run.sh`` (or the curl installer) to canonicalise the
    install. ``/update`` itself never bootstraps -- it would have to
    touch pm2 and the venv to do that, which is run.sh's job.
    """
    src = paths.src_dir
    if not (src / ".git").exists():
        raise UpdateError(
            f"no canonical arbos install at {src}.\n"
            "/update only touches the canonical install. To bootstrap "
            "(or migrate an existing dev install) re-run run.sh:\n"
            "  curl -sSf https://github.com/unarbos/arbos/raw/main/run.sh | bash\n"
            "or, if you already have a clone elsewhere:\n"
            "  bash <existing_clone>/run.sh\n"
            "which will sync ~/.arbos/src and re-anchor pm2 there."
        )
    if not _is_arbos_repo(src):
        raise UpdateError(
            f"refusing to /update: {src} is a git repo but its `origin` "
            "URL does not look like arbos. Set ARBOS_UPSTREAM_REMOTES "
            "(comma-separated substrings) if you intend to update from "
            "a fork; otherwise re-run run.sh to re-bootstrap a clean "
            "arbos clone at ~/.arbos/src."
        )
    return src


def verify_pm2_canonical(src_dir: Path, pm2_name: str) -> None:
    """Ensure pm2's running entry actually launches the arbos binary
    from inside ``src_dir``'s venv. Raises :class:`UpdateError` if not.

    Without this check, ``/update`` would happily ``git reset --hard``
    ``~/.arbos/src`` and then ``pm2 reload <name>``, but the reload
    would re-launch a binary from a *different* install (e.g. a leftover
    dev install at ``/Users/const/arbos/.venv/bin/arbos``). The user
    would see "restart succeeded" and assume the new code was live --
    but pm2 is running stale code from a path we never touched.

    The fix is run.sh: it has stale-pm2-entry detection that drops the
    pm2 entry and re-creates it pointing at the canonical binary. We
    refuse here and tell the user to run it.
    """
    if shutil.which("pm2") is None:
        raise UpdateError("`pm2` not on PATH; cannot verify pm2 entry")
    expected_prefix = str((src_dir / ".venv").resolve())
    try:
        proc = subprocess.run(
            ["pm2", "jlist"],
            capture_output=True,
            text=True,
            timeout=_GIT_TIMEOUT,
            check=False,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired) as exc:
        raise UpdateError(f"`pm2 jlist` failed: {exc}") from exc
    if proc.returncode != 0:
        tail = (proc.stderr or proc.stdout or "").strip()[-300:]
        raise UpdateError(f"`pm2 jlist` exited {proc.returncode}: {tail}")
    try:
        entries = json.loads(proc.stdout or "[]")
    except json.JSONDecodeError as exc:
        raise UpdateError(f"`pm2 jlist` returned non-JSON: {exc}") from exc
    if not isinstance(entries, list):
        raise UpdateError("`pm2 jlist` did not return a list")

    entry: Optional[dict] = None
    for raw in entries:
        if isinstance(raw, dict) and raw.get("name") == pm2_name:
            entry = raw
            break
    if entry is None:
        raise UpdateError(
            f"pm2 entry {pm2_name!r} not found. /update needs an existing "
            "pm2 entry to reload; bootstrap one with `bash "
            f"{src_dir}/run.sh`."
        )

    pm2_env = entry.get("pm2_env") or {}
    exec_path = str(pm2_env.get("pm_exec_path") or "")
    if not exec_path:
        raise UpdateError(
            f"pm2 entry {pm2_name!r} has no `pm_exec_path`; cannot "
            "verify it points at the canonical install"
        )

    # Resolve to handle symlinks / relative paths consistently with the
    # expected prefix above.
    try:
        exec_resolved = str(Path(exec_path).resolve())
    except OSError:
        exec_resolved = exec_path

    if not exec_resolved.startswith(expected_prefix + os.sep):
        raise UpdateError(
            "refusing to /update: pm2 is running\n"
            f"  {exec_resolved}\n"
            "but the canonical arbos binary lives at\n"
            f"  {expected_prefix}/bin/arbos\n"
            "A `pm2 reload` would re-launch the wrong binary, so the new "
            f"code would never take effect. Run `bash {src_dir}/run.sh` "
            "to re-anchor pm2 at the canonical install, then /update again."
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
