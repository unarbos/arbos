# 02 — Data Model (kernel)

> Status: foundational. These shapes are the most expensive thing to change
> later, so they are decided first and on purpose. A type change here is a
> rewrite; a type change in this doc is a one-line edit. Everything downstream
> (engine, persistence, providers, frontends) derives from these.

The kernel ports `Intent`s in and emits `KernelEvent`s out, and persists an
append-only `Event` log per session. It knows nothing about transports,
providers, or storage engines beyond the [ports](./03-ports.md).

## The spine: an append-only event log

A session's content is **not** a mutable list of messages. It is a sequence of
`core.Event`s. The conversation a provider sees is a *projection* of that log
(`core.Project`, a pure function). This single decision buys us, for free:

- deterministic replay (golden tests fold the same log into the same turn),
- trajectory generation for training (the log *is* the trajectory),
- audit / time-travel debugging,
- compression as "fold old events into a summary event," not "copy rows into a
  new table."

The payload is a **sealed sum type** (`EventPayload`), not nullable pointers, so
"exactly one payload, matching the kind" is enforced by the compiler — you
cannot build an event with two payloads or none. This mirrors how `Intent` and
`KernelEvent` are modeled, and it matters most here because `Event` is what gets
persisted (ADR-0008). `Kind()` doubles as the seal marker and the persisted
discriminator.

```go
type EventKind string // user_message | assistant_message | tool_result |
                       // usage | compressed | interrupted | config

type EventPayload interface { Kind() EventKind } // sealed

    MessagePayload{ Message Message }          // user or assistant (role-derived kind)
    ToolResultPayload{ Result ToolResult }
    UsagePayload{ Usage Usage }                // token accounting, in the log (ADR-0011)
    CompressionPayload{ Summary string; ReplacedSeqLo, ReplacedSeqHi int64 }
    InterruptPayload{ Reason string }
    ConfigPayload{ Model, SystemPrompt string; Tools []ToolSchema } // what the model ran with, so a
                                               // log is a self-contained trajectory (replay/training
                                               // never depends on the current binary's templates)

const CurrentEventVersion = 1 // stamped on every written event; see ADR-0010

type Event struct {
    ID        int64        // store-assigned
    SessionID SessionID    // distinct type, not a bare string
    Seq       int64        // monotonic within a session, store-assigned
    Version   int          // schema version of this row (ADR-0010)
    CreatedAt time.Time
    Payload   EventPayload // exactly one variant, structurally guaranteed
}
```

Events are built only via per-kind constructors (`NewMessageEvent`, …) that
stamp `Version`, and `Event.Validate` (called by the store on every append)
rejects a nil payload or zero version. A type change to a persisted payload is
**not** a free edit — it requires a version bump + an upcaster (ADR-0010); new
*kinds*, by contrast, are additive (older readers ignore unknown payloads in
`Project`).

## Provider-neutral conversation types

Providers never see our internal types directly; each adapter translates these
to/from its wire format. The kernel stays free of provider quirks.

```go
type Role string // system | user | assistant | tool

type Message struct {
    Role       Role
    Content    string
    Reasoning  string     // assistant thinking trace text, when provided
    ToolCalls  []ToolCall // on assistant messages requesting tools
    ToolCallID string     // on RoleTool messages, links to a ToolCall

    // Opaque per-provider data that must round-trip back on later turns
    // (Anthropic thinking signatures, Gemini parts, Responses item ids).
    // Keyed by provider name; the kernel never interprets it. See ADR-0003.
    ProviderMeta map[string]json.RawMessage
}

type ToolCall struct {
    ID   string
    Name string
    Args json.RawMessage // kernel never decodes; the ToolRuntime owns the shape
}

type ToolResult struct {
    CallID  string
    Content string
    IsError bool
}

type ToolSchema struct {
    Name        string
    Description string
    Parameters  json.RawMessage // JSON Schema; generated from Go structs at
                                // build time so it can't drift from the handler
}
```

## LLM request / response

```go
type LLMRequest struct {
    Model       string
    Messages    []Message
    Tools       []ToolSchema
    Stream      bool
    Temperature *float64 // nil = provider default
    MaxTokens   int      // 0 = provider default
}

type LLMChunk struct {
    ContentDelta   string
    ReasoningDelta string
    ToolCalls      []ToolCall // emitted fully-formed (adapter accumulates partials)
    Usage          *Usage
    Done           bool
}
```

A provider returns `<-chan LLMChunk`, closed on completion, and must honor
`ctx` cancellation (an interrupt cancels the turn's context).

## Session metadata

```go
type SessionStatus string // active | ended

type Session struct {
    ID         SessionID
    ParentID   SessionID // fork/branch lineage; empty for a root session (NOT compression)
    Status     SessionStatus
    Model      string    // durable model authority: set at creation, updated by SetModelIntent
    Principal  string    // RESERVED: who owns/authorizes (gateway/frontend phase)
    Origin     string    // RESERVED: originating surface, e.g. "telegram:chat/123"
    CreatedAt  time.Time
    UpdatedAt  time.Time
}
```

`Principal`/`Origin` are reserved now (cheap) for auth and reply-routing when the
gateway/frontend phase arrives, rather than threaded through every call site
later (a migration). Empty for local single-user sessions today. See ADR-0019.

Compression is **in-place**, not a session rotation: a `CompressionPayload`
event is appended to the live session's log and `Project` folds the replaced
span, rendering the summary in its place. The session stays `active` — no child
session, no row copying, no compression lock. `ParentID` is reserved for
explicit forks/branches. See ADR-0014.

## Injected context (memory, skills, retrieval)

Context the agent pulls in for a turn — memory recall, a skill body, retrieved
docs — is logged as a `ContextPayload` event (not assembled ephemerally) so a
replayed session reproduces exactly what the model saw:

```go
type Segment struct { Source, Content string } // provenance-tagged

type ContextPayload struct { Segments []Segment }
```

`Project` renders only the **latest segment per source**, fenced
(`<<source>> … <</source>>`), as a stable prefix right after the system prompt:
older injections stay in the log for audit but are superseded, so the live
prompt stays lean and prefix-cache friendly, and injected text is visibly
delimited from system instructions. The retrieval/write-back seam is the
`MemoryProvider` port; the compression-trigger seam is the `ContextPolicy` port.
See ADR-0015.

## The kernel boundary: Intent in, Event out

These two sealed interfaces are the kernel's entire public vocabulary. Every
frontend/adapter (CLI, TUI, gateway, socket) translates its transport into
`Intent`s and renders `KernelEvent`s. Nothing calls into the turn loop directly,
which is what lets new surfaces attach without re-implementing turn logic.

```go
type Intent interface { Kind() IntentKind }        // sealed by the discriminator
    PromptIntent{ Text }
    InterruptIntent{}
    ResumeIntent{ SessionID }
    ApprovalResponseIntent{ RequestID, Approved, Reason }   // answers an ApprovalRequest

type KernelEvent interface { Kind() KernelEventKind } // sealed by the discriminator
    MessageDelta{ Text }
    ReasoningDelta{ Text }
    ToolStarted{ Call }
    ToolFinished{ Result }
    TurnComplete{ FinalResponse }
    Interrupted{}
    ErrorEvent{ Err }
    ApprovalRequest{ RequestID, Call, Reason }              // pause: may this tool call proceed?
```

Both interfaces are sealed by a **`Kind()` discriminator** (not an unexported
marker), mirroring `EventPayload.Kind()`. This keeps the sets closed *and* makes
the vocabulary wire-ready: in-process frontends switch on the concrete type;
out-of-process frontends (web, desktop, remote arbos, the control seam) switch
on `Kind()` across a tagged-union wire without reflection. Variant fields carry
`json` tags; the encode/decode codec mirrors `core/codec.go` and lands with the
control seam. See ADR-0018.

**Suspend-and-await.** `ApprovalRequest` (out) pairs with
`ApprovalResponseIntent` (in), correlated by `RequestID`.
When a turn emits a request, the session actor blocks it until the matching
response arrives (an interrupt still cancels). This is what lets the kernel pause
mid-turn for tool approval. Secret *values* never use
this path — they are resolved by the broker at the request boundary (ADR-0016).
The actor await-wiring is built with the tool runtime; the vocabulary is fixed
now. See ADR-0018.

## Concurrency note (see 05-concurrency.md)

Each session is owned by one goroutine (actor model). Outside actors influence
it only by sending `Intent` on its inbound channel and observe it only by
reading `KernelEvent` on its outbound channel. There is **no shared mutable
session state**, so there is no session-level locking. Interrupts are
`context.Context` cancellation, not a polled flag. The `SessionStore` is the one
genuinely shared resource and is therefore the one place that guards itself.

## What is intentionally absent

No provider SDK types, no transport types, no storage-engine types, no tool
implementations. Those live behind [ports](./03-ports.md). The kernel ships one
reference adapter per port (`internal/fake/*` for the walking skeleton); breadth
arrives later, each behind these same interfaces.
