//! `GET /projects/{id}/inspect?since=&chat=&kind=&limit=&format=json|md` — how an agent
//! behaved, for a debugging reader (Jacob's Grok Bot on a full device token, Jacob's own
//! devices, the admin). The relay asks the box (`GET {box_url}/w/{id}/inspect`, the same
//! preview-URL path `GET /projects/{id}/history/{chat}` uses, with the MCP bearer only the
//! relay holds) for the agent's window — turns, tool calls, route and mention-gate decisions,
//! quick replies, approvals, secret requests, its audit tail and warnings, the identity block it
//! runs with — and merges its own rows: metered model usage, voice-call membership, routine
//! runs, the secrets audit. When the box is asleep the relay still answers from what it has
//! (plus the mirrored agentd audit) and says so in `box.reachable`.
//!
//! Scope. The project's owner (any device, the box, the admin) sees everything. Another person
//! sees an agent only through a chat they share with it: `chat=` is required, must be a people
//! chat they are a member of (or a project chat of a project they are a guest of) with the
//! agent in it, and the window is cut to that chat and to member-visible kinds — no tool
//! output, secrets, audit, warnings or box facts. Everyone else: 403.
//!
//! Cursor. Events are `at`-ordered Unix ms; `next_cursor` is the `at` of the last event
//! returned and a page never splits a millisecond, so `since=<next_cursor>` continues exactly.
//! Scrub: the box redacts at write time and scrubs again on the way out; the relay runs
//! `arbos_core::redact::scrub` over every row of its own before it answers.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use arbos_core::ids::now_ms;
use arbos_core::redact::scrub_value;

use crate::auth::Principal;
use crate::db::Project;
use crate::signin::api_error;
use crate::state::AppState;

pub const DEFAULT_WINDOW_MS: i64 = 60 * 60 * 1000;
pub const MAX_WINDOW_MS: i64 = 7 * 24 * 60 * 60 * 1000;
pub const DEFAULT_LIMIT: usize = 200;
pub const MAX_LIMIT: usize = 1000;
const BOX_TIMEOUT: Duration = Duration::from_secs(20);

/// Kinds a chat member (not the owner) may see of someone else's agent.
const MEMBER_KINDS: &[&str] = &[
    "user",
    "assistant",
    "interim",
    "tool_call",
    "route",
    "gate",
    "quick_reply",
    "respond_decision",
    "turn",
    "approval",
    "renamed",
    "feedback",
    "call_agent_joined",
    "call_agent_left",
    "group_send",
    "ask_user",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Owner,
    /// Someone else in a shared chat: that chat only, member-visible kinds only.
    Member {
        chat: String,
    },
}

#[derive(Debug, Clone)]
pub struct InspectQuery {
    pub since: i64,
    pub chat: Option<String>,
    pub kinds: HashSet<String>,
    pub limit: usize,
    pub markdown: bool,
}

impl InspectQuery {
    pub fn parse(q: &HashMap<String, String>) -> Self {
        let now = now_ms();
        let since = q
            .get("since")
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(now - DEFAULT_WINDOW_MS)
            .max(now - MAX_WINDOW_MS);
        Self {
            since,
            chat: q
                .get("chat")
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            kinds: q
                .get("kind")
                .map(|k| {
                    k.split(',')
                        .map(|s| s.trim().to_lowercase())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            limit: q
                .get("limit")
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(DEFAULT_LIMIT)
                .clamp(1, MAX_LIMIT),
            markdown: q.get("format").map(|f| f.trim() == "md").unwrap_or(false),
        }
    }

    /// Same aliases as the box (`arbos-agentd::inspect`), plus `relay` for the relay's own rows.
    fn wants(&self, kind: &str, source: &str) -> bool {
        if self.kinds.is_empty() {
            return true;
        }
        self.kinds.iter().any(|k| match k.as_str() {
            "turns" => matches!(
                kind,
                "user" | "assistant" | "tool_call" | "tool_output" | "route" | "turn" | "interim"
            ),
            "gate" => matches!(kind, "gate" | "respond_decision" | "quick_reply"),
            "secrets" => {
                kind.starts_with("secret") || matches!(kind, "egress" | "browser_fill" | "deny")
            }
            "warnings" => matches!(kind, "warn" | "error"),
            "audit" => source == "audit" || source == "relay_audit",
            "relay" => source.starts_with("relay"),
            k => k == kind,
        })
    }
}

/// Who may see what. `Err` is the 403/404 to answer with.
pub fn scope(
    state: &AppState,
    principal: &Principal,
    project: &Project,
) -> Result<Scope, Box<Response>> {
    if state.owns(principal, &project.id) {
        return Ok(Scope::Owner);
    }
    let Principal::Device { user_id, .. } = principal else {
        return Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "not your agent",
        )));
    };
    Err(Box::new(api_error(
        StatusCode::FORBIDDEN,
        "forbidden",
        format!(
            "not your agent: name a chat you share with it (`chat=`) to see its rows there (user {user_id})"
        ),
    )))
}

/// Member scope for a named chat: a people chat the caller is in with the agent in it, or a
/// project chat of a project the caller is a guest of.
pub fn member_scope(
    state: &AppState,
    principal: &Principal,
    project: &Project,
    chat: &str,
) -> Result<Scope, Box<Response>> {
    let Principal::Device { user_id, .. } = principal else {
        return Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "not your agent",
        )));
    };
    if state.db.people_chat(chat).ok().flatten().is_some() {
        let members = state.db.members_of(chat).unwrap_or_default();
        let me_in = members
            .iter()
            .any(|m| m.user_id() == Some(user_id.as_str()));
        let agent_in = members
            .iter()
            .any(|m| m.project_id() == Some(project.id.as_str()));
        if me_in && agent_in {
            return Ok(Scope::Member {
                chat: chat.to_owned(),
            });
        }
        return Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "you and this agent are not both in that chat",
        )));
    }
    let project_chat = state
        .db
        .chat_project_id(chat)
        .ok()
        .flatten()
        .is_some_and(|p| p == project.id);
    if project_chat && state.allows(principal, &project.id) {
        return Ok(Scope::Member {
            chat: chat.to_owned(),
        });
    }
    Err(Box::new(api_error(
        StatusCode::FORBIDDEN,
        "forbidden",
        "no such shared chat with this agent",
    )))
}

/// The events of one page plus the cursor bookkeeping (see module docs).
fn cut(mut events: Vec<Value>, q: &InspectQuery) -> (Vec<Value>, i64, bool) {
    events.retain(|e| {
        q.wants(
            e["kind"].as_str().unwrap_or(""),
            e["source"].as_str().unwrap_or(""),
        )
    });
    events.sort_by(|a, b| {
        a["at"]
            .as_i64()
            .cmp(&b["at"].as_i64())
            .then_with(|| a["id"].to_string().cmp(&b["id"].to_string()))
    });
    let truncated = events.len() > q.limit;
    if truncated {
        events.truncate(q.limit);
        let last_at = events
            .last()
            .and_then(|e| e["at"].as_i64())
            .unwrap_or(q.since);
        let first_at = events
            .first()
            .and_then(|e| e["at"].as_i64())
            .unwrap_or(q.since);
        if last_at != first_at {
            events.retain(|e| e["at"].as_i64() != Some(last_at));
        }
    }
    let next = events
        .last()
        .and_then(|e| e["at"].as_i64())
        .unwrap_or(q.since);
    (events, next, truncated)
}

fn event(
    id: String,
    at: i64,
    kind: &str,
    chat_id: Option<&str>,
    text: String,
    data: Value,
    source: &str,
) -> Value {
    let mut v = json!({
        "id": id, "at": at, "kind": kind, "chat_id": chat_id, "role": Value::Null,
        "text": text, "data": data, "source": source,
    });
    scrub_value(&mut v);
    v
}

/// The relay's own rows for the window.
fn relay_events(
    state: &AppState,
    project: &Project,
    q: &InspectQuery,
    scope: &Scope,
    box_ok: bool,
) -> Vec<Value> {
    let limit = (q.limit + 1) as i64;
    let mut out = Vec::new();
    if matches!(scope, Scope::Owner) {
        for u in state
            .db
            .usage_rows_after(&project.id, q.since, limit)
            .unwrap_or_default()
        {
            let at = u["at"].as_i64().unwrap_or(0);
            let text = format!(
                "model call {} · {} · in {} out {} · ${:.4} · {} ms",
                u["api"].as_str().unwrap_or("?"),
                u["model"].as_str().unwrap_or("?"),
                u["input"].as_i64().unwrap_or(0),
                u["output"].as_i64().unwrap_or(0),
                u["usd"].as_f64().unwrap_or(0.0),
                u["latency_ms"].as_i64().unwrap_or(0)
            );
            out.push(event(
                format!("usage-{}", u["id"].as_i64().unwrap_or(0)),
                at,
                "usage",
                None,
                text,
                u,
                "relay_usage",
            ));
        }
        let audit_q = crate::db_secrets::AuditQuery {
            project_id: Some(project.id.clone()),
            since: Some(q.since + 1),
            limit: limit.min(500),
            ..Default::default()
        };
        for a in state
            .db
            .list_audit(&project.user_id, &audit_q)
            .unwrap_or_default()
        {
            let text = format!(
                "secret {} · {} {} by {}{}",
                a.action,
                a.target_kind,
                a.target_name,
                a.by,
                a.host
                    .as_deref()
                    .map(|h| format!(" · host {h}"))
                    .unwrap_or_default()
            );
            let chat = a.chat_id.clone();
            out.push(event(
                format!("secret-audit-{}", a.id),
                a.at,
                "secret_audit",
                chat.as_deref(),
                text,
                json!({
                    "action": a.action, "target_kind": a.target_kind, "target_id": a.target_id, "target_name": a.target_name,
                    "request_id": a.request_id, "access_id": a.access_id, "use_kind": a.use_kind, "host": a.host,
                    "by": a.by, "detail": a.detail,
                }),
                "relay_secret_audit",
            ));
        }
        for r in state
            .db
            .routine_run_rows_after(&project.id, q.since, limit)
            .unwrap_or_default()
        {
            let at = r["started_at"].as_i64().unwrap_or(0);
            let chat = r["chat_id"].as_str().map(str::to_owned);
            let text = format!(
                "routine {} · {}{}",
                r["routine"].as_str().unwrap_or("?"),
                r["status"].as_str().unwrap_or("?"),
                r["note"]
                    .as_str()
                    .filter(|n| !n.is_empty())
                    .map(|n| format!(" · {n}"))
                    .unwrap_or_default()
            );
            out.push(event(
                format!("routine-run-{}", r["id"].as_str().unwrap_or("")),
                at,
                "routine_run",
                chat.as_deref(),
                text,
                r,
                "relay_routines",
            ));
        }
    }
    for c in state
        .db
        .call_agent_rows_after(&project.id, q.since, limit)
        .unwrap_or_default()
    {
        let chat = c["chat_id"].as_str().map(str::to_owned);
        let call = c["call_id"].as_str().unwrap_or("").to_owned();
        let joined = c["joined_at"].as_i64().unwrap_or(0);
        out.push(event(
            format!("call-join-{call}"),
            joined,
            "call_agent_joined",
            chat.as_deref(),
            format!(
                "joined call {call} ({}) started by {}",
                c["kind"].as_str().unwrap_or("?"),
                c["started_by"].as_str().unwrap_or("?")
            ),
            c.clone(),
            "relay_calls",
        ));
        if let Some(left) = c["left_at"].as_i64().filter(|l| *l > q.since) {
            out.push(event(
                format!("call-left-{call}"),
                left,
                "call_agent_left",
                chat.as_deref(),
                format!(
                    "left call {call}{}",
                    c["end_reason"]
                        .as_str()
                        .map(|r| format!(" · {r}"))
                        .unwrap_or_default()
                ),
                c,
                "relay_calls",
            ));
        }
    }
    // The box's audit log is authoritative; its mirror stands in only when the box is away.
    if !box_ok && matches!(scope, Scope::Owner) {
        for a in state
            .db
            .audit_rows_after(&project.id, q.since, limit)
            .unwrap_or_default()
        {
            let at = a["at"].as_i64().unwrap_or(0);
            let kind = a["kind"].as_str().unwrap_or("audit").to_owned();
            let chat = a["entry"]["chat_id"].as_str().map(str::to_owned);
            let mut entry = a["entry"].clone();
            if let Some(o) = entry.as_object_mut() {
                o.remove("ts");
                o.remove("kind");
                o.remove("source");
            }
            out.push(event(
                format!("relay-audit-{}", a["id"].as_i64().unwrap_or(0)),
                at,
                &kind,
                chat.as_deref(),
                String::new(),
                entry,
                "relay_audit",
            ));
        }
    }
    if let Some(chat) = &q.chat {
        out.retain(|e| {
            e["chat_id"].as_str() == Some(chat.as_str())
                || e["chat_id"].is_null() && matches!(scope, Scope::Owner)
        });
    }
    out
}

/// Ask the box for its window. `Err(reason)` when it cannot be reached.
async fn box_page(state: &AppState, project: &Project, q: &InspectQuery) -> Result<Value, String> {
    let sb = state
        .db
        .user_box(&project.user_id)
        .ok()
        .flatten()
        .ok_or_else(|| "no box for this user".to_owned())?;
    let url = sb
        .box_url
        .clone()
        .ok_or_else(|| "box has no url yet (asleep or starting)".to_owned())?;
    let mut params: Vec<(&str, String)> = vec![
        ("since", q.since.to_string()),
        ("limit", q.limit.to_string()),
    ];
    if let Some(c) = &q.chat {
        params.push(("chat", c.clone()));
    }
    if !q.kinds.is_empty() {
        let mut k: Vec<&str> = q.kinds.iter().map(String::as_str).collect();
        k.sort_unstable();
        params.push(("kind", k.join(",")));
    }
    let r = crate::boxes::send_to_box(state, &sb, || {
        state
            .http
            .get(format!("{url}/w/{}/inspect", project.id))
            .query(&params)
            .bearer_auth(&sb.mcp_token)
            .timeout(BOX_TIMEOUT)
    })
    .await
    .map_err(|e| format!("box unreachable: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("box answered {}", r.status()));
    }
    r.json::<Value>()
        .await
        .map_err(|e| format!("box answered with bad json: {e}"))
}

fn iso(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|t| t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_else(|| ms.to_string())
}

/// `format=md`: the shape the feedback rollout uses, for a bot's context window.
fn render_markdown(body: &Value) -> String {
    let mut out = String::new();
    let ident = &body["identity"];
    out.push_str(&format!(
        "## Agent {} (@{}) — project `{}`\n\n",
        ident["name"].as_str().unwrap_or("?"),
        ident["handle"].as_str().unwrap_or("?"),
        body["project_id"].as_str().unwrap_or("?")
    ));
    if let Some(former) = ident["former_names"].as_array().filter(|f| !f.is_empty()) {
        out.push_str(&format!(
            "Former names: {}\n\n",
            former
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let b = &body["box"];
    out.push_str(&format!(
        "Box: {} · state {} · agentd {} · image {}\n\n",
        if b["reachable"].as_bool() == Some(true) {
            "reachable"
        } else {
            "not reached"
        },
        b["state"].as_str().unwrap_or("?"),
        b["agentd"]["sha"].as_str().unwrap_or("?"),
        b["agentd"]["image"].as_str().unwrap_or("?")
    ));
    let events = body["events"].as_array().cloned().unwrap_or_default();
    out.push_str(&format!(
        "## Events ({}, oldest first; since {} → next_cursor {})\n\n",
        events.len(),
        iso(body["since"].as_i64().unwrap_or(0)),
        body["next_cursor"].as_i64().unwrap_or(0)
    ));
    let mut prev: Option<i64> = None;
    for e in &events {
        let at = e["at"].as_i64().unwrap_or(0);
        let gap = prev
            .map(|p| format!(" (+{:.1}s)", (at - p).max(0) as f64 / 1000.0))
            .unwrap_or_default();
        prev = Some(at);
        let chat = e["chat_id"]
            .as_str()
            .map(|c| format!(" · chat `{c}`"))
            .unwrap_or_default();
        let text = e["text"].as_str().unwrap_or("");
        let text = if text.is_empty() {
            e["data"].to_string()
        } else {
            text.to_owned()
        };
        out.push_str(&format!(
            "- {}{gap} · {}{chat}:\n  {}\n",
            iso(at),
            e["kind"].as_str().unwrap_or("?"),
            text.replace('\n', "\n  ")
        ));
    }
    if body["truncated"].as_bool() == Some(true) {
        out.push_str("\n_More events follow; continue with `since=next_cursor`._\n");
    }
    out
}

/// `GET /projects/{id}/inspect`.
pub async fn project_inspect(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Query(raw): Query<HashMap<String, String>>,
) -> Response {
    let project = match state.db.project(&id) {
        Ok(Some(p)) => p,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "not_found", "no such project"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let mut q = InspectQuery::parse(&raw);
    let scope = match (scope(&state, &principal, &project), q.chat.as_deref()) {
        (Ok(s), _) => s,
        (Err(_), Some(chat)) => match member_scope(&state, &principal, &project, chat) {
            Ok(s) => s,
            Err(r) => return *r,
        },
        (Err(r), None) => return *r,
    };
    if let Scope::Member { chat } = &scope {
        q.chat = Some(chat.clone());
    }
    let box_result = box_page(&state, &project, &q).await;
    let box_ok = box_result.is_ok();
    let (bs, bs_since) = state.box_state(&project.user_id).await;
    let agentd = state.db.box_agentd(&project.user_id).ok().flatten();
    let sb = state.db.user_box(&project.user_id).ok().flatten();
    let mut box_json = json!({
        "reachable": box_ok,
        "state": format!("{bs:?}").to_lowercase(),
        "state_since": bs_since,
        "agentd": agentd.map(|a| json!({ "sha": a.sha, "version": a.version, "image": a.image, "reported_at": a.reported_at })),
    });
    if matches!(scope, Scope::Owner) {
        let sb = sb.as_ref();
        box_json["sandbox_id"] = json!(sb.and_then(|s| s.sandbox_id.clone()));
        box_json["provider"] = json!(sb.map(|s| s.provider.clone()));
        box_json["snapshot"] = json!(sb.and_then(|s| s.snapshot.clone()));
        box_json["last_started_at"] = json!(sb.and_then(|s| s.last_started_at));
        box_json["last_seen_at"] = json!(sb.and_then(|s| s.last_seen_at));
    }
    let mut events: Vec<Value> = Vec::new();
    let mut identity = Value::Null;
    let mut turn = Value::Null;
    let mut box_cursor: Option<i64> = None;
    match box_result {
        Ok(mut page) => {
            scrub_value(&mut page);
            identity = page["identity"].take();
            turn = page["turn"].take();
            if page["truncated"].as_bool() == Some(true) {
                box_cursor = page["next_cursor"].as_i64();
            }
            if let Some(arr) = page["events"].as_array_mut() {
                events.append(arr);
            }
        }
        Err(reason) => {
            box_json["error"] = json!(reason);
        }
    }
    if identity.is_null() {
        identity = json!({
            "project_id": project.id, "name": project.name,
            "handle": crate::people::agent_slug(&project.name), "live": false,
        });
    }
    let mut relay = relay_events(&state, &project, &q, &scope, box_ok);
    // The box stopped at its own cursor: nothing newer from the relay may come first.
    if let Some(cut_at) = box_cursor {
        relay.retain(|e| e["at"].as_i64().unwrap_or(0) <= cut_at);
    }
    events.append(&mut relay);
    if let Scope::Member { .. } = &scope {
        events.retain(|e| MEMBER_KINDS.contains(&e["kind"].as_str().unwrap_or("")));
        if let Some(o) = identity.as_object_mut() {
            o.retain(|k, _| {
                matches!(
                    k.as_str(),
                    "project_id" | "name" | "handle" | "former_names" | "live"
                )
            });
        }
        turn = json!({ "running": turn["running"] });
    }
    let (events, next_cursor, truncated) = cut(events, &q);
    let body = json!({
        "project_id": project.id,
        "scope": match &scope { Scope::Owner => "owner", Scope::Member { .. } => "member" },
        "chat": q.chat,
        "since": q.since,
        "next_cursor": next_cursor,
        "truncated": truncated,
        "count": events.len(),
        "box": box_json,
        "identity": identity,
        "turn": turn,
        "events": events,
    });
    if q.markdown {
        (
            [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
            render_markdown(&body),
        )
            .into_response()
    } else {
        Json(body).into_response()
    }
}

/// `GET /projects/{id}/jobs/{job_id}?after=<seq>[&chat=<id>]` — the rollout view's document for
/// one background job (duplex §2.3; client contract `duplex-rollout-client-contract.md`): the
/// box builds it from the job's row and the worker's T3 thread (`agentd::jobs_view`), the relay
/// passes it through. The owner may read any job of the agent; a chat member names the chat
/// (`chat=`) and reads only jobs asked in that chat. Polled at 1 s while a rollout is open;
/// `after` bounds the body to newer entries.
pub async fn project_job(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, job_id)): Path<(String, String)>,
    Query(raw): Query<HashMap<String, String>>,
) -> Response {
    let project = match state.db.project(&id) {
        Ok(Some(p)) => p,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "not_found", "no such project"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let scope = match (scope(&state, &principal, &project), raw.get("chat")) {
        (Ok(s), _) => s,
        (Err(_), Some(chat)) => match member_scope(&state, &principal, &project, chat) {
            Ok(s) => s,
            Err(r) => return *r,
        },
        (Err(r), None) => return *r,
    };
    let after = raw
        .get("after")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);
    let sb = match state.db.user_box(&project.user_id) {
        Ok(Some(sb)) => sb,
        Ok(None) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "box_offline",
                "no box for this agent",
            )
        }
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let Some(url) = sb.box_url.clone() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "box_offline",
            "the agent's box is asleep or starting",
        );
    };
    let sent = crate::boxes::send_to_box(&state, &sb, || {
        state
            .http
            .get(format!("{url}/w/{}/jobs/{job_id}", project.id))
            .query(&[("after", after.to_string())])
            .bearer_auth(&sb.mcp_token)
            .timeout(BOX_TIMEOUT)
    })
    .await;
    let r = match sent {
        Ok(r) => r,
        Err(e) => {
            return api_error(
                StatusCode::BAD_GATEWAY,
                "box_unreachable",
                format!("box unreachable: {e}"),
            )
        }
    };
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such job");
    }
    if !r.status().is_success() {
        return api_error(
            StatusCode::BAD_GATEWAY,
            "box_error",
            format!("box answered {}", r.status()),
        );
    }
    let mut doc = match r.json::<Value>().await {
        Ok(v) => v,
        Err(e) => {
            return api_error(
                StatusCode::BAD_GATEWAY,
                "box_error",
                format!("box answered with bad json: {e}"),
            )
        }
    };
    if let Scope::Member { chat } = &scope {
        if doc["chat_id"].as_str() != Some(chat.as_str()) {
            return api_error(
                StatusCode::FORBIDDEN,
                "forbidden",
                "that job was not asked in your chat",
            );
        }
    }
    scrub_value(&mut doc);
    Json(doc).into_response()
}

#[cfg(test)]
mod tests {
    //! Scope (another user's agent → 403; a shared-chat member sees that chat only, member kinds
    //! only; the owner sees secrets and usage), pagination (limit + cursor over relay rows, a
    //! millisecond never split) and scrubbing (a token in an audit row never leaves), all with
    //! the box unreachable (no `box_url`), which is also the asleep-box answer.

    use axum::body::to_bytes;

    use super::*;
    use crate::db_secrets::AuditEntry;
    use crate::people::tests_support::{person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    async fn call(
        state: &AppState,
        who: &Principal,
        project: &str,
        q: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        let raw: HashMap<String, String> = q
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let r = project_inspect(
            State(state.clone()),
            who.clone(),
            Path(project.to_owned()),
            Query(raw),
        )
        .await;
        let status = r.status();
        let bytes = to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, v)
    }

    fn group_with(state: &AppState, owner: &str, guest: &str, project: &str) -> String {
        let chat = crate::db_people::PeopleChat {
            id: arbos_core::ids::ulid(),
            kind: "group".into(),
            created_by: owner.to_owned(),
            created_at: now_ms(),
            title: Some("Arbonites".into()),
            image_key: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            description: None,
        };
        state.db.insert_people_chat(&chat).unwrap();
        state
            .db
            .add_member(
                &chat.id,
                &format!("user:{owner}"),
                "owner",
                owner,
                &json!({}),
            )
            .unwrap();
        state
            .db
            .add_member(
                &chat.id,
                &format!("user:{guest}"),
                "member",
                owner,
                &json!({}),
            )
            .unwrap();
        state
            .db
            .add_member(
                &chat.id,
                &format!("agent:{project}"),
                "agent",
                owner,
                &json!({}),
            )
            .unwrap();
        chat.id
    }

    #[tokio::test]
    async fn other_users_agent_is_forbidden_without_a_shared_chat() {
        let state = test_state(AuthMode::Github);
        let (owner, _) = person(&state, 1);
        let (_, stranger) = person(&state, 2);
        let project = project_of(&state, &owner, "GigChad");
        let (status, body) = call(&state, &stranger, &project, &[]).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["type"], "forbidden");
        // Naming a chat they are not in does not help.
        let (status, _) = call(&state, &stranger, &project, &[("chat", "nope")]).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn owner_sees_everything_member_sees_their_chat_only() {
        let state = test_state(AuthMode::Github);
        let (owner, owner_dev) = person(&state, 1);
        let (guest, guest_dev) = person(&state, 2);
        let project = project_of(&state, &owner, "GigChad");
        let chat = group_with(&state, &owner, &guest, &project);
        let base = now_ms() - 60_000;
        state
            .db
            .record_usage(&project, "chat", "m", 10, 5, 0, 0, 0.01, 120, None)
            .unwrap();
        state
            .db
            .append_secret_audit(
                &owner,
                &AuditEntry {
                    action: "use".into(),
                    target_kind: "secret".into(),
                    target_id: "s1".into(),
                    target_name: "GITHUB".into(),
                    project_id: Some(project.clone()),
                    chat_id: Some(chat.clone()),
                    by: "agent".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        state
            .db
            .record_audit(
                &project,
                "renamed",
                &json!({ "ts": now_ms(), "kind": "renamed", "chat_id": chat, "from": "Home", "to": "GigChad" }),
            )
            .unwrap();

        let (status, body) = call(
            &state,
            &owner_dev,
            &project,
            &[("since", &base.to_string())],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["scope"], "owner");
        assert_eq!(body["box"]["reachable"], false);
        let kinds: Vec<&str> = body["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["kind"].as_str().unwrap())
            .collect();
        assert!(kinds.contains(&"usage"), "{kinds:?}");
        assert!(kinds.contains(&"secret_audit"), "{kinds:?}");
        assert!(
            kinds.contains(&"renamed"),
            "mirrored audit when the box is away: {kinds:?}"
        );
        assert!(body["box"].get("sandbox_id").is_some());

        let (status, body) = call(
            &state,
            &guest_dev,
            &project,
            &[("since", &base.to_string()), ("chat", &chat)],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["scope"], "member");
        assert_eq!(body["chat"], chat);
        let kinds: Vec<&str> = body["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["kind"].as_str().unwrap())
            .collect();
        assert!(!kinds.contains(&"usage"), "{kinds:?}");
        assert!(!kinds.contains(&"secret_audit"), "{kinds:?}");
        assert!(
            body["box"].get("sandbox_id").is_none(),
            "no box facts for a member"
        );
        assert!(body["identity"].get("talking_model").is_none());
        // A member must name the chat.
        let (status, _) = call(&state, &guest_dev, &project, &[]).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn pages_follow_the_cursor_without_splitting_a_millisecond() {
        let state = test_state(AuthMode::Github);
        let (owner, owner_dev) = person(&state, 1);
        let project = project_of(&state, &owner, "Pager");
        let t0 = now_ms() - 30_000;
        // Six audit rows: t0+1, t0+2, t0+2, t0+3, t0+4, t0+5.
        for (i, at) in [1, 2, 2, 3, 4, 5].iter().enumerate() {
            state
                .db
                .record_audit(
                    &project,
                    "egress",
                    &json!({ "ts": t0 + at, "kind": "egress", "n": i }),
                )
                .unwrap();
        }
        let since = t0.to_string();
        let (status, p1) = call(
            &state,
            &owner_dev,
            &project,
            &[("since", &since), ("limit", "2")],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{p1}");
        // Two fit, but the second (t0+2) shares its millisecond with a third: the page stops at t0+1.
        assert_eq!(p1["count"], 1, "{}", p1["events"]);
        assert_eq!(p1["truncated"], true);
        assert_eq!(p1["next_cursor"], t0 + 1);
        let (_, p2) = call(
            &state,
            &owner_dev,
            &project,
            &[("since", &p1["next_cursor"].to_string()), ("limit", "3")],
        )
        .await;
        let ats: Vec<i64> = p2["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["at"].as_i64().unwrap())
            .collect();
        // Three fit, but the page cannot know whether more rows share t0+3: it stops at t0+2.
        assert_eq!(ats, vec![t0 + 2, t0 + 2]);
        assert_eq!(p2["next_cursor"], t0 + 2);
        let (_, p3) = call(
            &state,
            &owner_dev,
            &project,
            &[("since", &p2["next_cursor"].to_string()), ("limit", "10")],
        )
        .await;
        let ats: Vec<i64> = p3["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["at"].as_i64().unwrap())
            .collect();
        assert_eq!(ats, vec![t0 + 3, t0 + 4, t0 + 5]);
        assert_eq!(p3["truncated"], false);
        // `kind=` filters, `format=md` renders.
        let (_, only) = call(
            &state,
            &owner_dev,
            &project,
            &[("since", &since), ("kind", "usage")],
        )
        .await;
        assert_eq!(only["count"], 0);
        let raw: HashMap<String, String> = [("since", since.as_str()), ("format", "md")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let r = project_inspect(
            State(state.clone()),
            owner_dev.clone(),
            Path(project.clone()),
            Query(raw),
        )
        .await;
        assert_eq!(
            r.headers()[header::CONTENT_TYPE],
            "text/markdown; charset=utf-8"
        );
        let md =
            String::from_utf8(to_bytes(r.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
        assert!(md.contains("## Agent Pager"), "{md}");
        assert!(md.contains("· egress"), "{md}");
    }

    #[tokio::test]
    async fn relay_rows_are_scrubbed() {
        let state = test_state(AuthMode::Github);
        let (owner, owner_dev) = person(&state, 1);
        let project = project_of(&state, &owner, "Leaky");
        let t0 = now_ms() - 5_000;
        state
            .db
            .record_audit(
                &project,
                "egress",
                &json!({ "ts": t0 + 1, "kind": "egress", "header": "Authorization: Bearer ghp_abcdefghijklmnopqrstuvwxyz0123",
                         "ref": "op://Arbos/item/credential", "key": "sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789" }),
            )
            .unwrap();
        let (_, body) = call(&state, &owner_dev, &project, &[("since", &t0.to_string())]).await;
        let text = body.to_string();
        assert!(
            !text.contains("ghp_abcdefghijklmnopqrstuvwxyz0123"),
            "{text}"
        );
        assert!(!text.contains("op://Arbos"), "{text}");
        assert!(
            !text.contains("sk-or-v1-abcdefghijklmnopqrstuvwxyz"),
            "{text}"
        );
        assert!(
            text.contains("[REDACTED:bearer]") || text.contains("[REDACTED:token]"),
            "{text}"
        );
        assert!(text.contains("[vault reference]"), "{text}");
    }

    #[test]
    fn query_bounds() {
        let mut raw = HashMap::new();
        raw.insert("since".to_owned(), "5".to_owned());
        raw.insert("limit".to_owned(), "0".to_owned());
        raw.insert("format".to_owned(), "md".to_owned());
        let q = InspectQuery::parse(&raw);
        assert!(q.since >= now_ms() - MAX_WINDOW_MS - 1000);
        assert_eq!(q.limit, 1);
        assert!(q.markdown);
    }
}
