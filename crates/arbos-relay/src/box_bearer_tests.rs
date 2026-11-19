//! Every call the relay makes to a box's port for chat content carries the box bearer.
//!
//! agentd asks for it on the history page and the browser stream (unarbos/arbos#3108). The relay
//! holds it as `boxes.mcp_token`; a call that forgot it would read as a broken box.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;

use arbos_core::ids::now_ms;

use crate::db::Sandbox;
use crate::people::tests_support::{person, project_of};
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};

const BOX_BEARER: &str = "mcp-of-this-box";

/// What the fake box was asked, as `(path, authorization header)`.
type Asked = Arc<Mutex<Vec<(String, Option<String>)>>>;

async fn note(
    State(asked): State<Asked>,
    uri: axum::http::Uri,
    headers: HeaderMap,
) -> &'static str {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    asked.lock().unwrap().push((uri.path().to_owned(), auth));
    "[]"
}

/// A box that only writes down what it is asked, and the row that points the relay at it.
async fn a_box_for(state: &AppState, user: &str) -> Asked {
    let asked = Asked::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().fallback(note).with_state(asked.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    state
        .db
        .upsert_box(&Sandbox {
            user_id: user.into(),
            provider: "gvisor".into(),
            sandbox_id: Some("arbos-box-test".into()),
            daytona_state: None,
            device_id: "box:arbos-box-test".into(),
            box_token_hash: "hash".into(),
            mcp_token: BOX_BEARER.into(),
            snapshot: None,
            last_started_at: Some(1_000),
            last_stopped_at: None,
            last_seen_at: Some(now_ms()),
            box_url: Some(format!("http://{addr}")),
            preview_token: None,
            local_pid: None,
            migrated_from: Vec::new(),
        })
        .expect("box row");
    asked
}

async fn asked_for(asked: &Asked, path: &str) -> Option<String> {
    for _ in 0..200 {
        if let Some((_, auth)) = asked.lock().unwrap().iter().find(|(p, _)| p == path) {
            return auth.clone();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "the box was never asked for {path}: {:?}",
        asked.lock().unwrap()
    );
}

#[tokio::test]
async fn the_history_page_is_asked_for_with_the_box_bearer() {
    let state = test_state(AuthMode::Github);
    let (user, principal) = person(&state, 9911);
    let project = project_of(&state, &user, "History agent");
    let asked = a_box_for(&state, &user).await;

    let answer = crate::api::chat_history(
        State(state),
        principal,
        Path((project.clone(), "c1".into())),
        Query(HashMap::new()),
    )
    .await;
    assert!(answer.is_ok(), "the relay hands the box's page on");
    assert_eq!(
        asked_for(&asked, &format!("/w/{project}/history/c1")).await,
        Some(format!("Bearer {BOX_BEARER}"))
    );
}

#[tokio::test]
async fn the_browser_stream_is_dialled_with_the_box_bearer() {
    let state = test_state(AuthMode::Github);
    let (user, _) = person(&state, 9912);
    let project = project_of(&state, &user, "Browser agent");
    let asked = a_box_for(&state, &user).await;
    let token = format!("adv_{}", arbos_core::ids::ulid());
    state
        .db
        .insert_device(
            &arbos_core::ids::ulid(),
            "harness",
            &arbos_core::auth::hash(&token),
            &user,
            "github",
        )
        .expect("device");

    // The handler takes a websocket upgrade, so it is served and dialled for real.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .route(
            "/projects/{id}/browser/stream",
            axum::routing::get(crate::browser::browser_stream),
        )
        .with_state(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let (client, _) = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/projects/{project}/browser/stream?token={token}"
    ))
    .await
    .expect("the relay takes the upgrade");

    // The fake box does not upgrade, so the proxy closes; what matters is what it was sent.
    assert_eq!(
        asked_for(&asked, &format!("/w/{project}/browser/stream")).await,
        Some(format!("Bearer {BOX_BEARER}"))
    );
    drop(client);
}
