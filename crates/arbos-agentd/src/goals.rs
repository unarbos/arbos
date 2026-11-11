//! Goals MCP tools: long-running objectives tracked across days. Storage and check-in
//! routines live on the relay; this module lists/creates/updates them and appends the
//! progress log. Active goals are also injected into the working set so every chat sees them.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::Agent;
use crate::mcp::{err_result, text_result};

/// MCP tool specs for Goals.
pub fn tool_specs() -> Vec<Value> {
    vec![
        json!({
            "name": "goals_list",
            "description": "List this Project's goals (long-running objectives): id, title, outcome, status (active|blocked|waiting|paused|done), next_step, plan, last_moved_at, autonomy, check_in_cron.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "goals_create",
            "description": "Create a goal the agent will track across days. Prefer when the user says \"make this a goal\" or describes work that will take days. Stores title, desired outcome, plan steps, status, and schedules check-ins via routines when check_in_cron is set (five-field cron, e.g. \"0 9 * * *\" daily). autonomy: ask (default, ask before acting), suggest (propose then wait), act (do the next step).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "outcome": { "type": "string" },
                    "plan": { "type": "array", "items": { "type": "object" } },
                    "status": { "type": "string" },
                    "next_step": { "type": "string" },
                    "chat_id": { "type": "string" },
                    "check_in_cron": { "type": "string" },
                    "check_in_tz": { "type": "string" },
                    "autonomy": { "type": "string", "enum": ["ask", "suggest", "act"] }
                },
                "required": ["title"]
            }
        }),
        json!({
            "name": "goals_update",
            "description": "Update a goal by id: title, outcome, plan, status, next_step, check_in_cron, autonomy, waiting_question (one clear question when blocked; set clear_waiting=true after they answer). Do not nag if waiting_question is already set.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "outcome": { "type": "string" },
                    "plan": { "type": "array" },
                    "status": { "type": "string" },
                    "next_step": { "type": "string" },
                    "check_in_cron": { "type": "string" },
                    "check_in_tz": { "type": "string" },
                    "autonomy": { "type": "string" },
                    "waiting_question": { "type": "string" },
                    "clear_waiting": { "type": "boolean" },
                    "linked_chat_ids": { "type": "array", "items": { "type": "string" } },
                    "linked_subagent_ids": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "goals_log",
            "description": "Append one dated progress note to a goal's log (what moved, what is next).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "text": { "type": "string" }
                },
                "required": ["id", "text"]
            }
        }),
        json!({
            "name": "goals_delete",
            "description": "Delete a goal by id (also removes its check-in routine).",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }
        }),
    ]
}

fn goals_path(agent: &Agent, suffix: &str) -> String {
    format!(
        "{}/projects/{}/goals{}",
        agent.cfg.relay_url, agent.cfg.project_id, suffix
    )
}

async fn relay_goals_json(
    agent: &Agent,
    method: reqwest::Method,
    suffix: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let mut req = agent
        .http_client()
        .request(method, goals_path(agent, suffix))
        .bearer_auth(&agent.cfg.box_token);
    if let Some(b) = body {
        req = req.json(&b);
    }
    // Bounded: the goals poller calls this in a loop (see the push poll in `routines.rs`).
    let r = req
        .timeout(crate::workspace::REGISTRY_CALL_TIMEOUT)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "relay goals{suffix}: HTTP {status} {}",
            crate::relay_error::message(&text)
        ));
    }
    if text.trim().is_empty() {
        return Ok(json!({ "ok": true }));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Dispatch `goals_*` MCP tools.
pub async fn call_tool(agent: &Arc<Agent>, name: &str, args: &Value, chat_hint: &str) -> Value {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_owned);
    let result = match name {
        "goals_list" => match relay_goals_json(agent, reqwest::Method::GET, "", None).await {
            Ok(v) => text_result(v.to_string()),
            Err(e) => err_result(e),
        },
        "goals_create" => {
            let title = s("title").unwrap_or_default();
            if title.trim().is_empty() {
                return err_result("title is required");
            }
            let chat_id = s("chat_id").filter(|c| !c.is_empty()).or_else(|| {
                let hint = chat_hint.trim();
                (!hint.is_empty() && hint != "shared").then(|| hint.to_owned())
            });
            let mut body = json!({
                "title": title.trim(),
                "outcome": s("outcome").unwrap_or_default(),
                "next_step": s("next_step").unwrap_or_default(),
                "status": s("status").unwrap_or_else(|| "active".into()),
                "autonomy": s("autonomy").unwrap_or_else(|| "ask".into()),
                "chatId": chat_id,
            });
            if let Some(o) = body.as_object_mut() {
                if let Some(p) = args.get("plan") {
                    o.insert("plan".into(), p.clone());
                }
                if let Some(c) = s("check_in_cron").filter(|c| !c.is_empty()) {
                    o.insert("checkInCron".into(), json!(c));
                }
                if let Some(tz) = s("check_in_tz").filter(|c| !c.is_empty()) {
                    o.insert("checkInTz".into(), json!(tz));
                }
            }
            match relay_goals_json(agent, reqwest::Method::POST, "", Some(body)).await {
                Ok(v) => text_result(v.to_string()),
                Err(e) => err_result(e),
            }
        }
        "goals_update" => {
            let id = s("id").unwrap_or_default();
            if id.trim().is_empty() {
                return err_result("id is required");
            }
            let mut body = json!({});
            if let Some(o) = body.as_object_mut() {
                for (k, wire) in [
                    ("title", "title"),
                    ("outcome", "outcome"),
                    ("status", "status"),
                    ("next_step", "next_step"),
                    ("check_in_cron", "checkInCron"),
                    ("check_in_tz", "checkInTz"),
                    ("autonomy", "autonomy"),
                    ("waiting_question", "waitingQuestion"),
                ] {
                    if let Some(v) = s(k) {
                        o.insert(wire.into(), json!(v));
                    }
                }
                if let Some(p) = args.get("plan") {
                    o.insert("plan".into(), p.clone());
                }
                if let Some(v) = args.get("clear_waiting").and_then(Value::as_bool) {
                    o.insert("clear_waiting".into(), json!(v));
                }
                if let Some(v) = args.get("linked_chat_ids") {
                    o.insert("linkedChatIds".into(), v.clone());
                }
                if let Some(v) = args.get("linked_subagent_ids") {
                    o.insert("linkedSubagentIds".into(), v.clone());
                }
            }
            match relay_goals_json(agent, reqwest::Method::PATCH, &format!("/{id}"), Some(body))
                .await
            {
                Ok(v) => text_result(v.to_string()),
                Err(e) => err_result(e),
            }
        }
        "goals_log" => {
            let id = s("id").unwrap_or_default();
            let text = s("text").unwrap_or_default();
            if id.trim().is_empty() || text.trim().is_empty() {
                return err_result("id and text are required");
            }
            let body = json!({ "text": text.trim() });
            match relay_goals_json(
                agent,
                reqwest::Method::POST,
                &format!("/{id}/log"),
                Some(body),
            )
            .await
            {
                Ok(v) => text_result(v.to_string()),
                Err(e) => err_result(e),
            }
        }
        "goals_delete" => {
            let id = s("id").unwrap_or_default();
            if id.trim().is_empty() {
                return err_result("id is required");
            }
            match relay_goals_json(agent, reqwest::Method::DELETE, &format!("/{id}"), None).await {
                Ok(_) => text_result(json!({ "ok": true, "id": id }).to_string()),
                Err(e) => err_result(e),
            }
        }
        other => err_result(format!("unknown goals tool {other}")),
    };
    // Keep the working-set pin fresh after any list or mutation.
    agent.refresh_goals_cache().await;
    result
}

impl Agent {
    /// Fetch active goals from the relay for the working set (pinned in memory).
    pub async fn active_goals_text(&self) -> String {
        match relay_goals_json(self, reqwest::Method::GET, "", None).await {
            Ok(v) => {
                let items = v
                    .get("goals")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let active: Vec<&Value> = items
                    .iter()
                    .filter(|g| {
                        matches!(
                            g.get("status").and_then(Value::as_str),
                            Some("active" | "blocked" | "waiting")
                        )
                    })
                    .collect();
                if active.is_empty() {
                    return String::new();
                }
                let mut lines = vec![format!(
                    "Active goals ({}): long-running objectives — link new work to them when it fits; use goals_update / goals_log to move them forward.",
                    active.len()
                )];
                for g in active.iter().take(12) {
                    let title = g.get("title").and_then(Value::as_str).unwrap_or("Goal");
                    let id = g.get("id").and_then(Value::as_str).unwrap_or("?");
                    let status = g.get("status").and_then(Value::as_str).unwrap_or("active");
                    let next = g
                        .get("next_step")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim();
                    let autonomy = g.get("autonomy").and_then(Value::as_str).unwrap_or("ask");
                    let next_bit = if next.is_empty() {
                        String::new()
                    } else {
                        format!(" · next: {next}")
                    };
                    lines.push(format!(
                        "- {title} ({id}, {status}, autonomy={autonomy}){next_bit}"
                    ));
                }
                lines.join("\n")
            }
            Err(_) => String::new(),
        }
    }
}
