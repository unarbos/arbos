//! Sync rooms (crdt-sync §7, §9): `WS /room/{projectId}/{chatId}`. The relay
//! stores and forwards opaque rows; it never parses Loro bytes.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::broadcast;

use arbos_core::wire::{self, Frame, FrameType, PROJECT_DOC_CHAT_ID};

use crate::auth::Principal;
use crate::boxes;
use crate::people;
use crate::state::{AppState, RoomBroadcast};

const ROWS_BATCH: i64 = 256;
/// How long live rows are held for a socket that has not finished (or started) its catch-up.
const CATCHUP_HOLD: Duration = Duration::from_secs(10);

/// Longest device name a HELLO may carry (`dev:<ulid>` is 30 bytes, `box:<sandbox id>` under 80).
const DEVICE_NAME_MAX: usize = 128;

/// A device may push back history the relay lacks once per connection (`PushHeader.heal`).
const HEALS_PER_CONNECTION: u32 = 1;

/// Rooms rebuilt in full, in order (tests count them).
#[cfg(test)]
static BUILDS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Full rebuild of the room's stored doc (checkpoint ∪ every row). Runs off the socket task via
/// `spawn_blocking` on a cold cache.
fn build_room_doc(state: &AppState, room_name: &str) -> anyhow::Result<crate::state::RoomDocCache> {
    use arbos_core::doc::ChatDoc;
    #[cfg(test)]
    BUILDS.lock().unwrap().push(room_name.to_owned());
    let key = state.db.room_position(room_name)?;
    let mut doc = match state.db.checkpoint_bytes(room_name)? {
        Some((_, bytes)) => ChatDoc::from_snapshot(None, false, &bytes)?,
        None => ChatDoc::new(None, false),
    };
    let mut after = 0;
    loop {
        let rows = state.db.rows_after(room_name, after, None, ROWS_BATCH)?;
        let n = rows.len() as i64;
        for r in rows {
            after = r.seq;
            let _ = doc.import(&r.bytes);
        }
        if n < ROWS_BATCH {
            break;
        }
    }
    let message_ids = doc.message_identities();
    Ok(crate::state::RoomDocCache {
        key,
        doc,
        message_ids,
    })
}

/// Import only rows after the cached head when the head moved; rebuild when the floor moved.
fn advance_room_doc(
    state: &AppState,
    room_name: &str,
    cached: &mut crate::state::RoomDocCache,
) -> anyhow::Result<()> {
    let key = state.db.room_position(room_name)?;
    if cached.key == key {
        return Ok(());
    }
    if cached.key.1 != key.1 || key.0 < cached.key.0 {
        *cached = build_room_doc(state, room_name)?;
        return Ok(());
    }
    let mut after = cached.key.0;
    let mut imported = false;
    let before = cached.doc.frontiers();
    loop {
        let rows = state.db.rows_after(room_name, after, None, ROWS_BATCH)?;
        let n = rows.len() as i64;
        for r in rows {
            after = r.seq;
            if cached.doc.import(&r.bytes).is_ok() {
                imported = true;
            }
        }
        if n < ROWS_BATCH {
            break;
        }
    }
    // The mirror of message ids is carried forward by the same diff the push path uses, not
    // rebuilt. Rebuilding costs 15 ms at 2 000 messages and 82 ms at 10 000, and every push
    // moves the head — so a rebuild here would put that cost on the reader after every single
    // message. A diff costs what the change costs.
    if imported {
        let changes =
            cached
                .doc
                .messages_changed(&before, &cached.doc.frontiers(), &cached.message_ids);
        // A mirror that may have drifted is worse than a slow one: an id out of place would
        // make a later push's deletion name the wrong message. Pay the sweep instead.
        cached.message_ids = if changes.unreadable {
            cached.doc.message_identities()
        } else {
            changes.ids_after
        };
    }
    cached.key = key;
    Ok(())
}

/// One admission owns its speculative Loro doc until its row is durably accepted.
/// Readers can rebuild stored state while this value is private; no Loro clone escapes.
struct PendingRoomPush<'a> {
    room: &'a crate::state::Room,
    room_name: &'a str,
    cached: crate::state::RoomDocCache,
    /// What the push does to the messages, or the code it is refused with when that cannot
    /// be known.
    changes: Result<arbos_core::doc::MessageChanges, &'static str>,
    _admission: tokio::sync::MutexGuard<'a, ()>,
}

impl PendingRoomPush<'_> {
    /// The relay's own writes in answer to this push (`push_guard::corrections`), applied to
    /// the speculative doc and returned as the update to store right behind the push. Still
    /// under the admission lock: the next push on any socket is diffed against the corrected
    /// doc. An error here fails the push — nothing is stored and the device sends it again —
    /// because a push stored without its correction would become the state the relay defends.
    fn correct(&mut self, state: &AppState, chat_id: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let now = arbos_core::ids::now_ms();
        // A push that could not be judged was refused before this is reached.
        let Ok(changes) = &self.changes else {
            return Ok(None);
        };
        let fixes = crate::push_guard::corrections(state, chat_id, changes, now)?;
        if fixes.is_empty() {
            return Ok(None);
        }
        let doc = &self.cached.doc;
        let base = doc.oplog_vv();
        doc.transact(|doc| fixes.iter().try_for_each(|fix| fix.apply(doc)))?;
        // Every field already held the value the relay would write (the document skips a
        // field set to the value it has): no change was made, so there is no row to store.
        // Told by the version, not by the bytes — an export of no changes is not empty.
        if doc.oplog_vv() == base {
            return Ok(None);
        }
        let bytes = doc.export_updates_since(&base);
        self.cached
            .message_ids
            .retain(|(id, _)| !fixes.iter().any(|fix| fix.delete && fix.message_id == *id));
        Ok(Some(bytes))
    }

    /// Consuming publication is legal only after a successful nonduplicate append.
    fn commit(self, state: &AppState) -> anyhow::Result<()> {
        publish_room_doc(state, self.room, self.room_name, self.cached, |_| ())
    }
}

/// Publish only durable state, retaining and advancing a reader's newer cache if present.
/// A cold build may finish after another row was appended or another cache was published.
fn publish_room_doc<T>(
    state: &AppState,
    room: &crate::state::Room,
    room_name: &str,
    built: crate::state::RoomDocCache,
    read: impl FnOnce(&crate::state::RoomDocCache) -> T,
) -> anyhow::Result<T> {
    let mut guard = room.doc_cache.lock().unwrap();
    if guard.is_none() {
        *guard = Some(built);
    }
    let cached = guard.as_mut().expect("room doc installed");
    if let Err(error) = advance_room_doc(state, room_name, cached) {
        *guard = None; // a failed incremental refresh must not leave a partial mirror
        return Err(error);
    }
    Ok(read(cached))
}

/// Move the warm doc into one private admission; return it only after durable acceptance.
/// The admission lease serializes message-id preimages through gates and author recording.
async fn push_message_changes<'a>(
    state: &AppState,
    room: &'a crate::state::Room,
    room_name: &'a str,
    payload: &[u8],
    admission: tokio::sync::MutexGuard<'a, ()>,
) -> anyhow::Result<PendingRoomPush<'a>> {
    let cached = room.doc_cache.lock().unwrap().take();
    let mut cached = match cached {
        Some(mut cached) => {
            advance_room_doc(state, room_name, &mut cached)?;
            cached
        }
        None => {
            let state_c = state.clone();
            let name = room_name.to_owned();
            tokio::task::spawn_blocking(move || build_room_doc(&state_c, &name))
                .await
                .map_err(|e| anyhow::anyhow!("room doc rebuild: {e}"))??
        }
    };
    let changes = diff_pushed_row(&mut cached, payload);
    Ok(PendingRoomPush {
        room,
        room_name,
        cached,
        changes,
        _admission: admission,
    })
}

/// The code a push is refused with when the room cannot apply all of it.
const INCOMPLETE: &str = "incomplete";
/// The code a push is refused with when the relay cannot read what it does.
const UNREADABLE: &str = "unreadable";

/// [`push_message_changes`] over a cache already in step with the room's stored rows.
///
/// A push is judged by the diff its import produces, so the import has to be the payload: all
/// of its changes and no others. Anything else has no diff to judge and is an `Err` with the
/// code to refuse it with.
fn diff_pushed_row(
    cached: &mut crate::state::RoomDocCache,
    payload: &[u8],
) -> Result<arbos_core::doc::MessageChanges, &'static str> {
    let before = cached.doc.frontiers();
    match cached.doc.import_exactly(payload) {
        Ok(true) => {}
        Ok(false) => return Err(INCOMPLETE),
        Err(_) => return Err(UNREADABLE),
    }
    let changes =
        cached
            .doc
            .messages_changed(&before, &cached.doc.frontiers(), &cached.message_ids);
    if changes.unreadable {
        return Err(UNREADABLE);
    }
    cached.message_ids = changes.ids_after.clone();
    Ok(changes)
}

/// The answer to a push the room cannot apply in full.
///
/// In a people chat the gates answer first, with the codes a client stays connected on: a
/// device that is refused there keeps its batches and sends them again in order, and every
/// batch after the first builds on the one before it. Otherwise the code is one no client
/// special-cases, so a device redials, and on the new connection its heal carries what the
/// room lacked together with this batch.
///
/// A push the relay cannot read is one no resend and no heal changes: it builds on history
/// the room no longer holds, or it is not an update at all. Its answer is `not_available`,
/// the code every client already stays connected on and waits after, so a device holding
/// such changes neither redials nor sends them again at once.
fn unapplied_frame(
    state: &AppState,
    principal: &Principal,
    room_name: &str,
    batch_id: &str,
    code: &str,
    detail: &str,
) -> Message {
    room_name
        .strip_prefix(&format!("{}/", people::PEOPLE_PROJECT))
        .and_then(|chat_id| people::room_push_refusal(state, principal, chat_id))
        .map(|r| push_refused_frame(&r, batch_id))
        .unwrap_or_else(|| match code {
            UNREADABLE => room_push_refused_frame(
                "not_available",
                "this push cannot be applied as it stands",
                batch_id,
            ),
            INCOMPLETE => room_push_refused_frame(
                INCOMPLETE,
                "this push builds on changes the room does not hold",
                batch_id,
            ),
            _ => room_push_refused_frame(code, detail, batch_id),
        })
}

/// Read the room's stored doc (checkpoint ∪ rows) from the cache the room keeps: head moves
/// import only new rows, and a cold cache is rebuilt in `spawn_blocking`.
///
/// One rebuild per room at a time. Every socket of a room asks for the doc as it catches up, so
/// after a relay restart a 500-member chat used to be rebuilt 500 times at once, each on its own
/// blocking thread with its own copy of the doc. The others now wait and read what the first built.
async fn read_room_doc<T>(
    state: &AppState,
    room: &crate::state::Room,
    room_name: &str,
    read: impl Fn(&crate::state::RoomDocCache) -> T,
) -> anyhow::Result<T> {
    let warm = || -> anyhow::Result<Option<T>> {
        let mut guard = room.doc_cache.lock().unwrap();
        let Some(cached) = guard.as_mut() else {
            return Ok(None);
        };
        advance_room_doc(state, room_name, cached)?;
        Ok(Some(read(cached)))
    };
    if let Some(v) = warm()? {
        return Ok(v);
    }
    let _building = room.doc_build.lock().await;
    if let Some(v) = warm()? {
        return Ok(v);
    }
    let state_c = state.clone();
    let name = room_name.to_owned();
    let built = tokio::task::spawn_blocking(move || build_room_doc(&state_c, &name))
        .await
        .map_err(|e| anyhow::anyhow!("room doc rebuild: {e}"))??;
    publish_room_doc(state, room, room_name, built, read)
}

/// Encoded version vector of the room's stored doc.
async fn room_version_vector(
    state: &AppState,
    room: &crate::state::Room,
    room_name: &str,
) -> anyhow::Result<Vec<u8>> {
    read_room_doc(state, room, room_name, |cached| {
        arbos_core::doc::encode_vv(&cached.doc.oplog_vv())
    })
    .await
}

/// Version vector of the room's stored doc (same cache as [`room_version_vector`]).
async fn room_oplog_vv(
    state: &AppState,
    room: &crate::state::Room,
    room_name: &str,
) -> anyhow::Result<arbos_core::doc::VersionVector> {
    read_room_doc(state, room, room_name, |cached| cached.doc.oplog_vv()).await
}

/// The newest `n` message rows of a room's stored doc (projected JSON, oldest first), from the
/// same cache the version-vector readers use. The link-preview scan reads the tail this way
/// so a busy room is not rebuilt from its rows on every message.
pub async fn room_tail_rows(
    state: &AppState,
    room_name: &str,
    n: usize,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let room = state.room(room_name).await;
    read_room_doc(state, &room, room_name, |cached| {
        let list = cached.doc.messages_list();
        let len = list.len();
        (len.saturating_sub(n)..len)
            .filter_map(|i| cached.doc.projected_row(i, None))
            .collect()
    })
    .await
}

/// Per-socket row ordering (see `run`).
struct Delivery {
    /// True from connect / HELLO until this socket's ROWS_REQ reply ended with ROWS_DONE.
    catching_up: bool,
    /// Highest row seq written to this socket, by either path.
    last_sent_seq: i64,
    /// Live rows that arrived while catching up, in arrival (= seq) order.
    held: Vec<RoomBroadcast>,
    /// History-heal pushes accepted on this socket (`PushHeader.heal`); one per connection.
    heals: u32,
}

impl Default for Delivery {
    fn default() -> Self {
        Self {
            catching_up: true,
            last_sent_seq: 0,
            held: Vec::new(),
            heals: 0,
        }
    }
}

/// How long one frame may wait for its socket to take it.
const SEND_STALL: Duration = Duration::from_secs(if cfg!(test) { 2 } else { 30 });

/// The sending half of a room socket. A peer that has stopped reading (a phone that left the
/// network without closing, a client that hung) lets the kernel's buffers fill, and then a
/// plain `send` waits for as long as the connection exists: the task, its socket and its place
/// in the room were kept for good. A frame that cannot be handed over within [`SEND_STALL`]
/// ends the socket instead; a client that is alive redials and catches up from its cursor.
struct Out {
    sink: futures::stream::SplitSink<WebSocket, Message>,
    stalled: bool,
}

impl Out {
    async fn send(&mut self, m: Message) -> Result<(), axum::Error> {
        match tokio::time::timeout(SEND_STALL, self.sink.send(m)).await {
            Ok(sent) => sent,
            Err(_) => {
                self.stalled = true;
                Err(axum::Error::new(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "the socket took no frame for too long",
                )))
            }
        }
    }
}

async fn flush_held(d: &mut Delivery, sink: &mut Out) -> Result<(), axum::Error> {
    let held = std::mem::take(&mut d.held);
    for b in held {
        if b.seq > d.last_sent_seq {
            d.last_sent_seq = b.seq;
            sink.send(Message::Binary(b.bytes)).await?;
        }
    }
    Ok(())
}

pub async fn ws_room(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    principal: Principal,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    // People chats live under the reserved project slot `people`; their ACL is membership.
    if !state.allows(&principal, &acl_key(&project_id, &chat_id)) {
        return (StatusCode::FORBIDDEN, "not your project").into_response();
    }
    if project_id != people::PEOPLE_PROJECT {
        if state.db.project(&project_id).ok().flatten().is_none() {
            return (StatusCode::NOT_FOUND, "no such project").into_response();
        }
        if chat_id != PROJECT_DOC_CHAT_ID
            && !state.db.chat_exists(&project_id, &chat_id).unwrap_or(false)
        {
            return (StatusCode::NOT_FOUND, "no such chat").into_response();
        }
    }
    let room_name = wire::room_name(&project_id, &chat_id);
    // Headroom above the limit the frame handler enforces, so an oversized push is answered
    // rather than dropped. At `ROW_MAX + 8192` anything past the cap died inside tungstenite,
    // which cannot send a close frame for it — the client saw only "Connection reset without
    // closing handshake" and could not tell a frame it must split from a link that had gone
    // (Jacob, Test5 round 3, 2026-11-04: twenty minutes of once-a-second resets, no diagnosis
    // on either side). The headroom is bounded: a client that ignores `too_large` and keeps
    // growing still loses the socket, it just has to work much harder to get there.
    ws.max_message_size(wire::ROW_MAX * 2)
        .on_upgrade(move |socket| run(socket, state, principal, project_id, room_name))
}

/// What `AppState::allows` checks for a room: the project id, or `people:<chat>` for a people chat.
fn acl_key(project_id: &str, chat_id: &str) -> String {
    if project_id == people::PEOPLE_PROJECT {
        format!("{}{chat_id}", people::PEOPLE_ACL_PREFIX)
    } else {
        project_id.to_owned()
    }
}

fn frame(ty: FrameType, header: serde_json::Value, payload: Bytes) -> Message {
    Message::Binary(Frame::new(ty, header, payload).encode())
}

fn error_frame(code: &str, message: &str) -> Message {
    frame(
        FrameType::Error,
        json!({ "code": code, "message": message }),
        Bytes::new(),
    )
}

/// Room-push refusal: [`error_frame`] plus the `batchId` being refused, so the pusher can drop
/// that batch at once instead of waiting out its ack timeout — a refused batch is never acked,
/// and agentd read that silence as a dead link and reconnected 10 s later (2026-11-04). The
/// people-push refusal below has always carried `batchId`; this is the same courtesy for rooms.
fn room_push_refused_frame(code: &str, message: &str, batch_id: &str) -> Message {
    crate::metrics::room_push_refused(code);
    frame(
        FrameType::Error,
        json!({ "code": code, "message": message, "batchId": batch_id }),
        Bytes::new(),
    )
}

/// People-push refusal: always carries `batchId`; `rate_limited` also carries `retryS`;
/// `not_available` carries `reason` (`blocked_by` / `blocks_member` / `silenced` /
/// `chat_unavailable`) and, on a block, `byHandle` (+ `byName`). The code set never grows:
/// fielded clients only keep a room Live for these two codes.
fn push_refused_frame(refused: &people::PushRefused, batch_id: &str) -> Message {
    crate::metrics::room_push_refused(refused.code());
    let mut header = json!({
        "code": refused.code(),
        "message": refused.message(),
        "batchId": batch_id,
    });
    if let Some(retry_s) = refused.retry_s() {
        header["retryS"] = json!(retry_s);
    }
    if let Some(reason) = refused.reason() {
        header["reason"] = json!(reason);
    }
    if let Some((by_handle, by_name)) = refused.other_party() {
        header["byHandle"] = json!(by_handle);
        if let Some(name) = by_name {
            header["byName"] = json!(name);
        }
    }
    frame(FrameType::Error, header, Bytes::new())
}

/// The close frame a planned restart sends: 1012 Service Restart. Clients treat it as "redial
/// now" rather than a fault (connection pill, #1339).
fn restart_close() -> Message {
    Message::Close(Some(axum::extract::ws::CloseFrame {
        code: axum::extract::ws::close_code::RESTART,
        reason: "service restart".into(),
    }))
}

/// The close frame a revoked device gets: 1008 Policy, which clients treat as a fault rather
/// than "redial now". A redial is refused at the upgrade anyway, because the bearer no longer
/// resolves, and that 401 is the app's existing signed-out path.
fn signed_out_close() -> Message {
    Message::Close(Some(axum::extract::ws::CloseFrame {
        code: axum::extract::ws::close_code::POLICY,
        reason: "signed out".into(),
    }))
}

/// Counts this socket in `RestartSignal::open_sockets` for as long as it lives, so a restart
/// can drain (wait for the 1012 closes to go out) before the process exits.
struct OpenSocket(Arc<crate::state::RestartSignal>);

impl OpenSocket {
    fn new(state: &AppState) -> Self {
        state.restart.open_sockets.fetch_add(1, Ordering::Relaxed);
        Self(state.restart.clone())
    }
}

impl Drop for OpenSocket {
    fn drop(&mut self) {
        self.0.open_sockets.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Drops the room's rebuilt doc when this was the last socket. Declared before the broadcast
/// subscribe so it runs after the subscribe is dropped (reverse drop order).
struct LastSocketDocCacheClear(Arc<crate::state::Room>);

impl Drop for LastSocketDocCacheClear {
    fn drop(&mut self) {
        if self.0.tx.receiver_count() == 0 {
            *self.0.doc_cache.lock().unwrap() = None;
        }
    }
}

async fn run(
    socket: WebSocket,
    state: AppState,
    principal: Principal,
    project_id: String,
    room_name: String,
) {
    let conn_id = state.conn_seq.fetch_add(1, Ordering::Relaxed) + 1;
    // Subscribe before anything else this function awaits, and before the liveness check below.
    // `Sender::subscribe` marks the current value as seen, so every revocation that lands after
    // this line wakes the watch arm, and the check that follows catches any that landed before
    // it — including during the upgrade, where the principal was resolved and then several more
    // awaits ran. The re-review's probe E won exactly that window: reconnecting dialers, which
    // is what the app does when the 1008 close arrives, put sockets past the resolve and into
    // `run` while the revocation committed, and those sockets were deaf to it permanently.
    let mut revocations = state.revocations.subscribe();
    let room = state.room(&room_name).await;
    // Before subscribe: Drop runs after `rx` is gone, so receiver_count excludes this socket.
    let _clear_doc = LastSocketDocCacheClear(room.clone());
    let mut rx = room.tx.subscribe();
    let (sink, mut stream) = socket.split();
    let mut sink = Out {
        sink,
        stalled: false,
    };
    // A planned restart (restart.rs): the socket closes with 1012 so the client redials at once
    // instead of backing off. A socket that arrives mid-drain is told the same thing right away.
    let mut closing = state.restart.closing.subscribe();
    if *closing.borrow_and_update() {
        let _ = sink.send(restart_close()).await;
        return;
    }
    // Sign-out has to reach a socket that is already open. The principal was resolved once, at
    // upgrade, so without this a revoked device keeps reading the room and keeps writing rows
    // that post as its owner until it disconnects of its own accord (#2460 review, finding 1).
    // The subscribe is at the top of this function; this is the other half, for a revocation
    // that beat it.
    if let Principal::Device { id, .. } = &principal {
        if !state.device_is_live(id) {
            let _ = sink.send(signed_out_close()).await;
            return;
        }
    }
    let _open = OpenSocket::new(&state);
    let is_box = principal.is_box();
    // Rows from the box carry the box's device id (`box:<sandbox>` / `box:local-<user>`), the same
    // id agentd writes into the doc, so clients see one stable author across restarts.
    let mut device = match (&principal, principal.user_id()) {
        (Principal::Box { .. }, Some(u)) => state
            .db
            .user_box(u)
            .ok()
            .flatten()
            .map(|b| b.device_id)
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| principal.device_name()),
        _ => principal.device_name(),
    };
    // Box sockets count per user box (one box serves every project of the user).
    let box_user = principal.user_id().map(str::to_owned).unwrap_or_default();
    if is_box {
        boxes::box_connected(&state, &box_user).await;
        room.box_joined(&box_user);
    }
    tracing::debug!(%room_name, %device, conn_id, "room join");

    // In-order delivery per socket (the lost-first-turn bug): a live ROW must never reach a
    // client before the rows below it that the client is still fetching with ROWS_REQ. Until the
    // client's catch-up reply is complete, live rows are held; afterwards they are flushed in seq
    // order and anything at or below the last seq already sent is dropped as a duplicate. A
    // client that never asks for rows gets the held ones after `CATCHUP_HOLD`.
    let mut delivery = Delivery::default();
    let hold_deadline = tokio::time::sleep(CATCHUP_HOLD);
    tokio::pin!(hold_deadline);

    loop {
        tokio::select! {
            incoming = stream.next() => {
                let Some(Ok(msg)) = incoming else { break };
                let data = match msg {
                    Message::Binary(b) => b,
                    Message::Close(_) => break,
                    _ => continue,
                };
                let f = match Frame::decode(data) {
                    Ok(Some(f)) => f,
                    Ok(None) => continue,
                    Err(e) => {
                        let _ = sink.send(error_frame("bad_frame", &e.to_string())).await;
                        continue;
                    }
                };
                // A frame already in flight when the device was revoked would race the
                // revocation arm below, so every frame asks the rows again: nothing is read
                // and nothing is written for a device the account signed out.
                if let Principal::Device { id, .. } = &principal {
                    if !state.device_is_live(id) {
                        tracing::info!(%room_name, device = %id, conn_id, "room frame refused: device signed out");
                        let _ = sink.send(signed_out_close()).await;
                        break;
                    }
                }
                let was_hello = f.ty == FrameType::Hello;
                match handle_frame(&state, &room, &project_id, &room_name, conn_id, &mut device, &principal, f, &mut sink, &mut delivery).await {
                    Ok(()) => {}
                    Err(e) => {
                        tracing::warn!(%room_name, error = %e, "room frame error");
                        if sink.stalled {
                            break;
                        }
                        let _ = sink.send(error_frame("internal", &e.to_string())).await;
                    }
                }
                if was_hello {
                    hold_deadline.as_mut().reset(tokio::time::Instant::now() + CATCHUP_HOLD);
                }
                if !delivery.catching_up
                    && !delivery.held.is_empty()
                    && flush_held(&mut delivery, &mut sink).await.is_err()
                {
                    break;
                }
            }
            out = rx.recv() => {
                match out {
                    Ok(b) => {
                        if b.from_conn == conn_id {
                            continue;
                        }
                        if b.seq == 0 {
                            if sink.send(Message::Binary(b.bytes)).await.is_err() {
                                break;
                            }
                        } else if delivery.catching_up {
                            delivery.held.push(b);
                        } else if b.seq > delivery.last_sent_seq {
                            delivery.last_sent_seq = b.seq;
                            if sink.send(Message::Binary(b.bytes)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Rows were missed on the channel: the client re-syncs from the store,
                        // and until it does, nothing live may jump ahead of that re-sync.
                        delivery.catching_up = true;
                        delivery.held.clear();
                        let _ = sink.send(error_frame("lagging", "re-sync with ROWS_REQ")).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = &mut hold_deadline, if delivery.catching_up => {
                // No ROWS_REQ came: release held rows rather than starving the socket.
                delivery.catching_up = false;
                if flush_held(&mut delivery, &mut sink).await.is_err() {
                    break;
                }
            }
            changed = closing.changed() => {
                if changed.is_err() || *closing.borrow_and_update() {
                    let _ = sink.send(restart_close()).await;
                    break;
                }
            }
            changed = revocations.changed() => {
                if changed.is_err() {
                    break;
                }
                // Some device was revoked. The counter does not say which, so ask the rows.
                if let Principal::Device { id, .. } = &principal {
                    if !state.device_is_live(id) {
                        tracing::info!(%room_name, device = %id, conn_id, "room socket closed: device signed out");
                        let _ = sink.send(signed_out_close()).await;
                        break;
                    }
                }
            }
        }
    }
    tracing::debug!(%room_name, %device, conn_id, "room leave");
    if let Principal::Device { id, .. } = &principal {
        if let Some((_, chat_id)) = room_name.rsplit_once('/') {
            crate::notify::clear_focus(&state, id, chat_id).await;
        }
    }
    if is_box {
        room.box_left(&box_user);
        boxes::box_disconnected(&state, &box_user).await;
    }
    // `_clear_doc` (declared before `rx`) drops after `rx`, clearing the doc cache when this
    // was the last socket.
}

#[allow(clippy::too_many_arguments)]
async fn handle_frame(
    state: &AppState,
    room: &crate::state::Room,
    project_id: &str,
    room_name: &str,
    conn_id: u64,
    device: &mut String,
    principal: &Principal,
    f: Frame,
    sink: &mut Out,
    delivery: &mut Delivery,
) -> anyhow::Result<()> {
    let is_box = principal.is_box();
    match f.ty {
        FrameType::Hello => {
            delivery.catching_up = true;
            let h: wire::Hello = serde_json::from_value(f.header)?;
            if h.proto != arbos_core::PROTO_VERSION {
                sink.send(error_frame("proto", "unsupported protocol version"))
                    .await?;
                return Ok(());
            }
            // The name is the client's own and is kept per room (`room_health`) and on every
            // row it pushes: a frame-sized one is refused, not stored.
            if h.device.len() > DEVICE_NAME_MAX {
                sink.send(error_frame("bad_hello", "device name too long"))
                    .await?;
                return Ok(());
            }
            if !h.device.is_empty() {
                *device = h.device.clone();
            }
            crate::room_health::note_hello(room, device, h.cursor);
            send_state(state, project_id, room_name, sink).await?;
        }
        FrameType::RowsReq => {
            let r: wire::RowsReq = serde_json::from_value(f.header)?;
            let exclude = if r.exclude_own {
                Some(device.as_str())
            } else {
                None
            };
            let mut after = r.after;
            delivery.catching_up = true;
            loop {
                let rows = state.db.rows_after(room_name, after, exclude, ROWS_BATCH)?;
                let n = rows.len() as i64;
                for row in rows {
                    after = row.seq;
                    delivery.last_sent_seq = delivery.last_sent_seq.max(row.seq);
                    sink.send(frame(
                        FrameType::Row,
                        json!({ "seq": row.seq, "device": row.device, "batchId": row.batch_id }),
                        Bytes::from(row.bytes),
                    ))
                    .await?;
                }
                if n < ROWS_BATCH {
                    break;
                }
            }
            // `headSeq` is the last row this socket was actually walked past, not the room head:
            // a row appended between the store read and now is broadcast live and arrives next,
            // and a client must not move its cursor beyond what it has been sent.
            delivery.catching_up = false;
            crate::room_health::note_caught_up(room, device, after);
            // The payload is the relay's version vector for the room (checkpoint ∪ rows): a
            // device compares it with its own doc and pushes back what the relay lacks
            // (`PushHeader.heal`), so one complete device heals a room whose rows were lost.
            let relay_vv = room_version_vector(state, room, room_name)
                .await
                .unwrap_or_default();
            sink.send(frame(
                FrameType::RowsDone,
                json!({ "headSeq": after }),
                Bytes::from(relay_vv),
            ))
            .await?;
        }
        FrameType::Push => {
            let p: wire::PushHeader = serde_json::from_value(f.header)?;
            if f.payload.len() > wire::ROW_MAX {
                // Named, sized, and attributed to its batch. Without the `batchId` the pusher
                // cannot tell which push was refused, so it leaves the batch in flight and
                // kills a healthy session on the ack timeout ten seconds later.
                tracing::warn!(
                    %room_name, %device, bytes = f.payload.len(), limit = wire::ROW_MAX,
                    "push refused: too large for one frame"
                );
                sink.send(room_push_refused_frame(
                    "too_large",
                    &format!(
                        "push is {} bytes; one frame holds {}. Split it and send the parts in order.",
                        f.payload.len(),
                        wire::ROW_MAX
                    ),
                    &p.batch_id,
                ))
                .await?;
                return Ok(());
            }
            if p.heal {
                if delivery.heals >= HEALS_PER_CONNECTION {
                    tracing::warn!(%room_name, %device, "history heal refused: one per connection");
                    sink.send(error_frame("heal_limit", "one history heal per connection"))
                        .await?;
                    return Ok(());
                }
                delivery.heals += 1;
                crate::room_health::note_heal(room, device, f.payload.len());
                tracing::info!(
                    %room_name, %device, bytes = f.payload.len(), batch_id = %p.batch_id,
                    "history heal: a device pushed ops the relay lacked"
                );
            }
            // Serialize the existing duplicate checks with every prepared admission.
            let admission = room.push_admission.lock().await;
            // A batch id the room already stored is a dup before the people gates — no rate
            // event, no store, no broadcast.
            if let Some(seq) = state.db.row_seq_for_batch(room_name, &p.batch_id)? {
                drop(admission);
                sink.send(frame(
                    FrameType::Ack,
                    json!({ "batchId": p.batch_id, "seq": seq, "dup": true }),
                    Bytes::new(),
                ))
                .await?;
                return Ok(());
            }
            // Covered pushes are duplicates: ops the room already holds (often a reconnect
            // resending refused heals as ordinary pushes). ACK with dup, skip gates and store.
            if let Ok(vv) = room_oplog_vv(state, room, room_name).await {
                if !p.heal && arbos_core::doc::update_covered_by(&f.payload, &vv) {
                    let seq = state.db.room_state(room_name).map(|s| s.head).unwrap_or(0);
                    drop(admission);
                    sink.send(frame(
                        FrameType::Ack,
                        json!({ "batchId": p.batch_id, "seq": seq, "dup": true }),
                        Bytes::new(),
                    ))
                    .await?;
                    return Ok(());
                }
                // Known from the bytes alone, so the room's doc is left as it is.
                if arbos_core::doc::update_skips_ahead(&f.payload, &vv) {
                    drop(admission);
                    sink.send(unapplied_frame(
                        state,
                        principal,
                        room_name,
                        &p.batch_id,
                        INCOMPLETE,
                        "",
                    ))
                    .await?;
                    return Ok(());
                }
            }
            // What this row does to the chat's messages. Until now the arm checked size,
            // duplicate batch ids and the people gates, and nothing about content — so three
            // client-written things decided authorisation: whose message a delete touched,
            // whose name an insert carried, and whether the push was a heal. All three are now
            // read from the diff and from the relay's own record of who announced what.
            let chat_of_room = room_name
                .rsplit_once('/')
                .map(|(_, c)| c)
                .unwrap_or(room_name);
            let mut pending =
                push_message_changes(state, room, room_name, &f.payload, admission).await?;
            // A push that cannot be judged is refused like one that was judged and refused,
            // for every principal.
            let check = match &pending.changes {
                Ok(changes) => crate::push_guard::check_push(
                    state,
                    principal,
                    project_id,
                    chat_of_room,
                    p.heal,
                    changes,
                ),
                Err(code) => crate::push_guard::PushCheck::Refuse {
                    code,
                    detail: "this push cannot be applied as it stands".to_owned(),
                },
            };
            let (carries_new_message, attributed) = match check {
                crate::push_guard::PushCheck::Allow {
                    carries_new_message,
                    attributed,
                } => (carries_new_message, attributed),
                crate::push_guard::PushCheck::Refuse { code, detail } => {
                    drop(pending); // discard private speculation and release admission before I/O
                    tracing::warn!(
                        %room_name, %device, code, detail = %detail, batch_id = %p.batch_id,
                        "room push refused"
                    );
                    let refusal =
                        unapplied_frame(state, principal, room_name, &p.batch_id, code, &detail);
                    sink.send(refusal).await?;
                    return Ok(());
                }
            };
            // The one notion of kind for the rest of the arm. The header asks to be treated as
            // history; the diff decides whether it is. A push carrying a message the relay has
            // no record of is a new message however the header is spelled, and from here on it
            // is handled as one — gated, unfurled, notified — so the flag changes nothing a
            // client could want it to change.
            let is_heal = p.heal && !carries_new_message;
            let mut wake_owners = Vec::new();
            // A heal carries other members' history, not a message of this device: the
            // message gates (rate limit, blocks, member pushes) do not apply — the socket's ACL
            // already does. 20:42 Oct 23: the first production heal (60 KB, Arbonites) was
            // dropped because a member had blocked the healing user.
            if is_heal {
                // nothing to gate or wake
            } else if let Some(chat_id) =
                room_name.strip_prefix(&format!("{}/", people::PEOPLE_PROJECT))
            {
                match people::on_room_push(state, principal, chat_id) {
                    Ok(owners) => wake_owners = owners,
                    Err(r) => {
                        drop(pending);
                        sink.send(push_refused_frame(&r, &p.batch_id)).await?;
                        return Ok(());
                    }
                }
            }
            // The push and the relay's answer to it are one write: stored together or not at
            // all, before the admission lock is released.
            let correction = pending.correct(state, chat_of_room)?;
            let correction_id = arbos_core::ids::ulid();
            let ids = |rows: &[arbos_core::doc::MessageIdentity]| -> Vec<String> {
                rows.iter().map(|(id, _)| id.clone()).collect()
            };
            let (inserted, removed) = pending
                .changes
                .as_ref()
                .map(|c| (ids(&c.inserted), ids(&c.removed)))
                .unwrap_or_default();
            // Whose message each new row carries is recorded from the socket that pushed it,
            // with the row. The announce routes write an author record too, but they take
            // the message id from a request body and are gated only on membership — so that
            // record says who announced an id, never that the relay holds a row for it
            // (`stored`). A message this push took out is remembered as deleted, so it cannot
            // be pushed back under the same id (`push_guard::corrections`).
            let (seq, dup, correction_seq) = state.db.append_row_with(
                room_name,
                device,
                &p.batch_id,
                &f.payload,
                &crate::db::RowEffects {
                    answer: correction
                        .as_deref()
                        .map(|bytes| (crate::notices::RELAY_DEVICE, correction_id.as_str(), bytes)),
                    chat_id: chat_of_room,
                    authors: &attributed,
                    stored: &inserted,
                    deleted: &removed,
                },
            )?;
            crate::metrics::room_push(dup);
            // Broadcast while the admission lock is still held: the push and the relay's row
            // behind it leave in seq order, with no other push's row between or ahead of
            // them. A receiver drops a row whose seq is not above the last one it sent on, so
            // a correction overtaken by a later row would never arrive.
            if !dup {
                let mut rows = vec![(
                    seq,
                    device.as_str(),
                    p.batch_id.as_str(),
                    f.payload.clone(),
                    conn_id,
                )];
                if let (Some(seq), Some(bytes)) = (correction_seq, correction) {
                    rows.push((
                        seq,
                        crate::notices::RELAY_DEVICE,
                        correction_id.as_str(),
                        Bytes::from(bytes),
                        0,
                    ));
                }
                for (seq, device, batch_id, bytes, from_conn) in rows {
                    let row = Frame::new(
                        FrameType::Row,
                        json!({ "seq": seq, "device": device, "batchId": batch_id }),
                        bytes,
                    )
                    .encode();
                    let _ = room.tx.send(RoomBroadcast {
                        from_conn,
                        bytes: row,
                        seq,
                    });
                }
            }
            if dup {
                drop(pending); // another stored row, not these speculative bytes
            } else {
                pending.commit(state)?;
            }
            sink.send(frame(
                FrameType::Ack,
                json!({ "batchId": p.batch_id, "seq": seq, "dup": dup }),
                Bytes::new(),
            ))
            .await?;
            if !dup {
                if !is_heal {
                    // A link in a finished message gets its preview card (#2064), off the
                    // socket task.
                    if let Some((_, chat_id)) = room_name.rsplit_once('/') {
                        crate::unfurl::on_row(state, project_id, chat_id, is_box);
                    }
                }
                if is_heal {
                    // History, not a new message: no read-cursor move, no notification, no
                    // wake. The Matrix mirror resync picks the rows up on its own.
                } else if !is_box {
                    let mut chat_ref = None;
                    // A person wrote without announcing a message (older apps): the fallback
                    // notifies the other members by name only.
                    if let Principal::Device { user_id, .. } = principal {
                        let chat_id = room_name
                            .rsplit_once('/')
                            .map(|(_, c)| c.to_owned())
                            .unwrap_or_default();
                        let chat = if project_id == people::PEOPLE_PROJECT {
                            Some(crate::notify::ChatRef::people(&chat_id))
                        } else if chat_id != PROJECT_DOC_CHAT_ID {
                            Some(crate::notify::ChatRef::project(project_id, &chat_id))
                        } else {
                            None
                        };
                        if let Some(chat) = chat {
                            state.matrix.touch_rows(
                                state,
                                &chat.chat_id,
                                chat.project_id.as_deref(),
                            );
                            // A person's own row is read by definition: their per-user cursor
                            // moves to it, so no device of theirs sees it as unread.
                            let _ = state.db.advance_user_cursor(user_id, &chat.chat_id, seq);
                            crate::notify::on_row_from_device(state, chat.clone(), user_id, seq);
                            // Outside agents in this chat (#2140): announced by the app in a
                            // moment, else read from the doc.
                            crate::outside::on_row(
                                state,
                                chat.clone(),
                                user_id,
                                arbos_core::ids::now_ms(),
                            );
                            chat_ref = Some(chat);
                        }
                    }
                    // Wake obligation: a client row for a sleeping box (crdt-sync §8.4 step 1–2),
                    // and a box with no socket on this room is told to open it (#2120). A
                    // people chat with shared agents reaches every agent's box.
                    match &chat_ref {
                        Some(chat) => {
                            let agents = match &chat.project_id {
                                None => wake_owners,
                                // An outside agent's chat (#2140): the relay hosts it, no box.
                                Some(pid) if crate::outside::is_outside(state, pid) => Vec::new(),
                                Some(pid) => state
                                    .user_of(pid)
                                    .map(|owner| {
                                        vec![boxes::ServingAgent {
                                            project_id: pid.clone(),
                                            owner,
                                        }]
                                    })
                                    .unwrap_or_default(),
                            };
                            boxes::after_person_row(state, room, chat, &agents).await;
                        }
                        None if crate::outside::is_outside(state, project_id) => {}
                        None => {
                            if let Some(u) = state.user_of(project_id) {
                                state
                                    .last_client_push
                                    .lock()
                                    .await
                                    .insert(u, arbos_core::ids::now_ms());
                            }
                            boxes::ensure_awake_project(state, project_id, "send").await;
                            if room_name.rsplit_once('/').map(|(_, c)| c)
                                == Some(PROJECT_DOC_CHAT_ID)
                            {
                                boxes::after_project_doc_row(state, room, project_id);
                            }
                        }
                    }
                }
            }
        }
        FrameType::Presence => {
            // `focused_chat` (crdt-sync §7.5): the chat on this device's screen; its message
            // notifications are suppressed while the presence stays fresh.
            if let Principal::Device { id, .. } = principal {
                let focused = f.header.get("focused_chat").and_then(|v| v.as_str());
                crate::notify::set_focus(state, id, focused).await;
                // docs/box-scaling.md §4.2: a chat on screen asks its cold box back.
                if focused.is_some() {
                    crate::boxd::prefetch_for_project(state, project_id).await;
                    // §4.2: and wakes a sleeping one while its node has room to spare.
                    if project_id != people::PEOPLE_PROJECT {
                        crate::boxd::wake_for_focus(state, project_id).await;
                    }
                }
                // §4.2: typing in an agent's chat wakes its sleeping box ahead of the message.
                if f.header.get("typing").and_then(|v| v.as_bool()) == Some(true)
                    && project_id != people::PEOPLE_PROJECT
                {
                    crate::boxd::wake_for_typing(state, project_id).await;
                }
                // Presence counts as "seen" (last_seen_at / online). Stamped in memory here;
                // `flush_presence` writes every device's stamp in one transaction.
                let now = arbos_core::ids::now_ms();
                state.presence_seen.lock().await.insert(id.clone(), now);
                if let Some(user) = principal.user_id() {
                    state.user_seen.lock().unwrap().insert(user.to_owned(), now);
                }
            }
            let out = Frame::new(FrameType::Presence, f.header, f.payload).encode();
            let _ = room.tx.send(RoomBroadcast {
                from_conn: conn_id,
                bytes: out,
                seq: 0,
            });
        }
        FrameType::Probe => {
            let st = state.db.room_state(room_name)?;
            sink.send(frame(
                FrameType::ProbeOk,
                json!({ "headSeq": st.head }),
                Bytes::new(),
            ))
            .await?;
        }
        FrameType::Wake => {
            let w: wire::WakeHeader =
                serde_json::from_value(f.header).unwrap_or(wire::WakeHeader {
                    reason: "open".into(),
                });
            // An outside agent's room never wakes the owner's box (#2140).
            if !crate::outside::is_outside(state, project_id) {
                boxes::ensure_awake_project(state, project_id, &w.reason).await;
            }
            let (st, since) = state.project_box_state(project_id).await;
            sink.send(frame(
                FrameType::Status,
                json!({ "box": st, "since": since }),
                Bytes::new(),
            ))
            .await?;
        }
        FrameType::State
        | FrameType::Row
        | FrameType::RowsDone
        | FrameType::Ack
        | FrameType::ProbeOk
        | FrameType::Error
        | FrameType::Status => {
            // Server-to-client types; a client sending them is ignored (never fatal).
        }
    }
    Ok(())
}

async fn send_state(
    state: &AppState,
    project_id: &str,
    room_name: &str,
    sink: &mut Out,
) -> anyhow::Result<()> {
    let st = state.db.room_state(room_name)?;
    let (box_state, _) = state.project_box_state(project_id).await;
    let header = wire::State {
        head_seq: st.head,
        seq_floor: st.floor,
        checkpoint_seq: st.checkpoint_seq,
        checkpoint_size: st.checkpoint_size,
        row_count: st.row_count,
        row_bytes: st.row_bytes,
        box_state,
    };
    sink.send(frame(
        FrameType::State,
        serde_json::to_value(header)?,
        Bytes::from(st.checkpoint_frontier),
    ))
    .await?;
    Ok(())
}

/// `GET /room/{project}/{chat}/checkpoint` — full checkpoint bytes (clients load it, then rows after `seq`).
pub async fn get_checkpoint(
    State(state): State<AppState>,
    principal: Principal,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    if !state.allows(&principal, &acl_key(&project_id, &chat_id)) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let room_name = wire::room_name(&project_id, &chat_id);
    match state.db.checkpoint_bytes(&room_name) {
        Ok(Some((seq, bytes))) => {
            ([("x-arbos-checkpoint-seq", seq.to_string())], bytes).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Least time between two checkpoint attempts by one caller on one room, and after a stored
/// checkpoint between it and anyone's next attempt: an attempt reads the whole room, and a room
/// that has just been folded needs no second folding. A caller's refused attempt holds back
/// only that caller, so one box with a bad replica cannot keep a shared chat from compacting.
const CHECKPOINT_MIN_INTERVAL: Duration = Duration::from_secs(30);

/// `POST /room/{project}/{chat}/checkpoint?seqCovered=` — box only; body = snapshot.
///
/// The body is evidence, not content: it must hold every row it covers, which is how a box with
/// an incomplete replica learns it. What is stored is the room's own doc (checkpoint ∪ rows), so
/// a checkpoint adds nothing: every op in it came through a push.
pub async fn post_checkpoint(
    State(state): State<AppState>,
    principal: Principal,
    Path((project_id, chat_id)): Path<(String, String)>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
    body: Bytes,
) -> Response {
    if !state.allows(&principal, &acl_key(&project_id, &chat_id))
        || !matches!(principal, Principal::Box { .. } | Principal::Admin)
    {
        return (StatusCode::FORBIDDEN, "box only").into_response();
    }
    let seq_covered: i64 = q
        .get("seqCovered")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    // As on the socket: a project's ACL covers every chat id under it, so the chat has to
    // exist before the relay keeps anything in memory for its room.
    if project_id != people::PEOPLE_PROJECT {
        if state.db.project(&project_id).ok().flatten().is_none() {
            return (StatusCode::NOT_FOUND, "no such project").into_response();
        }
        if chat_id != PROJECT_DOC_CHAT_ID
            && !state.db.chat_exists(&project_id, &chat_id).unwrap_or(false)
        {
            return (StatusCode::NOT_FOUND, "no such chat").into_response();
        }
    }
    let room_name = wire::room_name(&project_id, &chat_id);
    let device = principal.device_name();
    let room = state.room(&room_name).await;
    {
        let recent = |t: &std::time::Instant| t.elapsed() < CHECKPOINT_MIN_INTERVAL;
        let mut tried = room.checkpoint_tried.lock().unwrap();
        tried.callers.retain(|_, t| recent(t));
        if tried.stored.as_ref().is_some_and(recent) || tried.callers.contains_key(&device) {
            crate::metrics::room_checkpoint("too_soon");
            return (StatusCode::TOO_MANY_REQUESTS, "checkpoint_too_soon").into_response();
        }
        tried
            .callers
            .insert(device.clone(), std::time::Instant::now());
    }
    let (st, name) = (state.clone(), room_name.clone());
    let stored = tokio::task::spawn_blocking(move || {
        store_own_checkpoint(&st, &project_id, &chat_id, &name, seq_covered, &body)
    })
    .await
    .map_err(anyhow::Error::from)
    .and_then(|r| r);
    let (result, response) = match stored {
        Ok(Ok(true)) => {
            room.checkpoint_tried.lock().unwrap().stored = Some(std::time::Instant::now());
            ("stored", StatusCode::NO_CONTENT.into_response())
        }
        Ok(Ok(false)) => (
            "below_floor",
            (StatusCode::CONFLICT, "seqCovered below floor").into_response(),
        ),
        Ok(Err((candidate, why))) => {
            // `candidate` says whose snapshot fell short: the box's upload (its replica is
            // incomplete) or the room's own doc (the relay holds a row it cannot apply, and
            // the room does not compact until that is repaired).
            tracing::warn!(
                %room_name, %device, seq_covered, candidate, %why,
                "checkpoint refused: the snapshot lacks ops the relay holds"
            );
            (
                candidate,
                (
                    StatusCode::CONFLICT,
                    format!("checkpoint_incomplete: {why}"),
                )
                    .into_response(),
            )
        }
        Err(e) => (
            "error",
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        ),
    };
    crate::metrics::room_checkpoint(result);
    response
}

/// The blocking half of [`post_checkpoint`]. `Ok(Err((candidate, why)))` names the snapshot that
/// does not cover the room's rows: `upload_incomplete` or `room_incomplete`. The upload is
/// checked first, so a body that proves nothing costs no rebuild of the room.
fn store_own_checkpoint(
    state: &AppState,
    project_id: &str,
    chat_id: &str,
    room_name: &str,
    seq_covered: i64,
    upload: &[u8],
) -> anyhow::Result<Result<bool, (&'static str, String)>> {
    if let CheckpointVerdict::Incomplete(why) =
        checkpoint_covers_store(&state.db, room_name, seq_covered, upload)?
    {
        return Ok(Err(("upload_incomplete", why)));
    }
    let Some(own) = crate::matrix::backfill::load_doc(state, chat_id, Some(project_id))? else {
        return Ok(Err(("room_incomplete", "the room holds no doc".to_owned())));
    };
    let (bytes, frontier) = (own.snapshot(), arbos_core::doc::encode_vv(&own.oplog_vv()));
    // The room's own snapshot faces the same check as the upload: a row its doc could not apply
    // is not in it, and storing it would delete that row.
    if let CheckpointVerdict::Incomplete(why) =
        checkpoint_covers_store(&state.db, room_name, seq_covered, &bytes)?
    {
        return Ok(Err(("room_incomplete", why)));
    }
    Ok(Ok(state.db.put_checkpoint(
        room_name,
        seq_covered,
        &frontier,
        &bytes,
    )?))
}

/// Outcome of [`checkpoint_covers_store`].
#[derive(Debug, PartialEq, Eq)]
pub enum CheckpointVerdict {
    /// The snapshot holds every op the relay has for `seq <= seq_covered`.
    Covers,
    /// It does not; the reason names what is missing.
    Incomplete(String),
}

/// A checkpoint replaces the rows it covers, so it must contain every op in them. Boxes posted
/// snapshots of incomplete replicas (two agents of one owner share a device id, so each replica
/// skipped the other's rows — #1409) and the relay deleted 194 messages of a 446-message group
/// chat on 2026-10-23 (Arbonites). The relay can parse the doc (`load_doc` does for export and
/// the Matrix mirror), so it checks before it deletes anything:
///
/// 1. `seq_covered` may not exceed the room head (nothing can cover rows that do not exist);
/// 2. the candidate must contain the previous checkpoint's version vector;
/// 3. importing every stored row with `seq <= seq_covered` into the candidate must apply nothing
///    new and park nothing as pending — i.e. the candidate already had every op, with its deps.
pub fn checkpoint_covers_store(
    db: &crate::db::Db,
    room: &str,
    seq_covered: i64,
    snapshot: &[u8],
) -> anyhow::Result<CheckpointVerdict> {
    use arbos_core::doc::ChatDoc;

    let st = db.room_state(room)?;
    if seq_covered > st.head {
        return Ok(CheckpointVerdict::Incomplete(format!(
            "seqCovered {seq_covered} is above the room head {}",
            st.head
        )));
    }
    let mut candidate = match ChatDoc::from_snapshot(None, false, snapshot) {
        Ok(d) => d,
        Err(e) => {
            return Ok(CheckpointVerdict::Incomplete(format!(
                "snapshot does not load: {e}"
            )))
        }
    };
    let mut after = 0;
    if let Some((prev_seq, prev_bytes)) = db.checkpoint_bytes(room)? {
        let prev = ChatDoc::from_snapshot(None, false, &prev_bytes)?;
        if !candidate.contains_vv(&prev.oplog_vv()) {
            return Ok(CheckpointVerdict::Incomplete(format!(
                "missing ops of the current checkpoint (seq {prev_seq})"
            )));
        }
        after = prev_seq;
    }
    let mut before = candidate.oplog_vv();
    let mut missing_rows = 0usize;
    let mut first_missing: Option<i64> = None;
    loop {
        let rows = db.rows_between(room, after, seq_covered, ROWS_BATCH)?;
        let n = rows.len() as i64;
        for row in rows {
            after = row.seq;
            let applied_or_parked = match candidate.import(&row.bytes) {
                Ok(status) => {
                    let now = candidate.oplog_vv();
                    let applied = now != before;
                    before = now;
                    status.pending.is_some() || applied
                }
                // A row that does not decode cannot be in any snapshot, so no candidate can ever
                // cover it and this arm refuses every one of them — correct for this function's
                // purpose, and on its own a permanent wedge for the room. The exit is
                // `repair_undecodable_rows`, which takes the row out of `rows` altogether. Making
                // this arm `false` instead would reopen the hole the whole function exists to close:
                // it would let a candidate that is genuinely missing ops delete the rows holding
                // them.
                Err(_) => true,
            };
            if applied_or_parked {
                missing_rows += 1;
                first_missing.get_or_insert(row.seq);
            }
        }
        if n < ROWS_BATCH {
            break;
        }
    }
    if missing_rows > 0 {
        return Ok(CheckpointVerdict::Incomplete(format!(
            "{missing_rows} stored row(s) from seq {} are not in the snapshot",
            first_missing.unwrap_or_default()
        )));
    }
    Ok(CheckpointVerdict::Covers)
}

/// What [`repair_undecodable_rows`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RoomRepair {
    /// Seqs moved out of `rows` and into `quarantine`.
    pub quarantined: Vec<i64>,
    /// The seq the relay's own checkpoint now covers, when one was written.
    pub checkpoint_seq: Option<i64>,
}

/// Reopen compaction for a room wedged by a row the relay cannot decode: move those rows to
/// `quarantine`, then write a checkpoint covering their seqs.
///
/// **Both halves are required, and the checkpoint is the half that is easy to miss.** Moving the
/// row only unblocks the *relay*: a box that failed to import it is holding it as owed, and the
/// box's compaction gate has exactly one exit, a checkpoint covering that seq. The box is the
/// ordinary producer of checkpoints, so a closed gate cannot clear itself, and a re-pushed copy of
/// the bad row lands *above* any existing checkpoint, so nothing already written clears it either.
/// The relay is the only actor that can break that cycle, which is why the repair ends here rather
/// than leaving a box to notice.
///
/// The candidate is built by the relay from its own rows, as `notices::compact_shallow` does, but it
/// is a **full** snapshot and it is still put through [`checkpoint_covers_store`] before anything is
/// deleted. Skipping that guard would be defensible on the same grounds `compact_shallow` skips it,
/// and it is not worth it: running the repair's own candidate through the same check every box
/// candidate faces is what keeps this path from becoming a second way to lose a room's history.
pub fn repair_undecodable_rows(
    state: &AppState,
    project_id: Option<&str>,
    chat_id: &str,
) -> anyhow::Result<RoomRepair> {
    use arbos_core::doc::ChatDoc;

    let room_name = wire::room_name(project_id.unwrap_or(people::PEOPLE_PROJECT), chat_id);
    let head = state.db.room_state(&room_name)?.head;
    let mut repair = RoomRepair::default();

    // Which rows cannot be imported, decided exactly as the validation walk decides it: an update
    // whose dependencies are missing parks as pending and returns `Ok`, so only a genuine decode
    // failure reaches the `Err` arm. Accumulating one scratch doc therefore cannot manufacture a
    // failure that the walk would not also see.
    let mut scratch = ChatDoc::new(None, false);
    let mut after = 0i64;
    loop {
        let rows = state.db.rows_between(&room_name, after, head, ROWS_BATCH)?;
        let n = rows.len() as i64;
        for row in rows {
            after = row.seq;
            if scratch.import(&row.bytes).is_err() {
                repair.quarantined.push(row.seq);
            }
        }
        if n < ROWS_BATCH {
            break;
        }
    }
    if repair.quarantined.is_empty() {
        return Ok(repair);
    }
    for seq in &repair.quarantined {
        state.db.quarantine_row(&room_name, *seq)?;
        tracing::warn!(
            %room_name, seq, "row quarantined: it does not decode, and it was blocking compaction"
        );
    }

    let Some(doc) = crate::matrix::backfill::load_doc(state, chat_id, project_id)? else {
        return Ok(repair);
    };
    let bytes = doc.snapshot();
    anyhow::ensure!(!bytes.is_empty(), "empty snapshot");
    let frontier = arbos_core::doc::encode_vv(&doc.oplog_vv());
    match checkpoint_covers_store(&state.db, &room_name, head, &bytes)? {
        CheckpointVerdict::Covers => {}
        CheckpointVerdict::Incomplete(why) => {
            anyhow::bail!("repair candidate does not cover the room's rows: {why}")
        }
    }
    anyhow::ensure!(
        state
            .db
            .put_checkpoint(&room_name, head, &frontier, &bytes)?,
        "head below floor"
    );
    repair.checkpoint_seq = Some(head);
    Ok(repair)
}

/// `POST /room/{project}/{chat}/repair` — admin only. Reopens compaction for a room wedged by an
/// undecodable row.
///
/// Operator-triggered rather than automatic, deliberately. The relay failing to decode a row is not
/// proof the row is garbage: apps and boxes ship their own Loro builds, so bytes this binary cannot
/// read may be a newer encode version that a peer reads fine. Quarantining on the relay's own
/// verdict is therefore a judgement about version skew, and this keeps a human making it. The bytes
/// are retained either way (`Db::quarantined_rows`).
pub async fn post_room_repair(
    State(state): State<AppState>,
    principal: Principal,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    if !matches!(principal, Principal::Admin) {
        return (StatusCode::FORBIDDEN, "admin only").into_response();
    }
    let room_name = wire::room_name(&project_id, &chat_id);
    match repair_undecodable_rows(&state, Some(&project_id), &chat_id) {
        Ok(r) => {
            // What is still held, so "the bytes are retained" is answerable rather than asserted.
            let retained: Vec<_> = state
                .db
                .quarantined_rows(&room_name)
                .unwrap_or_default()
                .into_iter()
                .map(|q| json!({ "seq": q.seq, "batchId": q.batch_id, "bytes": q.bytes.len() }))
                .collect();
            axum::Json(json!({
                "quarantined": r.quarantined,
                "checkpointSeq": r.checkpoint_seq,
                "retained": retained,
            }))
            .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /room/{project}/{chat}/stats`.
pub async fn room_stats(
    State(state): State<AppState>,
    principal: Principal,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    if !state.allows(&principal, &acl_key(&project_id, &chat_id)) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let room_name = wire::room_name(&project_id, &chat_id);
    match state.db.room_state(&room_name) {
        Ok(st) => axum::Json(json!({
            "headSeq": st.head, "seqFloor": st.floor, "checkpointSeq": st.checkpoint_seq,
            "checkpointSize": st.checkpoint_size, "rowCount": st.row_count, "rowBytes": st.row_bytes,
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod refusal_frame_tests {
    use super::*;

    /// Jacob, Test5, 2026-11-04: the relay refused a room push at 14:04:46.870 and the pusher
    /// could not tell which batch it meant, so it waited out its ack timeout and declared the
    /// link dead at 14:04:56.918. The refusal names its batch so the pusher can drop it at
    /// once. `batchId` is the contract agentd's `on_refused` reads; the other error codes are
    /// not about one batch and must stay as they are.
    #[test]
    fn a_room_push_refusal_names_the_batch_it_refused() {
        let decoded = |m: Message| match m {
            Message::Binary(b) => Frame::decode(b).unwrap().unwrap().header,
            other => panic!("not a binary frame: {other:?}"),
        };

        let h = decoded(room_push_refused_frame(
            "not_yours",
            "01M6M2KVEGM6QN5K37QYYKTNVF is not yours to delete",
            "01BATCHTOREFUSE0000000000",
        ));
        assert_eq!(h["code"], "not_yours");
        assert_eq!(h["batchId"], "01BATCHTOREFUSE0000000000");
        assert!(h["message"].as_str().unwrap().contains("not yours"));

        // A refusal is the only error about one batch: `lagging` asks for a re-sync and the
        // rest are about the connection, so a pusher must not drop a batch on them.
        let h = decoded(error_frame("lagging", "re-sync with ROWS_REQ"));
        assert!(h.get("batchId").is_none(), "{h}");
    }
}

/// How often presence stamps reach `devices.last_seen_at`.
///
/// `online` does not wait for it: `AppState::seen_at` answers from the in-memory stamp first.
/// The column is what survives a restart and what `last_seen_at` shows, floored to the minute.
const PRESENCE_FLUSH: Duration = Duration::from_secs(60);

/// Write every presence stamp newer than `since` to `devices.last_seen_at`, all in one
/// transaction (2,500 sockets used to be 2,500 single-row writes a minute, each one queued
/// ahead of some room's PUSH). Returns how many devices were written.
pub async fn flush_presence(state: &AppState, since: i64) -> usize {
    let seen: Vec<(String, i64)> = state
        .presence_seen
        .lock()
        .await
        .iter()
        .filter(|(_, at)| **at > since)
        .map(|(id, at)| (id.clone(), *at))
        .collect();
    if seen.is_empty() {
        return 0;
    }
    // A failed flush loses nothing that matters: the next frame stamps the device again.
    if let Err(e) = state.db.touch_devices(&seen) {
        tracing::warn!(error = %e, devices = seen.len(), "presence flush failed");
    }
    seen.len()
}

/// Grace-period sweeper: a box socket that stays gone marks the project asleep (crdt-sync §8.5).
/// Also the presence flush ([`flush_presence`]).
pub async fn spawn_sweeper(state: AppState) {
    let presence = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            boxes::sweep_box_liveness(&state).await;
        }
    });
    tokio::spawn(async move {
        let mut since = 0;
        loop {
            tokio::time::sleep(PRESENCE_FLUSH).await;
            // Taken before the flush reads the map, so a stamp that lands meanwhile is
            // written by the next one rather than skipped.
            let now = arbos_core::ids::now_ms();
            flush_presence(&presence, since).await;
            since = now;
        }
    });
}

#[cfg(test)]
mod ordering_tests {
    //! The lost-first-turn bug (2026-10-19): a socket that connects while another peer is
    //! pushing could receive a live ROW before the lower rows its ROWS_REQ was still fetching;
    //! clients that treat "seq ≤ cursor" as a duplicate, or that reconnect with that cursor,
    //! then never see the lower rows and the chat looks empty / stuck streaming. Invariant: on
    //! any one socket, row seqs arrive strictly increasing with no gaps from `after + 1` to the
    //! last one the writer produced.

    use axum::routing::get;
    use axum::Router;
    use futures::{SinkExt, StreamExt};
    use serde_json::json;
    use std::net::SocketAddr;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as TMsg;

    use arbos_core::ids::{now_ms, ulid};
    use arbos_core::wire::{Frame, FrameType};

    use crate::people::tests_support::{person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;
    use crate::{auth::Principal, db::Chat, state::AppState};

    async fn serve(state: AppState) -> SocketAddr {
        let app = Router::new()
            .route("/room/{project}/{chat}", get(super::ws_room))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    /// A device whose raw token we know (legacy `adv_` shape: the relay stores its hash).
    fn device_with_token(state: &AppState, user: &str) -> (Principal, String) {
        let id = ulid();
        let token = format!("adv_{}", ulid().to_ascii_lowercase());
        state
            .db
            .insert_device(&id, "t", &arbos_core::auth::hash(&token), user, "paircode")
            .unwrap();
        (
            Principal::Device {
                id,
                token: token.clone(),
                user_id: user.to_owned(),
            },
            token,
        )
    }

    fn decode(m: TMsg) -> Option<Frame> {
        match m {
            TMsg::Binary(b) => Frame::decode(b).ok().flatten(),
            _ => None,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rows_reach_a_catching_up_socket_in_order_without_gaps() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "Order");
        let (_, writer_token) = device_with_token(&state, &alice);
        let (_, reader_token) = device_with_token(&state, &alice);
        let addr = serve(state.clone()).await;
        const ROWS: i64 = 40;
        const ITERATIONS: usize = 40;

        for it in 0..ITERATIONS {
            let chat = ulid();
            state
                .db
                .insert_chat(&Chat {
                    id: chat.clone(),
                    project_id: project.clone(),
                    title: "t".into(),
                    is_pinned: false,
                    position: 0,
                    created_at: now_ms(),
                    doc_epoch: 1,
                    kind: "chat".into(),
                    participants: Vec::new(),
                    forked_from: None,
                })
                .unwrap();
            let url = |tok: &str| format!("ws://{addr}/room/{project}/{chat}?token={tok}");

            // Writer: connects, catches up, then pushes ROWS rows as fast as the relay acks.
            let (wsw, _) = tokio_tungstenite::connect_async(url(&writer_token))
                .await
                .unwrap();
            let (mut wsink, mut wstream) = wsw.split();
            wsink
                .send(TMsg::Binary(
                    Frame::new(
                        FrameType::Hello,
                        json!({ "cursor": 0, "device": "dev:w", "room": format!("{project}/{chat}"), "proto": arbos_core::PROTO_VERSION }),
                        bytes::Bytes::new(),
                    )
                    .encode(),
                ))
                .await
                .unwrap();
            // A few rows exist before the reader arrives; the rest race its catch-up.
            let head_before_reader = 3 + (it as i64 % 5);
            let (reader_go_tx, reader_go) = tokio::sync::oneshot::channel::<()>();
            let doc = arbos_core::doc::ChatDoc::new(Some(5), false);
            let payloads: Vec<Vec<u8>> = (1..=ROWS)
                .map(|n| {
                    let mark = doc.oplog_vv();
                    doc.init_meta("c", "p", n);
                    doc.export_updates_since(&mark)
                })
                .collect();
            let writer = tokio::spawn(async move {
                let mut pushed = 0i64;
                let mut reader_go = Some(reader_go_tx);
                let push_frame = |n: i64| {
                    let payload = payloads[n as usize - 1].clone();
                    TMsg::Binary(
                        Frame::new(
                            FrameType::Push,
                            json!({ "batchId": format!("b{n}") }),
                            bytes::Bytes::from(payload),
                        )
                        .encode(),
                    )
                };
                // Push one row, wait for its ack, push the next (keeps the stream realistic).
                while let Some(Ok(m)) = wstream.next().await {
                    let Some(f) = decode(m) else { continue };
                    match f.ty {
                        FrameType::State => {
                            let req = Frame::new(
                                FrameType::RowsReq,
                                json!({ "after": 0, "excludeOwn": true }),
                                bytes::Bytes::new(),
                            )
                            .encode();
                            wsink.send(TMsg::Binary(req)).await.unwrap();
                        }
                        FrameType::RowsDone => {
                            pushed += 1;
                            wsink.send(push_frame(pushed)).await.unwrap();
                        }
                        FrameType::Ack => {
                            if pushed == head_before_reader {
                                if let Some(go) = reader_go.take() {
                                    let _ = go.send(());
                                }
                            }
                            if pushed < ROWS {
                                pushed += 1;
                                wsink.send(push_frame(pushed)).await.unwrap();
                            } else {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                pushed
            });
            // The reader arrives once a few rows exist and the writer is still pushing.
            let _ = reader_go.await;
            let (wsr, _) = tokio_tungstenite::connect_async(url(&reader_token))
                .await
                .unwrap();
            let (mut rsink, mut rstream) = wsr.split();
            rsink
                .send(TMsg::Binary(
                    Frame::new(
                        FrameType::Hello,
                        json!({ "cursor": 0, "device": "dev:r", "room": format!("{project}/{chat}"), "proto": arbos_core::PROTO_VERSION }),
                        bytes::Bytes::new(),
                    )
                    .encode(),
                ))
                .await
                .unwrap();
            let mut seqs: Vec<i64> = Vec::new();
            let mut rows_done = false;
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
            // Both conditions, not just the rows. `RowsDone` is sent after the last row of a
            // catch-up batch, so stopping at the last row can stop one frame too early: when
            // the writer finishes before the reader's ROWS_REQ is answered, every row arrives
            // as catch-up and `RowsDone` is the frame after the one that ends this loop.
            while seqs.last().copied() != Some(ROWS) || !rows_done {
                let m = tokio::time::timeout_at(deadline, rstream.next()).await;
                let Ok(Some(Ok(m))) = m else {
                    panic!("iteration {it}: reader timed out with {seqs:?}, rows_done={rows_done}");
                };
                let Some(f) = decode(m) else { continue };
                match f.ty {
                    FrameType::State => {
                        let req = Frame::new(
                            FrameType::RowsReq,
                            json!({ "after": 0, "excludeOwn": true }),
                            bytes::Bytes::new(),
                        )
                        .encode();
                        rsink.send(TMsg::Binary(req)).await.unwrap();
                    }
                    FrameType::Row => {
                        let seq = f.header["seq"].as_i64().unwrap();
                        seqs.push(seq);
                    }
                    FrameType::RowsDone => rows_done = true,
                    _ => {}
                }
            }
            let pushed = writer.await.unwrap();
            assert_eq!(pushed, ROWS);
            assert!(rows_done, "iteration {it}: no ROWS_DONE");
            let expected: Vec<i64> = (1..=ROWS).collect();
            assert_eq!(
                seqs, expected,
                "iteration {it}: rows must arrive strictly in order with no gaps and no duplicates"
            );
        }
    }

    /// #1347: a planned restart tells every room socket `restart_soon`, then closes it with 1012
    /// (not a dropped stream), drains to zero, and a socket that arrives mid-drain gets 1012 at once.
    /// A device's row into a project doc the box holds no socket on (a settings change while
    /// the box has closed the room for being idle) queues the push that reopens it; with the
    /// box on the room it arrives live and nothing is queued.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_device_row_into_a_project_doc_without_a_box_socket_queues_its_reopen_push() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "Doc");
        let (_, token) = device_with_token(&state, &alice);
        let addr = serve(state.clone()).await;
        let doc = arbos_core::wire::PROJECT_DOC_CHAT_ID;
        let (ws, _) = tokio_tungstenite::connect_async(format!(
            "ws://{addr}/room/{project}/{doc}?token={token}"
        ))
        .await
        .unwrap();
        let (mut sink, mut stream) = ws.split();
        sink.send(TMsg::Binary(
            Frame::new(
                FrameType::Hello,
                json!({ "cursor": 0, "device": "dev:w", "room": format!("{project}/{doc}"), "proto": arbos_core::PROTO_VERSION }),
                bytes::Bytes::new(),
            )
            .encode(),
        ))
        .await
        .unwrap();
        let _ = state.db.take_pushes(&project).unwrap();
        let opens = |state: &AppState| {
            state
                .db
                .take_pushes(&project)
                .unwrap()
                .into_iter()
                .filter(|(_, k, _)| k == crate::boxes::PROJECT_DOC_OPEN_PUSH)
                .count()
        };
        let row = arbos_core::doc::ChatDoc::new(Some(5), false);
        for (n, expect) in [(1, 1), (2, 0)] {
            let mark = row.oplog_vv();
            row.init_meta("c", "p", n);
            sink.send(TMsg::Binary(
                Frame::new(
                    FrameType::Push,
                    json!({ "batchId": format!("b{n}") }),
                    bytes::Bytes::from(row.export_updates_since(&mark)),
                )
                .encode(),
            ))
            .await
            .unwrap();
            // Frames are handled in order on the socket task, so the probe's answer means the
            // row before it has been handled in full (the ack alone is sent mid-way).
            sink.send(TMsg::Binary(
                Frame::new(FrameType::Probe, json!({}), bytes::Bytes::new()).encode(),
            ))
            .await
            .unwrap();
            let handled = tokio::time::timeout(Duration::from_secs(10), async {
                while let Some(Ok(m)) = stream.next().await {
                    if decode(m).is_some_and(|f| f.ty == FrameType::ProbeOk) {
                        return true;
                    }
                }
                false
            })
            .await
            .unwrap_or(false);
            assert!(handled, "row {n} handled");
            assert_eq!(opens(&state), expect, "reopen pushes after row {n}");
            // The box joins the room before the second row.
            state
                .room(&format!("{project}/{doc}"))
                .await
                .box_joined(&alice);
        }
    }

    /// Presence stamps reach `last_seen_at` through the batched flush: only the ones newer than
    /// the last flush are written, and a stamp never moves a device back behind a later HTTP
    /// call's `touch_device`.
    #[tokio::test]
    async fn presence_stamps_are_flushed_in_a_batch_and_never_move_a_device_back() {
        let state = test_state(AuthMode::Github);
        let (alice, dev) = person(&state, 1);
        let Principal::Device { id, .. } = dev else {
            unreachable!()
        };
        let t = now_ms() + 60_000;
        state.presence_seen.lock().await.insert(id.clone(), t);
        assert_eq!(super::flush_presence(&state, 0).await, 1);
        assert_eq!(state.db.user_last_seen(&alice).unwrap(), Some(t));
        assert_eq!(
            super::flush_presence(&state, t).await,
            0,
            "nothing new since"
        );

        state.presence_seen.lock().await.insert(id, t - 500);
        assert_eq!(super::flush_presence(&state, t - 1_000).await, 1);
        assert_eq!(state.db.user_last_seen(&alice).unwrap(), Some(t));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn planned_restart_notifies_then_closes_room_sockets_with_1012() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "Restart");
        let (_, token) = device_with_token(&state, &alice);
        let addr = serve(state.clone()).await;
        let chat = ulid();
        state
            .db
            .insert_chat(&Chat {
                id: chat.clone(),
                project_id: project.clone(),
                title: "t".into(),
                is_pinned: false,
                position: 0,
                created_at: now_ms(),
                doc_epoch: 1,
                kind: "chat".into(),
                participants: Vec::new(),
                forked_from: None,
            })
            .unwrap();
        let url = format!("ws://{addr}/room/{project}/{chat}?token={token}");
        let hello = || {
            TMsg::Binary(
                Frame::new(
                    FrameType::Hello,
                    json!({ "cursor": 0, "device": "dev:a", "room": format!("{project}/{chat}"), "proto": arbos_core::PROTO_VERSION }),
                    bytes::Bytes::new(),
                )
                .encode(),
            )
        };
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        ws.send(hello()).await.unwrap();
        // Drain the catch-up reply (State/Rows frames) until the socket is idle.
        while tokio::time::timeout(Duration::from_millis(300), ws.next())
            .await
            .ok()
            .flatten()
            .is_some()
        {}
        assert_eq!(state.restart.open_sockets.load(Ordering::Relaxed), 1);

        let rooms = crate::restart::announce(&state, 10).await;
        assert_eq!(rooms, 1);
        let notice = tokio::time::timeout(Duration::from_secs(2), ws.next())
            .await
            .expect("notice in time")
            .unwrap()
            .unwrap();
        let f = decode(notice).expect("a frame");
        assert_eq!(f.ty, FrameType::Error);
        assert_eq!(f.header["code"], "restart_soon");
        assert_eq!(f.header["message"], "10");

        let (before, left) = crate::restart::close_and_drain(&state, Duration::from_secs(3)).await;
        assert_eq!((before, left), (1, 0), "one socket closed, drained to zero");
        let close = tokio::time::timeout(Duration::from_secs(2), ws.next())
            .await
            .expect("close in time")
            .unwrap()
            .unwrap();
        match close {
            TMsg::Close(Some(cf)) => assert_eq!(u16::from(cf.code), 1012),
            other => panic!("expected a 1012 close frame, got {other:?}"),
        }

        // Mid-drain arrival: closed with 1012 right away, never counted.
        let (mut late, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let first = tokio::time::timeout(Duration::from_secs(2), late.next())
            .await
            .expect("late close in time")
            .unwrap()
            .unwrap();
        match first {
            TMsg::Close(Some(cf)) => assert_eq!(u16::from(cf.code), 1012),
            other => panic!("late socket: expected 1012, got {other:?}"),
        }
        assert_eq!(state.restart.open_sockets.load(Ordering::Relaxed), 0);
    }
}

#[cfg(test)]
mod history_tests {
    //! Arbonites, 2026-10-23 (#1518): a fresh device must get the whole history of a long chat —
    //! from the checkpoint plus the rows after it — through a graceful 1012 restart in the middle
    //! of its backfill, and when it reuses a device id that wrote rows before. And the relay must
    //! refuse a checkpoint whose snapshot lacks rows it holds, because storing one deletes them.

    use axum::routing::get;
    use axum::Router;
    use futures::{SinkExt, StreamExt};
    use serde_json::json;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as TMsg;

    use arbos_core::doc::{ChatDoc, UserMessageOpts};
    use arbos_core::ids::{now_ms, ulid};
    use arbos_core::wire::{Frame, FrameType};

    use super::{checkpoint_covers_store, CheckpointVerdict};
    use crate::people::tests_support::{person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;
    use crate::{db::Chat, state::AppState};

    const ROWS: usize = 700;

    async fn serve(state: AppState) -> SocketAddr {
        let app = Router::new()
            .route("/room/{project}/{chat}", get(super::ws_room))
            .route(
                "/room/{project}/{chat}/checkpoint",
                get(super::get_checkpoint),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    fn device_token(state: &AppState, user: &str) -> String {
        let token = format!("adv_{}", ulid().to_ascii_lowercase());
        state
            .db
            .insert_device(
                &ulid(),
                "t",
                &arbos_core::auth::hash(&token),
                user,
                "paircode",
            )
            .unwrap();
        token
    }

    fn new_chat(state: &AppState, project: &str) -> String {
        let chat = ulid();
        state
            .db
            .insert_chat(&Chat {
                id: chat.clone(),
                project_id: project.to_owned(),
                title: "t".into(),
                is_pinned: false,
                position: 0,
                created_at: now_ms(),
                doc_epoch: 1,
                kind: "chat".into(),
                participants: Vec::new(),
                forked_from: None,
            })
            .unwrap();
        chat
    }

    /// Write `n` user messages as `n` rows under `device`, the way an app does (one commit, one
    /// PUSH each). Returns the writer's doc.
    fn write_rows(state: &AppState, room: &str, device: &str, n: usize) -> ChatDoc {
        let doc = ChatDoc::new(Some(7), false);
        let base = doc.oplog_vv();
        doc.init_meta("c", "p", 1);
        state
            .db
            .append_row(room, device, "meta", &doc.export_updates_since(&base))
            .unwrap();
        for i in 0..n {
            let before = doc.oplog_vv();
            doc.append_user_message(
                &format!("m{i}"),
                &format!("message {i}"),
                device,
                &UserMessageOpts::default(),
            )
            .unwrap();
            let bytes = doc.export_updates_since(&before);
            state
                .db
                .append_row(room, device, &format!("b{i}"), &bytes)
                .unwrap();
        }
        doc
    }

    fn decode(m: TMsg) -> Option<Frame> {
        match m {
            TMsg::Binary(b) => Frame::decode(b).ok().flatten(),
            _ => None,
        }
    }

    /// A device's sync, as the apps do it (arbos-client `RoomSession`, which the relay may not
    /// depend on): HELLO with the saved cursor, fetch the checkpoint when the local doc does not
    /// contain its frontier, ROWS_REQ after the cursor with `excludeOwn:false`, cursor moves on
    /// each applied ROW and on ROWS_DONE. `restart_after` rows into the first backfill the relay
    /// restarts gracefully (1012); the device reconnects from its saved cursor.
    struct Device {
        doc: ChatDoc,
        cursor: i64,
        device: String,
        /// Bytes this device pushed back as history heals.
        healed_bytes: usize,
    }

    impl Device {
        fn fresh(device: &str) -> Self {
            Self {
                doc: ChatDoc::new(Some(9), false),
                cursor: 0,
                device: device.to_owned(),
                healed_bytes: 0,
            }
        }

        /// A device that already holds `doc` (it was live when the rows were written).
        fn holding(device: &str, doc: ChatDoc, cursor: i64) -> Self {
            Self {
                doc,
                cursor,
                device: device.to_owned(),
                healed_bytes: 0,
            }
        }

        /// One connection: returns true when ROWS_DONE was reached, false on a close.
        async fn connect_once(
            &mut self,
            addr: SocketAddr,
            room: &str,
            token: &str,
            state: &AppState,
            restart_after: Option<usize>,
        ) -> bool {
            let url = format!("ws://{addr}/room/{room}?token={token}");
            let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
            let (mut sink, mut stream) = ws.split();
            sink.send(TMsg::Binary(
                Frame::new(
                    FrameType::Hello,
                    json!({ "cursor": self.cursor, "device": self.device, "room": room, "proto": arbos_core::PROTO_VERSION }),
                    bytes::Bytes::new(),
                )
                .encode(),
            ))
            .await
            .unwrap();
            let mut rows_this_session = 0usize;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            loop {
                let m = tokio::time::timeout_at(deadline, stream.next())
                    .await
                    .expect("relay answers in time");
                let Some(Ok(m)) = m else { return false };
                if let TMsg::Close(_) = m {
                    return false;
                }
                let Some(f) = decode(m) else { continue };
                match f.ty {
                    FrameType::State => {
                        let cp = f.header["checkpointSeq"].as_i64().unwrap_or(0);
                        if cp > 0 && !self.doc.contains_frontier(&f.payload) {
                            let res = reqwest::Client::new()
                                .get(format!("http://{addr}/room/{room}/checkpoint"))
                                .bearer_auth(token)
                                .send()
                                .await
                                .unwrap();
                            let seq: i64 = res.headers()["x-arbos-checkpoint-seq"]
                                .to_str()
                                .unwrap()
                                .parse()
                                .unwrap();
                            let bytes = res.bytes().await.unwrap();
                            self.doc.import(&bytes).unwrap();
                            self.cursor = self.cursor.max(seq);
                        }
                        sink.send(TMsg::Binary(
                            Frame::new(
                                FrameType::RowsReq,
                                json!({ "after": self.cursor, "excludeOwn": false }),
                                bytes::Bytes::new(),
                            )
                            .encode(),
                        ))
                        .await
                        .unwrap();
                    }
                    FrameType::Row => {
                        let seq = f.header["seq"].as_i64().unwrap();
                        self.doc.import(&f.payload).unwrap();
                        self.cursor = self.cursor.max(seq);
                        rows_this_session += 1;
                        if restart_after.is_some_and(|n| rows_this_session == n) {
                            // The relay goes down gracefully mid-stream: 1012 to every socket.
                            // Whatever was still in flight on the wire is gone with the process;
                            // the device keeps the cursor of the last row it applied.
                            crate::restart::close_and_drain(state, Duration::from_secs(3)).await;
                            return false;
                        }
                    }
                    FrameType::RowsDone => {
                        let head = f.header["headSeq"].as_i64().unwrap();
                        self.cursor = self.cursor.max(head);
                        // History heal: push what this replica holds beyond the relay's vv.
                        if let Some(relay_vv) = arbos_core::doc::decode_vv(&f.payload) {
                            let missing = self.doc.updates_beyond(&relay_vv);
                            if !missing.is_empty() {
                                self.healed_bytes += missing.len();
                                sink.send(TMsg::Binary(
                                    Frame::new(
                                        FrameType::Push,
                                        json!({ "batchId": format!("heal-{}", ulid()), "heal": true }),
                                        bytes::Bytes::from(missing),
                                    )
                                    .encode(),
                                ))
                                .await
                                .unwrap();
                                // Wait for the ACK so the test knows the row is stored.
                                loop {
                                    let m = tokio::time::timeout_at(deadline, stream.next())
                                        .await
                                        .expect("ack in time");
                                    let Some(Ok(m)) = m else { return false };
                                    if let Some(f) = decode(m) {
                                        if f.ty == FrameType::Ack {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        return true;
                    }
                    _ => {}
                }
            }
        }

        async fn sync(
            &mut self,
            addr: SocketAddr,
            room: &str,
            token: &str,
            state: &AppState,
            restart_after: Option<usize>,
        ) -> usize {
            let mut reconnects = 0;
            let mut restart_after = restart_after;
            loop {
                let done = self
                    .connect_once(addr, room, token, state, restart_after.take())
                    .await;
                if done {
                    return reconnects;
                }
                // "systemctl restart": the new process serves sockets again.
                state.restart.closing.send_replace(false);
                reconnects += 1;
                assert!(reconnects < 5, "device could not finish its backfill");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }

        fn message_ids(&self) -> Vec<String> {
            self.doc.messages().into_iter().map(|m| m.id).collect()
        }
    }

    fn expected_ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("m{i}")).collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn fresh_device_gets_all_700_rows_through_a_graceful_restart_mid_backfill() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "History");
        let chat = new_chat(&state, &project);
        let room = format!("{project}/{chat}");
        let token = device_token(&state, &alice);
        write_rows(&state, &room, "dev:phone", ROWS);
        let addr = serve(state.clone()).await;

        // A brand-new Mac: cursor 0, empty doc. The relay restarts after 300 rows; the device
        // redials from its saved cursor and finishes.
        let mut mac = Device::fresh("dev:mac");
        let reconnects = mac.sync(addr, &room, &token, &state, Some(300)).await;
        assert_eq!(reconnects, 1, "one 1012 close, one redial");
        assert_eq!(mac.message_ids(), expected_ids(ROWS));
        assert_eq!(mac.cursor, ROWS as i64 + 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn same_device_id_relogin_gets_its_own_rows_back() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "Relogin");
        let chat = new_chat(&state, &project);
        let room = format!("{project}/{chat}");
        let token = device_token(&state, &alice);
        // Every row was written under the id the device reconnects with (#1409: the own-row
        // exclusion would hide the whole chat from a re-login or a second replica).
        write_rows(&state, &room, "dev:mac", ROWS);
        let addr = serve(state.clone()).await;

        let mut again = Device::fresh("dev:mac");
        again.sync(addr, &room, &token, &state, None).await;
        assert_eq!(again.message_ids(), expected_ids(ROWS));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn fresh_device_after_a_checkpoint_gets_checkpoint_plus_rows() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "Checkpointed");
        let chat = new_chat(&state, &project);
        let room = format!("{project}/{chat}");
        let token = device_token(&state, &alice);
        let writer = write_rows(&state, &room, "dev:phone", ROWS);
        // A complete replica checkpoints the first 500 rows (seq 1 is the meta row).
        let head_500 = 501;
        let snapshot_500 = {
            let mut d = ChatDoc::new(None, false);
            for r in state.db.rows_between(&room, 0, head_500, 1000).unwrap() {
                d.import(&r.bytes).unwrap();
            }
            d.snapshot()
        };
        assert_eq!(
            checkpoint_covers_store(&state.db, &room, head_500, &snapshot_500).unwrap(),
            CheckpointVerdict::Covers
        );
        assert!(state
            .db
            .put_checkpoint(&room, head_500, &[], &snapshot_500)
            .unwrap());
        assert_eq!(state.db.room_state(&room).unwrap().floor, head_500);
        let _ = writer;
        let addr = serve(state.clone()).await;

        let mut mac = Device::fresh("dev:mac");
        mac.sync(addr, &room, &token, &state, Some(100)).await;
        assert_eq!(mac.message_ids(), expected_ids(ROWS));
    }

    /// The Arbonites failure: a replica that skipped another writer's rows (and so parks every
    /// dependant as pending) posts its snapshot as the checkpoint. The relay must refuse it and
    /// keep its rows; a complete snapshot is accepted.
    #[tokio::test]
    async fn incomplete_snapshot_is_refused_and_rows_stay() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "Guard");
        let chat = new_chat(&state, &project);
        let room = format!("{project}/{chat}");
        // Phone writes 300 rows; a second writer that has them all writes 10 more (its ops
        // depend on the phone's); the phone then writes 10 more on top of both.
        let mut phone = write_rows(&state, &room, "dev:phone", 300);
        let mut agent = ChatDoc::new(Some(8), false);
        agent.import(&phone.snapshot()).unwrap();
        for i in 0..10 {
            let before = agent.oplog_vv();
            agent
                .append_user_message(
                    &format!("a{i}"),
                    "agent",
                    "box:1",
                    &UserMessageOpts::default(),
                )
                .unwrap();
            state
                .db
                .append_row(
                    &room,
                    "box:1",
                    &format!("a{i}"),
                    &agent.export_updates_since(&before),
                )
                .unwrap();
        }
        phone.import(&agent.snapshot()).unwrap();
        for i in 300..310 {
            let before = phone.oplog_vv();
            phone
                .append_user_message(
                    &format!("m{i}"),
                    "late",
                    "dev:phone",
                    &UserMessageOpts::default(),
                )
                .unwrap();
            state
                .db
                .append_row(
                    &room,
                    "dev:phone",
                    &format!("b{i}"),
                    &phone.export_updates_since(&before),
                )
                .unwrap();
        }
        let head = state.db.room_state(&room).unwrap().head;
        assert_eq!(head, 1 + 300 + 10 + 10);

        // A replica behind the agent's device id never received the agent's rows ("own") and
        // parked the phone's last 10 as pending: its snapshot has 300 messages, not 320.
        let mut frozen = ChatDoc::new(Some(5), false);
        for r in state.db.rows_between(&room, 0, head, 1000).unwrap() {
            if r.device != "box:1" {
                frozen.import(&r.bytes).unwrap();
            }
        }
        assert_eq!(frozen.messages().len(), 300);
        let verdict = checkpoint_covers_store(&state.db, &room, head, &frozen.snapshot()).unwrap();
        assert!(
            matches!(&verdict, CheckpointVerdict::Incomplete(why) if why.contains("stored row")),
            "{verdict:?}"
        );
        assert_eq!(
            state.db.room_state(&room).unwrap().row_count,
            head,
            "nothing deleted"
        );

        // Above the head: refused too.
        assert!(matches!(
            checkpoint_covers_store(&state.db, &room, head + 1, &phone.snapshot()).unwrap(),
            CheckpointVerdict::Incomplete(_)
        ));

        // The complete replica's snapshot covers everything and compacts the room.
        assert_eq!(
            checkpoint_covers_store(&state.db, &room, head, &phone.snapshot()).unwrap(),
            CheckpointVerdict::Covers
        );
        assert!(state
            .db
            .put_checkpoint(&room, head, &[], &phone.snapshot())
            .unwrap());
        let st = state.db.room_state(&room).unwrap();
        assert_eq!((st.floor, st.row_count), (head, 0));

        // A later snapshot that drops the checkpoint's own ops is refused as well.
        let mut partial = ChatDoc::new(Some(6), false);
        partial.import(&frozen.snapshot()).unwrap();
        assert!(matches!(
            checkpoint_covers_store(&state.db, &room, head, &partial.snapshot()).unwrap(),
            CheckpointVerdict::Incomplete(why) if why.contains("current checkpoint")
        ));
    }

    /// A heal is history, not a message: a healer that another member has blocked (or that is
    /// over the message rate limit) still heals the room. The first production heal (Oct 23
    /// 20:42, 60 KB) was dropped by the block gate.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn blocked_member_can_still_heal() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = crate::people::tests_support::dm(&state, &alice, &bob);
        let room = format!("people/{chat}");
        let token = device_token(&state, &bob);
        let phone = write_rows(&state, &room, "dev:bob", 300);
        // Alice blocks Bob; Bob's messages are refused, but his history is not.
        state.db.block(&alice, &bob).unwrap();
        let stale = {
            let mut d = ChatDoc::new(None, false);
            for r in state.db.rows_between(&room, 0, 101, 1000).unwrap() {
                d.import(&r.bytes).unwrap();
            }
            d.snapshot()
        };
        assert!(state.db.put_checkpoint(&room, 201, &[], &stale).unwrap());
        let addr = serve(state.clone()).await;
        let mut bob_mac = Device::holding("dev:bob", phone, 301);
        bob_mac.sync(addr, &room, &token, &state, None).await;
        assert!(bob_mac.healed_bytes > 0);
        assert_eq!(
            state.db.room_state(&room).unwrap().head,
            302,
            "the heal row is stored"
        );
        let mut probe = Device::fresh("dev:probe");
        let token_a = device_token(&state, &alice);
        probe.sync(addr, &room, &token_a, &state, None).await;
        assert_eq!(probe.message_ids(), expected_ids(300));
    }

    /// The Arbonites recovery (#1518 follow-up): the relay's history was cut to a stale
    /// checkpoint (rows deleted, later rows parked as pending); a device that was live and still
    /// holds every op reconnects, gets the relay's version vector in ROWS_DONE, pushes the
    /// difference back as one `heal` row, and a brand-new device then sees the whole chat.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn live_device_heals_a_relay_that_lost_rows_and_a_fresh_device_converges() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "Heal");
        let chat = new_chat(&state, &project);
        let room = format!("{project}/{chat}");
        let token = device_token(&state, &alice);
        // The phone writes 700 rows; the relay then gets a stale checkpoint covering 400 of them
        // but built from a replica that only had the first 100 (what the old relay accepted),
        // so 300 rows' ops are gone from the relay and the remaining 300 rows are pending.
        let mut phone = write_rows(&state, &room, "dev:phone", ROWS);
        let head_400 = 401;
        let stale = {
            let mut d = ChatDoc::new(None, false);
            for r in state.db.rows_between(&room, 0, 101, 1000).unwrap() {
                d.import(&r.bytes).unwrap();
            }
            d.snapshot()
        };
        assert!(state
            .db
            .put_checkpoint(&room, head_400, &[], &stale)
            .unwrap());
        let addr = serve(state.clone()).await;

        // Before the heal: a fresh device gets 100 messages (the rest are parked).
        let mut probe = Device::fresh("dev:probe");
        probe.sync(addr, &room, &token, &state, None).await;
        assert_eq!(
            probe.message_ids().len(),
            100,
            "damaged relay shows 100 of 700"
        );

        // The phone (holding everything) reconnects and heals the relay with one row.
        let phone_vv_before = phone.oplog_vv();
        let mut live = Device::holding(
            "dev:phone",
            std::mem::replace(&mut phone, ChatDoc::new(None, false)),
            ROWS as i64 + 1,
        );
        live.sync(addr, &room, &token, &state, None).await;
        assert!(live.healed_bytes > 0, "the live device pushed a heal row");
        assert_eq!(
            live.doc.oplog_vv(),
            phone_vv_before,
            "heal adds nothing to the healer"
        );
        let st = state.db.room_state(&room).unwrap();
        let extra: Vec<(i64, String, usize)> = state
            .db
            .rows_between(&room, ROWS as i64 + 1, st.head, 100)
            .unwrap()
            .into_iter()
            .map(|r| (r.seq, r.device, r.bytes.len()))
            .collect();
        assert_eq!(
            st.head,
            ROWS as i64 + 2,
            "exactly one heal row stored; extra rows {extra:?}; probe healed {}",
            probe.healed_bytes
        );

        // After the heal: a brand-new device gets all 700.
        let mut mac = Device::fresh("dev:mac");
        mac.sync(addr, &room, &token, &state, None).await;
        assert_eq!(mac.message_ids(), expected_ids(ROWS));
        // And the probe, reconnecting, catches up to the full chat too.
        probe.sync(addr, &room, &token, &state, None).await;
        assert_eq!(probe.message_ids(), expected_ids(ROWS));
        // A second connect of the healer finds nothing to heal (relay vv now covers it).
        let healed_before = live.healed_bytes;
        live.sync(addr, &room, &token, &state, None).await;
        assert_eq!(
            live.healed_bytes, healed_before,
            "no second heal once the relay is whole"
        );
    }

    /// #2069 (long-task eval run 9): in a people chat the owner's device sent a message and the
    /// box's reply causally depends on it. Every row was acked and advanced `head_seq`, yet a
    /// replay that omits the device's own rows (`excludeOwn:true`) parks the box's reply as
    /// pending and truncates the doc at the last row before the omitted message — the owner's
    /// question and the agent's answer vanish. A full replay (`excludeOwn:false`, what the apps,
    /// agentd and arbosctl now all use) returns every acked row. The relay stores everything; the
    /// truncation was a client-side replay choice.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn replay_after_ack_returns_every_acked_row_across_cross_device_deps() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "Repro2069");
        let chat = new_chat(&state, &project);
        let room = format!("{project}/{chat}");
        let token = device_token(&state, &alice);

        // The owner's device writes meta and a user message.
        let owner = ChatDoc::new(Some(11), false);
        let base = owner.oplog_vv();
        owner.init_meta("c", "p", 1);
        state
            .db
            .append_row(
                &room,
                "dev:owner",
                "meta",
                &owner.export_updates_since(&base),
            )
            .unwrap();
        let before = owner.oplog_vv();
        owner
            .append_user_message(
                "m-owner",
                "read the summary back",
                "dev:owner",
                &UserMessageOpts::default(),
            )
            .unwrap();
        state
            .db
            .append_row(
                &room,
                "dev:owner",
                "b-owner",
                &owner.export_updates_since(&before),
            )
            .unwrap();

        // The box imports the owner's message, then replies — its reply's ops depend on the
        // owner's ops, so the reply cannot apply without them.
        let mut box_doc = ChatDoc::new(Some(22), false);
        box_doc.import(&owner.snapshot()).unwrap();
        let before_reply = box_doc.oplog_vv();
        box_doc
            .append_user_message(
                "m-box",
                "the summary currently reads ...",
                "box:x",
                &UserMessageOpts::default(),
            )
            .unwrap();
        let (reply_seq, _) = state
            .db
            .append_row(
                &room,
                "box:x",
                "b-box",
                &box_doc.export_updates_since(&before_reply),
            )
            .unwrap();

        let addr = serve(state.clone()).await;

        // A fresh device replays with `excludeOwn:false`: every acked row arrives and both
        // messages rebuild, none pending.
        let mut fresh = Device::fresh("dev:owner");
        fresh.sync(addr, &room, &token, &state, None).await;
        assert_eq!(fresh.cursor, reply_seq, "replay reaches the room head");
        let ids = fresh.message_ids();
        assert!(
            ids.contains(&"m-owner".to_string()),
            "owner message present"
        );
        assert!(
            ids.contains(&"m-box".to_string()),
            "the box reply is visible after a full replay"
        );

        // The regression: `excludeOwn:true` omits this device's own rows, so the box reply that
        // depends on them is parked as pending and never becomes a visible message.
        let kept = state
            .db
            .rows_after(&room, 0, Some("dev:owner"), 1024)
            .unwrap();
        assert!(
            kept.iter().all(|r| r.device != "dev:owner"),
            "excludeOwn drops the device's own rows"
        );
        let mut truncated = ChatDoc::new(None, false);
        let mut pending_seen = false;
        for r in &kept {
            if truncated.import(&r.bytes).unwrap().pending.is_some() {
                pending_seen = true;
            }
        }
        assert!(pending_seen, "the box reply is parked as pending");
        assert!(
            !truncated.messages().iter().any(|m| m.id == "m-box"),
            "the acked box reply is missing from the excludeOwn replay"
        );
    }
}

/// The apps' room session, compiled from its own source so a test can drive the real one
/// against this relay. `arbos-client` is app-only and the relay may not depend on it; the
/// module needs nothing beyond `arbos-core` and serde, which the relay already has.
#[cfg(test)]
#[allow(dead_code)]
#[path = "../../arbos-client/src/room.rs"]
mod client_room;

#[cfg(test)]
mod covered_push_tests {
    //! Reconnect storm (#1570): pending batches whose ops the room already holds must ACK as
    //! `dup` before the people gates — no new rows, no rate events — or every connect is refused
    //! with `rate_limited` forever.

    use axum::routing::get;
    use axum::Router;
    use futures::{SinkExt, StreamExt};
    use serde_json::json;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as TMsg;

    use arbos_core::doc::{encode_vv, ChatDoc, UserMessageOpts};
    use arbos_core::ids::ulid;
    use arbos_core::wire::{Frame, FrameType};

    use crate::people::tests_support::{dm, person};
    use crate::state::test_support::test_state;
    use crate::state::{AppState, AuthMode};

    async fn serve(state: AppState) -> SocketAddr {
        let app = Router::new()
            .route("/room/{project}/{chat}", get(super::ws_room))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    fn device_token(state: &AppState, user: &str) -> String {
        let token = format!("adv_{}", ulid().to_ascii_lowercase());
        state
            .db
            .insert_device(
                &ulid(),
                "t",
                &arbos_core::auth::hash(&token),
                user,
                "paircode",
            )
            .unwrap();
        token
    }

    fn decode(m: TMsg) -> Option<Frame> {
        match m {
            TMsg::Binary(b) => Frame::decode(b).ok().flatten(),
            _ => None,
        }
    }

    /// HELLO + ROWS_REQ through ROWS_DONE, then push `batches` and collect ACK / error frames.
    async fn push_after_catchup(
        addr: SocketAddr,
        room: &str,
        token: &str,
        device: &str,
        batches: &[(String, Vec<u8>)],
    ) -> Vec<Frame> {
        push_after_catchup_with_heal(addr, room, token, device, batches, false).await
    }

    async fn push_after_catchup_with_heal(
        addr: SocketAddr,
        room: &str,
        token: &str,
        device: &str,
        batches: &[(String, Vec<u8>)],
        heal: bool,
    ) -> Vec<Frame> {
        let url = format!("ws://{addr}/room/{room}?token={token}");
        let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let (mut sink, mut stream) = ws.split();
        sink.send(TMsg::Binary(
            Frame::new(
                FrameType::Hello,
                json!({
                    "cursor": 0,
                    "device": device,
                    "room": room,
                    "proto": arbos_core::PROTO_VERSION
                }),
                bytes::Bytes::new(),
            )
            .encode(),
        ))
        .await
        .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let m = tokio::time::timeout_at(deadline, stream.next())
                .await
                .expect("relay answers")
                .unwrap()
                .unwrap();
            let Some(f) = decode(m) else { continue };
            match f.ty {
                FrameType::State => {
                    sink.send(TMsg::Binary(
                        Frame::new(
                            FrameType::RowsReq,
                            json!({ "after": 0, "excludeOwn": false }),
                            bytes::Bytes::new(),
                        )
                        .encode(),
                    ))
                    .await
                    .unwrap();
                }
                FrameType::RowsDone => break,
                FrameType::Row | FrameType::Error | FrameType::Ack => {}
                _ => {}
            }
        }
        for (batch_id, bytes) in batches {
            sink.send(TMsg::Binary(
                Frame::new(
                    FrameType::Push,
                    json!({ "batchId": batch_id, "heal": heal }),
                    bytes::Bytes::from(bytes.clone()),
                )
                .encode(),
            ))
            .await
            .unwrap();
        }
        let mut out = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while out.len() < batches.len() {
            let m = tokio::time::timeout_at(deadline, stream.next())
                .await
                .expect("ack/error")
                .unwrap()
                .unwrap();
            if let Some(f) = decode(m) {
                if matches!(f.ty, FrameType::Ack | FrameType::Error) {
                    out.push(f);
                }
            }
        }
        out
    }

    /// 250 pending batches whose ops the room already holds → 250 dup ACKs, no new rows, no
    /// rate events.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn covered_pending_batches_ack_as_dup_without_rate_events() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let room = format!("people/{chat}");
        let token = device_token(&state, &alice);

        // Room already holds 250 message ops (written under bob).
        let writer = ChatDoc::new(Some(7), false);
        let base = writer.oplog_vv();
        writer.init_meta(&chat, "people", 1);
        state
            .db
            .append_row(
                &room,
                "dev:bob",
                "meta",
                &writer.export_updates_since(&base),
            )
            .unwrap();
        let mut pending = Vec::new();
        for i in 0..250 {
            let before = writer.oplog_vv();
            writer
                .append_user_message(
                    &format!("m{i}"),
                    &format!("msg {i}"),
                    "dev:bob",
                    &UserMessageOpts::default(),
                )
                .unwrap();
            let bytes = writer.export_updates_since(&before);
            state
                .db
                .append_row(&room, "dev:bob", &format!("stored-{i}"), &bytes)
                .unwrap();
            // Alice's pending list uses different batch ids for the same ops (refused heals
            // resent as ordinary pushes).
            pending.push((format!("pending-{i}"), bytes));
        }
        let head_before = state.db.room_state(&room).unwrap().head;
        let rates_before = state.db.rate_count(&alice, "msg", 60_000).unwrap();

        let addr = serve(state.clone()).await;
        let frames = push_after_catchup(addr, &room, &token, "dev:alice", &pending).await;

        assert_eq!(frames.len(), 250);
        for f in &frames {
            assert_eq!(f.ty, FrameType::Ack, "header={}", f.header);
            assert_eq!(f.header["dup"], true, "header={}", f.header);
        }
        assert_eq!(
            state.db.room_state(&room).unwrap().head,
            head_before,
            "no new rows stored"
        );
        assert_eq!(
            state.db.rate_count(&alice, "msg", 60_000).unwrap(),
            rates_before,
            "covered dups must not count as messages"
        );
    }

    /// Alice and Bob in a chat whose stored doc holds `m-bob`, written by Bob's replica
    /// (peer 7) and attested to him. Returns Bob's replica and the two device tokens.
    fn chat_with_bobs_message(state: &AppState) -> (String, ChatDoc, String, String) {
        let (alice, _) = person(state, 1);
        let (bob, _) = person(state, 2);
        let chat = dm(state, &alice, &bob);
        let room = format!("people/{chat}");
        let bob_doc = ChatDoc::new(Some(7), false);
        let base = bob_doc.oplog_vv();
        bob_doc.init_meta(&chat, "people", 1);
        bob_doc
            .append_user_message("m-bob", "hello", "dev:bob", &as_user(&bob))
            .unwrap();
        state
            .db
            .append_row(
                &room,
                "dev:bob",
                "seed",
                &bob_doc.export_updates_since(&base),
            )
            .unwrap();
        state
            .db
            .record_message_author(&chat, "m-bob", &format!("user:{bob}"))
            .unwrap();
        let (alice_token, bob_token) = (device_token(state, &alice), device_token(state, &bob));
        (room, bob_doc, alice_token, bob_token)
    }

    fn as_user(user: &str) -> UserMessageOpts {
        UserMessageOpts {
            sender: Some(format!("user:{user}")),
            ..Default::default()
        }
    }

    /// A checkpoint is the room's own history, folded. What a box uploads proves it holds every
    /// row; anything more in it goes through a push or not at all. Asked through the relay's own
    /// router with a box's token.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_checkpoint_stores_what_the_room_holds_and_nothing_the_upload_adds() {
        let state = test_state(AuthMode::Github);
        let (room, bob_doc, alice_token, _) = chat_with_bobs_message(&state);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let box_token = box_token_of(&state, &alice);
        // The box's replica: everything the room holds, and on top of it a message in Bob's
        // name and an edit of Bob's own message, neither of them ever pushed.
        let mut box_doc = ChatDoc::new(Some(5), false);
        box_doc.import(&bob_doc.snapshot()).unwrap();
        box_doc
            .append_user_message("m-extra", "never pushed", "dev:bob", &as_user(&bob))
            .unwrap();
        box_doc.set_message_fields("m-bob", &[("preview", json!("edited"))]);
        let head = state.db.room_state(&room).unwrap().head;
        let addr = serve_relay(&state).await;
        let post = |token: String, body: Vec<u8>| ask_checkpoint(addr, &room, head, token, body);

        // A device may not ask, and is turned away before the room is touched.
        let res = post(alice_token, box_doc.snapshot()).await.unwrap();
        assert_eq!(res.status(), 403);
        assert!(state.db.checkpoint_bytes(&room).unwrap().is_none());

        let res = post(box_token.clone(), box_doc.snapshot()).await.unwrap();
        assert_eq!(res.status(), 204);
        let (seq, bytes) = state.db.checkpoint_bytes(&room).unwrap().unwrap();
        assert_eq!(seq, head);
        let stored = ChatDoc::from_snapshot(None, false, &bytes).unwrap();
        assert_eq!(
            stored.oplog_vv(),
            bob_doc.oplog_vv(),
            "no op but the room's"
        );
        let messages = stored.messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, "m-bob");
        assert!(messages[0].rest.get("preview").is_none());

        // The room was read once; asking again straight away is refused before any work.
        let res = post(box_token, b"nothing".to_vec()).await.unwrap();
        assert_eq!(res.status(), 429);
    }

    /// A box with the token it signs in with.
    fn box_token_of(state: &AppState, user: &str) -> String {
        let token = format!("abx_{}", ulid().to_ascii_lowercase());
        state
            .db
            .upsert_box(&crate::db::Sandbox {
                user_id: user.to_owned(),
                provider: "gvisor".into(),
                sandbox_id: None,
                daytona_state: None,
                device_id: format!("box:{user}"),
                box_token_hash: arbos_core::auth::hash(&token),
                mcp_token: ulid(),
                snapshot: None,
                last_started_at: None,
                last_stopped_at: None,
                last_seen_at: None,
                box_url: None,
                preview_token: None,
                local_pid: None,
                migrated_from: Vec::new(),
            })
            .unwrap();
        token
    }

    /// The relay's own router, on a loopback port.
    async fn serve_relay(state: &AppState) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = crate::app(state.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    async fn ask_checkpoint(
        addr: SocketAddr,
        room: &str,
        seq_covered: i64,
        token: String,
        body: Vec<u8>,
    ) -> reqwest::Result<reqwest::Response> {
        reqwest::Client::new()
            .post(format!(
                "http://{addr}/room/{room}/checkpoint?seqCovered={seq_covered}"
            ))
            .bearer_auth(token)
            .body(body)
            .send()
            .await
    }

    /// A project's ACL covers every chat id under it. A checkpoint asked for a chat that does
    /// not exist is answered 404 and the relay keeps nothing for it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_checkpoint_for_a_chat_that_does_not_exist_leaves_no_room_behind() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = crate::people::tests_support::project_of(&state, &alice, "P");
        let token = box_token_of(&state, &alice);
        let addr = serve_relay(&state).await;
        for _ in 0..3 {
            let room = format!("{project}/{}", ulid());
            let res = ask_checkpoint(addr, &room, 0, token.clone(), Vec::new())
                .await
                .unwrap();
            assert_eq!(res.status(), 404);
        }
        assert!(state.rooms.read().await.is_empty());
    }

    /// A refused attempt holds back only the box that made it; a stored checkpoint holds back
    /// everyone, since the room has just been folded.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_refused_checkpoint_does_not_keep_another_members_box_from_storing_one() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let (alice_box, bob_box) = (box_token_of(&state, &alice), box_token_of(&state, &bob));
        let addr = serve_relay(&state).await;

        let (room, doc, _, _) = chat_with_bobs_message(&state);
        let head = state.db.room_state(&room).unwrap().head;
        let ask =
            |token: &String, body: Vec<u8>| ask_checkpoint(addr, &room, head, token.clone(), body);
        assert_eq!(
            ask(&alice_box, b"nothing".to_vec()).await.unwrap().status(),
            409
        );
        assert_eq!(ask(&alice_box, doc.snapshot()).await.unwrap().status(), 429);
        assert_eq!(ask(&bob_box, doc.snapshot()).await.unwrap().status(), 204);

        // In a second room: two good attempts inside the interval store once.
        let (room, doc, _, _) = chat_with_bobs_message(&state);
        let head = state.db.room_state(&room).unwrap().head;
        let ask =
            |token: &String, body: Vec<u8>| ask_checkpoint(addr, &room, head, token.clone(), body);
        assert_eq!(ask(&bob_box, doc.snapshot()).await.unwrap().status(), 204);
        let first = state.db.checkpoint_full(&room).unwrap().unwrap();
        assert_eq!(ask(&alice_box, doc.snapshot()).await.unwrap().status(), 429);
        assert_eq!(state.db.checkpoint_full(&room).unwrap().unwrap(), first);
    }

    /// An upload that proves nothing is refused as the upload's fault, and one the room's own
    /// doc cannot back is refused as the room's: the two are told apart for whoever reads the
    /// log and the counter.
    #[test]
    fn a_refused_checkpoint_names_the_snapshot_that_fell_short() {
        let state = test_state(AuthMode::Github);
        let (room, bob_doc, _, _) = chat_with_bobs_message(&state);
        let chat = room.strip_prefix("people/").unwrap();
        let head = state.db.room_state(&room).unwrap().head;
        let ask = |upload: &[u8], head| {
            super::store_own_checkpoint(&state, "people", chat, &room, head, upload).unwrap()
        };
        assert_eq!(ask(b"nothing", head).unwrap_err().0, "upload_incomplete");

        // A row the room's doc cannot apply: a change whose parent the room was never sent.
        let mark = bob_doc.oplog_vv();
        bob_doc
            .append_user_message("m-2", "two", "dev:bob", &UserMessageOpts::default())
            .unwrap();
        let missing = bob_doc.oplog_vv();
        bob_doc
            .append_user_message("m-3", "three", "dev:bob", &UserMessageOpts::default())
            .unwrap();
        let late = bob_doc.export_updates_since(&missing);
        let _ = mark;
        state
            .db
            .append_row(&room, "dev:bob", "late", &late)
            .unwrap();
        assert_eq!(
            ask(&bob_doc.snapshot(), head + 1).unwrap_err().0,
            "room_incomplete"
        );
        assert!(state.db.checkpoint_bytes(&room).unwrap().is_none());
        assert_eq!(state.db.room_state(&room).unwrap().row_count, 2);
    }

    /// A push that imports and whose diff the relay still cannot read is refused with the code
    /// a device stays connected on, and nothing is stored. The room's doc is rebuilt for the
    /// next push, so the same change sent again is taken.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_diff_that_cannot_be_read_is_refused_and_the_next_push_reads_a_rebuilt_doc() {
        let state = test_state(AuthMode::Github);
        let (room, bob_doc, _, bob_token) = chat_with_bobs_message(&state);
        let head = state.db.room_state(&room).unwrap().head;
        // The room's mirror of ids out of step with its list: no row can be named.
        let mut cached = super::build_room_doc(&state, &room).unwrap();
        cached.message_ids.clear();
        *state.room(&room).await.doc_cache.lock().unwrap() = Some(cached);
        let mark = bob_doc.oplog_vv();
        bob_doc.set_message_fields("m-bob", &[("preview", json!("x"))]);
        let edit = bob_doc.export_updates_since(&mark);
        let addr = serve(state.clone()).await;

        let frames = push_after_catchup(
            addr,
            &room,
            &bob_token,
            "dev:bob",
            &[("edit".into(), edit.clone())],
        )
        .await;
        assert_eq!(frames[0].ty, FrameType::Error, "{}", frames[0].header);
        assert_eq!(frames[0].header["code"], "not_available");
        assert_eq!(frames[0].header["batchId"], "edit");
        assert_eq!(state.db.room_state(&room).unwrap().head, head);

        let frames =
            push_after_catchup(addr, &room, &bob_token, "dev:bob", &[("edit".into(), edit)]).await;
        assert_eq!(frames[0].ty, FrameType::Ack, "{}", frames[0].header);
        assert_eq!(state.db.room_state(&room).unwrap().head, head + 1);
    }

    /// In a people chat a sender the gates refuse gets the gates' answer for a push the guard
    /// refuses too, so the device stays connected.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_sender_the_gates_refuse_gets_their_answer_for_a_push_the_guard_refuses() {
        let state = test_state(AuthMode::Github);
        let (room, bob_doc, _, bob_token) = chat_with_bobs_message(&state);
        let (bob, _) = person(&state, 2);
        let chat = room.strip_prefix("people/").unwrap();
        let me = format!("user:{bob}");
        assert!(state.db.set_member_silenced(chat, &me, true, None).unwrap());
        let mark = bob_doc.oplog_vv();
        bob_doc.set_message_fields("m-bob", &[("id", json!("m-other"))]);
        let frames = push_after_catchup(
            serve(state.clone()).await,
            &room,
            &bob_token,
            "dev:bob",
            &[("b1".into(), bob_doc.export_updates_since(&mark))],
        )
        .await;
        assert_eq!(
            frames[0].ty,
            FrameType::Error,
            "header={}",
            frames[0].header
        );
        assert_eq!(frames[0].header["code"], "not_available");
        assert_eq!(frames[0].header["reason"], "silenced");
        assert_eq!(frames[0].header["batchId"], "b1");
    }

    /// Runs a real client session against the room until the relay has nothing more to say,
    /// answering it as an app does. Returns what the session asked its host to do.
    async fn run_session(
        state: &AppState,
        addr: SocketAddr,
        token: &str,
        session: &mut super::client_room::RoomSession,
        doc: &mut ChatDoc,
    ) -> Vec<super::client_room::RoomAction> {
        use super::client_room::RoomAction;
        let url = format!("ws://{addr}/room/{}?token={token}", session.room());
        let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let (mut sink, mut stream) = ws.split();
        session.on_open(0);
        let mut seen = Vec::new();
        loop {
            // Answering one action can queue more, so drain until the session is quiet.
            let mut actions = std::collections::VecDeque::from(session.drain_actions());
            while let Some(action) = actions.pop_front() {
                match &action {
                    RoomAction::SendFrame(bytes) => {
                        sink.send(TMsg::Binary(bytes.clone().into())).await.unwrap()
                    }
                    RoomAction::ApplyBytes(bytes) => {
                        doc.import(bytes).unwrap();
                    }
                    RoomAction::HealCheck { relay_vv } => {
                        let vv = arbos_core::doc::decode_vv(relay_vv).unwrap();
                        session.on_heal(doc.updates_beyond(&vv));
                    }
                    RoomAction::FetchCheckpoint { frontier, .. } => {
                        if doc.contains_frontier(frontier) {
                            session.on_checkpoint_skipped();
                        } else {
                            let (seq, bytes) =
                                state.db.checkpoint_bytes(session.room()).unwrap().unwrap();
                            doc.import(&bytes).unwrap();
                            session.on_checkpoint(seq);
                        }
                    }
                    // The refused ops are in the old document: the app starts a new one.
                    RoomAction::Rebuild { .. } => *doc = ChatDoc::new(None, false),
                    _ => {}
                }
                seen.push(action);
                actions.extend(session.drain_actions());
            }
            match tokio::time::timeout(Duration::from_millis(400), stream.next()).await {
                Ok(Some(Ok(TMsg::Binary(bytes)))) => session.on_frame(&bytes),
                Ok(Some(Ok(_))) => {}
                _ => break,
            }
        }
        seen
    }

    /// What an app does today passes every rule: a device that comes back with several
    /// messages written offline has each of them stored under its author, and its session
    /// stays live with nothing refused and no redial asked for.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_real_client_session_sends_what_it_wrote_offline_and_stays_connected() {
        use super::client_room::{RoomAction, RoomSession};
        let state = test_state(AuthMode::Github);
        let (room, bob_doc, alice_token, _) = chat_with_bobs_message(&state);
        let (alice, _) = person(&state, 1);
        let addr = serve(state.clone()).await;

        let mut doc = ChatDoc::new(Some(5), false);
        doc.import(&bob_doc.snapshot()).unwrap();
        let mut session = RoomSession::new("dev:alice", room.clone(), 0, Vec::new());
        for id in ["m1", "m2", "m3"] {
            let mark = doc.oplog_vv();
            doc.append_user_message(id, "hi", "dev:alice", &as_user(&alice))
                .unwrap();
            session.on_local_commit(doc.export_updates_since(&mark));
        }
        let seen = run_session(&state, addr, &alice_token, &mut session, &mut doc).await;
        // The heal on connect carries all three; the batches behind it ack as already held.
        assert!(session.is_live() && session.pending().is_empty());
        assert!(!seen.iter().any(|a| matches!(
            a,
            RoomAction::ScheduleReconnect { .. }
                | RoomAction::SessionError { .. }
                | RoomAction::PushRefused { .. }
        )));
        let stored = super::build_room_doc(&state, &room).unwrap();
        assert_eq!(stored.doc.message_ids(), ["m-bob", "m1", "m2", "m3"]);
        for id in ["m1", "m2", "m3"] {
            let chat = room.strip_prefix("people/").unwrap();
            assert_eq!(
                state.db.message_author(chat, id).unwrap(),
                Some(format!("user:{alice}"))
            );
        }
    }

    /// A device that wrote something the relay refuses for good used to send it again on every
    /// connection, with everything behind it. Now the apps' session lets go of it: the device
    /// reads the room back from the relay's checkpoint and rows, what it writes afterwards is
    /// stored, and its document is the relay's.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_device_refused_for_good_reads_the_room_back_and_delivers_again() {
        use super::client_room::{RoomAction, RoomSession};
        let state = test_state(AuthMode::Github);
        let (room, bob_doc, alice_token, bob_token) = chat_with_bobs_message(&state);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let addr = serve(state.clone()).await;
        // The relay holds a checkpoint of Bob's first message and a row for his second.
        let frontier = arbos_core::doc::encode_vv(&bob_doc.oplog_vv());
        assert!(state
            .db
            .put_checkpoint(&room, 1, &frontier, &bob_doc.snapshot())
            .unwrap());
        let mark = bob_doc.oplog_vv();
        bob_doc
            .append_user_message("m-bob-2", "again", "dev:bob", &as_user(&bob))
            .unwrap();
        let batch = [("bob-2".to_owned(), bob_doc.export_updates_since(&mark))];
        let frames = push_after_catchup(addr, &room, &bob_token, "dev:bob", &batch).await;
        assert_eq!(frames[0].ty, FrameType::Ack, "header={}", frames[0].header);
        let head = state.db.room_state(&room).unwrap().head;
        let relay_rows = || {
            super::build_room_doc(&state, &room)
                .unwrap()
                .doc
                .projected_rows()
        };

        // Alice's device holds the chat and, offline, rewrites Bob's message: not hers to do.
        let mut doc = ChatDoc::new(Some(5), false);
        doc.import(&bob_doc.snapshot()).unwrap();
        let mut session = RoomSession::new("dev:alice", room.clone(), head, Vec::new());
        let mark = doc.oplog_vv();
        doc.set_message_fields("m-bob", &[("id", json!("m-other"))]);
        session.on_local_commit(doc.export_updates_since(&mark));
        let seen = run_session(&state, addr, &alice_token, &mut session, &mut doc).await;
        let told: Vec<_> = seen
            .iter()
            .filter_map(|a| match a {
                RoomAction::Rebuild { code, .. } => Some(code.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(told, ["not_yours"], "the app is told once");
        assert!(session.is_closed() && session.pending().is_empty());
        assert_eq!(session.cursor(), 0);
        assert_eq!(
            state.db.room_state(&room).unwrap().head,
            head,
            "nothing stored"
        );

        // The next connection fills the new document from the checkpoint and the row after it,
        // and has nothing to send: no resend, no heal, no refusal.
        session.prepare_reconnect();
        let seen = run_session(&state, addr, &alice_token, &mut session, &mut doc).await;
        assert!(seen
            .iter()
            .any(|a| matches!(a, RoomAction::FetchCheckpoint { .. })));
        assert!(!seen.iter().any(|a| matches!(
            a,
            RoomAction::Rebuild { .. }
                | RoomAction::PushRefused { .. }
                | RoomAction::SessionError { .. }
                | RoomAction::Pending(_)
        )));
        assert!(session.is_live());
        assert_eq!(doc.message_ids(), ["m-bob", "m-bob-2"]);
        assert_eq!(doc.projected_rows(), relay_rows());

        // What she writes now is stored under her name, and the two documents still agree.
        session.on_close(0);
        session.prepare_reconnect();
        let mark = doc.oplog_vv();
        doc.append_user_message("m-alice", "hi", "dev:alice", &as_user(&alice))
            .unwrap();
        session.on_local_commit(doc.export_updates_since(&mark));
        run_session(&state, addr, &alice_token, &mut session, &mut doc).await;
        assert!(session.is_live() && session.pending().is_empty());
        let chat = room.strip_prefix("people/").unwrap();
        assert_eq!(
            state.db.message_author(chat, "m-alice").unwrap(),
            Some(format!("user:{alice}"))
        );
        assert_eq!(doc.message_ids(), ["m-bob", "m-bob-2", "m-alice"]);
        assert_eq!(doc.projected_rows(), relay_rows());
    }

    /// A push is judged by what importing it changes, so one that is not applied in full is
    /// not stored, and nothing of it reaches the room when the rest arrives later.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_push_that_is_not_fully_applied_is_not_stored() {
        let state = test_state(AuthMode::Github);
        let (room, bob_doc, alice_token, bob_token) = chat_with_bobs_message(&state);
        let head = state.db.room_state(&room).unwrap().head;

        let mut ahead = ChatDoc::new(Some(8), false);
        ahead.import(&bob_doc.snapshot()).unwrap();
        let base = ahead.oplog_vv();
        ahead.set_message_fields("m-bob", &[("seen", json!(true))]);
        let mut early = ChatDoc::new(Some(99), false);
        early.import(&ahead.snapshot()).unwrap();
        let mark = early.oplog_vv();
        early
            .append_user_message("m-early", "hi", "dev:alice", &UserMessageOpts::default())
            .unwrap();

        let addr = serve(state.clone()).await;
        let first = push_after_catchup(
            addr,
            &room,
            &alice_token,
            "dev:alice",
            &[("early".into(), early.export_updates_since(&mark))],
        )
        .await;
        assert_eq!(first[0].ty, FrameType::Error, "header={}", first[0].header);
        assert_eq!(first[0].header["code"], "incomplete");
        assert_eq!(first[0].header["batchId"], "early");

        let second = push_after_catchup(
            addr,
            &room,
            &bob_token,
            "dev:bob",
            &[("seen".into(), ahead.export_updates_since(&base))],
        )
        .await;
        assert_eq!(second[0].ty, FrameType::Ack, "header={}", second[0].header);
        let stored = super::build_room_doc(&state, &room).unwrap();
        assert_eq!(
            stored.doc.message_ids(),
            ["m-bob"],
            "the room holds what was stored and nothing else"
        );
        assert_eq!(state.db.room_state(&room).unwrap().head, head + 1);
    }

    /// A push that carries a change the room was holding back together with what that change
    /// waits for is judged whole, as its pusher's.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_push_carrying_a_held_change_and_what_it_waits_for_is_judged_as_its_pushers() {
        let state = test_state(AuthMode::Github);
        let (room, bob_doc, _, bob_token) = chat_with_bobs_message(&state);
        let mut ahead = ChatDoc::new(Some(8), false);
        ahead.import(&bob_doc.snapshot()).unwrap();
        let base = ahead.oplog_vv();
        ahead.set_message_fields("m-bob", &[("seen", json!(true))]);
        let mut held = ChatDoc::new(Some(99), false);
        held.import(&ahead.snapshot()).unwrap();
        let mark = held.oplog_vv();
        held.set_message_fields("m-bob", &[("id", json!("m-other"))]);
        state
            .db
            .append_row(&room, "dev:old", "held", &held.export_updates_since(&mark))
            .unwrap();
        let head = state.db.room_state(&room).unwrap().head;

        let frames = push_after_catchup(
            serve(state.clone()).await,
            &room,
            &bob_token,
            "dev:bob",
            &[("both".into(), held.export_updates_since(&base))],
        )
        .await;
        assert_eq!(
            frames[0].ty,
            FrameType::Error,
            "header={}",
            frames[0].header
        );
        assert_eq!(frames[0].header["code"], "not_yours");
        assert_eq!(state.db.room_state(&room).unwrap().head, head);
    }

    /// History the room no longer holds cannot be built on: the push is not stored, and the
    /// answer is the one a device stays connected on and waits after, for the push and for
    /// the heal that would carry the same changes. Bytes that are no update get the same.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_push_the_room_cannot_read_gets_an_answer_a_device_waits_on() {
        let state = test_state(AuthMode::Github);
        let (room, bob_doc, alice_token, _) = chat_with_bobs_message(&state);
        let mut alice_doc = ChatDoc::new(Some(5), false);
        alice_doc.import(&bob_doc.snapshot()).unwrap();
        let mark = bob_doc.oplog_vv();
        bob_doc
            .append_user_message("m-bob-2", "more", "dev:bob", &UserMessageOpts::default())
            .unwrap();
        state
            .db
            .append_row(&room, "dev:bob", "b2", &bob_doc.export_updates_since(&mark))
            .unwrap();
        crate::notices::compact_shallow(&state, None, room.strip_prefix("people/").unwrap())
            .unwrap();
        let head = state.db.room_state(&room).unwrap().head;

        let mark = alice_doc.oplog_vv();
        alice_doc
            .append_user_message("m-alice", "hi", "dev:alice", &UserMessageOpts::default())
            .unwrap();
        let batch = alice_doc.export_updates_since(&mark);
        let relay_vv = super::build_room_doc(&state, &room).unwrap().doc.oplog_vv();
        let heal = alice_doc.updates_beyond(&relay_vv);
        let addr = serve(state.clone()).await;
        for (id, bytes, is_heal) in [
            ("a1", batch, false),
            ("h1", heal, true),
            ("junk", b"not an update".to_vec(), false),
        ] {
            let f = push_after_catchup_with_heal(
                addr,
                &room,
                &alice_token,
                "dev:alice",
                &[(id.into(), bytes)],
                is_heal,
            )
            .await
            .remove(0);
            assert_eq!(f.ty, FrameType::Error, "header={}", f.header);
            assert_eq!(f.header["code"], "not_available", "header={}", f.header);
            assert_eq!(f.header["batchId"], id);
            assert!(f.header.get("reason").is_none());
        }
        assert_eq!(state.db.room_state(&room).unwrap().head, head);
    }

    /// The record of who wrote what failing to read is not an empty record: the push is
    /// refused and nothing is stored.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_push_is_refused_while_the_author_record_cannot_be_read() {
        let state = test_state(AuthMode::Github);
        let (room, bob_doc, alice_token, _) = chat_with_bobs_message(&state);
        let head = state.db.room_state(&room).unwrap().head;
        state
            .db
            .with(|c| c.execute_batch("ALTER TABLE message_authors RENAME TO message_authors_gone"))
            .unwrap();
        let mut alice_doc = ChatDoc::new(Some(5), false);
        alice_doc.import(&bob_doc.snapshot()).unwrap();
        let mark = alice_doc.oplog_vv();
        alice_doc
            .append_user_message("m-alice", "hi", "dev:alice", &UserMessageOpts::default())
            .unwrap();
        let frames = push_after_catchup(
            serve(state.clone()).await,
            &room,
            &alice_token,
            "dev:alice",
            &[("a1".into(), alice_doc.export_updates_since(&mark))],
        )
        .await;
        assert_eq!(
            frames[0].ty,
            FrameType::Error,
            "header={}",
            frames[0].header
        );
        assert_eq!(frames[0].header["code"], "unavailable");
        assert_eq!(state.db.room_state(&room).unwrap().head, head);
    }

    /// A device that has not seen the room's latest rows builds on rows the room does hold,
    /// so its push applies in full and is stored like any other.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_push_from_a_device_that_is_behind_is_stored() {
        let state = test_state(AuthMode::Github);
        let (room, bob_doc, alice_token, _) = chat_with_bobs_message(&state);
        let mut alice_doc = ChatDoc::new(Some(5), false);
        alice_doc.import(&bob_doc.snapshot()).unwrap();

        let mark = bob_doc.oplog_vv();
        bob_doc
            .append_user_message("m-bob-2", "more", "dev:bob", &UserMessageOpts::default())
            .unwrap();
        state
            .db
            .append_row(&room, "dev:bob", "b2", &bob_doc.export_updates_since(&mark))
            .unwrap();

        let mark = alice_doc.oplog_vv();
        alice_doc
            .append_user_message("m-alice", "hi", "dev:alice", &UserMessageOpts::default())
            .unwrap();
        alice_doc
            .set_reaction("m-bob", "👍", "user:alice", true)
            .unwrap();
        let batch = alice_doc.export_updates_since(&mark);
        let frames = push_after_catchup(
            serve(state.clone()).await,
            &room,
            &alice_token,
            "dev:alice",
            &[("a1".into(), batch)],
        )
        .await;
        assert_eq!(frames[0].ty, FrameType::Ack, "header={}", frames[0].header);
        assert_eq!(frames[0].header["dup"], false);
        let stored = super::build_room_doc(&state, &room).unwrap();
        // Written side by side, so the doc decides the order of the last two.
        let mut ids = stored.doc.message_ids();
        ids.sort();
        assert_eq!(ids, ["m-alice", "m-bob", "m-bob-2"]);
    }

    /// The honest way to hold a batch the room cannot apply: the room lost a row this device
    /// still has. The batch alone is refused; on the next connection the heal carries the
    /// lost row and the batch together, and the batch then acks as already held.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_refused_batch_arrives_with_the_heal_on_the_next_connection() {
        let state = test_state(AuthMode::Github);
        let (room, bob_doc, alice_token, _) = chat_with_bobs_message(&state);
        let mut alice_doc = ChatDoc::new(Some(5), false);
        alice_doc.import(&bob_doc.snapshot()).unwrap();
        let relay_has = alice_doc.oplog_vv();
        alice_doc
            .append_user_message(
                "m-lost",
                "the room lost this",
                "dev:alice",
                &UserMessageOpts::default(),
            )
            .unwrap();
        let mark = alice_doc.oplog_vv();
        alice_doc
            .append_user_message(
                "m-next",
                "and this builds on it",
                "dev:alice",
                &UserMessageOpts::default(),
            )
            .unwrap();
        let batch = alice_doc.export_updates_since(&mark);
        let heal = alice_doc.updates_beyond(&relay_has);
        let addr = serve(state.clone()).await;

        let refused = push_after_catchup(
            addr,
            &room,
            &alice_token,
            "dev:alice",
            &[("b1".into(), batch.clone())],
        )
        .await;
        assert_eq!(refused[0].ty, FrameType::Error);
        assert_eq!(refused[0].header["code"], "incomplete");

        let healed = push_after_catchup_with_heal(
            addr,
            &room,
            &alice_token,
            "dev:alice",
            &[("heal-1".into(), heal)],
            true,
        )
        .await;
        assert_eq!(healed[0].ty, FrameType::Ack, "header={}", healed[0].header);
        let resent = push_after_catchup(
            addr,
            &room,
            &alice_token,
            "dev:alice",
            &[("b1".into(), batch)],
        )
        .await;
        assert_eq!(resent[0].ty, FrameType::Ack);
        assert_eq!(resent[0].header["dup"], true);
        let stored = super::build_room_doc(&state, &room).unwrap();
        assert_eq!(stored.doc.message_ids(), ["m-bob", "m-lost", "m-next"]);
    }

    /// A row stored before this rule that the room's doc still holds back does not make the
    /// room refuse the pushes that come after it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_room_already_holding_a_row_it_cannot_apply_still_takes_pushes() {
        let state = test_state(AuthMode::Github);
        let (room, bob_doc, alice_token, _) = chat_with_bobs_message(&state);
        let mut gone = ChatDoc::new(Some(8), false);
        gone.import(&bob_doc.snapshot()).unwrap();
        let base = gone.oplog_vv();
        gone.set_message_fields("m-bob", &[("seen", json!(true))]);
        let awaited = gone.export_updates_since(&base);
        let mark = gone.oplog_vv();
        gone.set_message_fields("m-bob", &[("seen", json!(false))]);
        state
            .db
            .append_row(&room, "dev:old", "held", &gone.export_updates_since(&mark))
            .unwrap();

        let mut alice_doc = ChatDoc::new(Some(5), false);
        alice_doc.import(&bob_doc.snapshot()).unwrap();
        let mark = alice_doc.oplog_vv();
        alice_doc
            .append_user_message("m-alice", "hi", "dev:alice", &UserMessageOpts::default())
            .unwrap();
        let batch = alice_doc.export_updates_since(&mark);
        let addr = serve(state.clone()).await;
        let push = |id: &str, bytes: Vec<u8>| {
            let (room, token) = (room.clone(), alice_token.clone());
            let id = id.to_owned();
            async move {
                push_after_catchup(addr, &room, &token, "dev:alice", &[(id, bytes)])
                    .await
                    .remove(0)
            }
        };
        let f = push("a1", batch).await;
        assert_eq!(f.ty, FrameType::Ack, "header={}", f.header);
        assert_eq!(f.header["dup"], false);

        // A push is its own changes and no others. The one the held row waits for would
        // bring the held row in with it, so by itself it is refused and nothing is stored.
        let head = state.db.room_state(&room).unwrap().head;
        let f = push("awaited", awaited).await;
        assert_eq!(f.ty, FrameType::Error, "header={}", f.header);
        assert_eq!(f.header["code"], "incomplete");
        assert_eq!(state.db.room_state(&room).unwrap().head, head);

        // Carried in one push, both are that push's own changes and are judged as such.
        let f = push("both", gone.export_updates_since(&base)).await;
        assert_eq!(f.ty, FrameType::Ack, "header={}", f.header);
        assert_eq!(f.header["dup"], false);
    }

    /// Whoever pushes it and however it is flagged, a push that is not applied in full is
    /// not stored.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_heal_that_is_not_fully_applied_is_not_stored() {
        let state = test_state(AuthMode::Github);
        let (room, bob_doc, alice_token, _) = chat_with_bobs_message(&state);
        let head = state.db.room_state(&room).unwrap().head;
        let mut unseen = ChatDoc::new(Some(8), false);
        unseen.import(&bob_doc.snapshot()).unwrap();
        unseen.set_message_fields("m-bob", &[("seen", json!(true))]);
        let mut alice_doc = ChatDoc::new(Some(5), false);
        alice_doc.import(&unseen.snapshot()).unwrap();
        let mark = alice_doc.oplog_vv();
        alice_doc
            .append_user_message("m-alice", "hi", "dev:alice", &UserMessageOpts::default())
            .unwrap();
        let frames = push_after_catchup_with_heal(
            serve(state.clone()).await,
            &room,
            &alice_token,
            "dev:alice",
            &[("heal-1".into(), alice_doc.export_updates_since(&mark))],
            true,
        )
        .await;
        assert_eq!(
            frames[0].ty,
            FrameType::Error,
            "header={}",
            frames[0].header
        );
        assert_eq!(frames[0].header["code"], "incomplete");
        assert_eq!(state.db.room_state(&room).unwrap().head, head);
    }

    /// A whole snapshot is a payload like any other: applied in full, judged, stored.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_snapshot_pushed_as_a_row_is_judged_and_stored() {
        let state = test_state(AuthMode::Github);
        let (room, bob_doc, alice_token, _) = chat_with_bobs_message(&state);
        let mut alice_doc = ChatDoc::new(Some(5), false);
        alice_doc.import(&bob_doc.snapshot()).unwrap();
        alice_doc
            .append_user_message("m-alice", "hi", "dev:alice", &UserMessageOpts::default())
            .unwrap();
        let addr = serve(state.clone()).await;
        let frames = push_after_catchup(
            addr,
            &room,
            &alice_token,
            "dev:alice",
            &[("snap".into(), alice_doc.snapshot())],
        )
        .await;
        assert_eq!(frames[0].ty, FrameType::Ack, "header={}", frames[0].header);
        assert_eq!(frames[0].header["dup"], false);
        let stored = super::build_room_doc(&state, &room).unwrap();
        assert_eq!(stored.doc.message_ids(), ["m-bob", "m-alice"]);

        // And refused like any other when what it does is not the pusher's to do.
        alice_doc.set_message_fields("m-bob", &[("id", json!("m-other"))]);
        let frames = push_after_catchup(
            addr,
            &room,
            &alice_token,
            "dev:alice",
            &[("snap-2".into(), alice_doc.snapshot())],
        )
        .await;
        assert_eq!(
            frames[0].ty,
            FrameType::Error,
            "header={}",
            frames[0].header
        );
        assert_eq!(frames[0].header["code"], "not_yours");
    }

    /// A sender the people gates refuse keeps their batches and sends them again in order,
    /// and each batch builds on the one before. Every one of them gets the gate's answer,
    /// the one a client stays connected on.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_refused_senders_later_batches_get_the_gates_answer_too() {
        for (gate, code) in [
            ("blocked", "not_available"),
            ("silenced", "not_available"),
            ("over the limit", "rate_limited"),
        ] {
            let state = test_state(AuthMode::Github);
            let (alice, _) = person(&state, 1);
            let (bob, _) = person(&state, 2);
            let chat = dm(&state, &alice, &bob);
            let room = format!("people/{chat}");
            let token = device_token(&state, &alice);
            match gate {
                "blocked" => state.db.block(&bob, &alice).unwrap(),
                "silenced" => {
                    let me = format!("user:{alice}");
                    assert!(state
                        .db
                        .set_member_silenced(&chat, &me, true, None)
                        .unwrap());
                }
                _ => {
                    for _ in 0..200 {
                        state.db.rate_event(&alice, "msg", 60_000).unwrap();
                    }
                }
            }

            let doc = ChatDoc::new(Some(5), false);
            let mut batches = Vec::new();
            for id in ["m1", "m2", "m3"] {
                let mark = doc.oplog_vv();
                doc.append_user_message(id, "hi", "dev:alice", &UserMessageOpts::default())
                    .unwrap();
                batches.push((id.to_owned(), doc.export_updates_since(&mark)));
            }
            let frames = push_after_catchup(
                serve(state.clone()).await,
                &room,
                &token,
                "dev:alice",
                &batches,
            )
            .await;
            for (f, (id, _)) in frames.iter().zip(&batches) {
                assert_eq!(f.ty, FrameType::Error);
                assert_eq!(f.header["code"], code, "{gate}: header={}", f.header);
                assert_eq!(f.header["batchId"], *id);
            }
            assert_eq!(state.db.room_state(&room).unwrap().head, 0);
        }
    }

    /// A batch that starts beyond what the room holds for its own peer is refused from its
    /// bytes alone: the room's doc is neither touched nor rebuilt for it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_batch_that_skips_ahead_is_refused_without_touching_the_rooms_doc() {
        let state = test_state(AuthMode::Github);
        let (name, bob_doc, alice_token, _) = chat_with_bobs_message(&state);
        let room = state.room(&name).await;
        let _keep_cache = room.tx.subscribe();
        super::room_version_vector(&state, &room, &name)
            .await
            .unwrap();

        let mut alice_doc = ChatDoc::new(Some(5), false);
        alice_doc.import(&bob_doc.snapshot()).unwrap();
        alice_doc
            .append_user_message("m1", "hi", "dev:alice", &UserMessageOpts::default())
            .unwrap();
        let mark = alice_doc.oplog_vv();
        alice_doc
            .append_user_message("m2", "hi", "dev:alice", &UserMessageOpts::default())
            .unwrap();
        let frames = push_after_catchup(
            serve(state.clone()).await,
            &name,
            &alice_token,
            "dev:alice",
            &[("b2".into(), alice_doc.export_updates_since(&mark))],
        )
        .await;
        assert_eq!(frames[0].ty, FrameType::Error);
        assert_eq!(frames[0].header["code"], "incomplete");
        assert!(room.doc_cache.lock().unwrap().is_some());
    }

    /// Late poll writes through the socket, one connection per push: each
    /// is acknowledged — a refusal would undo what the device had not yet delivered in
    /// the chat — and each is stored with the relay's correction right behind it, so the
    /// second push is judged against the corrected row and never against the first one's
    /// value (`push_guard::corrections`, `docs/polls.md` §11).
    ///
    /// Bob changes his vote and then retracts it after the close: without the correction
    /// between them the retraction's "what it was" would be the changed vote. Alice moves her
    /// close forward twice: the second move would otherwise defend the first.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn late_poll_writes_are_acked_and_corrected_before_the_next_push() {
        use arbos_core::doc::{POLL_CLOSED_FIELD, POLL_VOTE_AT_PREFIX, POLL_VOTE_PREFIX};
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let room = format!("people/{chat}");
        let now = arbos_core::ids::now_ms();
        let (closed, year) = (now - 300_000, 365 * 24 * 3_600_000);
        let (vote, at) = (
            format!("{POLL_VOTE_PREFIX}user:{bob}"),
            format!("{POLL_VOTE_AT_PREFIX}user:{bob}"),
        );

        // The room holds Alice's poll, Bob's vote for Lisbon ten minutes ago, and her close
        // five minutes ago.
        let writer = ChatDoc::new(Some(7), false);
        let base = writer.oplog_vv();
        writer.init_meta(&chat, "people", 1);
        let poll = arbos_core::poll::Poll::new(
            "Where?",
            &["Lisbon".to_owned(), "Berlin".to_owned()],
            false,
            false,
            None,
        )
        .unwrap();
        let opts = UserMessageOpts {
            sender: Some(format!("user:{alice}")),
            ..Default::default()
        };
        writer
            .append_poll_message("p1", &poll, "dev:alice", &opts)
            .unwrap();
        writer.set_message_fields(
            "p1",
            &[
                (&vote, json!(["1"])),
                (&at, json!(now - 600_000)),
                (POLL_CLOSED_FIELD, json!(closed)),
            ],
        );
        state
            .db
            .append_row(
                &room,
                "dev:alice",
                "seed",
                &writer.export_updates_since(&base),
            )
            .unwrap();
        state
            .db
            .record_message_author(&chat, "p1", &format!("user:{alice}"))
            .unwrap();
        let poll = || {
            let doc = crate::matrix::backfill::load_doc(&state, &chat, None)
                .unwrap()
                .expect("the room's doc");
            doc.projected_rows()[0]["parts"][1].clone()
        };
        assert_eq!(poll()["counts"], json!({ "1": 1 }));

        // Each write is made on a replica of what the relay holds at that moment, as a
        // device that has received the relay's last correction would make it.
        let addr = serve(state.clone()).await;
        let writes: [(&str, &str, &str, serde_json::Value); 4] = [
            (&bob, "dev:bob", &vote, json!(["2"])),
            (&bob, "dev:bob", &vote, json!([])),
            (&alice, "dev:alice2", POLL_CLOSED_FIELD, json!(now + year)),
            (
                &alice,
                "dev:alice2",
                POLL_CLOSED_FIELD,
                json!(now + 2 * year),
            ),
        ];
        for (n, (who, device, key, value)) in writes.into_iter().enumerate() {
            let held = crate::matrix::backfill::load_doc(&state, &chat, None)
                .unwrap()
                .expect("the room's doc");
            let replica =
                ChatDoc::from_snapshot(Some(20 + n as u64), false, &held.snapshot()).unwrap();
            let before = replica.oplog_vv();
            replica.set_message_fields("p1", &[(key, value.clone())]);
            let batch = vec![(format!("late-{n}"), replica.export_updates_since(&before))];
            let head = state.db.room_state(&room).unwrap().head;
            let token = device_token(&state, who);
            let frames = push_after_catchup(addr, &room, &token, device, &batch).await;
            assert_eq!(frames[0].ty, FrameType::Ack, "header={}", frames[0].header);
            assert_eq!(
                state.db.room_state(&room).unwrap().head,
                head + 2,
                "{key}={value}: the push is stored with the relay's row right behind it"
            );
            assert_eq!(poll()["counts"], json!({ "1": 1 }), "{key}={value}");
            assert_eq!(poll()["deadline"], closed, "{key}={value}");
        }

        // Alice deletes her poll. Bob pushes it back under the same id, open again and with
        // a vote in her name: acknowledged, and taken out again in the same step.
        let held = |peer: u64| {
            let doc = crate::matrix::backfill::load_doc(&state, &chat, None)
                .unwrap()
                .expect("the room's doc");
            ChatDoc::from_snapshot(Some(peer), false, &doc.snapshot()).unwrap()
        };
        let replica = held(30);
        let before = replica.oplog_vv();
        replica.transact(|d| d.delete_message("p1").unwrap());
        let delete = vec![("delete".to_owned(), replica.export_updates_since(&before))];
        let token = device_token(&state, &alice);
        let frames = push_after_catchup(addr, &room, &token, "dev:alice2", &delete).await;
        assert_eq!(frames[0].ty, FrameType::Ack, "header={}", frames[0].header);
        assert!(state.db.message_deleted(&chat, "p1").unwrap());

        let replica = held(31);
        let before = replica.oplog_vv();
        let forged = arbos_core::poll::Poll::new(
            "Where?",
            &["Lisbon".to_owned(), "Berlin".to_owned()],
            false,
            false,
            None,
        )
        .unwrap();
        replica.transact(|d| {
            d.append_poll_message("p1", &forged, "dev:bob", &opts)
                .unwrap();
            d.set_message_fields(
                "p1",
                &[
                    (&format!("{POLL_VOTE_PREFIX}user:{alice}"), json!(["2"])),
                    (&format!("{POLL_VOTE_AT_PREFIX}user:{alice}"), json!(now)),
                ],
            );
        });
        let back = vec![("back".to_owned(), replica.export_updates_since(&before))];
        let token = device_token(&state, &bob);
        let frames = push_after_catchup(addr, &room, &token, "dev:bob", &back).await;
        assert_eq!(frames[0].ty, FrameType::Ack, "header={}", frames[0].header);
        let doc = crate::matrix::backfill::load_doc(&state, &chat, None)
            .unwrap()
            .expect("the room's doc");
        assert!(
            doc.projected_rows().is_empty(),
            "the deleted poll is not back"
        );
    }

    /// The announce route records its caller as the author of any message id it is given,
    /// row or no row. That record must not make a poll pushed under the id "history a heal
    /// restores": Bob announces a fresh id, then pushes a closed poll under it with a vote in
    /// Alice's name, claiming a heal. Refused. His own poll under an announced id, with his
    /// own vote and a close, is a new message like any other: gated, and stamped by the relay.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_announced_id_does_not_make_a_forged_poll_history() {
        use arbos_core::doc::{POLL_CLOSED_FIELD, POLL_VOTE_AT_PREFIX, POLL_VOTE_PREFIX};
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let room = format!("people/{chat}");
        let writer = ChatDoc::new(Some(7), false);
        let base = writer.oplog_vv();
        writer.init_meta(&chat, "people", 1);
        state
            .db
            .append_row(
                &room,
                "dev:alice",
                "meta",
                &writer.export_updates_since(&base),
            )
            .unwrap();
        let poll = arbos_core::poll::Poll::new(
            "Where?",
            &["Lisbon".to_owned(), "Berlin".to_owned()],
            false,
            true,
            None,
        )
        .unwrap();
        let old = arbos_core::ids::now_ms() - 3_600_000;
        let forged = |id: &str, voter: &str| {
            let replica = ChatDoc::from_snapshot(Some(8), false, &writer.snapshot()).unwrap();
            let before = replica.oplog_vv();
            let opts = UserMessageOpts {
                sender: Some(format!("user:{bob}")),
                ..Default::default()
            };
            replica.transact(|d| {
                d.append_poll_message(id, &poll, "dev:bob", &opts).unwrap();
                d.set_message_fields(
                    id,
                    &[
                        (&format!("{POLL_VOTE_PREFIX}user:{voter}"), json!(["2"])),
                        (&format!("{POLL_VOTE_AT_PREFIX}user:{voter}"), json!(old)),
                        (POLL_CLOSED_FIELD, json!(old + 1)),
                    ],
                );
            });
            vec![(format!("push-{id}"), replica.export_updates_since(&before))]
        };
        let announce = |id: &str| {
            state
                .db
                .insert_chat_event(&chat, None, id, Some(&bob), &format!("user:{bob}"))
                .unwrap();
            assert_eq!(
                state.db.message_stored(&chat, id).unwrap(),
                Some(false),
                "announced, not stored"
            );
        };
        let addr = serve(state.clone()).await;
        let token = device_token(&state, &bob);

        announce("p_forged");
        let frames = push_after_catchup_with_heal(
            addr,
            &room,
            &token,
            "dev:bob",
            &forged("p_forged", &alice),
            true,
        )
        .await;
        assert_eq!(
            frames[0].ty,
            FrameType::Error,
            "header={}",
            frames[0].header
        );
        assert_eq!(
            frames[0].header["code"], "not_yours",
            "a vote in Alice's name"
        );
        let held = || {
            crate::matrix::backfill::load_doc(&state, &chat, None)
                .unwrap()
                .expect("the room's doc")
                .projected_rows()
        };
        assert!(held().is_empty(), "nothing of it is stored");

        // His own poll under an announced id, claiming a heal all the same.
        announce("p_own");
        let rates = state.db.rate_count(&bob, "msg", 60_000).unwrap();
        let frames = push_after_catchup_with_heal(
            addr,
            &room,
            &token,
            "dev:bob",
            &forged("p_own", &bob),
            true,
        )
        .await;
        assert_eq!(frames[0].ty, FrameType::Ack, "header={}", frames[0].header);
        assert_eq!(
            state.db.rate_count(&bob, "msg", 60_000).unwrap(),
            rates + 1,
            "a new message, counted against the rate limit whatever the heal flag says"
        );
        assert_eq!(
            state.db.message_stored(&chat, "p_own").unwrap(),
            Some(true),
            "stored now"
        );
        let part = held()[0]["parts"][1].clone();
        assert!(
            part["deadline"].as_i64().unwrap() > old + 1_000_000,
            "the close carries the relay's time, not the hour-old one: {part}"
        );
        assert_eq!(
            part["counts"],
            json!({ "2": 1 }),
            "his vote, cast before his close"
        );
    }

    /// Two devices of one person write against the same state, neither having seen the
    /// other or any correction: after the close, one changes the vote and the other retracts
    /// it. Both are stored, in either order, and the count the poll closed with stands. The
    /// second push is judged against what the relay wrote after the first, whichever of the
    /// two the document's own last-write-wins would have kept.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_late_votes_written_against_the_same_state_both_leave_the_count_alone() {
        use arbos_core::doc::{POLL_CLOSED_FIELD, POLL_VOTE_AT_PREFIX, POLL_VOTE_PREFIX};
        for first in [0usize, 1] {
            let state = test_state(AuthMode::Github);
            let (alice, _) = person(&state, 1);
            let (bob, _) = person(&state, 2);
            let chat = dm(&state, &alice, &bob);
            let room = format!("people/{chat}");
            let now = arbos_core::ids::now_ms();
            let (vote, at) = (
                format!("{POLL_VOTE_PREFIX}user:{bob}"),
                format!("{POLL_VOTE_AT_PREFIX}user:{bob}"),
            );
            let writer = ChatDoc::new(Some(7), false);
            let base = writer.oplog_vv();
            writer.init_meta(&chat, "people", 1);
            let poll = arbos_core::poll::Poll::new(
                "Where?",
                &["Lisbon".to_owned(), "Berlin".to_owned()],
                false,
                false,
                None,
            )
            .unwrap();
            let opts = UserMessageOpts {
                sender: Some(format!("user:{alice}")),
                ..Default::default()
            };
            writer
                .append_poll_message("p1", &poll, "dev:alice", &opts)
                .unwrap();
            writer.set_message_fields(
                "p1",
                &[
                    (&vote, json!(["1"])),
                    (&at, json!(now - 600_000)),
                    (POLL_CLOSED_FIELD, json!(now - 300_000)),
                ],
            );
            state
                .db
                .append_row(
                    &room,
                    "dev:alice",
                    "seed",
                    &writer.export_updates_since(&base),
                )
                .unwrap();
            state
                .db
                .record_message_author(&chat, "p1", &format!("user:{alice}"))
                .unwrap();

            // Both replicas start from the seed. The second writes its field several times,
            // so its last write outranks the relay's first correction in the document.
            let snapshot = writer.snapshot();
            let late = |peer: u64, value: serde_json::Value, writes: usize| {
                let replica = ChatDoc::from_snapshot(Some(peer), false, &snapshot).unwrap();
                let before = replica.oplog_vv();
                for _ in 0..writes {
                    replica.set_message_fields("p1", &[(&vote, json!(["1", "x"]))]);
                }
                replica.set_message_fields("p1", &[(&vote, value)]);
                vec![(
                    format!("late-{peer}"),
                    replica.export_updates_since(&before),
                )]
            };
            let mut pushes = [late(8, json!(["2"]), 0), late(9, json!([]), 6)];
            pushes.rotate_left(first);
            let addr = serve(state.clone()).await;
            let token = device_token(&state, &bob);
            for (n, batch) in pushes.iter().enumerate() {
                let frames =
                    push_after_catchup(addr, &room, &token, &format!("dev:bob{n}"), batch).await;
                assert_eq!(frames[0].ty, FrameType::Ack, "header={}", frames[0].header);
                let doc = crate::matrix::backfill::load_doc(&state, &chat, None)
                    .unwrap()
                    .expect("the room's doc");
                assert_eq!(
                    doc.projected_rows()[0]["parts"][1]["counts"],
                    json!({ "1": 1 }),
                    "order {first}, push {n}"
                );
            }
        }
    }

    /// A replica of what the relay holds for `chat`, as a device that is caught up has it.
    fn held(state: &AppState, chat: &str, peer: u64) -> ChatDoc {
        let doc = crate::matrix::backfill::load_doc(state, chat, None)
            .unwrap()
            .expect("the room's doc");
        ChatDoc::from_snapshot(Some(peer), false, &doc.snapshot()).unwrap()
    }

    /// One push from `user`: `write` made on a caught-up replica, sent over a real socket.
    /// The first frame the relay answers with.
    async fn poll_push(
        state: &AppState,
        addr: SocketAddr,
        chat: &str,
        user: &str,
        peer: u64,
        write: impl FnOnce(&ChatDoc),
    ) -> Frame {
        poll_push_as(state, addr, chat, user, peer, false, write).await
    }

    /// [`poll_push`], with the push calling itself a heal or not.
    async fn poll_push_as(
        state: &AppState,
        addr: SocketAddr,
        chat: &str,
        user: &str,
        peer: u64,
        heal: bool,
        write: impl FnOnce(&ChatDoc),
    ) -> Frame {
        let replica = held(state, chat, peer);
        let before = replica.oplog_vv();
        replica.transact(write);
        let batch = vec![(format!("b-{peer}"), replica.export_updates_since(&before))];
        let token = device_token(state, user);
        let device = format!("dev:{peer}");
        let room = format!("people/{chat}");
        push_after_catchup_with_heal(addr, &room, &token, &device, &batch, heal)
            .await
            .remove(0)
    }

    /// One write a test makes on a replica before pushing it.
    type Write = Box<dyn FnOnce(&ChatDoc) + Send>;

    fn open_poll() -> arbos_core::poll::Poll {
        let labels = ["Lisbon".to_owned(), "Berlin".to_owned()];
        arbos_core::poll::Poll::new("Where?", &labels, false, false, None).unwrap()
    }

    /// The poll rules no honest device breaks are refused by the socket handler itself, with
    /// nothing stored: a close by someone who is not the maker, a vote in another member's
    /// name, more choices than the poll allows, an edited poll, and a new poll that closes
    /// later than a poll may run.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn poll_writes_no_honest_device_makes_are_refused_on_the_socket() {
        use arbos_core::doc::{
            POLL_CLOSED_FIELD, POLL_FIELD, POLL_VOTE_AT_PREFIX, POLL_VOTE_PREFIX,
        };
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let room = format!("people/{chat}");
        let addr = serve(state.clone()).await;
        let now = arbos_core::ids::now_ms();
        let as_alice = UserMessageOpts {
            sender: Some(format!("user:{alice}")),
            ..Default::default()
        };

        // Alice posts her poll through the socket, so the relay stored and attested it.
        let writer = ChatDoc::new(Some(7), false);
        let base = writer.oplog_vv();
        writer.init_meta(&chat, "people", 1);
        writer
            .append_poll_message("p1", &open_poll(), "dev:alice", &as_alice)
            .unwrap();
        let batch = vec![("post".to_owned(), writer.export_updates_since(&base))];
        let token = device_token(&state, &alice);
        let frames = push_after_catchup(addr, &room, &token, "dev:alice", &batch).await;
        assert_eq!(frames[0].ty, FrameType::Ack, "header={}", frames[0].header);

        let vote = |who: &str, picked: serde_json::Value| {
            let (who, at) = (who.to_owned(), json!(now));
            move |d: &ChatDoc| {
                d.set_message_fields(
                    "p1",
                    &[
                        (&format!("{POLL_VOTE_PREFIX}user:{who}"), picked),
                        (&format!("{POLL_VOTE_AT_PREFIX}user:{who}"), at),
                    ],
                )
            }
        };
        let edited = arbos_core::poll::Poll {
            question: "Where to?".to_owned(),
            ..open_poll()
        };
        let hour = 3_600_000i64;
        let timed = arbos_core::poll::Poll {
            closes_in: Some(hour),
            ..open_poll()
        };
        let late = arbos_core::poll::Poll {
            closes_in: Some(arbos_core::poll::DURATION_MAX_MS + 1),
            ..open_poll()
        };
        let as_alice_again = as_alice.clone();
        let refused: [(&str, &str, &str, Write); 5] = [
            (
                "a close by a member who did not make the poll",
                &bob,
                "not_yours",
                Box::new(move |d| d.set_message_fields("p1", &[(POLL_CLOSED_FIELD, json!(now))])),
            ),
            (
                "a vote in another member's name",
                &bob,
                "not_yours",
                Box::new(vote(&alice, json!(["1"]))),
            ),
            (
                "two choices where the poll allows one",
                &bob,
                "bad_vote",
                Box::new(vote(&bob, json!(["1", "2"]))),
            ),
            (
                "an edited poll",
                &alice,
                "poll_fixed",
                Box::new(move |d| {
                    let edited = serde_json::to_value(&edited).unwrap();
                    d.set_message_fields("p1", &[(POLL_FIELD, edited)])
                }),
            ),
            (
                "a new poll that runs longer than a week",
                &alice,
                "bad_poll",
                Box::new(move |d| {
                    d.append_poll_message("p2", &late, "dev:alice", &as_alice)
                        .unwrap()
                }),
            ),
        ];
        let head = state.db.room_state(&room).unwrap().head;
        for (n, (what, who, code, write)) in refused.into_iter().enumerate() {
            let frame = poll_push(&state, addr, &chat, who, 40 + n as u64, write).await;
            assert_eq!(frame.ty, FrameType::Error, "{what}: {}", frame.header);
            assert_eq!(frame.header["code"], code, "{what}");
            assert_eq!(state.db.room_state(&room).unwrap().head, head, "{what}");
        }

        // The same socket takes the honest version of each: Bob's own single vote.
        let frame = poll_push(&state, addr, &chat, &bob, 50, vote(&bob, json!(["1"]))).await;
        assert_eq!(frame.ty, FrameType::Ack, "header={}", frame.header);
        let part = held(&state, &chat, 51).projected_rows()[0]["parts"][1].clone();
        assert_eq!(part["counts"], json!({ "1": 1 }));

        // A one-hour poll from a device half an hour slow, and one from a device more than
        // a day fast: both are taken, and both close an hour from now by the relay's clock.
        for (n, skew) in [-30 * 60_000i64, 26 * hour].into_iter().enumerate() {
            let id = format!("p-skew-{n}");
            let (poll, opts, row) = (timed.clone(), as_alice_again.clone(), id.clone());
            let head = state.db.room_state(&room).unwrap().head;
            let before = arbos_core::ids::now_ms();
            let frame = poll_push(&state, addr, &chat, &alice, 60 + n as u64, move |d| {
                d.append_poll_message(&row, &poll, "dev:alice", &opts)
                    .unwrap();
                let wrote = json!(arbos_core::ids::now_ms() + skew + hour);
                d.set_message_fields(&row, &[(arbos_core::doc::POLL_CLOSES_FIELD, wrote)]);
            })
            .await;
            assert_eq!(frame.ty, FrameType::Ack, "{skew}: {}", frame.header);
            assert_eq!(state.db.room_state(&room).unwrap().head, head + 2, "{skew}");
            let rows = held(&state, &chat, 70 + n as u64).projected_rows();
            let row = rows.iter().find(|r| r["id"] == id.as_str()).unwrap();
            let closes = row["parts"][1]["deadline"].as_i64().unwrap();
            assert!(
                (before + hour..=before + hour + 60_000).contains(&closes),
                "{skew}: closes {} ms after posting",
                closes - before
            );
        }
    }

    /// A member the people gates refuse does not vote or close by calling the push a heal.
    /// Such a push carries no new message, and a heal used to skip the gates: a silenced or
    /// blocked member's vote was stored and broadcast, with the relay's row behind it, at no
    /// rate limit. Each is answered as a message of theirs would be, and nothing is stored.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_silenced_or_blocked_member_cannot_vote_or_close_as_a_heal() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let room = format!("people/{chat}");
        let addr = serve(state.clone()).await;
        let writer = ChatDoc::new(Some(7), false);
        let base = writer.oplog_vv();
        writer.init_meta(&chat, "people", 1);
        let as_alice = UserMessageOpts {
            sender: Some(format!("user:{alice}")),
            ..Default::default()
        };
        writer
            .append_poll_message("p1", &open_poll(), "dev:alice", &as_alice)
            .unwrap();
        let batch = vec![("post".to_owned(), writer.export_updates_since(&base))];
        let token = device_token(&state, &alice);
        let frames = push_after_catchup(addr, &room, &token, "dev:alice", &batch).await;
        assert_eq!(frames[0].ty, FrameType::Ack, "header={}", frames[0].header);
        let head = state.db.room_state(&room).unwrap().head;

        let (voter, maker) = (format!("user:{bob}"), format!("user:{alice}"));
        let vote = |d: &ChatDoc| d.set_poll_vote("p1", &voter, &["1".to_owned()]).unwrap();
        let close = |d: &ChatDoc| d.close_poll("p1", &maker).unwrap();

        assert!(state
            .db
            .set_member_silenced(&chat, &voter, true, None)
            .unwrap());
        for heal in [true, false] {
            let peer = 40 + u64::from(heal);
            let frame = poll_push_as(&state, addr, &chat, &bob, peer, heal, vote).await;
            assert_eq!(frame.ty, FrameType::Error, "heal={heal}: {}", frame.header);
            assert_eq!(frame.header["code"], "not_available", "heal={heal}");
            assert_eq!(frame.header["reason"], "silenced", "heal={heal}");
        }
        assert!(state
            .db
            .set_member_silenced(&chat, &voter, false, None)
            .unwrap());

        // Bob blocks Alice: her close is a write of hers in this chat, and is refused.
        state.db.block(&bob, &alice).unwrap();
        let frame = poll_push_as(&state, addr, &chat, &alice, 42, true, close).await;
        assert_eq!(frame.ty, FrameType::Error, "header={}", frame.header);
        assert_eq!(frame.header["code"], "not_available");
        assert_eq!(
            state.db.room_state(&room).unwrap().head,
            head,
            "nothing stored"
        );
    }

    /// The app offers "Close poll" to the member whose name the row wears; the relay's record
    /// names whoever pushed the row. Where the two differ — a row one member pushed under
    /// another's name, with no record to check it against — the named maker's close is taken
    /// and timed like any other. Refused, it would take the rest of that device's undelivered
    /// writes with it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_maker_named_on_a_poll_closes_it_whoever_the_record_names() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let room = format!("people/{chat}");
        let addr = serve(state.clone()).await;

        // Alice pushes a poll that names Bob as its sender. The record names Alice.
        let writer = ChatDoc::new(Some(7), false);
        let base = writer.oplog_vv();
        writer.init_meta(&chat, "people", 1);
        let as_bob = UserMessageOpts {
            sender: Some(format!("user:{bob}")),
            ..Default::default()
        };
        writer
            .append_poll_message("p1", &open_poll(), "dev:alice", &as_bob)
            .unwrap();
        let batch = vec![("post".to_owned(), writer.export_updates_since(&base))];
        let token = device_token(&state, &alice);
        let frames = push_after_catchup(addr, &room, &token, "dev:alice", &batch).await;
        assert_eq!(frames[0].ty, FrameType::Ack, "header={}", frames[0].header);
        assert_eq!(
            state.db.message_author(&chat, "p1").unwrap(),
            Some(format!("user:{alice}"))
        );

        // Bob's app closes it exactly as `ChatDoc::close_poll` writes it.
        let head = state.db.room_state(&room).unwrap().head;
        let me = format!("user:{bob}");
        let before = arbos_core::ids::now_ms();
        let frame = poll_push(&state, addr, &chat, &bob, 40, |d| {
            d.close_poll("p1", &me).unwrap();
        })
        .await;
        assert_eq!(frame.ty, FrameType::Ack, "header={}", frame.header);
        assert_eq!(state.db.room_state(&room).unwrap().head, head + 2);
        let part = held(&state, &chat, 41).projected_rows()[0]["parts"][1].clone();
        assert!(part["deadline"].as_i64().is_some_and(|at| at >= before));
    }

    /// A map that is not a message cannot stand in for one. Bob hangs a map on his own
    /// message that carries the poll's id, a poll, a vote in Alice's name and a later close,
    /// then rewrites it. The relay judges and corrects a poll only from the row in the
    /// `messages` list: both pushes are stored as they come, the relay writes nothing in
    /// answer, and Alice's closed poll reads exactly as before.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_decoy_map_wearing_a_polls_id_changes_nothing_on_the_poll() {
        use arbos_core::doc::{
            POLL_CLOSED_FIELD, POLL_FIELD, POLL_VOTE_AT_PREFIX, POLL_VOTE_PREFIX,
        };
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let room = format!("people/{chat}");
        let addr = serve(state.clone()).await;
        let now = arbos_core::ids::now_ms();
        let closed = now - 300_000;

        // Alice's poll, closed five minutes ago, and a message of Bob's, both stored.
        let writer = ChatDoc::new(Some(7), false);
        let base = writer.oplog_vv();
        writer.init_meta(&chat, "people", 1);
        let as_user = |user: &str| UserMessageOpts {
            sender: Some(format!("user:{user}")),
            ..Default::default()
        };
        writer
            .append_poll_message("p1", &open_poll(), "dev:alice", &as_user(&alice))
            .unwrap();
        writer.set_message_fields("p1", &[(POLL_CLOSED_FIELD, json!(closed))]);
        writer
            .append_user_message("m-bob", "hello", "dev:bob", &as_user(&bob))
            .unwrap();
        let seed = writer.export_updates_since(&base);
        state
            .db
            .append_row(&room, "dev:alice", "seed", &seed)
            .unwrap();
        for (id, who) in [("p1", &alice), ("m-bob", &bob)] {
            let author = format!("user:{who}");
            state.db.record_message_author(&chat, id, &author).unwrap();
        }
        let poll = || held(&state, &chat, 60).projected_rows()[0]["parts"][1].clone();
        let was = poll();
        assert_eq!(was["deadline"], closed);

        let value = |v: serde_json::Value| serde_json::from_value::<loro::LoroValue>(v).unwrap();
        let (vote, at) = (
            format!("{POLL_VOTE_PREFIX}user:{alice}"),
            format!("{POLL_VOTE_AT_PREFIX}user:{alice}"),
        );
        let year = 365 * 24 * 3_600_000i64;
        let writes: [Write; 2] = [
            Box::new(move |d| {
                let message = d.find_message("m-bob").unwrap();
                let decoy = message
                    .insert_container("decoy", loro::LoroMap::new())
                    .unwrap();
                decoy.insert("id", "p1").unwrap();
                let poll = serde_json::to_value(open_poll()).unwrap();
                decoy.insert(POLL_FIELD, value(poll)).unwrap();
                decoy.insert(POLL_CLOSED_FIELD, now + year).unwrap();
            }),
            Box::new(move |d| {
                let message = d.find_message("m-bob").unwrap();
                let Some(loro::ValueOrContainer::Container(loro::Container::Map(decoy))) =
                    message.get("decoy")
                else {
                    panic!("the decoy is a map on Bob's message");
                };
                decoy.insert(&vote, value(json!(["2"]))).unwrap();
                decoy.insert(&at, now).unwrap();
                decoy.insert(POLL_CLOSED_FIELD, now + 2 * year).unwrap();
            }),
        ];
        for (n, write) in writes.into_iter().enumerate() {
            let head = state.db.room_state(&room).unwrap().head;
            let frame = poll_push(&state, addr, &chat, &bob, 61 + n as u64, write).await;
            assert_eq!(frame.ty, FrameType::Ack, "push {n}: {}", frame.header);
            assert_eq!(
                state.db.room_state(&room).unwrap().head,
                head + 1,
                "push {n}: stored, and the relay wrote no row of its own"
            );
            assert_eq!(poll(), was, "push {n}");
        }
    }

    /// What a push does to the record is written with its row, in one transaction: who each
    /// new message is by, that the relay stored it (an announce cannot say that), and that a
    /// message the push took out is deleted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_push_records_what_it_stored_and_what_it_deleted_with_its_row() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let room = format!("people/{chat}");
        let addr = serve(state.clone()).await;
        let token = device_token(&state, &bob);

        let doc = ChatDoc::new(Some(7), false);
        let base = doc.oplog_vv();
        doc.init_meta(&chat, "people", 1);
        doc.append_user_message("m1", "hello", "dev:bob", &as_user(&bob))
            .unwrap();
        let post = vec![("post".to_owned(), doc.export_updates_since(&base))];
        let frames = push_after_catchup(addr, &room, &token, "dev:bob", &post).await;
        assert_eq!(frames[0].ty, FrameType::Ack, "header={}", frames[0].header);
        assert_eq!(
            state.db.message_author(&chat, "m1").unwrap(),
            Some(format!("user:{bob}"))
        );
        assert_eq!(state.db.message_stored(&chat, "m1").unwrap(), Some(true));
        assert!(!state.db.message_deleted(&chat, "m1").unwrap());

        let base = doc.oplog_vv();
        doc.transact(|d| d.delete_message("m1").unwrap());
        let delete = vec![("delete".to_owned(), doc.export_updates_since(&base))];
        let frames = push_after_catchup(addr, &room, &token, "dev:bob", &delete).await;
        assert_eq!(frames[0].ty, FrameType::Ack, "header={}", frames[0].header);
        assert!(state.db.message_deleted(&chat, "m1").unwrap());
    }

    /// A batch id the room already stored is a dup before the gates (no rate event).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn resent_stored_batch_id_is_dup_before_gates() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let room = format!("people/{chat}");
        let token = device_token(&state, &alice);

        let doc = ChatDoc::new(Some(7), false);
        let before = doc.oplog_vv();
        doc.init_meta(&chat, "people", 1);
        doc.append_user_message("m1", "hi", "dev:alice", &UserMessageOpts::default())
            .unwrap();
        let bytes = doc.export_updates_since(&before);
        let (seq, _) = state
            .db
            .append_row(&room, "dev:alice", "same-batch", &bytes)
            .unwrap();
        let rates_before = state.db.rate_count(&alice, "msg", 60_000).unwrap();

        let addr = serve(state.clone()).await;
        let frames = push_after_catchup(
            addr,
            &room,
            &token,
            "dev:alice",
            &[(("same-batch".into()), bytes)],
        )
        .await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].ty, FrameType::Ack);
        assert_eq!(frames[0].header["dup"], true);
        assert_eq!(frames[0].header["seq"], seq);
        assert_eq!(
            state.db.rate_count(&alice, "msg", 60_000).unwrap(),
            rates_before
        );
    }

    /// Rate-limit refusal carries batchId + retryS; a refused push adds no rate event.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rate_limited_error_has_batch_id_and_retry_s_and_does_not_count() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let room = format!("people/{chat}");
        let token = device_token(&state, &alice);

        // Fill the message window to the young-account limit (half of 200).
        let limit = 100; // young accounts in test_state are brand-new
        for _ in 0..limit {
            assert!(state
                .db
                .rate_check_and_note(&alice, "msg", 60_000, limit)
                .unwrap());
        }
        assert_eq!(state.db.rate_count(&alice, "msg", 60_000).unwrap(), limit);

        let doc = ChatDoc::new(Some(11), false);
        let before = doc.oplog_vv();
        doc.append_user_message("m-new", "fresh", "dev:alice", &UserMessageOpts::default())
            .unwrap();
        let bytes = doc.export_updates_since(&before);
        let batch_id = ulid();

        let addr = serve(state.clone()).await;
        let frames = push_after_catchup(
            addr,
            &room,
            &token,
            "dev:alice",
            &[(batch_id.clone(), bytes)],
        )
        .await;
        assert_eq!(frames.len(), 1);
        let err = &frames[0];
        assert_eq!(err.ty, FrameType::Error);
        assert_eq!(err.header["code"], "rate_limited");
        assert_eq!(err.header["message"], "slow down");
        assert_eq!(err.header["batchId"], batch_id);
        assert!(
            err.header["retryS"].as_i64().unwrap_or(0) >= 1,
            "retryS={}",
            err.header["retryS"]
        );
        assert_eq!(
            state.db.rate_count(&alice, "msg", 60_000).unwrap(),
            limit,
            "refused push must not insert a rate event"
        );
    }

    /// Cached room version vector matches a full rebuild after rows are appended.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cached_vv_matches_full_rebuild_after_append() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let room_name = format!("people/{chat}");
        let room = state.room(&room_name).await;

        let doc = ChatDoc::new(Some(7), false);
        for i in 0..5 {
            let before = doc.oplog_vv();
            doc.append_user_message(&format!("m{i}"), "x", "dev", &UserMessageOpts::default())
                .unwrap();
            state
                .db
                .append_row(
                    &room_name,
                    "dev",
                    &format!("b{i}"),
                    &doc.export_updates_since(&before),
                )
                .unwrap();
        }
        let vv1 = super::room_version_vector(&state, &room, &room_name)
            .await
            .unwrap();
        // Append more rows; the cache must advance by importing only the new ones.
        for i in 5..12 {
            let before = doc.oplog_vv();
            doc.append_user_message(&format!("m{i}"), "y", "dev", &UserMessageOpts::default())
                .unwrap();
            state
                .db
                .append_row(
                    &room_name,
                    "dev",
                    &format!("b{i}"),
                    &doc.export_updates_since(&before),
                )
                .unwrap();
        }
        let vv_cached = super::room_version_vector(&state, &room, &room_name)
            .await
            .unwrap();
        // Full rebuild (ignore cache).
        *room.doc_cache.lock().unwrap() = None;
        let vv_full = super::room_version_vector(&state, &room, &room_name)
            .await
            .unwrap();
        assert_eq!(vv_cached, vv_full);
        assert_ne!(vv1, vv_cached);
        // And it equals encoding the writer's own vv once the room holds the same ops.
        assert_eq!(vv_cached, encode_vv(&doc.oplog_vv()));
    }
    /// A room's sockets all ask for its doc as they catch up; a cold room is rebuilt once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_cold_room_is_rebuilt_once_however_many_sockets_ask() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 11);
        let (bob, _) = person(&state, 12);
        let chat = dm(&state, &alice, &bob);
        let room_name = format!("people/{chat}");
        let room = state.room(&room_name).await;
        let doc = ChatDoc::new(Some(7), false);
        let base = doc.oplog_vv();
        doc.append_user_message("m0", "x", "dev", &UserMessageOpts::default())
            .unwrap();
        state
            .db
            .append_row(&room_name, "dev", "b0", &doc.export_updates_since(&base))
            .unwrap();

        let asks = (0..64).map(|_| super::room_version_vector(&state, &room, &room_name));
        let vvs = futures::future::join_all(asks).await;
        assert!(vvs
            .iter()
            .all(|vv| vv.as_ref().unwrap() == &encode_vv(&doc.oplog_vv())));
        let builds = super::BUILDS.lock().unwrap();
        assert_eq!(builds.iter().filter(|r| **r == room_name).count(), 1);
    }

    async fn prepare_push<'a>(
        state: &AppState,
        room: &'a crate::state::Room,
        name: &'a str,
        payload: &[u8],
    ) -> super::PendingRoomPush<'a> {
        let admission = room.push_admission.lock().await;
        super::push_message_changes(state, room, name, payload, admission)
            .await
            .unwrap()
    }

    fn speculative_fixture(state: &AppState) -> (String, String, String, Vec<u8>) {
        let (alice, _) = person(state, 1);
        let (bob, _) = person(state, 2);
        let chat = dm(state, &alice, &bob);
        let room = format!("people/{chat}");
        let token = device_token(state, &alice);
        let writer = ChatDoc::new(Some(71), false);
        let before = writer.oplog_vv();
        writer.init_meta(&chat, "people", 1);
        writer
            .append_user_message(
                "m-seed",
                "synthetic history",
                "dev:bob",
                &UserMessageOpts {
                    sender: Some(format!("user:{bob}")),
                    ..Default::default()
                },
            )
            .unwrap();
        state
            .db
            .append_row(
                &room,
                "dev:bob",
                "seed",
                &writer.export_updates_since(&before),
            )
            .unwrap();
        state
            .db
            .record_message_author(&chat, "m-seed", &format!("user:{bob}"))
            .unwrap();
        let before = writer.oplog_vv();
        writer
            .append_user_message(
                "m-new",
                "synthetic pending",
                "dev:alice",
                &UserMessageOpts {
                    sender: Some(format!("user:{alice}")),
                    ..Default::default()
                },
            )
            .unwrap();
        (alice, room, token, writer.export_updates_since(&before))
    }

    fn fill_synthetic_rate_window(state: &AppState, alice: &str) {
        for _ in 0..100 {
            assert!(state
                .db
                .rate_check_and_note(alice, "msg", 60_000, 100)
                .unwrap());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn speculative_refusal_cannot_be_reclassified_as_history() {
        let state = test_state(AuthMode::Github);
        let (alice, name, token, payload) = speculative_fixture(&state);
        fill_synthetic_rate_window(&state, &alice);
        let room = state.room(&name).await;
        // Real other subscribers keep the cache warm; last-socket cleanup must not hide a refusal.
        let _keep_cache = room.tx.subscribe();
        let addr = serve(state.clone()).await;
        let first = push_after_catchup_with_heal(
            addr,
            &name,
            &token,
            "dev:alice",
            &[("refused-first".into(), payload.clone())],
            true,
        )
        .await;
        assert_eq!(first[0].ty, FrameType::Error);
        assert_eq!(first[0].header["code"], "rate_limited");
        assert_eq!(state.db.room_state(&name).unwrap().head, 1);
        let second = push_after_catchup_with_heal(
            addr,
            &name,
            &token,
            "dev:alice",
            &[("refused-again".into(), payload)],
            true,
        )
        .await;
        assert_eq!(
            second[0].ty,
            FrameType::Error,
            "the same refused ops are still a new message, not stored history: {}",
            second[0].header
        );
        assert_eq!(second[0].header["code"], "rate_limited");
        assert_eq!(state.db.room_state(&name).unwrap().head, 1);
        assert_eq!(state.db.rate_count(&alice, "msg", 60_000).unwrap(), 100);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn speculative_refusal_keeps_cache_equal_to_stored_doc() {
        let state = test_state(AuthMode::Github);
        let (alice, name, token, payload) = speculative_fixture(&state);
        fill_synthetic_rate_window(&state, &alice);
        let room = state.room(&name).await;
        let _keep_cache = room.tx.subscribe();
        let addr = serve(state.clone()).await;
        let frames = push_after_catchup(
            addr,
            &name,
            &token,
            "dev:alice",
            &[("refused".into(), payload)],
        )
        .await;
        assert_eq!(frames[0].ty, FrameType::Error);
        assert_eq!(frames[0].header["code"], "rate_limited");
        let cached = super::room_version_vector(&state, &room, &name)
            .await
            .unwrap();
        let stored = super::build_room_doc(&state, &name).unwrap();
        assert_eq!(
            cached,
            encode_vv(&stored.doc.oplog_vv()),
            "a refused push must never become the room's advertised stored version"
        );
        assert_eq!(state.db.room_state(&name).unwrap().head, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn speculative_inflight_cache_is_not_visible_to_other_readers() {
        let state = test_state(AuthMode::Github);
        let (_, name, _, payload) = speculative_fixture(&state);
        let room = state.room(&name).await;
        let before = super::room_version_vector(&state, &room, &name)
            .await
            .unwrap();
        // Pause at the actual production preparation/authorization boundary by owning its result.
        let pending = prepare_push(&state, &room, &name, &payload).await;
        let visible = super::room_version_vector(&state, &room, &name)
            .await
            .unwrap();
        assert_eq!(
            visible, before,
            "another reader must not see ops which have no durable admission"
        );
        let peer = room
            .doc_cache
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .doc
            .doc
            .peer_id();
        drop(pending);
        assert_eq!(
            room.doc_cache
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .doc
                .doc
                .peer_id(),
            peer
        );
        assert_eq!(state.db.room_state(&name).unwrap().head, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn speculative_preparations_hold_one_admission_lease() {
        let state = test_state(AuthMode::Github);
        let (_, name, _, payload) = speculative_fixture(&state);
        let room = state.room(&name).await;
        let pending = prepare_push(&state, &room, &name, &payload).await;
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (second_state, second_room, second_name) = (state.clone(), room.clone(), name.clone());
        let second = tokio::spawn(async move {
            let _ = started_tx.send(());
            let prepared = prepare_push(&second_state, &second_room, &second_name, &payload).await;
            let _ = ready_tx.send(());
            drop(prepared);
        });
        started_rx.await.unwrap();
        let raced = tokio::time::timeout(Duration::from_millis(50), ready_rx)
            .await
            .is_ok();
        drop(pending);
        second.await.unwrap();
        assert!(
            !raced,
            "a second preparation cannot authorize against an uncommitted pre-image"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn speculative_aborted_waiter_preserves_owner_and_releases_its_queue_slot() {
        let state = test_state(AuthMode::Github);
        let (_, name, _, payload) = speculative_fixture(&state);
        let room = state.room(&name).await;
        let pending = prepare_push(&state, &room, &name, &payload).await;
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (wait_state, wait_room, wait_name, wait_payload) =
            (state.clone(), room.clone(), name.clone(), payload.clone());
        let waiter = tokio::spawn(async move {
            let _ = started_tx.send(());
            let prepared = prepare_push(&wait_state, &wait_room, &wait_name, &wait_payload).await;
            drop(prepared);
        });
        started_rx.await.unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(
            room.push_admission.try_lock().is_err(),
            "a canceled waiter cannot release the active lease"
        );
        drop(pending);
        let next = tokio::time::timeout(
            Duration::from_secs(1),
            prepare_push(&state, &room, &name, &payload),
        )
        .await
        .unwrap();
        drop(next);
        assert!(room.push_admission.try_lock().is_ok());
    }

    fn append_other_durable_row(state: &AppState, name: &str) {
        let writer = super::build_room_doc(state, name).unwrap().doc;
        let before = writer.oplog_vv();
        writer
            .append_user_message(
                "m-other",
                "synthetic intervening history",
                "dev:relay",
                &UserMessageOpts::default(),
            )
            .unwrap();
        state
            .db
            .append_row(
                name,
                "dev:relay",
                "intervening",
                &writer.export_updates_since(&before),
            )
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn speculative_commit_refreshes_every_intervening_durable_row() {
        let state = test_state(AuthMode::Github);
        let (_, name, _, payload) = speculative_fixture(&state);
        let room = state.room(&name).await;
        let pending = prepare_push(&state, &room, &name, &payload).await;
        // Notice/import paths can append without the websocket admission lease.
        append_other_durable_row(&state, &name);
        let (seq, dup) = state
            .db
            .append_row(&name, "dev:alice", "accepted", &payload)
            .unwrap();
        assert_eq!(seq, 3);
        assert!(!dup);
        pending.commit(&state).unwrap();
        let stored = super::build_room_doc(&state, &name).unwrap();
        let guard = room.doc_cache.lock().unwrap();
        let cached = guard.as_ref().unwrap();
        assert_eq!(cached.key, (3, 0));
        assert_eq!(
            encode_vv(&cached.doc.oplog_vv()),
            encode_vv(&stored.doc.oplog_vv())
        );
        assert_eq!(cached.message_ids, cached.doc.message_identities());
        assert!(cached.message_ids.iter().any(|(id, _)| id == "m-new"));
        assert!(cached.message_ids.iter().any(|(id, _)| id == "m-other"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn speculative_late_commit_and_cold_reader_preserve_a_newer_published_cache() {
        let state = test_state(AuthMode::Github);
        let (_, name, _, payload) = speculative_fixture(&state);
        let room = state.room(&name).await;
        let old_reader = super::build_room_doc(&state, &name).unwrap();
        let pending = prepare_push(&state, &room, &name, &payload).await;
        state
            .db
            .append_row(&name, "dev:alice", "accepted", &payload)
            .unwrap();
        append_other_durable_row(&state, &name);
        let newer = super::build_room_doc(&state, &name).unwrap();
        let peer = newer.doc.doc.peer_id();
        let vv = encode_vv(&newer.doc.oplog_vv());
        *room.doc_cache.lock().unwrap() = Some(newer);
        let key =
            super::publish_room_doc(&state, &room, &name, old_reader, |cached| cached.key).unwrap();
        assert_eq!(key, (3, 0));
        assert_eq!(
            room.doc_cache
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .doc
                .doc
                .peer_id(),
            peer
        );
        pending.commit(&state).unwrap();
        let guard = room.doc_cache.lock().unwrap();
        let cached = guard.as_ref().unwrap();
        assert_eq!(cached.key, (3, 0));
        assert_eq!(
            cached.doc.doc.peer_id(),
            peer,
            "late publication must retain the newer cache object"
        );
        assert_eq!(encode_vv(&cached.doc.oplog_vv()), vv);
    }

    /// Seeded finite native laws over the actual Loro/SQLite/owned-admission boundary.
    /// This supplements the actual socket authorization REDs; it is not an enum/model copy.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn speculative_seeded_native_laws_preserve_durable_visibility_and_replay() {
        let mut seed = 0x05ee_d513_u64;
        let (mut accepted, mut refused) = (0, 0);
        for case in 0..32 {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let reject = seed & 1 == 0;
            let extra = ((seed >> 4) % 4) as usize;
            let replays = 1 + ((seed >> 9) % 3);
            let state = test_state(AuthMode::Github);
            let (_, name, _, payload) = speculative_fixture(&state);
            let room = state.room(&name).await;
            let old_reader = super::build_room_doc(&state, &name).unwrap();
            let stored_before = encode_vv(&old_reader.doc.oplog_vv());
            let pending = prepare_push(&state, &room, &name, &payload).await;
            assert_eq!(
                super::room_version_vector(&state, &room, &name)
                    .await
                    .unwrap(),
                stored_before,
                "uncommitted visibility, case={case} seed={seed}"
            );
            if reject {
                refused += 1;
                drop(pending);
                assert_eq!(
                    super::room_version_vector(&state, &room, &name)
                        .await
                        .unwrap(),
                    stored_before,
                    "refused visibility, case={case} seed={seed}"
                );
            } else {
                accepted += 1;
                for index in 0..extra {
                    let writer = super::build_room_doc(&state, &name).unwrap().doc;
                    let before = writer.oplog_vv();
                    writer
                        .append_user_message(
                            &format!("m-generated-{index}"),
                            "synthetic generated history",
                            "dev:relay",
                            &UserMessageOpts::default(),
                        )
                        .unwrap();
                    state
                        .db
                        .append_row(
                            &name,
                            "dev:relay",
                            &format!("generated-{index}"),
                            &writer.export_updates_since(&before),
                        )
                        .unwrap();
                }
                let (seq, duplicate) = state
                    .db
                    .append_row(&name, "dev:alice", "generated-accepted", &payload)
                    .unwrap();
                assert!(!duplicate);
                pending.commit(&state).unwrap();
                for _ in 0..replays {
                    assert_eq!(
                        state
                            .db
                            .append_row(&name, "dev:alice", "generated-accepted", &payload)
                            .unwrap(),
                        (seq, true),
                        "durable replay, case={case} seed={seed}"
                    );
                }
            }
            let peer = room
                .doc_cache
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .doc
                .doc
                .peer_id();
            super::publish_room_doc(&state, &room, &name, old_reader, |_| ()).unwrap();
            let stored = super::build_room_doc(&state, &name).unwrap();
            let guard = room.doc_cache.lock().unwrap();
            let cached = guard.as_ref().unwrap();
            assert_eq!(
                cached.doc.doc.peer_id(),
                peer,
                "stale publication, case={case} seed={seed}"
            );
            assert_eq!(cached.key, stored.key);
            assert_eq!(
                encode_vv(&cached.doc.oplog_vv()),
                encode_vv(&stored.doc.oplog_vv()),
                "durable completeness, case={case} seed={seed}"
            );
            assert_eq!(
                cached.message_ids,
                cached.doc.message_identities(),
                "mirror identity, case={case} seed={seed}"
            );
        }
        assert_eq!((accepted, refused), (16, 16));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn speculative_authorized_heal_still_commits() {
        let state = test_state(AuthMode::Github);
        let (alice, name, token, payload) = speculative_fixture(&state);
        let room = state.room(&name).await;
        let _keep_cache = room.tx.subscribe();
        let addr = serve(state.clone()).await;
        let frames = push_after_catchup_with_heal(
            addr,
            &name,
            &token,
            "dev:alice",
            &[("authorized".into(), payload)],
            true,
        )
        .await;
        assert_eq!(frames[0].ty, FrameType::Ack);
        assert_eq!(state.db.room_state(&name).unwrap().head, 2);
        let chat = name.strip_prefix("people/").unwrap();
        assert_eq!(
            state.db.message_author(chat, "m-new").unwrap().as_deref(),
            Some(format!("user:{alice}").as_str())
        );
        let cached = super::room_version_vector(&state, &room, &name)
            .await
            .unwrap();
        let stored = super::build_room_doc(&state, &name).unwrap();
        assert_eq!(cached, encode_vv(&stored.doc.oplog_vv()));
    }
}

#[cfg(test)]
mod quarantine_tests {
    //! One pushed row that does not decode used to end compaction for a room permanently: no
    //! snapshot can contain it, so `checkpoint_covers_store` counted it as missing from every
    //! candidate, forever, and `put_checkpoint` is the only path that prunes rows. The bar to
    //! trigger it was "be in the room" — the `Push` handler checks the payload's size and nothing
    //! else. These tests pin the wedge, the repair, and the two ways the repair could be undone.

    use arbos_core::doc::{ChatDoc, UserMessageOpts};

    use super::{checkpoint_covers_store, repair_undecodable_rows, CheckpointVerdict};
    use crate::people::tests_support::{person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AppState;
    use crate::state::AuthMode;

    const GARBAGE: &[u8] = b"this is not a loro update";

    /// A room with `n` real messages, then one undecodable row, then one more real message, so the
    /// bad row sits in the middle of history rather than at its end.
    fn wedged_room(state: &AppState, project: &str, chat: &str) -> (String, ChatDoc, i64) {
        let room = arbos_core::wire::room_name(project, chat);
        let doc = ChatDoc::new(Some(5), false);
        for i in 0..3 {
            let before = doc.oplog_vv();
            doc.append_user_message(
                &format!("m{i}"),
                "hello",
                "dev:phone",
                &UserMessageOpts::default(),
            )
            .unwrap();
            state
                .db
                .append_row(
                    &room,
                    "dev:phone",
                    &format!("b{i}"),
                    &doc.export_updates_since(&before),
                )
                .unwrap();
        }
        let (bad_seq, _) = state
            .db
            .append_row(&room, "dev:bad", "bad1", GARBAGE)
            .unwrap();
        let before = doc.oplog_vv();
        doc.append_user_message("m9", "after", "dev:phone", &UserMessageOpts::default())
            .unwrap();
        state
            .db
            .append_row(&room, "dev:phone", "b9", &doc.export_updates_since(&before))
            .unwrap();
        (room, doc, bad_seq)
    }

    /// The wedge, and that the repair is the exit. The first assertion is the old behaviour: a
    /// snapshot from the replica that holds *every* readable op is still refused.
    #[tokio::test]
    async fn an_undecodable_row_wedges_compaction_until_the_repair_runs() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "Wedge");
        let chat = arbos_core::ids::ulid();
        let (room, doc, bad_seq) = wedged_room(&state, &project, &chat);
        let head = state.db.room_state(&room).unwrap().head;

        // A perfect candidate is refused, and names the bad row as the first thing missing.
        let verdict = checkpoint_covers_store(&state.db, &room, head, &doc.snapshot()).unwrap();
        assert!(
            matches!(&verdict, CheckpointVerdict::Incomplete(why)
                if why.contains(&format!("from seq {bad_seq}"))),
            "{verdict:?}"
        );
        assert_eq!(state.db.room_state(&room).unwrap().row_count, head);

        let repair = repair_undecodable_rows(&state, Some(&project), &chat).unwrap();
        assert_eq!(repair.quarantined, vec![bad_seq]);
        assert_eq!(repair.checkpoint_seq, Some(head));

        // Compaction happened: the rows are gone and the relay's checkpoint covers the bad seq,
        // which is the only thing that retires the row a box is holding as owed.
        let st = state.db.room_state(&room).unwrap();
        assert_eq!((st.row_count, st.floor), (0, head));
        assert!(st.checkpoint_seq >= bad_seq);

        // No message was lost with the row: a full snapshot, not a shallow one.
        let restored = ChatDoc::from_snapshot(
            None,
            false,
            &state.db.checkpoint_bytes(&room).unwrap().unwrap().1,
        )
        .unwrap();
        assert_eq!(restored.messages().len(), doc.messages().len());

        // And the bytes are retained, so the relay's verdict is reviewable rather than final.
        let held = state.db.quarantined_rows(&room).unwrap();
        assert_eq!(held.len(), 1);
        assert_eq!((held[0].seq, held[0].bytes.as_slice()), (bad_seq, GARBAGE));
    }

    /// The inverse of the wedge, and the one change that would bring it back in full: if the
    /// validation walk ever unioned `quarantine`, it would re-import exactly the bytes that cannot
    /// be imported and refuse every candidate again. A later room still compacts with a quarantined
    /// row on file, which is only true while that union does not exist.
    #[tokio::test]
    async fn checkpoint_validation_never_reads_the_quarantine_table() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "Isolation");
        let chat = arbos_core::ids::ulid();
        let (room, doc, bad_seq) = wedged_room(&state, &project, &chat);
        repair_undecodable_rows(&state, Some(&project), &chat).unwrap();
        assert_eq!(state.db.quarantined_rows(&room).unwrap().len(), 1);

        // More history, then a candidate that covers it. Refusal here would mean validation had
        // read the quarantined row back.
        let before = doc.oplog_vv();
        doc.append_user_message("m10", "more", "dev:phone", &UserMessageOpts::default())
            .unwrap();
        state
            .db
            .append_row(
                &room,
                "dev:phone",
                "b10",
                &doc.export_updates_since(&before),
            )
            .unwrap();
        let head = state.db.room_state(&room).unwrap().head;
        assert!(head > bad_seq);
        assert_eq!(
            checkpoint_covers_store(&state.db, &room, head, &doc.snapshot()).unwrap(),
            CheckpointVerdict::Covers
        );
    }

    /// The cross-table duplicate check. Before quarantine existed the deepest layer of this defence
    /// was the `rows_batch` UNIQUE index rather than any query, and a moved row leaves that index
    /// with no error and no diagnostic. SQLite cannot span two tables with one unique index, so the
    /// lookup is the only remaining defence — and a batch that is not recognised is appended at a
    /// **new seq** and broadcast, which the box's seq-keyed bookkeeping cannot account for.
    #[tokio::test]
    async fn a_replayed_batch_whose_row_was_quarantined_is_still_a_duplicate() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "Replay");
        let chat = arbos_core::ids::ulid();
        let (room, _doc, bad_seq) = wedged_room(&state, &project, &chat);
        let head_before = state.db.room_state(&room).unwrap().head;
        assert!(state.db.quarantine_row(&room, bad_seq).unwrap());

        // The batch id is still taken, at the seq it was given.
        assert_eq!(
            state.db.row_seq_for_batch(&room, "bad1").unwrap(),
            Some(bad_seq)
        );
        // And re-pushing it is a dup rather than a second seq.
        assert_eq!(
            state
                .db
                .append_row(&room, "dev:bad", "bad1", GARBAGE)
                .unwrap(),
            (bad_seq, true)
        );
        assert_eq!(state.db.room_state(&room).unwrap().head, head_before);
    }
}

#[cfg(test)]
mod revoked_socket_tests {
    //! A room socket resolves its principal once, at the upgrade. Sign-out therefore has to
    //! reach the connection itself: until the #2460 review a signed-out laptop kept reading its
    //! chats and kept posting rows that appear as the owner, with the relay having already
    //! answered "every session closed".

    use axum::routing::get;
    use axum::Router;
    use futures::{SinkExt, StreamExt};
    use serde_json::json;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as TMsg;

    use arbos_core::doc::{ChatDoc, UserMessageOpts};
    use arbos_core::ids::ulid;
    use arbos_core::wire::{Frame, FrameType};

    use crate::people::tests_support::{dm, person};
    use crate::state::test_support::test_state;
    use crate::state::{AppState, AuthMode};

    async fn serve(state: AppState) -> SocketAddr {
        let app = Router::new()
            .route("/room/{project}/{chat}", get(super::ws_room))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    fn device_token(state: &AppState, user: &str) -> (String, String) {
        let id = ulid();
        let token = format!("adv_{}", ulid().to_ascii_lowercase());
        state
            .db
            .insert_device(&id, "t", &arbos_core::auth::hash(&token), user, "paircode")
            .unwrap();
        (id, token)
    }

    fn decode(m: TMsg) -> Option<Frame> {
        match m {
            TMsg::Binary(b) => Frame::decode(b).ok().flatten(),
            _ => None,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_revoked_device_is_cut_off_its_open_room_socket() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 4601);
        let (bob, _) = person(&state, 4602);
        let chat = dm(&state, &alice, &bob);
        let room = format!("people/{chat}");
        let (dev_id, token) = device_token(&state, &alice);
        let addr = serve(state.clone()).await;

        // The laptop opens its socket and catches up while it is still signed in.
        let url = format!("ws://{addr}/room/{room}?token={token}");
        let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let (mut sink, mut stream) = ws.split();
        sink.send(TMsg::Binary(
            Frame::new(
                FrameType::Hello,
                json!({ "cursor": 0, "device": "dev:laptop", "room": room,
                        "proto": arbos_core::PROTO_VERSION }),
                bytes::Bytes::new(),
            )
            .encode(),
        ))
        .await
        .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let m = tokio::time::timeout_at(deadline, stream.next())
                .await
                .expect("relay answers")
                .unwrap()
                .unwrap();
            let Some(f) = decode(m) else { continue };
            if f.ty == FrameType::State {
                sink.send(TMsg::Binary(
                    Frame::new(
                        FrameType::RowsReq,
                        json!({ "after": 0, "excludeOwn": false }),
                        bytes::Bytes::new(),
                    )
                    .encode(),
                ))
                .await
                .unwrap();
            }
            if f.ty == FrameType::RowsDone {
                break;
            }
        }
        let head_before = state.db.room_state(&room).unwrap().head;

        crate::signin::revoke_device_everywhere(&state, &dev_id)
            .await
            .unwrap();

        // Whatever the laptop does next, the row does not land. The write is attempted without
        // waiting for the close, so this covers the frame that was already in flight as well as
        // the one typed afterwards.
        let doc = ChatDoc::new(Some(9), false);
        let base = doc.oplog_vv();
        doc.init_meta(&chat, "people", 1);
        doc.append_user_message(
            "after-revoke",
            "written by a device the account signed out",
            "dev:laptop",
            &UserMessageOpts::default(),
        )
        .unwrap();
        let _ = sink
            .send(TMsg::Binary(
                Frame::new(
                    FrameType::Push,
                    json!({ "batchId": "after-revoke-1" }),
                    bytes::Bytes::from(doc.export_updates_since(&base)),
                )
                .encode(),
            ))
            .await;

        // The relay closes the socket rather than serving it.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut closed = false;
        while !closed {
            match tokio::time::timeout_at(deadline, stream.next()).await {
                Err(_) => panic!("the socket was neither closed nor errored"),
                Ok(None) => closed = true,
                Ok(Some(Err(_))) => closed = true,
                Ok(Some(Ok(TMsg::Close(_)))) => closed = true,
                Ok(Some(Ok(m))) => assert!(
                    !matches!(decode(m), Some(f) if f.ty == FrameType::Ack),
                    "a revoked device's write was accepted"
                ),
            }
        }
        assert_eq!(
            state.db.room_state(&room).unwrap().head,
            head_before,
            "nothing the revoked device sent was stored"
        );
    }

    /// Adversarial re-review of #2460, M1. The first fix subscribed to `state.revocations`
    /// deep inside `run`, after the upgrade had resolved the principal and several more awaits
    /// had gone by. `Sender::subscribe` marks the current value as seen, so a revocation that
    /// committed anywhere in that span was invisible to that socket **permanently** — and the
    /// per-frame check did not help, because a socket that only listens never sends a frame.
    ///
    /// The window is small, so this dials repeatedly to find it, which is what the app itself
    /// does: the 1008 close provokes a redial. It cannot fail spuriously — a socket reading a
    /// post made after the sign-out is a real defect every time — it can only miss one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn a_socket_opening_as_the_sign_out_lands_does_not_read_on_regardless() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 4603);
        let (bob, _) = person(&state, 4604);
        let chat = dm(&state, &alice, &bob);
        let room = format!("people/{chat}");
        let (dev_id, token) = device_token(&state, &alice);
        let addr = serve(state.clone()).await;

        let url = format!("ws://{addr}/room/{room}?token={token}");
        let mut dialers = Vec::new();
        for _ in 0..16 {
            let url = url.clone();
            dialers.push(tokio::spawn(async move {
                let mut held = Vec::new();
                // Until the bearer stops resolving, which is when the revocation has landed.
                while let Ok((ws, _)) = tokio_tungstenite::connect_async(url.clone()).await {
                    held.push(ws);
                    if held.len() > 2 {
                        held.remove(0);
                    }
                }
                held
            }));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        crate::signin::revoke_device_everywhere(&state, &dev_id)
            .await
            .unwrap();

        let mut sockets = Vec::new();
        for d in dialers {
            sockets.extend(d.await.unwrap());
        }
        // Somebody still in the chat posts. No socket of the signed-out device may see it.
        let _ = state
            .room(&room)
            .await
            .tx
            .send(crate::state::RoomBroadcast {
                from_conn: u64::MAX,
                bytes: Frame::new(
                    FrameType::Status,
                    json!({ "after_sign_out": true }),
                    bytes::Bytes::new(),
                )
                .encode(),
                seq: 0,
            });

        for mut ws in sockets {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            // Anything that is not a binary frame — closed, errored or silent — is fine: none
            // of them is the post being read.
            while let Ok(Some(Ok(TMsg::Binary(b)))) =
                tokio::time::timeout_at(deadline, ws.next()).await
            {
                let seen = matches!(Frame::decode(b), Ok(Some(f))
                    if f.header.get("after_sign_out").is_some());
                assert!(
                    !seen,
                    "a signed-out device read a post made after the sign-out"
                );
            }
        }
    }
}

#[cfg(test)]
mod oversized_push_tests {
    //! A push too big for one frame has to come back as a refusal the pusher can act on.
    //!
    //! Jacob, Test5 round 3, 2026-11-04: a research job's backlog crossed the socket's own
    //! limit, tungstenite dropped the connection without a close frame, and the room reset
    //! about once a second for twenty minutes while the same bytes were re-sent after every
    //! reconnect. Nothing on either side said "too big" — the client could only see a reset,
    //! which is what a dead link looks like too.

    use axum::routing::get;
    use axum::Router;
    use futures::{SinkExt, StreamExt};
    use serde_json::json;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as TMsg;

    use arbos_core::ids::{now_ms, ulid};
    use arbos_core::wire::{self, Frame, FrameType};

    use crate::db::Chat;
    use crate::people::tests_support::{person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::{AppState, AuthMode};

    async fn serve(state: AppState) -> SocketAddr {
        let app = Router::new()
            .route("/room/{project}/{chat}", get(super::ws_room))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    fn device_token(state: &AppState, user: &str) -> String {
        let token = format!("adv_{}", ulid().to_ascii_lowercase());
        state
            .db
            .insert_device(
                &ulid(),
                "t",
                &arbos_core::auth::hash(&token),
                user,
                "paircode",
            )
            .unwrap();
        token
    }

    fn new_chat(state: &AppState, project: &str) -> String {
        let chat = ulid();
        state
            .db
            .insert_chat(&Chat {
                id: chat.clone(),
                project_id: project.to_owned(),
                title: "t".into(),
                is_pinned: false,
                position: 0,
                created_at: now_ms(),
                doc_epoch: 1,
                kind: "chat".into(),
                participants: Vec::new(),
                forked_from: None,
            })
            .unwrap();
        chat
    }

    /// Push `payload_len` bytes and report what came back: the answer frame, or the transport
    /// error the client saw if the socket died instead.
    async fn push_of(payload_len: usize) -> Result<(Frame, String), String> {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "Oversize");
        let chat = new_chat(&state, &project);
        let room = format!("{project}/{chat}");
        let token = device_token(&state, &alice);
        let addr = serve(state.clone()).await;
        let url = format!("ws://{addr}/room/{room}?token={token}");
        let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let (mut sink, mut stream) = ws.split();
        sink.send(TMsg::Binary(
            Frame::new(
                FrameType::Hello,
                json!({ "cursor": 0, "device": "dev:probe", "room": room, "proto": arbos_core::PROTO_VERSION }),
                bytes::Bytes::new(),
            )
            .encode(),
        ))
        .await
        .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let decode = |m: TMsg| match m {
            TMsg::Binary(b) => Frame::decode(b).ok().flatten(),
            _ => None,
        };
        loop {
            let m = tokio::time::timeout_at(deadline, stream.next())
                .await
                .expect("relay answers")
                .unwrap()
                .unwrap();
            if decode(m).is_some_and(|f| matches!(f.ty, FrameType::State)) {
                break;
            }
        }
        let batch = ulid();
        sink.send(TMsg::Binary(
            Frame::new(
                FrameType::Push,
                json!({ "batchId": batch }),
                bytes::Bytes::from(vec![7u8; payload_len]),
            )
            .encode(),
        ))
        .await
        .map_err(|e| e.to_string())?;
        loop {
            match tokio::time::timeout_at(deadline, stream.next()).await {
                Err(_) => return Err("no answer at all".into()),
                Ok(None) => return Err("socket closed silently".into()),
                Ok(Some(Err(e))) => return Err(e.to_string()),
                Ok(Some(Ok(m))) => {
                    if let Some(f) = decode(m) {
                        if matches!(f.ty, FrameType::Error | FrameType::Ack) {
                            return Ok((f, batch));
                        }
                    }
                }
            }
        }
    }

    /// The refusal must name the batch. Without it agentd cannot match the error to the push
    /// it is waiting on, so it leaves the batch in flight and tears down a working session ten
    /// seconds later on "push not acked" — the same defect #2749 fixed for every other room
    /// refusal, which this one was missed out of.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_oversized_push_is_refused_by_name_and_keeps_the_socket() {
        let (f, batch) = push_of(wire::ROW_MAX + 1)
            .await
            .expect("the relay answers rather than dropping the link");
        assert_eq!(f.ty, FrameType::Error, "expected a refusal, got {:?}", f.ty);
        assert_eq!(
            f.header.get("code").and_then(|v| v.as_str()),
            Some("too_large")
        );
        assert_eq!(
            f.header.get("batchId").and_then(|v| v.as_str()),
            Some(batch.as_str()),
            "the refusal does not say which batch it refused: {}",
            f.header
        );
        assert!(
            f.header
                .get("message")
                .and_then(|v| v.as_str())
                .is_some_and(|m| m.contains(&(wire::ROW_MAX + 1).to_string())),
            "the refusal does not say how big the push was: {}",
            f.header
        );
    }

    /// The size that used to kill the socket in silence. It is still too big to store, but now
    /// it comes back as a refusal instead of a reset the client cannot tell from a dead link.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_size_that_reset_test5_now_gets_an_answer() {
        let was_fatal = wire::ROW_MAX + 8192 + 1;
        match push_of(was_fatal).await {
            Ok((f, _)) => assert_eq!(
                f.header.get("code").and_then(|v| v.as_str()),
                Some("too_large"),
                "unexpected answer: {}",
                f.header
            ),
            Err(e) => panic!("the link died instead of answering: {e}"),
        }
    }
}

#[cfg(test)]
mod jam_recovery_tests {
    //! End to end for the jam that cost live replies (#2751): a box's own assistant row written
    //! by an older agentd carries the bare sender `agent`, which the relay could never
    //! attribute — so it held no author and refused the box's own removal of the row.
    //!
    //! The box cannot skip a refused batch, because Loro needs its ops gap-free, so the refused
    //! removal held every op written behind it. This asserts the end the owner asked for, and
    //! not the gate in front of it: the queued reply is *in the room's stored doc* afterwards.

    use axum::routing::get;
    use axum::Router;
    use futures::{SinkExt, StreamExt};
    use serde_json::json;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as TMsg;

    use arbos_core::auth::hash;
    use arbos_core::doc::{ChatDoc, MessageStatus};
    use arbos_core::ids::{now_ms, ulid};
    use arbos_core::wire::{Frame, FrameType};

    use crate::db::{Chat, Sandbox};
    use crate::people::tests_support::{person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::{AppState, AuthMode};

    async fn serve(state: AppState) -> SocketAddr {
        let app = Router::new()
            .route("/room/{project}/{chat}", get(super::ws_room))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    /// A box of `user`'s, and the raw token it dials with.
    fn box_token(state: &AppState, user: &str) -> String {
        let token = format!("box_{}", ulid().to_ascii_lowercase());
        state
            .db
            .upsert_box(&Sandbox {
                user_id: user.to_owned(),
                provider: "local".into(),
                sandbox_id: Some(ulid()),
                daytona_state: None,
                device_id: String::new(),
                box_token_hash: hash(&token),
                mcp_token: ulid(),
                snapshot: None,
                last_started_at: None,
                last_stopped_at: None,
                last_seen_at: None,
                box_url: None,
                preview_token: None,
                local_pid: None,
                migrated_from: Vec::new(),
            })
            .unwrap();
        token
    }

    fn new_chat(state: &AppState, project: &str) -> String {
        let chat = ulid();
        state
            .db
            .insert_chat(&Chat {
                id: chat.clone(),
                project_id: project.to_owned(),
                title: "t".into(),
                is_pinned: false,
                position: 0,
                created_at: now_ms(),
                doc_epoch: 1,
                kind: "chat".into(),
                participants: Vec::new(),
                forked_from: None,
            })
            .unwrap();
        chat
    }

    async fn push(addr: SocketAddr, room: &str, token: &str, batch: &str, bytes: Vec<u8>) -> Frame {
        let url = format!("ws://{addr}/room/{room}?token={token}");
        let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let (mut sink, mut stream) = ws.split();
        sink.send(TMsg::Binary(
            Frame::new(
                FrameType::Hello,
                json!({
                    "cursor": 0,
                    "device": "box:test",
                    "room": room,
                    "proto": arbos_core::PROTO_VERSION
                }),
                bytes::Bytes::new(),
            )
            .encode(),
        ))
        .await
        .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let decode = |m: TMsg| match m {
            TMsg::Binary(b) => Frame::decode(b).ok().flatten(),
            _ => None,
        };
        loop {
            let m = tokio::time::timeout_at(deadline, stream.next())
                .await
                .expect("relay answers")
                .unwrap()
                .unwrap();
            let Some(f) = decode(m) else { continue };
            match f.ty {
                FrameType::State => {
                    sink.send(TMsg::Binary(
                        Frame::new(
                            FrameType::RowsReq,
                            json!({ "after": 0, "excludeOwn": false }),
                            bytes::Bytes::new(),
                        )
                        .encode(),
                    ))
                    .await
                    .unwrap();
                }
                FrameType::RowsDone => break,
                _ => {}
            }
        }
        sink.send(TMsg::Binary(
            Frame::new(
                FrameType::Push,
                json!({ "batchId": batch }),
                bytes::Bytes::from(bytes),
            )
            .encode(),
        ))
        .await
        .unwrap();
        loop {
            let m = tokio::time::timeout_at(deadline, stream.next())
                .await
                .expect("ack or error")
                .unwrap()
                .unwrap();
            if let Some(f) = decode(m) {
                if matches!(f.ty, FrameType::Ack | FrameType::Error) {
                    return f;
                }
            }
        }
    }

    /// A box is held to the same rule as a device: a push that is not applied in full is
    /// not stored.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_box_push_that_is_not_fully_applied_is_not_stored() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "Held");
        let chat = new_chat(&state, &project);
        let room = format!("{project}/{chat}");
        let token = box_token(&state, &alice);
        let addr = serve(state.clone()).await;

        let unseen = ChatDoc::new(Some(7), false);
        unseen.init_meta("c", "p", 1);
        let mut doc = ChatDoc::new(Some(9), false);
        doc.import(&unseen.snapshot()).unwrap();
        let mark = doc.oplog_vv();
        doc.start_assistant_message("m_reply", "", "box:test");
        let f = push(
            addr,
            &room,
            &token,
            &ulid(),
            doc.export_updates_since(&mark),
        )
        .await;
        assert_eq!(f.ty, FrameType::Error, "header={}", f.header);
        assert_eq!(f.header["code"], "incomplete");
        assert_eq!(state.db.room_state(&room).unwrap().head, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_reply_queued_behind_a_refused_removal_reaches_the_rooms_stored_doc() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "Jam");
        let chat = new_chat(&state, &project);
        let room = format!("{project}/{chat}");
        let token = box_token(&state, &alice);
        let addr = serve(state.clone()).await;

        // An older agentd's assistant row: the bare sender the relay can never attribute.
        let doc = ChatDoc::new(Some(7), false);
        let base = doc.oplog_vv();
        doc.init_meta("c", "p", 1);
        doc.start_assistant_message("m_wordless", "", "box:test");
        doc.finish_message("m_wordless", MessageStatus::Done);
        let first = doc.export_updates_since(&base);
        let f = push(addr, &room, &token, &ulid(), first).await;
        assert_eq!(f.ty, FrameType::Ack, "the row itself was never the problem");

        // The batch that used to jam: the box retires its own row, and the reply a person is
        // waiting for is written after it, so it rides in the same batch.
        let mark = doc.oplog_vv();
        doc.delete_message("m_wordless").unwrap();
        doc.append_user_message(
            "m_reply",
            "the answer",
            "box:test",
            &arbos_core::doc::UserMessageOpts::default(),
        )
        .unwrap();
        let second = doc.export_updates_since(&mark);
        let f = push(addr, &room, &token, &ulid(), second).await;
        assert_eq!(
            f.ty,
            FrameType::Ack,
            "the relay refused the batch: {}",
            f.header
        );

        // The assertion nobody has to reason about: the reply is in the room's stored doc,
        // rebuilt from the rows the relay kept, not merely un-refused on the way in.
        let stored = super::build_room_doc(&state, &room).expect("rebuild");
        assert!(
            stored.doc.message("m_reply").is_some(),
            "the reply never reached the room's stored doc"
        );
        assert!(
            stored.doc.message("m_wordless").is_none(),
            "and the row the box retired is gone"
        );
    }
}

#[cfg(test)]
mod socket_bound_tests {
    //! One client must not be able to hold the relay's memory or a socket for good: a peer that
    //! stops reading is dropped, and the device name a HELLO carries is bounded.

    use axum::routing::get;
    use axum::Router;
    use futures::{SinkExt, StreamExt};
    use serde_json::json;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as TMsg;

    use arbos_core::doc::{ChatDoc, UserMessageOpts};
    use arbos_core::ids::ulid;
    use arbos_core::wire::{Frame, FrameType};

    use crate::people::tests_support::{dm, person};
    use crate::state::test_support::test_state;
    use crate::state::{AppState, AuthMode};

    type Ws = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    fn hello(room: &str, device: &str) -> TMsg {
        let header = json!({ "cursor": 0, "device": device, "room": room,
                             "proto": arbos_core::PROTO_VERSION });
        TMsg::Binary(Frame::new(FrameType::Hello, header, bytes::Bytes::new()).encode())
    }

    /// A DM between two people and one socket for each of them on it.
    async fn two_sockets(state: &AppState) -> (String, String, Ws, Ws) {
        let (alice, _) = person(state, 4701);
        let (bob, _) = person(state, 4702);
        let chat = dm(state, &alice, &bob);
        let app = Router::new()
            .route("/room/{project}/{chat}", get(super::ws_room))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut sockets = Vec::new();
        for user in [&alice, &bob] {
            let token = format!("adv_{}", ulid().to_ascii_lowercase());
            let hash = arbos_core::auth::hash(&token);
            state
                .db
                .insert_device(&ulid(), "t", &hash, user, "paircode")
                .unwrap();
            let url = format!("ws://{addr}/room/people/{chat}?token={token}");
            sockets.push(tokio_tungstenite::connect_async(url).await.unwrap().0);
        }
        let bob_ws = sockets.pop().unwrap();
        (
            chat.clone(),
            format!("people/{chat}"),
            sockets.pop().unwrap(),
            bob_ws,
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_socket_whose_peer_stops_reading_is_dropped() {
        let state = test_state(AuthMode::Github);
        let (chat, room, mut idle, mut busy) = two_sockets(&state).await;
        // Alice says hello and never reads again. Her socket stays open on her side.
        idle.send(hello(&room, "dev:idle")).await.unwrap();
        busy.send(hello(&room, "dev:busy")).await.unwrap();
        while state.restart.open_sockets.load(Ordering::Relaxed) < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // Bob sends more than the connection to Alice can buffer.
        let doc = ChatDoc::new(Some(9), false);
        // From before the meta, so the first push carries everything it builds on. The bulk
        // is one large field a push: a megabyte of text would take the relay seconds to apply.
        let mut base = doc.oplog_vv();
        doc.init_meta(&chat, "people", 1);
        doc.append_user_message("m0", "hi", "dev:busy", &UserMessageOpts::default())
            .unwrap();
        let big = json!("x".repeat(900_000));
        for i in 0..40 {
            doc.set_message_fields("m0", &[(format!("f{i}").as_str(), big.clone())]);
            let push = Frame::new(
                FrameType::Push,
                json!({ "batchId": format!("b{i}") }),
                bytes::Bytes::from(doc.export_updates_since(&base)),
            );
            base = doc.oplog_vv();
            busy.send(TMsg::Binary(push.encode())).await.unwrap();
        }
        let reader = tokio::spawn(async move { while busy.next().await.is_some() {} });

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        while state.restart.open_sockets.load(Ordering::Relaxed) > 1 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the socket nobody reads is still held"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        reader.abort();
        drop(idle);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hello_with_an_oversized_device_name_is_refused_and_not_remembered() {
        let state = test_state(AuthMode::Github);
        let (_, room, mut ws, _other) = two_sockets(&state).await;
        ws.send(hello(&room, &"d".repeat(super::DEVICE_NAME_MAX + 1)))
            .await
            .unwrap();
        let answer = tokio::time::timeout(Duration::from_secs(30), ws.next())
            .await
            .expect("the relay answers")
            .unwrap()
            .unwrap();
        let TMsg::Binary(b) = answer else {
            panic!("a frame")
        };
        let f = Frame::decode(b).unwrap().unwrap();
        assert_eq!(
            (f.ty, f.header["code"].as_str()),
            (FrameType::Error, Some("bad_hello"))
        );
        assert!(state.room(&room).await.replicas.lock().unwrap().is_empty());
    }
}
