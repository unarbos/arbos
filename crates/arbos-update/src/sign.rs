//! The signature a payload has to carry, and the key that checks it.
//!
//! Ed25519 over the bytes of the file itself — the same promise Sparkle's
//! appcast signing makes, and for the same reason: it is ours to make. Apple's
//! notarization says a downloaded app has been seen by Apple and found clean,
//! which is a different sentence about a different moment, and it is still
//! waiting on a Developer ID certificate. This says the running app fetched
//! the bytes the build published and nothing in between changed them. The two
//! stack: when the certificate arrives the payload inside gains a notarized
//! signature and keeps this one, and nothing in this file changes.
//!
//! The private half exists in exactly one place, the repository secret
//! `ARBOS_UPDATE_SIGNING_KEY`. The public half is `update-key.pub` beside this
//! crate, compiled into the app. A build with no key in it refuses every
//! update rather than taking an unsigned one.

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
    signature::{self, Ed25519KeyPair, KeyPair, UnparsedPublicKey},
};

/// The key file's one meaningful line begins with this. Written out rather
/// than implied so a second algorithm could be added later without a file
/// full of bare base64 becoming ambiguous.
const ALGORITHM: &str = "ed25519";

/// An Ed25519 seed: 32 bytes, and 32 bytes is also what the public half is.
const KEY_LEN: usize = 32;

/// The public half, as it is compiled into the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey([u8; KEY_LEN]);

/// The private half. Only CI and whoever ran `keygen` ever holds one.
pub struct SecretKey([u8; KEY_LEN]);

impl PublicKey {
    pub fn from_base64(text: &str) -> Result<Self> {
        Ok(Self(fixed(text).context("public key")?))
    }

    pub fn to_base64(&self) -> String {
        B64.encode(self.0)
    }

    /// The line this key is written as in `update-key.pub`.
    pub fn to_line(&self) -> String {
        format!("{ALGORITHM} {}", self.to_base64())
    }

    /// Whether `signature` — base64, as the feed carries it — is this key's
    /// signature over `payload`.
    pub fn verify(&self, payload: &[u8], signature: &str) -> Result<()> {
        let raw = B64
            .decode(signature.trim())
            .context("signature is not base64")?;
        UnparsedPublicKey::new(&signature::ED25519, self.0)
            .verify(payload, &raw)
            .map_err(|_| anyhow!("signature does not match the update signing key"))
    }
}

impl SecretKey {
    pub fn from_base64(text: &str) -> Result<Self> {
        Ok(Self(fixed(text).context("signing key")?))
    }

    pub fn to_base64(&self) -> String {
        B64.encode(self.0)
    }

    pub fn public(&self) -> Result<PublicKey> {
        let pair = self.pair()?;
        Ok(PublicKey(
            fixed_bytes(pair.public_key().as_ref()).context("public half of the signing key")?,
        ))
    }

    /// Sign `payload`, base64, as the feed carries it.
    pub fn sign(&self, payload: &[u8]) -> Result<String> {
        Ok(B64.encode(self.pair()?.sign(payload).as_ref()))
    }

    fn pair(&self) -> Result<Ed25519KeyPair> {
        Ed25519KeyPair::from_seed_unchecked(&self.0)
            .map_err(|e| anyhow!("signing key is not a usable Ed25519 seed: {e}"))
    }
}

/// A fresh pair from the system's randomness. Run once, by a person, on the
/// machine that will hold the secret.
pub fn generate() -> Result<(SecretKey, PublicKey)> {
    let mut seed = [0u8; KEY_LEN];
    SystemRandom::new()
        .fill(&mut seed)
        .map_err(|_| anyhow!("no randomness from the system"))?;
    let secret = SecretKey(seed);
    let public = secret.public()?;
    Ok((secret, public))
}

/// The public key compiled into this build, or nothing where the repository
/// has not been given one yet.
///
/// Nothing is the honest answer and the safe one: every caller treats it as
/// "this build cannot verify an update", which is a message, not a silent
/// acceptance.
pub fn built_in_key() -> Option<PublicKey> {
    parse_key_file(include_str!("../update-key.pub"))
}

/// The one line of a `.pub` file that is not a comment.
pub fn parse_key_file(text: &str) -> Option<PublicKey> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .find_map(|line| {
            let rest = line.strip_prefix(ALGORITHM)?;
            PublicKey::from_base64(rest.trim()).ok()
        })
}

/// Lowercase hex SHA-256 of `payload` — the second thing the feed carries
/// about a download.
///
/// The signature is what makes a payload trustworthy; this is what makes a
/// mismatch legible. A truncated download and a tampered one both fail the
/// signature, and only the digest says which in a way a person can check by
/// hand with `shasum -a 256`.
pub fn sha256_hex(payload: &[u8]) -> String {
    digest::digest(&digest::SHA256, payload)
        .as_ref()
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write;
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn fixed(text: &str) -> Result<[u8; KEY_LEN]> {
    let raw = B64.decode(text.trim()).context("is not base64")?;
    fixed_bytes(&raw)
}

fn fixed_bytes(raw: &[u8]) -> Result<[u8; KEY_LEN]> {
    if raw.len() != KEY_LEN {
        bail!("is {} bytes, not {KEY_LEN}", raw.len());
    }
    let mut key = [0u8; KEY_LEN];
    key.copy_from_slice(raw);
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::{PublicKey, SecretKey, generate, parse_key_file, sha256_hex};

    #[test]
    fn a_signature_over_the_payload_verifies() {
        let (secret, public) = generate().unwrap();
        let payload = b"Arbos.app, zipped";
        let signature = secret.sign(payload).unwrap();
        public.verify(payload, &signature).unwrap();
    }

    #[test]
    fn one_changed_byte_fails() {
        let (secret, public) = generate().unwrap();
        let signature = secret.sign(b"Arbos.app, zipped").unwrap();
        assert!(public.verify(b"Arbos.app, zipped!", &signature).is_err());
        assert!(public.verify(b"Arbos.app, zippee", &signature).is_err());
        assert!(public.verify(b"", &signature).is_err());
    }

    #[test]
    fn another_key_cannot_sign_for_this_one() {
        let (_, public) = generate().unwrap();
        let (other, _) = generate().unwrap();
        let payload = b"a payload somebody else built";
        assert!(
            public
                .verify(payload, &other.sign(payload).unwrap())
                .is_err()
        );
    }

    #[test]
    fn a_mangled_signature_is_an_error_and_not_a_panic() {
        let (secret, public) = generate().unwrap();
        let payload = b"Arbos.app, zipped";
        let good = secret.sign(payload).unwrap();
        for bad in ["", "not base64!!", "AAAA", &good[..good.len() - 4]] {
            assert!(public.verify(payload, bad).is_err(), "`{bad}` verified");
        }
    }

    #[test]
    fn keys_survive_the_round_trip_through_a_secret_and_a_file() {
        // CI holds the secret as base64 in an environment variable and the
        // public half as a line in a committed file. Both have to come back.
        let (secret, public) = generate().unwrap();
        let secret = SecretKey::from_base64(&secret.to_base64()).unwrap();
        let file = format!("# a comment\n\n{}\n", public.to_line());
        let read = parse_key_file(&file).unwrap();
        assert_eq!(read, public);
        assert_eq!(PublicKey::from_base64(&public.to_base64()).unwrap(), public);
        read.verify(b"payload", &secret.sign(b"payload").unwrap())
            .unwrap();
    }

    #[test]
    fn a_key_file_with_no_key_in_it_reads_as_no_key() {
        // What ships before anybody has run `keygen`. The app must see
        // "cannot verify", never "nothing to check".
        assert!(parse_key_file("# only comments\n\n   \n").is_none());
        assert!(parse_key_file("ed25519 not-base64").is_none());
        assert!(parse_key_file("rsa AAAA").is_none());
    }

    #[test]
    fn the_digest_is_the_one_shasum_prints() {
        // `printf '' | shasum -a 256`
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // `printf 'abc' | shasum -a 256`
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
