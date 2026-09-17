"""`agent.activity`: what the kernel is doing right now, for the client to play and to show.

Derived from the kernel's own frames, one frame per transition and a heartbeat while work runs:

    {"type": "agent.activity", "agent": "root", "state": "working" | "tool" | "idle",
     "tool": "bash", "detail": "cargo build", "since_ms": 4200, "heartbeat": false}

- `working`: the agent's turn is running (`turn {state: running}`), no tool in flight.
- `tool`: a tool call started (a live `event {kind: tool}` with no `seq`) and has not ended
  (the same tool's recorded line, with a `seq`). `tool` names it, `detail` is what it runs.
- `idle`: the turn ended (`turn {state: idle}`).
- Every agent the call's kernel reports (the main agent and its workers) gets its own frames, so
  the client can tell "your agent is working" from "one of its workers is working".
- While any agent is not idle the current state is re-sent every HEARTBEAT_S with
  `heartbeat: true`, so a client can tell a long quiet turn from a dropped link and stop its
  sound when the beats stop.

The client's sound follows these frames and nothing else: no filler spoken by the speech
model, no timer. Agreed with the desktop as the consumer (2026-09-17).
"""

from __future__ import annotations

import asyncio
import logging
import time
from dataclasses import dataclass, field
from typing import Callable

from . import protocol as P

log = logging.getLogger("voice.activity")

HEARTBEAT_S = 5.0
DETAIL_CAP = 80

Emit = Callable[..., None]


@dataclass
class AgentActivity:
    state: str = "idle"
    tool: str = ""
    detail: str = ""
    call_id: str = ""
    since: float = field(default_factory=time.monotonic)


class ActivityReporter:
    """One per call. Listens to a kernel client, emits `agent.activity` to the session's client."""

    def __init__(self, kernel, emit: Emit):
        self.kernel = kernel
        self.emit = emit
        self.agents: dict[str, AgentActivity] = {}
        self._beat: asyncio.Task | None = None
        self.sent = 0

    def start(self) -> None:
        self.kernel.listeners.append(self.on_frame)
        self._beat = asyncio.create_task(self._heartbeat(), name="activity-heartbeat")

    def close(self) -> None:
        if self.on_frame in self.kernel.listeners:
            self.kernel.listeners.remove(self.on_frame)
        if self._beat:
            self._beat.cancel()

    @property
    def busy(self) -> bool:
        return any(a.state != "idle" for a in self.agents.values())

    def on_frame(self, frame: dict) -> None:
        kind = frame.get("type")
        agent = str(frame.get("agent") or "")
        if not agent:
            return
        if kind == "turn":
            state = "working" if frame.get("state") == "running" else "idle"
            cur = self.agents.setdefault(agent, AgentActivity())
            if state == "idle" or cur.state == "idle":
                self._set(agent, state, "", "", "")
        elif kind == "event":
            ev = frame.get("event") or {}
            if ev.get("kind") != "tool":
                return
            cur = self.agents.setdefault(agent, AgentActivity())
            name = str(ev.get("name") or "")
            call_id = str(ev.get("call_id") or "")
            if not ev.get("seq") and ev.get("ended") is None:
                # A live emit with no transcript line yet: the tool just started.
                detail = _detail(ev)
                self._set(agent, "tool", name, detail, call_id)
            elif cur.state == "tool" and (not call_id or call_id == cur.call_id or not cur.call_id):
                # Its recorded line: the tool ended; the turn goes on.
                self._set(agent, "working", "", "", "")
        elif kind == "link" and frame.get("state") == "lost":
            # A dropped link: nothing is known; say idle so the client's sound stops honestly.
            for name in list(self.agents):
                if self.agents[name].state != "idle":
                    self._set(name, "idle", "", "", "")

    def _set(self, agent: str, state: str, tool: str, detail: str, call_id: str) -> None:
        cur = self.agents.setdefault(agent, AgentActivity())
        if cur.state == state and cur.tool == tool and cur.call_id == call_id:
            return
        cur.state, cur.tool, cur.detail, cur.call_id = state, tool, detail, call_id
        cur.since = time.monotonic()
        self._send(agent, cur, heartbeat=False)

    def _send(self, agent: str, a: AgentActivity, *, heartbeat: bool) -> None:
        self.sent += 1
        self.emit(
            P.AGENT_ACTIVITY,
            agent=agent,
            state=a.state,
            tool=a.tool,
            detail=a.detail,
            since_ms=int((time.monotonic() - a.since) * 1000),
            heartbeat=heartbeat,
        )

    async def _heartbeat(self) -> None:
        while True:
            await asyncio.sleep(HEARTBEAT_S)
            for agent, a in list(self.agents.items()):
                if a.state != "idle":
                    self._send(agent, a, heartbeat=True)


def _detail(ev: dict) -> str:
    args = ev.get("args")
    if isinstance(args, dict):
        for key in ("cmd", "command", "path", "file", "query", "brief", "to"):
            v = args.get(key)
            if isinstance(v, str) and v.strip():
                return " ".join(v.split())[:DETAIL_CAP]
    return ""
