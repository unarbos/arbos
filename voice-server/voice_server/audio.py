"""PCM helpers and a small stateful polyphase resampler (numpy only)."""

from __future__ import annotations

from math import gcd

import numpy as np


def pcm16_to_float(data: bytes) -> np.ndarray:
    return np.frombuffer(data, dtype="<i2").astype(np.float32) / 32768.0


def float_to_pcm16(samples: np.ndarray) -> bytes:
    clipped = np.clip(samples, -1.0, 1.0)
    return (clipped * 32767.0).astype("<i2").tobytes()


class Resampler:
    """Rational resampler that keeps filter state between calls.

    Audio arrives in small frames; a stateless per-frame resample would put a
    click at every frame edge. This one carries the FIR tail and the decimation
    phase across calls, so a stream of frames resamples like one long signal.
    """

    def __init__(self, src_rate: int, dst_rate: int, taps_per_phase: int = 24):
        g = gcd(src_rate, dst_rate)
        self.up = dst_rate // g
        self.down = src_rate // g
        self.identity = self.up == 1 and self.down == 1
        if self.identity:
            return
        n = taps_per_phase * max(self.up, self.down)
        if n % 2 == 0:
            n += 1
        cutoff = 0.5 / max(self.up, self.down)  # fraction of the upsampled rate
        t = np.arange(n) - (n - 1) / 2
        h = 2 * cutoff * np.sinc(2 * cutoff * t) * np.hamming(n)
        self.h = (h * (self.up / h.sum())).astype(np.float32)
        self.hist = np.zeros(n - 1, dtype=np.float32)
        self.phase = 0

    def process(self, x: np.ndarray) -> np.ndarray:
        if self.identity or x.size == 0:
            return x.astype(np.float32, copy=False)
        xu = np.zeros(x.size * self.up, dtype=np.float32)
        xu[:: self.up] = x
        buf = np.concatenate([self.hist, xu])
        y = np.convolve(buf, self.h, mode="valid")
        idx = np.arange(self.phase, y.size, self.down)
        out = y[idx] if idx.size else np.zeros(0, dtype=np.float32)
        self.phase = (idx[-1] + self.down - y.size) if idx.size else self.phase - y.size
        self.hist = buf[-(self.h.size - 1):]
        return out.astype(np.float32, copy=False)


def resample_whole(x: np.ndarray, src_rate: int, dst_rate: int) -> np.ndarray:
    """Resample a complete buffer (used for TTS output when the client rate differs)."""
    if src_rate == dst_rate:
        return x
    r = Resampler(src_rate, dst_rate)
    tail = np.zeros(r.h.size, dtype=np.float32)
    return np.concatenate([r.process(x), r.process(tail)])[: int(round(x.size * dst_rate / src_rate))]
