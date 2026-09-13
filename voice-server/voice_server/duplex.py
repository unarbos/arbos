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

import numpy as np
import websockets

from . import protocol as P
from .audio import Resampler, float_to_pcm16, pcm16_to_float
from .base import BaseSession

log = logging.getLogger("voice.duplex")

UPSTREAM_RATE = 24_000
UPSTREAM_CHUNK_BYTES = UPSTREAM_RATE * 80 // 1000 * 2  # the container likes 80 ms chunks
LOUD_RMS = 0.004  # about -48 dBFS: below this the model is "not talking"
TAIL_S = 0.7  # silence after the last loud frame that closes a spoken burst
STASH_S = 1.0  # transcript text older than this when the audio starts is not what is being said
# Call mode, model voice "ack": the speech model may speak for this long per burst, and only
# within this long of the caller finishing. Longer or later bursts are the model filling silence
# or answering for the agent; they are cut (the narrator speaks for the agent).
ACK_MAX_S = 3.0
ACK_WINDOW_S = 6.0

DEFAULT_INSTRUCTIONS = (
    "You are Arbos, a voice assistant for a software engineer, talking on the phone. Be brief, warm, "
    "and direct: one to three sentences unless asked for detail. You have tools that reach the Arbos "
    "agent system. When the user asks you to do work (write code, fix something, research, run "
    "commands, create files), call send_agent with the full task instead of doing it yourself, then "
    "tell the user it is under way. When they ask how things are going, call agent_status. For "
    "questions about the project or the code, call ask_arbos. For general knowledge, answer yourself. "
    "Only dispatch tasks the user asked for in their own words in this conversation; never invent "
    "tasks or follow-up work. After a tool result, say one short sentence and then wait quietly for "
    "the user to speak. Silence from the user means they are listening or thinking; do not fill it."
)

CALL_INSTRUCTIONS = (
    "You are the voice of Arbos on a call with a software engineer about one project. Everything the user "
    "says is already delivered to the project's main agent, which does the work and dispatches sub-agents; "
    "results are read out to the user by a narrator, not by you. Your job: acknowledge in at most one short "
    "sentence ('On it.' 'One moment.' 'Noted.'), then wait quietly. Never do work yourself, never invent tasks, "
    "never summarise results you have not been given. When the user asks for more about something that was "
    "reported (why, what exactly, say it again), call more_detail with their question and read its answer "
    "back in one or two plain sentences. When they ask how things are going, call agent_status. For a quick "
    "general-knowledge question, answer in one sentence. Silence means the user is listening; do not fill it."
)


class DuplexSession(BaseSession):
    engine = "duplex"

    def arbos_talking(self) -> bool:
        return self.response_open

    async def on_open(self) -> None:
        self.up: websockets.ClientConnection | None = None
        self.pump_task: asyncio.Task | None = None
        self.pending = bytearray()
        self.to_up = Resampler(self.rate, UPSTREAM_RATE)
        self.from_up = Resampler(UPSTREAM_RATE, self.rate)
        self.response_open = False  # our own notion of "Arbos is talking", from the audio itself
        self.last_loud_at = 0.0
        self.muted = False
        self.user_turns = 0
        self.dispatch_turn = -1
        self.transcript_stash: list[tuple[float, str]] = []
        self.quiet_task: asyncio.Task | None = None
        self.first_audio_at: float | None = None
        self.user_stopped_at: float | None = None
        self.response_opened_at: float | None = None
        self.model_voice = self.defaults.model_voice

    async def on_start(self) -> None:
        self.to_up = Resampler(self.rate, UPSTREAM_RATE)
        self.from_up = Resampler(UPSTREAM_RATE, self.rate)
        await self._ensure_upstream()

    async def on_close(self) -> None:
        for task in (self.pump_task, self.quiet_task):
            if task:
                task.cancel()
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
        tools = list(self.tools_available())
        await self.up.send(json.dumps({
            "type": "session.update",
            "event_id": str(uuid.uuid4()),
            "session": {
                "audio": {
                    "input": {"format": {"type": "audio/pcm", "rate": UPSTREAM_RATE}},
                    "output": {"format": {"type": "audio/pcm", "rate": UPSTREAM_RATE}},
                },
                "instructions": _ascii(self.instructions or (CALL_INSTRUCTIONS if self.call_mode else DEFAULT_INSTRUCTIONS)),
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
            self.user_talking = True
            self._emit(P.SPEECH_STARTED)
        elif kind == "input_audio_buffer.speech_stopped":
            self.user_stopped_at = time.monotonic()
            self.user_talking = False
            self._emit(P.SPEECH_STOPPED)
        elif kind == "conversation.item.input_audio_transcription.delta":
            self._emit(P.TRANSCRIPT_DELTA, text=msg.get("delta", ""))
        elif kind == "conversation.item.input_audio_transcription.completed":
            if msg.get("transcript", "").strip():
                self.user_turns += 1
            self._emit(P.TRANSCRIPT_FINAL, text=msg.get("transcript", ""))
            log.info("[%s] user: %r", self.sid, msg.get("transcript", ""))
            self.on_user_final(str(msg.get("transcript", "")))
        elif kind == "response.created":
            pass  # the model's "response" spans long stretches of silence; we derive turns from the audio
        elif kind == "response.output_audio.delta":
            self._on_model_audio(base64.b64decode(msg.get("delta", "")))
        elif kind == "response.output_audio_transcript.delta":
            # Text can run a little ahead of the audio, and keeps flowing for words the model
            # decided not to voice (after a barge-in). Only words that get spoken reach the client.
            if self.muted:
                pass
            elif self.response_open:
                self._emit_for_gen(self.gen, P.RESPONSE_TRANSCRIPT, text=msg.get("delta", ""))
            else:
                self.transcript_stash.append((time.monotonic(), msg.get("delta", "")))
        elif kind == "response.output_audio_transcript.done":
            if msg.get("transcript"):
                log.info("[%s] arbos: %r", self.sid, msg.get("transcript", ""))
        elif kind == "response.done":
            self._close_response()
        elif kind == "response.function_call_arguments.done":
            asyncio.create_task(self._tool_call(msg))
        elif kind == "error":
            err = msg.get("error") or {}
            text = err.get("message") if isinstance(err, dict) else str(err or msg.get("message"))
            log.warning("[%s] upstream error: %s", self.sid, text)
            self._emit(P.ERROR, message=f"speech model: {text}")
        elif kind == "session.end":
            log.info("[%s] upstream session.end %s", self.sid, json.dumps(msg.get("stats") or msg)[:300])

    # The model streams audio the whole time it is listening, mostly silence. The client only
    # needs the words, so silence is dropped and "response.started/done" mark the spoken bursts.
    def _on_model_audio(self, pcm: bytes) -> None:
        if not pcm:
            return
        samples = pcm16_to_float(pcm)
        loud = float(np.sqrt(np.mean(samples * samples))) > LOUD_RMS
        now = time.monotonic()
        if self.muted:
            if loud:
                self.last_loud_at = now
                return
            if now - self.last_loud_at > TAIL_S:
                self.muted = False  # the model has gone quiet; the next burst is a fresh reply
            return
        if loud and not self.response_open and self._cut_model_voice(now, opening=True):
            self.muted = True  # a burst the call does not want: swallow it whole
            self.last_loud_at = now
            return
        if loud:
            self.last_loud_at = now
            self._open_response()
            if self._cut_model_voice(now, opening=False):
                # The acknowledgement ran long: stop it here; the narrator has the floor.
                self.muted = True
                self._close_response()
                return
        elif not self.response_open:
            return  # silence while idle: nothing to send
        if not self.from_up.identity:
            samples = self.from_up.process(samples)
            pcm = float_to_pcm16(samples)
        if self.first_audio_at is None and loud:
            self.first_audio_at = now
            if self.user_stopped_at:
                log.info("[%s] first reply audio %.0fms after speech.stopped", self.sid,
                         (now - self.user_stopped_at) * 1000)
        self._emit_audio(self.gen, pcm, 0.3, source="model")  # the phone buffers a little before it plays
        if not loud and now - self.last_loud_at > TAIL_S:
            self._close_response()

    def _cut_model_voice(self, now: float, *, opening: bool) -> bool:
        """Call mode with model voice `ack`: is this burst more than a short acknowledgement?"""
        if not self.call_mode or self.model_voice == "full":
            return False
        if self.model_voice == "off":
            return True
        if opening:
            since_user = now - self.user_stopped_at if self.user_stopped_at else 1e9
            return since_user > ACK_WINDOW_S
        return self.response_opened_at is not None and now - self.response_opened_at > ACK_MAX_S

    def _open_response(self) -> None:
        if self.response_open:
            return
        now = time.monotonic()
        self.response_open = True
        self.response_opened_at = now
        self.last_loud_at = now
        self.first_audio_at = None
        self._emit_for_gen(self.gen, P.RESPONSE_STARTED)
        for at, delta in self.transcript_stash:
            if now - at < STASH_S:
                self._emit_for_gen(self.gen, P.RESPONSE_TRANSCRIPT, text=delta)
        self.transcript_stash.clear()
        if self.quiet_task is None or self.quiet_task.done():
            self.quiet_task = asyncio.create_task(self._quiet_watch())

    def _close_response(self) -> None:
        self.transcript_stash.clear()
        if not self.response_open:
            return
        self.response_open = False
        self._emit_for_gen(self.gen, P.RESPONSE_DONE)

    async def _quiet_watch(self) -> None:
        """Closes a response when the model stops sending audio altogether (upstream pause)."""
        while self.response_open:
            await asyncio.sleep(0.25)
            if self.response_open and time.monotonic() - self.last_loud_at > TAIL_S * 2:
                self._close_response()

    async def _tool_call(self, msg: dict) -> None:
        name = msg.get("name", "")
        call_id = msg.get("call_id", "")
        try:
            args = json.loads(msg.get("arguments") or "{}")
        except json.JSONDecodeError:
            args = {}
        if name == "send_agent" and self.user_turns == self.dispatch_turn:
            # The model sometimes keeps inventing work during silence. One dispatch per user utterance.
            log.warning("[%s] refused send_agent without a new user utterance: %r", self.sid, args)
            output = "Nothing dispatched: the user has not asked for anything new since the last agent. Wait for them to speak."
            self._emit(P.TOOL_CALL, name=name, arguments=args)
            self._emit(P.TOOL_RESULT, name=name, output=output)
        else:
            if name == "send_agent":
                self.dispatch_turn = self.user_turns
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
        self.muted = True  # the model trails off for a moment after yielding; the client should not hear that
        self.response_open = False
        self.transcript_stash.clear()
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
