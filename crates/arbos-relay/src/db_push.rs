//! Storage for message notifications: APNs device tokens, the per-chat message event log the
//! relay counts unread from (it never reads Loro rows, so writers tell it about whole
//! messages), per-user read marks, and which of those messages mentioned whom.
//! Same SQLite file; created from `Db::open`.

use std::collections::{BTreeMap, HashSet};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use arbos_core::ids::now_ms;

use crate::db::Db;

pub const PUSH_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS push_tokens (
  device_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, platform TEXT NOT NULL, token TEXT NOT NULL,
  environment TEXT NOT NULL, bundle_id TEXT NOT NULL, app_version TEXT NOT NULL DEFAULT '',
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS push_tokens_user ON push_tokens(user_id);
CREATE TABLE IF NOT EXISTS chat_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT, chat_id TEXT NOT NULL, project_id TEXT,
  message_id TEXT NOT NULL, sender_user TEXT, created_at INTEGER NOT NULL,
  UNIQUE (chat_id, message_id)
);
CREATE INDEX IF NOT EXISTS chat_events_chat ON chat_events(chat_id, id);
CREATE TABLE IF NOT EXISTS chat_user_reads (
  user_id TEXT NOT NULL, chat_id TEXT NOT NULL, event_id INTEGER NOT NULL, at INTEGER NOT NULL,
  PRIMARY KEY (user_id, chat_id)
);
CREATE TABLE IF NOT EXISTS chat_user_archive (
  user_id TEXT NOT NULL, chat_id TEXT NOT NULL, archived_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, chat_id)
);
CREATE TABLE IF NOT EXISTS space_user_archive (
  user_id TEXT NOT NULL, space_id TEXT NOT NULL, kind TEXT NOT NULL, archived_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, space_id)
);
CREATE TABLE IF NOT EXISTS chat_user_cursor (
  user_id TEXT NOT NULL, chat_id TEXT NOT NULL, seq INTEGER NOT NULL, at INTEGER NOT NULL,
  PRIMARY KEY (user_id, chat_id)
);
CREATE TABLE IF NOT EXISTS chat_mentions (
  event_id INTEGER NOT NULL, chat_id TEXT NOT NULL, user_id TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'direct', created_at INTEGER NOT NULL,
  PRIMARY KEY (event_id, user_id)
);
CREATE INDEX IF NOT EXISTS chat_mentions_user ON chat_mentions(user_id, chat_id, event_id);
CREATE TABLE IF NOT EXISTS chat_user_mention_reads (
  user_id TEXT NOT NULL, chat_id TEXT NOT NULL, event_id INTEGER NOT NULL, at INTEGER NOT NULL,
  PRIMARY KEY (user_id, chat_id)
);
"#;

/// What raised a mention row. Only [`MENTION_DIRECT`] counts toward the `@` mark
/// (Jacob, 2026-10-31): a collective is recorded so the rule can widen later
/// without a schema change, and so an `@here` that pushed can still be explained.
pub const MENTION_DIRECT: &str = "direct";
/// `@here` / `@everyone`: pushes today, never badges.
pub const MENTION_COLLECTIVE: &str = "collective";

/// Which mention rows (alias `m`) in one chat are still unread. `?1` user, `?2` chat, `?3`
/// mention kind. Narrower than [`UNREAD_SCOPE`] on purpose: it leaves out the membership and
/// archive clauses, because its one caller is reached through a route that has already checked
/// membership, and a mention in a chat the user archived is still theirs to clear.
const MENTION_UNREAD_WHERE: &str = "JOIN chat_events e ON e.id = m.event_id
     WHERE m.user_id = ?1 AND m.chat_id = ?2 AND m.kind = ?3
       AND m.event_id > COALESCE((SELECT r.event_id FROM chat_user_mention_reads r
                                   WHERE r.user_id = ?1 AND r.chat_id = ?2), 0)
       AND (e.sender_user IS NULL OR e.sender_user != ?1)
       AND (e.sender_user IS NULL OR e.sender_user NOT IN (SELECT blocked FROM blocks WHERE blocker = ?1))";

/// Keep the event log a month deep; unread counts older than that are noise.
const EVENT_RETENTION_MS: i64 = 30 * 24 * 3_600_000;

#[derive(Debug, Clone, Serialize)]
pub struct PushToken {
    pub device_id: String,
    pub user_id: String,
    pub platform: String,
    pub token: String,
    /// `development` (Xcode builds, APNs sandbox) or `production` (TestFlight / App Store).
    pub environment: String,
    pub bundle_id: String,
    pub app_version: String,
    pub updated_at: i64,
}

fn token_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PushToken> {
    Ok(PushToken {
        device_id: r.get(0)?,
        user_id: r.get(1)?,
        platform: r.get(2)?,
        token: r.get(3)?,
        environment: r.get(4)?,
        bundle_id: r.get(5)?,
        app_version: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

const TOKEN_COLS: &str =
    "device_id, user_id, platform, token, environment, bundle_id, app_version, updated_at";

#[derive(Debug, Clone, Serialize)]
pub struct UnreadChat {
    pub chat_id: String,
    /// `None` for people chats (the apps show `people`).
    pub project_id: Option<String>,
    pub unread: i64,
    /// Unread direct mentions of the user in this chat. Separate state from
    /// `unread`, and allowed to outlive it: opening a chat reads its messages,
    /// it does not mean the mention was seen.
    pub mentions: i64,
}

impl Db {
    // ---- device tokens ------------------------------------------------------------------------

    pub fn upsert_push_token(&self, t: &PushToken) -> Result<()> {
        self.with(|c| {
            // The same APNs token re-registered from a re-paired device moves to that device.
            c.execute(
                "DELETE FROM push_tokens WHERE token = ?1 AND device_id != ?2",
                params![t.token, t.device_id],
            )?;
            c.execute(
                "INSERT INTO push_tokens(device_id, user_id, platform, token, environment, bundle_id, app_version, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(device_id) DO UPDATE SET user_id = excluded.user_id, platform = excluded.platform,
                   token = excluded.token, environment = excluded.environment, bundle_id = excluded.bundle_id,
                   app_version = excluded.app_version, updated_at = excluded.updated_at",
                params![t.device_id, t.user_id, t.platform, t.token, t.environment, t.bundle_id, t.app_version, t.updated_at],
            )?;
            Ok(())
        })
    }

    pub fn delete_push_token(&self, device_id: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "DELETE FROM push_tokens WHERE device_id = ?1",
                params![device_id],
            )
            .map(|n| n > 0)
        })
    }

    /// APNs said the token is gone (`410 Unregistered`, `400 BadDeviceToken`).
    pub fn delete_push_token_value(&self, token: &str) -> Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM push_tokens WHERE token = ?1", params![token])
                .map(|_| ())
        })
    }

    /// Tokens of a user's live (non-revoked) devices.
    pub fn push_tokens_of_user(&self, user_id: &str) -> Result<Vec<PushToken>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {TOKEN_COLS} FROM push_tokens t
                 WHERE t.user_id = ?1
                   AND EXISTS (SELECT 1 FROM devices d WHERE d.id = t.device_id AND d.revoked_at IS NULL)
                 ORDER BY t.updated_at"
            ))?;
            let rows = st.query_map(params![user_id], token_from_row)?;
            rows.collect()
        })
    }

    pub fn push_token(&self, device_id: &str) -> Result<Option<PushToken>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {TOKEN_COLS} FROM push_tokens WHERE device_id = ?1"),
                params![device_id],
                token_from_row,
            )
            .optional()
        })
    }

    /// Every registered token (admin view).
    pub fn all_push_tokens(&self) -> Result<Vec<PushToken>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {TOKEN_COLS} FROM push_tokens ORDER BY updated_at DESC"
            ))?;
            let rows = st.query_map([], token_from_row)?;
            rows.collect()
        })
    }

    // ---- message events and unread --------------------------------------------------------------

    /// Record one whole message. Returns its event id, or `None` when `(chat, message_id)` was
    /// already recorded (the same message announced twice is one notification).
    pub fn insert_chat_event(
        &self,
        chat_id: &str,
        project_id: Option<&str>,
        message_id: &str,
        sender_user: Option<&str>,
    ) -> Result<Option<i64>> {
        self.with(|c| {
            let now = now_ms();
            let n = c.execute(
                "INSERT OR IGNORE INTO chat_events(chat_id, project_id, message_id, sender_user, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![chat_id, project_id, message_id, sender_user, now],
            )?;
            c.execute(
                "DELETE FROM chat_events WHERE created_at < ?1",
                params![now - EVENT_RETENTION_MS],
            )?;
            // Mention rows hang off the log, so they age out with it. Without this they would
            // outlive their event forever: the join already stops counting them, so they
            // would be invisible and unbounded at the same time.
            c.execute(
                "DELETE FROM chat_mentions WHERE created_at < ?1",
                params![now - EVENT_RETENTION_MS],
            )?;
            Ok((n > 0).then(|| c.last_insert_rowid()))
        })
    }

    /// Was this message announced (`chat_events` row present)?
    pub fn chat_event_exists(&self, chat_id: &str, message_id: &str) -> Result<bool> {
        self.with(|c| {
            c.query_row(
                "SELECT EXISTS (SELECT 1 FROM chat_events WHERE chat_id = ?1 AND message_id = ?2)",
                params![chat_id, message_id],
                |r| r.get(0),
            )
        })
    }

    /// Has `sender_user` announced a message in `chat_id` since `since_ms`? (The row-level
    /// fallback checks this before sending a content-less notification.)
    pub fn chat_event_since(
        &self,
        chat_id: &str,
        sender_user: &str,
        since_ms: i64,
    ) -> Result<bool> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM chat_events WHERE chat_id = ?1 AND sender_user = ?2 AND created_at >= ?3",
                params![chat_id, sender_user, since_ms],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
        })
    }

    // ---- archive (per user, per chat; hides only) ---------------------------------------------

    /// Archive or unarchive a chat for a user. Returns the new `archived_at` (`None` = not archived).
    pub fn set_chat_archived(
        &self,
        user_id: &str,
        chat_id: &str,
        archived: bool,
    ) -> Result<Option<i64>> {
        self.with(|c| {
            if archived {
                let now = now_ms();
                c.execute(
                    "INSERT INTO chat_user_archive(user_id, chat_id, archived_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(user_id, chat_id) DO NOTHING",
                    params![user_id, chat_id, now],
                )?;
                c.query_row(
                    "SELECT archived_at FROM chat_user_archive WHERE user_id = ?1 AND chat_id = ?2",
                    params![user_id, chat_id],
                    |r| r.get::<_, i64>(0),
                )
                .map(Some)
            } else {
                c.execute(
                    "DELETE FROM chat_user_archive WHERE user_id = ?1 AND chat_id = ?2",
                    params![user_id, chat_id],
                )?;
                Ok(None)
            }
        })
    }

    // ---- archive of a sidebar space (an agent / project or a server), per user --------------
    // Hides the row on every device of the user; never deletes anything. An archived agent
    // keeps running its routines.

    /// Archive or unarchive a space for a user; `kind` is `project` or `server`. Returns the new
    /// `archived_at` (`None` = not archived).
    pub fn set_space_archived(
        &self,
        user_id: &str,
        space_id: &str,
        kind: &str,
        archived: bool,
    ) -> Result<Option<i64>> {
        self.with(|c| {
            if archived {
                c.execute(
                    "INSERT INTO space_user_archive(user_id, space_id, kind, archived_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(user_id, space_id) DO NOTHING",
                    params![user_id, space_id, kind, now_ms()],
                )?;
                c.query_row(
                    "SELECT archived_at FROM space_user_archive WHERE user_id = ?1 AND space_id = ?2",
                    params![user_id, space_id],
                    |r| r.get::<_, i64>(0),
                )
                .map(Some)
            } else {
                c.execute(
                    "DELETE FROM space_user_archive WHERE user_id = ?1 AND space_id = ?2",
                    params![user_id, space_id],
                )?;
                Ok(None)
            }
        })
    }

    pub fn space_archived_at(&self, user_id: &str, space_id: &str) -> Result<Option<i64>> {
        self.with(|c| {
            c.query_row(
                "SELECT archived_at FROM space_user_archive WHERE user_id = ?1 AND space_id = ?2",
                params![user_id, space_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()
        })
    }

    pub fn chat_archived_at(&self, user_id: &str, chat_id: &str) -> Result<Option<i64>> {
        self.with(|c| {
            c.query_row(
                "SELECT archived_at FROM chat_user_archive WHERE user_id = ?1 AND chat_id = ?2",
                params![user_id, chat_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()
        })
    }

    /// Everything in the chat up to now is read for this user.
    pub fn mark_chat_read(&self, user_id: &str, chat_id: &str) -> Result<()> {
        self.with(|c| {
            let head: i64 = c.query_row(
                "SELECT COALESCE(MAX(id), 0) FROM chat_events WHERE chat_id = ?1",
                params![chat_id],
                |r| r.get(0),
            )?;
            c.execute(
                "INSERT INTO chat_user_reads(user_id, chat_id, event_id, at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(user_id, chat_id) DO UPDATE SET event_id = MAX(chat_user_reads.event_id, excluded.event_id), at = excluded.at",
                params![user_id, chat_id, head, now_ms()],
            )?;
            Ok(())
        })
    }

    // ---- unread mentions ----------------------------------------------------------------------
    // Two pieces of state, deliberately apart from the read mark above: `mark_chat_read` jumps to
    // the chat's head, so a mention hung off it would clear the moment the chat was opened.

    /// Record that `user_id` was mentioned by the message `event_id`, and how (`kind`:
    /// [`MENTION_DIRECT`] or [`MENTION_COLLECTIVE`]). Written once per (message, person);
    /// announcing the same message twice is still one mention.
    pub fn insert_chat_mention(
        &self,
        event_id: i64,
        chat_id: &str,
        user_id: &str,
        kind: &str,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO chat_mentions(event_id, chat_id, user_id, kind, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(event_id, user_id) DO NOTHING",
                params![event_id, chat_id, user_id, kind, now_ms()],
            )?;
            Ok(())
        })
    }

    /// Advance the user's mention cursor, never backwards. `MAX` is the merge: two devices
    /// writing different values converge on the larger one, so a mention can be neither
    /// resurrected nor lost. The caller only ever passes the oldest unread mention, which is
    /// what keeps the collapse from skipping one nobody saw.
    pub fn advance_mention_cursor(
        &self,
        user_id: &str,
        chat_id: &str,
        event_id: i64,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO chat_user_mention_reads(user_id, chat_id, event_id, at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(user_id, chat_id) DO UPDATE SET
                   event_id = MAX(chat_user_mention_reads.event_id, excluded.event_id), at = excluded.at",
                params![user_id, chat_id, event_id.max(0), now_ms()],
            )?;
            Ok(())
        })
    }

    /// The oldest unread direct mention in a chat, as `(event_id, message_id)`. The in-chat
    /// jump lands here, and only this row may advance the cursor.
    pub fn oldest_unread_mention(
        &self,
        user_id: &str,
        chat_id: &str,
    ) -> Result<Option<(i64, String)>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT m.event_id, e.message_id FROM chat_mentions m {MENTION_UNREAD_WHERE}
                     ORDER BY m.event_id LIMIT 1"
                ),
                params![user_id, chat_id, MENTION_DIRECT],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
    }

    /// The user's row cursor in a chat (one per user, shared by every device): the highest
    /// room `seq` any of their devices reported read, or that they wrote themselves.
    pub fn user_cursor(&self, user_id: &str, chat_id: &str) -> Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT seq FROM chat_user_cursor WHERE user_id = ?1 AND chat_id = ?2",
                params![user_id, chat_id],
                |r| r.get(0),
            )
            .optional()
            .map(|o| o.unwrap_or(0))
        })
    }

    /// Advance the user's row cursor (never backwards).
    pub fn advance_user_cursor(&self, user_id: &str, chat_id: &str, seq: i64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO chat_user_cursor(user_id, chat_id, seq, at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(user_id, chat_id) DO UPDATE SET seq = MAX(chat_user_cursor.seq, excluded.seq), at = excluded.at",
                params![user_id, chat_id, seq.max(0), now_ms()],
            )?;
            Ok(())
        })
    }

    /// Unread messages in one chat for a user: announced events after their read mark that
    /// someone else wrote. System lines and rows nobody announced do not count.
    pub fn unread_in_chat(&self, user_id: &str, chat_id: &str) -> Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM chat_events e
                 WHERE e.chat_id = ?2
                   AND e.id > COALESCE((SELECT r.event_id FROM chat_user_reads r WHERE r.user_id = ?1 AND r.chat_id = ?2), 0)
                   AND (e.sender_user IS NULL OR e.sender_user != ?1)
                   AND (e.sender_user IS NULL OR e.sender_user NOT IN (SELECT blocked FROM blocks WHERE blocker = ?1))",
                params![user_id, chat_id],
                |r| r.get(0),
            )
        })
    }

    /// Unread messages per chat for a user: events after their read mark, not their own, in
    /// chats they are a member of (people chats by membership; project chats by owning the
    /// project or being a live guest). Each row also carries the chat's unread direct
    /// mentions, and a chat with a mention but no unread message is listed with `unread: 0`.
    pub fn unread_for_user(&self, user_id: &str) -> Result<Vec<UnreadChat>> {
        self.with(|c| unread_query(c, user_id))
    }

    pub fn unread_total(&self, user_id: &str) -> Result<i64> {
        self.with(|c| Ok(unread_query(c, user_id)?.iter().map(|u| u.unread).sum()))
    }

    /// Add unread direct mentions to counts that did not come from [`Self::unread_for_user`] —
    /// the Stage B homeserver counts. The mention mark is the relay's own state whether or not
    /// a user's unread *messages* are counted by Synapse, so it is read from here either way
    /// and a flipped user is not left without the `@`.
    pub fn merge_unread_mentions(&self, user_id: &str, chats: &mut Vec<UnreadChat>) -> Result<()> {
        let member = format!("user:{user_id}");
        self.with(|c| merge_mentions(c, user_id, &member, chats))
    }

    /// The project a (project) chat belongs to, when `chat_id` is one.
    pub fn chat_project_id(&self, chat_id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT project_id FROM chats WHERE id = ?1 AND closed_at IS NULL",
                params![chat_id],
                |r| r.get(0),
            )
            .optional()
        })
    }
}

/// Which `chat_events` rows (alias `e`) a user may be told about at all: never their own,
/// never a sender they blocked, never an archived chat, and only in a chat they belong to —
/// people chats by membership, project chats by owning the project or being a live guest.
/// `?1` is the user id and `?2` is `user:<id>`. Shared by the unread and the mention queries
/// so a chat can never count for one and not the other.
const UNREAD_SCOPE: &str = "(e.sender_user IS NULL OR e.sender_user != ?1)
           AND (e.sender_user IS NULL OR e.sender_user NOT IN (SELECT blocked FROM blocks WHERE blocker = ?1))
           AND NOT EXISTS (SELECT 1 FROM chat_user_archive a WHERE a.user_id = ?1 AND a.chat_id = e.chat_id)
           AND (
             e.chat_id IN (SELECT chat_id FROM people_members WHERE member = ?2 AND left_at IS NULL)
             OR e.project_id IN (SELECT id FROM projects WHERE user_id = ?1)
             OR e.project_id IN (SELECT project_id FROM project_members WHERE user_id = ?1 AND left_at IS NULL)
           )";

fn unread_query(c: &Connection, user_id: &str) -> rusqlite::Result<Vec<UnreadChat>> {
    let member = format!("user:{user_id}");
    let mut st = c.prepare(&format!(
        "SELECT e.chat_id, e.project_id, COUNT(*) FROM chat_events e
         WHERE e.id > COALESCE((SELECT r.event_id FROM chat_user_reads r WHERE r.user_id = ?1 AND r.chat_id = e.chat_id), 0)
           AND {UNREAD_SCOPE}
         GROUP BY e.chat_id ORDER BY MAX(e.id) DESC"
    ))?;
    let rows = st.query_map(params![user_id, member], |r| {
        Ok(UnreadChat {
            chat_id: r.get(0)?,
            project_id: r.get(1)?,
            unread: r.get(2)?,
            mentions: 0,
        })
    })?;
    let mut out: Vec<UnreadChat> = rows.collect::<rusqlite::Result<_>>()?;
    merge_mentions(c, user_id, &member, &mut out)?;
    Ok(out)
}

/// Fill each row's unread direct mentions in, and append a row for a chat that holds one but
/// has no unread message.
///
/// Mentions are counted separately and merged rather than folded into the unread query: a chat
/// whose messages are all read can still hold an unread mention (opening a chat reads its
/// messages, it does not mean the mention was seen), and such a chat is absent from the unread
/// rows entirely. Merging is what puts the `@` on it.
fn merge_mentions(
    c: &Connection,
    user_id: &str,
    member: &str,
    out: &mut Vec<UnreadChat>,
) -> rusqlite::Result<()> {
    let mentions = mention_unread_query(c, user_id, member)?;
    for row in out.iter_mut() {
        row.mentions = mentions.get(&row.chat_id).map(|m| m.1).unwrap_or(0);
    }
    let seen: HashSet<&str> = out.iter().map(|r| r.chat_id.as_str()).collect();
    let mut extra: Vec<UnreadChat> = mentions
        .iter()
        .filter(|(chat_id, _)| !seen.contains(chat_id.as_str()))
        .map(|(chat_id, (project_id, count))| UnreadChat {
            chat_id: chat_id.clone(),
            project_id: project_id.clone(),
            unread: 0,
            mentions: *count,
        })
        .collect();
    extra.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
    out.append(&mut extra);
    Ok(())
}

/// chat id → (project id, unread direct mentions) for every chat where the user has one.
fn mention_unread_query(
    c: &Connection,
    user_id: &str,
    member: &str,
) -> rusqlite::Result<BTreeMap<String, (Option<String>, i64)>> {
    let mut st = c.prepare(&format!(
        "SELECT e.chat_id, MAX(e.project_id), COUNT(*) FROM chat_mentions m
         JOIN chat_events e ON e.id = m.event_id
         WHERE m.user_id = ?1 AND m.kind = ?3
           AND m.event_id > COALESCE((SELECT r.event_id FROM chat_user_mention_reads r
                                       WHERE r.user_id = ?1 AND r.chat_id = e.chat_id), 0)
           AND {UNREAD_SCOPE}
         GROUP BY e.chat_id"
    ))?;
    let rows = st.query_map(params![user_id, member, MENTION_DIRECT], |r| {
        Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?)))
    })?;
    rows.collect()
}
