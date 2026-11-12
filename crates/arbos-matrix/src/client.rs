//! One `matrix-sdk` client per signed-in device, driven by sliding sync through the relay.
//!
//! The apps see three things: a [`Snapshot`] of servers and channels whenever the room list
//! changes, the timeline rows of a room they opened (and every new row after), and the result
//! of their own sends. Everything else — tokens, retries, pagination tokens, the state events
//! that say which room is a channel of which server, encryption keys — stays in here.

use std::collections::{BTreeMap, HashMap, HashSet};
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
use matrix_sdk::ruma::api::client::push::set_pushrule;
use matrix_sdk::ruma::api::client::receipt::create_receipt::v3::ReceiptType;
use matrix_sdk::ruma::api::client::sync::sync_events::v5 as sliding;
use matrix_sdk::ruma::events::receipt::ReceiptThread;
use matrix_sdk::ruma::events::room::encrypted::OriginalSyncRoomEncryptedEvent;
use matrix_sdk::ruma::events::{AnySyncTimelineEvent, MessageLikeEventType, StateEventType};
use matrix_sdk::ruma::push::NewPushRule;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{EventId, OwnedRoomId, OwnedUserId, RoomId, TransactionId, UInt, UserId};
use matrix_sdk::{Client, Room, SessionMeta, SlidingSyncList, SlidingSyncMode};
use matrix_sdk_crypto::vodozemac::Ed25519PublicKey;
use matrix_sdk_crypto::{CollectStrategy, DecryptionSettings, TrustRequirement};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::rows::{
    self, member_of_mxid, message_row, ChannelRow, MessageRow, PeopleRoomRow, SpaceRow,
};
use crate::{Error, Result};

/// What `GET /me/matrix` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub homeserver: String,
    pub user_id: String,
    pub access_token: String,
    pub device_id: String,
    /// The relay's per-user switch (`users.matrix_people_native`): DMs and groups are listed
    /// in the [`Snapshot`] and read from the homeserver. Off unless the relay says so.
    #[serde(default)]
    pub people_native: bool,
    /// The key the relay signs a chat's earlier history with (`history_key`). This device
    /// keeps the first one it is told ([`pinned_history_key`]).
    #[serde(default)]
    pub history_key: Option<String>,
}

/// `GET /chats/{id}/matrix/history`, as the relay writes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryPage {
    v: u32,
    key: String,
    room_id: String,
    rows: Vec<rows::HistoryRow>,
    #[allow(dead_code)] // the app's to read: whether to ask again
    more: bool,
}

/// The most rows of one [`HistoryPage`] this device carries.
const HISTORY_BATCH: usize = 50;

/// The relay's history key this device goes by: the one in `store_dir` when there is one,
/// else `told`, which is then kept there. Pinned so that a relay answer some later day, from
/// whoever then answers as the relay, cannot make this device believe a different signer.
fn pinned_history_key(store_dir: &std::path::Path, told: Option<&str>) -> Option<Ed25519PublicKey> {
    let path = store_dir.join("history-key");
    if let Ok(pinned) = std::fs::read_to_string(&path) {
        if told.is_some_and(|t| t != pinned.trim()) {
            tracing::warn!("the relay names a history key other than the one this device pinned");
        }
        return Ed25519PublicKey::from_base64(pinned.trim()).ok();
    }
    let key = Ed25519PublicKey::from_base64(told?).ok()?;
    if let Err(e) = std::fs::write(&path, key.to_base64()) {
        tracing::warn!(error = %e, "the relay's history key could not be pinned");
    }
    Some(key)
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
    /// DMs and groups; empty unless [`Session::people_native`].
    #[serde(default)]
    pub people: Vec<PeopleRoomRow>,
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
    /// A people chat's reactions, by the reaction's own event id (what a removal names).
    reactions: HashMap<String, rows::Reaction>,
    /// The encrypted events this device could not read (a locked row, or a reaction), by
    /// event id: read again when a key arrives ([`MatrixClient::unlock`]).
    sealed: HashMap<String, String>,
}

impl Timeline {
    /// The rows as the apps get them: each with its reactions and the row it answers.
    fn view(&self) -> Vec<MessageRow> {
        let mut by_row: HashMap<&str, BTreeMap<&str, BTreeMap<&str, bool>>> = HashMap::new();
        for r in self.reactions.values() {
            by_row
                .entry(&r.target)
                .or_default()
                .entry(&r.key)
                .or_default()
                .insert(&r.sender, true);
        }
        self.rows
            .iter()
            .map(|row| {
                let mut row = row.clone();
                let reactions = by_row.get(row.event_id.as_str());
                let answers = rows::reply_target(&row)
                    .and_then(|e| self.rows.iter().find(|x| x.event_id == e))
                    .map(|x| x.id.as_str());
                if reactions.is_some() || answers.is_some() {
                    rows::decorate(&mut row, reactions.unwrap_or(&BTreeMap::new()), answers);
                }
                row
            })
            .collect()
    }
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
    people_native: bool,
    history_key: Option<Ed25519PublicKey>,
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
            people_native: session.people_native,
            history_key: pinned_history_key(&crypto.store_dir, session.history_key.as_deref()),
            listener,
            timelines: Mutex::new(HashMap::new()),
            stop,
        });
        this.install_event_handler();
        tokio::spawn(Arc::clone(&this).key_loop(stop_rx.clone()));
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
                        let Ok(v) = serde_json::from_str::<Value>(ev.json().get()) else {
                            return;
                        };
                        let locked = v["type"] == "m.room.encrypted";
                        this.ingest(&room, v, info.is_some()).await;
                        // Its key may have landed (a backup download) while it was read.
                        if locked {
                            this.unlock(&room).await;
                        }
                    }
                },
            );
    }

    /// One timeline event of a room, as it arrives or once its key does: folded into the
    /// room's rows and handed to the listener. `decrypted` is whether this device decrypted it.
    async fn ingest(&self, room: &Room, mut v: Value, decrypted: bool) {
        let encrypted = Self::is_encrypted(room).await;
        let chat = self.people_chat_id(room).await;
        if !self.takes(
            room.room_id(),
            &v,
            (encrypted, chat.is_some()),
            decrypted,
            true,
        ) {
            return;
        }
        v["room_id"] = json!(room.room_id().as_str());
        let names = self.names_for(room, std::slice::from_ref(&v)).await;
        if let Some(row) = self.row_of(room.room_id(), chat.as_deref(), &v, &names) {
            self.push_rows(room.room_id(), vec![row], true);
        }
    }

    /// The row of an event [`Self::takes`] let through; `chat` is the people chat the room
    /// is. There, an event that carries a row of the chat's earlier history is that row or
    /// nothing ([`rows::carried_row`]).
    fn row_of(
        &self,
        room_id: &RoomId,
        chat: Option<&str>,
        ev: &Value,
        names: &HashMap<String, String>,
    ) -> Option<MessageRow> {
        match chat {
            Some(chat) if ev["content"].get(rows::HISTORY).is_some() => {
                rows::carried_row(ev, room_id.as_str(), chat, self.history_key.as_ref()?)
            }
            _ => message_row(ev, &|m| names.get(m).cloned()),
        }
    }

    /// Whether `ev` becomes a row of the room, which is `(encrypted, a people chat)`. An
    /// encrypted event this device cannot read is kept for [`Self::unlock`], and a people
    /// chat's reaction or removal is folded into the rows here ([`Self::fold`]).
    fn takes(
        &self,
        room_id: &RoomId,
        ev: &Value,
        (encrypted, people): (bool, bool),
        decrypted: bool,
        notify: bool,
    ) -> bool {
        if ev["event_id"].is_null() {
            return false;
        }
        if ev["type"] == "m.room.encrypted" && !rows::is_redacted(ev) {
            self.seal(room_id, ev);
        }
        if people && self.fold(room_id, ev, !encrypted || decrypted, notify) {
            return false;
        }
        rows::shows(ev, encrypted, decrypted)
            && (!people || rows::sender_is_own(ev) || ev["content"].get(rows::HISTORY).is_some())
    }

    /// Keep an encrypted event for [`Self::unlock`].
    fn seal(&self, room_id: &RoomId, ev: &Value) {
        let Some(id) = ev["event_id"].as_str() else {
            return;
        };
        let mut tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
        tl.entry(room_id.to_owned())
            .or_default()
            .sealed
            .insert(id.to_owned(), ev.to_string());
    }

    /// Read the room's encrypted events this device could not, again. A key can arrive after its message (sent late, or
    /// downloaded from the backup once the passphrase opened it); the row then becomes the
    /// message, through the same rules as an event that arrived readable.
    async fn unlock(&self, room: &Room) {
        let sealed: Vec<String> = {
            let tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
            tl.get(room.room_id())
                .map(|t| t.sealed.values().cloned().collect())
                .unwrap_or_default()
        };
        for json in sealed {
            let Ok(raw) = Raw::<OriginalSyncRoomEncryptedEvent>::from_json_string(json) else {
                continue;
            };
            let Ok(ev) = room.decrypt_event(&raw, None).await else {
                continue;
            };
            if ev.encryption_info().is_none() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(ev.raw().json().get()) {
                self.ingest(room, v, true).await;
            }
        }
    }

    /// Every room key this device receives (from a member's device or the backup) opens the
    /// locked rows of its room.
    async fn key_loop(self: Arc<Self>, mut stop: watch::Receiver<bool>) {
        let Some(keys) = self.client.encryption().room_keys_received_stream().await else {
            return;
        };
        futures::pin_mut!(keys);
        loop {
            let rooms: HashSet<OwnedRoomId> = tokio::select! {
                _ = stop.changed() => return,
                next = keys.next() => match next {
                    Some(Ok(keys)) => keys.into_iter().map(|k| k.room_id).collect(),
                    // Missed some: every room this device has read.
                    Some(Err(_)) => {
                        let tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
                        tl.keys().cloned().collect()
                    }
                    None => return,
                },
            };
            for id in rooms {
                if let Some(room) = self.client.get_room(&id) {
                    self.unlock(&room).await;
                }
            }
        }
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
        let mut reactions_quiet = false;
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
                                // Who joined and who left, as it happens: an encrypted
                                // room's next key goes to the members this device knows of.
                                (StateEventType::RoomMember, "$LAZY".to_owned()),
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
                            if self.people_native && !reactions_quiet {
                                reactions_quiet = self.quiet_reactions().await.is_ok();
                            }
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

    /// Tell the homeserver a reaction is not an unread message ([`rows::reaction_push_rule`]):
    /// it cannot tell by itself once reactions are encrypted. The same rule every time, so
    /// asked once per run, after the first sync that got through.
    async fn quiet_reactions(&self) -> Result<()> {
        let rule = serde_json::from_value(rows::reaction_push_rule())
            .map_err(|e| Error::Homeserver(e.to_string()))?;
        self.client
            .send(set_pushrule::v3::Request::new(NewPushRule::Override(rule)))
            .await?;
        Ok(())
    }

    // ---- rooms --------------------------------------------------------------------------------

    async fn arbos_state(room: &Room) -> Option<Value> {
        Self::state_content(room, StateEventType::from("life.arbos.chat")).await
    }

    async fn state_content(room: &Room, kind: StateEventType) -> Option<Value> {
        let raw = room.get_state_event(kind, "").await.ok().flatten()?;
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

    /// `(chat_id, kind)` when the room is an open people chat ([`rows::people_chat`]).
    async fn people_chat(room: &Room) -> Option<(String, String)> {
        let state = Self::arbos_state(room).await?;
        let create = Self::state_content(room, StateEventType::RoomCreate).await?;
        rows::people_chat(&state, &create)
    }

    /// Whether the room's rows follow the people-chat rule ([`rows::sender_is_own`]).
    async fn is_people(&self, room: &Room) -> bool {
        self.people_chat_id(room).await.is_some()
    }

    /// The people chat a room is, for a device that reads people chats here.
    async fn people_chat_id(&self, room: &Room) -> Option<String> {
        if !self.people_native {
            return None;
        }
        Self::people_chat(room).await.map(|(chat_id, _)| chat_id)
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
        let mut people: Vec<PeopleRoomRow> = Vec::new();
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
                Some("dm" | "group") if self.people_native => {
                    let Some((chat_id, kind)) = Self::people_chat(&room).await else {
                        continue;
                    };
                    let counts = room.unread_notification_counts();
                    people.push(PeopleRoomRow {
                        room_id: room.room_id().to_string(),
                        chat_id,
                        kind,
                        encrypted: Self::is_encrypted(&room).await,
                        unread: counts.notification_count,
                        mentions: counts.highlight_count,
                        last_text,
                        last_sender,
                        last_sender_name,
                        last_ts,
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
        people.sort_by(|a, b| b.last_ts.cmp(&a.last_ts).then(a.chat_id.cmp(&b.chat_id)));
        Snapshot {
            spaces,
            channels,
            people,
        }
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

    /// A people chat's reaction or removal, folded into the room's rows; `false` when `ev`
    /// is neither. `vouched` is false for an event that did not arrive encrypted in an
    /// encrypted room, where a reaction is dropped like any such message ([`rows::shows`]).
    /// A removal is never encrypted and the homeserver, which can withhold any event, is
    /// taken at its word for it.
    fn fold(&self, room_id: &RoomId, ev: &Value, vouched: bool, notify: bool) -> bool {
        let removes = rows::redacts(ev);
        // An encrypted reaction this device has no key for is not a locked message.
        let reacts = ev["type"] == rows::REACTION || rows::sealed_reaction(ev);
        if removes.is_none() && !reacts {
            return false;
        }
        let mut tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
        let t = tl.entry(room_id.to_owned()).or_default();
        let before = t.rows.len();
        let changed = match (removes, rows::reaction(ev).filter(|_| vouched)) {
            (Some(gone), _) => {
                t.rows.retain(|r| r.event_id != gone);
                t.sealed.remove(&gone);
                t.reactions.remove(&gone).is_some()
            }
            (None, Some((id, r))) => {
                // Read at last: no longer kept, and no locked row stands in for it (one did
                // if its envelope did not say what it was).
                t.rows.retain(|x| x.event_id != id);
                t.sealed.remove(&id);
                t.reactions.insert(id, r.clone()) != Some(r)
            }
            (None, None) => false,
        } || t.rows.len() != before;
        let (out, loaded) = (t.view(), t.loaded);
        drop(tl);
        if changed && notify && loaded {
            self.listener.on_timeline(room_id.to_string(), out);
        }
        true
    }

    /// Merge rows into a room's timeline (deduplicated by event id, kept in time order) and
    /// tell the listener when something changed.
    fn push_rows(&self, room_id: &RoomId, rows: Vec<MessageRow>, notify: bool) -> Vec<MessageRow> {
        let mut tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
        let t = tl.entry(room_id.to_owned()).or_default();
        let mut changed = false;
        for r in rows {
            // The event of a message this device sent takes its pending row's place.
            if !r.pending {
                let before = t.rows.len();
                t.rows
                    .retain(|x| !(x.pending && x.id == r.id && x.sender == r.sender));
                changed |= t.rows.len() != before;
            }
            // One row per id and sender, whichever event brought it: a row carried twice (two
            // devices) is drawn once, and the sender's own event for it is the one kept.
            let same = |x: &MessageRow| {
                x.id == r.id && x.sender.eq_ignore_ascii_case(&r.sender) && x.event_id != r.event_id
            };
            if r.carried && t.rows.iter().any(|x| !x.pending && same(x)) {
                continue;
            }
            if !r.carried && !r.pending && !r.locked {
                let before = t.rows.len();
                t.rows.retain(|x| !(x.carried && same(x)));
                changed |= t.rows.len() != before;
            }
            // Same event again: keep the newest reading (a locked row becomes the message once
            // its key arrives).
            if !r.locked {
                t.sealed.remove(&r.event_id);
            }
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
        let out = t.view();
        let loaded = t.loaded;
        drop(tl);
        if changed && notify && loaded {
            self.listener.on_timeline(room_id.to_string(), out.clone());
        }
        out
    }

    /// Take a pending row back out (its send failed).
    fn drop_pending(&self, room_id: &RoomId, local_id: &str) {
        let mut tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
        let Some(t) = tl.get_mut(room_id) else { return };
        t.rows.retain(|r| !(r.pending && r.id == local_id));
        let (out, loaded) = (t.view(), t.loaded);
        drop(tl);
        if loaded {
            self.listener.on_timeline(room_id.to_string(), out);
        }
    }

    /// Whether `event_id` is a row of the room this device has read from the homeserver.
    fn has_row(&self, room_id: &RoomId, event_id: &str) -> bool {
        let tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
        tl.get(room_id)
            .is_some_and(|t| t.rows.iter().any(|r| !r.pending && r.event_id == event_id))
    }

    /// The room, when it is a people chat this device reads natively: the only rooms whose
    /// rows carry reactions, removals and replies ([`Self::fold`]).
    async fn people_room(&self, room_id: &str) -> Result<Room> {
        let room = self.room(room_id)?;
        if !self.is_people(&room).await {
            return Err(Error::NoRoom(room_id.to_owned()));
        }
        Ok(room)
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
        let chat = self.people_chat_id(room).await;
        let events: Vec<Value> = page
            .chunk
            .iter()
            .filter_map(|e| {
                let v = serde_json::from_str::<Value>(e.raw().json().get()).ok()?;
                let decrypted = e.encryption_info().is_some();
                self.takes(
                    room.room_id(),
                    &v,
                    (encrypted, chat.is_some()),
                    decrypted,
                    false,
                )
                .then_some(v)
            })
            .collect();
        let names = self.names_for(room, &events).await;
        let rows: Vec<MessageRow> = events
            .iter()
            .filter_map(|v| self.row_of(room.room_id(), chat.as_deref(), v, &names))
            .collect();
        Ok((rows, page.end))
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
            // A sender's device can become trusted with no key arriving: look again here.
            self.unlock(&room).await;
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
        self.send(room_id, text, Vec::new(), None, local_id).await
    }

    /// [`Self::send_text`] with extra transcript parts (`life.arbos.parts`, e.g. a cheque);
    /// `text` is what other Matrix clients and the notification show. `reply_to` is the event
    /// id of the row it answers (people chats; a row this device has read in this room).
    ///
    /// In a people chat the row is in the timeline before the homeserver has it
    /// ([`rows::echo_row`], `pending`) and leaves it again if the send fails. An encrypted
    /// room's message is encrypted or not sent: the SDK asks the homeserver when it does not
    /// know the room's state and fails the send when that fails.
    pub async fn send(
        &self,
        room_id: &str,
        text: &str,
        parts: Vec<Value>,
        reply_to: Option<&str>,
        local_id: &str,
    ) -> Result<String> {
        let room = self.room(room_id)?;
        let people = self.is_people(&room).await;
        let mut content = json!({ "msgtype": "m.text", "body": text, "life.arbos.id": local_id });
        if !parts.is_empty() {
            content["life.arbos.parts"] = Value::Array(parts);
        }
        if let Some(target) = reply_to {
            if !people || !self.has_row(room.room_id(), target) {
                return Err(Error::Homeserver("no such message to reply to".into()));
            }
            content[rows::REPLY_TO] = json!(target);
        }
        let pending = people && {
            let known = {
                let tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
                tl.get(room.room_id())
                    .is_some_and(|t| t.rows.iter().any(|r| r.id == local_id))
            };
            // A retry of a message that already went through shows nothing twice.
            match rows::echo_row(self.me.as_str(), &content, local_id, now_ms()) {
                Some(row) if !known => {
                    self.push_rows(room.room_id(), vec![row], true);
                    true
                }
                _ => false,
            }
        };
        let txn: &TransactionId = local_id.into();
        let sent = room
            .send_raw("m.room.message", content)
            .with_transaction_id(txn)
            .await;
        if sent.is_err() && pending {
            self.drop_pending(room.room_id(), local_id);
        }
        Ok(sent?.response.event_id.to_string())
    }

    /// Carry rows of a chat's earlier history into its room: `page` is what the relay
    /// answered `GET /chats/{id}/matrix/history`, the result what to `POST` back
    /// (`{"carried":[{"id","event_id"}]}`), so the relay stops offering them.
    ///
    /// Nothing is sent unless the page is for this room, under the history key this device
    /// pinned, and every row in it verifies: what this device puts in the room under the
    /// relay's name is what the relay signed. Each row is one event, encrypted where the room
    /// is; its transaction id is the row's, so a batch sent again doubles nothing. A send that
    /// fails ends the batch, and what went before it is reported.
    ///
    /// Into an encrypted room only a device its owner's identity has signed carries: what
    /// any other device sends stays locked for everyone, and the relay would count the row as
    /// carried all the same.
    pub async fn carry_history(&self, room_id: &str, page: &str) -> Result<String> {
        let refused = |why: &str| Error::Homeserver(format!("history not carried: {why}"));
        let room = self.people_room(room_id).await?;
        let chat = self
            .people_chat_id(&room)
            .await
            .ok_or_else(|| Error::NoRoom(room_id.to_owned()))?;
        let page: HistoryPage =
            serde_json::from_str(page).map_err(|_| refused("not a page of history"))?;
        let key = self
            .history_key
            .as_ref()
            .ok_or_else(|| refused("no history key"))?;
        if page.v != 1
            || page.room_id != room_id
            || page.key != key.to_base64()
            || page.rows.len() > HISTORY_BATCH
            || !page
                .rows
                .iter()
                .all(|r| rows::history_verified(key, room_id, &chat, r))
        {
            return Err(refused("not this room's history"));
        }
        let vouched = match self.client.encryption().get_own_device().await {
            Ok(Some(device)) => device.is_cross_signed_by_owner(),
            _ => false,
        };
        if Self::is_encrypted(&room).await && !vouched {
            return Err(Error::Keys(
                "this device has not opened the key backup".into(),
            ));
        }
        let mut carried = Vec::new();
        for row in &page.rows {
            let txn = format!("h{}", row.id);
            let txn: &TransactionId = txn.as_str().into();
            match room
                .send_raw("m.room.message", rows::history_content(row))
                .with_transaction_id(txn)
                .await
            {
                Ok(sent) => {
                    carried.push(json!({ "id": row.id, "event_id": sent.response.event_id }))
                }
                Err(e) if carried.is_empty() => return Err(e.into()),
                Err(_) => break,
            }
        }
        Ok(json!({ "carried": carried }).to_string())
    }

    /// Put (`on`) or take back this person's reaction `key` on a row of a people chat. Asking
    /// for what already holds does nothing. The row changes when the homeserver's event
    /// arrives, not before. In an encrypted chat the reaction is encrypted like a message:
    /// the homeserver learns that this account reacted, not to which row nor with what
    /// ([`rows::reaction_content`]).
    pub async fn react(&self, room_id: &str, event_id: &str, key: &str, on: bool) -> Result<()> {
        let room = self.people_room(room_id).await?;
        if !rows::is_reaction_key(key) || !self.has_row(room.room_id(), event_id) {
            return Err(Error::Homeserver("no such message or reaction".into()));
        }
        let me = self.me_member();
        // Every one of them: two devices of this person can each have put it on.
        let mine: Vec<String> = {
            let tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
            tl.get(room.room_id())
                .map(|t| {
                    t.reactions
                        .iter()
                        .filter(|(_, r)| r.target == event_id && r.key == key && r.sender == me)
                        .map(|(id, _)| id.clone())
                        .collect()
                })
                .unwrap_or_default()
        };
        if on && mine.is_empty() {
            room.send_raw(rows::REACTION, rows::reaction_content(event_id, key))
                .await?;
        }
        if !on {
            for reaction in mine {
                self.redact(room_id, &reaction).await?;
            }
        }
        Ok(())
    }

    /// Remove an event of a people chat (a message, or a reaction by its own event id). The
    /// homeserver decides who may: the sender, and the room's moderators. The row leaves the
    /// timeline when the removal arrives.
    pub async fn redact(&self, room_id: &str, event_id: &str) -> Result<()> {
        let room = self.people_room(room_id).await?;
        let event_id = EventId::parse(event_id).map_err(|e| Error::Homeserver(e.to_string()))?;
        room.redact(&event_id, None, None).await?;
        Ok(())
    }

    /// Read receipt on one row, private ([`RECEIPT_KIND`]).
    pub async fn mark_read_at(&self, room_id: &str, event_id: &str) -> Result<()> {
        let room = self.room(room_id)?;
        let event_id = EventId::parse(event_id).map_err(|e| Error::Homeserver(e.to_string()))?;
        room.send_single_receipt(RECEIPT_KIND, ReceiptThread::Unthreaded, event_id)
            .await?;
        Ok(())
    }

    /// Read receipt on the newest row the app has, private ([`RECEIPT_KIND`]).
    pub async fn mark_read(&self, room_id: &str) -> Result<()> {
        let room = self.room(room_id)?;
        let last = {
            let tl = self.timelines.lock().unwrap_or_else(|p| p.into_inner());
            tl.get(room.room_id())
                .and_then(|t| t.rows.iter().rfind(|r| !r.pending))
                .map(|r| r.event_id.clone())
        };
        let Some(last) = last else { return Ok(()) };
        self.mark_read_at(room_id, &last).await
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
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
                people_native: false,
                history_key: None,
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
