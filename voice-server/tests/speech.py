"""Caller utterances as audio.

`utterance(text)` returns PCM16 mono 24 kHz for a scripted line. With the Kokoro weights present
(`models/kokoro-v1.0.onnx`, `models/voices-v1.0.bin`; `deploy/fetch-models.sh models`) it is real
speech; without them it is a deterministic speech-like signal (voiced syllable bursts with word
gaps) that every VAD in the stack hears as talking. Takes are cached as WAV under `tests/.cache/`.
"""

from __future__ import annotations

import hashlib
import os
import wave
from pathlib import Path

import numpy as np

from voice_server.audio import float_to_pcm16, pcm16_to_float
from voice_server.tts import KokoroTTS, speakable, split_for_speech

RATE = 24_000
HERE = Path(__file__).resolve().parent
CACHE = HERE / ".cache" / "utterances"
MODELS = Path(os.environ.get("VOICE_MODEL_DIR", HERE.parent / "models"))

_kokoro: KokoroTTS | None = None


def kokoro_available() -> bool:
    return (MODELS / "kokoro-v1.0.onnx").is_file() and (MODELS / "voices-v1.0.bin").is_file()


def _kokoro_synth(text: str, voice: str) -> np.ndarray:
    global _kokoro
    if _kokoro is None:
        _kokoro = KokoroTTS(str(MODELS / "kokoro-v1.0.onnx"), str(MODELS / "voices-v1.0.bin"))
    # Synchronous on purpose: callers may already be inside an event loop.
    parts = [_kokoro._synth(piece, voice, 1.0) for piece in split_for_speech(speakable(text))]  # noqa: SLF001
    return np.concatenate(parts) if parts else np.zeros(0, dtype=np.float32)


def synthetic(text: str) -> np.ndarray:
    """Speech-like, not speech: one voiced burst per syllable (a 110-140 Hz buzz shaped by two
    moving resonances), 40-70 ms gaps inside a word, 120 ms between words, seeded by the text."""
    seed = int(hashlib.sha1(text.encode()).hexdigest()[:8], 16)
    rng = np.random.default_rng(seed)
    out: list[np.ndarray] = [np.zeros(int(0.15 * RATE), np.float32)]
    for word in text.split():
        syllables = max(1, round(len(word) / 3))
        for s in range(syllables):
            dur = rng.uniform(0.11, 0.19)
            n = int(dur * RATE)
            t = np.arange(n) / RATE
            f0 = rng.uniform(105, 145) * (1 + 0.04 * np.sin(2 * np.pi * 3 * t))
            phase = np.cumsum(2 * np.pi * f0 / RATE)
            buzz = sum(np.sin(k * phase) / k for k in range(1, 12))
            f1, f2 = rng.uniform(350, 800), rng.uniform(1100, 2400)
            formant = np.sin(2 * np.pi * f1 * t) * 0.5 + np.sin(2 * np.pi * f2 * t) * 0.25
            env = np.sin(np.pi * np.linspace(0, 1, n)) ** 0.7
            burst = (buzz * (0.6 + 0.4 * formant) * env).astype(np.float32)
            burst *= 0.18 / (np.sqrt(np.mean(burst * burst)) + 1e-6)
            out.append(burst)
            if s < syllables - 1:
                out.append(np.zeros(int(rng.uniform(0.04, 0.07) * RATE), np.float32))
        out.append(np.zeros(int(0.12 * RATE), np.float32))
    out.append(np.zeros(int(0.2 * RATE), np.float32))
    return np.clip(np.concatenate(out), -1, 1)


def utterance(text: str, *, voice: str = "af_heart", prefer: str = "auto") -> bytes:
    """PCM16 mono 24 kHz for `text`. `prefer`: auto (Kokoro when present) | kokoro | synthetic."""
    use_kokoro = prefer == "kokoro" or (prefer == "auto" and kokoro_available())
    key = hashlib.sha1(f"{'k' if use_kokoro else 's'}:{voice}:{text}".encode()).hexdigest()[:16]
    CACHE.mkdir(parents=True, exist_ok=True)
    path = CACHE / f"{key}.wav"
    if path.exists():
        with wave.open(str(path), "rb") as w:
            return w.readframes(w.getnframes())
    samples = _kokoro_synth(text, voice) if use_kokoro else synthetic(text)
    pcm = float_to_pcm16(samples)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(pcm)
    return pcm


def seconds(pcm: bytes) -> float:
    return len(pcm) / 2 / RATE


def rms(pcm: bytes) -> float:
    x = pcm16_to_float(pcm)
    return float(np.sqrt(np.mean(x * x))) if x.size else 0.0
