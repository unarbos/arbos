"""Who answers text turns. Pluggable.

- `none`: nobody; the client drives replies with `speak` (speech-only mode).
- `openrouter`: any OpenRouter model, with the Arbos tools (function calling).
- `kernel`: the Arbos kernel's main agent answers directly (text.input goes to it).

To add a backend, implement `ReplyBackend.stream` and register it in `build_reply`.
"""

from __future__ import annotations

import json
import logging
import os
from typing import AsyncIterator, Protocol

import httpx

from .kernel import KernelClient
from .tools import ToolRunner, openai_tools

log = logging.getLogger("voice.reply")

DEFAULT_SYSTEM = (
    "You are Arbos, speaking out loud in a phone call. Answer in one to three short sentences. "
    "No markdown, no lists, no code. Be direct and human. When the user asks you to do work, send an "
    "agent with the send_agent tool instead of doing it yourself; when they ask how it is going, use "
    "agent_status; for project questions use ask_arbos."
)


class ReplyBackend(Protocol):
    name: str

    def stream(self, history: list[dict], tools: ToolRunner | None) -> AsyncIterator[str]:
        """Yield reply text deltas. `history` is a list of OpenAI-style messages; the backend may
        append tool call/result messages to it."""
        ...


class OpenRouterReply:
    def __init__(self, model: str, api_key: str, system_prompt: str = DEFAULT_SYSTEM):
        self.model = model
        self.api_key = api_key
        self.system_prompt = system_prompt
        self.name = f"openrouter/{model}"
        self.client = httpx.AsyncClient(timeout=httpx.Timeout(90.0, connect=10.0))

    async def stream(self, history: list[dict], tools: ToolRunner | None) -> AsyncIterator[str]:
        for _round in range(4):  # tool calls loop back into the model at most this many times
            calls: dict[int, dict] = {}
            finish = None
            async for kind, payload in self._one_call(history, with_tools=tools is not None):
                if kind == "text":
                    yield payload
                elif kind == "tool":
                    slot = calls.setdefault(payload["index"], {"id": "", "name": "", "arguments": ""})
                    slot["id"] = payload.get("id") or slot["id"]
                    slot["name"] = payload.get("name") or slot["name"]
                    slot["arguments"] += payload.get("arguments") or ""
                elif kind == "finish":
                    finish = payload
            if finish != "tool_calls" or not calls or tools is None:
                return
            history.append({
                "role": "assistant", "content": None,
                "tool_calls": [
                    {"id": c["id"] or f"call_{i}", "type": "function",
                     "function": {"name": c["name"], "arguments": c["arguments"] or "{}"}}
                    for i, c in sorted(calls.items())
                ],
            })
            for i, call in sorted(calls.items()):
                try:
                    args = json.loads(call["arguments"] or "{}")
                except json.JSONDecodeError:
                    args = {}
                result = await tools.run(call["name"], args)
                history.append({"role": "tool", "tool_call_id": call["id"] or f"call_{i}", "content": result})

    async def _one_call(self, history: list[dict], *, with_tools: bool) -> AsyncIterator[tuple[str, object]]:
        body: dict = {
            "model": self.model,
            "stream": True,
            "messages": [{"role": "system", "content": self.system_prompt}, *history],
        }
        if with_tools:
            body["tools"] = openai_tools()
        headers = {
            "Authorization": f"Bearer {self.api_key}",
            "HTTP-Referer": "https://github.com/unarbos/arbos",
            "X-Title": "Arbos voice server",
        }
        async with self.client.stream(
            "POST", "https://openrouter.ai/api/v1/chat/completions", json=body, headers=headers
        ) as resp:
            if resp.status_code != 200:
                detail = (await resp.aread()).decode(errors="replace")[:300]
                raise RuntimeError(f"OpenRouter {resp.status_code}: {detail}")
            async for line in resp.aiter_lines():
                if not line.startswith("data: "):
                    continue
                payload = line[6:].strip()
                if payload == "[DONE]":
                    break
                try:
                    event = json.loads(payload)
                except json.JSONDecodeError:
                    continue
                if event.get("error"):  # OpenRouter reports provider refusals in-band with HTTP 200
                    err = event["error"]
                    raise RuntimeError(f"OpenRouter/{self.model}: {err.get('message', err) if isinstance(err, dict) else err}")
                for choice in event.get("choices", []):
                    delta = choice.get("delta") or {}
                    if delta.get("content"):
                        yield "text", delta["content"]
                    for call in delta.get("tool_calls") or []:
                        fn = call.get("function") or {}
                        yield "tool", {"index": call.get("index", 0), "id": call.get("id"),
                                       "name": fn.get("name"), "arguments": fn.get("arguments")}
                    if choice.get("finish_reason"):
                        yield "finish", choice["finish_reason"]


class KernelReply:
    """Text turns go straight to the kernel's main agent (it has its own tools)."""

    def __init__(self, kernel: KernelClient, agent: str = "root"):
        self.kernel = kernel
        self.agent = agent
        self.name = f"kernel/{agent}"

    async def stream(self, history: list[dict], tools: ToolRunner | None) -> AsyncIterator[str]:
        last = next((m for m in reversed(history) if m.get("role") == "user"), None)
        if last is None:
            return
        async for delta in self.kernel.turn(str(last["content"]), agent=self.agent, timeout=180):
            yield delta


def build_reply(kind: str, *, model: str, kernel: KernelClient | None) -> ReplyBackend | None:
    if kind == "none":
        return None
    if kind == "openrouter":
        key = os.environ.get("OPENROUTER_API_KEY") or os.environ.get("OPENROUTER")
        if not key:
            raise SystemExit("--reply openrouter needs OPENROUTER_API_KEY in the environment")
        return OpenRouterReply(model, key)
    if kind == "kernel":
        if kernel is None:
            raise SystemExit("--reply kernel needs --kernel or --kernel-place")
        return KernelReply(kernel)
    raise ValueError(f"unknown reply backend {kind!r}")
