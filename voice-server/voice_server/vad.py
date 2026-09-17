"""Silero VAD (v5 ONNX) without torch: one onnxruntime session, one state per stream."""

from __future__ import annotations

import os

import numpy as np
import onnxruntime as ort

from .protocol import ASR_RATE

WINDOW = 512  # samples at 16 kHz = 32 ms, the size the v5 model was trained on
WINDOW_MS = WINDOW * 1000 // ASR_RATE
CONTEXT = 64  # v5 wants the last 64 samples of the previous window in front


class SileroVAD:
    def __init__(self, model_path: str):
        opts = ort.SessionOptions()
        opts.inter_op_num_threads = 1
        opts.intra_op_num_threads = 1
        opts.log_severity_level = 3
        self.session = ort.InferenceSession(model_path, sess_options=opts, providers=["CPUExecutionProvider"])
        self.input_names = {i.name for i in self.session.get_inputs()}

    def stream(self) -> "VADStream":
        return VADStream(self)


class EnergyVAD:
    """No model: speech probability from the window's loudness. For the test harness and for a
    machine without `silero_vad.onnx`; the duplex engine never consults it."""

    def __init__(self, threshold: float = 0.01):
        self.threshold = threshold

    def stream(self) -> "EnergyVADStream":
        return EnergyVADStream(self)


class EnergyVADStream:
    def __init__(self, vad: EnergyVAD):
        self.vad = vad
        self.pending = np.zeros(0, dtype=np.float32)

    def push(self, samples16k: np.ndarray) -> list[tuple[np.ndarray, float]]:
        self.pending = np.concatenate([self.pending, samples16k])
        out: list[tuple[np.ndarray, float]] = []
        while self.pending.size >= WINDOW:
            window, self.pending = self.pending[:WINDOW], self.pending[WINDOW:]
            rms = float(np.sqrt(np.mean(window * window)))
            out.append((window, min(1.0, rms / (self.vad.threshold * 2))))
        return out


def build_vad(model_path: str) -> SileroVAD | EnergyVAD:
    if os.path.exists(model_path):
        return SileroVAD(model_path)
    return EnergyVAD()


class VADStream:
    """Feeds 32 ms windows to the model and returns the speech probability of each."""

    def __init__(self, vad: SileroVAD):
        self.vad = vad
        self.state = np.zeros((2, 1, 128), dtype=np.float32)
        self.context = np.zeros(CONTEXT, dtype=np.float32)
        self.pending = np.zeros(0, dtype=np.float32)

    def push(self, samples16k: np.ndarray) -> list[tuple[np.ndarray, float]]:
        """Returns (window, speech_probability) for every full window now available."""
        self.pending = np.concatenate([self.pending, samples16k])
        out: list[tuple[np.ndarray, float]] = []
        while self.pending.size >= WINDOW:
            window, self.pending = self.pending[:WINDOW], self.pending[WINDOW:]
            out.append((window, self._prob(window)))
        return out

    def _prob(self, window: np.ndarray) -> float:
        x = np.concatenate([self.context, window])[None, :].astype(np.float32)
        feeds = {"input": x, "state": self.state, "sr": np.array(ASR_RATE, dtype=np.int64)}
        feeds = {k: v for k, v in feeds.items() if k in self.vad.input_names}
        prob, self.state = self.vad.session.run(None, feeds)
        self.context = window[-CONTEXT:]
        return float(prob.reshape(-1)[0])
