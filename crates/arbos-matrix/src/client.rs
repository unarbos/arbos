//! One `matrix-sdk` client per signed-in device, driven by sliding sync through the relay.
//!
//! The apps see three things: a [`Snapshot`] of servers and channels whenever the room list
//! changes, the timeline rows of a room they opened (and every new row after), and the result
//! of their own sends. Everything else — tokens, retries, pagination tokens, the state events
//! that say which room is a channel of which server — stays in here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use matrix_sdk::authentication::matrix::MatrixSession;
use matrix_sdk::authentication::SessionTokens;
use matrix_sdk::room::MessagesOptions;
use matrix_sdk::ruma::api::client::receipt::create_receipt::v3::ReceiptType;
use matrix_sdk::ruma::api::client::sync::sync_events::v5 as sliding;
use matrix_sdk::ruma::events::reaction::OriginalSyncReactionEvent;
use matrix_sdk::ruma::events::receipt::ReceiptThread;
use matrix_sdk::ruma::events::room::message::OriginalSyncRoomMessageEvent;
use matrix_sdk::ruma::events::room::redaction::OriginalSyncRoomRedactionEvent;
use matrix_sdk::ruma::events::{MessageLikeEventType, StateEventType};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{EventId, OwnedRoomId, OwnedUserId, RoomId, TransactionId, UInt, UserId};
use matrix_sdk::{Client, Room, SessionMeta, SlidingSyncList, SlidingSyncMode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::rows::{
    member_of_mxid, message_row, reaction_from_event, redacts_from_event, row_with_reactions,
    ChannelRow, MessageRow, SpaceRow, StoredReaction,
};
use crate::{Error, Result};

/// What `GET /me/matrix` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub homeserver: String,
    pub user_id: String,
    pub access_token: String,
    pub device_id: String,
}

/// Servers and channels this device is in, as the left list shows them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub spaces: Vec<SpaceRow>,
    pub channels: Vec<ChannelRow>,
}

/// The app side. Called from the sync task; keep the handlers quick (post to the UI thread).
pub trait Listener: Send + Sync {
    fn on_rooms(&self, snapshot: Snapshot);
    fn on_timeline(&self, room_id: String, rows: Vec<MessageRow>);
    /// `true` once a sync round trip succeeded; `false` with the error while it keeps retrying.
    fn on_connection(&self, connected: bool, error: Option<String>);
}

#[derive(Default)]
struct Timeline {
    rows: Vec<MessageRow>,
    /// Reactions keyed by the target message's Matrix event id.
    reactions: HashMap<String, Vec<StoredReaction>>,
    /// `/messages` token for the next page back; `None` before the first page or at the start.
    prev_batch: Option<String>,
    /// The first page was loaded (so the rows are complete from `prev_batch` to now).
    loaded: bool,
    at_start: bool,
}

const HISTORY_PAGE: u32 = 50;
const ROOM_RANGE_END: u32 = 1999;
const POLL_TIMEOUT: Duration = Duration::from_secs(30);

pub struct MatrixClient {
    client: Client,
    me: OwnedUserId,
    listener: Arc<dyn Listener>,
    timelines: Mutex<HashMap<OwnedRoomId, Timeline>>,
    stop: watch::Sender<bool>,
}

impl MatrixClient {
    /// Build the client, restore the device session and start syncing. Must run on a tokio
    /// runtime; returns once the client exists (the first snapshot arrives through the listener).
    pub async fn start(session: Session, listener: Arc<dyn Listener>) -> Result<Arc<Self>> {
        let user_id = UserId::parse(&session.user_id).map_err(|e| Error::Session(e.to_string()))?;
        let http = reqwest13::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent("Arbos")
            .build()
            .map_err(|e| Error::Homeserver(e.to_string()))?;
        let client = Client::builder()
            .homeserver_url(&session.homeserver)
            .http_client(http)
            .build()
            .await
            .map_err(|e| Error::Homeserver(e.to_string()))?;
        client
            .restore_session(MatrixSession {
                meta: SessionMeta {
                    user_id: user_id.clone(),
                    device_id: session.device_id.as_str().into(),
                },
                tokens: SessionTokens {
                    access_token: session.access_token.clone(),
                    refresh_token: None,
                },
            })
            .await
            .map_err(|e| Error::Session(e.to_string()))?;
        let (stop, stop_rx) = watch::channel(false);
        let this = Arc::new(Self {
            client,
            me: user_id,
            listener,
            timelines: Mutex::new(HashMap::new()),
            stop,
        });
        this.install_event_handler();
        tokio::spawn(Arc::clone(&this).sync_loop(stop_rx));
        Ok(this)
    }

    /// Stop syncing; the handle stays usable for nothing else.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }

    pub fn user_id(&self) -> String {
        self.me.to_string()
    }

    /// Own member string (`user:<id>`), for the transcript's "mine" rule.
    pub fn me_member(&self) -> String {
        member_of_mxid(self.me.as_str())
    }

    // ---- sync ---------------------------------------------------------------------------------

    fn install_event_handler(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.client
            .add_event_handler(move |ev: Raw<OriginalSyncRoomMessageEvent>, room: Room| {
                let weak = weak.clone();
                async move {
                    let Some(this) = weak.upgrade() else { return };
                    let Ok(mut v) = serde_json::from_str::<Value>(ev.json().get()) else {
                        return;
                    };
                    if v["event_id"].is_null() {
                        return;
                    }
                    v["room_id"] = json!(room.room_id().as_str());
                    let names = this.names_for(&room, std::slice::from_ref(&v)).await;
                    if let Some(row) = message_row(&v, &|m| names.get(m).cloned()) {
                        this.push_rows(room.room_id(), vec![row], true);
                    }
                }
            });
        let weak = Arc::downgrade(self);
        self.client
            .add_event_handler(move |ev: Raw<OriginalSyncReactionEvent>, room: Room| {
                let weak = weak.clone();
                async move {
                    let Some(this) = weak.upgrade() else { return };
                    let Ok(v) = serde_json::from_str::<Value>(ev.json().get()) else {
                        return;
                    };
                    if let Some((target, reaction)) = reaction_from_event(&v) {
                        this.apply_reaction(room.room_id(), target, reaction, true);
                    }
                }
            });
        let weak = Arc::downgrade(self);
        self.client.add_event_handler(
            move |ev: Raw<OriginalSyncRoomRedactionEvent>, room: Room| {
                let weak = weak.clone();
                async move {
                    let Some(this) = weak.upgrade() else { return };
                    let Ok(v) = serde_json::from_str::<Value>(ev.json().get()) else {
                        return;
                    };
                    if let Some(redacted) = redacts_from_event(&v) {
                        this.apply_redaction(room.room_id(), &redacted, true);
                    }
                }
            },
        );
    }

    async fn sync_loop(self: Arc<Self>, mut stop: watch::Receiver<bool>) {
        let me_key = self.me.to_string();
        loop {
            if *stop.borrow() {
                return;
            }
            let built = self.client.sliding_sync("arbos").map(|b| {
                b.poll_timeout(POLL_TIMEOUT)
                    .network_timeout(POLL_TIMEOUT + Duration::from_secs(10))
                    .with_receipt_extension({
                        let mut r = sliding::request::Receipts::default();
                        r.enabled = Some(true);
                        r
                    })
                    .add_list(
                        SlidingSyncList::builder("all")
                            .sync_mode(
                                SlidingSyncMode::new_selective().add_range(0..=ROOM_RANGE_END),
                            )
                            .timeline_limit(1)
                            .required_state(vec![
                                (StateEventType::RoomName, String::new()),
                                (StateEventType::RoomCreate, String::new()),
                                (StateEventType::RoomPowerLevels, String::new()),
                                (StateEventType::RoomJoinRules, String::new()),
                                (StateEventType::from("life.arbos.chat"), String::new()),
                                (StateEventType::SpaceParent, "*".to_owned()),
                                (StateEventType::SpaceChild, "*".to_owned()),
                                (StateEventType::RoomMember, me_key.clone()),
                            ]),
                    )
            });
            let ss = match built {
                Ok(b) => match b.build().await {
                    Ok(ss) => ss,
                    Err(e) => {
                        self.listener.on_connection(false, Some(e.to_string()));
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                },
                Err(e) => {
                    self.listener.on_connection(false, Some(e.to_string()));
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            let stream = ss.sync();
            futures::pin_mut!(stream);
            let mut backoff = Duration::from_secs(1);
            loop {
                tokio::select! {
                    _ = stop.changed() => return,
                    next = stream.next() => match next {
                        Some(Ok(summary)) => {
                            backoff = Duration::from_secs(1);
                            self.listener.on_connection(true, None);
                            if !summary.rooms.is_empty() || !summary.lists.is_empty() {
                                let snap = self.snapshot().await;
                                self.listener.on_rooms(snap);
                            }
                        }
                        Some(Err(e)) => {
                            self.listener.on_connection(false, Some(e.to_string()));
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(Duration::from_secs(30));
                            if e.client_api_error_kind().is_some() {
                                // A rejected position: rebuild the session from scratch.
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    }

    // ---- rooms --------------------------------------------------------------------------------

    async fn arbos_state(room: &Room) -> Option<Value> {
        let raw = room
            .get_state_event(StateEventType::from("life.arbos.chat"), "")
            .await
            .ok()
            .flatten()?;
        let v: Value = match raw {
            matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState::Sync(r) => {
                serde_json::from_str(r.json().get()).ok()?
            }
            matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState::Stripped(r) => {
                serde_json::from_str(r.json().get()).ok()?
            }
        };
        Some(v["content"].clone())
    }

    async fn child_order(room: &Room) -> HashMap<String, String> {
        let mut out = HashMap::new();
        if let Ok(children) = room.get_state_events(StateEventType::SpaceChild).await {
            for c in children {
                let json = match c {
                    matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState::Sync(r) => {
                        r.json().get().to_owned()
                    }
                    matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState::Stripped(r) => {
                        r.json().get().to_owned()
                    }
                };
                if let Ok(v) = serde_json::from_str::<Value>(&json) {
                    if let Some(k) = v["state_key"].as_str() {
                        if v["content"]["via"].is_array() {
                            out.insert(
                                k.to_owned(),
                                v["content"]["order"].as_str().unwrap_or("~").to_owned(),
                            );
                        }
                    }
                }
            }
        }
        out
    }

    fn last_of(&self, room_id: &RoomId) -> (String, String, i64) {
        let tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
        tl.get(room_id)
            .and_then(|t| t.rows.last())
            .map(|r| (r.text.clone(), r.sender_name.clone(), r.created_at))
            .unwrap_or_default()
    }

    /// Rebuild the left-list snapshot from the rooms the client knows.
    pub async fn snapshot(&self) -> Snapshot {
        let mut spaces: Vec<(SpaceRow, HashMap<String, String>)> = Vec::new();
        let mut channels: Vec<ChannelRow> = Vec::new();
        for room in self.client.joined_rooms() {
            let Some(state) = Self::arbos_state(&room).await else {
                continue;
            };
            let (last_text, last_sender_name, last_ts) = self.last_of(room.room_id());
            match state["kind"].as_str() {
                Some("space") => {
                    let order = Self::child_order(&room).await;
                    spaces.push((
                        SpaceRow {
                            room_id: room.room_id().to_string(),
                            server_id: state["chat_id"].as_str().unwrap_or_default().to_owned(),
                            name: room.name().unwrap_or_default(),
                            member_count: room.joined_members_count(),
                            unread: 0,
                            last_text: String::new(),
                            last_sender_name: String::new(),
                            last_ts: 0,
                            channel_ids: Vec::new(),
                        },
                        order,
                    ));
                }
                Some("channel") => {
                    let pl = room.power_levels_or_default().await;
                    let restricted = i64::from(pl.events_default) > 0;
                    let can_post =
                        pl.user_can_send_message(&self.me, MessageLikeEventType::RoomMessage);
                    channels.push(ChannelRow {
                        room_id: room.room_id().to_string(),
                        channel_id: state["chat_id"].as_str().unwrap_or_default().to_owned(),
                        server_id: state["server_id"].as_str().unwrap_or_default().to_owned(),
                        name: room.name().unwrap_or_default(),
                        restricted,
                        can_post,
                        unread: room.unread_notification_counts().notification_count,
                        last_text,
                        last_sender_name,
                        last_ts,
                        order: String::new(),
                    });
                }
                _ => {}
            }
        }
        // Channels take their order from the Space's children; a Space's activity is its
        // newest channel.
        for (space, order) in &mut spaces {
            for ch in channels
                .iter_mut()
                .filter(|c| c.server_id == space.server_id)
            {
                ch.order = order
                    .get(&ch.room_id)
                    .cloned()
                    .unwrap_or_else(|| "~".to_owned());
                space.channel_ids.push(ch.room_id.clone());
                space.unread += ch.unread;
                if ch.last_ts > space.last_ts {
                    space.last_ts = ch.last_ts;
                    space.last_text = format!("{} · {}", ch.name, ch.last_text);
                    space.last_sender_name = ch.last_sender_name.clone();
                }
            }
        }
        channels.sort_by(|a, b| a.order.cmp(&b.order).then(a.name.cmp(&b.name)));
        let mut spaces: Vec<SpaceRow> = spaces.into_iter().map(|(s, _)| s).collect();
        spaces.sort_by(|a, b| b.last_ts.cmp(&a.last_ts).then(a.name.cmp(&b.name)));
        Snapshot { spaces, channels }
    }

    // ---- timelines ----------------------------------------------------------------------------

    fn room(&self, room_id: &str) -> Result<Room> {
        let id = RoomId::parse(room_id).map_err(|e| Error::NoRoom(e.to_string()))?;
        self.client
            .get_room(&id)
            .ok_or_else(|| Error::NoRoom(room_id.to_owned()))
    }

    async fn names_for(&self, room: &Room, events: &[Value]) -> HashMap<String, String> {
        let mut names = HashMap::new();
        for ev in events {
            let Some(mxid) = ev["sender"].as_str() else {
                continue;
            };
            if names.contains_key(mxid) {
                continue;
            }
            let Ok(uid) = UserId::parse(mxid) else {
                continue;
            };
            // Sliding sync only carries our own membership; the first miss in a room fetches
            // its member list once (`/members`), after that the store answers.
            let member = match room.get_member_no_sync(&uid).await {
                Ok(Some(m)) => Some(m),
                _ => room.get_member(&uid).await.ok().flatten(),
            };
            if let Some(m) = member {
                names.insert(mxid.to_owned(), m.name().to_owned());
            }
        }
        names
    }

    /// Stamp stored reactions onto every message row for the apps.
    fn projected(t: &Timeline) -> Vec<MessageRow> {
        t.rows
            .iter()
            .map(|r| {
                let empty: &[StoredReaction] = &[];
                let list = t
                    .reactions
                    .get(&r.event_id)
                    .map(|v| v.as_slice())
                    .unwrap_or(empty);
                row_with_reactions(r, list)
            })
            .collect()
    }

    /// Merge rows into a room's timeline (deduplicated by event id, kept in time order) and
    /// tell the listener when something changed.
    fn push_rows(&self, room_id: &RoomId, rows: Vec<MessageRow>, notify: bool) -> Vec<MessageRow> {
        let mut tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
        let t = tl.entry(room_id.to_owned()).or_default();
        let mut changed = false;
        for r in rows {
            if t.rows.iter().any(|x| x.event_id == r.event_id) {
                continue;
            }
            t.rows.push(r);
            changed = true;
        }
        if changed {
            t.rows.sort_by(|a, b| {
                a.created_at
                    .cmp(&b.created_at)
                    .then(a.event_id.cmp(&b.event_id))
            });
        }
        let out = Self::projected(t);
        let loaded = t.loaded;
        drop(tl);
        if changed && notify && loaded {
            self.listener.on_timeline(room_id.to_string(), out.clone());
        }
        out
    }

    fn apply_reaction(
        &self,
        room_id: &RoomId,
        target: String,
        reaction: StoredReaction,
        notify: bool,
    ) {
        let mut tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
        let t = tl.entry(room_id.to_owned()).or_default();
        let list = t.reactions.entry(target).or_default();
        if list.iter().any(|r| r.event_id == reaction.event_id) {
            return;
        }
        // One emoji per sender on a message: replace an earlier pick of the same key.
        list.retain(|r| !(r.sender == reaction.sender && r.emoji == reaction.emoji));
        list.push(reaction);
        let out = Self::projected(t);
        let loaded = t.loaded;
        drop(tl);
        if notify && loaded {
            self.listener.on_timeline(room_id.to_string(), out);
        }
    }

    fn apply_redaction(&self, room_id: &RoomId, redacted_event_id: &str, notify: bool) {
        let mut tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
        let Some(t) = tl.get_mut(room_id) else {
            return;
        };
        let mut changed = false;
        for list in t.reactions.values_mut() {
            let before = list.len();
            list.retain(|r| r.event_id != redacted_event_id);
            changed |= list.len() != before;
        }
        t.reactions.retain(|_, list| !list.is_empty());
        if !changed {
            return;
        }
        let out = Self::projected(t);
        let loaded = t.loaded;
        drop(tl);
        if notify && loaded {
            self.listener.on_timeline(room_id.to_string(), out);
        }
    }

    async fn page(
        &self,
        room: &Room,
        from: Option<String>,
    ) -> Result<(
        Vec<MessageRow>,
        Vec<(String, StoredReaction)>,
        Vec<String>,
        Option<String>,
    )> {
        let mut opts = MessagesOptions::backward();
        opts.from = from;
        opts.limit = UInt::from(HISTORY_PAGE);
        let page = room.messages(opts).await?;
        let events: Vec<Value> = page
            .chunk
            .iter()
            .filter_map(|e| serde_json::from_str::<Value>(e.raw().json().get()).ok())
            .collect();
        let names = self.names_for(room, &events).await;
        let rows: Vec<MessageRow> = events
            .iter()
            .filter_map(|v| message_row(v, &|m| names.get(m).cloned()))
            .collect();
        let reactions: Vec<(String, StoredReaction)> =
            events.iter().filter_map(reaction_from_event).collect();
        let redacted: Vec<String> = events.iter().filter_map(redacts_from_event).collect();
        Ok((rows, reactions, redacted, page.end))
    }

    fn ingest_side_effects(
        &self,
        room_id: &RoomId,
        reactions: Vec<(String, StoredReaction)>,
        redacted: Vec<String>,
    ) {
        for id in redacted {
            self.apply_redaction(room_id, &id, false);
        }
        for (target, reaction) in reactions {
            self.apply_reaction(room_id, target, reaction, false);
        }
    }

    /// The room's recent history (newest page), chronological. Later rows arrive through the
    /// listener. Marks the room as open; call again freely (cached after the first load).
    pub async fn open_room(&self, room_id: &str) -> Result<Vec<MessageRow>> {
        let room = self.room(room_id)?;
        let loaded = {
            let tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
            tl.get(room.room_id()).is_some_and(|t| t.loaded)
        };
        if loaded {
            return Ok(self.push_rows(room.room_id(), Vec::new(), false));
        }
        let (rows, reactions, redacted, end) = self.page(&room, None).await?;
        {
            let mut tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
            let t = tl.entry(room.room_id().to_owned()).or_default();
            t.loaded = true;
            t.at_start = end.is_none();
            t.prev_batch = end;
        }
        self.ingest_side_effects(room.room_id(), reactions, redacted);
        Ok(self.push_rows(room.room_id(), rows, false))
    }

    /// One more page of older history; `Ok(None)` at the beginning of the room.
    pub async fn load_older(&self, room_id: &str) -> Result<Option<Vec<MessageRow>>> {
        let room = self.room(room_id)?;
        let from = {
            let tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
            match tl.get(room.room_id()) {
                Some(t) if t.at_start => return Ok(None),
                Some(t) => t.prev_batch.clone(),
                None => None,
            }
        };
        let (rows, reactions, redacted, end) = self.page(&room, from).await?;
        {
            let mut tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
            let t = tl.entry(room.room_id().to_owned()).or_default();
            t.at_start = end.is_none() || rows.is_empty();
            t.prev_batch = end;
            t.loaded = true;
        }
        self.ingest_side_effects(room.room_id(), reactions, redacted);
        Ok(Some(self.push_rows(room.room_id(), rows, false)))
    }

    /// Send a text message; `local_id` (a ULID the app made) becomes `life.arbos.id` and the
    /// transaction id, so a retry never doubles it. Returns the event id.
    pub async fn send_text(&self, room_id: &str, text: &str, local_id: &str) -> Result<String> {
        let room = self.room(room_id)?;
        let content = json!({ "msgtype": "m.text", "body": text, "life.arbos.id": local_id });
        let txn: &TransactionId = local_id.into();
        let r = room
            .send_raw("m.room.message", content)
            .with_transaction_id(txn)
            .await?;
        Ok(r.response.event_id.to_string())
    }

    /// Add or remove this device's emoji on a channel message (`message_id` is the app's
    /// `life.arbos.id`, or the Matrix event id when that was never set). Members may react
    /// even when the channel is announcement-only (`events_default` blocks posts, not
    /// `m.reaction`).
    pub async fn toggle_reaction(
        &self,
        room_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<()> {
        if emoji.is_empty() || message_id.is_empty() {
            return Ok(());
        }
        let room = self.room(room_id)?;
        let me = member_of_mxid(self.me.as_str());
        let (target_event_id, existing) = {
            let tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
            let Some(t) = tl.get(room.room_id()) else {
                return Err(Error::NoRoom(room_id.to_owned()));
            };
            let Some(row) = t
                .rows
                .iter()
                .find(|r| r.id == message_id || r.event_id == message_id)
            else {
                return Err(Error::Homeserver(format!(
                    "no message {message_id} in this channel"
                )));
            };
            let existing = t.reactions.get(&row.event_id).and_then(|list| {
                list.iter()
                    .find(|r| r.sender == me && r.emoji == emoji)
                    .map(|r| r.event_id.clone())
            });
            (row.event_id.clone(), existing)
        };
        if let Some(reaction_event_id) = existing {
            let eid =
                EventId::parse(&reaction_event_id).map_err(|e| Error::Homeserver(e.to_string()))?;
            room.redact(&eid, None, None).await?;
            self.apply_redaction(room.room_id(), &reaction_event_id, true);
            return Ok(());
        }
        let content = json!({
            "m.relates_to": {
                "rel_type": "m.annotation",
                "event_id": target_event_id,
                "key": emoji,
            }
        });
        let r = room.send_raw("m.reaction", content).await?;
        self.apply_reaction(
            room.room_id(),
            target_event_id,
            StoredReaction {
                event_id: r.response.event_id.to_string(),
                sender: me,
                emoji: emoji.to_owned(),
            },
            true,
        );
        Ok(())
    }

    /// Public read receipt on the newest row the app has.
    pub async fn mark_read(&self, room_id: &str) -> Result<()> {
        let room = self.room(room_id)?;
        let last = {
            let tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
            tl.get(room.room_id())
                .and_then(|t| t.rows.last())
                .map(|r| r.event_id.clone())
        };
        let Some(last) = last else { return Ok(()) };
        let event_id = EventId::parse(&last).map_err(|e| Error::Homeserver(e.to_string()))?;
        room.send_single_receipt(ReceiptType::Read, ReceiptThread::Unthreaded, event_id)
            .await?;
        Ok(())
    }
}
