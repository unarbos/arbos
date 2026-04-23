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
    "most recent visible state below, reply with ONE short PAST-TENSE "
    "phrase, capitalized first letter, describing what just happened. "
    "Examples: 'Edited agent.py', 'Ran pytest', 'Read prompts.py', "
    "'Wrote /tmp/foo'. Hard limit: {max_chars} characters. No preamble, "
    "no quotes, no markdown, no trailing period. If the agent looks idle, "
    "say 'Thinking'."
)

TASK_HINT_PROMPT = (
    "Compress the user's request below into a 2-4 word lowercase task "
    "description suitable for filling in 'Starting ___'. Do NOT begin "
    "with 'starting', 'start', 'begin', or any verb of initiation -- "
    "the rollout already prepends 'Starting'. Be concrete. No "
    "punctuation, no quotes. Examples: 'building a test file', "
    "'fixing the cron loop', 'reading the supervisor module'. Reply "
    "with just the phrase, nothing else."
)

FINAL_PROMPT = (
    "You ARE the coding agent that just finished this run -- speak in "
    "the first person. Reply with ONE first-person past-tense sentence "
    "(<= {max_chars} chars) telling the user what you did, using 'I' "
    "(e.g. 'I created the file...', 'I edited agent.py...'). Be "
    "specific about files / actions if obvious from the inputs, vague "
    "if not. No preamble, no markdown, no quotes, NEVER refer to "
    "yourself as 'the agent' or in the third person. Do not restate "
    "the duration -- the rollout already shows it."
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
    Used only as a deterministic *fallback* when no LLM is available --
    ``on_update`` itself uses :func:`_action_signature` for classification.
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


# How many chars of the tool detail (path / shell command / pattern) we
# show before truncating with "...". Combined with the intent prefix
# this stays well under the supervisor's per-line max_chars budget.
DETAIL_MAX = 60

# Cursor-runner's tool-log emits short verbs like "edit" / "read" /
# "grep". We rewrite them as past-tense capitalized phrases so the
# rollout reads like a journal of completed actions ("Edited /tmp/foo"
# rather than "edit /tmp/foo") even before the LLM swap-in fires.
# Verbs not in this map fall through capitalized + truncated.
_INTENT_VERBS: dict[str, str] = {
    "edit":   "Edited",
    "write":  "Wrote",
    "read":   "Read",
    "delete": "Deleted",
    "grep":   "Searched for",
    "glob":   "Found",
    "web":    "Searched web for",
    "fetch":  "Fetched",
    "task":   "Spawned subagent for",
    "todos":  "Updated todos",
}


def _intent_label(verb: str, detail: str) -> str:
    """Render ``(verb, detail)`` as a short past-tense capitalized phrase.

    Shell commands are rendered with a leading ``Ran $`` and aggressively
    truncated; long compound commands (``a && b && c``) only show the
    first segment so the line stays glanceable.
    """
    verb = (verb or "").strip()
    detail = (detail or "").strip()
    if verb == "$":
        head = detail.split("&&", 1)[0].strip() or detail
        return _truncate(f"Ran $ {head}", DETAIL_MAX + 6)
    intent = _INTENT_VERBS.get(verb)
    if intent is None:
        cap_verb = (verb[:1].upper() + verb[1:]) if verb else ""
        return _truncate(f"{cap_verb} {detail}".strip(), DETAIL_MAX + 8)
    if not detail or detail == "(updated)":
        return intent
    return _truncate(f"{intent} {detail}", DETAIL_MAX + len(intent) + 1)


def _action_signature(snapshot: str) -> tuple[str, str, str]:
    """Classify a worker snapshot into ``(kind, sig_label, display)``.

    The first two elements form a stable signature for dedup -- they do
    not change as a streaming reply grows, and they collapse the
    ``in-progress -> completed`` glyph swap into a single signature so
    the rollout shows one line per logical action. The third element is
    the intent-styled seed text the supervisor will display until (and
    if) the LLM swap-in produces something better.

    cursor_runner's render layout is ``[tool_log lines]\\n\\n[thinking |
    assistant text]``, so we scan all lines and pick:

    * the most recent in-progress tool call (``\u23f3``) if any;
    * else any assistant prose -> ``("answer", "writing reply", ...)``;
    * else the most recent completed tool (``\u2713`` / ``\u2717``);
    * else ``("idle", "thinking", "thinking")``.
    """
    if not snapshot:
        return ("idle", "thinking", "Thinking")
    inflight: Optional[str] = None
    last_completed: Optional[str] = None
    has_assistant = False
    for raw in snapshot.splitlines():
        line = raw.strip()
        if not line:
            continue
        if line.startswith("\u23f3 "):
            inflight = line[2:].strip()
        elif line.startswith("\u2713 ") or line.startswith("\u2717 "):
            last_completed = line[2:].strip()
        elif line.startswith("thinking..."):
            continue
        else:
            has_assistant = True

    def _split(slug: str) -> tuple[str, str]:
        head, _, rest = slug.partition(" ")
        return head, rest

    if inflight:
        verb, detail = _split(inflight)
        return ("tool", inflight, _intent_label(verb, detail))
    if has_assistant:
        return ("answer", "writing reply", "Writing reply")
    if last_completed:
        verb, detail = _split(last_completed)
        return ("tool", last_completed, _intent_label(verb, detail))
    return ("idle", "thinking", "Thinking")


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

    ``kind`` and ``sig_label`` form the entry's *signature* (frozen at
    creation, not touched by LLM swap-ins). ``on_update`` only appends
    a new entry when the snapshot's signature differs from the current
    entry's signature -- this is what stops streaming deltas from
    spamming the rollout.
    """

    started_at: float
    text: str
    kind: str = "tool"
    sig_label: str = ""
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
        initial_text: str = "Starting",
        final_max_chars: int = FINAL_MAX_CHARS,
        stall_hint_after: float = STALL_HINT_AFTER,
        stall_warn_after: float = STALL_WARN_AFTER,
        task_prompt: Optional[str] = None,
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
        self._task_prompt = (task_prompt or "").strip()[:1500]
        self._task_hint_task: Optional[asyncio.Task[None]] = None

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
        self._append_entry(
            self._initial_text, kind="starting", sig_label="starting"
        )
        await self._push()
        self._heartbeat_task = asyncio.create_task(self._heartbeat())
        # Background: ask the LLM for a 2-4 word task description and
        # swap "Starting" -> "Starting <hint>" when it returns. Skipped
        # cleanly when there's no key or no prompt.
        if self._task_prompt and self._client is not None:
            self._task_hint_task = asyncio.create_task(self._fetch_task_hint())
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
        if self._task_hint_task is not None:
            self._task_hint_task.cancel()
            try:
                await self._task_hint_task
            except (asyncio.CancelledError, Exception):
                pass
            self._task_hint_task = None
        if self._client is not None:
            try:
                await self._client.aclose()
            except Exception:
                pass
            self._client = None

    # --- public API ------------------------------------------------------

    async def on_update(self, text: str) -> None:
        """Receive a fresh worker snapshot.

        Classifies the snapshot into a stable ``(kind, sig_label)``
        signature and only appends a new rollout entry when the
        signature represents a real, distinct *tool* action. Streaming
        replies (``answer``) and idle blips between tool calls
        (``idle``) leave no rollout entry -- the actual answer lives in
        the post-run summary bubble, so logging "writing reply" or
        "thinking" here just adds noise (and, for ``answer``, was
        producing duplicate lines because the LLM swap-in described the
        prior tool action).
        """
        if not isinstance(text, str) or text == self._latest_snapshot:
            return
        self._latest_snapshot = text
        self._last_snapshot_at = time.monotonic()
        if not self._first_seen.is_set():
            self._first_seen.set()

        new_kind, new_sig, new_display = _action_signature(text)

        # Drop transient non-tool states. Refresh the current entry's
        # pending_snapshot so any in-flight LLM call still benefits from
        # the latest context, but never append / push.
        if new_kind in ("answer", "idle"):
            cur = self._entries[-1] if self._entries else None
            if cur is not None and not cur.terminal and cur.pending_snapshot is not None:
                cur.pending_snapshot = text
            return

        cur = self._entries[-1] if self._entries else None
        if (
            cur is not None
            and not cur.terminal
            and (cur.kind, cur.sig_label) == (new_kind, new_sig)
        ):
            # Same action; just more bytes streaming through (e.g.
            # in-progress glyph swapping to completed). Keep the
            # current entry; refresh its pending_snapshot so the next
            # heartbeat tick can ask the LLM about the latest text.
            cur.pending_snapshot = text
            self._wake.set()
            return

        self._snapshot_seq += 1
        self._append_entry(
            new_display,
            kind=new_kind,
            sig_label=new_sig,
            pending_snapshot=text,
        )
        await self._push()
        self._wake.set()

    async def on_final(self, text: str) -> None:
        """Stash worker's final answer, append a 'Done' terminal entry,
        and finalise the rollout bubble. The duration shows up as the
        terminal entry's standard M:SS prefix."""
        self._worker_final_text = text or ""
        await self._terminal_finalise("Done")

    async def mark_interrupted(self) -> None:
        await self._terminal_finalise("Interrupted")

    async def mark_crashed(self, error: str) -> None:
        msg = (error or "").strip().splitlines()[0] if error else ""
        line = "Crashed"
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
        kind: str = "tool",
        sig_label: Optional[str] = None,
        terminal: bool = False,
        pending_snapshot: Optional[str] = None,
    ) -> None:
        # Freeze the previous current entry.
        if self._entries:
            self._entries[-1].final = True
            self._entries[-1].pending_snapshot = None
        display = _truncate(text or "thinking", self._max_chars)
        entry = _Entry(
            started_at=time.monotonic(),
            text=display,
            kind=kind,
            sig_label=sig_label if sig_label is not None else display,
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
        # The most recent (still-running, non-terminal) entry uses a
        # *live* timer prefix that ticks up with wall-clock elapsed,
        # so the user can see the action is actually progressing.
        # Once the entry is frozen (a successor was appended), its
        # prefix snaps to its started_at -- "when this action began"
        # relative to the run start.
        is_current_live = (
            self._entries
            and self._entries[-1] is e
            and not e.final
            and not e.terminal
            and not self._finalised
        )
        if is_current_live:
            prefix = _mmss(time.monotonic() - self._started_at)
        else:
            prefix = _mmss(e.started_at - self._started_at)
        # Two-space separator (no faux-bullet glyph). Cleaner alignment
        # and stays out of the way of the actual content. Terminal
        # entries use the same shape so the closing line reads like
        # "0:28  Done" rather than a special-cased footer.
        return f"{prefix}  {e.text}"

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
        self._append_entry(
            line, kind="terminal", sig_label="terminal", terminal=True
        )
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
        if silent_for >= self._stall_warn_after:
            if self._stall_state == _STALL_WARNED:
                return False
            self._stall_state = _STALL_WARNED
            last_action = self._last_real_action_text() or "the same thing"
            self._append_entry(
                f"Stalled? Last seen: {last_action}",
                kind="stall",
                sig_label="stall:warned",
            )
            return True
        if silent_for >= self._stall_hint_after:
            if self._stall_state == _STALL_WAITING:
                return False
            self._stall_state = _STALL_WAITING
            last_action = self._last_real_action_text() or cur.text
            self._append_entry(
                f"{last_action} (waiting...)",
                kind="stall",
                sig_label="stall:waiting",
            )
            return True
        # Below hint threshold: if we previously logged a stall and the
        # worker has spoken again since, that itself appended a fresh
        # entry via on_update -- so we don't need to add a "resolved"
        # line here. Just reset the state machine.
        self._stall_state = _STALL_OK
        return False

    def _last_real_action_text(self) -> str:
        """Walk back through entries to find the last entry that
        represents an actual agent action (tool call or assistant
        prose), skipping starting / stall / terminal markers."""
        for e in reversed(self._entries):
            if e.kind in ("starting", "stall", "terminal"):
                continue
            if e.terminal:
                continue
            return e.text
        return ""

    async def _fetch_task_hint(self) -> None:
        """Background task: ask the LLM for a 2-4 word task description and,
        if the starting entry is still current, rewrite it as
        ``"Starting <hint>"``. Drops cleanly if the entry was already
        frozen by the worker spawning a tool before the LLM returned."""
        try:
            hint = await self._summarise_task(self._task_prompt)
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.exception("supervisor task-hint task crashed")
            return
        if not hint or not self._entries:
            return
        first = self._entries[0]
        if first.kind != "starting" or first.final or first.terminal:
            return
        first.text = _truncate(
            f"{self._initial_text} {hint}", self._max_chars
        )
        try:
            await self._push()
        except Exception:
            logger.exception("supervisor task-hint push failed")

    async def _summarise_task(self, prompt: str) -> Optional[str]:
        """One-shot LLM call: compress the user prompt into a short
        lowercase task phrase. Returns None on any failure."""
        client = self._client
        if client is None or not self._key or not prompt:
            return None
        body = {
            "model": self._model,
            "messages": [
                {"role": "system", "content": TASK_HINT_PROMPT},
                {"role": "user", "content": prompt[:1500]},
            ],
            "max_tokens": 30,
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
            logger.warning("supervisor task-hint request failed: %s", exc)
            return None
        if resp.status_code != 200:
            logger.warning(
                "supervisor task-hint HTTP %s: %s",
                resp.status_code,
                resp.text[:200],
            )
            return None
        try:
            data = resp.json()
            choice = (data.get("choices") or [{}])[0]
            content = ((choice.get("message") or {}).get("content") or "").strip()
        except Exception:
            logger.exception("supervisor task-hint parse failed")
            return None
        if not content:
            return None
        line = " ".join(content.split()).strip().strip('"\'')
        line = line.rstrip(".!?,;:").strip()
        # Defense in depth: strip a leading "starting"-style word so we
        # never render "Starting starting ..." even when the LLM
        # ignores the prompt's no-leading-verb instruction.
        lower = line.lower()
        for prefix in (
            "starting to ", "starting the ", "starting a ", "starting ",
            "starts ", "start ",
            "beginning to ", "beginning the ", "beginning a ", "beginning ",
            "begin ",
        ):
            if lower.startswith(prefix):
                line = line[len(prefix):].strip()
                break
        return line[:60] or None

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
