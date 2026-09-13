"""A stand-in for `arbos-kernel serve`: the attach wire over loopback TCP, and a real `.arbos/` on disk.

It behaves like the kernel from the gateway's point of view: `hello`, `snapshot`, `tree` frames;
`user` frames become inbox files (with `channel`) and `user` transcript lines; `read`/`tail`/`list`
are served from the place folder. What the "agents" then do is scripted per utterance: stream a
reply, spawn a child that finishes after N seconds with given last words and a big tool output,
write the child's `done` file to the parent and wake it, or ask the user a question. No model.

A real kernel can replace it (`run.py --kernel tcp://...`); the assertions on files stay the same.
"""

from __future__ import annotations

import asyncio
import json
import logging
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

log = logging.getLogger("mock.kernel")


@dataclass
class Child:
    name: str
    done_after: float = 2.0
    last_words: str = "Done."
    ok: bool = True
    tool_output_lines: int = 0  # a `tool` event with this many lines of body on the child's transcript
    tool_name: str = "bash"


@dataclass
class Behaviour:
    """What the main agent does with one utterance."""

    reply: str = ""  # streamed, then the whole text as an `assistant` event
    reply_delay: float = 0.3  # thinking time before the first token
    spawn: Child | None = None
    reply_after_done: str = ""  # the parent's turn when the child's `done` file wakes it
    ask: str = ""  # question for the user (an `ask` frame); the answer ends the turn
    ask_options: list[str] = field(default_factory=list)
    tool_output_lines: int = 0  # a big tool body on the root transcript during the turn
    steer_reply: str = ""  # what the running turn says when steered


class MockKernel:
    def __init__(self, place: Path, host: str = "127.0.0.1", port: int = 0):
        self.place = place
        self.arbos = place / ".arbos"
        self.host, self.port = host, port
        self.script: list[Behaviour] = []
        self.users: list[dict] = []  # every `user` frame received
        self.answers: list[dict] = []
        self.running: set[str] = set()
        self.agents: dict[str, str | None] = {"root": None}  # id -> parent
        self._clients: list[asyncio.StreamWriter] = []
        self._server: asyncio.AbstractServer | None = None
        self._turns = 0
        self._tasks: list[asyncio.Task] = []
        self._pending_ask: dict | None = None
        self._answer_waiter: asyncio.Future | None = None
        self._seq = 0
        self.events_sent: list[dict] = []

    # ------------------------------------------------------------------ lifecycle

    async def start(self) -> str:
        self._bootstrap()
        self._server = await asyncio.start_server(self._client, self.host, self.port)
        self.port = self._server.sockets[0].getsockname()[1]
        url = f"tcp://{self.host}:{self.port}"
        info = json.dumps({"url": url, "pid": 0})
        (self.arbos / "kernel.json").write_text(info)
        (self.arbos / "runtime").mkdir(exist_ok=True)
        (self.arbos / "runtime" / "kernel.json").write_text(info)
        self._tasks.append(asyncio.create_task(self._ticker()))
        return url

    async def stop(self) -> None:
        for t in self._tasks:
            t.cancel()
        if self._server:
            self._server.close()
            await self._server.wait_closed()

    def _bootstrap(self) -> None:
        agent = self.arbos / "agents" / "root"
        (agent / "inbox").mkdir(parents=True, exist_ok=True)
        (agent / "turns").mkdir(exist_ok=True)
        if not (agent / "agent.md").exists():
            (agent / "agent.md").write_text("name: Main\nmodel: mock\n")
        (agent / "transcript.jsonl").touch()
        (self.arbos / "focus").write_text("agents/root")
        (self.arbos / "GOALS.md").write_text("# Goals\n\nKeep CI green.\n")

    # ------------------------------------------------------------------ wire

    async def _client(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        self._clients.append(writer)
        self._send_to(writer, {"type": "hello", "protocol": 1, "kernel": "mock-0.2.0", "tail": 200, "focus": "root"})
        self._send_to(writer, {"type": "snapshot", "tree": self._tree(), "focus": "agents/root", "budget": None})
        self._send_to(writer, {"type": "history_end", "agent": "root", "from": 0, "to": 0, "total": self._lines("root")})
        try:
            while True:
                line = await reader.readline()
                if not line:
                    break
                try:
                    frame = json.loads(line)
                except json.JSONDecodeError:
                    continue
                await self._on_frame(frame, writer)
        except (ConnectionError, asyncio.IncompleteReadError):
            pass
        finally:
            if writer in self._clients:
                self._clients.remove(writer)
            writer.close()

    def _send_to(self, writer: asyncio.StreamWriter, frame: dict) -> None:
        if writer.is_closing():
            return
        try:
            writer.write((json.dumps(frame) + "\n").encode())
        except Exception:
            pass

    def broadcast(self, frame: dict) -> None:
        self.events_sent.append(frame)
        for w in list(self._clients):
            self._send_to(w, frame)

    async def _ticker(self) -> None:
        while True:
            await asyncio.sleep(5)
            self.broadcast({"type": "tree", "tree": self._tree()})

    def _tree(self) -> list[dict]:
        return [
            {"id": a, "name": a if a != "root" else "Main", "parent": p, "paused": False, "model": "mock", "kind": "agent", "mode": "auto"}
            for a, p in self.agents.items()
        ]

    async def _on_frame(self, frame: dict, writer: asyncio.StreamWriter) -> None:
        kind = frame.get("type")
        if kind == "user":
            self.users.append(frame)
            agent = frame.get("agent", "root")
            text = str(frame.get("text", ""))
            # The kernel's rule: a user frame without a channel is a typed line.
            channel = str(frame.get("channel") or "text")
            device = str(frame.get("device") or "")
            steer = bool(frame.get("steer")) and agent in self.running
            self._inbox(agent, "user", "steer" if steer else "request", text, channel=channel, device=device)
            ev = {"kind": "user", "text": text, "attachments": [], "channel": channel}
            if device:
                ev["device"] = device
            self._append(agent, ev)
            self.broadcast({"type": "event", "agent": agent, "event": {"seq": self._lines(agent), "ts": now_ms(), **ev}})
            if steer:
                b = self.script[self._turns - 1] if 0 < self._turns <= len(self.script) else Behaviour()
                if b.steer_reply:
                    self._tasks.append(asyncio.create_task(self._say(agent, b.steer_reply, end_turn=False)))
                return
            n = self._turns
            self._turns += 1
            b = self.script[n] if n < len(self.script) else Behaviour(reply="Noted.")
            self._tasks.append(asyncio.create_task(self._run_turn(agent, b)))
        elif kind == "answer":
            self.answers.append(frame)
            self._append("root", {"kind": "answer", "text": frame.get("text", "")})
            if self._answer_waiter and not self._answer_waiter.done():
                self._answer_waiter.set_result(str(frame.get("text", "")))
        elif kind == "approve":
            pass
        elif kind in ("read", "tail", "list"):
            self._send_to(writer, self._file(frame))
        elif kind == "history":
            self._send_to(writer, {"type": "history_end", "agent": frame.get("agent", "root"), "from": 0, "to": 0, "total": 0})

    # ------------------------------------------------------------------ files

    def _file(self, frame: dict) -> dict:
        rel = str(frame.get("path", "")).strip("/")
        path = (self.arbos / rel) if rel else self.arbos
        try:
            path.resolve().relative_to(self.arbos.resolve())
        except ValueError:
            return {"type": {"read": "file", "tail": "chunk", "list": "listing"}[frame["type"]], "path": rel, "error": "outside .arbos"}
        if frame["type"] == "list":
            if not path.is_dir():
                return {"type": "listing", "path": rel, "entries": [], "error": "not a folder"}
            entries = [
                {"name": p.name, "dir": p.is_dir(), "size": p.stat().st_size if p.is_file() else 0, "modified": int(p.stat().st_mtime * 1000)}
                for p in sorted(path.iterdir())
            ]
            return {"type": "listing", "path": rel, "entries": entries}
        if not path.is_file():
            return {"type": "file" if frame["type"] == "read" else "chunk", "path": rel, "error": "no such file", "size": 0, "text": ""}
        data = path.read_bytes()
        if frame["type"] == "read":
            cap = 256 * 1024
            return {"type": "file", "path": rel, "text": data[:cap].decode(errors="replace"), "size": len(data), "truncated": len(data) > cap}
        start = int(frame.get("from") or 0)
        limit = int(frame.get("limit") or 65536)
        chunk = data[start : start + limit]
        if start > 0 and chunk and data[start - 1 : start] != b"\n":
            nl = chunk.find(b"\n")
            if nl >= 0:
                chunk = chunk[nl + 1 :]
                start = start + nl + 1
        end = start + len(chunk)
        if end < len(data):
            last = chunk.rfind(b"\n")
            if last >= 0:
                chunk = chunk[: last + 1]
                end = start + len(chunk)
        return {"type": "chunk", "path": rel, "from": start, "to": end, "size": len(data), "text": chunk.decode(errors="replace")}

    def _inbox(self, agent: str, sender: str, kind: str, body: str, *, channel: str = "", device: str = "", wake: bool = True) -> str:
        """One inbox file, in the kernel's `Message::render` shape (TOML front matter between +++)."""
        d = self.arbos / "agents" / agent / "inbox"
        d.mkdir(parents=True, exist_ok=True)
        stamp = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H-%M-%SZ")
        who = sender.replace(":", "-").replace("/", "-")
        for seq in range(1000):
            name = f"{stamp}-{who}-{seq:03d}.md"
            if not (d / name).exists():
                break
        front = [f'from = "{sender}"', f'kind = "{kind}"', f"wake = {'true' if wake else 'false'}", "hops = 0",
                 f'sent = "{datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")}"']
        if channel:
            front.append(f'channel = "{channel}"')
        if device:
            front.append(f'device = "{device}"')
        text = "+++\n" + "\n".join(front) + "\n+++\n" + body.rstrip("\n") + "\n"
        tmp = d / f".{name}.tmp"
        tmp.write_text(text)
        tmp.rename(d / name)
        return name

    def _append(self, agent: str, event: dict) -> int:
        path = self.arbos / "agents" / agent / "transcript.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        event = {"ts": now_ms(), **event}
        with path.open("a") as fh:
            fh.write(json.dumps(event) + "\n")
        # The kernel tells attached clients that a watched file moved; the desktop re-reads it.
        self.broadcast({"type": "changed", "path": f"agents/{agent}/transcript.jsonl", "kind": "modified", "size": path.stat().st_size})
        return self._lines(agent)

    def _lines(self, agent: str) -> int:
        path = self.arbos / "agents" / agent / "transcript.jsonl"
        if not path.exists():
            return 0
        return sum(1 for _ in path.open())

    def inbox_files(self, agent: str = "root") -> list[dict]:
        """Every inbox file of an agent, parsed: `{name, from, kind, channel, wake, body}`, oldest first.
        The mock never consumes files (a real kernel renames them into turns/), so the record stays."""
        d = self.arbos / "agents" / agent / "inbox"
        out = []
        for p in sorted(d.glob("*.md")) if d.exists() else []:
            text = p.read_text()
            if not text.startswith("+++\n"):
                continue
            front, _, body = text[4:].partition("\n+++\n")
            fields: dict = {"name": p.name, "body": body.strip()}
            for line in front.splitlines():
                k, _, v = line.partition(" = ")
                fields[k.strip()] = v.strip().strip('"')
            out.append(fields)
        return out

    # ------------------------------------------------------------------ scripted agents

    async def _run_turn(self, agent: str, b: Behaviour) -> None:
        self.running.add(agent)
        self.broadcast({"type": "turn", "agent": agent, "state": "running", "budget": None})
        await asyncio.sleep(b.reply_delay)
        if b.tool_output_lines:
            self._tool(agent, "bash", b.tool_output_lines)
        child: Child | None = b.spawn
        if child is not None:
            self._spawn(agent, child)
        if b.ask:
            await self._ask(agent, b.ask, b.ask_options)
        if b.reply:
            await self._say(agent, b.reply)
        else:
            self._end_turn(agent)
        if child is not None:
            await asyncio.sleep(child.done_after)
            await self._finish_child(agent, child, b.reply_after_done)

    def _tool(self, agent: str, name: str, lines: int) -> None:
        body = "\n".join(
            f"[{i:04d}] test_skeptic.py::test_timeout ... {'FAILED' if i == lines - 3 else 'ok'}  (assert timeout == 30, got 60)"
            if i == lines - 3 else f"[{i:04d}] tests/test_module_{i % 17}.py::test_case_{i} ... ok"
            for i in range(lines)
        )
        seq = self._append(agent, {"kind": "tool", "name": name, "call_id": f"c{self._next()}", "body": body, "result_size": len(body), "args": {"cmd": "pytest -q"}})
        self.broadcast({"type": "event", "agent": agent, "event": {"seq": seq, "ts": now_ms(), "kind": "tool", "name": name, "call_id": f"c{seq}", "result_size": len(body)}})

    def _spawn(self, parent: str, child: Child) -> None:
        self.agents[child.name] = parent
        d = self.arbos / "agents" / child.name
        (d / "inbox").mkdir(parents=True, exist_ok=True)
        (d / "agent.md").write_text(f"name: {child.name}\nparent: {parent}\nmodel: mock\n")
        self._append(child.name, {"kind": "user", "text": f"You were spawned by agent {parent}.", "attachments": []})
        seq = self._append(parent, {"kind": "tool", "name": "spawn", "call_id": f"c{self._next()}", "child": child.name, "args": {"brief": "..."}})
        self.broadcast({"type": "event", "agent": parent, "event": {"seq": seq, "ts": now_ms(), "kind": "tool", "name": "spawn", "call_id": f"c{seq}", "child": child.name}})
        self.broadcast({"type": "tree", "tree": self._tree()})
        self.running.add(child.name)
        self.broadcast({"type": "turn", "agent": child.name, "state": "running", "budget": None})

    async def _say(self, agent: str, text: str, *, end_turn: bool = True) -> None:
        words = text.split(" ")
        for i, w in enumerate(words):
            self.broadcast({"type": "assistant_delta", "agent": agent, "text": w + (" " if i < len(words) - 1 else "")})
            await asyncio.sleep(0.02)
        seq = self._append(agent, {"kind": "assistant", "text": text})
        if end_turn:
            self._end_turn(agent, final=None)
        # The kernel sends `turn idle` first and the whole reply as an event after it.
        await asyncio.sleep(0.05)
        self.broadcast({"type": "event", "agent": agent, "event": {"seq": seq, "ts": now_ms(), "kind": "assistant", "text": text}})

    def _end_turn(self, agent: str, final: str | None = None) -> None:
        self._append(agent, {"kind": "turn_complete", "usage": {"used": 1000, "size": 128000}})
        self.running.discard(agent)
        self.broadcast({"type": "turn", "agent": agent, "state": "idle", "budget": {"used": 1000, "size": 128000}})

    async def _ask(self, agent: str, question: str, options: list[str]) -> None:
        self._append(agent, {"kind": "ask", "question": question, "options": options, "call_id": "ask-1"})
        self._answer_waiter = asyncio.get_running_loop().create_future()
        self.broadcast({"type": "ask", "agent": agent, "question": question, "options": options, "id": "ask-1"})
        try:
            await asyncio.wait_for(self._answer_waiter, 60)
        except asyncio.TimeoutError:
            pass

    async def _finish_child(self, parent: str, child: Child, reply_after_done: str) -> None:
        if child.tool_output_lines:
            self._tool(child.name, child.tool_name, child.tool_output_lines)
        seq = self._append(child.name, {"kind": "assistant", "text": child.last_words})
        self._append(child.name, {"kind": "turn_complete"})
        self.running.discard(child.name)
        self.broadcast({"type": "turn", "agent": child.name, "state": "idle", "budget": None})
        self.broadcast({"type": "event", "agent": child.name, "event": {"seq": seq, "ts": now_ms(), "kind": "assistant", "text": child.last_words}})
        # The kernel's `done` file to the parent (#98), then the parent wakes on it: the file is
        # claimed and its words land on the parent's transcript as a `say`.
        status = "ended" if child.ok else "ended badly"
        body = f"Turn {status}. Last words: {child.last_words}\n(transcript: .arbos/agents/{child.name}/transcript.jsonl)"
        self._inbox(parent, f"agent:{child.name}", "done", body)
        await asyncio.sleep(0.2)
        seq = self._append(parent, {"kind": "say", "from": child.name, "text": body})
        self.broadcast({"type": "event", "agent": parent, "event": {"seq": seq, "ts": now_ms(), "kind": "say", "from": child.name, "text": body}})
        if reply_after_done:
            self.running.add(parent)
            self.broadcast({"type": "turn", "agent": parent, "state": "running", "budget": None})
            await asyncio.sleep(0.4)
            await self._say(parent, reply_after_done)

    def _next(self) -> int:
        self._seq += 1
        return self._seq


def now_ms() -> int:
    return int(time.time() * 1000)
