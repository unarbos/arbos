"""OpenAI GPT-Live backend (client delegation): a second full-duplex engine beside the hosted model.

GPT-Live listens, speaks and decides when to hand off; the Arbos kernel is the backend. The
wire protocol to the app and the phone is unchanged. Differences from the hosted duplex model:

- transport: `wss://api.openai.com/v1/live/sessions`, `session.start` with
  `delegation: {type: "client"}`; audio in `session.input_audio.append`, out in
  `session.output_audio.delta`; transcripts in `session.*_transcript.delta`.
- delegation: `session.delegation.created` carries only an id. The gateway takes the caller's
  last utterance (our VAD + Whisper transcript, GPT-Live's own transcript as fallback), asks the
  kernel, streams progress as `session.thinking.append` and the answer as
  `session.commentary.append`, which GPT-Live speaks in its own voice.
- interruption: GPT-Live yields on its own. A barge-in does not cancel the kernel turn (nor
  should it); a new question does, through the same `stop` path as the hosted engine.
- billing: $0.05 per minute of session, per second, plus the kernel's own model calls. The
  final `session.closed` carries the voice usage; it is logged per call.
"""

from __future__ import annotations

import asyncio
import base64
import json
import logging
import os
import time
import uuid

import re

import websockets

from . import protocol as P
from .activity import ActivityReporter, _detail
from .base import context_text
from .narrator import Narrator
from .routing import is_small_talk
from .audio import float_to_pcm16, pcm16_to_float
from .duplex import UPSTREAM_CHUNK_BYTES, DuplexSession, _ascii
from .tts import speakable

log = logging.getLogger("voice.openai")

LIVE_URL = os.environ.get("VOICE_OPENAI_URL") or "wss://api.openai.com/v1/live/sessions"
DEFAULT_MODEL = os.environ.get("VOICE_OPENAI_MODEL", "gpt-live-1")
DEFAULT_VOICE = os.environ.get("VOICE_OPENAI_VOICE", "marin")
MAX_APPEND_CHARS = 1500  # the append limit is 500 tokens; stay well under it
SCREEN_BRIEF_CHARS = 8000  # the on-screen chat in the instructions; session.input holds the rest

# The model's "I'll check" only makes sense if a delegation is really in flight. If it says such a
# thing and no session.delegation.created follows, the gateway delegates the utterance itself.
_ACK_WORDS = re.compile(r"\b(let me (check|look|get|see|find)|one (sec|second|moment)|checking|i'?ll (check|look|find out|get)|"
                        r"hold on|give me a (sec|second|moment)|looking (that|it) up)\b", re.I)
ACK_GRACE_S = 2.5

LIVE_INSTRUCTIONS = (
    "You are Arbos, the voice of a software engineer's agent system, on a call. Be brief, warm and "
    "direct: one or two sentences. You are given three stores and they are updated while you talk: "
    "which project this is, the chat the caller is looking at, and what the main agent and its "
    "sub-agents (workers) are doing. Answer those from the stores. Never invent a folder, a status "
    "or a result. Never quote diffs, file contents or a whole repository. Delegate to the backend "
    "to do work (write, fix, run, send an agent) or when the stores do not have the answer. The "
    "backend is the Arbos kernel; it does the work and returns the result for you to say. Say "
    "'one sec, let me check' only when you have actually delegated; then wait for the result and "
    "say it when it arrives, even if the conversation has moved on. Answer yourself greetings, "
    "thanks, small talk, and anything the stores already cover."
)

# Recent project-chat lines seeded into the session at start (session.input): how many, and how
# much of each. The API takes 128 messages / 8,192 tokens; this stays far under.
HISTORY_LINES = int(os.environ.get("VOICE_LIVE_HISTORY", "40"))
HISTORY_LINE_CHARS = 600
HISTORY_TOTAL_CHARS = 12000
# Live context appends (typed lines, chat replies, what workers do) are coalesced to this rate.
CONTEXT_MIN_GAP_S = 3.0

# Roles the desktop (and the phone) send as the on-screen chat. Thinking/notice/asked are
# visible rows; tool lines are labels only (never a diff or a file body).
_SCREEN_ROLES = {
    "user": "user",
    "assistant": "assistant",
    "arbos": "assistant",
    "worker": "worker",
    "tool": "tool",
    "notice": "notice",
    "asked": "asked",
    "thinking": "thinking",
}


def _clip_line(text: object, limit: int = HISTORY_LINE_CHARS) -> str:
    return " ".join(str(text or "").split())[:limit]


def _squash_line(text: str) -> str:
    return "".join(text.split()).lower()


def _lines_from_screen(context: dict) -> list[dict]:
    """What the caller is looking at: session.start.project.context.recent."""
    out: list[dict] = []
    for line in (context or {}).get("recent") or []:
        if not isinstance(line, dict):
            continue
        text = _clip_line(line.get("text"))
        if not text:
            continue
        kind = _SCREEN_ROLES.get(str(line.get("role") or ""), "")
        if not kind:
            continue
        out.append({"kind": kind, "text": text})
    return out


def _lines_from_transcript(events: list[dict]) -> list[dict]:
    """The kernel transcript as visible chat: user, Arbos, worker reports, tool names.

    Tool bodies, diffs and file contents are dropped. A tool line is the name and a
    short detail (the command or path), nothing else.
    """
    out: list[dict] = []
    for event in events:
        kind = event.get("kind")
        text = event.get("text")
        if kind in ("user", "assistant") and str(text or "").strip():
            out.append({"kind": kind, "text": _clip_line(text)})
        elif kind == "say" and str(text or "").strip():
            who = event.get("from") or "worker"
            out.append({"kind": "worker", "text": _clip_line(f"{who}: {text}")})
        elif kind == "tool" and event.get("name"):
            detail = _detail(event)
            label = str(event["name"]) + (f" ({detail})" if detail else "")
            out.append({"kind": "tool", "text": _clip_line(label)})
    return out


def _merge_chat_lines(screen: list[dict], kernel: list[dict], *, cap: int) -> list[dict]:
    """On-screen chat first (what he is looking at), then older kernel lines he has not seen."""
    seen = {_squash_line(line["text"]) for line in screen}
    extra = [line for line in kernel if _squash_line(line["text"]) not in seen]
    screen = screen[-cap:]
    room = max(0, cap - len(screen))
    return extra[-room:] + screen


def _seed_messages(lines: list[dict]) -> list[dict]:
    seed: list[dict] = []
    total = 0
    for line in lines:
        text = line["text"]
        total += len(text)
        if total > HISTORY_TOTAL_CHARS:
            break
        kind = line["kind"]
        if kind == "user":
            seed.append({"type": "message", "role": "user", "content": [{"type": "input_text", "text": _ascii(text)}]})
        elif kind == "assistant":
            seed.append({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": _ascii(text)}]})
        else:
            seed.append({"type": "message", "role": "developer", "content": [{"type": "input_text", "text": _ascii(f"{kind}: {text}")}]})
    if seed:
        seed.insert(0, {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": _ascii(
            "The following messages are this project's on-screen chat and recent history, oldest first, "
            "as the caller sees them. Worker and tool lines are names and short labels only — not diffs "
            "or file contents. They are context; the caller may refer to them.")}]})
    return seed


def _workers_brief(context: dict, kernel, activity) -> str:
    """Sub-agents on the screen, plus live kernel and activity state."""
    parts: list[str] = []
    agents = [a for a in (context or {}).get("agents", []) if isinstance(a, dict) and a.get("name")]
    if agents:
        parts.append("Sub-agents on the caller's screen:")
        for agent in agents:
            step = f" — {str(agent['step'])[:80]}" if agent.get("step") else ""
            parts.append(f"- {agent['name']}: {agent.get('state', '')}{step}")
    if (context or {}).get("running"):
        parts.append("The main agent had a turn running when the call started.")
    if kernel is not None:
        status = kernel.status_text()
        if status:
            parts.append("Live worker state from the kernel: " + status)
    if activity is not None:
        summary = activity.summary()
        if summary:
            parts.append("Live activity right now: " + summary)
    if not parts:
        return "No sub-agents were on the caller's screen when the call started, and none are running now."
    return "\n".join(parts)


class OpenAILiveSession(DuplexSession):
    engine = "openai"
    HOLD_MODEL_WHILE_DECIDING = False  # GPT-Live decides itself; never delay its first word

    async def on_open(self) -> None:
        await super().on_open()
        self.api_key = os.environ.get("OPENAI_API_KEY", "")
        self.live_model = getattr(self.engines, "openai_model", DEFAULT_MODEL)
        self.live_voice = getattr(self.engines, "openai_voice", DEFAULT_VOICE)
        self.answerer = "model"  # GPT-Live decides when the kernel is needed (client delegation)
        self.model_voice = "full"  # main's call-mode voice cutting is for the hosted model
        self.delegations: dict[str, asyncio.Task] = {}
        self.last_final = ""  # our latest Whisper transcript of the caller
        self.last_final_at = 0.0
        self.live_input = ""  # GPT-Live's own running transcript of the caller (fallback context)
        self.session_id = ""
        self.started_at = 0.0
        self.usage: dict = {}
        self.delegations_seen = 0  # session.delegation.created events so far
        self.live_output_since_final = ""  # the model's words since the caller last finished
        self.ack_watch: asyncio.Task | None = None
        self.brief_sent = ""  # the project brief GPT-Live has (instructions or a later append)
        self.context_queue: list[str] = []  # thinking appends waiting for the rate limit
        self.context_task: asyncio.Task | None = None
        self.context_kernel = None  # the kernel whose frames feed GPT-Live's context
        self.workers_known: set[str] = set()
        self.seed_task: asyncio.Task | None = None  # the chat-history read, started when the kernel is known
        self.last_answer_at = 0.0  # when a delegation's answer went to the model (its chat copy is not news)

    # ------------------------------------------------------------------ upstream

    async def _ensure_upstream(self) -> None:
        if self.up is not None:
            return
        if not self.api_key:
            self._emit(P.ERROR, code="no_openai_key", message="OPENAI_API_KEY is not set on the voice server")
            raise RuntimeError("no OPENAI_API_KEY")
        t0 = time.monotonic()
        # What the model knows from the first word: who and where, the chat on screen, and the
        # workers (the brief) plus the same chat as session.input. From the call's kernel and
        # the client's snapshot, never from the gateway's own folder.
        brief = self._project_brief()
        seed_task = self.seed_task or asyncio.create_task(self._history_seed())
        # The connect and the chat-history read run side by side; the seed may cost the model's
        # start at most a moment, never the caller's first word.
        live_url = os.environ.get("VOICE_OPENAI_URL") or LIVE_URL
        connect = asyncio.ensure_future(websockets.connect(
            live_url, additional_headers={"Authorization": f"Bearer {self.api_key}"},
            max_size=16 * 1024 * 1024, compression=None, open_timeout=20,
        ))
        try:
            seed = await asyncio.wait_for(asyncio.shield(seed_task), 1.5)
        except Exception as exc:
            log.warning("[%s] chat history not ready in time for the seed (%s); starting without it", self.sid, type(exc).__name__)
            seed = []
        self.up = await connect
        session: dict = {
            "model": self.live_model,
            "instructions": _ascii((self.instructions or LIVE_INSTRUCTIONS) + "\n\n" + brief),
            "audio": {"format": {"type": "audio/pcm", "rate": 24000}, "output": {"voice": self.live_voice}},
            "delegation": {"type": "client"},
        }
        if seed:
            session["input"] = seed
        self.brief_sent = brief
        await self.up.send(json.dumps({"type": "session.start", "event_id": "start", "session": session}))
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            msg = json.loads(await asyncio.wait_for(self.up.recv(), 20))
            if msg.get("type") == "session.started":
                self.session_id = (msg.get("session") or {}).get("id", "")
                break
            if msg.get("type") == "error":
                err = msg.get("error") or {}
                text = err.get("message") if isinstance(err, dict) else str(err)
                self._emit(P.ERROR, code="openai_live", message=f"GPT-Live refused the session: {text}")
                await self.up.close()
                self.up = None
                raise RuntimeError(f"GPT-Live: {text}")
        self.started_at = time.monotonic()
        self.pump_task = asyncio.create_task(self._pump(), name=f"live-pump-{self.sid}")
        self._feed_context_from(self.kernel)
        self._context_snapshot()
        log.info("[%s] GPT-Live session %s ready in %.0fms (%s, voice %s, client delegation; brief for %s, %d history lines)",
                 self.sid, self.session_id, (time.monotonic() - t0) * 1000, self.live_model, self.live_voice,
                 (self.project_info or {}).get("place") or (self.project_info or {}).get("name") or "no kernel", len(seed))

    # ------------------------------------------------------------------ what the model knows

    def _project_brief(self) -> str:
        """The call's place, the chat on screen, and the workers — so the model can answer
        those without guessing and without waking the kernel. From the hub roster when the
        call named a project; the gateway's own kernel is named as such, never presented as
        the caller's project."""
        info = self.project_info or {}
        folder = info.get("path") or info.get("place") or ""
        if info.get("via") in ("hub", "local", "own"):
            name = info.get("name") or info.get("project")
            machine = info.get("machine") or ""
            label = f"{machine}/{info.get('project')}" if machine else str(info.get("project"))
            where = f"on the machine '{machine}'" if machine else "on the caller's own machine"
            lines = [
                "PROJECT IDENTITY (authoritative; never invent or guess any of it):",
                f"This call is about the project '{name}' ({label}) {where}.",
            ]
            if folder:
                lines.append(f"Its folder, the working directory of everything the backend runs for this call, is {folder}.")
            else:
                lines.append("Its folder is not known to you; the backend knows it. If asked and you cannot see it here, delegate.")
            if info.get("store"):
                lines.append(f"Its Arbos address is {info['store']}.")
            if info.get("via"):
                lines.append(f"The call reached this kernel via {info['via']}.")
            lines.append("If asked where you are, which folder, which machine or which project: answer from this, and only this.")
            screen = context_text(self.start_context, lines=HISTORY_LINES, clip_at=400)
            if screen:
                lines.append("ON-SCREEN CHAT (what the caller is looking at; answer 'what is on screen' and 'what were we talking about' from this):")
                lines.append(screen[:SCREEN_BRIEF_CHARS])
            else:
                lines.append(
                    "ON-SCREEN CHAT: the client sent no snapshot of the chat. Recent history, if any, is in the "
                    "session input. If asked what is on screen and you have no history, say you cannot see the chat yet."
                )
            lines.append("WORKERS AND ACTIVITY (answer 'what are the agents doing' from this, then from later project updates):")
            lines.append(_workers_brief(self.start_context, self.kernel, self.activity))
            lines.append(
                "Do not dump diffs, file contents or a whole repository. Delegate only to do work, or when "
                "these stores do not have the answer."
            )
            return "\n".join(lines)
        if info:
            where = f"serving the folder {folder}" if folder else f"at {info.get('url') or 'its address'}"
            return (
                "PROJECT IDENTITY: no project was named for this call, so the backend is the voice server's "
                f"default kernel, {where}. Do not present it as the caller's project or machine. If the caller "
                "asks about their project, say this call is not attached to a project and where the backend really is."
            )
        return (
            "PROJECT IDENTITY: no Arbos kernel is attached to this call. If asked to check, run or change "
            "anything, say so plainly; never invent files, folders, machines or status."
        )

    async def _history_seed(self) -> list[dict]:
        """The chat the caller is looking at, plus older kernel lines, as session.input.

        The client's snapshot is what is on screen (user, Arbos, workers, tool labels, notices).
        The kernel transcript fills in older lines the snapshot dropped. Tool bodies and diffs
        are never seeded. Nothing from a different kernel.
        """
        if HISTORY_LINES <= 0:
            return []
        kernel_lines: list[dict] = []
        kernel = self.kernel
        if kernel is not None:
            try:
                events = await asyncio.wait_for(kernel.transcript_tail("root", bytes_=80_000), 4.0)
                kernel_lines = _lines_from_transcript(events)
            except Exception as exc:
                log.warning("[%s] no chat history from the kernel for the seed: %s", self.sid, type(exc).__name__)
        screen = _lines_from_screen(self.start_context)
        return _seed_messages(_merge_chat_lines(screen, kernel_lines, cap=HISTORY_LINES))

    def _feed_context_from(self, kernel) -> None:
        """Follow the call kernel's frames so GPT-Live hears what happens in the project while it
        talks: lines typed in the chat, Arbos's text replies it did not relay itself, workers
        starting and finishing, and live activity. Quiet context (session.thinking.append),
        never spoken on its own."""
        if kernel is None or kernel is self.context_kernel:
            return
        if self.context_kernel is not None and self._context_frame in self.context_kernel.listeners:
            self.context_kernel.listeners.remove(self._context_frame)
        self.context_kernel = kernel
        kernel.listeners.append(self._context_frame)
        self.workers_known = {name for name in kernel.agents if name != "root"}

    def _who(self, agent: str) -> str:
        return "The main agent" if agent in ("", "root") else f"Worker {agent}"

    def _context_snapshot(self) -> None:
        """One quiet line of who is running right now, so a question at the start of the call
        has an answer before the next tree or tool frame."""
        if self.kernel is None:
            return
        status = self.kernel.status_text()
        if status:
            self._context("Workers and activity now: " + status)
        if self.activity is not None:
            summary = self.activity.summary()
            if summary:
                self._context("Live activity: " + summary)

    def _context_frame(self, frame: dict) -> None:
        kind = frame.get("type")
        agent = str(frame.get("agent") or "")
        who = self._who(agent)
        if kind == "event":
            ev = frame.get("event") or {}
            ek, text = ev.get("kind"), " ".join(str(ev.get("text") or "").split())
            if ek == "user" and text and agent == "root":
                if ev.get("channel") == "voice":
                    return  # the caller's own words, already in the conversation
                device = f"on the {ev['device']}" if ev.get("device") else "in the project chat"
                self._context(f"The user typed {device}: {text[:500]}")
            elif (ek == "assistant" and text and agent == "root" and not self.delegations_in_flight()
                  and time.monotonic() - self.last_answer_at > 8.0):
                # a reply to a typed line or to another client; a delegation's own answer already
                # went to the model as commentary and is skipped
                self._context(f"Arbos replied in the project chat (text, not spoken): {text[:700]}")
            elif ek == "say" and text:
                src = ev.get("from") or agent or "worker"
                self._context(f"Worker {src} reported: {text[:500]}")
            elif ek == "tool" and ev.get("name") and not ev.get("seq") and ev.get("ended") is None:
                # name + short detail only — never a body or a diff
                detail = _ascii(_detail(ev)) if _detail(ev) else ""
                self._context(f"{who} is running {ev['name']}" + (f": {detail}" if detail else ""))
        elif kind in ("tree", "snapshot"):
            names = {str(n.get("id")) for n in frame.get("tree", []) if n.get("id") and n.get("id") != "root"}
            new, gone = names - self.workers_known, self.workers_known - names
            self.workers_known = names
            if new:
                self._context("Workers (sub-agents) now running for this project: " + ", ".join(sorted(new)))
            if gone:
                self._context("Workers finished: " + ", ".join(sorted(gone)))
        elif kind == "turn":
            if frame.get("state") == "running":
                self._context(f"{who} started a turn.")
            elif frame.get("state") == "idle" and agent != "root":
                self._context(f"{who} finished its turn.")
            elif frame.get("state") == "idle" and agent == "root" and time.monotonic() - self.last_answer_at > 2.0:
                self._context("The main agent finished its turn.")

    def delegations_in_flight(self) -> bool:
        return any(not t.done() for t in self.delegations.values())

    def _context(self, line: str) -> None:
        """Queue one quiet line for the model; lines within CONTEXT_MIN_GAP_S go as one append."""
        if self.up is None:
            return
        self.context_queue.append(line)
        if self.context_task is None or self.context_task.done():
            self.context_task = asyncio.create_task(self._flush_context(), name=f"live-context-{self.sid}")

    async def _flush_context(self) -> None:
        await asyncio.sleep(CONTEXT_MIN_GAP_S)
        lines, self.context_queue = self.context_queue, []
        if not lines or self.up is None:
            return
        body = "Project update: " + " | ".join(lines)
        await self._append("session.thinking.append", None, body)
        log.info("[%s] context -> GPT-Live: %d line(s), %d chars", self.sid, len(lines), len(body))

    async def on_audio(self, data: bytes) -> None:
        await self._ensure_upstream()
        self._uplink(data)
        if not self.to_up.identity:
            data = float_to_pcm16(self.to_up.process(pcm16_to_float(data)))
        self.pending += data
        while len(self.pending) >= UPSTREAM_CHUNK_BYTES and self.up is not None:
            chunk, self.pending = bytes(self.pending[:UPSTREAM_CHUNK_BYTES]), self.pending[UPSTREAM_CHUNK_BYTES:]
            await self.up.send(json.dumps({"type": "session.input_audio.append", "audio": base64.b64encode(chunk).decode()}))

    async def on_close(self) -> None:
        if self.up is not None:
            try:
                await self.up.send(json.dumps({"type": "session.close"}))
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline and not self.usage:
                    await asyncio.sleep(0.1)
            except Exception:
                pass
        for task in self.delegations.values():
            task.cancel()
        if self.context_task is not None:
            self.context_task.cancel()
        if self.context_kernel is not None and self._context_frame in self.context_kernel.listeners:
            self.context_kernel.listeners.remove(self._context_frame)
        if self.started_at:
            seconds = time.monotonic() - self.started_at
            log.info("[%s] GPT-Live usage: %s (wall %.0fs, ~$%.4f voice at $0.05/min)", self.sid,
                     json.dumps(self.usage)[:300], seconds, seconds / 60 * 0.05)
        await super().on_close()

    # ------------------------------------------------------------------ events

    def _translate(self, msg: dict) -> None:
        kind = msg.get("type", "")
        if kind == "session.output_audio.delta":
            self._on_model_audio(base64.b64decode(msg.get("delta", "")))
        elif kind == "session.output_transcript.delta":
            delta = msg.get("delta", "")
            self.live_output_since_final = (self.live_output_since_final + delta)[-400:]
            if self.response_open:
                self._emit_for_gen(self.gen, P.RESPONSE_TRANSCRIPT, text=delta)
            else:
                self.transcript_stash.append((time.monotonic(), delta))
        elif kind == "session.input_transcript.delta":
            self.live_input += msg.get("delta", "")
            self.live_input = self.live_input[-2000:]
        elif kind == "session.delegation.created":
            delegation = msg.get("delegation") or {}
            did = str(delegation.get("id", ""))
            if delegation.get("target", "client") == "client" and did:
                self.delegations_seen += 1
                self.delegations[did] = asyncio.create_task(self._delegate(did), name=f"delegate-{did}")
        elif kind in ("session.commentary.appended", "session.thinking.appended", "session.instructions.appended"):
            pass
        elif kind == "session.closed":
            self.usage = msg.get("usage") or {"closed": True}
        elif kind == "error":
            err = msg.get("error") or {}
            text = err.get("message") if isinstance(err, dict) else str(err or msg)
            log.warning("[%s] GPT-Live error: %s", self.sid, text)
            self._emit(P.ERROR, code="openai_live", message=f"GPT-Live: {text}")
        elif kind.startswith("session.") and kind.endswith((".updated", ".started")):
            pass
        else:
            log.debug("[%s] GPT-Live event %s", self.sid, kind)

    # ------------------------------------------------------------------ our transcript

    def _route_final(self, text: str, *, source: str) -> None:
        """Our VAD + Whisper transcript is the record; GPT-Live decides what to delegate."""
        if not text:
            self._release_model()
            return
        self.user_turns += 1
        self.last_final, self.last_final_at = text, time.monotonic()
        if self.live_input.strip():
            log.info("[%s] GPT-Live heard: %r", self.sid, self.live_input.strip()[-160:])
        self.live_input = ""
        self.live_output_since_final = ""
        log.info("[%s] user (%s): %r", self.sid, source, text)
        self._release_model()
        narrator = self.narrator
        if narrator is not None and narrator.pending_ask is not None:
            # An approval or a question from the kernel is open: the caller's words answer it.
            # Ours to decide, never the provider's.
            narrator.user_said(text, channel=self.channel)
            return
        if self.ack_watch is not None and not self.ack_watch.done():
            self.ack_watch.cancel()
        self.ack_watch = asyncio.create_task(self._ensure_delegated(self.delegations_seen, text))

    async def _ensure_delegated(self, seen_before: int, text: str) -> None:
        """The gateway's guarantee, whatever the model decides: anything that is not allowlisted
        small talk reaches the kernel. If GPT-Live has not created a delegation within the grace
        period for such an utterance, or said it would check without one, delegate it ourselves."""
        await asyncio.sleep(ACK_GRACE_S)
        if self.delegations_seen > seen_before:
            return
        said_check = bool(_ACK_WORDS.search(self.live_output_since_final))
        if is_small_talk(text) and not said_check:
            return  # allowlisted small talk: the model's own answer is the answer
        log.warning("[%s] model %s without delegating; delegating %r ourselves", self.sid,
                    "said it would check" if said_check else "took a work question itself", text)
        await self._delegate(None, forced_question=text)

    # ------------------------------------------------------------------ the kernel as backend

    async def _delegate(self, did: str | None, forced_question: str = "") -> None:
        """GPT-Live asked for backend help: ask the kernel, return the answer for it to speak."""
        t0 = time.monotonic()
        # The event carries no text. Our transcript of the utterance usually lands within a
        # second of it (600 ms end-silence + Whisper); wait for it, else use GPT-Live's own.
        question = forced_question
        deadline = t0 + 1.5
        while not question and time.monotonic() < deadline:
            if self.last_final and t0 - self.last_final_at < 6.0:
                question = self.last_final
                break
            await asyncio.sleep(0.05)
        if not question:
            question = self.live_input.strip() or self.last_final
        if not question:
            await self._append("session.commentary.append", did, "I did not catch what you asked. Could you say it again?")
            return
        kernel = self.kernel
        if kernel is None or not kernel.connected:
            await self._append("session.commentary.append", did, "The Arbos kernel is not reachable right now.")
            return
        tag = (did or "forced")[-8:]
        log.info("[%s] delegation %s -> kernel: %r", self.sid, tag, question)
        self._emit(P.TOOL_CALL, name="delegate", arguments={"question": question, "delegation": did})
        if self.activity is not None:
            self.activity.mark_working()  # the working sound starts now, before the kernel's own turn frame
        self.kernel_launched_at = t0
        await self._append("session.thinking.append", did, "Arbos is working on it.")
        answer = ""
        try:
            # Filed as spoken (`channel: voice`, the caller's device): the chat shows how the line
            # arrived, and a client that already drew it from transcript.final can tell it apart.
            async for delta in kernel.turn(question, timeout=120, channel="voice", device=self.device):
                answer += delta
        except Exception as exc:
            log.exception("[%s] delegation failed", self.sid)
            await self._append("session.commentary.append", did, f"The kernel failed: {_ascii(str(exc))[:200]}")
            return
        spoken = speakable(answer).replace("\n", " ").strip() or "Arbos had no answer."
        self._emit(P.TOOL_RESULT, name="delegate", output=spoken[:400])
        self.last_answer_at = time.monotonic()
        await self._append("session.commentary.append", did, spoken[:MAX_APPEND_CHARS])
        log.info("[%s] delegation %s answered in %.1fs (%d chars)", self.sid, tag, time.monotonic() - t0, len(spoken))

    async def _append(self, kind: str, did: str | None, content: str) -> None:
        if self.up is None:
            return
        await self.up.send(json.dumps({
            "type": kind, "event_id": str(uuid.uuid4()),
            "delegation_id": did, "content": _ascii(content)[:MAX_APPEND_CHARS],
        }))

    async def speak_narration(self, text: str) -> None:
        """Kernel asks and approvals, child reports: GPT-Live says them (its voice), else Kokoro."""
        if self.up is not None:
            self._emit(P.RESPONSE_TRANSCRIPT, text=text)
            await self._append("session.commentary.append", None, text)
        else:
            await super().speak_narration(text)

    async def _start_call(self) -> None:
        """Call mode with GPT-Live: the model delegates to the kernel itself, so the narrator only
        speaks asks and approvals (and takes the caller's yes or no); it never forwards or
        summarises, or the answer would be spoken twice."""
        if self.call_kernel is None:
            kernel = await self._kernel_for(self.project)
            if kernel is not None and kernel is not self.engines.kernel:
                self.call_kernel = kernel
                self.tools.rebind(kernel)
                if self.engines.kernel and self._mirror in self.engines.kernel.listeners:
                    self.engines.kernel.listeners.remove(self._mirror)
                kernel.listeners.append(self._mirror)
        if self.narrator is not None:
            self.narrator.close()
            self.narrator = None
        if self.kernel is not None:
            self.narrator = Narrator(
                self.kernel, speak=self.speak_narration, emit=self._emit, screen=self.screen, device=self.device,
                user_talking=lambda: self.user_talking, arbos_talking=self.arbos_talking,
                ack=False, speak_details=False, only_asks=True, approval_timeout=self.defaults.approval_timeout,
            )
            self.narrator.start()
            if self.activity is None:
                # agent.activity for the client's working sound: the kernel's own turn and tool
                # frames (ActivityReporter, the shape agreed with the desktop), plus `working` the
                # moment a delegation leaves for the kernel (mark_working in _delegate).
                self.activity = ActivityReporter(self.kernel, self._emit)
                self.activity.start()
        if self.kernel is not None and self.seed_task is None and self.up is None:
            self.seed_task = asyncio.create_task(self._history_seed(), name=f"live-seed-{self.sid}")
        if self.up is not None:
            # The model is already up (audio came before session.start): give it the brief now.
            brief = self._project_brief()
            if brief != self.brief_sent:
                self.brief_sent = brief
                await self._append("session.instructions.append", None, brief)
            self._feed_context_from(self.kernel)
            self._context_snapshot()
        info = self.project_info or {}
        log.info("[%s] call mode (GPT-Live, client delegation) on %s (%s, folder %s)", self.sid,
                 self.project or "the gateway's kernel", info.get("via", "none"), info.get("place") or "unknown")

    async def on_call_text(self, text: str) -> None:
        """Typed during the call: to the project's kernel, as a steer when a turn is running (the
        narrator here only takes asks, so it must not be the one to file it). GPT-Live learns of
        the line from the kernel's own `user` event, through the context feed."""
        narrator = self.narrator
        if narrator is not None and narrator.pending_ask is not None:
            narrator.user_said(text, channel="text")  # the typed line answers the open ask
        elif self.kernel is not None and self.kernel.connected:
            self.kernel.send_user(text, channel="text", device=self.device)  # steer = the agent is running
            log.info("[%s] typed -> kernel: %r", self.sid, text[:120])
        else:
            self._emit(P.ERROR, message="typed line not delivered: the kernel is not reachable")
            return
        self._emit(P.TEXT_DONE, text="", cancelled=False, forwarded=True)

    async def on_report_speech(self, text: str) -> None:
        """An agent finished: let GPT-Live say it in its own voice."""
        if self.up is not None:
            await self._append("session.commentary.append", None, text)
        else:
            await super().on_report_speech(text)

    async def on_interrupt(self, cause: str) -> None:
        # GPT-Live stops itself when the caller talks; backend work continues (by design). Frames
        # already in flight are dropped until it goes quiet, like the hosted engine.
        self.gen += 1
        self.muted = True
        self._emit(P.RESPONSE_DONE, interrupted=True, reason="interrupted")
        self.transcript_stash.clear()
        self._close_response()
