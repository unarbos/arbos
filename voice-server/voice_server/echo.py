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

import logging
import time

import numpy as np

from .audio import Resampler

log = logging.getLogger("voice.echo")

GATE_RATE = 8_000  # correlation runs at 8 kHz: cheap, and speech energy lives below 4 kHz
REF_SECONDS = 3.0  # how far back we look for our own voice (covers up to ~1.5 s of client lead)
TAIL_S = 0.6  # keep gating this long after the last reply frame (playout drains)
FLOOR_RMS = 0.01  # -40 dBFS: quieter frames are never "the user talking over us"
WINDOW_SAMPLES = GATE_RATE * 160 // 1000  # correlate 160 ms of uplink, not one frame


class EchoGate:
    def __init__(self, rate: int, margin: float = 1.6):
        self.rate = rate
        self.margin = margin  # user must be this many times louder than the predicted echo to pass
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
        self.confirmations = 0  # clean-echo windows seen; the gate closes only after 2
        self.last_confirm_at = 0.0
        self.uplink = np.zeros(0, dtype=np.float32)  # last 160 ms of uplink at 8 kHz
        self.bypass = False  # client says the route has its own echo cancellation (headset)
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
        """Returns (audio to pass on, is_echo). Echo frames come back as silence.

        The gate only ever closes once an echo path has been *confirmed*: several recent
        uplink windows that match our reply audio closely (corr > 0.75). With a headset,
        in a simulator, or with good echo cancellation on the phone there is no such path,
        and every frame passes untouched. Correlation runs on the last 160 ms of uplink,
        not one 40 ms frame, so speech that merely resembles ours by chance does not match.
        """
        self.stats["frames"] += 1
        x8 = self.down_in.process(pcm_f32)  # keep the resampler state continuous
        self._push_uplink(x8)
        if self.bypass or (not self.active and not self.client_speaking):
            return pcm_f32, False
        if x8.size < 80:
            return pcm_f32, False
        in_rms = float(np.sqrt(np.mean(x8 * x8)))
        if in_rms < 1e-4:
            return pcm_f32, False
        now = time.monotonic()
        if now < self.user_until:
            # Mid-sentence from the user: let every frame through so the VAD sees one
            # continuous run. A little echo rides along; the words still dominate.
            self.stats["passed_over_echo"] += 1
            return pcm_f32, False
        window = self.uplink[-WINDOW_SAMPLES:]
        corr, lag = self._best_match(window)
        if corr > 0.75:  # a clean echo: this is how a path gets confirmed, and how its gain is learned
            seg = self._segment(lag, window.size)
            observed = float(np.sqrt(np.mean(window * window))) / (float(np.sqrt(np.mean(seg * seg))) + 1e-6)
            self.gain = observed if self.confirmations < 2 else 0.85 * self.gain + 0.15 * observed
            self.confirmations += 1
            self.last_confirm_at = now
        elif now - self.last_confirm_at > 300.0:
            self.confirmations = 0  # nothing matched for five minutes: forget the path (volume down, room changed)
        if self.confirmations < 2:
            return pcm_f32, False  # no confirmed echo path: nothing to gate
        threshold = 0.45 if self.client_speaking else 0.5
        if corr < threshold:
            return pcm_f32, False
        seg = self._segment(lag, window.size)
        predicted_echo = self.gain * (float(np.sqrt(np.mean(seg[-x8.size:] ** 2))) + 1e-6)
        # A person on top of the echo adds energy the echo path cannot explain.
        margin = self.margin * (0.85 if self.client_speaking else 1.0)
        if in_rms > FLOOR_RMS and in_rms > margin * predicted_echo:
            self.stats["passed_over_echo"] += 1
            self.user_until = now + 0.6  # keep the door open through the next few words
            log.debug("pass corr=%.2f in=%.4f pred=%.4f gain=%.2f lag=%d", corr, in_rms, predicted_echo, self.gain, lag)
            return pcm_f32, False
        self.stats["echo"] += 1
        return np.zeros_like(pcm_f32), True

    def _push_uplink(self, x8: np.ndarray) -> None:
        self.uplink = np.concatenate([self.uplink, x8])[-WINDOW_SAMPLES:]

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
