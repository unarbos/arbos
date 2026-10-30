//! Relay pushes delivered to the box (`GET /projects/{id}/pushes`), and the `routine_run` push
//! (F3): the prompt lands in the chosen chat as a user message with `sender = routine:<id>`, runs
//! as a normal turn, and the run status goes back to the relay.
//!
//! MCP tools (`routines_list` / `routines_create` / `routines_update` / `routines_delete`) let the
//! main agent schedule cron jobs and one-shot reminders through the relay scheduler.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use arbos_core::doc::CommandView;
use arbos_core::ids::{now_ms, ulid};

use crate::agent::Agent;
use crate::mcp::{err_result, text_result};

const PUSH_POLL: Duration = Duration::from_secs(5);

/// MCP tool specs for cron / once / webhook / event routines.
pub fn tool_specs() -> Vec<Value> {
    vec![
        json!({
            "name": "routines_list",
            "description": "List this Project's scheduled routines (cron jobs, one-shot reminders, webhooks, event triggers): id, name, prompt, trigger, enabled, next_run_at.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "routines_create",
            "description": "Schedule a routine that will message this chat later. Use for reminders and recurring jobs. trigger: {kind:\"cron\", expr, tz?} (five-field cron, e.g. \"0 9 * * 1-5\"), {kind:\"once\", in_seconds} or {kind:\"once\", at} (unix ms), {kind:\"webhook\"}, or {kind:\"event\", event}. prompt is what you will be asked to do when it fires (write it as an instruction to yourself, e.g. \"Tell the user: time is up.\"). Defaults to the current chat.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "prompt": { "type": "string" },
                    "trigger": { "type": "object" },
                    "chat_id": { "type": "string" },
                    "enabled": { "type": "boolean" }
                },
                "required": ["name", "prompt", "trigger"]
            }
        }),
        json!({
            "name": "routines_update",
            "description": "Pause, resume, or edit a routine by id (name, prompt, trigger, enabled, chat_id).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "prompt": { "type": "string" },
                    "trigger": { "type": "object" },
                    "chat_id": { "type": "string" },
                    "enabled": { "type": "boolean" }
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "routines_delete",
            "description": "Delete a routine by id.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }
        }),
    ]
}

fn routines_path(agent: &Agent, suffix: &str) -> String {
    format!(
        "{}/projects/{}/routines{}",
        agent.cfg.relay_url, agent.cfg.project_id, suffix
    )
}

async fn relay_routines_json(
    agent: &Agent,
    method: reqwest::Method,
    suffix: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let mut req = agent
        .http_client()
        .request(method, routines_path(agent, suffix))
        .bearer_auth(&agent.cfg.box_token);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let r = req.send().await.map_err(|e| e.to_string())?;
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "relay routines{suffix}: HTTP {status} {}",
            crate::relay_error::message(&text)
        ));
    }
    if text.trim().is_empty() {
        return Ok(json!({ "ok": true }));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Dispatch `routines_*` MCP tools.
pub async fn call_tool(agent: &Arc<Agent>, name: &str, args: &Value, chat_hint: &str) -> Value {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_owned);
    match name {
        "routines_list" => match relay_routines_json(agent, reqwest::Method::GET, "", None).await {
            Ok(v) => text_result(v.to_string()),
            Err(e) => err_result(e),
        },
        "routines_create" => {
            let name = s("name").unwrap_or_default();
            let prompt = s("prompt").unwrap_or_default();
            let Some(trigger) = args.get("trigger").cloned() else {
                return err_result("trigger is required");
            };
            if name.trim().is_empty() || prompt.trim().is_empty() {
                return err_result("name and prompt are required");
            }
            let chat_id = s("chat_id").filter(|c| !c.is_empty()).or_else(|| {
                let hint = chat_hint.trim();
                (!hint.is_empty() && hint != "shared").then(|| hint.to_owned())
            });
            let body = json!({
                "name": name.trim(),
                "prompt": prompt.trim(),
                "trigger": trigger,
                "chatId": chat_id,
                "enabled": args.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            });
            match relay_routines_json(agent, reqwest::Method::POST, "", Some(body)).await {
                Ok(v) => text_result(v.to_string()),
                Err(e) => err_result(e),
            }
        }
        "routines_update" => {
            let id = s("id").unwrap_or_default();
            if id.trim().is_empty() {
                return err_result("id is required");
            }
            let mut body = json!({});
            if let Some(o) = body.as_object_mut() {
                if let Some(v) = s("name") {
                    o.insert("name".into(), json!(v));
                }
                if let Some(v) = s("prompt") {
                    o.insert("prompt".into(), json!(v));
                }
                if let Some(v) = s("chat_id") {
                    o.insert("chatId".into(), json!(v));
                }
                if let Some(v) = args.get("trigger") {
                    o.insert("trigger".into(), v.clone());
                }
                if let Some(v) = args.get("enabled").and_then(Value::as_bool) {
                    o.insert("enabled".into(), json!(v));
                }
            }
            match relay_routines_json(agent, reqwest::Method::PATCH, &format!("/{id}"), Some(body))
                .await
            {
                Ok(v) => text_result(v.to_string()),
                Err(e) => err_result(e),
            }
        }
        "routines_delete" => {
            let id = s("id").unwrap_or_default();
            if id.trim().is_empty() {
                return err_result("id is required");
            }
            match relay_routines_json(agent, reqwest::Method::DELETE, &format!("/{id}"), None).await
            {
                Ok(_) => text_result(json!({ "ok": true, "id": id }).to_string()),
                Err(e) => err_result(e),
            }
        }
        other => err_result(format!("unknown routines tool {other}")),
    }
}

impl Agent {
    /// Dispatch one push by kind. Unknown kinds are logged into the T3 store as before so
    /// nothing is lost while other features add their handlers.
    pub async fn handle_push(self: &Arc<Self>, kind: &str, payload: Value) {
        match kind {
            "routine_run" => {
                if let Err(e) = self.handle_routine_run(payload).await {
                    tracing::warn!(error = %e, "routine_run push failed");
                }
            }
            // A chat of this project was opened (a new tab, or a person's row landed in a room
            // this box holds no socket in): open its room now, not at the next listing (#2127,
            // #2120). Same path as the registry pass; a room already open is left alone.
            "chat_open" => {
                let Some(chat_id) = payload.get("chat_id").and_then(Value::as_str) else {
                    tracing::warn!("chat_open push without chat_id");
                    return;
                };
                let mut chat = payload.clone();
                chat["id"] = Value::String(chat_id.to_owned());
                crate::workspace::open_listed_chat(
                    &self.cfg,
                    &self.store,
                    &self.sync,
                    &self.cfg.project_id,
                    &chat,
                    true,
                )
                .await;
                tracing::info!(chat_id, reason = ?payload.get("reason"), "chat_open push: room opened");
            }
            // F5: the library changed on the relay; refetch the index now rather than at the
            // next 60 s tick.
            "skills_changed" => self.refresh_skills().await,
            "project_renamed" => {
                // #1431: the relay renamed this project; speak as the new name from the next
                // turn on (the 3 s poll would catch it too; this is the immediate path).
                if let Some(to) = payload.get("to").and_then(Value::as_str) {
                    if let Some(r) = self.identity.observe(to) {
                        self.on_renamed(&r).await;
                    }
                }
            }
            "channel_message" => {
                if let Err(e) = self.handle_channel_message(payload).await {
                    tracing::warn!(error = %e, "channel_message push failed");
                }
            }
            // `people_chat_open` (#2120): a person's row landed in a people room this box
            // holds no socket in — open it now with the relay's joined_at as the line (never a
            // late-join "now", which would discard the very row that triggered the push).
            "people_agent_added" | "people_chat_open" => {
                if let Some(chat_id) = payload.get("chat_id").and_then(Value::as_str) {
                    if let Err(e) = self.join_people_chat(chat_id).await {
                        tracing::warn!(error = %e, chat_id, kind, "people chat push failed");
                    }
                }
            }
            "people_agent_removed" => {
                if let Some(chat_id) = payload.get("chat_id").and_then(Value::as_str) {
                    self.people_agent_removed(chat_id).await;
                }
            }
            "group_message" => {
                if let Err(e) = self.handle_group_message(payload).await {
                    tracing::warn!(error = %e, "group_message push failed");
                }
            }
            // Agent-to-agent messaging: another agent wrote into this chat / answered this agent /
            // the pair hit its hourly cap.
            "agent_message" => {
                if let Err(e) = self.handle_agent_message(payload).await {
                    tracing::warn!(error = %e, "agent_message push failed");
                }
            }
            "agent_reply" => {
                if let Err(e) = self.handle_agent_reply(payload).await {
                    tracing::warn!(error = %e, "agent_reply push failed");
                }
            }
            "agent_message_paused" => {
                if let Err(e) = self.handle_agent_message_paused(payload).await {
                    tracing::warn!(error = %e, "agent_message_paused push failed");
                }
            }
            // D83: the owner flipped a switch, paused everything, replaced a value or answered
            // a card; the grants come from the relay again at once.
            "secret_access_changed"
            | "secrets_paused"
            | "secret_value_replaced"
            | "secret_decided"
            | "secret_notice"
            | "secret_scrub" => crate::vault::on_push(self, kind, &payload).await,
            // #1544: the owner added or revoked a standing shell grant in Settings.
            "shell_grants_changed" => {
                if let Err(e) = self.refresh_shell_grants().await {
                    tracing::warn!(error = %e, "shell grants: refresh after push failed");
                }
            }
            // The feedback loop opened the issue for a report the agent filed (`submit_feedback`).
            "feedback_issue" => crate::feedback::on_issue_push(self, &payload).await,
            other => {
                tracing::info!(kind = other, push = %payload, "push delivered");
                let _ = self.store.t3_append(
                    "projects",
                    None,
                    None,
                    "push",
                    Some("system"),
                    &json!({ "kind": other, "payload": payload }).to_string(),
                    None,
                );
            }
        }
    }

    /// Poll the relay push queue. Pushes arriving while the box is up (cross-project messages,
    /// routine runs, channel messages) come this way; the ones queued while it slept come with
    /// `box/hello`.
    pub fn spawn_push_poller(self: &Arc<Self>) {
        let agent = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(PUSH_POLL).await;
                let resp = agent
                    .http_client()
                    .get(format!(
                        "{}/projects/{}/pushes",
                        agent.cfg.relay_url, agent.cfg.project_id
                    ))
                    .bearer_auth(&agent.cfg.box_token)
                    .send()
                    .await;
                let Ok(r) = resp else { continue };
                if !r.status().is_success() {
                    continue;
                }
                let Ok(v) = r.json::<Value>().await else {
                    continue;
                };
                for p in v.as_array().cloned().unwrap_or_default() {
                    agent.dispatch_push(p);
                }
            }
        });
    }

    /// Each push runs on its own task; `send`-like work still serialises on the turn lock.
    pub fn dispatch_push(self: &Arc<Self>, push: Value) {
        let kind = push
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let payload = push.get("payload").cloned().unwrap_or(Value::Null);
        let agent = self.clone();
        tokio::spawn(async move { agent.handle_push(&kind, payload).await });
    }

    async fn report_run(
        &self,
        routine_id: &str,
        run_id: &str,
        status: &str,
        message_id: Option<&str>,
        note: Option<&str>,
    ) -> Result<()> {
        let r = self
            .http_client()
            .post(format!(
                "{}/projects/{}/routines/{}/runs/{}",
                self.cfg.relay_url, self.cfg.project_id, routine_id, run_id
            ))
            .bearer_auth(&self.cfg.box_token)
            .json(&json!({ "status": status, "messageId": message_id, "note": note }))
            .send()
            .await?;
        if !r.status().is_success() {
            return Err(anyhow!("run report: HTTP {}", r.status()));
        }
        Ok(())
    }

    /// `routine_run { routineId, runId, chatId, prompt, routineName, cause, clientsConnected }`.
    async fn handle_routine_run(self: &Arc<Self>, payload: Value) -> Result<()> {
        let field = |k: &str| {
            payload
                .get(k)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("routine_run push missing {k}"))
        };
        let routine_id = field("routineId")?;
        let run_id = field("runId")?;
        let chat_id = field("chatId")?;
        let prompt = field("prompt")?;
        let name = payload
            .get("routineName")
            .and_then(Value::as_str)
            .unwrap_or("Routine")
            .to_owned();
        let clients_connected = payload
            .get("clientsConnected")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let sender = format!("routine:{routine_id}");
        tracing::info!(routine = %routine_id, run = %run_id, chat = %chat_id, "routine run");

        if let Err(e) = self
            .report_run(&routine_id, &run_id, "running", None, None)
            .await
        {
            tracing::warn!(error = %e, "routine run: report running");
        }

        // The chat may have been created moments ago by the client; the registry poll opens
        // rooms every 3 s, so open it here when it is not hosted yet.
        let host = match self.sync.host(&chat_id).await {
            Some(h) => h,
            None => self.sync.open_room(&chat_id).await?,
        };
        let msg_id = ulid();
        let cmd = {
            let mut h = host.lock().await;
            let cmd_id = h.doc.append_user_message_from(
                &msg_id,
                &prompt,
                &self.cfg.device_id,
                &sender,
                &[
                    ("routineName", json!(name)),
                    ("routineRunId", json!(run_id)),
                ],
            );
            h.mark_seen(&cmd_id);
            CommandView {
                id: cmd_id,
                kind: "send".into(),
                payload: json!({ "messageId": msg_id }),
                issued_by: self.cfg.device_id.clone(),
                issued_at: now_ms(),
                status: "pending".into(),
            }
        };
        let outcome = self.handle_command(&chat_id, cmd).await;

        // Outcome: the turn result plus the assistant reply to our message.
        let (status, reply_id, note) = {
            let h = host.lock().await;
            let reply = h.doc.messages().into_iter().rev().find(|m| {
                m.role == "assistant"
                    && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(msg_id.as_str())
            });
            match (outcome, reply) {
                (Ok(()), Some(m)) if m.status == "done" => {
                    let excerpt: String = m.text().split_whitespace().collect::<Vec<_>>().join(" ");
                    (
                        "done",
                        Some(m.id),
                        Some(excerpt.chars().take(160).collect::<String>()),
                    )
                }
                (Ok(()), Some(m)) => (
                    "failed",
                    Some(m.id),
                    Some(format!("reply ended with status {}", m.status)),
                ),
                (Ok(()), None) => ("failed", None, Some("no reply was written".to_owned())),
                (Err(e), m) => ("failed", m.map(|m| m.id), Some(e.to_string())),
            }
        };
        if let Err(e) = self
            .report_run(
                &routine_id,
                &run_id,
                status,
                reply_id.as_deref(),
                note.as_deref(),
            )
            .await
        {
            tracing::warn!(error = %e, "routine run: report outcome");
        }

        // Nobody was watching: leave a one-line notice in the pinned tab (D49 picks it up).
        if !clients_connected {
            self.notify_pinned_tab(&chat_id, &sender, &name, status)
                .await;
        }
        Ok(())
    }

    async fn notify_pinned_tab(&self, chat_id: &str, sender: &str, name: &str, status: &str) {
        let chats = self.store.chats().unwrap_or_default();
        let Some(pinned) = chats.iter().find(|c| c.is_pinned) else {
            return;
        };
        let chat_title = chats
            .iter()
            .find(|c| c.id == chat_id)
            .map(|c| c.title.clone())
            .unwrap_or_else(|| "its chat".to_owned());
        let text = if pinned.id == chat_id {
            format!("Routine “{name}” ran while you were away ({status}).")
        } else {
            format!("Routine “{name}” ran while you were away ({status}) → open “{chat_title}”.")
        };
        // The pinned chat's room may be closed for being idle (#2367): the notice opens it,
        // rather than being dropped.
        match self.room(&pinned.id).await {
            Ok(host) => {
                let h = host.lock().await;
                h.doc
                    .append_system_note(&ulid(), sender, &text, &self.cfg.device_id);
            }
            Err(e) => tracing::warn!(error = %e, chat = %pinned.id, "routine notice: no room"),
        }
    }
}
