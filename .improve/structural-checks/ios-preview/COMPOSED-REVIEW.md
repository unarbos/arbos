# Final Root preview/callback composition: scoped read-only review

**PASS for preview propagation/cache/helper composition. No new actionable defect found.**
This review is independent of Root's manual composition; the reviewer authored the
original preview patch and does not claim independent authorship review of that patch.
Exact four final source hashes are SOURCE.json (Chat3b274c38, Transcript60012311,
AppModel7c944e04, Tests7884ca7d). No live source/index/branch was edited.

- ChatView.refreshRows1090 retains `guard account.accepts(model)` before reading
  policy or mutating cache state. Its complete body is byte-identical to the frozen
  preview method plus that admission guard. The preview check uses an explicitly
  always-current test account only to reach this guarded path; it makes no claim
  about actual account acceptance/retirement. Root's separate ownership controls
  remain that proof.
- Transcript's whole enum equals the frozen preview implementation. Both overloads
  take default-true immutable policy; forwarding passes it into the indexed form.
  User and assistant card decisions use that argument. No global switch remains
  in app/test source. All three production row builders (Chat1159/1187/1212) pass
  the captured model policy; the menu1473 uses the same model setting. Other calls
  are test/measurement callers where default-true is intentional compatibility.
- Policy identity is recorded on every normal return; changed policy bypasses both
  unchanged/tail and append shortcuts. The setting observer1742 calls refreshRows,
  whose admission guard remains first. Same-policy unchanged/tail reuse is pinned.
  Preference storage/default and per-message preview removal remain unchanged.
- Existing Map callback/RowView/media sections are outside the modified row-builder
  region and are not replaced by the preview patch. No extra await or new callback
  effect was added. Actual SwiftUI scheduling, UIKit layout and retirement semantics
  are not certified by this host leaf; full SDK and native UI gates remain separate.
- runWindowMs/runGap/turnGap/systemGap declarations match the frozen source exactly;
  no physical geometry, media/scroll cadence, parser or date policy was changed.
- The mandatory synthetic user-row helper now asserts via XCTUnwrap. Its actual
  retained XCTest negative detects a restored XCTSkip, while the present-row control
  still passes. Original existing LinkPreviewCardTests cases remain.

Final actual-source replay:12row/cache scenarios pass (compile1.541s/run.189s),
2real host XCTest cases pass (compile1.760s/run.236s). Three actual-source mutants
are detected: user-policy ignored6row failures; cache-policy ignored3row failures;
mandatory-row skip restored1helper failure. Full source hashes remain unchanged
through the run. GATES.json records exact scope/durations and COMPOSITION.json
records body/calibration equality.

The only portable-tool adaptation is fixture-only.patch (SHA2b9d569b7e604a77bdd115e1f4da38836d990fc78a56a6b05c5737372f14d3a6):
a minimal always-current account collaborator and a matching explicit scope note.
It changes no product source or policy/renderer expectation. Apply it to the Phase3
retained checker before using that checker on the combined Phase4 guarded source.
