# Spec: Matrix DMs & Groups Cutover, Tiered E2EE, and Telegram-Class iPhone Performance

Status: **Proposed engineering spec** (2026-11-10).
Builds directly on open PRs [#2931](https://github.com/unarbos/arbos/pull/2931), [#2934](https://github.com/unarbos/arbos/pull/2934), and [#2935](https://github.com/unarbos/arbos/pull/2935).
Companion docs in this repository: [`docs/crdt-sync.md`](crdt-sync.md), [`docs/self-host.md`](self-host.md), [`docs/mac-design-rules.md`](mac-design-rules.md).

---

## 0. Goal & Core Decisions

Ship **DMs and groups on Matrix (Synapse + `arbos-matrix`) with end-to-end encryption where privacy matters**, keep **servers and channels as they run today**, and fix the **iPhone client bottlenecks** in the same arc so group chats open and scroll at Telegram speed.

1. **Servers & channels stay untouched (unencrypted).** Leave Stage C1 servers/channels running as-is on `arbos-matrix` without enabling `m.room.encryption` on channels.
2. **Tiered encryption by room size:**
   - **DMs and private groups up to `N = 200` members:** End-to-end encrypted (`m.megolm.v1.aes-sha2`) from room creation, backed by the silent wallet-phrase recovery in `#2934` / `#2935`.
   - **Large groups (`> 200` members) and server channels:** Plaintext on Synapse (TLS in transit, disk encryption at rest). Avoids O(devices) Megolm key-share stalls and preserves server search, `?w=` media resizing, and fast joins.
3. **One sync pipe per device.** Moving DMs and groups from Loro `SessionRoom` onto `arbos-matrix` sliding sync retires ~158 per-chat WebSockets, 1 s main-thread room ticks, and 15 s presence loops.
4. **Client speed is co-equal with backend cutover.** Matrix paging + persistent SQLite `EventCache` removes whole-chat decoding, while targeted iPhone UI fixes remove the measured scroll freezes, frame drops, and memory leaks.

---

## 1. Starting Point: Reuse `#2931` + `#2934` + `#2935` (With One Crucial Flip)

Roughly 60% of the client E2EE stack is already implemented and tested against Synapse 1.164 in the open PR stack:

| PR | What it already ships | How this spec uses it |
| --- | --- | --- |
| [#2931](https://github.com/unarbos/arbos/pull/2931) (`deps/rusqlite`) | Bumps workspace `rusqlite` `0.32 -> 0.40` cleanly across `arbos-relay`, `arbos-agentd`, and `arbos-mac` | Merge first; unblocks `matrix-sdk-sqlite` + `bundled-sqlite` on iOS and macOS |
| [#2934](https://github.com/unarbos/arbos/pull/2934) (`matrix/e2ee`) | Enables `e2e-encryption`, `sqlite`, `bundled-sqlite` in `crates/arbos-matrix`; Keychain-encrypted per-device SQLite store; sliding-sync `e2ee` / `to_device` / `account_data` extensions; `Backup` + `recover(passphrase)` (`BackupDownloadStrategy::OneShot`); locked-row placeholder (`rows::LOCKED`) that swaps in-place on key arrival; `send(room, text, parts, id)` | Keep all `crates/arbos-matrix`, `arbos-ffi`, and `arbos-mac` crypto/store changes; **change the relay target** (below) |
| [#2935](https://github.com/unarbos/arbos/pull/2935) (`ios/wallet`) | Silently generates one 12-word `arbos-wallet` phrase on the first device, stores it in synchronizable iCloud Keychain (`ensureKeys`), and auto-recovers Matrix room keys on subsequent Apple devices with zero prompts | Keep `ensureKeys` + iCloud Keychain passphrase flow as the "Invisible Crypto" recovery layer |

### The inversion in `#2934` to change before flipping `ARBOS_MATRIX_E2EE`

In `#2934`, `crates/arbos-relay/src/matrix/spaces.rs:291` attaches `m.room.encryption` to **new server channels**, while `crates/arbos-relay/src/matrix/worker.rs:441` (`Worker::create_room`) leaves **people chats (DMs and groups)** unencrypted on Loro dual-write.

**Change:**
- Keep `spaces.rs` channel creation **unencrypted** (unless a channel is explicitly marked private-encrypted in a future server tier).
- Put `m.room.encryption` (`m.megolm.v1.aes-sha2`) on **new DMs and private groups (`<= 200` members)** created for Matrix-cutover rooms, and stop plaintext Loro dual-write on encrypted rooms (the relay cannot and must not forge sender ciphertext).

---

## 2. Room Tiers & Encryption Rules

| Property | Private Tier (DM & Group `<= 200`) | Large Group Tier (`> 200`) & Channels |
| --- | --- | --- |
| Matrix `m.room.encryption` | **Enabled** in `initial_state` (`m.megolm.v1.aes-sha2`) | **Off** |
| Relay visibility | Metadata & membership only; message bodies and `life.arbos.parts` are ciphertext | Full event content |
| History for new devices of existing member | Restored via encrypted server key backup (`MatrixClient::recover` + `OneShot`) | Loaded directly from Synapse `/messages` |
| History for newly added human members | Shared from point of join (`history_visibility: shared` + Megolm session rotation on leave) | Full shared history via `/messages` |
| Media (`m.image`, `m.file`, `m.audio`) | Client generates thumbnail, encrypts asset + thumbnail (`EncryptedFile`), uploads via media proxy | Plain upload; server `?w=` thumbnail resize works |
| Search | On-device SQLite index over decrypted `EventCache` | Server search + local cache |
| Push preview (Phase 1 -> Phase 2) | `"New message"` in Phase 1 -> decrypted in `ArbosNotificationService` in Phase 2 | Full sender + body preview immediately |

### Crossing the `N = 200` threshold

Matrix does not allow removing `m.room.encryption` from an existing room (any downgrade in place would let a compromised homeserver silently strip encryption).
- **Rule:** Private encrypted groups enforce a hard member cap of `200` on `POST /chats/{id}/members`.
- **Upgrade path:** An owner adding members past `200` gets a single confirmation (*"Convert to a large group? New messages won't be end-to-end encrypted so the group stays fast above 200 members."*). Confirming creates a linked plaintext Matrix room (`m.room.tombstone` -> replacement room id in `life.arbos.chat`), migrates the roster, and keeps older encrypted history readable as an archived segment above the upgrade notice.

---

## 3. Client Architecture (`arbos-matrix`, `arbos-ffi`, iOS, Mac)

### 3.1 Unify people chats into `arbos-matrix` sliding sync

Today `crates/arbos-matrix/src/client.rs` requests `SpaceParent` and `SpaceChild` in `SlidingSyncList` and emits `Snapshot { spaces, channels }`.

Extend `arbos-matrix`:
1. Add `(StateEventType::from("life.arbos.chat"), String::new())` and `is_direct` account data to `required_state` in the `"all"` `SlidingSyncList` (`client.rs:272`), using selective range `[0..=50]` for visible top rooms plus growing background ranges or room subscriptions for open rooms.
2. Add `people: Vec<PeopleRoomRow>` to `Snapshot` (`chat_id` from `life.arbos.chat`, `room_id`, `is_encrypted`, `unread_notifications`, `unread_mentions`, `latest_row`, `last_ts`).
3. Expose unified timeline verbs over UniFFI (`crates/arbos-ffi/src/matrix.rs`):
   - `open_room(room_id)` -> returns cached tail immediately (<= 50 rows), then streams live diffs
   - `paginate_back(room_id, limit)` -> loads older chunk via `EventCache` / `/messages`
   - `send(room_id, text, parts, reply_to, local_id)` -> local echo with `local_id` (`life.arbos.id`) before server ack
   - `send_attachment(room_id, path, mime, caption, thumb, local_id)` -> encrypts automatically when `room.is_encrypted()`
   - `react(room_id, event_id, key)` / `redact(room_id, event_id)` / `mark_read(room_id, event_id)` (private read receipts, matching [#2712](https://github.com/unarbos/arbos/pull/2712)).

### 3.2 Replace the custom `HashMap<OwnedRoomId, Timeline>` with persistent `EventCache`

In `#2934`, `crates/arbos-matrix/src/client.rs:78` still holds timelines in an in-memory `Mutex<HashMap<OwnedRoomId, Timeline>>`, so every app launch starts with empty timelines and waits on network round-trips when opening a room.

**Change:**
- Enable `client.event_cache().subscribe()` backed by the same per-device SQLite store (`Crypto::store_dir` in `#2934`).
- Drive open chats from `matrix-sdk-ui::Timeline` (or `RoomEventCache` linked chunks) so:
  - **Cold and warm chat open reads the last 50 decrypted events straight from local SQLite in `< 15 ms`** — zero network wait.
  - Edits (`m.replace`), reactions (`m.annotation`), replies, redactions, and local send echo are aggregated in Rust and delivered to Swift/GPUI as **indexed vector diffs** rather than rebuilding a 2,200-item array.
  - Limited sync gaps automatically shrink stale chunks (`matrix-sdk` PR #4694 behavior) so memory stays flat.

### 3.3 Retire per-chat `SessionRoom` sockets for people chats

Once a people chat is Matrix-native (`chat.matrix_native = true`):
- `AppModel.ensureRoom` skips opening a Loro `SessionRoom` WebSocket for it.
- Only **agent 1:1 project chats** and **`project` status docs** remain on Loro rooms (bounded: ~16 rooms on a heavy 16-project box, keeping the box status channel intact).

---

## 4. Agents & Token Streaming in Encrypted Groups

### 4.1 Where agent encryption keys live

People chats can include project agents (`@_a_<project_id>:arbos.life`). Two options exist; **ship Mode A first** unless strict zero-access on agent rooms is required on day one:

| Mode | How it works | Trade-offs |
| --- | --- | --- |
| **Mode A (Recommended v1): Human-only rooms are zero-access; agent rooms use MSC3202/MSC4190** | Human-only DMs and groups hold **only user device keys** — neither the relay nor Synapse can ever decrypt them. When an agent is added to a room, `@_a_<project>` joins visibly; the relay's `arbos-agents` appservice (MSC3202 + MSC4190 already configured in `scripts/deploy/matrix/`) holds that agent's Olm/Megolm device, runs the respond-gate before waking a sleeping box (`arbos-boxd` suspend stays intact), hands plaintext context to `agentd`, and encrypts the agent's final reply. | Zero box-wake penalty on noisy groups; works cleanly with box suspend; transparent trust boundary (if an agent is in the room, the agent host already sees the prompt). |
| **Mode B (Optional v2): Box-held agent keys** | `arbos-agentd` runs its own `arbos-matrix` SQLite crypto store on `/work`. Relay holds zero keys even for agent rooms. | Every message in a group containing an agent must wake a suspended box just to decrypt and run the respond-gate. |

### 4.2 Ephemeral token streaming (no `m.replace` spam)

Today `arbos-agentd` flushes streaming text every `120 ms` (`STREAM_COMMIT_MS`). Writing 8 encrypted `m.replace` events/sec into Synapse would bloat room history, hammer Postgres, and burn battery re-encrypting/decrypting partial paragraphs.

**Design:**
1. **Start of turn:** Agent posts typing/working state + opens an ephemeral stream id (`turn_id`) routed over a single lightweight per-device relay SSE/WS side-channel (encrypted to the room's current Megolm session key or TLS-scoped to room members when Mode A is active).
2. **During generation (`120 ms` cadence):** Only connected viewers currently watching `chat_id` receive ephemeral deltas; `ChatView` updates `StreamingText` on the single live tail cell. Zero Synapse DB writes.
3. **End of turn:** Agent sends **one** canonical Matrix `m.room.message` carrying `life.arbos.turn_id = turn_id` (plus `life.arbos.parts` for charts, files, approvals, or questions). Clients swap the ephemeral tail bubble for the durable Matrix event in place with no flicker.

---

## 5. iPhone Performance Spec (Required for Telegram-Class Group Speed)

Switching the backend from Loro to Matrix fixes socket sprawl and whole-history downloads, **but does not by itself fix SwiftUI main-thread hitches**. Measured numbers on the iPhone 17 Pro simulator (Release build, `PerfBaselineUITests`, 2,200-row rich group) set the exact client work required in this arc:

| Bottleneck (measured, Release, 2,200-row group) | Current cost | Target | Required change |
| --- | --- | --- | --- |
| **`ComposerDraft` retain cycle** (`apps/ios/Arbos/Arbos/UI/ChatView.swift:1520, 4325`) | **~3.75 MB leaked per group open** (45 -> 90 MB over 24 opens; jetsam risk) | **0 KB leaked** across 50 opens | Capture weakly / nil out `onCaretOrMentions` and `onChange` closures on disappear; add a `leaks` / weak-ref regression test |
| **Unscoped `AppModel` publishes** (`ChatView.swift:92–116`, `HomeView.swift`) | 25 `.onReceive` triggers re-run ~3,700-line `ChatSession` body; `HomeView` re-ranks 8x while hidden under push (~17 ms wasted during open) | **0 body runs** from other chats; **0 `HomeView` runs** while pushed | Per-chat `ChatRoomStore` observable scoped to `chatId`; gate `HomeView` row recomputation when `!path.isEmpty` |
| **Full-history `Transcript.rows` on every open** (`HomeView.swift:130`, `ChatView.swift:1014`) | Walks all 2,200+ messages on every open because `.id(chatId)` wipes `@State` | **< 10 ms** for tail 50 rows | Feed `ChatRoomStore` from `arbos-matrix` `EventCache` tail slice (50 rows) + incremental diffs; cache built `[Row]` in `ChatRoomStore` across back/reopen |
| **History `growWindow` step** (`ChatView.swift:179–191`) | **90–260 ms main-thread freeze** every 250 ms while scrolling up (34–164 hitch ms/s) | **< 5 hitch ms/s**, 0 freezes | Replace `LazyVStack` + sliding `windowMax` (`scrollTo` re-pin) with a **flipped UIKit `UICollectionView` / `UITableViewDiffableDataSource` host** (Element X's `TimelineTableViewController` pattern) + off-main cell height/text sizing and `UICollectionViewDataSourcePrefetching` |
| **Per-row entry layout** (`MentionPillRenderer`, `RowFrameKey`) | **10–15 ms/row** (> 8.3 ms 120 Hz ProMotion budget) | **< 4 ms/row** on main thread | Pre-measure `NSAttributedString` / mention pill frames on background queue inside `ChatRoomStore`; drop `RowFrameKey` preference when not actively tracking |
| **First-open SwiftUI cold tax** | **450–598 ms** first open (+272 ms one-time Swift conformance/reflection + regex compile) | **< 150 ms** first open, **< 90 ms** repeat open | Detached post-launch idle warmup of `CodeHighlight` regexes, `AttributeScope`, and off-screen row cell registration |

---

## 6. Push Notifications on iPhone (`ArbosNotificationService`)

1. **Phase 1 (ships with cutover, zero blocker):** Matches `#2934`. Encrypted rooms wake the device via the relay push gateway with generic fallback `"New message"`; unencrypted large groups and channels continue showing full sender + message previews.
2. **Phase 2 (fast follow):** Move `Crypto::store_dir` into the shared App Group container (`group.com.unarbos.arbos`, SQLite WAL mode with busy timeout) and link a stripped `ArbosFFI` target (or dedicated small `arbos-nse-ffi` static lib without LiveKit/Loro/Mac symbols so RSS stays well under iOS's **24 MB** extension ceiling) to decrypt the single event by `(room_id, event_id)` and populate sender + body + `MessageNotification` avatar.

---

## 7. Phased Rollout & Migration Plan

Every step has an explicit feature flag and rollback path before user history depends on ciphertext:

- **Phase 1 — Land the crypto & SQLite foundation (PRs `#2931`, `#2934` adjusted, `#2935`):**
  - Merge `#2931` (`rusqlite 0.40`).
  - Adjust `#2934` so `ARBOS_MATRIX_E2EE` targets **private people chats (`<= 200`)**, leaving `spaces.rs` server channels plaintext by default. Keep `ARBOS_MATRIX_E2EE=off` in production until all clients carry the build.
  - Ship `#2935`'s silent iCloud Keychain backup/recovery (`ensureKeys`) + Mac login-keychain recovery UI so 100% of upgraded devices establish key backup automatically.
- **Phase 2 — iPhone speed & memory fixes (independent, ship immediately to TestFlight):**
  - Fix `ComposerDraft` 4 MB leak, scope `ChatSessionHost` + pause `HomeView` under push, warm first-open caches, and land the flipped UIKit timeline host behind a debug toggle -> default on once `PerfBaselineUITests` is green.
- **Phase 3 — People chats on `arbos-matrix` + persistent `EventCache` (unencrypted cutover first):**
  - Add `life.arbos.chat` rooms + persistent `EventCache` to `arbos-matrix`.
  - Add ephemeral agent stream side-channel so agent turns in groups write only one final Matrix message.
  - Flip per-user flag (`matrix_people_native = 1`) for internal dogfooders (Arbonites + DMs): apps read/write Synapse via `arbos-matrix` and stop opening Loro `SessionRoom` sockets for people chats. Backfill existing Loro groups into Synapse once, verify parity with `matrix/shadow.rs`. Rollback = flip flag back to `0`.
- **Phase 4 — Turn on E2EE for DMs & private groups (`<= 200`):**
  - Once minimum supported Mac/iPhone versions include Phase 1 + 3, flip `ARBOS_MATRIX_E2EE=on` for newly created DMs and groups `<= 200`.
  - Migrate existing DMs and small groups via transparent room upgrade (`m.room.tombstone` -> encrypted successor room) with old plaintext history still browsable seamlessly above the seam, or keep existing large groups (`> 200` like large community chats) in the unencrypted tier.
- **Phase 5 — Rich E2EE push previews (`ArbosNotificationService`).**

---

## 8. Three Owner Decisions Needed Before Coding Phase 3–4

1. **Private E2EE group size cap (`N`):** Confirm **`N = 200` members** as the cutoff above which a group runs unencrypted (with an explicit owner confirmation when upgrading past `200`).
2. **Agent keys in v1 (Section 4.1):** Confirm **Mode A** for v1 — human-only DMs and groups are strictly zero-access to the relay, while rooms that invite an `@agent` let the `arbos-agents` appservice (MSC3202) hold that agent's key so sleeping boxes don't wake on every group message.
3. **Existing group chats at flip time:** Upgrade existing small groups (`<= 200`) automatically via linked encrypted successor rooms, or apply E2EE to **newly created** DMs/groups first and offer a one-tap *"Turn on end-to-end encryption"* in group settings?

---

## 9. Acceptance Gates

- **Correctness & E2EE (`crates/arbos-matrix/tests/e2ee.rs` + loopback UI tests):**
  - Human DM & `<= 200` group: Synapse `/messages` contains only `m.room.encrypted`; relay DB holds zero plaintext; second device recovers full history via wallet recovery phrase; wrong phrase fails cleanly.
  - Group `> 200` & server channels: created without `m.room.encryption`; `?w=` thumbnails and server search work.
  - Agent in encrypted group: streaming tokens render at `120 ms` without writing intermediate `m.replace` events to Synapse; final turn lands as one encrypted `m.room.message`.
- **iPhone Performance (`PerfBaselineUITests` Release build, 2,200-row rich group):**
  - Warm group open (`open-chat` signpost): **median `< 90 ms`** (down from 183–210 ms).
  - First group open after launch: **`< 150 ms`** (down from 450–598 ms).
  - History scroll hitch ratio: **`< 5 ms/s`** (down from 34–164 ms/s), zero 90+ ms window-step stalls.
  - Memory across 24 alternating group/DM opens: **`< +2 MB` drift** (down from `+45 MB` leak).
  - Open WebSockets on a 40-project + 60-people-chat account: **1 sliding-sync stream** + active project docs (down from ~158 sockets).
