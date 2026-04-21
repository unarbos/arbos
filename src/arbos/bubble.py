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
"""

from __future__ import annotations

import asyncio
import logging
from typing import Optional

import httpx

logger = logging.getLogger(__name__)


BOT_API = "https://api.telegram.org"

# Telegram caps a text message at 4096 UTF-16 code units. We use byte/char
# count as a conservative proxy and trim with a marker.
MAX_TEXT = 4000

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


def _truncate(text: str, limit: int = MAX_TEXT) -> str:
    if len(text) <= limit:
        return text
    keep = limit - 20
    return text[:keep] + "\n…[truncated]"


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

        self._wake = asyncio.Event()
        self._closed = asyncio.Event()
        self._task: Optional[asyncio.Task[None]] = asyncio.create_task(self._flusher())

    @property
    def message_id(self) -> int:
        return self._message_id

    async def update(self, text: str) -> None:
        """Stage ``text`` as the bubble's next snapshot. Coalesced; cheap."""
        text = _truncate(text)
        if text == self._pending_text:
            return
        self._pending_text = text
        self._wake.set()

    async def finalize(self, text: str) -> None:
        """Cancel the flusher and force one last edit with ``text``.

        Unlike mid-stream edits (which silently defer on 429), the final
        edit is "the answer" -- if Telegram throttles us we wait it out
        once, up to a sane cap, so the user actually sees the result
        instead of a stale "thinking…" or partial chunk.
        """
        text = _truncate(text)
        if self._task is not None:
            self._closed.set()
            self._wake.set()
            try:
                await asyncio.wait_for(self._task, timeout=2.0)
            except (asyncio.TimeoutError, asyncio.CancelledError):
                self._task.cancel()
            self._task = None

        if text == self._last_sent:
            return

        loop = asyncio.get_running_loop()
        global_wait = _GLOBAL_BACKOFF_UNTIL - loop.time()
        if global_wait > 0:
            await asyncio.sleep(min(global_wait, 60.0))

        await self._edit(text)
        if text == self._last_sent:
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
                await self._edit(snapshot)
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.exception("bubble flusher crashed")

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
