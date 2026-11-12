//! Pay a person, not an address: the opt-in link between a user and their TAO address.
//!
//! The wallet itself lives only on the user's devices (`arbos-wallet`); the relay never sees a
//! key or a phrase. A user who chooses "people can pay me by @handle" sends
//! `PUT /me/wallet {address, proof}`, where `proof` is an sr25519 signature by that address over
//! [`link_message`] — so nobody can point someone else's handle at an address they don't hold,
//! or claim an address that isn't theirs. Linking is idempotent, so a replayed proof can only
//! re-link the same address to the same user. `GET /users/{id}/wallet` answers `{address}` only
//! to someone in a direct chat with that user, neither blocking the other; `DELETE /me/wallet`
//! unlinks (the address stays on chain, the handle stops pointing at it).
//!
//! The relay is not the authority on who gets paid: a client shows the address it was given and
//! pins it per person, so a relay that swaps one is noticed.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use blake2::{Blake2b512, Digest};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::json;

use crate::auth::Principal;
use crate::signin::api_error;
use crate::state::AppState;

pub const WALLETS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS user_wallets (
  user_id TEXT PRIMARY KEY, address TEXT NOT NULL, linked_at INTEGER NOT NULL
);
"#;

/// Must match `arbos_wallet::link_message` byte for byte.
pub fn link_message(user_id: &str, address: &str) -> String {
    format!("arbos/wallet-link/v1\nuser:{user_id}\naddress:{address}")
}

/// A Bittensor (SS58 prefix 42) address → its 32-byte public key, checksum verified.
pub fn public_key(address: &str) -> Option<[u8; 32]> {
    if address.len() > 64 {
        return None;
    }
    let raw = bs58::decode(address).into_vec().ok()?;
    if raw.len() != 35 || raw[0] != 42 {
        return None;
    }
    let sum = Blake2b512::new()
        .chain_update(b"SS58PRE")
        .chain_update(&raw[..33])
        .finalize();
    (sum[..2] == raw[33..]).then(|| raw[1..33].try_into().expect("32 bytes"))
}

/// Does `proof` (hex sr25519, Substrate's `substrate` context) sign `user_id`'s link to `address`?
pub fn verify(user_id: &str, address: &str, proof: &str) -> bool {
    let (Some(pk), Ok(sig)) = (public_key(address), hex::decode(proof)) else {
        return false;
    };
    let (Ok(pk), Ok(sig)) = (
        schnorrkel::PublicKey::from_bytes(&pk),
        schnorrkel::Signature::from_bytes(&sig),
    ) else {
        return false;
    };
    pk.verify_simple(
        b"substrate",
        link_message(user_id, address).as_bytes(),
        &sig,
    )
    .is_ok()
}

impl crate::db::Db {
    pub fn wallet(&self, user_id: &str) -> anyhow::Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT address FROM user_wallets WHERE user_id = ?1",
                params![user_id],
                |r| r.get(0),
            )
            .optional()
        })
    }

    pub fn set_wallet(&self, user_id: &str, address: Option<&str>) -> anyhow::Result<()> {
        self.with(|c| {
            match address {
                Some(a) => c.execute(
                    "INSERT INTO user_wallets(user_id, address, linked_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(user_id) DO UPDATE SET address = excluded.address,
                       linked_at = excluded.linked_at
                     WHERE user_wallets.address != excluded.address",
                    params![user_id, a, chrono::Utc::now().timestamp_millis()],
                )?,
                None => c.execute(
                    "DELETE FROM user_wallets WHERE user_id = ?1",
                    params![user_id],
                )?,
            };
            Ok(())
        })
    }
}

fn user_of(principal: &Principal) -> Result<String, Box<Response>> {
    match principal {
        Principal::Device { user_id, .. } => Ok(user_id.clone()),
        Principal::Box { .. } | Principal::Admin => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        ))),
    }
}

/// The detail goes to the log, never to the client.
fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!(error = %e, "wallet link storage");
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "db",
        "something went wrong",
    )
}

/// Address lookups one user may make a minute: a person paying people, not a crawler.
const LOOKUPS_PER_MIN: i64 = 30;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutWalletBody {
    pub address: String,
    pub proof: String,
}

/// `PUT /me/wallet {address, proof}` → `{address}`. A proof that does not verify → 400
/// `bad_proof`, nothing changes.
pub async fn put_wallet(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<PutWalletBody>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    if b.proof.len() != 128 || !verify(&me, &b.address, &b.proof) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_proof",
            "that address did not sign this link",
        );
    }
    match state.db.set_wallet(&me, Some(&b.address)) {
        Ok(()) => Json(json!({ "address": b.address })).into_response(),
        Err(e) => internal(e),
    }
}

/// `DELETE /me/wallet` → 204.
pub async fn delete_wallet(State(state): State<AppState>, principal: Principal) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    match state.db.set_wallet(&me, None) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal(e),
    }
}

/// `GET /users/{id}/wallet` → `{address}` for someone the caller has a direct chat with. One
/// 404 covers everything else — no such user, no chat, a block either way, nothing linked — so
/// the answer says nothing about which.
pub async fn user_wallet(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    match state.db.rate_event(&me, "wallet_lookup", 60_000) {
        Ok(n) if n > LOOKUPS_PER_MIN => {
            return api_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "slow down");
        }
        Ok(_) => {}
        Err(e) => return internal(e),
    }
    let address = (|| -> anyhow::Result<Option<String>> {
        if state.db.dm_between(&me, &id)?.is_none() || state.db.blocked_either_way(&me, &id)? {
            return Ok(None);
        }
        state.db.wallet(&id)
    })();
    match address {
        Ok(Some(address)) => Json(json!({ "address": address })).into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "not_found", "no linked wallet"),
        Err(e) => internal(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Substrate's development phrase, root sr25519 key.
    const DEV: &str = "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV";

    #[test]
    fn addresses_decode_only_for_bittensor_with_a_good_checksum() {
        assert!(public_key(DEV).is_some());
        // The same key on Polkadot (prefix 0).
        assert!(public_key("15oF4uVJwmo4TdGW7VfQxNLavjCXviqxT9S1MgbjMNHr6Sp5").is_none());
        assert!(public_key(&DEV.replace('D', "E")).is_none());
        assert!(public_key("").is_none());
        assert!(public_key(&"5".repeat(500)).is_none());
    }

    #[test]
    fn a_proof_binds_its_user_and_address() {
        // Signed by the dev phrase's root key for `gh_1` (`arbos_wallet::Seed::link_proof`);
        // sr25519 signatures are randomised, so this is one valid signature of many.
        let kp = schnorrkel::MiniSecretKey::from_bytes(
            &hex::decode("fac7959dbfe72f052e5a0c3c8d6530f202b02fd8f9f5ca3580ec8deb7797479e")
                .unwrap(),
        )
        .unwrap()
        .expand_to_keypair(schnorrkel::ExpansionMode::Ed25519);
        assert_eq!(
            kp.public.to_bytes(),
            public_key(DEV).unwrap(),
            "the dev root key"
        );
        let sign = |u: &str, a: &str| {
            hex::encode(
                kp.sign_simple(b"substrate", link_message(u, a).as_bytes())
                    .to_bytes(),
            )
        };
        let proof = sign("gh_1", DEV);
        assert!(verify("gh_1", DEV, &proof));
        assert!(!verify("gh_2", DEV, &proof), "another user");
        assert!(
            !verify("gh_1", DEV, &sign("gh_1", "5anything")),
            "another address"
        );
        assert!(!verify("gh_1", DEV, "00"), "junk");
    }

    #[tokio::test]
    async fn an_address_is_only_told_to_a_direct_chat_partner() {
        use crate::people::tests_support::{dm, person};
        let state = crate::state::test_support::test_state(crate::state::AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        state.db.set_wallet(&alice, Some(DEV)).unwrap();
        let ask = || async {
            user_wallet(State(state.clone()), bob_dev.clone(), Path(alice.clone()))
                .await
                .status()
        };
        assert_eq!(ask().await, StatusCode::NOT_FOUND, "strangers");
        dm(&state, &alice, &bob);
        assert_eq!(ask().await, StatusCode::OK);
        state.db.block(&alice, &bob).unwrap();
        assert_eq!(ask().await, StatusCode::NOT_FOUND, "blocked");
        state.db.unblock(&alice, &bob).unwrap();
        for _ in 3..LOOKUPS_PER_MIN {
            assert_eq!(ask().await, StatusCode::OK);
        }
        assert_eq!(ask().await, StatusCode::TOO_MANY_REQUESTS);
    }
}
