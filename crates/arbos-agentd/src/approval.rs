//! Draft-and-approve on the box (feature `approve`, design §10 / S18): post an `approval` card
//! into the chat doc, wait for the client's `approve` / `reject` command, update the card, and
//! write `approval_requested` / `approval_decided` audit entries (box `audit.jsonl` + relay
//! mirror). Three ways a caller waits:
//!
//! - **inline**: `request_approval` blocks the calling tool until the decision (gated MCP tools);
//! - **polled**: the pi-ext `tool_call` hook posts the card and polls `/hook/approval-status`;
//! - **deferred**: `draft_message` posts the card and ends the turn; the decision dispatches the
//!   draft and prompts the main agent with the outcome as a `[Arbos]` notice.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tokio::sync::{oneshot, Mutex};

use arbos_core::approval::{
    classify_tool_with, describe_tool, may_dispatch, resolve_path, ApprovalOutcome,
    ApprovalRequest, ApprovalState, PathClass, PathFacts, Risk, RiskMatch, ShellGrant,
    APPROVAL_TTL_MS, GRANTABLE_CLASSES, WORKSPACE_ROOT,
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
    classify_tool_with(tool, args, PathFacts::with_content(&file_has_content))
}
use arbos_core::doc::MessageStatus;
use arbos_core::ids::{now_ms, ulid};

use crate::agent::{Agent, TurnKind};
use crate::connectors;
use crate::store::{ApprovalRow, JobRow};

/// A re-run of a gated call is pre-approved for this long after the user approved it late.
const GRANT_TTL_MS: i64 = 10 * 60 * 1000;

/// The job step while one of its cards waits: what `list_jobs`, the rollout view and the chat's
/// presence row show, since the card itself is posted in the owner's main chat.
const APPROVAL_WAIT_STEP: &str = "Waiting for approval";

/// The step of a job that stopped because nobody answered its card.
const APPROVAL_EXPIRED_STEP: &str = "Stopped: needs your approval";

/// Harness tools whose call writes the file it names (`path` / `file_path`). An edit is in the
/// list because the file it names may not exist yet; one that does is not recorded either way.
const WRITE_TOOLS: &[&str] = &[
    "write",
    "write_file",
    "create_file",
    "edit",
    "edit_file",
    "str_replace",
    "multi_edit",
];

/// Most paths one job's "it wrote this itself" list holds. A job that writes more than this
/// keeps the cards it used to get for the rest; the list never grows without a bound.
const JOB_WRITES_MAX: usize = 512;

/// The error a job carries when one of its own cards expired unanswered. The job stops there:
/// no second card, no fresh worker, and the result turn tells the user why (Test5 round 2 bug H
/// — T8 sat through four cards, each expiring after the extension's 14-minute wait, because
/// every refusal was retried).
pub(crate) const APPROVAL_EXPIRED_ERROR: &str =
    "an approval card for this job expired unanswered, so the job stopped; it was not retried";

/// Whether this job stopped because a card of its own expired.
pub(crate) fn stopped_by_expired_approval(job: &JobRow) -> bool {
    job.error.as_deref() == Some(APPROVAL_EXPIRED_ERROR)
}

/// The one line a job's own surfaces get while a card of that job waits in another chat: what
/// is waiting, and which chat to answer it in.
///
/// It names the work, because a notice that does not is worth nothing. Test5 round 3, bug M:
/// "The background work I started has finished" went out with two jobs in flight, naming
/// neither, and told the reader nothing they could act on. So the card's own title carries the
/// line, and its summary — for a shell card, the command itself — is appended when it adds
/// anything.
///
/// `chat` is a name, never an id. A ULID is not a place a person can be asked to go, so the
/// caller passes a generic pointer to the main chat when the chat it found has no title.
pub(crate) fn approval_wait_line(title: &str, summary: &str, chat: &str) -> String {
    let title = title.trim();
    // Only `describe_tool`'s shell shape opens with a capitalised verb, and only that one needs
    // lowering to continue this sentence. Its other two shapes — `{provider}: {action}` and
    // `{tool} ({class})` — read correctly after "to" as they are, and lowering by case alone
    // turned "Gmail: send" into "gmail: send".
    let mut what = match (title.strip_prefix("Run "), title.is_empty()) {
        (Some(rest), _) => format!("run {rest}"),
        (None, false) => title.to_owned(),
        (None, true) => "a step of this job".to_owned(),
    };
    let summary = summary.trim();
    if !summary.is_empty() && !what.contains(summary) {
        what = format!("{what}: {summary}");
    }
    format!("Waiting for your approval to {what}. The card is in {chat}.")
}

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
    /// Per job id, the absolute paths that job's own writes created: deleting one of those
    /// destroys nothing the user had, so it raises no card (`Agent::note_job_write`).
    job_writes: Mutex<HashMap<String, HashSet<String>>>,
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

/// What the harness tells the model when its gated call is parked.
///
/// Three things it has to carry, because each was got wrong somewhere in Test5: nothing ran, so
/// the model may not report the work as done; the call must not be repeated, since the re-run is
/// Arbos's job once the card is answered; and the turn should end rather than wait, which is the
/// point of parking it at all.
///
/// It names no chat. The model is not told where the card went (`hook_approval`'s pending reply
/// carries that), so a sentence here would be the invention this is meant to stop.
fn parked_reason(approval_id: &str) -> String {
    format!(
        "Nothing ran. This needs the user's approval and card {approval_id} is waiting for them. \
         Do not call it again and do not wait for it: say in one short sentence that it needs \
         their approval, then end your turn. When they approve, Arbos runs the call and gives \
         you the result."
    )
}

/// How long a card is answerable when nothing shorter than the TTL bounds the wait for it:
/// `APPROVAL_TTL_MS` (24 h), or `ARBOS_APPROVAL_TIMEOUT_SECS` where a box shortens it.
pub(crate) fn approval_ttl_ms() -> i64 {
    std::env::var("ARBOS_APPROVAL_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .map(|secs| secs.saturating_mul(1000))
        .filter(|ms| *ms > 0)
        .unwrap_or(APPROVAL_TTL_MS)
}

/// How long the harness blocks a gated tool call while it polls `/hook/approval-status`
/// (`ARBOS_APPROVAL_WAIT_MS`, 14 minutes), after which the call is refused and nothing runs.
///
/// agentd owns this number and hands it to the extension in the `/hook/approval` reply, because
/// the card's deadline has to be the same one the waiter uses. The extension kept its own copy
/// of the default, and that is the whole of Test5 round 5 bug R: card
/// `01M6MRB89V75ZEX909EDWQJRDV` advertised `expires_at` at +24 h and was expired by
/// `arbos:timeout` at +14 min, the moment the extension stopped polling. Round 4 shows the same
/// fourteen minutes, which is what tells you it was a fixed cap rather than a slow user.
pub(crate) fn harness_wait_cap_ms() -> i64 {
    std::env::var("ARBOS_APPROVAL_WAIT_MS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(14 * 60 * 1000)
}

/// How long a main-session gated call blocks inside the harness hook before it is parked and the
/// chat's turn released (`ARBOS_APPROVAL_PARK_AFTER_MS`).
///
/// Short, because every millisecond of it is a millisecond the chat cannot hold a conversation:
/// the main turn is held for the whole wait, so a new message in that chat can only reach the
/// quick lane. Round 5 held a side chat's turn this way for fourteen minutes.
///
/// Not zero, because approving straight away is the common case, and answering inside the grace
/// keeps that case on its original tool call — nothing is parked, nothing is re-run, and no
/// second model call is spent. Ten seconds buys that and still frees the turn long before
/// anyone has typed a follow-up.
fn park_after_ms() -> i64 {
    std::env::var("ARBOS_APPROVAL_PARK_AFTER_MS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(10_000)
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
    ///
    /// `expires_in_ms` is the deadline this card gets, and it is the caller's statement of when
    /// its own wait stops. The two cannot be chosen separately: a card is answerable only while
    /// something is still prepared to act on the answer, so whatever cancels the wait *is* the
    /// expiry, and `expires_at` has to say so. Round 5 is what happens when they drift — the
    /// card advertised 24 h to the user while the harness gave up at 14 minutes
    /// ([`harness_wait_cap_ms`]) and expired it.
    pub async fn begin_approval(
        self: &Arc<Self>,
        chat_id: &str,
        req: ApprovalRequest,
        mode: WaitMode,
        waiter: Option<oneshot::Sender<ApprovalOutcome>>,
        expires_in_ms: i64,
    ) -> Result<String> {
        let host = self
            .sync
            .host(chat_id)
            .await
            .ok_or_else(|| anyhow!("approval: chat {chat_id} has no doc"))?;
        let approval_id = ulid();
        let expires_at = now_ms() + expires_in_ms.max(1);
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
        self.expire_at_deadline(&approval_id, expires_in_ms);
        Ok(approval_id)
    }

    /// Expire this card when its own `expires_at` arrives.
    ///
    /// The minute sweep (`expire_approvals`) is the backstop that survives a restart; it cannot
    /// be the deadline, because a card is then expired up to a minute after the time the user
    /// was shown. This timer is what makes `expires_at` the answer to "when does it stop being
    /// answerable" rather than a rounded-down hint.
    ///
    /// Firing twice is harmless: `Store::decide_approval` only updates a row still `pending`, so
    /// the first decision wins and a later one returns `Err` here, where it is dropped.
    fn expire_at_deadline(self: &Arc<Self>, approval_id: &str, expires_in_ms: i64) {
        let me = self.clone();
        let id = approval_id.to_owned();
        let wait = Duration::from_millis(expires_in_ms.max(1) as u64);
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            let _ = me
                .decide_approval(&id, ApprovalOutcome::Expired, "arbos:expiry")
                .await;
        });
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
    ///
    /// The wait and the card's deadline are the same `approval_ttl_ms()`, so this caller cannot
    /// cancel a card that still looks answerable. It used to time out on
    /// `ARBOS_APPROVAL_TIMEOUT_SECS` while the card advertised the fixed 24 h TTL, which on a
    /// box that set that variable was round 5's bug with a different number in it.
    pub async fn request_approval(
        self: &Arc<Self>,
        chat_id: &str,
        req: ApprovalRequest,
    ) -> ApprovalOutcome {
        let (tx, rx) = oneshot::channel();
        let ttl = approval_ttl_ms();
        let approval_id = match self
            .begin_approval(chat_id, req, WaitMode::Inline, Some(tx), ttl)
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
        // A second of slack past the card's own deadline, so the expiry that lands is the card's
        // (`expire_at_deadline`, `decided_by = arbos:expiry`) and this is only the backstop for
        // a waiter whose card somehow never settled.
        let backstop = Duration::from_millis(ttl.saturating_add(1_000) as u64);
        match tokio::time::timeout(backstop, rx).await {
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
        if state == ApprovalState::Expired {
            self.fail_job_on_expired_approval(&row).await;
        }

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

    /// Remember the files this call of a job is about to create, for `PathFacts::created_here`:
    /// the target of a `write` / `edit` tool, and the `>` / `>>` redirects of a shell command.
    /// The call has not run yet, so a target that names nothing on disk is one this job makes.
    ///
    /// Three limits keep this from growing into a licence to delete:
    ///
    /// - a target that already exists is never recorded (an overwrite is not a creation);
    /// - a shell command the matcher reads as deleting anything records nothing, so no gated
    ///   command can widen its own exemption;
    /// - what is recorded is one exact absolute path per spelling the matcher can produce for
    ///   that file — relative to the workspace root, which is how `delete_targets` resolves a
    ///   relative target, and relative to this project's own folder, which is where the file
    ///   really is. Each spelling is checked against the disk on its own.
    pub(crate) async fn note_job_write(&self, job_id: &str, tool: &str, input: &Value) {
        let tool = crate::activity::bare_tool_name(tool);
        let work_dir = self.cfg.work_dir.to_string_lossy().to_string();
        let targets: Vec<String> = if WRITE_TOOLS.contains(&tool) {
            let Some(path) = ["path", "file_path"]
                .iter()
                .find_map(|k| input.get(k).and_then(Value::as_str))
            else {
                return;
            };
            [WORKSPACE_ROOT, &work_dir]
                .iter()
                .map(|cwd| resolve_path(cwd, path))
                .collect()
        } else if arbos_core::approval::SHELL_TOOLS.contains(&tool) {
            let cmd = ["command", "cmd"]
                .iter()
                .find_map(|k| input.get(k).and_then(Value::as_str))
                .unwrap_or_default();
            if arbos_core::approval::delete_targets(cmd).is_some() {
                return;
            }
            [WORKSPACE_ROOT, &work_dir]
                .iter()
                .flat_map(|cwd| arbos_core::approval::redirect_targets(cwd, cmd))
                .collect()
        } else {
            return;
        };
        let mut writes = self.approvals.job_writes.lock().await;
        let paths = writes.entry(job_id.to_owned()).or_default();
        for path in targets {
            if paths.len() >= JOB_WRITES_MAX || std::fs::metadata(&path).is_ok() {
                continue;
            }
            paths.insert(path);
        }
    }

    /// `classify_tool` for one job's call: the paths that job created itself are exempt from the
    /// deletion rule. Every other caller — the main session, an MCP call that names no session —
    /// gets the plain view, where nothing is exempt.
    async fn classify_tool_for_job(
        &self,
        tool: &str,
        args: &Value,
        job: Option<&JobRow>,
    ) -> Option<RiskMatch> {
        let Some(job) = job else {
            return classify_tool(tool, args);
        };
        let mine = self
            .approvals
            .job_writes
            .lock()
            .await
            .get(&job.id)
            .cloned()
            .unwrap_or_default();
        classify_tool_with(
            tool,
            args,
            PathFacts {
                has_content: &file_has_content,
                created_here: &|p| mine.contains(p),
            },
        )
    }

    /// A job's card is posted where the owner can answer it — their main chat — so the job's own
    /// surfaces have to say why it stopped: a line in the worker's thread, which the rollout view
    /// draws, the step `list_jobs` and the chat's presence row read, and one row in the job's own
    /// chat.
    ///
    /// The chat row is the part round 4 was missing. The step alone left the side chat reading as
    /// silent for fourteen minutes: a presence chip is not the transcript, and it clears the
    /// moment the job stops running, so a person scrolling that chat afterwards finds no trace of
    /// what the job was waiting for.
    ///
    /// That row is never removed. It states a fact with a time on it — approval was asked for
    /// then, and the card went to that chat — so nothing later makes it untrue and no supersede
    /// is owed. Designing the removal out is also what keeps it clear of the relay's refusal of a
    /// removal it cannot attribute (#2751): were one ever wanted, it would be `retire_row`, never
    /// `delete_message`. The row is written through the chat's live doc, which signs the box's
    /// assistant rows `agent:<project_id>`, so it is attributable from birth.
    async fn note_job_awaits_approval(
        &self,
        job: &JobRow,
        card_chat_id: &str,
        line: &str,
        source: &Value,
    ) {
        if let Some(sid) = job.subagent_id.as_deref() {
            let _ = self.store.t3_append(
                sid,
                Some(sid),
                Some(sid),
                "approval",
                Some("system"),
                line,
                Some(source),
            );
        }
        let _ = self.store.set_job_step(&job.id, APPROVAL_WAIT_STEP);
        // Only when the card is somewhere else: a notice beside the card it describes is noise.
        if job.chat_id != card_chat_id {
            if let Some(host) = self.sync.host(&job.chat_id).await {
                let h = host.lock().await;
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
                h.doc
                    .set_message_fields(&id, &[("turn", json!({ "kind": "approval" }))]);
                h.doc
                    .add_part_from_json(&id, &json!({ "kind": "text", "text": line }));
                h.doc.finish_message(&id, MessageStatus::Done);
            } else {
                tracing::debug!(job = %job.id, chat = %job.chat_id, "no open room for the job's chat; its card notice is in the rollout view only");
            }
        }
        self.refresh_job_activity(&job.chat_id).await;
    }

    /// The job a worker session runs, when its card belongs to one.
    fn job_of_session(&self, session_id: &str) -> Option<JobRow> {
        self.store.job_for_subagent(session_id).ok().flatten()
    }

    /// One expired card ends the job that raised it: the worker is stopped, the job is failed
    /// with the reason the result turn relays, and the next gated call of that job is refused
    /// without another card. Four cards, each expiring after fourteen minutes, was the livelock.
    async fn fail_job_on_expired_approval(self: &Arc<Self>, row: &ApprovalRow) {
        let Some(job) = row
            .payload
            .get("session_id")
            .and_then(Value::as_str)
            .and_then(|sid| self.job_of_session(sid))
            .filter(JobRow::is_open)
        else {
            return;
        };
        let _ = self
            .store
            .set_job_state(&job.id, "failed", Some(APPROVAL_EXPIRED_ERROR));
        let _ = self.store.set_job_step(&job.id, APPROVAL_EXPIRED_STEP);
        if let Some(sid) = job.subagent_id.as_deref() {
            let _ = self.store.t3_append(
                sid,
                Some(sid),
                Some(sid),
                "approval",
                Some("system"),
                &format!("Your approval expired, so this stopped: {}", row.title),
                Some(&json!({ "approval_id": row.id, "job_id": job.id, "state": "expired" })),
            );
            let adapter = self.adapter(arbos_core::route::Harness::Pi);
            if let Err(e) = adapter.cancel(sid).await {
                tracing::debug!(job = %job.id, error = %e, "expired card: falling back to stop");
                let _ = adapter.stop(sid).await;
            }
        }
        self.release_job_locks(&job.id).await;
        self.refresh_job_activity(&job.chat_id).await;
        tracing::warn!(job = %job.id, approval_id = %row.id, "a card of this job expired unanswered; the job stops and is not retried");
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
        let job = self.job_of_session(session_id);
        // A job whose card already expired asks nothing again: the refusal is immediate, so the
        // worker cannot spend another fourteen minutes on a card nobody will answer.
        if let Some(j) = job.as_ref().filter(|j| stopped_by_expired_approval(j)) {
            tracing::warn!(job = %j.id, tool, "a card of this job expired; refusing without another");
            return json!({ "ok": true, "state": ApprovalState::Expired.as_str(), "approvalId": "", "why": APPROVAL_EXPIRED_ERROR });
        }
        let Some(hit) = self.classify_tool_for_job(tool, input, job.as_ref()).await else {
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
            summary: summary.clone(),
            risk: hit.risk,
            payload: json!({ "kind": "tool_call", "tool": tool, "input": input, "class": hit.class, "why": hit.why, "session_id": session_id, "role": role,
                "path_class": hit.path_class.map(PathClass::as_str), "grant_options": grant_options(&hit) }),
        };
        // How long the caller blocks, and therefore what bounds this card.
        //
        // A main-session call is parked when the grace passes (`park_approval`): the turn is
        // released and the decision is picked up later, so nothing is waiting on a clock and the
        // card keeps the full TTL. A worker's call is not parked — it has no chat turn to release
        // and its job is ended by one expired card (`fail_job_on_expired_approval`) — so it is
        // still bounded by the extension's cap, and the card says so.
        let parks = role == "main" && job.is_none();
        let wait_ms = if parks {
            park_after_ms()
        } else {
            harness_wait_cap_ms()
        };
        let expires_in = if parks {
            approval_ttl_ms()
        } else {
            harness_wait_cap_ms()
        };
        match self
            .begin_approval(&chat_id, req, WaitMode::Polled, None, expires_in)
            .await
        {
            Ok(id) => {
                if let Some(j) = &job {
                    // Where the card is, by name: a job's card goes to the chat the owner reads,
                    // which is rarely the chat the job runs in.
                    let where_it_is = self
                        .store
                        .chat(&chat_id)
                        .ok()
                        .flatten()
                        .map(|c| c.title)
                        .filter(|t| !t.trim().is_empty())
                        .unwrap_or_else(|| "your main chat".to_owned());
                    self.note_job_awaits_approval(
                        j,
                        &chat_id,
                        &approval_wait_line(&title, &summary, &where_it_is),
                        &json!({ "approval_id": id, "chat_id": chat_id, "job_id": j.id }),
                    )
                    .await;
                }
                json!({ "ok": true, "state": "pending", "approvalId": id, "title": title, "why": hit.why, "waitMs": wait_ms, "park": parks })
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

    /// `/hook/approval-park`: the harness stopped blocking a main-session call and the turn is
    /// about to end, but the card is left answerable for its full deadline.
    ///
    /// This is Test5 round 5 bug R's second half. The gated call used to hold the chat's main
    /// turn for the whole wait — fourteen minutes in rounds 4 and 5 — so a message arriving in
    /// that chat meanwhile could only get a quick-lane answer. Nothing about an approval needs
    /// the turn to stay open: the card is in the chat, and the user answers the card.
    ///
    /// So the mode moves from `Polled` to `Deferred`, which is the whole mechanism. Everything
    /// that finishes the work already exists for the late-approval case and is reached by that
    /// one change:
    ///
    /// - `decide_approval` treats a `Deferred` entry as deferred, so the decision runs
    ///   `follow_up` instead of handing the outcome to a waiter that has gone;
    /// - `follow_up`'s `("tool_call", Approved)` arm records a `Grant` for the identical call and
    ///   starts a turn in the card's chat telling the agent to run it again;
    /// - `take_grant` lets that re-run through without a second card.
    ///
    /// Nothing here is a new waiting state, and `draft_message` has used this path since S18.
    pub async fn park_approval(self: &Arc<Self>, approval_id: &str) -> Value {
        let Some(row) = self.store.approval(approval_id).ok().flatten() else {
            return json!({ "ok": false, "error": "unknown approval" });
        };
        if row.state != ApprovalState::Pending.as_str() {
            // Decided inside the grace: the caller reads the real state and acts on it.
            return json!({ "ok": true, "state": row.state, "parked": false });
        }
        let parked = {
            let mut pending = self.approvals.pending.lock().await;
            match pending.get_mut(approval_id) {
                Some(p) if p.mode == WaitMode::Polled => {
                    p.mode = WaitMode::Deferred;
                    true
                }
                _ => false,
            }
        };
        if !parked {
            return json!({ "ok": true, "state": row.state, "parked": false });
        }
        self.set_approval_wait_activity(&row.chat_id, true).await;
        let _ = self.store.t3_append(
            &row.chat_id,
            None,
            Some(&row.chat_id),
            "approval",
            Some("system"),
            &format!(
                "approval parked: {} — the turn was released; the decision resumes it",
                row.title
            ),
            Some(&json!({ "approval_id": approval_id })),
        );
        self.append_audit(
            "approval_parked",
            json!({ "approval_id": approval_id, "chat_id": row.chat_id, "title": row.title,
                "expires_at": row.expires_at }),
        );
        tracing::info!(approval_id, chat_id = %row.chat_id, "approval parked; the turn is released until the card is decided");
        json!({ "ok": true, "state": row.state, "parked": true, "reason": parked_reason(approval_id) })
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

/// Test5 round 2, bug H: a job stalled on cards the user never saw in the job's own view, got
/// carded for files it had created itself, and retried every expired card — T8 sat through four
/// of them, fourteen minutes each.
#[cfg(test)]
mod job_approval_tests {
    use super::*;
    use crate::status::tests::test_agent;
    use crate::sync::SharedHost;

    const CHAT: &str = "01TESTCHATJOBAPPROVAL00000";
    const JOB: &str = "01TESTJOBAPPROVAL000000000";
    const WORKER: &str = "01TESTJOBWORKER00000000000";

    /// An agent with one running job in a pinned chat, as `start_job` leaves it.
    async fn agent_running_a_job() -> (Arc<Agent>, SharedHost, std::path::PathBuf) {
        let (agent, root) = test_agent().await;
        agent.store.upsert_chat(CHAT, "Main", true, 0).unwrap();
        let host = agent.sync.open_room(CHAT).await.unwrap();
        agent
            .store
            .insert_job(
                JOB,
                CHAT,
                "01TESTASKMESSAGE0000000000",
                "Clearing the old exports",
                "code",
                "Delete last month's export files.",
                &[],
            )
            .unwrap();
        agent.store.set_job_subagent(JOB, WORKER).unwrap();
        agent.store.set_job_state(JOB, "running", None).unwrap();
        (agent, host, root)
    }

    const SIDE: &str = "01TESTSIDECHATJOBAPPROVAL0";
    const SIDE_JOB: &str = "01TESTSIDEJOBAPPROVAL00000";
    const SIDE_WORKER: &str = "01TESTSIDEJOBWORKER0000000";

    /// Round 4's geometry: the job runs in a side chat, and its card goes to the pinned main
    /// chat, because that is the chat the owner reads (`approval_chat`). Both rooms are open,
    /// so a notice in either one is a thing the test could see.
    async fn agent_running_a_job_in_a_side_chat(
    ) -> (Arc<Agent>, SharedHost, SharedHost, std::path::PathBuf) {
        let (agent, root) = test_agent().await;
        agent.store.upsert_chat(CHAT, "Main", true, 0).unwrap();
        agent
            .store
            .upsert_chat(SIDE, "Trip planning", false, 0)
            .unwrap();
        let main = agent.sync.open_room(CHAT).await.unwrap();
        let side = agent.sync.open_room(SIDE).await.unwrap();
        agent
            .store
            .insert_job(
                SIDE_JOB,
                SIDE,
                "01TESTSIDEASKMESSAGE000000",
                "Cleaning up the build tree",
                "code",
                "Build it, test it, then clean up.",
                &[],
            )
            .unwrap();
        agent.store.set_job_subagent(SIDE_JOB, SIDE_WORKER).unwrap();
        agent
            .store
            .set_job_state(SIDE_JOB, "running", None)
            .unwrap();
        (agent, main, side, root)
    }

    /// The assistant rows of one chat, as text.
    fn chat_rows(host: &SharedHost) -> Vec<String> {
        let h = host.try_lock().expect("host free");
        h.doc
            .messages()
            .into_iter()
            .filter(|m| m.role == "assistant")
            .map(|m| m.text())
            .collect()
    }

    /// The rows of one chat that tell the reader a card is waiting.
    fn waiting_rows(host: &SharedHost) -> Vec<String> {
        chat_rows(host)
            .into_iter()
            .filter(|t| t.contains("Waiting for your approval"))
            .collect()
    }

    fn approval_lines(agent: &Agent) -> Vec<String> {
        let job = agent.store.job(JOB).unwrap().unwrap();
        let doc = crate::jobs_view::document(&agent.store, "p", "Doug", &job.id, 0).unwrap();
        doc["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["kind"] == "approval")
            .map(|e| e["text"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    /// The card is posted in the owner's main chat, where they can answer it, and the job's own
    /// surfaces now say so: the step `list_jobs`, the rollout view and the chat's presence row
    /// read, and a "needs your approval" line in the job's own exchange.
    #[tokio::test]
    async fn a_job_waiting_for_a_card_says_so_in_its_own_view() {
        let (agent, _host, root) = agent_running_a_job().await;
        let v = agent
            .hook_approval(
                "bash",
                &json!({ "command": "rm -rf /work/p/exports" }),
                WORKER,
                "subagent",
            )
            .await;
        assert_eq!(v["state"], "pending", "{v}");
        assert_eq!(
            agent.store.job(JOB).unwrap().unwrap().last_step,
            APPROVAL_WAIT_STEP
        );
        let doc = crate::jobs_view::document(&agent.store, "p", "Doug", JOB, 0).unwrap();
        assert_eq!(doc["step"], APPROVAL_WAIT_STEP);
        let lines = approval_lines(&agent);
        assert_eq!(lines.len(), 1, "one line in the job's own view: {lines:?}");
        assert!(
            lines[0].starts_with("Waiting for your approval to"),
            "{}",
            lines[0]
        );
        assert!(
            lines[0].contains("rm -rf /work/p/exports"),
            "the line names the work: {}",
            lines[0]
        );
        assert!(
            lines[0].ends_with("The card is in Main."),
            "the line says which chat holds the card: {}",
            lines[0]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The deletion rule flagged files the job itself had just made. T8's own outputs —
    /// `out/taxi_app_curl.txt` written by a shell redirect, `out/taxi_app.zip` written by the
    /// write tool — are exempt now. Everything around them still raises a card: a file the job
    /// did not write, the folder holding its file, a glob, the `find … -exec rm -rf` walk, a
    /// write over a file that was already there, a command that deletes while it redirects, and
    /// the same delete from a session that is no job of ours.
    #[tokio::test]
    async fn a_job_deleting_a_file_it_created_itself_raises_no_card() {
        let (agent, _host, root) = agent_running_a_job().await;
        agent
            .note_job_write(
                JOB,
                "bash",
                &json!({ "command": "curl -sS https://example.invalid/x > out/taxi_app_curl.txt" }),
            )
            .await;
        agent
            .note_job_write(JOB, "write", &json!({ "path": "out/taxi_app.zip" }))
            .await;
        let state = |cmd: &str| {
            let agent = agent.clone();
            let cmd = cmd.to_owned();
            async move {
                agent
                    .hook_approval("bash", &json!({ "command": cmd }), WORKER, "subagent")
                    .await["state"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            }
        };
        assert_eq!(state("rm -f out/taxi_app_curl.txt").await, "allowed");
        assert_eq!(state("rm -f out/taxi_app.zip").await, "allowed");
        assert_eq!(state("rm -f out/vendor.zip").await, "pending");
        assert_eq!(
            state("rm -f out/taxi_app.zip out/vendor.zip").await,
            "pending",
            "one file it did not write keeps the card"
        );
        assert_eq!(
            state("rm -rf out").await,
            "pending",
            "the folder that holds its file is not its file"
        );
        assert_eq!(
            state("rm -f out/*.zip").await,
            "pending",
            "a glob is not an exact path"
        );
        assert_eq!(
            state("find . -name __pycache__ -exec rm -rf {} +").await,
            "pending",
            "a walk over a tree names no file this job can be shown to have written"
        );
        // A command that deletes records nothing, so no gated command widens its own exemption.
        agent
            .note_job_write(
                JOB,
                "bash",
                &json!({ "command": "rm -f out/old.zip > out/clean.log" }),
            )
            .await;
        assert_eq!(state("rm -f out/clean.log").await, "pending");
        // A write over a file that is already there creates nothing, so it records nothing —
        // this crate's own manifest stands in for a file the job found rather than made.
        let already_there = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
        agent
            .note_job_write(JOB, "write", &json!({ "path": already_there }))
            .await;
        assert_eq!(state(&format!("rm -f {already_there}")).await, "pending");
        // The same delete from a session that is no job of ours: nothing is exempt.
        assert_eq!(
            agent
                .hook_approval(
                    "bash",
                    &json!({ "command": "rm -f out/taxi_app.zip" }),
                    CHAT,
                    "main"
                )
                .await["state"],
            "pending"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The livelock: every expired card was retried, so T8 waited four times fourteen minutes.
    /// One expired card ends the job — it is failed with the reason the result turn relays, its
    /// own view says so, no second card is raised, and no fresh worker is given.
    #[tokio::test]
    async fn one_expired_card_fails_the_job_instead_of_raising_another() {
        let (agent, _host, root) = agent_running_a_job().await;
        let cmd = json!({ "command": "rm -rf /work/p/exports" });
        let v = agent.hook_approval("bash", &cmd, WORKER, "subagent").await;
        let id = v["approvalId"].as_str().expect("a card").to_owned();
        // The extension stopped waiting after its 14-minute cap.
        assert_eq!(agent.abandon_approval(&id).await["state"], "expired");
        let job = agent.store.job(JOB).unwrap().unwrap();
        assert_eq!(job.state, "failed");
        assert_eq!(job.error.as_deref(), Some(APPROVAL_EXPIRED_ERROR));
        assert!(
            approval_lines(&agent)
                .iter()
                .any(|l| l.starts_with("Your approval expired")),
            "the job's own view says why it stopped: {:?}",
            approval_lines(&agent)
        );
        // The next gated call of that job is refused at once, with no second card.
        let again = agent.hook_approval("bash", &cmd, WORKER, "subagent").await;
        assert_eq!(again["state"], "expired", "{again}");
        assert!(again["approvalId"].as_str().unwrap_or_default().is_empty());
        assert!(agent.store.pending_approvals().unwrap().is_empty());
        // And the job is not handed a fresh worker to raise the same card again.
        assert_eq!(
            crate::agent::worker_failure(Some(APPROVAL_EXPIRED_ERROR), "", 3, "done").map(|f| f.1),
            Some(false)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The sentence itself, which the owner may tune: it names the work and where to answer.
    #[test]
    fn the_waiting_line_names_the_work_and_the_chat() {
        assert_eq!(
            approval_wait_line(
                "Run a destructive command (file.delete)",
                "git clean -fdX",
                "Main"
            ),
            "Waiting for your approval to run a destructive command (file.delete): \
             git clean -fdX. The card is in Main."
        );
        // No summary to add: the title carries it alone.
        assert_eq!(
            approval_wait_line("Gmail: send", "", "Trip planning"),
            "Waiting for your approval to Gmail: send. The card is in Trip planning."
        );
        // A chat with no title is pointed at generically, never by id — the caller's fallback.
        assert!(approval_wait_line(
            "Run a destructive command (file.delete)",
            "rm -rf x",
            "your main chat"
        )
        .ends_with("The card is in your main chat."));
        // Nothing to name is still a sentence.
        assert_eq!(
            approval_wait_line("", "", "Main"),
            "Waiting for your approval to a step of this job. The card is in Main."
        );
    }

    /// Test5 round 4, bug H again: the card for `git clean -fdX` was raised in the main chat and
    /// the side chat the job ran in said nothing at all for fourteen minutes, until the card
    /// expired. The step alone is not enough — it is a presence chip that clears when the job
    /// stops running, and it leaves nothing in the transcript the person was reading.
    ///
    /// So the job's chat gets one row of its own: what is waiting, and which chat to answer it
    /// in. One row per card and not one per poll, since the wait is fourteen minutes long and
    /// the extension polls throughout it.
    #[tokio::test]
    async fn a_job_in_a_side_chat_says_in_that_chat_that_a_card_is_waiting() {
        let (agent, main, side, root) = agent_running_a_job_in_a_side_chat().await;
        let v = agent
            .hook_approval(
                "bash",
                &json!({ "command": "git clean -fdX" }),
                SIDE_WORKER,
                "subagent",
            )
            .await;
        assert_eq!(v["state"], "pending", "{v}");
        let id = v["approvalId"].as_str().expect("a card").to_owned();

        // The geometry this test exists for: the card is in the pinned main chat, not in the
        // chat the job runs in. Without that split the two surfaces are the same surface.
        let card = agent.store.approval(&id).unwrap().unwrap();
        assert_eq!(card.chat_id, CHAT, "the card belongs to the main chat");
        assert_eq!(
            agent.store.job(SIDE_JOB).unwrap().unwrap().chat_id,
            SIDE,
            "and the job to the side chat"
        );

        // The side chat says so in the transcript, naming the work and where to answer it.
        let rows = waiting_rows(&side);
        assert_eq!(rows.len(), 1, "one row in the job's own chat: {rows:?}");
        assert!(
            rows[0].contains("git clean -fdX"),
            "the notice names the work, as bug M taught: {}",
            rows[0]
        );
        assert!(
            rows[0].ends_with("The card is in Main."),
            "the notice says where to act, by name: {}",
            rows[0]
        );
        assert!(
            !rows[0].contains(CHAT) && !rows[0].contains(SIDE),
            "a chat is named, never printed as an id: {}",
            rows[0]
        );
        // The card is already in the main chat; a notice there as well would be noise.
        assert!(
            waiting_rows(&main).is_empty(),
            "the notice goes to the job's chat, not to the card's: {:?}",
            waiting_rows(&main)
        );
        // And the step still moves, as the single-chat test pins.
        assert_eq!(
            agent.store.job(SIDE_JOB).unwrap().unwrap().last_step,
            APPROVAL_WAIT_STEP
        );

        // Fourteen minutes of polling adds no second row.
        for _ in 0..8 {
            assert_eq!(agent.approval_status(&id).await["state"], "pending");
        }
        assert_eq!(
            waiting_rows(&side).len(),
            1,
            "one row per card, not one per poll: {:?}",
            waiting_rows(&side)
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Test5 round 5, bug R, second half: a gated main-session call held the chat's turn for the
/// whole approval wait, so that chat could only answer from the quick lane meanwhile.
#[cfg(test)]
mod park_tests {
    use super::*;
    use crate::status::tests::test_agent;

    fn rm() -> Value {
        json!({ "command": "rm -rf /opt/x" })
    }

    /// The main session's wait is a short grace, and because the call is parked rather than
    /// refused at the end of it, nothing shorter than the TTL bounds the card any more — so the
    /// card goes back to advertising the full TTL, and that is again the truth.
    #[tokio::test]
    async fn a_main_session_wait_is_short_and_the_card_keeps_the_full_deadline() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATAPPROVALPARK0000";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let _host = agent.sync.open_room(chat).await.unwrap();

        let before = now_ms();
        let v = agent.hook_approval("bash", &rm(), chat, "main").await;
        assert_eq!(v["state"], "pending", "{v}");
        assert_eq!(v["park"], json!(true), "a main-session call parks: {v}");
        let wait = v["waitMs"].as_i64().unwrap();
        assert!(
            wait < 30_000,
            "the turn must be free before a follow-up message arrives: {wait} ms"
        );
        assert!(
            wait < harness_wait_cap_ms(),
            "round 5 blocked for the whole {} ms cap",
            harness_wait_cap_ms()
        );
        let row = agent
            .store
            .approval(v["approvalId"].as_str().unwrap())
            .unwrap()
            .unwrap();
        let life = row.expires_at - before;
        assert!(
            (life - approval_ttl_ms()).abs() < 5_000,
            "a parked card is bounded by nothing but its TTL: {life} ms"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The owner's third test: approving before `expires_at` runs the command and the result
    /// comes back in the same chat. Parking moves the entry to `Deferred`, which is what hands
    /// the decision to `follow_up` — the late-approval path that records a `Grant` for the
    /// identical call and starts a turn to run it. No second card is raised.
    #[tokio::test]
    async fn a_parked_card_approved_later_runs_the_call_without_a_second_card() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATAPPROVALPARK0001";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let _host = agent.sync.open_room(chat).await.unwrap();

        let v = agent.hook_approval("bash", &rm(), chat, "main").await;
        let id = v["approvalId"].as_str().unwrap().to_owned();

        let parked = agent.park_approval(&id).await;
        assert_eq!(parked["parked"], json!(true), "{parked}");
        assert_eq!(
            parked["state"], "pending",
            "parking decides nothing: {parked}"
        );
        let reason = parked["reason"].as_str().unwrap();
        assert!(
            reason.contains("Nothing ran") && reason.contains("end your turn"),
            "the model must not report work it did not do, and must stop: {reason}"
        );
        assert_eq!(
            agent.approvals.pending.lock().await[&id].mode,
            WaitMode::Deferred,
            "the parked entry is what makes the decision deferred"
        );

        agent
            .decide_approval_from_command(
                &id,
                ApprovalOutcome::Approved,
                "dev-mac",
                &json!({ "approvalId": id }),
            )
            .await
            .unwrap();

        // `follow_up` runs on its own task; the grant it records is the observable half.
        let mut granted = None;
        for _ in 0..100 {
            if let Some(g) = agent
                .approvals
                .grants
                .lock()
                .await
                .iter()
                .find(|g| g.approval_id == id)
            {
                granted = Some(g.tool.clone());
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            granted.as_deref(),
            Some("bash"),
            "an approved parked call is pre-approved for its re-run"
        );
        assert_eq!(
            agent.hook_approval("bash", &rm(), chat, "main").await["state"],
            "allowed",
            "the re-run must not raise a second card"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #2770 sweeps every row a turn opened that shows nothing, and a parked wait outlives its
    /// turn — so the question is whether the sweep hides the card.
    ///
    /// It does not, and the reason is structural rather than lucky. This change opens no row of
    /// its own: the status it shows is the turn's activity label and the relay's presence line,
    /// and the only row involved is the one `begin_approval` already writes the card onto. That
    /// row holds an `approval` part, and `wordless_row` counts anything other than `text` and
    /// `status` as content the client draws, so the row is not wordless and neither
    /// `retire_wordless_rows` nor `trim_wordless_result_row` touches it. The assertion below
    /// hands the row's own id to the real predicate rather than reasoning about it.
    #[tokio::test]
    async fn a_parked_cards_row_survives_the_wordless_sweep() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATAPPROVALPARK0002";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            agent.test_open_row(&host, t).await;
        }

        let v = agent.hook_approval("bash", &rm(), chat, "main").await;
        let id = v["approvalId"].as_str().unwrap().to_owned();
        assert_eq!(agent.park_approval(&id).await["parked"], json!(true));

        let (rows, card_row) = {
            let tl = agent.turn.lock().await;
            let t = tl.as_ref().unwrap();
            (t.rows_opened.clone(), t.assistant_msg_id.clone())
        };
        assert!(
            rows.contains(&card_row),
            "the card is on a row this turn opened, which is what the sweep looks at"
        );

        let h = host.lock().await;
        h.doc.finish_message(&card_row, MessageStatus::Done);
        assert!(
            !crate::agent::wordless_row(&h.doc.message(&card_row).unwrap()).0,
            "an approval part is content the client draws"
        );
        // `last` is a row the sweep settles elsewhere, so the card's row is really considered.
        let retired =
            crate::agent::retire_wordless_rows(&h.doc, &rows, "01NOTAROWOFTHISTURN0000000");
        assert_eq!(retired, 0, "the card's row must survive its own turn");
        assert_ne!(
            h.doc
                .message(&card_row)
                .unwrap()
                .rest
                .get("hidden")
                .and_then(Value::as_bool),
            Some(true),
            "a hidden row takes the card away from the user who has to answer it"
        );
        drop(h);
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    use crate::status::tests::test_agent;

    /// Test5 round 5, bug R. Card `01M6MRB89V75ZEX909EDWQJRDV` was posted at 20:24:29 with
    /// `expires_at` 1793931869499 — exactly +24 h, `APPROVAL_TTL_MS` — and was expired by
    /// `arbos:timeout` at 20:38:29, +14 minutes. Round 4 landed on the same fourteen minutes
    /// (19:19:17 → 19:33:18), which is what says "fixed cap" rather than "slow user": the
    /// extension stops polling at `ARBOS_APPROVAL_WAIT_MS` and nothing told the card.
    ///
    /// The card now carries the deadline its own waiter enforces, and the reply hands that
    /// waiter the number instead of letting it keep a second copy of the default.
    #[tokio::test]
    async fn a_gated_card_advertises_the_deadline_its_waiter_enforces() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATAPPROVALTTL00000";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let _host = agent.sync.open_room(chat).await.unwrap();

        // A worker's call is not parked — it has no chat turn to release — so the extension's cap
        // is still what bounds it, and the card has to say so rather than claim 24 hours.
        let before = now_ms();
        let v = agent
            .hook_approval(
                "bash",
                &json!({ "command": "rm -rf /opt/x" }),
                chat,
                "subagent",
            )
            .await;
        assert_eq!(v["state"], "pending", "{v}");
        assert_eq!(
            v["park"],
            json!(false),
            "a worker's call is not parked: {v}"
        );
        let row = agent
            .store
            .approval(v["approvalId"].as_str().unwrap())
            .unwrap()
            .unwrap();
        let life = row.expires_at - before;
        assert!(
            (life - harness_wait_cap_ms()).abs() < 5_000,
            "the card must expire when its waiter stops waiting: {life} ms"
        );
        assert!(
            life < APPROVAL_TTL_MS,
            "round 5 advertised the whole 24 h TTL on a card capped at 14 minutes: {life} ms"
        );
        assert_eq!(
            v["waitMs"].as_i64(),
            Some(harness_wait_cap_ms()),
            "the waiter reads agentd's cap, so the two cannot drift apart again: {v}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The owner's first test: leave a card undecided and its real expiry matches `expires_at`
    /// within five seconds. The minute sweep cannot answer that on its own — it would expire a
    /// card up to 60 s after the time the user was shown — so each card carries a timer for its
    /// own deadline and the sweep stays the restart backstop.
    #[tokio::test]
    async fn an_undecided_card_expires_at_the_time_it_advertises() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATAPPROVALTTL00001";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let _host = agent.sync.open_room(chat).await.unwrap();

        let req = ApprovalRequest {
            title: "Run a destructive command (file.delete)".into(),
            summary: "rm -rf /opt/x".into(),
            risk: Risk::Destructive,
            payload: json!({ "kind": "tool_call", "tool": "bash" }),
        };
        let id = agent
            .begin_approval(chat, req, WaitMode::Polled, None, 200)
            .await
            .unwrap();
        let row = agent.store.approval(&id).unwrap().unwrap();
        assert_eq!(agent.approval_status(&id).await["state"], "pending");

        tokio::time::sleep(Duration::from_millis(900)).await;
        let after = agent.store.approval(&id).unwrap().unwrap();
        assert_eq!(after.state, "expired", "the deadline must be enforced");
        let late = after.decided_at.unwrap() - row.expires_at;
        assert!(
            late.abs() < 5_000,
            "expired {late} ms away from the advertised deadline"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
