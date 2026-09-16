"""Client for `arbos-kernel serve`: loopback TCP (newline-delimited JSON) or a remote WebSocket.

Frames are `Frame` in crates/arbos-core/src/wire.rs. We send `user` turns and
`approve`; we read `hello`, `event`, `assistant_delta`, `turn`, `tree`, `ask`.
"""

from __future__ import annotations

import asyncio
import json
import logging
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import AsyncIterator, Awaitable, Callable

import websockets

log = logging.getLogger("voice.kernel")

Listener = Callable[[dict], Awaitable[None] | None]
DISCONNECTED = "__disconnected__"


@dataclass
class AgentState:
    name: str
    parent: str | None
    running: bool = False
    says: list[str] = field(default_factory=list)
    assistant: str = ""
    started_at: float = field(default_factory=time.monotonic)


class KernelClient:
    """Attach client with two transports and automatic reconnection.

    - ``tcp://host:port`` (or a place dir with ``.arbos/kernel.json``): newline-delimited
      JSON, trusted by the kernel because it is loopback.
    - ``ws://`` / ``wss://host[/path][?token=...]``: one JSON object per WebSocket message,
      for a kernel behind a Cloudflare tunnel. The token (``?token=`` in the URL, or
      ``token=`` / ``VOICE_KERNEL_TOKEN``) goes in the ``Authorization`` header, never
      in a log line or in the URL we connect to.

    When the link drops, ``connected`` turns false, callers in a turn get a
    ``__disconnected__`` frame, and a background task redials with backoff. On
    reconnect the kernel replays hello/snapshot/plans; the tree is rebuilt from that.
    """

    def __init__(self, url: str | None = None, place: str | None = None, *, token: str | None = None,
                 auto_approve: bool = True):
        if not url and not place:
            raise ValueError("need a kernel url or place")
        self.url = url
        self.place = place
        self.auto_approve = auto_approve
        self.token = token
        if url and "?" in url:  # move a query token into the header, and keep it out of logs
            base, _, query = url.partition("?")
            for kv in query.split("&"):
                key, _, value = kv.partition("=")
                if key == "token" and value:
                    self.token = self.token or value
            self.url = base
        self.display = self.url or f"place:{place}"
        self.reader: asyncio.StreamReader | None = None
        self.writer: asyncio.StreamWriter | None = None
        self.ws = None
        self.agents: dict[str, AgentState] = {}
        self.focus = "root"
        self.kernel_version = ""
        self.listeners: list[Listener] = []
        self._reader_task: asyncio.Task | None = None
        self._redial_task: asyncio.Task | None = None
        self._closing = False
        self._connected = False

    # ------------------------------------------------------------------ connection

    @property
    def is_ws(self) -> bool:
        return bool(self.url and self.url.startswith(("ws://", "wss://")))

    def _resolve_tcp(self) -> tuple[str, int]:
        url = self.url
        if not url:
            info = json.loads((Path(self.place) / ".arbos" / "kernel.json").read_text())
            url = info["url"]
        host, port = url.removeprefix("tcp://").rsplit(":", 1)
        return host, int(port)

    async def connect(self, *, redial: bool = True) -> None:
        """Dial once. Raises on failure; with ``redial`` a background task keeps trying after a drop."""
        await self._dial()
        if redial and (self._redial_task is None or self._redial_task.done()):
            self._redial_task = asyncio.create_task(self._redial_loop(), name="kernel-redial")

    def start_background(self) -> None:
        """Like connect(), but never raises: keep dialing in the background until it works."""
        if self._redial_task is None or self._redial_task.done():
            self._redial_task = asyncio.create_task(self._redial_loop(), name="kernel-redial")

    async def _dial(self) -> None:
        if self.is_ws:
            headers = {"Authorization": f"Bearer {self.token}"} if self.token else {}
            self.ws = await websockets.connect(
                self.url, additional_headers=headers, open_timeout=15, ping_interval=20, ping_timeout=20,
                max_size=8 * 1024 * 1024, compression=None,
            )
            first = await asyncio.wait_for(self.ws.recv(), 15)
            frame = json.loads(first)
            if frame.get("type") == "error":
                await self.ws.close()
                self.ws = None
                raise PermissionError(f"kernel refused the attach: {frame.get('detail', 'error')}")
            self._connected = True
            self._reader_task = asyncio.create_task(self._read_loop_ws(frame), name="kernel-read")
        else:
            host, port = self._resolve_tcp()
            self.reader, self.writer = await asyncio.open_connection(host, port)
            self._connected = True
            self._reader_task = asyncio.create_task(self._read_loop_tcp(), name="kernel-read")
        log.info("attached to kernel at %s", self.display)

    async def _redial_loop(self) -> None:
        delay = 1.0
        while not self._closing:
            if self._connected:
                delay = 1.0
                await asyncio.sleep(1.0)
                continue
            try:
                await self._dial()
            except PermissionError as exc:
                log.error("%s; retrying in 60s", exc)
                await asyncio.sleep(60)
            except Exception as exc:
                log.warning("kernel %s unreachable (%s); retrying in %.0fs", self.display, type(exc).__name__, delay)
                await asyncio.sleep(delay)
                delay = min(delay * 2, 30.0)

    @property
    def connected(self) -> bool:
        return self._connected

    async def close(self) -> None:
        self._closing = True
        for task in (self._redial_task, self._reader_task):
            if task:
                task.cancel()
        if self.writer:
            self.writer.close()
        if self.ws:
            await self.ws.close()

    def send(self, frame: dict) -> None:
        if not self._connected:
            raise RuntimeError("kernel not connected")
        if self.is_ws:
            asyncio.create_task(self._ws_send(json.dumps(frame)))
        else:
            self.writer.write((json.dumps(frame) + "\n").encode())

    async def _ws_send(self, text: str) -> None:
        try:
            await self.ws.send(text)
        except Exception as exc:
            log.warning("kernel send failed: %s", type(exc).__name__)

    async def _read_loop_tcp(self) -> None:
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
                await self._dispatch(frame)
        finally:
            await self._dropped()

    async def _read_loop_ws(self, first: dict) -> None:
        try:
            await self._dispatch(first)
            async for raw in self.ws:
                try:
                    frame = json.loads(raw)
                except json.JSONDecodeError:
                    continue
                await self._dispatch(frame)
        except Exception as exc:
            log.warning("kernel link error: %s", type(exc).__name__)
        finally:
            await self._dropped()

    async def _dropped(self) -> None:
        was = self._connected
        self._connected = False
        if self.writer:
            self.writer.close()
            self.writer = None
        self.ws = None
        for a in self.agents.values():
            a.running = False
        if was and not self._closing:
            log.warning("kernel connection to %s closed; will redial", self.display)
            await self._dispatch({"type": "__disconnected__"})

    async def _dispatch(self, frame: dict) -> None:
        if frame.get("type") == "hello":
            self.kernel_version = str(frame.get("kernel", ""))
            log.info("kernel hello: version %s, focus %s", self.kernel_version, frame.get("focus"))
        self._track(frame)
        for listener in list(self.listeners):
            try:
                result = listener(frame)
                if asyncio.iscoroutine(result):
                    await result
            except Exception:
                log.exception("kernel listener failed")

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
        elif kind == "assistant_delta":  # kernels >= 0.2 stream text this way
            state = self.agents.setdefault(frame["agent"], AgentState(name=frame["agent"], parent=None))
            state.assistant += frame.get("text") or ""
        elif kind == "event":
            event = frame.get("event") or {}
            state = self.agents.setdefault(frame["agent"], AgentState(name=frame["agent"], parent=None))
            if event.get("kind") == "say" and event.get("text"):
                state.says.append(event["text"])
                if event.get("from"):  # a child's say is delivered on the parent's stream
                    self.agents.setdefault(event["from"], AgentState(name=event["from"], parent=frame["agent"])).says.append(event["text"])
            elif event.get("kind") == "assistant" and event.get("text"):
                if _squash(event["text"]) not in _squash(state.assistant):
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
            kind = frame.get("type")
            if kind == "__disconnected__":
                queue.put_nowait(DISCONNECTED)
                return
            if frame.get("agent") != agent:
                return
            if kind == "turn":
                if frame.get("state") == "running":
                    started = True
                elif started:
                    queue.put_nowait(None)
            elif kind == "assistant_delta" and frame.get("text"):
                queue.put_nowait(frame["text"])
            elif kind == "event":
                event = frame.get("event") or {}
                if event.get("kind") == "assistant" and event.get("text"):
                    queue.put_nowait(event["text"])
                elif event.get("kind") == "say" and event.get("text") and event.get("from"):
                    queue.put_nowait(f"\n[{event['from']}] {event['text']}\n")

        self.listeners.append(listener)
        emitted = ""
        idle = False
        try:
            if not self._connected:
                yield "The Arbos kernel is not reachable right now; I will keep trying to reconnect."
                return
            self.send({"type": "user", "agent": agent, "text": text, "steer": steer, "attachments": []})
            deadline = time.monotonic() + timeout
            while True:
                # After `turn idle` the kernel may still send the whole assistant text once more
                # (or, for a turn without deltas, for the first time): wait a moment for it.
                remaining = 0.4 if idle else deadline - time.monotonic()
                if remaining <= 0:
                    yield "\n(the agent is still working; I will report when it finishes)"
                    return
                try:
                    item = await asyncio.wait_for(queue.get(), remaining)
                except asyncio.TimeoutError:
                    if idle:
                        return
                    yield "\n(the agent is still working; I will report when it finishes)"
                    return
                if item is DISCONNECTED:
                    yield "\n(the link to the Arbos kernel dropped; I will reconnect, ask again in a moment)"
                    return
                if item is None:
                    idle = True
                    continue
                squashed = _squash(item)
                if emitted and len(squashed) > 8 and _squash(emitted).endswith(squashed):
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

