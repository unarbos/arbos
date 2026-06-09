# ADR-0008 — Event payloads are a sealed sum type

- Status: Accepted
- Date: 2026-06-08

## Context

The first cut of `core.Event` carried payloads as nullable pointers
(`Message *Message; ToolResult *ToolResult`) with a documented "exactly one is
set" invariant. That invariant was false from the start — `EventCompressed` and
`EventInterrupted` set neither — and unenforceable: nothing stopped a caller
from setting both or none, and "typed pointers keep the compiler honest" does
not hold for one-of-N nullable fields. `Event` is the **persisted spine** ("a
type change here is a rewrite"), and the rest of the kernel already models
closed sets as sealed interfaces (`Intent`, `KernelEvent`). The payload was the
one place that reached for nullable-pointer soup — and the one that gets written
to disk.

## Decision

Model the payload as a sealed `EventPayload` interface with one variant per kind
(`MessagePayload`, `ToolResultPayload`, `UsagePayload`, `CompressionPayload`,
`InterruptPayload`). The marker method is `Kind() EventKind`, which also yields
the persisted discriminator. `Event` holds a single `Payload EventPayload`, so
"exactly one payload, matching the kind" is **structural** — you cannot
construct an event with two payloads or none. Events are built only via per-kind
constructors (`NewMessageEvent`, …) and validated (`Event.Validate`) on every
append.

## Consequences

- The invariant is enforced by the type system, not documentation.
- Adding a kind is a new variant + a `Project` case; consumers type-switch
  exhaustively (with explicit defaults).
- Persistence (Phase 2) serializes `Kind()` as a discriminator column and the
  payload as JSON; decoding dispatches on the discriminator.
