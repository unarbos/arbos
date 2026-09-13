"""Duplex engine: bridge one client call to NVIDIA NemotronLabs VoiceChat (full-duplex S2S model).

The model listens and talks at the same time, handles interruptions itself, and
calls tools mid-conversation. Its container speaks an OpenAI-Realtime-style
JSON protocol (base64 audio); this session translates that to the Arbos wire
protocol (binary PCM + small JSON control frames) and runs the tool calls
against the Arbos kernel.
"""

from __future__ import annotations

import asyncio
import base64
import json
import logging
import time
import uuid

import websockets

from . import protocol as P
from .audio import Resampler, float_to_pcm16, pcm16_to_float
from .base import BaseSession
from .tools import TOOLS

log = logging.getLogger("voice.duplex")

UPSTREAM_RATE = 24_000
UPSTREAM_CHUNK_BYTES = UPSTREAM_RATE * 80 // 1000 * 2  # the container likes 80 ms chunks

DEFAULT_INSTRUCTIONS = (
    "You are Arbos, a voice assistant for a software engineer, talking on the phone. Be brief, warm, "
    "and direct: one to three sentences unless asked for detail. You have tools that reach the Arbos "
    "agent system. When the user asks you to do work (write code, fix something, research, run "
    "commands, create files), call send_agent with the full task instead of doing it yourself, then "
    "tell the user it is under way. When they ask how things are going, call agent_status. For "
    "questions about the project or the code, call ask_arbos. For general knowledge, answer yourself."
)


class DuplexSession(BaseSession):
    engine = "duplex"

    async def on_open(self) -> None:
        self.up: websockets.ClientConnection | None = None
        self.pump_task: asyncio.Task | None = None
        self.pending = bytearray()
        self.to_up = Resampler(self.rate, UPSTREAM_RATE)
        self.from_up = Resampler(UPSTREAM_RATE, self.rate)
        self.response_open = False
        self.first_audio_at: float | None = None
        self.user_stopped_at: float | None = None

    async def on_start(self) -> None:
        self.to_up = Resampler(self.rate, UPSTREAM_RATE)
        self.from_up = Resampler(UPSTREAM_RATE, self.rate)
        await self._ensure_upstream()

    async def on_close(self) -> None:
        if self.pump_task:
            self.pump_task.cancel()
        if self.up:
            try:
                await self.up.send(json.dumps({"type": "session.close", "event_id": str(uuid.uuid4())}))
                await asyncio.wait_for(self.up.close(), 3)
            except Exception:
                pass

    # ------------------------------------------------------------------ upstream

    async def _ensure_upstream(self) -> None:
        if self.up is not None:
            return
        t0 = time.monotonic()
        self.up = await websockets.connect(self.engines.duplex_url, max_size=16 * 1024 * 1024, compression=None)
        created = json.loads(await asyncio.wait_for(self.up.recv(), 20))
        if created.get("type") != "session.created":
            log.warning("[%s] upstream first message was %s", self.sid, created.get("type"))
        tools = [t for t in TOOLS] if self.tools_available() else []
        await self.up.send(json.dumps({
            "type": "session.update",
            "event_id": str(uuid.uuid4()),
            "session": {
                "audio": {
                    "input": {"format": {"type": "audio/pcm", "rate": UPSTREAM_RATE}},
                    "output": {"format": {"type": "audio/pcm", "rate": UPSTREAM_RATE}},
                },
                "instructions": _ascii(self.instructions or DEFAULT_INSTRUCTIONS),
                "tools": tools,
            },
        }))
        self.pump_task = asyncio.create_task(self._pump(), name=f"duplex-pump-{self.sid}")
        log.info("[%s] upstream session ready in %.0fms, %d tools", self.sid, (time.monotonic() - t0) * 1000, len(tools))

    async def _pump(self) -> None:
        assert self.up
        try:
            async for raw in self.up:
                try:
                    msg = json.loads(raw)
                except json.JSONDecodeError:
                    continue
                self._translate(msg)
        except websockets.ConnectionClosed as exc:
            log.warning("[%s] upstream closed: %s", self.sid, exc)
            self._emit(P.ERROR, message="speech model connection closed")
        finally:
            self.up = None

    def _translate(self, msg: dict) -> None:
        kind = msg.get("type", "")
        if kind == "input_audio_buffer.speech_started":
            self._emit(P.SPEECH_STARTED)
        elif kind == "input_audio_buffer.speech_stopped":
            self.user_stopped_at = time.monotonic()
            self._emit(P.SPEECH_STOPPED)
        elif kind == "conversation.item.input_audio_transcription.delta":
            self._emit(P.TRANSCRIPT_DELTA, text=msg.get("delta", ""))
        elif kind == "conversation.item.input_audio_transcription.completed":
            self._emit(P.TRANSCRIPT_FINAL, text=msg.get("transcript", ""))
            log.info("[%s] user: %r", self.sid, msg.get("transcript", ""))
        elif kind == "response.created":
            self.response_open = True
            self.first_audio_at = None
            self._emit(P.RESPONSE_STARTED)
        elif kind == "response.output_audio.delta":
            pcm = base64.b64decode(msg.get("delta", ""))
            if not self.from_up.identity:
                pcm = float_to_pcm16(self.from_up.process(pcm16_to_float(pcm)))
            if pcm:
                if self.first_audio_at is None:
                    self.first_audio_at = time.monotonic()
                    if self.user_stopped_at:
                        log.info("[%s] first reply audio %.0fms after speech.stopped", self.sid,
                                 (self.first_audio_at - self.user_stopped_at) * 1000)
                self._emit_audio(self.gen, pcm)
        elif kind == "response.output_audio_transcript.delta":
            self._emit_for_gen(self.gen, P.RESPONSE_TRANSCRIPT, text=msg.get("delta", ""))
        elif kind == "response.output_audio_transcript.done":
            log.info("[%s] arbos: %r", self.sid, msg.get("transcript", ""))
        elif kind == "response.done":
            self.response_open = False
            self._emit_for_gen(self.gen, P.RESPONSE_DONE)
        elif kind == "response.function_call_arguments.done":
            asyncio.create_task(self._tool_call(msg))
        elif kind == "error":
            err = msg.get("error") or {}
            text = err.get("message") if isinstance(err, dict) else str(err or msg.get("message"))
            log.warning("[%s] upstream error: %s", self.sid, text)
            self._emit(P.ERROR, message=f"speech model: {text}")
        elif kind == "session.end":
            log.info("[%s] upstream session.end %s", self.sid, json.dumps(msg.get("stats") or msg)[:300])

    async def _tool_call(self, msg: dict) -> None:
        name = msg.get("name", "")
        call_id = msg.get("call_id", "")
        try:
            args = json.loads(msg.get("arguments") or "{}")
        except json.JSONDecodeError:
            args = {}
        output = await self.tools.run(name, args)
        if self.up is None:
            return
        await self.up.send(json.dumps({
            "type": "conversation.item.create",
            "event_id": str(uuid.uuid4()),
            "item": {"type": "function_call_output", "call_id": call_id, "output": _ascii(output)},
        }))

    # ------------------------------------------------------------------ client side

    async def on_audio(self, data: bytes) -> None:
        await self._ensure_upstream()
        if not self.to_up.identity:
            data = float_to_pcm16(self.to_up.process(pcm16_to_float(data)))
        self.pending += data
        while len(self.pending) >= UPSTREAM_CHUNK_BYTES and self.up is not None:
            chunk, self.pending = bytes(self.pending[:UPSTREAM_CHUNK_BYTES]), self.pending[UPSTREAM_CHUNK_BYTES:]
            await self.up.send(json.dumps({
                "type": "input_audio_buffer.append",
                "event_id": str(uuid.uuid4()),
                "audio": base64.b64encode(chunk).decode(),
            }))

    async def on_speak(self, text: str) -> None:
        # The duplex model cannot be told what to say; the gateway voices client text itself.
        gen = self.gen
        await self._speak(text, gen)
        self._emit_for_gen(gen, P.RESPONSE_DONE)

    async def on_interrupt(self, cause: str) -> None:
        self.gen += 1
        self._emit(P.RESPONSE_DONE, interrupted=True)
        if self.up is not None:
            try:  # OpenAI-Realtime shape; the container may ignore it (its own VAD already yields on speech)
                await self.up.send(json.dumps({"type": "response.cancel", "event_id": str(uuid.uuid4())}))
            except Exception:
                pass


def _ascii(text: str) -> str:
    """The model's prompt/tool channel is ASCII-only."""
    return (
        text.replace("\u2014", "-").replace("\u2013", "-").replace("\u2019", "'").replace("\u201c", '"').replace("\u201d", '"')
        .encode("ascii", "ignore").decode()
    )
