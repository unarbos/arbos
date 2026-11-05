# Client ↔ relay sync (crdt-sync)

Source of truth for how an Arbos **client** (Mac app, iPhone app, `arbosctl`) talks to the **relay** (`arbos-relay`) about chat state.

This document is drawn from the code as it exists today:

| Layer | Code |
| --- | --- |
| Shared Loro chat writes (box / core) | `crates/arbos-core/src/doc.rs` |
| App sans-IO room session | `crates/arbos-client/src/room.rs` (`RoomSession`) |
| Shared Matrix client (D88 C1: servers and channels) | `crates/arbos-matrix` (`MatrixClient`: sliding sync through the relay proxy, channel timelines as chat-doc message rows, send, read) |
| UniFFI / Swift bindings | `crates/arbos-ffi` (wraps client + core + the Matrix client; produces xcframework) |
| Golden doc / wire fixtures | `crates/arbos-core/fixtures/` |
| Mac chat doc (reader shim; writes via arbos-core) | `crates/arbos-mac/src/sync/doc.rs` |
| iOS chat doc (arbos-core via ArbosFFI) | `apps/ios/Arbos/Arbos/Sync/CoreChatDoc.swift` |
| Binary room frames | `crates/arbos-core/src/wire.rs` (Swift via ArbosFFI) |
| Protocol version | `arbos_core::PROTO_VERSION`, iOS `protoVersion` |
| Mac HTTP + room client | `crates/arbos-mac/src/sync/relay.rs` |
| iOS HTTP + room client | `apps/ios/Arbos/Arbos/Sync/RelayAPI.swift`, `SessionRoom.swift` (drives arbos-client `RoomSession`) |
| Relay rooms / REST | `crates/arbos-relay/src/rooms.rs`, `api.rs`, `blobs.rs`, `main.rs` |

### Core vs client crates

- **`arbos-core`** — ids, `wire` (including `wire::room_name`), the Loro chat-doc schema, writers, `project_reactions`, `touched`, `presence` (the one "who is typing" sentence both apps read — the Mac calls it directly, the iPhone through `arbos-ffi`, so the wording cannot drift), and the shared HTTP **response DTOs** in `crates/arbos-core/src/api.rs` (`ProjectDto`, `ChatDto`, `PeopleChatDto`, `UserCardDto`, `AccountUserDto`, `MeResponseDto`, `MeUserResponseDto`, `UsersPageDto`, `UnreadDto`, `MachineStatusDto`, `MachineStateDto`, …). Shared by the box, relay, and every client.

### Projects and chats response shapes (`api.rs`)

- **Source of truth:** `crates/arbos-core/src/api.rs` — serde structs marked `#[typeshare]`, each with a `Dto` suffix. The relay builds these and serializes them; the JSON on the wire is unchanged from the old `json!` helpers.
- **Swift:** `./scripts/gen-swift-types.sh` runs `typeshare-cli` (pinned) on `crates/arbos-core` and writes `packages/ArbosFFI/Sources/ArbosFFI/RelayTypes.swift`. CI fails when that file is stale. Do not edit `RelayTypes.swift` by hand.
- **`arbos-client`** — app-only sans-IO `RoomSession` (handshake, catch-up, pending PUSH, reconnect backoff, held wake/presence). Depends on `arbos-core`. No tokio, tungstenite, or HTTP.
- **`arbos-matrix`** — the apps' Matrix client (D88 C1): one `matrix-sdk` client per device (no E2EE, memory store, injected HTTP client), sliding sync through `relay/_matrix/client/*`, servers (Spaces) and channels as list rows, channel timelines as the same message rows a chat doc projects, sending with `life.arbos.id` = the app's ULID, read receipts. Depends on tokio and `matrix-sdk`; like `arbos-client`, **only `arbos-mac` and `arbos-ffi` may depend on it** (CI checks). The relay never uses it (it has its own appservice client).
- **`arbos-ffi`** — UniFFI layer over `arbos-client`, `arbos-matrix` and the core pieces Swift needs. The only crate that names Swift or `uniffi`. Builds an xcframework + generated Swift bindings (`scripts/build-xcframework.sh`).
- **Who may depend on `arbos-client`:** device apps and the FFI crate (`arbos-mac`, `arbos-ffi`). **`arbos-relay`, `arbos-agentd`, `arbosctl`, and `arbos-local` must not.** CI enforces this with `cargo tree -i arbos-client`.

### arbos-ffi / xcframework

- **Who builds it:** a Mac with Xcode (CI Mac job or by hand). Linux CI only type-checks / tests the Rust crate; it does not produce the Apple artifact.
- **How:** from the repo root, `./scripts/build-xcframework.sh`. The script `rustup target add`s `aarch64-apple-ios`, `aarch64-apple-ios-sim`, and `x86_64-apple-ios`, builds `arbos-ffi` for all three, `lipo`s the two simulator libs into one universal slice, runs `uniffi-bindgen` pinned to the same version as the workspace `uniffi` crate (`=0.32.2`), and installs `ArbosFFIRust.xcframework` plus `ArbosFFI.swift` into the local Swift package at `packages/ArbosFFI/` (staging under `target/xcframework/` via `OUT_DIR=`).
- **Local Swift package (iOS):** checked-in `packages/ArbosFFI/Package.swift` exposes a **dynamic** library product `ArbosFFI` over a Swift target plus a `.binaryTarget` `ArbosFFIRust` (the gitignored xcframework). The Xcode project depends on that product for **Arbos** and **ArbosTests**. The **Arbos** app target embeds `ArbosFFI.framework` via an explicit **Embed Frameworks** build phase — a local package referenced as a folder is not embedded on its own. **ArbosNotificationService** does not embed it. A separate dynamic image keeps UniFFI names (`ChatDoc`, `Touched`, `BoxState`, `FrameType`) in module `ArbosFFI` so they do not clash with the app’s own types.
- **ArbosTests:** unit-test target hosted by the Arbos app (`@testable import Arbos`). It links the **ArbosFFI** package product (not the xcframework / generated Swift directly), reads golden fixtures from `crates/arbos-core/fixtures` via a `#filePath`-derived host path, and checks `CoreChatDoc` / `ArbosFFI.ChatDoc` / `ArbosFFI.decodeFrame`.
- **Hand check:** on a Mac, from the repo root: `./scripts/build-xcframework.sh`, then `cd apps/ios/Arbos && fastlane test` (no signing). CI does the same in `.github/workflows/swift-core-check.yml` on pushes/PRs that touch the Rust client crates, the xcframework script, Cargo lockfiles, `packages/ArbosFFI`, or `apps/ios/Arbos`. TestFlight / archive lanes require the same package products; `.github/workflows/ios.yml` builds them before `fastlane archive` / `beta` and fails if they are missing.
- **Version tie-in:** ship the xcframework under the **same release tag** as the Mac app (workspace `version`). Bump them together; do not publish an FFI binary from a different commit than the Mac build it pairs with.
- **Size:** the device `libarbos_ffi.a` is roughly **50–60 MiB** before linking (Loro dominates). The `archive` lane reports the shipped `ArbosFFI.framework` size (prints the measured byte size and writes `apps/ios/Arbos/build/arbosffi-size.txt`); re-check after each release build.

The relay never parses Loro bytes. It stores opaque **rows** and optional **checkpoints**, and forwards them on WebSocket rooms.

---

## 1. Roles

- **Device client** — Mac (`arbos-mac`) or iPhone (`apps/ios/Arbos`). Holds a local Loro replica per chat, dials one WebSocket per room, and calls HTTP for registry, blobs, auth, and machine control.
- **Box** (`arbos-agentd`) — the user's cloud machine. Same room protocol as a device; it alone writes assistant streaming, tools, and most project-doc maps. One box serves every project of its user.
- **Relay** — internet-facing store and fan-out. Opaque rows, checkpoints, REST, wake/liveness. No Loro import.

---

## 2. Storage model

Each **room** is named `{projectId}/{chatId}` (`wire::room_name`). Special chat id `project` (`PROJECT_DOC_CHAT_ID`) is the **project doc** room for that project.

The relay keeps, per room:

- an append-only sequence of **rows** (opaque Loro update bytes), each with `seq`, `device`, `batchId`
- an optional **checkpoint**: snapshot bytes, `checkpointSeq` (highest seq covered), and a **frontier** (encoded Loro version vector)
- `headSeq`, `seqFloor` (oldest retained row after compaction)

Clients and the box keep a local replica: Loro doc + last applied `cursor` (seq) + pending unacked PUSH batches.

### 2.3 Cursor and snapshot together

On the box (`arbos-agentd` `store.rs`), each room's `cursor`, `snapshot`, and `acked_vv` are stored in one row (`room_docs`) and must be committed together. Clients that persist a replica (Mac on-device store, iOS store, `arbosctl` room JSON) follow the same rule: never advance the saved cursor without the matching snapshot (or equivalent import).

---

## 6. Loro documents

Clients decode documents additively: unknown part kinds and extra map keys are ignored or rendered as empty. Schema version is `1` (`SCHEMA_VERSION`).

### 6.1 Two doc kinds

| Room | Chat id | Root containers (typical) |
| --- | --- | --- |
| Chat transcript | any chat ULID (or people-chat id under project slot `people`) | `meta`, `messages`, `commands` |
| Project doc | `project` | `settings`, `status`, `threads`, `memory`, `chats` (box-written index) |

### 6.2 Chat document

Layout (shared intent across core, Mac, iOS):

```text
meta:      Map { chatId, projectId, schemaVersion, epoch }
messages:  List<Map { id, role, sender, status, parts, … }>
commands:  List<Map { id, kind, payload, issuedBy, issuedAt, status }>
```

**`meta`** — written only by the box via `ChatDoc::init_meta` in `arbos-core`. Fields: `chatId`, `projectId`, `schemaVersion` (i64 `1`), `epoch` (i64). Clients open an empty doc and read `meta` from sync (Mac: `ChatDoc::empty`; iOS: `CoreChatDoc` / ArbosFFI).

**`messages[]`** — each entry is a map. Common fields:

| Field | Type | Notes |
| --- | --- | --- |
| `id` | string | ULID |
| `role` | string | `user` \| `assistant` \| `system` \| `project` |
| `sender` | string | e.g. `user`, `agent`, `agent:<projectId>`, `project:<id>`, `routine:<id>` |
| `status` | string | `queued` \| `streaming` \| `done` \| `error` \| `cancelled` |
| `deviceId` | string | authoring device / box id |
| `createdAt` / `completedAt` | i64 ms | |
| `parts` | List\<Map\> | ordered parts |
| `inReplyTo` | string? | assistant reply link |
| `replyTo` | string? | user reply link to a row **in this chat** (the quote header) |
| `replyRef` | Map? | **Reply in DM** (cross-chat reply): `{ chatId, projectId ("people" for DMs/groups), messageId, chatTitle, sender, senderName, senderHandle, text }` — the address of the quoted row plus a ≤ 240-char snapshot, so a reader who is not in the source chat still sees the quote. Written by `ChatDoc::set_reply_ref` (core `ReplyRef`); clients draw "Replying to `senderName` in `chatTitle`" + the quote, and make it a link only when the viewer is a member of `chatId`. agentd quotes the snapshot in the user turn. A row carries `replyTo` **or** `replyRef`, never both. |
| `activity` | string? | plain-words working line while streaming |
| `waitingUser` | bool? | open question |
| `answersQuestionId` | string? | user answer row |
| `reactions` | Map | CRDT: flat `"emoji\|senderId" → true`. Readers project to `emoji → { senderId → true }` via `project_reactions`. Core writers create an empty map at message creation. |
| `voice` / `callId` / `greeting` | various | voice lines, call system lines, D82 greeting |

**Reactions shape.** Writers store flat keys under `reactions` (`"👍|user:ken" → true`) so concurrent first reacts on the **same** emoji merge as map-key inserts. An empty `reactions` map is created with the message (`append_user_message`, `append_system_line`, `append_greeting`, `append_done_message`, `start_assistant_message`, `append_user_answer`, `append_system_note`) so peers do not race on creating that container. `set_reaction(..., on=false)` is a no-op when the `reactions` map is missing. UI / serde still see `emoji → { sender → true }` after `project_reactions` (also folds legacy nested emoji-bucket maps).

**Loro container create race.** When two peers concurrently `insert_container` under the **same** key, one container wins and the **losing container’s contents are dropped**. Do not rely on “usually survive” for nested bucket creates; the flat reaction keys avoid that race for same-emoji first-react on rows that already have an empty `reactions` map.

**Legacy shapes readers accept.** Core (and the Swift client via `CoreChatDoc`) still read docs written by older builds. The committed snapshot `crates/arbos-core/fixtures/chat_doc_legacy.bin` (expected: `chat_doc_legacy_expected.json`) pins each shape so a later reader change cannot silently drop them:

| Shape | Fixture row id |
| --- | --- |
| Old Swift-client user row (`chatId`, `pending`, `waitingUser: false`, empty `hiddenParts`, `replyTo`, `mentions`, nested `attachment`) | `msg-ios-user` |
| Pre-0.6.64 row with **no** `reactions` map | `msg-no-reactions` |
| `turn` as a plain string | `msg-turn-string` |
| `turn` as a `{ "kind": … }` object | `msg-turn-object` |
| Flat attachment part (no nested `attachment` wrapper) | `msg-flat-attach` |
| Text part whose `text` is a plain string (not `LoroText`) | `msg-plain-text` |
| Agent `status` (`waiting_user`) next to an open `question` part | `msg-status-question` |

Nested emoji-bucket reactions (also pre-flat-key) are pinned on `msg-legacy-react` in `chat_doc.bin`. Mac placeholder rows (`chatId` / `pending` / `hiddenParts` on every message) are pinned in `mac_placeholder_rows.bin`.

**Pre-0.6.64 rows.** Message rows written before 0.6.64 have **no** `reactions` map (see `msg-no-reactions` above). The first concurrent react on such a row can still lose one side’s container, because `set_reaction(..., on=true)` creates the map when it is missing — the same Loro create race as nested buckets.

**Rollout (flat keys).** Builds before 0.6.64 do not understand flat `"emoji|sender"` keys. An older Mac drops the whole row: `Message.reactions` in `crates/arbos-mac/src/model.rs` is a strict nested map and `read_message` ends in `.ok()`. Older Swift client builds simply do not show those reactions.

**`callId` on system lines.** Core and Mac write optional `callId` (never `call_id`) on system lines. The Swift client ignores `callId` and renders the text only.

**Client write rule:** a user message and its `send` command land in **one commit** (core: `transact` around `append_user_message` + `push_command("send", …)`; Mac and iOS both use core writers through the same `transact` batching). Streaming text is `LoroText` append-only; the box batches commits on a ~120 ms tick (`STREAM_COMMIT_MS`).

**`parts[]`** — each part has `id`, `kind`, and kind-specific fields. Known kinds written or decoded today:

| `kind` | Shape (summary) | Who writes |
| --- | --- | --- |
| `text` | `text: LoroText` (or string on some paths) | client (user), box (assistant) |
| `reasoning` | `reasoning: LoroText` | box |
| `tool` | `call { id, name, input }`, `outputSummary` (≤ **160** chars, `TOOL_OUTPUT_SUMMARY_MAX`), optional `outputRef` / `outputBytes`, `isError`, usually `hidden: true` | box |
| `attachment` | `attachment { name, mime, ref, bytes?, kind?, title? }` | client or box; bytes at blob URL |
| `status` | `code`, `message` — e.g. `working`, `done`, `failed`, `stopped`, `waiting_user`, `waiting_approval`, `note` | box (`append_system_note` uses `status`+`note`; neither Mac nor iPhone renders that shape) |
| `error` | `code`, `message` | box |
| `question` | `questionId`, `question`, optional `widget`, `state` (`open`/`answered`), `answer` | box; client answers via command |
| `approval` | `approvalId`, title/summary/risk/payload, `state`, timestamps | box (`arbos-core`); Mac model knows the shape |
| `subagent_result` / other drawer-only | often `hidden: true` | box |
| Mac-only decode extras | `diff`, `file`, `browser`, `skills`, … | box / future; iOS maps unknown → empty |

Unknown `kind` values decode to “unknown” and render as nothing (additive rule).

**`commands[]`** — client→box outbox:

| Field | Notes |
| --- | --- |
| `id` | ULID |
| `kind` | `send`, `stop`, `answer`, `retry`, `retitle`, `approve`, `reject`, … |
| `payload` | map/JSON (e.g. `{ "messageId" }` for `send`) |
| `issuedBy` | device id |
| `issuedAt` | ms |
| `status` | `pending` → box marks `taken` → `done` / `failed` (mark-before-execute) |
| `resolution` | optional, set by box |

#### Writer rules (canonical core path)

`arbos-core::doc` is the single canonical chat-doc writer API. Mac and iPhone call core writers for client message / reaction / command / model-settings paths (Mac directly; iPhone through ArbosFFI).

1. **Hidden parts** are inline `hidden: true` on entries in `parts`. `hiddenParts` is a **reader projection** only (`project_hidden_parts` in core). Writers never create a `hiddenParts` list.
2. **Only the box writes `meta`.** Clients read it from sync (Mac relay mode and iOS). Additive schema changes never bump `schemaVersion`; readers ignore unknown fields and unknown part kinds.
3. **Client writers** (`append_user_message`, `append_system_line`, `append_greeting`, `set_reaction`, `push_command`, `set_model_settings`) do not commit themselves — wrap them in `ChatDoc::transact` (one commit = one PUSH). `transact` is re-entrant: nested calls share one depth counter and only the outermost commits; a drop guard restores depth if `f` panics. `snapshot()` / `export_updates_since()` return empty and do not call Loro `export` while a `transact` is open (Loro treats export as a commit barrier). They write no null placeholders, `chatId`, `pending`, `hiddenParts`, or `call_id`. New message rows get an empty `reactions` map. System lines use a `text` part and optional `callId` only (Mac and iOS already decode that shape; Swift ignores `callId` when rendering).
4. **`push_command` / `append_command`.** `push_command` is the non-committing writer; object payloads are nested `LoroMap` containers (arrays → `LoroList`), not a single JSON blob value. `append_command` is `push_command` then commit (honors an open `transact`). Box helpers that used to duplicate the command row now call through this path.
5. **Box helpers** (`append_user_message_from`, `append_command`, `append_system_note`, streaming/tool/question/approval writers, …) still commit inside the helper when no `transact` is open; they honor an outer `transact` (only the outermost commits). `append_system_note` (`status` + `code: "note"`) is unchanged; neither app renders it in the transcript.
6. **`has_greeting()`** is true when any message has `greeting: true` (D82 canned first line).
7. **Mac facts.** Relay-mode Mac does not write `meta` (`ChatDoc::empty`). Mac already uses Loro's random peer, which satisfies R7. Mac system lines are text parts (same as iOS / core `append_system_line`).

### 6.3 Project document

Room `{projectId}/project`. Clients subscribe like any chat room; the box is the primary writer for indexes.

| Root map/list | Purpose |
| --- | --- |
| `settings` | Models and profile mirrors. Clients set model picker fields; box mirrors registry. |
| `status` | Live dots: e.g. `workspace { state, since }`, `box { state, since }` (D70). |
| `threads` | T1 index: map/list of `{ id, name, purpose, summary, status, harness, lastTurnAt }` (`ThreadInfo`). |
| `memory` | Client-visible T1 memory text (e.g. `t1` string). |
| `chats` | Movable list index of open chats (box-only writer in agentd). |

**Model settings (talking model).** Both apps mirror the picker into `settings` **and** PATCH the registry:

- `settings.agentModel` — model id string  
- `settings.agentEffort` — effort string  
- `settings.models.talking_model` — `{ id, effort }`  
- Registry: `PATCH /projects/{id}` body `{ "models": { "talking_model": { "id", "effort" } } }`  

iOS: `ChatDoc.setModelSettings` + `RelayAPI.setModel`. Mac: `ClientCommand::SetModel` in `relay.rs`. Box `write_settings_doc` also writes `defaultSubagentModel`, `decisionModel`, `modelNote`, spend caps, name/icon/color, etc.

`settings.modelNote` — set when a model id was replaced by a default; clients show it on the agent.

### 6.4 Blobs and off-doc bytes

Large bytes do not live in Loro rows:

- **`PUT/GET /blob/{projectId}/{key}`** — attachment and tool-output bodies. Keys are sha256 hex, ULIDs, or similar (`safe_key`). People chats use project slot `people:{chatId}`. Content-Type on PUT is remembered. Used by **Mac and iOS**.
- Tool parts keep ≤160 chars in `outputSummary`; the rest is `outputRef` → blob key.
- **`GET /projects/{id}/history/{chat}`** — older T3 history proxied from the box when awake (409 if not). Related off-doc history; not Loro.

---

## 7. Room WebSocket protocol

Endpoint: **`WS /room/{projectId}/{chatId}`** (query token auth, §10.2). One socket per room per client. Max message size ≈ `ROW_MAX + 8192`.

### 7.1 Frame layout

Binary frame (`arbos_core::wire`; Swift uses the same bytes through ArbosFFI):

```text
[type: u8][headerLen: u32 little-endian][header: UTF-8 JSON][payload: raw bytes]
```

- Header JSON object, max **`HEADER_MAX` = 4096** bytes.
- Payload is opaque (usually Loro update bytes for ROW/PUSH).

### 7.2 Frame types and `PROTO_VERSION`

`PROTO_VERSION` / `protoVersion` = **`1`**. Carried in every `HELLO` (`proto` field). Relay rejects mismatched proto with `ERROR { code: "proto" }`.

| Type | Byte | Direction | Role |
| --- | --- | --- | --- |
| `HELLO` | `0x01` | C→S | Join: `{ cursor, device, room, proto }` |
| `STATE` | `0x02` | S→C | Room summary + frontier payload |
| `ROWS_REQ` | `0x03` | C→S | `{ after, excludeOwn? }` backfill |
| `ROW` | `0x04` | S→C | `{ seq, device, batchId }` + Loro bytes |
| `ROWS_DONE` | `0x05` | S→C | `{ headSeq }` end of catch-up |
| `PUSH` | `0x06` | C→S | `{ batchId }` + Loro bytes |
| `ACK` | `0x07` | S→C | `{ batchId, seq, dup? }` |
| `PRESENCE` | `0x08` | both | Presence / focus / typing |
| `PROBE` / `PROBE_OK` | `0x09` / `0x0a` | C→S / S→C | Liveness / head check |
| `ERROR` | `0x0b` | S→C | `{ code, message }` (e.g. `lagging`, `too_large`) |
| `STATUS` | `0x0c` | S→C | Box lifecycle (§8.2) |
| `WAKE` | `0x0d` | C→S | `{ reason }` ask relay to wake box |

Unknown type bytes: decode returns “skip” (`Ok(None)` / `nil`) so callers ignore forward-compatible types (§7.2).

**Handshake (both apps):** dial → `HELLO` → wait for `STATE` → optional checkpoint fetch (§7.3) → `ROWS_REQ` → replay pending `PUSH`es → live `ROW` / `ACK` / `STATUS` / `PRESENCE`.

`STATE` header fields: `headSeq`, `seqFloor`, `checkpointSeq`, `checkpointSize`, `rowCount`, `rowBytes`, `box` (BoxState). Payload = encoded checkpoint frontier (version vector bytes). Clients accept unknown `box` values and missing `STATE` fields, skipping only the box update so the handshake still continues.

### 7.2a Room session state machine (`arbos-client::room`)

Shared **app** client protocol lives in `RoomSession` (sans-IO: no tokio/tungstenite). The host opens a socket and drives:

| Input | Role |
| --- | --- |
| `on_open(now)` | Send `HELLO`; start STATE deadline |
| `on_frame(bytes)` | Decode wire frames; enqueue actions |
| `on_local_commit(bytes)` | Queue `PUSH` (`batchId` ULID); persist pending |
| `on_tick(now)` | STATE timeout; healthy-session clock |
| `on_close(now)` | Compute backoff; `ScheduleReconnect` |
| `on_checkpoint_skipped` / `on_checkpoint(seq)` / `on_checkpoint_failed` | Host resolution of §7.3 |

| Output action | Host does |
| --- | --- |
| `SendFrame` | Write binary on the socket |
| `FetchCheckpoint { room, frontier, … }` | Containment check or `GET …/checkpoint` |
| `ApplyBytes` | `doc.import` |
| `Cursor` / `Pending` | Persist room cursor and unacked PUSHes |
| `BoxState` / `LinkLive` / `Presence` / `Ack` | UI / registry events |
| `ScheduleReconnect { after }` | Sleep, then dial again |

Room naming for the app session: `people/{chatId}` for people chats, else `{projectId}/{chatId}` (`arbos_client::room_path`, which calls `wire::room_name`). Mac `relay.rs` keeps only WebSocket + HTTP glue and performs actions. Backoff: min 250 ms, doubling, max 16 s, reset after a connection that stayed live ≥ 30 s (§8.6). Wake and presence issued while not Live are held (latest only) across close / reconnect and flushed from `finish_handshake` after `RowsReq` and pending PUSH resend.

### 7.3 Checkpoints

After `STATE`, if `checkpointSeq > 0`:

1. If the local doc **contains** the frontier (`ChatDoc::contains_frontier` / `contains_vv` / Swift `contains(frontier:)`), advance `cursor = max(cursor, checkpointSeq)` — compacted rows are already in the replica.
2. Else **`GET /room/{project}/{chat}/checkpoint`** — body = snapshot bytes; response header `x-arbos-checkpoint-seq`. Import into Loro, then set cursor.

Box (or admin) writes checkpoints: **`POST /room/{project}/{chat}/checkpoint?seqCovered=`** with body = snapshot and header `x-arbos-frontier` = hex-encoded VV. Conflict if `seqCovered` is below the floor.

Also: **`GET /room/{project}/{chat}/stats`** returns the same counters as STATE (no frontier).

### 7.4 `STATUS` frames

Relay broadcasts `STATUS { box, since, detail? }` on box lifecycle changes to rooms of the user’s projects (`0x0c`). Clients update machine/project dots (D70).

### 7.5 Presence and `focused_chat`

Clients send `PRESENCE` periodically (iOS ~15 s) with `device`, `at`, optional user identity, `in_call`, and **`focused_chat`**: the chat id currently on screen while the app is active. The relay suppresses push notifications for that chat while focus is fresh, and may update `last_seen_at`.

### 7.6 Row size cap

`ROW_MAX = 1024 * 1024 - 4096` (~1 MiB minus header budget). Larger PUSH → `ERROR too_large`.

---

## 8. Box lifecycle

The relay tracks one box per user. Clients learn state from `STATE`/`STATUS` and from `GET /machine` / project `box` fields.

### 8.2 Wire `box` values

`BoxState` / iOS `BoxState` (snake_case JSON):

| Value | Meaning |
| --- | --- |
| `awake` | Box sockets / heartbeats live |
| `waking` | Wake job in flight |
| `asleep` | Idle past grace; no live box |
| `error` | Wake failed (S13) or stale detector gave up |

### 8.4 Wake path

When a **device** PUSHes a row into a project room (or people chat that needs agents), or sends `WAKE`, or calls HTTP wake:

1. Record last client push time for the user.
2. `ensure_awake` / `ensure_awake_project` — schedule or run a wake job if not already awake/waking.
3. Provider starts the sandbox / process / docker box.
4. Box dials rooms and `/box/hello` (and related heartbeats).
5. Wake-job runner: **3 attempts** with backoff; then set `error` (S13).

Clients also send `WAKE{reason}` on the project-doc room (Mac/iOS) and/or `POST /projects/{id}/wake`.

### 8.5 Asleep after grace

A sweeper (~5 s) marks the box **asleep** when no box sockets (and no HTTP heartbeat) remain for `box_idle_grace_ms`. If client rows arrived while the box was gone, the sweeper may wake again so commands are not stranded.

### 8.6 Client reconnect

Clients reconnect forever with exponential backoff (0.25 s → 16 s, reset after a session that stayed live ≥ 30 s). Policy and pending-PUSH resend after catch-up live in `RoomSession` (§7.2a); Mac/iOS only sleep and re-dial. Pending PUSH batches are persisted and resent until ACK’d (§11).

---

## 9. Relay HTTP used by clients

Auth: `Authorization: Bearer <device_token>` unless noted. Apps column: **Mac** = `arbos-mac` `relay.rs`; **iOS** = `RelayAPI.swift` (and related). Box-only routes are omitted unless a client also calls them.

### Relay responses

- Every relay response a client reads is a `Dto` in `crates/arbos-core/src/api.rs`. The relay serializes it. The Mac decodes it with serde (`sync/dto.rs` maps it onto model types), and iOS decodes it with `JSONDecoder`.
- After you change a `Dto`, run `./scripts/gen-swift-types.sh` to regenerate `packages/ArbosFFI/Sources/ArbosFFI/RelayTypes.swift`.
- The fixtures in `crates/arbos-relay/tests/fixtures/api` come from the relay's real handlers and builders (`api_fixture_capture.rs`). Both clients decode them in tests: `sync/dto.rs` on the Mac and `CoreFixturesTests.swift` on iOS.
- Change a `Dto` only by addition, so older app builds keep decoding. A new field is optional. Never rename a field or change its type. Keep `null` and an absent key distinct where the relay already does.
- Every relay error body is `ApiErrorDto` (`{"error":{"type","message"}}`); clients branch on `type`, never on message text.
- Free-form JSON stays untyped on purpose: Matrix, request bodies, connector `config`, action `args`, and goal plan and log.

### 9.1 Pairing and session

| Method | Path | Body / response | Apps |
| --- | --- | --- | --- |
| `POST` | `/pair` | `{ code, name? }` → `{ device_id, device_token }` (paircode mode; 410 if GitHub-only) | Mac, iOS |
| `POST` | `/pair/new` | admin → `{ code, expires_in_s }` | ops / self-host |
| `GET` | `/auth/github/start` | browser OAuth start | Mac, iOS (browser) |
| `POST` | `/auth/token` | OAuth code exchange → tokens | Mac, iOS |
| `POST` | `/auth/refresh` | refresh → `{ access_token, refresh_token?, expires_in, user, device_id }` | Mac, iOS |
| `POST` | `/auth/logout` | | Mac, iOS |
| `GET` | `/me` | `{ user, credits, device, machine?, … }` | Mac, iOS |
| `PATCH` | `/me` | profile fields | Mac, iOS |
| `PUT` | `/me/avatar` | image body → avatar URL | Mac, iOS |
| `GET` | `/healthz` | `{ ok, proto, … }` | probes |
| `GET` | `/chats/{id}/health` | room health (#1470): head/floor, checkpoint, stored vs pending rows, replicas, `divergent` + reasons, and `gate` — the newest respond-gate verdicts per message (each agent's reply/silent, the judge's reason, confidence, who the message was for) | members, debug views |
| `GET` | `/admin/room-health/{chat}?n=` | the same block for a people chat with up to `n` (≤ 500) gate decisions | ops |
| `POST` | `/box/people/{chat}/gate` | a box records its agent's verdict on one message `{ message_id, agent_id, handle, decision: reply\|silent, reason, chosen[], confidence, addressee, model, latency_ms, cost_usd }`; 500 kept per chat | agentd |

### 9.2 Registry: projects and chats

| Method | Path | Notes | Apps |
| --- | --- | --- | --- |
| `GET` | `/projects` | list | Mac, iOS |
| `GET` | `/projects/{id}` | `{ project, chats, box, … }` | Mac, iOS |
| `POST` | `/projects` | `{ name, models?, avatar?, …, source_project_id? }` | Mac, iOS |
| `PATCH` | `/projects/{id}` | name, models (`talking_model`), voice, vision, description, … | Mac, iOS |
| `DELETE` | `/projects/{id}` | soft-delete owned agent | Mac, iOS |
| `GET`/`POST` | `/projects/{id}/chats` | list / `{ title }` → `{ chat }` | Mac, iOS |
| `PATCH`/`DELETE` | `/projects/{id}/chats/{chat}` | retitle / close tab | Mac, iOS |
| `POST` | `/projects/{id}/wake` | wake box for project | Mac, iOS |
| `GET` | `/projects/{id}/members` | | Mac, iOS |
| `POST`/`DELETE` | `/projects/{id}/members[/{user}]` | | Mac, iOS |

Default models on create include `talking_model`, `subagent_model`, `decision_model` (see `api::default_models`).

### 9.3 Machine (one box per user)

| Method | Path | Apps |
| --- | --- | --- |
| `GET` | `/machine` | Mac, iOS |
| `POST` | `/machine/start` \| `/stop` \| `/restart` \| `/migrate` \| `/upgrade` | Mac, iOS (subset) |

### 9.4 Rooms over HTTP

| Method | Path | Notes | Apps |
| --- | --- | --- | --- |
| `GET` | `/room/{project}/{chat}/checkpoint` | snapshot + `x-arbos-checkpoint-seq` | Mac, iOS |
| `GET` | `/room/{project}/{chat}/stats` | counters | tooling |
| `POST` | `/room/{project}/{chat}/checkpoint` | box/admin only | box |

Live sync is WebSocket §7, not HTTP.

### 9.5 Blobs

| Method | Path | Notes | Apps |
| --- | --- | --- | --- |
| `PUT` | `/blob/{project}/{key}` | raw body, `Content-Type` | Mac, iOS |
| `GET` | `/blob/{project}/{key}` | bytes + content-type | Mac, iOS |

### 9.6 People chats, calls, push (clients)

Used by both apps for messenger features (not Loro schema, but same relay):

- `GET`/`POST` `/chats`, `POST /chats/dm`, `GET|PATCH|DELETE /chats/{id}`, read/settings/leave/hide, agents/members
- `POST` `/chats/{id}/calls` or `/projects/{pid}/chats/{cid}/calls`; `GET …/call`; `POST /calls/{id}/join|leave`; `GET /calls/{id}`
- `POST|DELETE /devices/{id}/push`, `/devices/{id}/voip` (iOS); `GET /me/unread`, `GET /me/pushes`
- `POST /feedback`, capture helpers

### 9.7 Other client surfaces (Mac and/or iOS)

Secrets/vaults, connectors, skills, channels, routines, goals, voices, local devices — both apps call overlapping subsets (`/secrets`, `/vaults`, `/connectors`, `/skills`, `/voices`, `/goals`, …). Shapes live next to the handlers in `arbos-relay`; they are not part of the Loro chat schema.

The relay also exposes box-only (`/box/…`), admin, proxy, and channel webhook routes that **clients do not** call for sync.

---

## 10. Auth tokens

### 10.1 Device credential storage

- **iOS:** Keychain service `sh.arbos.ios.relay`, keyed by relay URL (`Keychain.swift`). Holds `deviceId` + `deviceToken`.
- **Mac:** OS keychain / credentials module (same idea: token not in the Loro doc).

### 10.2 Presenting the token

HTTP: `Authorization: Bearer <token>`.  
WebSocket dials: same header or **`?token=`** query (`auth::token_from_parts`). Prefer Bearer when the stack allows it; query exists because some WS clients cannot set headers.

---

## 11. PUSH batch ids

Every `PUSH` carries a client-generated **`batchId`** (ULID). The relay appends the row (or detects a duplicate id) and replies `ACK { batchId, seq, dup }`.

Clients:

1. On local Loro update, enqueue `(batchId, bytes)`, persist the pending list.
2. Send `PUSH`; keep the item until `ACK`.
3. On reconnect, resend all pending batches after catch-up. Duplicate `batchId` → `dup: true`, still safe to drop locally.

This is how a message typed offline or across a crash still lands once.

---

## Rules (R)

### R7 — Peer ids and timestamps

`ChatDoc::new(peer: Option<u64>, record_timestamps)`:

- **`peer: None`** — leave Loro's random peer (today's Mac and iPhone behaviour). Never derive a peer from a device id.
- **`peer: Some(id)`** — set that peer. The box and `arbosctl` mint a **fresh peer per boot/run** so a crash between PUSH and persist cannot reuse op ids.
- **`record_timestamps`** — `true` on the box (wall-clock metadata for box ops); `false` on device clients.

Reusing a peer id after a crash that lost unacked ops can fork history. Random peers already satisfy uniqueness for Mac/iOS.

---

## Quick client checklist

1. Authenticate (§10) and load registry (§9.2).
2. Open `WS /room/{project}/{chat}` with `HELLO` / `PROTO_VERSION` (§7).
3. Apply checkpoint if needed (§7.3), then `ROWS_REQ`, then live rows.
4. Write user rows + `commands` in one commit (§6.2); PUSH with `batchId` (§11).
5. Put large files on `/blob/…` (§6.4); reference them from parts.
6. Mirror model picks into project-doc `settings` and `PATCH /projects/{id}` (§6.3).
7. Treat `STATUS` / `/machine` as box liveness (§8); wake when sending to a sleeping box (§8.4).
