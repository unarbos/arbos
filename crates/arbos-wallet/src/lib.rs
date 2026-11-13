//! Every user's wallet, without the user having to know it is one.
//!
//! One 12-word phrase (BIP39) made on the device at first sign-in is the root of everything a
//! person owns that the relay must never hold:
//!
//! - a **Bittensor account** (sr25519 from the phrase, SS58 prefix 42) — the same address
//!   `btcli wallet regen_coldkey` or any Substrate wallet derives from those words, so the
//!   money is never locked into Arbos;
//! - the **recovery passphrase** for end-to-end encrypted chats (Matrix secret storage and key
//!   backup): a new device that has the phrase reads the old history, one that does not cannot;
//! - the **link proof** that ties the account to the person's Arbos id, so others can pay a
//!   handle instead of an address.
//!
//! Each derivation is domain-separated, so no output reveals another (or the phrase). Offline
//! cheques and chain access live in [`chain`]. Like `arbos-matrix`, only device apps
//! (`arbos-mac`, `arbos-ffi`) may depend on this crate; the relay only verifies link proofs.

pub mod chain;

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subxt::utils::AccountId32;
use subxt_signer::bip39::Mnemonic;
use subxt_signer::sr25519::Keypair;

/// Planck units per TAO.
pub const RAO_PER_TAO: u128 = 1_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("not a valid recovery phrase")]
    Phrase,
    #[error("{0}")]
    Address(&'static str),
    #[error("not an amount of TAO")]
    Amount,
    #[error("cheque: {0}")]
    Cheque(String),
    #[error("chain: {0}")]
    Chain(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The phrase. Never logged, never sent; `Debug` prints nothing of it.
#[derive(Clone)]
pub struct Seed(Mnemonic);

impl std::fmt::Debug for Seed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Seed(..)")
    }
}

impl Seed {
    /// A fresh 12-word phrase from the OS random source.
    pub fn generate() -> Self {
        let mut entropy = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut entropy);
        Self(Mnemonic::from_entropy(&entropy).expect("16 bytes is 12 words"))
    }

    /// Whatever the user typed or pasted: case, extra spaces and newlines don't matter; the
    /// BIP39 checksum does.
    pub fn parse(phrase: &str) -> Result<Self> {
        let words = phrase
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        Mnemonic::parse(&words).map(Self).map_err(|_| Error::Phrase)
    }

    pub fn phrase(&self) -> String {
        self.0.to_string()
    }

    pub(crate) fn keypair(&self) -> Keypair {
        Keypair::from_phrase(&self.0, None).expect("a parsed mnemonic always derives")
    }

    /// The Bittensor account (coldkey) address.
    pub fn address(&self) -> String {
        ss58(&self.keypair().public_key().0)
    }

    /// Passphrase for the chat key backup. HMAC over the BIP39 seed under its own label, so it
    /// is unrelated to the account key and to the phrase itself.
    pub fn recovery_passphrase(&self) -> String {
        hex::encode(self.derive(b"arbos/matrix-recovery/v1"))
    }

    /// Proof that `user_id` holds this account, for `PUT /me/wallet`. The signed message names
    /// the purpose, version, user and address ([`link_message`]); a replay can only re-link the
    /// same address to the same user, which the relay already treats as a no-op.
    pub fn link_proof(&self, user_id: &str) -> String {
        let msg = link_message(user_id, &self.address());
        hex::encode(self.keypair().sign(msg.as_bytes()).0)
    }

    fn derive(&self, label: &[u8]) -> [u8; 32] {
        let mut mac = Hmac::<Sha256>::new_from_slice(label).expect("any key length");
        mac.update(&self.0.to_seed_normalized(""));
        mac.finalize().into_bytes().into()
    }
}

/// The exact bytes a link proof signs (sr25519, Substrate's `substrate` signing context).
pub fn link_message(user_id: &str, address: &str) -> String {
    format!("arbos/wallet-link/v1\nuser:{user_id}\naddress:{address}")
}

// ---- addresses --------------------------------------------------------------------------------

/// SS58 with Bittensor's (generic Substrate) prefix 42.
pub fn ss58(account: &[u8; 32]) -> String {
    AccountId32(*account).to_string()
}

/// A Bittensor address, checksum verified. Other Substrate networks are refused so money cannot
/// go to the wrong chain: the address must be exactly what prefix 42 encodes.
pub fn parse_address(s: &str) -> Result<[u8; 32]> {
    let s = s.trim();
    let acc: AccountId32 = s
        .parse()
        .map_err(|_| Error::Address("not a valid address"))?;
    if acc.to_string() != s {
        return Err(Error::Address("not a Bittensor address"));
    }
    Ok(acc.0)
}

// ---- amounts ----------------------------------------------------------------------------------

/// "1.5" / "0.000000001" / "2 τ" → rao. Strict: at most 9 decimals, digits only, overflow
/// checked, never zero.
pub fn parse_tao(s: &str) -> Result<u128> {
    let s = s.trim().trim_end_matches(['τ', ' ']);
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    if (whole.is_empty() && frac.is_empty())
        || frac.len() > 9
        || !whole
            .chars()
            .chain(frac.chars())
            .all(|c| c.is_ascii_digit())
    {
        return Err(Error::Amount);
    }
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole.parse().map_err(|_| Error::Amount)?
    };
    let frac: u128 = format!("{frac:0<9}").parse().map_err(|_| Error::Amount)?;
    whole
        .checked_mul(RAO_PER_TAO)
        .and_then(|w| w.checked_add(frac))
        .filter(|&r| r > 0)
        .ok_or(Error::Amount)
}

/// rao → "1.5 τ" (trailing zeros trimmed).
pub fn format_tao(rao: u128) -> String {
    let (w, f) = (rao / RAO_PER_TAO, rao % RAO_PER_TAO);
    if f == 0 {
        return format!("{w} τ");
    }
    format!("{w}.{} τ", format!("{f:09}").trim_end_matches('0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Substrate's well-known development phrase.
    const DEV: &str = "bottom drive obey lake curtain smoke basket hold race lonely fit walk";

    #[test]
    fn the_phrase_derives_the_address_every_substrate_wallet_does() {
        let s = Seed::parse(DEV).unwrap();
        assert_eq!(
            s.address(),
            "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV"
        );
        // Typing slop is forgiven, a bad checksum is not.
        let sloppy = format!("  {}\n", DEV.to_uppercase().replace(' ', "   "));
        assert_eq!(Seed::parse(&sloppy).unwrap().address(), s.address());
        assert_eq!(
            Seed::parse(&DEV.replace("walk", "fit")).unwrap_err(),
            Error::Phrase
        );
        assert_eq!(format!("{s:?}"), "Seed(..)");
    }

    #[test]
    fn derivations_are_stable_and_separated() {
        let s = Seed::parse(DEV).unwrap();
        let r = s.recovery_passphrase();
        assert_eq!(r, Seed::parse(DEV).unwrap().recovery_passphrase());
        assert_eq!(r.len(), 64);
        assert!(!r.contains(&s.phrase()));
        assert_ne!(r, Seed::generate().recovery_passphrase());
        assert_eq!(Seed::generate().phrase().split(' ').count(), 12);
    }

    #[test]
    fn link_proofs_verify_only_for_their_user_and_address() {
        let s = Seed::parse(DEV).unwrap();
        let sig: [u8; 64] = hex::decode(s.link_proof("gh_1"))
            .unwrap()
            .try_into()
            .unwrap();
        let pk = subxt_signer::sr25519::PublicKey(parse_address(&s.address()).unwrap());
        let sig = subxt_signer::sr25519::Signature(sig);
        let ok = |m: String| subxt_signer::sr25519::verify(&sig, m, &pk);
        assert!(ok(link_message("gh_1", &s.address())));
        assert!(!ok(link_message("gh_2", &s.address())));
        assert!(!ok(link_message("gh_1", &Seed::generate().address())));
    }

    #[test]
    fn addresses_round_trip_and_refuse_other_chains() {
        let a = Seed::parse(DEV).unwrap().address();
        assert_eq!(ss58(&parse_address(&a).unwrap()), a);
        // Polkadot (prefix 0) encoding of the same key.
        assert!(parse_address("15oF4uVJwmo4TdGW7VfQxNLavjCXviqxT9S1MgbjMNHr6Sp5").is_err());
        let mut bad = a.clone().into_bytes();
        bad[10] = if bad[10] == b'a' { b'b' } else { b'a' };
        assert!(parse_address(std::str::from_utf8(&bad).unwrap()).is_err());
    }

    #[test]
    fn amounts_are_strict() {
        assert_eq!(parse_tao("1.5"), Ok(1_500_000_000));
        assert_eq!(parse_tao(".000000001 τ"), Ok(1));
        assert_eq!(parse_tao("2"), Ok(2 * RAO_PER_TAO));
        for bad in ["", "0", "-1", "1e9", "0.0000000001", "1,5", "τ"] {
            assert_eq!(parse_tao(bad), Err(Error::Amount), "{bad:?}");
        }
        assert_eq!(format_tao(1_500_000_000), "1.5 τ");
        assert_eq!(format_tao(3 * RAO_PER_TAO), "3 τ");
        assert_eq!(format_tao(1), "0.000000001 τ");
    }
}
