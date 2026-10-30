//! `GET /projects/{id}/browser/stream` (WebSocket): the relay dials the box's `/browser/stream`
//! (agentd's proxy of agent-browser's JPEG stream, S16) with the preview token a laptop never
//! holds, and pipes frames to the device. Watch-only: client messages are forwarded too, so a
//! later version can enable input.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as UpMsg;

use crate::auth::Principal;
use crate::state::AppState;

pub async fn browser_stream(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let project = match state.db.project(&id) {
        Ok(Some(p)) => p,
        _ => return (StatusCode::NOT_FOUND, "no such project").into_response(),
    };
    if !state.allows(&principal, &project.id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let sb = match state.db.user_box(&project.user_id) {
        Ok(Some(s)) => s,
        _ => return (StatusCode::NOT_FOUND, "no sandbox").into_response(),
    };
    let Some(box_url) = sb.box_url.clone() else {
        return (StatusCode::CONFLICT, "box has no url yet").into_response();
    };
    let upstream = format!(
        "{}/w/{}/browser/stream",
        box_url
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1),
        project.id
    );
    let token = crate::boxes::preview_token_for(&state, &sb).await;
    ws.max_message_size(32 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            if let Err(e) = pipe(socket, &upstream, token.as_deref()).await {
                tracing::debug!(error = %e, project = %project.id, "browser stream proxy closed");
            }
        })
}

async fn pipe(
    client: WebSocket,
    upstream: &str,
    preview_token: Option<&str>,
) -> anyhow::Result<()> {
    let mut req = upstream.into_client_request()?;
    if let Some(t) = preview_token {
        req.headers_mut()
            .insert("x-daytona-preview-token", t.parse()?);
    }
    let (up, _) = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio_tungstenite::connect_async(req),
    )
    .await??;
    let (mut up_tx, mut up_rx) = up.split();
    let (mut cl_tx, mut cl_rx) = client.split();
    let to_client = async {
        while let Some(Ok(m)) = up_rx.next().await {
            let out = match m {
                UpMsg::Binary(b) => Message::Binary(b),
                UpMsg::Text(t) => Message::Text(t.as_str().into()),
                UpMsg::Close(_) => break,
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
                Message::Binary(b) => UpMsg::Binary(b),
                Message::Text(t) => UpMsg::Text(t.as_str().into()),
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
