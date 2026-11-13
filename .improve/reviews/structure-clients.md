# Phase 3: native-client structural audit and redesign

Historical audit snapshot: later implementation, corrected native artifact isolation
and current prerequisites are recorded in [the phase report](../structure.md).
The original disk/fork-approval observations below are not current blanket blockers.

Inspected base: `2865cb29b806121d4a90f55ba6ce151a150279a0`.
Scope inventory: 125 Mac Rust files (~156K lines), 88 iOS application Swift files
(~60K), 85 XCTest files (~11K), 14 UI-test files (~3.7K), generated relay DTOs and
the FFI package manifest. Broad scans covered waits, retries, swallowed errors,
globals, duplicate helpers, cross-layer references, dead-code suppressions and
test quarantine. Focused reads traced chat/device/media/auth/update/search/people/
server/settings/cache flows. Match lists are audit aids, not a claim that every
marker is a defect. No source change preceded this problem list/redesign.

## Fixes grouped above the problems they solve

### A. One pure chat-title policy below UI and transport (small; implement now)

Put the existing unchanged helper in `chat_title.rs`, used by app, relay and
fixture. Pin current behavior on the original function before extraction, then run
the same tests on the shared module. This dependency-free module can be tested
with `rustc --test` without compiling GPUI/WebRTC/Matrix.

- Hidden link: `app.rs:12567` and `sync/fixture.rs:3989` copy the same first-line,
  trim, 34-Unicode-scalar title policy byte for byte. A title-policy change must
  currently edit both, and neither copy advertises the other.
- Needless link: `sync/relay.rs:4995` calls `crate::app::title_from`, importing
  stable text policy from the large, volatile GPUI application owner. Fixture has
  its own copy instead of using the same production policy.
- Required invariants: only first line; empty first line remains `New chat` even
  if later lines contain text; Unicode trim; CRLF; Unicode scalars rather than
  bytes or grapheme clusters; exactly 34 scalars without ellipsis, 35 with one.

### B. Room-owned synchronization state/handles (high; blocked on native gates)

Let a room own its doc handle, subscriptions, pending user ids, transport/session
and cursor-save state. A short-lived map lookup gives callers a room handle;
expensive decode/import/export happens behind that room's own synchronization.
Keep the shared `arbos-client::RoomSession` protocol as the pure core.

- Hidden link: global `unacked: Mutex<HashSet<Id>>` in `relay.rs:218` tracks
  messages from every room, but ACK at `:3913` drains it wholesale. One room's ACK
  changes unrelated room pending state. iOS's `[chat: Set<message>]` owner at
  `AppModel.swift:906` explicitly avoids this cross-room convention.
- Needless link: global Mac `docs` mutex is held across full transcript projection
  (`relay.rs:3014`), imports (`:3837`, `:3876`), snapshot export (`:1420`) and send
  writes (`:4937`). Independent rooms therefore contend over unrelated work.
- Hidden lifetime links: Mac room task, change-forwarding task, subscriptions and
  doc live in separate collections; `ensure_room_with` (`:3183`) constructs them
  by convention, with a separately retained subscription vector at `:223`.
  `Room::drop` (`:9537`) aborts only its stored room task. Do not assert a leak
  from static reading alone; use drop/subscription tests before changing ownership.
- iOS room lifecycle is scattered through maps/sets at `AppModel.swift:902–985`
  and cleanup routines (`wipeRoom:2565`, unavailable/recreate paths). A new state
  field must be initialized/reset in several places, a typical hidden link.
- Behavioral regressions required: ACK isolation across A/B and project docs,
  stale reconnect callback after stop, pending retry original batch id, snapshot
  cursor never ahead of persisted bytes, room deletion drops tasks/subscriptions,
  large room must not block unrelated room send. Full native gates are blocked;
  avoid performing this redesign as an uncompiled broad edit.

### C. Bounded independent work at hydration/upload seams (medium; Phase 6 lead)

Fetch independent project detail/member requests concurrently with bounded fan-out,
then merge according to existing deterministic semantics. Upload independent blobs
concurrently but retain attachment order and per-chat message FIFO.

- Hidden call-order link: `refresh_registry` (`relay.rs:2585`) waits for every
  project's detail/members before requesting people chats; unrelated project
  metadata availability controls people-chat readiness.
- Same concern iOS `startAfterPairing` (`AppModel.swift:1853`): serial registry,
  account, people, servers, Matrix and pushes, plus machine refresh before room
  creation. Some depend on auth/registry state; only proved-independent work moves.
- Sequential attachments: Mac `relay.rs:4900`, iOS `AppModel.swift:3657,5543`.
  Keep failure reporting and display order; do not delete `ChatSendQueue` because
  its ordering prevents photo/text inversion.
- Conversely iOS old-relay directory fallback (`AppModel.swift:3178`) launches 36
  tasks at once. This is a compatibility requirement implemented with unbounded
  concurrency; a bound belongs at the same hydration seam.

### D. Explicit rendering inputs instead of mutable ambient settings (medium)

Supply link-preview presentation policy to the row builder (or a small immutable
render context) and include that policy in cache invalidation. Do not force every
view to know account/network state.

- Hidden link: `Transcript.linkPreviewsEnabled` is `nonisolated(unsafe) static`
  (`UI/Transcript.swift:391`), written by AppModel init and property observers
  (`Store/AppModel.swift:149,998`) and read by row building and chat menus
  (`Transcript.swift:512,716`; `ChatView.swift:1392`). Its dependency is absent
  from the otherwise pure row-builder interface.
- Mac has similar atomic global at `ui/link_card.rs:27`. It is data-race-safe but
  still ambient policy for tests/caches; don't replace it with a more complex
  dependency mechanism without exercising settings changes across open chats.
- `AppModel.current` weak static (`:994`) and `LinkIcons.shared.fetch` assigned by
  model init (`:1030,1039`) tie lookup behavior to which model was initialized
  most recently. Real app uses one model; fixture tests may create several.
  First test concurrent model/settings isolation before calling this a user bug.

### E. One mention policy with deliberately documented differences (medium)

Keep typed member ids/chips authoritative. Extract genuinely common lexical
matching policy, and explicitly model differences between collective addressing,
agent addressing and ordinary people handles. Do not simply pick one regex.

- Hidden duplicate: `text_has_at_token` exists in `sync/mod.rs:1585`,
  `sync/relay.rs:3321` and `sync/fixture.rs:7764`.
- Two are identical; the shared version checks the character *before* `@` and
  disallows a dot after the token, while relay/fixture check only after and allow
  dot. Thus `mail x@agent.com` can match relay's agent helper while not matching
  the shared collective helper. Whether all variants should converge is a
  behavior decision, requiring current-behavior tests and intended bug tests.
- iOS mention/leading-name policy spans `AppModel.swift:3567–3867`; Mac spreads
  membership resolution across `sync/mod.rs` and relay/fixture. The lexical core
  can eventually live in shared Rust with cross-platform fixture tests, but
  mention targeting can wake agents and expose messages, so no silent rewrite.

### F. Keep transport separate from presentation utility placement (low/medium)

Move pure identity/title/expiry helpers to stable owners, and return OS-open intent
to an application adapter rather than making transport import UI implementation.

- `relay.rs:2703` gets stable avatar color from `crate::ui::avatars`, which merely
  reexports core avatar policy (`ui/avatars.rs:32`); direct core dependency avoids
  the needless transport→UI edge.
- `relay.rs:9341` calls `ui::agent_links::expiry_label`, a pure data-label mapping
  among view construction. Move only if refactoring that feature; scope/expiry
  validation and privilege policy must stay pinned.
- `relay.rs:8165` invokes `ui::attachments::open_with_system` after downloading.
  Existing `SyncEvent::AttachmentOpened` already reports results; OS behavior could
  be application-adapter work. Preserve macOS AVPlayer/Quick Look/Linux semantics.
- `model.rs:3545,3598` depends on `sync::user_error::UserError`. This is a semantic
  error type, not inherently a bad dependency, but placement under transport makes
  model stability rely on a layer name. Move a real cohesive error module once,
  not create pass-through aliases to make the graph look prettier.

### G. Remove proven unused surfaces only after callers/tests are checked (low)

- `CallTarget` / `AppState::start_call` (`app.rs:158,8025`) retain old pane-call
  entry points, suppressed as dead code. Current calls begin in chats; remove only
  if exhaustive references/platform cfgs confirm no rig/feature caller remains.
- `notify_approval_requested` (`app.rs:9059`) documents no caller on this branch.
  Approval notifications are user-visible/security-adjacent; prove its intended
  replacement before deleting rather than accidentally removing missing behavior.
- Message-grant UI methods (`app.rs:9796–9840`) are retained for a future way in.
  They are speculative surface, but the backend consent feature itself is real.
  Remove unused picker plumbing only; do not delete authorization enforcement.
- `profiles/pane.rs` broadly allows dead-code variants in its page DSL (`:75`)
  and an unused `name` renderer (`:103`). Several declarative variants may be
  genuinely used. Do not call the entire profile module surplus from a lint marker.

## Comments pass (requested addition)

Redundant examples include `CoreChatDoc.swift:69` “MARK: Reading”, thin wrapper
descriptions that duplicate method names, and inline “insert + commit” narration.
They are low-value cleanup only in touched code. Generated `RelayTypes.swift`
must be changed at its Rust source/generator, never hand-edited for comment style.

Retain the following externally justified comments and constraints:

- `ui/picture_fit.rs:1–24`: GPUI has no mipmaps; Retina backing scale and
  resampling thresholds preserve physical image quality. Its thread-local scale
  input from `shell.rs:5160` is an interface convention worth documenting, not a
  reason to erase calibration.
- `SessionRoom.swift:233`: iOS foreground WebSockets can be half-dead, requiring
  redial. It is an OS behavior workaround, not a decorative sleep.
- Loro cursor/pending/import ordering comments; core and device readers must agree.
- CallKit immediate reporting/fulfillment rules and audio interruption handling.
- Design-rule constants for control sizes, Glass/Matte and quote/reveal geometry.
- UI scrolling correction cadence (`ChatView.swift:2930`) is measured against
  LazyVStack estimated heights and viewport realization; keep bounded retry loop
  and device calibration until an equivalent native test passes.

Do not churn tens of thousands of explanatory comment lines as a separate untested
cleanup. The exit criterion is better locality with preserved behavior.

## Tests pass (requested addition; evaluated after proposed structural fixes)

- **Worth rewriting, not deleting:** Mac `snapshot_coalesce_tests`
  (`relay.rs:9591`) exercises two `#[cfg(test)]` time-model helpers at
  `:3049,3076`. Production `forward_changes` at `:3258` implements scheduling
  separately. Tests remain green if production coalescing disappears. Extract a
  real pure scheduling decision used in production, or drive the real Tokio task
  with a controllable clock and assert emitted snapshots. Keep current tests until
  the replacement behavioral checks are proven to fail for the regression.
- Keep `OptimisticSendTests`, `SessionRoomTests`, `BlankTranscriptGuardTests`,
  `RowStorageTests`, long-transcript tests and Mac send FIFO tests: they exercise
  real outputs/failure paths and address reported regressions.
- Do not delete numeric tests like `ControlMetricsTests` or Glass material tests
  as “implementation mirrors”: many pin explicit owner design rules. Source-pattern
  guard tests should prove their scanner catches a mutant before changing them.
- `ChatSearchRevealLandingUITests.swift:96,702,727` quarantines two short-row arms
  because rig drag travel exceeds the target's aim window. The comment records
  measured 140–471 pt travel and a separate rig issue. Keep the passing long-row
  control; restoring these arms needs a fixed rig and native device verification,
  not removal of the assertions or increased sleeps.
- Optional perf/capture/live-smoke suites intentionally require explicit rig flags;
  a skipped live suite is not a useless test. Never enable it against defaults
  because defaults point to hosted services.

## Exit status

**Native structural exit checks are blocked, not passed.** Important worth-fixing
items B–F remain. Full Rust/Xcode regression execution cannot fit the current
~2.7 GiB free disk, and fork CI is awaiting maintainer approval with no jobs run.
The small title-policy extraction can be proven independently; it does not prove
the rest of the native graph clean, that all behavior stayed unchanged, or that
reported application latency improved. Recommended resume: adequate local scratch
storage and native gates, then room-owned state/ACK isolation before concurrency
and snapshot optimization.

## Implemented extraction and verification

After recording the audit/redesign above, extracted unchanged chat-title policy
into `crates/arbos-mac/src/chat_title.rs` and updated application, fixture and relay
callers. Both prior duplicate bodies matched before extraction; all three callers
now import the same module, and relay no longer imports this helper from AppState.
No title behavior, UI layout, device calibration or protocol changed.

| Check | Before | After |
| --- | --- | --- |
| Original/extracted title behavior tests | 8 passed, 0 failed, 0 ignored; 0.249 s invocation | 8 passed, 0 failed, 0 ignored; 0.210 s invocation |
| Standalone test compilation | original extracted source compiled in 0.664 s | actual shared module compiled in 0.114 s |
| `cargo fmt --all --check` | baseline pass, 3.016 s | pass, 2.607 s |
| `git diff --check` | pass | pass |
| Full native lint/typecheck/tests/build | blocked | blocked; unmeasured |

Commands: `rustc --edition=2021 --test <source> -o <worktree-owned-test-binary>`,
then execute that binary. Before source and JSON logs are in local evidence
`chat_title_before.rs`, `chat-title-before.json`, `chat-title-after.json`.
These subsecond differences are compiler/test-invocation noise, **not a speedup
claim**. The benefit is one policy owner and a smaller dependency graph. Final
disk observation was 1.6 GiB free, so no full native compilation was attempted.
