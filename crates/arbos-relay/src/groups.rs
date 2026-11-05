//! Group chats (F6, design D69): one room with the user and several project agents. The host
//! project's box owns the chat doc; the relay fans messages out to the other participants as
//! `group_message` pushes and carries their replies back, under the cross-project guards of D37
//! (trace id, depth ≤ 2, no cycles, 20 messages per project per hour). Agents talk only
//! project-agent to project-agent here; subagents never reach this surface (agentd refuses them).

use std::collections::BTreeSet;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::api::{GroupDto, GroupParticipantDto};
use arbos_core::ids::{now_ms, ulid};

use crate::agent_messages::grant_allows;
use crate::api::{chat_dto, internal, lookup_project, no_such_project, project_for, ApiResult};
use crate::auth::Principal;
use crate::boxes;
use crate::db::{Chat, Project};
use crate::state::AppState;

/// A trace may hop at most this many times (A→B→C); D37.
pub const MAX_DEPTH: u32 = 2;
/// Messages one project may put into groups per hour; D37.
pub const RATE_PER_HOUR: i64 = 20;
const HOUR_MS: i64 = 3_600_000;

fn bad(msg: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, msg.into())
}

fn forbidden() -> (StatusCode, String) {
    (StatusCode::FORBIDDEN, "not your project".into())
}

/// Open the group named by `{id}/{chat}` as this caller: the host's side (the host project's
/// devices, its owner's box, a person the owner invited) or a participant's box (D70).
///
/// Everything a caller outside the group could tell apart is collapsed into one answer. The
/// host may not exist, the chat may not exist, the chat may not be a group, or the caller may
/// simply have nothing to do with it — all four are `no such project`, because the group
/// routes took an id *or a name*, and the difference leaked which names were taken
/// (reported 2026-11-02).
fn open_group(
    state: &AppState,
    principal: &Principal,
    id: &str,
    chat_id: &str,
) -> Result<(Project, Chat), (StatusCode, String)> {
    let host = lookup_project(state, id)?;
    let chat = state.db.chat(&host.id, chat_id).map_err(internal)?;
    let inside = state.allows(principal, &host.id)
        || (principal.is_box()
            && chat.as_ref().is_some_and(|c| {
                c.is_group() && c.participants.iter().any(|p| state.allows(principal, p))
            }));
    if !inside {
        return Err(no_such_project());
    }
    let chat = chat.ok_or_else(no_such_project)?;
    if !chat.is_group() {
        return Err((StatusCode::CONFLICT, "not a group chat".into()));
    }
    Ok((host, chat))
}

fn participant_details(state: &AppState, chat: &Chat) -> Vec<Value> {
    chat.participants
        .iter()
        .filter_map(|id| state.db.project(id).ok().flatten())
        .map(|p| json!({ "id": p.id, "name": p.name, "description": p.description, "icon": p.icon, "color": p.color, "state": p.state }))
        .collect()
}

pub(crate) async fn group_json(state: &AppState, chat: &Chat) -> Value {
    serde_json::to_value(group_dto(state, chat).await).expect("GroupDto")
}

pub(crate) async fn group_dto(state: &AppState, chat: &Chat) -> GroupDto {
    let base = chat_dto(chat);
    let mut details = Vec::new();
    for id in &chat.participants {
        if let Ok(Some(p)) = state.db.project(id) {
            let (bs, _) = state.box_state(&p.user_id).await;
            details.push(GroupParticipantDto {
                id: p.id.clone(),
                name: p.name.clone(),
                description: p.description.clone(),
                icon: p.icon.clone(),
                color: p.color.clone(),
                box_state: bs,
            });
        }
    }
    GroupDto {
        id: base.id,
        project_id: base.project_id,
        title: base.title,
        is_pinned: base.is_pinned,
        is_system: base.is_system,
        kind: base.kind,
        participants: base.participants,
        position: base.position,
        created_at: base.created_at,
        doc_epoch: base.doc_epoch,
        room: base.room,
        participants_detail: details,
    }
}

/// May a group hosted by `host` contain `candidate`?
///
/// Two ways in, and no third. The agent is the host owner's own, or its owner invited the host
/// owner onto it. Or its owner has already said yes to hearing from this host, through the
/// "Who may message X" grants that `agent_messages.rs` keeps — a group is agent-to-agent
/// traffic, so that is the consent it needs, and it is the consent the product already has.
///
/// Asked of the host rather than of whoever is calling, so that one answer governs both who
/// may be added and who the fan-out may reach.
fn may_join(state: &AppState, host: &Project, candidate: &Project) -> bool {
    candidate.user_id == host.user_id
        || state
            .db
            .is_project_member(&candidate.id, &host.user_id)
            .unwrap_or(false)
        || grant_allows(state, candidate, host)
}

/// `may_join` for a participant id, for rosters that are already written down.
fn roster_entry_ok(state: &AppState, host: &Project, participant: &str) -> bool {
    state
        .db
        .project(participant)
        .ok()
        .flatten()
        .is_some_and(|p| may_join(state, host, &p))
}

/// Resolve the names or ids the caller asked to add, dropping duplicates and the host.
///
/// An agent the caller may not add is reported as `unknown`, word for word, like a name nobody
/// has taken — this is the hole the reporter walked through, so the refusal must not say which
/// of the two happened.
fn resolve_participants(
    state: &AppState,
    host: &Project,
    raw: &[String],
) -> Result<Vec<String>, (StatusCode, String)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for r in raw {
        let wanted = r.trim();
        // The caller's own agents answer to their name first: two owners may both have a
        // `Lium`, and the bare lookup hands back whichever was active last.
        let found = match state
            .db
            .project_of_user_by_name(host.user_id.as_str(), wanted)
            .map_err(internal)?
        {
            Some(p) => Some(p),
            None => state.db.project(wanted).map_err(internal)?,
        };
        let candidate = found
            .filter(|p| may_join(state, host, p))
            .ok_or_else(|| bad(format!("unknown participant project {wanted:?}")))?;
        if candidate.id != host.id && seen.insert(candidate.id.clone()) {
            out.push(candidate.id);
        }
    }
    Ok(out)
}

/// Find one of the group's own participants by id or name. Nothing outside the roster is
/// looked up, so naming a stranger's agent here reads exactly like naming nobody.
fn in_roster(state: &AppState, chat: &Chat, wanted: &str) -> Option<String> {
    let wanted = wanted.trim();
    chat.participants
        .iter()
        .find(|p| {
            p.as_str() == wanted
                || state
                    .db
                    .project(p)
                    .ok()
                    .flatten()
                    .is_some_and(|x| x.name == wanted)
        })
        .cloned()
}

fn default_title(state: &AppState, others: &[String]) -> String {
    let names: Vec<String> = others
        .iter()
        .filter_map(|id| state.db.project(id).ok().flatten())
        .map(|p| p.name)
        .collect();
    if names.is_empty() {
        "Group".to_owned()
    } else {
        format!("Group · {}", names.join(", "))
    }
}

#[derive(Deserialize)]
pub struct CreateGroupBody {
    #[serde(default)]
    pub title: Option<String>,
    /// Project ids or names; the host project is implied and goes first.
    pub participants: Vec<String>,
}

/// `POST /projects/{id}/groups`: a group chat hosted by `{id}` with the given participants.
pub async fn create_group(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<CreateGroupBody>,
) -> ApiResult {
    let host = project_for(&state, &principal, &id)?;
    let others = resolve_participants(&state, &host, &b.participants)?;
    if others.is_empty() {
        return Err(bad("a group needs at least one other participant project"));
    }
    let mut participants = vec![host.id.clone()];
    participants.extend(others.iter().cloned());
    let n = state.db.list_chats(&host.id).map_err(internal)?.len() as i64;
    let chat = Chat {
        id: ulid(),
        project_id: host.id.clone(),
        title: b
            .title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| default_title(&state, &others)),
        is_pinned: false,
        position: n,
        created_at: now_ms(),
        doc_epoch: 1,
        kind: "group".into(),
        participants,
        forked_from: None,
    };
    state.db.insert_chat(&chat).map_err(internal)?;
    Ok(Json(group_json(&state, &chat).await).into_response())
}

/// `GET /projects/{id}/groups/{chat}`: the group entry plus participant details.
pub async fn get_group(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, chat_id)): Path<(String, String)>,
) -> ApiResult {
    let (host, chat) = open_group(&state, &principal, &id, &chat_id)?;
    state.matrix.touch_chat(&state, &chat_id, Some(&host.id));
    Ok(Json(group_json(&state, &chat).await).into_response())
}

#[derive(Deserialize)]
pub struct PatchGroupBody {
    #[serde(default)]
    pub add: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
    #[serde(default)]
    pub title: Option<String>,
}

/// `PATCH /projects/{id}/groups/{chat}`: add or remove participants, retitle. The host stays.
pub async fn patch_group(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, chat_id)): Path<(String, String)>,
    Json(b): Json<PatchGroupBody>,
) -> ApiResult {
    // The roster is the host's to change: a participant's box reads the group but does not
    // bring anyone else into it.
    let host = project_for(&state, &principal, &id)?;
    let (_, chat) = open_group(&state, &principal, &host.id, &chat_id)?;
    let add = resolve_participants(&state, &host, &b.add)?;
    // Removals are resolved against the roster only, so an agent whose owner has since
    // withdrawn its grant can still be shown out.
    let remove: Vec<String> = b
        .remove
        .iter()
        .filter_map(|r| in_roster(&state, &chat, r))
        .collect();
    let mut participants: Vec<String> = chat
        .participants
        .iter()
        .filter(|p| !remove.contains(p))
        .cloned()
        .collect();
    for p in add {
        if !participants.contains(&p) {
            participants.push(p);
        }
    }
    if participants.first() != Some(&host.id) {
        participants.retain(|p| p != &host.id);
        participants.insert(0, host.id.clone());
    }
    if participants.len() < 2 {
        return Err(bad("a group keeps at least one other participant project"));
    }
    state
        .db
        .set_chat_participants(&host.id, &chat_id, &participants)
        .map_err(internal)?;
    if let Some(t) = b.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        state
            .db
            .update_chat_title(&host.id, &chat_id, t)
            .map_err(internal)?;
    }
    let (_, chat) = open_group(&state, &principal, &host.id, &chat_id)?;
    Ok(Json(group_json(&state, &chat).await).into_response())
}

/// Who put a message into the group: the user (from the host chat) or a project agent.
enum Sender {
    User,
    Project(String),
}

fn parse_sender(from: &str) -> Result<Sender, (StatusCode, String)> {
    match from {
        "user" => Ok(Sender::User),
        other => other
            .strip_prefix("project:")
            .map(|p| Sender::Project(p.to_owned()))
            .ok_or_else(|| bad("from must be `user` or `project:<id>`")),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSendBody {
    /// `user` or `project:<id>`.
    pub from: String,
    pub text: String,
    /// Address one participant (project id or name); default: every other participant.
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default)]
    pub depth: Option<u32>,
    /// Projects already in this trace (cycle guard).
    #[serde(default)]
    pub trace: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
fn push_payload(
    chat: &Chat,
    host: &Project,
    from: &str,
    from_project: Option<&Project>,
    to: Option<&str>,
    text: &str,
    trace_id: &str,
    depth: u32,
    trace: &[String],
    reply: bool,
    participants: &[Value],
) -> Value {
    json!({
        "groupChatId": chat.id,
        "groupTitle": chat.title,
        "hostProjectId": host.id,
        "hostName": host.name,
        "from": from,
        "fromProjectId": from_project.map(|p| p.id.clone()),
        "fromName": from_project.map(|p| p.name.clone()),
        "fromIcon": from_project.map(|p| p.icon.clone()),
        "to": to,
        "text": text,
        "traceId": trace_id,
        "depth": depth,
        "trace": trace,
        "reply": reply,
        "participants": participants,
    })
}

/// D37 fan-out decision for one recipient: `Ok(next depth)` or the refusal reason.
pub fn hop(depth: u32, trace: &[String], recipient: &str) -> Result<u32, &'static str> {
    if trace.iter().any(|t| t == recipient) {
        return Err("cycle");
    }
    let next = depth + 1;
    if next > MAX_DEPTH {
        return Err("depth");
    }
    Ok(next)
}

/// `POST /projects/{id}/groups/{chat}/send`: fan a message out to the participants. The host
/// box calls it for each user message (the host agent's own turn runs locally); a participant
/// box calls it for `group_send(text, to?)`.
pub async fn group_send(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, chat_id)): Path<(String, String)>,
    Json(b): Json<GroupSendBody>,
) -> ApiResult {
    let (host, chat) = open_group(&state, &principal, &id, &chat_id)?;
    let host_id = host.id.clone();
    let text = b.text.trim().to_owned();
    if text.is_empty() {
        return Err(bad("text required"));
    }
    let sender = parse_sender(&b.from)?;
    let sender_project = match &sender {
        Sender::User => host_id.clone(),
        Sender::Project(p) => p.clone(),
    };
    if !chat.participants.contains(&sender_project) {
        return Err(bad("sender is not a participant of this group"));
    }
    // Nobody puts words in anyone else's mouth. A box speaks only for its user's projects (D70:
    // one box serves every project of a user) and the user's own line is the host side's to
    // fan out. Devices were exempt from both until 2026-11-02, which let any signed-in device
    // post into any group on the relay, as the host's user or as a participant agent.
    let may_send = match &sender {
        Sender::User => state.allows(&principal, &host_id),
        Sender::Project(p) => state.allows(&principal, p),
    };
    if !may_send {
        return Err(forbidden());
    }
    let trace_id = b.trace_id.clone().unwrap_or_else(ulid);
    let used = state
        .db
        .group_traffic_record(&sender_project, &chat.id, &trace_id, HOUR_MS)
        .map_err(internal)?;
    if used >= RATE_PER_HOUR {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            format!("rate: {RATE_PER_HOUR} group messages per project per hour"),
        ));
    }
    let depth = b.depth.unwrap_or(0);
    let mut trace: Vec<String> = b.trace.clone();
    if !trace.contains(&sender_project) {
        trace.push(sender_project.clone());
    }
    let from_project = match &sender {
        Sender::User => None,
        Sender::Project(_) => state.db.project(&sender_project).map_err(internal)?,
    };
    let to = match b.to.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => Some(
            in_roster(&state, &chat, t)
                .ok_or_else(|| bad("recipient is not a participant of this group"))?,
        ),
        None => None,
    };
    let details = participant_details(&state, &chat);

    let mut delivered: Vec<String> = Vec::new();
    let mut refused: Vec<Value> = Vec::new();

    // A project's words land in the host chat as a `project` message. When the host agent is
    // the one addressed, that message also drives its turn (`to` set).
    if matches!(sender, Sender::Project(_)) && sender_project != host_id {
        let addressed_to_host = to.as_deref() == Some(host_id.as_str());
        let payload = push_payload(
            &chat,
            &host,
            &b.from,
            from_project.as_ref(),
            to.as_deref(),
            &text,
            &trace_id,
            depth,
            &trace,
            !addressed_to_host,
            &details,
        );
        state
            .db
            .enqueue_push(&host_id, "group_message", &payload)
            .map_err(internal)?;
        boxes::ensure_awake_project(&state, &host_id, "group").await;
        delivered.push(host_id.clone());
    }

    let recipients: Vec<String> = match &to {
        Some(pid) => vec![pid.clone()],
        None => chat
            .participants
            .iter()
            .filter(|p| **p != sender_project && **p != host_id)
            .cloned()
            .collect(),
    };
    for r in recipients {
        if r == host_id {
            continue; // handled above
        }
        // The roster is asked again rather than trusted. A group written before the owner
        // check existed can name an agent this host was never entitled to reach, and a grant
        // can be withdrawn after the group was made; either way the fan-out stops here and
        // the agent is not woken.
        if !roster_entry_ok(&state, &host, &r) {
            refused.push(json!({ "project": r, "reason": "consent" }));
            continue;
        }
        match hop(depth, &trace, &r) {
            Ok(next) => {
                let payload = push_payload(
                    &chat,
                    &host,
                    &b.from,
                    from_project.as_ref(),
                    to.as_deref(),
                    &text,
                    &trace_id,
                    next,
                    &trace,
                    false,
                    &details,
                );
                state
                    .db
                    .enqueue_push(&r, "group_message", &payload)
                    .map_err(internal)?;
                boxes::ensure_awake_project(&state, &r, "group").await;
                delivered.push(r);
            }
            Err(reason) => refused.push(json!({ "project": r, "reason": reason })),
        }
    }
    tracing::info!(group = %chat.id, from = %b.from, ?delivered, ?refused, depth, "group fan-out");
    Ok(Json(
        json!({ "traceId": trace_id, "depth": depth, "delivered": delivered, "refused": refused }),
    )
    .into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupReplyBody {
    pub from_project_id: String,
    pub text: String,
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default)]
    pub depth: Option<u32>,
}

/// `POST /projects/{id}/groups/{chat}/reply`: a participant agent's answer to a group message,
/// appended to the host chat as a `project` message. No further fan-out.
pub async fn group_reply(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, chat_id)): Path<(String, String)>,
    Json(b): Json<GroupReplyBody>,
) -> ApiResult {
    let (host, chat) = open_group(&state, &principal, &id, &chat_id)?;
    let host_id = host.id.clone();
    // The answer has to come from the agent it claims to come from. The `is_box()` guard this
    // replaces let any signed-in device file a reply under any participant's name.
    if !state.allows(&principal, &b.from_project_id) {
        return Err(forbidden());
    }
    if !chat.participants.contains(&b.from_project_id) {
        return Err(bad("sender is not a participant of this group"));
    }
    let text = b.text.trim().to_owned();
    if text.is_empty() {
        return Err(bad("text required"));
    }
    let from_project = state
        .db
        .project(&b.from_project_id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such project".to_owned()))?;
    let trace_id = b.trace_id.clone().unwrap_or_else(ulid);
    let used = state
        .db
        .group_traffic_record(&b.from_project_id, &chat.id, &trace_id, HOUR_MS)
        .map_err(internal)?;
    if used >= RATE_PER_HOUR {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            format!("rate: {RATE_PER_HOUR} group messages per project per hour"),
        ));
    }
    let details = participant_details(&state, &chat);
    let payload = push_payload(
        &chat,
        &host,
        &format!("project:{}", from_project.id),
        Some(&from_project),
        None,
        &text,
        &trace_id,
        b.depth.unwrap_or(1),
        &[],
        true,
        &details,
    );
    state
        .db
        .enqueue_push(&host_id, "group_message", &payload)
        .map_err(internal)?;
    boxes::ensure_awake_project(&state, &host_id, "group").await;
    Ok(StatusCode::ACCEPTED.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D37: a trace hops at most twice and never back into a project it already visited.
    #[test]
    fn hop_enforces_depth_and_cycles() {
        let trace = vec!["a".to_owned(), "b".to_owned()];
        assert_eq!(hop(0, &trace, "c"), Ok(1));
        assert_eq!(hop(1, &trace, "c"), Ok(2));
        assert_eq!(hop(2, &trace, "c"), Err("depth"));
        assert_eq!(hop(0, &trace, "a"), Err("cycle"));
    }
}
