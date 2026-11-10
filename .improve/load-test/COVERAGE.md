# Supplementary unread, box search and job probes

These probes broaden the [main room/registry profile](latency.html) with
independent relay unread and per-box FTS/job query families. They generate new
temporary databases only, accept no database path or service destination, use
exact extracted source SELECTs, and reuse the main suite's cached production
SQLite 3.46.0 build. The largest supplementary fixture was 59,625,472 bytes;
temporary files were deleted between scales.

```sh
python3 .improve/load-test/coverage.py --check
python3 .improve/load-test/coverage.py
python3 .improve/load-test/job_index.py --check
python3 .improve/load-test/job_index.py
python3 .improve/load-test/render_job_index.py
```

Scripts derive paths from their location and were also run from `/tmp`. They
require the main suite's local prerequisites; nothing is downloaded. Profiles
own their CPU timing window and should run sequentially. `coverage.py` reruns
the current source and overwrites `COVERAGE_RESULTS.json`; its committed result
was captured **before** adding the worker index. `job_index.py` retains original
DDL/query in `JOB_INDEX_BEFORE.json` and compares it with the actual current
source DDL. Its `--capture-before` operation refuses to overwrite that snapshot.

## Workload assumptions

These are declared synthetic proportions, not measurements of production:

- Relay growth: 10×scale group chats, 20 announced events per group. The target
  user belongs to two groups, one archived, and has read half their events. A
  blocked peer, the target's own messages, another peer and an agent exercise
  sender filtering. Distinct group owners each own an agent project. Two
  separate project chats exercise project ownership and live guest ACL branches.
  The visible result stays three chats (6, 1 and 1 unread), while unrelated
  events grow from 200 to 200,000. Generated timestamps span under seven days,
  below the source event log's 30-day retention window.
- Box growth: 10×scale chats, each with a worker/session, ten t3 rows and ten
  jobs. Every t3 entry has its FTS mirror. History kinds include user, assistant
  and refused user rows. A shared common term stresses ranking and the global
  FTS candidate set. One latest job per worker is running, nine earlier jobs
  are complete, and worker-history rows link all jobs. History and job counts
  each grow from 100 to 100,000. This is one large per-box store, not a
  relay-wide population distributed over many boxes.

The supplement's ctypes runner binds multiple parameters without rewriting SQL,
consumes result columns, records p50/p95/p99 from 41 warm repetitions, and
captures actual NVISIT logical visits, VM steps and plans. Absolute latency
includes Python ctypes result consumption and scanstatus instrumentation; it
does not model Rust allocation/JSON parsing. Compare scaling within a probe,
not absolute timings against the main C-only runner. No before/after change or
significance claim is made for these baseline-only probes.

Recorded p50 milliseconds (complete percentiles/raw samples in
[COVERAGE_RESULTS.json](COVERAGE_RESULTS.json)):

| Exact query | 1× | 10× | 100× | 1000× | Logical visits 1× → 1000× |
| --- | ---: | ---: | ---: | ---: | ---: |
| Relay unread | 0.054 | 0.216 | 1.909 | 20.501 | 233 → 200,033 |
| Box FTS global, limit 30 | 0.163 | 0.607 | 5.540 | 59.533 | 200 → 200,000 |
| Box FTS one thread, six results | 0.071 | 0.364 | 3.904 | 42.843 | 200 → 200,000 |
| Box latest job for one worker | 0.018 | 0.037 | 0.434 | 4.485 | 100 → 100,000 |
| Box open jobs | 0.049 | 0.401 | 4.010 | 42.061 | 100 → 100,000 |

The unread plan scans the `chat_events_chat` index even though the target sees a
fixed three-chat result. FTS scans global matches and joins t3 by id before
thread filtering/ranking. The original worker lookup scans jobs and builds a
temporary sort for a one-row result. `open_jobs` scans and sorts too, but also
returns a growing 10–10,000 rows. These are concrete remaining scaling leads,
not proof of observed user latency. Unread ACL/privacy predicates, FTS ranking
and open-job output semantics were preserved.

## Narrow worker index change

The worker lookup had a directly verified additive fix: one production schema
line creates `jobs_subagent_created(subagent_id, created_at DESC)`. The SELECT,
projection and LIMIT are unchanged. Existing `Store::open` replays the schema,
so old stores receive the index automatically; repeated replay is idempotent.

Original actual DDL and winner behavior were pinned before editing. Six
retained regression groups pass after editing, including fresh and existing
schema replay, complete projected-row equality, equal-created-at ties, NULL
workers, missing workers, and preservation of populated old data during upgrade.
Full-row equality is checked through both Python SQLite 3.53.4 and production
bundled SQLite 3.46.0. The tied winner remains the first inserted row. No query
tie-breaker or state behavior changed.

The [worker comparison chart](job-index.html) and
[raw paired results](JOB_INDEX_RESULTS.json) compare the same generated fixture
on old and actual new schemas. Preparation, binding, stepping, column
consumption and finalization are timed in C, matching the main engine.
There are 101 randomized old/new pairs per scale, five warmups, seeded 2,000-
resample paired-median bootstrap intervals and exact two-sided sign tests.

| Jobs | Old p50/p95/p99 ms | Indexed p50/p95/p99 ms | Saved median 95% CI ms | Visits old → indexed |
| ---: | ---: | ---: | ---: | ---: |
| 100 | 0.012/0.015/0.024 | 0.008/0.009/0.023 | 0.004–0.005 | 100 → 1 |
| 1,000 | 0.029/0.033/0.048 | 0.008/0.009/0.021 | 0.021–0.021 | 1,000 → 1 |
| 10,000 | 0.187/0.195/0.219 | 0.008/0.011/0.016 | 0.178–0.180 | 10,000 → 1 |
| 100,000 | 4.260/4.519/4.817 | 0.039/0.051/0.056 | 4.192–4.268 | 100,000 → 1 |

All four saved-median intervals are positive. The maximum unadjusted sign-test
p is 4.07×10⁻²⁷; Bonferroni adjustment across four comparisons is at most
1.63×10⁻²⁶. Plans change from `SCAN jobs` plus temporary ORDER BY sorting to
`SEARCH jobs USING INDEX jobs_subagent_created (subagent_id=?)`.

The index costs space and maintenance on insert/worker reassignment. After all
timed baseline reads, replaying actual new SCHEMA on the populated old fixture
added 4,096 / 24,576 / 184,320 / 970,752 file bytes at the four sizes. That
one-time schema replay plus index creation took 0.921 / 0.955 / 3.946 / 36.660
milliseconds. These are single local observations, not startup guarantees or a
separate write-throughput benchmark. Both temporary fixtures together stayed
below the 300 MiB budget. No index was added to unread, FTS or `open_jobs`.

## Verification and remaining coverage

[COVERAGE_GATES.json](COVERAGE_GATES.json) records the six supplementary check
groups passing in 0.116 seconds and six index check groups passing in 0.113
seconds, zero failures. The pre-edit source snapshot passed its original
fresh/replay/latest/tie/missing/NULL assertions; its duration was not separately
instrumented, so no before-duration claim is made. These are newly added
SQL-boundary checks, not pre-existing native tests.

At the original supplementary capture, native workspace/FFI/GPUI execution was
unrun. Later targeted native roster/unread gates use unique source-pinned targets;
current full workspace/platform CI status is reported by the parent phase. Device blob restore/eviction, valid Loro snapshots and
concurrent writers/WAL pressure are outside these SELECT profiles. Synapse's
external PostgreSQL and Matrix SDK in-memory timelines are separate systems.
Other relay/box families in the scope review remain unmeasured. These limits
do not erase the runnable SQL checks or turn them into a whole-app latency claim.


## Unread index tradeoff follow-up

The [withheld-index study](UNREAD_RESEARCH.md) and [all-case chart](unread-index.html)
add101paired comparisons at four scales for sparse/broad ACLs and people-heavy/
mixed legacy histories. The partial project index makes sparse reads bounded but
regresses all broad-visibility cases, so no unread index is added. The source SELECT
and schema remain unchanged; two actual native ACL/full-row characterization tests
are retained. The portable profiler applies the captured candidate only to newly
generated temporary files. This is a measured design decision, not a claim that
local testing is blocked or that synthetic ratios describe production.
