"""Persisted single-chat session for the per-machine cursor-agent.

Arbos keeps every Telegram message in this machine's forum topic inside a
single, persistent ``cursor-agent`` chat (resumed via ``--resume <id>`` on
each spawn). The chat id and a little bookkeeping live in
``<install-root>/.arbos/chat_session.json``.

The store is intentionally tiny + sync: one small JSON read at agent boot,
one atomic write whenever the id changes (which is rarely — only on first
spawn after install or after a ``/reset``). Atomic write = ``tmp`` file +
``os.replace`` so a crash mid-write can never produce a half-written id.
"""

from __future__ import annotations

import json
import logging
import os
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from typing import Optional

from .workspace import InstallPaths

logger = logging.getLogger(__name__)


@dataclass
class ChatSession:
    session_id: str
    updated_at: str
    model: Optional[str] = None

    @classmethod
    def now(cls, session_id: str, *, model: Optional[str] = None) -> "ChatSession":
        return cls(
            session_id=session_id,
            updated_at=datetime.now(timezone.utc).isoformat(timespec="seconds"),
            model=model,
        )


def load(paths: InstallPaths) -> Optional[ChatSession]:
    """Read the persisted chat session, or ``None`` if missing/corrupt."""
    p = paths.chat_session_path
    if not p.exists():
        return None
    try:
        raw = json.loads(p.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        logger.warning("ignoring corrupt %s: %s", p, exc)
        return None
    if not isinstance(raw, dict):
        return None
    sid = raw.get("session_id")
    if not isinstance(sid, str) or not sid:
        return None
    return ChatSession(
        session_id=sid,
        updated_at=str(raw.get("updated_at") or ""),
        model=raw.get("model") if isinstance(raw.get("model"), str) else None,
    )


def save(paths: InstallPaths, session: ChatSession) -> None:
    """Atomically persist ``session`` to ``chat_session.json`` (0600)."""
    p = paths.chat_session_path
    p.parent.mkdir(parents=True, exist_ok=True)
    tmp = p.with_suffix(p.suffix + ".tmp")
    payload = json.dumps(asdict(session), indent=2)
    # Open with restrictive perms from the start so we never briefly expose
    # the file world-readable on a shared filesystem.
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "w") as fh:
            fh.write(payload)
    except Exception:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise
    os.replace(tmp, p)
    try:
        os.chmod(p, 0o600)
    except OSError:
        pass


def clear(paths: InstallPaths) -> bool:
    """Remove the persisted session. Returns ``True`` if a file was removed."""
    p = paths.chat_session_path
    try:
        p.unlink()
        return True
    except FileNotFoundError:
        return False
    except OSError as exc:
        logger.warning("could not unlink %s: %s", p, exc)
        return False
