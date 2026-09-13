"""What every engine shares: the socket, the control frames, the text channel, the tool bridge."""

from __future__ import annotations

import asyncio
import json
import logging
import time
from dataclasses import dataclass

import numpy as np

from . import protocol as P
from .audio import Normalizer, float_to_pcm16, pcm16_to_float, resample_whole
from .echo import EchoGate
from .engines import Engines
from .tools import ToolRunner

log = logging.getLogger("voice.session")


@dataclass
class Tuning:
    """Turn-taking knobs for the pipeline engine, plus output pacing. Milliseconds unless the name says otherwise."""

    start_threshold: float = 0.5
    end_threshold: float = 0.35
    min_speech_ms: int = 96  # 3 windows of confident speech before speech.started
    barge_in_min_ms: int = 256  # stricter while we are talking, so our own echo does not cut us off
    end_silence_ms: int = 600
    preroll_ms: int = 320
    partial_interval_ms: int = 700
    partial_window_s: int = 20
    max_utterance_s: int = 60
    out_frame_ms: int = 100
    max_lead_ms: int = 1500  # how far ahead of real-time playback we send reply audio
    echo_gate: bool = True  # silence uplink frames that are our own reply coming back through the mic
    echo_margin: float = 1.6  # the user must be this much louder than the predicted echo to count as talking over us
    normalize: bool = True  # peak-follow reply audio toward out_target_dbfs with a soft limiter
    out_target_dbfs: float = -3.0


@dataclass
class SessionDefaults:
    rate: int = P.DEFAULT_RATE
    language: str | None = "en"
    voice: str = "af_heart"
    speed: float = 1.0
    reply: str = "none"
    instructions: str | None = None


class BaseSession:
    engine = "base"

    def __init__(self, ws, engines: Engines, defaults: SessionDefaults, tuning: Tuning):
        self.ws = ws
        self.engines = engines
        self.defaults = defaults
        self.tuning = tuning
        self.sid = f"{int(time.time() * 1000) % 100000:05d}"
        self.rate = defaults.rate
        self.language = defaults.language
        self.voice = defaults.voice
        self.speed = defaults.speed
        self.reply_kind = defaults.reply if engines.reply else "none"
        self.instructions = defaults.instructions
        self.out: asyncio.Queue[tuple[int | None, bytes | str]] = asyncio.Queue()
        self.gen = 0  # bumped on interrupt; queued audio tagged with an older gen is dropped
        self.ready_sent = False
        self.history: list[dict] = []  # text-channel conversation (OpenAI message shape)
        self.text_task: asyncio.Task | None = None
        self.tools = ToolRunner(engines.kernel, on_report=self._on_agent_report)
        self.tools.on_call = self._on_tool_call
        self.tools.on_result = self._on_tool_result
        self.mirror_agents = engines.kernel is not None
        self.echo = EchoGate(self.rate, tuning.echo_margin) if tuning.echo_gate else None
        self.normalizers: dict[str, Normalizer] = {}

    # ------------------------------------------------------------------ hooks for engines

    async def on_open(self) -> None: ...

    async def on_start(self) -> None:
        """After session.start was applied (rate/voice/... may have changed)."""

    async def on_audio(self, data: bytes) -> None: ...

    async def on_speak(self, text: str) -> None: ...

    async def on_interrupt(self, cause: str) -> None: ...

    async def on_report_speech(self, text: str) -> None:
        """Voice an agent's report. Default: the gateway's own TTS."""
        await self._speak(text, self.gen)
        self._emit_for_gen(self.gen, P.RESPONSE_DONE)

    async def on_close(self) -> None: ...

    # ------------------------------------------------------------------ lifecycle

    async def run(self) -> None:
        sender = asyncio.create_task(self._sender(), name=f"send-{self.sid}")
        log.info("[%s] connected (%s)", self.sid, self.engine)
        if self.mirror_agents and self.engines.kernel:
            self.engines.kernel.listeners.append(self._mirror)
        try:
            await self.on_open()
            async for message in self.ws:
                if isinstance(message, (bytes, bytearray)):
                    if not self.ready_sent:
                        self._send_ready()
                    await self.on_audio(self._gate_uplink(bytes(message)))
                elif await self._on_control(message):
                    break
        finally:
            if self.engines.kernel and self._mirror in self.engines.kernel.listeners:
                self.engines.kernel.listeners.remove(self._mirror)
            self.tools.close()
            if self.text_task:
                self.text_task.cancel()
            try:
                await self.on_close()
            finally:
                sender.cancel()
            if self.echo:
                log.info("[%s] echo gate: %s", self.sid, self.echo.stats)
            log.info("[%s] closed", self.sid)

    def _gate_uplink(self, data: bytes) -> bytes:
        """Silence frames that are our own reply coming back through the mic."""
        if self.echo is None:
            return data
        samples, is_echo = self.echo.filter(pcm16_to_float(data))
        return float_to_pcm16(samples) if is_echo else data

    async def _sender(self) -> None:
        while True:
            gen, payload = await self.out.get()
            try:
                if gen is None or gen == self.gen:
                    await self.ws.send(payload)
            except Exception:  # connection gone; run() will notice and stop
                pass

    def _emit(self, msg_type: str, **fields) -> None:
        self.out.put_nowait((None, json.dumps({"type": msg_type, **fields})))

    def _emit_for_gen(self, gen: int, msg_type: str, **fields) -> None:
        self.out.put_nowait((gen, json.dumps({"type": msg_type, **fields})))

    def _emit_audio(self, gen: int, pcm: bytes, ahead_s: float = 0.0, source: str = "tts") -> None:
        """Every reply frame leaves through here: level normalisation (one gain state per
        source, so the quiet duplex model and the loud TTS do not pump each other), then the
        echo reference (post-gain, so the gate matches what the speaker actually plays)."""
        if self.tuning.normalize or self.echo is not None:
            samples = pcm16_to_float(pcm)
            if self.tuning.normalize:
                norm = self.normalizers.get(source)
                if norm is None:
                    norm = self.normalizers[source] = Normalizer(self.rate, self.tuning.out_target_dbfs)
                samples = norm.process(samples)
                pcm = float_to_pcm16(samples)
            if self.echo is not None and gen == self.gen:
                self.echo.remember(samples, ahead_s)
        self.out.put_nowait((gen, pcm))

    def _send_ready(self) -> None:
        self.ready_sent = True
        reply = self.engines.reply.name if (self.engines.reply and self.reply_kind != "none") else "none"
        self._emit(
            P.SESSION_READY,
            rate=self.rate,
            engine=self.engine,
            asr=self.engines.asr.name if self.engine == "pipeline" else self.engines.duplex_name,
            tts=self.engines.tts.name if self.engine == "pipeline" else self.engines.duplex_name,
            reply=reply,
            text=reply if reply != "none" else "none",
            tools=[t["name"] for t in self.tools_available()],
            kernel=bool(self.engines.kernel and self.engines.kernel.connected),
            voice=self.voice,
        )

    def tools_available(self) -> list[dict]:
        from .tools import TOOLS

        return TOOLS if self.engines.kernel else []

    # ------------------------------------------------------------------ control

    async def _on_control(self, raw: str) -> bool:
        """Returns True when the session should end."""
        try:
            msg = json.loads(raw)
            kind = msg["type"]
        except (json.JSONDecodeError, KeyError, TypeError):
            self._emit(P.ERROR, message="control frames must be JSON objects with a 'type'")
            return False

        if kind == P.SESSION_START:
            self._apply_start(msg)
            await self.on_start()
            self._send_ready()
        elif kind == P.SPEAK:
            text = str(msg.get("text", "")).strip()
            if text:
                await self.on_speak(text)
        elif kind == P.INTERRUPT:
            await self.on_interrupt("client")
        elif kind == P.TEXT_INPUT:
            text = str(msg.get("text", "")).strip()
            if text:
                self._start_text_turn(text)
        elif kind == P.TEXT_CANCEL:
            if self.text_task and not self.text_task.done():
                self.text_task.cancel()
        elif kind == P.CLIENT_SPEAKING:
            if self.echo is not None:
                self.echo.client_speaking = bool(msg.get("speaking", False))
        elif kind == P.SESSION_END:
            return True
        else:
            self._emit(P.ERROR, message=f"unknown message type {kind!r}")
        return False

    def _apply_start(self, msg: dict) -> None:
        fmt = msg.get("format") or {}
        rate = int(fmt.get("rate", self.rate) or self.rate)
        if not 8000 <= rate <= 48000:
            self._emit(P.ERROR, message=f"unsupported rate {rate}; using {self.rate}")
        elif rate != self.rate:
            self.rate = rate
            if self.echo is not None:
                self.echo = EchoGate(rate, self.tuning.echo_margin)
            self.normalizers.clear()
        if "language" in msg:
            self.language = msg["language"] or None
        voice = msg.get("voice")
        if voice:
            if voice in self.engines.tts.voices:
                self.voice = voice
            else:
                self._emit(P.ERROR, message=f"unknown voice {voice!r}; using {self.voice}")
        if "speed" in msg:
            self.speed = float(np.clip(float(msg["speed"]), 0.5, 2.0))
        if isinstance(msg.get("instructions"), str) and msg["instructions"].strip():
            self.instructions = msg["instructions"].strip()
        if "agents" in msg:
            self.mirror_agents = bool(msg["agents"]) and self.engines.kernel is not None
        reply = msg.get("reply")
        if reply in ("none", "openrouter", "kernel"):
            if reply != "none" and not self.engines.reply:
                self._emit(P.ERROR, message="server started without a reply backend; staying speech-only")
            else:
                self.reply_kind = reply

    # ------------------------------------------------------------------ gateway TTS (Kokoro)

    async def _speak(self, text: str, gen: int) -> float | None:
        """Streams the gateway's own TTS for `text`. Returns the monotonic time of the first frame.

        Frames go out ahead of real time, but only up to --max-lead-ms. That
        keeps the client's buffer small, so an interrupt stops the sound fast
        and does not waste bandwidth on audio nobody will hear.
        """
        first_at: float | None = None
        frame_bytes = self.rate * self.tuning.out_frame_ms // 1000 * 2
        frame_s = self.tuning.out_frame_ms / 1000
        lead_s = self.tuning.max_lead_ms / 1000
        sent_s = 0.0
        async for chunk in self.engines.tts.stream(text, self.voice, self.speed):
            if self.engines.tts.rate != self.rate:
                chunk = resample_whole(chunk, self.engines.tts.rate, self.rate)
            pcm = float_to_pcm16(chunk)
            for i in range(0, len(pcm), frame_bytes):
                ahead = 0.0
                if first_at is not None:
                    ahead = sent_s - (time.monotonic() - first_at)
                    if ahead > lead_s:
                        await asyncio.sleep(ahead - lead_s)
                        ahead = lead_s
                self._emit_audio(gen, pcm[i : i + frame_bytes], max(0.0, ahead))
                sent_s += frame_s
                if first_at is None:
                    first_at = time.monotonic()
        return first_at

    # ------------------------------------------------------------------ text channel

    def _start_text_turn(self, text: str) -> None:
        if self.text_task and not self.text_task.done():
            self._emit(P.ERROR, message="a text turn is already running; send text.cancel first")
            return
        self.text_task = asyncio.create_task(self._text_turn(text), name=f"text-{self.sid}")

    async def _text_turn(self, text: str) -> None:
        backend = self.engines.reply if self.reply_kind != "none" else None
        if backend is None:
            self._emit(P.ERROR, message="no text backend: start the server with --reply openrouter or --reply kernel")
            self._emit(P.TEXT_DONE, text="", cancelled=False)
            return
        self.history.append({"role": "user", "content": text})
        started = time.monotonic()
        full = ""
        try:
            async for delta in backend.stream(self.history, self.tools if self.tools_available() else None):
                full += delta
                self._emit(P.TEXT_DELTA, text=delta)
        except asyncio.CancelledError:
            self._emit(P.TEXT_DONE, text=full, cancelled=True)
            raise
        except Exception as exc:
            log.exception("[%s] text turn failed", self.sid)
            self._emit(P.ERROR, message=f"text turn failed: {exc}")
        if full:
            self.history.append({"role": "assistant", "content": full})
        self.history[:] = self.history[-40:]
        self._emit(P.TEXT_DONE, text=full, cancelled=False)
        log.info("[%s] text turn %d chars in %.1fs", self.sid, len(full), time.monotonic() - started)

    # ------------------------------------------------------------------ agents and tools

    async def _on_tool_call(self, name: str, args: dict) -> None:
        self._emit(P.TOOL_CALL, name=name, arguments=args)

    async def _on_tool_result(self, name: str, output: str) -> None:
        self._emit(P.TOOL_RESULT, name=name, output=output)

    async def _on_agent_report(self, agent: str, text: str) -> None:
        self._emit(P.AGENT_DONE, agent=agent, text=text)
        spoken = f"Agent {_speak_name(agent)} is back. {text}"
        self.history.append({"role": "assistant", "content": f"(report from agent {agent}) {text}"})
        try:
            await self.on_report_speech(spoken)
        except Exception:
            log.exception("[%s] could not voice the agent report", self.sid)

    def _mirror(self, frame: dict) -> None:
        if not self.mirror_agents:
            return
        kind = frame.get("type")
        if kind == "assistant_delta":
            if frame.get("agent") == "root" and frame.get("text"):  # only the main agent streams token by token
                self._emit(P.AGENT_EVENT, agent="root", kind="assistant", text=frame["text"])
        elif kind == "event":
            event = frame.get("event") or {}
            ek = event.get("kind")
            if ek in ("assistant", "say", "user", "notice", "ask"):
                if ek == "assistant":
                    if not event.get("text") or frame.get("agent") != "root":
                        return
                    ek = "assistant_final"  # the whole reply once the turn ends; replaces the streamed deltas
                self._emit(P.AGENT_EVENT, agent=frame.get("agent"), kind=ek, text=event.get("text"),
                           **({"from": event["from"]} if event.get("from") else {}))
            elif ek == "tool":
                self._emit(P.AGENT_EVENT, agent=frame.get("agent"), kind="tool", text=event.get("name"))
        elif kind == "turn":
            self._emit(P.AGENT_TURN, agent=frame.get("agent"), state=frame.get("state"))
        elif kind in ("tree", "snapshot"):
            self._emit(P.AGENT_TREE, agents=[
                {"id": n.get("id"), "name": n.get("name"), "parent": n.get("parent")} for n in frame.get("tree", [])
            ])


def _speak_name(agent: str) -> str:
    """Kernel agent ids are squashed words ('writeahaikuaboutriversto'); say something shorter."""
    return agent[:24]
