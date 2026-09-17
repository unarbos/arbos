"""A stand-in for OpenAI GPT-Live: the session.start protocol `openai_live.py` speaks, no network.

Records the instructions and the startup history (`session.input`) so a harness can assert what
the model was told. Accepts audio and appends; answers `session.started` and `session.closed`.
Does not speak and does not create delegations — this mock is for context, not for talk.
"""

from __future__ import annotations

import json
import logging
import uuid

from http import HTTPStatus
from websockets.asyncio.server import ServerConnection, serve

log = logging.getLogger("mock.openai")


class MockOpenAILive:
    def __init__(self, host: str = "127.0.0.1", port: int = 0):
        self.host, self.port = host, port
        self.instructions = ""
        self.input: list[dict] = []
        self.appends: list[dict] = []
        self.started = 0
        self._server = None
        self._ws: ServerConnection | None = None

    async def start(self) -> str:
        self._server = await serve(self._handler, self.host, self.port, process_request=self._http,
                                   max_size=16 * 1024 * 1024, compression=None)
        self.port = self._server.sockets[0].getsockname()[1]
        return f"ws://{self.host}:{self.port}/v1/live/sessions"

    async def stop(self) -> None:
        if self._server:
            self._server.close()
            await self._server.wait_closed()

    def input_text(self) -> str:
        parts: list[str] = []
        for item in self.input:
            for block in item.get("content") or []:
                if isinstance(block, dict) and block.get("text"):
                    parts.append(str(block["text"]))
        return "\n".join(parts)

    def _http(self, connection: ServerConnection, request):
        if request.path in ("/", "/healthz"):
            return connection.respond(HTTPStatus.OK, "ok")
        return None

    async def _handler(self, ws: ServerConnection) -> None:
        self._ws = ws
        try:
            async for raw in ws:
                try:
                    msg = json.loads(raw)
                except json.JSONDecodeError:
                    continue
                kind = msg.get("type")
                if kind == "session.start":
                    session = msg.get("session") or {}
                    self.instructions = str(session.get("instructions") or "")
                    self.input = list(session.get("input") or [])
                    self.started += 1
                    await self._send({"type": "session.started", "session": {"id": "mock-live"}})
                elif kind in ("session.thinking.append", "session.commentary.append", "session.instructions.append"):
                    self.appends.append(msg)
                elif kind == "session.close":
                    await self._send({"type": "session.closed", "usage": {"seconds": 0}})
                    break
        finally:
            self._ws = None

    async def _send(self, msg: dict) -> None:
        if self._ws is None:
            return
        msg.setdefault("event_id", str(uuid.uuid4()))
        try:
            await self._ws.send(json.dumps(msg))
        except Exception:
            pass
