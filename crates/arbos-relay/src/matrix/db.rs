//! Storage for the Matrix mirror (D88 §3.3, §4 stage A): the outbox of queued jobs, the
//! `chat_id ↔ room_id` map, `message_id ↔ event_id`, per-device Matrix tokens (stage C), the
//! inbound transaction ids we have already acknowledged, and the per-user read flip (stage B).
//! Same SQLite file; created from `Db::open` through [`MATRIX_SCHEMA`].

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use arbos_core::ids::now_ms;

use super::Job;
use crate::db::Db;

pub const MATRIX_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS matrix_outbox (
  id INTEGER PRIMARY KEY AUTOINCREMENT, chat_key TEXT NOT NULL, kind TEXT NOT NULL, payload TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL, created_at INTEGER NOT NULL,
  done_at INTEGER, failed_at INTEGER, last_error TEXT
);
CREATE INDEX IF NOT EXISTS matrix_outbox_pending ON matrix_outbox(done_at, next_at, id);
CREATE INDEX IF NOT EXISTS matrix_outbox_chat ON matrix_outbox(chat_key, done_at, id);
CREATE TABLE IF NOT EXISTS matrix_rooms (
  chat_id TEXT PRIMARY KEY, room_id TEXT NOT NULL UNIQUE, project_id TEXT, kind TEXT NOT NULL,
  created_at INTEGER NOT NULL, closed_at INTEGER
);
CREATE TABLE IF NOT EXISTS matrix_events (
  chat_id TEXT NOT NULL, message_id TEXT NOT NULL, event_id TEXT NOT NULL, sender TEXT NOT NULL,
  ts INTEGER NOT NULL, PRIMARY KEY (chat_id, message_id)
);
CREATE INDEX IF NOT EXISTS matrix_events_chat ON matrix_events(chat_id, ts);
CREATE TABLE IF NOT EXISTS matrix_users (
  id TEXT PRIMARY KEY, mxid TEXT NOT NULL UNIQUE, displayname TEXT, created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS matrix_devices (
  device_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, mxid TEXT NOT NULL, mx_device_id TEXT NOT NULL,
  mx_token_enc TEXT NOT NULL, created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS matrix_txn_seen (
  appservice TEXT NOT NULL, txn_id TEXT NOT NULL, seen_at INTEGER NOT NULL, events INTEGER NOT NULL,
  PRIMARY KEY (appservice, txn_id)
);
"#;

pub fn migrate_matrix(c: &Connection) {
    let _ = c.execute(
        "ALTER TABLE users ADD COLUMN matrix_reads INTEGER NOT NULL DEFAULT 0",
        [],
    );
}

/// Backoff after a failed attempt: 2 s, 4 s, … capped at 5 min.
pub fn backoff_ms(attempts: i64) -> i64 {
    (2_000i64 << attempts.clamp(0, 8)).min(300_000)
}

/// After this many failures a job is parked (`failed_at`) so it stops blocking its chat; the
/// shadow-compare job then shows the gap.
pub const MAX_ATTEMPTS: i64 = 40;

#[derive(Debug, Clone)]
pub struct OutboxRow {
    pub id: i64,
    pub attempts: i64,
    pub job: Job,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct OutboxStats {
    pub pending: i64,
    pub failed: i64,
    pub done: i64,
    pub oldest_pending_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MatrixRoom {
    pub chat_id: String,
    pub room_id: String,
    pub project_id: Option<String>,
    pub kind: String,
    pub created_at: i64,
    pub closed_at: Option<i64>,
}

impl Db {
    // ---- outbox -------------------------------------------------------------------------------

    pub fn matrix_enqueue(&self, job: &Job) -> Result<i64> {
        let v = serde_json::to_value(job)?;
        let kind = v["kind"].as_str().unwrap_or("?").to_owned();
        let payload = serde_json::to_string(&v)?;
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "INSERT INTO matrix_outbox(chat_key, kind, payload, next_at, created_at) VALUES (?1, ?2, ?3, ?4, ?4)",
                params![job.chat_key(), kind, payload, now],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    /// Like `matrix_enqueue`, due `delay_ms` from now.
    pub fn matrix_enqueue_after(&self, job: &Job, delay_ms: i64) -> Result<i64> {
        let v = serde_json::to_value(job)?;
        let kind = v["kind"].as_str().unwrap_or("?").to_owned();
        let payload = serde_json::to_string(&v)?;
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "INSERT INTO matrix_outbox(chat_key, kind, payload, next_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![job.chat_key(), kind, payload, now + delay_ms, now],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    /// Whether a job of `kind` for `chat_key` is still unfinished.
    pub fn matrix_pending_kind(&self, chat_key: &str, kind: &str) -> Result<bool> {
        self.with(|c| {
            c.query_row(
                "SELECT EXISTS (SELECT 1 FROM matrix_outbox WHERE chat_key = ?1 AND kind = ?2 AND done_at IS NULL)",
                params![chat_key, kind],
                |r| r.get(0),
            )
        })
    }

    /// Due jobs whose chat has no earlier unfinished job (order per chat is kept), oldest first.
    pub fn matrix_outbox_due(&self, limit: i64) -> Result<Vec<OutboxRow>> {
        let now = now_ms();
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT o.id, o.attempts, o.payload FROM matrix_outbox o
                 WHERE o.done_at IS NULL AND o.next_at <= ?1
                   AND NOT EXISTS (SELECT 1 FROM matrix_outbox p WHERE p.done_at IS NULL AND p.chat_key = o.chat_key AND p.id < o.id)
                 ORDER BY o.id LIMIT ?2",
            )?;
            let rows = st.query_map(params![now, limit], |r| {
                let payload: String = r.get(2)?;
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, payload))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, attempts, payload) = row?;
                match serde_json::from_str::<Job>(&payload) {
                    Ok(job) => out.push(OutboxRow { id, attempts, job }),
                    Err(e) => {
                        // Unreadable payload: park it so it never blocks the queue.
                        c.execute(
                            "UPDATE matrix_outbox SET done_at = ?2, failed_at = ?2, last_error = ?3 WHERE id = ?1",
                            params![id, now, format!("bad payload: {e}")],
                        )?;
                    }
                }
            }
            Ok(out)
        })
    }

    pub fn matrix_outbox_done(&self, id: i64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE matrix_outbox SET done_at = ?2, last_error = NULL WHERE id = ?1",
                params![id, now_ms()],
            )?;
            Ok(())
        })
    }

    /// Record a failure: retry later, or park the job after [`MAX_ATTEMPTS`].
    pub fn matrix_outbox_failed(&self, id: i64, attempts: i64, error: &str) -> Result<bool> {
        let now = now_ms();
        let parked = attempts + 1 >= MAX_ATTEMPTS;
        self.with(|c| {
            if parked {
                c.execute(
                    "UPDATE matrix_outbox SET attempts = attempts + 1, done_at = ?2, failed_at = ?2, last_error = ?3 WHERE id = ?1",
                    params![id, now, error],
                )?;
            } else {
                c.execute(
                    "UPDATE matrix_outbox SET attempts = attempts + 1, next_at = ?2, last_error = ?3 WHERE id = ?1",
                    params![id, now + backoff_ms(attempts), error],
                )?;
            }
            Ok(parked)
        })
    }

    pub fn matrix_outbox_stats(&self) -> Result<OutboxStats> {
        self.with(|c| {
            let pending: i64 = c.query_row("SELECT COUNT(*) FROM matrix_outbox WHERE done_at IS NULL", [], |r| r.get(0))?;
            let failed: i64 = c.query_row("SELECT COUNT(*) FROM matrix_outbox WHERE failed_at IS NOT NULL", [], |r| r.get(0))?;
            let done: i64 = c.query_row("SELECT COUNT(*) FROM matrix_outbox WHERE done_at IS NOT NULL AND failed_at IS NULL", [], |r| r.get(0))?;
            let oldest_pending_at: Option<i64> =
                c.query_row("SELECT MIN(created_at) FROM matrix_outbox WHERE done_at IS NULL", [], |r| r.get(0))?;
            Ok(OutboxStats { pending, failed, done, oldest_pending_at })
        })
    }

    /// Parked jobs (for `GET /admin/matrix/status?failed=1` and the retry command).
    pub fn matrix_outbox_failed_rows(&self, limit: i64) -> Result<Vec<serde_json::Value>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, chat_key, kind, attempts, last_error, created_at, failed_at FROM matrix_outbox
                 WHERE failed_at IS NOT NULL ORDER BY id DESC LIMIT ?1",
            )?;
            let rows = st.query_map(params![limit], |r| {
                Ok(serde_json::json!({
                    "id": r.get::<_, i64>(0)?, "chat_id": r.get::<_, String>(1)?, "kind": r.get::<_, String>(2)?,
                    "attempts": r.get::<_, i64>(3)?, "error": r.get::<_, Option<String>>(4)?,
                    "created_at": r.get::<_, i64>(5)?, "failed_at": r.get::<_, i64>(6)?,
                }))
            })?;
            rows.collect()
        })
    }

    /// Put parked jobs back in the queue and make every waiting job due now (after a fix).
    /// Returns how many parked jobs were revived.
    pub fn matrix_outbox_retry_failed(&self) -> Result<usize> {
        self.with(|c| {
            let now = now_ms();
            let revived = c.execute(
                "UPDATE matrix_outbox SET done_at = NULL, failed_at = NULL, attempts = 0, next_at = ?1 WHERE failed_at IS NOT NULL",
                params![now],
            )?;
            c.execute(
                "UPDATE matrix_outbox SET attempts = 0, next_at = ?1 WHERE done_at IS NULL",
                params![now],
            )?;
            Ok(revived)
        })
    }

    /// Finished jobs older than `keep_ms` are dropped (the worker prunes once an hour).
    pub fn matrix_outbox_prune(&self, keep_ms: i64) -> Result<usize> {
        self.with(|c| {
            c.execute(
                "DELETE FROM matrix_outbox WHERE done_at IS NOT NULL AND failed_at IS NULL AND done_at < ?1",
                params![now_ms() - keep_ms],
            )
        })
    }

    // ---- rooms and events ---------------------------------------------------------------------

    pub fn matrix_room(&self, chat_id: &str) -> Result<Option<MatrixRoom>> {
        self.with(|c| {
            c.query_row(
                "SELECT chat_id, room_id, project_id, kind, created_at, closed_at FROM matrix_rooms WHERE chat_id = ?1",
                params![chat_id],
                room_from_row,
            )
            .optional()
        })
    }

    pub fn matrix_room_by_room_id(&self, room_id: &str) -> Result<Option<MatrixRoom>> {
        self.with(|c| {
            c.query_row(
                "SELECT chat_id, room_id, project_id, kind, created_at, closed_at FROM matrix_rooms WHERE room_id = ?1",
                params![room_id],
                room_from_row,
            )
            .optional()
        })
    }

    pub fn matrix_insert_room(
        &self,
        chat_id: &str,
        room_id: &str,
        project_id: Option<&str>,
        kind: &str,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO matrix_rooms(chat_id, room_id, project_id, kind, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(chat_id) DO UPDATE SET room_id = excluded.room_id, closed_at = NULL",
                params![chat_id, room_id, project_id, kind, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn matrix_close_room(&self, chat_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE matrix_rooms SET closed_at = ?2 WHERE chat_id = ?1",
                params![chat_id, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn matrix_rooms_all(&self) -> Result<Vec<MatrixRoom>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT chat_id, room_id, project_id, kind, created_at, closed_at FROM matrix_rooms ORDER BY created_at",
            )?;
            let rows = st.query_map([], room_from_row)?;
            rows.collect()
        })
    }

    /// Open rooms of a project's chats.
    pub fn matrix_chats_of_project(&self, project_id: &str) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT chat_id FROM matrix_rooms WHERE project_id = ?1 AND closed_at IS NULL ORDER BY created_at",
            )?;
            let rows = st.query_map(params![project_id], |r| r.get(0))?;
            rows.collect()
        })
    }

    pub fn matrix_room_count(&self) -> Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM matrix_rooms WHERE closed_at IS NULL",
                [],
                |r| r.get(0),
            )
        })
    }

    pub fn matrix_event(&self, chat_id: &str, message_id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT event_id FROM matrix_events WHERE chat_id = ?1 AND message_id = ?2",
                params![chat_id, message_id],
                |r| r.get(0),
            )
            .optional()
        })
    }

    pub fn matrix_insert_event(
        &self,
        chat_id: &str,
        message_id: &str,
        event_id: &str,
        sender: &str,
        ts: i64,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO matrix_events(chat_id, message_id, event_id, sender, ts) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![chat_id, message_id, event_id, sender, ts],
            )?;
            Ok(())
        })
    }

    /// Newest event the relay wrote into a chat, in room stream order (insertion order: the
    /// worker sends one chat's jobs in sequence). A receipt here reads everything, including a
    /// live message that landed after an older row while history was still being replayed.
    pub fn matrix_latest_event(&self, chat_id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT event_id FROM matrix_events WHERE chat_id = ?1 ORDER BY rowid DESC LIMIT 1",
                params![chat_id],
                |r| r.get(0),
            )
            .optional()
        })
    }

    /// The mirrored event of the newest announced message at or before a user's read mark in a
    /// chat, and whether anything from someone else came after the mark. `None` when the user
    /// has no mark there.
    pub fn matrix_read_mark(
        &self,
        user_id: &str,
        chat_id: &str,
    ) -> Result<Option<(Option<String>, bool)>> {
        self.with(|c| {
            let mark: Option<i64> = c
                .query_row(
                    "SELECT event_id FROM chat_user_reads WHERE user_id = ?1 AND chat_id = ?2",
                    params![user_id, chat_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(mark) = mark else {
                return Ok(None);
            };
            let event: Option<String> = c
                .query_row(
                    "SELECT m.event_id FROM matrix_events m
                     JOIN chat_events e ON e.chat_id = m.chat_id AND e.message_id = m.message_id
                     WHERE m.chat_id = ?1 AND e.id <= ?2
                     ORDER BY e.id DESC LIMIT 1",
                    params![chat_id, mark],
                    |r| r.get(0),
                )
                .optional()?;
            let caught_up: bool = c.query_row(
                "SELECT NOT EXISTS (SELECT 1 FROM chat_events WHERE chat_id = ?1 AND id > ?2
                                    AND (sender_user IS NULL OR sender_user != ?3))",
                params![chat_id, mark, user_id],
                |r| r.get(0),
            )?;
            Ok(Some((event, caught_up)))
        })
    }

    /// Chats where a user has a read mark.
    pub fn matrix_read_marked_chats(&self, user_id: &str) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT chat_id FROM chat_user_reads WHERE user_id = ?1")?;
            let rows = st.query_map(params![user_id], |r| r.get(0))?;
            rows.collect()
        })
    }

    /// Open rooms of people/project chats (not Spaces or channels): `(chat_id, project_id)`.
    pub fn matrix_open_chat_rooms(&self) -> Result<Vec<(String, Option<String>)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT chat_id, project_id FROM matrix_rooms WHERE closed_at IS NULL AND kind NOT IN ('space', 'channel')",
            )?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
    }

    pub fn matrix_event_count(&self) -> Result<i64> {
        self.with(|c| c.query_row("SELECT COUNT(*) FROM matrix_events", [], |r| r.get(0)))
    }

    /// `(message_id, event_id, ts)` of a chat, oldest first (shadow-compare).
    pub fn matrix_events_of(&self, chat_id: &str) -> Result<Vec<(String, String, i64)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT message_id, event_id, ts FROM matrix_events WHERE chat_id = ?1 ORDER BY ts, rowid",
            )?;
            let rows = st.query_map(params![chat_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect()
        })
    }

    // ---- users --------------------------------------------------------------------------------

    /// Known Matrix user (registered + profile set). `None` means not yet.
    pub fn matrix_user(&self, id: &str) -> Result<Option<(String, Option<String>)>> {
        self.with(|c| {
            c.query_row(
                "SELECT mxid, displayname FROM matrix_users WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
    }

    pub fn matrix_upsert_user(
        &self,
        id: &str,
        mxid: &str,
        displayname: Option<&str>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO matrix_users(id, mxid, displayname, created_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET mxid = excluded.mxid, displayname = excluded.displayname",
                params![id, mxid, displayname, now_ms()],
            )?;
            Ok(())
        })
    }

    // ---- devices (stage C) --------------------------------------------------------------------

    /// `(matrix device id, encrypted access token)` of an app device.
    pub fn matrix_device(&self, device_id: &str) -> Result<Option<(String, String)>> {
        self.with(|c| {
            c.query_row(
                "SELECT mx_device_id, mx_token_enc FROM matrix_devices WHERE device_id = ?1",
                params![device_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
    }

    /// Every user that has at least one device with a Matrix identity (pusher backfill).
    pub fn matrix_device_users(&self) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT DISTINCT user_id FROM matrix_devices")?;
            let rows = st.query_map([], |r| r.get(0))?;
            rows.collect()
        })
    }

    pub fn matrix_insert_device(
        &self,
        device_id: &str,
        user_id: &str,
        mxid: &str,
        mx_device_id: &str,
        enc: &str,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO matrix_devices(device_id, user_id, mxid, mx_device_id, mx_token_enc, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(device_id) DO UPDATE SET mx_device_id = excluded.mx_device_id, mx_token_enc = excluded.mx_token_enc",
                params![device_id, user_id, mxid, mx_device_id, enc, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn matrix_delete_device(&self, device_id: &str) -> Result<Option<(String, String)>> {
        self.with(|c| {
            let row = c
                .query_row(
                    "SELECT mxid, mx_device_id FROM matrix_devices WHERE device_id = ?1",
                    params![device_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            c.execute(
                "DELETE FROM matrix_devices WHERE device_id = ?1",
                params![device_id],
            )?;
            Ok(row)
        })
    }

    // ---- inbound transactions -----------------------------------------------------------------

    /// Remember an acknowledged transaction; `false` when it was seen before (Synapse retried).
    pub fn matrix_txn_mark(&self, appservice: &str, txn_id: &str, events: i64) -> Result<bool> {
        self.with(|c| {
            let n = c.execute(
                "INSERT OR IGNORE INTO matrix_txn_seen(appservice, txn_id, seen_at, events) VALUES (?1, ?2, ?3, ?4)",
                params![appservice, txn_id, now_ms(), events],
            )?;
            c.execute(
                "DELETE FROM matrix_txn_seen WHERE seen_at < ?1",
                params![now_ms() - 7 * 24 * 3_600_000],
            )?;
            Ok(n > 0)
        })
    }

    // ---- read flip (stage B) ------------------------------------------------------------------

    pub fn set_matrix_reads(&self, user_id: &str, on: bool) -> Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE users SET matrix_reads = ?2 WHERE id = ?1",
                params![user_id, on as i64],
            )? > 0)
        })
    }

    pub fn matrix_reads_users(&self) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT id FROM users WHERE matrix_reads = 1 ORDER BY id")?;
            let rows = st.query_map([], |r| r.get(0))?;
            rows.collect()
        })
    }
}

fn room_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MatrixRoom> {
    Ok(MatrixRoom {
        chat_id: r.get(0)?,
        room_id: r.get(1)?,
        project_id: r.get(2)?,
        kind: r.get(3)?,
        created_at: r.get(4)?,
        closed_at: r.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbos_core::ids::ulid;

    fn db() -> Db {
        let dir = std::env::temp_dir().join(format!("arbos-matrix-db-{}", ulid()));
        Db::open(&dir.join("relay.db")).expect("db")
    }

    #[test]
    fn outbox_keeps_order_per_chat_and_parks_poison() {
        let d = db();
        let a1 = d
            .matrix_enqueue(&Job::SyncRoom {
                chat_id: "a".into(),
                project_id: None,
            })
            .unwrap();
        let a2 = d
            .matrix_enqueue(&Job::Read {
                chat_id: "a".into(),
                user_id: "u".into(),
            })
            .unwrap();
        let b1 = d
            .matrix_enqueue(&Job::CloseRoom {
                chat_id: "b".into(),
            })
            .unwrap();
        let due: Vec<i64> = d
            .matrix_outbox_due(10)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(due, vec![a1, b1], "a2 waits behind a1; b runs in parallel");
        // a1 fails: it is retried later and still blocks a2.
        assert!(!d.matrix_outbox_failed(a1, 0, "boom").unwrap());
        let due: Vec<i64> = d
            .matrix_outbox_due(10)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(due, vec![b1]);
        // After enough failures it is parked and a2 becomes runnable.
        assert!(d
            .matrix_outbox_failed(a1, MAX_ATTEMPTS - 1, "boom")
            .unwrap());
        let due: Vec<i64> = d
            .matrix_outbox_due(10)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(due, vec![a2, b1]);
        let st = d.matrix_outbox_stats().unwrap();
        assert_eq!((st.pending, st.failed, st.done), (2, 1, 0));
        assert_eq!(d.matrix_outbox_retry_failed().unwrap(), 1);
        d.matrix_outbox_done(b1).unwrap();
        assert_eq!(d.matrix_outbox_stats().unwrap().done, 1);
    }

    #[test]
    fn rooms_events_users_and_flip() {
        let d = db();
        d.matrix_insert_room("c1", "!r1:arbos.life", None, "dm")
            .unwrap();
        assert_eq!(
            d.matrix_room("c1").unwrap().unwrap().room_id,
            "!r1:arbos.life"
        );
        assert_eq!(
            d.matrix_room_by_room_id("!r1:arbos.life")
                .unwrap()
                .unwrap()
                .chat_id,
            "c1"
        );
        d.matrix_insert_event("c1", "m1", "$e1", "user:u", 10)
            .unwrap();
        d.matrix_insert_event("c1", "m2", "$e2", "user:u", 20)
            .unwrap();
        d.matrix_insert_event("c1", "m2", "$dup", "user:u", 30)
            .unwrap();
        assert_eq!(d.matrix_event("c1", "m2").unwrap().as_deref(), Some("$e2"));
        assert_eq!(d.matrix_latest_event("c1").unwrap().as_deref(), Some("$e2"));
        assert_eq!(d.matrix_event_count().unwrap(), 2);
        d.matrix_upsert_user("gh_1", "@gh_1:arbos.life", Some("one"))
            .unwrap();
        assert_eq!(
            d.matrix_user("gh_1").unwrap().unwrap().1.as_deref(),
            Some("one")
        );
        assert!(d.matrix_txn_mark("arbos-users", "t1", 3).unwrap());
        assert!(!d.matrix_txn_mark("arbos-users", "t1", 3).unwrap());
        assert!(!d.set_matrix_reads("nobody", true).unwrap());
        assert_eq!(backoff_ms(0), 2_000);
        assert_eq!(backoff_ms(20), 300_000);
    }
}
