"""Persistent install state machine.

Each phase commits its terminal state via :meth:`StateStore.set` so reruns of
``arbos install`` can pick up at the last successful boundary.
"""

from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from enum import Enum
from pathlib import Path
from typing import Any


class InstallState(str, Enum):
    INIT = "INIT"
    ARBOS_AUTH_PENDING = "ARBOS_AUTH_PENDING"
    ARBOS_AUTH_COMPLETE = "ARBOS_AUTH_COMPLETE"
    BOTFATHER_PENDING = "BOTFATHER_PENDING"
    BOT_CREATED = "BOT_CREATED"
    BOT_PRIVACY_OFF = "BOT_PRIVACY_OFF"
    SUPERGROUP_CREATED = "SUPERGROUP_CREATED"
    FORUM_ENABLED = "FORUM_ENABLED"
    BOT_ADDED = "BOT_ADDED"
    TOPICS_CREATED = "TOPICS_CREATED"
    COMPLETE = "COMPLETE"
    FAILED_RECOVERABLE = "FAILED_RECOVERABLE"
    FAILED_FATAL = "FAILED_FATAL"


_ORDER: dict[InstallState, int] = {
    InstallState.INIT: 0,
    InstallState.ARBOS_AUTH_PENDING: 1,
    InstallState.ARBOS_AUTH_COMPLETE: 2,
    InstallState.BOTFATHER_PENDING: 3,
    InstallState.BOT_CREATED: 4,
    InstallState.BOT_PRIVACY_OFF: 5,
    InstallState.SUPERGROUP_CREATED: 6,
    InstallState.FORUM_ENABLED: 7,
    InstallState.BOT_ADDED: 8,
    InstallState.TOPICS_CREATED: 9,
    InstallState.COMPLETE: 10,
}


def is_at_least(current: InstallState, target: InstallState) -> bool:
    """True if ``current`` represents a phase >= ``target`` in the happy path.

    Failure states are never >= happy-path targets.
    """
    if current not in _ORDER or target not in _ORDER:
        return False
    return _ORDER[current] >= _ORDER[target]


@dataclass
class StateSnapshot:
    state: InstallState = InstallState.INIT
    updated_at: str = ""
    last_error: str | None = None
    data: dict[str, Any] = field(default_factory=dict)

    def to_json(self) -> dict[str, Any]:
        return {
            "state": self.state.value,
            "updated_at": self.updated_at,
            "last_error": self.last_error,
            "data": self.data,
        }

    @classmethod
    def from_json(cls, raw: dict[str, Any]) -> "StateSnapshot":
        return cls(
            state=InstallState(raw.get("state", InstallState.INIT.value)),
            updated_at=raw.get("updated_at", ""),
            last_error=raw.get("last_error"),
            data=raw.get("data", {}) or {},
        )


class StateStore:
    """JSON-backed state store at ``<install-root>/.arbos/install_state.json``."""

    def __init__(self, path: Path) -> None:
        self.path = path

    def load(self) -> StateSnapshot:
        if not self.path.exists():
            return StateSnapshot()
        try:
            raw = json.loads(self.path.read_text())
        except (json.JSONDecodeError, OSError):
            return StateSnapshot()
        return StateSnapshot.from_json(raw)

    def save(self, snapshot: StateSnapshot) -> None:
        snapshot.updated_at = datetime.now(timezone.utc).isoformat()
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_suffix(".tmp")
        tmp.write_text(json.dumps(snapshot.to_json(), indent=2, sort_keys=True))
        tmp.replace(self.path)

    def set(self, state: InstallState, **patch: Any) -> StateSnapshot:
        """Advance to ``state``, merging ``patch`` into the persisted data blob."""
        snapshot = self.load()
        snapshot.state = state
        snapshot.last_error = None
        if patch:
            snapshot.data.update(patch)
        self.save(snapshot)
        return snapshot

    def fail(self, state: InstallState, error: str) -> StateSnapshot:
        snapshot = self.load()
        snapshot.state = state
        snapshot.last_error = error
        self.save(snapshot)
        return snapshot
