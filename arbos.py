import base64
import json
import os
import selectors
import signal
import subprocess
import sys
import time
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from datetime import datetime
from dataclasses import dataclass, field
from typing import Any

import hashlib
import re

from dotenv import load_dotenv
import requests
from cryptography.fernet import Fernet, InvalidToken
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC
from cryptography.hazmat.primitives import hashes

WORKING_DIR = Path(__file__).parent
PROMPT_FILE = WORKING_DIR / "PROMPT.md"
CONTEXT_DIR = WORKING_DIR / "context"
GOAL_FILE = CONTEXT_DIR / "GOAL.md"
# Presence of GO.md = goal loop may run steps; absence = paused (GOAL.md unchanged).
GO_FLAG_FILE = CONTEXT_DIR / "GO.md"
STATE_FILE = CONTEXT_DIR / "STATE.md"
INBOX_FILE = CONTEXT_DIR / "INBOX.md"
RUNS_DIR = CONTEXT_DIR / "runs"
META_FILE = CONTEXT_DIR / "meta.json"
STEP_MSG_FILE = CONTEXT_DIR / ".step_msg"
CONTEXT_LOGS_DIR = CONTEXT_DIR / "logs"
CHATLOG_DIR = CONTEXT_DIR / "chat"
FILES_DIR = CONTEXT_DIR / "files"
RESTART_FLAG = WORKING_DIR / ".restart"
CHAT_ID_FILE = WORKING_DIR / "chat_id.txt"
# Forum supergroup topic for operator + goal-loop bubbles (same thread as streaming /operator replies).
OPERATOR_THREAD_ID_FILE = WORKING_DIR / "operator_thread_id.txt"
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
    bot_token = os.environ.get("TAU_BOT_TOKEN", "")
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

    bot_token = os.environ.get("TAU_BOT_TOKEN", "")
    if ENV_ENC_FILE.exists() and bot_token:
        if _load_encrypted_env(bot_token):
            return
        print("ERROR: failed to decrypt .env.enc — wrong TAU_BOT_TOKEN?", file=sys.stderr)
        sys.exit(1)

    if ENV_ENC_FILE.exists() and not bot_token:
        print("ERROR: .env.enc exists but TAU_BOT_TOKEN not set.", file=sys.stderr)
        print("Pass it as an env var: TAU_BOT_TOKEN=xxx python arbos.py", file=sys.stderr)
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
            bot_token = os.environ.get("TAU_BOT_TOKEN", "")
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
HEALTH_PORT = int(os.environ.get("ARBOS_HEALTH_PORT") or os.environ.get("PROXY_PORT", "8089"))
CLAUDE_MODEL = os.environ.get("CLAUDE_MODEL", "anthropic/claude-opus-4.6")
LLM_API_KEY = os.environ.get("OPENROUTER_API_KEY", "")
LLM_BASE_URL = "https://openrouter.ai/api"
IS_ROOT = os.getuid() == 0
MAX_RETRIES = int(os.environ.get("CLAUDE_MAX_RETRIES", "5"))
try:
    _claude_timeout_raw = int(os.environ.get("CLAUDE_TIMEOUT", "600").strip())
except ValueError:
    _claude_timeout_raw = 600
if _claude_timeout_raw < 0:
    _claude_timeout_raw = 600
CLAUDE_TIMEOUT = _claude_timeout_raw
CLAUDE_IDLE_KILL = CLAUDE_TIMEOUT > 0
_tls = threading.local()
_log_lock = threading.Lock()
_chatlog_lock = threading.Lock()
_pending_env_lock = threading.Lock()

_ENV_KEY_NAME_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
_MAX_TELEGRAM_ENV_KEY_LEN = 128
_MAX_TELEGRAM_ENV_VALUE_LEN = 8192
_MAX_TELEGRAM_ENV_DESC_LEN = 500


def _sanitize_telegram_env_description(text: str) -> str:
    """Single-line comment text; no newlines or control characters."""
    t = " ".join((text or "").split())
    t = "".join(ch for ch in t if ch >= " " or ch in "\t")
    t = t.replace("\t", " ")
    return t.strip()[:_MAX_TELEGRAM_ENV_DESC_LEN].strip()


def _dotenv_double_quote_value(val: str) -> str:
    if len(val) > _MAX_TELEGRAM_ENV_VALUE_LEN:
        raise ValueError(f"value exceeds {_MAX_TELEGRAM_ENV_VALUE_LEN} characters")
    if "\n" in val or "\r" in val:
        raise ValueError("value must not contain line breaks")
    return val.replace("\\", "\\\\").replace('"', '\\"')


def _apply_env_key_comment_block(
    lines: list[str], key: str, value_line: str, comment_text: str
) -> list[str]:
    """Insert or update ``# comment`` + ``KEY=...`` block in .env-style lines."""
    assign_prefix = f"{key}="
    idx = None
    for i, line in enumerate(lines):
        stripped = line.split("#", 1)[0].strip()
        if stripped.startswith(assign_prefix):
            idx = i
            break
    comment_line = f"# {comment_text}".rstrip()
    if idx is None:
        out = list(lines)
        if out and out[-1].strip():
            out.append("")
        out.append(comment_line)
        out.append(value_line)
        return out
    out = list(lines)
    if idx > 0 and out[idx - 1].strip().startswith("#"):
        out[idx - 1] = comment_line
        out[idx] = value_line
    else:
        out.insert(idx, comment_line)
        out[idx + 1] = value_line
    return out


def _persist_env_var_with_comment(key: str, value: str, description: str) -> tuple[bool, str]:
    """Append or update a variable in ``.env`` / ``.env.enc`` with a preceding comment line."""
    key = key.strip()
    if not key or len(key) > _MAX_TELEGRAM_ENV_KEY_LEN or not _ENV_KEY_NAME_RE.match(key):
        return False, "Invalid KEY: use letters, digits, underscore; must start with letter or _."
    desc = _sanitize_telegram_env_description(description)
    if not desc:
        return False, "DESCRIPTION required (non-empty after trimming)."
    try:
        escaped = _dotenv_double_quote_value(value)
    except ValueError as e:
        return False, str(e)
    value_line = f'{key}="{escaped}"'
    env_path = WORKING_DIR / ".env"

    with _pending_env_lock:
        if env_path.exists():
            content = env_path.read_text()
            new_lines = _apply_env_key_comment_block(
                content.splitlines(), key, value_line, desc
            )
            env_path.write_text("\n".join(new_lines) + "\n")
        elif ENV_ENC_FILE.exists():
            bot_token = os.environ.get("TAU_BOT_TOKEN", "")
            if not bot_token:
                return False, "Cannot update encrypted .env: TAU_BOT_TOKEN not set in this process."
            try:
                content = _decrypt_env_content(bot_token)
            except InvalidToken:
                return False, "Could not decrypt .env.enc (wrong token?)."
            new_lines = _apply_env_key_comment_block(
                content.splitlines(), key, value_line, desc
            )
            payload = "\n".join(new_lines) + "\n"
            fernet = Fernet(_derive_fernet_key(bot_token))
            ENV_ENC_FILE.write_bytes(fernet.encrypt(payload.encode()))
            os.chmod(str(ENV_ENC_FILE), 0o600)
        else:
            return False, "No .env or .env.enc in project directory."

        os.environ[key] = value
        _reload_env_secrets()

    return True, f"Saved `{key}` with comment (value not shown). Active in this process."


_shutdown = threading.Event()
_step_count = 0
# Approximate size of the prompt Arbos passes into the agent (request scale for OpenRouter).
_context_est_tokens = 0
_context_lock = threading.Lock()
_child_procs: set[subprocess.Popen] = set()
_child_procs_lock = threading.Lock()


# ── Single-agent state ───────────────────────────────────────────────────────


@dataclass
class AgentState:
    summary: str = ""
    delay_minutes: int = 0
    started: bool = False
    paused: bool = False
    step_count: int = 0
    goal_hash: str = ""
    last_run: str = ""
    last_finished: str = ""
    last_step_ok: bool | None = None
    last_step_error: str = ""
    thread: threading.Thread | None = field(default=None, repr=False)
    wake: threading.Event = field(default_factory=threading.Event, repr=False)
    stop_event: threading.Event = field(default_factory=threading.Event, repr=False)


_agent = AgentState()
# Agent state is saved from both locked and unlocked call sites, so this must
# be re-entrant to avoid self-deadlocking during /loop, /resume, and step churn.
_agent_lock = threading.RLock()

# First step bubble from /loop (sent in the bot handler so it appears before the agent thread runs).
_pending_step_msg_id: int | None = None
_pending_step_msg_lock = threading.Lock()

# Process-wide operator visibility (HTTP /health, Telegram /status).
_arbos_boot_wall: float = 0.0
_operator_lock = threading.Lock()
_operator: dict[str, Any] = {
    "phase": "boot",
    "detail": "",
    "last_tick_wall": 0.0,
    "last_error": "",
}


def _operator_set(
    phase: str,
    detail: str = "",
    *,
    last_error: str | None = None,
) -> None:
    with _operator_lock:
        _operator["phase"] = phase
        _operator["detail"] = detail
        _operator["last_tick_wall"] = time.time()
        if last_error is not None:
            _operator["last_error"] = last_error[:800]


def _operator_tick() -> None:
    with _operator_lock:
        _operator["last_tick_wall"] = time.time()


def _operator_health_payload() -> dict[str, Any]:
    """JSON-serializable snapshot for GET /health (and operators)."""
    now = time.time()
    with _operator_lock:
        tick = float(_operator["last_tick_wall"])
        op = {
            "phase": _operator["phase"],
            "detail": _operator["detail"],
            "seconds_since_activity": max(0, int(now - tick)),
            "last_error": _operator["last_error"] or None,
        }
    boot = _arbos_boot_wall or now
    out: dict[str, Any] = {
        "status": "ok",
        "uptime_seconds": int(now - boot),
        "operator": op,
        "agent": {},
    }
    with _agent_lock:
        gs = _agent
        out["agent"] = {
            "state": _agent_status_label(gs),
            "go": GO_FLAG_FILE.exists(),
            "delay_minutes": gs.delay_minutes,
            "step_count": gs.step_count,
            "last_step_ok": gs.last_step_ok,
            "last_run": gs.last_run or None,
            "last_finished": gs.last_finished or None,
            "last_step_error": (gs.last_step_error[:240] + "…")
            if len(gs.last_step_error) > 240
            else (gs.last_step_error or None),
            "summary": gs.summary or None,
        }
    if not LLM_API_KEY:
        out["status"] = "degraded"
        out["degraded_reason"] = "OPENROUTER_API_KEY not set — LLM calls will fail"
    return out


def _save_agent():
    """Persist agent metadata to meta.json.

    Safe to call both inside and outside an existing ``with _agent_lock`` block.
    """
    with _agent_lock:
        st = _agent.started
        data = {
            "summary": _agent.summary,
            "delay_minutes": _agent.delay_minutes,
            "started": st,
            "paused": _paused_persistent(started=st),
            "step_count": _agent.step_count,
            "goal_hash": _agent.goal_hash,
            "last_run": _agent.last_run,
            "last_finished": _agent.last_finished,
            "last_step_ok": _agent.last_step_ok,
            "last_step_error": _agent.last_step_error,
        }
    META_FILE.parent.mkdir(parents=True, exist_ok=True)
    META_FILE.write_text(json.dumps(data, indent=2))


def _load_agent():
    """Load agent metadata from meta.json into _agent."""
    if not META_FILE.exists():
        return
    try:
        info = json.loads(META_FILE.read_text())
    except (json.JSONDecodeError, OSError):
        return
    with _agent_lock:
        _agent.summary = info.get("summary", "")
        if "delay_minutes" in info:
            _agent.delay_minutes = int(info["delay_minutes"])
        else:
            legacy_s = int(info.get("delay", 0))
            _agent.delay_minutes = (legacy_s + 59) // 60 if legacy_s > 0 else 0
        _agent.started = info.get("started", False)
        _agent.step_count = info.get("step_count", 0)
        _agent.goal_hash = info.get("goal_hash", "")
        _agent.last_run = info.get("last_run", "")
        _agent.last_finished = info.get("last_finished", "")
        _agent.last_step_ok = info.get("last_step_ok")
        _agent.last_step_error = info.get("last_step_error", "")
        _agent.paused = _paused_persistent(started=_agent.started)


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


def _paused_persistent(*, started: bool | None = None) -> bool:
    """True when a goal exists but GO.md is absent (intentional pause).

    Pass ``started`` when already holding ``_agent_lock`` to avoid deadlock.
    """
    if started is None:
        with _agent_lock:
            st = _agent.started
    else:
        st = started
    if not st:
        return False
    if not GOAL_FILE.exists() or not GOAL_FILE.read_text().strip():
        return False
    return not GO_FLAG_FILE.exists()


def _write_go_flag() -> None:
    CONTEXT_DIR.mkdir(parents=True, exist_ok=True)
    GO_FLAG_FILE.write_text(
        "# Arbos: run enabled\n"
        "# Remove this file (or /pause) to pause the loop without changing GOAL.md.\n"
    )


def _agent_status_label(gs: AgentState) -> str:
    if not gs.started:
        return "stopped"
    if not GOAL_FILE.exists() or not GOAL_FILE.read_text().strip():
        return "idle"
    if not GO_FLAG_FILE.exists():
        return "paused"
    return "running"


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


def _approx_prompt_context_tokens(prompt: str) -> int:
    """Rough token count from UTF-8 length (~4 bytes/token); matches common heuristics."""
    if not prompt:
        return 0
    n = len(prompt.encode("utf-8"))
    return max(1, (n + 3) // 4)


def _set_prompt_context_estimate(prompt: str) -> None:
    global _context_est_tokens
    with _context_lock:
        _context_est_tokens = _approx_prompt_context_tokens(prompt)


def _fmt_context_for_header(est: int) -> str:
    if est <= 0:
        return "~0 ctx"
    if est >= 1000:
        return f"~{est / 1000:.1f}k ctx"
    return f"~{est} ctx"


def _arbos_response_header(elapsed_s: float) -> str:
    """First line on Telegram: elapsed + estimated request context (our prompt to the agent)."""
    with _context_lock:
        est = _context_est_tokens
    return f"Arbos ( {fmt_duration(elapsed_s)}, {_fmt_context_for_header(est)} )"


def _step_response_header(elapsed_s: float) -> str:
    """First line on step bubbles: same timing/ctx as Arbos replies, prefixed with Step."""
    with _context_lock:
        est = _context_est_tokens
    return f"Step ( {fmt_duration(elapsed_s)}, {_fmt_context_for_header(est)} )"


# ── Prompt helpers ───────────────────────────────────────────────────────────

# Built-in descriptions for env vars Arbos / Claude Code use (values never go in prompts).
_ARBOS_ENV_BUILTIN_DOC: dict[str, str] = {
    "AGENT_DELAY": "Extra seconds added to the loop delay between agent steps.",
    "ANTHROPIC_API_KEY": "API key passed into the agent subprocess for LLM calls (from OPENROUTER_API_KEY).",
    "ANTHROPIC_AUTH_TOKEN": "Optional auth token for the API (cleared when using OpenRouter).",
    "ANTHROPIC_BASE_URL": "Anthropic-compatible API base URL (OpenRouter).",
    "ARBOS_HEALTH_PORT": "Local HTTP /health port for this Arbos process.",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "Claude Code setting to reduce background traffic.",
    "CLAUDE_MAX_RETRIES": "Retries when a Claude run fails.",
    "CLAUDE_MODEL": "Model id for Claude Code (e.g. anthropic/claude-opus-4.6).",
    "CLAUDE_TIMEOUT": "Idle watchdog timeout in seconds for Claude runs; 0 disables.",
    "OPENROUTER_API_KEY": "OpenRouter API key (loaded into the agent as ANTHROPIC_API_KEY).",
    "PROXY_PORT": "Fallback for health port if ARBOS_HEALTH_PORT is unset.",
    "TAU_BOT_TOKEN": "Telegram bot token (Arbos host only; not passed to the coding agent subprocess).",
    "TELEGRAM_OWNER_ID": "Telegram user id allowed to control the bot.",
}

_ENV_PROMPT_SKIP_KEYS = frozenset({
    "PATH", "HOME", "USER", "LOGNAME", "MAIL", "SHELL", "PWD", "OLDPWD",
    "LANG", "LANGUAGE", "LC_ALL", "LC_CTYPE", "LC_NUMERIC", "LC_TIME",
    "TERM", "TERMINAL", "SHLVL", "HOSTNAME", "HOST", "INVOCATION_ID",
    "JOURNAL_STREAM", "MANPATH", "LS_COLORS", "COMP_WORDS", "COMP_LINE", "_",
    "CLUTTER_IM_MODULE", "SESSION_MANAGER", "XAUTHORITY", "DISPLAY",
    "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS",
})

_ENV_PROMPT_SKIP_PREFIXES = (
    "XDG_", "VSCODE", "CURSOR_", "npm_", "SSH_CONNECTION", "SSH_CLIENT",
    "SSH_TTY", "LESSOPEN", "LESSCLOSE", "BUNDLED_DEBUGPY", "PYDEVD_",
)


def _env_key_suppressed_for_prompt(key: str) -> bool:
    if key in _ENV_PROMPT_SKIP_KEYS:
        return True
    return any(key.startswith(p) for p in _ENV_PROMPT_SKIP_PREFIXES)


def _env_key_looks_config_like(key: str) -> bool:
    if _env_key_suppressed_for_prompt(key):
        return False
    u = key.upper()
    if u.startswith(
        (
            "CLAUDE_", "ARBOS_", "OPENROUTER_", "TELEGRAM_", "ANTHROPIC_",
            "AGENT_", "AWS_", "AZURE_", "GOOGLE_", "GCP_", "GITHUB", "GITLAB_",
            "DOCKER_", "OPENAI_", "HF_", "DATABASE", "DB_",
        )
    ) or u.startswith("KUBECONFIG"):
        return True
    needles = (
        "API_KEY", "_API_KEY", "_TOKEN", "_SECRET", "SECRET_", "PASSWORD",
        "CREDENTIAL", "WEBHOOK", "ENDPOINT", "_KEY", "AUTH",
    )
    return any(n in u for n in needles)


def _read_project_dotenv_plaintext() -> str | None:
    """Raw .env text for metadata (comments + keys); None if unavailable."""
    env_path = WORKING_DIR / ".env"
    if env_path.exists():
        return env_path.read_text()
    if not ENV_ENC_FILE.exists():
        return None
    tok = os.environ.get("TAU_BOT_TOKEN", "")
    if not tok:
        return None
    try:
        return _decrypt_env_content(tok)
    except InvalidToken:
        return None


def _parse_dotenv_key_comment_map(content: str) -> tuple[dict[str, str], set[str]]:
    """Map assignment keys to preceding full-line # comments; also return all assignment keys."""
    desc: dict[str, str] = {}
    all_keys: set[str] = set()
    pending: list[str] = []
    for raw in content.splitlines():
        s = raw.strip()
        if not s:
            continue
        if s.startswith("#"):
            pending.append(s[1:].strip())
            continue
        assign = s.split("#", 1)[0].strip()
        if "=" not in assign:
            pending.clear()
            continue
        key = assign.split("=", 1)[0].strip()
        if not _ENV_KEY_NAME_RE.match(key):
            pending.clear()
            continue
        all_keys.add(key)
        joined = "; ".join(x for x in pending if x)
        if joined:
            desc[key] = joined
        pending.clear()
    return desc, all_keys


def _agent_subprocess_env_keys() -> set[str]:
    """Env keys visible to the Claude Code subprocess (matches _claude_env)."""
    keys = set(os.environ.keys())
    keys.discard("TAU_BOT_TOKEN")
    keys.update(("ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN"))
    return keys


def _prompt_env_keys_to_show(agent_keys: set[str], dotenv_keys: set[str]) -> list[str]:
    show: set[str] = set()
    show |= agent_keys & dotenv_keys
    for k in agent_keys:
        if k in _ARBOS_ENV_BUILTIN_DOC or _env_key_looks_config_like(k):
            show.add(k)
    return sorted(show)


def format_available_env_vars_section(*, max_keys: int = 120) -> str:
    """Human-readable KEY + description for the agent (never values)."""
    agent_keys = _agent_subprocess_env_keys()
    raw = _read_project_dotenv_plaintext()
    file_desc: dict[str, str] = {}
    dotenv_keys: set[str] = set()
    if raw:
        file_desc, dotenv_keys = _parse_dotenv_key_comment_map(raw)

    all_keys = _prompt_env_keys_to_show(agent_keys, dotenv_keys)
    truncated = len(all_keys) > max_keys
    keys = all_keys[:max_keys]

    lines = [
        "## Environment variables available to you",
        "",
        "These keys exist in your agent subprocess (via the Arbos host). "
        "Values are never listed here — use them as needed; do not print secrets.",
        "",
    ]
    for k in keys:
        desc = file_desc.get(k) or _ARBOS_ENV_BUILTIN_DOC.get(k)
        if not desc:
            desc = "Present in the agent environment (no description in project env file)."
        lines.append(f"- `{k}`: {desc}")
    if truncated:
        lines.append("")
        lines.append(f"(… and {len(all_keys) - max_keys} more keys not shown.)")
    return "\n".join(lines)


def load_prompt(consume_inbox: bool = False, agent_step: int = 0) -> str:
    """Build full prompt: PROMPT.md + GOAL/STATE/INBOX + chatlog."""
    parts = []
    if PROMPT_FILE.exists():
        text = PROMPT_FILE.read_text().strip()
        if text:
            parts.append(text)
    parts.append(format_available_env_vars_section())
    if GOAL_FILE.exists():
        goal_text = GOAL_FILE.read_text().strip()
        if goal_text:
            header = f"## Loop (step {agent_step})" if agent_step else "## Loop"
            parts.append(
                f"{header}\n\n{goal_text}\n\n"
                "Your context files are under context/: STATE.md, INBOX.md, runs/, "
                "chat/, files/, tools/, workspace/ (see PROMPT.md)."
            )
    if STATE_FILE.exists():
        state_text = STATE_FILE.read_text().strip()
        if state_text:
            parts.append(f"## State\n\n{state_text}")
    if INBOX_FILE.exists():
        inbox_text = INBOX_FILE.read_text().strip()
        if inbox_text:
            parts.append(f"## Inbox\n\n{inbox_text}")
        if consume_inbox:
            INBOX_FILE.write_text("")
    chatlog = load_chatlog()
    if chatlog:
        parts.append(chatlog)
    return "\n\n".join(parts)


def make_run_dir() -> Path:
    RUNS_DIR.mkdir(parents=True, exist_ok=True)
    ts = datetime.now().strftime("%Y%m%d_%H%M%S")
    run_dir = RUNS_DIR / ts
    run_dir.mkdir(parents=True, exist_ok=True)
    return run_dir


def log_chat(role: str, text: str):
    """Append to chatlog, rolling to a new file when size exceeds limit."""
    with _chatlog_lock:
        CHATLOG_DIR.mkdir(parents=True, exist_ok=True)
        max_file_size = 4000
        max_files = 50

        existing = sorted(CHATLOG_DIR.glob("*.jsonl"))

        current: Path | None = None
        if existing and existing[-1].stat().st_size < max_file_size:
            current = existing[-1]

        if current is None:
            ts = datetime.now().strftime("%Y%m%d_%H%M%S")
            current = CHATLOG_DIR / f"{ts}.jsonl"

        entry = json.dumps({"role": role, "text": _redact_secrets(text[:1000]), "ts": datetime.now().isoformat()})
        with open(current, "a", encoding="utf-8") as f:
            f.write(entry + "\n")

        all_files = sorted(CHATLOG_DIR.glob("*.jsonl"))
        for old in all_files[:-max_files]:
            old.unlink(missing_ok=True)


def load_chatlog(max_chars: int = 8000) -> str:
    """Load recent Telegram chat history."""
    if not CHATLOG_DIR.exists():
        return ""
    files = sorted(CHATLOG_DIR.glob("*.jsonl"))
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
                return "## Recent Telegram chat\n\n" + "\n".join(lines)
            lines.append(entry)
            total += len(entry) + 1

    lines.reverse()
    if not lines:
        return ""
    return "## Recent Telegram chat\n\n" + "\n".join(lines)


# ── Step update helpers ──────────────────────────────────────────────────────

TELEGRAM_TEXT_MAX = 4096
TELEGRAM_SAFE_TEXT = 3900

TELEGRAM_HELP_TEXT = """\
Arbos:
- /loop GOAL.md
- /pause 
- /resume
- /force
- /clear
- /delay <mins>
- /restart
- /env KEY VAL DESC
"""


def _is_leading_slash_command(message) -> bool:
    """True if the message is a Telegram-style /command (entity or leading /)."""
    text = (message.text or "").strip()
    if not text.startswith("/"):
        return False
    entities = getattr(message, "entities", None) or []
    for ent in entities:
        if getattr(ent, "type", None) == "bot_command" and getattr(ent, "offset", None) == 0:
            return True
    # Some clients omit entities; leading / still means slash-command UX.
    return True


def _operator_message_thread_id() -> int | None:
    if not OPERATOR_THREAD_ID_FILE.exists():
        return None
    raw = OPERATOR_THREAD_ID_FILE.read_text().strip()
    if not raw:
        return None
    try:
        return int(raw)
    except ValueError:
        return None


def _save_operator_telegram(message: Any) -> None:
    """Persist chat id and forum topic so loop steps and HTTP sends match the operator thread."""
    CHAT_ID_FILE.parent.mkdir(parents=True, exist_ok=True)
    CHAT_ID_FILE.write_text(str(message.chat.id))
    tid = getattr(message, "message_thread_id", None)
    if tid is not None:
        OPERATOR_THREAD_ID_FILE.write_text(str(int(tid)))
    else:
        OPERATOR_THREAD_ID_FILE.unlink(missing_ok=True)


def _truncate_telegram_text(text: str, limit: int = TELEGRAM_SAFE_TEXT) -> str:
    """Trim for Telegram message body; append notice if truncated."""
    text = text or ""
    if len(text) <= limit:
        return text
    notice = f"\n\n… [truncated, {len(text)} chars total]"
    return text[: max(0, limit - len(notice))] + notice


def _telegram_send_message_fallback(
    bot: Any,
    chat_id: int,
    text: str,
    base_kw: dict[str, Any],
) -> tuple[Any, dict[str, Any]]:
    """Send a Telegram message; drop reply/thread kwargs if the API rejects the first attempt."""
    body = _truncate_telegram_text(_redact_secrets(text))[:TELEGRAM_TEXT_MAX]
    attempts: list[dict[str, Any]] = []
    attempts.append(dict(base_kw))
    no_reply = {k: v for k, v in base_kw.items() if k != "reply_to_message_id"}
    if no_reply != attempts[-1]:
        attempts.append(no_reply)
    tid = base_kw.get("message_thread_id")
    if tid is not None:
        thread_only: dict[str, Any] = {"message_thread_id": tid}
        if thread_only != attempts[-1]:
            attempts.append(thread_only)
    if attempts[-1]:
        attempts.append({})

    last_exc: Exception | None = None
    for kw in attempts:
        try:
            return bot.send_message(chat_id, body, **kw), kw
        except Exception as e:
            last_exc = e
    assert last_exc is not None
    raise last_exc


def _telegram_result_ok(data: Any) -> tuple[bool, str]:
    """Telegram often returns HTTP 200 with {\"ok\": false}."""
    if not isinstance(data, dict):
        return False, str(data)[:300]
    if data.get("ok"):
        return True, ""
    desc = str(data.get("description", data))
    return False, desc


def _streaming_empty_summary(returncode: int, stderr_output: str, attempts: int) -> str:
    """Telegram body when Claude returns no assistant text."""
    lines = [
        f"No assistant text after {attempts} attempt(s).",
        f"exit_code={returncode}",
    ]
    if stderr_output.strip():
        lines.append("--- stderr ---")
        tail = stderr_output.strip()
        lines.append(tail[-3200:] if len(tail) > 3200 else tail)
    else:
        lines.append("stderr=(empty) — check server logs for arbos.py lines.")
    return "\n".join(lines)


def _build_agent_failure_detail(result: subprocess.CompletedProcess) -> str:
    """Human-readable diagnostics when a Claude run did not succeed."""
    lines: list[str] = [f"exit_code={result.returncode}"]
    err = (result.stderr or "").strip()
    if err.startswith("(timed out") or "timed out after" in err[:120]:
        lines.append(
            "cause=idle_watchdog (no stdout/stderr for CLAUDE_TIMEOUT; "
            "set CLAUDE_TIMEOUT higher or 0 to disable)"
        )
    if err:
        lines.append("--- stderr ---")
        lines.append(err[-3500:] if len(err) > 3500 else err)
    else:
        lines.append("stderr=(empty)")
    out = (result.stdout or "").strip()
    if out:
        lines.append("--- stdout excerpt ---")
        lines.append(out[:2000] + ("…" if len(out) > 2000 else ""))
    return "\n".join(lines)


def _step_update_target() -> tuple[str, str, int | None] | None:
    token = os.getenv("TAU_BOT_TOKEN")
    if not token:
        _log("step update skipped: TAU_BOT_TOKEN not set")
        return None
    if not CHAT_ID_FILE.exists():
        _log("step update skipped: chat_id.txt not found")
        return None
    chat_id = CHAT_ID_FILE.read_text().strip()
    if not chat_id:
        _log("step update skipped: empty chat_id.txt")
        return None
    return token, chat_id, _operator_message_thread_id()


def _send_telegram_text(text: str, *, target: tuple[str, str, int | None] | None = None) -> bool:
    target = target or _step_update_target()
    if not target:
        return False
    token, chat_id, thread_id = target
    text = _redact_secrets(text)
    payload: dict[str, Any] = {"chat_id": chat_id, "text": text[:TELEGRAM_TEXT_MAX]}
    if thread_id is not None:
        payload["message_thread_id"] = thread_id
    try:
        response = requests.post(
            f"https://api.telegram.org/bot{token}/sendMessage",
            json=payload,
            timeout=15,
        )
        response.raise_for_status()
        ok, desc = _telegram_result_ok(response.json())
        if not ok:
            _log(f"telegram sendMessage API error: {desc[:300]}")
            return False
    except Exception as exc:
        _log(f"telegram send failed: {str(exc)[:120]}")
        return False
    log_chat("bot", text[:1000])
    _log("telegram message sent")
    return True


def _send_telegram_new(text: str, *, target: tuple[str, str, int | None] | None = None) -> int | None:
    """Send a new Telegram message and return its message_id."""
    target = target or _step_update_target()
    if not target:
        return None
    token, chat_id, thread_id = target
    text = _redact_secrets(text)
    payload: dict[str, Any] = {"chat_id": chat_id, "text": text[:TELEGRAM_TEXT_MAX]}
    if thread_id is not None:
        payload["message_thread_id"] = thread_id
    try:
        response = requests.post(
            f"https://api.telegram.org/bot{token}/sendMessage",
            json=payload,
            timeout=15,
        )
        response.raise_for_status()
        data = response.json()
        ok, desc = _telegram_result_ok(data)
        if not ok:
            _log(f"telegram sendMessage API error: {desc[:300]}")
            return None
        return data.get("result", {}).get("message_id")
    except Exception as exc:
        _log(f"telegram send failed: {str(exc)[:120]}")
        return None


def _edit_telegram_text(message_id: int, text: str, *, target: tuple[str, str, int | None] | None = None) -> bool:
    """Edit an existing Telegram message."""
    target = target or _step_update_target()
    if not target:
        return False
    token, chat_id, thread_id = target
    text = _redact_secrets(text)
    payload: dict[str, Any] = {
        "chat_id": chat_id,
        "message_id": message_id,
        "text": text[:TELEGRAM_TEXT_MAX],
    }
    if thread_id is not None:
        payload["message_thread_id"] = thread_id
    try:
        response = requests.post(
            f"https://api.telegram.org/bot{token}/editMessageText",
            json=payload,
            timeout=15,
        )
        response.raise_for_status()
        data = response.json()
        ok, desc = _telegram_result_ok(data)
        if ok:
            return True
        if "message is not modified" in desc.lower():
            return True
        _log(f"telegram editMessageText API error: {desc[:300]}")
        return False
    except Exception as exc:
        _log(f"telegram editMessageText request failed: {str(exc)[:200]}")
        return False


def _send_telegram_document(file_path: str, caption: str = "", *, target: tuple[str, str, int | None] | None = None) -> bool:
    """Send a file as a Telegram document."""
    target = target or _step_update_target()
    if not target:
        return False
    token, chat_id, thread_id = target
    caption = _redact_secrets(caption)[:1024]
    data: dict[str, Any] = {"chat_id": chat_id, "caption": caption}
    if thread_id is not None:
        data["message_thread_id"] = str(thread_id)
    try:
        with open(file_path, "rb") as f:
            response = requests.post(
                f"https://api.telegram.org/bot{token}/sendDocument",
                data=data,
                files={"document": (Path(file_path).name, f)},
                timeout=60,
            )
        response.raise_for_status()
        _log(f"telegram document sent: {Path(file_path).name}")
        log_chat("bot", f"[sent file: {Path(file_path).name}] {caption}")
        return True
    except Exception as exc:
        _log(f"telegram document send failed: {str(exc)[:120]}")
        return False


def _send_telegram_photo(file_path: str, caption: str = "", *, target: tuple[str, str, int | None] | None = None) -> bool:
    """Send an image as a Telegram photo (compressed)."""
    target = target or _step_update_target()
    if not target:
        return False
    token, chat_id, thread_id = target
    caption = _redact_secrets(caption)[:1024]
    data: dict[str, Any] = {"chat_id": chat_id, "caption": caption}
    if thread_id is not None:
        data["message_thread_id"] = str(thread_id)
    try:
        with open(file_path, "rb") as f:
            response = requests.post(
                f"https://api.telegram.org/bot{token}/sendPhoto",
                data=data,
                files={"photo": (Path(file_path).name, f)},
                timeout=60,
            )
        response.raise_for_status()
        _log(f"telegram photo sent: {Path(file_path).name}")
        log_chat("bot", f"[sent photo: {Path(file_path).name}] {caption}")
        return True
    except Exception as exc:
        _log(f"telegram photo send failed: {str(exc)[:120]}")
        return False


def _download_telegram_file(bot, file_id: str, filename: str) -> Path:
    """Download a file from Telegram and save it to FILES_DIR."""
    FILES_DIR.mkdir(parents=True, exist_ok=True)
    file_info = bot.get_file(file_id)
    downloaded = bot.download_file(file_info.file_path)
    save_path = FILES_DIR / filename
    # avoid overwriting: append a suffix if the file already exists
    if save_path.exists():
        stem, suffix = save_path.stem, save_path.suffix
        ts = datetime.now().strftime("%H%M%S")
        save_path = FILES_DIR / f"{stem}_{ts}{suffix}"
    save_path.write_bytes(downloaded)
    _log(f"saved telegram file: {save_path.name} ({len(downloaded)} bytes)")
    return save_path


# ── Local health HTTP (GET /health only) ─────────────────────────────────────


class _HealthHandler(BaseHTTPRequestHandler):
    def log_message(self, fmt: str, *args: Any) -> None:
        return

    def do_GET(self) -> None:
        path = self.path.split("?", 1)[0]
        if path != "/health":
            self.send_response(404)
            self.end_headers()
            return
        body = json.dumps(_operator_health_payload()).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def _start_health_server() -> None:
    server = ThreadingHTTPServer(("127.0.0.1", HEALTH_PORT), _HealthHandler)
    server.serve_forever()


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
    """Point Claude Code at OpenRouter (Anthropic-compatible API)."""
    settings_dir = WORKING_DIR / ".claude"
    settings_dir.mkdir(exist_ok=True)

    env_block = {
        "ANTHROPIC_API_KEY": LLM_API_KEY,
        "ANTHROPIC_BASE_URL": LLM_BASE_URL,
        "ANTHROPIC_AUTH_TOKEN": "",
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
    }

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
    _log(f"wrote .claude/settings.local.json (openrouter, model={CLAUDE_MODEL}, target={LLM_BASE_URL})")


def _claude_env() -> dict[str, str]:
    env = os.environ.copy()
    env.pop("TAU_BOT_TOKEN", None)
    env["ANTHROPIC_API_KEY"] = LLM_API_KEY
    env["ANTHROPIC_BASE_URL"] = LLM_BASE_URL
    env["ANTHROPIC_AUTH_TOKEN"] = ""
    return env


def _join_stream_text_chunks(parts: list[str]) -> str:
    """Join Claude stream-json text deltas without gluing sentences together.

    Sequential deltas often omit whitespace at boundaries (e.g. ``...tests.`` +
    ``Building...``). Insert a newline when sentence-ending punctuation meets a
    capital letter so context logs and Telegram stay readable; this avoids
    splitting inside words (``Build`` + ``ing``).
    """
    out: list[str] = []
    for t in parts:
        if not t:
            continue
        if out:
            prev = out[-1]
            if (
                prev
                and t
                and (not prev[-1].isspace())
                and (not t[0].isspace())
                and prev[-1] in ".!?"
                and t[0].isupper()
            ):
                out.append("\n")
        out.append(t)
    return "".join(out)


def _prefer_richer_assistant_text(
    result_text: str,
    complete_texts: list[str],
    streaming_tokens: list[str],
) -> str:
    """Pick assistant text to show after the CLI exits.

    The final stream-json ``result`` field is sometimes a short post-tool phrase
    (for example after sending a message) while the substantive answer was
    already streamed. Prefer the substantially longer assistant payload so the
    operator keeps the full reply.
    """
    r = (result_text or "").strip()
    joined_complete = "\n\n".join(complete_texts).strip()
    streamed = _join_stream_text_chunks(streaming_tokens).strip()
    candidates = [x for x in (joined_complete, streamed, r) if x]
    if not candidates:
        return result_text or ""
    longest = max(candidates, key=len)
    if len(longest) > len(r) + 80:
        return longest
    return r if r else longest


def _run_claude_once(cmd, env, on_text=None, on_activity=None):
    """Run a single claude subprocess, return (returncode, result_text, raw_lines, stderr).

    on_text: optional callback(accumulated_text) fired as assistant text streams in.
    on_activity: optional callback(status_str) fired on tool use and other activity.
    If CLAUDE_IDLE_KILL is true, kills the process after CLAUDE_TIMEOUT seconds with no
    stdout or stderr activity. Set CLAUDE_TIMEOUT=0 to disable that watchdog (still exits
    when the process ends). Stderr is drained continuously so a chatty CLI cannot deadlock
    on a full PIPE buffer.
    """
    proc = subprocess.Popen(
        cmd, cwd=WORKING_DIR, env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, bufsize=1,
    )
    with _child_procs_lock:
        _child_procs.add(proc)

    result_text = ""
    complete_texts: list[str] = []
    streaming_tokens: list[str] = []
    raw_lines: list[str] = []
    stderr_acc: list[str] = []
    timed_out = False
    last_activity = time.monotonic()
    stdout_registered = True

    sel = selectors.DefaultSelector()
    sel.register(proc.stdout, selectors.EVENT_READ)
    sel.register(proc.stderr, selectors.EVENT_READ)

    def _consume_stdout_line(line: str) -> None:
        nonlocal result_text
        raw_lines.append(line)
        try:
            evt = json.loads(line)
        except json.JSONDecodeError:
            return
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
                            on_text(_join_stream_text_chunks(streaming_tokens))
                elif btype == "tool_use" and on_activity:
                    tool_name = block.get("name", "")
                    tool_input = block.get("input", {})
                    on_activity(_format_tool_activity(tool_name, tool_input))
        elif etype == "item.completed":
            item = evt.get("item", {})
            if item.get("type") == "agent_message" and item.get("text"):
                complete_texts.append(item["text"])
                streaming_tokens.clear()
                if on_text:
                    on_text(item["text"])
        elif etype == "result":
            result_text = evt.get("result", "")

    try:
        while True:
            select_timeout = min(CLAUDE_TIMEOUT, 30) if CLAUDE_IDLE_KILL else 30.0
            ready = sel.select(timeout=select_timeout)
            if not ready:
                if CLAUDE_IDLE_KILL and (time.monotonic() - last_activity > CLAUDE_TIMEOUT):
                    _log(
                        f"claude timeout: no stdout/stderr activity for {CLAUDE_TIMEOUT}s, "
                        f"killing pid={proc.pid}"
                    )
                    proc.kill()
                    timed_out = True
                    break
                if proc.poll() is not None:
                    break
                continue

            for key, _ in ready:
                if key.fileobj == proc.stdout:
                    line = proc.stdout.readline()
                    if not line:
                        if stdout_registered:
                            try:
                                sel.unregister(proc.stdout)
                            except (KeyError, ValueError):
                                pass
                            stdout_registered = False
                    else:
                        last_activity = time.monotonic()
                        _consume_stdout_line(line)
                elif key.fileobj == proc.stderr:
                    err_line = proc.stderr.readline()
                    if err_line:
                        last_activity = time.monotonic()
                        stderr_acc.append(err_line)

            if not stdout_registered and proc.poll() is not None:
                break
    finally:
        for fo in (proc.stdout, proc.stderr):
            try:
                sel.unregister(fo)
            except (KeyError, ValueError):
                pass
        sel.close()

    if proc.stdout:
        try:
            rest = proc.stdout.read()
            if rest:
                for line in rest.splitlines(keepends=True):
                    _consume_stdout_line(line)
        except Exception:
            pass

    if proc.stderr:
        try:
            rest = proc.stderr.read()
            if rest:
                stderr_acc.append(rest)
        except Exception:
            pass

    if not result_text:
        if complete_texts:
            result_text = complete_texts[-1]
        elif streaming_tokens:
            result_text = _join_stream_text_chunks(streaming_tokens)

    result_text = _prefer_richer_assistant_text(
        result_text, complete_texts, streaming_tokens
    )

    stderr_output = "".join(stderr_acc)
    if timed_out:
        stderr_output = (
            f"(timed out after {CLAUDE_TIMEOUT}s idle)\n{stderr_output}".strip()
        )

    returncode = proc.wait()
    with _child_procs_lock:
        _child_procs.discard(proc)
    return returncode, result_text, raw_lines, stderr_output


def run_agent(cmd: list[str], phase: str, output_file: Path,
              on_text=None, on_activity=None) -> subprocess.CompletedProcess:
    env = _claude_env()
    flags = " ".join(a for a in cmd if a.startswith("-"))

    returncode, result_text, raw_lines, stderr_output = 1, "", [], "no attempts made"

    for attempt in range(1, MAX_RETRIES + 1):
        _log(f"{phase}: starting (attempt={attempt}) flags=[{flags}]")
        t0 = time.monotonic()

        returncode, result_text, raw_lines, stderr_output = _run_claude_once(
            cmd, env, on_text=on_text, on_activity=on_activity,
        )
        elapsed = time.monotonic() - t0

        output_file.write_text(_redact_secrets("".join(raw_lines)))
        _log(f"{phase}: finished rc={returncode} {fmt_duration(elapsed)}")

        if returncode != 0:
            if stderr_output.strip():
                _log(f"{phase}: stderr {stderr_output.strip()[:300]}")
            else:
                _log(f"{phase}: nonzero exit with empty stderr")
            if attempt < MAX_RETRIES:
                delay = min(2 ** attempt, 30)
                _log(f"{phase}: retrying in {delay}s (attempt {attempt}/{MAX_RETRIES})")
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


def extract_text(result: subprocess.CompletedProcess) -> str:
    out = (result.stdout or "").strip()
    if out:
        return result.stdout or ""
    err = (result.stderr or "").strip()
    if err:
        return result.stderr or ""
    return (
        f"(no stdout/stderr from agent; exit_code={result.returncode})"
    )


def _format_step_live_bubble(
    elapsed_s: float,
    step_label: str,
    rollout: str,
    tool_or_status: str,
    *,
    placeholder: str | None = None,
) -> str:
    """Single Telegram bubble while a loop step runs: header, step id line, optional tool line, rollout."""
    lines = [
        _step_response_header(elapsed_s),
        "",
        f"{step_label} (running)",
    ]
    ts = (tool_or_status or "").strip()
    if ts and ts not in ("working...",):
        lines.append(ts)
    lines.append("")
    if rollout.strip():
        lines.append(rollout.strip())
    elif placeholder:
        lines.append(placeholder)
    else:
        lines.append("(waiting for assistant text…)")
    return "\n".join(lines)


def _pre_send_normal_step_bubble(agent_step: int, global_step: int) -> int | None:
    """Send the step bubble before load_prompt so Telegram shows a new message immediately."""
    target = _step_update_target()
    if not target:
        return None
    step_label = f"Step #{agent_step}" if agent_step else f"Step #{global_step}"
    initial = _format_step_live_bubble(
        0.0, step_label, "", "", placeholder="(starting…)",
    )
    mid = _send_telegram_new(initial, target=target)
    if mid:
        STEP_MSG_FILE.parent.mkdir(parents=True, exist_ok=True)
        STEP_MSG_FILE.write_text(json.dumps({"msg_id": mid, "text": initial}))
    else:
        _log("pre_send step bubble: sendMessage returned no message_id")
    return mid


def run_step(
    prompt: str,
    step_number: int,
    agent_step: int = 0,
    *,
    force_step: bool = False,
    existing_step_msg_id: int | None = None,
) -> tuple[bool, str]:
    run_dir: Path | None = None
    t0 = time.monotonic()
    _set_prompt_context_estimate(prompt)

    smf = STEP_MSG_FILE
    use_step_msg_file = not force_step

    if force_step:
        step_label = "Step (forced)"
        _operator_set("agent_step", "forced step — Claude running")
    else:
        step_label = f"Step #{agent_step}" if agent_step else f"Step #{step_number}"

    target = _step_update_target()
    step_msg_id: int | None = None
    step_msg_text = ""
    last_edit = 0.0

    _step_initial_text = _format_step_live_bubble(
        0.0, step_label, "", "", placeholder="(starting…)",
    )
    if existing_step_msg_id is not None and not force_step:
        step_msg_id = existing_step_msg_id
        if use_step_msg_file and not smf.exists():
            smf.parent.mkdir(parents=True, exist_ok=True)
            smf.write_text(json.dumps({
                "msg_id": step_msg_id, "text": _step_initial_text,
            }))
    elif target:
        step_msg_id = _send_telegram_new(_step_initial_text, target=target)
        if step_msg_id and use_step_msg_file:
            smf.parent.mkdir(parents=True, exist_ok=True)
            smf.write_text(json.dumps({
                "msg_id": step_msg_id, "text": _step_initial_text,
            }))
    else:
        smf.unlink(missing_ok=True)

    def _persist_smf(text: str, *, new_id: int | None = None) -> None:
        if not use_step_msg_file:
            return
        mid = new_id if new_id is not None else step_msg_id
        if not mid:
            return
        smf.parent.mkdir(parents=True, exist_ok=True)
        smf.write_text(json.dumps({"msg_id": mid, "text": text}))

    def _edit_step_msg(
        text: str,
        *,
        force: bool = False,
        fallback_send: bool = False,
        min_interval: float = 3.0,
    ):
        nonlocal last_edit, step_msg_text, step_msg_id
        if not step_msg_id or not target:
            return
        now = time.time()
        if not force and now - last_edit < min_interval:
            return
        body = _truncate_telegram_text(text)
        ok = _edit_telegram_text(step_msg_id, body, target=target)
        if not ok and fallback_send:
            _log("step message edit failed; sending new Telegram message with final state")
            new_id = _send_telegram_new(
                "[step: could not edit in-place]\n\n" + body, target=target,
            )
            if new_id:
                step_msg_text = text
                _persist_smf(text, new_id=new_id)
                step_msg_id = new_id
                last_edit = now
            return
        if ok:
            step_msg_text = text
            _persist_smf(text)
            last_edit = now

    _last_activity = [""]
    _rollout_buf = [""]
    _heartbeat_stop = threading.Event()

    def _on_text(streaming: str):
        _operator_tick()
        _rollout_buf[0] = streaming
        elapsed_s = time.monotonic() - t0
        bubble = _format_step_live_bubble(
            elapsed_s,
            step_label,
            _redact_secrets(streaming),
            _last_activity[0] or "",
        )
        _edit_step_msg(bubble, min_interval=1.0)

    def _on_activity(status: str):
        _operator_tick()
        _last_activity[0] = status
        elapsed_s = time.monotonic() - t0
        bubble = _format_step_live_bubble(
            elapsed_s,
            step_label,
            _redact_secrets(_rollout_buf[0]),
            status,
        )
        _edit_step_msg(bubble, min_interval=1.2)

    def _heartbeat():
        while not _heartbeat_stop.wait(timeout=3.0):
            _operator_tick()
            elapsed_s = time.monotonic() - t0
            status = _last_activity[0] or "working..."
            bubble = _format_step_live_bubble(
                elapsed_s,
                step_label,
                _redact_secrets(_rollout_buf[0]),
                status if status != "working..." else "",
            )
            _edit_step_msg(bubble, force=True)

    if existing_step_msg_id is not None and step_msg_id:
        _edit_step_msg(
            _format_step_live_bubble(
                0.0, step_label, "", "", placeholder="(starting…)",
            ),
            force=True,
        )

    run_dir = make_run_dir()
    log_file = run_dir / "logs.txt"
    _tls.log_fh = open(log_file, "a", encoding="utf-8")

    success = False
    result: subprocess.CompletedProcess | None = None
    failure_summary = ""
    try:
        _log(f"run dir {run_dir}")

        preview = prompt[:200] + ("…" if len(prompt) > 200 else "")
        _log(f"prompt preview: {preview}")

        _log(f"agent step {agent_step}: executing" + (" [force]" if force_step else ""))

        threading.Thread(target=_heartbeat, daemon=True).start()

        try:
            result = run_agent(
                _claude_cmd(prompt),
                phase="agent_step",
                output_file=run_dir / "output.txt",
                on_text=_on_text,
                on_activity=_on_activity,
            )
        except Exception as exc:
            failure_summary = _redact_secrets(f"{type(exc).__name__}: {exc}")[:800]
            _log(f"run_step: run_agent raised: {failure_summary}")
            return False, failure_summary

        rollout_text = _redact_secrets(extract_text(result))
        (run_dir / "rollout.md").write_text(rollout_text)
        _log(f"rollout saved ({len(rollout_text)} chars)")

        elapsed = time.monotonic() - t0
        success = result.returncode == 0
        _log(f"step {'succeeded' if success else 'failed'} in {fmt_duration(elapsed)}")
        if not success and result is not None:
            failure_summary = _redact_secrets(_build_agent_failure_detail(result))[:800]
            _log(
                "step failure detail:\n"
                + _redact_secrets(_build_agent_failure_detail(result))[:4000]
            )
        else:
            failure_summary = ""
        return success, failure_summary
    finally:
        _heartbeat_stop.set()
        fh = getattr(_tls, "log_fh", None)
        if fh:
            fh.close()
            _tls.log_fh = None
        try:
            rollout = ""
            if run_dir is not None and (run_dir / "rollout.md").exists():
                rollout = (run_dir / "rollout.md").read_text()
            status = "done" if success else "failed"
            goal_text = GOAL_FILE.read_text().strip() if GOAL_FILE.exists() else ""
            state_text = STATE_FILE.read_text().strip() if STATE_FILE.exists() else ""
            inbox_text = INBOX_FILE.read_text().strip() if INBOX_FILE.exists() else ""
            go_text = GO_FLAG_FILE.read_text().strip() if GO_FLAG_FILE.exists() else ""

            agent_text = ""
            if smf.exists():
                try:
                    state = json.loads(smf.read_text())
                    saved = state.get("text", "")
                    if (
                        saved != _step_initial_text
                        and not saved.startswith("Step ( ")
                        and not saved.startswith(f"{step_label} (running)")
                    ):
                        agent_text = saved
                except (json.JSONDecodeError, KeyError):
                    pass

            elapsed_s = time.monotonic() - t0
            hdr = _step_response_header(elapsed_s)
            if not success and result is not None:
                hdr = f"{hdr} — FAILED (exit {result.returncode})"
            else:
                hdr = f"{hdr} — {status}"
            parts = [hdr]
            log_tail = ""
            log_path = (run_dir / "logs.txt") if run_dir is not None else None
            if log_path and log_path.exists():
                raw_log = log_path.read_text(errors="replace").strip()
                if raw_log:
                    log_tail = raw_log[-2500:] if len(raw_log) > 2500 else raw_log

            stdout_text = (result.stdout or "").strip() if result is not None else ""
            failure_detail = (
                _redact_secrets(_build_agent_failure_detail(result))
                if (not success and result is not None)
                else ""
            )
            summary = _summarize_step_outcome(
                step_label=step_label,
                success=success,
                elapsed_s=elapsed_s,
                goal_text=goal_text,
                state_text=state_text,
                inbox_text=inbox_text,
                go_text=go_text,
                rollout_text=rollout,
                log_tail=log_tail,
                stdout_text=stdout_text,
                failure_detail=failure_detail,
            )
            has_summary = bool(summary)
            if has_summary:
                parts.append(summary)
            elif agent_text:
                parts.append(agent_text)
            if (not has_summary) and not success and result is not None:
                if stdout_text:
                    parts.append("--- model text (stdout) ---\n" + stdout_text[:2000])
                parts.append(failure_detail[:2800])
            elif (not has_summary) and rollout.strip():
                parts.append(rollout.strip()[:3500])
            if (not has_summary) and log_tail:
                parts.append("--- step log (tail) ---\n" + _redact_secrets(log_tail))
            final = _truncate_telegram_text("\n\n".join(parts))

            _edit_step_msg(final, force=True, fallback_send=True)
            log_chat("bot", final[:1000])
            smf.unlink(missing_ok=True)
        except Exception as exc:
            _log(f"step message finalize failed: {str(exc)[:120]}")
            tgt = _step_update_target()
            if tgt:
                _send_telegram_text(
                    f"{step_label}: finalize/crash in run_step: {str(exc)[:500]}",
                    target=tgt,
                )


# ── Agent loop ───────────────────────────────────────────────────────────────


def _agent_wait(gs: AgentState, timeout: float) -> None:
    """Wait up to timeout seconds or until gs.wake; refresh operator ticks during long sleeps."""
    if timeout <= 0:
        gs.wake.clear()
        return
    end = time.monotonic() + timeout
    while not gs.stop_event.is_set():
        remaining = end - time.monotonic()
        if remaining <= 0:
            break
        slice_sec = min(remaining, 10.0)
        if gs.wake.wait(timeout=slice_sec):
            break
        _operator_tick()
    gs.wake.clear()


def _agent_loop():
    """Run the agent loop. Exits when stop_event is set."""
    global _step_count, _pending_step_msg_id

    with _agent_lock:
        gs = _agent

    failures = 0

    while not gs.stop_event.is_set():
        if not GOAL_FILE.exists() or not GOAL_FILE.read_text().strip():
            if gs.goal_hash:
                _log(f"goal cleared after {gs.step_count} steps")
                gs.goal_hash = ""
                gs.step_count = 0
            with _agent_lock:
                if gs.paused:
                    gs.paused = False
                    _save_agent()
            _operator_set("idle", "waiting for context/GOAL.md content")
            _agent_wait(gs, 5.0)
            continue

        if not GO_FLAG_FILE.exists():
            _operator_set(
                "paused",
                "paused — no context/GO.md (/resume or /loop to enable steps; GOAL.md unchanged)",
            )
            with _agent_lock:
                if not gs.paused:
                    gs.paused = True
                    _save_agent()
            _agent_wait(gs, 5.0)
            continue

        with _agent_lock:
            if gs.paused:
                gs.paused = False
                _save_agent()

        current_goal = GOAL_FILE.read_text().strip()
        current_hash = hashlib.sha256(current_goal.encode()).hexdigest()[:16]
        if current_hash != gs.goal_hash:
            if gs.goal_hash:
                _log(f"goal changed after {gs.step_count} steps on previous text")
            gs.goal_hash = current_hash
            gs.step_count = 0
            _log(f"goal new [{current_hash}]: {current_goal[:100]}")

        _step_count += 1
        gs.step_count += 1
        gs.last_run = datetime.now().isoformat()
        with _agent_lock:
            _save_agent()

        _log(f"Loop step {gs.step_count} (global step {_step_count})", blank=True)

        with _pending_step_msg_lock:
            pre_id = _pending_step_msg_id
            _pending_step_msg_id = None
        if pre_id is None:
            pre_id = _pre_send_normal_step_bubble(gs.step_count, _step_count)

        prompt = load_prompt(consume_inbox=True, agent_step=gs.step_count)
        if not prompt:
            _operator_set("idle", "empty prompt; waiting")
            if pre_id:
                tgt = _step_update_target()
                if tgt:
                    _edit_telegram_text(
                        pre_id,
                        _truncate_telegram_text(
                            "Step skipped: empty prompt. Add GOAL/STATE or use /loop."
                        ),
                        target=tgt,
                    )
                STEP_MSG_FILE.unlink(missing_ok=True)
            _agent_wait(gs, 5.0)
            continue

        _log(f"agent: prompt={len(prompt)} chars")

        _operator_set("agent_step", f"step {gs.step_count} — Claude running")
        success, failure_summary = run_step(
            prompt,
            _step_count,
            agent_step=gs.step_count,
            existing_step_msg_id=pre_id,
        )

        gs.last_finished = datetime.now().isoformat()
        with _agent_lock:
            _save_agent()

        if success:
            failures = 0
            gs.last_step_ok = True
            gs.last_step_error = ""
            _operator_set(
                "between_steps",
                f"step {gs.step_count} finished OK",
                last_error="",
            )
        else:
            failures += 1
            gs.last_step_ok = False
            gs.last_step_error = (failure_summary or "step failed (no detail)")[:1200]
            _log(f"agent: failure #{failures}")

        gs.wake.clear()

        delay_secs = gs.delay_minutes * 60 + int(os.environ.get("AGENT_DELAY", "0"))
        if failures:
            backoff = min(2 ** failures, 120)
            delay_secs += backoff
            _log(f"agent: waiting {delay_secs}s (failure backoff + delay)")
            _operator_set(
                "between_steps",
                f"waiting {delay_secs}s (backoff + delay) then next step",
                last_error=gs.last_step_error,
            )
            _agent_wait(gs, float(delay_secs))
        elif delay_secs > 0:
            _log(f"agent: waiting {delay_secs}s (delay)")
            _operator_set(
                "between_steps",
                f"waiting {delay_secs}s before next step",
            )
            _agent_wait(gs, float(delay_secs))
        else:
            _operator_tick()

    _log("agent loop exited")


def _ensure_agent_thread() -> None:
    """Spawn the agent loop thread if started but not running; clear stale dead threads.

    Call after /loop or /resume so we do not wait for the 2s _agent_manager poll.
    """
    with _agent_lock:
        gs = _agent
        if gs.thread is not None and not gs.thread.is_alive():
            gs.thread = None
        if gs.started and gs.thread is None:
            gs.stop_event.clear()
            t = threading.Thread(target=_agent_loop, daemon=True, name="agent")
            gs.thread = t
            t.start()
            _log("agent thread spawned (ensure)")


def _agent_manager():
    """Spawn or stop the single agent thread based on AgentState."""
    while not _shutdown.is_set():
        with _agent_lock:
            gs = _agent
            if not gs.started and gs.thread is not None:
                gs.stop_event.set()
                gs.wake.set()
        _ensure_agent_thread()
        _shutdown.wait(timeout=2)


def _summarize_goal(text: str) -> str:
    """Generate a one-line summary of a goal via OpenRouter. Falls back to truncation."""
    try:
        url = f"{LLM_BASE_URL}/v1/chat/completions"
        headers = {"Authorization": f"Bearer {LLM_API_KEY}", "Content-Type": "application/json"}

        resp = requests.post(url, json={
            "model": CLAUDE_MODEL,
            "max_tokens": 50,
            "messages": [
                {"role": "system", "content": "Summarize the user's goal in 8 words or fewer. Reply with ONLY the summary."},
                {"role": "user", "content": text[:500]},
            ],
        }, headers=headers, timeout=30)

        if resp.status_code == 200:
            data = resp.json()
            choices = data.get("choices", [])
            if choices:
                summary = choices[0].get("message", {}).get("content", "").strip().strip('"\'.')
                if summary:
                    return summary[:80]
    except Exception as exc:
        _log(f"summarize failed: {str(exc)[:100]}")

    first_line = text[:60].split('\n')[0].strip()
    return first_line + ("..." if len(text) > 60 else "")


def _openrouter_chat_text(
    messages: list[dict[str, str]],
    *,
    max_tokens: int,
    timeout: int = 45,
) -> str:
    """Send a direct OpenRouter chat request and return the text reply."""
    if not LLM_API_KEY:
        return ""
    try:
        response = requests.post(
            f"{LLM_BASE_URL}/v1/chat/completions",
            json={
                "model": CLAUDE_MODEL,
                "max_tokens": max_tokens,
                "messages": messages,
            },
            headers={
                "Authorization": f"Bearer {LLM_API_KEY}",
                "Content-Type": "application/json",
            },
            timeout=timeout,
        )
        if response.status_code != 200:
            _log(
                "openrouter chat failed: "
                f"status={response.status_code} body={response.text[:200]}"
            )
            return ""
        data = response.json()
        choices = data.get("choices", [])
        if not choices:
            return ""
        content = choices[0].get("message", {}).get("content", "")
        return _redact_secrets((content or "").strip())
    except Exception as exc:
        _log(f"openrouter chat failed: {str(exc)[:120]}")
        return ""


def _clip_summary_context(text: str, max_chars: int) -> str:
    text = (text or "").strip()
    if not text:
        return ""
    if len(text) <= max_chars:
        return text
    return text[:max_chars].rstrip() + "\n...[truncated]..."


def _summarize_step_outcome(
    *,
    step_label: str,
    success: bool,
    elapsed_s: float,
    goal_text: str,
    state_text: str,
    inbox_text: str,
    go_text: str,
    rollout_text: str,
    log_tail: str,
    stdout_text: str = "",
    failure_detail: str = "",
) -> str:
    """Create a concise operator-facing summary of the completed step."""
    status = "success" if success else "failure"
    _log(f"step summary: requesting OpenRouter summary for {step_label} ({status})")
    packet_parts = [
        f"Step label: {step_label}",
        f"Status: {status}",
        f"Elapsed seconds: {elapsed_s:.1f}",
    ]
    if goal_text.strip():
        packet_parts.append("## GOAL.md\n" + _clip_summary_context(goal_text, 2500))
    if state_text.strip():
        packet_parts.append("## STATE.md\n" + _clip_summary_context(state_text, 2500))
    if inbox_text.strip():
        packet_parts.append("## INBOX.md\n" + _clip_summary_context(inbox_text, 1200))
    if go_text.strip():
        packet_parts.append("## GO.md\n" + _clip_summary_context(go_text, 600))
    if rollout_text.strip():
        packet_parts.append("## rollout.md\n" + _clip_summary_context(rollout_text, 5000))
    if stdout_text.strip():
        packet_parts.append("## stdout\n" + _clip_summary_context(stdout_text, 2000))
    if failure_detail.strip():
        packet_parts.append(
            "## failure detail\n" + _clip_summary_context(failure_detail, 3000)
        )
    if log_tail.strip():
        packet_parts.append("## logs tail\n" + _clip_summary_context(log_tail, 3500))

    user_packet = _redact_secrets("\n\n".join(packet_parts))
    summary = _openrouter_chat_text(
        [
            {
                "role": "system",
                "content": (
                    "You are summarizing a just-finished Arbos loop step for the operator. "
                    "Summarize only what actually happened from the supplied artifacts. "
                    "Be concrete about files or state changes, mention failures or blockers if present, "
                    "and include the most important next action only if it is clearly implied. "
                    "Reply in plain text, concise, no markdown headings, at most 6 short lines."
                ),
            },
            {"role": "user", "content": user_packet},
        ],
        max_tokens=220,
    )
    if summary:
        _log(f"step summary: received {len(summary)} chars for {step_label}")
    else:
        _log(f"step summary: no summary returned for {step_label}")
    return summary


def transcribe_voice(file_path: str, fmt: str = "ogg") -> str:
    """Voice notes are not transcribed (no STT backend configured)."""
    return "(voice notes are not supported — send text instead)"


# ── Telegram bot ─────────────────────────────────────────────────────────────

def _recent_context(max_chars: int = 6000) -> str:
    """Collect recent rollouts under context/runs/."""
    parts: list[str] = []
    total = 0
    all_runs: list[tuple[str, Path]] = []
    if RUNS_DIR.exists():
        for d in RUNS_DIR.iterdir():
            if d.is_dir():
                all_runs.append((d.name, d))
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
        if total > max_chars:
            break
    return "".join(parts)


def _telegram_message_excerpt(msg: Any, max_len: int = 2000) -> str:
    """Best-effort text from a Telegram Message-like object (telebot; duck-typed)."""
    if msg is None:
        return ""
    t = (getattr(msg, "text", None) or "").strip()
    if t:
        return (t[:max_len] + "…") if len(t) > max_len else t
    cap = (getattr(msg, "caption", None) or "").strip()
    if cap:
        return (cap[:max_len] + "…") if len(cap) > max_len else cap
    if getattr(msg, "document", None):
        doc = msg.document
        fn = getattr(doc, "file_name", None) or "file"
        return f"[Document: {fn}]"
    if getattr(msg, "photo", None):
        return "[Photo]"
    if getattr(msg, "voice", None) or getattr(msg, "audio", None):
        return "[Voice or audio message]"
    return "[Message without inline text]"


def _operator_telegram_reply_nudge(
    current_from_user_id: int | None, parent_msg: Any
) -> str:
    """Section for the operator prompt when Telegram reply-to is used."""
    pu = parent_msg.from_user.id if getattr(parent_msg, "from_user", None) else None
    if current_from_user_id is not None and pu == current_from_user_id:
        who = "the operator's own earlier message in this chat"
    else:
        who = "a previous Arbos (assistant) message in this chat"
    excerpt = _telegram_message_excerpt(parent_msg)
    return (
        "## Telegram reply context\n\n"
        "The operator used **Telegram's reply** to reference a specific bubble. They are "
        f"responding to **{who}**, quoted below. The **Operator message** section at the "
        "end is their **new** text; treat it as a follow-up to the quoted message when "
        "that is the natural reading.\n\n"
        f"**Quoted message:**\n{excerpt}"
    )


def _telegram_reply_context_for_prompt(message: Any) -> str | None:
    parent = getattr(message, "reply_to_message", None)
    if parent is None:
        return None
    uid = message.from_user.id if getattr(message, "from_user", None) else None
    return _operator_telegram_reply_nudge(uid, parent)


def _build_operator_prompt(user_text: str, *, reply_context: str | None = None) -> str:
    """Build prompt for the CLI agent to handle any operator request."""
    chatlog = load_chatlog(max_chars=4000)

    parts = [
        "You are the operator interface for Arbos, a coding agent running in a loop via pm2.\n"
        "The operator communicates with you through Telegram. Be concise and direct.\n"
        "When the operator asks you to do something, do it by modifying the relevant files.\n"
        "When the operator asks a question, answer from the available context.\n\n"
        "## Security\n\n"
        "NEVER read, output, or reveal the contents of `.env`, `.env.enc`, or any secret/key/token values.\n"
        "Do not include API keys, passwords, seed phrases, or credentials in any response.\n"
        "If asked to show secrets, refuse. The .env file is encrypted; do not attempt to decrypt it.\n\n"
        f"{format_available_env_vars_section()}\n\n"
        "## Single agent loop\n\n"
        "One agent loop uses flat files under `context/`: GOAL.md, GO.md, STATE.md, INBOX.md, and `context/runs/<timestamp>/`.\n"
        "- **GOAL.md**: loop instructions (set by /loop).\n"
        "- **GO.md**: run flag — must exist for steps to execute. /loop and /resume create it; /pause deletes it.\n"
        "Telegram: /loop, /pause, /resume, /force, /clear, /delay (see /help).\n"
        "- **Message the agent**: append a timestamped line to `context/INBOX.md`.\n"
        "- **Update agent state**: write to `context/STATE.md`.\n"
        "- **Set system prompt**: write to `PROMPT.md`.\n"
        "- **Set env variable**: write `KEY='VALUE'` lines (one per line) to `context/.env.pending`. They are picked up automatically and persisted.\n"
        "- **View logs**: read files in `context/runs/<timestamp>/` (rollout.md, logs.txt).\n"
        "- **Modify code & restart**: edit code files, then run `touch .restart`.\n"
        "- **Send follow-up**: run `python arbos.py send \"your text here\"`.\n"
        "- **Send file to operator**: run `python arbos.py sendfile path/to/file [--caption 'text'] [--photo]`.\n"
        "- **Received files**: operator-sent files are saved in `context/files/` and their path is shown in the message.",
    ]

    with _agent_lock:
        gs = _agent
        status = _agent_status_label(gs)
        delay_note = f"{gs.delay_minutes}m between steps" if gs.delay_minutes else "no delay between steps"
        goal_text = GOAL_FILE.read_text().strip()[:200] if GOAL_FILE.exists() else "(empty)"
        go_line = "yes (loop may run steps)" if GO_FLAG_FILE.exists() else "no (paused — create GO.md or /resume)"
        state_text = STATE_FILE.read_text().strip()[:200] if STATE_FILE.exists() else "(empty)"
        parts.append(
            f"## Agent [{status}] ({delay_note}, step {gs.step_count})\n"
            f"Current goal (GOAL.md): {goal_text}\n"
            f"Run flag (GO.md): {go_line}\n"
            f"State (STATE.md): {state_text}"
        )

    if chatlog:
        parts.append(chatlog)

    context = _recent_context(max_chars=4000)
    if context:
        parts.append(f"## Recent activity\n{context}")
    if reply_context:
        parts.append(reply_context)
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


def run_agent_streaming(
    bot,
    prompt: str,
    chat_id: int,
    *,
    reply_to_message_id: int | None = None,
    message_thread_id: int | None = None,
) -> str:
    """Run Claude Code CLI and stream output into Telegram.

    The *active* segment is updated with ``editMessageText``. When the CLI
    starts a new assistant segment (non-prefix text), the previous segment is
    left frozen in its bubble and a new message is sent so the chat shows the
    full rollout (each bubble up to Telegram's size limit).

    When ``reply_to_message_id`` is set, new Telegram messages (including the
    initial status bubble) are sent as replies to that operator message.

    When ``message_thread_id`` is set (forum supergroup topic), it is passed on
    every ``sendMessage`` so standalone operator messages appear in the same
    topic. Without it, Telegram posts to the General topic while replies would
    still land in the topic of the quoted message.
    """
    cmd = _claude_cmd(prompt)

    t0 = time.monotonic()
    _set_prompt_context_estimate(prompt)

    def _elapsed() -> float:
        return time.monotonic() - t0

    def _format_display(core: str) -> str:
        core = (core or "").strip() or "…"
        return _truncate_telegram_text(
            f"{_arbos_response_header(_elapsed())}\n\n{core}"
        )

    _reply_kw: dict[str, Any] = {}
    if reply_to_message_id is not None:
        _reply_kw["reply_to_message_id"] = reply_to_message_id
    if message_thread_id is not None:
        _reply_kw["message_thread_id"] = message_thread_id

    current_text = ""
    _start_core = f"Starting Claude… (attempt 1/{MAX_RETRIES})"
    _phase_line = _start_core
    _segment_done: list[str] = []
    _stream_seg: str = ""
    _stream_tail: str = ""

    def _stream_core() -> str:
        chunks = []
        if _stream_seg.strip():
            chunks.append(_stream_seg.strip())
        if _stream_tail.strip():
            chunks.append(_stream_tail.strip())
        return "\n\n".join(chunks) if chunks else ""

    def _display_core() -> str:
        core = _stream_core()
        if core:
            return core
        return (_phase_line or "").strip() or "…"

    def _final_transcript() -> str:
        chunks = [s.strip() for s in _segment_done if s.strip()]
        if _stream_seg.strip():
            chunks.append(_stream_seg.strip())
        return "\n\n".join(chunks) if chunks else ""

    try:
        msg, used_kw = _telegram_send_message_fallback(
            bot, chat_id, _format_display(_start_core), _reply_kw,
        )
        if used_kw != _reply_kw:
            _log(
                "run_agent_streaming: initial send used relaxed Telegram kwargs "
                f"{list(used_kw.keys()) or '(none)'}"
            )
    except Exception as exc:
        _log(f"run_agent_streaming: initial send_message failed: {str(exc)[:250]}")
        notice = (
            "Arbos could not open a status message on Telegram.\n\n"
            f"{exc}\n\n"
            "Your text was still received. If this repeats, check DNS/network to api.telegram.org."
        )
        try:
            _telegram_send_message_fallback(bot, chat_id, notice, {})
        except Exception as exc2:
            _log(
                "run_agent_streaming: could not notify chat of Telegram failure: "
                f"{str(exc2)[:220]}"
            )
        return f"(could not post operator status to Telegram: {exc})"

    run_dir = make_run_dir()
    _tls.log_fh = open(run_dir / "logs.txt", "a", encoding="utf-8")
    _log(f"operator run dir {run_dir}")
    _pp = _redact_secrets(prompt[:200] + ("…" if len(prompt) > 200 else ""))
    _log(f"operator prompt preview: {_pp}")
    _rto = reply_to_message_id
    _tid = message_thread_id
    _log(
        f"operator meta: model={CLAUDE_MODEL} chat_id={chat_id}"
        + (f" reply_to_message_id={_rto}" if _rto is not None else "")
        + (f" message_thread_id={_tid}" if _tid is not None else "")
    )

    last_edit = 0.0
    _heartbeat_stop = threading.Event()
    last_raw_lines: list[str] = []
    last_attempt = 1
    last_rc, last_stderr = 0, ""

    def _stream_heartbeat():
        while not _heartbeat_stop.wait(timeout=3.0):
            _operator_tick()
            _paint(force=True, refresh_only=True)

    def _paint(
        force: bool = False,
        *,
        refresh_only: bool = False,
        send_if_edit_fails: bool = False,
    ):
        nonlocal last_edit, msg
        now = time.time()
        if not force and not refresh_only and now - last_edit < 1.5:
            return
        display = _redact_secrets(_format_display(_display_core()))
        if not display.strip():
            return
        try:
            bot.edit_message_text(display, chat_id, msg.message_id)
            last_edit = now
        except Exception as exc:
            _log(f"run_agent_streaming: edit_message_text failed: {str(exc)[:220]}")
            if send_if_edit_fails:
                try:
                    bot.send_message(
                        chat_id, display[:TELEGRAM_TEXT_MAX], **_reply_kw
                    )
                    _log("run_agent_streaming: sent fallback new message after edit failure")
                except Exception as exc2:
                    _log(f"run_agent_streaming: fallback send_message failed: {str(exc2)[:220]}")

    def _freeze_segment_and_new_message(completed: str, new_seg_raw: str) -> None:
        """Leave *completed* in the current bubble; open a new message for *new_seg_raw*."""
        nonlocal msg
        c = (completed or "").strip()
        if c:
            body = _redact_secrets(_format_display(c))
            try:
                bot.edit_message_text(body, chat_id, msg.message_id)
            except Exception as exc:
                _log(f"run_agent_streaming: freeze-segment edit failed: {str(exc)[:220]}")
                try:
                    bot.send_message(
                        chat_id, body[:TELEGRAM_TEXT_MAX], **_reply_kw
                    )
                    _log("run_agent_streaming: freeze segment sent as new message after edit fail")
                except Exception as exc2:
                    _log(
                        f"run_agent_streaming: freeze segment send_message failed: "
                        f"{str(exc2)[:220]}"
                    )
            _segment_done.append(c)
        core_new = (new_seg_raw or "").strip() or "…"
        try:
            msg = bot.send_message(
                chat_id,
                _redact_secrets(_format_display(core_new)),
                **_reply_kw,
            )
        except Exception as exc:
            _log(f"run_agent_streaming: new-segment send_message failed: {str(exc)[:250]}")

    def _on_text(text: str):
        nonlocal _stream_seg, _stream_tail
        t = text or ""
        pt = _stream_seg.strip()
        tt = t.strip()
        _stream_tail = ""
        if not pt:
            _stream_seg = t
        elif tt.startswith(pt):
            _stream_seg = t
        else:
            _freeze_segment_and_new_message(pt, t)
            _stream_seg = t
        _paint()

    def _on_activity(status: str):
        nonlocal _stream_tail
        _operator_tick()
        _stream_tail = (status or "").strip()
        _paint()

    with _operator_lock:
        _prev_operator_phase = _operator["phase"]
        _prev_operator_detail = _operator["detail"]
    _operator_set("operator_chat", "Telegram /operator — Claude streaming")

    try:
        threading.Thread(
            target=_stream_heartbeat, daemon=True, name="operator-stream-hb",
        ).start()

        env = _claude_env()

        for attempt in range(1, MAX_RETRIES + 1):
            last_attempt = attempt
            current_text = ""
            last_edit = 0.0
            _segment_done.clear()
            _stream_seg = ""
            _stream_tail = ""
            _phase_line = "Thinking ..."
            _paint(force=True)
            _log(f"run_agent_streaming: attempt {attempt}/{MAX_RETRIES} starting")
            t_attempt = time.monotonic()

            returncode, result_text, raw_lines, stderr_output = _run_claude_once(
                cmd, env, on_text=_on_text, on_activity=_on_activity,
            )
            last_raw_lines = raw_lines
            last_rc, last_stderr = returncode, stderr_output
            attempt_s = time.monotonic() - t_attempt

            _log(
                f"run_agent_streaming: attempt {attempt} finished rc={returncode} "
                f"duration_s={attempt_s:.2f} raw_lines={len(raw_lines)} "
                f"text_len={len(result_text or '')} stderr_len={len(stderr_output or '')}"
            )
            _se = (stderr_output or "").strip()
            if _se:
                _log(
                    "run_agent_streaming: attempt "
                    f"{attempt} stderr preview: {_redact_secrets(_se)[:800]}"
                )
            elif returncode != 0:
                _log(
                    f"run_agent_streaming: attempt {attempt} rc={returncode} "
                    "with empty stderr"
                )

            tr = _final_transcript().strip()
            rt = (result_text or "").strip()
            if tr or rt:
                if tr:
                    current_text = _final_transcript()
                else:
                    current_text = rt
                    _stream_tail = ""
                    _stream_seg = result_text or rt
                break

            if attempt < MAX_RETRIES:
                delay = min(2 ** attempt, 30)
                detail = _streaming_empty_summary(returncode, stderr_output, attempt)
                _segment_done.clear()
                _stream_seg = ""
                _stream_tail = ""
                _phase_line = (
                    f"{detail}\n\nRetrying in {delay}s "
                    f"(next {attempt + 1}/{MAX_RETRIES})…"
                )
                _paint(force=True)
                time.sleep(delay)
                continue

            current_text = _streaming_empty_summary(returncode, stderr_output, attempt)
            _segment_done.clear()
            _stream_seg = ""
            _stream_tail = ""
            _phase_line = current_text
            break

        _paint(force=True, send_if_edit_fails=True)

        if not current_text.strip():
            fallback = _streaming_empty_summary(last_rc, last_stderr, last_attempt)
            _log(f"run_agent_streaming: final still empty; pushing diagnostic len={len(fallback)}")
            try:
                bot.send_message(
                    chat_id,
                    _redact_secrets(_format_display(fallback))[:TELEGRAM_TEXT_MAX],
                    **_reply_kw,
                )
            except Exception as exc:
                _log(f"run_agent_streaming: could not send final diagnostic: {str(exc)[:200]}")

    except Exception as e:
        current_text = f"(operator failed: {type(e).__name__}: {e})"
        _log(f"run_agent_streaming: exception: {str(e)[:500]}")
        _operator_set(
            "operator_chat_error",
            "Telegram operator run crashed",
            last_error=f"{type(e).__name__}: {e}"[:800],
        )
        err_body = _truncate_telegram_text(
            f"{_arbos_response_header(_elapsed())}\n\n"
            f"Arbos error (operator run):\n{type(e).__name__}: {e}"
        )
        try:
            bot.edit_message_text(_redact_secrets(err_body), chat_id, msg.message_id)
        except Exception as exc:
            _log(f"run_agent_streaming: could not edit with error text: {str(exc)[:200]}")
            try:
                bot.send_message(
                    chat_id,
                    _redact_secrets(err_body)[:TELEGRAM_TEXT_MAX],
                    **_reply_kw,
                )
            except Exception as exc2:
                _log(f"run_agent_streaming: could not send error message: {str(exc2)[:200]}")
    finally:
        try:
            (run_dir / "output.txt").write_text(
                _redact_secrets("".join(last_raw_lines))
            )
            rbody = (current_text or "").strip()
            if not rbody:
                rbody = _redact_secrets(
                    _streaming_empty_summary(last_rc, last_stderr, last_attempt)
                )
            else:
                rbody = _redact_secrets(rbody)
            (run_dir / "rollout.md").write_text(rbody)
            _log(
                f"operator rollout saved ({len(rbody)} chars) "
                f"total_elapsed={fmt_duration(_elapsed())} last_rc={last_rc} "
                f"attempts_used={last_attempt}"
            )
        except Exception as exc:
            _log(f"operator run artifact save failed: {str(exc)[:120]}")
        _heartbeat_stop.set()
        with _operator_lock:
            _operator["phase"] = _prev_operator_phase
            _operator["detail"] = _prev_operator_detail
            _operator["last_tick_wall"] = time.time()
        fh = getattr(_tls, "log_fh", None)
        if fh:
            try:
                fh.close()
            except OSError:
                pass
            _tls.log_fh = None

    return current_text


def _is_owner(user_id: int) -> bool:
    owner = os.environ.get("TELEGRAM_OWNER_ID", "").strip()
    if not owner:
        return False
    return str(user_id) == owner


def _enroll_owner(user_id: int):
    """Auto-enroll the first /start user as the owner and persist."""
    owner_id = str(user_id)
    os.environ["TELEGRAM_OWNER_ID"] = owner_id
    env_path = WORKING_DIR / ".env"
    if env_path.exists():
        existing = env_path.read_text()
        if "TELEGRAM_OWNER_ID" not in existing:
            with open(env_path, "a") as f:
                f.write(f"\nTELEGRAM_OWNER_ID='{owner_id}'\n")
    elif ENV_ENC_FILE.exists():
        _save_to_encrypted_env("TELEGRAM_OWNER_ID", owner_id)
    _log(f"enrolled owner: {owner_id}")


def run_bot():
    """Run the Telegram bot."""
    token = os.getenv("TAU_BOT_TOKEN")
    if not token:
        _log("TAU_BOT_TOKEN not set; add it to .env and restart")
        sys.exit(1)

    import telebot

    class _TelegramNetworkHandler(telebot.ExceptionHandler):
        """Treat DNS/network failures as handled so threaded polling backs off instead of raising."""

        def handle(self, exception):
            if isinstance(
                exception,
                (requests.exceptions.ConnectionError, requests.exceptions.Timeout),
            ):
                _log(
                    "Telegram API unreachable (network/DNS); backing off "
                    f"({type(exception).__name__})"
                )
                return True
            return False

    bot = telebot.TeleBot(token, exception_handler=_TelegramNetworkHandler())

    def _reply(message, text: str, **kwargs):
        """Send *text* as a Telegram reply to the user's message."""
        send_kw: dict[str, Any] = {
            "reply_to_message_id": message.message_id,
        }
        tid = getattr(message, "message_thread_id", None)
        if tid is not None:
            send_kw["message_thread_id"] = tid
        send_kw.update(kwargs)
        return bot.send_message(message.chat.id, text, **send_kw)

    def _reject(message):
        uid = message.from_user.id if message.from_user else None
        _log(f"rejected message from unauthorized user {uid}")
        if not os.environ.get("TELEGRAM_OWNER_ID", "").strip():
            _reply(message, "Send /start to register as the owner.")
        else:
            _reply(message, "Unauthorized.")

    @bot.message_handler(commands=["start"])
    def handle_start(message):
        uid = message.from_user.id if message.from_user else None
        if not os.environ.get("TELEGRAM_OWNER_ID", "").strip() and uid is not None:
            _enroll_owner(uid)
        if not _is_owner(uid):
            _reject(message)
            return
        _save_operator_telegram(message)
        _reply(
            message,
            _truncate_telegram_text(TELEGRAM_HELP_TEXT.strip()),
        )

    @bot.message_handler(commands=["help"])
    def handle_help(message):
        _reply(
            message,
            _truncate_telegram_text(TELEGRAM_HELP_TEXT.strip()),
        )

    @bot.message_handler(commands=["status"])
    def handle_status(message):
        uid = message.from_user.id if message.from_user else None
        if not _is_owner(uid):
            _reject(message)
            return
        _save_operator_telegram(message)
        hp = _operator_health_payload()
        op = hp["operator"]
        head = [
            f"Arbos {hp['status']} | uptime {hp['uptime_seconds']}s",
            f"Now: {op['phase']} — {op['detail']}",
            f"Activity clock: {op['seconds_since_activity']}s since last update "
            f"(refreshes while Claude runs, during backoff waits, etc.)",
        ]
        if op.get("last_error"):
            head.append(f"Last error (global): {op['last_error'][:450]}")
        if hp.get("degraded_reason"):
            head.append(f"Degraded: {hp['degraded_reason']}")
        banner = "\n".join(head) + "\n\n"

        with _agent_lock:
            gs = _agent
        status = _agent_status_label(gs)
        goal_text = GOAL_FILE.read_text().strip()[:500] if GOAL_FILE.exists() else "(empty)"
        state_text = STATE_FILE.read_text().strip()[:500] if STATE_FILE.exists() else "(empty)"
        if gs.last_step_ok is None:
            step_out = "no step completed yet"
        elif gs.last_step_ok:
            step_out = "last step OK"
        else:
            step_out = "last step FAILED"
        lines = [
            banner.rstrip(),
            f"Agent [{status}] (delay: {gs.delay_minutes}m, step {gs.step_count})",
            step_out,
            f"Last run: {gs.last_run or 'never'}",
            f"Last finished: {gs.last_finished or 'never'}",
        ]
        if gs.last_step_error:
            lines.append(f"Last step error:\n{gs.last_step_error[:900]}")
        lines.extend([
            "",
            f"Loop: {goal_text}",
            f"Run flag: {'GO.md present' if GO_FLAG_FILE.exists() else 'GO.md absent'}",
            "",
            f"State: {state_text}",
            "",
            f"Total steps: {_step_count}",
            f"HTTP GET http://127.0.0.1:{HEALTH_PORT}/health for JSON.",
        ])
        _reply(message, "\n".join(lines))

    @bot.message_handler(commands=["pause"])
    def handle_pause(message):
        uid = message.from_user.id if message.from_user else None
        if not _is_owner(uid):
            _reject(message)
            return
        _save_operator_telegram(message)
        if not GO_FLAG_FILE.exists():
            _reply(message, "Already paused (no GO.md).")
            return
        GO_FLAG_FILE.unlink(missing_ok=True)
        with _agent_lock:
            gs = _agent
            gs.paused = True
            _save_agent()
        gs.wake.set()
        _reply(
            message,
            "Paused (removed context/GO.md). GOAL.md unchanged. /resume to run again.",
        )
        _log("agent paused via /pause (GO.md removed)")

    @bot.message_handler(commands=["resume"])
    def handle_resume(message):
        uid = message.from_user.id if message.from_user else None
        if not _is_owner(uid):
            _reject(message)
            return
        _save_operator_telegram(message)
        if not GOAL_FILE.exists() or not GOAL_FILE.read_text().strip():
            _reply(message, "No goal in GOAL.md — use /loop <goal> first.")
            return
        if GO_FLAG_FILE.exists():
            _reply(message, "Already going (GO.md already present).")
            return
        _write_go_flag()
        with _agent_lock:
            gs = _agent
            gs.stop_event.clear()
            gs.paused = False
            gs.started = True
            _save_agent()
        gs.wake.set()
        _ensure_agent_thread()
        _reply(message, "Resumed (created context/GO.md).")
        _log("agent resumed via /resume (GO.md created)")

    @bot.message_handler(commands=["force"])
    def handle_force(message):
        uid = message.from_user.id if message.from_user else None
        if not _is_owner(uid):
            _reject(message)
            return
        _save_operator_telegram(message)
        if not GOAL_FILE.exists() or not GOAL_FILE.read_text().strip():
            _reply(message, "No goal in GOAL.md — use /loop <goal> first.")
            return
        with _agent_lock:
            gs = _agent
            astep = gs.step_count if gs.step_count > 0 else 1
        prompt = load_prompt(consume_inbox=False, agent_step=astep)
        if not prompt.strip():
            _reply(message, "Prompt is empty.")
            return
        _reply(
            message,
            "Starting a forced step in the background — watch for a new "
            "**Step (forced)** bubble that streams the rollout.",
        )

        def _run_force_step():
            try:
                run_step(prompt, 0, agent_step=astep, force_step=True)
            except Exception as exc:
                _log(f"/force step crashed: {type(exc).__name__}: {exc!s}")

        threading.Thread(target=_run_force_step, daemon=True, name="force-step").start()

    @bot.message_handler(commands=["delay"])
    def handle_delay(message):
        uid = message.from_user.id if message.from_user else None
        if not _is_owner(uid):
            _reject(message)
            return
        _save_operator_telegram(message)
        args = (message.text or "").split()
        if len(args) < 2:
            _reply(message, "Usage: /delay <minutes>")
            return
        try:
            minutes = int(args[1])
        except ValueError:
            _reply(message, "Usage: /delay <minutes> (integer)")
            return
        if minutes < 0:
            _reply(message, "Delay must be >= 0.")
            return
        with _agent_lock:
            _agent.delay_minutes = minutes
            _save_agent()
        _reply(message, f"Delay set to {minutes} minute(s) between successful steps.")
        _log(f"delay set to {minutes}m via /delay")

    @bot.message_handler(commands=["env"])
    def handle_env(message):
        uid = message.from_user.id if message.from_user else None
        if not _is_owner(uid):
            _reject(message)
            return
        _save_operator_telegram(message)
        parts = (message.text or "").split(None, 3)
        if len(parts) < 4:
            _reply(
                message,
                "Usage: /env KEY VALUE DESCRIPTION\n"
                "VALUE must be one word (no spaces). Description can be several words.",
            )
            return
        _, key, value, description = parts
        ok, msg = _persist_env_var_with_comment(key, value, description)
        _reply(message, msg if ok else f"Error: {msg}")
        if ok:
            _log(f"/env persisted key={key!r}")
            try:
                bot.delete_message(message.chat.id, message.message_id)
            except Exception as exc:
                _log(f"/env: could not delete operator message (token may remain in chat): {exc!r}")

    @bot.message_handler(commands=["loop"])
    def handle_loop(message):
        global _pending_step_msg_id
        uid = message.from_user.id if message.from_user else None
        if not _is_owner(uid):
            _reject(message)
            return
        _save_operator_telegram(message)
        text = (message.text or "").split(None, 1)
        if len(text) < 2 or not text[1].strip():
            _reply(message, "Usage: /loop GOAL")
            return
        goal_text = text[1].strip()
        msg = _reply(message, "Starting loop...")
        CONTEXT_DIR.mkdir(parents=True, exist_ok=True)
        RUNS_DIR.mkdir(parents=True, exist_ok=True)
        GOAL_FILE.write_text(goal_text)
        _write_go_flag()
        if not STATE_FILE.exists():
            STATE_FILE.write_text("")
        if not INBOX_FILE.exists():
            INBOX_FILE.write_text("")
        with _agent_lock:
            next_global = _step_count + 1
        loop_pre_id = _pre_send_normal_step_bubble(1, next_global)
        with _pending_step_msg_lock:
            _pending_step_msg_id = loop_pre_id
        with _agent_lock:
            gs = _agent
            gs.stop_event.clear()
            gs.summary = (goal_text[:80] + ("…" if len(goal_text) > 80 else "")) or "…"
            gs.goal_hash = ""
            gs.started = True
            gs.paused = False
            _save_agent()
        gs.wake.set()
        _ensure_agent_thread()
        summary = _summarize_goal(goal_text)
        with _agent_lock:
            gs = _agent
            gs.summary = summary
            _save_agent()
        edit_kw: dict[str, Any] = {}
        _tid = getattr(message, "message_thread_id", None)
        if _tid is not None:
            edit_kw["message_thread_id"] = _tid
        bot.edit_message_text(
            f"Loop set: {summary}\nRunning (GO.md created; /pause removes it).",
            message.chat.id, msg.message_id,
            **edit_kw,
        )
        _log(f"loop set ({len(goal_text)} chars), auto-start: {summary}")

    @bot.message_handler(commands=["clear"])
    def handle_clear(message):
        global _pending_step_msg_id
        uid = message.from_user.id if message.from_user else None
        if not _is_owner(uid):
            _reject(message)
            return
        _save_operator_telegram(message)
        import shutil
        with _agent_lock:
            gs = _agent
            gs.stop_event.set()
            gs.wake.set()
            thread = gs.thread
            gs.started = False
            gs.paused = False
            gs.summary = ""
            gs.delay_minutes = 0
            gs.step_count = 0
            gs.goal_hash = ""
            gs.last_run = ""
            gs.last_finished = ""
            gs.last_step_ok = None
            gs.last_step_error = ""
        if thread and thread.is_alive():
            thread.join(timeout=8)
        with _agent_lock:
            gs.stop_event.clear()
            gs.thread = None
            _save_agent()
        STEP_MSG_FILE.unlink(missing_ok=True)
        with _pending_step_msg_lock:
            _pending_step_msg_id = None
        for path in (GOAL_FILE, GO_FLAG_FILE, STATE_FILE, INBOX_FILE):
            path.unlink(missing_ok=True)
        if RUNS_DIR.exists():
            shutil.rmtree(RUNS_DIR, ignore_errors=True)
        RUNS_DIR.mkdir(parents=True, exist_ok=True)
        _reply(
            message,
            "Loop cleared (GOAL/GO/STATE/INBOX/runs reset). Use /loop to start again.",
        )
        _log("goal cleared via /clear")

    @bot.message_handler(commands=["restart"])
    def handle_restart(message):
        uid = message.from_user.id if message.from_user else None
        if not _is_owner(uid):
            _reject(message)
            return
        _save_operator_telegram(message)
        _reply(message, "Restarting ...")
        _log("restart requested via /restart command")
        _kill_child_procs()
        RESTART_FLAG.touch()

    @bot.message_handler(commands=["update"])
    def handle_update(message):
        uid = message.from_user.id if message.from_user else None
        if not _is_owner(uid):
            _reject(message)
            return
        _save_operator_telegram(message)
        msg = _reply(message, "Pulling latest changes...")
        try:
            r = subprocess.run(
                ["git", "pull", "--ff-only"],
                cwd=WORKING_DIR, capture_output=True, text=True, timeout=30,
            )
            output = (r.stdout.strip() + "\n" + r.stderr.strip()).strip()
            if r.returncode != 0:
                bot.edit_message_text(f"Git pull failed:\n{output[:3800]}", message.chat.id, msg.message_id)
                _log(f"update failed: {output[:200]}")
                return
            bot.edit_message_text(f"Pulled:\n{output[:3800]}\n\nRestarting...", message.chat.id, msg.message_id)
            _log(f"update pulled: {output[:200]}")
        except Exception as exc:
            bot.edit_message_text(f"Git pull error: {str(exc)[:3800]}", message.chat.id, msg.message_id)
            _log(f"update error: {str(exc)[:200]}")
            return
        _kill_child_procs()
        RESTART_FLAG.touch()

    @bot.message_handler(content_types=["voice", "audio"])
    def handle_voice(message):
        uid = message.from_user.id if message.from_user else None
        if not _is_owner(uid):
            _reject(message)
            return
        _save_operator_telegram(message)
        _reply(message, "Transcribing voice note...")

        voice_or_audio = message.voice or message.audio
        file_info = bot.get_file(voice_or_audio.file_id)
        downloaded = bot.download_file(file_info.file_path)

        ext = file_info.file_path.rsplit(".", 1)[-1] if "." in file_info.file_path else "ogg"
        tmp_path = WORKING_DIR / f"_voice_tmp.{ext}"
        tmp_path.write_bytes(downloaded)

        try:
            transcript = transcribe_voice(str(tmp_path), fmt=ext)
        finally:
            tmp_path.unlink(missing_ok=True)

        caption = message.caption or ""
        user_text = f"[Voice note transcription]: {transcript}"
        if caption:
            user_text += f"\n[Caption]: {caption}"

        log_chat("user", user_text[:1000])
        prompt = _build_operator_prompt(
            user_text,
            reply_context=_telegram_reply_context_for_prompt(message),
        )

        def _run():
            response = run_agent_streaming(
                bot,
                prompt,
                message.chat.id,
                reply_to_message_id=message.message_id,
                message_thread_id=getattr(message, "message_thread_id", None),
            )
            log_chat("bot", response[:1000])
            _process_pending_env()

        threading.Thread(target=_run, daemon=True).start()

    @bot.message_handler(content_types=["document"])
    def handle_document(message):
        uid = message.from_user.id if message.from_user else None
        if not _is_owner(uid):
            _reject(message)
            return
        _save_operator_telegram(message)

        doc = message.document
        filename = doc.file_name or f"file_{doc.file_id[:8]}"
        saved_path = _download_telegram_file(bot, doc.file_id, filename)

        caption = message.caption or ""
        size_kb = doc.file_size / 1024 if doc.file_size else saved_path.stat().st_size / 1024
        user_text = f"[Sent file: {saved_path.name}] saved to {saved_path} ({size_kb:.1f} KB)"
        if caption:
            user_text += f"\n[Caption]: {caption}"

        is_text = False
        try:
            content = saved_path.read_text(errors="strict")
            if len(content) <= 8000:
                user_text += f"\n[File contents]:\n{content}"
                is_text = True
        except (UnicodeDecodeError, ValueError):
            pass

        if not is_text:
            user_text += "\n(Binary file — not included inline. Read it from the saved path if needed.)"

        log_chat("user", user_text[:1000])
        prompt = _build_operator_prompt(
            user_text,
            reply_context=_telegram_reply_context_for_prompt(message),
        )

        def _run():
            response = run_agent_streaming(
                bot,
                prompt,
                message.chat.id,
                reply_to_message_id=message.message_id,
                message_thread_id=getattr(message, "message_thread_id", None),
            )
            log_chat("bot", response[:1000])
            _process_pending_env()

        threading.Thread(target=_run, daemon=True).start()

    @bot.message_handler(content_types=["photo"])
    def handle_photo(message):
        uid = message.from_user.id if message.from_user else None
        if not _is_owner(uid):
            _reject(message)
            return
        _save_operator_telegram(message)

        photo = message.photo[-1]  # highest resolution
        ts = datetime.now().strftime("%Y%m%d_%H%M%S")
        filename = f"photo_{ts}.jpg"
        saved_path = _download_telegram_file(bot, photo.file_id, filename)

        caption = message.caption or ""
        user_text = f"[Sent photo: {saved_path.name}] saved to {saved_path}"
        if caption:
            user_text += f"\n[Caption]: {caption}"

        log_chat("user", user_text[:1000])
        prompt = _build_operator_prompt(
            user_text,
            reply_context=_telegram_reply_context_for_prompt(message),
        )

        def _run():
            response = run_agent_streaming(
                bot,
                prompt,
                message.chat.id,
                reply_to_message_id=message.message_id,
                message_thread_id=getattr(message, "message_thread_id", None),
            )
            log_chat("bot", response[:1000])
            _process_pending_env()

        threading.Thread(target=_run, daemon=True).start()

    @bot.message_handler(func=lambda m: True)
    def handle_message(message):
        uid = message.from_user.id if message.from_user else None
        if not _is_owner(uid):
            _reject(message)
            return
        if _is_leading_slash_command(message):
            _reply(
                message,
                _truncate_telegram_text(TELEGRAM_HELP_TEXT.strip()),
            )
            return
        _save_operator_telegram(message)
        raw_text = (message.text or "").strip()
        if not raw_text:
            _reply(message, "Send a non-empty text message.")
            return
        log_chat("user", raw_text)
        prompt = _build_operator_prompt(
            raw_text,
            reply_context=_telegram_reply_context_for_prompt(message),
        )

        def _run():
            response = run_agent_streaming(
                bot,
                prompt,
                message.chat.id,
                reply_to_message_id=message.message_id,
                message_thread_id=getattr(message, "message_thread_id", None),
            )
            log_chat("bot", response[:1000])
            _process_pending_env()

        threading.Thread(target=_run, daemon=True).start()

    _log("telegram bot started")
    while True:
        try:
            # None disables TeleBot's internal error/traceback spam (its ERROR>=DEBUG check is wrong).
            bot.infinity_polling(logger_level=None)
        except Exception as e:
            _log(f"bot polling error: {str(e)[:80]}, reconnecting in 5s")
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

    Within a step, all sends are consolidated into a single Telegram message.
    The first send creates it; subsequent sends edit it by appending.
    Uses context/.step_msg for the active step bubble.
    """
    import argparse
    parser = argparse.ArgumentParser(description="Send a Telegram message to the operator")
    parser.add_argument("message", nargs="?", help="Message text to send")
    parser.add_argument("--file", help="Send contents of a file instead")
    parsed = parser.parse_args(args)

    if not parsed.message and not parsed.file:
        parser.error("Provide a message or --file")

    if parsed.file:
        text = Path(parsed.file).read_text()
    else:
        text = parsed.message

    smf = STEP_MSG_FILE
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
        if _edit_telegram_text(msg_id, combined):
            smf.write_text(json.dumps({"msg_id": msg_id, "text": combined}))
            log_chat("bot", combined[:1000])
            print(f"Edited step message ({len(combined)} chars)")
        else:
            new_id = _send_telegram_new(text)
            if new_id:
                smf.write_text(json.dumps({"msg_id": new_id, "text": text}))
                log_chat("bot", text[:1000])
                print(f"Sent new message ({len(text)} chars)")
            else:
                print("Failed to send", file=sys.stderr)
                sys.exit(1)
    else:
        new_id = _send_telegram_new(text)
        if new_id:
            smf.write_text(json.dumps({"msg_id": new_id, "text": text}))
            log_chat("bot", text[:1000])
            print(f"Sent ({len(text)} chars)")
        else:
            print("Failed to send (check TAU_BOT_TOKEN and chat_id.txt)", file=sys.stderr)
            sys.exit(1)


def _sendfile_cli(args: list[str]):
    """CLI entry point: python arbos.py sendfile path/to/file [--caption 'text'] [--photo]"""
    import argparse
    parser = argparse.ArgumentParser(description="Send a file to the operator via Telegram")
    parser.add_argument("path", help="Path to the file to send")
    parser.add_argument("--caption", default="", help="Caption for the file")
    parser.add_argument("--photo", action="store_true", help="Send as a compressed photo instead of a document")
    parsed = parser.parse_args(args)

    file_path = Path(parsed.path)
    if not file_path.exists():
        print(f"File not found: {file_path}", file=sys.stderr)
        sys.exit(1)

    if parsed.photo:
        ok = _send_telegram_photo(str(file_path), caption=parsed.caption)
    else:
        ok = _send_telegram_document(str(file_path), caption=parsed.caption)

    if ok:
        print(f"Sent {'photo' if parsed.photo else 'file'}: {file_path.name}")
    else:
        print("Failed to send (check TAU_BOT_TOKEN and chat_id.txt)", file=sys.stderr)
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
        bot_token = os.environ.get("TAU_BOT_TOKEN", "")
        if not bot_token:
            print("TAU_BOT_TOKEN must be set in .env", file=sys.stderr)
            sys.exit(1)
        _encrypt_env_file(bot_token)
        print("Encrypted .env → .env.enc, deleted plaintext.")
        print(f"On future starts: TAU_BOT_TOKEN='{bot_token}' python arbos.py")
        return

    if len(sys.argv) > 1 and sys.argv[1] not in ("send", "encrypt", "sendfile"):
        print(f"Unknown subcommand: {sys.argv[1]}", file=sys.stderr)
        print("Usage: arbos.py [send|sendfile|encrypt]", file=sys.stderr)
        sys.exit(1)

    global _arbos_boot_wall
    _arbos_boot_wall = time.time()
    _operator_set("supervising", "Arbos starting (health HTTP, agent loop, Telegram)")

    _log(f"arbos starting in {WORKING_DIR} (openrouter, model={CLAUDE_MODEL})")
    _kill_stale_claude_procs()
    _reload_env_secrets()
    CONTEXT_DIR.mkdir(parents=True, exist_ok=True)
    RUNS_DIR.mkdir(parents=True, exist_ok=True)
    CONTEXT_LOGS_DIR.mkdir(parents=True, exist_ok=True)
    CHATLOG_DIR.mkdir(parents=True, exist_ok=True)
    FILES_DIR.mkdir(parents=True, exist_ok=True)

    _load_agent()
    _log("loaded agent state from meta.json (if present)")

    if not LLM_API_KEY:
        _log("WARNING: OPENROUTER_API_KEY not set — LLM calls will fail")

    def _handle_sigterm(signum, frame):
        _log("SIGTERM received; shutting down gracefully")
        _shutdown.set()

    signal.signal(signal.SIGTERM, _handle_sigterm)

    _log(f"starting health server on 127.0.0.1:{HEALTH_PORT}/health")
    threading.Thread(target=_start_health_server, daemon=True, name="health-http").start()

    _write_claude_settings()

    _send_telegram_text(_truncate_telegram_text(TELEGRAM_HELP_TEXT.strip()))

    threading.Thread(target=_agent_manager, daemon=True).start()
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
