//! The project MCP server (design §6.7): Streamable-HTTP JSON-RPC on loopback.
//! Tools: history_search, history_read, read_thread, chat_messages, chat_read, chat_search,
//! memory_search, memory_read, peek, sub_llm, remember, recall, projects_list, ask_user,
//! spawn_subagent, wake_subagent, connectors_list, connector_call, local_devices,
//! local_run, local_read, local_list, deliver_file, generate_image, send_message, vault_list,
//! vault_request, egress, browser_fill, routines_list, routines_create, routines_update,
//! routines_delete, goals_list, goals_create, goals_update, goals_log, goals_delete,
//! web_search, web_fetch, submit_feedback.
//!
//! Sessions authenticate with a role-bound bearer token (main vs subagent, `Config::
//! mcp_token_for_role`); tools that speak to other Projects are refused to subagents (D69).

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use arbos_core::approval::{ApprovalRequest, Risk};
use arbos_core::widget::WIDGET_SCHEMA_JSON;

use crate::agent::Agent;
use crate::approval::WaitMode;

type S = Arc<Agent>;

const MAX_HITS: i64 = 20;
const PAGE_TOKENS_CHARS: usize = 32_000; // ≈ 8k tokens
const DRAFT_CHANNELS: &[&str] = &["email", "slack", "telegram", "whatsapp", "sms"];

fn tools() -> Value {
    let mut list = json!([
        { "name": "context_search", "description": "Search this agent's memory tree: raw events (every past message, tool call and result) plus index cards of episodes and threads. Returns ids for context_open / context_around. Scoped to what this chat may see.",
          "inputSchema": { "type": "object", "properties": { "query": { "type": "string" }, "limit": { "type": "integer" } }, "required": ["query"] } },
        { "name": "context_open", "description": "Open a memory-tree node: a thread or project returns its child cards; an episode returns its raw events verbatim; an event id returns the full text (up to 12k chars).",
          "inputSchema": { "type": "object", "properties": { "node_id": { "type": "string" } }, "required": ["node_id"] } },
        { "name": "context_children", "description": "Alias of context_open for thread/episode nodes.",
          "inputSchema": { "type": "object", "properties": { "node_id": { "type": "string" } }, "required": ["node_id"] } },
        { "name": "context_around", "description": "The raw events just before and after one event in the same thread.",
          "inputSchema": { "type": "object", "properties": { "leaf_id": { "type": "string" }, "before": { "type": "integer" }, "after": { "type": "integer" } }, "required": ["leaf_id"] } },
        { "name": "history_search", "description": "Search this Project's whole history (chats, subagent sessions, tool outputs). Ranked hits with snippets; every hit id is readable with history_read.",
          "inputSchema": { "type": "object", "properties": { "query": { "type": "string" }, "thread_id": { "type": "string" }, "kind": { "type": "string", "enum": ["user","assistant","tool_call","tool_output","compaction","memory","session_entry"] }, "limit": { "type": "integer", "maximum": 20 }, "cursor": { "type": "string" } }, "required": ["query"] } },
        { "name": "history_read", "description": "Read one history entry verbatim by id, or a range {thread_id, from_id, to_id}. Big entries return a handle for peek.",
          "inputSchema": { "type": "object", "properties": { "id": { "type": "string" }, "range": { "type": "object", "properties": { "thread_id": { "type": "string" }, "from_id": { "type": "string" }, "to_id": { "type": "string" } } }, "cursor": { "type": "string" } } } },
        { "name": "read_thread", "description": "Verbatim turns of one chat or subagent thread (default: last 20).",
          "inputSchema": { "type": "object", "properties": { "thread_id": { "type": "string" }, "from": { "type": "integer" }, "to": { "type": "integer" }, "cursor": { "type": "string" } }, "required": ["thread_id"] } },
        { "name": "chat_messages", "description": "Read chat messages verbatim from the live chat doc (DMs, people groups, project chats): one message in full by message_id, or a page of history (newest last; `before`/`after` a message id; default the current chat, 20 messages, max 50). Use it whenever a message in your context carries a [truncated …] or [N earlier messages not shown …] marker.",
          "inputSchema": { "type": "object", "properties": { "chat_id": { "type": "string" }, "message_id": { "type": "string" }, "before": { "type": "string" }, "after": { "type": "string" }, "limit": { "type": "integer", "maximum": 50 } } } },
        { "name": "memory_search", "description": "Search Project memory (T0 user core, T1 project core, T2 thread memory). Bi-temporal fields included.",
          "inputSchema": { "type": "object", "properties": { "query": { "type": "string" }, "tiers": { "type": "array", "items": { "type": "string" } }, "include_expired": { "type": "boolean" }, "limit": { "type": "integer", "maximum": 20 } }, "required": ["query"] } },
        { "name": "memory_read", "description": "Read a thread's T2 memory, or memory records by id.",
          "inputSchema": { "type": "object", "properties": { "thread_id": { "type": "string" }, "ids": { "type": "array", "items": { "type": "string" } } } } },
        { "name": "peek", "description": "Read a slice of an offloaded output by handle: lines [from,to], chars [from,to], or grep pattern; max 200 lines.",
          "inputSchema": { "type": "object", "properties": { "handle": { "type": "string" }, "lines": { "type": "array", "items": { "type": "integer" }, "minItems": 2, "maxItems": 2 }, "chars": { "type": "array", "items": { "type": "integer" }, "minItems": 2, "maxItems": 2 }, "grep": { "type": "string" }, "max": { "type": "integer", "maximum": 200 } }, "required": ["handle"] } },
        { "name": "sub_llm", "description": "Run a cheap model over a handle or text with an instruction (map-reduce in 60k-char chunks). Returns <= 2,000 chars inline; the full result becomes a new handle.",
          "inputSchema": { "type": "object", "properties": { "handle": { "type": "string" }, "text": { "type": "string" }, "instruction": { "type": "string" }, "chunk": { "type": "integer" } }, "required": ["instruction"] } },
        { "name": "remember", "description": "Save a short named note in this chat's working set (a proposal to project memory). value null forgets it.",
          "inputSchema": { "type": "object", "properties": { "name": { "type": "string" }, "value": { "type": ["string","null"] } }, "required": ["name"] } },
        { "name": "recall", "description": "Read a note saved with remember.", "inputSchema": { "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] } },
        { "name": "projects_list", "description": "List the user's other Projects (name, state).", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "send_message", "description": "Message another agent. `to` = its name, @handle or id (your owner's agents; another owner's only when they allowed it). The text lands in that agent's own chat and its reply comes back to you here — never write `@name …` as a bubble in this chat to reach them. Inside a group chat, omit `to` to speak to the other agents in the group. Without `to`, the progress-note `send_message(text)` is for the user in this chat only.",
          "inputSchema": { "type": "object", "properties": { "to": { "type": "string", "description": "Agent name, @handle or id" }, "text": { "type": "string" } }, "required": ["text"] } },
        { "name": "ask_user", "description": "Ask the user one question and wait; the answer arrives as the next message. Pass `widget` for a typed answer (choice, multi_select, text, date, form).",
          "inputSchema": { "type": "object", "properties": { "question": { "type": "string" }, "widget": widget_schema() }, "required": ["question"] } },
        { "name": "spawn_subagent", "description": "Hand a task to a new subagent (Prime by default). Prefer for long or research-heavy work.",
          "inputSchema": { "type": "object", "properties": { "name": { "type": "string" }, "purpose": { "type": "string" }, "brief": { "type": "string" }, "isolate": { "type": "boolean" } }, "required": ["name","purpose","brief"] } },
        { "name": "wake_subagent", "description": "Continue an existing subagent with a new brief.", "inputSchema": { "type": "object", "properties": { "id": { "type": "string" }, "brief": { "type": "string" } }, "required": ["id","brief"] } },
        { "name": "skills_list", "description": "List the skills library (reusable how-tos shared across Projects): slug, name, description, when to use. Optional q filters by keyword.",
          "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } } },
        { "name": "skills_read", "description": "Read one skill's full Markdown by slug.", "inputSchema": { "type": "object", "properties": { "slug": { "type": "string" } }, "required": ["slug"] } },
        { "name": "skills_save", "description": "Save a how-to you learned as a skill (upsert by slug; this Project owns it). shared=true makes it visible to every Project.",
          "inputSchema": { "type": "object", "properties": { "slug": { "type": "string" }, "name": { "type": "string" }, "description": { "type": "string" }, "tags": { "type": "array", "items": { "type": "string" } }, "when_to_use": { "type": "string" }, "body": { "type": "string" }, "shared": { "type": "boolean" } }, "required": ["name","body"] } },
        { "name": "draft_message", "description": "Draft an outbound message (email, Slack, Telegram, WhatsApp, SMS) as an approval card. Nothing is sent until the user approves; then stop and end the turn. Call once per draft.",
          "inputSchema": { "type": "object", "properties": { "channel": { "type": "string", "enum": ["email","slack","telegram","whatsapp","sms"] }, "to": { "type": "string" }, "subject": { "type": "string" }, "body": { "type": "string" } }, "required": ["channel","to","body"] } }
    ]);
    if let Some(a) = list.as_array_mut() {
        a.extend(crate::connectors::tool_specs());
        a.extend(crate::local::tool_specs());
        a.extend(crate::media::tool_specs());
        a.extend(crate::vault::tool_specs());
        a.extend(crate::routines::tool_specs());
        a.extend(crate::goals::tool_specs());
        a.extend(crate::web::tool_specs());
        a.extend(crate::feedback::tool_specs());
        a.extend(crate::disk::tool_specs());
        a.extend(crate::chat_tools::tool_specs());
    }
    list
}

fn widget_schema() -> Value {
    serde_json::from_str(WIDGET_SCHEMA_JSON).unwrap_or_else(|_| json!({ "type": "object" }))
}

/// Who is calling the MCP server, derived from the bearer token it presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRole {
    Main,
    Subagent,
}

/// Tools only the main agent may use: anything that talks to another agent (D69 — subagents
/// never reach the cross-project surface). `group_send` / `projects_send` are the old names,
/// still accepted from older sessions.
const MAIN_ONLY_TOOLS: &[&str] = &["send_message", "group_send", "projects_send"];

/// `Some(reason)` when `role` may not call `tool`.
pub fn refusal_for(role: SessionRole, tool: &str) -> Option<String> {
    match role {
        SessionRole::Main => None,
        SessionRole::Subagent => MAIN_ONLY_TOOLS
            .contains(&tool)
            .then(|| format!("{tool} is available to the main agent only; report back and let it speak for the Project")),
    }
}

pub(crate) fn text_result(text: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }] })
}

pub(crate) fn err_result(msg: impl std::fmt::Display) -> Value {
    json!({ "content": [{ "type": "text", "text": format!("error: {msg}") }], "isError": true })
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_owned()
    } else {
        format!("{}…[{} more chars]", &s[..max], s.len() - max)
    }
}

async fn call_tool(a: &Arc<Agent>, name: &str, args: &Value) -> Value {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_owned);
    let chat_for_scratch = a
        .turn
        .lock()
        .await
        .as_ref()
        .map(|t| t.chat_id.clone())
        .unwrap_or_else(|| "shared".into());
    let _ = a.store.log_decision(
        "pull",
        None,
        None,
        Some(name),
        &[],
        Some(&clip(&args.to_string(), 200)),
        None,
        None,
        None,
    );
    // Risky actions run only after an approved card (design S18, `arbos_core::approval`).
    // `connector_call` is gated inside `connectors::connector_call`, where the catalog kind is
    // known, so it is classified exactly once and gets one card. `local_run/read/list` are gated
    // inside `local::local_call`, which knows the device and its "allow reads" switch.
    // `vault_request` / `browser_fill` post their own card from `vault::obtain_grant`, carrying the
    // relay request id the client forwards the decision to.
    if !matches!(
        name,
        "connector_call"
            | "local_run"
            | "local_read"
            | "local_list"
            | "vault_request"
            | "secrets_request"
            | "request_secret"
            | "secrets_save_from_message"
            | "browser_fill"
    ) {
        if let Err(refusal) = a.gate_tool(name, args, &chat_for_scratch).await {
            return err_result(format!("not executed: {refusal}"));
        }
    }
    match name {
        "context_search" => {
            let q = s("query").unwrap_or_default();
            let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(10);
            let restricted = a.tree_restricted(&chat_for_scratch);
            text_result(
                a.tree
                    .tool_search(&chat_for_scratch, restricted, &q, limit)
                    .to_string(),
            )
        }
        "context_open" | "context_children" => {
            let id = s("node_id").unwrap_or_default();
            let restricted = a.tree_restricted(&chat_for_scratch);
            text_result(
                a.tree
                    .tool_open(&chat_for_scratch, restricted, &id)
                    .to_string(),
            )
        }
        "context_around" => {
            let id = s("leaf_id").unwrap_or_default();
            let restricted = a.tree_restricted(&chat_for_scratch);
            let before = args.get("before").and_then(Value::as_i64).unwrap_or(4);
            let after = args.get("after").and_then(Value::as_i64).unwrap_or(4);
            text_result(
                a.tree
                    .tool_around(&chat_for_scratch, restricted, &id, before, after)
                    .to_string(),
            )
        }
        "history_search" => {
            let q = s("query").unwrap_or_default();
            let limit = args
                .get("limit")
                .and_then(Value::as_i64)
                .unwrap_or(10)
                .clamp(1, MAX_HITS);
            let offset: i64 = s("cursor").and_then(|c| c.parse().ok()).unwrap_or(0);
            match a.store.t3_search(
                &q,
                s("thread_id").as_deref(),
                s("kind").as_deref(),
                limit,
                offset,
            ) {
                Ok(hits) => {
                    let n = hits.len() as i64;
                    let items: Vec<Value> = hits
                        .into_iter()
                        .map(|(r, score, snip)| json!({ "id": r.id, "thread_id": r.thread_id, "subagent_id": r.subagent_id, "kind": r.kind, "at": r.at, "score": -score, "snippet": clip(&snip, 300) }))
                        .collect();
                    let next = if n == limit {
                        Some((offset + n).to_string())
                    } else {
                        None
                    };
                    text_result(json!({ "hits": items, "next_cursor": next }).to_string())
                }
                Err(e) => err_result(e),
            }
        }
        "history_read" => {
            if let Some(id) = s("id") {
                match a.store.t3_get(&id) {
                    Ok(Some(r)) => {
                        if r.text.len() > PAGE_TOKENS_CHARS {
                            let h = materialize_handle(
                                a,
                                &chat_for_scratch,
                                &format!("history {}", r.id),
                                &r.text,
                            );
                            text_result(json!({ "id": r.id, "kind": r.kind, "at": r.at, "text": clip(&r.text, 8000), "handle": h }).to_string())
                        } else {
                            text_result(json!({ "id": r.id, "thread_id": r.thread_id, "kind": r.kind, "role": r.role, "at": r.at, "text": r.text }).to_string())
                        }
                    }
                    Ok(None) => err_result("no such entry"),
                    Err(e) => err_result(e),
                }
            } else if let Some(range) = args.get("range") {
                let tid = range.get("thread_id").and_then(Value::as_str).unwrap_or("");
                match a.store.t3_thread(
                    tid,
                    range.get("from_id").and_then(Value::as_str),
                    range.get("to_id").and_then(Value::as_str),
                    200,
                ) {
                    Ok(rows) => text_result(page_rows(&rows)),
                    Err(e) => err_result(e),
                }
            } else {
                err_result("id or range required")
            }
        }
        "chat_messages" => {
            let chat_id = s("chat_id")
                .filter(|c| !c.trim().is_empty())
                .unwrap_or_else(|| chat_for_scratch.clone());
            let Some(host) = a.sync.host(&chat_id).await else {
                return err_result(format!("no open chat {chat_id} on this box"));
            };
            let all = { host.lock().await.doc.messages() };
            let row = |m: &arbos_core::doc::MessageView, full: bool| -> Value {
                let text = m.text();
                let text = if full || text.chars().count() <= crate::truncate::FULL_MSG_CHARS {
                    text
                } else {
                    crate::truncate::clip_marked(
                        &text,
                        crate::truncate::FULL_MSG_CHARS,
                        &m.id,
                        Some(&crate::truncate::fetch_message_hint(&chat_id, &m.id)),
                    )
                };
                let from = m
                    .rest
                    .get("senderName")
                    .and_then(Value::as_str)
                    .or_else(|| m.rest.get("senderHandle").and_then(Value::as_str))
                    .or_else(|| m.rest.get("sender").and_then(Value::as_str))
                    .unwrap_or(if m.role == "assistant" {
                        "agent"
                    } else {
                        "user"
                    });
                json!({
                    "id": m.id, "role": m.role, "from": from, "status": m.status,
                    "at": m.rest.get("createdAt"), "reply_to": m.rest.get("replyTo"),
                    "chars": text.chars().count(), "text": text,
                })
            };
            if let Some(mid) = s("message_id").filter(|x| !x.trim().is_empty()) {
                return match all.iter().find(|m| m.id == mid) {
                    Some(m) => {
                        let text = m.text();
                        if text.len() > PAGE_TOKENS_CHARS {
                            let h = materialize_handle(
                                a,
                                &chat_for_scratch,
                                &format!("chat message {mid}"),
                                &text,
                            );
                            let mut v = row(m, false);
                            v["handle"] = json!(h);
                            v["note"] = json!("longer than one page: `text` is the head; read the rest with peek on `handle`");
                            text_result(v.to_string())
                        } else {
                            text_result(row(m, true).to_string())
                        }
                    }
                    None => err_result(format!("no message {mid} in chat {chat_id}")),
                };
            }
            let limit = args
                .get("limit")
                .and_then(Value::as_i64)
                .unwrap_or(20)
                .clamp(1, 50) as usize;
            let visible: Vec<&arbos_core::doc::MessageView> = all
                .iter()
                .filter(|m| m.role != "system" && !m.text().trim().is_empty())
                .collect();
            let (slice, older): (Vec<&arbos_core::doc::MessageView>, bool) =
                if let Some(after) = s("after").filter(|x| !x.trim().is_empty()) {
                    let start = visible
                        .iter()
                        .position(|m| m.id == after)
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    (
                        visible[start..].iter().take(limit).copied().collect(),
                        start > 0,
                    )
                } else {
                    let end = match s("before").filter(|x| !x.trim().is_empty()) {
                        Some(b) => visible
                            .iter()
                            .position(|m| m.id == b)
                            .unwrap_or(visible.len()),
                        None => visible.len(),
                    };
                    let start = end.saturating_sub(limit);
                    (visible[start..end].to_vec(), start > 0)
                };
            let mut budget = PAGE_TOKENS_CHARS;
            let mut rows = Vec::new();
            for m in &slice {
                let mut v = row(m, false);
                let n = m.text().chars().count();
                if n > 4000 && !rows.is_empty() {
                    v["text"] = json!(crate::truncate::clip_marked(
                        &m.text(),
                        4000,
                        &m.id,
                        Some(&crate::truncate::fetch_message_hint(&chat_id, &m.id))
                    ));
                }
                let cost = v["text"].as_str().map(str::len).unwrap_or(0);
                if cost > budget && !rows.is_empty() {
                    break;
                }
                budget = budget.saturating_sub(cost);
                rows.push(v);
            }
            let oldest = rows
                .first()
                .and_then(|r| r["id"].as_str())
                .unwrap_or("")
                .to_owned();
            let newest = rows
                .last()
                .and_then(|r| r["id"].as_str())
                .unwrap_or("")
                .to_owned();
            text_result(
                json!({
                    "chat_id": chat_id,
                    "messages": rows,
                    "older_before": if older || rows.len() < slice.len() { Some(&oldest) } else { None },
                    "newer_after": if newest.is_empty() { None } else { Some(&newest) },
                    "total_in_chat": visible.len(),
                })
                .to_string(),
            )
        }
        "read_thread" => {
            let tid = s("thread_id").unwrap_or_default();
            let limit = args
                .get("to")
                .and_then(Value::as_i64)
                .map(|t| t - args.get("from").and_then(Value::as_i64).unwrap_or(0))
                .unwrap_or(20)
                .clamp(1, 200);
            match a.store.t3_thread(&tid, None, None, limit) {
                Ok(rows) => text_result(page_rows(&rows)),
                Err(e) => err_result(e),
            }
        }
        "memory_search" => {
            let q = s("query").unwrap_or_default();
            let tiers: Vec<String> = args
                .get("tiers")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let inc = args
                .get("include_expired")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let limit = args
                .get("limit")
                .and_then(Value::as_i64)
                .unwrap_or(10)
                .clamp(1, MAX_HITS);
            match a.store.memory_search(&q, &tiers, inc, limit) {
                Ok(hits) => {
                    let ids: Vec<String> = hits.iter().map(|(m, _)| m.id.clone()).collect();
                    let _ = a.store.memory_touch(&ids);
                    text_result(
                        json!(hits
                            .into_iter()
                            .map(|(m, sc)| json!({ "record": m, "score": -sc }))
                            .collect::<Vec<_>>())
                        .to_string(),
                    )
                }
                Err(e) => err_result(e),
            }
        }
        "memory_read" => {
            if let Some(tid) = s("thread_id") {
                let t2 = a
                    .store
                    .memory_live("T2", Some(&tid), 40)
                    .unwrap_or_default();
                let sub = a.store.subagent(&tid).ok().flatten();
                text_result(json!({ "thread": sub, "t2": t2 }).to_string())
            } else {
                let ids: Vec<String> = args
                    .get("ids")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                match a.store.memory_by_ids(&ids) {
                    Ok(rows) => text_result(json!(rows).to_string()),
                    Err(e) => err_result(e),
                }
            }
        }
        "peek" => {
            let id = s("handle").unwrap_or_default();
            let Ok(Some(h)) = a.store.handle(&id) else {
                return err_result("unknown handle");
            };
            let Ok(full) = std::fs::read_to_string(&h.path) else {
                return err_result("handle file missing");
            };
            let max = args
                .get("max")
                .and_then(Value::as_i64)
                .unwrap_or(200)
                .clamp(1, 200) as usize;
            let out = if let Some(pat) = s("grep") {
                match regex_lite::Regex::new(&pat) {
                    Ok(re) => full
                        .lines()
                        .enumerate()
                        .filter(|(_, l)| re.is_match(l))
                        .take(max)
                        .map(|(i, l)| format!("{}: {l}", i + 1))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    Err(e) => return err_result(e),
                }
            } else if let Some(c) = args.get("chars").and_then(Value::as_array) {
                let from = c.first().and_then(Value::as_u64).unwrap_or(0) as usize;
                let to = c.get(1).and_then(Value::as_u64).unwrap_or(0) as usize;
                full.chars()
                    .skip(from)
                    .take(to.saturating_sub(from))
                    .collect()
            } else {
                let (from, to) = args
                    .get("lines")
                    .and_then(Value::as_array)
                    .map(|l| {
                        (
                            l.first().and_then(Value::as_i64).unwrap_or(1).max(1) as usize,
                            l.get(1)
                                .and_then(Value::as_i64)
                                .unwrap_or(max as i64)
                                .max(1) as usize,
                        )
                    })
                    .unwrap_or((1, max));
                full.lines()
                    .enumerate()
                    .skip(from - 1)
                    .take((to + 1 - from).min(max))
                    .map(|(i, l)| format!("{}: {l}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            text_result(clip(&out, 16_000))
        }
        "sub_llm" => {
            let instruction = s("instruction").unwrap_or_default();
            let text = if let Some(h) = s("handle") {
                match a
                    .store
                    .handle(&h)
                    .ok()
                    .flatten()
                    .and_then(|h| std::fs::read_to_string(h.path).ok())
                {
                    Some(t) => t,
                    None => return err_result("unknown handle"),
                }
            } else {
                s("text").unwrap_or_default()
            };
            let chunk = args
                .get("chunk")
                .and_then(Value::as_u64)
                .unwrap_or(60_000)
                .clamp(2_000, 120_000) as usize;
            let chunks: Vec<String> = text
                .as_bytes()
                .chunks(chunk)
                .map(|c| String::from_utf8_lossy(c).into_owned())
                .collect();
            let mut parts = Vec::new();
            for (i, c) in chunks.iter().enumerate().take(32) {
                match a
                    .llm
                    .complete(&a.cfg.talking_model, &instruction, c, 1500)
                    .await
                {
                    Ok((t, _)) => parts.push(format!("[chunk {}]\n{t}", i + 1)),
                    Err(e) => return err_result(e),
                }
            }
            let mut joined = parts.join("\n\n");
            if parts.len() > 1 && joined.len() > 8000 {
                if let Ok((t, _)) = a
                    .llm
                    .complete(
                        &a.cfg.talking_model,
                        &format!(
                            "Reduce these per-chunk results into one answer for: {instruction}"
                        ),
                        &joined,
                        1500,
                    )
                    .await
                {
                    joined = t;
                }
            }
            let handle = materialize_handle(a, &chat_for_scratch, "sub_llm result", &joined);
            text_result(json!({ "result": clip(&joined, 2000), "handle": handle }).to_string())
        }
        "remember" => {
            let n = s("name").unwrap_or_default();
            match a
                .store
                .scratch_set(&chat_for_scratch, &n, s("value").as_deref())
            {
                Ok(()) => text_result(format!("remembered {n}")),
                Err(e) => err_result(e),
            }
        }
        "recall" => {
            let n = s("name").unwrap_or_default();
            text_result(
                a.store
                    .scratch_get(&chat_for_scratch, &n)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| format!("(no note named {n})")),
            )
        }
        "projects_list" => match reqwest::Client::new()
            .get(format!("{}/projects", a.cfg.relay_url))
            .bearer_auth(&a.cfg.box_token)
            .send()
            .await
        {
            Ok(r) => {
                let status = r.status();
                let text = r.text().await.unwrap_or_default();
                if status.is_success() {
                    text_result(text)
                } else {
                    err_result(crate::relay_error::message(&text))
                }
            }
            Err(e) => err_result(e),
        },
        "send_message" | "group_send" | "projects_send" => {
            let text = s("text").unwrap_or_default();
            let to = s("to")
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty());
            match a.send_message_from_turn(to.as_deref(), &text).await {
                Ok(t) => text_result(t),
                Err(e) => err_result(e),
            }
        }
        "ask_user" => {
            let q = s("question").unwrap_or_default();
            match a
                .hook_ask_user(&q, args.get("widget"), &chat_for_scratch)
                .await
            {
                Ok(_) => text_result(
                    "Question posted. Stop here; the user's answer arrives as the next message.",
                ),
                Err(e) => err_result(e),
            }
        }
        "spawn_subagent" => match a
            .spawn_from_tool(
                None,
                &s("name").unwrap_or_default(),
                &s("purpose").unwrap_or_default(),
                &s("brief").unwrap_or_default(),
                args.get("isolate")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )
            .await
        {
            Ok(t) => text_result(t),
            Err(e) => err_result(e),
        },
        "wake_subagent" => match a
            .spawn_from_tool(s("id"), "", "", &s("brief").unwrap_or_default(), false)
            .await
        {
            Ok(t) => text_result(t),
            Err(e) => err_result(e),
        },
        "connectors_list" => crate::connectors::connectors_list(a).await,
        "connector_call" => crate::connectors::connector_call(a, args, &chat_for_scratch).await,
        "skills_list" => {
            if a.skills.is_stale() {
                a.refresh_skills().await;
            }
            let q = s("q").unwrap_or_default();
            let all = a.skills.snapshot();
            let items: Vec<Value> = all
                .iter()
                .filter(|sk| {
                    q.trim().is_empty() || {
                        let hay = sk.match_text().to_lowercase();
                        q.to_lowercase()
                            .split_whitespace()
                            .any(|w| hay.contains(w))
                    }
                })
                .map(|sk| json!({ "slug": sk.slug, "name": sk.name, "description": sk.description, "tags": sk.tags, "when_to_use": sk.when_to_use, "shared": sk.shared, "version": sk.version }))
                .collect();
            text_result(json!({ "skills": items }).to_string())
        }
        "skills_read" => {
            let slug = s("slug").unwrap_or_default();
            if a.skills.find(&slug).is_none() {
                a.refresh_skills().await;
            }
            match a.skills.find(&slug) {
                Some(sk) => text_result(sk.to_markdown()),
                None => err_result(format!("no skill {slug}")),
            }
        }
        "skills_save" => {
            let name = s("name").unwrap_or_default();
            let body = s("body").unwrap_or_default();
            if name.trim().is_empty() || body.trim().is_empty() {
                return err_result("name and body are required");
            }
            let slug = s("slug")
                .map(|x| arbos_core::skill::slugify(&x))
                .unwrap_or_else(|| arbos_core::skill::slugify(&name));
            let payload = json!({
                "slug": slug, "name": name, "description": s("description").unwrap_or_default(),
                "tags": args.get("tags").cloned().unwrap_or(json!([])),
                "when_to_use": s("when_to_use").unwrap_or_default(), "body": body,
                "shared": args.get("shared").and_then(Value::as_bool).unwrap_or(false),
            });
            match reqwest::Client::new()
                .post(format!("{}/skills", a.cfg.relay_url))
                .header("x-arbos-project", &a.cfg.project_id)
                .bearer_auth(&a.cfg.box_token)
                .json(&payload)
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => {
                    let v: Value = r.json().await.unwrap_or_default();
                    let version = v.get("version").and_then(Value::as_i64).unwrap_or(1);
                    audit_line(
                        a,
                        json!({ "kind": "skills_save", "slug": slug, "version": version, "chat_id": chat_for_scratch, "at": arbos_core::ids::now_ms() }),
                    );
                    let _ = a.store.t3_append(
                        &chat_for_scratch,
                        None,
                        Some(&chat_for_scratch),
                        "skills",
                        Some("assistant"),
                        &format!("skills_save {slug} v{version}: {name}"),
                        None,
                    );
                    a.refresh_skills().await;
                    text_result(
                        json!({ "slug": slug, "version": version, "status": "saved" }).to_string(),
                    )
                }
                Ok(r) => {
                    let status = r.status();
                    let text = r.text().await.unwrap_or_default();
                    err_result(format!(
                        "relay refused: HTTP {} {}",
                        status,
                        crate::relay_error::message(&text)
                    ))
                }
                Err(e) => err_result(e),
            }
        }
        "draft_message" => {
            let channel = s("channel").unwrap_or_default();
            if !DRAFT_CHANNELS.contains(&channel.as_str()) {
                return err_result(format!(
                    "channel must be one of {}",
                    DRAFT_CHANNELS.join(", ")
                ));
            }
            let to = s("to").unwrap_or_default();
            let body = s("body").unwrap_or_default();
            if to.trim().is_empty() || body.trim().is_empty() {
                return err_result("to and body are required");
            }
            let subject = s("subject").filter(|x| !x.trim().is_empty());
            let title = match &subject {
                Some(subj) => format!("Send {channel} to {to}: {subj}"),
                None => format!("Send {channel} to {to}"),
            };
            let summary: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
            let summary = clip(&summary, 160);
            let req = ApprovalRequest {
                title,
                summary,
                risk: Risk::Send,
                payload: json!({ "kind": "draft_message", "channel": channel, "to": to, "subject": subject, "body": body }),
            };
            match a
                .begin_approval(&chat_for_scratch, req, WaitMode::Deferred, None)
                .await
            {
                Ok(id) => text_result(format!(
                    "Draft posted as approval card {id}. Nothing was sent. Stop here: tell the user in one sentence to approve or reject the card above, then end the turn. Arbos will tell you the outcome in a later message."
                )),
                Err(e) => err_result(e),
            }
        }
        "local_devices" => crate::local::local_devices(a).await,
        "local_run" | "local_read" | "local_list" => {
            crate::local::local_call(a, name, args, &chat_for_scratch).await
        }
        "deliver_file" => crate::media::deliver_file(a, args).await,
        "generate_image" => crate::media::generate_image(a, args).await,
        "vault_list" | "secrets_list" => crate::vault::secrets_list(a).await,
        "vault_request" | "secrets_request" => {
            crate::vault::secrets_request(a, args, &chat_for_scratch).await
        }
        "egress" => crate::vault::egress(a, args, &chat_for_scratch).await,
        "request_secret" => crate::handoff::request_secret(a, args, &chat_for_scratch).await,
        "secrets_save_from_message" => {
            crate::handoff::secrets_save_from_message(a, args, &chat_for_scratch).await
        }
        "web_search" => crate::web::web_search(a, args, &chat_for_scratch).await,
        "web_fetch" => crate::web::web_fetch(a, args, &chat_for_scratch).await,
        "submit_feedback" => crate::feedback::submit_feedback(a, args, &chat_for_scratch).await,
        "free_space" => crate::disk::free_space_tool(a, args).await,
        "browser_fill" => crate::vault::browser_fill(a, args, &chat_for_scratch).await,
        "routines_list" | "routines_create" | "routines_update" | "routines_delete" => {
            crate::routines::call_tool(a, name, args, &chat_for_scratch).await
        }
        "goals_list" | "goals_create" | "goals_update" | "goals_log" | "goals_delete" => {
            crate::goals::call_tool(a, name, args, &chat_for_scratch).await
        }
        "chat_read" | "chat_search" => {
            crate::chat_tools::call_tool(a, name, args, &chat_for_scratch).await
        }
        _ => err_result(format!("unknown tool {name}")),
    }
}

/// One line in `/work/.arbos/audit.jsonl`, the same file the pi extension writes.
fn audit_line(a: &Arc<Agent>, entry: Value) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(a.cfg.audit_path())
    {
        let _ = writeln!(f, "{entry}");
    }
}

fn page_rows(rows: &[crate::store::T3Row]) -> String {
    let mut out = Vec::new();
    let mut budget = PAGE_TOKENS_CHARS;
    let mut next: Option<String> = None;
    for r in rows {
        let text = if r.text.len() > 8000 {
            clip(&r.text, 8000)
        } else {
            r.text.clone()
        };
        if text.len() > budget {
            next = Some(r.id.clone());
            break;
        }
        budget -= text.len();
        out.push(json!({ "id": r.id, "kind": r.kind, "role": r.role, "at": r.at, "text": text }));
    }
    json!({ "turns": out, "next_cursor": next }).to_string()
}

pub(crate) fn materialize_handle(a: &Agent, chat_id: &str, note: &str, text: &str) -> String {
    use sha2::Digest;
    let sha = hex::encode(sha2::Sha256::digest(text.as_bytes()));
    let id = format!("h_{}", &sha[..8]);
    let dir = a.cfg.blobs_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(&sha);
    let _ = std::fs::write(&path, text);
    let _ = a.store.upsert_handle(
        &id,
        Some(chat_id),
        None,
        "text",
        text.len() as i64,
        text.lines().count() as i64,
        &path.display().to_string(),
        note,
        None,
    );
    id
}

async fn mcp(State(a): State<S>, headers: HeaderMap, Json(req): Json<Value>) -> Response {
    let tok = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let role = if tok == a.cfg.mcp_token {
        SessionRole::Main
    } else if tok == a.cfg.subagent_mcp_token() {
        SessionRole::Subagent
    } else {
        tracing::warn!(got = %tok.chars().take(8).collect::<String>(), "mcp: bad token");
        return (StatusCode::UNAUTHORIZED, "bad token").into_response();
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(json!({}));
    let result = match method {
        "initialize" => json!({ "protocolVersion": "2025-03-26", "capabilities": { "tools": {} }, "serverInfo": { "name": "arbos", "version": env!("CARGO_PKG_VERSION") } }),
        "notifications/initialized" | "notifications/cancelled" => return StatusCode::ACCEPTED.into_response(),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match refusal_for(role, name) {
                Some(reason) => {
                    tracing::info!(tool = name, ?role, "mcp: refused for role");
                    err_result(reason)
                }
                None => call_tool(&a, name, &args).await,
            }
        }
        _ => return Json(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("unknown method {method}") } })).into_response(),
    };
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

pub fn router(agent: Arc<Agent>) -> Router {
    Router::new().route("/mcp", post(mcp)).with_state(agent)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D69: a subagent-role session may not speak for the Project to other Projects.
    #[test]
    fn subagent_cannot_reach_other_projects() {
        assert!(refusal_for(SessionRole::Subagent, "send_message").is_some());
        assert!(refusal_for(SessionRole::Subagent, "group_send").is_some());
        assert!(refusal_for(SessionRole::Subagent, "projects_send").is_some());
        assert!(refusal_for(SessionRole::Subagent, "history_search").is_none());
        assert!(refusal_for(SessionRole::Main, "send_message").is_none());
        assert!(refusal_for(SessionRole::Main, "group_send").is_none());
        assert!(refusal_for(SessionRole::Main, "projects_send").is_none());
    }
}
