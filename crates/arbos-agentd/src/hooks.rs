//! Loopback hook API for `arbos-pi-ext` (sandbox-image.md §3.3): route validation,
//! ask_user, handles, working set, scratchpad.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::agent::Agent;

type S = Arc<Agent>;

fn authed(agent: &Agent, headers: &HeaderMap) -> bool {
    let tok = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    tok == agent.cfg.mcp_token || tok == agent.cfg.subagent_mcp_token()
}

fn ok() -> Response {
    Json(json!({ "ok": true })).into_response()
}

fn err(e: impl std::fmt::Display) -> Response {
    Json(json!({ "ok": false, "error": e.to_string() })).into_response()
}

async fn route(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // Only the main session routes; a subagent's call must never touch the running turn's state.
    if b.get("role").and_then(Value::as_str).unwrap_or("main") != "main" {
        return err("route is only for the main session");
    }
    match a.hook_route(b.get("args").unwrap_or(&Value::Null)).await {
        Ok(()) => ok(),
        Err(e) => err(e),
    }
}

async fn ask_user(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let q = b
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let sid = b
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    match a.hook_ask_user(&q, b.get("widget"), &sid).await {
        Ok(id) => Json(json!({ "ok": true, "question_id": id })).into_response(),
        Err(e) => err(e),
    }
}

/// `send_message(text, activity?, to?)`: progress bubble mid-turn (no `to`), or a
/// cross-subnet message to another agent when `to` is set — or when `text` is the
/// antipattern `@Handle words` (feedback app-01M5NX3GHC37Y8FH3KKR0PRWG1).
async fn start_job(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let s = |k: &str| b.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let touches: Vec<String> = b
        .get("touches")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    match a
        .hook_start_job(&s("title"), &s("brief"), &s("kind"), touches)
        .await
    {
        Ok(v) => Json(json!({ "ok": true, "job": v })).into_response(),
        Err(e) => err(e),
    }
}

async fn steer_job(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let s = |k: &str| b.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    match a.hook_steer_job(&s("job_id"), &s("text")).await {
        Ok(v) => Json(json!({ "ok": true, "job": v })).into_response(),
        Err(e) => err(e),
    }
}

async fn stop_job(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let job_id = b
        .get("job_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    match a.hook_stop_job(&job_id).await {
        Ok(v) => Json(json!({ "ok": true, "result": v })).into_response(),
        Err(e) => err(e),
    }
}

async fn list_jobs(State(a): State<S>, headers: HeaderMap) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match a.hook_list_jobs().await {
        Ok(v) => Json(json!({ "ok": true, "jobs": v["jobs"] })).into_response(),
        Err(e) => err(e),
    }
}

async fn send_message(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let text = b.get("text").and_then(Value::as_str).unwrap_or("");
    let activity = b
        .get("activity")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let to = b
        .get("to")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    // Explicit `to`, or `@Handle words` mistyped as a progress note in this chat.
    let (agent_to, agent_text) = match to {
        Some(t) => (Some(t), text.to_owned()),
        None => match crate::agent_messages::mention_address(text) {
            Some((handle, msg)) => (Some(handle), msg),
            None => (None, String::new()),
        },
    };
    if let Some(to) = agent_to {
        return match a.send_message_from_turn(Some(&to), &agent_text).await {
            Ok(note) => Json(json!({ "ok": true, "note": note })).into_response(),
            Err(e) => err(e),
        };
    }
    let reply_to = b
        .get("reply_to")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match a.hook_send_message(text, activity, reply_to).await {
        Ok(()) => ok(),
        Err(e) => err(e),
    }
}

/// Risky harness tool calls (feature `approve`): `{ tool, input, session_id, role }` →
/// `{ state: "allowed" }` or `{ state: "pending", approvalId }` for the extension to poll.
async fn approval(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let str_of = |k: &str| b.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let input = b.get("input").cloned().unwrap_or(Value::Null);
    let role = b
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("main")
        .to_owned();
    Json(
        a.hook_approval(&str_of("tool"), &input, &str_of("session_id"), &role)
            .await,
    )
    .into_response()
}

async fn approval_status(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let id = b.get("approvalId").and_then(Value::as_str).unwrap_or("");
    Json(a.approval_status(id).await).into_response()
}

async fn approval_abandon(
    State(a): State<S>,
    headers: HeaderMap,
    Json(b): Json<Value>,
) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let id = b.get("approvalId").and_then(Value::as_str).unwrap_or("");
    Json(a.abandon_approval(id).await).into_response()
}

async fn deny(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let sid = b
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let _ = a.store.t3_append(
        &sid,
        None,
        Some(&sid),
        "deny",
        Some("system"),
        &format!("denied tool call: {b}"),
        None,
    );
    tracing::warn!(deny = %b, "deny-list hit");
    ok()
}

async fn handle(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let sid = b
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let h = b.get("handle").cloned().unwrap_or(Value::Null);
    let id = h.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
    let path = h
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if id.is_empty() || path.is_empty() {
        return err("handle needs id and path");
    }
    let blob_key = std::fs::read(&path)
        .ok()
        .map(|bytes| a.upload_blob(bytes, "text/plain"));
    let r = a.store.upsert_handle(
        &id,
        Some(&sid),
        Some(&sid),
        h.get("kind").and_then(Value::as_str).unwrap_or("output"),
        h.get("chars").and_then(Value::as_i64).unwrap_or(0),
        h.get("lines").and_then(Value::as_i64).unwrap_or(0),
        &path,
        h.get("note").and_then(Value::as_str).unwrap_or(""),
        blob_key.as_deref(),
    );
    match r {
        Ok(()) => ok(),
        Err(e) => err(e),
    }
}

async fn handle_path(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let id = b.get("handle").and_then(Value::as_str).unwrap_or("");
    match a.store.handle(id) {
        Ok(Some(h)) => Json(json!({ "ok": true, "text": h.path })).into_response(),
        _ => err("unknown handle"),
    }
}

async fn working_set(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let sid = b
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let chat_id = a
        .turn
        .lock()
        .await
        .as_ref()
        .map(|t| t.chat_id.clone())
        .unwrap_or(sid);
    // Drop zombie `running` subagents (task gone, status never cleared) before
    // the model reads the working set — otherwise a terminated pod keeps the
    // agent thinking work is still in flight and pins the client's indicator.
    a.reclaim_orphan_subagents(false).await;
    // `skills` is its own prompt section (F5): the working-set line cap must not cut it.
    let skills = a.skills_section_text().await;
    // D87: project memory stays in the (debounced) system prompt; `memory` is reserved for a
    // later experiment and empty today.
    Json(json!({ "ok": true, "text": a.working_set_text(&chat_id), "skills": skills, "memory": "", "stable_prefix": a.cfg.stable_prefix() }))
        .into_response()
}

async fn remember(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let sid = b
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let chat_id = a
        .turn
        .lock()
        .await
        .as_ref()
        .map(|t| t.chat_id.clone())
        .unwrap_or(sid);
    let name = b.get("name").and_then(Value::as_str).unwrap_or("");
    let value = b.get("value").and_then(Value::as_str);
    match a.store.scratch_set(&chat_id, name, value) {
        Ok(()) => ok(),
        Err(e) => err(e),
    }
}

async fn noop(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    tracing::debug!(hook = %b, "hook event");
    ok()
}

async fn turn_without_route(
    State(a): State<S>,
    headers: HeaderMap,
    Json(b): Json<Value>,
) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let corrected = b.get("corrected").and_then(Value::as_bool).unwrap_or(false);
    let sid = b
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    a.hook_turn_without_route(&sid, corrected).await;
    ok()
}

async fn route_corrected(State(a): State<S>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&a, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    tracing::info!(action = ?b.get("action"), "route recorded by the post-hoc guard");
    if let Some(t) = a.turn.lock().await.as_mut() {
        t.metrics.route_corrected = true;
    }
    ok()
}

/// Memory tree (D77) node counts, build cursor and today's spend; for operators (`arbosctl`,
/// the roll-out check). Same bearer as the other hooks.
async fn tree_stats(State(a): State<S>, headers: HeaderMap) -> Response {
    if !authed(&a, &headers) {
        return err("unauthorized");
    }
    let mut v = a.tree.stats();
    v["mode"] = json!(a.tree_mode().as_str());
    v["spend"] = a.tree.spend_report();
    Json(v).into_response()
}

pub fn router(agent: Arc<Agent>) -> Router {
    Router::new()
        .route("/hook/route", post(route))
        .route("/hook/ask_user", post(ask_user))
        .route("/hook/send_message", post(send_message))
        .route("/hook/start_job", post(start_job))
        .route("/hook/list_jobs", post(list_jobs))
        .route("/hook/steer_job", post(steer_job))
        .route("/hook/stop_job", post(stop_job))
        .route("/hook/approval", post(approval))
        .route("/hook/approval-status", post(approval_status))
        .route("/hook/approval-abandon", post(approval_abandon))
        .route("/hook/deny", post(deny))
        .route("/hook/handle", post(handle))
        .route("/hook/handle-path", post(handle_path))
        .route("/hook/working-set", post(working_set))
        .route("/hook/remember", post(remember))
        .route("/hook/compacted", post(noop))
        .route("/hook/turn-without-route", post(turn_without_route))
        .route("/hook/route-corrected", post(route_corrected))
        .route("/hook/tree-stats", axum::routing::get(tree_stats))
        .with_state(agent)
}
