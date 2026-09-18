"""A stand-in for OpenAI GPT-Live: the session.start protocol `openai_live.py` speaks, no network.

Records the instructions and the startup history (`session.input`) so a harness can assert what
the model was told. Accepts audio and appends; answers `session.started` and `session.closed`.

It can also talk, when scripted (`script`: one `Reply` per caller utterance): it hears the end of
an utterance in the uplink audio (loud, then quiet), waits `after` seconds, and speaks — audio
frames (a tone) plus `session.output_transcript.delta` words — the way the real model answers on
its own. With `delegate` it raises `session.delegation.created` first. Every
`session.commentary.append` is spoken back after a beat, as the real model paraphrases one.
`spoken` is everything it said, in order, so a scenario can tell the model's own answer from the
kernel's.
"""

from __future__ import annotations

import asyncio
import base64
import json
import logging
import math
import struct
import time
import uuid
from dataclasses import dataclass

from http import HTTPStatus
from websockets.asyncio.server import ServerConnection, serve

log = logging.getLogger("mock.openai")

RATE = 24_000
FRAME_MS = 100
LOUD = 0.01  # RMS above this in the uplink is the caller talking
END_QUIET_S = 0.5  # this much quiet after talk is the end of an utterance
WORD_S = 0.28  # spoken pace of the tone reply


@dataclass
class Reply:
    say: str = ""
    after: float = 0.5  # seconds after the utterance ends before the model speaks
    delegate: bool = False  # raise session.delegation.created for this utterance


class MockOpenAILive:
    def __init__(self, host: str = "127.0.0.1", port: int = 0):
        self.host, self.port = host, port
        self.instructions = ""
        self.input: list[dict] = []
        self.appends: list[dict] = []
        self.started = 0
        self.script: list[Reply] = []
        self.spoken: list[str] = []  # what the model said, in order (its own replies and commentary)
        self.delegations = 0
        self._server = None
        self._ws: ServerConnection | None = None
        self._talking = False
        self._loud_since: float | None = None
        self._last_loud: float = 0.0
        self._speak_lock = asyncio.Lock()

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
                elif kind == "session.input_audio.append":
                    self._hear(base64.b64decode(msg.get("audio", "")))
                elif kind in ("session.thinking.append", "session.commentary.append", "session.instructions.append"):
                    self.appends.append(msg)
                    if kind == "session.commentary.append" and str(msg.get("content") or "").strip():
                        asyncio.create_task(self._speak(str(msg["content"]), after=0.3))
                elif kind == "session.close":
                    await self._send({"type": "session.closed", "usage": {"seconds": 0}})
                    break
        finally:
            self._ws = None

    # ------------------------------------------------------------------ hearing and talking

    def _hear(self, pcm: bytes) -> None:
        if len(pcm) < 2:
            return
        n = len(pcm) // 2
        samples = struct.unpack(f"<{n}h", pcm[: n * 2])
        rms = math.sqrt(sum(v * v for v in samples) / n) / 32768.0
        now = time.monotonic()
        if rms > LOUD:
            if self._loud_since is None:
                self._loud_since = now
            self._last_loud = now
        elif self._loud_since is not None and now - self._last_loud > END_QUIET_S:
            talked_for = self._last_loud - self._loud_since
            self._loud_since = None
            if talked_for >= 0.3:
                self._on_utterance_end()

    def _on_utterance_end(self) -> None:
        if not self.script:
            return
        reply = self.script.pop(0)
        asyncio.create_task(self._answer(reply))

    async def _answer(self, reply: Reply) -> None:
        if reply.delegate:
            self.delegations += 1
            await self._send({"type": "session.delegation.created",
                              "delegation": {"id": f"mock-del-{self.delegations}", "type": "client", "target": "client"}})
        if reply.say:
            await self._speak(reply.say, after=reply.after)

    async def _speak(self, text: str, *, after: float) -> None:
        """Audio (a steady tone, loud) and the words, at speaking pace, one utterance at a time."""
        await asyncio.sleep(after)
        async with self._speak_lock:
            self.spoken.append(text)
            words = text.split()
            total_s = max(1.0, len(words) * WORD_S)
            frames = int(total_s * 1000 / FRAME_MS)
            per_frame = max(1, len(words) // max(1, frames))
            frame = _tone_frame()
            sent = 0
            t = time.monotonic()
            for i in range(frames):
                await self._send({"type": "session.output_audio.delta", "delta": base64.b64encode(frame).decode()})
                if sent < len(words):
                    chunk = " ".join(words[sent: sent + per_frame])
                    sent += per_frame
                    await self._send({"type": "session.output_transcript.delta", "delta": (" " if i else "") + chunk})
                t += FRAME_MS / 1000
                await asyncio.sleep(max(0.0, t - time.monotonic()))
            if sent < len(words):
                await self._send({"type": "session.output_transcript.delta", "delta": " " + " ".join(words[sent:])})

    async def _send(self, msg: dict) -> None:
        if self._ws is None:
            return
        msg.setdefault("event_id", str(uuid.uuid4()))
        try:
            await self._ws.send(json.dumps(msg))
        except Exception:
            pass


def _tone_frame() -> bytes:
    """100 ms of a 440 Hz tone at -14 dBFS: unmistakably "the model talking" to the gateway."""
    n = RATE * FRAME_MS // 1000
    amp = 0.2 * 32767
    return struct.pack(f"<{n}h", *(int(amp * math.sin(2 * math.pi * 440 * i / RATE)) for i in range(n)))
