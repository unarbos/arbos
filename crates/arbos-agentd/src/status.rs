//! Quick replies while the agent is busy (Jacob, 2026-10-22: Doug sat on "Working…" for minutes
//! while a long optimisation loop ran; later one stuck relay turn held every chat).
//!
//! The main pi session runs one turn at a time behind `turn_lock`. A user message that arrives
//! while the lock is held used to wait, silent, until that turn ended. Now it gets its own row
//! within milliseconds and a real answer within seconds: one small model call over a snapshot
//! of the running work (what step is running, what it already said, which helpers are up, the
//! open background jobs and the step each is on, the recent turns), answered without the main
//! session. The model tags its answer:
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
    /// Nothing is running — no turn on the lock, no message waiting for it, no open job: the
    /// normal path answers (nothing was written).
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
/// The same cap for an endpoint that will not turn reasoning off (Opus 5.5): the thinking shares
/// the budget, so it has to hold both. A cap is not a target — a model that answers in one
/// sentence still stops there — so the wider room costs no latency, and without it the quick
/// answer comes back cut at the cap, which sends it to the slow path exactly as a failed call did.
pub const QUICK_MAX_TOKENS_WITH_THINKING: u32 = 2048;
/// Model calls per chat per minute before quick replies fall back to a silent queue.
pub const QUICK_CALLS_PER_MINUTE: usize = 6;
/// Characters of the user's message the quick call sees.
const USER_TEXT_MAX: usize = 1200;
/// How long the agent may stay busy before a new message earns a status note. Short turns that
/// free the lock inside this window never get a status row (#1448). Kept under the busy-reply
/// gate (~5 s) so a real status question still answers within seconds. Task acks never become
/// bubbles at all (#1479). An open background job skips the wait: it is not a turn that might
/// free, it is work the user was already told runs on its own.
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
/// (`finish_cleanly`) and must not put a clock on the finish (Jacob, Test5 T7), else it is not
/// posted and the message queues (`Task`, `None`) for the main session, which answers with the
/// step it is on. A `[guide]` is a steer whatever the quick line looks like — the line is never
/// shown, so a missing full stop must not demote the steer to a silent job (#1657). A `[task]`
/// has no line.
pub fn settle_quick(kind: SideKind, text: &str, truncated: bool) -> (SideKind, Option<String>) {
    match kind {
        SideKind::Status => match finish_cleanly(text, truncated) {
            Some(clean) if !crate::agent::promises_a_time(&clean) => {
                (SideKind::Status, Some(clean))
            }
            Some(_) | None => (SideKind::Task, None),
        },
        SideKind::Guide => (SideKind::Guide, None),
        SideKind::Task => (SideKind::Task, None),
    }
}

/// One open background job of this chat (`queued` or `running`), for the quick call. `step` is
/// the job row's own `last_step` — the plain-words line the working row and `list_jobs` already
/// show — so the quick lane reads the job's real progress and not the turn that started it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct JobLine {
    pub title: String,
    /// `queued` (waiting for a slot or a path lock) or `running`.
    pub state: String,
    /// Empty until the worker's first labelled step.
    pub step: String,
    pub elapsed_s: i64,
    /// Seconds since `step` was recorded; `None` while there is no step yet.
    pub step_age_s: Option<i64>,
}

impl JobLine {
    fn line(&self) -> String {
        let step = match (self.step.trim(), self.step_age_s) {
            ("", _) => "no step yet".to_owned(),
            (s, Some(age)) => format!("now: {s} ({age} s ago)"),
            (s, None) => format!("now: {s}"),
        };
        format!(
            "{} — {} for {} s — {step}",
            self.title,
            if self.state == "queued" {
                "waiting to start"
            } else {
                "running"
            },
            self.elapsed_s
        )
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
    /// The open background jobs of this chat, the freshest step first.
    pub jobs: Vec<JobLine>,
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
        if !self.jobs.is_empty() {
            out.push_str(
                "- Background tasks you started for this conversation (each answers on its own when it finishes):\n",
            );
            for j in &self.jobs {
                out.push_str(&format!("    {}\n", j.line()));
            }
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

    /// The open background jobs of `chat_id`, freshest step first. The step comes from the job
    /// row (`jobs.last_step`, written on every labelled step of its worker), which is where the
    /// user-facing progress of a job lives — the `subagents` row the helper lines are built from
    /// carries the internal `step: …` label instead.
    fn open_job_lines(&self, chat_id: &str) -> Vec<JobLine> {
        let mut open: Vec<crate::store::JobRow> = self
            .store
            .jobs_for_chat(chat_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|j| j.is_open())
            .collect();
        open.sort_by_key(|j| std::cmp::Reverse(j.last_step_at.unwrap_or(j.created_at)));
        let now = arbos_core::ids::now_ms();
        open.iter()
            .take(5)
            .map(|j| JobLine {
                title: clip(&j.title, 120),
                state: j.state.clone(),
                step: clip(&crate::store::redact(&j.last_step), 160),
                elapsed_s: j.elapsed_s(),
                step_age_s: j.last_step_at.map(|t| (now - t).max(0) / 1000),
            })
            .collect()
    }

    /// The running work as the quick call sees it; `None` only when nothing is running at all.
    ///
    /// Three things count as running. A turn holds `turn_lock`; a user message waits for it; or
    /// a background job of this chat is open. The third was missing (Jacob, Test5 T7): a job
    /// runs on its own `tokio::spawn`, so the turn that called `start_job` has long dropped the
    /// lock and `sends_waiting()` is 0 — the box did not read as busy, and a status question
    /// asked during a job went to a full main-session turn instead of this lane.
    pub(crate) async fn busy_snapshot(
        &self,
        chat_id: &str,
        host: &SharedHost,
    ) -> Option<BusySnapshot> {
        let lock_busy = self.turn_lock_busy();
        let waiting = self.sends_waiting();
        let jobs = self.open_job_lines(chat_id);
        if !lock_busy && waiting == 0 && jobs.is_empty() {
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
                // No turn on the lock. A job is then the running work, and its own step — not
                // "getting ready" — is what the user is asking about.
                None => {
                    snap.same_chat = true;
                    let lead = jobs
                        .iter()
                        .find(|j| j.state == "running")
                        .or_else(|| jobs.first());
                    if waiting > 0 {
                        snap.step = format!("{waiting} earlier message(s) ahead of this one");
                    } else if let Some(j) = lead {
                        snap.step = "a background task".to_owned();
                        snap.elapsed_s = j.elapsed_s.max(0) as u64;
                        snap.activity =
                            (!j.step.trim().is_empty()).then(|| j.step.trim().to_owned());
                    } else {
                        snap.step = "getting ready".to_owned();
                    }
                }
            }
        }
        snap.jobs = jobs;
        // A pending approval card is not running work: never report the gated tool's label.
        let turn_chat = {
            let tl = self.turn.lock().await;
            tl.as_ref().map(|t| t.chat_id.clone())
        };
        if let Some(tc) = turn_chat {
            if let Some(label) = self.approval_wait_label(&tc) {
                snap.activity = Some(label);
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
        // "As a background job (not inline)" is an instruction about how this ask runs, and the
        // quick lane has no way to carry it out (bug U, `forced_job.rs`). Worse, it can end the
        // ask: a `[status]` reading posts an answer and tells the main session never to answer
        // the message again, and a `[guide]` reading folds the words into the running turn as a
        // steer. Either way nothing ever starts a job. So the message queues without a quick
        // call at all, and `start_job_for_send` makes it the job the user asked for.
        if crate::forced_job::forces_a_job(&user_text) {
            tracing::info!(
                chat_id,
                "the message asks for a background job; the quick lane stands aside"
            );
            return QuickOutcome::Queued;
        }
        // Wait out short turns: if the lock frees (or the queue drains) inside BUSY_GRACE the
        // user is not really waiting — no status note (#1448). Poll the snapshot so a free
        // agent exits early instead of only watching the lock. An open job ends the wait at
        // once: there is no short turn to ride out, the work outlives the turn by construction
        // and the user is already watching it on the working row.
        let t_wait = Instant::now();
        let snap = loop {
            match self.busy_snapshot(chat_id, host).await {
                None => return QuickOutcome::NotBusy,
                Some(s) if t_wait.elapsed() >= BUSY_GRACE || !s.jobs.is_empty() => break s,
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
        // A cheaper quick-reply lane is allowed, but never over the agent's head: with a model
        // picked for this agent, the box override is ignored and the quick reply speaks on that
        // model like the rest of the turn (Jacob, 2026-11-04).
        let cheap = std::env::var("ARBOS_QUICK_REPLY_MODEL")
            .ok()
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty());
        let model = self
            .model_for(crate::settings::ModelRole::QuickReply {
                env_override: cheap.as_deref(),
            })
            .await;
        let mut used_model = false;
        let (kind, text) = if self.quick_call_allowed(chat_id) {
            used_model = true;
            let system = quick_system_prompt(&self.name(), address.as_deref());
            let user = snap.render(&user_text);
            let llm = self.llm.with_purpose("quick-reply");
            // Reasoning off where the endpoint allows it (#1411: thinking counted against
            // `max_tokens` and cut the visible sentence), and on the endpoints that refuse,
            // the larger budget instead — either way one call, and the answer stays on this
            // fast path rather than falling back to a full background job.
            match tokio::time::timeout(
                quick_reply_timeout(),
                llm.complete_visible(
                    &model,
                    &system,
                    &user,
                    QUICK_MAX_TOKENS,
                    QUICK_MAX_TOKENS_WITH_THINKING,
                ),
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
                                "quick reply could not stand as a status line; silent queue"
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
            // The one row that does not wait for the chat's send order (bug O, `send_order.rs`).
            // A status answer is about the agent *now* and is marked `quick`; holding it behind
            // the older messages it is asking about would answer a stale question after the work
            // it asked about had finished, and the gate it must land inside is
            // `FIRST_REPLY_GATE_SECS`, not a turn. Every other row the lanes write — a main-turn
            // reply, a job's answer, a job's failure line — is ordered.
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

    /// Jacob, Test5 T7, both rounds, on agentd 0.1.192: asked "ETA?" the agent answered "10–15
    /// minutes more… a guess" while ≈3.5 min of a 5 min 04 s task remained, and in round 1
    /// "5–10 minutes" while 34 s remained. Nothing in the box derives that figure — how many
    /// more model turns the rest needs is not known until they run — so every one of them is
    /// invented, and the model's own "a guess" label was judged not to help. The owner's rule:
    /// no invented number; an honest range or the current step.
    ///
    /// The property, not the wording: a posted status line carries no fabricated duration, and
    /// an answer that describes progress instead is still posted, so "ETA?" is answered rather
    /// than deflected. A line with a clock is dropped and the message queues for the main
    /// session (the silent-queue path of #1479), which answers with the step it is on.
    #[test]
    fn a_status_answer_describes_the_step_and_invents_no_duration() {
        for promise in [
            // Round 2, as observed.
            "I'm about two thirds of the way through — another 10–15 minutes.",
            // The same figure with the hedge the report judged insufficient.
            "Roughly 10–15 minutes more, though that is a guess.",
            // Round 1, as observed.
            "I've converted three of the five files; 5–10 minutes to go.",
            "About 3 minutes left on the last file.",
            "ETA 10 minutes.",
            "Give me 5-10 minutes and I'll have the summary.",
            "It should be done in about 10 minutes.",
            "I'll have the table ready in 5 minutes.",
            "Two files left to convert, so maybe 10 more minutes until it's done.",
        ] {
            assert_eq!(
                settle_quick(SideKind::Status, promise, false),
                (SideKind::Task, None),
                "a status line put an invented clock on the finish: {promise}"
            );
        }
        // The answers the rule asks for instead — the step, what is done, what is left as work
        // — still post, and so do the spans that are facts: time already spent, the user's own
        // clock, a check-in a routine keeps.
        for fine in [
            "I'm on the third of five model families; the charts and the PDF come after that.",
            "Three of the five files are converted; the fourth is running now.",
            "I've been on this one for about seven minutes.",
            "The benchmark took 12 minutes.",
            "I'll check back in 10 minutes with where it stands.",
            "Your meeting starts in 15 minutes, so I'll keep this short.",
            "Still on it (about a minute in).",
        ] {
            assert_eq!(
                settle_quick(SideKind::Status, fine, false),
                (SideKind::Status, Some(fine.to_owned())),
                "a status line that invents nothing was dropped: {fine}"
            );
        }
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
            jobs: vec![
                JobLine {
                    title: "Searching for surf schools".into(),
                    state: "running".into(),
                    step: "Reading delsoulsurf.com".into(),
                    elapsed_s: 72,
                    step_age_s: Some(14),
                },
                JobLine {
                    title: "Comparing hosting prices".into(),
                    state: "queued".into(),
                    elapsed_s: 5,
                    ..JobLine::default()
                },
            ],
            recent: vec!["user: start the loop".into()],
            steerable: true,
        };
        let r = s.render("what is going on now?");
        assert!(r.contains("Current step: working on a request in this conversation, running for 30 s. Right now: Running a command…."));
        assert!(r.contains("sleep 100"));
        assert!(r.contains(
            "Searching for surf schools — running for 72 s — now: Reading delsoulsurf.com (14 s ago)"
        ));
        assert!(r.contains("Comparing hosting prices — waiting to start for 5 s — no step yet"));
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

    /// Quick calls the Opus-5.5-shaped endpoint refused because reasoning was disabled, and the
    /// ones it then served. A log, not a flag: the memo is proved by what stops arriving.
    pub(crate) static REASONING_REFUSALS: std::sync::Mutex<usize> = std::sync::Mutex::new(0);
    pub(crate) static REASONING_SERVED: std::sync::Mutex<usize> = std::sync::Mutex::new(0);

    /// The user text that makes the stub answer like Opus 5.5: reasoning cannot be turned off.
    pub(crate) const REASONING_MANDATORY_ASK: &str = "what is going on with the mandatory one?";

    /// The user text that makes the stub return a `[status]` tag with no words after it, which
    /// is what `quick reply could not stand as a status line; silent queue … chars 0` is: the
    /// call came back, the tag parsed, and there was no sentence to post. Jacob's Test5 round 11
    /// logged it before every stalled message of `r11burst`.
    pub(crate) const EMPTY_STATUS_ASK: &str = "what is going on with the empty one?";

    /// The unmet acceptance criterion round 12's worker wrote, as it sits in the notes a
    /// job-result call carries (`job_background_claim_tests.rs`, bug AD). A call whose notes hold
    /// it is answered below with the sentence the real talking model wrote from it.
    pub(crate) const AD_UNMET_BACKGROUND_CRITERION: &str =
        "\"criterion\": \"background job\", \"met\": false";

    /// Round 12's final post, as the talking model wrote it: the result, and then the unmet
    /// criterion restated to the user as a description of what happened.
    pub(crate) const AD_MODEL_ANSWER: &str = "All four steps finished: 1,200 rows parsed, 1,196 normalised, and the summary table is written to summary.csv. One thing to flag: the four steps ran in the foreground, not as a background job.";

    async fn mock_quick(axum::Json(body): axum::Json<Value>) -> axum::response::Response {
        use axum::response::IntoResponse;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let user = body["messages"][1]["content"].as_str().unwrap_or("");
        if user.contains(AD_UNMET_BACKGROUND_CRITERION) {
            return axum::Json(json!({
                "choices": [{ "message": { "role": "assistant", "content": AD_MODEL_ANSWER }, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 900, "completion_tokens": 40 }
            }))
            .into_response();
        }
        // Opus 5.5 (Jacob, Test5, 2026-11-04): the endpoint rejects `reasoning {enabled:false}`
        // outright, and serves the answer only when the flag is absent.
        if user.contains(REASONING_MANDATORY_ASK) {
            if body["reasoning"]["enabled"] == json!(false) {
                *REASONING_REFUSALS.lock().unwrap() += 1;
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(json!({ "error": {
                        "message": "Reasoning is mandatory for this endpoint and cannot be disabled.",
                        "code": 400
                    } })),
                )
                    .into_response();
            }
            *REASONING_SERVED.lock().unwrap() += 1;
            return axum::Json(json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "[status]\nThe benchmark is still running, about a minute in; nothing has failed." },
                    "finish_reason": "stop"
                }],
                "usage": { "prompt_tokens": 400, "completion_tokens": 30 }
            }))
            .into_response();
        }
        let (content, finish) = if user.contains(EMPTY_STATUS_ASK) {
            ("[status]\n", "stop")
        } else if user.contains("stop at 20, not 40") {
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
        // The invariant belongs to the quick lane, and the quick lane is the caller whose system
        // prompt asks for the tag. Other callers reach this stub as well — `send_order_tests`
        // drives a real job result, which is a plain `complete` with no `reasoning` key at all —
        // and asserting on those says nothing about #1411.
        if body["messages"][0]["content"]
            .as_str()
            .unwrap_or("")
            .contains("Start your reply with exactly one tag")
        {
            assert_eq!(
                body["reasoning"]["enabled"],
                json!(false),
                "reasoning is off for quick calls"
            );
        }
        axum::Json(json!({
            "choices": [{ "message": { "role": "assistant", "content": content }, "finish_reason": finish }],
            "usage": { "prompt_tokens": 400, "completion_tokens": 30 }
        }))
        .into_response()
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

    /// A test's own `tracing` output, kept so what the box logged can be read back as evidence
    /// instead of taken on trust.
    ///
    /// `#[tokio::test]` is a current-thread runtime on one thread, and `set_default` is per
    /// thread, so the capture covers the lanes one test spawns and no other test's events can
    /// reach it.
    #[derive(Clone, Default)]
    pub(crate) struct Journal(Arc<std::sync::Mutex<Vec<u8>>>);

    pub(crate) struct JournalSink(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for JournalSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Journal {
        type Writer = JournalSink;

        fn make_writer(&'a self) -> Self::Writer {
            JournalSink(self.0.clone())
        }
    }

    impl Journal {
        pub(crate) fn capture(&self) -> tracing::subscriber::DefaultGuard {
            self.capture_at(tracing::Level::WARN)
        }

        pub(crate) fn capture_info(&self) -> tracing::subscriber::DefaultGuard {
            self.capture_at(tracing::Level::INFO)
        }

        fn capture_at(&self, level: tracing::Level) -> tracing::subscriber::DefaultGuard {
            tracing::subscriber::set_default(
                tracing_subscriber::fmt()
                    .with_writer(self.clone())
                    .with_ansi(false)
                    .with_max_level(level)
                    .finish(),
            )
        }

        pub(crate) fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
        }
    }

    /// Same, with a real `pi_bin`, so `start_pi` and its supervisor run as they do on a box.
    ///
    /// Every other test box points `pi_bin` at nothing, which leaves the agent with no main
    /// session and no supervisor at all — bug Y's tests need the production loop itself, so
    /// they give it a scripted child to supervise.
    pub(crate) async fn test_agent_with_pi(pi_bin: &str) -> (Arc<Agent>, std::path::PathBuf) {
        test_agent_full(axum::Router::new(), pi_bin).await
    }

    /// Same, with extra mock-relay routes merged in (`/box/secrets/…` for the hand-off tests).
    pub(crate) async fn test_agent_with(extra: axum::Router) -> (Arc<Agent>, std::path::PathBuf) {
        test_agent_full(extra, "/nonexistent/pi").await
    }

    /// Same, pointed at a relay the caller serves itself. The model-stub tests count the TCP
    /// connections the box opens, which `axum::serve` cannot report.
    pub(crate) async fn test_agent_on_relay(relay_url: &str) -> (Arc<Agent>, std::path::PathBuf) {
        test_agent_on(relay_url.to_owned(), "/nonexistent/pi").await
    }

    async fn test_agent_full(
        extra: axum::Router,
        pi_bin: &str,
    ) -> (Arc<Agent>, std::path::PathBuf) {
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
        test_agent_on(format!("http://127.0.0.1:{port}"), pi_bin).await
    }

    async fn test_agent_on(relay_url: String, pi_bin: &str) -> (Arc<Agent>, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("arbos-quick-{}", ulid()));
        std::fs::create_dir_all(&root).unwrap();
        let bx = crate::config::BoxConfig {
            user_id: Some("u-test".into()),
            relay_url,
            box_token: "test-token".into(),
            mcp_token: "test-mcp".into(),
            work_root: root.clone(),
            bind: "127.0.0.1".into(),
            port: 0,
            local_box: true,
            pi_bin: pi_bin.into(),
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

    /// Shorten the act-ack promotion threshold for one agent only. The knob file lives in this
    /// agent's own state dir, so two promotion tests can run at once; the process-global
    /// `ARBOS_ACT_ACK_BUBBLE_SECS` they used to set could not (either one's cleanup restored the
    /// 4 s default inside the other's sleep and killed the promotion under test).
    fn promote_act_ack_after_1s(agent: &Arc<Agent>) {
        let f = agent
            .cfg
            .state_dir
            .join(crate::config::ACT_ACK_BUBBLE_SECS_FILE);
        std::fs::write(&f, "1").unwrap();
        assert_eq!(agent.cfg.act_ack_bubble_after().as_secs(), 1);
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

    /// Jacob on Test5 (2026-11-04, agentd 0.1.191, chat 01M6M0G9257SCE4RRQDW2GXFS9): the quick
    /// call sends `reasoning {enabled: false}`, and the Opus 5.5 endpoint answers HTTP 400
    /// "Reasoning is mandatory for this endpoint and cannot be disabled". Every side-session
    /// message during a busy turn spent ~3 s failing and then became a full background job —
    /// "hey" took 10.1 s against 2.4 s idle, "what did I say here?" 16.8 s.
    ///
    /// The refusal now costs one retry, once per model: the answer comes back on the quick
    /// path, and the model is remembered so later messages never make the attempt that cannot
    /// work.
    #[tokio::test]
    async fn a_quick_reply_survives_an_endpoint_that_demands_reasoning() {
        *REASONING_REFUSALS.lock().unwrap() = 0;
        *REASONING_SERVED.lock().unwrap() = 0;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000018";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        let (msg_id, cmd) = send_cmd(&host, REASONING_MANDATORY_ASK);

        let t0 = Instant::now();
        agent.handle_command(chat, cmd).await.unwrap();
        let elapsed = t0.elapsed();

        let rows = reply_rows(&host, &msg_id);
        assert_eq!(
            rows.len(),
            1,
            "the refused flag must not send the answer down the background-job path"
        );
        let text = rows[0].text();
        assert!(text.contains("benchmark is still running"), "{text}");
        assert_eq!(rows[0].status, "done");
        assert!(
            elapsed < Duration::from_secs(FIRST_REPLY_GATE_SECS),
            "quick reply took {elapsed:?} with the turn lock held"
        );
        assert_eq!(
            (
                *REASONING_REFUSALS.lock().unwrap(),
                *REASONING_SERVED.lock().unwrap()
            ),
            (1, 1),
            "one refused attempt, then the answer on the retry"
        );
        let model = agent
            .model_for(crate::settings::ModelRole::QuickReply { env_override: None })
            .await;
        assert!(
            agent.llm.reasoning_always_on(&model),
            "the model is remembered, so the next quick reply skips the refused attempt"
        );
        assert!(agent.take_side_notes(chat).is_some());
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
            brief: "Fix the failing tests.".into(),
            touches: vec!["path:/work/p/src".into()],
            attempt: 0,
        };
        let b = JobSpec {
            id: "job-b".into(),
            title: "Renaming a symbol".into(),
            kind: "code".into(),
            brief: "Rename the symbol everywhere.".into(),
            touches: vec!["path:/work/p".into()],
            attempt: 0,
        };
        let c = JobSpec {
            id: "job-c".into(),
            title: "Searching the web".into(),
            kind: "research".into(),
            brief: "Search the web for the answer.".into(),
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

    /// Jacob's Test5 round 2, bug I: a code job that declares no `touches` used to take
    /// `path:/` and the project's git, so every code job queued behind every other and one
    /// stuck job froze the rest. Its default is now the project subfolder its brief names and
    /// that folder's own git — while a job that declares the project folder still serializes
    /// against both subfolders.
    #[tokio::test]
    async fn a_code_job_locks_its_own_subfolder_not_the_whole_project() {
        use crate::agent::JobSpec;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATJOBLOCKSCOPE0001";
        let _host = agent.sync.open_room(chat).await.unwrap();
        // Two repos, one per folder, as a box that cloned two projects has them.
        let project = agent.cfg.work_dir.clone();
        for folder in ["alpha", "beta"] {
            std::fs::create_dir_all(project.join(folder).join(".git")).unwrap();
        }
        let code_job = |id: &str, title: &str, brief: &str| JobSpec {
            id: id.into(),
            title: title.into(),
            kind: "code".into(),
            brief: brief.into(),
            touches: vec![],
            attempt: 0,
        };
        let a = code_job(
            "job-alpha",
            "Fixing the alpha tests",
            "Fix the failing tests in alpha/ and run the suite.",
        );
        let b = code_job(
            "job-beta",
            "Renaming a symbol",
            "Rename the symbol across beta/ and build it.",
        );
        for j in [&a, &b] {
            agent
                .store
                .insert_job(
                    &j.id, chat, "m-busy", &j.title, "code", &j.brief, &j.touches,
                )
                .unwrap();
        }
        let held_a = agent.acquire_job_locks(&a).await;
        // The job in alpha/ never finishes in this test: it is the stuck job.
        let held_b = tokio::time::timeout(Duration::from_millis(500), agent.acquire_job_locks(&b))
            .await
            .expect(
                "a code job in beta/ must start while a code job in alpha/ runs: a code job's \
                 default lock is its own subfolder, not the whole project",
            );
        assert_eq!(
            held_a,
            vec![
                format!("path:{}/alpha", project.display()),
                format!("git:{}/alpha", project.display()),
            ]
        );
        assert_eq!(
            held_b,
            vec![
                format!("path:{}/beta", project.display()),
                format!("git:{}/beta", project.display()),
            ]
        );
        // A brief that names no folder of this project (a fresh clone) still takes the whole
        // project folder and its git, which is what every code job used to take.
        assert_eq!(
            agent.job_lock_keys(&code_job(
                "job-clone",
                "Cloning subtensor",
                "Clone https://github.com/opentensor/subtensor and build it.",
            )),
            vec![format!("path:{}", project.display()), "git".to_owned()]
        );
        // A job that needs the whole project says so, and then it does wait for both.
        let wide = JobSpec {
            touches: vec!["path:/".into(), "git".into()],
            ..code_job("job-wide", "Reformatting everything", "Run the formatter.")
        };
        agent
            .store
            .insert_job(
                "job-wide",
                chat,
                "m-busy",
                &wide.title,
                "code",
                &wide.brief,
                &wide.touches,
            )
            .unwrap();
        let agent2 = agent.clone();
        let wide2 = wide.clone();
        let waiter = tokio::spawn(async move { agent2.acquire_job_locks(&wide2).await });
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            !waiter.is_finished(),
            "a job that declares the project folder still waits for every subfolder in it"
        );
        agent.release_job_locks("job-alpha").await;
        agent.release_job_locks("job-beta").await;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), waiter)
                .await
                .unwrap()
                .unwrap(),
            vec!["path:/".to_owned(), "git".to_owned()]
        );
        agent.release_job_locks("job-wide").await;
        // A project-relative `touches` entry is the same lock as the absolute folder.
        assert_eq!(
            agent.job_lock_keys(&JobSpec {
                touches: vec!["path:alpha".into()],
                ..code_job("job-rel", "Patching alpha", "Patch it.")
            }),
            vec![format!("path:{}/alpha", project.display())]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Bug I, second half: `start_job` used to answer `state: "running"` whenever a worker slot
    /// was free, so the reply read as if the work had begun while the job was really waiting for
    /// another job's files. A blocked job comes back queued and names what it is behind.
    #[tokio::test]
    async fn start_job_says_it_is_queued_behind_the_job_holding_the_files() {
        use crate::agent::JobSpec;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUEUEDBEHIND0001";
        let _host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        let project = agent.cfg.work_dir.clone();
        std::fs::create_dir_all(project.join("alpha").join(".git")).unwrap();
        let holder = JobSpec {
            id: "job-holder".into(),
            title: "Reformatting everything".into(),
            kind: "code".into(),
            brief: "Run the formatter over the project.".into(),
            touches: vec!["path:/".into(), "git".into()],
            attempt: 0,
        };
        agent
            .store
            .insert_job(
                &holder.id,
                chat,
                "m-busy",
                &holder.title,
                "code",
                &holder.brief,
                &holder.touches,
            )
            .unwrap();
        assert!(!agent.acquire_job_locks(&holder).await.is_empty());
        let v = agent
            .hook_start_job(
                "Fixing the alpha tests",
                "Fix the failing parser tests in alpha/, run the suite, and return the diff with the test output.",
                "code",
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(
            v["state"], "queued",
            "a job behind another job's files has not started: {v}"
        );
        assert_eq!(v["queued_behind"]["title"], "Reformatting everything");
        assert_eq!(v["queued_behind"]["job_id"], "job-holder");
        let note = v["note"].as_str().unwrap_or_default();
        assert!(
            note.contains("queued behind \"Reformatting everything\""),
            "the reply must name what the job waits for: {note}"
        );
        agent.release_job_locks("job-holder").await;
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Bug I follow-up, the half the tool result cannot reach. The acknowledgement row the main
    /// agent posts before `start_job` is closed and pushed by then, and the jobs prompt has the
    /// turn end on that call, so a queued job's state can only reach the user if the box says it
    /// itself. T9 in Jacob's Test5 round 2 sat queued ten minutes behind T8 while its own interim
    /// said it was starting a blobless clone.
    #[tokio::test]
    async fn a_queued_job_tells_the_chat_itself() {
        use crate::agent::JobSpec;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUEUEDROW00000001";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        let holder = JobSpec {
            id: "job-holder".into(),
            title: "Building NYC taxi Flask app".into(),
            kind: "code".into(),
            brief: "Build a small Flask app in taxi_app/ serving the stats.".into(),
            touches: vec!["path:/".into(), "git".into()],
            attempt: 0,
        };
        agent
            .store
            .insert_job(
                &holder.id,
                chat,
                "m-busy",
                &holder.title,
                "code",
                &holder.brief,
                &holder.touches,
            )
            .unwrap();
        assert!(!agent.acquire_job_locks(&holder).await.is_empty());
        // The interim the model posts before `start_job`, as the prompt has it: already a closed
        // row, and wrong the moment the job turns out to be queued.
        agent
            .hook_send_message("I'll start with a blobless clone of pandas.", None, None)
            .await
            .unwrap();
        agent
            .hook_start_job(
                "Analyzing pandas repository",
                "Blobless clone pandas, compute commits per month, and write out/pandas_commits.csv.",
                "code",
                vec![],
            )
            .await
            .unwrap();
        let said: Vec<String> = reply_rows(&host, "m-busy")
            .iter()
            .map(|r| r.text().trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect();
        assert!(
            said.iter().any(|t| t
                == "\"Analyzing pandas repository\" is queued behind \"Building NYC taxi Flask \
                    app\" — it starts when that one frees what they both need."),
            "the box must say the job is queued without waiting for the model: {said:?}"
        );
        assert_eq!(
            said.iter().filter(|t| t.contains("queued behind")).count(),
            1,
            "one line, not one per surface: {said:?}"
        );
        agent.release_job_locks("job-holder").await;
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #2770 retires every row a turn opened and left wordless. The queued notice is a row the
    /// box writes during a turn, so it has to be shown not to be swept — otherwise the notice
    /// #2761 exists to post would vanish again, and silently.
    ///
    /// Three grounds, and the test leans on the first two rather than on reading the predicate.
    /// Its id never enters `rows_opened`, because only `open_row` and the turn driver's own
    /// `TurnState` literal push there and `post_chat_line` is neither. It is never wordless,
    /// because its text goes in before it is finished. And there is no window in between: the
    /// row is built under one `host` lock, which is the same lock every caller of
    /// `retire_wordless_rows` holds, so no sweep can observe it half-written.
    #[tokio::test]
    async fn the_queued_notice_survives_the_wordless_sweep() {
        use crate::agent::{retire_wordless_rows, JobSpec};
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUEUEDSWEEP00001";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        let holder = JobSpec {
            id: "job-holder".into(),
            title: "Building NYC taxi Flask app".into(),
            kind: "code".into(),
            brief: "Build a small Flask app in taxi_app/ serving the stats.".into(),
            touches: vec!["path:/".into(), "git".into()],
            attempt: 0,
        };
        agent
            .store
            .insert_job(
                &holder.id,
                chat,
                "m-busy",
                &holder.title,
                "code",
                &holder.brief,
                &holder.touches,
            )
            .unwrap();
        assert!(!agent.acquire_job_locks(&holder).await.is_empty());
        agent
            .hook_start_job(
                "Analyzing pandas repository",
                "Blobless clone pandas and write out/pandas_commits.csv.",
                "code",
                vec![],
            )
            .await
            .unwrap();
        // The notice row, found the way a client would: by the delivery key it carries.
        let notice = {
            let h = host.lock().await;
            h.doc
                .messages()
                .into_iter()
                .find(|m| {
                    m.rest
                        .get("deliveryKey")
                        .and_then(Value::as_str)
                        .is_some_and(|k| k.starts_with("job:") && k.ends_with(":queued"))
                })
                .expect("the queued notice was posted")
        };
        assert!(
            notice.text().contains("is queued behind"),
            "the row carries its words from the start: {:?}",
            notice.text()
        );
        let (rows_opened, trailing) = {
            let tl = agent.turn.lock().await;
            let t = tl.as_ref().unwrap();
            (t.rows_opened.clone(), t.assistant_msg_id.clone())
        };
        // Ground one: the sweep never sees this row, because only `open_row` enrols rows.
        assert!(
            !rows_opened.contains(&notice.id),
            "the notice is not one of the turn's opened rows: {rows_opened:?}"
        );
        // Ground two: handed to the sweep anyway, it is kept — so enrolling it later, by
        // routing the notice through `open_row`, would still not lose it.
        let forced: Vec<String> = vec![notice.id.clone()];
        assert_eq!(
            {
                let h = host.lock().await;
                retire_wordless_rows(&h.doc, &forced, &trailing)
            },
            0,
            "the notice is not wordless, so the sweep leaves it alone"
        );
        {
            let h = host.lock().await;
            let after = h.doc.message(&notice.id).unwrap();
            assert_ne!(
                after.rest.get("hidden").and_then(Value::as_bool),
                Some(true),
                "the notice is still in the chat"
            );
            assert!(after.text().contains("is queued behind"));
        }
        agent.release_job_locks("job-holder").await;
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The other half of the same rule: a job that waits for nothing says nothing, so an ordinary
    /// start is not narrated twice.
    #[tokio::test]
    async fn a_job_that_waits_for_nothing_posts_no_queued_line() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATNOQUEUEDROW00001";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        agent
            .hook_start_job(
                "Charting the zones",
                "Chart the top ten pickup zones from out/nyc_taxi_summary.csv and post it.",
                "code",
                vec![],
            )
            .await
            .unwrap();
        let said: Vec<String> = reply_rows(&host, "m-busy")
            .iter()
            .map(|r| r.text().trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect();
        assert!(
            !said.iter().any(|t| t.contains("queued behind")),
            "nothing held a key, so nothing was queued: {said:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Two code jobs in two folders of *one* repo take turns on that repo's index — `git` writes
    /// `.git/index.lock` whichever subfolder it runs from — while their file work still overlaps.
    /// Only the git key rises to the repo root; the path key stays on the named folder, which is
    /// what keeps the parallelism this default exists to win.
    #[tokio::test]
    async fn one_repo_two_folders_take_turns_on_git_but_not_on_files() {
        use crate::agent::JobSpec;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATONEREPOTWODIRS01";
        let _host = agent.sync.open_room(chat).await.unwrap();
        let project = agent.cfg.work_dir.clone();
        // One repo at the project root, with the two folders a refactor would name.
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::create_dir_all(project.join("tests")).unwrap();
        let code_job = |id: &str, title: &str, brief: &str| JobSpec {
            id: id.into(),
            title: title.into(),
            kind: "code".into(),
            brief: brief.into(),
            touches: vec![],
            attempt: 0,
        };
        let on_src = code_job(
            "job-src",
            "Refactoring the parser",
            "Rework the parser in src/ and commit it.",
        );
        let on_tests = code_job(
            "job-tests",
            "Widening the suite",
            "Add the missing cases in tests/ and commit them.",
        );
        // The git key is the repo root for both; the path keys stay on the two folders.
        assert_eq!(
            agent.job_lock_keys(&on_src),
            vec![
                format!("path:{}/src", project.display()),
                format!("git:{}", project.display()),
            ],
            "only git rises to the repo root"
        );
        assert_eq!(
            agent.job_lock_keys(&on_tests),
            vec![
                format!("path:{}/tests", project.display()),
                format!("git:{}", project.display()),
            ]
        );
        for j in [&on_src, &on_tests] {
            agent
                .store
                .insert_job(
                    &j.id, chat, "m-busy", &j.title, "code", &j.brief, &j.touches,
                )
                .unwrap();
        }
        agent.acquire_job_locks(&on_src).await;
        // Half one: the second job's git work waits, because one index cannot take two.
        let agent2 = agent.clone();
        let waiter = tokio::spawn(async move { agent2.acquire_job_locks(&on_tests).await });
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            !waiter.is_finished(),
            "two jobs must not run git against one index at once"
        );
        // Half two: file work in the other folder is not held up — the path lock never rose, so
        // `tests/` is free while `src/` is taken. A fix that locked the whole project would pass
        // the assertion above and fail this one.
        let files_only = JobSpec {
            touches: vec!["path:tests".into()],
            ..code_job("job-files", "Formatting the suite", "Run the formatter.")
        };
        agent
            .store
            .insert_job(
                &files_only.id,
                chat,
                "m-busy",
                &files_only.title,
                "code",
                &files_only.brief,
                &files_only.touches,
            )
            .unwrap();
        tokio::time::timeout(
            Duration::from_millis(500),
            agent.acquire_job_locks(&files_only),
        )
        .await
        .expect("file work in another folder of the same repo still runs side by side");
        agent.release_job_locks("job-files").await;
        agent.release_job_locks("job-src").await;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), waiter)
                .await
                .unwrap()
                .unwrap(),
            vec![
                format!("path:{}/tests", project.display()),
                format!("git:{}", project.display()),
            ],
            "the waiting job takes its turn once the index is free"
        );
        agent.release_job_locks("job-tests").await;
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Bug I, measured against the round-2 traces. Each brief below carries the folder token its
    /// real brief carried (`t4`/`t5` and `t8`/`t9` in `test5/*.events.json`), and in the run T5
    /// waited 6.5 min behind T4 and T9 waited about 10 min behind T8. None of these folders
    /// exists when its job starts — each job is the thing that creates it — so the pair only runs
    /// side by side when a named-but-absent folder scopes both the path and the git lock.
    #[tokio::test]
    async fn the_round_two_traces_run_side_by_side() {
        use crate::agent::JobSpec;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATTRACEREPLAY00001";
        let _host = agent.sync.open_room(chat).await.unwrap();
        let project = agent.cfg.work_dir.clone();
        // By the time T8 and T9 ran, `out/` was on disk: T4 and T5 had written into it hours
        // earlier. Their own folders — humanize, data, taxi_app — were still absent.
        std::fs::create_dir_all(project.join("out")).unwrap();
        // The run's project folder was a plain workspace, not a repo: every git command in the
        // dumps is run from a subfolder, and the only one at the root is `git clone`. So no repo
        // root sits above these folders, and the four git keys below are each the folder itself.
        // Were the repo lookup to resolve somewhere shared, these assertions would catch it.
        assert!(!project.join(".git").exists());
        let code_job = |id: &str, title: &str, brief: &str| JobSpec {
            id: id.into(),
            title: title.into(),
            kind: "code".into(),
            brief: brief.into(),
            touches: vec![],
            attempt: 0,
        };
        // T4 and T5: the clone target and the download target, neither on disk yet.
        let t4 = code_job(
            "job-t4",
            "Hunting a humanize bug",
            "git clone https://github.com/python-humanize/humanize into humanize/. Write the \
             failing test output to out/humanize_before.txt.",
        );
        let t5 = code_job(
            "job-t5",
            "Analyzing NYC taxi data",
            "Download the parquet files into data/nyc_tlc/ (verify sizes). Save the script to \
             out/nyc_taxi.py.",
        );
        // T8 and T9: the app folder and, for T9, the output folder, since T9 names its clone
        // target as a bare word with no slash.
        let t8 = code_job(
            "job-t8",
            "Building NYC taxi Flask app",
            "Build a small Flask app in taxi_app/ serving the stats already computed in \
             out/nyc_taxi_summary.csv.",
        );
        let t9 = code_job(
            "job-t9",
            "Analyzing pandas repository",
            "Blobless clone: git clone --filter=blob:none https://github.com/pandas-dev/pandas \
             pandas. Commits per month to out/pandas_commits_per_month.csv.",
        );
        for j in [&t4, &t5, &t8, &t9] {
            agent
                .store
                .insert_job(
                    &j.id, chat, "m-busy", &j.title, "code", &j.brief, &j.touches,
                )
                .unwrap();
        }
        assert_eq!(
            agent.job_lock_keys(&t4),
            vec![
                format!("path:{}/humanize", project.display()),
                format!("git:{}/humanize", project.display()),
            ],
            "T4 works in the folder it clones into"
        );
        assert_eq!(
            agent.job_lock_keys(&t5),
            vec![
                format!("path:{}/data", project.display()),
                format!("git:{}/data", project.display()),
            ],
            "T5 works in the folder it downloads into"
        );
        assert_eq!(
            agent.job_lock_keys(&t8),
            vec![
                format!("path:{}/taxi_app", project.display()),
                format!("git:{}/taxi_app", project.display()),
            ]
        );
        assert_eq!(
            agent.job_lock_keys(&t9),
            vec![
                format!("path:{}/out", project.display()),
                format!("git:{}/out", project.display()),
            ],
            "T9 names no folder for its clone, so its first named folder is the output one"
        );
        // T4 is the stalled job: it never releases in this test.
        agent.acquire_job_locks(&t4).await;
        tokio::time::timeout(Duration::from_millis(500), agent.acquire_job_locks(&t5))
            .await
            .expect("T5 waited 6.5 min behind T4 in the run; it must start at once now");
        agent.acquire_job_locks(&t8).await;
        tokio::time::timeout(Duration::from_millis(500), agent.acquire_job_locks(&t9))
            .await
            .expect("T9 waited about 10 min behind T8 in the run; it must start at once now");
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
        let (agent, root) = test_agent().await;
        promote_act_ack_after_1s(&agent);
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

    /// Jacob, Test5, 2026-11-04, T2 (agentd 0.1.192): an act turn searched the web, posted the
    /// whole answer with `send_message` at 13:45:29.439, and ended 1.5 s later — and 67 ms after
    /// that a job started on the same ask. The job's worker re-read the page and the result turn
    /// said "the answer I sent you stands", which cost a second Opus run and told the user
    /// nothing. A turn that has already answered is finishing, not long work to hand off.
    #[tokio::test]
    async fn an_act_turn_that_already_answered_is_not_converted_to_a_job() {
        use crate::config::SendTool;
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
            t.act_transcript.clear();
            agent.test_open_row(&host, t).await;
        }
        agent
            .hook_route(&json!({ "action": "act", "ack": "Checking the python.org downloads page now.", "plan": "open the release page and read the version" }))
            .await
            .unwrap();
        // The web tool runs, so the conversion is armed exactly as it was in T2.
        agent
            .test_fold(
                chat,
                &host,
                &arbos_core::rpc::Event::ToolExecutionStart {
                    tool_call_id: "c1".into(),
                    tool_name: "mcp__arbos__web_fetch".into(),
                    args: json!({ "url": "https://www.python.org/downloads/" }),
                    parent_tool_call_id: None,
                },
            )
            .await;
        // …and then the turn answers.
        agent
            .hook_send_message(
                "The newest Python is 3.15.2, released on 28 October 2026.",
                None,
                None,
            )
            .await
            .unwrap();
        {
            let tl = agent.turn.lock().await;
            let t = tl.as_ref().unwrap();
            assert!(t.web_tool_used, "the conversion trigger is armed");
            assert!(
                t.progress_texts.iter().any(|p| p.contains("3.15.2")),
                "the answer is out: {:?}",
                t.progress_texts
            );
        }
        assert!(
            !agent
                .maybe_convert_act_to_job(&host, Duration::from_secs(13))
                .await,
            "the answer is already in the chat; a job would only contradict it"
        );
        {
            let tl = agent.turn.lock().await;
            assert!(!tl.as_ref().unwrap().act_to_job);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Round 2 of the same stress run: the answer to "make me the CSV" is the CSV. That turn
    /// posts a file and never calls `send_message`, so `progress_texts` is empty while the
    /// user plainly has their answer — and it was still being handed to a job. Whatever shape
    /// the answer took, a turn that delivered one is finishing.
    #[tokio::test]
    async fn an_act_turn_that_delivered_a_file_is_not_converted_to_a_job() {
        use crate::config::SendTool;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000017";
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
            .hook_route(&json!({ "action": "act", "ack": "Pulling the release dates into a CSV now.", "plan": "read the release pages, build the table, post it" }))
            .await
            .unwrap();
        agent
            .test_fold(
                chat,
                &host,
                &arbos_core::rpc::Event::ToolExecutionStart {
                    tool_call_id: "c1".into(),
                    tool_name: "mcp__arbos__web_search".into(),
                    args: json!({ "query": "python release dates" }),
                    parent_tool_call_id: None,
                },
            )
            .await;
        // The file goes into the chat, and that is the whole answer: no bubble follows it.
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.attachments_posted
                .push("01POSTEDCSVROW000000000000".to_owned());
            assert!(t.web_tool_used, "the conversion trigger is armed");
            assert!(
                t.progress_texts.iter().all(|p| p.trim().is_empty()),
                "nothing was said: the file is the answer, which is the case the earlier \
                 progress-text guard missed"
            );
        }
        assert!(
            !agent
                .maybe_convert_act_to_job(&host, Duration::from_secs(13))
                .await,
            "the file is already in the chat; a job would only duplicate it"
        );
        {
            let tl = agent.turn.lock().await;
            assert!(!tl.as_ref().unwrap().act_to_job);
        }
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
        let (agent, root) = test_agent().await;
        promote_act_ack_after_1s(&agent);
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
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Jacob, "run bubble sort" (Mac, 2026-10-25 18:08 UTC, image 0.1.132): the answer went out as
    /// a `send_message` bubble, the fresh result row it opened ended wordless, and the Mac drew
    /// "The agent didn't answer. Ask again." under the answer. The trailing wordless row is
    /// hidden, whether it holds nothing or carries tool detail.
    ///
    /// It used to be deleted when it held nothing. The relay refuses a removal it cannot
    /// attribute to the pusher, and that refusal cost a 10 s link stall (Jacob, Test5,
    /// 2026-11-04); hiding is a plain field write, which the guard allows. Nothing the user
    /// sees changes: the row has no visible content either way, and `arbos-mac`'s
    /// `no_stock_line_when_the_turn_answered_in_another_row` holds the other half — a wordless
    /// row whose turn answered elsewhere draws no row and no "didn't answer" line.
    #[tokio::test]
    async fn wordless_row_after_a_bubble_is_hidden_never_deleted() {
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
            assert_eq!(
                trim_wordless_result_row(&h.doc, &trailing),
                "hidden_empty",
                "an empty trailing row is hidden, not deleted: a deletion the relay cannot \
                 attribute to us is refused and stalls the link"
            );
            // The row is still there — no `removed` op goes into the batch, which is the whole
            // point — and it is marked hidden and still holds nothing a reader can see.
            let m = h.doc.message(&trailing).expect("the row survives the trim");
            assert_eq!(m.rest.get("hidden").and_then(Value::as_bool), Some(true));
            assert!(
                !m.has_visible_content(),
                "nothing on it for the apps to draw"
            );
            // Finished, never left streaming. The iPhone's working line scans `status` parts
            // for `working` / `step`, and that scan is gated on the row still streaming, with
            // a 15-minute freshness window — so a row kept streaming would drive "Working…"
            // on the phone for a quarter of an hour after the turn ended. Deleting the row hid
            // that choice; keeping it makes the status agentd leaves load-bearing.
            assert_eq!(
                m.status.as_str(),
                "done",
                "a hidden row left streaming drives the phone's working line"
            );
        }
        let rows = reply_rows(&host, "m-busy");
        assert_eq!(rows.len(), 2, "the bubble and the hidden row");
        assert_eq!(rows[0].text().trim(), "Sorted: 1, 2, 3, 5, 8.");
        assert!(rows[1].text().trim().is_empty());
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

    /// Test5 round 4 (2026-11-04, agentd 0.1.193): during the thirty-message burst six assistant
    /// rows sat on the relay finished `done` with nothing in them (19:13:55 → 19:14:15). #2749
    /// only judges the row a turn *ends* on, so a row the turn opened and then moved on from —
    /// merged into another, or superseded while the burst queued behind it — was neither filled
    /// nor hidden. Every row the turn opened is swept now, not only the last.
    ///
    /// Each one is retired, never deleted, for the reason in [`crate::agent::retire_row`]: the
    /// relay refuses a removal it cannot attribute to the pusher, and a refused op holds every
    /// op queued behind it, replies included.
    #[tokio::test]
    async fn every_wordless_row_the_turn_left_behind_is_retired() {
        use crate::agent::{retire_wordless_rows, trim_wordless_result_row};
        use crate::config::SendTool;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000024";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.ack_text.clear();
            t.activity = None;
            t.send_tool = SendTool::On;
            // Opened, finished, and walked away from with nothing written into it.
            agent.test_open_row(&host, t).await;
            let empty = t.assistant_msg_id.clone();
            // The same, but carrying tool detail the Details drawer reads and the
            // transcript does not: still nothing the user sees.
            agent.test_open_row(&host, t).await;
            let with_detail = t.assistant_msg_id.clone();
            {
                let h = host.lock().await;
                h.doc.finish_message(&empty, MessageStatus::Done);
                h.doc.add_tool_part(
                    &with_detail,
                    "c1",
                    "bash",
                    &json!({"command": "ls"}),
                    "a b",
                    None,
                    Some(3),
                    false,
                );
                // `with_detail` is deliberately left streaming: the other shape of an
                // abandoned row, which the sweep has to finish as well as hide.
            }
            agent.test_open_row(&host, t).await;
        }
        agent
            .hook_send_message("Sorted: 1, 2, 3, 5, 8.", None, None)
            .await
            .unwrap();
        let (rows_opened, trailing) = {
            let tl = agent.turn.lock().await;
            let t = tl.as_ref().unwrap();
            (t.rows_opened.clone(), t.assistant_msg_id.clone())
        };
        assert_eq!(
            rows_opened.len(),
            4,
            "three opened by hand, one more by the bubble"
        );
        assert_eq!(
            rows_opened.last().map(String::as_str),
            Some(trailing.as_str()),
            "a turn only ever ends on the last row it opened"
        );
        {
            let h = host.lock().await;
            // The round-4 symptom, before the sweep runs: neither row is hidden and neither
            // has anything on it for a client to draw. The first is the shape the report
            // counted — finished `done` and empty.
            for id in &rows_opened[..2] {
                let m = h.doc.message(id).unwrap();
                assert!(m.rest.get("hidden").is_none());
                assert!(!m.has_visible_content());
            }
            assert_eq!(h.doc.message(&rows_opened[0]).unwrap().status, "done");
            assert_eq!(h.doc.message(&rows_opened[1]).unwrap().status, "streaming");
            h.doc.finish_message(&trailing, MessageStatus::Done);
            assert_eq!(trim_wordless_result_row(&h.doc, &trailing), "hidden_empty");
            assert_eq!(
                retire_wordless_rows(&h.doc, &rows_opened, &trailing),
                2,
                "both rows the turn left behind, the empty one and the tool-detail one"
            );
            for id in &rows_opened[..2] {
                let m = h.doc.message(id).expect("the row survives the sweep");
                assert_eq!(
                    m.rest.get("hidden").and_then(Value::as_bool),
                    Some(true),
                    "a row the turn abandoned is hidden"
                );
                assert!(!m.has_visible_content());
                // Finished too, including the one abandoned mid-stream: a hidden row left
                // streaming still feeds the iPhone's working line, which scans `status`
                // parts for `working` / `step` on streaming rows inside a fifteen-minute
                // window.
                assert_eq!(m.status.as_str(), "done");
            }
            let bubble = h.doc.message(&rows_opened[2]).unwrap();
            assert!(
                bubble.rest.get("hidden").is_none(),
                "the row that holds the answer is not swept"
            );
            assert_eq!(bubble.text().trim(), "Sorted: 1, 2, 3, 5, 8.");
            assert_eq!(
                retire_wordless_rows(&h.doc, &rows_opened, &trailing),
                0,
                "a row already hidden is not written a second time"
            );
        }
        let rows = reply_rows(&host, "m-busy");
        assert_eq!(
            rows.len(),
            4,
            "retired, never deleted: a removal the relay cannot attribute to the box is \
             refused `not_yours`, and a refused op holds the whole push queue behind it"
        );
        let visible: Vec<_> = rows
            .iter()
            .filter(|m| m.rest.get("hidden").and_then(Value::as_bool) != Some(true))
            .collect();
        assert_eq!(visible.len(), 1, "the answer, and no empty 'done' rows");
        assert_eq!(visible[0].text().trim(), "Sorted: 1, 2, 3, 5, 8.");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Test5 round 5: the same empty `done` row outside any burst. A request that runs as an
    /// inline subagent leaves the helper's own row holding an empty text part and a `Done`
    /// status — everything it took on the way (the tool parts, the raw `subagent_result`) is
    /// `hidden`, and the answer the user reads is a separate row. Three were visible on the
    /// relay: `01M6MQV1VPE51J4BQKG7FYBER5`, `01M6MR21MV7N4TKN70BSRMJ1QS`,
    /// `01M6MS91NW0FYS9EBNGHV8XVA8`.
    ///
    /// #2770's sweep missed them for one reason only: a helper's row is minted by
    /// `run_subagent`, never by `open_turn_row` or `open_row`, so it never joins
    /// `TurnState::rows_opened` and was never a candidate. The predicate was always right about
    /// this shape, which is why the fix widens where the rule is applied and leaves the rule
    /// alone. The first assertion below is that claim, pinned.
    #[tokio::test]
    async fn an_inline_subagent_row_that_shows_nothing_is_retired() {
        use crate::agent::{retire_if_wordless, wordless_row};
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000026";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        // The helper's row, written exactly as `run_subagent` settles one.
        let clean = "01M6MQV1VPE51J4BQKG7FYBER5";
        let failed = "01M6MR21MV7N4TKN70BSRMJ1QS";
        for (id, with_error) in [(clean, false), (failed, true)] {
            let h = host.lock().await;
            h.doc
                .start_assistant_message(id, "m-busy", &agent.cfg.device_id);
            h.doc.set_message_fields(
                id,
                &[
                    ("subagentId", json!("01TESTSUBAGENT000000000001")),
                    ("subagentName", json!("Pip")),
                    ("turn", json!({ "kind": "spawn" })),
                    ("worker", json!(true)),
                ],
            );
            h.doc.add_tool_part(
                id,
                "c1",
                "bash",
                &json!({"command": "cargo test"}),
                "ok",
                None,
                Some(2),
                false,
            );
            if with_error {
                h.doc
                    .add_error_part(id, "subagent_error", "the worker could not start");
            }
            h.doc.add_hidden_part(
                id,
                "subagent_result",
                "Sorted the list and checked it twice.",
                &json!({ "subagentId": "01TESTSUBAGENT000000000001", "status": "done" }),
            );
            h.doc.add_status_part(
                id,
                if with_error { "failed" } else { "done" },
                if with_error { "Failed" } else { "Done" },
            );
            h.doc.finish_message(id, MessageStatus::Done);
        }
        {
            let h = host.lock().await;
            let m = h.doc.message(clean).unwrap();
            // The round-5 symptom, and the reason #2770 missed it: the predicate already reads
            // this row as wordless, so nothing was wrong with the rule — the row was simply
            // never offered to it.
            assert!(
                wordless_row(&m).0,
                "the predicate already covers this shape"
            );
            assert!(m.rest.get("hidden").is_none());
            assert!(!m.has_visible_content());
            assert_eq!(m.status.as_str(), "done");
        }
        // The answer arrives on its own row, through the quick lane, while the turn lock is held.
        let (quick_ask, cmd) = send_cmd(&host, "what is going on now?");
        agent.handle_command(chat, cmd).await.unwrap();
        let quick = reply_rows(&host, &quick_ask);
        assert_eq!(quick.len(), 1, "one quick row");
        assert!(!quick[0].text().trim().is_empty());
        let quick_id = quick[0].id.clone();
        {
            let h = host.lock().await;
            assert!(
                retire_if_wordless(&h.doc, clean),
                "a helper's row with nothing left on it is retired"
            );
            assert!(
                !retire_if_wordless(&h.doc, failed),
                "a failure keeps its row: the error line is the user's only account of it"
            );
            assert!(
                !retire_if_wordless(&h.doc, &quick_id),
                "the row that holds the answer is never touched"
            );
            assert!(
                !retire_if_wordless(&h.doc, clean),
                "a row already hidden is not written a second time"
            );
            let m = h.doc.message(clean).unwrap();
            assert_eq!(m.rest.get("hidden").and_then(Value::as_bool), Some(true));
            assert_eq!(m.status.as_str(), "done");
            assert!(
                h.doc.message(clean).is_some(),
                "retired, never deleted: a removal the relay cannot attribute to the box is \
                 refused `not_yours`, and a refused op holds the whole push queue behind it"
            );
            assert!(h.doc.message(failed).unwrap().rest.get("hidden").is_none());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Bubble in Arbonites (2026-10-28 21:50 UTC, agentd 0.6.338): `route reply` with a first
    /// line, then `send_message` parts. The first part was appended to the first line's row and
    /// the bubble read "…never became a reply for him.Bob was up. When I messaged him…".
    #[tokio::test]
    async fn send_message_after_route_reply_line_is_its_own_bubble() {
        use crate::config::SendTool;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000015";
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
        let first = "Bob was up. That line never became a reply for him.";
        let part1 = "Bob was up. When I messaged him a minute later he answered right away.";
        let part2 = "Want me to ask Bob what he saw on that message?";
        agent
            .hook_route(&json!({ "action": "reply", "reply": first }))
            .await
            .unwrap();
        agent.hook_send_message(part1, None, None).await.unwrap();
        agent.hook_send_message(part2, None, None).await.unwrap();
        let posted: Vec<String> = reply_rows(&host, "m-busy")
            .iter()
            .map(|r| r.text().trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect();
        assert_eq!(
            posted,
            vec![first.to_owned(), part1.to_owned(), part2.to_owned()],
            "each line is one bubble, none glued onto another"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// @mwbob on a scratch box (2026-10-29 02:43 UTC, agentd 0.1.181): the relay turn for
    /// @mwbubble's answer routed `reply` with the line, the extension said the line "is not the
    /// answer", and the model sent the same line with `send_message`. The group showed it twice.
    #[tokio::test]
    async fn send_message_repeating_the_route_reply_posts_once() {
        use crate::config::SendTool;
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000016";
        let host = agent.sync.open_room(chat).await.unwrap();
        let _busy = agent.test_hold_turn(chat).await;
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.ack_text.clear();
            t.activity = None;
            t.send_tool = SendTool::On;
            t.reply_only = true;
            agent.test_open_row(&host, t).await;
        }
        let line = "mwbubble says dinner after the offsite is booked at Saffron House, at 7 pm. It got that from the Long-task eval peer's message in this chat.";
        let more = "Want me to ask it who else is coming?";
        agent
            .hook_route(&json!({ "action": "reply", "reply": line }))
            .await
            .unwrap();
        agent.hook_send_message(line, None, None).await.unwrap();
        agent
            .hook_send_message(&format!("{line}\n"), None, None)
            .await
            .unwrap();
        agent.hook_send_message(more, None, None).await.unwrap();
        agent.hook_send_message(more, None, None).await.unwrap();
        let posted: Vec<String> = reply_rows(&host, "m-busy")
            .iter()
            .map(|r| r.text().trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect();
        assert_eq!(
            posted,
            vec![line.to_owned(), more.to_owned()],
            "a line already posted in this turn is not posted again"
        );
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

    /// Jacob on Test5 (2026-11-04 13:13:30 CR, agentd 0.1.192): research job
    /// 01M6KZET08ANES5MRRXWBSYJXC ended `done` with the worker's return contract at `partial`
    /// (six sites answered 403) and a CSV in `out/` the worker never posted. The relay turn
    /// called `bash` beside its `route`, which cut the turn, and the user read "The background
    /// work I started did not finish" instead of the 45 rows.
    ///
    /// The same sequence, replayed: `partial` is a finished job, one tool call beside the first
    /// `route` leaves the route its retry, and the relay turn may post the file it is told to.
    #[tokio::test]
    async fn a_partial_job_delivers_its_unposted_artifact_from_the_relay_turn() {
        use crate::agent::{relay_fallback_line, TurnKind};
        use crate::config::SendTool;
        use crate::media::{deliver_file, out_dir};

        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000017";
        let host = agent.sync.open_room(chat).await.unwrap();
        {
            let h = host.lock().await;
            h.doc
                .append_user_message(
                    "m-busy",
                    "compare the shortlisted sites and send me the table",
                    "dev-mac",
                    &arbos_core::doc::UserMessageOpts::default(),
                )
                .unwrap();
        }
        let _busy = agent.test_hold_turn(chat).await;

        // What the worker left behind: the rows on disk, posted nowhere.
        let csv = "site,price,verdict\nalpha.example,120,shortlist\nbeta.example,98,shortlist\n";
        let dir = out_dir(&agent.cfg);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sites.csv"), csv).unwrap();

        let kind = TurnKind::SubagentResult {
            subagent_id: "01TESTSUBAGENT000000000001".into(),
            name: "job:sites".into(),
            job_id: None,
            status: "partial".into(),
            title: "Compare the shortlisted sites".into(),
            result: "Two of the four sites are on the shortlist.".into(),
        };
        let fallback = relay_fallback_line(&kind);
        assert!(
            !fallback.contains("did not finish"),
            "a job that returns `partial` finished: {fallback}"
        );
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.ack_text.clear();
            t.activity = None;
            t.send_tool = SendTool::On;
            t.reply_only = true;
            t.relay_fallback = fallback.clone();
            agent.test_open_row(&host, t).await;
        }

        // The model routes `act` and calls `bash` in the same assistant message.
        let err = agent
            .hook_route(&json!({
                "action": "act",
                "ack": "Pulling the rows together.",
                "plan": "look at the csv first",
            }))
            .await
            .unwrap_err();
        assert!(err.contains("relay turn"), "{err}");
        agent
            .test_fold(
                chat,
                &host,
                &arbos_core::rpc::Event::ToolExecutionStart {
                    tool_call_id: "c1".into(),
                    tool_name: "bash".into(),
                    args: json!({ "command": "wc -l out/sites.csv" }),
                    parent_tool_call_id: None,
                },
            )
            .await;
        {
            let tl = agent.turn.lock().await;
            let t = tl.as_ref().unwrap();
            assert!(
                !t.stopped,
                "a tool call beside the first route must not cost the turn its route retry"
            );
        }

        // The refusal reaches the model, which retries with `reply` and posts the file.
        agent
            .hook_route(&json!({ "action": "reply", "reply": "Here is the comparison." }))
            .await
            .unwrap();
        agent
            .test_fold(
                chat,
                &host,
                &arbos_core::rpc::Event::ToolExecutionStart {
                    tool_call_id: "c2".into(),
                    tool_name: "mcp__arbos__deliver_file".into(),
                    args: json!({ "path": "out/sites.csv" }),
                    parent_tool_call_id: None,
                },
            )
            .await;
        {
            let tl = agent.turn.lock().await;
            let t = tl.as_ref().unwrap();
            assert!(
                !t.stopped,
                "a relay turn may deliver the file it was told to"
            );
        }
        let v = deliver_file(&agent, &json!({ "path": "out/sites.csv" })).await;
        assert!(v.get("isError").is_none(), "{v}");
        let posted: Value =
            serde_json::from_str(v["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(posted["posted"], json!(true));

        let table = "| Site | Price |\n| --- | --- |\n| [alpha](https://alpha.example) | 120 |";
        agent.hook_send_message(table, None, None).await.unwrap();

        // The rows reached the user: the file as its own card, the table as a bubble.
        let card = host
            .lock()
            .await
            .doc
            .message(posted["message_id"].as_str().unwrap())
            .unwrap();
        let attachment = card
            .parts
            .iter()
            .find_map(|p| p.get("attachment").cloned())
            .expect("the card carries the file");
        assert_eq!(attachment["name"], json!("sites.csv"));
        assert_eq!(
            attachment["bytes"],
            json!(csv.len()),
            "the bytes in the chat are the bytes the worker wrote"
        );
        assert_eq!(card.rest["inReplyTo"], json!("m-busy"));
        let said: String = reply_rows(&host, "m-busy")
            .iter()
            .map(|r| r.text())
            .collect();
        assert!(said.contains("alpha"), "the table reached the chat: {said}");
        assert!(
            !said.contains("did not finish"),
            "a partial job never reads as a failure: {said}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Jacob on Test5 (2026-11-04, agentd 0.1.191, chat 01M6M0G9257SCE4RRQDW2GXFS9): a relay
    /// turn delivered its answer as `send_message` parts, was then cut short, and the user read
    /// "The background work I started did not finish" under the answer it had just been given.
    /// The cut only looked at streamed text, which is the one place a relay turn's words are
    /// not.
    ///
    /// Same family as the `partial` bug above — both announce a failure that did not happen —
    /// but a different question: that one asks whether the work finished, this one asks whether
    /// anything reached the user.
    #[tokio::test]
    async fn a_relay_turn_that_already_answered_is_cut_short_without_a_failure_line() {
        use crate::agent::{relay_fallback_line, TurnKind};
        use crate::config::SendTool;

        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000019";
        let host = agent.sync.open_room(chat).await.unwrap();
        {
            let h = host.lock().await;
            h.doc
                .append_user_message(
                    "m-busy",
                    "how did the migration go?",
                    "dev-mac",
                    &arbos_core::doc::UserMessageOpts::default(),
                )
                .unwrap();
        }
        let _busy = agent.test_hold_turn(chat).await;

        // A relay turn whose fallback line is the failure one: the worker really did fail, so
        // the line is the right one to hold ready — it just must not be written over an answer.
        let kind = TurnKind::SubagentResult {
            subagent_id: "01TESTSUBAGENT000000000002".into(),
            name: "job:migration".into(),
            job_id: None,
            status: "failed".into(),
            title: "Migrate the rows".into(),
            result: String::new(),
        };
        let fallback = relay_fallback_line(&kind);
        assert!(fallback.contains("did not finish"), "{fallback}");
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            t.ack_text.clear();
            t.activity = None;
            t.send_tool = SendTool::On;
            t.reply_only = true;
            t.relay_fallback = fallback;
            agent.test_open_row(&host, t).await;
        }

        // The answer goes out as separate messages, never as streamed text.
        agent
            .hook_route(&json!({ "action": "reply", "reply": "" }))
            .await
            .unwrap();
        agent
            .hook_send_message("The migration finished: 45 rows moved.", None, None)
            .await
            .unwrap();
        agent
            .hook_send_message("Want me to run the row-count check next?", None, None)
            .await
            .unwrap();
        {
            let tl = agent.turn.lock().await;
            let t = tl.as_ref().unwrap();
            assert!(
                t.streamed_text.trim().is_empty(),
                "the answer is in the parts, not in streamed text — that is the whole trap"
            );
        }

        // Now the turn is cut short (a user message waiting, the deadline, a tool call).
        {
            let mut tl = agent.turn.lock().await;
            let t = tl.as_mut().unwrap();
            agent
                .test_cut_relay_turn_short(&host, t, "user waiting")
                .await;
            assert!(t.stopped);
        }

        let said: Vec<String> = reply_rows(&host, "m-busy")
            .iter()
            .map(|r| r.text().trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect();
        assert_eq!(
            said,
            vec![
                "The migration finished: 45 rows moved.".to_owned(),
                "Want me to run the row-count check next?".to_owned()
            ],
            "the answer stands alone: no failure line under it"
        );
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
        let mut ticket = agent.send_order.join(chat, &msg_id);
        let took = agent
            .start_job_for_send(chat, &host, &cmd, &mut ticket)
            .await
            .unwrap();
        assert!(took, "the ask runs as a job beside the running turn");
        assert!(
            ticket.is_none(),
            "the job keeps the ask's place in the chat's send order"
        );
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
            !agent
                .start_job_for_send(chat, &host, &cmd2, &mut None)
                .await
                .unwrap(),
            "cap reached: queue"
        );
        agent.turn.lock().await.as_mut().unwrap().reply_only = true;
        agent
            .jobs_running
            .store(0, std::sync::atomic::Ordering::SeqCst);
        assert!(
            !agent
                .start_job_for_send(chat, &host, &cmd2, &mut None)
                .await
                .unwrap(),
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
        agent
            .hook_turn_without_route(chat, true, crate::mcp::SessionRole::Main)
            .await;
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

    /// Test5 round 5, bug R, part 3. While a wait is parked the chat has to say what it is
    /// waiting for, and name it: "Waiting for approval" said that a decision was wanted without
    /// saying which, and the turn-end sweep then cleared even that, leaving the chat looking
    /// idle while it was in fact waiting on the person.
    ///
    /// The tool here is deliberately a one-off name, because the stub relay's activity log is
    /// process-wide and every shell card shares the title "Run a destructive command (…)".
    #[tokio::test]
    async fn a_parked_approval_leaves_the_chat_saying_what_it_waits_for() {
        const TOOL: &str = "publish_parkstatus_probe";
        let want = format!("Waiting for your approval: {TOOL} (tool-send)");
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATPARKSTATUS000000";
        agent.store.upsert_chat(chat, "Main", true, 0).unwrap();
        let host = agent.sync.open_room(chat).await.unwrap();

        let id = {
            let _busy = agent.test_hold_turn(chat).await;
            {
                let mut tl = agent.turn.lock().await;
                let t = tl.as_mut().unwrap();
                agent.test_open_row(&host, t).await;
            }
            let v = agent
                .hook_approval(TOOL, &json!({ "to": "someone" }), chat, "main")
                .await;
            assert_eq!(v["state"], "pending", "{v}");
            // The running turn's own row says it too, in place of "Running a command…".
            let row = agent
                .turn
                .lock()
                .await
                .as_ref()
                .unwrap()
                .assistant_msg_id
                .clone();
            assert_eq!(
                host.lock()
                    .await
                    .doc
                    .message(&row)
                    .unwrap()
                    .rest
                    .get("activity")
                    .and_then(Value::as_str),
                Some(want.as_str()),
                "the working row must name the card, not the gated tool"
            );
            v["approvalId"].as_str().unwrap().to_owned()
        };
        assert_eq!(agent.park_approval(&id).await["parked"], json!(true));

        // The turn ends here, which is where the line used to be dropped. Every presence post is
        // a spawned HTTP call to the in-process stub, so let the earlier ones land before
        // clearing: without that the card's own post arrives after the clear and stands in for
        // the one under test.
        *agent.turn.lock().await = None;
        tokio::time::sleep(Duration::from_millis(300)).await;
        ACTIVITY_LOG.lock().unwrap().clear();
        agent.refresh_job_activity(chat).await;
        let t0 = Instant::now();
        loop {
            if ACTIVITY_LOG.lock().unwrap().contains(&want) {
                break;
            }
            assert!(
                t0.elapsed() < Duration::from_secs(5),
                "the chat went silent while it was waiting on the user; posted {:?}",
                ACTIVITY_LOG.lock().unwrap()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // Answering the card is what clears it.
        agent
            .decide_approval(
                &id,
                arbos_core::approval::ApprovalOutcome::Rejected { reason: None },
                "dev-mac",
            )
            .await
            .unwrap();
        assert_eq!(agent.approval_wait_label(chat), None);
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

    /// Jacob, Test5 T7 (agentd 0.1.192): a research job ran for 5 min 04 s and the "ETA?" asked
    /// 60 s in was answered by a full main-session turn. `busy_snapshot` read `turn_lock` and
    /// the waiting-send counter only, and a job touches neither — `start_job` spawns its worker
    /// and the turn drops the lock — so the box did not read as busy and the lane built for
    /// "how is it going?" was skipped exactly while work was running.
    ///
    /// The property, not the wording: the question is taken by the quick lane, and the job's
    /// own current step (`jobs.last_step`, the line the working row already shows) is in the
    /// snapshot the quick call reads, so the lane has something true to answer with.
    #[tokio::test]
    async fn a_status_question_during_a_background_job_takes_the_quick_path() {
        let (agent, root) = test_agent().await;
        let chat = "01TESTCHATQUICKREPLY000019";
        let host = agent.sync.open_room(chat).await.unwrap();
        agent
            .store
            .insert_job(
                "job-eta",
                chat,
                "m-ask",
                "Searching for surf schools",
                "research",
                "Find the three best-reviewed surf schools near Santa Teresa.",
                &[],
            )
            .unwrap();
        agent
            .store
            .set_job_state("job-eta", "running", None)
            .unwrap();
        agent
            .store
            .set_job_step("job-eta", "Reading delsoulsurf.com")
            .unwrap();

        // Nothing holds the turn lock and nothing waits for it: the job is the only work.
        assert!(!agent.turn_lock_busy());
        assert_eq!(agent.sends_waiting(), 0);
        let snap = agent
            .busy_snapshot(chat, &host)
            .await
            .expect("a running background job reads as busy");
        let job = snap
            .jobs
            .iter()
            .find(|j| j.title == "Searching for surf schools")
            .expect("the running job is in the snapshot");
        assert_eq!(job.state, "running");
        assert_eq!(
            job.step, "Reading delsoulsurf.com",
            "the job's real current step is not reachable from the quick lane"
        );
        assert!(snap.render("ETA?").contains("Reading delsoulsurf.com"));

        let (msg_id, cmd) = send_cmd(&host, "what is going on now?");
        let t0 = Instant::now();
        assert_eq!(
            agent.quick_reply(chat, &host, &cmd).await,
            QuickOutcome::Answered,
            "a status question asked while a background job runs must take the quick-reply lane"
        );
        let elapsed = t0.elapsed();
        assert!(
            elapsed < Duration::from_secs(FIRST_REPLY_GATE_SECS),
            "quick reply took {elapsed:?} with a job running"
        );
        let rows = reply_rows(&host, &msg_id);
        assert_eq!(rows.len(), 1, "one quick row, no main-session row");
        assert_eq!(
            rows[0].rest.get("decision").and_then(Value::as_str),
            Some("quick")
        );
        assert!(
            agent.take_side_notes(chat).is_some(),
            "the main session is told once and never answers it again"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
