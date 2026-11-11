# Structural exit: Comments and Tests last

The finite second pass selected source/config from all12 Rust crates, iOS/FFI,
harness, voice, edge/site, image, operational scripts, evals and CI. Exact historical
refs,825 Phase3 /868 Phase4 file hashes, domain counts and workspace dependency
edges are in [structural-inventory.json](structural-inventory.json). Evidence,
dependencies, runtime data, assets and `.env*` paths were excluded. This was broad
inventory with focused flow reads, not manual inspection of every statement.

28,060 raw marker matches and320 comment candidates were navigation aids; they
include tests/comments and are not defect counts or automatic deletion lists.
Simple literal-true assertion scanning found none. Comments tied to external
protocols/incidents/calibration were retained; tests were reviewed after the
structural changes and actual implementation mutations checked their sensitivity.

## Concrete repeat findings

| Finding | Shared fix | Evidence |
|---|---|---|
| Transport imported UI helpers for OS file launching | Move unchanged std-only launch/reveal functions into `external_open.rs`; app, chart, relay and fixture use it | Seven fake-PATH argument/stdio/failure controls; Quick Look flag mutant; full Mac all-target Clippy/app link. The initial targeted scan missed chart export; the compiler caught it and the caller was corrected. |
| Missing mandatory synthetic row became a skipped test | Use failing `XCTUnwrap`, retain actual expected-failure negative control | Original host XCTest1pass/1failure becomes2passes; removing the required row fails. See ios-preview/COMPOSED-GATES.json. |
| Ambient iOS preview switch was absent from warm-row cache identity | Pass immutable preview policy through builders/menu and include it in cache identity | Twelve actual warm-row/isolation/cache scenarios plus2hostXCTest cases;3 actual-source mutants detected. Existing account admission remains before cache/policy work. |

These three issues are implemented. The earlier Mac transcript policy, room-owned
handles/ACKs, atomic SQLite append, sidecar mirror/wake/cursor and shared title/
mention/avatar/expiry rules retain their actual-source regressions. Four copied
scheduler-model tests were removed only after seven production-module tests
caught three real-function mutations. No runtime test or assertion was dropped
to hide a product regression.

## Native test-contract follow-up

Actual native execution exposed a pre-existing mismatch between an injected
calendar and DateFormatter's ambient timezone. LibraryList's two date formatters
now explicitly use that calendar's timezone; default `.current` behavior, locale
and format words remain. One new XCTest case covers opposite timezones and day/
year boundaries. BlockedState's public sentence remains local-time behavior; its
14:30 test input is now constructed in that same local calendar rather than UTC.
The retained selected-source calendar gate has29 assertions, including eight
original-method `.current` parity controls. See
[calendar checks](../finish-checks/calendar/README.md).

The P3 preview argument also exposed one brittle closing-parenthesis source count.
Its replacement compiles all three actual tail/append/full rows calls against a
required hiddenSenders argument and checks nonempty/empty input values (six
checks). P4 extends the same CI command with23 actual callback checks, migrating
three more formatting literals without dropping native/UI test methods or other
assertions. Missing row input, result suppression, delayed ownership, picture
admission and close-before-reveal mutations are detected. See
[source-consumer checks](../finish-checks/source-consumers/README.md).

A separate P4 UI follow-up removed only the redundant inner `.id(account)` from
repeated RowView values. Host account identity, row IDs, owner equality and
interaction calibration remain. The same five existing avatar/back-swipe cases
changed from three sender-route failures plus two header controls to five passes.
That isolated causal class is not a whole-suite verdict. A later global composer
live-count failure (0 versus an earlier baseline1) did not identify a retained
hosted object. The DEBUG-only weak-instance observer and deterministic unrelated-
release controls pass in the actual integrated native successor: all559 unit
cases are accepted (one intentional expected-failure control), and all five
unchanged avatar cases pass. The separate preceding full UI run passes55 cases
with13 existing opt-in skips. These source versions and finite scopes remain
distinct in the finish report; no final-source unfiltered full-UI pass is claimed.

## Retained requirements and limits

Per-room synchronization remains because Loro documents contain interior mutable
state. Registry guards are short, owned handles and retirement checks prevent
stale publication, and resources drop outside registry/account guards. Existing
within-room any-ACK clearing, historical privacy defaults, replay/quarantine and
OS/media compatibility paths remain explicit requirements. Physical timing and
calibration were preserved; source/controller checks are not device acceptance.

The approval-notification hook has no established producer; its declared surface
and tested security/UI contract remain instead of inventing another producer.
Unknown persistent legacy cache provenance is retained as an owner migration
question. High account/process/media ownership and bounded hydration/uploads are
implemented by Phases4/6; measured roster batching is Phase7. Broader telemetry
is owner-deferred for discussion with Jacob (const).

No additional concrete worthwhile structural change was established by this
finite review. Final source composition, the bounded review of its deltas, full
compiler/runtime outcomes and explained red gates are recorded in the finish
report. This is a scoped engineering exit opinion, not exhaustive correctness,
formal proof, whole-app performance or a physical-device certification.
