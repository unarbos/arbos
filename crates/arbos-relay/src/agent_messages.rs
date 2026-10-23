//! Agent-to-agent messaging — the Grok Bot teammate send (design: `docs/agent-to-agent-messaging.md`).
//!
//! One agent's `send_message(to, text)` lands as a row **in the target agent's own chat** (no
//! group post, no new thread, nobody is joined to any room — #1436's rule). The relay resolves
//! `to` among the sender's owner's agents (id, name, `@handle`), checks permission (same owner:
//! always; another owner: a grant on the target agent's "Who may message X" card), applies the
//! D37 guards (depth ≤ 2, no cycles) plus a **per-pair cap** of [`PAIR_PER_HOUR`] turns, and
//! queues an `agent_message` push for the target box. The target's reply comes back as an
//! `agent_reply` push to the sender, who relays it to its user in prose.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use arbos_core::api::{MessageGrantDto, MessageGrantsDto};
use arbos_core::ids::ulid;
use arbos_core::wire::BoxState;

use crate::api::{internal, resolve_project_id, ApiResult};
use crate::auth::Principal;
use crate::boxes;
use crate::db::Project;
use crate::db_agent_messages::MessageGrant;
use crate::groups::{hop, MAX_DEPTH};
use crate::people::agent_slug;
use crate::state::AppState;

/// Turns one ordered pair of agents (A → B) may exchange per hour before both chats get a
/// "paused" line and the sender's tool is refused with `rate`.
pub const PAIR_PER_HOUR: i64 = 10;
const HOUR_MS: i64 = 3_600_000;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS agent_messages (
  id TEXT PRIMARY KEY, from_project TEXT NOT NULL, to_project TEXT NOT NULL, text TEXT NOT NULL,
  trace_id TEXT NOT NULL, depth INTEGER NOT NULL, chat_id TEXT, message_id TEXT,
  created_at INTEGER NOT NULL, replied_at INTEGER, reply_status TEXT
);
CREATE INDEX IF NOT EXISTS agent_messages_pair ON agent_messages(from_project, to_project, created_at);
CREATE TABLE IF NOT EXISTS agent_message_grants (
  project_id TEXT NOT NULL, grantee TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1,
  note TEXT NOT NULL DEFAULT '', added_by TEXT NOT NULL, created_at INTEGER NOT NULL, last_at INTEGER,
  PRIMARY KEY (project_id, grantee)
);
"#;

fn bad(msg: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, msg.into())
}

fn forbidden(msg: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::FORBIDDEN, msg.into())
}

/// Why a resolution found nothing or more than one agent.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolve {
    One(String),
    None,
    Ambiguous(Vec<String>),
}

/// Pick the agent `to` names among `candidates` (the owner's agents first, then agents that
/// granted the sender). Order: exact id → exact name or `@handle` (case-insensitive) → a
/// unique name/handle prefix. Pure, so the rule is unit-tested without a database.
pub fn resolve_target(to: &str, candidates: &[(String, String)]) -> Resolve {
    let raw = to.trim();
    let wanted = raw.trim_start_matches('@').to_lowercase();
    if wanted.is_empty() {
        return Resolve::None;
    }
    if let Some((id, _)) = candidates.iter().find(|(id, _)| id == raw) {
        return Resolve::One(id.clone());
    }
    let exact: Vec<&(String, String)> = candidates
        .iter()
        .filter(|(_, name)| name.to_lowercase() == wanted || agent_slug(name) == wanted)
        .collect();
    match exact.len() {
        1 => return Resolve::One(exact[0].0.clone()),
        n if n > 1 => return Resolve::Ambiguous(exact.iter().map(|(_, n)| n.clone()).collect()),
        _ => {}
    }
    let prefix: Vec<&(String, String)> = candidates
        .iter()
        .filter(|(_, name)| {
            name.to_lowercase().starts_with(&wanted) || agent_slug(name).starts_with(&wanted)
        })
        .collect();
    match prefix.len() {
        0 => Resolve::None,
        1 => Resolve::One(prefix[0].0.clone()),
        _ => Resolve::Ambiguous(prefix.iter().map(|(_, n)| n.clone()).collect()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendBody {
    /// Agent id, name or `@handle`.
    pub to: String,
    pub text: String,
    /// The sender's chat and hand-off row, so the reply can land on them.
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default)]
    pub depth: Option<u32>,
    #[serde(default)]
    pub trace: Vec<String>,
}

/// Grants the target's owner made, as `(grantee, enabled)`; `grantee` = `user:<id>` (every
/// agent of that person) or `project:<id>` (one agent).
fn grant_allows(state: &AppState, target: &Project, sender: &Project) -> bool {
    state
        .db
        .message_grant_enabled(&target.id, &format!("user:{}", sender.user_id))
        .unwrap_or(false)
        || state
            .db
            .message_grant_enabled(&target.id, &format!("project:{}", sender.id))
            .unwrap_or(false)
}

/// `POST /projects/{id}/agent-messages` (box of the sender's owner): deliver `text` from agent
/// `id` to agent `to`.
pub async fn send(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<SendBody>,
) -> ApiResult {
    let sender_id = resolve_project_id(&state, &id)?;
    if !state.allows(&principal, &sender_id) {
        return Err(forbidden("not your project"));
    }
    let sender = state
        .db
        .project(&sender_id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such project".to_owned()))?;
    let text = b.text.trim().to_owned();
    if text.is_empty() {
        return Err(bad("text required"));
    }
    if text.chars().count() > 8_000 {
        return Err(bad("text too long (8,000 characters max)"));
    }

    // Candidates: the owner's other agents, then agents of other owners that granted this
    // sender (by person or by agent).
    let mut candidates: Vec<(String, String)> = state
        .db
        .projects_of_user(&sender.user_id)
        .map_err(internal)?
        .into_iter()
        .filter(|p| p.id != sender.id)
        .map(|p| (p.id, p.name))
        .collect();
    for pid in state
        .db
        .projects_granting(&[
            format!("user:{}", sender.user_id),
            format!("project:{}", sender.id),
        ])
        .map_err(internal)?
    {
        if let Ok(Some(p)) = state.db.project(&pid) {
            if p.user_id != sender.user_id && !candidates.iter().any(|(cid, _)| *cid == p.id) {
                candidates.push((p.id, p.name));
            }
        }
    }
    let target_id = match resolve_target(&b.to, &candidates) {
        Resolve::One(id) => id,
        Resolve::None => {
            // An exact id of another owner's agent that did not grant: refuse, not "unknown".
            if let Ok(Some(p)) = state.db.project(b.to.trim()) {
                if p.id != sender.id && p.user_id != sender.user_id {
                    return Err(forbidden(format!(
                        "refused: {}'s owner has not allowed messages from your agents",
                        p.name
                    )));
                }
            }
            return Err((
                StatusCode::NOT_FOUND,
                format!(
                    "no agent named {:?}; your agents: {}",
                    b.to.trim(),
                    names(&candidates)
                ),
            ));
        }
        Resolve::Ambiguous(names) => {
            return Err(bad(format!(
                "ambiguous: {:?} matches {}",
                b.to.trim(),
                names.join(", ")
            )))
        }
    };
    if target_id == sender.id {
        return Err(bad("an agent cannot message itself"));
    }
    let target = state
        .db
        .project(&target_id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such agent".to_owned()))?;
    if target.user_id != sender.user_id && !grant_allows(&state, &target, &sender) {
        return Err(forbidden(format!(
            "refused: {}'s owner has not allowed messages from your agents",
            target.name
        )));
    }

    // D37: depth and cycles, then the per-pair cap.
    let depth = b.depth.unwrap_or(0);
    let mut trace = b.trace.clone();
    if !trace.contains(&sender.id) {
        trace.push(sender.id.clone());
    }
    let next_depth = match hop(depth, &trace, &target.id) {
        Ok(d) => d,
        Err("cycle") => {
            return Err(bad(
                "refused: this message already passed through that agent",
            ))
        }
        Err(_) => {
            return Err(bad(format!(
                "refused: a message may hop at most {MAX_DEPTH} agents"
            )))
        }
    };
    let used = state
        .db
        .agent_messages_in_window(&sender.id, &target.id, HOUR_MS)
        .map_err(internal)?;
    if used >= PAIR_PER_HOUR {
        // Both chats get one muted line (the target through its push queue; the sender
        // writes its own on the refusal). Only the first refusal of the window tells the target.
        if used == PAIR_PER_HOUR {
            let _ = state.db.enqueue_push(
                &target.id,
                "agent_message_paused",
                &json!({ "withProjectId": sender.id, "withName": sender.name, "cap": PAIR_PER_HOUR }),
            );
            let _ = state.db.record_agent_message(
                &ulid(),
                &sender.id,
                &target.id,
                "",
                "paused",
                0,
                None,
                None,
            );
        }
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "rate: {PAIR_PER_HOUR} messages between {} and {} this hour; paused until the hour passes",
                sender.name, target.name
            ),
        ));
    }

    let mid = ulid();
    let trace_id = b.trace_id.clone().unwrap_or_else(ulid);
    trace.push(target.id.clone());
    state
        .db
        .record_agent_message(
            &mid,
            &sender.id,
            &target.id,
            &text,
            &trace_id,
            next_depth,
            b.chat_id.as_deref(),
            b.message_id.as_deref(),
        )
        .map_err(internal)?;
    if target.user_id != sender.user_id {
        let _ = state.db.touch_message_grant(
            &target.id,
            &[
                format!("user:{}", sender.user_id),
                format!("project:{}", sender.id),
            ],
        );
    }
    let payload = json!({
        "id": mid,
        "fromProjectId": sender.id,
        "fromName": sender.name,
        "fromHandle": agent_slug(&sender.name),
        "fromIcon": sender.icon,
        "fromOwner": sender.user_id,
        "crossOwner": target.user_id != sender.user_id,
        "text": text,
        "traceId": trace_id,
        "depth": next_depth,
        "trace": trace,
        "replyTo": { "projectId": sender.id, "chatId": b.chat_id, "messageId": b.message_id },
    });
    state
        .db
        .enqueue_push(&target.id, "agent_message", &payload)
        .map_err(internal)?;
    let (box_state, _) = state.project_box_state(&target.id).await;
    boxes::ensure_awake_project(&state, &target.id, "agent-message").await;
    let status = if box_state == BoxState::Awake {
        "delivered"
    } else {
        "queued"
    };
    tracing::info!(from = %sender.id, to = %target.id, %mid, status, "agent message");
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "id": mid,
            "status": status,
            "to": { "id": target.id, "name": target.name, "handle": agent_slug(&target.name) },
        })),
    )
        .into_response())
}

fn names(candidates: &[(String, String)]) -> String {
    if candidates.is_empty() {
        return "(none)".to_owned();
    }
    candidates
        .iter()
        .map(|(_, n)| n.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyBody {
    pub text: String,
    /// `done` | `failed` | `empty`.
    #[serde(default)]
    pub status: Option<String>,
}

/// `POST /projects/{id}/agent-messages/{mid}/reply` (box of the target's owner): the target's
/// answer goes back to the sender as an `agent_reply` push.
pub async fn reply(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, mid)): Path<(String, String)>,
    Json(b): Json<ReplyBody>,
) -> ApiResult {
    let target_id = resolve_project_id(&state, &id)?;
    if !state.allows(&principal, &target_id) {
        return Err(forbidden("not your project"));
    }
    let row = state
        .db
        .agent_message(&mid)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such message".to_owned()))?;
    if row.to_project != target_id {
        return Err(forbidden("not the recipient of this message"));
    }
    if row.replied_at.is_some() {
        return Err((StatusCode::CONFLICT, "already answered".into()));
    }
    let status = b.status.clone().unwrap_or_else(|| "done".to_owned());
    state
        .db
        .mark_agent_message_replied(&mid, &status)
        .map_err(internal)?;
    let target = state.db.project(&target_id).map_err(internal)?;
    let payload = json!({
        "id": mid,
        "fromProjectId": target_id,
        "fromName": target.as_ref().map(|p| p.name.clone()),
        "fromHandle": target.as_ref().map(|p| agent_slug(&p.name)),
        "text": b.text.trim(),
        "status": status,
        "chatId": row.chat_id,
        "messageId": row.message_id,
        "traceId": row.trace_id,
        "depth": row.depth,
        "originalText": row.text,
    });
    state
        .db
        .enqueue_push(&row.from_project, "agent_reply", &payload)
        .map_err(internal)?;
    boxes::ensure_awake_project(&state, &row.from_project, "agent-reply").await;
    Ok(StatusCode::ACCEPTED.into_response())
}

// ---- grants: "Who may message X" on the agent's profile pane ----------------------------------

fn grant_dto(state: &AppState, project_id: &str, g: &MessageGrant) -> MessageGrantDto {
    let (kind, id) = g
        .grantee
        .split_once(':')
        .unwrap_or(("user", g.grantee.as_str()));
    let (name, handle, avatar_url) = match kind {
        "project" => match state.db.project(id).ok().flatten() {
            Some(p) => (Some(p.name.clone()), Some(agent_slug(&p.name)), None),
            None => (None, None, None),
        },
        _ => match state.db.user_card(id).ok().flatten() {
            Some(c) => (
                c.name,
                (!c.handle.is_empty()).then_some(c.handle),
                c.avatar_url,
            ),
            None => (None, None, None),
        },
    };
    MessageGrantDto {
        project_id: project_id.to_owned(),
        grantee: g.grantee.clone(),
        kind: if kind == "project" { "agent" } else { "person" }.to_owned(),
        id: id.to_owned(),
        name,
        handle,
        avatar_url,
        enabled: g.enabled,
        note: g.note.clone(),
        created_at: g.created_at,
        last_at: g.last_at,
    }
}

pub(crate) fn grants_dto(state: &AppState, project: &Project) -> MessageGrantsDto {
    MessageGrantsDto {
        project_id: project.id.clone(),
        grants: state
            .db
            .message_grants(&project.id)
            .unwrap_or_default()
            .iter()
            .map(|g| grant_dto(state, &project.id, g))
            .collect(),
    }
}

/// `GET /projects/{id}/message-access` — owner, member or the owner's box.
pub async fn list_grants(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    let pid = resolve_project_id(&state, &id)?;
    if !state.allows(&principal, &pid) {
        return Err(forbidden("not your project"));
    }
    let project = state
        .db
        .project(&pid)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such project".to_owned()))?;
    Ok(Json(grants_dto(&state, &project)).into_response())
}

#[derive(Deserialize)]
pub struct GrantBody {
    /// `@handle` / `user:<id>` for a person (every agent of theirs), `project:<id>` for one agent.
    pub grantee: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub note: Option<String>,
}

fn owner_only(principal: &Principal, project: &Project) -> Result<(), (StatusCode, String)> {
    match principal {
        Principal::Admin => Ok(()),
        Principal::Device { user_id, .. } if *user_id == project.user_id => Ok(()),
        _ => Err(forbidden(
            "only the agent's owner grants who may message it",
        )),
    }
}

/// `PUT /projects/{id}/message-access` — the owner adds or updates one grantee.
pub async fn put_grant(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<GrantBody>,
) -> ApiResult {
    let pid = resolve_project_id(&state, &id)?;
    let project = state
        .db
        .project(&pid)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such project".to_owned()))?;
    owner_only(&principal, &project)?;
    let grantee = normalize_grantee(&state, b.grantee.trim())?;
    if grantee == format!("user:{}", project.user_id)
        || grantee == format!("project:{}", project.id)
    {
        return Err(bad("your own agents may already message each other"));
    }
    state
        .db
        .upsert_message_grant(
            &pid,
            &grantee,
            b.enabled.unwrap_or(true),
            b.note.as_deref(),
            principal.user_id().unwrap_or("admin"),
        )
        .map_err(internal)?;
    Ok(Json(grants_dto(&state, &project)).into_response())
}

/// `DELETE /projects/{id}/message-access/{grantee}` — the owner removes one grantee.
pub async fn delete_grant(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, grantee)): Path<(String, String)>,
) -> ApiResult {
    let pid = resolve_project_id(&state, &id)?;
    let project = state
        .db
        .project(&pid)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such project".to_owned()))?;
    owner_only(&principal, &project)?;
    let grantee = normalize_grantee(&state, grantee.trim())?;
    state
        .db
        .delete_message_grant(&pid, &grantee)
        .map_err(internal)?;
    Ok(Json(grants_dto(&state, &project)).into_response())
}

fn normalize_grantee(state: &AppState, raw: &str) -> Result<String, (StatusCode, String)> {
    if raw.is_empty() {
        return Err(bad("grantee required"));
    }
    if let Some(rest) = raw.strip_prefix("project:") {
        let p = state
            .db
            .project(rest)
            .map_err(internal)?
            .ok_or((StatusCode::NOT_FOUND, "no such agent".to_owned()))?;
        return Ok(format!("project:{}", p.id));
    }
    if let Some(rest) = raw.strip_prefix("user:") {
        return Ok(format!("user:{rest}"));
    }
    let handle = raw.trim_start_matches('@');
    let card = state
        .db
        .user_by_handle(handle)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, format!("no Arbos user @{handle}")))?;
    Ok(format!("user:{}", card.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(i, n)| (i.to_string(), n.to_string()))
            .collect()
    }

    #[test]
    fn resolves_by_id_name_handle_and_unique_prefix() {
        let cands = c(&[
            ("p1", "Doug"),
            ("p2", "Home"),
            ("p3", "Head of Intelligence"),
        ]);
        assert_eq!(resolve_target("p2", &cands), Resolve::One("p2".into()));
        assert_eq!(resolve_target("doug", &cands), Resolve::One("p1".into()));
        assert_eq!(resolve_target("@Doug", &cands), Resolve::One("p1".into()));
        assert_eq!(
            resolve_target("@head-of-intelligence", &cands),
            Resolve::One("p3".into())
        );
        assert_eq!(resolve_target("hea", &cands), Resolve::One("p3".into()));
        assert_eq!(resolve_target("", &cands), Resolve::None);
        assert_eq!(resolve_target("Comms", &cands), Resolve::None);
    }

    #[test]
    fn ambiguous_prefix_is_reported_not_guessed() {
        let cands = c(&[("p1", "Doug"), ("p2", "Douglas")]);
        // An exact name wins over a longer one that shares the prefix.
        assert_eq!(resolve_target("doug", &cands), Resolve::One("p1".into()));
        assert_eq!(
            resolve_target("dou", &cands),
            Resolve::Ambiguous(vec!["Doug".into(), "Douglas".into()])
        );
    }
}
