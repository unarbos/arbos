# Phase 3: structural refactoring

**Status: structural implementation and the finite Comments/Tests repeat are
complete; final combined runtime verification is recorded in the finish report.**
The original pure extractions were followed by actual native durability,
authorization, room ownership and render-policy fixes. Full Mac all-target Clippy
and application linking pass in the reviewed composition. The final iOS app and
test-source SDK gates are separate from simulator execution and device acceptance.
Historical capacity/SDK/approval blockers in earlier reviews are superseded.

Scope and all findings are in [backend review](reviews/structure-backend.md),
[client review](reviews/structure-clients.md) and
[auxiliary review](reviews/structure-auxiliary.md). Broad inventories and focused
flow reads covered all domains; this does not claim every line was audited.
Problems and grouped redesigns were written before source changes.

## Implemented shared seams

- Mac chat titles: `app.rs` and `sync/fixture.rs` had identical first-line,
  trim/34-character/fallback rules. `sync/relay.rs` imported this stable policy
  from the volatile GPUI app module. All three now use dependency-free
  `chat_title.rs`. The implementation is unchanged; eight behavior checks pin
  empty/first-line/CRLF/Unicode/truncation behavior before and after extraction.
- Harness cancellation evidence: `StopJobResult`, `sweepClause` and `stopJobText`
  now live together in import-free `stop-job-text.ts`. Pure callers can use this
  evidence-preserving presentation rule without initializing environment-driven
  pi registration. `index.ts` keeps its existing named exports and uses the same
  function. All real-hook integration tests stay in place.

No generic framework, configuration layer or speculative adapter was added.
Effect is not introduced into these pure Rust/TypeScript seams. Native code stays
native; a later development-only Effect FastCheck dependency is appropriate for
generated TypeScript laws, without converting stable runtime code.

## Comments and Tests

Both additions were applied at the implemented seams and repeated after the
structural changes, with Tests last:

| Addition | Requested description | Result |
| --- | --- | --- |
| Comments | Remove comments that only restate the code. | Reviewed touched seams; retained external protocol/incident/absence semantics and calibration explanations. Touched comments reviewed again with the later removals; externally justified constraints remain. |
| Tests | Remove tests that wouldn't catch a real bug, and fix the ones that fail without one. | Reviewed last. Kept all 14 harness tests, including real-hook and negative controls. Later replaced four test-only scheduler-model checks with seven checks of the actual production decision module; three real-function mutants are detected. Other tests remain. |

## Before / after

Times are gate durations, not speedup measurements; warm filesystem/tool caches
make comparisons unsuitable for application performance claims.

| Row | Before | After |
| --- | --- | --- |
| Harness typecheck | pass, 3.498 s | pass, 2.889 s |
| Harness build (`tsc --noEmit`) | pass, 3.916 s | pass, 2.456 s |
| Harness tests | 14 passed, 0 failed/skipped; 0.711 s | 14 passed, 0 failed/skipped; 0.530 s |
| Mac title behavior | 8 passed, 0 failed/ignored; original function | 8 passed, 0 failed/ignored; actual shared module |
| Title test compilation / execution | 0.664 / 0.249 s | 0.114 / 0.210 s |
| Workspace formatting | baseline pass, 3.016 s | pass, 2.607 s |
| Full native clippy/check/test/build | unmeasured at initial extraction | Actual full Mac all-target Clippy and app linking pass in composition; runtime CI is recorded in the finish report |
| Title definitions / UI dependency | 2 copies; sync → app | 1 shared definition; sync → pure helper |
| Cancellation renderer runtime imports | bundled in environment-loaded extension | 0 in the pure module |
| Intended behavior | baseline | unchanged |

The dependency direction is improved at these two seams. It is not reasonable
to claim every import or special case across the entire repository is resolved.
Independent review distinguishes the original extraction gates, the later finite
whole-repository structural repeat, and final runtime CI; none proves every line.

## Subsequent regression-driven structural work

The earlier blanket SDK/disk/CI blockers were reassessed. Targeted native core/client
builds passed174tests with network denied; the fork now runs its own free standard
runners. The following changes use actual source and local failure fixtures.

- **Atomic append receipt.** One connection-level batch lookup preserves the exact
  stored/quarantined dedupe query. Append's dedupe, room initialization, head
  increment and row insert now share one SQLite transaction. Real insert faults
  can no longer leave an empty room position or a head promising nonexistent
  bytes; retry preserves sequence numbering, original bytes/device and room scope.
- **Privacy at its shared authorization boundary.** Group addition permits only a
  successfully read `anyone`, or `people_i_know` with an existing DM. Read errors,
  unknown values and `nobody` refuse through the existing opaque403. Actual group
  create/member-add/project-share/invite handlers leave no partial writes. Existing
  missing-row/NULL open defaults, input parsing and ownership checks stay intact.
- **Replayable sidecar page receipt.** Complete mirror application precedes local
  wake handoff, and the cursor is installed only after persistence succeeds.
  Errors now propagate. A matching already-mirrored uncommitted event can recover
  its wake, while completed-page/history/missing-ID dedup remains. Page-local
  dedup uses complete chat/event/message identity: two distinct messages must not
  collapse when event positions are missing or shared. This is local
  channel acceptance, **not** durable agent delivery or an end-to-end exactly-once/
  at-least-once guarantee; crash windows remain documented.
- **Room-owned pending indicators.** A production `PendingMessages` module makes
  room keys mandatory at insert/projection/failure-removal/ACK edges. One room or
  project-doc ACK no longer clears another chat. Existing within-room any-ACK
  clear-all behavior stays explicit; no exact batch/message receipt is inferred.
- **Actual scheduling tests.** `forward_changes` now uses the std-only snapshot
  decision module tested directly. The80ms first trailing edge, immediate live
  paints, LinkLive suppression, supplied Tokio clock and message-upsert/project-doc
  bypass remain. Four old tests of two separate test-only implementations were
  removed last, after the seven actual-source checks caught three mutants.
- **Stable client policies.** Identical relay/fixture agent-token matching now has
  one std-only owner; collective boundaries intentionally differ. Link-expiry
  choices/default labels move together below transport/UI, and transport calls the
  actual core avatar policy directly. No targeting/expiry/privilege policy changes.
- **Proven surplus.** Removed nine unused backend JSON/feature/fetch wrappers and
  their stale prose, retaining real typed DTO/fixture APIs. Removed unreachable
  pane-call/grant-picker plumbing and unused pane DSL pieces after tracked
  source/test/rig/cfg/doc reference inspection. Real chat calls and backend consent
  APIs remain. The unresolved approval-notification producer is explicitly retained.
- **Cross-runtime route contract.** One102-name/35-route fixture binds actual core
  and Worker code. Worker now refuses core-inallocatable punycode, double-dash app
  and reserved routes; legitimate landing/slash adapters remain. This intentional
  bug fix had failing-before Worker cases and shared actual Rust tests.

The reviewed Swift test-plan isolation is included as a verification prerequisite:
any core-path push can start the unfiltered simulator gate. It is identical to the
Phase9 guard, keeps the complete scheme, patches/verifies test-host and UI-app
relay URLs before test execution, and has no unfenced fallback.

| Subsequent seam | Before | After |
| --- | --- | --- |
| Append transaction |1positive/2atomicity failures; failed insert advanced room position |3pass; rollback and retry numbering pinned |
| Privacy actual handlers |6controls pass/5new refusals fail with200/201 |11pass; opaque403 and no partial writes |
| Relay broader filters | Original source controls retained |33DBpass/1pre-existing live E2E ignored;48peoplepass; filters overlap |
| Relay/native combined gates | Earlier local capacity blocked broad build; later shared-target records superseded | Fresh isolated core/client/relay/sidecar all-target Clippy19.673s and exact35regressions pass; broader isolated controls retained |
| Sidecar page receipt | Isolated same12case set7pass/5fail,4.462s; later two identity-preservation cases fail against the intermediate refactor | Final15daemonpass2.780s+4mirrorpass.292s; isolated package checks pass |
| Room pending |4fail/1same-room control on extracted production module,.236s | Same5pass,.185s; strict std compile pass |
| Snapshot scheduling |4checks of test-only copies |7actual-source checks pass,.184s;3broken production decisions detected |
| Mention/expiry policies | Identical original3+2checks pass | Same5checks pass; token copies2→1; two expiry transport→UI edges removed |
| Mac std-only production modules | Existing title8 plus new pinned original cases |32pass/0fail/ignored, including7room ownership checks;3snapshot mutants detected by portable runner |
| Route parity | Worker9fail/28pass | Worker37pass,.079s; actual Rust2pass52.109s incl feature compilation |
| Backend surplus checks | Nine wrappers unused by runtime; baseline44isolated controls pass | Same44isolated native controls pass(42relay+2core); check18.551s/clippy20.687s/build30.601s/format pass |
| Swift plan contracts | Original12groups contain13failing subcase assertions |12groups pass; no app launch in these local checks |

A count guard caught Cargo selecting zero tests from a reused shared-target
artifact. Those native records were withdrawn and replaced by forced workspace
rebuilds in unique per-snapshot targets; exact nonzero counts are mandatory.
[The correction](structural-checks/cache-isolation.md) and authoritative isolated
commands are retained. Native SQL C-driver measurements, direct-rustc/Node and
voice checks use separate runtimes/targets.

These are check durations, not performance measurements. Package-filtered counts
overlap. The first broad fork CI run covers the independent finish/baseline head,
not all these new changes; combined native integration is still required. Compact
commands/hashes and the portable runner are in [structural checks](structural-checks/README.md).

## Structural exit and retained requirements

The finite second pass covered all12 Rust crates plus iOS/FFI, harness, voice,
edge/site, image, operational scripts, evals and CI. It reviewed825 Phase3 and868
Phase4 selected source/config files, with dependency edges and scoped flow reads.
Raw marker/comment matches were navigation aids, not defect counts or permission
to delete compatibility behavior. The summary and exact historical source pins
are retained in [structural-exit.md](structural-checks/structural-exit.md).

All three concrete findings from that repeat are implemented: the last transport
imports of UI-only OS-open helpers now use shared `external_open.rs`; iOS preview
policy is explicit and part of cache identity; and a missing mandatory synthetic
row fails its test instead of becoming a skip. Full Mac compilation caught one
missed chart-export caller; it was corrected and the complete caller set rebuilt.
The two actual Mac preview-builder runtime cases now pass as well.

Account/callback ownership and bounded hydration/uploads are implemented by
Phases4/6 at the same shared boundaries. Phase7 addresses the measured roster N+1.
The reviewed final media delta keeps account origin through nested viewer/library
operations. No further concrete worthwhile structural fix remained in the finite
review; this is a bounded engineering review, not exhaustive certification.
Existing protocol host adapters, signed verification, quarantine/legacy
compatibility, physical timing and OS/media workarounds remain requirements.

The approval-notification hook has no established producer. Its declared
security-adjacent surface is retained with stable identity, replay/decision races,
safe summary, account lifetime and current visibility/mute/coalescing/click rules;
deleting it would not establish a replacement alert. Unknown persistent legacy
cache ownership remains explicitly owner work. The finish report distinguishes
local compiler/controller proof, final CI and device acceptance. Broader telemetry
is deferred for the owner's discussion with Jacob (const), without blocking this
structural work.

## Needs the owner

Review the intentional fail-closed privacy, append/receipt, pending-indicator and
route-refusal fixes with their failing-before regressions. Recommended default:
retain opaque denials and existing replay/quarantine/within-room receipt semantics,
then require combined native/device gates before merging. The final Phase9 report
collects owner decisions; no production, deployment, paid-runner or reduced-test
permission is requested here.


## Transcript-owned render policy

The process-global link-preview switch coupled separate AppState instances to
row construction, while its value was absent from the message-row cache key.
A warm transcript could therefore retain old preview rows after the setting
changed. The row builder and message menu now take the current AppState setting
explicitly, and that immutable value participates in cache identity. Stored
preference/default, sender removal, card content/layout and plain links stay the
same. The global atomic and its test-only serialization lock are removed.

A local harness compiles the exact extracted production fingerprint and real
message DTO definitions, without GPUI/SDK effects. The original source passes
same-policy stability and fails changed-policy invalidation; the candidate
passes both assertions (9.482/9.966 s including compilation). This is a real
function replay, not a copied hash model. An initial incomplete harness manifest
failed compilation and is excluded from the RED evidence. Separate targets and
forced package cleanup prevent cross-snapshot artifact reuse.

Two retained native builder tests cover on→off→on through a warm cache, unchanged
policy cache hits, plain-link restoration, and independent transcript settings.
They compile in the actual full Mac all-target gate; both actual builder cases execute successfully in the native composition. The extraction does not prove the complete renderer. See
[render-policy.json](structural-checks/render-policy.json). No performance claim
is made for this settings correctness fix.

Independent read-only review confirms policy propagation at every caller and
cache lookup, unchanged row-version comparison, and removal of all global-switch
readers. Workspace formatting and whitespace checks pass. Full native caller compilation
and both actual builder runtime cases now pass in the composition.

## Owned room documents

The document registry previously held one mutex through snapshot reads, imports,
exports and writes for every room. It now returns an owned handle after a short
lookup; expensive work holds only that room's mutex. Core documents contain
RefCell/Cell, so same-room synchronization stays. Original handles follow room
sessions and forward-change tasks instead of being looked up again after awaits.
Replacement/retirement marks handles before detaching them; publication checks
pointer identity under the registry lock. Publishers are short channel/in-memory
operations and never await, perform SQL or lock a document. Teardown drops
resources outside registry/account guards, including poisoned-publisher cleanup.

An exact-source harness pauses the real snapshot method's document collaborator
and invokes the real newest-message method for another room. The original passes
four payload/default assertions but fails independent-room progress; the final
candidate passes all five assertions (19.270s including isolated compilation).
Seven retained tests of the actual production ownership module cover progress,
replacement, retirement before/after a wait, and destruction. A mutant removing
the post-lock retirement check is detected. See
[room-docs.json](structural-checks/room-docs.json) for hashes and exact scope.

Independent bounded review found no registry/document lock-order defect. This
change does not fence every mutable Shared background task or store flush. The
separate Phase4 account close/replacement hooks and fresh per-account Shared
boundary remain prerequisites for broader account-isolation claims. No measured
whole-app speedup, cross-room throughput or forced cancellation of running Loro
work is claimed.

The actual Mac all-target compiler/Clippy gate passes with warnings denied
(23.291s); see [native-mac.json](structural-checks/native-mac.json). The scoped
runtime checks do not replace full combined tests or device/media acceptance.
Native compilation also exposed unused platform-only clipboard helpers and two
unconstructed commands left by removed grant-picker UI. Clipboard helpers now
compile only on their actual platform/test path; the unused command/fixture/
transport arms are removed. Server consent routes, DTOs and authorization stay
intact. The SF cache type alias and const thread-local initialization preserve
runtime behavior without lint suppressions.
