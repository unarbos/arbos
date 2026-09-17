//! Push to a phone that is asleep. The attach protocol's `notify` frame
//! reaches a client only while its socket lives; iOS suspends the app
//! about thirty seconds after a switch-away (mobile loop, `2026-09-16-
//! mobile-push-needs-apns.md`). The hub is the one internet-facing
//! process that hears every kernel's notifications (they ride the
//! registration socket as `HubFrame::Notify`), so it is the sender.
//!
//! A phone registers its device token through its attach socket:
//! `{"type":"push","platform":"apns","token":"<hex>","project":
//! "<machine>/<project>","sandbox":false}` — the hub keeps `token →
//! projects` in `hub-push.json` beside the config (the newest token for a
//! device wins; a 410 from Apple drops one). On a kernel's `Notify` the
//! hub POSTs an alert to `https://api.push.apple.com/3/device/<token>`
//! with an ES256 JWT from the `[push]` key; on `Seen`, a silent push with
//! the new badge. The key never leaves the hub's host.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};

/// `[push]` in hub-server.toml.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PushConfig {
    /// The APNs auth key (`.p8`, PKCS#8 PEM) from the Apple developer
    /// account, or `apns_key_env` naming a variable that holds its text.
    #[serde(default)]
    pub apns_key: Option<PathBuf>,
    #[serde(default)]
    pub apns_key_env: Option<String>,
    /// The key's id (ten characters) and the team id.
    #[serde(default)]
    pub key_id: String,
    #[serde(default)]
    pub team_id: String,
    /// The app's bundle id: the `apns-topic`.
    #[serde(default = "default_topic")]
    pub topic: String,
    /// Debug builds register with the sandbox host; a token says which.
    /// This is the default for tokens that do not say.
    #[serde(default)]
    pub sandbox: bool,
    /// Override of the APNs base URL (tests point it at a local server).
    #[serde(default)]
    pub url: Option<String>,
}

fn default_topic() -> String {
    "com.unarbos.arbos.ios".into()
}

pub const PRODUCTION_URL: &str = "https://api.push.apple.com";
pub const SANDBOX_URL: &str = "https://api.sandbox.push.apple.com";
/// Apple asks for a fresh token at least every hour, and no more often
/// than every twenty minutes.
const JWT_LIFETIME: Duration = Duration::from_secs(45 * 60);

/// One phone's registration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Device {
    pub token: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub sandbox: bool,
    /// `<machine>/<project>` addresses this device wants; `*` for every
    /// project of its user.
    #[serde(default)]
    pub projects: BTreeSet<String>,
    /// The user the registering token belonged to.
    #[serde(default)]
    pub user: String,
    pub since_ms: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    devices: Vec<Device>,
}

/// The devices to push to, and how.
pub struct Push {
    cfg: PushConfig,
    path: PathBuf,
    devices: Mutex<Registry>,
    signer: Option<Signer>,
    /// Why pushes are off when they are: no key, or a key that did not
    /// load. Shown on `GET /push` and in the `pushed` reply, so a bad
    /// path or a malformed .p8 is seen the day it is set.
    reason: Option<String>,
    http: reqwest::Client,
    /// The last attempts, newest last, for `GET /push`: the first place
    /// to look when a push did not arrive.
    journal: Mutex<Vec<Attempt>>,
}

/// How many attempts `GET /push` remembers.
const JOURNAL_LEN: usize = 50;

/// One attempt as the journal keeps it.
#[derive(Debug, Clone, Serialize)]
pub struct Attempt {
    pub ts: i64,
    /// What was sent: `alert`, `background`, `test`.
    pub kind: String,
    pub project: String,
    /// The token's last eight characters, never the whole.
    pub token: String,
    /// The device's user, kept here so a forgotten device's attempts
    /// still show to its owner.
    pub user: String,
    pub status: u16,
    pub detail: String,
}

impl Push {
    /// `cfg` from the hub's config; `dir` is where `hub-push.json` lives.
    /// Without a key the registry still works (tokens are kept for when a
    /// key is configured) and nothing is sent. A key that does not load —
    /// a wrong path, a malformed .p8, an id missing — does not stop the
    /// hub: pushes are off with the reason kept, and the hub serves.
    pub fn new(cfg: PushConfig, dir: &Path) -> Result<Self> {
        let (signer, reason) = match load_signer(&cfg) {
            Ok(Some(s)) => (Some(s), None),
            Ok(None) => (
                None,
                Some("no APNs key on the hub: set [push] apns_key, key_id and team_id in hub-server.toml".to_string()),
            ),
            Err(e) => (None, Some(format!("{e:#}"))),
        };
        let path = dir.join("hub-push.json");
        let devices = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Ok(Self {
            cfg,
            path,
            devices: Mutex::new(devices),
            signer,
            reason,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()?,
            journal: Mutex::new(Vec::new()),
        })
    }

    /// Whether a key is configured: pushes go out.
    pub fn enabled(&self) -> bool {
        self.signer.is_some()
    }

    /// Why pushes are off, when they are.
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// The state for `GET /push`: whether pushes go out and why not, the
    /// topic, the devices (token tails only) for `user` — every user's
    /// for an owner or admin — and the last attempts.
    pub fn status(&self, user: &str, all_users: bool) -> serde_json::Value {
        let devices: Vec<serde_json::Value> = self
            .devices
            .lock()
            .unwrap()
            .devices
            .iter()
            .filter(|d| all_users || d.user == user)
            .map(|d| {
                serde_json::json!({
                    "token": tail(&d.token),
                    "platform": d.platform,
                    "sandbox": d.sandbox,
                    "projects": d.projects,
                    "user": d.user,
                    "since_ms": d.since_ms,
                })
            })
            .collect();
        let attempts: Vec<Attempt> = self
            .journal
            .lock()
            .unwrap()
            .iter()
            .filter(|a| all_users || a.user == user)
            .cloned()
            .collect();
        serde_json::json!({
            "enabled": self.enabled(),
            "reason": self.reason,
            "topic": self.cfg.topic,
            "key_id": self.cfg.key_id,
            "sandbox_default": self.cfg.sandbox,
            "url": self.cfg.url,
            "devices": devices,
            "attempts": attempts,
        })
    }

    /// A test alert to `user`'s devices whose token ends in `token_tail`
    /// (all of the user's when empty): the way to see a push arrive the
    /// day the key is set, without waiting for a worker to finish.
    pub async fn test(&self, user: &str, token_tail: &str) -> Vec<Delivery> {
        let payload = serde_json::json!({
            "aps": {
                "alert": {"title": "Arbos", "body": "Push works. This is a test from the hub."},
                "sound": "default",
            },
            "target": "hub:test",
            "kind": "test",
        });
        let targets: Vec<Device> = self
            .devices
            .lock()
            .unwrap()
            .devices
            .iter()
            .filter(|d| d.user == user && (token_tail.is_empty() || tail(&d.token) == token_tail))
            .cloned()
            .collect();
        self.send_to(&targets, "test", "test", &payload, "alert", None)
            .await
    }

    pub fn device_count(&self) -> usize {
        self.devices.lock().unwrap().devices.len()
    }

    /// A phone's `push` frame from an attach socket. `project` is the
    /// address it attached to (`<machine>/<project>`), or `*`. The same
    /// token registering again replaces its row (tokens rotate; projects
    /// accumulate).
    pub fn register(
        &self,
        token: &str,
        platform: &str,
        sandbox: Option<bool>,
        project: &str,
        user: &str,
    ) -> Result<()> {
        let token = token.trim().to_ascii_lowercase();
        if token.is_empty() || token.len() > 200 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("push: token must be the device token as hex");
        }
        let mut reg = self.devices.lock().unwrap();
        let row = match reg.devices.iter_mut().find(|d| d.token == token) {
            Some(d) => d,
            None => {
                reg.devices.push(Device {
                    token: token.clone(),
                    platform: platform.to_string(),
                    sandbox: sandbox.unwrap_or(self.cfg.sandbox),
                    projects: BTreeSet::new(),
                    user: user.to_string(),
                    since_ms: arbos_core::now_ms(),
                });
                reg.devices.last_mut().unwrap()
            }
        };
        if let Some(s) = sandbox {
            row.sandbox = s;
        }
        row.user = user.to_string();
        row.projects.insert(project.to_string());
        // Registered means on disk: a phone told "pushed" whose row was
        // only in memory lost its notifications at the hub's next start,
        // and nothing said so. The save's failure is the answer.
        self.save(&reg).with_context(|| {
            format!(
                "push: the device could not be recorded in {}",
                self.path.display()
            )
        })
    }

    fn save(&self, reg: &Registry) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(reg)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn devices_for(&self, project: &str, user: &str) -> Vec<Device> {
        self.devices
            .lock()
            .unwrap()
            .devices
            .iter()
            .filter(|d| {
                d.projects.contains(project)
                    || (d.projects.contains("*") && (user.is_empty() || d.user == user))
            })
            .cloned()
            .collect()
    }

    fn forget(&self, token: &str) {
        let mut reg = self.devices.lock().unwrap();
        let before = reg.devices.len();
        reg.devices.retain(|d| d.token != token);
        if reg.devices.len() != before
            && let Err(e) = self.save(&reg)
        {
            eprintln!(
                "push: forgetting {}…: the registry could not be saved: {e:#}",
                &token[..token.len().min(8)]
            );
        }
    }

    /// A kernel's notification: an alert to every device registered for
    /// its project. `project` is `<machine>/<project>`; `user` the machine
    /// token's user (for `*` registrations).
    pub async fn notify(&self, project: &str, user: &str, n: &Notice) -> Vec<Delivery> {
        let title = format!(
            "{} · {}",
            project.rsplit('/').next().unwrap_or(project),
            n.title
        );
        let mut aps = serde_json::json!({
            "alert": {"title": title, "body": n.body},
            "badge": n.unseen,
            "sound": "default",
            "thread-id": project,
        });
        if n.kind == "ask" {
            aps["interruption-level"] = serde_json::Value::String("time-sensitive".into());
        }
        let payload = serde_json::json!({
            "aps": aps,
            "target": format!("hub:{project}"),
            "id": n.id,
            "kind": n.kind,
            "agent": n.agent,
        });
        self.send_all(project, user, &payload, "alert", Some(project))
            .await
    }

    /// The user saw through `through` elsewhere: a silent push with the
    /// badge that is left, so the phone's number drops without the app.
    pub async fn seen(&self, project: &str, user: &str, unseen: u64) -> Vec<Delivery> {
        let payload = serde_json::json!({
            "aps": {"content-available": 1, "badge": unseen},
            "target": format!("hub:{project}"),
            "seen": true,
        });
        self.send_all(project, user, &payload, "background", None)
            .await
    }

    async fn send_all(
        &self,
        project: &str,
        user: &str,
        payload: &serde_json::Value,
        push_type: &str,
        collapse: Option<&str>,
    ) -> Vec<Delivery> {
        let targets = self.devices_for(project, user);
        self.send_to(&targets, project, push_type, payload, push_type, collapse)
            .await
    }

    async fn send_to(
        &self,
        targets: &[Device],
        project: &str,
        journal_kind: &str,
        payload: &serde_json::Value,
        push_type: &str,
        collapse: Option<&str>,
    ) -> Vec<Delivery> {
        let mut out = Vec::new();
        let Some(signer) = &self.signer else {
            for d in targets {
                self.record(
                    journal_kind,
                    project,
                    d,
                    0,
                    self.reason.clone().unwrap_or_default(),
                );
            }
            return out;
        };
        for d in targets.iter().cloned() {
            let base = match (&self.cfg.url, d.sandbox) {
                (Some(u), _) => u.trim_end_matches('/').to_string(),
                (None, true) => SANDBOX_URL.into(),
                (None, false) => PRODUCTION_URL.into(),
            };
            let url = format!("{base}/3/device/{}", d.token);
            let jwt = match signer.token() {
                Ok(j) => j,
                Err(e) => {
                    out.push(Delivery {
                        token: d.token.clone(),
                        status: 0,
                        detail: format!("jwt: {e:#}"),
                    });
                    continue;
                }
            };
            let mut req = self
                .http
                .post(&url)
                .header("authorization", format!("bearer {jwt}"))
                .header("apns-topic", &self.cfg.topic)
                .header("apns-push-type", push_type)
                .header(
                    "apns-priority",
                    if push_type == "alert" { "10" } else { "5" },
                )
                .json(payload);
            if let Some(c) = collapse {
                req = req.header("apns-collapse-id", c);
            }
            let delivery = match req.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let detail = resp.text().await.unwrap_or_default();
                    if status == 410
                        || detail.contains("BadDeviceToken")
                        || detail.contains("Unregistered")
                    {
                        self.forget(&d.token);
                    }
                    Delivery {
                        token: d.token.clone(),
                        status,
                        detail,
                    }
                }
                Err(e) => Delivery {
                    token: d.token.clone(),
                    status: 0,
                    detail: e.to_string(),
                },
            };
            self.record(
                journal_kind,
                project,
                &d,
                delivery.status,
                delivery.detail.clone(),
            );
            out.push(delivery);
        }
        out
    }

    fn record(&self, kind: &str, project: &str, device: &Device, status: u16, detail: String) {
        let mut j = self.journal.lock().unwrap();
        j.push(Attempt {
            ts: arbos_core::now_ms(),
            kind: kind.to_string(),
            project: project.to_string(),
            token: tail(&device.token),
            user: device.user.clone(),
            status,
            detail: detail.trim().chars().take(300).collect(),
        });
        let extra = j.len().saturating_sub(JOURNAL_LEN);
        j.drain(..extra);
    }
}

/// The last eight characters of a token: enough to tell devices apart in
/// a status page, never the whole.
fn tail(token: &str) -> String {
    let n = token.len().saturating_sub(8);
    token[n..].to_string()
}

/// The signer from the config, or `None` when no key is named.
fn load_signer(cfg: &PushConfig) -> Result<Option<Signer>> {
    match key_text(cfg)? {
        Some(pem) if !cfg.key_id.is_empty() && !cfg.team_id.is_empty() => {
            Ok(Some(Signer::from_pem(&pem, &cfg.key_id, &cfg.team_id)?))
        }
        Some(_) => bail!("[push]: apns_key is set but key_id or team_id is empty"),
        None => Ok(None),
    }
}

/// What a `HubFrame::Notify` carries, for `Push::notify`.
#[derive(Debug, Clone)]
pub struct Notice {
    pub id: u64,
    pub agent: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub unseen: u64,
}

/// One attempt to one device.
#[derive(Debug, Clone)]
pub struct Delivery {
    pub token: String,
    /// HTTP status; 0 when the request itself failed.
    pub status: u16,
    pub detail: String,
}

fn key_text(cfg: &PushConfig) -> Result<Option<String>> {
    if let Some(env) = cfg
        .apns_key_env
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
    {
        return match std::env::var(env) {
            Ok(v) if !v.trim().is_empty() => Ok(Some(v)),
            _ => bail!("[push]: apns_key_env = {env:?} is not set"),
        };
    }
    match &cfg.apns_key {
        Some(p) => {
            Ok(Some(std::fs::read_to_string(p).with_context(|| {
                format!("[push]: read {}", p.display())
            })?))
        }
        None => Ok(None),
    }
}

/// ES256 JWTs for APNs from the `.p8` key, refreshed every 45 minutes.
pub struct Signer {
    key: ring::signature::EcdsaKeyPair,
    key_id: String,
    team_id: String,
    cached: Mutex<Option<(Instant, String)>>,
}

impl Signer {
    pub fn from_pem(pem: &str, key_id: &str, team_id: &str) -> Result<Self> {
        let der = pem_to_der(pem)?;
        let rng = ring::rand::SystemRandom::new();
        let key = ring::signature::EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &der,
            &rng,
        )
        .map_err(|e| anyhow::anyhow!("[push]: the APNs key is not a P-256 PKCS#8 key: {e}"))?;
        Ok(Self {
            key,
            key_id: key_id.to_string(),
            team_id: team_id.to_string(),
            cached: Mutex::new(None),
        })
    }

    /// The current token: reused within its lifetime, signed afresh after.
    pub fn token(&self) -> Result<String> {
        if let Some((at, t)) = self.cached.lock().unwrap().as_ref()
            && at.elapsed() < JWT_LIFETIME
        {
            return Ok(t.clone());
        }
        let t = self.sign(arbos_core::now_ms() / 1000)?;
        *self.cached.lock().unwrap() = Some((Instant::now(), t.clone()));
        Ok(t)
    }

    /// `header.claims.signature`, base64url without padding.
    pub fn sign(&self, iat: i64) -> Result<String> {
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header =
            b64.encode(serde_json::json!({"alg": "ES256", "kid": self.key_id}).to_string());
        let claims = b64.encode(serde_json::json!({"iss": self.team_id, "iat": iat}).to_string());
        let signing_input = format!("{header}.{claims}");
        let rng = ring::rand::SystemRandom::new();
        let sig = self
            .key
            .sign(&rng, signing_input.as_bytes())
            .map_err(|e| anyhow::anyhow!("sign: {e}"))?;
        Ok(format!("{signing_input}.{}", b64.encode(sig.as_ref())))
    }
}

/// The DER inside a `-----BEGIN PRIVATE KEY-----` block.
fn pem_to_der(pem: &str) -> Result<Vec<u8>> {
    let body: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .map(str::trim)
        .collect();
    if body.is_empty() {
        bail!("[push]: the APNs key file has no PEM body");
    }
    base64::engine::general_purpose::STANDARD
        .decode(body.as_bytes())
        .context("[push]: the APNs key is not base64 PEM")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh P-256 key as a `.p8` would hold it.
    fn test_pem() -> String {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &rng,
        )
        .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(pkcs8.as_ref());
        let lines: Vec<String> = b64
            .as_bytes()
            .chunks(64)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect();
        format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            lines.join("\n")
        )
    }

    #[test]
    fn the_jwt_has_apples_shape_and_verifies() {
        let pem = test_pem();
        let s = Signer::from_pem(&pem, "ABC123DEFG", "25SCF3Q2AK").unwrap();
        let jwt = s.sign(1_700_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header: serde_json::Value =
            serde_json::from_slice(&b64.decode(parts[0]).unwrap()).unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&b64.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["kid"], "ABC123DEFG");
        assert_eq!(claims["iss"], "25SCF3Q2AK");
        assert_eq!(claims["iat"], 1_700_000_000);
        // The signature verifies against the key's public half.
        let der = pem_to_der(&pem).unwrap();
        let rng = ring::rand::SystemRandom::new();
        let key = ring::signature::EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &der,
            &rng,
        )
        .unwrap();
        use ring::signature::KeyPair;
        let public = ring::signature::UnparsedPublicKey::new(
            &ring::signature::ECDSA_P256_SHA256_FIXED,
            key.public_key().as_ref(),
        );
        let sig = b64.decode(parts[2]).unwrap();
        public
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
            .unwrap();
        // Cached within its lifetime.
        assert_eq!(s.token().unwrap(), s.token().unwrap());
    }

    #[test]
    fn the_registry_keeps_one_row_per_token_and_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let push = Push::new(PushConfig::default(), dir.path()).unwrap();
        assert!(!push.enabled(), "no key: nothing is sent, tokens are kept");
        push.register("ABCDEF0123", "apns", Some(true), "arboslife/phone", "jacob")
            .unwrap();
        push.register("abcdef0123", "apns", None, "arboslife/demo", "jacob")
            .unwrap();
        push.register("ffff", "apns", None, "*", "jacob").unwrap();
        assert!(
            push.register("not hex!", "apns", None, "x/y", "jacob")
                .is_err()
        );
        assert_eq!(push.device_count(), 2);
        let again = Push::new(PushConfig::default(), dir.path()).unwrap();
        assert_eq!(again.device_count(), 2);
        let phone = again.devices_for("arboslife/phone", "jacob");
        assert_eq!(
            phone.len(),
            2,
            "the named device and the * device: {phone:?}"
        );
        assert!(
            phone
                .iter()
                .any(|d| d.token == "abcdef0123" && d.sandbox && d.projects.len() == 2)
        );
        assert!(
            again
                .devices_for("arboslife/other", "someone-else")
                .is_empty()
        );
        assert_eq!(
            again.devices_for("arboslife/other", "jacob").len(),
            1,
            "* covers the user's projects"
        );
    }

    /// A fake APNs on loopback: the request has Apple's headers and the
    /// payload the phone expects; a 410 drops the token.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_alert_is_posted_with_apples_headers_and_a_410_drops_the_token() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let seen2 = std::sync::Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let seen = std::sync::Arc::clone(&seen2);
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 8192];
                    loop {
                        let n = stream.read(&mut chunk).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..pos]).to_string();
                            let len = head
                                .lines()
                                .find_map(|l| {
                                    l.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                                })
                                .unwrap_or(0);
                            if buf.len() >= pos + 4 + len {
                                break;
                            }
                        }
                    }
                    let text = String::from_utf8_lossy(&buf).to_string();
                    let gone = text.contains("/3/device/dead");
                    seen.lock().unwrap().push(text);
                    let (status, body) = if gone {
                        (410, r#"{"reason":"Unregistered"}"#)
                    } else {
                        (200, "")
                    };
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\napns-id: 1\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.flush();
                });
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("apns.p8");
        std::fs::write(&key, test_pem()).unwrap();
        let cfg = PushConfig {
            apns_key: Some(key),
            apns_key_env: None,
            key_id: "ABC123DEFG".into(),
            team_id: "25SCF3Q2AK".into(),
            topic: "com.unarbos.arbos.ios".into(),
            sandbox: false,
            url: Some(format!("http://127.0.0.1:{port}")),
        };
        let push = Push::new(cfg, dir.path()).unwrap();
        assert!(push.enabled());
        push.register("abc123", "apns", None, "arboslife/demo", "jacob")
            .unwrap();
        push.register("dead", "apns", None, "arboslife/demo", "jacob")
            .unwrap();
        let n = Notice {
            id: 7,
            agent: "root".into(),
            kind: "ask".into(),
            title: "root asks".into(),
            body: "Which branch?".into(),
            unseen: 2,
        };
        let deliveries = push.notify("arboslife/demo", "jacob", &n).await;
        assert_eq!(deliveries.len(), 2, "{deliveries:?}");
        assert!(
            deliveries
                .iter()
                .any(|d| d.token == "abc123" && d.status == 200),
            "{deliveries:?}"
        );
        assert!(
            deliveries
                .iter()
                .any(|d| d.token == "dead" && d.status == 410),
            "{deliveries:?}"
        );
        assert_eq!(push.device_count(), 1, "the 410 token is forgotten");
        let requests = seen.lock().unwrap().clone();
        let req = requests
            .iter()
            .find(|r| r.contains("/3/device/abc123"))
            .unwrap();
        let lower = req.to_ascii_lowercase();
        assert!(lower.contains("apns-topic: com.unarbos.arbos.ios"), "{req}");
        assert!(lower.contains("apns-push-type: alert"), "{req}");
        assert!(lower.contains("apns-collapse-id: arboslife/demo"), "{req}");
        assert!(lower.contains("authorization: bearer ey"), "{req}");
        let body: serde_json::Value =
            serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["aps"]["alert"]["title"], "demo · root asks");
        assert_eq!(body["aps"]["alert"]["body"], "Which branch?");
        assert_eq!(body["aps"]["badge"], 2);
        assert_eq!(body["aps"]["interruption-level"], "time-sensitive");
        assert_eq!(body["aps"]["thread-id"], "arboslife/demo");
        assert_eq!(body["target"], "hub:arboslife/demo");
        assert_eq!(body["id"], 7);
        // Seen: a silent push with the badge left.
        let d = push.seen("arboslife/demo", "jacob", 0).await;
        assert_eq!(d.len(), 1);
        let requests = seen.lock().unwrap().clone();
        let req = requests.last().unwrap();
        assert!(
            req.to_ascii_lowercase()
                .contains("apns-push-type: background"),
            "{req}"
        );
        let body: serde_json::Value =
            serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["aps"]["content-available"], 1);
        assert_eq!(body["aps"]["badge"], 0);

        // The journal and the status page: token tails only, the 410 on
        // record, and a test alert for the day the key is set.
        let status = push.status("jacob", false);
        assert_eq!(status["enabled"], true);
        assert!(status["reason"].is_null());
        let devices = status["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(
            devices[0]["token"], "abc123",
            "a short token is its own tail"
        );
        let attempts = status["attempts"].as_array().unwrap();
        assert!(
            attempts
                .iter()
                .any(|a| a["token"] == "dead" && a["status"] == 410),
            "{attempts:?}"
        );
        assert!(
            attempts
                .iter()
                .any(|a| a["kind"] == "background" && a["status"] == 200),
            "{attempts:?}"
        );
        assert!(status.to_string().contains("abc123"));
        let other = push.status("someone-else", false);
        assert!(
            other["devices"].as_array().unwrap().is_empty(),
            "another user sees no devices"
        );
        let t = push.test("jacob", "").await;
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].status, 200);
        let req = seen.lock().unwrap().last().cloned().unwrap();
        let body: serde_json::Value =
            serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["kind"], "test");
        assert!(
            body["aps"]["alert"]["body"]
                .as_str()
                .unwrap()
                .contains("Push works")
        );
        assert!(
            push.test("jacob", "zzz").await.is_empty(),
            "no device with that tail"
        );
    }

    /// A key that does not load must not take the hub down: pushes are
    /// off with the reason kept, registrations still work, and a test
    /// push says why nothing went.
    #[tokio::test]
    async fn a_bad_key_leaves_the_hub_up_with_push_off_and_the_reason_kept() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = PushConfig {
            apns_key: Some(dir.path().join("does-not-exist.p8")),
            apns_key_env: None,
            key_id: "ABC123DEFG".into(),
            team_id: "25SCF3Q2AK".into(),
            topic: default_topic(),
            sandbox: false,
            url: None,
        };
        let push = Push::new(cfg, dir.path()).expect("the hub still constructs");
        assert!(!push.enabled());
        let why = push.reason().unwrap();
        assert!(why.contains("does-not-exist.p8"), "{why}");
        push.register("abcdef1234567890", "apns", None, "arboslife/demo", "jacob")
            .unwrap();
        assert_eq!(push.device_count(), 1);
        let t = push.test("jacob", "").await;
        assert!(t.is_empty(), "nothing is sent without a key");
        let status = push.status("jacob", false);
        assert_eq!(status["enabled"], false);
        assert!(
            status["reason"]
                .as_str()
                .unwrap()
                .contains("does-not-exist.p8")
        );
        assert_eq!(
            status["devices"][0]["token"], "34567890",
            "the tail, not the token"
        );
        let attempts = status["attempts"].as_array().unwrap();
        assert_eq!(
            attempts.len(),
            1,
            "the refused attempt is on record: {attempts:?}"
        );
        assert_eq!(attempts[0]["status"], 0);
        assert!(
            attempts[0]["detail"]
                .as_str()
                .unwrap()
                .contains("does-not-exist.p8")
        );

        // A malformed .p8 is the same story, with its own reason.
        let bad = dir.path().join("bad.p8");
        std::fs::write(
            &bad,
            "-----BEGIN PRIVATE KEY-----\nnot a key\n-----END PRIVATE KEY-----\n",
        )
        .unwrap();
        let cfg = PushConfig {
            apns_key: Some(bad),
            apns_key_env: None,
            key_id: "ABC123DEFG".into(),
            team_id: "25SCF3Q2AK".into(),
            topic: default_topic(),
            sandbox: false,
            url: None,
        };
        let push = Push::new(cfg, dir.path()).unwrap();
        assert!(!push.enabled() && push.reason().is_some());
        // No key named at all: the plain reason.
        let push = Push::new(PushConfig::default(), dir.path()).unwrap();
        assert!(push.reason().unwrap().starts_with("no APNs key on the hub"));
    }
}
