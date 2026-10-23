//! Draft-and-approve on the box (feature `approve`, design §10 / S18): post an `approval` card
//! into the chat doc, wait for the client's `approve` / `reject` command, update the card, and
//! write `approval_requested` / `approval_decided` audit entries (box `audit.jsonl` + relay
//! mirror). Three ways a caller waits:
//!
//! - **inline**: `request_approval` blocks the calling tool until the decision (gated MCP tools);
//! - **polled**: the pi-ext `tool_call` hook posts the card and polls `/hook/approval-status`;
//! - **deferred**: `draft_message` posts the card and ends the turn; the decision dispatches the
//!   draft and prompts the main agent with the outcome as a `[Arbos]` notice.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tokio::sync::{oneshot, Mutex};

use arbos_core::approval::{
    classify_tool, describe_tool, may_dispatch, ApprovalOutcome, ApprovalRequest, ApprovalState,
    Risk, APPROVAL_TTL_MS,
};
use arbos_core::doc::MessageStatus;
use arbos_core::ids::{now_ms, ulid};

use crate::agent::{Agent, TurnKind};
use crate::connectors;

/// A re-run of a gated call is pre-approved for this long after the user approved it late.
const GRANT_TTL_MS: i64 = 10 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitMode {
    Inline,
    Polled,
    Deferred,
}

pub struct PendingApproval {
    pub chat_id: String,
    pub msg_id: String,
    pub in_reply_to: String,
    pub request: ApprovalRequest,
    pub mode: WaitMode,
    pub waiter: Option<oneshot::Sender<ApprovalOutcome>>,
}

struct Grant {
    tool: String,
    args_hash: String,
    approval_id: String,
    until: i64,
}

#[derive(Default)]
pub struct Approvals {
    pending: Mutex<HashMap<String, PendingApproval>>,
    grants: Mutex<Vec<Grant>>,
}

fn args_hash(tool: &str, args: &Value) -> String {
    use sha2::Digest;
    let s = format!("{tool}\n{}", canonical(args));
    hex::encode(sha2::Sha256::digest(s.as_bytes()))[..16].to_owned()
}

/// Stable text for hashing: serde_json keeps map order as inserted, so sort keys first.
fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", k, canonical(&m[k])))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        other => other.to_string(),
    }
}

fn timeout_secs() -> u64 {
    std::env::var("ARBOS_APPROVAL_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or((APPROVAL_TTL_MS / 1000) as u64)
}

impl Agent {
    /// Chat a card belongs to when the caller only knows a session id.
    /// A request raised in a people chat (a DM or a group this agent is in) is answered where
    /// it was asked: the owner reads that chat, and a card that went to the agent's own pinned
    /// chat instead was never seen (#1440). Otherwise prefer the pinned ("main") chat so access
    /// / approval requests are not buried in a side thread the owner is not looking at; fall
    /// back to the running turn, a known chat id, then any chat.
    pub async fn approval_chat(&self, hint: &str) -> Option<String> {
        if self.people.is_people_chat(hint) {
            return Some(hint.to_owned());
        }
        if let Some(t) = self.turn.lock().await.as_ref() {
            if self.people.is_people_chat(&t.chat_id) {
                return Some(t.chat_id.clone());
            }
        }
        let chats = self.store.chats().ok().unwrap_or_default();
        if let Some(pinned) = chats.iter().find(|c| c.is_pinned) {
            return Some(pinned.id.clone());
        }
        if let Some(t) = self.turn.lock().await.as_ref() {
            return Some(t.chat_id.clone());
        }
        if let Ok(Some(c)) = self.store.chat(hint) {
            return Some(c.id);
        }
        chats.first().map(|c| c.id.clone())
    }

    /// Append one line to `/work/.arbos/audit.jsonl` and mirror it to the relay (`POST
    /// /projects/{id}/audit`, fire-and-forget).
    pub fn append_audit(self: &Arc<Self>, kind: &str, mut fields: Value) {
        if let Some(o) = fields.as_object_mut() {
            o.insert("ts".into(), json!(now_ms()));
            o.insert("kind".into(), json!(kind));
            o.insert("source".into(), json!("agentd"));
            // People chats: which person's request this action serves (owner or guest).
            if let Some(a) = self.people.current_asker() {
                o.entry("asked_by").or_insert(json!(a.handle));
                o.entry("asked_by_role").or_insert(json!(a.role));
            }
        }
        let path = self.cfg.audit_path();
        if let Some(p) = path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let _ = writeln!(f, "{fields}");
        }
        let me = self.clone();
        tokio::spawn(async move {
            let _ = me
                .http_client()
                .post(format!(
                    "{}/projects/{}/audit",
                    me.cfg.relay_url, me.cfg.project_id
                ))
                .bearer_auth(&me.cfg.box_token)
                .json(&fields)
                .send()
                .await;
        });
    }

    /// Post the card and register the pending entry. Returns the approval id.
    pub async fn begin_approval(
        self: &Arc<Self>,
        chat_id: &str,
        req: ApprovalRequest,
        mode: WaitMode,
        waiter: Option<oneshot::Sender<ApprovalOutcome>>,
    ) -> Result<String> {
        let host = self
            .sync
            .host(chat_id)
            .await
            .ok_or_else(|| anyhow!("approval: chat {chat_id} has no doc"))?;
        let approval_id = ulid();
        let expires_at = now_ms() + APPROVAL_TTL_MS;
        // Attach to the running turn's message when the card belongs to it; otherwise the card
        // gets a message of its own (`turn.kind = "approval"`).
        let turn_msg = {
            let tl = self.turn.lock().await;
            tl.as_ref()
                .filter(|t| t.chat_id == chat_id)
                .map(|t| (t.assistant_msg_id.clone(), t.user_msg_id.clone()))
        };
        let (msg_id, in_reply_to) = {
            let h = host.lock().await;
            let (msg_id, in_reply_to, own) = match turn_msg {
                Some((m, u)) => (m, u, false),
                None => {
                    let last_user = h
                        .doc
                        .messages()
                        .iter()
                        .rev()
                        .find(|m| m.role == "user")
                        .map(|m| m.id.clone())
                        .unwrap_or_default();
                    let id = ulid();
                    h.doc
                        .start_assistant_message(&id, &last_user, &self.cfg.device_id);
                    h.doc.set_message_fields(
                        &id,
                        &[
                            ("turn", json!({ "kind": "approval" })),
                            ("decision", json!("approval")),
                        ],
                    );
                    (id, last_user, true)
                }
            };
            h.doc
                .add_approval_part(&msg_id, &approval_id, &req, expires_at);
            h.doc
                .add_status_part(&msg_id, "waiting_approval", &req.title);
            h.doc
                .set_message_fields(&msg_id, &[("waitingApproval", json!(true))]);
            if own {
                h.doc.finish_message(&msg_id, MessageStatus::Done);
            }
            (msg_id, in_reply_to)
        };
        let _ = self.store.insert_approval(
            &approval_id,
            chat_id,
            &msg_id,
            &req.title,
            req.risk.as_str(),
            &req.payload,
            expires_at,
        );
        let _ = self.store.t3_append(
            chat_id,
            None,
            Some(chat_id),
            "approval",
            Some("system"),
            &format!("approval requested: {} — {}", req.title, req.summary),
            Some(&json!({ "approval_id": approval_id, "risk": req.risk.as_str() })),
        );
        self.append_audit(
            "approval_requested",
            json!({
                "approval_id": approval_id, "chat_id": chat_id, "message_id": msg_id,
                "title": req.title, "summary": req.summary, "risk": req.risk.as_str(),
                "payload_kind": req.payload.get("kind").cloned().unwrap_or(Value::Null),
                "input_hash": args_hash("approval", &req.payload), "expires_at": expires_at,
            }),
        );
        tracing::info!(approval_id, chat_id, title = %req.title, risk = req.risk.as_str(), ?mode, "approval requested");
        self.approvals.pending.lock().await.insert(
            approval_id.clone(),
            PendingApproval {
                chat_id: chat_id.to_owned(),
                msg_id,
                in_reply_to,
                request: req,
                mode,
                waiter,
            },
        );
        // Working line / quick-reply status: "Waiting for approval", not the gated tool's
        // label ("Removing files") — nothing has run yet.
        self.set_approval_wait_activity(chat_id, true).await;
        Ok(approval_id)
    }

    /// Activity while a card of `chat_id` waits for Approve/Reject (or clear when none do).
    async fn set_approval_wait_activity(&self, chat_id: &str, waiting: bool) {
        let label = waiting.then(|| "Waiting for approval".to_owned());
        let (msg_id, row_closed) = {
            let mut tl = self.turn.lock().await;
            let Some(t) = tl.as_mut().filter(|t| t.chat_id == chat_id) else {
                return;
            };
            if t.activity == label {
                return;
            }
            t.activity = label.clone();
            (t.assistant_msg_id.clone(), t.row_closed)
        };
        if !row_closed {
            if let Some(host) = self.sync.host(chat_id).await {
                let h = host.lock().await;
                h.doc.set_activity(&msg_id, label.as_deref());
            }
        }
        self.post_activity(chat_id, label);
    }

    /// The shared entry point (brief §4): write the card, wait for `approve`/`reject` (or the
    /// 24 h expiry / `ARBOS_APPROVAL_TIMEOUT_SECS`), update its state, audit the decision.
    pub async fn request_approval(
        self: &Arc<Self>,
        chat_id: &str,
        req: ApprovalRequest,
    ) -> ApprovalOutcome {
        let (tx, rx) = oneshot::channel();
        let approval_id = match self
            .begin_approval(chat_id, req, WaitMode::Inline, Some(tx))
            .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(error = %e, "approval card could not be posted; refusing");
                return ApprovalOutcome::Rejected {
                    reason: Some(format!("approval card could not be posted: {e}")),
                };
            }
        };
        match tokio::time::timeout(Duration::from_secs(timeout_secs()), rx).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => ApprovalOutcome::Expired,
            Err(_) => {
                let _ = self
                    .decide_approval(&approval_id, ApprovalOutcome::Expired, "arbos:timeout")
                    .await;
                ApprovalOutcome::Expired
            }
        }
    }

    /// Apply a decision: store (first decision wins), doc part, audit, then whoever is waiting.
    pub async fn decide_approval(
        self: &Arc<Self>,
        approval_id: &str,
        outcome: ApprovalOutcome,
        decided_by: &str,
    ) -> Result<()> {
        let state = outcome.state();
        let reason = match &outcome {
            ApprovalOutcome::Rejected { reason } => reason.clone(),
            ApprovalOutcome::Approved | ApprovalOutcome::Expired => None,
        };
        let row = self
            .store
            .approval(approval_id)?
            .ok_or_else(|| anyhow!("unknown approval {approval_id}"))?;
        if !self.store.decide_approval(
            approval_id,
            state.as_str(),
            Some(decided_by),
            reason.as_deref(),
        )? {
            return Err(anyhow!("approval {approval_id} is already {}", row.state));
        }
        let pending = self.approvals.pending.lock().await.remove(approval_id);
        let (chat_id, msg_id) = match &pending {
            Some(p) => (p.chat_id.clone(), p.msg_id.clone()),
            None => (row.chat_id.clone(), row.message_id.clone()),
        };
        if let Some(host) = self.sync.host(&chat_id).await {
            let h = host.lock().await;
            h.doc
                .set_approval_state(approval_id, state, Some(decided_by), reason.as_deref());
            let still_pending = h
                .doc
                .message(&msg_id)
                .map(|m| {
                    m.parts.iter().any(|p| {
                        p.get("kind").and_then(Value::as_str) == Some("approval")
                            && p.get("state").and_then(Value::as_str) == Some("pending")
                    })
                })
                .unwrap_or(false);
            if !still_pending {
                h.doc
                    .set_message_fields(&msg_id, &[("waitingApproval", json!(false))]);
            }
        }
        // Drop the waiting activity once this chat has no undecided cards left.
        if !self.approval_pending_for(&chat_id).await {
            self.set_approval_wait_activity(&chat_id, false).await;
        }
        let _ = self.store.t3_append(
            &chat_id,
            None,
            Some(&chat_id),
            "approval",
            Some("user"),
            &format!(
                "approval {}: {}{}",
                state.as_str(),
                row.title,
                reason
                    .as_deref()
                    .map(|r| format!(" — {r}"))
                    .unwrap_or_default()
            ),
            Some(&json!({ "approval_id": approval_id, "decided_by": decided_by })),
        );
        self.append_audit(
            "approval_decided",
            json!({
                "approval_id": approval_id, "chat_id": chat_id, "message_id": msg_id,
                "title": row.title, "risk": row.risk, "state": state.as_str(),
                "decided_by": decided_by, "reason": reason,
                "payload_kind": row.payload.get("kind").cloned().unwrap_or(Value::Null),
            }),
        );
        tracing::info!(
            approval_id,
            state = state.as_str(),
            decided_by,
            "approval decided"
        );

        let request = pending
            .as_ref()
            .map(|p| p.request.clone())
            .unwrap_or(ApprovalRequest {
                title: row.title.clone(),
                summary: String::new(),
                risk: Risk::parse(&row.risk),
                payload: row.payload.clone(),
            });
        let in_reply_to = pending
            .as_ref()
            .map(|p| p.in_reply_to.clone())
            .unwrap_or_default();
        let deferred = match pending {
            Some(PendingApproval {
                mode: WaitMode::Inline,
                waiter: Some(tx),
                ..
            }) => {
                // The gated tool is still blocked on this: it continues inline.
                tx.send(outcome.clone()).is_err()
            }
            Some(PendingApproval {
                mode: WaitMode::Polled,
                ..
            }) => false,
            Some(PendingApproval {
                mode: WaitMode::Inline | WaitMode::Deferred,
                ..
            })
            | None => true,
        };
        if deferred && state != ApprovalState::Expired {
            let me = self.clone();
            let id = approval_id.to_owned();
            tokio::spawn(async move {
                me.follow_up(&chat_id, &in_reply_to, &id, request, outcome)
                    .await;
            });
        }
        Ok(())
    }

    /// Late or deferred decision: dispatch what was approved, then let the main agent tell the user.
    async fn follow_up(
        self: &Arc<Self>,
        chat_id: &str,
        in_reply_to: &str,
        approval_id: &str,
        request: ApprovalRequest,
        outcome: ApprovalOutcome,
    ) {
        let kind = request
            .payload
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let title = request.title.clone();
        let mut prompt = match &outcome {
            ApprovalOutcome::Approved => {
                format!("[Arbos] The user approved \"{title}\" (approval {approval_id}).")
            }
            ApprovalOutcome::Rejected { reason } => format!(
                "[Arbos] The user rejected \"{title}\" (approval {approval_id}){}.",
                reason
                    .as_deref()
                    .filter(|r| !r.trim().is_empty())
                    .map(|r| format!(" with the reason: {r}"))
                    .unwrap_or_default()
            ),
            ApprovalOutcome::Expired => return,
        };
        match (kind.as_str(), &outcome) {
            ("draft_message", ApprovalOutcome::Approved) => {
                match self
                    .dispatch_draft(chat_id, approval_id, &request.payload)
                    .await
                {
                    Ok(note) => prompt.push_str(&format!(" {note}")),
                    Err(e) => prompt.push_str(&format!(" Sending failed: {e}.")),
                }
                prompt.push_str(" Route `reply` and confirm to the user in one or two sentences what was sent and to whom.");
            }
            ("draft_message", ApprovalOutcome::Rejected { .. }) => {
                prompt.push_str(" Nothing was sent. Route `reply`, acknowledge in one sentence, and ask what to change if that helps.");
            }
            ("tool_call", ApprovalOutcome::Approved) => {
                let tool = request
                    .payload
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let args = request.payload.get("input").cloned().unwrap_or(Value::Null);
                self.approvals.grants.lock().await.push(Grant {
                    tool: tool.clone(),
                    args_hash: args_hash(&tool, &args),
                    approval_id: approval_id.to_owned(),
                    until: now_ms() + GRANT_TTL_MS,
                });
                prompt.push_str(&format!(" The original `{tool}` call was not executed because the turn had already ended. Route `act`, call it again now with exactly the same arguments (it is pre-approved for 10 minutes), then tell the user the result."));
            }
            ("tool_call", ApprovalOutcome::Rejected { .. }) => {
                prompt.push_str(" The call was not executed. Route `reply` and tell the user; do not retry it unchanged.");
            }
            (_, ApprovalOutcome::Approved) => {
                prompt.push_str(" Route `reply` and continue with what the approval unblocked, telling the user what you are doing.");
            }
            (_, ApprovalOutcome::Rejected { .. }) => {
                prompt.push_str(" Route `reply` and acknowledge in one sentence.");
            }
            (_, ApprovalOutcome::Expired) => return,
        }
        if let Err(e) = self
            .run_approval_turn(chat_id, in_reply_to, approval_id, &title, &prompt)
            .await
        {
            tracing::warn!(error = %e, approval_id, "approval follow-up turn failed");
        }
    }

    /// Deliver an approved draft through the connector that carries its channel
    /// (`connectors::route_draft`; the broker call is not gated again, the card was the gate).
    /// Without a matching connector the draft lands in the box's `outbox` table and the note
    /// says so. The returned note is what the main agent is told to relay to the user.
    async fn dispatch_draft(
        self: &Arc<Self>,
        chat_id: &str,
        approval_id: &str,
        payload: &Value,
    ) -> Result<String> {
        let s = |k: &str| {
            payload
                .get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        let (channel, to, subject, body) = (s("channel"), s("to"), s("subject"), s("body"));
        let connected = match connectors::connected_providers(self).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "connector list unavailable; draft goes to the outbox");
                Vec::new()
            }
        };
        let subject_opt = if subject.is_empty() {
            None
        } else {
            Some(subject.as_str())
        };
        if let Some((provider, action, args)) =
            connectors::route_draft(&channel, &to, subject_opt, &body, &connected)
        {
            let reply = connectors::send_through_relay(self, chat_id, provider, action, &args)
                .await
                .map_err(|e| anyhow!("{e}"))?;
            let mock = serde_json::from_str::<Value>(&reply)
                .ok()
                .and_then(|v| v.get("mock").and_then(Value::as_bool))
                .unwrap_or(false);
            self.append_audit(
                "draft_dispatched",
                json!({
                    "approval_id": approval_id, "chat_id": chat_id, "channel": channel,
                    "provider": provider, "action": action, "to": to, "mock": mock,
                }),
            );
            return Ok(format!(
                "Arbos sent the {channel} to {to} through the {provider} connector ({provider}.{action}){}. Say exactly that.",
                if mock {
                    "; the relay runs connectors in mock mode, so it was recorded in the relay's connector outbox instead of leaving the machine"
                } else {
                    ""
                }
            ));
        }
        let id = self.store.outbox_insert(
            approval_id,
            &channel,
            &to,
            if subject.is_empty() {
                None
            } else {
                Some(&subject)
            },
            &body,
        )?;
        Ok(format!(
            "Arbos recorded the {channel} to {to} in this box's outbox (entry {id}); no connected connector carries {channel} (Settings → Connectors), so it was not delivered externally. Say exactly that."
        ))
    }

    async fn run_approval_turn(
        self: &Arc<Self>,
        chat_id: &str,
        in_reply_to: &str,
        approval_id: &str,
        title: &str,
        prompt: &str,
    ) -> Result<()> {
        let host = self
            .sync
            .host(chat_id)
            .await
            .ok_or_else(|| anyhow!("chat {chat_id} has no doc"))?;
        let in_reply_to = if in_reply_to.is_empty() {
            host.lock()
                .await
                .doc
                .messages()
                .iter()
                .rev()
                .find(|m| m.role == "user")
                .map(|m| m.id.clone())
                .unwrap_or_default()
        } else {
            in_reply_to.to_owned()
        };
        let kind = TurnKind::ApprovalOutcome {
            approval_id: approval_id.to_owned(),
            title: title.to_owned(),
        };
        self.run_notice_turn(chat_id, &host, &in_reply_to, prompt, kind)
            .await
    }

    /// Gate for MCP tool dispatch (`mcp.rs`): `Ok` means run it, `Err` carries the refusal text.
    /// A risky call runs only after an `Approved` outcome (`arbos_core::approval::may_dispatch`).
    pub async fn gate_tool(
        self: &Arc<Self>,
        tool: &str,
        args: &Value,
        chat_hint: &str,
    ) -> Result<(), String> {
        let Some(hit) = classify_tool(tool, args) else {
            return Ok(());
        };
        if let Some(granted) = self.take_grant(tool, args).await {
            tracing::info!(tool, approval_id = granted, "pre-approved call");
            return Ok(());
        }
        let Some(chat_id) = self.approval_chat(chat_hint).await else {
            return Err("no chat to post the approval card in".into());
        };
        let (title, summary) = describe_tool(tool, args, &hit);
        let req = ApprovalRequest {
            title,
            summary,
            risk: hit.risk,
            payload: json!({ "kind": "tool_call", "tool": tool, "input": args, "class": hit.class, "why": hit.why }),
        };
        let outcome = self.request_approval(&chat_id, req).await;
        if may_dispatch(Some(&outcome)) {
            return Ok(());
        }
        Err(match outcome {
            ApprovalOutcome::Rejected { reason } => format!(
                "the user rejected this action{}; do not retry it unchanged",
                reason
                    .filter(|r| !r.trim().is_empty())
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default()
            ),
            ApprovalOutcome::Expired => "the approval expired before the user decided".into(),
            ApprovalOutcome::Approved => unreachable!("approved outcomes dispatch"),
        })
    }

    /// Consume a late-approval grant for an identical re-run (tool + canonical args hash).
    pub(crate) async fn take_grant(&self, tool: &str, args: &Value) -> Option<String> {
        let now = now_ms();
        let h = args_hash(tool, args);
        let mut grants = self.approvals.grants.lock().await;
        grants.retain(|g| g.until > now);
        let ix = grants
            .iter()
            .position(|g| g.tool == tool && g.args_hash == h)?;
        Some(grants.remove(ix).approval_id)
    }

    /// `/hook/approval` (pi-ext `tool_call`): classify a harness tool call; risky ones get a card
    /// and the extension polls `/hook/approval-status` until it is decided.
    pub async fn hook_approval(
        self: &Arc<Self>,
        tool: &str,
        input: &Value,
        session_id: &str,
        role: &str,
    ) -> Value {
        let Some(hit) = classify_tool(tool, input) else {
            return json!({ "ok": true, "state": "allowed" });
        };
        if let Some(granted) = self.take_grant(tool, input).await {
            return json!({ "ok": true, "state": "allowed", "approvalId": granted });
        }
        let Some(chat_id) = self.approval_chat(session_id).await else {
            return json!({ "ok": false, "error": "no chat to post the approval card in" });
        };
        let (title, summary) = describe_tool(tool, input, &hit);
        let req = ApprovalRequest {
            title: title.clone(),
            summary,
            risk: hit.risk,
            payload: json!({ "kind": "tool_call", "tool": tool, "input": input, "class": hit.class, "why": hit.why, "session_id": session_id, "role": role }),
        };
        match self
            .begin_approval(&chat_id, req, WaitMode::Polled, None)
            .await
        {
            Ok(id) => {
                json!({ "ok": true, "state": "pending", "approvalId": id, "title": title, "why": hit.why })
            }
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        }
    }

    /// True while any card of `chat_id` is undecided (the turn's clocks pause meanwhile).
    pub async fn approval_pending_for(&self, chat_id: &str) -> bool {
        self.approvals
            .pending
            .lock()
            .await
            .values()
            .any(|p| p.chat_id == chat_id)
    }

    /// `/hook/approval-abandon`: the extension stopped polling (its wait cap passed) and the
    /// blocked call was not executed. Expire the card so the UI surfaces the timeout promptly
    /// instead of leaving a pending card and a "still working" status for hours.
    pub async fn abandon_approval(self: &Arc<Self>, approval_id: &str) -> Value {
        match self
            .decide_approval(approval_id, ApprovalOutcome::Expired, "arbos:timeout")
            .await
        {
            Ok(()) => json!({ "ok": true, "state": "expired" }),
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        }
    }

    pub async fn approval_status(&self, approval_id: &str) -> Value {
        match self.store.approval(approval_id) {
            Ok(Some(r)) => {
                json!({ "ok": true, "state": r.state, "reason": r.reason, "decidedBy": r.decided_by })
            }
            Ok(None) => json!({ "ok": false, "error": "unknown approval" }),
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        }
    }

    /// Sweep pending cards past `expiresAt` (every minute from `main.rs`).
    pub async fn expire_approvals(self: &Arc<Self>) {
        let now = now_ms();
        for row in self.store.pending_approvals().unwrap_or_default() {
            if row.expires_at <= now {
                let _ = self
                    .decide_approval(&row.id, ApprovalOutcome::Expired, "arbos:expiry")
                    .await;
            }
        }
    }
}
