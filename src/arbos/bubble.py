"""Single Telegram message bubble with throttled edits.

A :class:`Bubble` wraps a previously-sent Telegram message and lets callers
push new full-text snapshots into it. Edits are coalesced and rate-limited to
at most one ``editMessageText`` call per second per bubble, which is the
practical ceiling Telegram's Bot API gives us for editing the same message
inside a group.

Typical usage from a streaming pipeline::

    bubble = Bubble(http, token, chat_id, thread_id, message_id)
    async for chunk in stream:
        await bubble.update(render(chunk))   # cheap, returns immediately
    await bubble.finalize(render_final())    # one last unconditional edit
    await bubble.aclose()

The flusher coroutine is the only thing that actually talks to Telegram for
mid-stream updates; ``update()`` itself only mutates in-memory state.

Long-running runs whose live snapshot would exceed Telegram's 4096-char
per-message cap are handled transparently: when an ``update()`` or
``finalize()`` would overflow the active bubble, the bubble freezes the
current part with a "[part N · continued ⬇]" footer, sends a brand-new
message in the same thread (as a Telegram reply to the previous part), and
swaps the new message id in as the active bubble. From the caller's point
of view nothing changes -- it keeps calling ``update`` / ``finalize`` and
``message_id`` / ``last_sent`` always reflect the *currently active* part.
Callers that need to mirror per-bubble side state (e.g. the run journal
keyed on message_id) can pass ``on_continuation=cb`` and will be notified
as ``cb(prev_id, new_id)`` on each rollover.
"""

from __future__ import annotations

import asyncio
import logging
from typing import Awaitable, Callable, Optional

import httpx

logger = logging.getLogger(__name__)


BOT_API = "https://api.telegram.org"

# Telegram caps a text message at 4096 UTF-16 code units. We use byte/char
# count as a conservative proxy and trim with a marker.
MAX_TEXT = 4000

# When the live snapshot would overflow MAX_TEXT we no longer truncate-and-
# forget; we freeze the current bubble and spawn a continuation bubble in
# the same Telegram thread, replying to the previous part so the chain is
# visually linked. The frozen part gets this footer so the user immediately
# sees there's more below.
FROZEN_FOOTER = "\n\n[part {n} · continued ⬇]"

# The continuation bubble opens with this banner so the user can see the
# chain at a glance. The {n} placeholder is the new part number (>=2).
CONTINUATION_HEADER = "[part {n} · continued from above]\n\n"

# Headroom below MAX_TEXT reserved for the FROZEN_FOOTER / CONTINUATION_HEADER
# decorations so they never themselves push us over the cap.
DECORATION_BUDGET = 64

# Minimum gap between successive editMessageText calls for one bubble.
# Telegram nominally allows ~1 edit/sec inside a group, but in practice with
# multiple bubbles sharing the same chat (parallel agents, /plan's two
# phases, the outbox watcher) we routinely tripped 20-35s 429 back-offs at
# 1.0s. 2.5s is the sweet spot we settled on after observation: still feels
# responsive, but stays well under the per-chat global ceiling.
EDIT_INTERVAL = 2.5

# When Telegram tells us to back off, we share that floor across *all*
# bubbles in this process so a single 429 doesn't cause every other bubble
# to immediately retry and amplify the throttle. This is updated by any
# bubble that sees a 429 and read by every bubble before each edit.
_GLOBAL_BACKOFF_UNTIL: float = 0.0


# Callback signature for callers that want to mirror per-bubble side state
# (e.g. the run journal) onto each newly-spawned continuation message_id.
# Receives ``(previous_message_id, new_message_id)``. Must not raise; the
# bubble swallows exceptions from this hook so a misbehaving callback can
# never break streaming.
ContinuationCb = Callable[[int, int], Awaitable[None]]


def _truncate(text: str, limit: int = MAX_TEXT) -> str:
    """Single-message truncation. Used for legacy paths that don't roll over.

    Most live edits go through the rollover-aware path in ``update`` /
    ``finalize`` instead, but ``clear_reply_markup`` and similar still
    just need a single capped string.
    """
    if len(text) <= limit:
        return text
    keep = limit - 20
    return text[:keep] + "\n…[truncated]"


def _split_for_rollover(text: str, limit: int = MAX_TEXT) -> tuple[str, Optional[str]]:
    """Split ``text`` into ``(fits_in_current_bubble, overflow_or_none)``.

    The current bubble shows the *head* of the text (so the frozen part
    preserves what the user has been reading), and the overflow becomes
    the seed for the next continuation bubble. The split point prefers a
    paragraph break, then a line break, then a word boundary, then a hard
    cut -- whichever is closest to the cap from below.
    """
    if len(text) <= limit:
        return text, None

    cut = limit - DECORATION_BUDGET
    # Find the latest "nice" boundary at or before the cut.
    head = text[:cut]
    for sep in ("\n\n", "\n", " "):
        idx = head.rfind(sep)
        if idx >= cut // 2:  # don't backtrack so far we lose half the bubble
            head = text[:idx]
            cut = idx + len(sep)
            break
    overflow = text[cut:]
    return head, overflow


class Bubble:
    def __init__(
        self,
        http: httpx.AsyncClient,
        bot_token: str,
        chat_id: int,
        message_thread_id: int,
        message_id: int,
        *,
        edit_interval: float = EDIT_INTERVAL,
        on_continuation: Optional[ContinuationCb] = None,
    ) -> None:
        self._http = http
        self._bot_token = bot_token
        self._chat_id = chat_id
        self._thread_id = message_thread_id
        self._message_id = message_id
        self._edit_interval = edit_interval

        self._pending_text: Optional[str] = None
        self._last_sent: Optional[str] = None
        self._last_edit_at: float = 0.0

        # Rollover bookkeeping. ``_part_index`` is 1 for the very first
        # bubble we were handed; each successful continuation increments
        # it. ``_message_ids`` keeps every part in order so the agent
        # layer can mirror per-bubble state (e.g. the run journal) onto
        # the whole chain.
        self._part_index: int = 1
        self._message_ids: list[int] = [message_id]
        self._on_continuation = on_continuation

        # Lock taken around any operation that mutates _message_id or
        # spawns a continuation bubble. Prevents the flusher and a
        # concurrent finalize() from racing each other into double
        # rollovers.
        self._rollover_lock = asyncio.Lock()

        self._wake = asyncio.Event()
        self._closed = asyncio.Event()
        self._task: Optional[asyncio.Task[None]] = asyncio.create_task(self._flusher())

    @property
    def message_id(self) -> int:
        """The currently-active bubble's id.

        After a rollover this is the *latest* part, which is what callers
        want -- the run-journal entry for "this run" should attach to the
        bubble the user actually sees as the answer. The full chain is
        available via :attr:`message_ids` for callers that need it.
        """
        return self._message_id

    @property
    def message_ids(self) -> list[int]:
        """All Telegram message ids in this bubble's continuation chain.

        Index 0 is the original bubble, last element is the active one.
        Returned as a copy so callers can't mutate our internal list.
        """
        return list(self._message_ids)

    @property
    def last_sent(self) -> Optional[str]:
        """The most recent text the *currently-active* bubble displayed.

        After a rollover this is the text shown in the latest part, not
        an aggregation across all parts. Used by the run journal to
        capture "what the user saw" without having to thread the final
        text through every code path that calls ``finalize``.
        """
        return self._last_sent

    async def update(self, text: str) -> None:
        """Stage ``text`` as the bubble's next snapshot. Coalesced; cheap.

        If ``text`` would overflow Telegram's per-message cap, the bubble
        rolls over: the current part is frozen with a "[continued ⬇]"
        footer and a new bubble is sent in the same thread (as a
        Telegram reply to the previous part) to host the overflow plus
        all subsequent edits. Live streaming continues seamlessly into
        the new bubble.
        """
        if len(text) <= MAX_TEXT:
            if text == self._pending_text:
                return
            self._pending_text = text
            self._wake.set()
            return

        # Overflow path. Roll over under the lock so a concurrent
        # finalize() can't double-spawn.
        async with self._rollover_lock:
            await self._roll_over(text)

    async def finalize(self, text: str) -> None:
        """Cancel the flusher and force one last edit with ``text``.

        Unlike mid-stream edits (which silently defer on 429), the final
        edit is "the answer" -- if Telegram throttles us we wait it out
        once, up to a sane cap, so the user actually sees the result
        instead of a stale "thinking…" or partial chunk.

        Rollover-aware: if ``text`` doesn't fit in the active bubble it
        will be split across as many continuation bubbles as needed so
        the *full* answer is preserved across the chain. The active
        bubble (the last part) holds the tail, which is the conventional
        answer location and what later replies will attach to.
        """
        if self._task is not None:
            self._closed.set()
            self._wake.set()
            try:
                await asyncio.wait_for(self._task, timeout=2.0)
            except (asyncio.TimeoutError, asyncio.CancelledError):
                self._task.cancel()
            self._task = None

        # Roll over enough times to fit the full final answer.
        # After each _roll_over: _last_sent reflects what the new
        # active bubble currently shows (the prefix that fit), and
        # _pending_text holds the *full intended* render for the new
        # bubble (if it didn't all fit in one shot) or None.
        async with self._rollover_lock:
            while len(text) > MAX_TEXT:
                await self._roll_over(text)
                pending = self._pending_text
                if pending is None or len(pending) <= MAX_TEXT:
                    # The active bubble can hold either the rest in one
                    # shot (pending None == full_seed already shown) or
                    # a small remainder. Either way, the loop is done.
                    text = pending if pending is not None else (self._last_sent or "")
                    break
                text = pending

        if text == self._last_sent:
            self._pending_text = None
            return

        loop = asyncio.get_running_loop()
        global_wait = _GLOBAL_BACKOFF_UNTIL - loop.time()
        if global_wait > 0:
            await asyncio.sleep(min(global_wait, 60.0))

        await self._edit(text)
        if text == self._last_sent:
            self._pending_text = None
            return
        wait = _GLOBAL_BACKOFF_UNTIL - loop.time()
        if wait > 0:
            await asyncio.sleep(min(wait, 60.0))
            await self._edit(text)

    async def aclose(self) -> None:
        """Best-effort cleanup; safe to call after :meth:`finalize`."""
        self._closed.set()
        self._wake.set()
        if self._task is not None:
            self._task.cancel()
            try:
                await self._task
            except (asyncio.CancelledError, Exception):
                pass
            self._task = None

    async def clear_reply_markup(self) -> None:
        """Strip any inline keyboard from the bubble. Safe if there isn't one.

        Used after a confirmation callback fires so the (now-stale) buttons
        disappear before we proceed with the wrapped action.
        """
        url = f"{BOT_API}/bot{self._bot_token}/editMessageReplyMarkup"
        params = {
            "chat_id": self._chat_id,
            "message_id": self._message_id,
            "reply_markup": {"inline_keyboard": []},
        }
        try:
            resp = await self._http.post(url, json=params)
        except httpx.HTTPError as exc:
            logger.warning("editMessageReplyMarkup network error: %s", exc)
            return
        if resp.status_code != 200:
            body = resp.text[:300]
            if "message is not modified" in body:
                return
            logger.warning(
                "editMessageReplyMarkup HTTP %d: %s", resp.status_code, body
            )

    async def _flusher(self) -> None:
        loop = asyncio.get_running_loop()
        try:
            while not self._closed.is_set():
                await self._wake.wait()
                self._wake.clear()
                if self._closed.is_set():
                    return

                # Respect the per-bubble rate limit AND any process-wide
                # back-off another bubble negotiated for us via 429.
                now = loop.time()
                local_wait = self._edit_interval - (now - self._last_edit_at)
                global_wait = _GLOBAL_BACKOFF_UNTIL - now
                wait = max(local_wait, global_wait, 0.0)
                if wait > 0:
                    try:
                        await asyncio.wait_for(self._closed.wait(), timeout=wait)
                        return  # closed during the wait
                    except asyncio.TimeoutError:
                        pass

                snapshot = self._pending_text
                if snapshot is None or snapshot == self._last_sent:
                    continue
                if len(snapshot) > MAX_TEXT:
                    # Took the long path: a previously-staged short
                    # snapshot was replaced by an oversized one between
                    # ticks. Roll over to make room, then re-loop so the
                    # next iteration sends whatever's left as the new
                    # active bubble's first edit.
                    async with self._rollover_lock:
                        await self._roll_over(snapshot)
                    self._wake.set()
                    continue
                await self._edit(snapshot)
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.exception("bubble flusher crashed")

    async def _roll_over(self, full_text: str) -> None:
        """Freeze the current bubble and spawn a continuation in the same thread.

        Splits ``full_text`` into ``(visible_now, overflow)`` at the
        nicest boundary that fits in the current bubble. Edits the
        current bubble one last time with ``visible_now`` plus a
        "[part N · continued ⬇]" footer (if it fits), sends a brand new
        Telegram message in this thread as a reply to the (now-frozen)
        previous part, and swaps it in as the new active bubble. The
        ``overflow`` becomes the new bubble's initial pending text.

        Caller must hold ``self._rollover_lock``.
        """
        head, overflow = _split_for_rollover(full_text)
        if overflow is None:
            # No-op safety: caller miscounted; treat as a normal edit.
            self._pending_text = head
            self._wake.set()
            return

        next_part = self._part_index + 1
        frozen = head + FROZEN_FOOTER.format(n=self._part_index)
        if len(frozen) > MAX_TEXT:
            # Fall back to bare head if the footer somehow tipped us
            # over (shouldn't happen given DECORATION_BUDGET, but be
            # defensive -- losing the footer is better than a 400).
            frozen = head

        # Commit the freeze edit on the *current* bubble. We don't care
        # if Telegram says "not modified" -- the important thing is that
        # _edit updates _last_sent so we never try to overwrite the
        # frozen text again.
        await self._edit(frozen)

        # Build what the new bubble should display. Conceptually:
        #   full_seed = CONTINUATION_HEADER + overflow
        # If full_seed fits in one bubble we send it as-is and we're
        # done. If not, we send only what fits now and stash full_seed
        # as _pending_text so the *next* roll-over (driven by the
        # finalize loop or the flusher) keeps splitting it down.
        header = CONTINUATION_HEADER.format(n=next_part)
        full_seed = header + overflow
        if len(full_seed) <= MAX_TEXT:
            sent_now = full_seed
        else:
            # Cut at a nice boundary inside the overflow so the user
            # doesn't see an ugly mid-word break on the new bubble.
            sent_now, _leftover = _split_for_rollover(full_seed)

        new_id = await self._send_continuation(
            reply_to=self._message_id, text=sent_now
        )
        if new_id is None:
            # Couldn't roll over. Mark the current bubble as full so we
            # at least stop trying to edit it with bigger and bigger
            # snapshots. This is a degraded state but not fatal.
            self._pending_text = None
            return

        # Swap the active bubble.
        prev_id = self._message_id
        self._message_id = new_id
        self._message_ids.append(new_id)
        self._part_index = next_part
        # Reset edit-rate state for the new bubble. Telegram tracks the
        # 1 edit/sec ceiling per message_id, so we get a fresh budget.
        self._last_edit_at = 0.0
        # _last_sent must match exactly what Telegram is currently
        # showing on the new bubble -- otherwise dedupe in update() /
        # the flusher will misfire.
        self._last_sent = sent_now
        # If the full intended seed didn't fit, stash it for the next
        # tick. The flusher / finalize loop will see _pending_text >
        # MAX_TEXT and roll over again. If it did fit, nothing pending.
        if len(full_seed) > len(sent_now):
            self._pending_text = full_seed
            self._wake.set()
        else:
            self._pending_text = None

        # Notify the agent layer so per-bubble side state (run journal,
        # inflight, etc.) can mirror onto the new id. Hook is best-effort.
        cb = self._on_continuation
        if cb is not None:
            try:
                await cb(prev_id, new_id)
            except Exception:
                logger.exception("on_continuation hook raised; ignoring")

    async def _send_continuation(
        self, *, reply_to: int, text: str
    ) -> Optional[int]:
        """sendMessage in this thread, replying to ``reply_to``. Returns new id or None."""
        url = f"{BOT_API}/bot{self._bot_token}/sendMessage"
        # Pre-emptively respect the global back-off so we don't 429 the
        # first edit on the new bubble.
        loop = asyncio.get_running_loop()
        global_wait = _GLOBAL_BACKOFF_UNTIL - loop.time()
        if global_wait > 0:
            await asyncio.sleep(min(global_wait, 30.0))
        params: dict[str, object] = {
            "chat_id": self._chat_id,
            "message_thread_id": self._thread_id,
            "text": text if len(text) <= MAX_TEXT else text[:MAX_TEXT],
            "disable_web_page_preview": True,
            "reply_parameters": {
                "message_id": reply_to,
                "allow_sending_without_reply": True,
            },
        }
        try:
            resp = await self._http.post(url, json=params)
        except httpx.HTTPError as exc:
            logger.warning("bubble continuation sendMessage network error: %s", exc)
            return None
        if resp.status_code != 200:
            logger.warning(
                "bubble continuation sendMessage HTTP %d: %s",
                resp.status_code,
                resp.text[:300],
            )
            return None
        body = resp.json()
        if not body.get("ok"):
            logger.warning("bubble continuation sendMessage not ok: %r", body)
            return None
        result = body.get("result") or {}
        new_id = result.get("message_id")
        if not isinstance(new_id, int):
            return None
        logger.info(
            "bubble rolled over chat=%s thread=%s prev=%s new=%s part=%d",
            self._chat_id,
            self._thread_id,
            reply_to,
            new_id,
            self._part_index + 1,
        )
        return new_id

    async def _edit(self, text: str) -> None:
        global _GLOBAL_BACKOFF_UNTIL
        url = f"{BOT_API}/bot{self._bot_token}/editMessageText"
        params = {
            "chat_id": self._chat_id,
            "message_id": self._message_id,
            "text": text,
            "disable_web_page_preview": True,
        }
        try:
            resp = await self._http.post(url, json=params)
        except httpx.HTTPError as exc:
            logger.warning("editMessageText network error: %s", exc)
            return

        if resp.status_code == 200:
            self._last_sent = text
            self._last_edit_at = asyncio.get_running_loop().time()
            return

        if resp.status_code == 429:
            # Don't burn this bubble's loop sleeping for 30s -- mark a
            # process-wide floor so every other bubble also waits, then
            # bail out. The flusher will retry the *latest* snapshot
            # after the floor lifts (we don't want to send stale text
            # we've already overwritten in-memory).
            try:
                retry_after = float(
                    resp.json().get("parameters", {}).get("retry_after", 1.0)
                )
            except Exception:
                retry_after = 1.0
            # Pad slightly so we don't immediately re-trip the limit.
            until = asyncio.get_running_loop().time() + retry_after + 0.5
            if until > _GLOBAL_BACKOFF_UNTIL:
                _GLOBAL_BACKOFF_UNTIL = until
            logger.info(
                "bubble edit 429; global back-off for %.2fs (deferring this snapshot)",
                retry_after,
            )
            # Re-arm the flusher so it sleeps until the floor lifts and
            # then sends whatever the latest pending text is.
            self._wake.set()
            return

        body = resp.text[:300]
        # "message is not modified" is harmless; treat as success.
        if "message is not modified" in body:
            self._last_sent = text
            self._last_edit_at = asyncio.get_running_loop().time()
            return
        logger.warning("editMessageText HTTP %d: %s", resp.status_code, body)
