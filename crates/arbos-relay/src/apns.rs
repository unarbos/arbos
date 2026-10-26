//! Apple Push Notification service client: HTTP/2 with token-based (JWT ES256) authentication.
//!
//! Configuration (all three required, else push is off and the relay says so once at start):
//! `APNS_KEY_P8` (the `.p8` PEM text, or a path to it), `APNS_KEY_ID`, `APNS_TEAM_ID`;
//! `APNS_TOPIC` defaults to the iOS bundle id. The provider token is re-signed every 50 minutes
//! (Apple refuses tokens older than an hour and asks for no more than one per 20 minutes).

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use arbos_core::ids::now_ms;

pub const DEFAULT_TOPIC: &str = "com.unarbos.arbos.ios";
const TOKEN_LIFETIME_MS: i64 = 50 * 60 * 1000;

/// One notification as handed to the transport.
#[derive(Debug, Clone)]
pub struct Outgoing {
    pub device_token: String,
    /// `development` → sandbox host, anything else → production.
    pub environment: String,
    /// `alert`, `background`, or `voip` (PushKit).
    pub push_type: &'static str,
    /// Overrides the client's topic: VoIP pushes go to `<bundle_id>.voip`.
    pub topic: Option<String>,
    /// APNs `apns-collapse-id`; later notifications with the same id replace earlier ones.
    pub collapse_id: Option<String>,
    pub payload: Value,
}

/// What APNs said about one send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    Sent,
    /// The token is dead (`410 Unregistered`, `400 BadDeviceToken`/`DeviceTokenNotForTopic`): forget it.
    TokenGone,
    Failed(String),
}

/// The transport behind message notifications: real APNs, a capture list (tests), or nothing.
pub enum Pusher {
    Apns(Box<Apns>),
    /// Tests: every notification lands here instead of at Apple.
    #[allow(dead_code)]
    Capture(Mutex<Vec<Outgoing>>),
    Off,
}

impl Pusher {
    pub fn is_on(&self) -> bool {
        !matches!(self, Pusher::Off)
    }

    pub async fn send(&self, out: Outgoing) -> Delivery {
        match self {
            Pusher::Apns(a) => a.send(&out).await,
            Pusher::Capture(list) => {
                list.lock().await.push(out);
                Delivery::Sent
            }
            Pusher::Off => Delivery::Failed("push is off".into()),
        }
    }

    /// Everything sent so far (capture mode only; empty otherwise).
    #[allow(dead_code)]
    pub async fn captured(&self) -> Vec<Outgoing> {
        match self {
            Pusher::Capture(list) => list.lock().await.clone(),
            Pusher::Apns(_) | Pusher::Off => Vec::new(),
        }
    }
}

pub struct Apns {
    key: EcdsaKeyPair,
    key_id: String,
    team_id: String,
    pub topic: String,
    http: reqwest::Client,
    token: Mutex<Option<(i64, String)>>,
    rng: SystemRandom,
}

/// Build the client from the environment; `None` (with one log line) when the key is not set.
pub fn from_env() -> Option<Arc<Pusher>> {
    let p8 = std::env::var("APNS_KEY_P8")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let key_id = std::env::var("APNS_KEY_ID")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let team_id = std::env::var("APNS_TEAM_ID")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let (Some(p8), Some(key_id), Some(team_id)) = (p8, key_id, team_id) else {
        tracing::warn!(
            "APNS_KEY_P8/APNS_KEY_ID/APNS_TEAM_ID not set: iOS push notifications are off \
             (relay-server.sh reads them from the vault item \"Arbos APNs auth key\")"
        );
        return None;
    };
    let topic = std::env::var("APNS_TOPIC")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_TOPIC.to_owned());
    match Apns::new(&p8, key_id.trim(), team_id.trim(), &topic) {
        Ok(a) => {
            tracing::info!(key_id = %a.key_id, team_id = %a.team_id, topic = %a.topic, "APNs push on");
            Some(Arc::new(Pusher::Apns(Box::new(a))))
        }
        Err(e) => {
            tracing::error!(error = %e, "APNS_KEY_P8 is not a usable P-256 key; push is off");
            None
        }
    }
}

fn pkcs8_from_pem(text: &str) -> Result<Vec<u8>> {
    let body: String = text
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .map(str::trim)
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(body.as_bytes())
        .context("APNs key is not base64 PEM")
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

impl Apns {
    pub fn new(p8_or_path: &str, key_id: &str, team_id: &str, topic: &str) -> Result<Self> {
        let pem = if p8_or_path.contains("-----BEGIN") {
            p8_or_path.to_owned()
        } else {
            std::fs::read_to_string(p8_or_path.trim())
                .with_context(|| format!("reading APNS_KEY_P8 file {}", p8_or_path.trim()))?
        };
        let der = pkcs8_from_pem(&pem)?;
        let rng = SystemRandom::new();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng)
            .map_err(|e| anyhow!("APNs key is not a PKCS#8 P-256 key: {e}"))?;
        let http = reqwest::Client::builder()
            .http2_prior_knowledge()
            .pool_idle_timeout(Duration::from_secs(600))
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            key,
            key_id: key_id.to_owned(),
            team_id: team_id.to_owned(),
            topic: topic.to_owned(),
            http,
            token: Mutex::new(None),
            rng,
        })
    }

    fn sign_token(&self) -> Result<String> {
        let header = json!({ "alg": "ES256", "kid": self.key_id });
        let claims = json!({ "iss": self.team_id, "iat": now_ms() / 1000 });
        let signing_input = format!(
            "{}.{}",
            b64url(header.to_string().as_bytes()),
            b64url(claims.to_string().as_bytes())
        );
        let sig = self
            .key
            .sign(&self.rng, signing_input.as_bytes())
            .map_err(|_| anyhow!("signing the APNs provider token"))?;
        Ok(format!("{signing_input}.{}", b64url(sig.as_ref())))
    }

    async fn provider_token(&self) -> Result<String> {
        let mut slot = self.token.lock().await;
        let now = now_ms();
        if let Some((at, tok)) = slot.as_ref() {
            if now - at < TOKEN_LIFETIME_MS {
                return Ok(tok.clone());
            }
        }
        let tok = self.sign_token()?;
        *slot = Some((now, tok.clone()));
        Ok(tok)
    }

    fn host(environment: &str) -> &'static str {
        if environment == "development" {
            "https://api.sandbox.push.apple.com"
        } else {
            "https://api.push.apple.com"
        }
    }

    pub async fn send(&self, out: &Outgoing) -> Delivery {
        let token = match self.provider_token().await {
            Ok(t) => t,
            Err(e) => return Delivery::Failed(e.to_string()),
        };
        let url = format!(
            "{}/3/device/{}",
            Self::host(&out.environment),
            out.device_token
        );
        let mut req = self
            .http
            .post(&url)
            .bearer_auth(token)
            .header("apns-topic", out.topic.as_deref().unwrap_or(&self.topic))
            .header("apns-push-type", out.push_type)
            .header(
                "apns-priority",
                if out.push_type == "background" {
                    "5"
                } else {
                    "10"
                },
            )
            // VoIP must expire immediately (Apple): a late delivery after the
            // call ended makes CallKit report a ghost ring, and repeated misses
            // stop PushKit delivery entirely.
            .header(
                "apns-expiration",
                if out.push_type == "voip" {
                    "0".to_owned()
                } else {
                    ((now_ms() / 1000) + 24 * 3600).to_string()
                },
            )
            .json(&out.payload);
        if let Some(id) = &out.collapse_id {
            req = req.header("apns-collapse-id", id);
        }
        // One line per attempt, whatever Apple answers: the delivery audit trail
        // (`journalctl -u arbos-relay | grep apns`) names the device by token
        // prefix only — the full token is a credential for that one device.
        let token_prefix: String = out.device_token.chars().take(8).collect();
        let topic = out.topic.as_deref().unwrap_or(&self.topic).to_owned();
        let collapse_id = out.collapse_id.clone().unwrap_or_default();
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    token = %token_prefix, environment = %out.environment, topic = %topic,
                    push_type = out.push_type, collapse_id = %collapse_id, error = %e,
                    "apns send: transport error"
                );
                return Delivery::Failed(format!("apns transport: {e}"));
            }
        };
        let status = resp.status();
        let apns_id = resp
            .headers()
            .get("apns-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        if status.is_success() {
            tracing::info!(
                status = status.as_u16(), apns_id = %apns_id, token = %token_prefix,
                environment = %out.environment, topic = %topic, push_type = out.push_type,
                collapse_id = %collapse_id, "apns send: accepted"
            );
            return Delivery::Sent;
        }
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        let reason = body["reason"].as_str().unwrap_or("").to_owned();
        tracing::warn!(
            status = status.as_u16(), reason = %reason, apns_id = %apns_id, token = %token_prefix,
            environment = %out.environment, topic = %topic, push_type = out.push_type,
            collapse_id = %collapse_id, "apns send: rejected"
        );
        match (status.as_u16(), reason.as_str()) {
            (410, _) | (400, "BadDeviceToken") | (400, "DeviceTokenNotForTopic") => {
                Delivery::TokenGone
            }
            _ => Delivery::Failed(format!("apns {status}: {reason}")),
        }
    }
}

#[cfg(test)]
mod tests {
    //! A freshly generated P-256 key signs a well-formed three-part ES256 token.

    use super::*;

    #[test]
    fn provider_token_is_a_signed_jwt() {
        let rng = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).expect("key");
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            base64::engine::general_purpose::STANDARD.encode(pkcs8.as_ref())
        );
        let apns = Apns::new(&pem, "KEYID12345", "TEAMID1234", DEFAULT_TOPIC).expect("client");
        let tok = apns.sign_token().expect("token");
        let parts: Vec<&str> = tok.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(parts[0])
                .expect("b64"),
        )
        .expect("json");
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["kid"], "KEYID12345");
        // Fixed-length ECDSA signature: r || s, 64 bytes.
        assert_eq!(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(parts[2])
                .expect("sig")
                .len(),
            64
        );
    }
}
