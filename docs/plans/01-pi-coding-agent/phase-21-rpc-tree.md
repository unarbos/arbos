# Phase 21 — RPC tree commands (fork / switch_session)

Back to [overview](./overview.md). Status: done. ADR-0027.

## Changes (as implemented)

- `internal/sessiontree`: `Fork`/`Clone` relocated here (shared by engine + agent
  layer; the agent's branch-summary stays in `internal/agent/pi`).
- `engine.ForkSession` / `CloneSession`: control-plane copy over the engine's
  store.
- `control.Serve`: new `fork` and `switch_session` frames with `forked` /
  `switched` responses. Serve restructured to bind one session at a time via a
  per-session sub-context, tearing down the current actor and its event pump
  before binding the target.

## Verification

Static gate green, no new lints, existing control-seam tests still pass with
-race. Runtime over the real seam: open A + prompt (3 events) -> fork to B
(forked + switched, B has A's history and `ParentID=A`) -> a turn on B left A
unchanged (branch isolation) -> switch_session back to A. The full non-tree +
tree RPC surface is now carried by the seam.
