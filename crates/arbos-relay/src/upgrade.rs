//! Box upgrade: roll a user's box onto the relay's current default image.
//!
//! A gVisor box (D89) is recreated in place: the container is stopped and started again from the
//! target image on the same per-user `/work` image, so nothing is copied and the roll takes
//! seconds. Progress is in `GET /machine` (`upgrade.phase`), and both outcomes land in the audit
//! log as `box_upgrade` / `box_upgrade_failed`.
//!
//! Daytona's upgrade was a different thing entirely — pack `/work` into parts, carry them through
//! the relay's spool disk onto a freshly created sandbox, swap the row over, keep the old sandbox
//! as a fallback — and it went with the provider (Jacob, 2026-11-09). Gone with it: the carry
//! (`upgrade_carry`), the measure-step refusals, the retained-sandbox archive and delete sweeper,
//! and `POST /admin/users/{id}/box/repair`, which re-adopted a retained Daytona sandbox. The
//! `box_history` rows those left behind are kept in the database for the operator-run deletion
//! step; no relay code reads them any more.

use std::time::Instant;

use anyhow::{anyhow, Result};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use arbos_core::ids::now_ms;
use arbos_core::wire::BoxState;

use crate::auth::Principal;
use crate::boxes::Provider;
use crate::signin::api_error;
use crate::state::{AppState, UpgradeStatus};

/// Is the box behind the relay's default snapshot?
pub fn image_outdated(state: &AppState, sb: &crate::db::Sandbox) -> bool {
    sb.provider == Provider::Gvisor.as_str()
        && sb
            .snapshot
            .as_deref()
            .is_some_and(|s| *s != state.default_snapshot())
}

/// `upgrade` object for `GET /machine` and `/admin/users`.
pub async fn status_json(state: &AppState, user_id: &str) -> Value {
    match state.upgrading.lock().await.get(user_id).cloned() {
        Some(u) => json!({
            "in_progress": true, "phase": u.phase, "since": u.since,
            "from": u.from_snapshot, "to": u.to_snapshot,
        }),
        None => json!({ "in_progress": false }),
    }
}

/// A planned restart is under way (`/admin/restart`, SIGTERM): no new run may start, or the
/// restart would cut it short with the box half recreated (#2262). Called under the `upgrading`
/// lock, which the restart holds while it checks for runs in flight.
fn refuse_while_restarting(state: &AppState) -> Result<()> {
    if state.restart.box_work_frozen() {
        return Err(anyhow!(
            "{RESTARTING_PHRASE}; the box was not touched, try again after the restart"
        ));
    }
    Ok(())
}

/// Words the deploy worker can match in a refused upgrade.
pub const RESTARTING_PHRASE: &str = "the relay is restarting";

/// Start an upgrade for `user_id` in the background. `Ok(false)` when one is already running or
/// the box is already on the target snapshot.
pub async fn start(state: &AppState, user_id: &str, force: bool) -> Result<bool> {
    let sb = state
        .db
        .user_box(user_id)?
        .ok_or_else(|| anyhow!("no box for user {user_id}"))?;
    // `provider_of` names the retired provider in its message, so a box left on Daytona reads as
    // what it is rather than as an unknown provider.
    if crate::boxes::provider_of(&sb)? != Provider::Gvisor {
        return Err(anyhow!(
            "only gvisor boxes are rolled (this one is {})",
            sb.provider
        ));
    }
    let target = state
        .db
        .user_snapshot(user_id)?
        .unwrap_or_else(|| state.default_snapshot());
    if !force && sb.snapshot.as_deref() == Some(target.as_str()) {
        return Ok(false);
    }
    {
        let mut running = state.upgrading.lock().await;
        if running.contains_key(user_id) {
            return Ok(false);
        }
        refuse_while_restarting(state)?;
        running.insert(
            user_id.to_owned(),
            UpgradeStatus {
                phase: "recreating".into(),
                since: now_ms(),
                from_snapshot: sb.snapshot.clone(),
                to_snapshot: target.clone(),
            },
        );
    }
    let state2 = state.clone();
    let user = user_id.to_owned();
    tokio::spawn(async move {
        let t0 = Instant::now();
        state2
            .set_box_state(&user, BoxState::Waking, Some("upgrading: recreating"))
            .await;
        let outcome = crate::fleet::recreate_with_snapshot(&state2, &user, &target).await;
        state2.upgrading.lock().await.remove(&user);
        let ms = t0.elapsed().as_millis() as i64;
        match outcome {
            Ok(()) => {
                tracing::info!(user = %user, target, ms, "gvisor box upgraded");
                let _ = state2.db.record_audit(
                    "relay-admin",
                    "box_upgrade",
                    &json!({ "ts": now_ms(), "user_id": user, "report": { "provider": "gvisor", "to": target }, "ms": ms }),
                );
            }
            Err(e) => {
                // The deploy worker greps `box upgrade failed user=<uid> error=…` — keep the words.
                tracing::error!(user = %user, error = %format!("{e:#}"), "box upgrade failed");
                let _ = state2.db.record_audit(
                    "relay-admin",
                    "box_upgrade_failed",
                    &json!({ "ts": now_ms(), "user_id": user, "error": format!("{e:#}"), "ms": ms }),
                );
                state2
                    .set_box_state(
                        &user,
                        BoxState::Error,
                        Some(&format!("upgrade failed: {e}")),
                    )
                    .await;
            }
        }
    });
    Ok(true)
}

// ---- handlers -----------------------------------------------------------------------------------

#[derive(serde::Deserialize)]
pub struct UpgradeQuery {
    /// Recreate even when the snapshot already matches.
    #[serde(default)]
    pub force: bool,
}

async fn kick(state: &AppState, user_id: &str, force: bool) -> Response {
    match start(state, user_id, force).await {
        Ok(true) => (
            StatusCode::ACCEPTED,
            Json(json!({ "user_id": user_id, "upgrade": status_json(state, user_id).await })),
        )
            .into_response(),
        Ok(false) => {
            let st = status_json(state, user_id).await;
            if st["in_progress"].as_bool() == Some(true) {
                (
                    StatusCode::ACCEPTED,
                    Json(json!({ "user_id": user_id, "upgrade": st })),
                )
                    .into_response()
            } else {
                Json(json!({ "user_id": user_id, "upgrade": st, "note": "already on the current snapshot (use ?force=true to recreate anyway)" }))
                    .into_response()
            }
        }
        Err(e) if e.to_string().contains(RESTARTING_PHRASE) => {
            api_error(StatusCode::SERVICE_UNAVAILABLE, "restarting", e.to_string())
        }
        Err(e) => api_error(StatusCode::BAD_GATEWAY, "upgrade", e.to_string()),
    }
}

/// `POST /machine/upgrade[?force=true]` — the caller's own box.
pub async fn machine_upgrade(
    State(state): State<AppState>,
    principal: Principal,
    axum::extract::Query(q): axum::extract::Query<UpgradeQuery>,
) -> Response {
    let Principal::Device { user_id, .. } = &principal else {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        );
    };
    if let Err(r) = crate::credits::check(&state, user_id) {
        return crate::credits::refused_response(&r);
    }
    kick(&state, user_id, q.force).await
}

/// `POST /admin/users/{id}/box/upgrade[?force=true]`.
pub async fn admin_upgrade(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<UpgradeQuery>,
) -> Response {
    if !matches!(principal, Principal::Admin) {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "admin token required");
    }
    kick(&state, &id, q.force).await
}
