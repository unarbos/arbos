//! App control for the owner's agent: one MCP tool (`arbos_app`) that discovers and runs the
//! same people-chat / settings operations the iPhone and Mac apps call on the relay.
//!
//! The system prompt only names the tool and how to discover commands. Full argument schemas
//! live here and are returned by `list_commands` / `describe`. Destructive runs (delete a
//! group, remove members) ask for an approval card first, write a visible activity notice in
//! the current chat (tappable target metadata, same shape as "Messaged <agent>"), and rely on
//! relay authz for what the owner's user may do.

use std::sync::Arc;

use serde_json::{json, Value};

use arbos_core::approval::{ApprovalRequest, Risk};
use arbos_core::ids::ulid;

use crate::agent::Agent;
use crate::mcp::{err_result, text_result};

/// One command the `arbos_app` tool can run.
#[derive(Clone, Copy)]
struct Command {
    name: &'static str,
    summary: &'static str,
    /// When true, `run` posts an approval card before calling the relay.
    destructive: bool,
    args_schema: &'static str,
}

const COMMANDS: &[Command] = &[
    Command {
        name: "create_group",
        summary: "Create a people group with named people (GitHub handles) and/or the owner's agents (name or project id). The owner is added as owner.",
        destructive: false,
        args_schema: r#"{
            "type":"object",
            "properties":{
                "title":{"type":"string","description":"Optional group name (≤80 chars)"},
                "description":{"type":"string","description":"Optional about text (≤200 chars)"},
                "members":{"type":"array","items":{"type":"string"},"description":"GitHub handles of people to add"},
                "agents":{"type":"array","items":{"type":"string"},"description":"Agent names, @handles or project ids (must be the owner's)"}
            }
        }"#,
    },
    Command {
        name: "add_members",
        summary: "Add people and/or the owner's agents to an existing people group.",
        destructive: false,
        args_schema: r#"{
            "type":"object",
            "required":["chat_id"],
            "properties":{
                "chat_id":{"type":"string"},
                "members":{"type":"array","items":{"type":"string"},"description":"GitHub handles"},
                "agents":{"type":"array","items":{"type":"string"},"description":"Agent names or project ids"}
            }
        }"#,
    },
    Command {
        name: "remove_members",
        summary: "Remove people or agents from a group the owner may moderate. Pass handles, user ids, agent names or project ids. Removing yourself leaves the group.",
        destructive: true,
        args_schema: r#"{
            "type":"object",
            "required":["chat_id","members"],
            "properties":{
                "chat_id":{"type":"string"},
                "members":{"type":"array","items":{"type":"string"},"description":"Handles, user ids, agent names or project ids to remove"}
            }
        }"#,
    },
    Command {
        name: "leave_group",
        summary: "Leave a people group as the owner (the owner's agents detach with them).",
        destructive: false,
        args_schema: r#"{"type":"object","required":["chat_id"],"properties":{"chat_id":{"type":"string"}}}"#,
    },
    Command {
        name: "delete_group",
        summary: "Delete a people group. Owner only — the relay refuses anyone else.",
        destructive: true,
        args_schema: r#"{"type":"object","required":["chat_id"],"properties":{"chat_id":{"type":"string"}}}"#,
    },
    Command {
        name: "archive_chat",
        summary: "Archive a chat for the owner (hides it in the sidebar; history stays).",
        destructive: false,
        args_schema: r#"{"type":"object","required":["chat_id"],"properties":{"chat_id":{"type":"string"}}}"#,
    },
    Command {
        name: "unarchive_chat",
        summary: "Unarchive a chat for the owner.",
        destructive: false,
        args_schema: r#"{"type":"object","required":["chat_id"],"properties":{"chat_id":{"type":"string"}}}"#,
    },
    Command {
        name: "update_group",
        summary: "Rename a group or change its description (owner/admin only).",
        destructive: false,
        args_schema: r#"{
            "type":"object",
            "required":["chat_id"],
            "properties":{
                "chat_id":{"type":"string"},
                "title":{"type":"string","description":"New name; empty string clears it"},
                "description":{"type":"string","description":"New about text; empty string clears it"}
            }
        }"#,
    },
    Command {
        name: "update_settings",
        summary: "Change safe owner settings: display name and/or who may add them to groups (anyone | people_i_know | nobody). Handle changes are refused.",
        destructive: false,
        args_schema: r#"{
            "type":"object",
            "properties":{
                "name":{"type":"string","description":"Display name; empty clears it"},
                "group_add":{"type":"string","enum":["anyone","people_i_know","nobody"]}
            }
        }"#,
    },
    Command {
        name: "search_messages",
        summary: "Search message text across the owner's people chats, or inside one chat.",
        destructive: false,
        args_schema: r#"{
            "type":"object",
            "required":["q"],
            "properties":{
                "q":{"type":"string"},
                "chat_id":{"type":"string","description":"Optional: limit to one chat"},
                "limit":{"type":"integer","minimum":1,"maximum":40}
            }
        }"#,
    },
    Command {
        name: "send_message",
        summary: "Send a message as the owner to a people chat (user DM, group) by chat_id, or open/create a DM by handle. For another of the owner's agents, prefer the existing send_message tool with `to`.",
        destructive: false,
        args_schema: r#"{
            "type":"object",
            "required":["text"],
            "properties":{
                "text":{"type":"string"},
                "chat_id":{"type":"string","description":"People chat id"},
                "handle":{"type":"string","description":"Person's GitHub handle — opens or reuses their DM"},
                "reply_to":{"type":"string"}
            }
        }"#,
    },
    Command {
        name: "list_chats",
        summary: "List the owner's people chats (id, title, kind, members summary).",
        destructive: false,
        args_schema: r#"{"type":"object","properties":{}}"#,
    },
    Command {
        name: "list_contacts",
        summary: "List or search people the owner knows (directory / handle prefix).",
        destructive: false,
        args_schema: r#"{
            "type":"object",
            "properties":{
                "q":{"type":"string","description":"Optional handle prefix search"},
                "limit":{"type":"integer","minimum":1,"maximum":100}
            }
        }"#,
    },
];

pub fn tool_specs() -> Vec<Value> {
    vec![json!({
        "name": "arbos_app",
        "description": "Control the Arbos app as the owner (groups, members, archive, settings, search, messaging, contacts). Discover first: action=list_commands, then describe with cmd, then run with cmd + args. Schemas are not in the system prompt.",
        "inputSchema": {
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": { "type": "string", "enum": ["list_commands", "describe", "run"] },
                "cmd": { "type": "string", "description": "Command name for describe/run" },
                "args": { "type": "object", "description": "Arguments for run (see describe)" }
            }
        }
    })]
}

fn find_cmd(name: &str) -> Option<&'static Command> {
    COMMANDS
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(name.trim()))
}

fn list_commands_json() -> Value {
    json!(COMMANDS
        .iter()
        .map(|c| json!({
            "name": c.name,
            "summary": c.summary,
            "destructive": c.destructive,
        }))
        .collect::<Vec<_>>())
}

fn describe_json(cmd: &Command) -> Value {
    let schema: Value = serde_json::from_str(cmd.args_schema).unwrap_or(json!({}));
    json!({
        "name": cmd.name,
        "summary": cmd.summary,
        "destructive": cmd.destructive,
        "args": schema,
    })
}

pub async fn call_tool(agent: &Arc<Agent>, args: &Value, chat_id: &str) -> Value {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    match action {
        "list_commands" => text_result(list_commands_json().to_string()),
        "describe" => {
            let name = args.get("cmd").and_then(Value::as_str).unwrap_or("");
            match find_cmd(name) {
                Some(c) => text_result(describe_json(c).to_string()),
                None => err_result(format!(
                    "unknown cmd {name:?}; call list_commands for the catalogue"
                )),
            }
        }
        "run" => {
            let name = args.get("cmd").and_then(Value::as_str).unwrap_or("");
            let Some(cmd) = find_cmd(name) else {
                return err_result(format!(
                    "unknown cmd {name:?}; call list_commands for the catalogue"
                ));
            };
            let cmd_args = args.get("args").cloned().unwrap_or_else(|| json!({}));
            if cmd.destructive {
                if let Err(e) = approve_destructive(agent, cmd, &cmd_args, chat_id).await {
                    return err_result(e);
                }
            }
            match run_command(agent, cmd.name, &cmd_args).await {
                Ok(result) => {
                    if cmd.destructive {
                        post_activity_notice(agent, chat_id, cmd.name, &cmd_args, &result).await;
                    }
                    text_result(result.to_string())
                }
                Err(e) => err_result(e),
            }
        }
        "" => err_result("action is required: list_commands | describe | run"),
        other => err_result(format!(
            "unknown action {other:?}; use list_commands, describe, or run"
        )),
    }
}

async fn approve_destructive(
    agent: &Arc<Agent>,
    cmd: &Command,
    args: &Value,
    chat_id: &str,
) -> Result<(), String> {
    let title = match cmd.name {
        "delete_group" => format!(
            "Delete group {}",
            args.get("chat_id").and_then(Value::as_str).unwrap_or("?")
        ),
        "remove_members" => format!(
            "Remove members from {}",
            args.get("chat_id").and_then(Value::as_str).unwrap_or("?")
        ),
        other => format!("Run {other}"),
    };
    let summary = args.to_string();
    let summary: String = summary.chars().take(200).collect();
    let req = ApprovalRequest {
        title,
        summary,
        risk: Risk::Destructive,
        payload: json!({
            "kind": "arbos_app",
            "cmd": cmd.name,
            "args": args,
        }),
    };
    let outcome = agent.request_approval(chat_id, req).await;
    if arbos_core::approval::may_dispatch(outcome.as_ref()) {
        Ok(())
    } else {
        Err(match outcome {
            Some(arbos_core::approval::ApprovalOutcome::Rejected { reason }) => format!(
                "the user rejected this action{}",
                reason
                    .filter(|r| !r.trim().is_empty())
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default()
            ),
            Some(arbos_core::approval::ApprovalOutcome::Cancelled) => {
                "this work was cancelled, so the approval card was closed unanswered".into()
            }
            None => "nothing ran: this needs the user's approval and the card is still waiting \
                     for them. Do not retry it — Arbos runs the call when they approve"
                .into(),
            Some(arbos_core::approval::ApprovalOutcome::Approved) => {
                unreachable!("approved outcomes dispatch")
            }
        })
    }
}

/// Visible activity line in the current chat for a destructive app action. Clients that tap
/// "Messaged <agent>" lines can use `appAction.chatId` the same way once they learn this field.
async fn post_activity_notice(
    agent: &Arc<Agent>,
    chat_id: &str,
    cmd: &str,
    args: &Value,
    result: &Value,
) {
    let target = args
        .get("chat_id")
        .and_then(Value::as_str)
        .or_else(|| result.get("chat_id").and_then(Value::as_str))
        .unwrap_or("");
    let label = match cmd {
        "delete_group" => format!("Deleted group {target}"),
        "remove_members" => {
            let names = args
                .get("members")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            format!("Removed {names} from {target}")
        }
        other => format!("App action: {other}"),
    };
    let Ok(host) = agent.room(chat_id).await else {
        return;
    };
    let h = host.lock().await;
    let row_id = ulid();
    h.doc.transact(|d| {
        let _ = d.append_system_line(&row_id, &label, &agent.cfg.device_id, None);
    });
    h.doc.set_message_fields(
        &row_id,
        &[
            (
                "appAction",
                json!({
                    "kind": cmd,
                    "chatId": target,
                    "args": args,
                }),
            ),
            // Same tap affordance family as handoff rows (clients open the target).
            ("handoffMessages", json!(1)),
            ("subagentName", json!(target)),
        ],
    );
}

async fn run_command(agent: &Arc<Agent>, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "create_group" => create_group(agent, args).await,
        "add_members" => add_members(agent, args).await,
        "remove_members" => remove_members(agent, args).await,
        "leave_group" => {
            let chat_id = req_str(args, "chat_id")?;
            relay_json(agent, "POST", &format!("/chats/{chat_id}/leave"), None).await
        }
        "delete_group" => {
            let chat_id = req_str(args, "chat_id")?;
            let _ = relay_json(agent, "DELETE", &format!("/chats/{chat_id}"), None).await?;
            Ok(json!({ "ok": true, "chat_id": chat_id, "deleted": true }))
        }
        "archive_chat" => set_archived(agent, args, true).await,
        "unarchive_chat" => set_archived(agent, args, false).await,
        "update_group" => update_group(agent, args).await,
        "update_settings" => update_settings(agent, args).await,
        "search_messages" => search_messages(agent, args).await,
        "send_message" => send_app_message(agent, args).await,
        "list_chats" => list_chats(agent).await,
        "list_contacts" => list_contacts(agent, args).await,
        other => Err(format!("unhandled command {other}")),
    }
}

fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn str_list(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

async fn create_group(agent: &Arc<Agent>, args: &Value) -> Result<Value, String> {
    let members = str_list(args, "members");
    let agent_ids = resolve_agents(agent, &str_list(args, "agents")).await?;
    if members.is_empty() && agent_ids.is_empty() {
        return Err("name at least one person (members) or agent (agents)".into());
    }
    let body = json!({
        "kind": "group",
        "members": members,
        "agent_ids": agent_ids,
        "title": args.get("title").and_then(Value::as_str),
        "description": args.get("description").and_then(Value::as_str),
    });
    relay_json(agent, "POST", "/chats", Some(&body)).await
}

async fn add_members(agent: &Arc<Agent>, args: &Value) -> Result<Value, String> {
    let chat_id = req_str(args, "chat_id")?;
    let handles = str_list(args, "members");
    let agent_ids = resolve_agents(agent, &str_list(args, "agents")).await?;
    if handles.is_empty() && agent_ids.is_empty() {
        return Err("name at least one person or agent".into());
    }
    let body = json!({ "handles": handles, "agent_ids": agent_ids });
    relay_json(agent, "POST", &format!("/chats/{chat_id}/members"), Some(&body)).await
}

async fn remove_members(agent: &Arc<Agent>, args: &Value) -> Result<Value, String> {
    let chat_id = req_str(args, "chat_id")?;
    let members = str_list(args, "members");
    if members.is_empty() {
        return Err("members is required".into());
    }
    let roster = relay_json(agent, "GET", &format!("/chats/{chat_id}"), None).await?;
    let mut removed = Vec::new();
    let mut errors = Vec::new();
    for raw in members {
        match resolve_member_target(agent, &roster, &raw).await {
            Ok(MemberTarget::User(uid)) => {
                match relay_status(agent, "DELETE", &format!("/chats/{chat_id}/members/{uid}"), None)
                    .await
                {
                    Ok(()) => removed.push(json!({ "kind": "user", "id": uid, "input": raw })),
                    Err(e) => errors.push(json!({ "input": raw, "error": e })),
                }
            }
            Ok(MemberTarget::Agent(pid)) => {
                match relay_status(
                    agent,
                    "DELETE",
                    &format!("/chats/{chat_id}/agents/{pid}"),
                    None,
                )
                .await
                {
                    Ok(()) => removed.push(json!({ "kind": "agent", "id": pid, "input": raw })),
                    Err(e) => errors.push(json!({ "input": raw, "error": e })),
                }
            }
            Ok(MemberTarget::SelfLeave) => {
                match relay_json(agent, "POST", &format!("/chats/{chat_id}/leave"), None).await {
                    Ok(_) => removed.push(json!({ "kind": "self", "input": raw })),
                    Err(e) => errors.push(json!({ "input": raw, "error": e })),
                }
            }
            Err(e) => errors.push(json!({ "input": raw, "error": e })),
        }
    }
    Ok(json!({ "chat_id": chat_id, "removed": removed, "errors": errors }))
}

enum MemberTarget {
    User(String),
    Agent(String),
    SelfLeave,
}

async fn resolve_member_target(
    agent: &Arc<Agent>,
    roster: &Value,
    raw: &str,
) -> Result<MemberTarget, String> {
    let trimmed = raw.trim().trim_start_matches('@');
    if trimmed.eq_ignore_ascii_case("me")
        || trimmed.eq_ignore_ascii_case("myself")
        || trimmed.eq_ignore_ascii_case("self")
    {
        return Ok(MemberTarget::SelfLeave);
    }
    if trimmed == agent.cfg.project_id
        || agent
            .cfg
            .project_name
            .eq_ignore_ascii_case(trimmed)
    {
        return Ok(MemberTarget::Agent(agent.cfg.project_id.clone()));
    }
    if let Some(members) = roster.get("members").and_then(Value::as_array) {
        for m in members {
            let member = m.get("member").and_then(Value::as_str).unwrap_or("");
            let handle = m.get("handle").and_then(Value::as_str).unwrap_or("");
            let name = m.get("name").and_then(Value::as_str).unwrap_or("");
            if let Some(uid) = member.strip_prefix("user:") {
                if uid == trimmed
                    || handle.eq_ignore_ascii_case(trimmed)
                    || name.eq_ignore_ascii_case(trimmed)
                {
                    if uid == agent.people.project_owner() {
                        return Ok(MemberTarget::SelfLeave);
                    }
                    return Ok(MemberTarget::User(uid.to_owned()));
                }
            }
            if let Some(pid) = member.strip_prefix("agent:") {
                if pid == trimmed || name.eq_ignore_ascii_case(trimmed) {
                    return Ok(MemberTarget::Agent(pid.to_owned()));
                }
            }
        }
    }
    let ids = resolve_agents(agent, &[trimmed.to_owned()]).await?;
    if let Some(pid) = ids.into_iter().next() {
        return Ok(MemberTarget::Agent(pid));
    }
    Err(format!("no member matches {raw:?}"))
}

async fn set_archived(agent: &Arc<Agent>, args: &Value, archived: bool) -> Result<Value, String> {
    let chat_id = req_str(args, "chat_id")?;
    let body = json!({ "archived": archived });
    relay_json(
        agent,
        "PUT",
        &format!("/chats/{chat_id}/settings"),
        Some(&body),
    )
    .await
}

async fn update_group(agent: &Arc<Agent>, args: &Value) -> Result<Value, String> {
    let chat_id = req_str(args, "chat_id")?;
    let mut body = serde_json::Map::new();
    if let Some(t) = args.get("title") {
        body.insert("title".into(), t.clone());
    }
    if let Some(d) = args.get("description") {
        body.insert("description".into(), d.clone());
    }
    if body.is_empty() {
        return Err("provide title and/or description".into());
    }
    relay_json(
        agent,
        "PATCH",
        &format!("/chats/{chat_id}"),
        Some(&Value::Object(body)),
    )
    .await
}

async fn update_settings(agent: &Arc<Agent>, args: &Value) -> Result<Value, String> {
    let mut body = serde_json::Map::new();
    if let Some(n) = args.get("name") {
        body.insert("name".into(), n.clone());
    }
    if let Some(g) = args.get("group_add") {
        body.insert("group_add".into(), g.clone());
    }
    if body.is_empty() {
        return Err("provide name and/or group_add".into());
    }
    relay_json(agent, "PATCH", "/me", Some(&Value::Object(body))).await
}

async fn search_messages(agent: &Arc<Agent>, args: &Value) -> Result<Value, String> {
    let q = req_str(args, "q")?;
    let mut path = format!("/chats/search/messages?q={}", urlencoding_lite(q));
    if let Some(cid) = args.get("chat_id").and_then(Value::as_str) {
        path.push_str(&format!("&chat_id={}", urlencoding_lite(cid)));
    }
    if let Some(limit) = args.get("limit").and_then(Value::as_i64) {
        path.push_str(&format!("&limit={limit}"));
    }
    relay_json(agent, "GET", &path, None).await
}

async fn send_app_message(agent: &Arc<Agent>, args: &Value) -> Result<Value, String> {
    let text = req_str(args, "text")?;
    let chat_id = if let Some(id) = args.get("chat_id").and_then(Value::as_str) {
        id.to_owned()
    } else if let Some(handle) = args.get("handle").and_then(Value::as_str) {
        let dm = relay_json(agent, "POST", "/chats/dm", Some(&json!({ "handle": handle }))).await?;
        dm.get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "dm create returned no id".to_string())?
            .to_owned()
    } else {
        return Err("provide chat_id or handle".into());
    };
    let mut body = json!({ "text": text });
    if let Some(r) = args.get("reply_to") {
        body["reply_to"] = r.clone();
    }
    let sent = relay_json(
        agent,
        "POST",
        &format!("/chats/{chat_id}/messages"),
        Some(&body),
    )
    .await?;
    Ok(json!({ "chat_id": chat_id, "sent": sent }))
}

async fn list_chats(agent: &Arc<Agent>) -> Result<Value, String> {
    let chats = relay_json(agent, "GET", "/chats", None).await?;
    let slim: Vec<Value> = chats
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|c| {
            json!({
                "id": c.get("id"),
                "kind": c.get("kind"),
                "title": c.get("title"),
                "archived_at": c.get("archived_at"),
                "member_count": c.get("members").and_then(Value::as_array).map(|a| a.len()),
            })
        })
        .collect();
    Ok(json!({ "chats": slim }))
}

async fn list_contacts(agent: &Arc<Agent>, args: &Value) -> Result<Value, String> {
    if let Some(q) = args
        .get("q")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return relay_json(
            agent,
            "GET",
            &format!("/users/search?q={}", urlencoding_lite(q)),
            None,
        )
        .await;
    }
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(50);
    relay_json(agent, "GET", &format!("/users?limit={limit}"), None).await
}

/// Resolve agent names / ids to project ids the owner owns.
async fn resolve_agents(agent: &Arc<Agent>, names: &[String]) -> Result<Vec<String>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let projects = match relay_json(agent, "GET", "/box/projects", None).await {
        Ok(v) => v,
        Err(_) => relay_json(agent, "GET", "/projects", None).await?,
    };
    let list = projects
        .as_array()
        .cloned()
        .or_else(|| projects.get("projects").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let mut out = Vec::new();
    for raw in names {
        let key = raw.trim().trim_start_matches('@');
        if key.is_empty() {
            continue;
        }
        if key == "self" || key == "me" || key.eq_ignore_ascii_case(&agent.cfg.project_name) {
            out.push(agent.cfg.project_id.clone());
            continue;
        }
        let found = list.iter().find_map(|p| {
            let id = p.get("id").and_then(Value::as_str)?;
            let name = p.get("name").and_then(Value::as_str).unwrap_or("");
            let slug = p.get("slug").and_then(Value::as_str).unwrap_or("");
            if id == key || name.eq_ignore_ascii_case(key) || slug.eq_ignore_ascii_case(key) {
                Some(id.to_owned())
            } else {
                None
            }
        });
        match found {
            Some(id) => {
                if !out.iter().any(|x| x == &id) {
                    out.push(id);
                }
            }
            None if list.iter().any(|p| p.get("id").and_then(Value::as_str) == Some(key)) => {
                out.push(key.to_owned());
            }
            None => return Err(format!("no owned agent matches {raw:?}")),
        }
    }
    Ok(out)
}

fn urlencoding_lite(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

async fn relay_json(
    agent: &Arc<Agent>,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, String> {
    let url = format!("{}{path}", agent.cfg.relay_url);
    let mut req = match method {
        "GET" => agent.http_client().get(&url),
        "POST" => agent.http_client().post(&url),
        "PUT" => agent.http_client().put(&url),
        "PATCH" => agent.http_client().patch(&url),
        "DELETE" => agent.http_client().delete(&url),
        other => return Err(format!("unsupported method {other}")),
    };
    req = req
        .bearer_auth(&agent.cfg.box_token)
        .header("x-arbos-project", &agent.cfg.project_id);
    if let Some(b) = body {
        req = req.json(b);
    }
    let r = req.send().await.map_err(|e| e.to_string())?;
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(if serde_json::from_str::<arbos_core::api::ApiErrorDto>(&text).is_ok() {
            crate::relay_error::message(&text)
        } else {
            format!("relay {path}: HTTP {status} {text}")
        });
    }
    if text.trim().is_empty() {
        return Ok(json!({ "ok": true }));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

async fn relay_status(
    agent: &Arc<Agent>,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(), String> {
    let _ = relay_json(agent, method, path, body).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_covers_the_minimum_set() {
        let names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
        for need in [
            "create_group",
            "add_members",
            "remove_members",
            "leave_group",
            "delete_group",
            "archive_chat",
            "unarchive_chat",
            "update_group",
            "update_settings",
            "search_messages",
            "send_message",
            "list_chats",
            "list_contacts",
        ] {
            assert!(names.contains(&need), "missing {need}");
        }
        assert!(find_cmd("delete_group").unwrap().destructive);
        assert!(find_cmd("remove_members").unwrap().destructive);
        assert!(!find_cmd("create_group").unwrap().destructive);
    }

    #[test]
    fn describe_returns_parseable_schema() {
        let d = describe_json(find_cmd("create_group").unwrap());
        assert_eq!(d["name"], "create_group");
        assert!(d["args"]["properties"]["agents"].is_object());
    }

    #[test]
    fn tool_spec_is_one_discovery_tool() {
        let specs = tool_specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0]["name"], "arbos_app");
        let action = &specs[0]["inputSchema"]["properties"]["action"]["enum"];
        assert_eq!(action, &json!(["list_commands", "describe", "run"]));
    }

    #[test]
    fn list_commands_json_names_every_command() {
        let list = list_commands_json();
        assert_eq!(list.as_array().unwrap().len(), COMMANDS.len());
    }
}
