# ADR-0027 — Control-seam fork and switch_session

- Status: Accepted
- Date: 2026-06-08

## Context

pi's RPC surface includes session-tree navigation: `fork` (branch from a point)
and `switch_session` (move to another session). These are control-plane
operations, not turn intents — they change *which* session a connection drives
and manipulate the store — so they belong at the control seam, not as engine
Intents (which a single session actor consumes).

## Decision

Extend the control protocol with two client frames and one engine capability.

- `engine.ForkSession` / `CloneSession` copy a session (whole or a seq-bounded
  prefix) into a new branch via `internal/sessiontree` (Phase 20), recording
  `ParentID` lineage. Fork/Clone live in `sessiontree` so both the engine and the
  agent layer use one implementation with no import cycle.
- `control.Serve` gains `fork` (`{session_id?, new_session_id?, through_seq?}`)
  and `switch_session` (`{session_id}`) frames, plus `forked` / `switched`
  responses. Serve was restructured to bind one session at a time with a
  per-session sub-context: a switch/fork tears the current actor down (cancel +
  wait its event pump) before binding the target, so two pumps never write to one
  connection. Fork tears down the source first so its log is stable for the copy,
  then forks and switches to the new branch (matching pi, which forks then
  continues on the fork).

## Consequences

- The control seam now carries the tree-dependent pi RPC commands. Combined with
  ADR-0026, the seam covers pi's full non-trivial RPC surface (prompt, events,
  set_model, compact, abort, follow_up/steer, fork, switch_session).
- No store schema change (Phase 20 reused `ParentID`). The protocol additions are
  new frame types; existing `open`/`intent` behavior is unchanged and its tests
  still pass.
