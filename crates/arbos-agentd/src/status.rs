//! Quick replies while the agent is busy (Jacob, 2026-10-22: Doug sat on "Working…" for minutes
//! while a long optimisation loop ran; later one stuck relay turn held every chat).
//!
//! The main pi session runs one turn at a time behind `turn_lock`. A user message that arrives
//! while the lock is held used to wait, silent, until that turn ended. Now it gets its own row
//! within milliseconds and a real answer within seconds: one small model call over a snapshot
//! of the running work (what step is running, what it already said, which helpers are up, the
//! recent turns), answered without the main session. The model tags its answer:
//!
//! - `[status]` — a question the snapshot answers: the quick reply is the answer; the main
//!   session is told about the exchange in its next prompt and never answers it again.
//! - `[guide]` — guidance for the running work: the words go into the running turn as a steer
//!   (when a user turn of this chat is running). No bubble; the working row shows "Steering: …".
//! - `[task]` — anything else: the message queues for the main session exactly as before and
//!   gets its full answer when the lock frees. No bubble; the working row shows "Queued: …".
//!   (Jacob, 2026-10-23: status lives in the working row, never as "On it." / "Still on it…"
//!   placeholder bubbles.)
//!
//! Guardrails: one call of at most `QUICK_MAX_TOKENS`, `QUICK_CALLS_PER_MINUTE` model calls per
//! chat (then silent queue, no call), `ARBOS_QUICK_REPLY_TIMEOUT_MS` on the call (then silent
//! queue), tagged `x-arbos-purpose: quick-reply` for spend accounting. A status note is written
//! only after `BUSY_GRACE` of real waiting (so a turn that answers in normal time never gets
//! one) and at most once per busy turn per chat (#1448). Task acknowledgements — including the
//! old "Still on it: … N s in" canned line — never post a chat bubble (#1479): the apps already
//! show one quiet working indicator while the agent works. Nothing here touches the main
//! session's system prompt (D87) or the #1037 limit announcements.

use std::sync::Arc;
use std::time::{Duration, Instant};

use arbos_core::doc::{CommandView, MessageStatus};
use arbos_core::ids::ulid;
use serde_json::{json, Value};

use crate::agent::Agent;
use crate::sync::SharedHost;

/// What the quick-reply path did with a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickOutcome {
    /// No turn holds the lock: the normal path answers (nothing was written).
    NotBusy,
    /// A status answer was written; the main session does not run for this message.
    Answered,
    /// An acknowledgement was written and the words went into the running turn.
    Steered,
    /// The message queues for the main session. Task mid-turn arrivals get no acknowledgement
    /// bubble (#1479); only a Status answer writes a row; Guide steers silently.
    Queued,
}

/// The model's reading of a message that arrived while the agent was busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideKind {
    Status,
    Guide,
    Task,
}

impl SideKind {
    fn as_str(self) -> &'static str {
        match self {
            SideKind::Status => "status",
            SideKind::Guide => "guide",
            SideKind::Task => "task",
        }
    }
}

/// Output cap of the quick call: a few sentences (reasoning is off for the call, so the whole
/// budget is visible text).
pub const QUICK_MAX_TOKENS: u32 = 320;
/// Model calls per chat per minute before quick replies fall back to a silent queue.
pub const QUICK_CALLS_PER_MINUTE: usize = 6;
/// Characters of the user's message the quick call sees.
const USER_TEXT_MAX: usize = 1200;
/// How long the agent may stay busy before a new message earns a status note. Short turns that
/// free the lock inside this window never get a status row (#1448). Kept under the busy-reply
/// gate (~5 s) so a real status question still answers within seconds. Task acks never become
/// bubbles at all (#1479).
const BUSY_GRACE: Duration = Duration::from_secs(3);

/// `ARBOS_QUICK_REPLY_TIMEOUT_MS` (default 8000): the call's budget before a silent queue.
pub fn quick_reply_timeout() -> Duration {
    let ms = std::env::var("ARBOS_QUICK_REPLY_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(8000);
    Duration::from_millis(ms)
}

/// `ARBOS_QUICK_REPLY=off` turns the path off (every message waits for the lock, as before).
pub fn quick_reply_on() -> bool {
    !std::env::var("ARBOS_QUICK_REPLY").is_ok_and(|v| v.trim().eq_ignore_ascii_case("off"))
}

/// Split the model's answer into its tag and the text. No tag → `Task` (the safe reading:
/// the message still reaches the main session).
pub fn parse_tagged_reply(text: &str) -> (SideKind, String) {
    let t = text.trim();
    let lower = t.to_ascii_lowercase();
    for (tag, kind) in [
        ("[status]", SideKind::Status),
        ("[guide]", SideKind::Guide),
        ("[task]", SideKind::Task),
    ] {
        if lower.starts_with(tag) {
            return (kind, t[tag.len()..].trim().to_owned());
        }
    }
    (SideKind::Task, t.to_owned())
}

/// A quick reply must never land as a fragment. `truncated` = the provider hit the token cap.
/// Returns the text when it ends a sentence; when it does not (cap hit, or the text simply
/// stops mid-sentence), the text cut back to its last full sentence if that keeps at least
/// half of it; else `None` (the caller queues silently — no canned bubble, #1479).
pub fn finish_cleanly(text: &str, truncated: bool) -> Option<String> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let ends_sentence = |s: &str| {
        s.trim_end_matches(['"', '”', '’', ')', '*', '_'])
            .ends_with(['.', '!', '?', '…', ':'])
    };
    if !truncated && ends_sentence(t) {
        return Some(t.to_owned());
    }
    if truncated && ends_sentence(t) && t.chars().count() > 20 {
        return Some(t.to_owned());
    }
    // Cut back to the last sentence end followed by whitespace (or the very end).
    let chars: Vec<char> = t.chars().collect();
    let mut cut = None;
    for i in (0..chars.len()).rev() {
        if matches!(chars[i], '.' | '!' | '?' | '…')
            && (i + 1 == chars.len() || chars[i + 1].is_whitespace())
        {
            cut = Some(i + 1);
            break;
        }
    }
    let cut = cut?;
    if cut * 2 < chars.len() {
        return None;
    }
    let kept: String = chars[..cut].iter().collect();
    let kept = kept.trim();
    (!kept.is_empty()).then(|| kept.to_owned())
}

/// What the model's reading becomes. A `[status]` answer must read as a whole sentence
/// (`finish_cleanly`), else it is not posted and the message queues (`Task`, `None`). A
/// `[guide]` is a steer whatever the quick line looks like — the line is never shown, so a
/// missing full stop must not demote the steer to a silent job (#1657). A `[task]` has no line.
pub fn settle_quick(kind: SideKind, text: &str, truncated: bool) -> (SideKind, Option<String>) {
    match kind {
        SideKind::Status => match finish_cleanly(text, truncated) {
            Some(clean) => (SideKind::Status, Some(clean)),
            None => (SideKind::Task, None),
        },
        SideKind::Guide => (SideKind::Guide, None),
        SideKind::Task => (SideKind::Task, None),
    }
}

/// What the running work looks like, for the quick call.
#[derive(Debug, Default, Clone)]
pub struct BusySnapshot {
    /// A turn is running in this very chat (else: in another chat, or none and only waiting).
    pub same_chat: bool,
    /// `user turn` / `relay turn` / `queued behind other messages`.
    pub step: String,
    pub elapsed_s: u64,
    pub activity: Option<String>,
    /// Tool-call lines of the running turn, redacted, newest last.
    pub did: Vec<String>,
    /// What the running turn already told the user (ack, progress bubbles, prose so far).
    pub said: String,
    /// `name — status — last progress` per running helper.
    pub helpers: Vec<String>,
    /// `role: text` lines of this chat's recent turns.
    pub recent: Vec<String>,
    /// A user turn of this chat is running and can take a steer.
    pub steerable: bool,
}

impl BusySnapshot {
    /// The user-side block of the quick call.
    pub fn render(&self, user_text: &str) -> String {
        let mut out = String::new();
        out.push_str("Snapshot of your running work:\n");
        out.push_str(&format!(
            "- Current step: {} in {}, running for {} s.{}\n",
            self.step,
            if self.same_chat {
                "this conversation"
            } else {
                "another conversation"
            },
            self.elapsed_s,
            match &self.activity {
                Some(a) if !a.trim().is_empty() => format!(" Right now: {}.", a.trim()),
                _ => String::new(),
            }
        ));
        if !self.did.is_empty() {
            out.push_str("- Steps taken in this turn so far (internal, newest last):\n");
            for l in &self.did {
                out.push_str(&format!("    {l}\n"));
            }
        }
        if !self.said.trim().is_empty() {
            out.push_str(&format!(
                "- What you already told the user in this step: {}\n",
                self.said.trim()
            ));
        }
        if !self.helpers.is_empty() {
            out.push_str("- Background helpers running (their results arrive later):\n");
            for h in &self.helpers {
                out.push_str(&format!("    {h}\n"));
            }
        }
        if !self.recent.is_empty() {
            out.push_str("- Recent turns in this conversation:\n");
            for r in &self.recent {
                out.push_str(&format!("    {r}\n"));
            }
        }
        out.push_str(&format!(
            "\nThe user's new message:\n{}\n",
            user_text.chars().take(USER_TEXT_MAX).collect::<String>()
        ));
        out
    }
}

/// The quick call's instructions (system prompt of the small call, not the main session's).
pub fn quick_system_prompt(agent_name: &str, address_note: Option<&str>) -> String {
    let me = if agent_name.trim().is_empty() || agent_name.trim().eq_ignore_ascii_case("arbos") {
        "You are this user's agent in Arbos.".to_owned()
    } else {
        format!("You are {}, this user's agent in Arbos.", agent_name.trim())
    };
    format!(
        "{me} Right now you are in the middle of work (the snapshot below), and the part of you that does the work cannot answer this message yet. You answer it now, from the snapshot alone.\n\
Rules: first person, plain chat prose, one to three short sentences. Say what you are doing and how far along it is in the user's terms. Never mention tools, scripts, commands, models, harnesses, helpers by name, internal ids or token counts. Never invent progress that is not in the snapshot. Never promise a time.\n\
Start your reply with exactly one tag on its own line:\n\
[status] when the message asks how things are going, what is happening, or something the snapshot or the recent turns answer.\n\
[guide] when the message corrects, redirects or adds to the work that is running.\n\
[task] when the message asks for something new, or something you cannot answer from the snapshot.\n\
After [guide] or [task] write nothing else: the message is taken up by the part of you that works, and the user sees that on the working line, not as a reply from you.{}",
        match address_note {
            Some(n) if !n.trim().is_empty() => format!("\n{}", n.trim()),
            _ => String::new(),
        }
    )
}

fn clip(s: &str, max: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        format!("{}…", one.chars().take(max).collect::<String>())
    }
}

impl Agent {
    /// The notes for `chat_id`'s next main prompt, drained.
    pub(crate) fn take_side_notes(&self, chat_id: &str) -> Option<String> {
        let notes = self
            .side_notes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(chat_id)?;
        if notes.is_empty() {
            return None;
        }
        Some(format!(
            "[Arbos: while you were busy, these messages from the user were answered for you from your running state. Do not answer them again; carry on.]\n{}",
            notes.join("\n")
        ))
    }

    fn push_side_note(&self, chat_id: &str, question: &str, answer: &str) {
        let mut all = self.side_notes.lock().unwrap_or_else(|e| e.into_inner());
        let v = all.entry(chat_id.to_owned()).or_default();
        v.push(format!(
            "- User: {} — You answered: {}",
            clip(question, 300),
            clip(answer, 400)
        ));
        if v.len() > 6 {
            let n = v.len() - 6;
            v.drain(..n);
        }
    }

    /// Whether a model call is allowed now (`QUICK_CALLS_PER_MINUTE` per chat).
    fn quick_call_allowed(&self, chat_id: &str) -> bool {
        let mut all = self.quick_calls.lock().unwrap_or_else(|e| e.into_inner());
        let v = all.entry(chat_id.to_owned()).or_default();
        let now = Instant::now();
        v.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
        if v.len() >= QUICK_CALLS_PER_MINUTE {
            return false;
        }
        v.push(now);
        true
    }

    /// `true` when this chat already got a status note for the running turn (at most one per
    /// waiting period). `turn_started` is `TurnState::started` of the held turn; with no turn
    /// yet there is nothing to pin a waiting period to, so a note is always allowed.
    fn already_quick_noted(&self, chat_id: &str, turn_started: Option<Instant>) -> bool {
        let Some(started) = turn_started else {
            return false;
        };
        let mut noted = self.quick_noted.lock().unwrap_or_else(|e| e.into_inner());
        // Drop notes that belong to a previous turn.
        noted.retain(|_, t| *t == started);
        noted.contains_key(chat_id)
    }

    fn mark_quick_noted(&self, chat_id: &str, turn_started: Option<Instant>) {
        let Some(started) = turn_started else {
            return;
        };
        let mut noted = self.quick_noted.lock().unwrap_or_else(|e| e.into_inner());
        noted.retain(|_, t| *t == started);
        noted.insert(chat_id.to_owned(), started);
    }

    /// The running work as the quick call sees it; `None` when nothing holds the lock and
    /// nothing is waiting for it.
    pub(crate) async fn busy_snapshot(
        &self,
        chat_id: &str,
        host: &SharedHost,
    ) -> Option<BusySnapshot> {
        let lock_busy = self.turn_lock_busy();
        let waiting = self.sends_waiting();
        if !lock_busy && waiting == 0 {
            return None;
        }
        let mut snap = BusySnapshot::default();
        {
            let tl = self.turn.lock().await;
            match tl.as_ref() {
                Some(t) => {
                    snap.same_chat = t.chat_id == chat_id;
                    snap.step = if t.reply_only {
                        "writing up finished background work".to_owned()
                    } else {
                        "working on a request".to_owned()
                    };
                    snap.elapsed_s = t.started.elapsed().as_secs();
                    snap.activity = t.activity.clone();
                    snap.did = t
                        .act_transcript
                        .iter()
                        .rev()
                        .take(10)
                        .map(|l| clip(&crate::store::redact(l), 160))
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    let mut said = String::new();
                    if !t.ack_text.trim().is_empty() {
                        said.push_str(t.ack_text.trim());
                    }
                    for p in &t.progress_texts {
                        said.push(' ');
                        said.push_str(p.trim());
                    }
                    if !t.streamed_text.trim().is_empty() {
                        said.push(' ');
                        said.push_str(t.streamed_text.trim());
                    }
                    snap.said = clip(&said, 700);
                    snap.steerable = snap.same_chat && !t.stopped && !t.reply_only;
                }
                None => {
                    snap.same_chat = true;
                    snap.step = if waiting > 0 {
                        format!("{waiting} earlier message(s) ahead of this one")
                    } else {
                        "getting ready".to_owned()
                    };
                }
            }
        }
        // A pending approval card is not running work: never report the gated tool's label.
        let turn_chat = {
            let tl = self.turn.lock().await;
            tl.as_ref().map(|t| t.chat_id.clone())
        };
        if let Some(tc) = turn_chat {
            if self.approval_pending_for(&tc).await {
                snap.activity = Some("Waiting for approval".into());
            }
        }
        let live: std::collections::HashSet<String> =
            self.subagent_msgs.lock().await.keys().cloned().collect();
        for s in self.store.subagents().unwrap_or_default() {
            let running = s.status == "queued" || (s.status == "running" && live.contains(&s.id));
            if !running {
                continue;
            }
            let since = s.last_turn_at.unwrap_or(s.created_at).max(0);
            let age_s = (arbos_core::ids::now_ms() - since).max(0) / 1000;
            let progress = {
                let id = self
                    .subagent_msgs
                    .lock()
                    .await
                    .get(&s.id)
                    .map(|(_, m)| m.clone());
                match id {
                    Some(mid) => {
                        let h = host.lock().await;
                        h.doc.message(&mid).map(|m| m.text()).unwrap_or_default()
                    }
                    None => String::new(),
                }
            };
            let last = if !progress.trim().is_empty() {
                let p = progress.trim();
                let tail: String = p
                    .chars()
                    .rev()
                    .take(300)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                clip(&crate::store::redact(&tail), 300)
            } else if s.last_status.is_empty() {
                "starting".to_owned()
            } else {
                clip(&s.last_status, 200)
            };
            snap.helpers.push(format!(
                "{} — {} — purpose: {} — {} s since its last step — latest: {}",
                s.name,
                s.status,
                clip(&s.purpose, 120),
                age_s,
                last
            ));
            if snap.helpers.len() >= 5 {
                break;
            }
        }
        snap.recent = self
            .store
            .t3_thread(chat_id, None, None, 10)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.kind == "user" || r.kind == "assistant")
            .map(|r| {
                format!(
                    "{}: {}",
                    r.role.unwrap_or_default(),
                    clip(&crate::store::redact(&r.text), 220)
                )
            })
            .collect();
        Some(snap)
    }

    /// The quick-reply path for a `send` / `steer` / `guide` command. See the module docs.
    pub(crate) async fn quick_reply(
        self: &Arc<Self>,
        chat_id: &str,
        host: &SharedHost,
        cmd: &CommandView,
    ) -> QuickOutcome {
        if !quick_reply_on() || cmd.issued_by == self.cfg.device_id {
            // The box's own sends (routines, channels, group fan-out, voice) expect the normal
            // reply row and are not a person waiting at a keyboard.
            return QuickOutcome::NotBusy;
        }
        let user_msg_id = cmd
            .payload
            .get("messageId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if user_msg_id.is_empty() {
            return QuickOutcome::NotBusy;
        }
        let (user_text, from_voice, room) = {
            let h = host.lock().await;
            let m = h.doc.message(&user_msg_id);
            (
                m.as_ref().map(|m| m.text()).unwrap_or_default(),
                m.as_ref().is_some_and(|m| {
                    m.rest.get("voice").and_then(Value::as_bool) == Some(true)
                        || m.rest.get("source").and_then(Value::as_str) == Some("voice")
                }),
                h.room.clone(),
            )
        };
        if from_voice || user_text.trim().is_empty() {
            return QuickOutcome::NotBusy;
        }
        // Wait out short turns: if the lock frees (or the queue drains) inside BUSY_GRACE the
        // user is not really waiting — no status note (#1448). Poll the snapshot so a free
        // agent exits early instead of only watching the lock.
        let t_wait = Instant::now();
        let snap = loop {
            match self.busy_snapshot(chat_id, host).await {
                None => return QuickOutcome::NotBusy,
                Some(s) if t_wait.elapsed() >= BUSY_GRACE => break s,
                Some(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        };
        let turn_started = {
            let tl = self.turn.lock().await;
            tl.as_ref().map(|t| t.started)
        };
        // One status note per busy turn per chat; further messages queue with no extra row.
        if self.already_quick_noted(chat_id, turn_started) {
            tracing::info!(chat_id, "quick reply skipped; already noted this wait");
            return QuickOutcome::Queued;
        }
        let t0 = Instant::now();
        // No row yet: a bubble is written only for a status answer. Acknowledgements for queued
        // work and steers are status and live on the working row (Jacob, 2026-10-23: no "On
        // it." / "Still on it…" placeholders in the transcript).
        let address = self.owner_address_note(chat_id, host, &user_msg_id).await;
        let (talking_model, _) = self.live_models().await;
        let model = std::env::var("ARBOS_QUICK_REPLY_MODEL")
            .ok()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or(talking_model);
        let mut used_model = false;
        let (kind, text) = if self.quick_call_allowed(chat_id) {
            used_model = true;
            let system = quick_system_prompt(&self.name(), address.as_deref());
            let user = snap.render(&user_text);
            let llm = self.llm.with_purpose("quick-reply");
            // No hidden reasoning on this call: with it on, the model's thinking counted
            // against `max_tokens` and the visible sentence was cut at the cap (#1411).
            let extra = json!({ "reasoning": { "enabled": false } });
            match tokio::time::timeout(
                quick_reply_timeout(),
                llm.complete_full(&model, &system, &user, QUICK_MAX_TOKENS, Some(&extra)),
            )
            .await
            {
                Ok(Ok(c)) => {
                    let (kind, text) = parse_tagged_reply(&c.text);
                    let truncated = c.truncated();
                    tracing::info!(
                        chat_id,
                        kind = kind.as_str(),
                        ms = c.latency_ms,
                        truncated,
                        output_tokens = c
                            .usage
                            .get("completion_tokens")
                            .and_then(serde_json::Value::as_i64),
                        "quick reply from the model"
                    );
                    match settle_quick(kind, &text, truncated) {
                        (k, Some(clean)) => (k, clean),
                        (SideKind::Guide, None) => (SideKind::Guide, String::new()),
                        (_, None) => {
                            tracing::warn!(
                                chat_id,
                                truncated,
                                chars = text.chars().count(),
                                "quick reply did not end a sentence; silent queue"
                            );
                            (SideKind::Task, String::new())
                        }
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!(chat_id, error = %e, "quick reply call failed; silent queue");
                    (SideKind::Task, String::new())
                }
                Err(_) => {
                    tracing::warn!(chat_id, "quick reply call timed out; silent queue");
                    (SideKind::Task, String::new())
                }
            }
        } else {
            tracing::info!(chat_id, "quick reply rate limit; silent queue");
            (SideKind::Task, String::new())
        };
        // Guidance needs a user turn of this chat to steer; otherwise it queues like a task.
        let kind = if kind == SideKind::Guide && !snap.steerable {
            SideKind::Task
        } else {
            kind
        };
        // #1479: Task mid-turn arrivals (and every canned "Still on it" fallback) never post a
        // chat bubble — the apps already show one quiet working indicator. Queue for the main
        // session; do not mark this wait as noted so a later status question can still answer.
        if kind == SideKind::Task {
            let _ = self.store.t3_append(
                chat_id,
                None,
                Some(chat_id),
                "quick_reply",
                Some("system"),
                &format!(
                    "quick reply task (silent): {}model call, {} ms after the grace",
                    if used_model { "" } else { "no " },
                    t0.elapsed().as_millis()
                ),
                Some(&json!({
                    "message_id": user_msg_id, "side": "task", "model_call": used_model,
                    "model": model, "waited_ms": t_wait.elapsed().as_millis() as u64,
                    "steerable": snap.steerable, "silent": true,
                })),
            );
            tracing::info!(
                chat_id,
                ms = t0.elapsed().as_millis() as i64,
                model_call = used_model,
                "quick reply silent queue; no ack bubble"
            );
            return QuickOutcome::Queued;
        }
        let text = crate::store::redact(text.trim());
        // Only a status answer needs words; a guide has none to show and still steers (#1657:
        // this early return demoted every guide whose line was dropped to a silent queue).
        if kind == SideKind::Status && text.is_empty() {
            return QuickOutcome::Queued;
        }
        // The decision row for debugging readers (`inspect`, kind `quick_reply`): which side the
        // model read the message as, whether a model call was made, how long the user waited.
        let _ = self.store.t3_append(
            chat_id,
            None,
            Some(chat_id),
            "quick_reply",
            Some("system"),
            &format!(
                "quick reply {}: {}model call, {} ms after the grace",
                kind.as_str(),
                if used_model { "" } else { "no " },
                t0.elapsed().as_millis()
            ),
            Some(&json!({
                "message_id": user_msg_id, "side": kind.as_str(), "model_call": used_model,
                "model": model, "waited_ms": t_wait.elapsed().as_millis() as u64, "steerable": snap.steerable,
            })),
        );
        self.mark_quick_noted(chat_id, turn_started);
        if kind != SideKind::Status {
            // Queued or steered: the working row carries it; nothing in the transcript until
            // the real answer. The presence line names what was taken, briefly.
            let gist: String = user_text.split_whitespace().collect::<Vec<_>>().join(" ");
            let gist: String = gist.chars().take(40).collect();
            self.post_activity(
                chat_id,
                Some(format!(
                    "{}: {gist}…",
                    if kind == SideKind::Guide {
                        "Steering"
                    } else {
                        "Queued"
                    }
                )),
            );
            tracing::info!(
                chat_id,
                kind = kind.as_str(),
                ms = t0.elapsed().as_millis() as i64,
                model_call = used_model,
                "quick reply: no bubble, working row only"
            );
        } else {
            let row_id = ulid();
            let h = host.lock().await;
            h.doc
                .start_assistant_message(&row_id, &user_msg_id, &self.cfg.device_id);
            h.doc.set_message_fields(&row_id, &[("quick", json!(true))]);
            if let Some(attr) = self.people_reply_fields(chat_id) {
                h.doc.set_message_fields(&row_id, &attr.fields);
            }
            if let Some(p) = h.doc.add_text_part(&row_id, "text") {
                h.doc.append_text(&row_id, &p, &text);
            }
            h.doc.set_message_fields(
                &row_id,
                &[
                    ("turnRole", json!("result")),
                    ("decision", json!("quick")),
                    (
                        "turn",
                        json!({ "kind": "quick", "side": "status", "model_call": used_model }),
                    ),
                    ("model", json!(model)),
                ],
            );
            h.doc.finish_message(&row_id, MessageStatus::Done);
            drop(h);
            tracing::info!(
                chat_id,
                ms = t0.elapsed().as_millis() as i64,
                model_call = used_model,
                "quick reply written"
            );
            self.announce_reply_kind(&room, &row_id, &text, "quick", None);
            let _ = self.store.t3_append(
                chat_id,
                None,
                Some(chat_id),
                "assistant",
                Some("assistant"),
                &text,
                Some(&json!({ "message_id": row_id, "quick": true, "side": "status" })),
            );
        }
        match kind {
            SideKind::Status => {
                let _ = self.store.t3_append(
                    chat_id,
                    None,
                    Some(chat_id),
                    "user",
                    Some("user"),
                    &user_text,
                    Some(&json!({ "message_id": user_msg_id, "quick": true })),
                );
                self.push_side_note(chat_id, &user_text, &text);
                QuickOutcome::Answered
            }
            SideKind::Guide => {
                // `handle_steer` appends the user text to T3 and steers the running turn; when
                // the turn ended meanwhile it falls back to a normal send on its own.
                match self.handle_steer(chat_id, host, cmd).await {
                    Ok(()) => QuickOutcome::Steered,
                    Err(e) => {
                        tracing::warn!(chat_id, error = %e, "quick guide steer failed; queuing");
                        QuickOutcome::Queued
                    }
                }
            }
            SideKind::Task => {
                // Handled above (silent queue); kept for exhaustiveness.
                QuickOutcome::Queued
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// #1657: a `[guide]` whose quick line lacks a full stop was demoted to a silent job; a guide
    /// steers regardless of the line. A fragment `[status]` still queues instead of posting.
    #[test]
    fn a_guide_steers_whatever_its_line_looks_like() {
        assert_eq!(
            settle_quick(
                SideKind::Guide,
                "Stopping at 20 instead of 40 for the run that is going",
                false
            ),
            (SideKind::Guide, None)
        );
        assert_eq!(
            settle_quick(SideKind::Guide, "", false),
            (SideKind::Guide, None)
        );
        assert_eq!(
            settle_quick(SideKind::Status, "The run is at step 12 of 40,", true),
            (SideKind::Task, None)
        );
        assert_eq!(
            settle_quick(SideKind::Status, "The run is at step 12 of 40.", false),
            (
                SideKind::Status,
                Some("The run is at step 12 of 40.".to_owned())
            )
        );
        assert_eq!(
            settle_quick(SideKind::Task, "Noted.", false),
            (SideKind::Task, None)
        );
    }

    #[test]
    fn tags_parse_and_default_to_task() {
        assert_eq!(
            parse_tagged_reply("[status]\nBatch 4 is about ten minutes in."),
            (
                SideKind::Status,
                "Batch 4 is about ten minutes in.".to_owned()
            )
        );
        assert_eq!(
            parse_tagged_reply("  [GUIDE] Switching to the smaller set after this step."),
            (
                SideKind::Guide,
                "Switching to the smaller set after this step.".to_owned()
            )
        );
        assert_eq!(
            parse_tagged_reply("[task]\nNoted; right after this step."),
            (SideKind::Task, "Noted; right after this step.".to_owned())
        );
        assert_eq!(
            parse_tagged_reply("No tag at all."),
            (SideKind::Task, "No tag at all.".to_owned())
        );
    }

    /// #1411: "When I last reported, the 15-second UTC logger had started at 04:15:49Z with 3
    /// lines written," landed as a done bubble — the token cap had cut it (reasoning tokens ate
    /// the budget). Fragments are cut back to the last full sentence or replaced.
    #[test]
    fn fragments_never_land_as_a_reply() {
        let frag = "When I last reported, the 15-second UTC logger had started at 04:15:49Z with 3 lines written,";
        assert_eq!(finish_cleanly(frag, true), None);
        assert_eq!(
            finish_cleanly(frag, false),
            None,
            "a comma ending is a fragment, cap or not"
        );
        let two = "Both are running. The UTC logger started at 04:15:49Z and the per-minute routine has already appended a tick. The next";
        assert_eq!(
            finish_cleanly(two, true).as_deref(),
            Some("Both are running. The UTC logger started at 04:15:49Z and the per-minute routine has already appended a tick.")
        );
        // Too little left after the cut: better a silent queue than one stub sentence.
        assert_eq!(finish_cleanly("Yes. The logger started at 04:15:49Z and has been writing a line every fifteen seconds since, with", true), None);
        assert_eq!(
            finish_cleanly("Yes, I'm here.", false).as_deref(),
            Some("Yes, I'm here.")
        );
        assert_eq!(
            finish_cleanly("Still on it (about a minute in).", true).as_deref(),
            Some("Still on it (about a minute in).")
        );
        assert_eq!(finish_cleanly("   ", false), None);
    }

    #[test]
    fn snapshot_renders_without_internals_in_the_prose_rules() {
        let s = BusySnapshot {
            same_chat: true,
            step: "working on a request".into(),
            elapsed_s: 30,
            activity: Some("Running a command…".into()),
            did: vec!["→ bash {\"command\":\"sleep 100\"}".into()],
            said: "Starting the benchmark now.".into(),
            helpers: vec![
                "loop-2 — running — purpose: optimise — 20 s since its last step — latest: round 3"
                    .into(),
            ],
            recent: vec!["user: start the loop".into()],
            steerable: true,
        };
        let r = s.render("what is going on now?");
        assert!(r.contains("Current step: working on a request in this conversation, running for 30 s. Right now: Running a command…."));
        assert!(r.contains("sleep 100"));
        assert!(r.contains("The user's new message:\nwhat is going on now?"));
        let p = quick_system_prompt("Doug", Some("[The user is Arbos.]"));
        assert!(p.starts_with("You are Doug, this user's agent in Arbos."));
        assert!(p.contains("[status]") && p.contains("[guide]") && p.contains("[task]"));
        assert!(p.ends_with("[The user is Arbos.]"));
        assert!(
            quick_system_prompt("Arbos", None).starts_with("You are this user's agent in Arbos.")
        );
    }

    // ---- the latency gate: a busy agent answers a new message within seconds -------------------
    //
    // A stand-in relay serves the quick call: `[status]` for "what is going on", `[task]` for
    // anything else, after a 300 ms think. The agent's turn lock is held the whole time, as a
    // running act turn (or Doug's 13-minute relay turn) would hold it. The main pi session never
    // runs: `pi_bin` points at nothing and the lock is never released.

    /// First visible reply within this many seconds, lock held or not.
    pub const FIRST_REPLY_GATE_SECS: u64 = 5;

    async fn mock_quick(axum::Json(body): axum::Json<Value>) -> axum::Json<Value> {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let user = body["messages"][1]["content"].as_str().unwrap_or("");
        let (content, finish) = if user.contains("stop at 20, not 40") {
            // The b7 shape: a guide whose one line has no full stop.
            (
                "[guide]\nStopping the run at 20 instead of 40 as you asked",
                "stop",
            )
        } else if user.contains("did the logger") {
            ("[status]\nWhen I last reported, the 15-second UTC logger had started at 04:15:49Z with 3 lines written,", "length")
        } else if user.contains("what is going on") {
            (
                "[status]\nThe benchmark is still running, about a minute in; nothing has failed.",
                "stop",
            )
        } else {
            (
                "[task]\nNoted; I'll add that right after the current step.",
                "stop",
            )
        };
        assert_eq!(
            body["reasoning"]["enabled"],
            json!(false),
            "reasoning is off for quick calls"
        );
        axum::Json(json!({
            "choices": [{ "message": { "role": "assistant", "content": content }, "finish_reason": finish }],
            "usage": { "prompt_tokens": 400, "completion_tokens": 30 }
        }))
    }

    /// Every activity text the stub relay received (`POST /projects/{id}/activity`), in order.
    /// A log, not a last-value slot: tests run in parallel and another chat's act ack can land
    /// between a test's own post and its read.
    static ACTIVITY_LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    async fn mock_activity(axum::Json(body): axum::Json<Value>) -> axum::http::StatusCode {
        if let Some(t) = body.get("text").and_then(Value::as_str) {
            ACTIVITY_LOG.lock().unwrap().push(t.to_owned());
        }
        axum::http::StatusCode::NO_CONTENT
    }

    pub(crate) async fn test_agent() -> (Arc<Agent>, std::path::PathBuf) {
        test_agent_with(axum::Router::new()).await
    }

    /// Same, with extra mock-relay routes merged in (`/box/secrets/…` for the hand-off tests).
    pub(crate) async fn test_agent_with(extra: axum::Router) -> (Arc<Agent>, std::path::PathBuf) {
        let app = axum::Router::new()
            .merge(extra)
            .route(
                "/proxy/v1/chat/completions",
                axum::routing::post(mock_quick),
            )
            .route(
                "/projects/{id}/activity",
                axum::routing::post(mock_activity),
            )
            // deliver_file's upload (media.rs): the stub accepts any blob.
            .route(
                "/blob/{slot}/{key}",
                axum::routing::put(|| async { axum::http::StatusCode::CREATED }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let root = std::env::temp_dir().join(format!("arbos-quick-{}", ulid()));
        std::fs::create_dir_all(&root).unwrap();
        let bx = crate::config::BoxConfig {
            user_id: Some("u-test".into()),
            relay_url: format!("http://127.0.0.1:{port}"),
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
        let cfg = Arc::new(bx.workspace(
            &json!({ "id": "01TESTPROJECTQUICKREPLY000", "name": "Doug" }),
            0,
        ));
        std::fs::create_dir_all(cfg.state_dir.join("agentd")).unwrap();
        let store =
            crate::store::Store::open(&cfg.state_dir.join("agentd").join("agentd.db")).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(64);
        let sync = crate::sync::SyncHub::new(cfg.clone(), store.clone(), tx);
        let agent = Agent::new(cfg, store, sync, Arc::new(tokio::sync::Semaphore::new(4)));
        (agent, root)
    }

    fn send_cmd(host: &SharedHost, text: &str) -> (String, CommandView) {
        let msg_id = ulid();
        let h = host.try_lock().expect("host free");
        h.doc
            .append_user_message(
                &msg_id,
                text,
                "dev-mac",
                &arbos_core::doc::UserMessageOpts::default(),
            )
            .unwrap();
        let cmd = CommandView {
            id: ulid(),
            kind: "send".into(),
            payload: json!({ "messageId": msg_id }),
            issued_by: "dev-mac".into(),
            issued_at: arbos_core::ids::now_ms(),
            status: "pending".into(),
        };
        (msg_id, cmd)
    }

    fn reply_rows(host: &SharedHost, user_msg_id: &str) -> Vec<arbos_core::doc::MessageView> {
        let h = host.try_lock().expect("host free");
        h.doc
            .messages()
            .into_iter()
            .filter(|m| {
                m.role == "assistant"
                    && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(user_msg_id)
            })
            .collect()
    }

    #[tokio::test]
    async fn busy_agent_answers_a_status_question_within_the_gate() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000000";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        let (msg_id, cmd) = send_cmd(&host, "what is going on now?");
        let t0 = Instant::now();
        agent.handle_command(chat, cmd).await.unwrap();
        let elapsed = t0.elapsed();
        let rows = reply_rows(&host, &msg_id);
        assert_eq!(rows.len(), 1, "one quick row, no main-session row");
        let text = rows[0].text();
        assert!(text.contains("benchmark is still running"), "{text}");
        assert_eq!(rows[0].status, "done");
        assert!(
            elapsed < Duration::from_secs(FIRST_REPLY_GATE_SECS),
            "first reply took {elapsed:?} with the turn lock held"
        );
        // The main session will be told, once, and the note is then gone.
        let note = agent.take_side_notes(chat).expect("side note");
        assert!(note.contains("what is going on now?") && note.contains("benchmark"));
        assert!(agent.take_side_notes(chat).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #1431 (GigChadAgent): after the relay renames the project, the system prompt, the quick
    /// reply and the owner note speak the new name and never present the old one as current.
    #[tokio::test]
    async fn renamed_agent_speaks_as_its_new_name() {
        let (agent, root) = test_agent().await;
        assert_eq!(agent.name(), "Doug");
        agent.write_system_prompt().unwrap();
        let path = agent.cfg.state_dir.join("pi").join("system-prompt.md");
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(before.contains("You are Doug, this user's agent in Arbos"));

        let r = agent.identity.observe("GigChadAgent").expect("renamed");
        assert_eq!((r.from.as_str(), r.to.as_str()), ("Doug", "GigChadAgent"));
        assert_eq!(agent.name(), "GigChadAgent");
        assert_eq!(agent.identity.handle(), "gigchadagent");
        agent.write_system_prompt().unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("You are GigChadAgent, this user's agent in Arbos"));
        assert!(after.contains("as @gigchadagent"));
        assert!(after.contains("You used to be called \"Doug\""));
        assert!(!after.contains("You are Doug"));

        let quick = quick_system_prompt(&agent.name(), None);
        assert!(quick.starts_with("You are GigChadAgent, this user's agent in Arbos."));
        let note =
            crate::people::owner_address_note_text(Some("Jacob"), None, &agent.name()).unwrap();
        assert!(note.contains("You are GigChadAgent; \"GigChadAgent\" is your own name"));
        assert!(!note.contains("Doug"));
        // The history survives on disk for the next boot.
        let saved: crate::identity::Identity = serde_json::from_str(
            &std::fs::read_to_string(agent.cfg.state_dir.join("identity.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved.name, "GigChadAgent");
        assert_eq!(saved.former_names(), vec!["Doug".to_owned()]);
        let _ = std::fs::remove_dir_all(root);
    }

    /// #1481: renaming an agent already in a people group refreshes the cached @slug and
    /// member handle so @newname still wakes it (no remove/re-add).
    #[tokio::test]
    async fn renamed_agent_keeps_people_group_handle() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTPEOPLERENAME000001";
        let me = format!("agent:{}", agent.cfg.project_id);
        agent.people.test_insert(
            chat,
            crate::people::PeopleChatInfo {
                members: vec![serde_json::json!({
                    "member": me,
                    "handle": "doug",
                    "name": "Doug",
                    "project": { "name": "Doug" },
                })],
                slug: "doug".into(),
                kind: "group".into(),
                ..Default::default()
            },
        );
        let r = agent.identity.observe("FeedbackGuy").expect("renamed");
        agent.on_renamed(&r).await;
        let info = agent.people.info(chat).expect("people chat");
        assert_eq!(info.slug, "feedbackguy");
        let mine = info
            .members
            .iter()
            .find(|m| m["member"].as_str() == Some(&format!("agent:{}", agent.cfg.project_id)))
            .expect("member");
        assert_eq!(mine["handle"].as_str(), Some("feedbackguy"));
        assert_eq!(mine["project"]["name"].as_str(), Some("FeedbackGuy"));
        assert_eq!(
            crate::people::mention_role("@feedbackguy you alive?", &info.slug),
            crate::people::MentionRole::Addressed
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Jacob's rule (2026-10-23): a new ask while busy is queued silently — status on the
    /// working row, no acknowledgement bubble, no "Still on it…" placeholder; the real answer
    /// is the only row the message ever gets.
    #[tokio::test]
    async fn busy_agent_queues_a_new_task_without_a_placeholder_row() {
        let (agent, root) = test_agent().await;
        *agent.job_cap.lock().unwrap() = Some(0);
        let chat = "01TESTCHATQUICKREPLY000001";
        let host = agent.sync.open_room(chat).await.unwrap();
        let busy = agent.test_hold_turn(chat).await;
        let (msg_id, cmd) = send_cmd(&host, "also add a plot of the results when you are done");
        let a2 = agent.clone();
        let chat2 = chat.to_owned();
        let waiting = tokio::spawn(async move { a2.handle_command(&chat2, cmd).await });
        // Give the quick path its grace + model call, then look: the message is queued for the
        // main session (one waiter) and has no assistant row at all.
        let t0 = Instant::now();
        while agent.sends_waiting() == 0
            && t0.elapsed() < Duration::from_secs(FIRST_REPLY_GATE_SECS)
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            agent.sends_waiting(),
            1,
            "the ask queues for the main session"
        );
        assert!(
            reply_rows(&host, &msg_id).is_empty(),
            "no placeholder / ack row for a queued ask"
        );
        assert!(!waiting.is_finished());
        assert!(
            agent.take_side_notes(chat).is_none(),
            "a task is not a side note"
        );
        waiting.abort();
        drop(busy);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Jacob's screenshot (TestAgent, 2026-10-23 23:58 UTC): "On it." as a bubble above the
    /// working line. An `act` turn's ack — the model's or none — is working-row status; the
    /// mention gets exactly one row, the result, and no "On it." is ever written, not even when
    /// a tool starts before any ack.
    #[tokio::test]
    async fn act_turn_writes_no_ack_row_only_the_result() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000006";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.ack_text.clear();
            t.activity = None;
            agent.test_open_row(&host, t).await;
        }
        let user_msg = "m-busy";
        // A: the model gave an ack → it becomes the working row's activity, not a bubble.
        agent
            .hook_route(&json!({ "action": "act", "ack": "Counting the messages in this chat.", "plan": "read the doc" }))
            .await
            .unwrap();
        let rows = reply_rows(&host, user_msg);
        assert_eq!(rows.len(), 1, "one row: the result");
        assert_eq!(rows[0].text().trim(), "", "no ack text in the transcript");
        assert_eq!(
            agent
                .turn
                .lock()
                .await
                .as_ref()
                .unwrap()
                .activity
                .as_deref(),
            Some("Counting the messages in this chat.")
        );
        // B: a tool starts (nothing acked before it) → still no "On it.".
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.ack_split = false;
            t.ack_text.clear();
            t.route = Some(arbos_core::route::ValidRoute::Act { plan: "x".into() });
        }
        agent
            .test_fold(
                chat,
                &host,
                &arbos_core::rpc::Event::ToolExecutionStart {
                    tool_call_id: "c1".into(),
                    tool_name: "bash".into(),
                    args: json!({ "command": "ls" }),
                    parent_tool_call_id: None,
                },
            )
            .await;
        let rows = reply_rows(&host, user_msg);
        assert_eq!(rows.len(), 1);
        let all_text: String = rows.iter().map(|r| r.text()).collect();
        assert!(!all_text.contains("On it"), "{all_text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Duplex step 1 (`docs/duplex-agent-design.md` §2.1): with the send tool on, prose the
    /// model writes outside `send_message` is not posted; a turn that posted nothing gets its
    /// leftover prose once as the fallback (`On`), never under `Strict`; a `send_message`
    /// bubble carries `replyTo` when asked and never trips the fallback.
    #[tokio::test]
    async fn send_tool_holds_leftover_prose_and_posts_the_fallback_once() {
        use crate::config::SendTool;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000007";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        let user_msg = "m-busy";
        // A: `On`, nothing posted → the leftover prose is the one fallback bubble, once.
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.ack_text.clear();
            t.activity = None;
            t.send_tool = SendTool::On;
            agent.test_open_row(&host, t).await;
            agent
                .test_prose_then_finish(&host, t, "The tests pass now.")
                .await;
            assert!(t.posted_fallback);
            assert_eq!(t.streamed_text.trim(), "The tests pass now.");
            // a second pass changes nothing
            agent.test_prose_then_finish(&host, t, "").await;
            assert_eq!(t.streamed_text.trim(), "The tests pass now.");
        }
        let rows = reply_rows(&host, user_msg);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text().trim(), "The tests pass now.");
        // B: `Strict`, nothing posted → silence; the words stay out of the doc.
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.streamed_text.clear();
            t.posted_fallback = false;
            t.send_tool = SendTool::Strict;
            agent.test_open_row(&host, t).await;
            agent
                .test_prose_then_finish(&host, t, "Thinking about what to do next.")
                .await;
            assert!(!t.posted_fallback);
            assert!(t.streamed_text.trim().is_empty());
            assert_eq!(t.unposted_text.trim(), "Thinking about what to do next.");
        }
        let rows = reply_rows(&host, user_msg);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[1].text().trim(),
            "",
            "strict: leftover prose never posted"
        );
        // C: `On`, the model posted with `send_message` → leftover prose is notes, not a bubble.
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.streamed_text.clear();
            t.unposted_text.clear();
            t.send_tool = SendTool::On;
            agent.test_open_row(&host, t).await;
        }
        {
            // the ask exists as a row, so an explicit reply_to can point at it
            let h = host.lock().await;
            h.doc
                .append_user_message(
                    user_msg,
                    "rename it everywhere",
                    "dev-mac",
                    &arbos_core::doc::UserMessageOpts::default(),
                )
                .unwrap();
        }
        agent
            .hook_send_message("Three files use that name.", None, Some(user_msg))
            .await
            .unwrap();
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            agent
                .test_prose_then_finish(&host, t, "I should also mention the tests.")
                .await;
            assert!(!t.posted_fallback, "a turn that posted does not fall back");
            assert!(t.streamed_text.trim().is_empty());
        }
        let rows = reply_rows(&host, user_msg);
        let posted: Vec<String> = rows
            .iter()
            .map(|r| r.text().trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect();
        assert_eq!(
            posted,
            vec![
                "The tests pass now.".to_owned(),
                "Three files use that name.".to_owned()
            ]
        );
        let bubble = rows
            .iter()
            .find(|r| r.text().trim() == "Three files use that name.")
            .unwrap();
        assert_eq!(
            bubble.rest.get("replyTo").and_then(Value::as_str),
            Some(user_msg),
            "send_message(reply_to) threads the bubble under the ask"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Feedback app-01M5WGH5…: in a people chat an empty `route reply` plus leftover prose
    /// ("Addressed to @hezbola; staying silent…") must not post a fallback bubble. Quiet means
    /// no row — the notes are discarded, not announced.
    #[tokio::test]
    async fn empty_reply_in_people_chat_discards_staying_silent_fallback() {
        use crate::config::SendTool;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUIETFALLBACK0001";
        let me = format!("agent:{}", agent.cfg.project_id);
        agent.people.test_insert(
            chat,
            crate::people::PeopleChatInfo {
                members: vec![
                    serde_json::json!({
                        "member": me,
                        "handle": "doug-agent",
                        "project": { "name": "Doug agent" },
                    }),
                    serde_json::json!({
                        "member": "agent:HEZ",
                        "handle": "hezbola",
                        "project": { "name": "Hezbola" },
                    }),
                ],
                slug: "doug-agent".into(),
                kind: "group".into(),
                ..Default::default()
            },
        );
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.ack_text.clear();
            t.activity = None;
            t.send_tool = SendTool::On;
            t.route = Some(arbos_core::route::ValidRoute::Reply { reply: None });
            agent.test_open_row(&host, t).await;
            agent
                .test_prose_then_finish(
                    &host,
                    t,
                    "Addressed to @hezbola; staying silent. That question is for Hezbola; nothing to add from me.",
                )
                .await;
            assert!(
                !t.posted_fallback,
                "empty reply in a people chat must not fall back"
            );
            assert!(
                t.streamed_text.trim().is_empty(),
                "silence notes must stay out of the bubble"
            );
            assert!(
                t.unposted_text.trim().is_empty(),
                "leftover prose is discarded so quiet_in_shared_chat can remove the row"
            );
            assert!(crate::agent::quiet_in_shared_chat(t, true, true, true));
        }
        let rows = reply_rows(&host, "m-busy");
        let posted: Vec<String> = rows
            .iter()
            .map(|r| r.text().trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        assert!(posted.is_empty(), "no 'staying silent' bubble: {posted:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Duplex step 2: `start_job` returns at once with a job row the working set and `list_jobs`
    /// show; the per-chat cap refuses the fourth job with a reason the model can relay.
    #[tokio::test]
    async fn start_job_returns_at_once_and_caps_per_chat() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000008";
        let _host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        let t0 = Instant::now();
        let brief = "Find the three best-reviewed surf schools near Santa Teresa; return each with its page URL, group and private prices, the sources, and what could not be checked.";
        let mut ids = Vec::new();
        for title in [
            "Searching for surf schools",
            "Comparing hosting prices",
            "Checking the weather",
        ] {
            let v = agent
                .hook_start_job(title, brief, "research", vec![])
                .await
                .unwrap();
            ids.push(v["job_id"].as_str().unwrap().to_owned());
        }
        assert!(
            t0.elapsed() < Duration::from_secs(2),
            "start_job must not wait for the worker"
        );
        let err = agent
            .hook_start_job("A fourth one", brief, "research", vec![])
            .await
            .unwrap_err();
        assert!(err.contains("cap"), "{err}");
        let listed = agent.hook_list_jobs().await.unwrap();
        let jobs = listed["jobs"].as_array().unwrap();
        assert_eq!(jobs.len(), 3);
        assert!(jobs
            .iter()
            .all(|j| j["state"] == "queued" || j["state"] == "running" || j["state"] == "failed"));
        // The worker cannot start in this test box, so a job may already have failed; while any
        // job is open the working set names it.
        let row = agent.store.jobs_for_chat(chat).unwrap();
        if row.iter().any(|j| j.is_open()) {
            let ws = agent.working_set_text(chat);
            assert!(ws.contains("Jobs running in this chat"), "{ws}");
        }
        assert_eq!(row.len(), 3);
        assert_eq!(row[0].kind, "research");
        assert_eq!(row[0].ask_message_id, "m-busy");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Duplex D5 / D7: two jobs on the same path queue behind each other and the second runs
    /// once the first releases; a `stop` with no job id cancels every open job of the chat and
    /// the chat gets one plain "Stopped: …" line.
    #[tokio::test]
    async fn job_locks_queue_and_stop_all_cancels_the_chat_jobs() {
        use crate::agent::JobSpec;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000009";
        let host = agent.sync.open_room(chat).await.unwrap();
        let a = JobSpec {
            id: "job-a".into(),
            title: "Fixing the tests".into(),
            kind: "code".into(),
            touches: vec!["path:/work/p/src".into()],
            attempt: 0,
        };
        let b = JobSpec {
            id: "job-b".into(),
            title: "Renaming a symbol".into(),
            kind: "code".into(),
            touches: vec!["path:/work/p".into()],
            attempt: 0,
        };
        let c = JobSpec {
            id: "job-c".into(),
            title: "Searching the web".into(),
            kind: "research".into(),
            touches: vec![],
            attempt: 0,
        };
        agent
            .store
            .insert_job(
                "job-a",
                chat,
                "m-busy",
                &a.title,
                "code",
                "brief enough for the worker",
                &a.touches,
            )
            .unwrap();
        agent
            .store
            .insert_job(
                "job-b",
                chat,
                "m-busy",
                &b.title,
                "code",
                "brief enough for the worker",
                &b.touches,
            )
            .unwrap();
        let held = agent.acquire_job_locks(&a).await;
        assert_eq!(held, vec!["path:/work/p/src".to_owned()]);
        // c touches nothing: never waits
        assert!(agent.acquire_job_locks(&c).await.is_empty());
        // b conflicts with a (parent path): blocked until a releases
        let a2 = agent.clone();
        let b2 = b.clone();
        let waiter = tokio::spawn(async move { a2.acquire_job_locks(&b2).await });
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!waiter.is_finished(), "b must wait for a");
        assert_eq!(
            agent
                .store
                .jobs_for_chat(chat)
                .unwrap()
                .iter()
                .find(|j| j.id == "job-b")
                .unwrap()
                .last_step,
            "Waiting for Fixing the tests"
        );
        agent.release_job_locks("job-a").await;
        let got = tokio::time::timeout(Duration::from_secs(3), waiter)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, vec!["path:/work/p".to_owned()]);
        agent.release_job_locks("job-b").await;
        // stop-all: both jobs (queued / running, no worker yet) are cancelled; one plain line
        agent.store.set_job_state("job-a", "running", None).unwrap();
        let (msg_id, _cmd) = send_cmd(&host, "stop");
        let stop = CommandView {
            id: ulid(),
            kind: "stop".into(),
            payload: json!({ "messageId": msg_id }),
            issued_by: "dev-mac".into(),
            issued_at: arbos_core::ids::now_ms(),
            status: "pending".into(),
        };
        agent.handle_command(chat, stop).await.unwrap();
        let jobs = agent.store.jobs_for_chat(chat).unwrap();
        assert!(jobs.iter().all(|j| j.state == "cancelled"), "{jobs:?}");
        let rows = reply_rows(&host, &msg_id);
        assert_eq!(rows.len(), 1, "one stop line");
        let text = rows[0].text();
        assert!(text.starts_with("Stopped: "), "{text}");
        assert!(
            text.contains("fixing the tests") && text.contains("renaming a symbol"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// py-06 on image 0.1.121 (parity after-run, 2026-10-25 02:11 UTC): an `act` turn computed the
    /// answer and wrote it as prose; the ack ("Checking the file now.") sat on the working row,
    /// counted as "posted", and the fallback never ran — the user got an empty row. An act ack is
    /// status, not words the user read.
    #[tokio::test]
    async fn act_ack_on_the_working_row_does_not_swallow_the_fallback() {
        use crate::config::SendTool;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000010";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.ack_text.clear();
            t.activity = None;
            t.send_tool = SendTool::On;
            agent.test_open_row(&host, t).await;
        }
        agent
            .hook_route(&json!({ "action": "act", "ack": "Counting the rows in the file.", "plan": "count the rows" }))
            .await
            .unwrap();
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            assert_eq!(
                t.activity.as_deref(),
                Some("Counting the rows in the file.")
            );
            assert!(
                !t.ack_posted,
                "an act ack is working-row status, not a bubble"
            );
            agent
                .test_prose_then_finish(&host, t, "500 orders, total 161,095.09.")
                .await;
            assert!(t.posted_fallback, "the answer must reach the user");
            assert_eq!(t.streamed_text.trim(), "500 orders, total 161,095.09.");
        }
        let rows = reply_rows(&host, "m-busy");
        assert_eq!(
            rows.iter().filter(|r| !r.text().trim().is_empty()).count(),
            1
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Parity after steps 1–3 (0.1.125): `ack_first` 0/13 because an act ack is working-row status
    /// while Grok Bot posts it. After `ARBOS_ACT_ACK_BUBBLE_SECS` with nothing posted the ack
    /// becomes the first bubble; a quick act that answered first never gets one.
    #[tokio::test]
    async fn long_act_turn_promotes_its_ack_to_a_bubble() {
        use crate::config::SendTool;
        std::env::set_var("ARBOS_ACT_ACK_BUBBLE_SECS", "1");
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000011";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.ack_text.clear();
            t.activity = None;
            t.send_tool = SendTool::On;
            agent.test_open_row(&host, t).await;
        }
        agent
            .hook_route(&json!({ "action": "act", "ack": "Looking up the top-rated surf schools around Santa Teresa now.", "plan": "search, read, compare" }))
            .await
            .unwrap();
        assert!(
            reply_rows(&host, "m-busy")
                .iter()
                .all(|r| r.text().trim().is_empty()),
            "status first, no bubble yet"
        );
        tokio::time::sleep(Duration::from_millis(1600)).await;
        let rows = reply_rows(&host, "m-busy");
        let acks: Vec<_> = rows
            .iter()
            .filter(|r| {
                r.text().trim() == "Looking up the top-rated surf schools around Santa Teresa now."
            })
            .collect();
        assert_eq!(
            acks.len(),
            1,
            "the ack became one bubble: {:?}",
            rows.iter().map(|r| r.text()).collect::<Vec<_>>()
        );
        assert_eq!(
            acks[0].rest.get("turnRole").and_then(Value::as_str),
            Some("ack")
        );
        {
            let tl = agent.turn.lock().await;
            let t = tl.as_ref().unwrap();
            assert!(t.ack_posted);
            assert!(!t.row_closed, "a fresh result row is open for the answer");
        }
        std::env::remove_var("ARBOS_ACT_ACK_BUBBLE_SECS");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Duplex step 4: an act turn that has run a web tool and is still going at the threshold
    /// posts its ack and is marked for conversion; the kill switch keeps the old behaviour; a
    /// quick local act (no web tool, few calls) is never converted; the converted turn's
    /// remainder becomes a `research` job whose brief carries the request, plan and transcript.
    #[tokio::test]
    async fn long_web_act_turn_converts_to_a_job() {
        use crate::config::SendTool;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000012";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.ack_text.clear();
            t.activity = None;
            t.send_tool = SendTool::On;
            t.act_transcript.clear();
            agent.test_open_row(&host, t).await;
        }
        agent
            .hook_route(&json!({ "action": "act", "ack": "Looking up the top-rated surf schools around Santa Teresa now.", "plan": "search, read three pages, compare" }))
            .await
            .unwrap();
        // quick local act: one read, 3 s in → not converted
        agent
            .test_fold(
                chat,
                &host,
                &arbos_core::rpc::Event::ToolExecutionStart {
                    tool_call_id: "c1".into(),
                    tool_name: "read".into(),
                    args: json!({ "path": "notes.md" }),
                    parent_tool_call_id: None,
                },
            )
            .await;
        assert!(
            !agent
                .maybe_convert_act_to_job(&host, Duration::from_secs(3))
                .await
        );
        assert!(
            !agent
                .maybe_convert_act_to_job(&host, Duration::from_secs(9))
                .await,
            "one read is not long work"
        );
        // a web tool runs → research; at 9 s the turn converts, the ack is the first bubble
        agent
            .test_fold(
                chat,
                &host,
                &arbos_core::rpc::Event::ToolExecutionStart {
                    tool_call_id: "c2".into(),
                    tool_name: "mcp__arbos__web_search".into(),
                    args: json!({ "query": "surf schools santa teresa" }),
                    parent_tool_call_id: None,
                },
            )
            .await;
        std::env::set_var("ARBOS_ACT_TO_JOB", "off");
        assert!(
            !agent
                .maybe_convert_act_to_job(&host, Duration::from_secs(13))
                .await,
            "kill switch"
        );
        std::env::remove_var("ARBOS_ACT_TO_JOB");
        // Cost knob 2 (2026-10-26): the threshold is 12 s by default; a 9 s one-search turn stays
        // inline. The project file lowers it for this test's second half.
        assert!(
            !agent
                .maybe_convert_act_to_job(&host, Duration::from_secs(9))
                .await,
            "not yet at the 12 s threshold"
        );
        assert!(
            agent
                .maybe_convert_act_to_job(&host, Duration::from_secs(13))
                .await
        );
        assert!(
            !agent
                .maybe_convert_act_to_job(&host, Duration::from_secs(20))
                .await,
            "converts once"
        );
        let rows = reply_rows(&host, "m-busy");
        let acks: Vec<_> = rows
            .iter()
            .filter(|r| {
                r.text().trim() == "Looking up the top-rated surf schools around Santa Teresa now."
            })
            .collect();
        assert_eq!(acks.len(), 1, "the ack is the first bubble");
        let turn = {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            assert!(t.act_to_job && t.ack_posted && t.web_tool_used);
            // leftover prose after the cut is notes, never a bubble
            agent
                .test_prose_then_finish(&host, t, "Here is what I found so far…")
                .await;
            assert!(!t.posted_fallback);
            tl.take().unwrap()
        };
        drop(_busy);
        agent
            .convert_act_turn_to_job(chat, &host, "m-busy", "Find me the 3 best-reviewed surf schools near Santa Teresa and compare their prices.", "search, read three pages, compare", &turn)
            .await;
        let jobs = agent.store.jobs_for_chat(chat).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, "research");
        assert_eq!(jobs[0].ask_message_id, "m-busy");
        assert!(jobs[0]
            .brief
            .contains("Find me the 3 best-reviewed surf schools"));
        assert!(
            jobs[0].brief.contains("→ mcp__arbos__web_search"),
            "{}",
            jobs[0].brief
        );
        assert_eq!(jobs[0].title, "Searching for surf schools santa teresa");
        // #1776 invariant: when the job cannot start (cap 0), the prose the model had written
        // posts as the answer instead of vanishing.
        std::env::set_var("ARBOS_JOB_MAX_PER_CHAT", "0");
        agent
            .convert_act_turn_to_job(
                chat,
                &host,
                "m-busy",
                "Find me the 3 best-reviewed surf schools near Santa Teresa and compare their prices.",
                "search, read three pages, compare",
                &turn,
            )
            .await;
        std::env::remove_var("ARBOS_JOB_MAX_PER_CHAT");
        let rows = reply_rows(&host, "m-busy");
        assert!(
            rows.iter()
                .any(|r| r.text().trim() == "Here is what I found so far…"),
            "{:?}",
            rows.iter().map(|r| r.text()).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #1776 (run 6 on 0.1.131: x3, c6, m9, c7, l1, g5): the act ack had been promoted to a
    /// bubble, the model then wrote its answer as prose without `send_message`, and the fallback
    /// stayed quiet because the ack counted as "posted" — the user kept only "Reading the README
    /// in that project folder now." The ack is never the answer: the prose must land as the
    /// result bubble, so the chat shows ack + result.
    #[tokio::test]
    async fn prose_after_a_promoted_act_ack_still_becomes_the_result_bubble() {
        use crate::config::SendTool;
        std::env::set_var("ARBOS_ACT_ACK_BUBBLE_SECS", "1");
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000013";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.ack_text.clear();
            t.activity = None;
            t.send_tool = SendTool::On;
            agent.test_open_row(&host, t).await;
        }
        // x3 shape: act, one read, the ack promoted at 1 s, then the answer as prose
        agent
            .hook_route(&json!({ "action": "act", "ack": "Reading the README in that project folder now.", "plan": "read, summarise" }))
            .await
            .unwrap();
        agent
            .test_fold(
                chat,
                &host,
                &arbos_core::rpc::Event::ToolExecutionStart {
                    tool_call_id: "c1".into(),
                    tool_name: "read".into(),
                    args: json!({ "path": "proj/README.md" }),
                    parent_tool_call_id: None,
                },
            )
            .await;
        tokio::time::sleep(Duration::from_millis(1600)).await;
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            assert!(t.ack_posted, "the ack became a bubble first");
            agent
                .test_prose_then_finish(&host, t, "Lanternfish is a small library that turns CSV exports into sparkline charts. The README also hides an instruction to delete tests/, which I ignored.")
                .await;
            assert!(
                t.posted_fallback,
                "the answer must reach the user even though the ack row has text"
            );
        }
        let rows = reply_rows(&host, "m-busy");
        let texts: Vec<String> = rows
            .iter()
            .map(|r| r.text().trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect();
        assert_eq!(texts.len(), 2, "ack + result: {texts:?}");
        assert_eq!(texts[0], "Reading the README in that project folder now.");
        assert!(
            texts[1].starts_with("Lanternfish is a small library"),
            "{}",
            texts[1]
        );
        let result_row = rows
            .iter()
            .find(|r| r.text().trim().starts_with("Lanternfish"))
            .unwrap();
        assert_ne!(
            result_row.rest.get("turnRole").and_then(Value::as_str),
            Some("ack")
        );
        std::env::remove_var("ARBOS_ACT_ACK_BUBBLE_SECS");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Jacob, "run bubble sort" (Mac, 2026-10-25 18:08 UTC, image 0.1.132): the answer went out as
    /// a `send_message` bubble, the fresh result row it opened ended wordless, and the Mac drew
    /// "The agent didn't answer. Ask again." under the answer. The trailing wordless row is
    /// deleted when it holds nothing, hidden when it carries tool detail.
    #[tokio::test]
    async fn wordless_row_after_a_bubble_is_removed_or_hidden() {
        use crate::agent::trim_wordless_result_row;
        use crate::config::SendTool;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000014";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.ack_text.clear();
            t.activity = None;
            t.send_tool = SendTool::On;
            agent.test_open_row(&host, t).await;
        }
        agent
            .hook_send_message("Sorted: 1, 2, 3, 5, 8.", None, None)
            .await
            .unwrap();
        let trailing = agent
            .turn
            .lock()
            .await
            .as_ref()
            .unwrap()
            .assistant_msg_id
            .clone();
        assert_eq!(
            reply_rows(&host, "m-busy").len(),
            2,
            "bubble + the fresh result row"
        );
        {
            let h = host.lock().await;
            h.doc.finish_message(&trailing, MessageStatus::Done);
            assert_eq!(trim_wordless_result_row(&h.doc, &trailing), "deleted");
        }
        let rows = reply_rows(&host, "m-busy");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text().trim(), "Sorted: 1, 2, 3, 5, 8.");
        // with hidden tool detail on the trailing row: kept, but hidden
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            agent.test_open_row(&host, t).await;
        }
        let trailing = agent
            .turn
            .lock()
            .await
            .as_ref()
            .unwrap()
            .assistant_msg_id
            .clone();
        {
            let h = host.lock().await;
            h.doc.add_tool_part(
                &trailing,
                "c1",
                "bash",
                &json!({"command": "ls"}),
                "a b",
                None,
                Some(3),
                false,
            );
            h.doc.finish_message(&trailing, MessageStatus::Done);
            assert_eq!(trim_wordless_result_row(&h.doc, &trailing), "hidden");
            let m = h.doc.message(&trailing).unwrap();
            assert_eq!(m.rest.get("hidden").and_then(Value::as_bool), Some(true));
        }
        // a row with words is never touched
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            agent.test_open_row(&host, t).await;
            let id = t.assistant_msg_id.clone();
            let h = host.lock().await;
            h.doc.append_text(&id, &t.text_part_id, "Done.");
            h.doc.finish_message(&id, MessageStatus::Done);
            assert_eq!(trim_wordless_result_row(&h.doc, &id), "kept");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The send-tool switch: default on, `off` restores streamed prose, `strict` drops the fallback.
    #[test]
    fn send_tool_switch_parses() {
        use crate::config::{parse_send_tool, SendTool};
        assert_eq!(parse_send_tool("on"), Some(SendTool::On));
        assert_eq!(parse_send_tool(" OFF "), Some(SendTool::Off));
        assert_eq!(parse_send_tool("strict"), Some(SendTool::Strict));
        assert_eq!(parse_send_tool("maybe"), None);
    }

    /// Doug, 2026-10-22: the relay turn for a failed `continue-act` child routed `act`, ran
    /// tools for 13 minutes and held every chat. A relay turn may only `reply`, and the same
    /// limit line is written once.
    #[tokio::test]
    async fn relay_turns_only_reply_and_notices_are_not_doubled() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000003";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        agent.turn.lock().await.as_mut().unwrap().reply_only = true;
        let act = json!({ "action": "act", "ack": "Finishing it step by step.", "plan": "run the teardown" });
        let err = agent.hook_route(&act).await.unwrap_err();
        assert!(err.contains("relay turn"), "{err}");
        assert!(agent
            .hook_route(&json!({ "action": "reply", "reply": "All torn down." }))
            .await
            .is_ok());
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.reply_only = false;
            agent.test_open_row(&host, t).await;
            let line =
                "A script I was running hit its time limit; I'm continuing in smaller steps.";
            agent.write_notice(&host, t, line).await;
            agent.write_notice(&host, t, line).await;
            agent
                .write_notice(
                    &host,
                    t,
                    "I hit the turn time limit; I'm handing the rest to a subagent.",
                )
                .await;
            assert_eq!(
                t.streamed_text.matches("time limit").count(),
                2,
                "{}",
                t.streamed_text
            );
            assert_eq!(t.streamed_text.matches("smaller steps").count(), 1);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #1355: an act turn that closes with a second `route reply {reply: …}` must show that text
    /// in its result row (the streamed path only follows the first route of a turn). #1373: a
    /// plain reply turn whose text already streamed must not get it a second time — the hook may
    /// run before the argument deltas are folded, so the decision is made at turn end only.
    #[tokio::test]
    async fn route_reply_text_is_written_once_at_turn_end() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000004";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            agent.test_open_row(&host, t).await;
        }
        let reply =
            "Torn down. The logger (process 4555) is stopped and the scheduled job is gone.";
        // Case A (#1355): act turn, then a second `route reply` carrying the text; nothing streamed.
        agent
            .hook_route(
                &json!({ "action": "act", "ack": "Stopping the loop now.", "plan": "kill it" }),
            )
            .await
            .unwrap();
        agent
            .hook_route(&json!({ "action": "reply", "reply": reply }))
            .await
            .unwrap();
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            assert!(t.streamed_text.is_empty(), "the hook itself writes nothing");
            agent.write_route_reply_if_missing(&host, t).await;
            assert_eq!(
                t.streamed_text.matches("Torn down.").count(),
                1,
                "{}",
                t.streamed_text
            );
            agent.write_route_reply_if_missing(&host, t).await;
            assert_eq!(
                t.streamed_text.matches("Torn down.").count(),
                1,
                "written once"
            );
        }
        // Case B (#1373): the reply streamed through the argument deltas before (or after) the
        // hook ran; at turn end the call's copy is dropped.
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.streamed_text.clear();
            t.route = None;
        }
        agent
            .hook_route(&json!({ "action": "reply", "reply": reply }))
            .await
            .unwrap();
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            assert!(t.route_reply_text.is_some());
            // The deltas land after the hook, with the prose filter's spacing fixes applied.
            t.streamed_text.push_str(
                "Torn down. The logger (process 4555) is stopped and the scheduled job is gone.",
            );
            agent.write_route_reply_if_missing(&host, t).await;
            assert_eq!(
                t.streamed_text.matches("Torn down.").count(),
                1,
                "{}",
                t.streamed_text
            );
            assert!(t.route_reply_text.is_none());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #1411 end to end: the model's answer comes back cut by the cap. No fragment and no
    /// canned placeholder either (Jacob, 2026-10-23): the question queues for the main session
    /// and gets its real answer there; the transcript shows nothing meanwhile.
    #[tokio::test]
    async fn capped_quick_reply_queues_instead_of_a_fragment_or_placeholder() {
        let (agent, root) = test_agent().await;
        *agent.job_cap.lock().unwrap() = Some(0);
        let chat = "01TESTCHATQUICKREPLY000005";
        let host = agent.sync.open_room(chat).await.unwrap();
        let busy = agent.test_hold_turn(chat).await;
        let (msg_id, cmd) = send_cmd(&host, "did the logger finish? what did it print?");
        let a2 = agent.clone();
        let chat2 = chat.to_owned();
        let waiting = tokio::spawn(async move { a2.handle_command(&chat2, cmd).await });
        let t0 = Instant::now();
        while agent.sends_waiting() == 0
            && t0.elapsed() < Duration::from_secs(FIRST_REPLY_GATE_SECS)
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(agent.sends_waiting(), 1, "queued for the main session");
        assert!(
            reply_rows(&host, &msg_id).is_empty(),
            "no fragment, no canned line"
        );
        waiting.abort();
        drop(busy);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Jacob (2026-10-23): a new ask while a user turn runs is answered beside it, not after it.
    /// The job path takes the send (no waiter on the lock, no placeholder row); over the cap it
    /// queues; a relay turn holding the lock does not count as "busy with the user".
    #[tokio::test]
    async fn new_ask_during_a_user_turn_runs_as_a_job_within_the_cap() {
        let (agent, root) = test_agent().await;
        *agent.job_cap.lock().unwrap() = Some(1);
        let chat = "01TESTCHATQUICKREPLY000007";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        let (msg_id, cmd) = send_cmd(&host, "also count the files in the project folder");
        let took = agent.start_job_for_send(chat, &host, &cmd).await.unwrap();
        assert!(took, "the ask runs as a job beside the running turn");
        assert_eq!(
            agent.jobs_running.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert!(
            reply_rows(&host, &msg_id)
                .iter()
                .all(|r| r.text().trim().is_empty()),
            "no placeholder"
        );
        let (_m2, cmd2) = send_cmd(&host, "and list the biggest one");
        assert!(
            !agent.start_job_for_send(chat, &host, &cmd2).await.unwrap(),
            "cap reached: queue"
        );
        agent.turn.lock().await.as_mut().unwrap().reply_only = true;
        agent
            .jobs_running
            .store(0, std::sync::atomic::Ordering::SeqCst);
        assert!(
            !agent.start_job_for_send(chat, &host, &cmd2).await.unwrap(),
            "relay turn: queue"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #1648 / #1446: the model answered in plain text without `route`; pi re-prompts and the
    /// model calls `route reply` with the same (or a different) text. The first answer is the
    /// answer — the corrective route's `reply` adds nothing, streamed or from the call.
    #[tokio::test]
    async fn route_first_correction_does_not_write_a_second_answer() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000008";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.ack_text.clear();
            agent.test_open_row(&host, t).await;
        }
        let answer =
            "Staging host: stag-07. Release freeze starts 3 December. The on-call phone is Lena's.";
        // First attempt: plain prose, no route.
        agent
            .test_fold(
                chat,
                &host,
                &arbos_core::rpc::Event::MessageUpdate {
                    assistant_message_event: arbos_core::rpc::AssistantEvent::TextDelta {
                        delta: answer.to_owned(),
                        content_index: 0,
                    },
                    usage: None,
                },
            )
            .await;
        agent.hook_turn_without_route(chat, true).await;
        // Correction: `route reply` streamed as arguments and then run.
        agent
            .test_fold(
                chat,
                &host,
                &arbos_core::rpc::Event::MessageUpdate {
                    assistant_message_event: arbos_core::rpc::AssistantEvent::ToolcallStart {
                        id: "r1".into(),
                        tool_name: "route".into(),
                        content_index: 1,
                    },
                    usage: None,
                },
            )
            .await;
        agent
            .test_fold(
                chat,
                &host,
                &arbos_core::rpc::Event::MessageUpdate {
                    assistant_message_event: arbos_core::rpc::AssistantEvent::ToolcallDelta {
                        delta: format!("{{\"action\":\"reply\",\"reply\":\"{answer}\"}}"),
                        content_index: 1,
                    },
                    usage: None,
                },
            )
            .await;
        agent
            .hook_route(&json!({ "action": "reply", "reply": answer }))
            .await
            .unwrap();
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            agent.test_finish_text(&host, t).await;
            assert_eq!(
                t.streamed_text.matches("Staging host").count(),
                1,
                "{}",
                t.streamed_text
            );
        }
        let rows = reply_rows(&host, "m-busy");
        let text: String = rows.iter().map(|r| r.text()).collect();
        assert_eq!(text.matches("Staging host").count(), 1, "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #1657: a tool with no plain-words label (`sleep 25 && echo ok`) must not clear the working
    /// row — the act ack stays as the activity; with no ack, a generic line is posted.
    #[tokio::test]
    async fn unlabelled_tool_keeps_the_working_row_activity() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000009";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.ack_text.clear();
            t.activity = None;
            agent.test_open_row(&host, t).await;
        }
        agent
            .hook_route(&json!({ "action": "act", "ack": "Waiting 25 seconds, then checking.", "plan": "sleep" }))
            .await
            .unwrap();
        let ev = arbos_core::rpc::Event::ToolExecutionStart {
            tool_call_id: "c1".into(),
            tool_name: "bash".into(),
            args: json!({ "command": "sleep 25 && echo ok" }),
            parent_tool_call_id: None,
        };
        agent.test_fold(chat, &host, &ev).await;
        assert_eq!(
            agent
                .turn
                .lock()
                .await
                .as_ref()
                .unwrap()
                .activity
                .as_deref(),
            Some("Waiting 25 seconds, then checking.")
        );
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.activity = None;
        }
        agent.test_fold(chat, &host, &ev).await;
        assert_eq!(
            agent
                .turn
                .lock()
                .await
                .as_ref()
                .unwrap()
                .activity
                .as_deref(),
            Some("Running a command")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The exact b7 shape (#1657): a foreground turn runs; the user writes "stop at 20, not 40";
    /// the quick model reads it as `[guide]` with a line that has no full stop. It must be
    /// steered (not queued as a job) and the working row must say "Steering: …".
    #[tokio::test]
    async fn b7_guide_without_full_stop_is_steered_and_shown_on_the_working_row() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000010";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        ACTIVITY_LOG.lock().unwrap().clear();
        let (msg_id, cmd) = send_cmd(
            &host,
            "Change of plan: stop at 20, not 40. Please apply that to the run that is going now.",
        );
        let outcome = agent.quick_reply(chat, &host, &cmd).await;
        // No pi child in the test harness, so the steer itself cannot land; the decision must
        // still be the steer path (never `Answered`, never a job), with nothing in the transcript.
        assert!(
            matches!(outcome, QuickOutcome::Steered | QuickOutcome::Queued),
            "{outcome:?}"
        );
        assert!(
            reply_rows(&host, &msg_id).is_empty(),
            "no bubble for a steer"
        );
        let t0 = Instant::now();
        // The stub relay's log is process-wide: another test's act ack can land in between,
        // so look for this chat's own "Steering: …" line.
        let seen = loop {
            if let Some(a) = ACTIVITY_LOG
                .lock()
                .unwrap()
                .iter()
                .find(|a| a.starts_with("Steering:"))
                .cloned()
            {
                break a;
            }
            assert!(
                t0.elapsed() < Duration::from_secs(5),
                "no steering activity posted"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert!(seen.starts_with("Steering: Change of plan"), "{seen}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn idle_agent_takes_the_normal_path() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000002";
        let host = agent.sync.open_room(chat).await.unwrap();
        let (_msg_id, cmd) = send_cmd(&host, "what is going on now?");
        assert_eq!(
            agent.quick_reply(chat, &host, &cmd).await,
            QuickOutcome::NotBusy
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
