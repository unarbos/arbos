//! Admin surface for self-serve accounts (admin token only): users with their credits, grants
//! and caps, and the global kill switch. `arbosctl admin …` is the CLI over these.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use arbos_core::ids::{now_ms, ulid};

use crate::auth::Principal;
use crate::db_accounts::KillSwitch;
use crate::signin::api_error;
use crate::state::AppState;

fn admin_only(principal: &Principal) -> Result<(), Box<Response>> {
    match principal {
        Principal::Admin => Ok(()),
        Principal::Device { .. } | Principal::Box { .. } => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "admin token required",
        ))),
    }
}

/// `GET /admin/users`: every account with balance, caps, today's spend, and its machine.
pub async fn list_users(State(state): State<AppState>, principal: Principal) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let users = match state.db.list_users() {
        Ok(u) => u,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let mut out = Vec::with_capacity(users.len());
    for u in users {
        let (bs, since) = state.box_state(&u.id).await;
        let sb = state.db.user_box(&u.id).ok().flatten();
        let mut v = crate::credits::summary(&state, &u.id);
        v["id"] = json!(u.id);
        v["login"] = json!(u.login);
        v["github_id"] = json!(u.github_id);
        v["test"] = json!(u.test);
        v["created_at"] = json!(u.created_at);
        v["last_login_at"] = json!(u.last_login_at);
        v["machine"] = json!({
            "exists": sb.is_some(),
            "state": bs,
            "since": since,
            "sandbox_id": sb.as_ref().and_then(|s| s.sandbox_id.clone()),
            "snapshot": sb.as_ref().and_then(|s| s.snapshot.clone()),
            "image_outdated": sb.as_ref().is_some_and(|s| crate::upgrade::image_outdated(&state, s)),
            "upgrade": crate::upgrade::status_json(&state, &u.id).await,
            "agentd": state.db.box_agentd(&u.id).ok().flatten().map(|a| json!({ "sha": a.sha, "version": a.version, "image": a.image, "seed_sha": a.seed_sha, "reported_at": a.reported_at })),
        });
        out.push(v);
    }
    Json(out).into_response()
}

#[derive(Deserialize)]
pub struct CreditsBody {
    /// Add this many USD to the balance.
    #[serde(default)]
    pub grant_usd: Option<f64>,
    /// Daily cap; present-and-null means unlimited.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub daily_cap_usd: Option<Option<f64>>,
    #[serde(default)]
    pub allow_negative: Option<bool>,
    #[serde(default)]
    pub note: Option<String>,
}

fn deserialize_double_option<'de, D>(d: D) -> Result<Option<Option<f64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<f64>::deserialize(d).map(Some)
}

/// `PUT /admin/users/{id}/credits {grant_usd?, daily_cap_usd?: number|null, allow_negative?}`.
pub async fn set_credits(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<CreditsBody>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    if let Some(usd) = b.grant_usd {
        if let Err(e) = state.db.grant_credits(
            &id,
            usd,
            b.note.as_deref().unwrap_or("admin grant"),
            "admin",
        ) {
            return api_error(StatusCode::BAD_REQUEST, "bad_grant", e.to_string());
        }
    }
    if b.daily_cap_usd.is_some() || b.allow_negative.is_some() {
        if let Err(e) = state
            .db
            .set_user_limits(&id, b.daily_cap_usd, b.allow_negative)
        {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
        }
    }
    let mut v = crate::credits::summary(&state, &id);
    v["id"] = json!(id);
    Json(v).into_response()
}

#[derive(Deserialize)]
pub struct ProjectCapBody {
    /// Daily cap in USD; present-and-null means no project cap.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub daily_cap_usd: Option<Option<f64>>,
}

/// `PUT /admin/projects/{id}/spend-cap {daily_cap_usd: number|null}`: the per-project daily cap
/// (`projects.spend_cap_daily`), `null` = unlimited. Before this the only lever was an UPDATE on
/// the relay DB (Jacob's 11 projects, 2026-10-26 22:15 UTC).
pub async fn set_project_cap(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<ProjectCapBody>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let Some(cap) = b.daily_cap_usd else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "daily_cap_usd (number or null) is required",
        );
    };
    let stored = match cap {
        Some(v) if v.is_finite() && v >= 0.0 => v,
        Some(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "daily_cap_usd must be a non-negative number or null",
            )
        }
        None => crate::credits::PROJECT_CAP_UNLIMITED,
    };
    match state.db.set_project_spend_cap(&id, stored) {
        Ok(true) => Json(json!({ "id": id, "daily_cap_usd": cap, "unlimited": cap.is_none() }))
            .into_response(),
        Ok(false) => api_error(StatusCode::NOT_FOUND, "not_found", "no such project"),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct TestUserBody {
    pub handle: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// `POST /admin/users/test {handle, name?, avatar_url?}`: a user with no GitHub account behind
/// it (`test_<ulid>`), default credits, and a signed-in device with its token pair — for
/// recordings and two-account tests in GitHub mode.
pub async fn create_test_user(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<TestUserBody>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let handle = b.handle.trim().trim_start_matches('@').to_owned();
    if handle.is_empty()
        || handle.len() > 39
        || !handle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_handle",
            "handle must look like a GitHub login",
        );
    }
    if let Ok(Some(_)) = state.db.user_by_handle(&handle) {
        return api_error(
            StatusCode::CONFLICT,
            "handle_taken",
            "a user with that handle exists",
        );
    }
    let id = format!("test_{}", ulid());
    if let Err(e) = state.db.insert_test_user(
        &id,
        &handle,
        b.name.as_deref(),
        b.avatar_url.as_deref(),
        state.cfg.default_daily_cap_usd,
    ) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    let _ = state
        .db
        .grant_credits(&id, state.cfg.default_grant_usd, "test user grant", "admin");
    let device_id = ulid();
    if let Err(e) = state.db.insert_device(
        &device_id,
        &format!("{handle} (test device)"),
        &arbos_core::auth::hash(&format!("jwt:{device_id}:{}", ulid())),
        &id,
        "github",
    ) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    match crate::signin::issue_tokens(&state, &id, &device_id) {
        Ok(dto) => {
            let mut tokens = crate::signin::token_response_json(&dto);
            tokens["test"] = json!(true);
            (StatusCode::CREATED, Json(tokens)).into_response()
        }
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

#[derive(Deserialize, Default)]
pub struct AddDeviceBody {
    #[serde(default)]
    pub name: Option<String>,
}

/// `POST /admin/users/{id}/devices {"name"?}` — attach another signed-in device to a user and
/// hand back its token pair (same shape as `/auth/token`), so a Mac and a simulator can be two
/// devices of one account for push-path and archive-sync proofs. Test users only unless the
/// body says `"name"` for a real one and the caller knows what they are doing: refused for
/// non-test users without a name.
pub async fn add_device(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    body: Option<Json<AddDeviceBody>>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let b = body.map(|b| b.0).unwrap_or_default();
    let user = match state.db.user(&id) {
        Ok(Some(u)) => u,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "not_found", "no such user"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    if !user.test && b.name.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "a device for a real account needs a name (this mints a signed-in session for them)",
        );
    }
    let n = state.db.devices_of_user(&id).map(|d| d.len()).unwrap_or(0);
    let name = b
        .name
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            format!(
                "{} (test device {})",
                user.login.clone().unwrap_or_else(|| id.clone()),
                n + 1
            )
        });
    let device_id = ulid();
    if let Err(e) = state.db.insert_device(
        &device_id,
        &name,
        &arbos_core::auth::hash(&format!("jwt:{device_id}:{}", ulid())),
        &id,
        "github",
    ) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    tracing::info!(user = %id, device = %device_id, %name, "admin attached a device");
    match crate::signin::issue_tokens(&state, &id, &device_id) {
        Ok(dto) => {
            let mut tokens = crate::signin::token_response_json(&dto);
            tokens["device_name"] = json!(name);
            tokens["test"] = json!(user.test);
            (StatusCode::CREATED, Json(tokens)).into_response()
        }
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `DELETE /admin/users/{id}` — test users only; refused while the user still owns projects or a
/// box (delete those first so nothing is orphaned).
pub async fn delete_test_user(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    match state.db.user(&id) {
        Ok(Some(u)) if u.test => {}
        Ok(Some(_)) => {
            return api_error(
                StatusCode::FORBIDDEN,
                "not_test_user",
                "only test users can be deleted",
            )
        }
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "not_found", "no such user"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
    if !state
        .db
        .projects_of_user(&id)
        .unwrap_or_default()
        .is_empty()
    {
        return api_error(
            StatusCode::CONFLICT,
            "has_projects",
            "delete the user's projects first",
        );
    }
    if state.db.user_box(&id).ok().flatten().is_some() {
        return api_error(
            StatusCode::CONFLICT,
            "has_box",
            "stop and delete the user's box first",
        );
    }
    match state.db.delete_test_user(&id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => api_error(
            StatusCode::FORBIDDEN,
            "not_test_user",
            "only test users can be deleted",
        ),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `GET /admin/killswitch`.
pub async fn get_kill_switch(State(state): State<AppState>, principal: Principal) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let day = crate::proxy::start_of_utc_day_ms();
    let (boxes, baseline) = crate::credits::machine_baseline_usd_per_day(&state);
    Json(json!({
        "kill_switch": crate::credits::kill_switch(&state),
        "global_daily_cap_usd": state.cfg.global_daily_cap_usd,
        "spent_today_usd": state.db.debited_since_all(day).unwrap_or(0.0),
        "machine": {
            "always_on": state.cfg.always_on,
            "boxes": boxes,
            "baseline_usd_per_day": baseline,
            "today_usd": state.db.machine_cost_since_all(day).unwrap_or(0.0),
            "alert_cap_usd": state.cfg.global_machine_daily_cap_usd,
        },
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct KillSwitchBody {
    pub on: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

/// `PUT /admin/killswitch {on, reason?}`: on = the relay refuses every spend (503 `read_only`).
pub async fn set_kill_switch(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<KillSwitchBody>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let ks = KillSwitch {
        on: b.on,
        reason: if b.on {
            Some(
                b.reason
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| "switched off by the operator".to_owned()),
            )
        } else {
            None
        },
        by: Some("admin".to_owned()),
        at: now_ms(),
        auto: false,
    };
    if let Err(e) = state.db.set_kill_switch(&ks) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    tracing::warn!(on = b.on, reason = ?ks.reason, "kill switch set by admin");
    Json(json!({ "kill_switch": ks })).into_response()
}

/// `GET /admin/push`: whether APNs is configured, and every registered device token (the token
/// itself shortened — it is a credential for that one device).
pub async fn push_status(State(state): State<AppState>, principal: Principal) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let tokens = match state.db.all_push_tokens() {
        Ok(t) => t,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    Json(json!({
        "push": state.push.is_on(),
        "pipeline": state.push.status(),
        "tokens": tokens.iter().map(|t| json!({
            "device_id": t.device_id, "user_id": t.user_id, "platform": t.platform,
            "environment": t.environment, "bundle_id": t.bundle_id, "app_version": t.app_version,
            "token": format!("{}…", t.token.chars().take(8).collect::<String>()),
            "updated_at": t.updated_at,
        })).collect::<Vec<_>>(),
    }))
    .into_response()
}

/// `POST /admin/push/drain`: work the outbox now (what the worker does on its own); answers
/// how many rows were handled and the pipeline status after.
pub async fn push_drain(State(state): State<AppState>, principal: Principal) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let handled = state.push.drain().await;
    Json(json!({ "handled": handled, "pipeline": state.push.status() })).into_response()
}

#[derive(Deserialize)]
pub struct TestPushBody {
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
}

/// `POST /admin/push/test {user_id | device_id, title?, body?}`: one plain alert to each of
/// those devices, with APNs' answer per device — proves the key, topic, and token end to end.
pub async fn push_test(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<TestPushBody>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    if !state.push.is_on() {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "push_off",
            "APNS_KEY_P8 / APNS_KEY_ID / APNS_TEAM_ID are not set on this relay",
        );
    }
    let tokens = match (&b.user_id, &b.device_id) {
        (Some(u), _) => state.db.push_tokens_of_user(u),
        (None, Some(d)) => state
            .db
            .all_push_tokens()
            .map(|all| all.into_iter().filter(|t| &t.device_id == d).collect()),
        (None, None) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "user_id or device_id is required",
            )
        }
    };
    let tokens = match tokens {
        Ok(t) => t,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let title = b.title.unwrap_or_else(|| "Arbos".to_owned());
    let body = b
        .body
        .unwrap_or_else(|| format!("Test notification from the relay at {}", now_ms()));
    let mut results = Vec::with_capacity(tokens.len());
    for t in tokens {
        let out = crate::push::Outgoing {
            device_token: t.token.clone(),
            environment: t.environment.clone(),
            push_type: "alert",
            topic: None,
            collapse_id: Some(format!("test:{}", ulid())),
            payload: json!({
                "aps": { "alert": { "title": title, "body": body }, "sound": "default" },
                "test": true,
            }),
        };
        let outcome = state.push.send_now(out).await;
        if matches!(outcome, crate::push::SendOutcome::TokenInvalid { .. }) {
            let _ = state.db.delete_push_token_value(&t.token);
        }
        results.push(json!({
            "device_id": t.device_id, "environment": t.environment,
            "result": match &outcome {
                crate::push::SendOutcome::Accepted => "sent".to_owned(),
                crate::push::SendOutcome::TokenInvalid { reason } => format!("token gone (forgotten): {reason}"),
                crate::push::SendOutcome::Permanent { .. } | crate::push::SendOutcome::Transient { .. } => format!("failed: {outcome}"),
            },
        }));
    }
    Json(json!({ "sent_to": results.len(), "results": results })).into_response()
}

#[derive(Deserialize, Default)]
pub struct UsageDayQuery {
    /// UTC day `YYYY-MM-DD`; default today.
    #[serde(default)]
    pub day: Option<String>,
}

/// Start of a UTC calendar day in ms, from `YYYY-MM-DD`.
fn day_start_ms(day: &str) -> Option<i64> {
    let mut it = day.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Days since 1970-01-01 (proleptic Gregorian), the usual civil-from-days arithmetic.
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400_000)
}

fn today_utc() -> String {
    let days = now_ms().div_euclid(86_400_000);
    // Inverse of `day_start_ms`.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// `GET /admin/usage/{purpose}?day=YYYY-MM-DD` — spend of one tagged purpose (`x-arbos-purpose`,
/// e.g. `memory-tree`) for a UTC day, split real/test by the project owner, with totals: the
/// audit numbers without reading the relay's SQLite file (which also holds key material).
pub async fn usage_by_purpose(
    State(state): State<AppState>,
    principal: Principal,
    Path(purpose): Path<String>,
    axum::extract::Query(q): axum::extract::Query<UsageDayQuery>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let purpose: String = purpose
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect::<String>()
        .to_ascii_lowercase();
    if purpose.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "bad_request", "purpose required");
    }
    let day = q.day.unwrap_or_else(today_utc);
    let Some(start) = day_start_ms(&day) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "day must be YYYY-MM-DD",
        );
    };
    let rows = match state
        .db
        .usage_by_purpose(&purpose, start, start + 86_400_000)
    {
        Ok(r) => r,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let mut real = 0.0;
    let mut test = 0.0;
    for (who, _, _, usd) in &rows {
        if who == "test" {
            test += usd;
        } else {
            real += usd;
        }
    }
    Json(json!({
        "purpose": purpose,
        "day": day,
        "from_ms": start,
        "to_ms": start + 86_400_000,
        "rows": rows.iter().map(|(who, api, calls, usd)| json!({ "who": who, "api": api, "calls": calls, "usd": usd })).collect::<Vec<_>>(),
        "totals": { "real": real, "test": test, "total": real + test },
    }))
    .into_response()
}

#[cfg(test)]
mod day_tests {
    use super::*;

    #[test]
    fn utc_day_arithmetic_round_trips() {
        assert_eq!(day_start_ms("1970-01-01"), Some(0));
        assert_eq!(day_start_ms("2026-10-20"), Some(1_792_454_400_000));
        assert_eq!(day_start_ms("2026-13-01"), None);
        assert_eq!(day_start_ms("nope"), None);
        let today = today_utc();
        let start = day_start_ms(&today).expect("today parses");
        let now = now_ms();
        assert!(start <= now && now < start + 86_400_000);
    }
}

#[derive(serde::Deserialize)]
pub struct DefaultSnapshotBody {
    pub snapshot: String,
}

/// `POST /admin/snapshot/default {"snapshot": "arbos-agent-0.1.72"}` — switch the snapshot new boxes are made
/// from without restarting the relay. The deploy worker calls this after registering an image (and still
/// writes `ARBOS_DAYTONA_SNAPSHOT` to relay.env for the next start); before this every image roll did
/// `systemctl restart arbos-relay`, which dropped every client's websocket (~15 rolls a day).
pub async fn set_default_snapshot(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<DefaultSnapshotBody>,
) -> Response {
    if !matches!(principal, Principal::Admin) {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "admin token required");
    }
    let name = b.snapshot.trim();
    if name.is_empty()
        || name.len() > 80
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/'))
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_snapshot",
            "snapshot name must be 1-80 chars of [A-Za-z0-9-_.:/]",
        );
    }
    let previous = state.default_snapshot();
    if let Ok(mut cur) = state.default_snapshot.write() {
        *cur = name.to_owned();
    }
    tracing::info!(from = %previous, to = %name, "default snapshot switched (no restart)");
    Json(serde_json::json!({ "snapshot": name, "previous": previous })).into_response()
}
