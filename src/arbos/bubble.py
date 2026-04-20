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
EDIT_INTERVAL = 1.0


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
        """Cancel the flusher and force one last edit with ``text``."""
        text = _truncate(text)
        # Stop the loop first so it cannot race us with a stale snapshot.
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

                # Respect the per-bubble rate limit.
                now = loop.time()
                wait = self._edit_interval - (now - self._last_edit_at)
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
        url = f"{BOT_API}/bot{self._bot_token}/editMessageText"
        params = {
            "chat_id": self._chat_id,
            "message_id": self._message_id,
            "text": text,
            "disable_web_page_preview": True,
        }
        for attempt in range(3):
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
                try:
                    retry_after = float(resp.json().get("parameters", {}).get("retry_after", 1.0))
                except Exception:
                    retry_after = 1.0
                logger.info("bubble edit 429; sleeping %.2fs", retry_after)
                await asyncio.sleep(retry_after)
                continue

            body = resp.text[:300]
            # "message is not modified" is harmless; treat as success.
            if "message is not modified" in body:
                self._last_sent = text
                self._last_edit_at = asyncio.get_running_loop().time()
                return
            logger.warning("editMessageText HTTP %d: %s", resp.status_code, body)
            return
