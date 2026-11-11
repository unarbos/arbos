# Unread index experiment: withheld

The unchanged unread query scans global events as unrelated rooms grow. One
experimental partial index on `(project_id,id) WHERE project_id IS NOT NULL`
enables SQLite's `MULTI-INDEX OR` plan, but the complete workload matrix shows
that it is unsuitable as an unconditional default. **The product schema and
query stay unchanged.**

The [self-contained comparison](unread-index.html), [export](unread-index.png)
and [raw pairs/plans](UNREAD_INDEX_RESULTS.json) retain all 16 cases. Each has
101 randomized pairs and five warmups per arm at scales 1/10/100/1000.

| Workload at 1000× | Original p50/p95/p99 ms | Candidate p50/p95/p99 ms | Saved median 95% CI ms |
| --- | ---: | ---: | ---: |
|People-heavy, sparse ACL|19.974/20.409/21.018|0.090/0.120/0.128|19.848 to 19.924|
|People-heavy, broad ACL|67.941/69.097/69.726|76.954/78.971/80.215|-9.467 to -8.903|
|Mixed legacy, sparse ACL|50.542/51.657/54.911|0.110/0.148/0.150|50.266 to 50.535|
|Mixed legacy, broad ACL|144.332/147.927/149.512|158.781/163.107/164.407|-14.532 to -13.363|

Negative savings mean the candidate is slower. Every broad-access case regressed,
including small fixtures. At the largest people-heavy sparse fixture, logical
visits fall 200,033→73; with mixed history they fall 400,033→73. Broad visibility
visits the same rows before/after and the candidate needs temporary grouping.
This is a measured tradeoff, rather than an external test blocker. A sparse-only
comparison would conceal it.

## Reproduce on fresh temporary files

```sh
python3 .improve/load-test/unread_index.py --check
python3 .improve/load-test/unread_index.py
python3 .improve/load-test/render_unread.py
cargo test --locked --offline -p arbos-relay unread_query_tests
```

Use the same local prerequisites as the main suite and a verified network fence
for native test execution. Run the timed profiler in an exclusive CPU window.
The script accepts no DB path, DSN, SQL or destination. It verifies the current
source query/schema against captured source snapshots, then applies the withheld
candidate **only to newly generated temporary databases**. It works with the
unchanged product schema; it does not require adding the index to the app.

`UNREAD_BEFORE_SOURCE.json` and `UNREAD_CANDIDATE_SOURCE.json` capture the exact
original/experimental source schema and identical SELECT. The candidate was
compiled and tested privately. Its index-existence tests are not committed.
Two actual native characterization tests are retained: owner/live-guest and
people-membership ACLs, overlap deduplication, absent/left/archived cases,
read/own/blocked/NULL-sender filtering, ordering, and the full projected winner
for schema-permitted legacy mixed-project metadata. Both passed after a forced
workspace-package rebuild in a unique target; Clippy over all targets with
warnings denied also passed. Historical shared-target Cargo results are not
used as proof.

## Workloads, checks and limits

The original fixture has 10×scale people groups and 20 events per group; the
target sees two groups, one archived, and two project chats. Broad cases join
every group. Mixed history additionally creates one valid legacy project chat
per group-agent project, with 20 events each; broad cases join those projects as
live guests too. Senders are current owner/guests or a NULL agent sender, and
the original own-message, block, archive and partial-read controls remain. The
declared proportions are synthetic, not production observations.

Both arms use fresh identical databases. Complete ordered rows agree through
production bundled SQLite 3.46.0 and independent seed SQLite 3.53.4. The largest
two-fixture pair is 108,072,960 bytes. All temporary files were deleted after
each case. Mixed 1000× fixtures add 5,828,608 index bytes; single observed index
creation was 62.217/84.716 ms for sparse/broad variants. Those creation timings
use seed SQLite and are not production startup/write-throughput guarantees.

Timed samples include Python ctypes preparation/binding/column consumption and
scanstatus instrumentation. They exclude Rust mapping, mutex contention,
concurrent writers, network and rendering. Intervals use a seeded 2,000-resample
paired-median bootstrap; exact two-sided sign tests exclude ties. They summarize
one coordinated local warm single-reader run, not production confidence bounds.
Source hashes, raw samples, order, complete percentiles, counter values and plans
are in the JSON. No app speedup is claimed.

Merging the two project allowlists with a UNION is a small logical factoring,
but it still leaves the global event scan without the index. Avoiding both the
sparse scan and broad regrouping would need a larger strategy selection or
query redesign; no adaptive framework or unverified skew assumption is added.
