//! Routines (design §10, F3): prompts that run on a cron schedule, once at a set time, on a
//! webhook, or on a relay event while the user is away. The scheduler lives in the relay: every
//! tick it picks due routines, wakes the box (Flow 5), and queues a `routine_run` push; the box
//! runs the prompt as a user turn with `sender = routine:<id>` and reports the run back here.

use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{TimeZone, Utc};
use chrono_tz::Tz;
use croner::Cron;
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::api::{FireRoutineResponseDto, RoutineDto, RoutineRunDto};
use arbos_core::ids::{now_ms, ulid};
use arbos_core::wire::BoxState;

use crate::api::resolve_project_id;
use crate::auth::Principal;
use crate::boxes;
use crate::db::{Routine, RoutineRun};
use crate::state::AppState;

type ApiResult = Result<Response, (StatusCode, String)>;

pub const SCHEDULER_TICK: Duration = Duration::from_secs(15);
/// A run the box neither finished nor failed within this window is marked failed by the relay.
const RUN_STALE_MS: i64 = 30 * 60 * 1000;
const RUN_HISTORY_LIMIT: i64 = 20;
pub const EVENTS: [&str; 3] = ["cross_project_message", "connector_event", "box_awake"];

fn internal(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn bad(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, e.to_string())
}

/// Reject a one-shot whose fire time is not strictly in the future. Used by create/patch
/// only — the scheduler must still be able to parse a stored once trigger after it is due.
fn require_once_in_future(trigger: &Trigger) -> Result<(), String> {
    match trigger {
        Trigger::Once { at_ms } if *at_ms <= now_ms() => {
            Err("once at must be in the future".into())
        }
        _ => Ok(()),
    }
}

/// A validated trigger. The JSON form in the table is what clients send.
#[derive(Debug, Clone)]
pub enum Trigger {
    Cron {
        expr: String,
        tz: Tz,
    },
    /// Fire once at `at_ms` (unix ms), then stop. Accepts `at` / `at_ms`, or `in_seconds` (converted at parse).
    Once {
        at_ms: i64,
    },
    Webhook {
        secret: String,
    },
    Event {
        event: String,
    },
}

impl Trigger {
    pub fn parse(v: &Value) -> Result<Self, String> {
        match v.get("kind").and_then(Value::as_str) {
            Some("cron") => {
                let expr = v
                    .get("expr")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or("cron trigger needs expr")?
                    .to_owned();
                Cron::from_str(&expr).map_err(|e| format!("bad cron expression {expr:?}: {e}"))?;
                let tz_name = v.get("tz").and_then(Value::as_str).unwrap_or("UTC");
                let tz: Tz = tz_name
                    .parse()
                    .map_err(|_| format!("unknown time zone {tz_name:?}"))?;
                Ok(Trigger::Cron { expr, tz })
            }
            Some("once") => {
                // Past `at` values are allowed here: the scheduler re-parses the stored
                // trigger after `next_run_at` is due. Create/patch reject past times via
                // `require_once_in_future` so clients cannot schedule into the past.
                let now = now_ms();
                let at_ms = if let Some(at) = v
                    .get("at")
                    .or_else(|| v.get("at_ms"))
                    .and_then(Value::as_i64)
                {
                    at
                } else if let Some(secs) = v.get("in_seconds").and_then(Value::as_i64) {
                    if !(1..=30 * 24 * 60 * 60).contains(&secs) {
                        return Err("once in_seconds must be between 1 and 2592000".into());
                    }
                    now.saturating_add(secs.saturating_mul(1000))
                } else {
                    return Err("once trigger needs at or in_seconds".into());
                };
                Ok(Trigger::Once { at_ms })
            }
            Some("webhook") => {
                let secret = v
                    .get("secret")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| ulid().to_lowercase());
                Ok(Trigger::Webhook { secret })
            }
            Some("event") => {
                let event = v
                    .get("event")
                    .and_then(Value::as_str)
                    .ok_or("event trigger needs event")?;
                if !EVENTS.contains(&event) {
                    return Err(format!("event must be one of {}", EVENTS.join("|")));
                }
                Ok(Trigger::Event {
                    event: event.to_owned(),
                })
            }
            Some(other) => Err(format!(
                "trigger kind must be cron|once|webhook|event, got {other:?}"
            )),
            None => Err("trigger needs kind".into()),
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            Trigger::Cron { expr, tz } => json!({ "kind": "cron", "expr": expr, "tz": tz.name() }),
            Trigger::Once { at_ms } => json!({ "kind": "once", "at": at_ms }),
            Trigger::Webhook { secret } => json!({ "kind": "webhook", "secret": secret }),
            Trigger::Event { event } => json!({ "kind": "event", "event": event }),
        }
    }
}

/// The next fire time at or after `now_ms` for time-based triggers, or `None` otherwise.
/// Cron uses the next occurrence *strictly after* `now_ms`; once uses `at_ms` while it is still due.
pub fn next_run_ms(trigger: &Trigger, now_ms: i64) -> Option<i64> {
    match trigger {
        Trigger::Cron { expr, tz } => {
            let cron = Cron::from_str(expr).ok()?;
            let now = Utc.timestamp_millis_opt(now_ms).single()?.with_timezone(tz);
            cron.find_next_occurrence(&now, false)
                .ok()
                .map(|t| t.timestamp_millis())
        }
        Trigger::Once { at_ms } => (*at_ms > now_ms).then_some(*at_ms),
        Trigger::Webhook { .. } | Trigger::Event { .. } => None,
    }
}

/// One scheduler step for one routine, pure: `(fires now, next_run_at afterwards)`. A routine
/// fires when its stored `next_run_at` has passed; the new `next_run_at` is computed from the
/// clock, so a second step inside the same minute never fires again.
pub fn schedule_step(
    trigger: &Trigger,
    next_run_at: Option<i64>,
    now_ms: i64,
) -> (bool, Option<i64>) {
    match next_run_at {
        Some(due) if due <= now_ms => (true, next_run_ms(trigger, now_ms)),
        Some(due) => (false, Some(due)),
        None => (false, next_run_ms(trigger, now_ms)),
    }
}

/// Clients (non-box sockets) currently attached to any room of the project.
async fn clients_connected(state: &AppState, project_id: &str) -> bool {
    let prefix = format!("{project_id}/");
    let sockets: usize = state
        .rooms
        .read()
        .await
        .iter()
        .filter(|(name, _)| name.starts_with(&prefix))
        .map(|(_, room)| room.tx.receiver_count())
        .sum();
    let box_sockets = state
        .box_conns
        .lock()
        .await
        .get(project_id)
        .map(|c| c.0 as usize)
        .unwrap_or(0);
    sockets > box_sockets
}

/// Queue one run of a routine: a `routine_runs` row, a `routine_run` push, and a wake.
pub async fn fire(
    state: &AppState,
    rt: &Routine,
    cause: &str,
    context: Option<Value>,
) -> anyhow::Result<RoutineRun> {
    let run = RoutineRun {
        id: ulid(),
        routine_id: rt.id.clone(),
        started_at: now_ms(),
        finished_at: None,
        status: "queued".into(),
        message_id: None,
        note: Some(format!("cause: {cause}")),
    };
    state.db.insert_routine_run(&run)?;
    let mut prompt = rt.prompt.clone();
    if let Some(ctx) =
        context.filter(|c| !c.is_null() && c.as_object().is_none_or(|o| !o.is_empty()))
    {
        prompt.push_str("\n\nTrigger context:\n```json\n");
        prompt.push_str(&serde_json::to_string_pretty(&ctx).unwrap_or_default());
        prompt.push_str("\n```");
    }
    let payload = json!({
        "routineId": rt.id, "runId": run.id, "chatId": rt.chat_id, "prompt": prompt,
        "routineName": rt.name, "cause": cause,
        "clientsConnected": clients_connected(state, &rt.project_id).await,
    });
    state
        .db
        .enqueue_push(&rt.project_id, "routine_run", &payload)?;
    tracing::info!(routine = %rt.id, run = %run.id, project = %rt.project_id, cause, "routine fired");
    // The box belongs to the project's owner (D32), not to the project: waking by project id
    // created phantom boxes for "users" that do not exist (lane-c / lane-d, 2026-10-22).
    boxes::ensure_awake_project(state, &rt.project_id, "routine").await;
    Ok(run)
}

/// Fire every enabled `event` routine of a project for `event` (`cross_project_message`,
/// `connector_event`, `box_awake`). Other relay modules call this; it never fails loudly.
pub async fn fire_event(state: &AppState, project_id: &str, event: &str, context: Value) {
    let routines = match state.db.event_routines(project_id, event) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "event routines");
            return;
        }
    };
    for rt in routines {
        let _ = state.db.mark_routine_fired(&rt.id, now_ms(), None);
        if let Err(e) = fire(state, &rt, &format!("event:{event}"), Some(context.clone())).await {
            tracing::warn!(routine = %rt.id, error = %e, "event routine fire failed");
        }
    }
}

pub async fn spawn_scheduler(state: AppState) {
    tokio::spawn(async move {
        let mut awake_prev: HashMap<String, bool> = HashMap::new();
        loop {
            tokio::time::sleep(SCHEDULER_TICK).await;
            let now = now_ms();
            let due = match state.db.due_routines(now) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(error = %e, "due routines");
                    continue;
                }
            };
            for rt in due {
                let trigger = match Trigger::parse(&rt.trigger) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::warn!(routine = %rt.id, error = %e, "unparseable trigger; disabling");
                        let _ = state.db.update_routine(
                            &rt.id,
                            None,
                            None,
                            None,
                            None,
                            Some(false),
                            None,
                        );
                        continue;
                    }
                };
                let (fires, next) = schedule_step(&trigger, rt.next_run_at, now);
                // Advance before firing so a slow wake cannot make the next tick fire it again.
                let _ = state.db.mark_routine_fired(&rt.id, now, next);
                // One-shot routines disable after they fire so they leave the due set cleanly.
                if fires && matches!(trigger, Trigger::Once { .. }) {
                    let _ =
                        state
                            .db
                            .update_routine(&rt.id, None, None, None, None, Some(false), None);
                }
                if fires {
                    let cause = match &trigger {
                        Trigger::Cron { .. } => "cron",
                        Trigger::Once { .. } => "once",
                        Trigger::Webhook { .. } | Trigger::Event { .. } => "schedule",
                    };
                    if let Err(e) = fire(&state, &rt, cause, None).await {
                        tracing::warn!(routine = %rt.id, error = %e, "routine fire failed");
                    }
                }
            }
            if let Ok(stale) = state.db.stale_routine_runs(RUN_STALE_MS) {
                for run in stale {
                    let _ = state.db.update_routine_run(
                        &run.id,
                        "failed",
                        None,
                        Some("the box did not finish this run within 30 min"),
                    );
                }
            }
            if let Ok(projects) = state.db.list_projects() {
                for p in projects {
                    let (st, _) = state.box_state(&p.id).await;
                    let awake = st == BoxState::Awake;
                    if awake && awake_prev.get(&p.id) == Some(&false) {
                        fire_event(&state, &p.id, "box_awake", json!({ "projectId": p.id })).await;
                    }
                    awake_prev.insert(p.id, awake);
                }
            }
        }
    });
}

// ---- endpoints --------------------------------------------------------------------------------

pub(crate) fn routine_dto(state: &AppState, rt: &Routine) -> RoutineDto {
    let fire_url = rt
        .trigger
        .get("secret")
        .and_then(Value::as_str)
        .filter(|_| rt.trigger.get("kind").and_then(Value::as_str) == Some("webhook"))
        .map(|secret| {
            format!(
                "{}/routines/{}/fire?secret={secret}",
                state.cfg.public_url, rt.id
            )
        });
    RoutineDto {
        id: rt.id.clone(),
        project_id: rt.project_id.clone(),
        chat_id: rt.chat_id.clone(),
        name: rt.name.clone(),
        prompt: rt.prompt.clone(),
        trigger: rt.trigger.clone(),
        enabled: rt.enabled,
        created_at: rt.created_at,
        last_run_at: rt.last_run_at,
        next_run_at: rt.next_run_at,
        fire_url,
    }
}

fn routine_json(state: &AppState, rt: &Routine) -> Value {
    serde_json::to_value(routine_dto(state, rt)).expect("RoutineDto")
}

pub(crate) fn routine_run_dto(run: &RoutineRun) -> RoutineRunDto {
    RoutineRunDto {
        id: run.id.clone(),
        routine_id: run.routine_id.clone(),
        started_at: run.started_at,
        finished_at: run.finished_at,
        status: run.status.clone(),
        message_id: run.message_id.clone(),
        note: run.note.clone(),
    }
}

fn load_routine(
    state: &AppState,
    project_id: &str,
    rid: &str,
) -> Result<Routine, (StatusCode, String)> {
    match state.db.routine(rid).map_err(internal)? {
        Some(r) if r.project_id == project_id => Ok(r),
        Some(_) | None => Err((StatusCode::NOT_FOUND, "no such routine".into())),
    }
}

pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    let project_id = resolve_project_id(&state, &principal, &id)?;
    let out: Vec<RoutineDto> = state
        .db
        .list_routines(&project_id)
        .map_err(internal)?
        .iter()
        .map(|r| routine_dto(&state, r))
        .collect();
    Ok(Json(out).into_response())
}

#[derive(Deserialize)]
pub struct CreateRoutineBody {
    pub name: String,
    pub prompt: String,
    #[serde(default, alias = "chatId")]
    pub chat_id: Option<String>,
    pub trigger: Value,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<CreateRoutineBody>,
) -> ApiResult {
    let project_id = resolve_project_id(&state, &principal, &id)?;
    let name = b.name.trim().to_owned();
    if name.is_empty() {
        return Err(bad("name required"));
    }
    if b.prompt.trim().is_empty() {
        return Err(bad("prompt required"));
    }
    let trigger = Trigger::parse(&b.trigger).map_err(bad)?;
    require_once_in_future(&trigger).map_err(bad)?;
    let chats = state.db.list_chats(&project_id).map_err(internal)?;
    let chat_id = match b.chat_id.filter(|c| !c.is_empty()) {
        Some(c) => {
            if !chats.iter().any(|x| x.id == c) {
                return Err(bad(format!("no chat {c} in this project")));
            }
            c
        }
        None => chats
            .iter()
            .find(|c| c.is_pinned)
            .or(chats.first())
            .map(|c| c.id.clone())
            .ok_or_else(|| bad("project has no chat"))?,
    };
    let now = now_ms();
    let rt = Routine {
        id: ulid(),
        project_id,
        chat_id,
        name,
        prompt: b.prompt.trim().to_owned(),
        trigger: trigger.to_json(),
        enabled: b.enabled,
        created_at: now,
        last_run_at: None,
        next_run_at: b.enabled.then(|| next_run_ms(&trigger, now)).flatten(),
    };
    state.db.insert_routine(&rt).map_err(internal)?;
    Ok((StatusCode::CREATED, Json(routine_json(&state, &rt))).into_response())
}

#[derive(Deserialize, Default)]
pub struct PatchRoutineBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default, alias = "chatId")]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub trigger: Option<Value>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// `PATCH …/routines/{rid}`: pause/resume/edit. `next_run_at` is recomputed from now.
pub async fn patch(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, rid)): Path<(String, String)>,
    Json(b): Json<PatchRoutineBody>,
) -> ApiResult {
    let project_id = resolve_project_id(&state, &principal, &id)?;
    let rt = load_routine(&state, &project_id, &rid)?;
    let trigger_json = b.trigger.clone().unwrap_or_else(|| rt.trigger.clone());
    let trigger = Trigger::parse(&trigger_json).map_err(bad)?;
    // Only a newly supplied trigger is checked for "in the future"; re-enabling or
    // renaming must not fail because a still-stored once `at` has already passed.
    if b.trigger.is_some() {
        require_once_in_future(&trigger).map_err(bad)?;
    }
    let enabled = b.enabled.unwrap_or(rt.enabled);
    if let Some(c) = &b.chat_id {
        if !state.db.chat_exists(&project_id, c).map_err(internal)? {
            return Err(bad(format!("no chat {c} in this project")));
        }
    }
    let next = if enabled {
        next_run_ms(&trigger, now_ms())
    } else {
        None
    };
    let new_trigger = b.trigger.is_some().then(|| trigger.to_json());
    state
        .db
        .update_routine(
            &rt.id,
            b.name.as_deref().map(str::trim),
            b.prompt.as_deref().map(str::trim),
            b.chat_id.as_deref(),
            new_trigger.as_ref(),
            b.enabled,
            next,
        )
        .map_err(internal)?;
    let rt = load_routine(&state, &project_id, &rid)?;
    Ok(Json(routine_json(&state, &rt)).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, rid)): Path<(String, String)>,
) -> ApiResult {
    let project_id = resolve_project_id(&state, &principal, &id)?;
    load_routine(&state, &project_id, &rid)?;
    state.db.delete_routine(&rid).map_err(internal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST …/routines/{rid}/run`: run now, regardless of schedule or paused state.
pub async fn run_now(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, rid)): Path<(String, String)>,
) -> ApiResult {
    let project_id = resolve_project_id(&state, &principal, &id)?;
    let rt = load_routine(&state, &project_id, &rid)?;
    let _ = state
        .db
        .mark_routine_fired(&rt.id, now_ms(), rt.next_run_at);
    let run = fire(&state, &rt, "manual", None).await.map_err(internal)?;
    Ok((StatusCode::ACCEPTED, Json(routine_run_dto(&run))).into_response())
}

#[derive(Deserialize)]
pub struct RunsQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

pub async fn runs(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, rid)): Path<(String, String)>,
    Query(q): Query<RunsQuery>,
) -> ApiResult {
    let project_id = resolve_project_id(&state, &principal, &id)?;
    load_routine(&state, &project_id, &rid)?;
    let limit = q.limit.unwrap_or(RUN_HISTORY_LIMIT).clamp(1, 200);
    let runs: Vec<RoutineRunDto> = state
        .db
        .routine_runs(&rid, limit)
        .map_err(internal)?
        .iter()
        .map(routine_run_dto)
        .collect();
    Ok(Json(runs).into_response())
}

#[derive(Deserialize)]
pub struct RunReportBody {
    pub status: String,
    #[serde(default, alias = "messageId")]
    pub message_id: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// `POST …/routines/{rid}/runs/{run}`: the box reports `running`, then `done|failed|skipped`.
pub async fn report_run(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, rid, run_id)): Path<(String, String, String)>,
    Json(b): Json<RunReportBody>,
) -> ApiResult {
    let project_id = resolve_project_id(&state, &principal, &id)?;
    load_routine(&state, &project_id, &rid)?;
    if !matches!(
        b.status.as_str(),
        "queued" | "running" | "done" | "failed" | "skipped"
    ) {
        return Err(bad("status must be queued|running|done|failed|skipped"));
    }
    match state.db.routine_run(&run_id).map_err(internal)? {
        Some(r) if r.routine_id == rid => {}
        Some(_) | None => return Err((StatusCode::NOT_FOUND, "no such run".into())),
    }
    state
        .db
        .update_routine_run(
            &run_id,
            &b.status,
            b.message_id.as_deref(),
            b.note.as_deref(),
        )
        .map_err(internal)?;
    let run = state
        .db
        .routine_run(&run_id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such run".to_owned()))?;
    Ok(Json(routine_run_dto(&run)).into_response())
}

#[derive(Deserialize)]
pub struct FireQuery {
    #[serde(default)]
    pub secret: Option<String>,
}

/// `POST /routines/{rid}/fire?secret=`: webhook trigger, no device token. The JSON body (if any)
/// is handed to the agent as trigger context.
pub async fn webhook_fire(
    State(state): State<AppState>,
    Path(rid): Path<String>,
    Query(q): Query<FireQuery>,
    body: axum::body::Bytes,
) -> ApiResult {
    let rt = state
        .db
        .routine(&rid)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such routine".to_owned()))?;
    let Trigger::Webhook { secret } = Trigger::parse(&rt.trigger).map_err(bad)? else {
        return Err((StatusCode::NOT_FOUND, "not a webhook routine".into()));
    };
    if q.secret.as_deref() != Some(secret.as_str()) {
        return Err((StatusCode::FORBIDDEN, "bad secret".into()));
    }
    if !rt.enabled {
        return Err((StatusCode::CONFLICT, "routine is paused".into()));
    }
    let context = if body.is_empty() {
        None
    } else {
        Some(
            serde_json::from_slice::<Value>(&body)
                .unwrap_or_else(|_| json!({ "body": String::from_utf8_lossy(&body) })),
        )
    };
    let _ = state.db.mark_routine_fired(&rt.id, now_ms(), None);
    let run = fire(&state, &rt, "webhook", context)
        .await
        .map_err(internal)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(FireRoutineResponseDto { run_id: run.id }),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(s)
            .expect("rfc3339")
            .timestamp_millis()
    }

    /// Invariant (F3): next-run computation is deterministic for a fixed clock, and a routine
    /// never fires twice inside one minute.
    #[test]
    fn next_run_is_deterministic_and_never_double_fires_within_a_minute() {
        let every_minute = Trigger::parse(&json!({ "kind": "cron", "expr": "* * * * *" })).unwrap();
        let weekdays_9 =
            Trigger::parse(&json!({ "kind": "cron", "expr": "0 9 * * 1-5", "tz": "UTC" })).unwrap();
        let berlin_9 =
            Trigger::parse(&json!({ "kind": "cron", "expr": "0 9 * * *", "tz": "Europe/Berlin" }))
                .unwrap();

        // Fixed clock: Thursday 2026-10-15 10:00:05 UTC.
        let now = at("2026-10-15T10:00:05Z");
        assert_eq!(
            next_run_ms(&every_minute, now),
            Some(at("2026-10-15T10:01:00Z"))
        );
        assert_eq!(
            next_run_ms(&weekdays_9, now),
            Some(at("2026-10-16T09:00:00Z"))
        );
        // Friday 10:00 → Monday 09:00, skipping the weekend.
        let friday = at("2026-10-16T10:00:00Z");
        assert_eq!(
            next_run_ms(&weekdays_9, friday),
            Some(at("2026-10-19T09:00:00Z"))
        );
        // 09:00 Berlin (CEST, UTC+2) is 07:00 UTC.
        assert_eq!(
            next_run_ms(&berlin_9, now),
            Some(at("2026-10-16T07:00:00Z"))
        );
        // Webhooks and events have no next run.
        let hook = Trigger::parse(&json!({ "kind": "webhook" })).unwrap();
        assert_eq!(next_run_ms(&hook, now), None);

        // One-shot: due at `at`, then gone. Parse must accept a past `at` so the scheduler
        // can re-read the stored trigger after next_run_at is due (create/patch still reject).
        let once_at = at("2026-10-15T10:00:20Z");
        let once = Trigger::parse(&json!({ "kind": "once", "at": once_at })).unwrap();
        assert_eq!(next_run_ms(&once, now), Some(once_at));
        let (fired, next) = schedule_step(&once, Some(once_at), at("2026-10-15T10:00:25Z"));
        assert!(fired);
        assert_eq!(next, None);
        let past = Trigger::parse(&json!({ "kind": "once", "at": now_ms() - 1 })).unwrap();
        assert!(matches!(past, Trigger::Once { .. }));
        assert!(require_once_in_future(&past).is_err());
        let in_secs = Trigger::parse(&json!({ "kind": "once", "in_seconds": 30 })).unwrap();
        assert!(matches!(in_secs, Trigger::Once { at_ms } if at_ms > now_ms()));
        assert!(require_once_in_future(&in_secs).is_ok());

        // Scheduler steps inside one minute: fires once at the due tick, then stays quiet until
        // the next minute boundary even when ticks keep coming.
        let due = at("2026-10-15T10:00:00Z");
        let (fired, next) = schedule_step(&every_minute, Some(due), at("2026-10-15T10:00:05Z"));
        assert!(fired);
        assert_eq!(next, Some(at("2026-10-15T10:01:00Z")));
        for tick in ["10:00:20Z", "10:00:35Z", "10:00:50Z", "10:00:59Z"] {
            let (fired_again, next_again) =
                schedule_step(&every_minute, next, at(&format!("2026-10-15T{tick}")));
            assert!(!fired_again, "double fire at {tick}");
            assert_eq!(next_again, next);
        }
        let (fired, next2) = schedule_step(&every_minute, next, at("2026-10-15T10:01:05Z"));
        assert!(fired);
        assert_eq!(next2, Some(at("2026-10-15T10:02:00Z")));

        // A missed tick (relay down) fires exactly once and resumes from the clock, not from the
        // stale due time.
        let (fired, next) = schedule_step(&every_minute, Some(due), at("2026-10-15T10:07:30Z"));
        assert!(fired);
        assert_eq!(next, Some(at("2026-10-15T10:08:00Z")));
    }

    #[test]
    fn trigger_validation() {
        assert!(Trigger::parse(&json!({ "kind": "cron", "expr": "61 * * * *" })).is_err());
        assert!(Trigger::parse(
            &json!({ "kind": "cron", "expr": "0 9 * * *", "tz": "Mars/Olympus" })
        )
        .is_err());
        assert!(Trigger::parse(&json!({ "kind": "event", "event": "full_moon" })).is_err());
        assert!(Trigger::parse(&json!({ "kind": "event", "event": "box_awake" })).is_ok());
        let hook = Trigger::parse(&json!({ "kind": "webhook" })).unwrap();
        assert!(matches!(hook, Trigger::Webhook { ref secret } if !secret.is_empty()));
    }

    const FIXED_AT: i64 = 1_700_000_000_000;
    /// Obviously fake; never a real webhook secret.
    const FAKE_WEBHOOK_SECRET: &str = "fixture-webhook-secret-not-real";

    fn fixtures_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/api")
    }

    fn load_fixture(name: &str) -> Value {
        let path = fixtures_dir().join(name);
        let raw =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        serde_json::from_str(&raw).expect("fixture json")
    }

    fn assert_builder(name: &str, got: Value) {
        let path = fixtures_dir().join(name);
        if !path.exists() || std::env::var_os("UPDATE_API_FIXTURES").is_some() {
            let pretty = serde_json::to_string_pretty(&got).expect("pretty");
            std::fs::write(&path, format!("{pretty}\n")).expect("write fixture");
        }
        let want = load_fixture(name);
        assert_eq!(got, want, "fixture {name}");
    }

    #[test]
    fn routine_wire_fixtures() {
        use crate::state::test_support::test_state;
        use crate::state::AuthMode;

        let state = test_state(AuthMode::Github);
        let schedule = Routine {
            id: "routine_schedule_fixture".into(),
            project_id: "proj_fixture".into(),
            chat_id: "chat_primary".into(),
            name: "Weekday check-in".into(),
            prompt: "Review open goals.".into(),
            trigger: json!({
                "kind": "cron",
                "expr": "0 9 * * 1-5",
                "tz": "UTC"
            }),
            enabled: true,
            created_at: FIXED_AT,
            last_run_at: Some(FIXED_AT - 86_400_000),
            next_run_at: Some(FIXED_AT + 86_400_000),
        };
        assert_builder("routine_schedule.json", routine_json(&state, &schedule));

        let webhook = Routine {
            id: "routine_webhook_fixture".into(),
            project_id: "proj_fixture".into(),
            chat_id: "chat_primary".into(),
            name: "Deploy hook".into(),
            prompt: "A deploy finished; summarize.".into(),
            trigger: json!({
                "kind": "webhook",
                "secret": FAKE_WEBHOOK_SECRET
            }),
            enabled: true,
            created_at: FIXED_AT,
            last_run_at: None,
            next_run_at: None,
        };
        assert_builder("routine_webhook.json", routine_json(&state, &webhook));

        let run = RoutineRun {
            id: "run_fixture".into(),
            routine_id: schedule.id.clone(),
            started_at: FIXED_AT,
            finished_at: Some(FIXED_AT + 5_000),
            status: "done".into(),
            message_id: Some("msg_fixture".into()),
            note: None,
        };
        assert_builder(
            "routine_run.json",
            serde_json::to_value(routine_run_dto(&run)).expect("RoutineRunDto"),
        );
    }
}
