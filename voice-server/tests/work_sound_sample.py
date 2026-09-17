"""Render the two work sounds the way a caller hears them: against a reply at its real level.

    python -m tests.work_sound_sample [--out DIR]

Writes `work-sound-bed-sample.wav` and `work-sound-ticks-sample.wav` (24 kHz mono): a spoken
reply (Kokoro, normalised to the desktop's -6 dBFS peak), then ~6 s of the agent working with
two command ticks, then the reply that ends the turn. The bed and ticks are rendered with the
same numbers as `desktop/src/voice_ws.rs::BedSynth` (levels, filters, ramps); nothing is
boosted, so what is heard is the true relation between voice and sound.
"""

from __future__ import annotations

import argparse
import math
import wave
from pathlib import Path

import numpy as np

from tests import speech
from voice_server.audio import float_to_pcm16

RATE = 24_000
BED_LEVEL = 0.012
TICK_LEVEL = 0.05
TICK_HZ = 880.0
TICK_S = 0.045
PERIODIC_TICK_S = 2.5
TARGET_PEAK = 0.5


def bed(seconds: float, rate: int = RATE, seed: int = 0x9E3779B9) -> np.ndarray:
    """The bed as BedSynth makes it: xorshift white → two one-pole low-passes (1.4 kHz) → one-pole
    high-pass (150 Hz), × BED_LEVEL × 6 × a 0.25 Hz breath (0.7–1.0), with the 400 ms fade in and
    40 ms fade out."""
    n = int(seconds * rate)
    rng = seed & 0xFFFFFFFF
    white = np.empty(n, dtype=np.float32)
    for i in range(n):
        rng ^= (rng << 13) & 0xFFFFFFFF
        rng ^= rng >> 17
        rng ^= (rng << 5) & 0xFFFFFFFF
        white[i] = (rng / 0xFFFFFFFF) * 2.0 - 1.0
    a = min(0.9, 2 * math.pi * 1400.0 / rate)
    lp1 = np.zeros(n, dtype=np.float32)
    lp2 = np.zeros(n, dtype=np.float32)
    s1 = s2 = 0.0
    for i in range(n):
        s1 += a * (white[i] - s1)
        s2 += a * (s1 - s2)
        lp1[i], lp2[i] = s1, s2
    hp_a = 1.0 - min(0.9, 2 * math.pi * 150.0 / rate)
    hp = np.zeros(n, dtype=np.float32)
    prev_in = h = 0.0
    for i in range(n):
        h = hp_a * (h + lp2[i] - prev_in)
        prev_in = lp2[i]
        hp[i] = h
    t = np.arange(n) / rate
    breath = 0.7 + 0.3 * np.sin(2 * math.pi * 0.25 * t)
    out = hp * BED_LEVEL * 6.0 * breath
    ramp_in = np.minimum(1.0, t / 0.4)
    ramp_out = np.minimum(1.0, (seconds - t) / 0.04)
    return (out * ramp_in * ramp_out).astype(np.float32)


def tick(rate: int = RATE) -> np.ndarray:
    n = int(TICK_S * rate)
    x = np.arange(n) / rate
    return (np.sin(2 * math.pi * TICK_HZ * x) * np.exp(-x * 60.0) * TICK_LEVEL).astype(np.float32)


def normalised_speech(text: str) -> np.ndarray:
    pcm = speech.utterance(text, prefer="kokoro")
    x = np.frombuffer(pcm, dtype="<i2").astype(np.float32) / 32768.0
    peak = float(np.abs(x).max()) or 1.0
    return (x * min(4.0, TARGET_PEAK / peak)).astype(np.float32)


def render(mode: str, work_s: float = 6.0) -> np.ndarray:
    first = normalised_speech("On it. I sent the agent run-tests to run the whole suite.")
    last = normalised_speech("All thirty tests pass.")
    pause = np.zeros(int(0.3 * RATE), np.float32)
    work = bed(work_s) if mode == "bed" else np.zeros(int(work_s * RATE), np.float32)
    # Command ticks: the main agent starts two commands. In ticks mode also one every 2.5 s.
    at = [0.6, 3.4] if mode == "bed" else [0.6, 3.4] + [PERIODIC_TICK_S * k for k in range(1, int(work_s / PERIODIC_TICK_S) + 1)]
    tk = tick()
    for t in sorted(set(round(v, 2) for v in at)):
        i = int(t * RATE)
        if i + len(tk) <= len(work):
            work[i : i + len(tk)] += tk
    return np.concatenate([first, pause, work, pause, last])


def write(path: Path, x: np.ndarray) -> None:
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(float_to_pcm16(x))


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--out", default=str(Path(__file__).resolve().parent / "out"))
    opts = p.parse_args()
    out = Path(opts.out)
    out.mkdir(parents=True, exist_ok=True)
    if not speech.kokoro_available():
        raise SystemExit(f"Kokoro weights missing in {speech.MODELS}; run deploy/fetch-models.sh models")
    for mode in ("bed", "ticks"):
        x = render(mode)
        path = out / f"work-sound-{mode}-sample.wav"
        write(path, x)
        rms = 20 * math.log10(float(np.sqrt(np.mean(x[int(3 * RATE):int(6 * RATE)] ** 2))) + 1e-9)
        print(f"{path}: {len(x) / RATE:.1f} s; work section rms {rms:.1f} dBFS; speech peak {20 * math.log10(float(np.abs(x[:RATE]).max())):.1f} dBFS")


if __name__ == "__main__":
    main()
