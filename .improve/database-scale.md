# Phase 7: database scale

**Status: source-SQL scale suite and narrow cache-query improvement verified;
full native integration and remaining query rewrites are blocked.**

The retained [suite, assumptions and reproduction commands](load-test/README.md)
uses fresh generated local databases only. It extracts actual Rust DDL/SELECTs,
rejects stale shapes, checks relationships and missing-room defaults, and measures
SQLite 3.46.0 from the root-lock bundled source. It never accepts a service URL,
existing database path or arbitrary SQL. SQLite loop visits are measured with
`SQLITE_SCANSTAT_NVISIT`; they are logical visits, not physical disk reads.

## Change and evidence

Two room-document cache callers need only `(head, floor)`, but previously called
`room_state`, which also reads checkpoint metadata and scans retained row sizes.
A shared position helper now performs the existing indexed head/floor SELECT.
The full statistics method still takes one lock and performs its original work;
compaction, quotas, protocol payloads, schema and authorization are unchanged.
Missing rooms still yield `(0, 0)` and database errors still propagate.

History and registry scale independently at 1×/10×/100×/1000×, each reaching one
million retained rows. Ratios are declared synthetic assumptions, not production
measurements: half two-human DMs, half eight-member groups; the registry stress
owner belongs to every room. Checkpointed rows are not retained below the floor.
Synthetic blobs exercise SQL storage, not valid Loro decoding.

| History scale | Group p50 before → after | Logical visits before → after |
| --- | ---: | ---: |
| 1×: 100 retained rows/room | 17 → 4 µs | 102 → 1 |
| 10×: 1,000 rows/room | 49 → 4 µs | 1,002 → 1 |
| 100×: 10,000 rows/room | 380 → 6 µs | 10,002 → 1 |
| 1000×: 100,000 rows/room | 5,983 → 31 µs | 100,002 → 1 |

Each changed query has 101 randomized paired samples per scale and DM/group
variant, warmups, p50/p95/p99, raw samples, plans, a paired bootstrap 95% interval
and two-sided exact sign test. At the largest history fixture, the group median
saving is 5.956 ms (95% interval 5.937–5.981 ms), p=7.89e-31. This is SQL-only
work, excluding relay lock waits, Rust allocation, transport, CRDT decoding and UI.
It does not establish that the reported app delays are fixed.

The unchanged roster control exposes an N+1 flow: `chats_of_member` followed by
`members_of` per chat. At 10,000 rooms it executes 10,001 statements, visits 70,000
logical rows and has p50 90.845 ms / p95 94.058 ms / p99 100.503 ms. It remains an
explicit failure to address after native membership/order regressions can run;
there is no claim of improvement for the unchanged control.

## Box-worker lookup index

The supplementary profile also found `Store::job_for_subagent` scanning all jobs
and sorting to return one worker's newest row. The only local-box source change
adds `jobs_subagent_created(subagent_id, created_at DESC)` to the existing
idempotent schema. The SELECT and row mapping remain unchanged. SQL regressions
compare complete rows for latest selection, equal timestamps, NULL/missing workers,
and fresh/existing-store schema replay, through both Python SQLite and bundled
SQLite 3.46.0. The production Store adapter is still unexecuted.

At 100 / 1,000 / 10,000 / 100,000 jobs, p50 changes from 12 / 29 / 187 / 4,260 µs
to 8 / 8 / 8 / 39 µs; logical visits fall from the total job count to one. There
are 101 randomized pairs per scale; all four paired-median saving intervals are
positive, with maximum sign-test p=4.06e-27. At 100,000 jobs the saving interval is
4.192–4.268 ms. The index adds 970,752 bytes to that synthetic fixture. One
existing-schema replay including index creation takes 36.66 ms locally; this is
a single sample, not a write-throughput claim. Index maintenance adds write work.
See the [self-contained worker chart](load-test/job-index.html) and retained
`JOB_INDEX_RESULTS.json` for percentiles, raw pairs and query plans.

Supplementary unchanged probes retain exact unread, common-term full-text search,
and open-job SELECTs at all four scales, 41 samples each. They expose remaining
work rather than claim fixes: unread returns three visible chats but visits
200,033 rows at 1000×; thread-filtered common-term FTS visits 200,000 for six rows;
open jobs returns 10,000 rows. Membership/filter semantics, FTS ranking/refusals,
and unbounded-result API behavior require dedicated native regression coverage
before broader rewrites. See the suite's coverage report for metrics and limits.

## Before / after gates

| Gate | Before | After |
| --- | --- | --- |
| Formatting / whitespace | baseline workspace fmt pass 3.016 s | workspace fmt and diff check pass; final duration not separately recorded |
| Main SQL-suite tests | new suite, initial 8 passed/0 failed/skipped, 8.138 s | 8 passed/0 failed/skipped, 6.028 s; final cached rerun 0.351 s wall |
| Supplementary SQL checks | new suite; baseline profile retained | 6 check groups passed, 0 failed; final 0.114 s wall |
| Index equivalence/replay checks | old-DDL latest/tie/NULL/default checks pass before edit | 6 check groups pass, 0 failed; final 0.108 s wall |
| Main scaled fixtures | original exact SELECTs retained | 8/8 equivalence/integrity fixtures pass; 3,568 timed executions |
| Worker-index scaled fixtures | 4 old-schema fixtures | 4 equivalent new-schema fixtures, 808 timed executions |
| Full native lint/typecheck/test/build | blocked/unmeasured | blocked/unmeasured; two Rust regressions written, not executed |

The new suite's gate timings include cache/compiler differences, not application
speedups. Counts of regression tests and grouped SQL checks are kept separate.

## Verification boundary

Native regressions pin missing-room defaults, append/checkpoint head/floor and
statistics, and database-error propagation. They are written but **not executed**:
the full Rust build remains blocked by disk capacity and fork-CI approval. SQL
fixture/equivalence tests exercise actual queries, not the Rust mutex/adapter or
Loro cache lifecycle. Full native clippy/check/test/build counts are unmeasured.
The retained suite reports its own checks separately from these native gates.

The [scope review](reviews/load-test-scope.md) inventories other relay, local-box,
device-cache and external homeserver families. Supplementary probes and their
limits are retained with the suite. Unmeasured paths are not declared scalable.
No shared migration, production/staging connection or service change was made.

## Needs the owner

Recommended default: approve existing fork CI or provide native build capacity,
then run the two Rust regressions and room-cache integration gates before merging.
Prioritize a privacy-preserving batched roster read with pinned membership and
ordering behavior next. Preserve current authorization and durability semantics.
