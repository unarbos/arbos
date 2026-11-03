//! Goals: long-running objectives the agent tracks across days (feedback #1091).
//! Each goal stores a title, outcome, plan steps, status, progress log, check-in cadence
//! and autonomy. Active goals with a cron schedule get a linked routine that wakes the
//! agent to move the goal forward.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::api::{GoalDto, GoalsListDto};
use arbos_core::ids::{now_ms, ulid};

use crate::api::resolve_project_id;
use crate::auth::Principal;
use crate::db::{Goal, Routine};
use crate::routines::{next_run_ms, Trigger};
use crate::state::AppState;

type ApiResult = Result<Response, (StatusCode, String)>;

fn internal(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn bad(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, e.to_string())
}

const STATUSES: [&str; 5] = ["active", "blocked", "waiting", "paused", "done"];
const AUTONOMIES: [&str; 3] = ["ask", "suggest", "act"];

fn load_goal(state: &AppState, project_id: &str, gid: &str) -> Result<Goal, (StatusCode, String)> {
    match state.db.goal(gid).map_err(internal)? {
        Some(g) if g.project_id == project_id => Ok(g),
        Some(_) | None => Err((StatusCode::NOT_FOUND, "no such goal".into())),
    }
}

fn normalize_status(s: &str) -> Result<String, (StatusCode, String)> {
    let t = s.trim().to_ascii_lowercase();
    // Accept "waiting on me" from clients as `waiting`.
    let key = if t == "waiting on me" || t == "waiting_on_me" {
        "waiting"
    } else {
        t.as_str()
    };
    if STATUSES.contains(&key) {
        Ok(key.to_owned())
    } else {
        Err(bad(format!("status must be one of {}", STATUSES.join("|"))))
    }
}

fn normalize_autonomy(s: &str) -> Result<String, (StatusCode, String)> {
    let t = s.trim().to_ascii_lowercase();
    if AUTONOMIES.contains(&t.as_str()) {
        Ok(t)
    } else {
        Err(bad(format!(
            "autonomy must be one of {}",
            AUTONOMIES.join("|")
        )))
    }
}

fn as_array(v: Option<&Value>) -> Value {
    match v {
        Some(Value::Array(a)) => Value::Array(a.clone()),
        Some(other) if !other.is_null() => json!([other]),
        _ => json!([]),
    }
}

pub(crate) fn goal_dto(g: &Goal, project_name: Option<&str>) -> GoalDto {
    GoalDto {
        id: g.id.clone(),
        project_id: g.project_id.clone(),
        chat_id: g.chat_id.clone(),
        title: g.title.clone(),
        outcome: g.outcome.clone(),
        plan: g.plan.clone(),
        status: g.status.clone(),
        progress_log: g.progress_log.clone(),
        next_step: g.next_step.clone(),
        check_in_cron: g.check_in_cron.clone(),
        check_in_tz: g.check_in_tz.clone(),
        routine_id: g.routine_id.clone(),
        autonomy: g.autonomy.clone(),
        linked_chat_ids: g.linked_chat_ids.clone(),
        linked_subagent_ids: g.linked_subagent_ids.clone(),
        waiting_question: g.waiting_question.clone(),
        created_at: g.created_at,
        updated_at: g.updated_at,
        last_moved_at: g.last_moved_at,
        project_name: project_name.map(str::to_owned),
    }
}

fn goal_json(g: &Goal, project_name: Option<&str>) -> Value {
    serde_json::to_value(goal_dto(g, project_name)).expect("GoalDto")
}

fn check_in_prompt(g: &Goal) -> String {
    format!(
        "Goal check-in for goal_id={} \"{}\".\n\
Desired outcome: {}\n\
Status: {}\n\
Next step: {}\n\
Autonomy: {} (ask = ask before acting; suggest = propose then wait; act = do the next step).\n\
Plan (JSON): {}\n\
Recent log (JSON): {}\n\
If waiting_question is set and still unanswered, do not nag — wait.\n\
waiting_question: {}\n\n\
Review progress. Do the next step or wake the right subagent. Update the goal via goals_update \
(status, next_step, plan) and append a dated progress note via goals_log. If blocked or you need \
a decision, ask the user ONE clear question once and set waiting_question; do not repeat nags.",
        g.id,
        g.title,
        if g.outcome.is_empty() {
            "(none set)"
        } else {
            &g.outcome
        },
        g.status,
        if g.next_step.is_empty() {
            "(none set)"
        } else {
            &g.next_step
        },
        g.autonomy,
        g.plan,
        g.progress_log,
        g.waiting_question
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("(none)")
    )
}

/// Create or update the linked check-in routine, or pause it when the goal is not active.
fn sync_check_in_routine(state: &AppState, goal: &mut Goal) -> Result<(), (StatusCode, String)> {
    let wants = goal.status == "active"
        && goal
            .check_in_cron
            .as_ref()
            .map(|c| !c.trim().is_empty())
            .unwrap_or(false);
    if !wants {
        if let Some(rid) = goal.routine_id.clone() {
            let _ = state
                .db
                .update_routine(&rid, None, None, None, None, Some(false), None);
        }
        return Ok(());
    }
    let cron = goal
        .check_in_cron
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_owned();
    let tz = goal.check_in_tz.trim();
    let trigger = Trigger::parse(&json!({
        "kind": "cron",
        "expr": cron,
        "tz": if tz.is_empty() { "UTC" } else { tz },
    }))
    .map_err(bad)?;
    let trigger_json = trigger.to_json();
    let prompt = check_in_prompt(goal);
    let name = format!("Goal: {}", goal.title);
    let chat_id = goal.chat_id.clone().ok_or_else(|| {
        // Fall back to the project's pinned/first chat.
        bad("goal needs a chat for check-ins")
    });
    let chat_id = match chat_id {
        Ok(c) => c,
        Err(_) => {
            let chats = state.db.list_chats(&goal.project_id).map_err(internal)?;
            chats
                .iter()
                .find(|c| c.is_pinned)
                .or(chats.first())
                .map(|c| c.id.clone())
                .ok_or_else(|| bad("project has no chat"))?
        }
    };
    if goal.chat_id.is_none() {
        goal.chat_id = Some(chat_id.clone());
    }
    let now = now_ms();
    let next = next_run_ms(&trigger, now);
    if let Some(rid) = goal.routine_id.clone() {
        if state.db.routine(&rid).map_err(internal)?.is_some() {
            state
                .db
                .update_routine(
                    &rid,
                    Some(&name),
                    Some(&prompt),
                    Some(&chat_id),
                    Some(&trigger_json),
                    Some(true),
                    next,
                )
                .map_err(internal)?;
            return Ok(());
        }
    }
    let rt = Routine {
        id: ulid(),
        project_id: goal.project_id.clone(),
        chat_id,
        name,
        prompt,
        trigger: trigger_json,
        enabled: true,
        created_at: now,
        last_run_at: None,
        next_run_at: next,
    };
    state.db.insert_routine(&rt).map_err(internal)?;
    goal.routine_id = Some(rt.id);
    Ok(())
}

/// `GET /goals` — every goal across the caller's projects (sidebar list).
pub async fn list_all(State(state): State<AppState>, principal: Principal) -> ApiResult {
    let user_id = principal
        .user_id()
        .ok_or((StatusCode::FORBIDDEN, "admin cannot list goals".into()))?;
    let projects = state.db.projects_of_user(user_id).map_err(internal)?;
    let mut goals = Vec::new();
    for p in &projects {
        for g in state.db.list_goals(&p.id).map_err(internal)? {
            goals.push(goal_dto(&g, Some(&p.name)));
        }
    }
    goals.sort_by(|a, b| {
        let la = a.last_moved_at.unwrap_or(0);
        let lb = b.last_moved_at.unwrap_or(0);
        lb.cmp(&la)
    });
    Ok(Json(GoalsListDto { goals }).into_response())
}

pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    let project_id = resolve_project_id(&state, &principal, &id)?;
    let name = state
        .db
        .project(&project_id)
        .map_err(internal)?
        .map(|p| p.name);
    let goals: Vec<GoalDto> = state
        .db
        .list_goals(&project_id)
        .map_err(internal)?
        .iter()
        .map(|g| goal_dto(g, name.as_deref()))
        .collect();
    Ok(Json(GoalsListDto { goals }).into_response())
}

#[derive(Deserialize)]
pub struct CreateGoalBody {
    pub title: String,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub plan: Option<Value>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub next_step: Option<String>,
    #[serde(default, alias = "chatId")]
    pub chat_id: Option<String>,
    #[serde(default, alias = "checkInCron")]
    pub check_in_cron: Option<String>,
    #[serde(default, alias = "checkInTz")]
    pub check_in_tz: Option<String>,
    #[serde(default)]
    pub autonomy: Option<String>,
    #[serde(default, alias = "linkedChatIds")]
    pub linked_chat_ids: Option<Value>,
    #[serde(default, alias = "linkedSubagentIds")]
    pub linked_subagent_ids: Option<Value>,
}

pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<CreateGoalBody>,
) -> ApiResult {
    let project_id = resolve_project_id(&state, &principal, &id)?;
    let title = b.title.trim().to_owned();
    if title.is_empty() {
        return Err(bad("title required"));
    }
    if title.len() > 200 {
        return Err(bad("title too long"));
    }
    let status = normalize_status(b.status.as_deref().unwrap_or("active"))?;
    let autonomy = normalize_autonomy(b.autonomy.as_deref().unwrap_or("ask"))?;
    let chats = state.db.list_chats(&project_id).map_err(internal)?;
    let chat_id = match b.chat_id.filter(|c| !c.is_empty()) {
        Some(c) => {
            if !chats.iter().any(|x| x.id == c) {
                return Err(bad(format!("no chat {c} in this project")));
            }
            Some(c)
        }
        None => chats
            .iter()
            .find(|c| c.is_pinned)
            .or(chats.first())
            .map(|c| c.id.clone()),
    };
    let now = now_ms();
    let mut goal = Goal {
        id: ulid(),
        project_id,
        chat_id,
        title,
        outcome: b.outcome.unwrap_or_default().trim().to_owned(),
        plan: as_array(b.plan.as_ref()),
        status,
        progress_log: json!([]),
        next_step: b.next_step.unwrap_or_default().trim().to_owned(),
        check_in_cron: b
            .check_in_cron
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()),
        check_in_tz: b
            .check_in_tz
            .unwrap_or_else(|| "UTC".into())
            .trim()
            .to_owned(),
        routine_id: None,
        autonomy,
        linked_chat_ids: as_array(b.linked_chat_ids.as_ref()),
        linked_subagent_ids: as_array(b.linked_subagent_ids.as_ref()),
        waiting_question: None,
        created_at: now,
        updated_at: now,
        last_moved_at: Some(now),
    };
    if goal.check_in_tz.is_empty() {
        goal.check_in_tz = "UTC".into();
    }
    sync_check_in_routine(&state, &mut goal)?;
    state.db.insert_goal(&goal).map_err(internal)?;
    let name = state
        .db
        .project(&goal.project_id)
        .ok()
        .flatten()
        .map(|p| p.name);
    Ok((StatusCode::CREATED, Json(goal_json(&goal, name.as_deref()))).into_response())
}

pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, gid)): Path<(String, String)>,
) -> ApiResult {
    let project_id = resolve_project_id(&state, &principal, &id)?;
    let g = load_goal(&state, &project_id, &gid)?;
    let name = state.db.project(&project_id).ok().flatten().map(|p| p.name);
    Ok(Json(goal_json(&g, name.as_deref())).into_response())
}

#[derive(Deserialize, Default)]
pub struct PatchGoalBody {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub plan: Option<Value>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub next_step: Option<String>,
    #[serde(default, alias = "chatId")]
    pub chat_id: Option<String>,
    /// Empty string clears the check-in cron.
    #[serde(default, alias = "checkInCron")]
    pub check_in_cron: Option<String>,
    #[serde(default, alias = "checkInTz")]
    pub check_in_tz: Option<String>,
    #[serde(default)]
    pub autonomy: Option<String>,
    #[serde(default, alias = "linkedChatIds")]
    pub linked_chat_ids: Option<Value>,
    #[serde(default, alias = "linkedSubagentIds")]
    pub linked_subagent_ids: Option<Value>,
    #[serde(default, alias = "waitingQuestion")]
    pub waiting_question: Option<String>,
    /// When true, clear `waiting_question` (user answered / agent resolved).
    #[serde(default)]
    pub clear_waiting: Option<bool>,
}

pub async fn patch(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, gid)): Path<(String, String)>,
    Json(b): Json<PatchGoalBody>,
) -> ApiResult {
    let project_id = resolve_project_id(&state, &principal, &id)?;
    let mut g = load_goal(&state, &project_id, &gid)?;
    if let Some(t) = b.title {
        let t = t.trim().to_owned();
        if t.is_empty() {
            return Err(bad("title required"));
        }
        g.title = t;
    }
    if let Some(o) = b.outcome {
        g.outcome = o.trim().to_owned();
    }
    if let Some(p) = b.plan {
        g.plan = as_array(Some(&p));
    }
    if let Some(s) = b.status {
        g.status = normalize_status(&s)?;
    }
    if let Some(n) = b.next_step {
        g.next_step = n.trim().to_owned();
    }
    if let Some(c) = b.chat_id {
        if !c.is_empty() && !state.db.chat_exists(&project_id, &c).map_err(internal)? {
            return Err(bad(format!("no chat {c} in this project")));
        }
        g.chat_id = (!c.is_empty()).then_some(c);
    }
    if let Some(cron) = b.check_in_cron {
        let t = cron.trim().to_owned();
        g.check_in_cron = if t.is_empty() { None } else { Some(t) };
    }
    if let Some(tz) = b.check_in_tz {
        let tz = tz.trim().to_owned();
        g.check_in_tz = if tz.is_empty() { "UTC".into() } else { tz };
    }
    if let Some(a) = b.autonomy {
        g.autonomy = normalize_autonomy(&a)?;
    }
    if let Some(v) = b.linked_chat_ids {
        g.linked_chat_ids = as_array(Some(&v));
    }
    if let Some(v) = b.linked_subagent_ids {
        g.linked_subagent_ids = as_array(Some(&v));
    }
    if b.clear_waiting == Some(true) {
        g.waiting_question = None;
    } else if let Some(q) = b.waiting_question {
        let t = q.trim().to_owned();
        g.waiting_question = if t.is_empty() { None } else { Some(t) };
    }
    let now = now_ms();
    g.updated_at = now;
    g.last_moved_at = Some(now);
    sync_check_in_routine(&state, &mut g)?;
    state.db.update_goal(&g).map_err(internal)?;
    let name = state.db.project(&project_id).ok().flatten().map(|p| p.name);
    Ok(Json(goal_json(&g, name.as_deref())).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, gid)): Path<(String, String)>,
) -> ApiResult {
    let project_id = resolve_project_id(&state, &principal, &id)?;
    let g = load_goal(&state, &project_id, &gid)?;
    if let Some(rid) = g.routine_id {
        let _ = state.db.delete_routine(&rid);
    }
    state.db.delete_goal(&gid).map_err(internal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
pub struct LogBody {
    pub text: String,
    #[serde(default)]
    pub at: Option<i64>,
}

/// `POST …/goals/{gid}/log` — append one dated progress line.
pub async fn append_log(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, gid)): Path<(String, String)>,
    Json(b): Json<LogBody>,
) -> ApiResult {
    let project_id = resolve_project_id(&state, &principal, &id)?;
    let mut g = load_goal(&state, &project_id, &gid)?;
    let text = b.text.trim().to_owned();
    if text.is_empty() {
        return Err(bad("text required"));
    }
    let at = b.at.unwrap_or_else(now_ms);
    let mut log = g.progress_log.as_array().cloned().unwrap_or_default();
    log.push(json!({ "at": at, "text": text }));
    // Keep the newest 200 entries.
    if log.len() > 200 {
        let skip = log.len() - 200;
        log = log.into_iter().skip(skip).collect();
    }
    g.progress_log = Value::Array(log);
    let now = now_ms();
    g.updated_at = now;
    g.last_moved_at = Some(now);
    // Refresh the check-in prompt so the next fire sees the new log.
    sync_check_in_routine(&state, &mut g)?;
    state.db.update_goal(&g).map_err(internal)?;
    let name = state.db.project(&project_id).ok().flatten().map(|p| p.name);
    Ok(Json(goal_json(&g, name.as_deref())).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_aliases() {
        assert_eq!(normalize_status("Active").unwrap(), "active");
        assert_eq!(normalize_status("waiting on me").unwrap(), "waiting");
        assert!(normalize_status("nope").is_err());
        assert_eq!(normalize_autonomy("ACT").unwrap(), "act");
    }

    const FIXED_AT: i64 = 1_700_000_000_000;

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

    fn fixture_goal() -> Goal {
        Goal {
            id: "goal_fixture".into(),
            project_id: "proj_fixture".into(),
            chat_id: Some("chat_primary".into()),
            title: "Ship the DTO step".into(),
            outcome: "Clients decode GoalDto".into(),
            plan: json!([
                { "id": "step_1", "title": "Add GoalDto", "status": "done" },
                { "id": "step_2", "title": "Wire the relay", "status": "doing" }
            ]),
            status: "active".into(),
            progress_log: json!([{ "at": FIXED_AT, "text": "Started the shared client core step." }]),
            next_step: "Regenerate RelayTypes.swift".into(),
            check_in_cron: Some("0 9 * * 1-5".into()),
            check_in_tz: "UTC".into(),
            routine_id: Some("routine_goal_checkin".into()),
            autonomy: "suggest".into(),
            linked_chat_ids: json!(["chat_primary"]),
            linked_subagent_ids: json!([]),
            waiting_question: None,
            created_at: FIXED_AT,
            updated_at: FIXED_AT + 60_000,
            last_moved_at: Some(FIXED_AT + 60_000),
        }
    }

    #[test]
    fn goal_wire_fixtures() {
        let goal = fixture_goal();
        let dto = goal_dto(&goal, Some("Fixture"));
        assert_builder("goal_json.json", goal_json(&goal, Some("Fixture")));
        assert_builder(
            "goals_list.json",
            serde_json::to_value(GoalsListDto { goals: vec![dto] }).expect("GoalsListDto"),
        );
    }
}
