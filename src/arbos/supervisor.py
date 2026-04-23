"""Compact-summary layer between :class:`CursorRunner` and :class:`Bubble`.

The worker (cursor-agent) emits a fresh full-snapshot to ``on_update`` on
every event -- tool log, thinking buffer, partial assistant text. Pushed
straight into the bubble that produces the noisy "wall of stream" the user
sees today.

The supervisor sits in front of the bubble. It buffers the worker's latest
snapshot in memory and, on a fixed cadence (default 2s), asks a small LLM
for a single ``<= max_chars`` line describing what the agent is currently
doing. Only that line is forwarded to the bubble, so the chat stays
glanceable mid-run. When the worker finishes, the supervisor stops the
ticker and forwards the worker's *final* answer to ``bubble.finalize`` so
the chat ends with the actual response (not a summary of it).

Failure modes (no key, HTTP error, timeout, model-side error) degrade
gracefully: that tick falls back to forwarding the raw worker snapshot to
the bubble, so we never go silent. A warning is logged and the next tick
retries.

Usage::

    async with Supervisor(bubble=bubble, openai_api_key=key) as sup:
        await runner.run(on_update=sup.on_update, on_final=sup.on_final)
"""

from __future__ import annotations

import asyncio
import logging
from typing import Any, Optional

import httpx

logger = logging.getLogger(__name__)


OPENAI_CHAT_URL = "https://api.openai.com/v1/chat/completions"

# Hard cap on what we ship to the model as "current state". The worker's
# rendered snapshot can reach ~3500 chars; we keep the tail (latest tool
# calls + most recent assistant tokens) since that's what describes "now".
SNAPSHOT_TAIL = 2000

# Per-request budget for the OpenAI call. Must be well under the bubble's
# 2.5s edit floor so a slow request doesn't pile up across ticks.
OPENAI_TIMEOUT = 8.0

DEFAULT_SYSTEM_PROMPT = (
    "You are a terse status reporter for an autonomous coding agent. "
    "Given the agent's most recent visible activity (tool calls, thinking, "
    "partial answer), reply with exactly ONE line, present tense, that "
    "describes what the agent is doing right now. Hard limit: {max_chars} "
    "characters. No preamble, no quotes, no markdown, no trailing period if "
    "you are tight on space. If the agent appears idle, say so."
)


def _truncate(text: str, n: int) -> str:
    if len(text) <= n:
        return text
    return text[: n - 1] + "…"


class Supervisor:
    """Throttle worker snapshots through a small LLM for a compact bubble."""

    def __init__(
        self,
        *,
        bubble: Any,
        openai_api_key: Optional[str],
        model: str = "gpt-4o-mini",
        interval: float = 2.0,
        max_chars: int = 250,
        initial_text: str = "thinking…",
        system_prompt: Optional[str] = None,
    ) -> None:
        self._bubble = bubble
        self._key = (openai_api_key or "").strip()
        self._model = model
        self._interval = max(0.25, float(interval))
        self._max_chars = max(40, int(max_chars))
        self._initial_text = initial_text
        self._system_prompt = (system_prompt or DEFAULT_SYSTEM_PROMPT).format(
            max_chars=self._max_chars
        )

        self._latest_snapshot: str = ""
        self._last_summarised: str = ""
        self._dirty: bool = False
        self._stopped = asyncio.Event()
        self._ticker_task: Optional[asyncio.Task[None]] = None
        self._client: Optional[httpx.AsyncClient] = None
        self._inflight_lock = asyncio.Lock()

    async def __aenter__(self) -> "Supervisor":
        if self._key:
            self._client = httpx.AsyncClient(timeout=OPENAI_TIMEOUT)
            self._ticker_task = asyncio.create_task(self._ticker())
        return self

    async def __aexit__(self, exc_type, exc, tb) -> None:
        self._stopped.set()
        if self._ticker_task is not None:
            self._ticker_task.cancel()
            try:
                await self._ticker_task
            except (asyncio.CancelledError, Exception):
                pass
            self._ticker_task = None
        if self._client is not None:
            try:
                await self._client.aclose()
            except Exception:
                pass
            self._client = None

    async def on_update(self, text: str) -> None:
        """Receive a fresh worker snapshot. Returns immediately."""
        if not isinstance(text, str):
            return
        if text == self._latest_snapshot:
            return
        self._latest_snapshot = text
        self._dirty = True
        # No supervisor (no key / disabled): pass straight through so the
        # bubble still updates on every event, matching the legacy path.
        if self._client is None:
            try:
                await self._bubble.update(text)
            except Exception:
                logger.exception("bubble.update raised in passthrough")

    async def on_final(self, text: str) -> None:
        """Stop the ticker, then push the worker's final answer to the bubble."""
        self._stopped.set()
        if self._ticker_task is not None:
            self._ticker_task.cancel()
            try:
                await self._ticker_task
            except (asyncio.CancelledError, Exception):
                pass
            self._ticker_task = None
        try:
            await self._bubble.finalize(text)
        except Exception:
            logger.exception("bubble.finalize raised")

    async def _ticker(self) -> None:
        try:
            while not self._stopped.is_set():
                try:
                    await asyncio.wait_for(
                        self._stopped.wait(), timeout=self._interval
                    )
                    return
                except asyncio.TimeoutError:
                    pass
                if not self._dirty:
                    continue
                snapshot = self._latest_snapshot
                if snapshot == self._last_summarised:
                    self._dirty = False
                    continue
                self._dirty = False
                self._last_summarised = snapshot
                async with self._inflight_lock:
                    summary = await self._summarise(snapshot)
                if not summary:
                    continue
                try:
                    await self._bubble.update(summary)
                except Exception:
                    logger.exception("bubble.update raised in supervisor tick")
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.exception("supervisor ticker crashed; falling back to passthrough")
            try:
                await self._bubble.update(self._latest_snapshot or self._initial_text)
            except Exception:
                pass

    async def _summarise(self, snapshot: str) -> Optional[str]:
        """Call OpenAI for a single-line summary. Returns ``None`` if we
        couldn't get one (caller skips the bubble update for this tick)."""
        client = self._client
        if client is None or not self._key:
            return _truncate(snapshot, self._max_chars) if snapshot else None

        body = {
            "model": self._model,
            "messages": [
                {"role": "system", "content": self._system_prompt},
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
            logger.warning("supervisor openai request failed: %s", exc)
            return _truncate(snapshot, self._max_chars) if snapshot else None
        if resp.status_code != 200:
            logger.warning(
                "supervisor openai HTTP %s: %s",
                resp.status_code,
                resp.text[:200],
            )
            return _truncate(snapshot, self._max_chars) if snapshot else None
        try:
            data = resp.json()
            choice = (data.get("choices") or [{}])[0]
            content = ((choice.get("message") or {}).get("content") or "").strip()
        except Exception:
            logger.exception("supervisor openai parse failed")
            return None
        if not content:
            return None
        # Collapse to a single line, hard-cap to the bubble's budget.
        line = " ".join(content.split())
        return _truncate(line, self._max_chars)
