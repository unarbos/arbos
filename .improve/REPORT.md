# Arbos codebase improvement report

Base: `2865cb29b806121d4a90f55ba6ce151a150279a0`. Ten independent phase branches
and ten draft PRs are ready for review; nothing was merged or deployed. Completed
logging fixes remain. Broader telemetry is explicitly deferred while the owner
asks Jacob (const) what Arbos already uses; it does not block these contributions.

All seven completed Linux verification runs pass, including 2,586 Rust tests on
the combined Rust source. Final iOS verification accepts all 559 unit cases
(558 strict passes and one intentional expected-failure control) and passes five
unchanged avatar UI cases. A preceding complete UI run passes 55 cases with
13 existing opt-in skips. The two native source versions and scopes are recorded
separately below. Local benchmarks measure selected paths, rather than live-user
chat-open or send latency.

## Review order

| Phase | Branch | Pull request | Scope |
|---|---|---|---|
| 0 | `improve/baseline` | [#2947](https://github.com/unarbos/arbos/pull/2947) | Original counts, red gates and local setup |
| 1 | `improve/map` | [#2948](https://github.com/unarbos/arbos/pull/2948) | Whole-system flows, stores, boundaries and hot paths |
| 2 | `improve/ci-audit` | [#2949](https://github.com/unarbos/arbos/pull/2949) | 150-run metadata audit; report only |
| 3 | `improve/structure` | [#2950](https://github.com/unarbos/arbos/pull/2950) | Shared policies, owned room documents, durable receipts, privacy and structural cleanup |
| 4 | `improve/state-machines` | [#2951](https://github.com/unarbos/arbos/pull/2951) | Account/task/process/consent/publication ownership and actual generated laws |
| 5 | `improve/observability` | [#2952](https://github.com/unarbos/arbos/pull/2952) | Logging correctness/privacy; broader telemetry owner-deferred |
| 6 | `improve/performance` | [#2953](https://github.com/unarbos/arbos/pull/2953) | Bounded registry/uploads, reusable payloads, URL/regex hot paths |
| 7 | `improve/database-scale` | [#2954](https://github.com/unarbos/arbos/pull/2954) | Real local SQLite scale profiles, projection/batching and indexes |
| 8 | `improve/laws` | [#2955](https://github.com/unarbos/arbos/pull/2955) | Generated actual-source redaction/URL laws; native ownership laws indexed |
| 9 | `improve/finish` | [#2956](https://github.com/unarbos/arbos/pull/2956) | Free CI improvements, isolated combined verification and this report |

Phase 1 has a documentation PR under the user's one-PR-per-phase instruction.
Independent main-based heads deliberately overlap. Use the reviewed integration
instructions and source pins; a blanket ours/theirs resolution would lose fixes.
No branch was merged to create the verification tree: its single parent is the
recorded upstream base and its exact source is assembled with guarded Git indexes.

## Before and after by phase

P/F/S means passed/failed/skipped. Individual check durations include cache and
scheduling differences and are not performance comparisons. Scoped native/helper
counts overlap; they must not be added as a whole-workspace test count.

| Phase | Before | After / explanation | Phase-specific evidence |
|---|---|---|---|
| 0 Baseline | Format3.016s; harness typecheck/build4.548/3.718s; harness14P0F0S2.797s; Python78 AST; JS3 syntax; Swift192 parse | Original source unchanged in this phase. Bash3/GNU-stat/platform and voice collection reds recorded | 773 source files/~513k lines/12 Rust crates; local setup and exact original commands retained |
| 1 Map | No product edits | Complete flow/subsystem inventories; cheap day-one cross-room ACK regression implemented with Phase3 | Five architecture questions answered; no fabricated whole-app latency baseline |
| 2 CI audit | 150 runs /526 jobs:114 success,12 failure,7 cancelled,17 action_required | Report only; nine audit questions and top5 job queue/setup/task costs retained | Successful Rust median724.5s, setup125/task598.5/queue2; estimates distinguished from measured savings |
| 3 Structure | Actual room read4P1F; preview fingerprint1P1F; copied scheduler checks; original privacy/durability failures | Actual room snapshot5P; owned-doc7checks/1mutant; actual schedule7checks/3mutants; privacy11P; sidecar15daemon+4mirror; full actual Mac all-target Clippy/link pass in composition | Shared title/mention/avatar/expiry; room ACK ownership; atomic append; fail-closed unreadable privacy; pure Worker routes; dead/wrapper cleanup. Explicit iOS preview/cache policy and mandatory-row failure checks pass; final composed exit check below |
| 4 State machines | Reproduced illegal combinations at actual resource/effect boundaries, retaining positive controls before each fix | Account/process/native/voice/controller fixes and laws below;305/122/80 scoped iOS cases and5compiler controls pass; exact final runtime CI below | Genuine High boundaries implemented or already explicit; Medium/Low remain in catalog; no blanket boolean rewrite |
| 5 Logging | 9 controls pass;6 new regressions fail |15P0F0S, shell syntax pass | Valid structured JSON, portable timestamps/output creation, no raw tool input; existing denial patterns/exec behavior unchanged. OTLP/propagation/lint/wide terminal events/tail sampling deferred by owner |
| 6 Performance | Serial independent hydration/uploads, repeated regex/URL work and payload-sized copy | Exact-output/source regressions pass and paired local profiles improve selected paths; larger network/memory tradeoffs stated below | Retained runnable paired benchmarks; no app-open/send/device latency claim |
| 7 Database | Unindexed/extensive real query work at1/10/100/1000x | Query/projection/index regressions pass; local real bundled SQLite profiles/charts retained; combined relay Clippy +privacy/DTO checks pass | Room position100002→1 visits; roster1+N→2; whole list93,822→8,005 SQL statements at1000rooms; rejected broad-ACL index retained as negative result |
| 8 Laws | Supported hyphen-ending credentials could be confidently classified yet remain visible (10P2F boundary suite) |12P boundary suite,13P combined source;7×256 generated redaction/URL inputs,18 tests+24 protocol checks;4 actual mutations falsify/shrink/replay | Actual-source bridges, bounded Schema-derived observations and exact counterexamples; optional Bend2 proofs deferred because toolchain unavailable |
| 9 Finish | Format3.016s; harness14P; duplicate identical tsc alias; image lint waits for unrelated jobs | Combined Linux2586P0F3ignored, harness94P0F0S;12 Swift-fence+14 runner+5 namespace contracts pass. Swift runtime disposition below | Formatting before expensive setup, existing dependency cache retained, duplicate identical tsc alias removed, independent image lint; exact full composition and provenance |

## What changed at the state boundaries

The [state-machine catalog](https://github.com/cisterciansis/arbos/blob/improve/state-machines/.improve/state-machines.md) separates actual exclusive modes
from orthogonal facts. Durable SQL jobs, authorization rotation and typed upgrade
pipelines already have explicit authority; a duplicate unused reducer was not added.

- Account and UI work captures an opaque lifetime ticket once. Mac Shared/task,
  store-close/open, call-request and Matrix ownership reject stale completion.
  iOS async mutations, snapshots, sheet state, cards, chat callbacks, media-header
  acquisition, clipboard publication and decoded image keys retain their origin.
  Viewer/Library and private link-preview chains use that same captured receipt;
  all current fetch/copy producers pass their predicate explicitly.
  Per-draft DM queues preserve FIFO without sending B's message to A's created DM.
- Main Pi slot and RPC/Claude worker registries own starting/ready/stopping/closed
  transitions and captured process resources. RPC retains one five-second grace
  across EOF and a blocked writer; at its deadline it kills/reaps the captured
  child before final stdin cleanup. Claude's blocked-writer shutdown kills before
  waiting for stdin. A superseded completion cannot clear a replacement. Actual
  physical writer/log-survival tests preserve unknown versus readable liveness.
- Pending approval is inline, polled or deferred; the durable decision is claimed
  before dispatch. One-use secret/vault authorization atomically claims a scoped
  non-cloneable receipt. Uncertain external effects consume the receipt instead
  of silently restoring permission. Configured validation failures fail closed.
- Box creation uses a per-user permit; a cancelled creator releases only its own
  state. Restart owns freeze/notify/drain readiness. Release, feedback, runner and
  deployment work uses typed transitions and real OS/SQL leases at effect edges.
  Revision/approval/hold/signature evidence and existing manual policies remain.
- Billed voice settlement commits receipt, ordered debit/clamp and legacy setup in
  one transaction, rejecting skipped required writes. Repeated/refused reports
  cannot double debit. Existing thresholds, soft-cap order, pricing and informational
  meter policy stay pinned. Speculative room imports retain a private lease and
  become shared only after authorization/append succeeds.
- Voice lifetime, mute/mode and vision consent own effects and revocable stream
  generations. Late frames/tasks cannot restart ended or revoked work. Recording
  permission is consumable, cancellation revokes pending requests, and captures
  own UUID temporary files. Codecs, meter/gesture/camera/audio calibration and
  display filenames remain pinned.

Before/after runtime cases, real SQL faults, literal source hashes, negative
controls, seeds and mutation replays live with each retained check. Examples:
voice84 native/controller cases +1536 generated inputs; pending approval/creation
1280 generated inputs; billed settlement1024 inputs +9 protocol negatives;
release768, runner1024, feedback1280, one-use/restart1024 each; native account
lifetimes1792 and per-draft queue768. These are finite, overlapping observations,
not exhaustive proofs or independent full-suite counts. Test-only native bridges
compile actual definitions; they do not implement a second money/state algorithm.

## Measured performance and limits

Configured approval validation has a retained cold-start cost: first check
8.94→60.60ms with the lazy shared Effect runtime; warmed checks
0.919→1.354ms across50 samples per arm and post-check process RSS106.5→126.6MB.
Index import remains68.30→68.27ms across10 fresh Node processes per arm. These
are intentional validation/privacy costs in a local allowed-hook fixture, not
chat-open/send timings or a speed improvement.

| Actual local path / workload | Before p50 | After p50 | Meaning and tradeoff |
|---|---:|---:|---|
| Mac registry,12 projects +people,27 scripted8ms requests |330.809ms|90.993ms|3.64x; same request multiset/typed output; peak1→5 (four details plus people) |
| iOS registry,12 scripted8ms details/13GETs |129.691ms|33.651ms|3.85x; peak1→4; first input error/order/omissions preserved; bounded speculative GETs may finish after an error |
| iOS people attachments,4×64B, actual intercepted Foundation PUTs |47.410ms|24.994ms|47.28% lower; four exact bytes/MIME/authorizations, ordered rows, one commit |
| iOS project attachments,4×1MiB /4×8MiB |47.849/52.828ms|25.722/28.079ms|46.24/46.85% lower; at most two payloads in flight; logical body working set increases |
| Mac forwarded upload,16MiB, local12ms GET/PUT |85.584ms|83.660ms|Small positive paired saving; smaller payload latency intervals include zero. Payload-sized allocation count2→1; requested bytes53.0→36.2MB, peak heap unchanged |
| Agent short HTML /32links/card parsing |33.680/870.729/5.674us|7.327/772.695/2.223us|4.60/1.13/2.55x;64 exact outputs; unchanged17 regexes |
| Combined security-fixed redaction/classification |63.311/31.317us|12.306/1.085us|5.14/28.85x;130 complete fixture outputs match; retained canonical overlap resolution |
| Mac1000HTTPS URL hints |2004.850us|134.608us|14.89x in deliberately long messages; same visible/copy text, no frame-rate claim |
| Real whole room-list handler,1000rooms |1832.479ms|321.528ms|5.70x; complete JSON byte equality; SQL93,822→8,005; no HTTP/UI/network measurement |
| Room activity,100k retained rows |5503us|41us|Visits100001→2; preserves MAX(timestamp), including ties/backfills |

Registry/uploads/parser profiles use21 paired samples (URL31; SQL series21/31/101
as documented), randomized or alternating order, seeded bootstrap intervals and
paired significance tests. The reported improved rows have positive retained
intervals; both registry and iOS upload examples have two-sided sign p9.54e-7;
the Mac16MiB upload latency p-value is2.098e-5. Synthetic timing
is not an end-to-end app performance guarantee. Upload profiles used coordinated pauses of team compiles/tests; no CPU affinity
or complete host isolation is claimed. Other profiles state their cache conditions.

Upload RSS is whole-fixture `time -l`, including deliberate body copies used to
verify bytes: project4×8MiB81.363→81.510MB median. It is neither allocation
attribution nor a real-device upper bound. Mac requested allocation reductions
do not establish lower RSS or peak heap. Streaming was withheld after an actual
307 replay regression; unobserved multi-upload producers were not invented.

The room-activity index adds27,045,888 bytes per million generated rows and took
0.280s in the retained Python DDL replay. Realistic SQL WAL/NORMAL writes rose
26.158→34.627us (paired median +8.926us,95% interval5.415–10.908us). This is synthetic
SQL write cost, not native Db.append/Loro/send cost. An unread index was rejected
because broad-ACL cases regressed; premature FTS filtering would have violated
historical scrub visibility. Directory fallback remains its existing hard-bound
36 queries; adding a four-slot queue without a measured benefit would add rounds.
[Runnable database profiles and standalone charts](https://github.com/cisterciansis/arbos/tree/improve/database-scale/.improve/load-test)
retain the complete local measurements. No owned PostgreSQL jsonb hot path was found. Existing SDK caches and URL security
validation remain where they serve their current contract.

## Verification status

The corrected integration is tree `f3903159bb239a21f68a62ec029b82102ffe3f87`, reproduced from immutable
phase 3–9 source pins by the [versioned recipe](integration/assembly/README.md).
Its source manifest contains 2,724 regular files / 68,696,786 bytes;
SHA256 `c9cf2b2d0cec1f7b2369561ba3c66e86ae5e5f25148695f55ad901057748b877`. Assembly provenance SHA256 is
`f6a01163d0bb14fed10ba2c45faf7ca6489b3e331b0226c20155577e665ba9af`. The recipe performs no merge, ref update, application
execution or network access. Metadata publication advances PR heads separately
from the pinned code input. Phase3 is `658266a56e21eff1f740b6040d802033b4620873`;
Phase4 is `03a12e30e7d956070cfcacfd0e30cd35fca16598`.

The [source projection](verification/source-projection/README.md) lists every
change from historical C19 tree `c19a6e2ab764c53065d8f79e53bbe90f76508c4d`.
All Rust/Cargo/build/fixture inputs, harness sources and external Rust include
inputs are unchanged. Its Linux execution is reused by exact input identity;
it is not relabeled as a new workflow run on the corrected commit. Nine reviewed
Swift/test files change, alongside separately checked workflow/helper/proof/docs
inputs. The [native compiler/project/resource/fixture closure](verification/source-projection/native/README.md)
matches the corrected assembly at all470 tracked inputs. Uncommitted generated FFI artifacts are verified separately,
rather than claimed as contents of a source-only assembly.

Completed Linux runs below have actual parsed test summaries and
runtime fence evidence. Green workflow badges alone are not test counts.

| Source | Fork run | Rust P/F/ignored | Harness P/F/skip | Other gates |
|---|---|---:|---:|---|
| P3 `d40572b8f` | [42826558559](https://github.com/cisterciansis/arbos/actions/runs/42826558559) |2423/0/3|14/0/0|Format, all-target Clippy, typecheck,31 isolation contracts, shell/workflow lint and image lint pass |
| P4 `6247938b6` | [42833942694](https://github.com/cisterciansis/arbos/actions/runs/42833942694) |2548/0/3|94/0/0|Same gates pass; Rust job693s including513s fenced full-workspace step |
| P5 `ef3cfc72a` | [42833941005](https://github.com/cisterciansis/arbos/actions/runs/42833941005) |2373/0/3|14/0/0|Format, Clippy, typecheck,19 Rust isolation contracts, shell/workflow lint and image lint pass. Swift is N/A for this logging-only head. |
| P6 `84cb882fe` | [42828799835](https://github.com/cisterciansis/arbos/actions/runs/42828799835) |2374/0/3|14/0/0|All four Linux jobs and31 isolation contracts pass |
| P7 `a35e9ccfd` | [42833940432](https://github.com/cisterciansis/arbos/actions/runs/42833940432) |2387/0/3|14/0/0|All four Linux jobs pass, including31 isolation contracts |
| P8 `b15e3f9ba` | [42826558764](https://github.com/cisterciansis/arbos/actions/runs/42826558764) |2376/0/3|14/0/0|All four Linux jobs and31 isolation contracts pass |
| Combined `419d3944c` | [42833942969](https://github.com/cisterciansis/arbos/actions/runs/42833942969) |2586/0/3|94/0/0|Same gates pass;25 Rust summaries,16 nonzero. Rust job2106s including1461s fenced build/test step |

These are independent source projections; their counts overlap and are not
summed. All Rust summaries follow the real namespace smoke check. All seven Linux
rows are terminal passes. Runtime results, source identities, step durations and
scope are retained in the [CI verification packet](verification/ci/README.md).

Native results are distinct executions; overlapping cases are not summed:

| Source and scope | Unit results | UI results | Result |
|---|---|---|---|
| Historical hosted composition `a4d22466` |548 console passes /5 failures|27 passes /28 failures /13 skips|Actual source failures; fixture admission, formatting assertions and repeated row identity were investigated and corrected |
| Corrected complete local run, eight Swift overrides |555 strict passes /1 intentional expected failure /1 global-counter failure|55 passes /0 failures /13 existing opt-in skips; every scrolling/first-frame case completed|Process65 in1,571.339s; accurately retained as a failed full run |
| Final DEBUG observer successor, all unit cases +entire avatar class |558 strict passes /1 intentional expected failure /0 failures /0 skips|5 passes /0 failures /0 skips|Process0 in77.863s; normal all-target incremental build13.297s |

The final successor changes only DEBUG lifetime observation and its XCTest
cases. Entire ChatView outside that DEBUG block, Release class projection and
all UI test/gesture/timing source are byte-identical to its predecessor. The
final hosted-close test and two unrelated-release/intentional-retention controls
pass. This establishes the declared final scope; the complete UI/performance run
belongs to the preceding observer version. It is not a final-source unfiltered
whole-UI-suite pass. `LinkPreviewCardTests.testMissingUserRowIsAnAssertionFailure`
is the intentional expected-failure control; console pass counts include it.
See [complete UI receipt](verification/swift/corrected-full/README.md) and
[final observer receipt](verification/swift/observer-final/README.md).

Native execution caught and resolved real integration problems:

- Repeated inner `.id(account)` values produced blank transcripts. Removing that
  inner identity retains the account-keyed host, unique row IDs and owner equality.
  The unchanged five-case avatar class changed from3 failures/2 controls passing
  to5 passes on the same simulator. This is a causal rendering repair, not a speed
  measurement or a gesture/threshold adjustment.
- Direct DEBUG `loadFixture()` lacked relay credentials, so an account-work guard
  refused its existing local first-send branch. Tagged local lifetime authority
  restores that path while live work keeps its pre-task captured API. Original
  live checks and18 draft controls pass; credentials and storage are not fabricated.
- Library date formatters now honor the injected calendar timezone. Opposite-zone
  boundary regressions fail before the two-line repair and pass afterward.
  The local-time BlockedState fixture preserves its literal14:30 expectation.
- Four brittle formatting assertions moved to compiler/runtime consumer contracts;
  native methods and other assertions remain. P3 has6 row-input checks; P4 extends
  to29 call/callback checks, with actual omission/ownership/order mutations caught.
- A global live count could both falsely fail a freed draft and conceal retention
  when another draft freed. Weak identity probes replace that oracle without
  longer sleeps; Release behavior is unchanged.

The initial private source copy omitted110 repository goldens. That attempt also
hit its outer time ceiling and remains incomplete evidence. Complete fixture
closure was restored before the later complete/native successor runs; no iOS
resource additions confound the one-line row-identity UI comparison. Earlier
Unix-socket runner setup failures contain no assertion verdicts and remain
separate from actual source failures. The official matching Apple26.2 simulator
runtime was verified; existing devices/runtimes were preserved. Only the owned
verification simulator was shut down after results were frozen.

Historical hosted Swift reds remain visible. P8 has551 console unit passes and
54 UI passes /1 UI failure /13 skips. P6 has550 unit passes /1 failure and54 UI
passes /1 failure /13 skips: its fallback add-member case and sender-avatar search
case failed, with no unit assertion detail in the uploaded/console logs. The
relevant unit fixtures/direct methods and UI source match the baseline; no
unsupported flaky/setup diagnosis is made. The current integrated full unit
runs pass the fallback case, and the completed full UI run passes that exact
sender-avatar case. These later results do not retrospectively pass standalone
P6. [Hosted P6 evidence](verification/swift/hosted-p6/README.md) records that limit.
The earlier separate search-landing failure belongs to open PR#2539; its branch,
gestures and tests were untouched. That existing case also passes in the completed
local full UI run, without a claim that this work fixes every intermittent cause.
Other P3/P4/P7/new-C19 hosted Swift runs failed to acquire macOS capacity and ran
no assertions. P5's eligibility-only success is N/A for its logging-only source.

[Original baseline coverage](verification/baseline/README.md) and additional gates
are accounted for proportionately:

| Gate | Final disposition |
|---|---|
| Format, full Rust compilation/tests, harness typecheck/build/tests |All seven completed Linux runs pass; final Rust source2586P/0F/3existing ignored, harness94P/0F/0skip. Counts overlap across sources. |
| Avatar/glass lint; merge/rebase/verify-cache/deploy selftests |Actual supported Linux Layout passes. Original Mac Bash3/GNU-stat failures remain historical platform limitations. |
| iOS control-size lint |Fresh corrected-source pass in0.072s; exact input hashes retained. |
| Original78 Python files |74 unchanged; four changed modules have actual-current parse/import coverage in CI/feedback/voice checks. No fresh78-file invocation or aggregate claim for all new helpers. |
| Original3 standalone JS files |Two unchanged; current edge Worker `node --check` passes and37 actual tests pass. |
| Voice SDK prerequisites |Safe four-file suite84P/0F/0skip,6.203s, real SDK imports with synthetic effects. Opt-in live/evaluation scripts excluded. |
| Busy-reply fake relay |2P/0F/0skip,29.112s, helper/test bytes unchanged. |
| Mac app |Actual all-target Clippy29.505s and complete binary link53.687s pass on unchanged final Mac/Rust inputs; network denied, no app launch. |
| iOS app/test build |Normal actual Xcode all-target build passes. Compiler membership:93 app sources +1generated asset,4 extension entries,85 unit sources,13 unique UI sources (14PBX entries). Incremental build is not a fresh recompilation of every source. |
| Native FFI |All12 workspace packages cleaned for host and explicit arm64 simulator; locked offline rebuild11.840s, UniFFI generation1.615s; interface bytes and280 declared C exports match. Compile/ABI evidence, distinct from XCTest. |
| Final scoped iOS checks |305 account-work/38groups35.707s;122chat8.211s;80media6.065s;5 compiler contracts0.785s;29 consumer1.488s;29 calendar1.670s. Finite synthetic effects; suites overlap. |
| New Swift workflow |Native actionlint1.7.12 passes0.023s; seven existing eligibility tests and five decision cases pass for each phase. External local ShellCheck/pyflakes unavailable; native prior CI ShellCheck remains separate evidence. |

The [Phase3 structural exit](https://github.com/cisterciansis/arbos/blob/658266a56e21eff1f740b6040d802033b4620873/.improve/structural-checks/structural-exit.md)
records the825/868-file inventory, dependency review, Comments and Tests last,
and repeated independent review. Concrete repeat findings and native-test
regressions are fixed and checked against actual source. This finite review found
no additional worthwhile in-scope fix; it is not an every-statement correctness
proof. Persistent unknown legacy-cache provenance remains explicitly owner work.

Runtime tests use local/synthetic data. Canonical Linux tests verify a fresh
only-loopback namespace, nonroot identity, no supplementary groups/capabilities,
NoNewPrivs and external ENETUNREACH. The local Docker follow-up uses its separately
reviewed closed-routing fallback adapter, not an only-loopback inventory claim.
Native parent-process network denial allows only the exact owned active-boot
Apple testmanagerd socket. All unit/UI hosts and the UI app have verified loopback
relay endpoints. This does not establish universal simulator egress isolation.
Two earlier owned fork Swift runs with a reachable hosted-relay probe were
cancelled; actual contact was unverified. Subsequent plans use the reviewed
endpoint boundary. Public pinned dependency/SDK downloads occur outside runtime
fences. No shared database, deployment, provider session or real user database
was used. Private logs, build/SDK artifacts, synthetic databases and the local
uncommitted decision ledger are excluded from all PRs.

## Needs the owner

1. **Observability with Jacob (const).** Confirm existing telemetry backend and
   data/identifier/sampling contract. Recommended default: keep completed logging
   correctness/privacy fixes; defer broad OTLP tracing, propagation, async-function
   lint, wide terminal events and tail sampling until that answer. This does not
   block the other phases and creates no account or endpoint.
2. **Review intentional high-risk behavior before merging.** The PRs identify
   fail-closed auth/privacy/configured approvals, at-most-once consumed receipts,
   billing transaction faults and deployment admission. Recommended default:
   retain tested refusal/uncertain-effect semantics; review existing fail-open
   credit read/decode policy and manual cooperative deployment paths separately.
   No silent pricing/signing/calibration change or production application occurred.
3. **Persistent legacy cache ownership.** In-process generations and close/open
   serialization do not prove who owns an existing unbound legacy cache file.
   Recommended default: a new hash namespace and persisted binding with a reviewed
   quarantine/migration plan; do not silently reuse or delete possibly unsent data.
4. **Database index storage/startup/write budget.** Recommended default: review
   retained sizes, DDL and write profiles before adopting migrations. No DDL was
   applied to a shared database. Validate native append/device cost and operational
   rollout using the team's own controlled acceptance process.
5. **Optional Bend2 proofs.** Toolchain absent. Recommended default: retain runnable
   generated laws, source hashes, counterexamples and actual mutants now; add formal
   proof artifacts later without replacing native boundary/concurrency tests.
