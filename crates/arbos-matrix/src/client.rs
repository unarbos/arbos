//! One `matrix-sdk` client per signed-in device, driven by sliding sync through the relay.
//!
//! The apps see three things: a [`Snapshot`] of servers and channels whenever the room list
//! changes, the timeline rows of a room they opened (and every new row after), and the result
//! of their own sends. Everything else — tokens, retries, pagination tokens, the state events
//! that say which room is a channel of which server, encryption keys — stays in here.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use matrix_sdk::authentication::matrix::MatrixSession;
use matrix_sdk::authentication::SessionTokens;
use matrix_sdk::deserialized_responses::EncryptionInfo;
use matrix_sdk::encryption::recovery::RecoveryState;
use matrix_sdk::encryption::{BackupDownloadStrategy, EncryptionSettings};
use matrix_sdk::room::MessagesOptions;
use matrix_sdk::ruma::api::client::receipt::create_receipt::v3::ReceiptType;
use matrix_sdk::ruma::api::client::sync::sync_events::v5 as sliding;
use matrix_sdk::ruma::events::receipt::ReceiptThread;
use matrix_sdk::ruma::events::{AnySyncTimelineEvent, MessageLikeEventType, StateEventType};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId, RoomId, TransactionId, UInt, UserId};
use matrix_sdk::{Client, Room, SessionMeta, SlidingSyncList, SlidingSyncMode};
use matrix_sdk_crypto::{CollectStrategy, DecryptionSettings, TrustRequirement};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::rows::{self, member_of_mxid, message_row, ChannelRow, MessageRow, SpaceRow};
use crate::{Error, Result};

/// What `GET /me/matrix` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub homeserver: String,
    pub user_id: String,
    pub access_token: String,
    pub device_id: String,
}

/// Where this device keeps its encryption keys.
#[derive(Clone)]
pub struct Crypto {
    /// This device's store (keys and room state). One directory per Matrix device id.
    pub store_dir: PathBuf,
    /// Encrypts the store at rest; the app keeps it in the keychain.
    pub store_key: Option<String>,
    /// Whether this device makes the account's cross-signing identity and key backup when
    /// the account has none. Only an app that then puts them under the person's phrase
    /// ([`MatrixClient::recover`]) may: no later device can open, or replace, what a device
    /// that cannot has made.
    pub first_device: bool,
}

/// The account's key backup, as the first sync reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backup {
    /// Set up, and this device holds the keys.
    On,
    /// Never set up: this is the account's first encrypting device.
    Off,
    /// Set up on another device; this one needs the passphrase ([`MatrixClient::recover`]).
    Locked,
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
    /// `/messages` token for the next page back; `None` before the first page or at the start.
    prev_batch: Option<String>,
    /// The first page was loaded (so the rows are complete from `prev_batch` to now).
    loaded: bool,
    at_start: bool,
}

/// Channel read receipts are private (#1484; Jacob, 2026-10-30: "make all mirrored read
/// receipts private"). Private still clears the room's own unread count and still reaches
/// this person's other devices; it is not federated and no other member can read it.
const RECEIPT_KIND: ReceiptType = ReceiptType::ReadPrivate;

const HISTORY_PAGE: u32 = 50;
const ROOM_RANGE_END: u32 = 1999;
const POLL_TIMEOUT: Duration = Duration::from_secs(30);
const BACKUP_STATE_WAIT: Duration = Duration::from_secs(30);

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
    pub async fn start(
        session: Session,
        crypto: Crypto,
        listener: Arc<dyn Listener>,
    ) -> Result<Arc<Self>> {
        let user_id = UserId::parse(&session.user_id).map_err(|e| Error::Session(e.to_string()))?;
        let http = reqwest13::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent("Arbos")
            .build()
            .map_err(|e| Error::Homeserver(e.to_string()))?;
        let client = Client::builder()
            .homeserver_url(&session.homeserver)
            .http_client(http)
            .sqlite_store(&crypto.store_dir, crypto.store_key.as_deref())
            .with_encryption_settings(EncryptionSettings {
                auto_enable_cross_signing: crypto.first_device,
                backup_download_strategy: BackupDownloadStrategy::OneShot,
                auto_enable_backups: crypto.first_device,
            })
            // The homeserver can add a device to anyone's account. Only a device signed by
            // its owner's cross-signing identity is sent room keys, and a message from one
            // that is not stays locked.
            .with_room_key_recipient_strategy(CollectStrategy::IdentityBasedStrategy)
            .with_decryption_settings(DecryptionSettings {
                sender_device_trust_requirement: TrustRequirement::CrossSignedOrLegacy,
            })
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

    /// The account's key backup, once the homeserver has said where it stands; an error when
    /// it has not within [`BACKUP_STATE_WAIT`] (offline, or the homeserver is not answering).
    pub async fn backup(&self) -> Result<Backup> {
        self.backup_within(BACKUP_STATE_WAIT).await
    }

    async fn backup_within(&self, wait: Duration) -> Result<Backup> {
        let mut states = self.client.encryption().recovery().state_stream();
        let known = async {
            while let Some(state) = states.next().await {
                match state {
                    RecoveryState::Unknown => continue,
                    RecoveryState::Enabled => return Backup::On,
                    RecoveryState::Disabled => return Backup::Off,
                    RecoveryState::Incomplete => return Backup::Locked,
                }
            }
            Backup::Locked
        };
        tokio::time::timeout(wait, known).await.map_err(|_| {
            Error::Keys("the homeserver has not said where the key backup stands".into())
        })
    }

    /// Back the room keys up under the person's recovery passphrase
    /// (`arbos_wallet::Seed::recovery_passphrase`): the account's first device sets up secret
    /// storage and key backup with it; every later device imports from it and then downloads
    /// the room keys ([`BackupDownloadStrategy::OneShot`]), so a new phone reads the old
    /// history. A wrong passphrase on a later device is an error and changes nothing.
    pub async fn recover(&self, passphrase: &str) -> Result<()> {
        let encryption = self.client.encryption();
        let recovery = encryption.recovery();
        let done = match self.backup().await? {
            Backup::On => Ok(()),
            // `Off` is what this device last heard. Another device of the account may have set
            // the backup up since (two new devices, both told `Off`), and enabling again would
            // replace its secret storage: ask the homeserver, and open what is there instead.
            // The homeserver has no compare-and-set for account data, so two devices inside
            // the same few round trips of `enable` can still both win.
            Backup::Off if !encryption.secret_storage().is_enabled().await? => recovery
                .enable()
                .with_passphrase(passphrase)
                .await
                .map(drop),
            Backup::Off | Backup::Locked => recovery.recover(passphrase).await,
        };
        done.map_err(|e| Error::Keys(e.to_string()))
    }

    /// Whether `passphrase` opens the account's secret storage; nothing is imported or
    /// changed. [`Self::recover`] does not look at the passphrase once this device holds the
    /// keys ([`Backup::On`]), so this is how a typed phrase is checked against the account.
    pub async fn check_passphrase(&self, passphrase: &str) -> Result<()> {
        self.client
            .encryption()
            .secret_storage()
            .open_secret_store(passphrase)
            .await
            .map(drop)
            .map_err(|e| Error::Keys(e.to_string()))
    }

    // ---- sync ---------------------------------------------------------------------------------

    fn install_event_handler(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.client
            // Every timeline event: messages, and encrypted ones this device has no key for
            // yet (a locked row, replaced once the key arrives).
            .add_event_handler(
                move |ev: Raw<AnySyncTimelineEvent>, room: Room, info: Option<EncryptionInfo>| {
                    let weak = weak.clone();
                    async move {
                        let Some(this) = weak.upgrade() else { return };
                        let Ok(mut v) = serde_json::from_str::<Value>(ev.json().get()) else {
                            return;
                        };
                        let encrypted = Self::is_encrypted(&room).await;
                        if v["event_id"].is_null() || !rows::shows(&v, encrypted, info.is_some()) {
                            return;
                        }
                        v["room_id"] = json!(room.room_id().as_str());
                        let names = this.names_for(&room, std::slice::from_ref(&v)).await;
                        if let Some(row) = message_row(&v, &|m| names.get(m).cloned()) {
                            this.push_rows(room.room_id(), vec![row], true);
                        }
                    }
                },
            );
    }

    /// Whether the room is end-to-end encrypted; not knowing counts as encrypted, so a
    /// cleartext message is never shown on a guess ([`rows::shows`]).
    async fn is_encrypted(room: &Room) -> bool {
        room.latest_encryption_state()
            .await
            .map_or(true, |s| s.is_encrypted())
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
                    // Keys travel as to-device messages and account data; device lists and
                    // one-time key counts ride the e2ee extension.
                    .with_e2ee_extension({
                        let mut e = sliding::request::E2EE::default();
                        e.enabled = Some(true);
                        e
                    })
                    .with_to_device_extension({
                        let mut t = sliding::request::ToDevice::default();
                        t.enabled = Some(true);
                        t
                    })
                    .with_account_data_extension({
                        let mut a = sliding::request::AccountData::default();
                        a.enabled = Some(true);
                        a
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
                                (StateEventType::RoomEncryption, String::new()),
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

    /// The newest row's (text, sender, sender name, time) for the list's preview.
    fn last_of(&self, room_id: &RoomId) -> (String, String, String, i64) {
        let tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
        tl.get(room_id)
            .and_then(|t| t.rows.last())
            .map(|r| {
                (
                    r.text.clone(),
                    r.sender.clone(),
                    r.sender_name.clone(),
                    r.created_at,
                )
            })
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
            let (last_text, last_sender, last_sender_name, last_ts) = self.last_of(room.room_id());
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
                            last_sender: String::new(),
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
                        last_sender,
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
                    space.last_sender = ch.last_sender.clone();
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

    /// Merge rows into a room's timeline (deduplicated by event id, kept in time order) and
    /// tell the listener when something changed.
    fn push_rows(&self, room_id: &RoomId, rows: Vec<MessageRow>, notify: bool) -> Vec<MessageRow> {
        let mut tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
        let t = tl.entry(room_id.to_owned()).or_default();
        let mut changed = false;
        for r in rows {
            // Same event again: keep the newest reading (a locked row becomes the message once
            // its key arrives).
            if let Some(x) = t.rows.iter_mut().find(|x| x.event_id == r.event_id) {
                if *x != r {
                    *x = r;
                    changed = true;
                }
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
        let out = t.rows.clone();
        let loaded = t.loaded;
        drop(tl);
        if changed && notify && loaded {
            self.listener.on_timeline(room_id.to_string(), out.clone());
        }
        out
    }

    async fn page(
        &self,
        room: &Room,
        from: Option<String>,
    ) -> Result<(Vec<MessageRow>, Option<String>)> {
        let mut opts = MessagesOptions::backward();
        opts.from = from;
        opts.limit = UInt::from(HISTORY_PAGE);
        let page = room.messages(opts).await?;
        let encrypted = Self::is_encrypted(room).await;
        let events: Vec<Value> = page
            .chunk
            .iter()
            .filter_map(|e| {
                let v = serde_json::from_str::<Value>(e.raw().json().get()).ok()?;
                rows::shows(&v, encrypted, e.encryption_info().is_some()).then_some(v)
            })
            .collect();
        let names = self.names_for(room, &events).await;
        let rows: Vec<MessageRow> = events
            .iter()
            .filter_map(|v| message_row(v, &|m| names.get(m).cloned()))
            .collect();
        Ok((rows, page.end))
    }

    /// The room's recent history (newest page), chronological. Later rows arrive through the
    /// listener. Marks the room as open; call again freely (cached after the first load).
    pub async fn open_room(&self, room_id: &str) -> Result<Vec<MessageRow>> {
        let room = self.room(room_id)?;
        let loaded = {
            let tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
            // A locked row means keys may have arrived since: read the newest page again.
            tl.get(room.room_id())
                .is_some_and(|t| t.loaded && !t.rows.iter().any(|r| r.locked))
        };
        if loaded {
            return Ok(self.push_rows(room.room_id(), Vec::new(), false));
        }
        let (rows, end) = self.page(&room, None).await?;
        {
            let mut tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
            let t = tl.entry(room.room_id().to_owned()).or_default();
            t.loaded = true;
            t.at_start = end.is_none();
            t.prev_batch = end;
        }
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
        let (rows, end) = self.page(&room, from).await?;
        {
            let mut tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
            let t = tl.entry(room.room_id().to_owned()).or_default();
            t.at_start = end.is_none() || rows.is_empty();
            t.prev_batch = end;
            t.loaded = true;
        }
        Ok(Some(self.push_rows(room.room_id(), rows, false)))
    }

    /// Send a text message; `local_id` (a ULID the app made) becomes `life.arbos.id` and the
    /// transaction id, so a retry never doubles it. Returns the event id.
    pub async fn send_text(&self, room_id: &str, text: &str, local_id: &str) -> Result<String> {
        self.send(room_id, text, Vec::new(), local_id).await
    }

    /// [`Self::send_text`] with extra transcript parts (`life.arbos.parts`, e.g. a cheque);
    /// `text` is what other Matrix clients and the notification show.
    pub async fn send(
        &self,
        room_id: &str,
        text: &str,
        parts: Vec<Value>,
        local_id: &str,
    ) -> Result<String> {
        let room = self.room(room_id)?;
        let mut content = json!({ "msgtype": "m.text", "body": text, "life.arbos.id": local_id });
        if !parts.is_empty() {
            content["life.arbos.parts"] = Value::Array(parts);
        }
        let txn: &TransactionId = local_id.into();
        let r = room
            .send_raw("m.room.message", content)
            .with_transaction_id(txn)
            .await?;
        Ok(r.response.event_id.to_string())
    }

    /// Read receipt on the newest row the app has, private ([`RECEIPT_KIND`]).
    pub async fn mark_read(&self, room_id: &str) -> Result<()> {
        let room = self.room(room_id)?;
        let last = {
            let tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
            tl.get(room.room_id())
                .and_then(|t| t.rows.last())
                .map(|r| r.event_id.clone())
        };
        let Some(last) = last else { return Ok(()) };
        let event_id = matrix_sdk::ruma::EventId::parse(&last)
            .map_err(|e| Error::Homeserver(e.to_string()))?;
        room.send_single_receipt(RECEIPT_KIND, ReceiptThread::Unthreaded, event_id)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod receipt_kind_tests {
    use super::*;

    #[test]
    fn a_channel_receipt_goes_to_the_private_endpoint() {
        // The wire string, not the variant name: this is the part of the URL the homeserver
        // reads, and it is what decides whether other members see the read.
        assert_eq!(RECEIPT_KIND.as_str(), "m.read.private");
        assert_ne!(RECEIPT_KIND.as_str(), ReceiptType::Read.as_str());
    }
}

#[cfg(test)]
mod backup_tests {
    use super::*;

    struct Quiet;

    impl Listener for Quiet {
        fn on_rooms(&self, _: Snapshot) {}
        fn on_timeline(&self, _: String, _: Vec<MessageRow>) {}
        fn on_connection(&self, _: bool, _: Option<String>) {}
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_homeserver_that_never_answers_is_an_error_not_a_hang() {
        let dir = std::env::temp_dir().join(format!("arbos-backup-wait-{}", std::process::id()));
        let client = MatrixClient::start(
            Session {
                // Nothing listens on the discard port.
                homeserver: "http://127.0.0.1:9".into(),
                user_id: "@gh_1:arbos.test".into(),
                access_token: "t".into(),
                device_id: "D".into(),
            },
            Crypto {
                store_dir: dir.clone(),
                store_key: None,
                first_device: true,
            },
            Arc::new(Quiet),
        )
        .await
        .unwrap();
        let waited = client.backup_within(Duration::from_millis(300)).await;
        assert!(matches!(waited, Err(Error::Keys(_))), "{waited:?}");
        client.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
