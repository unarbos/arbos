"""One WebSocket call: VAD -> chunked ASR -> (optional reply) -> streamed TTS, with barge-in."""

from __future__ import annotations

import asyncio
import json
import logging
import re
import time
from collections import deque
from dataclasses import dataclass, field

import numpy as np

from . import protocol as P
from .audio import Resampler, float_to_pcm16, pcm16_to_float, resample_whole
from .engines import Engines
from .vad import WINDOW_MS

log = logging.getLogger("voice.session")

_SENTENCE_END = re.compile(r"(?<=[.!?])\s+|\n+")
_CLAUSE_END = re.compile(r"(?<=[,;:])\s+")


@dataclass
class Tuning:
    """Turn-taking knobs. Milliseconds unless the name says otherwise."""

    start_threshold: float = 0.5
    end_threshold: float = 0.35
    min_speech_ms: int = 96  # 3 windows of confident speech before speech.started
    barge_in_min_ms: int = 256  # stricter while we are talking, so our own echo does not cut us off
    end_silence_ms: int = 600
    preroll_ms: int = 320
    partial_interval_ms: int = 700
    partial_window_s: int = 20
    max_utterance_s: int = 60
    out_frame_ms: int = 100
    max_lead_ms: int = 1500  # how far ahead of real-time playback we send reply audio


@dataclass
class SessionDefaults:
    rate: int = P.DEFAULT_RATE
    language: str | None = "en"
    voice: str = "af_heart"
    speed: float = 1.0
    reply: str = "none"


@dataclass
class SpeakItem:
    text: str
    gen: int


@dataclass
class ReplyItem:
    user_text: str
    gen: int


@dataclass
class Session:
    ws: object
    engines: Engines
    defaults: SessionDefaults
    tuning: Tuning
    sid: str = field(default_factory=lambda: f"{int(time.time()) % 100000:05d}")

    def __post_init__(self) -> None:
        self.rate = self.defaults.rate
        self.language = self.defaults.language
        self.voice = self.defaults.voice
        self.speed = self.defaults.speed
        self.reply_kind = self.defaults.reply if self.engines.reply else "none"
        self.resampler = Resampler(self.rate, P.ASR_RATE)
        self.vad = self.engines.vad.stream()
        self.out: asyncio.Queue[tuple[int | None, bytes | str]] = asyncio.Queue()
        self.work: asyncio.Queue[SpeakItem | ReplyItem] = asyncio.Queue()
        self.gen = 0
        self.ready_sent = False
        self.response_task: asyncio.Task | None = None
        self.history: list[dict[str, str]] = []
        self._reset_utterance_state()

    # ------------------------------------------------------------------ lifecycle

    async def run(self) -> None:
        sender = asyncio.create_task(self._sender(), name=f"send-{self.sid}")
        worker = asyncio.create_task(self._worker(), name=f"work-{self.sid}")
        log.info("[%s] connected", self.sid)
        try:
            async for message in self.ws:
                if isinstance(message, (bytes, bytearray)):
                    if not self.ready_sent:
                        self._send_ready()
                    self._on_audio(bytes(message))
                else:
                    if await self._on_control(message):
                        break
        finally:
            for task in (self.response_task, self.partial_task, worker, sender):
                if task is not None:
                    task.cancel()
            log.info("[%s] closed", self.sid)

    async def _sender(self) -> None:
        while True:
            gen, payload = await self.out.get()
            try:
                if gen is None or gen == self.gen:
                    await self.ws.send(payload)
            except Exception:  # connection gone; run() will notice and stop
                pass
            finally:
                self.out.task_done()

    def _emit(self, msg_type: str, **fields) -> None:
        self.out.put_nowait((None, json.dumps({"type": msg_type, **fields})))

    def _emit_for_gen(self, gen: int, msg_type: str, **fields) -> None:
        self.out.put_nowait((gen, json.dumps({"type": msg_type, **fields})))

    def _emit_audio(self, gen: int, pcm: bytes) -> None:
        self.out.put_nowait((gen, pcm))

    def _send_ready(self) -> None:
        self.ready_sent = True
        self._emit(
            P.SESSION_READY,
            rate=self.rate,
            asr=self.engines.asr.name,
            tts=self.engines.tts.name,
            reply=self.engines.reply.name if (self.engines.reply and self.reply_kind != "none") else "none",
            voice=self.voice,
        )

    # ------------------------------------------------------------------ control

    async def _on_control(self, raw: str) -> bool:
        """Returns True when the session should end."""
        try:
            msg = json.loads(raw)
            kind = msg["type"]
        except (json.JSONDecodeError, KeyError, TypeError):
            self._emit(P.ERROR, message="control frames must be JSON objects with a 'type'")
            return False

        if kind == P.SESSION_START:
            self._apply_start(msg)
            self._send_ready()
        elif kind == P.SPEAK:
            text = str(msg.get("text", "")).strip()
            if text:
                self.work.put_nowait(SpeakItem(text=text, gen=self.gen))
        elif kind == P.INTERRUPT:
            self._interrupt("client")
        elif kind == P.SESSION_END:
            return True
        else:
            self._emit(P.ERROR, message=f"unknown message type {kind!r}")
        return False

    def _apply_start(self, msg: dict) -> None:
        fmt = msg.get("format") or {}
        rate = int(fmt.get("rate", self.rate) or self.rate)
        if not 8000 <= rate <= 48000:
            self._emit(P.ERROR, message=f"unsupported rate {rate}; using {self.rate}")
        elif rate != self.rate:
            self.rate = rate
            self.resampler = Resampler(self.rate, P.ASR_RATE)
        if "language" in msg:
            self.language = msg["language"] or None
        voice = msg.get("voice")
        if voice:
            if voice in self.engines.tts.voices:
                self.voice = voice
            else:
                self._emit(P.ERROR, message=f"unknown voice {voice!r}; using {self.voice}")
        if "speed" in msg:
            self.speed = float(np.clip(float(msg["speed"]), 0.5, 2.0))
        reply = msg.get("reply")
        if reply in ("none", "openrouter"):
            if reply != "none" and not self.engines.reply:
                self._emit(P.ERROR, message="server started without a reply backend; staying speech-only")
            else:
                self.reply_kind = reply

    def _interrupt(self, cause: str) -> None:
        active = (self.response_task is not None and not self.response_task.done()) or not self.work.empty()
        self.gen += 1
        while not self.work.empty():
            self.work.get_nowait()
        if self.response_task is not None and not self.response_task.done():
            self.response_task.cancel()
        if active:
            log.info("[%s] interrupted (%s)", self.sid, cause)
            self._emit(P.RESPONSE_DONE, interrupted=True)

    @property
    def responding(self) -> bool:
        return (self.response_task is not None and not self.response_task.done()) or not self.work.empty()

    # ------------------------------------------------------------------ audio in

    def _reset_utterance_state(self) -> None:
        self.in_speech = False
        self.speech_run_ms = 0
        self.silence_ms = 0
        self.utter_ms = 0
        self.utterance: list[np.ndarray] = []
        self.preroll: deque[np.ndarray] = deque(maxlen=max(1, self.tuning.preroll_ms // WINDOW_MS))
        self.utt_id = getattr(self, "utt_id", 0)
        self.emitted_words: list[str] = []
        self.last_partial_ms = 0
        self.partial_task: asyncio.Task | None = None
        self.speech_started_at = 0.0
        self.speech_stopped_at = 0.0

    def _on_audio(self, data: bytes) -> None:
        samples = self.resampler.process(pcm16_to_float(data))
        for window, prob in self.vad.push(samples):
            self._vad_step(window, prob)

    def _vad_step(self, window: np.ndarray, prob: float) -> None:
        t = self.tuning
        if not self.in_speech:
            self.preroll.append(window)
            if prob >= t.start_threshold:
                self.speech_run_ms += WINDOW_MS
                need = t.barge_in_min_ms if self.responding else t.min_speech_ms
                if self.speech_run_ms >= need:
                    self._start_utterance()
            else:
                self.speech_run_ms = 0
            return

        self.utterance.append(window)
        self.utter_ms += WINDOW_MS
        if prob < t.end_threshold:
            self.silence_ms += WINDOW_MS
        else:
            self.silence_ms = 0

        if self.silence_ms >= t.end_silence_ms or self.utter_ms >= t.max_utterance_s * 1000:
            self._end_utterance()
            return
        due = self.utter_ms - self.last_partial_ms >= t.partial_interval_ms
        if due and self.utter_ms >= 800 and (self.partial_task is None or self.partial_task.done()):
            self.last_partial_ms = self.utter_ms
            self.partial_task = asyncio.create_task(self._partial(self.utt_id))

    def _start_utterance(self) -> None:
        self.utt_id += 1
        self.in_speech = True
        self.speech_started_at = time.monotonic()
        self.utterance = list(self.preroll)
        self.utter_ms = len(self.utterance) * WINDOW_MS
        self.silence_ms = 0
        self.last_partial_ms = 0
        self.emitted_words = []
        self._emit(P.SPEECH_STARTED)
        log.info("[%s] speech.started", self.sid)
        if self.responding:
            self._interrupt("barge-in")

    def _end_utterance(self) -> None:
        self.in_speech = False
        self.speech_run_ms = 0
        self.speech_stopped_at = time.monotonic()
        self._emit(P.SPEECH_STOPPED)
        keep_silence = max(1, 240 // WINDOW_MS)
        trim = max(0, self.silence_ms // WINDOW_MS - keep_silence)
        windows = self.utterance[: len(self.utterance) - trim] if trim else self.utterance
        audio = np.concatenate(windows) if windows else np.zeros(0, dtype=np.float32)
        self.utterance = []
        self.preroll.clear()
        asyncio.create_task(self._final(audio, self.utt_id))

    # ------------------------------------------------------------------ ASR

    async def _partial(self, utt_id: int) -> None:
        window = int(self.tuning.partial_window_s * P.ASR_RATE)
        audio = np.concatenate(self.utterance)[-window:]
        try:
            text = await asyncio.to_thread(
                self.engines.asr.transcribe, audio, partial=True, language=self.language
            )
        except Exception as exc:  # keep the call alive; the final will retry
            log.warning("[%s] partial ASR failed: %s", self.sid, exc)
            return
        if utt_id != self.utt_id or not self.in_speech:
            return
        delta = self._delta(text)
        if delta:
            self._emit(P.TRANSCRIPT_DELTA, text=delta)

    def _delta(self, hypothesis: str) -> str | None:
        def norm(word: str) -> str:
            return re.sub(r"[^\w']", "", word.lower())

        new_words = hypothesis.split()
        old = self.emitted_words
        if len(new_words) <= len(old):
            return None
        if [norm(w) for w in new_words[: len(old)]] != [norm(w) for w in old]:
            return None  # earlier words changed; the final will replace the line
        added = new_words[len(old) : -1]  # the last word is usually cut mid-way
        if not added:
            return None
        self.emitted_words = old + added
        return (" " if old else "") + " ".join(added)

    async def _final(self, audio: np.ndarray, utt_id: int) -> None:
        started = time.monotonic()
        try:
            text = await asyncio.to_thread(
                self.engines.asr.transcribe, audio, partial=False, language=self.language
            )
        except Exception as exc:
            log.exception("[%s] final ASR failed", self.sid)
            self._emit(P.ERROR, message=f"transcription failed: {exc}")
            text = ""
        if utt_id != self.utt_id:
            return  # a newer utterance started; its own final will follow
        self._emit(P.TRANSCRIPT_FINAL, text=text)
        log.info(
            "[%s] transcript.final %.0fms audio, asr %.0fms: %r",
            self.sid, audio.size / P.ASR_RATE * 1000, (time.monotonic() - started) * 1000, text,
        )
        if text and self.reply_kind != "none" and self.engines.reply:
            self.work.put_nowait(ReplyItem(user_text=text, gen=self.gen))

    # ------------------------------------------------------------------ responses

    async def _worker(self) -> None:
        while True:
            item = await self.work.get()
            if item.gen != self.gen:
                continue
            self.response_task = asyncio.create_task(self._run(item))
            try:
                await self.response_task
            except asyncio.CancelledError:
                if not self.response_task.done():
                    self.response_task.cancel()
                    raise
            except Exception as exc:
                log.exception("[%s] response failed", self.sid)
                self._emit(P.ERROR, message=f"response failed: {exc}")
                self._emit_for_gen(item.gen, P.RESPONSE_DONE)
            finally:
                self.response_task = None

    async def _run(self, item: SpeakItem | ReplyItem) -> None:
        if isinstance(item, SpeakItem):
            started = time.monotonic()
            first = await self._speak(item.text, item.gen)
            log.info(
                "[%s] speak %d chars: first audio %s", self.sid, len(item.text),
                f"{(first - started) * 1000:.0f}ms" if first else "none",
            )
            self._emit_for_gen(item.gen, P.RESPONSE_DONE)
        elif isinstance(item, ReplyItem):
            await self._reply(item)
        else:
            raise TypeError(f"unknown work item {item!r}")

    async def _speak(self, text: str, gen: int) -> float | None:
        """Streams TTS for `text`. Returns the monotonic time of the first audio frame.

        Frames go out ahead of real time, but only up to --max-lead-ms. That
        keeps the client's buffer small, so an interrupt stops the sound fast
        and does not waste bandwidth on audio nobody will hear.
        """
        first_at: float | None = None
        frame_bytes = self.rate * self.tuning.out_frame_ms // 1000 * 2
        frame_s = self.tuning.out_frame_ms / 1000
        lead_s = self.tuning.max_lead_ms / 1000
        sent_s = 0.0
        async for chunk in self.engines.tts.stream(text, self.voice, self.speed):
            if self.engines.tts.rate != self.rate:
                chunk = resample_whole(chunk, self.engines.tts.rate, self.rate)
            pcm = float_to_pcm16(chunk)
            for i in range(0, len(pcm), frame_bytes):
                if first_at is not None:
                    ahead = sent_s - (time.monotonic() - first_at)
                    if ahead > lead_s:
                        await asyncio.sleep(ahead - lead_s)
                self._emit_audio(gen, pcm[i : i + frame_bytes])
                sent_s += frame_s
                if first_at is None:
                    first_at = time.monotonic()
        return first_at

    async def _reply(self, item: ReplyItem) -> None:
        assert self.engines.reply is not None
        started = time.monotonic()
        self.history.append({"role": "user", "content": item.user_text})
        self._emit_for_gen(item.gen, P.RESPONSE_STARTED)
        spoken: list[str] = []
        buffer = ""
        first_token: float | None = None
        first_audio: float | None = None

        async def flush(segment: str) -> None:
            nonlocal first_audio
            segment = segment.strip()
            if not segment:
                return
            spoken.append(segment)
            self._emit_for_gen(item.gen, P.RESPONSE_TRANSCRIPT, text=(" " if len(spoken) > 1 else "") + segment)
            at = await self._speak(segment, item.gen)
            if first_audio is None and at is not None:
                first_audio = at

        try:
            async for delta in self.engines.reply.stream(self.history):
                if first_token is None:
                    first_token = time.monotonic()
                buffer += delta
                ready, buffer = _split_ready(buffer)
                for segment in ready:
                    await flush(segment)
            await flush(buffer)
        finally:
            if spoken:
                self.history.append({"role": "assistant", "content": " ".join(spoken)})
            self.history[:] = self.history[-20:]
        self._emit_for_gen(item.gen, P.RESPONSE_DONE)
        log.info(
            "[%s] reply: first token %s, first audio %s (from transcript.final)",
            self.sid,
            f"{(first_token - started) * 1000:.0f}ms" if first_token else "none",
            f"{(first_audio - started) * 1000:.0f}ms" if first_audio else "none",
        )


def _split_ready(buffer: str) -> tuple[list[str], str]:
    """Cut the LLM stream into speakable pieces: full sentences, or a clause once
    the buffer is long enough that waiting for the period would cost latency."""
    parts = _SENTENCE_END.split(buffer)
    ready, rest = parts[:-1], parts[-1]
    if len(rest) >= 80:
        clauses = _CLAUSE_END.split(rest)
        if len(clauses) > 1:
            ready += clauses[:-1]
            rest = clauses[-1]
    if len(rest) >= 240:
        ready.append(rest)
        rest = ""
    return [r for r in ready if r.strip()], rest
