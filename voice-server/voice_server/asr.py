"""Speech-to-text backends. All take float32 mono 16 kHz and return text."""

from __future__ import annotations

import logging
import threading
from typing import Protocol

import numpy as np

log = logging.getLogger("voice.asr")

# Whisper produces these on silence or noise; drop them when the clip is short.
HALLUCINATIONS = {
    "thank you.", "thanks for watching.", "thank you for watching.", "you", "you.", "bye.", "thanks.",
    "subtitles by the amara.org community", "thank you very much.", ".", "...",
}


class ASR(Protocol):
    name: str

    def transcribe(self, audio16k: np.ndarray, *, partial: bool, language: str | None) -> str: ...


class FasterWhisperASR:
    """faster-whisper (CTranslate2). Runs on CPU (int8) or CUDA (float16).

    Streaming is chunked: the session re-decodes the growing utterance for
    partials and once more at the end for the final text. Not natively
    streaming, but one dependency, every language, and runs anywhere.
    """

    def __init__(self, model: str, device: str, compute_type: str, beam_size: int, threads: int):
        from faster_whisper import WhisperModel  # heavy import, keep it inside the backend

        kwargs = {"device": device, "compute_type": compute_type}
        if device == "cpu":
            kwargs["cpu_threads"] = threads
        self.model = WhisperModel(model, **kwargs)
        self.name = f"faster-whisper/{model}@{device}/{compute_type}"
        self.beam_size = beam_size
        self.lock = threading.Lock()

    def transcribe(self, audio16k: np.ndarray, *, partial: bool, language: str | None) -> str:
        if audio16k.size < 1600:  # under 100 ms: nothing to say
            return ""
        with self.lock:
            segments, _info = self.model.transcribe(
                audio16k,
                language=language,
                beam_size=1 if partial else self.beam_size,
                best_of=1,
                temperature=0.0,
                vad_filter=False,
                condition_on_previous_text=False,
                without_timestamps=True,
                word_timestamps=False,
            )
            parts = []
            for seg in segments:
                if seg.no_speech_prob > 0.8 and seg.avg_logprob < -1.0:
                    continue
                parts.append(seg.text.strip())
        text = " ".join(p for p in parts if p).strip()
        if audio16k.size < 16000 * 2 and text.lower() in HALLUCINATIONS:
            return ""
        return text


def build_asr(kind: str, *, model: str, device: str, compute_type: str, beam_size: int, threads: int) -> ASR:
    if kind == "faster-whisper":
        return FasterWhisperASR(model, device, compute_type, beam_size, threads)
    raise ValueError(f"unknown ASR backend {kind!r}")
