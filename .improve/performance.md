# Phase 6: hot-path performance

**Status: verified pure-path optimizations; broader native paths remain blocked.**
The entire source inventory was scanned by category, with focused flow reads.
This is not an every-line proof or a claim that reported app latency is resolved.
Before/after measurements below are local synthetic microbenchmarks, not
production, network, device-frame or end-to-end chat timings.

## Scan and dispositions

| Category | Evidence / action |
| --- | --- |
| Independent sequential async work | Mac registry serial project detail/member hydration before people chats (`sync/relay.rs:2585`); Matrix per-room snapshots/sender resolution (`arbos-matrix/src/client.rs`); both clients' attachment uploads; iOS initialization/cache publication. Concurrency must be bounded and preserve cancellation, ordered sends, identity epochs and deterministic merge semantics. Existing native lifetime bugs and unavailable adapter gates block these changes. |
| Loading extra DB data | `rooms::build_room_doc` / `advance_room_doc` ask for full `Db::room_state` statistics to get only head/floor. This is the next phase's source-SQL scale target. Keep full statistics at compaction/budget/protocol callers that consume them. Agent-link SELECT-star reads cross authorization DTOs; Store history reads are bounded and consume full events, so do not automatically narrow every star. |
| Unnecessary URL parsing | The edge worker's one `new URL(request.url)` supplies routing data and is required. Native typed URL creation handles actual requests/links/validation; removing it would discard behavior. `SenderAccents` guard order is a possible redundant cached-hit parse, unmeasured and blocked on native gates. |
| Repeated static regex construction | Shared `core/redact.rs` reconstructs token, bearer/env and placeholder regexes on every call; cache compiled patterns once, retaining the single key-rule table, order and skip semantics. Harness detached-shell literals allocate per call; measure before deciding whether hoisting earns a change. Agentd web parsing has static regexes; tree salvage has two fixed regexes. Their native callers remain unverified and are deferred. |
| Input-specific regex construction | Harness `peek` grep and agentd MCP grep compile user patterns once per request. These vary by input and remain dynamic; a global unbounded pattern cache would add unrelated state. |
| Large JSON-key reads / jsonb | Owned stores are SQLite; no owned PostgreSQL jsonb column was found. Agentd uses `json_extract(source, '$.message_id')` over thread-scoped metadata; measure that workload before a new column/migration. Synapse's PostgreSQL schema is external to this codebase. No shared migration/backfill is applied. |
| Extra hot-loop finding | Mac `hint_bare_urls` repeatedly searches the whole remaining suffix for both schemes. With many HTTPS links and no HTTP link, the unsuccessful search repeats work quadratically. A forward candidate scan preserves earliest-match behavior and the existing wrap output. |

The [complete client/Matrix/FFI scan](reviews/performance-clients.md) records
existing optimizations and precise caller/test prerequisites. In particular,
Matrix channel hydration is distinct from the Loro people-group opening path.
The Mac URL helper is called by transcript/markdown bubble rendering; redaction
is shared by store, feedback and inspection paths. Neither measurement alone
establishes the cause of group-open or Mac-send complaints.

## Behavior and verification boundary

Shared redaction keeps all patterns/replacements/boundaries and derives both key
anchorings from the existing single table. Source tests and the portable leaf
harness compile the actual repository file, not a copied implementation. The URL
driver includes the actual std-only runtime module with cfg(test) disabled;
existing GPUI visual-wrapper tests remain untouched. Fixed output fixtures and
round-trip checks pin the pure behavior before/after; layout calibration and
invisible wrap characters remain unchanged.

Full workspace clippy/check/test/build and native UI/device measurements remain
blocked by local build space and fork-CI approval. Narrow source tests do not
establish whole-app integration. Independent review and recorded measurements
must distinguish these scopes.

## Needs the owner

Unblock native gates and local fixture/device profiling before changing shared
document lock ownership, registry concurrency, attachment FIFO/cancellation or
Matrix identity lifetimes. Recommended default: preserve current ordering and
authorization/durability checks, and prioritize a cold/warm group-open and send
profile after the cheap store-query experiment. No production/staging access is
needed or authorized.

## Before / after measurements

| Warm workload | Before | After | Interpretation |
| --- | ---: | ---: | --- |
| Shared pattern redaction | 50.237 µs/call | 11.970 µs/call | 4.20×; cached compiled patterns |
| Token classification | 30.369 µs/call | 1.065 µs/call | 28.52×; cached whole-token anchoring |
| Placeholder redaction | 5.758 µs/call | 1.927 µs/call | 2.99× |
| Nested JSON scrub (includes fixture clone) | 111.792 µs/call | 26.989 µs/call | 4.14× |
| Mac URL hints, 1,000 HTTPS links / 37,890 bytes | 2,004.850 µs/call | 134.608 µs/call | 14.89×; long-message stress fixture |
| Mac URL hints, 1,000 HTTP links / 36,890 bytes | 3,108.083 µs/call | 132.083 µs/call | 23.53× |
| Mac URL hints, 1,000 mixed links / 37,390 bytes | 177.275 µs/call | 141.250 µs/call | 1.26× |
| Detached-shell classifier, foreground corpus | 109.48 ns/call | 98.93 ns/call | ~10 ns allocation cleanup, not perceptible chat latency |
| Detached-shell classifier, mixed corpus | 182.23 ns/call | 172.05 ns/call | same unchanged patterns, ~10 ns |

Redaction uses 21 seeded randomized paired process samples, 2,000 calls per
workload/sample and 109 deterministic cases, on an Apple M5 Pro/macOS arm64 with
rustc 1.98.0 release profile. All 21 pairs improve for each workload (paired
sign-test p=9.54e-7); bootstrap intervals/ranges and raw samples are in
[redaction results](perf/redact/RESULTS.md) and its JSON. First-use compilation
is outside warm samples; 39 compiled regexes remain allocated after first use.
The unchanged output fingerprint is fixture equivalence, not a universal proof.

URL measurements use 31 alternating paired optimized-binary runs, eight warmups
and 5,000/links calls per sample; all nine workload rows, raw samples and source
hashes are in [URL results](perf/urlwrap/README.md). One-link medians also improve
from 0.219–0.233 to 0.156–0.161 µs; the much larger ratios apply to deliberately
long same-scheme messages. No GPUI visual/frame performance claim is made.

The TS microbenchmark uses ten fresh Node processes per arm/corpus, alternating
order, 50,000 warmups and 300,000 timed calls. Paired bootstrap mean-saving
intervals are 6.84–13.55 ns (foreground) and 8.63–13.59 ns (mixed). V8 already
caches regex compilation; this change removes literal-object/array allocation.
Dynamic user grep remains unchanged. Team benchmarks ran in separate windows;
filesystem/system caches were not flushed.

## Before / after gates

| Row | Before | After |
| --- | --- | --- |
| Actual redaction leaf formatting | pass, 0.260 s | pass, 0.260 s |
| Leaf clippy / typecheck | pass, 2.731 / 1.883 s | pass, 0.510 / 0.189 s |
| Actual-source Rust tests | 9 passed / 0 failed/ignored; 1.843 s | 10 passed / 0 failed/ignored; 0.687 s |
| Leaf build / optimized benchmark build | pass, 0.161 / 3.182 s | pass, 0.168 / 0.633 s |
| Redaction fixture outputs | baseline fingerprint over 109 cases | identical; key-prefix table unchanged |
| URL actual-source driver | 10 exact goldens + 125 round trips pass | all pass; wrap/GPUI tests untouched |
| Harness typecheck / build | pass, 2.816 / 2.260 s | pass, 2.630 / 2.252 s |
| Harness tests | 14 passed / 0 failed/skipped; 0.541 s | 14 passed / 0 failed/skipped; 2.243 s |
| Workspace formatting | pass, baseline 3.016 s | pass, 2.567 s |
| Full native clippy/check/test/build | blocked | blocked; leaf success does not replace them |

Compiler/test wall durations include cache/scheduling differences and are not
speedup measurements. No tests were removed. The new source regression checks
concurrent readers, Unicode byte ranges and nested JSON keys/scalars. The
portable retained redaction crate directly uses the repository file as its lib;
URL compilation uses the exact source bytes and records their hash.

Reproduce with the commands in [redaction harness](perf/redact/README.md) and
[URL driver](perf/urlwrap/README.md). Both are small local-only checks that avoid
unrelated native dependencies. Native high-risk ordering/identity/storage work
and the store-scale experiment remain separate.
