"""A stand-in for OpenAI GPT-Live: the session.start protocol `openai_live.py` speaks, no network.

Records the instructions and the startup history (`session.input`) so a harness can assert what
the model was told. Accepts audio and appends; answers `session.started` and `session.closed`.

Scripted talk, when a scenario asks for it (`live = { self_answer = "…", answer_seconds = 6 }`):
about a second after the caller's audio goes loud the mock speaks `self_answer` — a tone with the
transcript leading it, as the real model streams — without creating a delegation, the shape of
GPT-Live answering a work question from its stores. Every `session.commentary.append` it receives
is spoken the same way, as GPT-Live speaks the kernel's answer. It never creates delegations.
"""

from __future__ import annotations

import asyncio
import base64
import json
import logging
import time
import uuid

import numpy as np

from http import HTTPStatus
from websockets.asyncio.server import ServerConnection, serve

RATE = 24_000
CHUNK_MS = 80
CHUNK = RATE * CHUNK_MS // 1000
LOUD = 0.02  # RMS of a caller chunk that counts as speech (the harness's utterances are ~0.2)

log = logging.getLogger("mock.openai")


class MockOpenAILive:
    def __init__(self, host: str = "127.0.0.1", port: int = 0, *, self_answer: str = "",
                 answer_after: float = 1.0, answer_seconds: float = 6.0):
        self.host, self.port = host, port
        self.instructions = ""
        self.input: list[dict] = []
        self.appends: list[dict] = []
        self.started = 0
        self.self_answer = self_answer
        self.answer_after = answer_after
        self.answer_seconds = answer_seconds
        self.spoken: list[str] = []  # what the mock said, whole, in order
        self.speak_events: list[tuple[float, str]] = []
        self._server = None
        self._ws: ServerConnection | None = None
        self._heard_loud_at: float | None = None
        self._talk: asyncio.Task | None = None
        # One voice: a reply waits for the one before it to end, as the real model speaks.
        self._mouth = asyncio.Lock()

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
                elif kind == "session.input_audio.append" and self.self_answer:
                    pcm = np.frombuffer(base64.b64decode(msg.get("audio", "")), dtype="<i2").astype(np.float32) / 32768.0
                    loud = pcm.size > 0 and float(np.sqrt(np.mean(pcm * pcm))) > LOUD
                    if loud and self._heard_loud_at is None:
                        self._heard_loud_at = time.monotonic()
                        self._talk = asyncio.create_task(self._answer_after(self.self_answer))
                elif kind in ("session.thinking.append", "session.commentary.append", "session.instructions.append"):
                    self.appends.append(msg)
                    if kind == "session.commentary.append":
                        # GPT-Live speaks the kernel's answer in its own voice.
                        asyncio.create_task(self._speak(str(msg.get("content") or ""), 3.0))
                elif kind == "session.close":
                    await self._send({"type": "session.closed", "usage": {"seconds": 0}})
                    break
        finally:
            self._ws = None

    async def _answer_after(self, text: str) -> None:
        await asyncio.sleep(self.answer_after)
        await self._speak(text, self.answer_seconds)

    async def _speak(self, text: str, seconds: float) -> None:
        """A 180 Hz tone for `seconds`, the transcript leading it in pieces, real time. One
        reply at a time: the kernel's answer, appended while the mock is mid-sentence, is
        spoken after it, never over it."""
        async with self._mouth:
            await self._speak_now(text, seconds)

    async def _speak_now(self, text: str, seconds: float) -> None:
        words = text.split()
        frames = max(1, int(seconds * 1000 / CHUNK_MS))
        per_frame = max(1, len(words) // max(1, frames // 2))
        self.speak_events.append((time.monotonic(), "start"))
        spoken: list[str] = []
        for i in range(frames):
            if i % 2 == 0 and (i // 2) * per_frame < len(words):
                piece = " ".join(words[(i // 2) * per_frame : (i // 2 + 1) * per_frame])
                if piece:
                    spoken.append(piece)
                    await self._send({"type": "session.output_transcript.delta", "delta": piece + " "})
            t = (np.arange(CHUNK) + i * CHUNK) / RATE
            pcm = (0.25 * np.sin(2 * np.pi * 180.0 * t) * 32767).astype("<i2").tobytes()
            await self._send({"type": "session.output_audio.delta", "delta": base64.b64encode(pcm).decode()})
            await asyncio.sleep(CHUNK_MS / 1000)
        self.spoken.append(text)
        self.speak_events.append((time.monotonic(), "end"))

    async def _send(self, msg: dict) -> None:
        if self._ws is None:
            return
        msg.setdefault("event_id", str(uuid.uuid4()))
        try:
            await self._ws.send(json.dumps(msg))
        except Exception:
            pass
