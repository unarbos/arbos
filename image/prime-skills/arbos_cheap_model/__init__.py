"""arbos_cheap_model: D64 Prime skill. Calls the relay model proxy through agentd's loopback stub."""

import os

import httpx

PROXY = os.environ.get("ARBOS_MODEL_PROXY_URL", "http://127.0.0.1:7791/v1")
TOKEN = os.environ.get("ARBOS_MODEL_TOKEN") or os.environ.get("ARBOS_BOX_TOKEN", "")
MODEL = os.environ.get("ARBOS_TALKING_MODEL", "anthropic/claude-haiku-5.5")


async def run(instruction: str, text: str, model: str | None = None) -> str:
    body = {
        "model": model or MODEL,
        "messages": [
            {"role": "system", "content": instruction},
            {"role": "user", "content": text[:60000]},
        ],
        "max_tokens": 2000,
    }
    async with httpx.AsyncClient(timeout=120) as client:
        resp = await client.post(
            f"{PROXY}/chat/completions",
            json=body,
            headers={"Authorization": f"Bearer {TOKEN}"},
        )
        resp.raise_for_status()
        data = resp.json()
    return data["choices"][0]["message"]["content"]
