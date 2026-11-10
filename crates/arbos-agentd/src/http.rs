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
///
/// `state` is the boot answer (2026-11-09): `initializing` while the first listing from the
/// relay is still being applied, `ready` after it, with the workspace counts beside it. `ok` keeps its
/// meaning — the workspaces that *are* running are well — so a reader that only ever asked
/// "is this box up" reads the same thing it always did, and a reader that waited on a box can
/// now tell "still starting" from "started and unwell" instead of inferring it from silence.
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
    let mut body = json!({
        "ok": all_ok, "user": b.cfg.user_id, "device": b.cfg.device_id, "version": env!("CARGO_PKG_VERSION"), "booted_at": b.booted_at,
        "isolation": { "enforced": b.isolation.enabled, "cross_read_blocked": *b.cross_read_blocked.read().await },
        "workspaces": items,
    });
    merge_boot(&mut body, b.boot.json());
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

/// Put the boot fields on a status body. One helper so `/healthz` and `/box/state` cannot drift.
pub(crate) fn merge_boot(body: &mut serde_json::Value, boot: serde_json::Value) {
    let (Some(body), Some(boot)) = (body.as_object_mut(), boot.as_object()) else {
        return;
    };
    for (k, v) in boot {
        body.insert(k.clone(), v.clone());
    }
}

async fn metrics(State(b): State<B>, Path(project): Path<String>) -> Response {
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
    Path(project): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
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

/// Worker threads the box surface gets. Two, not one: a `/w/{project}/history` page or a
/// `/box/state` disk read is a blocking call on this runtime, and health must answer beside it.
const BOX_HTTP_THREADS: usize = 2;

/// Serve the box surface (`/healthz`, `/box/state`, the per-workspace read endpoints) on a
/// runtime of its own, on threads of its own (2026-11-09).
///
/// Boot is the busiest minute the box ever has. Every workspace loads its chat documents — one
/// Loro snapshot per room, read out of SQLite and decoded — starts its children and indexes its
/// skills, and all of that is synchronous work between await points. Served on the main runtime,
/// a `/healthz` or `/box/state` request is not slow during that: it is **not polled at all**
/// until the boot drains, because every worker thread of that runtime is inside the boot. On
/// gVisor, where a box has few cores and every syscall is dearer, that was 9 minutes on one box
/// and 13 on another; the same boxes on Daytona answered in about 3.5 because they had cores to
/// spare. The surface that is supposed to report a slow boot must not be the thing the slow boot
/// silences, and no amount of making the boot cheaper would fix that — it would only move the
/// number.
///
/// The listener is bound by the caller, so a port already taken is still a boot failure with the
/// usual message, and a test can take port 0 and learn which port it got.
pub fn serve_box(
    listener: std::net::TcpListener,
    sup: Arc<Supervisor>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    listener.set_nonblocking(true)?;
    let app = box_router(sup);
    std::thread::Builder::new()
        .name("box-http".to_owned())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(BOX_HTTP_THREADS)
                .thread_name("box-http")
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!(error = %e, "box http runtime");
                    return;
                }
            };
            rt.block_on(async move {
                match tokio::net::TcpListener::from_std(listener) {
                    Ok(l) => {
                        axum::serve(l, app).await.ok();
                    }
                    Err(e) => tracing::error!(error = %e, "box http listener"),
                }
            });
        })
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
    // The agent's own client, so its connection pool is reused: a client per call meant a new
    // connection, and on the relay's real base a new TLS handshake, before every model call.
    // One box serves every project of the user: the relay meters usage per project by this header.
    let mut req = a
        .http_client()
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
    let t0 = std::time::Instant::now();
    match req.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let mut out_headers = HeaderMap::new();
            for (k, v) in resp.headers() {
                // `x-arbos-req-id` is the relay's id for this model call: passing it on is what
                // lets a turn on the box be read against the relay's record of the same call.
                if k == "content-type" || k == "cache-control" || k == "x-arbos-req-id" {
                    out_headers.insert(k.clone(), v.clone());
                }
            }
            tracing::info!(
                req_id = out_headers
                    .get("x-arbos-req-id")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or(""),
                %path,
                status = status.as_u16(),
                headers_ms = t0.elapsed().as_millis() as u64,
                "model call forwarded"
            );
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// One request to the box surface, made with the standard library on the caller's own
    /// thread. The point of these tests is what happens when the main runtime is unavailable,
    /// and a client that needs that runtime could not ask the question.
    fn http_get(port: u16, path: &str, bearer: Option<&str>) -> String {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.set_read_timeout(Some(NO_ANSWER_AFTER))
            .expect("read timeout");
        let auth = bearer
            .map(|t| format!("authorization: Bearer {t}\r\n"))
            .unwrap_or_default();
        write!(
            s,
            "GET {path} HTTP/1.1\r\nhost: box\r\n{auth}connection: close\r\n\r\n"
        )
        .expect("write request");
        let mut out = String::new();
        // A surface that never answers is the defect under test, so an unanswered call comes
        // back as a reading rather than as a panic in the probe thread: the test then fails on
        // the assertion that names what went wrong.
        match s.read_to_string(&mut out) {
            Ok(_) => out,
            Err(e) => format!("(no answer in {NO_ANSWER_AFTER:?}: {e})"),
        }
    }

    /// Past this the call counts as unanswered. Well over any real answer, and the window the
    /// gVisor boxes showed was minutes, so nothing borderline rests on the number.
    const NO_ANSWER_AFTER: Duration = Duration::from_secs(15);

    /// How long the main runtime is held. Comfortably longer than any answer should take, and
    /// short enough that the suite does not notice.
    const BOOT_HOLDS_RUNTIME_FOR: Duration = Duration::from_secs(3);

    /// Ask the box surface one question while this runtime is doing what a boot does to it.
    ///
    /// `#[tokio::test]` is a single worker thread, which is the shape that matters: a gVisor box
    /// has few cores, and a workspace start is synchronous between await points — loading a
    /// chat document is a SQLite read and a Loro decode, with nothing to yield at. Served on
    /// this runtime, the request below is not answered slowly; it is not polled at all until
    /// the hold ends. Returns how long the answer took, and the answer.
    fn ask_while_the_boot_runs(port: u16, path: &str, bearer: Option<&str>) -> (Duration, String) {
        let go = Arc::new(AtomicBool::new(false));
        let start = go.clone();
        let path = path.to_owned();
        let bearer = bearer.map(str::to_owned);
        let probe = std::thread::spawn(move || {
            while !start.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            let t = Instant::now();
            let body = http_get(port, &path, bearer.as_deref());
            (t.elapsed(), body)
        });
        go.store(true, Ordering::SeqCst);
        let held = Instant::now();
        let mut spin = 0u64;
        while held.elapsed() < BOOT_HOLDS_RUNTIME_FOR {
            spin = spin.wrapping_add(std::hint::black_box(1));
        }
        std::hint::black_box(spin);
        probe.join().expect("probe thread")
    }

    fn box_surface(tag: &str) -> (Arc<Supervisor>, u16, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("arbos-{tag}-{}", arbos_core::ids::ulid()));
        let sup = crate::workspace::tests::test_supervisor(&root);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        serve_box(listener, sup.clone()).expect("serve");
        (sup, port, root)
    }

    /// 2026-11-09: `/healthz` answers while the box's own boot owns the main runtime.
    ///
    /// This is the regression itself. On gVisor a box with many workspaces spent 9–13 minutes
    /// after boot with no answer from this endpoint at all, because the workspace starts and
    /// the handler shared one scheduler and the starts never gave it up. The answer it gives
    /// during that window has to be honest rather than empty, so the state is asserted too:
    /// `initializing`, not a silent `ok` over a workspace list that is not the real one yet.
    #[tokio::test]
    async fn healthz_answers_while_the_boot_holds_the_runtime() {
        let (_sup, port, root) = box_surface("healthz-boot");
        let (took, body) = ask_while_the_boot_runs(port, "/healthz", None);
        assert!(
            took < Duration::from_secs(1),
            "/healthz took {took:?} while the boot held the runtime (the boot held it for \
             {BOOT_HOLDS_RUNTIME_FOR:?}); the surface is sharing a scheduler with the boot again"
        );
        assert!(body.starts_with("HTTP/1.1 200"), "answer was: {body}");
        assert!(
            body.contains("\"state\":\"initializing\""),
            "a box that has not applied a listing says so: {body}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The same for `/box/state`, which is the call the deploy worker's readback and
    /// `/machine/debug` make — the "status" of a box, as opposed to its health.
    #[tokio::test]
    async fn box_state_answers_while_the_boot_holds_the_runtime() {
        let (_sup, port, root) = box_surface("boxstate-boot");
        let (took, body) = ask_while_the_boot_runs(port, "/box/state", Some("test-mcp"));
        assert!(
            took < Duration::from_secs(1),
            "/box/state took {took:?} while the boot held the runtime"
        );
        assert!(body.starts_with("HTTP/1.1 200"), "answer was: {body}");
        assert!(
            body.contains("\"state\":\"initializing\"") && body.contains("\"ready\":false"),
            "the status call says where the boot is: {body}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// And once the listing has been applied the same surfaces say `ready` — the state is a
    /// report, not a constant that happens to read well during a boot.
    #[tokio::test]
    async fn the_surfaces_say_ready_once_the_listing_is_applied() {
        let (sup, port, root) = box_surface("ready");
        sup.apply_listing(&json!({ "workspaces": [] })).await;
        let body = tokio::task::spawn_blocking(move || http_get(port, "/healthz", None))
            .await
            .expect("probe");
        assert!(
            body.contains("\"state\":\"ready\"") && body.contains("\"ready\":true"),
            "answer was: {body}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every model call the box makes goes through this stub, and a fresh client per call meant
    /// a fresh TCP handshake and TLS session to the relay before the prompt could even be sent.
    /// That cost sits inside the first-token figure and nothing on either side could see it.
    #[tokio::test]
    async fn the_model_stub_reuses_one_connection_to_the_relay() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(AtomicUsize::new(0));
        let counted = connections.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                counted.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = [0u8; 8192];
                    while let Ok(n) = sock.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        let reply = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                             x-arbos-req-id: 0a1b2c3d4e5f\r\nset-cookie: nope\r\n\
                             content-length: 2\r\n\r\n{}";
                        if sock.write_all(reply.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });

        let (agent, _root) =
            crate::status::tests::test_agent_on_relay(&format!("http://127.0.0.1:{port}")).await;
        for _ in 0..3 {
            let r = stub_forward(
                State(agent.clone()),
                Method::POST,
                "/v1/messages".parse::<Uri>().unwrap(),
                HeaderMap::new(),
                Bytes::from_static(b"{}"),
            )
            .await;
            assert_eq!(r.status(), StatusCode::OK);
            assert_eq!(
                r.headers()
                    .get("x-arbos-req-id")
                    .and_then(|v| v.to_str().ok()),
                Some("0a1b2c3d4e5f"),
                "the relay's id for this call reaches the caller"
            );
            assert!(
                r.headers().get("set-cookie").is_none(),
                "and nothing else new does"
            );
            // The connection goes back to the pool only once its body is drained.
            axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        }
        assert_eq!(
            connections.load(Ordering::SeqCst),
            1,
            "three model calls, one connection"
        );
    }
}
