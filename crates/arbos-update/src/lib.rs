//! Arbos updates — the one description of a published build, shared by the
//! step that publishes it and the app that installs it.
//!
//! The two sides of an update have to agree on three things: what "newer"
//! means, what a channel publishes, and what makes a payload trustworthy.
//! They agree by sharing these types rather than by both being written to
//! produce the same JSON, which is the failure this crate exists to prevent.
//!
//! - [`version`] — the marketing version and the build number, ordered.
//! - [`feed`] — the document a channel publishes and the app reads.
//! - [`sign`] — Ed25519 over the payload's bytes, and the key that checks it.
//! - [`install`] — putting the new build in place, and putting the old one
//!   back when that goes wrong.
//!
//! Nothing here opens a socket. The desktop fetches with the HTTP client it
//! already has; this crate only says what the bytes mean and where they go.

pub mod feed;
pub mod install;
pub mod sign;
pub mod version;

pub use feed::{Available, Channel, Download, Feed, Format, Platform, Release};
pub use sign::{PublicKey, SecretKey};
pub use version::Version;
