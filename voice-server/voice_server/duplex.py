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
from collections import deque

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
        self.cap_active = False
        self.cap_buf: list[np.ndarray] = []
        self.cap_preroll: deque = deque(maxlen=max(1, self.tuning.preroll_ms // WINDOW_MS))
        self.cap_silence_ms = 0
        self.cap_ms = 0
        self.gateway_speaking = False
        self.pending_text = ""  # kernel-bound words waiting for the user to finish
        # With a real ASR loaded, our VAD + Whisper produce the transcript (the model drops first
        # syllables and splits at pauses); with --asr none, the model's own transcript events are used.
        self.own_asr = getattr(self.engines.asr, "name", "none") != "none"
        self.decision = "model"  # who answers the current turn: pending (user speaking / transcribing) | model | kernel
        self.model_hold: list[bytes] = []  # model audio held while the decision is pending
        self.kernel_launched_at = 0.0
        self.kernel_question = ""
        self.kernel_first_audio: float | None = None
        self.kernel_announced = False  # response.started sent for the current kernel answer
        self.pending_task: asyncio.Task | None = None
        self.our_speech_started_at = 0.0
        self.transcript_stash: list[tuple[float, str]] = []
        self.quiet_task: asyncio.Task | None = None
        self.first_audio_at: float | None = None
        self.user_stopped_at: float | None = None
        self.response_opened_at: float | None = None
        self.model_voice = self.defaults.model_voice

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
        tools = list(self.tools_available())
        await self.up.send(json.dumps({
            "type": "session.update",
            "event_id": str(uuid.uuid4()),
            "session": {
                "audio": {
                    "input": {"format": {"type": "audio/pcm", "rate": UPSTREAM_RATE}},
                    "output": {"format": {"type": "audio/pcm", "rate": UPSTREAM_RATE}},
                },
                "instructions": _ascii(self.instructions or (CALL_INSTRUCTIONS if self.call_mode else DEFAULT_INSTRUCTIONS))
                + (("\n\n" + self.context_text()) if self.call_mode and self.context_text() else ""),
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
            if not self.own_asr:
                self.user_talking = True
                self.decision = "pending"
                self.model_hold = []
                self._emit(P.SPEECH_STARTED)
            # else: our VAD announces speech.started ~1 s sooner (see _uplink)
        elif kind == "input_audio_buffer.speech_stopped":
            if not self.own_asr:
                self.user_stopped_at = time.monotonic()
                self.user_talking = False
                self._emit(P.SPEECH_STOPPED)
            # else: our VAD ends the utterance; the model's endpointing splits sentences at pauses
        elif kind == "conversation.item.input_audio_transcription.delta":
            self._emit(P.TRANSCRIPT_DELTA, text=msg.get("delta", ""))  # live words; the final replaces them
        elif kind == "conversation.item.input_audio_transcription.completed":
            # The model's own transcript drops the first syllable of an utterance ("Hello" -> "o")
            # and cuts at pauses, measured with the model alone. It is logged for comparison only;
            # the transcript the client and the kernel get comes from Whisper on our VAD segment.
            log.info("[%s] model heard: %r", self.sid, msg.get("transcript", ""))
            if not self.own_asr:
                text = str(msg.get("transcript", "")).strip()
                self._emit(P.TRANSCRIPT_FINAL, text=text)
                self._route_final(text, source="model")
        elif kind == "response.created":
            pass  # the model's "response" spans long stretches of silence; we derive turns from the audio
        elif kind == "response.output_audio.delta":
            self._on_model_audio(base64.b64decode(msg.get("delta", "")))
        elif kind == "response.output_audio_transcript.delta":
            # Text can run a little ahead of the audio, and keeps flowing for words the model
            # decided not to voice (after a barge-in). Only words that get spoken reach the client.
            if self.muted or self.kernel_turn or self.decision != "model":
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
        if self.kernel_turn or self.decision == "kernel":
            if loud:
                self.last_loud_at = now
            return  # this turn belongs to the kernel; the model's own reply stays unheard
        if self.decision == "pending":
            # The user just spoke and Whisper is still deciding who answers. Hold the model's
            # reply (it starts ~0.5 s after the endpoint) so small talk stays whole and a
            # project question never leaks the model's own first words.
            if loud or self.model_hold:
                self.model_hold.append(pcm)
                if len(self.model_hold) > 40:  # ~3 s: something is stuck, stop holding
                    self.model_hold = self.model_hold[-40:]
            return
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
        """Call mode: is this burst of the speech model's own voice one the caller should not hear?
        `full`: never. `off`: always. `ack`: anything but a short acknowledgement right after the
        caller. `auto`: an answer to small talk passes whole; a burst after a work request (which
        the main agent got, and the narrator speaks for) or unprompted chatter is cut."""
        if not self.call_mode or self.model_voice == "full":
            return False
        if self.model_voice == "off":
            return True
        since_user = now - self.user_stopped_at if self.user_stopped_at else 1e9
        if self.model_voice == "auto":
            if opening:
                return since_user > ACK_WINDOW_S or not self.last_conversational
            return not self.last_conversational and self.response_opened_at is not None and now - self.response_opened_at > ACK_MAX_S
        if opening:
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
        self._emit_for_gen(self.gen, P.RESPONSE_DONE, reason="completed")

    async def _quiet_watch(self) -> None:
        """Closes a response when the model stops sending audio altogether (upstream pause)."""
        while self.response_open:
            await asyncio.sleep(0.25)
            if self.response_open and time.monotonic() - self.last_loud_at > TAIL_S * 2:
                self._close_response()

    def _uplink(self, data: bytes) -> None:
        """Silero VAD on the uplink does two jobs the model does badly: it cuts a gateway-voiced
        reply ~0.3 s after the user starts talking, and it segments the user's utterance
        (with 320 ms of pre-roll) so Whisper can transcribe the whole thing, first word included."""
        talking = self.gateway_speaking or (self.kernel_task is not None and not self.kernel_task.done())
        t = self.tuning
        for window, prob in self.vad.push(self.to_vad.process(pcm16_to_float(data))):
            if prob >= 0.5:
                self.vad_run_ms += WINDOW_MS
                self.vad_quiet_ms = 0
            else:
                self.vad_run_ms = 0
                self.vad_quiet_ms += WINDOW_MS
            # barge-in over our own voice
            if talking and self.vad_run_ms >= max(320, t.barge_in_min_ms):
                self.vad_run_ms = 0
                if self.kernel_task is not None and not self.kernel_task.done():
                    if self.kernel_announced:
                        self._cancel_kernel_answer("barge-in (vad)")
                else:
                    self.gen += 1
                    self.gateway_speaking = False
                    self._emit(P.RESPONSE_DONE, interrupted=True, reason="interrupted")
                    log.info("[%s] speak cut (barge-in, vad)", self.sid)
                talking = False
            # utterance capture (only when we transcribe ourselves)
            if not self.own_asr:
                continue
            if not self.cap_active:
                self.cap_preroll.append(window)
                if prob >= t.start_threshold and self.vad_run_ms >= t.min_speech_ms:
                    self.cap_active = True
                    self.cap_buf = list(self.cap_preroll)
                    self.cap_ms = len(self.cap_buf) * WINDOW_MS
                    self.cap_silence_ms = 0
                    self.our_speech_started_at = time.monotonic()
                    self.decision = "pending"
                    self.model_hold = []
                    self.user_talking = True
                    self._emit(P.SPEECH_STARTED)
            else:
                self.cap_buf.append(window)
                self.cap_ms += WINDOW_MS
                self.cap_silence_ms = self.cap_silence_ms + WINDOW_MS if prob < t.end_threshold else 0
                if self.cap_silence_ms >= t.end_silence_ms or self.cap_ms >= t.max_utterance_s * 1000:
                    self.cap_active = False
                    keep = max(1, 240 // WINDOW_MS)
                    trim = max(0, self.cap_silence_ms // WINDOW_MS - keep)
                    windows = self.cap_buf[: len(self.cap_buf) - trim] if trim else self.cap_buf
                    audio = np.concatenate(windows) if windows else np.zeros(0, dtype=np.float32)
                    self.cap_buf = []
                    self.cap_preroll.clear()
                    self.user_stopped_at = time.monotonic()
                    self.user_talking = False
                    self._emit(P.SPEECH_STOPPED)
                    asyncio.create_task(self._utterance_done(audio))

    async def _utterance_done(self, audio: np.ndarray) -> None:
        started = time.monotonic()
        try:
            text = await asyncio.to_thread(self.engines.asr.transcribe, audio, partial=False, language=self.language)
        except Exception as exc:
            log.exception("[%s] transcription failed", self.sid)
            self._emit(P.ERROR, message=f"transcription failed: {exc}")
            text = ""
        text = text.strip()
        self._emit(P.TRANSCRIPT_FINAL, text=text)
        self._route_final(text, source=f"{audio.size / P.ASR_RATE * 1000:.0f}ms audio, asr {(time.monotonic() - started) * 1000:.0f}ms")

    def _route_final(self, text: str, *, source: str) -> None:
        """A finished caller utterance: who answers it."""
        if not text:
            self._release_model()
            return
        self.user_turns += 1
        self.on_user_final(text)  # call mode: the narrator's inbox; otherwise a pending ask/approval
        if self.call_mode:
            # In call mode the narrator (over the main agent) answers; the speech model's voice is
            # governed by --call-model-voice in _on_model_audio.
            log.info("[%s] user (call, %s): %r", self.sid, source, text)
            self._release_model()
            return
        route = "kernel" if self._kernel_answers(text) else "model"
        answering = self.kernel_task is not None and not self.kernel_task.done()
        if route == "kernel" and answering and not self.kernel_announced \
                and time.monotonic() - self.kernel_launched_at < 3.0:
            # The user paused and went on before the kernel said anything: one question, not two.
            text = f"{self.kernel_question} {text}".strip()
            route = "continuation"
        elif (route == "kernel" and len(text.split()) <= 3 and self.kernel_task is not None
                and time.monotonic() - self.kernel_launched_at < 4.0):
            route = "fragment"  # a tail of the question we already asked
        log.info("[%s] user (%s, %s): %r", self.sid, route, source, text)
        if route in ("kernel", "continuation"):
            self.decision = "kernel"
            self.model_hold = []
            self._start_kernel_answer(text)
        elif route == "model":
            self._release_model()
        else:
            self.decision = "kernel"

    def _release_model(self) -> None:
        """The model answers this turn: let its held reply out, then stream live."""
        self.decision = "model"
        held, self.model_hold = self.model_hold, []
        for pcm in held:
            self._on_model_audio(pcm)

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
            self._cancel_kernel_answer("new question", reason="superseded")
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
        self.kernel_question = text
        self.kernel_first_audio = None
        self.kernel_announced = False
        log.info("[%s] asking the kernel: %r", self.sid, text)
        self.kernel_turn = True
        self.transcript_stash.clear()
        self._close_response()
        self.kernel_task = asyncio.create_task(self._kernel_answer(text), name=f"kernel-answer-{self.sid}")

    def _cancel_kernel_answer(self, cause: str, reason: str = "interrupted") -> None:
        """Stop a kernel answer. `reason` is what the client is told: "interrupted" (the user
        talked over it) or "superseded" (more of the question arrived; the answer never played).
        A superseded answer that has not produced audio yet was never announced, so no
        response.done goes out for it: nothing the client could see has ended."""
        self.kernel_cut_at = time.monotonic()
        self.gen += 1  # drops queued Kokoro frames
        if self.kernel_task is not None and not self.kernel_task.done():
            self.kernel_task.cancel()
        if self.kernel is not None and self.kernel.connected:
            # Tell the kernel too, or it keeps generating the old answer and the next question
            # queues behind it (and its deltas would leak into the next turn's text).
            try:
                self.kernel.send({"type": "stop", "agent": "root"})
            except Exception:
                pass
        if self.kernel_announced:
            self._emit(P.RESPONSE_DONE, interrupted=True, reason=reason)
        log.info("[%s] kernel answer cut (%s, %s)", self.sid, cause, reason)

    async def _kernel_answer(self, text: str) -> None:
        """Ask the kernel's main agent and voice its reply with the gateway TTS, sentence by sentence."""
        gen = self.gen
        started = time.monotonic()
        spoken: list[str] = []
        buffer = ""
        first_audio: float | None = None

        async def flush(segment: str) -> None:
            nonlocal first_audio
            segment = segment.strip()
            if not segment:
                return
            if not spoken:
                # Only now is there something a client could interrupt; announcing earlier made
                # clients cut a still-silent answer when the user merely paused and went on.
                self._emit_for_gen(gen, P.RESPONSE_STARTED)
                self.kernel_announced = True
            spoken.append(segment)
            self._emit_for_gen(gen, P.RESPONSE_TRANSCRIPT, text=(" " if len(spoken) > 1 else "") + segment)
            at = await self._speak(segment, gen)
            if first_audio is None and at is not None:
                first_audio = at
                self.kernel_first_audio = at

        try:
            async for delta in self.kernel.turn(text, timeout=120):
                buffer += delta
                ready, buffer = _split_ready(buffer)
                for segment in ready:
                    await flush(segment)
            await flush(buffer)
            if not spoken:
                self._emit_for_gen(gen, P.RESPONSE_STARTED)
            self._emit_for_gen(gen, P.RESPONSE_DONE, reason="completed")
            log.info("[%s] kernel answer: first audio %s, %d chars",
                     self.sid, f"{(first_audio - started) * 1000:.0f}ms" if first_audio else "none", sum(map(len, spoken)))
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            log.exception("[%s] kernel answer failed", self.sid)
            self._emit(P.ERROR, message=f"kernel answer failed: {exc}")
            self._emit_for_gen(gen, P.RESPONSE_DONE, reason="failed")
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
        if self.kernel_turn or self.decision == "kernel" or (self.kernel_task is not None and not self.kernel_task.done()):
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
        self._uplink(data)
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
        self._emit_for_gen(gen, P.RESPONSE_DONE, reason="completed")

    async def on_interrupt(self, cause: str) -> None:
        if self.kernel_task is not None and not self.kernel_task.done():
            if self.kernel_announced:
                self._cancel_kernel_answer(cause)
            return  # nothing is playing yet; the user is still finishing the question
        if time.monotonic() - self.kernel_cut_at < 1.0:
            return  # the client echoes our barge-in with its own interrupt; already handled
        self.gen += 1
        self.muted = True  # the model trails off for a moment after yielding; the client should not hear that
        self.response_open = False
        self.transcript_stash.clear()
        self._emit(P.RESPONSE_DONE, interrupted=True, reason="interrupted")
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
