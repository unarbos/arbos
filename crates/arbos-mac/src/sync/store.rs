//! On-device store (#929): one SQLite file per (relay, device) under the config
//! dir. It keeps the last registry / people / account JSON and, per room, a
//! Loro snapshot together with the relay cursor it was saved at, so a cold
//! launch paints the rail, tabs and transcripts from disk and every room only
//! asks the relay for rows after its cursor.
//!
//! Rules: cursor and snapshot are written in one row (never out of step); a
//! schema bump drops everything and refetches; sign-out deletes the file;
//! per-doc and total caps with least-recently-opened eviction.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::model::{now_ms, Id};

pub const SCHEMA_VERSION: i64 = 1;
/// A single chat snapshot above this is not kept (it streams from the relay instead).
pub const PER_DOC_CAP_BYTES: usize = 8_000_000;
/// Everything on disk for one account.
pub const TOTAL_CAP_BYTES: i64 = 250_000_000;

pub const KEY_PROJECTS: &str = "projects";
pub const KEY_CHATS: &str = "chats";
pub const KEY_PEOPLE: &str = "people";
pub const KEY_ACCOUNT: &str = "account";
pub const KEY_SELECTION: &str = "selection";
/// `{project_id: chat_id}`: the chat last opened per agent on this Mac.
pub const KEY_LAST_CHATS: &str = "last_chats";
/// Chat ids the user archived on this Mac (hidden from the tab strip, kept everywhere else).
pub const KEY_ARCHIVED_CHATS: &str = "archived_chats";
/// Set once the local archived ids were pushed up to the relay (§9c union migration).
pub const KEY_ARCHIVE_MIGRATED: &str = "archive_migrated";
/// `avatar:<url>` → the picture's bytes. The URL carries the picture's version
/// (`…/avatar.jpg?v=<stamp>`, `/avatars/<user>.jpg?v=…`), so a changed picture is a new key
/// and a cached one can never be stale; keys no current URL names are pruned with the registry.
pub const KEY_AVATAR_PREFIX: &str = "avatar:";

#[derive(Clone, Debug)]
pub struct SavedDoc {
    pub key: Id,
    pub project_id: Id,
    pub room_chat: String,
    pub cursor: i64,
    pub snapshot: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub bytes: i64,
    pub docs: i64,
}

pub struct Store {
    conn: Connection,
    path: PathBuf,
}

fn slug(relay_url: &str, device_id: &str) -> String {
    let raw: String = format!("{relay_url}|{device_id}")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if raw.len() > 80 {
        raw[raw.len() - 80..].to_string()
    } else {
        raw
    }
}

pub fn file_path(relay_url: &str, device_id: &str) -> PathBuf {
    super::credentials::config_dir()
        .join("store")
        .join(format!("{}.sqlite", slug(relay_url, device_id)))
}

impl Store {
    /// Open (creating or migrating); a file that cannot be read is replaced.
    pub fn open(relay_url: &str, device_id: &str) -> Option<Store> {
        if std::env::var("ARBOS_NO_STORE").is_ok_and(|v| v == "1") {
            return None;
        }
        let path = file_path(relay_url, device_id);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match Store::open_at(&path) {
            Ok(s) => Some(s),
            Err(err) => {
                eprintln!("store: {}: {err:#}; recreating", path.display());
                remove_files(&path);
                Store::open_at(&path).ok()
            }
        }
    }

    pub(crate) fn open_at(path: &Path) -> Result<Store> {
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != 0 && version != SCHEMA_VERSION {
            eprintln!("store: schema {version} → {SCHEMA_VERSION}; dropping");
            conn.execute_batch(
                "DROP TABLE IF EXISTS kv; DROP TABLE IF EXISTS docs; DROP TABLE IF EXISTS pending;",
            )?;
        }
        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS kv (
               k TEXT PRIMARY KEY, v BLOB NOT NULL, updated_at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS docs (
               key TEXT PRIMARY KEY, project_id TEXT NOT NULL, room_chat TEXT NOT NULL,
               cursor INTEGER NOT NULL, snapshot BLOB NOT NULL, bytes INTEGER NOT NULL,
               updated_at INTEGER NOT NULL, opened_at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS pending (
               key TEXT NOT NULL, batch_id TEXT NOT NULL, bytes BLOB NOT NULL, seq INTEGER NOT NULL,
               PRIMARY KEY (key, batch_id));
             PRAGMA user_version={SCHEMA_VERSION};"
        ))?;
        Ok(Store {
            conn,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    // --- key/value -----------------------------------------------------------

    pub fn put(&self, key: &str, value: &[u8]) {
        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO kv (k, v, updated_at) VALUES (?1, ?2, ?3)",
            params![key, value, now_ms() as i64],
        );
    }

    pub fn put_json<T: serde::Serialize>(&self, key: &str, value: &T) {
        if let Ok(bytes) = serde_json::to_vec(value) {
            self.put(key, &bytes);
        }
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.conn
            .query_row("SELECT v FROM kv WHERE k = ?1", params![key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn get_json<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        serde_json::from_slice(&self.get(key)?).ok()
    }

    pub fn remove(&self, key: &str) {
        let _ = self
            .conn
            .execute("DELETE FROM kv WHERE k = ?1", params![key]);
    }

    /// Every key under `prefix` (the avatar cache's URLs).
    pub fn keys_with_prefix(&self, prefix: &str) -> Vec<String> {
        let Ok(mut st) = self
            .conn
            .prepare("SELECT k FROM kv WHERE k LIKE ?1 ESCAPE '\\'")
        else {
            return Vec::new();
        };
        let pattern = format!(
            "{}%",
            prefix
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        st.query_map(params![pattern], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    // --- docs ----------------------------------------------------------------

    /// Snapshot and cursor land in one statement so they can never disagree.
    /// A snapshot over the per-doc cap replaces nothing and drops the old row.
    pub fn save_doc(
        &self,
        key: &str,
        project_id: &str,
        room_chat: &str,
        cursor: i64,
        snapshot: &[u8],
    ) {
        if snapshot.len() > PER_DOC_CAP_BYTES {
            let _ = self
                .conn
                .execute("DELETE FROM docs WHERE key = ?1", params![key]);
            return;
        }
        let now = now_ms() as i64;
        let _ = self.conn.execute(
            "INSERT INTO docs (key, project_id, room_chat, cursor, snapshot, bytes, updated_at, opened_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(key) DO UPDATE SET cursor = excluded.cursor, snapshot = excluded.snapshot,
               bytes = excluded.bytes, updated_at = excluded.updated_at",
            params![key, project_id, room_chat, cursor, snapshot, snapshot.len() as i64, now],
        );
    }

    /// Every saved room, most recently opened first.
    pub fn load_docs(&self) -> Vec<SavedDoc> {
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT key, project_id, room_chat, cursor, snapshot FROM docs ORDER BY opened_at DESC",
        ) else {
            return Vec::new();
        };
        stmt.query_map([], |r| {
            Ok(SavedDoc {
                key: r.get(0)?,
                project_id: r.get(1)?,
                room_chat: r.get(2)?,
                cursor: r.get(3)?,
                snapshot: r.get(4)?,
            })
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// The user opened this chat: it is the last to be evicted.
    pub fn touch_doc(&self, key: &str) {
        let _ = self.conn.execute(
            "UPDATE docs SET opened_at = ?1 WHERE key = ?2",
            params![now_ms() as i64, key],
        );
    }

    pub fn delete_doc(&self, key: &str) {
        let _ = self
            .conn
            .execute("DELETE FROM docs WHERE key = ?1", params![key]);
        let _ = self
            .conn
            .execute("DELETE FROM pending WHERE key = ?1", params![key]);
    }

    // --- pending pushes (rows the relay has not ACKed yet) ---------------------

    pub fn save_pending(&self, key: &str, batches: &[(String, Vec<u8>)]) {
        let _ = self
            .conn
            .execute("DELETE FROM pending WHERE key = ?1", params![key]);
        for (i, (batch_id, bytes)) in batches.iter().enumerate() {
            let _ = self.conn.execute(
                "INSERT OR REPLACE INTO pending (key, batch_id, bytes, seq) VALUES (?1, ?2, ?3, ?4)",
                params![key, batch_id, bytes, i as i64],
            );
        }
    }

    pub fn load_pending(&self) -> HashMap<Id, Vec<(String, Vec<u8>)>> {
        let mut out: HashMap<Id, Vec<(String, Vec<u8>)>> = HashMap::new();
        let Ok(mut stmt) = self
            .conn
            .prepare("SELECT key, batch_id, bytes FROM pending ORDER BY key, seq")
        else {
            return out;
        };
        if let Ok(rows) = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        }) {
            for (key, batch, bytes) in rows.flatten() {
                out.entry(key).or_default().push((batch, bytes));
            }
        }
        out
    }

    // --- caps and usage --------------------------------------------------------

    pub fn usage(&self) -> Usage {
        let docs = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(bytes), 0), COUNT(CASE WHEN key NOT LIKE 'proj:%' THEN 1 END) FROM docs",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
            .unwrap_or((0, 0));
        let kv: i64 = self
            .conn
            .query_row("SELECT COALESCE(SUM(LENGTH(v)), 0) FROM kv", [], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        Usage {
            bytes: docs.0 + kv,
            docs: docs.1,
        }
    }

    /// Drop the least recently opened docs until the total fits the cap.
    pub fn enforce_caps(&self) {
        let mut total: i64 = self
            .conn
            .query_row("SELECT COALESCE(SUM(bytes), 0) FROM docs", [], |r| r.get(0))
            .unwrap_or(0);
        if total <= TOTAL_CAP_BYTES {
            return;
        }
        let Ok(mut stmt) = self
            .conn
            .prepare("SELECT key, bytes FROM docs ORDER BY opened_at ASC, updated_at ASC")
        else {
            return;
        };
        let victims: Vec<(String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default();
        for (key, bytes) in victims {
            if total <= TOTAL_CAP_BYTES {
                break;
            }
            let _ = self
                .conn
                .execute("DELETE FROM docs WHERE key = ?1", params![key]);
            total -= bytes;
        }
        eprintln!("store: evicted down to {total} bytes");
    }
}

/// Delete the database and its WAL side files (sign-out, corrupt file).
pub fn remove_files(path: &Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(p));
    }
}
