# ADR-0009 — Store failures abort the turn with an ErrorEvent

- Status: Accepted
- Date: 2026-06-08

## Context

The event log is the source of truth. The first engine cut discarded every store
error (`_ = store.AppendEvent(...)`, `events, _ := store.Events(...)`). For an
event-sourced kernel that is a correctness bug, not a style nit:

- a failed append lets the in-memory turn keep going while the log is missing an
  event, so **replay diverges from the live turn** — breaking the central
  promise (deterministic replay, trajectory == log);
- a failed `Events()` silently projects an empty/truncated conversation and the
  model is called on the wrong context with no signal.

These are real, frequent SQLite failure modes (locked db, disk full), so the
convention must be fixed before persistence and before adapters assume the
silent-success shape.

## Decision

Any failure to **append** an event or **load** history is turn-fatal: the engine
emits `core.ErrorEvent` and aborts the turn. The engine's `append` helper
returns a bool and callers abort on false; `runTurn` aborts on an `Events()`
error.

Exception: genuinely **non-authoritative** writes are best-effort and explicitly
ignored with a comment — the derived `Session.TokenCount` mirror
(`UpdateSession`) and the best-effort interrupt marker. These never affect the
conversation log, so losing one cannot cause replay divergence.

## Consequences

- Replay can never diverge from the live turn due to a swallowed append.
- The control flow (abort-on-failure) is fixed in Phase 1, before three adapters
  depend on the old shape.
- Callers must treat `ErrorEvent` as terminal (the CLI exits; the gateway will
  surface and end the turn).
