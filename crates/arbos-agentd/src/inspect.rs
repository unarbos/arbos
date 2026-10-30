//! Agent inspection for debugging readers (`GET /w/{project}/inspect`, `GET /box/state` on the
//! box port; the relay proxies them as `GET /projects/{id}/inspect` and `GET /machine/debug`).
//!
//! One bounded, time-ordered window of what this agent did, from the sources that already
//! exist on the box: T3 rows (user → route → tool calls → result, gate and quick-reply
//! decisions, approvals, secret requests), `turn_metrics`, the responder decision log,
//! `audit.jsonl` (tail only) and a ring of the process's recent `warn!`/`error!` lines. Every
//! event is `{id, at, kind, chat_id, role, text, data, source}`; `next_cursor` is the `at` of
//! the last event returned, so a reader polls with `since=<next_cursor>`. Nothing here reads a
//! whole file: SQL hits the `at` indexes, the audit log is read from its tail.
//!
//! Scrubbing: T3 text is redacted at write time (D19); everything else goes through the same
//! `feedback::scrub` the rollout uses (key shapes + secret placeholders + live vault values).

use std::collections::{HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;

use serde_json::{json, Value};
use tracing::field::{Field, Visit};
use tracing::Subscriber;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

use arbos_core::ids::now_ms;

use crate::agent::Agent;
use crate::feedback::scrub;
use crate::workspace::{Supervisor, Workspace};

/// Default look-back when `since` is missing.
pub const DEFAULT_WINDOW_MS: i64 = 60 * 60 * 1000;
/// Oldest `since` a caller may ask for; older history is the chat export's job.
pub const MAX_WINDOW_MS: i64 = 7 * 24 * 60 * 60 * 1000;
pub const DEFAULT_LIMIT: usize = 200;
pub const MAX_LIMIT: usize = 1000;
/// How much of `audit.jsonl` one call reads (from the end).
const AUDIT_TAIL_BYTES: u64 = 512 * 1024;
/// Recent warnings kept in memory for `kind=warn`.
const WARN_RING: usize = 500;
/// Longest `text` an event carries (tool output is clipped harder, like the rollout).
const TEXT_CHARS: usize = 2_000;
const TOOL_OUTPUT_CHARS: usize = 800;

#[derive(Debug, Clone)]
pub struct Query {
    /// Events strictly after this Unix ms.
    pub since: i64,
    /// Only this chat (T3 thread / `chat_id` field); `None` = every chat of the agent.
    pub chat: Option<String>,
    /// Kind names and group aliases (`turns`, `gate`, `secrets`, `warnings`, `audit`); empty = all.
    pub kinds: HashSet<String>,
    pub limit: usize,
}

impl Query {
    /// Parse the query string of the HTTP route. Unknown keys are ignored.
    pub fn from_params(q: &std::collections::HashMap<String, String>) -> Self {
        let now = now_ms();
        let since = q
            .get("since")
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(now - DEFAULT_WINDOW_MS)
            .max(now - MAX_WINDOW_MS);
        let chat = q
            .get("chat")
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        let kinds = q
            .get("kind")
            .map(|k| {
                k.split(',')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let limit = q
            .get("limit")
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT);
        Self {
            since,
            chat,
            kinds,
            limit,
        }
    }

    fn wants(&self, kind: &str, source: &str) -> bool {
        if self.kinds.is_empty() {
            return true;
        }
        self.kinds.iter().any(|k| match k.as_str() {
            "turns" => matches!(
                kind,
                "user" | "assistant" | "tool_call" | "tool_output" | "route" | "turn" | "interim"
            ),
            "gate" => matches!(kind, "gate" | "respond_decision" | "quick_reply"),
            "secrets" => {
                kind.starts_with("secret") || matches!(kind, "egress" | "browser_fill" | "deny")
            }
            "warnings" => matches!(kind, "warn" | "error"),
            "audit" => source == "audit",
            k => k == kind,
        })
    }
}

/// One event of the window.
#[derive(Debug, Clone)]
struct Event {
    id: String,
    at: i64,
    kind: String,
    chat_id: Option<String>,
    role: Option<String>,
    text: String,
    data: Option<Value>,
    source: &'static str,
}

impl Event {
    fn to_json(&self) -> Value {
        let mut v = json!({
            "id": self.id, "at": self.at, "kind": self.kind, "chat_id": self.chat_id,
            "role": self.role, "text": self.text, "source": self.source,
        });
        if let Some(d) = &self.data {
            v["data"] = d.clone();
        }
        v
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}… [{} more chars]", s.chars().count() - max)
    }
}

fn scrub_value(v: &mut Value) {
    match v {
        Value::String(s) => {
            let clean = scrub(s);
            if clean != *s {
                *s = clean;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(scrub_value),
        Value::Object(map) => map.values_mut().for_each(scrub_value),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

// ---- warnings ring ----------------------------------------------------------------------------

fn warn_ring() -> &'static Mutex<VecDeque<Value>> {
    static RING: std::sync::OnceLock<Mutex<VecDeque<Value>>> = std::sync::OnceLock::new();
    RING.get_or_init(|| Mutex::new(VecDeque::with_capacity(WARN_RING)))
}

/// A `tracing` layer that keeps the last `WARN_RING` warn/error events in memory, so a
/// debugging reader sees "box-level warnings" without a log shipper.
pub struct WarnRing;

struct FieldCollector {
    message: String,
    fields: serde_json::Map<String, Value>,
}

impl Visit for FieldCollector {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let s = format!("{value:?}");
        if field.name() == "message" {
            self.message = s;
        } else {
            self.fields.insert(field.name().to_owned(), json!(s));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else {
            self.fields.insert(field.name().to_owned(), json!(value));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields.insert(field.name().to_owned(), json!(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields.insert(field.name().to_owned(), json!(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields.insert(field.name().to_owned(), json!(value));
    }
}

impl<S: Subscriber> Layer<S> for WarnRing {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let level = *event.metadata().level();
        if level > tracing::Level::WARN {
            return;
        }
        let mut c = FieldCollector {
            message: String::new(),
            fields: serde_json::Map::new(),
        };
        event.record(&mut c);
        let chat_id = c
            .fields
            .get("chat_id")
            .or_else(|| c.fields.get("chat"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let project = c
            .fields
            .get("project")
            .or_else(|| c.fields.get("project_id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let entry = json!({
            "at": now_ms(),
            "level": if level == tracing::Level::ERROR { "error" } else { "warn" },
            "target": event.metadata().target(),
            "message": c.message,
            "fields": Value::Object(c.fields),
            "chat_id": chat_id,
            "project_id": project,
        });
        if let Ok(mut ring) = warn_ring().lock() {
            if ring.len() >= WARN_RING {
                ring.pop_front();
            }
            ring.push_back(entry);
        }
    }
}

/// Warnings after `since`, newest last. `project_id` keeps lines tagged with another project
/// out of this agent's window; untagged (box-level) lines are kept.
fn warnings_after(since: i64, project_id: Option<&str>) -> Vec<Event> {
    let Ok(ring) = warn_ring().lock() else {
        return Vec::new();
    };
    ring.iter()
        .filter(|w| w["at"].as_i64().unwrap_or(0) > since)
        .filter(|w| match (project_id, w["project_id"].as_str()) {
            (Some(me), Some(tagged)) => me == tagged,
            _ => true,
        })
        .enumerate()
        .map(|(i, w)| {
            let at = w["at"].as_i64().unwrap_or(0);
            Event {
                id: format!("warn-{at}-{i}"),
                at,
                kind: w["level"].as_str().unwrap_or("warn").to_owned(),
                chat_id: w["chat_id"].as_str().map(str::to_owned),
                role: None,
                text: scrub(&clip(w["message"].as_str().unwrap_or(""), TEXT_CHARS)),
                data: Some(json!({ "target": w["target"], "fields": w["fields"] })),
                source: "log",
            }
        })
        .collect()
}

// ---- audit tail -------------------------------------------------------------------------------

/// The last `AUDIT_TAIL_BYTES` of `audit.jsonl` as parsed lines with `ts > since`.
fn audit_after(path: &std::path::Path, since: i64) -> Vec<Event> {
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(AUDIT_TAIL_BYTES);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = String::new();
    if f.read_to_string(&mut buf).is_err() {
        return Vec::new();
    }
    let mut lines = buf.lines();
    if start > 0 {
        // The first line is almost surely a partial one.
        lines.next();
    }
    lines
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["ts"].as_i64().unwrap_or(0) > since)
        .enumerate()
        .map(|(i, mut v)| {
            let at = v["ts"].as_i64().unwrap_or(0);
            let kind = v["kind"].as_str().unwrap_or("audit").to_owned();
            let chat_id = v["chat_id"].as_str().map(str::to_owned);
            if let Some(o) = v.as_object_mut() {
                o.remove("ts");
                o.remove("kind");
                o.remove("source");
            }
            scrub_value(&mut v);
            Event {
                id: format!("audit-{at}-{i}"),
                at,
                kind,
                chat_id,
                role: None,
                text: String::new(),
                data: Some(v),
                source: "audit",
            }
        })
        .collect()
}

// ---- the window -------------------------------------------------------------------------------

fn t3_events(w: &Workspace, q: &Query) -> Vec<Event> {
    w.store
        .t3_after(q.since, q.chat.as_deref(), (q.limit + 1) as i64)
        .unwrap_or_default()
        .into_iter()
        .map(|(r, mut source)| {
            let max = if r.kind == "tool_output" {
                TOOL_OUTPUT_CHARS
            } else {
                TEXT_CHARS
            };
            // The row's `source` column holds the structured facts (gate reason, quick-reply
            // side, tool name, message ids); the subagent id rides along when present.
            if let Some(sub) = r.subagent_id {
                let d = source.get_or_insert_with(|| json!({}));
                if let Some(o) = d.as_object_mut() {
                    o.insert("subagent_id".into(), json!(sub));
                }
            }
            if let Some(d) = source.as_mut() {
                scrub_value(d);
            }
            Event {
                id: r.id,
                at: r.at,
                kind: r.kind,
                chat_id: Some(r.thread_id),
                role: r.role,
                text: scrub(&clip(r.text.trim(), max)),
                data: source,
                source: "t3",
            }
        })
        .collect()
}

fn turn_events(w: &Workspace, q: &Query) -> Vec<Event> {
    w.store
        .turn_metrics_after(q.since, q.chat.as_deref(), (q.limit + 1) as i64)
        .unwrap_or_default()
        .into_iter()
        .map(|mut m| {
            let at = m["created_at"].as_i64().unwrap_or(0);
            let id = m["id"].as_str().unwrap_or("").to_owned();
            let chat_id = m["chat_id"].as_str().map(str::to_owned);
            let text = format!(
                "turn {}: route {} · first token {} · total {} ms · {} tool calls{}",
                m["message_id"].as_str().unwrap_or("?"),
                m["route"].as_str().unwrap_or("-"),
                m["ttft_ms"]
                    .as_i64()
                    .map(|v| format!("{v} ms"))
                    .unwrap_or_else(|| "-".into()),
                m["total_ms"].as_i64().unwrap_or(0),
                m["tool_calls"].as_i64().unwrap_or(0),
                if m["escalated"].as_bool() == Some(true) {
                    " · escalated"
                } else {
                    ""
                }
            );
            if let Some(o) = m.as_object_mut() {
                o.remove("id");
                o.remove("created_at");
                o.remove("chat_id");
            }
            Event {
                id,
                at,
                kind: "turn".into(),
                chat_id,
                role: None,
                text,
                data: Some(m),
                source: "turn_metrics",
            }
        })
        .collect()
}

fn decision_events(w: &Workspace, q: &Query) -> Vec<Event> {
    w.store
        .decisions_after(q.since, "respond", (q.limit + 1) as i64)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|mut d| {
            let chat_id = d["gate"]["chat_id"].as_str().map(str::to_owned);
            if let Some(want) = &q.chat {
                if chat_id.as_deref() != Some(want.as_str()) {
                    return None;
                }
            }
            let at = d["created_at"].as_i64().unwrap_or(0);
            let id = d["id"].as_str().unwrap_or("").to_owned();
            let text = format!(
                "respond gate: {} {} — {}",
                d["action"].as_str().unwrap_or("?"),
                d["target_ids"]
                    .as_array()
                    .map(|a| a
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(","))
                    .unwrap_or_default(),
                d["reason"].as_str().unwrap_or("")
            );
            if let Some(o) = d.as_object_mut() {
                o.remove("id");
                o.remove("created_at");
                o.remove("side");
            }
            scrub_value(&mut d);
            Some(Event {
                id,
                at,
                kind: "respond_decision".into(),
                chat_id,
                role: None,
                text: scrub(&clip(&text, TEXT_CHARS)),
                data: Some(d),
                source: "decision_log",
            })
        })
        .collect()
}

/// Sort, filter, cut to `limit` without splitting a millisecond: when the page is full, events
/// sharing the last `at` are held back (unless the page is only that millisecond), so
/// `since=next_cursor` never skips one.
fn cut(mut events: Vec<Event>, q: &Query) -> (Vec<Event>, i64, bool) {
    events.retain(|e| q.wants(&e.kind, e.source));
    events.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
    let truncated = events.len() > q.limit;
    if truncated {
        events.truncate(q.limit);
        let last_at = events.last().map(|e| e.at).unwrap_or(q.since);
        let first_at = events.first().map(|e| e.at).unwrap_or(q.since);
        if last_at != first_at {
            events.retain(|e| e.at != last_at);
        }
    }
    let next = events.last().map(|e| e.at).unwrap_or(q.since);
    (events, next, truncated)
}

/// The identity this agent answers with right now (#1431) and the box facts.
pub async fn identity_json(agent: &Agent) -> Value {
    let ident = agent.identity.snapshot();
    let (talking, subagent) = agent.live_models().await;
    json!({
        "project_id": agent.cfg.project_id,
        "name": ident.name,
        "handle": agent.identity.handle(),
        "former_names": ident.former_names(),
        "renames": ident.renames,
        "talking_model": talking,
        "subagent_model": subagent,
        "code_mode": agent.cfg.code_mode(),
    })
}

/// What the agent is doing right now: lock state and the live turn.
pub async fn turn_json(agent: &Agent) -> Value {
    let t = agent.turn.lock().await;
    json!({
        "running": agent.any_turn_running(),
        "sends_waiting": agent.sends_waiting(),
        "pi_ready": agent.pi_ready.load(std::sync::atomic::Ordering::SeqCst),
        "current": t.as_ref().map(|t| json!({
            "chat_id": t.chat_id, "user_message_id": t.user_msg_id, "assistant_message_id": t.assistant_msg_id,
            "elapsed_ms": t.started.elapsed().as_millis() as u64,
            "route": t.route.as_ref().map(|r| format!("{r:?}")),
            "route_attempts": t.route_attempts, "tool_calls": t.metrics.tool_calls,
            "model_calls": t.metrics.model_calls,
        })),
    })
}

/// `GET /w/{project}/inspect`.
pub async fn page(w: &Workspace, q: &Query) -> Value {
    let mut events = t3_events(w, q);
    events.extend(turn_events(w, q));
    events.extend(decision_events(w, q));
    events.extend(
        audit_after(&w.cfg.audit_path(), q.since)
            .into_iter()
            .filter(|e| q.chat.is_none() || e.chat_id == q.chat),
    );
    events.extend(
        warnings_after(q.since, Some(&w.project_id))
            .into_iter()
            .filter(|e| q.chat.is_none() || e.chat_id.is_none() || e.chat_id == q.chat),
    );
    let (events, next_cursor, truncated) = cut(events, q);
    json!({
        "project_id": w.project_id,
        "since": q.since,
        "next_cursor": next_cursor,
        "truncated": truncated,
        "count": events.len(),
        "identity": identity_json(&w.agent).await,
        "turn": turn_json(&w.agent).await,
        "links": w.sync.link_summary().await,
        "events": events.iter().map(Event::to_json).collect::<Vec<_>>(),
    })
}

// ---- box state --------------------------------------------------------------------------------

fn meminfo_kb(key: &str, text: &str) -> Option<i64> {
    text.lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
}

// statvfs field widths differ per libc (u32 on macOS, u64 on Linux): the casts are needed on
// one platform and "unnecessary" on the other.
#[allow(clippy::unnecessary_cast)]
fn disk_json(path: &std::path::Path) -> Value {
    match nix::sys::statvfs::statvfs(path) {
        Ok(s) => {
            let frag = s.fragment_size() as u64;
            let total = s.blocks() as u64 * frag;
            let avail = s.blocks_available() as u64 * frag;
            json!({ "path": path, "total_mb": total / (1024 * 1024), "available_mb": avail / (1024 * 1024) })
        }
        Err(e) => json!({ "path": path, "error": e.to_string() }),
    }
}

/// `GET /box/state`: image, agentd build, uptime, load, memory and disk headroom, every
/// workspace with its turn-lock state, jobs and resource share; `resources` is the sampler's
/// cgroup-aware view (`resources.rs`) for Settings → Machine.
pub async fn box_state(sup: &Supervisor) -> Value {
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let loadavg = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let load: Vec<f64> = loadavg
        .split_whitespace()
        .take(3)
        .filter_map(|v| v.parse().ok())
        .collect();
    // The sampler's view (cgroup caps inside Daytona; /proc is the host's there).
    let resources = crate::resources::snapshot();
    let mut workspaces = Vec::new();
    for w in sup.list().await {
        workspaces.push(json!({
            "project_id": w.project_id,
            "name": w.agent.name(),
            "handle": w.agent.identity.handle(),
            "healthy": w.agent.is_healthy(),
            "started_at": w.started_at,
            "run_as": w.cfg.run_as.as_ref().map(|r| r.user.clone()),
            "turn": turn_json(&w.agent).await,
            "jobs_running": w.agent.jobs_running.load(std::sync::atomic::Ordering::SeqCst),
            "usage": resources.workspace(&w.project_id),
            "links": w.sync.link_summary().await,
        }));
    }
    let now = now_ms();
    let warnings = warn_ring().lock().map(|r| r.len()).unwrap_or(0);
    // `memory_mb` keeps its shape for older readers but carries the box's own cap once the
    // sampler has it (inside Daytona /proc/meminfo is the 755 GiB host).
    let memory_mb = match &resources.memory {
        Some(m) => {
            json!({ "total": m.total_mb, "available": m.total_mb.saturating_sub(m.used_mb), "source": m.source })
        }
        None => json!({
            "total": meminfo_kb("MemTotal:", &meminfo).map(|k| k / 1024),
            "available": meminfo_kb("MemAvailable:", &meminfo).map(|k| k / 1024),
            "source": "proc",
        }),
    };
    json!({
        "device_id": sup.cfg.device_id,
        "user_id": sup.cfg.user_id,
        "agentd_version": env!("CARGO_PKG_VERSION"),
        "agentd_sha": std::env::var("ARBOS_AGENTD_SHA").ok().filter(|v| !v.is_empty() && v != "unknown"),
        "image_version": std::env::var("ARBOS_IMAGE_VERSION").ok().filter(|v| !v.is_empty() && v != "unknown"),
        "booted_at": sup.booted_at,
        "uptime_s": (now - sup.booted_at).max(0) / 1000,
        "local_box": sup.cfg.local_box,
        "isolation": { "enforced": sup.isolation.enabled, "cross_read_blocked": *sup.cross_read_blocked.read().await },
        "load": load,
        "load_scope": resources.load_scope,
        "memory_mb": memory_mb,
        "disk": disk_json(&sup.cfg.work_root),
        "resources": resources,
        "held": sup.held.read().await.iter().cloned().collect::<Vec<_>>(),
        "warnings_in_ring": warnings,
        "workspaces": workspaces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str, at: i64, kind: &str) -> Event {
        Event {
            id: id.into(),
            at,
            kind: kind.into(),
            chat_id: None,
            role: None,
            text: String::new(),
            data: None,
            source: "t3",
        }
    }

    fn q(limit: usize, kinds: &[&str]) -> Query {
        Query {
            since: 0,
            chat: None,
            kinds: kinds.iter().map(|k| k.to_string()).collect(),
            limit,
        }
    }

    #[test]
    fn page_cut_never_splits_a_millisecond() {
        let events = vec![
            ev("a", 10, "user"),
            ev("b", 20, "assistant"),
            ev("c", 20, "tool_call"),
            ev("d", 30, "tool_output"),
        ];
        let (page, next, truncated) = cut(events, &q(2, &[]));
        // Two fit, but the second shares `at` with a third: the page stops before 20.
        assert_eq!(
            page.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["a"]
        );
        assert_eq!(next, 10);
        assert!(truncated);
    }

    #[test]
    fn page_cut_keeps_a_single_millisecond_page() {
        let events = vec![
            ev("a", 20, "user"),
            ev("b", 20, "assistant"),
            ev("c", 20, "x"),
        ];
        let (page, next, _) = cut(events, &q(2, &[]));
        assert_eq!(page.len(), 2);
        assert_eq!(next, 20);
    }

    #[test]
    fn kind_filter_knows_groups() {
        let events = vec![
            ev("a", 1, "user"),
            ev("b", 2, "gate"),
            ev("c", 3, "secret_requested"),
            ev("d", 4, "warn"),
            ev("e", 5, "turn"),
        ];
        let (page, _, _) = cut(events.clone(), &q(10, &["gate", "warnings"]));
        assert_eq!(
            page.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["b", "d"]
        );
        let (page, _, _) = cut(events.clone(), &q(10, &["turns"]));
        assert_eq!(
            page.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["a", "e"]
        );
        let (page, _, _) = cut(events, &q(10, &["secrets"]));
        assert_eq!(
            page.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["c"]
        );
    }

    #[test]
    fn query_bounds_window_and_limit() {
        let mut p = std::collections::HashMap::new();
        p.insert("since".to_owned(), "1".to_owned());
        p.insert("limit".to_owned(), "99999".to_owned());
        p.insert("kind".to_owned(), "Gate, Turns".to_owned());
        let q = Query::from_params(&p);
        assert!(q.since >= now_ms() - MAX_WINDOW_MS - 1000);
        assert_eq!(q.limit, MAX_LIMIT);
        assert!(q.kinds.contains("gate") && q.kinds.contains("turns"));
    }

    #[test]
    fn audit_tail_scrubs_and_filters() {
        let dir = std::env::temp_dir().join(format!("arbos-inspect-{}", arbos_core::ids::ulid()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audit.jsonl");
        std::fs::write(
            &path,
            "{\"ts\":5,\"kind\":\"old\"}\n{\"ts\":50,\"kind\":\"egress\",\"chat_id\":\"c1\",\"host\":\"api.example\",\"note\":\"Authorization: Bearer abc.def\"}\n",
        )
        .unwrap();
        let events = audit_after(&path, 10);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "egress");
        assert_eq!(events[0].chat_id.as_deref(), Some("c1"));
        let data = events[0].data.as_ref().unwrap();
        assert_eq!(data["note"], "Authorization: Bearer [REDACTED:bearer]");
        assert!(data.get("ts").is_none());
    }

    #[test]
    fn warn_ring_feeds_events() {
        if let Ok(mut r) = warn_ring().lock() {
            r.clear();
            r.push_back(json!({ "at": 100, "level": "warn", "target": "t", "message": "x ghp_abcdefghijklmnopqrstuvwxyz", "fields": {}, "project_id": "p1" }));
            r.push_back(json!({ "at": 101, "level": "error", "target": "t", "message": "y", "fields": {}, "project_id": "p2" }));
            r.push_back(
                json!({ "at": 102, "level": "warn", "target": "t", "message": "z", "fields": {} }),
            );
        }
        let mine = warnings_after(50, Some("p1"));
        assert_eq!(mine.len(), 2, "own project + untagged lines");
        assert!(mine[0].text.contains("[REDACTED:token]"));
        assert_eq!(mine[1].kind, "warn");
    }
}
