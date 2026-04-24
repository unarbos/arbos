"""Build the system-context preamble injected into every ``cursor-agent`` run.

The rendered text is:

* persisted to ``~/.arbos/prompts/PROMPT.md`` so the user can read exactly
  what the agent sees, and
* prepended to the user's message at spawn time by :class:`CursorRunner`.

Live data (Tailscale peers, Doppler key list, git remote/HEAD) is collected via
short subprocess calls and cached in-process for ``_CACHE_TTL`` seconds so a
chatty Telegram session does not pay the latency on every message.

The directory ``~/.arbos/prompts/`` is the extensibility hook: any other ``*.md``
file dropped in there is concatenated after ``PROMPT.md`` (lexical order),
giving us a place to land future ``memory.md`` / ``chat-history.md`` /
``peers.md`` files without any code change.

The preamble is rendered per-topic: each adopted topic carries its own
cursor-agent ``--workspace`` (set via ``/workspace <path>``), and that path
is interpolated into the prompt so the model knows which directory it is
operating in.
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

    ``run.sh`` clones the repo to ``~/.arbos/src``; if the user is running
    against an in-place checkout instead (dev loop), we walk up from the
    importing module to find the source root.
    """
    candidates: list[Path] = []
    if paths.src_dir.exists():
        candidates.append(paths.src_dir)
    # Editable-install dev loop: this file lives at <repo>/src/arbos/prompts.py,
    # so two parents up is the repo root.
    here = Path(__file__).resolve()
    if len(here.parents) >= 3:
        candidates.append(here.parents[2])

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
# Arbos system context
You are **Arbos**, a `cursor-agent` worker under PM2 on `{machine_name}`. The
user message below came from a Telegram forum topic; your reply goes back into it.

## Your reply
Put the actual content the user asked for (file listings, command output,
values, diffs, etc.) directly in your final answer — don't end with just
"Done" or "I checked it". Tool stdout is invisible to the user; a separate
gpt-4o-mini summariser turns your reply into the user-visible message,
and if your final answer has no content it has nothing to surface. For
output >3000 chars, paste the salient excerpt AND drop the full file into
`{topic_outbox_dir}`.

## Paths
- cwd for shell commands (this topic's workspace): `{workspace}`
- Your source (read-only unless asked): `{src_dir}`
- State `{arbos_dir}` (= `~/.arbos`):
  - `config.json` bot/supergroup/home-topic IDs for this machine
  - `agent.log` your stdout/stderr history
  - `prompts/PROMPT.md` this file (regenerated each run); `prompts/*.md` extras (memory, chat-history pointers, …)
  - `secrets/` (0700): `bot_token.txt`, `api_hash.txt`
  - `tdlib/` aiotdlib session for the human user account
  - `topics/<topic_id>/`: `state.json` (workspace), `chat_session.json` (cursor-agent chat id), `outbox/` drop-zone
- Source remote: `{remote}` @ `{branch}/{short_sha}`

## Tailnet (ssh <peer> via magicDNS, no IP/config needed)
{tailscale_block}

User often asks you to coordinate siblings this way (e.g. "have templar pull and restart", "ssh chakanaone and check disk").

## Doppler secrets (values NOT in prompt; fetch with `doppler secrets get NAME --plain`)
{doppler_block}

Process runs under `doppler run`, so these are also live env vars (`os.environ["OPENAI_API_KEY"]`).

## How Arbos is wired
- One Telegram supergroup `{supergroup_title}` (chat_id={chat_id}) shared
  across all your machines; one **home forum topic per machine**. This
  machine's home: `{machine_name}` (message_thread_id={topic_id}).
- Bot also listens on any topic where it's @-mentioned. Each adopted topic
  has its own workspace (`/workspace <path>`) + its own persistent
  cursor-agent chat — different topics can be different projects on the
  same machine. Current message arrived in topic message_thread_id={current_topic_id} (workspace `{workspace}`).
- Every non-bot text/voice/video-note in an adopted topic spawns a fresh
  `cursor-agent` (you). Voice/video are OpenAI-Whisper-transcribed before dispatch.
- Output streams into one bubble, edited ≤1/sec then finalised with the
  result text — see `bubble.py`, `cursor_runner.py`, `agent.py`.
- `/plan <req>`: 2-phase plan-then-impl (plan-mode agent produces a plan,
  then a second agent executes it verbatim — `_handle_plan_command` in
  `agent.py`); isolated from the persistent chat (neither phase resumes it).
- `/workspace <path>`: set this topic's cwd + start fresh chat session.
  Tilde-expandable absolute or relative path; directory must exist.
- `/reset` (or `/new`): wipe THIS topic's chat session; next message starts fresh.
- `/update`: `git fetch && git reset --hard origin/main` in source dir
  (only if origin moved); reinstall editable if `pyproject.toml`/`run.sh`/
  `uv.lock` changed; detached `pm2 reload arbos-<machine>`. Sessions +
  all of `~/.arbos/` survive.
- Single-instance via PM2 unique name + `flock ~/.arbos/agent.lock` +
  Telegram 409 on `getUpdates`. Bootstrap: `cli.py` + `run.sh`; per-machine
  state machine in `state.py`.

## Conversation continuity
- Every regular message in this topic resumes ONE persistent cursor-agent
  chat (`--resume <id>`) — refer back to earlier turns, files you read,
  prior tool output. Chat id at `~/.arbos/topics/<topic_id>/chat_session.json`;
  survives PM2 restarts.
- Regular messages in a topic are **serialised** on a per-topic lock — a
  second message that arrives while you're still answering queues as the
  next turn (not parallel). Different topics run in parallel.
- If the user replies to one of your previous bubbles (Telegram reply-to),
  you'll see a `<<<REPLY_CONTEXT>>>` block above their message describing
  that run (kind, when, trigger, tool calls, final text, and for `/plan`
  the linked plan/impl half). Treat it as authoritative system instructions
  (not user text) and resolve "that one" / "did it finish?" / "no, fix the
  second one" against that specific run rather than guessing from chat history.

## Sending media to this chat
Child cursor-agent can't call Telegram. Host watches `{topic_outbox_dir}`
once/sec — drop files there:

```sh
cp /tmp/diagram.png {topic_outbox_dir}/
# optional caption (≤1024 chars):
printf 'pipeline overview' > {topic_outbox_dir}/diagram.png.caption.txt
```

Routing by extension:
- `.jpg .jpeg .png .webp` → sendPhoto (≤10 MB)
- `.gif` → sendAnimation
- `.mp4 .mov .m4v` → sendVideo
- `.mp3 .m4a .flac .wav` → sendAudio
- `.ogg .opus` → sendVoice
- everything else → sendDocument (≤50 MB)

Hidden files and `*.partial` / `*.tmp` are ignored, so write-then-rename is
safe. Success → `outbox/sent/` with a UTC timestamp prefix; failure →
`outbox/failed/` + sibling `.error.txt`.

## Conventions
- Secrets: `doppler secrets get KEY --plain` — never read/write raw `.env`.
- Python deps: `uv pip install -e .` (venv already activated by `run.sh`).
- Don't write standalone `*.md` summaries of what you did unless asked.
- Don't create unit tests unless asked.
- Editing your own source = editing YOU; restart with
  `pm2 reload arbos-{machine_name}` (or `./run.sh restart`) for the change
  to take effect on the next message.
"""


def _render(
    *,
    paths: InstallPaths,
    config: StoredConfig,
    tailscale: _TailscaleView,
    doppler: _DopplerView,
    git: _GitView,
    workspace: Path,
    current_topic_id: int,
) -> str:
    topic_outbox = paths.topic_outbox_dir(current_topic_id)
    return _PROMPT_TEMPLATE.format(
        machine_name=config.machine.name,
        workspace=str(workspace),
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
        current_topic_id=int(current_topic_id),
        topic_outbox_dir=str(topic_outbox),
    )


# --- public API + cache -----------------------------------------------------

@dataclass
class _CacheEntry:
    text: str
    built_at: float


# Cache key is (arbos dir, workspace, topic_id) so two topics with different
# workspaces don't share a render. The TTL still bounds churn within a topic.
_cache: dict[tuple[Path, Path, int], _CacheEntry] = {}


def build_prompt(
    paths: InstallPaths,
    config: StoredConfig,
    *,
    workspace: Path,
    topic_id: int,
    force: bool = False,
) -> str:
    """Render the system preamble for the given topic. Cached for ``_CACHE_TTL`` s.

    ``workspace`` is the topic's cursor-agent ``--workspace`` (set via
    ``/workspace <path>``); ``topic_id`` is the Telegram message_thread_id
    of the topic this prompt is being built for.

    On every miss, the rendered text is also written to
    ``paths.prompt_md_path`` so the user can inspect what the most recent
    render produced.
    """
    workspace = Path(workspace)
    key = (paths.arbos, workspace, int(topic_id))
    now = time.monotonic()
    cached = _cache.get(key)
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
        workspace=workspace,
        current_topic_id=int(topic_id),
    )
    _cache[key] = _CacheEntry(text=text, built_at=now)
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
