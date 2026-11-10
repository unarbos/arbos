# iOS chat callback ownership checks

Run on macOS with the project Swift toolchain, from any working directory:

```sh
python3 .improve/state-machine-checks/ios-chat/run_all.py
```

`--source-root /path/to/checkout` selects a separate materialized source tree;
`--output /new/directory` retains compiler logs, generated fixtures and JSON gates.
The runner extracts actual product method/closure bodies, the real tagged
`ChatAccountContext`, WorkLifetime and account acceptance helper at runtime. It
pins all eight input source files before and after all checks. There is no copied
transition or algorithm used as an implementation oracle.

The 122 cases cover stale, unavailable, cancelled and current card/session/menu
callbacks; same-owner refreshed-API row equality; different-owner invalidation;
late image decode, thumbnail, audio and clipboard completion; fetch admission
before current-model URL/bearer reads; and current-scroll versus account-retirement
playback cleanup. URLSession, MediaFetch, AVAudioPlayer, API, cache and pasteboard
collaborators are local fakes. No account credentials or user files are read.
Every executable fixture runs with a clean environment and macOS network denial.
These are selected controller checks, not rendered SwiftUI or physical audio tests.
Full iOS SDK compilation is a separate gate.

[Recorded before results](RESULTS-BEFORE.json) retain 111 original-body cases:
61 semantic failures and 50 positive controls. The original StickerBubble work
parameter is exercised unchanged (an earlier normalized local trial is excluded).
[Final results](RESULTS-AFTER.json) retain all 122 passing cases in 12 groups,
with measured duration in the JSON artifact, with the exact eight source hashes unchanged through execution.
The extra five cases protect normal playback while exercising retirement cleanup;
the pending-permission slide-cancel path now revokes the recorder request even
before recording starts (one actual semantic failure plus five preserved controls);
the initial draft incorrectly stopped playback in a cancelled current-owner scroll
callback, and the retained refinement fixes that distinction.
[Preservation evidence](PRESERVATION.json) pins unchanged sendMemo and audio
session/player/timer methods, unchanged original numeric tokens, and the strict
StickerBubble-only edit boundary in FunPicker.
