# Arbos improvement report

Base: `2865cb29b806121d4a90f55ba6ce151a150279a0` (upstream main).
Worktree: `/Users/cister/arbos life/arbos-codex`.

This pass inspected the whole repository by subsystem and delivered independently
reviewable changes. The broader native refactor and High state-machine conversions remain
unresolved while targeted regressions and safe native gates are running.
All-service OpenTelemetry still needs an operator destination contract;
device latency validation remains unperformed. The
Phase 3 exit check does **not** pass: reviewers still identified worthwhile
native durability, authorization and lifecycle work. Scoped green tests do not
make those requirements complete.

No pull request was merged or deployed. Local verification used generated state,
fakes and documented network guards. Earlier fork pushes automatically started
Swift tests; static review found a reachable hosted-relay health probe. Actual
prior production contact is **unverified**; the two affected runs were cancelled.
This corrects any earlier blanket no-production-access claim. Test isolation is
being verified before native reruns. Other contributor worktrees and open branches
were preserved. Every branch starts independently at the base
above, targets upstream main, and is pushed only to the contributor fork.

## Pull requests in review / merge order

All PRs are drafts. This is a suggested review order; merging requires the owner
and upstream maintainers. Detailed scope, measurements and rerun commands are in
each PR's committed report.

| Phase | Branch | PR | Disposition |
| --- | --- | --- | --- |
| 0 Baseline | `improve/baseline` | [#2947](https://github.com/unarbos/arbos/pull/2947) | Recorded; native/environment gates explained |
| 1 Map | `improve/map` | [#2948](https://github.com/unarbos/arbos/pull/2948) | Complete inventory and system map |
| 2 CI audit | `improve/ci-audit` | [#2949](https://github.com/unarbos/arbos/pull/2949) | Complete metadata audit; no CI change here |
| 3 Untangle | `improve/structure` | [#2950](https://github.com/unarbos/arbos/pull/2950) | Verified pure extractions; global exit blocked |
| 4 State machines | `improve/state-machines` | [#2951](https://github.com/unarbos/arbos/pull/2951) | Approval plus three voice High controllers fixed; other High adapters unresolved |
| 5 Observability | `improve/observability` | [#2952](https://github.com/unarbos/arbos/pull/2952) | Shell audit correctness/privacy fixed; global tracing blocked |
| 6 Performance | `improve/performance` | [#2953](https://github.com/unarbos/arbos/pull/2953) | Verified redaction/URL/static-regex work; other native paths blocked |
| 7 Database scale | `improve/database-scale` | [#2954](https://github.com/unarbos/arbos/pull/2954) | SQL scale suite; native integration blocked |
| 8 Laws | `improve/laws` | [#2955](https://github.com/unarbos/arbos/pull/2955) | Verified pure laws and redaction fix; proofs/broader native laws blocked |
| 9 Finish | `improve/finish` | this PR | Free CI changes and repeated local gates; blocked requirements remain explicit |

Documentation-only Phase 1 follows the user's explicit one-PR-per-phase rule.
Phase 4 intentionally changes malformed/configured approval failures to fail
closed and fixes tested voice lifecycle/consent races, with failing-before regressions. Phase 8's generated checks are intended
to remain valid with the behavior-preserving Phase 6 optimizations. Independent
heads require redaction overlap resolution between Phases 6 and 8. The retained
[integration patch](integration/redaction-after-performance.patch) applies the
security fix to the cached implementation. See combined verification below; no
full combined native workspace build is claimed.

## Before / after by phase

Durations below are wall time for individual checks unless marked benchmark.
Different cache/scheduling conditions make gate durations unsuitable as speedup
claims. P/F/S means passed/failed/skipped; Rust ignored tests are explicitly noted.
Unmeasured native counts remain unmeasured, not zero. Full per-phase tables and
commands are retained in each PR.

| Phase | Formatting / lint before → after | Typecheck / build before → after | Tests before → after | Phase-specific evidence |
| --- | --- | --- | --- | --- |
| 0 | Cargo fmt pass 3.016s; avatar/glass/control lint pass .084/2.590/.065s; unchanged | Harness pass 4.548/3.718s; 12-crate metadata/offline resolution pass .013/1.107s; unchanged | Harness 14/0/0, 2.797s; busy-reply 2/0/0, 46.103s; unchanged | 773 source files, 12 Rust crates, ~513k lines. Native gates blocked; baseline red rows below |
| 1 | No code changes; Phase 0 results apply | No code changes; Phase 0 results apply | No test changes; Phase 0 results apply | System map plus backend/client/auxiliary inventories; day-one cross-room ACK test designed, native fixture blocked |
| 2 | No code changes; Phase 0 results apply | No code changes; Phase 0 results apply | No test changes; Phase 0 results apply | 150 runs / 526 jobs: 114 success, 12 failure, 7 cancelled, 17 action_required. Rust success median724.5s, median queue2s; task/setup split retained |
| 3 | Workspace fmt pass 3.016→2.607s | Harness typecheck3.498→2.889s, build3.916→2.456s; portable title compile .664→.114s | Harness14/0/0, .711→.530s; actual title8/0/0, .249→.210s | One pure title policy shared by3Mac callers; pure stop-job presentation independent of volatile runtime. No tests deleted |
| 4 | Strict TS/diff checks pass; physical calibration parity reviewed | Approval typecheck4.282s/build4.337s; expanded strictTS2.558s | Approval24controls+3RED→37pass1.424s; voice actual-controller RED stages→84/0/0,6.203s; expanded harness44/0/0,7.300s; packaging9pass | Four High controllers explicit;1536voice generated cases plus256approval;4voice function mutants detected/replayed. Approval validation adds firstcheck latency8.94→60.60ms; no voice/device speed claim |
| 5 | Both shell syntax gates pass .003s each; no compiled service changes | Native checks blocked; shell has no typecheck/build | 9controls pass14.616s; 6new regressions fail before; after15/0/0,22.479s | Valid jq JSON, portable numeric timestamp, no raw input preview. Admission events only; globalOTLP/function lint/propagation/wide terminal events not implemented |
| 6 | Leaf fmt .260→.260s, clippy2.731→.510s; workspacefmt2.567s after | Leafcheck1.883→.189s, build.161→.168s; harness2.816/2.260→2.630/2.252s | Actual redaction9→10 pass,0fail/ignored,1.843→.687s; URL10goldens+125roundtrips passboth; harness14/0/0,.541→2.243s | Warmredact50.237→11.970µs (4.20×), classifier30.369→1.065µs (28.52×). Mac1000HTTPSlinks2004.850→134.608µs (14.89×). StaticTSregex~10ns saved/call |
| 7 | Workspace fmt/diff pass; duration not separately recorded | Full native blocked; bundled SQLite measurement engine builds | New suite 8/0/0: initial8.138s, final6.028s, cached .351s; two sets of6SQLcheck groups pass .114/.108s | 1/10/100/1000× through1M rows; group17→4µs at1×,5983→31µs at1000×, visits100002→1. Worker100k jobs4260→39µs, visits100000→1;101pairs/query. Native2regressions written, not run |
| 8 | Workspace fmt2.448s after; leaf fmt .055→.051s/clippy1.721→.439s | Leaf check1.915→.808s/releasebuild3.363→.861s; new strictTS pass .690s | Original9native pass1.142s; boundaryRED10pass/2fail→12pass0fail/ignored,.529s. TS18/0/0,2.150s; final8lawtests1.692s; nativeprotocol24pass | 7laws ×256=1792generated cases; boundary failure shrunk/replayed before fix;4actual-function mutations detected/replayed with256-case green controls. Proofs and broader native laws unverified |
| 9 | Workspace fmt3.016→2.606s; layout lints pass; all8workflow actionlint0diagnostics | Harness4.548/3.718→3.125/2.379s; metadata .013→.014s/resolution1.107→1.122s | Harness14/0/0,2.797→.509s; busy-reply2/0/0,46.103→29.112s; unchanged platform red rows explained below | Three free CI changes, no tests dropped/paid runners; historical estimates6s duplicate compile and14s push-workflow tail. No new Actions timing claim |

## What changed

- Colocated the duplicated Mac title policy and separated pure stop-job output
  from extension runtime initialization, preserving the public export.
- Rebuilt configured approval handling with explicit states and strict wire
  schemas. Hook errors, HTTP failures and missing decisions now block risky
  calls; unconfigured standalone mode stays explicit. Audit records omit raw
  tool input. The pinned Effect runtime is lazy-loaded on first configured check,
  with shared container packaging and tested missing-import failure behavior.
- Made voice call lifetime, mode/mute application and vision consent explicit.
  Failed effects cannot claim success, shutdown owns/retries only failed local
  cleanup, and stale callbacks/frames cannot restart ended/revoked work.
- Fixed shell audit JSON quoting, timestamp portability, output-directory
  creation and input privacy while retaining denial patterns and exec behavior.
- Cached 39 fixed redaction regexes from the existing single rule table, replaced
  repeated Mac URL suffix scans with one forward candidate scan, and hoisted
  four unchanged TS literals. Patterns, replacements and wrap hints stay intact.
- Replaced full room-statistics reads with indexed head/floor reads at two
  room-cache callers, retaining full statistics where consumed. Added the missing
  latest-job-by-worker index with complete-row/tie/NULL/schema-replay checks. Database scale
  results and remaining bottlenecks are retained as runnable local artifacts.
- Added a generated-law suite over actual production modules. Its counterexample
  exposed supported tokens ending in hyphens being classified but left visible;
  the tested fix retains surrounding punctuation and the source shape table.
  Four deliberate actual-function regressions are detected and replayed.
- Moved formatting ahead of expensive Rust setup, removed an identical no-output
  typecheck/build invocation, and let push-only image lint run independently.
  Job names and all tests remain intact; the improvement-branch push filter and
  test isolation changes are documented separately. See [CI details](ci-finish.md).

Reported gains are synthetic source-function or SQL measurements at their
respective independent phase heads; Phase 8 intentionally broadens redaction
behavior and those Phase 6 microbenchmarks are not a new measurement of the
combined security fix. No end-to-end
Mac send, group-open, Matrix hydration or iOS rendering speedup is established.

## Final repeat of baseline gates

These run on the independent finish branch. Successful checks use existing local
caches. The shell selftests use local mocks/scratch git; busy reply uses a fake
loopback relay. Neither invokes a real merge or deployment.

| Gate / count | Phase 0 | Phase 9 |
| --- | --- | --- |
| Workspace format | pass,3.016s | pass,2.606s |
| 12-crate metadata / full offline resolution | pass,.013/1.107s | pass,.014/1.122s |
| Harness typecheck / build | pass,4.548/3.718s | pass,3.125/2.379s; duplicate alias run locally to compare baseline |
| Harness tests | 14 pass/0 fail/skip,2.797s | 14 pass/0 fail/skip,.509s |
| Avatar / glass / control-size lints | pass,.084/2.590/.065s | pass,.120/2.957/.068s;76existing unjudged glass sites |
| Python AST | 78 pass/0 fail/skip,.126s | 78 pass/0 fail/skip,.145s |
| Standalone JS syntax | 3 pass/0 fail/skip,.067s | 3 pass/0 fail/skip,.070s |
| Swift parsing, not typecheck/build | 192 pass/0 fail/skip,3.092s | 192 pass/0 fail/skip,.369s |
| Voice tests | 0run/2collection errors,6.386s | later10original tests pass16.988s; Phase4expanded84/0/0,6.203s under network/credential guards |
| Merge selftest | 57success markers thenBashparsefailure,34.659s | same57markers/exit2,.306s |
| Rebase selftest | 10success/4failure markers,68.290s | same10/4/exit1,2.912s |
| Verify-cache selftest | 76success/26failure markers,16.389s | same76/26/exit1,20.793s |
| Deploy-worker **selftest only** | pass,1.146s | terminalselftest-ok/exit0,1.139s; internal assertion count not exposed |
| Busy-reply fake-relay unittest | 2 pass/0 fail/skip,46.103s | 2 pass/0 fail/skip,29.112s |
| Actionlint | tool missing at baseline | verified1.7.12:all8workflows pass0diagnostics,.009070s; external shellcheck/pyflakes unavailable |
| Full Rust clippy/check/test/build and iOS build/tests | blocked/unmeasured | Fullworkspace remains unmeasured; targeted core/client174tests pass. Local space later18–19GiB, below workspace25GiB floor. EarlierFFI build steps pass; cancelled simulator runs are not complete gates |

A later targeted native baseline passes174tests (31client,140core,3fixtures),
0failed/ignored, with network denied. Cached test wall2.292s; check19.898s,
clippy21.493s, build.714s. First test compilation took42.00s by Cargo.
[Exact commands and suite counts](verification/core-client-gates.json) distinguish
this real package build from the earlier isolated source leaf and from the
still-unmeasured fullworkspace.

## Combined scoped verification

No branches were merged. An isolated 127-file relevant subset (3.924 MB, including
code and documentation/evidence) was materialized from baseline and Phases3–8
patches. Phases3–7 apply cleanly; the only Phase8 conflict is redaction, resolved
with the reviewed canonical cached/security-fixed version. Source hashes,
commits, application records and all commands are retained in
[verification summaries](verification/combined-summary.json).

All19 scoped functional gate commands pass, with zero failing commands:

| Combined check | Result / duration |
| --- | --- |
| Harness typecheck / build | pass,2.648/2.221s |
| Harness tests | 37 pass/0 fail/skip,.872s |
| Shell audit / runtime packaging | 15 and9 pass/0 fail/skip,2.152/2.974s |
| Actual Mac title module | 8 pass/0 fail/ignored,.190s; compile .574s |
| Actual core leaf fmt / check / clippy / release build | pass,.212/1.561/1.778/3.336s |
| Actual core tests / native JSONL protocol | 13 and24 pass/0 fail/ignored,1.637/.231s |
| Laws strict TS / tests | pass,.659s;18 pass/0 fail/skip,1.571s |
| Generated laws | 7×256=1792 trials,0discards; same seeds repeated for the record, not extra coverage |
| SQL suite / supplementary / index | 8 tests +6 +6 check groups pass,8.927/.184/.109s |

A fresh frozen/offline/ignore-scripts dependency install **failed** because the
pnpm store lacks the `ts-algebra2.0.0` tarball. Recovery privately APFS-cloned
already-installed worktree-owned dependencies and verified exact versions and
private links; it is not a clean locked-install success. Final logical artifacts
are228.3MB; peak logical265.3MB. APFS shared extents mean summed reported blocks
are not exclusive physical disk use; no peak-physical-space claim is made.

The [reviewed integration patch and instructions](integration/README.md) preserve
Phase6's caches and Phase8's security fix. A new equivalent-policy warm benchmark
compares **fixed uncached versus fixed cached**, independently of the historical
Phase6 numbers. All130 complete fixture output dumps match byte-for-byte.

| Fixed policy, warm call | Uncached | Cached | Ratio |
| --- | ---: | ---: | ---: |
| Pattern redaction | 63.311µs | 12.306µs | 5.14× |
| Token classification | 31.317µs | 1.085µs | 28.85× |
| Placeholder redaction | 5.829µs | 1.896µs | 3.07× |
| JSON scrub, includes fixture clone | 138.111µs | 27.884µs | 4.95× |

There are21 randomized paired samples ×2000calls, original109cases plus21minimum-
tail cases, with an exclusive team timing window. All paired-median saving
intervals are positive; each two-sided sign-test p=9.54e-7 (uncorrected; four-test
Bonferroni bound3.82e-6). Raw pairs, intervals, exact hashes, driver and reproduction
instructions are retained in [the integration benchmark](integration/bench/README.md).
Warm caches retain39compiled regexes and replacement strings; first-use costs are
excluded. These local samples are not production confidence bounds or app latency.

Full workspace compilation, GPUI geometry, native room/CRDT integration, container
builds and device behavior remain unverified; these19 scoped commands do not
replace those gates. [Baseline command records](verification/baseline-repeat.json)
are separate from the combined checks.

## What remains, and why

The repository-wide map/catalog covers relay, agent box, native Mac/iOS/FFI,
Matrix, harness, voice, edge and operational scripts. Focused inspections and
representative load fixtures are not an every-line proof or production workload.

Native High findings include cross-room pending-send acknowledgements, stale
sign-in/call/Matrix completions, detached cache publication after cleanup, durable
append head/row atomicity, sidecar cursor advancement after failed application,
and privacy read errors defaulting too broadly. Tests pinning these native
adapters and failure paths must run before changing them. Voice test prerequisites were subsequently installed in a private verified environment.
The approval, call lifetime, mode/mute application and vision consent controllers
now have runnable regressions and pure transition laws;84guardedSDKcases pass
(10original,53worker,21vision), with zero recorded access attempts. Six voice laws
exercise1536cases, separately from256approval terminal cases. Existing physical
calibration values are retained; new local cleanup waits are cooperative five-second
deadlines, not hard bounds against arbitrary SDK cancellation suppression. The catalog records effective states,
illegal combinations, owners and suggested regression seams.

No owned PostgreSQL jsonb store was found; Synapse owns its separate schema.
Dynamic user regexes and required URL validation remain dynamic/required.
Attachment order, checkpoint semantics, retry/parking grace and physical voice/UI
calibration remain intact. No migration was applied to a shared database.

The load tests expose the unchanged roster N+1, unread work over unrelated events,
common-term FTS work and unbounded open-job output. Query families not exercised
are explicitly inventoried. No blanket database scalability claim is made.

Broader observability is explicitly deferred by the owner while they ask Jacob
(const) which telemetry backend and conventions Arbos already uses. This includes
OTLP export, definition-level tracing and coverage lint, W3C propagation, account
context, wide terminal events and tail sampling. Keep the completed logging
improvements. This decision does not block the remaining phases; no endpoint,
account, key or production configuration is invented or inspected.

Baseline red/environment rows: full native clippy/check/test/build and iOS
artifacts unavailable; voice originally0tests/2collection errors, later resolved by the private SDK gate;
merge shell selftests rely on modern Bash/GNUstat unavailable on this macOS setup.
The original merge test reached57successmarkers thenBashparseerror; rebase10pass/
4fail and cache76pass/26fail are platform failures, not evidence of Linux CI
regressions. Existing operational scripts were not changed to conceal them.

## Needs the owner

1. **Native/device verification after safe fork gates.** Recommended default:
   use the existing public fork's free standard runners and isolated test plans,
   then require full native adapter/device checks before merging. Upstream PR runs
   still need maintainer approval; that is not a blanket fork-CI blocker. Local
   space improved from2.84GiB at baseline to18–19GiB, enough for targeted packages
   but below the fullworkspace25GiB floor. Preserve Claude's checkouts and outputs.
   Current safe rerun outcomes will be recorded here; absent checks are not green.
2. **Review the approval security fix and prioritize unresolved High findings.**
   Recommended default: retain tested fail-closed configured-hook behavior,
   input privacy and supported-token redaction; then prioritize room ACK ownership, stale identity callbacks,
   atomic append/cursor durability and membership error handling once native
   regressions run. Phases3,4and8 describe intentional security behavior changes and failing-before
   regressions.
3. **Observability after the discussion with Jacob (const).** Recommended default:
   keep the completed logging changes and finish the other phases now. The owner
   will ask Jacob what telemetry Arbos already uses; implement broader tracing,
   propagation, coverage lint, wide events and tail sampling only after that
   existing-backend contract is known. No new paid account or invented endpoint.
4. **Bend2 proofs.** Recommended default: keep runnable generated properties now;
   agree on a pinned Bend2 toolchain and proof scope later. Neither `bend` nor
   `bend2` is installed. No unexecuted proof is represented as established.

The requested [skill](/Users/cister/AGE/skills/improve-codebase/SKILL.md) requires:
“A change there needs a test that pins the current behaviour first.” Native
verification and the unresolved High findings prevent its global exit condition
from passing. Those requirements are blocked, not declared complete.

No paid-runner upgrade or reduced test suite is recommended: measured successful
Rust queueing is2s; the present bottleneck is compilation/test/setup work.
