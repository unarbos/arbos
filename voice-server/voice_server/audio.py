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


class Normalizer:
    """Peak-following gain toward a target level, with a soft-knee limiter.

    Reply audio from different engines lands at very different levels (the duplex
    model about 14 dB under the pipeline's TTS). This brings every frame toward
    `target_dbfs` peak: gain drops at once when the input gets louder, rises
    slowly (`rise_db_per_s`) when it gets quieter, never above `max_gain_db`, and
    holds still over silence so noise is not pumped up. Whatever still overshoots
    is bent smoothly into [-1, 1] instead of clipping.
    """

    def __init__(self, rate: int, target_dbfs: float = -3.0, max_gain_db: float = 24.0,
                 rise_db_per_s: float = 6.0, release_s: float = 1.5, knee: float = 0.6):
        self.rate = rate
        self.target = 10 ** (target_dbfs / 20)
        self.max_gain = 10 ** (max_gain_db / 20)
        self.rise_db_per_s = rise_db_per_s
        self.release_s = release_s
        self.knee = knee
        self.env = 0.0  # peak envelope of the input
        self.gain = 1.0
        self.loud_frames = 0

    def process(self, x: np.ndarray) -> np.ndarray:
        if x.size == 0:
            return x
        dt = x.size / self.rate
        peak = float(np.max(np.abs(x)))
        if peak < 0.002:  # silence: keep the envelope decaying, do not touch the gain
            self.env *= 0.5 ** (dt / self.release_s)
            return self._limit(x * self.gain)
        self.env = max(peak, self.env * 0.5 ** (dt / self.release_s))
        wanted = min(self.max_gain, max(1.0, self.target / max(self.env, 1e-4)))
        self.loud_frames += 1
        if wanted < self.gain or self.loud_frames <= 3:
            self.gain = wanted  # louder input: back off at once; first frames: jump to the level
        else:
            step = 10 ** (self.rise_db_per_s * dt / 20)
            self.gain = min(wanted, self.gain * step)
        return self._limit(x * self.gain)

    def _limit(self, y: np.ndarray) -> np.ndarray:
        a = np.abs(y)
        over = a > self.knee
        if not np.any(over):
            return y
        span = 1.0 - self.knee
        soft = self.knee + span * np.tanh((a[over] - self.knee) / span)
        out = y.copy()
        out[over] = np.sign(y[over]) * soft
        return out


def resample_whole(x: np.ndarray, src_rate: int, dst_rate: int) -> np.ndarray:
    """Resample a complete buffer (used for TTS output when the client rate differs)."""
    if src_rate == dst_rate:
        return x
    r = Resampler(src_rate, dst_rate)
    tail = np.zeros(r.h.size, dtype=np.float32)
    return np.concatenate([r.process(x), r.process(tail)])[: int(round(x.size * dst_rate / src_rate))]
