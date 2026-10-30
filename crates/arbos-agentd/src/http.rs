//! The box's HTTP surfaces: `:7788` (box-level: `/healthz`, `/browser/stream`; per workspace:
//! `/w/{project}/history`, `/w/{project}/metrics`, `/w/{project}/t3/search`) and each workspace's
//! loopback model stub that forwards to the relay proxy with the box token.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde_json::json;

use crate::agent::Agent;
use crate::workspace::Supervisor;

type S = Arc<Agent>;
type B = Arc<Supervisor>;

/// Box health: every workspace with its pi/extension state. 503 until at least one workspace runs
/// (or the relay listed none yet).
async fn healthz(State(b): State<B>) -> Response {
    let ws = b.list().await;
    let mut items = Vec::new();
    let mut all_ok = true;
    for w in &ws {
        let ok = w.agent.is_healthy();
        all_ok &= ok;
        items.push(json!({
            "project": w.project_id,
            "ok": ok,
            "user": w.cfg.run_as.as_ref().map(|r| r.user.clone()),
            "pi": w.agent.pi_ready.load(std::sync::atomic::Ordering::SeqCst),
            "pi_parked": w.agent.pi_parked.load(std::sync::atomic::Ordering::SeqCst),
            "extension": w.agent.ext_loaded.load(std::sync::atomic::Ordering::SeqCst),
            "mcp_port": w.cfg.mcp_port,
            "started_at": w.started_at,
        }));
    }
    let body = json!({
        "ok": all_ok, "user": b.cfg.user_id, "device": b.cfg.device_id, "version": env!("CARGO_PKG_VERSION"), "booted_at": b.booted_at,
        "isolation": { "enforced": b.isolation.enabled, "cross_read_blocked": *b.cross_read_blocked.read().await },
        "workspaces": items,
    });
    (
        if all_ok {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(body),
    )
        .into_response()
}

async fn metrics(State(b): State<B>, headers: HeaderMap, Path(project): Path<String>) -> Response {
    if !box_admin_authed(&b, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match b.get(&project).await {
        Some(w) => {
            Json(json!({ "turns": w.store.turn_metrics(200).unwrap_or_default() })).into_response()
        }
        None => (StatusCode::NOT_FOUND, "no such workspace").into_response(),
    }
}

async fn history(
    State(b): State<B>,
    Path((project, chat_id)): Path<(String, String)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(w) = b.get(&project).await else {
        return (StatusCode::NOT_FOUND, "no such workspace").into_response();
    };
    let limit = q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(50);
    match w
        .store
        .t3_thread_before(&chat_id, q.get("before").map(String::as_str), limit)
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Pre-D70 clients ask `/history/{chat}` without a project: find the workspace that owns the chat.
async fn history_legacy(
    State(b): State<B>,
    Path(chat_id): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(w) = b.find_chat(&chat_id).await else {
        return (StatusCode::NOT_FOUND, "no workspace knows this chat").into_response();
    };
    let limit = q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(50);
    match w
        .store
        .t3_thread_before(&chat_id, q.get("before").map(String::as_str), limit)
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn t3_search(
    State(b): State<B>,
    headers: HeaderMap,
    Path(project): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if !box_admin_authed(&b, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(w) = b.get(&project).await else {
        return (StatusCode::NOT_FOUND, "no such workspace").into_response();
    };
    let query = q.get("q").cloned().unwrap_or_default();
    match w.store.t3_search(&query, None, None, 20, 0) {
        Ok(hits) => Json(hits.into_iter().map(|(r, s, snip)| json!({ "id": r.id, "thread_id": r.thread_id, "kind": r.kind, "score": -s, "snippet": snip })).collect::<Vec<_>>()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// S16: proxy a workspace's agent-browser JPEG stream (`ws://127.0.0.1:<9223+n>`) on
/// `/w/{project}/browser/stream`; the bare `/browser/stream` is the first workspace (legacy).
async fn browser_stream_ws(
    ws: WebSocketUpgrade,
    State(b): State<B>,
    Path(project): Path<String>,
) -> Response {
    let Some(w) = b.get(&project).await else {
        return (StatusCode::NOT_FOUND, "no such workspace").into_response();
    };
    let url = format!("ws://127.0.0.1:{}", w.cfg.browser_stream_port);
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = pipe_browser(socket, &url).await {
            tracing::debug!(error = %e, project, "browser stream closed");
        }
    })
}

async fn browser_stream(ws: WebSocketUpgrade, State(b): State<B>) -> Response {
    let url = match std::env::var("ARBOS_BROWSER_STREAM_URL") {
        Ok(u) => u,
        Err(_) => match b.list().await.first() {
            Some(w) => format!("ws://127.0.0.1:{}", w.cfg.browser_stream_port),
            None => "ws://127.0.0.1:9223".to_owned(),
        },
    };
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = pipe_browser(socket, &url).await {
            tracing::debug!(error = %e, "browser stream closed");
        }
    })
}

async fn pipe_browser(client: WebSocket, upstream_url: &str) -> anyhow::Result<()> {
    let (upstream, _) = tokio_tungstenite::connect_async(upstream_url).await?;
    let (mut up_tx, mut up_rx) = upstream.split();
    let (mut cl_tx, mut cl_rx) = client.split();
    let to_client = async {
        while let Some(Ok(m)) = up_rx.next().await {
            let out = match m {
                tokio_tungstenite::tungstenite::Message::Binary(b) => Message::Binary(b),
                tokio_tungstenite::tungstenite::Message::Text(t) => {
                    Message::Text(t.as_str().into())
                }
                tokio_tungstenite::tungstenite::Message::Close(_) => break,
                _ => continue,
            };
            if cl_tx.send(out).await.is_err() {
                break;
            }
        }
    };
    let to_upstream = async {
        while let Some(Ok(m)) = cl_rx.next().await {
            let out = match m {
                Message::Binary(b) => tokio_tungstenite::tungstenite::Message::Binary(b),
                Message::Text(t) => {
                    tokio_tungstenite::tungstenite::Message::Text(t.as_str().into())
                }
                Message::Close(_) => break,
                _ => continue,
            };
            if up_tx.send(out).await.is_err() {
                break;
            }
        }
    };
    tokio::select! { _ = to_client => {}, _ = to_upstream => {} }
    Ok(())
}

/// `POST /w/{project}/hold` and `/release` (bearer = the box's MCP token, which the relay holds):
/// the relay stops a workspace while it copies a legacy box's files under it, then releases it.
async fn workspace_hold(
    State(b): State<B>,
    Path(project): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !box_admin_authed(&b, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    b.hold(&project).await;
    Json(json!({ "project": project, "held": true })).into_response()
}

async fn workspace_release(
    State(b): State<B>,
    Path(project): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !box_admin_authed(&b, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    b.release(&project).await;
    Json(json!({ "project": project, "held": false })).into_response()
}

/// `GET /w/{project}/inspect?since=&chat=&kind=&limit=` (bearer = the box's MCP token, which
/// only the relay holds): one bounded window of the agent's recent events (`inspect.rs`).
async fn workspace_inspect(
    State(b): State<B>,
    Path(project): Path<String>,
    headers: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if !box_admin_authed(&b, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(w) = b.get(&project).await else {
        return (StatusCode::NOT_FOUND, "no such workspace").into_response();
    };
    let query = crate::inspect::Query::from_params(&q);
    Json(crate::inspect::page(&w, &query).await).into_response()
}

/// `GET /w/{project}/jobs/{job_id}?after=<seq>` (same bearer): the rollout view's document for
/// one job — brief, entries (prompt | text | tool | steer | result), state, step (`jobs_view.rs`).
async fn workspace_job(
    State(b): State<B>,
    Path((project, job_id)): Path<(String, String)>,
    headers: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if !box_admin_authed(&b, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(w) = b.get(&project).await else {
        return (StatusCode::NOT_FOUND, "no such workspace").into_response();
    };
    let after = q
        .get("after")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let name = w.agent.identity.name();
    match crate::jobs_view::document(&w.store, &project, &name, &job_id, after) {
        Some(doc) => Json(doc).into_response(),
        None => (StatusCode::NOT_FOUND, "no such job").into_response(),
    }
}

/// `GET /box/state` (same bearer): image, build, uptime, load, memory, disk, workspaces.
async fn box_state(State(b): State<B>, headers: HeaderMap) -> Response {
    if !box_admin_authed(&b, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(crate::inspect::box_state(&b).await).into_response()
}

fn box_admin_authed(b: &Supervisor, headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| !b.cfg.mcp_token.is_empty() && t.trim() == b.cfg.mcp_token)
        .unwrap_or(false)
}

pub fn box_router(sup: Arc<Supervisor>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/w/{project}/hold", axum::routing::post(workspace_hold))
        .route(
            "/w/{project}/release",
            axum::routing::post(workspace_release),
        )
        .route("/w/{project}/metrics", get(metrics))
        .route("/w/{project}/inspect", get(workspace_inspect))
        .route("/w/{project}/jobs/{job_id}", get(workspace_job))
        .route("/box/state", get(box_state))
        .route("/w/{project}/history/{chat_id}", get(history))
        .route("/w/{project}/t3/search", get(t3_search))
        .route("/history/{chat_id}", get(history_legacy))
        .route("/w/{project}/browser/stream", get(browser_stream_ws))
        .route("/browser/stream", get(browser_stream))
        .with_state(sup)
}

// ---- loopback model stub (:7791) --------------------------------------------------------------

/// `/v1/messages` → relay Anthropic passthrough; everything else → relay OpenAI-compatible proxy.
async fn stub_forward(
    State(a): State<S>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // SDKs append their own `/v1/...` to a base URL that already ends in `/v1`; collapse it.
    let path = uri.path().replacen("/v1/v1/", "/v1/", 1);
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    tracing::debug!(%path, "model stub forward");
    let target = if path.starts_with("/v1/messages") || path.starts_with("/anthropic/") {
        format!(
            "{}/proxy/anthropic{}{}",
            a.cfg.relay_url,
            path.trim_start_matches("/anthropic"),
            query
        )
    } else {
        format!("{}/proxy{}{}", a.cfg.relay_url, path, query)
    };
    let client = reqwest::Client::new();
    // One box serves every project of the user: the relay meters usage per project by this header.
    let mut req = client
        .request(method, &target)
        .bearer_auth(&a.cfg.box_token)
        .header("x-arbos-project", &a.cfg.project_id)
        .body(body);
    for h in [
        "content-type",
        "accept",
        "anthropic-version",
        "anthropic-beta",
    ] {
        if let Some(v) = headers.get(h) {
            req = req.header(h, v);
        }
    }
    match req.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let mut out_headers = HeaderMap::new();
            for (k, v) in resp.headers() {
                if k == "content-type" || k == "cache-control" {
                    out_headers.insert(k.clone(), v.clone());
                }
            }
            let stream = resp
                .bytes_stream()
                .map(|r| r.map_err(std::io::Error::other));
            (status, out_headers, Body::from_stream(stream)).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, format!("relay unreachable: {e}")).into_response(),
    }
}

pub fn stub_router(agent: Arc<Agent>) -> Router {
    Router::new()
        .route("/{*path}", any(stub_forward))
        .with_state(agent)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The box port is `0.0.0.0` and every workspace's unprivileged user can reach loopback, so
    /// a workspace read route without a bearer is a cross-project read with no credential at
    /// all: `/w/{other}/t3/search` searches another project's whole history. Both debug reads
    /// check the box-admin bearer before they look the workspace up, so an unauthenticated call
    /// cannot even learn whether a project is on this box.
    #[tokio::test]
    async fn a_workspace_debug_read_needs_the_box_bearer() {
        let root = std::env::temp_dir().join(format!("arbos-http-{}", arbos_core::ids::ulid()));
        std::fs::create_dir_all(&root).unwrap();
        let mut cfg = crate::config::BoxConfig::from_env().unwrap();
        cfg.work_root = root.clone();
        cfg.mcp_token = "box-admin-token".into();
        cfg.local_box = true;
        let sup = Supervisor::new(cfg);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, box_router(sup)).await.unwrap();
        });
        let base = format!("http://127.0.0.1:{port}");
        let http = reqwest::Client::new();

        for path in [
            "/w/01OTHERPROJECT/t3/search?q=password",
            "/w/01OTHERPROJECT/metrics",
        ] {
            let r = http.get(format!("{base}{path}")).send().await.unwrap();
            assert_eq!(
                r.status(),
                reqwest::StatusCode::UNAUTHORIZED,
                "{path} answered an unauthenticated caller"
            );
            let r = http
                .get(format!("{base}{path}"))
                .bearer_auth("guessed")
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED, "{path}");
            // With the bearer the route works and reports the missing workspace as missing.
            let r = http
                .get(format!("{base}{path}"))
                .bearer_auth("box-admin-token")
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), reqwest::StatusCode::NOT_FOUND, "{path}");
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
