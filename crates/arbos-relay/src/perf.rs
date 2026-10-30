//! Anonymous performance diagnostics from the apps (iphone-scroll-perf, 2026-10-27). The iPhone
//! app, when its "Share performance diagnostics" switch is on (off by default), forwards what
//! MetricKit hands it — the daily `MXMetricPayload` (scroll hitch time ratio, hang time
//! histogram, launch / resume times, CPU, memory, exits) and `MXDiagnosticPayload`s (hang and
//! crash diagnostics with call trees) — as the payload's own JSON. `POST /perf/report` (device
//! token, so only signed-in apps can post) stores it under `<data>/perf/<id>.json`.
//!
//! No PII and no identity: the device token authenticates the request, but nothing about the
//! user or device is written — the stored item is the payload plus the platform, app version /
//! build, OS version and device model the app put in the envelope (MetricKit's `metaData`
//! carries the same). `GET /admin/perf?since=<ms>` lists the envelopes without the payloads,
//! `GET /admin/perf/{id}` returns one whole item (admin token), `GET /admin/perf/summary` the
//! headline numbers per app build (hitch ratio, hang seconds, launch percentiles) so the loop
//! can read "Jacob's phone" numbers from a TestFlight build without his phone being a rig.

use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use arbos_core::ids::{now_ms, ulid};

use crate::auth::Principal;
use crate::signin::api_error;
use crate::state::AppState;

/// A MetricKit metric payload is tens of KB; a diagnostic payload with call trees a few hundred.
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
/// The envelope's free-text fields are short identifiers, never prose.
const MAX_FIELD_LEN: usize = 80;

#[derive(Deserialize)]
pub struct Report {
    /// `metrics` (an `MXMetricPayload`) or `diagnostics` (an `MXDiagnosticPayload`).
    pub kind: String,
    #[serde(default = "default_platform")]
    pub platform: String,
    #[serde(default)]
    pub app_version: String,
    #[serde(default)]
    pub build: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub device_model: String,
    /// The payload's own `jsonRepresentation()`.
    pub payload: Value,
}

fn default_platform() -> String {
    "ios".to_owned()
}

/// What is stored (`<data>/perf/<id>.json`) and what the admin endpoints return.
#[derive(Serialize, Deserialize, Clone)]
pub struct Item {
    pub id: String,
    pub received_at: i64,
    pub kind: String,
    pub platform: String,
    pub app_version: String,
    pub build: String,
    pub os: String,
    pub device_model: String,
    pub bytes: usize,
    /// Omitted from the list; present on `GET /admin/perf/{id}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

fn root(state: &AppState) -> PathBuf {
    state.cfg.data_dir.join("perf")
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Short, printable, single-line; anything else becomes empty rather than stored.
fn clean(s: &str) -> String {
    let t = s.trim();
    if t.len() > MAX_FIELD_LEN || t.chars().any(char::is_control) {
        return String::new();
    }
    t.to_owned()
}

/// `POST /perf/report` (signed-in device): keep one MetricKit payload, anonymously.
pub async fn submit(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<Report>,
) -> Response {
    if !matches!(principal, Principal::Device { .. }) {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device sends performance diagnostics",
        );
    }
    let kind = match body.kind.as_str() {
        "metrics" | "diagnostics" => body.kind.clone(),
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "kind",
                "kind must be metrics or diagnostics",
            )
        }
    };
    let bytes = serde_json::to_vec(&body.payload)
        .map(|b| b.len())
        .unwrap_or(usize::MAX);
    if !body.payload.is_object() || bytes == 0 || bytes > MAX_PAYLOAD_BYTES {
        return api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload",
            "payload must be a JSON object of at most 1 MB",
        );
    }
    let item = Item {
        id: ulid(),
        received_at: now_ms(),
        kind,
        platform: clean(&body.platform),
        app_version: clean(&body.app_version),
        build: clean(&body.build),
        os: clean(&body.os),
        device_model: clean(&body.device_model),
        bytes,
        payload: Some(body.payload),
    };
    let dir = root(&state);
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string());
    }
    let Ok(json) = serde_json::to_vec(&item) else {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "json", "encode");
    };
    if let Err(e) = tokio::fs::write(dir.join(format!("{}.json", item.id)), json).await {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string());
    }
    (StatusCode::OK, Json(json!({ "id": item.id }))).into_response()
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub since: i64,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    200
}

async fn read_item(path: &std::path::Path) -> Option<Item> {
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice::<Item>(&bytes).ok()
}

async fn items_since(state: &AppState, since: i64) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(root(state)).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            if let Some(item) = read_item(&entry.path()).await {
                if item.received_at > since {
                    items.push(item);
                }
            }
        }
    }
    items.sort_by_key(|i| i.received_at);
    items
}

/// `GET /admin/perf?since=<ms>&limit=<n>`: envelopes after `since`, oldest first, no payloads.
pub async fn admin_list(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<ListQuery>,
) -> Response {
    if !matches!(principal, Principal::Admin) {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "admin token required");
    }
    let mut items = items_since(&state, q.since).await;
    items.truncate(q.limit.clamp(1, 2000));
    for item in &mut items {
        item.payload = None;
    }
    Json(json!({ "items": items })).into_response()
}

/// `GET /admin/perf/{id}`: one item with its payload.
pub async fn admin_item(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    if !matches!(principal, Principal::Admin) {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "admin token required");
    }
    if !safe_id(&id) {
        return api_error(StatusCode::BAD_REQUEST, "id", "bad id");
    }
    match read_item(&root(&state).join(format!("{id}.json"))).await {
        Some(item) => Json(item).into_response(),
        None => api_error(StatusCode::NOT_FOUND, "not_found", "no such item"),
    }
}

/// The headline numbers of one metric payload, as MetricKit's JSON spells them (ms are
/// `"<n> ms"` strings, ratios `"<n> ms/s"`); `None` when a field is missing.
#[derive(Default, Serialize, Clone, Copy, PartialEq, Debug)]
pub struct Headline {
    /// `animationMetrics.scrollHitchTimeRatio` in ms of hitch per second of scrolling.
    pub scroll_hitch_ms_per_s: Option<f64>,
    /// Total hang time in seconds from `applicationResponsivenessMetrics.histogrammedAppHangTime`
    /// (bucket midpoint × count).
    pub hang_seconds: Option<f64>,
    /// How many hangs the histogram counted.
    pub hangs: Option<u64>,
    /// Mean time to first draw in ms from `applicationLaunchMetrics.histogrammedTimeToFirstDraw`.
    pub launch_first_draw_ms: Option<f64>,
    /// Mean resume time in ms from `applicationLaunchMetrics.histogrammedResumeTime`.
    pub resume_ms: Option<f64>,
    /// `memoryMetrics.peakMemoryUsage` in MB.
    pub peak_memory_mb: Option<f64>,
    /// `cpuMetrics.cumulativeCPUTime` in seconds.
    pub cpu_seconds: Option<f64>,
}

/// `"12.5 ms"`, `"3 s"`, `"1.2 ms/s"`, `"450 MB"`: the number and the unit, scaled to the
/// unit asked for (`ms`, `s`, `MB`); plain numbers pass through.
pub fn measure(v: &Value, unit: &str) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return Some(n);
    }
    let s = v.as_str()?.trim();
    let mut parts = s.splitn(2, ' ');
    let n: f64 = parts.next()?.parse().ok()?;
    let have = parts.next().unwrap_or(unit).trim();
    let have = have.split('/').next().unwrap_or(have);
    let factor = match (have, unit) {
        (a, b) if a == b => 1.0,
        ("ms", "s") => 0.001,
        ("s", "ms") => 1000.0,
        ("sec", "s") => 1.0,
        ("sec", "ms") => 1000.0,
        ("kB", "MB") => 1.0 / 1024.0,
        ("GB", "MB") => 1024.0,
        ("bytes", "MB") => 1.0 / (1024.0 * 1024.0),
        _ => return None,
    };
    Some(n * factor)
}

/// MetricKit histograms: `{ "histogramNumBuckets": n, "histogramValue": { "0": { "bucketCount",
/// "bucketStart", "bucketEnd" }, … } }`. Returns (sum of midpoint × count in `unit`, count).
fn histogram(v: &Value, unit: &str) -> Option<(f64, u64)> {
    let buckets = v.get("histogramValue")?.as_object()?;
    let mut total = 0.0;
    let mut count = 0u64;
    for bucket in buckets.values() {
        let n = bucket.get("bucketCount")?.as_u64()?;
        let start = measure(bucket.get("bucketStart")?, unit)?;
        let end = measure(bucket.get("bucketEnd")?, unit)?;
        total += (start + end) / 2.0 * n as f64;
        count += n;
    }
    Some((total, count))
}

pub fn headline(payload: &Value) -> Headline {
    let mut h = Headline::default();
    if let Some(r) = payload
        .pointer("/animationMetrics/scrollHitchTimeRatio")
        .and_then(|v| measure(v, "ms"))
    {
        h.scroll_hitch_ms_per_s = Some(r);
    }
    if let Some((total, n)) = payload
        .pointer("/applicationResponsivenessMetrics/histogrammedAppHangTime")
        .and_then(|v| histogram(v, "s"))
    {
        h.hang_seconds = Some(total);
        h.hangs = Some(n);
    }
    if let Some((total, n)) = payload
        .pointer("/applicationLaunchMetrics/histogrammedTimeToFirstDraw")
        .and_then(|v| histogram(v, "ms"))
    {
        if n > 0 {
            h.launch_first_draw_ms = Some(total / n as f64);
        }
    }
    if let Some((total, n)) = payload
        .pointer("/applicationLaunchMetrics/histogrammedResumeTime")
        .and_then(|v| histogram(v, "ms"))
    {
        if n > 0 {
            h.resume_ms = Some(total / n as f64);
        }
    }
    if let Some(m) = payload
        .pointer("/memoryMetrics/peakMemoryUsage")
        .and_then(|v| measure(v, "MB"))
    {
        h.peak_memory_mb = Some(m);
    }
    if let Some(c) = payload
        .pointer("/cpuMetrics/cumulativeCPUTime")
        .and_then(|v| measure(v, "s"))
    {
        h.cpu_seconds = Some(c);
    }
    h
}

/// `GET /admin/perf/summary?since=<ms>`: per app build, the metric payloads' headline numbers
/// (each row is one payload: one device-day) plus the count of diagnostic payloads by kind.
pub async fn admin_summary(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<ListQuery>,
) -> Response {
    if !matches!(principal, Principal::Admin) {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "admin token required");
    }
    let items = items_since(&state, q.since).await;
    let mut builds: std::collections::BTreeMap<String, Value> = Default::default();
    for item in &items {
        let key = format!(
            "{} {} ({})",
            item.platform,
            item.app_version,
            if item.build.is_empty() {
                "?"
            } else {
                &item.build
            }
        );
        let entry = builds.entry(key).or_insert_with(|| {
            json!({ "metric_days": [], "diagnostics": { "hangs": 0, "crashes": 0, "other": 0 } })
        });
        let Some(payload) = &item.payload else {
            continue;
        };
        if item.kind == "metrics" {
            let mut row = serde_json::to_value(headline(payload)).unwrap_or(Value::Null);
            if let Some(obj) = row.as_object_mut() {
                obj.insert("id".into(), json!(item.id));
                obj.insert("received_at".into(), json!(item.received_at));
                obj.insert("os".into(), json!(item.os));
                obj.insert("device_model".into(), json!(item.device_model));
            }
            if let Some(days) = entry.get_mut("metric_days").and_then(Value::as_array_mut) {
                days.push(row);
            }
        } else {
            let hangs = payload
                .get("hangDiagnostics")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let crashes = payload
                .get("crashDiagnostics")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            if let Some(d) = entry.get_mut("diagnostics").and_then(Value::as_object_mut) {
                let bump = |d: &mut serde_json::Map<String, Value>, k: &str, by: usize| {
                    let n = d.get(k).and_then(Value::as_u64).unwrap_or(0) + by as u64;
                    d.insert(k.to_owned(), json!(n));
                };
                bump(d, "hangs", hangs);
                bump(d, "crashes", crashes);
                if hangs == 0 && crashes == 0 {
                    bump(d, "other", 1);
                }
            }
        }
    }
    Json(json!({ "since": q.since, "items": items.len(), "builds": builds })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrickit_measures_are_read_with_their_units() {
        assert_eq!(measure(&json!("12.5 ms"), "ms"), Some(12.5));
        assert_eq!(measure(&json!("2 s"), "ms"), Some(2000.0));
        assert_eq!(measure(&json!("1.5 ms/s"), "ms"), Some(1.5));
        assert_eq!(measure(&json!("450 MB"), "MB"), Some(450.0));
        assert_eq!(measure(&json!(7), "ms"), Some(7.0));
        assert_eq!(measure(&json!("nope"), "ms"), None);
    }

    #[test]
    fn headline_reads_hitch_hangs_and_launch_from_a_payload() {
        let payload = json!({
            "animationMetrics": { "scrollHitchTimeRatio": "3.2 ms/s" },
            "applicationResponsivenessMetrics": {
                "histogrammedAppHangTime": { "histogramNumBuckets": 2, "histogramValue": {
                    "0": { "bucketCount": 3, "bucketStart": "250 ms", "bucketEnd": "500 ms" },
                    "1": { "bucketCount": 1, "bucketStart": "2000 ms", "bucketEnd": "4000 ms" }
                } }
            },
            "applicationLaunchMetrics": {
                "histogrammedTimeToFirstDraw": { "histogramNumBuckets": 1, "histogramValue": {
                    "0": { "bucketCount": 4, "bucketStart": "1000 ms", "bucketEnd": "1200 ms" }
                } }
            },
            "memoryMetrics": { "peakMemoryUsage": "612 MB" },
            "cpuMetrics": { "cumulativeCPUTime": "95 s" }
        });
        let h = headline(&payload);
        assert_eq!(h.scroll_hitch_ms_per_s, Some(3.2));
        assert_eq!(h.hangs, Some(4));
        assert!((h.hang_seconds.unwrap() - (0.375 * 3.0 + 3.0)).abs() < 1e-9);
        assert_eq!(h.launch_first_draw_ms, Some(1100.0));
        assert_eq!(h.resume_ms, None);
        assert_eq!(h.peak_memory_mb, Some(612.0));
        assert_eq!(h.cpu_seconds, Some(95.0));
    }

    #[test]
    fn envelope_fields_are_short_and_printable() {
        assert_eq!(clean("  iPhone17,1 "), "iPhone17,1");
        assert_eq!(clean("a\nb"), "");
        assert_eq!(clean(&"x".repeat(200)), "");
        assert!(safe_id("01HZX5J0K3Q8R9T2V4W6Y8Z0AB"));
        assert!(!safe_id("../etc"));
    }
}
