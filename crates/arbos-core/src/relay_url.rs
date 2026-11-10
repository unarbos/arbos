//! The address a box uses for the relay, with the relay's proof that it named it.
//!
//! A box learns its relay address from a file the relay writes into the sandbox at every wake,
//! because the address baked in at create goes stale when the relay's public URL changes (quick
//! tunnels change on every restart). Reaching that address proves only that something is
//! listening, so the address travels with a detached Ed25519 signature and agentd verifies it
//! against a key pinned in its environment at create — offline, before it sends anything.
//!
//! This module is the one definition of what gets signed and of the file that carries it: the
//! relay signs, agentd verifies, and neither owns the format. Only the message builder and the
//! non-cryptographic checks live here; the signing key never leaves the relay and the verifying
//! key never leaves agentd.

use serde::{Deserialize, Serialize};

/// Domain separator. A signature over one of these messages cannot be replayed as a signature
/// over anything else the relay signs, and bumping the version invalidates every old one.
pub const DOMAIN: &str = "arbos-relay-url:v1";

/// Environment variable carrying the base64 Ed25519 **public** key a box verifies pushed
/// addresses with. The relay puts it in the create-time environment of the box's root process,
/// which is the one input into the box's choice of relay that nothing inside the box can rewrite.
pub const VERIFY_KEY_ENV: &str = "ARBOS_RELAY_VERIFY_KEY";

/// Name of the file carrying the signature, beside the plain-text `relay-url` file. The plain
/// file keeps its exact one-line contract so that a box still running an older agentd reads it
/// unchanged; this one is additive.
pub const SIG_FILE_SUFFIX: &str = ".sig";

/// A relay address the relay has signed for one box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedRelayUrl {
    pub url: String,
    /// The box this address was signed for: a signature taken from one box is not valid in another.
    pub box_id: String,
    /// Milliseconds since the epoch at signing. Only the order matters: of two valid signatures
    /// the higher counter is the newer address, which is what stops a rollback to an older one.
    pub counter: i64,
    pub expires_at_ms: i64,
    /// Base64 (standard alphabet, padded) Ed25519 signature over [`SignedRelayUrl::message`].
    pub sig: String,
}

/// The exact bytes signed and verified. Field order and separator are part of the format.
pub fn signing_message(url: &str, box_id: &str, counter: i64, expires_at_ms: i64) -> String {
    format!("{DOMAIN}|{box_id}|{counter}|{expires_at_ms}|{url}")
}

/// Why an otherwise well-formed signed address must not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// Signed for a different box: one box's address is not valid in another.
    WrongBox,
    /// Past its expiry.
    Expired,
    /// Not a URL this box may talk to at all.
    NotHttp,
}

impl Reject {
    pub fn as_str(self) -> &'static str {
        match self {
            Reject::WrongBox => "wrong_box",
            Reject::Expired => "expired",
            Reject::NotHttp => "not_http",
        }
    }
}

impl SignedRelayUrl {
    pub fn message(&self) -> String {
        signing_message(&self.url, &self.box_id, self.counter, self.expires_at_ms)
    }

    /// The URL without its trailing slash, as every caller wants it.
    pub fn trimmed_url(&self) -> &str {
        self.url.trim().trim_end_matches('/')
    }

    /// Everything that can be decided without the key: that this signature is for this box, that
    /// it has not expired, and that the address is one we would talk to. The signature itself is
    /// checked by the caller, which is the only side holding the key.
    pub fn check(&self, box_id: &str, now_ms: i64) -> Result<(), Reject> {
        if self.box_id != box_id {
            return Err(Reject::WrongBox);
        }
        if now_ms > self.expires_at_ms {
            return Err(Reject::Expired);
        }
        let u = self.trimmed_url();
        if !(u.starts_with("http://") || u.starts_with("https://")) {
            return Err(Reject::NotHttp);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed() -> SignedRelayUrl {
        SignedRelayUrl {
            url: "https://relay.example".into(),
            box_id: "user:alice".into(),
            counter: 10,
            expires_at_ms: 1_000,
            sig: "AAAA".into(),
        }
    }

    #[test]
    fn the_signed_message_binds_every_field_it_needs_to() {
        let m = signed().message();
        assert!(m.starts_with("arbos-relay-url:v1|"));
        assert_eq!(
            m,
            "arbos-relay-url:v1|user:alice|10|1000|https://relay.example"
        );
        // Changing any field changes the bytes, so a signature cannot be carried across.
        for other in [
            signing_message("https://evil.example", "user:alice", 10, 1_000),
            signing_message("https://relay.example", "user:bob", 10, 1_000),
            signing_message("https://relay.example", "user:alice", 11, 1_000),
            signing_message("https://relay.example", "user:alice", 10, 1_001),
        ] {
            assert_ne!(m, other);
        }
    }

    #[test]
    fn a_signature_for_another_box_is_not_valid_here() {
        assert_eq!(
            signed().check("user:bob", 0),
            Err(Reject::WrongBox),
            "a genuine signature lifted from another box must not be usable in this one"
        );
    }

    #[test]
    fn expiry_is_inclusive_of_the_last_valid_millisecond() {
        assert_eq!(signed().check("user:alice", 1_000), Ok(()));
        assert_eq!(signed().check("user:alice", 1_001), Err(Reject::Expired));
    }

    #[test]
    fn an_address_that_is_not_http_is_refused() {
        let mut s = signed();
        s.url = "file:///etc/passwd".into();
        assert_eq!(s.check("user:alice", 0), Err(Reject::NotHttp));
    }

    #[test]
    fn the_trailing_slash_never_reaches_a_caller() {
        let mut s = signed();
        s.url = "https://relay.example/".into();
        assert_eq!(s.trimmed_url(), "https://relay.example");
        assert_eq!(s.check("user:alice", 0), Ok(()));
    }

    #[test]
    fn the_file_round_trips_through_json() {
        let s = signed();
        let back: SignedRelayUrl =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
    }
}
