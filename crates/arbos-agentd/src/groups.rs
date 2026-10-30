//! Group chats on the box (F6, design D69 / S15). The host project's box owns the group chat
//! doc: a user message there is fanned out by the relay to the participants the decision model
//! picks (none, one, several, or all) as a `group_message` push; their answers come back the
//! same way and land in the host chat as `role: project` messages. A participant box receives
//! group traffic in its own "Projects ⇄" tab (`kind: projects`), runs a normal turn, and posts
//! the prose reply back through the relay. Agents address one another only with the main-session
//! `group_send` tool (`mcp.rs` refuses subagents); the relay applies the D37 guards (trace,
//! depth, cycle, rate).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use arbos_core::doc::{CommandView, MessageView};
use arbos_core::ids::{now_ms, ulid};

use crate::agent::Agent;
use crate::responders::{Candidate, Room};
use crate::sync::SharedHost;

/// Title the relay gives the system tab that collects group traffic on a participant box.
pub const PROJECTS_TAB_TITLE: &str = "Projects ⇄";
const PRELUDE_RECENT_MAX: usize = 12;

#[derive(Default)]
pub struct GroupState {
    /// Chat id → kind from the registry, so a turn knows without a relay round-trip whether
    /// its chat is a group.
    kinds: Mutex<HashMap<String, String>>,
    projects_tab: Mutex<Option<String>>,
}

impl GroupState {
    /// The registry kind of a chat (`group`, `people`, …) when known.
    pub fn kind_of(&self, chat_id: &str) -> Option<String> {
        self.kinds.lock().expect("groups").get(chat_id).cloned()
    }
}

/// The group a turn belongs to: the host side (the chat itself is a group) or the participant
/// side (the message arrived from a group through the relay).
#[derive(Debug, Clone)]
pub struct GroupTurn {
    pub host_project_id: String,
    pub group_chat_id: String,
    pub trace_id: String,
    pub depth: u32,
    pub trace: Vec<String>,
    /// Prepended to the prompt so the agent knows the room it speaks in.
    pub prelude: String,
    /// Host side, a user message: the text to fan out to the other participants.
    pub fan_out_user_text: Option<String>,
    /// Participant project ids (host included), for the reply-router decision.
    pub participant_ids: Vec<String>,
    /// `[{id, name, description}, …]` for the responder decision.
    pub participants_detail: Vec<Value>,
    pub title: String,
    /// Host side: what was said in the group before this message (`[{from, text}]`, oldest
    /// first), for the responder decision.
    pub recent: Vec<Value>,
}

fn str_field(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned)
}

fn participant_name(participants: &[Value], id: &str) -> String {
    participants
        .iter()
        .find(|p| p.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|p| p.get("name").and_then(Value::as_str))
        .unwrap_or(id)
        .to_owned()
}

fn sender_label(participants: &[Value], from: &str) -> String {
    match from.strip_prefix("project:") {
        Some(pid) => format!("Project {}", participant_name(participants, pid)),
        None => "the user".to_owned(),
    }
}

fn names(participants: &[Value], host: &str) -> String {
    participants
        .iter()
        .map(|p| {
            let id = p.get("id").and_then(Value::as_str).unwrap_or("");
            let name = p.get("name").and_then(Value::as_str).unwrap_or(id);
            if id == host {
                format!("{name} (host)")
            } else {
                name.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn participant_ids(participants: &[Value]) -> Vec<String> {
    participants
        .iter()
        .filter_map(|p| p.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

/// Project messages since the previous user message, oldest first, for the host's prelude.
fn recent_project_messages<'a>(
    messages: &'a [MessageView],
    current_id: &str,
) -> Vec<&'a MessageView> {
    let mut out: Vec<&MessageView> = Vec::new();
    for m in messages.iter().rev() {
        if m.id == current_id {
            continue;
        }
        match m.role.as_str() {
            "project" => out.push(m),
            "user" => break,
            _ => {}
        }
    }
    out.reverse();
    out
}

/// The last `max` messages before `current_id` as `[{from, text}]`, oldest first, for the
/// responder decision: the user, the host's own agent (`assistant`), and the other Projects.
fn recent_group_context(
    messages: &[MessageView],
    current_id: &str,
    participants: &[Value],
    host_name: &str,
    max: usize,
    chat_id: &str,
) -> Vec<Value> {
    let reply_to = messages
        .iter()
        .find(|m| m.id == current_id)
        .and_then(|m| str_field(&m.rest, "replyTo"));
    let mut lines = Vec::new();
    for m in messages {
        if m.id == current_id {
            break;
        }
        let text = m.text();
        if text.trim().is_empty() {
            continue;
        }
        let from = match m.role.as_str() {
            "user" => "the user".to_owned(),
            "assistant" => format!("Project {host_name}"),
            "project" => str_field(&m.rest, "senderName")
                .map(|n| format!("Project {n}"))
                .unwrap_or_else(|| {
                    sender_label(
                        participants,
                        &str_field(&m.rest, "sender").unwrap_or_default(),
                    )
                }),
            _ => continue,
        };
        lines.push(crate::truncate::WindowLine {
            id: m.id.clone(),
            from,
            text,
            full: reply_to.as_deref() == Some(m.id.as_str()),
        });
    }
    crate::truncate::build_window(chat_id, lines, max).0
}

impl Agent {
    /// Remember each chat's kind and participants from the registry list (`chats` of
    /// `GET /projects/{id}`), called on every chats-index write.
    pub fn note_chats_index(&self, chats: &[Value]) {
        let mut map = HashMap::new();
        for c in chats {
            let Some(id) = str_field(c, "id") else {
                continue;
            };
            let kind = str_field(c, "kind").unwrap_or_else(|| "chat".into());
            if kind == "projects" {
                *self.groups.projects_tab.lock().expect("groups") = Some(id.clone());
            }
            map.insert(id, kind);
        }
        *self.groups.kinds.lock().expect("groups") = map;
    }

    pub(crate) fn chat_kind(&self, chat_id: &str) -> Option<String> {
        self.groups
            .kinds
            .lock()
            .expect("groups")
            .get(chat_id)
            .cloned()
    }

    /// Does `name` (project name, @handle or id) belong to a participant of the group this
    /// agent hosts in `chat_id`? Used by `send_message` to keep in-group words in the group.
    pub(crate) async fn group_participant_named(&self, chat_id: &str, name: &str) -> bool {
        let Ok(group) = self.relay_group(&self.cfg.project_id, chat_id).await else {
            return false;
        };
        let wanted = name.trim().trim_start_matches('@').to_lowercase();
        group
            .get("participants_detail")
            .and_then(Value::as_array)
            .map(|ps| {
                ps.iter().any(|p| {
                    str_field(p, "id").is_some_and(|id| id == name.trim())
                        || str_field(p, "name").is_some_and(|n| {
                            n.to_lowercase() == wanted
                                || n.to_lowercase()
                                    .split_whitespace()
                                    .collect::<Vec<_>>()
                                    .join("-")
                                    == wanted
                        })
                })
            })
            .unwrap_or(false)
    }

    async fn relay_group(&self, host_project_id: &str, group_chat_id: &str) -> Result<Value> {
        let r = self
            .http_client()
            .get(format!(
                "{}/projects/{}/groups/{}",
                self.cfg.relay_url, host_project_id, group_chat_id
            ))
            .bearer_auth(&self.cfg.box_token)
            .send()
            .await?;
        if !r.status().is_success() {
            return Err(anyhow!("group lookup: HTTP {}", r.status()));
        }
        Ok(r.json().await?)
    }

    /// The group a turn speaks in, or `None` for an ordinary chat message. Host side: the chat
    /// is a group hosted here. Participant side: the message carries `groupChatId`.
    pub async fn group_turn_context(
        &self,
        chat_id: &str,
        host: &SharedHost,
        user_msg_id: &str,
    ) -> Option<GroupTurn> {
        let (msg, messages) = {
            let h = host.lock().await;
            (h.doc.message(user_msg_id)?, h.doc.messages())
        };
        let me = self.cfg.project_id.clone();
        if let Some(group_chat_id) = str_field(&msg.rest, "groupChatId") {
            let host_project_id = str_field(&msg.rest, "hostProjectId")?;
            let title = str_field(&msg.rest, "groupTitle").unwrap_or_else(|| "Group".into());
            let participants = msg
                .rest
                .get("participants")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let from = str_field(&msg.rest, "sender").unwrap_or_else(|| "user".into());
            let host_name =
                str_field(&msg.rest, "hostName").unwrap_or_else(|| host_project_id.clone());
            let prelude = format!(
                "[Group “{title}”, hosted by Project {host_name} — participants: {}. This message is from {}. You are Project {} (id `{me}`); your reply is shown to the whole group.]\n\n",
                names(&participants, &host_project_id),
                sender_label(&participants, &from),
                self.name(),
            );
            let ids = participant_ids(&participants);
            return Some(GroupTurn {
                host_project_id,
                group_chat_id,
                trace_id: str_field(&msg.rest, "traceId").unwrap_or_else(ulid),
                depth: msg.rest.get("depth").and_then(Value::as_u64).unwrap_or(1) as u32,
                trace: msg
                    .rest
                    .get("trace")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
                prelude,
                fan_out_user_text: None,
                participant_ids: ids,
                participants_detail: participants,
                title,
                recent: Vec::new(),
            });
        }
        if self.chat_kind(chat_id).as_deref() != Some("group") {
            return None;
        }
        let group = self.relay_group(&me, chat_id).await.ok()?;
        let title = str_field(&group, "title").unwrap_or_else(|| "Group".into());
        let participants = group
            .get("participants_detail")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let from = str_field(&msg.rest, "sender").unwrap_or_else(|| "user".into());
        let mut prelude = format!(
            "[Group “{title}” — participants: {}. You are Project {} (id `{me}`) and host this group; the user and the other Projects' agents read what you write here. This message is from {}.]\n",
            names(&participants, &me),
            self.name(),
            sender_label(&participants, &from),
        );
        let recent = recent_project_messages(&messages, user_msg_id);
        if !recent.is_empty() {
            prelude.push_str("Said in the group since your last turn:\n");
            let lines: Vec<crate::truncate::WindowLine> = recent
                .iter()
                .map(|m| crate::truncate::WindowLine {
                    id: m.id.clone(),
                    from: str_field(&m.rest, "senderName")
                        .or_else(|| str_field(&m.rest, "sender"))
                        .unwrap_or_else(|| "Project".into()),
                    text: m.text(),
                    full: false,
                })
                .collect();
            let (window, _) = crate::truncate::build_window(chat_id, lines, PRELUDE_RECENT_MAX);
            prelude.push_str(&crate::truncate::render_window(&window));
            prelude.push_str(&crate::truncate::window_note(chat_id));
            prelude.push('\n');
        }
        prelude.push('\n');
        let is_user = msg.role == "user" && from == "user";
        let ids = participant_ids(&participants);
        let recent = recent_group_context(
            &messages,
            user_msg_id,
            &participants,
            &self.name(),
            PRELUDE_RECENT_MAX,
            chat_id,
        );
        Some(GroupTurn {
            host_project_id: me,
            group_chat_id: chat_id.to_owned(),
            trace_id: str_field(&msg.rest, "traceId").unwrap_or_else(ulid),
            depth: 0,
            trace: Vec::new(),
            prelude,
            fan_out_user_text: is_user.then(|| msg.text()),
            participant_ids: ids,
            participants_detail: participants,
            title,
            recent,
        })
    }

    async fn relay_send(
        &self,
        gt: &GroupTurn,
        from: &str,
        text: &str,
        to: Option<&str>,
    ) -> Result<Value> {
        let body = json!({
            "from": from,
            "text": text,
            "to": to,
            "traceId": gt.trace_id,
            "depth": gt.depth,
            "trace": gt.trace,
        });
        let r = self
            .http_client()
            .post(format!(
                "{}/projects/{}/groups/{}/send",
                self.cfg.relay_url, gt.host_project_id, gt.group_chat_id
            ))
            .bearer_auth(&self.cfg.box_token)
            .json(&body)
            .send()
            .await?;
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(anyhow!(
                "relay refused group message: HTTP {status} {}",
                v.as_str().unwrap_or("")
            ));
        }
        Ok(v)
    }

    /// Host side: fan the user's message to `targets` (project ids). `None` means every other
    /// participant; an empty list sends nothing.
    pub fn spawn_group_fan_out(
        self: &Arc<Self>,
        gt: &GroupTurn,
        text: &str,
        targets: Option<Vec<String>>,
    ) {
        let agent = self.clone();
        let gt = gt.clone();
        let text = text.to_owned();
        tokio::spawn(async move {
            let sends: Vec<Option<String>> = match targets {
                None => vec![None],
                Some(list) if list.is_empty() => return,
                Some(list) => list.into_iter().map(Some).collect(),
            };
            for to in sends {
                match agent.relay_send(&gt, "user", &text, to.as_deref()).await {
                    Ok(v) => {
                        tracing::info!(
                            group = %gt.group_chat_id,
                            to = to.as_deref().unwrap_or("*"),
                            result = %v,
                            "group: user message fanned out"
                        )
                    }
                    Err(e) => {
                        tracing::warn!(
                            group = %gt.group_chat_id,
                            to = to.as_deref().unwrap_or("*"),
                            error = %e,
                            "group: fan-out failed"
                        )
                    }
                }
            }
        });
    }

    /// Which participant project ids should answer this user message: the judge
    /// (`responders.rs`). When it fails, falls open to every participant (a project group is
    /// the user with their own agents; the people-chat gate fails closed instead).
    pub async fn decide_group_responders(
        &self,
        gt: &GroupTurn,
        user_msg_id: &str,
        text: &str,
    ) -> Vec<String> {
        let known = &gt.participant_ids;
        if known.is_empty() {
            return vec![self.cfg.project_id.clone()];
        }
        let mut candidates: Vec<Candidate> = gt
            .participants_detail
            .iter()
            .filter_map(|p| {
                let id = p.get("id").and_then(Value::as_str)?;
                Some(Candidate {
                    id: id.to_owned(),
                    name: p
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .to_owned(),
                    role: p
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    host: id == gt.host_project_id,
                    mentioned_as: None,
                })
            })
            .collect();
        candidates.sort_by(|a, b| a.id.cmp(&b.id));
        let room = Room {
            kind: "project_group",
            chat_id: gt.group_chat_id.clone(),
            title: gt.title.clone(),
            from: "the user".into(),
            text: text.to_owned(),
            recent: gt.recent.clone(),
            candidates,
            people: Vec::new(),
            reply_to: None,
        };
        match self.decide_responders(&room).await {
            Ok(d) => {
                self.log_responder_decision(&room, user_msg_id, Ok(&d), None);
                d.chosen
            }
            Err(e) => {
                let error = e.to_string();
                self.log_responder_decision(&room, user_msg_id, Err(&error), Some(("ALL", known)));
                known.clone()
            }
        }
    }

    /// The `group_send` MCP tool: the main agent's words to the other participants of the group
    /// its current turn runs in (or to one of them).
    pub async fn group_send_from_turn(&self, text: &str, to: Option<&str>) -> Result<String> {
        let text = text.trim();
        if text.is_empty() {
            return Err(anyhow!("text required"));
        }
        let (chat_id, user_msg_id) = self
            .turn
            .lock()
            .await
            .as_ref()
            .map(|t| (t.chat_id.clone(), t.user_msg_id.clone()))
            .ok_or_else(|| anyhow!("no turn is running"))?;
        let host = self
            .sync
            .host(&chat_id)
            .await
            .ok_or_else(|| anyhow!("no room for chat {chat_id}"))?;
        let gt = self
            .group_turn_context(&chat_id, &host, &user_msg_id)
            .await
            .ok_or_else(|| anyhow!("this chat is not a group chat; group_send only works there"))?;
        let from = format!("project:{}", self.cfg.project_id);
        let v = self.relay_send(&gt, &from, text, to).await?;
        let _ = self.store.t3_append(
            &chat_id,
            None,
            Some(&chat_id),
            "group_send",
            Some("assistant"),
            text,
            Some(&json!({ "group": gt.group_chat_id, "to": to, "result": v })),
        );
        Ok(json!({ "status": "sent", "delivered": v.get("delivered"), "refused": v.get("refused") })
            .to_string())
    }

    /// A `group_message` push: on the host box a participant's words land in the group chat;
    /// on a participant box the message runs a turn in the "Projects ⇄" tab and the prose
    /// reply goes back to the host.
    pub async fn handle_group_message(self: &Arc<Self>, payload: Value) -> Result<()> {
        let host_project_id = str_field(&payload, "hostProjectId")
            .ok_or_else(|| anyhow!("group_message missing hostProjectId"))?;
        let group_chat_id = str_field(&payload, "groupChatId")
            .ok_or_else(|| anyhow!("group_message missing groupChatId"))?;
        let text = str_field(&payload, "text").unwrap_or_default();
        if text.trim().is_empty() {
            return Err(anyhow!("group_message without text"));
        }
        let from = str_field(&payload, "from").unwrap_or_else(|| "user".into());
        tracing::info!(group = %group_chat_id, %from, host = host_project_id == self.cfg.project_id, "group message");
        if host_project_id == self.cfg.project_id {
            self.host_receive(&group_chat_id, &from, &text, &payload)
                .await
        } else {
            self.participant_receive(&host_project_id, &group_chat_id, &from, &text, &payload)
                .await
        }
    }

    fn message_extras(payload: &Value) -> Vec<(&'static str, Value)> {
        let mut extra = vec![
            ("groupChatId", payload["groupChatId"].clone()),
            ("groupTitle", payload["groupTitle"].clone()),
            ("hostProjectId", payload["hostProjectId"].clone()),
            ("hostName", payload["hostName"].clone()),
            ("traceId", payload["traceId"].clone()),
            ("depth", payload["depth"].clone()),
            ("trace", payload["trace"].clone()),
            ("participants", payload["participants"].clone()),
        ];
        if let Some(n) = payload.get("fromName").filter(|v| !v.is_null()) {
            extra.push(("senderName", n.clone()));
        }
        if let Some(i) = payload.get("fromIcon").filter(|v| !v.is_null()) {
            extra.push(("senderIcon", i.clone()));
        }
        if let Some(t) = payload.get("to").filter(|v| !v.is_null()) {
            extra.push(("to", t.clone()));
        }
        extra
    }

    pub(crate) async fn room(&self, chat_id: &str) -> Result<SharedHost> {
        match self.sync.host(chat_id).await {
            Some(h) => Ok(h),
            None => self.sync.open_room(chat_id).await,
        }
    }

    /// Host box: another Project spoke in our group. A reply is appended as it is; a message
    /// addressed to this Project also drives a turn.
    async fn host_receive(
        self: &Arc<Self>,
        group_chat_id: &str,
        from: &str,
        text: &str,
        payload: &Value,
    ) -> Result<()> {
        let host = self.room(group_chat_id).await?;
        let addressed_to_me = str_field(payload, "to").as_deref() == Some(&self.cfg.project_id)
            && !payload
                .get("reply")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let msg_id = ulid();
        let extras = Self::message_extras(payload);
        let cmd = {
            let mut h = host.lock().await;
            let cmd_id = h.doc.append_project_message(
                &msg_id,
                text,
                &self.cfg.device_id,
                from,
                &extras,
                addressed_to_me,
            );
            cmd_id.map(|id| {
                h.mark_seen(&id);
                CommandView {
                    id,
                    kind: "send".into(),
                    payload: json!({ "messageId": msg_id }),
                    issued_by: self.cfg.device_id.clone(),
                    issued_at: now_ms(),
                    status: "pending".into(),
                }
            })
        };
        let _ = self.store.touch_chat(group_chat_id);
        let _ = self.store.t3_append(
            group_chat_id,
            None,
            Some(group_chat_id),
            "project",
            Some("project"),
            text,
            Some(&json!({ "message_id": msg_id, "sender": from })),
        );
        if let Some(cmd) = cmd {
            self.handle_command(group_chat_id, cmd).await?;
        }
        Ok(())
    }

    /// The "Projects ⇄" tab of this Project, created through the relay when missing.
    pub(crate) async fn ensure_projects_tab(&self) -> Result<String> {
        if let Some(id) = self.groups.projects_tab.lock().expect("groups").clone() {
            return Ok(id);
        }
        // The "Projects ⇄" system tab is not a plain extra chat: agent-hosted groups (F6) keep
        // working with multi-chat off, and their traffic must not spill into the owner's one
        // chat, so the tab is created regardless of the flag (the relay serves it too).
        let r = self
            .http_client()
            .post(format!(
                "{}/projects/{}/chats",
                self.cfg.relay_url, self.cfg.project_id
            ))
            .bearer_auth(&self.cfg.box_token)
            .json(&json!({ "kind": "projects", "title": PROJECTS_TAB_TITLE }))
            .send()
            .await?;
        if !r.status().is_success() {
            return Err(anyhow!("projects tab: HTTP {}", r.status()));
        }
        let v: Value = r.json().await?;
        let v = v.get("chat").cloned().unwrap_or(v);
        let id = str_field(&v, "id").ok_or_else(|| anyhow!("projects tab: no id"))?;
        let _ = self.store.upsert_chat(
            &id,
            str_field(&v, "title")
                .as_deref()
                .unwrap_or(PROJECTS_TAB_TITLE),
            false,
            v.get("created_at")
                .and_then(Value::as_i64)
                .unwrap_or(now_ms()),
        );
        *self.groups.projects_tab.lock().expect("groups") = Some(id.clone());
        Ok(id)
    }

    /// Participant box: run the message as a turn in the "Projects ⇄" tab and send the prose
    /// reply back to the host chat.
    async fn participant_receive(
        self: &Arc<Self>,
        host_project_id: &str,
        group_chat_id: &str,
        from: &str,
        text: &str,
        payload: &Value,
    ) -> Result<()> {
        let chat_id = self.ensure_projects_tab().await?;
        let host = self.room(&chat_id).await?;
        let msg_id = ulid();
        let extras = Self::message_extras(payload);
        let cmd = {
            let mut h = host.lock().await;
            let cmd_id = if from == "user" {
                h.doc
                    .append_user_message_from(&msg_id, text, &self.cfg.device_id, "user", &extras)
            } else {
                h.doc
                    .append_project_message(&msg_id, text, &self.cfg.device_id, from, &extras, true)
                    .expect("send command requested")
            };
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
        let reply = {
            let h = host.lock().await;
            h.doc.messages().into_iter().rev().find(|m| {
                m.role == "assistant"
                    && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(msg_id.as_str())
            })
        };
        let reply_text = match (outcome, reply) {
            (Ok(()), Some(m)) if m.status == "done" => m.text().trim().to_owned(),
            (Ok(()), Some(m)) => {
                return Err(anyhow!("reply ended with status {}", m.status));
            }
            (Ok(()), None) => return Err(anyhow!("no reply was written")),
            (Err(e), _) => return Err(e),
        };
        if reply_text.is_empty() {
            return Ok(());
        }
        let r = self
            .http_client()
            .post(format!(
                "{}/projects/{}/groups/{}/reply",
                self.cfg.relay_url, host_project_id, group_chat_id
            ))
            .bearer_auth(&self.cfg.box_token)
            .json(&json!({
                "fromProjectId": self.cfg.project_id,
                "text": reply_text,
                "traceId": payload.get("traceId"),
                "depth": payload.get("depth"),
            }))
            .send()
            .await?;
        if !r.status().is_success() {
            return Err(anyhow!("group reply: HTTP {}", r.status()));
        }
        tracing::info!(group = %group_chat_id, chars = reply_text.len(), "group: reply sent to host");
        Ok(())
    }
}
