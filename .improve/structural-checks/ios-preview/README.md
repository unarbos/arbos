# iOS transcript preview policy and mandatory-row checks

Transcript rows take an immutable `linkPreviews: Bool` input (default true for
legacy callers). ChatView supplies its model's current setting to every builder
and menu, includes it in cached-row identity, and refreshes on setting changes.
An unchanged message list, streaming tail or append cannot retain rows rendered
with the previous policy. Same-policy unchanged/tail reuse remains. The unsafe
process-global switch and AppModel's side writes are removed; saved preference,
per-message preview removal, plain links, row layout and physical constants stay.

The existing test helper now uses XCTUnwrap for a mandatory user row. Its retained
XCTExpectFailure negative check requires an assertion on absence; a missing row
cannot silently skip the rendering regression. All existing test cases remain,
using per-call policy rather than shared test-global toggles.

Run on a macOS Xcode host (no simulator/app/provider or account required):

```sh
python3 -I -B .improve/structural-checks/ios-preview/check.py --mutants
```

The runner owns a no-egress Seatbelt fence, clean HOME/TMPDIR and PATH-only child
environment; there is no unfenced fallback. Invoke it outside another Seatbelt
fence, since macOS does not support nesting these profiles. It reads only six
fixed source files plus its explicit local fixture files, records hashes and
asserts originals remain unchanged. Generated outputs are ignored. Non-macOS
invocation explicitly fails rather than claiming a skipped runtime check passed.

The host leaf compiles29literal actual definitions:19Model DTOs, MediaItem/Album,
Block/Quote/RowKind/MediaSender/Row/Transcript/Delivery, and ChatView.refreshRows.
The actual cache method is enclosed in a test owner with observer/data fixtures;
no cache decision or renderer is reimplemented. The test account collaborator is
always current solely to reach the guarded cache method; account rejection and
retirement are covered by separate actual ownership checks, not this fixture. Baseline/new API binding adapters
only provide the same policy input through the old global or new argument.
Twelve output scenarios include two caller-isolation failures and stale cached
previews on unchanged/tail/append paths, plus seven positive controls. The fixed
source passes all12. An additional real host XCTest suite runs the actual native
userRow helper and retained negative test, plus a present-row control: original
1pass/1failure, repaired2pass. Actual source mutants that ignore user policy,
ignore cache policy or restore the skip are detected separately.

Scope limits: FFI date division is a fixed no-divider collaborator for this fixture;
markdown, math, system/membership and project-name collaborators fail if unexpectedly
called. No preview decision is stubbed. Real row/preview/message/media DTOs and
production row/cache bodies are used. The assistant fixture contains a link part
without prose. This does not certify FFI parsing/date rules, UIKit layout, SwiftUI
onChange scheduling, full AppModel initialization, a simulator or an entire app.
Full SDK compilation and fenced native UI tests remain separate integration gates.
Two original unused-prev compiler warnings in the actual cache method are retained;
this is a host compile, not a warnings-denied whole-app check.

Before capture used the original43b source:12row scenarios7pass/5semantic failures;
original mandatory helper1pass/1semantic failure. An incomplete exploratory leaf
and a prototype empty-text fixture that reached its intentional parser tripwire
are excluded from this RED evidence. GATES.json records exact source/function
hashes, durations and finite results. This is a correctness/structure change; gate
durations are not app-speed measurements or an exhaustive proof.
