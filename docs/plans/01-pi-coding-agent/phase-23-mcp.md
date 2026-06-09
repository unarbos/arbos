# Phase 23 — MCP client and server

Back to [overview](./overview.md). Status: done. ADR-0029.

## Changes (as implemented)

- `internal/mcp`: a JSON-RPC 2.0 (newline-delimited, MCP stdio framing) client
  and server. Client: `Initialize`, `ListTools`, `CallTool`, and
  `Runtime(prefix)` exposing an external server's tools as a `ports.ToolRuntime`
  (prefixed names, mutex-serialized calls). Server: `Serve(rt, r, w)` exposes any
  arbos `ToolRuntime` over MCP.
- `internal/tool/multi.go`: `tool.Multi` composes runtimes (schemas concatenated,
  Dispatch routed by name) so coding + MCP tools present as one runtime.

## Verification

Static gate green, no new lints. Runtime: a client/server loopback over real
JSON-RPC pipes — the server exposed arbos's 7 coding tools; the client
initialized, listed them, and dispatched `ext__ls` through the protocol back into
arbos's `ls` (correct listing); `tool.Multi` routed both local and MCP tools.
