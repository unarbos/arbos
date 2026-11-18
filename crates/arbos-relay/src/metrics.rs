//! Prometheus metrics for the relay (`docs/metrics.md` lists every series).
//!
//! `GET /metrics` is served on a listener of its own (`ARBOS_RELAY_METRICS_LISTEN`), which must
//! be a loopback address and is off when unset: Caddy only proxies to the main listener and a
//! box sits on a bridge network, so neither the internet nor a box can reach it, and there is no
//! token to leak or rotate.
//!
//! Most series are gauges read at scrape time from state the relay already holds. A scrape never
//! waits: in-memory maps are read with `try_lock` (a busy one leaves its series out of that
//! scrape) and the database is asked by a sampler every [`SAMPLE_EVERY`], not by the scrape. The
//! few counters and histograms are where a scrape-time read cannot answer.
//!
//! Labels are bounded sets (route template, status class, state, result, refusal code). Never a
//! user, chat, room, token or address: the dashboards are shared outside the team.
//!
//! The exposition is written by hand: the text format is a few lines, and the workspace has no
//! metrics crate to reuse.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::Response;
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
const MODEL_REQUESTS: &str = "arbos_relay_model_requests_total";
const MODEL_SECONDS: &str = "arbos_relay_model_response_seconds";
const DB_WRITER_WAIT: &str = "arbos_relay_db_writer_wait_seconds";

/// Every family: name, type, help. Rendered in this order; `docs/metrics.md` names each one.
#[rustfmt::skip]
const FAMILIES: &[(&str, &str, &str)] = &[
    ("arbos_relay_build_info", "gauge", "Always 1; the version label is the running build."),
    ("arbos_relay_uptime_seconds", "gauge", "Seconds since the relay started."),
    ("arbos_relay_rooms_open", "gauge", "Sync rooms held in memory."),
    ("arbos_relay_sockets", "gauge", "Open room websockets, by kind (client, box)."),
    ("arbos_relay_boxes", "gauge", "Boxes by the state the relay believes (awake, waking, asleep, error)."),
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
    ("arbos_relay_notifications_total", "counter", "Device notifications by outcome."),
    (MODEL_REQUESTS, "counter", "Upstream model calls by provider and status class (2xx..5xx, unreachable)."),
    (MODEL_SECONDS, "histogram", "Time for the model provider to answer with response headers."),
    ("arbos_relay_spend_today_usd", "gauge", "Relay-wide debits since 00:00 UTC, machine cost excluded."),
    ("arbos_relay_spend_daily_cap_usd", "gauge", "Global daily spend cap; reaching it trips the kill switch."),
    ("arbos_relay_kill_switch", "gauge", "1 while the kill switch is on (every spend is refused)."),
    (HTTP_REQUESTS, "counter", "HTTP requests by route template and status class."),
    (HTTP_SECONDS, "counter", "Seconds spent answering HTTP requests (to response headers), same labels."),
    (DB_WRITER_WAIT, "histogram", "Wait for the database writer lock, probed every 10 s."),
    ("process_resident_memory_bytes", "gauge", "Resident memory."),
    ("process_open_fds", "gauge", "Open file descriptors."),
    ("process_cpu_seconds_total", "counter", "User and system CPU time."),
    ("process_start_time_seconds", "gauge", "Start time, seconds since the epoch."),
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
/// What the sampler last read from the database.
static SAMPLED: Mutex<Vec<Series>> = Mutex::new(Vec::new());

fn add(name: &'static str, labels: String, v: f64) {
    *COUNTERS
        .lock()
        .expect("metrics lock")
        .entry((name, labels))
        .or_default() += v;
}

fn observe(name: &'static str, seconds: f64) {
    let slot = BOUNDS.partition_point(|b| *b < seconds);
    let mut h = HISTOGRAMS.lock().expect("metrics lock");
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

/// One upstream model call: the provider's status, or `None` when it could not be reached.
/// The provider is read from the upstream URL the relay chose, never from the request.
pub fn model_request(url: &str, status: Option<u16>, took: Duration) {
    let provider = ["openrouter", "anthropic", "openai"]
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

/// Serve `GET /metrics` on `listen`, which must be loopback, and start the database sampler.
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
        .with_state(state)
}

async fn scrape(State(state): State<AppState>) -> Response {
    let mut res = Response::new(render(&state).into());
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
            if let Ok(sampled) = tokio::task::spawn_blocking(move || sample_db(&s)).await {
                *SAMPLED.lock().expect("metrics lock") = sampled;
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
    if let Ok(ks) = state.db.kill_switch() {
        out.push((
            "arbos_relay_kill_switch",
            String::new(),
            f64::from(u8::from(ks.on)),
        ));
    }
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
    series.extend(SAMPLED.lock().expect("metrics lock").iter().cloned());
    let counters = COUNTERS.lock().expect("metrics lock").clone();
    series.extend(counters.into_iter().map(|((n, l), v)| (n, l, v)));
    let histograms = HISTOGRAMS.lock().expect("metrics lock").clone();

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
        *SAMPLED.lock().unwrap() = sample_db(&state);

        let text = fetch(addr, "/metrics").await;
        let after = parse(&text);
        assert_eq!(after["arbos_relay_boxes{state=\"awake\"}"], 1.0);
        assert_eq!(after["arbos_relay_rooms_open"], 1.0);
        assert_eq!(after["arbos_relay_waitlist_length"], 1.0);
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

    #[test]
    fn the_doc_names_every_family() {
        let doc = include_str!("../../../docs/metrics.md");
        for (name, _, _) in FAMILIES {
            assert!(
                doc.contains(&format!("`{name}`")),
                "docs/metrics.md lacks {name}"
            );
        }
    }
}
