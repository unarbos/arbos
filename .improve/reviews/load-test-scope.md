# Phase 7 independent database scope and workload review

This review inventories source schemas, query relationships and the local
load-test suite. It also implements the approved supplementary generated-local
SELECT probes and one additive box-worker index after capturing its baseline.
No existing application database, credentials, production/staging endpoint or
service was accessed. Profiling windows were serialized between agents. All
fixture proportions are synthetic assumptions, not observed production
proportions.

## What the targeted suite can establish

The main suite extracts relay `SCHEMA` and `PEOPLE_SCHEMA` and exact SQL from
`Db::room_state`, `Db::members_of` and `Db::chats_of_member`. The implemented fix
adds a narrow head/floor query for two cache validation callers. There are two
independent size axes: retained rows per room with room count fixed, and registry
room count with retained rows per room fixed. This is the correct distinction:
history growth and the number of room-query invocations are different costs.

The normal relay uses one mutex-protected SQLite connection (`db.rs:26,948`),
WAL, NORMAL synchronous mode, foreign keys and secure deletion (`db.rs:631`).
The local SQL microbenchmark can establish statement/chain work and query plans;
it cannot establish mutex contention, async scheduling, network latency,
checkpoint import, Loro decoding, device rendering or end-to-end chat opening.
Synthetic 65-byte row payloads are deliberately not Loro updates. A SQL fixture
must never be presented as a valid CRDT integration fixture.

## Concrete fixture defects caught before the main run

Corrections were sent to the fixture owner. A second read of `fixture.py` and
`run.py` confirms all five were applied before the main profile:

1. **DM roster:** the initial DM was owner plus agent. Actual `dm_create`
   (`people.rs:1053–1140`) creates two human `user:` memberships, both owners.
   Agents belong in groups; the proposed group owner + six peers + one agent is
   a plausible declared workload, not a measured population distribution.
2. **Committed floor versus retained rows:** the initial fixture set a quarter-
   history checkpoint floor while retaining every row from sequence 1. Actual
   `put_checkpoint` deletes `rows.seq <= seq_covered` and advances the floor in
   the same lock-held operation (`db.rs:2033`). A steady-state fixture must retain
   only rows above floor, or use floor/checkpoint zero. To preserve the declared
   retained-row scale, a nonzero floor can precede the first retained sequence.
3. **Project name constraint:** initial same-owner projects all had the same
   name. `migrate_project_names_per_user` always creates the unique
   `(user_id, name)` index (`db.rs:940`). Names must be unique per owner and that
   actual index must be included in the fixture's schema subset.
4. **Independent chat namespaces:** the initial fixture used every people-chat
   id as a project-chat id too. People chats and project chats have independent
   registries and room names (`people.rs:35,45`; `db.rs:47`). Seed distinct ids,
   or omit unrelated project-chat rows. Otherwise the fixture invents links a
   real creation flow does not make.
5. **Actor representation:** normal member creation stores the bare creator
   user id as `added_by` (`people.rs:1001`), while the initial fixture stored a
   `user:` membership key. It does not affect these predicates, but fixture
   relationship assertions should follow the actual representation.

The initial `before-initial.json` is a preliminary probe of that initial
fixture, not evidence for a corrected workload. Main before/after data must
regenerate both variants from the same corrected fixture.

## Relationships and proportions that limit interpretation

- `room_seq.head` is total rows ever appended, not current retained-row count
  (`db.rs:1301`). A checkpoint deletes old rows while retaining head/floor.
  Announced messages in `chat_events`, Matrix event mappings, and Loro update
  batches are different populations; one row must not be assumed to equal one
  human message.
- `people_members` is the room ACL. Membership keys are `user:<id>` or
  `agent:<project_id>` (`db_people.rs:172`). The roster query filters departed
  rows and orders equal join times by people-before-agents and member key
  (`db_people.rs:446`). Fixtures must preserve that tie-breaking contract.
- A project can join many groups, and groups can have multiple agents. One
  project per group is only an intentionally simple workload. Project owners,
  guests, devices and group members are independent relationships; creating
  one row in each table is not evidence that production ratios are one-to-one.
- A shared owner in every registry chat is a useful worst-case unbounded list.
  Reusing the same seven users while scaling to 10,000 rooms models one heavily
  connected account; it does not model a relay-wide population growing across
  many mostly unrelated users. The chart should name that shape.
- People rooms enforce 200 pushes per person per minute, and 50 created chats
  per person per day, both halved for young accounts (`people.rs:39–44`). Large
  histories/registries can accumulate over time; they should not be described
  as one minute/day of legitimate newly created user traffic.
- A 65-byte payload isolates SQL scan work. Actual payload distributions,
  snapshot sizes, compaction cadence, warm/cold page cache, number of concurrent
  callers and storage device costs are unknown here. Varying rows alone is not
  equivalent to preserving all real data proportions.

## Database and query-family coverage inventory

Measured coverage now includes the main room-position/people registry suite and
supplementary relay unread, box t3 FTS, worker lookup and open-job probes at all
four scales. Two narrow changes have paired old/new evidence: relay position
query projection and the box worker index. Other families below remain explicit
coverage gaps, not claims of acceptable performance.

| Family / ownership | Concrete query evidence | Current coverage and required next seam |
| --- | --- | --- |
| Relay room validation + people registry/rosters | `db.rs:1843` room state; `db_people.rs:460,484` roster and member's chats | Measured. The complete N+1 chain is retained as an unchanged control, including one roster SELECT per chat. At 10,000 chats it executes 10,001 statements and takes about 90.8ms p50 locally; it is not fixed. The position-only change removes 100,002→1 logical visits for the 100,000-row history case. |
| Relay replay/write/compaction/quarantine | `db.rs:1887,1902,1938,1958,1980,2033` | Not measured. Needs separate fresh fixtures for duplicate batch lookup in rows/quarantine, cursor pages, append/dedup and checkpoint deletion. Preserve quarantine's intentional exclusion from normal replay; `db.rs:89` explains why. |
| Relay unread, author lookup and read/archive state | `db_push.rs:431,448,469` | `unread_query` measured with mixed self/other/agent senders, blocks, archives, read marks, project owners and guests. With three visible output chats fixed, logical visits grow 233→200,033 and p50 0.054→20.501ms as unrelated events grow. No privacy/ACL rewrite attempted. Author lookup and write behavior remain unmeasured. |
| Relay account/session, billing and box registry | `db_accounts.rs:237,608,895,978,1042`; `db.rs:1610,1738` | Not measured. Token consumption and ledger debit involve correctness-sensitive writes; test SQL and transactional invariants before indexes/query rewrites. Account-size fixtures must distinguish ledger history from active devices and projects. No financial path changed. |
| Relay durable push and Matrix queues | `push/store.rs:144,180,229`; `matrix/db.rs:153,195,261` | Not measured. Lease expiration, future retries, poison jobs and per-chat head-of-line ordering affect selection; a queue filled only with due jobs is insufficient. Matrix due query intentionally prevents overtaking an older pending job in the same chat. |
| Relay Matrix mirror and server control plane | `matrix/db.rs:352,397,470`; `matrix/servers_db.rs:235,263,583` | Not measured. Mirror event counts differ from raw update rows; server memberships/channels have separate fan-out and soft deletion. Device SDK and homeserver work are outside these SQL tables. |
| Relay calls/voice and fleet retention | `db_calls.rs:161,243,391`; `db_voice.rs:232,278,305`; `db_fleet.rs:192,226` | Not measured. Active versus ended calls, per-user ledger windows and retained fleet samples are different size axes. No inference from room-row timings. |
| Relay automation/search/audit | `db.rs:3062,3184,3282,3350,3540`; `db_inspect.rs:23,47` | Not measured. Includes due routines, goal histories, bounded inspect queries and `skills_fts` full-text ranking. An index's existence alone does not prove bounded rows visited or latency. |
| Relay connectors/bridges/agent links | `db.rs:2596,2757,2843`; `outside.rs:66`; `outside_log.rs:111`; `agent_link.rs:57`; `db_agent_messages.rs:56` | Not measured. External inbox/outbox cursors, per-user link events and pair-rate windows have their own composite keys and retention. No external requests are needed for future pure-SQL fixtures. |
| Relay sites and secrets/vault/pins | `db.rs:225,3634,3765`; `db_secrets.rs:16–101`; `pins.rs:30`; `shell_grants.rs:24` | Not measured. Manifest/version history and ACL/audit rows must be generated synthetically. Do not read actual secret stores to obtain a distribution. Pure SQL remains possible; security semantics require dedicated regression checks. |
| Box agentd SQLite + FTS5 | `arbos-agentd/src/store.rs:20–124,1149,1200,1242,1573,1593` | t3 FTS global/thread-filtered search, latest-worker lookup and open-jobs SELECT measured on separate per-box fixtures. Thread-filtered common-term search returns six rows but visits 200→200,000; open-job output grows 10→10,000. Only the worker lookup is fixed, with one source-derived additive index and full-row/tie/default equivalence. Memory-tier search, sessions/approval writes, room snapshot persistence and other job transitions remain unmeasured. |
| Device iOS and Mac SQLite caches | `DeviceStore.swift:115,198,252,285`; `arbos-mac/src/sync/store.rs:116,229,281,326` | SQL shapes reviewed; not profiled. Three tables: registry/avatar blobs, whole-room snapshots/cursors, ordered pending batches. Full restore copies all snapshots before decode; its runtime cost needs valid Loro data and native tests. SQLite-only generation could examine sorting/eviction but would not verify client lifecycle or rendering. |
| Matrix SDK and Synapse | `arbos-matrix/Cargo.toml:11`; `arbos-matrix/src/client.rs:92`; `docs/self-host.md:200–207` | The device SDK deliberately uses no SQLite store; its timelines are in memory. Synapse is a separate unmodified deployment with PostgreSQL 16, not this relay's SQLite file. No Synapse/Postgres load test or device Matrix timeline test ran. |

Other files named Store are not necessarily databases: iOS `SearchStore.swift`
is an AppModel extension over the in-memory search index, and `ServerStore.swift`
integrates relay metadata with SDK callbacks. Keychain/UserDefaults preferences
do not have SQLite query-plan coverage. User-created project applications may
choose their own databases; those generated app schemas are not Arbos-owned
storage covered by this repository fixture.

## Native constraints and concrete follow-ups

Both device stores cap each retained snapshot at 8,000,000 bytes and document
payload totals at 250,000,000 bytes (`DeviceStore.swift:18–20`;
`sync/store.rs:21–23`). A native fixture scaled blindly by 1000 would cross
eviction boundaries and no longer represent the same retained population.

There is a **cap semantics mismatch** worth retaining as a follow-up: usage
reports count documents plus `kv`, while cap enforcement sums documents only
(`DeviceStore.swift:268,273,285`; `sync/store.rs:302,310,326`). Pending batches,
SQLite overhead and WAL size are outside both reported payload totals. The
comment says "everything on disk", which is not a hard file-size bound. This
does not justify evicting pending writes or changing limits without native
durability/restore regressions. It does require naming the fixture budget as
document payload bytes rather than an exact total disk bound.

The active SQL suite uses a fresh local database and can run independently of
native builds. Full app/FFI/GPUI validation remains blocked by scratch capacity
and unapproved fork CI. Live Synapse/Postgres is deliberately untouched; there
is no authorized isolated deployment or representative workload available for
that integration. Most omitted relay/box pure-SQL families above are **coverage
gaps**, not inherently blocked by CI or a need for production access. The final
report must keep that distinction and cannot claim a complete all-schema load
test from these narrow measurements alone.

## Final suite/statistics/HTML review

The extractor reads exact SELECT strings and expands the production roster
column constant; changed supported query shapes fail explicitly. `run.py`
compares missing-room defaults and both DM/group head/floor results using a
separate read-only Python connection; it also asserts rows above floor, no
agent in a DM and SQLite integrity. The registry control executes the initial
chat lookup and one actual roster SELECT per returned chat, including the N+1
chain rather than just its first statement. It is an unchanged control, not a
claimed registry optimization.

`analysis_stats.py` computes linearly interpolated sample p50/p95/p99 and paired
before-minus-after savings, 2,000 seeded bootstrap resamples of median savings,
and an exact two-sided sign test excluding tied pairs. The direction is
consistent: positive savings mean the after query is faster. The pair order is
randomized with a recorded deterministic seed. These are warm, fixed-fixture
samples; the confidence interval describes this local run, not a production
population. A 101-observation p99 is a descriptive tail estimate from very few
extreme samples, not a release-service p99 guarantee.

SQLite `SQLITE_SCANSTAT_NVISIT` is summed across executed statement loops and
labeled **logical row visits**, distinct from rows returned, VM steps and
physical disk reads. The engine compiles the cached root-lock amalgamation with
scanstatus enabled, so absolute latency includes instrumented SQLite overhead.
C result-column consumption is an approximation of materialization; it is not
Rust string allocation/serde mapping or Loro parsing.

Both pre-run harness findings are fixed and verified in final source: the
registry id array grows geometrically, and the SQLite library is content-keyed
by source/header/driver/compile policy with a runtime SQLite version check.

The main chart `latency.png` was visually inspected: units, axis labels, legends
and percentile shading are legible with no clipping. `latency.html` embeds its
SVG and full percentile/CI table without external resources. The unchanged
registry control is explicitly labeled as identical SQL in both arms. Root
also rendered/inspected the separate worker-index `job-index.html`/`.png` from
the final paired JSON. The fuller supplementary methodology/results are in
`.improve/load-test/COVERAGE.md`.

The worker index changes exactly one production DDL line:
`jobs(subagent_id, created_at DESC)`. The query/projection/limit are unchanged.
Actual original DDL and winner semantics were captured before editing, and
regressions pass against both SQLite 3.53.4 and bundled 3.46.0 for full-row
equality, equal timestamps, NULL/missing workers and repeated old-store schema
upgrade without data changes. At 100,000 jobs, 101 randomized old/new pairs give
p50 4.260→0.039ms, p95 4.519→0.051ms, logical visits 100,000→1 and saved-median
95% CI 4.192–4.268ms. All four scales have positive intervals; maximum sign-test
p is 4.07×10⁻²⁷. That existing-store schema replay adds 970,752 file bytes and
takes 36.66ms locally at the largest fixture, a single observation including
index creation. Future job inserts/worker reassignment maintain the index; no
write-throughput or native startup guarantee is claimed.

Final root gates pass: eight main unittest cases (0.351s wall), six supplementary
check groups (0.114s), six index groups (0.108s), formatting and diff checks.
Original pre-edit winner assertions passed but were not separately timed;
these are new SQL-boundary tests, not a claimed improvement in native test
duration. `.improve/load-test/FINAL_GATES.json` retains the final cache-aware
gate evidence. Full native/runtime integration remains blocked as above; the
measured unchanged registry/unread/FTS/open-job leads and unmeasured families
remain follow-up work, not silently passed exit checks.
