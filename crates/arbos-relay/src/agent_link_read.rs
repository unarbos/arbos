//! What an agent access link may **read** (PR 3 of 5; spec §5.4, §6.2, §8). The lifecycle is
//! [`crate::agent_link`]; this module is every `GET` an `arbl_…` bearer can reach.
//!
//! Three things the owner asked for, in the order he asked for them: his chats, his
//! conversations with his own agents, and the agent's log so the outside agent can actually
//! debug something rather than guess.
//!
//! | Route | Serves |
//! | --- | --- |
//! | `GET /agent-link/chats` | every chat the link may see, `?agent=<handle>` to narrow |
//! | `GET /agent-link/chats/{id}` | one chat and its full member list |
//! | `GET /agent-link/chats/{id}/messages` | history, `?before=&limit=&format=json\|md` |
//! | `GET /agent-link/agents` | the owner's agents, each with its chats |
//! | `GET /agent-link/agents/{id}/log` | that agent's debugging window and session log |
//! | `GET /agent-link/files/{slot}/{key}` | an attachment named by a message, `?w=` to scale |
//!
//! **One link, one credential.** Every route here authenticates with the same `arbl_…` bearer
//! the agent collected from the instruction sheet. Attachments are deliberately *not* served as
//! the hour-long pre-signed URLs `export.rs` mints for a reader device: a second credential
//! shape would be a second thing to hold, and the owner's rule is that he hands over exactly one
//! thing. `GET /agent-link/files/…` takes the bearer like everything else, so revoking the link
//! closes attachments in the same instant it closes the rest, with no signed URL outliving it.
//!
//! **One chokepoint.** [`visible_chats`] is the only function that decides what a link can
//! reach, and [`may_see`] is the only way a route asks about one chat. The stream (PR 4) and the
//! writes (PR 5) go through the same two. That is on purpose: the leak this feature has to not
//! have is "the agent saw a chat it was never granted", and one function is a thing a reviewer
//! can read and an attacker has to beat, where four agreeing filters are four chances to drift.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::doc::message_text;
use arbos_core::ids::now_ms;

use crate::agent_link::LinkPrincipal;
use crate::agent_link::{client_ip, db_error, not_found, user_agent, AgentLink};
use crate::db::Db;
use crate::matrix::backfill::load_doc;
use crate::people::{self, PEOPLE_ACL_PREFIX};
use crate::signin::api_error;
use crate::state::AppState;

/// Messages one link may be handed in an hour, and messages every link of one account may be
/// handed together. The request budget in [`crate::agent_link`] bounds how *often* an agent may
/// call; these bound how much it may carry away, which is the number that matters for a grant
/// made to a stranger. Spec §8.1.
const READ_MESSAGES_PER_HOUR: i64 = 5_000;
const READ_MESSAGES_PER_HOUR_USER: i64 = 10_000;
const HOUR_MS: i64 = 3_600_000;

/// Messages one page of history returns.
const MESSAGES_DEFAULT: usize = 50;
const MESSAGES_MAX: usize = 200;

/// Session-log entries one call returns.
const LOG_DEFAULT: i64 = 200;
const LOG_MAX: i64 = 500;

// ---- the one chokepoint -------------------------------------------------------------------------

/// A chat a link is allowed to reach.
///
/// `project` is what separates the two halves of the union and it is load-bearing, not
/// bookkeeping: a project chat's room and blob slot are keyed on the project id, a people chat's
/// on the chat id. Getting it wrong does not leak anything — it reads the wrong room and finds
/// nothing — but it is the difference between answering the owner's question and answering an
/// empty one.
#[derive(Debug, Clone)]
pub struct ChatView {
    pub id: String,
    /// `Some(project_id)` for a conversation with one of the owner's own agents; `None` for a
    /// DM or group with other people.
    pub project: Option<String>,
    pub kind: String,
    pub title: String,
    pub created_at: i64,
}

impl ChatView {
    /// The blob slot a message's attachments live under.
    pub(crate) fn slot(&self) -> String {
        match &self.project {
            Some(pid) => pid.clone(),
            None => format!("{PEOPLE_ACL_PREFIX}{}", self.id),
        }
    }

    pub(crate) fn json(&self) -> Value {
        json!({
            "id": self.id,
            "kind": self.kind,
            "title": self.title,
            "created_at": self.created_at,
            "agent": self.project,
        })
    }
}

/// Every chat this link may read, newest activity first.
///
/// The union is both halves of what "my chats" means to the owner, and the second half is the
/// one a careless implementation drops:
///
/// - **his agents.** For each project of the account, every chat in it. Jacob's own example is
///   `codingarbos`, which is a native agent, so that conversation is a *project* chat. A design
///   that walked only the people tables would list every DM he has and miss the exact
///   conversation he asked about.
/// - **his people.** DMs and groups, through `chats_of_member("user:{id}")` — the same call
///   `people::chats_list` makes for his own app.
///
/// Then the link's own restriction. `link.chats` is `None` for a link over the whole account and
/// `Some(list)` for one pinned to named chats; an empty list is refused at mint, so `Some` can
/// never read as "no filter".
pub fn visible_chats(state: &AppState, link: &AgentLink) -> Vec<ChatView> {
    let mut out: Vec<ChatView> = Vec::new();
    for project in state.db.projects_of_user(&link.user_id).unwrap_or_default() {
        for chat in state.db.list_chats(&project.id).unwrap_or_default() {
            out.push(ChatView {
                id: chat.id,
                project: Some(project.id.clone()),
                kind: if chat.kind == "chat" {
                    "agent".to_owned()
                } else {
                    chat.kind.clone()
                },
                title: if chat.title.trim().is_empty() {
                    project.name.clone()
                } else {
                    chat.title.clone()
                },
                created_at: chat.created_at,
            });
        }
    }
    for cm in state
        .db
        .chats_of_member(&format!("user:{}", link.user_id))
        .unwrap_or_default()
    {
        out.push(ChatView {
            title: people_title(state, &cm),
            id: cm.chat.id,
            project: None,
            kind: cm.chat.kind,
            created_at: cm.chat.created_at,
        });
    }
    if let Some(only) = &link.chats {
        out.retain(|c| only.iter().any(|w| w == &c.id));
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
    out
}

/// The one chat, if this link may see it. Every route that names a chat id goes through here,
/// and a chat the link may not reach is a `404` rather than a `403`: a link pinned to one
/// conversation should not be able to enumerate the existence of the rest of the account.
pub fn may_see(state: &AppState, link: &AgentLink, chat_id: &str) -> Option<ChatView> {
    visible_chats(state, link)
        .into_iter()
        .find(|c| c.id == chat_id)
}

/// A people chat's display name: its own title, or the member handles, matching what
/// `export::export_chat` builds so the two surfaces cannot disagree.
fn people_title(state: &AppState, cm: &crate::db_people::ChatWithMembers) -> String {
    if let Some(t) = cm.chat.title.as_ref().filter(|t| !t.trim().is_empty()) {
        return t.clone();
    }
    let handles: Vec<String> = cm
        .members
        .iter()
        .filter_map(|m| m.member.strip_prefix("user:"))
        .filter_map(|uid| state.db.user(uid).ok().flatten())
        .filter_map(|u| u.login)
        .collect();
    handles.join(", ")
}

// ---- the read budget ----------------------------------------------------------------------------

impl Db {
    /// Messages this link has been handed inside `window_ms`, summed from the `read` audit rows
    /// it already writes.
    ///
    /// Counted off the audit rather than off `rate_events` for two reasons. `rate_events` holds
    /// one row per event with no weight column, so charging 200 messages would mean 200 inserts
    /// through the relay's single SQLite writer on every page — the budget would cost more than
    /// the read it guards. And the audit is already the authority for what a link did (spec
    /// §9.3), so summing it means the number the owner reads and the number the budget enforces
    /// cannot disagree. No new table and no new column: `link_audit` is one of the three Jacob
    /// approved.
    pub fn link_messages_read(&self, link_id: &str, window_ms: i64) -> anyhow::Result<i64> {
        let since = now_ms() - window_ms;
        self.with(|c| {
            c.query_row(
                "SELECT COALESCE(SUM(CAST(json_extract(detail, '$.messages') AS INTEGER)), 0)
                 FROM link_audit WHERE link_id = ?1 AND action = 'read' AND at >= ?2",
                params![link_id, since],
                |r| r.get(0),
            )
        })
    }

    /// The same sum across every link of one account, so a stolen device that minted the full
    /// five links cannot read five times the budget.
    pub fn user_link_messages_read(&self, user_id: &str, window_ms: i64) -> anyhow::Result<i64> {
        let since = now_ms() - window_ms;
        self.with(|c| {
            c.query_row(
                "SELECT COALESCE(SUM(CAST(json_extract(detail, '$.messages') AS INTEGER)), 0)
                 FROM link_audit WHERE user_id = ?1 AND action = 'read' AND at >= ?2",
                params![user_id, since],
                |r| r.get(0),
            )
        })
    }
}

/// Is there room in both budgets for `want` more messages? Checked **before** the page is built,
/// so a refusal hands over nothing at all.
pub(crate) fn read_budget(
    state: &AppState,
    link: &AgentLink,
    want: i64,
) -> Result<(), Box<Response>> {
    let mine = state
        .db
        .link_messages_read(&link.id, HOUR_MS)
        .unwrap_or(READ_MESSAGES_PER_HOUR);
    if mine + want > READ_MESSAGES_PER_HOUR {
        return Err(Box::new(read_limited(
            "this link has read its hourly message budget; wait and continue from the cursor",
        )));
    }
    let ours = state
        .db
        .user_link_messages_read(&link.user_id, HOUR_MS)
        .unwrap_or(READ_MESSAGES_PER_HOUR_USER);
    if ours + want > READ_MESSAGES_PER_HOUR_USER {
        return Err(Box::new(read_limited(
            "this account's agent links have read their hourly message budget together; wait and continue from the cursor",
        )));
    }
    Ok(())
}

/// A message-budget refusal. `Retry-After` is an hour because that is this budget's window —
/// unlike the per-minute request budget, retrying in a minute cannot help.
fn read_limited(message: &str) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, "3600")],
        Json(json!({"error": {"type": "rate_limited", "message": message}})),
    )
        .into_response()
}

/// Record a read. `messages` is what [`Db::link_messages_read`] sums, so every path that hands
/// messages over must come through here or the budget silently stops counting.
pub(crate) fn note_read(
    state: &AppState,
    link: &AgentLink,
    headers: &HeaderMap,
    chat: &str,
    messages: i64,
) {
    let _ = state.db.note_link_audit(
        &link.id,
        &link.user_id,
        "read",
        Some(chat),
        None,
        client_ip(headers).as_deref(),
        user_agent(headers).as_deref(),
        &json!({ "messages": messages }),
    );
}

// ---- the chat list ------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct ChatsQuery {
    /// An agent's handle or id: only conversations with that agent of the owner's.
    #[serde(default)]
    pub agent: Option<String>,
}

/// `GET /agent-link/chats?agent=<handle>` — every chat this link may read.
///
/// No message text and no member list: a list the agent walks to find the one conversation it
/// was asked about should not be a way to read the whole account in one call.
pub async fn chats(
    State(state): State<AppState>,
    who: LinkPrincipal,
    Query(q): Query<ChatsQuery>,
) -> Response {
    let mut visible = visible_chats(&state, &who.link);
    if let Some(want) = q.agent.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let Some(project) = owned_agent(&state, &who.link, want) else {
            return api_error(
                StatusCode::NOT_FOUND,
                "not_found",
                "no such agent on this account",
            );
        };
        visible.retain(|c| c.project.as_deref() == Some(project.id.as_str()));
    }
    Json(json!({
        "chats": visible.iter().map(ChatView::json).collect::<Vec<_>>(),
        "count": visible.len(),
    }))
    .into_response()
}

/// `GET /agent-link/chats/{id}` — one chat with everybody in it, people and agents alike, so the
/// reader knows who it is about to read before it reads them.
pub async fn chat(
    State(state): State<AppState>,
    who: LinkPrincipal,
    Path(id): Path<String>,
) -> Response {
    let Some(view) = may_see(&state, &who.link, &id) else {
        return not_found();
    };
    let mut body = view.json();
    body["members"] = json!(members_json(&state, &view));
    Json(body).into_response()
}

/// Who is in a chat. A project chat's roster is not stored as memberships — it is the owner and
/// the agent (and, for a group chat, the other agents it names), so it is assembled here rather
/// than queried.
fn members_json(state: &AppState, view: &ChatView) -> Vec<Value> {
    match &view.project {
        Some(pid) => {
            let mut out = Vec::new();
            if let Some(p) = state.db.project(pid).ok().flatten() {
                if let Some(card) = state.db.user_card(&p.user_id).ok().flatten() {
                    out.push(json!({
                        "kind": "user", "id": p.user_id, "handle": card.handle,
                        "name": card.name, "role": "owner",
                    }));
                }
                out.push(json!({
                    "kind": "agent", "id": p.id, "handle": people::agent_slug(&p.name),
                    "name": p.name, "role": "agent",
                }));
            }
            out
        }
        None => state
            .db
            .members_of(&view.id)
            .unwrap_or_default()
            .into_iter()
            .map(|m| {
                let who = m.member.clone();
                if let Some(uid) = who.strip_prefix("user:") {
                    let card = state.db.user_card(uid).ok().flatten();
                    return json!({
                        "kind": "user", "id": uid,
                        "handle": card.as_ref().map(|c| c.handle.clone()).unwrap_or_default(),
                        "name": card.and_then(|c| c.name).unwrap_or_default(),
                        "role": m.role,
                    });
                }
                if let Some(pid) = who.strip_prefix("agent:") {
                    let p = state.db.project(pid).ok().flatten();
                    return json!({
                        "kind": "agent", "id": pid,
                        "handle": p.as_ref().map(|p| people::agent_slug(&p.name)).unwrap_or_default(),
                        "name": p.map(|p| p.name).unwrap_or_default(),
                        "role": m.role,
                    });
                }
                json!({ "kind": "system", "id": who, "role": m.role })
            })
            .collect(),
    }
}

// ---- history ------------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct MessagesQuery {
    /// A message id: the page ends just before it, so paging walks backwards through history.
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    /// `json` (default) or `md`.
    #[serde(default)]
    pub format: Option<String>,
}

/// One message as a link reads it.
struct Line {
    id: String,
    at: i64,
    sender: Value,
    text: String,
    attachments: Vec<Value>,
    reply_to: Option<String>,
}

impl Line {
    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "at": self.at,
            "sender": self.sender,
            "text": self.text,
            "attachments": self.attachments,
            "reply_to": self.reply_to,
        })
    }
}

/// One message of a chat doc, rendered exactly as a page of history renders it.
///
/// The stream (PR 4) sends this rather than building its own shape, so the two surfaces cannot
/// drift: an agent that read a message from `…/messages` and then saw it arrive on the stream
/// sees the same text, the same sender and the same attachment paths both times. `None` when the
/// doc does not hold the message — deleted, or not applied yet.
pub(crate) fn line_json(
    state: &AppState,
    view: &ChatView,
    rows: &[Value],
    message_id: &str,
) -> Option<Value> {
    lines(state, view, rows)
        .iter()
        .find(|l| l.id == message_id)
        .map(Line::json)
}

/// `GET /agent-link/chats/{id}/messages?before=&limit=&format=json|md` — history, newest last.
///
/// Newest-last inside the page but paged backwards from the newest: an agent asked to debug a
/// conversation wants the end of it first, and then to walk back only as far as it needs. The
/// alternative — oldest first — makes the common case read the whole history to reach the part
/// that matters, which is both slower for the agent and more of the account spent out of the
/// budget for no gain.
pub async fn messages(
    State(state): State<AppState>,
    who: LinkPrincipal,
    Path(id): Path<String>,
    Query(q): Query<MessagesQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(view) = may_see(&state, &who.link, &id) else {
        return not_found();
    };
    let limit = q.limit.unwrap_or(MESSAGES_DEFAULT).clamp(1, MESSAGES_MAX);
    if let Err(r) = read_budget(&state, &who.link, limit as i64) {
        return *r;
    }
    let doc = match load_doc(&state, &view.id, view.project.as_deref()) {
        Ok(d) => d,
        Err(e) => return db_error("load chat doc", e),
    };
    let all: Vec<Line> = doc
        .as_ref()
        .map(|d| lines(&state, &view, &d.projected_rows()))
        .unwrap_or_default();
    // Everything strictly before `before`, then the last `limit` of it: the page nearest the
    // cursor, in reading order.
    let end = q
        .before
        .as_deref()
        .and_then(|b| all.iter().position(|l| l.id == b))
        .unwrap_or(all.len());
    let head = &all[..end];
    let start = head.len().saturating_sub(limit);
    let page = &head[start..];
    // Charged for what was actually handed over, not for what was asked for.
    note_read(&state, &who.link, &headers, &view.id, page.len() as i64);
    let more = start > 0;
    let next = if more {
        page.first().map(|l| l.id.clone())
    } else {
        None
    };
    if q.format.as_deref() == Some("md") {
        let mut md = render_md(&view.title, page);
        if let Some(n) = &next {
            md.push_str(&format!("\n<!-- older messages: before={n} -->\n"));
        }
        return ([(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], md).into_response();
    }
    Json(json!({
        "chat": view.json(),
        "messages": page.iter().map(Line::json).collect::<Vec<_>>(),
        "has_more": more,
        "before": next,
    }))
    .into_response()
}

/// The finished messages of a chat doc, oldest first.
///
/// `queued` and `streaming` rows are dropped for the same reason `export::lines` drops them: a
/// half-written agent reply is not yet a message, and handing one over would have the reader
/// quoting text the owner never saw settle.
fn lines(state: &AppState, view: &ChatView, rows: &[Value]) -> Vec<Line> {
    let slot = view.slot();
    let mut out: Vec<Line> = rows
        .iter()
        .filter(|r| !matches!(r["status"].as_str(), Some("queued" | "streaming")))
        .filter_map(|r| {
            let id = r["id"].as_str()?.to_owned();
            let text = message_text(r);
            let attachments: Vec<Value> = r["parts"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .filter(|p| {
                            p["kind"] == "attachment" && p["hidden"].as_bool() != Some(true)
                        })
                        .filter_map(|p| {
                            let a = &p["attachment"];
                            let key = a["ref"].as_str()?;
                            Some(json!({
                                "name": a["name"], "mime": a["mime"], "kind": a["kind"],
                                // Fetched with the link's own bearer. No pre-signed URL, so
                                // nothing here outlives a revoke.
                                "path": format!("/agent-link/files/{slot}/{key}"),
                            }))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if text.trim().is_empty() && attachments.is_empty() {
                return None;
            }
            Some(Line {
                id,
                at: r["createdAt"].as_i64().unwrap_or(0),
                sender: crate::export::sender_json(state, r),
                text,
                attachments,
                reply_to: r["replyTo"].as_str().map(str::to_owned),
            })
        })
        .collect();
    out.sort_by_key(|l| l.at);
    out
}

fn render_md(title: &str, lines: &[Line]) -> String {
    let mut out = format!("# {title}\n\n");
    for l in lines {
        let who = match l.sender["kind"].as_str() {
            Some("user") => format!("@{}", l.sender["handle"].as_str().unwrap_or("?")),
            Some("agent") => format!("{} (agent)", l.sender["name"].as_str().unwrap_or("agent")),
            _ => "system".to_owned(),
        };
        let when = chrono::DateTime::from_timestamp_millis(l.at)
            .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_default();
        out.push_str(&format!("**{who}** · {when}\n{}\n", l.text.trim()));
        for a in &l.attachments {
            out.push_str(&format!(
                "- attachment: {} ({}) {}\n",
                a["name"].as_str().unwrap_or("file"),
                a["mime"].as_str().unwrap_or(""),
                a["path"].as_str().unwrap_or("")
            ));
        }
        out.push('\n');
    }
    out
}

// ---- the owner's agents, and their logs ---------------------------------------------------------

/// One agent of this account, by handle or id, if the link may reach it at all.
///
/// "May reach" is deliberately defined through [`visible_chats`] rather than through project
/// ownership: a link pinned to named chats must not be able to read the log of an agent whose
/// conversations it was never given. Ownership alone would have let it.
fn owned_agent(
    state: &AppState,
    link: &AgentLink,
    id_or_handle: &str,
) -> Option<crate::db::Project> {
    let want = id_or_handle.trim().trim_start_matches('@');
    let reachable: Vec<String> = visible_chats(state, link)
        .into_iter()
        .filter_map(|c| c.project)
        .collect();
    state
        .db
        .projects_of_user(&link.user_id)
        .unwrap_or_default()
        .into_iter()
        .find(|p| {
            reachable.iter().any(|r| r == &p.id)
                && (p.id == want
                    || p.name.eq_ignore_ascii_case(want)
                    || people::agent_slug(&p.name) == people::agent_slug(want))
        })
}

/// `GET /agent-link/agents` — the owner's agents this link may reach, each with its chats.
///
/// This is the index for the debugging job: it answers "which of these is `codingarbos`, and
/// which conversation do I read" in one call, so the agent does not have to guess an id.
pub async fn agents(State(state): State<AppState>, who: LinkPrincipal) -> Response {
    let visible = visible_chats(&state, &who.link);
    let mut out: Vec<Value> = Vec::new();
    for p in state
        .db
        .projects_of_user(&who.link.user_id)
        .unwrap_or_default()
    {
        let chats: Vec<Value> = visible
            .iter()
            .filter(|c| c.project.as_deref() == Some(p.id.as_str()))
            .map(ChatView::json)
            .collect();
        if chats.is_empty() {
            continue;
        }
        out.push(json!({
            "id": p.id,
            "handle": people::agent_slug(&p.name),
            "name": p.name,
            "state": p.state,
            "last_active_at": p.last_active_at,
            "chats": chats,
        }));
    }
    Json(json!({ "agents": out, "count": out.len() })).into_response()
}

#[derive(Deserialize, Default)]
pub struct LogQuery {
    /// Unix ms; events at or after it. Defaults to the last hour, as the owner's own view does.
    #[serde(default)]
    pub since: Option<i64>,
    /// Narrow the window to one chat.
    #[serde(default)]
    pub chat: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    /// `json` (default) or `md`.
    #[serde(default)]
    pub format: Option<String>,
}

/// `GET /agent-link/agents/{id}/log?since=&chat=&limit=&format=` — how this agent behaved.
///
/// This is the route Jacob's sentence asks for: *read the agent logs so it can debug*. It is the
/// owner's own `GET /projects/{id}/inspect` window — turns, tool calls, route and mention-gate
/// decisions, quick replies, approvals, warnings and the agentd audit tail — reached through
/// [`crate::inspect::window`] so the agent reads the **same** window the owner does. A separate
/// view assembled here would drift, and a debugging view that disagrees with the owner's is
/// worse than none.
///
/// Scope is [`crate::inspect::Scope::Owner`], because a link *is* the owner's delegate and a
/// member-scope window hides exactly the tool calls and warnings a debugger needs. Two
/// deliberate narrowings on top of it:
///
/// 1. **The box's infrastructure block is removed** — `sandbox_id`, `provider`, `snapshot`,
///    `last_started_at`, `last_seen_at`. None of it helps debug a conversation and all of it is
///    cloud-account detail about the owner's sandbox. `reachable`, `state` and the agentd
///    version stay, because "the box was asleep" is often the whole answer.
/// 2. **The session log is merged in** — the sidecar's own `asp_log` entries, which the owner
///    reads on the Details pane. They are already text-free by construction (`outside_log`
///    drops every key that could carry content or a secret), so they add latency and error
///    facts without adding any message text.
pub async fn agent_log(
    State(state): State<AppState>,
    who: LinkPrincipal,
    Path(id): Path<String>,
    Query(q): Query<LogQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(project) = owned_agent(&state, &who.link, &id) else {
        return api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no such agent on this account",
        );
    };
    // A `chat=` narrowing must still be a chat this link may see, or the log would be a way
    // around `may_see`.
    if let Some(chat) = q.chat.as_deref().filter(|c| !c.trim().is_empty()) {
        if may_see(&state, &who.link, chat).is_none() {
            return not_found();
        }
    }
    let mut raw: HashMap<String, String> = HashMap::new();
    if let Some(since) = q.since {
        raw.insert("since".into(), since.to_string());
    }
    if let Some(chat) = q.chat.clone() {
        raw.insert("chat".into(), chat);
    }
    if let Some(limit) = q.limit {
        raw.insert("limit".into(), limit.to_string());
    }
    if q.format.as_deref() == Some("md") {
        raw.insert("format".into(), "md".into());
    }
    let iq = crate::inspect::InspectQuery::parse(&raw);
    let mut body =
        crate::inspect::window(&state, &project, &crate::inspect::Scope::Owner, &iq).await;
    if let Some(b) = body["box"].as_object_mut() {
        for k in [
            "sandbox_id",
            "provider",
            "snapshot",
            "last_started_at",
            "last_seen_at",
        ] {
            b.remove(k);
        }
    }
    body["scope"] = json!("agent_link");
    let limit = q.limit.unwrap_or(LOG_DEFAULT).clamp(1, LOG_MAX);
    let session: Vec<Value> = state
        .db
        .asp_log_after(&project.id, 0, limit)
        .unwrap_or_default()
        .into_iter()
        .map(|(id, mut v)| {
            v["id"] = json!(id);
            v
        })
        .collect();
    body["session_log"] = json!(session);
    let _ = state.db.note_link_audit(
        &who.link.id,
        &who.link.user_id,
        "read",
        q.chat.as_deref(),
        None,
        client_ip(&headers).as_deref(),
        user_agent(&headers).as_deref(),
        // No `messages`: a log carries no message text, so it spends the request budget but not
        // the message budget. Charging it would make debugging compete with reading the
        // conversation being debugged.
        &json!({ "agent": project.id, "log": true }),
    );
    if q.format.as_deref() == Some("md") {
        return (
            [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
            crate::inspect::render_markdown(&body),
        )
            .into_response();
    }
    Json(body).into_response()
}

// ---- attachments --------------------------------------------------------------------------------

/// `GET /agent-link/files/{slot}/{key}?w=<px>` — the bytes of an attachment a message named.
///
/// `slot` is checked against the link's own visible chats, not merely against the account: a
/// link pinned to one conversation cannot read a picture out of another. `?w=` scales an image
/// on the way out, the same path the apps' thumbnails use.
pub async fn file(
    State(state): State<AppState>,
    who: LinkPrincipal,
    Path((slot, key)): Path<(String, String)>,
    Query(size): Query<crate::avatar_image::SizeQuery>,
) -> Response {
    let allowed = visible_chats(&state, &who.link)
        .iter()
        .any(|c| c.slot() == slot);
    if !allowed {
        return not_found();
    }
    if let Some(w) = size.w {
        if let crate::blobs::StoredImage::Image(kind) =
            crate::blobs::stored_image(&state, &slot, &key).await
        {
            if let Some(bytes) = crate::blobs::read(&state, &slot, &key).await {
                return crate::avatar_image::respond(
                    &state,
                    crate::avatar_image::Master { kind, bytes },
                    Some(w),
                    None,
                )
                .await;
            }
        }
    }
    crate::blobs::serve(&state, &slot, &key).await
}
#[cfg(test)]
mod tests {
    //! What a link may read, and the four ways it must not: another account, a chat it was not
    //! granted, an attachment outside those chats, and more of the account than its budget.

    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use serde_json::json;

    use arbos_core::doc::{ChatDoc, UserMessageOpts};
    use arbos_core::ids::ulid;
    use arbos_core::wire;

    use super::*;
    use crate::agent_link::Scope;
    use crate::db::Chat;
    use crate::people::tests_support::{dm, person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    async fn json_of(r: Response) -> (StatusCode, Value) {
        let status = r.status();
        let bytes = to_bytes(r.into_body(), 1 << 22).await.expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn text_of(r: Response) -> (StatusCode, String) {
        let status = r.status();
        let bytes = to_bytes(r.into_body(), 1 << 22).await.expect("body");
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// An opened link of `user`, straight through the storage the lifecycle PR built. Going
    /// through `mint` + `open_sheet` is covered by that PR's own tests; here the link is a
    /// fixture, not the thing under test.
    fn link_for(state: &AppState, user: &str, scope: Scope, chats: Option<&[String]>) -> AgentLink {
        let id = ulid();
        state
            .db
            .insert_agent_link(
                &id,
                user,
                "Debugger",
                scope,
                &arbos_core::auth::hash(&ulid()),
                chats,
                "dev:test",
                7 * 24 * 3_600_000,
            )
            .unwrap();
        state
            .db
            .open_agent_link(&id, &arbos_core::auth::hash(&ulid()))
            .unwrap();
        state.db.agent_link(&id).unwrap().expect("the link")
    }

    fn who(link: &AgentLink) -> LinkPrincipal {
        LinkPrincipal { link: link.clone() }
    }

    /// A chat inside one of the owner's own projects — the `codingarbos` shape.
    fn agent_chat(state: &AppState, project_id: &str, title: &str) -> String {
        let id = ulid();
        state
            .db
            .insert_chat(&Chat {
                id: id.clone(),
                project_id: project_id.to_owned(),
                title: title.to_owned(),
                is_pinned: false,
                position: 0,
                created_at: now_ms(),
                doc_epoch: 0,
                kind: "chat".to_owned(),
                participants: vec![],
                forked_from: None,
            })
            .unwrap();
        id
    }

    /// Put `n` real messages in a chat's room, so a read has something to return.
    fn say(state: &AppState, chat_id: &str, project: Option<&str>, texts: &[&str]) {
        let room = wire::room_name(project.unwrap_or(crate::people::PEOPLE_PROJECT), chat_id);
        let doc = ChatDoc::new(Some(5), false);
        for (i, t) in texts.iter().enumerate() {
            let before = doc.oplog_vv();
            doc.append_user_message(
                &format!("m{i}-{chat_id}"),
                t,
                "dev:phone",
                &UserMessageOpts::default(),
            )
            .unwrap();
            state
                .db
                .append_row(
                    &room,
                    "dev:phone",
                    &format!("b{i}-{chat_id}"),
                    &doc.export_updates_since(&before),
                )
                .unwrap();
        }
    }

    /// The spec's own warning, as a test: Jacob asked about `codingarbos`, which is a **native
    /// agent**, so that conversation is a project chat. An implementation that walked only the
    /// people tables would list every DM and miss the exact chat he named.
    #[tokio::test]
    async fn the_list_holds_both_his_agent_chats_and_his_people_chats() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 6101);
        let (bob, _) = person(&state, 6102);
        let project = project_of(&state, &alice, "codingarbos");
        let with_agent = agent_chat(&state, &project, "Launch week");
        let with_bob = dm(&state, &alice, &bob);

        let link = link_for(&state, &alice, Scope::Read, None);
        let (status, body) = json_of(
            chats(
                State(state.clone()),
                who(&link),
                Query(ChatsQuery::default()),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let ids: Vec<&str> = body["chats"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert!(
            ids.contains(&with_agent.as_str()),
            "the agent conversation is listed: {body}"
        );
        assert!(
            ids.contains(&with_bob.as_str()),
            "the people chat is listed: {body}"
        );

        // And the agent chat says which agent it belongs to, so `?agent=` can narrow to it.
        let (_, narrowed) = json_of(
            chats(
                State(state.clone()),
                who(&link),
                Query(ChatsQuery {
                    agent: Some("codingarbos".into()),
                }),
            )
            .await,
        )
        .await;
        let only = narrowed["chats"].as_array().unwrap();
        assert!(
            only.iter()
                .any(|c| c["id"].as_str() == Some(with_agent.as_str())),
            "the named agent's chat survives the filter: {narrowed}"
        );
        assert!(
            only.iter().all(|c| c["agent"].as_str() == Some(&project)),
            "and nothing that is not that agent's does: {narrowed}"
        );
        assert!(
            !only
                .iter()
                .any(|c| c["id"].as_str() == Some(with_bob.as_str())),
            "the people chat is filtered out: {narrowed}"
        );
    }

    /// A link pinned to named chats reads those and nothing else, and the rest of the account
    /// answers `404` rather than `403`: a restricted link must not be able to prove which other
    /// conversations exist.
    #[tokio::test]
    async fn a_restricted_link_cannot_see_or_probe_the_rest_of_the_account() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 6201);
        let (bob, _) = person(&state, 6202);
        let project = project_of(&state, &alice, "codingarbos");
        let granted = agent_chat(&state, &project, "Granted");
        let withheld = agent_chat(&state, &project, "Withheld");
        let private_dm = dm(&state, &alice, &bob);
        say(&state, &granted, Some(&project), &["hello"]);
        say(&state, &withheld, Some(&project), &["secret"]);

        let link = link_for(
            &state,
            &alice,
            Scope::Read,
            Some(std::slice::from_ref(&granted)),
        );
        let seen = visible_chats(&state, &link);
        assert_eq!(seen.len(), 1, "only the granted chat: {seen:?}");
        assert_eq!(seen[0].id, granted);
        assert!(may_see(&state, &link, &withheld).is_none());
        assert!(may_see(&state, &link, &private_dm).is_none());

        for probe in [&withheld, &private_dm] {
            let (status, body) = json_of(
                messages(
                    State(state.clone()),
                    who(&link),
                    Path(probe.clone()),
                    Query(MessagesQuery::default()),
                    HeaderMap::new(),
                )
                .await,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
            assert_eq!(body["error"]["type"], "not_found", "{body}");
        }

        // The withheld chat's text never reached the link, so nothing was charged for it.
        let (status, ok) = json_of(
            messages(
                State(state.clone()),
                who(&link),
                Path(granted.clone()),
                Query(MessagesQuery::default()),
                HeaderMap::new(),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ok["messages"][0]["text"], "hello", "{ok}");
    }

    /// Another account's chats are not reachable, with or without a chat restriction naming
    /// them: the restriction narrows what the owner granted, it can never widen it.
    #[tokio::test]
    async fn another_account_is_unreachable_even_if_the_link_names_its_chat() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 6301);
        let (bob, _) = person(&state, 6302);
        let (carol, _) = person(&state, 6303);
        let bobs_project = project_of(&state, &bob, "bobsbot");
        let bobs_chat = agent_chat(&state, &bobs_project, "Bob's work");
        let bob_carol = dm(&state, &bob, &carol);
        say(&state, &bobs_chat, Some(&bobs_project), &["bob's text"]);

        let link = link_for(
            &state,
            &alice,
            Scope::Read,
            Some(&[bobs_chat.clone(), bob_carol.clone()]),
        );
        assert!(
            visible_chats(&state, &link).is_empty(),
            "naming another account's chats grants nothing"
        );
        for probe in [&bobs_chat, &bob_carol] {
            let (status, _) =
                json_of(chat(State(state.clone()), who(&link), Path(probe.clone())).await).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
        }
        let (status, _) = json_of(
            agent_log(
                State(state.clone()),
                who(&link),
                Path(bobs_project.clone()),
                Query(LogQuery::default()),
                HeaderMap::new(),
            )
            .await,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "another account's agent has no log here"
        );
    }

    /// The one-link rule, on the one route where it would have been easiest to break. An
    /// attachment is named as a path fetched with the link's own bearer — not as a pre-signed
    /// URL carrying its own credential and its own expiry, which is what the reader-device
    /// export mints. A second credential shape would be a second thing to hold, and it would
    /// outlive a revoke.
    #[tokio::test]
    async fn an_attachment_is_reached_with_the_one_bearer_and_not_a_second_credential() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 6401);
        let project = project_of(&state, &alice, "codingarbos");
        let granted = agent_chat(&state, &project, "Granted");
        let withheld = agent_chat(&state, &project, "Withheld");

        let room = wire::room_name(&project, &granted);
        let doc = ChatDoc::new(Some(5), false);
        let before = doc.oplog_vv();
        doc.append_user_message(
            "with-file",
            "see the log",
            "dev:phone",
            &UserMessageOpts {
                attachments: vec![arbos_core::doc::UserAttachment {
                    name: "log.txt".into(),
                    mime: "text/plain".into(),
                    kind: Some("file".into()),
                    blob_ref: Some("blobkey1".into()),
                    url: None,
                    source: None,
                }],
                ..Default::default()
            },
        )
        .unwrap();
        state
            .db
            .append_row(
                &room,
                "dev:phone",
                "b-file",
                &doc.export_updates_since(&before),
            )
            .unwrap();

        let link = link_for(
            &state,
            &alice,
            Scope::Read,
            Some(std::slice::from_ref(&granted)),
        );
        let (status, body) = json_of(
            messages(
                State(state.clone()),
                who(&link),
                Path(granted.clone()),
                Query(MessagesQuery::default()),
                HeaderMap::new(),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let att = &body["messages"][0]["attachments"][0];
        let path = att["path"].as_str().unwrap_or_default();
        assert_eq!(
            path,
            format!("/agent-link/files/{project}/blobkey1"),
            "{body}"
        );
        let whole = serde_json::to_string(&body).unwrap();
        for leaked in ["sig=", "exp=", "?token=", "Bearer ", "arbl_"] {
            assert!(
                !whole.contains(leaked),
                "no second credential in the body ({leaked}): {whole}"
            );
        }
        assert!(att.get("url").is_none(), "no pre-signed URL: {att}");

        // And the slot is checked against the link's own chats, not merely the account's.
        let r = file(
            State(state.clone()),
            who(&link),
            Path((format!("{PEOPLE_ACL_PREFIX}{withheld}"), "blobkey1".into())),
            Query(crate::avatar_image::SizeQuery::default()),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    /// The message budget, which is the bound on how much of the account a stranger can carry
    /// away. It is summed off the `read` audit rows the reads already write, so the number the
    /// owner sees and the number enforced are the same number.
    #[tokio::test]
    async fn the_message_budget_is_summed_from_the_audit_and_refuses_before_handing_anything_over()
    {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 6501);
        let project = project_of(&state, &alice, "codingarbos");
        let chat_id = agent_chat(&state, &project, "Long");
        say(&state, &chat_id, Some(&project), &["a", "b", "c"]);
        let link = link_for(&state, &alice, Scope::Read, None);

        let (status, body) = json_of(
            messages(
                State(state.clone()),
                who(&link),
                Path(chat_id.clone()),
                Query(MessagesQuery::default()),
                HeaderMap::new(),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["messages"].as_array().unwrap().len(), 3);

        // Charged for what was handed over, not for the 50 the default asked for.
        assert_eq!(state.db.link_messages_read(&link.id, HOUR_MS).unwrap(), 3);
        assert_eq!(
            state.db.user_link_messages_read(&alice, HOUR_MS).unwrap(),
            3
        );

        // Spend the rest of the hour's budget, then the next read is refused outright.
        state
            .db
            .note_link_audit(
                &link.id,
                &alice,
                "read",
                Some(&chat_id),
                None,
                None,
                None,
                &json!({ "messages": READ_MESSAGES_PER_HOUR }),
            )
            .unwrap();
        let (status, refused) = json_of(
            messages(
                State(state.clone()),
                who(&link),
                Path(chat_id.clone()),
                Query(MessagesQuery::default()),
                HeaderMap::new(),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
        assert_eq!(refused["error"]["type"], "rate_limited", "{refused}");
        assert!(
            refused["messages"].is_null(),
            "a refusal hands over nothing: {refused}"
        );
        // The refusal charged nothing, so the owner's activity list is not inflated by it.
        assert_eq!(
            state.db.link_messages_read(&link.id, HOUR_MS).unwrap(),
            3 + READ_MESSAGES_PER_HOUR
        );
    }

    /// Paging walks backwards from the newest message, because an agent asked to debug a
    /// conversation wants its end first.
    #[tokio::test]
    async fn history_pages_backwards_from_the_newest_message() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 6601);
        let project = project_of(&state, &alice, "codingarbos");
        let chat_id = agent_chat(&state, &project, "Long");
        say(&state, &chat_id, Some(&project), &["one", "two", "three"]);
        let link = link_for(&state, &alice, Scope::Read, None);

        let page = |before: Option<String>| {
            let state = state.clone();
            let link = link.clone();
            let chat_id = chat_id.clone();
            async move {
                json_of(
                    messages(
                        State(state),
                        who(&link),
                        Path(chat_id),
                        Query(MessagesQuery {
                            before,
                            limit: Some(2),
                            format: None,
                        }),
                        HeaderMap::new(),
                    )
                    .await,
                )
                .await
            }
        };
        let (_, newest) = page(None).await;
        let texts: Vec<&str> = newest["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["text"].as_str().unwrap())
            .collect();
        assert_eq!(
            texts,
            vec!["two", "three"],
            "newest page, in order: {newest}"
        );
        assert_eq!(newest["has_more"], true);

        let cursor = newest["before"].as_str().unwrap().to_owned();
        let (_, older) = page(Some(cursor)).await;
        let texts: Vec<&str> = older["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, vec!["one"], "{older}");
        assert_eq!(older["has_more"], false);
        assert!(older["before"].is_null());

        // Markdown is the same page, rendered.
        let (status, md) = text_of(
            messages(
                State(state.clone()),
                who(&link),
                Path(chat_id.clone()),
                Query(MessagesQuery {
                    before: None,
                    limit: Some(2),
                    format: Some("md".into()),
                }),
                HeaderMap::new(),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(md.contains("# Long") && md.contains("three"), "{md}");
    }

    /// The debugging window: the same one the owner reads, minus the sandbox facts that help
    /// debug nothing, and with the session log alongside. A `chat=` narrowing still has to be a
    /// chat the link may see, or the log would be a way around `may_see`.
    #[tokio::test]
    async fn the_agent_log_is_the_owners_window_without_the_sandbox_facts() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 6701);
        let project = project_of(&state, &alice, "codingarbos");
        let granted = agent_chat(&state, &project, "Granted");
        let withheld = agent_chat(&state, &project, "Withheld");
        state
            .db
            .insert_asp_log(
                &project,
                &[json!({ "kind": "reply", "at": now_ms(), "ms": { "event_to_reply": 1203 } })],
            )
            .unwrap();

        let link = link_for(
            &state,
            &alice,
            Scope::Read,
            Some(std::slice::from_ref(&granted)),
        );
        let (status, body) = json_of(
            agent_log(
                State(state.clone()),
                who(&link),
                Path("codingarbos".into()),
                Query(LogQuery::default()),
                HeaderMap::new(),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["project_id"], project);
        assert_eq!(body["scope"], "agent_link");
        // The debugging substance is there.
        assert!(body["events"].is_array(), "{body}");
        assert!(body["box"]["reachable"].is_boolean(), "{body}");
        assert_eq!(
            body["session_log"][0]["ms"]["event_to_reply"], 1203,
            "{body}"
        );
        // The owner's sandbox is not.
        for gone in [
            "sandbox_id",
            "provider",
            "snapshot",
            "last_started_at",
            "last_seen_at",
        ] {
            assert!(
                body["box"].get(gone).is_none(),
                "{gone} is not a debugging fact: {body}"
            );
        }

        // A withheld chat cannot be used as the window's narrowing.
        let (status, _) = json_of(
            agent_log(
                State(state.clone()),
                who(&link),
                Path("codingarbos".into()),
                Query(LogQuery {
                    chat: Some(withheld),
                    ..Default::default()
                }),
                HeaderMap::new(),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Reading a log spends no message budget: debugging must not compete with reading the
        // conversation being debugged.
        assert_eq!(state.db.link_messages_read(&link.id, HOUR_MS).unwrap(), 0);
    }

    /// A read-only link reads. There is no scope gate on any route in this module, and this is
    /// the test that says so on purpose rather than by omission.
    #[tokio::test]
    async fn a_read_only_link_may_read_every_route_here() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 6801);
        let project = project_of(&state, &alice, "codingarbos");
        let chat_id = agent_chat(&state, &project, "Granted");
        say(&state, &chat_id, Some(&project), &["hello"]);

        for scope in [Scope::Read, Scope::Reply] {
            let link = link_for(&state, &alice, scope, None);
            for status in [
                chats(
                    State(state.clone()),
                    who(&link),
                    Query(ChatsQuery::default()),
                )
                .await
                .status(),
                chat(State(state.clone()), who(&link), Path(chat_id.clone()))
                    .await
                    .status(),
                messages(
                    State(state.clone()),
                    who(&link),
                    Path(chat_id.clone()),
                    Query(MessagesQuery::default()),
                    HeaderMap::new(),
                )
                .await
                .status(),
                agents(State(state.clone()), who(&link)).await.status(),
            ] {
                assert_eq!(status, StatusCode::OK, "scope {}", scope.as_str());
            }
        }
    }

    /// The agents index answers "which of these is the one I was asked about" in one call, and
    /// lists only agents whose chats the link may actually reach.
    #[tokio::test]
    async fn the_agents_index_lists_only_agents_the_link_can_reach() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 6901);
        let coding = project_of(&state, &alice, "codingarbos");
        let other = project_of(&state, &alice, "otherbot");
        let coding_chat = agent_chat(&state, &coding, "Launch week");
        let _other_chat = agent_chat(&state, &other, "Elsewhere");

        let link = link_for(
            &state,
            &alice,
            Scope::Read,
            Some(std::slice::from_ref(&coding_chat)),
        );
        let (status, body) = json_of(agents(State(state.clone()), who(&link)).await).await;
        assert_eq!(status, StatusCode::OK);
        let listed: Vec<&str> = body["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["handle"].as_str().unwrap())
            .collect();
        assert_eq!(listed, vec!["codingarbos"], "{body}");
        assert_eq!(body["agents"][0]["chats"][0]["id"], coding_chat, "{body}");
        assert!(
            owned_agent(&state, &link, "otherbot").is_none(),
            "an agent whose chats were not granted is not reachable by handle either"
        );
    }
}
