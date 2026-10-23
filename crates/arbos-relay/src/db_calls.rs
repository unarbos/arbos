//! Storage for calls: one row per call, a participant log (who was in it, when), and VoIP
//! (PushKit) tokens. Same SQLite file; created from `Db::open` through `CALLS_SCHEMA`.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use arbos_core::ids::now_ms;

use crate::db::Db;

pub const CALLS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS calls (
  id TEXT PRIMARY KEY, chat_id TEXT NOT NULL, project_id TEXT, kind TEXT NOT NULL, room TEXT NOT NULL UNIQUE,
  started_by TEXT NOT NULL, started_at INTEGER NOT NULL, ended_at INTEGER, end_reason TEXT,
  last_activity_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS calls_chat_active ON calls(chat_id, ended_at);
CREATE TABLE IF NOT EXISTS call_participants (
  call_id TEXT NOT NULL, user_id TEXT NOT NULL, joined_at INTEGER NOT NULL, left_at INTEGER,
  PRIMARY KEY (call_id, user_id)
);
CREATE TABLE IF NOT EXISTS voip_tokens (
  device_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, token TEXT NOT NULL, environment TEXT NOT NULL,
  bundle_id TEXT NOT NULL, updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS voip_tokens_user ON voip_tokens(user_id);
CREATE TABLE IF NOT EXISTS call_speakers (
  call_id TEXT NOT NULL, user_id TEXT NOT NULL,
  PRIMARY KEY (call_id, user_id)
);
"#;

/// Columns added after the first calls release (ALTER is idempotent through the error).
pub fn migrate_calls(c: &Connection) {
    for col in ["started_device TEXT", "platform TEXT", "app_version TEXT"] {
        let _ = c.execute(&format!("ALTER TABLE calls ADD COLUMN {col}"), []);
    }
    let _ = c.execute(
        "CREATE TABLE IF NOT EXISTS call_speakers (
          call_id TEXT NOT NULL, user_id TEXT NOT NULL,
          PRIMARY KEY (call_id, user_id)
        )",
        [],
    );
}

#[derive(Debug, Clone, Serialize)]
pub struct CallRow {
    pub id: String,
    pub chat_id: String,
    /// `None` for people chats.
    pub project_id: Option<String>,
    pub kind: String,
    pub room: String,
    pub started_by: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub end_reason: Option<String>,
    /// Last join/leave/webhook touch; the sweeper ends calls that went quiet.
    pub last_activity_at: i64,
    /// The device that started it, and what it said it was (`X-Arbos-Client`, else its push
    /// registration): the call-log answer to "which app version placed this call?".
    pub started_device: Option<String>,
    pub platform: Option<String>,
    pub app_version: Option<String>,
}

impl CallRow {
    pub fn active(&self) -> bool {
        self.ended_at.is_none()
    }
}

const CALL_COLS: &str =
    "id, chat_id, project_id, kind, room, started_by, started_at, ended_at, end_reason, last_activity_at, started_device, platform, app_version";

fn call_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<CallRow> {
    Ok(CallRow {
        id: r.get(0)?,
        chat_id: r.get(1)?,
        project_id: r.get(2)?,
        kind: r.get(3)?,
        room: r.get(4)?,
        started_by: r.get(5)?,
        started_at: r.get(6)?,
        ended_at: r.get(7)?,
        end_reason: r.get(8)?,
        last_activity_at: r.get(9)?,
        started_device: r.get(10)?,
        platform: r.get(11)?,
        app_version: r.get(12)?,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct Participant {
    pub user_id: String,
    pub joined_at: i64,
    pub left_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VoipToken {
    pub device_id: String,
    pub user_id: String,
    pub token: String,
    pub environment: String,
    pub bundle_id: String,
    pub updated_at: i64,
}

fn voip_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<VoipToken> {
    Ok(VoipToken {
        device_id: r.get(0)?,
        user_id: r.get(1)?,
        token: r.get(2)?,
        environment: r.get(3)?,
        bundle_id: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

impl Db {
    // ---- calls --------------------------------------------------------------------------------

    pub fn insert_call(&self, c: &CallRow) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO calls(id, chat_id, project_id, kind, room, started_by, started_at, last_activity_at, started_device, platform, app_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![c.id, c.chat_id, c.project_id, c.kind, c.room, c.started_by, c.started_at, c.last_activity_at, c.started_device, c.platform, c.app_version],
            )
            .map(|_| ())
        })
    }

    pub fn call(&self, id: &str) -> Result<Option<CallRow>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {CALL_COLS} FROM calls WHERE id = ?1"),
                params![id],
                call_from_row,
            )
            .optional()
        })
    }

    pub fn call_by_room(&self, room: &str) -> Result<Option<CallRow>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {CALL_COLS} FROM calls WHERE room = ?1"),
                params![room],
                call_from_row,
            )
            .optional()
        })
    }

    /// The one call that is live in a chat (there is never more than one).
    pub fn active_call_in_chat(&self, chat_id: &str) -> Result<Option<CallRow>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {CALL_COLS} FROM calls WHERE chat_id = ?1 AND ended_at IS NULL ORDER BY started_at DESC LIMIT 1"
                ),
                params![chat_id],
                call_from_row,
            )
            .optional()
        })
    }

    pub fn active_calls(&self) -> Result<Vec<CallRow>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {CALL_COLS} FROM calls WHERE ended_at IS NULL ORDER BY started_at"
            ))?;
            let rows = st.query_map([], call_from_row)?;
            rows.collect()
        })
    }

    pub fn recent_calls(&self, limit: i64) -> Result<Vec<CallRow>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {CALL_COLS} FROM calls WHERE ended_at IS NOT NULL ORDER BY ended_at DESC LIMIT ?1"
            ))?;
            let rows = st.query_map(params![limit], call_from_row)?;
            rows.collect()
        })
    }

    /// Mark a call over. Returns `false` when it had already ended (so the end is announced once).
    pub fn end_call(&self, id: &str, reason: &str) -> Result<bool> {
        let now = now_ms();
        self.with(|c| {
            let n = c.execute(
                "UPDATE calls SET ended_at = ?2, end_reason = ?3, last_activity_at = ?2 WHERE id = ?1 AND ended_at IS NULL",
                params![id, now, reason],
            )?;
            if n > 0 {
                c.execute(
                    "UPDATE call_participants SET left_at = ?2 WHERE call_id = ?1 AND left_at IS NULL",
                    params![id, now],
                )?;
            }
            Ok(n > 0)
        })
    }

    pub fn touch_call(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE calls SET last_activity_at = ?2 WHERE id = ?1",
                params![id, now_ms()],
            )
            .map(|_| ())
        })
    }

    // ---- participants ---------------------------------------------------------------------------

    /// A user is (back) in the call.
    pub fn call_join(&self, call_id: &str, user_id: &str) -> Result<()> {
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "INSERT INTO call_participants(call_id, user_id, joined_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(call_id, user_id) DO UPDATE SET left_at = NULL,
                   joined_at = CASE WHEN call_participants.left_at IS NULL THEN call_participants.joined_at ELSE excluded.joined_at END",
                params![call_id, user_id, now],
            )?;
            c.execute(
                "UPDATE calls SET last_activity_at = ?2 WHERE id = ?1",
                params![call_id, now],
            )?;
            Ok(())
        })
    }

    /// A user left. Returns how many are still in the call.
    pub fn call_leave(&self, call_id: &str, user_id: &str) -> Result<i64> {
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "UPDATE call_participants SET left_at = ?3 WHERE call_id = ?1 AND user_id = ?2 AND left_at IS NULL",
                params![call_id, user_id, now],
            )?;
            c.execute(
                "UPDATE calls SET last_activity_at = ?2 WHERE id = ?1",
                params![call_id, now],
            )?;
            c.query_row(
                "SELECT COUNT(*) FROM call_participants WHERE call_id = ?1 AND left_at IS NULL",
                params![call_id],
                |r| r.get(0),
            )
        })
    }

    pub fn call_participants(&self, call_id: &str) -> Result<Vec<Participant>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT user_id, joined_at, left_at FROM call_participants WHERE call_id = ?1 ORDER BY joined_at",
            )?;
            let rows = st.query_map(params![call_id], |r| {
                Ok(Participant {
                    user_id: r.get(0)?,
                    joined_at: r.get(1)?,
                    left_at: r.get(2)?,
                })
            })?;
            rows.collect()
        })
    }

    /// Invited speakers on a hosted group call (the host is `calls.started_by`, not listed here).
    pub fn call_speakers(&self, call_id: &str) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st =
                c.prepare("SELECT user_id FROM call_speakers WHERE call_id = ?1 ORDER BY user_id")?;
            let rows = st.query_map(params![call_id], |r| r.get(0))?;
            rows.collect()
        })
    }

    pub fn add_call_speaker(&self, call_id: &str, user_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO call_speakers(call_id, user_id) VALUES (?1, ?2)
                 ON CONFLICT(call_id, user_id) DO NOTHING",
                params![call_id, user_id],
            )
            .map(|_| ())
        })
    }

    pub fn remove_call_speaker(&self, call_id: &str, user_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM call_speakers WHERE call_id = ?1 AND user_id = ?2",
                params![call_id, user_id],
            )
            .map(|_| ())
        })
    }

    /// When the last live participant left (`None` while someone is still in).
    pub fn call_empty_since(&self, call_id: &str) -> Result<Option<i64>> {
        self.with(|c| {
            let live: i64 = c.query_row(
                "SELECT COUNT(*) FROM call_participants WHERE call_id = ?1 AND left_at IS NULL",
                params![call_id],
                |r| r.get(0),
            )?;
            if live > 0 {
                return Ok(None);
            }
            c.query_row(
                "SELECT MAX(left_at) FROM call_participants WHERE call_id = ?1",
                params![call_id],
                |r| r.get::<_, Option<i64>>(0),
            )
        })
    }

    // ---- VoIP tokens ----------------------------------------------------------------------------

    pub fn upsert_voip_token(&self, t: &VoipToken) -> Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM voip_tokens WHERE token = ?1 AND device_id != ?2",
                params![t.token, t.device_id],
            )?;
            c.execute(
                "INSERT INTO voip_tokens(device_id, user_id, token, environment, bundle_id, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(device_id) DO UPDATE SET user_id = excluded.user_id, token = excluded.token,
                   environment = excluded.environment, bundle_id = excluded.bundle_id, updated_at = excluded.updated_at",
                params![t.device_id, t.user_id, t.token, t.environment, t.bundle_id, t.updated_at],
            )?;
            Ok(())
        })
    }

    pub fn delete_voip_token(&self, device_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM voip_tokens WHERE device_id = ?1",
                params![device_id],
            )
            .map(|_| ())
        })
    }

    pub fn delete_voip_token_value(&self, token: &str) -> Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM voip_tokens WHERE token = ?1", params![token])
                .map(|_| ())
        })
    }

    /// VoIP tokens of a user's live devices.
    pub fn voip_tokens_of_user(&self, user_id: &str) -> Result<Vec<VoipToken>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT device_id, user_id, token, environment, bundle_id, updated_at FROM voip_tokens t
                 WHERE t.user_id = ?1
                   AND EXISTS (SELECT 1 FROM devices d WHERE d.id = t.device_id AND d.revoked_at IS NULL)
                 ORDER BY t.updated_at",
            )?;
            let rows = st.query_map(params![user_id], voip_from_row)?;
            rows.collect()
        })
    }

    pub fn all_voip_tokens(&self) -> Result<Vec<VoipToken>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT device_id, user_id, token, environment, bundle_id, updated_at FROM voip_tokens ORDER BY updated_at DESC",
            )?;
            let rows = st.query_map([], voip_from_row)?;
            rows.collect()
        })
    }

    /// A user's live devices with what the relay knows about their app: `(id, name, kind,
    /// last_seen_at, push platform, push app_version, has_voip_token)`.
    #[allow(clippy::type_complexity)]
    pub fn device_reach_rows(
        &self,
        user_id: &str,
    ) -> Result<
        Vec<(
            String,
            String,
            String,
            Option<i64>,
            Option<String>,
            Option<String>,
            bool,
        )>,
    > {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT d.id, d.name, d.kind, d.last_seen_at, p.platform, p.app_version,
                        EXISTS (SELECT 1 FROM voip_tokens v WHERE v.device_id = d.id)
                 FROM devices d LEFT JOIN push_tokens p ON p.device_id = d.id
                 WHERE d.user_id = ?1 AND d.revoked_at IS NULL ORDER BY COALESCE(d.last_seen_at, d.created_at) DESC",
            )?;
            let rows = st.query_map(params![user_id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get::<_, i64>(6)? != 0,
                ))
            })?;
            rows.collect()
        })
    }

    /// The device of a user that was seen most recently (the one asked to write a system line).
    pub fn freshest_device_of(&self, user_id: &str) -> Result<Option<String>> {
        self.with(|c: &Connection| {
            c.query_row(
                "SELECT id FROM devices WHERE user_id = ?1 AND revoked_at IS NULL
                 ORDER BY COALESCE(last_seen_at, created_at) DESC LIMIT 1",
                params![user_id],
                |r| r.get(0),
            )
            .optional()
        })
    }
}
