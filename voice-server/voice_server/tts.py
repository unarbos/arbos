"""Text-to-speech backends. All yield float32 mono chunks at `rate`."""

from __future__ import annotations

import asyncio
import logging
import re
from typing import AsyncIterator, Protocol

import numpy as np

log = logging.getLogger("voice.tts")

_MD_FENCE = re.compile(r"```.*?```", re.S)
_MD_INLINE = re.compile(r"[*_`#>]+")
_URL = re.compile(r"https?://\S+")
_WS = re.compile(r"[ \t]+")


def speakable(text: str) -> str:
    """Strip markdown that sounds wrong when read aloud."""
    text = _MD_FENCE.sub(" code block omitted. ", text)
    text = _URL.sub(" link ", text)
    text = _MD_INLINE.sub("", text)
    text = re.sub(r"^\s*[-•]\s+", "", text, flags=re.M)
    text = _WS.sub(" ", text)
    return text.strip()


class TTS(Protocol):
    name: str
    rate: int
    voices: list[str]

    def stream(self, text: str, voice: str, speed: float) -> AsyncIterator[np.ndarray]: ...


_SENTENCE = re.compile(r"(?<=[.!?])\s+")
_CLAUSE = re.compile(r"(?<=[,;:])\s+")


def split_for_speech(text: str, first_max: int = 60, piece_max: int = 160, piece_min: int = 24) -> list[str]:
    """Cut text into pieces the synthesiser can produce one at a time.

    A short first piece gets the first audio out fast; later pieces are longer
    so prosody stays natural. Tiny fragments are merged into their neighbour.
    """
    pieces: list[str] = []
    for sentence in _SENTENCE.split(text):
        sentence = sentence.strip()
        if not sentence:
            continue
        limit = first_max if not pieces else piece_max
        if len(sentence) <= limit:
            pieces.append(sentence)
            continue
        current = ""
        for clause in _CLAUSE.split(sentence):
            if current and len(current) + 1 + len(clause) > limit:
                pieces.append(current)
                current = clause
                limit = piece_max
            else:
                current = f"{current} {clause}".strip()
        if current:
            pieces.append(current)
    merged: list[str] = []
    for piece in pieces:
        if merged and len(piece) < piece_min:
            merged[-1] = f"{merged[-1]} {piece}"
        else:
            merged.append(piece)
    return merged


class KokoroTTS:
    """Kokoro-82M through onnxruntime (CPU or CUDA). Streams one clause or
    sentence at a time, synthesising the next while the current one is sent.
    """

    def __init__(self, model_path: str, voices_path: str):
        from kokoro_onnx import Kokoro  # heavy import, keep it inside the backend

        self.kokoro = Kokoro(model_path, voices_path)
        self.rate = 24_000
        self.voices = self.kokoro.get_voices()
        provider = self.kokoro.sess.get_providers()[0]
        self.name = f"kokoro-onnx/v1.0@{provider.replace('ExecutionProvider', '').lower()}"

    def _synth(self, piece: str, voice: str, speed: float) -> np.ndarray:
        lang = "en-gb" if voice.startswith("b") else "en-us"
        samples, _ = self.kokoro.create(piece, voice=voice, speed=speed, lang=lang, trim=True)
        pause = 0.18 if piece[-1] in ".!?" else 0.08
        return np.concatenate([samples.astype(np.float32, copy=False), np.zeros(int(self.rate * pause), np.float32)])

    async def stream(self, text: str, voice: str, speed: float) -> AsyncIterator[np.ndarray]:
        pieces = split_for_speech(speakable(text))
        if not pieces:
            return
        pending = asyncio.create_task(asyncio.to_thread(self._synth, pieces[0], voice, speed))
        for index in range(len(pieces)):
            samples = await pending
            if index + 1 < len(pieces):
                pending = asyncio.create_task(asyncio.to_thread(self._synth, pieces[index + 1], voice, speed))
            yield samples


class ToneTTS:
    """No model: each piece of text becomes a short tone whose length follows the text (about 60 ms
    per word). For the test harness and for machines without the Kokoro weights: the wire, the
    pacing, the interrupt path and the narrator all run for real; only the sound is a placeholder.
    Voices are named so `session.start {voice}` still validates."""

    def __init__(self, rate: int = 24_000):
        self.rate = rate
        self.voices = ["af_heart", "tone"]
        self.name = "tone"
        self.spoken: list[str] = []  # every text voiced, oldest first (tests read this)

    def _synth(self, piece: str, speed: float) -> np.ndarray:
        words = max(1, len(piece.split()))
        seconds = max(0.25, 0.06 * words / max(0.5, speed))
        n = int(self.rate * seconds)
        t = np.arange(n, dtype=np.float32) / self.rate
        # 220 Hz with a soft envelope: clearly audible, clearly not speech.
        env = np.minimum(1.0, np.minimum(t / 0.02, (seconds - t) / 0.05)).clip(0.0, 1.0)
        return (0.2 * np.sin(2 * np.pi * 220.0 * t) * env).astype(np.float32)

    async def stream(self, text: str, voice: str, speed: float) -> AsyncIterator[np.ndarray]:
        for piece in split_for_speech(speakable(text)):
            self.spoken.append(piece)
            yield self._synth(piece, speed)
            await asyncio.sleep(0)


def build_tts(kind: str, *, model_dir: str) -> TTS:
    if kind == "kokoro":
        return KokoroTTS(f"{model_dir}/kokoro-v1.0.onnx", f"{model_dir}/voices-v1.0.bin")
    if kind == "tone":
        return ToneTTS()
    raise ValueError(f"unknown TTS backend {kind!r}")
