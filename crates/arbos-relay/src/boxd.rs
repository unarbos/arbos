//! The relay's side of `arbos-boxd` (docs/box-scaling.md §4.1 and Phase 2): the `boxd` provider.
//! A box on it runs as an OCI bundle under the host's root daemon; the relay asks the daemon to
//! make it, wake it and put it to sleep, and sleeps it itself after 15 idle minutes (§3.1).
//!
//! Off unless `ARBOS_BOXD_URL` is set. One daemon, on loopback, written to `boxes.node_id` as
//! [`NODE`]: a second host is Phase 4, and the in-place move of the Docker `runsc` boxes is an operator's step.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::net::SocketAddrV4;
use std::os::unix::fs::MetadataExt;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
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
/// §4.2: an asleep box is woken this long before its routine is due, so the run finds it up.
const ROUTINE_HEAD_START_MS: i64 = 60_000;
/// §4.2: below this much free disk on the daemon's host no new box is made.
const MIN_FREE_DISK_GB: f64 = 80.0;
/// §4.2 `max_resident_boxes`: awake and waking boxes on the spec's 503 GB host.
/// `ARBOS_BOXD_MAX_RESIDENT` sets it for another one.
const MAX_RESIDENT: usize = 160;
/// Error text of a wake on a full host where no awake box could be put to sleep to make room.
pub const FULL: &str = "the host is full and every machine on it is busy";
/// `boxes.node_id` of every box: the one daemon, on this host (§4.1 "Node 0").
const NODE: &str = "node-0";
/// Shorter than this is not a secret anyone generated (the daemon's own rule).
const MIN_TOKEN_LEN: usize = 32;
/// The largest reply read from the daemon. A host status with a thousand boxes is ~150 KB.
const MAX_REPLY: usize = 4 << 20;

/// Where the daemon is and the bearer it asks for.
#[derive(Clone)]
pub struct Boxd {
    url: String,
    token: String,
    max_resident: usize,
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

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Asleep {
    AsleepClean,
    AsleepCkpt,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
        let mut boxd = Self::new(&url, &file)?;
        if let Some(n) = var("ARBOS_BOXD_MAX_RESIDENT") {
            boxd.max_resident = n
                .parse()
                .ok()
                .filter(|n| *n > 0)
                .with_context(|| format!("ARBOS_BOXD_MAX_RESIDENT must be a count, not {n}"))?;
        }
        Ok(Some(boxd))
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
            max_resident: MAX_RESIDENT,
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

    /// Stop or checkpoint the box and release its disk. `None` is agentd refusing because the
    /// box is busy: nothing happened to it. With `force` agentd is not asked and nothing that
    /// runs in the box is kept.
    async fn sleep(
        &self,
        http: &reqwest::Client,
        user_id: &str,
        force: bool,
    ) -> Result<Option<Slept>> {
        let mut path = box_path(user_id, "sleep")?;
        if force {
            path.push_str("?force=true");
        }
        // agentd gets 50 s to stop cleanly, and a checkpoint writes ~0.8 s per GB after that.
        let (status, bytes) = self
            .send(http, Method::POST, &path, None, Duration::from_secs(180))
            .await?;
        if status == StatusCode::CONFLICT && refusal(&bytes).starts_with("busy") {
            return Ok(None);
        }
        let slept: Slept = reply(&path, status, &bytes)?;
        tracing::info!(%user_id, state = ?slept.state, work_bytes = slept.work_bytes, ckpt_bytes = slept.ckpt_bytes, "boxd: box asleep");
        Ok(Some(slept))
    }

    async fn host(&self, http: &reqwest::Client) -> Result<HostStatus> {
        let path = "/v1/host/status";
        let (status, bytes) = self
            .send(http, Method::GET, path, None, Duration::from_secs(30))
            .await?;
        let host: HostStatus = reply(path, status, &bytes)?;
        let size = |gb: f64| gb.is_finite() && gb >= 0.0;
        if !size(host.disk_free_gb) || !host.mem_available_gb.is_none_or(size) {
            bail!("boxd {path}: free disk or memory is not a size");
        }
        Ok(host)
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
    let free = boxd.host(&state.http).await?.disk_free_gb;
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
    make_room(state, user_id).await?;
    // A box being made has no row yet, and this writes nothing.
    record(state, user_id, "waking", None);
    let woken = of(state)?.wake(&state.http, user_id).await;
    if woken.is_err() {
        record(state, user_id, "error", None);
    }
    sb.box_url = Some(woken?);
    sb.last_started_at = Some(now_ms());
    state.db.upsert_box(sb)?;
    record(state, user_id, "warm", None);
    Ok(())
}

/// Put a box to sleep and keep what the daemon said of it: how it sleeps, and its storage split
/// (§3.3: the Machine card is to be served from this without waking the box). [`BUSY`] when
/// agentd refused and the box keeps running, which `force` (the user's Stop, an operator's
/// delete) does not let it do.
pub async fn stop_box(state: &AppState, user_id: &str, force: bool) -> Result<()> {
    let Some(slept) = of(state)?.sleep(&state.http, user_id, force).await? else {
        bail!(BUSY);
    };
    // The split is agentd's: there is none from a box that was not running to be asked.
    let storage = slept.files_bytes.map(|files| {
        json!({ "files_bytes": files, "scratch_bytes": slept.scratch_bytes,
            "trash_bytes": slept.trash_bytes, "work_bytes": slept.work_bytes,
            "ckpt_bytes": slept.ckpt_bytes, "at": now_ms() })
        .to_string()
    });
    let lifecycle = serde_json::to_value(slept.state)?;
    record(
        state,
        user_id,
        lifecycle.as_str().unwrap_or("error"),
        storage.as_deref(),
    );
    Ok(())
}

/// `boxes.lifecycle_state`: where the daemon last left the box. It is a record of what the
/// daemon said, not what is counted or acted on: a box that died by itself still reads `warm`
/// here until its next wake or sleep, and the relay's own view of it (`AppState::box_state`)
/// is what admission and the idle supervisor go by.
fn record(state: &AppState, user_id: &str, lifecycle: &str, storage_json: Option<&str>) {
    if let Err(e) = state
        .db
        .set_box_lifecycle(user_id, NODE, lifecycle, storage_json)
    {
        tracing::warn!(%user_id, lifecycle, error = %e, "boxd: lifecycle state not written");
    }
}

// ---- admission (§4.2) ------------------------------------------------------------------------

/// The `boxd` half of `fleet::Capacity`. `resident` boxes hold memory and are capped; `present`
/// ones hold only disk, and free disk is all that gates a new box.
#[derive(Debug, Clone, Serialize)]
pub struct Capacity {
    pub max_resident: usize,
    /// Awake and waking boxes, as the relay sees them.
    pub resident: usize,
    /// Boxes with a disk on the host, asleep or not, as the daemon counts them.
    pub present: u64,
    pub mem_available_gb: Option<f64>,
    pub disk_free_gb: f64,
    /// A new box may be made right now.
    pub has_room: bool,
}

/// `None` when the provider is off, or its daemon gave no answer to count from.
pub async fn capacity(state: &AppState) -> Option<Capacity> {
    let boxd = state.cfg.boxd.as_ref()?;
    let host = boxd
        .host(&state.http)
        .await
        .inspect_err(|e| tracing::warn!(error = %e, "boxd: no host status"))
        .ok()?;
    Some(Capacity {
        max_resident: boxd.max_resident,
        resident: resident(state, "").await,
        present: host.present_boxes,
        mem_available_gb: host.mem_available_gb,
        disk_free_gb: host.disk_free_gb,
        has_room: host.disk_free_gb >= MIN_FREE_DISK_GB,
    })
}

/// `boxd` boxes that are awake or waking, not counting `except` (the one about to wake).
async fn resident(state: &AppState, except: &str) -> usize {
    let mut n = 0;
    // Fail closed: boxes that cannot be counted leave no room.
    let Ok(rows) = state.db.list_boxes() else {
        return usize::MAX;
    };
    for b in rows {
        if b.provider == boxes::Provider::Boxd.as_str()
            && b.user_id != except
            && matches!(
                state.box_state(&b.user_id).await.0,
                BoxState::Awake | BoxState::Waking
            )
        {
            n += 1;
        }
    }
    n
}

/// The resident gate, pure: under the cap, and the host above its memory floor where the daemon
/// can tell (it cannot off Linux).
pub fn room_to_wake(resident: usize, max: usize, mem_gb: Option<f64>, min_mem_gb: f64) -> bool {
    resident < max && mem_gb.is_none_or(|m| m >= min_mem_gb)
}

/// §4.2: a box that wakes on a full host does not wait in a line; the least-recently-active
/// awake box with no turn running is put to sleep first. One out, one in.
async fn make_room(state: &AppState, user_id: &str) -> Result<()> {
    let boxd = of(state)?;
    let host = boxd.host(&state.http).await?;
    // The memory floor is the one the gVisor fleet evicts at (`ARBOS_HOST_EVICT_MIN_AVAILABLE_GB`).
    if room_to_wake(
        resident(state, user_id).await,
        boxd.max_resident,
        host.mem_available_gb,
        state.cfg.fleet.evict_min_available_gb,
    ) {
        return Ok(());
    }
    let now = now_ms();
    let mut idle = crate::fleet::candidates(state, boxes::Provider::Boxd).await;
    idle.retain(|c| c.user_id != user_id);
    while let Some(victim) = crate::fleet::pick_eviction(&idle) {
        match sleep_box(state, &victim, now).await {
            Ok(()) => {
                tracing::info!(user_id = %victim, for_user = %user_id, "boxd: host full; idle box put to sleep to make room");
                return Ok(());
            }
            // agentd knows of a job or a send the relay does not: the next one.
            Err(e) if e.to_string() == BUSY => idle.retain(|c| c.user_id != victim),
            Err(e) => return Err(e),
        }
    }
    // Owner's call: with nothing to sleep the wake is refused (and retried by the wake job)
    // rather than let past the cap.
    bail!(FULL)
}

/// Sleep one box, and wake it again if a client pushed a row for it since `since`: that row has
/// no box to take it.
async fn sleep_box(state: &AppState, user_id: &str, since: i64) -> Result<()> {
    boxes::stop_box(state, user_id, false).await?;
    let pushed = state.last_client_push.lock().await.get(user_id).copied();
    if pushed.is_some_and(|t| t >= since) {
        boxes::ensure_awake(state, user_id, "rows pending after box sleep").await;
    }
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
        match sleep_box(state, &user_id, now).await {
            Ok(()) => tracing::info!(%user_id, "boxd: idle box put to sleep"),
            Err(e) if e.to_string() == BUSY => {
                tracing::debug!(%user_id, "boxd: idle box is busy after all; not slept");
            }
            Err(e) => tracing::warn!(%user_id, error = %e, "boxd: idle sleep failed"),
        }
    }
    *idle_since = kept;
}

/// The routine lookahead (§4.2), on the routine scheduler's tick: wake each asleep `boxd` box
/// whose user has a routine due within [`ROUTINE_HEAD_START_MS`]. The same routine then holds
/// it up (`Sample::routine_soon`). The five-minute prefetch of a cold box is Phase 3.
pub async fn wake_ahead_of_routines(state: &AppState, now: i64) {
    if state.cfg.boxd.is_none() {
        return;
    }
    let Ok(due) = state.db.due_routines(now + ROUTINE_HEAD_START_MS) else {
        return;
    };
    let users: HashSet<String> = due
        .iter()
        .filter_map(|rt| state.user_of(&rt.project_id))
        .collect();
    for user_id in users {
        let on_boxd = matches!(state.db.user_box(&user_id), Ok(Some(sb)) if sb.provider == boxes::Provider::Boxd.as_str());
        if on_boxd && state.box_state(&user_id).await.0 == BoxState::Asleep {
            tracing::info!(%user_id, "boxd: waking ahead of a routine");
            boxes::ensure_awake(state, &user_id, "routine lookahead").await;
        }
    }
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
        /// Every authenticated request: path (and query, if any) and body.
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
        let req_uri = req.uri().to_string();
        let bytes = axum::body::to_bytes(req.into_body(), 1 << 20)
            .await
            .unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        fake.calls.lock().unwrap().push((req_uri, body));
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
        let boxd = Boxd {
            url,
            token: token.to_owned(),
            max_resident: MAX_RESIDENT,
        };
        (fake, boxd)
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
        assert!(boxd.sleep(&http, "u_a", false).await.unwrap().is_some());
        assert_eq!(boxd.host(&http).await.unwrap().disk_free_gb, 900.0);
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
            assert!(boxd.sleep(&http, id, false).await.is_err(), "{id:?}");
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
        assert!(boxd.sleep(&http, "u_a", false).await.is_err());
        fake.answer("ensure", 200, json!({ "created": true, "size_gb": 9 }));
        let env = BTreeMap::new();
        assert!(boxd.ensure(&http, "u_a", "1.0", &env).await.is_err());
        for disk in [json!(-1.0), json!("900"), Value::Null] {
            let mut status = usual("status").1;
            status["disk_free_gb"] = disk.clone();
            fake.answer("status", 200, status);
            assert!(boxd.host(&http).await.is_err(), "{disk}");
        }
        let mut status = usual("status").1;
        status["mem_available_gb"] = json!(-1.0);
        fake.answer("status", 200, status);
        assert!(boxd.host(&http).await.is_err());
    }

    /// agentd refusing is not an error and not a sleep; any other refusal is an error, with the
    /// daemon's reason cut to a line.
    #[tokio::test]
    async fn busy_is_told_apart_from_every_other_refusal() {
        let (fake, boxd) = served(TOKEN).await;
        let http = reqwest::Client::new();
        fake.answer("sleep", 409, json!({ "error": "busy: a turn is running" }));
        assert!(boxd.sleep(&http, "u_a", false).await.unwrap().is_none());
        fake.answer("sleep", 404, json!({ "error": "x".repeat(5000) }));
        let e = boxd
            .sleep(&http, "u_a", false)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("404") && e.len() < 300, "{e}");
        fake.answer("sleep", 502, json!({ "error": "agentd did not answer" }));
        assert!(boxd.sleep(&http, "u_a", false).await.is_err());
        fake.answer("sleep", 409, json!("busy"));
        assert!(boxd.sleep(&http, "u_a", false).await.is_err());
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
        // The host is asked before a create (its disk) and before every wake (its room).
        assert_eq!(
            ops,
            ["status", "ensure", "status", "wake", "sleep", "sleep", "status", "wake"]
        );
    }

    /// The row says where the daemon last left the box, and keeps the storage split of the last
    /// sleep that had one. Boxes of other providers say nothing of the kind.
    #[tokio::test]
    async fn the_row_keeps_the_lifecycle_state_and_the_last_storage_split() {
        let (state, fake, user) = relay().await;
        let at = |state: &AppState| state.db.box_lifecycle(&user).unwrap();
        boxes::create_box(&state, &user, Provider::Boxd, None)
            .await
            .unwrap();
        assert_eq!(at(&state), Some(("node-0".into(), "warm".into(), None)));

        state.set_box_state(&user, BoxState::Awake, None).await;
        fake.answer("sleep", 409, json!({ "error": "busy: a turn is running" }));
        boxes::stop_box(&state, &user, false).await.unwrap_err();
        assert_eq!(at(&state).unwrap().1, "warm", "a busy box did not sleep");

        fake.answer(
            "sleep",
            200,
            json!({ "state": "asleep_ckpt", "files_bytes": 5, "scratch_bytes": 6,
                "trash_bytes": null, "work_bytes": 4096, "ckpt_bytes": 7 }),
        );
        boxes::stop_box(&state, &user, false).await.unwrap();
        let shown = boxes::provider_status(&state, &user).await;
        assert_eq!(shown["lifecycle_state"], "asleep_ckpt");
        assert_eq!(shown["node_id"], "node-0");
        let mut storage = shown["storage"].clone();
        assert!(storage["at"].take().as_i64().unwrap() > 0);
        assert_eq!(
            storage,
            json!({ "files_bytes": 5, "scratch_bytes": 6, "trash_bytes": null,
                "work_bytes": 4096, "ckpt_bytes": 7, "at": null })
        );

        // A wake the daemon refuses, then one it takes, then the sleep of a box that was not
        // running: no agentd to measure a split, so the one before is kept.
        fake.answer("wake", 500, json!({ "error": "runsc restore failed" }));
        boxes::start_box(&state, &user).await.unwrap_err();
        assert_eq!(at(&state).unwrap().1, "error");
        fake.replies.lock().unwrap().remove("wake");
        boxes::start_box(&state, &user).await.unwrap();
        assert_eq!(at(&state).unwrap().1, "warm");
        fake.answer(
            "sleep",
            200,
            json!({ "state": "asleep_clean", "files_bytes": null, "scratch_bytes": null,
                "trash_bytes": null, "work_bytes": 8192, "ckpt_bytes": 0 }),
        );
        boxes::stop_box(&state, &user, false).await.unwrap();
        let (_, lifecycle, kept) = at(&state).unwrap();
        assert_eq!(lifecycle, "asleep_clean");
        let kept: Value = serde_json::from_str(&kept.unwrap()).unwrap();
        assert_eq!(
            (&kept["files_bytes"], &kept["work_bytes"]),
            (&json!(5), &json!(4096))
        );

        // The row a relay writes for any box leaves all of it alone, and a box of another
        // provider shows what it showed before.
        let mut row = state.db.user_box(&user).unwrap().unwrap();
        state.db.upsert_box(&row).unwrap();
        assert_eq!(at(&state).unwrap().1, "asleep_clean");
        row.user_id = "u_gvisor".into();
        row.provider = "gvisor".into();
        state.db.upsert_box(&row).unwrap();
        let other = boxes::provider_status(&state, "u_gvisor").await;
        assert!(other.get("lifecycle_state").is_none() && other.get("node_id").is_none());
    }

    /// The three columns arrive on a database from before them without touching its rows, the
    /// statements that relay ran still run on it afterwards, and opening it again changes nothing.
    #[test]
    fn the_lifecycle_columns_are_added_to_an_old_database_and_an_old_relay_still_reads_it() {
        let dir = std::env::temp_dir().join(format!("arbos-relay-boxd-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("relay.db");
        // What `write_box` and `user_box` ran before this change, column for column.
        const OLD_WRITE: &str = "INSERT INTO boxes(user_id, provider, sandbox_id, daytona_state, device_id, box_token_hash, mcp_token, snapshot,
                last_started_at, last_stopped_at, last_seen_at, box_url, preview_token, local_pid, migrated_from)
             VALUES (?1,'gvisor','s',NULL,'box:s','h','m',NULL,1,NULL,NULL,NULL,NULL,NULL,'[]')";
        const OLD_READ: &str = "SELECT user_id, provider, sandbox_id, daytona_state, device_id, box_token_hash, mcp_token, snapshot,
                last_started_at, last_stopped_at, last_seen_at, box_url, preview_token, local_pid, migrated_from FROM boxes";
        drop(crate::db::Db::open(&path).unwrap());
        let old = rusqlite::Connection::open(&path).unwrap();
        for col in ["node_id", "lifecycle_state", "storage_json"] {
            old.execute(&format!("ALTER TABLE boxes DROP COLUMN {col}"), [])
                .unwrap();
        }
        old.execute(OLD_WRITE, ["u_before"]).unwrap();
        drop(old);

        for _ in 0..2 {
            let db = crate::db::Db::open(&path).expect("migrate");
            assert_eq!(db.user_box("u_before").unwrap().unwrap().provider, "gvisor");
            assert_eq!(db.box_lifecycle("u_before").unwrap(), None);
        }
        let db = crate::db::Db::open(&path).unwrap();
        db.set_box_lifecycle("u_before", NODE, "warm", Some("{}"))
            .unwrap();
        db.set_box_lifecycle("u_nobody", NODE, "warm", None)
            .unwrap();
        drop(db);
        let old = rusqlite::Connection::open(&path).unwrap();
        old.execute(OLD_WRITE, ["u_after"]).unwrap();
        let rows = old
            .prepare(OLD_READ)
            .unwrap()
            .query_map([], |_| Ok(()))
            .unwrap()
            .count();
        assert_eq!(rows, 2);
        drop(old);
        let db = crate::db::Db::open(&path).unwrap();
        assert_eq!(db.box_lifecycle("u_after").unwrap(), None);
        assert_eq!(
            db.box_lifecycle("u_before").unwrap(),
            Some((NODE.into(), "warm".into(), Some("{}".into())))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `DELETE /admin/users/{id}/box` is the admin's alone, stops the box for good on the host
    /// (forced: a busy box too, and no checkpoint kept) and only then drops the row. A box the
    /// daemon did not stop keeps its row, or nothing could stop it afterwards.
    #[tokio::test]
    async fn an_admin_delete_stops_the_box_on_the_host_before_the_row_goes() {
        use axum::extract::Path;
        let (state, fake, user) = relay().await;
        let (_, device) = person(&state, 9701);
        boxes::create_box(&state, &user, Provider::Boxd, None)
            .await
            .unwrap();
        state.set_box_state(&user, BoxState::Awake, None).await;
        let delete =
            |who| crate::api::admin_delete_user_box(State(state.clone()), who, Path(user.clone()));
        let before = fake.paths().len();

        let refused = delete(device).await.unwrap_err();
        assert_eq!(refused, (StatusCode::FORBIDDEN, "admin only".to_owned()));
        assert_eq!(
            fake.paths().len(),
            before,
            "nothing was asked of the daemon"
        );

        fake.answer("sleep", 500, json!({ "error": "umount failed" }));
        let failed = delete(crate::auth::Principal::Admin).await.unwrap_err();
        assert_eq!(failed.0, StatusCode::BAD_GATEWAY);
        assert!(state.db.user_box(&user).unwrap().is_some(), "the row stays");

        fake.replies.lock().unwrap().clear();
        delete(crate::auth::Principal::Admin).await.unwrap();
        assert!(state.db.user_box(&user).unwrap().is_none());
        let forced = format!("/v1/boxes/{user}/sleep?force=true");
        assert_eq!(fake.paths()[before..], [forced.clone(), forced]);
        // The idle supervisor and a box making room never force.
        assert!(!fake.paths()[..before].iter().any(|p| p.contains("force")));
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

    #[test]
    fn a_wake_has_room_under_the_cap_and_above_the_memory_floor() {
        assert!(room_to_wake(159, 160, Some(64.0), 64.0));
        assert!(!room_to_wake(160, 160, Some(400.0), 64.0), "the cap");
        assert!(!room_to_wake(0, 160, Some(63.9), 64.0), "the memory floor");
        assert!(room_to_wake(0, 160, None, 64.0), "no /proc to read");
        assert!(!room_to_wake(usize::MAX, 160, None, 64.0), "uncounted");
    }

    /// A relay with no `boxd` provider counts what it counted before: the gVisor rows, and
    /// nothing new in what `GET /admin/fleet` shows. With the provider on, a `boxd` box is not
    /// one of those rows; it is counted on its own, awake or present.
    #[tokio::test]
    async fn capacity_without_the_provider_is_what_it_was() {
        let (state, _fake, user) = relay().await;
        let off = test_state(AuthMode::Github);
        for st in [&state, &off] {
            let mut row = Sandbox {
                user_id: "u_gvisor".into(),
                provider: "gvisor".into(),
                sandbox_id: None,
                daytona_state: None,
                device_id: "box:g".into(),
                box_token_hash: "h".into(),
                mcp_token: "m".into(),
                snapshot: None,
                last_started_at: None,
                last_stopped_at: None,
                last_seen_at: None,
                box_url: None,
                preview_token: None,
                local_pid: None,
                migrated_from: Vec::new(),
            };
            st.db.upsert_box(&row).unwrap();
            row.device_id = "box:b".into();
            row.user_id = user.clone();
            row.provider = "boxd".into();
            st.db.upsert_box(&row).unwrap();
            st.set_box_state(&user, BoxState::Waking, None).await;
        }
        let cap = crate::fleet::capacity(&off).await;
        assert_eq!((cap.resident, cap.max_boxes), (1, 80));
        assert!(cap.boxd.is_none());
        let shown = serde_json::to_value(&cap).unwrap();
        let mut keys: Vec<&str> = shown.as_object().unwrap().keys().map(|k| &**k).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "disk_free_gb",
                "has_room",
                "max_boxes",
                "min_free_disk_gb",
                "resident",
                "waitlist"
            ]
        );

        let cap = crate::fleet::capacity(&state).await;
        assert_eq!((cap.resident, cap.max_boxes), (1, 80));
        let boxd = cap.boxd.expect("the provider is on");
        assert_eq!(
            (boxd.resident, boxd.present, boxd.max_resident),
            (1, 1, 160)
        );
        assert!(boxd.has_room);
        state.set_box_state(&user, BoxState::Asleep, None).await;
        assert_eq!(capacity(&state).await.unwrap().resident, 0);
    }

    /// At the cap a waking box takes the place of the least recently active awake one; a box
    /// mid-turn is never the one, and with nothing to sleep the wake is refused.
    #[tokio::test]
    async fn a_wake_on_a_full_host_sleeps_the_least_recently_active_idle_box() {
        let (mut state, fake, a) = relay().await;
        let cfg = Arc::get_mut(&mut state.cfg).expect("one holder");
        cfg.boxd.as_mut().unwrap().max_resident = 2;
        let (b, _) = person(&state, 9701);
        let (c, _) = person(&state, 9702);
        let sleeps = |user: &str| {
            let path = format!("/v1/boxes/{user}/sleep");
            fake.paths().iter().filter(|p| **p == path).count()
        };
        for user in [&a, &b] {
            boxes::create_box(&state, user, Provider::Boxd, None)
                .await
                .unwrap();
            state.set_box_state(user, BoxState::Awake, None).await;
        }
        assert_eq!(
            sleeps(&a) + sleeps(&b),
            0,
            "two boxes fit under a cap of two"
        );
        // `a` was used last, so `b` is the one that goes.
        let mut pushes = state.last_client_push.lock().await;
        pushes.insert(a.clone(), now_ms() + 60_000);
        drop(pushes);
        boxes::create_box(&state, &c, Provider::Boxd, None)
            .await
            .unwrap();
        assert_eq!((sleeps(&a), sleeps(&b)), (0, 1));
        assert_eq!(state.box_state(&b).await.0, BoxState::Asleep);
        assert_eq!(state.box_state(&a).await.0, BoxState::Awake);
        state.set_box_state(&c, BoxState::Awake, None).await;

        // `a` is the least recently active now only if its turn is not counted: it is running
        // one, so `c` goes instead.
        state.last_client_push.lock().await.clear();
        let mut row = state.db.user_box(&a).unwrap().unwrap();
        row.last_started_at = Some(1);
        row.last_seen_at = None;
        state.db.upsert_box(&row).unwrap();
        state.turn_running.lock().await.insert(a.clone(), now_ms());
        boxes::start_box(&state, &b).await.unwrap();
        assert_eq!((sleeps(&a), sleeps(&c)), (0, 1));
        state.set_box_state(&b, BoxState::Awake, None).await;

        // Both awake boxes are busy, one by the relay's account and one by agentd's: `c` stays
        // asleep, and the daemon is not asked to wake it.
        fake.answer("sleep", 409, json!({ "error": "busy: a job is running" }));
        let wakes = fake.paths().len();
        let full = boxes::start_box(&state, &c).await.unwrap_err();
        assert_eq!(full.to_string(), FULL);
        assert_eq!(sleeps(&b), 2, "agentd was asked, and said busy");
        assert!(!fake.paths()[wakes..].iter().any(|p| p.ends_with("/wake")));
        assert_eq!(state.box_state(&b).await.0, BoxState::Awake);

        // A daemon that cannot say how the host is doing wakes nothing.
        fake.answer("status", 500, json!({ "error": "runsc list failed" }));
        assert!(boxes::start_box(&state, &c).await.is_err());
        assert!(!fake.paths()[wakes..].iter().any(|p| p.ends_with("/wake")));
    }

    /// A routine due within the minute wakes its user's sleeping box; one further off, a box
    /// that is not asleep, a box on another provider or a relay without the provider does not.
    #[tokio::test]
    async fn an_asleep_box_is_woken_a_minute_ahead_of_its_routine() {
        let (mut state, _fake, user) = relay().await;
        let project_id = crate::people::tests_support::project_of(&state, &user, "First");
        boxes::create_box(&state, &user, Provider::Boxd, None)
            .await
            .unwrap();
        let now = now_ms();
        let due = |id: &str, at: i64| crate::db::Routine {
            id: id.into(),
            project_id: project_id.clone(),
            chat_id: "c".into(),
            name: "n".into(),
            prompt: "p".into(),
            trigger: json!({ "kind": "cron", "expr": "0 9 * * *", "tz": "UTC" }),
            enabled: true,
            created_at: now,
            last_run_at: None,
            next_run_at: Some(at),
        };
        let waking = |state: &AppState| {
            let (state, user) = (state.clone(), user.clone());
            async move {
                let job = state.db.due_wake_jobs().unwrap();
                let st = state.box_state(&user).await.0;
                let _ = state.db.finish_wake_job(&user);
                (st, job.len())
            }
        };

        state
            .db
            .insert_routine(&due("far", now + ROUTINE_HEAD_START_MS + 1))
            .unwrap();
        state.set_box_state(&user, BoxState::Asleep, None).await;
        wake_ahead_of_routines(&state, now).await;
        assert_eq!(waking(&state).await, (BoxState::Asleep, 0), "too early");

        state
            .db
            .insert_routine(&due("near", now + ROUTINE_HEAD_START_MS))
            .unwrap();
        for st in [BoxState::Awake, BoxState::Error] {
            state.set_box_state(&user, st, None).await;
            wake_ahead_of_routines(&state, now).await;
            assert_eq!(waking(&state).await, (st, 0));
        }
        state.set_box_state(&user, BoxState::Asleep, None).await;
        wake_ahead_of_routines(&state, now).await;
        assert_eq!(waking(&state).await, (BoxState::Waking, 1));

        // The same routine and the same sleeping box, on a relay that has no `boxd` provider
        // and on a box of another provider.
        state.set_box_state(&user, BoxState::Asleep, None).await;
        let mut row = state.db.user_box(&user).unwrap().unwrap();
        row.provider = "gvisor".into();
        state.db.upsert_box(&row).unwrap();
        wake_ahead_of_routines(&state, now).await;
        assert_eq!(waking(&state).await, (BoxState::Asleep, 0));
        row.provider = "boxd".into();
        state.db.upsert_box(&row).unwrap();
        Arc::get_mut(&mut state.cfg).expect("one holder").boxd = None;
        wake_ahead_of_routines(&state, now).await;
        assert_eq!(waking(&state).await, (BoxState::Asleep, 0));
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
