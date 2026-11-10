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
