# 03 — Ports (kernel outbound interfaces)

> Status: foundational. Ports are the seams adapters plug into. The engine
> depends only on these interfaces; concrete implementations live at the edge.
> The kernel ships exactly one reference adapter per port (`internal/fake/*`);
> breadth arrives later behind the same interfaces.

A port is small on purpose. If an interface starts accumulating provider- or
platform-specific methods, that knowledge belongs in the adapter, not the port.

## LLMProvider — stateless completion (drives our loop)

```go
type LLMProvider interface {
    Name() string
    Capabilities() Capabilities
    Stream(ctx context.Context, req core.LLMRequest) (<-chan core.LLMChunk, error)
}
```

Contract:

- The returned channel is closed when the response completes.
- Implementations **must** honor `ctx` cancellation — an interrupt cancels the
  turn's context, and a provider that ignores it leaks goroutines and blocks the
  session actor.
- Partial tool-call fragments are accumulated **inside the adapter**; the engine
  only ever sees fully-formed `core.ToolCall`s on a chunk.
- Opaque data that must return on the next turn (Anthropic thinking signatures,
  Gemini parts, Responses item ids) is written to `Message.ProviderMeta[Name()]`
  and read back from the same key. See ADR-0003.

`Capabilities` lets the engine adapt the request without per-provider `if`s in
the loop:

```go
type Capabilities struct {
    Vision    bool
    Reasoning bool
    Tools     bool
}
```

## AgentTransport — owns its own loop (FUTURE, not built yet)

Not every "provider" fits the completion shape. Codex app-server and Copilot
ACP are **session-oriented subprocess agents** that own their own
LLM→tool→repeat loop; in Hermes these bypass the turn loop entirely. Forcing
them into `LLMProvider` would break the engine's core assumption.

Decision (ADR-0002): model them as a **second port**, and structure the session
actor so a turn can be *delegated* to a transport instead of run by our loop:

```go
// FUTURE — declared here as the agreed shape, implemented in its own phase.
type AgentTransport interface {
    Name() string
    // RunTurn drives a complete turn itself, emitting the same KernelEvents our
    // loop would, and persisting via the same SessionStore. The engine hands
    // off control and consumes events; it does not call an LLM directly.
    RunTurn(ctx context.Context, sessionID string, in <-chan core.Intent, out chan<- core.KernelEvent) error
}
```

The payoff of deciding this now (without building it): the session actor's turn
step is "run a turn," abstract over *who* runs it. We will not hard-code "call
`LLMProvider.Stream`" as the only way a turn happens.

## ToolRuntime

```go
type ToolRuntime interface {
    Schemas() []core.ToolSchema
    Dispatch(ctx context.Context, call core.ToolCall) core.ToolResult
}
```

Contract:

- `Dispatch` never panics to the caller and never returns an error out-of-band —
  failures come back as `ToolResult{IsError: true}` so the model can see them.
- `Dispatch` honors `ctx` cancellation.
- Schemas are generated from Go structs at build time (ADR-0004) so the schema
  can't drift from the handler.
- Read/write classification for safe parallelism (Phase 3) is metadata on the
  runtime, not a concern the engine hard-codes.

## SessionStore

```go
type SessionStore interface {
    CreateSession(ctx context.Context, s core.Session) error
    Get(ctx context.Context, sessionID core.SessionID) (core.Session, error)
    UpdateSession(ctx context.Context, s core.Session) error
    AppendEvent(ctx context.Context, e *core.Event) error
    Events(ctx context.Context, sessionID core.SessionID) ([]core.Event, error)
}
```

Contract:

- `AppendEvent` assigns `Seq` (monotonic per session) and `ID`, and **rejects**
  events failing `core.Event.Validate` (nil payload / unversioned).
- `UpdateSession` persists mutable metadata (status `active→compressed→ended`,
  token count, `UpdatedAt`); compression is unimplementable without it (ADR-0009
  covers why metadata writes are best-effort, not turn-fatal).
- Store failures on append/load are **turn-fatal** in the engine (ErrorEvent +
  abort, ADR-0009) so the live turn can never diverge from the log.
- Safe for concurrent use **across** sessions. Single-writer **within** a
  session is guaranteed by the engine (the session actor), so the store does not
  need per-session write serialization beyond basic integrity.
- The fake (in-memory) and the durable store (SQLite, Phase 2 / ADR-0005)
  implement the identical interface — Phase 2 is a drop-in.

## Clock

```go
type Clock interface { Now() time.Time }
```

Injected so turns are deterministically replayable (fixed clock in tests, wall
clock in production). Do not call `time.Now()` anywhere in the engine.

## Why these five, and no more (for the kernel)

| Concern | Port | Reference adapter |
|---|---|---|
| Talk to a model | `LLMProvider` | `fake.Provider` |
| Run a tool | `ToolRuntime` | `fake.Tools` |
| Persist a session | `SessionStore` | `fake.Store` → SQLite |
| Tell time | `Clock` | `fake.Clock` → wall clock |
| (future) delegate a whole turn | `AgentTransport` | — |

Memory, MCP, skills, platforms, and frontends are **not** kernel ports — they
attach above the kernel via the control seam (Phase 7) or compose as tools. The
kernel stays small.

## Above the ports: the Agent layer

`LLMProvider` and `AgentTransport` are *leaf capabilities*. The unit of
composition and delegation is the **`Agent`** (run a `Task`, stream
`KernelEvent`s), which uses these ports internally. Codex/Claude Code/Cursor,
other Arbos agents (local and remote), Hermes, and plain LLM queries all pass off
through the same `Agent` + `delegate` mechanism. See ADR-0013 and
[08-agent-delegation](./08-agent-delegation.md).
