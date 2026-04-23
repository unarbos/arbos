"""Append-only rollout timeline + separate summary bubble for the chat.

The supervisor sits between :class:`CursorRunner` and a Telegram
:class:`Bubble` (the "rollout bubble") and produces an append-only
timeline of what the agent is doing. Each visible action / stall
transition / completion is a frozen line in the bubble:

    0:00 . starting
    0:02 . reading agent.py and prompts.py
    0:14 . running pytest
    0:32 . running pytest (waiting...)
    1:08 . stalled? last seen: running pytest
    done in 1:25

Once the worker finishes, the supervisor appends a terminal line
(``done in M:SS`` / ``interrupted at M:SS`` / ``crashed at M:SS: ...``)
and finalises the rollout. The agent layer then posts a *separate*
summary bubble with a one-paragraph past-tense recap.

Snappiness:

* the rollout flips from ``thinking...`` to ``0:00 . starting``
  immediately on ``__aenter__`` (before the worker even spawns);
* a fresh ``on_update`` appends a new entry whose text starts as a
  deterministic last-line summary, so the user sees the new action
  within one tick (no LLM round-trip on the critical path);
* the LLM call for that entry runs in the background; if it returns
  while the entry is still current, its text is swapped in place. If
  the user's worker has already moved on, the swap is dropped (we
  never edit a frozen line).

Failure modes:

* live LLM tick fails: the deterministic text stays; the timeline keeps
  growing as new actions happen, so the bubble never goes silent;
* ``summarise_final()`` LLM call fails: returns the first 300 chars of
  the worker's final answer instead.
"""

from __future__ import annotations

import asyncio
import logging
import time
from dataclasses import dataclass, field
from typing import Any, Optional

import httpx

logger = logging.getLogger(__name__)


OPENAI_CHAT_URL = "https://api.openai.com/v1/chat/completions"

# Hard cap on snapshot tail shipped to the LLM as "current state".
SNAPSHOT_TAIL = 2000

# Per-request budget for live status calls (must be << bubble edit floor).
OPENAI_TIMEOUT = 8.0

# Rollout cap: how many entries we keep before trimming the head. The
# bubble itself rolls over at 4096 chars but a 200-action run would
# spam continuation messages; trimming here keeps a single bubble
# usable. The head is what gets dropped (oldest), so the user always
# sees the latest activity.
MAX_ENTRIES = 80

# Worker has been quiet (no new snapshot) for this long: append a hint
# entry so the user understands the agent is waiting on something.
STALL_HINT_AFTER = 15.0

# Worker has been quiet for this long: switch to a "stalled?" entry so
# the user knows we're not sure it's progressing.
STALL_WARN_AFTER = 60.0

# One-shot end-of-run wrap-up budget.
FINAL_MAX_CHARS = 500
FINAL_TIMEOUT = 12.0

# Stall states; tracked so we only append a new line on transition.
_STALL_OK = 0
_STALL_WAITING = 1
_STALL_WARNED = 2


LIVE_PROMPT = (
    "You are a terse status reporter for a coding agent. From the agent's "
    "most recent visible state below, reply with ONE present-tense line "
    "that describes what it is doing right now. Hard limit: {max_chars} "
    "characters. No preamble, no quotes, no markdown, no period if you "
    "are tight on space. If the agent looks idle, say 'thinking'."
)

FINAL_PROMPT = (
    "You are wrapping up a coding agent run. In ONE past-tense line "
    "(<= {max_chars} chars), tell the user what the agent did. Be "
    "specific about files / actions if obvious from the inputs, vague "
    "if not. No preamble, no markdown, no quotes. Do not restate the "
    "duration -- the rollout already shows it."
)


def _truncate(text: str, n: int) -> str:
    if len(text) <= n:
        return text
    return text[: n - 1] + "..."


def _mmss(seconds: float) -> str:
    s = max(0, int(seconds))
    if s < 3600:
        return f"{s // 60}:{s % 60:02d}"
    h, rem = divmod(s, 3600)
    return f"{h}h{rem // 60:02d}"


def _last_meaningful_line(snapshot: str) -> str:
    """Pick a representative single line from the worker's rendered snapshot.

    Walks bottom-up and returns the first non-empty, non-decoration line.
    Used as the deterministic seed for a new entry's text (and as the
    permanent text when no LLM is available).
    """
    if not snapshot:
        return "thinking"
    for raw in reversed(snapshot.splitlines()):
        line = raw.strip()
        if not line:
            continue
        # Strip cursor_runner's tool-log status glyphs so the entry
        # reads like a plain command instead of glyph soup.
        for prefix in ("\u23f3 ", "\u2713 ", "\u2717 ", "thinking..."):
            if line.startswith(prefix):
                line = line[len(prefix):].strip()
                break
        if line:
            return line
    return "thinking"


@dataclass
class _Entry:
    """One line in the rollout timeline.

    ``started_at`` is monotonic and frozen at creation; the displayed
    prefix is ``mmss(started_at - supervisor_started_at)``. ``text`` is
    mutable while this is the *current* entry (LLM swap-in); once
    another entry is appended, ``final`` flips True and the text is
    never rewritten again. ``terminal`` marks the closing line
    (``done in ...`` / ``interrupted at ...`` / ``crashed at ...``) so
    we can render it without the timer prefix.
    """

    started_at: float
    text: str
    final: bool = False
    terminal: bool = False
    # Snapshot the LLM is being asked about for this entry; used to
    # discard stale responses if the worker has already moved on.
    pending_snapshot: Optional[str] = None


class Supervisor:
    """Append-only rollout timeline + LLM-backed end-of-run recap."""

    def __init__(
        self,
        *,
        bubble: Any,
        openai_api_key: Optional[str],
        model: str = "gpt-4o-mini",
        interval: float = 2.0,
        max_chars: int = 250,
        initial_text: str = "starting",
        final_max_chars: int = FINAL_MAX_CHARS,
        stall_hint_after: float = STALL_HINT_AFTER,
        stall_warn_after: float = STALL_WARN_AFTER,
    ) -> None:
        self._bubble = bubble
        self._key = (openai_api_key or "").strip()
        self._model = model
        self._interval = max(0.25, float(interval))
        self._max_chars = max(40, int(max_chars))
        self._final_max_chars = max(80, int(final_max_chars))
        self._initial_text = initial_text
        self._stall_hint_after = max(1.0, float(stall_hint_after))
        self._stall_warn_after = max(self._stall_hint_after + 1.0, float(stall_warn_after))
        self._live_prompt = LIVE_PROMPT.format(max_chars=self._max_chars)
        self._final_prompt = FINAL_PROMPT.format(max_chars=self._final_max_chars)

        # Timeline state.
        self._entries: list[_Entry] = []
        self._stall_state: int = _STALL_OK

        # Worker state.
        self._latest_snapshot: str = ""
        self._snapshot_seq: int = 0  # bumped on every meaningful change
        self._last_snapshot_at: float = 0.0
        self._first_seen = asyncio.Event()
        self._wake = asyncio.Event()
        self._worker_final_text: str = ""

        # Timing.
        self._started_at: float = 0.0

        # Lifecycle.
        self._stopped = asyncio.Event()
        self._finalised = False
        self._heartbeat_task: Optional[asyncio.Task[None]] = None
        self._client: Optional[httpx.AsyncClient] = None

    # --- lifecycle -------------------------------------------------------

    async def __aenter__(self) -> "Supervisor":
        self._started_at = time.monotonic()
        self._last_snapshot_at = self._started_at
        self._client = httpx.AsyncClient(timeout=OPENAI_TIMEOUT) if self._key else None
        # Snappy first push: the rollout shows "0:00 . starting" before
        # we return, so the bubble visibly responds before the worker
        # subprocess even gets spawned.
        self._append_entry(self._initial_text)
        await self._push()
        self._heartbeat_task = asyncio.create_task(self._heartbeat())
        return self

    async def __aexit__(self, exc_type, exc, tb) -> None:
        self._stopped.set()
        self._wake.set()
        if self._heartbeat_task is not None:
            self._heartbeat_task.cancel()
            try:
                await self._heartbeat_task
            except (asyncio.CancelledError, Exception):
                pass
            self._heartbeat_task = None
        if self._client is not None:
            try:
                await self._client.aclose()
            except Exception:
                pass
            self._client = None

    # --- public API ------------------------------------------------------

    async def on_update(self, text: str) -> None:
        """Receive a fresh worker snapshot.

        On a meaningful (i.e. content-changed) snapshot we append a new
        entry seeded with deterministic text, push it to the bubble
        immediately for snappiness (the bubble itself only stages text in
        memory, so this never blocks on a Telegram round-trip), and wake
        the heartbeat so the LLM swap-in fires on the next tick.
        """
        if not isinstance(text, str) or text == self._latest_snapshot:
            return
        self._latest_snapshot = text
        self._snapshot_seq += 1
        self._last_snapshot_at = time.monotonic()
        self._append_entry(_last_meaningful_line(text), pending_snapshot=text)
        if not self._first_seen.is_set():
            self._first_seen.set()
        await self._push()
        self._wake.set()

    async def on_final(self, text: str) -> None:
        """Stash worker's final answer, append 'done in M:SS', finalise."""
        self._worker_final_text = text or ""
        await self._terminal_finalise(f"done in {self.duration_str()}")

    async def mark_interrupted(self) -> None:
        await self._terminal_finalise(f"interrupted at {self.duration_str()}")

    async def mark_crashed(self, error: str) -> None:
        msg = (error or "").strip().splitlines()[0] if error else ""
        line = f"crashed at {self.duration_str()}"
        if msg:
            line += f": {_truncate(msg, 200)}"
        await self._terminal_finalise(line)

    def duration_str(self) -> str:
        if self._started_at == 0.0:
            return "0:00"
        return _mmss(time.monotonic() - self._started_at)

    async def summarise_final(self) -> str:
        """One-shot LLM recap. Safe to call after on_final / mark_*.

        Returns a short past-tense paragraph describing what happened.
        Fallback when the LLM is unavailable / errors out: the first
        ~300 chars of the worker's final answer (or '(no output)').
        """
        worker_final = (self._worker_final_text or "").strip()
        fallback = _truncate(worker_final, 300) or "(no output)"
        if self._client is None or not self._key:
            return _truncate(fallback, self._final_max_chars)

        tool_tail = self._latest_snapshot[-SNAPSHOT_TAIL:]
        body = {
            "model": self._model,
            "messages": [
                {"role": "system", "content": self._final_prompt},
                {
                    "role": "user",
                    "content": (
                        f"Duration: {self.duration_str()}.\n\n"
                        "Recent visible state / tool log (tail):\n"
                        f"{tool_tail}\n\n"
                        "Worker's final answer:\n"
                        f"{worker_final[:3000]}"
                    ),
                },
            ],
            "max_tokens": 220,
            "temperature": 0.2,
        }
        try:
            resp = await self._client.post(
                OPENAI_CHAT_URL,
                headers={
                    "Authorization": f"Bearer {self._key}",
                    "Content-Type": "application/json",
                },
                json=body,
                timeout=FINAL_TIMEOUT,
            )
        except httpx.HTTPError as exc:
            logger.warning("supervisor final openai request failed: %s", exc)
            return _truncate(fallback, self._final_max_chars)
        if resp.status_code != 200:
            logger.warning(
                "supervisor final openai HTTP %s: %s",
                resp.status_code,
                resp.text[:200],
            )
            return _truncate(fallback, self._final_max_chars)
        try:
            data = resp.json()
            choice = (data.get("choices") or [{}])[0]
            content = ((choice.get("message") or {}).get("content") or "").strip()
        except Exception:
            logger.exception("supervisor final openai parse failed")
            return _truncate(fallback, self._final_max_chars)
        if not content:
            return _truncate(fallback, self._final_max_chars)
        recap = " ".join(content.split())
        return _truncate(recap, self._final_max_chars)

    # --- internal: timeline mutation ------------------------------------

    def _append_entry(
        self,
        text: str,
        *,
        terminal: bool = False,
        pending_snapshot: Optional[str] = None,
    ) -> None:
        # Freeze the previous current entry.
        if self._entries:
            self._entries[-1].final = True
            self._entries[-1].pending_snapshot = None
        entry = _Entry(
            started_at=time.monotonic(),
            text=_truncate(text or "thinking", self._max_chars),
            final=terminal,  # terminal entries are immediately frozen
            terminal=terminal,
            pending_snapshot=pending_snapshot,
        )
        self._entries.append(entry)
        # Cap the timeline length; drop the oldest when over.
        if len(self._entries) > MAX_ENTRIES:
            drop = len(self._entries) - MAX_ENTRIES
            del self._entries[:drop]

    def _format_entry(self, e: _Entry) -> str:
        if e.terminal:
            return e.text
        prefix = _mmss(e.started_at - self._started_at)
        return f"{prefix} . {e.text}"

    def _render(self) -> str:
        if not self._entries:
            return self._initial_text
        return "\n".join(self._format_entry(e) for e in self._entries)

    async def _push(self) -> None:
        text = self._render()
        try:
            await self._bubble.update(text)
        except Exception:
            logger.exception("bubble.update raised in supervisor")

    async def _terminal_finalise(self, line: str) -> None:
        """Append a terminal line and finalise the rollout bubble. Idempotent."""
        if self._finalised:
            return
        self._finalised = True
        # Stop the heartbeat first so it doesn't race us pushing edits.
        self._stopped.set()
        self._wake.set()
        if self._heartbeat_task is not None:
            self._heartbeat_task.cancel()
            try:
                await self._heartbeat_task
            except (asyncio.CancelledError, Exception):
                pass
            self._heartbeat_task = None
        self._append_entry(line, terminal=True)
        text = self._render()
        try:
            await self._bubble.finalize(text)
        except Exception:
            logger.exception("bubble.finalize raised in supervisor")

    # --- internal: heartbeat --------------------------------------------

    async def _heartbeat(self) -> None:
        """Drive entry mutations + stall transitions + LLM swap-ins.

        We don't push the bubble on a fixed cadence; we push when
        something visible has actually changed (new entry, LLM swap-in,
        stall transition). This keeps the bubble honest and saves
        editMessageText calls.
        """
        try:
            while not self._stopped.is_set():
                # Wake on either the wake event (snapshot arrived) or
                # the interval (so we can check stall transitions).
                try:
                    await asyncio.wait_for(self._wake.wait(), timeout=self._interval)
                except asyncio.TimeoutError:
                    pass
                self._wake.clear()
                if self._stopped.is_set():
                    return
                changed = False
                # Stall transition first: appending an entry shifts the
                # current entry, so we want stalls to land before we
                # fire the LLM call below.
                if self._maybe_stall_transition():
                    changed = True
                # Fire LLM for the current entry if it has a pending
                # snapshot and we have a client.
                cur = self._entries[-1] if self._entries else None
                if (
                    cur is not None
                    and cur.pending_snapshot is not None
                    and self._client is not None
                ):
                    snapshot = cur.pending_snapshot
                    seq = self._snapshot_seq
                    fresh = await self._summarise_live(snapshot)
                    cur.pending_snapshot = None
                    if (
                        fresh
                        and not self._stopped.is_set()
                        and self._snapshot_seq == seq
                        and self._entries
                        and self._entries[-1] is cur
                        and not cur.final
                    ):
                        cur.text = _truncate(fresh, self._max_chars)
                        changed = True
                if changed:
                    await self._push()
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.exception("supervisor heartbeat crashed")

    def _maybe_stall_transition(self) -> bool:
        """Append a new entry if the worker silence has crossed a
        threshold since we last logged it. Returns True iff an entry
        was appended."""
        if self._finalised or not self._entries:
            return False
        # Don't stall-decorate the very initial 'starting' entry; only
        # start tracking once the worker has actually emitted something.
        if not self._first_seen.is_set():
            return False
        silent_for = time.monotonic() - self._last_snapshot_at
        cur = self._entries[-1]
        # If the last entry is already a stall marker for this state,
        # don't re-emit.
        if silent_for >= self._stall_warn_after:
            if self._stall_state == _STALL_WARNED:
                return False
            self._stall_state = _STALL_WARNED
            last_action = self._last_non_stall_text() or "the same thing"
            self._append_entry(f"stalled? last seen: {last_action}")
            return True
        if silent_for >= self._stall_hint_after:
            if self._stall_state == _STALL_WAITING:
                return False
            self._stall_state = _STALL_WAITING
            last_action = self._last_non_stall_text() or cur.text
            self._append_entry(f"{last_action} (waiting...)")
            return True
        # Below hint threshold: if we previously logged a stall and the
        # worker has spoken again since, that itself appended a fresh
        # entry via on_update -- so we don't need to add a "resolved"
        # line here. Just reset the state machine.
        self._stall_state = _STALL_OK
        return False

    def _last_non_stall_text(self) -> str:
        """Walk back through entries to find the last non-stall, non-
        terminal action text."""
        for e in reversed(self._entries):
            if e.terminal:
                continue
            t = e.text
            if t.endswith("(waiting...)"):
                t = t[: -len("(waiting...)")].strip()
            if t.startswith("stalled? last seen: "):
                t = t[len("stalled? last seen: "):]
                return t
            if t and not t.startswith("stalled?"):
                return t
        return ""

    async def _summarise_live(self, snapshot: str) -> Optional[str]:
        """LLM-backed single-line summary of the current snapshot. Returns
        ``None`` on failure (caller keeps the deterministic text)."""
        client = self._client
        if client is None or not self._key:
            return None
        body = {
            "model": self._model,
            "messages": [
                {"role": "system", "content": self._live_prompt},
                {
                    "role": "user",
                    "content": (
                        "Agent's most recent visible state (last "
                        f"{SNAPSHOT_TAIL} chars):\n\n"
                        + snapshot[-SNAPSHOT_TAIL:]
                    ),
                },
            ],
            "max_tokens": 120,
            "temperature": 0.2,
        }
        try:
            resp = await client.post(
                OPENAI_CHAT_URL,
                headers={
                    "Authorization": f"Bearer {self._key}",
                    "Content-Type": "application/json",
                },
                json=body,
            )
        except httpx.HTTPError as exc:
            logger.warning("supervisor live openai request failed: %s", exc)
            return None
        if resp.status_code != 200:
            logger.warning(
                "supervisor live openai HTTP %s: %s",
                resp.status_code,
                resp.text[:200],
            )
            return None
        try:
            data = resp.json()
            choice = (data.get("choices") or [{}])[0]
            content = ((choice.get("message") or {}).get("content") or "").strip()
        except Exception:
            logger.exception("supervisor live openai parse failed")
            return None
        if not content:
            return None
        return " ".join(content.split())
