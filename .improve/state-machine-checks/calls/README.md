# Native call ownership regressions

From any cwd, point Python at `run_all.py` by absolute path. In a checkout:

```
python3 .improve/state-machine-checks/calls/run_all.py
python3 .improve/state-machine-checks/calls/run_all.py --rust-only
```

The full macOS suite runs 66 checks; Linux can run the 47 Rust checks. `--tree PATH` selects a separate source snapshot. `--record-dir PATH` optionally records source/fixture hashes, exact test counts, durations and logs; ordinary runs leave the checkout unchanged. Each Rust fixture gets a fresh temporary Cargo target and offline cached dependencies, with Tokio's version derived from the checkout lock. Swift compiles in the project's Swift5 mode. macOS child execution also denies network and Shared reads. Linux CI supplies its existing network namespace fence.

Each bridge extracts actual named production methods or command arms, fails on unexpected layout, and substitutes only SDK/HTTP/UI collaborators. The Swift HTTP test uses an intercept-all URLProtocol while OS network access is denied. The Rust loop/relay/HTTP collaborators are in-memory futures; no network SDK, camera, microphone, CallKit, keychain, configuration or fixture app is launched. No production algorithms are copied into the assertions.

Checks cover permission reservation, canceled/late REST and media completion, old queued room callbacks, same-call replacement cleanup, account-bound post-leave effects, same-device sign-out/re-adoption receipts, refresh continuity, immutable stale-cleanup authority, actor-atomic bearer capture, pending connect cancellation, capture/session drop ownership, and server cleanup ordering. Positive paths prevent refusal-only success. Before evidence used the immutable original methods/arms with the same generated collaborators; the new opaque cancellation ticket was only a stimulus ignored by old code.

These are controller and boundary regressions, not a full LiveKit/GPUI/iOS SDK integration or device test. Full native compilation and normal platform tests remain separate gates. Physical audio/video calibration, capture dimensions/frame pacing and ring timings are unchanged.

Remote cleanup compares authenticated user membership within the relay scope, matching the server `(call_id,user_id)` key. It preserves a same-user replacement across local re-adoption and still cleans up a different user. Missing legacy user metadata preserves conservatively.


The fresh account-session successor expands relay checks8→17 and adds the real
FIFO reservation3 and StoreSlot11 controls. Existing Swift14/API5/MacUI6/engine6/
ownedHTTP4 checks remain. StoreSlot uses actual temporary files and production
path/slug/remove functions with a local connection collaborator. It covers native
case aliases, same-file close/unlink/open exclusion, late/duplicate receipts,
other-file independence and retained known-origin/user refusal. The old16-case
registry-only adapter is historical; its real helpers/Leave arm are superseded by
the current17 relay checks, not shipped as a broken entrypoint. See adjacent
store-owner-gates.json and ../session-owner/STORE-LIFETIME.md for policy limits.
