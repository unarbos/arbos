"""The chat, live, for the call (#1832). Jacob's rule (2026-10-25): the main voice agent is
the ONLY speaker on a call. Background jobs and the full agent never speak; what they post in
the chat streams into the voice agent's context as it arrives, and the voice agent decides
when and how to say it — a short summary at a natural pause, never over the person.

`CallUpdates` long-polls the relay's per-call inbox (`GET /voice/calls/{id}/inbox`) and, for
every update:

* puts a `[Chat]` note into the realtime model's conversation (a user-role text item, no
  response requested), so a later question ("is it done?") is answered from real state;
* for a finished answer (`result`) or a job outcome (`job`): asks the voice agent to speak a
  short summary once, after the room is quiet and the agent is idle. It waits for the pause;
  it never calls `interrupt()`. If the pause does not come within `SPEAK_WAIT_S`, the note stays
  in context and the agent brings it up on its next turn.
* `quick` status lines and people's chat messages are context only.

Updates are deduplicated by message id: the same bubble may reach the worker twice (the tool
result of `ask_agent` and the inbox).
"""

from __future__ import annotations

import asyncio
import logging
import time
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from typing import Any

log = logging.getLogger("arbos-voice.updates")

# Kinds that the voice agent speaks about on its own (a pause permitting).
SPOKEN_KINDS = frozenset({"result", "job"})
# How long a proactive summary waits for a pause before it is left to the next turn.
SPEAK_WAIT_S = 30.0
# Room quiet (nobody talking, agent idle) for this long = a natural pause.
PAUSE_S = 0.9
# Context note length (the full text is in the chat).
NOTE_CHARS = 1500
POLL_WAIT_S = 20
POLL_RETRY_S = 2.0


@dataclass
class Update:
    kind: str
    message_id: str
    text: str
    sender: dict = field(default_factory=dict)
    seq: int = 0

    @classmethod
    def from_json(cls, v: dict) -> Update:
        return cls(
            kind=str(v.get("kind") or "message"),
            message_id=str(v.get("message_id") or ""),
            text=str(v.get("text") or ""),
            sender=v.get("sender") or {},
            seq=int(v.get("seq") or 0),
        )


def note_for(u: Update, agent_name: str) -> str:
    """The `[Chat]` line the model sees."""
    who = u.sender.get("name") or u.sender.get("handle") or "someone"
    text = u.text.strip()
    if len(text) > NOTE_CHARS:
        text = text[:NOTE_CHARS].rstrip() + " …"
    if u.kind == "job":
        return f"[Chat] A background job of yours finished and posted this in the chat:\n{text}"
    if u.kind == "result":
        return f"[Chat] Your agent posted this answer in the chat:\n{text}"
    if u.kind == "quick":
        return f"[Chat] Your agent posted a status line in the chat:\n{text}"
    return f"[Chat] {who} wrote in the chat:\n{text}"


def speak_instructions(u: Update) -> str:
    what = "A background job of yours just finished" if u.kind == "job" else "Your agent just posted an answer"
    return (
        f"{what} in the chat — it is the last [Chat] note above. If it answers something asked on this call, "
        "tell the person now in one or two short spoken sentences: what was found and that the details are in "
        "the chat. Do not read tables, links or phone numbers aloud. If it is not relevant to this call, say nothing."
    )


class CallUpdates:
    """One per call. Pure logic with injected I/O so it can be tested without LiveKit:

    * `fetch(after, wait)` → `{items, seq, live}` (the relay long-poll);
    * `add_context(text)` puts a note into the model's conversation;
    * `speak(instructions)` asks the voice agent for one reply and resolves when it has played;
    * `quiet()` → True when nobody is talking and the agent is idle;
    * `leaving()` → True once the call is being torn down.
    """

    def __init__(
        self,
        *,
        agent_name: str,
        fetch: Callable[[int, int], Awaitable[dict]],
        add_context: Callable[[str], None],
        speak: Callable[[str], Awaitable[None]],
        quiet: Callable[[], bool],
        leaving: Callable[[], bool],
        on_update: Callable[[Update], None] | None = None,
    ):
        self.agent_name = agent_name
        self.fetch = fetch
        self.add_context = add_context
        self.speak = speak
        self.quiet = quiet
        self.leaving = leaving
        self.on_update = on_update
        self.seen: set[str] = set()
        self.seq = 0
        self.received = 0
        self.spoken = 0
        self._speak_lock = asyncio.Lock()
        self._task: asyncio.Task | None = None
        self._pending: set[asyncio.Task] = set()

    # ---- lifecycle ----------------------------------------------------------------------------

    def start(self) -> None:
        if self._task is None:
            self._task = asyncio.create_task(self._poll())

    async def stop(self) -> None:
        if self._task:
            self._task.cancel()
            self._task = None
        for t in list(self._pending):
            t.cancel()

    async def _poll(self) -> None:
        while not self.leaving():
            try:
                body = await self.fetch(self.seq, POLL_WAIT_S)
            except asyncio.CancelledError:
                raise
            except Exception as e:
                log.debug("inbox poll failed: %s", e)
                await asyncio.sleep(POLL_RETRY_S)
                continue
            for item in body.get("items") or []:
                await self.handle(Update.from_json(item))
            self.seq = max(self.seq, int(body.get("seq") or 0))
            if body.get("live") is False:
                return

    # ---- one update ----------------------------------------------------------------------------

    def mark_seen(self, message_id: str) -> bool:
        """Remember a bubble the call already has (the tool result). True when it was new."""
        if not message_id or message_id in self.seen:
            return False
        self.seen.add(message_id)
        return True

    async def handle(self, u: Update) -> bool:
        """Context note for every new update; a spoken summary for results and job outcomes.
        Returns False for a duplicate."""
        if u.message_id and u.message_id in self.seen:
            return False
        if u.message_id:
            self.seen.add(u.message_id)
        if not u.text.strip():
            return False
        self.received += 1
        self.add_context(note_for(u, self.agent_name))
        log.info("call update %s from %s (%d chars)", u.kind, u.sender.get("handle") or u.sender.get("kind"), len(u.text))
        if self.on_update:
            self.on_update(u)
        if u.kind in SPOKEN_KINDS:
            t = asyncio.create_task(self.say_at_pause(u))
            self._pending.add(t)
            t.add_done_callback(self._pending.discard)
        return True

    async def say_at_pause(self, u: Update) -> bool:
        """Speak once, at the next natural pause; never cut anyone off. False when no pause came."""
        async with self._speak_lock:
            deadline = time.monotonic() + SPEAK_WAIT_S
            quiet_since: float | None = None
            while time.monotonic() < deadline:
                if self.leaving():
                    return False
                if self.quiet():
                    quiet_since = quiet_since or time.monotonic()
                    if time.monotonic() - quiet_since >= PAUSE_S:
                        break
                else:
                    quiet_since = None
                await asyncio.sleep(0.05)
            else:
                log.info("no pause for the %s update within %.0f s; left to the next turn", u.kind, SPEAK_WAIT_S)
                return False
            try:
                await self.speak(speak_instructions(u))
            except Exception as e:
                log.warning("spoken update failed: %s", e)
                return False
            self.spoken += 1
            return True


def context_item(text: str) -> dict[str, Any]:
    """The realtime client event that adds a note to the conversation without a reply."""
    return {
        "type": "conversation.item.create",
        "item": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]},
    }
