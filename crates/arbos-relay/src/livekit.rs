//! LiveKit (self-hosted SFU) glue for calls: join tokens, webhook verification, and the room
//! admin API. Configuration: `LIVEKIT_URL` (`wss://livekit.example`), `LIVEKIT_API_KEY`,
//! `LIVEKIT_API_SECRET`, optional `LIVEKIT_WEBHOOK_SECRET` (when the webhook is signed with a
//! different key than the API key pair). Without the first three, calls answer 503.

use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use arbos_core::ids::now_ms;

type HmacSha256 = Hmac<Sha256>;

/// How long a join token is good for. A client that drops past this re-joins through the relay.
pub const TOKEN_TTL_S: i64 = 3600;
/// Admin tokens for the room API live seconds, not hours.
const ADMIN_TTL_S: i64 = 60;
pub const MAX_PARTICIPANTS: u32 = 16;

#[derive(Clone)]
pub struct LiveKit {
    /// What clients connect to (`wss://…`).
    pub url: String,
    /// Same host over HTTPS, for the Twirp room service.
    pub http_url: String,
    pub api_key: String,
    api_secret: String,
    webhook_secret: String,
}

impl std::fmt::Debug for LiveKit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveKit")
            .field("url", &self.url)
            .field("api_key", &self.api_key)
            .finish_non_exhaustive()
    }
}

fn env_nonempty(k: &str) -> Option<String> {
    std::env::var(k)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

pub fn from_env() -> Option<LiveKit> {
    let (Some(url), Some(api_key), Some(api_secret)) = (
        env_nonempty("LIVEKIT_URL"),
        env_nonempty("LIVEKIT_API_KEY"),
        env_nonempty("LIVEKIT_API_SECRET"),
    ) else {
        tracing::info!("LIVEKIT_URL/LIVEKIT_API_KEY/LIVEKIT_API_SECRET not set: calls are off");
        return None;
    };
    let lk = LiveKit::new(
        &url,
        &api_key,
        &api_secret,
        env_nonempty("LIVEKIT_WEBHOOK_SECRET"),
    );
    tracing::info!(url = %lk.url, api_key = %lk.api_key, "LiveKit calls on");
    Some(lk)
}

fn sign(secret: &str, data: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(data.as_bytes());
    B64URL.encode(mac.finalize().into_bytes())
}

fn jwt(secret: &str, claims: &Value) -> String {
    let header = B64URL.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = B64URL.encode(claims.to_string());
    let input = format!("{header}.{payload}");
    let sig = sign(secret, &input);
    format!("{input}.{sig}")
}

/// Decode and verify one of LiveKit's HS256 JWTs (webhook Authorization); returns the claims.
fn verify_jwt(secret: &str, token: &str) -> Option<Value> {
    let mut parts = token.trim().split('.');
    let (Some(h), Some(p), Some(s), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let expected = B64URL.decode(sign(secret, &format!("{h}.{p}"))).ok()?;
    let given = B64URL.decode(s).ok()?;
    if given.len() != expected.len()
        || given
            .iter()
            .zip(expected.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            != 0
    {
        return None;
    }
    let claims: Value = serde_json::from_slice(&B64URL.decode(p).ok()?).ok()?;
    if let Some(exp) = claims["exp"].as_i64() {
        if exp < now_ms() / 1000 {
            return None;
        }
    }
    Some(claims)
}

impl LiveKit {
    pub fn new(url: &str, api_key: &str, api_secret: &str, webhook_secret: Option<String>) -> Self {
        let url = url.trim_end_matches('/').to_owned();
        let http_url = if let Some(rest) = url.strip_prefix("wss://") {
            format!("https://{rest}")
        } else if let Some(rest) = url.strip_prefix("ws://") {
            format!("http://{rest}")
        } else {
            url.clone()
        };
        Self {
            url,
            http_url,
            api_key: api_key.to_owned(),
            api_secret: api_secret.to_owned(),
            webhook_secret: webhook_secret.unwrap_or_else(|| api_secret.to_owned()),
        }
    }

    /// A join token for `identity` in `room`: subscribe + data, and publish when `can_publish`.
    /// Returns `(token, expires_at_ms)`.
    pub fn join_token(
        &self,
        room: &str,
        identity: &str,
        name: &str,
        metadata: &Value,
        can_publish: bool,
    ) -> (String, i64) {
        let now = now_ms() / 1000;
        let exp = now + TOKEN_TTL_S;
        let claims = json!({
            "iss": self.api_key,
            "sub": identity,
            "iat": now,
            "nbf": now - 10,
            "exp": exp,
            "name": name,
            "metadata": metadata.to_string(),
            "video": {
                "roomJoin": true,
                "room": room,
                "canPublish": can_publish,
                "canSubscribe": true,
                "canPublishData": true,
            },
        });
        (jwt(&self.api_secret, &claims), exp * 1000)
    }

    /// Change whether a live participant may publish (host invites / revokes a speaker).
    pub async fn update_publish(
        &self,
        http: &reqwest::Client,
        room: &str,
        identity: &str,
        can_publish: bool,
    ) {
        let resp = http
            .post(format!(
                "{}/twirp/livekit.RoomService/UpdateParticipant",
                self.http_url
            ))
            .bearer_auth(self.admin_token(room))
            .json(&json!({
                "room": room,
                "identity": identity,
                "permission": {
                    "canSubscribe": true,
                    "canPublish": can_publish,
                    "canPublishData": true,
                },
            }))
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() || r.status().as_u16() == 404 => {}
            Ok(r) => tracing::warn!(
                room,
                identity,
                status = %r.status(),
                "LiveKit UpdateParticipant refused"
            ),
            Err(e) => {
                tracing::warn!(room, identity, error = %e, "LiveKit UpdateParticipant failed")
            }
        }
    }

    fn admin_token(&self, room: &str) -> String {
        let now = now_ms() / 1000;
        jwt(
            &self.api_secret,
            &json!({
                "iss": self.api_key, "sub": "arbos-relay", "iat": now, "nbf": now - 10, "exp": now + ADMIN_TTL_S,
                "video": { "roomCreate": true, "roomAdmin": true, "room": room, "agent": true },
            }),
        )
    }

    /// `POST /livekit/webhook`: LiveKit signs each delivery with a JWT whose `sha256` claim is
    /// the base64 SHA-256 of the body. Returns the event when the signature and hash match.
    pub fn verify_webhook(&self, authorization: &str, body: &[u8]) -> Option<Value> {
        let token = authorization
            .strip_prefix("Bearer ")
            .unwrap_or(authorization);
        let claims = verify_jwt(&self.webhook_secret, token)?;
        let digest = B64.encode(Sha256::digest(body));
        if claims["sha256"].as_str() != Some(digest.as_str()) {
            return None;
        }
        serde_json::from_slice(body).ok()
    }

    /// Ask LiveKit to send a registered agent worker (`agent_name`) into `room` with `metadata`
    /// (Calls Phase 2: the voice worker joins as `a_<project_id>`). Returns the dispatch id.
    pub async fn create_dispatch(
        &self,
        http: &reqwest::Client,
        room: &str,
        agent_name: &str,
        metadata: &Value,
    ) -> anyhow::Result<String> {
        let resp = http
            .post(format!(
                "{}/twirp/livekit.AgentDispatchService/CreateDispatch",
                self.http_url
            ))
            .bearer_auth(self.admin_token(room))
            .json(&json!({ "room": room, "agent_name": agent_name, "metadata": metadata.to_string() }))
            .send()
            .await?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            anyhow::bail!("LiveKit CreateDispatch: HTTP {status} {body}");
        }
        Ok(body["id"].as_str().unwrap_or_default().to_owned())
    }

    /// Close the LiveKit room (everyone still connected is disconnected). Best effort.
    pub async fn delete_room(&self, http: &reqwest::Client, room: &str) {
        let resp = http
            .post(format!(
                "{}/twirp/livekit.RoomService/DeleteRoom",
                self.http_url
            ))
            .bearer_auth(self.admin_token(room))
            .json(&json!({ "room": room }))
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() || r.status().as_u16() == 404 => {}
            Ok(r) => tracing::warn!(room, status = %r.status(), "LiveKit DeleteRoom refused"),
            Err(e) => tracing::warn!(room, error = %e, "LiveKit DeleteRoom failed"),
        }
    }
}

/// Decode a token's claims without verifying (tests).
#[cfg(test)]
pub fn claims_of(token: &str) -> Option<Value> {
    let p = token.split('.').nth(1)?;
    serde_json::from_slice(&B64URL.decode(p).ok()?).ok()
}

#[cfg(test)]
mod tests {
    //! Token claims are what LiveKit expects: issuer = API key, subject = identity, video grant
    //! for exactly this room with publish/subscribe, one-hour expiry; the webhook check accepts
    //! a correctly signed body and rejects a tampered one.

    use super::*;

    #[test]
    fn join_token_claims() {
        let lk = LiveKit::new("wss://lk.example", "APIkey", "secret123", None);
        assert_eq!(lk.http_url, "https://lk.example");
        let meta = json!({ "kind": "user", "id": "gh_1", "handle": "alice" });
        let (tok, exp_ms) = lk.join_token("call_01H", "u_gh_1", "Alice", &meta, true);
        let claims = verify_jwt("secret123", &tok).expect("verifies with the API secret");
        assert_eq!(claims["iss"], "APIkey");
        assert_eq!(claims["sub"], "u_gh_1");
        assert_eq!(claims["name"], "Alice");
        assert_eq!(claims["video"]["room"], "call_01H");
        assert_eq!(claims["video"]["roomJoin"], true);
        assert_eq!(claims["video"]["canPublish"], true);
        assert_eq!(claims["video"]["canSubscribe"], true);
        assert_eq!(claims["video"]["roomAdmin"], Value::Null);
        let meta_back: Value = serde_json::from_str(claims["metadata"].as_str().unwrap()).unwrap();
        assert_eq!(meta_back["handle"], "alice");
        let exp = claims["exp"].as_i64().unwrap();
        assert_eq!(exp * 1000, exp_ms);
        assert!((exp - now_ms() / 1000 - TOKEN_TTL_S).abs() <= 2);
        assert!(verify_jwt("wrong", &tok).is_none());
        let (listen, _) = lk.join_token("call_01H", "u_gh_2", "Bob", &meta, false);
        let listen_claims = verify_jwt("secret123", &listen).unwrap();
        assert_eq!(listen_claims["video"]["canPublish"], false);
    }

    #[test]
    fn webhook_signature_and_body_hash() {
        let lk = LiveKit::new("wss://lk.example", "APIkey", "secret123", None);
        let body = br#"{"event":"room_finished","room":{"name":"call_01H"}}"#;
        let auth = jwt(
            "secret123",
            &json!({ "iss": "APIkey", "sha256": B64.encode(Sha256::digest(body)), "exp": now_ms() / 1000 + 60 }),
        );
        let ev = lk.verify_webhook(&auth, body).expect("valid");
        assert_eq!(ev["event"], "room_finished");
        assert!(lk
            .verify_webhook(&auth, br#"{"event":"room_finished"}"#)
            .is_none());
        assert!(lk.verify_webhook("Bearer nope", body).is_none());
    }
}
