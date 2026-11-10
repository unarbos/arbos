# Phase 3: structural refactoring

**Status: implementation and verification in progress; the global exit does not
pass yet.** The original two extractions were followed by native durability,
authorization, pending-room ownership and policy cleanup once targeted package
builds became feasible. The public contributor fork now runs its own isolated
CI; upstream approval is not a blanket native blocker. Full local GPUI capacity
remains below its documented floor. Outstanding work is listed explicitly below.

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

Both selected additions were applied in order:

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
| Full native clippy/check/test/build | unmeasured at initial extraction | Later targeted package checks and fork baseline gates below; complete combined GUI gate still pending |
| Title definitions / UI dependency | 2 copies; sync → app | 1 shared definition; sync → pure helper |
| Cancellation renderer runtime imports | bundled in environment-loaded extension | 0 in the pure module |
| Intended behavior | baseline | unchanged |

The dependency direction is improved at these two seams. It is not reasonable
to claim every import or special case across the entire repository is resolved.
Independent review must distinguish these verified extractions from the blocked
whole-repository exit condition.

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
| Mac std-only production modules | Existing title8 plus new pinned original cases |25pass/0fail/ignored;3snapshot mutants detected by portable runner |
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

## Remaining structural exit

The global doc-map lock/room handle lifetime, stale identity/native callbacks,
bounded hydration/upload work and ambient render-policy inputs remain real
follow-up work. They are not all externally blocked merely because tests must be
written. The measured roster N+1 is being addressed in Phase7. Voice ownership is
superseded by the independently verified Phase4 implementation. Existing protocol
host adapters, signed verification, quarantine/legacy compatibility and physical
OS/media workarounds are requirements, not arbitrary surplus to delete.

The approval-notification hook has no established producer. Keep the declared
security-adjacent surface while pinning stable request identity, replay/decision
races, safe summary, account lifetime and existing visibility/mute/coalescing/click
rules; deleting the hook would not establish a replacement alert. Full source
reviews and calibrated native/GUI checks remain necessary. No whole-codebase
“nothing worth fixing left” claim is made.

## Needs the owner

Review the intentional fail-closed privacy, append/receipt, pending-indicator and
route-refusal fixes with their failing-before regressions. Recommended default:
retain opaque denials and existing replay/quarantine/within-room receipt semantics,
then require combined native/device gates before merging. The final Phase9 report
collects owner decisions; no production, deployment, paid-runner or reduced-test
permission is requested here.
