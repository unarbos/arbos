# Local SQLite scale suite

This suite creates **new temporary synthetic databases only**. It accepts no database path, DSN, SQL argument or service destination. It never opens an application database, loads environment configuration, or connects to a service. Temporary fixture files are deleted before the next scale. The largest recorded file is 177,668,096 bytes (169.4 MiB); the measurement library is 1,341,688 bytes. Data and compiled libraries are ignored, never committed.

```sh
python3 -m unittest discover -s .improve/load-test -p test_suite.py -v
python3 .improve/load-test/run.py
python3 .improve/load-test/render.py
```

Run from any directory using absolute script paths if desired. The runner resolves the repository from its own location. Default samples are 101 randomized before/after pairs per changed query and 21 pairs for the unchanged registry control. `--repetitions` and `--control-repetitions` permit larger repetitions, with minimums21/5. The complete run owns its timing window; avoid concurrent builds/benchmarks. Reruns overwrite RESULTS.json and chart outputs.

Prerequisites: Python3 with stdlib sqlite3, a C compiler, and the cached `libsqlite3-sys` version in the root Cargo.lock. Rendering additionally uses installed Matplotlib (recorded3.10.8, NumPy2.4.1). No dependencies are downloaded. The compiler builds inspected cached SQLite3.46.0 (`libsqlite3-sys0.30.1`) with FTS5 and `SQLITE_ENABLE_STMT_SCANSTATUS`. A content-addressed library key includes amalgamation, header, driver and compile-policy hashes; runtime version is verified. PythonSQLite3.53.4 seeds fixtures and independently checks values/defaults. SourceSQL is measured by the bundled engine, not Python's SQLite.

## Source contract and before evidence

`source.py` extracts the exact `SCHEMA`, `PEOPLE_SCHEMA`, required ownership migrations, project-name unique index, all three original `room_state` SELECTs, and the `chats_of_member`/`members_of` SELECTs from Rust. The frozen `before-source.json` was captured before native edits; the final after query is extracted from the actual `ROOM_POSITION_SQL` constant and must equal the original first SELECT. The runner rejects unreviewed schema/query changes. This fixture intentionally applies only the source schema and migrations needed for these relationships/queries, rather than pretending to reproduce every account/provider migration.

`before-initial.json` is the initial21-sample pre-edit smoke capture. It is **excluded from final statistics**: its provisional roster/compaction assumptions were corrected after independent review. Final paired measurements run both retained original SQL and actual after SQL over the same corrected fixtures, connection and engine. This is a query comparison; it does not execute Rust `Db` or the relay/native app.

## Fixtures and assumptions

Each independent experiment has 1000,10,000,100,000 and1,000,000 retained rows:

- History growth fixes10 rooms (5DM/5group), with100×scale retained rows per room.
- Registry growth has10×scale rooms, with100 retained rows each.

These are declared synthetic assumptions, not measured production proportions. DMs have two human owners, matching the real creator. Groups have one owner, six human peers and an agent project. One owner belongs to every room, deliberately stressing the unbounded registry. Seven users, corresponding devices, owner-linked projects, independently named project chats, people chats/memberships, project memberships and partial read marks keep the real relationships. Unique project names satisfy the source per-user name index. Human `added_by` is the bare creator id. Each room retains exactly N rows above floor=N/4, with head=floor+N and a checkpoint at floor. The65-byte row payload and checkpoint/frontier are opaque SQL fixtures, **not valid Loro data** and never used for CRDT/import claims.

Value checks cover existing DM/group positions, missing-room `(0,0)` defaults, compaction bounds, human-only DM membership and database integrity. Regression tests additionally pin relational links, unique-name enforcement, real visit/statement counters, SQL errors and readonly/multi-statement rejection.

## Recorded verification

The suite is new; there was no pre-existing suite-test baseline. Its initial completed regression gate passed8 tests/0fail/0skip in8.138s. After the final engine content-cache/version check, the same8 tests passed in6.028s. These are suite gates, not native Rust test results or performance improvements. Before/after query profiles each contain1616 changed-query executions plus168 unchanged-control executions (3568 total). All8 fixtures passed value/default/integrity checks. Native integration was unrun at that original capture. The later roster follow-up below adds actual native package gates; full workspace/platform CI status is reported separately by the parent phase.

`test_suite.py` checks known percentile interpolation, strict pairing, exact sign-test values, tie behavior, stale-source rejection, meaningful fixture relationships/defaults, row counters, N+1 statement counts, and failure paths. No production behavior is mocked as a passing native gate.

## Measurements and limits

C times preparation, binding, stepping/column consumption and finalization, matching the source SQL sequence: three statements before, one position statement after. The control executes the original chat listing followed by one original roster SELECT per chat. It uses amortized vector growth for collected chat IDs. Rust allocation/JSON parsing, lock waits and HTTP/native work are not reproduced. The readonly engine rejects non-SELECT, multi-statement and NUL-containing queries; the public runner exposes no arbitrary query or database parameters.

Actual `sqlite3_stmt_scanstatus(SQLITE_SCANSTAT_NVISIT)` logical visits and VM steps are recorded separately from timed samples, together with bundled-engine EXPLAIN QUERY PLAN output. Visits are **not physical disk reads**. Scanstatus instrumentation is still compiled into latency samples, so absolute latencies include its overhead. Statement preparation is included in both arms. Data are warm and single-reader; index-page churn from preceding history scans can still affect the small position query. Observed timer granularity is1µs, quantizing very small intervals.

Statistics use p50/p95/p99 with linear interpolation, seeded2000-resample bootstrap intervals for paired median savings, and exact two-sided sign tests excluding tied pairs. These summarize one local randomized paired run, without cross-process/device replication; they are not production confidence bounds. Significance is reported per changed query; the summary also gives Bonferroni correction across16 comparisons. Registry before/after labels execute identical SQL and are an unchanged control, with no improvement claim.

`RESULTS.json` retains source hashes, all raw samples/order, fixture counts/bytes, plans, logical visits and VM steps. `RESULTS.md` is the compact summary. `latency.html` is self-contained inline SVG plus complete p50/p95/p99 table, with no external resources; `latency.png` is the inspected QA/export image. All chart panels use milliseconds, p50 lines and p50→p95 shading.

Other databases/read paths remain separate coverage: box local store/FTS/jobs and relay unread get supplementary probes, while native DeviceStore/Mac blob persistence, Matrix SDK/Synapse, operational migration history and CRDT validity need their own targeted environments and gates. Concurrent writes and WAL/checkpoint pressure are unmeasured workloads, rather than an external blocker to local SQL testing. Production distributions are unknown. The original room-position improvement removes an unnecessary indexed **range scan** for statistics; the later roster comparison below addresses the measured N+1 separately.

## Native roster batching follow-up

```sh
cargo test --locked --offline -p arbos-relay roster_batch_tests
python3 -m unittest discover -s .improve/load-test -p 'test_*.py' -v
python3 .improve/load-test/roster.py
python3 .improve/load-test/render_roster.py
```

Use a verified network fence for native test execution. The recorded native gates used a fresh HOME and a sandbox denying every network operation; the fixtures use generated local data. `ROSTER_BEFORE_SOURCE.json` is extracted from published Phase7 commit `586f37aa5708e88b91cd7dc45a10aad86eb36aa7`. The first chat SELECT and individual-room roster SELECT are unchanged. `source.py` additionally extracts the reviewed batched roster SELECT and rejects unknown query shapes.

The three actual Rust regressions pin complete list/detail JSON parity with hidden/read/banned state, live/deleted/left/foreign membership scope, tie ordering and all membership fields. Before the fix, both behavioral controls passed while the actual SQLite trace budget failed: 10 rooms executed 11 statements. The fixed function executes one statement for an empty result and two for nonempty results. All three tests passed; broader DB and people filters, package check/build and Clippy over all targets with warnings denied also passed. The Python suite now has 12 passing tests, including a deleted-chat-guard negative control. Native verification uses separate target directories per source snapshot, with workspace packages forcibly cleaned before each rebuild. A discovered shared-target Cargo cache alias superseded the earlier shared-cache GREEN rows. Isolated original verification reproduced two passing behavioral controls plus the 11-versus-2 statement-budget failure; the independently rebuilt fixed snapshot passed all three tests, broader filters, check, Clippy and build. No SQL timing artifact changed.

`roster.py` compares the retained original N+1 against the actual two-SELECT path on the same four registry fixtures with 101 randomized pairs per scale. It checks complete ordered SQL row equality for owner, peer, agent and missing principals. `ROSTER_RESULTS.json` retains raw pairs, source hashes, counters and plans. `roster.html` is a self-contained Matplotlib SVG/table report; `roster.png` is its inspected export.

At 10,000 rooms, warm SQL p50 was 84.308→27.615 ms (3.05×), p95 89.277→29.275 ms, with 10,001→2 statements and identical 60,000 returned chat/member rows. Paired median saving 95% interval was 56.538–56.852 ms; exact sign-test p was 7.89e-31. Repeated scope selection raises logical visits 70,000→90,000 and VM steps 2,165,015→2,465,039. These are declared synthetic SQL measurements, excluding Rust mapping, mutex waits, network and rendering. The new native method holds one connection lock through both queries, whereas the old method reacquired it between rosters; concurrent writer latency is unmeasured. No complete app speedup is claimed.

The earlier `run.py` registry series remains a retained original-SQL control for the room-position experiment. It is not the current native roster path; use `roster.py` for the batching comparison.

Measurement provenance check: the final main RESULTS.json, supplementary metadata and current engine all share build_content_sha256 `297787c1d9c34e3d4bd34ea0b1f58f5b634d99734218f16da4ac01ba109961c4`, SQLite source hash and compile flags. The retained main measurement driver therefore matches current source exactly. The earlier1,341,672-byte figure came from the unnamed `scan.dylib` smoke build; the content-addressed Mach-O filename embeds a longer install name and is1,341,688 bytes. The final profiles all used the latter; no query/driver logic changed after profiling.


## Complete handler and long-history follow-through

[PEOPLE_DTO.md](PEOPLE_DTO.md) documents the real Rust handler/JSON benchmark,
with request-local profile batching and live-device indexing. Its committed
component timings pin d499090b → b96ef1f90. The separate activity suite measures
the unchanged head/maximum-timestamp SELECTs and the additive history index,
including its explicit append-write and storage costs:

```sh
python3 .improve/load-test/activity_index.py --check
python3 .improve/load-test/activity_index.py
python3 .improve/load-test/render_people_dto.py
python3 .improve/load-test/render_activity.py
```

Both scripts accept generated-data controls only, with no DSN or service URL.
Charts are standalone HTML with embedded SVG and separate PNG exports. Native
checks and combined CI remain distinct from SQL profiling.
