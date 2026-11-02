//! Budget refusals from the relay (402 `daily_cap` / `credits_exhausted`, 503 `read_only`) are
//! not transient: no retry answers until the owner's cap, credits or the kill switch change.
//! Before this, a cap hit looked like any outage — the group gate stayed silent ("every decision
//! provider failed"), explicit mentions got "I couldn't answer that: the model call failed", and
//! every pending message still cost a (refused) model call (2026-10-23 21:xx, Arbonites: the
//! owner's $20/day cap, 6,344 calls, agents mute for half an hour).
//!
//! Now a refusal arms a short **block** per chat: one visible line says whose budget is used up
//! and when it resets; while the block holds, every further send in that chat gets that same
//! line as its own answer (countdown as of now) without a model call (#1649: the second message
//! used to get silence); when it lapses the next send probes normally (one cheap 402 re-arms it;
//! the centred system line repeats at most every `NOTICE_EVERY`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::json;

use arbos_core::ids::{now_ms, ulid};

use crate::agent::Agent;

/// How long sends are answered without a model call after a refusal.
pub const BLOCK_FOR: Duration = Duration::from_secs(120);
/// How often the chat line repeats while refusals continue.
pub const NOTICE_EVERY: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The owner's daily cap; resets at 00:00 UTC.
    DailyCap,
    /// The owner's credits are gone; needs a top-up.
    CreditsExhausted,
    /// The relay's kill switch / global cap: read-only until cleared.
    ReadOnly,
}

impl Refusal {
    /// Classify an error string from `Llm` (`model call failed: HTTP 402 …{"type":"daily_cap"}`),
    /// a decision call, or pi's `errorMessage`.
    pub fn classify(err: &str) -> Option<Self> {
        let l = err.to_ascii_lowercase();
        // Read-only first: its message quotes the global cap ("daily spend cap").
        if l.contains("read_only") || l.contains("read-only mode") {
            return Some(Self::ReadOnly);
        }
        if l.contains("credits_exhausted")
            || l.contains("credits are used up")
            || l.contains("insufficient credits")
            || l.contains("spend_cap")
        {
            return Some(Self::CreditsExhausted);
        }
        if l.contains("daily_cap") || l.contains("daily cap") || l.contains("daily spend cap") {
            return Some(Self::DailyCap);
        }
        None
    }

    /// Whole hours and minutes until the next 00:00 UTC.
    fn until_reset(now_ms: i64) -> (i64, i64) {
        let day = 24 * 60 * 60 * 1000;
        let left = day - now_ms.rem_euclid(day);
        (left / 3_600_000, (left % 3_600_000) / 60_000)
    }

    /// The one line for the chat. `owner` is "Your" / "Jacob's" / "my owner's".
    pub fn chat_line(self, owner: &str) -> String {
        match self {
            Self::DailyCap => {
                let (h, m) = Self::until_reset(now_ms());
                format!(
                    "{owner} daily Arbos budget is used up; it resets at 00:00 UTC (in {h} h {m} min). I'll answer again then."
                )
            }
            Self::CreditsExhausted => {
                format!(
                    "{owner} Arbos credits are used up; I can't answer until they're topped up."
                )
            }
            Self::ReadOnly => {
                "Arbos is in read-only mode right now; I'll answer again when it's lifted."
                    .to_owned()
            }
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::DailyCap => "daily_cap",
            Self::CreditsExhausted => "credits_exhausted",
            Self::ReadOnly => "read_only",
        }
    }
}

#[derive(Debug, Clone)]
struct Block {
    until: Instant,
    last_notice: Option<Instant>,
    refusal: Refusal,
}

/// Per chat: the current block, if any.
#[derive(Default)]
pub struct BudgetState {
    blocks: Mutex<HashMap<String, Block>>,
}

impl BudgetState {
    /// A refusal just happened in `chat`. Returns `true` when the chat line should be written
    /// now (first refusal, or the last notice is older than `NOTICE_EVERY`).
    pub fn note(&self, chat_id: &str, refusal: Refusal) -> bool {
        let mut m = self.blocks.lock().expect("budget");
        let now = Instant::now();
        let b = m.entry(chat_id.to_owned()).or_insert(Block {
            until: now,
            last_notice: None,
            refusal,
        });
        b.until = now + BLOCK_FOR;
        b.refusal = refusal;
        let due = b
            .last_notice
            .map(|t| now.duration_since(t) >= NOTICE_EVERY)
            .unwrap_or(true);
        if due {
            b.last_notice = Some(now);
        }
        due
    }

    /// Is the chat inside a block right now?
    pub fn active(&self, chat_id: &str) -> Option<Refusal> {
        let m = self.blocks.lock().expect("budget");
        m.get(chat_id)
            .filter(|b| Instant::now() < b.until)
            .map(|b| b.refusal)
    }

    /// A model call went through: the budget is back, forget the block.
    pub fn clear(&self, chat_id: &str) {
        self.blocks.lock().expect("budget").remove(chat_id);
    }
}

impl Agent {
    /// "Jacob's" / "my owner's": whose budget this agent spends, as the chat knows them.
    pub fn owner_label(&self, chat_id: &str) -> String {
        self.people
            .info(chat_id)
            .map(|i| i.owner_spoken_possessive())
            .unwrap_or_else(|| "my owner's".to_owned())
    }

    /// The label as the reader of `user_msg_id` should hear it: "Your" when the reader is the
    /// owner (every 1:1 project chat, or the owner asking in a people chat), else the owner's
    /// name (#1649: "my owner's budget" read oddly to the owner).
    pub async fn budget_owner_label(
        &self,
        chat_id: &str,
        host: &crate::sync::SharedHost,
        user_msg_id: &str,
    ) -> String {
        let Some(info) = self.people.info(chat_id) else {
            return "Your".to_owned();
        };
        let Some(msg) = host.lock().await.doc.message(user_msg_id) else {
            return "Your".to_owned();
        };
        // Same author rule as the asker prelude (#1580): an unidentified author is a guest.
        match info.display_author_of(&msg).as_deref() {
            Some(uid) if uid == info.owner_user => "Your".to_owned(),
            _ => info.owner_spoken_possessive(),
        }
    }

    /// A model call was refused for budget reasons in `chat_id`: arm the block and write the
    /// one visible line (a centred system line, like the feedback-issue notice) when due.
    /// `line_already_shown` = the caller put the text in its own bubble (a main turn's error
    /// part), so only the block is armed. Returns the refusal.
    pub async fn note_budget_refusal(
        &self,
        chat_id: &str,
        err: &str,
        line_already_shown: bool,
    ) -> Option<Refusal> {
        let refusal = Refusal::classify(err)?;
        let due = self.budget.note(chat_id, refusal);
        tracing::warn!(
            chat_id,
            code = refusal.code(),
            notice = due && !line_already_shown,
            "budget refusal; sends in this chat are answered without a model call for {}s",
            BLOCK_FOR.as_secs()
        );
        if due && !line_already_shown {
            self.write_budget_line(chat_id, refusal, err).await;
        }
        Some(refusal)
    }

    /// The centred system line that names whose budget ran out and when it resets.
    pub async fn write_budget_line(&self, chat_id: &str, refusal: Refusal, err: &str) {
        let text = refusal.chat_line(&self.owner_label(chat_id));
        if let Some(host) = self.sync.host(chat_id).await {
            let h = host.lock().await;
            h.doc.append_voice_line(
                "system",
                &ulid(),
                &text,
                &self.cfg.device_id,
                &format!("agent:{}", self.cfg.project_id),
                &[("budget", json!(refusal.code()))],
            );
        }
        let _ = self.store.t3_append(
            chat_id,
            None,
            Some(chat_id),
            "budget_notice",
            Some("system"),
            &text,
            Some(&json!({ "code": refusal.code(), "error": err.chars().take(300).collect::<String>() })),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_refusals_are_classified() {
        assert_eq!(
            Refusal::classify(
                r#"model call failed: HTTP 402 Payment Required: {"error":{"message":"daily cap of $20.00 reached ($20.01 spent today); it resets at midnight UTC","type":"daily_cap"}}"#
            ),
            Some(Refusal::DailyCap)
        );
        assert_eq!(
            Refusal::classify(
                "HTTP 402: your credits are used up (balance $0.00); ask for a top-up"
            ),
            Some(Refusal::CreditsExhausted)
        );
        assert_eq!(
            Refusal::classify("HTTP 503: Arbos is in read-only mode: the global daily spend cap"),
            Some(Refusal::ReadOnly)
        );
        assert_eq!(Refusal::classify("HTTP 429 Too Many Requests"), None);
        assert_eq!(Refusal::classify("error sending request"), None);
    }

    #[test]
    fn chat_line_names_owner_and_reset() {
        let line = Refusal::DailyCap.chat_line("Jacob's");
        assert!(
            line.starts_with("Jacob's daily Arbos budget is used up; it resets at 00:00 UTC (in "),
            "{line}"
        );
        assert!(line.ends_with("min). I'll answer again then."), "{line}");
        let (h, m) = Refusal::until_reset(0);
        assert_eq!((h, m), (24, 0));
        let (h, m) = Refusal::until_reset(23 * 3_600_000 + 30 * 60_000);
        assert_eq!((h, m), (0, 30));
    }

    /// #1649: the line speaks to its reader — "Your" for the owner, the owner's name for guests.
    #[test]
    fn chat_line_addresses_the_reader() {
        assert!(Refusal::DailyCap
            .chat_line("Your")
            .starts_with("Your daily Arbos budget is used up"));
        assert!(Refusal::CreditsExhausted
            .chat_line("Jacob's")
            .starts_with("Jacob's Arbos credits are used up"));
    }

    #[test]
    fn block_notices_once_then_holds() {
        let s = BudgetState::default();
        assert!(s.active("c").is_none());
        assert!(
            s.note("c", Refusal::DailyCap),
            "first refusal writes the line"
        );
        assert!(
            !s.note("c", Refusal::DailyCap),
            "the next one inside NOTICE_EVERY does not"
        );
        assert_eq!(s.active("c"), Some(Refusal::DailyCap));
        assert!(s.active("other").is_none());
        s.clear("c");
        assert!(s.active("c").is_none());
        assert!(
            s.note("c", Refusal::CreditsExhausted),
            "a fresh block notices again"
        );
    }
}
