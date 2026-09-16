# P-08 MCP servers per project — QA note (features agent, 2026-09-13)

Branch `cursor/mcp-servers-b027`, base `rust`.

## What it does

MCP (Model Context Protocol) servers are programs that offer tools over JSON-RPC. Until now one stdio server came from `ARBOS_MCP_CMD`. Now they are declared per place and per user, and HTTP servers work too.

Files read at kernel start, first name wins: `.arbos/mcp.toml`, `.cursor/mcp.json`, `.mcp.json` (Cursor / Claude Code shape), `~/.config/arbos/mcp.toml`. `ARBOS_MCP_CMD` still works as a server named `env`.

```toml
[servers.fs]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
env = { LOG = "1" }           # literal values
env_from = ["GITHUB_TOKEN"]   # copied from the kernel's environment

[servers.docs]
url = "https://mcp.example.com/mcp"       # Streamable HTTP
headers = { Authorization = "Bearer …" }
```

Tools are registered as `mcp__<server>__<tool>`; every agent may call them (as before). Stdio servers are spawned per request (initialize, call, exit), as before. HTTP: `initialize` then the request, `Mcp-Session-Id` echoed, JSON or SSE replies accepted.

A server that fails at start is reported once on stderr and skipped; the others load. `mcp` list appears in the kernel's start log.

## Attack ideas

1. Server that takes 20 s to start (npx download): `tools/list` at kernel start blocks serve — measure; propose lazy discovery if > 3 s.
2. `.cursor/mcp.json` with `"type": "sse"` (old transport): unsupported → skipped with a clear line.
3. Two servers with the same tool name: distinct because of the `mcp__<server>__` prefix.
4. Tool name with characters the model API rejects (`.`, `/`): replaced with `_` in the registered name; the original goes to the server.
5. HTTP server returning 401: error names the server and status; the kernel keeps serving.
6. `env_from` naming a variable that does not exist: skipped silently? Implemented: warns on stderr.
7. Secrets in `env = {}` literals: the file is in the repo. Note; prefer `env_from` (and #30's secrets door later).
8. A server whose `tools/list` returns 300 tools: all registered; the prompt's tool list grows — check the schema token cost.
9. Remote place: the kernel on the remote machine reads its own `.arbos/mcp.toml`; the desktop does nothing. Fine.
10. Kill the kernel mid stdio call: the child server process is `kill`ed in `rpc` — but a kernel SIGKILL leaves it; short-lived anyway.

## How to run

Tiny stdio server in Python (see PR body), `.arbos/mcp.toml` naming it; `arbos-kernel serve` logs `mcp: echo offers …`; prompt "call the echo tool with hello" → `mcp__echo__echo` runs.
