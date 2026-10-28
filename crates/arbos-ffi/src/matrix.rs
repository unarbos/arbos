//! Servers and channels for the Swift client (D88 C1): a thin UniFFI skin over
//! [`arbos_matrix::MatrixClient`]. Records mirror the core rows one to one so the core stays
//! free of UniFFI derives; the async calls run on the bindings' tokio runtime.

use std::sync::Arc;

use arbos_matrix::{
    ChannelRow as CoreChannel, Listener as CoreListener, MatrixClient as CoreClient,
    MessageRow as CoreMessage, Session as CoreSession, Snapshot as CoreSnapshot,
    SpaceRow as CoreSpace,
};

#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum MatrixError {
    #[error("{0}")]
    Session(String),
    #[error("{0}")]
    Homeserver(String),
    #[error("{0}")]
    NoRoom(String),
    #[error("stopped")]
    Stopped,
}

impl From<arbos_matrix::Error> for MatrixError {
    fn from(e: arbos_matrix::Error) -> Self {
        match e {
            arbos_matrix::Error::Session(s) => Self::Session(s),
            arbos_matrix::Error::Homeserver(s) => Self::Homeserver(s),
            arbos_matrix::Error::NoRoom(s) => Self::NoRoom(s),
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
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MatrixMessageRow {
    pub event_id: String,
    pub id: String,
    pub sender: String,
    pub sender_name: String,
    pub text: String,
    pub created_at: i64,
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

#[derive(Debug, Clone, uniffi::Record)]
pub struct MatrixSnapshot {
    pub spaces: Vec<MatrixSpaceRow>,
    pub channels: Vec<MatrixChannelRow>,
}

fn row(r: CoreMessage) -> MatrixMessageRow {
    MatrixMessageRow {
        event_id: r.event_id,
        id: r.id,
        sender: r.sender,
        sender_name: r.sender_name,
        text: r.text,
        created_at: r.created_at,
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

fn snapshot(s: CoreSnapshot) -> MatrixSnapshot {
    MatrixSnapshot {
        spaces: s.spaces.into_iter().map(space).collect(),
        channels: s.channels.into_iter().map(channel).collect(),
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

/// Start syncing for this device. Returns once the client exists; the first snapshot comes
/// through the listener.
#[uniffi::export(async_runtime = "tokio")]
pub async fn matrix_start(
    session: MatrixSession,
    listener: Arc<dyn MatrixListener>,
) -> Result<Arc<MatrixClient>, MatrixError> {
    let inner = CoreClient::start(
        CoreSession {
            homeserver: session.homeserver,
            user_id: session.user_id,
            access_token: session.access_token,
            device_id: session.device_id,
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

    pub async fn mark_read(&self, room_id: String) -> Result<(), MatrixError> {
        Ok(self.inner.mark_read(&room_id).await?)
    }
}
