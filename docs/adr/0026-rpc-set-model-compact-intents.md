# ADR-0026 — set_model and compact intents

- Status: Accepted
- Date: 2026-06-08

## Context

pi's RPC surface includes commands beyond prompt and events. Mapped onto arbos's
Intent-in / Event-out seam: abort is `InterruptIntent`, and follow_up / steer are
`PromptIntent` (queued mid-turn, surfaced as `Queued`) — all pre-existing. Two pi
commands had no arbos equivalent: switching the model mid-session (`set_model`)
and forcing a context compaction (`compact`).

## Decision

Add two intents to the sealed set (extends ADR-0018): `SetModelIntent{Model}` and
`CompactIntent{}`, with codec cases in `core/interaction.go`.

The engine applies both only when the session is idle (handled in `runActor`,
not mid-turn). `SetModelIntent` sets a per-session model override on the
`Conversation` (actor-owned; read by `runTurn` at request build, mutated only
while no turn runs, so no synchronization is needed). `CompactIntent` calls a new
`compactNow` that forces a compaction pass via the configured `ContextPolicy`
regardless of the budget trigger. Mid-turn, both are benign no-ops (a frontend
resends when idle), which preserves the actor's single-writer and
no-mid-turn-mutation invariants.

## Consequences

- The control seam now carries the full set of non-tree pi RPC commands:
  set_model, compact, abort (interrupt), follow_up/steer (prompt).
- Additive: new intents are ephemeral wire messages, never persisted, so there
  is no log-shape change or version bump. fork/switch_session (tree-dependent)
  follow in a later phase.
