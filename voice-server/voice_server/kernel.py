"""Client for `arbos-kernel serve`: newline-delimited JSON over loopback TCP, or the same frames
over a WebSocket (a kernel bound with `--bind`, or the hub's `/attach/<machine>/<project>`).

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


def hub_attach_url(hub: str, project: str, token: str = "") -> str:
    """`wss://<hub>/attach/<machine>/<project>?token=…` for a hub name `<machine>/<project>`
    (`<machine>` alone when the machine runs one kernel)."""
    base = hub.rstrip("/")
    if base.startswith("http://"):
        base = "ws://" + base[len("http://"):]
    elif base.startswith("https://"):
        base = "wss://" + base[len("https://"):]
    url = f"{base}/attach/{project.strip('/')}"
    return f"{url}?token={token}" if token else url


class KernelClient:
    def __init__(self, url: str | None = None, place: str | None = None, *, auto_approve: bool = False,
                 token: str = "", name: str = "", reconnect: bool = True):
        if not url and not place:
            raise ValueError("need a kernel url or place")
        self.url = url
        self.place = place
        # Never on by default: an `allow …` ask is a person's decision. The narrator speaks it and the
        # caller answers; `--auto-approve` opts the gateway's own kernel in, never a hub attach.
        self.auto_approve = auto_approve
        self.token = token  # bearer for a WebSocket kernel or hub attach
        self.name = name or (url or place or "kernel")
        self.reader: asyncio.StreamReader | None = None
        self.writer: asyncio.StreamWriter | None = None
        self.ws: websockets.ClientConnection | None = None
        self._closed = False
        # A kernel restart or a dropped tunnel is not the end of a call: the link comes back with
        # backoff, listeners hear `link {state: lost | restored}`, and frames sent meanwhile wait.
        self.reconnect = reconnect
        self._reconnect_task: asyncio.Task | None = None
        self._outbox: list[dict] = []
        self.link_lost = 0  # how many times the link dropped
        self._opening = False
        self.agents: dict[str, AgentState] = {}
        self.focus = "root"
        self.listeners: list[Listener] = []
        self._reader_task: asyncio.Task | None = None
        # Answers to `read`/`tail`/`list`, keyed by (reply type, path); one waiter per key.
        self._waiting: dict[tuple[str, str], asyncio.Future] = {}
        self.hello: dict = {}
        self._first_error: str = ""

    # ------------------------------------------------------------------ connection

    def _resolve(self) -> tuple[str, int]:
        url = self.url
        if not url:
            info = json.loads((Path(self.place) / ".arbos" / "kernel.json").read_text())
            url = info["url"]
        host, port = url.removeprefix("tcp://").rsplit(":", 1)
        return host, int(port)

    @property
    def over_websocket(self) -> bool:
        return bool(self.url and self.url.startswith(("ws://", "wss://")))

    async def connect(self, *, hello_timeout: float = 15.0) -> None:
        self._closed = False
        await self._open(hello_timeout=hello_timeout)

    async def _open(self, *, hello_timeout: float = 15.0) -> None:
        self.hello = {}
        self._first_error = ""
        self._opening = True
        try:
            await self._open_transport(hello_timeout=hello_timeout)
        finally:
            self._opening = False

    async def _open_transport(self, *, hello_timeout: float) -> None:
        if self.over_websocket:
            headers = {"Authorization": f"Bearer {self.token}"} if self.token and "token=" not in self.url else {}
            self.ws = await asyncio.wait_for(
                websockets.connect(self.url, additional_headers=headers, max_size=16 * 1024 * 1024, compression=None),
                hello_timeout,
            )
            self._reader_task = asyncio.create_task(self._ws_read_loop(), name="kernel-ws-read")
            # The first frame back is `hello` (or the hub's `error` for a bad name).
            deadline = time.monotonic() + hello_timeout
            while not self.hello and time.monotonic() < deadline:
                if self._first_error:
                    await self._drop_transport()
                    raise RuntimeError(self._first_error)
                if self.ws is None:
                    raise RuntimeError("the kernel closed the connection before hello")
                await asyncio.sleep(0.05)
            if not self.hello:
                await self._drop_transport()
                raise RuntimeError(f"no hello from {self.url.split('?')[0]} in {hello_timeout:.0f}s")
            log.info("attached to kernel over %s", self.url.split("?")[0])
            return
        host, port = self._resolve()
        self.reader, self.writer = await asyncio.open_connection(host, port)
        self._reader_task = asyncio.create_task(self._read_loop(), name="kernel-read")
        log.info("attached to kernel at %s:%d", host, port)

    async def _drop_transport(self) -> None:
        if self._reader_task and self._reader_task is not asyncio.current_task():
            self._reader_task.cancel()
        self._reader_task = None
        if self.writer:
            self.writer.close()
        self.writer = None
        if self.ws is not None:
            try:
                await asyncio.wait_for(self.ws.close(), 3)
            except Exception:
                pass
            self.ws = None

    def _on_link_lost(self) -> None:
        """The read loop ended without `close()`: tell the listeners, then bring the link back."""
        if self._closed or not self.reconnect or self._opening:
            return
        if self._reconnect_task and not self._reconnect_task.done():
            return
        self.link_lost += 1
        self._notify({"type": "link", "state": "lost", "kernel": self.name})
        self._reconnect_task = asyncio.create_task(self._reconnect_loop(), name=f"kernel-reconnect-{self.name}")

    async def _reconnect_loop(self) -> None:
        delay = 1.0
        attempt = 0
        while not self._closed:
            attempt += 1
            await asyncio.sleep(delay)
            if self._closed:
                return
            try:
                await self._open(hello_timeout=10.0)
            except Exception as exc:
                log.warning("kernel %s: reconnect %d failed (%s); next in %.0fs", self.name, attempt, str(exc)[:80], min(delay * 2, 30))
                delay = min(delay * 2, 30.0)
                continue
            log.info("kernel %s: link restored after %d attempt(s)", self.name, attempt)
            self._notify({"type": "link", "state": "restored", "kernel": self.name, "attempts": attempt})
            outbox, self._outbox = self._outbox, []
            for frame in outbox:
                try:
                    self.send(frame)
                except Exception:
                    self._outbox.append(frame)
            return

    def _notify(self, frame: dict) -> None:
        for listener in list(self.listeners):
            try:
                result = listener(frame)
                if asyncio.iscoroutine(result):
                    asyncio.get_running_loop().create_task(result)
            except Exception:
                log.exception("kernel listener failed")

    @property
    def connected(self) -> bool:
        if self._closed:
            return False
        if self.over_websocket:
            return self.ws is not None
        return self.writer is not None and not self.writer.is_closing()

    @property
    def reconnecting(self) -> bool:
        return bool(self._reconnect_task and not self._reconnect_task.done())

    async def close(self) -> None:
        self._closed = True
        if self._reconnect_task:
            self._reconnect_task.cancel()
        await self._drop_transport()

    def send(self, frame: dict) -> None:
        """One frame to the kernel. While the link is being brought back, the frame waits and goes
        out on restore (a caller's words survive a kernel restart)."""
        if self._closed:
            raise RuntimeError("kernel client closed")
        if not self.connected:
            if self.reconnect and len(self._outbox) < 64:
                self._outbox.append(frame)
                return
            raise RuntimeError("kernel not connected")
        if self.over_websocket:
            asyncio.get_running_loop().create_task(self._ws_send(json.dumps(frame)))
            return
        self.writer.write((json.dumps(frame) + "\n").encode())

    async def _ws_send(self, text: str) -> None:
        try:
            if self.ws is not None:
                await self.ws.send(text)
        except Exception as exc:
            log.warning("kernel ws send failed: %s", exc)

    async def _dispatch(self, frame: dict) -> None:
        self._track(frame)
        for listener in list(self.listeners):
            try:
                result = listener(frame)
                if asyncio.iscoroutine(result):
                    await result
            except Exception:
                log.exception("kernel listener failed")

    async def _ws_read_loop(self) -> None:
        assert self.ws
        try:
            async for message in self.ws:
                text = message if isinstance(message, str) else bytes(message).decode(errors="replace")
                for line in text.splitlines():
                    line = line.strip()
                    if not line:
                        continue
                    try:
                        frame = json.loads(line)
                    except json.JSONDecodeError:
                        continue
                    if not self.hello and frame.get("type") == "error":
                        self._first_error = str(frame.get("detail") or "kernel refused the attach")
                    await self._dispatch(frame)
        except Exception as exc:
            log.warning("kernel ws closed: %s", exc)
        finally:
            log.warning("kernel connection closed (%s)", self.name)
            self.ws = None
            self._on_link_lost()

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
                await self._dispatch(frame)
        finally:
            log.warning("kernel connection closed (%s)", self.name)
            if self.writer:
                self.writer.close()
            self.writer = None
            self._on_link_lost()

    def _track(self, frame: dict) -> None:
        kind = frame.get("type")
        if kind in ("file", "chunk", "listing"):
            fut = self._waiting.pop((kind, str(frame.get("path", ""))), None)
            if fut is not None and not fut.done():
                fut.set_result(frame)
            return
        if kind == "hello":
            self.hello = frame
            return
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
            budget = frame.get("budget") or {}
            if frame.get("state") == "idle" and budget.get("cost") is not None:
                log.info("turn cost %s: $%.5f (%s tokens in context)", frame["agent"], float(budget["cost"]), budget.get("used"))
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

    # ------------------------------------------------------------------ files (Read/Tail/List frames)

    async def _ask_file(self, request: dict, reply_type: str, timeout: float) -> dict:
        key = (reply_type, str(request.get("path", "")))
        fut: asyncio.Future = asyncio.get_running_loop().create_future()
        self._waiting[key] = fut
        try:
            self.send(request)
            return await asyncio.wait_for(fut, timeout)
        except asyncio.TimeoutError:
            return {"type": reply_type, "path": key[1], "error": f"no answer from the kernel in {timeout:.0f}s"}
        finally:
            self._waiting.pop(key, None)

    async def read(self, path: str, *, timeout: float = 10.0) -> dict:
        """One file under `.arbos/` as text: `{path, text, size, truncated?, error?}`."""
        return await self._ask_file({"type": "read", "path": path}, "file", timeout)

    async def tail(self, path: str, *, from_: int = 0, limit: int = 65536, timeout: float = 10.0) -> dict:
        """Bytes `from_..from_+limit` of a file, cut to a line boundary: `{path, from, to, size, text, error?}`."""
        return await self._ask_file({"type": "tail", "path": path, "from": from_, "limit": limit}, "chunk", timeout)

    async def list(self, path: str = "", *, timeout: float = 10.0) -> dict:
        """Entries of a folder under `.arbos/`: `{path, entries: [{name, dir, size, modified}], error?}`."""
        return await self._ask_file({"type": "list", "path": path}, "listing", timeout)

    async def transcript_tail(self, agent: str, *, bytes_: int = 200_000) -> list[dict]:
        """The last events of an agent's transcript, parsed. Uses `tail` so a long log costs one read."""
        path = f"agents/{agent}/transcript.jsonl"
        probe = await self.tail(path, from_=0, limit=1)
        size = int(probe.get("size") or 0)
        if probe.get("error") and not size:
            return []
        start = max(0, size - bytes_)
        chunk = await self.tail(path, from_=start, limit=bytes_)
        events: list[dict] = []
        for line in str(chunk.get("text") or "").splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError:
                continue
        return events

    # ------------------------------------------------------------------ turns

    def send_user(self, text: str, agent: str = "root", *, channel: str = "", device: str = "",
                  steer: bool | None = None) -> None:
        """One user message to an agent, and back to whatever else you were doing. `channel` says
        where the words came from (`voice` | `text`) and `device` which client carried them
        (`phone` | `desktop`); the kernel writes both into the inbox file and the transcript line.
        `steer` defaults to "the agent is running now"."""
        if steer is None:
            state = self.agents.get(agent)
            steer = bool(state and state.running)
        frame: dict = {"type": "user", "agent": agent, "text": text, "steer": steer, "attachments": []}
        if channel:
            frame["channel"] = channel
        if device:
            frame["device"] = device
        self.send(frame)

    async def turn(self, text: str, agent: str = "root", *, steer: bool = False, timeout: float = 120.0,
                   channel: str = "") -> AsyncIterator[str]:
        """Send one user turn and yield the agent's assistant text deltas until it goes idle."""
        queue: asyncio.Queue[str | None] = asyncio.Queue()
        started = False

        def listener(frame: dict) -> None:
            nonlocal started
            kind = frame.get("type")
            if kind == "link" and frame.get("state") == "lost":
                queue.put_nowait(DISCONNECTED)
                return
            if frame.get("agent") != agent:
                return
            if kind == "turn":
                if frame.get("state") == "running":
                    started = True
                elif started:
                    queue.put_nowait(None)
            elif kind == "assistant_delta" and frame.get("text") and started:
                queue.put_nowait(frame["text"])  # only once *our* turn is running: an earlier turn
                # still finishing (or being stopped) must not leak into this answer
            elif kind == "event" and started:
                event = frame.get("event") or {}
                if event.get("kind") == "assistant" and event.get("text"):
                    queue.put_nowait(event["text"])
                elif event.get("kind") == "say" and event.get("text") and event.get("from"):
                    queue.put_nowait(f"\n[{event['from']}] {event['text']}\n")

        self.listeners.append(listener)
        emitted = ""
        idle = False
        try:
            self.send_user(text, agent, channel=channel, steer=steer)
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

