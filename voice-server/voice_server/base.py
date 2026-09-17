"""What every engine shares: the socket, the control frames, the text channel, the tool bridge."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import time
from dataclasses import dataclass, field

import httpx
import numpy as np

from . import protocol as P
from .audio import Normalizer, float_to_pcm16, pcm16_to_float, resample_whole
from .echo import EchoGate
from .engines import Engines
from .kernel import KernelClient, hub_attach_url, kernel_alive, kernel_url_of
from .activity import ActivityReporter
from .narrator import Narrator, is_conversational, openrouter_key
from .tools import CALL_TOOLS, TOOLS, ToolRunner

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
    # Narrator model for `more_detail` answers (OpenRouter id); None = extractive answers only.
    narrator_model: str | None = None
    # Highlights by the narrator model (True) or by the policy alone (False).
    model_highlights: bool = False
    # Seconds an `allow …` ask waits for the caller before it is denied.
    approval_timeout: float = 45.0
    # JSONL of question-shaped utterances that went to the agent right after a highlight (for
    # growing the drill-down phrase list). Empty = off.
    escalations_log: str = ""
    # Call mode, duplex engine: how much of the speech model's own voice the caller hears.
    # `auto` = its answers to small talk and general questions (the phone's feel), while work
    # requests are narrated; `ack` = short acknowledgements only; `full` = everything; `off` = none.
    model_voice: str = "auto"
    # Duplex engine outside call mode: who answers a spoken turn. kernel | model | auto (kernel unless small talk).
    answerer: str = "auto"


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
        # Call mode: the caller talks to a project's main agent; the narrator speaks highlights.
        self.call_mode = False
        self.channel = "voice"  # what the caller's utterances are filed as in the inbox
        self.device = ""  # the client on the call: phone | desktop (session.start.device)
        self.project = ""  # `<machine>/<project>` from session.start; empty = the gateway's kernel
        self.screen = "on your screen"
        self.narrator: Narrator | None = None
        self.activity: ActivityReporter | None = None  # call mode: agent.activity frames to the client
        self.call_kernel: KernelClient | None = None  # a per-call attach through the hub, when the call names one
        self.project_info: dict | None = None  # machine/project/name/icon/store/kind from the hub roster, when scoped
        self.project_path = ""  # the place's folder as the client sent it (session.start.project.path)
        self.start_context: dict = {}  # session.start.project.context: the chat so far, as the client showed it
        self.dictation = False  # ASR only: words to the client, no reply, no agent, no asks
        self.last_conversational = False  # the last utterance was small talk (auto model voice lets it through)
        self.user_talking = False

    # ------------------------------------------------------------------ hooks for engines

    async def on_open(self) -> None: ...

    async def on_start(self) -> None:
        """After session.start was applied (rate/voice/... may have changed)."""

    def _ensure_asker(self) -> None:
        """Outside call mode a kernel still asks questions and for approvals. Nobody auto-fills
        them: a narrator that speaks only asks and approvals takes the caller's yes or no."""
        if self.narrator is not None or self.engines.kernel is None or self.dictation:
            return
        self.narrator = Narrator(
            self.engines.kernel,
            speak=self.speak_narration,
            emit=self._emit,
            screen=self.screen,
            device=self.device,
            user_talking=lambda: self.user_talking,
            arbos_talking=self.arbos_talking,
            ack=False,
            speak_details=False,
            only_asks=True,
            approval_timeout=self.defaults.approval_timeout,
        )
        self.narrator.start()

    async def on_audio(self, data: bytes) -> None: ...

    async def on_speak(self, text: str) -> None: ...

    async def on_interrupt(self, cause: str) -> None: ...

    async def on_report_speech(self, text: str) -> None:
        """Voice an agent's report. Default: the gateway's own TTS."""
        await self._speak(text, self.gen)
        self._emit_for_gen(self.gen, P.RESPONSE_DONE)

    def arbos_talking(self) -> bool:
        """Is reply audio (the model's or ours) on its way to the caller right now?"""
        return False

    async def on_close(self) -> None: ...

    @property
    def kernel(self) -> KernelClient | None:
        """The kernel this call talks to: the per-call hub attach when there is one, else the gateway's."""
        return self.call_kernel or self.engines.kernel

    # ------------------------------------------------------------------ call mode

    def context_text(self) -> str:
        """The chat so far as the client sent it at session.start, for the speech model's
        instructions: the last lines and the sub-agents. Empty when the client sent none."""
        return context_text(self.start_context)

    def on_user_final(self, text: str) -> None:
        """A finished caller utterance. In call mode it goes to the main agent as a `voice` message;
        outside it, only a pending approval or question takes it (as the yes/no or the answer)."""
        self.user_talking = False
        if not text.strip() or self.narrator is None:
            return
        self.last_conversational = is_conversational(text)
        if self.call_mode:
            self.narrator.user_said_later(text, channel=self.channel)
        elif self.narrator.pending_ask is not None:
            self.narrator.user_said(text, channel=self.channel)

    async def on_call_text(self, text: str) -> None:
        """Typed during a call (text.input): the same inbox as the spoken words, filed as `text`.
        The kernel takes it as a steer when a turn is running."""
        self.narrator.user_said(text, channel="text")
        self._emit(P.TEXT_DONE, text="", cancelled=False, forwarded=True)

    def note_interrupt(self) -> None:
        """The caller cut in (barge-in or an `interrupt` frame): the narrator drops what it was saying."""
        if self.narrator is not None:
            self.narrator.interrupted()

    async def speak_narration(self, text: str) -> None:
        """Voice one narrator line as a reply turn the client can play: response.started,
        response.transcript, audio, response.done. Interrupted audio is dropped by the gen tag."""
        gen = self.gen
        self._emit_for_gen(gen, P.RESPONSE_STARTED, speaker="narrator")
        self._emit_for_gen(gen, P.RESPONSE_TRANSCRIPT, text=text)
        await self._speak(text, gen)
        self._emit_for_gen(gen, P.RESPONSE_DONE, speaker="narrator")

    async def _start_call(self) -> None:
        if self.narrator is not None and not self.narrator.only_asks:
            return
        if self.narrator is not None:
            self.narrator.close()
            self.narrator = None
        kernel = await self._kernel_for(self.project)
        if kernel is None:
            self._emit(P.ERROR, message="call mode needs a kernel behind the gateway; staying in plain voice mode")
            self.call_mode = False
            return
        if kernel is not self.engines.kernel:
            # The call's own attach: the tools and the agent mirror follow it.
            self.call_kernel = kernel
            self.tools.rebind(kernel)
            if self.engines.kernel and self._mirror in self.engines.kernel.listeners:
                self.engines.kernel.listeners.remove(self._mirror)
            kernel.listeners.append(self._mirror)
        self.narrator = Narrator(
            kernel,
            speak=self.speak_narration,
            emit=self._emit,
            screen=self.screen,
            device=self.device,
            model=self.defaults.narrator_model,
            api_key=openrouter_key() if self.defaults.narrator_model else None,
            user_talking=lambda: self.user_talking,
            arbos_talking=self.arbos_talking,
            # With the speech model's voice off (or cut to acks), the narrator says "On it." and
            # reads more_detail answers itself; with it in full, the model does both.
            ack=self.defaults.model_voice != "full",
            speak_details=self.defaults.model_voice != "full",
            model_highlights=self.defaults.model_highlights,
            approval_timeout=self.defaults.approval_timeout,
            escalations_log=self.defaults.escalations_log,
            conversation_to_model=self.defaults.model_voice == "auto",
        )
        self.tools.narrator = self.narrator
        self.tools.schemas = CALL_TOOLS
        self.narrator.start()
        # What the kernel is doing, as frames, so the client can play the sound of work and
        # show it. From the kernel's own turn and tool frames, never from what was said.
        if self.activity is None:
            self.activity = ActivityReporter(kernel, self._emit)
            self.activity.start()
        log.info("[%s] call mode: narrating %s (channel %s)", self.sid, self.project or "the gateway's kernel", self.channel)

    async def _kernel_for(self, project: str) -> KernelClient | None:
        """The kernel a call is for. Empty, or a name for this gateway's own kernel: that one. A hub
        name (`<machine>/<project>`) with a hub configured: a fresh attach through the hub, owned
        by this call. Any other name: none — a call named for a project never talks to a different
        kernel (session.start already refused it; this is the safety net behind that)."""
        own = self.engines.kernel
        if self.call_kernel is not None:
            return self.call_kernel  # session.start named the project and _attach_project already attached
        if (self.project_info or {}).get("via") == "own":
            return own  # session.start sent the path and it is the folder this gateway's kernel serves
        if not project or project in self.engines.own_project_names():
            return own
        if "/" not in project or not self.engines.hub_url:
            log.warning("[%s] no kernel for %r (bare name or no --hub); not using another", self.sid, project)
            return None
        url = hub_attach_url(self.engines.hub_url, project, self.engines.hub_token)
        client = KernelClient(url=url, auto_approve=self.engines.auto_approve, token=self.engines.hub_token, name=project)
        try:
            await client.connect()
        except Exception as exc:
            log.warning("[%s] hub attach to %s failed: %s", self.sid, project, exc)
            self._emit(P.ERROR, message=f"could not reach {project} through the hub ({_ascii_short(exc)}); no other kernel takes the call")
            return None
        log.info("[%s] call attached to %s through the hub", self.sid, project)
        return client

    # ------------------------------------------------------------------ project scoping

    async def _scope_call(self, target: "Target") -> bool:
        """Bind this call to exactly the project the client opened. The client sends the
        place's path with its machine and folder name; the path decides:

        1. The gateway's own kernel serves that very folder: use it.
        2. The folder is on this host with a live kernel: attach to it (the desktop and the
           gateway on one Mac, any number of open tabs).
        3. The folder is on this host with no kernel: refuse — never another folder's agent.
        4. Otherwise the hub, matched by the roster's `place` when the path is known, else by
           `<machine>/<project>`; refused when the roster has no such project.

        Without a path (older clients) the name decides as before: the gateway's own kernel
        when the name is one of its own, the hub otherwise."""
        own = self.engines.kernel
        own_place = str(getattr(own, "place", "") or "") if own is not None else ""
        if target.path:
            want = _norm(target.path)
            if own_place and _norm(own_place) == want:
                log.info("[%s] call scoped to the gateway's own kernel at %s", self.sid, want)
                self.project_info = _local_info(target, want) | {"via": "own"}
                return True
            if os.path.isdir(want) and not target.host:
                url = kernel_url_of(want)
                if url and own is not None and _same_kernel_url(url, str(getattr(own, "url", "") or "")):
                    # The gateway's kernel was named by address (--kernel tcp://...), not by folder;
                    # the folder's kernel.json says it is this one.
                    log.info("[%s] call scoped to the gateway's own kernel (by address) at %s", self.sid, want)
                    self.project_info = _local_info(target, want) | {"via": "own"}
                    return True
                if url and kernel_alive(url):
                    return await self._attach_local(target, want, url)
                return await self._refuse(
                    "project_offline", target.label,
                    f"{target.label} is open at {want} on this machine but no kernel is running there; open the folder in the desktop first",
                )
        if not target.path and target.label in self.engines.own_project_names():
            return True
        return await self._attach_project(target.machine, target.project, path=target.path)

    async def _attach_local(self, target: "Target", place: str, url: str) -> bool:
        kernel = KernelClient(url=url, auto_approve=self.engines.auto_approve, name=target.label or place)
        kernel.place = place
        try:
            await asyncio.wait_for(kernel.connect(), 10)
        except Exception as exc:
            await kernel.close()
            return await self._refuse("project_unreachable", target.label, f"the kernel at {place} did not answer: {_ascii_short(exc)}")
        self._swap_kernel(kernel, target.label)
        self.project_info = _local_info(target, place)
        log.info("[%s] call scoped to the kernel at %s (%s)", self.sid, place, target.label)
        return True

    def _swap_kernel(self, kernel: KernelClient, label: str) -> None:
        """The call's tools and the agent mirror follow its own attach."""
        self.call_kernel = kernel
        self.project = label
        self.tools.rebind(kernel)
        if self.engines.kernel and self._mirror in self.engines.kernel.listeners:
            self.engines.kernel.listeners.remove(self._mirror)
        if self.mirror_agents:
            kernel.listeners.append(self._mirror)

    async def _attach_project(self, machine: str, project: str, path: str = "") -> bool:
        """Attach this call to `<machine>/<project>` through the hub. Refuses (error frame, then
        close 4404) when there is no hub or the project is not on the roster, not live, or
        does not answer; never falls back to another kernel."""
        hub, token = self.engines.hub_url, self.engines.hub_token
        label = f"{machine}/{project}" if machine else project
        if not machine:
            # The client is off the hub (no machine name) and the folder is not on this host: nothing
            # can reach that kernel. Say what to do; never take another kernel.
            return await self._refuse("project_not_on_hub", label, _not_on_hub_message(label, path))
        if not hub:
            return await self._refuse("no_hub", label, "this voice server has no hub configured, so it cannot scope a call to a project")
        info = await self._roster_lookup(hub, token, machine, project, path=path)
        if info is None:
            return await self._refuse("project_unknown", label, f"{label} is not on the hub roster")
        # The roster may know the project under another name than the client's folder; attach by
        # the roster's name so the words land in the right place.
        project = str(info.get("name") or project)
        label = f"{machine}/{project}"
        if not info.get("live", True):
            return await self._refuse("project_offline", label, f"{label} is on the roster but its kernel is not running")
        url = hub_attach_url(hub, label, token)
        kernel = KernelClient(url=url, auto_approve=self.engines.auto_approve, token=token, name=label)
        try:
            await asyncio.wait_for(kernel.connect(), 15)
        except Exception as exc:
            await kernel.close()
            return await self._refuse("project_unreachable", label, f"could not attach to {label}: {_ascii_short(exc)}")
        self._swap_kernel(kernel, label)
        identity = info.get("identity") or {}
        self.project_info = {
            "machine": machine, "project": project,
            "name": identity.get("name") or project, "icon": identity.get("icon"),
            "store": info.get("store") or f"arbos://{machine}/{project}/",
            "kind": info.get("kind", "project"),
            # the folder the project's kernel serves, from the roster (else the path the client sent):
            # the call's working directory. `place` is the same value under the roster's name.
            "path": info.get("place") or path or "",
            "place": info.get("place") or path or None,
            "via": "hub",
        }
        log.info("[%s] call scoped to %s (%s)", self.sid, label, self.project_info["name"])
        return True

    async def _roster_lookup(self, hub: str, token: str | None, machine: str, project: str, path: str = "") -> dict | None:
        base = hub.replace("wss://", "https://").replace("ws://", "http://").rstrip("/")
        headers = {"Authorization": f"Bearer {token}"} if token else {}
        try:
            async with httpx.AsyncClient(timeout=8.0) as client:
                roster = (await client.get(f"{base}/list", headers=headers)).json()
        except Exception as exc:
            log.warning("[%s] hub roster unavailable: %s", self.sid, type(exc).__name__)
            return {"live": True}  # cannot check; let the attach itself decide
        for m in roster.get("machines", []):
            if m.get("name") != machine:
                continue
            projects = m.get("projects", [])
            # The folder decides when both sides know it: two projects may share a leaf name.
            if path:
                for pr in projects:
                    if pr.get("place") and _norm(str(pr["place"])) == _norm(path):
                        return pr
            for pr in projects:
                if pr.get("name") == project:
                    return pr
        return None

    async def _refuse(self, code: str, label: str, message: str) -> bool:
        log.warning("[%s] refused call for %s: %s", self.sid, label, code)
        self._emit(P.ERROR, code=code, project=label, message=message)
        await asyncio.sleep(0.2)  # let the error frame leave before the close
        try:
            await self.ws.close(4404, f"project unreachable: {code}")
        except Exception:
            pass
        return False

    # ------------------------------------------------------------------ lifecycle

    async def run(self, first: str | bytes | None = None) -> None:
        sender = asyncio.create_task(self._sender(), name=f"send-{self.sid}")
        log.info("[%s] connected (%s)", self.sid, self.engine)
        try:
            await self.on_open()
            stop = False
            if first is not None:
                stop = await self._take(first)
            if not self.dictation:
                if self.engines.kernel and self._mirror not in self.engines.kernel.listeners:
                    self.engines.kernel.listeners.append(self._mirror)  # transcript mirror + agent.activity
                self._ensure_asker()
            if not stop:
                async for message in self.ws:
                    if await self._take(message):
                        break
        finally:
            if self.engines.kernel and self._mirror in self.engines.kernel.listeners:
                self.engines.kernel.listeners.remove(self._mirror)
            if self.activity is not None:
                self.activity.close()
            if self.narrator is not None:
                log.info("[%s] narrator: %s %s", self.sid, self.narrator.stats,
                         {k: v for k, v in self.narrator.bench.items() if v})
                self.narrator.close()
            self.tools.close()
            if self.call_kernel is not None:
                await self.call_kernel.close()
                self.call_kernel = None
            if self.text_task:
                self.text_task.cancel()
            try:
                await self.on_close()
            finally:
                sender.cancel()
            if self.echo:
                log.info("[%s] echo gate: %s", self.sid, self.echo.stats)
            log.info("[%s] closed", self.sid)

    async def _take(self, message: str | bytes) -> bool:
        """One frame from the client. True when the session should end."""
        if isinstance(message, (bytes, bytearray)):
            if not self.ready_sent:
                self._send_ready()
            await self.on_audio(self._gate_uplink(bytes(message)))
            return False
        return await self._on_control(message)

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
            kernel=bool((self.call_kernel or self.engines.kernel) and (self.call_kernel or self.engines.kernel).connected),
            voice=self.voice,
            mode="call" if self.call_mode else ("dictation" if self.dictation else "voice"),
            narrator=self.narrator is not None,
            activity=self.activity is not None,  # agent.activity frames follow
            channel=self.channel,
            device=self.device,
            project=(self.call_kernel.name if self.call_kernel else (self.project or "")),
            project_info=self.project_info,
            project_path=(self.project_info or {}).get("path") or (str(getattr(self.call_kernel or self.engines.kernel, "place", "") or "")),
            answerer=getattr(self, "answerer", "n/a"),
            via=(self.project_info or {}).get("via") or ("hub" if self.call_kernel else "gateway"),
        )

    def tools_available(self) -> list[dict]:
        if not self.engines.kernel:
            return []
        return CALL_TOOLS if self.call_mode else TOOLS

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
            target = _parse_target(msg)
            if target is not None and self.call_kernel is None and not await self._scope_call(target):
                return True  # refused: the caller asked for a project we cannot reach; never another one
            elif target is None and self.project and self.project not in self.engines.own_project_names():
                # A bare folder name ("discord_backups") from a machine that is not on the hub, with no
                # path (an older client). The gateway cannot reach that kernel, and it used to answer
                # from its own instead — a call about Jacob's Mac project talking to the phone kernel on
                # ArbosLife. Refuse.
                if not await self._refuse("project_not_on_hub", self.project, _not_on_hub_message(self.project)):
                    return True
            if self.call_kernel is None and self.project_info is None and self.engines.kernel is not None:
                self.project_info = self.engines.own_project_info()
            if self.call_mode and (self.narrator is None or self.narrator.only_asks):
                await self._start_call()
                if self.narrator is not None and self.start_context:
                    self.narrator.seed(self.start_context)
            await self.on_start()
            self._send_ready()
        elif kind == P.SPEAK:
            text = str(msg.get("text", "")).strip()
            if text:
                await self.on_speak(text)
        elif kind == P.INTERRUPT:
            self.note_interrupt()
            await self.on_interrupt("client")
        elif kind == P.TEXT_INPUT:
            text = str(msg.get("text", "")).strip()
            if text and self.call_mode and self.narrator is not None:
                await self.on_call_text(text)
            elif text:
                self._start_text_turn(text)
        elif kind == P.TEXT_CANCEL:
            if self.text_task and not self.text_task.done():
                self.text_task.cancel()
        elif kind == P.CLIENT_SPEAKING:
            log.debug("[%s] client.speaking %s", self.sid, bool(msg.get("speaking", False)))
            if self.echo is not None:
                self.echo.client_speaking = bool(msg.get("speaking", False))
                route = str(msg.get("route", "") or "").lower()
                if route:  # headsets cancel their own echo; the gate would only get in the way
                    bypass = route in ("airpods", "headset", "headphones", "bluetooth", "wired", "earpiece")
                    if bypass != self.echo.bypass:
                        self.echo.bypass = bypass
                        self.echo.confirmations = 0  # a new route is a new echo path
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
        target = _parse_target(msg)
        if target is not None:
            self.project = target.label  # dict, "machine/project" or arbos:// forms all land here
            self.project_path = target.path
            self.start_context = target.context
        elif isinstance(msg.get("project"), str):
            self.project = msg["project"].strip()  # a bare name: this gateway's own kernel
        answerer = msg.get("answerer")
        if answerer in ("kernel", "model", "auto") and hasattr(self, "answerer"):
            self.answerer = answerer
        if msg.get("channel") in ("voice", "text"):
            self.channel = msg["channel"]
        if isinstance(msg.get("device"), str):
            self.device = msg["device"].strip()[:24]
        if isinstance(msg.get("screen"), str) and msg["screen"].strip():
            self.screen = msg["screen"].strip()
        mode = msg.get("mode")
        if mode == "call":
            self.call_mode = True
        elif mode == "voice":
            self.call_mode = False
        elif mode == "dictation":
            self.dictation = True
            self.call_mode = False
            self.reply_kind = "none"
            self.mirror_agents = False

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


def _ascii_short(exc: BaseException) -> str:
    return str(exc).encode("ascii", "ignore").decode()[:120]


def _speak_name(agent: str) -> str:
    """Kernel agent ids are squashed words ('writeahaikuaboutriversto'); say something shorter."""
    return agent[:24]


@dataclass
class Target:
    """What the client opened: the machine and folder name the hub knows it by, and — the part
    that decides — the place's path. `host` is the client's ssh alias for a remote place (empty
    for a folder on the client's own machine)."""

    machine: str
    project: str
    path: str = ""
    host: str = ""
    name: str = ""
    context: dict = field(default_factory=dict)  # {"recent": [{"role","text"}], "agents": [{"name","state","step"}], "running": bool}

    @property
    def label(self) -> str:
        return f"{self.machine}/{self.project}" if self.machine else self.project


def _parse_target(msg: dict) -> Target | None:
    """session.start may name the project as {"project": {"machine","project"|"place","path","host","name"}},
    "project": "machine/project", {"kernel": "machine/project"}, or an "arbos://machine/project/" address.
    A dict with a `path` is the desktop's full identity of the open tab; the path is what binds the call."""
    raw = msg.get("project") or msg.get("kernel")
    if not raw:
        return None
    if isinstance(raw, dict):
        machine = str(raw.get("machine", "")).strip()
        project = str(raw.get("project") or raw.get("place") or "").strip()
        path = str(raw.get("path") or "").strip()
        if not project and path:
            project = os.path.basename(path.rstrip("/"))
        if not project:
            return None
        context = raw.get("context") if isinstance(raw.get("context"), dict) else {}
        return Target(machine=machine, project=project, path=path, host=str(raw.get("host") or "").strip(),
                      name=str(raw.get("name") or "").strip(), context=context)
    text = str(raw).strip()
    if text.startswith("arbos://"):
        text = text[len("arbos://"):]
    parts = [p for p in text.strip("/").split("/") if p]
    if len(parts) < 2:
        return None  # a bare name means the gateway's own kernel (see Engines.own_project_names)
    machine, project = parts[:2]
    if not machine or not project:
        return None
    return Target(machine=machine, project=project)


def _not_on_hub_message(project: str, path: str = "") -> str:
    where = f" (open at {path} on your machine)" if path else ""
    return (
        f"{project!r}{where} names a project on a machine that is not on the hub, and this voice server does "
        f"not serve it. A call can only reach a project through the hub as <machine>/<project>: on the machine "
        f"that has the folder, put the hub url, machine name and token in ~/.config/arbos/hub.toml, restart its "
        f"kernel so it registers, then call again. Not attaching to any other kernel."
    )


def _norm(path: str) -> str:
    return os.path.realpath(os.path.expanduser(path)).rstrip("/")


def _same_kernel_url(a: str, b: str) -> bool:
    def canon(u: str) -> str:
        u = u.strip().removeprefix("tcp://")
        return u.replace("localhost", "127.0.0.1").rstrip("/")
    return bool(a and b) and canon(a) == canon(b)


def _local_info(target: Target, place: str) -> dict:
    return {
        "machine": target.machine, "project": target.project, "name": target.name or target.project,
        "icon": None, "store": f"arbos://{target.machine}/{target.project}/" if target.machine else "",
        "kind": "project", "path": place, "via": "local",
    }


def context_text(context: dict, *, lines: int = 12, clip_at: int = 300) -> str:
    """`session.start.project.context` as a few plain lines of text."""
    if not context:
        return ""
    out: list[str] = []
    recent = [l for l in context.get("recent", []) if isinstance(l, dict) and str(l.get("text", "")).strip()]
    if recent:
        out.append("The chat so far (latest last):")
        for line in recent[-lines:]:
            role = {"user": "user", "assistant": "arbos", "worker": "worker", "tool": "tool"}.get(str(line.get("role", "")), str(line.get("role", "")))
            text = " ".join(str(line.get("text", "")).split())
            out.append(f"- {role}: {text[:clip_at]}{'...' if len(text) > clip_at else ''}")
    agents = [a for a in context.get("agents", []) if isinstance(a, dict) and a.get("name")]
    if agents:
        out.append("Sub-agents on the screen:")
        for a in agents:
            step = f" - {a['step']}" if a.get("step") else ""
            out.append(f"- {a['name']}: {a.get('state', '')}{step}")
    if context.get("running"):
        out.append("The main agent has a turn running right now.")
    return "\n".join(out)
