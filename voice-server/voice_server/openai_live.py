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
from .narrator import Narrator
from .routing import is_small_talk
from .audio import float_to_pcm16, pcm16_to_float
from .duplex import UPSTREAM_CHUNK_BYTES, DuplexSession, _ascii
from .tts import speakable

log = logging.getLogger("voice.openai")

LIVE_URL = "wss://api.openai.com/v1/live/sessions"
DEFAULT_MODEL = os.environ.get("VOICE_OPENAI_MODEL", "gpt-live-1")
DEFAULT_VOICE = os.environ.get("VOICE_OPENAI_VOICE", "marin")
MAX_APPEND_CHARS = 1500  # the append limit is 500 tokens; stay well under it

# The model's "I'll check" only makes sense if a delegation is really in flight. If it says such a
# thing and no session.delegation.created follows, the gateway delegates the utterance itself.
_ACK_WORDS = re.compile(r"\b(let me (check|look|get|see|find)|one (sec|second|moment)|checking|i'?ll (check|look|find out|get)|"
                        r"hold on|give me a (sec|second|moment)|looking (that|it) up)\b", re.I)
ACK_GRACE_S = 2.5

LIVE_INSTRUCTIONS = (
    "You are Arbos, the voice of a software engineer's agent system, on a call. Be brief, warm and "
    "direct: one or two sentences. Delegate to the backend anything about the user's project, code, "
    "agents, files, work, status or progress, and any request to do something (write, fix, run, "
    "check, send an agent). When in doubt, delegate. The backend is the Arbos kernel; it does the work "
    "and returns the result for you to say. Say 'one sec, let me check' only when you have actually "
    "delegated; then wait for the result and say it when it arrives, even if the conversation has "
    "moved on. Never invent a result or a status. Answer yourself only greetings, thanks, small talk "
    "and general knowledge that has nothing to do with the user's work."
)


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

    # ------------------------------------------------------------------ upstream

    async def _ensure_upstream(self) -> None:
        if self.up is not None:
            return
        if not self.api_key:
            self._emit(P.ERROR, code="no_openai_key", message="OPENAI_API_KEY is not set on the voice server")
            raise RuntimeError("no OPENAI_API_KEY")
        t0 = time.monotonic()
        self.up = await websockets.connect(
            LIVE_URL, additional_headers={"Authorization": f"Bearer {self.api_key}"},
            max_size=16 * 1024 * 1024, compression=None, open_timeout=20,
        )
        await self.up.send(json.dumps({
            "type": "session.start",
            "event_id": "start",
            "session": {
                "model": self.live_model,
                "instructions": _ascii(self.instructions or LIVE_INSTRUCTIONS),
                "audio": {"format": {"type": "audio/pcm", "rate": 24000}, "output": {"voice": self.live_voice}},
                "delegation": {"type": "client"},
            },
        }))
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
        log.info("[%s] GPT-Live session %s ready in %.0fms (%s, voice %s, client delegation)",
                 self.sid, self.session_id, (time.monotonic() - t0) * 1000, self.live_model, self.live_voice)

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
        self._activity("working")  # the working sound starts now, before the kernel's own turn frame
        self.kernel_launched_at = t0
        await self._append("session.thinking.append", did, "Arbos is working on it.")
        answer = ""
        try:
            async for delta in kernel.turn(question, timeout=120):
                answer += delta
        except Exception as exc:
            log.exception("[%s] delegation failed", self.sid)
            await self._append("session.commentary.append", did, f"The kernel failed: {_ascii(str(exc))[:200]}")
            return
        spoken = speakable(answer).replace("\n", " ").strip() or "Arbos had no answer."
        self._emit(P.TOOL_RESULT, name="delegate", output=spoken[:400])
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
        log.info("[%s] call mode (GPT-Live, client delegation) on %s", self.sid, self.project or "the gateway's kernel")

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
