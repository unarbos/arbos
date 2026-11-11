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
  delivered_key TEXT, restarts INTEGER NOT NULL DEFAULT 0, steers TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX IF NOT EXISTS jobs_subagent_created ON jobs(subagent_id, created_at DESC);

-- Every worker a job has ever been given, not only the one it is on now (bug AO). `jobs.subagent_id`
-- is overwritten by each fresh worker, and the processes the replaced worker left behind are
-- reachable only under *its* id — so a cancel that reads the column alone asks the kernel about the
-- wrong session. This is the job's whole history of workers, which is what a cancel sweeps.
CREATE TABLE IF NOT EXISTS job_workers (
  job_id TEXT NOT NULL, subagent_id TEXT NOT NULL, at INTEGER NOT NULL,
  PRIMARY KEY (job_id, subagent_id)
);
CREATE INDEX IF NOT EXISTS job_workers_job ON job_workers(job_id, at);
"#;

/// The `t3` kind a user row takes once the provider declined the turn it opened.
///
/// A kind rather than a deletion: the words are the user's own, so the row keeps its place in the
/// thread for the transcript, for `t3_thread` and for a deliberate history read. Only the memory
/// walk treats the kind as "not mine to offer", which is what stops a declined turn coming back
/// as remembered context on a later turn (see `Store::t3_mark_refused`).
pub const T3_KIND_REFUSED: &str = "user_refused";

/// The step of a job the user stopped, in the same plain words as every other step the working
/// row and `list_jobs` show.
pub const CANCELLED_STEP: &str = "Cancelled";

/// The step of a job that ended without finishing.
///
/// A dead worker writes no more steps, so the line it was last on stands for ever as what the
/// job is doing. Test5 round 8: job `01M6N916PS4TR6EHDEM51NM6XG` failed at 01:17:28 with "pi
/// main agent not running" and still read "Searching the project" fourteen minutes later, which
/// reads as a job that is still working. `list_jobs` carries no error field, so the step is the
/// only line there that can say otherwise.
///
/// Generic on purpose: a caller that knows why writes its own step afterwards, as the expired
/// approval card does. Why it failed is in `error`, and how far it got is in its own thread.
pub const FAILED_STEP: &str = "Failed";

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
    /// Every `steer_job` correction this job has been given, oldest first.
    ///
    /// A steer is delivered into the worker's live pi session, which dies with that worker. The
    /// job outlives it — a fresh worker, or the inline fallback — and each of those used to run
    /// the brief the user had already corrected (bug V, Jacob's Test5 round 7). Kept on the job
    /// row because that is the thing with the longer life.
    pub steers: Vec<String>,
}

impl JobRow {
    pub fn is_open(&self) -> bool {
        matches!(self.state.as_str(), "queued" | "running")
    }

    /// How long the job ran: up to `ended_at` once it has one, else up to now.
    ///
    /// A job that has ended has a fixed figure. Test5 round 7, bug X: this was `now` in every
    /// state, so a cancelled job's time went on growing — 80 s in one reading and 194 s in the
    /// next, for job `01M6N1TRC0NWER66QV8XCHEX6T`, which was stopped 37 s in.
    pub fn elapsed_s(&self) -> i64 {
        let from = self.started_at.unwrap_or(self.created_at);
        let to = self.ended_at.unwrap_or_else(now_ms);
        (to - from).max(0) / 1000
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

/// One historical refused user turn the backfill would mark (`refused_backfill_candidates`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedBackfillCandidate {
    pub thread_id: String,
    pub message_id: String,
    pub row_id: String,
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
/// Shortest value [`Store::scrub_all_text`] will look for across the whole schema.
///
/// One above `interrupted`, the longest status literal any column of this schema is read back
/// by; `cancelled` and `delivered` are nine, `approved` eight, `running` and `pending` seven.
/// A shorter value still gets swept from the two text columns that carry prose, where the worst
/// a collision does is redact a word. It is not swept from the structural columns, where the
/// worst it does is change a row's meaning.
///
/// `no_status_literal_reaches_the_sweep_floor` reads those literals back off the source and fails
/// if a new one reaches this number, so the clearance is checked rather than remembered.
const GENERIC_SWEEP_MIN: usize = 12;

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
        // Additive migration: the corrections a job was steered with (bug V), which outlive
        // the worker session they were delivered to.
        let has_steers: bool = c
            .prepare("PRAGMA table_info(jobs)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|n| n == "steers");
        if !has_steers {
            c.execute_batch("ALTER TABLE jobs ADD COLUMN steers TEXT NOT NULL DEFAULT '[]'")?;
        }
        // Additive migration: when a late approval's re-run was let through without a second
        // card. This is the whole of the grant — it used to be a ten-minute entry in memory,
        // which a restart lost and a long wait for a worker slot outlived, both of which asked
        // the user to approve the same thing twice. Nullable, because "never used" is the state
        // most rows are in, and a row that predates this column has not been used either.
        let has_grant_consumed: bool = c
            .prepare("PRAGMA table_info(approvals)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|n| n == "grant_consumed_at");
        if !has_grant_consumed {
            c.execute_batch("ALTER TABLE approvals ADD COLUMN grant_consumed_at INTEGER")?;
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

    const JOB_COLS: &'static str = "id, chat_id, ask_message_id, subagent_id, title, kind, brief, touches, state, last_step, last_step_at, created_at, started_at, ended_at, error, delivered_key, restarts, steers";

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
            steers: serde_json::from_str(&r.get::<_, String>(17)?).unwrap_or_default(),
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

    /// Give a job its worker: the one it is on now, and one more entry in the history a cancel
    /// sweeps.
    ///
    /// Both, because they answer different questions. Every lane that asks "which session is this
    /// job talking to" wants the column, and it must keep being overwritten. A cancel asks
    /// "whose processes belong to this job", and a job that was given a fresh worker — after a
    /// crash, or after a box restart — has two answers to that (bug AO).
    pub fn set_job_subagent(&self, id: &str, subagent_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE jobs SET subagent_id = ?2 WHERE id = ?1",
                params![id, subagent_id],
            )?;
            c.execute(
                "INSERT OR IGNORE INTO job_workers(job_id, subagent_id, at) VALUES (?1,?2,?3)",
                params![id, subagent_id, now_ms()],
            )
            .map(|_| ())
        })
    }

    /// Every worker this job has been given, newest first.
    ///
    /// Newest first because the worker the job is on now is the one that must be closed before
    /// anything is swept: a worker that is still live can answer a kill by starting something
    /// else, and the replaced ones cannot.
    pub fn job_workers(&self, job_id: &str) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT subagent_id FROM job_workers WHERE job_id = ?1 ORDER BY at DESC, rowid DESC",
            )?;
            let rows = st.query_map(params![job_id], |r| r.get(0))?;
            rows.collect()
        })
    }

    /// Move a job to `state`. The ending — `ended_at`, and the step for the states that write
    /// one — is written once, by whichever terminal state ended the job: a later terminal state
    /// leaves all of it alone, and a job put back in the queue for a fresh worker clears the
    /// stamp, because it has not ended. (Every `CASE` here reads the row as it was before this
    /// statement, so `ended_at IS NULL` means "nothing has ended this job yet".)
    ///
    /// The `IS NULL` guard is the same one `started_at` has always had, and it matters for the
    /// same reason. A cancelled job's worker settles some time afterwards and writes its own
    /// terminal state; unguarded, that write moved the end of the job to the settle — 1 618 s
    /// past the start for bug S's job. `elapsed_s` reads this stamp, so a moved one is a wrong
    /// elapsed time.
    ///
    /// A cancel and a failure also write the step, because neither leaves a worker to write one
    /// and the line it was last on would otherwise stand for ever as what the job is doing — a
    /// cancelled job reading "Reading >" (bug X) or a failed one reading "Searching the project"
    /// fourteen minutes on (round 8). `done` keeps its worker's last line: it finished, its
    /// result arrives, and that line is the true account of what it finished with.
    /// Move a job to `state`.
    ///
    /// A cancelled job cannot be moved to `running`, and that clause is the backstop for bug AN
    /// rather than decoration. `run_subagent` waits for a subagent slot of unbounded length
    /// before it starts a job, a stop can land anywhere inside that wait, and the start then
    /// wrote `running` straight over `cancelled` — which this statement was happy to do, and
    /// which also cleared `ended_at`, leaving no trace in the row that the job had ever been
    /// cancelled. `run_subagent` re-checks after the wait so the work does not start; this makes
    /// the row unable to lie about it even if some future path forgets to ask.
    ///
    /// `cancelled` to `queued` is deliberately still allowed: that is the box-restart reclaim,
    /// which is a new decision about the job rather than a start racing a stop.
    pub fn set_job_state(&self, id: &str, state: &str, error: Option<&str>) -> Result<()> {
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "UPDATE jobs SET state = ?2, error = COALESCE(?3, error),
                    started_at = CASE WHEN ?2 = 'running' AND started_at IS NULL THEN ?4 ELSE started_at END,
                    ended_at = CASE WHEN ?2 IN ('done','failed','cancelled','interrupted') AND ended_at IS NULL THEN ?4
                                    WHEN ?2 IN ('queued','running') THEN NULL
                                    ELSE ended_at END,
                    last_step = CASE WHEN ?2 = 'cancelled' AND ended_at IS NULL THEN ?5
                                     WHEN ?2 = 'failed' AND ended_at IS NULL THEN ?6
                                     ELSE last_step END,
                    last_step_at = CASE WHEN ?2 IN ('cancelled','failed') AND ended_at IS NULL THEN ?4
                                        ELSE last_step_at END
                 WHERE id = ?1 AND NOT (?2 = 'running' AND state = 'cancelled')",
                params![id, state, error, now, CANCELLED_STEP, FAILED_STEP],
            )
            .map(|_| ())
        })
    }

    /// Test seam: move a finished job's `ended_at` back by `ms_ago`, so a test can place a job
    /// outside a window without sleeping through one.
    #[cfg(test)]
    pub fn test_backdate_job_end(&self, id: &str, ms_ago: i64) -> Result<()> {
        let at = now_ms().saturating_sub(ms_ago);
        self.with(|c| {
            c.execute(
                "UPDATE jobs SET ended_at = ?2 WHERE id = ?1",
                params![id, at],
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

    /// Record a `steer_job` correction on the job row, so a fresh worker or the inline fallback
    /// still has it after the session it was delivered to is gone (bug V).
    pub fn append_job_steer(&self, id: &str, text: &str) -> Result<()> {
        let text = redact(text);
        self.with(|c| {
            c.execute(
                "UPDATE jobs SET steers = json_insert(steers, '$[#]', ?2) WHERE id = ?1",
                params![id, text],
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

    /// The job a worker session runs, newest first: what a card raised by that session belongs
    /// to. A job keeps its row across a fresh worker, so this is the job, not the attempt.
    pub fn job_for_subagent(&self, subagent_id: &str) -> Result<Option<JobRow>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM jobs WHERE subagent_id = ?1 ORDER BY created_at DESC LIMIT 1",
                    Self::JOB_COLS
                ),
                params![subagent_id],
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

    /// A job already open for this ask under this title, if there is one.
    ///
    /// What makes an enqueue idempotent (bug AL): the same ask enqueued twice — a retried tool
    /// call, a duplicated one — is one job, and the second call is told about the first instead
    /// of taking a second place in the queue. Keyed on the ask as well as the title because one
    /// ask may legitimately open several jobs, and on the title as well as the ask because a
    /// retry repeats both.
    ///
    /// The oldest match, so a caller that races itself converges on one row rather than on
    /// whichever insert happened to land last.
    pub fn open_job_for_ask(
        &self,
        chat_id: &str,
        ask_message_id: &str,
        title: &str,
    ) -> Result<Option<JobRow>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM jobs WHERE chat_id = ?1 AND ask_message_id = ?2 AND title = ?3 \
                 AND state IN ('queued','running') ORDER BY created_at ASC, rowid ASC LIMIT 1",
                Self::JOB_COLS
            ))?;
            let mut rows =
                st.query_map(params![chat_id, ask_message_id, title], Self::row_to_job)?;
            rows.next().transpose()
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

    /// Mark the row of one user message as a turn the provider declined, by its `message_id`.
    ///
    /// The row stays where it is: the transcript, `t3_thread` and a deliberate history read all
    /// still have it. What changes is its kind, and the memory walk offers no row of this kind
    /// (`tree::leaves_by_ids`), so a declined turn is never handed back to the provider that
    /// declined it as remembered context.
    pub fn t3_mark_refused(&self, thread_id: &str, message_id: &str) -> Result<usize> {
        self.with(|c| {
            let n = c.execute(
                "UPDATE t3 SET kind = ?3 WHERE thread_id = ?1 AND kind = 'user' AND json_extract(source, '$.message_id') = ?2",
                params![thread_id, message_id, T3_KIND_REFUSED],
            )?;
            if n > 0 {
                c.execute(
                    "DELETE FROM t3_fts WHERE id IN (SELECT id FROM t3 WHERE thread_id = ?1 AND kind = ?3 AND json_extract(source, '$.message_id') = ?2)",
                    params![thread_id, message_id, T3_KIND_REFUSED],
                )?;
                c.execute(
                    "INSERT INTO t3_fts(text, id, thread_id, kind) SELECT text, id, thread_id, kind FROM t3 WHERE thread_id = ?1 AND kind = ?3 AND json_extract(source, '$.message_id') = ?2",
                    params![thread_id, message_id, T3_KIND_REFUSED],
                )?;
            }
            Ok(n)
        })
    }

    /// The t3 row id of a message now marked `T3_KIND_REFUSED`, so the memory tree can invalidate
    /// any card that summarised it (`Tree::invalidate_refused_leaf`). `None` when no such row.
    pub fn t3_refused_row_id(&self, thread_id: &str, message_id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT id FROM t3 WHERE thread_id = ?1 AND kind = ?3 AND json_extract(source, '$.message_id') = ?2",
                params![thread_id, message_id, T3_KIND_REFUSED],
                |r| r.get::<_, String>(0),
            )
            .optional()
        })
    }

    /// Historical user turns the provider declined before live marking existed (#2925), found by
    /// the one durable, non-heuristic signal a refused turn leaves in `t3`: the `compaction` row
    /// `rotate_chat_session` writes when `refused_turn_to_drop` fires, whose text names
    /// `refusal_dropped`. That row is written only for a real provider content-refusal (it rides
    /// `ModelError::refusal`, never a keyword guess), so matching it never hides benign memory.
    ///
    /// The governing user turn of a `refusal_dropped` row is the most recent `user` row before it
    /// in the same thread with no answer in between — an answer being an `assistant` row or a
    /// `route` row whose text begins `route reply`/`route act`. A turn whose retry answered gets
    /// no `refusal_dropped` row, and a refusal on a result/fallback turn has an answer between the
    /// opening ask and the drop, so both fall out here: this errs toward under-marking.
    ///
    /// Only rows still `kind = 'user'` are returned; a turn already marked `T3_KIND_REFUSED` is
    /// skipped, so a second run finds nothing new. Turns that predate the `refusal_dropped`
    /// machinery itself (they left only `turn without route` rows) carry no durable signature and
    /// are deliberately not matched.
    pub fn refused_backfill_candidates(&self) -> Result<Vec<RefusedBackfillCandidate>> {
        self.with(|c| {
            let mut threads: Vec<String> = Vec::new();
            {
                let mut st = c.prepare(
                    "SELECT DISTINCT thread_id FROM t3 \
                     WHERE kind = 'compaction' AND text LIKE '%refusal_dropped%' \
                     ORDER BY thread_id",
                )?;
                let rows = st.query_map([], |r| r.get::<_, String>(0))?;
                for t in rows {
                    threads.push(t?);
                }
            }
            let mut out: Vec<RefusedBackfillCandidate> = Vec::new();
            for tid in threads {
                let mut st = c.prepare(
                    "SELECT id, kind, text, json_extract(source, '$.message_id') \
                     FROM t3 WHERE thread_id = ?1 ORDER BY id",
                )?;
                let rows: Vec<(String, String, String, Option<String>)> = st
                    .query_map(params![tid], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                let mut open: Option<usize> = None;
                for (i, (_id, kind, text, _mid)) in rows.iter().enumerate() {
                    match kind.as_str() {
                        "user" | T3_KIND_REFUSED => open = Some(i),
                        "assistant" => open = None,
                        "route"
                            if text.starts_with("route reply") || text.starts_with("route act") =>
                        {
                            open = None
                        }
                        "compaction" if text.contains("refusal_dropped") => {
                            if let Some(ui) = open.take() {
                                let (uid, ukind, _, umid) = &rows[ui];
                                if ukind == "user" {
                                    if let Some(mid) = umid.clone() {
                                        out.push(RefusedBackfillCandidate {
                                            thread_id: tid.clone(),
                                            message_id: mid,
                                            row_id: uid.clone(),
                                        });
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(out)
        })
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

    /// `kind = "user"` also finds `T3_KIND_REFUSED`, because that marker is a user row with a
    /// flag on it and not a kind of its own. A search is a deliberate read — `history_search`,
    /// the secret sweep, the inspect endpoint — and none of those should lose a message because
    /// the provider declined the turn it opened. Only the memory walk passes over the marker, in
    /// `tree.rs`, where it is read off `T3Row::kind` rather than filtered for here.
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
                 WHERE t3_fts MATCH ?1 AND (?2 IS NULL OR t.thread_id = ?2)
                   AND (?3 IS NULL OR t.kind = ?3 OR (?3 = 'user' AND t.kind = ?6))
                 ORDER BY score LIMIT ?4 OFFSET ?5",
            )?;
            let rows = st.query_map(
                params![q, thread_id, kind, limit, offset, T3_KIND_REFUSED],
                |r| {
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
                },
            )?;
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

    /// Every other TEXT column the schema has, found by walking the schema rather than by naming
    /// the stores.
    ///
    /// A job's brief, a drafted message body, a scratch value, an ask, an approval payload and a
    /// handle's note all carried pasted text that the two passes above never reached, because
    /// those passes name their tables and nothing obliged the names to keep up with the schema.
    /// Walking `sqlite_master` means a table added later is swept the day it exists.
    ///
    /// Run it after [`Store::t3_scrub`] and [`Store::memory_scrub`], which own the `text` column
    /// of the two indexed tables and the size that describes it. This pass reaches everything
    /// those two leave behind, including their own `source` columns, and rebuilds a table's
    /// full-text index when it rewrote any of its rows.
    ///
    /// Primary keys are skipped: an id is never a pasted secret, and rewriting one would break
    /// every row that refers to it. Only columns the schema declares `TEXT` are touched, so a
    /// Loro snapshot stored as a BLOB is left alone.
    ///
    /// The length floor is higher here than the six the two passes above use, because these
    /// columns differ in kind. A scrub value is not always a long random token — a `login`
    /// secret's password is one too, and a person chose it — and matching is a literal substring
    /// replacement. Over most of the schema a collision costs one word of a user's history, but
    /// `jobs.state`, `approvals.state` and their neighbours are read back by equality, so a
    /// vaulted password of `running` or `pending` would rewrite a state machine's value instead
    /// of redacting prose. [`GENERIC_SWEEP_MIN`] clears the longest of those literals.
    pub fn scrub_all_text(&self, values: &[String]) -> Result<usize> {
        let needles: Vec<&str> = values
            .iter()
            .map(String::as_str)
            .filter(|v| v.len() >= GENERIC_SWEEP_MIN)
            .collect();
        if needles.is_empty() {
            return Ok(0);
        }
        self.with(|c| {
            let tables: Vec<String> = c
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")?
                .query_map([], |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let mut n = 0;
            for t in &tables {
                if t.contains("_fts") {
                    continue;
                }
                let cols: Vec<String> = c
                    .prepare(&format!("PRAGMA table_info({t})"))?
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, i64>(5)?,
                        ))
                    })?
                    .filter_map(Result::ok)
                    .filter(|(_, ty, pk)| ty.eq_ignore_ascii_case("TEXT") && *pk == 0)
                    .map(|(name, _, _)| name)
                    .collect();
                let mut hit = 0;
                for col in cols {
                    for v in &needles {
                        hit += c.execute(
                            &format!(
                                "UPDATE {t} SET {col} = replace({col}, ?1, ?2) WHERE {col} LIKE '%' || ?1 || '%'"
                            ),
                            params![v, "[REDACTED:vault]"],
                        )?;
                    }
                }
                n += hit;
                let fts = format!("{t}_fts");
                if hit > 0 && tables.contains(&fts) {
                    let mirrored: Vec<String> = c
                        .prepare(&format!("PRAGMA table_info({fts})"))?
                        .query_map([], |r| r.get::<_, String>(1))?
                        .collect::<Result<Vec<_>, _>>()?;
                    let list = mirrored.join(",");
                    c.execute(&format!("DELETE FROM {fts}"), [])?;
                    c.execute(
                        &format!("INSERT INTO {fts}({list}) SELECT {list} FROM {t}"),
                        [],
                    )?;
                }
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

    /// `expires_at` is written `0`: this card has no deadline and never gets one.
    ///
    /// The column stays because rows written before this change hold a real time in it and the
    /// relay's outside-agent window still has deadlines of its own. `0` is the honest value for
    /// "no deadline" and is in the past, so a legacy reader that compares it against the clock
    /// draws a closed card rather than inventing a future one.
    pub fn insert_approval(
        &self,
        id: &str,
        chat_id: &str,
        message_id: &str,
        title: &str,
        risk: &str,
        payload: &Value,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO approvals(id, chat_id, message_id, title, risk, payload, created_at, expires_at) VALUES (?1,?2,?3,?4,?5,?6,?7,0)",
                params![id, chat_id, message_id, title, risk, payload.to_string(), now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn approval(&self, id: &str) -> Result<Option<ApprovalRow>> {
        self.with(|c| {
            c.query_row(
                "SELECT id, chat_id, message_id, title, risk, payload, state, created_at, expires_at, decided_at, decided_by, reason, grant_consumed_at FROM approvals WHERE id = ?1",
                params![id],
                approval_row,
            )
            .optional()
        })
    }

    pub fn pending_approvals(&self) -> Result<Vec<ApprovalRow>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT id, chat_id, message_id, title, risk, payload, state, created_at, expires_at, decided_at, decided_by, reason, grant_consumed_at FROM approvals WHERE state = 'pending' ORDER BY created_at")?;
            let rows = st.query_map([], approval_row)?;
            rows.collect()
        })
    }

    /// The open card one **worker session** is waiting on, if any.
    ///
    /// Worker grain, deliberately. The retained-worker bound asks this about one session id, and
    /// a job-shaped answer would be wrong there: `job_worker_ids` is every worker a job has ever
    /// been given, so "this job has a card" names a lineage rather than the worker that is
    /// actually parked on it.
    pub fn pending_approval_for_session(&self, session_id: &str) -> Result<Option<ApprovalRow>> {
        Ok(self
            .pending_approvals()?
            .into_iter()
            .find(|r| r.payload.get("session_id").and_then(Value::as_str) == Some(session_id)))
    }

    /// The open card a **job** is parked on, if any: it has no worker running and will not get
    /// one until the user answers.
    ///
    /// Job grain, for the two questions that really are about the job rather than a worker — the
    /// per-agent capacity cap, and the boot reclaim that must leave a parked job alone instead
    /// of restarting its work from the brief.
    pub fn pending_approval_for_job(&self, job_id: &str) -> Result<Option<ApprovalRow>> {
        Ok(self
            .pending_approvals()?
            .into_iter()
            .find(|r| r.payload.get("job_id").and_then(Value::as_str) == Some(job_id)))
    }

    /// Every approved card whose single-use grant is still unclaimed, newest first.
    ///
    /// This is the grant: there is no second copy of it in memory to fall out of step, and it is
    /// as durable as the decision itself, so a restart or a long wait for a worker slot between
    /// the answer and the re-run cannot ask the user to approve the same call twice.
    pub fn unconsumed_grants(&self) -> Result<Vec<ApprovalRow>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT id, chat_id, message_id, title, risk, payload, state, created_at, expires_at, decided_at, decided_by, reason, grant_consumed_at FROM approvals WHERE state = 'approved' AND grant_consumed_at IS NULL ORDER BY decided_at DESC")?;
            let rows = st.query_map([], approval_row)?;
            rows.collect()
        })
    }

    /// Every approved card whose grant has already been claimed, newest use first. Read only to
    /// explain a second card for the same call: that first attempt was approved and did not
    /// finish.
    pub fn consumed_grants(&self) -> Result<Vec<ApprovalRow>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT id, chat_id, message_id, title, risk, payload, state, created_at, expires_at, decided_at, decided_by, reason, grant_consumed_at FROM approvals WHERE state = 'approved' AND grant_consumed_at IS NOT NULL ORDER BY grant_consumed_at DESC")?;
            let rows = st.query_map([], approval_row)?;
            rows.collect()
        })
    }

    /// Claim an approved card's grant. `true` means this caller got it; `false` means the row was
    /// not approved, or somebody else already used it.
    ///
    /// Single use is enforced by the database rather than by holding a lock across the re-run:
    /// the same conditional-update idiom as [`Store::decide_approval`], where the first writer
    /// wins and every later one sees zero rows affected.
    pub fn consume_grant(&self, id: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE approvals SET grant_consumed_at = ?2 WHERE id = ?1 AND state = 'approved' AND grant_consumed_at IS NULL",
                params![id, now_ms()],
            )
            .map(|n| n > 0)
        })
    }

    /// Record the decision; returns false when already decided (first decision wins).
    /// Active inline/polled callers must own the one-use grant before Approved is visible.
    /// Deferred decisions retain it for the existing canonical re-run claim path.
    pub fn decide_approval(
        &self,
        id: &str,
        state: &str,
        decided_by: Option<&str>,
        reason: Option<&str>,
        claim_grant: bool,
    ) -> Result<bool> {
        if claim_grant && state != "approved" {
            anyhow::bail!("only an approved decision can claim a grant");
        }
        self.with(|c| {
            let transaction = rusqlite::Transaction::new_unchecked(
                c,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let now = now_ms();
            let decided = transaction.execute(
                "UPDATE approvals SET state = ?2, decided_at = ?3, decided_by = ?4, reason = ?5 WHERE id = ?1 AND state = 'pending'",
                params![id, state, now, decided_by, reason],
            )?;
            if decided == 0 {
                return Ok(false);
            }
            if claim_grant {
                let claimed = transaction.execute(
                    "UPDATE approvals SET grant_consumed_at = ?2 WHERE id = ?1 AND state = 'approved' AND grant_consumed_at IS NULL",
                    params![id, now],
                )?;
                if claimed != 1 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
            }
            transaction.commit()?;
            Ok(true)
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
    /// `0` for every card this version writes: a card has no deadline. Legacy rows carry the
    /// time they were going to be closed at.
    pub expires_at: i64,
    pub decided_at: Option<i64>,
    pub decided_by: Option<String>,
    pub reason: Option<String>,
    /// When this card's single-use grant was claimed by a re-run, if it has been.
    pub grant_consumed_at: Option<i64>,
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
        grant_consumed_at: r.get(12)?,
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

    /// The floor only guards while it clears every literal these columns are read back against.
    ///
    /// [`GENERIC_SWEEP_MIN`] is a number tuned to the literals that exist today. A
    /// twelve-character state added next month would make it stop guarding with nothing failing:
    /// the two-sided test below pins the behaviour at today's literals and would pass unchanged.
    /// So the link is made here rather than remembered — walk the crate for equality comparisons
    /// on a `state` or `status` column, and for the schema's own defaults, and fail if any
    /// literal reaches the floor.
    ///
    /// Scoped to comparisons on purpose: widening it to every line mentioning a state also
    /// collects JSON field names like `recent_messages`, which no column is compared to. A state
    /// only ever written and never compared would slip past, which is the known limit of reading
    /// this off the source instead of off a type the compiler owns.
    #[test]
    fn no_status_literal_reaches_the_sweep_floor() {
        fn has_word(line: &str, w: &str) -> bool {
            let edge = |c: Option<char>| !c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            line.match_indices(w).any(|(i, _)| {
                edge(line[..i].chars().next_back()) && edge(line[i + w.len()..].chars().next())
            })
        }
        fn plain(tok: &str) -> bool {
            tok.starts_with(|c: char| c.is_ascii_lowercase())
                && tok
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        }

        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&src).expect("src dir").flatten() {
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) != Some("rs") {
                continue;
            }
            for line in std::fs::read_to_string(&path).unwrap_or_default().lines() {
                if (has_word(line, "state") || has_word(line, "status"))
                    && (line.contains("==") || line.contains("matches!"))
                {
                    found.extend(
                        line.split('"')
                            .skip(1)
                            .step_by(2)
                            .filter(|t| plain(t))
                            .map(str::to_owned),
                    );
                }
                if let Some(lit) = line
                    .split("DEFAULT '")
                    .nth(1)
                    .and_then(|r| r.split('\'').next())
                    .filter(|t| plain(t))
                {
                    found.push(lit.to_owned());
                }
            }
        }

        // A walk that finds nothing would pass this test without checking anything.
        assert!(found.len() >= 20, "source walk found too little: {found:?}");
        let over: Vec<&String> = found
            .iter()
            .filter(|l| l.len() >= GENERIC_SWEEP_MIN)
            .collect();
        assert!(
            over.is_empty(),
            "these are compared by equality and are at or above GENERIC_SWEEP_MIN ({GENERIC_SWEEP_MIN}), \
             so a vaulted value matching one would rewrite it: {over:?}. Raise the floor or shorten the state."
        );
    }

    /// A value short enough to be a status literal is left to the two text passes.
    ///
    /// `running` is a plausible password, a password is a scrub value, and `jobs.state` is read
    /// back by equality — so sweeping it across the whole schema would change what a row means
    /// rather than redact what it says.
    #[test]
    fn a_value_short_enough_to_be_a_status_is_left_to_the_text_passes() {
        let store = job_store();
        store
            .insert_job(
                "j1",
                "c1",
                "a1",
                "T",
                "ops",
                "the worker is running now",
                &[],
            )
            .unwrap();
        assert_eq!(store.scrub_all_text(&["running".to_owned()]).unwrap(), 0);
        assert!(store.job("j1").unwrap().unwrap().brief.contains("running"));

        // A value long enough to be a key still sweeps the same columns.
        const V: &str = "Zq7Z4x9W2m5K8n1P6r3T0v";
        store.scratch_set("c1", "n", Some(V)).unwrap();
        assert_eq!(store.scrub_all_text(&[V.to_owned()]).unwrap(), 1);
    }

    /// A vaulted value must not survive in any TEXT column the schema has.
    ///
    /// The cases are read off the live schema rather than listed, so a table or column added
    /// later is covered the day it exists or this fails. Before `scrub_all_text`, six columns
    /// kept the value: `jobs.brief`, `outbox.body`, `scratch.value`, `asks.question`,
    /// `approvals.payload` and `handles.note`.
    ///
    /// The value has no recognisable key shape on purpose: no pattern can match it, so this
    /// measures the sweep and nothing else.
    #[test]
    fn no_text_column_keeps_a_vaulted_value() {
        const V: &str = "Zq7Z4x9W2m5K8n1P6r3T0v";
        let store = job_store();
        let mut planted: Vec<(String, String)> = Vec::new();

        store
            .with(|c| {
                let tables: Vec<String> = c
                    .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")?
                    .query_map([], |r| r.get(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                for (i, t) in tables.iter().filter(|t| !t.contains("_fts")).enumerate() {
                    let cols: Vec<(String, String, i64)> = c
                        .prepare(&format!("PRAGMA table_info({t})"))?
                        .query_map([], |r| Ok((r.get(1)?, r.get(2)?, r.get(5)?)))?
                        .collect::<Result<Vec<_>, _>>()?;
                    let names: Vec<String> = cols.iter().map(|(n, _, _)| n.clone()).collect();
                    let vals: Vec<String> = cols
                        .iter()
                        .enumerate()
                        .map(|(j, (n, ty, pk))| {
                            if !ty.eq_ignore_ascii_case("TEXT") {
                                "0".to_owned()
                            } else if *pk != 0 {
                                format!("'id-{i}-{j}'")
                            } else {
                                planted.push((t.clone(), n.clone()));
                                format!("'{n} holds {V} here'")
                            }
                        })
                        .collect();
                    c.execute(
                        &format!(
                            "INSERT INTO {t}({}) VALUES ({})",
                            names.join(","),
                            vals.join(",")
                        ),
                        [],
                    )?;
                }
                Ok(())
            })
            .unwrap();

        assert!(
            planted.len() >= 20,
            "schema walk found too little: {planted:?}"
        );

        let vals = [V.to_owned()];
        store.t3_scrub(&vals).unwrap();
        store.memory_scrub(&vals).unwrap();
        store.scrub_all_text(&vals).unwrap();

        let survivors: Vec<String> = planted
            .iter()
            .filter(|(t, col)| {
                store
                    .with(|c| {
                        c.query_row(
                            &format!("SELECT count(*) FROM {t} WHERE {col} LIKE '%' || ?1 || '%'"),
                            params![V],
                            |r| r.get::<_, i64>(0),
                        )
                    })
                    .unwrap_or(0)
                    > 0
            })
            .map(|(t, col)| format!("{t}.{col}"))
            .collect();
        assert!(
            survivors.is_empty(),
            "kept the vaulted value: {survivors:?}"
        );
    }

    /// D19 redaction is applied before anything lands in T3.
    #[test]
    fn redaction_removes_known_secret_shapes() {
        let s = redact("key sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789 and Authorization: Bearer abc.def-ghi and\nOPENAI_API_KEY=sk-live-whatever\n");
        assert!(!s.contains("sk-or-v1-abcdef"));
        assert!(s.contains("[REDACTED:openrouter-key]"));
        assert!(s.contains("Authorization: Bearer [REDACTED:bearer]"));
        assert!(s.contains("OPENAI_API_KEY=[REDACTED:env]"));
    }

    fn job_store() -> Store {
        let dir = std::env::temp_dir().join(format!("arbos-store-{}", ulid()));
        Store::open(&dir.join("state.sqlite")).unwrap()
    }

    /// A job running since a moment ago, on the step its worker last reported.
    fn running_job(store: &Store, id: &str) {
        store
            .insert_job(
                id,
                "chat-1",
                "ask-1",
                "Sieving to 3e9",
                "ops",
                "Run the segmented sieve to 3e9 and report the count.",
                &[],
            )
            .unwrap();
        store.set_job_state(id, "running", None).unwrap();
        store.set_job_step(id, "Reading >").unwrap();
    }

    /// Move every stamp of a job row `by_ms` into the past, which is what a reading of the same
    /// row that much later sees.
    fn age_job(store: &Store, id: &str, by_ms: i64) {
        store
            .with(|c| {
                c.execute(
                    "UPDATE jobs SET created_at = created_at - ?2, started_at = started_at - ?2,
                        ended_at = ended_at - ?2, last_step_at = last_step_at - ?2
                     WHERE id = ?1",
                    params![id, by_ms],
                )
                .map(|_| ())
            })
            .unwrap();
    }

    /// Test5 round 7, bug X: a job cancelled 37 s in reported 80 s of elapsed time in one
    /// reading and 194 s in the next, and its step still read the line its worker died on.
    ///
    /// The figure has to be *stable*, not merely right once. The reading taken straight after
    /// the cancel was already correct with the bug in place, so the row is read again as if
    /// 55 s and then ten more minutes had passed.
    #[test]
    fn a_cancelled_job_holds_its_elapsed_time_and_reads_cancelled() {
        let store = job_store();
        running_job(&store, "job-x");
        age_job(&store, "job-x", 5_000);
        store
            .set_job_state("job-x", "cancelled", Some("stopped by the user"))
            .unwrap();

        let at_cancel = store.job("job-x").unwrap().unwrap();
        assert_eq!(at_cancel.state, "cancelled");
        assert_eq!(at_cancel.last_step, "Cancelled");
        assert!(at_cancel.ended_at.is_some(), "no ended_at: {at_cancel:?}");
        assert!(
            (3..=7).contains(&at_cancel.elapsed_s()),
            "5 s ± 2 at the cancel, got {} s",
            at_cancel.elapsed_s()
        );

        for later_ms in [55_000, 600_000] {
            age_job(&store, "job-x", later_ms);
            let row = store.job("job-x").unwrap().unwrap();
            assert!(
                (3..=7).contains(&row.elapsed_s()),
                "elapsed grew to {} s when the row was read {later_ms} ms later",
                row.elapsed_s()
            );
            assert_eq!(row.last_step, "Cancelled");
        }
    }

    /// `done` is reached from the worker's own completion path and never went through a cancel,
    /// so it is its own case. It keeps the worker's last real line, because it finished.
    #[test]
    fn a_done_job_holds_its_elapsed_time_and_its_own_last_step() {
        let store = job_store();
        running_job(&store, "job-x");
        age_job(&store, "job-x", 12_000);
        store.set_job_state("job-x", "done", None).unwrap();
        age_job(&store, "job-x", 600_000);
        let row = store.job("job-x").unwrap().unwrap();
        assert!(
            (10..=14).contains(&row.elapsed_s()),
            "a done job read ten minutes on: {} s",
            row.elapsed_s()
        );
        assert_eq!(row.last_step, "Reading >");
    }

    /// `failed` is covered on purpose, and separately: the defect was always in `elapsed_s`
    /// rather than in the cancel path, so a failure counts on exactly as a cancel did. Test5
    /// round 8: job `01M6N916PS4TR6EHDEM51NM6XG` failed at 01:17:28 with "pi main agent not
    /// running" and at about 01:31 still read `elapsed_s` 912 and the step "Searching the
    /// project" — fourteen minutes of growth on work that had already stopped.
    ///
    /// Read twice over that same fourteen minutes, because the reading taken shortly after the
    /// failure was correct with the bug in place.
    #[test]
    fn a_failed_job_holds_its_elapsed_time_and_reads_failed() {
        let store = job_store();
        running_job(&store, "job-x");
        age_job(&store, "job-x", 12_000);
        store
            .set_job_state("job-x", "failed", Some("pi main agent not running"))
            .unwrap();

        let at_failure = store.job("job-x").unwrap().unwrap();
        assert!(at_failure.ended_at.is_some(), "no ended_at: {at_failure:?}");
        assert!(
            (10..=14).contains(&at_failure.elapsed_s()),
            "12 s ± 2 at the failure, got {} s",
            at_failure.elapsed_s()
        );

        for later_ms in [60_000, 840_000] {
            age_job(&store, "job-x", later_ms);
            let row = store.job("job-x").unwrap().unwrap();
            assert!(
                (10..=14).contains(&row.elapsed_s()),
                "elapsed grew to {} s when the row was read {later_ms} ms later",
                row.elapsed_s()
            );
            assert_eq!(
                row.last_step, "Failed",
                "a dead worker's last line still reads as work in progress"
            );
        }
    }

    /// A cancelled job's worker settles some time afterwards and writes its own terminal state.
    /// The end of the job is the cancel, not that settle: bug S's job settled 1 618 s past its
    /// start, which is the elapsed time an unguarded `ended_at` would report for it.
    #[test]
    fn a_second_terminal_write_leaves_the_end_of_the_job_alone() {
        let store = job_store();
        running_job(&store, "job-x");
        age_job(&store, "job-x", 5_000);
        store
            .set_job_state("job-x", "cancelled", Some("stopped by the user"))
            .unwrap();
        let at_cancel = store.job("job-x").unwrap().unwrap().ended_at.unwrap();

        age_job(&store, "job-x", 1_613_000);
        store.set_job_state("job-x", "done", None).unwrap();
        let row = store.job("job-x").unwrap().unwrap();
        assert_eq!(
            row.ended_at,
            Some(at_cancel - 1_613_000),
            "the settle moved the end of the job: {row:?}"
        );
        assert!(
            (3..=7).contains(&row.elapsed_s()),
            "the job ran 5 s before the cancel, got {} s",
            row.elapsed_s()
        );
        assert_eq!(row.last_step, "Cancelled");
    }

    /// The retry branch requeues a job whose worker died, waits up to `PI_RESPAWN_WAIT` for a
    /// live pi child, then reads the state again and gives up if the user cancelled inside that
    /// window (bug V). The cancel is the first terminal write of the job's life, because the
    /// requeue cleared the stamp, and it is the one the job keeps: the write it races is the one
    /// that guard prevents, and a cancelled job is not open, so no path requeues it back.
    #[test]
    fn a_cancel_inside_the_respawn_wait_is_the_end_the_job_keeps() {
        let store = job_store();
        running_job(&store, "job-x");
        age_job(&store, "job-x", 5_000);

        store
            .set_job_state("job-x", "queued", Some("child exited"))
            .unwrap();
        let requeued = store.job("job-x").unwrap().unwrap();
        assert_eq!(
            requeued.ended_at, None,
            "a requeued job has not ended: {requeued:?}"
        );

        age_job(&store, "job-x", 40_000);
        store
            .set_job_state("job-x", "cancelled", Some("stopped by the user"))
            .unwrap();
        let at_cancel = store.job("job-x").unwrap().unwrap();
        let end = at_cancel.ended_at.expect("the cancel stamps the end");
        assert_eq!(at_cancel.last_step, "Cancelled");
        assert!(
            (43..=47).contains(&at_cancel.elapsed_s()),
            "45 s of work before the cancel, got {} s",
            at_cancel.elapsed_s()
        );
        assert!(
            !at_cancel.is_open(),
            "a cancelled job is not open, so nothing requeues it and clears the stamp: {at_cancel:?}"
        );

        age_job(&store, "job-x", 300_000);
        store
            .set_job_state("job-x", "failed", Some("the worker settled late"))
            .unwrap();
        let after = store.job("job-x").unwrap().unwrap();
        assert_eq!(
            after.ended_at,
            Some(end - 300_000),
            "a late settle moved the end of the job: {after:?}"
        );
        assert_eq!(
            after.last_step, "Cancelled",
            "the cancel ended this job, so the ending is its: {after:?}"
        );
    }

    /// A failed attempt stamps `ended_at`, and a job put back in the queue for a fresh worker
    /// has not ended: its time counts on instead of freezing at the attempt that died.
    #[test]
    fn a_requeued_job_counts_on() {
        let store = job_store();
        running_job(&store, "job-x");
        age_job(&store, "job-x", 8_000);
        store
            .set_job_state("job-x", "failed", Some("child exited"))
            .unwrap();
        store.set_job_state("job-x", "queued", None).unwrap();
        let row = store.job("job-x").unwrap().unwrap();
        assert_eq!(row.ended_at, None, "a requeued job has not ended: {row:?}");

        age_job(&store, "job-x", 30_000);
        let row = store.job("job-x").unwrap().unwrap();
        assert!(
            row.elapsed_s() >= 38,
            "an open job counts on: {} s",
            row.elapsed_s()
        );
    }
}
