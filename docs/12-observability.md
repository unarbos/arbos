# 12 — Observability & Debuggability

> Status: foundation landed (correlation), rest staged. The goal is unusual and
> explicit: **when something breaks, a debugging agent should be able to fix it
> from a deterministic reproduction, not a vague stack trace.** Our
> event-sourced design makes that achievable; this doc says how.

## The thesis: the event log is a reproduction

arbos already records every turn as an append-only `core.Event` log, and the
conversation a provider sees is a *projection* of that log (`core.Project`).
Because the engine is deterministic given (events, fakes/clock), **a session's
log can be replayed to reproduce exactly what happened.** That is the foundation
everything else builds on — observability here is less about scraping metrics and
more about preserving a perfect, replayable record.

## Three planes, three jobs

| Plane | Question it answers | Medium | Status |
|---|---|---|---|
| **Event log** | *What happened?* (domain truth, replayable) | `core.Event` in the store | ✅ exists |
| **Structured logs** | *Why did it go wrong?* (operational narrative) | `log/slog`, JSON to file | staged |
| **Traces / metrics** | *How is it behaving in aggregate?* (latency, error rate) | `Observer` port → OTel | staged |

The planes are joinable. That join is the whole point, and it only works if
correlation is threaded from the start — which is why it is the one piece built
now, before the SQLite schema is written (a join key retrofitted later is a
migration across every table and every log line).

## Correlation model (landed)

- **Domain join key: `SessionID` + `TurnID`.** Every event the engine persists
  carries a `TurnID` (`core.Event.TurnID`) — a per-session monotonic counter
  identifying one user prompt and *all* events it produced (user message →
  injected context → assistant → tool results → assistant …). It is assigned by
  the engine, not the store, because a turn spans multiple appends. `TurnID == 0`
  means "not produced inside an engine turn"; `Validate` permits it so fixtures
  and out-of-band writers stay legal.
- **Operational id: `trace_id`.** A random per-turn id carried in
  `context.Context` (`internal/obs.Correlation`, attached once in
  `engine.processPrompt`). It is stamped on logs and future spans but
  intentionally **not** persisted on the domain `Event` — the domain joins on
  `SessionID+TurnID`; `trace_id` ties the *operational* planes to a turn.

```go
// internal/obs
type Correlation struct {
    SessionID string
    TurnID    int64
    TraceID   string // operational; on logs/spans, not on the domain Event
}
obs.With(ctx, Correlation{...})  // engine attaches once per prompt
obs.From(ctx)                    // every persistence/log site reads it
```

`engine.stamp(ctx, ev)` is the single choke point that copies `TurnID` onto
every event, so no persistence path can forget it.

## Replay — the agent-debug mechanism (committed)

Two first-class capabilities, designed now, built in their phase:

1. **`arbos debug replay <session_id>`** — load a session's event log and re-run
   the turn(s) against deterministic fakes (or the recorded provider responses),
   reproducing the exact projection, tool calls, and failure. This turns "it
   broke in production" into a local, deterministic repro a fixing agent can
   iterate against — the same machinery `engine_test.go` golden replays already
   use, exposed as a command.
2. **Session-export bundle** — `arbos debug export <session_id>` produces a
   single artifact (event log + correlated structured logs + redacted config)
   suitable for attaching to a bug report or handing to an agent. Secret values
   are redacted via the secret broker (ADR-0016) before export.

Design constraints that keep replay honest (already respected by the engine):

- Context injection (memory, skills, retrieval) is **logged**, not assembled
  ephemerally (`EventContext`), so a replay sees exactly what the model saw.
- Usage and reasoning are persisted (ADR-0011), so a replay reconstructs token
  accounting and the thinking trace.
- `ProviderMeta` round-trips (ADR-0003), so provider-specific state replays.
- The `Clock` is injected, so timestamps are reproducible.

To make provider-backed replay exact (not just fake-backed), the provider phase
will record raw responses keyed by `trace_id` so a replay can feed them back —
captured here as a requirement so the provider adapter is built record-aware.

## Staged, not built now (and why that's fine)

- **`slog` wiring** — inject a `*slog.Logger` into the engine (like `Clock`),
  emit correlated lifecycle logs (turn start/end, llm call, tool call, retry,
  error) with `session_id`/`turn_id`/`trace_id` attributes. Deferred because the
  correlation it depends on now exists; adding the sink is a localized change.
- **`Observer` port** — a no-op-by-default interface the engine calls at
  lifecycle points so metrics/traces have one home instead of being sprinkled
  through the loop. Declared as the seam; OTel and the observability plugin slot
  behind it later. (Avoids the premature-telemetry trap: one seam now, exporters
  later.)
- **Secret redaction in logs** — a `slog.Handler` concern, layered on the secret
  broker when logging lands.

## Concurrency

The logger and `Observer` will be shared across session actors, so their
implementations must be goroutine-safe (`slog` is). Per-turn correlation rides in
each actor's `ctx`, so there is no shared mutable observability state — consistent
with the actor model (see [05-concurrency](./05-concurrency.md)).
