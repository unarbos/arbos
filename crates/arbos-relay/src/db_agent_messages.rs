//! Storage for agent-to-agent messages and the "Who may message X" grants
//! (`agent_messages::SCHEMA`, applied from `Db::open`).

use anyhow::Result;
use rusqlite::{params, OptionalExtension};

use arbos_core::ids::now_ms;

use crate::db::Db;

#[derive(Debug, Clone)]
pub struct AgentMessageRow {
    pub from_project: String,
    pub to_project: String,
    pub text: String,
    pub trace_id: String,
    pub depth: u32,
    pub chat_id: Option<String>,
    pub message_id: Option<String>,
    pub replied_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct MessageGrant {
    pub grantee: String,
    pub enabled: bool,
    pub note: String,
    pub created_at: i64,
    pub last_at: Option<i64>,
}

impl Db {
    #[allow(clippy::too_many_arguments)]
    pub fn record_agent_message(
        &self,
        id: &str,
        from: &str,
        to: &str,
        text: &str,
        trace_id: &str,
        depth: u32,
        chat_id: Option<&str>,
        message_id: Option<&str>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO agent_messages(id, from_project, to_project, text, trace_id, depth, chat_id, message_id, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![id, from, to, text, trace_id, depth, chat_id, message_id, now_ms()],
            )
            .map(|_| ())
        })
    }

    /// Messages A → B in the last `window_ms` (the per-pair cap; `paused` markers count too).
    pub fn agent_messages_in_window(&self, from: &str, to: &str, window_ms: i64) -> Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM agent_messages WHERE from_project = ?1 AND to_project = ?2 AND created_at > ?3 AND trace_id != 'paused'",
                params![from, to, now_ms() - window_ms],
                |r| r.get(0),
            )
        })
    }

    pub fn agent_message(&self, id: &str) -> Result<Option<AgentMessageRow>> {
        self.with(|c| {
            c.query_row(
                "SELECT from_project, to_project, text, trace_id, depth, chat_id, message_id, replied_at
                 FROM agent_messages WHERE id = ?1",
                params![id],
                |r| {
                    Ok(AgentMessageRow {
                        from_project: r.get(0)?,
                        to_project: r.get(1)?,
                        text: r.get(2)?,
                        trace_id: r.get(3)?,
                        depth: r.get::<_, i64>(4)? as u32,
                        chat_id: r.get(5)?,
                        message_id: r.get(6)?,
                        replied_at: r.get(7)?,
                    })
                },
            )
            .optional()
        })
    }

    pub fn mark_agent_message_replied(&self, id: &str, status: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE agent_messages SET replied_at = ?2, reply_status = ?3 WHERE id = ?1",
                params![id, now_ms(), status],
            )
            .map(|_| ())
        })
    }

    pub fn message_grants(&self, project_id: &str) -> Result<Vec<MessageGrant>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT grantee, enabled, note, created_at, last_at FROM agent_message_grants WHERE project_id = ?1 ORDER BY created_at",
            )?;
            let rows = st
                .query_map(params![project_id], |r| {
                    Ok(MessageGrant {
                        grantee: r.get(0)?,
                        enabled: r.get::<_, i64>(1)? != 0,
                        note: r.get(2)?,
                        created_at: r.get(3)?,
                        last_at: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    pub fn message_grant_enabled(&self, project_id: &str, grantee: &str) -> Result<bool> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT enabled FROM agent_message_grants WHERE project_id = ?1 AND grantee = ?2",
                params![project_id, grantee],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .is_some_and(|e| e != 0))
        })
    }

    /// Projects that granted any of `grantees` (enabled rows only).
    pub fn projects_granting(&self, grantees: &[String]) -> Result<Vec<String>> {
        self.with(|c| {
            let mut out = Vec::new();
            let mut st = c.prepare(
                "SELECT project_id FROM agent_message_grants WHERE grantee = ?1 AND enabled = 1",
            )?;
            for g in grantees {
                let ids = st
                    .query_map(params![g], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for id in ids {
                    if !out.contains(&id) {
                        out.push(id);
                    }
                }
            }
            Ok(out)
        })
    }

    pub fn upsert_message_grant(
        &self,
        project_id: &str,
        grantee: &str,
        enabled: bool,
        note: Option<&str>,
        added_by: &str,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO agent_message_grants(project_id, grantee, enabled, note, added_by, created_at)
                 VALUES (?1, ?2, ?3, COALESCE(?4, ''), ?5, ?6)
                 ON CONFLICT(project_id, grantee) DO UPDATE SET enabled = excluded.enabled,
                   note = COALESCE(?4, agent_message_grants.note)",
                params![project_id, grantee, enabled as i64, note, added_by, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn delete_message_grant(&self, project_id: &str, grantee: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM agent_message_grants WHERE project_id = ?1 AND grantee = ?2",
                params![project_id, grantee],
            )
            .map(|_| ())
        })
    }

    /// Stamp `last_at` on whichever grant let a message through.
    pub fn touch_message_grant(&self, project_id: &str, grantees: &[String]) -> Result<()> {
        self.with(|c| {
            for g in grantees {
                c.execute(
                    "UPDATE agent_message_grants SET last_at = ?3 WHERE project_id = ?1 AND grantee = ?2",
                    params![project_id, g, now_ms()],
                )?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::db::Db;

    #[test]
    fn grants_and_pair_window() {
        let dir = std::env::temp_dir().join(format!(
            "arbos-agent-messages-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir.join("relay.db")).expect("db");
        assert!(!db.message_grant_enabled("doug", "user:u2").unwrap());
        db.upsert_message_grant("doug", "user:u2", true, Some("ops"), "u1")
            .unwrap();
        assert!(db.message_grant_enabled("doug", "user:u2").unwrap());
        assert_eq!(
            db.projects_granting(&["user:u2".into()]).unwrap(),
            vec!["doug"]
        );
        db.upsert_message_grant("doug", "user:u2", false, None, "u1")
            .unwrap();
        assert!(!db.message_grant_enabled("doug", "user:u2").unwrap());
        assert_eq!(db.message_grants("doug").unwrap()[0].note, "ops");
        assert!(db
            .projects_granting(&["user:u2".into()])
            .unwrap()
            .is_empty());

        for i in 0..3 {
            db.record_agent_message(&format!("m{i}"), "hjkb", "doug", "hi", "t", 1, None, None)
                .unwrap();
        }
        assert_eq!(
            db.agent_messages_in_window("hjkb", "doug", 3_600_000)
                .unwrap(),
            3
        );
        assert_eq!(
            db.agent_messages_in_window("doug", "hjkb", 3_600_000)
                .unwrap(),
            0
        );
        let row = db.agent_message("m1").unwrap().unwrap();
        assert_eq!(row.to_project, "doug");
        assert!(row.replied_at.is_none());
        db.mark_agent_message_replied("m1", "done").unwrap();
        assert!(db
            .agent_message("m1")
            .unwrap()
            .unwrap()
            .replied_at
            .is_some());
        db.delete_message_grant("doug", "user:u2").unwrap();
        assert!(db.message_grants("doug").unwrap().is_empty());
    }
}
