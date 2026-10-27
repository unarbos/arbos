//! The durable outbox table. Every push the relay decides to send is a row here first; the
//! worker in [`super::outbox`] claims due rows, hands them to the provider and records the
//! answer. A relay crash between enqueue and send loses nothing: the row is still queued when
//! the next process starts, and its lease has lapsed, so the worker picks it up again.
//!
//! Row states: `0` queued (due at `next_attempt_at`, in flight while `leased_until` is in the
//! future), `1` sent. Sent rows stay [`SENT_RETENTION_MS`] so the dedupe key keeps a second
//! announcement of the same message from pushing again; queued rows past `expires_at` are
//! dropped when the worker meets them.

use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

use arbos_core::ids::now_ms;

use crate::db::Db;

use super::payload::PushKind;

pub const OUTBOX_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS push_outbox (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  dedupe_key TEXT NOT NULL UNIQUE,
  user_id TEXT NOT NULL, device_id TEXT NOT NULL,
  kind TEXT NOT NULL, payload TEXT NOT NULL, collapse_id TEXT,
  state INTEGER NOT NULL DEFAULT 0,
  attempts INTEGER NOT NULL DEFAULT 0, next_attempt_at INTEGER NOT NULL,
  leased_until INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, sent_at INTEGER,
  last_error TEXT
);
CREATE INDEX IF NOT EXISTS push_outbox_due ON push_outbox(state, next_attempt_at);
CREATE INDEX IF NOT EXISTS push_outbox_device ON push_outbox(device_id);
"#;

/// Sent rows are kept this long for dedupe (a message re-announced within the window pushes
/// no second time), then pruned.
pub const SENT_RETENTION_MS: i64 = 48 * 3_600_000;

/// One queued push, as the worker sees it.
#[derive(Debug, Clone)]
pub struct OutboxRow {
    pub id: i64,
    pub user_id: String,
    pub device_id: String,
    pub kind: PushKind,
    pub payload: Value,
    pub collapse_id: Option<String>,
    pub attempts: i64,
    pub expires_at: i64,
}

/// What `enqueue` is given: the recipient device and the finished payload.
#[derive(Debug, Clone)]
pub struct NewPush {
    pub user_id: String,
    pub device_id: String,
    pub kind: PushKind,
    pub payload: Value,
    pub collapse_id: Option<String>,
    /// How long the push stays worth sending; after this the worker drops it unsent.
    pub ttl_ms: i64,
}

/// `Err(id)` when the stored kind or payload no longer parses (an old process wrote a shape
/// this one does not know): the caller drops that row so it cannot block the queue.
fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<OutboxRow, i64>> {
    let id: i64 = r.get(0)?;
    let kind_raw: String = r.get(3)?;
    let payload_raw: String = r.get(4)?;
    let kind = serde_json::from_str::<Value>(&kind_raw)
        .ok()
        .and_then(|v| PushKind::from_stored(&v));
    let payload = serde_json::from_str::<Value>(&payload_raw).ok();
    let (Some(kind), Some(payload)) = (kind, payload) else {
        return Ok(Err(id));
    };
    Ok(Ok(OutboxRow {
        id,
        user_id: r.get(1)?,
        device_id: r.get(2)?,
        kind,
        payload,
        collapse_id: r.get(5)?,
        attempts: r.get(6)?,
        expires_at: r.get(7)?,
    }))
}

const ROW_COLS: &str = "id, user_id, device_id, kind, payload, collapse_id, attempts, expires_at";

/// Counts for the admin view.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct OutboxCounts {
    pub queued: i64,
    pub due: i64,
    pub in_flight: i64,
    pub sent_recent: i64,
}

impl Db {
    /// Queue pushes in one transaction. Returns how many were new (a duplicate dedupe key is
    /// silently skipped — that push already happened or is already queued).
    pub fn outbox_enqueue(&self, pushes: &[NewPush]) -> Result<usize> {
        if pushes.is_empty() {
            return Ok(0);
        }
        self.with(|c| {
            let now = now_ms();
            let tx = c.unchecked_transaction()?;
            let mut inserted = 0usize;
            for p in pushes {
                let n = tx.execute(
                    "INSERT OR IGNORE INTO push_outbox
                       (dedupe_key, user_id, device_id, kind, payload, collapse_id, state, attempts,
                        next_attempt_at, leased_until, created_at, expires_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, ?7, 0, ?7, ?8)",
                    params![
                        p.kind.dedupe_key(&p.device_id, now),
                        p.user_id,
                        p.device_id,
                        p.kind.to_stored().to_string(),
                        p.payload.to_string(),
                        p.collapse_id,
                        now,
                        now + p.ttl_ms.max(1_000),
                    ],
                )?;
                inserted += n;
            }
            tx.execute(
                "DELETE FROM push_outbox WHERE state = 1 AND sent_at < ?1",
                params![now - SENT_RETENTION_MS],
            )?;
            tx.commit()?;
            Ok(inserted)
        })
    }

    /// Take up to `limit` due rows for `lease_ms`: a crashed worker's lease lapses and the
    /// row is claimed again. Expired rows are not returned; [`Db::outbox_drop_expired`]
    /// removes them.
    pub fn outbox_claim_due(&self, limit: usize, lease_ms: i64) -> Result<Vec<OutboxRow>> {
        self.with(|c| {
            let now = now_ms();
            let tx = c.unchecked_transaction()?;
            let rows = {
                let mut st = tx.prepare(&format!(
                    "SELECT {ROW_COLS} FROM push_outbox
                     WHERE state = 0 AND next_attempt_at <= ?1 AND leased_until <= ?1 AND expires_at > ?1
                     ORDER BY next_attempt_at, id LIMIT ?2"
                ))?;
                let found = st
                    .query_map(params![now, limit as i64], row_from)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                found
            };
            let mut out = Vec::with_capacity(rows.len());
            for r in rows {
                match r {
                    Ok(r) => {
                        tx.execute(
                            "UPDATE push_outbox SET leased_until = ?2 WHERE id = ?1",
                            params![r.id, now + lease_ms],
                        )?;
                        out.push(r);
                    }
                    Err(id) => {
                        tx.execute("DELETE FROM push_outbox WHERE id = ?1", params![id])?;
                    }
                }
            }
            tx.commit()?;
            Ok(out)
        })
    }

    /// Queued rows past their expiry are dropped unsent. Returns how many.
    pub fn outbox_drop_expired(&self) -> Result<usize> {
        self.with(|c| {
            c.execute(
                "DELETE FROM push_outbox WHERE state = 0 AND expires_at <= ?1",
                params![now_ms()],
            )
        })
    }

    pub fn outbox_mark_sent(&self, id: i64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE push_outbox SET state = 1, sent_at = ?2, leased_until = 0, last_error = NULL WHERE id = ?1",
                params![id, now_ms()],
            )
            .map(|_| ())
        })
    }

    /// Try again at `next_attempt_at`, with the error on the row for the admin view.
    pub fn outbox_retry_later(&self, id: i64, next_attempt_at: i64, error: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE push_outbox SET attempts = attempts + 1, next_attempt_at = ?2, leased_until = 0, last_error = ?3
                 WHERE id = ?1",
                params![id, next_attempt_at, error],
            )
            .map(|_| ())
        })
    }

    pub fn outbox_drop(&self, id: i64) -> Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM push_outbox WHERE id = ?1", params![id])
                .map(|_| ())
        })
    }

    /// The device's token is dead: nothing queued for it can be delivered.
    pub fn outbox_drop_device(&self, device_id: &str) -> Result<usize> {
        self.with(|c| {
            c.execute(
                "DELETE FROM push_outbox WHERE device_id = ?1 AND state = 0",
                params![device_id],
            )
        })
    }

    /// When the next queued row becomes due (`None` = nothing queued).
    pub fn outbox_next_due(&self) -> Result<Option<i64>> {
        self.with(|c| {
            c.query_row(
                "SELECT MIN(next_attempt_at) FROM push_outbox WHERE state = 0",
                [],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()
            .map(Option::flatten)
        })
    }

    pub fn outbox_counts(&self) -> Result<OutboxCounts> {
        self.with(|c| {
            let now = now_ms();
            let queued: i64 = c.query_row(
                "SELECT COUNT(*) FROM push_outbox WHERE state = 0",
                [],
                |r| r.get(0),
            )?;
            let due: i64 = c.query_row(
                "SELECT COUNT(*) FROM push_outbox WHERE state = 0 AND next_attempt_at <= ?1 AND leased_until <= ?1",
                params![now],
                |r| r.get(0),
            )?;
            let in_flight: i64 = c.query_row(
                "SELECT COUNT(*) FROM push_outbox WHERE state = 0 AND leased_until > ?1",
                params![now],
                |r| r.get(0),
            )?;
            let sent_recent: i64 = c.query_row(
                "SELECT COUNT(*) FROM push_outbox WHERE state = 1 AND sent_at >= ?1",
                params![now - 24 * 3_600_000],
                |r| r.get(0),
            )?;
            Ok(OutboxCounts {
                queued,
                due,
                in_flight,
                sent_recent,
            })
        })
    }

    /// Every queued row, oldest first.
    #[cfg(test)]
    pub fn outbox_queued(&self) -> Result<Vec<OutboxRow>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {ROW_COLS} FROM push_outbox WHERE state = 0 ORDER BY id"
            ))?;
            let rows = st
                .query_map([], row_from)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows.into_iter().filter_map(Result::ok).collect())
        })
    }

    /// Lift every lease (a fresh process owns the queue alone).
    pub fn outbox_release_leases(&self) -> Result<usize> {
        self.with(|c| {
            c.execute(
                "UPDATE push_outbox SET leased_until = 0 WHERE state = 0 AND leased_until > 0",
                [],
            )
        })
    }

    /// Tests: make every queued row due now.
    #[cfg(test)]
    pub fn outbox_force_due(&self) -> Result<usize> {
        self.with(|c| {
            c.execute(
                "UPDATE push_outbox SET next_attempt_at = ?1, leased_until = 0 WHERE state = 0",
                params![now_ms()],
            )
        })
    }

    /// Tests: every queued row is past its expiry.
    #[cfg(test)]
    pub fn outbox_expire_all(&self) -> Result<usize> {
        self.with(|c| {
            c.execute(
                "UPDATE push_outbox SET expires_at = ?1 WHERE state = 0",
                params![now_ms() - 1],
            )
        })
    }
}
