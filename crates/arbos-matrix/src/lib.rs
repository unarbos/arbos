//! Shared Matrix client for the device apps (D88 C1).
//!
//! Servers and channels exist only as rooms on the homeserver (a server is a Space, a channel a
//! room inside it; the relay is the control plane: `GET /servers`, roles, invite links,
//! `GET /me/matrix` for this device's token). This crate is the data plane the apps share: one
//! `matrix-sdk` client per signed-in device, sliding sync through the relay's
//! `/_matrix/client/*` proxy, channel timelines as the same message rows the apps already
//! render, sending, read receipts. The Mac uses it directly on its tokio runtime; the iPhone
//! through `arbos-ffi` (UniFFI). No app talks Matrix on its own. Users never see the word.
//!
//! Nothing here is encrypted (stage D) and nothing is persisted between launches: the memory
//! store plus sliding sync rebuilds the (small) room list in one round trip.

pub mod client;
pub mod rows;

pub use client::{Listener, MatrixClient, Session, Snapshot};
pub use rows::{ChannelRow, MessageRow, SpaceRow, StoredReaction};

/// Anything that can go wrong talking to the homeserver, flattened for the apps and the FFI.
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("bad session: {0}")]
    Session(String),
    #[error("homeserver: {0}")]
    Homeserver(String),
    #[error("no such room: {0}")]
    NoRoom(String),
    #[error("stopped")]
    Stopped,
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<matrix_sdk::Error> for Error {
    fn from(e: matrix_sdk::Error) -> Self {
        Error::Homeserver(e.to_string())
    }
}

impl From<matrix_sdk::HttpError> for Error {
    fn from(e: matrix_sdk::HttpError) -> Self {
        Error::Homeserver(e.to_string())
    }
}
