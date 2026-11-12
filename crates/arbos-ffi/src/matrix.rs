//! Servers and channels for the Swift client (D88 C1): a thin UniFFI skin over
//! [`arbos_matrix::MatrixClient`]. Records mirror the core rows one to one so the core stays
//! free of UniFFI derives; the async calls run on the bindings' tokio runtime.

use std::sync::Arc;

use arbos_matrix::{
    ChannelRow as CoreChannel, Listener as CoreListener, MatrixClient as CoreClient,
    MessageRow as CoreMessage, PeopleRoomRow as CorePeople, Session as CoreSession,
    Snapshot as CoreSnapshot, SpaceRow as CoreSpace,
};

#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum MatrixError {
    #[error("{0}")]
    Session(String),
    #[error("{0}")]
    Homeserver(String),
    #[error("{0}")]
    NoRoom(String),
    #[error("{0}")]
    Keys(String),
    #[error("stopped")]
    Stopped,
}

impl From<arbos_matrix::Error> for MatrixError {
    fn from(e: arbos_matrix::Error) -> Self {
        match e {
            arbos_matrix::Error::Session(s) => Self::Session(s),
            arbos_matrix::Error::Homeserver(s) => Self::Homeserver(s),
            arbos_matrix::Error::NoRoom(s) => Self::NoRoom(s),
            arbos_matrix::Error::Keys(s) => Self::Keys(s),
            arbos_matrix::Error::Stopped => Self::Stopped,
        }
    }
}

/// What `GET /me/matrix` returned.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MatrixSession {
    pub homeserver: String,
    pub user_id: String,
    pub access_token: String,
    pub device_id: String,
    /// `people_native` of `GET /me/matrix`: list DMs and groups in the snapshot.
    #[uniffi(default = false)]
    pub people_native: bool,
    /// `history_key` of `GET /me/matrix`: the key the relay signs carried history with.
    #[uniffi(default = None)]
    pub history_key: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MatrixMessageRow {
    pub event_id: String,
    pub id: String,
    pub sender: String,
    pub sender_name: String,
    pub text: String,
    pub created_at: i64,
    /// This device has no key for the message (yet); `text` is a placeholder.
    pub locked: bool,
    /// This device's own send, shown before the homeserver has it; `event_id` is not an
    /// event's until the row comes again without this.
    #[uniffi(default = false)]
    pub pending: bool,
    /// Chat-doc message JSON (same shape `CoreChatDoc.readRowsJson` rows have).
    pub json: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MatrixChannelRow {
    pub room_id: String,
    pub channel_id: String,
    pub server_id: String,
    pub name: String,
    pub restricted: bool,
    pub can_post: bool,
    pub unread: u64,
    pub last_text: String,
    /// `user:<id>` | `agent:<project>` | `system` of the newest row.
    pub last_sender: String,
    pub last_sender_name: String,
    pub last_ts: i64,
    pub order: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MatrixSpaceRow {
    pub room_id: String,
    pub server_id: String,
    pub name: String,
    pub member_count: u64,
    pub unread: u64,
    pub last_text: String,
    pub last_sender: String,
    pub last_sender_name: String,
    pub last_ts: i64,
    pub channel_ids: Vec<String>,
}

/// A DM or group (see `arbos_matrix::PeopleRoomRow`).
#[derive(Debug, Clone, uniffi::Record)]
pub struct MatrixPeopleRow {
    pub room_id: String,
    pub chat_id: String,
    /// `dm` | `group`.
    pub kind: String,
    pub encrypted: bool,
    pub unread: u64,
    pub mentions: u64,
    pub last_text: String,
    pub last_sender: String,
    pub last_sender_name: String,
    pub last_ts: i64,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MatrixSnapshot {
    pub spaces: Vec<MatrixSpaceRow>,
    pub channels: Vec<MatrixChannelRow>,
    /// Empty unless the session said `people_native`.
    #[uniffi(default = [])]
    pub people: Vec<MatrixPeopleRow>,
}

/// One file of a message on its way out (see `arbos_matrix::Upload`).
#[derive(Clone, uniffi::Record)]
pub struct MatrixUpload {
    pub name: String,
    pub mime: String,
    /// The attachment's `kind` (`image`, `sticker`, …), when the app writes one.
    pub kind: Option<String>,
    pub bytes: Vec<u8>,
}

#[derive(Clone, uniffi::Record)]
pub struct MatrixMedia {
    pub mime: String,
    pub bytes: Vec<u8>,
}

fn row(r: CoreMessage) -> MatrixMessageRow {
    MatrixMessageRow {
        event_id: r.event_id,
        id: r.id,
        sender: r.sender,
        sender_name: r.sender_name,
        text: r.text,
        created_at: r.created_at,
        locked: r.locked,
        pending: r.pending,
        json: r.json,
    }
}

fn channel(c: CoreChannel) -> MatrixChannelRow {
    MatrixChannelRow {
        room_id: c.room_id,
        channel_id: c.channel_id,
        server_id: c.server_id,
        name: c.name,
        restricted: c.restricted,
        can_post: c.can_post,
        unread: c.unread,
        last_text: c.last_text,
        last_sender: c.last_sender,
        last_sender_name: c.last_sender_name,
        last_ts: c.last_ts,
        order: c.order,
    }
}

fn space(s: CoreSpace) -> MatrixSpaceRow {
    MatrixSpaceRow {
        room_id: s.room_id,
        server_id: s.server_id,
        name: s.name,
        member_count: s.member_count,
        unread: s.unread,
        last_text: s.last_text,
        last_sender: s.last_sender,
        last_sender_name: s.last_sender_name,
        last_ts: s.last_ts,
        channel_ids: s.channel_ids,
    }
}

fn people(p: CorePeople) -> MatrixPeopleRow {
    MatrixPeopleRow {
        room_id: p.room_id,
        chat_id: p.chat_id,
        kind: p.kind,
        encrypted: p.encrypted,
        unread: p.unread,
        mentions: p.mentions,
        last_text: p.last_text,
        last_sender: p.last_sender,
        last_sender_name: p.last_sender_name,
        last_ts: p.last_ts,
    }
}

fn snapshot(s: CoreSnapshot) -> MatrixSnapshot {
    MatrixSnapshot {
        spaces: s.spaces.into_iter().map(space).collect(),
        channels: s.channels.into_iter().map(channel).collect(),
        people: s.people.into_iter().map(people).collect(),
    }
}

/// Foreign callbacks, delivered from the sync task (hop to the main actor in Swift).
#[uniffi::export(foreign)]
pub trait MatrixListener: Send + Sync {
    fn on_rooms(&self, snapshot: MatrixSnapshot);
    fn on_timeline(&self, room_id: String, rows: Vec<MatrixMessageRow>);
    fn on_connection(&self, connected: bool, error: Option<String>);
}

struct Bridge(Arc<dyn MatrixListener>);

impl CoreListener for Bridge {
    fn on_rooms(&self, s: CoreSnapshot) {
        self.0.on_rooms(snapshot(s));
    }

    fn on_timeline(&self, room_id: String, rows: Vec<CoreMessage>) {
        self.0
            .on_timeline(room_id, rows.into_iter().map(row).collect());
    }

    fn on_connection(&self, connected: bool, error: Option<String>) {
        self.0.on_connection(connected, error);
    }
}

#[derive(uniffi::Object)]
pub struct MatrixClient {
    inner: Arc<CoreClient>,
}

/// Where this device keeps its encryption keys (see `arbos_matrix::Crypto`).
#[derive(Clone, uniffi::Record)]
pub struct MatrixCrypto {
    /// App-private directory, one per Matrix device id.
    pub store_dir: String,
    /// Keychain-held key that encrypts the store at rest.
    pub store_key: Option<String>,
}

/// The store key never reaches a log line.
impl std::fmt::Debug for MatrixCrypto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatrixCrypto")
            .field("store_dir", &self.store_dir)
            .field("store_key", &self.store_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Start syncing for this device. Returns once the client exists; the first snapshot comes
/// through the listener.
#[uniffi::export(async_runtime = "tokio")]
pub async fn matrix_start(
    session: MatrixSession,
    crypto: MatrixCrypto,
    listener: Arc<dyn MatrixListener>,
) -> Result<Arc<MatrixClient>, MatrixError> {
    let inner = CoreClient::start(
        CoreSession {
            homeserver: session.homeserver,
            user_id: session.user_id,
            access_token: session.access_token,
            device_id: session.device_id,
            people_native: session.people_native,
            history_key: session.history_key,
        },
        arbos_matrix::Crypto {
            store_dir: crypto.store_dir.into(),
            store_key: crypto.store_key,
            // The phone has the wallet: it makes the phrase the keys go under.
            first_device: true,
        },
        Arc::new(Bridge(listener)),
    )
    .await?;
    Ok(Arc::new(MatrixClient { inner }))
}

#[uniffi::export(async_runtime = "tokio")]
impl MatrixClient {
    pub fn stop(&self) {
        self.inner.stop();
    }

    pub fn user_id(&self) -> String {
        self.inner.user_id()
    }

    /// `user:<id>` for this device's person.
    pub fn me_member(&self) -> String {
        self.inner.me_member()
    }

    pub async fn snapshot(&self) -> MatrixSnapshot {
        snapshot(self.inner.snapshot().await)
    }

    pub async fn open_room(&self, room_id: String) -> Result<Vec<MatrixMessageRow>, MatrixError> {
        Ok(self
            .inner
            .open_room(&room_id)
            .await?
            .into_iter()
            .map(row)
            .collect())
    }

    /// `None` at the beginning of the room.
    pub async fn load_older(
        &self,
        room_id: String,
    ) -> Result<Option<Vec<MatrixMessageRow>>, MatrixError> {
        Ok(self
            .inner
            .load_older(&room_id)
            .await?
            .map(|rows| rows.into_iter().map(row).collect()))
    }

    pub async fn send_text(
        &self,
        room_id: String,
        text: String,
        local_id: String,
    ) -> Result<String, MatrixError> {
        Ok(self.inner.send_text(&room_id, &text, &local_id).await?)
    }

    /// [`Self::send_text`] plus transcript parts, each a JSON object (e.g. a cheque part, or
    /// an attachment already on the relay); `reply_to` is the event id of the row it answers.
    pub async fn send_parts(
        &self,
        room_id: String,
        text: String,
        parts_json: Vec<String>,
        reply_to: Option<String>,
        local_id: String,
    ) -> Result<String, MatrixError> {
        let parts = parts_json
            .iter()
            .map(|p| serde_json::from_str(p))
            .collect::<Result<Vec<serde_json::Value>, _>>()
            .map_err(|e| MatrixError::Homeserver(format!("bad part: {e}")))?;
        Ok(self
            .inner
            .send(&room_id, &text, parts, reply_to.as_deref(), &local_id)
            .await?)
    }

    /// A text message answering the row `reply_to` (its event id) in a people chat.
    pub async fn send_reply(
        &self,
        room_id: String,
        text: String,
        reply_to: String,
        local_id: String,
    ) -> Result<String, MatrixError> {
        Ok(self
            .inner
            .send(&room_id, &text, Vec::new(), Some(&reply_to), &local_id)
            .await?)
    }

    /// A message with files into a people chat, each encrypted on this device before it is
    /// uploaded (see `arbos_matrix::MatrixClient::send_files`). `reply_to` is an event id.
    pub async fn send_files(
        &self,
        room_id: String,
        text: String,
        files: Vec<MatrixUpload>,
        reply_to: Option<String>,
        local_id: String,
    ) -> Result<String, MatrixError> {
        let files = files
            .into_iter()
            .map(|f| arbos_matrix::Upload {
                name: f.name,
                mime: f.mime,
                kind: f.kind,
                bytes: f.bytes,
            })
            .collect();
        Ok(self
            .inner
            .send_files(&room_id, &text, files, reply_to.as_deref(), &local_id)
            .await?)
    }

    /// The bytes of a file of a people chat's message, decrypted: `reference` is what follows
    /// `mx:` in the attachment's `ref` (see `arbos_matrix::MatrixClient::media`).
    pub async fn media(
        &self,
        room_id: String,
        reference: String,
    ) -> Result<MatrixMedia, MatrixError> {
        let (mime, bytes) = self.inner.media(&room_id, &reference).await?;
        Ok(MatrixMedia { mime, bytes })
    }

    /// Carry rows of a people chat's earlier history into its room: `page_json` is the body
    /// of `GET /chats/{id}/matrix/history`, the result the body to `POST` back (see
    /// `arbos_matrix::MatrixClient::carry_history`).
    pub async fn carry_history(
        &self,
        room_id: String,
        page_json: String,
    ) -> Result<String, MatrixError> {
        Ok(self.inner.carry_history(&room_id, &page_json).await?)
    }

    /// Put (`on`) or take back my reaction `key` on a row of a people chat. Refused in an
    /// end-to-end encrypted chat (see `arbos_matrix::MatrixClient::react`).
    pub async fn react(
        &self,
        room_id: String,
        event_id: String,
        key: String,
        on: bool,
    ) -> Result<(), MatrixError> {
        Ok(self.inner.react(&room_id, &event_id, &key, on).await?)
    }

    /// Remove a message of a people chat (the homeserver decides who may).
    pub async fn redact(&self, room_id: String, event_id: String) -> Result<(), MatrixError> {
        Ok(self.inner.redact(&room_id, &event_id).await?)
    }

    /// Private read receipt on one row.
    pub async fn mark_read_at(&self, room_id: String, event_id: String) -> Result<(), MatrixError> {
        Ok(self.inner.mark_read_at(&room_id, &event_id).await?)
    }

    /// The account's key backup once the homeserver answered: `on` (this device has the keys),
    /// `off` (never set up: make the phrase, then `recover`), `locked` (set up elsewhere: needs
    /// the person's phrase), or `unknown` when it has not answered in time (ask again).
    pub async fn backup(&self) -> String {
        match self.inner.backup().await {
            Ok(arbos_matrix::Backup::On) => "on",
            Ok(arbos_matrix::Backup::Off) => "off",
            Ok(arbos_matrix::Backup::Locked) => "locked",
            Err(_) => "unknown",
        }
        .to_owned()
    }

    /// Set up (first device) or open (later devices) the key backup with
    /// `wallet_recovery_passphrase(phrase)`.
    pub async fn recover(&self, passphrase: String) -> Result<(), MatrixError> {
        Ok(self.inner.recover(&passphrase).await?)
    }

    /// Whether `wallet_recovery_passphrase(phrase)` opens this account's backup, changing
    /// nothing. `recover` succeeds without reading the passphrase once `backup` is `on`.
    pub async fn check_passphrase(&self, passphrase: String) -> Result<(), MatrixError> {
        Ok(self.inner.check_passphrase(&passphrase).await?)
    }

    pub async fn mark_read(&self, room_id: String) -> Result<(), MatrixError> {
        Ok(self.inner.mark_read(&room_id).await?)
    }
}

/// The words of an encrypted message, for its notification (see `arbos_matrix::Preview`).
#[derive(Debug, Clone, uniffi::Record)]
pub struct MatrixPreview {
    /// `user:<id>` of the account the message's key is from.
    pub sender: String,
    pub text: String,
}

/// For the notification extension: read one encrypted event (`event_json`, as the alert
/// carries it) with the keys in this device's store, or `None` (see
/// `arbos_matrix::preview`). No client and no request. Blocks the calling thread, on a
/// runtime of that one thread: the extension runs nothing else of this library and has
/// little memory to run it in.
#[uniffi::export]
pub fn matrix_preview(
    crypto: MatrixCrypto,
    user_id: String,
    device_id: String,
    room_id: String,
    event_json: String,
) -> Option<MatrixPreview> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    let preview = runtime.block_on(arbos_matrix::preview(
        crypto.store_dir.as_ref(),
        crypto.store_key.as_deref()?,
        &user_id,
        &device_id,
        &room_id,
        &event_json,
    ))?;
    Some(MatrixPreview {
        sender: preview.sender,
        text: preview.text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_does_not_print_the_store_key() {
        let crypto = MatrixCrypto {
            store_dir: "/store".into(),
            store_key: Some("s3cret".into()),
        };
        let shown = format!("{crypto:?}");
        assert!(
            shown.contains("/store") && !shown.contains("s3cret"),
            "{shown}"
        );
    }
}
