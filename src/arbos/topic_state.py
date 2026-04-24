"""Per-topic state for the multi-topic agent.

Each forum topic the agent listens on (its home topic, plus any other topic
in the same supergroup where the bot has been @-mentioned) gets a small
state document at ``~/.arbos/topics/<topic_id>/state.json``::

    {
      "topic_id": 12345,
      "workspace": "/Users/const/workspace",
      "adopted_at": 1714000000.0,
      "last_seen_at": 1714000123.0,
      "title": "templar"
    }

The default workspace is the user's home directory (``~``); ``/workspace
<path>`` (handled in ``agent.py``) overwrites the ``workspace`` field for
the topic the command is sent in. The presence of a ``state.json`` is what
flags a topic as "adopted" -- the agent loads every adopted topic at boot
into its routing set and starts answering messages there immediately.

Atomic writes via tmp+os.replace, mirroring ``session_store.py``.
"""

from __future__ import annotations

import json
import logging
import os
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Optional

from .workspace import InstallPaths

logger = logging.getLogger(__name__)


@dataclass
class TopicState:
    """One adopted topic's persistent state.

    Three name components are tracked separately so the agent can render
    a stable ``<name> · <base> · <workspace>`` Telegram topic title:

    * ``name``       -- the user-given topic name (captured from a
      ``forum_topic_created`` service message, or set explicitly later).
    * ``base_title`` -- a locked prefix the agent owns; defaults to
      this machine's name (e.g. ``"arbos"``).
    * ``workspace``  -- the cursor-agent ``--workspace`` for this topic.

    ``title`` caches the most recent name the agent pushed via
    ``editForumTopic`` so it can detect no-op renames.
    """

    topic_id: int
    workspace: str
    adopted_at: float
    last_seen_at: float
    title: Optional[str] = None
    base_title: Optional[str] = None
    name: Optional[str] = None

    @classmethod
    def fresh(
        cls,
        topic_id: int,
        *,
        workspace: Optional[str] = None,
        title: Optional[str] = None,
        base_title: Optional[str] = None,
        name: Optional[str] = None,
    ) -> "TopicState":
        now = time.time()
        return cls(
            topic_id=int(topic_id),
            workspace=workspace or str(Path.home()),
            adopted_at=now,
            last_seen_at=now,
            title=title,
            base_title=base_title,
            name=name,
        )


def _atomic_write(path: Path, payload: str) -> None:
    """Write ``payload`` to ``path`` atomically with 0600 perms."""
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
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
    os.replace(tmp, path)
    try:
        os.chmod(path, 0o600)
    except OSError:
        pass


def load(paths: InstallPaths, topic_id: int) -> Optional[TopicState]:
    """Read the per-topic state, or ``None`` if missing/corrupt."""
    p = paths.topic_state_path(topic_id)
    if not p.exists():
        return None
    try:
        raw = json.loads(p.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        logger.warning("ignoring corrupt %s: %s", p, exc)
        return None
    if not isinstance(raw, dict):
        return None
    try:
        return TopicState(
            topic_id=int(raw.get("topic_id") or topic_id),
            workspace=str(raw.get("workspace") or str(Path.home())),
            adopted_at=float(raw.get("adopted_at") or 0.0),
            last_seen_at=float(raw.get("last_seen_at") or 0.0),
            title=(raw.get("title") if isinstance(raw.get("title"), str) else None),
            base_title=(
                raw.get("base_title") if isinstance(raw.get("base_title"), str) else None
            ),
            name=(raw.get("name") if isinstance(raw.get("name"), str) else None),
        )
    except (TypeError, ValueError) as exc:
        logger.warning("malformed topic state %s: %s", p, exc)
        return None


def save(paths: InstallPaths, state: TopicState) -> None:
    """Persist ``state`` atomically with restrictive perms."""
    paths.bootstrap_topic(state.topic_id)
    payload = json.dumps(asdict(state), indent=2, sort_keys=True)
    _atomic_write(paths.topic_state_path(state.topic_id), payload)


def list_adopted(paths: InstallPaths) -> list[int]:
    """Return all adopted topic ids by scanning ``topics/`` for state files."""
    out: list[int] = []
    if not paths.topics_dir.exists():
        return out
    try:
        entries = list(paths.topics_dir.iterdir())
    except OSError as exc:
        logger.warning("could not list %s: %s", paths.topics_dir, exc)
        return out
    for entry in entries:
        if not entry.is_dir():
            continue
        try:
            tid = int(entry.name)
        except ValueError:
            continue
        if (entry / "state.json").exists():
            out.append(tid)
    out.sort()
    return out


def is_adopted(paths: InstallPaths, topic_id: int) -> bool:
    return paths.topic_state_path(topic_id).exists()


def adopt(
    paths: InstallPaths,
    topic_id: int,
    *,
    workspace: Optional[str] = None,
    title: Optional[str] = None,
    base_title: Optional[str] = None,
    name: Optional[str] = None,
) -> TopicState:
    """Idempotently mark ``topic_id`` as adopted, creating fresh state if needed.

    If state already exists, refreshes ``last_seen_at`` and fills in
    ``base_title`` / ``title`` / ``name`` only when they're currently
    unset (so we never overwrite a previously locked prefix, a
    freshly-rendered cached name, or a user-set topic name). Returns
    the resulting :class:`TopicState`.
    """
    topic_id = int(topic_id)
    existing = load(paths, topic_id)
    if existing is None:
        state = TopicState.fresh(
            topic_id,
            workspace=workspace,
            title=title,
            base_title=base_title,
            name=name,
        )
        save(paths, state)
        logger.info(
            "adopted topic %s (workspace=%s, name=%r, base_title=%r)",
            topic_id, state.workspace, state.name, state.base_title,
        )
        return state

    changed = False
    now = time.time()
    if existing.last_seen_at < now:
        existing.last_seen_at = now
        changed = True
    if title and not existing.title:
        existing.title = title
        changed = True
    if base_title and not existing.base_title:
        existing.base_title = base_title
        changed = True
    if name and not existing.name:
        existing.name = name
        changed = True
    if changed:
        save(paths, existing)
    return existing


def set_name(paths: InstallPaths, topic_id: int, name: str) -> Optional[TopicState]:
    """Update the user-facing ``name`` component for a topic.

    Returns the updated state, or ``None`` if the topic is not yet
    adopted (caller should adopt first). No-op when the value is
    unchanged.
    """
    state = load(paths, topic_id)
    if state is None:
        return None
    cleaned = name.strip()
    if not cleaned or state.name == cleaned:
        return state
    state.name = cleaned
    state.last_seen_at = time.time()
    save(paths, state)
    return state


def touch(paths: InstallPaths, topic_id: int) -> None:
    """Bump ``last_seen_at`` for an already-adopted topic. No-op if absent."""
    state = load(paths, topic_id)
    if state is None:
        return
    state.last_seen_at = time.time()
    try:
        save(paths, state)
    except OSError as exc:
        logger.warning("topic_state.touch failed for %s: %s", topic_id, exc)


def set_workspace(paths: InstallPaths, topic_id: int, workspace: str) -> TopicState:
    """Update the per-topic workspace path. Adopts the topic if absent."""
    state = load(paths, topic_id) or TopicState.fresh(int(topic_id))
    state.workspace = workspace
    state.last_seen_at = time.time()
    save(paths, state)
    return state


def workspace_for(paths: InstallPaths, topic_id: int) -> Path:
    """Resolve the cursor-agent ``--workspace`` for a topic.

    Falls back to ``Path.home()`` when the topic has no state yet
    (shouldn't happen post-adopt, but keeps callers total).
    """
    state = load(paths, topic_id)
    if state is None or not state.workspace:
        return Path.home()
    return Path(state.workspace).expanduser()
