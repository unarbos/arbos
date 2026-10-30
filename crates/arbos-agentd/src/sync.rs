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

use arbos_core::doc::{decode_vv, encode_vv, ChatDoc, CommandStatus, CommandView};
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
    /// Lowest row seq Loro parked as pending (deps missing); the persisted cursor stays below
    /// it so a restart re-requests the row (parked ops live only in memory).
    pub first_pending_seq: Option<i64>,
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
    /// `agent:<project_id>` — this agent as a people-chat member, for the catch-up ordering.
    me: String,
    /// People chats: when this agent joined (the relay's membership row). Rows issued before
    /// that are history, never work — also for a workspace that boots after the join
    /// (TestAgent2, 2026-10-24 00:43: created, added, and its first act was Jacob's 00:42
    /// "File feedback" from before it existed). `0` = unknown / not a people chat.
    pub joined_at_ms: i64,
    /// Room link state for debugging readers (`/box/state`, `inspect`): is the relay session
    /// up, since when, when it last fell, how many sessions so far.
    pub link: LinkState,
}

/// What a room's relay session is doing right now (2026-10-23 #1575: two rooms came back
/// minutes after a sub-second relay restart and nothing said so).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct LinkState {
    pub connected: bool,
    pub connected_at: Option<i64>,
    pub disconnected_at: Option<i64>,
    pub sessions: u32,
    pub last_error: Option<String>,
}

/// Room redials after a relay restart go through this gate: at most this many handshakes in
/// flight per box, each after a little jitter, so 200+ rooms do not storm one relay the second
/// it is back (and time out together, doubling their backoff together).
const REDIAL_CONCURRENCY: usize = 16;
/// A push the relay has not acked within this is a dead link, not a slow relay (the relay acks
/// from its frame handler right after `append_row`).
const PUSH_ACK_TIMEOUT: Duration = Duration::from_secs(10);
/// Liveness probe cadence; two silent periods end the session (was one probe per 15 min).
const PROBE_EVERY: Duration = Duration::from_secs(60);
/// Backoff ceiling for a room whose session failed for any reason but a refusal.
const ROOM_BACKOFF_MAX: Duration = Duration::from_secs(4);

fn redial_gate() -> &'static tokio::sync::Semaphore {
    static GATE: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    GATE.get_or_init(|| tokio::sync::Semaphore::new(REDIAL_CONCURRENCY))
}

/// A pending people-chat `send` older than this at catch-up is history, not work: nobody asked
/// this agent by name and the conversation has moved on, so it is not even gated (each gate of
/// an unaddressed group message costs a decision-model call; 2026-10-23 a box restart replayed
/// 530 of them in a row and a fresh @mention waited eight minutes behind them).
pub const BACKLOG_MAX_AGE_MS: i64 = 10 * 60 * 1000;

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

    /// The cursor to persist and resume from: held below the first parked row.
    pub fn resume_cursor(&self) -> i64 {
        match self.first_pending_seq {
            Some(p) => self.cursor.min(p - 1),
            None => self.cursor,
        }
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
    ///
    /// People chats, catch-up order: sends that name this agent (`agent:<me>` mention, a reply to
    /// it, or any `@` in the text — the gate sorts those out without a model call) go first,
    /// newest first; unaddressed sends older than [`BACKLOG_MAX_AGE_MS`] are skipped outright;
    /// the rest follow in doc order.
    fn new_pending(&mut self) -> Vec<CommandView> {
        // A late-opened host (re-join) has nothing of its own to recover.
        let recovering = !self.recovered && self.ignore_before_ms == 0;
        self.recovered = true;
        let now = now_ms();
        // How long a `taken` people `send` stays claimable by other agents (shared address).
        const SHARED_SEND_OPEN_MS: i64 = 15 * 60 * 1000;
        let people = self.room.starts_with("people/");
        let mut addressed: Vec<(CommandView, Option<String>)> = Vec::new();
        let mut rest = Vec::new();
        let mut skipped = 0usize;
        for c in self.doc.commands() {
            if c.issued_at < self.ignore_before_ms || c.issued_at < self.joined_at_ms {
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
                // People chats: `taken` is not "mine, interrupted" — any agent of the chat
                // marks a send taken, and at boot every one of the day's commands reads as
                // recovery (2026-10-24 02:23: 189 of them re-gated, oldest 12 h). Same rule for
                // pending and taken: named → first; stale and unaddressed → not even gated.
                if people && c.kind == "send" {
                    if let Some(sender) = self.names_me(&c) {
                        if recovering && c.status == "taken" {
                            self.mark_interrupted(&c);
                        }
                        addressed.push((c, sender));
                        continue;
                    }
                    if now.saturating_sub(c.issued_at) > BACKLOG_MAX_AGE_MS {
                        skipped += 1;
                        continue;
                    }
                }
                if recovering && c.status == "taken" {
                    self.mark_interrupted(&c);
                }
                rest.push(c);
            }
        }
        if skipped > 0 {
            tracing::info!(room = %self.room, skipped, addressed = addressed.len(), live = rest.len(), "catch-up: stale unaddressed people sends skipped");
        }
        addressed.sort_by_key(|(c, _)| std::cmp::Reverse(c.issued_at));
        // A backlog of pings from one person collapses to their newest: "@testagent2 hey" /
        // "you there?" / "you there?" got three replies with the same content (2026-10-24
        // 00:45). The older ones are done without a reply; the newest answers for all.
        let mut seen_sender: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut collapsed = 0usize;
        let mut out = Vec::new();
        for (c, sender) in addressed {
            let duplicate = sender.is_some_and(|who| !seen_sender.insert(who));
            if duplicate {
                self.doc
                    .set_command_status(&c.id, CommandStatus::Done, None);
                collapsed += 1;
            } else {
                out.push(c);
            }
        }
        if collapsed > 0 {
            tracing::info!(room = %self.room, collapsed, "catch-up: older pings from the same person collapsed into their newest");
        }
        out.extend(rest);
        out
    }

    /// Does this `send` name this agent — a resolved `agent:<me>` mention, a reply to one of its
    /// rows, or any `@` in the prose (the gate decides cheaply whether that `@` is for it)?
    /// `Some(sender)` when it does (the sender, for the one-reply-per-person collapse).
    fn names_me(&self, c: &CommandView) -> Option<Option<String>> {
        let id = c
            .payload
            .get("messageId")
            .and_then(serde_json::Value::as_str)?;
        let m = self.doc.message(id)?;
        let sender = m
            .rest
            .get("sender")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let resolved = m
            .rest
            .get("mentions")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(self.me.as_str())));
        if resolved
            || m.rest
                .get("replyTo")
                .and_then(serde_json::Value::as_str)
                .is_some()
            || m.text().contains('@')
        {
            Some(sender)
        } else {
            None
        }
    }

    /// A reply that was streaming when the box died: close it as cancelled so the UI settles.
    fn mark_interrupted(&mut self, cmd: &CommandView) {
        let user_msg = cmd
            .payload
            .get("messageId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        // Only this agent's own streaming rows: in a people chat another agent may be mid-reply
        // to the same message on its own box, and that reply is not ours to cancel.
        let me = self.me.as_str();
        let stale: Vec<String> = self
            .doc
            .messages()
            .into_iter()
            .filter(|m| {
                m.role == "assistant"
                    && m.status == "streaming"
                    && m.rest.get("inReplyTo").and_then(|v| v.as_str()) == Some(user_msg.as_str())
                    && m.rest
                        .get("sender")
                        .and_then(|v| v.as_str())
                        .is_none_or(|s| s == me)
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
            first_pending_seq: None,
            acked_vv,
            inflight: None,
            caught_up: false,
            seen_commands: Default::default(),
            recovered: false,
            ever_caught_up: false,
            closed: false,
            ignore_before_ms: 0,
            me: format!("agent:{}", self.cfg.project_id),
            joined_at_ms: 0,
            link: LinkState::default(),
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

    /// The relay says this agent joined the people chat at `joined_at_ms`: commands issued
    /// before that are never run here, whether the room opened at boot or on the add.
    pub async fn mark_joined_at(&self, chat_id: &str, joined_at_ms: i64) {
        if joined_at_ms <= 0 {
            return;
        }
        if let Some(h) = self.hosts.lock().await.get(chat_id) {
            let mut h = h.lock().await;
            h.joined_at_ms = h.joined_at_ms.max(joined_at_ms);
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
        self.store
            .save_room_doc(&h.room, h.resume_cursor(), &snap, &acked)
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
            let mut handshook = false;
            let result = {
                // Jitter before the dial; the box-wide gate (`REDIAL_CONCURRENCY`) is taken
                // inside `room_session` around the handshake only — holding it for the whole
                // session capped a box at 16 live rooms (images 0.1.101–0.1.103: the canary's
                // chat never connected, its turn never started).
                let jitter = Duration::from_millis(rand::random::<u64>() % 750);
                tokio::time::sleep(jitter).await;
                self.room_session(&host, &mut handshook).await
            };
            match result {
                Ok(()) => tracing::info!(room = %room_name, "room session closed"),
                Err(e) if handshake_refused(&e) => {
                    refused = true;
                    if backoff < REFUSED_ROOM_BACKOFF {
                        tracing::warn!(room = %room_name, error = %e, "room refused by the relay; retrying every {REFUSED_ROOM_BACKOFF:?}");
                    } else {
                        tracing::debug!(room = %room_name, error = %e, "room still refused");
                    }
                    host.lock().await.link.last_error = Some(format!("refused: {e}"));
                }
                Err(e) => {
                    tracing::warn!(room = %room_name, error = %e, handshook, "room session error");
                    host.lock().await.link.last_error = Some(e.to_string());
                }
            }
            {
                let mut h = host.lock().await;
                h.clear_inflight();
                h.caught_up = false;
                h.link.connected = false;
                h.link.disconnected_at = Some(now_ms());
            }
            if refused {
                backoff = REFUSED_ROOM_BACKOFF;
            } else if handshook || started.elapsed() > Duration::from_secs(30) {
                // The relay answered: this was a restart or a dropped link, not a dead relay.
                backoff = Duration::from_millis(250);
            }
            tokio::time::sleep(backoff).await;
            if !refused {
                backoff = (backoff * 2).min(ROOM_BACKOFF_MAX);
            }
        }
    }

    async fn room_session(&self, host: &SharedHost, handshook: &mut bool) -> Result<()> {
        let (room, cursor) = {
            let h = host.lock().await;
            (h.room.clone(), h.resume_cursor())
        };
        let ws_url = format!(
            "{}/room/{}?token={}",
            self.cfg.relay_url.replacen("http", "ws", 1),
            room,
            self.cfg.box_token
        );
        // The gate covers the dial + Hello only, then the permit goes back for the next room.
        let permit = redial_gate().acquire().await;
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
        drop(permit);
        *handshook = true;
        let chat_id = {
            let mut h = host.lock().await;
            h.link.connected = true;
            h.link.connected_at = Some(now_ms());
            h.link.sessions = h.link.sessions.saturating_add(1);
            h.link.last_error = None;
            if h.link.sessions > 1 {
                tracing::info!(room = %room, sessions = h.link.sessions, cursor, unacked = h.doc.oplog_vv() != h.acked_vv, "room session reconnected");
            }
            h.chat_id.clone()
        };

        let mut tick = tokio::time::interval(Duration::from_millis(STREAM_COMMIT_MS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut persist_tick = tokio::time::interval(Duration::from_secs(2));
        let mut probe_tick = tokio::time::interval(PROBE_EVERY);
        probe_tick.tick().await;
        // Dead-link detection (#1575): a socket the relay side has dropped can look open from
        // here (the tunnel keeps our half up); pushes then sit in flight with no ack and no
        // frame ever arrives, and the room is silent until the 15-minute probe. Now a push not
        // acked within `PUSH_ACK_TIMEOUT`, or a probe unanswered, ends the session so it redials.
        let mut inflight_since: Option<std::time::Instant> = None;
        let mut last_rx = std::time::Instant::now();
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
                        Message::Ping(p) => { last_rx = std::time::Instant::now(); sink.send(Message::Pong(p)).await?; continue; }
                        Message::Pong(_) => { last_rx = std::time::Instant::now(); continue; }
                        _ => continue,
                    };
                    let Some(f) = Frame::decode(data)? else { continue };
                    last_rx = std::time::Instant::now();
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
                                    if h.first_pending_seq.is_some_and(|p| p <= st.checkpoint_seq) {
                                        h.first_pending_seq = None;
                                    }
                                    after = h.resume_cursor();
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
                                    Ok(_) => {
                                        imports_clean = false;
                                        h.first_pending_seq =
                                            Some(h.first_pending_seq.map_or(rh.seq, |p| p.min(rh.seq)));
                                    }
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
                                if imports_clean {
                                    // The walk re-covered any parked range and parked nothing.
                                    h.first_pending_seq = None;
                                }
                                (h.resume_cursor(), h.new_pending())
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
                            if h.inflight.is_none() {
                                inflight_since = None;
                            }
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
                    if let Some(since) = inflight_since {
                        if since.elapsed() > PUSH_ACK_TIMEOUT {
                            return Err(anyhow!("push not acked in {PUSH_ACK_TIMEOUT:?}; link presumed dead"));
                        }
                    }
                    if last_rx.elapsed() > PROBE_EVERY * 2 {
                        return Err(anyhow!("no frame from the relay in {:?}; link presumed dead", PROBE_EVERY * 2));
                    }
                    let push = { host.lock().await.next_push() };
                    if let Some((batch, bytes)) = push {
                        let f = Frame::new(FrameType::Push, json!({ "batchId": batch }), Bytes::from(bytes));
                        sink.send(Message::Binary(f.encode())).await?;
                        inflight_since = Some(std::time::Instant::now());
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

    /// Room links of this workspace for debugging readers: counts plus every room that is down
    /// or holds changes the relay has not acked.
    pub async fn link_summary(&self) -> serde_json::Value {
        let hosts: Vec<SharedHost> = self.hosts.lock().await.values().cloned().collect();
        let mut total = 0usize;
        let mut connected = 0usize;
        let mut trouble = Vec::new();
        for h in hosts {
            let h = h.lock().await;
            if h.closed {
                continue;
            }
            total += 1;
            let unacked = h.doc.oplog_vv() != h.acked_vv;
            if h.link.connected {
                connected += 1;
            }
            if !h.link.connected || unacked {
                trouble.push(json!({
                    "chat_id": h.chat_id, "connected": h.link.connected, "connected_at": h.link.connected_at,
                    "disconnected_at": h.link.disconnected_at, "sessions": h.link.sessions,
                    "unacked": unacked, "inflight": h.inflight.is_some(), "caught_up": h.caught_up,
                    "last_error": h.link.last_error,
                }));
            }
        }
        json!({ "rooms": total, "connected": connected, "trouble": trouble })
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

    fn host_with(doc: ChatDoc) -> DocHost {
        DocHost {
            chat_id: "c1".into(),
            room: "people/c1".into(),
            doc,
            cursor: 0,
            first_pending_seq: None,
            acked_vv: VersionVector::default(),
            inflight: None,
            caught_up: true,
            seen_commands: Default::default(),
            recovered: false,
            ever_caught_up: true,
            closed: false,
            ignore_before_ms: 0,
            me: "agent:ME".into(),
            joined_at_ms: 0,
            link: LinkState::default(),
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

    /// 2026-10-23 21:17: a box restart replayed 530 unaddressed group sends of the day through
    /// the decision model (one call each) and Jacob's fresh "@testagent …" waited eight minutes
    /// behind them. At catch-up, sends that name this agent come first (newest first), stale
    /// unaddressed ones are dropped without a gate, recent ones follow in order.
    #[test]
    fn catch_up_puts_mentions_first_and_drops_stale_backlog() {
        let doc = ChatDoc::new(Some(7), true);
        let now = now_ms();
        let hour_ago = now - 60 * 60 * 1000;
        let msg = |id: &str, text: &str, sender: &str, mentions: Vec<&str>| {
            doc.append_user_message(
                id,
                text,
                "mac",
                &arbos_core::doc::UserMessageOpts {
                    mentions: mentions.into_iter().map(str::to_owned).collect(),
                    sender: Some(sender.into()),
                    ..Default::default()
                },
            )
            .unwrap();
        };
        msg("old-chatter", "lfg", "user:sam", vec![]);
        msg(
            "old-mention",
            "@testagent try again",
            "user:sam",
            vec!["agent:ME"],
        );
        msg("old-other", "@affinity hi", "user:ken", vec!["user:aff"]);
        msg("recent-chatter", "bullish", "user:sam", vec![]);
        msg(
            "new-mention",
            "@testagent do you have credits now?",
            "user:sam",
            vec!["agent:ME"],
        );
        let stale_chatter = doc.push_command_at(
            "send",
            serde_json::json!({ "messageId": "old-chatter" }),
            "mac",
            hour_ago,
        );
        let old_mention = doc.push_command_at(
            "send",
            serde_json::json!({ "messageId": "old-mention" }),
            "mac",
            hour_ago + 1,
        );
        let old_other = doc.push_command_at(
            "send",
            serde_json::json!({ "messageId": "old-other" }),
            "mac",
            hour_ago + 2,
        );
        let recent_chatter = doc.push_command_at(
            "send",
            serde_json::json!({ "messageId": "recent-chatter" }),
            "mac",
            now - 60_000,
        );
        let new_mention = doc.push_command_at(
            "send",
            serde_json::json!({ "messageId": "new-mention" }),
            "mac",
            now - 1_000,
        );
        let stale_stop = doc.push_command_at("stop", serde_json::json!({}), "mac", hour_ago);

        let mut host = host_with(doc);
        let ids: Vec<String> = host.new_pending().into_iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            vec![new_mention, old_other, recent_chatter, stale_stop],
            "mentions newest-first (an @person line still reaches the cheap gate), sam's older ping collapsed into the newest, then the rest in doc order"
        );
        assert!(
            !ids.contains(&stale_chatter),
            "an hour-old unaddressed send is not gated at all"
        );
        // 2026-10-24 00:45: three replies to "@testagent2 hey" / "you there?" / "you there?".
        // The older ping from the same person is done, not answered; the newest answers for all.
        let statuses: std::collections::HashMap<String, String> = host
            .doc
            .commands()
            .into_iter()
            .map(|c| (c.id, c.status))
            .collect();
        assert_eq!(statuses[&old_mention], "done");
        assert_eq!(
            statuses[&stale_chatter], "pending",
            "stale chatter is merely not gated"
        );
        assert!(host.new_pending().is_empty(), "handed over once");

        // A project chat (not people/) keeps every pending send: the owner's backlog is work.
        let doc = ChatDoc::new(Some(7), true);
        let old = doc.push_command_at(
            "send",
            serde_json::json!({ "messageId": "x" }),
            "mac",
            hour_ago,
        );
        let mut host = host_with(doc);
        host.room = "01PROJECT/c1".into();
        assert_eq!(
            host.new_pending()
                .into_iter()
                .map(|c| c.id)
                .collect::<Vec<_>>(),
            vec![old]
        );
    }

    /// A workspace that boots after the agent was added to a people chat (TestAgent2,
    /// 2026-10-24 00:43) must not act on rows from before its join — `joined_at` from the
    /// relay's membership row, whether the room opened at boot or on the add.
    #[test]
    fn rows_before_my_join_are_never_work() {
        let doc = ChatDoc::new(Some(7), true);
        let now = now_ms();
        doc.append_user_message(
            "pre",
            "File feedback. the sub agent bubble is misaligned",
            "mac",
            &arbos_core::doc::UserMessageOpts {
                sender: Some("user:jacob".into()),
                ..Default::default()
            },
        )
        .unwrap();
        doc.append_user_message(
            "post",
            "@testagent2 hey",
            "mac",
            &arbos_core::doc::UserMessageOpts {
                sender: Some("user:jacob".into()),
                mentions: vec!["agent:ME".into()],
                ..Default::default()
            },
        )
        .unwrap();
        let pre = doc.push_command_at(
            "send",
            serde_json::json!({ "messageId": "pre" }),
            "mac",
            now - 70_000,
        );
        let post = doc.push_command_at(
            "send",
            serde_json::json!({ "messageId": "post" }),
            "mac",
            now - 5_000,
        );
        let mut host = host_with(doc);
        host.joined_at_ms = now - 10_000;
        let ids: Vec<String> = host.new_pending().into_iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![post]);
        assert!(!ids.contains(&pre));
    }

    /// #1575: a reply written while the relay was restarting must go out on the next session.
    /// The un-acked batch is dropped from `inflight` when a session ends and re-exported from
    /// `acked_vv` by the next `next_push`; an ack moves the mark, a dropped session does not.
    #[test]
    fn unacked_rows_are_replayed_after_a_dropped_session() {
        let doc = ChatDoc::new(Some(7), true);
        let mut host = host_with(doc);
        host.doc
            .append_user_message(
                "m1",
                "hi",
                "mac",
                &arbos_core::doc::UserMessageOpts::default(),
            )
            .unwrap();
        let (batch1, bytes1) = host.next_push().expect("first push");
        assert!(host.next_push().is_none(), "one batch in flight at a time");
        // The socket died (1012) before the ack: the session loop clears inflight only.
        host.clear_inflight();
        let (batch2, bytes2) = host.next_push().expect("replayed on the next session");
        assert_ne!(batch1, batch2);
        assert_eq!(
            bytes1, bytes2,
            "same ops, exported from the same acked mark"
        );
        host.on_ack(&batch2);
        assert!(host.next_push().is_none(), "acked: nothing left to push");
        // A stale ack for the dropped batch is ignored.
        host.doc
            .push_command("send", serde_json::json!({ "messageId": "m1" }), "mac");
        let (batch3, _) = host.next_push().expect("new change");
        host.on_ack(&batch1);
        assert!(host.next_push().is_none(), "still in flight");
        host.on_ack(&batch3);
        assert!(host.next_push().is_none());
    }

    /// Jacob's rule (#1573): @mentions set the ORDER, never a filter. A fresh un-mentioned send
    /// still goes through the normal respond gate after a backlog drain, and the stale rows that
    /// were not gated stay in the doc, so the reply can be in context.
    #[test]
    fn fresh_unmentioned_sends_still_reach_the_gate_with_the_backlog_in_context() {
        let doc = ChatDoc::new(Some(7), true);
        let now = now_ms();
        let hour_ago = now - 60 * 60 * 1000;
        let msg = |id: &str, text: &str, mentions: Vec<&str>| {
            doc.append_user_message(
                id,
                text,
                "mac",
                &arbos_core::doc::UserMessageOpts {
                    mentions: mentions.into_iter().map(str::to_owned).collect(),
                    sender_handle: Some("sam".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        };
        msg(
            "stale-fact",
            "fyi the launch date moved to the 12th",
            vec![],
        );
        msg("mention", "@testagent are you back?", vec!["agent:ME"]);
        msg("follow-up", "so when is the launch now?", vec![]);
        doc.push_command_at(
            "send",
            serde_json::json!({ "messageId": "stale-fact" }),
            "mac",
            hour_ago,
        );
        let mention = doc.push_command_at(
            "send",
            serde_json::json!({ "messageId": "mention" }),
            "mac",
            now - 2_000,
        );
        let follow_up = doc.push_command_at(
            "send",
            serde_json::json!({ "messageId": "follow-up" }),
            "mac",
            now - 1_000,
        );
        let mut host = host_with(doc);
        let ids: Vec<String> = host.new_pending().into_iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            vec![mention, follow_up],
            "the mention first, then the fresh un-mentioned send — it is gated normally, not dropped"
        );
        // The stale row was not gated, but it is still in the doc and in the context the gate
        // and the turn read for the follow-up.
        assert!(host.doc.message("stale-fact").is_some());
        let info = crate::people::PeopleChatInfo {
            members: vec![serde_json::json!({ "member": "user:sam", "handle": "sam" })],
            slug: "testagent".into(),
            kind: "group".into(),
            ..Default::default()
        };
        let ctx =
            crate::people::recent_people_context(&host.doc.messages(), "follow-up", 12, &info);
        assert!(
            ctx.iter()
                .any(|m| m["text"].as_str() == Some("fyi the launch date moved to the 12th")),
            "{ctx:?}"
        );
    }

    /// Images 0.1.101–0.1.103 failed the canary: the redial gate's permit was held for the
    /// whole session, so a box could keep at most `REDIAL_CONCURRENCY` rooms connected and the
    /// canary's chat never came up. Forty rooms against a fake relay must all be open at once.
    #[tokio::test]
    async fn more_rooms_than_the_redial_gate_stay_connected_at_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc as StdArc;

        let open = StdArc::new(AtomicUsize::new(0));
        let peak = StdArc::new(AtomicUsize::new(0));
        let (open2, peak2) = (open.clone(), peak.clone());
        async fn room_ws(
            ws: axum::extract::ws::WebSocketUpgrade,
            axum::extract::State((open, peak)): axum::extract::State<(
                StdArc<AtomicUsize>,
                StdArc<AtomicUsize>,
            )>,
        ) -> axum::response::Response {
            ws.on_upgrade(move |mut socket| async move {
                let n = open.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(n, Ordering::SeqCst);
                // Hold the socket open; the client sends Hello and waits for STATE.
                while let Some(Ok(_)) = futures::StreamExt::next(&mut socket).await {}
                open.fetch_sub(1, Ordering::SeqCst);
            })
        }
        let app = axum::Router::new()
            .route("/room/{project}/{chat}", axum::routing::get(room_ws))
            .with_state((open2, peak2));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let root = std::env::temp_dir().join(format!("arbos-rooms-{}", ulid()));
        std::fs::create_dir_all(&root).unwrap();
        let bx = crate::config::BoxConfig {
            user_id: Some("u-test".into()),
            relay_url: format!("http://127.0.0.1:{port}"),
            box_token: "test-token".into(),
            mcp_token: "test-mcp".into(),
            work_root: root.clone(),
            bind: "127.0.0.1".into(),
            port: 0,
            local_box: true,
            pi_bin: "/nonexistent/pi".into(),
            prime_bin: "/nonexistent/prime-agent".into(),
            claude_bin: "/nonexistent/claude".into(),
            pi_ext_override: None,
            device_id: "box:test".into(),
            default_talking_model: "mock-talking".into(),
            default_subagent_model: "mock-sub".into(),
            legacy_project_id: None,
        };
        let cfg = Arc::new(bx.workspace(
            &json!({ "id": "01TESTPROJECTROOMS0000000", "name": "Rooms" }),
            0,
        ));
        std::fs::create_dir_all(cfg.state_dir.join("agentd")).unwrap();
        let store = Store::open(&cfg.state_dir.join("agentd").join("agentd.db")).unwrap();
        let (tx, _rx) = mpsc::channel(64);
        let hub = SyncHub::new(cfg, store, tx);
        const ROOMS: usize = 40;
        for i in 0..ROOMS {
            hub.open_room_named(
                &format!("chat{i}"),
                &format!("people/chat{i}"),
                PROJECT_DOC_CHAT_ID,
            )
            .await
            .unwrap();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while peak.load(Ordering::SeqCst) < ROOMS && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            peak.load(Ordering::SeqCst),
            ROOMS,
            "every room connected at once; {} > {REDIAL_CONCURRENCY} gate permits",
            ROOMS
        );
        hub.close_all().await;
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 2026-10-24 02:23 (image 0.1.106): after the roll TestAgent re-gated 189 Arbonites
    /// sends, the oldest 12 h old — all `taken` (by whichever agent answered), which boot read
    /// as its own interrupted turns. In a people chat a stale taken send is history like a
    /// stale pending one; a stale taken send that names me still runs.
    #[test]
    fn stale_taken_people_sends_are_not_recovered() {
        let doc = ChatDoc::new(Some(7), true);
        let now = now_ms();
        let hour_ago = now - 60 * 60 * 1000;
        let msg = |id: &str, text: &str, mentions: Vec<&str>| {
            doc.append_user_message(
                id,
                text,
                "mac",
                &arbos_core::doc::UserMessageOpts {
                    mentions: mentions.into_iter().map(str::to_owned).collect(),
                    sender: Some("user:sam".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        };
        msg("old-chat", "lfg", vec![]);
        msg("old-ping", "@testagent try again", vec!["agent:ME"]);
        msg("fresh", "gm", vec![]);
        let old_chat = doc.push_command_at(
            "send",
            serde_json::json!({ "messageId": "old-chat" }),
            "mac",
            hour_ago,
        );
        let old_ping = doc.push_command_at(
            "send",
            serde_json::json!({ "messageId": "old-ping" }),
            "mac",
            hour_ago + 1,
        );
        let fresh = doc.push_command_at(
            "send",
            serde_json::json!({ "messageId": "fresh" }),
            "mac",
            now - 30_000,
        );
        for id in [&old_chat, &old_ping, &fresh] {
            doc.set_command_status(id, CommandStatus::Taken, None);
        }
        let mut host = host_with(doc);
        let ids: Vec<String> = host.new_pending().into_iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            vec![old_ping, fresh],
            "stale taken chatter is not recovered; a stale ping that names me and a fresh taken send are"
        );
        assert!(!ids.contains(&old_chat));
    }

    /// A box restart must not cancel another agent's live reply in a shared chat.
    #[test]
    fn recovery_cancels_only_my_own_streaming_rows() {
        let doc = ChatDoc::new(Some(7), true);
        doc.append_user_message(
            "q",
            "@testagent @doug hi",
            "mac",
            &arbos_core::doc::UserMessageOpts::default(),
        )
        .unwrap();
        doc.start_assistant_message("mine", "q", "box:test");
        doc.set_message_fields("mine", &[("sender", serde_json::json!("agent:ME"))]);
        doc.start_assistant_message("theirs", "q", "box:other");
        doc.set_message_fields("theirs", &[("sender", serde_json::json!("agent:DOUG"))]);
        let cmd = doc.push_command("send", serde_json::json!({ "messageId": "q" }), "mac");
        doc.set_command_status(&cmd, CommandStatus::Taken, None);
        let mut host = host_with(doc);
        let _ = host.new_pending();
        let status = |id: &str| host.doc.message(id).map(|m| m.status).unwrap_or_default();
        assert_eq!(status("mine"), "cancelled", "my interrupted row is closed");
        assert_eq!(
            status("theirs"),
            "streaming",
            "the other agent's live row is left alone"
        );
    }
}
