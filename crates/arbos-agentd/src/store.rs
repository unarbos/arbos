//! Box storage: SQLite + FTS5 (design §5, §6.3, §6.7; memory-decision-models §5.6).
//! T3 history is append-only and searchable; memory is never hard-deleted (D7).

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use arbos_core::ids::{now_ms, ulid};

#[derive(Clone)]
pub struct Store(Arc<Mutex<Connection>>);

/// `(cursor, snapshot, acked_vv)` of one room.
pub type RoomDocRow = (i64, Option<Vec<u8>>, Option<Vec<u8>>);

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

-- Room cursor + snapshot committed together (crdt-sync §2.3).
CREATE TABLE IF NOT EXISTS room_docs (
  room TEXT PRIMARY KEY, cursor INTEGER NOT NULL DEFAULT 0, snapshot BLOB, acked_vv BLOB, updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS chats (
  id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT 'New chat', is_pinned INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL, last_message_at INTEGER,
  compaction_summary TEXT, compacted_through TEXT, compaction_version INTEGER NOT NULL DEFAULT 0,
  pi_session_path TEXT
);

CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY, subagent_id TEXT, path TEXT, cwd TEXT, model TEXT, thinking_level TEXT,
  last_cursor TEXT, compactions INTEGER NOT NULL DEFAULT 0, context_pct INTEGER NOT NULL DEFAULT 0,
  harness TEXT NOT NULL, role TEXT NOT NULL, harness_version TEXT, created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS subagents (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, purpose TEXT NOT NULL, summary TEXT NOT NULL DEFAULT '',
  tags TEXT NOT NULL DEFAULT '[]', last_seen_at INTEGER, status TEXT NOT NULL DEFAULT 'idle',
  turns INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, last_turn_at INTEGER, retired_reason TEXT,
  harness TEXT NOT NULL, handles TEXT NOT NULL DEFAULT '[]', chat_id TEXT
);

-- T3: one row per message / tool call / tool output / compaction / memory event.
CREATE TABLE IF NOT EXISTS t3 (
  id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, subagent_id TEXT, session_id TEXT, kind TEXT NOT NULL,
  role TEXT, at INTEGER NOT NULL, text TEXT NOT NULL, source TEXT, size INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS t3_thread_at ON t3(thread_id, at);
CREATE INDEX IF NOT EXISTS t3_at ON t3(at);
CREATE VIRTUAL TABLE IF NOT EXISTS t3_fts USING fts5(text, id UNINDEXED, thread_id UNINDEXED, kind UNINDEXED, tokenize = 'porter unicode61');

CREATE TABLE IF NOT EXISTS memory (
  id TEXT PRIMARY KEY, tier TEXT NOT NULL, thread_id TEXT, kind TEXT NOT NULL, scope TEXT NOT NULL,
  text TEXT NOT NULL, source TEXT, valid_at INTEGER, invalid_at INTEGER, created_at INTEGER NOT NULL, expired_at INTEGER,
  supersedes TEXT NOT NULL DEFAULT '[]', confidence REAL NOT NULL DEFAULT 0.5, importance INTEGER NOT NULL DEFAULT 5,
  last_accessed_at INTEGER, access_count INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS memory_tier_live ON memory(tier, expired_at);
CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(text, id UNINDEXED, tier UNINDEXED, tokenize = 'porter unicode61');

CREATE TABLE IF NOT EXISTS memory_decision_log (
  id TEXT PRIMARY KEY, side TEXT NOT NULL, event_id TEXT, gate TEXT, action TEXT, target_ids TEXT,
  reason TEXT, confidence REAL, model TEXT, latency_ms INTEGER, usage TEXT, created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS handles (
  id TEXT PRIMARY KEY, chat_id TEXT, session_id TEXT, kind TEXT NOT NULL, chars INTEGER NOT NULL, lines INTEGER NOT NULL,
  path TEXT NOT NULL, note TEXT NOT NULL, blob_key TEXT, created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS scratch (
  chat_id TEXT NOT NULL, name TEXT NOT NULL, value TEXT NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY (chat_id, name)
);

CREATE TABLE IF NOT EXISTS turn_metrics (
  id TEXT PRIMARY KEY, chat_id TEXT NOT NULL, message_id TEXT NOT NULL, route TEXT, ttft_ms INTEGER, route_ms INTEGER,
  total_ms INTEGER, tool_calls INTEGER NOT NULL DEFAULT 0, escalated INTEGER NOT NULL DEFAULT 0, usage TEXT, created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS asks (
  id TEXT PRIMARY KEY, chat_id TEXT, session_id TEXT, question TEXT NOT NULL, answered INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL
);

-- Approval cards (feature approve): the doc part is the source of truth for clients; this row
-- survives a restart so a late decision still dispatches.
CREATE TABLE IF NOT EXISTS approvals (
  id TEXT PRIMARY KEY, chat_id TEXT NOT NULL, message_id TEXT NOT NULL, title TEXT NOT NULL, risk TEXT NOT NULL,
  payload TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'pending', created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL,
  decided_at INTEGER, decided_by TEXT, reason TEXT
);

-- Approved drafts with no connector to deliver them (feature approve, until connectors land).
CREATE TABLE IF NOT EXISTS outbox (
  id TEXT PRIMARY KEY, approval_id TEXT NOT NULL, channel TEXT NOT NULL, recipient TEXT NOT NULL, subject TEXT,
  body TEXT NOT NULL, created_at INTEGER NOT NULL
);

-- Background jobs the main agent starts with `start_job` (duplex design §2.3): one row per job, from
-- the ask to its delivery. `subagent_id` names the harness session that runs it.
CREATE TABLE IF NOT EXISTS jobs (
  id TEXT PRIMARY KEY, chat_id TEXT NOT NULL, ask_message_id TEXT NOT NULL, subagent_id TEXT,
  title TEXT NOT NULL, kind TEXT NOT NULL, brief TEXT NOT NULL, touches TEXT NOT NULL DEFAULT '[]',
  state TEXT NOT NULL, last_step TEXT NOT NULL DEFAULT '', last_step_at INTEGER,
  created_at INTEGER NOT NULL, started_at INTEGER, ended_at INTEGER, error TEXT,
  delivered_key TEXT, restarts INTEGER NOT NULL DEFAULT 0
);
"#;

/// One background job (duplex design §2.3). States: `queued` (waiting for a slot), `running`,
/// `done`, `failed`, `cancelled`, `interrupted` (the box restarted under it).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRow {
    pub id: String,
    pub chat_id: String,
    pub ask_message_id: String,
    pub subagent_id: Option<String>,
    pub title: String,
    pub kind: String,
    pub brief: String,
    pub touches: Vec<String>,
    pub state: String,
    pub last_step: String,
    pub last_step_at: Option<i64>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub error: Option<String>,
    pub delivered_key: Option<String>,
    pub restarts: i64,
}

impl JobRow {
    pub fn is_open(&self) -> bool {
        matches!(self.state.as_str(), "queued" | "running")
    }

    /// Seconds since the job started (or was created while queued).
    pub fn elapsed_s(&self) -> i64 {
        let from = self.started_at.unwrap_or(self.created_at);
        (now_ms() - from).max(0) / 1000
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRow {
    pub id: String,
    pub title: String,
    pub is_pinned: bool,
    pub created_at: i64,
    /// Last turn in this chat ([`Store::touch_chat`]); `None` until it has had one. The boot
    /// room budget opens the most recently used chats first (#2367).
    pub last_message_at: Option<i64>,
    pub compaction_summary: Option<String>,
    pub compacted_through: Option<String>,
    pub compaction_version: i64,
    pub pi_session_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentRow {
    pub id: String,
    pub name: String,
    pub purpose: String,
    pub summary: String,
    pub tags: Value,
    pub last_seen_at: Option<i64>,
    pub status: String,
    pub turns: i64,
    pub created_at: i64,
    pub last_turn_at: Option<i64>,
    pub harness: String,
    pub chat_id: Option<String>,
    /// Last compact progress line (`working` / `step: …` / `done` / `failed`), for the working set.
    pub last_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct T3Row {
    pub id: String,
    pub thread_id: String,
    pub subagent_id: Option<String>,
    pub session_id: Option<String>,
    pub kind: String,
    pub role: Option<String>,
    pub at: i64,
    pub text: String,
    pub size: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRow {
    pub id: String,
    pub tier: String,
    pub thread_id: Option<String>,
    pub kind: String,
    pub scope: String,
    pub text: String,
    pub valid_at: Option<i64>,
    pub invalid_at: Option<i64>,
    pub created_at: i64,
    pub expired_at: Option<i64>,
    pub supersedes: Value,
    pub confidence: f64,
    pub importance: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandleRow {
    pub id: String,
    pub chat_id: Option<String>,
    pub kind: String,
    pub chars: i64,
    pub lines: i64,
    pub path: String,
    pub note: String,
    pub created_at: i64,
}

/// Plaintext values that passed through this process (vault logins typed into the browser, F8).
/// They are kept only so that `redact` can strip them wherever they resurface (page text, tool
/// output); nothing reads them back.
fn live_secrets() -> &'static std::sync::Mutex<Vec<String>> {
    static LIVE: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> = std::sync::OnceLock::new();
    LIVE.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Register a value for write-time redaction (D19). Short values are ignored: they would blank
/// ordinary words.
pub fn redact_live_value(value: &str) {
    if value.len() < 6 {
        return;
    }
    let mut v = live_secrets().lock().expect("live secrets");
    if !v.iter().any(|x| x == value) {
        v.push(value.to_owned());
    }
}

/// Write-time redaction before T3 append (D19): tokens, keys, Authorization headers, `.env`-style
/// values, plus every live vault value registered with `redact_live_value`.
pub fn redact(text: &str) -> String {
    let mut out = text.to_owned();
    {
        let live = live_secrets().lock().expect("live secrets");
        for v in live.iter() {
            if out.contains(v.as_str()) {
                out = out.replace(v.as_str(), "[REDACTED:vault]");
            }
        }
    }
    arbos_core::redact::redact_patterns(&out)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let c = Connection::open(path)?;
        // secure_delete: overwritten and deleted content is zeroed in the page, so a scrubbed
        // T3 row or a re-saved room snapshot leaves no old bytes behind in the file.
        c.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA secure_delete=ON;",
        )?;
        c.execute_batch(SCHEMA)?;
        // Additive migration for stores created before demo-review item 2.
        let has_last_status: bool = c
            .prepare("PRAGMA table_info(subagents)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|n| n == "last_status");
        if !has_last_status {
            c.execute_batch(
                "ALTER TABLE subagents ADD COLUMN last_status TEXT NOT NULL DEFAULT ''",
            )?;
        }
        // Additive migration: question widgets (feature F4) keep the widget JSON per ask.
        let has_widget: bool = c
            .prepare("PRAGMA table_info(asks)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|n| n == "widget");
        if !has_widget {
            c.execute_batch("ALTER TABLE asks ADD COLUMN widget TEXT")?;
        }
        Ok(Self(Arc::new(Mutex::new(c))))
    }

    pub(crate) fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        let c = self.0.lock().expect("store lock");
        Ok(f(&c)?)
    }

    /// Run additive DDL (a module's own tables, `CREATE … IF NOT EXISTS`).
    pub(crate) fn exec_batch(&self, sql: &str) -> Result<()> {
        self.with(|c| c.execute_batch(sql))
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()
        })
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.with(|c| c.execute("INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, value]).map(|_| ()))
    }

    // ---- room docs ----------------------------------------------------------------------------

    pub fn room_doc(&self, room: &str) -> Result<Option<RoomDocRow>> {
        self.with(|c| {
            c.query_row(
                "SELECT cursor, snapshot, acked_vv FROM room_docs WHERE room = ?1",
                params![room],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
        })
    }

    /// Cursor and snapshot land in one transaction so a restored box never disagrees with its cursor.
    pub fn save_room_doc(
        &self,
        room: &str,
        cursor: i64,
        snapshot: &[u8],
        acked_vv: &[u8],
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO room_docs(room, cursor, snapshot, acked_vv, updated_at) VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(room) DO UPDATE SET cursor=excluded.cursor, snapshot=excluded.snapshot, acked_vv=excluded.acked_vv, updated_at=excluded.updated_at",
                params![room, cursor, snapshot, acked_vv, now_ms()],
            )
            .map(|_| ())
        })
    }

    // ---- chats --------------------------------------------------------------------------------

    pub fn upsert_chat(
        &self,
        id: &str,
        title: &str,
        is_pinned: bool,
        created_at: i64,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO chats(id, title, is_pinned, created_at) VALUES (?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET title = excluded.title, is_pinned = excluded.is_pinned",
                params![id, title, is_pinned as i64, created_at],
            )
            .map(|_| ())
        })
    }

    pub fn chats(&self) -> Result<Vec<ChatRow>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT id, title, is_pinned, created_at, compaction_summary, compacted_through, compaction_version, pi_session_path, last_message_at FROM chats ORDER BY created_at")?;
            let rows = st.query_map([], |r| {
                Ok(ChatRow {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    is_pinned: r.get::<_, i64>(2)? != 0,
                    created_at: r.get(3)?,
                    compaction_summary: r.get(4)?,
                    compacted_through: r.get(5)?,
                    compaction_version: r.get(6)?,
                    pi_session_path: r.get(7)?,
                    last_message_at: r.get(8)?,
                })
            })?;
            rows.collect()
        })
    }

    pub fn chat(&self, id: &str) -> Result<Option<ChatRow>> {
        Ok(self.chats()?.into_iter().find(|c| c.id == id))
    }

    pub fn set_chat_session_path(&self, id: &str, path: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE chats SET pi_session_path = ?2 WHERE id = ?1",
                params![id, path],
            )
            .map(|_| ())
        })
    }

    pub fn set_chat_compaction(&self, id: &str, summary: &str, through: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE chats SET compaction_summary = ?2, compacted_through = ?3, compaction_version = compaction_version + 1 WHERE id = ?1",
                params![id, summary, through],
            )
            .map(|_| ())
        })
    }

    pub fn touch_chat(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE chats SET last_message_at = ?2 WHERE id = ?1",
                params![id, now_ms()],
            )
            .map(|_| ())
        })
    }

    // ---- sessions and subagents ---------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_session(
        &self,
        id: &str,
        subagent_id: Option<&str>,
        path: Option<&str>,
        cwd: &str,
        model: &str,
        harness: &str,
        role: &str,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO sessions(id, subagent_id, path, cwd, model, harness, role, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(id) DO UPDATE SET path = COALESCE(excluded.path, sessions.path), model = excluded.model",
                params![id, subagent_id, path, cwd, model, harness, role, now_ms()],
            )
            .map(|_| ())
        })
    }

    /// The model a session was started with (`upsert_session`), for a resume on the same model.
    pub fn session_model(&self, id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT model FROM sessions WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
        })
    }

    pub fn session_cursor(&self, id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT last_cursor FROM sessions WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
            .map(|o: Option<Option<String>>| o.flatten())
        })
    }

    pub fn set_session_cursor(&self, id: &str, cursor: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE sessions SET last_cursor = ?2 WHERE id = ?1",
                params![id, cursor],
            )
            .map(|_| ())
        })
    }

    pub fn session_path(&self, id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT path FROM sessions WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
            .map(|o: Option<Option<String>>| o.flatten())
        })
    }

    pub fn bump_session_compactions(&self, id: &str) -> Result<i64> {
        self.with(|c| {
            c.execute(
                "UPDATE sessions SET compactions = compactions + 1 WHERE id = ?1",
                params![id],
            )?;
            c.query_row(
                "SELECT compactions FROM sessions WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
        })
    }

    pub fn insert_subagent(
        &self,
        id: &str,
        name: &str,
        purpose: &str,
        harness: &str,
        chat_id: &str,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO subagents(id, name, purpose, harness, chat_id, created_at, status) VALUES (?1,?2,?3,?4,?5,?6,'idle')",
                params![id, name, purpose, harness, chat_id, now_ms()],
            )
            .map(|_| ())
        })
    }

    fn row_to_subagent(r: &rusqlite::Row<'_>) -> rusqlite::Result<SubagentRow> {
        Ok(SubagentRow {
            id: r.get(0)?,
            name: r.get(1)?,
            purpose: r.get(2)?,
            summary: r.get(3)?,
            tags: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(json!([])),
            last_seen_at: r.get(5)?,
            status: r.get(6)?,
            turns: r.get(7)?,
            created_at: r.get(8)?,
            last_turn_at: r.get(9)?,
            harness: r.get(10)?,
            chat_id: r.get(11)?,
            last_status: r.get(12)?,
        })
    }

    const SUBAGENT_COLS: &'static str = "id, name, purpose, summary, tags, last_seen_at, status, turns, created_at, last_turn_at, harness, chat_id, last_status";

    pub fn subagents(&self) -> Result<Vec<SubagentRow>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM subagents ORDER BY COALESCE(last_turn_at, created_at) DESC",
                Self::SUBAGENT_COLS
            ))?;
            let rows = st.query_map([], Self::row_to_subagent)?;
            rows.collect()
        })
    }

    pub fn subagent(&self, id: &str) -> Result<Option<SubagentRow>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM subagents WHERE id = ?1",
                    Self::SUBAGENT_COLS
                ),
                params![id],
                Self::row_to_subagent,
            )
            .optional()
        })
    }

    pub fn set_subagent_status(&self, id: &str, status: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE subagents SET status = ?2, last_turn_at = CASE WHEN ?2 = 'running' THEN ?3 ELSE last_turn_at END,
                        turns = turns + CASE WHEN ?2 = 'running' THEN 1 ELSE 0 END, last_seen_at = ?3 WHERE id = ?1",
                params![id, status, now_ms()],
            )
            .map(|_| ())
        })
    }

    /// Most recent subagent with this name (the router sometimes passes the name as `subagent_id`).
    pub fn subagent_by_name(&self, name: &str) -> Result<Option<SubagentRow>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM subagents WHERE name = ?1 ORDER BY COALESCE(last_turn_at, created_at) DESC LIMIT 1",
                    Self::SUBAGENT_COLS
                ),
                params![name],
                Self::row_to_subagent,
            )
            .optional()
        })
    }

    // ---- jobs (duplex design §2.3) ------------------------------------------------------------

    const JOB_COLS: &'static str = "id, chat_id, ask_message_id, subagent_id, title, kind, brief, touches, state, last_step, last_step_at, created_at, started_at, ended_at, error, delivered_key, restarts";

    fn row_to_job(r: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
        Ok(JobRow {
            id: r.get(0)?,
            chat_id: r.get(1)?,
            ask_message_id: r.get(2)?,
            subagent_id: r.get(3)?,
            title: r.get(4)?,
            kind: r.get(5)?,
            brief: r.get(6)?,
            touches: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default(),
            state: r.get(8)?,
            last_step: r.get(9)?,
            last_step_at: r.get(10)?,
            created_at: r.get(11)?,
            started_at: r.get(12)?,
            ended_at: r.get(13)?,
            error: r.get(14)?,
            delivered_key: r.get(15)?,
            restarts: r.get(16)?,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_job(
        &self,
        id: &str,
        chat_id: &str,
        ask_message_id: &str,
        title: &str,
        kind: &str,
        brief: &str,
        touches: &[String],
    ) -> Result<()> {
        let touches = serde_json::to_string(touches).unwrap_or_else(|_| "[]".into());
        self.with(|c| {
            c.execute(
                "INSERT INTO jobs(id, chat_id, ask_message_id, title, kind, brief, touches, state, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'queued',?8)",
                params![id, chat_id, ask_message_id, title, kind, brief, touches, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn set_job_subagent(&self, id: &str, subagent_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE jobs SET subagent_id = ?2 WHERE id = ?1",
                params![id, subagent_id],
            )
            .map(|_| ())
        })
    }

    /// Move a job to `state`; `running` stamps `started_at`, a final state stamps `ended_at`.
    pub fn set_job_state(&self, id: &str, state: &str, error: Option<&str>) -> Result<()> {
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "UPDATE jobs SET state = ?2, error = COALESCE(?3, error),
                    started_at = CASE WHEN ?2 = 'running' AND started_at IS NULL THEN ?4 ELSE started_at END,
                    ended_at = CASE WHEN ?2 IN ('done','failed','cancelled','interrupted') THEN ?4 ELSE ended_at END
                 WHERE id = ?1",
                params![id, state, error, now],
            )
            .map(|_| ())
        })
    }

    pub fn set_job_step(&self, id: &str, step: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE jobs SET last_step = ?2, last_step_at = ?3 WHERE id = ?1",
                params![id, step, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn set_job_delivered(&self, id: &str, key: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE jobs SET delivered_key = ?2 WHERE id = ?1",
                params![id, key],
            )
            .map(|_| ())
        })
    }

    pub fn bump_job_restarts(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE jobs SET restarts = restarts + 1 WHERE id = ?1",
                params![id],
            )
            .map(|_| ())
        })
    }

    pub fn job(&self, id: &str) -> Result<Option<JobRow>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {} FROM jobs WHERE id = ?1", Self::JOB_COLS),
                params![id],
                Self::row_to_job,
            )
            .optional()
        })
    }

    /// Jobs of one chat, newest first.
    pub fn jobs_for_chat(&self, chat_id: &str) -> Result<Vec<JobRow>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM jobs WHERE chat_id = ?1 ORDER BY created_at DESC",
                Self::JOB_COLS
            ))?;
            let rows = st.query_map(params![chat_id], Self::row_to_job)?;
            rows.collect()
        })
    }

    /// Every job still `queued` or `running`, oldest first.
    pub fn open_jobs(&self) -> Result<Vec<JobRow>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM jobs WHERE state IN ('queued','running') ORDER BY created_at ASC",
                Self::JOB_COLS
            ))?;
            let rows = st.query_map([], Self::row_to_job)?;
            rows.collect()
        })
    }

    pub fn set_subagent_last_status(&self, id: &str, line: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE subagents SET last_status = ?2, last_seen_at = ?3 WHERE id = ?1",
                params![id, line, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn set_subagent_summary(&self, id: &str, summary: &str, tags: &Value) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE subagents SET summary = ?2, tags = ?3 WHERE id = ?1",
                params![id, summary, tags.to_string()],
            )
            .map(|_| ())
        })
    }

    // ---- T3 -----------------------------------------------------------------------------------

    /// Append one T3 entry (redacted at write time, D19). Returns the entry id.
    #[allow(clippy::too_many_arguments)]
    pub fn t3_append(
        &self,
        thread_id: &str,
        subagent_id: Option<&str>,
        session_id: Option<&str>,
        kind: &str,
        role: Option<&str>,
        text: &str,
        source: Option<&Value>,
    ) -> Result<String> {
        let id = ulid();
        let text = redact(text);
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "INSERT INTO t3(id, thread_id, subagent_id, session_id, kind, role, at, text, source, size) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![id, thread_id, subagent_id, session_id, kind, role, now, text, source.map(|s| s.to_string()), text.len() as i64],
            )?;
            c.execute("INSERT INTO t3_fts(text, id, thread_id, kind) VALUES (?1,?2,?3,?4)", params![text, id, thread_id, kind])?;
            Ok(())
        })?;
        Ok(id)
    }

    pub fn t3_search(
        &self,
        query: &str,
        thread_id: Option<&str>,
        kind: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<(T3Row, f64, String)>> {
        self.t3_search_fts(&fts_query(query), thread_id, kind, limit, offset)
    }

    /// Keyword candidates for the memory tree: any of the content words (OR), ranked by bm25.
    pub fn t3_search_any(&self, words: &[String], limit: i64) -> Result<Vec<(T3Row, f64, String)>> {
        let terms: Vec<String> = words
            .iter()
            .map(|w| format!("\"{}\"", w.replace('"', "")))
            .filter(|w| w.len() > 2)
            .collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        self.t3_search_fts(&terms.join(" OR "), None, None, limit, 0)
    }

    fn t3_search_fts(
        &self,
        q: &str,
        thread_id: Option<&str>,
        kind: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<(T3Row, f64, String)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT t.id, t.thread_id, t.subagent_id, t.session_id, t.kind, t.role, t.at, t.text, t.size, bm25(t3_fts) AS score,
                        snippet(t3_fts, 0, '[', ']', '…', 24)
                 FROM t3_fts JOIN t3 t ON t.id = t3_fts.id
                 WHERE t3_fts MATCH ?1 AND (?2 IS NULL OR t.thread_id = ?2) AND (?3 IS NULL OR t.kind = ?3)
                 ORDER BY score LIMIT ?4 OFFSET ?5",
            )?;
            let rows = st.query_map(params![q, thread_id, kind, limit, offset], |r| {
                Ok((
                    T3Row {
                        id: r.get(0)?,
                        thread_id: r.get(1)?,
                        subagent_id: r.get(2)?,
                        session_id: r.get(3)?,
                        kind: r.get(4)?,
                        role: r.get(5)?,
                        at: r.get(6)?,
                        text: r.get(7)?,
                        size: r.get(8)?,
                    },
                    r.get::<_, f64>(9)?,
                    r.get::<_, String>(10)?,
                ))
            })?;
            rows.collect()
        })
    }

    pub fn t3_get(&self, id: &str) -> Result<Option<T3Row>> {
        self.with(|c| {
            c.query_row("SELECT id, thread_id, subagent_id, session_id, kind, role, at, text, size FROM t3 WHERE id = ?1", params![id], |r| {
                Ok(T3Row { id: r.get(0)?, thread_id: r.get(1)?, subagent_id: r.get(2)?, session_id: r.get(3)?, kind: r.get(4)?, role: r.get(5)?, at: r.get(6)?, text: r.get(7)?, size: r.get(8)? })
            })
            .optional()
        })
    }

    /// Verbatim turns of a thread in order; `from_id`/`to_id` bound the range, `limit` from the end when unbounded.
    /// History paging: the `limit` entries strictly before `before` (exclusive, so pages never
    /// repeat the cursor row), oldest first.
    pub fn t3_thread_before(
        &self,
        thread_id: &str,
        before: Option<&str>,
        limit: i64,
    ) -> Result<Vec<T3Row>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, thread_id, subagent_id, session_id, kind, role, at, text, size FROM (
                    SELECT * FROM t3 WHERE thread_id = ?1 AND (?2 IS NULL OR id < ?2) ORDER BY id DESC LIMIT ?3
                 ) ORDER BY id ASC",
            )?;
            let rows = st.query_map(params![thread_id, before, limit], |r| {
                Ok(T3Row { id: r.get(0)?, thread_id: r.get(1)?, subagent_id: r.get(2)?, session_id: r.get(3)?, kind: r.get(4)?, role: r.get(5)?, at: r.get(6)?, text: r.get(7)?, size: r.get(8)? })
            })?;
            rows.collect()
        })
    }

    /// Inclusive id range (`from_id..=to_id`) for MCP `history_read`.
    pub fn t3_thread(
        &self,
        thread_id: &str,
        from_id: Option<&str>,
        to_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<T3Row>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, thread_id, subagent_id, session_id, kind, role, at, text, size FROM (
                    SELECT * FROM t3 WHERE thread_id = ?1 AND (?2 IS NULL OR id >= ?2) AND (?3 IS NULL OR id <= ?3) ORDER BY id DESC LIMIT ?4
                 ) ORDER BY id ASC",
            )?;
            let rows = st.query_map(params![thread_id, from_id, to_id, limit], |r| {
                Ok(T3Row { id: r.get(0)?, thread_id: r.get(1)?, subagent_id: r.get(2)?, session_id: r.get(3)?, kind: r.get(4)?, role: r.get(5)?, at: r.get(6)?, text: r.get(7)?, size: r.get(8)? })
            })?;
            rows.collect()
        })
    }

    /// Every row of one thread in insertion order (rowid), oldest first, with its source meta —
    /// the rollout view's feed (`jobs_view.rs`). Insertion order, not `at`, because a worker
    /// writes several rows within one millisecond and their ids are not ordered within it.
    pub fn t3_thread_in_order(
        &self,
        thread_id: &str,
        limit: i64,
    ) -> Result<Vec<(T3Row, Option<Value>)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, thread_id, subagent_id, session_id, kind, role, at, text, size, source FROM t3
                 WHERE thread_id = ?1 ORDER BY rowid ASC LIMIT ?2",
            )?;
            let rows = st.query_map(params![thread_id, limit], |r| {
                let source: Option<String> = r.get(9)?;
                Ok((
                    T3Row {
                        id: r.get(0)?,
                        thread_id: r.get(1)?,
                        subagent_id: r.get(2)?,
                        session_id: r.get(3)?,
                        kind: r.get(4)?,
                        role: r.get(5)?,
                        at: r.get(6)?,
                        text: r.get(7)?,
                        size: r.get(8)?,
                    },
                    source.and_then(|s| serde_json::from_str::<Value>(&s).ok()),
                ))
            })?;
            rows.collect()
        })
    }

    /// Rows written after `after_ms` (exclusive), every thread or one chat, oldest first, at
    /// most `limit` — the inspect window (`inspect.rs`). Both shapes hit an index
    /// (`t3_at`, `t3_thread_at`), so a call never scans the table.
    pub fn t3_after(
        &self,
        after_ms: i64,
        thread_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<(T3Row, Option<Value>)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, thread_id, subagent_id, session_id, kind, role, at, text, size, source FROM t3
                 WHERE at > ?1 AND (?2 IS NULL OR thread_id = ?2) ORDER BY at ASC, id ASC LIMIT ?3",
            )?;
            let rows = st.query_map(params![after_ms, thread_id, limit], |r| {
                let source: Option<String> = r.get(9)?;
                Ok((
                    T3Row {
                        id: r.get(0)?,
                        thread_id: r.get(1)?,
                        subagent_id: r.get(2)?,
                        session_id: r.get(3)?,
                        kind: r.get(4)?,
                        role: r.get(5)?,
                        at: r.get(6)?,
                        text: r.get(7)?,
                        size: r.get(8)?,
                    },
                    source.and_then(|s| serde_json::from_str::<Value>(&s).ok()),
                ))
            })?;
            rows.collect()
        })
    }

    /// Replace every occurrence of `values` in T3 text (and its FTS index). Returns how many
    /// rows changed. Used by Scrub from context.
    /// After a scrub: fold the WAL into the main file and truncate it, so the pages that held
    /// the old text are gone from disk too (with `secure_delete` on, the freed content inside
    /// the main file is already zeroed).
    pub fn flush_after_scrub(&self) -> Result<()> {
        self.with(|c| {
            c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            Ok(())
        })
    }

    pub fn t3_scrub(&self, values: &[String]) -> Result<usize> {
        let needles: Vec<&str> = values
            .iter()
            .map(String::as_str)
            .filter(|v| v.len() >= 6)
            .collect();
        if needles.is_empty() {
            return Ok(0);
        }
        self.with(|c| {
            let mut st = c.prepare("SELECT id, text FROM t3")?;
            let rows: Vec<(String, String)> = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            let mut n = 0;
            for (id, text) in rows {
                let mut next = text.clone();
                for v in &needles {
                    if next.contains(v) {
                        next = next.replace(v, "[REDACTED:vault]");
                    }
                }
                if next == text {
                    continue;
                }
                c.execute(
                    "UPDATE t3 SET text = ?2, size = ?3 WHERE id = ?1",
                    params![id, next, next.len() as i64],
                )?;
                c.execute("DELETE FROM t3_fts WHERE id = ?1", params![id])?;
                c.execute(
                    "INSERT INTO t3_fts(text, id, thread_id, kind) SELECT text, id, thread_id, kind FROM t3 WHERE id = ?1",
                    params![id],
                )?;
                n += 1;
            }
            Ok(n)
        })
    }

    /// Replace every occurrence of `values` in durable memory text (and its FTS index).
    pub fn memory_scrub(&self, values: &[String]) -> Result<usize> {
        let needles: Vec<&str> = values
            .iter()
            .map(String::as_str)
            .filter(|v| v.len() >= 6)
            .collect();
        if needles.is_empty() {
            return Ok(0);
        }
        self.with(|c| {
            let mut st = c.prepare("SELECT id, text FROM memory")?;
            let rows: Vec<(String, String)> = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            let mut n = 0;
            for (id, text) in rows {
                let mut next = text.clone();
                for v in &needles {
                    if next.contains(v) {
                        next = next.replace(v, "[REDACTED:vault]");
                    }
                }
                if next == text {
                    continue;
                }
                c.execute("UPDATE memory SET text = ?2 WHERE id = ?1", params![id, next])?;
                c.execute("DELETE FROM memory_fts WHERE id = ?1", params![id])?;
                c.execute(
                    "INSERT INTO memory_fts(text, id, tier) SELECT text, id, tier FROM memory WHERE id = ?1",
                    params![id],
                )?;
                n += 1;
            }
            Ok(n)
        })
    }

    // ---- memory -------------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn memory_add(
        &self,
        tier: &str,
        thread_id: Option<&str>,
        kind: &str,
        scope: &str,
        text: &str,
        source: Option<&Value>,
        valid_at: Option<i64>,
        confidence: f64,
        importance: i64,
        supersedes: &[String],
    ) -> Result<String> {
        let id = ulid();
        let text = redact(text);
        self.with(|c| {
            c.execute(
                "INSERT INTO memory(id, tier, thread_id, kind, scope, text, source, valid_at, created_at, supersedes, confidence, importance) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![id, tier, thread_id, kind, scope, text, source.map(|s| s.to_string()), valid_at, now_ms(), serde_json::to_string(supersedes).unwrap_or_default(), confidence, importance],
            )?;
            c.execute("INSERT INTO memory_fts(text, id, tier) VALUES (?1,?2,?3)", params![text, id, tier])?;
            Ok(())
        })?;
        Ok(id)
    }

    /// Rewrite a record's text in place (FTS kept in step); the id, tier and provenance stay.
    pub fn memory_set_text(&self, id: &str, text: &str) -> Result<()> {
        let text = redact(text);
        self.with(|c| {
            c.execute(
                "UPDATE memory SET text = ?2 WHERE id = ?1",
                params![id, text],
            )?;
            c.execute("DELETE FROM memory_fts WHERE id = ?1", params![id])?;
            c.execute(
                "INSERT INTO memory_fts(text, id, tier) SELECT text, id, tier FROM memory WHERE id = ?1",
                params![id],
            )?;
            Ok(())
        })
    }

    /// INVALIDATE: set `invalid_at` and `expired_at`; the row stays (D7).
    /// Soft-invalidate every live memory learned in one thread (an agent leaving a people chat,
    /// D7): rows keep their text and become `invalid_at = now`. Returns how many.
    pub fn memory_invalidate_thread(&self, thread_id: &str, now: i64) -> Result<usize> {
        self.with(|c| {
            c.execute(
                "UPDATE memory SET invalid_at = ?2, expired_at = ?2 WHERE thread_id = ?1 AND expired_at IS NULL",
                params![thread_id, now],
            )
        })
    }

    pub fn memory_invalidate(&self, id: &str, invalid_at: Option<i64>) -> Result<()> {
        let now = now_ms();
        self.with(|c| c.execute("UPDATE memory SET invalid_at = COALESCE(?2, ?3), expired_at = ?3 WHERE id = ?1 AND expired_at IS NULL", params![id, invalid_at, now]).map(|_| ()))
    }

    fn row_to_memory(r: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRow> {
        Ok(MemoryRow {
            id: r.get(0)?,
            tier: r.get(1)?,
            thread_id: r.get(2)?,
            kind: r.get(3)?,
            scope: r.get(4)?,
            text: r.get(5)?,
            valid_at: r.get(6)?,
            invalid_at: r.get(7)?,
            created_at: r.get(8)?,
            expired_at: r.get(9)?,
            supersedes: serde_json::from_str(&r.get::<_, String>(10)?).unwrap_or(json!([])),
            confidence: r.get(11)?,
            importance: r.get(12)?,
        })
    }

    const MEMORY_COLS: &'static str = "m.id, m.tier, m.thread_id, m.kind, m.scope, m.text, m.valid_at, m.invalid_at, m.created_at, m.expired_at, m.supersedes, m.confidence, m.importance";

    pub fn memory_search(
        &self,
        query: &str,
        tiers: &[String],
        include_expired: bool,
        limit: i64,
    ) -> Result<Vec<(MemoryRow, f64)>> {
        let q = fts_query(query);
        let tiers_csv = if tiers.is_empty() {
            None
        } else {
            Some(tiers.join(","))
        };
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {}, bm25(memory_fts) FROM memory_fts JOIN memory m ON m.id = memory_fts.id
                 WHERE memory_fts MATCH ?1 AND (?2 OR m.expired_at IS NULL) AND (?3 IS NULL OR instr(?3, m.tier) > 0)
                 ORDER BY bm25(memory_fts) LIMIT ?4",
                Self::MEMORY_COLS
            ))?;
            let rows = st.query_map(params![q, include_expired, tiers_csv, limit], |r| Ok((Self::row_to_memory(r)?, r.get::<_, f64>(13)?)))?;
            rows.collect()
        })
    }

    pub fn memory_live(
        &self,
        tier: &str,
        thread_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<MemoryRow>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM memory m WHERE m.tier = ?1 AND m.expired_at IS NULL AND (?2 IS NULL OR m.thread_id = ?2) ORDER BY m.importance DESC, m.created_at DESC LIMIT ?3",
                Self::MEMORY_COLS
            ))?;
            let rows = st.query_map(params![tier, thread_id, limit], Self::row_to_memory)?;
            rows.collect()
        })
    }

    pub fn memory_by_ids(&self, ids: &[String]) -> Result<Vec<MemoryRow>> {
        let mut out = Vec::new();
        for id in ids {
            if let Some(m) = self.with(|c| {
                c.query_row(
                    &format!("SELECT {} FROM memory m WHERE m.id = ?1", Self::MEMORY_COLS),
                    params![id],
                    Self::row_to_memory,
                )
                .optional()
            })? {
                out.push(m);
            }
        }
        Ok(out)
    }

    pub fn memory_touch(&self, ids: &[String]) -> Result<()> {
        self.with(|c| {
            for id in ids {
                c.execute("UPDATE memory SET last_accessed_at = ?2, access_count = access_count + 1 WHERE id = ?1", params![id, now_ms()])?;
            }
            Ok(())
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn log_decision(
        &self,
        side: &str,
        event_id: Option<&str>,
        gate: Option<&Value>,
        action: Option<&str>,
        target_ids: &[String],
        reason: Option<&str>,
        confidence: Option<f64>,
        model: Option<&str>,
        latency_ms: Option<i64>,
    ) -> Result<()> {
        self.log_decision_with_usage(
            side, event_id, gate, action, target_ids, reason, confidence, model, latency_ms, None,
        )
    }

    /// `log_decision` plus the call's usage (`{input_tokens, output_tokens, cost_usd, provider}`),
    /// which the responder decision records per call.
    #[allow(clippy::too_many_arguments)]
    pub fn log_decision_with_usage(
        &self,
        side: &str,
        event_id: Option<&str>,
        gate: Option<&Value>,
        action: Option<&str>,
        target_ids: &[String],
        reason: Option<&str>,
        confidence: Option<f64>,
        model: Option<&str>,
        latency_ms: Option<i64>,
        usage: Option<&Value>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO memory_decision_log(id, side, event_id, gate, action, target_ids, reason, confidence, model, latency_ms, usage, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![ulid(), side, event_id, gate.map(|g| g.to_string()), action, serde_json::to_string(target_ids).unwrap_or_default(), reason, confidence, model, latency_ms, usage.map(|u| u.to_string()), now_ms()],
            )
            .map(|_| ())
        })
    }

    // ---- handles and scratch ------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_handle(
        &self,
        id: &str,
        chat_id: Option<&str>,
        session_id: Option<&str>,
        kind: &str,
        chars: i64,
        lines: i64,
        path: &str,
        note: &str,
        blob_key: Option<&str>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO handles(id, chat_id, session_id, kind, chars, lines, path, note, blob_key, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                 ON CONFLICT(id) DO UPDATE SET note = excluded.note, blob_key = COALESCE(excluded.blob_key, handles.blob_key)",
                params![id, chat_id, session_id, kind, chars, lines, path, note, blob_key, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn handle(&self, id: &str) -> Result<Option<HandleRow>> {
        self.with(|c| {
            c.query_row("SELECT id, chat_id, kind, chars, lines, path, note, created_at FROM handles WHERE id = ?1", params![id], |r| {
                Ok(HandleRow { id: r.get(0)?, chat_id: r.get(1)?, kind: r.get(2)?, chars: r.get(3)?, lines: r.get(4)?, path: r.get(5)?, note: r.get(6)?, created_at: r.get(7)? })
            })
            .optional()
        })
    }

    pub fn handles_for_chat(&self, chat_id: &str, limit: i64) -> Result<Vec<HandleRow>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT id, chat_id, kind, chars, lines, path, note, created_at FROM handles WHERE chat_id = ?1 ORDER BY created_at DESC LIMIT ?2")?;
            let rows = st.query_map(params![chat_id, limit], |r| {
                Ok(HandleRow { id: r.get(0)?, chat_id: r.get(1)?, kind: r.get(2)?, chars: r.get(3)?, lines: r.get(4)?, path: r.get(5)?, note: r.get(6)?, created_at: r.get(7)? })
            })?;
            rows.collect()
        })
    }

    pub fn scratch_set(&self, chat_id: &str, name: &str, value: Option<&str>) -> Result<()> {
        self.with(|c| {
            match value {
                Some(v) => c.execute(
                    "INSERT INTO scratch(chat_id, name, value, updated_at) VALUES (?1,?2,?3,?4) ON CONFLICT(chat_id, name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                    params![chat_id, name, v, now_ms()],
                )?,
                None => c.execute("DELETE FROM scratch WHERE chat_id = ?1 AND name = ?2", params![chat_id, name])?,
            };
            Ok(())
        })
    }

    pub fn scratch_get(&self, chat_id: &str, name: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT value FROM scratch WHERE chat_id = ?1 AND name = ?2",
                params![chat_id, name],
                |r| r.get(0),
            )
            .optional()
        })
    }

    pub fn scratch_all(&self, chat_id: &str) -> Result<Vec<(String, String)>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT name, value FROM scratch WHERE chat_id = ?1 ORDER BY updated_at DESC LIMIT 40")?;
            let rows = st.query_map(params![chat_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
    }

    // ---- metrics and asks ---------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn record_turn(
        &self,
        chat_id: &str,
        message_id: &str,
        route: Option<&str>,
        ttft_ms: Option<i64>,
        route_ms: Option<i64>,
        total_ms: i64,
        tool_calls: i64,
        escalated: bool,
        usage: Option<&Value>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO turn_metrics(id, chat_id, message_id, route, ttft_ms, route_ms, total_ms, tool_calls, escalated, usage, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![ulid(), chat_id, message_id, route, ttft_ms, route_ms, total_ms, tool_calls, escalated as i64, usage.map(|u| u.to_string()), now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn turn_metrics(&self, limit: i64) -> Result<Vec<Value>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT chat_id, message_id, route, ttft_ms, route_ms, total_ms, tool_calls, escalated, usage, created_at FROM turn_metrics ORDER BY created_at DESC LIMIT ?1")?;
            let rows = st.query_map(params![limit], |r| {
                Ok(json!({
                    "chat_id": r.get::<_, String>(0)?, "message_id": r.get::<_, String>(1)?, "route": r.get::<_, Option<String>>(2)?,
                    "ttft_ms": r.get::<_, Option<i64>>(3)?, "route_ms": r.get::<_, Option<i64>>(4)?, "total_ms": r.get::<_, i64>(5)?,
                    "tool_calls": r.get::<_, i64>(6)?, "escalated": r.get::<_, i64>(7)? != 0,
                    "usage": r.get::<_, Option<String>>(8)?.and_then(|s| serde_json::from_str::<Value>(&s).ok()), "created_at": r.get::<_, i64>(9)?,
                }))
            })?;
            rows.collect()
        })
    }

    /// Turn metrics recorded after `after_ms`, oldest first (inspect window).
    pub fn turn_metrics_after(
        &self,
        after_ms: i64,
        chat_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Value>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, chat_id, message_id, route, ttft_ms, route_ms, total_ms, tool_calls, escalated, usage, created_at
                 FROM turn_metrics WHERE created_at > ?1 AND (?2 IS NULL OR chat_id = ?2) ORDER BY created_at ASC LIMIT ?3",
            )?;
            let rows = st.query_map(params![after_ms, chat_id, limit], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "chat_id": r.get::<_, String>(1)?, "message_id": r.get::<_, String>(2)?, "route": r.get::<_, Option<String>>(3)?,
                    "ttft_ms": r.get::<_, Option<i64>>(4)?, "route_ms": r.get::<_, Option<i64>>(5)?, "total_ms": r.get::<_, i64>(6)?,
                    "tool_calls": r.get::<_, i64>(7)?, "escalated": r.get::<_, i64>(8)? != 0,
                    "usage": r.get::<_, Option<String>>(9)?.and_then(|s| serde_json::from_str::<Value>(&s).ok()), "created_at": r.get::<_, i64>(10)?,
                }))
            })?;
            rows.collect()
        })
    }

    /// Decision-log rows of one `side` (`respond`, `route`, …) after `after_ms`, oldest first
    /// (inspect window): the responder gate's scores and reasons.
    pub fn decisions_after(&self, after_ms: i64, side: &str, limit: i64) -> Result<Vec<Value>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, side, event_id, gate, action, target_ids, reason, confidence, model, latency_ms, usage, created_at
                 FROM memory_decision_log WHERE created_at > ?1 AND side = ?2 ORDER BY created_at ASC LIMIT ?3",
            )?;
            let rows = st.query_map(params![after_ms, side, limit], |r| {
                let parse = |s: Option<String>| s.and_then(|s| serde_json::from_str::<Value>(&s).ok());
                Ok(json!({
                    "id": r.get::<_, String>(0)?, "side": r.get::<_, String>(1)?, "event_id": r.get::<_, Option<String>>(2)?,
                    "gate": parse(r.get::<_, Option<String>>(3)?), "action": r.get::<_, Option<String>>(4)?,
                    "target_ids": parse(r.get::<_, Option<String>>(5)?), "reason": r.get::<_, Option<String>>(6)?,
                    "confidence": r.get::<_, Option<f64>>(7)?, "model": r.get::<_, Option<String>>(8)?,
                    "latency_ms": r.get::<_, Option<i64>>(9)?, "usage": parse(r.get::<_, Option<String>>(10)?),
                    "created_at": r.get::<_, i64>(11)?,
                }))
            })?;
            rows.collect()
        })
    }

    pub fn insert_ask(
        &self,
        id: &str,
        chat_id: Option<&str>,
        session_id: Option<&str>,
        question: &str,
        widget: Option<&Value>,
    ) -> Result<()> {
        let widget = widget.map(Value::to_string);
        self.with(|c| c.execute("INSERT INTO asks(id, chat_id, session_id, question, widget, created_at) VALUES (?1,?2,?3,?4,?5,?6)", params![id, chat_id, session_id, question, widget, now_ms()]).map(|_| ()))
    }

    pub fn open_ask(&self, chat_id: &str) -> Result<Option<(String, Option<String>, String)>> {
        self.with(|c| {
            c.query_row("SELECT id, session_id, question FROM asks WHERE chat_id = ?1 AND answered = 0 ORDER BY created_at DESC LIMIT 1", params![chat_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .optional()
        })
    }

    /// One ask by id: `(question, widget, answered)`.
    pub fn ask(&self, id: &str) -> Result<Option<(String, Option<Value>, bool)>> {
        self.with(|c| {
            c.query_row(
                "SELECT question, widget, answered FROM asks WHERE id = ?1",
                params![id],
                |r| {
                    let widget: Option<String> = r.get(1)?;
                    Ok((
                        r.get::<_, String>(0)?,
                        widget.and_then(|w| serde_json::from_str(&w).ok()),
                        r.get::<_, i64>(2)? != 0,
                    ))
                },
            )
            .optional()
        })
    }

    pub fn answer_ask(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute("UPDATE asks SET answered = 1 WHERE id = ?1", params![id])
                .map(|_| ())
        })
    }

    // ---- approvals (feature approve) ------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn insert_approval(
        &self,
        id: &str,
        chat_id: &str,
        message_id: &str,
        title: &str,
        risk: &str,
        payload: &Value,
        expires_at: i64,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO approvals(id, chat_id, message_id, title, risk, payload, created_at, expires_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![id, chat_id, message_id, title, risk, payload.to_string(), now_ms(), expires_at],
            )
            .map(|_| ())
        })
    }

    pub fn approval(&self, id: &str) -> Result<Option<ApprovalRow>> {
        self.with(|c| {
            c.query_row(
                "SELECT id, chat_id, message_id, title, risk, payload, state, created_at, expires_at, decided_at, decided_by, reason FROM approvals WHERE id = ?1",
                params![id],
                approval_row,
            )
            .optional()
        })
    }

    pub fn pending_approvals(&self) -> Result<Vec<ApprovalRow>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT id, chat_id, message_id, title, risk, payload, state, created_at, expires_at, decided_at, decided_by, reason FROM approvals WHERE state = 'pending' ORDER BY created_at")?;
            let rows = st.query_map([], approval_row)?;
            rows.collect()
        })
    }

    /// Record the decision; returns false when the row was already decided (first decision wins).
    pub fn decide_approval(
        &self,
        id: &str,
        state: &str,
        decided_by: Option<&str>,
        reason: Option<&str>,
    ) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE approvals SET state = ?2, decided_at = ?3, decided_by = ?4, reason = ?5 WHERE id = ?1 AND state = 'pending'",
                params![id, state, now_ms(), decided_by, reason],
            )
            .map(|n| n > 0)
        })
    }

    pub fn outbox_insert(
        &self,
        approval_id: &str,
        channel: &str,
        recipient: &str,
        subject: Option<&str>,
        body: &str,
    ) -> Result<String> {
        let id = ulid();
        self.with(|c| {
            c.execute(
                "INSERT INTO outbox(id, approval_id, channel, recipient, subject, body, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![id, approval_id, channel, recipient, subject, body, now_ms()],
            )
            .map(|_| ())
        })?;
        Ok(id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRow {
    pub id: String,
    pub chat_id: String,
    pub message_id: String,
    pub title: String,
    pub risk: String,
    pub payload: Value,
    pub state: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub decided_at: Option<i64>,
    pub decided_by: Option<String>,
    pub reason: Option<String>,
}

fn approval_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ApprovalRow> {
    Ok(ApprovalRow {
        id: r.get(0)?,
        chat_id: r.get(1)?,
        message_id: r.get(2)?,
        title: r.get(3)?,
        risk: r.get(4)?,
        payload: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or(Value::Null),
        state: r.get(6)?,
        created_at: r.get(7)?,
        expires_at: r.get(8)?,
        decided_at: r.get(9)?,
        decided_by: r.get(10)?,
        reason: r.get(11)?,
    })
}

/// Turn free text into a safe FTS5 query: quoted terms joined with OR-less implicit AND, prefix match on the last.
fn fts_query(q: &str) -> String {
    let terms: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect();
    if terms.is_empty() {
        return "\"\"".to_owned();
    }
    terms.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D19 redaction is applied before anything lands in T3.
    #[test]
    fn redaction_removes_known_secret_shapes() {
        let s = redact("key sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789 and Authorization: Bearer abc.def-ghi and\nOPENAI_API_KEY=sk-live-whatever\n");
        assert!(!s.contains("sk-or-v1-abcdef"));
        assert!(s.contains("[REDACTED:openrouter-key]"));
        assert!(s.contains("Authorization: Bearer [REDACTED:bearer]"));
        assert!(s.contains("OPENAI_API_KEY=[REDACTED:env]"));
    }
}
