//! The project MCP server (design §6.7): Streamable-HTTP JSON-RPC on loopback.
//! Tools: history_search, history_read, read_thread, memory_search, memory_read,
//! peek, sub_llm, remember, recall, projects_list, ask_user,
//! spawn_subagent, wake_subagent, connectors_list, connector_call, local_devices,
//! local_run, local_read, local_list, deliver_file, generate_image, post_chart, send_message, vault_list,
//! vault_request, egress, browser_fill, routines_list, routines_create, routines_update,
//! routines_delete, goals_list, goals_create, goals_update, goals_log, goals_delete,
//! web_search, web_fetch, submit_feedback, publish_site, unpublish_site, list_sites.
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
use crate::approval::WaitDelivery;

type S = Arc<Agent>;

const MAX_HITS: i64 = 20;
const PAGE_TOKENS_CHARS: usize = 32_000; // ≈ 8k tokens
const DRAFT_CHANNELS: &[&str] = &["email", "slack", "telegram", "whatsapp", "sms"];

/// The `send_message` description. `chat_text_max` is this box's configured character limit for
/// one chat message, or `None` for no limit — which is the default and the truth about the box:
/// see `config::chat_text_max_with`. The figure here is the one the refusal names, so the model
/// has a real number to quote instead of the invented one of Test5 round 5 (bug Q).
fn send_message_description(chat_text_max: Option<usize>) -> String {
    let size = match chat_text_max {
        Some(max) => format!("One message holds at most {max} characters on this box; over that the call is refused and names the limit."),
        None => "There is no size limit: one call posts the whole thing, however long — a 1,500-row markdown table goes in one message. Never split an answer because you believe a limit exists, and never tell the user one does.".to_owned(),
    };
    format!("Message another agent, or post to this chat. The words go in `text` (`message` is accepted as the same field). `to` = its name, @handle or id (your owner's agents; another owner's only when they allowed it). The text lands in that agent's own chat and its reply comes back to you here — never write `@name …` as a bubble in this chat to reach them. Inside a group chat, omit `to` to speak to the other agents in the group. Without `to` outside a group, the text is posted in this chat for the user, as its own bubble — this is how a job posts a progress note in the chat it runs beside, and the only way its words are shown before its result. {size}")
}

fn tools(chat_text_max: Option<usize>) -> Value {
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
        { "name": "send_message", "description": send_message_description(chat_text_max),
          "inputSchema": { "type": "object", "properties": { "to": { "type": "string", "description": "Agent name, @handle or id" }, "text": { "type": "string", "description": "The words to post or send" }, "message": { "type": "string", "description": "Alias of `text`, accepted so a call does not fail on the field name" } } } },
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
        a.extend(crate::chart::tool_specs());
        a.extend(crate::vault::tool_specs());
        a.extend(crate::routines::tool_specs());
        a.extend(crate::goals::tool_specs());
        a.extend(crate::sites::tool_specs());
        a.extend(crate::web::tool_specs());
        a.extend(crate::feedback::tool_specs());
        a.extend(crate::disk::tool_specs());
    }
    list
}

/// The tool definitions this box serves, fixed for the life of the process.
///
/// Tool definitions sit inside the model's cached prefix — on this box about 27,900 tokens of
/// system prompt and tools, ahead of every turn of every chat. A definition that moves while a
/// chat is live throws that cache away for the chat and for each of its later turns: the
/// prompt-size measurement of 2026-11-05 caught a request whose only change was one tool's
/// definition, and it read nothing from cache and paid about $0.14 to write the prefix again.
///
/// So the one part of the block that is not a literal is read once. `send_message`'s size
/// sentence comes from the `chat_text_max` knob, which is read from disk at every call and can
/// therefore be written under a running box; pinning it here means an operator's edit reaches
/// the model at the next start, the same moment the rest of the prefix may move, rather than
/// at whatever turn a pi child happens to re-list the tools on.
fn pinned_tools(chat_text_max: Option<usize>) -> &'static Value {
    static PINNED: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    PINNED.get_or_init(|| tools(chat_text_max))
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

/// The bearer a request presented, trimmed; `""` when it presented none.
fn bearer(headers: &HeaderMap) -> &str {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .trim()
}

/// Which role a bearer token earns here, or `None` for no role at all.
///
/// The same check the hook server makes (`hooks::role_for_token`), which this one must not drift
/// from: both verifiers answer one question from one pair of tokens, so a session either admits
/// to both or to neither.
///
/// Both guards fail closed before any comparison. `ARBOS_MCP_TOKEN` defaults to empty
/// (`config.rs`) and an absent header reads as `""`, so the bare comparison this replaced matched
/// the main arm for a request carrying no credential — the highest role, granted anonymously. The
/// subagent arm was no safer, because `subagent_mcp_token()` hashes the configured value and so is
/// a constant anyone can compute offline once that value is empty. A workspace that was given a
/// token refuses nothing it used to accept.
fn role_for_token(configured: &str, derived: &str, headers: &HeaderMap) -> Option<SessionRole> {
    if configured.trim().is_empty() {
        return None;
    }
    let tok = bearer(headers);
    if tok.is_empty() {
        return None;
    }
    if tok == configured {
        Some(SessionRole::Main)
    } else if tok == derived {
        Some(SessionRole::Subagent)
    } else {
        None
    }
}

/// Tools only the main agent may use: anything that talks to another agent (D69 — subagents
/// never reach the cross-project surface). `group_send` / `projects_send` are the old names,
/// still accepted from older sessions.
const MAIN_ONLY_TOOLS: &[&str] = &["send_message", "group_send", "projects_send"];

/// `Some(reason)` when `role` may not call `tool` addressed at `to`.
///
/// `send_message` is two tools under one name. With `to` it speaks to another Project, which
/// stays the main agent's alone (D69). Without `to` it posts a bubble in the caller's own chat,
/// which is not a cross-project surface at all — refusing that is bug T of Test5 round 6
/// (r6cardjob): a job asked to name the approval card it waited on called
/// `mcp__arbos__send_message`, was told the tool belonged to the main agent, and stopped. The
/// user was told nothing, by anyone.
///
/// The refusal that remains names the open door, so a model that hits the wall is not left to
/// guess that the same tool answers without `to`.
pub fn refusal_for(role: SessionRole, tool: &str, to: Option<&str>) -> Option<String> {
    match role {
        SessionRole::Main => None,
        SessionRole::Subagent => {
            let addressed = to.map(str::trim).is_some_and(|t| !t.is_empty());
            (MAIN_ONLY_TOOLS.contains(&tool) && addressed).then(|| {
                format!(
                    "{tool} with `to` reaches another Project and is available to the main agent only; report back and let it speak for the Project. To post in your own chat — a progress note, the card you are waiting on — call {tool} with `text` and no `to`."
                )
            })
        }
    }
}

/// The pi session a request belongs to (`X-Arbos-Session`, written into `mcp.json` by
/// `workspace::seed_state` and filled from the child's `ARBOS_SESSION_ID`). For a worker it is
/// the subagent id, which is how a job's `send_message` finds the chat it runs beside; the main
/// session needs none, because it has the running turn.
const SESSION_HEADER: &str = "x-arbos-session";

fn session_of(headers: &HeaderMap) -> String {
    headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_owned()
}

/// Which chat a chat-scoped call belongs to, and the job behind it when a worker is calling.
///
/// The main agent is in its turn's chat. A worker is never in that turn: a job runs beside the
/// chat, the turn that started it ended long before, and `Agent::turn` holds the one turn state
/// of the box's main agent — so reading it for a worker would hand a job whatever chat the main
/// agent happens to be answering in. Bug W of Test5 round 7 is the quiet version of that: with
/// no turn at all the hint was the literal `"shared"`, and the routine went to the project's
/// main chat.
///
/// An unknown worker session (an older pi child that sends no header) resolves to no chat, which
/// is the behaviour this replaced: the relay still picks. It never borrows the main agent's turn.
pub(crate) async fn caller_for(
    a: &Arc<Agent>,
    role: SessionRole,
    session: &str,
) -> crate::routines::Caller {
    match role {
        SessionRole::Main => crate::routines::Caller {
            chat_id: a.turn.lock().await.as_ref().map(|t| t.chat_id.clone()),
            job_id: None,
        },
        SessionRole::Subagent => {
            if session.is_empty() {
                return crate::routines::Caller::default();
            }
            // The job row first, so `job_id` is set for the refusal; a worker running no job
            // still has the chat it was spawned for.
            if let Some(job) = a.store.job_for_subagent(session).ok().flatten() {
                return crate::routines::Caller {
                    chat_id: Some(job.chat_id),
                    job_id: Some(job.id),
                };
            }
            crate::routines::Caller {
                chat_id: a
                    .store
                    .subagent(session)
                    .ok()
                    .flatten()
                    .and_then(|s| s.chat_id),
                job_id: None,
            }
        }
    }
}

pub(crate) fn text_result(text: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }] })
}

pub(crate) fn err_result(msg: impl std::fmt::Display) -> Value {
    json!({ "content": [{ "type": "text", "text": format!("error: {msg}") }], "isError": true })
}

/// Head of `s`, counted in characters, like the `clip` in `local.rs` and `connectors.rs`.
///
/// Counting bytes here cut inside multi-byte characters and panicked, which `peek` could reach
/// with any slice over 16,000 bytes of non-ASCII text — one curly apostrophe across the cut is
/// enough. Characters are also what `peek` slices by and what the suffix below claims to count.
fn clip(s: &str, max: usize) -> String {
    // A string never holds more characters than bytes, so this fast path is exact.
    if s.len() <= max {
        return s.to_owned();
    }
    let chars = s.chars().count();
    if chars <= max {
        return s.to_owned();
    }
    format!(
        "{}…[{} more chars]",
        s.chars().take(max).collect::<String>(),
        chars - max
    )
}

async fn call_tool(
    a: &Arc<Agent>,
    name: &str,
    args: &Value,
    role: SessionRole,
    session: &str,
) -> Value {
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
            // Any chat of this project, not only the one the turn runs in, so the idle close may
            // well have taken its room (#2367). Opening it is the difference between reading the
            // history and telling the agent the chat does not exist.
            let Ok(host) = a.room(&chat_id).await else {
                return err_result(format!("no chat {chat_id} on this box"));
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
            // `sub_llm` is the agent reading something for itself, so it reads on the agent's
            // own model, not on the id the box booted with (Jacob, 2026-11-04).
            let model = a.model_for(crate::settings::ModelRole::Talking).await;
            for (i, c) in chunks.iter().enumerate().take(32) {
                match a.llm.complete(&model, &instruction, c, 1500).await {
                    Ok((t, _)) => parts.push(format!("[chunk {}]\n{t}", i + 1)),
                    Err(e) => return err_result(e),
                }
            }
            let mut joined = parts.join("\n\n");
            if parts.len() > 1 && joined.len() > 8000 {
                if let Ok((t, _)) = a
                    .llm
                    .complete(
                        &model,
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
            // `message` is the field name every other chat API uses, so it is the one a model
            // reaches for first — the r6cardjob call spent its only attempt on it. Accepting it
            // as an alias costs one line; the refusal below still names `text`, so the canonical
            // field is what gets learned rather than quietly replaced.
            let text = s("text").or_else(|| s("message")).unwrap_or_default();
            let to = s("to")
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty());
            if text.trim().is_empty() {
                err_result("text required: put the words in `text` (`message` is accepted as the same field)")
            } else {
                // `to` is the main agent's alone and `refusal_for` has already refused it for a
                // worker, so a worker reaching here is posting in its own chat. It has no turn
                // to post into: its chat is the one its job runs beside.
                let posted = match role {
                    SessionRole::Main => a.send_message_from_turn(to.as_deref(), &text).await,
                    SessionRole::Subagent => a.post_to_job_chat(session, &text).await,
                };
                match posted {
                    Ok(t) => text_result(t),
                    Err(e) => err_result(e),
                }
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
                .begin_approval(&chat_for_scratch, req, WaitDelivery::Deferred)
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
        "post_chart" => crate::chart::post_chart(a, args).await,
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
        "publish_site" => crate::sites::publish_site(a, args).await,
        "unpublish_site" => crate::sites::unpublish_site(a, args).await,
        "list_sites" => crate::sites::list_sites(a).await,
        "free_space" => crate::disk::free_space_tool(a, args).await,
        "browser_fill" => crate::vault::browser_fill(a, args, &chat_for_scratch).await,
        "routines_list" | "routines_create" | "routines_update" | "routines_delete" => {
            let caller = caller_for(a, role, session).await;
            crate::routines::call_tool(a, name, args, &caller).await
        }
        "goals_list" | "goals_create" | "goals_update" | "goals_log" | "goals_delete" => {
            crate::goals::call_tool(a, name, args, &chat_for_scratch).await
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
    let Some(role) = role_for_token(&a.cfg.mcp_token, &a.cfg.subagent_mcp_token(), &headers) else {
        tracing::warn!(got = %bearer(&headers).chars().take(8).collect::<String>(), "mcp: bad token");
        return (StatusCode::UNAUTHORIZED, "bad token").into_response();
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(json!({}));
    let result = match method {
        "initialize" => json!({ "protocolVersion": "2025-03-26", "capabilities": { "tools": {} }, "serverInfo": { "name": "arbos", "version": env!("CARGO_PKG_VERSION") } }),
        "notifications/initialized" | "notifications/cancelled" => return StatusCode::ACCEPTED.into_response(),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": pinned_tools(a.cfg.chat_text_max()) }),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let to = args.get("to").and_then(Value::as_str);
            match refusal_for(role, name, to) {
                Some(reason) => {
                    tracing::info!(tool = name, ?role, "mcp: refused for role");
                    err_result(reason)
                }
                None => call_tool(&a, name, &args, role, &session_of(&headers)).await,
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

    /// The tool block is the cached prefix, so the bar is identity rather than presence: the
    /// bytes a later `tools/list` answers must be the bytes of the first one, name for name,
    /// description for description, schema for schema and in the same order. A tool whose
    /// definition merely still exists, reworded or moved, has already cost the chat its cache.
    ///
    /// The one way this block could move under a live box is the `chat_text_max` knob, which
    /// `send_message`'s description quotes and which is read from disk at every call. Here that
    /// knob is written between two calls, as an operator may write it at any moment.
    #[test]
    fn the_tool_block_a_later_call_gets_is_the_one_the_first_call_got() {
        let first = serde_json::to_string(pinned_tools(None)).expect("the tool block");
        let after_a_knob_change =
            serde_json::to_string(pinned_tools(Some(4_000))).expect("the tool block");
        assert_eq!(
            first, after_a_knob_change,
            "a knob written under a running box must not move the tool block: it takes effect at the next start"
        );

        let names: Vec<&str> = pinned_tools(None)
            .as_array()
            .expect("a list of tools")
            .iter()
            .map(|t| t["name"].as_str().expect("a named tool"))
            .collect();
        assert!(
            names.contains(&"send_message") && names.len() > 20,
            "the pinned block is the real one, not an empty list: {names:?}"
        );
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            names.len(),
            "and no tool is defined twice, which would make the block's order ambiguous: {names:?}"
        );
    }

    /// `clip` used to slice by byte, so a cut landing inside a multi-byte character panicked.
    /// Every offset below is one a caller really passes: the approval-card summary 160, the
    /// per-call argument log 200, search snippets 300, `sub_llm` 2,000, `history_read` and
    /// `page_rows` 8,000, `peek` 16,000.
    #[test]
    fn clip_cuts_between_characters_never_inside_one() {
        for max in [160, 200, 300, 2_000, 8_000, 16_000] {
            // Three bytes per character, so no multiple of a byte offset is a boundary.
            let cjk = "日".repeat(max);
            let out = clip(&cjk, max / 2);
            assert_eq!(out.chars().filter(|c| *c == '日').count(), max / 2);
            assert!(out.ends_with(&format!("…[{} more chars]", max - max / 2)));

            // The realistic case: ordinary prose with one curly apostrophe across the cut.
            let mut prose = "a".repeat(max - 1);
            prose.push('\u{2019}');
            prose.push_str(&"b".repeat(max));
            assert!(clip(&prose, max).ends_with(&format!("…[{max} more chars]")));
        }
    }

    /// The suffix says "chars", so it must count characters. It used to print `s.len() - max`,
    /// a byte delta, which overstated the remainder by the multi-byte overhead.
    #[test]
    fn clip_reports_the_remainder_in_characters() {
        assert_eq!(clip("日日日日", 1), "日…[3 more chars]");
        assert_eq!(clip("abcd", 2), "ab…[2 more chars]");
    }

    /// The quiet half, on its own. The cut here lands on a character boundary, so the old
    /// byte-slicing `clip` did not panic and returned this exact head — it just called the
    /// three remaining characters nine. A correct slice with a lying count is still wrong.
    #[test]
    fn clip_counted_the_remainder_in_bytes_even_when_it_did_not_panic() {
        let s = "aaaa日日日";
        assert!(s.is_char_boundary(4), "the old byte cut was legal here");
        assert_eq!(s.len() - 4, 9, "the old count would have said nine");
        assert_eq!(clip(s, 4), "aaaa…[3 more chars]");
    }

    /// Text that fits comes back untouched, including text whose byte length exceeds `max`.
    #[test]
    fn clip_leaves_short_text_alone() {
        assert_eq!(clip("abc", 8), "abc");
        assert_eq!(clip("日日", 8), "日日");
        assert_eq!(clip("日日", 2), "日日");
    }

    /// A markdown table of `rows` data rows, in the shape the apps draw.
    fn sensor_table(rows: usize) -> String {
        let mut t = String::from("| # | sensor | reading | note |\n| --- | --- | --- | --- |");
        for i in 1..=rows {
            t.push_str(&format!(
                "\n| {i} | sensor-{i:04} | {}.{:02} | steady |",
                i % 90,
                i % 100
            ));
        }
        t
    }

    /// Jacob, Test5 round 5 (bug Q, r5giant 20:22). The user asked for a 1,500-row table in one
    /// chat message. The turn built it in a `codemode` script and posted it with
    /// `mcp__arbos__send_message({text})` — the only text surface a script has, since the
    /// harness `send_message` is direct-only. The call was refused ("say who to message …") and
    /// the turn told the user the table was too big for one chat message, and that an earlier
    /// 200-row table had therefore needed four. Both claims were invented.
    ///
    /// Nothing bounds a bubble's text, so the whole table goes out as one row, inside one relay
    /// frame, and a history fetch afterwards is healthy.
    #[tokio::test]
    async fn a_fifty_one_thousand_character_table_posts_as_one_row_in_this_chat() {
        let (agent, root) = crate::status::tests::test_agent().await;
        let chat = "01TESTCHATBIGTABLE00000001";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().expect("the held turn");
            agent.test_open_row(&host, t).await;
        }

        let table = sensor_table(1_500);
        let chars = table.chars().count();
        assert!(
            chars >= 51_000,
            "the round-5 table was 51,372 chars; this one is {chars}"
        );
        let out = call_tool(
            &agent,
            "send_message",
            &json!({ "text": table }),
            SessionRole::Main,
            "",
        )
        .await;
        assert!(out.get("isError").is_none(), "the table was refused: {out}");

        let rows: Vec<arbos_core::doc::MessageView> = {
            let h = host.lock().await;
            h.doc
                .messages()
                .into_iter()
                .filter(|m| m.has_visible_content())
                .collect()
        };
        assert_eq!(rows.len(), 1, "one message, one row: {} rows", rows.len());
        assert_eq!(rows[0].text(), table, "the row is not the table verbatim");

        // The one real bound on the path, for scale: a push frame. One table is a few per cent
        // of it, and a backlog over it is cut into frames that fit (#2762) rather than refused.
        let update = {
            let h = host.lock().await;
            h.doc.export_updates_since(&loro::VersionVector::default())
        };
        assert!(
            update.len() <= arbos_core::wire::ROW_MAX,
            "{chars} chars exported to {} bytes, over the {} a frame holds",
            update.len(),
            arbos_core::wire::ROW_MAX
        );

        // A history fetch afterwards is healthy: a message past one page comes back as its head
        // plus a handle for `peek`, the same way every large output does, not as an error.
        let fetched = call_tool(
            &agent,
            "chat_messages",
            &json!({ "chat_id": chat, "message_id": rows[0].id }),
            SessionRole::Main,
            "",
        )
        .await;
        assert!(fetched.get("isError").is_none(), "{fetched}");
        let body: Value = serde_json::from_str(
            fetched["content"][0]["text"]
                .as_str()
                .expect("one text part"),
        )
        .expect("the row is json");
        let head = body["text"].as_str().unwrap_or_default();
        assert!(
            head.starts_with("| # | sensor | reading | note |"),
            "the fetch lost the head of the table"
        );
        assert!(
            head.contains(&format!("of {chars} chars")),
            "the fetch must name the message's true size: {head:?}"
        );
        assert!(
            body["handle"].as_str().is_some_and(|h| !h.is_empty()),
            "a message past one page must come back with a handle to peek: {body}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The description is where the model learns whether a limit exists, so it states the truth
    /// either way: no number when there is none, the exact number when an operator set one.
    #[test]
    fn the_send_message_description_names_a_limit_only_when_one_exists() {
        let none = send_message_description(None);
        assert!(none.contains("There is no size limit"), "{none}");
        assert!(
            none.contains("posted in this chat for the user"),
            "the no-`to` behaviour is undocumented: {none}"
        );
        let capped = send_message_description(Some(4_000));
        assert!(
            capped.contains("at most 4000 characters on this box"),
            "{capped}"
        );
        assert!(
            !capped.contains("no size limit"),
            "a capped box must not also claim there is none: {capped}"
        );
        let listed = tools(Some(4_000));
        let spec = listed
            .as_array()
            .and_then(|a| a.iter().find(|t| t["name"] == "send_message"))
            .expect("send_message is listed");
        assert_eq!(spec["description"], json!(capped));
    }

    /// D69: a subagent-role session may not speak for the Project to other Projects. What it may
    /// do is post in its own chat, which is the same tool without `to` (bug T) — and the refusal
    /// of the cross-project form says so, because a sentence that only closes a door leaves the
    /// model to guess that the open one exists.
    #[test]
    fn subagent_cannot_reach_other_projects_but_may_post_in_its_own_chat() {
        for tool in ["send_message", "group_send", "projects_send"] {
            let refused = refusal_for(SessionRole::Subagent, tool, Some("lane-b"))
                .unwrap_or_else(|| panic!("{tool} with `to` is the main agent's"));
            assert!(
                refused.contains("with `text` and no `to`"),
                "the refusal must name the path that is open: {refused}"
            );
            assert!(
                refusal_for(SessionRole::Subagent, tool, None).is_none(),
                "{tool} with no `to` posts in this chat and is open to a worker"
            );
            assert!(
                refusal_for(SessionRole::Subagent, tool, Some("  ")).is_none(),
                "a blank `to` is no `to`"
            );
            assert!(refusal_for(SessionRole::Main, tool, Some("lane-b")).is_none());
        }
        assert!(refusal_for(SessionRole::Subagent, "history_search", None).is_none());
    }

    /// The r6cardjob call, replayed through the surface it was made on. A job's script reached
    /// `mcp__arbos__send_message` with a `message` argument and got two walls: the field name
    /// was wrong, and the tool was the main agent's. Both are gone — the alias lands, the words
    /// are in the job's chat, and a call with neither field is told which one to use.
    #[tokio::test]
    async fn a_jobs_script_posts_through_the_mcp_tool_with_either_field_name() {
        let (agent, root) = crate::status::tests::test_agent().await;
        let chat = "01TESTCHATMCPJOBNOTE000001";
        let ask = "01TESTASKMCPJOBNOTE0000001";
        let worker = "01TESTWORKERMCPJOBNOTE0001";
        let host = agent.sync.open_room(chat).await.unwrap();
        {
            let h = host.lock().await;
            h.doc
                .append_user_message(
                    ask,
                    "Name the card you wait on.",
                    "dev-mac",
                    &Default::default(),
                )
                .unwrap();
        }
        agent
            .store
            .insert_subagent(
                worker,
                "job-name-the-card-0001",
                "name the card",
                "pi",
                chat,
            )
            .unwrap();
        agent
            .store
            .insert_job(
                "01TESTJOBMCPJOBNOTE0000001",
                chat,
                ask,
                "Naming the card",
                "ops",
                "Raise the card and name it in the chat.",
                &[],
            )
            .unwrap();
        agent
            .store
            .set_job_subagent("01TESTJOBMCPJOBNOTE0000001", worker)
            .unwrap();

        let note = "Waiting on the card \"Delete 12 stale branches\".";
        let out = call_tool(
            &agent,
            "send_message",
            &json!({ "message": note }),
            SessionRole::Subagent,
            worker,
        )
        .await;
        assert!(out.get("isError").is_none(), "the call was refused: {out}");

        let rows: Vec<String> = {
            let h = host.lock().await;
            h.doc
                .messages()
                .into_iter()
                .filter(|m| m.role == "assistant" && m.has_visible_content())
                .map(|m| m.text())
                .collect()
        };
        assert_eq!(rows, vec![note.to_owned()], "the note is in the job's chat");

        let empty = call_tool(
            &agent,
            "send_message",
            &json!({}),
            SessionRole::Subagent,
            worker,
        )
        .await;
        let said = empty["content"][0]["text"].as_str().unwrap_or_default();
        assert_eq!(empty["isError"], json!(true), "{empty}");
        assert!(
            said.contains("put the words in `text`"),
            "the refusal must name the field: {said}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The session header is the only thing that tells this server which worker is calling: one
    /// subagent token is shared by every worker on the box, so without it a job's note has no
    /// chat to go to. pi fills it from `ARBOS_SESSION_ID`, which `PiAdapter::envs` always sets.
    #[test]
    fn the_mcp_config_asks_pi_for_the_session_id() {
        let root = std::env::temp_dir().join(format!("arbos-mcpcfg-{}", arbos_core::ids::ulid()));
        let bx = crate::config::BoxConfig {
            user_id: Some("u-test".into()),
            relay_url: "http://127.0.0.1:1".into(),
            box_token: "test-token".into(),
            mcp_token: "test-mcp".into(),
            work_root: root.clone(),
            bind: "127.0.0.1".into(),
            port: 0,
            local_box: true,
            pi_bin: "/nonexistent/pi".into(),
            prime_bin: "/nonexistent/prime-agent".into(),
            claude_bin: "/nonexistent/claude".into(),
            pi_ext_override: None,
            device_id: "box:test".into(),
            default_talking_model: "mock-talking".into(),
            default_subagent_model: "mock-sub".into(),
            legacy_project_id: None,
        };
        let cfg = bx.workspace(
            &json!({ "id": "01TESTPROJECTMCPCONFIG0000", "name": "Doug" }),
            0,
        );
        crate::workspace::seed_state(&cfg).expect("seed");
        let written: Value = serde_json::from_str(
            &std::fs::read_to_string(cfg.pi_dir().join("mcp.json")).expect("mcp.json"),
        )
        .expect("mcp.json is json");
        let headers = &written["mcpServers"]["arbos"]["headers"];
        assert_eq!(headers["Authorization"], json!("Bearer ${ARBOS_MCP_TOKEN}"));
        assert_eq!(headers["X-Arbos-Session"], json!("${ARBOS_SESSION_ID}"));
        assert_eq!(
            crate::harness::PiAdapter::envs(&cfg, "01TESTWORKERENVSESSION0001", "subagent")
                .into_iter()
                .find(|(k, _)| k == "ARBOS_SESSION_ID")
                .map(|(_, v)| v),
            Some("01TESTWORKERENVSESSION0001".to_owned()),
            "the header resolves from this env var, so every pi child must carry it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn auth(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            value.parse().expect("header value"),
        );
        h
    }

    /// `sub_` + sha256("subagent:" + token), as `Config::subagent_mcp_token` derives it.
    fn derive(token: &str) -> String {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(format!("subagent:{token}").as_bytes());
        format!("sub_{}", hex::encode(&digest[..16]))
    }

    /// An unconfigured daemon authenticates nobody. `ARBOS_MCP_TOKEN` defaults to empty and an
    /// absent header reads as `""` too, so the bare comparison this guard replaced handed the
    /// main role to a request carrying no credential at all. The subagent arm was no safer: it
    /// hashes the configured value, so with nothing configured it is a constant computable
    /// offline — asserted here with that very constant, not with a placeholder.
    #[test]
    fn an_unset_token_authenticates_nobody() {
        let computable = derive("");
        assert_eq!(role_for_token("", &computable, &HeaderMap::new()), None);
        assert_eq!(role_for_token("", &computable, &auth("Bearer ")), None);
        assert_eq!(
            role_for_token("", &computable, &auth(&format!("Bearer {computable}"))),
            None,
        );
        assert_eq!(
            role_for_token("   ", &computable, &auth("Bearer    ")),
            None
        );
    }

    /// A configured workspace still refuses a request that brings no token.
    #[test]
    fn a_request_without_a_token_earns_no_role() {
        let main = "mcp_0123456789abcdef";
        assert_eq!(role_for_token(main, &derive(main), &HeaderMap::new()), None,);
        assert_eq!(role_for_token(main, &derive(main), &auth("Bearer ")), None);
        assert_eq!(
            role_for_token(main, &derive(main), &auth("Basic xyz")),
            None
        );
    }

    /// One workspace's token is not another's. Since #2630 each workspace derives its own
    /// `mcp_…` value, and this is the comparison that has to hold the line: a sibling's token,
    /// and the sibling's subagent token, earn no role here.
    #[test]
    fn one_workspaces_token_is_not_anothers() {
        let (mine, sibling) = ("mcp_aaaaaaaaaaaaaaaa", "mcp_bbbbbbbbbbbbbbbb");

        assert_eq!(
            role_for_token(mine, &derive(mine), &auth(&format!("Bearer {mine}"))),
            Some(SessionRole::Main),
        );
        assert_eq!(
            role_for_token(
                mine,
                &derive(mine),
                &auth(&format!("Bearer {}", derive(mine))),
            ),
            Some(SessionRole::Subagent),
        );

        assert_eq!(
            role_for_token(mine, &derive(mine), &auth(&format!("Bearer {sibling}"))),
            None,
        );
        assert_eq!(
            role_for_token(
                mine,
                &derive(mine),
                &auth(&format!("Bearer {}", derive(sibling))),
            ),
            None,
        );
    }
}
