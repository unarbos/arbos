# Viewer, Library and link-card account origin checks

A rendered A viewer or Library row could queue work after B replaced the account, or publish A media/clipboard/Photos results after retirement. `MediaOrigin` carries the existing account receipt through these actions. It creates no new generation and no API instance. Supplied stale/unavailable chat context never falls back to B. Standalone viewers capture their origin at render/appearance; asynchronous closures retain that value. Library cards pass one origin through rows, previews and the viewer.

From the repository root, on macOS with Xcode command-line Swift and Python 3:

```sh
python3 -I -B .improve/state-machine-checks/ios-media/run.py --out /tmp/arbos-media-checks
python3 -I -B .improve/state-machine-checks/ios-media/run_mutants.py --tree . --out /tmp/arbos-media-mutants
```

Output directories must be fresh. `--tree PATH` can select an isolated source snapshot. Swift fixtures compile into temporary directories, execute with an empty credential environment and macOS `deny network*`, then are removed. There are no package installs, services, real credentials, Photos writes, devices or clipboard writes.

The original 48 checks comprise 8 actual copy callbacks/shared methods, 20 actual save/image/video/file methods, 8 exact rendered constructor expressions, and 12 actual media-origin/existing-receipt predicate controls. Production bodies and constructor expressions are extracted verbatim; models, codec/player, fetch, Photos permission/save, temporary-file and clipboard effects are synthetic. The baseline-only `MediaOrigin` alias stores an existing rendered chat receipt that legacy methods did not consume; it does not implement a replacement policy. The source hash and fixture hash are recorded, and source changes during a run fail the check. The identical image loader body used by the viewer and `BlobImage` is also pinned in `PARITY.json`.

Canonical `0f3044dff` baseline: copy 5 failed/3 controls passed (1.668s); save/pages 12 failed/8 controls passed (1.820s). These 28 behavioral checks become 28/28 green. Constructor propagation separately fails 8/8 on the baseline because it did not carry a rendered origin (1.483s), then passes 8/8. The 12 origin controls apply to the newly introduced predicate; no baseline pass count is invented. The additional private link-preview load has 4 baseline failures/4 controls (0.982s), then 8/8 pass (1.223s). Eighteen exact constructor controls cover all three link cards and their BlockView/TypedBody/quote propagation. The exact link-tap callback adds 6 controls (current, fixture, retired, canceled, anonymous and absent URL): baseline 3 failed/3 controls (0.656s), then 6/6 pass (0.685s). Final 80/80 pass. Timings include Swift fixture compilation, are verification timings under shared CPU load, and make no performance claim.

Three actual-function mutants are retained and detected with identical deterministic replay: dropping the copied predicate produces 2 failures/6 controls; removing the post-Photos-permission owner check produces 1 failure/19 controls. Removing the post-icon-fetch owner check additionally produces 1 failure/7 controls. They modify disposable source copies only. This is deterministic regression detection, not a mathematical proof or randomized shrink claim.

Public profile pictures keep their existing behavior: `pictureURL` fetches have no bearer; the original viewer exposes Close/read/zoom only for pictures, and has no Copy/Download/Forward affordance. Private blobs require an account or explicit fixture receipt. Numeric calibration, gesture code, media URLs/headers/status handling, fixture painting, MIME/file naming and platform helpers remain unchanged. An already-issued HTTP or Photos effect cannot be recalled; retirement prevents later admission/publication and notices. These selected-source checks do not render SwiftUI or exercise real Photos, playback or devices; the full application and XCTest SDK compile is a separate gate.

See `SOURCE-INVENTORY.md`, `GATES.json`, `PARITY.json` and `mutants/RESULTS.json` for exact scope and evidence.

The final signature-only follow-up makes `isCurrent` mandatory on `MediaFetch.bytes`, `copyMessage`, `copyAttachment` and their private copy helper. All 10 fetch, 2 attachment, 1 message and 2 internal helper calls already supplied it; method bodies are unchanged. Historical `GATES.json`, `SOURCE-FROZEN.json`, and complete-enum parity refer to the preceding functional checkpoint. `CONTRACT.json` pins the subsequent two-file signature change and caller inventory; the functional runner still contains 80 checks.

The separate compiler contract checks one explicit-call control and four omitted-argument refusals using the exact production signatures with inert bodies; no body executes and no SDK behavior is inferred:

```sh
python3 -I -B .improve/state-machine-checks/ios-media/check_required.py --tree . --expect-required --out /tmp/arbos-required-origins
```
