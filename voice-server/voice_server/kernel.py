"""Client for `arbos-kernel serve`: newline-delimited JSON over loopback TCP.

The kernel writes `<place>/.arbos/kernel.json` = {"url": "tcp://127.0.0.1:PORT"}.
Frames are `Frame` in crates/arbos-core/src/wire.rs. We send `user` turns and
`approve`; we read `event`, `turn`, `tree`, `ask`.
"""

from __future__ import annotations

import asyncio
import json
import logging
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import AsyncIterator, Awaitable, Callable

log = logging.getLogger("voice.kernel")

Listener = Callable[[dict], Awaitable[None] | None]


@dataclass
class AgentState:
    name: str
    parent: str | None
    running: bool = False
    says: list[str] = field(default_factory=list)
    assistant: str = ""
    started_at: float = field(default_factory=time.monotonic)


class KernelClient:
    def __init__(self, url: str | None = None, place: str | None = None, *, auto_approve: bool = True):
        if not url and not place:
            raise ValueError("need a kernel url or place")
        self.url = url
        self.place = place
        self.auto_approve = auto_approve
        self.reader: asyncio.StreamReader | None = None
        self.writer: asyncio.StreamWriter | None = None
        self.agents: dict[str, AgentState] = {}
        self.focus = "root"
        self.listeners: list[Listener] = []
        self._reader_task: asyncio.Task | None = None

    # ------------------------------------------------------------------ connection

    def _resolve(self) -> tuple[str, int]:
        url = self.url
        if not url:
            info = json.loads((Path(self.place) / ".arbos" / "kernel.json").read_text())
            url = info["url"]
        host, port = url.removeprefix("tcp://").rsplit(":", 1)
        return host, int(port)

    async def connect(self) -> None:
        host, port = self._resolve()
        self.reader, self.writer = await asyncio.open_connection(host, port)
        self._reader_task = asyncio.create_task(self._read_loop(), name="kernel-read")
        log.info("attached to kernel at %s:%d", host, port)

    @property
    def connected(self) -> bool:
        return self.writer is not None and not self.writer.is_closing()

    async def close(self) -> None:
        if self._reader_task:
            self._reader_task.cancel()
        if self.writer:
            self.writer.close()

    def send(self, frame: dict) -> None:
        if not self.writer:
            raise RuntimeError("kernel not connected")
        self.writer.write((json.dumps(frame) + "\n").encode())

    async def _read_loop(self) -> None:
        assert self.reader
        try:
            while True:
                line = await self.reader.readline()
                if not line:
                    break
                try:
                    frame = json.loads(line)
                except json.JSONDecodeError:
                    continue
                self._track(frame)
                for listener in list(self.listeners):
                    try:
                        result = listener(frame)
                        if asyncio.iscoroutine(result):
                            await result
                    except Exception:
                        log.exception("kernel listener failed")
        finally:
            log.warning("kernel connection closed")
            if self.writer:
                self.writer.close()
            self.writer = None

    def _track(self, frame: dict) -> None:
        kind = frame.get("type")
        if kind in ("snapshot", "tree"):
            seen = set()
            for node in frame.get("tree", []):
                name = node["id"]
                seen.add(name)
                if name not in self.agents:
                    self.agents[name] = AgentState(name=name, parent=node.get("parent"))
                else:  # a turn/event frame may have created it before the tree arrived
                    self.agents[name].parent = node.get("parent")
            for name in [n for n in self.agents if n not in seen]:
                del self.agents[name]
            if kind == "snapshot":
                self.focus = str(frame.get("focus", "")).split("/")[-1] or "root"
        elif kind == "turn":
            state = self.agents.setdefault(frame["agent"], AgentState(name=frame["agent"], parent=None))
            state.running = frame.get("state") == "running"
        elif kind == "event":
            event = frame.get("event") or {}
            state = self.agents.setdefault(frame["agent"], AgentState(name=frame["agent"], parent=None))
            if event.get("kind") == "say" and event.get("text"):
                state.says.append(event["text"])
                if event.get("from"):  # a child's say is delivered on the parent's stream
                    self.agents.setdefault(event["from"], AgentState(name=event["from"], parent=frame["agent"])).says.append(event["text"])
            elif event.get("kind") == "assistant" and event.get("text"):
                state.assistant += event["text"]
            elif event.get("kind") == "user":
                state.assistant = ""
        elif kind == "ask" and self.auto_approve and str(frame.get("question", "")).startswith("allow "):
            self.send({"type": "approve", "agent": frame["agent"], "call_id": "", "allow": True})

    # ------------------------------------------------------------------ turns

    async def turn(self, text: str, agent: str = "root", *, steer: bool = False, timeout: float = 120.0) -> AsyncIterator[str]:
        """Send one user turn and yield the agent's assistant text deltas until it goes idle."""
        queue: asyncio.Queue[str | None] = asyncio.Queue()
        started = False

        def listener(frame: dict) -> None:
            nonlocal started
            if frame.get("agent") != agent:
                return
            if frame.get("type") == "turn":
                if frame.get("state") == "running":
                    started = True
                elif started:
                    queue.put_nowait(None)
            elif frame.get("type") == "event":
                event = frame.get("event") or {}
                if event.get("kind") == "assistant" and event.get("text"):
                    queue.put_nowait(event["text"])
                elif event.get("kind") == "say" and event.get("text") and event.get("from"):
                    queue.put_nowait(f"\n[{event['from']}] {event['text']}\n")

        self.listeners.append(listener)
        emitted = ""
        try:
            self.send({"type": "user", "agent": agent, "text": text, "steer": steer, "attachments": []})
            deadline = time.monotonic() + timeout
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    yield "\n(the agent is still working; I will report when it finishes)"
                    return
                try:
                    item = await asyncio.wait_for(queue.get(), remaining)
                except asyncio.TimeoutError:
                    yield "\n(the agent is still working; I will report when it finishes)"
                    return
                if item is None:
                    return
                if emitted and item.strip() and _squash(emitted).endswith(_squash(item)):
                    continue  # the kernel re-sends the full text at the end of a turn
                emitted += item
                yield item
        finally:
            self.listeners.remove(listener)

    def children_of(self, parent: str = "root") -> list[AgentState]:
        return [a for a in self.agents.values() if a.parent == parent]

    def status_text(self) -> str:
        return _status_text(self.agents)


def _squash(text: str) -> str:
    return "".join(text.split())


def _status_text(agents: dict[str, AgentState]) -> str:
    if not agents:
        return "No agents are running."
    lines = []
    for a in agents.values():
        if a.name == "root":
            continue
        state = "working" if a.running else "finished"
        last = a.says[-1] if a.says else (a.assistant.strip()[-200:] if a.assistant.strip() else "no report yet")
        lines.append(f"{a.name}: {state}. Last report: {last}")
    root = agents.get("root")
    if root:
        lines.insert(0, f"Main agent is {'busy' if root.running else 'idle'}.")
    return " ".join(lines) if lines else "The main agent is idle and no sub-agents exist."

