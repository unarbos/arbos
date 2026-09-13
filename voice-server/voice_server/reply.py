"""Who answers the user. Pluggable; the default is nobody (the client drives with `speak`).

To add a backend (for example the Arbos kernel's attach socket), implement
`ReplyBackend.stream` and register it in `build_reply`.
"""

from __future__ import annotations

import json
import logging
import os
from typing import AsyncIterator, Protocol

import httpx

log = logging.getLogger("voice.reply")

DEFAULT_SYSTEM = (
    "You are Arbos, speaking out loud in a phone call. Answer in one to three short "
    "sentences. No markdown, no lists, no code. Be direct and human."
)


class ReplyBackend(Protocol):
    name: str

    def stream(self, history: list[dict[str, str]]) -> AsyncIterator[str]:
        """Yield reply text deltas for the conversation so far (user/assistant turns)."""
        ...


class OpenRouterReply:
    def __init__(self, model: str, api_key: str, system_prompt: str = DEFAULT_SYSTEM):
        self.model = model
        self.api_key = api_key
        self.system_prompt = system_prompt
        self.name = f"openrouter/{model}"
        self.client = httpx.AsyncClient(timeout=httpx.Timeout(60.0, connect=10.0))

    async def stream(self, history: list[dict[str, str]]) -> AsyncIterator[str]:
        body = {
            "model": self.model,
            "stream": True,
            "messages": [{"role": "system", "content": self.system_prompt}, *history],
        }
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
                for choice in event.get("choices", []):
                    delta = (choice.get("delta") or {}).get("content")
                    if delta:
                        yield delta


def build_reply(kind: str, *, model: str) -> ReplyBackend | None:
    if kind == "none":
        return None
    if kind == "openrouter":
        key = os.environ.get("OPENROUTER_API_KEY") or os.environ.get("OPENROUTER")
        if not key:
            raise SystemExit("--reply openrouter needs OPENROUTER_API_KEY in the environment")
        return OpenRouterReply(model, key)
    raise ValueError(f"unknown reply backend {kind!r}")
