"""Persisted per-topic cursor-agent chat session(s).

Every adopted forum topic keeps two parallel persistent ``cursor-agent``
chats (each resumed via ``--resume <id>`` on each spawn):

* the **worker** chat (``chat_session.json``) -- the heavy coding thread
  the user actually has long-form conversations with;
* the **router** chat (``router_session.json``) -- the SmartRouter's own
  thread, used purely for one-shot routing decisions (which slash command
  to emit / whether to delegate). Kept separate so neither side pollutes
  the other's history.

Both ids and a little bookkeeping live in ``~/.arbos/topics/<topic_id>/``.

The store is intentionally tiny + sync: one small JSON read at agent boot
per topic, one atomic write whenever the id changes (which is rare --
only on first spawn after adoption or after a ``/reset``). Atomic write =
``tmp`` file + ``os.replace`` so a crash mid-write can never produce a
half-written id.
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


def _load_from(p) -> Optional[ChatSession]:
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


def _save_to(paths: InstallPaths, topic_id: int, p, session: ChatSession) -> None:
    paths.bootstrap_topic(topic_id)
    tmp = p.with_suffix(p.suffix + ".tmp")
    payload = json.dumps(asdict(session), indent=2)
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


def _clear_at(p) -> bool:
    try:
        p.unlink()
        return True
    except FileNotFoundError:
        return False
    except OSError as exc:
        logger.warning("could not unlink %s: %s", p, exc)
        return False


def load(paths: InstallPaths, topic_id: int) -> Optional[ChatSession]:
    """Read the persisted worker chat session for ``topic_id``, or ``None``."""
    return _load_from(paths.topic_chat_session_path(topic_id))


def save(paths: InstallPaths, topic_id: int, session: ChatSession) -> None:
    """Atomically persist ``session`` to the topic's worker chat session file (0600)."""
    _save_to(paths, topic_id, paths.topic_chat_session_path(topic_id), session)


def clear(paths: InstallPaths, topic_id: int) -> bool:
    """Remove the persisted worker chat session for ``topic_id``. Returns True if removed."""
    return _clear_at(paths.topic_chat_session_path(topic_id))


def load_router(paths: InstallPaths, topic_id: int) -> Optional[ChatSession]:
    """Read the persisted SmartRouter chat session for ``topic_id``, or ``None``."""
    return _load_from(paths.topic_router_session_path(topic_id))


def save_router(paths: InstallPaths, topic_id: int, session: ChatSession) -> None:
    """Atomically persist ``session`` to the topic's router session file (0600)."""
    _save_to(paths, topic_id, paths.topic_router_session_path(topic_id), session)


def clear_router(paths: InstallPaths, topic_id: int) -> bool:
    """Remove the persisted router session for ``topic_id``. Returns True if removed."""
    return _clear_at(paths.topic_router_session_path(topic_id))
