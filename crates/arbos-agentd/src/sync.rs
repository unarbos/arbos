//! Doc host: one Loro doc per room, synced with the relay as a peer
//! (`device = box:{sandboxId}`). Streaming commits every 120 ms, one in-flight
//! PUSH per room, resume by cursor (crdt-sync §7, §8).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use loro::VersionVector;
use serde_json::json;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

use arbos_core::doc::{decode_vv, encode_vv, ChatDoc, CommandView};
use arbos_core::ids::{now_ms, ulid};
use arbos_core::wire::{self, Frame, FrameType, PROJECT_DOC_CHAT_ID};
use arbos_core::STREAM_COMMIT_MS;

use crate::config::Config;
use crate::store::Store;

/// Something the agent loop must act on.
#[derive(Debug, Clone)]
pub enum Incoming {
    /// A pending command appeared in a chat doc (from a client row or a resumed doc).
    Command { chat_id: String, cmd: CommandView },
    /// A room finished its catch-up.
    CaughtUp { chat_id: String, head_seq: i64 },
    /// The relay reported a box status change (informational).
    Status {
        chat_id: String,
        state: wire::BoxState,
    },
    /// A client wrote into the project doc (settings).
    ProjectDocChanged,
}

pub struct DocHost {
    pub chat_id: String,
    pub room: String,
    pub doc: ChatDoc,
    pub cursor: i64,
    acked_vv: VersionVector,
    inflight: Option<(String, VersionVector)>,
    pub caught_up: bool,
    seen_commands: std::collections::HashSet<String>,
    recovered: bool,
    ever_caught_up: bool,
    /// Set by `close_room`: the session ends and is not reopened.
    pub closed: bool,
    /// Commands issued before this instant are not ours to run: a host opened after the
    /// workspace started (the agent was re-added to a people chat) must not replay the room's
    /// recent history into the agent loop (#1440: ~40 old sends re-dispatched on a re-join).
    /// `0` for hosts opened at start, which do recover their own interrupted turns.
    pub ignore_before_ms: i64,
}

impl DocHost {
    /// Export the next frame if anything changed and nothing is in flight.
    fn next_push(&mut self) -> Option<(String, Vec<u8>)> {
        if self.inflight.is_some() {
            return None;
        }
        let vv = self.doc.oplog_vv();
        if vv == self.acked_vv {
            return None;
        }
        let bytes = self.doc.export_updates_since(&self.acked_vv);
        if bytes.is_empty() {
            self.acked_vv = vv;
            return None;
        }
        let batch = ulid();
        self.inflight = Some((batch.clone(), vv));
        Some((batch, bytes))
    }

    fn on_ack(&mut self, batch_id: &str) {
        if let Some((b, vv)) = &self.inflight {
            if b == batch_id {
                self.acked_vv = vv.clone();
                self.inflight = None;
            }
        }
    }

    fn clear_inflight(&mut self) {
        self.inflight = None;
    }

    /// A command this box wrote itself (routine, channel, cross-project message) and will run
    /// directly: keep `new_pending` from handing it to the agent loop a second time.
    pub fn mark_seen(&mut self, cmd_id: &str) {
        self.seen_commands.insert(cmd_id.to_owned());
    }

    /// New pending commands not yet handed to the agent loop. On the first catch-up after a
    /// (re)start, commands this box had marked `taken` but never finished are recovered too.
    ///
    /// People chats can address several agents at once ("hey agents", multi-@, chip multi-select).
    /// The first box marks the command `taken`; others must still see it while the user message
    /// is recent, so each addressed agent can reply. `handle_command` skips agents that do not
    /// want the message or that already wrote a reply.
    fn new_pending(&mut self) -> Vec<CommandView> {
        // A late-opened host (re-join) has nothing of its own to recover.
        let recovering = !self.recovered && self.ignore_before_ms == 0;
        self.recovered = true;
        let now = now_ms();
        // How long a `taken` people `send` stays claimable by other agents (shared address).
        const SHARED_SEND_OPEN_MS: i64 = 15 * 60 * 1000;
        let mut out = Vec::new();
        for c in self.doc.commands() {
            if c.issued_at < self.ignore_before_ms {
                self.seen_commands.insert(c.id.clone());
                continue;
            }
            let mut eligible = c.status == "pending" || (recovering && c.status == "taken");
            if !eligible
                && c.kind == "send"
                && c.status == "taken"
                && now.saturating_sub(c.issued_at) < SHARED_SEND_OPEN_MS
            {
                eligible = true;
            }
            if eligible && self.seen_commands.insert(c.id.clone()) {
                if recovering && c.status == "taken" {
                    self.mark_interrupted(&c);
                }
                out.push(c);
            }
        }
        out
    }

    /// A reply that was streaming when the box died: close it as cancelled so the UI settles.
    fn mark_interrupted(&mut self, cmd: &CommandView) {
        let user_msg = cmd
            .payload
            .get("messageId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let stale: Vec<String> = self
            .doc
            .messages()
            .into_iter()
            .filter(|m| {
                m.role == "assistant"
                    && m.status == "streaming"
                    && m.rest.get("inReplyTo").and_then(|v| v.as_str()) == Some(user_msg.as_str())
            })
            .map(|m| m.id)
            .collect();
        for id in stale {
            self.doc.add_error_part(
                &id,
                "interrupted",
                "The box restarted while this reply was streaming; retrying.",
            );
            self.doc
                .finish_message(&id, arbos_core::doc::MessageStatus::Cancelled);
        }
    }
}

pub type SharedHost = Arc<Mutex<DocHost>>;

#[derive(Clone)]
pub struct SyncHub {
    pub cfg: Arc<Config>,
    pub store: Store,
    pub hosts: Arc<Mutex<HashMap<String, SharedHost>>>,
    pub incoming: mpsc::Sender<Incoming>,
    pub peer_id: u64,
    /// The room client tasks, so a stopped workspace takes its sockets down with it.
    room_tasks: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
}

/// Backoff while the relay refuses a room outright (HTTP 403 / 404: the project was deleted,
/// the chat is gone, the box lost access). Redialing every 16 s per room turned a deleted
/// project with 19 chats into ~80 refused handshakes a minute (Jacob's box, 2026-10-22).
const REFUSED_ROOM_BACKOFF: Duration = Duration::from_secs(10 * 60);

/// A fresh Loro peer id per process boot. A box killed between a PUSH and its persist would
/// otherwise mint new ops under ids the relay already holds (same peer, same counters).
fn peer_id_for(device: &str) -> u64 {
    use sha2::Digest;
    let nonce = ulid();
    let h = sha2::Sha256::digest(format!("{device}:{nonce}").as_bytes());
    u64::from_le_bytes(h[..8].try_into().expect("8 bytes")) | 1
}

/// The relay answered the websocket upgrade with 403 or 404: no retrying fast will change that.
fn handshake_refused(e: &anyhow::Error) -> bool {
    use tokio_tungstenite::tungstenite::http::StatusCode;
    use tokio_tungstenite::tungstenite::Error as WsError;
    matches!(
        e.downcast_ref::<WsError>(),
        Some(WsError::Http(r)) if matches!(r.status(), StatusCode::FORBIDDEN | StatusCode::NOT_FOUND)
    )
}

impl SyncHub {
    pub fn new(cfg: Arc<Config>, store: Store, incoming: mpsc::Sender<Incoming>) -> Self {
        let peer_id = peer_id_for(&cfg.device_id);
        Self {
            cfg,
            store,
            hosts: Arc::new(Mutex::new(HashMap::new())),
            incoming,
            peer_id,
            room_tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Open (or re-open) a room: load the local snapshot + cursor, start the client task.
    pub async fn open_room(&self, chat_id: &str) -> Result<SharedHost> {
        let room = wire::room_name(&self.cfg.project_id, chat_id);
        self.open_room_named(chat_id, &room, &self.cfg.project_id.clone())
            .await
    }

    /// Open a room that is not under this project (a people chat: `people/<chat>`); `doc_project`
    /// is what a fresh doc's `meta.projectId` says.
    pub async fn open_room_named(
        &self,
        chat_id: &str,
        room: &str,
        doc_project: &str,
    ) -> Result<SharedHost> {
        if let Some(h) = self.hosts.lock().await.get(chat_id) {
            return Ok(h.clone());
        }
        let room = room.to_owned();
        let (cursor, snapshot, acked) = self.store.room_doc(&room)?.unwrap_or((0, None, None));
        let doc = match &snapshot {
            Some(s) if !s.is_empty() => ChatDoc::from_snapshot(Some(self.peer_id), true, s)?,
            _ => {
                let d = ChatDoc::new(Some(self.peer_id), true);
                if chat_id != PROJECT_DOC_CHAT_ID {
                    d.init_meta(chat_id, doc_project, 1);
                }
                d
            }
        };
        let acked_vv = acked.as_deref().and_then(decode_vv).unwrap_or_default();
        let host = Arc::new(Mutex::new(DocHost {
            chat_id: chat_id.to_owned(),
            room: room.clone(),
            doc,
            cursor,
            acked_vv,
            inflight: None,
            caught_up: false,
            seen_commands: Default::default(),
            recovered: false,
            ever_caught_up: false,
            closed: false,
            ignore_before_ms: 0,
        }));
        self.hosts
            .lock()
            .await
            .insert(chat_id.to_owned(), host.clone());
        let hub = self.clone();
        let h2 = host.clone();
        let task = tokio::spawn(async move { hub.run_room(h2).await });
        if let Some(old) = self
            .room_tasks
            .lock()
            .await
            .insert(chat_id.to_owned(), task)
        {
            old.abort();
        }
        Ok(host)
    }

    /// The agent was (re-)added to this chat while the workspace was running: what happened in
    /// the room before now is history, not work — nothing issued before this instant is run.
    pub async fn mark_late_join(&self, chat_id: &str) {
        if let Some(h) = self.hosts.lock().await.get(chat_id) {
            let mut h = h.lock().await;
            if h.ignore_before_ms == 0 {
                h.ignore_before_ms = now_ms();
            }
        }
    }

    /// The workspace is stopping (its project was deleted, or it is held for a migration): close
    /// every room for good and end the sessions, so nothing keeps redialing the relay. The room
    /// tasks were detached before, and a removed workspace's 20 rooms kept reconnecting with 403
    /// every 16 s each until the box restarted.
    pub async fn close_all(&self) {
        let hosts: Vec<SharedHost> = self.hosts.lock().await.drain().map(|(_, h)| h).collect();
        for h in &hosts {
            h.lock().await.closed = true;
        }
        for (_, t) in self.room_tasks.lock().await.drain() {
            t.abort();
        }
    }

    /// Leave a room for good (the agent was removed from a people chat): the session closes and
    /// is not reopened; the local snapshot stays until the next persist sweep ignores it. The
    /// socket task is aborted too — the `closed` flag alone only took effect at the next
    /// reconnect, so an old session kept importing rows and handing commands to the agent loop
    /// beside the host that replaced it (#1440).
    pub async fn close_room(&self, chat_id: &str) {
        if let Some(h) = self.hosts.lock().await.remove(chat_id) {
            h.lock().await.closed = true;
        }
        if let Some(t) = self.room_tasks.lock().await.remove(chat_id) {
            t.abort();
        }
    }

    pub async fn host(&self, chat_id: &str) -> Option<SharedHost> {
        self.hosts.lock().await.get(chat_id).cloned()
    }

    /// Persist cursor + snapshot + acked vv together.
    pub async fn persist(&self, host: &SharedHost) -> Result<()> {
        let h = host.lock().await;
        let snap = h.doc.snapshot();
        let acked = encode_vv(&h.acked_vv);
        self.store.save_room_doc(&h.room, h.cursor, &snap, &acked)
    }

    pub async fn persist_all(&self) {
        let hosts: Vec<SharedHost> = self.hosts.lock().await.values().cloned().collect();
        for h in hosts {
            let _ = self.persist(&h).await;
        }
    }

    /// Post a full checkpoint for a room (box is the only compactor, D20).
    pub async fn post_checkpoint(&self, host: &SharedHost, seq_covered: i64) -> Result<()> {
        let (room, snap, vv) = {
            let h = host.lock().await;
            (
                h.room.clone(),
                h.doc.snapshot(),
                encode_vv(&h.doc.oplog_vv()),
            )
        };
        let url = format!(
            "{}/room/{}/checkpoint?seqCovered={}",
            self.cfg.relay_url, room, seq_covered
        );
        let resp = reqwest::Client::new()
            .post(url)
            .bearer_auth(&self.cfg.box_token)
            .header("x-arbos-frontier", hex::encode(vv))
            .body(snap)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(anyhow!("checkpoint: HTTP {}", resp.status()));
        }
        Ok(())
    }

    async fn run_room(&self, host: SharedHost) {
        let mut backoff = Duration::from_millis(250);
        loop {
            if host.lock().await.closed {
                return;
            }
            let started = std::time::Instant::now();
            let room_name = host.lock().await.room.clone();
            let mut refused = false;
            match self.room_session(&host).await {
                Ok(()) => tracing::info!(room = %room_name, "room session closed"),
                Err(e) if handshake_refused(&e) => {
                    refused = true;
                    if backoff < REFUSED_ROOM_BACKOFF {
                        tracing::warn!(room = %room_name, error = %e, "room refused by the relay; retrying every {REFUSED_ROOM_BACKOFF:?}");
                    } else {
                        tracing::debug!(room = %room_name, error = %e, "room still refused");
                    }
                }
                Err(e) => tracing::warn!(room = %room_name, error = %e, "room session error"),
            }
            {
                let mut h = host.lock().await;
                h.clear_inflight();
                h.caught_up = false;
            }
            if refused {
                backoff = REFUSED_ROOM_BACKOFF;
            } else if started.elapsed() > Duration::from_secs(30) {
                backoff = Duration::from_millis(250);
            }
            tokio::time::sleep(backoff).await;
            if !refused {
                backoff = (backoff * 2).min(Duration::from_secs(16));
            }
        }
    }

    async fn room_session(&self, host: &SharedHost) -> Result<()> {
        let (room, cursor) = {
            let h = host.lock().await;
            (h.room.clone(), h.cursor)
        };
        let ws_url = format!(
            "{}/room/{}?token={}",
            self.cfg.relay_url.replacen("http", "ws", 1),
            room,
            self.cfg.box_token
        );
        let (ws, _) = tokio::time::timeout(
            Duration::from_secs(15),
            tokio_tungstenite::connect_async(&ws_url),
        )
        .await
        .map_err(|_| anyhow!("dial timeout"))??;
        let (mut sink, mut stream) = ws.split();
        let hello = Frame::new(
            FrameType::Hello,
            serde_json::to_value(wire::Hello {
                cursor,
                device: self.cfg.device_id.clone(),
                room: room.clone(),
                proto: arbos_core::PROTO_VERSION,
            })?,
            Bytes::new(),
        );
        sink.send(Message::Binary(hello.encode())).await?;
        let chat_id = host.lock().await.chat_id.clone();

        let mut tick = tokio::time::interval(Duration::from_millis(STREAM_COMMIT_MS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut persist_tick = tokio::time::interval(Duration::from_secs(2));
        let mut probe_tick = tokio::time::interval(Duration::from_secs(15 * 60));
        probe_tick.tick().await;
        let mut dirty_since_persist = false;
        let mut head_seq_for_checkpoint: Option<(i64, i64, i64)> = None;
        // False once a row of this session failed to import or was parked by Loro as pending
        // (its deps are missing here): a snapshot of such a doc must never become the relay's
        // checkpoint — the relay deletes the rows a checkpoint covers (Arbonites, 2026-10-23).
        let mut imports_clean = true;
        let mut healed = false;

        loop {
            tokio::select! {
                msg = stream.next() => {
                    let Some(msg) = msg else { return Ok(()) };
                    let data = match msg? {
                        Message::Binary(b) => b,
                        Message::Close(_) => return Ok(()),
                        Message::Ping(p) => { sink.send(Message::Pong(p)).await?; continue; }
                        _ => continue,
                    };
                    let Some(f) = Frame::decode(data)? else { continue };
                    match f.ty {
                        FrameType::State => {
                            let st: wire::State = serde_json::from_value(f.header)?;
                            head_seq_for_checkpoint = Some((st.head_seq, st.row_count, st.row_bytes));
                            let mut after = cursor;
                            let contained = match decode_vv(&f.payload) {
                                Some(vv) => host.lock().await.doc.contains_vv(&vv),
                                None => true,
                            };
                            if st.checkpoint_seq > 0 && (!contained || cursor < st.checkpoint_seq) && !f.payload.is_empty() {
                                if let Ok(bytes) = self.fetch_checkpoint(&room).await {
                                    let mut h = host.lock().await;
                                    let _ = h.doc.import(&bytes);
                                    h.cursor = h.cursor.max(st.checkpoint_seq);
                                    after = h.cursor;
                                }
                            }
                            after = after.max(st.checkpoint_seq.min(cursor));
                            // Every row is requested, own device id included (`excludeOwn:false`,
                            // the client opt-out of #1409): the device id `box:<sandbox>` is shared
                            // by every workspace of the box, so two agents of one owner in the same
                            // people chat are two replicas behind one id, and "own" rows are the
                            // other agent's. Loro drops what this replica already has.
                            imports_clean = true;
                            let req = Frame::new(FrameType::RowsReq, serde_json::to_value(wire::RowsReq { after, exclude_own: false })?, Bytes::new());
                            sink.send(Message::Binary(req.encode())).await?;
                        }
                        FrameType::Row => {
                            let rh: wire::RowHeader = serde_json::from_value(f.header)?;
                            let cmds = {
                                let mut h = host.lock().await;
                                // Imported whatever device the header names (see the ROWS_REQ note).
                                match h.doc.import(&f.payload) {
                                    Ok(status) if status.pending.is_none() => {}
                                    Ok(_) => imports_clean = false,
                                    Err(e) => {
                                        imports_clean = false;
                                        tracing::warn!(room = %room, seq = rh.seq, error = %e, "row import failed");
                                    }
                                }
                                h.cursor = h.cursor.max(rh.seq);
                                dirty_since_persist = true;
                                if h.caught_up { h.new_pending() } else { Vec::new() }
                            };
                            for cmd in cmds {
                                let _ = self.incoming.send(Incoming::Command { chat_id: chat_id.clone(), cmd }).await;
                            }
                            if chat_id == PROJECT_DOC_CHAT_ID && rh.device != self.cfg.device_id {
                                let _ = self.incoming.send(Incoming::ProjectDocChanged).await;
                            }
                        }
                        FrameType::RowsDone => {
                            let head = f.header.get("headSeq").and_then(|v| v.as_i64()).unwrap_or(0);
                            // History heal (#1518 follow-up): the payload is the relay's version
                            // vector for the room. Ops this replica holds beyond it were lost on
                            // the relay (a stale checkpoint); push them back once per session so
                            // every other replica's parked rows can apply.
                            if !healed && !f.payload.is_empty() {
                                if let Some(relay_vv) = decode_vv(&f.payload) {
                                    let missing = host.lock().await.doc.updates_beyond(&relay_vv);
                                    if !missing.is_empty() && missing.len() <= wire::ROW_MAX {
                                        healed = true;
                                        tracing::info!(room = %room, bytes = missing.len(), "history heal: pushing ops the relay lacked");
                                        let f = Frame::new(FrameType::Push, json!({ "batchId": ulid(), "heal": true }), Bytes::from(missing));
                                        sink.send(Message::Binary(f.encode())).await?;
                                    }
                                }
                            }
                            let (covered, cmds) = {
                                let mut h = host.lock().await;
                                h.caught_up = true;
                                if !h.ever_caught_up {
                                    // Everything the relay has from us is now in the oplog.
                                    h.acked_vv = h.doc.oplog_vv();
                                    h.ever_caught_up = true;
                                }
                                h.cursor = h.cursor.max(head);
                                (h.cursor, h.new_pending())
                            };
                            let _ = self.persist(host).await;
                            let _ = self.incoming.send(Incoming::CaughtUp { chat_id: chat_id.clone(), head_seq: head }).await;
                            for cmd in cmds {
                                let _ = self.incoming.send(Incoming::Command { chat_id: chat_id.clone(), cmd }).await;
                            }
                            if let Some((_, rows, bytes)) = head_seq_for_checkpoint.take() {
                                // The checkpoint covers what this replica has actually walked past
                                // (ROWS_DONE `headSeq`, the cursor), never the room head from STATE:
                                // rows appended since then arrive live and are not in the snapshot.
                                // A session with a failed or parked import posts nothing.
                                if (rows > 200 || bytes > 512 * 1024) && imports_clean {
                                    if let Err(e) = self.post_checkpoint(host, covered).await {
                                        tracing::warn!(error = %e, "checkpoint post failed");
                                    }
                                } else if !imports_clean {
                                    tracing::warn!(room = %room, "checkpoint skipped: this replica's doc has rows it could not apply");
                                }
                            }
                        }
                        FrameType::Ack => {
                            // The cursor does not move on ACK: the relay acks from the frame
                            // handler while rows from other sockets can still be queued for this
                            // one; a cursor past them would skip them on the next ROWS_REQ and the
                            // doc would freeze on their dependants (#1406, mirrored from the apps).
                            let ack: wire::Ack = serde_json::from_value(f.header)?;
                            let mut h = host.lock().await;
                            h.on_ack(&ack.batch_id);
                            dirty_since_persist = true;
                        }
                        FrameType::Status => {
                            if let Ok(s) = serde_json::from_value::<wire::StatusHeader>(f.header) {
                                let _ = self.incoming.send(Incoming::Status { chat_id: chat_id.clone(), state: s.box_state }).await;
                            }
                        }
                        FrameType::Error => {
                            let code = f.header.get("code").and_then(|v| v.as_str()).unwrap_or("");
                            if code == "lagging" {
                                let after = host.lock().await.cursor;
                                let req = Frame::new(FrameType::RowsReq, serde_json::to_value(wire::RowsReq { after, exclude_own: false })?, Bytes::new());
                                sink.send(Message::Binary(req.encode())).await?;
                            } else {
                                tracing::warn!(room = %room, header = %f.header, "relay error frame");
                            }
                        }
                        FrameType::ProbeOk | FrameType::Presence | FrameType::Hello | FrameType::RowsReq | FrameType::Push | FrameType::Probe | FrameType::Wake => {}
                    }
                }
                _ = tick.tick() => {
                    if host.lock().await.closed {
                        return Ok(());
                    }
                    let push = { host.lock().await.next_push() };
                    if let Some((batch, bytes)) = push {
                        let f = Frame::new(FrameType::Push, json!({ "batchId": batch }), Bytes::from(bytes));
                        sink.send(Message::Binary(f.encode())).await?;
                        dirty_since_persist = true;
                    }
                }
                _ = persist_tick.tick() => {
                    if dirty_since_persist {
                        dirty_since_persist = false;
                        let _ = self.persist(host).await;
                    }
                }
                _ = probe_tick.tick() => {
                    sink.send(Message::Binary(Frame::new(FrameType::Probe, json!({}), Bytes::new()).encode())).await?;
                }
            }
        }
    }

    async fn fetch_checkpoint(&self, room: &str) -> Result<Vec<u8>> {
        let url = format!("{}/room/{}/checkpoint", self.cfg.relay_url, room);
        let resp = reqwest::Client::new()
            .get(url)
            .bearer_auth(&self.cfg.box_token)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(anyhow!("checkpoint fetch: HTTP {}", resp.status()));
        }
        Ok(resp.bytes().await?.to_vec())
    }

    /// Write the box status into the project doc (`status` map).
    pub async fn write_status(&self, fields: &[(&str, serde_json::Value)]) {
        if let Some(h) = self.host(PROJECT_DOC_CHAT_ID).await {
            let h = h.lock().await;
            let status = h.doc.doc.get_map("status");
            for (k, v) in fields {
                let lv: loro::LoroValue =
                    serde_json::from_value(v.clone()).unwrap_or(loro::LoroValue::Null);
                status.insert(k, lv).ok();
            }
            status.insert("since", now_ms()).ok();
            h.doc.doc.commit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbos_core::doc::CommandStatus;

    fn host_with(doc: ChatDoc) -> DocHost {
        DocHost {
            chat_id: "c1".into(),
            room: "people/c1".into(),
            doc,
            cursor: 0,
            acked_vv: VersionVector::default(),
            inflight: None,
            caught_up: true,
            seen_commands: Default::default(),
            recovered: false,
            ever_caught_up: true,
            closed: false,
            ignore_before_ms: 0,
        }
    }

    /// #1440: GigChadAgent was removed and re-added to a group; the fresh host replayed ~40
    /// recent `taken` sends (the 15-minute shared window plus "recovery") into the agent loop.
    /// A late-joined host runs nothing issued before it joined; a host opened at start still
    /// recovers its own interrupted turns.
    #[test]
    fn late_join_runs_nothing_from_before_it_joined() {
        let doc = ChatDoc::new(Some(7), true);
        let old_pending = doc.push_command("send", serde_json::json!({ "messageId": "m1" }), "mac");
        let old_taken = doc.push_command("send", serde_json::json!({ "messageId": "m2" }), "mac");
        doc.set_command_status(&old_taken, CommandStatus::Taken, None);

        let copy =
            |d: &ChatDoc| ChatDoc::from_snapshot(Some(9), true, &d.snapshot()).expect("copy");
        let mut at_start = host_with(copy(&doc));
        let ids: Vec<String> = at_start.new_pending().into_iter().map(|c| c.id).collect();
        assert!(
            ids.contains(&old_pending),
            "a host opened at start runs pending commands"
        );
        assert!(ids.contains(&old_taken), "…and recovers taken ones");

        let mut late = host_with(copy(&doc));
        late.ignore_before_ms = now_ms() + 1;
        assert!(
            late.new_pending().is_empty(),
            "a late-joined host leaves the room's past alone"
        );
        std::thread::sleep(std::time::Duration::from_millis(3));
        let fresh = late
            .doc
            .push_command("send", serde_json::json!({ "messageId": "m3" }), "mac");
        let ids: Vec<String> = late.new_pending().into_iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            vec![fresh],
            "but it runs what arrives after the join, once"
        );
        assert!(late.new_pending().is_empty());
    }
}
