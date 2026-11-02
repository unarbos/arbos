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
    classify_tool_with, describe_tool, may_dispatch, ApprovalOutcome, ApprovalRequest,
    ApprovalState, PathClass, Risk, RiskMatch, ShellGrant, APPROVAL_TTL_MS, GRANTABLE_CLASSES,
};

/// The box's view for the classifier (#1719): a truncation idiom (`: > f`, `> f`,
/// `cp /dev/null f`) deletes something only if `f` already exists with content. agentd runs
/// on the box, so it can look. Unresolvable paths (`$D/x`) do not exist → not a delete.
fn file_has_content(path: &str) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

fn classify_tool(tool: &str, args: &Value) -> Option<RiskMatch> {
    classify_tool_with(tool, args, &file_has_content)
}
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
    /// Standing shell grants from the relay (#1544), refreshed on `shell_grants_changed`.
    shell_grants: Mutex<Vec<ShellGrant>>,
}

/// `approve` payload field a card option B/C sets: the path class to allow from now on.
pub const GRANT_FIELD: &str = "grant";

/// The standing options a destructive-shell card offers, in order, for the card renderer:
/// `[{ path_class, label, detail }]`. Empty for anything a standing grant cannot cover.
pub fn grant_options(hit: &RiskMatch) -> Vec<Value> {
    let Some(class) = hit.path_class else {
        return Vec::new();
    };
    if !GRANTABLE_CLASSES.contains(&hit.class) {
        return Vec::new();
    }
    let mut out = Vec::new();
    // The narrowest class that covers this command, then "anywhere".
    if class != PathClass::Anywhere {
        out.push(json!({ "path_class": class.as_str(), "label": class.label(), "detail": class.describe() }));
    }
    out.push(json!({ "path_class": PathClass::Anywhere.as_str(), "label": PathClass::Anywhere.label(), "detail": PathClass::Anywhere.describe() }));
    out
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
                o.entry("asked_by_role").or_insert(json!(a.role()));
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
        // An optional card (the secret hand-off's "that looks like a key" offer) asks nothing
        // of the turn: no waiting status, no composer lock, no "Waiting for approval" line.
        let optional = req
            .payload
            .get("optional")
            .and_then(Value::as_bool)
            .unwrap_or(false);
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
            if !optional {
                h.doc
                    .add_status_part(&msg_id, "waiting_approval", &req.title);
                h.doc
                    .set_message_fields(&msg_id, &[("waitingApproval", json!(true))]);
            }
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
        if !optional {
            self.set_approval_wait_activity(chat_id, true).await;
        }
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

    /// An `approve` command from a card: when it carries `grant` (option B/C), the standing grant
    /// is recorded before the call runs, so the record exists even if the run fails.
    pub async fn decide_approval_from_command(
        self: &Arc<Self>,
        approval_id: &str,
        outcome: ApprovalOutcome,
        decided_by: &str,
        payload: &Value,
    ) -> Result<()> {
        // Secret hand-off: the request card's answer names the saved secret (ids and names,
        // never a value) for the tool waiting on it; the offer card's Save runs the save.
        crate::handoff::note_card_answer(approval_id, payload);
        let detected = self
            .store
            .approval(approval_id)
            .ok()
            .flatten()
            .filter(|row| {
                row.state == ApprovalState::Pending.as_str()
                    && row.payload.get("kind").and_then(Value::as_str)
                        == Some(crate::handoff::DETECTED_KIND)
            })
            .map(|row| row.payload.clone());
        let grant = payload
            .get(GRANT_FIELD)
            .and_then(Value::as_str)
            .and_then(PathClass::parse);
        if let (Some(class), true) = (grant, outcome.is_approved()) {
            if let Ok(Some(row)) = self.store.approval(approval_id) {
                if row.state == ApprovalState::Pending.as_str() {
                    let mut row_payload = row.payload.clone();
                    if payload
                        .get("grant_chat_only")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        row_payload["grant_chat_only"] = json!(true);
                    }
                    self.record_shell_grant(
                        &row_payload,
                        &row.chat_id,
                        approval_id,
                        class,
                        decided_by,
                    )
                    .await;
                    if let Some(host) = self.sync.host(&row.chat_id).await {
                        host.lock()
                            .await
                            .doc
                            .set_approval_grant(approval_id, class.as_str());
                    }
                }
            }
        }
        let approved = outcome.is_approved();
        self.decide_approval(approval_id, outcome, decided_by)
            .await?;
        if let Some(card) = detected {
            let agent = self.clone();
            tokio::spawn(async move {
                crate::handoff::on_detected_decided(&agent, &card, approved).await;
            });
        }
        Ok(())
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

    /// Replace the standing shell grants with the relay's current list
    /// (`GET /box/shell-grants`, at boot and on every `shell_grants_changed` push).
    pub async fn refresh_shell_grants(self: &Arc<Self>) -> Result<(), String> {
        let v = crate::vault::relay_get(self, "/box/shell-grants").await?;
        let items: Vec<ShellGrant> = v["items"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|g| serde_json::from_value(g.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        tracing::info!(grants = items.len(), "shell grants refreshed");
        *self.approvals.shell_grants.lock().await = items;
        Ok(())
    }

    /// The standing grant that lets `hit` run in `chat_id` without a card, if any. Audited as
    /// `shell_grant_used` so every allowed run is in Activity.
    async fn shell_grant_for(
        self: &Arc<Self>,
        hit: &RiskMatch,
        chat_id: Option<&str>,
        tool: &str,
        args: &Value,
    ) -> Option<ShellGrant> {
        let grants = self.approvals.shell_grants.lock().await;
        let g = grants.iter().find(|g| g.covers(hit, chat_id))?.clone();
        drop(grants);
        let (_, summary) = describe_tool(tool, args, hit);
        self.append_audit(
            "shell_grant_used",
            json!({
                "grant_id": g.id, "rule_class": g.rule_class, "path_class": g.path_class.as_str(),
                "command": summary, "chat_id": chat_id, "granted_by": g.granted_by,
            }),
        );
        tracing::info!(grant = %g.id, class = hit.class, "shell grant covers the call");
        Some(g)
    }

    /// Card option B/C was chosen: record the standing grant on the relay and locally, so the
    /// next matching command runs without a card even before the push comes back.
    async fn record_shell_grant(
        self: &Arc<Self>,
        row_payload: &Value,
        chat_id: &str,
        approval_id: &str,
        class: PathClass,
        decided_by: &str,
    ) {
        let rule_class = row_payload
            .get("class")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if !GRANTABLE_CLASSES.contains(&rule_class.as_str()) {
            tracing::warn!(
                approval_id,
                rule_class,
                "grant asked for a class that cannot be granted"
            );
            return;
        }
        let per_chat = row_payload
            .get("grant_chat_only")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let body = json!({
            "rule_class": rule_class, "path_class": class.as_str(),
            "chat_id": if per_chat { Some(chat_id) } else { None },
            "approval_id": approval_id, "granted_by": decided_by,
        });
        let local = ShellGrant {
            id: format!("local-{approval_id}"),
            project_id: self.cfg.project_id.clone(),
            rule_class: rule_class.clone(),
            path_class: class,
            chat_id: per_chat.then(|| chat_id.to_owned()),
            granted_by: decided_by.to_owned(),
            approval_id: Some(approval_id.to_owned()),
            created_at: now_ms(),
        };
        self.approvals.shell_grants.lock().await.push(local);
        match crate::vault::relay_post(self, "/box/shell-grants", &body).await {
            Ok(_) => {
                let _ = self.refresh_shell_grants().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, approval_id, "shell grant not recorded on the relay")
            }
        }
        self.append_audit(
            "shell_grant_added",
            json!({ "approval_id": approval_id, "rule_class": rule_class, "path_class": class.as_str(),
                "chat_id": per_chat.then_some(chat_id), "by": decided_by }),
        );
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
        if self
            .shell_grant_for(&hit, Some(&chat_id), tool, args)
            .await
            .is_some()
        {
            return Ok(());
        }
        let (title, summary) = describe_tool(tool, args, &hit);
        let req = ApprovalRequest {
            title,
            summary,
            risk: hit.risk,
            payload: json!({ "kind": "tool_call", "tool": tool, "input": args, "class": hit.class, "why": hit.why,
                "path_class": hit.path_class.map(PathClass::as_str), "grant_options": grant_options(&hit) }),
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
        if let Some(g) = self
            .shell_grant_for(&hit, Some(&chat_id), tool, input)
            .await
        {
            return json!({ "ok": true, "state": "allowed", "grantId": g.id });
        }
        let (title, summary) = describe_tool(tool, input, &hit);
        let req = ApprovalRequest {
            title: title.clone(),
            summary,
            risk: hit.risk,
            payload: json!({ "kind": "tool_call", "tool": tool, "input": input, "class": hit.class, "why": hit.why, "session_id": session_id, "role": role,
                "path_class": hit.path_class.map(PathClass::as_str), "grant_options": grant_options(&hit) }),
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

#[cfg(test)]
mod shell_grant_tests {
    use super::*;
    use crate::status::tests::test_agent;

    fn grant(class: PathClass, chat: Option<&str>) -> ShellGrant {
        ShellGrant {
            id: format!("g-{}", class.as_str()),
            project_id: "01TESTPROJECTQUICKREPLY000".into(),
            rule_class: "file-delete".into(),
            path_class: class,
            chat_id: chat.map(str::to_owned),
            granted_by: "u-test".into(),
            approval_id: None,
            created_at: 0,
        }
    }

    /// #1544: a standing grant skips the card for commands it covers and nothing wider; a
    /// revoked grant (the relay's list without it) cards again; the card offers the grant
    /// options for its class.
    #[tokio::test]
    async fn standing_grant_skips_the_card_until_revoked() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATSHELLGRANT000000";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let _host = agent.sync.open_room(chat).await.unwrap();
        let ws = json!({ "command": "rm -rf build" });
        let any = json!({ "command": "rm -rf /etc/x" });

        // No grant: the card is raised and offers "workspace" then "anywhere".
        let v = agent.hook_approval("bash", &ws, chat, "main").await;
        assert_eq!(v["state"], "pending", "{v}");
        let first = agent
            .store
            .approval(v["approvalId"].as_str().unwrap())
            .unwrap()
            .unwrap();
        let opts: Vec<&str> = first.payload["grant_options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["path_class"].as_str().unwrap())
            .collect();
        assert_eq!(opts, ["workspace", "anywhere"]);
        assert_eq!(first.payload["path_class"], "workspace");

        // A scratch-only grant does not cover the workspace; a workspace grant does, and not /etc.
        *agent.approvals.shell_grants.lock().await = vec![grant(PathClass::Scratch, None)];
        assert_eq!(
            agent.hook_approval("bash", &ws, chat, "main").await["state"],
            "pending"
        );
        *agent.approvals.shell_grants.lock().await = vec![grant(PathClass::Workspace, None)];
        let v = agent.hook_approval("bash", &ws, chat, "main").await;
        assert_eq!(v["state"], "allowed", "{v}");
        assert_eq!(v["grantId"], "g-workspace");
        assert_eq!(
            agent.hook_approval("bash", &any, chat, "main").await["state"],
            "pending"
        );
        assert!(agent.gate_tool("bash", &ws, chat).await.is_ok());

        // Per-chat grant: only that chat.
        *agent.approvals.shell_grants.lock().await =
            vec![grant(PathClass::Anywhere, Some("other"))];
        assert_eq!(
            agent.hook_approval("bash", &ws, chat, "main").await["state"],
            "pending"
        );

        // Revoked (the relay's list no longer has it): the card is back.
        agent.approvals.shell_grants.lock().await.clear();
        assert_eq!(
            agent.hook_approval("bash", &ws, chat, "main").await["state"],
            "pending"
        );

        // Grants never cover a non-path rule.
        *agent.approvals.shell_grants.lock().await = vec![grant(PathClass::Anywhere, None)];
        let push = json!({ "command": "git push -f origin main" });
        let v = agent.hook_approval("bash", &push, chat, "main").await;
        assert_eq!(v["state"], "pending");
        assert!(v["approvalId"].is_string());
        let row = agent
            .store
            .approval(v["approvalId"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert!(row.payload["grant_options"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Option B/C on the card (`approve` with `grant`): the grant takes effect locally at once,
    /// even before the relay confirms it, and the next matching command runs without a card.
    #[tokio::test]
    async fn approve_with_grant_records_a_standing_grant() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATSHELLGRANT000001";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let _host = agent.sync.open_room(chat).await.unwrap();
        let ws = json!({ "command": "cd /work/app && rm -rf node_modules" });
        let v = agent.hook_approval("bash", &ws, chat, "main").await;
        let id = v["approvalId"].as_str().unwrap().to_owned();
        agent
            .decide_approval_from_command(
                &id,
                ApprovalOutcome::Approved,
                "dev-mac",
                &json!({ "approvalId": id, GRANT_FIELD: "workspace" }),
            )
            .await
            .unwrap();
        assert_eq!(agent.approval_status(&id).await["state"], "approved");
        let grants = agent.approvals.shell_grants.lock().await.clone();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].path_class, PathClass::Workspace);
        assert_eq!(grants[0].granted_by, "dev-mac");
        assert_eq!(grants[0].chat_id, None);
        let v = agent
            .hook_approval("bash", &json!({ "command": "rm -rf dist" }), chat, "main")
            .await;
        assert_eq!(v["state"], "allowed", "{v}");
        assert_eq!(
            agent
                .hook_approval("bash", &json!({ "command": "rm -rf /opt/x" }), chat, "main")
                .await["state"],
            "pending"
        );
        // A plain approve (option A) adds nothing.
        let v = agent
            .hook_approval("bash", &json!({ "command": "rm -rf /opt/y" }), chat, "main")
            .await;
        let id2 = v["approvalId"].as_str().unwrap().to_owned();
        agent
            .decide_approval_from_command(
                &id2,
                ApprovalOutcome::Approved,
                "dev-mac",
                &json!({ "approvalId": id2 }),
            )
            .await
            .unwrap();
        assert_eq!(agent.approvals.shell_grants.lock().await.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
