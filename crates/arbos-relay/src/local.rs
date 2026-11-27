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
use tokio::sync::{mpsc, oneshot, Mutex, Notify};

use arbos_core::api::{LocalDeviceDto, LocalDevicesDto};
use arbos_core::ids::ulid;
use arbos_core::local::{
    LocalClientFrame, LocalDecide, LocalDecision, LocalExecRequest, LocalOp, LocalRequestCreate,
    LocalRequestCreated, LocalResult, LocalServerFrame, LOCAL_RUN_TIMEOUT_MAX_S,
};

use crate::auth::Principal;
use crate::db::{ApprovalRequest, LocalDevice};
use crate::state::AppState;

const PING_EVERY: Duration = Duration::from_secs(20);
/// Relay-side slack on top of the op's own timeout (network + helper scheduling).
const EXEC_GRACE: Duration = Duration::from_secs(5);
const RESULT_MAX_BYTES: usize = 4 * 1024 * 1024;

/// One live helper socket.
pub struct LocalConn {
    pub tx: mpsc::Sender<String>,
    pending: Mutex<HashMap<String, oneshot::Sender<LocalResult>>>,
    /// Fired by [`disconnect`] to stop [`run`]. Taking the connection out of `local_conns` does
    /// not end it: `run` holds its own `Arc<LocalConn>`, so the channel stays open and the loop
    /// keeps reading. Until the re-review's probe B that meant a signed-out machine could still
    /// rewrite its `local_devices` row — hostname, os, home and `last_seen` — and the row stayed
    /// `connected = true` for ever, because the teardown only clears it while this connection is
    /// still the registered one.
    closed: Notify,
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
        closed: Notify::new(),
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
            _ = conn.closed.notified() => {
                tracing::info!(%device_id, "local helper cut off: its device was signed out");
                break;
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

/// Unregister a helper socket, fail whatever it was asked to do, and end the connection.
/// Revoking the device row does not reach the socket: the helper registered once and stays in
/// `local_conns` until it disconnects, so without this a signed-out machine still answers ops.
///
/// All three steps are needed. Unregistering alone leaves `run` reading from a socket the
/// account has signed out, which keeps writing the machine's `local_devices` row and leaves it
/// showing as online for ever, because the teardown in `run` only clears `connected` while this
/// connection is still the registered one — which unregistering has just made false. So the
/// `connected` flag is cleared here instead, and `closed` stops the loop.
pub(crate) async fn disconnect(state: &AppState, device_id: &str, why: &str) {
    let Some(conn) = state.local_conns.lock().await.remove(device_id) else {
        return;
    };
    fail_pending(&conn, why).await;
    let _ = state.db.local_device_seen(device_id, false);
    conn.closed.notify_one();
}

/// `approval_requests.kind` for a local op.
const KIND: &str = "local";
const CMD_MAX: usize = 16 * 1024;
const PATH_MAX: usize = 4 * 1024;
const CHAT_ID_MAX: usize = 128;

fn bad(why: &str) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, why.to_owned())
}

/// The one answer for an op the relay holds no usable approval for, whatever the reason:
/// unknown id, still pending, denied, already used, too old, or made for another device.
fn not_approved() -> (StatusCode, String) {
    (StatusCode::FORBIDDEN, "not_approved".to_owned())
}

/// What a box must hold to ask anything of a computer: its own project, and a helper that is its
/// own user's and still signed in.
fn reachable(
    state: &AppState,
    principal: &Principal,
    project_id: &str,
    device_id: &str,
) -> Result<LocalDevice, (StatusCode, String)> {
    if !state.allows(principal, project_id) {
        return Err((StatusCode::FORBIDDEN, "not your project".into()));
    }
    if !matches!(principal, Principal::Box { .. } | Principal::Admin) {
        return Err((StatusCode::FORBIDDEN, "box token required".into()));
    }
    if state.db.project(project_id).map_err(internal)?.is_none() {
        return Err((StatusCode::NOT_FOUND, "no such project".into()));
    }
    // The project is the caller's (`allows` above); the computer has to be the caller's too.
    // Without this a box could run ops against any helper whose device id it had seen.
    if !owns_device(state, principal, device_id)? {
        return Err(no_such_device());
    }
    // And it has to still be signed in. `disconnect` takes a revoked device out of
    // `local_conns`, but that is a teardown racing a request rather than a check: the
    // re-review's probe C ran file operations against a signed-out computer for the whole of a
    // hung Matrix logout. The map is a cache of who is reachable; `devices` is the authority.
    if !state.device_is_live(device_id) {
        return Err(no_such_device());
    }
    state
        .db
        .local_device(device_id)
        .map_err(internal)?
        .ok_or_else(no_such_device)
}

/// Parse a body that carries an op under `"op"`, refusing any field the op's own shape does not
/// have. `LocalOp` cannot say `deny_unknown_fields` itself (it is flattened into the helper's
/// frame), so the check is that the op serializes back to exactly what was sent. An explicit
/// `null` for an optional field is refused with the rest.
fn parse_exact<T: serde::de::DeserializeOwned>(
    raw: serde_json::Value,
    op_of: impl Fn(&T) -> &LocalOp,
) -> Result<T, (StatusCode, String)> {
    let sent = raw.get("op").cloned();
    let body: T = serde_json::from_value(raw).map_err(|e| bad(&e.to_string()))?;
    if serde_json::to_value(op_of(&body)).ok() != sent {
        return Err(bad("unknown field in op"));
    }
    Ok(body)
}

fn check_bounds(op: &LocalOp) -> Result<(), (StatusCode, String)> {
    let ok = match op {
        LocalOp::Run {
            cmd,
            cwd,
            timeout_s,
        } => {
            !cmd.is_empty()
                && cmd.len() <= CMD_MAX
                && cwd.as_ref().is_none_or(|d| d.len() <= PATH_MAX)
                && timeout_s.is_none_or(|t| (1..=LOCAL_RUN_TIMEOUT_MAX_S).contains(&t))
        }
        LocalOp::ReadFile { path, .. } | LocalOp::ListDir { path } => {
            !path.is_empty() && path.len() <= PATH_MAX
        }
    };
    if ok {
        Ok(())
    } else {
        Err(bad("op is empty or out of bounds"))
    }
}

/// `POST /projects/{id}/local/{device}/requests` (box token): register the op a card is about to
/// ask for. The answer is the id the card carries; nothing runs.
pub async fn request(
    State(state): State<AppState>,
    principal: Principal,
    Path((project_id, device_id)): Path<(String, String)>,
    Json(raw): Json<serde_json::Value>,
) -> ApiResult {
    reachable(&state, &principal, &project_id, &device_id)?;
    // A request belongs to a user, so the admin token, which acts for none, cannot make one.
    let Principal::Box { user_id } = &principal else {
        return Err((StatusCode::FORBIDDEN, "box token required".into()));
    };
    let body: LocalRequestCreate = parse_exact(raw, |b: &LocalRequestCreate| &b.op)?;
    check_bounds(&body.op)?;
    if body.chat_id.len() > CHAT_ID_MAX {
        return Err(bad("chat_id is too long"));
    }
    let id = ulid();
    state
        .db
        .approval_create(&ApprovalRequest {
            id: id.clone(),
            kind: KIND.to_owned(),
            user_id: user_id.clone(),
            project_id,
            target: device_id,
            chat_id: body.chat_id,
            payload: serde_json::to_string(&body.op).map_err(internal)?,
            status: "pending".to_owned(),
            created_at: arbos_core::ids::now_ms(),
        })
        .map_err(internal)?;
    Ok(Json(LocalRequestCreated { id }).into_response())
}

/// `POST /local/requests/{id}/decide` (device token): the user's answer to one request.
///
/// Two things make it the user's answer to this command and not something a box arranged. The
/// caller is a signed-in device of the user the request belongs to: a box token is refused, and
/// in a shared chat nobody else's device can answer for the owner's computer. And the body
/// echoes the op the card showed, which has to be the op stored when the box asked. The card's
/// payload lives in the chat document, which the box writes, so a card that shows one command
/// for a request made for another is refused here.
pub async fn decide(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(raw): Json<serde_json::Value>,
) -> ApiResult {
    let Principal::Device {
        id: device,
        user_id,
        ..
    } = &principal
    else {
        return Err((StatusCode::FORBIDDEN, "device token required".into()));
    };
    let body: LocalDecide = parse_exact(raw, |b: &LocalDecide| &b.op)?;
    // Another user's request answers like one that does not exist.
    let request = state
        .db
        .approval(&id)
        .map_err(internal)?
        .filter(|r| r.kind == KIND && r.user_id == *user_id)
        .ok_or((StatusCode::NOT_FOUND, "no such request".to_owned()))?;
    if serde_json::to_string(&body.op).map_err(internal)? != request.payload {
        return Err((
            StatusCode::CONFLICT,
            "that is not the op this request is for".into(),
        ));
    }
    let wanted = match body.decision {
        LocalDecision::Approve => "approved",
        LocalDecision::Deny => "denied",
    };
    // Only a pending request is decided; a second answer gets the first one back.
    let status = if state
        .db
        .approval_decide(&id, wanted, device)
        .map_err(internal)?
    {
        wanted.to_owned()
    } else {
        let now = state.db.approval(&id).map_err(internal)?;
        now.map_or(request.status, |r| r.status)
    };
    Ok(Json(json!({ "status": status })).into_response())
}

/// The body of an exec: one `LocalOp`, and the approval it runs under.
#[derive(Deserialize)]
pub struct ExecBody {
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(flatten)]
    pub op: LocalOp,
}

/// `POST /projects/{id}/local/{device}/exec` (box token).
///
/// With a `request_id` the relay holds as approved, that approval is spent and the op that runs
/// is the one stored with it, not the one in this body. Without one, a read runs when the device
/// allows reads without asking, and anything else is refused under `ARBOS_LOCAL_APPROVAL=enforce`
/// and logged otherwise.
pub async fn exec(
    State(state): State<AppState>,
    principal: Principal,
    Path((project_id, device_id)): Path<(String, String)>,
    Json(body): Json<ExecBody>,
) -> ApiResult {
    let device = reachable(&state, &principal, &project_id, &device_id)?;
    let conn = state.local_conns.lock().await.get(&device_id).cloned();
    let Some(conn) = conn else {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "kind": "error", "message": "device is offline" })),
        )
            .into_response());
    };
    // Spent only now, with a helper to send it to: an offline computer costs the user no approval.
    let claimed = match (&body.request_id, principal.user_id()) {
        (Some(request_id), Some(user)) => state
            .db
            .approval_claim(request_id, KIND, user, &project_id, &device_id)
            .map_err(internal)?,
        (Some(_), None) | (None, _) => None,
    };
    let op = match claimed {
        Some(stored) => serde_json::from_str::<LocalOp>(&stored).map_err(internal)?,
        // The admin token is the operator's and belongs to no user, so there is nobody to ask.
        None if matches!(principal, Principal::Admin) => body.op,
        None if body.op.is_read() && device.allow_reads => body.op,
        None if state.cfg.local_approval_enforce => return Err(not_approved()),
        None => {
            tracing::warn!(
                %project_id,
                %device_id,
                op = body.op.name(),
                presented = body.request_id.is_some(),
                "local: op ran without an approved request (ARBOS_LOCAL_APPROVAL is not enforce)"
            );
            body.op
        }
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

    pub(super) fn helper(state: &AppState, owner_device: &str) {
        state
            .db
            .local_device_hello(owner_device, "mac.local", "macos", "/Users/t")
            .expect("hello");
    }

    pub(super) fn bare(op: LocalOp) -> ExecBody {
        ExecBody {
            request_id: None,
            op,
        }
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
    pub(super) fn status_of(r: ApiResult) -> StatusCode {
        match r {
            Ok(res) => res.status(),
            Err((code, _)) => code,
        }
    }

    pub(super) fn box_of(user: &str) -> Principal {
        Principal::Box {
            user_id: user.to_owned(),
        }
    }

    pub(super) fn project_of(state: &AppState, id: &str, user: &str) {
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
            Json(bare(LocalOp::ListDir { path: "/".into() })),
        )
        .await;
        assert_eq!(
            status_of(r),
            StatusCode::NOT_FOUND,
            "Alice's box reached a helper belonging to Bob"
        );
    }

    /// Adversarial re-review of #2460, M2's second half. `disconnect` takes a revoked device
    /// out of `local_conns`, but that is a teardown racing a request rather than a check: with a
    /// homeserver that never answers, the row was revoked in milliseconds and the device sat in
    /// the map for the whole ten-second Matrix timeout, dispatching file operations against a
    /// computer the account had just signed out. `devices` is the authority, so `exec` asks it.
    #[tokio::test]
    async fn file_operations_are_refused_against_a_signed_out_computer() {
        let state = test_state(AuthMode::Github);
        let (alice, dev) = person(&state, 30);
        let Principal::Device { id, .. } = &dev else {
            panic!("device")
        };
        let id = id.clone();
        helper(&state, &id);
        project_of(&state, "p_alice", &alice);

        // Still signed in: the op gets as far as looking for a live socket, which is the
        // "device is offline" answer rather than a refusal.
        let before = exec(
            State(state.clone()),
            box_of(&alice),
            Path(("p_alice".to_owned(), id.clone())),
            Json(bare(LocalOp::ListDir { path: "/".into() })),
        )
        .await;
        assert_eq!(status_of(before), StatusCode::SERVICE_UNAVAILABLE);

        state.db.revoke_device_fully(&id).expect("revoke");
        let after = exec(
            State(state.clone()),
            box_of(&alice),
            Path(("p_alice".to_owned(), id.clone())),
            Json(bare(LocalOp::ListDir { path: "/".into() })),
        )
        .await;
        assert_eq!(
            status_of(after),
            StatusCode::NOT_FOUND,
            "a signed-out computer is not reachable, whatever is still in local_conns"
        );
    }

    /// Adversarial re-review of #2460, L1. Unregistering the helper is not the same as ending
    /// it: `run` holds its own `Arc<LocalConn>`, so the loop kept reading from a socket whose
    /// device had been signed out, and the `local_devices` row stayed `connected = true` for
    /// ever because the teardown only clears that flag while the connection is still the
    /// registered one — which unregistering has just made false.
    #[tokio::test]
    async fn signing_a_device_out_ends_its_helper_socket_and_marks_it_offline() {
        let state = test_state(AuthMode::Github);
        let (_alice, dev) = person(&state, 31);
        let Principal::Device { id, .. } = &dev else {
            panic!("device")
        };
        let id = id.clone();
        helper(&state, &id);
        let _ = state.db.local_device_seen(&id, true);

        let (tx, _rx) = mpsc::channel::<String>(4);
        let conn = Arc::new(LocalConn {
            tx,
            pending: Mutex::new(HashMap::new()),
            closed: Notify::new(),
        });
        state
            .local_conns
            .lock()
            .await
            .insert(id.clone(), conn.clone());
        // A pending op, so the caller is answered rather than left hanging.
        let (done, waiting) = oneshot::channel();
        conn.pending.lock().await.insert("op1".into(), done);
        // Somebody has to be listening when `disconnect` fires, the way `run`'s select arm is.
        let woken = tokio::spawn({
            let conn = conn.clone();
            async move { conn.closed.notified().await }
        });
        tokio::task::yield_now().await;

        disconnect(&state, &id, "device signed out").await;

        assert!(
            !state.local_conns.lock().await.contains_key(&id),
            "the socket is unregistered"
        );
        assert!(
            matches!(waiting.await, Ok(LocalResult::Error { .. })),
            "whatever it was asked to do is failed rather than left pending"
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(5), woken)
                .await
                .is_ok(),
            "and the read loop is told to stop, so the machine cannot keep writing"
        );
        assert_eq!(
            state.db.local_device(&id).unwrap().map(|d| d.connected),
            Some(false),
            "a signed-out computer must not keep showing as online"
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

#[cfg(test)]
mod approval_tests {
    //! Whether a local op was approved used to be decided inside the box, from a row in a chat
    //! document the box itself writes. These are the relay deciding it instead.

    use axum::body::to_bytes;
    use serde_json::Value;

    use crate::people::tests_support::person;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    use super::owner_tests::{bare, box_of, helper, project_of, status_of};
    use super::*;

    const P: &str = "p_alice";

    struct World {
        state: AppState,
        alice: String,
        phone: Principal,
        /// Alice's computer, with a helper connected.
        mac: String,
        conn: Arc<LocalConn>,
        sent: mpsc::Receiver<String>,
    }

    async fn world(enforce: bool) -> World {
        let mut state = test_state(AuthMode::Github);
        Arc::get_mut(&mut state.cfg)
            .expect("cfg")
            .local_approval_enforce = enforce;
        let (alice, phone) = person(&state, 41);
        let mac = ulid();
        state
            .db
            .insert_device(&mac, "d", &ulid(), &alice, "github")
            .expect("device");
        helper(&state, &mac);
        project_of(&state, P, &alice);
        let (conn, sent) = connect(&state, &mac).await;
        World {
            state,
            alice,
            phone,
            mac,
            conn,
            sent,
        }
    }

    async fn connect(state: &AppState, device: &str) -> (Arc<LocalConn>, mpsc::Receiver<String>) {
        let (tx, sent) = mpsc::channel::<String>(4);
        let conn = Arc::new(LocalConn {
            tx,
            pending: Mutex::new(HashMap::new()),
            closed: Notify::new(),
        });
        state
            .local_conns
            .lock()
            .await
            .insert(device.to_owned(), conn.clone());
        (conn, sent)
    }

    fn run(cmd: &str) -> LocalOp {
        LocalOp::Run {
            cmd: cmd.to_owned(),
            cwd: None,
            timeout_s: None,
        }
    }

    impl World {
        /// The box asks; the id of the stored request.
        async fn ask(&self, device: &str, op: &LocalOp) -> String {
            let r = request(
                State(self.state.clone()),
                box_of(&self.alice),
                Path((P.to_owned(), device.to_owned())),
                Json(json!({ "op": op, "chat_id": "c1" })),
            )
            .await
            .expect("request");
            let bytes = to_bytes(r.into_body(), 1 << 20).await.unwrap();
            let v: Value = serde_json::from_slice(&bytes).unwrap();
            v["id"].as_str().expect("id").to_owned()
        }

        async fn answer(
            &self,
            who: Principal,
            id: &str,
            decision: &str,
            op: &LocalOp,
        ) -> ApiResult {
            decide(
                State(self.state.clone()),
                who,
                Path(id.to_owned()),
                Json(json!({ "decision": decision, "op": op })),
            )
            .await
        }

        async fn approve(&self, id: &str, op: &LocalOp) {
            let r = self.answer(self.phone.clone(), id, "approve", op).await;
            assert_eq!(status_of(r), StatusCode::OK);
        }

        fn status(&self, id: &str) -> String {
            self.state.db.approval(id).unwrap().expect("row").status
        }

        /// The box execs. The status it got, and the op the helper was sent, if any.
        async fn exec(&mut self, device: &str, body: ExecBody) -> (StatusCode, Option<LocalOp>) {
            let mut call = tokio::spawn(exec(
                State(self.state.clone()),
                box_of(&self.alice),
                Path((P.to_owned(), device.to_owned())),
                Json(body),
            ));
            tokio::select! {
                r = &mut call => (status_of(r.unwrap()), None),
                Some(text) = self.sent.recv() => {
                    let LocalServerFrame::Exec(sent) = serde_json::from_str(&text).unwrap() else {
                        panic!("exec frame")
                    };
                    let waiter = self.conn.pending.lock().await.remove(&sent.id).expect("pending");
                    let _ = waiter.send(LocalResult::Error { message: "test".into() });
                    (status_of(call.await.unwrap()), Some(sent.op))
                }
            }
        }
    }

    fn with(id: &str, op: LocalOp) -> ExecBody {
        ExecBody {
            request_id: Some(id.to_owned()),
            op,
        }
    }

    /// The hole itself: a box token and nothing else.
    #[tokio::test]
    async fn a_box_token_alone_runs_nothing() {
        let mut w = world(true).await;
        let mac = w.mac.clone();
        assert_eq!(
            w.exec(&mac, bare(run("id"))).await,
            (StatusCode::FORBIDDEN, None)
        );
        assert_eq!(
            w.exec(&mac, with("made-up", run("id"))).await,
            (StatusCode::FORBIDDEN, None)
        );
    }

    /// Until apps and boxes send approvals the relay only watches: today's exec still runs.
    #[tokio::test]
    async fn observe_mode_refuses_nothing() {
        let mut w = world(false).await;
        let mac = w.mac.clone();
        assert_eq!(
            w.exec(&mac, bare(run("id"))).await,
            (StatusCode::OK, Some(run("id")))
        );
    }

    #[tokio::test]
    async fn an_approval_runs_its_own_op_once() {
        let mut w = world(true).await;
        let mac = w.mac.clone();
        let id = w.ask(&mac, &run("ls")).await;
        assert_eq!(
            w.exec(&mac, with(&id, run("ls"))).await.0,
            StatusCode::FORBIDDEN,
            "asked, not yet answered"
        );
        w.approve(&id, &run("ls")).await;

        // The box says one thing in the exec body; what was approved is what is sent.
        assert_eq!(
            w.exec(&mac, with(&id, run("rm -rf ~"))).await,
            (StatusCode::OK, Some(run("ls")))
        );
        assert_eq!(w.status(&id), "used");
        assert_eq!(
            w.exec(&mac, with(&id, run("ls"))).await,
            (StatusCode::FORBIDDEN, None),
            "an approval is spent by its first use"
        );
    }

    #[tokio::test]
    async fn a_denied_or_stale_approval_runs_nothing() {
        let mut w = world(true).await;
        let mac = w.mac.clone();
        let denied = w.ask(&mac, &run("ls")).await;
        let r = w.answer(w.phone.clone(), &denied, "deny", &run("ls")).await;
        assert_eq!(status_of(r), StatusCode::OK);
        assert_eq!(
            w.exec(&mac, with(&denied, run("ls"))).await.0,
            StatusCode::FORBIDDEN
        );

        let stale = w.ask(&mac, &run("ls")).await;
        w.approve(&stale, &run("ls")).await;
        w.state
            .db
            .with(|c| {
                c.execute(
                    "UPDATE approval_requests SET decided_at = decided_at - ?2 WHERE id = ?1",
                    rusqlite::params![stale, crate::db::APPROVAL_USABLE_MS],
                )
            })
            .unwrap();
        assert_eq!(
            w.exec(&mac, with(&stale, run("ls"))).await,
            (StatusCode::FORBIDDEN, None),
            "a yes is good for five minutes"
        );
    }

    /// A request is for one computer. Alice's second machine is hers too, and still not it.
    #[tokio::test]
    async fn an_approval_for_one_computer_does_not_run_on_another() {
        let mut w = world(true).await;
        let mac = w.mac.clone();
        let other = ulid();
        w.state
            .db
            .insert_device(&other, "d", &ulid(), &w.alice, "github")
            .expect("device");
        helper(&w.state, &other);
        let (_conn, _sent) = connect(&w.state, &other).await;

        let id = w.ask(&mac, &run("ls")).await;
        w.approve(&id, &run("ls")).await;
        assert_eq!(
            w.exec(&other, with(&id, run("ls"))).await,
            (StatusCode::FORBIDDEN, None)
        );
        assert_eq!(w.status(&id), "approved", "and it is not spent by the try");
    }

    #[tokio::test]
    async fn only_the_owner_decides_and_only_for_the_op_that_was_asked() {
        let w = world(true).await;
        let id = w.ask(&w.mac, &run("ls -la")).await;

        let (_bob, bob_phone) = person(&w.state, 42);
        let r = w.answer(bob_phone, &id, "approve", &run("ls -la")).await;
        assert_eq!(status_of(r), StatusCode::NOT_FOUND, "another user's device");
        let r = w
            .answer(box_of(&w.alice), &id, "approve", &run("ls -la"))
            .await;
        assert_eq!(status_of(r), StatusCode::FORBIDDEN, "the box itself");
        // One byte off what the box registered: the card showed something else.
        let r = w
            .answer(w.phone.clone(), &id, "approve", &run("ls -lb"))
            .await;
        assert_eq!(status_of(r), StatusCode::CONFLICT);
        assert_eq!(w.status(&id), "pending", "none of those decided anything");

        w.approve(&id, &run("ls -la")).await;
        let r = w
            .answer(w.phone.clone(), &id, "deny", &run("ls -la"))
            .await
            .expect("second answer");
        let bytes = to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["status"], "approved", "the first answer stands");
    }

    /// "Allow reads without asking" is the relay's own setting, and it covers reads only.
    #[tokio::test]
    async fn reads_run_unasked_only_where_the_device_allows_them() {
        let mut w = world(true).await;
        let mac = w.mac.clone();
        let read = LocalOp::ListDir { path: "/".into() };
        assert_eq!(
            w.exec(&mac, bare(read.clone())).await,
            (StatusCode::FORBIDDEN, None)
        );
        w.state.db.local_device_set_allow_reads(&mac, true).unwrap();
        assert_eq!(
            w.exec(&mac, bare(read.clone())).await,
            (StatusCode::OK, Some(read))
        );
        assert_eq!(
            w.exec(&mac, bare(run("id"))).await,
            (StatusCode::FORBIDDEN, None),
            "the switch never covers a command"
        );
    }

    #[tokio::test]
    async fn a_request_is_validated_and_a_project_holds_a_bounded_number() {
        let w = world(true).await;
        let ask = |body: Value| {
            request(
                State(w.state.clone()),
                box_of(&w.alice),
                Path((P.to_owned(), w.mac.clone())),
                Json(body),
            )
        };
        for body in [
            json!({ "op": { "op": "run", "cmd": "ls", "sudo": true }, "chat_id": "c" }),
            json!({ "op": { "op": "run", "cmd": "ls" }, "chat_id": "c", "extra": 1 }),
            json!({ "op": { "op": "run", "cmd": "" }, "chat_id": "c" }),
            json!({ "op": { "op": "run", "cmd": "x".repeat(CMD_MAX + 1) }, "chat_id": "c" }),
            json!({ "op": { "op": "run", "cmd": "ls", "timeout_s": 601 }, "chat_id": "c" }),
            json!({ "op": { "op": "list_dir", "path": "/".repeat(PATH_MAX + 1) }, "chat_id": "c" }),
        ] {
            let shown = body.to_string().chars().take(60).collect::<String>();
            assert_eq!(
                status_of(ask(body).await),
                StatusCode::BAD_REQUEST,
                "{shown}"
            );
        }
        // A device token cannot register a request; only the box that will raise the card.
        let r = request(
            State(w.state.clone()),
            w.phone.clone(),
            Path((P.to_owned(), w.mac.clone())),
            Json(json!({ "op": run("ls"), "chat_id": "c" })),
        )
        .await;
        assert_eq!(status_of(r), StatusCode::FORBIDDEN);

        let first = w.ask(&w.mac, &run("ls")).await;
        for _ in 0..crate::db::APPROVALS_PENDING_MAX {
            w.ask(&w.mac, &run("ls")).await;
        }
        assert_eq!(
            w.status(&first),
            "denied",
            "the oldest pending one gives way"
        );
    }
}
