# Phase 6: hot-path performance

**Status: pure-path optimizations and bounded Mac registry hydration verified;
remaining native hot paths and final composition gates are in progress.**
The entire source inventory was scanned by category, with focused flow reads.
This is not an every-line proof or a claim that reported app latency is resolved.
Before/after measurements below are local synthetic microbenchmarks, not
production, network, device-frame or end-to-end chat timings.

## Scan and dispositions

| Category | Evidence / action |
| --- | --- |
| Independent sequential async work | Mac registry now hydrates four projects concurrently, with independent people hydration and ordered results; unread and project-list calls run together. Actual-code fixtures preserve cancellation, deterministic merges and partial failures. Both clients' attachment uploads and iOS directory/registry work are under implementation review. Matrix per-room awaits may read SDK cache; network dependency must be established before parallelizing roster misses. |
| Loading extra DB data | `rooms::build_room_doc` / `advance_room_doc` ask for full `Db::room_state` statistics to get only head/floor. This is the next phase's source-SQL scale target. Keep full statistics at compaction/budget/protocol callers that consume them. Agent-link SELECT-star reads cross authorization DTOs; Store history reads are bounded and consume full events, so do not automatically narrow every star. |
| Unnecessary URL parsing | The edge worker's one `new URL(request.url)` supplies routing data and is required. Native typed URL creation handles actual requests/links/validation; removing it would discard behavior. `SenderAccents` guard order is a possible redundant cached-hit parse, unmeasured and blocked on native gates. |
| Repeated static regex construction | Shared `core/redact.rs` reconstructs token, bearer/env and placeholder regexes on every call; cache compiled patterns once, retaining the single key-rule table, order and skip semantics. Harness detached-shell literals allocate per call; measure before deciding whether hoisting earns a change. Agentd HTML/search parsing and truncated memory-card salvage now cache 17 fixed patterns through std LazyLock. All 64 actual-function fixture outputs and literal pattern inventories match; actual agentd all-target Clippy passes 66.538 s; final combined gates remain separate. |
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

Actual native SDK builds, safe full Linux/Swift baseline CI and local adapter
gates are now available. Earlier scratch-space, SDK and fork-approval limits
are superseded. Final changed-source composition gates remain in progress.
Narrow source tests and scripted gateway timings do not establish live-app
latency or physical-device frame performance.

## Remaining verification

Release-device cold/warm group-open and Mac-send profiles need a local owner
session and generated/non-private fixtures. Recommended default: retain all
ordering, authorization and durability checks, and distinguish first visible row,
frame time, upload/queue wait and durable ACK. No production/staging access is
authorized. Broader telemetry is deferred to the owner/Jacob decision in Phase5;
it does not block the local changes or final gates.

## Before / after measurements

| Workload | Before | After | Interpretation |
| --- | ---: | ---: | --- |
| Mac registry, 12 projects + people, scripted 8 ms/request | 330.809 ms | 90.993 ms | 3.64× controlled critical path; at most five requests |
| Shared pattern redaction | 50.237 µs/call | 11.970 µs/call | 4.20×; cached compiled patterns |
| Token classification | 30.369 µs/call | 1.065 µs/call | 28.52×; cached whole-token anchoring |
| Placeholder redaction | 5.758 µs/call | 1.927 µs/call | 2.99× |
| Nested JSON scrub (includes fixture clone) | 111.792 µs/call | 26.989 µs/call | 4.14× |
| Mac URL hints, 1,000 HTTPS links / 37,890 bytes | 2,004.850 µs/call | 134.608 µs/call | 14.89×; long-message stress fixture |
| Mac URL hints, 1,000 HTTP links / 36,890 bytes | 3,108.083 µs/call | 132.083 µs/call | 23.53× |
| Mac URL hints, 1,000 mixed links / 37,390 bytes | 177.275 µs/call | 141.250 µs/call | 1.26× |
| Agentd short HTML conversion | 33.680 µs/call | 7.327 µs/call | 4.60×; unchanged parsing policy |
| Agentd32-link HTML conversion | 870.729 µs/call | 772.695 µs/call | 1.13× |
| Truncated memory-card salvage | 5.674 µs/call | 2.223 µs/call | 2.55× |
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

Registry results use 21 randomized paired runs of the selected actual production
functions and typed mappers. The 27-request multiset and complete typed outputs
match; the paired median saving is 238.353 ms (95% bootstrap interval
230.700–241.418 ms), two-sided sign-test p=9.54e-7. Two success/failure scenarios,
cancellation and a delayed concurrency/order check pass in both arms. The peak
changes 1→5 per refresh; this is a bounded load tradeoff, not fewer requests.
[The retained check](perf/registry/README.md) uses OS network isolation, a fresh
per-arm Cargo target and locked/offline builds. It does not execute authentication,
GUI, Keychain or real HTTP behavior. Per-chat send order is unaffected.

The [agent parser check](perf/static-regex/README.md) extracts actual pure
production functions and search regex bindings, with 64 fixed/generated outputs.
It uses 21 randomized paired runs, 50 warmups and 100/1000/2000 calls per sample.
Each workload improves in every pair (p=9.54e-7); retained JSON includes the
paired bootstrap intervals, source hashes and raw samples. The host is shared,
with no CPU isolation or cache flushing. An uncommitted mechanical edit changed
a closing-list-item pattern; review caught it, the added behavioral fixture
failed on that candidate, and the literal inventory now verifies exact patterns.
Earlier candidate timings are withdrawn. Final 64 outputs match upstream. No
search endpoint, live agent or app latency was measured.

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
| Actual agentd all-target Clippy | upstream full gate passed earlier | changed-source snapshot passes 66.538 s, unique cloned target/all 12 source packages cleaned; OS network denied |
| Existing pure native agentd parser tests | 3 retained tests | 3 pass/0fail/ignored, exact one-test guards; first build174.261s, cached0.789/0.259s |
| Agent parser actual-code adapter | 64 HTML/card/search outputs pass against upstream | same 64 outputs; pattern inventory matches; fresh locked/offline release builds 3.668/3.756s |
| Registry actual-code adapter | 2scenarios + cancellation + delayed bound/order pass | same typed outputs; bound5 verified, locked/offline builds32.136/31.402s |
| Swift test-plan fence | old plan allowed default endpoint | 12contracts pass1.515s; exact shared fence carried before future push |
| Mac all-target Clippy, actual SDK composed tree | native ownership snapshot previously passed | metadata composition passes 21.500 s, all 12 workspace packages cleaned in its private cloned target; final successor/runtime/full CI remain separate |

Compiler/test wall durations include cache/scheduling differences and are not
speedup measurements. No tests were removed. The new source regression checks
concurrent readers, Unicode byte ranges and nested JSON keys/scalars. The
portable retained redaction crate directly uses the repository file as its lib;
URL compilation uses the exact source bytes and records their hash.

Reproduce with the commands in [redaction harness](perf/redact/README.md) and
[URL driver](perf/urlwrap/README.md). Both are small local-only checks that avoid
unrelated native dependencies. Native ownership changes and the measured store-scale work are tracked in
Phases3/4/7. Their integration remains a separate validation step.


## Client hydration and attachment batches

The iOS registry now hydrates at most four independent details and folds them in
input order, preserving actual DTO/default/archive/omission/error policy. Thirteen
scripted GETs remain thirteen; p50 falls129.691→33.651ms (3.85x), with mean-saving
95% interval94.642–98.029ms over21 randomized pairs, p9.54e-7. Median whole-fixture
RSS increases176KiB. Already-admitted GETs may complete after a fatal result;
ordered folding retains the first input-position error. See
[registry scope and rerun](perf/ios-registry/README.md).

Project/people attachments share one private two-request batch helper, preserving
input-minted keys, ordered outcomes/errors and one final commit. People omit failed
attachments; projects retain nil-blob rows. Actual intercepted Foundation PUT
profiles retain exact bytes/MIME/captured synthetic bearer, four PUTs, ordered
rows and one commit. Across21 random pairs per workload, p50 is47.410→24.994ms
(4x64B people),47.849→25.722ms(4x1MiB project),52.828→28.079ms(4x8MiB project).
All paired intervals are positive, p9.54e-7 each. Logical in-flight bodies1→2;
whole-fixture RSS includes deliberate oracle copies and is not a device memory
ceiling. Concurrent batches have separate limits. Errors publish after the batch;
individual cancellation errors remain item failures, while task cancellation
stops further admission and drains without a result. See
[upload contracts,63pairs and memory limits](perf/ios-uploads/README.md).

The independent public check passes11 before/11 after contracts,4 new helper
contracts and2 actual Foundation transport scenarios per version. The explicit
Phase4 composition passes17 ownership/performance contracts per version; no
missing ownership prerequisites are silently skipped. Its sixteen real raw API
owner regressions remain separate. The performance branch uses captured baseline
APIs; composition must preserve work.api, every owner guard and the bound raw
request helper. A full93-source SDK compile of the guarded combined variant
passes27.399s; this is compile-only, not a simulator/device or app-speed verdict.

## Forwarded payload reuse

Mac forwarded copies retain a reusable Bytes-backed request body and avoid one
payload-sized allocation; owned sticker/voice data uses Arc::unwrap_or_clone.
The selected actual upload bodies preserve hashes/length/MIME/order and307 replay.
Eighteen selected controls pass. Actual Mac all-target Clippy and complete app
linking also pass on the final structural/ownership/performance composition.

Twenty-one paired local12ms GET/PUT samples show a small16MiB latency improvement
85.584→83.660ms, paired-median-saving95% interval1.281–3.533ms. Smaller payload intervals
include zero and do not establish latency improvement. Separate allocation pairs
show requested bytes53,017,257→36,240,032 at16MiB (one payload allocation2→1).
Peak heap is unchanged; no lower RSS or end-to-end send claim follows. Streaming
was withheld after a real307 replay regression; nonexistent multi-upload producers
were not fabricated to justify batching. See[retained upload proof](perf/uploads/README.md).
