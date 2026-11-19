//! Prometheus metrics for the box (`docs/metrics.md` lists every series).
//!
//! `GET /metrics` on the box surface, behind the same bearer as `/box/state`: the box's MCP
//! token, which only the relay holds. Prometheus does not come here itself; the relay lists the
//! awake boxes and proxies each scrape (`arbos-relay`'s `metrics.rs`), so no box port or token
//! leaves the relay.
//!
//! Gauges are read at scrape time from what the supervisor already holds. Counters exist only
//! where a read cannot answer: how turns ended and how long they took, lost workers, model
//! calls by status class, room sessions that had to be dialled again.
//!
//! No label names a user, a workspace, a chat or a model: which box a series belongs to comes
//! from the scrape target. The small registry below is the relay's, copied: two crates, forty
//! lines, no dependency.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use crate::workspace::Supervisor;

const TURNS_STARTED: &str = "arbos_agentd_turns_started_total";
const TURNS_FINISHED: &str = "arbos_agentd_turns_finished_total";
const TURN_SECONDS: &str = "arbos_agentd_turn_duration_seconds";
const WORKER_CRASHES: &str = "arbos_agentd_worker_crashes_total";
const MODEL_REQUESTS: &str = "arbos_agentd_model_requests_total";
const RELAY_RECONNECTS: &str = "arbos_agentd_relay_reconnects_total";

/// Every family: name, type, help. Rendered in this order; `docs/metrics.md` names each one.
#[rustfmt::skip]
const FAMILIES: &[(&str, &str, &str)] = &[
    ("arbos_agentd_build_info", "gauge", "Always 1; the version label is the running build."),
    ("arbos_agentd_uptime_seconds", "gauge", "Seconds since agentd booted."),
    ("arbos_agentd_workspaces", "gauge", "Workspaces by health (healthy, unhealthy)."),
    ("arbos_agentd_turns_running", "gauge", "Main-agent turns running now."),
    ("arbos_agentd_sends_waiting", "gauge", "User messages waiting for a turn."),
    ("arbos_agentd_workers_running", "gauge", "Worker (subagent) turns running now."),
    ("arbos_agentd_jobs", "gauge", "Background jobs by state (running, queued)."),
    ("arbos_agentd_rooms_open", "gauge", "Rooms this box holds open."),
    ("arbos_agentd_rooms_connected", "gauge", "Open rooms whose relay session is up."),
    (TURNS_STARTED, "counter", "Main-agent turns started."),
    (TURNS_FINISHED, "counter", "Main-agent turns finished, by result (done, error, cancelled)."),
    (TURN_SECONDS, "histogram", "Main-agent turn duration."),
    (WORKER_CRASHES, "counter", "Lost workers by kind (main: the main agent's child; job: a job's worker)."),
    (MODEL_REQUESTS, "counter", "Model calls forwarded to the relay, by status class (2xx..5xx, unreachable)."),
    (RELAY_RECONNECTS, "counter", "Room sessions dialled again after the first."),
];

/// Open jobs by state. Answered from the `jobs_state` index alone, so the store lock is held for
/// the open rows and not for a walk over every job the workspace ever ran.
const JOBS_BY_STATE: &str =
    "SELECT state, COUNT(*) FROM jobs WHERE state IN ('queued','running') GROUP BY state";

/// Upper bounds in seconds: a turn is seconds to half an hour.
const BOUNDS: [f64; 10] = [1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0];

static COUNTERS: Mutex<BTreeMap<(&'static str, String), f64>> = Mutex::new(BTreeMap::new());
/// Per-bucket counts of [`TURN_SECONDS`] (the last slot is +Inf) and the sum.
static TURNS: Mutex<([u64; BOUNDS.len() + 1], f64)> = Mutex::new(([0; BOUNDS.len() + 1], 0.0));

fn inc(name: &'static str, labels: String) {
    *COUNTERS
        .lock()
        .expect("metrics lock")
        .entry((name, labels))
        .or_default() += 1.0;
}

pub fn turn_started() {
    inc(TURNS_STARTED, String::new());
}

/// `result` is the turn's final message status.
pub fn turn_finished(result: &'static str, total_ms: i64) {
    inc(TURNS_FINISHED, format!("result=\"{result}\""));
    let seconds = total_ms as f64 / 1000.0;
    let mut t = TURNS.lock().expect("metrics lock");
    t.0[BOUNDS.partition_point(|b| *b < seconds)] += 1;
    t.1 += seconds;
}

/// `kind` is `main` or `job`.
pub fn worker_crashed(kind: &'static str) {
    inc(WORKER_CRASHES, format!("kind=\"{kind}\""));
}

/// One model call through the stub: the relay's status, or `None` when it could not be reached.
pub fn model_request(status: Option<u16>) {
    let class = status.map_or_else(|| "unreachable".to_owned(), |s| format!("{}xx", s / 100));
    inc(MODEL_REQUESTS, format!("class=\"{class}\""));
}

pub fn relay_reconnected() {
    inc(RELAY_RECONNECTS, String::new());
}

fn line(out: &mut String, name: &str, labels: &str, value: f64) {
    if labels.is_empty() {
        let _ = writeln!(out, "{name} {value}");
    } else {
        let _ = writeln!(out, "{name}{{{labels}}} {value}");
    }
}

/// The whole exposition.
pub async fn render(sup: &Supervisor) -> String {
    let mut series: Vec<(&str, String, f64)> = vec![
        (
            "arbos_agentd_build_info",
            format!("version=\"{}\"", env!("CARGO_PKG_VERSION")),
            1.0,
        ),
        (
            "arbos_agentd_uptime_seconds",
            String::new(),
            (arbos_core::ids::now_ms() - sup.booted_at).max(0) as f64 / 1000.0,
        ),
    ];
    let (mut healthy, mut unhealthy, mut turns, mut waiting, mut workers) = (0, 0, 0, 0, 0);
    let (mut running, mut queued, mut connected) = (0i64, 0i64, 0u64);
    for w in sup.list().await {
        if w.agent.is_healthy() {
            healthy += 1;
        } else {
            unhealthy += 1;
        }
        turns += usize::from(w.agent.turn_running.load(Ordering::SeqCst));
        waiting += w.agent.sends_waiting();
        workers += w.agent.subagent_turns.load(Ordering::SeqCst);
        let by_state = w.store.with(|c| {
            c.prepare(JOBS_BY_STATE)?
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()
        });
        for (state, n) in by_state.unwrap_or_default() {
            if state == "queued" {
                queued += n;
            } else {
                running += n;
            }
        }
        connected += w.sync.link_summary().await["connected"]
            .as_u64()
            .unwrap_or(0);
    }
    for (name, labels, value) in [
        (
            "arbos_agentd_workspaces",
            "health=\"healthy\"",
            healthy as f64,
        ),
        (
            "arbos_agentd_workspaces",
            "health=\"unhealthy\"",
            unhealthy as f64,
        ),
        ("arbos_agentd_turns_running", "", turns as f64),
        ("arbos_agentd_sends_waiting", "", waiting as f64),
        ("arbos_agentd_workers_running", "", f64::from(workers)),
        ("arbos_agentd_jobs", "state=\"running\"", running as f64),
        ("arbos_agentd_jobs", "state=\"queued\"", queued as f64),
        ("arbos_agentd_rooms_open", "", sup.room_count().await as f64),
        ("arbos_agentd_rooms_connected", "", connected as f64),
    ] {
        series.push((name, labels.to_owned(), value));
    }
    let counters = COUNTERS.lock().expect("metrics lock").clone();
    series.extend(counters.into_iter().map(|((n, l), v)| (n, l, v)));
    let (buckets, sum) = *TURNS.lock().expect("metrics lock");

    let mut out = String::new();
    for (name, kind, help) in FAMILIES {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
        for (_, labels, value) in series.iter().filter(|(n, _, _)| n == name) {
            line(&mut out, name, labels, *value);
        }
        if *kind == "histogram" {
            let mut seen = 0;
            for (bound, n) in BOUNDS.iter().zip(buckets) {
                seen += n;
                line(
                    &mut out,
                    &format!("{name}_bucket"),
                    &format!("le=\"{bound}\""),
                    seen as f64,
                );
            }
            let count = (seen + buckets[BOUNDS.len()]) as f64;
            line(&mut out, &format!("{name}_bucket"), "le=\"+Inf\"", count);
            line(&mut out, &format!("{name}_sum"), "", sum);
            line(&mut out, &format!("{name}_count"), "", count);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every sample belongs to a family declared above it, has a numeric value, appears once.
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
            let value: f64 = value.parse().unwrap_or_else(|_| panic!("value: {l}"));
            assert!(
                samples.insert(series.to_owned(), value).is_none(),
                "twice: {l}"
            );
        }
        samples
    }

    /// The handler over the wire: the bearer `/box/state` asks for, an exposition that parses,
    /// and series that move when a turn runs.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_scrape_needs_the_bearer_parses_and_moves_with_a_turn() {
        let root = std::env::temp_dir().join(format!("arbos-metrics-{}", arbos_core::ids::ulid()));
        let sup = crate::workspace::tests::test_supervisor(&root);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/metrics", listener.local_addr().unwrap());
        crate::http::serve_box(listener, sup.clone()).unwrap();
        let http = reqwest::Client::new();
        let get = |bearer: Option<&'static str>| {
            let req = http.get(&url);
            match bearer {
                Some(b) => req.bearer_auth(b),
                None => req,
            }
            .send()
        };

        for bearer in [None, Some("wrong")] {
            assert_eq!(get(bearer).await.unwrap().status(), 401, "{bearer:?}");
        }
        let before = parse(&get(Some("test-mcp")).await.unwrap().text().await.unwrap());
        assert_eq!(before["arbos_agentd_turns_running"], 0.0);
        assert_eq!(before["arbos_agentd_workspaces{health=\"healthy\"}"], 0.0);
        assert_eq!(before["arbos_agentd_jobs{state=\"queued\"}"], 0.0);
        assert!(before.contains_key("arbos_agentd_uptime_seconds"));
        let count = |m: &BTreeMap<String, f64>, k: &str| m.get(k).copied().unwrap_or(0.0);

        turn_started();
        turn_finished("done", 45_000);
        turn_finished("error", 2_000);
        worker_crashed("job");
        model_request(Some(502));
        model_request(None);
        relay_reconnected();

        let after = parse(&get(Some("test-mcp")).await.unwrap().text().await.unwrap());
        for series in [
            "arbos_agentd_turns_started_total",
            "arbos_agentd_turns_finished_total{result=\"done\"}",
            "arbos_agentd_turns_finished_total{result=\"error\"}",
            "arbos_agentd_worker_crashes_total{kind=\"job\"}",
            "arbos_agentd_model_requests_total{class=\"5xx\"}",
            "arbos_agentd_model_requests_total{class=\"unreachable\"}",
            "arbos_agentd_relay_reconnects_total",
        ] {
            assert!(count(&after, series) > count(&before, series), "{series}");
        }
        let hist = "arbos_agentd_turn_duration_seconds";
        assert_eq!(
            after[&format!("{hist}_count")],
            count(&before, &format!("{hist}_count")) + 2.0
        );
        // A 45 s turn is in the 60 s bucket and not in the 30 s one.
        assert!(
            after[&format!("{hist}_bucket{{le=\"60\"}}")]
                > after[&format!("{hist}_bucket{{le=\"30\"}}")]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn open_jobs_are_counted_from_the_index_not_the_table() {
        let root = std::env::temp_dir().join(format!("arbos-metrics-{}", arbos_core::ids::ulid()));
        std::fs::create_dir_all(&root).unwrap();
        let store = crate::store::Store::open(&root.join("store.db")).unwrap();
        let plan = store
            .with(|c| {
                c.prepare(&format!("EXPLAIN QUERY PLAN {JOBS_BY_STATE}"))?
                    .query_map([], |r| r.get::<_, String>(3))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .join("; ");
        assert!(plan.contains("USING COVERING INDEX jobs_state"), "{plan}");
        let _ = std::fs::remove_dir_all(root);
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
