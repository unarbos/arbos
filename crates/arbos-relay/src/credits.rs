//! Credits and metering for self-serve Arbos. Every user has a ledger (`credit_ledger`): grants
//! in, debits out for inference (what the upstream charged us) and machine time (our Daytona
//! cost, zero margin). Spending is gated in three layers, checked in this order:
//!
//!   1. the global kill switch — manual (`PUT /admin/killswitch`) or tripped automatically when
//!      the relay-wide debits of the UTC day reach `ARBOS_GLOBAL_DAILY_CAP_USD`; the relay is
//!      read-only (503 `read_only`) until it is cleared or the day turns;
//!   2. the user's balance — at or below zero means 402 `credits_exhausted` unless the admin set
//!      `allow_negative`;
//!   3. the user's daily cap — 402 `daily_cap` once today's debits reach it (`None` = unlimited).
//!
//! In `ARBOS_AUTH_MODE=paircode` (self-hosters) nothing is gated or debited: the ledger is only
//! meaningful when accounts are GitHub users.

use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use arbos_core::ids::now_ms;
use arbos_core::wire::BoxState;

use crate::auth::Principal;
use crate::db_accounts::{DebitRefused, KillSwitch};
use crate::proxy::start_of_utc_day_ms;
use crate::signin::api_error;
use crate::state::{AppState, AuthMode};

/// Debits below this are not worth a ledger row (and a metering tick on an idle box rounds to it).
const MIN_DEBIT_USD: f64 = 0.000_001;
const METER_PERIOD: Duration = Duration::from_secs(60);

pub fn metered(state: &AppState) -> bool {
    state.cfg.auth_mode == AuthMode::Github
}

/// The error the apps show for a refused spend.
pub fn refused_response(r: &DebitRefused) -> Response {
    match r {
        DebitRefused::Insufficient { balance_usd, .. } => api_error(
            StatusCode::PAYMENT_REQUIRED,
            "credits_exhausted",
            format!("your credits are used up (balance ${balance_usd:.2}); ask for a top-up"),
        ),
        DebitRefused::DailyCap {
            cap_usd,
            spent_today_usd,
        } => api_error(
            StatusCode::PAYMENT_REQUIRED,
            "daily_cap",
            format!("daily cap of ${cap_usd:.2} reached (${spent_today_usd:.2} spent today); it resets at midnight UTC"),
        ),
        DebitRefused::ReadOnly { reason } => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "read_only",
            format!("Arbos is in read-only mode: {reason}"),
        ),
    }
}

/// The kill switch as it stands now: an automatic trip from an earlier UTC day is cleared, and
/// today's relay-wide debits are compared with the global cap.
pub fn kill_switch(state: &AppState) -> KillSwitch {
    let mut ks = state.db.kill_switch().unwrap_or_default();
    let day = start_of_utc_day_ms();
    if ks.on && ks.auto && ks.at < day {
        ks = KillSwitch::default();
        let _ = state.db.set_kill_switch(&ks);
    }
    if !ks.on {
        let spent = state.db.debited_since_all(day).unwrap_or(0.0);
        if spent >= state.cfg.global_daily_cap_usd {
            ks = KillSwitch {
                on: true,
                reason: Some(format!(
                    "the global daily spend cap of ${:.2} was reached (${spent:.2} today)",
                    state.cfg.global_daily_cap_usd
                )),
                by: Some("relay".to_owned()),
                at: now_ms(),
                auto: true,
            };
            tracing::warn!(
                spent,
                cap = state.cfg.global_daily_cap_usd,
                "kill switch tripped"
            );
            let _ = state.db.set_kill_switch(&ks);
        }
    }
    ks
}

/// A user known only from a device token (paired before GitHub sign-in, e.g. `u_default`,
/// `u_mac`) has no `users` row and so no credits; give them the default account on first
/// contact instead of blocking their box. Idempotent.
pub fn ensure_account(state: &AppState, user_id: &str) {
    if user_id.is_empty() {
        return;
    }
    let known = matches!(state.db.user(user_id), Ok(Some(_)));
    if known {
        return;
    }
    if let Err(e) =
        state
            .db
            .set_user_limits(user_id, Some(Some(state.cfg.default_daily_cap_usd)), None)
    {
        tracing::warn!(error = %e, user = %user_id, "legacy account row");
        return;
    }
    match state.db.grant_credits(
        user_id,
        state.cfg.default_grant_usd,
        "legacy account grant (device-token user)",
        "relay",
    ) {
        Ok(()) => {
            tracing::info!(user = %user_id, usd = state.cfg.default_grant_usd, "legacy user got the default grant")
        }
        Err(e) => tracing::warn!(error = %e, user = %user_id, "legacy grant"),
    }
}

/// Only the relay-wide kill switch (machines: always on unless the operator switches off).
pub fn check_read_only(state: &AppState) -> Result<(), DebitRefused> {
    if !metered(state) {
        return Ok(());
    }
    let ks = kill_switch(state);
    if ks.on {
        return Err(DebitRefused::ReadOnly {
            reason: ks
                .reason
                .unwrap_or_else(|| "switched off by the operator".to_owned()),
        });
    }
    Ok(())
}

/// May this user spend on inference right now? `Ok` or the reason.
pub fn check(state: &AppState, user_id: &str) -> Result<(), DebitRefused> {
    if !metered(state) {
        return Ok(());
    }
    ensure_account(state, user_id);
    check_read_only(state)?;
    // A zero debit runs the balance and cap checks without writing a row.
    match state
        .db
        .debit(user_id, 0.0, "check", None, start_of_utc_day_ms())
    {
        Ok(Ok(balance)) if balance <= 0.0 => {
            let user = state.db.user(user_id).ok().flatten();
            if user.is_some_and(|u| u.allow_negative) {
                Ok(())
            } else {
                Err(DebitRefused::Insufficient {
                    balance_usd: balance,
                    needed_usd: 0.0,
                })
            }
        }
        Ok(Ok(_)) => Ok(()),
        Ok(Err(r)) => Err(r),
        Err(e) => {
            tracing::warn!(error = %e, "credits check");
            Ok(())
        }
    }
}

/// A project's daily cap below zero means "no project cap" (the column is `REAL NOT NULL`, so
/// the sentinel is a negative value; `PUT /admin/projects/{id}/spend-cap {"daily_cap_usd": null}`
/// and the owner's `PATCH /projects/{id} {"spend_cap_daily": null}` store it).
pub const PROJECT_CAP_UNLIMITED: f64 = -1.0;

/// Pure rule for the per-project daily cap (Jacob, 2026-10-26: "Doug agent" hit its $25 project
/// cap at 22:07 UTC while his *user* cap was unlimited). An owner without a user cap has no
/// project caps either; a project stored with the unlimited sentinel has none; otherwise the
/// usual `today >= cap`.
pub fn project_cap_allows(cap: f64, owner_unlimited: bool, today_usd: f64) -> bool {
    if owner_unlimited || cap < 0.0 {
        return true;
    }
    today_usd < cap
}

/// The owner of `project_id` has no daily cap (`users.cap_unlimited`).
pub fn owner_unlimited(state: &AppState, project_id: &str) -> bool {
    state
        .user_of(project_id)
        .and_then(|u| state.db.user(&u).ok().flatten())
        .is_some_and(|u| u.daily_cap_usd.is_none())
}

/// `Some(402 spend_cap)` when the project's own daily cap is spent; `None` when the call may go.
pub fn project_cap_refusal(state: &AppState, project_id: &str) -> Option<Response> {
    let p = state.db.project(project_id).ok().flatten()?;
    let today = state
        .db
        .usage_since(project_id, start_of_utc_day_ms())
        .map(|u| u.usd)
        .unwrap_or(0.0);
    if project_cap_allows(p.spend_cap_daily, owner_unlimited(state, project_id), today) {
        return None;
    }
    let _ = state.db.enqueue_push(
        project_id,
        "spend_cap",
        &json!({ "spend_today": today, "cap": p.spend_cap_daily }),
    );
    Some(
        (
            StatusCode::PAYMENT_REQUIRED,
            json!({ "error": { "type": "spend_cap", "message": format!("daily spend cap ${:.2} reached", p.spend_cap_daily) } }).to_string(),
        )
            .into_response(),
    )
}

/// Who pays for a call: the device's or box's user when the bearer has one, else the owner of
/// the project the call names. `None` only for the admin on an unknown project.
pub fn payer(state: &AppState, principal: &Principal, project_id: &str) -> Option<String> {
    principal
        .user_id()
        .map(str::to_owned)
        .or_else(|| state.user_of(project_id))
}

/// `check` for whoever pays for a call.
pub fn check_call(
    state: &AppState,
    principal: &Principal,
    project_id: &str,
) -> Result<(), DebitRefused> {
    match payer(state, principal, project_id) {
        Some(u) => check(state, &u),
        None => Ok(()),
    }
}

/// Take `usd` from the user after the fact (inference already happened, the box already ran).
/// The pre-check kept the balance positive; if the final cost overshoots what is left, the debit
/// is clamped at the balance so the ledger never goes negative without the admin override.
pub fn charge(state: &AppState, user_id: &str, usd: f64, kind: &str, reference: Option<&str>) {
    if !metered(state) || !usd.is_finite() || usd < MIN_DEBIT_USD {
        return;
    }
    ensure_account(state, user_id);
    let day = start_of_utc_day_ms();
    match state.db.debit(user_id, usd, kind, reference, day) {
        Ok(Ok(_)) => {}
        Ok(Err(DebitRefused::Insufficient { balance_usd, .. })) if balance_usd > 0.0 => {
            tracing::info!(user = %user_id, usd, balance_usd, "debit clamped at the remaining balance");
            let _ = state.db.debit(user_id, balance_usd, kind, reference, day);
        }
        Ok(Err(r)) => {
            tracing::info!(user = %user_id, usd, kind, %r, "debit refused");
        }
        Err(e) => tracing::warn!(error = %e, user = %user_id, "debit"),
    }
}

/// `charge` for whoever pays for a call.
pub fn charge_call(
    state: &AppState,
    principal: &Principal,
    project_id: &str,
    usd: f64,
    kind: &str,
    reference: Option<&str>,
) {
    if let Some(u) = payer(state, principal, project_id) {
        charge(state, &u, usd, kind, reference);
    }
}

/// Machine metering: once a minute each user box is charged for the time since its last tick —
/// the running rate while awake or waking, the stopped-disk rate otherwise (Daytona only; local
/// process/docker boxes cost nothing). A box whose user ran out of credits is stopped.
pub async fn spawn_meter(state: AppState) {
    if !metered(&state) {
        return;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(METER_PERIOD);
        loop {
            tick.tick().await;
            meter_once(&state).await;
        }
    });
}

async fn meter_once(state: &AppState) {
    let boxes = match state.db.list_boxes() {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "meter: list boxes");
            return;
        }
    };
    let now = now_ms();
    for sb in boxes {
        if sb.provider != "daytona" {
            continue;
        }
        let (bs, _) = state.box_state(&sb.user_id).await;
        let running = matches!(bs, BoxState::Awake | BoxState::Waking);
        let label = if running { "running" } else { "stopped" };
        let last = state.db.meter_tick(&sb.user_id).ok().flatten();
        if let Err(e) = state.db.set_meter_tick(&sb.user_id, now, label) {
            tracing::warn!(error = %e, "meter tick");
            continue;
        }
        let Some((last_at, last_state)) = last else {
            continue;
        };
        let hours = (now - last_at).max(0) as f64 / 3_600_000.0;
        // The elapsed interval is billed at the state the box was in when it started.
        let usd = if last_state == "running" {
            hours * state.cfg.machine_usd_per_hour
        } else {
            hours / 24.0 * state.cfg.machine_stopped_usd_per_day
        };
        if usd < MIN_DEBIT_USD {
            continue;
        }
        ensure_account(state, &sb.user_id);
        if let Err(e) = state
            .db
            .record_machine_cost(&sb.user_id, usd, sb.sandbox_id.as_deref())
        {
            tracing::warn!(error = %e, user = %sb.user_id, "machine cost");
        }
        // Only the kill switch stops a running machine (always-on): machine time never gates.
        if running {
            if let Err(r) = check_read_only(state) {
                tracing::warn!(user = %sb.user_id, %r, "stopping the box: relay is read-only");
                if let Err(e) = crate::boxes::stop_box(state, &sb.user_id, false).await {
                    tracing::warn!(error = %e, user = %sb.user_id, "meter stop");
                }
                state
                    .set_box_state(&sb.user_id, BoxState::Asleep, Some(&r.to_string()))
                    .await;
            }
        }
    }
    machine_alert(state);
}

/// Relay-wide machine cost today vs `ARBOS_GLOBAL_MACHINE_DAILY_CAP_USD`: a warning once per UTC
/// day when crossed (log-only; it never gates anything).
fn machine_alert(state: &AppState) {
    let day = start_of_utc_day_ms();
    let today = state.db.machine_cost_since_all(day).unwrap_or(0.0);
    if today < state.cfg.global_machine_daily_cap_usd {
        return;
    }
    let key = "machine_alert_day";
    let already = state
        .db
        .meta_get(key)
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        == Some(day);
    if !already {
        tracing::warn!(
            today,
            cap = state.cfg.global_machine_daily_cap_usd,
            "machine spend alert: today's always-on machine cost passed the alert threshold"
        );
        let _ = state.db.meta_set(key, &day.to_string());
    }
}

/// Expected always-on machine cost per day at the current box count.
pub fn machine_baseline_usd_per_day(state: &AppState) -> (usize, f64) {
    let boxes = state
        .db
        .list_boxes()
        .unwrap_or_default()
        .into_iter()
        .filter(|b| b.provider == "daytona")
        .count();
    (boxes, boxes as f64 * 24.0 * state.cfg.machine_usd_per_hour)
}

/// `GET /me`-style credits summary plus the gate result, for `/admin/users`.
pub fn summary(state: &AppState, user_id: &str) -> serde_json::Value {
    let credits = state
        .db
        .credits(user_id, start_of_utc_day_ms())
        .ok()
        .map(|c| serde_json::to_value(c).unwrap_or_default())
        .unwrap_or_default();
    let blocked = check(state, user_id).err().map(|r| r.to_string());
    json!({ "credits": credits, "blocked": blocked })
}

#[cfg(test)]
mod tests {
    //! The invariants the design names: tokens are bound to a device, one machine per account,
    //! the ledger never goes negative without the admin override, the kill switch blocks debits.

    use arbos_core::ids::ulid;

    use super::*;

    /// Jacob, 2026-10-26 22:07 UTC: an unlimited owner's agent was refused by its $25 project cap.
    #[test]
    fn project_cap_follows_the_owner_and_the_sentinel() {
        assert!(project_cap_allows(25.0, false, 24.99));
        assert!(!project_cap_allows(25.0, false, 25.0));
        assert!(!project_cap_allows(25.0, false, 25.24));
        assert!(
            project_cap_allows(25.0, true, 25.24),
            "an unlimited owner has no project cap"
        );
        assert!(
            project_cap_allows(PROJECT_CAP_UNLIMITED, false, 1.0e6),
            "sentinel = no cap"
        );
        assert!(project_cap_allows(-5.0, false, 1.0e6));
        assert!(
            !project_cap_allows(0.0, false, 0.0),
            "a zero cap still blocks (not the sentinel)"
        );
    }
    use crate::auth::{resolve_checked, Refused};
    use crate::connectors::crypto::RelayKey;
    use crate::db::Sandbox;
    use crate::db_accounts::GithubIdentity;
    use crate::signin::{issue_tokens, mint_access};

    use crate::state::test_support::test_state;

    fn sign_up(state: &AppState, github_id: i64, device_name: &str) -> (String, String) {
        let (user, created) = state
            .db
            .upsert_github_user(
                &GithubIdentity {
                    id: github_id,
                    login: format!("user{github_id}"),
                    avatar_url: None,
                    name: None,
                },
                state.cfg.default_daily_cap_usd,
            )
            .expect("user");
        if created {
            state
                .db
                .grant_credits(&user.id, state.cfg.default_grant_usd, "welcome", "relay")
                .expect("grant");
        }
        let device_id = ulid();
        state
            .db
            .insert_device(&device_id, device_name, &ulid(), &user.id, "github")
            .expect("device");
        (user.id, device_id)
    }

    #[test]
    fn access_tokens_are_bound_to_their_device() {
        let state = test_state(AuthMode::Github);
        let (user, dev_a) = sign_up(&state, 1, "a");
        let (_, dev_b) = sign_up(&state, 1, "b");
        let tokens = issue_tokens(&state, &user, &dev_a).expect("tokens");
        let access = tokens.access_token.clone();
        let refresh = tokens.refresh_token.clone();

        // The token resolves to the device it was minted for, not to a sibling device.
        match resolve_checked(&state, &access).expect("valid") {
            Principal::Device { id, user_id, .. } => {
                assert_eq!(id, dev_a);
                assert_eq!(user_id, user);
            }
            other => panic!("unexpected principal {other:?}"),
        }
        // A token forged for device B under another user is refused even with a valid signature
        // shape: the key differs.
        let forged = mint_access(&RelayKey::derive("other").subkey("jwt"), &user, &dev_b, 60);
        assert!(matches!(
            resolve_checked(&state, &forged),
            Err(Refused::Invalid)
        ));
        // A token whose claims name a user that does not own the device is refused.
        let mismatched = mint_access(&state.jwt_key, "gh_999", &dev_a, 60);
        assert!(matches!(
            resolve_checked(&state, &mismatched),
            Err(Refused::Invalid)
        ));
        // An expired token says so (clients refresh instead of signing in again).
        let expired = mint_access(&state.jwt_key, &user, &dev_a, -1);
        assert!(matches!(
            resolve_checked(&state, &expired),
            Err(Refused::Expired)
        ));
        // Revoking the device kills its access token before expiry and its refresh token.
        state.db.revoke_device_fully(&dev_a).expect("revoke");
        assert!(matches!(
            resolve_checked(&state, &access),
            Err(Refused::Invalid)
        ));
        let row = state
            .db
            .refresh_row(&arbos_core::auth::hash(&refresh))
            .unwrap()
            .unwrap();
        assert!(row.revoked_at.is_some());
        // The sibling device is untouched.
        let b = issue_tokens(&state, &user, &dev_b).expect("tokens b");
        assert!(resolve_checked(&state, &b.access_token).is_ok());
    }

    #[test]
    fn one_machine_per_account() {
        let state = test_state(AuthMode::Github);
        let (user, _) = sign_up(&state, 2, "a");
        let (same_user, _) = sign_up(&state, 2, "b");
        assert_eq!(user, same_user, "the same GitHub account is the same user");
        let make = |sandbox: &str| Sandbox {
            user_id: user.clone(),
            provider: "daytona".into(),
            sandbox_id: Some(sandbox.to_owned()),
            daytona_state: None,
            device_id: format!("box:{sandbox}"),
            box_token_hash: ulid(),
            mcp_token: ulid(),
            snapshot: None,
            last_started_at: None,
            last_stopped_at: None,
            last_seen_at: None,
            box_url: None,
            preview_token: None,
            local_pid: None,
            migrated_from: Vec::new(),
        };
        state.db.upsert_box(&make("sb-1")).expect("box 1");
        state
            .db
            .upsert_box(&make("sb-2"))
            .expect("box 2 replaces, never adds");
        let boxes: Vec<_> = state
            .db
            .list_boxes()
            .unwrap()
            .into_iter()
            .filter(|b| b.user_id == user)
            .collect();
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].sandbox_id.as_deref(), Some("sb-2"));
    }

    #[test]
    fn ledger_never_negative_without_admin_override() {
        let state = test_state(AuthMode::Github);
        let (user, _) = sign_up(&state, 3, "a");
        state
            .db
            .set_user_limits(&user, Some(None), None)
            .expect("unlimited daily cap");
        assert_eq!(state.db.credits(&user, 0).unwrap().balance_usd, 100.0);
        // A debit larger than the balance is refused; the balance is untouched.
        let refused = state.db.debit(&user, 150.0, "inference", None, 0).unwrap();
        assert!(matches!(refused, Err(DebitRefused::Insufficient { .. })));
        assert_eq!(state.db.credits(&user, 0).unwrap().balance_usd, 100.0);
        // `charge` clamps an overshoot at what is left, so the ledger ends at exactly zero.
        charge(&state, &user, 150.0, "inference", None);
        assert_eq!(state.db.credits(&user, 0).unwrap().balance_usd, 0.0);
        assert!(matches!(
            check(&state, &user),
            Err(DebitRefused::Insufficient { .. })
        ));
        // The admin override lets the user go negative.
        state
            .db
            .set_user_limits(&user, None, Some(true))
            .expect("allow negative");
        assert!(check(&state, &user).is_ok());
        charge(&state, &user, 2.5, "inference", None);
        assert_eq!(state.db.credits(&user, 0).unwrap().balance_usd, -2.5);
        // Machine time is its own bucket: never part of the balance or the caps.
        state
            .db
            .record_machine_cost(&user, 7.97, Some("sb"))
            .unwrap();
        let c = state.db.credits(&user, 0).unwrap();
        assert_eq!(c.balance_usd, -2.5);
        assert_eq!(c.machine_today_usd, 7.97);
        // Daily cap: a capped user with money left is still refused once today's debits reach it.
        let (capped, _) = sign_up(&state, 4, "a");
        state
            .db
            .set_user_limits(&capped, Some(Some(1.0)), None)
            .expect("cap");
        charge(&state, &capped, 1.0, "inference", None);
        assert!(matches!(
            check(&state, &capped),
            Err(DebitRefused::DailyCap { .. })
        ));
        assert_eq!(state.db.credits(&capped, 0).unwrap().balance_usd, 99.0);
    }

    #[test]
    fn kill_switch_blocks_debits() {
        let state = test_state(AuthMode::Github);
        let (user, _) = sign_up(&state, 5, "a");
        assert!(check(&state, &user).is_ok());
        state
            .db
            .set_kill_switch(&KillSwitch {
                on: true,
                reason: Some("test".into()),
                by: Some("admin".into()),
                at: now_ms(),
                auto: false,
            })
            .expect("set");
        assert!(matches!(
            check(&state, &user),
            Err(DebitRefused::ReadOnly { .. })
        ));
        state
            .db
            .set_kill_switch(&KillSwitch::default())
            .expect("clear");
        assert!(check(&state, &user).is_ok());
        // Reaching the global daily cap trips it automatically.
        let (whale, _) = sign_up(&state, 6, "a");
        state
            .db
            .set_user_limits(&whale, Some(None), Some(true))
            .expect("unlimited whale");
        charge(
            &state,
            &whale,
            state.cfg.global_daily_cap_usd,
            "inference",
            None,
        );
        assert!(matches!(
            check(&state, &user),
            Err(DebitRefused::ReadOnly { .. })
        ));
        assert!(kill_switch(&state).auto);
        // Self-hosters (pair-code mode) are never gated.
        let open = test_state(AuthMode::Paircode);
        assert!(check(&open, "u_default").is_ok());
    }

    #[test]
    fn legacy_device_token_user_gets_the_default_account() {
        let state = test_state(AuthMode::Github);
        state.db.set_kill_switch(&KillSwitch::default()).unwrap();
        assert!(state.db.user("u_mac").unwrap().is_none());
        assert!(
            check(&state, "u_mac").is_ok(),
            "not blocked for having no row"
        );
        let c = state.db.credits("u_mac", 0).unwrap();
        assert_eq!(c.balance_usd, state.cfg.default_grant_usd);
        assert_eq!(c.daily_cap_usd, Some(state.cfg.default_daily_cap_usd));
        charge(&state, "u_mac", 1.0, "inference", None);
        assert_eq!(
            state.db.credits("u_mac", 0).unwrap().balance_usd,
            state.cfg.default_grant_usd - 1.0
        );
        // Idempotent: a second contact grants nothing more.
        ensure_account(&state, "u_mac");
        assert_eq!(
            state.db.credits("u_mac", 0).unwrap().granted_usd,
            state.cfg.default_grant_usd
        );
    }
}
