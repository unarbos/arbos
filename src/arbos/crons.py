"""Persistent recurring-prompt schedules for the per-machine Telegram agent.

Each cron is a ``(mins, prompt)`` pair; the agent runs the prompt every
``mins`` minutes and posts the output into the topic. Crons live in
``<install-root>/.arbos/crons.json`` so they survive PM2 restarts and
``/update`` -- the agent re-spawns one asyncio task per persisted entry
on boot.

The store is intentionally tiny + sync: one small JSON read at agent
boot, one atomic write whenever the set changes (which is rare). Atomic
write = ``tmp`` file + ``os.replace`` so a crash mid-write can never
produce a half-written state.

IDs are short, human-friendly (``c1``, ``c2``, ...). The ``next_id``
counter is persisted alongside the entries so removed ids are never
reused -- you don't accidentally end up with two ``c1``s in the journal
across the lifetime of an install.
"""

from __future__ import annotations

import json
import logging
import os
import time
from dataclasses import asdict, dataclass, field
from typing import Any, Optional

from .workspace import InstallPaths

logger = logging.getLogger(__name__)


@dataclass
class CronEntry:
    """One scheduled recurring prompt."""

    id: str
    mins: int
    prompt: str
    created_at: float = field(default_factory=time.time)
    created_by: str = ""
    # Forum topic this cron fires into. ``0`` is a sentinel for "use the
    # home topic at load time" -- set by older crons.json files written
    # before the multi-topic refactor. ``CronStore.load`` rewrites these
    # in-place once the agent supplies its home topic id.
    topic_id: int = 0

    def to_json(self) -> dict[str, Any]:
        return asdict(self)

    @classmethod
    def from_json(cls, raw: Any) -> Optional["CronEntry"]:
        if not isinstance(raw, dict):
            return None
        try:
            cid = str(raw["id"]).strip()
            mins = int(raw["mins"])
            prompt = str(raw["prompt"])
        except (KeyError, TypeError, ValueError) as exc:
            logger.warning("ignoring malformed cron row: %s (%s)", raw, exc)
            return None
        if not cid or mins < 1 or not prompt.strip():
            return None
        try:
            topic_id = int(raw.get("topic_id") or 0)
        except (TypeError, ValueError):
            topic_id = 0
        return cls(
            id=cid,
            mins=mins,
            prompt=prompt,
            created_at=float(raw.get("created_at") or time.time()),
            created_by=str(raw.get("created_by") or ""),
            topic_id=topic_id,
        )


class CronStore:
    """Synchronous, atomic, single-file JSON store for crons.

    Not thread-safe. The agent's polling loop, cron loops, and message
    handlers all run on a single asyncio event loop, so there is no
    real concurrency on this object today.
    """

    def __init__(self, paths: InstallPaths) -> None:
        self._path = paths.crons_path
        self._entries: dict[str, CronEntry] = {}
        self._next_id: int = 1
        self._loaded = False

    # ---- IO --------------------------------------------------------------

    def load(self) -> dict[str, CronEntry]:
        """Read the journal from disk into memory and return a snapshot."""
        self._entries = {}
        self._next_id = 1
        self._loaded = True
        if not self._path.exists():
            return {}
        try:
            raw = json.loads(self._path.read_text())
        except (OSError, json.JSONDecodeError) as exc:
            logger.warning("ignoring corrupt %s: %s", self._path, exc)
            return {}
        if not isinstance(raw, dict):
            return {}
        rows = raw.get("crons")
        if isinstance(rows, list):
            for row in rows:
                entry = CronEntry.from_json(row)
                if entry is None:
                    continue
                self._entries[entry.id] = entry
        try:
            self._next_id = max(1, int(raw.get("next_id") or 1))
        except (TypeError, ValueError):
            self._next_id = 1
        # Defensive: bump next_id past anything we observed in the file
        # so a hand-edited journal can't make us reuse an id.
        for cid in self._entries:
            if cid.startswith("c"):
                try:
                    n = int(cid[1:])
                except ValueError:
                    continue
                if n >= self._next_id:
                    self._next_id = n + 1
        logger.info(
            "cron store loaded: %d entries (next_id=%d) from %s",
            len(self._entries), self._next_id, self._path,
        )
        return dict(self._entries)

    def _flush(self) -> None:
        payload = {
            "version": 1,
            "next_id": self._next_id,
            "crons": [e.to_json() for e in self._entries.values()],
        }
        self._path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self._path.with_suffix(self._path.suffix + ".tmp")
        # Same restrictive perms pattern as session_store.save() /
        # inflight._flush(): create the file 0600 so we never briefly
        # expose it world-readable on a shared filesystem.
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        try:
            with os.fdopen(fd, "w") as fh:
                fh.write(json.dumps(payload, indent=2))
        except Exception:
            try:
                os.unlink(tmp)
            except OSError:
                pass
            raise
        os.replace(tmp, self._path)
        try:
            os.chmod(self._path, 0o600)
        except OSError:
            pass

    # ---- mutation --------------------------------------------------------

    def add(
        self,
        *,
        mins: int,
        prompt: str,
        topic_id: int,
        created_by: str = "",
    ) -> CronEntry:
        """Allocate a new id and persist the entry."""
        if not self._loaded:
            self.load()
        cid = f"c{self._next_id}"
        self._next_id += 1
        entry = CronEntry(
            id=cid,
            mins=int(mins),
            prompt=prompt,
            created_by=created_by,
            topic_id=int(topic_id),
        )
        self._entries[cid] = entry
        self._flush()
        return entry

    def backfill_topic_id(self, default_topic_id: int) -> int:
        """Rewrite any legacy entry with ``topic_id == 0`` to the home topic.

        Returns the number of entries patched. No-op when nothing to fix.
        Called by the agent at boot so persisted crons survive the
        multi-topic refactor without needing a manual edit.
        """
        if not self._loaded:
            self.load()
        patched = 0
        for entry in self._entries.values():
            if entry.topic_id == 0:
                entry.topic_id = int(default_topic_id)
                patched += 1
        if patched:
            self._flush()
            logger.info(
                "backfilled topic_id=%s onto %d legacy cron(s)",
                default_topic_id, patched,
            )
        return patched

    def remove(self, cid: str) -> bool:
        """Drop ``cid`` from the store. Returns True iff it existed."""
        if not self._loaded:
            self.load()
        if cid not in self._entries:
            return False
        self._entries.pop(cid, None)
        self._flush()
        return True

    # ---- introspection ---------------------------------------------------

    def get(self, cid: str) -> Optional[CronEntry]:
        if not self._loaded:
            self.load()
        return self._entries.get(cid)

    def all(self) -> list[CronEntry]:
        if not self._loaded:
            self.load()
        return list(self._entries.values())

    def __len__(self) -> int:
        return len(self._entries)
