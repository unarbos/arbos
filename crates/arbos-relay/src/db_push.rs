//! Storage for message notifications: APNs device tokens, the per-chat message event log the
//! relay counts unread from (it never reads Loro rows, so writers tell it about whole
//! messages), and per-user read marks. Same SQLite file; created from `Db::open`.

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
"#;

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
    /// project or being a live guest).
    pub fn unread_for_user(&self, user_id: &str) -> Result<Vec<UnreadChat>> {
        self.with(|c| unread_query(c, user_id))
    }

    pub fn unread_total(&self, user_id: &str) -> Result<i64> {
        self.with(|c| Ok(unread_query(c, user_id)?.iter().map(|u| u.unread).sum()))
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

fn unread_query(c: &Connection, user_id: &str) -> rusqlite::Result<Vec<UnreadChat>> {
    let member = format!("user:{user_id}");
    let mut st = c.prepare(
        "SELECT e.chat_id, e.project_id, COUNT(*) FROM chat_events e
         WHERE e.id > COALESCE((SELECT r.event_id FROM chat_user_reads r WHERE r.user_id = ?1 AND r.chat_id = e.chat_id), 0)
           AND (e.sender_user IS NULL OR e.sender_user != ?1)
           AND (e.sender_user IS NULL OR e.sender_user NOT IN (SELECT blocked FROM blocks WHERE blocker = ?1))
           AND NOT EXISTS (SELECT 1 FROM chat_user_archive a WHERE a.user_id = ?1 AND a.chat_id = e.chat_id)
           AND (
             e.chat_id IN (SELECT chat_id FROM people_members WHERE member = ?2 AND left_at IS NULL)
             OR e.project_id IN (SELECT id FROM projects WHERE user_id = ?1)
             OR e.project_id IN (SELECT project_id FROM project_members WHERE user_id = ?1 AND left_at IS NULL)
           )
         GROUP BY e.chat_id ORDER BY MAX(e.id) DESC",
    )?;
    let rows = st.query_map(params![user_id, member], |r| {
        Ok(UnreadChat {
            chat_id: r.get(0)?,
            project_id: r.get(1)?,
            unread: r.get(2)?,
        })
    })?;
    rows.collect()
}
