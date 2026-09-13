"""The loaded models and connections, shared by every session."""

from __future__ import annotations

import logging
import time
from dataclasses import dataclass

import httpx
import numpy as np

from .asr import ASR, build_asr
from .kernel import KernelClient
from .protocol import ASR_RATE
from .reply import ReplyBackend, build_reply
from .tts import TTS, build_tts
from .vad import EnergyVAD, SileroVAD, build_vad

log = logging.getLogger("voice.engines")


@dataclass
class Engines:
    vad: SileroVAD | EnergyVAD
    asr: ASR
    tts: TTS
    reply: ReplyBackend | None
    kernel: KernelClient | None
    engine: str  # "duplex" or "pipeline"
    duplex_url: str
    duplex_name: str
    hub_url: str = ""
    hub_token: str = ""
    hub_machine: str = ""
    auto_approve: bool = False  # per-call hub attaches: never

    def own_project_names(self) -> set[str]:
        """How a caller may name this gateway's own kernel: the place folder's name, and
        `<hub machine>/<name>` when the machine is known."""
        place = str(getattr(self.kernel, "place", "") or "")
        names: set[str] = set()
        if place:
            leaf = place.rstrip("/").rsplit("/", 1)[-1]
            names.add(leaf)
            if self.hub_machine:
                names.add(f"{self.hub_machine}/{leaf}")
        if self.hub_machine:
            names.add(self.hub_machine)
        return names

    @classmethod
    async def load(cls, args) -> "Engines":
        t0 = time.monotonic()
        kernel: KernelClient | None = None
        if args.kernel or args.kernel_place:
            kernel = KernelClient(url=args.kernel, place=args.kernel_place, auto_approve=bool(getattr(args, "auto_approve", False)))
            if kernel.auto_approve:
                log.warning("--auto-approve: the gateway answers its own kernel's allow asks unasked")
            try:
                await kernel.connect()
            except Exception as exc:
                raise SystemExit(f"could not attach to the Arbos kernel: {exc}")

        vad = build_vad(f"{args.model_dir}/silero_vad.onnx")
        asr = build_asr(
            args.asr, model=args.asr_model, device=args.device, compute_type=args.compute_type,
            beam_size=args.beam_size, threads=args.threads,
        )
        tts = build_tts(args.tts, model_dir=args.model_dir)
        reply = build_reply(args.reply, model=args.reply_model, kernel=kernel)

        engine = args.engine
        duplex_name = ""
        if engine in ("duplex", "auto"):
            duplex_name = await probe_duplex(args.duplex_url)
            if duplex_name:
                engine = "duplex"
            elif engine == "duplex":
                raise SystemExit(f"--engine duplex but nothing healthy at {args.duplex_url}")
            else:
                engine = "pipeline"
        log.info(
            "engines ready in %.1fs: engine=%s duplex=%s asr=%s tts=%s reply=%s kernel=%s",
            time.monotonic() - t0, engine, duplex_name or "-", asr.name, tts.name,
            reply.name if reply else "none", "attached" if kernel else "none",
        )
        return cls(vad=vad, asr=asr, tts=tts, reply=reply, kernel=kernel, engine=engine,
                   duplex_url=args.duplex_url, duplex_name=duplex_name,
                   hub_url=(getattr(args, "hub", None) or ""), hub_token=getattr(args, "hub_token", "") or "",
                   hub_machine=getattr(args, "hub_machine", "") or "", auto_approve=False)

    async def warm_up(self, voice: str) -> None:
        """First calls are slow (kernel selection, lazy loads). Pay that before the first caller."""
        t0 = time.monotonic()
        if self.asr.name not in ("none", "mock"):  # dictation uses it on either engine
            self.asr.transcribe(np.zeros(ASR_RATE, dtype=np.float32), partial=False, language="en")
        async for _ in self.tts.stream("Ready.", voice, 1.0):
            pass
        log.info("warm-up done in %.1fs", time.monotonic() - t0)


async def probe_duplex(ws_url: str) -> str:
    """Health-check the NemotronLabs VoiceChat container. Returns a model name or ''."""
    base = ws_url.replace("wss://", "https://").replace("ws://", "http://").split("/v1/")[0]
    try:
        async with httpx.AsyncClient(timeout=5.0) as client:
            health = (await client.get(f"{base}/v1/realtime/health")).json()
            if health.get("status") != "ok":
                log.warning("duplex model not ready: %s", health)
                return ""
            info = (await client.get(f"{base}/")).json()
            return f"nvidia/{info.get('model_name', 'nemotron-voicechat')}"
    except Exception as exc:
        log.warning("no duplex model at %s (%s)", base, exc)
        return ""
