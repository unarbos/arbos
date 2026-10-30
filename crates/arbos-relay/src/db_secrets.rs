//! Storage for the secrets system (D83): secrets with versioned ciphertext, per-agent Access
//! rows, requests the agent is waiting on, the per-user kill switch, the per-user wrapped DEK,
//! and the append-only audit log. Same SQLite file; created from `Db::open`. Values are
//! ciphertext here and nowhere else; no row in this module ever holds a plaintext value.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use arbos_core::ids::now_ms;

use crate::db::Db;

pub const SECRETS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS secrets (
  id TEXT PRIMARY KEY, user_id TEXT NOT NULL, name TEXT NOT NULL, env TEXT NOT NULL,
  kind TEXT NOT NULL, note TEXT NOT NULL DEFAULT '', hosts TEXT NOT NULL DEFAULT '[]',
  ask_every_time INTEGER NOT NULL DEFAULT 0, expires_at INTEGER, current_version INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, last_used_at INTEGER,
  daytona_secrets TEXT NOT NULL DEFAULT '{}', inject_env INTEGER NOT NULL DEFAULT 0,
  others_ask TEXT NOT NULL DEFAULT 'refuse',
  source_kind TEXT NOT NULL DEFAULT 'settings', source_project_id TEXT, source_chat_id TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS secrets_user_name ON secrets(user_id, name);
CREATE UNIQUE INDEX IF NOT EXISTS secrets_user_env ON secrets(user_id, env);
CREATE TABLE IF NOT EXISTS secret_versions (
  secret_id TEXT NOT NULL, version INTEGER NOT NULL, fields_enc TEXT NOT NULL,
  created_at INTEGER NOT NULL, retire_at INTEGER,
  PRIMARY KEY (secret_id, version)
);
CREATE TABLE IF NOT EXISTS secret_access (
  id TEXT PRIMARY KEY, user_id TEXT NOT NULL, project_id TEXT NOT NULL,
  target_kind TEXT NOT NULL, target_id TEXT NOT NULL, note TEXT NOT NULL DEFAULT '',
  hosts TEXT NOT NULL DEFAULT '[]', scope_kind TEXT NOT NULL DEFAULT 'all', scope_chat_id TEXT,
  enabled INTEGER NOT NULL DEFAULT 1, created_from TEXT NOT NULL DEFAULT 'settings',
  created_at INTEGER NOT NULL, disabled_at INTEGER, last_used_at INTEGER,
  UNIQUE (project_id, target_kind, target_id)
);
CREATE INDEX IF NOT EXISTS secret_access_user ON secret_access(user_id);
CREATE TABLE IF NOT EXISTS secret_requests (
  id TEXT PRIMARY KEY, user_id TEXT NOT NULL, project_id TEXT NOT NULL, chat_id TEXT NOT NULL DEFAULT '',
  target_kind TEXT NOT NULL, target_id TEXT, target_ref TEXT NOT NULL, purpose TEXT NOT NULL DEFAULT '',
  use_kind TEXT NOT NULL, host TEXT, status TEXT NOT NULL DEFAULT 'pending', decided_by TEXT,
  created_at INTEGER NOT NULL, decided_at INTEGER, expires_at INTEGER NOT NULL, asked_by TEXT
);
CREATE INDEX IF NOT EXISTS secret_requests_user ON secret_requests(user_id, created_at);
CREATE TABLE IF NOT EXISTS user_keys (
  user_id TEXT PRIMARY KEY, dek_wrapped TEXT NOT NULL, kek_version INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL, rotated_at INTEGER
);
CREATE TABLE IF NOT EXISTS user_secrets_settings (
  user_id TEXT PRIMARY KEY, paused INTEGER NOT NULL DEFAULT 0, paused_at INTEGER, paused_by TEXT
);
CREATE TABLE IF NOT EXISTS secret_audit (
  id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, at INTEGER NOT NULL, action TEXT NOT NULL,
  target_kind TEXT NOT NULL, target_id TEXT NOT NULL, target_name TEXT NOT NULL,
  project_id TEXT, chat_id TEXT, request_id TEXT, access_id TEXT, use_kind TEXT, host TEXT,
  by TEXT NOT NULL, detail TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS secret_audit_user_at ON secret_audit(user_id, at);
CREATE INDEX IF NOT EXISTS secret_audit_target ON secret_audit(target_id, at);
-- Connected vaults (D83 §3, Phase 2): a provider token (ciphertext under the user DEK, AAD =
-- vault id), what it can see, and the cached item list (names only).
CREATE TABLE IF NOT EXISTS vaults (
  id TEXT PRIMARY KEY, user_id TEXT NOT NULL, provider TEXT NOT NULL, name TEXT NOT NULL,
  token_enc TEXT, external_vaults TEXT NOT NULL DEFAULT '[]', items TEXT NOT NULL DEFAULT '[]',
  items_count INTEGER NOT NULL DEFAULT 0, items_refreshed_at INTEGER, token_expires_at INTEGER,
  status TEXT NOT NULL DEFAULT 'ok', last_error TEXT, last_refreshed_at INTEGER,
  created_at INTEGER NOT NULL, disconnected_at INTEGER
);
CREATE INDEX IF NOT EXISTS vaults_user ON vaults(user_id);
-- Per-item host pinning for a vault Access: the first use of an item at a host pins it.
CREATE TABLE IF NOT EXISTS vault_access_hosts (
  access_id TEXT NOT NULL, item_ref TEXT NOT NULL, host TEXT NOT NULL, pinned_at INTEGER NOT NULL,
  PRIMARY KEY (access_id, item_ref, host)
);
-- Connected-vault items the owner put into the agents' environment as real values (D83
-- decision 2 as decided 2026-10-18): one env name per item; the row is the switch.
CREATE TABLE IF NOT EXISTS vault_item_env (
  vault_id TEXT NOT NULL, item_ref TEXT NOT NULL, env TEXT NOT NULL, created_at INTEGER NOT NULL,
  PRIMARY KEY (vault_id, item_ref)
);
-- Provider reads per vault, per UTC hour and per UTC day, so the Vault page can show
-- "Reads this hour 12 / 1,000" and the resolver stays under the plan's limits.
CREATE TABLE IF NOT EXISTS vault_reads (
  vault_id TEXT NOT NULL, window TEXT NOT NULL, reads INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (vault_id, window)
);
-- People the owner named who may ask an agent to use this secret (#1041, #1442; spec §7.7).
-- The owner always may; everyone else needs a row with `enabled`. `note` is constraints the
-- agent must honour (e.g. "only 1k usd of credit usage per day") and is shown with the secret
-- in the box. `scope_kind` = all | chat; `created_from` = settings | card:<request id>.
CREATE TABLE IF NOT EXISTS secret_directors (
  secret_id TEXT NOT NULL, user_id TEXT NOT NULL, note TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL, enabled INTEGER NOT NULL DEFAULT 1, disabled_at INTEGER,
  scope_kind TEXT NOT NULL DEFAULT 'all', scope_chat_id TEXT, last_asked_at INTEGER,
  created_from TEXT NOT NULL DEFAULT 'settings',
  PRIMARY KEY (secret_id, user_id)
);
CREATE INDEX IF NOT EXISTS secret_directors_user ON secret_directors(user_id);
"#;

/// Forward-only, additive migrations for databases created before a column existed.
pub fn migrate_secrets(c: &rusqlite::Connection) {
    let _ = c.execute(
        "ALTER TABLE secrets ADD COLUMN inject_env INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = c.execute_batch(
        "CREATE TABLE IF NOT EXISTS secret_directors (
           secret_id TEXT NOT NULL, user_id TEXT NOT NULL, note TEXT NOT NULL DEFAULT '',
           created_at INTEGER NOT NULL,
           PRIMARY KEY (secret_id, user_id)
         );
         CREATE INDEX IF NOT EXISTS secret_directors_user ON secret_directors(user_id);",
    );
    let _ = c.execute(
        "ALTER TABLE secrets ADD COLUMN others_ask TEXT NOT NULL DEFAULT 'refuse'",
        [],
    );
    let _ = c.execute("ALTER TABLE secret_requests ADD COLUMN asked_by TEXT", []);
    for col in [
        "source_kind TEXT NOT NULL DEFAULT 'settings'",
        "source_project_id TEXT",
        "source_chat_id TEXT",
    ] {
        let _ = c.execute(&format!("ALTER TABLE secrets ADD COLUMN {col}"), []);
    }
    for col in [
        "enabled INTEGER NOT NULL DEFAULT 1",
        "disabled_at INTEGER",
        "scope_kind TEXT NOT NULL DEFAULT 'all'",
        "scope_chat_id TEXT",
        "last_asked_at INTEGER",
        "created_from TEXT NOT NULL DEFAULT 'settings'",
    ] {
        let _ = c.execute(
            &format!("ALTER TABLE secret_directors ADD COLUMN {col}"),
            [],
        );
    }
}

/// A pending request nobody answers expires after this long (D76: 10 min silence = denied).
pub const REQUEST_PENDING_MS: i64 = 10 * 60 * 1000;
/// `Allow once` is good for one request within this window.
pub const ALLOW_ONCE_MS: i64 = 5 * 60 * 1000;
/// The previous value keeps working this long after a replace (AWSCURRENT/AWSPREVIOUS idea).
pub const PREVIOUS_VERSION_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize)]
pub struct Secret {
    pub id: String,
    pub user_id: String,
    pub name: String,
    /// Environment variable name the agent sees (`STRIPE_KEY`); derived from `name`.
    pub env: String,
    /// `key | login | text`.
    pub kind: String,
    pub note: String,
    pub hosts: Vec<String>,
    pub ask_every_time: bool,
    pub expires_at: Option<i64>,
    pub current_version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used_at: Option<i64>,
    /// field → Daytona secret id, when mirrored as Daytona Secrets (D28).
    pub daytona_secrets: Value,
    /// "Real value in the agent's environment": agents with Access get the value itself as
    /// `$ENV` in their box, no card per use (D83 decision 2 as decided 2026-10-18).
    pub inject_env: bool,
    /// Where it came from: `settings` (the New secret form) or `chat` (handed to an agent in a
    /// chat — the request card or "save this"); `source_project_id` / `source_chat_id` say where.
    pub source_kind: String,
    pub source_project_id: Option<String>,
    pub source_chat_id: Option<String>,
    /// What happens when someone who is neither the owner nor on the People list asks an agent
    /// to use it: `refuse` (default) or `card` (the owner gets a card naming who asked).
    pub others_ask: String,
}

#[derive(Debug, Clone)]
pub struct SecretVersion {
    pub secret_id: String,
    pub version: i64,
    pub fields_enc: String,
    pub created_at: i64,
    pub retire_at: Option<i64>,
}

/// A person the owner authorized to ask agents to use one secret (the People list; the API and
/// the apps say "people", the table keeps its #1041 name). `note` is shown to the agent.
#[derive(Debug, Clone, Serialize)]
pub struct SecretDirector {
    pub secret_id: String,
    pub user_id: String,
    pub note: String,
    pub created_at: i64,
    pub enabled: bool,
    pub disabled_at: Option<i64>,
    /// `all | chat`.
    pub scope_kind: String,
    pub scope_chat_id: Option<String>,
    pub last_asked_at: Option<i64>,
    /// `settings | card:<request id>`.
    pub created_from: String,
}

impl SecretDirector {
    /// A fresh standing row added from Settings.
    pub fn new(secret_id: &str, user_id: &str, note: &str, now: i64) -> Self {
        Self {
            secret_id: secret_id.to_owned(),
            user_id: user_id.to_owned(),
            note: note.to_owned(),
            created_at: now,
            enabled: true,
            disabled_at: None,
            scope_kind: "all".into(),
            scope_chat_id: None,
            last_asked_at: None,
            created_from: "settings".into(),
        }
    }

    /// Does this row let its person ask in `chat_id` right now?
    pub fn covers(&self, chat_id: Option<&str>) -> bool {
        self.enabled && (self.scope_kind != "chat" || self.scope_chat_id.as_deref() == chat_id)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Access {
    pub id: String,
    pub user_id: String,
    pub project_id: String,
    /// `secret | vault`.
    pub target_kind: String,
    pub target_id: String,
    pub note: String,
    /// Hosts pinned for this agent (first use or Settings); the secret's own hosts also apply.
    pub hosts: Vec<String>,
    /// `all | chat`.
    pub scope_kind: String,
    pub scope_chat_id: Option<String>,
    pub enabled: bool,
    /// `settings | card:<request_id>`.
    pub created_from: String,
    pub created_at: i64,
    pub disabled_at: Option<i64>,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SecretRequest {
    pub id: String,
    pub user_id: String,
    pub project_id: String,
    pub chat_id: String,
    /// `secret | vault`.
    pub target_kind: String,
    pub target_id: Option<String>,
    /// The secret's env name, or an `op://` reference.
    pub target_ref: String,
    pub purpose: String,
    /// `header | env | browser_fill`.
    pub use_kind: String,
    pub host: Option<String>,
    /// `pending | allowed_once | allowed_chat | allowed_always | denied | expired`.
    pub status: String,
    pub decided_by: Option<String>,
    pub created_at: i64,
    pub decided_at: Option<i64>,
    pub expires_at: i64,
    /// The person (user id) who asked the agent, when it was not the owner (spec §7.7).
    pub asked_by: Option<String>,
}

impl SecretRequest {
    pub fn is_allowed(&self) -> bool {
        matches!(
            self.status.as_str(),
            "allowed_once" | "allowed_chat" | "allowed_always"
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SecretAudit {
    pub id: i64,
    pub user_id: String,
    pub at: i64,
    pub action: String,
    pub target_kind: String,
    pub target_id: String,
    pub target_name: String,
    pub project_id: Option<String>,
    pub chat_id: Option<String>,
    pub request_id: Option<String>,
    pub access_id: Option<String>,
    pub use_kind: Option<String>,
    pub host: Option<String>,
    pub by: String,
    pub detail: Value,
}

/// What to write to the audit log (`SecretAudit` without the row id and time).
#[derive(Debug, Clone, Default)]
pub struct AuditEntry {
    pub action: String,
    pub target_kind: String,
    pub target_id: String,
    pub target_name: String,
    pub project_id: Option<String>,
    pub chat_id: Option<String>,
    pub request_id: Option<String>,
    pub access_id: Option<String>,
    pub use_kind: Option<String>,
    pub host: Option<String>,
    pub by: String,
    pub detail: Value,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct SecretsSettings {
    pub paused: bool,
    pub paused_at: Option<i64>,
    pub paused_by: Option<String>,
}

/// Filter for `list_audit`.
#[derive(Debug, Clone, Default)]
pub struct AuditQuery {
    pub target_id: Option<String>,
    pub project_id: Option<String>,
    pub before: Option<i64>,
    pub since: Option<i64>,
    pub limit: i64,
}

fn json_list(s: String) -> Vec<String> {
    serde_json::from_str(&s).unwrap_or_default()
}

fn to_json_list(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".into())
}

const SECRET_COLS: &str = "id, user_id, name, env, kind, note, hosts, ask_every_time, expires_at, current_version, created_at, updated_at, last_used_at, daytona_secrets, inject_env, others_ask, source_kind, source_project_id, source_chat_id";

fn row_to_secret(r: &rusqlite::Row<'_>) -> rusqlite::Result<Secret> {
    Ok(Secret {
        id: r.get(0)?,
        user_id: r.get(1)?,
        name: r.get(2)?,
        env: r.get(3)?,
        kind: r.get(4)?,
        note: r.get(5)?,
        hosts: json_list(r.get(6)?),
        ask_every_time: r.get::<_, i64>(7)? != 0,
        expires_at: r.get(8)?,
        current_version: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        last_used_at: r.get(12)?,
        daytona_secrets: serde_json::from_str(&r.get::<_, String>(13)?).unwrap_or_default(),
        inject_env: r.get::<_, i64>(14)? != 0,
        others_ask: r.get(15)?,
        source_kind: r.get(16)?,
        source_project_id: r.get(17)?,
        source_chat_id: r.get(18)?,
    })
}

const ACCESS_COLS: &str = "id, user_id, project_id, target_kind, target_id, note, hosts, scope_kind, scope_chat_id, enabled, created_from, created_at, disabled_at, last_used_at";

fn row_to_access(r: &rusqlite::Row<'_>) -> rusqlite::Result<Access> {
    Ok(Access {
        id: r.get(0)?,
        user_id: r.get(1)?,
        project_id: r.get(2)?,
        target_kind: r.get(3)?,
        target_id: r.get(4)?,
        note: r.get(5)?,
        hosts: json_list(r.get(6)?),
        scope_kind: r.get(7)?,
        scope_chat_id: r.get(8)?,
        enabled: r.get::<_, i64>(9)? != 0,
        created_from: r.get(10)?,
        created_at: r.get(11)?,
        disabled_at: r.get(12)?,
        last_used_at: r.get(13)?,
    })
}

const REQUEST_COLS: &str = "id, user_id, project_id, chat_id, target_kind, target_id, target_ref, purpose, use_kind, host, status, decided_by, created_at, decided_at, expires_at, asked_by";

fn row_to_request(r: &rusqlite::Row<'_>) -> rusqlite::Result<SecretRequest> {
    Ok(SecretRequest {
        id: r.get(0)?,
        user_id: r.get(1)?,
        project_id: r.get(2)?,
        chat_id: r.get(3)?,
        target_kind: r.get(4)?,
        target_id: r.get(5)?,
        target_ref: r.get(6)?,
        purpose: r.get(7)?,
        use_kind: r.get(8)?,
        host: r.get(9)?,
        status: r.get(10)?,
        decided_by: r.get(11)?,
        created_at: r.get(12)?,
        decided_at: r.get(13)?,
        expires_at: r.get(14)?,
        asked_by: r.get(15)?,
    })
}

const AUDIT_COLS: &str = "id, user_id, at, action, target_kind, target_id, target_name, project_id, chat_id, request_id, access_id, use_kind, host, by, detail";

fn row_to_audit(r: &rusqlite::Row<'_>) -> rusqlite::Result<SecretAudit> {
    Ok(SecretAudit {
        id: r.get(0)?,
        user_id: r.get(1)?,
        at: r.get(2)?,
        action: r.get(3)?,
        target_kind: r.get(4)?,
        target_id: r.get(5)?,
        target_name: r.get(6)?,
        project_id: r.get(7)?,
        chat_id: r.get(8)?,
        request_id: r.get(9)?,
        access_id: r.get(10)?,
        use_kind: r.get(11)?,
        host: r.get(12)?,
        by: r.get(13)?,
        detail: serde_json::from_str(&r.get::<_, String>(14)?).unwrap_or_default(),
    })
}

/// Pending requests past `expires_at` become `expired` on read; `allowed_once` grants past
/// theirs too (one request, five minutes).
fn expire_requests(c: &Connection) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE secret_requests SET status = 'expired', decided_at = COALESCE(decided_at, ?1)
         WHERE status IN ('pending', 'allowed_once') AND expires_at < ?1",
        params![now_ms()],
    )
    .map(|_| ())
}

impl Db {
    // ---- per-user DEK -------------------------------------------------------------------------

    pub fn user_dek_wrapped(&self, user_id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT dek_wrapped FROM user_keys WHERE user_id = ?1",
                params![user_id],
                |r| r.get(0),
            )
            .optional()
        })
    }

    /// Store a freshly wrapped DEK; a row that already exists wins (two requests racing).
    pub fn insert_user_dek(&self, user_id: &str, wrapped: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO user_keys(user_id, dek_wrapped, kek_version, created_at) VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(user_id) DO NOTHING",
                params![user_id, wrapped, now_ms()],
            )
            .map(|_| ())
        })
    }

    // ---- secrets ------------------------------------------------------------------------------

    pub fn insert_secret(&self, s: &Secret, first: &SecretVersion) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO secrets(id, user_id, name, env, kind, note, hosts, ask_every_time, expires_at, current_version,
                   created_at, updated_at, last_used_at, daytona_secrets, inject_env, others_ask,
                   source_kind, source_project_id, source_chat_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
                params![
                    s.id, s.user_id, s.name, s.env, s.kind, s.note, to_json_list(&s.hosts),
                    s.ask_every_time as i64, s.expires_at, s.current_version, s.created_at, s.updated_at,
                    s.last_used_at, s.daytona_secrets.to_string(), s.inject_env as i64, s.others_ask,
                    s.source_kind, s.source_project_id, s.source_chat_id
                ],
            )?;
            c.execute(
                "INSERT INTO secret_versions(secret_id, version, fields_enc, created_at, retire_at) VALUES (?1,?2,?3,?4,?5)",
                params![first.secret_id, first.version, first.fields_enc, first.created_at, first.retire_at],
            )?;
            Ok(())
        })
    }

    /// Metadata update (name, env, note, hosts, ask_every_time, inject_env, expires_at, daytona
    /// ids); the owner, kind and version are untouched.
    pub fn update_secret(&self, s: &Secret) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE secrets SET name = ?3, env = ?4, note = ?5, hosts = ?6, ask_every_time = ?7, expires_at = ?8,
                   updated_at = ?9, daytona_secrets = ?10, inject_env = ?11, others_ask = ?12
                 WHERE id = ?1 AND user_id = ?2",
                params![
                    s.id, s.user_id, s.name, s.env, s.note, to_json_list(&s.hosts), s.ask_every_time as i64,
                    s.expires_at, now_ms(), s.daytona_secrets.to_string(), s.inject_env as i64, s.others_ask
                ],
            )
            .map(|_| ())
        })
    }

    /// New ciphertext: version n+1 becomes current, version n retires in 24 h, older ones go now.
    pub fn add_secret_version(
        &self,
        user_id: &str,
        secret_id: &str,
        fields_enc: &str,
    ) -> Result<i64> {
        let now = now_ms();
        self.with(|c| {
            let current: Option<i64> = c
                .query_row(
                    "SELECT current_version FROM secrets WHERE id = ?1 AND user_id = ?2",
                    params![secret_id, user_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(current) = current else {
                return Ok(0);
            };
            let next = current + 1;
            c.execute(
                "DELETE FROM secret_versions WHERE secret_id = ?1 AND version < ?2",
                params![secret_id, current],
            )?;
            c.execute(
                "UPDATE secret_versions SET retire_at = ?3 WHERE secret_id = ?1 AND version = ?2",
                params![secret_id, current, now + PREVIOUS_VERSION_MS],
            )?;
            c.execute(
                "INSERT INTO secret_versions(secret_id, version, fields_enc, created_at, retire_at) VALUES (?1,?2,?3,?4,NULL)",
                params![secret_id, next, fields_enc, now],
            )?;
            c.execute(
                "UPDATE secrets SET current_version = ?3, updated_at = ?4 WHERE id = ?1 AND user_id = ?2",
                params![secret_id, user_id, next, now],
            )?;
            Ok(next)
        })
    }

    /// Live versions of a secret: the current one first, then a previous one still inside its
    /// 24 h window. Retired rows are deleted on the way.
    pub fn secret_versions(&self, secret_id: &str) -> Result<Vec<SecretVersion>> {
        self.with(|c| {
            c.execute(
                "DELETE FROM secret_versions WHERE secret_id = ?1 AND retire_at IS NOT NULL AND retire_at < ?2",
                params![secret_id, now_ms()],
            )?;
            let mut st = c.prepare(
                "SELECT secret_id, version, fields_enc, created_at, retire_at FROM secret_versions
                 WHERE secret_id = ?1 ORDER BY version DESC",
            )?;
            let rows = st.query_map(params![secret_id], |r| {
                Ok(SecretVersion {
                    secret_id: r.get(0)?,
                    version: r.get(1)?,
                    fields_enc: r.get(2)?,
                    created_at: r.get(3)?,
                    retire_at: r.get(4)?,
                })
            })?;
            rows.collect()
        })
    }

    pub fn list_secrets(&self, user_id: &str) -> Result<Vec<Secret>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {SECRET_COLS} FROM secrets WHERE user_id = ?1 ORDER BY name COLLATE NOCASE"
            ))?;
            let rows = st.query_map(params![user_id], row_to_secret)?;
            rows.collect()
        })
    }

    pub fn secret(&self, user_id: &str, id: &str) -> Result<Option<Secret>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {SECRET_COLS} FROM secrets WHERE id = ?1 AND user_id = ?2"),
                params![id, user_id],
                row_to_secret,
            )
            .optional()
        })
    }

    /// By display name or env name (both unique per user).
    pub fn secret_by_name(&self, user_id: &str, name_or_env: &str) -> Result<Option<Secret>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {SECRET_COLS} FROM secrets WHERE user_id = ?1 AND (name = ?2 OR env = ?2)"
                ),
                params![user_id, name_or_env],
                row_to_secret,
            )
            .optional()
        })
    }

    pub fn touch_secret(&self, user_id: &str, id: &str, project_id: &str) -> Result<()> {
        self.with(|c| {
            let now = now_ms();
            c.execute(
                "UPDATE secrets SET last_used_at = ?3 WHERE id = ?1 AND user_id = ?2",
                params![id, user_id, now],
            )?;
            c.execute(
                "UPDATE secret_access SET last_used_at = ?3 WHERE target_kind = 'secret' AND target_id = ?1 AND project_id = ?2",
                params![id, project_id, now],
            )?;
            Ok(())
        })
    }

    /// Delete a secret, its versions, its accesses and its directors; open requests expire; audit stays.
    pub fn delete_secret(&self, user_id: &str, id: &str) -> Result<bool> {
        self.with(|c| {
            let n = c.execute(
                "DELETE FROM secrets WHERE id = ?1 AND user_id = ?2",
                params![id, user_id],
            )?;
            if n > 0 {
                c.execute(
                    "DELETE FROM secret_versions WHERE secret_id = ?1",
                    params![id],
                )?;
                c.execute(
                    "DELETE FROM secret_access WHERE target_kind = 'secret' AND target_id = ?1",
                    params![id],
                )?;
                c.execute(
                    "DELETE FROM secret_directors WHERE secret_id = ?1",
                    params![id],
                )?;
                c.execute(
                    "UPDATE secret_requests SET status = 'expired', decided_at = ?2 WHERE target_id = ?1 AND status IN ('pending', 'allowed_once')",
                    params![id, now_ms()],
                )?;
            }
            Ok(n > 0)
        })
    }

    // ---- directors (who may direct the agent to use a secret) --------------------------------

    pub fn upsert_secret_director(&self, d: &SecretDirector) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO secret_directors(secret_id, user_id, note, created_at, enabled, disabled_at,
                   scope_kind, scope_chat_id, last_asked_at, created_from)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                 ON CONFLICT(secret_id, user_id) DO UPDATE SET note = excluded.note, enabled = excluded.enabled,
                   disabled_at = excluded.disabled_at, scope_kind = excluded.scope_kind,
                   scope_chat_id = excluded.scope_chat_id, last_asked_at = excluded.last_asked_at",
                params![
                    d.secret_id, d.user_id, d.note, d.created_at, d.enabled as i64, d.disabled_at,
                    d.scope_kind, d.scope_chat_id, d.last_asked_at, d.created_from
                ],
            )
            .map(|_| ())
        })
    }

    /// Stamp the time a person on the list last asked (an allowed use directed by them).
    pub fn touch_secret_director(&self, secret_id: &str, user_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE secret_directors SET last_asked_at = ?3 WHERE secret_id = ?1 AND user_id = ?2",
                params![secret_id, user_id, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn delete_secret_director(&self, secret_id: &str, user_id: &str) -> Result<bool> {
        self.with(|c| {
            let n = c.execute(
                "DELETE FROM secret_directors WHERE secret_id = ?1 AND user_id = ?2",
                params![secret_id, user_id],
            )?;
            Ok(n > 0)
        })
    }

    pub fn secret_directors(&self, secret_id: &str) -> Result<Vec<SecretDirector>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT secret_id, user_id, note, created_at, enabled, disabled_at, scope_kind, scope_chat_id,
                   last_asked_at, created_from
                 FROM secret_directors WHERE secret_id = ?1 ORDER BY created_at",
            )?;
            let rows = st.query_map(params![secret_id], |r| {
                Ok(SecretDirector {
                    secret_id: r.get(0)?,
                    user_id: r.get(1)?,
                    note: r.get(2)?,
                    created_at: r.get(3)?,
                    enabled: r.get::<_, i64>(4)? != 0,
                    disabled_at: r.get(5)?,
                    scope_kind: r.get(6)?,
                    scope_chat_id: r.get(7)?,
                    last_asked_at: r.get(8)?,
                    created_from: r.get(9)?,
                })
            })?;
            rows.collect()
        })
    }

    /// The person's row on this secret's People list, if any (on or off, any scope).
    pub fn secret_director(
        &self,
        secret_id: &str,
        user_id: &str,
    ) -> Result<Option<SecretDirector>> {
        Ok(self
            .secret_directors(secret_id)?
            .into_iter()
            .find(|d| d.user_id == user_id))
    }

    /// May `user_id` ask an agent to use this secret in `chat_id` right now? (Row on, scope fits.)
    pub fn is_secret_director(
        &self,
        secret_id: &str,
        user_id: &str,
        chat_id: Option<&str>,
    ) -> Result<bool> {
        Ok(self
            .secret_director(secret_id, user_id)?
            .is_some_and(|d| d.covers(chat_id)))
    }

    /// Secrets whose expiry passed (for the sweeper that turns access off and tells the user).
    pub fn expired_secrets(&self) -> Result<Vec<Secret>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {SECRET_COLS} FROM secrets WHERE expires_at IS NOT NULL AND expires_at < ?1"
            ))?;
            let rows = st.query_map(params![now_ms()], row_to_secret)?;
            rows.collect()
        })
    }

    // ---- access -------------------------------------------------------------------------------

    pub fn upsert_access(&self, a: &Access) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO secret_access(id, user_id, project_id, target_kind, target_id, note, hosts, scope_kind, scope_chat_id,
                   enabled, created_from, created_at, disabled_at, last_used_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
                 ON CONFLICT(project_id, target_kind, target_id) DO UPDATE SET
                   note = excluded.note, hosts = excluded.hosts, scope_kind = excluded.scope_kind,
                   scope_chat_id = excluded.scope_chat_id, enabled = excluded.enabled, disabled_at = excluded.disabled_at",
                params![
                    a.id, a.user_id, a.project_id, a.target_kind, a.target_id, a.note, to_json_list(&a.hosts),
                    a.scope_kind, a.scope_chat_id, a.enabled as i64, a.created_from, a.created_at, a.disabled_at,
                    a.last_used_at
                ],
            )
            .map(|_| ())
        })
    }

    pub fn access(
        &self,
        project_id: &str,
        target_kind: &str,
        target_id: &str,
    ) -> Result<Option<Access>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {ACCESS_COLS} FROM secret_access WHERE project_id = ?1 AND target_kind = ?2 AND target_id = ?3"
                ),
                params![project_id, target_kind, target_id],
                row_to_access,
            )
            .optional()
        })
    }

    pub fn accesses_of_project(&self, project_id: &str) -> Result<Vec<Access>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {ACCESS_COLS} FROM secret_access WHERE project_id = ?1 ORDER BY created_at DESC"
            ))?;
            let rows = st.query_map(params![project_id], row_to_access)?;
            rows.collect()
        })
    }

    pub fn accesses_of_target(&self, target_kind: &str, target_id: &str) -> Result<Vec<Access>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {ACCESS_COLS} FROM secret_access WHERE target_kind = ?1 AND target_id = ?2 ORDER BY created_at"
            ))?;
            let rows = st.query_map(params![target_kind, target_id], row_to_access)?;
            rows.collect()
        })
    }

    /// Every access of a target off (`Remove from all agents`). Returns the rows that were on.
    pub fn disable_accesses_of_target(
        &self,
        user_id: &str,
        target_kind: &str,
        target_id: &str,
    ) -> Result<Vec<Access>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {ACCESS_COLS} FROM secret_access WHERE user_id = ?1 AND target_kind = ?2 AND target_id = ?3 AND enabled = 1"
            ))?;
            let on: Vec<Access> = st
                .query_map(params![user_id, target_kind, target_id], row_to_access)?
                .collect::<rusqlite::Result<_>>()?;
            c.execute(
                "UPDATE secret_access SET enabled = 0, disabled_at = ?4 WHERE user_id = ?1 AND target_kind = ?2 AND target_id = ?3 AND enabled = 1",
                params![user_id, target_kind, target_id, now_ms()],
            )?;
            Ok(on)
        })
    }

    pub fn delete_access(&self, user_id: &str, id: &str) -> Result<Option<Access>> {
        self.with(|c| {
            let row = c
                .query_row(
                    &format!(
                        "SELECT {ACCESS_COLS} FROM secret_access WHERE id = ?1 AND user_id = ?2"
                    ),
                    params![id, user_id],
                    row_to_access,
                )
                .optional()?;
            if row.is_some() {
                c.execute("DELETE FROM secret_access WHERE id = ?1", params![id])?;
            }
            Ok(row)
        })
    }

    // ---- requests -----------------------------------------------------------------------------

    pub fn insert_secret_request(&self, rq: &SecretRequest) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO secret_requests(id, user_id, project_id, chat_id, target_kind, target_id, target_ref, purpose, use_kind, host,
                   status, decided_by, created_at, decided_at, expires_at, asked_by)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                params![
                    rq.id, rq.user_id, rq.project_id, rq.chat_id, rq.target_kind, rq.target_id, rq.target_ref, rq.purpose,
                    rq.use_kind, rq.host, rq.status, rq.decided_by, rq.created_at, rq.decided_at, rq.expires_at, rq.asked_by
                ],
            )
            .map(|_| ())
        })
    }

    pub fn secret_request(&self, id: &str) -> Result<Option<SecretRequest>> {
        self.with(|c| {
            expire_requests(c)?;
            c.query_row(
                &format!("SELECT {REQUEST_COLS} FROM secret_requests WHERE id = ?1"),
                params![id],
                row_to_request,
            )
            .optional()
        })
    }

    /// A user's requests, pending first, newest first; `project_id` narrows to one agent.
    pub fn list_secret_requests(
        &self,
        user_id: &str,
        project_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<SecretRequest>> {
        self.with(|c| {
            expire_requests(c)?;
            let mut st = c.prepare(&format!(
                "SELECT {REQUEST_COLS} FROM secret_requests WHERE user_id = ?1 AND (?2 IS NULL OR project_id = ?2)
                 ORDER BY (status = 'pending') DESC, created_at DESC LIMIT ?3"
            ))?;
            let rows = st.query_map(params![user_id, project_id, limit], row_to_request)?;
            rows.collect()
        })
    }

    /// An undecided request for the same (project, target, use) resumes instead of a second card.
    pub fn pending_secret_request_for(
        &self,
        project_id: &str,
        target_ref: &str,
        use_kind: &str,
    ) -> Result<Option<SecretRequest>> {
        self.with(|c| {
            expire_requests(c)?;
            c.query_row(
                &format!(
                    "SELECT {REQUEST_COLS} FROM secret_requests WHERE project_id = ?1 AND target_ref = ?2 AND use_kind = ?3 AND status = 'pending'
                     ORDER BY created_at DESC LIMIT 1"
                ),
                params![project_id, target_ref, use_kind],
                row_to_request,
            )
            .optional()
        })
    }

    /// A live `allowed_once` grant for this (project, target, host, use), if any.
    pub fn allowed_once_for(
        &self,
        project_id: &str,
        target_id: &str,
        host: Option<&str>,
        use_kind: &str,
    ) -> Result<Option<SecretRequest>> {
        self.with(|c| {
            expire_requests(c)?;
            c.query_row(
                &format!(
                    "SELECT {REQUEST_COLS} FROM secret_requests WHERE project_id = ?1 AND target_id = ?2 AND use_kind = ?4
                     AND status = 'allowed_once' AND (host IS NULL OR ?3 IS NULL OR host = ?3) ORDER BY decided_at DESC LIMIT 1"
                ),
                params![project_id, target_id, host, use_kind],
                row_to_request,
            )
            .optional()
        })
    }

    /// Decide a pending request of one of `user_id`'s projects. `None` when it is not pending or
    /// not theirs. An `allowed_once` keeps a 5-minute window; the others are final.
    pub fn decide_secret_request(
        &self,
        user_id: &str,
        id: &str,
        status: &str,
        decided_by: &str,
    ) -> Result<Option<SecretRequest>> {
        self.with(|c| {
            expire_requests(c)?;
            let now = now_ms();
            let expires = if status == "allowed_once" { now + ALLOW_ONCE_MS } else { i64::MAX };
            let n = c.execute(
                "UPDATE secret_requests SET status = ?3, decided_by = ?4, decided_at = ?5, expires_at = ?6
                 WHERE id = ?1 AND user_id = ?2 AND status = 'pending'",
                params![id, user_id, status, decided_by, now, expires],
            )?;
            if n == 0 {
                return Ok(None);
            }
            c.query_row(
                &format!("SELECT {REQUEST_COLS} FROM secret_requests WHERE id = ?1"),
                params![id],
                row_to_request,
            )
            .optional()
        })
    }

    /// An `allowed_once` grant is spent by its one use.
    pub fn consume_allowed_once(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE secret_requests SET status = 'expired' WHERE id = ?1 AND status = 'allowed_once'",
                params![id],
            )
            .map(|_| ())
        })
    }

    // ---- settings (kill switch) ---------------------------------------------------------------

    pub fn secrets_settings(&self, user_id: &str) -> Result<SecretsSettings> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT paused, paused_at, paused_by FROM user_secrets_settings WHERE user_id = ?1",
                params![user_id],
                |r| {
                    Ok(SecretsSettings {
                        paused: r.get::<_, i64>(0)? != 0,
                        paused_at: r.get(1)?,
                        paused_by: r.get(2)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
        })
    }

    pub fn set_secrets_paused(
        &self,
        user_id: &str,
        paused: bool,
        by: &str,
    ) -> Result<SecretsSettings> {
        self.with(|c| {
            let now = now_ms();
            c.execute(
                "INSERT INTO user_secrets_settings(user_id, paused, paused_at, paused_by) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(user_id) DO UPDATE SET paused = excluded.paused, paused_at = excluded.paused_at, paused_by = excluded.paused_by",
                params![user_id, paused as i64, if paused { Some(now) } else { None }, if paused { Some(by) } else { None }],
            )?;
            Ok(SecretsSettings {
                paused,
                paused_at: if paused { Some(now) } else { None },
                paused_by: if paused { Some(by.to_owned()) } else { None },
            })
        })
    }

    pub fn paused_user_ids(&self) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT user_id FROM user_secrets_settings WHERE paused = 1")?;
            let rows = st.query_map([], |r| r.get(0))?;
            rows.collect()
        })
    }

    // ---- audit (append-only) ------------------------------------------------------------------

    pub fn append_secret_audit(&self, user_id: &str, e: &AuditEntry) -> Result<i64> {
        self.with(|c| {
            c.execute(
                "INSERT INTO secret_audit(user_id, at, action, target_kind, target_id, target_name, project_id, chat_id, request_id,
                   access_id, use_kind, host, by, detail)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                params![
                    user_id, now_ms(), e.action, e.target_kind, e.target_id, e.target_name, e.project_id, e.chat_id,
                    e.request_id, e.access_id, e.use_kind, e.host, e.by, e.detail.to_string()
                ],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn list_audit(&self, user_id: &str, q: &AuditQuery) -> Result<Vec<SecretAudit>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {AUDIT_COLS} FROM secret_audit WHERE user_id = ?1
                   AND (?2 IS NULL OR target_id = ?2) AND (?3 IS NULL OR project_id = ?3)
                   AND (?4 IS NULL OR at < ?4) AND (?5 IS NULL OR at >= ?5)
                 ORDER BY at DESC, id DESC LIMIT ?6"
            ))?;
            let rows = st.query_map(
                params![
                    user_id,
                    q.target_id,
                    q.project_id,
                    q.before,
                    q.since,
                    q.limit.clamp(1, 500)
                ],
                row_to_audit,
            )?;
            rows.collect()
        })
    }

    /// Counts for `GET /admin/secrets`: `(user_id, secrets, accesses_on)` per user with any secret.
    pub fn secrets_counts(&self) -> Result<Vec<(String, i64, i64)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT s.user_id, COUNT(DISTINCT s.id),
                        (SELECT COUNT(*) FROM secret_access a WHERE a.user_id = s.user_id AND a.enabled = 1)
                 FROM secrets s GROUP BY s.user_id ORDER BY s.user_id",
            )?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect()
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Vault {
    pub id: String,
    pub user_id: String,
    /// `onepassword` (more providers later, same shape).
    pub provider: String,
    pub name: String,
    pub token_enc: Option<String>,
    /// `[{ id, name }]` of the provider-side vaults the token can read.
    pub external_vaults: Value,
    /// Cached `[{ ref, id, title, category, fields: [name], urls: [] }]`; names only.
    pub items: Value,
    pub items_count: i64,
    pub items_refreshed_at: Option<i64>,
    pub token_expires_at: Option<i64>,
    /// `ok | token_invalid | rate_limited | expired`.
    pub status: String,
    pub last_error: Option<String>,
    pub last_refreshed_at: Option<i64>,
    pub created_at: i64,
    pub disconnected_at: Option<i64>,
}

const VAULT_COLS: &str = "id, user_id, provider, name, token_enc, external_vaults, items, items_count, items_refreshed_at, token_expires_at, status, last_error, last_refreshed_at, created_at, disconnected_at";

fn row_to_vault(r: &rusqlite::Row<'_>) -> rusqlite::Result<Vault> {
    Ok(Vault {
        id: r.get(0)?,
        user_id: r.get(1)?,
        provider: r.get(2)?,
        name: r.get(3)?,
        token_enc: r.get(4)?,
        external_vaults: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
        items: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
        items_count: r.get(7)?,
        items_refreshed_at: r.get(8)?,
        token_expires_at: r.get(9)?,
        status: r.get(10)?,
        last_error: r.get(11)?,
        last_refreshed_at: r.get(12)?,
        created_at: r.get(13)?,
        disconnected_at: r.get(14)?,
    })
}

impl Db {
    // ---- connected vaults -----------------------------------------------------------------------

    pub fn upsert_vault(&self, v: &Vault) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO vaults(id, user_id, provider, name, token_enc, external_vaults, items, items_count, items_refreshed_at,
                   token_expires_at, status, last_error, last_refreshed_at, created_at, disconnected_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name, token_enc = excluded.token_enc,
                   external_vaults = excluded.external_vaults, items = excluded.items, items_count = excluded.items_count,
                   items_refreshed_at = excluded.items_refreshed_at, token_expires_at = excluded.token_expires_at,
                   status = excluded.status, last_error = excluded.last_error, last_refreshed_at = excluded.last_refreshed_at,
                   disconnected_at = excluded.disconnected_at
                 WHERE vaults.user_id = excluded.user_id",
                params![
                    v.id, v.user_id, v.provider, v.name, v.token_enc, v.external_vaults.to_string(), v.items.to_string(),
                    v.items_count, v.items_refreshed_at, v.token_expires_at, v.status, v.last_error, v.last_refreshed_at,
                    v.created_at, v.disconnected_at
                ],
            )
            .map(|_| ())
        })
    }

    pub fn list_vaults(&self, user_id: &str) -> Result<Vec<Vault>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {VAULT_COLS} FROM vaults WHERE user_id = ?1 AND disconnected_at IS NULL ORDER BY created_at"
            ))?;
            let rows = st.query_map(params![user_id], row_to_vault)?;
            rows.collect()
        })
    }

    pub fn vault(&self, user_id: &str, id: &str) -> Result<Option<Vault>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {VAULT_COLS} FROM vaults WHERE id = ?1 AND user_id = ?2 AND disconnected_at IS NULL"),
                params![id, user_id],
                row_to_vault,
            )
            .optional()
        })
    }

    /// Disconnect: token ciphertext and cached items go; the row stays for the audit trail.
    pub fn disconnect_vault(&self, user_id: &str, id: &str) -> Result<bool> {
        self.with(|c| {
            let n = c.execute(
                "UPDATE vaults SET token_enc = NULL, items = '[]', items_count = 0, disconnected_at = ?3
                 WHERE id = ?1 AND user_id = ?2 AND disconnected_at IS NULL",
                params![id, user_id, now_ms()],
            )?;
            if n > 0 {
                c.execute(
                    "DELETE FROM vault_access_hosts WHERE access_id IN (SELECT id FROM secret_access WHERE target_kind = 'vault' AND target_id = ?1)",
                    params![id],
                )?;
                c.execute(
                    "DELETE FROM secret_access WHERE target_kind = 'vault' AND target_id = ?1",
                    params![id],
                )?;
                c.execute(
                    "DELETE FROM vault_item_env WHERE vault_id = ?1",
                    params![id],
                )?;
                c.execute(
                    "UPDATE secret_requests SET status = 'expired', decided_at = ?2 WHERE target_kind = 'vault' AND target_id = ?1 AND status IN ('pending', 'allowed_once')",
                    params![id, now_ms()],
                )?;
            }
            Ok(n > 0)
        })
    }

    /// Items of one vault the owner put into the environment: `(item ref, env name)`.
    pub fn vault_item_envs(&self, vault_id: &str) -> Result<Vec<(String, String)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT item_ref, env FROM vault_item_env WHERE vault_id = ?1 ORDER BY created_at",
            )?;
            let rows = st.query_map(params![vault_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
    }

    /// Env names already taken across the user's vault items, with the item each belongs to.
    pub fn vault_item_env_owner(
        &self,
        user_id: &str,
        env: &str,
    ) -> Result<Option<(String, String)>> {
        self.with(|c| {
            c.query_row(
                "SELECT e.vault_id, e.item_ref FROM vault_item_env e JOIN vaults v ON v.id = e.vault_id
                 WHERE v.user_id = ?1 AND e.env = ?2 LIMIT 1",
                params![user_id, env],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
    }

    pub fn set_vault_item_env(&self, vault_id: &str, item_ref: &str, env: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO vault_item_env(vault_id, item_ref, env, created_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(vault_id, item_ref) DO UPDATE SET env = excluded.env",
                params![vault_id, item_ref, env, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn clear_vault_item_env(&self, vault_id: &str, item_ref: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "DELETE FROM vault_item_env WHERE vault_id = ?1 AND item_ref = ?2",
                params![vault_id, item_ref],
            )
            .map(|n| n > 0)
        })
    }

    pub fn vault_item_hosts(&self, access_id: &str, item_ref: &str) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT host FROM vault_access_hosts WHERE access_id = ?1 AND item_ref = ?2 ORDER BY pinned_at",
            )?;
            let rows = st.query_map(params![access_id, item_ref], |r| r.get(0))?;
            rows.collect()
        })
    }

    /// Every pinned `(item_ref, host)` of one access.
    pub fn vault_access_hosts(&self, access_id: &str) -> Result<Vec<(String, String)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT item_ref, host FROM vault_access_hosts WHERE access_id = ?1 ORDER BY pinned_at",
            )?;
            let rows = st.query_map(params![access_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
    }

    pub fn pin_vault_item_host(&self, access_id: &str, item_ref: &str, host: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO vault_access_hosts(access_id, item_ref, host, pinned_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(access_id, item_ref, host) DO NOTHING",
                params![access_id, item_ref, host, now_ms()],
            )
            .map(|_| ())
        })
    }

    /// Count one provider read in the current hour and day windows; returns `(hour, day)` totals.
    pub fn count_vault_read(&self, vault_id: &str) -> Result<(i64, i64)> {
        let (hour, day) = read_windows();
        self.with(|c| {
            for w in [&hour, &day] {
                c.execute(
                    "INSERT INTO vault_reads(vault_id, window, reads) VALUES (?1, ?2, 1)
                     ON CONFLICT(vault_id, window) DO UPDATE SET reads = reads + 1",
                    params![vault_id, w],
                )?;
            }
            c.execute(
                "DELETE FROM vault_reads WHERE vault_id = ?1 AND window NOT IN (?2, ?3)",
                params![vault_id, hour, day],
            )?;
            let get = |w: &str| -> rusqlite::Result<i64> {
                c.query_row(
                    "SELECT reads FROM vault_reads WHERE vault_id = ?1 AND window = ?2",
                    params![vault_id, w],
                    |r| r.get(0),
                )
            };
            Ok((get(&hour)?, get(&day)?))
        })
    }

    pub fn vault_reads(&self, vault_id: &str) -> Result<(i64, i64)> {
        let (hour, day) = read_windows();
        self.with(|c| {
            let get = |w: &str| -> rusqlite::Result<i64> {
                c.query_row(
                    "SELECT reads FROM vault_reads WHERE vault_id = ?1 AND window = ?2",
                    params![vault_id, w],
                    |r| r.get(0),
                )
                .optional()
                .map(|v| v.unwrap_or(0))
            };
            Ok((get(&hour)?, get(&day)?))
        })
    }
}

/// `("h<unix hours>", "d<unix days>")` for the current UTC hour and day.
fn read_windows() -> (String, String) {
    let now_s = now_ms() / 1000;
    (format!("h{}", now_s / 3600), format!("d{}", now_s / 86_400))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "arbos-secrets-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn secret(id: &str, user: &str, name: &str) -> Secret {
        Secret {
            id: id.into(),
            user_id: user.into(),
            name: name.into(),
            env: name.to_ascii_uppercase(),
            kind: "key".into(),
            note: String::new(),
            hosts: vec![],
            ask_every_time: false,
            expires_at: None,
            current_version: 1,
            created_at: 1,
            updated_at: 1,
            last_used_at: None,
            daytona_secrets: serde_json::json!({}),
            inject_env: false,
            others_ask: "refuse".into(),
            source_kind: "settings".into(),
            source_project_id: None,
            source_chat_id: None,
        }
    }

    fn version(id: &str, v: i64) -> SecretVersion {
        SecretVersion {
            secret_id: id.into(),
            version: v,
            fields_enc: format!("enc-{v}"),
            created_at: 1,
            retire_at: None,
        }
    }

    /// Replace keeps the previous version for a while, drops older ones, and the secret stays
    /// scoped to its user; deleting the secret removes versions and accesses but not audit rows.
    #[test]
    fn versions_and_delete() {
        let dir = temp_db("versions");
        let db = Db::open(&dir.join("relay.db")).expect("db");
        db.insert_secret(&secret("s1", "alice", "stripe"), &version("s1", 1))
            .unwrap();
        assert_eq!(db.add_secret_version("alice", "s1", "enc-2").unwrap(), 2);
        assert_eq!(db.add_secret_version("alice", "s1", "enc-3").unwrap(), 3);
        assert_eq!(db.add_secret_version("bob", "s1", "enc-x").unwrap(), 0);
        let vs = db.secret_versions("s1").unwrap();
        assert_eq!(
            vs.iter().map(|v| v.version).collect::<Vec<_>>(),
            vec![3, 2],
            "current + previous only"
        );
        assert!(vs[0].retire_at.is_none() && vs[1].retire_at.is_some());
        assert_eq!(
            db.secret("alice", "s1").unwrap().unwrap().current_version,
            3
        );
        assert!(db.secret("bob", "s1").unwrap().is_none());
        assert!(db.secret_by_name("alice", "STRIPE").unwrap().is_some());

        db.upsert_access(&Access {
            id: "a1".into(),
            user_id: "alice".into(),
            project_id: "p1".into(),
            target_kind: "secret".into(),
            target_id: "s1".into(),
            note: "refunds".into(),
            hosts: vec![],
            scope_kind: "all".into(),
            scope_chat_id: None,
            enabled: true,
            created_from: "settings".into(),
            created_at: 1,
            disabled_at: None,
            last_used_at: None,
        })
        .unwrap();
        db.append_secret_audit(
            "alice",
            &AuditEntry {
                action: "created".into(),
                target_kind: "secret".into(),
                target_id: "s1".into(),
                target_name: "stripe".into(),
                by: "user:alice".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!db.delete_secret("bob", "s1").unwrap());
        assert!(db.delete_secret("alice", "s1").unwrap());
        assert!(db.secret_versions("s1").unwrap().is_empty());
        assert!(db.access("p1", "secret", "s1").unwrap().is_none());
        let q = AuditQuery {
            target_id: Some("s1".into()),
            limit: 10,
            ..Default::default()
        };
        assert_eq!(db.list_audit("alice", &q).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Requests: pending ones expire after 10 min, `allowed_once` is spent by one use, and only
    /// the owning user can decide.
    #[test]
    fn requests_decide_and_expire() {
        let dir = temp_db("requests");
        let db = Db::open(&dir.join("relay.db")).expect("db");
        let now = now_ms();
        let rq = SecretRequest {
            id: "r1".into(),
            user_id: "alice".into(),
            project_id: "p1".into(),
            chat_id: "c1".into(),
            target_kind: "secret".into(),
            target_id: Some("s1".into()),
            target_ref: "STRIPE".into(),
            purpose: "refunds".into(),
            use_kind: "header".into(),
            host: Some("api.stripe.com".into()),
            status: "pending".into(),
            decided_by: None,
            created_at: now,
            decided_at: None,
            expires_at: now + REQUEST_PENDING_MS,
            asked_by: None,
        };
        db.insert_secret_request(&rq).unwrap();
        assert!(db
            .decide_secret_request("bob", "r1", "allowed_always", "card:r1")
            .unwrap()
            .is_none());
        let decided = db
            .decide_secret_request("alice", "r1", "allowed_once", "card:r1")
            .unwrap()
            .unwrap();
        assert_eq!(decided.status, "allowed_once");
        assert!(db
            .allowed_once_for("p1", "s1", Some("api.stripe.com"), "header")
            .unwrap()
            .is_some());
        assert!(db
            .allowed_once_for("p1", "s1", Some("evil.example"), "header")
            .unwrap()
            .is_none());
        db.consume_allowed_once("r1").unwrap();
        assert!(db
            .allowed_once_for("p1", "s1", Some("api.stripe.com"), "header")
            .unwrap()
            .is_none());
        assert!(db
            .decide_secret_request("alice", "r1", "denied", "card:r1")
            .unwrap()
            .is_none());

        let mut stale = rq.clone();
        stale.id = "r2".into();
        stale.expires_at = now - 1;
        db.insert_secret_request(&stale).unwrap();
        assert_eq!(db.secret_request("r2").unwrap().unwrap().status, "expired");
        let pending = db
            .list_secret_requests("alice", None, 50)
            .unwrap()
            .into_iter()
            .filter(|r| r.status == "pending")
            .count();
        assert_eq!(pending, 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
