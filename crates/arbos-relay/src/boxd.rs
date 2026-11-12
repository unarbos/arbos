//! The relay's side of `arbos-boxd` (docs/box-scaling.md §4.1 and Phase 2): the `boxd` provider.
//! A box on it runs as an OCI bundle under the host's root daemon; the relay asks the daemon to
//! make it, wake it and put it to sleep, and sleeps it itself after 15 idle minutes (§3.1).
//!
//! Off unless `ARBOS_BOXD_URL` is set. One daemon, on loopback: `boxes.node_id` and a second
//! host are Phase 4, the resident/present admission split and the in-place move of the Docker
//! `runsc` boxes are the rest of Phase 2.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::net::SocketAddrV4;
use std::os::unix::fs::MetadataExt;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::ids::now_ms;
use arbos_core::wire::BoxState;

use crate::boxes;
use crate::db::Sandbox;
use crate::state::AppState;

/// Error text `boxes::stop_box` returns when agentd refused to sleep (a turn or a job is
/// running). The box keeps running.
pub const BUSY: &str = "box is busy";

/// §3.1: a box sleeps after this long with nothing running and nothing due.
const IDLE_MS: i64 = 15 * 60_000;
/// §3.1 condition 3: a routine due this soon keeps the box up.
const ROUTINE_HORIZON_MS: i64 = 20 * 60_000;
/// §4.2: below this much free disk on the daemon's host no new box is made.
const MIN_FREE_DISK_GB: f64 = 80.0;
/// Shorter than this is not a secret anyone generated (the daemon's own rule).
const MIN_TOKEN_LEN: usize = 32;
/// The largest reply read from the daemon. A host status with a thousand boxes is ~150 KB.
const MAX_REPLY: usize = 4 << 20;

/// Where the daemon is and the bearer it asks for.
#[derive(Clone)]
pub struct Boxd {
    url: String,
    token: String,
}

/// Every field the daemon sends is named in these, so a reply of any other shape is refused.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ensured {
    created: bool,
    size_gb: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Warm {
    Warm,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct Woken {
    state: Warm,
    restored: bool,
    box_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Asleep {
    AsleepClean,
    AsleepCkpt,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct Slept {
    state: Asleep,
    files_bytes: Option<u64>,
    scratch_bytes: Option<u64>,
    trash_bytes: Option<u64>,
    work_bytes: u64,
    ckpt_bytes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct HostBox {
    user_id: String,
    running: bool,
    size_gb: u64,
    work_bytes: u64,
    ckpt_bytes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct HostStatus {
    mem_available_gb: Option<f64>,
    swap_used_gb: Option<f64>,
    load5: Option<f64>,
    disk_free_gb: f64,
    resident_boxes: u64,
    present_boxes: u64,
    boxes: Vec<HostBox>,
}

/// The daemon's own word for a refusal, bounded and scrubbed before it reaches a log or a client.
fn refusal(body: &[u8]) -> String {
    #[derive(Deserialize)]
    struct Refusal {
        error: String,
    }
    serde_json::from_slice::<Refusal>(body)
        .map(|r| arbos_core::redact::scrub(&r.error.chars().take(200).collect::<String>()))
        .unwrap_or_default()
}

fn reply<T: DeserializeOwned>(path: &str, status: StatusCode, body: &[u8]) -> Result<T> {
    if status != StatusCode::OK {
        bail!("boxd {path}: {status} {}", refusal(body));
    }
    serde_json::from_slice(body)
        .map_err(|_| anyhow!("boxd {path}: a reply this relay does not know"))
}

/// The path of one box's operation. A user id is one path segment of the daemon's alphabet
/// (`arbos-boxd` `UserId::parse`), or nothing is sent.
fn box_path(user_id: &str, op: &str) -> Result<String> {
    let ok = !user_id.is_empty()
        && user_id.len() <= 64
        && user_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !ok {
        bail!("boxd: {user_id:?} is not a user id the daemon takes");
    }
    Ok(format!("/v1/boxes/{user_id}/{op}"))
}

impl Boxd {
    /// `ARBOS_BOXD_URL` and `ARBOS_BOXD_TOKEN_FILE`. Neither set: the provider is off. A URL
    /// without a usable token file stops the relay rather than run a provider that cannot work.
    pub fn from_env() -> Result<Option<Self>> {
        let var = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        };
        let Some(url) = var("ARBOS_BOXD_URL") else {
            return Ok(None);
        };
        let file = var("ARBOS_BOXD_TOKEN_FILE")
            .context("ARBOS_BOXD_URL is set; ARBOS_BOXD_TOKEN_FILE must name the token file")?;
        Self::new(&url, &file).map(Some)
    }

    /// The bearer goes over plain HTTP, so the daemon must be on loopback (mTLS to another host
    /// is Phase 4). The token file is the relay user's copy of the daemon's, a plain file nobody
    /// but its owner can read.
    fn new(url: &str, token_file: &str) -> Result<Self> {
        let addr = url
            .trim_end_matches('/')
            .strip_prefix("http://")
            .and_then(|a| a.parse::<SocketAddr>().ok())
            .filter(|a| a.ip().is_loopback())
            .with_context(|| {
                format!("ARBOS_BOXD_URL must be http://127.0.0.1:<port>, not {url}")
            })?;
        let md = std::fs::symlink_metadata(token_file)
            .with_context(|| format!("boxd token file {token_file}"))?;
        if !md.is_file() || md.mode() & 0o077 != 0 {
            bail!("boxd token file {token_file} must be a plain file with mode 0600");
        }
        let token = std::fs::read_to_string(token_file)?.trim().to_owned();
        if token.len() < MIN_TOKEN_LEN {
            bail!("boxd token file {token_file} holds fewer than {MIN_TOKEN_LEN} characters");
        }
        Ok(Self {
            url: format!("http://{addr}"),
            token,
        })
    }

    async fn send(
        &self,
        http: &reqwest::Client,
        method: Method,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<(StatusCode, Vec<u8>)> {
        let mut req = http
            .request(method, format!("{}{path}", self.url))
            .bearer_auth(&self.token)
            .timeout(timeout);
        if let Some(body) = body {
            req = req.json(body);
        }
        let mut r = req.send().await.with_context(|| format!("boxd {path}"))?;
        let status = r.status();
        let mut out = Vec::new();
        while let Some(chunk) = r.chunk().await.with_context(|| format!("boxd {path}"))? {
            if out.len() + chunk.len() > MAX_REPLY {
                bail!("boxd {path}: a reply over {MAX_REPLY} bytes");
            }
            out.extend_from_slice(&chunk);
        }
        Ok((status, out))
    }

    /// Make the box's disk and bundle if absent. The first box on a version unpacks its image.
    async fn ensure(
        &self,
        http: &reqwest::Client,
        user_id: &str,
        version: &str,
        env: &BTreeMap<String, String>,
    ) -> Result<()> {
        let path = box_path(user_id, "ensure")?;
        let body = json!({ "version": version, "env": env });
        let (status, bytes) = self
            .send(
                http,
                Method::POST,
                &path,
                Some(&body),
                Duration::from_secs(300),
            )
            .await?;
        let made: Ensured = reply(&path, status, &bytes)?;
        if ![8, 32, 128].contains(&made.size_gb) {
            bail!("boxd {path}: a disk of {} GiB is no tier", made.size_gb);
        }
        tracing::info!(%user_id, created = made.created, size_gb = made.size_gb, "boxd: box ensured");
        Ok(())
    }

    /// Start the box, from its checkpoint if it has one. Returns where agentd listens: the
    /// box's own private address on the daemon's bridge, and nothing else is taken for one,
    /// because the relay sends the box's MCP token there.
    async fn wake(&self, http: &reqwest::Client, user_id: &str) -> Result<String> {
        let path = box_path(user_id, "wake")?;
        let (status, bytes) = self
            .send(http, Method::POST, &path, None, Duration::from_secs(120))
            .await?;
        let woken: Woken = reply(&path, status, &bytes)?;
        let private = woken
            .box_url
            .strip_prefix("http://")
            .and_then(|a| a.parse::<SocketAddrV4>().ok())
            .is_some_and(|a| a.ip().is_private() && a.port() != 0);
        if !private {
            bail!("boxd {path}: the box address is not a private one");
        }
        tracing::info!(%user_id, restored = woken.restored, "boxd: box awake");
        Ok(woken.box_url)
    }

    /// Stop or checkpoint the box and release its disk. `false` is agentd refusing because the
    /// box is busy: nothing happened to it.
    pub async fn sleep(&self, http: &reqwest::Client, user_id: &str) -> Result<bool> {
        let path = box_path(user_id, "sleep")?;
        // agentd gets 50 s to stop cleanly, and a checkpoint writes ~0.8 s per GB after that.
        let (status, bytes) = self
            .send(http, Method::POST, &path, None, Duration::from_secs(180))
            .await?;
        if status == StatusCode::CONFLICT && refusal(&bytes).starts_with("busy") {
            return Ok(false);
        }
        let slept: Slept = reply(&path, status, &bytes)?;
        tracing::info!(%user_id, state = ?slept.state, work_bytes = slept.work_bytes, ckpt_bytes = slept.ckpt_bytes, "boxd: box asleep");
        Ok(true)
    }

    async fn disk_free_gb(&self, http: &reqwest::Client) -> Result<f64> {
        let path = "/v1/host/status";
        let (status, bytes) = self
            .send(http, Method::GET, path, None, Duration::from_secs(30))
            .await?;
        let host: HostStatus = reply(path, status, &bytes)?;
        if !host.disk_free_gb.is_finite() || host.disk_free_gb < 0.0 {
            bail!("boxd {path}: free disk is not a size");
        }
        Ok(host.disk_free_gb)
    }
}

pub fn of(state: &AppState) -> Result<&Boxd> {
    state
        .cfg
        .boxd
        .as_ref()
        .ok_or_else(|| anyhow!("the boxd provider is off: ARBOS_BOXD_URL is not set"))
}

/// Create the user's box on the daemon and start it. The row is written once, when the box is
/// up (`boxes::create_on_provider`, #2262).
pub async fn create_box(
    state: &AppState,
    user_id: &str,
    sb: &mut Sandbox,
    box_token: &str,
    mcp_token: &str,
) -> Result<()> {
    let boxd = of(state)?;
    let free = boxd.disk_free_gb(&state.http).await?;
    if free < MIN_FREE_DISK_GB {
        bail!("boxd host has {free:.0} GB of disk free; no new box below {MIN_FREE_DISK_GB:.0} GB");
    }
    let snapshot = sb
        .snapshot
        .clone()
        .unwrap_or_else(|| state.default_snapshot());
    // `arbos-agent-0.6.409` → `0.6.409`, the image version the operator installed on the host.
    let version = snapshot
        .rsplit_once('-')
        .map(|(_, v)| v)
        .unwrap_or(&snapshot);
    // The id the daemon runs the sandbox under.
    let name = format!("arbos-box-{user_id}");
    sb.sandbox_id = Some(name.clone());
    sb.device_id = format!("box:{name}");
    // Kept as for the other providers: the relay's database holds only the box token's hash, and
    // an image roll (a later phase) has to hand the daemon the environment again.
    boxes::save_local_tokens(state, user_id, box_token, mcp_token)?;
    let env = boxes::box_env(state, user_id, box_token, mcp_token, &state.cfg.public_url);
    boxd.ensure(&state.http, user_id, version, &env).await?;
    sb.snapshot = Some(snapshot);
    start_box(state, user_id, sb).await
}

/// Wake an existing box and record where it listens.
pub async fn start_box(state: &AppState, user_id: &str, sb: &mut Sandbox) -> Result<()> {
    sb.box_url = Some(of(state)?.wake(&state.http, user_id).await?);
    sb.last_started_at = Some(now_ms());
    state.db.upsert_box(sb)?;
    Ok(())
}

// ---- the idle supervisor ---------------------------------------------------------------------

/// What one tick sees of one awake box.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sample {
    /// A keepalive said a turn is running, recently enough to believe.
    pub turn_running: bool,
    /// The user has a routine due within [`ROUTINE_HORIZON_MS`].
    pub routine_soon: bool,
    /// When a client last pushed a row for this box, or 0.
    pub last_client_push_ms: i64,
}

/// One supervisor step for one awake box, pure: `(idle since, sleep now)`. The clock starts at
/// the first quiet tick, restarts at anything that is not quiet, and the box sleeps once it has
/// run for [`IDLE_MS`]. §3.1's fourth condition is half here (the client push) and half in the
/// caller: a box with a wake job is `waking`, and only awake boxes are sampled.
pub fn idle_step(idle_since: Option<i64>, s: &Sample, now: i64) -> (Option<i64>, bool) {
    let since = idle_since.unwrap_or(now);
    if s.turn_running || s.routine_soon || s.last_client_push_ms >= since {
        return (None, false);
    }
    (Some(since), now - since >= IDLE_MS)
}

/// Every minute: sleep each awake `boxd` box that has been idle for 15 minutes. This is what
/// stands in for always-on on these boxes (`boxes::spawn_always_on` leaves them alone).
pub async fn spawn_idle_sleeper(state: AppState) {
    if state.cfg.boxd.is_none() {
        return;
    }
    tokio::spawn(async move {
        // In memory only: a relay restart gives every box a fresh 15 minutes.
        let mut idle_since = HashMap::new();
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            idle_tick(&state, &mut idle_since).await;
        }
    });
}

async fn idle_tick(state: &AppState, idle_since: &mut HashMap<String, i64>) {
    let now = now_ms();
    // Fail closed: when the routines cannot be read, nothing is put to sleep this minute.
    let (Ok(due), Ok(rows)) = (
        state.db.due_routines(now + ROUTINE_HORIZON_MS),
        state.db.list_boxes(),
    ) else {
        return;
    };
    let soon: HashSet<String> = due
        .iter()
        .filter_map(|rt| state.user_of(&rt.project_id))
        .collect();
    let turns = state.turn_running.lock().await.clone();
    let pushes = state.last_client_push.lock().await.clone();
    let mut kept = HashMap::new();
    for b in rows {
        let user_id = b.user_id;
        if b.provider != boxes::Provider::Boxd.as_str()
            || state.box_state(&user_id).await.0 != BoxState::Awake
        {
            continue;
        }
        let sample = Sample {
            turn_running: turns
                .get(&user_id)
                .is_some_and(|t| now - *t < crate::fleet::TURN_FRESH_MS),
            routine_soon: soon.contains(&user_id),
            last_client_push_ms: pushes.get(&user_id).copied().unwrap_or(0),
        };
        let (since, sleep) = idle_step(idle_since.get(&user_id).copied(), &sample, now);
        if !sleep {
            kept.extend(since.map(|t| (user_id, t)));
            continue;
        }
        // agentd has the last word on the first two conditions (a turn, a waiting send, a job):
        // the daemon asks it, and a refusal restarts this box's clock.
        match boxes::stop_box(state, &user_id, false).await {
            Ok(()) => {
                tracing::info!(%user_id, "boxd: idle box put to sleep");
                // A row a client pushed while the box was going down has no box to take it.
                let pushed = state.last_client_push.lock().await.get(&user_id).copied();
                if pushed.is_some_and(|t| t >= now) {
                    boxes::ensure_awake(state, &user_id, "rows pending after box sleep").await;
                }
            }
            Err(e) if e.to_string() == BUSY => {
                tracing::debug!(%user_id, "boxd: idle box is busy after all; not slept");
            }
            Err(e) => tracing::warn!(%user_id, error = %e, "boxd: idle sleep failed"),
        }
    }
    *idle_since = kept;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};

    use axum::extract::{Request, State};
    use axum::response::{IntoResponse, Response};
    use axum::Json;

    use crate::boxes::Provider;
    use crate::people::tests_support::person;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    /// A daemon that answers what `arbos-boxd` answers (its README), unless told otherwise.
    #[derive(Default)]
    struct Fake {
        /// Every authenticated request: path and body.
        calls: Mutex<Vec<(String, Value)>>,
        /// Replies that differ from the daemon's usual one, by the last path segment.
        replies: Mutex<HashMap<&'static str, (u16, Value)>>,
    }

    impl Fake {
        fn answer(&self, op: &'static str, status: u16, body: Value) {
            self.replies.lock().unwrap().insert(op, (status, body));
        }
        fn paths(&self) -> Vec<String> {
            let calls = self.calls.lock().unwrap();
            calls.iter().map(|(p, _)| p.clone()).collect()
        }
    }

    fn usual(op: &str) -> (u16, Value) {
        let body = match op {
            "ensure" => json!({ "created": true, "size_gb": 8 }),
            "wake" => {
                json!({ "state": "warm", "restored": false, "box_url": "http://10.207.0.2:7788" })
            }
            "sleep" => json!({ "state": "asleep_clean", "files_bytes": 1, "scratch_bytes": 2,
                "trash_bytes": null, "work_bytes": 4096, "ckpt_bytes": 0 }),
            "status" => json!({ "mem_available_gb": 400.5, "swap_used_gb": null, "load5": 1.0,
                "disk_free_gb": 900.0, "resident_boxes": 1, "present_boxes": 1,
                "boxes": [{ "user_id": "u_a", "running": true, "size_gb": 8,
                    "work_bytes": 4096, "ckpt_bytes": 0 }] }),
            _ => return (404, json!({ "error": "no such route" })),
        };
        (200, body)
    }

    async fn answer(State(fake): State<Arc<Fake>>, req: Request) -> Response {
        let bearer = req.headers().get("authorization").cloned();
        if bearer.and_then(|v| v.to_str().ok().map(str::to_owned))
            != Some(format!("Bearer {TOKEN}"))
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let path = req.uri().path().to_owned();
        let bytes = axum::body::to_bytes(req.into_body(), 1 << 20)
            .await
            .unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        fake.calls.lock().unwrap().push((path.clone(), body));
        let op = path.rsplit('/').next().unwrap_or_default();
        let told = fake.replies.lock().unwrap().get(op).cloned();
        let (status, body) = told.unwrap_or_else(|| usual(op));
        (StatusCode::from_u16(status).unwrap(), Json(body)).into_response()
    }

    /// The fake on a loopback port, and a client for it.
    async fn served(token: &str) -> (Arc<Fake>, Boxd) {
        let fake = Arc::new(Fake::default());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let app = axum::Router::new()
            .fallback(answer)
            .with_state(fake.clone());
        tokio::spawn(async move { axum::serve(l, app).await });
        let token = token.to_owned();
        (fake, Boxd { url, token })
    }

    /// A relay whose `boxd` provider points at a fake daemon, and one user with a project.
    async fn relay() -> (AppState, Arc<Fake>, String) {
        let (fake, boxd) = served(TOKEN).await;
        let mut state = test_state(AuthMode::Github);
        Arc::get_mut(&mut state.cfg).expect("one holder").boxd = Some(boxd);
        let (user, _) = person(&state, 9700);
        (state, fake, user)
    }

    #[tokio::test]
    async fn the_client_speaks_the_daemons_routes_with_the_bearer() {
        let (fake, boxd) = served(TOKEN).await;
        let http = reqwest::Client::new();
        let env = BTreeMap::from([("A".to_owned(), "b".to_owned())]);
        boxd.ensure(&http, "u_a", "1.2.3", &env).await.unwrap();
        assert_eq!(
            boxd.wake(&http, "u_a").await.unwrap(),
            "http://10.207.0.2:7788"
        );
        assert!(boxd.sleep(&http, "u_a").await.unwrap());
        assert_eq!(boxd.disk_free_gb(&http).await.unwrap(), 900.0);
        let calls = fake.calls.lock().unwrap().clone();
        assert_eq!(
            calls[0],
            (
                "/v1/boxes/u_a/ensure".to_owned(),
                json!({ "version": "1.2.3", "env": { "A": "b" } })
            )
        );
        let paths: Vec<&str> = calls.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths[1..],
            [
                "/v1/boxes/u_a/wake",
                "/v1/boxes/u_a/sleep",
                "/v1/host/status"
            ]
        );

        // The wrong token is refused by the daemon, and the refusal says nothing of either token.
        let (_, stranger) = served("ffffffffffffffffffffffffffffffff").await;
        let refused = stranger.wake(&http, "u_a").await.unwrap_err().to_string();
        assert!(refused.contains("401"), "{refused}");
        assert!(
            !refused.contains(TOKEN) && !refused.contains("ffff"),
            "{refused}"
        );
    }

    /// A user id that is not one path segment never leaves the relay.
    #[tokio::test]
    async fn a_bad_user_id_is_not_sent() {
        let (fake, boxd) = served(TOKEN).await;
        let http = reqwest::Client::new();
        let long = "a".repeat(65);
        for id in ["", "a/b", "..", "a b", "a%2fb", "a.b", long.as_str()] {
            assert!(boxd.wake(&http, id).await.is_err(), "{id:?}");
            assert!(boxd.sleep(&http, id).await.is_err(), "{id:?}");
        }
        assert!(fake.paths().is_empty());
    }

    /// Only the replies the daemon documents are taken: a field nobody defined, a missing one, a
    /// state that is not one, a box address that is not the box's, a size that is not a size.
    #[tokio::test]
    async fn a_reply_of_another_shape_is_refused() {
        let (fake, boxd) = served(TOKEN).await;
        let http = reqwest::Client::new();
        for wake in [
            json!({ "state": "warm", "restored": false, "box_url": "http://10.207.0.2:7788", "x": 1 }),
            json!({ "state": "warm", "restored": false }),
            json!({ "state": "cold", "restored": false, "box_url": "http://10.207.0.2:7788" }),
            json!({ "state": "warm", "restored": "no", "box_url": "http://10.207.0.2:7788" }),
            json!({ "state": "warm", "restored": false, "box_url": "http://1.1.1.1:7788" }),
            json!({ "state": "warm", "restored": false, "box_url": "http://127.0.0.1:7788" }),
            json!({ "state": "warm", "restored": false, "box_url": "https://10.207.0.2:7788" }),
            json!({ "state": "warm", "restored": false, "box_url": "http://10.207.0.2:7788/x" }),
            json!({ "state": "warm", "restored": false, "box_url": "http://evil.example:7788" }),
            json!("warm"),
        ] {
            fake.answer("wake", 200, wake.clone());
            assert!(boxd.wake(&http, "u_a").await.is_err(), "{wake}");
        }
        fake.answer("sleep", 200, json!({ "state": "warm" }));
        assert!(boxd.sleep(&http, "u_a").await.is_err());
        fake.answer("ensure", 200, json!({ "created": true, "size_gb": 9 }));
        let env = BTreeMap::new();
        assert!(boxd.ensure(&http, "u_a", "1.0", &env).await.is_err());
        for disk in [json!(-1.0), json!("900"), Value::Null] {
            let mut status = usual("status").1;
            status["disk_free_gb"] = disk.clone();
            fake.answer("status", 200, status);
            assert!(boxd.disk_free_gb(&http).await.is_err(), "{disk}");
        }
    }

    /// agentd refusing is not an error and not a sleep; any other refusal is an error, with the
    /// daemon's reason cut to a line.
    #[tokio::test]
    async fn busy_is_told_apart_from_every_other_refusal() {
        let (fake, boxd) = served(TOKEN).await;
        let http = reqwest::Client::new();
        fake.answer("sleep", 409, json!({ "error": "busy: a turn is running" }));
        assert!(!boxd.sleep(&http, "u_a").await.unwrap());
        fake.answer("sleep", 404, json!({ "error": "x".repeat(5000) }));
        let e = boxd.sleep(&http, "u_a").await.unwrap_err().to_string();
        assert!(e.contains("404") && e.len() < 300, "{e}");
        fake.answer("sleep", 502, json!({ "error": "agentd did not answer" }));
        assert!(boxd.sleep(&http, "u_a").await.is_err());
        fake.answer("sleep", 409, json!("busy"));
        assert!(boxd.sleep(&http, "u_a").await.is_err());
    }

    /// The relay does not start with a daemon that is not on loopback, or a token file others
    /// can read, a link, or a short token.
    #[test]
    fn the_daemon_must_be_local_and_the_token_file_private() {
        let dir = std::env::temp_dir().join(format!("arbos-relay-boxd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("token");
        let p = path.to_str().unwrap();
        let write = |text: &str, mode: u32| {
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        write(&format!("{TOKEN}\n"), 0o600);
        let boxd = Boxd::new("http://127.0.0.1:7790/", p).unwrap();
        assert_eq!(
            (boxd.url.as_str(), boxd.token.as_str()),
            ("http://127.0.0.1:7790", TOKEN)
        );
        for url in [
            "http://10.0.0.5:7790",
            "https://127.0.0.1:7790",
            "http://localhost:7790",
            "http://127.0.0.1:7790/v1",
            "http://127.0.0.1",
            "http://user@127.0.0.1:7790",
        ] {
            assert!(Boxd::new(url, p).is_err(), "{url}");
        }
        write(TOKEN, 0o640);
        assert!(
            Boxd::new("http://127.0.0.1:7790", p).is_err(),
            "group can read it"
        );
        write("short", 0o600);
        assert!(Boxd::new("http://127.0.0.1:7790", p).is_err());
        write(TOKEN, 0o600);
        let link = dir.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(Boxd::new("http://127.0.0.1:7790", link.to_str().unwrap()).is_err());
        assert!(Boxd::new(
            "http://127.0.0.1:7790",
            dir.join("missing").to_str().unwrap()
        )
        .is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_box_sleeps_after_fifteen_quiet_minutes_and_anything_restarts_the_clock() {
        let quiet = Sample::default();
        let t0 = 1_000_000;
        // The first quiet tick starts the clock; the box sleeps at the tick 15 minutes later.
        assert_eq!(idle_step(None, &quiet, t0), (Some(t0), false));
        assert_eq!(
            idle_step(Some(t0), &quiet, t0 + IDLE_MS - 1),
            (Some(t0), false)
        );
        assert_eq!(idle_step(Some(t0), &quiet, t0 + IDLE_MS), (Some(t0), true));
        // A turn, a routine inside the horizon, or a client row since the clock started: no
        // sleep, and the 15 minutes start over.
        let late = t0 + IDLE_MS;
        for busy in [
            Sample {
                turn_running: true,
                ..quiet
            },
            Sample {
                routine_soon: true,
                ..quiet
            },
            Sample {
                last_client_push_ms: t0,
                ..quiet
            },
            Sample {
                last_client_push_ms: late,
                ..quiet
            },
        ] {
            assert_eq!(idle_step(Some(t0), &busy, late), (None, false), "{busy:?}");
        }
        // A row from before the clock started does not hold the box up.
        let old = Sample {
            last_client_push_ms: t0 - 1,
            ..quiet
        };
        assert_eq!(idle_step(Some(t0), &old, late), (Some(t0), true));
        assert_eq!(idle_step(None, &old, late), (Some(late), false));
    }

    /// Create, stop and start through the same `boxes` calls every provider goes through.
    #[tokio::test]
    async fn a_box_on_the_provider_is_made_slept_and_woken_through_the_daemon() {
        let (state, fake, user) = relay().await;
        let sb = boxes::create_box(&state, &user, Provider::Boxd, Some("arbos-agent-1.2.3"))
            .await
            .unwrap();
        assert_eq!(sb.provider, "boxd");
        let row = state.db.user_box(&user).unwrap().expect("row");
        assert_eq!(row.box_url.as_deref(), Some("http://10.207.0.2:7788"));
        assert_eq!(row.sandbox_id, Some(format!("arbos-box-{user}")));
        assert_eq!(row.snapshot.as_deref(), Some("arbos-agent-1.2.3"));
        let ensure = fake.calls.lock().unwrap()[1].clone();
        assert_eq!(ensure.0, format!("/v1/boxes/{user}/ensure"));
        assert_eq!(ensure.1["version"], "1.2.3");
        assert_eq!(ensure.1["env"]["ARBOS_USER_ID"], user.as_str());
        assert!(ensure.1["env"]["ARBOS_BOX_TOKEN"].is_string());

        state.set_box_state(&user, BoxState::Awake, None).await;
        fake.answer("sleep", 409, json!({ "error": "busy: a turn is running" }));
        let busy = boxes::stop_box(&state, &user, false).await.unwrap_err();
        assert_eq!(busy.to_string(), BUSY);
        assert_eq!(state.box_state(&user).await.0, BoxState::Awake);
        fake.replies.lock().unwrap().clear();
        boxes::stop_box(&state, &user, false).await.unwrap();
        assert_eq!(state.box_state(&user).await.0, BoxState::Asleep);
        assert!(state
            .db
            .user_box(&user)
            .unwrap()
            .unwrap()
            .last_stopped_at
            .is_some());

        boxes::start_box(&state, &user).await.unwrap();
        let op = |p: &String| p.rsplit('/').next().unwrap().to_owned();
        let ops: Vec<String> = fake.paths().iter().map(op).collect();
        assert_eq!(ops, ["status", "ensure", "wake", "sleep", "sleep", "wake"]);
    }

    /// A full disk, a daemon that refuses, or no daemon configured at all: no box, and no row
    /// left behind to be woken later.
    #[tokio::test]
    async fn a_box_that_cannot_be_made_leaves_no_row() {
        let (mut state, fake, user) = relay().await;
        let mut full = usual("status").1;
        full["disk_free_gb"] = json!(79.9);
        fake.answer("status", 200, full);
        let e = boxes::create_box(&state, &user, Provider::Boxd, None).await;
        assert!(e.unwrap_err().to_string().contains("disk"));
        assert_eq!(fake.paths(), ["/v1/host/status"]);
        assert!(state.db.user_box(&user).unwrap().is_none());

        fake.replies.lock().unwrap().clear();
        fake.answer(
            "ensure",
            409,
            json!({ "error": "version 9 is not installed" }),
        );
        let e = boxes::create_box(&state, &user, Provider::Boxd, None).await;
        assert!(e.unwrap_err().to_string().contains("not installed"));
        assert!(state.db.user_box(&user).unwrap().is_none());

        Arc::get_mut(&mut state.cfg).expect("one holder").boxd = None;
        let e = boxes::create_box(&state, &user, Provider::Boxd, None).await;
        assert!(e.unwrap_err().to_string().contains("ARBOS_BOXD_URL"));
        assert!(state.db.user_box(&user).unwrap().is_none());
    }

    /// One supervisor tick: the idle box sleeps, the one mid-turn does not and loses its clock,
    /// and a box agentd calls busy stays awake.
    #[tokio::test]
    async fn the_supervisor_sleeps_only_the_idle_box() {
        let (state, fake, user) = relay().await;
        boxes::create_box(&state, &user, Provider::Boxd, None)
            .await
            .unwrap();
        state.set_box_state(&user, BoxState::Awake, None).await;
        let long_ago = now_ms() - IDLE_MS - 1;
        let clock = || HashMap::from([(user.clone(), long_ago)]);
        let sleeps = || {
            fake.paths()
                .iter()
                .filter(|p| p.ends_with("/sleep"))
                .count()
        };

        let mut idle_since = HashMap::new();
        idle_tick(&state, &mut idle_since).await;
        assert!(idle_since.contains_key(&user), "the clock starts");
        assert_eq!(sleeps(), 0);

        state
            .turn_running
            .lock()
            .await
            .insert(user.clone(), now_ms());
        let mut idle_since = clock();
        idle_tick(&state, &mut idle_since).await;
        assert!(idle_since.is_empty());
        assert_eq!(sleeps(), 0);
        state.turn_running.lock().await.clear();

        fake.answer("sleep", 409, json!({ "error": "busy: a job is running" }));
        let mut idle_since = clock();
        idle_tick(&state, &mut idle_since).await;
        assert!(idle_since.is_empty());
        assert_eq!(state.box_state(&user).await.0, BoxState::Awake);
        fake.replies.lock().unwrap().clear();

        let mut idle_since = clock();
        idle_tick(&state, &mut idle_since).await;
        assert_eq!(sleeps(), 2);
        assert_eq!(state.box_state(&user).await.0, BoxState::Asleep);
        // Asleep boxes are not sampled, so nothing asks the daemon again.
        idle_tick(&state, &mut clock()).await;
        assert_eq!(sleeps(), 2);
    }
}
