"""The relay as the worker sees it: call info, tool calls, events, usage, and the decision proxy."""

from __future__ import annotations

import asyncio
import json
import logging
import time
from typing import Any

import aiohttp

from .config import Config

log = logging.getLogger("arbos-voice.relay")


class RelayError(Exception):
    def __init__(self, status: int, body: Any):
        super().__init__(f"relay HTTP {status}: {body}")
        self.status = status
        self.body = body


class ToolPending(Exception):
    """The box did not finish within the relay's wait; the result will not arrive on this request."""


class Relay:
    def __init__(self, cfg: Config, session: aiohttp.ClientSession):
        self.cfg = cfg
        self.http = session
        self._headers = {"authorization": f"Bearer {cfg.relay_token}", "content-type": "application/json"}

    async def _json(self, method: str, path: str, body: Any = None, *, timeout: float = 30.0, headers: dict | None = None) -> Any:
        h = dict(self._headers)
        if headers:
            h.update(headers)
        async with self.http.request(
            method, f"{self.cfg.relay_url}{path}", json=body, headers=h, timeout=aiohttp.ClientTimeout(total=timeout)
        ) as r:
            text = await r.text()
            try:
                data = json.loads(text) if text else {}
            except Exception:
                data = {"raw": text}
            if r.status >= 400:
                raise RelayError(r.status, data)
            return data

    async def call_info(self, call_id: str, project_id: str) -> dict:
        return await self._json("GET", f"/voice/calls/{call_id}?project_id={project_id}", timeout=12.0)

    async def tool(
        self,
        call_id: str,
        project_id: str,
        *,
        speaker: dict,
        request: str,
        mode: str,
        request_id: str,
        attachments: list[dict] | None = None,
    ) -> dict:
        """Runs until the agent finishes (minutes, for long work). Raises ToolPending on 504.
        `attachments` = `[{name, mime, ref}]` blobs already uploaded with `put_blob` (camera frames)."""
        body = {"project_id": project_id, "speaker": speaker, "request": request, "mode": mode, "request_id": request_id}
        if attachments:
            body["attachments"] = attachments
        try:
            return await self._json("POST", f"/voice/calls/{call_id}/tool", body, timeout=300.0)
        except RelayError as e:
            if e.status == 504:
                raise ToolPending() from e
            raise

    async def put_blob(self, slot: str, key: str, data: bytes, mime: str) -> None:
        """`PUT /blob/{slot}/{key}`: the chat's blob store (project id, or `people:<chat_id>`)."""
        async with self.http.put(
            f"{self.cfg.relay_url}/blob/{slot}/{key}",
            data=data,
            headers={"authorization": self._headers["authorization"], "content-type": mime},
            timeout=aiohttp.ClientTimeout(total=20.0),
        ) as r:
            if r.status >= 400:
                raise RelayError(r.status, await r.text())

    async def inbox(self, call_id: str, project_id: str, after: int, wait: int) -> dict:
        """`GET /voice/calls/{id}/inbox` (#1832): what landed in the chat since `after`, long-polled."""
        return await self._json(
            "GET",
            f"/voice/calls/{call_id}/inbox?project_id={project_id}&after={after}&wait={wait}",
            timeout=wait + 10.0,
        )

    async def agent_settings(self, project_id: str) -> dict:
        """`GET /projects/{id}/voice` — chattiness, wake name, voice, vision_enabled (live re-read)."""
        return await self._json("GET", f"/projects/{project_id}/voice", timeout=5.0)

    async def events(self, call_id: str, project_id: str, events: list[dict]) -> None:
        if not events:
            return
        try:
            await self._json("POST", f"/voice/calls/{call_id}/events", {"project_id": project_id, "events": events}, timeout=10.0)
        except Exception as e:  # events are best effort; never stall the audio path
            log.warning("events post failed: %s", e)

    async def usage(self, call_id: str, project_id: str, usage: dict) -> dict:
        body = {"project_id": project_id, **usage}
        return await self._json("POST", f"/voice/calls/{call_id}/usage", body, timeout=10.0)

    async def decide(self, project_id: str, model: str, state: dict, questions: dict, *, timeout: float = 3.0) -> dict:
        """`POST /proxy/decisions` (TypeSafe Jev on OpenRouter), metered to the agent's project."""
        return await self._json(
            "POST",
            "/proxy/decisions",
            {"model": model, "state": state, "questions": questions},
            timeout=timeout,
            headers={"x-arbos-project": project_id},
        )

    async def chat_json(self, project_id: str, model: str, system: str, user: str, *, timeout: float = 8.0) -> tuple[dict, dict]:
        """One JSON-mode chat completion through the relay proxy; returns (parsed, usage)."""
        body = {
            "model": model,
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
            "temperature": 0,
            "max_tokens": 300,
            "response_format": {"type": "json_object"},
        }
        data = await self._json("POST", "/proxy/v1/chat/completions", body, timeout=timeout, headers={"x-arbos-project": project_id})
        content = data.get("choices", [{}])[0].get("message", {}).get("content", "{}")
        start = content.find("{")
        end = content.rfind("}")
        parsed = json.loads(content[start : end + 1]) if start >= 0 and end > start else {}
        return parsed, data.get("usage", {}) or {}


class EventBatcher:
    """Collects events and flushes them to the relay every `every_s` or when `flush()` is called."""

    def __init__(self, relay: Relay, call_id: str, project_id: str, every_s: float = 1.0):
        self.relay = relay
        self.call_id = call_id
        self.project_id = project_id
        self.every_s = every_s
        self._queue: list[dict] = []
        self._task: asyncio.Task | None = None
        self._closed = False

    def start(self) -> None:
        self._task = asyncio.create_task(self._loop())

    def push(self, event: dict) -> None:
        event.setdefault("at", int(time.time() * 1000))
        self._queue.append(event)

    async def flush(self) -> None:
        if not self._queue:
            return
        batch, self._queue = self._queue, []
        await self.relay.events(self.call_id, self.project_id, batch)

    async def _loop(self) -> None:
        while not self._closed:
            await asyncio.sleep(self.every_s)
            await self.flush()

    async def close(self) -> None:
        self._closed = True
        if self._task:
            self._task.cancel()
        await self.flush()
