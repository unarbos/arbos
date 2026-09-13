"""The loaded models, shared by every session."""

from __future__ import annotations

import logging
import time
from dataclasses import dataclass

import numpy as np

from .asr import ASR, build_asr
from .protocol import ASR_RATE
from .reply import ReplyBackend, build_reply
from .tts import TTS, build_tts
from .vad import SileroVAD

log = logging.getLogger("voice.engines")


@dataclass
class Engines:
    vad: SileroVAD
    asr: ASR
    tts: TTS
    reply: ReplyBackend | None

    @classmethod
    def load(cls, args) -> "Engines":
        t0 = time.monotonic()
        vad = SileroVAD(f"{args.model_dir}/silero_vad.onnx")
        asr = build_asr(
            args.asr, model=args.asr_model, device=args.device, compute_type=args.compute_type,
            beam_size=args.beam_size, threads=args.threads,
        )
        tts = build_tts(args.tts, model_dir=args.model_dir)
        reply = build_reply(args.reply, model=args.reply_model)
        log.info("models loaded in %.1fs: %s, %s, reply=%s", time.monotonic() - t0, asr.name, tts.name,
                 reply.name if reply else "none")
        return cls(vad=vad, asr=asr, tts=tts, reply=reply)

    async def warm_up(self, voice: str) -> None:
        """First calls are slow (kernel selection, lazy loads). Pay that before the first caller."""
        t0 = time.monotonic()
        self.asr.transcribe(np.zeros(ASR_RATE, dtype=np.float32), partial=False, language="en")
        async for _ in self.tts.stream("Ready.", voice, 1.0):
            pass
        log.info("warm-up done in %.1fs", time.monotonic() - t0)
