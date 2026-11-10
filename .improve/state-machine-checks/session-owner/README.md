# Account-owned native work

The relay coordinator owns the current account session. A genuine adoption retires
A before constructing B's fresh `Shared`, store lease, caches, mailboxes and task
owner. Same-account token refresh preserves that session. UI command/event
admission carries the captured account and control tickets; delayed account UI
callbacks require both. The store-file owner serializes connection close/unlink
with a same-path successor. Call registration and finite captured-authority cleanup
remain relay-coordinator owned across session replacement.

Run explicitly on macOS from the repository root with Python3, cached locked Cargo
dependencies and the native Swift/Rust toolchains. No install, app launch, Keychain,
provider, device or existing database access occurs. Processes use synthetic effect
collaborators, fresh build directories and a network-denied execution sandbox.

```sh
python3 .improve/state-machine-checks/session-owner/check_ui.py
python3 .improve/state-machine-checks/session-owner/check_backend.py
python3 .improve/state-machine-checks/session-owner/check_matrix.py
python3 .improve/state-machine-checks/native-ownership/check_swift.py --output /tmp/arbos-swift-checks-1
```

Optional `--record-dir` saves the first three commands' logs/JSON. Their `--tree`
argument chooses the source checkout, with the current repository as the default.
The Swift output directory must be fresh. Each adapter extracts actual production
methods and hashes its source inputs before/after execution. Extraction or count
drift fails the command. Fresh temporary Cargo targets prevent reuse of another
snapshot's workspace binary. Effect collaborators are test fakes, not a second
implementation of the ownership transitions. The separate generated native laws
and their replayable actual-function mutants are documented in
[`../native-ownership/README.md`](../native-ownership/README.md).

Current retained gates: UI21/21 (4.347 s fresh target), backend23/23 (8.484 s fresh
target), Swift Matrix2/2 (.988 s), Swift auth/Matrix/snapshot16/16 (.919 s). These
are independent entrypoints; overlapping controller/helper cases are not a unique
whole-app coverage count. `GATES.json` pins sources and individual timings. The
backend fake call collaborators deliberately leave call cleanup to the separate
actual call-helper/arm suite under `../calls`; these23 tests do not prove media
or call-server behavior.

The backend checks include revoked queued command admission; same-account refresh;
late refresh success/refusal; a401 retry after switch refusing B's bearer; late
pin responses refusing store writes/events; stale SDK start/callback rejection;
current Matrix/pin controls; doc/room/subscription/task retirement; known-user collision refusal; and same-path
replacement waiting at an old Store drop barrier before its marker survives.
The latter barrier uses an in-memory effect ledger. The StoreSlot checker `../calls/check_store_owner.py` supplies independent real temporary filesystem/path-alias coverage.

`history/` preserves overlapping failure-before stages instead of inventing one
baseline total: queued control9P/1F; automated logout11P/1F; store12P/1F; Matrix1P/
1F; UI cache12P/1F, call origin15P/1F, local-device reset16P/1F, delayed rig17P/1F,
and same-control backend replacement18P/2F. Earlier narrow and common-boundary
failure traces remain historical evidence; no original application tests are
removed. Compiler/setup failures are not behavior RED evidence.

These are source-derived controller proofs with local collaborators. Actual SDK
compilation is a separate gate; physical GUI/media/provider behavior is not
simulated here. Cancellation can reject new work and late publication, but cannot
recall an already-issued remote request. Captured cleanup authority is the finite
retirement exception, without admitting new ordinary account work. Unbound legacy
cache-file attribution requires a separately decided migration policy; the new
in-process lease refuses known origin collisions and does not retroactively
attribute bytes from an earlier process.

Parent integration also compiled the actual Mac crate against the pinned public
LiveKit SDK: all-target Clippy with warnings denied passed in 27.412 s. Four actual
Cargo test filters passed 17 cases (RoomDocs7, UiSession4, AccountTasks3,
CallRequests3). `NATIVE-GATES.json` pins the exact 19 source hashes and commands.
This is a compiler/helper runtime gate, with network denied and no GUI, media,
account, HTTP or existing store access. Full native application linking and the
unfiltered iOS suite remain separate final integration gates. The earlier narrow
native build is not counted as a final whole-composition build.
