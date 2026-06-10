# 05 — Concurrency

> Status: foundational. Decided before the engine grew, because the model shapes
> every signature. Chosen: **actor-per-session**.

## The question that forced the decision

Session state (the working conversation, the iteration counter, the in-flight
LLM call, parallel tool results) is touched by the LLM stream, by tool dispatch,
and by interrupts. Foundational rule: *before sharing state between actors, ask
"what happens if another actor modifies this concurrently?" If the answer isn't
"nothing," isolate.* Here the answer wasn't "nothing", so we isolate.

## The model: one goroutine owns each session

```
intents ──▶ [ session actor goroutine ] ──▶ events
              sole owner of session state
```

- Outside actors influence a session **only** by sending `core.Intent` on its
  inbound channel.
- They observe it **only** by reading `core.KernelEvent` on its outbound
  channel.
- There is **no shared mutable session state**, therefore **no session-level
  locking**. The data race is designed out, not guarded against.

This directly retires two Hermes hacks: the polled `_interrupt_requested` flag
(now `context.Context` cancellation) and `propagate_context_to_thread` (now a
plain `ctx` argument threaded through).

## Interrupts are cancellation, not flags

While a prompt is in flight, the actor runs the turn in a child context and
selects over: turn-done, parent-done, and new inbound intents. An
`InterruptIntent` cancels the child context; the provider's `Stream` and any
tool `Dispatch` observe `ctx.Done()` and unwind. The actor then emits
`Interrupted` and returns to waiting. No flag is ever polled.

```go
turnCtx, cancel := context.WithCancel(parent)
go runTurn(turnCtx, ...)
select {
case <-done:            // turn finished
case <-parent.Done():   cancel(); <-done   // session shutting down
case intent := <-in:    if isInterrupt(intent) { cancel(); <-done; emit(Interrupted{}) }
}
```

## Channels and back-pressure

- `intents` is buffered (small) so a `Send` rarely blocks; the actor consumes it
  both while idle and while busy.
- `events` is buffered; **a frontend must drain it** or the actor blocks (this is
  intentional back-pressure, not a bug). The control seam (Phase 7) drains on
  behalf of remote frontends.
- The events channel is closed when the actor exits (parent context cancelled or
  intents channel closed), which is the frontend's signal that the session is
  gone.

## Parallel tools (Phase 3) — the one place we fan out

Within a single turn, a batch of tool calls may run concurrently, but only when
safe. Rule (ported from Hermes's conflict analysis): **read-only tools may run
in parallel; any write, or two calls touching the same path, force sequential.**
Implementation will be `errgroup` + a small semaphore, all within the owning
actor's turn — results are collected back before the next iteration, so the
session's single-writer invariant holds.

## Subagents / delegation compose cleanly

`delegate_task`-style subagents are just **child sessions**: a tool spawns a new
session actor via the engine and relays its `KernelEvent`s. Because each session
is independently owned, nesting is free — no shared *session* state crosses the
boundary, and a parent interrupt cancels the child via context propagation.
Sibling delegations fan out concurrently regardless of whether they read or
write; the one thing they do share is the live filesystem. Two guards keep that
survivable without serializing: the read-ledger staleness check (write/edit
refuse to clobber a file that changed since the agent last saw it this turn) and
an automatic per-turn restore point (before a file mutation, the coding toolset
snapshots the git repo that owns that target path, so `undo path=...` can roll
that repo back). When a stale apply happens, the model uses `changes`, re-reads,
and retries against the live content — the local Cursor workflow. True isolation
— a worktree per writer — stays the opt-in for when collisions must be
impossible rather than recoverable. See [08-agent-delegation](./08-agent-delegation.md).

## The one shared resource: SessionStore

The store is used across sessions, so it is the single place that guards itself
(the fake uses a mutex; SQLite uses its own locking + WAL). The engine
guarantees single-writer *per session* via the actor, so the store needs only
cross-session integrity, not per-session write serialization.

## Verification

Every change touching concurrency runs under `go test -race`. The race detector
is the high-signal check for this area and is wired into CI.
