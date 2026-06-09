# ADR-0029 — MCP client and server

- Status: Accepted
- Date: 2026-06-08

## Context

pi can consume external MCP tool servers and expose its own tools over MCP. The
Model Context Protocol is JSON-RPC 2.0 over stdio (newline-delimited messages):
initialize, tools/list, tools/call.

## Decision

Add `internal/mcp` with a client and a server, both over any io.Reader/Writer
(stdio of a spawned subprocess, or a pipe).

- Client: `Initialize`, `ListTools`, `CallTool`, and `Runtime(prefix)` which
  exposes the server's tools as a `ports.ToolRuntime` (names prefixed
  `prefix__tool` to avoid collisions; calls serialized by a mutex so the engine's
  parallel read-only dispatch maps to one request at a time).
- Server: `Serve(rt, r, w)` exposes any `ports.ToolRuntime` (e.g. the coding
  toolset) over MCP, handling initialize / tools/list / tools/call.
- `tool.Multi` composes runtimes so the coding toolset and MCP-provided tools
  present to the engine as one runtime (schemas concatenated; Dispatch routed by
  tool name).

No kernel change: MCP tools are just another `ToolRuntime` behind the existing
port.

## Consequences

- pi-on-arbos can use external MCP servers (their tools appear in the registry)
  and expose arbos's tools to external runtimes.
- Resources and prompts are out of scope for the coding path; the JSON-RPC seam
  is in place to add them. Proven by a client/server loopback over real pipes
  (server exposed the 7 coding tools; client listed and called `ls` through the
  protocol), no external dependency.
