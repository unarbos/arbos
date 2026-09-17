"""The caller: one WebSocket to the gateway, a microphone that never closes, ears that record.

Behaves like the desktop client (`desktop/src/voice_ws.rs`): sends `session.start` first, keeps
sending mic frames (silence when nobody talks) at real time, plays reply audio only between
`response.started` and `response.done`, and on `speech.started` while a reply plays sends
`interrupt` (barge-in). Everything received is kept with a timestamp for the assertions.
"""

from __future__ import annotations

import asyncio
import json
import time
from dataclasses import dataclass, field

import websockets

from tests.speech import RATE

FRAME_MS = 40
FRAME = RATE * FRAME_MS // 1000 * 2  # bytes


@dataclass
class Frame:
    at: float
    msg: dict


@dataclass
class Audio:
    at: float
    size: int
    speaking: bool  # between response.started and response.done


@dataclass
class Record:
    frames: list[Frame] = field(default_factory=list)
    audio: list[Audio] = field(default_factory=list)
    narrations: list[Frame] = field(default_factory=list)  # narrator.say
    interrupts_sent: list[float] = field(default_factory=list)
    utterances: list[tuple[float, float, str]] = field(default_factory=list)  # (start, end, text) as played
    pcm: bytearray = field(default_factory=bytearray)  # reply audio, when the caller keeps it

    def of(self, kind: str) -> list[Frame]:
        return [f for f in self.frames if f.msg.get("type") == kind]

    def spoken(self) -> list[str]:
        """Every text the gateway voiced or was about to voice: narrator lines and reply transcripts."""
        out: list[str] = []
        buf = ""
        for f in self.frames:
            t = f.msg.get("type")
            if t == "narrator.say":
                out.append(str(f.msg.get("text", "")))
            elif t == "response.transcript":
                buf += str(f.msg.get("text", ""))
            elif t == "response.done":
                if buf.strip():
                    out.append(buf.strip())
                buf = ""
        if buf.strip():
            out.append(buf.strip())
        return out

    def narrated(self) -> list[str]:
        return [str(f.msg.get("text", "")) for f in self.narrations]


class Caller:
    def __init__(self, url: str, *, token: str, screen: str = "on your screen", mode: str = "call",
                 channel: str = "voice", project: str = "", keep_audio: bool = False, device: str = "desktop"):
        self.url = url
        self.keep_audio = keep_audio
        self.device = device
        self.token = token
        self.screen = screen
        self.mode = mode
        self.channel = channel
        self.project = project
        self.rec = Record()
        self.ready: dict = {}
        self.refused: dict | None = None  # the error frame when the gateway refused the call
        self.speaking = False
        self._ws = None
        self._queue: asyncio.Queue[tuple[bytes, asyncio.Future]] = asyncio.Queue()
        self._tasks: list[asyncio.Task] = []
        self._playing = False
        self.t0 = time.monotonic()

    def now(self) -> float:
        return time.monotonic() - self.t0

    async def connect(self, timeout: float = 15.0) -> dict:
        self._ws = await websockets.connect(f"{self.url}?token={self.token}", max_size=4 * 1024 * 1024, compression=None)
        start = {"type": "session.start", "format": {"type": "audio/pcm", "rate": RATE}, "agents": False,
                 "mode": self.mode, "channel": self.channel, "screen": self.screen, "device": self.device}
        if self.project:
            start["project"] = self.project
        await self._ws.send(json.dumps(start))
        self._tasks.append(asyncio.create_task(self._receiver()))
        self._tasks.append(asyncio.create_task(self._mic()))
        deadline = time.monotonic() + timeout
        while not self.ready and self.refused is None and time.monotonic() < deadline:
            await asyncio.sleep(0.05)
        if self.refused is not None:
            return self.refused  # the gateway refused the call: an error frame with a code, then close 4404
        if not self.ready:
            raise TimeoutError("no session.ready from the gateway")
        return self.ready

    async def close(self) -> None:
        for t in self._tasks:
            t.cancel()
        if self._ws:
            try:
                await self._ws.send(json.dumps({"type": "session.end"}))
                await asyncio.wait_for(self._ws.close(), 3)
            except Exception:
                pass

    # ------------------------------------------------------------------ mouth

    async def say(self, pcm: bytes, text: str = "") -> None:
        """Play one utterance into the uplink at real time; returns when its last frame went out."""
        start = self.now()
        done: asyncio.Future = asyncio.get_running_loop().create_future()
        self._queue.put_nowait((pcm, done))
        await done
        self.rec.utterances.append((start, self.now(), text))

    async def _mic(self) -> None:
        """Always-on microphone: utterance frames when there is one, silence otherwise, 40 ms each."""
        silence = bytes(FRAME)
        pending = b""
        done: asyncio.Future | None = None
        loop = asyncio.get_running_loop()
        next_at = loop.time()
        while True:
            if not pending:
                if done is not None and not done.done():
                    done.set_result(None)
                    done = None
                try:
                    pending, done = self._queue.get_nowait()
                    self._playing = True
                except asyncio.QueueEmpty:
                    self._playing = False
            frame, pending = (pending[:FRAME], pending[FRAME:]) if pending else (silence, b"")
            if len(frame) < FRAME:
                frame = frame + bytes(FRAME - len(frame))
            if self._ws is not None:
                try:
                    await self._ws.send(frame)
                except Exception:
                    return
            next_at += FRAME_MS / 1000
            delay = next_at - loop.time()
            if delay > 0:
                await asyncio.sleep(delay)
            else:
                next_at = loop.time()

    async def text(self, text: str) -> None:
        await self._ws.send(json.dumps({"type": "text.input", "text": text}))

    async def interrupt(self) -> None:
        self.rec.interrupts_sent.append(self.now())
        await self._ws.send(json.dumps({"type": "interrupt"}))

    # ------------------------------------------------------------------ ears

    async def _receiver(self) -> None:
        async for message in self._ws:
            at = self.now()
            if isinstance(message, (bytes, bytearray)):
                self.rec.audio.append(Audio(at, len(message), self.speaking))
                if self.keep_audio and self.speaking:
                    self.rec.pcm += bytes(message)
                continue
            try:
                msg = json.loads(message)
            except json.JSONDecodeError:
                continue
            f = Frame(at, msg)
            self.rec.frames.append(f)
            kind = msg.get("type")
            if kind == "session.ready":
                self.ready = msg
            elif kind == "error" and msg.get("code") and not self.ready:
                self.refused = msg
            elif kind == "narrator.say":
                self.rec.narrations.append(f)
            elif kind == "response.started":
                self.speaking = True
            elif kind == "response.done":
                self.speaking = False
            elif kind == "speech.started" and self.speaking:
                # The desktop flushes its player and tells the server: barge-in.
                await self.interrupt()

    # ------------------------------------------------------------------ waiting

    async def wait_for(self, pred, timeout: float, what: str = "condition") -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if pred():
                return
            await asyncio.sleep(0.05)
        raise TimeoutError(f"timed out after {timeout:.0f}s waiting for {what}")

    async def wait_spoken(self, needle: str, timeout: float = 25.0) -> None:
        low = needle.lower()
        await self.wait_for(lambda: any(low in s.lower() for s in self.rec.spoken()), timeout, f"spoken: {needle!r}")

    async def wait_frame(self, kind: str, count: int = 1, timeout: float = 20.0) -> None:
        await self.wait_for(lambda: len(self.rec.of(kind)) >= count, timeout, f"{count}x {kind}")

    async def wait_quiet(self, seconds: float = 1.5, timeout: float = 30.0) -> None:
        """No reply audio and no narration for `seconds`."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            last = max([a.at for a in self.rec.audio if a.speaking] + [f.at for f in self.rec.narrations] + [0.0])
            if not self.speaking and self.now() - last >= seconds:
                return
            await asyncio.sleep(0.1)
