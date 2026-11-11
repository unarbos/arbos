# Phase 7: database scale

**Status: position/profile projections, roster batching and three lookup indexes
verified with local scale profiles. Actual native relay checks pass; final combined
workspace and physical app measurements remain separate gates.**

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

## Batched room rosters

`chats_of_member` now fetches its unchanged ordered chat list and all visible live
rosters in two SELECTs, instead of calling `members_of` once per chat. Both reads
use the existing membership mapper and the same live-member/nondeleted-chat
scope. The first query controls chat order, and the second preserves each
roster's existing total order. No permission, payload or pagination policy changes.

The [roster comparison](load-test/roster.html) retains 101 randomized pairs per
scale and complete ordered-row checks for owner, peer, agent and missing users.
At 10,000 rooms, p50 is **84.308 → 27.615 ms (3.05×)** and p95 is
89.277 → 29.275 ms, with **10,001 → 2 statements** and the same 60,000 returned
chat/member rows. Paired median saving 95% interval: 56.538–56.852 ms;
two-sided sign-test p=7.89e-31. All four scales improve.

Repeated scope validation raises logical visits 70,000 → 90,000 and VM steps
2,165,015 → 2,465,039. Native code now holds the connection mutex through both
queries and mapping instead of releasing it between rosters. These SQL profiles
exclude Rust mapping, concurrent writers, mutex waits, network and rendering;
they establish neither device latency nor a complete group-open improvement.

Three actual Rust regressions cover complete list/detail JSON, membership and
visibility boundaries, ordering/fields, and SQLite statement counts. Before the
fix, two behavioral controls passed and the 11-versus-2 statement budget failed.
The fixed code passes all three. A root verification additionally rebuilt the
exact applied source in a unique target with all workspace packages cleaned.
Its test-only trace callback has no borrowed context pointer, so unwinding cannot
leave SQLite pointing at freed counter storage.

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

## Measured changes withheld

The [unread experiment](load-test/UNREAD_RESEARCH.md) measures 16 combinations of
scale, sparse/broad ACLs and people-heavy/mixed legacy history. An experimental
partial project index greatly helps sparse access but **regresses every broad
case**, so it is not added to the product. At mixed 1000× scale, sparse p50 is
50.542 → 0.110 ms, while broad p50 is 144.332 → 158.781 ms; the broad paired
median saving interval is entirely negative, −14.532 to −13.363 ms. All cases,
raw pairs and plans remain in the [four-panel comparison](load-test/unread-index.html).
Two actual native tests pin the existing ACL/filter/full-row contract. This is
a measured workload tradeoff, not a missing-test blocker. Default: withhold it.

A thread-filtered FTS pushdown was also withheld: `Store::scrub_all_text` updates
authoritative rows before rebuilding FTS under separate autocommits. An interrupted
scrub can leave metadata stale, so adding a metadata filter could silently drop
matches that the existing authoritative-row guard returns. Do not claim a global
FTS mirror invariant or ship that optimization without addressing durability.

Unchanged supplementary probes retain open-job SELECTs at all four scales;
returning 10,000 open jobs is still an unbounded-result contract. A cap or paging
change would change callers' behavior. Other storage families and concurrent
writer/WAL pressure are unmeasured workloads, not evidence of scalability.

## Before / after gates

| Gate | Before | After |
| --- | --- | --- |
| Formatting / whitespace | baseline workspace fmt pass 3.016 s | workspace fmt and diff check pass; final duration not separately recorded |
| Main SQL-suite tests | new suite, initial 8 passed/0 failed/skipped, 8.138 s | 8 passed/0 failed/skipped, 6.028 s; final cached rerun 0.351 s wall |
| Supplementary SQL checks | new suite; baseline profile retained | 6 check groups passed, 0 failed; final 0.114 s wall |
| Index equivalence/replay checks | old-DDL latest/tie/NULL/default checks pass before edit | 6 check groups pass, 0 failed; final 0.108 s wall |
| Main scaled fixtures | original exact SELECTs retained | 8/8 equivalence/integrity fixtures pass; 3,568 timed executions |
| Worker-index scaled fixtures | 4 old-schema fixtures | 4 equivalent new-schema fixtures, 808 timed executions |
| Roster regressions | isolated original: 2 passed / 1 budget failure / 0 ignored, 39.829 s | exact applied source: 3 passed / 0 failed/ignored, 41.230 s including compilation |
| Native unread / position characterization | preserved behavior; original position gate previously unrun | 2 unread passed, 0.719 s; 2 position passed, 0.290 s; no failures/ignores |
| Relay/core all-target Clippy | original full native baseline was unavailable | exact applied source passed with warnings denied, 28.176 s |
| Extended Python suite | initial 8 passed | 12 passed / 0 failed/skipped, 10.778 s |
| Supplementary/index/unread contracts | original SQL retained | 6 supplementary + 6 index groups pass; unread source contract passes |
| Full combined workspace / native UI | initial disk/CI limits, now partly resolved | final combined branch pending; these scoped rows are not whole-app gates |

The new suite's gate timings include cache/compiler differences, not application
speedups. Counts of regression tests and grouped SQL checks are kept separate.

## Verification boundary

[NATIVE_GATES.json](load-test/NATIVE_GATES.json) records source hashes, exact
selected counts and commands. Dependencies were cloned into a new target and all
12 workspace packages cleaned before root verification. Earlier cross-workspace
shared-target results are superseded after an expected-count guard caught a stale
binary. No zero-test filter was accepted as a pass. Author-isolated broader DB
34 passed/1 pre-existing live test ignored, people 46 passed, package check/build
and all-target Clippy also passed. Counts overlap with selected checks and must
not be added as independent coverage. Worker-index SQL replay/equivalence checks
are distinct from executing the production Store adapter.

The [scope review](reviews/load-test-scope.md) inventories other relay, local-box,
device-cache and external homeserver families. Native DeviceStore/Mac persistence,
Matrix/Synapse and physical app timing require dedicated checks. No shared
migration, production/staging connection or service change was made by this suite.

## Needs the owner

Recommended default: keep the unread index withheld and preserve current FTS and
unbounded-result behavior. Run a representative device group-open/send trace and
concurrent writer benchmark before attributing user-visible latency to these SQL
savings. Security-sensitive membership behavior is pinned by failing-before or
characterization tests and deserves maintainer review before merge. The additive
live-device/history indexes also need storage/startup/write-budget review; default
recommendation is to retain the measured indexes and review rollout timing. Those
follow-up production/device decisions do not block these local contribution PRs.


## Whole chat-list DTO work

The two-query roster result measures storage. The actual handler also used
repeated profile, device-presence, agent-owner and block lookups: eight rooms
used 186/714 statements for 2/24 humans. Request-local public profile batching
reduces both to 69. The live-device user-id index avoids unrelated-account scans
while leaving the frequently updated timestamp outside its key and preserving
legacy tied-device order. Shared original DTO/name/avatar policy stays in one
assembler; a malformed card and valid presence (or the reverse) still fail
independently. Independent source review passes.

| Generated groups | Native handler + JSON p50 before → after | Statements | Logical visits |
| --- | ---: | ---: | ---: |
| 1 | 0.945 → 0.749 ms | 96 → 13 | 868 → 195 |
| 10 | 6.360 → 3.645 ms | 942 → 85 | 8,672 → 970 |
| 100 | 72.531 → 31.978 ms | 9,382 → 805 | 604,715 → 9,498 |
| 1,000 | 1,832.479 → 321.528 ms | 93,822 → 8,005 | 57,962,963 → 94,778 |

All complete JSON bytes match at every scale and sample. There are 21 randomized
pairs per scale; all paired median-saving intervals are positive, sign-test
p=9.54e-7 per scale. At 1,000 rooms the paired saving interval is
1,502.109–1,520.269 ms. Workspace opt0/dependencies opt2, warm workers, no exclusive
host; these numbers omit transport and UI. The component is pinned to d499090b →
b96ef1f90, before the activity index below. See the
[whole-handler chart](load-test/people-dto.html), raw PEOPLE_DTO_RESULTS.json and
[reproduction notes](load-test/PEOPLE_DTO.md). The 8,005 remaining statements show
per-room metadata work honestly; this is not a constant-query complete-list claim.

## Last-activity history index and costs

The existing activity query needs the maximum timestamp, not the last sequence's
timestamp. `rows(room, created_at)` lets its unchanged SQL read that maximum from
a covering index. At 100/1,000/10,000/100,000 retained rows per room, SQL p50 is
13→9 / 41→10 / 321→12 / 5,503→41 µs. Logical visits become 2 at every scale,
from 101/1,001/10,001/100,001. Each scale has 31 randomized pairs and positive
paired saving intervals; at the largest scale the saving interval is
5.413–5.502 ms, p=9.31e-10. Nonmonotonic/negative timestamps, maximum deletion,
missing/empty defaults, exact row preservation and index replay are checked.

This has a real cost. At one million generated rows the index adds 27,045,888
bytes; schema replay/index creation took 0.280 s with Python SQLite. In a separate
bundled SQLite3.46 WAL/NORMAL/secure_delete SQL append sample, p50 rises
26.158→34.627 µs; paired median added cost is 8.926 µs (95% interval
5.415–10.908 µs), p=9.54e-7. That loop approximates one-row transactions, including
Python dispatch/SQL preparation; it is not native sending latency. The index is
chosen for recurring history reads. Production read/write ratios and total history
size are unmeasured, so storage, startup DDL and write budget need review before
shared rollout. No shared migration was applied.

See the [activity chart](load-test/activity-index.html), ACTIVITY_RESULTS.json and
runnable `load-test/activity_index.py`. The final exact seven-source native filter
passes 10 cases in 28.349 s (including compile); all-target Clippy passes in
16.249 s. The actual pre-index regression failed on 15,025 VM steps over 2,500
rows; the indexed budget and full timestamp/deletion controls pass. The six SQL
compatibility groups also pass. Tests were preserved.
