//! HTTP surface of the Matrix mirror.
//!
//! Inbound from Synapse (the relay *is* the three appservices, design §2.6): transactions,
//! user/alias existence queries, ping. Stage A acknowledges transactions and counts them; the
//! fan-out to boxes (agents) and bridges comes with stages C2/C3. The push gateway endpoint
//! (`POST /_matrix/push/v1/notify`) exists for the same reason: it proves the path while today's
//! pushes keep flowing (§4 stage A.5).
//!
//! Admin (`ARBOS_RELAY_ADMIN_TOKEN`): status, backfill, retry of parked jobs, the per-user read
//! flip (stage B).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

use super::Appservice;
use super::{backfill, shadow};
use crate::auth::Principal;
use crate::signin::api_error;
use crate::state::AppState;

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
}

/// Which appservice Synapse is talking to, from the `hs_token` it presents (header, or the
/// legacy `?access_token=` query).
fn caller(
    state: &AppState,
    headers: &HeaderMap,
    q: &HashMap<String, String>,
) -> Result<Appservice, Box<Response>> {
    let token = bearer(headers).or_else(|| q.get("access_token").map(String::as_str));
    match token.and_then(|t| state.matrix.cfg.appservice_for_hs_token(t)) {
        Some(a) => Ok(a),
        None => Err(Box::new(
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "errcode": "M_UNKNOWN_TOKEN", "error": "bad hs_token" })),
            )
                .into_response(),
        )),
    }
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "errcode": "M_NOT_FOUND", "error": "not found" })),
    )
        .into_response()
}

/// `PUT /_matrix/app/v1/transactions/{txn}` — events for rooms our users are in. Idempotent on
/// the transaction id (Synapse retries until it gets a 200).
pub async fn transaction(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(txn): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    body: axum::body::Bytes,
) -> Response {
    let who = match caller(&state, &headers, &q) {
        Ok(a) => a,
        Err(r) => return *r,
    };
    let v: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let events = v["events"].as_array().map(Vec::len).unwrap_or(0) as i64;
    let ephemeral = v["ephemeral"].as_array().map(Vec::len).unwrap_or(0);
    let to_device = v["to_device"].as_array().map(Vec::len).unwrap_or(0);
    match state.db.matrix_txn_mark(who.id(), &txn, events) {
        Ok(true) => {
            tracing::debug!(appservice = who.id(), txn = %txn, events, ephemeral, to_device, "matrix transaction");
        }
        Ok(false) => {}
        Err(e) => tracing::warn!(error = %e, "matrix transaction: record failed"),
    }
    Json(json!({})).into_response()
}

/// `GET /_matrix/app/v1/users/{userId}` — Synapse asks before auto-creating a namespace user we
/// have not registered. The relay registers every user itself, so the answer is always "no".
pub async fn user_query(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    Path(_user): Path<String>,
) -> Response {
    if let Err(r) = caller(&state, &headers, &q) {
        return *r;
    }
    not_found()
}

/// `GET /_matrix/app/v1/rooms/{alias}` — no aliases are served yet (bridge portals come with C3).
pub async fn room_query(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    Path(_alias): Path<String>,
) -> Response {
    if let Err(r) = caller(&state, &headers, &q) {
        return *r;
    }
    not_found()
}

/// `POST /_matrix/app/v1/ping` (MSC2659).
pub async fn ping(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Err(r) = caller(&state, &headers, &q) {
        return *r;
    }
    Json(json!({})).into_response()
}

/// `POST /_matrix/push/v1/notify` — the relay as push gateway. Devices of users whose reads
/// are flipped (stage B) get their APNs alert from here; everything else is accepted and
/// ignored (today's `chat_events` pushes keep flowing).
pub async fn push_notify(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let v: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let devices = v["notification"]["devices"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    tracing::debug!(
        room = v["notification"]["room_id"].as_str().unwrap_or(""),
        devices,
        "matrix push notify"
    );
    let rejected = super::reads::on_notify(&state, &v).await;
    Json(json!({ "rejected": rejected })).into_response()
}

// ---- admin ----------------------------------------------------------------------------------------

fn admin_only(principal: &Principal) -> Result<(), Box<Response>> {
    match principal {
        Principal::Admin => Ok(()),
        Principal::Device { .. } | Principal::Box { .. } => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "admin only",
        ))),
    }
}

/// `GET /admin/matrix/status[?failed=1]`.
pub async fn status(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let mut v = state.matrix.status_json(&state);
    if state.matrix.cfg.mode.writes() {
        let client = super::client::Client::new(state.http.clone(), state.matrix.cfg.clone());
        v["homeserver_versions"] = match client.versions().await {
            Ok(vs) => json!(vs),
            Err(e) => json!({ "error": e.to_string() }),
        };
    }
    if q.get("failed").is_some_and(|f| f == "1") {
        v["failed_jobs"] = json!(state.db.matrix_outbox_failed_rows(100).unwrap_or_default());
    }
    v["reads_users"] = json!(state.db.matrix_reads_users().unwrap_or_default());
    Json(v).into_response()
}

#[derive(Deserialize, Default)]
pub struct BackfillBody {
    /// Only this user's chats (people chats they are in, projects they own).
    #[serde(default)]
    pub user_id: Option<String>,
    /// Only this chat (people chat id, or `project_id` + `chat_id`).
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    /// Queue rooms and members only, no message history.
    #[serde(default)]
    pub rooms_only: bool,
}

/// `POST /admin/matrix/backfill {user_id?, chat_id?, project_id?, rooms_only?}` — queue every
/// existing chat (or a slice) into the outbox: room, members, then history oldest-first with
/// timestamp massaging. Idempotent: events that already exist are skipped by the worker.
pub async fn backfill(
    State(state): State<AppState>,
    principal: Principal,
    body: Option<Json<BackfillBody>>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    if !state.matrix.cfg.mode.writes() {
        return api_error(StatusCode::CONFLICT, "matrix_off", "ARBOS_MATRIX is off");
    }
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let report = backfill::run(
        &state,
        backfill::Scope {
            user_id: b.user_id,
            chat_id: b.chat_id,
            project_id: b.project_id,
            rooms_only: b.rooms_only,
        },
    );
    state.matrix.wake.notify_one();
    Json(json!(report)).into_response()
}

/// `GET /admin/matrix/shadow[?chat=<id>&unread=1 | ?store=1 | ?last=1]` — shadow-compare (stage B
/// gate): per mirrored chat, message ids and members on both sides; with `unread=1` also per-user
/// unread counts. `store=1` runs the nightly shape and keeps the report; `last=1` returns the
/// newest stored report.
pub async fn shadow_compare(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    if !state.matrix.cfg.mode.writes() {
        return api_error(StatusCode::CONFLICT, "matrix_off", "ARBOS_MATRIX is off");
    }
    if q.get("last").is_some_and(|l| l == "1") {
        return match shadow::last_report(&state) {
            Some(v) => Json(v).into_response(),
            None => api_error(StatusCode::NOT_FOUND, "not_found", "no nightly report yet"),
        };
    }
    if q.get("store").is_some_and(|l| l == "1") {
        let rep = shadow::run_and_store(&state).await;
        return Json(shadow::to_json(&rep)).into_response();
    }
    let rep = shadow::run(
        &state,
        q.get("chat").map(String::as_str),
        q.get("unread").is_some_and(|u| u == "1"),
    )
    .await;
    Json(shadow::to_json(&rep)).into_response()
}

/// `POST /admin/matrix/retry` — put parked jobs back in the queue.
pub async fn retry_failed(State(state): State<AppState>, principal: Principal) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    match state.db.matrix_outbox_retry_failed() {
        Ok(n) => {
            state.matrix.wake.notify_one();
            Json(json!({ "requeued": n })).into_response()
        }
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct ReadsBody {
    pub on: bool,
}

/// `POST /admin/matrix/pushers` — register the relay as push gateway on every live device of
/// every user with a Matrix identity (one-time backfill for devices minted before channel
/// pushes existed; new devices register at `GET /me/matrix`). Answers how many users were queued.
pub async fn backfill_pushers(State(state): State<AppState>, principal: Principal) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let users = match state.db.matrix_device_users() {
        Ok(u) => u,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    for u in &users {
        state.matrix.pushers(&state, u, true);
    }
    Json(json!({ "queued_users": users.len() })).into_response()
}

/// `PUT /admin/matrix/reads/{user_id} {on}` — the per-user read flip (stage B). Takes effect on
/// the user's next request; rollback is the same call with `on: false`.
pub async fn set_reads(
    State(state): State<AppState>,
    principal: Principal,
    Path(user_id): Path<String>,
    Json(b): Json<ReadsBody>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    match state.db.set_matrix_reads(&user_id, b.on) {
        Ok(true) => {
            // Pushers stay registered either way since channel pushes ride on them (reads.rs);
            // a flip off only changes what `on_notify` does with a mirrored chat.
            state.matrix.pushers(&state, &user_id, true);
            Json(json!({ "user_id": user_id, "matrix_reads": b.on, "mode": state.matrix.cfg.mode.as_str() }))
                .into_response()
        }
        Ok(false) => api_error(StatusCode::NOT_FOUND, "not_found", "no such user"),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}
