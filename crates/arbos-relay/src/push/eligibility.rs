//! Who hears about a message. One pure decision ([`decide`]) over facts the caller loads
//! ([`Recipient::load`]), checked in this order — the first rule that says no wins:
//!
//! 1. the sender's own account never hears its own message;
//! 2. **blocked sender** — a recipient who blocked the (human) sender hears nothing. In a group
//!    that hides the row for the blocker alone (everyone else is unaffected); in a DM the two
//!    cannot post to each other at all, so this is belt and braces;
//! 3. **archived** — a chat the recipient archived is hidden on purpose;
//! 4. **notification level per chat** — `all` / `mentions` / `none` / `inherit`. `inherit`
//!    (the default) takes the user's account-wide default (`all` unless they set one); the
//!    old `muted` flag on a people-chat membership counts as `none`;
//! 5. **large groups** — a people group with at least [`LARGE_GROUP_MIN`] humans whose level
//!    is only inherited (nobody chose `all` for it) notifies on mentions alone.
//!
//! Agent messages and group messages run through the same rules as a person's DM: there is
//! no path that skips them.

use std::sync::OnceLock;

use anyhow::Result;
use rusqlite::{params, OptionalExtension};

use arbos_core::ids::now_ms;

use crate::db::Db;
use crate::notify::{ChatRef, Sender};
use crate::state::AppState;

pub const LEVEL_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS chat_notify_level (
  user_id TEXT NOT NULL, chat_id TEXT NOT NULL, level TEXT NOT NULL, at INTEGER NOT NULL,
  PRIMARY KEY (user_id, chat_id)
);
CREATE TABLE IF NOT EXISTS user_notify_default (
  user_id TEXT PRIMARY KEY, level TEXT NOT NULL, at INTEGER NOT NULL
);
"#;

/// A people group with this many humans (or more) defaults to mentions-only.
pub const LARGE_GROUP_MIN: usize = 25;

/// `PUSH_LARGE_GROUP_MIN` overrides the threshold (read once).
pub fn large_group_min() -> usize {
    static MIN: OnceLock<usize> = OnceLock::new();
    *MIN.get_or_init(|| {
        std::env::var("PUSH_LARGE_GROUP_MIN")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .filter(|n| *n >= 2)
            .unwrap_or(LARGE_GROUP_MIN)
    })
}

/// What a user asked for in one chat (or as their account default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    All,
    Mentions,
    None,
    /// Use the account default (chat setting only).
    Inherit,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match s.trim().to_ascii_lowercase().as_str() {
            "all" => Some(Level::All),
            "mentions" | "mentions_only" => Some(Level::Mentions),
            "none" | "off" | "muted" => Some(Level::None),
            "inherit" | "default" | "" => Some(Level::Inherit),
            _ => Option::None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::All => "all",
            Level::Mentions => "mentions",
            Level::None => "none",
            Level::Inherit => "inherit",
        }
    }
}

/// The room's shape, as far as the rules care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatShape {
    Dm,
    /// A people group; `humans` = members who are people (agents do not count).
    Group {
        humans: usize,
    },
    /// An agent's project chat; `shared` when guests are in the project.
    ProjectChat {
        shared: bool,
    },
}

/// Everything [`decide`] needs about one recipient of one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    pub user_id: String,
    pub is_sender: bool,
    pub blocks_sender: bool,
    pub archived: bool,
    /// The chat-level setting (`Inherit` when none).
    pub level: Level,
    /// Old `people_members.muted` flag — the apps' mute toggle until they learn levels.
    pub legacy_muted: bool,
    /// Account default; never `Inherit`.
    pub default_level: Level,
    pub mentioned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    Sender,
    BlockedSender,
    Archived,
    LevelNone,
    NotMentioned,
    LargeGroupNotMentioned,
}

impl Skip {
    pub fn label(self) -> &'static str {
        match self {
            Skip::Sender => "sender",
            Skip::BlockedSender => "blocked_sender",
            Skip::Archived => "archived",
            Skip::LevelNone => "level_none",
            Skip::NotMentioned => "not_mentioned",
            Skip::LargeGroupNotMentioned => "large_group_not_mentioned",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Push,
    Skip(Skip),
}

/// The level in force for this recipient in this chat, and whether it was only inherited.
pub fn effective_level(r: &Recipient, shape: ChatShape, large_group_min: usize) -> (Level, bool) {
    let (level, inherited) = match (r.level, r.legacy_muted) {
        (Level::Inherit, true) => (Level::None, false),
        (Level::Inherit, false) => (r.default_level, true),
        (explicit, _) => (explicit, false),
    };
    let large = matches!(shape, ChatShape::Group { humans } if humans >= large_group_min);
    if inherited && large && level == Level::All {
        return (Level::Mentions, true);
    }
    (level, inherited)
}

pub fn decide(r: &Recipient, shape: ChatShape, large_group_min: usize) -> Verdict {
    if r.is_sender {
        return Verdict::Skip(Skip::Sender);
    }
    if r.blocks_sender {
        return Verdict::Skip(Skip::BlockedSender);
    }
    if r.archived {
        return Verdict::Skip(Skip::Archived);
    }
    let (level, inherited) = effective_level(r, shape, large_group_min);
    match level {
        Level::None => Verdict::Skip(Skip::LevelNone),
        Level::All | Level::Inherit => Verdict::Push,
        Level::Mentions if r.mentioned => Verdict::Push,
        Level::Mentions => {
            let large = matches!(shape, ChatShape::Group { humans } if humans >= large_group_min);
            if inherited && large {
                Verdict::Skip(Skip::LargeGroupNotMentioned)
            } else {
                Verdict::Skip(Skip::NotMentioned)
            }
        }
    }
}

/// Does the message mention this person? `mentions` is what the writer declared
/// (`user:<id>`, `<id>`, `@handle` or `handle`); the text is scanned for `@handle` as a word,
/// for `@everyone`, and for `@here`.
///
/// The text scan is what answers in practice, because no app fills the declared list in on the
/// announce: it resolves mentions into the row it writes and posts `…/notify` with the message
/// id and the text alone. `@here` has to be read out of the prose for the same reason, and it
/// has to be read here: the apps turn it into member ids on the row, which this path never sees.
/// `@agents` is deliberately absent — it names the chat's agents, so it mentions no person
/// (`expand_collectives` in the Mac app adds the people for `@here` and not for `@agents`).
pub fn mentioned(text: &str, mentions: &[String], user_id: &str, handle: &str) -> bool {
    let handle = handle.trim().trim_start_matches('@');
    let declared = mentions.iter().any(|m| {
        let m = m.trim();
        m == user_id
            || m.strip_prefix("user:") == Some(user_id)
            || (!handle.is_empty() && m.trim_start_matches('@').eq_ignore_ascii_case(handle))
    });
    if declared {
        return true;
    }
    let lower = text.to_lowercase();
    if word_at(&lower, "@everyone")
        || crate::people::at_token(&lower, crate::people::COLLECTIVE_HERE)
    {
        return true;
    }
    !handle.is_empty() && word_at(&lower, &format!("@{}", handle.to_lowercase()))
}

/// `needle` appears in `hay` followed by a non-handle character (or the end).
fn word_at(hay: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(i) = hay[from..].find(needle) {
        let end = from + i + needle.len();
        let boundary = hay[end..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-'));
        if boundary {
            return true;
        }
        from = end;
    }
    false
}

/// The shape of a chat: DM, people group with its human count, or project chat.
pub fn chat_shape(state: &AppState, chat: &ChatRef) -> ChatShape {
    match &chat.project_id {
        Some(pid) => ChatShape::ProjectChat {
            shared: state
                .db
                .project_members(pid)
                .map(|m| !m.is_empty())
                .unwrap_or(false),
        },
        None => {
            let kind = state
                .db
                .people_chat(&chat.chat_id)
                .ok()
                .flatten()
                .map(|c| c.kind)
                .unwrap_or_default();
            if kind == "dm" {
                ChatShape::Dm
            } else {
                let humans = state
                    .db
                    .members_of(&chat.chat_id)
                    .unwrap_or_default()
                    .iter()
                    .filter(|m| m.user_id().is_some())
                    .count();
                ChatShape::Group { humans }
            }
        }
    }
}

impl Recipient {
    /// Load the facts for `user_id` as a recipient of `sender`'s message in `chat`.
    pub fn load(
        state: &AppState,
        chat: &ChatRef,
        sender: &Sender,
        user_id: &str,
        text: &str,
        mentions: &[String],
    ) -> Self {
        let is_sender = sender.user_id.as_deref() == Some(user_id);
        let blocks_sender = sender
            .user_id
            .as_deref()
            .is_some_and(|from| state.db.blocks(user_id, from).unwrap_or(false));
        let archived = state
            .db
            .chat_archived_at(user_id, &chat.chat_id)
            .ok()
            .flatten()
            .is_some();
        let legacy_muted = chat.project_id.is_none()
            && state
                .db
                .members_of(&chat.chat_id)
                .unwrap_or_default()
                .iter()
                .any(|m| m.user_id() == Some(user_id) && m.muted);
        let handle = state
            .db
            .user_card(user_id)
            .ok()
            .flatten()
            .map(|c| c.handle)
            .unwrap_or_default();
        Self {
            user_id: user_id.to_owned(),
            is_sender,
            blocks_sender,
            archived,
            level: state
                .db
                .chat_notify_level(user_id, &chat.chat_id)
                .unwrap_or(Level::Inherit),
            legacy_muted,
            default_level: state.db.user_notify_default(user_id).unwrap_or(Level::All),
            mentioned: mentioned(text, mentions, user_id, &handle),
        }
    }
}

/// Is this chat silent for the user by level alone (no message in hand, so mentions cannot
/// be judged)? The in-app event path (`people::on_room_push`) asks this.
pub fn silenced_by_level(state: &AppState, user_id: &str, chat: &ChatRef) -> bool {
    let r = Recipient {
        user_id: user_id.to_owned(),
        is_sender: false,
        blocks_sender: false,
        archived: false,
        level: state
            .db
            .chat_notify_level(user_id, &chat.chat_id)
            .unwrap_or(Level::Inherit),
        legacy_muted: false,
        default_level: state.db.user_notify_default(user_id).unwrap_or(Level::All),
        mentioned: false,
    };
    effective_level(&r, chat_shape(state, chat), large_group_min()).0 == Level::None
}

impl Db {
    /// The chat-level setting; `Inherit` when the user never set one.
    pub fn chat_notify_level(&self, user_id: &str, chat_id: &str) -> Result<Level> {
        self.with(|c| {
            c.query_row(
                "SELECT level FROM chat_notify_level WHERE user_id = ?1 AND chat_id = ?2",
                params![user_id, chat_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map(|o| {
                o.as_deref()
                    .and_then(Level::parse)
                    .unwrap_or(Level::Inherit)
            })
        })
    }

    /// `Inherit` removes the row (back to the account default).
    pub fn set_chat_notify_level(&self, user_id: &str, chat_id: &str, level: Level) -> Result<()> {
        self.with(|c| {
            if level == Level::Inherit {
                c.execute(
                    "DELETE FROM chat_notify_level WHERE user_id = ?1 AND chat_id = ?2",
                    params![user_id, chat_id],
                )?;
            } else {
                c.execute(
                    "INSERT INTO chat_notify_level(user_id, chat_id, level, at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(user_id, chat_id) DO UPDATE SET level = excluded.level, at = excluded.at",
                    params![user_id, chat_id, level.as_str(), now_ms()],
                )?;
            }
            Ok(())
        })
    }

    /// The account default; `All` when never set. Never `Inherit`.
    pub fn user_notify_default(&self, user_id: &str) -> Result<Level> {
        self.with(|c| {
            c.query_row(
                "SELECT level FROM user_notify_default WHERE user_id = ?1",
                params![user_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map(|o| match o.as_deref().and_then(Level::parse) {
                Some(Level::Inherit) | Option::None => Level::All,
                Some(l) => l,
            })
        })
    }

    pub fn set_user_notify_default(&self, user_id: &str, level: Level) -> Result<()> {
        let level = if level == Level::Inherit {
            Level::All
        } else {
            level
        };
        self.with(|c| {
            c.execute(
                "INSERT INTO user_notify_default(user_id, level, at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(user_id) DO UPDATE SET level = excluded.level, at = excluded.at",
                params![user_id, level.as_str(), now_ms()],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    //! The eligibility matrix, rule by rule, on the pure decision.

    use super::*;

    fn r() -> Recipient {
        Recipient {
            user_id: "u".into(),
            is_sender: false,
            blocks_sender: false,
            archived: false,
            level: Level::Inherit,
            legacy_muted: false,
            default_level: Level::All,
            mentioned: false,
        }
    }

    const SMALL: ChatShape = ChatShape::Group { humans: 4 };
    const LARGE: ChatShape = ChatShape::Group { humans: 40 };

    #[test]
    fn the_order_is_sender_blocked_archived_level_large_group() {
        let mut x = r();
        x.is_sender = true;
        x.blocks_sender = true;
        assert_eq!(decide(&x, ChatShape::Dm, 25), Verdict::Skip(Skip::Sender));
        let mut x = r();
        x.blocks_sender = true;
        x.archived = true;
        x.level = Level::All;
        assert_eq!(decide(&x, SMALL, 25), Verdict::Skip(Skip::BlockedSender));
        assert_eq!(
            decide(&x, ChatShape::Dm, 25),
            Verdict::Skip(Skip::BlockedSender)
        );
        let mut x = r();
        x.archived = true;
        x.level = Level::All;
        assert_eq!(decide(&x, SMALL, 25), Verdict::Skip(Skip::Archived));
    }

    #[test]
    fn levels_all_mentions_none_inherit() {
        let mut x = r();
        x.level = Level::All;
        assert_eq!(decide(&x, SMALL, 25), Verdict::Push);
        x.level = Level::None;
        assert_eq!(decide(&x, SMALL, 25), Verdict::Skip(Skip::LevelNone));
        x.level = Level::Mentions;
        assert_eq!(decide(&x, SMALL, 25), Verdict::Skip(Skip::NotMentioned));
        x.mentioned = true;
        assert_eq!(decide(&x, SMALL, 25), Verdict::Push);
        // Inherit → the account default.
        let mut x = r();
        x.level = Level::Inherit;
        x.default_level = Level::None;
        assert_eq!(
            decide(&x, ChatShape::Dm, 25),
            Verdict::Skip(Skip::LevelNone)
        );
        x.default_level = Level::Mentions;
        assert_eq!(
            decide(&x, ChatShape::Dm, 25),
            Verdict::Skip(Skip::NotMentioned)
        );
        x.default_level = Level::All;
        assert_eq!(decide(&x, ChatShape::Dm, 25), Verdict::Push);
        // The old mute flag is `none` while no level was chosen; an explicit level wins over it.
        let mut x = r();
        x.legacy_muted = true;
        assert_eq!(decide(&x, SMALL, 25), Verdict::Skip(Skip::LevelNone));
        x.level = Level::All;
        assert_eq!(decide(&x, SMALL, 25), Verdict::Push);
    }

    #[test]
    fn large_groups_default_to_mentions_only_unless_all_was_chosen() {
        let x = r();
        assert_eq!(decide(&x, SMALL, 25), Verdict::Push);
        assert_eq!(
            decide(&x, LARGE, 25),
            Verdict::Skip(Skip::LargeGroupNotMentioned)
        );
        assert_eq!(
            decide(&x, ChatShape::Group { humans: 25 }, 25),
            Verdict::Skip(Skip::LargeGroupNotMentioned),
            "the threshold is inclusive"
        );
        assert_eq!(
            decide(&x, LARGE, 100),
            Verdict::Push,
            "threshold is configurable"
        );
        let mut m = r();
        m.mentioned = true;
        assert_eq!(decide(&m, LARGE, 25), Verdict::Push);
        let mut a = r();
        a.level = Level::All;
        assert_eq!(
            decide(&a, LARGE, 25),
            Verdict::Push,
            "an explicit all is kept"
        );
        let mut d = r();
        d.default_level = Level::All;
        assert_eq!(
            decide(&d, LARGE, 25),
            Verdict::Skip(Skip::LargeGroupNotMentioned),
            "the account default is still only inherited"
        );
        // DMs and project chats never count as large.
        assert_eq!(decide(&x, ChatShape::Dm, 2), Verdict::Push);
        assert_eq!(
            decide(&x, ChatShape::ProjectChat { shared: true }, 2),
            Verdict::Push
        );
        assert_eq!(
            effective_level(&x, LARGE, 25),
            (Level::Mentions, true),
            "silenced_by_level sees mentions, not none"
        );
    }

    #[test]
    fn mentions_by_declaration_or_handle_in_text() {
        assert!(mentioned("hi @jacob, look", &[], "u1", "jacob"));
        assert!(mentioned("hi @Jacob!", &[], "u1", "jacob"));
        assert!(!mentioned("hi @jacobson", &[], "u1", "jacob"));
        assert!(!mentioned("mail jacob@x.y", &[], "u1", "jacob"));
        assert!(mentioned("@everyone ship it", &[], "u1", "jacob"));
        assert!(mentioned("", &["user:u1".into()], "u1", ""));
        assert!(mentioned("", &["u1".into()], "u1", ""));
        assert!(mentioned("", &["@Jacob".into()], "u1", "jacob"));
        assert!(!mentioned("", &["user:u2".into()], "u1", "jacob"));
        assert!(!mentioned("plain", &[], "u1", ""));
    }

    /// `@here` is everyone in the chat (Jacob, 2026-10-24), and the apps resolve it into member
    /// ids on the row, not on the announce — so a `mentions`-only recipient hears it only if the
    /// prose is read for the word. `@agents` names the agents, so it mentions no person.
    #[test]
    fn here_mentions_every_person_and_agents_mentions_none() {
        assert!(mentioned("@here standup in five", &[], "u1", "jacob"));
        assert!(mentioned("ship it @HERE", &[], "u1", "jacob"));
        assert!(mentioned("@here", &[], "u1", ""), "no handle of our own");
        assert!(!mentioned("@agents take this one", &[], "u1", "jacob"));
        // Not a collective: a longer word, and an address that happens to contain the token.
        assert!(!mentioned("@heretic takes", &[], "u1", "jacob"));
        assert!(!mentioned("mail me@here.example", &[], "u1", "jacob"));
    }

    #[test]
    fn level_words() {
        assert_eq!(Level::parse("All"), Some(Level::All));
        assert_eq!(Level::parse("mentions"), Some(Level::Mentions));
        assert_eq!(Level::parse("none"), Some(Level::None));
        assert_eq!(Level::parse("inherit"), Some(Level::Inherit));
        assert_eq!(Level::parse("loud"), Option::None);
        for l in [Level::All, Level::Mentions, Level::None, Level::Inherit] {
            assert_eq!(Level::parse(l.as_str()), Some(l));
        }
    }
}
