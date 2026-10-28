//! Wake URL for outside agents (2026-10-28, Jacob: "my Grok outside agent should wake
//! instantly instead of polling every 5 min"; spec `docs/outside-agents-spec.md` §6.1 retries,
//! §10.4 Grok, §13.2 SSRF). An owner can give a connected agent one HTTPS URL and the exact
//! `Authorization` header value its webhook expects (Grok Bot's routine panel shows both). When
//! an event with `wake: true` (or `member.added`) enters the agent's inbox, the relay POSTs a
//! small JSON nudge there — `{agent, chat, event_id, type}`, never the message text — so the
//! agent can read its inbox at once. Nudges are coalesced to one per [`DEBOUNCE`] per agent,
//! retried on the spec's schedule, and the last delivery is recorded for the Connection card.
//!
//! Headers on every nudge: `Authorization: <value, verbatim>` and
//! `X-Arbos-Signature: sha256=<hex hmac-sha256(key = value, body)>`.
//!
//! The header value is stored server-side and never returned by any API; logs carry the
//! agent id, the status and the error only.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use arbos_core::ids::now_ms;
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use reqwest::Url;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::sync::Notify;

use crate::link_icons::{is_private_ip, public_domain};
use crate::state::AppState;

/// The header the value goes in, verbatim (Jacob, 2026-10-28: no prefix added).
pub const HEADER: &str = "Authorization";
/// The signature header: `sha256=<hex>`.
pub const SIGNATURE_HEADER: &str = "X-Arbos-Signature";
/// At most one nudge per agent in this window; later wakes inside it coalesce into one.
pub const DEBOUNCE: Duration = Duration::from_secs(5);
/// Connect + response budget for one POST.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// Longest URL and header value the owner may set.
const URL_MAX: usize = 2048;
const AUTH_MAX: usize = 1024;
/// How much of an error reply is kept for the Connection card.
const ERROR_MAX: usize = 160;
const AGENT: &str = "arbos-relay (+https://arbos.life)";
/// Type of the nudge "Send test wake" sends.
pub const TEST_TYPE: &str = "wake.test";

/// Schema columns on `outside_agents` (additive; `db.rs` runs these after the base schema).
pub const MIGRATIONS: &[&str] = &[
    "ALTER TABLE outside_agents ADD COLUMN wake_url TEXT",
    "ALTER TABLE outside_agents ADD COLUMN wake_auth TEXT",
    "ALTER TABLE outside_agents ADD COLUMN wake_last_at INTEGER",
    "ALTER TABLE outside_agents ADD COLUMN wake_last_status INTEGER",
    "ALTER TABLE outside_agents ADD COLUMN wake_last_error TEXT",
];

/// Spec §6.1: 1 s, 5 s, 30 s, 2 min, 10 min, 30 min, then hourly until 24 h have passed;
/// `None` = give up.
pub fn retry_delay(attempt: usize) -> Option<Duration> {
    const FIRST: [u64; 6] = [1, 5, 30, 120, 600, 1800];
    // The first six add up to 41 min; 24 hourly tries take the total past 24 h.
    const HOURLY: usize = 24;
    if attempt < FIRST.len() {
        Some(Duration::from_secs(FIRST[attempt]))
    } else if attempt < FIRST.len() + HOURLY {
        Some(Duration::from_secs(3600))
    } else {
        None
    }
}

/// What one nudge says. `chat` is `None` for a test nudge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nudge {
    pub agent: String,
    pub chat: Option<String>,
    pub event_id: i64,
    pub kind: String,
}

impl Nudge {
    pub fn test(agent: &str) -> Self {
        Nudge {
            agent: agent.to_owned(),
            chat: None,
            event_id: 0,
            kind: TEST_TYPE.to_owned(),
        }
    }

    /// The body, byte-for-byte what is signed.
    pub fn body(&self) -> String {
        json!({
            "agent": self.agent,
            "chat": self.chat,
            "event_id": self.event_id,
            "type": self.kind,
        })
        .to_string()
    }
}

/// `sha256=<hex hmac-sha256(key, body)>`.
pub fn signature(key: &str, body: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("hmac takes any key size");
    mac.update(body.as_bytes());
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// The last delivery, for the Connection card and `GET /asp/me`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LastWake {
    pub at: i64,
    pub status: Option<u16>,
    pub error: Option<String>,
}

impl LastWake {
    pub fn json(&self) -> Value {
        json!({ "at": self.at, "status": self.status, "error": self.error })
    }
}

/// Where and how to nudge one agent.
#[derive(Clone, Debug)]
pub struct WakeConfig {
    pub url: String,
    pub auth: String,
}

// ---- validation ----------------------------------------------------------------------------------

/// A URL the owner may set: `https`, a public host name (no IP literal, nothing local), no
/// credentials in it, at most [`URL_MAX`] characters. Returns the normalised form.
pub fn check_url(raw: &str) -> Result<String, &'static str> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("a URL is required");
    }
    if raw.len() > URL_MAX {
        return Err("the URL is too long");
    }
    let url = Url::parse(raw).map_err(|_| "that is not a URL")?;
    if url.scheme() != "https" {
        return Err("the URL must start with https://");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("the URL must not carry a user name or password");
    }
    let host = url.host_str().ok_or("the URL needs a host")?;
    if public_domain(host).is_none() {
        return Err("the host must be a public domain name");
    }
    Ok(url.to_string())
}

/// A header value the owner may set: one line, printable, at most [`AUTH_MAX`] characters.
pub fn check_auth(raw: &str) -> Result<String, &'static str> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("the Authorization value is required");
    }
    if raw.len() > AUTH_MAX {
        return Err("the Authorization value is too long");
    }
    if HeaderValue::from_str(raw).is_err() || raw.chars().any(|c| c.is_control()) {
        return Err("the Authorization value has characters a header cannot carry");
    }
    Ok(raw.to_owned())
}

// ---- storage -------------------------------------------------------------------------------------

impl crate::db::Db {
    /// URL + header value, or `None` until both are set.
    pub fn wake_config(&self, project_id: &str) -> anyhow::Result<Option<WakeConfig>> {
        self.with(|c| {
            let row: Option<(Option<String>, Option<String>)> = c
                .query_row(
                    "SELECT wake_url, wake_auth FROM outside_agents WHERE project_id = ?1 AND revoked_at IS NULL",
                    params![project_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            Ok(match row {
                Some((Some(url), Some(auth))) if !url.is_empty() && !auth.is_empty() => {
                    Some(WakeConfig { url, auth })
                }
                _ => None,
            })
        })
    }

    /// `None` clears. A changed URL also forgets the last delivery.
    pub fn set_wake_url(&self, project_id: &str, url: Option<&str>) -> anyhow::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE outside_agents SET wake_url = ?2, wake_last_at = NULL, wake_last_status = NULL, wake_last_error = NULL WHERE project_id = ?1",
                params![project_id, url],
            )?;
            Ok(())
        })
    }

    /// `None` clears.
    pub fn set_wake_auth(&self, project_id: &str, auth: Option<&str>) -> anyhow::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE outside_agents SET wake_auth = ?2 WHERE project_id = ?1",
                params![project_id, auth],
            )?;
            Ok(())
        })
    }

    pub fn record_wake(&self, project_id: &str, last: &LastWake) -> anyhow::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE outside_agents SET wake_last_at = ?2, wake_last_status = ?3, wake_last_error = ?4 WHERE project_id = ?1",
                params![project_id, last.at, last.status, last.error],
            )?;
            Ok(())
        })
    }
}

// ---- sending -------------------------------------------------------------------------------------

static INSECURE: AtomicBool = AtomicBool::new(false);

fn insecure_allowed() -> bool {
    INSECURE.load(Ordering::Relaxed)
}

/// Tests only: let `http://127.0.0.1` through the guard so a local listener can catch nudges.
#[cfg(test)]
pub fn allow_insecure_for_tests() {
    INSECURE.store(true, Ordering::Relaxed);
}

fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= ERROR_MAX {
        return s.to_owned();
    }
    let cut: String = s.chars().take(ERROR_MAX).collect();
    format!("{cut}…")
}

/// One POST of `nudge` to `cfg.url`: host resolved first and pinned (no private, loopback,
/// link-local, CGNAT or multicast address), no redirects followed, [`TIMEOUT`] overall.
/// `Ok(status)` on 2xx; `Err(short reason)` otherwise.
pub async fn send(cfg: &WakeConfig, nudge: &Nudge) -> Result<u16, String> {
    let url = Url::parse(&cfg.url).map_err(|_| "bad URL".to_owned())?;
    let host = url
        .host_str()
        .ok_or_else(|| "bad URL".to_owned())?
        .to_owned();
    let port = url.port_or_known_default().unwrap_or(443);
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(TIMEOUT)
        .timeout(TIMEOUT)
        .no_proxy();
    if !insecure_allowed() {
        if url.scheme() != "https" {
            return Err("only https URLs".into());
        }
        if public_domain(&host).is_none() {
            return Err("host is not a public domain".into());
        }
        let addrs: Vec<std::net::SocketAddr> =
            tokio::time::timeout(TIMEOUT, tokio::net::lookup_host((host.as_str(), port)))
                .await
                .map_err(|_| "DNS timed out".to_owned())?
                .map_err(|e| format!("DNS: {e}"))?
                .collect();
        if addrs.is_empty() {
            return Err("host has no address".into());
        }
        if addrs.iter().any(|a| is_private_ip(a.ip())) {
            return Err("host resolves to a private address".into());
        }
        builder = builder.resolve_to_addrs(&host, &addrs);
    }
    let client = builder.build().map_err(|e| e.to_string())?;
    let body = nudge.body();
    let auth = HeaderValue::from_str(&cfg.auth).map_err(|_| "bad header value".to_owned())?;
    let started = Instant::now();
    let resp = client
        .post(url)
        .header(CONTENT_TYPE, "application/json")
        .header(USER_AGENT, AGENT)
        .header(AUTHORIZATION, auth)
        .header(SIGNATURE_HEADER, signature(&cfg.auth, &body))
        .body(body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                format!("timed out after {} ms", started.elapsed().as_millis())
            } else if e.is_connect() {
                "could not connect".to_owned()
            } else {
                short(&e.without_url().to_string())
            }
        })?;
    let status = resp.status();
    if status.is_success() {
        return Ok(status.as_u16());
    }
    if status.is_redirection() {
        return Err(format!(
            "HTTP {} (redirects are not followed)",
            status.as_u16()
        ));
    }
    let text = resp.text().await.unwrap_or_default();
    let text = short(&text);
    Err(if text.is_empty() {
        format!("HTTP {}", status.as_u16())
    } else {
        format!("HTTP {}: {text}", status.as_u16())
    })
}

/// Send one nudge now and record it (the test button and `POST /asp/wake/test`): no debounce,
/// no retry. `Err` when no Wake URL is set.
pub async fn send_now(
    state: &AppState,
    project_id: &str,
    nudge: &Nudge,
) -> Result<LastWake, String> {
    let cfg = state
        .db
        .wake_config(project_id)
        .ok()
        .flatten()
        .ok_or_else(|| "no Wake URL set".to_owned())?;
    let result = send(&cfg, nudge).await;
    let last = LastWake {
        at: now_ms(),
        status: result.as_ref().ok().copied(),
        error: result.as_ref().err().cloned(),
    };
    let _ = state.db.record_wake(project_id, &last);
    match &result {
        Ok(status) => {
            tracing::info!(project = %project_id, status, kind = %nudge.kind, "wake nudge delivered")
        }
        Err(e) => {
            tracing::info!(project = %project_id, error = %e, kind = %nudge.kind, "wake nudge failed")
        }
    }
    Ok(last)
}

// ---- the per-agent worker ------------------------------------------------------------------------

struct Slot {
    pending: StdMutex<Option<Nudge>>,
    notify: Notify,
    running: AtomicBool,
}

fn slots() -> &'static StdMutex<HashMap<String, Arc<Slot>>> {
    static S: OnceLock<StdMutex<HashMap<String, Arc<Slot>>>> = OnceLock::new();
    S.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn slot(project_id: &str) -> Arc<Slot> {
    slots()
        .lock()
        .unwrap()
        .entry(project_id.to_owned())
        .or_insert_with(|| {
            Arc::new(Slot {
                pending: StdMutex::new(None),
                notify: Notify::new(),
                running: AtomicBool::new(false),
            })
        })
        .clone()
}

/// An event that should wake the agent was queued: nudge its Wake URL (coalesced, retried).
/// Nothing happens for an agent without one.
pub fn nudge(state: &AppState, nudge: Nudge) {
    let project_id = nudge.agent.clone();
    if state.db.wake_config(&project_id).ok().flatten().is_none() {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let slot = slot(&project_id);
    *slot.pending.lock().unwrap() = Some(nudge);
    slot.notify.notify_one();
    if !slot.running.swap(true, Ordering::SeqCst) {
        let state = state.clone();
        handle.spawn(run(state, project_id, slot));
    }
}

async fn take_pending(slot: &Slot) -> Nudge {
    loop {
        if let Some(n) = slot.pending.lock().unwrap().take() {
            return n;
        }
        slot.notify.notified().await;
    }
}

async fn run(state: AppState, project_id: String, slot: Arc<Slot>) {
    let mut last_sent: Option<Instant> = None;
    loop {
        let first = take_pending(&slot).await;
        if let Some(t) = last_sent {
            let since = t.elapsed();
            if since < DEBOUNCE {
                tokio::time::sleep(DEBOUNCE - since).await;
            }
        }
        // Whatever arrived during the wait supersedes the first (same agent, newer event).
        let mut current = slot.pending.lock().unwrap().take().unwrap_or(first);
        let mut attempt = 0usize;
        // Read the config on every try: cleared or revoked meanwhile = nothing to send to.
        while let Some(cfg) = state.db.wake_config(&project_id).ok().flatten() {
            last_sent = Some(Instant::now());
            let result = send(&cfg, &current).await;
            let last = LastWake {
                at: now_ms(),
                status: result.as_ref().ok().copied(),
                error: result.as_ref().err().cloned(),
            };
            let _ = state.db.record_wake(&project_id, &last);
            match result {
                Ok(status) => {
                    tracing::info!(project = %project_id, status, event = current.event_id, attempt, "wake nudge delivered");
                    break;
                }
                Err(e) => {
                    let Some(delay) = retry_delay(attempt) else {
                        tracing::warn!(project = %project_id, error = %e, event = current.event_id, "wake nudge given up after 24 h");
                        break;
                    };
                    tracing::info!(project = %project_id, error = %e, event = current.event_id, attempt, retry_in_s = delay.as_secs(), "wake nudge failed");
                    attempt += 1;
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = slot.notify.notified() => {}
                    }
                    if let Some(newer) = slot.pending.lock().unwrap().take() {
                        current = newer;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn retry_schedule_is_the_spec_one() {
        let secs: Vec<u64> = (0..8)
            .filter_map(|i| retry_delay(i).map(|d| d.as_secs()))
            .collect();
        assert_eq!(secs, [1, 5, 30, 120, 600, 1800, 3600, 3600]);
        let total: u64 = (0..).map_while(retry_delay).map(|d| d.as_secs()).sum();
        assert!(total >= 24 * 3600, "schedule covers a day: {total}");
        assert!(retry_delay(6 + 24).is_none());
    }

    #[test]
    fn signature_is_hmac_sha256_hex() {
        // RFC 4231 test case 2: key "Jefe", data "what do ya want for nothing?".
        assert_eq!(
            signature("Jefe", "what do ya want for nothing?"),
            "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        let n = Nudge {
            agent: "01A".into(),
            chat: Some("01C".into()),
            event_id: 7,
            kind: "message.received".into(),
        };
        assert_eq!(
            n.body(),
            r#"{"agent":"01A","chat":"01C","event_id":7,"type":"message.received"}"#
        );
        assert_eq!(
            Nudge::test("01A").body(),
            r#"{"agent":"01A","chat":null,"event_id":0,"type":"wake.test"}"#
        );
    }

    #[test]
    fn url_and_header_checks() {
        assert!(check_url("https://hooks.example.com/grok/abc").is_ok());
        assert_eq!(
            check_url("http://hooks.example.com/x"),
            Err("the URL must start with https://")
        );
        assert!(check_url("https://127.0.0.1/x").is_err());
        assert!(check_url("https://localhost/x").is_err());
        assert!(check_url("https://relay.internal/x").is_err());
        assert!(check_url("https://user:pw@hooks.example.com/x").is_err());
        assert!(check_url("not a url").is_err());
        assert!(check_url("").is_err());
        assert_eq!(check_auth("  Bearer abc.def  ").unwrap(), "Bearer abc.def");
        assert!(check_auth("").is_err());
        assert!(check_auth("two\nlines").is_err());
        assert!(check_auth(&"x".repeat(AUTH_MAX + 1)).is_err());
    }

    /// A local catcher receives one nudge with the three headers; the signature verifies with
    /// the stored value; two nudges inside the debounce window reach it as one; a 500 is an
    /// error with the status in it.
    #[tokio::test]
    async fn nudges_arrive_signed_and_coalesced() {
        allow_insecure_for_tests();
        let seen: Arc<Mutex<Vec<(axum::http::HeaderMap, String)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let fail = Arc::new(AtomicBool::new(false));
        let app = {
            let seen = seen.clone();
            let fail = fail.clone();
            axum::Router::new().route(
                "/hook",
                axum::routing::post(move |headers: axum::http::HeaderMap, body: String| {
                    let seen = seen.clone();
                    let fail = fail.clone();
                    async move {
                        seen.lock().unwrap().push((headers, body));
                        if fail.load(Ordering::Relaxed) {
                            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom")
                        } else {
                            (axum::http::StatusCode::OK, "ok")
                        }
                    }
                }),
            )
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let cfg = WakeConfig {
            url: format!("http://{addr}/hook"),
            auth: "Bearer grok-routine-key".into(),
        };

        let n = Nudge {
            agent: "01AGENT".into(),
            chat: Some("01CHAT".into()),
            event_id: 42,
            kind: "message.received".into(),
        };
        assert_eq!(send(&cfg, &n).await, Ok(200));
        {
            let got = seen.lock().unwrap();
            assert_eq!(got.len(), 1);
            let (h, body) = &got[0];
            assert_eq!(h.get("authorization").unwrap(), "Bearer grok-routine-key");
            assert_eq!(h.get("content-type").unwrap(), "application/json");
            assert_eq!(
                h.get("x-arbos-signature").unwrap().to_str().unwrap(),
                signature(&cfg.auth, body)
            );
            let v: Value = serde_json::from_str(body).unwrap();
            assert_eq!(v["agent"], "01AGENT");
            assert_eq!(v["chat"], "01CHAT");
            assert_eq!(v["event_id"], 42);
            assert_eq!(v["type"], "message.received");
            assert!(v.get("text").is_none() && v.get("message").is_none());
        }

        fail.store(true, Ordering::Relaxed);
        let err = send(&cfg, &n).await.unwrap_err();
        assert!(err.starts_with("HTTP 500"), "{err}");
        fail.store(false, Ordering::Relaxed);

        // The worker: two nudges before it runs → one POST, the newer event.
        let state = crate::state::test_support::test_state(crate::state::AuthMode::Github);
        state
            .db
            .insert_outside_agent("01AGENT", "u1", "grok", "h")
            .unwrap();
        state.db.set_wake_url("01AGENT", Some(&cfg.url)).unwrap();
        state.db.set_wake_auth("01AGENT", Some(&cfg.auth)).unwrap();
        seen.lock().unwrap().clear();
        nudge(
            &state,
            Nudge {
                event_id: 1,
                ..n.clone()
            },
        );
        nudge(
            &state,
            Nudge {
                event_id: 2,
                ..n.clone()
            },
        );
        tokio::time::sleep(Duration::from_millis(600)).await;
        {
            let got = seen.lock().unwrap();
            assert_eq!(got.len(), 1, "coalesced into one nudge");
            let v: Value = serde_json::from_str(&got[0].1).unwrap();
            assert_eq!(v["event_id"], 2);
        }
        let agent = state.db.outside_agent("01AGENT").unwrap().unwrap();
        assert_eq!(agent.wake_last.as_ref().and_then(|l| l.status), Some(200));
        assert!(agent.wake_auth_set);
        assert_eq!(agent.wake_url.as_deref(), Some(cfg.url.as_str()));
        // A third one inside the window waits for the window to pass.
        nudge(
            &state,
            Nudge {
                event_id: 3,
                ..n.clone()
            },
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(seen.lock().unwrap().len(), 1, "still inside the 5 s window");
    }
}
