"""Server-side echo gate: keeps the model from hearing its own voice through the phone's mic.

Works without client-side echo cancellation. Every reply frame the gateway sends
is remembered (a few seconds, downsampled). While we are talking, each uplink
frame is cross-correlated against that reference over all plausible delays
(playout buffer + acoustic path + network). A frame that matches is our own
echo: it is replaced with silence so the speech model's clock keeps running
but it hears nothing. A frame with clearly more energy than the echo predicts
(the user talking over us) passes, so barge-in still works. A `client.speaking`
marker from the app tightens the thresholds while it plays audio.
"""

from __future__ import annotations

import time

import numpy as np

from .audio import Resampler

GATE_RATE = 8_000  # correlation runs at 8 kHz: cheap, and speech energy lives below 4 kHz
REF_SECONDS = 3.0  # how far back we look for our own voice (covers up to ~1.5 s of client lead)
TAIL_S = 0.6  # keep gating this long after the last reply frame (playout drains)


class EchoGate:
    def __init__(self, rate: int):
        self.rate = rate
        self.ref = np.zeros(int(REF_SECONDS * GATE_RATE), dtype=np.float32)
        self.ref_pos = 0
        self.ref_filled = 0
        self.down_out = Resampler(rate, GATE_RATE)
        self.down_in = Resampler(rate, GATE_RATE)
        self.last_out_at = 0.0
        self.out_ahead_s = 0.0  # how far ahead of real time the reply audio was sent
        self.client_speaking = False
        self.gain = 0.3  # echo path gain estimate (uplink / reply), adapted while echo is seen
        self.user_until = 0.0  # while set, frames pass more easily (the user is mid-sentence)
        self.gain_samples = 0
        self.double_talk_streak = 0
        self.stats = {"frames": 0, "echo": 0, "passed_over_echo": 0}

    # ------------------------------------------------------------------ reference (our voice)

    def remember(self, pcm_f32: np.ndarray, ahead_s: float = 0.0) -> None:
        """Called for every reply frame the gateway emits (float32 at the client rate)."""
        x = self.down_out.process(pcm_f32)
        n = x.size
        if n == 0:
            return
        if n >= self.ref.size:
            self.ref[:] = x[-self.ref.size:]
            self.ref_pos = 0
        else:
            end = self.ref_pos + n
            if end <= self.ref.size:
                self.ref[self.ref_pos:end] = x
            else:
                k = self.ref.size - self.ref_pos
                self.ref[self.ref_pos:] = x[:k]
                self.ref[: n - k] = x[k:]
            self.ref_pos = end % self.ref.size
        self.ref_filled = min(self.ref.size, self.ref_filled + n)
        self.last_out_at = time.monotonic()
        self.out_ahead_s = max(0.0, ahead_s)

    @property
    def active(self) -> bool:
        """Are we (possibly) still audible at the client?"""
        horizon = self.out_ahead_s + TAIL_S
        return self.ref_filled > 0 and (time.monotonic() - self.last_out_at) < horizon

    # ------------------------------------------------------------------ uplink

    def filter(self, pcm_f32: np.ndarray) -> tuple[np.ndarray, bool]:
        """Returns (audio to pass on, is_echo). Echo frames come back as silence."""
        self.stats["frames"] += 1
        x8 = self.down_in.process(pcm_f32)  # keep the resampler state continuous
        if not self.active and not self.client_speaking:
            return pcm_f32, False
        if x8.size < 80:
            return pcm_f32, False
        in_rms = float(np.sqrt(np.mean(x8 * x8)))
        if in_rms < 1e-4:
            return pcm_f32, False
        now = time.monotonic()
        in_user_mode = now < self.user_until
        if in_user_mode:
            # Mid-sentence from the user: let every frame through so the VAD sees one
            # continuous run. A little echo rides along; the words still dominate.
            self.stats["passed_over_echo"] += 1
            return pcm_f32, False
        corr, lag = self._best_match(x8)
        threshold = 0.28 if self.client_speaking else 0.35
        if corr < threshold:
            return pcm_f32, False
        # It correlates with our voice. Take our voice out (one tap at the best lag) and see
        # how much is left: a person talking over us leaves most of their energy behind.
        seg = self._segment(lag, x8.size)
        seg_rms = float(np.sqrt(np.mean(seg * seg))) + 1e-6
        if corr > 0.6:  # clean echo: learn the path gain (fast at first, then slowly)
            observed = in_rms / seg_rms
            self.gain = observed if self.gain_samples < 4 else 0.85 * self.gain + 0.15 * observed
            self.gain_samples += 1
        residual = x8 - self.gain * seg
        residual_rms = float(np.sqrt(np.mean(residual * residual)))
        predicted_echo = self.gain * seg_rms
        margin = 0.55 if self.client_speaking else 0.7
        double_talk = self.gain_samples >= 4 and residual_rms > margin * predicted_echo and corr < 0.8
        if double_talk and self.double_talk_streak >= 1:  # two frames in a row, not one glitch
            self.stats["passed_over_echo"] += 1
            self.user_until = now + 0.4  # keep the door open through the next few frames
            return pcm_f32, False
        self.double_talk_streak = self.double_talk_streak + 1 if double_talk else 0
        self.stats["echo"] += 1
        return np.zeros_like(pcm_f32), True

    def _ordered_ref(self) -> np.ndarray:
        if self.ref_filled < self.ref.size:
            return self.ref[: self.ref_filled]
        return np.concatenate([self.ref[self.ref_pos:], self.ref[: self.ref_pos]])

    def _best_match(self, frame: np.ndarray) -> tuple[float, int]:
        """Normalised cross-correlation peak of `frame` against the reference, and its offset."""
        ref = self._ordered_ref()
        n, m = ref.size, frame.size
        if n < m:
            return 0.0, 0
        size = 1 << int(np.ceil(np.log2(n + m)))
        spec = np.fft.rfft(ref, size) * np.conj(np.fft.rfft(frame, size))
        xc = np.fft.irfft(spec, size)[: n - m + 1]
        # energy of every reference window of length m, for normalisation
        csum = np.concatenate([[0.0], np.cumsum(ref.astype(np.float64) ** 2)])
        win = csum[m:] - csum[:-m]
        norm = np.sqrt(win * float(np.dot(frame, frame))) + 1e-9
        ncc = xc / norm
        idx = int(np.argmax(ncc))
        return float(ncc[idx]), idx

    def _segment(self, lag: int, m: int) -> np.ndarray:
        ref = self._ordered_ref()
        return ref[lag : lag + m]
