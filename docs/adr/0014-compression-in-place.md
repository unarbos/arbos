# ADR-0014 — Compression is in-place, not session rotation

- Status: Accepted
- Date: 2026-06-08

## Context

Context compression is core to any long-running agent. The Python original
(Hermes) implemented it by **rotating the session**: on compression it marked
the old session ended, created a child session, copied surviving messages
forward, and needed a SQLite **compression lock** to stop concurrent
compressions from spawning orphan sessions. Compression logic was also triggered
from four scattered call sites. Our own early model carried that assumption
(`Session.ParentID`/`SessionCompressed` described as "compression lineage")
while the event log already had a `CompressionPayload` — an unreconciled
contradiction.

## Decision

Compression is an **in-place log append**. To compress, append a
`CompressionPayload{Summary, ReplacedSeqLo, ReplacedSeqHi}` to the live
session's log. `core.Project` folds the `[lo,hi]` span away and renders the
summary at the span's start. Consequences:

- The session **stays `Active`** across any number of compressions. No child
  session, no row copying, no orphan sessions, no compression lock.
- The raw events remain in the log, so compression is **reversible** (re-project
  without folding for trajectory/training/audit) — the log stays the single
  source of truth.
- `SessionStatus` drops `compressed`; statuses are `active` and `ended`.
  `Session.ParentID` is repurposed for explicit forks/branches, not compression.
- Re-compression nests: a later, strictly larger span subsumes an earlier
  summary (the projection renders only the outer summary).
- The **decision** of when/how-much to compress is owned by one seam, the
  `ContextPolicy` port (the actual summarization runs via an auxiliary model in
  that phase) — replacing Hermes's four scattered triggers.

## Consequences

- `Project` now honors `ReplacedSeqLo/Hi` (previously it appended the summary
  without folding — a correctness gap, now closed and tested).
- Determinism: `Project` is pure, so compression behavior is golden-testable and
  fuzzed.
- Open: the `ContextPolicy` engine wiring and auxiliary-model summarization land
  in the compression phase; this ADR covers the data model and projection only.

## Alternatives rejected

- *Session rotation* (Hermes): orphan-session risk, lock complexity, dual
  persistence, lossy (raw turns discarded). Rejected.
