//! Connector logs the owner can read (spec §13.3, 2026-10-28): the sidecar on the agent's
//! machine uploads its text-free `session.jsonl` entries (wakes, acks, replies, latencies,
//! session events, errors) and the owner reads them for the Details pane's "Session log" card.
//!
//! Agent side: `POST /asp/log {entries:[…]}` (bearer key). Owner side:
//! `GET /outside-agents/{id}/log?since=&limit=`. Nothing here ever stores message text: keys
//! that could carry it are dropped on the way in.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::ids::now_ms;

use crate::api::{internal, ApiResult};
use crate::auth::Principal;
use crate::outside::AgentPrincipal;
use crate::signin::api_error;
use crate::state::AppState;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS asp_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL, at INTEGER NOT NULL,
  payload TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS asp_log_project ON asp_log(project_id, id);
"#;

/// Entries per upload call.
const MAX_ENTRIES: usize = 100;
/// Bytes per entry after cleaning.
const MAX_ENTRY_BYTES: usize = 2048;
/// Entries kept per agent.
const KEEP_PER_AGENT: i64 = 2000;
/// Days kept.
const KEEP_MS: i64 = 7 * 24 * 3600 * 1000;

/// Keys that could carry content or secrets; never stored whatever the sidecar sends.
const DROP_KEYS: &[&str] = &[
    "text",
    "prompt",
    "key",
    "token",
    "auth",
    "authorization",
    "secret",
    "body",
    "url",
];

/// Every field the owner may see. `kind` is required; the rest is kept as sent, minus dropped
/// keys, cut to [`MAX_ENTRY_BYTES`].
fn clean(entry: &Value) -> Option<Value> {
    let obj = entry.as_object()?;
    let kind = obj.get("kind")?.as_str()?;
    if kind.is_empty() || kind.len() > 40 {
        return None;
    }
    let mut out = serde_json::Map::new();
    for (k, v) in obj {
        let lk = k.to_ascii_lowercase();
        if DROP_KEYS.iter().any(|d| lk.contains(d)) {
            continue;
        }
        out.insert(k.clone(), v.clone());
    }
    if !out.contains_key("at") {
        out.insert("at".into(), json!(now_ms()));
    }
    let v = Value::Object(out);
    if v.to_string().len() > MAX_ENTRY_BYTES {
        return None;
    }
    Some(v)
}

impl crate::db::Db {
    pub fn insert_asp_log(&self, project_id: &str, entries: &[Value]) -> anyhow::Result<i64> {
        self.with(|c| {
            let mut last = 0;
            for e in entries {
                let at = e["at"].as_i64().unwrap_or_else(now_ms);
                c.execute(
                    "INSERT INTO asp_log(project_id, at, payload) VALUES (?1, ?2, ?3)",
                    params![project_id, at, e.to_string()],
                )?;
                last = c.last_insert_rowid();
            }
            // Keep the newest KEEP_PER_AGENT rows of this agent.
            c.execute(
                "DELETE FROM asp_log WHERE project_id = ?1 AND id NOT IN \
                 (SELECT id FROM asp_log WHERE project_id = ?1 ORDER BY id DESC LIMIT ?2)",
                params![project_id, KEEP_PER_AGENT],
            )?;
            Ok(last)
        })
    }

    pub fn asp_log_after(
        &self,
        project_id: &str,
        after: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<(i64, Value)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, payload FROM asp_log WHERE project_id = ?1 AND id > ?2 ORDER BY id ASC LIMIT ?3",
            )?;
            let rows = st
                .query_map(params![project_id, after, limit], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        serde_json::from_str(&r.get::<_, String>(1)?).unwrap_or(Value::Null),
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Newest replies for the card header: `(entries, last_at, last_reply_ms)`.
    pub fn asp_log_summary(&self, project_id: &str) -> anyhow::Result<Value> {
        self.with(|c| {
            let n: i64 = c.query_row(
                "SELECT COUNT(*) FROM asp_log WHERE project_id = ?1",
                params![project_id],
                |r| r.get(0),
            )?;
            let last_at: Option<i64> = c
                .query_row(
                    "SELECT MAX(at) FROM asp_log WHERE project_id = ?1",
                    params![project_id],
                    |r| r.get(0),
                )
                .ok()
                .flatten();
            let mut st = c.prepare(
                "SELECT payload FROM asp_log WHERE project_id = ?1 ORDER BY id DESC LIMIT 50",
            )?;
            let payloads: Vec<String> = st
                .query_map(params![project_id], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let last_reply_ms = payloads
                .iter()
                .filter_map(|p| serde_json::from_str::<Value>(p).ok())
                .find(|v| v["kind"] == "reply")
                .and_then(|v| v["ms"]["event_to_reply"].as_i64());
            Ok(json!({ "entries": n, "last_at": last_at, "last_reply_ms": last_reply_ms }))
        })
    }

    pub fn prune_asp_log(&self) -> anyhow::Result<usize> {
        self.with(|c| {
            c.execute(
                "DELETE FROM asp_log WHERE at < ?1",
                params![now_ms() - KEEP_MS],
            )
        })
    }
}

#[derive(Deserialize)]
pub struct UploadBody {
    #[serde(default)]
    pub entries: Vec<Value>,
}

/// `POST /asp/log` — the sidecar's text-free session log, up to 100 entries per call.
pub async fn upload(
    State(state): State<AppState>,
    me: AgentPrincipal,
    Json(b): Json<UploadBody>,
) -> Response {
    if b.entries.len() > MAX_ENTRIES {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "at most 100 entries per call",
        );
    }
    let cleaned: Vec<Value> = b.entries.iter().filter_map(clean).collect();
    let head = if cleaned.is_empty() {
        0
    } else {
        match state.db.insert_asp_log(&me.project.id, &cleaned) {
            Ok(h) => h,
            Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
        }
    };
    let _ = state.db.touch_outside_agent(&me.project.id, None);
    Json(json!({ "ok": true, "stored": cleaned.len(), "head": head })).into_response()
}

#[derive(Deserialize)]
pub struct LogQuery {
    #[serde(default)]
    pub since: i64,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// `GET /outside-agents/{id}/log?since=&limit=` — the owner reads the agent's session log,
/// oldest first from `since`.
pub async fn read(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Query(q): Query<LogQuery>,
) -> ApiResult {
    let (_, project) = crate::outside::owned_for(&state, &principal, &id)?;
    let limit = q.limit.unwrap_or(200).clamp(1, 500);
    let rows = state
        .db
        .asp_log_after(&project.id, q.since, limit)
        .map_err(internal)?;
    let next = rows.last().map(|(id, _)| *id).unwrap_or(q.since);
    let entries: Vec<Value> = rows
        .into_iter()
        .map(|(id, mut v)| {
            v["id"] = json!(id);
            v
        })
        .collect();
    Ok(Json(json!({ "entries": entries, "next": next })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_drops_content_and_secret_keys() {
        let e = json!({ "kind": "reply", "chat": "c1", "text": "hello", "prompt": "p",
            "Authorization": "Bearer x", "webhook_url": "https://x", "ms": { "event_to_reply": 9603 } });
        let c = clean(&e).unwrap();
        assert_eq!(c["kind"], "reply");
        assert_eq!(c["ms"]["event_to_reply"], 9603);
        for k in ["text", "prompt", "Authorization", "webhook_url"] {
            assert!(c.get(k).is_none(), "{k}");
        }
        assert!(c["at"].is_i64());
        assert!(clean(&json!({ "chat": "c1" })).is_none(), "kind required");
        let big = json!({ "kind": "x", "pad": "y".repeat(3000) });
        assert!(clean(&big).is_none());
    }

    /// Upload → owner read, oldest first with a cursor; the summary names the last reply.
    #[test]
    fn upload_and_read_roundtrip() {
        use crate::people::tests_support::{person, project_of};
        use crate::state::test_support::test_state;
        use crate::state::AuthMode;
        let state = test_state(AuthMode::Github);
        let (owner, _) = person(&state, 7401);
        let pid = project_of(&state, &owner, "Log Bot");
        let entries: Vec<Value> = [
            json!({ "kind": "wake", "chat": "c1", "ms": { "event_to_nudge": 23 } }),
            json!({ "kind": "ack", "chat": "c1", "ms": { "nudge_to_ack": 1180 } }),
            json!({ "kind": "reply", "chat": "c1", "ms": { "ack_to_reply": 8400, "event_to_reply": 9603 }, "text": "never stored" }),
        ]
        .iter()
        .filter_map(clean)
        .collect();
        let head = state.db.insert_asp_log(&pid, &entries).unwrap();
        assert_eq!(head, 3);
        let page = state.db.asp_log_after(&pid, 0, 2).unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].1["kind"], "wake");
        let rest = state.db.asp_log_after(&pid, page[1].0, 10).unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].1["kind"], "reply");
        assert!(rest[0].1.get("text").is_none());
        let s = state.db.asp_log_summary(&pid).unwrap();
        assert_eq!(s["entries"], 3);
        assert_eq!(s["last_reply_ms"], 9603);
        assert_eq!(state.db.prune_asp_log().unwrap(), 0);
    }
}
