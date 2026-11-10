# Phase 3: structural refactoring

**Status: partial implementation; whole-codebase exit blocked.** Native
durability, authorization and room-ownership findings remain worth fixing. Full
native gates cannot run with the available local disk; upstream fork CI requires
maintainer approval. These are not passing exit checks.

Scope and all findings are in [backend review](reviews/structure-backend.md),
[client review](reviews/structure-clients.md) and
[auxiliary review](reviews/structure-auxiliary.md). Broad inventories and focused
flow reads covered all domains; this does not claim every line was audited.
Problems and grouped redesigns were written before source changes.

## Implemented shared seams

- Mac chat titles: `app.rs` and `sync/fixture.rs` had identical first-line,
  trim/34-character/fallback rules. `sync/relay.rs` imported this stable policy
  from the volatile GPUI app module. All three now use dependency-free
  `chat_title.rs`. The implementation is unchanged; eight behavior checks pin
  empty/first-line/CRLF/Unicode/truncation behavior before and after extraction.
- Harness cancellation evidence: `StopJobResult`, `sweepClause` and `stopJobText`
  now live together in import-free `stop-job-text.ts`. Pure callers can use this
  evidence-preserving presentation rule without initializing environment-driven
  pi registration. `index.ts` keeps its existing named exports and uses the same
  function. All real-hook integration tests stay in place.

No generic framework, configuration layer or speculative adapter was added.
Effect is not introduced into these pure Rust/TypeScript seams. Native code stays
native; a later development-only Effect FastCheck dependency is appropriate for
generated TypeScript laws, without converting stable runtime code.

## Comments and Tests

Both selected additions were applied in order:

| Addition | Requested description | Result |
| --- | --- | --- |
| Comments | Remove comments that only restate the code. | Reviewed touched seams; retained external protocol/incident/absence semantics and calibration explanations. No broad comment deletion justified behind blocked native checks. |
| Tests | Remove tests that wouldn't catch a real bug, and fix the ones that fail without one. | Reviewed last. Kept all 14 harness tests, including real-hook and negative controls. Client audit identifies test-only scheduler models needing production-seam replacements; native gate blockers prevent safe replacement now. No tests removed. |

## Before / after

Times are gate durations, not speedup measurements; warm filesystem/tool caches
make comparisons unsuitable for application performance claims.

| Row | Before | After |
| --- | --- | --- |
| Harness typecheck | pass, 3.498 s | pass, 2.889 s |
| Harness build (`tsc --noEmit`) | pass, 3.916 s | pass, 2.456 s |
| Harness tests | 14 passed, 0 failed/skipped; 0.711 s | 14 passed, 0 failed/skipped; 0.530 s |
| Mac title behavior | 8 passed, 0 failed/ignored; original function | 8 passed, 0 failed/ignored; actual shared module |
| Title test compilation / execution | 0.664 / 0.249 s | 0.114 / 0.210 s |
| Workspace formatting | baseline pass, 3.016 s | pass, 2.607 s |
| Full native clippy/check/test/build | blocked | blocked; no passing counts claimed |
| Title definitions / UI dependency | 2 copies; sync → app | 1 shared definition; sync → pure helper |
| Cancellation renderer runtime imports | bundled in environment-loaded extension | 0 in the pure module |
| Intended behavior | baseline | unchanged |

The dependency direction is improved at these two seams. It is not reasonable
to claim every import or special case across the entire repository is resolved.
Independent review must distinguish these verified extractions from the blocked
whole-repository exit condition.

## Needs the owner

Approve fork CI or provide enough local native build space, then resume the
ranked native findings in the linked reviews. Recommended default: preserve
authorization, persistence, protocol compatibility, physical timing and all
existing tests; address the two-room ACK and durable-append regressions first.
