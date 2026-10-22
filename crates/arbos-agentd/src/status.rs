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
//! - `[guide]` — guidance for the running work: the sentence is the acknowledgement and the
//!   words go into the running turn as a steer (when a user turn of this chat is running).
//! - `[task]` — anything else: the sentence is the acknowledgement; the message queues for the
//!   main session exactly as before and gets its full answer when the lock frees.
//!
//! Guardrails: one call of at most `QUICK_MAX_TOKENS`, `QUICK_CALLS_PER_MINUTE` model calls per
//! chat (then a canned line, no call), `ARBOS_QUICK_REPLY_TIMEOUT_MS` on the call (then the
//! canned line), tagged `x-arbos-purpose: quick-reply` for spend accounting. Nothing here
//! touches the main session's system prompt (D87) or the #1037 limit announcements.

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
    /// An acknowledgement was written; the message still queues for the main session.
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

/// Output cap of the quick call: a few sentences.
pub const QUICK_MAX_TOKENS: u32 = 220;
/// Model calls per chat per minute before quick replies fall back to the canned line.
pub const QUICK_CALLS_PER_MINUTE: usize = 6;
/// Characters of the user's message the quick call sees.
const USER_TEXT_MAX: usize = 1200;
/// How long a held lock is given to free before a message counts as arriving at a busy agent.
const BUSY_GRACE: Duration = Duration::from_millis(1500);

/// `ARBOS_QUICK_REPLY_TIMEOUT_MS` (default 8000): the call's budget before the canned line.
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
    /// The canned line when no model call is made (rate limit, timeout, error).
    pub fn canned_line(&self) -> String {
        let what = match &self.activity {
            Some(a) if !a.trim().is_empty() => a.trim().trim_end_matches('…').to_owned(),
            _ if self.same_chat => "working on your last request".to_owned(),
            _ => "finishing something in another conversation".to_owned(),
        };
        let since = if self.elapsed_s >= 120 {
            format!("{} min in", self.elapsed_s / 60)
        } else {
            format!("{} s in", self.elapsed_s)
        };
        format!("Still on it: {what}, {since}. I'll pick this up right after.")
    }

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
After [guide] or [task] write one sentence that says what you understood and that you will take it up right after the current step; do not do it here and do not say it is done.{}",
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
        // A lock about to free (a short reply turn, the setup of the previous message) is not
        // "busy": give it a moment, so two quick messages in a row do not earn a canned ack.
        let t_wait = Instant::now();
        while self.turn_lock_busy() && t_wait.elapsed() < BUSY_GRACE {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let Some(snap) = self.busy_snapshot(chat_id, host).await else {
            return QuickOutcome::NotBusy;
        };
        let t0 = Instant::now();
        // The row first: the user sees the message was taken within milliseconds.
        let row_id = ulid();
        {
            let h = host.lock().await;
            h.doc
                .start_assistant_message(&row_id, &user_msg_id, &self.cfg.device_id);
            h.doc.add_status_part(&row_id, "working", "Working…");
            h.doc.set_message_fields(&row_id, &[("quick", json!(true))]);
            if let Some(attr) = self.people_reply_fields(chat_id) {
                h.doc.set_message_fields(&row_id, &attr.fields);
            }
            h.doc.set_activity(&row_id, Some("Checking on it…"));
        }
        let address = self.owner_address_note(chat_id, host, &user_msg_id).await;
        let (talking_model, _) = self.live_models().await;
        let model = std::env::var("ARBOS_QUICK_REPLY_MODEL")
            .ok()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or(talking_model);
        let mut used_model = false;
        let (kind, text) = if self.quick_call_allowed(chat_id) {
            used_model = true;
            let system = quick_system_prompt(&self.cfg.project_name, address.as_deref());
            let user = snap.render(&user_text);
            let llm = self.llm.with_purpose("quick-reply");
            match tokio::time::timeout(
                quick_reply_timeout(),
                llm.complete(&model, &system, &user, QUICK_MAX_TOKENS),
            )
            .await
            {
                Ok(Ok((answer, ms))) => {
                    let (kind, text) = parse_tagged_reply(&answer);
                    tracing::info!(
                        chat_id,
                        kind = kind.as_str(),
                        ms,
                        "quick reply from the model"
                    );
                    if text.trim().is_empty() {
                        (SideKind::Task, snap.canned_line())
                    } else {
                        (kind, text)
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!(chat_id, error = %e, "quick reply call failed; canned line");
                    (SideKind::Task, snap.canned_line())
                }
                Err(_) => {
                    tracing::warn!(chat_id, "quick reply call timed out; canned line");
                    (SideKind::Task, snap.canned_line())
                }
            }
        } else {
            tracing::info!(chat_id, "quick reply rate limit; canned line");
            (SideKind::Task, snap.canned_line())
        };
        // Guidance needs a user turn of this chat to steer; otherwise it queues like a task.
        let kind = if kind == SideKind::Guide && !snap.steerable {
            SideKind::Task
        } else {
            kind
        };
        let text = crate::store::redact(text.trim());
        {
            let h = host.lock().await;
            if let Some(p) = h.doc.add_text_part(&row_id, "text") {
                h.doc.append_text(&row_id, &p, &text);
            }
            h.doc.set_activity(&row_id, None);
            h.doc.set_message_fields(
                &row_id,
                &[
                    (
                        "turnRole",
                        json!(if kind == SideKind::Status {
                            "result"
                        } else {
                            "ack"
                        }),
                    ),
                    ("decision", json!("quick")),
                    (
                        "turn",
                        json!({ "kind": "quick", "side": kind.as_str(), "model_call": used_model }),
                    ),
                    ("model", json!(model)),
                ],
            );
            h.doc.finish_message(&row_id, MessageStatus::Done);
        }
        tracing::info!(
            chat_id,
            kind = kind.as_str(),
            ms = t0.elapsed().as_millis() as i64,
            model_call = used_model,
            "quick reply written"
        );
        self.announce_reply(&room, &row_id, &text);
        let _ = self.store.t3_append(
            chat_id,
            None,
            Some(chat_id),
            "assistant",
            Some("assistant"),
            &text,
            Some(&json!({ "message_id": row_id, "quick": true, "side": kind.as_str() })),
        );
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
                    Ok(()) => {
                        self.push_side_note(chat_id, &user_text, &text);
                        QuickOutcome::Steered
                    }
                    Err(e) => {
                        tracing::warn!(chat_id, error = %e, "quick guide steer failed; queuing");
                        QuickOutcome::Queued
                    }
                }
            }
            SideKind::Task => QuickOutcome::Queued,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn canned_line_names_the_activity_or_the_other_chat() {
        let s = BusySnapshot {
            same_chat: true,
            activity: Some("Running a command…".into()),
            elapsed_s: 42,
            ..Default::default()
        };
        assert_eq!(
            s.canned_line(),
            "Still on it: Running a command, 42 s in. I'll pick this up right after."
        );
        let s = BusySnapshot {
            same_chat: false,
            elapsed_s: 150,
            ..Default::default()
        };
        assert!(s
            .canned_line()
            .starts_with("Still on it: finishing something in another conversation, 2 min in."));
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
        let content = if user.contains("what is going on") {
            "[status]\nThe benchmark is still running, about a minute in; nothing has failed."
        } else {
            "[task]\nNoted; I'll add that right after the current step."
        };
        axum::Json(json!({
            "choices": [{ "message": { "role": "assistant", "content": content } }],
            "usage": { "prompt_tokens": 400, "completion_tokens": 30 }
        }))
    }

    async fn test_agent() -> (Arc<Agent>, std::path::PathBuf) {
        let app = axum::Router::new().route(
            "/proxy/v1/chat/completions",
            axum::routing::post(mock_quick),
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

    #[tokio::test]
    async fn busy_agent_acknowledges_a_new_task_and_still_queues_it() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000001";
        let host = agent.sync.open_room(chat).await.unwrap();
        let busy = agent.test_hold_turn(chat).await;
        let (msg_id, cmd) = send_cmd(&host, "also add a plot of the results when you are done");
        let t0 = Instant::now();
        let a2 = agent.clone();
        let chat2 = chat.to_owned();
        let waiting = tokio::spawn(async move { a2.handle_command(&chat2, cmd).await });
        // The acknowledgement lands while the lock is still held …
        let ack = loop {
            let rows = reply_rows(&host, &msg_id);
            if let Some(r) = rows.iter().find(|r| !r.text().trim().is_empty()) {
                break r.text();
            }
            assert!(
                t0.elapsed() < Duration::from_secs(FIRST_REPLY_GATE_SECS),
                "no acknowledgement within the gate"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert!(ack.contains("Noted"), "{ack}");
        // … and the message itself is still queued for the main session (one waiter).
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(agent.sends_waiting(), 1);
        assert!(!waiting.is_finished());
        assert!(
            agent.take_side_notes(chat).is_none(),
            "a task is not a side note"
        );
        waiting.abort();
        drop(busy);
        let _ = std::fs::remove_dir_all(&root);
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
