# Phase 4: state-machine inventory and disposition

Base: `2865cb29b806121d4a90f55ba6ce151a150279a0`. This inventory covers all
backend crates, native/FFI clients, voice, harness, edge/site, image, scripts and
evals. A risk label describes consequences, not whether a rewrite is needed.
Existing typed results, guarded SQL transitions and independent historical facts
are distinguished from an undeclared collection of mutually exclusive modes.
The original broad inventory incorrectly grouped several of those together;
the reassessment below gives the source-level reason for each correction.

**Implementation and bounded review:** the backend, operational and iOS
account/media boundaries below are explicit and exercised by actual-source
regressions and generated laws. Mac compilation/linking and iOS SDK app/test-
source gates are retained separately from runtime tests. The finish report records
the exact combined-source Linux/Swift verdicts and explained red rows. Component
totals overlap; no provider, deployment or physical-device result is claimed.

Native languages keep their existing runtime and use Rust ownership/RAII and
Swift tagged tickets at effect edges. Effect Schema arbitraries drive the
TypeScript law transport over those actual implementations; stable native code
is not rewritten into a second TypeScript runtime.

## Part 1: summary

### Backend and shared Rust

| ID / title | Effective lifecycle / one-sentence description | Risk | 2–5 key files |
| --- | --- | --- | --- |
| H1 Foreground turn / row presentation | TurnKind, ValidRoute and TurnOutcome already declare intent/route/result; remaining row and ACK presentation is narrower. | Medium; authority already explicit | `arbos-agentd/src/agent.rs`, `status.rs`, `handoff.rs`, `reply.rs` (same directory) |
| H2 Durable jobs / worker attempts | Shared guarded SQL declares durable job transitions; independently owned execution attempts are covered by H3/H4. | High authority; already explicit | `arbos-agentd/src/store.rs`, `agent.rs`, `approval.rs`, `isolation.rs`, `job_cancel_window_tests.rs` |
| H3 Main and worker processes — implemented | Main Pi slot and RPC/Claude worker registries own starting/ready/stopping/closed transitions with exact tickets and captured resources. | High | `arbos-agentd/src/rpc_child.rs`, `harness.rs`, `agent.rs`, `workspace.rs` |
| H4 Approval waiting and dispatch receipt — implemented | A pending card can be inline/polled/deferred, decided once, then its persisted single-use grant can be consumed and dispatched. | High | `arbos-agentd/src/approval.rs`, `store.rs`, `arbos-core/src/approval.rs`, `arbos-agentd/src/mcp.rs` |
| H5 Agent room synchronization | Exact inflight batch/VV ownership and guarded ACK/refusal transitions already exist; catchup/backoff presentation remains separate. | Medium; protocol authority already explicit | `arbos-agentd/src/sync.rs`, `store.rs`, `workspace.rs`, `arbos-core/src/wire.rs` |
| H6 Box provisioning/wake/liveness — admission implemented | A per-user box can be missing/creating/waking/awake/stale/stopping/held/failed while provider and local connection evidence arrive independently. | High | `arbos-relay/src/boxes.rs`, `state.rs`, `db.rs`, `db_accounts.rs`, `fleet.rs` |
| H7 Upgrade/repair and retained sandboxes | Typed Outcome/Task/Source and immutable error context already declare the one-shot pipeline and rollback policy. | High effects; already explicit | `arbos-relay/src/upgrade.rs`, `upgrade_carry.rs`, `db_accounts.rs`, `upgrade_rollback_tests.rs`, `upgrade_room_tests.rs` |
| H8 Planned relay restart — implemented | Running/frozen/announced/draining/stopped combines closing signal, work-freeze bit, in-progress creates and socket count. | High | `arbos-relay/src/restart.rs`, `state.rs`, `boxes.rs`, `rooms.rs` |
| H9 Relay sign-in/rotation/revocation | Conditional grant/rotation SQL and checked protocol cases already own authority; cross-service delivery errors are returned. | High authority; already explicit | `arbos-relay/src/signin.rs`, `auth.rs`, `db_accounts.rs`, `matrix/client.rs`, `rooms.rs` |
| H10 Secret/vault approval and access — receipt boundary implemented | Pending/allowed-once/standing/denied/expired/revoked grants combine expiry, source version, pause flags, directors and external vault access. | High | `arbos-relay/src/db_secrets.rs`, `secrets.rs`, `vault.rs`, `vaults.rs`, `shell_grants.rs` |
| H11 Billed usage settlement — implemented; informational meter retained | Allowed/daily-blocked/credit-blocked/read-only and auto/manual kill modes interact with ledger debits and persisted meter cursor. | High | `arbos-relay/src/credits.rs`, `db_accounts.rs`, `proxy.rs`, `db_voice.rs`, `arbos-agentd/src/budget.rs` |
| H12 Sidecar page receipt — implemented in Phase3 | Fetched/mirrored/wake-accepted/checkpointed stages govern whether replay can safely pass an ASP event. | High | `arbos-sidecar/src/daemon.rs`, `mirror.rs`, `waker.rs`, `relay.rs` |
| H13 Relay speculative document cache — implemented | Cold/rebuilt/advanced/push-imported/authorized/refused states decide whether cached Loro state can be reused after a speculative push. | High | `arbos-relay/src/rooms.rs`, `state.rs`, `push_guard.rs`, `db.rs` |
| M1 Room socket delivery | Catchup/live/held/timeout-released modes share a boolean, last seq, held broadcast list and heal counter. | Medium | `arbos-relay/src/rooms.rs`, `room_health.rs`, `state.rs` |
| M2 App session orthogonal substates | Existing explicit Phase coexists with heal, refusal, resend, checkpoint and pending-dependency state; audit the product of these fields rather than replacing Phase. | Medium | `arbos-client/src/room.rs`, `arbos-ffi/src/lib.rs`, `arbos-core/src/wire.rs` |
| M3 CLI room/send | HELLO/checkpoint/catchup/send/ACK/answer/timeout depends on local booleans and nullable pending push/message ID. | Medium | `arbosctl/src/room.rs`, `client.rs`, `arbos-core/src/wire.rs` |
| M4 Matrix outbox | Queued/due/retry/done/parked poison jobs derive from attempts, next_at, done_at and failed_at SQL columns. | Medium | `arbos-relay/src/matrix/db.rs`, `worker.rs`, `mod.rs` |
| M5 Matrix SDK synchronization | Stopped/starting/syncing/retrying state follows watch cancellation and nested build/sync loops with backoff. | Medium | `arbos-matrix/src/client.rs`, `lib.rs`, `arbos-ffi/src/matrix.rs` |
| M6 Matrix timeline paging | Unloaded/loading/cached/at-start state is represented by loaded/at_start flags and nullable pagination token without an in-flight page state. | Medium | `arbos-matrix/src/client.rs`, `rows.rs`, `arbos-ffi/src/matrix.rs` |
| M7 Sidecar wake/coalescing | Idle/coalescing/delivering/waiting-for-agent/trailing-nudge state combines dirty/done flags, pending map, join handle and deadline. | Medium | `arbos-sidecar/src/waker.rs`, `turns.rs`, `daemon.rs` |
| M8 Sidecar circuit breaker | Closed/open/half-open probe state combines open_until, half_open and failure window. | Medium | `arbos-sidecar/src/waker.rs`, `config.rs` |
| M9 Sidecar session rotation | New/resume/rotate/fallback/handoff state depends on persisted session identity, generations, turn budget and rotate_pending. | Medium | `arbos-sidecar/src/session.rs`, `waker.rs`, `config.rs` |
| M10 ACP prompt queue and permissions | Queued/active/session-create/mode-set/permission-wait/complete state spans busy lock, Turn Option, session map and response channels. | Medium | `arbos-acp/src/main.rs`, `arbos-core/src/approval.rs` (shared protocol reference only; ACP has no crate dependency) |
| M11 Local device connection/execution | Pair/connect/hello/serve/reconnect and per-operation task completion are implicit in the websocket loop and spawned requests. | Medium | `arbos-local/src/main.rs`, `config.rs`, `ops.rs`, `arbos-relay/src/local.rs` |
| M12 Signed update download/install | Absent/partial/resuming/downloaded/verified/staged/swapping/restored/relaunch state follows file suffixes and procedural retry/install branches. | Medium | `arbos-update/src/client.rs`, `fetch.rs`, `sign.rs`, `tests/download_retry.rs`, `tests/feed_retry.rs` |
| M13 Connector authorization/token refresh | Missing/needs-auth/connected/expired/refreshing/disconnected state lives in connector status/token/expiry rows and provider control flow. | Medium | `arbos-relay/src/connectors/mod.rs`, `providers.rs`, `crypto.rs`, `db.rs` |
| M14 Routine schedule/run | Disabled/armed/due/firing/running/finished/error state follows enabled/next_run/last_run/run rows and async reporting. | Medium | `arbos-relay/src/routines.rs`, `db.rs`, `arbos-agentd/src/routines.rs` |
| M15 Goal wake lifecycle | Active/blocked/waiting/paused/done is a validated string set, while wake eligibility also depends on budget/time/activity. | Medium | `arbos-relay/src/goals.rs`, `db.rs`, `arbos-agentd/src/goals.rs` |
| M16 Call/participant lifecycle | Starting/active/empty/ended state uses ended_at/reason, participant left_at, webhooks and sweeper timeout; voice usage receipts are covered by H11. | Medium | `arbos-relay/src/calls.rs`, `db_calls.rs`, `livekit.rs`, `voice.rs` |
| M17 People membership/moderation/invites | Invited/joined/left/banned and post-allowed/silenced/expired-silence state spans timestamps, roles and multiple mutation paths. | Medium | `arbos-relay/src/people.rs`, `db_people.rs`, `moderation.rs`, `groups.rs` |
| M18 Matrix server/channel membership | Server/channel creation, invite/join/leave/ban, owner handoff and power-level updates are coordinated through SQL and homeserver outbox. | Medium | `arbos-relay/src/matrix/servers.rs`, `servers_db.rs`, `spaces.rs`, `worker.rs` |
| M19 Site publication | Begin/upload/commit/live/unpublished/taken-down/edge-pending/synced state spans pending manifest, version, status and edge receipt. | Medium | `arbos-relay/src/sites.rs`, `db.rs`, `arbos-core/src/sites.rs` |
| M20 Outside-agent/link sessions | Provisioned/open/connected/expired/rotated/revoked state governs key/link scopes and ASP event cursors. | Medium | `arbos-relay/src/outside.rs`, `agent_link.rs`, `agent_link_stream.rs`, `agent_link_read.rs`, `agent_link_write.rs` |
| M21 Notification delivery/read | Announced/queued/delivered/read/archived and token-valid/invalid states combine cursor marks, per-device queues, focus and APNs outcomes. | Medium | `arbos-relay/src/notify.rs`, `push.rs`, `apns.rs`, `db_push.rs`, `db_people.rs` |
| M22 Fleet admission/pressure/eviction | Capacity/waitlist/pressure-confirmed/evicting/recovered state uses consecutive samples, liveness and turn-running evidence. | Medium | `arbos-relay/src/fleet.rs`, `db_fleet.rs`, `boxes.rs`, `state.rs` |
| M23 Memory-tree card build | Open/closed/stale/card-failed/ready lifecycle follows string status plus heading/card timestamps and rebuild/score cache logic. | Medium | `arbos-agentd/src/tree.rs`, `store.rs`, `memory.rs` |
| M24 Memory validity/compaction | Live/invalid/superseded/expired memories and in-progress/applied compaction are modeled with timestamps, versions, summaries and model-call effects. | Medium | `arbos-agentd/src/memory.rs`, `store.rs`, `tree.rs` |
| M25 Prompt/identity refresh | Current/dirty/debouncing/restarting/refreshed state combines persisted identity, pending atomic, due time and current pi session. | Medium | `arbos-agentd/src/identity.rs`, `agent.rs`, `workspace.rs`, `settings.rs` |
| M26 Group responder/ask routing | Not-member/history/unaddressed/gating/selected/inflight/answered state combines roster/join times, gate cache, mentions and command marks. | Medium | `arbos-agentd/src/people.rs`, `groups.rs`, `responders.rs`, `steer_gate.rs`, `sync.rs` |
| M27 Send-order claim | Joined/waiting/head-claimed/timed-out/released state combines queue order, claimed flag and ticket lifetime. | Medium | `arbos-agentd/src/send_order.rs`, `agent.rs`, `send_order_tests.rs` |
| M28 Feedback capture | Requested/queued/device-captured/box-captured/submitted/expired/deduplicated state is tracked through capture files, push requests and timeout cleanup. | Medium | `arbos-relay/src/feedback.rs`, `arbos-agentd/src/feedback.rs`, `arbos-relay/src/db.rs` |
| L1 FFI subscription delivery | Unsubscribed/subscribed/listening/batching/draining state spans listener Options, pending queues and subscription handles; ownership wrappers remain useful. | Low | `arbos-ffi/src/lib.rs`, `arbos-core/src/doc.rs` |

For paths shortened to filenames in a row, the directory is that row's first path directory unless a different crate is explicit. M10 includes a protocol reference as its second file; the implementation itself is deliberately one independent ACP source file, so there is no second implementation file to invent.

### Native clients and FFI

| Machine | Effective behavior and risk | 2–5 key files |
| --- | --- | --- |
| Account-session ownership | Explicit account/control/task/cache ownership rejects stale publications and actions; iOS callbacks, snapshots and nested media retain their rendered origin. **High** | I/Store/AppModel.swift; I/Sync/Auth.swift; I/Sync/RelayAPI.swift; M/sync/relay.rs; M/sync/credentials.rs |
| iOS transport-attempt lifecycle | Socket/task/generation flags coordinate idle, dialing, active, retrying and permanently stopped hosts around an already explicit room protocol. **High** | I/Sync/SessionRoom.swift; I/Store/AppModel.swift; F/lib.rs |
| Mac pending delivery ownership — implemented in Phase3 | Room-keyed PendingMessages owns enqueue, failure-removal and ACK clearing; another room cannot consume the receipt. **High** | M/pending_messages.rs; M/sync/relay.rs; M/sync/mod.rs; M/app.rs |
| Call-attempt ownership | REST call creation/join, UI cancellation and LiveKit/CallKit completion must belong to one current attempt. **High** | I/Store/CallManager.swift; I/Model/Call.swift; M/app.rs; M/calls.rs; M/ui/callview.rs |
| Matrix client identity lifetime | Starting/available/disconnected/stopped bridge state and callbacks must belong to the current account/client generation. **High** | I/Store/ServerStore.swift; I/Store/AppModel.swift; F/matrix.rs; M/sync/relay.rs |
| Snapshot publication lifetime | A detached doc read may complete after room/account cleanup and must not republish obsolete private messages. **High** | I/Store/AppModel.swift; I/Sync/CoreChatDoc.swift; I/UI/ChatView.swift |
| Attachment-send lifecycle | Picked/uploading/committing/pending/failed attachment sends use transient tasks and multiple collections rather than one send state. **Medium** | I/Store/AppModel.swift; I/UI/ChatView.swift; M/sync/relay.rs; M/app.rs |
| Per-chat FIFO worker lifetime | No-worker/active/queued/timed-out/cleared state is encoded by channel/task entries and timeout callbacks. **Medium** | M/sync/chat_send_queue.rs; M/sync/relay.rs |
| Draft-DM creation | A per-draft/account queue owns creation and FIFO sends; the reproduced wrong-recipient interleaving makes this **High**, now implemented with a retained per-draft owner and FIFO queue. | I/Store/AppModel.swift; M/draft_dm.rs; M/app.rs; M/sync/relay.rs |
| Cache flush lifecycle | Dirty/coalescing/exporting/persisted/evicted state spans doc/cursor snapshots, dirty sets and timers. **Medium** | I/Store/AppModel.swift; I/Store/DeviceStore.swift; M/sync/relay.rs; M/sync/store.rs |
| Read-mark coalescing | Unsent/in-flight/throttled/latest-queued read cursor is tracked through maps and per-chat tasks. **Medium** | I/Store/AppModel.swift; I/Sync/SessionRoom.swift; M/sync/relay.rs; M/sync/dto.rs |
| Composer operation mode | Plain send, same-chat reply, cross-chat reply and edit are mutually exclusive semantic modes stored as independent nullable fields. **Medium** | I/UI/ChatView.swift; I/Store/AppModel.swift; M/app.rs; M/ui/composer.rs |
| Navigation surface ownership | Selected project/tab/channel, profile stack, sheets and pending reveal/open intents are coordinated across optional fields. **Medium** | I/Store/AppModel.swift; I/App/ArbosApp.swift; M/app.rs; M/ui/shell.rs |
| Transcript scroll/reveal | Pinned/away/search-reveal/correcting/jump-back modes use flags, optional target and measured geometry. **Medium** | I/UI/ChatView.swift; M/ui/transcript.rs; I/UI/Transcript.swift |
| Voice-memo gesture/capture | Press, permission wait, toggle/hold recording, cancel and late release synchronize recorder state with gesture flags. **Medium** | I/UI/ChatView.swift; I/UI/Recorder.swift; M/ui/voice.rs |
| Speech playback/request ownership | Requested STT/TTS, cached/playback/silent/error states must remain associated with chat/message/request ids. **Medium** | M/ui/voice.rs; M/app.rs; M/sync/relay.rs; I/UI/VoicePickerView.swift |
| First-login provisioning coordinator | Known registry/one create/greeting/finish progress is coordinated around an existing Stage enum with optional ids and booleans. **Medium** | I/Store/FirstLogin.swift; I/Store/AppModel.swift; I/UI/FirstLoginView.swift; M/onboarding.rs |
| Channel pagination/send echo | Unopened/loading/live/at-start pages and optimistic sends reconcile through sets and callbacks. **Medium** | I/Store/ServerStore.swift; I/Store/AppModel.swift; M/app.rs; M/sync/relay.rs |
| Avatar upload/generate/save | Draft picture, active generation/upload, errors and saved identity version must not overwrite a newer selection. **Medium** | I/UI/AvatarPickerSheet.swift; I/Store/AppModel.swift; M/ui/avatar_picker.rs; M/sync/avatar_master.rs; M/sync/agent_avatar.rs |
| Secret/approval UI request | Selected request, pending action and error presentation mirror server-owned consent state without becoming its authority. **Medium** | I/Store/SecretsStore.swift; I/Model/Secrets.swift; M/ui/secret_card.rs; M/model.rs; M/sync/relay.rs |
| Notification registration/dedup | Device token registration, recent local/APNs banners, unread cursors and deferred opens coordinate asynchronous events. **Medium** | I/Store/PushManager.swift; I/Store/AppModel.swift; M/ui/notify.rs; M/app.rs |
| Thumbnail/sample fetch | Missing/requested/decoded/failed and wanted-versus-playing are represented by multiple caches/sets/options. **Low** | M/app.rs; M/sync/relay.rs; I/UI/LinkPreviewCard.swift; I/UI/VoicePickerView.swift |
| Search query/result lifetime | Query, pending generation/results and selected/revealed hit must stay aligned across async changes. **Low** | I/Store/SearchStore.swift; I/UI/GlobalSearchView.swift; M/search.rs; M/app.rs; M/ui/chat_search.rs |
| FFI callback subscription/drain | Queued doc changes/local updates, registered listeners and doc-lock release coordinate callback delivery without reentrant deadlock. **Medium** | F/lib.rs; I/Sync/CoreChatDoc.swift; I/Sync/SessionRoom.swift |

The above are meaningful cross-field lifecycles; cosmetic disclosure booleans,
theme choices and simple availability toggles are deliberately not inventoried as
state machines.

#### Already explicit — do not duplicate

- `arbos-client::RoomSession` protocol phases/actions are already the shared
  sans-IO state machine. FFI `RoomSession` is a mutex adapter; its boolean query
  methods do not make the core protocol implicit.
- Mac `sign_in::Flow` and auth events model OAuth UI transitions explicitly.
  Account-session lifetime around credential publication remains a separate issue.
- Mac `UpdatePhase` (`M/update.rs:164`) has payload-bearing Available/Downloading/
  Installing/Installed/Restarting/Failed cases; keep it and signed verification.
  App pending-relaunch coordination has existing dedicated methods/tests; no
  demonstrated illegal state justifies declaring updater redesign necessary.
- iOS `ChatSession.TranscriptLead` (`I/UI/ChatView.swift:373`) is an explicit pure
  projection of loading state, not an implicit machine to rewrite.
- iOS call Phase, FirstLogin Stage, Mac CallStatus/Voice CallPhase and wire
  approval/job enums already name their modes. Only the separately stored attempt,
  identity or required-payload coordination is listed above. An enum with display
  labels is not by itself proof its surrounding async lifecycle is sound.

### Harness, voice and operational tooling

| Machine | One-sentence description | Risk | Key files (2–5) |
|---|---|---|---|
| AUX-M6 Main-agent routing and correction | Route-first admission, stamped-steer continuation, relay replies and post-hoc correction emerge from five closure variables. | Medium | `harness/arbos-pi-ext/index.ts`; `harness/arbos-pi-ext/route-schema.ts`; `harness/arbos-pi-ext/index.test.ts`; `crates/arbos-agentd/src/agent.rs` |
| AUX-H2 Shell approval polling and parking — implemented | Validated checking/pending/parking/terminal transitions distinguish configured failure from standalone mode and fail closed. | High | `harness/arbos-pi-ext/approval.ts`; `harness/arbos-pi-ext/index.ts`; `harness/arbos-pi-ext/approval.test.ts`; `crates/arbos-agentd/src/hooks.rs` |
| AUX-H3 Voice call lifetime, charged work and shutdown — implemented | Startup, running, finishing, cleanup-failed and closed phases own tasks and retry only failed local cleanup while preserving one best-effort final usage report. | High | `voice/arbos_voice/call_state.py`; `voice/arbos_voice/worker.py`; `voice/tests/test_worker_lifecycle.py`; `voice/tests/test_usage_loop.py` |
| AUX-H4 Voice audio mode and mute application — implemented | Separate application lanes distinguish unapplied, applying, applied and failed effects; mode commits its participant identity atomically. | High | `voice/arbos_voice/call_state.py`; `voice/arbos_voice/worker.py`; `voice/arbos_voice/listener.py`; `voice/tests/test_worker_lifecycle.py` |
| AUX-H5 Vision consent, tracks and frame delivery — implemented | Enabled/disabled/closed generations and owned stream/session leases reject revoked frames and retain cleanup ownership. | High | `voice/arbos_voice/vision_state.py`; `voice/arbos_voice/vision.py`; `voice/tests/test_vision_consent.py`; `voice/tests/state_bridge.py` |
| AUX-H6 Feedback commit admission and delivery — implemented | Shared typed admission gates every normal/prepared merge; delivery phase and prerequisite evidence persist together. | High | `scripts/feedback/feedback_state.py`; `scripts/feedback/feedback_poller.py`; `scripts/feedback/test_feedback_state.py`; `harness/arbos-pi-ext/feedback-state-laws.test.ts` |
| AUX-H7 Deployment lanes and image rollout — implemented | Queue files, canary/restart markers, holds and per-box outcomes determine whether a release starts, pauses, retries or finishes. | High | `scripts/deploy/arbos-deploy-worker`; `scripts/deploy/arbos-deploy-hook`; `scripts/feedback/deploy-relay.sh`; `docs/process-rules.md` |
| AUX-H8 Release-object publication — implemented | Tagged observed/creating/uploading/finalizing/completed/refused/failed states preserve draft intent and require actual publication before success. Stateless merge reports and existing atomic receipts are excluded. | High | `scripts/release/publication.py`; `scripts/release/publish-release.sh`; `scripts/release/test_publication.py`; `harness/arbos-pi-ext/publication-state-laws.test.ts` |
| AUX-H9 Shared Mac heavy-slot ownership — implemented | Explicit request/lease/GUI/boot states and an OS-backed permanent mutex serialize grants, reclaim and cancellation without changing calibrated scheduling policy. | High | `scripts/runner/arbos-heavy-slot.sh`; `scripts/runner/xcrun-slot-guard.sh`; `scripts/runner/gui-lock.sh`; `scripts/runner/tests/test_slot_lifecycle.py` |
| AUX-M1 Voice inbox dedupe and pause scheduling | Seen IDs, pending speech tasks and quiet-window deadlines decide whether a chat update is contextualized and spoken once. | Medium | `voice/arbos_voice/updates.py`; `voice/arbos_voice/worker.py`; `voice/tests/test_updates.py` |
| AUX-M2 Listener/event-batcher worker lifetime | Nullable tasks, active/closed flags and pending child sets imply start, stop and restart conventions. | Medium | `voice/arbos_voice/listener.py`; `voice/arbos_voice/relay.py`; `voice/arbos_voice/vision.py` |
| AUX-M3 Build/test watchdog verdict | A subprocess can still run after test completion, so PID liveness, verdict presence and timeout jointly determine the gate result. | Medium | `scripts/feedback/ios-gate.sh`; `scripts/feedback/rust-gate.sh`; `scripts/ci/xctestrun-env.py` |
| AUX-M4 End-to-end eval turn observer | Stream output, replicated rows, approval rejection, stall repair and timeout determine whether the observer declares a turn complete. | Medium | `evals/long-tasks/lib/driver.py`; `evals/long-tasks/lib/relay.py`; `evals/long-tasks/run.py` |
| AUX-M5 A/B eval resource and budget lifetime | Arm selection, temporary model/flag mutations, remaining budget and restoration are coordinated by locals and a finally block. | Medium | `evals/convergence/ab.py`; `evals/long-tasks/run.py`; `evals/long-tasks/lib/relay.py` |

Image deny hooks and the edge site router are stateless decisions, not meaningful implicit lifecycle machines. Simple SERVE/configuration booleans, immutable judge decisions and task lists are excluded. GitHub Actions/Docker declarative ordering is already declared externally; the custom observation/controllers above are the relevant implicit machines.

## Part 2: higher-risk transitions, evidence and retained scope

Locations below name symbols rather than stale line offsets. Public regression
artifacts are under `.improve/state-machine-checks/`; the native tests live beside
the actual implementation. “Before” means actual original-source execution unless
explicitly identified as an introduced candidate defect. No row claims exhaustive
coverage of every possible schedule.

### Backend authority and resource boundaries

**H1/H2 — turn intent and durable jobs.** `TurnKind`, `ValidRoute`, `TurnOutcome`
already name foreground modes. `route_failed` records exhausting route attempts
and legitimately coexists with the fallback Reply. Routine/voice origin, posted
row IDs and ACK history are independent facts. `Store::set_job_state` rejects
cancelled→running, permits intentional queued/running reclaim, clears ended_at
for the new attempt and preserves the first terminal timestamp. Existing tests
pin these contracts. A blanket terminal-absorption law would break intentional
requeue. Keep the narrower row/ACK presentation Medium; worker identity and
approval authority are the separate High boundaries below. RAII semaphore
permits and cancellation rechecks remain; no second DTO enum was added.

**H3 — main Pi, worker registries and child metadata.** Main `PiSlot` has explicit
boot/ready/closed ownership. A `MainPiBoot` captures each child before a
cancellable wait; only its current ticket can publish, record metadata or clear
readiness. `CurrentPi` pairs the actual child and boot receipt. Model/effort/chat
operations capture that pair, stage desired values, and publish under the same
accepted owner; old recovery cannot kill its successor. Original main-child
checks had5 semantic failures/1 control; the expanded30 selected checks passed.
The later metadata baseline had7 failures/7 controls, plus a separately pinned
thinking-tail preservation defect; final17 metadata checks passed.

`worker_lifetime::State<T>` is Accepting with per-session Starting/Ready/Stopping,
or terminal Closed. Its single transition boundary owns Begin/Capture/Publish/
Abort/Stop/Stopped/Shutdown; opaque monotonic tickets prevent a stale stop waiter
from acquiring a resumed worker. RPC and Claude adapters use this actual state,
not a copied reference model. Captured-resource guards survive cancellation;
Store/session publication shares exact-ticket admission and never awaits a child
under the registry lock. Ordinary stop remains reusable; shutdown closes first.
Own Stopping or a contended registry reports unknown liveness; an independent
Ready sibling honestly remains readable. Already dispatched external work cannot
be recalled; reaping after runtime teardown remains best effort.

Original RPC10 cases had8 semantic failures/2 controls;2 additional stop-waiter
failures were pinned before the final refinement. Original Claude5 had4 semantic
failures/1 control. Review caught an **introduced** stdin-first cleanup deadlock:
both blocked-writer cases pass upstream, fail the held candidate, and pass the
final kill-first adapter. No new grace/duration, CLI argument, model policy or
process isolation was introduced. The latest independent Root checkpoint passes
78 selected native tests plus all-target Clippy; its overlapping totals and exact
source are in `worker-lifetime/CLAUDE-ROOT-COMPOSITION.json`. See `rpc-owner/`,
`rpc-metadata/` and `worker-lifetime/`.

A separate actual RPC defect was then reproduced: an outstanding large Prompt
holds stdin while a child stops reading; shutdown waited for that mutex before
starting its grace. The existing five-second grace now covers the whole EOF path.
The exact child handle is captured outside the timed future; timeout kills/reaps
it before waiting on stdin. Original stop/shutdown regressions fail with the
normal-EOF control passing; final four cases pass, including cancellation after
capture. Related harness33 includes those four; all-target Clippy/check and package
build pass. The Linux sweep fixture uses a test-only exact-child barrier instead
of an invalid400ms/obsolete-EOF premise, retaining all four physical-writer cases
and their existing writer budget. See `rpc-cleanup/`; fresh Linux replay is
recorded separately. RPC graceful EOF and Claude immediate-kill policies remain
distinct. Protocol/request deadlines and process isolation are unchanged.

**H4 — approval delivery.** Durable ApprovalState/Outcome and first-decision CAS
already existed. The repaired submachine is `WaitDelivery::Inline(sender) |
Polled | Deferred` plus `transition_delivery` and one transactional decision/
claim edge before grant-bearing dispatch. Inline-without-sender and an abandoned
approved authority are no longer independent flags. Failed or uncertain delivery
spends the claim; it never restores authority automatically. Pending cards still
have no invented deadline; late approval does not reopen cancelled jobs.
Original5 semantic failures cover SQL ABORT/IGNORE, polling before claim,
competing claim+inline and receiver loss. The rejected-notification race was
separately pinned during review. Final40 native cases passed, including6 park
controls; see `approval-delivery/`. Generated laws of the actual transition and
Store methods are a remaining Phase8 task at this checkpoint.

**H5 — agent room protocol.** Existing `Option<(batch,VV)>` owns exactly one
outbound batch. ACK/refusal requires that batch, refusal preserves acknowledged
VV, retirement clears inflight, and pending dependencies cap the durable cursor.
Current-session caught_up, historical ever_caught_up and one-time recovered are
intentionally independent. A retained closed host is rejected by registry/read
paths and retires its task; historical caught_up does not itself make it illegal.
Phase3 fixes the real cursor/snapshot receipt boundaries. Keep link/catchup/
backoff presentation Medium rather than add an unused parallel reducer.

**H6 — same-user creation admission.** Wire BoxState already declares box state;
provider status, sockets and last-seen timestamps are independent observations.
The reproduced gap was two ensure callers reading missing state, writing two
placeholders and issuing two provider POSTs, while one cancelled guard removed
another's restart marker. `CreationPermit` now owns one per-user async lease
before the existence read and throughout the existing provider pipeline.
`CreatingGuard` mutably borrows that permit; cleanup precedes lease release, and
a waiting caller owns no marker. Both ensure and staged replacement use the
admitted builder, deriving user identity from the permit. The short weak-slot
registry is never held over an await; different users still progress independently.
Original5 cases had2 semantic failures/3 controls; final13 box-row tests pass.
No distributed exactly-once provider or automatic retry guarantee is claimed.
Uncertain cancellation retains the existing durable create intent and placeholder
for reconciliation. Constants/hold/live-row/staged policy remain. See
`create-admission/`; generated real-future admission traces are in progress.

**H7 — upgrade/repair.** `Outcome`, `Task`, `Source`, successful Packed input and
structured carry/rollback already define this pipeline. Progress strings do not
dispatch control. `Staged { keep, swapped }` is immutable error context: unpack
failure has keep=holds_data/swapped=false; swap failure has true/true. Rollback
retains if keep OR swapped and restores the source row before cleanup. The
original “swapped but disposable” finding was a false positive. Keep actual
fake-provider rollback/row-identity tests, independent retained-history facts and
existing fail-safe cleanup; do not invent a second lifecycle or promise a remote
transaction across crashes. Operational deployment authority is AUX-H7 below.

**H8 — planned restart.** The used `Planned` owner moves Serving→Freezing→Notifying
→Draining→Ready, with terminal Shutdown. Drop thaws only its own cancelled plan;
a later plan cannot be cleared by an earlier owner. Creation admission and socket
counts remain separate observations. Original cancellation/overlap failures were
pinned; final10 native tests,4×256 actual-reducer laws and mutant/replay pass.
The90s reset and all notification/drain timing remain. See `RESTART.md`.

**H9 — relay auth.** Unexpired/unused auth grants are conditionally consumed before
token issue; rotation CAS is checked. A serialized lookup/row/claim/issue/grace
path already defines refresh replay and restart behavior. Rotated and revoked
history may legitimately coexist. Device revocation attempts local cutoff and
both relay/Matrix halves, returning a typed combined error on failure. There is
no durable cross-service outbox/transaction: that delivery/retry concern remains
explicit. Do not silently change grace, token family or revocation policy based
on DTO flags. Native clients' stale account authority is the separate repaired
High boundary below; this classification does not establish universal auth safety.

**H10 — one-use secret/vault authority.** Exact observed-grant claim and a
non-Clone receipt now own each disclosure/prepared external attempt. The shared
Immediate transaction deduplicates and claims every unique row in a batch;
changed status/scope/expiry, SQL ABORT/IGNORE or any lost row rolls back the set.
Preparation/decryption/validation precedes claim, while no transaction spans
external I/O. After claim, uncertain outcome consumes authority; no automatic
restoration/retry is added. Original6 handler cases had3 semantic failures/3
controls; builder/batch refinements have separately labelled introduced failures.
Final42 selected native cases and4×256 exact-SQL laws plus mutant/replay pass.
Existing project/target/host/use-kind/asker selection remains; the row's stored
chat is compared but no new operation-chat restriction is invented. This
intentional at-most-one attempt tradeoff requires owner review before merging.
See `authorization-receipts.md` and `ONCE_CLAIM.md`.

**H11 — billed voice/vision settlement.** One Immediate transaction owns cumulative
observation, exact existing ordered debit/clamp/refusal policy, legacy account
initialization and cursor advance. Transaction-private preparation can only yield
a committed `VoiceSettlement`/`PostfactDebit`; required account-cap writes verify
one affected row. Original13 cases had5 semantic failures/8 controls. Root review
caught two ignored-row metadata faults in the candidate; final41 scoped native
checks pass, including finite actual-helper traces. See `voice-settlement/`.
Generated-law coverage is being audited separately; finite cases are not labelled
property proofs. Existing kill-mode constructors are already explicit.
Informational machine time is a separate Medium durability issue: its ledger kind
is excluded from balances/spending caps and its daily alert is log-only. Existing
credits-read/kill-decode/aggregate failure defaults favor availability and remain
unchanged; fail-open versus fail-closed policy is owner-visible, not silently
rewritten as part of the state classification.

**H12 — sidecar receipt (Phase3).** Actual `apply_page` sequences mirror→accepted
wake handoff→durable cursor save→installed cursor. Event/mirror/wake/save failure
cannot advance the receipt; replay recovers wakes from already mirrored but
uncommitted rows.15 daemon +4 mirror cursor/wake checks pass. Accepted channel
delivery can repeat across a crash; this is not a persistent outbox or exactly-once
claim. No duplicate Phase4 model is needed.

**H13 — speculative room cache.** `PendingRoomPush` owns a private moved Loro cache
and exact per-room admission through durable append. Refusal/drop cannot publish;
only accepted nonduplicate commit installs bookkeeping. Cold readers reconcile
the durable head and registry publication uses exact identity. Original rate-limit
bypass, in-flight and concurrent failures are retained;13 native tests include32
finite real Loro/SQLite traces. See `room-cache-ownership.md` and CACHE_VERIFICATION.
The Phase4 standalone aggregate cost is replaced by Phase7's indexed room position
in the final composition; no standalone app speedup is claimed.

### Native clients and FFI

**Account/control/task/cache ownership (High).** `WorkLifetime`/account tickets,
`UiSession`/control tickets and TaskOwner open/closed state explicitly bind work
to account+relay+generation. A genuine adoption creates fresh Mac Shared state;
same-account refresh retains queues. Commands/events/connect/rig work carry their
admitted control and account; Matrix owns both client and generation. UI models
and private caches reset on genuine account replacement while calibration and
same-account progress remain. HTTP refresh captures A authority before await,
rejects stale publication/401, and cannot retry with B's bearer. CallRegistry
metadata is keyed by call+user and contains no bearer; only captured known-user
cleanup may leave a call. Existing unknown legacy provenance is conservatively
retained, not attributed to a guessed owner.

StoreOwner serializes close/unlink/open for the canonical account database path
with Empty/Open/Retiring states and an exact weak-slot receipt, retaining original
I/O paths and allowing independent paths to progress. This closes the live
retirement/successor-open race; it does **not** solve unknown persistent legacy
cache provenance. A new hash namespace and persisted owner-binding migration
remain owner work before treating old unbound files as authenticated data.

Actual Swift/Rust auth, UI, task, Matrix, call and cache tests accompany the
implementation;7 Effect Schema laws run1792 generated cases and8 actual-function
mutants falsify/replay. SDK/native controller checks have different scope; see
`native-ownership/`, `session-owner/`, and `session-owner/NATIVE-GATES.json`.

**iOS async/callback origin and draft queue (High).** AccountWork captures one
owner plus bound API before optimism, Task creation and awaited chains; accepted-
owner guards protect effects, late UI/error/busy publication and return values.
Secrets/devices/server/profile and onboarding/people/creation/picker callbacks
carry the same origin. Account-ticket view identity retires private @State across
atomic A→B adoption without resetting same-account refresh. DraftSendQueue owns
per-draft/account FIFO creation, callbacks and close/retire transitions, preventing
a B draft from flushing to A's newly created DM.

Chat, card, microphone, player, image/cache and clipboard callers preserve the
rendered context. MediaOrigin carries the existing opaque ticket through viewer/
Library copy, save, page fetch, permission and publication; it creates no epoch or
API per row. Supplied stale/unavailable origins cannot fall back to B. Anonymous
profile pictures keep zero-bearer read/Close/zoom behavior. An already issued HTTP
or Photos effect cannot be recalled; later admission, UI and clipboard publication
are fenced. The shared private link-preview blob caller is covered by the final
successor proof rather than relying only on SwiftUI task cancellation.

Source-derived before-failing regressions, positive controls and actual-function
mutants are retained in `ios-account-work/`, `ios-chat/`, `ios-media/` and `ios-chat-resources/`; Schema-derived queue laws execute actual Swift. The selected
method gates do not render SwiftUI or exercise devices. Full SDK source compilation
and canonical XCTest execution are distinct final layers, recorded in the finish
report. Persistent unknown legacy cache ownership remains owner work.

Final scoped iOS method/caller gates pass296 account-work cases (38groups),122
chat cases (12groups), and80 media/preview cases. The suites overlap and use
synthetic effects. Media original copy/save failures and preview8load/6tap controls
are distinguished from new receipt predicates and constructor propagation.
Three actual media-function mutations are detected and replayed. All10 actual
MediaFetch and both attachment-copy producers supply an origin predicate. Four
shared signatures require that argument; five compiler controls show explicit
calls compile and omitted predicates refuse. Runtime bodies stay unchanged. The
bounded constructor review includes all3 link-card paths. These checks do not
claim every UI callback or every possible interleaving is proven.

**Transport and snapshot lifetime (High).** SessionRoom terminal403/404 retires
generation before cancelling tasks/socket and emits terminal failure once. The
old started&&stopped combination remains legitimate for a permanently stopped
started object. Original3 controls/2 semantic failures become5 passes, with
actual XCTest added. Snapshot admission requires current account, exact live doc
and revision; detached completion cannot repaint obsolete private messages.
Same-live-doc empty-paint fallback remains intentional.

**Call/Matrix ownership (High).** Request-order admission and exact attempt/client
tickets prevent stale joins, leave cleanup, events or Matrix failure from changing
a replacement. Explicit logout can use its captured A cleanup authority once;
auto401 does not fabricate a logout. Same-account Matrix client replacement still
invalidates old work. Already issued server effects cannot be recalled. Detailed
scoped native/Swift controller gates remain separate from physical CallKit/
LiveKit/audio/device calibration, which was not exercised or changed.

### Harness, voice and operations

**AUX-H2 — shell approval.** Tagged checking/pending/parking/terminal states and
strict Effect Schema wire decoding fail closed for configured hook/network/
missing-decision errors. Unconfigured standalone mode remains explicit.
Actual24 controls+3 baseline failures become37 passes; validation does not expose
raw command input. The lazy pinned Effect runtime and packaging failure checks
are retained; first configured check adds measured import/validation latency.
See README/GATES and the harness approval tests.

**AUX-H3–H5 — voice lifetime, audio and vision consent.** Call phases own charged
work and finishing/failed-cleanup/closed state. Separate audio application lanes
commit participant+mode only after successful effects. Vision generations and
owned frame/stream/session leases reject revoked or retired work. Failed local
cleanup can be retried without reopening external work or duplicating best-effort
usage reporting. Original SDK/controller failures precede each fix;84 actual-SDK
checks,6×256 laws and4 actual-function mutant/replays pass. Existing VAD, prompts,
usage policy, volume and physical timing/calibration remain unchanged. No live
room/provider/device was used.

**AUX-H6 — feedback admission/delivery.** Shared typed admission validates actual
normal and prepared merge paths; required SHA ancestry/gate evidence and delivery
phase persist together. It cannot claim delivery before prerequisite evidence.
Original6 controls/14 semantic failures become20 passes;5×256 laws and a real
mutant/replay pass.15 existing policy helper bodies are byte-equal. See
`feedback-admission.md`; no real merge or deployment ran.

**AUX-H7 — deployment publication/activation.** Used queue/lease transitions bind
kind+revision+queued/installed/default evidence. Revision advances before accepted
publish/ack/activation mutation; automatic callers require their observed receipt.
Ancestry checks, builds and box rollout stay outside the short queue lock.
Activation keeps its lease through smoke, config/default, non200 restart/grace/
health and public marker. Core22 +publication19 +activation5 actual adapter groups
pass, with generated transition laws and actual mutants. Manual two-argument
operator policy remains; there is no remote atomic/crash-recovery guarantee.
See `deploy-lanes/`, `deploy-publication/`, `deploy-activation/`. Tests use fake
commands/endpoints and never invoke a service, read operator env or deploy.

**AUX-H8 — release publication.** Tagged observed/creating/uploading/finalizing/
completed/refused/failed transitions preserve draft intent and require actual
publication before success. Bounded64KiB/60s child handling and TERM reap are
owned;404 is Missing, draft NoPromote and stable Refused.21 native checks,
3×256 laws and a mutant/replay pass. Stateless merge reports/atomic receipts do
not need another state model. See `release-publication.md`.

**AUX-H9 — Mac heavy-slot ownership.** Explicit request/lease/GUI/boot transitions
and a permanent OS-backed mutex serialize grants, reclaim and cancellation.
Hidden staging leases prevent mkdir/PID-publication and check-then-move races;
cleanup requires the observed receipt.34 native checks,4×256 laws and2 mutants
pass.10 scheduling policy helpers remain byte-equal. See `runner-leases.md`.

### Medium/Low disposition and verification limits

The Part1 Medium/Low inventory is retained for the owner. Highest-value seams are
Matrix pagination's in-flight token, circuit-breaker single probe, outbox poison/
terminal receipts, room socket catchup/held-delivery order and informational
machine-meter durability. Independent read marks, historical join/leave facts,
cosmetic toggles, immutable DTO fields and already explicit RoomSession/UpdatePhase/
FirstLogin Stage/CallPhase are not duplicate machines to rebuild merely because
they contain booleans or Options. Navigation/composer/search behavior retains its
existing layout and physical calibration; account-origin guards are shared with
the High boundary rather than duplicated per presentation mode.

Verification is layered: finite before-failing native/SQL/Swift controller tests;
Schema-derived laws against actual Rust/Swift/Python boundaries with bounded
transports; actual-function mutants with replay; then whole-source compiler and
unfiltered isolated CI gates with their distinct verdicts. Component totals overlap. The successful safe
baseline CI (2373 Rust passes and606 Swift/UI passes) precedes these new sources;
it is not a final combined-source verdict. Final Linux/Swift source pins, outcomes and explained red rows are recorded
in the finish report. The existing search-reveal UI rig is red on two independent phase heads
and overlaps upstream PR2539; it is not reported as green or as a proven app bug.
Bend2 is absent and remains an optional owner follow-up. Broader observability is
explicitly deferred for the owner's discussion with Jacob (const); it does not
block ownership implementation or verification.
