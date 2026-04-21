"""Build the system-context preamble injected into every ``cursor-agent`` run.

The rendered text is:

* persisted to ``<install-root>/.arbos/prompts/PROMPT.md`` so the user can read
  exactly what the agent sees, and
* prepended to the user's message at spawn time by :class:`CursorRunner`.

Live data (Tailscale peers, Doppler key list, git remote/HEAD) is collected via
short subprocess calls and cached in-process for ``_CACHE_TTL`` seconds so a
chatty Telegram session does not pay the latency on every message.

The directory ``.arbos/prompts/`` is the extensibility hook: any other ``*.md``
file dropped in there is concatenated after ``PROMPT.md`` (lexical order),
giving us a place to land future ``memory.md`` / ``chat-history.md`` /
``peers.md`` files without any code change.
"""

from __future__ import annotations

import json
import logging
import os
import shutil
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

from .config import StoredConfig
from .workspace import InstallPaths

logger = logging.getLogger(__name__)


_CACHE_TTL = 60.0  # seconds; long enough to absorb message bursts, short
                   # enough to pick up a new tailnet peer / doppler key

_SUBPROC_TIMEOUT = 5.0


@dataclass
class _TailscaleNode:
    name: str           # short DNS label (e.g. "constmac")
    hostname: str       # raw HostName (often equal to name)
    os: str
    online: bool


@dataclass
class _TailscaleView:
    self_node: Optional[_TailscaleNode] = None
    peers: list[_TailscaleNode] = field(default_factory=list)
    available: bool = True
    error: Optional[str] = None


@dataclass
class _DopplerKey:
    name: str
    note: str


@dataclass
class _DopplerView:
    keys: list[_DopplerKey] = field(default_factory=list)
    available: bool = True
    error: Optional[str] = None


@dataclass
class _GitView:
    remote: str = "unknown"
    branch: str = "unknown"
    short_sha: str = "unknown"


# --- subprocess helpers -----------------------------------------------------

def _run(cmd: list[str], *, cwd: Optional[Path] = None) -> Optional[str]:
    """Run a short read-only subprocess, returning stdout or ``None`` on any
    failure (missing binary, non-zero exit, timeout). Never raises."""
    try:
        proc = subprocess.run(
            cmd,
            cwd=str(cwd) if cwd else None,
            capture_output=True,
            text=True,
            timeout=_SUBPROC_TIMEOUT,
            check=False,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired) as exc:
        logger.debug("subprocess %s failed: %s", cmd[0], exc)
        return None
    except OSError as exc:
        logger.debug("subprocess %s OSError: %s", cmd[0], exc)
        return None
    if proc.returncode != 0:
        logger.debug(
            "subprocess %s exited %d: %s",
            cmd[0], proc.returncode, (proc.stderr or "").strip()[:200],
        )
        return None
    return proc.stdout


# --- collectors -------------------------------------------------------------

def _collect_doppler() -> _DopplerView:
    """Enumerate available Doppler secrets, returning names + notes only.

    Values are deliberately discarded: the agent is told *which* keys exist
    and what they're for, and is expected to fetch values via
    ``doppler secrets get KEY --plain`` when it actually needs them.
    """
    if shutil.which("doppler") is None:
        return _DopplerView(available=False, error="doppler not on PATH")

    out = _run(["doppler", "secrets", "--json"])
    if out is None:
        return _DopplerView(
            available=False,
            error="`doppler secrets --json` failed (not authed or no scope?)",
        )

    try:
        data = json.loads(out)
    except json.JSONDecodeError as exc:
        return _DopplerView(available=False, error=f"doppler json: {exc}")

    if not isinstance(data, dict):
        return _DopplerView(available=False, error="doppler returned non-object")

    keys: list[_DopplerKey] = []
    for name, meta in data.items():
        note = ""
        if isinstance(meta, dict):
            raw_note = meta.get("note")
            if isinstance(raw_note, str):
                note = raw_note.strip()
        keys.append(_DopplerKey(name=name, note=note))
    keys.sort(key=lambda k: k.name)
    return _DopplerView(keys=keys)


def _dns_label(dns_name: str) -> str:
    """``constmac.tail-scale.ts.net.`` -> ``constmac``."""
    if not dns_name:
        return ""
    return dns_name.split(".", 1)[0]


def _collect_tailscale() -> _TailscaleView:
    """Pull the tailnet view, keeping only what's safe to read aloud.

    Specifically: short DNS label, hostname, OS, online flag. We omit
    TailscaleIPs, login email, and key material -- the agent doesn't need
    them and the prompt is round-tripped through the Cursor backend.
    """
    if shutil.which("tailscale") is None:
        return _TailscaleView(available=False, error="tailscale not on PATH")

    out = _run(["tailscale", "status", "--json"])
    if out is None:
        return _TailscaleView(
            available=False,
            error="`tailscale status --json` failed (not logged in?)",
        )

    try:
        data = json.loads(out)
    except json.JSONDecodeError as exc:
        return _TailscaleView(available=False, error=f"tailscale json: {exc}")

    def _node(raw: dict) -> Optional[_TailscaleNode]:
        if not isinstance(raw, dict):
            return None
        host = raw.get("HostName") or ""
        dns = raw.get("DNSName") or ""
        label = _dns_label(dns) or host
        if not label:
            return None
        return _TailscaleNode(
            name=label,
            hostname=host,
            os=(raw.get("OS") or "").strip() or "unknown",
            online=bool(raw.get("Online")),
        )

    self_node = _node(data.get("Self") or {})
    peers_raw = data.get("Peer") or {}
    peers: list[_TailscaleNode] = []
    if isinstance(peers_raw, dict):
        for raw in peers_raw.values():
            n = _node(raw)
            if n is not None:
                peers.append(n)
    peers.sort(key=lambda n: n.name)
    return _TailscaleView(self_node=self_node, peers=peers)


def _collect_git(paths: InstallPaths) -> _GitView:
    """Resolve the arbos source remote + HEAD.

    ``run.sh`` clones the repo to ``.arbos/src``; if the user is running
    against an in-place checkout instead (dev loop), we fall back to the
    install root.
    """
    candidates: list[Path] = []
    if paths.src_dir.exists():
        candidates.append(paths.src_dir)
    candidates.append(paths.root)

    remote = "unknown"
    branch = "unknown"
    short_sha = "unknown"

    for cand in candidates:
        if not (cand / ".git").exists():
            continue
        out = _run(["git", "remote", "get-url", "origin"], cwd=cand)
        if out:
            remote = out.strip() or remote
        out = _run(["git", "rev-parse", "--abbrev-ref", "HEAD"], cwd=cand)
        if out:
            branch = out.strip() or branch
        out = _run(["git", "rev-parse", "--short", "HEAD"], cwd=cand)
        if out:
            short_sha = out.strip() or short_sha
        if remote != "unknown":
            break

    if remote == "unknown":
        remote = "https://github.com/unarbos/arbos.git"
    return _GitView(remote=remote, branch=branch, short_sha=short_sha)


# --- rendering --------------------------------------------------------------

def _render_doppler(view: _DopplerView) -> str:
    if not view.available:
        return f"_(doppler unavailable: {view.error or 'unknown'})_"
    if not view.keys:
        return "_(no Doppler secrets visible in current scope)_"
    lines: list[str] = []
    for key in view.keys:
        if key.note:
            lines.append(f"- `{key.name}` — {key.note}")
        else:
            lines.append(f"- `{key.name}`")
    return "\n".join(lines)


def _render_tailscale(view: _TailscaleView) -> str:
    if not view.available:
        return f"_(tailscale unavailable: {view.error or 'unknown'})_"
    parts: list[str] = []
    if view.self_node is not None:
        parts.append(
            f"- self: `{view.self_node.name}` ({view.self_node.os})"
        )
    else:
        parts.append("- self: _(unknown)_")
    if view.peers:
        parts.append("- peers (reachable over Tailscale magicDNS by name):")
        for peer in view.peers:
            status = "online" if peer.online else "offline"
            parts.append(f"    - `{peer.name}` ({peer.os}, {status})")
    else:
        parts.append("- peers: _(none)_")
    return "\n".join(parts)


_PROMPT_TEMPLATE = """\
# Arbos — system context

You are **Arbos**, an always-on `cursor-agent` instance supervised by PM2 on
machine `{machine_name}`. The user just sent the message below into the
Telegram forum topic dedicated to this machine, and you are the worker that
will answer it.

## Where you live
- Install root (this is your `cwd` for every shell command): `{install_root}`
- Your own source code (read-only unless the user explicitly asks you to edit
  it): `{src_dir}`
- Local state, secrets, logs, prompts: `{arbos_dir}`
  - `config.json` — bot/supergroup/topic IDs for this install
  - `agent.log` — your own historical stdout/stderr
  - `prompts/PROMPT.md` — this file (regenerated each run)
  - `prompts/*.md` — additional context (memory, chat-history pointers, …)
  - `secrets/` — `bot_token.txt`, `api_hash.txt` (0700)
  - `tdlib/` — aiotdlib session for the human user account
  - `outbox/` — drop-zone watched once per second; any file written here
    gets posted to this Telegram topic and moved to `outbox/sent/`
    (or `outbox/failed/` with a `.error.txt` sidecar).
- Remote: `{remote}` (currently `{branch}@{short_sha}`)

## Who else exists (Tailscale tailnet)
{tailscale_block}

You can `ssh <peer-name>` directly over magicDNS — no IP, no extra config.
The user often asks you to coordinate with sibling machines this way (e.g.
"have templar pull and restart", "ssh into chakanaone and check disk").

## Available env vars (via `doppler secrets get NAME --plain`)
Values are **never** in this prompt. Fetch them on demand:

{doppler_block}

The whole agent process is launched under `doppler run`, so these are also
available as ordinary environment variables (`os.environ["OPENAI_API_KEY"]`).

## How Arbos is wired
- A single Telegram supergroup (`{supergroup_title}`, `chat_id={chat_id}`) is
  shared across every machine the user owns. Inside it there is one **forum
  topic per machine**; this machine's topic is `{machine_name}`
  (`message_thread_id={topic_id}`).
- Every non-bot text / voice / video-note message in this topic spawns a fresh
  `cursor-agent` (you). Voice + video notes are transcribed via OpenAI Whisper
  before dispatch.
- Your output streams back into a single Telegram message ("the bubble"),
  edited at most once per second, then finalised with the result text. See
  `bubble.py`, `cursor_runner.py`, `agent.py`.
- `/plan <request>` runs a 2-phase plan-then-implement: first a plan-mode
  cursor-agent produces a plan, then a second cursor-agent executes it
  verbatim. See `_handle_plan_command` in `agent.py`.
- Single-instance is enforced at three layers: PM2's unique-name constraint,
  `flock` on `.arbos/agent.lock`, and Telegram's own 409 Conflict on
  `getUpdates`.

## Conversation continuity
- Every regular message in this topic is part of **one persistent
  cursor-agent chat**, resumed via `--resume <id>` on each spawn. You can
  refer back to earlier messages, prior tool output, files you already read,
  etc. — they are all in your own session history.
- The chat id is persisted at `.arbos/chat_session.json` so memory survives
  PM2 restarts.
- The user can wipe the chat with `/reset` (or `/new`); the next message
  will then start a fresh session.
- Regular messages are **serialised** on a per-topic lock — if a second
  message arrives while you are still answering the first, it will queue
  and you will see it as the next turn rather than as a parallel run.
- `/plan` runs are intentionally **isolated** from this persistent chat —
  neither the plan nor the impl phase resumes it, so they cannot pollute
  the conversation history.
- `/update` does `git fetch + git reset --hard origin/main` in the arbos
  source dir (only when origin actually moved), reinstalls the editable
  package if `pyproject.toml`, `run.sh`, or `uv.lock` changed, then
  triggers a detached `pm2 reload arbos-<machine>`. The persistent chat
  session and all of `.arbos/` survive the restart, so memory is preserved.
- Installer + bootstrap flow lives in `cli.py` + `run.sh`; per-machine state
  machine in `state.py`.

## Sending media to this chat
You cannot call the Telegram Bot API from a child cursor-agent run, but the
host agent watches `{arbos_dir}/outbox/` once per second. To send a photo,
PDF, screenshot, log file, video, etc. into this topic, just write the file
there:

```sh
cp /tmp/diagram.png {arbos_dir}/outbox/
# optional caption (max 1024 chars):
printf 'pipeline overview' > {arbos_dir}/outbox/diagram.png.caption.txt
```

Routing by extension: `.jpg/.jpeg/.png/.webp` -> sendPhoto (≤10 MB);
`.gif` -> sendAnimation; `.mp4/.mov/.m4v` -> sendVideo; `.mp3/.m4a/.flac/
.wav` -> sendAudio; `.ogg/.opus` -> sendVoice; everything else ->
sendDocument (≤50 MB). Hidden files and `*.partial` / `*.tmp` are ignored,
so write-then-rename is safe. Successful files move to `outbox/sent/` with
a UTC timestamp prefix; failures move to `outbox/failed/` with a sibling
`.error.txt`.

## Conventions
- Use `doppler secrets get KEY --plain` to read secrets. Never write or read
  raw `.env` files.
- Python deps: `uv pip install -e .` inside `.venv/` (already activated by
  `run.sh`).
- Don't write standalone `*.md` summaries of what you did unless asked.
- Don't create unit tests unless asked.
- When editing your own source, the user's running agent is YOU — restart
  with `pm2 reload arbos-{machine_name}` (or `./run.sh restart`) for the
  change to take effect on the next message.
"""


def _render(
    *,
    paths: InstallPaths,
    config: StoredConfig,
    tailscale: _TailscaleView,
    doppler: _DopplerView,
    git: _GitView,
) -> str:
    return _PROMPT_TEMPLATE.format(
        machine_name=config.machine.name,
        install_root=paths.root,
        src_dir=paths.src_dir,
        arbos_dir=paths.arbos,
        remote=git.remote,
        branch=git.branch,
        short_sha=git.short_sha,
        tailscale_block=_render_tailscale(tailscale),
        doppler_block=_render_doppler(doppler),
        supergroup_title=config.supergroup.title,
        chat_id=config.supergroup.chat_id,
        topic_id=config.machine.topic_id,
    )


# --- public API + cache -----------------------------------------------------

@dataclass
class _CacheEntry:
    text: str
    built_at: float


_cache: dict[Path, _CacheEntry] = {}


def build_prompt(
    paths: InstallPaths,
    config: StoredConfig,
    *,
    force: bool = False,
) -> str:
    """Render the system preamble for ``paths``. Cached for ``_CACHE_TTL`` s.

    On every miss, the rendered text is also written to
    ``paths.prompt_md_path`` so the user can inspect it.
    """
    now = time.monotonic()
    cached = _cache.get(paths.root)
    if cached is not None and not force and (now - cached.built_at) < _CACHE_TTL:
        return cached.text

    tailscale = _collect_tailscale()
    doppler = _collect_doppler()
    git = _collect_git(paths)
    text = _render(
        paths=paths,
        config=config,
        tailscale=tailscale,
        doppler=doppler,
        git=git,
    )
    _cache[paths.root] = _CacheEntry(text=text, built_at=now)
    try:
        write_prompt_file(paths, text)
    except OSError as exc:
        logger.warning("could not write %s: %s", paths.prompt_md_path, exc)
    return text


def write_prompt_file(paths: InstallPaths, text: str) -> Path:
    """Persist the rendered prompt under ``.arbos/prompts/PROMPT.md``."""
    paths.prompts_dir.mkdir(parents=True, exist_ok=True)
    try:
        os.chmod(paths.prompts_dir, 0o700)
    except OSError:
        pass
    paths.prompt_md_path.write_text(text)
    return paths.prompt_md_path


def load_extra_prompts(paths: InstallPaths) -> str:
    """Concatenate every ``prompts/*.md`` other than ``PROMPT.md`` itself.

    Returned in lexical order, separated by blank lines. Empty string if the
    dir doesn't exist or contains nothing extra. Read errors on individual
    files are logged and skipped.
    """
    if not paths.prompts_dir.exists():
        return ""
    chunks: list[str] = []
    for entry in sorted(paths.prompts_dir.iterdir()):
        if not entry.is_file():
            continue
        if entry.suffix.lower() != ".md":
            continue
        if entry.name == paths.prompt_md_path.name:
            continue
        try:
            chunks.append(entry.read_text().rstrip())
        except OSError as exc:
            logger.warning("could not read %s: %s", entry, exc)
    return "\n\n".join(c for c in chunks if c)


def assemble_full_prompt(system: str, extras: str, user: str) -> str:
    """Frame the system block + extras + user message into one prompt string.

    cursor-agent has no ``--system-prompt`` flag, so we ship everything as a
    single positional prompt argument with explicit fences. The fences are
    obvious enough that downstream models reliably treat the system block as
    instructions and the user block as the actual request.
    """
    parts: list[str] = []
    if system:
        parts.append(
            "<<<ARBOS_SYSTEM_CONTEXT>>>\n" + system.rstrip()
            + "\n<<<END_ARBOS_SYSTEM_CONTEXT>>>"
        )
    if extras:
        parts.append(
            "<<<ARBOS_EXTRA_CONTEXT>>>\n" + extras.rstrip()
            + "\n<<<END_ARBOS_EXTRA_CONTEXT>>>"
        )
    parts.append(
        "<<<USER_MESSAGE>>>\n" + user.rstrip() + "\n<<<END_USER_MESSAGE>>>"
    )
    return "\n\n".join(parts)
