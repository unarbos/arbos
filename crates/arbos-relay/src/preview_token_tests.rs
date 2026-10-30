//! 2026-10-29 16:05–16:45: four users' boxes ran but every relay → box call answered 401. The
//! relay's own restart of a "silent" box made Daytona issue a new preview token; the wait for the
//! restart timed out ("still Stopping after 120s"), so the row kept the old token, and Daytona's
//! proxy refused it (the Resources pane: "The machine did not answer · box answered 401").
//!
//! Replayed here against a stand-in that is both Daytona and the box: the box takes only the
//! current preview token, the row holds the one from before the restart. Test data only.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use serde_json::{json, Value};

use arbos_core::ids::now_ms;

use crate::auth::Principal;
use crate::daytona::Daytona;
use crate::db::Sandbox;
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};

const USER: &str = "gh_preview_token_test";
const SANDBOX: &str = "sb-preview-test";
const MCP: &str = "mcp-test";
const OLD_TOKEN: &str = "pv-before-restart";
const NEW_TOKEN: &str = "pv-after-restart";

#[derive(Default)]
struct Fake {
    preview_url_calls: AtomicUsize,
    box_calls: AtomicUsize,
}

async fn fake_daytona_and_box() -> (Daytona, String, Arc<Fake>) {
    let fake = Arc::new(Fake::default());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let base = format!("http://{}", l.local_addr().expect("addr"));
    let (f, b) = (fake.clone(), base.clone());
    let app = axum::Router::new().fallback(move |method: Method, uri: Uri, headers: HeaderMap| {
        let (f, b) = (f.clone(), b.clone());
        async move {
            let header = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or("");
            match (method, uri.path()) {
                (Method::GET, "/box/box/state") => {
                    f.box_calls.fetch_add(1, Ordering::SeqCst);
                    if header("x-daytona-preview-token") != NEW_TOKEN
                        || header("authorization") != format!("Bearer {MCP}")
                    {
                        return (StatusCode::UNAUTHORIZED, "Unauthorized".to_owned());
                    }
                    (StatusCode::OK, json!({ "uptime_s": 1 }).to_string())
                }
                (Method::GET, p) if p == format!("/sandbox/{SANDBOX}/ports/7788/preview-url") => {
                    f.preview_url_calls.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        json!({ "url": format!("{b}/box"), "token": NEW_TOKEN }).to_string(),
                    )
                }
                (Method::GET, p) if p == format!("/sandbox/{SANDBOX}") => (
                    StatusCode::OK,
                    json!({ "id": SANDBOX, "state": "started" }).to_string(),
                ),
                _ => (StatusCode::NOT_FOUND, "{}".to_owned()),
            }
        }
    });
    tokio::spawn(async move { axum::serve(l, app).await.ok() });
    (
        Daytona::new("k".into(), Some(base.clone()), None),
        base,
        fake,
    )
}

async fn setup(mcp_token: &str) -> (AppState, Arc<Fake>) {
    let mut state = test_state(AuthMode::Github);
    let (dt, base, fake) = fake_daytona_and_box().await;
    state.daytona = Some(dt);
    state
        .db
        .insert_test_user(USER, "preview-token-test", None, None, 10.0)
        .expect("user");
    state
        .db
        .upsert_box(&Sandbox {
            user_id: USER.into(),
            provider: "daytona".into(),
            sandbox_id: Some(SANDBOX.into()),
            daytona_state: Some("started".into()),
            device_id: format!("box:{SANDBOX}"),
            box_token_hash: "hash-test".into(),
            mcp_token: mcp_token.into(),
            snapshot: Some("arbos-agent-test".into()),
            last_started_at: Some(1_000),
            last_stopped_at: None,
            last_seen_at: Some(now_ms()),
            box_url: Some(format!("{base}/box")),
            preview_token: Some(OLD_TOKEN.into()),
            local_pid: None,
            migrated_from: Vec::new(),
        })
        .expect("row");
    (state, fake)
}

async fn machine_debug(state: &AppState) -> Value {
    let q = HashMap::from([("user".to_owned(), USER.to_owned())]);
    let r = crate::debug_api::machine_debug(State(state.clone()), Principal::Admin, Query(q)).await;
    let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn a_stale_preview_token_is_replaced_and_the_box_answers() {
    let (state, fake) = setup(MCP).await;
    let v = machine_debug(&state).await;
    assert_eq!(v["box"]["reachable"], json!(true), "{}", v["box"]);
    assert_eq!(fake.preview_url_calls.load(Ordering::SeqCst), 1);

    let v = machine_debug(&state).await;
    assert_eq!(v["box"]["reachable"], json!(true), "{}", v["box"]);
    assert_eq!(
        fake.preview_url_calls.load(Ordering::SeqCst),
        1,
        "the fresh token is remembered"
    );

    let row = state.db.user_box(USER).unwrap().expect("row");
    assert_eq!(
        row.preview_token.as_deref(),
        Some(OLD_TOKEN),
        "the row is not rewritten"
    );
}

/// A 401 from agentd itself (wrong MCP token) is not retried in a loop and is reported as such.
#[tokio::test]
async fn a_401_with_the_current_preview_token_is_reported_once() {
    let (state, fake) = setup("mcp-wrong").await;
    let v = machine_debug(&state).await;
    assert_eq!(v["box"]["reachable"], json!(false));
    assert_eq!(v["box"]["error"], json!("box answered 401 Unauthorized"));
    assert_eq!(fake.box_calls.load(Ordering::SeqCst), 2);

    let v = machine_debug(&state).await;
    assert_eq!(v["box"]["error"], json!("box answered 401 Unauthorized"));
    assert_eq!(fake.box_calls.load(Ordering::SeqCst), 3);
    assert_eq!(fake.preview_url_calls.load(Ordering::SeqCst), 2);
}
