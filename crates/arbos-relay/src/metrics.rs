//! Prometheus metrics for the relay (`docs/metrics.md` lists every series).
//!
//! `GET /metrics` is served on a listener of its own (`ARBOS_RELAY_METRICS_LISTEN`), which must
//! be a loopback address and is off when unset: Caddy only proxies to the main listener and a
//! box sits on a bridge network, so neither the internet nor a box can reach it, and there is no
//! token to leak or rotate.
//!
//! The same listener is how Prometheus reaches the boxes: `GET /metrics/targets` lists the awake
//! ones and `GET /metrics/box/{id}` proxies a scrape to that box's agentd with its own token.
//!
//! Most series are gauges read at scrape time from state the relay already holds. A scrape never
//! waits: in-memory maps are read with `try_lock` (a busy one leaves its series out of that
//! scrape) and the database is asked by a sampler every [`SAMPLE_EVERY`], not by the scrape. A
//! box scrape reads that box's one row on a read connection, never through the writer lock. The
//! few counters and histograms are where a scrape-time read cannot answer.
//!
//! Labels are bounded sets (route template, status class, state, result, refusal code). Never a
//! user, chat, room, token or address: the dashboards are shared outside the team.
//!
//! The one exception has a path of its own: `GET /metrics/boxes` carries a series per box, and
//! the info series there names each box's container (which holds its user id) so the team's own
//! dashboards can join to cAdvisor. A Prometheus that scrapes only `/metrics` can never hold a
//! per-box series, whatever it is later configured to share.
//!
//! The exposition is written by hand: the text format is a few lines, and the workspace has no
//! metrics crate to reuse.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{MatchedPath, Path, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use arbos_core::wire::BoxState;

use crate::state::AppState;

const HTTP_REQUESTS: &str = "arbos_relay_http_requests_total";
const HTTP_SECONDS: &str = "arbos_relay_http_request_seconds_total";
const BOX_STARTS: &str = "arbos_relay_box_starts_total";
const BOX_START_SECONDS: &str = "arbos_relay_box_start_duration_seconds";
const BOX_WAKE_SECONDS: &str = "arbos_relay_box_wake_duration_seconds";
const ROOM_PUSHES: &str = "arbos_relay_room_pushes_total";
const ROOM_PUSH_REFUSALS: &str = "arbos_relay_room_push_refusals_total";
const ROOM_CHECKPOINTS: &str = "arbos_relay_room_checkpoints_total";
const MODEL_REQUESTS: &str = "arbos_relay_model_requests_total";
const MODEL_SECONDS: &str = "arbos_relay_model_response_seconds";
const BOX_DROPPED: &str = "arbos_relay_box_scrape_dropped_lines_total";
const DB_WRITER_WAIT: &str = "arbos_relay_db_writer_wait_seconds";
const TOKENS: &str = "arbos_relay_tokens_total";
/// The meter's four disjoint buckets, in the order `usage` rows store them.
const TOKEN_KINDS: [&str; 4] = ["input", "output", "cache_read", "cache_write"];

/// Every family: name, type, help. Rendered in this order; `docs/metrics.md` names each one.
#[rustfmt::skip]
const FAMILIES: &[(&str, &str, &str)] = &[
    ("arbos_relay_build_info", "gauge", "Always 1; the version label is the running build."),
    ("arbos_relay_uptime_seconds", "gauge", "Seconds since the relay started."),
    ("arbos_relay_rooms_open", "gauge", "Sync rooms held in memory."),
    ("arbos_relay_sockets", "gauge", "Open room websockets, by kind (client, box)."),
    ("arbos_relay_boxes", "gauge", "Boxes by the state the relay believes (awake, waking, asleep, error)."),
    ("arbos_relay_box_scrapes", "gauge", "Awake boxes by how their last metrics scrape ended (ok, old_image, unreachable, rejected, pending)."),
    (BOX_DROPPED, "counter", "Lines a box's exposition carried that are not agentd's, dropped by the relay."),
    ("arbos_relay_boxes_registered", "gauge", "Box rows by provider."),
    ("arbos_relay_boxes_max", "gauge", "Resident box cap on this host."),
    ("arbos_relay_boxes_turn_running", "gauge", "Boxes whose last keepalive said a turn is running."),
    ("arbos_relay_boxes_evicted", "gauge", "Boxes stopped under host pressure and not yet woken by their user."),
    ("arbos_relay_boxes_upgrading", "gauge", "Box upgrades in progress."),
    ("arbos_relay_boxes_creating", "gauge", "Box creates in progress."),
    ("arbos_relay_waitlist_length", "gauge", "Users waiting for a box slot."),
    ("arbos_relay_wake_queue_depth", "gauge", "Wake jobs queued or in flight."),
    (BOX_STARTS, "counter", "Box start attempts by result (ok, fetching, failed)."),
    (BOX_START_SECONDS, "histogram", "Time the provider took to start a box, successful starts only."),
    (BOX_WAKE_SECONDS, "histogram", "Time from a wake starting to the box connecting back."),
    (ROOM_PUSHES, "counter", "Room pushes by result (stored, dup, refused)."),
    (ROOM_PUSH_REFUSALS, "counter", "Refused room pushes by refusal code."),
    (ROOM_CHECKPOINTS, "counter", "Checkpoint requests by result (stored, too_soon, upload_incomplete, room_incomplete, below_floor, error)."),
    ("arbos_relay_notifications_total", "counter", "Device notifications by outcome."),
    (MODEL_REQUESTS, "counter", "Upstream model calls by provider and status class (2xx..5xx, unreachable)."),
    (MODEL_SECONDS, "histogram", "Time for the model provider to answer with response headers."),
    (TOKENS, "counter", "Tokens the proxy metered, by kind (input, output, cache_read, cache_write)."),
    ("arbos_relay_spend_today_usd", "gauge", "Relay-wide debits since 00:00 UTC, machine cost excluded."),
    ("arbos_relay_spend_daily_cap_usd", "gauge", "Global daily spend cap; reaching it trips the kill switch."),
    ("arbos_relay_kill_switch", "gauge", "1 while the kill switch is on (every spend is refused)."),
    (HTTP_REQUESTS, "counter", "HTTP requests by route template and status class."),
    (HTTP_SECONDS, "counter", "Seconds spent answering HTTP requests (to response headers), same labels."),
    ("arbos_relay_db_bytes", "gauge", "Size of the database files on disk (db, wal)."),
    (DB_WRITER_WAIT, "histogram", "Wait for the database writer lock, probed every 10 s."),
    ("process_resident_memory_bytes", "gauge", "Resident memory."),
    ("process_open_fds", "gauge", "Open file descriptors."),
    ("process_cpu_seconds_total", "counter", "User and system CPU time."),
    ("process_start_time_seconds", "gauge", "Start time, seconds since the epoch."),
];

/// The families of `/metrics/boxes`: one series per registered box (`box`), never on `/metrics`.
#[rustfmt::skip]
const BOX_FAMILIES: &[(&str, &str, &str)] = &[
    ("arbos_relay_box_info", "gauge", "Always 1; joins a box to its container."),
    ("arbos_relay_box_state", "gauge", "Always 1; the state label is what the relay believes (awake, waking, asleep, error, unknown)."),
    ("arbos_relay_box_turn_running", "gauge", "1 while the box's last keepalive said a turn is running."),
    ("arbos_relay_box_sockets", "gauge", "Open room websockets of the box."),
    ("arbos_relay_box_tokens_total", "counter", "Tokens metered for the box's own model calls, by kind."),
    ("arbos_relay_box_model_calls_total", "counter", "Model calls of the box that ended and were metered."),
    ("arbos_relay_box_spend_usd_total", "counter", "Metered cost of the box's successful model calls."),
];

/// Upper bounds, in seconds, shared by every histogram: a lock wait is milliseconds, a cold
/// box wake is minutes.
const BOUNDS: [f64; 14] = [
    0.001, 0.005, 0.025, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0,
];

/// How often the sampler asks the database.
const SAMPLE_EVERY: Duration = Duration::from_secs(10);

type Series = (&'static str, String, f64);

static COUNTERS: Mutex<BTreeMap<(&'static str, String), f64>> = Mutex::new(BTreeMap::new());
/// Per-bucket counts (the last slot is +Inf) and the sum.
static HISTOGRAMS: Mutex<BTreeMap<&'static str, ([u64; BOUNDS.len() + 1], f64)>> =
    Mutex::new(BTreeMap::new());
/// User id → how the box's last scrape through this relay ended, and when. A result older than
/// [`BOX_SCRAPE_FRESH_MS`] says nothing about the box now: it reads as `pending` and is dropped by
/// the next scrape, so a box that is gone, or a scrape job switched off, leaves nothing behind.
/// The key never leaves the process.
static BOX_SCRAPES: Mutex<BTreeMap<String, (&'static str, i64)>> = Mutex::new(BTreeMap::new());
/// A few scrape intervals: longer than any interval the job would be given, short enough that a
/// job switched off stops reading as boxes failing within minutes.
const BOX_SCRAPE_FRESH_MS: i64 = 5 * 60_000;
/// Box id → user id of the awake boxes, as the last `/metrics/targets` listed them.
static BOX_TARGETS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
/// What the sampler last read from the database.
static SAMPLED: Mutex<Vec<Series>> = Mutex::new(Vec::new());

/// What one box's own model calls have used since the relay started.
#[derive(Clone, Copy, Default)]
struct BoxUse {
    tokens: [f64; 4],
    calls: f64,
    usd: f64,
}

/// User id → use. The key never leaves the process: a scrape writes [`box_id`] in its place.
static BOX_USE: Mutex<BTreeMap<String, BoxUse>> = Mutex::new(BTreeMap::new());
/// The registered boxes as the sampler last read them: user id and container name.
static BOXES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
/// A metrics map, whatever happened to the last thread that held it. These maps are counters and
/// samples that stay usable after a panic mid-update, and the proxy counts a call on its way out:
/// a poisoned lock must cost a number, never the call.
fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What a scrape last read of each box from the relay's live maps: state, the last "turn
/// running" stamp, open sockets. A scrape never waits for those maps; when one is busy its boxes
/// keep what the scrape before saw instead of losing their series.
#[derive(Clone, Default)]
struct Live {
    states: BTreeMap<String, BoxState>,
    turns: BTreeMap<String, i64>,
    sockets: BTreeMap<String, u32>,
}
static LIVE: Mutex<Option<Live>> = Mutex::new(None);

fn add(name: &'static str, labels: String, v: f64) {
    *locked(&COUNTERS).entry((name, labels)).or_default() += v;
}

fn observe(name: &'static str, seconds: f64) {
    let slot = BOUNDS.partition_point(|b| *b < seconds);
    let mut h = locked(&HISTOGRAMS);
    let (buckets, sum) = h.entry(name).or_default();
    buckets[slot] += 1;
    *sum += seconds;
}

/// `key="value"` with the value escaped as the text format asks.
fn label(key: &str, value: &str) -> String {
    let v = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("{key}=\"{v}\"")
}

/// One provider start, as the wake runner saw it end.
pub fn box_start(result: &anyhow::Result<()>, took: Duration) {
    let outcome = match result {
        Ok(()) => {
            observe(BOX_START_SECONDS, took.as_secs_f64());
            "ok"
        }
        // The daemon is bringing a cold box back; the job looks again, it has not failed.
        Err(e) if e.to_string() == crate::boxd::FETCHING => "fetching",
        Err(_) => "failed",
    };
    add(BOX_STARTS, label("result", outcome), 1.0);
}

/// A box connected back `ms` after its wake started.
pub fn box_woke(ms: i64) {
    observe(BOX_WAKE_SECONDS, ms as f64 / 1000.0);
}

/// A room push was stored (`dup`: its batch was already there).
pub fn room_push(dup: bool) {
    let result = if dup { "dup" } else { "stored" };
    add(ROOM_PUSHES, label("result", result), 1.0);
}

/// A room push was refused. `code` is one of the relay's own refusal codes.
pub fn room_push_refused(code: &str) {
    add(ROOM_PUSHES, label("result", "refused"), 1.0);
    add(ROOM_PUSH_REFUSALS, label("code", code), 1.0);
}

/// A box asked for a room checkpoint. `room_incomplete` is the one to watch: the relay holds a
/// row its own doc cannot apply, and that room does not compact.
pub fn room_checkpoint(result: &str) {
    add(ROOM_CHECKPOINTS, label("result", result), 1.0);
}

/// One upstream model call: the provider's status, or `None` when it could not be reached.
/// The provider is read from the upstream URL the relay chose, never from the request.
pub fn model_request(url: &str, status: Option<u16>, took: Duration) {
    let provider = ["openrouter", "anthropic", "openai", "saygm", "engy"]
        .into_iter()
        .find(|p| url.contains(p))
        .unwrap_or("other");
    let class = status.map_or_else(|| "unreachable".to_owned(), |s| format!("{}xx", s / 100));
    add(
        MODEL_REQUESTS,
        format!("{},{}", label("provider", provider), label("class", &class)),
        1.0,
    );
    if status.is_some() {
        observe(MODEL_SECONDS, took.as_secs_f64());
    }
}

/// One metered model call, counted when it ends (the upstream reports usage last). `box_user`
/// is the box that made it, `None` for a device or the admin: those count in the fleet total
/// only. `usd` is what the call was charged, 0 for a failed one.
pub fn model_usage(box_user: Option<&str>, tokens: [i64; 4], usd: f64) {
    let tokens = tokens.map(|n| n.max(0) as f64);
    for (kind, n) in TOKEN_KINDS.iter().zip(tokens) {
        add(TOKENS, label("kind", kind), n);
    }
    let Some(user) = box_user else { return };
    let mut all = locked(&BOX_USE);
    let u = all.entry(user.to_owned()).or_default();
    for (have, n) in u.tokens.iter_mut().zip(tokens) {
        *have += n;
    }
    u.calls += 1.0;
    if usd.is_finite() && usd > 0.0 {
        u.usd += usd;
    }
}

/// A box's name in metrics: stable across restarts, and not derivable from its user id without
/// the relay key. User ids are public (`gh_<GitHub id>`), so a bare hash of one could be
/// matched to its box by anyone who can read a dashboard.
fn box_id(state: &AppState, user_id: &str) -> String {
    use hmac::Mac;
    let key = state.relay_key.subkey("metrics-box-id");
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(&key).expect("any key length");
    mac.update(user_id.as_bytes());
    hex::encode(&mac.finalize().into_bytes()[..6])
}

/// Count every request by route template and status class. The template, never the path: a path
/// carries ids. The time is to the response headers, so an upgrade or a stream counts its start.
pub async fn track_http(req: Request, next: Next) -> Response {
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "unmatched".to_owned(), |p| p.as_str().to_owned());
    let started = Instant::now();
    let res = next.run(req).await;
    let labels = format!(
        "{},class=\"{}xx\"",
        label("route", &route),
        res.status().as_u16() / 100
    );
    add(HTTP_REQUESTS, labels.clone(), 1.0);
    add(HTTP_SECONDS, labels, started.elapsed().as_secs_f64());
    res
}

/// Serve `GET /metrics` and `GET /metrics/boxes` on `listen`, which must be loopback, and start the database sampler.
pub async fn serve(state: AppState, listen: &str) -> anyhow::Result<()> {
    let addr: std::net::SocketAddr = listen
        .parse()
        .map_err(|e| anyhow::anyhow!("ARBOS_RELAY_METRICS_LISTEN {listen:?}: {e}"))?;
    anyhow::ensure!(
        addr.ip().is_loopback(),
        "ARBOS_RELAY_METRICS_LISTEN must be a loopback address (127.0.0.1:PORT), not {listen}: the endpoint has no other protection"
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%listen, "metrics listening");
    // The fleet total reads 0, not nothing, until the first call ends.
    model_usage(None, [0; 4], 0.0);
    spawn_sampler(state.clone());
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router(state)).await {
            tracing::error!(error = %e, "metrics listener stopped");
        }
    });
    Ok(())
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/metrics", get(scrape))
        .route("/metrics/boxes", get(scrape_boxes))
        .route("/metrics/targets", get(box_targets))
        .route("/metrics/box/{id}", get(box_scrape))
        .with_state(state)
}

/// The client the relay asks a box with. It follows no redirect: the request carries the box's
/// token, and where a box says to go next is the box's word, not the relay's.
fn box_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("a client with no redirects")
    })
}

/// How long one box may take to answer a scrape; under Prometheus's own 10 s default.
const BOX_SCRAPE_TIMEOUT: Duration = Duration::from_secs(5);

/// The most a box's exposition may weigh. agentd's is about 3 KB; a box is its user's machine,
/// so what it answers is untrusted and must not be able to fill the relay's memory.
const BOX_BODY_MAX: usize = 256 * 1024;

/// Exactly what agentd writes (`crates/arbos-agentd/src/metrics.rs`, `docs/metrics.md`): every
/// sample name with the one label it may carry. A box is its user's machine, so its answer is
/// held to this list; a name or a label that is not here never reaches Prometheus.
#[rustfmt::skip]
const BOX_SAMPLES: &[(&str, &str)] = &[
    ("arbos_agentd_uptime_seconds", ""),
    ("arbos_agentd_workspaces", "health"),
    ("arbos_agentd_turns_running", ""),
    ("arbos_agentd_sends_waiting", ""),
    ("arbos_agentd_workers_running", ""),
    ("arbos_agentd_jobs", "state"),
    ("arbos_agentd_rooms_open", ""),
    ("arbos_agentd_rooms_connected", ""),
    ("arbos_agentd_turns_started_total", ""),
    ("arbos_agentd_turns_finished_total", "result"),
    ("arbos_agentd_turn_duration_seconds_bucket", "le"),
    ("arbos_agentd_turn_duration_seconds_sum", ""),
    ("arbos_agentd_turn_duration_seconds_count", ""),
    ("arbos_agentd_worker_crashes_total", "kind"),
    ("arbos_agentd_model_requests_total", "class"),
    ("arbos_agentd_relay_reconnects_total", ""),
];

/// The one family the relay does not take from the box. A version a box writes is a string of
/// the box's choosing, a new series every time it changes; the version here is the image the
/// relay itself gave the box, so the family is bounded by the images ever rolled.
const BOX_BUILD_INFO: &str = "arbos_agentd_build_info";

/// The values each of those labels may take.
#[rustfmt::skip]
const BOX_LABEL_VALUES: &[(&str, &[&str])] = &[
    ("health", &["healthy", "unhealthy"]),
    ("state", &["running", "queued"]),
    ("result", &["done", "error", "cancelled"]),
    ("kind", &["main", "job"]),
    ("class", &["1xx", "2xx", "3xx", "4xx", "5xx", "unreachable"]),
    ("le", &["1", "2.5", "5", "10", "30", "60", "120", "300", "600", "1800", "+Inf"]),
];

/// `name` or `name{label="value"}` → the sample name, if it is one of agentd's with an allowed
/// label value.
fn box_series_name(series: &str) -> Option<&'static str> {
    let (name, label) = match series.split_once('{') {
        Some((name, rest)) => (name, Some(rest.strip_suffix("\"}")?.split_once("=\"")?)),
        None => (series, None),
    };
    let (name, key) = BOX_SAMPLES.iter().find(|(n, _)| *n == name)?;
    let ok = match label {
        None => key.is_empty(),
        Some((k, _)) if k != *key => false,
        Some((k, v)) => BOX_LABEL_VALUES
            .iter()
            .any(|(key, values)| *key == k && values.contains(&v)),
    };
    ok.then_some(*name)
}

/// A box's exposition, cut down to agentd's own samples: a known name, its one known label with
/// a known value, a numeric value, each series once. Comment lines are not passed on at all
/// (their text is free), so the relay writes the histogram's type itself, and `build_info` is
/// the relay's own line: `snapshot` is the image it gave the box (`arbos-agent-<version>`).
/// Returns the text and how many lines were dropped.
fn box_exposition(text: &str, snapshot: Option<&str>) -> (String, usize) {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = String::from("# TYPE arbos_agentd_turn_duration_seconds histogram\n");
    let version = snapshot.map(|s| s.strip_prefix("arbos-agent-").unwrap_or(s));
    let plain = |v: &&str| {
        (1..=32).contains(&v.len())
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    if let Some(version) = version.filter(plain) {
        let _ = writeln!(out, "{BOX_BUILD_INFO}{{{}}} 1", label("version", version));
    }
    let mut dropped = 0;
    for l in text.lines() {
        // agentd's own build_info and its comments are left out, not counted as foreign.
        if l.starts_with('#') || l.starts_with(BOX_BUILD_INFO) {
            continue;
        }
        let kept = l.rsplit_once(' ').is_some_and(|(series, value)| {
            box_series_name(series).is_some() && value.parse::<f64>().is_ok() && seen.insert(series)
        });
        if kept {
            out.push_str(l);
            out.push('\n');
        } else {
            dropped += 1;
        }
    }
    (out, dropped)
}

/// Box id → user id of the boxes that are awake now.
async fn awake_boxes(state: &AppState) -> BTreeMap<String, String> {
    let states = state.box_states.read().await;
    states
        .iter()
        .filter(|(_, (st, _))| *st == BoxState::Awake)
        .map(|(user_id, _)| (box_id(state, user_id), user_id.clone()))
        .collect()
}

/// `GET /metrics/targets`: Prometheus HTTP service discovery for the boxes. One target per awake
/// box, each this same listener with the box's own path, so Prometheus never learns a box's port
/// or token. A sleeping box is not listed: scraping must not be what keeps a box looking alive.
async fn box_targets(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    // The address Prometheus reached this listener on is the address it scrapes the boxes on.
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| {
            h.parse::<std::net::SocketAddr>()
                .is_ok_and(|a| a.ip().is_loopback())
        });
    let Some(host) = host else {
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    };
    let awake = awake_boxes(&state).await;
    let targets: Vec<serde_json::Value> = awake
        .keys()
        .map(|id| {
            serde_json::json!({
                "targets": [host],
                "labels": { "__metrics_path__": format!("/metrics/box/{id}"), "box": id },
            })
        })
        .collect();
    // What a scrape looks its id up in: Prometheus only ever asks for a target it was handed.
    *locked(&BOX_TARGETS) = awake;
    axum::Json(targets).into_response()
}

/// `GET /metrics/box/{id}`: one box's agentd `/metrics`, fetched with that box's own token.
async fn box_scrape(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    // Only a box that is awake now is asked. A sleeping box keeps its last `box_url`, and once
    // its port is another box's the token would be sent to the wrong machine.
    let mut user_id = locked(&BOX_TARGETS).get(&id).cloned();
    if user_id.is_none() {
        // Prometheus keeps its targets across a relay restart and this index does not: build it
        // again on a miss, so a restart costs no scrape.
        let awake = awake_boxes(&state).await;
        user_id = awake.get(&id).cloned();
        *locked(&BOX_TARGETS) = awake;
    }
    let user_id = match user_id {
        Some(u) if state.box_state(&u).await.0 == BoxState::Awake => Some(u),
        _ => None,
    };
    // One row on a read connection: this runs once per awake box per scrape interval, and must
    // not stand in the queue for the writer lock.
    let row = user_id.as_ref().and_then(|u| {
        let read = state.db.read(|c| {
            c.query_row(
                "SELECT box_url, mcp_token, snapshot FROM boxes WHERE user_id = ?1",
                [u],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                },
            )
        });
        read.ok()
    });
    let Some((Some(url), token, snapshot)) = row else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    let sent = box_client()
        .get(format!("{url}/metrics"))
        .bearer_auth(token)
        .timeout(BOX_SCRAPE_TIMEOUT)
        .send()
        .await;
    let down = || axum::http::StatusCode::BAD_GATEWAY.into_response();
    let (result, res) = match sent {
        Ok(mut r) if r.status().is_success() => {
            // Read to the cap and no further, then keep only agentd's own lines.
            let mut body = Vec::new();
            let read = loop {
                match r.chunk().await {
                    Ok(Some(chunk)) if body.len() + chunk.len() <= BOX_BODY_MAX => {
                        body.extend_from_slice(&chunk)
                    }
                    Ok(Some(_)) => break "rejected",
                    Ok(None) => break "ok",
                    Err(_) => break "unreachable",
                }
            };
            match (read, String::from_utf8(body)) {
                ("ok", Ok(text)) => {
                    let (kept, dropped) = box_exposition(&text, snapshot.as_deref());
                    if dropped > 0 {
                        add(BOX_DROPPED, String::new(), dropped as f64);
                    }
                    ("ok", exposition(kept))
                }
                ("ok", Err(_)) => ("rejected", down()),
                (failed, _) => (failed, down()),
            }
        }
        // agentd answers 404 for a route it does not have: an image from before `/metrics`.
        Ok(r) if r.status() == axum::http::StatusCode::NOT_FOUND => ("old_image", down()),
        _ => ("unreachable", down()),
    };
    if let Some(user_id) = user_id {
        let mut last = locked(&BOX_SCRAPES);
        let now = arbos_core::ids::now_ms();
        last.retain(|_, (_, at)| now - *at < BOX_SCRAPE_FRESH_MS);
        last.insert(user_id, (result, now));
    }
    res
}

async fn scrape(State(state): State<AppState>) -> Response {
    exposition(render(&state))
}

async fn scrape_boxes(State(state): State<AppState>) -> Response {
    exposition(render_boxes(&state))
}

/// Prometheus refuses a scrape whose content type it cannot place.
fn exposition(body: String) -> Response {
    let mut res = Response::new(body.into());
    res.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/plain; version=0.0.4"),
    );
    res
}

fn spawn_sampler(state: AppState) {
    tokio::spawn(async move {
        loop {
            let s = state.clone();
            let sample = move || {
                sample_boxes(&s);
                sample_db(&s)
            };
            if let Ok(sampled) = tokio::task::spawn_blocking(sample).await {
                *locked(&SAMPLED) = sampled;
            }
            tokio::time::sleep(SAMPLE_EVERY).await;
        }
    });
}

/// The database's share of the metrics: how long the writer lock makes a caller wait, the queue
/// and box counts, and today's spend. Each is one indexed or tiny-table query.
fn sample_db(state: &AppState) -> Vec<Series> {
    let started = Instant::now();
    let _ = state.db.with(|_| Ok(()));
    observe(DB_WRITER_WAIT, started.elapsed().as_secs_f64());

    let count = |sql: &str| {
        state
            .db
            .read(|c| c.query_row(sql, [], |r| r.get::<_, i64>(0)))
            .ok()
    };
    let mut out = vec![(
        "arbos_relay_boxes_max",
        String::new(),
        crate::fleet::max_boxes(state) as f64,
    )];
    if let Ok(spent) = state
        .db
        .debited_since_all(crate::proxy::start_of_utc_day_ms())
    {
        out.push(("arbos_relay_spend_today_usd", String::new(), spent));
    }
    out.push((
        "arbos_relay_spend_daily_cap_usd",
        String::new(),
        state.cfg.global_daily_cap_usd,
    ));
    // The gate's rule without the gate's write: an automatic trip ends at 00:00 UTC, whether or
    // not anything has asked the gate since. Monitoring reads; it never changes the switch.
    let on = state
        .db
        .kill_switch()
        .is_ok_and(|ks| ks.on && !(ks.auto && ks.at < crate::proxy::start_of_utc_day_ms()));
    out.push((
        "arbos_relay_kill_switch",
        String::new(),
        f64::from(u8::from(on)),
    ));
    if let Some(n) = count("SELECT COUNT(*) FROM waitlist") {
        out.push(("arbos_relay_waitlist_length", String::new(), n as f64));
    }
    if let Some(n) = count("SELECT COUNT(*) FROM box_wake_jobs") {
        out.push(("arbos_relay_wake_queue_depth", String::new(), n as f64));
    }
    let by_provider = state.db.read(|c| {
        c.prepare("SELECT provider, COUNT(*) FROM boxes GROUP BY provider")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()
    });
    for (provider, n) in by_provider.unwrap_or_default() {
        out.push((
            "arbos_relay_boxes_registered",
            label("provider", &provider),
            n as f64,
        ));
    }
    out
}

/// The registered boxes, for `/metrics/boxes`: a box's series exist while its row does, so the
/// cardinality is the number of boxes and a deleted box leaves with the next sample.
fn sample_boxes(state: &AppState) {
    let rows = state.db.read(|c| {
        c.prepare("SELECT user_id, sandbox_id FROM boxes ORDER BY user_id")?
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
    });
    let Ok(rows) = rows else { return };
    let boxes: Vec<(String, String)> = rows
        .into_iter()
        .map(|(user, container)| (user, container.unwrap_or_default()))
        .collect();
    locked(&BOX_USE).retain(|user, _| boxes.iter().any(|(u, _)| u == user));
    *locked(&BOXES) = boxes;
}

/// The gauges read at scrape time. Nothing here waits on a lock or touches the database.
fn gauges(state: &AppState) -> Vec<Series> {
    let mut g: Vec<Series> = vec![
        (
            "arbos_relay_build_info",
            label("version", env!("CARGO_PKG_VERSION")),
            1.0,
        ),
        (
            "arbos_relay_uptime_seconds",
            String::new(),
            state.started_at.elapsed().as_secs_f64(),
        ),
    ];
    if let Ok(rooms) = state.rooms.try_read() {
        g.push(("arbos_relay_rooms_open", String::new(), rooms.len() as f64));
    }
    if let Ok(conns) = state.box_conns.try_lock() {
        let all = state.restart.open_sockets.load(Ordering::Relaxed) as f64;
        let boxes = f64::from(conns.values().map(|(n, _)| *n).sum::<u32>());
        g.push(("arbos_relay_sockets", label("kind", "box"), boxes));
        g.push((
            "arbos_relay_sockets",
            label("kind", "client"),
            (all - boxes).max(0.0),
        ));
    }
    if let Ok(states) = state.box_states.try_read() {
        for (name, st) in [
            ("awake", BoxState::Awake),
            ("waking", BoxState::Waking),
            ("asleep", BoxState::Asleep),
            ("error", BoxState::Error),
        ] {
            let n = states.values().filter(|(s, _)| *s == st).count();
            g.push(("arbos_relay_boxes", label("state", name), n as f64));
        }
        // The awake boxes by how their last scrape ended, so "not on a metrics image yet" is a
        // number on the dashboard instead of an empty panel.
        let last = locked(&BOX_SCRAPES);
        let now = arbos_core::ids::now_ms();
        let last_of = |user_id: &String| match last.get(user_id) {
            Some((result, at)) if now - *at < BOX_SCRAPE_FRESH_MS => *result,
            _ => "pending",
        };
        let awake = || states.iter().filter(|(_, (st, _))| *st == BoxState::Awake);
        for result in ["ok", "old_image", "unreachable", "rejected", "pending"] {
            let n = awake().filter(|(u, _)| last_of(u) == result).count();
            g.push(("arbos_relay_box_scrapes", label("result", result), n as f64));
        }
    }
    if let Ok(turns) = state.turn_running.try_lock() {
        let fresh = arbos_core::ids::now_ms() - crate::fleet::TURN_FRESH_MS;
        let n = turns.values().filter(|at| **at >= fresh).count();
        g.push(("arbos_relay_boxes_turn_running", String::new(), n as f64));
    }
    if let Ok(evicted) = state.evicted.try_lock() {
        g.push((
            "arbos_relay_boxes_evicted",
            String::new(),
            evicted.len() as f64,
        ));
    }
    if let Ok(upgrading) = state.upgrading.try_lock() {
        g.push((
            "arbos_relay_boxes_upgrading",
            String::new(),
            upgrading.len() as f64,
        ));
    }
    if let Ok(creating) = state.restart.creating.try_lock() {
        g.push((
            "arbos_relay_boxes_creating",
            String::new(),
            creating.len() as f64,
        ));
    }
    let n = &state.push.stats;
    for (result, counter) in [
        ("enqueued", &n.enqueued),
        ("deduped", &n.deduped),
        ("delivered", &n.delivered),
        ("retried", &n.retried),
        ("dropped_expired", &n.dropped_expired),
        ("dropped_permanent", &n.dropped_permanent),
        ("dropped_no_token", &n.dropped_no_token),
    ] {
        g.push((
            "arbos_relay_notifications_total",
            label("result", result),
            counter.load(Ordering::Relaxed) as f64,
        ));
    }
    if let Ok(since_epoch) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        g.push((
            "process_start_time_seconds",
            String::new(),
            (since_epoch - state.started_at.elapsed()).as_secs_f64(),
        ));
    }
    // One stat each, no SQLite call: a WAL that stays far above its checkpoint size means a read
    // is being held open across many commits.
    for (file, name) in [("db", "relay.db"), ("wal", "relay.db-wal")] {
        if let Ok(meta) = std::fs::metadata(state.cfg.data_dir.join(name)) {
            g.push((
                "arbos_relay_db_bytes",
                label("file", file),
                meta.len() as f64,
            ));
        }
    }
    process(&mut g);
    g
}

/// The process's own numbers from `/proc` (Linux; elsewhere the series are absent).
fn process(g: &mut Vec<Series>) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let rss_kb = status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.split_whitespace().next()?.parse::<f64>().ok());
    if let Some(kb) = rss_kb {
        g.push(("process_resident_memory_bytes", String::new(), kb * 1024.0));
    }
    if let Ok(fds) = std::fs::read_dir("/proc/self/fd") {
        g.push(("process_open_fds", String::new(), fds.count() as f64));
    }
    // utime and stime are fields 14 and 15, counted after the `)` that ends the command name,
    // in clock ticks: 100 a second on every Linux this runs on.
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let ticks: Vec<f64> = stat
        .rsplit_once(')')
        .map(|(_, rest)| rest.split_whitespace().skip(11).take(2))
        .into_iter()
        .flatten()
        .filter_map(|v| v.parse().ok())
        .collect();
    if ticks.len() == 2 {
        g.push((
            "process_cpu_seconds_total",
            String::new(),
            (ticks[0] + ticks[1]) / 100.0,
        ));
    }
}

fn line(out: &mut String, name: &str, labels: &str, value: f64) {
    if labels.is_empty() {
        let _ = writeln!(out, "{name} {value}");
    } else {
        let _ = writeln!(out, "{name}{{{labels}}} {value}");
    }
}

/// The whole exposition.
pub fn render(state: &AppState) -> String {
    let mut series = gauges(state);
    series.extend(locked(&SAMPLED).iter().cloned());
    let counters = locked(&COUNTERS).clone();
    series.extend(counters.into_iter().map(|((n, l), v)| (n, l, v)));
    let histograms = locked(&HISTOGRAMS).clone();

    let mut out = String::new();
    for (name, kind, help) in FAMILIES {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
        for (_, labels, value) in series.iter().filter(|(n, _, _)| n == name) {
            line(&mut out, name, labels, *value);
        }
        if *kind == "histogram" {
            let (buckets, sum) = histograms.get(name).copied().unwrap_or_default();
            let mut seen = 0;
            for (bound, n) in BOUNDS.iter().zip(buckets) {
                seen += n;
                line(
                    &mut out,
                    &format!("{name}_bucket"),
                    &label("le", &bound.to_string()),
                    seen as f64,
                );
            }
            let count = (seen + buckets[BOUNDS.len()]) as f64;
            line(
                &mut out,
                &format!("{name}_bucket"),
                &label("le", "+Inf"),
                count,
            );
            line(&mut out, &format!("{name}_sum"), "", sum);
            line(&mut out, &format!("{name}_count"), "", count);
        }
    }
    out
}

/// The exposition of `/metrics/boxes`. Like [`render`], it reads memory only and never waits.
fn render_boxes(state: &AppState) -> String {
    let boxes = locked(&BOXES).clone();
    let used = locked(&BOX_USE).clone();
    let live = {
        let mut live = locked(&LIVE);
        let seen = live.get_or_insert_with(Live::default);
        if let Ok(states) = state.box_states.try_read() {
            seen.states = states.iter().map(|(u, (st, _))| (u.clone(), *st)).collect();
        }
        if let Ok(turns) = state.turn_running.try_lock() {
            seen.turns = turns.iter().map(|(u, at)| (u.clone(), *at)).collect();
        }
        if let Ok(conns) = state.box_conns.try_lock() {
            seen.sockets = conns.iter().map(|(u, (n, _))| (u.clone(), *n)).collect();
        }
        seen.clone()
    };
    let fresh = arbos_core::ids::now_ms() - crate::fleet::TURN_FRESH_MS;

    let mut series: Vec<Series> = Vec::new();
    for (user, container) in &boxes {
        let id = label("box", &box_id(state, user));
        if !container.is_empty() {
            let labels = format!("{id},{}", label("container", container));
            series.push(("arbos_relay_box_info", labels, 1.0));
        }
        let st = match live.states.get(user) {
            Some(BoxState::Awake) => "awake",
            Some(BoxState::Waking) => "waking",
            Some(BoxState::Asleep) => "asleep",
            Some(BoxState::Error) => "error",
            // Not heard of since the relay started.
            None => "unknown",
        };
        let labels = format!("{id},{}", label("state", st));
        series.push(("arbos_relay_box_state", labels, 1.0));
        let running = live.turns.get(user).is_some_and(|at| *at >= fresh);
        let v = f64::from(u8::from(running));
        series.push(("arbos_relay_box_turn_running", id.clone(), v));
        let n = live.sockets.get(user).copied().unwrap_or(0);
        series.push(("arbos_relay_box_sockets", id.clone(), f64::from(n)));
        // A box that has called nothing reads 0, so every box has a rate to plot.
        let u = used.get(user).copied().unwrap_or_default();
        for (kind, n) in TOKEN_KINDS.iter().zip(u.tokens) {
            let labels = format!("{id},{}", label("kind", kind));
            series.push(("arbos_relay_box_tokens_total", labels, n));
        }
        series.push(("arbos_relay_box_model_calls_total", id.clone(), u.calls));
        series.push(("arbos_relay_box_spend_usd_total", id, u.usd));
    }

    let mut out = String::new();
    for (name, kind, help) in BOX_FAMILIES {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
        for (_, labels, value) in series.iter().filter(|(n, _, _)| n == name) {
            line(&mut out, name, labels, *value);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{test_support::test_state, AuthMode};

    /// Parse the text format strictly enough to catch a broken exposition: every sample belongs
    /// to a family declared above it, has a numeric value, and appears once.
    fn parse(text: &str) -> BTreeMap<String, f64> {
        let mut typed = std::collections::HashSet::new();
        let mut samples = BTreeMap::new();
        for l in text.lines() {
            if let Some(rest) = l.strip_prefix("# TYPE ") {
                let (name, kind) = rest.split_once(' ').expect("type line");
                assert!(["gauge", "counter", "histogram"].contains(&kind), "{l}");
                assert!(typed.insert(name.to_owned()), "second TYPE for {name}");
                continue;
            }
            if l.starts_with("# HELP ") {
                continue;
            }
            let (series, value) = l.rsplit_once(' ').expect("sample line");
            let name = series.split('{').next().unwrap();
            let family = ["_bucket", "_sum", "_count"]
                .iter()
                .find_map(|s| name.strip_suffix(s).filter(|f| typed.contains(*f)))
                .unwrap_or(name);
            assert!(typed.contains(family), "sample before its TYPE: {l}");
            assert!(
                name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "{l}"
            );
            if let Some(labels) = series.strip_prefix(name).filter(|s| !s.is_empty()) {
                assert!(labels.starts_with('{') && labels.ends_with("\"}"), "{l}");
            }
            let value: f64 = value.parse().unwrap_or_else(|_| panic!("value: {l}"));
            assert!(
                samples.insert(series.to_owned(), value).is_none(),
                "twice: {l}"
            );
        }
        samples
    }

    async fn fetch(addr: std::net::SocketAddr, path: &str) -> String {
        reqwest::get(format!("http://{addr}{path}"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    }

    async fn serve_on_loopback(app: Router) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        addr
    }

    #[tokio::test]
    async fn a_scrape_parses_and_the_series_move_with_the_relay() {
        let state = test_state(AuthMode::Github);
        let addr = serve_on_loopback(router(state.clone())).await;

        let before = parse(&fetch(addr, "/metrics").await);
        assert_eq!(before["arbos_relay_boxes{state=\"awake\"}"], 0.0);
        assert_eq!(before["arbos_relay_rooms_open"], 0.0);
        assert!(before.contains_key("arbos_relay_uptime_seconds"));
        assert!(before
            .keys()
            .any(|k| k.starts_with("arbos_relay_build_info{version=")));
        let hist = |m: &BTreeMap<String, f64>, name: &str| {
            m.get(&format!("{name}_count")).copied().unwrap_or(0.0)
        };

        // A box wakes, a room opens, a start succeeds and one fails, pushes land and are refused.
        let user = arbos_core::ids::ulid();
        state.set_box_state(&user, BoxState::Awake, None).await;
        state.room("p/chat").await;
        box_start(&Ok(()), Duration::from_secs(3));
        box_start(&Err(anyhow::anyhow!("no")), Duration::from_secs(1));
        box_woke(4_000);
        room_push(false);
        room_push_refused("not_yours");
        let openrouter = "https://openrouter.ai/api/v1/chat/completions";
        model_request(openrouter, Some(429), Duration::from_millis(300));
        model_request(
            "https://api.anthropic.com/v1/messages",
            None,
            Duration::ZERO,
        );
        state.db.waitlist_add(&user).unwrap();
        *locked(&SAMPLED) = sample_db(&state);

        let text = fetch(addr, "/metrics").await;
        let after = parse(&text);
        assert_eq!(after["arbos_relay_boxes{state=\"awake\"}"], 1.0);
        assert_eq!(after["arbos_relay_rooms_open"], 1.0);
        assert_eq!(after["arbos_relay_waitlist_length"], 1.0);
        assert!(after["arbos_relay_db_bytes{file=\"db\"}"] > 0.0);
        assert!(after["arbos_relay_db_bytes{file=\"wal\"}"] > 0.0);
        assert_eq!(after["arbos_relay_wake_queue_depth"], 0.0);
        assert!(after["arbos_relay_box_starts_total{result=\"ok\"}"] >= 1.0);
        assert!(after["arbos_relay_box_starts_total{result=\"failed\"}"] >= 1.0);
        assert!(after["arbos_relay_room_pushes_total{result=\"stored\"}"] >= 1.0);
        assert!(after["arbos_relay_room_push_refusals_total{code=\"not_yours\"}"] >= 1.0);
        let models = |labels: &str| after[&format!("{MODEL_REQUESTS}{{{labels}}}")];
        assert!(models("provider=\"openrouter\",class=\"4xx\"") >= 1.0);
        assert!(models("provider=\"anthropic\",class=\"unreachable\"") >= 1.0);
        assert_eq!(after["arbos_relay_spend_today_usd"], 0.0);
        assert_eq!(after["arbos_relay_spend_daily_cap_usd"], 500.0);
        assert_eq!(after["arbos_relay_kill_switch"], 0.0);
        for name in [
            BOX_START_SECONDS,
            BOX_WAKE_SECONDS,
            DB_WRITER_WAIT,
            MODEL_SECONDS,
        ] {
            assert!(hist(&after, name) > hist(&before, name), "{name}");
        }
        // A 3 s start is in the 5 s bucket and not in the 2.5 s one.
        let bucket = |le: &str| after[&format!("{BOX_START_SECONDS}_bucket{{le=\"{le}\"}}")];
        assert!(bucket("5") > bucket("2.5"));
        assert_eq!(bucket("+Inf"), hist(&after, BOX_START_SECONDS));
        // The dashboards are shared: no series may name the user.
        assert!(!text.contains(&user));

        state.set_box_state(&user, BoxState::Asleep, None).await;
        let asleep = parse(&fetch(addr, "/metrics").await);
        assert_eq!(asleep["arbos_relay_boxes{state=\"awake\"}"], 0.0);
        assert_eq!(asleep["arbos_relay_boxes{state=\"asleep\"}"], 1.0);
    }

    #[tokio::test]
    async fn requests_are_counted_by_route_template_never_by_path() {
        let state = test_state(AuthMode::Github);
        let app = Router::new()
            .route("/metrics-test/{id}", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(track_http));
        let addr = serve_on_loopback(app).await;
        fetch(addr, "/metrics-test/secret-id").await;
        fetch(addr, "/metrics-test-nowhere/secret-id").await;

        let text = render(&state);
        let m = parse(&text);
        let labels = "{route=\"/metrics-test/{id}\",class=\"2xx\"}";
        assert_eq!(m[&format!("{HTTP_REQUESTS}{labels}")], 1.0);
        assert!(m.contains_key(&format!("{HTTP_SECONDS}{labels}")));
        assert!(m[&format!("{HTTP_REQUESTS}{{route=\"unmatched\",class=\"4xx\"}}")] >= 1.0);
        assert!(!text.contains("secret-id"));
    }

    #[tokio::test]
    async fn the_listener_refuses_an_address_that_is_not_loopback() {
        let state = test_state(AuthMode::Github);
        for listen in ["0.0.0.0:0", "192.0.2.1:9108", "localhost:9108", ""] {
            assert!(serve(state.clone(), listen).await.is_err(), "{listen}");
        }
    }

    /// Prometheus finds the awake boxes at `/metrics/targets` and scrapes each through the
    /// relay, which is the one that holds the box's address and token.
    #[tokio::test]
    async fn awake_boxes_are_listed_and_scraped_through_the_relay() {
        let state = test_state(AuthMode::Github);
        // A box's agentd: `/metrics` behind its own bearer.
        let agentd = serve_on_loopback(Router::new().route(
            "/metrics",
            get(|headers: axum::http::HeaderMap| async move {
                match headers.get("authorization").and_then(|v| v.to_str().ok()) {
                    Some("Bearer mcp-of-the-box") => BOX_ANSWER.into_response(),
                    _ => axum::http::StatusCode::UNAUTHORIZED.into_response(),
                }
            }),
        ))
        .await;
        let (awake, asleep) = (arbos_core::ids::ulid(), arbos_core::ids::ulid());
        state
            .db
            .upsert_box(&crate::db::Sandbox {
                user_id: awake.clone(),
                provider: "gvisor".into(),
                sandbox_id: Some("arbos-box-test".into()),
                daytona_state: None,
                device_id: "box:test".into(),
                box_token_hash: "hash".into(),
                mcp_token: "mcp-of-the-box".into(),
                snapshot: Some("arbos-agent-0.1.179".into()),
                last_started_at: None,
                last_stopped_at: None,
                last_seen_at: None,
                box_url: Some(format!("http://{agentd}")),
                preview_token: None,
                local_pid: None,
                migrated_from: Vec::new(),
            })
            .unwrap();
        state.set_box_state(&awake, BoxState::Awake, None).await;
        state.set_box_state(&asleep, BoxState::Asleep, None).await;
        let addr = serve_on_loopback(router(state.clone())).await;

        let listed = fetch(addr, "/metrics/targets").await;
        let targets: Vec<serde_json::Value> = serde_json::from_str(&listed).unwrap();
        assert_eq!(targets.len(), 1, "only the awake box: {listed}");
        assert_eq!(targets[0]["targets"][0], addr.to_string());
        let path = targets[0]["labels"]["__metrics_path__"].as_str().unwrap();
        assert_eq!(
            path,
            format!(
                "/metrics/box/{}",
                targets[0]["labels"]["box"].as_str().unwrap()
            )
        );
        assert!(!listed.contains(&awake), "a target must not name the user");

        assert_eq!(last_scrapes(&state)["pending"], 1.0);
        // A relay that restarted has no index, and Prometheus still has its targets: the first
        // scrape after a restart must find the box all the same.
        locked(&BOX_TARGETS).clear();
        let dropped = || {
            let now = parse(&render(&state));
            now.get(BOX_DROPPED).copied().unwrap_or(0.0)
        };
        let dropped_before = dropped();
        let scraped = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
        let last = last_scrapes(&state);
        assert_eq!((last["ok"], last["pending"]), (1.0, 0.0));
        assert_eq!(
            scraped.headers()["content-type"],
            "text/plain; version=0.0.4"
        );
        // Only agentd's own samples come through: a made-up family, a fresh label value, a
        // second version, a repeat and anything foreign are dropped, and counted.
        assert_eq!(scraped.text().await.unwrap(), BOX_KEPT);
        assert_eq!(dropped() - dropped_before, 10.0);
        // The id is keyed: the bare hash of a (public) user id does not find the box.
        let bare = {
            use sha2::Digest;
            hex::encode(&sha2::Sha256::digest(awake.as_bytes())[..6])
        };
        assert_ne!(targets[0]["labels"]["box"], bare);
        assert_eq!(box_id(&state, &awake), box_id(&state, &awake));
        let mut rekeyed = state.clone();
        rekeyed.relay_key =
            std::sync::Arc::new(crate::connectors::crypto::RelayKey::derive("another-key"));
        assert_ne!(box_id(&state, &awake), box_id(&rekeyed, &awake));
        let status = |path: &'static str| async move {
            reqwest::get(format!("http://{addr}{path}"))
                .await
                .unwrap()
                .status()
        };
        assert_eq!(status("/metrics/box/000000000000").await, 404);
        // An unreachable box reads as a failed scrape, not as an empty one.
        let mut gone = state.db.user_box(&awake).unwrap().unwrap();
        gone.box_url = Some("http://127.0.0.1:1".into());
        state.db.upsert_box(&gone).unwrap();
        let down = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
        assert_eq!(down.status(), 502);
        assert_eq!(last_scrapes(&state)["unreachable"], 1.0);
        // A box that answers but has no `/metrics` runs an image from before agentd had it.
        let old = serve_on_loopback(Router::new()).await;
        gone.box_url = Some(format!("http://{old}"));
        state.db.upsert_box(&gone).unwrap();
        let down = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
        assert_eq!(down.status(), 502);
        let last = last_scrapes(&state);
        assert_eq!((last["old_image"], last["unreachable"]), (1.0, 0.0));
        // A box that answers with more than the cap is cut off, not buffered.
        let huge = serve_on_loopback(Router::new().route(
            "/metrics",
            get(|| async { "arbos_agentd_turns_running 1\n".repeat(BOX_BODY_MAX / 16) }),
        ))
        .await;
        gone.box_url = Some(format!("http://{huge}"));
        state.db.upsert_box(&gone).unwrap();
        let down = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
        assert_eq!(down.status(), 502);
        assert_eq!(last_scrapes(&state)["rejected"], 1.0);
        // Scraped once, then never again (the job was switched off): the old result must not
        // stand for the box for ever. Past a few intervals the box is `pending` again, which no
        // rule counts as failing, and the next scrape of any box forgets the stale entry.
        let aged = arbos_core::ids::now_ms() - BOX_SCRAPE_FRESH_MS - 1_000;
        locked(&BOX_SCRAPES).get_mut(&awake).unwrap().1 = aged;
        let last = last_scrapes(&state);
        assert_eq!((last["rejected"], last["pending"]), (0.0, 1.0));
        let deleted = arbos_core::ids::ulid();
        locked(&BOX_SCRAPES).insert(deleted.clone(), ("old_image", aged));

        // Where the box's token may go. `other` stands for another user's box that now has the
        // port: it records every request that reaches it.
        let reached = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hits = reached.clone();
        let other = serve_on_loopback(Router::new().route(
            "/metrics",
            get(move || {
                hits.fetch_add(1, Ordering::SeqCst);
                async { "arbos_agentd_turns_running 0\n" }
            }),
        ))
        .await;
        // A redirect is not followed: the token stays with the box it belongs to.
        let location = format!("http://{other}/metrics");
        let redirecting = serve_on_loopback(Router::new().route(
            "/metrics",
            get(move || async move { axum::response::Redirect::temporary(&location) }),
        ))
        .await;
        gone.box_url = Some(format!("http://{redirecting}"));
        state.db.upsert_box(&gone).unwrap();
        let down = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
        assert_eq!(
            (down.status().as_u16(), reached.load(Ordering::SeqCst)),
            (502, 0)
        );
        assert!(
            !locked(&BOX_SCRAPES).contains_key(&deleted),
            "stale entries go"
        );
        // A box that is not awake is not asked at all, though its row still holds an address.
        gone.box_url = Some(format!("http://{other}"));
        state.db.upsert_box(&gone).unwrap();
        for st in [BoxState::Asleep, BoxState::Waking, BoxState::Error] {
            state.set_box_state(&awake, st, None).await;
            let res = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
            assert_eq!(
                (res.status().as_u16(), reached.load(Ordering::SeqCst)),
                (404, 0),
                "{st:?}"
            );
        }
        state.set_box_state(&awake, BoxState::Awake, None).await;
        let res = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
        assert_eq!(
            (res.status().as_u16(), reached.load(Ordering::SeqCst)),
            (200, 1)
        );
    }

    /// The list is agentd's own: every family its module names is one the relay lets through,
    /// so a family added there and forgotten here fails this test and not a dashboard.
    #[test]
    fn every_family_agentd_writes_is_on_the_list() {
        let agentd = include_str!("../../arbos-agentd/src/metrics.rs");
        let named: std::collections::BTreeSet<&str> = agentd
            .split('"')
            .filter(|s| s.starts_with("arbos_agentd_") && !s.contains([' ', '{']))
            .collect();
        assert!(named.len() >= 15, "{named:?}");
        for family in named.into_iter().filter(|f| *f != BOX_BUILD_INFO) {
            let listed = |n: &str| BOX_SAMPLES.iter().any(|(name, _)| *name == n);
            assert!(
                listed(family) || listed(&format!("{family}_bucket")),
                "{family} is not in BOX_SAMPLES"
            );
        }
    }

    /// The allowlist is exact: no family, label or value a box invents gets through, so a box
    /// cannot mint series or put its own words on a dashboard.
    #[test]
    fn a_box_cannot_add_a_family_a_label_or_a_value() {
        let minted: String = (0..6000)
            .map(|n| format!("arbos_agentd_x{n} 1\narbos_agentd_jobs{{state=\"s{n}\"}} 1\n"))
            .collect();
        let (kept, dropped) = box_exposition(&minted, None);
        assert_eq!(
            kept,
            "# TYPE arbos_agentd_turn_duration_seconds histogram\n"
        );
        assert_eq!(dropped, 12_000);
        // The version is the relay's word for the image, never the box's: a box that renames
        // its build every scrape adds no series, and is not counted as sending anything foreign.
        let renamed = "arbos_agentd_build_info{version=\"9.9.9\"} 1\n";
        let (kept, dropped) = box_exposition(renamed, Some("arbos-agent-0.7.0-rc1"));
        assert!(
            kept.ends_with("arbos_agentd_build_info{version=\"0.7.0-rc1\"} 1\n"),
            "{kept}"
        );
        assert_eq!((dropped, kept.contains("9.9.9")), (0, false));
        let (kept, _) = box_exposition(renamed, Some("not a \"version\"\n"));
        assert!(!kept.contains("build_info"), "{kept}");
        // Everything agentd really writes is on the list, each label value included.
        for (name, key) in BOX_SAMPLES {
            let values: &[&str] = match *key {
                "" => &[],
                key => BOX_LABEL_VALUES.iter().find(|(k, _)| *k == key).unwrap().1,
            };
            let series: Vec<String> = match values {
                [] => vec![name.to_string()],
                values => values
                    .iter()
                    .map(|v| format!("{name}{{{key}=\"{v}\"}}"))
                    .collect(),
            };
            for s in series {
                assert_eq!(box_series_name(&s), Some(*name), "{s}");
            }
        }
    }

    /// What a box answers: agentd's lines, and what a user who owns the port could add.
    const BOX_ANSWER: &str = "\
# HELP arbos_agentd_turns_running any text a box likes
# TYPE arbos_agentd_turns_running gauge
arbos_agentd_build_info{version=\"0.6.422\"} 1
arbos_agentd_turns_running 2
arbos_agentd_turns_finished_total{result=\"done\"} 5
arbos_agentd_turn_duration_seconds_bucket{le=\"+Inf\"} 5
arbos_agentd_turn_duration_seconds_bucket{le=\"2.5\"} 1
arbos_agentd_build_info{version=\"9.9.9\"} 1
arbos_agentd_build_info{version=\"any words\"} 1
arbos_agentd_turns_running 3
arbos_agentd_made_up_0001 1
arbos_agentd_jobs{state=\"fresh-value-1\"} 1
arbos_agentd_jobs{result=\"done\"} 1
arbos_agentd_jobs{state=\"queued\",box=\"000000000000\"} 1
arbos_relay_kill_switch 1
up 1
arbos_agentd_turns_running{user=\"gh_1\"} 9
arbos_agentd_turns_running not-a-number
arbos_agentd_turns_running{result=\"done\" 1
";
    const BOX_KEPT: &str = "\
# TYPE arbos_agentd_turn_duration_seconds histogram
arbos_agentd_build_info{version=\"0.1.179\"} 1
arbos_agentd_turns_running 2
arbos_agentd_turns_finished_total{result=\"done\"} 5
arbos_agentd_turn_duration_seconds_bucket{le=\"+Inf\"} 5
arbos_agentd_turn_duration_seconds_bucket{le=\"2.5\"} 1
";

    /// `arbos_relay_box_scrapes` by result, as a scrape of the relay reads it.
    fn last_scrapes(state: &AppState) -> BTreeMap<String, f64> {
        let prefix = "arbos_relay_box_scrapes{result=\"";
        parse(&render(state))
            .into_iter()
            .filter_map(|(k, v)| {
                Some((
                    k.strip_prefix(prefix)?.trim_end_matches("\"}").to_owned(),
                    v,
                ))
            })
            .collect()
    }

    /// Yesterday's automatic trip is over at 00:00 UTC, though the stored row still says `on`
    /// until something asks the gate. The gauge reads the rule and leaves the row alone.
    #[test]
    fn an_automatic_trip_from_yesterday_does_not_read_as_on() {
        let state = test_state(AuthMode::Github);
        let kill_switch = |state: &AppState| {
            let sampled = sample_db(state);
            let on = sampled.iter().find(|s| s.0 == "arbos_relay_kill_switch");
            on.expect("the kill switch series").2
        };
        let mut ks = crate::db_accounts::KillSwitch {
            on: true,
            auto: true,
            at: arbos_core::ids::now_ms(),
            ..Default::default()
        };
        state.db.set_kill_switch(&ks).unwrap();
        assert_eq!(kill_switch(&state), 1.0);
        ks.at = crate::proxy::start_of_utc_day_ms() - 1;
        state.db.set_kill_switch(&ks).unwrap();
        assert_eq!(kill_switch(&state), 0.0);
        assert!(state.db.kill_switch().unwrap().on, "reading must not write");
    }

    /// The public listener, as `main` builds it, has no metrics route: Caddy proxies all of it.
    #[tokio::test]
    async fn the_public_listener_answers_404_for_metrics() {
        let addr = serve_on_loopback(crate::app(test_state(AuthMode::Github))).await;
        let status = |path: &'static str| async move {
            let res = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
            res.status().as_u16()
        };
        assert_eq!(status("/healthz").await, 200);
        for path in [
            "/metrics",
            "/metrics/boxes",
            "/metrics/targets",
            "/metrics/box/0",
        ] {
            assert_eq!(status(path).await, 404, "{path}");
        }
    }

    /// A model call a box makes moves that box's series on `/metrics/boxes` and the fleet total
    /// on `/metrics`; the box is named by its keyed id, and `/metrics` carries nothing per box.
    #[tokio::test]
    async fn a_metered_call_moves_its_box_and_the_fleet_total() {
        use crate::people::tests_support::{person, project_of};
        use axum::extract::Path;
        let upstream = serve_on_loopback(Router::new().route(
            "/chat/completions",
            axum::routing::post(|| async {
                axum::Json(serde_json::json!({"model": "m", "usage": {
                    "prompt_tokens": 100, "completion_tokens": 20, "cost": 0.25,
                    "prompt_tokens_details": {"cached_tokens": 60, "cache_write_tokens": 10}}}))
            }),
        ))
        .await;
        let mut state = test_state(AuthMode::Github);
        let mut cfg = (*state.cfg).clone();
        cfg.openai_base = format!("http://{upstream}");
        cfg.openai_key = Some("test".into());
        state.cfg = std::sync::Arc::new(cfg);
        let (busy, _) = person(&state, 9871);
        let (idle, _) = person(&state, 9872);
        let project = project_of(&state, &busy, "Swarm");
        state.db.grant_credits(&busy, 10.0, "test", "test").unwrap();
        for user in [&busy, &idle] {
            state
                .db
                .with(|c| {
                    c.execute(
                        "INSERT INTO boxes(user_id, provider, sandbox_id, device_id, box_token_hash, mcp_token)
                         VALUES (?1, 'gvisor', ?2, 'd', 'h', 'm')",
                        [user, &crate::fleet::container_name(user)],
                    )
                })
                .unwrap();
        }
        state.set_box_state(&busy, BoxState::Awake, None).await;
        let now = arbos_core::ids::now_ms();
        state.turn_running.lock().await.insert(busy.clone(), now);
        sample_boxes(&state);

        let addr = serve_on_loopback(router(state.clone())).await;
        let fleet = |m: &BTreeMap<String, f64>, kind: &str| {
            let series = format!("{TOKENS}{{kind=\"{kind}\"}}");
            m.get(&series).copied().unwrap_or(0.0)
        };
        let before = parse(&fetch(addr, "/metrics").await);

        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-arbos-project", project.parse().unwrap());
        let res = crate::proxy::openai(
            State(state.clone()),
            crate::auth::Principal::Box {
                user_id: busy.clone(),
            },
            Path("chat/completions".into()),
            headers,
            r#"{"model":"m"}"#.into(),
        )
        .await;
        assert_eq!(res.status(), 200);
        axum::body::to_bytes(res.into_body(), 4096).await.unwrap();

        // The call is metered when its stream ends, a moment after the body is read.
        let id = box_id(&state, &busy);
        let calls = format!("arbos_relay_box_model_calls_total{{box=\"{id}\"}}");
        let mut text = String::new();
        for _ in 0..200 {
            text = fetch(addr, "/metrics/boxes").await;
            if parse(&text)[&calls] == 1.0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let boxes = parse(&text);
        let of = |name: &str, more: &str| boxes[&format!("{name}{{box=\"{id}\"{more}}}")];
        assert_eq!(of("arbos_relay_box_model_calls_total", ""), 1.0);
        // 100 prompt tokens, of which 60 were read from cache and 10 written to it.
        let tokens = [
            ("input", 30.0),
            ("output", 20.0),
            ("cache_read", 60.0),
            ("cache_write", 10.0),
        ];
        let after = parse(&fetch(addr, "/metrics").await);
        for (kind, n) in tokens {
            let more = format!(",kind=\"{kind}\"");
            assert_eq!(of("arbos_relay_box_tokens_total", &more), n, "{kind}");
            assert!(fleet(&after, kind) - fleet(&before, kind) >= n, "{kind}");
        }
        assert_eq!(of("arbos_relay_box_spend_usd_total", ""), 0.25);
        assert_eq!(of("arbos_relay_box_state", ",state=\"awake\""), 1.0);
        assert_eq!(of("arbos_relay_box_turn_running", ""), 1.0);
        let container = crate::fleet::container_name(&busy);
        let info = format!(",container=\"{container}\"");
        assert_eq!(of("arbos_relay_box_info", &info), 1.0);

        // The other box is there too, at rest, and none of the call landed on it.
        let other = box_id(&state, &idle);
        let at_rest = |name: &str, more: &str| boxes[&format!("{name}{{box=\"{other}\"{more}}}")];
        assert_eq!(
            at_rest("arbos_relay_box_tokens_total", ",kind=\"input\""),
            0.0
        );
        assert_eq!(at_rest("arbos_relay_box_state", ",state=\"unknown\""), 1.0);
        assert_eq!(at_rest("arbos_relay_box_turn_running", ""), 0.0);

        // The id is keyed: 12 hex characters that are neither the user id nor a bare hash of it.
        use sha2::Digest;
        assert!(id.len() == 12 && id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(id, hex::encode(&sha2::Sha256::digest(busy.as_bytes())[..6]));
        // Outside the info series, which exists to name the container, no line names the user.
        let leaks = |l: &&str| !l.starts_with("arbos_relay_box_info{") && l.contains(&busy);
        assert_eq!(text.lines().find(leaks), None);
        // `/metrics` carries no per-box series and no user.
        let fleet_text = fetch(addr, "/metrics").await;
        assert!(!fleet_text.contains("box=\"") && !fleet_text.contains("arbos_relay_box_info"));
        assert!(!fleet_text.contains(&busy) && !fleet_text.contains(&id));

        // A scrape that finds the state map busy serves what the last one read.
        let writer = state.box_states.write().await;
        let held = fetch(addr, "/metrics/boxes").await;
        drop(writer);
        let awake = format!("arbos_relay_box_state{{box=\"{id}\",state=\"awake\"}} 1");
        assert!(held.contains(&awake), "state kept while the map is locked");

        // A deleted box leaves with the next sample, its counters with it.
        state.db.delete_box_row(&busy).unwrap();
        sample_boxes(&state);
        let gone = fetch(addr, "/metrics/boxes").await;
        assert!(!gone.contains(&id) && gone.contains(&other));
        assert!(!locked(&BOX_USE).contains_key(&busy));
    }

    /// The proxy counts a call on its way out, and times it before it is charged. A panic that
    /// poisoned a metrics lock must not make the next count, or the next scrape, panic too.
    /// The assertions are on the fleet counters and the histogram, which no sampler prunes.
    #[test]
    fn counting_survives_poisoned_locks() {
        fn poison<T: Send>(m: &'static Mutex<T>) {
            let _ = std::thread::spawn(move || {
                let _held = m.lock().unwrap();
                panic!("poison the lock");
            })
            .join();
            assert!(m.is_poisoned());
        }
        poison(&BOX_USE);
        poison(&COUNTERS);
        poison(&HISTOGRAMS);
        poison(&SAMPLED);
        poison(&BOX_SCRAPES);
        poison(&BOX_TARGETS);
        let output = (TOKENS, label("kind", "output"));
        let tokens = || locked(&COUNTERS).get(&output).copied().unwrap_or(0.0);
        let answers = || locked(&HISTOGRAMS).get(MODEL_SECONDS).map_or(0, |h| h.0[2]);
        let (before, answered) = (tokens(), answers());
        model_usage(Some("gh_poison"), [1, 2, 3, 4], 0.5);
        model_request(
            "https://openrouter.ai/x",
            Some(200),
            Duration::from_millis(10),
        );
        assert!(tokens() >= before + 2.0);
        assert!(answers() > answered);
        let state = test_state(AuthMode::Github);
        assert!(render(&state).contains(TOKENS));
        assert!(render(&state).contains("arbos_relay_box_scrapes{result=\"pending\"}"));
        locked(&BOX_TARGETS).clear();
        assert!(render_boxes(&state).contains("arbos_relay_box_info"));
    }

    /// A listener that cannot start is an error for `main` to log, never a panic: a taken port
    /// must cost the dashboards and not the relay.
    #[tokio::test]
    async fn a_taken_port_is_an_error_and_not_a_panic() {
        let taken = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen = taken.local_addr().unwrap().to_string();
        let e = serve(test_state(AuthMode::Github), &listen).await;
        assert!(e.is_err(), "{listen} is taken");
    }

    #[test]
    fn the_doc_names_every_family() {
        let doc = include_str!("../../../docs/metrics.md");
        for (name, _, _) in FAMILIES.iter().chain(BOX_FAMILIES) {
            assert!(
                doc.contains(&format!("`{name}`")),
                "docs/metrics.md lacks {name}"
            );
        }
    }
}
