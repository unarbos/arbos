"""Tools the voice model can call, and the bridge that runs them against the Arbos kernel.

The same schemas serve both engines: the duplex model gets them through its
`session.update.tools`; the pipeline's OpenRouter hop gets them as OpenAI
function tools. Every tool returns a short, ASCII, speakable string.
"""

from __future__ import annotations

import asyncio
import logging
import re
import time
from typing import Awaitable, Callable

from .kernel import KernelClient
from .tts import speakable

log = logging.getLogger("voice.tools")

ReportHook = Callable[[str, str], Awaitable[None]]

TOOLS: list[dict] = [
    {
        "name": "send_agent",
        "description": (
            "Send an Arbos agent to do a task in the background: write or fix code, research something, "
            "run a command, create a file, anything that takes real work. Returns right away with what was "
            "dispatched. When the agent finishes, its report is spoken to the user automatically."
        ),
        "ack_messages": ["Sure, sending an agent to do that.", "On it, dispatching an agent now."],
        "parameters": {
            "type": "object",
            "properties": {
                "task": {"type": "string", "description": "The task, in full, as the user asked for it"}
            },
            "required": ["task"],
        },
    },
    {
        "name": "agent_status",
        "description": "What the Arbos agents are doing right now and their latest reports. Use when the user asks how it is going or what happened.",
        "ack_messages": ["Let me check on the agents."],
        "parameters": {"type": "object", "properties": {}},
    },
    {
        "name": "ask_arbos",
        "description": (
            "Ask the main Arbos agent a question about the project, the code, or the current work and wait for its "
            "answer. Use for questions that need project knowledge, not for general knowledge."
        ),
        "ack_messages": ["Let me ask Arbos.", "One moment, checking with the main agent."],
        "parameters": {
            "type": "object",
            "properties": {"question": {"type": "string", "description": "The question as the user asked it"}},
            "required": ["question"],
        },
    },
]


def openai_tools() -> list[dict]:
    """The same tools in OpenAI chat-completions shape (for OpenRouter)."""
    return [
        {"type": "function", "function": {k: v for k, v in t.items() if k in ("name", "description", "parameters")}}
        for t in TOOLS
    ]


def _clip(text: str, limit: int = 600) -> str:
    text = speakable(text).replace("\n", " ")
    text = re.sub(r"\s+", " ", text).strip()
    text = text.encode("ascii", "ignore").decode()
    return text if len(text) <= limit else text[: limit - 3].rsplit(" ", 1)[0] + "..."


class ToolRunner:
    """One per session. `on_report(agent, text)` fires when a dispatched agent finishes;
    `on_call(name, args)` / `on_result(name, output)` mirror tool activity to the client."""

    def __init__(self, kernel: KernelClient | None, on_report: ReportHook | None = None):
        self.kernel = kernel
        self.on_report = on_report
        self.on_call: Callable[[str, dict], Awaitable[None]] | None = None
        self.on_result: Callable[[str, str], Awaitable[None]] | None = None
        self.watching: set[str] = set()  # kept for introspection; reporting lives in the session now

    def close(self) -> None:
        return None

    async def run(self, name: str, args: dict) -> str:
        if self.on_call:
            await self.on_call(name, args)
        result = await self._run(name, args)
        if self.on_result:
            await self.on_result(name, result)
        return result

    async def _run(self, name: str, args: dict) -> str:
        started = time.monotonic()
        try:
            if name == "send_agent":
                result = await self._send_agent(str(args.get("task", "")).strip())
            elif name == "agent_status":
                result = self._need_kernel() or self.kernel.status_text()
            elif name == "ask_arbos":
                result = await self._ask(str(args.get("question", "")).strip())
            else:
                result = f"Unknown tool {name}."
        except Exception as exc:
            log.exception("tool %s failed", name)
            result = f"The tool failed: {_clip(str(exc), 200)}"
        log.info("tool %s(%s) -> %r in %.1fs", name, _clip(str(args), 80), result[:120], time.monotonic() - started)
        return _clip(result)

    def _need_kernel(self) -> str | None:
        if self.kernel is None or not self.kernel.connected:
            return "The Arbos kernel is not connected, so I cannot reach the agents right now."
        return None

    async def _send_agent(self, task: str) -> str:
        if not task:
            return "I need to know what the agent should do."
        if err := self._need_kernel():
            return err
        before = set(self.kernel.agents)
        prompt = (
            "Spawn one sub-agent for the task below and then reply with one short sentence saying what you sent "
            "off. Do not do the task yourself and do not wait for the sub-agent.\n\nTask: " + task
        )
        reply = ""
        async for delta in self.kernel.turn(prompt, timeout=60):
            reply += delta
        await asyncio.sleep(0.2)
        new = [a for a in self.kernel.agents.values() if a.name not in before and a.name != "root"]
        for child in new:
            self.watching.add(child.name)
        first_sentence = re.split(r"(?<=[.!?])\s", reply.strip(), maxsplit=1)[0]
        if new:
            return first_sentence or f"Dispatched agent {new[0].name}."
        return first_sentence or "The main agent did not start a sub-agent."

    async def _ask(self, question: str) -> str:
        if not question:
            return "What should I ask?"
        if err := self._need_kernel():
            return err
        prompt = (
            "Answer for a voice assistant: two or three plain sentences, no markdown, no lists. "
            "If this needs real work, say so instead of starting it.\n\n" + question
        )
        reply = ""
        async for delta in self.kernel.turn(prompt, timeout=45):
            reply += delta
        return reply.strip() or "Arbos did not answer."

    async def _watch_children(self, frame: dict) -> None:
        if frame.get("type") != "turn" or frame.get("state") != "idle":
            return
        name = frame.get("agent")
        if name not in self.watching:
            return
        self.watching.discard(name)
        await self._report(name)

    async def _report(self, name: str) -> None:
        state = self.kernel.agents.get(name) if self.kernel else None
        if state is None:
            return
        report = state.says[-1] if state.says else state.assistant.strip()
        text = _clip(report or "finished without a report", 500)
        log.info("agent %s finished: %s", name, text[:120])
        if self.on_report:
            await self.on_report(name, text)
