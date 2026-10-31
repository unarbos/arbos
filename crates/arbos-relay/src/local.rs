//! User-computer access (F9). `arbos-local` helpers hold one WebSocket each
//! (`WS /local/{deviceId}`, device token); a project box asks the relay to forward one op
//! (`POST /projects/{id}/local/{device}/exec`, box token) and the relay waits for the result
//! on that socket. The relay never runs anything itself and never sees a device's files beyond
//! what passes through the reply.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc, oneshot, Mutex};

use arbos_core::api::{LocalDeviceDto, LocalDevicesDto};
use arbos_core::ids::ulid;
use arbos_core::local::{
    LocalClientFrame, LocalExecRequest, LocalOp, LocalResult, LocalServerFrame,
};

use crate::auth::Principal;
use crate::state::AppState;

const PING_EVERY: Duration = Duration::from_secs(20);
/// Relay-side slack on top of the op's own timeout (network + helper scheduling).
const EXEC_GRACE: Duration = Duration::from_secs(5);
const RESULT_MAX_BYTES: usize = 4 * 1024 * 1024;

/// One live helper socket.
pub struct LocalConn {
    pub tx: mpsc::Sender<String>,
    pending: Mutex<HashMap<String, oneshot::Sender<LocalResult>>>,
}

pub type LocalConns = Arc<Mutex<HashMap<String, Arc<LocalConn>>>>;

type ApiResult = Result<Response, (StatusCode, String)>;

fn internal(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// Whether this principal may act on `device_id`.
///
/// `local_devices` has no owner column: a helper's row is keyed by the relay `devices.id` its
/// token was minted for, so the owner is that device row's user. Nothing in this module checked
/// it, and a device id is not a secret — `GET /local/devices` listed every helper on the relay to
/// any caller, and the `reachable` block of a call start hands out each callee's device ids — so
/// one signed-in user could sign another user's device out, flip their helper's "read without
/// asking" switch, and have a box run ops against their computer.
///
/// A device that is not the caller's answers like a device that does not exist, so these handlers
/// cannot be used to test whether an id is real.
fn owns_device(
    state: &AppState,
    principal: &Principal,
    device_id: &str,
) -> Result<bool, (StatusCode, String)> {
    let Some(caller) = principal.user_id() else {
        // The admin token acts for no user and may reach any device, as it does on
        // `DELETE /auth/devices/{id}`.
        return Ok(matches!(principal, Principal::Admin));
    };
    let owner = state.db.device_user(device_id).map_err(internal)?;
    Ok(owner.as_deref() == Some(caller))
}

fn no_such_device() -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, "no such local device".to_owned())
}

/// `WS /local/{deviceId}` — the helper's own device token only.
pub async fn ws_local(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    principal: Principal,
    Path(device_id): Path<String>,
) -> Response {
    match &principal {
        Principal::Device { id, .. } if *id == device_id => {}
        Principal::Device { .. } => {
            return (StatusCode::FORBIDDEN, "token belongs to another device").into_response()
        }
        Principal::Box { .. } | Principal::Admin => {
            return (StatusCode::FORBIDDEN, "device token required").into_response()
        }
    }
    ws.max_message_size(RESULT_MAX_BYTES)
        .on_upgrade(move |socket| run(socket, state, device_id))
}

async fn run(socket: WebSocket, state: AppState, device_id: String) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<String>(64);
    let conn = Arc::new(LocalConn {
        tx,
        pending: Mutex::new(HashMap::new()),
    });
    // A reconnect replaces the previous socket for this device; its pending ops fail below.
    let previous = state
        .local_conns
        .lock()
        .await
        .insert(device_id.clone(), conn.clone());
    if let Some(prev) = previous {
        fail_pending(&prev, "helper reconnected").await;
    }
    let mut said_hello = false;
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    loop {
        tokio::select! {
            incoming = stream.next() => {
                let Some(Ok(msg)) = incoming else { break };
                let text = match msg {
                    Message::Text(t) => t.to_string(),
                    Message::Close(_) => break,
                    Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => continue,
                };
                let frame: LocalClientFrame = match serde_json::from_str(&text) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!(%device_id, error = %e, "local: bad frame");
                        continue;
                    }
                };
                match frame {
                    LocalClientFrame::Hello(h) => {
                        said_hello = true;
                        let _ = state.db.local_device_hello(&device_id, &h.hostname, &h.os, &h.home);
                        tracing::info!(%device_id, host = %h.hostname, os = %h.os, "local helper connected");
                    }
                    LocalClientFrame::Result { id, result } => {
                        let _ = state.db.local_device_seen(&device_id, true);
                        if let Some(waiter) = conn.pending.lock().await.remove(&id) {
                            let _ = waiter.send(result);
                        }
                    }
                    LocalClientFrame::Pong => {
                        let _ = state.db.local_device_seen(&device_id, true);
                    }
                }
            }
            out = rx.recv() => {
                let Some(text) = out else { break };
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            _ = ping.tick() => {
                let Ok(text) = serde_json::to_string(&LocalServerFrame::Ping) else { continue };
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        }
    }
    let mut conns = state.local_conns.lock().await;
    if conns.get(&device_id).is_some_and(|c| Arc::ptr_eq(c, &conn)) {
        conns.remove(&device_id);
        if said_hello {
            let _ = state.db.local_device_seen(&device_id, false);
        }
    }
    drop(conns);
    fail_pending(&conn, "helper disconnected").await;
    tracing::info!(%device_id, "local helper disconnected");
}

async fn fail_pending(conn: &LocalConn, why: &str) {
    for (_, waiter) in conn.pending.lock().await.drain() {
        let _ = waiter.send(LocalResult::Error {
            message: why.to_owned(),
        });
    }
}

/// Unregister a helper socket and fail whatever it was asked to do. Revoking the device row
/// does not reach the socket: the helper registered once and stays in `local_conns` until it
/// disconnects, so without this a signed-out machine still answers ops.
pub(crate) async fn disconnect(state: &AppState, device_id: &str, why: &str) {
    if let Some(conn) = state.local_conns.lock().await.remove(device_id) {
        fail_pending(&conn, why).await;
    }
}

/// `POST /projects/{id}/local/{device}/exec` (box token): body = one `LocalOp`.
pub async fn exec(
    State(state): State<AppState>,
    principal: Principal,
    Path((project_id, device_id)): Path<(String, String)>,
    Json(op): Json<LocalOp>,
) -> ApiResult {
    if !state.allows(&principal, &project_id) {
        return Err((StatusCode::FORBIDDEN, "not your project".into()));
    }
    if !matches!(principal, Principal::Box { .. } | Principal::Admin) {
        return Err((StatusCode::FORBIDDEN, "box token required".into()));
    }
    if state.db.project(&project_id).map_err(internal)?.is_none() {
        return Err((StatusCode::NOT_FOUND, "no such project".into()));
    }
    // The project is the caller's (`allows` above); the computer has to be the caller's too.
    // Without this a box could run ops against any helper whose device id it had seen.
    if !owns_device(&state, &principal, &device_id)? {
        return Err(no_such_device());
    }
    let known = state.db.local_device(&device_id).map_err(internal)?;
    if known.is_none() {
        return Err(no_such_device());
    }
    let conn = state.local_conns.lock().await.get(&device_id).cloned();
    let Some(conn) = conn else {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "kind": "error", "message": "device is offline" })),
        )
            .into_response());
    };
    let id = ulid();
    let (tx, rx) = oneshot::channel();
    conn.pending.lock().await.insert(id.clone(), tx);
    let wait = Duration::from_secs(op.timeout_s()) + EXEC_GRACE;
    let frame = LocalServerFrame::Exec(LocalExecRequest {
        id: id.clone(),
        project_id: project_id.clone(),
        op,
    });
    let text = serde_json::to_string(&frame).map_err(internal)?;
    if conn.tx.send(text).await.is_err() {
        conn.pending.lock().await.remove(&id);
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "kind": "error", "message": "device socket closed" })),
        )
            .into_response());
    }
    let result = match tokio::time::timeout(wait, rx).await {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => LocalResult::Error {
            message: "helper disconnected".into(),
        },
        Err(_) => {
            conn.pending.lock().await.remove(&id);
            return Ok((
                StatusCode::GATEWAY_TIMEOUT,
                Json(json!({ "kind": "error", "message": format!("no result within {} s", wait.as_secs()) })),
            )
                .into_response());
        }
    };
    Ok(Json(serde_json::to_value(result).map_err(internal)?).into_response())
}

/// `GET /local/devices` — device tokens (Settings → This Mac) and box tokens (`local_devices`
/// tool). The caller's own helpers only: this list used to be every helper on the relay, which
/// is where another user's device ids came from.
pub async fn list(State(state): State<AppState>, principal: Principal) -> ApiResult {
    let devices = match principal.user_id() {
        Some(user) => state.db.local_devices_of_user(user),
        None => state.db.local_devices(),
    }
    .map_err(internal)?;
    let online = state.local_conns.lock().await;
    let items: Vec<LocalDeviceDto> = devices
        .into_iter()
        .map(|d| {
            let connected = online.contains_key(&d.device_id);
            LocalDeviceDto {
                device_id: d.device_id,
                name: d.name,
                hostname: d.hostname,
                os: d.os,
                home: d.home,
                connected,
                last_seen: d.last_seen,
                first_seen: d.first_seen,
                allow_reads: d.allow_reads,
            }
        })
        .collect();
    Ok(Json(LocalDevicesDto { items }).into_response())
}

#[derive(Deserialize)]
pub struct PatchBody {
    #[serde(default)]
    pub allow_reads: Option<bool>,
}

/// `PATCH /local/devices/{id}` (device token): the per-device "allow reads without asking" toggle.
pub async fn patch(
    State(state): State<AppState>,
    principal: Principal,
    Path(device_id): Path<String>,
    Json(b): Json<PatchBody>,
) -> ApiResult {
    if principal.is_box() {
        return Err((
            StatusCode::FORBIDDEN,
            "box tokens cannot change device settings".into(),
        ));
    }
    if !owns_device(&state, &principal, &device_id)? {
        return Err(no_such_device());
    }
    if let Some(allow) = b.allow_reads {
        let ok = state
            .db
            .local_device_set_allow_reads(&device_id, allow)
            .map_err(internal)?;
        if !ok {
            return Err(no_such_device());
        }
    }
    let d = state
        .db
        .local_device(&device_id)
        .map_err(internal)?
        .ok_or_else(no_such_device)?;
    Ok(Json(serde_json::to_value(d).map_err(internal)?).into_response())
}

/// `DELETE /local/devices/{id}` (device token): forget the helper and revoke its token.
///
/// The revoke is the full one: the `devices` row, its refresh tokens and its Matrix session.
/// The older weak revoke only flagged the device row, so a "removed" helper could still mint
/// access tokens from a refresh token it already held.
pub async fn delete(
    State(state): State<AppState>,
    principal: Principal,
    Path(device_id): Path<String>,
) -> ApiResult {
    if principal.is_box() {
        return Err((
            StatusCode::FORBIDDEN,
            "box tokens cannot remove devices".into(),
        ));
    }
    if !owns_device(&state, &principal, &device_id)? {
        return Err(no_such_device());
    }
    let ok = state.db.local_device_delete(&device_id).map_err(internal)?;
    // `revoke_device_everywhere` disconnects the helper too; this says why in the error the
    // pending ops get.
    disconnect(&state, &device_id, "device removed").await;
    crate::signin::revoke_device_everywhere(&state, &device_id)
        .await
        .map_err(internal)?;
    Ok(if ok {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
    .into_response())
}

#[cfg(test)]
mod owner_tests {
    //! A helper belongs to one user. Every handler here used to take a device id on trust, and a
    //! device id is not a secret: the box roster of a shared chat names each member's devices
    //! (`people::box_roster_carries_member_devices_and_the_app_roster_does_not`), the `reachable`
    //! block of a call start names each callee's, and `GET /local/devices` used to list every
    //! helper on the relay to anyone who asked.

    use axum::body::to_bytes;
    use serde_json::Value;

    use crate::db::Project;
    use crate::people::tests_support::person;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    use super::*;

    fn helper(state: &AppState, owner_device: &str) {
        state
            .db
            .local_device_hello(owner_device, "mac.local", "macos", "/Users/t")
            .expect("hello");
    }

    fn live(state: &AppState, device_id: &str) -> bool {
        matches!(state.db.device_status(device_id), Ok(Some((_, false))))
    }

    fn allow_reads(state: &AppState, device_id: &str) -> bool {
        state
            .db
            .local_device(device_id)
            .unwrap()
            .expect("row")
            .allow_reads
    }

    /// These handlers report "no such device" as an error and success as a response, and the
    /// older ones reported a missing row as an `Ok(404)`. The tests care about the status either
    /// way, so that a refusal that never happened fails on what it let through, not on shape.
    fn status_of(r: ApiResult) -> StatusCode {
        match r {
            Ok(res) => res.status(),
            Err((code, _)) => code,
        }
    }

    fn box_of(user: &str) -> Principal {
        Principal::Box {
            user_id: user.to_owned(),
        }
    }

    fn project_of(state: &AppState, id: &str, user: &str) {
        state
            .db
            .insert_project(&Project {
                id: id.into(),
                name: id.into(),
                icon: "P".into(),
                instructions: String::new(),
                models: json!({}),
                state: "awake".into(),
                created_at: 0,
                last_active_at: 0,
                spend_cap_daily: 25.0,
                color: "#000".into(),
                user_id: user.into(),
                workspace_state: "stopped".into(),
                avatar: None,
                avatar_color: None,
                description: String::new(),
                primary_chat_id: None,
                avatar_color_index: None,
                avatar_leaf: None,
                avatar_leaf_color: None,
                image_key: None,
                avatar_version: 0,
            })
            .expect("project");
    }

    /// The one that matters: Alice signing Bob's device out. Before the owner check this
    /// answered 404 — because Bob has no `local_devices` row — and revoked him anyway.
    #[tokio::test]
    async fn one_users_token_cannot_sign_another_users_device_out() {
        let state = test_state(AuthMode::Github);
        let (_alice, alice_dev) = person(&state, 21);
        let (_bob, bob_dev) = person(&state, 22);
        let Principal::Device { id: bob_id, .. } = &bob_dev else {
            panic!("device")
        };

        let bob_id = bob_id.clone();
        let r = delete(State(state.clone()), alice_dev, Path(bob_id.clone())).await;
        assert!(
            live(&state, &bob_id),
            "Alice's token signed Bob's device out"
        );
        assert_eq!(
            status_of(r),
            StatusCode::NOT_FOUND,
            "another user's device answers like no device at all"
        );
    }

    /// `allow_reads` is the switch that lets an agent read files without asking each time.
    #[tokio::test]
    async fn one_users_token_cannot_flip_another_users_read_switch() {
        let state = test_state(AuthMode::Github);
        let (_alice, alice_dev) = person(&state, 23);
        let (_bob, bob_dev) = person(&state, 24);
        let Principal::Device { id: bob_id, .. } = &bob_dev else {
            panic!("device")
        };
        let bob_id = bob_id.clone();
        helper(&state, &bob_id);

        let r = patch(
            State(state.clone()),
            alice_dev,
            Path(bob_id.clone()),
            Json(PatchBody {
                allow_reads: Some(true),
            }),
        )
        .await;
        assert!(
            !allow_reads(&state, &bob_id),
            "Alice's token turned on reads-without-asking on Bob's computer"
        );
        assert_eq!(status_of(r), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_device_list_is_only_the_callers_own() {
        let state = test_state(AuthMode::Github);
        let (_alice, alice_dev) = person(&state, 25);
        let (_bob, bob_dev) = person(&state, 26);
        let (Principal::Device { id: alice_id, .. }, Principal::Device { id: bob_id, .. }) =
            (&alice_dev, &bob_dev)
        else {
            panic!("devices")
        };
        let (alice_id, bob_id) = (alice_id.clone(), bob_id.clone());
        helper(&state, &alice_id);
        helper(&state, &bob_id);

        let r = list(State(state.clone()), alice_dev).await.expect("list");
        let body = to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        let ids: Vec<&str> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|d| d["device_id"].as_str())
            .collect();
        assert_eq!(ids, vec![alice_id.as_str()], "no other user's helper");
    }

    /// The other two arms of the same `match`, which the test above does not reach: a box asks
    /// on behalf of the user it serves (that is what the `local_devices` tool is for, so an
    /// empty answer there would break it silently), and the admin token has no user and keeps
    /// the whole list. Both would survive a change that only kept the device arm honest.
    #[tokio::test]
    async fn a_box_sees_its_own_users_helpers_and_the_admin_token_sees_every_one() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 29);
        let (_bob, bob_dev) = person(&state, 30);
        let (Principal::Device { id: alice_id, .. }, Principal::Device { id: bob_id, .. }) =
            (&alice_dev, &bob_dev)
        else {
            panic!("devices")
        };
        let (alice_id, bob_id) = (alice_id.clone(), bob_id.clone());
        helper(&state, &alice_id);
        helper(&state, &bob_id);

        let ids = |r: ApiResult| -> Vec<String> {
            let body = r.expect("list").into_body();
            let bytes = futures::executor::block_on(to_bytes(body, 1 << 20)).expect("body");
            let v: Value = serde_json::from_slice(&bytes).unwrap();
            v["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|d| d["device_id"].as_str().map(str::to_owned))
                .collect()
        };

        assert_eq!(
            ids(list(State(state.clone()), box_of(&alice)).await),
            vec![alice_id.clone()],
            "Alice's box must still see Alice's helper"
        );
        let mut all = ids(list(State(state.clone()), Principal::Admin).await);
        all.sort();
        let mut both = vec![alice_id, bob_id];
        both.sort();
        assert_eq!(all, both, "the admin token keeps the whole list");
    }

    /// A box runs ops on a computer. It may only reach its own user's.
    #[tokio::test]
    async fn a_box_cannot_run_ops_on_another_users_computer() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 27);
        let (_bob, bob_dev) = person(&state, 28);
        let Principal::Device { id: bob_id, .. } = &bob_dev else {
            panic!("device")
        };
        helper(&state, bob_id);
        project_of(&state, "p_alice", &alice);

        let r = exec(
            State(state.clone()),
            box_of(&alice),
            Path(("p_alice".to_owned(), bob_id.clone())),
            Json(LocalOp::ListDir { path: "/".into() }),
        )
        .await;
        assert_eq!(
            status_of(r),
            StatusCode::NOT_FOUND,
            "Alice's box reached a helper belonging to Bob"
        );
    }

    /// The owner still works, and the removal now takes the device's refresh tokens with it.
    #[tokio::test]
    async fn the_owner_removes_their_own_helper_and_its_tokens_die() {
        let state = test_state(AuthMode::Github);
        let (user, dev) = person(&state, 29);
        let Principal::Device { id, .. } = &dev else {
            panic!("device")
        };
        let id = id.clone();
        helper(&state, &id);
        let tokens = crate::signin::issue_tokens(&state, &user, &id).expect("tokens");
        let h = arbos_core::auth::hash(&tokens.refresh_token);

        let r = delete(State(state.clone()), dev, Path(id.clone())).await;
        assert_eq!(status_of(r), StatusCode::NO_CONTENT);
        assert!(state.db.local_device(&id).unwrap().is_none());
        assert!(!live(&state, &id), "the device is signed out");
        assert!(
            state
                .db
                .refresh_row(&h)
                .unwrap()
                .expect("row")
                .revoked_at
                .is_some(),
            "a removed helper cannot mint a new access token from a refresh token it kept"
        );
    }
}
