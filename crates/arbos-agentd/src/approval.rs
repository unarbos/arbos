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
    GRANTABLE_CLASSES,
};

/// The box's view for the classifier (#1719): a truncation idiom (`: > f`, `> f`,
/// `cp /dev/null f`) deletes something only if `f` already exists with content. agentd runs
/// on the box, so it can look. Unresolvable paths (`$D/x`) do not exist → not a delete.
fn file_has_content(path: &str) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

/// `workspace_root` is this agent's own project folder, which is what the deletion rule is
/// scoped to: `Config::work_dir`, never the `/work` the pure matcher falls back to.
fn classify_tool(tool: &str, args: &Value, workspace_root: &str) -> Option<RiskMatch> {
    classify_tool_with(
        tool,
        args,
        PathFacts::with_content(&file_has_content).in_workspace(workspace_root),
    )
}
use arbos_core::doc::MessageStatus;
use arbos_core::ids::{now_ms, ulid};

use crate::agent::{Agent, TurnKind};
use crate::connectors;
use crate::store::{ApprovalRow, JobRow};

/// The job step while one of its cards waits: what `list_jobs`, the rollout view and the chat's
/// presence row show, since the card itself is posted in the owner's main chat.
const APPROVAL_WAIT_STEP: &str = "Waiting for approval";

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

/// The state written into the chat doc for a card that has just been decided, which is the row's
/// own state.
///
/// It exists because the doc is the one approval surface read by a client that cannot tolerate a
/// state it has never heard of: the Mac app's `ApprovalState` is a plain serde enum, and an
/// unparseable value there costs the whole message — `read_message` ends `from_value(json).ok()?`
/// over the message and `read_all` filter-maps it, so the assistant's text goes with the card.
/// `cancelled` is new, so every build shipped before it loses that row until the app updates.
///
/// The honest value is the one that ships, and the fix for the client is a tolerant decode that
/// survives the *next* new state as well, not a producer that avoids emitting one. This function
/// is only the hedge: if a bridge is ever preferred over waiting for that client, substituting a
/// state older builds parse is this one line rather than a hunt through call sites. SQLite, the
/// hook replies and ACP would keep the real state either way — they read the row, not the doc.
fn doc_state(row_state: ApprovalState) -> ApprovalState {
    row_state
}

/// The current owner of a pending decision. Inline always owns its sender.
#[derive(Debug)]
pub enum WaitDelivery {
    Inline(oneshot::Sender<ApprovalOutcome>),
    Polled,
    Deferred,
}

#[derive(Clone, Copy)]
enum DeliveryEvent {
    Park,
    Decide,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeliveryAction {
    Parked,
    Unchanged,
    Inline,
    Polled,
    Deferred,
}

/// One transition boundary for caller ownership; the durable decision is committed separately.
fn transition_delivery(delivery: &mut WaitDelivery, event: DeliveryEvent) -> DeliveryAction {
    match event {
        DeliveryEvent::Park => match delivery {
            WaitDelivery::Deferred => DeliveryAction::Unchanged,
            _ => {
                *delivery = WaitDelivery::Deferred;
                DeliveryAction::Parked
            }
        },
        DeliveryEvent::Decide => {
            if matches!(delivery, WaitDelivery::Inline(sender) if sender.is_closed()) {
                // This is still pending: no approved decision has been published or claimed.
                *delivery = WaitDelivery::Deferred;
            }
            match delivery {
                WaitDelivery::Inline(_) => DeliveryAction::Inline,
                WaitDelivery::Polled => DeliveryAction::Polled,
                WaitDelivery::Deferred => DeliveryAction::Deferred,
            }
        }
    }
}

pub struct PendingApproval {
    pub chat_id: String,
    pub msg_id: String,
    pub in_reply_to: String,
    pub request: ApprovalRequest,
    pub delivery: WaitDelivery,
}

#[derive(Default)]
pub struct Approvals {
    pending: Mutex<HashMap<String, PendingApproval>>,
    /// Standing shell grants from the relay (#1544), refreshed on `shell_grants_changed`.
    shell_grants: Mutex<Vec<ShellGrant>>,
    /// Per job id, the absolute paths that job's own writes created: deleting one of those
    /// destroys nothing the user had, so it raises no card (`Agent::note_job_write`).
    job_writes: Mutex<HashMap<String, HashSet<String>>>,
}

/// `approve` payload field a card option B/C sets: the path class to allow from now on.
pub const GRANT_FIELD: &str = "grant";

/// What a parked worker is being resumed with.
#[derive(Debug, Clone, Copy)]
enum Decision<'a> {
    Approved { tool: &'a str },
    Rejected { reason: Option<&'a str> },
}

impl Decision<'_> {
    fn job_step(&self) -> &'static str {
        match self {
            Decision::Approved { .. } => "Approved; carrying on",
            Decision::Rejected { .. } => "Not approved; deciding what to do",
        }
    }

    /// What the resumed worker is told. It is a prompt into a session that still holds the whole
    /// thread, so it says what changed and nothing the session already knows.
    fn brief(&self, title: &str, approval_id: &str) -> String {
        match self {
            Decision::Approved { tool } => format!(
                "[Arbos] The user approved \"{title}\" (approval {approval_id}), so the step you \
                 stopped at can go ahead. Call `{tool}` once more with exactly the same \
                 arguments — it is pre-approved this one time and will not ask again — then \
                 carry on with the work from there.",
            ),
            Decision::Rejected { reason } => format!(
                "[Arbos] The user did not approve \"{title}\" (approval {approval_id}){}. That \
                 step did not run and must not be retried. Carry on without it if the work can \
                 be finished another way; if it cannot, stop and say in one sentence what is \
                 blocked and why.",
                reason
                    .map(str::trim)
                    .filter(|r| !r.is_empty())
                    .map(|r| format!(", saying: {r}"))
                    .unwrap_or_default()
            ),
        }
    }
}

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

/// The chat's status while a card of it waits: `Waiting for your approval: <card title>`.
///
/// It names the card, because the line it replaces — a bare "Waiting for approval" — said that
/// a decision was wanted without saying which, and with two cards in a chat it named neither.
///
/// Clipped, because this is a presence chip and a one-line working row rather than a transcript
/// sentence. That is also why it is not `approval_wait_line`: #2759's line is a full sentence
/// for a transcript row and names the chat the card is in, which is the wrong shape and the
/// wrong length for a status line in the very chat the person is already looking at.
pub(crate) fn wait_status_line(title: &str) -> String {
    const TITLE_MAX: usize = 56;
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = if title.chars().count() > TITLE_MAX {
        format!("{}…", title.chars().take(TITLE_MAX - 1).collect::<String>())
    } else {
        title
    };
    if title.is_empty() {
        return "Waiting for your approval".to_owned();
    }
    format!("Waiting for your approval: {title}")
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

/// How long *anybody* blocks on a card before the call is parked and the card left open
/// (`ARBOS_APPROVAL_PARK_AFTER_MS`).
///
/// **This is the only time bound left on the approval path, and it bounds a wait rather than a
/// card.** There used to be three numbers — the card's 24 h TTL, the extension's 14-minute
/// polling cap, and this grace — and every pair of them could disagree. Test5 round 5 bug R is
/// what that cost: card `01M6MRB89V75ZEX909EDWQJRDV` advertised `expiresAt` at +24 h and was
/// closed by `arbos:timeout` at +14 min, the moment the extension stopped polling. The fix for
/// a disagreement between three numbers is one number.
///
/// Short, because every millisecond of it is a millisecond the caller cannot do anything else:
/// a main-session call holds the chat's turn, so a new message in that chat can only reach the
/// quick lane, and a worker's call holds a `subagent_slots` permit and a live child.
///
/// Not zero, because approving straight away is the common case, and answering inside the grace
/// keeps that case on its original tool call — nothing is parked, nothing is re-run, and no
/// second model call is spent. Ten seconds buys that and still frees the caller long before
/// anyone has typed a follow-up.
///
/// Nothing happens to the card when it passes. The card is answerable until it is answered.
pub(crate) fn park_after_ms() -> i64 {
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
    /// **The card gets no deadline, and no caller may give it one.** A card is a question put to
    /// a person, and the honest answer to "when does it stop being answerable" is "when it is
    /// answered". The parameter that used to say otherwise was the root of the stall this change
    /// removes: whatever cancelled the waiter also closed the card, so an owner who was asleep
    /// came back to work that had quietly stopped. A caller still bounds *its own* wait — see
    /// [`park_after_ms`] — but that is a property of the caller, not of the card.
    pub async fn begin_approval(
        self: &Arc<Self>,
        chat_id: &str,
        req: ApprovalRequest,
        delivery: WaitDelivery,
    ) -> Result<String> {
        let host = self
            .sync
            .host(chat_id)
            .await
            .ok_or_else(|| anyhow!("approval: chat {chat_id} has no doc"))?;
        let approval_id = ulid();
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
            h.doc.add_approval_part(&msg_id, &approval_id, &req, None);
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
                "input_hash": args_hash("approval", &req.payload),
            }),
        );
        let mode = match &delivery {
            WaitDelivery::Inline(_) => "Inline",
            WaitDelivery::Polled => "Polled",
            WaitDelivery::Deferred => "Deferred",
        };
        tracing::info!(approval_id, chat_id, title = %req.title, risk = req.risk.as_str(), mode, "approval requested");
        self.approvals.pending.lock().await.insert(
            approval_id.clone(),
            PendingApproval {
                chat_id: chat_id.to_owned(),
                msg_id,
                in_reply_to,
                request: req,
                delivery,
            },
        );
        // Working line / quick-reply status: the card's own title, not the gated tool's label
        // ("Removing files") — nothing has run yet.
        if !optional {
            self.set_approval_wait_activity(chat_id).await;
        }
        Ok(approval_id)
    }

    /// The chat's status while one of its cards waits, or `None` when none does.
    ///
    /// Read from the store rather than taken as an argument, so the line always names a card
    /// that is really still waiting: answer one of two and the line moves to the other instead
    /// of going stale on the one just decided. `pending_approvals` is ordered by `created_at`,
    /// so the card named is the one that has waited longest.
    ///
    /// An optional card (the secret hand-off's "that looks like a key" offer) asks nothing of
    /// the chat and is skipped, the same exemption `begin_approval` makes for it.
    pub(crate) fn approval_wait_label(&self, chat_id: &str) -> Option<String> {
        self.store
            .pending_approvals()
            .unwrap_or_default()
            .into_iter()
            .find(|r| {
                r.chat_id == chat_id
                    && r.payload.get("optional").and_then(Value::as_bool) != Some(true)
            })
            .map(|r| wait_status_line(&r.title))
    }

    /// Show (or clear) `chat_id`'s waiting-approval status.
    ///
    /// Two things changed here for Test5 round 5 bug R, part 3.
    ///
    /// The label names the card, because "Waiting for approval" said that a decision was wanted
    /// without saying which — and with two cards in a chat it named neither.
    ///
    /// And it no longer gives up when no turn is running. A parked wait outlives the turn that
    /// raised it, which is the whole point of parking, so the chat would otherwise look idle
    /// while it was in fact waiting on the person. The turn's own field is still only touched
    /// when there is a turn to touch; the relay's presence line is posted either way.
    ///
    /// The `sync.host` lookup below has a `None` arm that is unreachable today and becomes live
    /// under #2395 (rooms that actually close). It is reached only with a turn on the lock whose
    /// row is still open, so the room is one the turn itself is holding; and a closed room would
    /// cost the row's `activity` field only, never the presence line, which needs no host.
    async fn set_approval_wait_activity(&self, chat_id: &str) {
        let label = self.approval_wait_label(chat_id);
        let turn_row = {
            let mut tl = self.turn.lock().await;
            match tl.as_mut().filter(|t| t.chat_id == chat_id) {
                Some(t) if t.activity == label => return,
                Some(t) => {
                    t.activity = label.clone();
                    Some((t.assistant_msg_id.clone(), t.row_closed))
                }
                None => None,
            }
        };
        if let Some((msg_id, false)) = turn_row {
            if let Some(host) = self.sync.host(chat_id).await {
                let h = host.lock().await;
                h.doc.set_activity(&msg_id, label.as_deref());
            }
        }
        self.post_activity(chat_id, label);
    }

    /// The shared entry point (brief §4): write the card, wait a short while for
    /// `approve`/`reject`, and return what was decided — or `None`, meaning **nothing was
    /// decided and the card is still open**.
    ///
    /// `None` is not a refusal of the action and must not be reported as one. It says only that
    /// this caller stopped waiting, which it must: an MCP tool call held open indefinitely is
    /// the same stall as a card that expires, just with the agent frozen instead of the work
    /// cancelled. So the wait is bounded by [`park_after_ms`] and the card is not: the user can
    /// still answer it, and the decision is picked up by `follow_up`.
    ///
    /// Callers refuse on `None` ([`may_dispatch`]) and say the card is still waiting.
    pub async fn request_approval(
        self: &Arc<Self>,
        chat_id: &str,
        req: ApprovalRequest,
    ) -> Option<ApprovalOutcome> {
        let (tx, rx) = oneshot::channel();
        let approval_id = match self
            .begin_approval(chat_id, req, WaitDelivery::Inline(tx))
            .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(error = %e, "approval card could not be posted; refusing");
                return Some(ApprovalOutcome::Rejected {
                    reason: Some(format!("approval card could not be posted: {e}")),
                });
            }
        };
        let grace = Duration::from_millis(park_after_ms().max(1) as u64);
        match tokio::time::timeout(grace, rx).await {
            Ok(Ok(outcome)) => Some(outcome),
            // The sender was dropped without a decision — `decide_approval` took the deferred
            // path. The card is open either way, so this is the same answer as the timeout.
            Ok(Err(_)) => None,
            Err(_) => {
                // Hand the card over to the deferred path, exactly as a polled caller's park
                // does, so the decision runs `follow_up` instead of being delivered to a waiter
                // that has gone.
                self.park_approval(&approval_id).await;
                None
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
                    if let Ok(host) = self.room(&row.chat_id).await {
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
            ApprovalOutcome::Approved | ApprovalOutcome::Cancelled => None,
        };
        let row = self
            .store
            .approval(approval_id)?
            .ok_or_else(|| anyhow!("unknown approval {approval_id}"))?;
        let mut pending_entries = self.approvals.pending.lock().await;
        let action = pending_entries
            .get_mut(approval_id)
            .map_or(DeliveryAction::Deferred, |pending| {
                transition_delivery(&mut pending.delivery, DeliveryEvent::Decide)
            });
        let claim = matches!(outcome, ApprovalOutcome::Approved)
            && matches!(action, DeliveryAction::Inline | DeliveryAction::Polled);
        if !self.store.decide_approval(
            approval_id,
            state.as_str(),
            Some(decided_by),
            reason.as_deref(),
            claim,
        )? {
            return Err(anyhow!("approval {approval_id} is already {}", row.state));
        }
        let mut pending = pending_entries.remove(approval_id);
        drop(pending_entries);
        // No await between durable admission and the channel send. A failed send is uncertain:
        // approval_status may already have exposed Approved, so a claimed grant stays spent.
        let deferred = match pending
            .as_mut()
            .map(|pending| std::mem::replace(&mut pending.delivery, WaitDelivery::Deferred))
        {
            Some(WaitDelivery::Inline(sender)) => {
                let undelivered = sender.send(outcome.clone()).is_err();
                if undelivered && claim {
                    tracing::warn!(
                        approval_id,
                        "approved inline delivery abandoned; grant remains spent"
                    );
                }
                // Rejection/cancellation claimed no authority; keep their deferred notification.
                // Published Approved admission remains spent even if its receiver disappeared.
                undelivered && !claim
            }
            Some(WaitDelivery::Polled) => false,
            Some(WaitDelivery::Deferred) | None => true,
        };
        let (chat_id, msg_id) = match &pending {
            Some(p) => (p.chat_id.clone(), p.msg_id.clone()),
            None => (row.chat_id.clone(), row.message_id.clone()),
        };
        // A card waits as long as the user takes, which is far past the idle window, so by now
        // the room is very likely closed (#2367). Without opening it the card in the chat would
        // stay "pending" for ever even though the decision landed.
        if let Ok(host) = self.room(&chat_id).await {
            let h = host.lock().await;
            h.doc.set_approval_state(
                approval_id,
                doc_state(state),
                Some(decided_by),
                reason.as_deref(),
            );
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
        // Re-read the waiting status: it clears when this chat has no undecided card left, and
        // otherwise names the next one rather than staying on the card just decided.
        self.set_approval_wait_activity(&chat_id).await;
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
        if deferred {
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
            // The work was called off, so there is nothing to follow up: whoever cancelled it
            // said so where they cancelled it, and a second notice here would be telling the
            // user about a card they closed themselves.
            ApprovalOutcome::Cancelled => return,
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
                // No grant is recorded here: the approved row *is* the grant (`take_grant`), so
                // the permission is as durable as the decision and survives everything between
                // this moment and the re-run.
                //
                // A card raised by a worker goes back to that worker's own session instead of
                // being answered in the owner's chat, because the work and its context are
                // there. That path acquires the worker permits, which is what keeps a user
                // answering six cards at once from starting six workers at once.
                if let Some(session_id) = request
                    .payload
                    .get("session_id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    self.resume_worker_after_approval(session_id, approval_id, &title, &tool)
                        .await;
                    return;
                }
                prompt.push_str(&format!(" The original `{tool}` call was not executed because the turn had already ended. Route `act`, call it again now with exactly the same arguments (it is pre-approved once), then tell the user the result."));
            }
            ("tool_call", ApprovalOutcome::Rejected { reason }) => {
                if let Some(session_id) = request
                    .payload
                    .get("session_id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    self.resume_worker_after_rejection(
                        session_id,
                        approval_id,
                        &title,
                        reason.as_deref(),
                    )
                    .await;
                    return;
                }
                prompt.push_str(" The call was not executed. Route `reply` and tell the user; do not retry it unchanged.");
            }
            (_, ApprovalOutcome::Approved) => {
                prompt.push_str(" Route `reply` and continue with what the approval unblocked, telling the user what you are doing.");
            }
            (_, ApprovalOutcome::Rejected { .. }) => {
                prompt.push_str(" Route `reply` and acknowledge in one sentence.");
            }
            (_, ApprovalOutcome::Cancelled) => return,
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
        let Some(hit) = classify_tool(tool, args, &self.own_workspace_root()) else {
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
        if may_dispatch(outcome.as_ref()) {
            return Ok(());
        }
        Err(match outcome {
            Some(ApprovalOutcome::Rejected { reason }) => format!(
                "the user rejected this action{}; do not retry it unchanged",
                reason
                    .filter(|r| !r.trim().is_empty())
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default()
            ),
            Some(ApprovalOutcome::Cancelled) => {
                "this work was cancelled, so the approval card was closed unanswered".into()
            }
            // Undecided: the card is open and the user can still answer it. Said as a wait
            // rather than as a refusal, because reporting it as "denied" is a lie the model
            // would pass on to the user.
            None => "nothing ran: this needs the user's approval and the card is still waiting \
                     for them. Do not retry it — Arbos runs the call when they approve"
                .into(),
            Some(ApprovalOutcome::Approved) => unreachable!("approved outcomes dispatch"),
        })
    }

    /// Consume a late-approval grant for an identical re-run (tool + canonical args hash).
    ///
    /// The grant is the approval row, claimed with one conditional update, and every gate reaches
    /// it through here — `gate_tool`, `hook_approval`, `connectors` and `local` — so there is no
    /// second copy of this rule for one of them to get wrong.
    ///
    /// It is single-use and has **no lifetime**. A ten-minute in-memory grant was the previous
    /// answer and it re-carded the user twice over: once on a restart, and once when the re-run
    /// queued behind a busy box's worker slots, which `run_subagent` itself documents as "a wait
    /// of unbounded length". A deadline on a grant is the same mistake as a deadline on a card.
    ///
    /// The match is exact — same tool, same canonical arguments — so what runs is the call the
    /// user read on the card and nothing adjacent to it.
    pub(crate) async fn take_grant(&self, tool: &str, args: &Value) -> Option<String> {
        let h = args_hash(tool, args);
        // Compared in Rust rather than in SQL: there are only ever a handful of unclaimed
        // grants, and the hash is over the canonical form of the payload's `input`, which is
        // not something SQLite's JSON functions can reproduce.
        let row = self
            .store
            .unconsumed_grants()
            .unwrap_or_default()
            .into_iter()
            .find(|r| {
                r.payload.get("tool").and_then(Value::as_str) == Some(tool)
                    && r.payload
                        .get("input")
                        .map(|input| args_hash(tool, input) == h)
                        .unwrap_or(false)
            })?;
        // Lost the race to another caller claiming the same row: that one runs the call, this
        // one is carded, which is the single-use fence doing its job.
        self.store.consume_grant(&row.id).ok()?.then_some(row.id)
    }

    /// The open card this session already has for this exact call, if any.
    ///
    /// Scoped to the session, because two sessions wanting the same thing are two questions: the
    /// user is being told who wants to do it, and answering for a worker should not silently
    /// authorize the main agent as well.
    fn open_card_for(&self, tool: &str, args: &Value, session_id: &str) -> Option<ApprovalRow> {
        let h = args_hash(tool, args);
        self.store
            .pending_approvals()
            .unwrap_or_default()
            .into_iter()
            .find(|r| {
                r.payload.get("session_id").and_then(Value::as_str) == Some(session_id)
                    && r.payload.get("tool").and_then(Value::as_str) == Some(tool)
                    && r.payload
                        .get("input")
                        .map(|input| args_hash(tool, input) == h)
                        .unwrap_or(false)
            })
    }

    /// The grant for this exact call that has already been used, if there is one.
    ///
    /// Only for telling the user why they are being asked twice. A re-run that died part-way
    /// through is indistinguishable at this gate from one that never started — the gate sees the
    /// call, never its result — so a second attempt is carded again rather than waved through on
    /// the strength of an answer given about the first. The card says so.
    fn spent_grant_for(&self, tool: &str, args: &Value) -> Option<ApprovalRow> {
        let h = args_hash(tool, args);
        self.store
            .consumed_grants()
            .unwrap_or_default()
            .into_iter()
            .find(|r| {
                r.payload.get("tool").and_then(Value::as_str) == Some(tool)
                    && r.payload
                        .get("input")
                        .map(|input| args_hash(tool, input) == h)
                        .unwrap_or(false)
            })
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
    /// - what is recorded is one exact absolute path, resolved against this project's own
    ///   folder: where the file really is, and the working directory both the harness child and
    ///   the matcher read a relative target against. It is checked against the disk on its own.
    pub(crate) async fn note_job_write(&self, job_id: &str, tool: &str, input: &Value) {
        let tool = crate::activity::bare_tool_name(tool);
        let work_dir = self.own_workspace_root();
        let targets: Vec<String> = if WRITE_TOOLS.contains(&tool) {
            let Some(path) = ["path", "file_path"]
                .iter()
                .find_map(|k| input.get(k).and_then(Value::as_str))
            else {
                return;
            };
            vec![resolve_path(&work_dir, path)]
        } else if arbos_core::approval::SHELL_TOOLS.contains(&tool) {
            let cmd = ["command", "cmd"]
                .iter()
                .find_map(|k| input.get(k).and_then(Value::as_str))
                .unwrap_or_default();
            if arbos_core::approval::delete_targets(cmd).is_some() {
                return;
            }
            arbos_core::approval::redirect_targets(&work_dir, cmd)
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
        let workspace_root = self.own_workspace_root();
        let Some(job) = job else {
            return classify_tool(tool, args, &workspace_root);
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
                workspace_root: &workspace_root,
            },
        )
    }

    /// This agent's own workspace: the folder its harness children run in, and the boundary the
    /// deletion rule is scoped to.
    ///
    /// Normalized, because the matcher compares it against paths it has normalized itself: a
    /// configured root carrying a `.` or `..` segment would match none of them and would read
    /// as a box this agent owns no part of.
    pub(crate) fn own_workspace_root(&self) -> String {
        resolve_path("/", &self.cfg.work_dir.to_string_lossy())
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
            // The job's chat, not the card's: its room may be closed while the card's is open,
            // and this notice is the only thing that says there why the job stopped (#2367).
            if let Ok(host) = self.room(&job.chat_id).await {
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
                // Reached only when the room cannot be opened at all — a chat the agent was
                // removed from. Before #2395 this arm meant "the room happens to be shut",
                // which no box ever was, so it was unreachable rather than rare.
                tracing::debug!(job = %job.id, chat = %job.chat_id, "the job's chat cannot be opened; its card notice is in the rollout view only");
            }
        }
        self.refresh_job_activity(&job.chat_id).await;
    }

    /// The job a worker session runs, when its card belongs to one.
    fn job_of_session(&self, session_id: &str) -> Option<JobRow> {
        self.store.job_for_subagent(session_id).ok().flatten()
    }

    /// Where a card went, by a name a person can be sent to.
    ///
    /// A ULID is not a place anyone can be asked to go, so this is what a caller may say out
    /// loud; the id travels beside it for exactness. A chat with no title falls back to a
    /// pointer to the main chat, which is where `approval_chat` puts a card with nowhere better
    /// to go.
    pub(crate) fn card_chat_name(&self, chat_id: &str) -> String {
        self.store
            .chat(chat_id)
            .ok()
            .flatten()
            .map(|c| c.title)
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "your main chat".to_owned())
    }

    /// A worker's card was answered: send the worker's **own session** back to finish the work.
    ///
    /// This is the other half of parking a worker. The park ended the worker's run so the card
    /// would not pin a slot, a child process and a job for however long the user took; the
    /// answer has to start a run again, and it goes to the session that was doing the work so
    /// the thread, the files it had open and the plan it was following are all still there.
    ///
    /// It goes through `spawn_subagent_task` → `run_subagent` rather than through
    /// `run_approval_turn`, and that is load-bearing rather than incidental: `run_subagent`
    /// acquires `subagent_slots` and `box_slots` before it runs anything, so a user who comes
    /// back and answers six cards at once starts them at the concurrency the box allows instead
    /// of six workers at once. `run_approval_turn` acquires nothing, which is correct for a
    /// notice in the owner's chat and wrong for a worker.
    ///
    /// The job goes back to `running` from the `queued` it was parked in. Nothing else needs to
    /// change: the row never left the open set, so it stayed listed, stayed cancellable, and
    /// kept its place in the per-chat cap the whole time it was waiting.
    async fn resume_worker_after_approval(
        self: &Arc<Self>,
        session_id: &str,
        approval_id: &str,
        title: &str,
        tool: &str,
    ) {
        let decision = Decision::Approved { tool };
        self.resume_worker_with(session_id, approval_id, title, decision)
            .await;
    }

    /// The same resume for a card the user turned down: the worker is told, and decides what to
    /// do about it.
    ///
    /// Telling the worker beats failing the job here. The gate refused one step; whether the
    /// work can continue without it is a question about the work, and the only thing holding
    /// enough context to answer it is the session that was doing it.
    async fn resume_worker_after_rejection(
        self: &Arc<Self>,
        session_id: &str,
        approval_id: &str,
        title: &str,
        reason: Option<&str>,
    ) {
        self.resume_worker_with(
            session_id,
            approval_id,
            title,
            Decision::Rejected { reason },
        )
        .await;
    }

    async fn resume_worker_with(
        self: &Arc<Self>,
        session_id: &str,
        approval_id: &str,
        title: &str,
        decision: Decision<'_>,
    ) {
        let Some(sub) = self.store.subagent(session_id).ok().flatten() else {
            tracing::warn!(
                approval_id,
                session_id,
                "the card's worker session is gone; nothing to resume"
            );
            return;
        };
        let job = self.job_of_session(session_id);
        // A job the user cancelled while its card waited is not restarted by answering the
        // card. The cancel is the later instruction and it already closed the work down.
        if let Some(j) = job.as_ref().filter(|j| !j.is_open()) {
            tracing::info!(job = %j.id, approval_id, state = %j.state, "the card's job is closed; the answer starts nothing");
            return;
        }
        let Some(chat_id) = job
            .as_ref()
            .map(|j| j.chat_id.clone())
            .or_else(|| sub.chat_id.clone())
        else {
            tracing::warn!(
                approval_id,
                session_id,
                "the card's worker belongs to no chat; nothing to resume into"
            );
            return;
        };
        let Some(host) = self.sync.host(&chat_id).await else {
            tracing::warn!(
                approval_id,
                chat_id,
                "no doc for the card's chat; cannot resume"
            );
            return;
        };
        let brief = decision.brief(title, approval_id);
        let _ = self.store.t3_append(
            session_id,
            Some(session_id),
            Some(session_id),
            "approval",
            Some("system"),
            &brief,
            Some(&json!({ "approval_id": approval_id, "resumed": true })),
        );
        if let Some(j) = &job {
            let _ = self.store.set_job_state(&j.id, "running", None);
            let _ = self.store.set_job_step(&j.id, decision.job_step());
        }
        let in_reply_to = job
            .as_ref()
            .map(|j| j.ask_message_id.clone())
            .unwrap_or_default();
        self.spawn_subagent_task(
            &chat_id,
            &host,
            &in_reply_to,
            Some(session_id.to_owned()),
            String::new(),
            job.as_ref().map(|j| j.title.clone()).unwrap_or_default(),
            brief,
            Some(arbos_core::route::Harness::Pi),
            false,
            false,
            // The attempt count is carried, not reset: see `resumed_job_spec`.
            job.as_ref().map(crate::agent::resumed_job_spec),
            // The ask this job answers is not waiting on this run: the worker was already
            // speaking for itself before the card, and the result turn it finishes with is what
            // the chat is ordered on.
            None,
        );
        tracing::info!(approval_id, session_id, job = ?job.as_ref().map(|j| &j.id), "a card was answered; its worker is resumed");
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
        // This session already has a card waiting for this exact call, so it is pointed at that
        // one rather than asked the same question twice.
        //
        // This is what carries Test5 round 2's bug H now that a card cannot close itself. Four
        // cards went to T8, each one a retry of the same refused call, because the previous card
        // had expired and the call was tried again. A card that waits for ever removes the
        // expiry half of that; this removes the other half, which is the one that was actually
        // the bug: the user is asked once per question, not once per attempt.
        //
        // After the grant checks above, both of them, because an open card must not stand in
        // the way of a permission the user has already given: a standing shell grant recorded
        // while a card was waiting lets the call run, and the card it leaves behind is closed
        // when the work it belonged to finishes or is cancelled.
        if let Some(open) = self.open_card_for(tool, input, session_id) {
            let where_it_is = self.card_chat_name(&open.chat_id);
            tracing::info!(approval_id = %open.id, tool, session_id, "this call already has a card waiting; no second one is raised");
            return json!({ "ok": true, "state": "pending", "approvalId": open.id, "title": open.title,
                "why": hit.why, "waitMs": park_after_ms(), "park": true, "chat": where_it_is, "chatId": open.chat_id });
        }
        let (title, summary) = describe_tool(tool, input, &hit);
        // A second card for a call that was already approved once says so, rather than reading
        // as a fresh request the user has never seen. The grant is single-use on purpose — the
        // gate sees the call and never its result, so a re-run that died part-way through is
        // indistinguishable from one that never started, and a half-finished destructive
        // command is exactly where a person should look again.
        let prior = self.spent_grant_for(tool, input);
        let retry_note = prior.as_ref().map(|_| {
            "This exact call was approved once already and was attempted; it did not report back, \
             so it needs approving again."
        });
        let mut payload = json!({ "kind": "tool_call", "tool": tool, "input": input, "class": hit.class, "why": hit.why, "session_id": session_id, "role": role,
            "path_class": hit.path_class.map(PathClass::as_str), "grant_options": grant_options(&hit) });
        // `job_id` beside `session_id`, so "is this job parked on a card" is a direct read. The
        // two are different questions at different grains and both have callers: the job for the
        // capacity cap and the boot reclaim, the session for the retained-worker bound.
        if let Some(j) = &job {
            payload["job_id"] = json!(j.id);
        }
        if let Some(p) = &prior {
            payload["prior_approval_id"] = json!(p.id);
            payload["prior_consumed_at"] = json!(p.grant_consumed_at);
        }
        let req = ApprovalRequest {
            title: title.clone(),
            summary: match retry_note {
                Some(note) if summary.is_empty() => note.to_owned(),
                Some(note) => format!("{summary} — {note}"),
                None => summary.clone(),
            },
            risk: hit.risk,
            payload,
        };
        // **Every** gated call parks, which is the change: the caller waits out the grace and
        // then lets go, and the card stays open for the user however long they take.
        //
        // A worker used to be excluded here, on the reasoning that it had no chat turn to
        // release. But it holds more than a turn — a `subagent_slots` permit, a `box_slots`
        // permit, a live harness child and an open job — and it held all of them for the
        // extension's fourteen-minute cap, after which the card was closed and the job failed
        // unasked. Releasing the worker and keeping the card is the inversion of that.
        match self
            .begin_approval(&chat_id, req, WaitDelivery::Polled)
            .await
        {
            Ok(id) => {
                // Where the card is, by name: a card goes to the chat the owner reads, which is
                // rarely the chat the call came from.
                let where_it_is = self.card_chat_name(&chat_id);
                if let Some(j) = &job {
                    self.note_job_awaits_approval(
                        j,
                        &chat_id,
                        &approval_wait_line(&title, &summary, &where_it_is),
                        &json!({ "approval_id": id, "chat_id": chat_id, "job_id": j.id }),
                    )
                    .await;
                }
                // `chat` and `chatId` are the card's location, for the one session that can act
                // on it. This reply goes to the session whose call was gated, which is the
                // session that otherwise tells the user "the card is in this chat" — true only
                // when the two happen to coincide. The name is what it may say; the id is there
                // so it is exact. The model is told nothing about location anywhere else.
                if let Some(p) = &prior {
                    self.append_audit(
                        "approval_recarded",
                        json!({ "approval_id": id, "prior_approval_id": p.id, "chat_id": chat_id,
                            "tool": tool, "session_id": session_id, "job_id": job.as_ref().map(|j| &j.id) }),
                    );
                }
                json!({ "ok": true, "state": "pending", "approvalId": id, "title": title, "why": hit.why,
                    "waitMs": park_after_ms(), "park": true, "chat": where_it_is, "chatId": chat_id })
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

    /// `/hook/approval-park`: the caller stopped blocking on this card and is letting go of
    /// whatever it was holding. **The card is untouched and stays answerable.**
    ///
    /// This is Test5 round 5 bug R's second half, now applied to every caller rather than to the
    /// main session alone. A gated call used to hold its caller for the whole wait — fourteen
    /// minutes in rounds 4 and 5 — and nothing about an approval needs that: the card is in the
    /// chat, and the user answers the card.
    ///
    /// What the caller lets go of depends on who it is, and the worker is why this matters:
    ///
    /// - a **main-session** call holds the chat's turn, so a message arriving meanwhile could
    ///   only get a quick-lane answer;
    /// - a **worker** holds a `subagent_slots` permit, a `box_slots` permit, a live harness
    ///   child and an open job. Parking returns all four, so an unanswered card costs the box
    ///   nothing while it waits and the retained-worker bound can evict the idle worker.
    ///
    /// The mode moves to `Deferred`, which is the whole mechanism. Everything that finishes the
    /// work already exists for the late-approval case and is reached by that one change:
    ///
    /// - `decide_approval` treats a `Deferred` entry as deferred, so the decision runs
    ///   `follow_up` instead of handing the outcome to a waiter that has gone;
    /// - `follow_up` either prompts the card's chat, or — for a worker's card — resumes that
    ///   worker's own session through `run_subagent` (`resume_worker_after_approval`);
    /// - `take_grant` claims the approved row's single-use grant, so the re-run goes through
    ///   without a second card no matter how long it was between the two.
    ///
    /// Nothing here is a new waiting state, and `draft_message` has used this path since S18.
    pub async fn park_approval(self: &Arc<Self>, approval_id: &str) -> Value {
        let (row, parked) = {
            let mut pending = self.approvals.pending.lock().await;
            let Some(row) = self.store.approval(approval_id).ok().flatten() else {
                return json!({ "ok": false, "error": "unknown approval" });
            };
            if row.state != ApprovalState::Pending.as_str() {
                return json!({ "ok": true, "state": row.state, "parked": false });
            }
            let parked = pending.get_mut(approval_id).is_some_and(|pending| {
                transition_delivery(&mut pending.delivery, DeliveryEvent::Park)
                    == DeliveryAction::Parked
            });
            (row, parked)
        };
        if !parked {
            return json!({ "ok": true, "state": row.state, "parked": false });
        }
        self.set_approval_wait_activity(&row.chat_id).await;
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
            json!({ "approval_id": approval_id, "chat_id": row.chat_id, "title": row.title }),
        );
        tracing::info!(approval_id, chat_id = %row.chat_id, "approval parked; the caller is released and the card stays open");
        json!({ "ok": true, "state": row.state, "parked": true, "reason": parked_reason(approval_id) })
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

    /// Close every open card of `chat_id`, or of one job, as `cancelled`: the work was called
    /// off, so there is nothing left to approve.
    ///
    /// `cancelled` rather than `rejected`, because the user refused nothing — they stopped the
    /// work — and telling the agent it was turned down would be a different fact. And rather
    /// than `expired`, which no longer happens and which meant "nobody answered in time".
    ///
    /// This is the only thing that closes a card without an answer, and it is always something
    /// the user did.
    pub(crate) async fn cancel_approvals_for_job(self: &Arc<Self>, job_id: &str) {
        let open: Vec<String> = self
            .store
            .pending_approvals()
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.payload.get("job_id").and_then(Value::as_str) == Some(job_id))
            .map(|r| r.id)
            .collect();
        for id in open {
            if let Err(e) = self
                .decide_approval(&id, ApprovalOutcome::Cancelled, "arbos:cancelled")
                .await
            {
                tracing::debug!(approval_id = %id, error = %e, "a cancelled job's card was already decided");
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
        // Outside the agent's workspace, because that is the only thing a file-delete card is
        // raised for now: a delete inside `/work/<project>` never reaches a grant.
        let outside = json!({ "command": "rm -rf /opt/cache" });
        let any = json!({ "command": "rm -rf /etc/x" });

        // No grant: the card is raised and offers "anywhere" alone.
        let v = agent.hook_approval("bash", &outside, chat, "main").await;
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
        assert_eq!(opts, ["anywhere"]);
        assert_eq!(first.payload["path_class"], "anywhere");

        // Neither a scratch nor a workspace grant reaches outside the workspace; "anywhere"
        // does.
        for narrow in [PathClass::Scratch, PathClass::Workspace] {
            *agent.approvals.shell_grants.lock().await = vec![grant(narrow, None)];
            assert_eq!(
                agent.hook_approval("bash", &outside, chat, "main").await["state"],
                "pending",
                "a {} grant must not cover /opt",
                narrow.as_str()
            );
        }
        *agent.approvals.shell_grants.lock().await = vec![grant(PathClass::Anywhere, None)];
        let v = agent.hook_approval("bash", &outside, chat, "main").await;
        assert_eq!(v["state"], "allowed", "{v}");
        assert_eq!(v["grantId"], "g-anywhere");
        assert!(agent.gate_tool("bash", &outside, chat).await.is_ok());

        // Per-chat grant: only that chat.
        *agent.approvals.shell_grants.lock().await =
            vec![grant(PathClass::Anywhere, Some("other"))];
        assert_eq!(
            agent.hook_approval("bash", &outside, chat, "main").await["state"],
            "pending"
        );

        // Revoked (the relay's list no longer has it): the card is back.
        agent.approvals.shell_grants.lock().await.clear();
        assert_eq!(
            agent.hook_approval("bash", &any, chat, "main").await["state"],
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
        let outside = json!({ "command": "cd /opt/app && rm -rf node_modules" });
        let v = agent.hook_approval("bash", &outside, chat, "main").await;
        let id = v["approvalId"].as_str().unwrap().to_owned();
        agent
            .decide_approval_from_command(
                &id,
                ApprovalOutcome::Approved,
                "dev-mac",
                &json!({ "approvalId": id, GRANT_FIELD: "anywhere" }),
            )
            .await
            .unwrap();
        assert_eq!(agent.approval_status(&id).await["state"], "approved");
        let grants = agent.approvals.shell_grants.lock().await.clone();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].path_class, PathClass::Anywhere);
        assert_eq!(grants[0].granted_by, "dev-mac");
        assert_eq!(grants[0].chat_id, None);
        let v = agent
            .hook_approval(
                "bash",
                &json!({ "command": "rm -rf /opt/dist" }),
                chat,
                "main",
            )
            .await;
        assert_eq!(v["state"], "allowed", "{v}");
        // A plain approve (option A) adds nothing. The grant just recorded would cover this
        // command, so it is dropped first to get a card at all.
        agent.approvals.shell_grants.lock().await.clear();
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
        assert!(agent.approvals.shell_grants.lock().await.is_empty());
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

    fn shell_argument(path: impl AsRef<std::path::Path>) -> String {
        format!(
            "'{}'",
            path.as_ref().to_string_lossy().replace('\'', "'\\''")
        )
    }

    /// An agent with one running job in a pinned chat, as `start_job` leaves it.
    async fn agent_running_a_job() -> (Arc<Agent>, SharedHost, std::path::PathBuf) {
        job_agent(test_agent().await).await
    }

    /// The same, on a box whose workspace is not inside a scratch root, for the tests that are
    /// about where the workspace boundary falls.
    async fn agent_running_a_job_outside_scratch() -> (Arc<Agent>, SharedHost, std::path::PathBuf) {
        job_agent(crate::status::tests::test_agent_outside_scratch().await).await
    }

    async fn job_agent(
        (agent, root): (Arc<Agent>, std::path::PathBuf),
    ) -> (Arc<Agent>, SharedHost, std::path::PathBuf) {
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

    /// The premise the two tests below rest on, asserted here rather than inherited from
    /// wherever the checkout sits: one delete, in the same place relative to the box, is
    /// allowed on a box whose workspace is inside a scratch root and carded on a box outside
    /// one.
    ///
    /// Both sides are asserted because only one of them can fail by itself. A box inside
    /// scratch cards nothing, so "this delete needs no card" holds there for a reason that has
    /// nothing to do with the workspace boundary and would keep holding with the boundary
    /// deleted — the quiet direction. Asking the same question of both fixtures makes any
    /// wrong premise break the other arm instead, loudly, and in one test rather than in every
    /// test that happens to use the fixture.
    #[tokio::test]
    async fn the_scratch_boundary_decides_the_card_and_both_sides_are_asserted() {
        let ask = |agent: Arc<Agent>, root: std::path::PathBuf| async move {
            // Another project's folder beside the box's own workspace: outside the boundary on
            // a box the boundary applies to, and in scratch on a box that sits in scratch.
            let target = root.join("01TESTOTHERPROJECT00000000").join("exports");
            let class = arbos_core::approval::classify_path(
                &target.to_string_lossy(),
                &agent.cfg.work_dir.to_string_lossy(),
            );
            let state = agent
                .hook_approval(
                    "bash",
                    &json!({ "command": format!("rm -rf {}", shell_argument(&target)) }),
                    WORKER,
                    "subagent",
                )
                .await["state"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            (class, state)
        };

        let (agent, _host, root) = agent_running_a_job().await;
        assert_eq!(
            ask(agent, root.clone()).await,
            (
                arbos_core::approval::PathClass::Scratch,
                "allowed".to_owned()
            ),
            "the ordinary fixture's root is `std::env::temp_dir()`, which is a scratch root, so \
             every path on that box reads as scratch and nothing in it is carded. A test about \
             the workspace boundary must use `test_agent_outside_scratch` instead. If this arm \
             fails, that box has moved out of scratch — check `TMPDIR` — and the comment on \
             `test_agent_outside_scratch` is now wrong. Root: {}",
            root.display(),
        );
        let _ = std::fs::remove_dir_all(&root);

        let (agent, _host, root) = agent_running_a_job_outside_scratch().await;
        assert_eq!(
            ask(agent, root.clone()).await,
            (
                arbos_core::approval::PathClass::Anywhere,
                "pending".to_owned()
            ),
            "off this box's own workspace and out of scratch, a delete is carded. If this arm \
             fails while the other passes, the checkout itself is under a scratch root and the \
             boundary cases here were measuring the checkout's location. Root: {}",
            root.display(),
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The deletion rule flagged files the job itself had just made. T8's own outputs —
    /// `out/taxi_app_curl.txt`, `out/taxi_app.zip` — are not even a question now: they sit in
    /// the job's own workspace, and nothing in there is carded.
    ///
    /// So what this holds is the half of #1719 that still does work. A job may also write
    /// outside its workspace, and there a file it created itself is exempt while everything
    /// around it keeps its card: a file the job did not write, the folder holding its file, a
    /// glob, a write over a file that was already there, a command that deletes while it
    /// redirects, and the same delete from a session that is no job of ours.
    #[tokio::test]
    async fn a_job_deleting_a_file_it_created_itself_raises_no_card() {
        let (agent, _host, root) = agent_running_a_job_outside_scratch().await;
        // A folder beside this agent's workspace: another project's, as far as the rule is
        // concerned, so a delete in it needs a card unless the exemption carries it.
        let outside = root.join("01TESTOTHERPROJECT00000000").join("out");
        let out = |name: &str| outside.join(name).to_string_lossy().to_string();
        agent
            .note_job_write(
                JOB,
                "bash",
                &json!({ "command": format!("curl -sS https://example.invalid/x > {}", shell_argument(out("taxi_app_curl.txt"))) }),
            )
            .await;
        agent
            .note_job_write(JOB, "write", &json!({ "path": out("taxi_app.zip") }))
            .await;
        let state = |cmd: String| {
            let agent = agent.clone();
            async move {
                agent
                    .hook_approval("bash", &json!({ "command": cmd }), WORKER, "subagent")
                    .await["state"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            }
        };
        assert_eq!(
            state(format!(
                "rm -f {}",
                shell_argument(out("taxi_app_curl.txt"))
            ))
            .await,
            "allowed"
        );
        assert_eq!(
            state(format!("rm -f {}", shell_argument(out("taxi_app.zip")))).await,
            "allowed"
        );
        assert_eq!(
            state(format!("rm -f {}", shell_argument(out("vendor.zip")))).await,
            "pending"
        );
        assert_eq!(
            state(format!(
                "rm -f {} {}",
                shell_argument(out("taxi_app.zip")),
                shell_argument(out("vendor.zip"))
            ))
            .await,
            "pending",
            "one file it did not write keeps the card"
        );
        assert_eq!(
            state(format!("rm -rf {}", shell_argument(&outside))).await,
            "pending",
            "the folder that holds its file is not its file"
        );
        assert_eq!(
            state(format!("rm -f {}/*.zip", shell_argument(&outside))).await,
            "pending",
            "a glob is not an exact path"
        );
        // A command that deletes records nothing, so no gated command widens its own exemption.
        agent
            .note_job_write(
                JOB,
                "bash",
                &json!({ "command": format!("rm -f {} > {}", shell_argument(out("old.zip")), shell_argument(out("clean.log"))) }),
            )
            .await;
        assert_eq!(
            state(format!("rm -f {}", shell_argument(out("clean.log")))).await,
            "pending"
        );
        // A write over a file that is already there creates nothing, so it records nothing. The
        // file stands for one the job found rather than made, and it is made here under the
        // fixture's own root: a path borrowed from the checkout (`CARGO_MANIFEST_DIR`) would
        // carry whatever class the checkout's location gives it.
        std::fs::create_dir_all(&outside).unwrap();
        let found = outside.join("found.txt");
        std::fs::write(&found, "written by someone else").unwrap();
        let already_there = found.to_string_lossy().to_string();
        agent
            .note_job_write(JOB, "write", &json!({ "path": already_there }))
            .await;
        assert_eq!(
            state(format!("rm -f {}", shell_argument(&already_there))).await,
            "pending"
        );
        // The same delete from a session that is no job of ours: nothing is exempt.
        assert_eq!(
            agent
                .hook_approval(
                    "bash",
                    &json!({ "command": format!("rm -f {}", shell_argument(out("taxi_app.zip"))) }),
                    CHAT,
                    "main"
                )
                .await["state"],
            "pending"
        );
        // The job's own workspace needs no exemption at all: these were never recorded as its
        // writes, and they are allowed because of where they are.
        assert_eq!(state("rm -rf out".into()).await, "allowed");
        assert_eq!(state("rm -f out/vendor.zip".into()).await, "allowed");
        assert_eq!(
            state("find . -name __pycache__ -exec rm -rf {} +".into()).await,
            "allowed"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Jacob's stall, at the gate rather than in the matcher: the agent's own `rm` inside
    /// `/work/<project>` is allowed outright, and the guard still cards a delete in another
    /// project's folder and in a system path. Both halves, because the permissive one alone
    /// would pass with the rule deleted.
    #[tokio::test]
    async fn the_gate_allows_the_agents_own_workspace_and_cards_everything_else() {
        let (agent, _host, root) = agent_running_a_job_outside_scratch().await;
        let state = |cmd: String| {
            let agent = agent.clone();
            async move {
                agent
                    .hook_approval("bash", &json!({ "command": cmd }), WORKER, "subagent")
                    .await["state"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            }
        };
        // The command from the incident, as the harness sent it.
        let incident = "sed -i 's/limit = 1000/limit = 10000/' p018.py; for f in p*.py; do python3 \"$f\" >> results.csv; done; du -sh .; rm d*.html.bak";
        assert_eq!(state(incident.into()).await, "allowed");
        assert_eq!(
            state(format!(
                "rm -rf {}",
                shell_argument(agent.cfg.work_dir.join("build"))
            ))
            .await,
            "allowed"
        );
        // Another project's folder, the box's own state, and the system.
        let other = root.join("01TESTOTHERPROJECT00000000");
        assert_eq!(
            state(format!("rm -rf {}", shell_argument(other.join("exports")))).await,
            "pending"
        );
        assert_eq!(
            state(format!(
                "rm -f {}",
                shell_argument(root.join(".arbos/audit.jsonl"))
            ))
            .await,
            "pending"
        );
        assert_eq!(state("rm -rf /etc/ssh".into()).await, "pending");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The inversion of `one_expired_card_fails_the_job_instead_of_raising_another`.
    ///
    /// That test asserted the old answer to the livelock — one unanswered card *failed* the job
    /// so no fresh worker could raise a fifth card — and the livelock it guarded against is
    /// still covered here, by the part that was the actual defect: no second card is raised for
    /// the same call. What changes is everything built on the card closing itself. An unanswered
    /// worker card now **parks**: the card stays open and answerable, the job stays open and
    /// cancellable, and nothing fails.
    #[tokio::test]
    async fn an_unanswered_worker_card_parks_the_job_instead_of_failing_it() {
        let (agent, _host, root) = agent_running_a_job().await;
        let cmd = json!({ "command": "rm -rf /work/p/exports" });
        let v = agent.hook_approval("bash", &cmd, WORKER, "subagent").await;
        let id = v["approvalId"].as_str().expect("a card").to_owned();
        // Every role parks now, and the wait the caller is told to make is the grace rather than
        // a cap on the card.
        assert_eq!(v["park"], json!(true), "{v}");
        assert_eq!(v["waitMs"], json!(park_after_ms()), "{v}");
        // The worker stopped waiting. Under the old behaviour this expired the card; now it
        // releases the caller and leaves the question standing.
        let parked = agent.park_approval(&id).await;
        assert_eq!(parked["parked"], json!(true), "{parked}");
        assert_eq!(parked["state"], "pending", "{parked}");
        let row = agent.store.approval(&id).unwrap().unwrap();
        assert_eq!(row.state, "pending");
        assert_eq!(row.decided_by, None, "nothing decided this card");
        // The job did not fail and did not end. It is open, so it is still listed and still
        // cancellable, and its step says what it is waiting for.
        let job = agent.store.job(JOB).unwrap().unwrap();
        assert!(job.is_open(), "a parked job stays open: {}", job.state);
        assert_eq!(job.error, None);
        // The payload carries both grains, which is what the bound and the reclaim read.
        assert_eq!(row.payload["session_id"], json!(WORKER));
        assert_eq!(row.payload["job_id"], json!(JOB));
        assert_eq!(
            agent
                .store
                .pending_approval_for_session(WORKER)
                .unwrap()
                .map(|r| r.id),
            Some(id.clone())
        );
        assert_eq!(
            agent
                .store
                .pending_approval_for_job(JOB)
                .unwrap()
                .map(|r| r.id),
            Some(id.clone())
        );
        // The same call again does not raise a second card: the one that is waiting is reused,
        // which is the half of the old test that was really about the livelock.
        let again = agent.hook_approval("bash", &cmd, WORKER, "subagent").await;
        assert_eq!(again["state"], "pending", "{again}");
        assert_eq!(
            agent.store.pending_approvals().unwrap().len(),
            1,
            "one open card for this call, not one per attempt"
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
                &json!({ "command": "git -C /opt/repo clean -fdX" }),
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
            rows[0].contains("git -C /opt/repo clean -fdX"),
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

/// What a parked job does to the rest of the machinery: the boot reclaim, the two job caps, and
/// the burst of answers that arrives when the user finally comes back.
#[cfg(test)]
mod parked_job_tests {
    use super::*;
    use crate::status::tests::{test_agent, test_agent_with_pi};

    const CHAT: &str = "01TESTCHATPARKEDJOB0000000";

    /// A worker that starts, answers the handshake and settles having done nothing. Enough to
    /// reach `run_subagent`'s settle, which is where the park decision is made — and a run that
    /// produced nothing is the right shape for it, because the settle reads the card rather than
    /// anything the model returned.
    const SETTLING_PI: &str = r#"#!/usr/bin/env python3
import json, sys

def reply(cmd, cid, data=None):
    sys.stdout.write(json.dumps({"type": "response", "id": cid, "command": cmd, "success": True, "data": data}) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    c = json.loads(line)
    kind, cid = c.get("type"), c.get("id")
    if kind == "get_state":
        reply(kind, cid, {"isStreaming": False})
        continue
    reply(kind, cid)
    if kind == "prompt":
        sys.stdout.write(json.dumps({"type": "agent_settled", "aborted": False}) + "\n")
        sys.stdout.flush()
"#;

    /// A parked job and its card, as a park leaves them: job open at `queued` with no worker
    /// running, card `pending` naming both the worker and the job.
    async fn a_parked_job(agent: &Arc<Agent>, n: usize) -> (String, String, String) {
        let job = format!("01TESTPARKEDJOB00000000{n:03}");
        let worker = format!("01TESTPARKEDWORKER00000{n:03}");
        let card = format!("01TESTPARKEDCARD00000000{n:02}");
        agent
            .store
            .insert_subagent(&worker, &format!("worker-{n}"), "work", "pi", CHAT)
            .unwrap();
        agent
            .store
            .set_subagent_status(&worker, "waiting_user")
            .unwrap();
        agent
            .store
            .insert_job(
                &job,
                CHAT,
                "01TESTPARKEDASK00000000000",
                "Clearing the old exports",
                "code",
                "Delete last month's export files.",
                &[],
            )
            .unwrap();
        agent.store.set_job_subagent(&job, &worker).unwrap();
        agent.store.set_job_state(&job, "queued", None).unwrap();
        agent
            .store
            .insert_approval(
                &card,
                CHAT,
                "01TESTPARKEDCARDMESSAGE000",
                "Run a destructive command (file-delete)",
                "destructive",
                &json!({ "kind": "tool_call", "tool": "bash",
                    "input": { "command": format!("rm -rf /opt/x{n}") },
                    "session_id": worker, "job_id": job }),
            )
            .unwrap();
        (job, worker, card)
    }

    /// **The sharpest of the six sites.** `reclaim_interrupted_jobs` walks `open_jobs()` at boot
    /// and marks every one `interrupted` with "box restart" — and a parked job is open.
    ///
    /// Left untouched, because it was not interrupted: it was waiting for the user before the
    /// restart and is still waiting after, which is the survive-a-restart property the whole
    /// change exists for. Without this the work would be restarted from the brief while the
    /// card still stood, and the user's answer would arrive for a job that had moved on.
    ///
    /// A unit test rather than canary coverage: a canary catches this only if its scenario
    /// happens to include a restart.
    #[tokio::test]
    async fn the_boot_reclaim_leaves_a_parked_job_parked() {
        let (agent, root) = test_agent().await;
        agent.store.upsert_chat(CHAT, "Main", true, 0).unwrap();
        let _host = agent.sync.open_room(CHAT).await.unwrap();
        let (parked, _w, card) = a_parked_job(&agent, 1).await;
        // A job that really was cut off by the restart, so the sweep is doing its job here and
        // the parked one is not simply being skipped along with everything else.
        let cut_off = "01TESTCUTOFFJOB00000000001";
        agent
            .store
            .insert_job(
                cut_off,
                CHAT,
                "01TESTPARKEDASK00000000000",
                "Mid-flight work",
                "code",
                "Do the thing.",
                &[],
            )
            .unwrap();
        agent.store.set_job_state(cut_off, "running", None).unwrap();

        agent.reclaim_interrupted_jobs().await;

        let row = agent.store.job(&parked).unwrap().unwrap();
        assert_eq!(
            row.state, "queued",
            "a parked job is not interrupted by a restart"
        );
        assert_eq!(row.error, None, "and carries no 'box restart' reason");
        assert_eq!(
            agent.store.job(cut_off).unwrap().unwrap().state,
            "interrupted",
            "the sweep still reclaims a job that really was cut off"
        );
        assert_eq!(
            agent.store.approval(&card).unwrap().unwrap().state,
            "pending",
            "and the card is still waiting, which is what it is all for"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Capacity: a parked job consumes no permit, no worker and no memory, so counting them
    /// would let a few unanswered cards stop the box starting any work at all, which is the
    /// original stall by another route.
    #[tokio::test]
    async fn parked_jobs_leave_the_capacity_cap_alone() {
        let (agent, root) = test_agent().await;
        agent.store.upsert_chat(CHAT, "Main", true, 0).unwrap();
        let host = agent.sync.open_room(CHAT).await.unwrap();
        let cap_agent = crate::config::job_max_per_agent();
        for n in 0..cap_agent + 2 {
            a_parked_job(&agent, n).await;
        }
        assert!(
            agent.store.open_jobs().unwrap().len() > cap_agent,
            "the premise: more parked jobs than the capacity cap allows"
        );

        let mut ticket = None;
        let started = agent
            .start_job(
                CHAT,
                &host,
                "01TESTPARKEDASK00000000000",
                "New work",
                "Do something else.",
                "code",
                Vec::new(),
                true,
                &mut ticket,
            )
            .await;
        assert!(
            started.is_ok(),
            "parked jobs must not wedge the box: {started:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The other half of what used to be one test, and it changed shape with bug AL.
    ///
    /// #2890 refused this call and named the cards in the refusal, because piling more work into
    /// the chat where the cards are is the wrong move. #2924 deletes the refusal — a refusal is
    /// exactly what drove round 22's capped work into unmanaged inline `bash` — and parked jobs
    /// leave the arithmetic entirely, so the work *starts*.
    ///
    /// What survives is the saying rather than the counting. The receipt the user reads names
    /// the cards nobody has answered, with their count, because the person who can clear them is
    /// reading this chat; and it does not describe this job as waiting on them, because it is
    /// not. An ordinary receipt in a chat with no cards says nothing about approvals at all.
    #[tokio::test]
    async fn the_start_receipt_names_the_cards_nobody_has_answered() {
        let (agent, root) = test_agent().await;
        agent.store.upsert_chat(CHAT, "Main", true, 0).unwrap();
        let host = agent.sync.open_room(CHAT).await.unwrap();

        // A chat with no cards first, while nothing has started: an ordinary receipt must not
        // grow an approvals line, or every receipt on the box pays for this one.
        const QUIET: &str = "01TESTCHATPARKEDQUIET00000";
        agent.store.upsert_chat(QUIET, "Quiet", true, 0).unwrap();
        let quiet_host = agent.sync.open_room(QUIET).await.unwrap();
        let mut quiet_ticket = None;
        let plain = agent
            .start_job(
                QUIET,
                &quiet_host,
                "01TESTQUIETASK000000000000",
                "Plain work",
                "Look something up.",
                "research",
                Vec::new(),
                false,
                &mut quiet_ticket,
            )
            .await
            .expect("a chat with no cards starts work the usual way");
        assert!(
            plain.get("cards_waiting").is_none(),
            "a chat with no cards must not report any: {plain}"
        );
        assert!(
            !plain["note"]
                .as_str()
                .unwrap_or_default()
                .contains("approval"),
            "nor say anything about approvals: {plain}"
        );

        let cards = 3;
        for n in 0..cards {
            a_parked_job(&agent, n).await;
        }
        let mut ticket = None;
        let started = agent
            .start_job(
                CHAT,
                &host,
                "01TESTPARKEDASK00000000000",
                "New work",
                "Look something else up.",
                "research",
                Vec::new(),
                // Not forced: this is the path that used to refuse.
                false,
                &mut ticket,
            )
            .await
            .expect("nothing refuses now — parked jobs are not capacity");
        assert_eq!(
            started["state"], "running",
            "the work starts rather than waiting on an answer nobody has given: {started}"
        );
        assert_eq!(
            started["cards_waiting"],
            json!(cards),
            "the receipt reports the cards it started alongside, and how many: {started}"
        );
        let note = started["note"].as_str().unwrap_or_default();
        assert!(
            note.contains(&format!("{cards} approval cards")),
            "the receipt must name the cards and their count: {note}"
        );
        assert!(
            !note.contains("queued"),
            "this job is not queued and the receipt must not say it is: {note}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The launch order, which is the repair with no visible symptom.
    ///
    /// A parked job reads as `queued`, so `await_job_capacity` counted it as being *ahead* of a
    /// job created later — holding a place in the launch order on the strength of an early
    /// `created_at` while holding no worker and no permit. Nothing in any row says this is
    /// happening: the later job simply never starts, until a person answers a card.
    ///
    /// Measured rather than inspected. With the repair the wait returns on its first pass with
    /// no sleep; without it, it sleeps a second and loops forever, so a sub-second deadline
    /// separates the two without waiting on either.
    #[tokio::test]
    async fn a_parked_job_does_not_delay_a_job_queued_after_it() {
        let (agent, root) = test_agent().await;
        agent.store.upsert_chat(CHAT, "Main", true, 0).unwrap();
        // Enough to fill both caps, every one of them parked and created before the job below.
        for n in 0..crate::config::job_max_per_agent() + 2 {
            a_parked_job(&agent, n).await;
        }
        // The id sorts after every parked id, so this job loses the `(created_at, id)` tiebreak
        // as well as the clock — the parked rows are unambiguously "ahead" of it.
        const LATER: &str = "01TESTZLATEJOB000000000000";
        agent
            .store
            .insert_job(
                LATER,
                CHAT,
                "01TESTPARKEDASK00000000000",
                "Later work",
                "research",
                "Look something up.",
                &[],
            )
            .unwrap();
        agent.store.set_job_state(LATER, "queued", None).unwrap();

        let waited = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            agent.await_job_capacity(CHAT, LATER),
        )
        .await;
        assert!(
            waited.is_ok(),
            "a parked job holds no worker and no permit, so it must hold no place in the launch \
             order either — this job was still waiting for capacity after 300ms"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The queued path survives for genuine capacity waits, and this is the sentence that was
    /// false before.
    ///
    /// Real running work fills the cap, so "it starts when one of the jobs already running
    /// finishes" is true — and cards are waiting in the same chat. Both are named, and neither
    /// is reported as the other: the wait is on a running job, the cards are on the user. Saying
    /// the job waits on running work when it waits on an answer is round 22's lie; saying
    /// nothing about the cards is #2890's stall. Nothing may collapse the two.
    #[tokio::test]
    async fn a_genuine_capacity_wait_names_the_running_work_and_the_cards_apart() {
        let (agent, root) = test_agent().await;
        agent.store.upsert_chat(CHAT, "Main", true, 0).unwrap();
        let host = agent.sync.open_room(CHAT).await.unwrap();
        // Real work, holding real workers, enough of it to fill the per-chat cap on its own.
        for n in 0..crate::config::job_max_per_chat() {
            let job = format!("01TESTRUNNINGJOB0000000{n:03}");
            agent
                .store
                .insert_job(
                    &job,
                    CHAT,
                    "01TESTPARKEDASK00000000000",
                    "Running work",
                    "research",
                    "Already going.",
                    &[],
                )
                .unwrap();
            agent.store.set_job_state(&job, "running", None).unwrap();
        }
        // And one card nobody has answered, on a job that is holding nothing.
        a_parked_job(&agent, 0).await;

        let mut ticket = None;
        let queued = agent
            .start_job(
                CHAT,
                &host,
                "01TESTPARKEDASK00000000000",
                "New work",
                "Look something else up.",
                "research",
                Vec::new(),
                false,
                &mut ticket,
            )
            .await
            .expect("over the cap it queues rather than refusing");
        assert_eq!(
            queued["state"], "queued",
            "real running work fills the cap, so this one genuinely waits: {queued}"
        );
        let note = queued["note"].as_str().unwrap_or_default();
        assert!(
            note.contains("when a running job finishes"),
            "the wait is on a running job, which is now true rather than a guess: {note}"
        );
        assert!(
            note.contains("approval card"),
            "and the card waiting alongside it is named rather than left out: {note}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The premise the other four rest on.** Every exclusion above reads a parked job as
    /// `queued`, and that is true today only because `run_subagent` writes it at the one place
    /// that parks. Nothing in the tree enforces it.
    ///
    /// So this drives the real park path rather than `a_parked_job`, whose whole job is to
    /// hand-write the state the other cases need. A test built on that helper would assert the
    /// helper's own `set_job_state(&job, "queued", None)` and would pass happily against a tree
    /// where the park path had stopped writing it — which is the one failure this exists to
    /// catch. A second park path that left a `running` row alone would put a job holding nothing
    /// back into the caps and the launch order, and every other case here would stay green.
    #[tokio::test]
    async fn a_parked_job_is_queued_not_running() {
        use std::os::unix::fs::PermissionsExt;
        let script_root = std::env::temp_dir().join(format!("arbos-park-path-{}", ulid()));
        std::fs::create_dir_all(&script_root).unwrap();
        let pi_bin = script_root.join("settling-pi.py");
        std::fs::write(&pi_bin, SETTLING_PI).unwrap();
        std::fs::set_permissions(&pi_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (agent, root) = test_agent_with_pi(&pi_bin.display().to_string()).await;
        agent.store.upsert_chat(CHAT, "Main", true, 0).unwrap();
        let host = agent.sync.open_room(CHAT).await.unwrap();
        const JOB: &str = "01TESTPARKPATHJOB000000000";
        const WORKER: &str = "01TESTPARKPATHWORKER000000";
        const CARD: &str = "01TESTPARKPATHCARD00000000";
        agent
            .store
            .insert_subagent(WORKER, "park-path", "work", "pi", CHAT)
            .unwrap();
        agent
            .store
            .insert_job(
                JOB,
                CHAT,
                "01TESTPARKEDASK00000000000",
                "Clearing the old exports",
                "code",
                "Delete last month's export files.",
                &[],
            )
            .unwrap();
        agent.store.set_job_subagent(JOB, WORKER).unwrap();
        agent.store.set_job_state(JOB, "running", None).unwrap();
        // The card this worker's session raised, as agentd writes it when the worker asks. What
        // is under test is what the settle does with it, which is not something set up here.
        agent
            .store
            .insert_approval(
                CARD,
                CHAT,
                "01TESTPARKPATHMESSAGE00000",
                "Run a destructive command (file-delete)",
                "destructive",
                &json!({ "kind": "tool_call", "tool": "bash",
                    "input": { "command": "rm -rf /opt/x" },
                    "session_id": WORKER, "job_id": JOB }),
            )
            .unwrap();

        // However this worker's run ends, the settle reads the card rather than the model's
        // return contract, so a run that produced nothing still parks.
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            agent.run_subagent(
                CHAT,
                &host,
                "01TESTPARKEDASK00000000000",
                Some(WORKER.to_owned()),
                "park-path",
                "clear the old exports",
                "Delete last month's export files.",
                Some(arbos_core::route::Harness::Pi),
                false,
                false,
                Some(crate::agent::JobSpec {
                    id: JOB.to_owned(),
                    title: "Clearing the old exports".to_owned(),
                    kind: "code".to_owned(),
                    brief: "Delete last month's export files.".to_owned(),
                    touches: vec![],
                    attempt: 0,
                }),
            ),
        )
        .await;

        let row = agent.store.job(JOB).unwrap().unwrap();
        assert_eq!(
            row.state, "queued",
            "the park path must write `queued`: every capacity and ordering exclusion reads a \
             parked job by that state, and a parked `running` row would be counted as live work"
        );
        // `queued` on its own would not prove this came from the park path: the retry branch a
        // few lines below writes the same state for a worker that produced nothing, which is
        // exactly what this worker did. These three separate them — a requeue carries the
        // worker's failure reason and spends a restart, and only a park writes this step.
        assert_eq!(
            row.last_step, "Waiting for approval",
            "and must say what it is waiting for, which is the card rather than a free slot"
        );
        assert_eq!(
            row.error, None,
            "a park is not a failure, so it carries no reason: {:?}",
            row.error
        );
        assert_eq!(
            row.restarts, 0,
            "nor does it spend a crash-budget attempt the way a requeue does"
        );
        let _ = std::fs::remove_dir_all(&script_root);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A resumed worker keeps the attempt count its job has already spent.
    ///
    /// The crash budget (`JOB_WORKER_RETRIES`) exists to stop a job burning the box on a crash
    /// loop — bug AJ, a pi child dying five times under a job burst. Answering a card says a
    /// person approved an action; it says nothing about whether the thing that was crashing has
    /// stopped. Reset the count here and a worker can crash through the budget, raise a card, be
    /// answered, and start the budget over, with the loop detection reset each time: bounded by
    /// the user's patience rather than by the box.
    ///
    /// Note the premise this case makes explicit — a parked worker's attempt is **not** always
    /// zero, so the reset could not be defended as a no-op. A job whose first worker crashed is
    /// given a retry worker, and that worker can reach a gated call and park exactly like the
    /// first one.
    ///
    /// Pinned on the one place the resume builds a spec rather than through a real child, which
    /// would need a worker that crashes on demand; the second assertion is what keeps the call
    /// site honest, since a literal there is how this drifts back to zero.
    #[tokio::test]
    async fn a_resumed_worker_keeps_the_attempt_count_its_job_has_spent() {
        let (agent, root) = test_agent().await;
        agent.store.upsert_chat(CHAT, "Main", true, 0).unwrap();
        let _host = agent.sync.open_room(CHAT).await.unwrap();
        let (job, _worker, _card) = a_parked_job(&agent, 7).await;

        let fresh = agent.store.job(&job).unwrap().unwrap();
        assert_eq!(fresh.restarts, 0);
        assert_eq!(
            crate::agent::resumed_job_spec(&fresh).attempt,
            0,
            "a job that has spent nothing resumes with nothing spent"
        );

        // One worker of this job has already crashed and been replaced, which is what
        // `bump_job_restarts` records. The resumed worker is the second, not a first.
        agent.store.bump_job_restarts(&job).unwrap();
        let spent = agent.store.job(&job).unwrap().unwrap();
        assert_eq!(
            crate::agent::resumed_job_spec(&spent).attempt,
            crate::agent::JOB_WORKER_RETRIES,
            "the budget is already spent, so the resume must not hand out another one"
        );
        // And the count is read from the row, so it survives the agentd restart a card now
        // outlives — the in-memory `JobSpec` does not.
        for _ in 0..4 {
            agent.store.bump_job_restarts(&job).unwrap();
        }
        assert_eq!(
            crate::agent::resumed_job_spec(&agent.store.job(&job).unwrap().unwrap()).attempt,
            5
        );

        assert!(
            !include_str!("approval.rs")
                .split("fn resume_worker_with")
                .nth(1)
                .expect("the resume")
                .split("\n    pub async fn")
                .next()
                .expect("its body")
                .contains("attempt:"),
            "the resume must build its spec through `resumed_job_spec`: an `attempt:` literal \
             here is how the crash budget silently refreshes again"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The answer burst: the user comes back and answers every card at once.
    ///
    /// This is the moment all three of the old bounds are absent together — the ask cap was
    /// decremented by each park, the retained-worker bound keeps anything mid-run, and
    /// `run_approval_turn` acquires no permits at all. So the resume is routed through
    /// `run_subagent`, which acquires `subagent_slots` and `box_slots` before it runs anything.
    ///
    /// Asserted on the permits rather than on wall-clock ordering: with every slot held, each
    /// resumed worker gets as far as writing `queued` and waits there. Nothing fans out.
    #[tokio::test]
    async fn answering_a_burst_of_cards_queues_on_the_worker_permits() {
        let (agent, root) = test_agent().await;
        agent.store.upsert_chat(CHAT, "Main", true, 0).unwrap();
        let _host = agent.sync.open_room(CHAT).await.unwrap();
        let mut cards = Vec::new();
        for n in 0..3 {
            let (_job, worker, card) = a_parked_job(&agent, n).await;
            cards.push((worker, card));
        }
        // Every worker slot is taken, as a busy box's would be.
        let held = agent
            .subagent_slots
            .clone()
            .acquire_many_owned(agent.subagent_slots.available_permits() as u32)
            .await
            .unwrap();
        assert_eq!(agent.subagent_slots.available_permits(), 0);

        for (_worker, card) in &cards {
            agent
                .decide_approval(card, ApprovalOutcome::Approved, "dev-mac")
                .await
                .unwrap();
        }

        // Each resume reaches the slot wait and stops there, which is what `queued` records.
        for _ in 0..200 {
            let queued = cards
                .iter()
                .filter(|(w, _)| {
                    agent
                        .store
                        .subagent(w)
                        .unwrap()
                        .is_some_and(|s| s.status == "queued")
                })
                .count();
            if queued == cards.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        for (worker, _) in &cards {
            let row = agent.store.subagent(worker).unwrap().unwrap();
            assert_eq!(
                row.status, "queued",
                "a resumed worker waits for a slot like any other work"
            );
        }
        assert_eq!(
            agent.subagent_slots.available_permits(),
            0,
            "and none of them invented a slot of its own"
        );
        drop(held);
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

    /// Every wait is a short grace, and the card it leaves behind has **no deadline at all**.
    ///
    /// Widened from the main session to both roles, which is the change: a worker used to be
    /// given the extension's fourteen-minute cap as both its wait and its card's lifetime. And
    /// "keeps the full deadline" became "keeps no deadline", because a card that closes itself
    /// is the stall.
    #[tokio::test]
    async fn every_wait_is_a_short_grace_and_no_card_gets_a_deadline() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATAPPROVALPARK0000";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let _host = agent.sync.open_room(chat).await.unwrap();

        for (role, session) in [("main", chat), ("subagent", "01TESTWORKERPARKSESSION00")] {
            let v = agent.hook_approval("bash", &rm(), session, role).await;
            assert_eq!(v["state"], "pending", "{v}");
            assert_eq!(v["park"], json!(true), "{role} parks: {v}");
            let wait = v["waitMs"].as_i64().unwrap();
            assert!(
                wait < 30_000,
                "{role} must let go before a follow-up message arrives: {wait} ms"
            );
            assert_eq!(wait, park_after_ms(), "one number bounds every wait: {v}");
            let row = agent
                .store
                .approval(v["approvalId"].as_str().unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(
                row.expires_at, 0,
                "a card has no deadline, and 0 says so rather than naming a time"
            );
            agent
                .decide_approval(
                    v["approvalId"].as_str().unwrap(),
                    ApprovalOutcome::Cancelled,
                    "test",
                )
                .await
                .unwrap();
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An answer that arrives **inside** the grace reaches the waiter that is still holding on,
    /// and the call runs on that outcome rather than being parked and re-run.
    ///
    /// Kept as a test of its own because the park took this assertion away from somewhere else.
    /// `the_valve_releases_an_answer_held_by_an_inline_approval_wait` used to end by deciding the
    /// card and watching the held call receive `Approved`; it now ends on the park, since its
    /// clock is advanced well past the grace before anything is decided. Without this case
    /// nothing would assert that a decision ever reaches a live waiter at all — every test would
    /// be watching the timeout path, and a `begin_approval` that dropped the sender would pass
    /// them all.
    #[tokio::test]
    async fn an_answer_inside_the_grace_reaches_the_waiter_still_holding_on() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATAPPROVALPARK0004";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let _host = agent.sync.open_room(chat).await.unwrap();

        let waiter = {
            let agent = agent.clone();
            tokio::spawn(async move {
                agent
                    .request_approval(
                        chat,
                        ApprovalRequest {
                            title: "Run a destructive command (file-delete)".into(),
                            summary: "rm -rf /opt/x".into(),
                            risk: Risk::Destructive,
                            payload: json!({ "kind": "tool_call", "tool": "bash" }),
                        },
                    )
                    .await
            })
        };
        while !agent.approval_pending_for(chat).await {
            tokio::task::yield_now().await;
        }
        let id = agent.store.pending_approvals().unwrap()[0].id.clone();
        agent
            .decide_approval(&id, ApprovalOutcome::Approved, "dev-mac")
            .await
            .unwrap();

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), waiter)
                .await
                .expect("the waiter was never woken")
                .expect("the waiter task"),
            Some(ApprovalOutcome::Approved),
            "the decision must reach the caller that is still waiting, not only the row"
        );
        // Answered inside the grace, so there is nothing to re-run later: the waiter ran the
        // call itself, and the row's single-use grant is spent rather than left lying around
        // for the next identical call to claim without a card. This caught a real hole — asked
        // once, done twice — which is the whole reason the case is worth having.
        assert!(
            agent.store.unconsumed_grants().unwrap().is_empty(),
            "the execution the user approved has happened; nothing may run a second time on it"
        );
        assert!(agent
            .store
            .approval(&id)
            .unwrap()
            .unwrap()
            .grant_consumed_at
            .is_some());
        assert_eq!(
            agent
                .hook_approval("bash", &json!({ "command": "rm -rf /opt/x" }), chat, "main")
                .await["state"],
            "pending",
            "the same call again is a new question, not a free pass"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The deadline is gone from what the user is shown, too: no `expiresAt` on the card part.
    ///
    /// Absent rather than zero on purpose. Both clients model it as an `Option` that nothing
    /// renders, so absent reads as "no deadline"; a `0` would be a 1970 date for any later
    /// reader that decided to draw one.
    #[tokio::test]
    async fn the_card_in_the_chat_advertises_no_deadline() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATAPPROVALPARK0003";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let host = agent.sync.open_room(chat).await.unwrap();

        let v = agent.hook_approval("bash", &rm(), chat, "main").await;
        let id = v["approvalId"].as_str().unwrap().to_owned();
        let h = host.lock().await;
        let part = h
            .doc
            .messages()
            .iter()
            .flat_map(|m| m.parts.clone())
            .find(|p| p.get("approvalId").and_then(Value::as_str) == Some(id.as_str()))
            .expect("the card part");
        assert_eq!(part.get("state").and_then(Value::as_str), Some("pending"));
        assert!(
            part.get("expiresAt").is_none(),
            "no deadline is advertised to the user: {part}"
        );
        drop(h);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The owner's third test: approving a parked card runs the command and the result comes
    /// back in the same chat. Parking moves the entry to `Deferred`, which is what hands the
    /// decision to `follow_up`, and the grant that lets the re-run through is the approved row
    /// itself. No second card is raised.
    ///
    /// The grant assertion is now made on the row rather than on an in-memory map, which is
    /// strictly stronger: it is the same state a restarted agentd would read.
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
        assert!(
            matches!(
                agent.approvals.pending.lock().await[&id].delivery,
                WaitDelivery::Deferred
            ),
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

        // The decision itself is the grant, so it is readable the moment the row is approved —
        // no task has to have run first, and nothing expires while the re-run waits its turn.
        let row = agent.store.approval(&id).unwrap().unwrap();
        assert_eq!(row.state, "approved");
        assert_eq!(
            row.grant_consumed_at, None,
            "unclaimed until the re-run takes it"
        );
        assert_eq!(
            agent
                .store
                .unconsumed_grants()
                .unwrap()
                .into_iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            vec![id.clone()]
        );
        assert_eq!(
            agent.hook_approval("bash", &rm(), chat, "main").await["state"],
            "allowed",
            "the re-run must not raise a second card"
        );
        assert!(
            agent
                .store
                .approval(&id)
                .unwrap()
                .unwrap()
                .grant_consumed_at
                .is_some(),
            "and the grant is spent by that one re-run"
        );
        // Single use: the same call a second time is carded again, and the new card says it is a
        // second attempt at something already approved rather than a fresh request.
        let again = agent.hook_approval("bash", &rm(), chat, "main").await;
        assert_eq!(again["state"], "pending", "{again}");
        let second = agent
            .store
            .approval(again["approvalId"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(second.payload["prior_approval_id"], json!(id));
        assert!(
            second.payload["prior_consumed_at"].as_i64().is_some(),
            "when the first attempt was authorized: {}",
            second.payload
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The grant outlives a restart, which the ten-minute in-memory one could not.
    ///
    /// Jacob's requirement is that answering a card finishes the work; a grant that lives in one
    /// process means answering a card and then restarting asks the user to answer it again. The
    /// store is reopened on the same file here, which is as close to a restart as a unit test
    /// gets, and the re-run is still admitted — once.
    #[tokio::test]
    async fn an_approved_calls_grant_survives_a_restart_and_is_still_single_use() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATAPPROVALPARK0004";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let _host = agent.sync.open_room(chat).await.unwrap();

        let v = agent.hook_approval("bash", &rm(), chat, "main").await;
        let id = v["approvalId"].as_str().unwrap().to_owned();
        assert_eq!(agent.park_approval(&id).await["parked"], json!(true));
        agent
            .decide_approval(&id, ApprovalOutcome::Approved, "dev-mac")
            .await
            .unwrap();

        let reopened =
            crate::store::Store::open(&agent.cfg.state_dir.join("agentd").join("agentd.db"))
                .unwrap();
        let grants = reopened.unconsumed_grants().unwrap();
        assert_eq!(
            grants.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
            vec![id.clone()],
            "a fresh process reads the grant from the row"
        );
        assert!(reopened.consume_grant(&id).unwrap(), "the first claim wins");
        assert!(
            !reopened.consume_grant(&id).unwrap(),
            "and the second finds it spent, with no lock held in between"
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
mod card_location_tests {
    use super::*;
    use crate::status::tests::test_agent;

    const MAIN: &str = "01TESTCHATCARDWHERE0MAIN00";
    const SIDE: &str = "01TESTCHATCARDWHERE0SIDE00";

    /// The model is never told where its card went, so it says "the card is in this chat" —
    /// true only when the asking chat and the card's chat happen to coincide. `approval_chat`
    /// sends a card to the chat the owner reads, so for a side-chat call they usually do not.
    ///
    /// The reply to the gated call is the right channel because it reaches exactly the session
    /// that tripped the gate, which is the session that writes the false line. The name is what
    /// it can say to the user; the id is beside it so it is exact. No prompt text changes.
    #[tokio::test]
    async fn the_session_that_asked_is_told_which_chat_the_card_is_in() {
        let (agent, root) = test_agent().await;
        agent.store.upsert_chat(MAIN, "Main", true, 0).unwrap();
        agent
            .store
            .upsert_chat(SIDE, "Trip planning", false, 1)
            .unwrap();
        let _main = agent.sync.open_room(MAIN).await.unwrap();
        let _side = agent.sync.open_room(SIDE).await.unwrap();

        let v = agent
            .hook_approval("bash", &json!({ "command": "rm -rf /opt/x" }), SIDE, "main")
            .await;
        assert_eq!(v["state"], "pending", "{v}");

        // The geometry first, so this fixture cannot quietly collapse into one chat and then
        // pass whatever the reply says.
        let row = agent
            .store
            .approval(v["approvalId"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            row.chat_id, MAIN,
            "the card goes to the chat the owner reads"
        );
        assert_ne!(row.chat_id, SIDE, "not to the chat the call came from");

        assert_eq!(v["chat"], "Main", "a name the model can say out loud: {v}");
        assert_eq!(v["chatId"], MAIN, "and the id, so it is exact: {v}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A chat with no title would otherwise put a raw ULID where a place name belongs, and the
    /// model reads whatever it is given out to the user.
    #[tokio::test]
    async fn an_untitled_chat_is_named_rather_than_spelled_out() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATCARDWHERE0BLANK0";
        agent.store.upsert_chat(chat, "", true, 0).unwrap();
        let _host = agent.sync.open_room(chat).await.unwrap();

        let v = agent
            .hook_approval("bash", &json!({ "command": "rm -rf /opt/x" }), chat, "main")
            .await;
        assert_eq!(v["chat"], "your main chat", "{v}");
        assert!(
            !v["chat"].as_str().unwrap().contains(chat),
            "a ULID is not a place a person can be sent to: {v}"
        );
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
    /// **Inverted.** Round 5's drift was between three numbers; there is now one, and it bounds
    /// a wait rather than a card, so the drift is unrepresentable. The card advertises no
    /// deadline at all and the reply hands the waiter agentd's own grace.
    #[tokio::test]
    async fn a_gated_card_advertises_no_deadline_and_one_number_bounds_the_wait() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATAPPROVALTTL00000";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let _host = agent.sync.open_room(chat).await.unwrap();

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
            json!(true),
            "a worker's call parks too, which is what frees its slot: {v}"
        );
        let row = agent
            .store
            .approval(v["approvalId"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            row.expires_at, 0,
            "round 5 could only happen because a card had a deadline; it has none"
        );
        assert_eq!(
            v["waitMs"].as_i64(),
            Some(park_after_ms()),
            "the waiter reads agentd's number, so no second copy can drift: {v}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **Inverted**, and this is the assertion Jacob's ruling turns on: an undecided card is
    /// still `pending` and still answerable long past every deadline that used to close it.
    ///
    /// The three old numbers are all tried — the 24 h TTL, the extension's 14-minute cap, and a
    /// run of the minute sweep — because each was a separate way for the card to close itself,
    /// and the sweep is checked by the only means left: it no longer exists, so a card is read
    /// again after the time one would have fired.
    #[tokio::test]
    async fn an_undecided_card_stays_answerable_past_every_old_deadline() {
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
            .begin_approval(chat, req, WaitDelivery::Polled)
            .await
            .unwrap();
        // Time is moved past all three rather than waited out: `tokio`'s clock is paused in
        // test, so this is the 24-hour case as well as the 14-minute one.
        tokio::time::pause();
        tokio::time::advance(Duration::from_millis(
            (arbos_core::approval::APPROVAL_TTL_MS + 14 * 60 * 1000) as u64 + 60_000,
        ))
        .await;
        tokio::time::resume();
        tokio::task::yield_now().await;

        let after = agent.store.approval(&id).unwrap().unwrap();
        assert_eq!(
            after.state, "pending",
            "a day and a half later the question is still in front of the user"
        );
        assert_eq!(after.decided_by, None, "and nothing has answered it");
        assert_eq!(agent.approval_status(&id).await["state"], "pending");
        // Still answerable, which is the half that makes the waiting worth anything.
        agent
            .decide_approval(&id, ApprovalOutcome::Approved, "dev-mac")
            .await
            .unwrap();
        assert_eq!(
            agent.store.approval(&id).unwrap().unwrap().state,
            "approved"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A cancel closes an open card as `cancelled`: the work was called off, so the question
    /// goes with it rather than standing in the chat for ever now that nothing expires.
    ///
    /// `cancelled` and not `rejected`, because the user refused nothing — and not `expired`,
    /// which is the state this change stopped producing.
    ///
    /// The card in the chat is asserted as well as the row, because they are written through
    /// different paths: `doc_state` sits between them so that substituting a state older clients
    /// parse is one line. Both halves must be read for that line to have anything holding it.
    #[tokio::test]
    async fn cancelling_the_work_closes_its_card_as_cancelled() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATAPPROVALTTL00002";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let host = agent.sync.open_room(chat).await.unwrap();

        let req = ApprovalRequest {
            title: "Run a destructive command (file.delete)".into(),
            summary: "rm -rf /opt/x".into(),
            risk: Risk::Destructive,
            payload: json!({ "kind": "tool_call", "tool": "bash", "job_id": "01TESTJOBCANCELCARD000000" }),
        };
        let id = agent
            .begin_approval(chat, req, WaitDelivery::Polled)
            .await
            .unwrap();
        agent
            .cancel_approvals_for_job("01TESTJOBCANCELCARD000000")
            .await;
        let row = agent.store.approval(&id).unwrap().unwrap();
        assert_eq!(row.state, "cancelled", "not expired, and not rejected");
        assert_eq!(row.decided_by.as_deref(), Some("arbos:cancelled"));
        assert!(agent.store.pending_approvals().unwrap().is_empty());
        let card = host.lock().await.doc.approval(&id).expect("the card");
        assert_eq!(
            card["state"].as_str(),
            Some("cancelled"),
            "the card in the chat carries the row's own state; `doc_state` is where that changes"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod delivery_claim_regressions {
    use super::*;
    use crate::status::tests::test_agent;
    use std::sync::atomic::Ordering;

    const CHAT: &str = "01TESTCLAIMDELIVERYCHAT000";
    fn request() -> ApprovalRequest {
        ApprovalRequest {
            title: "Synthetic single-use admission".into(),
            summary: "No command is executed".into(),
            risk: Risk::Destructive,
            payload: json!({"kind":"tool_call", "tool":"bash", "input":{"command":"rm -rf /synthetic-owned-fixture"}}),
        }
    }
    async fn fixture() -> (Arc<Agent>, std::path::PathBuf, crate::sync::SharedHost) {
        let (agent, root) = test_agent().await;
        agent.store.upsert_chat(CHAT, "Fixture", true, 0).unwrap();
        let host = agent.sync.open_room(CHAT).await.unwrap();
        (agent, root, host)
    }
    async fn inline_fault(action: &str) {
        let (agent, root, _host) = fixture().await;
        let (tx, mut rx) = oneshot::channel();
        let id = agent
            .begin_approval(CHAT, request(), WaitDelivery::Inline(tx))
            .await
            .unwrap();
        agent.store.with(|c| c.execute_batch(&format!(
            "CREATE TRIGGER refuse_claim BEFORE UPDATE OF grant_consumed_at ON approvals BEGIN SELECT RAISE({action}); END;"
        ))).unwrap();
        let result = agent
            .decide_approval(&id, ApprovalOutcome::Approved, "synthetic-owner")
            .await;
        let delivered = rx.try_recv();
        assert!(
            !matches!(delivered, Ok(ApprovalOutcome::Approved)),
            "a failed durable claim must never admit the inline tool"
        );
        assert!(result.is_err(), "claim failure must be reported");
        assert_eq!(
            agent.store.approval(&id).unwrap().unwrap().state,
            "pending",
            "decision must roll back with its failed claim"
        );
        agent
            .store
            .with(|c| c.execute_batch("DROP TRIGGER refuse_claim"))
            .unwrap();
        agent
            .decide_approval(&id, ApprovalOutcome::Approved, "synthetic-owner")
            .await
            .unwrap();
        assert!(
            matches!(rx.try_recv(), Ok(ApprovalOutcome::Approved)),
            "a local SQL failure leaves the original caller retryable"
        );
        assert!(agent
            .store
            .approval(&id)
            .unwrap()
            .unwrap()
            .grant_consumed_at
            .is_some());
        let _ = std::fs::remove_dir_all(root);
    }
    #[tokio::test]
    async fn an_aborted_claim_never_delivers_inline_approval() {
        inline_fault("ABORT, 'synthetic claim failure'").await;
    }
    #[tokio::test]
    async fn an_ignored_claim_never_delivers_inline_approval() {
        inline_fault("IGNORE").await;
    }

    #[tokio::test]
    async fn a_polled_approved_decision_is_already_claimed_before_doc_effects() {
        let (agent, root, host) = fixture().await;
        let id = agent
            .begin_approval(CHAT, request(), WaitDelivery::Polled)
            .await
            .unwrap();
        let paused_doc = host.lock().await;
        let task = {
            let agent = agent.clone();
            let id = id.clone();
            tokio::spawn(async move {
                agent
                    .decide_approval(&id, ApprovalOutcome::Approved, "synthetic-owner")
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if agent.approval_status(&id).await["state"] == "approved" {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("actual SQL decision did not complete");
        assert!(
            agent
                .store
                .approval(&id)
                .unwrap()
                .unwrap()
                .grant_consumed_at
                .is_some(),
            "the real polling response admitted a tool before claiming its single-use grant"
        );
        drop(paused_doc);
        task.await.unwrap().unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
    #[tokio::test]
    async fn inline_and_durable_grant_cannot_both_admit_the_same_card() {
        let (agent, root, host) = fixture().await;
        let (tx, rx) = oneshot::channel();
        let id = agent
            .begin_approval(CHAT, request(), WaitDelivery::Inline(tx))
            .await
            .unwrap();
        let paused_doc = host.lock().await;
        let task = {
            let agent = agent.clone();
            let id = id.clone();
            tokio::spawn(async move {
                agent
                    .decide_approval(&id, ApprovalOutcome::Approved, "synthetic-owner")
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if agent.store.approval(&id).unwrap().unwrap().state == "approved" {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("actual SQL decision did not complete");
        let other_admitted = agent.store.consume_grant(&id).unwrap();
        drop(paused_doc);
        let _ = task.await.unwrap();
        let inline_admitted = matches!(rx.await, Ok(ApprovalOutcome::Approved));
        assert!(
            !(other_admitted && inline_admitted),
            "one approval admitted both an inline call and another durable grant claimant"
        );
        let _ = std::fs::remove_dir_all(root);
    }
    #[tokio::test]
    async fn abandoning_inline_after_published_approval_never_restores_authority() {
        let (agent, root, host) = fixture().await;
        let (tx, rx) = oneshot::channel();
        let id = agent
            .begin_approval(CHAT, request(), WaitDelivery::Inline(tx))
            .await
            .unwrap();
        let paused_doc = host.lock().await;
        let task = {
            let agent = agent.clone();
            let id = id.clone();
            tokio::spawn(async move {
                agent
                    .decide_approval(&id, ApprovalOutcome::Approved, "synthetic-owner")
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if agent.approval_status(&id).await["state"] == "approved" {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("actual SQL decision did not complete");
        drop(rx);
        drop(paused_doc);
        let _ = task.await.unwrap();
        assert!(agent.store.approval(&id).unwrap().unwrap().grant_consumed_at.is_some(), "published approval may already have admitted an observer; never restore it after receiver loss");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_receiver_closed_while_pending_defers_without_spending_the_grant() {
        let (agent, root, _host) = fixture().await;
        let (tx, rx) = oneshot::channel();
        let id = agent
            .begin_approval(CHAT, request(), WaitDelivery::Inline(tx))
            .await
            .unwrap();
        drop(rx);
        agent
            .decide_approval(&id, ApprovalOutcome::Approved, "synthetic-owner")
            .await
            .unwrap();
        assert_eq!(
            agent
                .store
                .approval(&id)
                .unwrap()
                .unwrap()
                .grant_consumed_at,
            None
        );
        assert!(
            agent
                .take_grant(
                    "bash",
                    &json!({"command":"rm -rf /synthetic-owned-fixture"})
                )
                .await
                .is_some(),
            "existing canonical deferred claim is retained"
        );
        assert!(agent
            .take_grant(
                "bash",
                &json!({"command":"rm -rf /synthetic-owned-fixture"})
            )
            .await
            .is_none());
        let _ = std::fs::remove_dir_all(root);
    }
    #[tokio::test]
    async fn a_polled_claim_failure_keeps_pending_until_a_successful_retry() {
        for action in ["ABORT, 'synthetic claim failure'", "IGNORE"] {
            let (agent, root, _host) = fixture().await;
            let id = agent
                .begin_approval(CHAT, request(), WaitDelivery::Polled)
                .await
                .unwrap();
            agent.store.with(|c| c.execute_batch(&format!("CREATE TRIGGER refuse_claim BEFORE UPDATE OF grant_consumed_at ON approvals BEGIN SELECT RAISE({action}); END;"))).unwrap();
            assert!(agent
                .decide_approval(&id, ApprovalOutcome::Approved, "synthetic-owner")
                .await
                .is_err());
            assert_eq!(
                agent.approval_status(&id).await["state"],
                "pending",
                "the actual polling path cannot publish an unclaimed approval"
            );
            agent
                .store
                .with(|c| c.execute_batch("DROP TRIGGER refuse_claim"))
                .unwrap();
            agent
                .decide_approval(&id, ApprovalOutcome::Approved, "synthetic-owner")
                .await
                .unwrap();
            assert_eq!(agent.approval_status(&id).await["state"], "approved");
            assert!(agent
                .store
                .approval(&id)
                .unwrap()
                .unwrap()
                .grant_consumed_at
                .is_some());
            let _ = std::fs::remove_dir_all(root);
        }
    }
    #[tokio::test]
    async fn rejection_and_cancellation_deliver_without_claiming_authority() {
        for outcome in [
            ApprovalOutcome::Rejected {
                reason: Some("synthetic refusal".into()),
            },
            ApprovalOutcome::Cancelled,
        ] {
            let (agent, root, _host) = fixture().await;
            let (tx, rx) = oneshot::channel();
            let id = agent
                .begin_approval(CHAT, request(), WaitDelivery::Inline(tx))
                .await
                .unwrap();
            agent.store.with(|c| c.execute_batch("CREATE TRIGGER refuse_claim BEFORE UPDATE OF grant_consumed_at ON approvals BEGIN SELECT RAISE(ABORT, 'must not claim'); END;")).unwrap();
            agent
                .decide_approval(&id, outcome.clone(), "synthetic-owner")
                .await
                .unwrap();
            assert_eq!(rx.await.unwrap(), outcome);
            assert_eq!(
                agent
                    .store
                    .approval(&id)
                    .unwrap()
                    .unwrap()
                    .grant_consumed_at,
                None
            );
            assert!(agent
                .take_grant(
                    "bash",
                    &json!({"command":"rm -rf /synthetic-owned-fixture"})
                )
                .await
                .is_none());
            let _ = std::fs::remove_dir_all(root);
        }
    }
    #[tokio::test]
    async fn the_first_decision_and_its_claim_are_terminal() {
        let (agent, root, _host) = fixture().await;
        let (tx, rx) = oneshot::channel();
        let id = agent
            .begin_approval(CHAT, request(), WaitDelivery::Inline(tx))
            .await
            .unwrap();
        agent
            .decide_approval(&id, ApprovalOutcome::Approved, "synthetic-owner")
            .await
            .unwrap();
        assert_eq!(rx.await.unwrap(), ApprovalOutcome::Approved);
        let first = agent.store.approval(&id).unwrap().unwrap();
        for outcome in [
            ApprovalOutcome::Approved,
            ApprovalOutcome::Cancelled,
            ApprovalOutcome::Rejected { reason: None },
        ] {
            assert!(agent
                .decide_approval(&id, outcome, "later-owner")
                .await
                .is_err());
            let row = agent.store.approval(&id).unwrap().unwrap();
            assert_eq!(
                (row.state, row.grant_consumed_at, row.decided_by),
                (
                    first.state.clone(),
                    first.grant_consumed_at,
                    first.decided_by.clone()
                )
            );
        }
        assert_eq!(agent.park_approval(&id).await["parked"], false);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn generated_delivery_traces_keep_deferred_absorbing_and_never_invent_a_waiter() {
        let mut seed = 0x5eed_4110_u64;
        for case in 0..64 {
            let (tx, rx) = oneshot::channel();
            let mut receiver = Some(rx);
            let mut state = match case % 3 {
                0 => WaitDelivery::Inline(tx),
                1 => WaitDelivery::Polled,
                _ => WaitDelivery::Deferred,
            };
            let mut parked = matches!(state, WaitDelivery::Deferred);
            for _ in 0..16 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                if seed & 4 != 0 {
                    receiver.take();
                }
                let event = if seed & 1 == 0 {
                    DeliveryEvent::Park
                } else {
                    DeliveryEvent::Decide
                };
                let action = transition_delivery(&mut state, event);
                if matches!(action, DeliveryAction::Parked) {
                    parked = true;
                }
                if parked {
                    assert!(matches!(state, WaitDelivery::Deferred));
                    assert!(matches!(
                        action,
                        DeliveryAction::Parked
                            | DeliveryAction::Unchanged
                            | DeliveryAction::Deferred
                    ));
                }
                if matches!(action, DeliveryAction::Inline) {
                    assert!(receiver.is_some());
                    assert!(matches!(state, WaitDelivery::Inline(_)));
                }
                if matches!(state, WaitDelivery::Deferred) {
                    parked = true;
                }
            }
        }
    }
    struct CloseAtDecision {
        receiver: std::sync::Mutex<Option<oneshot::Receiver<ApprovalOutcome>>>,
        hits: std::sync::atomic::AtomicUsize,
    }
    struct ReceiverCloser {
        store: crate::store::Store,
        context: *const CloseAtDecision,
    }
    impl ReceiverCloser {
        fn install(
            store: &crate::store::Store,
            receiver: oneshot::Receiver<ApprovalOutcome>,
        ) -> Self {
            unsafe extern "C" fn close_receiver(
                event: u32,
                context: *mut std::ffi::c_void,
                statement: *mut std::ffi::c_void,
                _: *mut std::ffi::c_void,
            ) -> i32 {
                if event != rusqlite::ffi::SQLITE_TRACE_PROFILE as u32 {
                    return 0;
                }
                let sql = unsafe { rusqlite::ffi::sqlite3_sql(statement.cast()) };
                if sql.is_null() {
                    return 0;
                }
                let sql = unsafe { std::ffi::CStr::from_ptr(sql) }.to_bytes();
                if sql.starts_with(b"UPDATE approvals SET state =") {
                    let closer = unsafe { &*context.cast::<CloseAtDecision>() };
                    if let Ok(mut receiver) = closer.receiver.lock() {
                        receiver.take();
                    }
                    closer.hits.fetch_add(1, Ordering::SeqCst);
                }
                0
            }
            let context = Arc::into_raw(Arc::new(CloseAtDecision {
                receiver: std::sync::Mutex::new(Some(receiver)),
                hits: std::sync::atomic::AtomicUsize::new(0),
            }));
            store
                .with(|connection| {
                    // Context lives through registration; unregister under the same Store lock.
                    let code = unsafe {
                        rusqlite::ffi::sqlite3_trace_v2(
                            connection.handle(),
                            rusqlite::ffi::SQLITE_TRACE_PROFILE as u32,
                            Some(close_receiver),
                            context.cast_mut().cast(),
                        )
                    };
                    assert_eq!(code, rusqlite::ffi::SQLITE_OK);
                    Ok(())
                })
                .unwrap();
            Self {
                store: store.clone(),
                context,
            }
        }
        fn hits(&self) -> usize {
            unsafe { &*self.context }.hits.load(Ordering::SeqCst)
        }
    }
    impl Drop for ReceiverCloser {
        fn drop(&mut self) {
            self.store
                .with(|connection| {
                    unsafe {
                        rusqlite::ffi::sqlite3_trace_v2(
                            connection.handle(),
                            0,
                            None,
                            std::ptr::null_mut(),
                        );
                    }
                    Ok(())
                })
                .unwrap();
            unsafe {
                drop(Arc::from_raw(self.context));
            }
        }
    }
    async fn receiver_loss_after_nonapproved_admission(outcome: ApprovalOutcome, follow_up: bool) {
        let (agent, root, host) = fixture().await;
        let (tx, rx) = oneshot::channel();
        let id = agent
            .begin_approval(CHAT, request(), WaitDelivery::Inline(tx))
            .await
            .unwrap();
        let closer = ReceiverCloser::install(&agent.store, rx);
        agent
            .decide_approval(&id, outcome.clone(), "synthetic-owner")
            .await
            .unwrap();
        assert_eq!(
            closer.hits(),
            1,
            "receiver must close at the actual SQL decision edge"
        );
        drop(closer);
        assert_eq!(
            agent
                .store
                .approval(&id)
                .unwrap()
                .unwrap()
                .grant_consumed_at,
            None
        );
        if follow_up {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if host.lock().await.doc.messages().iter().any(|m| m.rest.get("approvalId").and_then(Value::as_str) == Some(&id)) { break; }
                    tokio::task::yield_now().await;
                }
            }).await.expect("a rejected decision whose receiver disappeared must still produce its real follow-up row");
        } else {
            tokio::task::yield_now().await;
            assert!(
                !host.lock().await.doc.messages().iter().any(|m| m
                    .rest
                    .get("approvalId")
                    .and_then(Value::as_str)
                    == Some(&id)),
                "cancellation must remain silent"
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }
    #[tokio::test]
    async fn a_rejected_receiver_lost_after_classification_still_gets_its_followup() {
        receiver_loss_after_nonapproved_admission(
            ApprovalOutcome::Rejected {
                reason: Some("synthetic refusal".into()),
            },
            true,
        )
        .await;
    }
    #[tokio::test]
    async fn a_cancelled_receiver_lost_after_classification_remains_silent() {
        receiver_loss_after_nonapproved_admission(ApprovalOutcome::Cancelled, false).await;
    }
}
