# Phase 4: implicit state machines

**Status: implementation and catalog reassessment in progress.** The
whole-repository inventory separates real cross-field lifecycles from trivial
booleans and already explicit reducers/enums. Static representability is not a
runtime reproduction; details label confirmed cases, interleavings requiring a
test, and invariants already enforced by existing guards.

The approval-hook lifecycle and voice call lifetime, audio application and vision
consent controllers (AUX-H2–H5) now use explicit tagged states and pure
transitions, with effects at their edges. The guarded SDK suite passes 84 tests;
six generated voice laws exercise 1,536 cases separately from the existing 256
approval cases. Remaining High native and operational migrations still require
their stated regression gates. Full native CI awaits maintainer approval and
local build disk is inadequate. Pure state laws do not verify native callbacks,
live SDK behavior, billing or deployment orchestration.

Base: `2865cb29b806121d4a90f55ba6ce151a150279a0`. Coverage includes all backend
crates, GPUI/Swift/FFI, harness, voice, edge/site, image, scripts and evals. Related
cross-layer lifecycles may have separate catalog entries for their actual owners;
the entry count is not a count of independent mathematical machines.

## Part 1: summary

### Backend and shared Rust

| ID / title | Effective lifecycle / one-sentence description | Risk | 2–5 key files |
| --- | --- | --- | --- |
| H1 Agent foreground turn | Waiting/route/ack/result/tool/waiting-for-approval/preempted/failed/finished modes emerge from TurnState flags, optional fields and lane locks. | High | `arbos-agentd/src/agent.rs`, `status.rs`, `handoff.rs`, `reply.rs` (same directory) |
| H2 Durable jobs and worker attempts | Queue/run/park/retry/cancel/interrupted/complete transitions coordinate SQL rows, worker maps, permits and process termination. | High | `arbos-agentd/src/store.rs`, `agent.rs`, `approval.rs`, `isolation.rs`, `job_cancel_window_tests.rs` |
| H3 RPC process and supervisor | Missing/spawning/ready/parked/exited/restarting/shutting-down state combines child/stdin Options, readiness/exited atomics and retained-worker maps. | High | `arbos-agentd/src/rpc_child.rs`, `harness.rs`, `agent.rs`, `workspace.rs` |
| H4 Approval waiting and dispatch receipt | A pending card can be inline/polled/deferred, decided once, then its persisted single-use grant can be consumed and dispatched. | High | `arbos-agentd/src/approval.rs`, `store.rs`, `arbos-core/src/approval.rs`, `arbos-agentd/src/mcp.rs` |
| H5 Agent room synchronization | Cold/open/catchup/live/refused/oversize-parked/closed modes interact with pending dependency cursors, inflight batches and persisted snapshots. | High | `arbos-agentd/src/sync.rs`, `store.rs`, `workspace.rs`, `arbos-core/src/wire.rs` |
| H6 Box provisioning/wake/liveness | A per-user box can be missing/creating/waking/awake/stale/stopping/held/failed while provider and local connection evidence arrive independently. | High | `arbos-relay/src/boxes.rs`, `state.rs`, `db.rs`, `db_accounts.rs`, `fleet.rs` |
| H7 Upgrade/repair and retained sandboxes | Measuring/carrying/staged/swapped/verifying/rollback/retained/deleted states span current row, staging booleans, provider effects and history. | High | `arbos-relay/src/upgrade.rs`, `upgrade_carry.rs`, `db_accounts.rs`, `upgrade_rollback_tests.rs`, `upgrade_room_tests.rs` |
| H8 Planned relay restart | Running/frozen/announced/draining/stopped combines closing signal, work-freeze bit, in-progress creates and socket count. | High | `arbos-relay/src/restart.rs`, `state.rs`, `boxes.rs`, `rooms.rs` |
| H9 Sign-in, refresh rotation and device revocation | One-time OAuth grant, live/rotated/revoked/expired refresh state and session shutdown are coordinated across SQL, Matrix and live sockets. | High | `arbos-relay/src/signin.rs`, `auth.rs`, `db_accounts.rs`, `matrix/client.rs`, `rooms.rs` |
| H10 Secret/vault approval and access | Pending/allowed-once/standing/denied/expired/revoked grants combine expiry, source version, pause flags, directors and external vault access. | High | `arbos-relay/src/db_secrets.rs`, `secrets.rs`, `vault.rs`, `vaults.rs`, `shell_grants.rs` |
| H11 Spend admission, kill switch and metering | Allowed/daily-blocked/credit-blocked/read-only and auto/manual kill modes interact with ledger debits and persisted meter cursor. | High | `arbos-relay/src/credits.rs`, `db_accounts.rs`, `proxy.rs`, `db_voice.rs`, `arbos-agentd/src/budget.rs` |
| H12 Sidecar page receipt | Fetched/mirrored/wake-accepted/checkpointed stages govern whether replay can safely pass an ASP event. | High | `arbos-sidecar/src/daemon.rs`, `mirror.rs`, `waker.rs`, `relay.rs` |
| H13 Relay speculative document cache | Cold/rebuilt/advanced/push-imported/authorized/refused states decide whether cached Loro state can be reused after a speculative push. | High | `arbos-relay/src/rooms.rs`, `state.rs`, `push_guard.rs`, `db.rs` |
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
| Account-session ownership | Login, pairing and token refresh asynchronously publish credentials/account state; stale work must never revive an ended session. **High** | I/Store/AppModel.swift; I/Sync/Auth.swift; I/Sync/RelayAPI.swift; M/sync/relay.rs; M/sync/credentials.rs |
| iOS transport-attempt lifecycle | Socket/task/generation flags coordinate idle, dialing, active, retrying and permanently stopped hosts around an already explicit room protocol. **High** | I/Sync/SessionRoom.swift; I/Store/AppModel.swift; F/lib.rs |
| Mac pending delivery ownership | User-message pending sets change on enqueue, failure and room ACK, but room ownership currently disappears in one global set. **High** | M/sync/relay.rs; M/sync/mod.rs; M/app.rs; M/sync/chat_send_queue.rs |
| Call-attempt ownership | REST call creation/join, UI cancellation and LiveKit/CallKit completion must belong to one current attempt. **High** | I/Store/CallManager.swift; I/Model/Call.swift; M/app.rs; M/calls.rs; M/ui/callview.rs |
| Matrix client identity lifetime | Starting/available/disconnected/stopped bridge state and callbacks must belong to the current account/client generation. **High** | I/Store/ServerStore.swift; I/Store/AppModel.swift; F/matrix.rs; M/sync/relay.rs |
| Snapshot publication lifetime | A detached doc read may complete after room/account cleanup and must not republish obsolete private messages. **High** | I/Store/AppModel.swift; I/Sync/CoreChatDoc.swift; I/UI/ChatView.swift |
| Attachment-send lifecycle | Picked/uploading/committing/pending/failed attachment sends use transient tasks and multiple collections rather than one send state. **Medium** | I/Store/AppModel.swift; I/UI/ChatView.swift; M/sync/relay.rs; M/app.rs |
| Per-chat FIFO worker lifetime | No-worker/active/queued/timed-out/cleared state is encoded by channel/task entries and timeout callbacks. **Medium** | M/sync/chat_send_queue.rs; M/sync/relay.rs |
| Draft-DM creation | Empty local draft becomes one in-flight create with queued sends, then real DM or retained failed draft. **Medium** | I/Store/AppModel.swift; M/draft_dm.rs; M/app.rs; M/sync/relay.rs |
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
| AUX-H7 Deployment lanes and image rollout | Queue files, canary/restart markers, holds and per-box outcomes determine whether a release starts, pauses, retries or finishes. | High | `scripts/deploy/arbos-deploy-worker`; `scripts/deploy/arbos-deploy-hook`; `scripts/feedback/deploy-relay.sh`; `docs/process-rules.md` |
| AUX-H8 Release-object publication — implemented | Tagged observed/creating/uploading/finalizing/completed/refused/failed states preserve draft intent and require actual publication before success. Stateless merge reports and existing atomic receipts are excluded. | High | `scripts/release/publication.py`; `scripts/release/publish-release.sh`; `scripts/release/test_publication.py`; `harness/arbos-pi-ext/publication-state-laws.test.ts` |
| AUX-H9 Shared Mac heavy-slot ownership — implemented | Explicit request/lease/GUI/boot states and an OS-backed permanent mutex serialize grants, reclaim and cancellation without changing calibrated scheduling policy. | High | `scripts/runner/arbos-heavy-slot.sh`; `scripts/runner/xcrun-slot-guard.sh`; `scripts/runner/gui-lock.sh`; `scripts/runner/tests/test_slot_lifecycle.py` |
| AUX-M1 Voice inbox dedupe and pause scheduling | Seen IDs, pending speech tasks and quiet-window deadlines decide whether a chat update is contextualized and spoken once. | Medium | `voice/arbos_voice/updates.py`; `voice/arbos_voice/worker.py`; `voice/tests/test_updates.py` |
| AUX-M2 Listener/event-batcher worker lifetime | Nullable tasks, active/closed flags and pending child sets imply start, stop and restart conventions. | Medium | `voice/arbos_voice/listener.py`; `voice/arbos_voice/relay.py`; `voice/arbos_voice/vision.py` |
| AUX-M3 Build/test watchdog verdict | A subprocess can still run after test completion, so PID liveness, verdict presence and timeout jointly determine the gate result. | Medium | `scripts/feedback/ios-gate.sh`; `scripts/feedback/rust-gate.sh`; `scripts/ci/xctestrun-env.py` |
| AUX-M4 End-to-end eval turn observer | Stream output, replicated rows, approval rejection, stall repair and timeout determine whether the observer declares a turn complete. | Medium | `evals/long-tasks/lib/driver.py`; `evals/long-tasks/lib/relay.py`; `evals/long-tasks/run.py` |
| AUX-M5 A/B eval resource and budget lifetime | Arm selection, temporary model/flag mutations, remaining budget and restoration are coordinated by locals and a finally block. | Medium | `evals/convergence/ab.py`; `evals/long-tasks/run.py`; `evals/long-tasks/lib/relay.py` |

Image deny hooks and the edge site router are stateless decisions, not meaningful implicit lifecycle machines. Simple SERVE/configuration booleans, immutable judge decisions and task lists are excluded. GitHub Actions/Docker declarative ordering is already declared externally; the custom observation/controllers above are the relevant implicit machines.

## Part 2: detail and regression prerequisites

The following records the discovered baseline and proposed transitions. The
implemented approval and voice changes are described below; baseline source line
numbers describe the audited revision, not positions after these extractions.
Main-agent route correction is ranked Medium: it controls acknowledgement/prose,
while actual tool permission is enforced separately. Its surprising fallback
combination is not by itself an authorization failure. It remains documented.

### Backend and shared Rust

#### H1 Agent foreground turn

- **Storage/states:** `agent.rs:1644–1788` TurnState plus Agent turn Option/turn_running/turn_lock (`:2095+`). Actual states include idle, queued, routing, acknowledgement live/closed, result live, tool-running, approval waiting, stopped/preempted, failed, complete; route mode and row mode are separate submachines.
- **Transition owners:** command/send/retry/stop/model-event handling in Agent; `acquire_turn_lock:4581` voice preemption; status quick lane (`status.rs:633`); handoff/relay-result paths.
- **Illegal combinations:** *representable, not established reachable* ack_split with incompatible assistant row, row_closed with dirty live text, turn_running with no owned turn or an ended turn, preempted but a later handoff still dispatching. Existing comments/tests document historical examples; do not assert they still occur.
- **Formalization:** yes, split route/row/work-ownership states with one transition owner and effect outputs. Preserve independent facts such as routine/from_voice; they are not all lifecycle flags.
- **Prerequisite:** capture existing user-visible ack/result/stop/voice-preempt behavior; add event-sequence illegal-combination regression tests; retain `turn_end_race_tests`, `ack_is_not_an_answer_tests`, refusal/model-switch suites. Full Agent integration is required.

#### H2 Durable jobs and worker attempts

- **Storage/states:** `store.rs:105–119,151–193` jobs/job_workers; `set_job_state:790–811` controls queued/running/done/failed/cancelled/interrupted and timestamps. Parked-for-approval is currently queued plus approval/step/worker facts, not a distinct SQL state.
- **Transitions:** Agent start/requeue/finish/cancel paths, approval resume, restart reclaim, process reaper; Store forbids cancelled→running, permits cancelled→queued for explicit restart reclaim and preserves first terminal timestamp.
- **Illegal combinations:** *representable* terminal with no ended_at; running with ended_at; cancelled job with worker/permit still starting; parked approval job consuming active capacity. Some races are already prevented by SQL and rechecks; treat those guards as evidence, not surplus.
- **Formalization:** yes, typed persisted lifecycle decoded at Store seam and separate worker-attempt state; distinguish explicit reclaim from racing start.
- **Prerequisite:** first-terminal-wins, cancelled-start exclusion, intentional reclaim, capacity accounting, one delivery per ask, and cancellation of actual process tree. Existing job cancel/queued/window/reparent/steer/retention tests must run. Exact SQL can be pinned in isolated SQLite, but that alone cannot verify process effects.

#### H3 RPC process and supervisor

- **Storage/states:** `rpc_child.rs:17–49` child/stdin Options, exited bit, pending replies and pid/tree; Agent pi_ready/ext_loaded/pi_parked/shutting_down and optional child. Effective absent→spawning→ready→parked/shutting down→exited→respawn.
- **Transitions:** spawn/read loop (`:70+`), call/call_timeout (`:208–240`), shutdown (`:250`), is_alive/exit report; supervisor/reaper and harness adapter maps participate.
- **Illegal combinations:** *representable* exited with pending callers never resolved, ready with absent stdin, shutdown while respawn allowed, contended map misread as dead. The last is a historical bug covered by current safe `Option<bool>` semantics in HarnessAdapter; not evidence of current reachability.
- **Formalization:** yes for process ownership plus one shutdown owner, while OS observations remain three-valued (alive/dead/unknown).
- **Prerequisite:** real stub subprocess EOF/timeout/crash/restart/shutdown plus cancellation/reap contention tests. No pure enum-only test proves OS behavior.

#### H4 Approval waiting and dispatch receipt

- **Storage/states:** `approval.rs:118–143` WaitMode plus optional waiter; SQL pending/approved/rejected/legacy-expired/cancelled and optional grant_consumed_at (`store.rs:1917–2007`). Effective waiting-inline/polled/deferred→decided→approved-unclaimed→claimed→dispatch attempted.
- **Transitions:** begin/request (`approval.rs:376,558`), decide command/decision (`:594,660`), grant consume (`store.rs:1979`), resume worker (`approval.rs:1444–1497`) and park (`:1717`). Core ApprovalState/Outcome are **already explicit**; the cross-layer dispatch/wait submachine is what remains implicit.
- **Illegal combinations:** *representable* Inline with no waiter, deferred with a live waiter, grant consumed on non-approved card, a decision re-dispatching a cancelled job, wait timeout treated as rejection. Existing CAS SQL and core outcome types prevent some of these today.
- **Formalization:** yes; tagged WaitDelivery and durable grant receipt type, one transition edge that claims before dispatch, preserve exactly-once uncertainty policy.
- **Prerequisite:** pin no deadline for unanswered agent cards, first decision wins, restart-surviving single-use grants, late approval doesn't reopen cancelled work, and bounded permit queue behavior. Existing tests at `approval.rs:3136,3287,3381` are required; auth/security boundaries apply.

#### H5 Agent room synchronization

- **Storage/states:** DocHost `sync.rs:47–111`: caught_up/closed/recovered/ever_caught_up, inflight Option, push_blocked_until, refusal/oversize counters, first_pending_seq, acknowledged VV; Store room_docs cursor+snapshot.
- **Transitions:** next_push (`:320+`), budget chunk export, inbound STATE/ROW/ACK/ERROR, catchup completion, reconnect and close/ceiling paths. Cold/catching up/live/blocked/refused/closed are lifecycle states; dependency repair and outbound state are orthogonal.
- **Illegal combinations:** *representable* closed host returned as active, cursor persisted past an unimported dependency, ACK advancing VV for a different batch, inflight plus export allowed, old pre-join command replayed as new work. Existing guards/tests address these and must remain.
- **Formalization:** yes, separate link/catchup and outbound push reducer with persistence actions.
- **Prerequisite:** restart with pending deps, same batch reconnect, refusal/rate timeout, oversize causal prefix, room cap close with live work, join-time replay exclusion and cursor/snapshot atomicity.

#### H6 Box provisioning/wake/liveness

- **Storage/states:** boxes/box_creates/box_wake_jobs SQL (`db.rs:60–82`), AppState maps (`state.rs:284+`), typed BoxState plus independent provider status/last_seen/held flag. States missing/creating/starting/reachable/connected/stale/stopped/held/failed.
- **Transitions:** create (`boxes.rs:123,134`), start/stop (`:540,665`), ensure_awake (`:981`), websocket/http liveness (`:1107–1157`), wake runner (`:1696`), fleet eviction/admission.
- **Illegal combinations:** *representable* a registered live box with no sandbox, held-stopped but automatic wake proceeds, duplicate create attempts, stale network sample overriding fresh socket evidence, create during freeze. Existing CreatingGuard/state helpers protect some cases.
- **Formalization:** yes, per-user durable lifecycle with provider effects as explicit commands; do not turn uncertain liveness into false.
- **Prerequisite:** fake provider create/start/stop failures and racing wake/hold/restart/eviction tests; verify local provider behavior without contacting cloud. Current full-native blocker applies; no provider smoke execution.

#### H7 Upgrade/repair and retained sandboxes

- **Storage/states:** UpgradeStatus string phase (`state.rs:425`), upgrade/refusal maps, current box row, retained history partial/archived/deleted flags, `Staged {keep,swapped}` (`upgrade.rs:549`). Measure→pack→stage→carry→swap→health verify→success or rollback→retention cleanup.
- **Transitions:** start (`upgrade.rs:151`), task execution, staged cleanup around `:549–620`, repair (`:891`), history sweeper (`:992`).
- **Illegal combinations:** *representable* swapped but disposable staged object, partial retained box eligible as repair source, current box deleted by cleanup, rollback selecting already-deleted fallback. Existing rollback and staged cleanup logic is load-bearing, not proof these remain reachable.
- **Formalization:** yes; owned tagged stages so cleanup eligibility follows stage, persistent recovery receipt for restart.
- **Prerequisite:** fake carry/upgrade matrices for every error/cancel edge, archive/rollback ownership, manifest preservation, no deletion of current box, restart mid-create. Keep existing rollback/room suites; paid Daytona ignored test stays unrun.

#### H8 Planned relay restart

- **Storage/states:** RestartSignal (`state.rs:221–252`) closing watch/open_sockets/work_frozen/creating and upgrading map; effective running/frozen/waiting work/announced/draining/exited.
- **Transitions:** freeze (`restart.rs:106`), thaw (`:114`), announce (`:42`), close/drain (`:70`), SIGTERM handler (`:213`), create guards and socket joins/leaves.
- **Illegal combinations:** *representable* closing while new box work starts, drain reports zero with late socket admitted, cancelled restart leaves frozen forever, no create receipt for externally running sandbox. Historical create orphan case is documented; existing guards need preservation.
- **Formalization:** yes; one restart coordinator state plus counted scoped leases for socket/create work.
- **Prerequisite:** late room opens, active upgrade, mid-create, interrupted restart and timeout tests. Existing room restart tests must run with fake providers.

#### H9 Sign-in, refresh rotation and device revocation

- **Storage/states:** auth_flows/auth_grants expiry+used timestamps; refresh_tokens expiry/rotation/revocation plus device revoked_at (`db_accounts.rs:24–36,117–133`). Session active/expired/rotated/revoked, with pending downstream device revocation.
- **Transitions:** OAuth start/callback/token (`signin.rs:410,529,668`), refresh (`:702`), logout/everywhere/device revoke (`:820,1025,1231`), Db take grant/mark rotation/revoke (`db_accounts.rs:608,857,919,930`).
- **Illegal combinations:** *representable* used grant exchanged again, rotated token accepted as fresh, revoked DB device still live on socket/Matrix, competing refresh requests minting two valid successors. Guards/CAS and immediate socket checks already address parts; no current exploit established.
- **Formalization:** yes, separate grant/token/device transition models and typed authority extraction. Maintain existing signatures and runtime checks.
- **Prerequisite:** replay/concurrent-refresh/revocation tests first; loopback stub OAuth/Matrix only; verify device-family semantics. No real credentials/accounts.

#### H10 Secret/vault approval and access

- **Storage/states:** secret_requests string status, decided_by/at, expires_at (`db_secrets.rs:41–46`); versions/retirement, access disabled state, vault status/disconnected_at, pause controls. Pending→allowed-once→consumed/expired, standing grant→revoked, source→rotated/retired.
- **Transitions:** expiry helper (`:447`), request lookup/decision/consume (`:914–996`), source revoke/disconnect paths (`:657,1212`) and secrets/vault handlers.
- **Illegal combinations:** *representable* one-use grant consumed twice; expired authorization used; paused system allows egress; grant points at retired secret or disconnected vault; prior director authority treated as current. SQL predicates and handler checks guard many paths; not demonstrated reachable.
- **Formalization:** yes at grant/access authorization seam; tagging status alone without atomic claim is insufficient.
- **Prerequisite:** existing ownership/director/host/source-version tests plus concurrent one-use consumption, pause/revoke between request and effect, expiry boundary/replay. Security policy applies to any implementation.

#### H11 Spend admission, kill switch and metering

- **Storage/states:** SpendLimit is already typed; implicit pieces are KillSwitch on/auto/reason/at (`db_accounts.rs:200–214`), account ledger/cap/override, machine_meter tick state, voice cumulative usage/charged state (`db_voice.rs:125`). Agent BudgetState maps refusal/expiry/notice (`budget.rs:117–130`).
- **Transitions:** check/read_only/call admission (`credits.rs:221,237,284`), charge (`:298,318`), meter loop (`:334`), ledger debit (`db_accounts.rs:978–1040`), UTC auto-clear and local refusal hold/retry.
- **Illegal combinations:** *representable* manual kill cleared by auto-reset, capped/credit blocked work dispatched before admission, duplicate usage report charged twice, meter tick advances without matching debit, voice/vision receipts drift. No new accounting defect established; existing code must be pinned as money-sensitive.
- **Formalization:** yes for kill-mode and charge/receipt lifecycle; preserve intentional soft-cap/current missing-account semantics unless owner approves a tested behavior fix.
- **Prerequisite:** conservation/idempotency over duplicate/reordered reports, UTC boundary, manual-vs-auto clear, nonfinite/negative rejection, budget notice/retry behavior. Isolated SQL laws are possible; real call/proxy integration still needed.

#### H12 Sidecar page receipt

- **Storage/states:** State.cursor persisted JSON, per-chat message JSONL and wake channel. Fetched→mirror written→wake accepted→cursor committed.
- **Transitions:** `daemon.rs:58–82 apply_page`, poll loop (`:88–142`), Mirror.apply_event (`mirror.rs:373+`), waker receipt/done.
- **Illegal combination:** **confirmed by code**: mirror.apply_event Err and failed wake send are ignored/logged but page.next persists. A forced local I/O error can therefore produce a committed cursor for an unmirrored event. Partial-success replay can also suppress a needed wake because already-written rows return no new candidate; this second edge needs a regression test, not an asserted runtime report.
- **Formalization:** yes, explicit per-event durable delivery receipt with retry state, preserving idempotent replay and separating best-effort brief updates.
- **Prerequisite:** fault injection before/after file append, wake channel closure, metadata failure and restart replay, plus current Source-driven success/reconnect tests. No external relay required; full sidecar crate is smaller than app but has HTTP/process adapters to compile/check.

#### H13 Relay speculative document cache

- **Storage/states:** `state.rs:159–189` optional room doc/key/message IDs. `rooms.rs:118–173 push_message_changes` imports untrusted payload before authorization into cached doc; caller must invalidate cache on refusal. States absent/rebuilt/current/speculative/accepted/invalidated.
- **Transitions:** cold build (`rooms.rs:33`), advance (`:62`), diff push (`:122–173`), authorize/refuse/append (`:762–834`), socket retirement cache drop.
- **Illegal combinations:** *representable* speculative refused state reused as committed, message-ID mirror advanced without stored append, two cold builds replacing newer accepted cache, key reports current while doc includes rejected bytes. Refusal-reset logic and existing regression at `rooms.rs:3471` cover a real historical failure; not evidence every listed race is present.
- **Formalization:** yes, transactional/speculative ownership type or reducer with accept/rollback action; preserve off-executor cold rebuild and incremental projection.
- **Prerequisite:** authorized/refused/append-failure interleavings, two simultaneous cold pushes, version-vector vs full rebuild law, rejected-removal followed by valid reply and quarantine consistency. Native integration required.

#### Medium/Low next-action details

Medium/Low are owner backlog, not silently migrated. Highest value among them: M6 timeline paging can issue duplicate concurrent loads and later response can overwrite a newer pagination token; add controlled out-of-order response tests before an explicit Loading/Loaded/AtStart state. M8 breaker should be `Closed { failures } | Open { until } | HalfOpen`, preserving single probe and configured windows. M4 outbox should prevent done_at+failed_at terminal overlap at write/decoded type seams while preserving poison-job parking and chat order. M1 socket CatchingUp/Live state must keep seq ordering and timeout semantics. M27 is already a narrow module with meaningful paused-clock tests; a claimed flag isn't automatically a defect.

Core command/status/route/widget approval enums, static feature/config booleans, one-shot parser flags, ordinary sorting and immutable DTO Options are **excluded** unless they participate in a lifecycle above. FFI forwarding enums are bindings, not a second implementation to rewrite.

#### Pure/high-risk seam feasibility

The existing High machines are integrated with Agent, Store, HTTP/process effects or durable receipts; none has a ready isolated reducer comparable to the verified standalone `core/redact.rs` test seam. Cheap prerequisite experiments can run the **exact** job transition/approval claim SQL against local in-memory SQLite without the app, including cancelled→running rejection and first terminal timestamp/single-use predicates. Those are genuine SQL laws but do not prove the Agent/process effects. A next implementation can extract a small native job/receipt transition file, test that actual file through an isolated Cargo harness, and retain native integration as blocked until full gates run. Do not call a newly modeled transition test a before-failing regression of existing runtime behavior.

No backend High transition rewrite was made. This portion is inventory complete /
High migrations blocked, with the exact native verification owner action recorded.

### Native clients and FFI

#### H1. Account-session ownership

**Effective states:** unauthenticated; browser/paircode sign-in in flight;
authenticated legacy device; authenticated refresh-capable session; refresh in
flight; signing out/revoked; changed relay/account. Errors are outcomes of an
attempt, not an independently combinable logged-in identity.

**Storage and transitions:** iOS credentials/tokens/user, signingIn/signInError,
pairingBusy/error and refreshTask live in AppModel (`:185–301`). Sign-in at
`:1438`, adopt at `:1496`, refresh at `:1523`, signOut at `:1570`, relay switch at
`:1596`, pair/unpair at `:1779,1801`. Mac stores optional creds and refresh lock
in Shared (`M/sync/relay.rs:207,226`), refreshes at `:344`, publishes a response
at `:394`, and clears session at `:525`.

**Illegal/surprising combinations and evidence:** iOS refresh captures current
tokens then awaits; completion calls adopt without checking that session/account
is still current. signOut/unpair/relay switch do not epoch-check the completion or
cancel refreshTask. A refresh finishing after logout can therefore attempt to
restore credentials; an obsolete failure can sign out a replacement session.
Equivalent Mac refresh response publication should be covered against concurrent
sign-out, even though its mutex already prevents refresh storms. This is a code
interleaving lead, not an executed exploit or demonstrated production incident.

**Worth formalizing:** yes. One session type with account/relay/generation identity;
attempt events carry that identity. Transition ignores stale success/failure;
credential writes and remote logout occur at accepted transition edges. Paircode
and refresh-capable sessions remain distinct legitimate cases.

**Regression first:** delay refresh using a loopback response; logout before
completion; assert no credentials/Keychain rewrite. Repeat A→B account/relay
switch and stale 401, simultaneous refresh single-flight, transient refresh error,
and revoked refresh. This touches auth and requires current behavior pinned plus
owner review. Native integration gates blocked; do not change auth here.

#### H2. iOS transport-attempt lifecycle

**Effective states:** new/not-started; dialing with socket+generation+deadline;
open awaiting protocol hello/catch-up; live host; reconnect delay; terminal
unavailable/stopped. The protocol's own states are already explicit and excluded.

**Storage and transitions:** `SessionRoom.swift:87–116` stores socket,
generation, started/stopped/sawOpen, drain flags, pendingFault/cleanClose and six
task handles. start/stop at `:195,201`; connect at `:247`; receive/failure at
`:340`; action drain at `:376`; reconnect at `:510`; send-chain at `:551`.
Generation guards already suppress many stale callbacks and must be retained.

**Illegal/surprising combinations:** the representation permits stopped with live
protocol/socket/task remnants, new host with obsolete pendingFault, or callback
delivery from a previous generation. `started && stopped` itself is **legal** for
a permanently stopped started object; do not “fix” it by guessing. The forbidden
condition is a stopped/obsolete attempt producing a send, cursor or live callback.

**Worth formalizing:** yes, around host attempt resources; avoid a second protocol
state machine. Carry socket+generation+tasks in active cases and edge effects for
cancel/dial. Reentrant drain remains a separate guarded execution concern.

**Regression first:** controllable socket: stop before open callback; foreground
redial then old receive/checkpoint/send completion; 403/404 terminal unavailable;
restart close fresh backoff; failed row leaves cursor intact; refused pushes keep
batch ids and do not reconnect. Existing `SessionRoomTests` covers several pieces,
but every newly forbidden interleaving needs a failing-before case. Native gates
blocked; no broad transport rewrite.

#### H3. Mac pending delivery ownership

**Effective states:** queued send; local row committed/pending; refused but retained;
ACKed/delivered; local failure/timeout; room/account removed. MessageStatus and
opacity projections alone do not own the pending transitions.

**Storage/transitions:** global pending user-id set in `M/sync/relay.rs:218`;
insert at `:4928`, failure remove at `:4944`, UI projection at `:3008`, ACK drains
the entire set at `:3913`. Queue timeout sends MessageSendFailed via initialization
at `:876`; UI consumes at `M/app.rs:3362`. Push refusal is already room-scoped.

**Illegal combination:** pending B lost because A's ACK drains global state.
Two simultaneous chats and project-doc versus user-chat ACK make ownership
ambiguous. No need to infer from boolean combinations: the current collection
operation explicitly erases unrelated ids. Test the actual behavior before change.

**Worth formalizing:** yes. A dependency-free pending-delivery type can own
`room → message ids` and room ACK/failure/removal transitions. Keep the real
protocol batch id/cursor semantics; a room ACK is not necessarily proof that every
later batch in that room was ACKed. Scope the improvement precisely.

**Standalone seam:** this collection state can be tested with std-only
`rustc --test`; reproduce original HashSet insert/drain first and then test the
production tracker module. Required properties: A ACK leaves B pending;
project-doc ACK leaves user messages pending; failure removes only its message;
duplicate ACK is idempotent; clearing account removes all. Native caller compile
still cannot be claimed green from this standalone check. Root owns whether this
limited extraction is appropriate under the blocked workspace gates.

#### H4. Call-attempt ownership around existing phase enums

**Effective states:** no attempt; outgoing REST create/join pending; incoming ring;
accepted permission/join pending; media connecting/connected/reconnecting; cancelled/
tearing down. Existing Phase/CallStatus describes media/UI progress, but current
attempt identity and cancellation acceptance are implicit.

**Storage/transitions:** iOS CallManager has Phase plus call/incoming/UUID,
outgoing/ring timeouts, ringback intent, audio ownership and isTearingDown
(`:97–173`). start/join await relay at `:227,257`; accepted join at `:482,499`;
connect at `:615`; teardown clears at `:674–713`. Mac `my_call`, incoming call,
timestamps and tones live in AppState (`:995–1021`); start at `:11902`, leave at
`:12066`, REST result at `:12129`; a separate worker has optional Session and
event receiver (`M/calls.rs:112`).

**Illegal/surprising interleavings:** Mac on_call_ready uses
`my_call.get_or_insert_with` and unconditionally emits Join. A delayed result can
recreate my_call after leave_call removed it. iOS start/join similarly adopts the
REST response and calls connect after an await without current-attempt validation;
hangUp does not own/cancel those async functions. Connecting with call=nil during
REST create is intentional, so a rule “every connecting phase must have a Call”
would be wrong. Distinguish requesting metadata from connecting media.

**Worth formalizing:** yes. Payload-bearing attempt states with request generation
and session/call identity. Accept REST/media completion only for the active attempt;
cancel edges stop audio/SDK work and make late success perform cleanup, never
rejoin. Preserve CallKit immediate reporting/fulfillment and audio-device rules.

**Regression first:** delay call-create and join responses; cancel before completion;
assert no Join, microphone/camera activation or call resurrection. Switch attempt A
to B and deliver A's late media/error callbacks; B must remain unchanged. Retain
CallKit duplicate push, denied mic and immediate-answer checks. Native gates/media
test environment blocked; no claim these races were reproduced on a device.

#### H5. Matrix bridge identity lifetime

**Effective states:** unavailable/off; eligible/not-started; starting for account
generation; active client+listener; disconnected active client; stopped/superseded.
Matrix SDK's own connection states are not being reinventoried.

**Storage/transitions:** iOS AppModel matrixClient/listener/snapshot/connected/
disconnectedSince/starting at `:469–489`; ServerStore start at `:718`, stop at
`:742`, callback dispatch at `:14–33`, connection/timeline apply at `:764,778`.
FFI bridge is an adapter over shared MatrixClient (`F/matrix.rs:171–197`). Mac
Shared optional client, started-at and servers-off flag at `M/sync/relay.rs:273–283`
with start/stop/restart paths (`:518,1737`).

**Illegal/surprising combinations:** iOS startup completion checks credentials !=nil
and matrixClient==nil. Account A can sign out then B sign in during the handshake;
the old A completion sees B's nonnil credentials and may install A's client.
Listener callbacks dispatch onto MainActor with a model reference but no session
epoch. A stale timeline/connection update can arrive after stop. Initial
matrixConnected=true with no client is deliberately “not showing a failure,” not
evidence of an illegal connected session by itself.

**Worth formalizing:** yes. Client identity/generation is required input to accepted
callbacks, with one state owner for starting/active resources and teardown effects.
FFI API can pass a generation-carrying native adapter rather than rewriting SDK.

**Regression first:** delay start A; sign out and start B; complete A; assert A is
stopped and never installed. Deliver A's queued timeline after B starts; assert B's
rows do not change. Retry current-session failure and duplicate start guards.
Native/Matrix SDK gates blocked; no credential/protocol edit in this audit.

#### H6. Detached snapshot publication lifetime

**Effective states:** no room; active doc revision; read queued/running for that
room generation; result accepted/superseded; room/account disposed.

**Storage/transitions:** AppModel docs/rooms/messages and snapshotGeneration at
`:902–940`; emitSnapshot captures doc and generation at `:2634`, reads detached
then MainActor applies when generation matches or current messages are empty.
wipeRoom at `:2565` and unpair at `:1801` remove docs/messages but do not pair the
background task with a room/account generation. applySnapshot at `:2671` accepts
list even when no current doc exists (its doc check is optional).

**Illegal/surprising interleaving:** an old read completing after room/account
disposal can satisfy the empty-messages escape hatch and republish the old list.
The escape hatch itself fixes a real starvation problem during continuous updates;
do not remove it without preserving first-paint behavior.

**Worth formalizing:** yes. Room epoch plus doc revision in each read ticket;
disposed epoch always rejects. Same-epoch superseded reads may still paint an empty
view when that is the deliberate starvation rule. Account epoch composes with room
epoch rather than making every row builder auth-aware.

**Regression first:** controllable read executor: start slow snapshot, wipe room or
unpair, finish read, assert messages remain absent. Then new room same key with new
doc, and continuous newer reads over same active doc; retain current blank-chat
and starvation recovery tests. Native test host blocked; no data-lifetime patch.

#### Medium/low disposition and completion boundary

Keep Medium/Low entries for owner prioritization. Strong first refinements after
native gates become available are composer-operation tagged union (mutually
exclusive fields are already documented), bounded draft creation/send state, and
cache/read-mark tests using current native seams. Gesture/scroll/audio state must
preserve calibrated distances, timing and accessibility behavior.

No formal state rewrite was made by this read-only subtask. The full native
Phase 4 outcome is **blocked**, not passed: High items require failing-before
regressions against real lifecycle seams and full caller/native checks. Current
local disk is ~1.6 GiB free; fork CI requires maintainer approval (`action_required`,
no jobs). The std-only pending-delivery seam can make limited progress, but cannot
establish that every High machine is explicit or native behavior fully verified.

### Harness, voice and operational tooling

#### AUX-M6 — Main-agent routing and correction (Medium)

Storage: closure `routeAttempts`, `routed`, `relayTurn`, `routeCorrections`, `correcting` at index.ts:262–270. Effective states include awaiting initial route, first-route rejection/retry, routed, stamped-steer continuation, pending one corrective continuation, corrected completion and fallback-reply completion. Relay turn is an orthogonal source/context, not itself a terminal state.

Transitions: before_agent_start resets state (:308–317); tool_call prevents unrouted direct calls and permits the relay send_message implicit reply (:356–417); route.execute handles invalid retry/fallback, successful route and correction completion (:580–632); input handles stamped continuations (:889–900); agent_before_settle requests at most one correction (:907–926). Effects are hook requests, audit events and user-facing tool results.

Illegal/surprising combinations: booleans allow routed plus correcting and a correction with no counted correction. The former is concretely reachable when correction is requested, two route attempts fail, and fallback sets routed=true but returns before clearing correcting (:587–595). This is an observed combination, not yet proof that current externally visible behavior is wrong; pin its intended semantics before forbidding it. Counter/state duplication also permits unconstrained repeated calls after routing.

Worth making explicit: yes. This can be a pure transition seam with effects outside it. The existing Node/TypeScript gates work: **native/LiveKit capacity does not block this machine**. First add local fake-hook regression sequences for second-invalid fallback during correction, ordinary next-turn reset, stamped steer, relay implicit reply and repeated settle. Do not silently decide new behavior for the surprising combination. A cosmetic object containing the same booleans does not complete the rewrite.

#### AUX-H2 — Approval polling and parking

Storage: `ApprovalState` union (:209), generic HookResponse, local state/approvalId/deadline/reason in awaitApproval (:468–533). The union enumerates wire labels, and the final switch is exhaustive; it does not declare the lifecycle or encode which states require an approval ID. Effective lifecycle: standalone local mode, checking, unavailable/error, allowed, pending-with-ID, polling, park requested, parked, approved, rejected, cancelled, and legacy expired.

Baseline transitions: /hook/approval supplies the initial decision; periodic /approval-status may decide during grace; /approval-park atomically hands pending execution back to agentd; decided outcomes either admit the client call or block it. The baseline first hook failure returned undefined (allow/no gate), whereas later pending failures retained pending/block semantics. This was pinned and intentionally fixed below; distinguish standalone no-hook configuration from configured-hook outage.

Illegal/surprising combinations: pending without a nonempty approvalId is representable and would be polled/parked; generic response shapes can describe parked and decided simultaneously; an initial malformed/absent state defaults to allowed; client execution after a successful park would duplicate agentd execution. Unknown final labels already fail closed. These are wire/lifecycle validation concerns, not a reason to remove the current exhaustive switch.

Worth making explicit: yes, at the shared approval protocol seam. Prerequisites: local fake-hook tests pin no-hook mode, initial outage, pending/empty ID, decision at deadline, park wins versus approval wins, cancellation and malformed labels. Authorization behavior changes require owner visibility and the affected security-boundary requirements. No live approval operations are necessary or permitted.

#### AUX-H3 — Voice call lifetime, charged work and shutdown — implemented

The baseline stored leaving/stop_reason/open_work alongside tasks and nullable
resources. Actual VoiceCall regressions reproduced stranded finalization after a
cleanup exception, caller cancellation canceling teardown, concurrent finish
returning early, and late callbacks starting work after termination.

`call_state.py` now declares Connecting, WaitingForHuman, StartingSession, Running,
Finishing, CleanupFailed and Closed. `lifecycle_transition` owns startup progress,
first-reason-wins stop requests, attempt tokens and failed-step retries. Closed is
absorbing. The worker shares one shielded finisher; it never cancels or gathers
itself. Failed local resource cleanup remains owned and retryable; successful
steps are not repeated. `_done` and one SDK shutdown request happen even when
cleanup raises. Startup, queued tool dispatch, deferred results, look/context and
group/greeting work check lifetime or ownership again after relevant awaits.

The final cumulative usage report and event flush remain **one best-effort
first-attempt operation each**. No financial retries, durable event replay or new
billing policy were introduced. Pre-session cap stops start no audio and send no
new final usage. Open-work status derives from owned pending tool tasks, rather
than an independent counter. Original unlimited/cost-stop/leak-guard tests remain.

New five-second cooperative waits cover local cleanup; the session close already
used five seconds. Task drain uses asyncio.wait and retains unfinished ownership.
asyncio.wait_for requires cancellation cooperation, so arbitrary SDK hangs are
not proved bounded. Native billing/durability and live-call behavior remain
outside this local verification.

#### AUX-H4 — Audio mode and mute application — implemented

The baseline wrote mode and mute intent before awaited SDK effects. Tests first
reproduced a failed mute followed by a false “Already muted,” partial mode
application looking complete, and late mode completion after close.

Two orthogonal application lanes now use Unapplied, Applying, Applied and
ApplyFailed plus monotonic attempt tokens through `apply_transition`. Group and
Duplex(identity) are tagged mode values; Duplex rejects absent/empty identities.
The mode and participant commit together only after all SDK steps succeed. A
failed/uncertain lane must apply again even when the requested target equals its
last confirmed value. Mode changes serialize, instructions use the target mode,
and finishing invalidates pending ownership without waiting on the mode lock.
Resources created during partial application remain owned for cleanup.

Proactive say/greeting requires confirmed unmuted output. Interactive group
model/tool handling remains available while stably or uncertainly muted so the
existing unmute_self recovery path remains reachable. Lifecycle/epoch checks
prevent stale judge/silence completions starting new work; they do not invent a
new provider or text-only mode. The one-shot goodbye while Finishing is preserved.

#### AUX-H5 — Vision consent and frame delivery — implemented

Paused-encoder tests reproduced frames/JPEG results escaping disable, close,
re-enable, session replacement, detach and mute/unmute. The first six cases had
4 failures and 2 positive controls; the expanded baseline had 7 failures and the
same 2 controls. Later source-before-change tests pinned cleanup ownership,
immediate seeing retirement and cancellation of stop/detach.

`vision_state.py` declares Enabled(generation), Disabled(generation) and
Closed(generation), with one pure transition function and generation leases.
Closed absorbs valid events; old leases stay revoked after re-enable. The adapter
also checks its exact camera-map ownership, stream and model-session identity at
frame delivery and after executor encoding. Seeing identities derive from
currently enabled owned streams; queued attributes/status writes check generation.

Retirement accounts camera time before awaits and retains actual SDK close tasks.
Stop schedules every retired close before awaiting; caller cancellation cannot
cancel those tasks, and later stop joins remaining cleanup even with an empty
camera map. Ordinary SDK close errors retain best-effort behavior with type-only
warnings; physical cleanup after an SDK error is not guaranteed. Already-dispatched
uploads are not recalled. No live camera, model or provider call was needed.

#### AUX-H6 — Feedback fix approval, gating and shipping

Storage: per-item state.json through Store (:960–997), mutable status strings, branch_head/gated_head, awaiting fields, approved_by_jacob, do_not_merge, gate/Rust/iOS results; global approval and cursor files. Effective item states include queued, running, gating, awaiting owner (ready/stale/conflict), pushed/merged awaiting release, shipped, failed, rejected, deferred, no_change, duplicate and handed_off.

Transitions are distributed across intake/launch/result (:1950–2099), hold_for_approval/keep_current (:1440–1585), gate and merge paths (:2140–3700), check_ship/ship/mark_deployed (:3782–4439, :5146), and late-push followups (:4793–4876).

Illegal combinations are those existing guards already protect: outside item merged without per-item approval; operator-held item merged; branch live head differs from the gated head; stale gate attached to new head; shipped before the required artifact/deploy proof; followup losing source approval tier. JSON permits these shapes, so enforcement is spread over many branches. Do not claim a current bypass merely because such combinations are representable.

Worth making explicit: yes, as versioned persisted item states plus a pure transition policy and effect adapters. Safe prerequisite is a hermetic fake-GitHub/store fixture replaying owner approval, live-head, followup inheritance, restart and artifact completion sequences. Existing repository rules and negative controls remain authoritative. Live operations are forbidden; absence of permission for live deployment alone is not an architectural blocker—offline coverage must be built before this high-impact controller rewrite. No operational changes were attempted.

#### AUX-H7 — Deployment lanes and image rollout

Storage: deployment queue requests, deployed-sha files, per-kind failed markers, agentd.hold/canary/rolling markers, relay.deploying/www.reloading markers, skip lists, retry/failure counters and externally reported per-box state. Effective lanes include idle, queued, settling, blocked by hold/quota/other lane, building image, canary, rolling test/real boxes, retry pending, failed/held and deployed.

Transitions: worker helpers :129–225, deploy_relay/deploy_voice :353–402, image build/roll and per-box passes :403–1015, finish_roll :945–996, roll_continue/tick thereafter, operator hold dispatch :1765–1769.

Illegal combinations: held lane proceeding; relay restart/canary conflict; declared deployed while eligible boxes remain pending or commit ancestry is unproven; skipped/refused boxes mistaken for successful upgrades; automatic prose/tag mutation lifting a manual hold. Existing selftest deliberately protects manual hold semantics and passed at baseline. Do not convert persisted markers to a single cosmetic enum while leaving all writers independent.

Worth making explicit: yes, with atomic persisted phase transitions and per-box results. First expand hermetic fake-command/filesystem tests for restart/crash and failure ordering while preserving existing hold controls. Operational effects and new runtime writes are outside this audit; no deployment runs or shared state were inspected.

#### AUX-H8 — Merge/release observation and publication

Storage: shell locals verdict/class/GO state, artifact digest cache, current tag and candidate commit, CI/release metadata, persistent loop files. The small verdict_class/verdict_clears_go functions in checks.sh:415–450 are already explicit, exhaustive, fail-closed classifiers. **Do not count those pure classifiers as an undeclared machine or replace their guarantees.** The implicit part is the wider temporal observation/coverage/publication lifecycle.

Transitions: merge-window verdict (:135–260), go (:419–551), after (:553–610), artifact verification helpers; auto-stable planning/tag/main publication; release-gate debt observations. Effective observations include waiting, red, untested, unknown/blocked, no applicable path, covered by a descendant, artifact pending, verified release and published/live.

Illegal combinations: aborted zero-task CI interpreted as green; unknown label interpreted as wait/pass; a covering release for the wrong tag/ancestry; cached verification for another digest; tag published but artifact absent presented as shipped. Existing negative controls cover many of these. Baseline merge selftests cannot fully execute on this Mac (Bash 3.2/GNU-stat assumptions).

Worth making explicit: yes at the orchestration seam while preserving existing declared classifiers. First restore a supported local GNU/Linux test environment or provide portable-test coverage, then replay the real negative controls and crash/partial publication sequences. Never merge, tag, publish or deploy to verify the rewrite.

#### AUX-H9 — Shared Mac heavy-slot ownership

Storage: atomic slot directories containing owner PID/name/time/UDID, gate/rig/release queue markers, fairness counters, priority/lease/load flags in arbos-heavy-slot.sh. Effective states: request not queued, queued, refused for release, owner dead, bounded timeout, granted normal/priority/fair/uncapped lease, released and stale/reclaimed.

Transitions: acquire (:271–429), liveness/lease reclamation helpers, release/bind/dispatch (:438–449), xcrun guard. Existing comments describe physical host overload and ownership bugs; preserve them and the calibrated limits.

Illegal combinations: grant after owner death; active rig grant during a release exclusion window; timed-out no-wait/bounded requester leaving a marker or becoming uncapped; simulator boot without owned slot; reclaiming a live exempt release lease. These are the intended guarded invariants, not newly demonstrated regressions.

Worth making explicit: yes, with separate request/lease states and filesystem/PID effect adapters. Pin fake-process/filesystem sequences, FIFO/fairness, release precedence, owner death and bounded timeout first. Never test on shared slot directories or boot shared simulators. The current audit changes nothing in this operational boundary.

#### Medium/low disposition and completion boundary

Medium machines remain report-only as the skill requests. In particular, voice updates must deduplicate tool/inbox copies and never speak over a person; watchdogs must distinguish a real completed verdict from a killed unfinished test; eval observers must distinguish an acknowledgement from terminal work. Existing sleeps encode these timing rules and are not removed.

The auxiliary inventory is complete for the meaningful controllers found by the scoped scan, with explicit exclusions above; it is not a proof that every source line lacks another machine. AUX-H2–H5 are implemented and locally verified. AUX-H6–H9 still require hermetic orchestration/process/filesystem fixtures before a high-impact rewrite. Medium route correction remains report-only per the phase rule; working TS tooling is not a blocker. The initial missing voice test prerequisites were resolved with a private, hash-verified wheel-only SDK environment, rather than retained as a blocker.


## Implemented High machine: shell approval

`harness/arbos-pi-ext/approval.ts` declares checking, pending-with-ID/deadline,
parking and terminal allowed/denied/parked/failed states, with one pure
`approvalTransition` function. Network, timer and audit effects stay at its edges.
Effect 4.1.2 is pinned for this actual async/validation/error seam; phase-specific
Effect Schema decoders reject malformed/unknown fields and bound identifiers,
text, response size (64 KiB), UTF-8 and each request (10 seconds). Initial,
status and park shapes match the Rust producer. Standalone no-hook mode remains
explicit; configured hook, import or runtime failures cannot grant permission.

This intentionally fixes three reproduced admission bugs: an error reply,
missing state and unsuccessful HTTP status with an allowed JSON body all formerly
allowed the call. Valid policy grants, single-use/standing grants, decisions and
parking races stay covered. Tool audit retains identifiers and input hash while
removing raw command previews; a synthetic privacy regression failed before the
removal. Other runtimes' existing raw previews are left to the logging phase.

Both image recipes supply one manifest-pinned shared Effect installation and
preserve workspace symlinks. The dev bootstrap must include package.json. Installs
occur at build/boot preparation, never at module initialization; lifecycle scripts
are disabled. `image/tests/test_runtime_packaging.py` exercises actual recipe
shell with a no-network npm double and real installed-module resolution.

Effect is loaded only on the first configured shell approval. Ten fresh Node
processes per arm, alternating order, measured cold index import 68.30 → 68.27 ms
(within noise); first approval 8.94 → 60.60 ms; 50 warmed checks 0.919 → 1.354 ms.
RSS after the first approval was 106.5 → 126.6 MB. This is an explicit security
validation cost, not an application speedup; ordinary index startup avoids it.
Filesystem caches were not flushed and no app/model latency was measured.

## Implemented High machines: voice lifetime, audio and vision

AUX-H3–H5 now have pure, immutable tagged states in `call_state.py` and
`vision_state.py`; `worker.py` and `vision.py` own SDK effects and task/stream
resources. The adapter regressions exercise the actual controllers with synthetic
local effects under the actual SDK imports. The final guarded gate is **84 passed,
0 failed, 0 skipped**, 6.203 s wall / 5.21 s pytest: unchanged original 10 tests,
53 worker cases and 21 vision cases. Network denial and credential/network/
subprocess audit reported zero access attempts. No provider, device, room join or
model download ran. One existing pytest import-rewrite warning remains.

The initial worker tests were RED before product changes: 6 failed / 2 controls
passed (3.070 s). Vision's first six cases were 4 failed / 2 controls passed
(1.73 s pytest); adding three revocation cases produced the separately retained
expanded baseline of 7 failed / 2 controls (1.989 s guarded wall). Later tests
pinned startup, late speech/tool work, stream ownership and cleanup cancellation
before those respective fixes. The final queued-dispatch regression was 1 failed /
52 passed before its fix. These are parameterized cases at different stages, not
counts of distinct bugs; [GATES.json](state-machine-checks/GATES.json) retains each
exact set/count and duration.

Six Effect Schema/Arbitrary laws, seeds 84001–84006 and 256 generated cases each,
exercise the actual pure Python transitions through an isolated SDK-free JSONL
bridge: **1,536 new voice cases**, separate from the approval law's 256. Mandatory
critical transitions prevent vacuous success; generated traces vary surrounding
events. Four isolated actual-function mutants falsify five target laws and each
shrinks once with retained replay data. This is finite exploration, not a proof.
The final harness is 44/44 (7.300 s); strict typecheck passes (2.558 s).

The two original voice test files and runtime dependency declarations are
unchanged. A small optional test extra and `voice/tests/README.md` expose routine
setup. The prior missing SDK/test dependencies were resolved with a private,
hash-verified wheel-only environment; there is no remaining SDK ABI/setup blocker
for these local tests. [Retained checks](state-machine-checks/README.md) provide
portable commands, sanitized gates, source hashes, review and a mutant harness.

Calibration checks preserve all original module constants, VAD/turn handling,
silence/idle/usage loops, goodbye body, original prompt strings and speech
arguments; vision encoding/diff, camera-time math and calibration constants are
unchanged. `respond_group` and `greet` gained ownership guards, so their whole
functions are deliberately **not** claimed AST-identical. New five-second local
cleanup waits are distinct from the preserved calibration values, and depend on
cooperative SDK cancellation. One best-effort final usage report and event flush
remain unchanged in policy; no financial retry, durable-event replay or live
media/billing guarantee is asserted. Independent scoped controller review is
PASS at the source hashes retained in the checks directory.

## Before / after verification

| Row | Before | After |
| --- | --- | --- |
| Harness typecheck | pass, 2.372 s (original package/compiler) | pass, 4.282 s (`pnpm -r typecheck`) |
| Harness build | pass, 2.803 s (same original no-emit compiler) | pass, 4.337 s (`pnpm -r build`) |
| Original harness tests | 14 passed, 0 failed/skipped; 0.343 s | all retained within 37 passing tests |
| Valid approval controls | 24 total passed, 0 failed/skipped; 0.713 s | retained |
| Three admission regressions | 24 passed / 3 failed / 0 skipped; 0.730 s | pass |
| Complete harness suite | original baseline 14 tests | 37 passed / 0 failed / 0 skipped; 1.424 s |
| Terminal refusal law | absent | 256 Schema-derived cases, fixed seed; included in suite |
| Final harness including voice | prior approval suite 37 tests | 44 passed / 0 failed / 0 skipped; 7.300 s; typecheck 2.558 s |
| Original voice tests | initial collection blocked by routine test dependencies; later original 10 pass | unchanged 10 retained within final 84 |
| Worker controller regressions | initial 6 failed / 2 passed; 3.070 s wall | 53 passed within final guarded gate |
| Vision controller regressions | initial 4 failed / 2 passed; expanded 7 failed / 2 passed | 21 passed within final guarded gate |
| Combined actual-SDK gate | original 10 tests; negative controls added before each respective fix | 84 passed / 0 failed / 0 skipped; 6.203 s wall / 5.21 s pytest |
| New voice transition laws | absent | 6 × 256 = 1,536 cases; seven invalid-protocol requests; 1.308 s |
| Voice mutation controls | absent | four isolated actual-function mutants detected; five laws falsified, each with one shrink/replay |
| Raw-command privacy regression | 1 failed before preview removal | pass; hash/IDs retained |
| Byte-limit negative control | byte-guard-only mutant fails 1/1 | real code passes padded valid-JSON test |
| Runtime packaging regression | absent | 9 passed / 0 failed / 0 skipped; 2.399 s |
| Formatting/lint | baseline Rust fmt pass; shellcheck/actionlint unavailable | Rust sources unchanged; diff check passes; actual generated shell parses in packaging suite |
| Native clippy/check/test/build | blocked | blocked; no passing counts claimed |
| Full Docker image build/smoke | blocked | blocked; local shell/import checks do not prove a full image |
| Whole High migration inventory | implicit cross-field lifecycles | AUX-H2–H5 explicit and locally verified; native/operational High prerequisites remain blocked |

Run `cd harness && pnpm install --frozen-lockfile --ignore-scripts && pnpm -r typecheck && pnpm -r build && pnpm -r test`,
and `python3 image/tests/test_runtime_packaging.py`. The latter uses temporary
files; its real Effect import check explicitly skips if dependencies are absent.
The 10-second timeout is source-inspected; no timing-based timeout test is claimed.
Independent approval, packaging and voice controller reviews found no remaining
actionable defect in the implemented scope. Voice commands and compact evidence
are retained in [state-machine-checks](state-machine-checks/README.md).

## Needs the owner

Review the intentional fail-closed approval/privacy behavior and the reproduced
voice failure-path fixes, then approve native fork CI (or provide local build
capacity) before the remaining native High migrations. Preserve standalone mode,
one best-effort voice final usage report and existing media calibration. Prioritize
room/account lifetime and durable receipt regressions next.

Operational AUX-H6–H9 still need hermetic controller/process/filesystem fixtures;
owner decisions are needed where those fixtures reveal ambiguous persisted
approval, hold or release semantics. No merge, deploy or live access is requested
to test them. Voice SDK setup is resolved; live provider/device/media behavior,
native billing/durability and arbitrary SDK cancellation suppression remain
unverified. The whole-repository Phase 4 exit remains **blocked**, despite the
implemented and locally passing AUX-H2–H5 scope.


## Operational ownership follow-up

The [publication inventory](state-machine-checks/release-publication.md) corrects
the original umbrella: merge-window/release-debt reports are fresh derived
observations, cache receipts are already explicit, and the actual release-object
publisher had two reachable lifecycle defects. Its five-case original replay had
3 positive passes and 2 semantic failures. The applied tagged-state implementation
passes 21 actual-wrapper/helper tests (root repeat 14.388 s), including malformed
observations, option-looking assets and owned-child cancellation. Three generated
laws cover 768 cases, with a real-function mutant shrunk and replayed. Signing,
verification and feed/prune boundaries remain byte-identical. No real release or
credential was used. NoPromote refuses an existing stable release, preserves an
existing draft, and retains the historical public-prerelease policy for new tags.

The [runner inventory](state-machine-checks/runner-leases.md) records atomic
admission/publication, authoritative owner metadata, retirement accounting, GUI
exclusion and cancellation. The original expanded batch had 7 passes/10 failures;
later overlapping failure-first batches are kept separately, never summed. The
applied source passes 34 actual controller/OS tests (root repeat 20.813 s), with
no skipped tests or real shared paths. Scheduling constants and policy helpers
are unchanged. macOS uses native lockf, Linux a flock adapter; actual Linux
controller execution remains a separate platform check. Runtime-tagged Bash
states do not offer a compile-time algebraic type. Hard host failure, PID reuse
and already-dispatched physical work remain explicit ceilings.

Both follow-ups are local correctness checks, with no application speed claim.
The exact applied-source counts are in
[ROOT_OPS_GATES.json](state-machine-checks/ROOT_OPS_GATES.json). The reviewed Swift
loopback test-plan fence is carried identically here before native-source pushes;
its 12 helper/DSL contract groups pass. Broader native ownership fixes and the
catalog's blanket blocked wording are under active reassessment; missing tests,
old SDK availability and code size are not external blockers.


## Feedback and generated operational laws

The [feedback inventory](state-machine-checks/feedback-admission.md) and
[verification](state-machine-checks/FEEDBACK_VERIFICATION.md) close a prepared-tree
approval bypass and interrupted shipping-state writes. Actual original Loop/Store
fixtures had 6 positive passes and 14 overlapping semantic failures; the applied
controller passes all 20 (root repeat 0.031 s test time), without live merge,
feedback messages, key/JWT operations or deployment. Human approval remains
per-item, while gate coverage and current branch identity are SHA-bound. All
members must pass before the single prepared-tree merge effect. Delivery phase,
pending work and device evidence use the existing atomic record write. Both
installers include the new stdlib runtime module; neither installer was executed.

Five feedback laws cover 1,280 generated cases with an actual-function mutant
shrunk/replayed. Four runner laws cover 1,024 cases plus bounded protocol recovery;
two actual-function mutants are detected, shrunk and replayed. The public runner
mutant adapter was corrected to accept an explicit repository/new output directory,
copy only its four required sources, and rerun its unmodified control, avoiding a
private-layout dependency. That adapter passes its control and both negative
controls. No controller source is mutated in place.

The complete applied harness passes **59 tests / 0 failed/skipped** in 23.294 s,
with TypeScript passing in 3.309 s. This includes the earlier approval/voice cases
and new publication/runner/feedback laws; do not add overlapping counts as separate
coverage. [OPS_HARNESS_GATES.json](state-machine-checks/OPS_HARNESS_GATES.json)
retains commands and summaries. Native account/call/snapshot ownership and remaining
backend classification are still active work, not blanket external blockers.
