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
from .pipeline import _split_ready
from .routing import is_small_talk
from .tools import TOOLS
from .vad import WINDOW_MS

log = logging.getLogger("voice.duplex")

UPSTREAM_RATE = 24_000
UPSTREAM_CHUNK_BYTES = UPSTREAM_RATE * 80 // 1000 * 2  # the container likes 80 ms chunks
LOUD_RMS = 0.004  # about -48 dBFS: below this the model is "not talking"
TAIL_S = 0.7  # silence after the last loud frame that closes a spoken burst
STASH_S = 1.0  # transcript text older than this when the audio starts is not what is being said

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


class DuplexSession(BaseSession):
    engine = "duplex"

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
        self.answerer = self.defaults.answerer
        self.kernel_task: asyncio.Task | None = None
        self.kernel_turn = False  # the kernel answers this turn; the model's own reply is dropped
        self.kernel_cut_at = 0.0
        # Fast barge-in while the gateway itself is talking (kernel answers, speak): Silero VAD on
        # the uplink reacts in ~0.3 s, long before the duplex model's own speech_started (~1-2 s).
        self.vad = self.engines.vad.stream()
        self.to_vad = Resampler(self.rate, P.ASR_RATE)
        self.vad_run_ms = 0
        self.vad_quiet_ms = 0
        self.gateway_speaking = False
        self.pending_text = ""  # kernel-bound words waiting for the user to finish
        self.model_turn = 0  # user_turns value of the last turn the model may answer itself
        self.kernel_launched_at = 0.0
        self.pending_task: asyncio.Task | None = None
        self.our_speech_started_at = 0.0
        self.transcript_stash: list[tuple[float, str]] = []
        self.quiet_task: asyncio.Task | None = None
        self.first_audio_at: float | None = None
        self.user_stopped_at: float | None = None

    async def on_start(self) -> None:
        self.to_up = Resampler(self.rate, UPSTREAM_RATE)
        self.from_up = Resampler(UPSTREAM_RATE, self.rate)
        self.to_vad = Resampler(self.rate, P.ASR_RATE)
        await self._ensure_upstream()

    async def on_close(self) -> None:
        for task in (self.pump_task, self.quiet_task, self.kernel_task, self.pending_task):
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
            # The model's speech_started often lags the user by 1-2 s, so right after a kernel
            # answer starts it usually refers to the tail of the question just asked; the client
            # would take it as a barge-in and stop playback. Real talk-over is caught by the VAD
            # path in _fast_barge_in within ~0.3 s, which also announces speech.started.
            answering = self.kernel_task is not None and not self.kernel_task.done()
            if not answering and time.monotonic() - self.our_speech_started_at > 2.0:
                self._emit(P.SPEECH_STARTED)  # our VAD did not already announce this one
            # While the kernel answers, only the VAD path may cut it (the model's event is too late
            # and too often about the question's own tail).
        elif kind == "input_audio_buffer.speech_stopped":
            self.user_stopped_at = time.monotonic()
            self._emit(P.SPEECH_STOPPED)
        elif kind == "conversation.item.input_audio_transcription.delta":
            self._emit(P.TRANSCRIPT_DELTA, text=msg.get("delta", ""))
        elif kind == "conversation.item.input_audio_transcription.completed":
            text = msg.get("transcript", "").strip()
            if text:
                self.user_turns += 1
            self._emit(P.TRANSCRIPT_FINAL, text=msg.get("transcript", ""))
            route = "kernel" if (text and self._kernel_answers(text)) else "model"
            if (route == "kernel" and len(text.split()) <= 3 and self.kernel_task is not None
                    and time.monotonic() - self.kernel_launched_at < 4.0):
                route = "fragment"  # the model finalised late; a tail of the question we already asked
            log.info("[%s] user (%s): %r", self.sid, route, text)
            if route == "kernel":
                self._start_kernel_answer(text)
            elif route == "model":
                self.model_turn = self.user_turns
        elif kind == "response.created":
            pass  # the model's "response" spans long stretches of silence; we derive turns from the audio
        elif kind == "response.output_audio.delta":
            self._on_model_audio(base64.b64decode(msg.get("delta", "")))
        elif kind == "response.output_audio_transcript.delta":
            # Text can run a little ahead of the audio, and keeps flowing for words the model
            # decided not to voice (after a barge-in). Only words that get spoken reach the client.
            if self.muted or self.kernel_turn or self.user_turns != self.model_turn:
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
        if self.kernel_turn or self.user_turns != self.model_turn:
            if loud:
                self.last_loud_at = now
            return  # this turn belongs to the kernel; the model's own reply stays unheard
        if self.muted:
            if loud:
                self.last_loud_at = now
                return
            if now - self.last_loud_at > TAIL_S:
                self.muted = False  # the model has gone quiet; the next burst is a fresh reply
            return
        if loud:
            self.last_loud_at = now
            self._open_response()
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

    def _open_response(self) -> None:
        if self.response_open:
            return
        now = time.monotonic()
        self.response_open = True
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

    def _fast_barge_in(self, data: bytes) -> None:
        """While the gateway's own TTS is playing, cut it as soon as the VAD hears the user."""
        talking = self.gateway_speaking or (self.kernel_task is not None and not self.kernel_task.done())
        for _window, prob in self.vad.push(self.to_vad.process(pcm16_to_float(data))):
            if prob >= 0.5:
                self.vad_run_ms += WINDOW_MS
                self.vad_quiet_ms = 0
            else:
                self.vad_run_ms = 0
                self.vad_quiet_ms += WINDOW_MS
            if talking and self.vad_run_ms >= max(320, self.tuning.barge_in_min_ms):
                self.vad_run_ms = 0
                self.our_speech_started_at = time.monotonic()
                self._emit(P.SPEECH_STARTED)
                if self.kernel_task is not None and not self.kernel_task.done():
                    self._cancel_kernel_answer("barge-in (vad)")
                else:
                    self.gen += 1
                    self.gateway_speaking = False
                    self._emit(P.RESPONSE_DONE, interrupted=True)
                    log.info("[%s] speak cut (barge-in, vad)", self.sid)
                talking = False

    # ------------------------------------------------------------------ the kernel answers

    def _kernel_answers(self, text: str) -> bool:
        """Call mode: the project (kernel) answers everything but small talk."""
        kernel = self.kernel
        if self.answerer == "model" or kernel is None or not kernel.connected:
            return False
        if self.answerer == "kernel":
            return True
        return not is_small_talk(text)  # "auto"

    def _start_kernel_answer(self, text: str) -> None:
        """The model sometimes finalises a transcript while the user is still talking. Hold the
        turn until the VAD has heard ~0.5 s of quiet (at most 4 s) and merge any fragment that
        arrives meanwhile, so the kernel gets one whole question."""
        if self.kernel_task is not None and not self.kernel_task.done():
            self._cancel_kernel_answer("new question")
        self.kernel_turn = True  # from now on the model's own reply is dropped
        self.transcript_stash.clear()
        self._close_response()
        self.pending_text = f"{self.pending_text} {text}".strip()
        if self.pending_task is None or self.pending_task.done():
            self.pending_task = asyncio.create_task(self._start_when_quiet(), name=f"kernel-hold-{self.sid}")

    async def _start_when_quiet(self) -> None:
        deadline = time.monotonic() + 4.0
        while time.monotonic() < deadline and self.vad_quiet_ms < 480:
            await asyncio.sleep(0.04)
        text, self.pending_text = self.pending_text, ""
        if text:
            self._launch_kernel_answer(text)

    def _launch_kernel_answer(self, text: str) -> None:
        self.kernel_launched_at = time.monotonic()
        self.kernel_turn = True
        self.transcript_stash.clear()
        self._close_response()
        self.kernel_task = asyncio.create_task(self._kernel_answer(text), name=f"kernel-answer-{self.sid}")

    def _cancel_kernel_answer(self, cause: str) -> None:
        self.kernel_cut_at = time.monotonic()
        self.gen += 1  # drops queued Kokoro frames
        if self.kernel_task is not None and not self.kernel_task.done():
            self.kernel_task.cancel()
        self._emit(P.RESPONSE_DONE, interrupted=True)
        log.info("[%s] kernel answer cut (%s)", self.sid, cause)

    async def _kernel_answer(self, text: str) -> None:
        """Ask the kernel's main agent and voice its reply with the gateway TTS, sentence by sentence."""
        gen = self.gen
        started = time.monotonic()
        spoken: list[str] = []
        buffer = ""
        first_audio: float | None = None
        self._emit_for_gen(gen, P.RESPONSE_STARTED)

        async def flush(segment: str) -> None:
            nonlocal first_audio
            segment = segment.strip()
            if not segment:
                return
            spoken.append(segment)
            self._emit_for_gen(gen, P.RESPONSE_TRANSCRIPT, text=(" " if len(spoken) > 1 else "") + segment)
            at = await self._speak(segment, gen)
            if first_audio is None and at is not None:
                first_audio = at

        try:
            async for delta in self.kernel.turn(text, timeout=120):
                buffer += delta
                ready, buffer = _split_ready(buffer)
                for segment in ready:
                    await flush(segment)
            await flush(buffer)
            self._emit_for_gen(gen, P.RESPONSE_DONE)
            log.info("[%s] kernel answer: first audio %s, %d chars",
                     self.sid, f"{(first_audio - started) * 1000:.0f}ms" if first_audio else "none", sum(map(len, spoken)))
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            log.exception("[%s] kernel answer failed", self.sid)
            self._emit(P.ERROR, message=f"kernel answer failed: {exc}")
            self._emit_for_gen(gen, P.RESPONSE_DONE)
        finally:
            self.kernel_turn = False
            self.muted = True  # the model may still be voicing its own (unheard) reply; drop it until it goes quiet

    async def _tool_call(self, msg: dict) -> None:
        name = msg.get("name", "")
        call_id = msg.get("call_id", "")
        try:
            args = json.loads(msg.get("arguments") or "{}")
        except json.JSONDecodeError:
            args = {}
        if self.kernel_turn or self.user_turns != self.model_turn or (self.kernel_task is not None and not self.kernel_task.done()):
            # The kernel took this turn (it dispatches agents itself); do not act twice.
            await self.up.send(json.dumps({
                "type": "conversation.item.create", "event_id": str(uuid.uuid4()),
                "item": {"type": "function_call_output", "call_id": call_id,
                         "output": "Arbos has already handled this request. Do not answer; wait for the user."},
            }))
            return
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
        self._fast_barge_in(data)
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
        self.gateway_speaking = True
        try:
            await self._speak(text, gen)
        finally:
            self.gateway_speaking = False
        self._emit_for_gen(gen, P.RESPONSE_DONE)

    async def on_interrupt(self, cause: str) -> None:
        if self.kernel_task is not None and not self.kernel_task.done():
            self._cancel_kernel_answer(cause)
            return
        if time.monotonic() - self.kernel_cut_at < 1.0:
            return  # the client echoes our barge-in with its own interrupt; already handled
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
