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
//! Stage D: rooms with `m.room.encryption` are end-to-end encrypted (Megolm, keys never leave
//! the devices). Each device keeps its keys and room state in SQLite under the app's
//! [`Crypto::store_dir`]; [`MatrixClient::recover`] backs the room keys up to the homeserver,
//! encrypted under the person's recovery passphrase, so another device of theirs reads the same
//! history. A message this
//! device cannot decrypt shows as a locked row ([`rows::LOCKED`]) until its key arrives.

pub mod client;
pub mod rows;

pub use client::{Backup, Crypto, Listener, MatrixClient, Session, Snapshot};
pub use rows::{ChannelRow, MessageRow, SpaceRow};

/// Anything that can go wrong talking to the homeserver, flattened for the apps and the FFI.
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("bad session: {0}")]
    Session(String),
    #[error("homeserver: {0}")]
    Homeserver(String),
    #[error("no such room: {0}")]
    NoRoom(String),
    /// Key backup: the passphrase did not open it, or the homeserver refused to store it.
    #[error("keys: {0}")]
    Keys(String),
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
