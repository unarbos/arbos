"""Pipeline engine: Silero VAD -> chunked faster-whisper -> (optional reply hop with tools) -> Kokoro.

Runs on any GPU or a CPU. Turn-taking is explicit (VAD end-of-speech), barge-in
is server-side cancellation on speech start.
"""

from __future__ import annotations

import asyncio
import logging
import re
import time
from collections import deque
from dataclasses import dataclass

import numpy as np

from . import protocol as P
from .audio import Resampler, pcm16_to_float
from .base import BaseSession
from .vad import WINDOW_MS

log = logging.getLogger("voice.pipeline")

_SENTENCE_END = re.compile(r"(?<=[.!?])\s+|\n+")
_CLAUSE_END = re.compile(r"(?<=[,;:])\s+")


@dataclass
class SpeakItem:
    text: str
    gen: int


@dataclass
class ReplyItem:
    user_text: str
    gen: int


class PipelineSession(BaseSession):
    engine = "pipeline"

    async def on_open(self) -> None:
        self.resampler = Resampler(self.rate, P.ASR_RATE)
        self.resampler_rate = self.rate
        self.vad = self.engines.vad.stream()
        self.work: asyncio.Queue[SpeakItem | ReplyItem] = asyncio.Queue()
        self.response_task: asyncio.Task | None = None
        self.voice_history: list[dict] = []
        self._reset_utterance_state()
        self.worker = asyncio.create_task(self._worker(), name=f"work-{self.sid}")

    async def on_start(self) -> None:
        if self.rate != self.resampler_rate:
            self.resampler = Resampler(self.rate, P.ASR_RATE)
            self.resampler_rate = self.rate

    async def on_close(self) -> None:
        for task in (self.response_task, self.partial_task, self.worker):
            if task is not None:
                task.cancel()

    async def on_speak(self, text: str) -> None:
        self.work.put_nowait(SpeakItem(text=text, gen=self.gen))

    async def on_interrupt(self, cause: str) -> None:
        self._interrupt(cause)

    async def on_report_speech(self, text: str) -> None:
        self.work.put_nowait(SpeakItem(text=text, gen=self.gen))

    def arbos_talking(self) -> bool:
        return self.responding

    def _interrupt(self, cause: str) -> None:
        active = self.responding
        if cause == "barge-in":
            self.note_interrupt()
        self.gen += 1
        while not self.work.empty():
            self.work.get_nowait()
        if self.response_task is not None and not self.response_task.done():
            self.response_task.cancel()
        if active:
            log.info("[%s] interrupted (%s)", self.sid, cause)
            self._emit(P.RESPONSE_DONE, interrupted=True, reason="interrupted")

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
        # The join window (Tuning.join_ms): when the last segment ended, the words of a segment
        # that is waiting for its continuation, and the id of the segment a new one continues.
        self.last_end_at: float = getattr(self, "last_end_at", 0.0)
        self.held_text: str | None = getattr(self, "held_text", None)
        self.continues: int | None = getattr(self, "continues", None)
        self.final_lock: asyncio.Lock = getattr(self, "final_lock", None) or asyncio.Lock()

    async def on_audio(self, data: bytes) -> None:
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
        # Speech again within the join window of the last segment's end: the same utterance goes
        # on after a breath. Its final will carry the earlier segment's words in front.
        since_end = (time.monotonic() - self.last_end_at) * 1000 if self.last_end_at else 1e9
        self.continues = self.utt_id if since_end < self.tuning.join_ms else None
        self.utt_id += 1
        self.in_speech = True
        self.utterance = list(self.preroll)
        self.utter_ms = len(self.utterance) * WINDOW_MS
        self.silence_ms = 0
        self.last_partial_ms = 0
        self.emitted_words = []
        self.user_talking = True
        self._emit(P.SPEECH_STARTED)
        log.info("[%s] speech.started", self.sid)
        if self.responding:
            self._interrupt("barge-in")

    def _end_utterance(self) -> None:
        self.in_speech = False
        self.speech_run_ms = 0
        self.user_talking = False
        self._emit(P.SPEECH_STOPPED)
        keep_silence = max(1, 240 // WINDOW_MS)
        trim = max(0, self.silence_ms // WINDOW_MS - keep_silence)
        windows = self.utterance[: len(self.utterance) - trim] if trim else self.utterance
        audio = np.concatenate(windows) if windows else np.zeros(0, dtype=np.float32)
        self.utterance = []
        self.preroll.clear()
        self.last_end_at = time.monotonic()
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
        # Finals settle one at a time, in order: a segment's words are held for its
        # continuation before the continuation's own final looks for them.
        async with self.final_lock:
            text = await self._joined(text, utt_id)
            if text is None:
                return
        self._emit(P.TRANSCRIPT_FINAL, text=text)
        log.info(
            "[%s] transcript.final %.0fms audio, asr %.0fms: %r",
            self.sid, audio.size / P.ASR_RATE * 1000, (time.monotonic() - started) * 1000, text,
        )
        if text and self.call_mode and self.narrator is not None:
            self.on_user_final(text)  # to the main agent; the narrator speaks the highlight
        elif text and self.reply_kind != "none" and self.engines.reply:
            self.work.put_nowait(ReplyItem(user_text=text, gen=self.gen))

    async def _joined(self, text: str, utt_id: int) -> str | None:
        """The utterance's whole text once it is really over, or None when these words belong to
        a continuation still being spoken (they are held for its final). A segment that ended is
        not answered until `join_ms` of quiet has followed it."""
        held, self.held_text = self.held_text, None
        if held:
            text = f"{held} {text}".strip() if text else held
        if utt_id != self.utt_id:
            if self.continues == utt_id:
                self.held_text = text
                log.info("[%s] segment %d continues after a pause; holding %r", self.sid, utt_id, text)
                return None
            return None  # a newer, unrelated utterance started; its own final will follow
        deadline = self.last_end_at + self.tuning.join_ms / 1000
        while time.monotonic() < deadline:
            await asyncio.sleep(0.02)
            if self.utt_id != utt_id:
                self.held_text = text
                log.info("[%s] segment %d continues after a pause; holding %r", self.sid, utt_id, text)
                return None
        return text

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
                self._emit_for_gen(item.gen, P.RESPONSE_DONE, reason="failed")
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
            self._emit_for_gen(item.gen, P.RESPONSE_DONE, reason="completed")
        elif isinstance(item, ReplyItem):
            await self._reply(item)
        else:
            raise TypeError(f"unknown work item {item!r}")

    async def _reply(self, item: ReplyItem) -> None:
        assert self.engines.reply is not None
        started = time.monotonic()
        self.voice_history.append({"role": "user", "content": item.user_text})
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

        tools = self.tools if self.tools_available() else None
        try:
            async for delta in self.engines.reply.stream(self.voice_history, tools):
                if first_token is None:
                    first_token = time.monotonic()
                buffer += delta
                ready, buffer = _split_ready(buffer)
                for segment in ready:
                    await flush(segment)
            await flush(buffer)
        finally:
            if spoken:
                self.voice_history.append({"role": "assistant", "content": " ".join(spoken)})
            self.voice_history[:] = self.voice_history[-30:]
        self._emit_for_gen(item.gen, P.RESPONSE_DONE, reason="completed")
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
