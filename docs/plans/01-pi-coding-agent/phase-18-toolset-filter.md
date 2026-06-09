# Phase 18 — Grant.Tools toolset filtering

Back to [overview](./overview.md). Status: done.

## Goal

Make a delegated child receive exactly the tools its `Grant.Tools` allows;
previously the allowlist was ignored.

## Changes (as implemented)

- `internal/tool/filter.go`: `tool.Filter(inner, names)` returns a
  `ports.ToolRuntime` that exposes and dispatches only the named tools. An empty
  allowlist means "no restriction" (returns the runtime unchanged). A call to a
  non-granted tool returns an error result (`tool not granted: <name>`), never a
  panic, so the model sees why.
- `internal/agent/pi`: `NewAgent` (extracted from `Register`) applies
  `tool.Filter(reg, g.Tools)` in the delegation engine factory.
- `cmd/arbos`: the `arbos` backend factory applies the same filter.

## Verification

Static gate green, no new lints. Runtime: a child granted `["read"]` was refused
`write` (`tool not granted: write`, file not created); an unrestricted child
wrote the file.
