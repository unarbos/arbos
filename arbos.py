import base64
import json
import os
import selectors
import signal
import subprocess
import sys
import time
import threading
import uuid
from pathlib import Path
from datetime import datetime
from dataclasses import dataclass, field
from typing import Any

import hashlib
import re

from dotenv import load_dotenv
import httpx
import requests
import uvicorn
from cryptography.fernet import Fernet, InvalidToken
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC
from cryptography.hazmat.primitives import hashes
from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse, StreamingResponse

WORKING_DIR = Path(__file__).parent
PROMPT_FILE = WORKING_DIR / "PROMPT.md"
CONTEXT_DIR = WORKING_DIR / "context"
CHANNELS_JSON = CONTEXT_DIR / "channels.json"
GENERAL_DIR = CONTEXT_DIR / "general"
GENERAL_CHAT_DIR = GENERAL_DIR / "chat"
FILES_DIR = CONTEXT_DIR / "files"
RESTART_FLAG = WORKING_DIR / ".restart"
CHANNEL_ID_FILE = WORKING_DIR / "channel_id.txt"
ENV_ENC_FILE = WORKING_DIR / ".env.enc"

# ── Encrypted .env ───────────────────────────────────────────────────────────

def _derive_fernet_key(passphrase: str) -> bytes:
    kdf = PBKDF2HMAC(algorithm=hashes.SHA256(), length=32,
                     salt=b"arbos-env-v1", iterations=200_000)
    return base64.urlsafe_b64encode(kdf.derive(passphrase.encode()))


def _encrypt_env_file(bot_token: str):
    """Encrypt .env → .env.enc and delete the plaintext file."""
    env_path = WORKING_DIR / ".env"
    plaintext = env_path.read_bytes()
    f = Fernet(_derive_fernet_key(bot_token))
    ENV_ENC_FILE.write_bytes(f.encrypt(plaintext))
    os.chmod(str(ENV_ENC_FILE), 0o600)
    env_path.unlink()


def _decrypt_env_content(bot_token: str) -> str:
    """Decrypt .env.enc and return plaintext (never written to disk)."""
    f = Fernet(_derive_fernet_key(bot_token))
    return f.decrypt(ENV_ENC_FILE.read_bytes()).decode()


def _load_encrypted_env(bot_token: str) -> bool:
    """Decrypt .env.enc, load into os.environ. Returns True on success."""
    if not ENV_ENC_FILE.exists():
        return False
    try:
        content = _decrypt_env_content(bot_token)
    except InvalidToken:
        return False
    for line in content.splitlines():
        line = line.split("#")[0].strip()
        if "=" not in line:
            continue
        k, v = line.split("=", 1)
        os.environ.setdefault(k.strip(), v.strip().strip("'\""))
    return True


def _save_to_encrypted_env(key: str, value: str):
    """Add/update a single key in the encrypted env file."""
    bot_token = os.environ.get("BOT_TOKEN", "")
    if not bot_token or not ENV_ENC_FILE.exists():
        return
    try:
        content = _decrypt_env_content(bot_token)
    except InvalidToken:
        return
    lines = content.splitlines()
    updated = False
    for i, line in enumerate(lines):
        stripped = line.split("#")[0].strip()
        if stripped.startswith(f"{key}="):
            lines[i] = f"{key}='{value}'"
            updated = True
            break
    if not updated:
        lines.append(f"{key}='{value}'")
    f = Fernet(_derive_fernet_key(bot_token))
    ENV_ENC_FILE.write_bytes(f.encrypt("\n".join(lines).encode()))
    os.environ[key] = value


ENV_PENDING_FILE = CONTEXT_DIR / ".env.pending"


def _init_env():
    """Load environment from .env (plaintext) or .env.enc (encrypted)."""
    env_path = WORKING_DIR / ".env"

    if env_path.exists():
        load_dotenv(env_path)
        return

    bot_token = os.environ.get("BOT_TOKEN", "")
    if ENV_ENC_FILE.exists() and bot_token:
        if _load_encrypted_env(bot_token):
            return
        print("ERROR: failed to decrypt .env.enc — wrong BOT_TOKEN?", file=sys.stderr)
        sys.exit(1)

    if ENV_ENC_FILE.exists() and not bot_token:
        print("ERROR: .env.enc exists but BOT_TOKEN not set.", file=sys.stderr)
        print("Pass it as an env var: BOT_TOKEN=xxx python arbos.py", file=sys.stderr)
        sys.exit(1)


def _process_pending_env():
    """Pick up env vars the operator agent wrote to .env.pending and persist them."""
    with _pending_env_lock:
        if not ENV_PENDING_FILE.exists():
            return
        content = ENV_PENDING_FILE.read_text().strip()
        ENV_PENDING_FILE.unlink(missing_ok=True)
        if not content:
            return

        for line in content.splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            k, v = line.split("=", 1)
            k, v = k.strip(), v.strip().strip("'\"")
            os.environ[k] = v

        env_path = WORKING_DIR / ".env"
        if env_path.exists():
            with open(env_path, "a") as f:
                f.write("\n" + content + "\n")
        elif ENV_ENC_FILE.exists():
            bot_token = os.environ.get("BOT_TOKEN", "")
            if bot_token:
                try:
                    existing = _decrypt_env_content(bot_token)
                except InvalidToken:
                    existing = ""
                new_content = existing.rstrip() + "\n" + content + "\n"
                enc = Fernet(_derive_fernet_key(bot_token))
                ENV_ENC_FILE.write_bytes(enc.encrypt(new_content.encode()))

        _reload_env_secrets()
        _log(f"loaded pending env vars from .env.pending")


_init_env()

# ── Redaction ────────────────────────────────────────────────────────────────

_SECRET_KEY_WORDS = {"KEY", "SECRET", "TOKEN", "PASSWORD", "SEED", "CREDENTIAL"}

_SECRET_PATTERNS = [
    re.compile(r'sk-[a-zA-Z0-9_\-]{20,}'),
    re.compile(r'sk_[a-zA-Z0-9_\-]{20,}'),
    re.compile(r'sk-proj-[a-zA-Z0-9_\-]{20,}'),
    re.compile(r'sk-or-v1-[a-fA-F0-9]{20,}'),
    re.compile(r'ghp_[a-zA-Z0-9]{20,}'),
    re.compile(r'gho_[a-zA-Z0-9]{20,}'),
    re.compile(r'hf_[a-zA-Z0-9]{20,}'),
    re.compile(r'AKIA[0-9A-Z]{16}'),
    re.compile(r'cpk_[a-zA-Z0-9._\-]{20,}'),
    re.compile(r'crsr_[a-zA-Z0-9]{20,}'),
    re.compile(r'dckr_pat_[a-zA-Z0-9_\-]{10,}'),
    re.compile(r'sn\d+_[a-zA-Z0-9_]{10,}'),
    re.compile(r'tpn-[a-zA-Z0-9_\-]{10,}'),
    re.compile(r'wandb_v\d+_[a-zA-Z0-9]{10,}'),
    re.compile(r'basilica_[a-zA-Z0-9]{20,}'),
    re.compile(r'MT[A-Za-z0-9]+\.[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]{20,}'),
]


def _load_env_secrets() -> set[str]:
    """Build redaction blocklist from env vars whose names suggest secrets."""
    secrets = set()
    for key, val in os.environ.items():
        if len(val) < 16:
            continue
        key_upper = key.upper()
        if any(w in key_upper for w in _SECRET_KEY_WORDS):
            secrets.add(val)
    return secrets


_env_secrets: set[str] = _load_env_secrets()


def _reload_env_secrets():
    global _env_secrets
    _env_secrets = _load_env_secrets()


def _redact_secrets(text: str) -> str:
    """Strip known secrets and common key patterns from outgoing text."""
    for secret in _env_secrets:
        if secret in text:
            text = text.replace(secret, "[REDACTED]")
    for pattern in _SECRET_PATTERNS:
        text = pattern.sub("[REDACTED]", text)
    return text
MAX_CONCURRENT = int(os.environ.get("CLAUDE_MAX_CONCURRENT", "4"))
PROVIDER = os.environ.get("PROVIDER", "chutes")
PROXY_PORT = int(os.environ.get("PROXY_PORT", "8089"))
PROXY_TIMEOUT = int(os.environ.get("PROXY_TIMEOUT", "600"))
CHUTES_API_KEY = os.environ.get("CHUTES_API_KEY", "")

if PROVIDER == "openrouter":
    CLAUDE_MODEL = os.environ.get("CLAUDE_MODEL", "anthropic/claude-opus-4.6")
    LLM_API_KEY = os.environ.get("OPENROUTER_API_KEY", "")
    LLM_BASE_URL = "https://openrouter.ai/api"
    COST_PER_M_INPUT = float(os.environ.get("COST_PER_M_INPUT", "5.00"))
    COST_PER_M_OUTPUT = float(os.environ.get("COST_PER_M_OUTPUT", "25.00"))
    CHUTES_ROUTING_AGENT = CLAUDE_MODEL
    CHUTES_ROUTING_BOT = CLAUDE_MODEL
else:
    CLAUDE_MODEL = os.environ.get("CLAUDE_MODEL", "moonshotai/Kimi-K2.5-TEE")
    CHUTES_BASE_URL = os.environ.get("CHUTES_BASE_URL", "https://llm.chutes.ai/v1")
    LLM_API_KEY = CHUTES_API_KEY
    LLM_BASE_URL = CHUTES_BASE_URL
    CHUTES_POOL = os.environ.get(
        "CHUTES_POOL",
        "moonshotai/Kimi-K2.5-TEE,zai-org/GLM-5-TEE,MiniMaxAI/MiniMax-M2.5-TEE,zai-org/GLM-4.7-TEE",
    )
    CHUTES_ROUTING_AGENT = os.environ.get("CHUTES_ROUTING_AGENT", f"{CHUTES_POOL}:throughput")
    CHUTES_ROUTING_BOT = os.environ.get("CHUTES_ROUTING_BOT", f"{CHUTES_POOL}:latency")
    COST_PER_M_INPUT = float(os.environ.get("COST_PER_M_INPUT", "0.14"))
    COST_PER_M_OUTPUT = float(os.environ.get("COST_PER_M_OUTPUT", "0.60"))
IS_ROOT = os.getuid() == 0
MAX_RETRIES = int(os.environ.get("CLAUDE_MAX_RETRIES", "5"))
CLAUDE_TIMEOUT = int(os.environ.get("CLAUDE_TIMEOUT", "600"))
_tls = threading.local()
_log_lock = threading.Lock()
_chatlog_lock = threading.Lock()
_pending_env_lock = threading.Lock()
_shutdown = threading.Event()
_claude_semaphore = threading.Semaphore(MAX_CONCURRENT)
_step_count = 0
_token_usage = {"input": 0, "output": 0}
_token_lock = threading.Lock()
_child_procs: set[subprocess.Popen] = set()
_child_procs_lock = threading.Lock()
_channel_procs: dict[str, subprocess.Popen] = {}
_channel_procs_lock = threading.Lock()


def _pm2_list_names() -> set[str]:
    """Return the set of currently running PM2 process names (excluding 'arbos' itself)."""
    try:
        r = subprocess.run(
            ["pm2", "jlist"], capture_output=True, text=True, timeout=10,
        )
        if r.returncode != 0:
            return set()
        procs = json.loads(r.stdout)
        return {p["name"] for p in procs if p.get("name") != "arbos"}
    except Exception:
        return set()


def _pm2_delete(names: list[str]):
    """Delete a list of PM2 processes by name."""
    for name in names:
        try:
            subprocess.run(
                ["pm2", "delete", name],
                capture_output=True, text=True, timeout=10,
            )
            _log(f"pm2 delete: {name}")
        except Exception as exc:
            _log(f"pm2 delete failed for {name}: {exc}")


# ── Channel state ────────────────────────────────────────────────────────────


@dataclass
class ChannelState:
    channel_id: str
    name: str = ""
    dir_name: str = ""
    goal: str = ""
    step_count: int = 0
    delay: int = 0
    running: bool = False
    pin_text: str = ""
    pin_hash: str = ""
    repo_url: str = ""
    category_id: str = ""
    category_name: str = ""
    last_run: str = ""
    last_finished: str = ""
    started_at: str = ""
    total_input_tokens: int = 0
    total_output_tokens: int = 0
    total_chat_input_tokens: int = 0
    total_chat_output_tokens: int = 0
    failures: int = 0
    pm2_procs: list[str] = field(default_factory=list)
    resources: list[dict] = field(default_factory=list)
    active_thread_id: str = ""  # Discord thread where the goal/loop is running
    loop_thread: threading.Thread | None = field(default=None, repr=False)
    wake: threading.Event = field(default_factory=threading.Event, repr=False)
    stop_event: threading.Event = field(default_factory=threading.Event, repr=False)


_channels: dict[str, ChannelState] = {}
_channels_lock = threading.Lock()
_categories: dict[str, str] = {}  # category_id -> category_name
_guild_id: str | None = None
_running_category_id: str | None = None
_paused_category_id: str | None = None


def _channel_dir(channel_id: str) -> Path:
    cs = _channels.get(channel_id)
    if cs and cs.dir_name:
        return CONTEXT_DIR / cs.dir_name
    return CONTEXT_DIR / f"channel-{channel_id}"


def _pin_file(channel_id: str) -> Path:
    return _channel_dir(channel_id) / "pin"


def _state_file(channel_id: str) -> Path:
    return _channel_dir(channel_id) / "state"


def _goal_file(channel_id: str) -> Path:
    return _channel_dir(channel_id) / "goal"


def _channel_runs_dir(channel_id: str) -> Path:
    return _channel_dir(channel_id) / "runs"


def _channel_chat_dir(channel_id: str) -> Path:
    return _channel_dir(channel_id) / "chat"


def _channel_env_file(channel_id: str) -> Path:
    return _channel_dir(channel_id) / ".env"


def _step_msg_file(channel_id: str) -> Path:
    return _channel_dir(channel_id) / ".step_msg"


def _save_channels():
    """Persist channel metadata to channels.json. Caller must hold _channels_lock."""
    data = {}
    for cid, cs in _channels.items():
        data[cid] = {
            "name": cs.name,
            "dir_name": cs.dir_name,
            "goal": cs.goal,
            "step_count": cs.step_count,
            "delay": cs.delay,
            "running": cs.running,
            "pin_text": cs.pin_text,
            "pin_hash": cs.pin_hash,
            "repo_url": cs.repo_url,
            "category_id": cs.category_id,
            "category_name": cs.category_name,
            "last_run": cs.last_run,
            "last_finished": cs.last_finished,
            "started_at": cs.started_at,
            "total_input_tokens": cs.total_input_tokens,
            "total_output_tokens": cs.total_output_tokens,
            "total_chat_input_tokens": cs.total_chat_input_tokens,
            "total_chat_output_tokens": cs.total_chat_output_tokens,
            "failures": cs.failures,
            "pm2_procs": cs.pm2_procs,
            "resources": cs.resources,
            "active_thread_id": cs.active_thread_id,
        }
    CHANNELS_JSON.parent.mkdir(parents=True, exist_ok=True)
    CHANNELS_JSON.write_text(json.dumps(data, indent=2))


def _load_channels():
    """Load channel metadata from channels.json into _channels dict."""
    global _channels
    if not CHANNELS_JSON.exists():
        return
    try:
        data = json.loads(CHANNELS_JSON.read_text())
    except (json.JSONDecodeError, OSError):
        return
    for cid, info in data.items():
        dir_name = info.get("dir_name", "")
        if not dir_name:
            continue
        if not (CONTEXT_DIR / dir_name).exists():
            continue
        pin_text = info.get("pin_text", "") or info.get("goal_text", "")
        pin_hash = info.get("pin_hash", "") or info.get("goal_hash", "")
        _channels[cid] = ChannelState(
            channel_id=cid,
            name=info.get("name", ""),
            dir_name=dir_name,
            goal=info.get("goal", ""),
            step_count=info.get("step_count", 0),
            delay=info.get("delay", 0),
            running=info.get("running", False),
            pin_text=pin_text,
            pin_hash=pin_hash,
            repo_url=info.get("repo_url", ""),
            category_id=info.get("category_id", ""),
            category_name=info.get("category_name", ""),
            last_run=info.get("last_run", ""),
            last_finished=info.get("last_finished", ""),
            started_at=info.get("started_at", ""),
            total_input_tokens=info.get("total_input_tokens", 0),
            total_output_tokens=info.get("total_output_tokens", 0),
            total_chat_input_tokens=info.get("total_chat_input_tokens", 0),
            total_chat_output_tokens=info.get("total_chat_output_tokens", 0),
            failures=info.get("failures", 0),
            pm2_procs=info.get("pm2_procs", []),
            resources=info.get("resources", []),
            active_thread_id=info.get("active_thread_id", ""),
        )


def _format_last_time(iso_ts: str) -> str:
    if not iso_ts:
        return "never"
    try:
        dt = datetime.fromisoformat(iso_ts)
        secs = (datetime.now() - dt).total_seconds()
        if secs < 60:
            return f"{int(secs)}s ago"
        if secs < 3600:
            return f"{int(secs / 60)}m ago"
        if secs < 86400:
            return f"{int(secs / 3600)}h ago"
        return f"{int(secs / 86400)}d ago"
    except (ValueError, TypeError):
        return "unknown"


def _channel_status_label(cs: ChannelState) -> str:
    if cs.running:
        return "running"
    return "stopped"


def _file_log(msg: str):
    fh = getattr(_tls, "log_fh", None)
    if fh:
        with _log_lock:
            ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
            fh.write(f"{ts}  {_redact_secrets(msg)}\n")
            fh.flush()


def _log(msg: str, *, blank: bool = False):
    safe = _redact_secrets(msg)
    if blank:
        print(flush=True)
    print(safe, flush=True)
    _file_log(safe)


def fmt_duration(seconds: float) -> str:
    if seconds < 60:
        return f"{seconds:.1f}s"
    m, s = divmod(int(seconds), 60)
    return f"{m}m {s}s"


def _fmt_uptime(iso_started: str) -> str:
    """Human-readable uptime from an ISO timestamp to now."""
    if not iso_started:
        return "—"
    try:
        started = datetime.fromisoformat(iso_started)
        delta = datetime.now() - started
        total_s = int(delta.total_seconds())
        if total_s < 0:
            return "—"
        days, rem = divmod(total_s, 86400)
        hours, rem = divmod(rem, 3600)
        mins, _ = divmod(rem, 60)
        parts = []
        if days:
            parts.append(f"{days}d")
        if hours:
            parts.append(f"{hours}h")
        parts.append(f"{mins}m")
        return " ".join(parts)
    except (ValueError, TypeError):
        return "—"


def _fmt_tokens_short(n: int) -> str:
    if n >= 1_000_000:
        return f"{n / 1_000_000:.1f}M"
    if n >= 1_000:
        return f"{n / 1_000:.1f}k"
    return str(n)


COST_PER_INPUT_MTOK = float(os.environ.get("COST_PER_INPUT_MTOK", "3.0"))
COST_PER_OUTPUT_MTOK = float(os.environ.get("COST_PER_OUTPUT_MTOK", "15.0"))


def _estimate_cost(input_tokens: int, output_tokens: int) -> float:
    return (input_tokens / 1_000_000) * COST_PER_INPUT_MTOK + (output_tokens / 1_000_000) * COST_PER_OUTPUT_MTOK


def _channel_status_text(cs: ChannelState) -> str:
    """Build a rich status block for a channel."""
    icon = "\U0001f7e9" if cs.running and cs.goal else "\u2b1c"
    status = "running" if cs.running else "stopped"

    loop_in = cs.total_input_tokens
    loop_out = cs.total_output_tokens
    chat_in = cs.total_chat_input_tokens
    chat_out = cs.total_chat_output_tokens
    total_in = loop_in + chat_in
    total_out = loop_out + chat_out

    loop_cost = _estimate_cost(loop_in, loop_out)
    chat_cost = _estimate_cost(chat_in, chat_out)
    total_cost = loop_cost + chat_cost

    delay_min = cs.delay / 60 if cs.delay else 0
    delay_str = f"{delay_min:.1f}m" if cs.delay else "none"
    scope_label = f"category: {cs.category_name}" if cs.category_id else "global"
    uptime = _fmt_uptime(cs.started_at) if cs.running else "—"

    siblings = [
        c for cid, c in _channels.items()
        if c.category_id == cs.category_id and cid != cs.channel_id and cs.category_id
    ]
    sibling_info = f" | siblings: {', '.join(c.name for c in siblings)}" if siblings else ""

    lines = [
        f"{icon} **{cs.name}** [{status}]",
        "",
        f"**Scope:** {scope_label}{sibling_info}",
        f"**Goal:** {cs.goal[:400] if cs.goal else '(no goal)'}",
        "",
        "```",
        f"Status    : {status}",
        f"Steps     : {cs.step_count}",
        f"Failures  : {cs.failures}",
        f"Delay     : {delay_str}",
        f"Uptime    : {uptime}",
        f"Last step : {_format_last_time(cs.last_finished)}",
        f"Invoked   : {_format_last_time(cs.last_run)}",
        "",
        f"Loop tokens  : {_fmt_tokens_short(loop_in)} in / {_fmt_tokens_short(loop_out)} out  (${loop_cost:.2f})",
        f"Chat tokens  : {_fmt_tokens_short(chat_in)} in / {_fmt_tokens_short(chat_out)} out  (${chat_cost:.2f})",
        f"Total tokens : {_fmt_tokens_short(total_in)} in / {_fmt_tokens_short(total_out)} out  (${total_cost:.2f})",
        "```",
    ]

    if cs.pm2_procs:
        lines.append(f"**Processes ({len(cs.pm2_procs)}):** {', '.join(cs.pm2_procs)}")

    if cs.resources:
        lines.append(f"**Resources ({len(cs.resources)}):**")
        for r in cs.resources:
            rtype = r.get("type", "?")
            rname = r.get("name", "?")
            rmeta = r.get("meta", {})
            meta_bits = " | ".join(f"{k}={v}" for k, v in rmeta.items()) if rmeta else ""
            destroy = " | destroy: `" + r["destroy_cmd"] + "`" if r.get("destroy_cmd") else ""
            meta_str = f" ({meta_bits})" if meta_bits else ""
            lines.append(f"  - `{rtype}` **{rname}**{meta_str}{destroy}")

    sf = _state_file(cs.channel_id)
    state_text = sf.read_text().strip()[:400] if sf.exists() else "(empty)"
    pin_preview = cs.pin_text[:200] if cs.pin_text else "(no pin)"
    lines.append(f"**Pin:** {pin_preview}")
    lines.append(f"**State:** {state_text}")

    return "\n".join(lines)


def _reset_tokens():
    with _token_lock:
        _token_usage["input"] = 0
        _token_usage["output"] = 0


def _get_tokens() -> tuple[int, int]:
    with _token_lock:
        return _token_usage["input"], _token_usage["output"]


def fmt_tokens(inp: int, out: int, elapsed: float = 0) -> str:
    def _k(n: int) -> str:
        return f"{n / 1000:.1f}k" if n >= 1000 else str(n)
    tps = ""
    if elapsed > 0 and out > 0:
        tps = f" | {out / elapsed:.0f} t/s"
    cost = (inp * COST_PER_M_INPUT + out * COST_PER_M_OUTPUT) / 1_000_000
    cost_str = f" | ${cost:.4f}" if cost >= 0.0001 else ""
    return f"{_k(inp)} in / {_k(out)} out{tps}{cost_str}"


# ── Scope context builders ───────────────────────────────────────────────────

def _channel_summary(cs: ChannelState, *, max_pin: int = 500, max_state: int = 500) -> str:
    """One-channel summary for scope context: status, goal, pin snippet, state snippet."""
    icon = "\U0001f7e9" if cs.running and cs.goal else "\u2b1c"
    step_info = f" step {cs.step_count}" if cs.step_count else ""
    lines = [f"### {icon} #{cs.name}{step_info}"]
    if cs.goal:
        goal_preview = cs.goal[:200] + ("..." if len(cs.goal) > 200 else "")
        lines.append(f"Goal: {goal_preview}")
    pf = _pin_file(cs.channel_id)
    if pf.exists():
        pin = pf.read_text().strip()
        if pin:
            snippet = pin[:max_pin] + ("..." if len(pin) > max_pin else "")
            lines.append(f"Pin: {snippet}")
    sf = _state_file(cs.channel_id)
    if sf.exists():
        state = sf.read_text().strip()
        if state:
            snippet = state[:max_state] + ("..." if len(state) > max_state else "")
            lines.append(f"State: {snippet}")
    return "\n".join(lines)


def _build_category_scope(channel_id: str, category_id: str) -> str:
    """Context from sibling channels in the same category."""
    siblings = [
        cs for cid, cs in _channels.items()
        if cs.category_id == category_id and cid != channel_id
    ]
    if not siblings:
        return ""
    cat_name = _categories.get(category_id, category_id)
    parts = [
        f"## Scope: {cat_name}\n",
        "You share this category with the following sibling channels.",
        "Their goals, pins, and state are shown for coordination.\n",
    ]
    for cs in sorted(siblings, key=lambda c: c.name):
        parts.append(_channel_summary(cs))
    return "\n\n".join(parts)


def _build_global_scope(channel_id: str) -> str:
    """Context from ALL channels grouped by category (condensed)."""
    by_cat: dict[str, list[ChannelState]] = {}
    uncategorized: list[ChannelState] = []
    for cid, cs in _channels.items():
        if cid == channel_id:
            continue
        if cs.category_id:
            by_cat.setdefault(cs.category_id, []).append(cs)
        else:
            uncategorized.append(cs)
    if not by_cat and not uncategorized:
        return ""
    parts = ["## Scope: Global\n", "You have global visibility across all categories and channels.\n"]
    for cat_id, members in sorted(by_cat.items(), key=lambda x: _categories.get(x[0], x[0])):
        cat_name = _categories.get(cat_id, cat_id)
        parts.append(f"### Category: {cat_name}")
        for cs in sorted(members, key=lambda c: c.name):
            icon = "\U0001f7e9" if cs.running and cs.goal else "\u2b1c"
            step_info = f" step {cs.step_count}" if cs.step_count else ""
            goal_preview = ""
            if cs.goal:
                goal_preview = f" | goal: {cs.goal[:100]}{'...' if len(cs.goal) > 100 else ''}"
            pin_preview = ""
            pf = _pin_file(cs.channel_id)
            if pf.exists():
                pin = pf.read_text().strip()
                if pin:
                    pin_preview = f" | pin: {pin[:120]}{'...' if len(pin) > 120 else ''}"
            parts.append(f"- {icon} **#{cs.name}**{step_info}{goal_preview}{pin_preview}")
    if uncategorized:
        parts.append("### Uncategorized")
        for cs in sorted(uncategorized, key=lambda c: c.name):
            status = "running" if cs.running else "paused"
            parts.append(f"- **#{cs.name}** [{status}]")
    return "\n".join(parts)


def _build_scope_context(channel_id: str) -> str:
    """Build scope context for a channel based on its category membership."""
    cs = _channels.get(channel_id)
    if not cs:
        return ""
    if cs.category_id:
        return _build_category_scope(channel_id, cs.category_id)
    return _build_global_scope(channel_id)


def _build_scope_tree(channel_id: str) -> str:
    """Build a tree-graph string showing where a channel sits in the scope hierarchy."""
    cs = _channels.get(channel_id)
    if not cs:
        return ""

    by_cat: dict[str, list[ChannelState]] = {}
    uncategorized: list[ChannelState] = []
    for cid, c in _channels.items():
        if c.category_id:
            by_cat.setdefault(c.category_id, []).append(c)
        else:
            uncategorized.append(c)

    def _ch_line(c: ChannelState, is_self: bool = False) -> str:
        icon = "\U0001f7e9" if c.running and c.goal else "\u2b1c"
        marker = " **\u2190 you are here**" if is_self else ""
        goal_bit = ""
        if c.goal:
            g = c.goal[:60] + ("..." if len(c.goal) > 60 else "")
            goal_bit = f" — {g}"
        return f"{icon} #{c.name}{goal_bit}{marker}"

    lines = []

    if cs.category_id:
        scope_label = f"Category: {cs.category_name or cs.category_id}"
    else:
        scope_label = "Global"
    lines.append(f"**Scope: {scope_label}**\n```")

    cat_ids = sorted(by_cat.keys(), key=lambda x: _categories.get(x, x))
    all_sections: list[tuple[str | None, list[ChannelState]]] = []
    for cat_id in cat_ids:
        all_sections.append((_categories.get(cat_id, cat_id), by_cat[cat_id]))
    if uncategorized:
        all_sections.append((None, uncategorized))

    for idx, (cat_name, members) in enumerate(all_sections):
        is_last_section = idx == len(all_sections) - 1
        branch = "\u2514\u2500\u2500" if is_last_section else "\u251c\u2500\u2500"
        cont = "    " if is_last_section else "\u2502   "
        if cat_name:
            lines.append(f"{branch} \U0001f4c1 {cat_name}")
        else:
            lines.append(f"{branch} (uncategorized)")
        sorted_members = sorted(members, key=lambda c: c.name)
        for j, c in enumerate(sorted_members):
            is_last_ch = j == len(sorted_members) - 1
            ch_branch = "\u2514\u2500\u2500" if is_last_ch else "\u251c\u2500\u2500"
            lines.append(f"{cont}{ch_branch} {_ch_line(c, c.channel_id == channel_id)}")

    lines.append("```")

    if cs.category_id:
        siblings = [c for c in by_cat.get(cs.category_id, []) if c.channel_id != channel_id]
        if siblings:
            names = ", ".join(f"#{c.name}" for c in sorted(siblings, key=lambda c: c.name))
            lines.append(f"Siblings: {names}")
        lines.append(f"Scope: channels in **{cs.category_name}** share context.")
    else:
        lines.append("Scope: **global** — you can see all categories and channels.")

    lines.append("\nUse `/help` for commands, `/goal <text>` to set a goal, `/start` to begin.")
    return "\n".join(lines)


def _send_scope_welcome(channel_id: str):
    """Send a scope/context welcome message to a channel or Discord thread."""
    token = os.getenv("BOT_TOKEN", "")
    if not token:
        return
    tree = _build_scope_tree(channel_id)
    if not tree:
        return
    try:
        requests.post(
            f"{DISCORD_API}/channels/{channel_id}/messages",
            headers=_discord_headers(token),
            json={"content": tree[:DISCORD_MSG_LIMIT]},
            timeout=15,
        )
    except Exception as exc:
        _log(f"failed to send scope welcome to {channel_id}: {str(exc)[:120]}")


def _send_thread_welcome(thread_id: str, parent_channel_id: str):
    """Send a context welcome message to a new Discord thread."""
    token = os.getenv("BOT_TOKEN", "")
    if not token:
        return
    cs = _channels.get(parent_channel_id)
    if not cs:
        return

    lines = ["**Context: thread under #{}**\n```".format(cs.name)]
    icon = "\U0001f7e9" if cs.running and cs.goal else "\u2b1c"
    goal_bit = f"\n\u2502   Goal: {cs.goal[:80]}{'...' if len(cs.goal) > 80 else ''}" if cs.goal else ""
    scope_bit = f" ({cs.category_name})" if cs.category_name else " (global)"
    lines.append(f"\u2514\u2500\u2500 {icon} #{cs.name}{scope_bit}{goal_bit}")
    lines.append(f"    \u2514\u2500\u2500 this thread \u2190 you are here")
    lines.append("```")
    lines.append(f"This thread inherits #{cs.name}'s context (pin, state, scope).")
    lines.append("Messages here are routed to the channel's chatbot.")

    text = "\n".join(lines)
    try:
        requests.post(
            f"{DISCORD_API}/channels/{thread_id}/messages",
            headers=_discord_headers(token),
            json={"content": text[:DISCORD_MSG_LIMIT]},
            timeout=15,
        )
    except Exception as exc:
        _log(f"failed to send thread welcome to {thread_id}: {str(exc)[:120]}")


# ── Prompt helpers ───────────────────────────────────────────────────────────

def _build_step_prompt(channel_id: str, step: int = 0) -> str:
    """Build prompt for an autonomous step: PROMPT + PIN + SCOPE + GOAL + STATE + CHAT."""
    parts = []
    if PROMPT_FILE.exists():
        text = PROMPT_FILE.read_text().strip()
        if text:
            parts.append(text)

    cs = _channels.get(channel_id)
    dir_name = cs.dir_name if cs else f"channel-{channel_id}"
    abs_dir = str(_channel_dir(channel_id).resolve())

    pf = _pin_file(channel_id)
    if pf.exists():
        pin_text = pf.read_text().strip()
        if pin_text:
            parts.append(f"## Pin\n\n{pin_text}")

    scope_ctx = _build_scope_context(channel_id)
    if scope_ctx:
        parts.append(scope_ctx)

    gf = _goal_file(channel_id)
    goal_text = gf.read_text().strip() if gf.exists() else (cs.goal if cs else "")
    header = f"## {cs.name if cs else 'Channel'} — Step {step}" if step else f"## {cs.name if cs else 'Channel'}"
    step_meta = (
        f"Your workspace is `context/{dir_name}/` (state, chat/, runs/).\n"
        f"Absolute workspace path: `{abs_dir}`\n"
        f"All code, scripts, data, and artifacts you create must go inside this directory.\n"
        f"You are running in autonomous mode. Focus on making progress toward the goal.\n"
        f"Your output will be posted as a step summary — do not call `send`.\n"
        f"When you have completed the goal, signal completion by running: `python arbos.py done`\n\n"
        f"## Operator chat\n"
        f"The operator may send messages in the chat below. Check for recent operator messages and respond to them.\n"
        f"If the operator asks you to do something (change approach, fix something, restart), prioritize that.\n\n"
        f"## Control commands\n"
        f"You can control your own loop:\n"
        f"- `python arbos.py ctl delay <seconds>` — set delay between steps\n"
        f"- `python arbos.py ctl restart` — restart the loop from the next step\n"
        f"- `python arbos.py ctl goal <new goal>` — update your goal\n"
        f"- `python arbos.py ctl pause` — pause your loop\n"
        f"- `python arbos.py done` — mark goal complete and stop\n\n"
        f"## Resource tracking\n"
        f"Track machines, VMs, containers, or services you create:\n"
        f"- `python arbos.py resource add <type> <name> [--destroy \"teardown cmd\"] [--meta key=value ...]`\n"
        f"- `python arbos.py resource rm <name>` — untrack a resource\n"
        f"- `python arbos.py resource list` — list all tracked resources\n"
        f"Register resources so they can be monitored and destroyed when the channel is deleted."
    )
    if cs and cs.repo_url:
        step_meta += f"\nGitHub repo: {cs.repo_url}"
    parts.append(f"{header}\n\n**Goal:**\n{goal_text}\n\n{step_meta}")

    sf = _state_file(channel_id)
    if sf.exists():
        state_text = sf.read_text().strip()
        if state_text:
            parts.append(f"## State\n\n{state_text}")

    chatlog = load_chatlog(channel_id=channel_id)
    if chatlog:
        parts.append(chatlog)

    return "\n\n".join(parts)


def _build_channel_chat_prompt(channel_id: str, user_text: str) -> str:
    """Build prompt for channel chat mode: PROMPT + PIN + SCOPE + CHANNEL + GOAL + STATE + CHAT + USER."""
    cs = _channels.get(channel_id)
    dir_name = cs.dir_name if cs else f"channel-{channel_id}"
    abs_dir = str(_channel_dir(channel_id).resolve())

    parts = []
    if PROMPT_FILE.exists():
        text = PROMPT_FILE.read_text().strip()
        if text:
            parts.append(text)

    pf = _pin_file(channel_id)
    if pf.exists():
        pin_text = pf.read_text().strip()
        if pin_text:
            parts.append(f"## Pin\n\n{pin_text}")

    scope_ctx = _build_scope_context(channel_id)
    if scope_ctx:
        parts.append(scope_ctx)

    loop_status = "running" if cs and cs.running else "stopped"
    meta = (
        f"Your workspace is `context/{dir_name}/` (state, chat/).\n"
        f"Absolute workspace path: `{abs_dir}`\n"
        f"All code, scripts, data, and artifacts you create must go inside this directory.\n"
        f"Loop status: {loop_status} (step {cs.step_count if cs else 0})"
    )
    if cs and cs.repo_url:
        meta += f"\nGitHub repo: {cs.repo_url}"
    parts.append(f"## Channel\n\nYou are a chatbot for this channel. Respond to messages directly — your text output is streamed to Discord. Do NOT use `arbos.py send` (it is suppressed in chat mode).\n\n{meta}")

    if cs and cs.goal:
        parts.append(f"## Goal\n\n{cs.goal}")

    sf = _state_file(channel_id)
    if sf.exists():
        state_text = sf.read_text().strip()
        if state_text:
            parts.append(f"## State\n\n{state_text}")

    chatlog = load_chatlog(channel_id=channel_id)
    if chatlog:
        parts.append(chatlog)

    parts.append(f"## User message\n{user_text}")
    return "\n\n".join(parts)


def make_run_dir(channel_id: str = "", thread_id: str = "") -> Path:
    if channel_id:
        runs_dir = _channel_runs_dir(channel_id)
    else:
        runs_dir = GENERAL_DIR / "runs"
    runs_dir.mkdir(parents=True, exist_ok=True)
    ts = datetime.now().strftime("%Y%m%d_%H%M%S")
    run_dir = runs_dir / ts
    run_dir.mkdir(parents=True, exist_ok=True)
    return run_dir


def _write_chatlog(chat_dir: Path, role: str, text: str):
    """Append to chatlog in the given directory, rolling files when size exceeds limit."""
    chat_dir.mkdir(parents=True, exist_ok=True)
    max_file_size, max_files = 4000, 50
    existing = sorted(chat_dir.glob("*.jsonl"))
    current = existing[-1] if existing and existing[-1].stat().st_size < max_file_size else None
    if current is None:
        current = chat_dir / f"{datetime.now().strftime('%Y%m%d_%H%M%S')}.jsonl"
    entry = json.dumps({"role": role, "text": _redact_secrets(text[:1000]), "ts": datetime.now().isoformat()})
    with open(current, "a", encoding="utf-8") as f:
        f.write(entry + "\n")
    all_files = sorted(chat_dir.glob("*.jsonl"))
    for old in all_files[:-max_files]:
        old.unlink(missing_ok=True)


def log_chat(role: str, text: str, channel_id: str = ""):
    """Append to chatlog. channel_id="" writes to general chat."""
    with _chatlog_lock:
        chat_dir = _channel_chat_dir(channel_id) if channel_id else GENERAL_CHAT_DIR
        _write_chatlog(chat_dir, role, text)


def _load_chatlog_from(chat_dir: Path, header: str, max_chars: int = 8000) -> str:
    """Load recent chat history from a chat directory."""
    if not chat_dir.exists():
        return ""
    files = sorted(chat_dir.glob("*.jsonl"))
    if not files:
        return ""
    lines: list[str] = []
    total = 0
    for f in reversed(files):
        for raw in reversed(f.read_text().strip().splitlines()):
            try:
                msg = json.loads(raw)
            except json.JSONDecodeError:
                continue
            entry = f"[{msg.get('ts', '?')[:16]}] {msg['role']}: {msg['text']}"
            if total + len(entry) > max_chars:
                lines.reverse()
                return f"{header}\n\n" + "\n".join(lines)
            lines.append(entry)
            total += len(entry) + 1
    lines.reverse()
    return f"{header}\n\n" + "\n".join(lines) if lines else ""


def load_chatlog(max_chars: int = 8000, channel_id: str = "") -> str:
    if channel_id:
        return _load_chatlog_from(_channel_chat_dir(channel_id), "## Channel chat", max_chars)
    return _load_chatlog_from(GENERAL_CHAT_DIR, "## Recent Discord chat", max_chars)




# ── Step update helpers ──────────────────────────────────────────────────────


DISCORD_API = "https://discord.com/api/v10"
DISCORD_MSG_LIMIT = 2000


def _discord_headers(token: str) -> dict:
    return {"Authorization": f"Bot {token}", "Content-Type": "application/json"}


def _step_update_target(channel_id: str = "") -> tuple[str, str] | None:
    token = os.getenv("BOT_TOKEN")
    if not token:
        _log("step update skipped: BOT_TOKEN not set")
        return None
    if channel_id:
        return token, channel_id
    if not CHANNEL_ID_FILE.exists():
        _log("step update skipped: channel_id.txt not found")
        return None
    general_id = CHANNEL_ID_FILE.read_text().strip()
    if not general_id:
        _log("step update skipped: empty channel_id.txt")
        return None
    return token, general_id


def _send_discord_text(text: str, *, target: tuple[str, str] | None = None, channel_id: str = "") -> bool:
    target = target or _step_update_target()
    if not target:
        return False
    token, cid = target
    text = _redact_secrets(text)
    try:
        response = requests.post(
            f"{DISCORD_API}/channels/{cid}/messages",
            headers=_discord_headers(token),
            json={"content": text[:DISCORD_MSG_LIMIT]},
            timeout=15,
        )
        response.raise_for_status()
    except Exception as exc:
        _log(f"discord send failed: {str(exc)[:120]}")
        return False
    log_chat("bot", text[:1000], channel_id=channel_id)
    _log("discord message sent")
    return True


def _send_discord_new(text: str, *, target: tuple[str, str] | None = None,
                      reply_to: str | None = None) -> str | None:
    """Send a new Discord message and return its message id."""
    target = target or _step_update_target()
    if not target:
        return None
    token, channel_id = target
    text = _redact_secrets(text)
    try:
        payload: dict = {"content": text[:DISCORD_MSG_LIMIT]}
        if reply_to:
            payload["message_reference"] = {"message_id": reply_to}
        response = requests.post(
            f"{DISCORD_API}/channels/{channel_id}/messages",
            headers=_discord_headers(token),
            json=payload,
            timeout=15,
        )
        response.raise_for_status()
        return response.json().get("id")
    except Exception as exc:
        _log(f"discord send failed: {str(exc)[:120]}")
        return None


def _edit_discord_text(message_id: str, text: str, *, target: tuple[str, str] | None = None) -> bool:
    """Edit an existing Discord message."""
    target = target or _step_update_target()
    if not target:
        return False
    token, channel_id = target
    text = _redact_secrets(text)
    try:
        requests.patch(
            f"{DISCORD_API}/channels/{channel_id}/messages/{message_id}",
            headers=_discord_headers(token),
            json={"content": text[:DISCORD_MSG_LIMIT]},
            timeout=15,
        )
        return True
    except Exception:
        return False


def _send_discord_document(file_path: str, caption: str = "", *, target: tuple[str, str] | None = None, channel_id: str = "") -> bool:
    """Send a file as a Discord attachment."""
    target = target or _step_update_target()
    if not target:
        return False
    token, cid = target
    caption = _redact_secrets(caption)[:DISCORD_MSG_LIMIT]
    try:
        with open(file_path, "rb") as f:
            response = requests.post(
                f"{DISCORD_API}/channels/{cid}/messages",
                headers={"Authorization": f"Bot {token}"},
                data={"content": caption} if caption else None,
                files={"files[0]": (Path(file_path).name, f)},
                timeout=60,
            )
        response.raise_for_status()
        _log(f"discord file sent: {Path(file_path).name}")
        log_chat("bot", f"[sent file: {Path(file_path).name}] {caption}", channel_id=channel_id)
        return True
    except Exception as exc:
        _log(f"discord file send failed: {str(exc)[:120]}")
        return False


_send_discord_photo = _send_discord_document


def _download_discord_attachment(url: str, filename: str) -> Path:
    """Download a Discord attachment and save it to FILES_DIR."""
    FILES_DIR.mkdir(parents=True, exist_ok=True)
    resp = requests.get(url, timeout=60)
    resp.raise_for_status()
    downloaded = resp.content
    save_path = FILES_DIR / filename
    if save_path.exists():
        stem, suffix = save_path.stem, save_path.suffix
        ts = datetime.now().strftime("%H%M%S")
        save_path = FILES_DIR / f"{stem}_{ts}{suffix}"
    save_path.write_bytes(downloaded)
    _log(f"saved discord file: {save_path.name} ({len(downloaded)} bytes)")
    return save_path


# ── Channel helpers ──────────────────────────────────────────────────────────


def _sanitize_channel_name(text: str) -> str:
    text = text.lower()
    text = re.sub(r'[^a-z0-9\s-]', '', text)
    text = re.sub(r'[\s]+', '-', text)
    text = re.sub(r'-+', '-', text)
    text = text.strip('-')
    return text[:100] or "channel"


def _is_managed_channel(channel_id: str) -> bool:
    return channel_id in _channels


def _discover_categories(guild_id: str) -> tuple[str | None, str | None]:
    """Discover ALL guild categories, populate _categories, return (running_id, paused_id) for backward compat."""
    global _categories
    token = os.getenv("BOT_TOKEN", "")
    if not token:
        return None, None
    running_id = None
    paused_id = None
    try:
        resp = requests.get(
            f"{DISCORD_API}/guilds/{guild_id}/channels",
            headers=_discord_headers(token),
            timeout=15,
        )
        resp.raise_for_status()
        for ch in resp.json():
            if ch["type"] == 4:
                cat_id = ch["id"]
                cat_name = ch["name"]
                _categories[cat_id] = cat_name
                if cat_name.lower() == "running":
                    running_id = cat_id
                elif cat_name.lower() == "paused":
                    paused_id = cat_id
    except Exception:
        pass
    if not running_id:
        try:
            resp = requests.post(
                f"{DISCORD_API}/guilds/{guild_id}/channels",
                headers=_discord_headers(token),
                json={"name": "Running", "type": 4},
                timeout=15,
            )
            resp.raise_for_status()
            running_id = resp.json()["id"]
            _categories[running_id] = "Running"
        except Exception as exc:
            _log(f"failed to create Running category: {str(exc)[:200]}")
    if not paused_id:
        try:
            resp = requests.post(
                f"{DISCORD_API}/guilds/{guild_id}/channels",
                headers=_discord_headers(token),
                json={"name": "Paused", "type": 4},
                timeout=15,
            )
            resp.raise_for_status()
            paused_id = resp.json()["id"]
            _categories[paused_id] = "Paused"
        except Exception as exc:
            _log(f"failed to create Paused category: {str(exc)[:200]}")
    _log(f"discovered {len(_categories)} categories: {list(_categories.values())}")
    return running_id, paused_id


def _fetch_channel_pins(channel_id: str) -> str:
    """Fetch pinned messages for a channel and return concatenated text."""
    token = os.getenv("BOT_TOKEN", "")
    if not token:
        return ""
    try:
        resp = requests.get(
            f"{DISCORD_API}/channels/{channel_id}/pins",
            headers=_discord_headers(token),
            timeout=15,
        )
        resp.raise_for_status()
        pins = resp.json()
        if not pins:
            return ""
        parts = []
        for pin in reversed(pins):
            content = pin.get("content", "").strip()
            if content:
                parts.append(content)
        return "\n\n".join(parts)
    except Exception as exc:
        _log(f"failed to fetch pins for {channel_id}: {str(exc)[:120]}")
        return ""


def _archive_thread(thread_id: str):
    """Archive a Discord thread."""
    token = os.getenv("BOT_TOKEN", "")
    if not token:
        return
    try:
        requests.patch(
            f"{DISCORD_API}/channels/{thread_id}",
            headers=_discord_headers(token),
            json={"archived": True, "locked": True},
            timeout=15,
        )
        _log(f"archived thread {thread_id}")
    except Exception as exc:
        _log(f"failed to archive thread {thread_id}: {str(exc)[:120]}")


def _create_pending_channels(guild_id: str):
    """Create Discord channels listed in context/pending_channels.json and register them."""
    pending_path = CONTEXT_DIR / "pending_channels.json"
    if not pending_path.exists():
        return
    token = os.getenv("BOT_TOKEN", "")
    if not token or not guild_id:
        return
    try:
        pending = json.loads(pending_path.read_text())
    except (json.JSONDecodeError, OSError):
        _log("failed to read pending_channels.json")
        return
    if not isinstance(pending, list):
        pending = [pending]
    for entry in pending:
        name = entry.get("name", "")
        if not name:
            continue
        goal = entry.get("goal", "")
        state = entry.get("state", "")
        delay = entry.get("delay", 0)
        running = entry.get("running", True)
        try:
            resp = requests.get(
                f"{DISCORD_API}/guilds/{guild_id}/channels",
                headers=_discord_headers(token), timeout=15,
            )
            resp.raise_for_status()
            existing = resp.json()
            already_exists = any(ch.get("name") == name and ch.get("type") == 0 for ch in existing)
            if already_exists:
                _log(f"pending channel '{name}' already exists in guild, skipping")
                continue
        except Exception as exc:
            _log(f"failed to check guild channels: {str(exc)[:120]}")
            continue
        payload: dict = {"name": name, "type": 0}
        if entry.get("category_id"):
            payload["parent_id"] = entry["category_id"]
        try:
            resp = requests.post(
                f"{DISCORD_API}/guilds/{guild_id}/channels",
                headers=_discord_headers(token), json=payload, timeout=15,
            )
            resp.raise_for_status()
            ch_data = resp.json()
            cid = str(ch_data["id"])
            _log(f"created pending Discord channel '{name}' (id={cid})")
        except Exception as exc:
            _log(f"failed to create pending channel '{name}': {str(exc)[:120]}")
            continue
        cat_id = entry.get("category_id", "")
        cat_name = entry.get("category_name", "")
        cs = _setup_channel_context(cid, name, running, category_id=cat_id, category_name=cat_name)
        if goal:
            cs.goal = goal
            (CONTEXT_DIR / cs.dir_name / "goal").write_text(goal)
        if state:
            (CONTEXT_DIR / cs.dir_name / "state").write_text(state)
        if delay:
            cs.delay = delay
        with _channels_lock:
            _save_channels()
        _sync_channel_name(cid)
    try:
        pending_path.unlink()
        _log("pending_channels.json processed and removed")
    except OSError:
        pass


def _setup_channel_context(channel_id: str, channel_name: str, is_running: bool,
                           category_id: str = "", category_name: str = "") -> ChannelState:
    """Create context directory and GitHub repo for a newly detected channel."""
    channel_name = _strip_name_prefix(channel_name)
    dir_name = _sanitize_channel_name(channel_name)
    if (CONTEXT_DIR / dir_name).exists():
        dir_name = f"{dir_name}-{channel_id[:8]}"
    cs = ChannelState(
        channel_id=channel_id, name=channel_name, dir_name=dir_name,
        running=is_running, category_id=category_id, category_name=category_name,
    )

    with _channels_lock:
        _channels[channel_id] = cs

    cdir = CONTEXT_DIR / dir_name
    cdir.mkdir(parents=True, exist_ok=True)
    state_f = cdir / "state"
    if not state_f.exists():
        state_f.write_text("")
    (cdir / "runs").mkdir(parents=True, exist_ok=True)
    (cdir / "chat").mkdir(parents=True, exist_ok=True)
    (cdir / ".gitignore").write_text("runs/*/output.txt\n.step_msg\n__pycache__/\n*.pyc\n.venv/\n")

    repo_url = _create_github_repo(f"arbos-{dir_name}", description=f"Arbos channel: {channel_name}")
    if repo_url:
        _init_channel_git_repo(cdir, repo_url)
        _add_channel_submodule(dir_name, repo_url)
        cs.repo_url = repo_url

    pin_text = _fetch_channel_pins(channel_id)
    if pin_text:
        (cdir / "pin").write_text(pin_text)
        cs.pin_text = pin_text
        cs.pin_hash = hashlib.sha256(pin_text.encode()).hexdigest()[:16]

    with _channels_lock:
        _save_channels()
    scope_label = f"category={category_name}" if category_id else "global"
    _log(f"channel setup: {channel_name} (id={channel_id}, dir={dir_name}, running={is_running}, scope={scope_label})")
    _send_scope_welcome(channel_id)
    _sync_channel_name(channel_id)
    return cs


def _destroy_channel_resources(cs: ChannelState):
    """Run destroy commands for all tracked resources on a channel."""
    for r in cs.resources:
        destroy_cmd = r.get("destroy_cmd", "")
        if not destroy_cmd:
            continue
        rname = r.get("name", "?")
        rtype = r.get("type", "?")
        _log(f"channel {cs.name}: destroying resource [{rtype}] {rname}: {destroy_cmd}")
        try:
            result = subprocess.run(
                destroy_cmd, shell=True, capture_output=True, text=True, timeout=60,
            )
            if result.returncode == 0:
                _log(f"channel {cs.name}: resource [{rtype}] {rname} destroyed successfully")
            else:
                _log(f"channel {cs.name}: resource [{rtype}] {rname} destroy failed (rc={result.returncode}): {result.stderr[:200]}")
        except subprocess.TimeoutExpired:
            _log(f"channel {cs.name}: resource [{rtype}] {rname} destroy timed out")
        except Exception as exc:
            _log(f"channel {cs.name}: resource [{rtype}] {rname} destroy error: {str(exc)[:200]}")


def _delete_channel_context(channel_id: str):
    """Kill processes, destroy resources, and remove context for a deleted channel."""
    import shutil
    with _channel_procs_lock:
        proc = _channel_procs.get(channel_id)
    if proc and proc.poll() is None:
        _log(f"channel {channel_id}: killing claude subprocess pid={proc.pid}")
        proc.kill()
    with _channels_lock:
        cs = _channels.get(channel_id)
        if not cs:
            return
        cdir = _channel_dir(channel_id)
        pm2_names = list(cs.pm2_procs)
        resources = list(cs.resources)
        cs.stop_event.set()
        cs.wake.set()
        cs.running = False
        lt = cs.loop_thread
        del _channels[channel_id]
        _save_channels()
    if lt and lt.is_alive():
        lt.join(timeout=10)
    if pm2_names:
        _log(f"channel {channel_id}: deleting pm2 processes: {pm2_names}")
        _pm2_delete(pm2_names)
    if resources:
        _log(f"channel {channel_id}: destroying {len(resources)} resource(s)")
        _destroy_channel_resources(cs)
    if cdir.exists():
        shutil.rmtree(cdir, ignore_errors=True)
    _log(f"channel {channel_id} context deleted")


def _add_discord_reaction(channel_id: str, message_id: str, emoji: str):
    token = os.getenv("BOT_TOKEN", "")
    if not token:
        return
    try:
        requests.put(
            f"{DISCORD_API}/channels/{channel_id}/messages/{message_id}/reactions/{emoji}/@me",
            headers=_discord_headers(token),
            timeout=10,
        )
    except Exception:
        pass


def _delete_discord_message(channel_id: str, message_id: str):
    token = os.getenv("BOT_TOKEN", "")
    if not token:
        return
    try:
        requests.delete(
            f"{DISCORD_API}/channels/{channel_id}/messages/{message_id}",
            headers=_discord_headers(token),
            timeout=10,
        )
    except Exception:
        pass


def _update_channel_topic(channel_id: str, topic: str):
    token = os.getenv("BOT_TOKEN", "")
    if not token or not channel_id:
        return
    try:
        requests.patch(
            f"{DISCORD_API}/channels/{channel_id}",
            headers=_discord_headers(token),
            json={"topic": topic[:1024]},
            timeout=15,
        )
    except Exception:
        pass


_NAME_PREFIX_RE = re.compile(r'^[\U0001f7e9\u2b1c][\d]*\s*[-\u2014]\s*')


def _strip_name_prefix(name: str) -> str:
    """Strip the status emoji + delay prefix from a channel name to get the base name."""
    return _NAME_PREFIX_RE.sub('', name).strip() or name


def _channel_display_name(cs: ChannelState) -> str:
    """Build the Discord display name: 🟩5 - name or ⬜ - name.
    If the goal is running in a thread, parent channel stays ⬜."""
    base = _strip_name_prefix(cs.name)
    if cs.running and cs.goal and not cs.active_thread_id:
        delay_min = cs.delay // 60 if cs.delay else 0
        delay_str = str(delay_min) if delay_min else ""
        return f"\U0001f7e9{delay_str} - {base}"
    return f"\u2b1c - {base}"


def _sync_channel_name(channel_id: str):
    """Rename the Discord channel to reflect current status (emoji + delay + name)."""
    token = os.getenv("BOT_TOKEN", "")
    if not token or not channel_id:
        return
    cs = _channels.get(channel_id)
    if not cs:
        return
    new_name = _channel_display_name(cs)
    try:
        requests.patch(
            f"{DISCORD_API}/channels/{channel_id}",
            headers=_discord_headers(token),
            json={"name": new_name[:100]},
            timeout=15,
        )
        _log(f"channel {cs.name}: renamed to '{new_name}'")
    except Exception as exc:
        _log(f"channel {cs.name}: rename failed: {str(exc)[:120]}")


# ── GitHub helpers ───────────────────────────────────────────────────────────

GITHUB_API = "https://api.github.com"


def _github_headers() -> dict:
    token = os.environ.get("GITHUB_TOKEN", "")
    return {"Authorization": f"token {token}", "Accept": "application/vnd.github.v3+json"}


def _github_username() -> str:
    return os.environ.get("GITHUB_USERNAME", "")


def _create_github_repo(repo_name: str, description: str = "") -> str | None:
    """Create a GitHub repo and return its https clone URL, or None on failure."""
    token = os.environ.get("GITHUB_TOKEN", "")
    if not token:
        _log("github: GITHUB_TOKEN not set, skipping repo creation")
        return None
    try:
        resp = requests.post(
            f"{GITHUB_API}/user/repos",
            headers=_github_headers(),
            json={"name": repo_name, "description": description[:350], "private": True, "auto_init": True},
            timeout=30,
        )
        if resp.status_code == 422:
            username = _github_username()
            if username:
                url = f"https://github.com/{username}/{repo_name}"
                _log(f"github: repo already exists: {url}")
                return url
        resp.raise_for_status()
        url = resp.json().get("html_url", "")
        _log(f"github: created repo {url}")
        return url
    except Exception as exc:
        _log(f"github: repo creation failed: {str(exc)[:200]}")
        return None


def _init_channel_git_repo(channel_dir: Path, repo_url: str) -> bool:
    """Initialize a git repo in the channel directory, set remote, and make initial commit."""
    token = os.environ.get("GITHUB_TOKEN", "")
    username = _github_username()
    if not token or not username:
        return False
    auth_url = repo_url.replace("https://", f"https://{username}:{token}@")
    try:
        env = os.environ.copy()
        env["GIT_TERMINAL_PROMPT"] = "0"
        def _run_git(*args, **kwargs):
            return subprocess.run(
                ["git"] + list(args), cwd=channel_dir, env=env,
                capture_output=True, text=True, timeout=30, **kwargs,
            )
        _run_git("init")
        _run_git("remote", "add", "origin", auth_url)
        _run_git("config", "user.email", "arbos@bot")
        _run_git("config", "user.name", "Arbos")
        _run_git("fetch", "origin")
        merge = _run_git("merge", "origin/main", "--allow-unrelated-histories", "-m", "merge remote")
        if merge.returncode != 0:
            _run_git("checkout", "-b", "main")
        _run_git("add", "-A")
        _run_git("commit", "-m", "initial channel context", "--allow-empty")
        push = _run_git("push", "-u", "origin", "main")
        if push.returncode != 0:
            _run_git("push", "-u", "origin", "main", "--force")
        _log(f"github: initialized channel repo at {channel_dir.name}")
        return True
    except Exception as exc:
        _log(f"github: init failed for {channel_dir.name}: {str(exc)[:200]}")
        return False


def _add_channel_submodule(channel_dir_name: str, repo_url: str):
    """Add the channel repo as a git submodule of the main Arbos project."""
    token = os.environ.get("GITHUB_TOKEN", "")
    username = _github_username()
    if not token or not username:
        return
    auth_url = repo_url.replace("https://", f"https://{username}:{token}@")
    try:
        env = os.environ.copy()
        env["GIT_TERMINAL_PROMPT"] = "0"
        submodule_path = f"context/{channel_dir_name}"
        result = subprocess.run(
            ["git", "submodule", "add", "-f", auth_url, submodule_path],
            cwd=WORKING_DIR, env=env, capture_output=True, text=True, timeout=30,
        )
        if result.returncode == 0:
            subprocess.run(
                ["git", "add", ".gitmodules", submodule_path],
                cwd=WORKING_DIR, env=env, capture_output=True, text=True, timeout=10,
            )
            subprocess.run(
                ["git", "commit", "-m", f"add submodule: {channel_dir_name}"],
                cwd=WORKING_DIR, env=env, capture_output=True, text=True, timeout=10,
            )
            _log(f"github: added submodule {submodule_path}")
        else:
            _log(f"github: submodule add returned {result.returncode}: {result.stderr[:200]}")
    except Exception as exc:
        _log(f"github: submodule add failed: {str(exc)[:200]}")


def _push_channel_context(channel_id: str, step_label: str = ""):
    """Commit and push the channel's context directory to its GitHub repo."""
    cs = _channels.get(channel_id)
    if not cs or not cs.repo_url:
        return
    cdir = _channel_dir(channel_id)
    if not (cdir / ".git").exists():
        return
    token = os.environ.get("GITHUB_TOKEN", "")
    username = _github_username()
    if not token or not username:
        return
    try:
        env = os.environ.copy()
        env["GIT_TERMINAL_PROMPT"] = "0"
        auth_url = cs.repo_url.replace("https://", f"https://{username}:{token}@")
        def _run_git(*args):
            return subprocess.run(
                ["git"] + list(args), cwd=cdir, env=env,
                capture_output=True, text=True, timeout=60,
            )
        _run_git("remote", "set-url", "origin", auth_url)
        _run_git("add", "-A")
        msg = step_label or "update"
        commit = _run_git("commit", "-m", msg)
        if commit.returncode != 0:
            return
        push = _run_git("push", "origin", "main")
        if push.returncode == 0:
            _log(f"github: pushed channel {channel_id[:8]} context ({msg})")
        else:
            _log(f"github: push failed for channel {channel_id[:8]}: {push.stderr[:200]}")
    except Exception as exc:
        _log(f"github: push failed for channel {channel_id[:8]}: {str(exc)[:200]}")


# ── Chutes proxy (Anthropic Messages API → OpenAI Chat Completions) ──────────

_proxy_app = FastAPI(title="Chutes Proxy")


def _convert_tools_to_openai(anthropic_tools: list[dict]) -> list[dict]:
    out = []
    for t in anthropic_tools:
        out.append({
            "type": "function",
            "function": {
                "name": t["name"],
                "description": t.get("description", ""),
                "parameters": t.get("input_schema", {"type": "object", "properties": {}}),
            },
        })
    return out


def _convert_messages_to_openai(
    messages: list[dict], system: str | list | None = None
) -> list[dict]:
    out: list[dict] = []

    if system:
        if isinstance(system, list):
            text_parts = [b["text"] for b in system if b.get("type") == "text"]
            system = "\n\n".join(text_parts)
        if system:
            out.append({"role": "system", "content": system})

    for msg in messages:
        role = msg["role"]
        content = msg.get("content", "")

        if isinstance(content, str):
            out.append({"role": role, "content": content})
            continue

        if not isinstance(content, list):
            out.append({"role": role, "content": str(content)})
            continue

        text_parts: list[str] = []
        tool_calls: list[dict] = []
        tool_results: list[dict] = []
        image_parts: list[dict] = []

        for block in content:
            btype = block.get("type", "")

            if btype == "text":
                text_parts.append(block["text"])

            elif btype == "tool_use":
                tool_calls.append({
                    "id": block["id"],
                    "type": "function",
                    "function": {
                        "name": block["name"],
                        "arguments": json.dumps(block.get("input", {})),
                    },
                })

            elif btype == "tool_result":
                result_content = block.get("content", "")
                if isinstance(result_content, list):
                    result_content = "\n".join(
                        b.get("text", "") for b in result_content if b.get("type") == "text"
                    )
                tool_results.append({
                    "role": "tool",
                    "tool_call_id": block["tool_use_id"],
                    "content": str(result_content),
                })

            elif btype == "image":
                source = block.get("source", {})
                if source.get("type") == "base64":
                    image_parts.append({
                        "type": "image_url",
                        "image_url": {
                            "url": f"data:{source.get('media_type', 'image/png')};base64,{source['data']}"
                        },
                    })

        if role == "assistant":
            oai_msg: dict[str, Any] = {"role": "assistant"}
            if text_parts:
                oai_msg["content"] = "\n".join(text_parts)
            else:
                oai_msg["content"] = None
            if tool_calls:
                oai_msg["tool_calls"] = tool_calls
            out.append(oai_msg)

        elif role == "user":
            if tool_results:
                for tr in tool_results:
                    out.append(tr)
            if text_parts or image_parts:
                if image_parts:
                    content_blocks = [{"type": "text", "text": t} for t in text_parts] + image_parts
                    out.append({"role": "user", "content": content_blocks})
                elif text_parts:
                    out.append({"role": "user", "content": "\n".join(text_parts)})
        else:
            out.append({"role": role, "content": "\n".join(text_parts) if text_parts else ""})

    return out


def _build_openai_request(body: dict, *, routing: str = "agent") -> dict:
    routing_model = CHUTES_ROUTING_BOT if routing == "bot" else CHUTES_ROUTING_AGENT
    oai: dict[str, Any] = {
        "model": routing_model,
        "messages": _convert_messages_to_openai(
            body.get("messages", []),
            system=body.get("system"),
        ),
    }
    if "max_tokens" in body:
        oai["max_tokens"] = body["max_tokens"]
    if body.get("tools"):
        oai["tools"] = _convert_tools_to_openai(body["tools"])
        oai["tool_choice"] = "auto"
    if body.get("temperature") is not None:
        oai["temperature"] = body["temperature"]
    if body.get("top_p") is not None:
        oai["top_p"] = body["top_p"]
    if body.get("stream"):
        oai["stream"] = True
        oai["stream_options"] = {"include_usage": True}
    return oai


def _openai_response_to_anthropic(oai_resp: dict, model: str) -> dict:
    choice = oai_resp.get("choices", [{}])[0]
    message = choice.get("message", {})
    finish = choice.get("finish_reason", "stop")

    content_blocks: list[dict] = []
    if message.get("content"):
        content_blocks.append({"type": "text", "text": message["content"]})
    for tc in (message.get("tool_calls") or []):
        try:
            args = json.loads(tc["function"]["arguments"])
        except (json.JSONDecodeError, KeyError):
            args = {}
        content_blocks.append({
            "type": "tool_use",
            "id": tc.get("id", f"toolu_{uuid.uuid4().hex[:12]}"),
            "name": tc["function"]["name"],
            "input": args,
        })

    if finish == "tool_calls":
        stop_reason = "tool_use"
    elif finish == "length":
        stop_reason = "max_tokens"
    else:
        stop_reason = "end_turn"

    usage = oai_resp.get("usage", {})
    return {
        "id": oai_resp.get("id", f"msg_{uuid.uuid4().hex}"),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content_blocks or [{"type": "text", "text": ""}],
        "stop_reason": stop_reason,
        "stop_sequence": None,
        "usage": {
            "input_tokens": usage.get("prompt_tokens", 0),
            "output_tokens": usage.get("completion_tokens", 0),
        },
    }


def _sse_event(event: str, data: dict) -> str:
    return f"event: {event}\ndata: {json.dumps(data)}\n\n"


async def _stream_openai_to_anthropic(oai_response: httpx.Response, model: str):
    msg_id = f"msg_{uuid.uuid4().hex}"
    yield _sse_event("message_start", {
        "type": "message_start",
        "message": {
            "id": msg_id, "type": "message", "role": "assistant",
            "model": model, "content": [], "stop_reason": None,
            "stop_sequence": None,
            "usage": {"input_tokens": 0, "output_tokens": 0},
        },
    })

    block_idx = 0
    in_text_block = False
    tool_calls_accum: dict[int, dict] = {}
    stop_reason = "end_turn"
    usage = {"input_tokens": 0, "output_tokens": 0}
    logged_stream_model = False

    async for line in oai_response.aiter_lines():
        if not line.startswith("data: "):
            continue
        data_str = line[6:].strip()
        if data_str == "[DONE]":
            break
        try:
            chunk = json.loads(data_str)
        except json.JSONDecodeError:
            continue

        if not logged_stream_model and chunk.get("model"):
            _log(f"proxy: stream model={chunk['model']}")
            logged_stream_model = True

        if chunk.get("usage"):
            u = chunk["usage"]
            usage["input_tokens"] = u.get("prompt_tokens", usage["input_tokens"])
            usage["output_tokens"] = u.get("completion_tokens", usage["output_tokens"])

        choices = chunk.get("choices", [])
        if not choices:
            continue

        delta = choices[0].get("delta", {})
        finish = choices[0].get("finish_reason")

        if finish == "tool_calls":
            stop_reason = "tool_use"
        elif finish == "length":
            stop_reason = "max_tokens"
        elif finish == "stop":
            stop_reason = "end_turn"

        if delta.get("content"):
            if not in_text_block:
                yield _sse_event("content_block_start", {
                    "type": "content_block_start",
                    "index": block_idx,
                    "content_block": {"type": "text", "text": ""},
                })
                in_text_block = True
            yield _sse_event("content_block_delta", {
                "type": "content_block_delta",
                "index": block_idx,
                "delta": {"type": "text_delta", "text": delta["content"]},
            })

        if delta.get("tool_calls"):
            if in_text_block:
                yield _sse_event("content_block_stop", {
                    "type": "content_block_stop", "index": block_idx,
                })
                block_idx += 1
                in_text_block = False
            for tc in delta["tool_calls"]:
                tc_idx = tc.get("index", 0)
                if tc_idx not in tool_calls_accum:
                    tool_calls_accum[tc_idx] = {
                        "id": tc.get("id", f"toolu_{uuid.uuid4().hex[:12]}"),
                        "name": tc.get("function", {}).get("name", ""),
                        "arguments": "",
                        "block_idx": block_idx,
                    }
                    yield _sse_event("content_block_start", {
                        "type": "content_block_start",
                        "index": block_idx,
                        "content_block": {
                            "type": "tool_use",
                            "id": tool_calls_accum[tc_idx]["id"],
                            "name": tool_calls_accum[tc_idx]["name"],
                            "input": {},
                        },
                    })
                    block_idx += 1
                args_chunk = tc.get("function", {}).get("arguments", "")
                if args_chunk:
                    tool_calls_accum[tc_idx]["arguments"] += args_chunk
                    yield _sse_event("content_block_delta", {
                        "type": "content_block_delta",
                        "index": tool_calls_accum[tc_idx]["block_idx"],
                        "delta": {"type": "input_json_delta", "partial_json": args_chunk},
                    })

    with _token_lock:
        _token_usage["input"] += usage["input_tokens"]
        _token_usage["output"] += usage["output_tokens"]

    if in_text_block:
        yield _sse_event("content_block_stop", {
            "type": "content_block_stop", "index": block_idx,
        })
    for tc in tool_calls_accum.values():
        yield _sse_event("content_block_stop", {
            "type": "content_block_stop", "index": tc["block_idx"],
        })

    yield _sse_event("message_delta", {
        "type": "message_delta",
        "delta": {"stop_reason": stop_reason, "stop_sequence": None},
        "usage": {"output_tokens": usage["output_tokens"]},
    })
    yield _sse_event("message_stop", {"type": "message_stop"})


def _chutes_headers() -> dict:
    return {
        "Authorization": f"Bearer {LLM_API_KEY}",
        "Content-Type": "application/json",
    }


@_proxy_app.get("/health")
async def _proxy_health():
    return {"status": "ok"}


@_proxy_app.get("/")
async def _proxy_root():
    return {
        "proxy": "chutes",
        "pool": CHUTES_POOL,
        "agent_routing": CHUTES_ROUTING_AGENT,
        "bot_routing": CHUTES_ROUTING_BOT,
        "status": "running",
    }


_CONTEXT_LENGTH_RE = re.compile(
    r"maximum context length is (\d+) tokens.*?(\d+) output tokens.*?(\d+) input tokens",
    re.DOTALL,
)
PROXY_MAX_RETRIES = 3


def _parse_context_length_error(error_msg: str) -> tuple[int, int, int] | None:
    """Extract (context_limit, requested_output, input_tokens) from a context-length 400."""
    m = _CONTEXT_LENGTH_RE.search(error_msg)
    if m:
        return int(m.group(1)), int(m.group(2)), int(m.group(3))
    return None


def _maybe_reduce_max_tokens(oai_request: dict, error_msg: str) -> bool:
    """If the error is a context-length overflow, reduce max_tokens to fit. Returns True if adjusted."""
    parsed = _parse_context_length_error(error_msg)
    if not parsed:
        return False
    ctx_limit, _req_output, input_tokens = parsed
    headroom = ctx_limit - input_tokens
    if headroom < 1024:
        return False
    new_max = max(1024, headroom - 64)
    old_max = oai_request.get("max_tokens", 0)
    if new_max >= old_max:
        return False
    oai_request["max_tokens"] = new_max
    _log(f"proxy: reduced max_tokens {old_max} -> {new_max} (ctx_limit={ctx_limit}, input={input_tokens})")
    return True


@_proxy_app.post("/v1/messages")
async def _proxy_messages(request: Request):
    body = await request.json()
    stream = body.get("stream", False)
    model = body.get("model", CLAUDE_MODEL)
    routing = "bot" if model == "bot" else "agent"
    oai_request = _build_openai_request(body, routing=routing)
    routing_label = CHUTES_ROUTING_BOT if routing == "bot" else CHUTES_ROUTING_AGENT

    if stream:
        last_error_msg = ""
        for attempt in range(1, PROXY_MAX_RETRIES + 1):
            try:
                client = httpx.AsyncClient(timeout=httpx.Timeout(PROXY_TIMEOUT))
                resp = await client.send(
                    client.build_request(
                        "POST", f"{CHUTES_BASE_URL}/chat/completions",
                        json=oai_request, headers=_chutes_headers(),
                    ),
                    stream=True,
                )
                if resp.status_code != 200:
                    error_body = await resp.aread()
                    await resp.aclose()
                    await client.aclose()
                    last_error_msg = error_body.decode()[:500]
                    _log(f"proxy: chutes returned {resp.status_code} (attempt {attempt}/{PROXY_MAX_RETRIES}): {last_error_msg[:300]}")

                    if resp.status_code == 400 and _maybe_reduce_max_tokens(oai_request, last_error_msg):
                        continue
                    if attempt < PROXY_MAX_RETRIES:
                        continue

                    return JSONResponse(status_code=502, content={
                        "type": "error", "error": {
                            "type": "api_error",
                            "message": f"Chutes routing failed ({resp.status_code}): {last_error_msg[:300]}",
                        },
                    })

                async def generate(resp=resp, cl=client):
                    try:
                        _log(f"proxy: streaming [{routing}] via {routing_label}")
                        async for event in _stream_openai_to_anthropic(resp, model):
                            yield event
                    finally:
                        await resp.aclose()
                        await cl.aclose()

                return StreamingResponse(
                    generate(), media_type="text/event-stream",
                    headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
                )
            except httpx.TimeoutException:
                last_error_msg = f"timed out after {PROXY_TIMEOUT}s"
                _log(f"proxy: {last_error_msg} (attempt {attempt}/{PROXY_MAX_RETRIES})")
                if attempt < PROXY_MAX_RETRIES:
                    continue
                return JSONResponse(status_code=502, content={
                    "type": "error", "error": {
                        "type": "api_error",
                        "message": f"Chutes routing {last_error_msg}",
                    },
                })
            except Exception as exc:
                last_error_msg = str(exc)[:300]
                _log(f"proxy: error (attempt {attempt}/{PROXY_MAX_RETRIES}): {last_error_msg}")
                if attempt < PROXY_MAX_RETRIES:
                    continue
                return JSONResponse(status_code=502, content={
                    "type": "error", "error": {
                        "type": "api_error",
                        "message": f"Chutes routing error: {last_error_msg}",
                    },
                })

    else:
        oai_request.pop("stream", None)
        oai_request.pop("stream_options", None)
        last_error_msg = ""
        for attempt in range(1, PROXY_MAX_RETRIES + 1):
            try:
                async with httpx.AsyncClient(timeout=httpx.Timeout(PROXY_TIMEOUT)) as client:
                    resp = await client.post(
                        f"{CHUTES_BASE_URL}/chat/completions",
                        json=oai_request, headers=_chutes_headers(),
                    )
                if resp.status_code != 200:
                    last_error_msg = resp.text[:500]
                    _log(f"proxy: chutes returned {resp.status_code} (attempt {attempt}/{PROXY_MAX_RETRIES}): {last_error_msg[:300]}")

                    if resp.status_code == 400 and _maybe_reduce_max_tokens(oai_request, last_error_msg):
                        continue
                    if attempt < PROXY_MAX_RETRIES:
                        continue

                    return JSONResponse(status_code=502, content={
                        "type": "error", "error": {
                            "type": "api_error",
                            "message": f"Chutes routing failed ({resp.status_code}): {last_error_msg[:300]}",
                        },
                    })
                oai_data = resp.json()
                actual_model = oai_data.get("model", "?")
                u = oai_data.get("usage", {})
                if u:
                    with _token_lock:
                        _token_usage["input"] += u.get("prompt_tokens", 0)
                        _token_usage["output"] += u.get("completion_tokens", 0)
                _log(f"proxy: response [{routing}] via {routing_label} model={actual_model}")
                return JSONResponse(content=_openai_response_to_anthropic(oai_data, model))
            except httpx.TimeoutException:
                last_error_msg = f"timed out after {PROXY_TIMEOUT}s"
                _log(f"proxy: {last_error_msg} (attempt {attempt}/{PROXY_MAX_RETRIES})")
                if attempt < PROXY_MAX_RETRIES:
                    continue
                return JSONResponse(status_code=502, content={
                    "type": "error", "error": {
                        "type": "api_error",
                        "message": f"Chutes routing {last_error_msg}",
                    },
                })
            except Exception as exc:
                last_error_msg = str(exc)[:300]
                _log(f"proxy: error (attempt {attempt}/{PROXY_MAX_RETRIES}): {last_error_msg}")
                if attempt < PROXY_MAX_RETRIES:
                    continue
                return JSONResponse(status_code=502, content={
                    "type": "error", "error": {
                        "type": "api_error",
                        "message": f"Chutes routing error: {last_error_msg}",
                    },
                })


@_proxy_app.post("/v1/messages/count_tokens")
async def _proxy_count_tokens(request: Request):
    body = await request.json()
    rough = sum(len(json.dumps(m)) for m in body.get("messages", [])) // 4
    rough += len(json.dumps(body.get("tools", []))) // 4
    rough += len(str(body.get("system", ""))) // 4
    return JSONResponse(content={"input_tokens": max(rough, 1)})


def _start_proxy():
    """Run the Chutes translation proxy in-process on a background thread."""
    config = uvicorn.Config(
        _proxy_app, host="127.0.0.1", port=PROXY_PORT, log_level="warning",
    )
    server = uvicorn.Server(config)
    server.run()


# ── Agent runner ─────────────────────────────────────────────────────────────

def _claude_cmd(prompt: str, extra_flags: list[str] | None = None) -> list[str]:
    cmd = ["claude", "-p", prompt]
    if not IS_ROOT:
        cmd.append("--dangerously-skip-permissions")
    cmd.extend(["--output-format", "stream-json", "--verbose"])
    if extra_flags:
        cmd.extend(extra_flags)
    return cmd


def _write_claude_settings():
    """Point Claude Code at the active provider (OpenRouter direct or Chutes proxy)."""
    settings_dir = WORKING_DIR / ".claude"
    settings_dir.mkdir(exist_ok=True)

    if PROVIDER == "openrouter":
        env_block = {
            "ANTHROPIC_API_KEY": LLM_API_KEY,
            "ANTHROPIC_BASE_URL": LLM_BASE_URL,
            "ANTHROPIC_AUTH_TOKEN": "",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
        }
        target_label = LLM_BASE_URL
    else:
        proxy_url = f"http://127.0.0.1:{PROXY_PORT}"
        env_block = {
            "ANTHROPIC_API_KEY": "chutes-proxy",
            "ANTHROPIC_BASE_URL": proxy_url,
            "ANTHROPIC_AUTH_TOKEN": "",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
        }
        target_label = proxy_url

    settings = {
        "model": CLAUDE_MODEL,
        "permissions": {
            "allow": [
                "Bash(*)", "Read(*)", "Write(*)", "Edit(*)",
                "Glob(*)", "Grep(*)", "WebFetch(*)", "WebSearch(*)",
                "TodoWrite(*)", "NotebookEdit(*)", "Task(*)",
            ],
        },
        "env": env_block,
    }
    (settings_dir / "settings.local.json").write_text(json.dumps(settings, indent=2))
    _log(f"wrote .claude/settings.local.json (provider={PROVIDER}, model={CLAUDE_MODEL}, target={target_label})")


def _parse_env_file(path: Path) -> dict[str, str]:
    """Parse a KEY=VALUE env file, returning a dict. Skips comments and blank lines."""
    result = {}
    if not path.exists():
        return result
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        k, v = k.strip(), v.strip().strip("'\"")
        if k:
            result[k] = v
    return result


def _write_scoped_env(env_file: Path, name: str, value: str):
    """Set a single env var in a scoped .env file (channel or thread level)."""
    env_file.parent.mkdir(parents=True, exist_ok=True)
    existing = _parse_env_file(env_file)
    existing[name] = value
    lines = [f"{k}='{v}'" for k, v in existing.items()]
    env_file.write_text("\n".join(lines) + "\n")
    # Restrict permissions: owner read/write only
    env_file.chmod(0o600)


def _delete_scoped_env(env_file: Path, name: str) -> bool:
    """Remove a single env var from a scoped .env file. Returns True if found."""
    existing = _parse_env_file(env_file)
    if name not in existing:
        return False
    del existing[name]
    if existing:
        lines = [f"{k}='{v}'" for k, v in existing.items()]
        env_file.write_text("\n".join(lines) + "\n")
        env_file.chmod(0o600)
    elif env_file.exists():
        env_file.unlink()
    return True


def _load_scoped_env(channel_id: str, thread_id: str = "") -> dict[str, str]:
    """Load scoped env vars: channel-level first, then thread-level overrides."""
    scoped = {}
    if channel_id:
        scoped.update(_parse_env_file(_channel_env_file(channel_id)))
    return scoped


def _claude_env(channel_id: str = "", thread_id: str = "", silent: bool = False) -> dict[str, str]:
    env = os.environ.copy()
    env.pop("BOT_TOKEN", None)
    # Apply scoped env vars (channel → thread, thread overrides channel)
    env.update(_load_scoped_env(channel_id, thread_id))
    if channel_id:
        env["ARBOS_CHANNEL_ID"] = channel_id
    if thread_id:
        env["ARBOS_THREAD_ID"] = thread_id
    if silent:
        env["ARBOS_SILENT"] = "1"
    if PROVIDER == "openrouter":
        env["ANTHROPIC_API_KEY"] = LLM_API_KEY
        env["ANTHROPIC_BASE_URL"] = LLM_BASE_URL
        env["ANTHROPIC_AUTH_TOKEN"] = ""
    else:
        env["ANTHROPIC_API_KEY"] = "chutes-proxy"
        env["ANTHROPIC_BASE_URL"] = f"http://127.0.0.1:{PROXY_PORT}"
        env["ANTHROPIC_AUTH_TOKEN"] = ""
    return env


def _run_claude_once(cmd, env, on_text=None, on_activity=None, channel_id: str = "", thread_id: str = "", stop_event: threading.Event | None = None):
    """Run a single claude subprocess, return (returncode, result_text, raw_lines, stderr).

    on_text: optional callback(accumulated_text) fired as assistant text streams in.
    on_activity: optional callback(status_str) fired on tool use and other activity.
    Kills the process if no output is received for CLAUDE_TIMEOUT seconds.
    If stop_event is set (e.g. thread paused), kills the process immediately.
    """
    proc = subprocess.Popen(
        cmd, cwd=WORKING_DIR, env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, bufsize=1,
    )
    with _child_procs_lock:
        _child_procs.add(proc)
    if channel_id:
        with _channel_procs_lock:
            _channel_procs[channel_id] = proc

    result_text = ""
    complete_texts: list[str] = []
    streaming_tokens: list[str] = []
    raw_lines: list[str] = []
    timed_out = False
    last_activity = time.monotonic()

    sel = selectors.DefaultSelector()
    sel.register(proc.stdout, selectors.EVENT_READ)

    stopped = False
    try:
        while True:
            if stop_event and stop_event.is_set():
                _log(f"channel stop_event set, killing pid={proc.pid}")
                proc.kill()
                stopped = True
                break
            ready = sel.select(timeout=min(CLAUDE_TIMEOUT, 30))
            if not ready:
                if stop_event and stop_event.is_set():
                    _log(f"channel stop_event set, killing pid={proc.pid}")
                    proc.kill()
                    stopped = True
                    break
                if time.monotonic() - last_activity > CLAUDE_TIMEOUT:
                    _log(f"claude timeout: no output for {CLAUDE_TIMEOUT}s, killing pid={proc.pid}")
                    proc.kill()
                    timed_out = True
                    break
                if proc.poll() is not None:
                    break
                continue
            line = proc.stdout.readline()
            if not line:
                break
            last_activity = time.monotonic()
            raw_lines.append(line)
            try:
                evt = json.loads(line)
            except json.JSONDecodeError:
                continue
            etype = evt.get("type", "")
            if etype == "assistant":
                msg = evt.get("message", {})
                for block in msg.get("content", []):
                    btype = block.get("type", "")
                    if btype == "text" and block.get("text"):
                        if evt.get("model_call_id"):
                            complete_texts.append(block["text"])
                            streaming_tokens.clear()
                        else:
                            streaming_tokens.append(block["text"])
                            if on_text:
                                on_text("".join(streaming_tokens))
                    elif btype == "tool_use" and on_activity:
                        tool_name = block.get("name", "")
                        tool_input = block.get("input", {})
                        on_activity(_format_tool_activity(tool_name, tool_input))
                if PROVIDER == "openrouter":
                    u = msg.get("usage", {})
                    if u:
                        with _token_lock:
                            _token_usage["input"] += u.get("input_tokens", 0)
                            _token_usage["output"] += u.get("output_tokens", 0)
            elif etype == "item.completed":
                item = evt.get("item", {})
                if item.get("type") == "agent_message" and item.get("text"):
                    complete_texts.append(item["text"])
                    streaming_tokens.clear()
                    if on_text:
                        on_text(item["text"])
            elif etype == "result":
                result_text = evt.get("result", "")
                if PROVIDER == "openrouter":
                    u = evt.get("usage", {})
                    if u:
                        with _token_lock:
                            _token_usage["input"] += u.get("input_tokens", 0)
                            _token_usage["output"] += u.get("output_tokens", 0)
    finally:
        sel.unregister(proc.stdout)
        sel.close()

    if not result_text:
        if complete_texts:
            result_text = complete_texts[-1]
        elif streaming_tokens:
            result_text = "".join(streaming_tokens)

    if stopped:
        stderr_output = "(stopped — channel paused)"
    elif timed_out:
        stderr_output = "(timed out)"
    else:
        stderr_output = proc.stderr.read() if proc.stderr else ""

    returncode = proc.wait()
    with _child_procs_lock:
        _child_procs.discard(proc)
    if channel_id:
        with _channel_procs_lock:
            if _channel_procs.get(channel_id) is proc:
                _channel_procs.pop(channel_id, None)
    return returncode, result_text, raw_lines, stderr_output


def run_agent(cmd: list[str], phase: str, output_file: Path,
              on_text=None, on_activity=None, channel_id: str = "", thread_id: str = "", silent: bool = False, stop_event: threading.Event | None = None) -> subprocess.CompletedProcess:
    _claude_semaphore.acquire()
    try:
        env = _claude_env(channel_id=channel_id, thread_id=thread_id, silent=silent)
        flags = " ".join(a for a in cmd if a.startswith("-"))

        returncode, result_text, raw_lines, stderr_output = 1, "", [], "no attempts made"

        for attempt in range(1, MAX_RETRIES + 1):
            _log(f"{phase}: starting (attempt={attempt}) flags=[{flags}]")
            t0 = time.monotonic()

            returncode, result_text, raw_lines, stderr_output = _run_claude_once(
                cmd, env, on_text=on_text, on_activity=on_activity, channel_id=channel_id,
                thread_id=thread_id, stop_event=stop_event,
            )
            elapsed = time.monotonic() - t0

            output_file.write_text(_redact_secrets("".join(raw_lines)))
            _log(f"{phase}: finished rc={returncode} {fmt_duration(elapsed)}")

            if stop_event and stop_event.is_set():
                _log(f"{phase}: stop_event set, aborting retries")
                return subprocess.CompletedProcess(
                    args=cmd, returncode=returncode,
                    stdout=result_text, stderr="(stopped — channel paused)",
                )

            if returncode != 0 and stderr_output.strip():
                _log(f"{phase}: stderr {stderr_output.strip()[:300]}")
                if attempt < MAX_RETRIES:
                    delay = min(2 ** attempt, 30)
                    _log(f"{phase}: retrying in {delay}s (attempt {attempt}/{MAX_RETRIES})")
                    if stop_event:
                        stop_event.wait(timeout=delay)
                        if stop_event.is_set():
                            _log(f"{phase}: stop_event set during retry wait, aborting")
                            return subprocess.CompletedProcess(
                                args=cmd, returncode=returncode,
                                stdout=result_text, stderr="(stopped — channel paused)",
                            )
                    else:
                        time.sleep(delay)
                    continue

            return subprocess.CompletedProcess(
                args=cmd, returncode=returncode,
                stdout=result_text, stderr=stderr_output,
            )

        _log(f"{phase}: all {MAX_RETRIES} retries exhausted")
        output_file.write_text(_redact_secrets("".join(raw_lines)))
        return subprocess.CompletedProcess(
            args=cmd, returncode=returncode,
            stdout=result_text, stderr=stderr_output,
        )
    finally:
        _claude_semaphore.release()


def extract_text(result: subprocess.CompletedProcess) -> str:
    output = result.stdout or ""
    if not output.strip():
        output = result.stderr or "(no output)"
    return output


def run_step(prompt: str, step_number: int, channel_id: str = "", channel_step: int = 0, thread_id: str = "", silent: bool = False) -> tuple[bool, str]:
    run_dir = make_run_dir(channel_id=channel_id, thread_id=thread_id)
    t0 = time.monotonic()

    log_file = run_dir / "logs.txt"
    _tls.log_fh = open(log_file, "a", encoding="utf-8")

    smf = _step_msg_file(channel_id) if channel_id else CONTEXT_DIR / ".step_msg"

    target_channel = thread_id if thread_id else channel_id
    target = _step_update_target(target_channel) if not silent else None
    cs = _channels.get(channel_id) if channel_id else None
    ch_name = cs.name if cs else ""
    step_label = f"{ch_name} Step {channel_step}" if channel_id else f"Step {step_number}"
    step_msg_id: str | None = None
    step_msg_text = ""
    last_edit = 0.0
    result_text = ""

    if silent:
        smf.unlink(missing_ok=True)
    elif target:
        smf.parent.mkdir(parents=True, exist_ok=True)
        smf.write_text(json.dumps({"msg_id": None, "text": ""}))
    else:
        smf.unlink(missing_ok=True)

    def _edit_step_msg(text: str, *, force: bool = False):
        nonlocal last_edit, step_msg_text, step_msg_id
        if not target:
            return
        if not step_msg_id:
            if smf.exists():
                try:
                    state = json.loads(smf.read_text())
                    step_msg_id = state.get("msg_id")
                except (json.JSONDecodeError, KeyError):
                    pass
            if not step_msg_id:
                return
        now = time.time()
        if not force and now - last_edit < 3.0:
            return
        step_msg_text = text
        _edit_discord_text(step_msg_id, text, target=target)
        smf.write_text(json.dumps({"msg_id": step_msg_id, "text": text}))
        last_edit = now

    _reset_tokens()

    _last_activity = [""]
    _heartbeat_stop = threading.Event()

    def _on_activity(status: str):
        _last_activity[0] = status
        if silent:
            return
        elapsed_s = time.monotonic() - t0
        inp, out = _get_tokens()
        tok = f" | {fmt_tokens(inp, out, elapsed_s)}" if (inp or out) else ""
        _edit_step_msg(f"{step_label} ({fmt_duration(elapsed_s)}{tok})\n{status}")

    def _heartbeat():
        while not _heartbeat_stop.wait(timeout=10):
            elapsed_s = time.monotonic() - t0
            inp, out = _get_tokens()
            tok = f" | {fmt_tokens(inp, out, elapsed_s)}" if (inp or out) else ""
            status = _last_activity[0] or "working..."
            _edit_step_msg(f"{step_label} ({fmt_duration(elapsed_s)}{tok})\n{status}", force=True)

    success = False
    try:
        _log(f"run dir {run_dir}")

        preview = prompt[:200] + ("…" if len(prompt) > 200 else "")
        _log(f"prompt preview: {preview}")

        _log(f"channel {ch_name or 'general'} step {channel_step}: executing")

        if not silent:
            threading.Thread(target=_heartbeat, daemon=True).start()

        channel_stop = cs.stop_event if cs else None

        result = run_agent(
            _claude_cmd(prompt),
            phase=f"ch:{ch_name or 'general'}",
            output_file=run_dir / "output.txt",
            on_activity=_on_activity,
            channel_id=channel_id,
            thread_id=thread_id,
            silent=silent,
            stop_event=channel_stop,
        )

        rollout_text = _redact_secrets(extract_text(result))
        (run_dir / "rollout.md").write_text(rollout_text)
        _log(f"rollout saved ({len(rollout_text)} chars)")
        result_text = rollout_text

        elapsed = time.monotonic() - t0
        success = result.returncode == 0
        _log(f"step {'succeeded' if success else 'failed'} in {fmt_duration(elapsed)}")
        return success, result_text
    finally:
        _heartbeat_stop.set()
        fh = getattr(_tls, "log_fh", None)
        if fh:
            fh.close()
            _tls.log_fh = None
        if silent:
            smf.unlink(missing_ok=True)
        else:
            try:
                agent_text = ""
                if smf.exists():
                    try:
                        state = json.loads(smf.read_text())
                        agent_text = state.get("text", "").strip()
                    except (json.JSONDecodeError, KeyError):
                        pass

                if step_msg_id and agent_text:
                    log_chat("bot", agent_text[:1000], channel_id=channel_id)

                smf.unlink(missing_ok=True)
            except Exception as exc:
                _log(f"step message finalize failed: {str(exc)[:120]}")


# ── Agent loop ───────────────────────────────────────────────────────────────


def _make_step_summary(rollout_text: str, step_number: int) -> str:
    """Extract a concise summary from rollout text for posting after a silent step."""
    lines = rollout_text.strip().splitlines() if rollout_text else []
    if not lines:
        return f"**Step {step_number}** completed (no output)."
    tail = "\n".join(lines[-60:])
    limit = DISCORD_MSG_LIMIT - 100
    if len(tail) > limit:
        tail = tail[-limit:]
    return f"**Step {step_number}**\n{tail}"


def _channel_loop(channel_id: str):
    """Run the autonomous loop for a channel. Executes steps toward the channel goal."""
    global _step_count

    cs = _channels.get(channel_id)
    if not cs:
        return

    failures = 0
    _log(f"channel {cs.name}: loop started")

    while not cs.stop_event.is_set():
        if not cs.running or not cs.goal:
            cs.wake.wait(timeout=5)
            cs.wake.clear()
            continue

        _step_count += 1
        cs.step_count += 1
        cs.last_run = datetime.now().isoformat()
        with _channels_lock:
            _save_channels()

        _log(f"{cs.name} Step {cs.step_count} (global step {_step_count})", blank=True)

        target_cid = cs.active_thread_id or channel_id
        token = os.getenv("BOT_TOKEN", "")
        if token:
            _send_discord_text(f"Step #{cs.step_count}", target=(token, target_cid))

        prompt = _build_step_prompt(channel_id, step=cs.step_count)
        if not prompt:
            cs.wake.wait(timeout=5)
            cs.wake.clear()
            continue

        _log(f"channel {cs.name}: prompt={len(prompt)} chars")

        _reset_tokens()
        pm2_before = _pm2_list_names()
        success, result_text = run_step(prompt, _step_count, channel_id=channel_id,
                                        channel_step=cs.step_count,
                                        thread_id=cs.active_thread_id,
                                        silent=True)
        step_in, step_out = _get_tokens()

        if cs.stop_event.is_set() or not cs.running:
            cs.total_input_tokens += step_in
            cs.total_output_tokens += step_out
            _log(f"channel {cs.name}: step killed (stopped)")
            break

        pm2_after = _pm2_list_names()
        new_pm2 = pm2_after - pm2_before
        if new_pm2:
            cs.pm2_procs = list(set(cs.pm2_procs) | new_pm2)
            _log(f"channel {cs.name}: tracked new pm2 processes: {new_pm2}")

        cs.total_input_tokens += step_in
        cs.total_output_tokens += step_out
        cs.last_finished = datetime.now().isoformat()
        with _channels_lock:
            _save_channels()

        _push_channel_context(channel_id, step_label=f"step {cs.step_count}")

        summary = _make_step_summary(result_text, cs.step_count)
        if token:
            _send_discord_text(summary, target=(token, target_cid))

        if success:
            failures = 0
        else:
            failures += 1
            cs.failures += 1
            _log(f"channel {cs.name}: failure #{failures}")

        cs.wake.clear()

        step_delay = cs.delay + int(os.environ.get("AGENT_DELAY", "0"))
        if failures:
            backoff = min(2 ** failures, 120)
            step_delay += backoff
            _log(f"channel {cs.name}: waiting {step_delay}s (failure backoff + delay)")
            cs.wake.wait(timeout=step_delay)
        elif step_delay > 0:
            _log(f"channel {cs.name}: waiting {step_delay}s (delay)")
            cs.wake.wait(timeout=step_delay)

    _log(f"channel {cs.name} loop exited")


def _channel_manager():
    """Monitor _channels and spawn/stop channel loops as needed."""
    while not _shutdown.is_set():
        with _channels_lock:
            for cid, cs in list(_channels.items()):
                if cs.running and cs.goal and cs.loop_thread is None:
                    cs.stop_event.clear()
                    t = threading.Thread(target=_channel_loop, args=(cid,), daemon=True, name=f"ch-{cs.name}")
                    cs.loop_thread = t
                    t.start()
                    _log(f"channel {cs.name} loop spawned")
                if cs.loop_thread is not None and not cs.loop_thread.is_alive():
                    cs.loop_thread = None
        _shutdown.wait(timeout=2)


def transcribe_voice(file_path: str, fmt: str = "ogg") -> str:
    """Transcribe audio via Chutes Whisper Large V3 STT endpoint."""
    try:
        with open(file_path, "rb") as f:
            b64_audio = base64.b64encode(f.read()).decode("utf-8")

        resp = requests.post(
            "https://chutes-whisper-large-v3.chutes.ai/transcribe",
            headers={
                "Authorization": f"Bearer {CHUTES_API_KEY}",
                "Content-Type": "application/json",
            },
            json={"language": None, "audio_b64": b64_audio},
            timeout=90,
        )
        if resp.status_code == 200:
            data = resp.json()
            text = data.get("text", "") if isinstance(data, dict) else str(data)
            if text.strip():
                _log(f"whisper transcription ok ({len(text)} chars)")
                return text.strip()
            return "(voice transcription returned empty — send text instead)"
        _log(f"whisper STT failed: status={resp.status_code} body={resp.text[:200]}")
        return "(voice transcription unavailable — send text instead)"
    except Exception as exc:
        _log(f"transcription failed: {str(exc)[:200]}")
        return "(voice transcription unavailable — send text instead)"


# ── Discord bot ──────────────────────────────────────────────────────────────

def _recent_context(max_chars: int = 6000) -> str:
    """Collect recent rollouts across all channels."""
    parts: list[str] = []
    total = 0
    all_runs: list[tuple[str, Path]] = []
    for cid, cs in sorted(_channels.items()):
        runs_dir = _channel_runs_dir(cid)
        if not runs_dir.exists():
            continue
        for d in runs_dir.iterdir():
            if d.is_dir():
                all_runs.append((f"{cs.name}/{d.name}", d))
    all_runs.sort(key=lambda x: x[1].name, reverse=True)
    for label, run_dir in all_runs:
        f = run_dir / "rollout.md"
        if f.exists():
            content = f.read_text()[:2000]
            hdr = f"\n--- rollout.md ({label}) ---\n"
            if total + len(hdr) + len(content) > max_chars:
                return "".join(parts)
            parts.append(hdr + content)
            total += len(hdr) + len(content)
    return "".join(parts)


def _build_operator_prompt(user_text: str) -> str:
    """Build prompt for the CLI agent to handle any operator request."""
    chatlog = load_chatlog(max_chars=4000)

    parts = [
        "You are the operator interface for Arbos, a coding agent running in a loop via pm2.\n"
        "The operator communicates with you through Discord. Be concise and direct.\n"
        "When the operator asks you to do something, do it by modifying the relevant files.\n"
        "When the operator asks a question, answer from the available context.\n\n"
        "## Security\n\n"
        "NEVER read, output, or reveal the contents of `.env`, `.env.enc`, or any secret/key/token values.\n"
        "Do not include API keys, passwords, seed phrases, or credentials in any response.\n"
        "If asked to show secrets, refuse. The .env file is encrypted; do not attempt to decrypt it.\n\n"
        "## Context structure\n\n"
        "Context is organized by Discord channels:\n"
        "```\n"
        "context/\n"
        "  general/chat/        — #general channel chat logs\n"
        "  <channel-name>/      — one directory per managed channel\n"
        "    goal                — channel objective\n"
        "    pin                 — pinned messages (always-on context)\n"
        "    state               — working memory\n"
        "    chat/               — channel chat logs\n"
        "    runs/               — per-step artifacts\n"
        "  channels.json        — channel metadata\n"
        "```\n"
        "Each channel is a work unit with at most one autonomous loop.\n"
        "Use `/goal <text>` to set a goal, `/start` to begin the loop, `/stop` to halt.\n"
        "Channels in the same Discord category share context.\n"
        "Deleting a channel cleans up its context and processes.\n\n"
        "## Available operations\n\n"
        "- **Update a channel's state**: write to `context/<channel-dir>/state`.\n"
        "- **Set system prompt**: write to `PROMPT.md`.\n"
        "- **Set env variable**: write `KEY='VALUE'` lines (one per line) to `context/.env.pending`. They are picked up automatically and persisted.\n"
        "- **View logs**: read files in `context/<channel-dir>/runs/<timestamp>/` (rollout.md, logs.txt).\n"
        "- **Modify code & restart**: edit code files, then run `touch .restart`.\n"
        "- **Respond to operator**: Your text output is streamed directly to Discord. Just write your response — do NOT use `arbos.py send` (it is suppressed in chat mode).\n"
        "- **Send file to operator**: run `python arbos.py sendfile path/to/file [--caption 'text'] [--photo]`.\n"
        "- **Received files**: operator-sent files are saved in `context/files/` and their path is shown in the message.",
    ]

    if _channels:
        by_cat: dict[str, list[tuple[str, ChannelState]]] = {}
        uncategorized: list[tuple[str, ChannelState]] = []
        for cid, cs in sorted(_channels.items(), key=lambda x: x[1].name):
            if cs.category_id:
                by_cat.setdefault(cs.category_id, []).append((cid, cs))
            else:
                uncategorized.append((cid, cs))

        def _fmt_channel(cid, cs):
            icon = "\U0001f7e9" if cs.running and cs.goal else "\u2b1c"
            pf = _pin_file(cid)
            pin_text = pf.read_text().strip()[:200] if pf.exists() else "(no pin)"
            sf = _state_file(cid)
            state_text = sf.read_text().strip()[:200] if sf.exists() else "(empty)"
            goal_text = cs.goal[:200] if cs.goal else "(no goal)"
            ch_line = (
                f"### {icon} {cs.name} step {cs.step_count} | dir=`context/{cs.dir_name}/` | delay: {cs.delay}s\n"
                f"Goal: {goal_text}\nPin: {pin_text}\nState: {state_text}"
            )
            return ch_line

        channels_section = []
        for cat_id, members in sorted(by_cat.items(), key=lambda x: _categories.get(x[0], x[0])):
            cat_name = _categories.get(cat_id, cat_id)
            channels_section.append(f"**Category: {cat_name}**")
            for cid, cs in members:
                channels_section.append(_fmt_channel(cid, cs))
        if uncategorized:
            if by_cat:
                channels_section.append("**Uncategorized (global scope)**")
            for cid, cs in uncategorized:
                channels_section.append(_fmt_channel(cid, cs))
        parts.append("## Channels\n" + "\n\n".join(channels_section))
    else:
        parts.append("## Channels\n(no managed channels)")

    if chatlog:
        parts.append(chatlog)

    context = _recent_context(max_chars=4000)
    if context:
        parts.append(f"## Recent activity\n{context}")
    parts.append(f"## Operator message\n{user_text}")

    return "\n\n".join(parts)


_TOOL_LABELS = {
    "Bash": "running",
    "Read": "reading",
    "Write": "writing",
    "Edit": "editing",
    "Glob": "searching",
    "Grep": "locating",
    "WebFetch": "downloading",
    "WebSearch": "browsing",
    "TodoWrite": "planning",
    "Task": "executing",
}


def _format_tool_activity(tool_name: str, tool_input: dict) -> str:
    label = _TOOL_LABELS.get(tool_name, tool_name)
    detail = ""
    if tool_name == "Bash":
        detail = (tool_input.get("command") or "")[:80]
    elif tool_name in ("Read", "Write", "Edit"):
        detail = (tool_input.get("file_path") or tool_input.get("path") or "")
        if detail:
            detail = detail.rsplit("/", 1)[-1]
    elif tool_name == "Glob":
        detail = (tool_input.get("pattern") or tool_input.get("glob") or "")[:60]
    elif tool_name == "Grep":
        detail = (tool_input.get("pattern") or tool_input.get("regex") or "")[:60]
    elif tool_name == "WebFetch":
        detail = (tool_input.get("url") or "")[:60]
    elif tool_name == "WebSearch":
        detail = (tool_input.get("query") or tool_input.get("search_term") or "")[:60]
    elif tool_name == "Task":
        detail = (tool_input.get("description") or "")[:60]
    if detail:
        return f"{label}: {detail}"
    return f"{label}..."


def run_agent_streaming(prompt: str, channel_id: str, *,
                        reply_to: str | None = None) -> str:
    """Run Claude Code CLI and stream output into a Discord message."""
    if PROVIDER == "openrouter":
        cmd = _claude_cmd(prompt)
    else:
        cmd = _claude_cmd(prompt, extra_flags=["--model", "bot"])

    token = os.getenv("BOT_TOKEN", "")
    target = (token, channel_id)
    # React with 🤔 (thinking) when Claude starts processing
    if reply_to:
        _add_discord_reaction(channel_id, reply_to, "\U0001f914")
    msg_id = _send_discord_new("thinking...", target=target, reply_to=reply_to)
    current_text = ""
    activity_status = ""
    last_edit = 0.0

    def _edit(text: str, force: bool = False):
        nonlocal last_edit
        if not msg_id:
            return
        now = time.time()
        if not force and now - last_edit < 1.5:
            return
        display = text[-(DISCORD_MSG_LIMIT - 50):] if len(text) > DISCORD_MSG_LIMIT - 50 else text
        display = _redact_secrets(display)
        if not display.strip():
            return
        _edit_discord_text(msg_id, display, target=target)
        last_edit = now

    def _on_text(text: str):
        nonlocal current_text
        current_text = text
        _edit(text)

    def _on_activity(status: str):
        nonlocal activity_status
        activity_status = status
        if not current_text:
            _edit(status)

    _claude_semaphore.acquire()
    try:
        env = _claude_env(channel_id=channel_id)
        # Suppress `arbos.py send` calls — the agent's text output is already
        # being streamed into the Discord message by run_agent_streaming.
        env["ARBOS_SILENT"] = "1"

        for attempt in range(1, MAX_RETRIES + 1):
            current_text = ""
            activity_status = ""
            last_edit = 0.0

            returncode, result_text, raw_lines, stderr_output = _run_claude_once(
                cmd, env, on_text=_on_text, on_activity=_on_activity,
            )

            if result_text.strip():
                current_text = result_text
                break

            if returncode != 0 and attempt < MAX_RETRIES:
                delay = min(2 ** attempt, 30)
                _edit(f"Error, retrying in {delay}s... (attempt {attempt}/{MAX_RETRIES})", force=True)
                time.sleep(delay)
                continue
            break

        _edit(current_text, force=True)

        if not current_text.strip() and msg_id:
            _edit_discord_text(msg_id, "(no output)", target=target)

        # React with 🏁 on the original trigger message — response complete
        if reply_to:
            _add_discord_reaction(channel_id, reply_to, "\U0001F3C1")

    except Exception as e:
        if msg_id:
            _edit_discord_text(msg_id, f"Error: {str(e)[:300]}", target=target)
    finally:
        _claude_semaphore.release()

    return current_text


def _is_owner(user_id: int) -> bool:
    owner = os.environ.get("DISCORD_OWNER_ID", "").strip()
    if not owner:
        return False
    return str(user_id) == owner


def _enroll_owner(user_id: int):
    """Auto-enroll the first /start user as the owner and persist."""
    owner_id = str(user_id)
    os.environ["DISCORD_OWNER_ID"] = owner_id
    env_path = WORKING_DIR / ".env"
    if env_path.exists():
        existing = env_path.read_text()
        if "DISCORD_OWNER_ID" not in existing:
            with open(env_path, "a") as f:
                f.write(f"\nDISCORD_OWNER_ID='{owner_id}'\n")
    elif ENV_ENC_FILE.exists():
        _save_to_encrypted_env("DISCORD_OWNER_ID", owner_id)
    _log(f"enrolled owner: {owner_id}")


def run_bot():
    """Run the Discord bot with channel-based architecture."""
    token = os.getenv("BOT_TOKEN")
    if not token:
        _log("BOT_TOKEN not set; add it to .env and restart")
        sys.exit(1)

    import discord

    intents = discord.Intents.default()
    intents.message_content = True
    intents.messages = True
    intents.guilds = True
    client = discord.Client(intents=intents)

    _general_channel: dict[str, str | None] = {"id": None}

    def _save_general_id(channel_id: str):
        CHANNEL_ID_FILE.write_text(channel_id)
        _general_channel["id"] = channel_id

    def _reply_sync(channel_id: str, text: str):
        """Send a message via REST API (sync, for use from any thread)."""
        text = _redact_secrets(text)[:DISCORD_MSG_LIMIT]
        try:
            requests.post(
                f"{DISCORD_API}/channels/{channel_id}/messages",
                headers=_discord_headers(token),
                json={"content": text},
                timeout=15,
            )
        except Exception as exc:
            _log(f"discord reply failed: {str(exc)[:120]}")

    def _handle_command(content: str, channel_id: str, author_id: int, msg_id: str = ""):
        """Parse and dispatch a /command in #general."""
        if not os.environ.get("DISCORD_OWNER_ID", "").strip():
            _enroll_owner(author_id)
        if not _is_owner(author_id):
            _reply_sync(channel_id, "Unauthorized.")
            return

        parts = content.split()
        cmd = parts[0].lower()

        if cmd == "/help":
            _reply_sync(channel_id, (
                "**Arbos — Autonomous Coding Agent**\n\n"
                "Arbos runs via pm2. Each Discord channel is an agent workspace with chat and threads.\n\n"
                "**How it works**\n"
                "• Create a channel under **Running** → agent workspace starts\n"
                "• **Pin a message** → adds always-on context (pin) to every prompt\n"
                "• Set a goal with `/goal <text>`, start the loop with `/start`\n"
                "• Each channel runs one autonomous loop toward its goal\n"
                "• Send a message in the channel → chatbot responds\n"
                "• Use `/stop` or `/pause` to halt the loop\n"
                "• Messages in **#general** go to an operator agent\n\n"
                "**Commands**\n"
                "In #general: `/status` `/update` `/restart` `/bash <cmd>` `/env <name> <value>` `/help`\n"
                "In a channel: `/goal <text>` `/start` `/stop` `/pause` `/delay <min>` `/status` `/restart` `/help`"
            ))

        elif cmd == "/status":
            if not _channels:
                _reply_sync(channel_id, "No managed channels. Create a channel under the 'Running' or 'Paused' category.")
                return
            lines = [f"**{len(_channels)} channel(s)** — total steps: {_step_count}"]
            for cid, cs in sorted(_channels.items(), key=lambda x: x[1].name):
                icon = "\U0001f7e9" if cs.running and cs.goal else "\u2b1c"
                delay_str = f" delay:{cs.delay // 60}m" if cs.delay else ""
                pin_str = " pin" if cs.pin_text else ""
                step_str = f" step:{cs.step_count}" if cs.step_count else ""
                last = _format_last_time(cs.last_finished)
                invoked = _format_last_time(cs.last_run)
                cat_str = f" [{cs.category_name}]" if cs.category_name else ""
                goal_preview = ""
                if cs.goal:
                    goal_preview = f"\n  goal: {cs.goal[:80]}{'...' if len(cs.goal) > 80 else ''}"
                lines.append(f"{icon} <#{cid}>{cat_str}{step_str}{delay_str}{pin_str} last:{last} invoked:{invoked}{goal_preview}")
            _reply_sync(channel_id, "\n".join(lines))

        elif cmd == "/restart":
            _reply_sync(channel_id, "Restarting — killing agent and exiting for pm2...")
            _log("restart requested via /restart command")
            _kill_child_procs()
            RESTART_FLAG.touch()

        elif cmd == "/bash":
            shell_cmd = content[len("/bash"):].strip()
            if not shell_cmd:
                _reply_sync(channel_id, "Usage: `/bash <command>`")
                return
            msg_id = _send_discord_new(f"```\n$ {shell_cmd}\n```\n⏳ Running...", target=(token, channel_id))
            try:
                r = subprocess.run(
                    shell_cmd, shell=True,
                    capture_output=True, text=True, timeout=120,
                    cwd=str(Path.home()),
                )
                out = r.stdout
                err = r.stderr
                parts_out = []
                if out.strip():
                    parts_out.append(out.strip())
                if err.strip():
                    parts_out.append(f"[stderr]\n{err.strip()}")
                body = "\n".join(parts_out) if parts_out else "(no output)"
                exit_line = f"\nexit code: {r.returncode}" if r.returncode != 0 else ""
                result = f"```\n$ {shell_cmd}\n{body}{exit_line}\n```"
                if len(result) > 1950:
                    result = f"```\n$ {shell_cmd}\n{body[:1800]}…\n(truncated){exit_line}\n```"
                if msg_id:
                    _edit_discord_text(msg_id, result, target=(token, channel_id))
                else:
                    _reply_sync(channel_id, result)
            except subprocess.TimeoutExpired:
                timeout_msg = f"```\n$ {shell_cmd}\n(timed out after 120s)\n```"
                if msg_id:
                    _edit_discord_text(msg_id, timeout_msg, target=(token, channel_id))
                else:
                    _reply_sync(channel_id, timeout_msg)
            except Exception as exc:
                err_msg = f"```\n$ {shell_cmd}\nError: {str(exc)[:1800]}\n```"
                if msg_id:
                    _edit_discord_text(msg_id, err_msg, target=(token, channel_id))
                else:
                    _reply_sync(channel_id, err_msg)

        elif cmd == "/update":
            msg_id = _send_discord_new("Pulling latest changes...", target=(token, channel_id))
            try:
                r = subprocess.run(
                    ["git", "pull", "--ff-only"],
                    cwd=WORKING_DIR, capture_output=True, text=True, timeout=30,
                )
                output = (r.stdout.strip() + "\n" + r.stderr.strip()).strip()
                if r.returncode != 0:
                    if msg_id:
                        _edit_discord_text(msg_id, f"Git pull failed:\n{output[:1900]}", target=(token, channel_id))
                    _log(f"update failed: {output[:200]}")
                    return
                if msg_id:
                    _edit_discord_text(msg_id, f"Pulled:\n{output[:1800]}\n\nRestarting...", target=(token, channel_id))
                _log(f"update pulled: {output[:200]}")
            except Exception as exc:
                if msg_id:
                    _edit_discord_text(msg_id, f"Git pull error: {str(exc)[:1900]}", target=(token, channel_id))
                _log(f"update error: {str(exc)[:200]}")
                return
            _kill_child_procs()
            RESTART_FLAG.touch()

        elif cmd == "/env":
            # /env NAME VALUE — store an env var securely
            if msg_id:
                _delete_discord_message(channel_id, msg_id)
            if len(parts) < 3:
                _reply_sync(channel_id, "Usage: `/env NAME VALUE`")
                return
            env_name = parts[1]
            env_value = " ".join(parts[2:])
            ENV_PENDING_FILE.write_text(f"{env_name}='{env_value}'\n")
            _process_pending_env()
            _reply_sync(channel_id, f"Set `{env_name}` and saved to encrypted env.")

        else:
            _reply_sync(channel_id, "Unknown command. Use /help to see available commands.")

    def _handle_text_message(content: str, channel_id: str, author_id: int,
                             msg_id: str | None = None):
        """Handle a plain text message from the operator."""
        if not _is_owner(author_id):
            if not os.environ.get("DISCORD_OWNER_ID", "").strip():
                _enroll_owner(author_id)
            else:
                _reply_sync(channel_id, "Unauthorized.")
                return
        if msg_id:
            _add_discord_reaction(channel_id, msg_id, "\u2705")  # ✅ accepted
        log_chat("user", content)
        prompt = _build_operator_prompt(content)
        response = run_agent_streaming(prompt, channel_id, reply_to=msg_id)
        log_chat("bot", response[:1000])
        _process_pending_env()

    def _handle_attachment_message(content: str, attachments: list, channel_id: str,
                                    author_id: int, msg_id: str | None = None):
        """Handle a message with file attachments."""
        if not _is_owner(author_id):
            _reply_sync(channel_id, "Unauthorized.")
            return
        if msg_id:
            _add_discord_reaction(channel_id, msg_id, "\u2705")  # ✅ accepted

        parts = []
        for att in attachments:
            filename = att["filename"]
            url = att["url"]
            size_bytes = att.get("size", 0)
            content_type = att.get("content_type", "")

            saved_path = _download_discord_attachment(url, filename)
            size_kb = size_bytes / 1024 if size_bytes else saved_path.stat().st_size / 1024

            att_text = f"[Sent file: {saved_path.name}] saved to {saved_path} ({size_kb:.1f} KB)"

            if content_type and content_type.startswith("audio/"):
                try:
                    ext = filename.rsplit(".", 1)[-1] if "." in filename else "ogg"
                    transcript = transcribe_voice(str(saved_path), fmt=ext)
                    att_text += f"\n[Audio transcription]: {transcript}"
                except Exception as exc:
                    _log(f"audio transcription failed: {str(exc)[:120]}")
            else:
                is_text = False
                try:
                    file_content = saved_path.read_text(errors="strict")
                    if len(file_content) <= 8000:
                        att_text += f"\n[File contents]:\n{file_content}"
                        is_text = True
                except (UnicodeDecodeError, ValueError):
                    pass
                if not is_text:
                    att_text += "\n(Binary file — not included inline. Read it from the saved path if needed.)"

            parts.append(att_text)

        user_text = "\n\n".join(parts)
        if content:
            user_text = f"{content}\n\n{user_text}"

        log_chat("user", user_text[:1000])
        prompt = _build_operator_prompt(user_text)
        response = run_agent_streaming(prompt, channel_id, reply_to=msg_id)
        log_chat("bot", response[:1000])
        _process_pending_env()

    def _handle_channel_command(content: str, channel_id: str, author_id: int, msg_id: str | None = None, reply_channel: str = ""):
        """Handle a /command in a managed channel.
        reply_channel: if set, send responses here instead of channel_id (for thread commands).
        """
        from_thread = bool(reply_channel)
        reply_to = reply_channel or channel_id
        if not _is_owner(author_id):
            _reply_sync(reply_to, "Unauthorized.")
            return
        parts = content.split()
        cmd = parts[0].lower()
        args = parts[1:]

        cs = _channels.get(channel_id)
        if not cs:
            _reply_sync(reply_to, "Channel not managed.")
            return

        if cmd == "/help":
            scope_label = f"category **{cs.category_name}**" if cs.category_id else "**global**"
            _reply_sync(reply_to, (
                f"**{cs.name}** (scope: {scope_label})\n\n"
                "Send messages and the agent responds as a chatbot.\n"
                "Each channel can run one autonomous loop working toward a goal.\n\n"
                "**Commands**\n"
                "`/goal` — show current goal\n"
                "`/goal <text>` — set the channel goal\n"
                "`/append <text>` — append to the current goal\n"
                "`/start` — start the autonomous loop\n"
                "`/stop` — stop the loop\n"
                "`/pause` — pause the loop\n"
                "`/delay <minutes>` — set delay between steps\n"
                "`/status` — show channel status\n"
                "`/restart` — kill current step and restart the loop\n"
                "`/env NAME VALUE` — set channel env var\n"
                "`/env -d NAME` — remove env var\n"
                "`/env` — list env vars\n"
                "`/help` — this message"
            ))

        elif cmd == "/goal":
            if not args:
                goal = cs.goal or "(no goal set)"
                _reply_sync(reply_to, f"**Goal:**\n{goal}")
            else:
                new_goal = " ".join(args)
                cs.goal = new_goal
                if from_thread:
                    cs.active_thread_id = reply_channel
                else:
                    cs.active_thread_id = ""
                gf = _goal_file(channel_id)
                gf.parent.mkdir(parents=True, exist_ok=True)
                gf.write_text(new_goal)
                with _channels_lock:
                    _save_channels()
                _reply_sync(reply_to, f"Goal set.\n{new_goal}")
                _log(f"channel {cs.name}: goal set ({len(new_goal)} chars)")

        elif cmd == "/append":
            if not args:
                _reply_sync(reply_to, "Usage: `/append <text to append to goal>`")
                return
            addition = " ".join(args)
            cs.goal = (cs.goal + "\n" + addition) if cs.goal else addition
            gf = _goal_file(channel_id)
            gf.parent.mkdir(parents=True, exist_ok=True)
            gf.write_text(cs.goal)
            with _channels_lock:
                _save_channels()
            _reply_sync(reply_to, f"Appended to goal.\n**Goal:**\n{cs.goal}")
            _log(f"channel {cs.name}: goal appended ({len(addition)} chars)")

        elif cmd == "/start":
            if cs.running:
                _reply_sync(reply_to, f"**{cs.name}** is already running.")
                return
            if not cs.goal:
                _reply_sync(reply_to, "No goal set. Use `/goal <text>` first.")
                return
            cs.running = True
            cs.started_at = datetime.now().isoformat()
            if from_thread:
                cs.active_thread_id = reply_channel
            cs.stop_event = threading.Event()
            cs.wake.set()
            with _channels_lock:
                _save_channels()
            _reply_sync(reply_to, f"**{cs.name}** started.")
            _sync_channel_name(channel_id)
            _log(f"channel {cs.name}: started via command")

        elif cmd in ("/stop", "/pause"):
            if not cs.running:
                _reply_sync(reply_to, f"**{cs.name}** is already stopped.")
                return
            cs.running = False
            cs.started_at = ""
            cs.active_thread_id = ""
            cs.stop_event.set()
            cs.wake.set()
            with _channel_procs_lock:
                proc = _channel_procs.get(channel_id)
            if proc and proc.poll() is None:
                _log(f"channel {cs.name}: killing claude subprocess pid={proc.pid}")
                proc.kill()
            with _channels_lock:
                _save_channels()
            label = "paused" if cmd == "/pause" else "stopped"
            _reply_sync(reply_to, f"**{cs.name}** {label}.")
            _sync_channel_name(channel_id)
            _log(f"channel {cs.name}: {label} via command")

        elif cmd == "/delay":
            if not args:
                delay_min = cs.delay / 60 if cs.delay else 0
                _reply_sync(reply_to, f"Current delay: {delay_min:.1f}m ({cs.delay}s)")
                return
            try:
                minutes = float(args[0])
            except ValueError:
                _reply_sync(reply_to, "Usage: `/delay <minutes>`")
                return
            if minutes < 0:
                _reply_sync(reply_to, "Delay must be >= 0.")
                return
            cs.delay = int(minutes * 60)
            with _channels_lock:
                _save_channels()
            _reply_sync(reply_to, f"Delay set to {minutes}m ({cs.delay}s).")
            _sync_channel_name(channel_id)

        elif cmd == "/status":
            _reply_sync(reply_to, _channel_status_text(cs))

        elif cmd == "/restart":
            _reply_sync(reply_to, f"Restarting **{cs.name}**...")
            cs.stop_event.set()
            cs.wake.set()
            with _channel_procs_lock:
                proc = _channel_procs.get(channel_id)
            if proc and proc.poll() is None:
                _log(f"channel {cs.name}: killing claude subprocess pid={proc.pid}")
                proc.kill()
            lt = cs.loop_thread
            if lt and lt.is_alive():
                lt.join(timeout=10)
            cs.stop_event = threading.Event()
            cs.wake = threading.Event()
            cs.loop_thread = None
            if cs.goal:
                cs.running = True
            with _channels_lock:
                _save_channels()
            _reply_sync(reply_to, f"**{cs.name}** restarted.")
            _sync_channel_name(channel_id)
            _log(f"channel {cs.name} restarted via command")

        elif cmd == "/env":
            if msg_id and len(args) >= 2 and args[0] != "-d":
                _delete_discord_message(reply_to, msg_id)
            if len(args) < 2:
                env_file = _channel_env_file(channel_id)
                current = _parse_env_file(env_file)
                if current:
                    lines = [f"`{k}` = (set)" for k in current]
                    _reply_sync(reply_to, "**Channel env vars:**\n" + "\n".join(lines))
                else:
                    _reply_sync(reply_to, "No env vars set.\nUsage: `/env NAME VALUE` or `/env -d NAME`")
                return
            env_name = args[0]
            if env_name == "-d" and len(args) >= 2:
                target = args[1]
                if _delete_scoped_env(_channel_env_file(channel_id), target):
                    _reply_sync(reply_to, f"Removed `{target}` from channel env.")
                else:
                    _reply_sync(reply_to, f"`{target}` not found in channel env.")
                return
            env_value = " ".join(args[1:])
            _write_scoped_env(_channel_env_file(channel_id), env_name, env_value)
            _reply_sync(reply_to, f"Set `{env_name}` in channel env.")
            _log(f"channel {cs.name}: set scoped env var {env_name}")

        else:
            _reply_sync(reply_to, "Unknown command. Use `/help` to see available commands.")

    def _handle_channel_message(content: str, attachments: list, channel_id: str,
                                message_id: str, author_id: int):
        """Handle a plain message in a managed channel (always chat mode)."""
        if not _is_owner(author_id):
            return

        ts_str = datetime.now().strftime("%Y-%m-%d %H:%M")
        parts = []
        if content:
            parts.append(f"[{ts_str}] {content}")
        for att in attachments:
            try:
                saved = _download_discord_attachment(att["url"], att["filename"])
                ct = att.get("content_type", "")
                if ct and ct.startswith("audio/"):
                    ext = att["filename"].rsplit(".", 1)[-1] if "." in att["filename"] else "ogg"
                    try:
                        transcript = transcribe_voice(str(saved), fmt=ext)
                        parts.append(f"[{ts_str}] [Audio transcription]: {transcript}")
                    except Exception as exc:
                        _log(f"audio transcription failed: {str(exc)[:120]}")
                        parts.append(f"[{ts_str}] [File: {saved}]")
                else:
                    att_text = f"[{ts_str}] [File: {saved}]"
                    try:
                        file_content = saved.read_text(errors="strict")
                        if len(file_content) <= 8000:
                            att_text += f"\n[File contents]:\n{file_content}"
                    except (UnicodeDecodeError, ValueError):
                        pass
                    parts.append(att_text)
            except Exception as exc:
                _log(f"failed to download attachment: {str(exc)[:120]}")

        entry = "\n".join(parts)
        if not entry.strip():
            return

        log_chat("user", content or entry, channel_id=channel_id)

        cs = _channels.get(channel_id)
        _add_discord_reaction(channel_id, message_id, "\u2705")  # ✅ accepted
        _log(f"channel {cs.name if cs else channel_id}: chatbot mode, responding directly")
        _reset_tokens()
        prompt = _build_channel_chat_prompt(channel_id, entry)
        response = run_agent_streaming(prompt, channel_id, reply_to=message_id)
        chat_in, chat_out = _get_tokens()
        if cs:
            cs.total_chat_input_tokens += chat_in
            cs.total_chat_output_tokens += chat_out
            with _channels_lock:
                _save_channels()
        log_chat("bot", response[:1000], channel_id=channel_id)

    @client.event
    async def on_ready():
        global _guild_id, _running_category_id, _paused_category_id
        _log(f"discord bot logged in as {client.user}")
        target_guild_id = os.environ.get("DISCORD_GUILD", "").strip()
        for guild in client.guilds:
            if target_guild_id and str(guild.id) != target_guild_id:
                continue
            _guild_id = str(guild.id)
            _running_category_id, _paused_category_id = _discover_categories(_guild_id)
            _log(f"guild: {guild.name} (id={_guild_id}), running_cat={_running_category_id}, paused_cat={_paused_category_id}")

            # Create any channels from pending_channels.json before discovery
            _create_pending_channels(_guild_id)

            for channel in guild.text_channels:
                if channel.name == "general":
                    _save_general_id(str(channel.id))
                    _log(f"found #general channel: {channel.id}")

            for channel in guild.text_channels:
                cid = str(channel.id)
                parent = str(channel.category_id) if channel.category_id else None
                if parent and parent in _categories:
                    is_running = (parent != _paused_category_id)
                    cat_name = _categories.get(parent, "")
                    if cid not in _channels:
                        _log(f"discovered channel: {channel.name} (id={cid}, running={is_running}, category={cat_name})")
                        def _setup(c=channel, r=is_running, ci=parent, cn=cat_name):
                            _setup_channel_context(str(c.id), c.name, r, category_id=ci, category_name=cn)
                        threading.Thread(target=_setup, daemon=True).start()
                    else:
                        cs = _channels[cid]
                        cs.category_id = parent
                        cs.category_name = cat_name
                        if parent in (_running_category_id, _paused_category_id):
                            new_running = (parent == _running_category_id)
                            if cs.running != new_running:
                                cs.running = new_running
                                if new_running:
                                    cs.wake.set()
                        with _channels_lock:
                            _save_channels()

            for cid, cs in list(_channels.items()):
                if cs.running:
                    chat_dir = _channel_chat_dir(cid)
                    if chat_dir.exists():
                        files = sorted(chat_dir.glob("*.jsonl"))
                        if files:
                            last_lines = files[-1].read_text().strip().splitlines()
                            if last_lines:
                                try:
                                    msg = json.loads(last_lines[-1])
                                    if msg.get("role") == "user":
                                        cs.wake.set()
                                        _log(f"channel {cs.name}: waking for unanswered messages")
                                except (json.JSONDecodeError, IndexError):
                                    pass

            if _general_channel["id"]:
                _send_discord_text("Restarted.", target=(token, _general_channel["id"]))
            return
        if target_guild_id:
            _log(f"WARNING: no guild found with id={target_guild_id}")
        else:
            _log("WARNING: no guild found")

    @client.event
    async def on_guild_channel_create(channel):
        if hasattr(channel, "type") and channel.type.value == 4:
            cat_id = str(channel.id)
            _categories[cat_id] = channel.name
            _log(f"new category created: {channel.name} (id={cat_id})")
            return
        if not hasattr(channel, "category_id") or channel.category_id is None:
            return
        parent = str(channel.category_id)
        if parent not in _categories:
            return
        cid = str(channel.id)
        if cid in _channels:
            return
        is_running = (parent != _paused_category_id)
        cat_name = _categories.get(parent, "")
        _log(f"new channel created: {channel.name} (id={cid}, running={is_running}, category={cat_name})")
        def _run():
            _setup_channel_context(cid, channel.name, is_running, category_id=parent, category_name=cat_name)
        threading.Thread(target=_run, daemon=True).start()

    @client.event
    async def on_guild_channel_update(before, after):
        if not hasattr(after, "category_id"):
            return
        before_parent = str(before.category_id) if before.category_id else None
        after_parent = str(after.category_id) if after.category_id else None
        if before_parent == after_parent:
            return
        cid = str(after.id)
        new_cat_name = _categories.get(after_parent, "") if after_parent else ""
        if after_parent and after_parent in _categories:
            is_running = (after_parent != _paused_category_id)
            if after_parent == _running_category_id:
                is_running = True
            elif after_parent == _paused_category_id:
                is_running = False
            if cid in _channels:
                cs = _channels[cid]
                old_scope = cs.category_name or "global"
                cs.category_id = after_parent
                cs.category_name = new_cat_name
                if after_parent in (_running_category_id, _paused_category_id):
                    cs.running = is_running
                if is_running:
                    cs.wake.set()
                with _channels_lock:
                    _save_channels()
                _log(f"channel {cs.name} scope changed: {old_scope} -> {new_cat_name}")
            else:
                _log(f"channel {after.name} moved to {new_cat_name} (new)")
                def _run():
                    _setup_channel_context(cid, after.name, is_running, category_id=after_parent, category_name=new_cat_name)
                threading.Thread(target=_run, daemon=True).start()
        elif after_parent is None or after_parent not in _categories:
            if cid in _channels:
                cs = _channels[cid]
                old_scope = cs.category_name or "global"
                if before_parent in _categories:
                    cs.category_id = ""
                    cs.category_name = ""
                    with _channels_lock:
                        _save_channels()
                    _log(f"channel {cs.name} scope changed: {old_scope} -> global (moved out of category)")
                else:
                    cs.running = False
                    cs.stop_event.set()
                    cs.wake.set()
                    with _channels_lock:
                        _save_channels()
                    _log(f"channel {cs.name} moved out of managed categories")

    @client.event
    async def on_guild_channel_delete(channel):
        ch_id = str(channel.id)
        if hasattr(channel, "type") and channel.type.value == 4:
            if ch_id in _categories:
                _log(f"category deleted: {_categories[ch_id]} (id={ch_id})")
                del _categories[ch_id]
            return
        if ch_id not in _channels:
            return
        _log(f"managed channel deleted: {channel.name} (id={ch_id})")
        def _run():
            _delete_channel_context(ch_id)
            _send_discord_text(f"Channel {channel.name} deleted (cleaned up context).")
        threading.Thread(target=_run, daemon=True).start()

    @client.event
    async def on_guild_channel_pins_update(channel, last_pin):
        cid = str(channel.id)
        if cid not in _channels:
            return
        _log(f"pins updated for channel {channel.name} (id={cid})")
        def _run():
            pin_text = _fetch_channel_pins(cid)
            cs = _channels.get(cid)
            if not cs:
                return
            pf = _pin_file(cid)
            pf.parent.mkdir(parents=True, exist_ok=True)
            pf.write_text(pin_text)
            cs.pin_text = pin_text
            new_hash = hashlib.sha256(pin_text.encode()).hexdigest()[:16] if pin_text else ""
            if new_hash != cs.pin_hash:
                cs.pin_hash = new_hash
                with _channels_lock:
                    _save_channels()
                _log(f"channel {cs.name}: pin updated ({len(pin_text)} chars)")
        threading.Thread(target=_run, daemon=True).start()

    @client.event
    async def on_thread_create(thread):
        if not hasattr(thread, "parent_id") or not thread.parent_id:
            return
        parent_cid = str(thread.parent_id)
        if parent_cid not in _channels:
            return
        thread_id = str(thread.id)
        def _run():
            _send_thread_welcome(thread_id, parent_cid)
        threading.Thread(target=_run, daemon=True).start()

    @client.event
    async def on_message(message):
        if message.author == client.user:
            return

        channel_id = str(message.channel.id)
        author_id = message.author.id
        content = message.content.strip()
        msg_id = str(message.id)

        # Messages in Discord threads under a managed channel
        if hasattr(message.channel, "parent_id") and message.channel.parent_id:
            parent_cid = str(message.channel.parent_id)
            if parent_cid in _channels and _is_owner(author_id) and content:
                # /commands in threads route to the parent channel's command handler
                if content.startswith("/"):
                    def _run_cmd(_content=content, _parent=parent_cid, _aid=author_id, _cid=channel_id, _mid=msg_id):
                        _handle_channel_command(_content, _parent, _aid, _mid, reply_channel=_cid)
                    threading.Thread(target=_run_cmd, daemon=True).start()
                    return
                log_chat("user", content, channel_id=parent_cid)
                _add_discord_reaction(channel_id, msg_id, "\u2705")
                def _run_thread_chat(_parent=parent_cid, _content=content, _cid=channel_id, _mid=msg_id):
                    cs = _channels.get(_parent)
                    _log(f"channel {cs.name if cs else _parent}: Discord thread chat reply")
                    prompt = _build_channel_chat_prompt(_parent, _content)
                    response = run_agent_streaming(prompt, _cid, reply_to=_mid)
                    log_chat("bot", response[:1000], channel_id=_parent)
                threading.Thread(target=_run_thread_chat, daemon=True).start()
                return

        if _is_managed_channel(channel_id):
            if content and content.startswith("/"):
                def _run():
                    _handle_channel_command(content, channel_id, author_id, msg_id)
                threading.Thread(target=_run, daemon=True).start()
            elif content or message.attachments:
                atts = [
                    {"filename": a.filename, "url": a.url, "size": a.size,
                     "content_type": a.content_type or ""}
                    for a in message.attachments
                ]
                def _run():
                    _handle_channel_message(content, atts, channel_id, msg_id, author_id)
                threading.Thread(target=_run, daemon=True).start()
            return

        expected_channel = _general_channel["id"]
        if expected_channel and channel_id != expected_channel:
            return

        if not expected_channel:
            if hasattr(message.channel, "name") and message.channel.name == "general":
                _save_general_id(channel_id)
            else:
                return

        if message.attachments:
            atts = [
                {
                    "filename": a.filename,
                    "url": a.url,
                    "size": a.size,
                    "content_type": a.content_type or "",
                }
                for a in message.attachments
            ]

            def _run():
                _handle_attachment_message(content, atts, channel_id, author_id, msg_id)
            threading.Thread(target=_run, daemon=True).start()
            return

        if not content:
            return

        if content.startswith("/"):
            def _run():
                _handle_command(content, channel_id, author_id, msg_id)
            threading.Thread(target=_run, daemon=True).start()
        else:
            def _run():
                _handle_text_message(content, channel_id, author_id, msg_id)
            threading.Thread(target=_run, daemon=True).start()

    _log("discord bot starting")
    while True:
        try:
            client.run(token, log_handler=None)
        except Exception as e:
            _log(f"discord bot error: {str(e)[:80]}, reconnecting in 5s")
            time.sleep(5)


# ── Main ─────────────────────────────────────────────────────────────────────

def _kill_child_procs():
    """Kill all tracked claude child processes."""
    with _child_procs_lock:
        procs = list(_child_procs)
    for proc in procs:
        try:
            if proc.poll() is None:
                _log(f"killing child claude pid={proc.pid}")
                proc.kill()
                proc.wait(timeout=5)
        except Exception:
            pass
    with _child_procs_lock:
        _child_procs.clear()


def _kill_stale_claude_procs():
    """Kill any leftover claude processes from a previous arbos instance."""
    my_pid = os.getpid()
    try:
        result = subprocess.run(
            ["pgrep", "-x", "claude"], capture_output=True, text=True, timeout=5,
        )
        for line in result.stdout.strip().splitlines():
            pid = int(line.strip())
            if pid == my_pid:
                continue
            try:
                os.kill(pid, signal.SIGKILL)
                _log(f"killed stale claude orphan pid={pid}")
            except ProcessLookupError:
                pass
            except PermissionError:
                pass
    except Exception:
        pass


def _send_cli(args: list[str]):
    """CLI entry point: python arbos.py send 'message' [--file path]

    Within a step, all sends are consolidated into a single Discord message.
    The first send creates it; subsequent sends edit it by appending.
    Uses ARBOS_CHANNEL_ID env var to find the per-channel step message file.
    In silent mode (ARBOS_SILENT=1), sends are suppressed — the loop posts a
    summary after the step instead.
    """
    if os.environ.get("ARBOS_SILENT") == "1":
        print("(silent mode — send suppressed)")
        return

    _load_channels()
    import argparse
    parser = argparse.ArgumentParser(description="Send a Discord message to the operator")
    parser.add_argument("message", nargs="?", help="Message text to send")
    parser.add_argument("--file", help="Send contents of a file instead")
    parsed = parser.parse_args(args)

    if not parsed.message and not parsed.file:
        parser.error("Provide a message or --file")

    if parsed.file:
        text = Path(parsed.file).read_text()
    else:
        text = parsed.message

    channel_id = os.environ.get("ARBOS_CHANNEL_ID", "")
    if channel_id:
        smf = _step_msg_file(channel_id)
    else:
        smf = CONTEXT_DIR / ".step_msg"
    smf.parent.mkdir(parents=True, exist_ok=True)

    if smf.exists():
        try:
            state = json.loads(smf.read_text())
            msg_id = state["msg_id"]
            prev_text = state.get("text", "")
        except (json.JSONDecodeError, KeyError):
            msg_id = None
            prev_text = ""
    else:
        msg_id = None
        prev_text = ""

    if msg_id:
        combined = (prev_text + "\n\n" + text).strip()
        if _edit_discord_text(msg_id, combined):
            smf.write_text(json.dumps({"msg_id": msg_id, "text": combined}))
            log_chat("bot", combined[:1000])
            print(f"Edited step message ({len(combined)} chars)")
        else:
            new_id = _send_discord_new(text)
            if new_id:
                smf.write_text(json.dumps({"msg_id": new_id, "text": text}))
                log_chat("bot", text[:1000])
                print(f"Sent new message ({len(text)} chars)")
            else:
                print("Failed to send", file=sys.stderr)
                sys.exit(1)
    else:
        new_id = _send_discord_new(text)
        if new_id:
            smf.write_text(json.dumps({"msg_id": new_id, "text": text}))
            log_chat("bot", text[:1000])
            print(f"Sent ({len(text)} chars)")
        else:
            print("Failed to send (check BOT_TOKEN and channel_id.txt)", file=sys.stderr)
            sys.exit(1)


def _sendfile_cli(args: list[str]):
    """CLI entry point: python arbos.py sendfile path/to/file [--caption 'text'] [--photo]"""
    _load_channels()
    import argparse
    parser = argparse.ArgumentParser(description="Send a file to the operator via Discord")
    parser.add_argument("path", help="Path to the file to send")
    parser.add_argument("--caption", default="", help="Caption for the file")
    parser.add_argument("--photo", action="store_true", help="Send as a compressed photo instead of a document")
    parsed = parser.parse_args(args)

    file_path = Path(parsed.path)
    if not file_path.exists():
        print(f"File not found: {file_path}", file=sys.stderr)
        sys.exit(1)

    if parsed.photo:
        ok = _send_discord_photo(str(file_path), caption=parsed.caption)
    else:
        ok = _send_discord_document(str(file_path), caption=parsed.caption)

    if ok:
        print(f"Sent {'photo' if parsed.photo else 'file'}: {file_path.name}")
    else:
        print("Failed to send (check BOT_TOKEN and channel_id.txt)", file=sys.stderr)
        sys.exit(1)


def _done_cli():
    """CLI: python arbos.py done — mark goal complete and stop the loop."""
    _load_channels()
    channel_id = os.environ.get("ARBOS_CHANNEL_ID", "")
    if not channel_id:
        print("ARBOS_CHANNEL_ID must be set", file=sys.stderr)
        sys.exit(1)
    cs = _channels.get(channel_id)
    if cs:
        cs.running = False
        cs.stop_event.set()
        with _channels_lock:
            _save_channels()
        _sync_channel_name(channel_id)
    token = os.getenv("BOT_TOKEN", "")
    if token and channel_id:
        try:
            requests.post(
                f"{DISCORD_API}/channels/{channel_id}/messages",
                headers=_discord_headers(token),
                json={"content": "Goal completed. Loop stopped."},
                timeout=15,
            )
        except Exception:
            pass
    print(f"Channel {channel_id} marked as done and stopped")


def _ctl_cli():
    """CLI: python arbos.py ctl <action> [args] — channel self-control.

    Actions:
        delay <seconds>   — set step delay
        restart           — restart the loop
        goal <new goal>   — update the goal
        pause             — pause the loop
    """
    _load_channels()
    channel_id = os.environ.get("ARBOS_CHANNEL_ID", "")
    if not channel_id:
        print("ARBOS_CHANNEL_ID must be set", file=sys.stderr)
        sys.exit(1)
    cs = _channels.get(channel_id)
    if not cs:
        print(f"Channel {channel_id} not found", file=sys.stderr)
        sys.exit(1)

    if len(sys.argv) < 3:
        print("Usage: python arbos.py ctl <delay|restart|goal|pause> [args]", file=sys.stderr)
        sys.exit(1)

    action = sys.argv[2].lower()

    if action == "delay":
        if len(sys.argv) < 4:
            print("Usage: python arbos.py ctl delay <seconds>", file=sys.stderr)
            sys.exit(1)
        try:
            new_delay = int(sys.argv[3])
        except ValueError:
            print("Delay must be an integer (seconds)", file=sys.stderr)
            sys.exit(1)
        cs.delay = max(0, new_delay)
        with _channels_lock:
            _save_channels()
        _sync_channel_name(channel_id)
        print(f"Delay set to {cs.delay}s")

    elif action == "restart":
        cs.stop_event.set()
        cs.wake.set()
        cs.loop_thread = None
        cs.stop_event = threading.Event()
        cs.wake = threading.Event()
        cs.running = True
        with _channels_lock:
            _save_channels()
        _sync_channel_name(channel_id)
        print("Restart signaled")

    elif action == "goal":
        if len(sys.argv) < 4:
            print("Usage: python arbos.py ctl goal <new goal text>", file=sys.stderr)
            sys.exit(1)
        new_goal = " ".join(sys.argv[3:])
        cs.goal = new_goal
        gf = _goal_file(channel_id)
        gf.parent.mkdir(parents=True, exist_ok=True)
        gf.write_text(new_goal)
        with _channels_lock:
            _save_channels()
        print("Goal updated")

    elif action == "pause":
        cs.running = False
        cs.stop_event.set()
        cs.wake.set()
        with _channel_procs_lock:
            proc = _channel_procs.get(channel_id)
        if proc and proc.poll() is None:
            proc.kill()
        with _channels_lock:
            _save_channels()
        _sync_channel_name(channel_id)
        print("Loop paused")

    else:
        print(f"Unknown action: {action}. Use: delay, restart, goal, pause", file=sys.stderr)
        sys.exit(1)


def _scope_cli():
    """CLI: python arbos.py scope — print current scope level and visible siblings."""
    channel_id = os.environ.get("ARBOS_CHANNEL_ID", "")
    if not channel_id:
        print("ARBOS_CHANNEL_ID must be set", file=sys.stderr)
        sys.exit(1)
    _load_channels()
    cs = _channels.get(channel_id)
    if not cs:
        print(f"Channel {channel_id} not found in channels.json", file=sys.stderr)
        sys.exit(1)
    if cs.category_id:
        cat_name = cs.category_name or cs.category_id
        siblings = [
            c for cid, c in _channels.items()
            if c.category_id == cs.category_id and cid != channel_id
        ]
        print(f"Scope: category \"{cat_name}\"")
        print(f"Channel: {cs.name} ({'running' if cs.running else 'paused'})")
        if siblings:
            print(f"Siblings ({len(siblings)}):")
            for s in sorted(siblings, key=lambda c: c.name):
                status = "running" if s.running else "stopped"
                step_info = f" step {s.step_count}" if s.step_count else ""
                print(f"  - {s.name} [{status}{step_info}]")
        else:
            print("No siblings in this category.")
    else:
        print("Scope: global")
        print(f"Channel: {cs.name} ({'running' if cs.running else 'paused'})")
        print(f"Visible channels: {len(_channels) - 1}")
        for cid, c in sorted(_channels.items(), key=lambda x: x[1].name):
            if cid == channel_id:
                continue
            cat = f" [{c.category_name}]" if c.category_name else ""
            status = "running" if c.running else "stopped"
            step_info = f" step {c.step_count}" if c.step_count else ""
            print(f"  - {c.name}{cat} [{status}{step_info}]")


def _siblings_cli():
    """CLI: python arbos.py siblings — list sibling channels with summaries."""
    channel_id = os.environ.get("ARBOS_CHANNEL_ID", "")
    if not channel_id:
        print("ARBOS_CHANNEL_ID must be set", file=sys.stderr)
        sys.exit(1)
    _load_channels()
    cs = _channels.get(channel_id)
    if not cs:
        print(f"Channel {channel_id} not found", file=sys.stderr)
        sys.exit(1)
    if not cs.category_id:
        siblings = [c for cid, c in _channels.items() if cid != channel_id]
    else:
        siblings = [
            c for cid, c in _channels.items()
            if c.category_id == cs.category_id and cid != channel_id
        ]
    if not siblings:
        print("No siblings found.")
        return
    for s in sorted(siblings, key=lambda c: c.name):
        print(_channel_summary(s))
        print()


def _send_to_cli():
    """CLI: python arbos.py send-to <channel-name> 'message' — send to a sibling."""
    if len(sys.argv) < 4:
        print("Usage: python arbos.py send-to <channel-name> <message>", file=sys.stderr)
        sys.exit(1)
    target_name = sys.argv[2]
    message = " ".join(sys.argv[3:])
    channel_id = os.environ.get("ARBOS_CHANNEL_ID", "")
    if not channel_id:
        print("ARBOS_CHANNEL_ID must be set", file=sys.stderr)
        sys.exit(1)
    _load_channels()
    cs = _channels.get(channel_id)
    if not cs:
        print(f"Channel {channel_id} not found", file=sys.stderr)
        sys.exit(1)
    if cs.category_id:
        visible = {cid: c for cid, c in _channels.items()
                   if c.category_id == cs.category_id and cid != channel_id}
    else:
        visible = {cid: c for cid, c in _channels.items() if cid != channel_id}
    target_cs = None
    target_cid = None
    for cid, c in visible.items():
        if c.name == target_name:
            target_cs = c
            target_cid = cid
            break
    if not target_cs:
        print(f"Channel '{target_name}' not found in scope.", file=sys.stderr)
        avail = ", ".join(c.name for c in visible.values())
        print(f"Available: {avail}", file=sys.stderr)
        sys.exit(1)
    log_chat("bot", f"[from #{cs.name}] {message}", channel_id=target_cid)
    log_chat("bot", f"[sent to #{target_name}] {message}", channel_id=channel_id)
    token = os.environ.get("BOT_TOKEN", "")
    if token and target_cid:
        _send_discord_text(f"**[from #{cs.name}]** {message}", target=(token, target_cid))
    print(f"Message sent to #{target_name}.")


def _read_sibling_cli():
    """CLI: python arbos.py read-sibling <channel-name> [state|pin|goal] — read sibling context."""
    if len(sys.argv) < 3:
        print("Usage: python arbos.py read-sibling <channel-name> [state|pin|goal]", file=sys.stderr)
        sys.exit(1)
    target_name = sys.argv[2]
    what = sys.argv[3] if len(sys.argv) > 3 else "all"
    channel_id = os.environ.get("ARBOS_CHANNEL_ID", "")
    if not channel_id:
        print("ARBOS_CHANNEL_ID must be set", file=sys.stderr)
        sys.exit(1)
    _load_channels()
    cs = _channels.get(channel_id)
    if not cs:
        print(f"Channel {channel_id} not found", file=sys.stderr)
        sys.exit(1)
    if cs.category_id:
        visible = {cid: c for cid, c in _channels.items()
                   if c.category_id == cs.category_id and cid != channel_id}
    else:
        visible = {cid: c for cid, c in _channels.items() if cid != channel_id}
    target_cs = None
    for cid, c in visible.items():
        if c.name == target_name:
            target_cs = c
            break
    if not target_cs:
        print(f"Channel '{target_name}' not found in scope.", file=sys.stderr)
        avail = ", ".join(c.name for c in visible.values())
        print(f"Available: {avail}", file=sys.stderr)
        sys.exit(1)
    if what in ("state", "all"):
        sf = _state_file(target_cs.channel_id)
        state = sf.read_text().strip() if sf.exists() else "(empty)"
        print(f"## State\n{state}\n")
    if what in ("pin", "all"):
        pf = _pin_file(target_cs.channel_id)
        pin = pf.read_text().strip() if pf.exists() else "(no pin)"
        print(f"## Pin\n{pin}\n")
    if what in ("goal", "all"):
        goal = target_cs.goal or "(no goal)"
        print(f"## Goal\n{goal}\n")


def _resource_cli():
    """CLI: python arbos.py resource <add|rm|list> — manage tracked resources.

    add <type> <name> [--destroy "cmd"] [--meta key=value ...]
    rm <name>
    list
    """
    _load_channels()
    channel_id = os.environ.get("ARBOS_CHANNEL_ID", "")
    if not channel_id:
        print("ARBOS_CHANNEL_ID must be set", file=sys.stderr)
        sys.exit(1)
    cs = _channels.get(channel_id)
    if not cs:
        print(f"Channel {channel_id} not found", file=sys.stderr)
        sys.exit(1)

    args = sys.argv[2:]
    if not args:
        print("Usage: python arbos.py resource <add|rm|list>", file=sys.stderr)
        sys.exit(1)

    action = args[0].lower()

    if action == "list":
        if not cs.resources and not cs.pm2_procs:
            print("No resources or processes tracked.")
            return
        if cs.pm2_procs:
            print(f"## PM2 Processes ({len(cs.pm2_procs)})")
            for p in cs.pm2_procs:
                print(f"  - {p}")
        if cs.resources:
            print(f"## Resources ({len(cs.resources)})")
            for r in cs.resources:
                meta = r.get("meta", {})
                meta_str = " | ".join(f"{k}={v}" for k, v in meta.items()) if meta else ""
                destroy = f" | destroy: {r['destroy_cmd']}" if r.get("destroy_cmd") else ""
                print(f"  - [{r.get('type', '?')}] {r.get('name', '?')}{' (' + meta_str + ')' if meta_str else ''}{destroy}")

    elif action == "add":
        if len(args) < 3:
            print("Usage: python arbos.py resource add <type> <name> [--destroy \"cmd\"] [--meta key=value ...]", file=sys.stderr)
            sys.exit(1)
        rtype = args[1]
        rname = args[2]
        destroy_cmd = ""
        meta: dict[str, str] = {}
        i = 3
        while i < len(args):
            if args[i] == "--destroy" and i + 1 < len(args):
                destroy_cmd = args[i + 1]
                i += 2
            elif args[i] == "--meta" and i + 1 < len(args):
                i += 1
                while i < len(args) and not args[i].startswith("--"):
                    if "=" in args[i]:
                        k, v = args[i].split("=", 1)
                        meta[k] = v
                    i += 1
            else:
                i += 1

        resource = {
            "type": rtype,
            "name": rname,
            "created_at": datetime.now().isoformat(),
            "destroy_cmd": destroy_cmd,
            "meta": meta,
        }
        cs.resources.append(resource)
        with _channels_lock:
            _save_channels()
        print(f"Resource added: [{rtype}] {rname}")
        if destroy_cmd:
            print(f"  destroy: {destroy_cmd}")
        if meta:
            print(f"  meta: {meta}")

    elif action == "rm":
        if len(args) < 2:
            print("Usage: python arbos.py resource rm <name>", file=sys.stderr)
            sys.exit(1)
        target = args[1]
        found = None
        for idx, r in enumerate(cs.resources):
            if r.get("name") == target:
                found = idx
                break
        if found is None:
            print(f"Resource '{target}' not found", file=sys.stderr)
            sys.exit(1)
        removed = cs.resources.pop(found)
        with _channels_lock:
            _save_channels()
        print(f"Resource removed: [{removed.get('type', '?')}] {removed.get('name', '?')}")

    else:
        print(f"Unknown action: {action}. Use: add, rm, list", file=sys.stderr)
        sys.exit(1)


def main() -> None:
    if len(sys.argv) > 1 and sys.argv[1] == "send":
        _send_cli(sys.argv[2:])
        return

    if len(sys.argv) > 1 and sys.argv[1] == "sendfile":
        _sendfile_cli(sys.argv[2:])
        return

    if len(sys.argv) > 1 and sys.argv[1] == "encrypt":
        env_path = WORKING_DIR / ".env"
        if not env_path.exists():
            if ENV_ENC_FILE.exists():
                print(".env.enc already exists (already encrypted)")
            else:
                print(".env not found, nothing to encrypt")
            return
        load_dotenv(env_path)
        bot_token = os.environ.get("BOT_TOKEN", "")
        if not bot_token:
            print("BOT_TOKEN must be set in .env", file=sys.stderr)
            sys.exit(1)
        _encrypt_env_file(bot_token)
        print("Encrypted .env → .env.enc, deleted plaintext.")
        print(f"On future starts: BOT_TOKEN='{bot_token}' python arbos.py")
        return

    if len(sys.argv) > 1 and sys.argv[1] == "done":
        _done_cli()
        return

    if len(sys.argv) > 1 and sys.argv[1] == "ctl":
        _ctl_cli()
        return

    if len(sys.argv) > 1 and sys.argv[1] == "resource":
        _resource_cli()
        return

    if len(sys.argv) > 1 and sys.argv[1] == "scope":
        _scope_cli()
        return

    if len(sys.argv) > 1 and sys.argv[1] == "siblings":
        _siblings_cli()
        return

    if len(sys.argv) > 1 and sys.argv[1] == "send-to":
        _send_to_cli()
        return

    if len(sys.argv) > 1 and sys.argv[1] == "read-sibling":
        _read_sibling_cli()
        return

    _KNOWN_CMDS = {"send", "encrypt", "sendfile", "done", "ctl",
                   "scope", "siblings", "send-to", "read-sibling", "resource"}
    if len(sys.argv) > 1 and sys.argv[1] not in _KNOWN_CMDS:
        print(f"Unknown subcommand: {sys.argv[1]}", file=sys.stderr)
        print("Usage: arbos.py [send|sendfile|encrypt|done|ctl|scope|siblings|send-to|read-sibling|resource]", file=sys.stderr)
        sys.exit(1)

    _log(f"arbos starting in {WORKING_DIR} (provider={PROVIDER}, model={CLAUDE_MODEL})")
    _kill_stale_claude_procs()
    _reload_env_secrets()
    CONTEXT_DIR.mkdir(parents=True, exist_ok=True)
    GENERAL_DIR.mkdir(parents=True, exist_ok=True)
    GENERAL_CHAT_DIR.mkdir(parents=True, exist_ok=True)

    _load_channels()
    _log(f"loaded {len(_channels)} channel(s) from channels.json")

    if not LLM_API_KEY:
        key_name = "OPENROUTER_API_KEY" if PROVIDER == "openrouter" else "CHUTES_API_KEY"
        _log(f"WARNING: {key_name} not set — LLM calls will fail")

    def _handle_sigterm(signum, frame):
        _log("SIGTERM received; shutting down gracefully")
        _shutdown.set()

    signal.signal(signal.SIGTERM, _handle_sigterm)

    if PROVIDER != "openrouter":
        _log(f"starting chutes proxy thread (port={PROXY_PORT}, agent={CHUTES_ROUTING_AGENT}, bot={CHUTES_ROUTING_BOT})")
        threading.Thread(target=_start_proxy, daemon=True).start()
        time.sleep(1)
    else:
        _log(f"openrouter direct mode — no proxy needed (target={LLM_BASE_URL})")

    _write_claude_settings()

    threading.Thread(target=_channel_manager, daemon=True).start()
    threading.Thread(target=run_bot, daemon=True).start()

    while not _shutdown.is_set():
        if RESTART_FLAG.exists():
            RESTART_FLAG.unlink()
            _log("restart requested; killing children and exiting for pm2")
            _kill_child_procs()
            sys.exit(0)
        _process_pending_env()
        _shutdown.wait(timeout=1)

    _log("shutdown: killing children")
    _kill_child_procs()
    _log("shutdown complete")
    sys.exit(0)


if __name__ == "__main__":
    main()
