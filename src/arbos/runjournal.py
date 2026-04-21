"""Per-bubble run journal.

Every bubble Arbos posts into the Telegram topic gets a small JSON file in
``<install-root>/.arbos/runs/<message_id>.json`` describing the run that
produced it: which command, which trigger, the rolling tool-call log, and
the final answer text. When the user **replies** to one of those bubbles,
the agent layer looks up the matching record and decorates the next
``cursor-agent`` prompt with a ``<<<REPLY_CONTEXT>>>`` block so the model
can resolve "no, fix the second one" / "did that finish?" / etc. against
the *specific* prior run instead of guessing from chat history.

Design constraints:

* **Sync, atomic, never raises**. Every public method swallows OSError and
  logs at warning level. A failed write must never bubble out into the
  Telegram handler -- losing one journal entry is acceptable; failing a
  user message is not.
* **O(1) lookup keyed by Telegram message_id**. No directory scans on the
  hot path; just `<runs_dir>/<msg_id>.json`.
* **Bounded growth**. Each entry capped at ``MAX_BYTES``; the directory
  is pruned to ``KEEP_N`` newest entries on every write.
* **Pure addition**. No coupling to inflight, session_store, or outbox.
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


# Per-entry hard cap. Realistically each entry is 1-4 KiB; this is the
# emergency brake against pathological cases (a tool_log filled with
# absurd commands).
MAX_BYTES = 32 * 1024

# Most recent N entries to retain. With ~32 KiB worst case that's ~6 MB
# of disk use; in practice closer to 100 KiB total.
KEEP_N = 200

# Cap on the rendered REPLY_CONTEXT block we feed back into cursor-agent.
# Counted in chars; keeps the prompt under control even if a single run
# had a huge final answer.
MAX_BLOCK_CHARS = 3000

# Per-field truncation budgets used by ``summarise``. Generous enough to
# preserve meaning, tight enough that the assembled block stays under
# MAX_BLOCK_CHARS even with a long tool log.
MAX_TRIGGER_CHARS = 600
MAX_FINAL_CHARS = 1500
MAX_TOOL_LOG_LINES = 12


@dataclass
class RunRecord:
    """One bubble's run-time metadata. See module docstring for usage."""

    bubble_message_id: int
    kind: str               # "chat" | "plan" | "impl" | "update" | "restart" | "reset" | "confirm"
    trigger_message_id: Optional[int] = None
    trigger_text: str = ""
    trigger_sender: str = ""
    started_at: float = 0.0
    ended_at: Optional[float] = None
    status: str = "running"  # "running" | "ok" | "error" | "interrupted"
    model: Optional[str] = None
    session_id: Optional[str] = None
    tool_log: list[str] = field(default_factory=list)
    final_text: str = ""
    paired_bubble_id: Optional[int] = None
    version: int = 1

    def to_json(self) -> str:
        return json.dumps(asdict(self), sort_keys=True)

    @classmethod
    def from_dict(cls, raw: dict[str, Any]) -> "RunRecord":
        # Be defensive: accept dicts written by older versions by only
        # consuming known fields and ignoring the rest.
        return cls(
            bubble_message_id=int(raw.get("bubble_message_id") or 0),
            kind=str(raw.get("kind") or "chat"),
            trigger_message_id=(
                int(raw["trigger_message_id"])
                if raw.get("trigger_message_id") is not None
                else None
            ),
            trigger_text=str(raw.get("trigger_text") or ""),
            trigger_sender=str(raw.get("trigger_sender") or ""),
            started_at=float(raw.get("started_at") or 0.0),
            ended_at=(
                float(raw["ended_at"])
                if raw.get("ended_at") is not None
                else None
            ),
            status=str(raw.get("status") or "running"),
            model=raw.get("model") if isinstance(raw.get("model"), str) else None,
            session_id=(
                raw.get("session_id")
                if isinstance(raw.get("session_id"), str)
                else None
            ),
            tool_log=[
                str(x) for x in (raw.get("tool_log") or [])
                if isinstance(x, (str, int, float))
            ],
            final_text=str(raw.get("final_text") or ""),
            paired_bubble_id=(
                int(raw["paired_bubble_id"])
                if raw.get("paired_bubble_id") is not None
                else None
            ),
            version=int(raw.get("version") or 1),
        )


def _truncate(s: str, n: int) -> str:
    if not s:
        return s
    if len(s) <= n:
        return s
    return s[: n - 1] + "…"


def _trim_record_for_disk(rec: RunRecord) -> RunRecord:
    """Apply per-field budgets so a single record can't blow MAX_BYTES."""
    rec.trigger_text = _truncate(rec.trigger_text, MAX_TRIGGER_CHARS)
    rec.final_text = _truncate(rec.final_text, MAX_FINAL_CHARS)
    if len(rec.tool_log) > MAX_TOOL_LOG_LINES:
        rec.tool_log = rec.tool_log[-MAX_TOOL_LOG_LINES:]
    return rec


def _ago(seconds_ago: float) -> str:
    seconds_ago = max(0.0, seconds_ago)
    if seconds_ago < 60:
        return f"{int(seconds_ago)}s ago"
    if seconds_ago < 3600:
        return f"{int(seconds_ago / 60)}m ago"
    if seconds_ago < 86400:
        return f"{int(seconds_ago / 3600)}h ago"
    return f"{int(seconds_ago / 86400)}d ago"


def summarise(rec: RunRecord, *, max_chars: int = MAX_BLOCK_CHARS) -> str:
    """Render a ``RunRecord`` as a human-readable block fragment.

    Caller wraps this in the ``<<<REPLY_CONTEXT>>>`` fences. We hard-cap
    the output so a degenerate record can't dominate the prompt.
    """
    when_iso = (
        time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(rec.started_at))
        if rec.started_at
        else "unknown"
    )
    when_rel = _ago(time.time() - rec.started_at) if rec.started_at else "?"

    lines: list[str] = []
    lines.append(f"- kind: {rec.kind}")
    lines.append(f"- when: {when_iso} ({when_rel})")
    lines.append(f"- status: {rec.status}")
    if rec.model:
        lines.append(f"- model: {rec.model}")
    if rec.trigger_sender or rec.trigger_text:
        sender = rec.trigger_sender or "user"
        text = _truncate(rec.trigger_text, MAX_TRIGGER_CHARS).replace("\n", " ")
        lines.append(f'- triggered by: {sender} said "{text}"')
    if rec.paired_bubble_id is not None:
        lines.append(f"- paired with bubble: {rec.paired_bubble_id}")
    if rec.tool_log:
        recent = rec.tool_log[-MAX_TOOL_LOG_LINES:]
        lines.append("- tool calls (oldest -> newest):")
        for line in recent:
            lines.append(f"    {line}")
    if rec.final_text:
        truncated_final = _truncate(rec.final_text, MAX_FINAL_CHARS)
        lines.append("- final answer:")
        # Indent each line of the answer so it stays visually grouped under
        # the bullet; use a triple-quote-ish fence inside.
        lines.append('    """')
        for ftxt_line in truncated_final.splitlines():
            lines.append(f"    {ftxt_line}")
        lines.append('    """')
    elif rec.status == "running":
        lines.append("- final answer: (run is still in progress)")

    rendered = "\n".join(lines)
    if len(rendered) > max_chars:
        rendered = rendered[: max_chars - 1] + "…"
    return rendered


class RunJournal:
    """Per-bubble run journal rooted at ``paths.runs_dir``."""

    def __init__(self, paths: InstallPaths) -> None:
        self.paths = paths
        # Ensure the dir exists even if bootstrap() wasn't run on this
        # exact path (e.g. dev loop in a fresh clone).
        try:
            self.paths.runs_dir.mkdir(parents=True, exist_ok=True)
        except OSError as exc:
            logger.warning("could not create runs dir %s: %s", self.paths.runs_dir, exc)

    def _path_for(self, message_id: int) -> Any:
        return self.paths.runs_dir / f"{int(message_id)}.json"

    def _atomic_write(self, message_id: int, rec: RunRecord) -> None:
        rec = _trim_record_for_disk(rec)
        payload = rec.to_json()
        if len(payload) > MAX_BYTES:
            # Last-resort: lop off final_text + tool_log entirely.
            rec.final_text = _truncate(rec.final_text, 200) + " (truncated)"
            rec.tool_log = rec.tool_log[-3:]
            payload = rec.to_json()
        path = self._path_for(message_id)
        tmp = path.with_suffix(path.suffix + ".tmp")
        try:
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
        except OSError as exc:
            logger.warning("runjournal write %s failed: %s", path, exc)

    def _prune(self) -> None:
        """Keep the KEEP_N newest entries by mtime; unlink the rest."""
        try:
            entries = []
            for entry in os.scandir(self.paths.runs_dir):
                if not entry.is_file():
                    continue
                if not entry.name.endswith(".json"):
                    continue
                try:
                    entries.append((entry.stat().st_mtime, entry.path))
                except OSError:
                    continue
        except OSError as exc:
            logger.debug("runjournal prune scandir failed: %s", exc)
            return
        if len(entries) <= KEEP_N:
            return
        entries.sort(key=lambda t: t[0], reverse=True)
        for _mtime, path in entries[KEEP_N:]:
            try:
                os.unlink(path)
            except OSError:
                pass

    # ---- public API ------------------------------------------------------

    def record_start(
        self,
        message_id: int,
        *,
        kind: str,
        trigger_message_id: Optional[int] = None,
        trigger_text: str = "",
        trigger_sender: str = "",
        model: Optional[str] = None,
        paired_bubble_id: Optional[int] = None,
    ) -> None:
        try:
            rec = RunRecord(
                bubble_message_id=int(message_id),
                kind=kind,
                trigger_message_id=trigger_message_id,
                trigger_text=trigger_text or "",
                trigger_sender=str(trigger_sender or ""),
                started_at=time.time(),
                ended_at=None,
                status="running",
                model=model,
                paired_bubble_id=paired_bubble_id,
            )
            self._atomic_write(message_id, rec)
            self._prune()
        except Exception:
            logger.exception("runjournal.record_start crashed (swallowed)")

    def record_finish(
        self,
        message_id: int,
        *,
        status: str,
        final_text: Optional[str] = None,
        tool_log: Optional[list[str]] = None,
        session_id: Optional[str] = None,
    ) -> None:
        """Update an existing record with terminal state. Idempotent.

        If no record exists for ``message_id`` (e.g. the start was lost or
        we're re-running an old code path that didn't journal), we create
        one on the fly with sensible defaults so the lookup still works.
        """
        try:
            existing = self.get(message_id)
            if existing is None:
                existing = RunRecord(
                    bubble_message_id=int(message_id),
                    kind="chat",
                    started_at=time.time(),
                )
            existing.status = status
            existing.ended_at = time.time()
            if final_text is not None:
                existing.final_text = final_text
            if tool_log is not None:
                existing.tool_log = list(tool_log)
            if session_id is not None:
                existing.session_id = session_id
            self._atomic_write(message_id, existing)
        except Exception:
            logger.exception("runjournal.record_finish crashed (swallowed)")

    def get(self, message_id: int) -> Optional[RunRecord]:
        """O(1) lookup by Telegram message_id. Returns None on any failure."""
        try:
            path = self._path_for(message_id)
            try:
                raw = path.read_text()
            except FileNotFoundError:
                return None
            except OSError as exc:
                logger.warning("runjournal read %s failed: %s", path, exc)
                return None
            try:
                data = json.loads(raw)
            except json.JSONDecodeError as exc:
                logger.warning("runjournal corrupt %s: %s", path, exc)
                return None
            if not isinstance(data, dict):
                return None
            return RunRecord.from_dict(data)
        except Exception:
            logger.exception("runjournal.get crashed (swallowed)")
            return None
