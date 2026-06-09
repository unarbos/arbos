# Phase 20 — Session tree (fork / clone / branch summaries / navigate)

Back to [overview](./overview.md). Status: done. No ADR or schema change needed.

## Design (data shapes first)

pi keeps a tree of entries inside one session file and moves a leaf pointer.
arbos models the same capability as a set of parent-linked sessions: a branch is
an independent, complete session log. The foundational win is that this needs NO
event-log or store schema change, because `core.Session` already reserves
`ParentID` for fork lineage and the SQLite store already persists `parent_id`.
Branch=session also means switching branches loses no context (each log is
complete), so the pi branch-summary-on-navigate is a context aid, not a
correctness requirement.

## Changes (as implemented)

- `internal/agent/pi/tree.go`: `Fork(store, source, newID, throughSeq)` copies
  the source's events with `seq <= throughSeq` into a new session parented to the
  source (copied events keep payload/turn/version/timestamp; the store reassigns
  per-branch seq). `Clone` forks through the whole log. `BranchSummary` ports
  pi's branch-summary prompt + preamble and appends file tracking.

(Phase 21 relocates `Fork`/`Clone` to a shared `internal/sessiontree` package so
the engine and control seam can reach them.)

## Verification

Static gate green, no new lints. Runtime over the real SQLite store: fork through
seq 1 produced a 2-event prefix branch parented to the source; clone produced a
full 4-event copy; branch-summary produced the preamble + structured checkpoint.
