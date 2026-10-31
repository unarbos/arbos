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

/// A device may push back history the relay lacks once per connection (`PushHeader.heal`).
const HEALS_PER_CONNECTION: u32 = 1;

/// Full rebuild of the room's stored doc (checkpoint ∪ every row). Runs off the socket task via
/// `spawn_blocking` on a cold cache.
fn build_room_doc(state: &AppState, room_name: &str) -> anyhow::Result<crate::state::RoomDocCache> {
    use arbos_core::doc::ChatDoc;
    let st = state.db.room_state(room_name)?;
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
    Ok(crate::state::RoomDocCache {
        key: (st.head, st.floor),
        doc,
    })
}

/// Import only rows after the cached head when the head moved; rebuild when the floor moved.
fn advance_room_doc(
    state: &AppState,
    room_name: &str,
    cached: &mut crate::state::RoomDocCache,
) -> anyhow::Result<()> {
    let st = state.db.room_state(room_name)?;
    let key = (st.head, st.floor);
    if cached.key == key {
        return Ok(());
    }
    if cached.key.1 != st.floor || st.head < cached.key.0 {
        *cached = build_room_doc(state, room_name)?;
        return Ok(());
    }
    let mut after = cached.key.0;
    loop {
        let rows = state.db.rows_after(room_name, after, None, ROWS_BATCH)?;
        let n = rows.len() as i64;
        for r in rows {
            after = r.seq;
            let _ = cached.doc.import(&r.bytes);
        }
        if n < ROWS_BATCH {
            break;
        }
    }
    cached.key = key;
    Ok(())
}

/// Encoded version vector of the room's stored doc (checkpoint ∪ rows). Keeps a ChatDoc on the
/// room: head moves import only new rows; the first build runs in `spawn_blocking`.
async fn room_version_vector(
    state: &AppState,
    room: &crate::state::Room,
    room_name: &str,
) -> anyhow::Result<Vec<u8>> {
    use arbos_core::doc::encode_vv;
    {
        let mut guard = room.doc_cache.lock().unwrap();
        if let Some(cached) = guard.as_mut() {
            advance_room_doc(state, room_name, cached)?;
            return Ok(encode_vv(&cached.doc.oplog_vv()));
        }
    }
    let state_c = state.clone();
    let name = room_name.to_owned();
    let built = tokio::task::spawn_blocking(move || build_room_doc(&state_c, &name))
        .await
        .map_err(|e| anyhow::anyhow!("room doc rebuild: {e}"))??;
    let vv = encode_vv(&built.doc.oplog_vv());
    *room.doc_cache.lock().unwrap() = Some(built);
    Ok(vv)
}

/// Version vector of the room's stored doc (same cache as [`room_version_vector`]).
async fn room_oplog_vv(
    state: &AppState,
    room: &crate::state::Room,
    room_name: &str,
) -> anyhow::Result<arbos_core::doc::VersionVector> {
    {
        let mut guard = room.doc_cache.lock().unwrap();
        if let Some(cached) = guard.as_mut() {
            advance_room_doc(state, room_name, cached)?;
            return Ok(cached.doc.oplog_vv());
        }
    }
    let state_c = state.clone();
    let name = room_name.to_owned();
    let built = tokio::task::spawn_blocking(move || build_room_doc(&state_c, &name))
        .await
        .map_err(|e| anyhow::anyhow!("room doc rebuild: {e}"))??;
    let vv = built.doc.oplog_vv();
    *room.doc_cache.lock().unwrap() = Some(built);
    Ok(vv)
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
    let tail = |doc: &arbos_core::doc::ChatDoc| {
        let list = doc.messages_list();
        let len = list.len();
        (len.saturating_sub(n)..len)
            .filter_map(|i| doc.projected_row(i))
            .collect::<Vec<_>>()
    };
    {
        let mut guard = room.doc_cache.lock().unwrap();
        if let Some(cached) = guard.as_mut() {
            advance_room_doc(state, room_name, cached)?;
            return Ok(tail(&cached.doc));
        }
    }
    let state_c = state.clone();
    let name = room_name.to_owned();
    let built = tokio::task::spawn_blocking(move || build_room_doc(&state_c, &name))
        .await
        .map_err(|e| anyhow::anyhow!("room doc rebuild: {e}"))??;
    let rows = tail(&built.doc);
    *room.doc_cache.lock().unwrap() = Some(built);
    Ok(rows)
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

async fn flush_held(
    d: &mut Delivery,
    sink: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), axum::Error> {
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
    ws.max_message_size(wire::ROW_MAX + 8192)
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

/// People-push refusal: always carries `batchId`; `rate_limited` also carries `retryS`;
/// `not_available` carries `reason` (`blocked_by` / `blocks_member` / `silenced` /
/// `chat_unavailable`) and, on a block, `byHandle` (+ `byName`). The code set never grows:
/// fielded clients only keep a room Live for these two codes.
fn push_refused_frame(refused: &people::PushRefused, batch_id: &str) -> Message {
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
    let room = state.room(&room_name).await;
    // Before subscribe: Drop runs after `rx` is gone, so receiver_count excludes this socket.
    let _clear_doc = LastSocketDocCacheClear(room.clone());
    let mut rx = room.tx.subscribe();
    let (mut sink, mut stream) = socket.split();
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
    let mut revocations = state.revocations.subscribe();
    revocations.mark_unchanged();
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
    sink: &mut futures::stream::SplitSink<WebSocket, Message>,
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
                sink.send(error_frame("too_large", "row exceeds 1 MiB"))
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
            // A batch id the room already stored is a dup before the people gates — no rate
            // event, no store, no broadcast.
            if let Some(seq) = state.db.row_seq_for_batch(room_name, &p.batch_id)? {
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
            if !p.heal {
                if let Ok(vv) = room_oplog_vv(state, room, room_name).await {
                    if arbos_core::doc::update_covered_by(&f.payload, &vv) {
                        let seq = state.db.room_state(room_name).map(|s| s.head).unwrap_or(0);
                        sink.send(frame(
                            FrameType::Ack,
                            json!({ "batchId": p.batch_id, "seq": seq, "dup": true }),
                            Bytes::new(),
                        ))
                        .await?;
                        return Ok(());
                    }
                }
            }
            let mut wake_owners = Vec::new();
            // A heal carries other members' history, not a message of this device: the
            // message gates (rate limit, blocks, member pushes) do not apply — the socket's ACL
            // already does. 20:42 Oct 23: the first production heal (60 KB, Arbonites) was
            // dropped because a member had blocked the healing user.
            if p.heal {
                // nothing to gate or wake
            } else if let Some(chat_id) =
                room_name.strip_prefix(&format!("{}/", people::PEOPLE_PROJECT))
            {
                match people::on_room_push(state, principal, chat_id) {
                    Ok(owners) => wake_owners = owners,
                    Err(r) => {
                        sink.send(push_refused_frame(&r, &p.batch_id)).await?;
                        return Ok(());
                    }
                }
            }
            let (seq, dup) = state
                .db
                .append_row(room_name, device, &p.batch_id, &f.payload)?;
            sink.send(frame(
                FrameType::Ack,
                json!({ "batchId": p.batch_id, "seq": seq, "dup": dup }),
                Bytes::new(),
            ))
            .await?;
            if !dup {
                let row = Frame::new(
                    FrameType::Row,
                    json!({ "seq": seq, "device": device, "batchId": p.batch_id }),
                    f.payload,
                )
                .encode();
                let _ = room.tx.send(RoomBroadcast {
                    from_conn: conn_id,
                    bytes: row,
                    seq,
                });
                if !p.heal {
                    // A link in a finished message gets its preview card (#2064), off the
                    // socket task.
                    if let Some((_, chat_id)) = room_name.rsplit_once('/') {
                        crate::unfurl::on_row(state, project_id, chat_id, is_box);
                    }
                }
                if p.heal {
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
                // Presence counts as "seen" (last_seen_at / online), at most one write a minute.
                let now = arbos_core::ids::now_ms();
                let mut seen = state.presence_seen.lock().await;
                if seen.get(id).is_none_or(|t| now - t >= 60_000) {
                    seen.insert(id.clone(), now);
                    let _ = state.db.touch_device(id);
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
    sink: &mut futures::stream::SplitSink<WebSocket, Message>,
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

/// `POST /room/{project}/{chat}/checkpoint?seqCovered=` — box only; body = snapshot; header `x-arbos-frontier` = base64 VV.
pub async fn post_checkpoint(
    State(state): State<AppState>,
    principal: Principal,
    Path((project_id, chat_id)): Path<(String, String)>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
    headers: axum::http::HeaderMap,
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
    let frontier = headers
        .get("x-arbos-frontier")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| hex::decode(s).ok())
        .unwrap_or_default();
    let room_name = wire::room_name(&project_id, &chat_id);
    let device = principal.device_name();
    match checkpoint_covers_store(&state.db, &room_name, seq_covered, &body) {
        Ok(CheckpointVerdict::Covers) => {}
        Ok(CheckpointVerdict::Incomplete(why)) => {
            tracing::warn!(
                %room_name, %device, seq_covered, %why,
                "checkpoint refused: the snapshot lacks ops the relay holds"
            );
            return (
                StatusCode::CONFLICT,
                format!("checkpoint_incomplete: {why}"),
            )
                .into_response();
        }
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }
    match state
        .db
        .put_checkpoint(&room_name, seq_covered, &frontier, &body)
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::CONFLICT, "seqCovered below floor").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
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

/// Grace-period sweeper: a box socket that stays gone marks the project asleep (crdt-sync §8.5).
pub async fn spawn_sweeper(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            boxes::sweep_box_liveness(&state).await;
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
            let writer = tokio::spawn(async move {
                let mut pushed = 0i64;
                let mut reader_go = Some(reader_go_tx);
                fn push_frame(n: i64) -> TMsg {
                    let payload = format!("row-{n}").into_bytes();
                    TMsg::Binary(
                        Frame::new(
                            FrameType::Push,
                            json!({ "batchId": format!("b{n}") }),
                            bytes::Bytes::from(payload),
                        )
                        .encode(),
                    )
                }
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
            while seqs.last().copied() != Some(ROWS) {
                let m = tokio::time::timeout_at(deadline, rstream.next()).await;
                let Ok(Some(Ok(m))) = m else {
                    panic!("iteration {it}: reader timed out with {seqs:?}");
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
                    json!({ "batchId": batch_id }),
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
}
