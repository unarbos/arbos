//! Relay-side rows a debugging reader merges with the box's window (`inspect.rs`): model usage,
//! voice-call membership, routine runs and the mirrored agentd audit. Every query is bounded by
//! `at > after_ms` and `LIMIT`, on an indexed column, oldest first.

use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use crate::db::Db;

impl Db {
    /// Metered model calls of a project after `after_ms` (`usage` table): api, model, tokens,
    /// cost and latency — the spend side of every turn.
    pub fn usage_rows_after(
        &self,
        project_id: &str,
        after_ms: i64,
        limit: i64,
    ) -> Result<Vec<Value>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT id, at, api, model, input, output, cache_read, cache_write, usd, latency_ms, ttfb_ms
                 FROM usage WHERE project_id = ?1 AND at > ?2 ORDER BY at ASC, id ASC LIMIT ?3",
            )?;
            let rows = st.query_map(params![project_id, after_ms, limit], |r| {
                Ok(json!({
                    "id": r.get::<_, i64>(0)?, "at": r.get::<_, i64>(1)?, "api": r.get::<_, String>(2)?,
                    "model": r.get::<_, String>(3)?, "input": r.get::<_, i64>(4)?, "output": r.get::<_, i64>(5)?,
                    "cache_read": r.get::<_, i64>(6)?, "cache_write": r.get::<_, i64>(7)?, "usd": r.get::<_, f64>(8)?,
                    "latency_ms": r.get::<_, i64>(9)?, "ttfb_ms": r.get::<_, Option<i64>>(10)?,
                }))
            })?;
            rows.collect()
        })
    }

    /// Calls an agent joined after `after_ms` (`call_agents` ⋈ `calls`): when it joined and left,
    /// which chat, who started the call (#1436 group-call subscribe).
    pub fn call_agent_rows_after(
        &self,
        project_id: &str,
        after_ms: i64,
        limit: i64,
    ) -> Result<Vec<Value>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT a.call_id, a.joined_at, a.left_at, c.chat_id, c.kind, c.started_by, c.started_at, c.ended_at, c.end_reason
                 FROM call_agents a LEFT JOIN calls c ON c.id = a.call_id
                 WHERE a.project_id = ?1 AND a.joined_at > ?2 ORDER BY a.joined_at ASC LIMIT ?3",
            )?;
            let rows = st.query_map(params![project_id, after_ms, limit], |r| {
                Ok(json!({
                    "call_id": r.get::<_, String>(0)?, "joined_at": r.get::<_, i64>(1)?, "left_at": r.get::<_, Option<i64>>(2)?,
                    "chat_id": r.get::<_, Option<String>>(3)?, "kind": r.get::<_, Option<String>>(4)?,
                    "started_by": r.get::<_, Option<String>>(5)?, "started_at": r.get::<_, Option<i64>>(6)?,
                    "ended_at": r.get::<_, Option<i64>>(7)?, "end_reason": r.get::<_, Option<String>>(8)?,
                }))
            })?;
            rows.collect()
        })
    }

    /// Routine runs of a project after `after_ms` (`routine_runs` ⋈ `routines`).
    pub fn routine_run_rows_after(
        &self,
        project_id: &str,
        after_ms: i64,
        limit: i64,
    ) -> Result<Vec<Value>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT r.id, r.routine_id, t.name, t.chat_id, r.started_at, r.finished_at, r.status, r.message_id, r.note
                 FROM routine_runs r JOIN routines t ON t.id = r.routine_id
                 WHERE t.project_id = ?1 AND r.started_at > ?2 ORDER BY r.started_at ASC LIMIT ?3",
            )?;
            let rows = st.query_map(params![project_id, after_ms, limit], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?, "routine_id": r.get::<_, String>(1)?, "routine": r.get::<_, String>(2)?,
                    "chat_id": r.get::<_, String>(3)?, "started_at": r.get::<_, i64>(4)?, "finished_at": r.get::<_, Option<i64>>(5)?,
                    "status": r.get::<_, String>(6)?, "message_id": r.get::<_, Option<String>>(7)?, "note": r.get::<_, Option<String>>(8)?,
                }))
            })?;
            rows.collect()
        })
    }

    /// Newest app version seen per device of a user (`device_versions`): `device_id →
    /// {platform, app_version, build, first_seen_at, last_seen_at}`.
    pub fn device_versions_of_user(&self, user_id: &str) -> Result<Vec<Value>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT device_id, platform, app_version, build, first_seen_at, last_seen_at FROM device_versions
                 WHERE user_id = ?1 ORDER BY last_seen_at DESC",
            )?;
            let rows = st.query_map(params![user_id], |r| {
                Ok(json!({
                    "device_id": r.get::<_, String>(0)?, "platform": r.get::<_, String>(1)?,
                    "app_version": r.get::<_, String>(2)?, "build": r.get::<_, String>(3)?,
                    "first_seen_at": r.get::<_, i64>(4)?, "last_seen_at": r.get::<_, i64>(5)?,
                }))
            })?;
            rows.collect()
        })
    }

    /// Device ids of a user with a message-push registration, and with a VoIP one.
    pub fn push_registrations_of_user(&self, user_id: &str) -> Result<(Vec<String>, Vec<String>)> {
        self.read(|c| {
            let mut st = c.prepare("SELECT device_id FROM push_tokens WHERE user_id = ?1")?;
            let push: Vec<String> = st
                .query_map(params![user_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut st = c.prepare("SELECT device_id FROM voip_tokens WHERE user_id = ?1")?;
            let voip: Vec<String> = st
                .query_map(params![user_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok((push, voip))
        })
    }

    /// The read-cursor facts of one user in one chat (#1438): the per-user row cursor
    /// (`chat_user_cursor`), the per-user event mark unread is counted from (`chat_user_reads`),
    /// and every device's older per-device mark (`people_reads`).
    pub fn read_state(&self, user_id: &str, chat_id: &str, device_ids: &[String]) -> Result<Value> {
        self.read(|c| {
            let cursor: Option<(i64, i64)> = c
                .query_row(
                    "SELECT seq, at FROM chat_user_cursor WHERE user_id = ?1 AND chat_id = ?2",
                    params![user_id, chat_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let read: Option<(i64, i64)> = c
                .query_row(
                    "SELECT event_id, at FROM chat_user_reads WHERE user_id = ?1 AND chat_id = ?2",
                    params![user_id, chat_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let mut st = c.prepare(
                "SELECT device_id, seq, at, hidden FROM people_reads WHERE chat_id = ?1",
            )?;
            let devices: Vec<Value> = st
                .query_map(params![chat_id], |r| {
                    Ok(json!({
                        "device_id": r.get::<_, String>(0)?, "seq": r.get::<_, i64>(1)?,
                        "at": r.get::<_, i64>(2)?, "hidden": r.get::<_, i64>(3)? != 0,
                    }))
                })?
                .filter_map(|r| r.ok())
                .filter(|v| {
                    device_ids
                        .iter()
                        .any(|d| v["device_id"].as_str() == Some(d.as_str()))
                })
                .collect();
            Ok(json!({
                "user_cursor": cursor.map(|(seq, at)| json!({ "seq": seq, "at": at })),
                "read_mark": read.map(|(event_id, at)| json!({ "event_id": event_id, "at": at })),
                "device_marks": devices,
            }))
        })
    }

    /// The last `limit` announced message events of a chat (what unread is counted over).
    pub fn chat_events_tail(&self, chat_id: &str, limit: i64) -> Result<Vec<Value>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT id, message_id, sender_user, created_at, project_id FROM chat_events WHERE chat_id = ?1 ORDER BY id DESC LIMIT ?2",
            )?;
            let mut rows: Vec<Value> = st
                .query_map(params![chat_id, limit], |r| {
                    Ok(json!({
                        "event_id": r.get::<_, i64>(0)?, "message_id": r.get::<_, String>(1)?,
                        "sender_user": r.get::<_, Option<String>>(2)?, "created_at": r.get::<_, i64>(3)?,
                        "project_id": r.get::<_, Option<String>>(4)?,
                    }))
                })?
                .collect::<rusqlite::Result<_>>()?;
            rows.reverse();
            Ok(rows)
        })
    }

    /// The agentd audit entries the box mirrored to the relay (`POST /projects/{id}/audit`) after
    /// `after_ms` — the fallback when the box itself cannot be reached.
    pub fn audit_rows_after(
        &self,
        project_id: &str,
        after_ms: i64,
        limit: i64,
    ) -> Result<Vec<Value>> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT id, kind, entry, at FROM audit WHERE project_id = ?1 AND at > ?2 ORDER BY at ASC, id ASC LIMIT ?3",
            )?;
            let rows = st.query_map(params![project_id, after_ms, limit], |r| {
                let raw: String = r.get(2)?;
                Ok(json!({
                    "id": r.get::<_, i64>(0)?, "kind": r.get::<_, String>(1)?,
                    "entry": serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null), "at": r.get::<_, i64>(3)?,
                }))
            })?;
            rows.collect()
        })
    }
}
