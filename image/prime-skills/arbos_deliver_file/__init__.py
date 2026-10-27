"""arbos_deliver_file: F10 Prime skill. Hands a file to the user as an attachment card.

Calls the project MCP server's `deliver_file` tool (the same one pi uses) over loopback with
this session's id, so the card lands on this subagent's own progress message.
"""

import json
import os

import httpx

MCP_URL = os.environ.get("ARBOS_MCP_URL", "http://127.0.0.1:7789/mcp")
TOKEN = os.environ.get("ARBOS_MCP_TOKEN", "")
SESSION_ID = os.environ.get("ARBOS_SESSION_ID", "")


async def _call(tool: str, arguments: dict) -> str:
    body = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments},
    }
    async with httpx.AsyncClient(timeout=180) as client:
        resp = await client.post(
            MCP_URL, json=body, headers={"Authorization": f"Bearer {TOKEN}"}
        )
        resp.raise_for_status()
        data = resp.json()
    result = data.get("result", {})
    text = "".join(c.get("text", "") for c in result.get("content", []))
    if result.get("isError"):
        raise RuntimeError(text or "deliver_file failed")
    return text


async def run(path: str, title: str | None = None, kind: str | None = None) -> str:
    """Deliver `path` (under the project folder's out/ by convention; relative paths resolve in the project folder). Returns the tool's JSON summary."""
    args = {"path": path, "session_id": SESSION_ID}
    if title:
        args["title"] = title
    if kind:
        args["kind"] = kind
    return await _call("deliver_file", args)


async def generate_image(prompt: str, size: str = "1024x1024", title: str | None = None) -> str:
    """Generate an image through the relay and deliver it as a card."""
    args = {"prompt": prompt, "size": size, "session_id": SESSION_ID}
    if title:
        args["title"] = title
    return await _call("generate_image", args)


async def post_chart(chart: dict, caption: str, reply_to: str | None = None) -> str:
    """Post a native chart (line, area, bar, scatter, pie) from structured series: `chart` is the
    `post_chart` spec (kind, title, series [{name, points | values}], categories, x/y {label, unit}),
    `caption` one or two sentences stating the finding. Caps: 8 series, 500 points each, 2 000 total,
    100 categories, 32 KB — bigger data is a CSV through `run`. Raises when NOT posted."""
    args = {"chart": chart, "caption": caption, "session_id": SESSION_ID}
    if reply_to:
        args["reply_to"] = reply_to
    return await _call("post_chart", args)


def summary(result: str) -> dict:
    """Parse the JSON text `run` returns."""
    try:
        return json.loads(result)
    except json.JSONDecodeError:
        return {"raw": result}
