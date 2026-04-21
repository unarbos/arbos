"""Crash-recovery journal for Telegram updates the agent is currently handling.

Telegram's ``getUpdates`` is acked by sending the next ``offset = max(update_id)
+ 1``. The polling loop in :mod:`agent` advances the offset as soon as it
*reads* an update -- not when the handler that processes the update finishes.
That is the right shape (we never want to re-pull the same update from
Telegram on a benign restart) but it leaves a gap: if the agent process dies
between accepting the update and finishing the handler, the user's question
silently disappears. There is no record on disk that we ever owed them an
answer.

This module closes that gap with a small filesystem journal:

* Before ``_save_offset`` is called for an update, ``mark_pending`` is invoked
  and the journal is written atomically (``tmp`` + ``os.replace``).
* When the spawned handler task finishes, ``mark_done``/``mark_failed`` is
  called from the ``add_done_callback`` hook.
* On graceful shutdown, ``mark_interrupted`` is called for every still-pending
  entry so the next boot can distinguish "we were killed mid-flight" from
  "we genuinely never saw this".
* On startup, :func:`load` returns every entry; the agent can replay the
  ones whose ``status`` is ``pending`` or ``interrupted`` (plain text) and
  apologise for the ones that touched mutating commands (``/update``,
  ``/reset``, ``/plan``).

Storage format is a single JSON document keyed by ``update_id`` (string).
Terminal entries (``done``/``failed``/``interrupted``) are pruned on every
write so the file stays small (``MAX_TERMINAL`` most-recent kept, oldest
trimmed). Pending entries are *never* auto-pruned -- if one outlives a long
restart it stays put until either replay completes it or an operator clears
the file by hand.

The store is designed to be tiny and synchronous. We accept the cost of one
~few-KiB atomic write per dispatch in exchange for not losing user messages
to a SIGINT.
"""

from __future__ import annotations

import json
import logging
import os
import time
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

from .workspace import InstallPaths

logger = logging.getLogger(__name__)

# Statuses ordered by lifecycle:
#   pending     -- handler task is currently running (or was, until we crashed)
#   interrupted -- handler was running when the agent received SIGTERM/SIGINT
#                  and we marked it before exiting cleanly. Caller should
#                  treat exactly like ``pending`` for replay purposes.
#   done        -- handler completed without raising
#   failed      -- handler raised; ``error`` carries a one-line summary
STATUS_PENDING = "pending"
STATUS_INTERRUPTED = "interrupted"
STATUS_DONE = "done"
STATUS_FAILED = "failed"

_TERMINAL = frozenset({STATUS_DONE, STATUS_FAILED, STATUS_INTERRUPTED})

# How many terminal (done/failed/interrupted) entries to retain. Pending
# entries are kept regardless of count -- they represent unfinished work.
MAX_TERMINAL = 200


@dataclass
class InflightEntry:
    """One row in the inflight journal.

    ``message`` is the raw Telegram ``message`` dict as we received it; we
    keep the whole thing so a replay can re-construct sender, message_id,
    text, voice payload, etc. without any extra lookups.
    """

    update_id: int
    message: dict[str, Any]
    status: str
    started_at: float  # monotonic-ish wall clock (time.time())
    finished_at: Optional[float] = None
    error: Optional[str] = None
    # Process id of the agent that wrote this row -- helps debug "did the
    # current process create this entry, or did a previous run leave it
    # behind?" without needing a separate boot-id file.
    pid: int = field(default_factory=os.getpid)

    @property
    def is_pending(self) -> bool:
        return self.status in (STATUS_PENDING, STATUS_INTERRUPTED)

    @property
    def text(self) -> str:
        return (self.message.get("text") or "").strip()

    def to_json(self) -> dict[str, Any]:
        return asdict(self)

    @classmethod
    def from_json(cls, raw: Any) -> Optional["InflightEntry"]:
        if not isinstance(raw, dict):
            return None
        try:
            return cls(
                update_id=int(raw["update_id"]),
                message=raw.get("message") or {},
                status=str(raw.get("status") or STATUS_PENDING),
                started_at=float(raw.get("started_at") or 0.0),
                finished_at=(
                    float(raw["finished_at"])
                    if raw.get("finished_at") is not None
                    else None
                ),
                error=(str(raw["error"]) if raw.get("error") else None),
                pid=int(raw.get("pid") or 0),
            )
        except (KeyError, TypeError, ValueError) as exc:
            logger.warning("ignoring malformed inflight row: %s (%s)", raw, exc)
            return None


def _iso(ts: float) -> str:
    """For diagnostic logs only -- the on-disk timestamp is the float."""
    try:
        return datetime.fromtimestamp(ts, tz=timezone.utc).isoformat(timespec="seconds")
    except (OverflowError, OSError, ValueError):
        return "?"


class InflightStore:
    """Synchronous, atomic, single-file journal.

    Not thread-safe by itself. The agent's polling loop and per-handler
    done-callback both run on a single asyncio event loop, so there is no
    real concurrency on this object today. If we ever spawn the watcher
    on a thread we'll need a lock.
    """

    def __init__(self, paths: InstallPaths) -> None:
        self._path: Path = paths.inflight_path
        self._entries: dict[str, InflightEntry] = {}
        self._loaded = False

    # ---- IO ----------------------------------------------------------------

    def load(self) -> dict[int, InflightEntry]:
        """Read the journal from disk into memory and return a snapshot.

        The snapshot is keyed by ``update_id`` (int) for the caller's
        convenience. Subsequent ``mark_*`` calls operate on the in-memory
        copy and rewrite the file.
        """
        self._entries = {}
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
        rows = raw.get("entries")
        if not isinstance(rows, list):
            return {}
        for row in rows:
            entry = InflightEntry.from_json(row)
            if entry is None:
                continue
            self._entries[str(entry.update_id)] = entry
        snapshot = {e.update_id: e for e in self._entries.values()}
        pending = sum(1 for e in snapshot.values() if e.is_pending)
        logger.info(
            "inflight journal loaded: %d entries (%d pending) from %s",
            len(snapshot), pending, self._path,
        )
        return snapshot

    def _flush(self) -> None:
        """Atomically write the in-memory state, pruning old terminal rows."""
        # Drop the oldest terminal entries first if we're over the cap, but
        # keep every pending/interrupted row no matter what.
        terminal = [e for e in self._entries.values() if e.status in _TERMINAL]
        if len(terminal) > MAX_TERMINAL:
            # Sort terminal entries by finished_at (None sorts last/oldest if
            # somehow missing); drop the oldest excess.
            terminal.sort(key=lambda e: (e.finished_at or e.started_at))
            for e in terminal[: len(terminal) - MAX_TERMINAL]:
                self._entries.pop(str(e.update_id), None)

        payload = {
            "version": 1,
            "entries": [e.to_json() for e in self._entries.values()],
        }
        self._path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self._path.with_suffix(self._path.suffix + ".tmp")
        # Same restrictive perms pattern as session_store.save().
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

    # ---- mutation ----------------------------------------------------------

    def mark_pending(
        self,
        update_id: int,
        message: dict[str, Any],
    ) -> InflightEntry:
        """Record that ``update_id`` has been accepted and is being handled.

        MUST be called *before* the polling loop persists the new offset --
        otherwise a crash between spawn and ``_save_offset`` would leave the
        journal blind to in-flight work.
        """
        if not self._loaded:
            self.load()
        entry = InflightEntry(
            update_id=update_id,
            message=message,
            status=STATUS_PENDING,
            started_at=time.time(),
        )
        self._entries[str(update_id)] = entry
        self._flush()
        return entry

    def mark_done(self, update_id: int) -> None:
        e = self._entries.get(str(update_id))
        if e is None:
            return
        e.status = STATUS_DONE
        e.finished_at = time.time()
        e.error = None
        self._flush()

    def mark_failed(self, update_id: int, error: str) -> None:
        e = self._entries.get(str(update_id))
        if e is None:
            return
        e.status = STATUS_FAILED
        e.finished_at = time.time()
        # Truncate so a giant traceback can't bloat the journal forever.
        e.error = error[:500] if error else "unknown error"
        self._flush()

    def mark_interrupted_all(self) -> int:
        """Stamp every still-pending entry as ``interrupted``.

        Returns the number of entries touched. Called from the agent's
        graceful shutdown path so the next boot can replay them with the
        correct provenance ("we were killed", not "we crashed silently").
        """
        if not self._loaded:
            self.load()
        n = 0
        now = time.time()
        for e in self._entries.values():
            if e.status == STATUS_PENDING:
                e.status = STATUS_INTERRUPTED
                e.finished_at = now
                n += 1
        if n:
            self._flush()
            logger.info(
                "marked %d in-flight handler(s) as interrupted on shutdown", n
            )
        return n

    # ---- introspection -----------------------------------------------------

    def pending(self) -> list[InflightEntry]:
        if not self._loaded:
            self.load()
        return [e for e in self._entries.values() if e.is_pending]

    def __len__(self) -> int:
        return len(self._entries)
