# ADR-0017 — Observability foundation: turn correlation + replay

- Status: Accepted (correlation built; logging/Observer/replay-command staged)
- Date: 2026-06-08

## Context

We want a system where, when something breaks, a debugging agent can fix it from
a **deterministic reproduction** rather than a context-free error. Our
event-sourced design already makes sessions replayable, but observability has to
be designed in: correlation IDs that join the domain log, structured logs, and
traces cannot be retrofitted across the codebase later, and the time to add the
domain join key is *before* the SQLite schema is written.

We deliberately scope this small to avoid the premature-telemetry trap (building
an exporter stack before there is anything to export).

## Decision

Build only the foundation now:

1. **Add `TurnID` to `core.Event`** — engine-assigned, per-session monotonic,
   grouping every event produced by one prompt. The domain join key
   (`SessionID + TurnID`) and the unit of deterministic replay. `Validate` does
   not require it (`0` = non-engine write) to keep fixtures/out-of-band writers
   legal.
2. **Add `internal/obs.Correlation`** carried in `context.Context`, attached once
   per prompt in `engine.processPrompt`, with a per-turn `trace_id`. `trace_id`
   is for the operational planes (logs/spans) and is **not** persisted on the
   domain `Event`.
3. **Single stamping choke point** (`engine.stamp`) so no persistence path can
   ship an event without its `TurnID`.

Commit to **replay as a first-class feature** (`arbos debug replay` + a
session-export bundle), designed in [12-observability](../12-observability.md),
built in its phase. This makes the provider adapter a record-aware design
requirement so provider-backed replay can be exact.

Defer (staged, behind the seams above): `slog` injection + correlated lifecycle
logs, a no-op `Observer` port for metrics/traces, and log secret-redaction.

## Consequences

- The SQLite schema (Phase 2) serializes `TurnID` from first write — no later
  migration for correlation.
- Every observability plane is joinable by turn from day one.
- The engine gained one field of plumbing and one helper; behavior is unchanged
  (verified: full suite green under `-race`).
- Provider adapters must be built to record raw responses keyed by `trace_id` for
  exact replay — captured now so it is not bolted on.

## Alternatives rejected

- *Persist `trace_id` on the domain Event*: pollutes the domain type with an
  operational concern; the domain joins on `SessionID+TurnID`. Rejected.
- *Defer correlation entirely*: would force a cross-codebase retrofit after
  persistence lands. Rejected — this is the cheap-now/expensive-later moment.
- *Build the full telemetry stack now*: premature; one seam now, exporters later.
