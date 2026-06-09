# ADR-0030 — Go-native extension seam and event bus

- Status: Accepted
- Date: 2026-06-08

## Context

pi is self-extensible: TypeScript extensions register tools, slash commands, and
event handlers, and subscribe to a rich event bus. A single static Go binary
cannot load TS modules at runtime, so the faithful-in-spirit port is a build-time
registration seam with the same event vocabulary.

## Decision

Add `internal/extension`. An `Extension` is a Go function `func(API) error`; the
host runs each at startup. The `API` offers `RegisterTool`, `RegisterCommand`,
and `On(event, handler)`. The event `Bus` is a `ports.Observer`: it receives
every KernelEvent the engine emits and fans out to handlers keyed by pi's event
names (`agent_end`, `turn_end`, `message_update`, `tool_execution_start/end`,
`tool_call`, `tool_result`, `error`), so handlers port across with the same
vocabulary. Extension tools register into the coding `tool.Registry`; slash
commands register handlers the input boundary can expand.

pi's `todo` and `subagent` are example extensions, not built-ins, and arbos's
first-class `delegate` / `start_coding_session` already cover delegation, so no
built-in-via-extension behavior needed porting; the seam is proven by a sample
extension.

## Consequences

- arbos gains pi-style extensibility within a static binary: build-time tool /
  command / event-hook registration with pi's event names.
- No kernel change: the bus is just an Observer; extension tools are just
  registry entries. Proven by an extension that registered a tool (dispatched
  through the engine), a `tool_result` handler (fired), and a slash command
  (expanded).
- A runtime TS plugin loader is intentionally not ported (incompatible with a
  static Go binary); that is the one pi extensibility affordance arbos replaces
  with build-time registration rather than reproduces.
