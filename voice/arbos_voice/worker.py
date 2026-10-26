"""arbos-voice: the LiveKit Agents worker that is the project agent's voice (Calls Phase 2, D75).

One process serves every call; LiveKit dispatches a job per (call, agent) with the relay's
metadata. The job joins as `a_<project_id>`, speaks with OpenAI `gpt-realtime-2.1`, and:

* **1:1 (duplex)** — the human's audio goes straight to the speech model (server VAD, barge-in
  by the model and by the framework); input/output transcripts become chat rows.
* **group** — nobody's audio reaches the speech model. A per-human listener (Silero VAD →
  `gpt-4o-mini-transcribe` per utterance) produces a speaker-labelled transcript; the judge
  (`judge.py`) decides speak / act / silent per utterance; only then does the model get the text
  and answer with audio. The agent waits for 700 ms of room silence before speaking and is cut
  at once when anyone starts talking.

Tools: `ask_agent(request)`, forwarded through the relay to agentd on the owner's machine, and
`look(what, hand_to_agent)`, a full-resolution camera still (vision.py). While a human's camera is
on the model also gets a small still about once a second (owner toggle `vision_enabled`).
Costs, time to first audio and barge-in cut time are measured here and reported to the relay
every 30 s and at the end; the relay's answer carries the cost stops.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import os
import sys
import threading
import time
import uuid
from dataclasses import dataclass, field
from typing import Any

import aiohttp
from livekit import rtc
from livekit.agents import (
    Agent,
    AgentSession,
    AutoSubscribe,
    JobContext,
    JobProcess,
    JobRequest,
    RoomInputOptions,
    RoomOutputOptions,
    RunContext,
    WorkerOptions,
    cli,
    function_tool,
)
from livekit.agents.metrics import RealtimeModelMetrics
from livekit.agents.voice.events import (
    AgentStateChangedEvent,
    ConversationItemAddedEvent,
    MetricsCollectedEvent,
    SpeechCreatedEvent,
    UserInputTranscribedEvent,
    UserStateChangedEvent,
)
from livekit.plugins import openai, silero
from openai.types.realtime.audio_transcription import AudioTranscription
from openai.types.realtime.realtime_audio_input_turn_detection import SemanticVad, ServerVad

from . import __version__

WORKER_SHA = os.environ.get("ARBOS_VOICE_SHA", "").strip() or None
from .config import CONFIG, Config
from .judge import CHATTINESS_THRESHOLD, Decision, Judge, JudgeContext, forced_decision
from .listener import GroupListeners, Utterance
from .meter import CallMeter
from .prompts import (
    ASK_AGENT_DESCRIPTION,
    LEAVE_CALL_DESCRIPTION,
    LOOK_DESCRIPTION,
    MUTE_SELF_DESCRIPTION,
    UNMUTE_SELF_DESCRIPTION,
    system_prompt,
)
from .relay import EventBatcher, Relay, RelayError, ToolPending
from .updates import CallUpdates, Update, context_item
from .vision import Vision, wants_a_look

log = logging.getLogger("arbos-voice")

ATTENTION_WINDOW_S = 20.0
# Group listeners: speech must last this long before it counts (and before it can barge in).
GROUP_BARGE_MIN_SPEECH_S = 0.35
# 1:1 (duplex) server VAD activation (0–1). Higher = less sensitive: ambient noise and
# speaker echo must be louder before they count as speech and cut the agent mid-sentence.
DUPLEX_VAD_THRESHOLD = 0.65
# An interrupted agent line shorter than this (words) that was cut within its first
# `TRUNCATED_KEEP_S` seconds is dropped from the transcript as a noise-triggered restart; a longer
# cut line, or a short one the humans heard almost whole, stays in the chat.
TRUNCATED_KEEP_WORDS = 6
TRUNCATED_KEEP_S = 1.2
# A lone STT token this long that is not a known short reply is almost always ambient-noise
# gibberish (e.g. "naabustame"), not a real one-word answer like "bet" / "okay".
NOISE_TOKEN_MIN_LEN = 8
# Phone / speakerphone mic: filter room noise before VAD so background sound does not become
# a user turn the model answers.
DUPLEX_NOISE_REDUCTION = "far_field"
# Until the owner's machine runs an agentd with the voice link (image roll), tools are off.
NO_LINK_LINE = "I can't run that for you yet on this call."

# Real brief spoken replies — never treat these as noise, even alone.
_SPOKEN_SHORT = frozenset(
    """
    hi hey hello yo sup yes yeah yep yup ya yah no nope nah ok okay k kk sure bet
    cool fine good great nice right true thanks thank ty thx bye goodbye later peace
    what why how when where who huh eh hm hmm mm mmm mhm uh um uhh umm ah oh ooh wow
    wait stop leave help please sorry go done ready you one two three four five six
    seven eight nine ten morning night afternoon tonight absolutely definitely
    tomorrow yesterday alright perfect awesome understood continue cancel deploy
    restart status update install python whatever interesting
    """.split()
)

GOODBYE = {
    "call_cap": "I've hit the spending cap for this call, so I'll drop off now. Everything we said is in the chat.",
    "voice_daily_cap": "I've reached today's voice budget, so I'll leave the call. I'm still here in the text chat.",
    "daily_cap": "Today's spending cap is reached, so I'll leave the call. I'm still here in the text chat.",
    "credits": "There are no credits left, so I'll leave the call.",
    "kill_switch": "Arbos is in read-only mode right now, so I'll leave the call.",
    "call_max": "We've been on for an hour, which is my limit per call. I'll drop off now.",
    "idle": "I'll drop off since it's gone quiet. Call me again any time.",
    "asked": "Okay, leaving. Bye.",
    "alone": "",
}


def now_ms() -> int:
    return int(time.time() * 1000)


def speaker_of(participant: rtc.RemoteParticipant | None, identity: str) -> dict:
    """`sender{}` of a human from their LiveKit metadata (set by the relay's join token)."""
    out: dict[str, Any] = {"kind": "user", "id": identity[2:] if identity.startswith("u_") else identity, "handle": "", "name": ""}
    if participant is not None:
        try:
            meta = json.loads(participant.metadata or "{}")
            if isinstance(meta, dict):
                out.update(
                    {
                        k: v
                        for k, v in meta.items()
                        if k in ("id", "handle", "name", "avatar_url", "kind", "platform") and v
                    }
                )
        except json.JSONDecodeError:
            pass
        if not out.get("name"):
            out["name"] = participant.name or ""
    return out


def label_of(speaker: dict) -> str:
    """How to speak about someone: display name, else bare handle (never @handle)."""
    name = (speaker.get("name") or "").strip()
    if name:
        return name
    handle = (speaker.get("handle") or "").strip()
    return handle or "someone"


def _norm_words(text: str) -> list[str]:
    return [w for w in "".join(c.lower() if c.isalnum() else " " for c in text).split() if w]


def is_noise_transcript(text: str) -> bool:
    """True when duplex STT almost certainly hallucinated ambient noise, not real speech.

    Keeps real short replies ("bet", "hey", "ok"). Drops one long nonsense token like
    "naabustame" that VAD+STT invent from background sound.
    """
    words = _norm_words(text)
    if not words:
        return True
    # Three or more words: treat as real speech (even if messy).
    if len(words) >= 3:
        return False
    if all(w in _SPOKEN_SHORT or len(w) <= 3 for w in words):
        return False
    if len(words) == 1:
        w = words[0]
        return w not in _SPOKEN_SHORT and len(w) >= NOISE_TOKEN_MIN_LEN
    # Two words: noise only when neither is a known short reply and at least one is a long junk token.
    return all(w not in _SPOKEN_SHORT for w in words) and any(len(w) >= NOISE_TOKEN_MIN_LEN for w in words)


def similar_reply(a: str, b: str) -> bool:
    """True when two agent lines are the same idea (interrupt + regenerate, or a near-repeat)."""
    a, b = a.strip(), b.strip()
    if not a or not b:
        return False
    if a == b:
        return True
    wa, wb = _norm_words(a), _norm_words(b)
    if not wa or not wb:
        return False
    sa, sb = set(wa), set(wb)
    overlap = len(sa & sb) / max(len(sa), len(sb))
    if overlap >= 0.6:
        return True
    # Truncated prefix of the regenerate (e.g. "what do you want" vs "what do you want to talk about").
    shorter, longer = (wa, wb) if len(wa) <= len(wb) else (wb, wa)
    return len(shorter) >= 3 and longer[: len(shorter)] == shorter


@dataclass
class CallState:
    mode: str = "duplex"  # duplex | group
    agent_state: str = "initializing"
    user_state: str = "listening"
    linked_identity: str | None = None
    # Timers for the measurements.
    user_turn_ended_at: float | None = None
    barge_started_at: float | None = None
    pending_ttfa_from: float | None = None
    # Group mode.
    last_asker: dict | None = None
    last_asker_identity: str | None = None
    # Human transcript lines so far: an agent line before the first one is its greeting.
    human_lines: int = 0
    last_reply_ended_at: float | None = None
    last_trigger_at: float = field(default_factory=time.monotonic)
    last_human_speech_at: float = field(default_factory=time.monotonic)
    window: list[dict] = field(default_factory=list)
    # Last agent transcript line we wrote (skip interrupted + near-duplicate paraphrases).
    speaking_since: float | None = None
    # When the agent last started a line (the ask_agent nudge: did it speak after the tool?).
    last_speech_started_at: float = 0.0
    last_speech_s: float = 0.0
    last_agent_text: str = ""
    last_agent_at: float = 0.0
    # When set, the next agent reply in this window is the auto-answer to a noise transcript — drop it.
    noise_turn_at: float | None = None
    open_work: int = 0
    idle_asked: bool = False
    leaving: bool = False
    stop_reason: str | None = None
    # True while mute_self has detached our audio output (people cannot hear us).
    muted: bool = False
    # leave_call: the model said its own goodbye; finish() must not add the canned one.
    goodbye_said: bool = False


class ArbosVoiceAgent(Agent):
    def __init__(self, call: VoiceCall, instructions: str):
        super().__init__(instructions=instructions, tools=call.make_tools())
        self.call = call


class VoiceCall:
    def __init__(self, ctx: JobContext, cfg: Config, meta: dict, http: aiohttp.ClientSession):
        self.ctx = ctx
        self.cfg = cfg
        self.meta = meta
        self.call_id: str = meta["call_id"]
        self.project_id: str = meta["project_id"]
        self.chat_id: str = meta["chat_id"]
        self.agent_sender: dict = meta.get("agent") or {}
        self.agent_name: str = self.agent_sender.get("name") or "Arbos"
        self.chattiness: str = meta.get("chattiness") or "balanced"
        self.wake_name: str | None = meta.get("wake_name")
        self.voice: str = meta.get("voice") or cfg.voice
        self.vision_enabled: bool = meta.get("vision_enabled", True) is not False
        self.vision: Vision | None = None
        self.updates: CallUpdates | None = None
        self.relay = Relay(cfg, http)
        self.events = EventBatcher(self.relay, self.call_id, self.project_id)
        self.meter = CallMeter(model=cfg.model, stt_model=cfg.stt_model)
        self.judge = Judge(self.relay, self.project_id, cfg.judge_model, cfg.judge_fallback_model)
        self.state = CallState()
        self.info: dict = {}
        self.session: AgentSession | None = None
        self.agent: ArbosVoiceAgent | None = None
        self.listeners: GroupListeners | None = None
        self.rt_model: openai.realtime.RealtimeModel | None = None
        self._done = asyncio.Event()
        self._tasks: set[asyncio.Task] = set()
        self._speak_lock = asyncio.Lock()
        self._pending_tools: set[asyncio.Task] = set()

    # ---- helpers -----------------------------------------------------------------------------------

    def spawn(self, coro) -> asyncio.Task:
        t = asyncio.create_task(coro)
        self._tasks.add(t)
        t.add_done_callback(self._tasks.discard)
        return t

    def humans(self) -> list[rtc.RemoteParticipant]:
        return [p for p in self.ctx.room.remote_participants.values() if p.identity.startswith("u_")]

    def participant(self, identity: str) -> rtc.RemoteParticipant | None:
        return self.ctx.room.remote_participants.get(identity)

    def event(self, kind: str, **fields) -> None:
        self.events.push({"kind": kind, **fields})

    def call_note(self, text: str) -> None:
        """Something that happened on the call (joined, left, muted, told to stop). A call event
        is not a chat message: it is logged and reported to the relay as a `call_note` event
        (admin view), never written into the chat (#1430)."""
        log.info("call: %s", text)
        self.event("call_note", text=text)

    def transcript(self, speaker: dict | None, text: str, *, truncated: bool = False) -> bool:
        """Write a transcript line. Returns False when a duplicate/interrupted agent line is dropped."""
        is_agent = speaker is None or speaker.get("kind") == "agent"
        if is_agent:
            # The greeting (nobody has said anything yet) and the goodbye are call audio, not
            # chat: a call is a call event, the agent did not answer in the chat (#1430).
            if self.state.human_lines == 0 or self.state.leaving:
                log.info("call audio only (not a chat row): %s", text[:120])
                return False
            # A reply cut within its first words is a noise-trigger regenerate (keep only the
            # final line); a longer cut line is a real barge-in and stays in the chat, as the
            # humans heard it (spec §4).
            spoke = (time.monotonic() - self.state.speaking_since) if self.state.speaking_since else self.state.last_speech_s
            if truncated and len(_norm_words(text)) < TRUNCATED_KEEP_WORDS and spoke < TRUNCATED_KEEP_S:
                return False
            now = time.monotonic()
            if self.state.last_agent_text and now - self.state.last_agent_at < 8.0 and similar_reply(
                self.state.last_agent_text, text
            ):
                return False
            self.state.last_agent_text = text
            self.state.last_agent_at = now
        else:
            self.state.human_lines += 1
        self.event("transcript", speaker=speaker if speaker else {"kind": "agent"}, text=text, truncated=truncated)
        log.info("transcript %s: %s%s", "agent" if is_agent else label_of(speaker), text[:160], " [cut]" if truncated else "")
        return True

    def agent_speaking(self) -> bool:
        return self.state.agent_state == "speaking"

    def speak_line(self, line: str, *, allow_interruptions: bool = True):
        """Speak a fixed line. A realtime session has no TTS, so the framework's say() raises;
        the model is told to say the line verbatim instead."""
        assert self.session
        return self.session.generate_reply(
            instructions=f"Say exactly this line and nothing else, in your normal voice: {line}",
            allow_interruptions=allow_interruptions,
        )

    # ---- lifecycle ---------------------------------------------------------------------------------

    async def run(self) -> None:
        ctx = self.ctx
        # Audio for everyone; video only when vision is on (vision.py subscribes per track).
        await ctx.connect(auto_subscribe=AutoSubscribe.AUDIO_ONLY)
        log.info("joined room %s as %s for call %s (project %s)", ctx.room.name, ctx.room.local_participant.identity, self.call_id, self.project_id)
        try:
            self.info = await self.relay.call_info(self.call_id, self.project_id)
        except Exception as e:
            log.error("call info failed: %s", e)
            self.info = {}
        caps = (self.info.get("caps") or {})
        if caps.get("stop"):
            log.warning("cost stop %s before joining; leaving", caps["stop"])
            self.event("left", reason=caps["stop"])
            await self.events.close()
            return
        settings = self.info.get("settings") or {}
        self.chattiness = settings.get("chattiness") or self.chattiness
        self.wake_name = settings.get("wake_name") or self.wake_name
        # The owner's pick for this agent (GET /voices); read at join, used for this call.
        self.voice = settings.get("voice") or self.meta.get("voice") or self.cfg.voice
        if "vision_enabled" in settings:
            self.vision_enabled = settings.get("vision_enabled") is not False
        self.events.start()
        self.event("joined", mode=self.state.mode, version=__version__, sha=WORKER_SHA)

        # Wait for a human if the room is empty at dispatch (the caller is still connecting).
        if not self.humans():
            try:
                await asyncio.wait_for(self._wait_for_human(), timeout=45.0)
            except asyncio.TimeoutError:
                log.info("nobody joined within 45 s; leaving")
                await self.finish("alone")
                return

        self.state.mode = "group" if len(self.humans()) >= 2 else "duplex"
        instructions = self.instructions()
        self.agent = ArbosVoiceAgent(self, instructions)
        self.rt_model = openai.realtime.RealtimeModel(
            model=self.cfg.model,
            voice=self.voice,
            api_key=self.cfg.openai_api_key,
            turn_detection=self.turn_detection_for(self.state.mode),
            input_audio_transcription=AudioTranscription(model=self.cfg.stt_model),
            # Filter ambient / room noise before VAD so background sound does not become a
            # user turn (and a chat line the agent answers).
            input_audio_noise_reduction=DUPLEX_NOISE_REDUCTION if self.state.mode == "duplex" else None,
        )
        self.session = AgentSession(
            llm=self.rt_model,
            allow_interruptions=True,
            turn_detection="realtime_llm" if self.state.mode == "duplex" else "manual",
            user_away_timeout=None,
        )
        self.hook_session(self.session)
        self.hook_room()
        linked = self.humans()[0].identity if self.state.mode == "duplex" else None
        self.state.linked_identity = linked
        input_opts: dict[str, Any] = {
            "audio_enabled": self.state.mode == "duplex",
            "text_enabled": False,
            "close_on_disconnect": False,
        }
        if linked:
            input_opts["participant_identity"] = linked
        await self.session.start(
            self.agent,
            room=ctx.room,
            room_input_options=RoomInputOptions(**input_opts),
            room_output_options=RoomOutputOptions(transcription_enabled=True, audio_enabled=True),
        )
        if self.state.mode == "group":
            await self.start_group_listening()
        self.hook_realtime()
        self.start_vision()
        self.start_updates()
        log.info("session started in %s mode (humans=%d, chattiness=%s, voice=%s, turn_detection=%s, vision=%s)", self.state.mode, len(self.humans()), self.chattiness, self.voice, self.cfg.turn_detection, "on" if self.vision_enabled else "off")
        self.spawn(self.usage_loop())
        self.spawn(self.idle_loop())
        self.spawn(self.settings_loop())
        await self.greet()
        await self._done.wait()

    async def _wait_for_human(self) -> None:
        while not self.humans():
            await asyncio.sleep(0.2)

    def instructions(self) -> str:
        ctxpack = (self.info.get("context") or {}) if isinstance(self.info.get("context"), dict) else {}
        extra = ctxpack.get("system") or ""
        if not extra:
            p = self.info.get("project") or {}
            bits = []
            if p.get("description"):
                bits.append(p["description"])
            if p.get("instructions"):
                bits.append("Owner instructions:\n" + str(p["instructions"])[:4000])
            extra = "\n\n".join(bits)
        recent = ctxpack.get("recent") or []
        if recent:
            extra += "\n\n# Last lines of this chat\n" + "\n".join(f"- {r.get('from')}: {r.get('text')}" for r in recent[-12:])
            self.state.window = [{"from": r.get("from"), "text": r.get("text")} for r in recent[-8:]]
        if self.info and not self.info.get("box_online", True):
            extra += (
                "\n\n# Tools on this call\nYour computer is not linked to this call yet, so ask_agent will fail. "
                f"When someone asks you to do or look up something, say \"{NO_LINK_LINE}\" and that you can do it in the text chat. "
                "Answer everything else from what you know."
            )
        if not self.vision_enabled:
            extra += (
                "\n\n# Eyes on this call\nThe owner turned camera vision off for you: you get no camera stills and look() "
                "returns nothing. If someone asks you to look at something, say vision is off for this agent."
            )
        call = self.info.get("call") or {}
        chat = self.info.get("chat") or {}
        agents = self.info.get("agents") or call.get("agents") or []
        model = (
            self.info.get("model")
            or (self.info.get("project") or {}).get("model")
            or ctxpack.get("talking_model")
            or self.cfg.model
        )
        people = self.people_on_call()
        return system_prompt(
            name=self.agent_name,
            group=self.state.mode == "group",
            extra=extra,
            people=people,
            platform=call.get("platform"),
            agents=agents if isinstance(agents, list) else [],
            self_id=self.project_id,
            model=str(model) if model else None,
            chat_title=(chat.get("title") or chat.get("name") or "").strip() or None,
            chat_kind=(chat.get("kind") or "").strip() or None,
        )

    def people_on_call(self) -> list[dict]:
        """Humans for the prompt: call_info rows, with LiveKit platform when the join token carried it."""
        by_id: dict[str, dict] = {}
        for raw in self.info.get("humans") or []:
            if not isinstance(raw, dict):
                continue
            entry = dict(raw)
            pid = str(entry.get("id") or "")
            if pid:
                by_id[pid] = entry
        for p in self.humans():
            sp = speaker_of(p, p.identity)
            pid = str(sp.get("id") or "")
            if not pid:
                continue
            entry = by_id.get(pid) or dict(sp)
            if sp.get("platform"):
                entry["platform"] = sp["platform"]
                entry["calling_from"] = sp["platform"]
            elif entry.get("calling_from") or entry.get("platform"):
                pass
            by_id[pid] = entry
        return list(by_id.values()) or list(self.info.get("humans") or [])

    def turn_detection_for(self, mode: str):
        if mode != "duplex":
            return None
        kind, _, arg = self.cfg.turn_detection.partition(":")
        if kind == "semantic_vad":
            eagerness = arg if arg in ("low", "medium", "high", "auto") else "auto"
            return SemanticVad(type="semantic_vad", eagerness=eagerness, create_response=True, interrupt_response=True)
        try:
            silence_ms = int(arg) if arg else 300
        except ValueError:
            silence_ms = 300
        return ServerVad(
            type="server_vad",
            # Above the OpenAI default (0.5) and the group Silero barge gate (0.6): ambient
            # noise / speaker echo must be louder before they cut the agent mid-sentence.
            threshold=DUPLEX_VAD_THRESHOLD,
            prefix_padding_ms=200,
            silence_duration_ms=silence_ms,
            create_response=True,
            interrupt_response=True,
        )

    async def greet(self) -> None:
        assert self.session
        humans = self.info.get("humans") or []
        owner = next((h for h in humans if h.get("role") == "owner"), None)
        owner_label = label_of(owner) if owner else "the owner"
        if self.state.mode == "group":
            line = f"{self.agent_name}, {owner_label}'s agent, has joined and can hear this call."
            self.call_note(line)
            await self.wait_for_silence()
            self.speak_line(line)
        else:
            # One fixed greeting line (no free-form generate: that raced the VAD and produced a
            # paraphrase after any noise cut).
            self.call_note(f"{self.agent_name} joined the call")
            self.speak_line(f"Hey — {self.agent_name} here. What do you need?")

    async def finish(self, reason: str) -> None:
        if self.state.leaving:
            return
        self.state.leaving = True
        self.state.stop_reason = reason
        self.meter.stop_reason = reason
        log.info("leaving call %s: %s", self.call_id, reason)
        try:
            if self.session and reason != "alone" and GOODBYE.get(reason):
                own = self.agent_speaking() or self.state.goodbye_said
                if not own and reason == "asked" and self.state.mode == "duplex":
                    # Kill word in 1:1: the model answers that same utterance itself (server VAD)
                    # and usually says goodbye; give it a moment before adding our own line.
                    deadline = time.monotonic() + 2.5
                    while not self.agent_speaking() and time.monotonic() < deadline:
                        await asyncio.sleep(0.05)
                    own = self.agent_speaking()
                if own:
                    # The model is saying its own goodbye (kill word or leave_call): a second
                    # response.create would cancel it and be refused; let it finish.
                    deadline = time.monotonic() + 8.0
                    while self.agent_speaking() and time.monotonic() < deadline:
                        await asyncio.sleep(0.05)
                else:
                    handle = self.speak_line(GOODBYE[reason], allow_interruptions=False)
                    try:
                        await asyncio.wait_for(handle.wait_for_playout(), timeout=12.0)
                    except asyncio.TimeoutError:
                        pass
                # Whatever follows (a leave_call tool result, a second "Goodbye") is not heard.
                try:
                    self.session.output.set_audio_enabled(False)
                except Exception:
                    pass
        except Exception as e:
            log.warning("goodbye failed: %s", e)
        self.call_note(f"{self.agent_name} left the call" + ("" if reason in ("alone", "asked", "idle") else f" · {reason.replace('_', ' ')}"))
        self.event("left", reason=reason)
        if self.listeners:
            await self.listeners.stop()
        if self.vision:
            self.sync_vision_meter()
            await self.vision.stop()
        if self.updates:
            await self.updates.stop()
        try:
            await self.relay.usage(self.call_id, self.project_id, self.meter.usage(ended=True))
        except Exception as e:
            log.warning("final usage post failed: %s", e)
        await self.events.close()
        log.info("call %s summary: %s", self.call_id, json.dumps({**self.meter.summary(), "vision": self.vision.summary() if self.vision else None}))
        if self.session:
            try:
                await asyncio.wait_for(self.session.aclose(), timeout=5.0)
            except Exception:
                pass
        for t in list(self._tasks):
            t.cancel()
        self._done.set()
        self.ctx.shutdown(reason=reason)

    # ---- session events ------------------------------------------------------------------------------

    def hook_session(self, session: AgentSession) -> None:
        @session.on("agent_state_changed")
        def _agent_state(ev: AgentStateChangedEvent) -> None:
            now = time.monotonic()
            prev = self.state.agent_state
            self.state.agent_state = ev.new_state
            if ev.new_state == "speaking":
                self.state.speaking_since = now
                self.state.last_speech_started_at = now
                src = self.state.pending_ttfa_from
                if src is not None:
                    ttfa = int((now - src) * 1000)
                    self.meter.ttfa_ms.append(ttfa)
                    self.state.pending_ttfa_from = None
                    self.event("metric", ttfa_ms=ttfa, mode=self.state.mode)
                    log.info("TTFA %d ms (%s)", ttfa, self.state.mode)
            if prev == "speaking" and ev.new_state != "speaking":
                self.state.last_reply_ended_at = now
                self.state.last_speech_s = now - (self.state.speaking_since or now)
                self.state.speaking_since = None
                if self.state.barge_started_at is not None:
                    cut = int((now - self.state.barge_started_at) * 1000)
                    self.meter.barge_ms.append(cut)
                    self.state.barge_started_at = None
                    self.event("metric", barge_ms=cut, mode=self.state.mode)
                    log.info("barge-in cut %d ms (%s)", cut, self.state.mode)

        @session.on("user_state_changed")
        def _user_state(ev: UserStateChangedEvent) -> None:
            now = time.monotonic()
            self.state.user_state = ev.new_state
            if self.state.mode != "duplex":
                return
            if ev.new_state == "speaking":
                self.state.last_human_speech_at = now
                self.state.idle_asked = False
                if self.agent_speaking() and self.state.barge_started_at is None:
                    self.state.barge_started_at = now
            elif ev.old_state == "speaking" and ev.new_state == "listening":
                self.state.user_turn_ended_at = now
                self.state.pending_ttfa_from = now

        @session.on("user_input_transcribed")
        def _user_transcribed(ev: UserInputTranscribedEvent) -> None:
            if self.state.mode != "duplex" or not ev.is_final:
                return
            text = (ev.transcript or "").strip()
            if not text:
                return
            if is_noise_transcript(text):
                # Ambient noise became a fake user turn (create_response=True). Do not write it
                # to the chat, cut any auto-reply, and drop that reply from the transcript.
                log.info("dropping noise transcript %r", text)
                self.state.noise_turn_at = time.monotonic()
                if self.session:
                    try:
                        self.session.interrupt()
                    except Exception as e:
                        log.warning("interrupt after noise transcript failed: %s", e)
                return
            self.state.noise_turn_at = None
            identity = self.state.linked_identity or ""
            speaker = speaker_of(self.participant(identity), identity)
            self.transcript(speaker, text)
            self.state.last_asker = speaker
            self.state.last_asker_identity = identity
            if self.vision and wants_a_look(text):
                self.vision.trigger(identity)
            self.state.window.append({"from": label_of(speaker), "text": text})
            self.state.window = self.state.window[-12:]
            fd = forced_decision(text, self.judge_ctx())
            if fd is not None and fd.action in ("stop", "leave"):
                self.spawn(self.kill_word(fd.action))

        @session.on("conversation_item_added")
        def _item_added(ev: ConversationItemAddedEvent) -> None:
            item = ev.item
            if getattr(item, "role", None) != "assistant":
                return
            text = (item.text_content or "").strip()
            if not text:
                return
            # Auto-reply to a dropped noise transcript — never show it in the chat.
            if self.state.noise_turn_at is not None and time.monotonic() - self.state.noise_turn_at < 4.0:
                self.state.noise_turn_at = None
                log.info("dropping agent reply to noise transcript")
                if self.session and self.agent_speaking():
                    try:
                        self.session.interrupt()
                    except Exception:
                        pass
                return
            wrote = self.transcript(None, text, truncated=bool(getattr(item, "interrupted", False)))
            if wrote:
                self.state.window.append({"from": f"{self.agent_name} (agent)", "text": text})
                self.state.window = self.state.window[-12:]

        @session.on("speech_created")
        def _speech(ev: SpeechCreatedEvent) -> None:
            self.meter.turns_spoken += 1

        @session.on("metrics_collected")
        def _metrics(ev: MetricsCollectedEvent) -> None:
            m = ev.metrics
            if isinstance(m, RealtimeModelMetrics):
                self.meter.add_realtime(
                    input_tokens=m.input_tokens,
                    output_tokens=m.output_tokens,
                    details_in=m.input_token_details,
                    details_out=m.output_token_details,
                    ttft_s=m.ttft,
                )

        @session.on("error")
        def _error(ev) -> None:
            log.warning("session error: %s", getattr(ev, "error", ev))

    def hook_realtime(self) -> None:
        """Wall-clock stamps of the speech model's own VAD, for the barge-in measurement."""
        try:
            rt = self.agent.realtime_llm_session if self.agent else None
        except Exception:
            rt = None
        if rt is None:
            return

        @rt.on("input_speech_started")
        def _started(_ev) -> None:
            log.info("rt input_speech_started wall=%d agent_state=%s", now_ms(), self.state.agent_state)

        @rt.on("input_speech_stopped")
        def _stopped(_ev) -> None:
            log.info("rt input_speech_stopped wall=%d", now_ms())

    def hook_room(self) -> None:
        room = self.ctx.room

        @room.on("participant_connected")
        def _joined(p: rtc.RemoteParticipant) -> None:
            if p.identity.startswith("u_"):
                log.info("human joined: %s", p.identity)
                self.spawn(self.on_humans_changed())

        @room.on("participant_disconnected")
        def _left(p: rtc.RemoteParticipant) -> None:
            if p.identity.startswith("u_"):
                log.info("human left: %s", p.identity)
                if self.listeners:
                    self.spawn(self.listeners.detach(p.identity))
                self.spawn(self.on_humans_changed())

        @room.on("track_subscribed")
        def _track(track: rtc.Track, pub: rtc.RemoteTrackPublication, p: rtc.RemoteParticipant) -> None:
            if pub.kind == rtc.TrackKind.KIND_VIDEO:
                if self.vision:
                    self.vision.attach(track, pub, p)
                return
            if self.listeners and self.state.mode == "group":
                self.listeners.attach(p, track)

        @room.on("track_published")
        def _published(pub: rtc.RemoteTrackPublication, p: rtc.RemoteParticipant) -> None:
            if self.vision:
                self.vision.consider(pub, p)

        @room.on("track_unsubscribed")
        def _unsubscribed(track: rtc.Track, pub: rtc.RemoteTrackPublication, p: rtc.RemoteParticipant) -> None:
            if self.vision and pub.kind == rtc.TrackKind.KIND_VIDEO:
                self.spawn(self.vision.detach(pub.sid))

        @room.on("track_unpublished")
        def _unpublished(pub: rtc.RemoteTrackPublication, p: rtc.RemoteParticipant) -> None:
            if self.vision and pub.kind == rtc.TrackKind.KIND_VIDEO:
                self.spawn(self.vision.detach(pub.sid))

        @room.on("track_muted")
        def _muted(p: rtc.Participant, pub: rtc.TrackPublication) -> None:
            if self.vision and pub.kind == rtc.TrackKind.KIND_VIDEO and isinstance(p, rtc.RemoteParticipant):
                self.vision.muted(pub, p)

        @room.on("track_unmuted")
        def _unmuted(p: rtc.Participant, pub: rtc.TrackPublication) -> None:
            if self.vision and pub.kind == rtc.TrackKind.KIND_VIDEO and isinstance(p, rtc.RemoteParticipant):
                self.vision.unmuted(pub, p)

        @room.on("disconnected")
        def _disconnected(*_a) -> None:
            log.info("room disconnected")
            self.spawn(self.finish(self.state.stop_reason or "room_closed"))

    async def on_humans_changed(self) -> None:
        await asyncio.sleep(0.3)
        humans = self.humans()
        if not humans:
            await asyncio.sleep(10.0)
            if not self.humans() and not self.state.leaving:
                await self.finish("alone")
            return
        want = "group" if len(humans) >= 2 else "duplex"
        if want != self.state.mode:
            await self.switch_mode(want)
        elif want == "duplex" and self.state.linked_identity != humans[0].identity and self.session:
            self.state.linked_identity = humans[0].identity
            self.session.room_io.set_participant(humans[0].identity)

    async def switch_mode(self, mode: str) -> None:
        assert self.session and self.agent and self.rt_model
        log.info("mode %s → %s", self.state.mode, mode)
        self.state.mode = mode
        self.meter.mode = mode if self.meter.mode == mode else "mixed"
        rt = self.agent.realtime_llm_session
        if mode == "group":
            self.session.input.set_audio_enabled(False)
            if rt:
                rt.update_options(turn_detection=None)
            self.session.update_options(turn_detection="manual")
            self.state.linked_identity = None
            await self.start_group_listening()
            await self.agent.update_instructions(self.instructions())
            line = f"{self.agent_name} can hear everyone on this call now."
            self.call_note(line)
        else:
            if self.listeners:
                await self.listeners.stop()
                self.listeners = None
            human = self.humans()[0]
            self.state.linked_identity = human.identity
            self.session.room_io.set_participant(human.identity)
            if rt:
                rt.update_options(turn_detection=self.turn_detection_for("duplex"))
            self.session.update_options(turn_detection="realtime_llm")
            self.session.input.set_audio_enabled(True)
            await self.agent.update_instructions(self.instructions())
        self.event("metric", mode_switch=mode)

    # ---- group mode ---------------------------------------------------------------------------------

    async def start_group_listening(self) -> None:
        if self.listeners:
            return
        vad = self.ctx.proc.userdata.get("group_vad") or silero.VAD.load(
            min_speech_duration=GROUP_BARGE_MIN_SPEECH_S, activation_threshold=0.6, min_silence_duration=self.cfg.group_end_silence_s
        )
        stt = openai.STT(model=self.cfg.stt_model, api_key=self.cfg.openai_api_key, use_realtime=False)
        self.listeners = GroupListeners(
            self.ctx.room,
            vad,
            stt,
            on_speech_start=self.on_group_speech_start,
            on_speech_end=self.on_group_speech_end,
            on_utterance=self.on_group_utterance,
            on_stt_seconds=self.meter.add_stt,
        )
        await self.listeners.start()

    def on_group_speech_start(self, identity: str, at: float) -> None:
        self.state.last_human_speech_at = at
        self.state.idle_asked = False
        if self.agent_speaking() and self.session:
            # Only sustained speech interrupts (the listener's VAD waits GROUP_BARGE_MIN_SPEECH_S
            # before reporting a start), so a cough, a door or the agent's own echo on someone's
            # speaker does not cut it off mid-sentence.
            if self.state.barge_started_at is None:
                self.state.barge_started_at = at
            log.info("barge-in by %s", identity)
            self.session.interrupt()

    def on_group_speech_end(self, identity: str, at: float) -> None:
        pass

    def judge_ctx(self) -> JudgeContext:
        last = self.state.last_reply_ended_at
        return JudgeContext(
            agent_name=self.agent_name,
            agent_role=(self.info.get("project") or {}).get("description") or "",
            chat_kind=(self.info.get("chat") or {}).get("kind") or "group",
            wake_name=self.wake_name,
            chattiness=self.chattiness,
            recent=self.state.window[-10:],
            agent_last_spoke_s_ago=(time.monotonic() - last) if last else None,
            agent_has_open_work=self.state.open_work > 0,
        )

    async def on_group_utterance(self, u: Utterance) -> None:
        speaker = speaker_of(self.participant(u.identity), u.identity)
        label = label_of(speaker)
        self.transcript(speaker, u.text)
        if self.vision and wants_a_look(u.text):
            self.vision.trigger(u.identity)
        ctx = self.judge_ctx()
        decision = await self.judge.decide(u.text, label, ctx)
        self.meter.add_decision(decision.cost_usd)
        # Attention window: the person the agent just answered gets the benefit of the doubt.
        in_window = (
            self.state.last_asker is not None
            and self.state.last_asker.get("id") == speaker.get("id")
            and self.state.last_reply_ended_at is not None
            and time.monotonic() - self.state.last_reply_ended_at <= ATTENTION_WINDOW_S
        )
        if decision.action == "silent" and in_window and decision.p_silent is not None and not decision.forced:
            thr = CHATTINESS_THRESHOLD.get(self.chattiness, 0.6) - 0.2
            if 1.0 - decision.p_silent >= thr:
                decision.action = "act" if (decision.p_act or 0) > (decision.p_speak or 0) else "speak"
                decision.reason = "attention window; " + decision.reason
        self.event("decision", **{k: v for k, v in decision.as_event(label, u.text).items() if k != "kind"})
        log.info(
            "judge %s → %s (speak=%.2f act=%.2f silent=%.2f, %s, %d ms) «%s»",
            label, decision.action, decision.p_speak or 0, decision.p_act or 0, decision.p_silent or 0,
            decision.provider, decision.latency_ms, u.text[:80],
        )
        self.state.window.append({"from": label, "text": u.text})
        self.state.window = self.state.window[-12:]
        if decision.action in ("stop", "leave"):
            await self.kill_word(decision.action)
            return
        if decision.action == "silent":
            return
        self.state.last_asker = speaker
        self.state.last_asker_identity = u.identity
        self.state.last_trigger_at = time.monotonic()
        await self.respond_group(u, speaker, decision)

    async def wait_for_silence(self, max_wait: float = 15.0) -> None:
        """Turn-taking: never start while a human is talking; wait for `group_silence_s` of quiet."""
        if not self.listeners:
            return
        deadline = time.monotonic() + max_wait
        while time.monotonic() < deadline:
            quiet_for = time.monotonic() - self.state.last_human_speech_at
            if not self.listeners.anyone_speaking() and quiet_for >= self.cfg.group_silence_s:
                return
            await asyncio.sleep(0.05)

    async def respond_group(self, u: Utterance, speaker: dict, decision: Decision) -> None:
        assert self.session
        async with self._speak_lock:
            await self.wait_for_silence()
            label = label_of(speaker)
            lines = "\n".join(f"{w['from']}: {w['text']}" for w in self.state.window[-8:-1])
            user_input = (f"Recent lines on the call:\n{lines}\n\n" if lines else "") + f"{label} just said: {u.text}"
            cam_on = bool(self.vision and any(c.identity == u.identity for c in self.vision.live_cameras()))
            if decision.action == "act":
                instr = (
                    f"{label} asked for work that needs your tools. Say one very short line like 'On it' addressed to {label}, "
                    f"then call ask_agent with their request (as they asked it, including who asked). Keep it under ten words before the tool."
                )
                if cam_on:
                    instr = (
                        f"{label} asked something while their camera is on. If it is about what they are showing (an object, a label, "
                        f"a screen, printed text), say a very short line like 'Let me look', then call look(what) and answer from the picture; "
                        f"add hand_to_agent=true only when they want something done with it (search, save, compare, note). "
                        f"Otherwise say 'On it' and call ask_agent with their request (as they asked it, including who asked)."
                    )
            else:
                instr = f"Answer {label} out loud in one or two short sentences. If the answer needs the computer, files or recent data, use ask_agent instead."
                if cam_on:
                    instr += f" {label}'s camera is on: if the question is about what they show, answer from their latest camera still, or call look(what) first for small print."
            self.state.pending_ttfa_from = u.ended_at
            self.session.generate_reply(user_input=user_input, instructions=instr)

    async def kill_word(self, action: str) -> None:
        assert self.session
        if action == "stop":
            log.info("kill word: stop")
            self.session.interrupt()
            for t in list(self._pending_tools):
                t.cancel()
            self.call_note(f"{self.agent_name} was told to stop")
        else:
            await self.finish("asked")

    # ---- tool ----------------------------------------------------------------------------------------

    def make_tools(self):
        call = self

        @function_tool(name="ask_agent", description=ASK_AGENT_DESCRIPTION)
        async def ask_agent(context: RunContext, request: str) -> str:
            return await call.ask_agent(request)

        @function_tool(name="look", description=LOOK_DESCRIPTION)
        async def look(context: RunContext, what: str, hand_to_agent: bool = False, participant: str = "") -> str:
            return await call.look(what, hand_to_agent=hand_to_agent, participant=participant)

        @function_tool(name="mute_self", description=MUTE_SELF_DESCRIPTION)
        async def mute_self(context: RunContext) -> str:
            return await call.set_muted(True)

        @function_tool(name="unmute_self", description=UNMUTE_SELF_DESCRIPTION)
        async def unmute_self(context: RunContext) -> str:
            return await call.set_muted(False)

        @function_tool(name="leave_call", description=LEAVE_CALL_DESCRIPTION)
        async def leave_call(context: RunContext) -> str:
            return await call.leave_by_tool()

        return [ask_agent, look, mute_self, unmute_self, leave_call]

    async def set_muted(self, muted: bool) -> str:
        """Detach or re-attach our audio output so the call cannot / can hear us."""
        if self.state.muted == muted:
            return "Already muted." if muted else "Already unmuted."
        self.state.muted = muted
        if muted and self.session:
            try:
                self.session.interrupt()
            except Exception as e:
                log.warning("interrupt on mute failed: %s", e)
        if self.session:
            try:
                self.session.output.set_audio_enabled(not muted)
            except Exception as e:
                log.warning("set_audio_enabled(%s) failed: %s", not muted, e)
                return (
                    "Could not change mute on the audio path; say so briefly."
                    if muted
                    else "Could not unmute on the audio path; say so briefly."
                )
        self.call_note(f"{self.agent_name} muted" if muted else f"{self.agent_name} unmuted")
        log.info("agent %s", "muted" if muted else "unmuted")
        return "Muted. Stay silent until unmute_self." if muted else "Unmuted. You can speak again."

    async def leave_by_tool(self) -> str:
        """Dismiss ourselves from the call after the model has said goodbye."""
        if self.state.leaving:
            return "Already leaving."
        # The model usually says its goodbye before calling; when it calls first, finish() says it.
        last = self.state.last_reply_ended_at
        self.state.goodbye_said = self.agent_speaking() or (last is not None and time.monotonic() - last < 4.0)
        self.spawn(self.finish("asked"))
        return "You are leaving the call now. Do not say anything else."

    # ---- the chat, live, for the call (#1832) ---------------------------------------------------------

    def room_quiet(self) -> bool:
        """Nobody is talking and the agent is idle: a pause the agent may use."""
        if self.agent_speaking():
            return False
        if self.state.mode == "duplex":
            return self.state.user_state != "speaking"
        return not (self.listeners and self.listeners.anyone_speaking()) and (
            time.monotonic() - self.state.last_human_speech_at >= self.cfg.group_silence_s
        )

    def add_context(self, text: str) -> None:
        """A note into the model's conversation, no reply asked for."""
        rt = None
        try:
            rt = self.agent.realtime_llm_session if self.agent else None
        except Exception:
            rt = None
        if rt is None:
            return
        rt.send_event(context_item(text))

    async def say(self, instructions: str) -> None:
        """One reply from the voice agent, played out. Never interrupts: callers wait for a pause."""
        if self.state.leaving or not self.session or self.state.muted:
            return
        async with self._speak_lock:
            if self.state.leaving or not self.session or self.state.muted:
                return
            handle = self.session.generate_reply(instructions=instructions)
            try:
                await asyncio.wait_for(handle.wait_for_playout(), timeout=45.0)
            except asyncio.TimeoutError:
                pass

    def start_updates(self) -> None:
        self.updates = CallUpdates(
            agent_name=self.agent_name,
            fetch=lambda after, wait: self.relay.inbox(self.call_id, self.project_id, after, wait),
            add_context=self.add_context,
            speak=self.say,
            quiet=self.room_quiet,
            leaving=lambda: self.state.leaving,
            on_update=lambda u: self.event("update", kind=u.kind, message_id=u.message_id, chars=len(u.text)),
        )
        self.updates.start()

    # ---- vision ------------------------------------------------------------------------------------

    def start_vision(self) -> None:
        rt = None
        try:
            rt = self.agent.realtime_llm_session if self.agent else None
        except Exception:
            pass
        self.vision = Vision(
            self.ctx.room,
            self.cfg,
            agent_identity=self.ctx.room.local_participant.identity,
            agent_name=self.agent_name,
            label_of=lambda identity: label_of(speaker_of(self.participant(identity), identity)),
            enabled=self.vision_enabled,
        )
        self.vision.rt = rt
        # Private on purpose: the read-only view cannot drop pruned placeholders (vision.py).
        self.vision.local_ctx = lambda: getattr(self.agent, "_chat_ctx", None)
        self.vision.on_first_frame = lambda identity: self.event("vision", identity=identity, seeing=True)
        self.vision.start()

    def sync_vision_meter(self) -> None:
        if not self.vision:
            return
        self.meter.frames = self.vision.frames_sent
        self.meter.looks = self.vision.looks
        self.meter.vision_seconds = self.vision.camera_seconds()

    def identity_for(self, participant: str) -> str | None:
        """`participant` as the model names it (handle, name, or identity) → LiveKit identity."""
        want = participant.strip().lstrip("@").lower()
        if not want:
            return self.state.last_asker_identity or self.state.linked_identity
        for p in self.humans():
            sp = speaker_of(p, p.identity)
            if want in {p.identity.lower(), str(sp.get("handle", "")).lower(), str(sp.get("name", "")).lower()}:
                return p.identity
        return self.state.last_asker_identity or self.state.linked_identity

    def blob_slot(self) -> str:
        chat = self.info.get("chat") or {}
        pid = chat.get("project_id")
        return pid if pid and pid != "people" else f"people:{self.chat_id}"

    async def look(self, what: str, *, hand_to_agent: bool = False, participant: str = "") -> str:
        if not self.vision or not self.vision_enabled:
            return "Vision is off for this agent, so you cannot look. Say so briefly."
        identity = self.identity_for(participant)
        got = await self.vision.look(identity)
        if got is None:
            return "No camera is on right now. Ask them to turn their camera on, then look again."
        jpeg, seen_identity = got
        label = label_of(speaker_of(self.participant(seen_identity), seen_identity))
        self.event("tool", name="look", status="done", request=what[:300], speaker=label, kb=len(jpeg) // 1024, hand_to_agent=hand_to_agent)
        log.info("look(%r) from %s: %d KB%s", what[:60], label, len(jpeg) // 1024, " → agent" if hand_to_agent else "")
        if not hand_to_agent:
            return f"A full-resolution frame of {label}'s camera is now in front of you (the last image). Answer about: {what}"
        key = hashlib.sha256(jpeg).hexdigest()
        try:
            await self.relay.put_blob(self.blob_slot(), key, jpeg, "image/jpeg")
        except Exception as e:
            log.warning("blob upload failed: %s", e)
            return f"The full-resolution frame is in front of you, but it could not be sent to your agent ({e}). Answer about: {what}"
        attachment = {"name": f"camera-{time.strftime('%H%M%S', time.gmtime())}.jpg", "mime": "image/jpeg", "ref": key}
        request = f"{what.strip()} (A photo from {label}'s camera on the call is attached.)"
        return await self.ask_agent(request, attachments=[attachment], tool_name="look")

    async def ask_agent(self, request: str, *, attachments: list[dict] | None = None, tool_name: str = "ask_agent") -> str:
        speaker = self.state.last_asker or (
            speaker_of(self.participant(self.state.linked_identity or ""), self.state.linked_identity or "") if self.state.linked_identity else {}
        )
        self.meter.tool_calls += 1
        self.state.open_work += 1
        request_id = uuid.uuid4().hex[:12]
        self.event("tool", name=tool_name, status="started", request=request[:500], request_id=request_id, speaker=label_of(speaker) if speaker else None)
        log.info("%s[%s] from %s: %s", tool_name, request_id, label_of(speaker) if speaker else "?", request[:120])
        t0 = time.monotonic()

        async def run() -> dict:
            return await self.relay.tool(
                self.call_id, self.project_id, speaker=speaker, request=request, mode=self.state.mode, request_id=request_id,
                attachments=attachments,
            )

        task = asyncio.create_task(run())
        self._pending_tools.add(task)
        task.add_done_callback(self._pending_tools.discard)
        try:
            result = await asyncio.wait_for(asyncio.shield(task), timeout=self.cfg.tool_quick_wait_s)
        except asyncio.TimeoutError:
            self.spawn(self.deliver_later(task, request_id, speaker))
            return "The agent is working on it; this may take a while. Tell the person you will report back when it is done, then stop talking."
        except asyncio.CancelledError:
            self.state.open_work -= 1
            return "Cancelled."
        except ToolPending:
            self.state.open_work -= 1
            # The relay's wait ran out; the answer lands in the chat later and reaches this
            # call through the inbox. Say so.
            return "The agent is still working on it; the answer will show up in the chat (and you will hear of it). Tell the person that in one short sentence."
        except RelayError as e:
            self.state.open_work -= 1
            self.event("tool", name="ask_agent", status="failed", request_id=request_id, error=str(e.body)[:300], ms=int((time.monotonic() - t0) * 1000))
            if e.status == 503:
                return NO_LINK_LINE + " Say exactly that, briefly, and offer to do it in the text chat."
            if e.status == 402:
                return "Spending is capped right now, so I can't run that. Say so briefly."
            return f"The request failed: {str(e.body)[:200]}. Say so briefly."
        except Exception as e:
            self.state.open_work -= 1
            return f"The request failed: {e}. Say so briefly."
        self.state.open_work -= 1
        ms = int((time.monotonic() - t0) * 1000)
        self.event("tool", name="ask_agent", status="done" if result.get("ok") else "failed", request_id=request_id, ms=ms, message_id=result.get("message_id"))
        if not result.get("ok"):
            return f"The agent could not do it: {result.get('error') or 'unknown error'}. Say so briefly."
        text = (result.get("text") or "").strip()
        log.info("ask_agent[%s] done in %d ms: %s", request_id, ms, text[:120])
        # The same bubble also arrives through the inbox: seen once.
        if self.updates:
            self.updates.mark_seen(str(result.get("message_id") or ""))
        # The voice agent speaks the result itself (Jacob's rule: one speaker). A realtime
        # model sometimes stays silent after a slow tool; `nudge_after_tool` asks it once.
        label = label_of(speaker) if speaker else "them"
        self.spawn(self.nudge_after_tool(label))
        return (
            f"Result from your agent (the full text is already in the chat). Tell {label} what matters "
            f"in one or two spoken sentences now — no tables, links or phone numbers:\n{text[:3000]}"
        )

    async def nudge_after_tool(self, label: str) -> None:
        """If the agent said nothing within a few seconds of a tool result, ask it to."""
        since = time.monotonic()
        await asyncio.sleep(3.0)
        if self.state.leaving or self.state.last_speech_started_at >= since or self.agent_speaking():
            return
        if self.state.mode == "duplex" and self.state.user_state == "speaking":
            return
        log.info("agent silent after the tool result; nudging")
        await self.say(f"Tell {label} the result of the tool you just ran, in one or two short spoken sentences.")

    async def deliver_later(self, task: asyncio.Task, request_id: str, speaker: dict) -> None:
        """ask_agent outlasted the quick wait: the model already said it would report back. When
        the box finishes, the answer joins the call like any chat update — into the context,
        then a short spoken summary at the next pause (the inbox may bring the same bubble;
        it is seen once)."""
        agent = {"kind": "agent", "name": self.agent_name, "handle": self.agent_sender.get("handle") or ""}
        try:
            result = await task
        except ToolPending:
            self.state.open_work -= 1
            log.warning("deferred tool %s still pending after relay wait; the inbox carries the answer", request_id)
            if self.updates:
                await self.updates.handle(
                    Update(
                        kind="quick",
                        message_id=f"pending:{request_id}",
                        text="The task from this call is still running on the computer; its answer will show up in the chat when it is ready.",
                        sender=agent,
                    )
                )
            return
        except Exception as e:
            self.state.open_work -= 1
            log.warning("deferred tool %s failed: %s", request_id, e)
            if self.updates:
                await self.updates.handle(
                    Update(kind="result", message_id=f"failed:{request_id}", text=f"The task from this call failed: {e}", sender=agent)
                )
            return
        self.state.open_work -= 1
        text = (result.get("text") or "").strip() if result.get("ok") else f"it failed: {result.get('error')}"
        message_id = str(result.get("message_id") or f"deferred:{request_id}")
        self.event("tool", name="ask_agent", status="done" if result.get("ok") else "failed", request_id=request_id, message_id=message_id, deferred=True)
        log.info("deferred tool %s done; into the call's context", request_id)
        if self.updates:
            await self.updates.handle(Update(kind="result", message_id=message_id, text=text, sender=agent))

    # ---- loops ----------------------------------------------------------------------------------------

    async def usage_loop(self) -> None:
        while not self.state.leaving:
            await asyncio.sleep(self.cfg.usage_every_s)
            if self.state.leaving:
                return
            self.sync_vision_meter()
            try:
                resp = await self.relay.usage(self.call_id, self.project_id, self.meter.usage())
            except Exception as e:
                log.warning("usage post failed: %s", e)
                continue
            stop = (resp.get("caps") or {}).get("stop")
            if stop:
                await self.finish(stop)
                return
            if self.meter.seconds() >= self.cfg.call_max_s:
                await self.finish("call_max")
                return

    async def settings_loop(self) -> None:
        """Owner toggles (vision on/off, chattiness) apply during the call: re-read every few seconds."""
        while not self.state.leaving:
            await asyncio.sleep(self.cfg.settings_poll_s)
            if self.state.leaving:
                return
            try:
                s = await self.relay.agent_settings(self.project_id)
            except Exception as e:
                log.debug("settings re-read failed: %s", e)
                continue
            enabled = s.get("vision_enabled", True) is not False
            if enabled != self.vision_enabled:
                self.vision_enabled = enabled
                log.info("owner turned vision %s", "on" if enabled else "off")
                if self.vision:
                    self.vision.set_enabled(enabled)
                if self.agent:
                    try:
                        await self.agent.update_instructions(self.instructions())
                    except Exception as e:
                        log.warning("update_instructions failed: %s", e)
                self.call_note(f"{self.agent_name} {'can see cameras on this call now' if enabled else 'no longer sees cameras on this call'}")
            chattiness = s.get("chattiness")
            if chattiness and chattiness != self.chattiness:
                self.chattiness = chattiness
                log.info("owner set chattiness %s", chattiness)

    async def idle_loop(self) -> None:
        while not self.state.leaving:
            await asyncio.sleep(5.0)
            if self.state.leaving or not self.session:
                return
            now = time.monotonic()
            if self.state.mode == "duplex":
                quiet = now - self.state.last_human_speech_at
                if quiet >= self.cfg.duplex_idle_leave_s:
                    await self.finish("idle")
                    return
                if quiet >= self.cfg.duplex_idle_ask_s and not self.state.idle_asked:
                    self.state.idle_asked = True
                    self.session.generate_reply(instructions="It has been quiet for a while. Ask in four words or fewer whether they are still there.")
            else:
                if now - self.state.last_trigger_at >= self.cfg.group_idle_leave_s:
                    await self.finish("idle")
                    return


# ---- worker -----------------------------------------------------------------------------------------


def prewarm(proc: JobProcess) -> None:
    proc.userdata["group_vad"] = silero.VAD.load(
        min_speech_duration=GROUP_BARGE_MIN_SPEECH_S, activation_threshold=0.6, min_silence_duration=CONFIG.group_end_silence_s
    )


async def request_fnc(req: JobRequest) -> None:
    """Accept the dispatch as `a_<project_id>` with the agent's `sender{}` as metadata."""
    try:
        meta = json.loads(req.job.metadata or "{}")
    except json.JSONDecodeError:
        meta = {}
    identity = meta.get("identity") or f"a_{meta.get('project_id', 'agent')}"
    agent = meta.get("agent") or {}
    await req.accept(
        name=agent.get("name") or "Arbos",
        identity=identity,
        metadata=json.dumps(agent) if agent else "",
        attributes={"arbos.kind": "agent", "arbos.call_id": str(meta.get("call_id", "")), "arbos.project_id": str(meta.get("project_id", ""))},
    )


async def entrypoint(ctx: JobContext) -> None:
    try:
        meta = json.loads(ctx.job.metadata or "{}")
    except json.JSONDecodeError:
        meta = {}
    if not meta.get("call_id") or not meta.get("project_id") or not meta.get("chat_id"):
        log.error("dispatch without call metadata: %r", ctx.job.metadata)
        return
    ctx.log_context_fields = {"call_id": meta["call_id"], "project_id": meta["project_id"]}
    async with aiohttp.ClientSession() as http:
        call = VoiceCall(ctx, CONFIG, meta, http)
        ctx.add_shutdown_callback(lambda: call.finish(call.state.stop_reason or "shutdown"))
        await call.run()


async def hello_relay() -> bool:
    """Tell the relay which build is running (`voice.worker_sha` in GET /admin/calls)."""
    try:
        async with aiohttp.ClientSession() as http:
            await Relay(CONFIG, http)._json(
                "POST", "/voice/worker/hello", {"sha": WORKER_SHA, "version": __version__, "model": CONFIG.model}, timeout=5.0
            )
        log.info("relay told: worker sha=%s version=%s", WORKER_SHA, __version__)
        return True
    except Exception as e:
        log.warning("relay hello failed: %s", e)
        return False


HELLO_EVERY_S = 300.0
HELLO_RETRY_S = 30.0


def start_hello_heartbeat() -> None:
    """Re-send the hello every 5 min (30 s after a failure) from a side thread, so a relay
    restart never leaves `voice.worker` blank. The worker has no persistent relay connection;
    the heartbeat is the reconnect."""

    async def loop() -> None:
        while True:
            ok = await hello_relay()
            await asyncio.sleep(HELLO_EVERY_S if ok else HELLO_RETRY_S)

    threading.Thread(target=lambda: asyncio.run(loop()), name="relay-hello", daemon=True).start()


def main() -> None:
    logging.basicConfig(
        level=os.environ.get("ARBOS_VOICE_LOG_LEVEL", "INFO"),
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
        stream=sys.stdout,
    )
    missing = CONFIG.check()
    if missing:
        log.error("missing configuration: %s", ", ".join(missing))
        raise SystemExit(2)
    if len(sys.argv) > 1 and sys.argv[1] in ("start", "dev"):
        start_hello_heartbeat()
    cli.run_app(
        WorkerOptions(
            entrypoint_fnc=entrypoint,
            request_fnc=request_fnc,
            prewarm_fnc=prewarm,
            agent_name=CONFIG.agent_name,
            ws_url=CONFIG.livekit_url,
            api_key=CONFIG.livekit_api_key,
            api_secret=CONFIG.livekit_api_secret,
            num_idle_processes=1,
            load_threshold=0.9,
            host="127.0.0.1",
        )
    )


if __name__ == "__main__":
    main()
