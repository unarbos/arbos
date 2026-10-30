//! Relay storage: one SQLite file, forward-only migrations (D50). The relay
//! stores opaque rows and never parses Loro bytes (crdt-sync §9).

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use arbos_core::ids::now_ms;
use arbos_core::skill::Skill;

/// What a user's box reported in its last `/box/hello`.
#[derive(Debug, Clone, Serialize)]
pub struct BoxAgentd {
    pub user_id: String,
    pub sha: Option<String>,
    pub version: Option<String>,
    pub image: Option<String>,
    /// `.source-sha` of the pi-ext seed inside the box (#1708).
    pub seed_sha: Option<String>,
    pub reported_at: i64,
}

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS pairing_codes (
  code TEXT PRIMARY KEY, created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, used_at INTEGER
);
CREATE TABLE IF NOT EXISTS devices (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL, revoked_at INTEGER, last_seen_at INTEGER
);
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, icon TEXT NOT NULL DEFAULT '',
  instructions TEXT NOT NULL DEFAULT '', models TEXT NOT NULL DEFAULT '{}',
  state TEXT NOT NULL DEFAULT 'waking', created_at INTEGER NOT NULL, last_active_at INTEGER NOT NULL,
  spend_cap_daily REAL NOT NULL DEFAULT 25.0,
  project_token_hash TEXT
);
CREATE TABLE IF NOT EXISTS chats (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL, title TEXT NOT NULL DEFAULT 'New chat',
  is_pinned INTEGER NOT NULL DEFAULT 0, position INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL, doc_epoch INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS chats_project ON chats(project_id, position);
CREATE TABLE IF NOT EXISTS sandboxes (
  project_id TEXT PRIMARY KEY, provider TEXT NOT NULL, sandbox_id TEXT,
  daytona_state TEXT, device_id TEXT NOT NULL, box_token_hash TEXT NOT NULL, mcp_token TEXT NOT NULL,
  snapshot TEXT, region TEXT, last_started_at INTEGER, last_stopped_at INTEGER, last_seen_at INTEGER,
  box_url TEXT, preview_token TEXT, local_pid INTEGER
);
-- One Daytona computer per user (design change 2026-10-15): every project of a user is a workspace
-- on that box. `sandboxes` above is the legacy per-project registry, kept for migration only.
CREATE TABLE IF NOT EXISTS boxes (
  user_id TEXT PRIMARY KEY, provider TEXT NOT NULL, sandbox_id TEXT,
  daytona_state TEXT, device_id TEXT NOT NULL, box_token_hash TEXT NOT NULL, mcp_token TEXT NOT NULL,
  snapshot TEXT, region TEXT, last_started_at INTEGER, last_stopped_at INTEGER, last_seen_at INTEGER,
  box_url TEXT, preview_token TEXT, local_pid INTEGER, migrated_from TEXT NOT NULL DEFAULT '[]'
);
-- A box create in flight (#2262): written before the provider is asked for the sandbox, removed
-- once the row holds the sandbox id or the create failed. A row left here after a restart names
-- the sandbox that may be running without a row pointing at it.
CREATE TABLE IF NOT EXISTS box_creates (
  user_id TEXT PRIMARY KEY, sandbox_name TEXT NOT NULL, provider TEXT NOT NULL, snapshot TEXT,
  replacing TEXT, started_at INTEGER NOT NULL
);
-- Per-user box class: the Daytona snapshot (which fixes vCPU/RAM/disk) a user's box is created
-- from; NULL = the relay default (ARBOS_DAYTONA_SNAPSHOT). Raise a user's class later by setting
-- it and recreating the box; no code change needed.
CREATE TABLE IF NOT EXISTS users (
  id TEXT PRIMARY KEY, snapshot TEXT, spend_cap_usd REAL NOT NULL DEFAULT 25.0, created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS box_wake_jobs (
  user_id TEXT PRIMARY KEY, reason TEXT NOT NULL, due_at INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL, last_error TEXT
);
CREATE TABLE IF NOT EXISTS rows (
  room TEXT NOT NULL, seq INTEGER NOT NULL, device TEXT NOT NULL, batch_id TEXT NOT NULL,
  bytes BLOB NOT NULL, created_at INTEGER NOT NULL,
  PRIMARY KEY (room, seq)
);
CREATE UNIQUE INDEX IF NOT EXISTS rows_batch ON rows(room, batch_id);
CREATE TABLE IF NOT EXISTS checkpoints (
  room TEXT PRIMARY KEY, seq INTEGER NOT NULL, frontier BLOB NOT NULL, bytes BLOB NOT NULL,
  size INTEGER NOT NULL, created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS room_seq (room TEXT PRIMARY KEY, head INTEGER NOT NULL, floor INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS blobs (
  project_id TEXT NOT NULL, key TEXT NOT NULL, size INTEGER NOT NULL, mime TEXT, created_at INTEGER NOT NULL,
  PRIMARY KEY (project_id, key)
);
CREATE TABLE IF NOT EXISTS usage (
  id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL, at INTEGER NOT NULL, api TEXT NOT NULL,
  model TEXT NOT NULL, input INTEGER NOT NULL, output INTEGER NOT NULL, cache_read INTEGER NOT NULL,
  cache_write INTEGER NOT NULL, usd REAL NOT NULL, latency_ms INTEGER NOT NULL, ttfb_ms INTEGER
);
CREATE INDEX IF NOT EXISTS usage_project_at ON usage(project_id, at);
CREATE TABLE IF NOT EXISTS wake_jobs (
  project_id TEXT PRIMARY KEY, reason TEXT NOT NULL, due_at INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL, last_error TEXT
);
CREATE TABLE IF NOT EXISTS push_queue (
  id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL, kind TEXT NOT NULL, payload TEXT NOT NULL,
  created_at INTEGER NOT NULL, delivered_at INTEGER
);
-- What each user's box reported in its last `/box/hello` (agentd commit, crate version, image).
CREATE TABLE IF NOT EXISTS box_agentd (
  user_id TEXT PRIMARY KEY, sha TEXT, version TEXT, image TEXT, reported_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS timings (
  id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL, kind TEXT NOT NULL, ms INTEGER NOT NULL,
  detail TEXT, at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS connectors (
  id TEXT PRIMARY KEY, provider TEXT NOT NULL, label TEXT NOT NULL DEFAULT '', scopes TEXT NOT NULL DEFAULT '[]',
  status TEXT NOT NULL DEFAULT 'needs_auth', config TEXT NOT NULL DEFAULT '{}', created_at INTEGER NOT NULL,
  last_used_at INTEGER, last_error TEXT
);
CREATE TABLE IF NOT EXISTS connector_tokens (
  connector_id TEXT PRIMARY KEY, access_token TEXT NOT NULL, refresh_token TEXT, expires_at INTEGER,
  token_type TEXT NOT NULL DEFAULT 'Bearer', updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS oauth_states (
  state TEXT PRIMARY KEY, provider TEXT NOT NULL, code_verifier TEXT NOT NULL, redirect_uri TEXT NOT NULL,
  scopes TEXT NOT NULL DEFAULT '[]', created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS connector_outbox (
  id INTEGER PRIMARY KEY AUTOINCREMENT, provider TEXT NOT NULL, action TEXT NOT NULL, args TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS routines (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL, chat_id TEXT NOT NULL, name TEXT NOT NULL,
  prompt TEXT NOT NULL, trigger TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL, last_run_at INTEGER, next_run_at INTEGER
);
CREATE INDEX IF NOT EXISTS routines_due ON routines(enabled, next_run_at);
CREATE TABLE IF NOT EXISTS routine_runs (
  id TEXT PRIMARY KEY, routine_id TEXT NOT NULL, started_at INTEGER NOT NULL, finished_at INTEGER,
  status TEXT NOT NULL, message_id TEXT, note TEXT
);
CREATE INDEX IF NOT EXISTS routine_runs_routine ON routine_runs(routine_id, started_at);
CREATE TABLE IF NOT EXISTS skills (
  id TEXT PRIMARY KEY, slug TEXT NOT NULL UNIQUE, name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
  tags TEXT NOT NULL DEFAULT '[]', when_to_use TEXT NOT NULL DEFAULT '', body TEXT NOT NULL DEFAULT '',
  origin_project_id TEXT, shared INTEGER NOT NULL DEFAULT 0, version INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS skills_fts USING fts5(text, id UNINDEXED, tokenize = 'porter unicode61');
CREATE TABLE IF NOT EXISTS channel_bindings (
  id TEXT PRIMARY KEY, platform TEXT NOT NULL, external_chat_id TEXT NOT NULL, external_user_id TEXT NOT NULL,
  project_id TEXT NOT NULL, chat_id TEXT NOT NULL, label TEXT NOT NULL DEFAULT '', bound_at INTEGER NOT NULL,
  UNIQUE(platform, external_chat_id, external_user_id)
);
CREATE TABLE IF NOT EXISTS channel_pair_codes (
  code TEXT PRIMARY KEY, project_id TEXT NOT NULL, chat_id TEXT NOT NULL, created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL, used_at INTEGER
);
CREATE TABLE IF NOT EXISTS channel_hints (
  platform TEXT NOT NULL, external_chat_id TEXT NOT NULL, external_user_id TEXT NOT NULL, hinted_at INTEGER NOT NULL,
  PRIMARY KEY (platform, external_chat_id, external_user_id)
);
CREATE TABLE IF NOT EXISTS channel_seen (event_key TEXT PRIMARY KEY, seen_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS channel_outbox (
  id INTEGER PRIMARY KEY AUTOINCREMENT, platform TEXT NOT NULL, external_chat_id TEXT NOT NULL, text TEXT NOT NULL,
  request_id TEXT NOT NULL, status TEXT NOT NULL, detail TEXT, binding_id TEXT, created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS audit (
  id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL, kind TEXT NOT NULL, entry TEXT NOT NULL,
  at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS audit_project ON audit(project_id, at);
CREATE TABLE IF NOT EXISTS local_devices (
  device_id TEXT PRIMARY KEY, name TEXT NOT NULL, hostname TEXT NOT NULL DEFAULT '', os TEXT NOT NULL DEFAULT '',
  home TEXT NOT NULL DEFAULT '', connected INTEGER NOT NULL DEFAULT 0, last_seen INTEGER, first_seen INTEGER NOT NULL,
  allow_reads INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS vault_items (
  id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, kind TEXT NOT NULL,
  fields TEXT NOT NULL, allowed_hosts TEXT NOT NULL DEFAULT '[]',
  created_at INTEGER NOT NULL, last_used_at INTEGER, daytona_secrets TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS vault_requests (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL, chat_id TEXT NOT NULL DEFAULT '',
  item_id TEXT, item_name TEXT NOT NULL, purpose TEXT NOT NULL DEFAULT '', use_kind TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending', created_at INTEGER NOT NULL, decided_at INTEGER, expires_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS vault_requests_project ON vault_requests(project_id, created_at);
-- Fork lineage: dest project → source project, until the workspace copy finishes (or forever as a hint).
CREATE TABLE IF NOT EXISTS project_forks (
  project_id TEXT PRIMARY KEY, source_project_id TEXT NOT NULL, created_at INTEGER NOT NULL
);
-- Agent-hosted sites (arbos.life/<prefix>/<app>/, #2060). The prefix is fixed per user on the
-- first publish; sites carry the live manifest; versions keep every published manifest; pending
-- rows are begin→commit sessions; events are the audit trail; quotas override the defaults.
CREATE TABLE IF NOT EXISTS site_prefixes (
  user_id TEXT PRIMARY KEY, prefix TEXT NOT NULL UNIQUE, created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sites (
  user_id TEXT NOT NULL, app TEXT NOT NULL, prefix TEXT NOT NULL, status TEXT NOT NULL,
  version INTEGER NOT NULL, manifest TEXT NOT NULL, bytes INTEGER NOT NULL, files INTEGER NOT NULL,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, published_at INTEGER,
  edge_synced_at INTEGER, takedown_reason TEXT, takedown_by TEXT,
  PRIMARY KEY (user_id, app)
);
CREATE TABLE IF NOT EXISTS site_versions (
  user_id TEXT NOT NULL, app TEXT NOT NULL, version INTEGER NOT NULL, manifest TEXT NOT NULL,
  bytes INTEGER NOT NULL, files INTEGER NOT NULL, created_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, app, version)
);
CREATE TABLE IF NOT EXISTS site_pending (
  user_id TEXT NOT NULL, app TEXT NOT NULL, files TEXT NOT NULL, created_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, app)
);
CREATE TABLE IF NOT EXISTS site_blobs (
  sha256 TEXT PRIMARY KEY, size INTEGER NOT NULL, created_at INTEGER NOT NULL, edge_pushed_at INTEGER
);
CREATE TABLE IF NOT EXISTS site_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, user_id TEXT NOT NULL, app TEXT NOT NULL,
  kind TEXT NOT NULL, actor TEXT NOT NULL, version INTEGER, bytes INTEGER, files INTEGER, detail TEXT
);
CREATE INDEX IF NOT EXISTS site_events_at ON site_events(at);
CREATE TABLE IF NOT EXISTS site_quotas (
  user_id TEXT PRIMARY KEY, max_sites INTEGER, max_bytes INTEGER, publish_blocked INTEGER NOT NULL DEFAULT 0
);
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub instructions: String,
    pub models: serde_json::Value,
    pub state: String,
    pub created_at: i64,
    pub last_active_at: i64,
    pub spend_cap_daily: f64,
    pub color: String,
    /// Owner; all of a user's projects share one box.
    pub user_id: String,
    /// Workspace state on the user box as last reported: `running` | `stopped`.
    pub workspace_state: String,
    /// Tree avatar slug (the manifest the apps ship); `None` on rows made before avatars, which
    /// read as the deterministic default for the id (`crate::avatar::default_for`).
    #[serde(default)]
    pub avatar: Option<String>,
    /// Avatar colour slug (manifest `colors[]`); `None` reads as the deterministic default.
    #[serde(default)]
    pub avatar_color: Option<String>,
    /// Short public blurb for the agent's profile page (not the instructions).
    #[serde(default)]
    pub description: String,
    /// The one chat every client shows for this agent (single-chat product): chosen once —
    /// most activity, ties → earliest — and kept until that chat is closed.
    #[serde(default)]
    pub primary_chat_id: Option<String>,
    /// Telegram-style initials colour override (D79), 0–6; `None` = derived from the id.
    #[serde(default)]
    pub avatar_color_index: Option<i64>,
    /// Leaf avatar (D92): the owner's shape pick, 0–8; `None` = derived from the id.
    #[serde(default)]
    pub avatar_leaf: Option<i64>,
    /// Leaf avatar (D92): the owner's disc colour pick, 0–9; `None` = derived from the id.
    #[serde(default)]
    pub avatar_leaf_color: Option<i64>,
    /// D92 amendment: an uploaded or generated picture (blob key in the project's own slot); wins
    /// over the leaf. `None` = the leaf.
    #[serde(default)]
    pub image_key: Option<String>,
    /// Stamp (ms) of the last avatar change (picture, leaf, colour); `0` = never changed.
    /// Clients order their copies of the agent by it (`avatar::identity`).
    #[serde(default)]
    pub avatar_version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub is_pinned: bool,
    pub position: i64,
    pub created_at: i64,
    pub doc_epoch: i64,
    /// `chat` (default), `group` (F6: the user plus several project agents), or `projects`
    /// (the S15 "Projects ⇄" system tab).
    #[serde(default = "default_chat_kind")]
    pub kind: String,
    /// Group chats only: participant project ids, the host project first.
    #[serde(default)]
    pub participants: Vec<String>,
    /// Session fork (#1257): the chat of the same project this one was forked from. The box
    /// seeds the new room from that chat's transcript and model context on first sight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<String>,
}

fn default_chat_kind() -> String {
    "chat".to_owned()
}

impl Chat {
    pub fn is_group(&self) -> bool {
        self.kind == "group"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sandbox {
    /// The owning user (one box per user). Legacy rows from `sandboxes` carry the project id here.
    pub user_id: String,
    pub provider: String,
    pub sandbox_id: Option<String>,
    pub daytona_state: Option<String>,
    pub device_id: String,
    pub box_token_hash: String,
    pub mcp_token: String,
    pub snapshot: Option<String>,
    pub last_started_at: Option<i64>,
    pub last_stopped_at: Option<i64>,
    pub last_seen_at: Option<i64>,
    pub box_url: Option<String>,
    pub preview_token: Option<String>,
    pub local_pid: Option<i64>,
    /// Project ids whose legacy per-project sandbox was copied into this box.
    #[serde(default)]
    pub migrated_from: Vec<String>,
}

/// One row of `box_creates`: a create that started and has not finished.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoxCreate {
    pub user_id: String,
    pub sandbox_name: String,
    pub provider: String,
    pub snapshot: Option<String>,
    /// The sandbox the user's row named when the create began (an upgrade or repair).
    pub replacing: Option<String>,
    pub started_at: i64,
}

/// A scheduled or event-triggered prompt (design §10 "Routines").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Routine {
    pub id: String,
    pub project_id: String,
    pub chat_id: String,
    pub name: String,
    pub prompt: String,
    /// `{ "kind": "cron", "expr", "tz" } | { "kind": "webhook", "secret" } | { "kind": "event", "event" }`.
    pub trigger: serde_json::Value,
    pub enabled: bool,
    pub created_at: i64,
    pub last_run_at: Option<i64>,
    pub next_run_at: Option<i64>,
}

/// A long-running objective the agent tracks across days (Goals, feedback #1091).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub project_id: String,
    pub chat_id: Option<String>,
    pub title: String,
    pub outcome: String,
    /// JSON array of `{ id?, title, status? }` steps.
    pub plan: serde_json::Value,
    /// `active | blocked | waiting | paused | done`.
    pub status: String,
    /// JSON array of `{ at, text }` progress notes.
    pub progress_log: serde_json::Value,
    pub next_step: String,
    /// Five-field cron for check-ins; `None` means no automatic check-in.
    pub check_in_cron: Option<String>,
    pub check_in_tz: String,
    /// Linked routine that fires the check-in prompt.
    pub routine_id: Option<String>,
    /// `ask | suggest | act` — how much the agent may do without asking.
    pub autonomy: String,
    pub linked_chat_ids: serde_json::Value,
    pub linked_subagent_ids: serde_json::Value,
    /// Last question asked of the user; set to avoid repeated nags.
    pub waiting_question: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_moved_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutineRun {
    pub id: String,
    pub routine_id: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    /// `queued | running | done | failed | skipped`.
    pub status: String,
    pub message_id: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub seq: i64,
    pub device: String,
    pub batch_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RoomState {
    pub head: i64,
    pub floor: i64,
    pub checkpoint_seq: i64,
    pub checkpoint_size: i64,
    pub checkpoint_frontier: Vec<u8>,
    pub row_count: i64,
    pub row_bytes: i64,
}

/// One connected external account (`connectors` table). Tokens live in `connector_tokens`, never here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connector {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub scopes: Vec<String>,
    /// `connected | needs_auth | error`.
    pub status: String,
    /// Provider-specific settings (e.g. `baseUrl` for `harness_comms`); never secrets.
    pub config: serde_json::Value,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub last_error: Option<String>,
}

/// Encrypted token row (`connector_tokens`). The strings are ciphertext from `connectors::crypto`.
#[derive(Debug, Clone)]
pub struct ConnectorTokenRow {
    pub connector_id: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub token_type: String,
}

#[derive(Debug, Clone)]
pub struct OauthState {
    pub state: String,
    pub provider: String,
    pub code_verifier: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

/// One paired external conversation (`channel_bindings`, F7): messages from `external_user_id` in
/// `external_chat_id` on `platform` land in `project_id`/`chat_id`; replies go back the same way.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelBinding {
    pub id: String,
    pub platform: String,
    pub external_chat_id: String,
    pub external_user_id: String,
    pub project_id: String,
    pub chat_id: String,
    pub label: String,
    pub bound_at: i64,
}

/// A pairing code minted from Settings → Channels (`channel_pair_codes`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelPairCode {
    pub code: String,
    pub project_id: String,
    pub chat_id: String,
    pub created_at: i64,
    pub expires_at: i64,
}

/// One credential in the vault (F8). `fields` is ciphertext from `connectors::crypto` over a JSON
/// object (`{ "url", "username", "password" }` for a login, `{ "value" }` for a secret/api_key).
/// Owned by one user: every read and write is scoped by `user_id`, and names are unique per user.
#[derive(Debug, Clone)]
pub struct VaultItem {
    pub id: String,
    #[allow(dead_code)]
    pub user_id: String,
    pub name: String,
    /// `login | secret | api_key`.
    pub kind: String,
    pub fields_enc: String,
    pub allowed_hosts: Vec<String>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    /// field → Daytona secret id, when the item was mirrored as Daytona Secrets (D28).
    pub daytona_secrets: serde_json::Value,
}

/// A box asking to use a vault item (F8). Decided by the user from a device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultRequest {
    pub id: String,
    pub project_id: String,
    pub chat_id: String,
    pub item_id: Option<String>,
    pub item_name: String,
    pub purpose: String,
    /// `env | header | browser_fill`.
    #[serde(rename = "use")]
    pub use_kind: String,
    /// `pending | approved | denied | expired`.
    pub status: String,
    pub created_at: i64,
    pub decided_at: Option<i64>,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageTotals {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub usd: f64,
    pub calls: i64,
}

/// One `arbos-local` helper that connected at least once (`local_devices`, F9).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalDevice {
    pub device_id: String,
    /// The name given at pairing (`arbos-local pair --name`), else the hostname.
    pub name: String,
    pub hostname: String,
    pub os: String,
    pub home: String,
    pub connected: bool,
    pub last_seen: Option<i64>,
    pub first_seen: i64,
    /// `read_file` / `list_dir` skip the approval card (still audited) when set.
    pub allow_reads: bool,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let c = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        c.execute_batch(
            // secure_delete: rows a shallow checkpoint replaces (secret hand-off) leave no old
            // bytes behind in the file.
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA secure_delete=ON;",
        )?;
        c.execute_batch(SCHEMA)?;
        c.execute_batch(crate::outside::SCHEMA)?;
        c.execute_batch(crate::outside_log::SCHEMA)?;
        for m in crate::outside::MIGRATIONS {
            let _ = c.execute(m, []);
        }
        for m in crate::outside_wake::MIGRATIONS {
            let _ = c.execute(m, []);
        }
        // Forward-only, additive migrations (D50).
        let _ = c.execute(
            "ALTER TABLE projects ADD COLUMN color TEXT NOT NULL DEFAULT '#8b7cf6'",
            [],
        );
        let _ = c.execute("ALTER TABLE chats ADD COLUMN closed_at INTEGER", []);
        let _ = c.execute("ALTER TABLE box_agentd ADD COLUMN seed_sha TEXT", []);
        // Agent sites: an explicit prefix move keeps the old name answering 301 for 30 days.
        let _ = c.execute("ALTER TABLE site_prefixes ADD COLUMN moved_from TEXT", []);
        let _ = c.execute("ALTER TABLE site_prefixes ADD COLUMN moved_at INTEGER", []);
        let _ = c.execute("ALTER TABLE projects ADD COLUMN avatar TEXT", []);
        let _ = c.execute("ALTER TABLE projects ADD COLUMN deleted_at INTEGER", []);
        let _ = c.execute("ALTER TABLE projects ADD COLUMN avatar_color TEXT", []);
        let _ = c.execute(
            "ALTER TABLE projects ADD COLUMN description TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = c.execute("ALTER TABLE projects ADD COLUMN primary_chat_id TEXT", []);
        let _ = c.execute(
            "ALTER TABLE projects ADD COLUMN avatar_color_index INTEGER",
            [],
        );
        let _ = c.execute("ALTER TABLE projects ADD COLUMN avatar_leaf INTEGER", []);
        let _ = c.execute(
            "ALTER TABLE projects ADD COLUMN avatar_leaf_color INTEGER",
            [],
        );
        let _ = c.execute("ALTER TABLE projects ADD COLUMN image_key TEXT", []);
        let _ = c.execute(
            "ALTER TABLE projects ADD COLUMN avatar_version INTEGER NOT NULL DEFAULT 0",
            [],
        );
        // Every row carries a stamp (ARB-394, 2026-10-26): agents whose avatar predates the
        // stamps sat at 0, so a client could hold two differing copies of one agent (the
        // old picture, a later reset) that tied — and had to guess. A row at 0 takes its
        // creation time: below any change the stamping clock can produce, above the 0 a
        // stale client copy carries, so the copy fetched after this start always wins.
        // Idempotent: only rows at 0; `stamp_avatar_version` moves them from here on.
        let _ = c.execute(
            "UPDATE projects SET avatar_version = MAX(created_at, 1) WHERE avatar_version = 0",
            [],
        );
        // One box per user: devices and projects belong to a user; legacy sandboxes record where
        // their /work went.
        let _ = c.execute(
            "ALTER TABLE devices ADD COLUMN user_id TEXT NOT NULL DEFAULT 'u_default'",
            [],
        );
        let _ = c.execute(
            "ALTER TABLE projects ADD COLUMN user_id TEXT NOT NULL DEFAULT 'u_default'",
            [],
        );
        let _ = c.execute(
            "ALTER TABLE projects ADD COLUMN workspace_state TEXT NOT NULL DEFAULT 'stopped'",
            [],
        );
        // Needs user_id and workspace_state above; rebuilds the table once on old databases.
        Self::migrate_project_names_per_user(&c)?;
        let _ = c.execute("ALTER TABLE sandboxes ADD COLUMN migrated_to TEXT", []);
        let _ = c.execute("ALTER TABLE sandboxes ADD COLUMN migrated_at INTEGER", []);
        let _ = c.execute(
            "ALTER TABLE chats ADD COLUMN kind TEXT NOT NULL DEFAULT 'chat'",
            [],
        );
        let _ = c.execute(
            "ALTER TABLE chats ADD COLUMN participants TEXT NOT NULL DEFAULT '[]'",
            [],
        );
        let _ = c.execute("ALTER TABLE chats ADD COLUMN forked_from TEXT", []);
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_traffic (
               id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL, chat_id TEXT NOT NULL,
               trace_id TEXT NOT NULL, at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS group_traffic_project_at ON group_traffic(project_id, at);",
        )?;
        crate::db_accounts::migrate_accounts(&c);
        c.execute_batch(crate::db_accounts::ACCOUNTS_SCHEMA)?;
        crate::db_people::migrate_people(&c);
        c.execute_batch(crate::db_people::PEOPLE_SCHEMA)?;
        Self::migrate_mono_avatar_defaults(&c)?;
        c.execute_batch(crate::db_push::PUSH_SCHEMA)?;
        c.execute_batch(crate::push::store::OUTBOX_SCHEMA)?;
        c.execute_batch(crate::push::eligibility::LEVEL_SCHEMA)?;
        c.execute_batch(crate::read_receipts::READ_RECEIPTS_SCHEMA)?;
        c.execute_batch(crate::db_calls::CALLS_SCHEMA)?;
        crate::db_calls::migrate_calls(&c);
        c.execute_batch(crate::db_voice::VOICE_SCHEMA)?;
        c.execute_batch(crate::db_fleet::FLEET_SCHEMA)?;
        crate::db_voice::migrate_voice(&c);
        Self::migrate_vault_items_per_user(&c)?;
        c.execute_batch(crate::db_secrets::SECRETS_SCHEMA)?;
        c.execute_batch(crate::pins::PINS_SCHEMA)?;
        c.execute_batch(crate::agent_messages::SCHEMA)?;
        crate::db_secrets::migrate_secrets(&c);
        crate::shell_grants::ensure_schema(&c);
        c.execute_batch(crate::matrix::db::MATRIX_SCHEMA)?;
        crate::matrix::db::migrate_matrix(&c);
        c.execute_batch(crate::matrix::servers_db::SERVERS_SCHEMA)?;
        crate::matrix::servers_db::migrate_servers(&c);
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS goals (
               id TEXT PRIMARY KEY, project_id TEXT NOT NULL, chat_id TEXT,
               title TEXT NOT NULL, outcome TEXT NOT NULL DEFAULT '',
               plan TEXT NOT NULL DEFAULT '[]', status TEXT NOT NULL DEFAULT 'active',
               progress_log TEXT NOT NULL DEFAULT '[]', next_step TEXT NOT NULL DEFAULT '',
               check_in_cron TEXT, check_in_tz TEXT NOT NULL DEFAULT 'UTC',
               routine_id TEXT, autonomy TEXT NOT NULL DEFAULT 'ask',
               linked_chat_ids TEXT NOT NULL DEFAULT '[]',
               linked_subagent_ids TEXT NOT NULL DEFAULT '[]',
               waiting_question TEXT,
               created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
               last_moved_at INTEGER
             );
             CREATE INDEX IF NOT EXISTS goals_project ON goals(project_id, status);
             CREATE INDEX IF NOT EXISTS goals_moved ON goals(last_moved_at);",
        )?;
        c.execute_batch(crate::releases::RELEASES_SCHEMA)?;
        Ok(Self(Arc::new(Mutex::new(c))))
    }

    /// Vault items used to belong to the whole relay (`name TEXT NOT NULL UNIQUE`, no owner), so
    /// any user's box could list and request every user's items. Give each row a `user_id`,
    /// make names unique per user instead of relay-wide, and assign the rows that exist to the
    /// user whose projects requested them (else the legacy `u_default`). Runs once: it looks for
    /// SQLite's automatic unique index on `name`.
    fn migrate_vault_items_per_user(c: &Connection) -> Result<()> {
        let _ = c.execute(
            "ALTER TABLE vault_items ADD COLUMN user_id TEXT NOT NULL DEFAULT 'u_default'",
            [],
        );
        let has_global_unique: bool = {
            let mut st = c.prepare("PRAGMA index_list(vault_items)")?;
            let names: Vec<(String, i64)> = st
                .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut found = false;
            for (name, unique) in names {
                if unique == 0 || !name.starts_with("sqlite_autoindex_vault_items") {
                    continue;
                }
                let mut cols = c.prepare(&format!("PRAGMA index_info({name})"))?;
                let col_names: Vec<String> = cols
                    .query_map([], |r| r.get::<_, String>(2))?
                    .collect::<rusqlite::Result<_>>()?;
                if col_names == ["name"] {
                    found = true;
                }
            }
            found
        };
        if has_global_unique {
            tracing::info!("migrating vault_items: one owner per item, names unique per user");
            c.execute_batch(
                "BEGIN;
                 CREATE TABLE vault_items_new (
                   id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL,
                   fields TEXT NOT NULL, allowed_hosts TEXT NOT NULL DEFAULT '[]',
                   created_at INTEGER NOT NULL, last_used_at INTEGER, daytona_secrets TEXT NOT NULL DEFAULT '{}',
                   user_id TEXT NOT NULL DEFAULT 'u_default'
                 );
                 INSERT INTO vault_items_new(id, name, kind, fields, allowed_hosts, created_at, last_used_at, daytona_secrets, user_id)
                   SELECT id, name, kind, fields, allowed_hosts, created_at, last_used_at, daytona_secrets, user_id FROM vault_items;
                 DROP TABLE vault_items;
                 ALTER TABLE vault_items_new RENAME TO vault_items;
                 -- An item that exactly one user's projects ever requested belongs to that user.
                 UPDATE vault_items SET user_id = (
                     SELECT p.user_id FROM vault_requests r JOIN projects p ON p.id = r.project_id
                     WHERE r.item_id = vault_items.id GROUP BY p.user_id
                   )
                   WHERE user_id = 'u_default' AND (
                     SELECT COUNT(DISTINCT p.user_id) FROM vault_requests r JOIN projects p ON p.id = r.project_id
                     WHERE r.item_id = vault_items.id
                   ) = 1;
                 COMMIT;",
            )?;
        }
        c.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS vault_items_user_name ON vault_items(user_id, name)",
            [],
        )?;
        Ok(())
    }

    /// D74: until 2026-10-15 the relay's hash default could land on `mono`, so about one project
    /// in thirteen stored `avatar_color = 'mono'` without anyone choosing it. Clear those once;
    /// NULL means "the (now always coloured) default" on every client. Later explicit `mono`
    /// picks stay: the `meta` flag makes this a one-shot.
    fn migrate_mono_avatar_defaults(c: &Connection) -> Result<()> {
        const KEY: &str = "avatar_mono_defaults_cleared";
        let done: Option<String> = c
            .query_row("SELECT value FROM meta WHERE key = ?1", params![KEY], |r| {
                r.get(0)
            })
            .optional()?;
        if done.is_some() {
            return Ok(());
        }
        c.execute(
            "UPDATE projects SET avatar_color = NULL WHERE avatar_color = 'mono'",
            [],
        )?;
        c.execute(
            "INSERT INTO meta(key, value) VALUES (?1, '1') ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![KEY],
        )?;
        Ok(())
    }

    /// Project names used to be unique across the whole relay (`name TEXT NOT NULL UNIQUE`), so
    /// two users could not both have "Home". Rebuild the table without that constraint and add a
    /// unique index on `(user_id, name)` instead. Runs once: it looks for SQLite's automatic
    /// unique index on `name`.
    fn migrate_project_names_per_user(c: &Connection) -> Result<()> {
        let has_global_unique: bool = {
            let mut st = c.prepare("PRAGMA index_list(projects)")?;
            let names: Vec<(String, i64)> = st
                .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut found = false;
            for (name, unique) in names {
                if unique == 0 || !name.starts_with("sqlite_autoindex_projects") {
                    continue;
                }
                let mut cols = c.prepare(&format!("PRAGMA index_info({name})"))?;
                let col_names: Vec<String> = cols
                    .query_map([], |r| r.get::<_, String>(2))?
                    .collect::<rusqlite::Result<_>>()?;
                if col_names == ["name"] {
                    found = true;
                }
            }
            found
        };
        if has_global_unique {
            tracing::info!("migrating projects: names unique per user instead of relay-wide");
            c.execute_batch(
                "BEGIN;
                 CREATE TABLE projects_new (
                   id TEXT PRIMARY KEY, name TEXT NOT NULL, icon TEXT NOT NULL DEFAULT '',
                   instructions TEXT NOT NULL DEFAULT '', models TEXT NOT NULL DEFAULT '{}',
                   state TEXT NOT NULL DEFAULT 'waking', created_at INTEGER NOT NULL, last_active_at INTEGER NOT NULL,
                   spend_cap_daily REAL NOT NULL DEFAULT 25.0,
                   project_token_hash TEXT,
                   color TEXT NOT NULL DEFAULT '#8b7cf6',
                   user_id TEXT NOT NULL DEFAULT 'u_default',
                   workspace_state TEXT NOT NULL DEFAULT 'stopped',
                   avatar TEXT,
                   deleted_at INTEGER,
                   avatar_color TEXT,
                   description TEXT NOT NULL DEFAULT '',
                   primary_chat_id TEXT,
                   avatar_color_index INTEGER,
                   avatar_leaf INTEGER,
                   avatar_leaf_color INTEGER,
                   image_key TEXT,
                   avatar_version INTEGER NOT NULL DEFAULT 0
                 );
                 INSERT INTO projects_new(id, name, icon, instructions, models, state, created_at, last_active_at, spend_cap_daily,
                                          project_token_hash, color, user_id, workspace_state, avatar, deleted_at, avatar_color, description, primary_chat_id, avatar_color_index, avatar_leaf, avatar_leaf_color, image_key, avatar_version)
                   SELECT id, name, icon, instructions, models, state, created_at, last_active_at, spend_cap_daily,
                          project_token_hash, color, user_id, workspace_state, avatar, deleted_at, avatar_color, description, primary_chat_id, avatar_color_index, avatar_leaf, avatar_leaf_color, image_key, avatar_version FROM projects;
                 DROP TABLE projects;
                 ALTER TABLE projects_new RENAME TO projects;
                 COMMIT;",
            )?;
        }
        c.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS projects_user_name ON projects(user_id, name)",
            [],
        )?;
        Ok(())
    }

    pub(crate) fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        let c = self.0.lock().expect("db lock");
        Ok(f(&c)?)
    }

    // ---- pairing and devices ------------------------------------------------------------------

    pub fn new_pairing_code(&self, code: &str, ttl_ms: i64) -> Result<()> {
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "INSERT INTO pairing_codes(code, created_at, expires_at) VALUES (?1, ?2, ?3)",
                params![code, now, now + ttl_ms],
            )
            .map(|_| ())
        })
    }

    /// Consume a pairing code and register a device. Returns the device id.
    pub fn redeem_pairing_code(
        &self,
        code: &str,
        device_id: &str,
        name: &str,
        token_hash: &str,
    ) -> Result<bool> {
        let now = now_ms();
        self.with(|c| {
            let n = c.execute(
                "UPDATE pairing_codes SET used_at = ?2 WHERE code = ?1 AND used_at IS NULL AND expires_at > ?2",
                params![code, now],
            )?;
            if n == 0 {
                return Ok(false);
            }
            c.execute(
                "INSERT INTO devices(id, name, token_hash, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![device_id, name, token_hash, now],
            )?;
            Ok(true)
        })
    }

    pub fn device_by_token_hash(&self, h: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT id FROM devices WHERE token_hash = ?1 AND revoked_at IS NULL",
                params![h],
                |r| r.get(0),
            )
            .optional()
        })
    }

    pub fn device_count(&self) -> Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM devices WHERE revoked_at IS NULL",
                [],
                |r| r.get(0),
            )
        })
    }

    pub fn revoke_device(&self, id: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE devices SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
                params![id, now_ms()],
            )
            .map(|n| n > 0)
        })
    }

    pub fn list_devices(&self) -> Result<Vec<serde_json::Value>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT id, name, created_at, revoked_at, last_seen_at FROM devices ORDER BY created_at")?;
            let rows = st.query_map([], |r| {
                Ok(serde_json::json!({
                    "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "created_at": r.get::<_, i64>(2)?,
                    "revoked_at": r.get::<_, Option<i64>>(3)?, "last_seen_at": r.get::<_, Option<i64>>(4)?,
                }))
            })?;
            rows.collect()
        })
    }

    // ---- projects and chats -------------------------------------------------------------------

    /// Admin: a project's daily cap in USD, or the unlimited sentinel.
    pub fn set_project_spend_cap(&self, id: &str, cap: f64) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE projects SET spend_cap_daily = ?2 WHERE id = ?1",
                params![id, cap],
            )
            .map(|n| n > 0)
        })
    }

    pub fn insert_project(&self, p: &Project) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO projects(id, name, icon, instructions, models, state, created_at, last_active_at, spend_cap_daily, color, user_id, workspace_state, avatar, avatar_color, description, primary_chat_id, avatar_color_index, avatar_leaf, avatar_leaf_color, image_key, avatar_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
                params![p.id, p.name, p.icon, p.instructions, p.models.to_string(), p.state, p.created_at, p.last_active_at, p.spend_cap_daily, p.color, p.user_id, p.workspace_state, p.avatar, p.avatar_color, p.description, p.primary_chat_id, p.avatar_color_index, p.avatar_leaf, p.avatar_leaf_color, p.image_key,
                        // A new row is stamped at birth (its creation time), never 0.
                        if p.avatar_version > 0 { p.avatar_version } else { p.created_at.max(1) }],
            )
            .map(|_| ())
        })
    }

    fn row_to_project(r: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
        Ok(Project {
            id: r.get(0)?,
            name: r.get(1)?,
            icon: r.get(2)?,
            instructions: r.get(3)?,
            models: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
            state: r.get(5)?,
            created_at: r.get(6)?,
            last_active_at: r.get(7)?,
            spend_cap_daily: r.get(8)?,
            color: r.get(9)?,
            user_id: r.get(10)?,
            workspace_state: r.get(11)?,
            avatar: r.get(12)?,
            avatar_color: r.get(13)?,
            description: r.get::<_, Option<String>>(14)?.unwrap_or_default(),
            primary_chat_id: r.get(15)?,
            avatar_color_index: r.get(16)?,
            avatar_leaf: r.get(17)?,
            avatar_leaf_color: r.get(18)?,
            image_key: r.get(19)?,
            avatar_version: r.get::<_, Option<i64>>(20)?.unwrap_or(0),
        })
    }

    const PROJECT_COLS: &'static str =
        "id, name, icon, instructions, models, state, created_at, last_active_at, spend_cap_daily, color, user_id, workspace_state, avatar, avatar_color, description, primary_chat_id, avatar_color_index, avatar_leaf, avatar_leaf_color, image_key, avatar_version";

    /// By id, else by name. Names are unique per user only, so a name lookup prefers the id
    /// match and otherwise returns the most recently active project with that name; callers that
    /// know the user should use `project_of_user_by_name`.
    pub fn project(&self, id_or_name: &str) -> Result<Option<Project>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM projects WHERE (id = ?1 OR name = ?1) AND deleted_at IS NULL ORDER BY (id = ?1) DESC, last_active_at DESC LIMIT 1",
                    Self::PROJECT_COLS
                ),
                params![id_or_name],
                Self::row_to_project,
            )
            .optional()
        })
    }

    pub fn project_of_user_by_name(&self, user_id: &str, name: &str) -> Result<Option<Project>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM projects WHERE user_id = ?1 AND name = ?2 AND deleted_at IS NULL",
                    Self::PROJECT_COLS
                ),
                params![user_id, name],
                Self::row_to_project,
            )
            .optional()
        })
    }

    /// Soft delete (D55 spirit: nothing is lost for seven days). The project leaves every list
    /// at once; the row, chats, rooms, and blobs go in `purge_deleted_projects`.
    pub fn soft_delete_project(&self, id: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE projects SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, now_ms()],
            )
            .map(|n| n > 0)
        })
    }

    /// `(id, user_id)` of projects soft-deleted before `before_ms`.
    pub fn projects_deleted_before(&self, before_ms: i64) -> Result<Vec<(String, String)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, user_id FROM projects WHERE deleted_at IS NOT NULL AND deleted_at < ?1",
            )?;
            let rows = st.query_map(params![before_ms], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
    }

    /// Remove a project and everything keyed to it (chats, rooms, blob index, pushes, usage,
    /// routines, audit, people-chat agent memberships, project members).
    pub fn purge_project(&self, id: &str) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT id FROM chats WHERE project_id = ?1")?;
            let chats: Vec<String> = st
                .query_map(params![id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut rooms: Vec<String> = chats.iter().map(|ch| format!("{id}/{ch}")).collect();
            rooms.push(format!("{id}/project"));
            for room in &rooms {
                c.execute("DELETE FROM rows WHERE room = ?1", params![room])?;
                c.execute("DELETE FROM checkpoints WHERE room = ?1", params![room])?;
                c.execute("DELETE FROM room_seq WHERE room = ?1", params![room])?;
            }
            for table in [
                "chats",
                "blobs",
                "usage",
                "push_queue",
                "timings",
                "routines",
                "audit",
                "wake_jobs",
                "sandboxes",
            ] {
                let _ = c.execute(
                    &format!("DELETE FROM {table} WHERE project_id = ?1"),
                    params![id],
                );
            }
            let _ = c.execute(
                "DELETE FROM people_members WHERE member = ?1",
                params![format!("agent:{id}")],
            );
            let _ = c.execute(
                "DELETE FROM project_members WHERE project_id = ?1",
                params![id],
            );
            c.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
            Ok(rooms)
        })
    }

    pub fn delete_box_row(&self, user_id: &str) -> Result<bool> {
        self.with(|c| {
            c.execute("DELETE FROM boxes WHERE user_id = ?1", params![user_id])
                .map(|n| n > 0)
        })
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM projects WHERE deleted_at IS NULL ORDER BY last_active_at DESC",
                Self::PROJECT_COLS
            ))?;
            let rows = st.query_map([], Self::row_to_project)?;
            rows.collect()
        })
    }

    /// Partial update of the editable project fields (settings writes from clients or the box).
    #[allow(clippy::too_many_arguments)]
    pub fn update_project(
        &self,
        id: &str,
        name: Option<&str>,
        icon: Option<&str>,
        color: Option<&str>,
        instructions: Option<&str>,
        models: Option<&serde_json::Value>,
        spend_cap_daily: Option<f64>,
        avatar: Option<Option<&str>>,
        avatar_color: Option<Option<&str>>,
        description: Option<&str>,
        avatar_color_index: Option<Option<i64>>,
        avatar_leaf: Option<Option<i64>>,
        avatar_leaf_color: Option<Option<i64>>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE projects SET name = COALESCE(?2, name), icon = COALESCE(?3, icon), color = COALESCE(?4, color),
                        instructions = COALESCE(?5, instructions), models = COALESCE(?6, models), spend_cap_daily = COALESCE(?7, spend_cap_daily),
                        description = COALESCE(?8, description)
                 WHERE id = ?1",
                params![id, name, icon, color, instructions, models.map(|m| m.to_string()), spend_cap_daily, description],
            )?;
            if let Some(a) = avatar {
                c.execute(
                    "UPDATE projects SET avatar = ?2 WHERE id = ?1",
                    params![id, a],
                )?;
            }
            if let Some(col) = avatar_color {
                c.execute(
                    "UPDATE projects SET avatar_color = ?2 WHERE id = ?1",
                    params![id, col],
                )?;
            }
            if let Some(idx) = avatar_color_index {
                c.execute(
                    "UPDATE projects SET avatar_color_index = ?2 WHERE id = ?1",
                    params![id, idx],
                )?;
            }
            if let Some(leaf) = avatar_leaf {
                c.execute(
                    "UPDATE projects SET avatar_leaf = ?2 WHERE id = ?1",
                    params![id, leaf],
                )?;
            }
            if let Some(color) = avatar_leaf_color {
                c.execute(
                    "UPDATE projects SET avatar_leaf_color = ?2 WHERE id = ?1",
                    params![id, color],
                )?;
            }
            let avatar_touched = avatar.is_some()
                || avatar_color.is_some()
                || avatar_color_index.is_some()
                || avatar_leaf.is_some()
                || avatar_leaf_color.is_some();
            if avatar_touched {
                Self::stamp_avatar_version(c, id)?;
            }
            Ok(())
        })
    }

    /// D92 amendment: the agent's uploaded / generated picture (`None` clears it).
    pub fn set_project_image_key(&self, id: &str, key: Option<&str>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE projects SET image_key = ?2 WHERE id = ?1",
                params![id, key],
            )?;
            Self::stamp_avatar_version(c, id)?;
            Ok(())
        })
    }

    /// Every avatar write moves `avatar_version` forward: the clock, or one past the current
    /// stamp when two writes land inside the same millisecond, so copies always order.
    fn stamp_avatar_version(c: &Connection, id: &str) -> rusqlite::Result<()> {
        c.execute(
            "UPDATE projects SET avatar_version = MAX(?2, avatar_version + 1) WHERE id = ?1",
            params![id, now_ms()],
        )?;
        Ok(())
    }

    pub fn set_primary_chat(&self, project_id: &str, chat_id: Option<&str>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE projects SET primary_chat_id = ?2 WHERE id = ?1",
                params![project_id, chat_id],
            )
            .map(|_| ())
        })
    }

    /// Total rows ever appended to a room (`room_seq.head`): the relay's measure of how much a
    /// chat has been used (it never reads the rows themselves).
    pub fn room_head(&self, room: &str) -> Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT head FROM room_seq WHERE room = ?1",
                params![room],
                |r| r.get(0),
            )
            .optional()
            .map(|h| h.unwrap_or(0))
        })
    }

    /// Soft-close a chat (D55: history is never deleted; the tab goes away). Pinned chats refuse.
    pub fn close_chat(&self, project_id: &str, chat_id: &str) -> Result<Option<bool>> {
        self.with(|c| {
            let pinned: Option<i64> = c
                .query_row(
                    "SELECT is_pinned FROM chats WHERE project_id = ?1 AND id = ?2",
                    params![project_id, chat_id],
                    |r| r.get(0),
                )
                .optional()?;
            match pinned {
                None => Ok(None),
                Some(1) => Ok(Some(false)),
                Some(_) => {
                    c.execute(
                        "UPDATE chats SET closed_at = ?3 WHERE project_id = ?1 AND id = ?2",
                        params![project_id, chat_id, now_ms()],
                    )?;
                    Ok(Some(true))
                }
            }
        })
    }

    pub fn update_chat_title(&self, project_id: &str, chat_id: &str, title: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE chats SET title = ?3 WHERE project_id = ?1 AND id = ?2",
                params![project_id, chat_id, title],
            )
            .map(|n| n > 0)
        })
    }

    pub fn set_project_state(&self, id: &str, state: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE projects SET state = ?2, last_active_at = ?3 WHERE id = ?1",
                params![id, state, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn insert_chat(&self, ch: &Chat) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO chats(id, project_id, title, is_pinned, position, created_at, doc_epoch, kind, participants, forked_from) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![ch.id, ch.project_id, ch.title, ch.is_pinned as i64, ch.position, ch.created_at, ch.doc_epoch, ch.kind, serde_json::to_string(&ch.participants).unwrap_or_else(|_| "[]".into()), ch.forked_from],
            )
            .map(|_| ())
        })
    }

    const CHAT_COLS: &'static str =
        "id, project_id, title, is_pinned, position, created_at, doc_epoch, kind, participants, forked_from";

    fn row_to_chat(r: &rusqlite::Row<'_>) -> rusqlite::Result<Chat> {
        Ok(Chat {
            id: r.get(0)?,
            project_id: r.get(1)?,
            title: r.get(2)?,
            is_pinned: r.get::<_, i64>(3)? != 0,
            position: r.get(4)?,
            created_at: r.get(5)?,
            doc_epoch: r.get(6)?,
            kind: r.get(7)?,
            participants: serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or_default(),
            forked_from: r.get(9)?,
        })
    }

    pub fn list_chats(&self, project_id: &str) -> Result<Vec<Chat>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM chats WHERE project_id = ?1 AND closed_at IS NULL ORDER BY position, created_at",
                Self::CHAT_COLS
            ))?;
            let rows = st.query_map(params![project_id], Self::row_to_chat)?;
            rows.collect()
        })
    }

    pub fn chat(&self, project_id: &str, chat_id: &str) -> Result<Option<Chat>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM chats WHERE project_id = ?1 AND id = ?2 AND closed_at IS NULL",
                    Self::CHAT_COLS
                ),
                params![project_id, chat_id],
                Self::row_to_chat,
            )
            .optional()
        })
    }

    /// Group chats (F6): replace the participant list (host first).
    pub fn set_chat_participants(
        &self,
        project_id: &str,
        chat_id: &str,
        participants: &[String],
    ) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE chats SET participants = ?3 WHERE project_id = ?1 AND id = ?2 AND kind = 'group'",
                params![project_id, chat_id, serde_json::to_string(participants).unwrap_or_else(|_| "[]".into())],
            )
            .map(|n| n > 0)
        })
    }

    /// D37 rate guard: group messages a project sent in the last hour, then record this one.
    pub fn group_traffic_record(
        &self,
        project_id: &str,
        chat_id: &str,
        trace_id: &str,
        window_ms: i64,
    ) -> Result<i64> {
        let now = now_ms();
        self.with(|c| {
            let n: i64 = c.query_row(
                "SELECT COUNT(*) FROM group_traffic WHERE project_id = ?1 AND at > ?2",
                params![project_id, now - window_ms],
                |r| r.get(0),
            )?;
            c.execute(
                "INSERT INTO group_traffic(project_id, chat_id, trace_id, at) VALUES (?1,?2,?3,?4)",
                params![project_id, chat_id, trace_id, now],
            )?;
            Ok(n)
        })
    }

    pub fn chat_exists(&self, project_id: &str, chat_id: &str) -> Result<bool> {
        self.with(|c| {
            c.query_row(
                "SELECT 1 FROM chats WHERE project_id = ?1 AND id = ?2",
                params![project_id, chat_id],
                |_| Ok(()),
            )
            .optional()
            .map(|o| o.is_some())
        })
    }

    // ---- boxes (one per user) + legacy per-project sandboxes ----------------------------------

    const BOX_COLS: &'static str = "user_id, provider, sandbox_id, daytona_state, device_id, box_token_hash, mcp_token, snapshot,
                        last_started_at, last_stopped_at, last_seen_at, box_url, preview_token, local_pid, migrated_from";

    fn row_to_box(r: &rusqlite::Row<'_>) -> rusqlite::Result<Sandbox> {
        Ok(Sandbox {
            user_id: r.get(0)?,
            provider: r.get(1)?,
            sandbox_id: r.get(2)?,
            daytona_state: r.get(3)?,
            device_id: r.get(4)?,
            box_token_hash: r.get(5)?,
            mcp_token: r.get(6)?,
            snapshot: r.get(7)?,
            last_started_at: r.get(8)?,
            last_stopped_at: r.get(9)?,
            last_seen_at: r.get(10)?,
            box_url: r.get(11)?,
            preview_token: r.get(12)?,
            local_pid: r.get(13)?,
            migrated_from: serde_json::from_str(
                &r.get::<_, String>(14).unwrap_or_else(|_| "[]".into()),
            )
            .unwrap_or_default(),
        })
    }

    /// Write the user's box row. A row that names a sandbox is never overwritten by one that does
    /// not (#2262: a sandbox-less row over a live box hid it from rolls and always-on); that write
    /// is refused with an error and the row stays as it was. Use
    /// [`Db::upsert_box_clearing_sandbox`] when the sandbox is really gone.
    pub fn upsert_box(&self, s: &Sandbox) -> Result<()> {
        let written = self.write_box(s, false)?;
        if !written {
            tracing::error!(user = %s.user_id, "refused to overwrite a box row that has a sandbox with one that has none");
            anyhow::bail!(
                "box row of {} has a sandbox; refusing to write one without it",
                s.user_id
            );
        }
        Ok(())
    }

    /// [`Db::upsert_box`] that may drop the row's sandbox: only for a sandbox that was deleted.
    pub fn upsert_box_clearing_sandbox(&self, s: &Sandbox) -> Result<()> {
        self.write_box(s, true).map(|_| ())
    }

    /// Write back a row read before a slow provider call (start, stop) only while the row still
    /// names `sandbox_id`. `Ok(false)` when it moved on meanwhile: an upgrade rollback put the
    /// source back (2026-10-25 21:35, a wake that started in the same second as the rollback wrote
    /// the half-filled new sandbox over it and the full source ran on with no row).
    pub fn write_back_box(&self, s: &Sandbox, sandbox_id: Option<&str>) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE boxes SET provider=?2, sandbox_id=?3, daytona_state=?4, device_id=?5, box_token_hash=?6, mcp_token=?7,
                    snapshot=?8, last_started_at=?9, last_stopped_at=?10, last_seen_at=?11, box_url=?12, preview_token=?13,
                    local_pid=?14, migrated_from=?15
                 WHERE user_id = ?1 AND sandbox_id IS ?16",
                params![
                    s.user_id, s.provider, s.sandbox_id, s.daytona_state, s.device_id, s.box_token_hash, s.mcp_token, s.snapshot,
                    s.last_started_at, s.last_stopped_at, s.last_seen_at, s.box_url, s.preview_token, s.local_pid,
                    serde_json::to_string(&s.migrated_from).unwrap_or_else(|_| "[]".into()),
                    sandbox_id
                ],
            )
            .map(|n| n > 0)
        })
    }

    fn write_box(&self, s: &Sandbox, may_clear_sandbox: bool) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "INSERT INTO boxes(user_id, provider, sandbox_id, daytona_state, device_id, box_token_hash, mcp_token, snapshot,
                    last_started_at, last_stopped_at, last_seen_at, box_url, preview_token, local_pid, migrated_from)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                 ON CONFLICT(user_id) DO UPDATE SET provider=excluded.provider, sandbox_id=excluded.sandbox_id,
                    daytona_state=excluded.daytona_state, device_id=excluded.device_id, box_token_hash=excluded.box_token_hash,
                    mcp_token=excluded.mcp_token, snapshot=excluded.snapshot, last_started_at=excluded.last_started_at,
                    last_stopped_at=excluded.last_stopped_at, last_seen_at=excluded.last_seen_at, box_url=excluded.box_url,
                    preview_token=excluded.preview_token, local_pid=excluded.local_pid, migrated_from=excluded.migrated_from
                 WHERE ?16 OR excluded.sandbox_id IS NOT NULL OR boxes.sandbox_id IS NULL",
                params![
                    s.user_id, s.provider, s.sandbox_id, s.daytona_state, s.device_id, s.box_token_hash, s.mcp_token, s.snapshot,
                    s.last_started_at, s.last_stopped_at, s.last_seen_at, s.box_url, s.preview_token, s.local_pid,
                    serde_json::to_string(&s.migrated_from).unwrap_or_else(|_| "[]".into()),
                    may_clear_sandbox
                ],
            )
            .map(|n| n > 0)
        })
    }

    /// Record a box create before the provider is asked for the sandbox (see `box_creates`).
    pub fn begin_box_create(
        &self,
        user_id: &str,
        sandbox_name: &str,
        provider: &str,
        snapshot: Option<&str>,
        replacing: Option<&str>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO box_creates(user_id, sandbox_name, provider, snapshot, replacing, started_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(user_id) DO UPDATE SET sandbox_name = excluded.sandbox_name, provider = excluded.provider,
                    snapshot = excluded.snapshot, replacing = excluded.replacing, started_at = excluded.started_at",
                params![user_id, sandbox_name, provider, snapshot, replacing, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn end_box_create(&self, user_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM box_creates WHERE user_id = ?1",
                params![user_id],
            )
            .map(|_| ())
        })
    }

    /// Box creates recorded before `before_ms` and never finished.
    pub fn unfinished_box_creates(&self, before_ms: i64) -> Result<Vec<BoxCreate>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT user_id, sandbox_name, provider, snapshot, replacing, started_at FROM box_creates
                 WHERE started_at < ?1 ORDER BY started_at",
            )?;
            let rows = st.query_map(params![before_ms], |r| {
                Ok(BoxCreate {
                    user_id: r.get(0)?,
                    sandbox_name: r.get(1)?,
                    provider: r.get(2)?,
                    snapshot: r.get(3)?,
                    replacing: r.get(4)?,
                    started_at: r.get(5)?,
                })
            })?;
            rows.collect()
        })
    }

    /// The user's box, if one was created.
    pub fn user_box(&self, user_id: &str) -> Result<Option<Sandbox>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {} FROM boxes WHERE user_id = ?1", Self::BOX_COLS),
                params![user_id],
                Self::row_to_box,
            )
            .optional()
        })
    }

    pub fn list_boxes(&self) -> Result<Vec<Sandbox>> {
        self.with(|c| {
            let mut st = c.prepare(&format!("SELECT {} FROM boxes", Self::BOX_COLS))?;
            let rows = st.query_map([], Self::row_to_box)?;
            rows.collect()
        })
    }

    /// The box that serves a project = its user's box.
    pub fn box_for_project(&self, project_id: &str) -> Result<Option<Sandbox>> {
        let Some(p) = self.project(project_id)? else {
            return Ok(None);
        };
        self.user_box(&p.user_id)
    }

    pub fn user_by_box_token_hash(&self, h: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT user_id FROM boxes WHERE box_token_hash = ?1",
                params![h],
                |r| r.get(0),
            )
            .optional()
        })
    }

    pub fn touch_box(&self, user_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE boxes SET last_seen_at = ?2 WHERE user_id = ?1",
                params![user_id, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn user_snapshot(&self, user_id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT snapshot FROM users WHERE id = ?1",
                params![user_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .map(|o| o.flatten())
        })
    }

    pub fn set_user_snapshot(&self, user_id: &str, snapshot: Option<&str>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO users(id, snapshot, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET snapshot = excluded.snapshot",
                params![user_id, snapshot, now_ms()],
            )
            .map(|_| ())
        })
    }

    /// Per-user box provider override (`users.box_provider`), `None` = the relay default.
    pub fn user_provider(&self, user_id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT box_provider FROM users WHERE id = ?1",
                params![user_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .map(|o| o.flatten())
        })
    }

    pub fn set_user_provider(&self, user_id: &str, provider: Option<&str>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE users SET box_provider = ?2 WHERE id = ?1",
                params![user_id, provider],
            )
            .map(|_| ())
        })
    }

    pub fn user_spend_cap(&self, user_id: &str) -> Result<f64> {
        self.with(|c| {
            c.query_row(
                "SELECT spend_cap_usd FROM users WHERE id = ?1",
                params![user_id],
                |r| r.get(0),
            )
            .optional()
            .map(|o| o.unwrap_or(25.0))
        })
    }

    /// Today's spend across every project of the user.
    pub fn user_usage_since(&self, user_id: &str, since_ms: i64) -> Result<f64> {
        self.with(|c| {
            c.query_row(
                "SELECT COALESCE(SUM(u.usd), 0) FROM usage u JOIN projects p ON p.id = u.project_id WHERE p.user_id = ?1 AND u.at >= ?2",
                params![user_id, since_ms],
                |r| r.get(0),
            )
        })
    }

    pub fn device_user(&self, device_id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT user_id FROM devices WHERE id = ?1",
                params![device_id],
                |r| r.get(0),
            )
            .optional()
        })
    }

    pub fn projects_of_user(&self, user_id: &str) -> Result<Vec<Project>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM projects WHERE user_id = ?1 AND deleted_at IS NULL ORDER BY last_active_at DESC",
                Self::PROJECT_COLS
            ))?;
            let rows = st.query_map(params![user_id], Self::row_to_project)?;
            rows.collect()
        })
    }

    pub fn set_workspace_state(&self, project_id: &str, state: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE projects SET workspace_state = ?2 WHERE id = ?1",
                params![project_id, state],
            )
            .map(|_| ())
        })
    }

    /// Record that `project_id` should receive a copy of `source_project_id`'s workspace (fork).
    pub fn set_project_fork(&self, project_id: &str, source_project_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO project_forks(project_id, source_project_id, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(project_id) DO UPDATE SET source_project_id = excluded.source_project_id",
                params![project_id, source_project_id, now_ms()],
            )
            .map(|_| ())
        })
    }

    /// Source project id for a pending/recorded fork, if any.
    pub fn project_fork_source(&self, project_id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT source_project_id FROM project_forks WHERE project_id = ?1",
                params![project_id],
                |r| r.get(0),
            )
            .optional()
        })
    }

    /// Legacy per-project sandboxes (pre one-box-per-user) that still hold a project's /work.
    pub fn legacy_sandboxes(&self) -> Result<Vec<(String, Sandbox, Option<String>)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT project_id, provider, sandbox_id, daytona_state, device_id, box_token_hash, mcp_token, snapshot,
                        last_started_at, last_stopped_at, last_seen_at, box_url, preview_token, local_pid, migrated_to
                 FROM sandboxes",
            )?;
            let rows = st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    Sandbox {
                        user_id: r.get(0)?,
                        provider: r.get(1)?,
                        sandbox_id: r.get(2)?,
                        daytona_state: r.get(3)?,
                        device_id: r.get(4)?,
                        box_token_hash: r.get(5)?,
                        mcp_token: r.get(6)?,
                        snapshot: r.get(7)?,
                        last_started_at: r.get(8)?,
                        last_stopped_at: r.get(9)?,
                        last_seen_at: r.get(10)?,
                        box_url: r.get(11)?,
                        preview_token: r.get(12)?,
                        local_pid: r.get(13)?,
                        migrated_from: Vec::new(),
                    },
                    r.get::<_, Option<String>>(14)?,
                ))
            })?;
            rows.collect()
        })
    }

    pub fn mark_legacy_migrated(&self, project_id: &str, user_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE sandboxes SET migrated_to = ?2, migrated_at = ?3 WHERE project_id = ?1",
                params![project_id, user_id, now_ms()],
            )
            .map(|_| ())
        })
    }

    // ---- rooms --------------------------------------------------------------------------------

    pub fn room_state(&self, room: &str) -> Result<RoomState> {
        self.with(|c| {
            let (head, floor): (i64, i64) = c
                .query_row(
                    "SELECT head, floor FROM room_seq WHERE room = ?1",
                    params![room],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
                .unwrap_or((0, 0));
            let cp: Option<(i64, i64, Vec<u8>)> = c
                .query_row(
                    "SELECT seq, size, frontier FROM checkpoints WHERE room = ?1",
                    params![room],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let (row_count, row_bytes): (i64, i64) = c.query_row(
                "SELECT COUNT(*), COALESCE(SUM(LENGTH(bytes)),0) FROM rows WHERE room = ?1",
                params![room],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let (checkpoint_seq, checkpoint_size, checkpoint_frontier) =
                cp.unwrap_or((0, 0, Vec::new()));
            Ok(RoomState {
                head,
                floor,
                checkpoint_seq,
                checkpoint_size,
                checkpoint_frontier,
                row_count,
                row_bytes,
            })
        })
    }

    /// Seq of a stored batch id, if any — used to ACK a resent push as `dup` before the people
    /// gates (so a reconnect storm does not burn the message rate limit).
    pub fn row_seq_for_batch(&self, room: &str, batch_id: &str) -> Result<Option<i64>> {
        self.with(|c| {
            c.query_row(
                "SELECT seq FROM rows WHERE room = ?1 AND batch_id = ?2",
                params![room, batch_id],
                |r| r.get(0),
            )
            .optional()
        })
    }

    /// Append a row. Returns `(seq, dup)`; a duplicate batch id returns the existing seq (idempotent push).
    pub fn append_row(
        &self,
        room: &str,
        device: &str,
        batch_id: &str,
        bytes: &[u8],
    ) -> Result<(i64, bool)> {
        let now = now_ms();
        self.with(|c| {
            let existing: Option<i64> = c
                .query_row("SELECT seq FROM rows WHERE room = ?1 AND batch_id = ?2", params![room, batch_id], |r| r.get(0))
                .optional()?;
            if let Some(seq) = existing {
                return Ok((seq, true));
            }
            c.execute("INSERT INTO room_seq(room, head, floor) VALUES (?1, 0, 0) ON CONFLICT(room) DO NOTHING", params![room])?;
            c.execute("UPDATE room_seq SET head = head + 1 WHERE room = ?1", params![room])?;
            let seq: i64 = c.query_row("SELECT head FROM room_seq WHERE room = ?1", params![room], |r| r.get(0))?;
            c.execute(
                "INSERT INTO rows(room, seq, device, batch_id, bytes, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
                params![room, seq, device, batch_id, bytes, now],
            )?;
            Ok((seq, false))
        })
    }

    pub fn rows_after(
        &self,
        room: &str,
        after: i64,
        exclude_device: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Row>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT seq, device, batch_id, bytes FROM rows WHERE room = ?1 AND seq > ?2 AND (?3 IS NULL OR device != ?3)
                 ORDER BY seq LIMIT ?4",
            )?;
            let rows = st.query_map(params![room, after, exclude_device, limit], |r| {
                Ok(Row { seq: r.get(0)?, device: r.get(1)?, batch_id: r.get(2)?, bytes: r.get(3)? })
            })?;
            rows.collect()
        })
    }

    /// Rows with `after < seq <= upto`, oldest first (the checkpoint validation walk).
    pub fn rows_between(&self, room: &str, after: i64, upto: i64, limit: i64) -> Result<Vec<Row>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT seq, device, batch_id, bytes FROM rows WHERE room = ?1 AND seq > ?2 AND seq <= ?3
                 ORDER BY seq LIMIT ?4",
            )?;
            let rows = st.query_map(params![room, after, upto, limit], |r| {
                Ok(Row { seq: r.get(0)?, device: r.get(1)?, batch_id: r.get(2)?, bytes: r.get(3)? })
            })?;
            rows.collect()
        })
    }

    /// After rows were replaced for a redaction: fold the WAL into the main file and truncate
    /// it, so the pages that held the old bytes are gone from disk as well.
    pub fn flush_after_scrub(&self) -> Result<()> {
        self.with(|c| {
            c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            Ok(())
        })
    }

    /// Store a checkpoint and delete rows it covers (guard: `seq_covered >= floor`).
    ///
    /// Callers must have verified that the snapshot contains every op in those rows
    /// (`rooms::checkpoint_covers_store`): this deletes them for good.
    pub fn put_checkpoint(
        &self,
        room: &str,
        seq_covered: i64,
        frontier: &[u8],
        bytes: &[u8],
    ) -> Result<bool> {
        self.with(|c| {
            let floor: i64 = c
                .query_row("SELECT floor FROM room_seq WHERE room = ?1", params![room], |r| r.get(0))
                .optional()?
                .unwrap_or(0);
            if seq_covered < floor {
                return Ok(false);
            }
            c.execute(
                "INSERT INTO checkpoints(room, seq, frontier, bytes, size, created_at) VALUES (?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(room) DO UPDATE SET seq=excluded.seq, frontier=excluded.frontier, bytes=excluded.bytes,
                 size=excluded.size, created_at=excluded.created_at",
                params![room, seq_covered, frontier, bytes, bytes.len() as i64, now_ms()],
            )?;
            c.execute("DELETE FROM rows WHERE room = ?1 AND seq <= ?2", params![room, seq_covered])?;
            c.execute("INSERT INTO room_seq(room, head, floor) VALUES (?1, ?2, ?2) ON CONFLICT(room) DO UPDATE SET floor = ?2, head = MAX(head, ?2)", params![room, seq_covered])?;
            Ok(true)
        })
    }

    /// `(seq, created_at, bytes)` of the room's checkpoint.
    pub fn checkpoint_full(&self, room: &str) -> Result<Option<(i64, i64, Vec<u8>)>> {
        self.with(|c| {
            c.query_row(
                "SELECT seq, created_at, bytes FROM checkpoints WHERE room = ?1",
                params![room],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
        })
    }

    /// Announced messages the relay indexed for a chat (`chat_events`).
    pub fn chat_events_count(&self, chat_id: &str) -> Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM chat_events WHERE chat_id = ?1",
                params![chat_id],
                |r| r.get(0),
            )
        })
    }

    /// Messages the Matrix mirror holds for a chat (`matrix_events`).
    pub fn matrix_events_count(&self, chat_id: &str) -> Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM matrix_events WHERE chat_id = ?1",
                params![chat_id],
                |r| r.get(0),
            )
        })
    }

    pub fn checkpoint_bytes(&self, room: &str) -> Result<Option<(i64, Vec<u8>)>> {
        self.with(|c| {
            c.query_row(
                "SELECT seq, bytes FROM checkpoints WHERE room = ?1",
                params![room],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
    }

    // ---- blobs --------------------------------------------------------------------------------

    pub fn record_blob(
        &self,
        project_id: &str,
        key: &str,
        size: i64,
        mime: Option<&str>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO blobs(project_id, key, size, mime, created_at) VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(project_id, key) DO UPDATE SET size=excluded.size, mime=excluded.mime",
                params![project_id, key, size, mime, now_ms()],
            )
            .map(|_| ())
        })
    }

    // ---- usage / spend ------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn record_usage(
        &self,
        project_id: &str,
        api: &str,
        model: &str,
        input: i64,
        output: i64,
        cache_read: i64,
        cache_write: i64,
        usd: f64,
        latency_ms: i64,
        ttfb_ms: Option<i64>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO usage(project_id, at, api, model, input, output, cache_read, cache_write, usd, latency_ms, ttfb_ms)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![project_id, now_ms(), api, model, input, output, cache_read, cache_write, usd, latency_ms, ttfb_ms],
            )
            .map(|_| ())
        })
    }

    /// How many usage rows a project wrote since `since_ms` for one API whose model starts with
    /// `model_prefix` (the avatar picker's daily generation count).
    pub fn usage_count_since(
        &self,
        project_id: &str,
        api: &str,
        model_prefix: &str,
        since_ms: i64,
    ) -> Result<i64> {
        self.with(|c| {
            let n = c.query_row(
                "SELECT COUNT(*) FROM usage WHERE project_id = ?1 AND api = ?2 AND at >= ?3
                        AND (model LIKE ?4 OR model LIKE ?5)",
                params![
                    project_id,
                    api,
                    since_ms,
                    format!("{model_prefix}%"),
                    format!("mock:{model_prefix}%")
                ],
                |r| r.get(0),
            )?;
            Ok(n)
        })
    }

    pub fn usage_since(&self, project_id: &str, since_ms: i64) -> Result<UsageTotals> {
        self.with(|c| {
            c.query_row(
                "SELECT COALESCE(SUM(input),0), COALESCE(SUM(output),0), COALESCE(SUM(cache_read),0), COALESCE(SUM(cache_write),0),
                        COALESCE(SUM(usd),0.0), COUNT(*) FROM usage WHERE project_id = ?1 AND at >= ?2",
                params![project_id, since_ms],
                |r| {
                    Ok(UsageTotals {
                        input: r.get(0)?,
                        output: r.get(1)?,
                        cache_read: r.get(2)?,
                        cache_write: r.get(3)?,
                        usd: r.get(4)?,
                        calls: r.get(5)?,
                    })
                },
            )
        })
    }

    /// Spend of one tagged purpose (`usage.api LIKE '%:<purpose>'`) in a time window, split by
    /// whether the project's owner is a test user: `(who, api, calls, usd)` — the audit query the
    /// feedback poller used to run against the SQLite file as root.
    pub fn usage_by_purpose(
        &self,
        purpose: &str,
        since_ms: i64,
        until_ms: i64,
    ) -> Result<Vec<(String, String, i64, f64)>> {
        let like = format!("%:{purpose}");
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT CASE WHEN us.test = 1 THEN 'test' ELSE 'real' END AS who, u.api, COUNT(*), COALESCE(SUM(u.usd), 0.0)
                 FROM usage u JOIN projects p ON p.id = u.project_id LEFT JOIN users us ON us.id = p.user_id
                 WHERE u.api LIKE ?1 AND u.at >= ?2 AND u.at < ?3
                 GROUP BY who, u.api ORDER BY who, u.api",
            )?;
            let rows = st.query_map(params![like, since_ms, until_ms], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
            rows.collect()
        })
    }

    // ---- wake jobs, push queue, timings -------------------------------------------------------

    pub fn upsert_wake_job(&self, user_id: &str, reason: &str) -> Result<()> {
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "INSERT INTO box_wake_jobs(user_id, reason, due_at, attempts, created_at) VALUES (?1,?2,?3,0,?3)
                 ON CONFLICT(user_id) DO UPDATE SET reason = excluded.reason, due_at = MIN(box_wake_jobs.due_at, excluded.due_at)",
                params![user_id, reason, now],
            )
            .map(|_| ())
        })
    }

    pub fn due_wake_jobs(&self) -> Result<Vec<(String, String, i64)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT user_id, reason, attempts FROM box_wake_jobs WHERE due_at <= ?1",
            )?;
            let rows =
                st.query_map(params![now_ms()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect()
        })
    }

    pub fn finish_wake_job(&self, user_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM box_wake_jobs WHERE user_id = ?1",
                params![user_id],
            )
            .map(|_| ())
        })
    }

    pub fn retry_wake_job(&self, user_id: &str, delay_ms: i64, err: &str) -> Result<i64> {
        self.with(|c| {
            c.execute(
                "UPDATE box_wake_jobs SET attempts = attempts + 1, due_at = ?2, last_error = ?3 WHERE user_id = ?1",
                params![user_id, now_ms() + delay_ms, err],
            )?;
            c.query_row("SELECT attempts FROM box_wake_jobs WHERE user_id = ?1", params![user_id], |r| r.get(0))
        })
    }

    pub fn enqueue_push(
        &self,
        project_id: &str,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO push_queue(project_id, kind, payload, created_at) VALUES (?1,?2,?3,?4)",
                params![project_id, kind, payload.to_string(), now_ms()],
            )
            .map(|_| ())
        })
    }

    /// [`Self::enqueue_push`] unless an identical push (kind and payload) is still undelivered:
    /// a box told once to open a room need not be told again per row. Returns whether a row
    /// was added.
    pub fn enqueue_push_once(
        &self,
        project_id: &str,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Result<bool> {
        let payload = payload.to_string();
        self.with(|c| {
            let n = c.execute(
                "INSERT INTO push_queue(project_id, kind, payload, created_at)
                 SELECT ?1, ?2, ?3, ?4
                 WHERE NOT EXISTS (SELECT 1 FROM push_queue WHERE project_id = ?1 AND kind = ?2 AND payload = ?3 AND delivered_at IS NULL)",
                params![project_id, kind, payload, now_ms()],
            )?;
            Ok(n > 0)
        })
    }

    pub fn take_pushes(&self, project_id: &str) -> Result<Vec<(i64, String, serde_json::Value)>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT id, kind, payload FROM push_queue WHERE project_id = ?1 AND delivered_at IS NULL ORDER BY id")?;
            let items: Vec<(i64, String, serde_json::Value)> = st
                .query_map(params![project_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default()))
                })?
                .collect::<rusqlite::Result<_>>()?;
            for (id, _, _) in &items {
                c.execute("UPDATE push_queue SET delivered_at = ?2 WHERE id = ?1", params![id, now_ms()])?;
            }
            Ok(items)
        })
    }

    /// Remember what a box's agentd reported in its hello (commit, version, image).
    pub fn set_box_agentd(
        &self,
        user_id: &str,
        sha: Option<&str>,
        version: Option<&str>,
        image: Option<&str>,
        seed_sha: Option<&str>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO box_agentd(user_id, sha, version, image, seed_sha, reported_at) VALUES (?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(user_id) DO UPDATE SET sha=excluded.sha, version=excluded.version, image=excluded.image,
                    seed_sha=excluded.seed_sha, reported_at=excluded.reported_at",
                params![user_id, sha, version, image, seed_sha, now_ms()],
            )
            .map(|_| ())
        })
    }

    /// `(sha, version, image, reported_at)` of one user's box, when it ever said hello.
    pub fn box_agentd(&self, user_id: &str) -> Result<Option<BoxAgentd>> {
        self.with(|c| {
            c.query_row(
                "SELECT user_id, sha, version, image, seed_sha, reported_at FROM box_agentd WHERE user_id = ?1",
                params![user_id],
                |r| Ok(BoxAgentd { user_id: r.get(0)?, sha: r.get(1)?, version: r.get(2)?, image: r.get(3)?, seed_sha: r.get(4)?, reported_at: r.get(5)? }),
            )
            .optional()
        })
    }

    pub fn all_box_agentd(&self) -> Result<Vec<BoxAgentd>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT user_id, sha, version, image, seed_sha, reported_at FROM box_agentd ORDER BY reported_at")?;
            let rows = st.query_map([], |r| Ok(BoxAgentd { user_id: r.get(0)?, sha: r.get(1)?, version: r.get(2)?, image: r.get(3)?, seed_sha: r.get(4)?, reported_at: r.get(5)? }))?;
            rows.collect()
        })
    }

    pub fn record_timing(
        &self,
        project_id: &str,
        kind: &str,
        ms: i64,
        detail: Option<&str>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO timings(project_id, kind, ms, detail, at) VALUES (?1,?2,?3,?4,?5)",
                params![project_id, kind, ms, detail, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn timings(&self, project_id: &str) -> Result<Vec<serde_json::Value>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT kind, ms, detail, at FROM timings WHERE project_id = ?1 ORDER BY at")?;
            let rows = st.query_map(params![project_id], |r| {
                Ok(serde_json::json!({"kind": r.get::<_, String>(0)?, "ms": r.get::<_, i64>(1)?, "detail": r.get::<_, Option<String>>(2)?, "at": r.get::<_, i64>(3)?}))
            })?;
            rows.collect()
        })
    }

    // ---- meta ---------------------------------------------------------------------------------

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()
        })
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(|_| ())
        })
    }

    // ---- connectors (F1) ----------------------------------------------------------------------

    const CONNECTOR_COLS: &'static str =
        "id, provider, label, scopes, status, config, created_at, last_used_at, last_error";

    fn row_to_connector(r: &rusqlite::Row<'_>) -> rusqlite::Result<Connector> {
        Ok(Connector {
            id: r.get(0)?,
            provider: r.get(1)?,
            label: r.get(2)?,
            scopes: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
            status: r.get(4)?,
            config: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
            created_at: r.get(6)?,
            last_used_at: r.get(7)?,
            last_error: r.get(8)?,
        })
    }

    pub fn upsert_connector(&self, cn: &Connector) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO connectors(id, provider, label, scopes, status, config, created_at, last_used_at, last_error)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(id) DO UPDATE SET provider=excluded.provider, label=excluded.label, scopes=excluded.scopes,
                    status=excluded.status, config=excluded.config, last_used_at=excluded.last_used_at, last_error=excluded.last_error",
                params![
                    cn.id, cn.provider, cn.label, serde_json::to_string(&cn.scopes).unwrap_or_else(|_| "[]".into()),
                    cn.status, cn.config.to_string(), cn.created_at, cn.last_used_at, cn.last_error
                ],
            )
            .map(|_| ())
        })
    }

    pub fn list_connectors(&self) -> Result<Vec<Connector>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM connectors ORDER BY created_at",
                Self::CONNECTOR_COLS
            ))?;
            let rows = st.query_map([], Self::row_to_connector)?;
            rows.collect()
        })
    }

    pub fn connector(&self, id: &str) -> Result<Option<Connector>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM connectors WHERE id = ?1",
                    Self::CONNECTOR_COLS
                ),
                params![id],
                Self::row_to_connector,
            )
            .optional()
        })
    }

    /// The newest connector of a provider (one account per provider on a single-user relay).
    pub fn connector_by_provider(&self, provider: &str) -> Result<Option<Connector>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM connectors WHERE provider = ?1 ORDER BY created_at DESC LIMIT 1",
                    Self::CONNECTOR_COLS
                ),
                params![provider],
                Self::row_to_connector,
            )
            .optional()
        })
    }

    pub fn set_connector_status(&self, id: &str, status: &str, error: Option<&str>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE connectors SET status = ?2, last_error = ?3 WHERE id = ?1",
                params![id, status, error],
            )
            .map(|_| ())
        })
    }

    pub fn touch_connector(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE connectors SET last_used_at = ?2 WHERE id = ?1",
                params![id, now_ms()],
            )
            .map(|_| ())
        })
    }

    /// Delete a connector and its tokens. Returns whether a row existed.
    pub fn delete_connector(&self, id: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "DELETE FROM connector_tokens WHERE connector_id = ?1",
                params![id],
            )?;
            c.execute("DELETE FROM connectors WHERE id = ?1", params![id])
                .map(|n| n > 0)
        })
    }

    pub fn put_connector_token(&self, t: &ConnectorTokenRow) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO connector_tokens(connector_id, access_token, refresh_token, expires_at, token_type, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(connector_id) DO UPDATE SET access_token=excluded.access_token, refresh_token=excluded.refresh_token,
                    expires_at=excluded.expires_at, token_type=excluded.token_type, updated_at=excluded.updated_at",
                params![t.connector_id, t.access_token, t.refresh_token, t.expires_at, t.token_type, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn connector_token(&self, connector_id: &str) -> Result<Option<ConnectorTokenRow>> {
        self.with(|c| {
            c.query_row(
                "SELECT connector_id, access_token, refresh_token, expires_at, token_type FROM connector_tokens WHERE connector_id = ?1",
                params![connector_id],
                |r| {
                    Ok(ConnectorTokenRow {
                        connector_id: r.get(0)?,
                        access_token: r.get(1)?,
                        refresh_token: r.get(2)?,
                        expires_at: r.get(3)?,
                        token_type: r.get(4)?,
                    })
                },
            )
            .optional()
        })
    }

    pub fn put_oauth_state(&self, s: &OauthState, ttl_ms: i64) -> Result<()> {
        let now = now_ms();
        self.with(|c| {
            c.execute("DELETE FROM oauth_states WHERE expires_at < ?1", params![now])?;
            c.execute(
                "INSERT INTO oauth_states(state, provider, code_verifier, redirect_uri, scopes, created_at, expires_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    s.state, s.provider, s.code_verifier, s.redirect_uri,
                    serde_json::to_string(&s.scopes).unwrap_or_else(|_| "[]".into()), now, now + ttl_ms
                ],
            )
            .map(|_| ())
        })
    }

    /// Consume an OAuth state (one use). `None` when unknown or expired.
    pub fn take_oauth_state(&self, state: &str) -> Result<Option<OauthState>> {
        self.with(|c| {
            let row = c
                .query_row(
                    "SELECT state, provider, code_verifier, redirect_uri, scopes FROM oauth_states WHERE state = ?1 AND expires_at >= ?2",
                    params![state, now_ms()],
                    |r| {
                        Ok(OauthState {
                            state: r.get(0)?,
                            provider: r.get(1)?,
                            code_verifier: r.get(2)?,
                            redirect_uri: r.get(3)?,
                            scopes: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
                        })
                    },
                )
                .optional()?;
            c.execute("DELETE FROM oauth_states WHERE state = ?1", params![state])?;
            Ok(row)
        })
    }

    pub fn record_connector_outbox(
        &self,
        provider: &str,
        action: &str,
        args: &serde_json::Value,
    ) -> Result<i64> {
        self.with(|c| {
            c.execute(
                "INSERT INTO connector_outbox(provider, action, args, created_at) VALUES (?1,?2,?3,?4)",
                params![provider, action, args.to_string(), now_ms()],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn connector_outbox(&self, limit: i64) -> Result<Vec<serde_json::Value>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT id, provider, action, args, created_at FROM connector_outbox ORDER BY id DESC LIMIT ?1")?;
            let rows = st.query_map(params![limit], |r| {
                Ok(serde_json::json!({
                    "id": r.get::<_, i64>(0)?, "provider": r.get::<_, String>(1)?, "action": r.get::<_, String>(2)?,
                    "args": serde_json::from_str::<serde_json::Value>(&r.get::<_, String>(3)?).unwrap_or_default(),
                    "created_at": r.get::<_, i64>(4)?,
                }))
            })?;
            rows.collect()
        })
    }

    // ---- channels (F7) ------------------------------------------------------------------------

    const BINDING_COLS: &'static str =
        "id, platform, external_chat_id, external_user_id, project_id, chat_id, label, bound_at";

    fn row_to_binding(r: &rusqlite::Row<'_>) -> rusqlite::Result<ChannelBinding> {
        Ok(ChannelBinding {
            id: r.get(0)?,
            platform: r.get(1)?,
            external_chat_id: r.get(2)?,
            external_user_id: r.get(3)?,
            project_id: r.get(4)?,
            chat_id: r.get(5)?,
            label: r.get(6)?,
            bound_at: r.get(7)?,
        })
    }

    pub fn new_channel_pair_code(&self, c: &ChannelPairCode) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "DELETE FROM channel_pair_codes WHERE expires_at < ?1",
                params![now_ms()],
            )?;
            conn.execute(
                "INSERT INTO channel_pair_codes(code, project_id, chat_id, created_at, expires_at) VALUES (?1,?2,?3,?4,?5)",
                params![c.code, c.project_id, c.chat_id, c.created_at, c.expires_at],
            )
            .map(|_| ())
        })
    }

    /// The newest unused, unexpired code (so a client can show it again after a restart).
    pub fn active_channel_pair_code(&self) -> Result<Option<ChannelPairCode>> {
        self.with(|conn| {
            conn.query_row(
                "SELECT code, project_id, chat_id, created_at, expires_at FROM channel_pair_codes
                 WHERE used_at IS NULL AND expires_at > ?1 ORDER BY created_at DESC LIMIT 1",
                params![now_ms()],
                |r| {
                    Ok(ChannelPairCode {
                        code: r.get(0)?,
                        project_id: r.get(1)?,
                        chat_id: r.get(2)?,
                        created_at: r.get(3)?,
                        expires_at: r.get(4)?,
                    })
                },
            )
            .optional()
        })
    }

    /// Consume a pairing code (one use). `None` when unknown, used, or expired.
    pub fn redeem_channel_pair_code(&self, code: &str) -> Result<Option<ChannelPairCode>> {
        let now = now_ms();
        self.with(|conn| {
            let row = conn
                .query_row(
                    "SELECT code, project_id, chat_id, created_at, expires_at FROM channel_pair_codes
                     WHERE code = ?1 AND used_at IS NULL AND expires_at > ?2",
                    params![code, now],
                    |r| {
                        Ok(ChannelPairCode {
                            code: r.get(0)?,
                            project_id: r.get(1)?,
                            chat_id: r.get(2)?,
                            created_at: r.get(3)?,
                            expires_at: r.get(4)?,
                        })
                    },
                )
                .optional()?;
            if row.is_some() {
                conn.execute(
                    "UPDATE channel_pair_codes SET used_at = ?2 WHERE code = ?1",
                    params![code, now],
                )?;
            }
            Ok(row)
        })
    }

    /// Bind (or re-bind) one external user + chat to a project chat.
    pub fn upsert_channel_binding(&self, b: &ChannelBinding) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO channel_bindings(id, platform, external_chat_id, external_user_id, project_id, chat_id, label, bound_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(platform, external_chat_id, external_user_id) DO UPDATE SET
                    project_id=excluded.project_id, chat_id=excluded.chat_id, label=excluded.label, bound_at=excluded.bound_at",
                params![b.id, b.platform, b.external_chat_id, b.external_user_id, b.project_id, b.chat_id, b.label, b.bound_at],
            )
            .map(|_| ())
        })
    }

    pub fn channel_binding_for(
        &self,
        platform: &str,
        external_chat_id: &str,
        external_user_id: &str,
    ) -> Result<Option<ChannelBinding>> {
        self.with(|conn| {
            conn.query_row(
                &format!(
                    "SELECT {} FROM channel_bindings WHERE platform = ?1 AND external_chat_id = ?2 AND external_user_id = ?3",
                    Self::BINDING_COLS
                ),
                params![platform, external_chat_id, external_user_id],
                Self::row_to_binding,
            )
            .optional()
        })
    }

    pub fn channel_binding(&self, id: &str) -> Result<Option<ChannelBinding>> {
        self.with(|conn| {
            conn.query_row(
                &format!(
                    "SELECT {} FROM channel_bindings WHERE id = ?1",
                    Self::BINDING_COLS
                ),
                params![id],
                Self::row_to_binding,
            )
            .optional()
        })
    }

    pub fn list_channel_bindings(&self) -> Result<Vec<ChannelBinding>> {
        self.with(|conn| {
            let mut st = conn.prepare(&format!(
                "SELECT {} FROM channel_bindings ORDER BY bound_at DESC",
                Self::BINDING_COLS
            ))?;
            let rows = st.query_map([], Self::row_to_binding)?;
            rows.collect()
        })
    }

    pub fn delete_channel_binding(&self, id: &str) -> Result<bool> {
        self.with(|conn| {
            conn.execute("DELETE FROM channel_bindings WHERE id = ?1", params![id])
                .map(|n| n > 0)
        })
    }

    /// Record that an unbound sender got the pairing hint. Returns `true` the first time only.
    pub fn mark_channel_hinted(
        &self,
        platform: &str,
        external_chat_id: &str,
        external_user_id: &str,
    ) -> Result<bool> {
        self.with(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO channel_hints(platform, external_chat_id, external_user_id, hinted_at) VALUES (?1,?2,?3,?4)",
                params![platform, external_chat_id, external_user_id, now_ms()],
            )
            .map(|n| n > 0)
        })
    }

    /// Idempotency for webhook retries: `true` when `event_key` is new.
    pub fn channel_event_first_seen(&self, event_key: &str) -> Result<bool> {
        self.with(|conn| {
            conn.execute(
                "DELETE FROM channel_seen WHERE seen_at < ?1",
                params![now_ms() - 7 * 86_400_000],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO channel_seen(event_key, seen_at) VALUES (?1, ?2)",
                params![event_key, now_ms()],
            )
            .map(|n| n > 0)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_channel_outbox(
        &self,
        platform: &str,
        external_chat_id: &str,
        text: &str,
        request_id: &str,
        status: &str,
        detail: Option<&str>,
        binding_id: Option<&str>,
    ) -> Result<i64> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO channel_outbox(platform, external_chat_id, text, request_id, status, detail, binding_id, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![platform, external_chat_id, text, request_id, status, detail, binding_id, now_ms()],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn set_channel_outbox_status(
        &self,
        id: i64,
        status: &str,
        detail: Option<&str>,
    ) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "UPDATE channel_outbox SET status = ?2, detail = ?3 WHERE id = ?1",
                params![id, status, detail],
            )
            .map(|_| ())
        })
    }

    pub fn channel_outbox(&self, limit: i64) -> Result<Vec<serde_json::Value>> {
        self.with(|conn| {
            let mut st = conn.prepare(
                "SELECT id, platform, external_chat_id, text, request_id, status, detail, binding_id, created_at
                 FROM channel_outbox ORDER BY id DESC LIMIT ?1",
            )?;
            let rows = st.query_map(params![limit], |r| {
                Ok(serde_json::json!({
                    "id": r.get::<_, i64>(0)?, "platform": r.get::<_, String>(1)?, "externalChatId": r.get::<_, String>(2)?,
                    "text": r.get::<_, String>(3)?, "requestId": r.get::<_, String>(4)?, "status": r.get::<_, String>(5)?,
                    "detail": r.get::<_, Option<String>>(6)?, "bindingId": r.get::<_, Option<String>>(7)?,
                    "created_at": r.get::<_, i64>(8)?,
                }))
            })?;
            rows.collect()
        })
    }

    // ---- local helpers (F9) -------------------------------------------------------------------

    const LOCAL_COLS: &'static str =
        "device_id, name, hostname, os, home, connected, last_seen, first_seen, allow_reads";

    fn row_to_local(r: &rusqlite::Row<'_>) -> rusqlite::Result<LocalDevice> {
        Ok(LocalDevice {
            device_id: r.get(0)?,
            name: r.get(1)?,
            hostname: r.get(2)?,
            os: r.get(3)?,
            home: r.get(4)?,
            connected: r.get::<_, i64>(5)? != 0,
            last_seen: r.get(6)?,
            first_seen: r.get(7)?,
            allow_reads: r.get::<_, i64>(8)? != 0,
        })
    }

    /// A helper said hello: create or refresh its row and mark it connected. The display name
    /// is the one chosen at pairing (`devices.name`) unless that is the generic default.
    pub fn local_device_hello(
        &self,
        device_id: &str,
        hostname: &str,
        os: &str,
        home: &str,
    ) -> Result<()> {
        let now = now_ms();
        self.with(|c| {
            let paired_name: Option<String> = c
                .query_row(
                    "SELECT name FROM devices WHERE id = ?1",
                    params![device_id],
                    |r| r.get(0),
                )
                .optional()?;
            let name = paired_name
                .filter(|n| !n.trim().is_empty() && n != "device")
                .unwrap_or_else(|| hostname.to_owned());
            c.execute(
                "INSERT INTO local_devices(device_id, name, hostname, os, home, connected, last_seen, first_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)
                 ON CONFLICT(device_id) DO UPDATE SET name = excluded.name, hostname = excluded.hostname,
                    os = excluded.os, home = excluded.home, connected = 1, last_seen = excluded.last_seen",
                params![device_id, name, hostname, os, home, now],
            )
            .map(|_| ())
        })
    }

    pub fn local_device_seen(&self, device_id: &str, connected: bool) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE local_devices SET connected = ?2, last_seen = ?3 WHERE device_id = ?1",
                params![device_id, connected as i64, now_ms()],
            )
            .map(|_| ())
        })
    }

    /// Relay restart: nothing is connected until a helper says hello again.
    pub fn local_devices_reset(&self) -> Result<()> {
        self.with(|c| {
            c.execute("UPDATE local_devices SET connected = 0", [])
                .map(|_| ())
        })
    }

    pub fn local_devices(&self) -> Result<Vec<LocalDevice>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM local_devices ORDER BY first_seen",
                Self::LOCAL_COLS
            ))?;
            let rows = st.query_map([], Self::row_to_local)?;
            rows.collect()
        })
    }

    pub fn local_device(&self, device_id: &str) -> Result<Option<LocalDevice>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM local_devices WHERE device_id = ?1",
                    Self::LOCAL_COLS
                ),
                params![device_id],
                Self::row_to_local,
            )
            .optional()
        })
    }

    pub fn local_device_set_allow_reads(&self, device_id: &str, allow: bool) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE local_devices SET allow_reads = ?2 WHERE device_id = ?1",
                params![device_id, allow as i64],
            )
            .map(|n| n > 0)
        })
    }

    pub fn local_device_delete(&self, device_id: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "DELETE FROM local_devices WHERE device_id = ?1",
                params![device_id],
            )
            .map(|n| n > 0)
        })
    }
}

impl Db {
    // ---- routines -----------------------------------------------------------------------------

    const ROUTINE_COLS: &'static str =
        "id, project_id, chat_id, name, prompt, trigger, enabled, created_at, last_run_at, next_run_at";

    fn row_to_routine(r: &rusqlite::Row<'_>) -> rusqlite::Result<Routine> {
        Ok(Routine {
            id: r.get(0)?,
            project_id: r.get(1)?,
            chat_id: r.get(2)?,
            name: r.get(3)?,
            prompt: r.get(4)?,
            trigger: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
            enabled: r.get::<_, i64>(6)? != 0,
            created_at: r.get(7)?,
            last_run_at: r.get(8)?,
            next_run_at: r.get(9)?,
        })
    }

    pub fn insert_routine(&self, rt: &Routine) -> Result<()> {
        self.with(|c| {
            c.execute(
                &format!(
                    "INSERT INTO routines({}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    Self::ROUTINE_COLS
                ),
                params![
                    rt.id,
                    rt.project_id,
                    rt.chat_id,
                    rt.name,
                    rt.prompt,
                    rt.trigger.to_string(),
                    rt.enabled as i64,
                    rt.created_at,
                    rt.last_run_at,
                    rt.next_run_at
                ],
            )
            .map(|_| ())
        })
    }

    pub fn routine(&self, id: &str) -> Result<Option<Routine>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {} FROM routines WHERE id = ?1", Self::ROUTINE_COLS),
                params![id],
                Self::row_to_routine,
            )
            .optional()
        })
    }

    pub fn list_routines(&self, project_id: &str) -> Result<Vec<Routine>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM routines WHERE project_id = ?1 ORDER BY created_at",
                Self::ROUTINE_COLS
            ))?;
            let rows = st.query_map(params![project_id], Self::row_to_routine)?;
            rows.collect()
        })
    }

    /// Enabled routines whose `next_run_at` has passed (cron only; webhook/event rows have no next run).
    pub fn due_routines(&self, now: i64) -> Result<Vec<Routine>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM routines WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ?1 ORDER BY next_run_at",
                Self::ROUTINE_COLS
            ))?;
            let rows = st.query_map(params![now], Self::row_to_routine)?;
            rows.collect()
        })
    }

    /// Enabled routines of a project whose trigger is `{ "kind": "event", "event": <event> }`.
    pub fn event_routines(&self, project_id: &str, event: &str) -> Result<Vec<Routine>> {
        Ok(self
            .list_routines(project_id)?
            .into_iter()
            .filter(|r| {
                r.enabled
                    && r.trigger.get("kind").and_then(serde_json::Value::as_str) == Some("event")
                    && r.trigger.get("event").and_then(serde_json::Value::as_str) == Some(event)
            })
            .collect())
    }

    /// Partial update (pause/resume/edit). `next_run_at` is always written: the caller recomputes it.
    #[allow(clippy::too_many_arguments)]
    pub fn update_routine(
        &self,
        id: &str,
        name: Option<&str>,
        prompt: Option<&str>,
        chat_id: Option<&str>,
        trigger: Option<&serde_json::Value>,
        enabled: Option<bool>,
        next_run_at: Option<i64>,
    ) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE routines SET name = COALESCE(?2, name), prompt = COALESCE(?3, prompt), chat_id = COALESCE(?4, chat_id),
                        trigger = COALESCE(?5, trigger), enabled = COALESCE(?6, enabled), next_run_at = ?7
                 WHERE id = ?1",
                params![id, name, prompt, chat_id, trigger.map(|t| t.to_string()), enabled.map(|e| e as i64), next_run_at],
            )
            .map(|n| n > 0)
        })
    }

    /// Record a fire: `last_run_at = now`, `next_run_at` to the following occurrence (or NULL).
    pub fn mark_routine_fired(&self, id: &str, now: i64, next_run_at: Option<i64>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE routines SET last_run_at = ?2, next_run_at = ?3 WHERE id = ?1",
                params![id, now, next_run_at],
            )
            .map(|_| ())
        })
    }

    pub fn delete_routine(&self, id: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "DELETE FROM routine_runs WHERE routine_id = ?1",
                params![id],
            )?;
            c.execute("DELETE FROM routines WHERE id = ?1", params![id])
                .map(|n| n > 0)
        })
    }

    pub fn insert_routine_run(&self, run: &RoutineRun) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO routine_runs(id, routine_id, started_at, finished_at, status, message_id, note) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![run.id, run.routine_id, run.started_at, run.finished_at, run.status, run.message_id, run.note],
            )
            .map(|_| ())
        })
    }

    pub fn routine_run(&self, id: &str) -> Result<Option<RoutineRun>> {
        self.with(|c| {
            c.query_row(
                "SELECT id, routine_id, started_at, finished_at, status, message_id, note FROM routine_runs WHERE id = ?1",
                params![id],
                Self::row_to_run,
            )
            .optional()
        })
    }

    fn row_to_run(r: &rusqlite::Row<'_>) -> rusqlite::Result<RoutineRun> {
        Ok(RoutineRun {
            id: r.get(0)?,
            routine_id: r.get(1)?,
            started_at: r.get(2)?,
            finished_at: r.get(3)?,
            status: r.get(4)?,
            message_id: r.get(5)?,
            note: r.get(6)?,
        })
    }

    /// Terminal statuses set `finished_at`; `running` leaves it NULL.
    pub fn update_routine_run(
        &self,
        id: &str,
        status: &str,
        message_id: Option<&str>,
        note: Option<&str>,
    ) -> Result<bool> {
        let finished = matches!(status, "done" | "failed" | "skipped").then(now_ms);
        self.with(|c| {
            c.execute(
                "UPDATE routine_runs SET status = ?2, message_id = COALESCE(?3, message_id), note = COALESCE(?4, note),
                        finished_at = COALESCE(?5, finished_at) WHERE id = ?1",
                params![id, status, message_id, note, finished],
            )
            .map(|n| n > 0)
        })
    }

    pub fn routine_runs(&self, routine_id: &str, limit: i64) -> Result<Vec<RoutineRun>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, routine_id, started_at, finished_at, status, message_id, note FROM routine_runs
                 WHERE routine_id = ?1 ORDER BY started_at DESC LIMIT ?2",
            )?;
            let rows = st.query_map(params![routine_id, limit], Self::row_to_run)?;
            rows.collect()
        })
    }

    /// Runs the box never picked up or never finished: `queued`/`running` older than `max_age_ms`.
    pub fn stale_routine_runs(&self, max_age_ms: i64) -> Result<Vec<RoutineRun>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, routine_id, started_at, finished_at, status, message_id, note FROM routine_runs
                 WHERE status IN ('queued', 'running') AND started_at < ?1",
            )?;
            let rows = st.query_map(params![now_ms() - max_age_ms], Self::row_to_run)?;
            rows.collect()
        })
    }

    // ---- goals (long-running objectives) ------------------------------------------------------

    const GOAL_COLS: &'static str = "id, project_id, chat_id, title, outcome, plan, status, progress_log, next_step, check_in_cron, check_in_tz, routine_id, autonomy, linked_chat_ids, linked_subagent_ids, waiting_question, created_at, updated_at, last_moved_at";

    fn row_to_goal(r: &rusqlite::Row<'_>) -> rusqlite::Result<Goal> {
        let parse = |i: usize| -> rusqlite::Result<serde_json::Value> {
            let s: String = r.get(i)?;
            Ok(serde_json::from_str(&s).unwrap_or_else(|_| serde_json::json!([])))
        };
        Ok(Goal {
            id: r.get(0)?,
            project_id: r.get(1)?,
            chat_id: r.get(2)?,
            title: r.get(3)?,
            outcome: r.get(4)?,
            plan: parse(5)?,
            status: r.get(6)?,
            progress_log: parse(7)?,
            next_step: r.get(8)?,
            check_in_cron: r.get(9)?,
            check_in_tz: r.get(10)?,
            routine_id: r.get(11)?,
            autonomy: r.get(12)?,
            linked_chat_ids: parse(13)?,
            linked_subagent_ids: parse(14)?,
            waiting_question: r.get(15)?,
            created_at: r.get(16)?,
            updated_at: r.get(17)?,
            last_moved_at: r.get(18)?,
        })
    }

    pub fn insert_goal(&self, g: &Goal) -> Result<()> {
        self.with(|c| {
            c.execute(
                &format!(
                    "INSERT INTO goals({}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
                    Self::GOAL_COLS
                ),
                params![
                    g.id,
                    g.project_id,
                    g.chat_id,
                    g.title,
                    g.outcome,
                    g.plan.to_string(),
                    g.status,
                    g.progress_log.to_string(),
                    g.next_step,
                    g.check_in_cron,
                    g.check_in_tz,
                    g.routine_id,
                    g.autonomy,
                    g.linked_chat_ids.to_string(),
                    g.linked_subagent_ids.to_string(),
                    g.waiting_question,
                    g.created_at,
                    g.updated_at,
                    g.last_moved_at,
                ],
            )
            .map(|_| ())
        })
    }

    pub fn goal(&self, id: &str) -> Result<Option<Goal>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {} FROM goals WHERE id = ?1", Self::GOAL_COLS),
                params![id],
                Self::row_to_goal,
            )
            .optional()
        })
    }

    pub fn list_goals(&self, project_id: &str) -> Result<Vec<Goal>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM goals WHERE project_id = ?1 ORDER BY COALESCE(last_moved_at, updated_at) DESC",
                Self::GOAL_COLS
            ))?;
            let rows = st.query_map(params![project_id], Self::row_to_goal)?;
            rows.collect()
        })
    }

    pub fn update_goal(&self, g: &Goal) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE goals SET chat_id=?2, title=?3, outcome=?4, plan=?5, status=?6, progress_log=?7,
                 next_step=?8, check_in_cron=?9, check_in_tz=?10, routine_id=?11, autonomy=?12,
                 linked_chat_ids=?13, linked_subagent_ids=?14, waiting_question=?15,
                 updated_at=?16, last_moved_at=?17 WHERE id=?1",
                params![
                    g.id,
                    g.chat_id,
                    g.title,
                    g.outcome,
                    g.plan.to_string(),
                    g.status,
                    g.progress_log.to_string(),
                    g.next_step,
                    g.check_in_cron,
                    g.check_in_tz,
                    g.routine_id,
                    g.autonomy,
                    g.linked_chat_ids.to_string(),
                    g.linked_subagent_ids.to_string(),
                    g.waiting_question,
                    g.updated_at,
                    g.last_moved_at,
                ],
            )
            .map(|_| ())
        })
    }

    pub fn delete_goal(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM goals WHERE id = ?1", params![id])
                .map(|_| ())
        })
    }

    // ---- audit mirror (feature `approve`): boxes POST their audit.jsonl lines here ------------

    pub fn record_audit(
        &self,
        project_id: &str,
        kind: &str,
        entry: &serde_json::Value,
    ) -> Result<()> {
        let at = entry.get("ts").and_then(|v| v.as_i64()).unwrap_or(now_ms());
        self.with(|c| {
            c.execute(
                "INSERT INTO audit(project_id, kind, entry, at) VALUES (?1,?2,?3,?4)",
                params![project_id, kind, entry.to_string(), at],
            )
            .map(|_| ())
        })
    }

    /// The newest `limit` audit entries, oldest first.
    pub fn audit(&self, project_id: &str, limit: i64) -> Result<Vec<serde_json::Value>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT entry FROM audit WHERE project_id = ?1 ORDER BY at DESC, id DESC LIMIT ?2",
            )?;
            let rows = st.query_map(params![project_id, limit], |r| {
                let raw: String = r.get(0)?;
                Ok(serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null))
            })?;
            let mut out: Vec<serde_json::Value> = rows.collect::<Result<_, _>>()?;
            out.reverse();
            Ok(out)
        })
    }
}

impl Db {
    // ---- skills (F5) --------------------------------------------------------------------------

    const SKILL_COLS: &'static str = "id, slug, name, description, tags, when_to_use, body, origin_project_id, shared, version, created_at, updated_at";

    fn row_to_skill(r: &rusqlite::Row<'_>) -> rusqlite::Result<Skill> {
        Ok(Skill {
            id: r.get(0)?,
            slug: r.get(1)?,
            name: r.get(2)?,
            description: r.get(3)?,
            tags: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
            when_to_use: r.get(5)?,
            body: r.get(6)?,
            origin_project_id: r.get(7)?,
            shared: r.get::<_, i64>(8)? != 0,
            version: r.get(9)?,
            created_at: r.get(10)?,
            updated_at: r.get(11)?,
        })
    }

    fn reindex_skill(c: &Connection, s: &Skill) -> rusqlite::Result<()> {
        c.execute("DELETE FROM skills_fts WHERE id = ?1", params![s.id])?;
        c.execute(
            "INSERT INTO skills_fts(text, id) VALUES (?1, ?2)",
            params![s.match_text(), s.id],
        )?;
        Ok(())
    }

    /// Insert a skill (fails on a duplicate slug).
    pub fn insert_skill(&self, s: &Skill) -> Result<()> {
        self.with(|c| {
            c.execute(
                &format!(
                    "INSERT INTO skills({}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                    Self::SKILL_COLS
                ),
                params![
                    s.id,
                    s.slug,
                    s.name,
                    s.description,
                    serde_json::to_string(&s.tags).unwrap_or_else(|_| "[]".into()),
                    s.when_to_use,
                    s.body,
                    s.origin_project_id,
                    s.shared as i64,
                    s.version,
                    s.created_at,
                    s.updated_at
                ],
            )?;
            Self::reindex_skill(c, s)
        })
    }

    /// Replace the editable fields of a skill; bumps `version` and `updated_at`. Returns the new row.
    pub fn update_skill(&self, s: &Skill) -> Result<Option<Skill>> {
        let now = now_ms();
        self.with(|c| {
            let n = c.execute(
                "UPDATE skills SET slug = ?2, name = ?3, description = ?4, tags = ?5, when_to_use = ?6, body = ?7,
                        shared = ?8, version = version + 1, updated_at = ?9 WHERE id = ?1",
                params![
                    s.id, s.slug, s.name, s.description, serde_json::to_string(&s.tags).unwrap_or_else(|_| "[]".into()),
                    s.when_to_use, s.body, s.shared as i64, now
                ],
            )?;
            if n == 0 {
                return Ok(None);
            }
            let row = c.query_row(
                &format!("SELECT {} FROM skills WHERE id = ?1", Self::SKILL_COLS),
                params![s.id],
                Self::row_to_skill,
            )?;
            Self::reindex_skill(c, &row)?;
            Ok(Some(row))
        })
    }

    pub fn delete_skill(&self, id: &str) -> Result<bool> {
        self.with(|c| {
            c.execute("DELETE FROM skills_fts WHERE id = ?1", params![id])?;
            c.execute("DELETE FROM skills WHERE id = ?1", params![id])
                .map(|n| n > 0)
        })
    }

    /// One skill by id or slug.
    pub fn skill(&self, id_or_slug: &str) -> Result<Option<Skill>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM skills WHERE id = ?1 OR slug = ?1",
                    Self::SKILL_COLS
                ),
                params![id_or_slug],
                Self::row_to_skill,
            )
            .optional()
        })
    }

    /// Every skill (devices), or the shared ones plus a project's own (`visible_to = Some(project)`).
    pub fn list_skills(&self, visible_to: Option<&str>) -> Result<Vec<Skill>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM skills WHERE ?1 IS NULL OR shared = 1 OR origin_project_id = ?1 ORDER BY name",
                Self::SKILL_COLS
            ))?;
            let rows = st.query_map(params![visible_to], Self::row_to_skill)?;
            rows.collect()
        })
    }

    /// FTS5 over name/description/tags/when_to_use, best match first.
    pub fn search_skills(
        &self,
        query: &str,
        visible_to: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Skill>> {
        let q = fts_query(query);
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM skills_fts JOIN skills s ON s.id = skills_fts.id
                 WHERE skills_fts MATCH ?1 AND (?2 IS NULL OR s.shared = 1 OR s.origin_project_id = ?2)
                 ORDER BY bm25(skills_fts) LIMIT ?3",
                Self::SKILL_COLS.split(", ").map(|col| format!("s.{col}")).collect::<Vec<_>>().join(", ")
            ))?;
            let rows = st.query_map(params![q, visible_to, limit], Self::row_to_skill)?;
            rows.collect()
        })
    }

    /// Cheap change marker for the box poll: `count:max(updated_at)` over the skills a project sees.
    pub fn skills_rev(&self, visible_to: Option<&str>) -> Result<String> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*), COALESCE(MAX(updated_at), 0) FROM skills WHERE ?1 IS NULL OR shared = 1 OR origin_project_id = ?1",
                params![visible_to],
                |r| Ok(format!("{}:{}", r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
        })
    }

    /// Project ids that have a sandbox row (targets for a `skills_changed` push).
    pub fn sandbox_project_ids(&self) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT project_id FROM sandboxes")?;
            let rows = st.query_map([], |r| r.get(0))?;
            rows.collect()
        })
    }
}

/// Free text → a safe FTS5 query: every term quoted, any term matches (OR), prefix match on the last.
fn fts_query(q: &str) -> String {
    let mut terms: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect();
    let Some(last) = terms.last_mut() else {
        return "\"\"".to_owned();
    };
    last.push('*');
    terms.join(" OR ")
}

/// Legacy F8 tables: read by the one-shot migration to `secrets` (`secrets::migrate_f8`) and
/// by its tests; nothing else writes them any more.
#[allow(dead_code)]
impl Db {
    // ---- vault (F8) ---------------------------------------------------------------------------

    const VAULT_ITEM_COLS: &'static str =
        "id, name, kind, fields, allowed_hosts, created_at, last_used_at, daytona_secrets, user_id";

    fn row_to_vault_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<VaultItem> {
        Ok(VaultItem {
            id: r.get(0)?,
            name: r.get(1)?,
            kind: r.get(2)?,
            fields_enc: r.get(3)?,
            allowed_hosts: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
            created_at: r.get(5)?,
            last_used_at: r.get(6)?,
            daytona_secrets: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default(),
            user_id: r.get(8)?,
        })
    }

    /// Insert or update one item. The owner never changes on update: an id that exists under
    /// another user is left alone (the row count tells the caller nothing happened).
    pub fn upsert_vault_item(&self, it: &VaultItem) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO vault_items(id, name, kind, fields, allowed_hosts, created_at, last_used_at, daytona_secrets, user_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name, kind=excluded.kind, fields=excluded.fields,
                    allowed_hosts=excluded.allowed_hosts, daytona_secrets=excluded.daytona_secrets
                 WHERE vault_items.user_id = excluded.user_id",
                params![
                    it.id, it.name, it.kind, it.fields_enc,
                    serde_json::to_string(&it.allowed_hosts).unwrap_or_else(|_| "[]".into()),
                    it.created_at, it.last_used_at, it.daytona_secrets.to_string(), it.user_id
                ],
            )
            .map(|_| ())
        })
    }

    /// One user's items, by name.
    pub fn list_vault_items(&self, user_id: &str) -> Result<Vec<VaultItem>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM vault_items WHERE user_id = ?1 ORDER BY name",
                Self::VAULT_ITEM_COLS
            ))?;
            let rows = st.query_map(params![user_id], Self::row_to_vault_item)?;
            rows.collect()
        })
    }

    /// One item of one user; another user's id reads as absent.
    pub fn vault_item(&self, user_id: &str, id: &str) -> Result<Option<VaultItem>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM vault_items WHERE id = ?1 AND user_id = ?2",
                    Self::VAULT_ITEM_COLS
                ),
                params![id, user_id],
                Self::row_to_vault_item,
            )
            .optional()
        })
    }

    pub fn vault_item_by_name(&self, user_id: &str, name: &str) -> Result<Option<VaultItem>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM vault_items WHERE user_id = ?1 AND name = ?2",
                    Self::VAULT_ITEM_COLS
                ),
                params![user_id, name],
                Self::row_to_vault_item,
            )
            .optional()
        })
    }

    pub fn touch_vault_item(&self, user_id: &str, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE vault_items SET last_used_at = ?3 WHERE id = ?1 AND user_id = ?2",
                params![id, user_id, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn delete_vault_item(&self, user_id: &str, id: &str) -> Result<bool> {
        self.with(|c| {
            let n = c.execute(
                "DELETE FROM vault_items WHERE id = ?1 AND user_id = ?2",
                params![id, user_id],
            )?;
            if n > 0 {
                c.execute(
                    "UPDATE vault_requests SET status = 'expired' WHERE item_id = ?1 AND status IN ('pending', 'approved')",
                    params![id],
                )?;
            }
            Ok(n > 0)
        })
    }

    const VAULT_REQ_COLS: &'static str =
        "id, project_id, chat_id, item_id, item_name, purpose, use_kind, status, created_at, decided_at, expires_at";

    fn row_to_vault_request(r: &rusqlite::Row<'_>) -> rusqlite::Result<VaultRequest> {
        Ok(VaultRequest {
            id: r.get(0)?,
            project_id: r.get(1)?,
            chat_id: r.get(2)?,
            item_id: r.get(3)?,
            item_name: r.get(4)?,
            purpose: r.get(5)?,
            use_kind: r.get(6)?,
            status: r.get(7)?,
            created_at: r.get(8)?,
            decided_at: r.get(9)?,
            expires_at: r.get(10)?,
        })
    }

    pub fn insert_vault_request(&self, rq: &VaultRequest) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO vault_requests(id, project_id, chat_id, item_id, item_name, purpose, use_kind, status, created_at, decided_at, expires_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    rq.id, rq.project_id, rq.chat_id, rq.item_id, rq.item_name, rq.purpose, rq.use_kind,
                    rq.status, rq.created_at, rq.decided_at, rq.expires_at
                ],
            )
            .map(|_| ())
        })
    }

    /// Pending requests past `expires_at` become `expired` on read, so clients never act on stale ones.
    fn expire_vault_requests(c: &Connection) -> rusqlite::Result<()> {
        c.execute(
            "UPDATE vault_requests SET status = 'expired', decided_at = ?1 WHERE status = 'pending' AND expires_at < ?1",
            params![now_ms()],
        )
        .map(|_| ())
    }

    pub fn vault_request(&self, id: &str) -> Result<Option<VaultRequest>> {
        self.with(|c| {
            Self::expire_vault_requests(c)?;
            c.query_row(
                &format!(
                    "SELECT {} FROM vault_requests WHERE id = ?1",
                    Self::VAULT_REQ_COLS
                ),
                params![id],
                Self::row_to_vault_request,
            )
            .optional()
        })
    }

    /// Requests made by one user's projects: pending first, then the newest decided ones.
    pub fn list_vault_requests(&self, user_id: &str, limit: i64) -> Result<Vec<VaultRequest>> {
        self.with(|c| {
            Self::expire_vault_requests(c)?;
            let cols: String = Self::VAULT_REQ_COLS
                .split(", ")
                .map(|col| format!("r.{col}"))
                .collect::<Vec<_>>()
                .join(", ");
            let mut st = c.prepare(&format!(
                "SELECT {cols} FROM vault_requests r JOIN projects p ON p.id = r.project_id
                 WHERE p.user_id = ?1 ORDER BY (r.status = 'pending') DESC, r.created_at DESC LIMIT ?2"
            ))?;
            let rows = st.query_map(params![user_id, limit], Self::row_to_vault_request)?;
            rows.collect()
        })
    }

    /// Decide a pending request of one of `user_id`'s projects. Returns the updated row, or
    /// `None` when it is not pending or belongs to someone else.
    pub fn decide_vault_request(
        &self,
        user_id: &str,
        id: &str,
        approve: bool,
    ) -> Result<Option<VaultRequest>> {
        self.with(|c| {
            Self::expire_vault_requests(c)?;
            let status = if approve { "approved" } else { "denied" };
            let n = c.execute(
                "UPDATE vault_requests SET status = ?2, decided_at = ?3 WHERE id = ?1 AND status = 'pending'
                 AND project_id IN (SELECT id FROM projects WHERE user_id = ?4)",
                params![id, status, now_ms(), user_id],
            )?;
            if n == 0 {
                return Ok(None);
            }
            c.query_row(
                &format!(
                    "SELECT {} FROM vault_requests WHERE id = ?1",
                    Self::VAULT_REQ_COLS
                ),
                params![id],
                Self::row_to_vault_request,
            )
            .optional()
        })
    }

    /// Distinct owners of legacy vault items (for the one-shot F8 → secrets migration).
    pub fn vault_item_users(&self) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT DISTINCT user_id FROM vault_items")?;
            let rows = st.query_map([], |r| r.get(0))?;
            rows.collect()
        })
    }

    /// Every request ever made for one legacy item, oldest first.
    pub fn vault_requests_for_item(&self, item_id: &str) -> Result<Vec<VaultRequest>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM vault_requests WHERE item_id = ?1 ORDER BY created_at",
                Self::VAULT_REQ_COLS
            ))?;
            let rows = st.query_map(params![item_id], Self::row_to_vault_request)?;
            rows.collect()
        })
    }

    /// Approved requests of a project for one item (any use), newest first.
    pub fn approved_vault_requests(
        &self,
        project_id: &str,
        item_id: &str,
    ) -> Result<Vec<VaultRequest>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM vault_requests WHERE project_id = ?1 AND item_id = ?2 AND status = 'approved' ORDER BY decided_at DESC",
                Self::VAULT_REQ_COLS
            ))?;
            let rows = st.query_map(params![project_id, item_id], Self::row_to_vault_request)?;
            rows.collect()
        })
    }
}

#[cfg(test)]
mod vault_scope_tests {
    use super::*;

    fn temp_db(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "arbos-vault-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn item(id: &str, user: &str, name: &str) -> VaultItem {
        VaultItem {
            id: id.into(),
            user_id: user.into(),
            name: name.into(),
            kind: "api_key".into(),
            fields_enc: "enc".into(),
            allowed_hosts: vec![],
            created_at: 1,
            last_used_at: None,
            daytona_secrets: serde_json::json!({}),
        }
    }

    /// Invariant: a user's box (or device) sees, names, touches, updates and deletes only that
    /// user's items. Two users may both own `STRIPE_KEY`.
    #[test]
    fn items_are_scoped_per_user() {
        let dir = temp_db("scope");
        let db = Db::open(&dir.join("relay.db")).expect("db");
        db.upsert_vault_item(&item("a1", "alice", "STRIPE_KEY"))
            .unwrap();
        db.upsert_vault_item(&item("a2", "alice", "MAILGUN"))
            .unwrap();
        db.upsert_vault_item(&item("b1", "bob", "STRIPE_KEY"))
            .unwrap();

        let names = |u: &str| -> Vec<String> {
            db.list_vault_items(u)
                .unwrap()
                .into_iter()
                .map(|i| i.name)
                .collect()
        };
        assert_eq!(names("alice"), vec!["MAILGUN", "STRIPE_KEY"]);
        assert_eq!(names("bob"), vec!["STRIPE_KEY"]);
        assert!(names("carol").is_empty());

        assert_eq!(
            db.vault_item_by_name("bob", "STRIPE_KEY")
                .unwrap()
                .unwrap()
                .id,
            "b1"
        );
        assert!(db.vault_item_by_name("bob", "MAILGUN").unwrap().is_none());
        assert!(db.vault_item("bob", "a1").unwrap().is_none());
        assert!(db.vault_item("alice", "a1").unwrap().is_some());

        // Bob cannot take over Alice's row by reusing its id, nor delete or touch it.
        let mut hijack = item("a1", "bob", "STOLEN");
        hijack.fields_enc = "bob-enc".into();
        db.upsert_vault_item(&hijack).unwrap();
        let a1 = db.vault_item("alice", "a1").unwrap().unwrap();
        assert_eq!(
            (a1.name.as_str(), a1.fields_enc.as_str()),
            ("STRIPE_KEY", "enc")
        );
        assert!(!db.delete_vault_item("bob", "a1").unwrap());
        db.touch_vault_item("bob", "a1").unwrap();
        assert!(db
            .vault_item("alice", "a1")
            .unwrap()
            .unwrap()
            .last_used_at
            .is_none());
        assert!(db.delete_vault_item("alice", "a1").unwrap());
        assert_eq!(names("alice"), vec!["MAILGUN"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A pre-scoping database (global unique `name`, no `user_id`) migrates: the item a user's
    /// projects requested becomes theirs, the rest stay with the legacy `u_default`, and two
    /// users can own the same name afterwards.
    #[test]
    fn legacy_items_get_an_owner() {
        let dir = temp_db("legacy");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("relay.db");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL, user_id TEXT NOT NULL DEFAULT 'u_default',
                   icon TEXT NOT NULL DEFAULT '', instructions TEXT NOT NULL DEFAULT '', models TEXT NOT NULL DEFAULT '{}',
                   state TEXT NOT NULL DEFAULT 'waking', created_at INTEGER NOT NULL DEFAULT 0, last_active_at INTEGER NOT NULL DEFAULT 0,
                   spend_cap_daily REAL NOT NULL DEFAULT 25.0, project_token_hash TEXT, color TEXT NOT NULL DEFAULT '#8b7cf6',
                   workspace_state TEXT NOT NULL DEFAULT 'stopped', avatar TEXT, deleted_at INTEGER, avatar_color TEXT,
                   description TEXT NOT NULL DEFAULT '', primary_chat_id TEXT, avatar_color_index INTEGER,
                   avatar_leaf INTEGER, avatar_leaf_color INTEGER, image_key TEXT, avatar_version INTEGER NOT NULL DEFAULT 0);
                 CREATE UNIQUE INDEX projects_user_name ON projects(user_id, name);
                 INSERT INTO projects(id, name, user_id) VALUES ('p_bob', 'Research', 'bob');
                 CREATE TABLE vault_items (
                   id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, kind TEXT NOT NULL,
                   fields TEXT NOT NULL, allowed_hosts TEXT NOT NULL DEFAULT '[]',
                   created_at INTEGER NOT NULL, last_used_at INTEGER, daytona_secrets TEXT NOT NULL DEFAULT '{}');
                 INSERT INTO vault_items(id, name, kind, fields, created_at) VALUES ('i_bob', 'STRIPE_KEY', 'api_key', 'enc', 1);
                 INSERT INTO vault_items(id, name, kind, fields, created_at) VALUES ('i_old', 'OLD_KEY', 'api_key', 'enc', 1);
                 CREATE TABLE vault_requests (
                   id TEXT PRIMARY KEY, project_id TEXT NOT NULL, chat_id TEXT NOT NULL DEFAULT '',
                   item_id TEXT, item_name TEXT NOT NULL, purpose TEXT NOT NULL DEFAULT '', use_kind TEXT NOT NULL,
                   status TEXT NOT NULL DEFAULT 'pending', created_at INTEGER NOT NULL, decided_at INTEGER, expires_at INTEGER NOT NULL);
                 INSERT INTO vault_requests(id, project_id, item_id, item_name, use_kind, status, created_at, expires_at)
                   VALUES ('r1', 'p_bob', 'i_bob', 'STRIPE_KEY', 'header', 'approved', 1, 2);",
            )
            .unwrap();
        }
        let db = Db::open(&path).expect("migrate");
        assert_eq!(
            db.vault_item("bob", "i_bob").unwrap().unwrap().name,
            "STRIPE_KEY"
        );
        assert!(db.vault_item("u_default", "i_bob").unwrap().is_none());
        assert_eq!(
            db.vault_item("u_default", "i_old").unwrap().unwrap().name,
            "OLD_KEY"
        );
        // Names are unique per user now, not relay-wide.
        db.upsert_vault_item(&item("i_alice", "alice", "STRIPE_KEY"))
            .unwrap();
        assert_eq!(db.list_vault_items("alice").unwrap().len(), 1);
        // Reopening is a no-op (the migration ran once).
        drop(db);
        let db = Db::open(&path).expect("reopen");
        assert_eq!(db.list_vault_items("bob").unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod avatar_migration_tests {
    use super::*;

    fn project(id: &str, avatar_color: Option<&str>) -> Project {
        Project {
            id: id.into(),
            name: id.into(),
            icon: "P".into(),
            instructions: String::new(),
            models: serde_json::json!({}),
            state: "awake".into(),
            created_at: 0,
            last_active_at: 0,
            spend_cap_daily: 25.0,
            color: "#000".into(),
            user_id: "u_default".into(),
            workspace_state: "stopped".into(),
            avatar: None,
            avatar_color: avatar_color.map(str::to_owned),
            description: String::new(),
            primary_chat_id: None,
            avatar_color_index: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            image_key: None,
            avatar_version: 0,
        }
    }

    /// D74: hash-assigned `mono` is cleared once; a `mono` picked after that stays.
    #[test]
    fn mono_defaults_cleared_once() {
        let dir = std::env::temp_dir().join(format!(
            "arbos-avatar-mig-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("relay.db");
        {
            let db = Db::open(&path).expect("db");
            db.insert_project(&project("p_mono", Some("mono"))).unwrap();
            db.insert_project(&project("p_red", Some("red"))).unwrap();
            // Simulate a database from before the migration existed.
            db.with(|c| {
                c.execute(
                    "DELETE FROM meta WHERE key = 'avatar_mono_defaults_cleared'",
                    [],
                )
            })
            .unwrap();
        }
        {
            let db = Db::open(&path).expect("reopen");
            assert_eq!(db.project("p_mono").unwrap().unwrap().avatar_color, None);
            assert_eq!(
                db.project("p_red")
                    .unwrap()
                    .unwrap()
                    .avatar_color
                    .as_deref(),
                Some("red")
            );
            // An explicit pick made after the migration survives the next start.
            db.insert_project(&project("p_pick", Some("mono"))).unwrap();
        }
        let db = Db::open(&path).expect("reopen again");
        assert_eq!(
            db.project("p_pick")
                .unwrap()
                .unwrap()
                .avatar_color
                .as_deref(),
            Some("mono")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// ARB-394: no two states of one agent's avatar ever share a stamp. A legacy row at 0 is
    /// stamped with its creation time at start-up; a new row is stamped at birth; every avatar
    /// write moves the stamp strictly forward, even inside one millisecond; and a client copy
    /// from before the backfill (stamp 0) loses to anything fetched after it.
    #[test]
    fn avatar_version_backfill_leaves_no_zero_and_no_shared_stamps() {
        let dir = std::env::temp_dir().join(format!(
            "arbos-avatar-stamp-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("relay.db");
        let created = 1_792_802_625_013; // Hezbola's creation time, a real pre-stamp row
        {
            let db = Db::open(&path).expect("db");
            let mut legacy = project("p_legacy", None);
            legacy.created_at = created;
            legacy.image_key = Some("avatar-old.jpg".into());
            db.insert_project(&legacy).unwrap();
            // Force the pre-backfill shape: the row at 0, as every pre-#1813 avatar was.
            db.with(|c| {
                c.execute(
                    "UPDATE projects SET avatar_version = 0 WHERE id = 'p_legacy'",
                    [],
                )
            })
            .unwrap();
            assert_eq!(db.project("p_legacy").unwrap().unwrap().avatar_version, 0);
        }
        let db = Db::open(&path).expect("reopen runs the backfill");
        let legacy = db.project("p_legacy").unwrap().unwrap();
        assert_eq!(
            legacy.avatar_version, created,
            "a 0 row takes its creation time"
        );
        assert!(
            legacy.avatar_version > 0,
            "a stale client copy at 0 loses to this row"
        );

        // A new row is born stamped (never 0), and a pre-stamp client copy always loses.
        let mut fresh = project("p_fresh", None);
        fresh.created_at = created + 5;
        db.insert_project(&fresh).unwrap();
        assert_eq!(
            db.project("p_fresh").unwrap().unwrap().avatar_version,
            created + 5
        );

        // Every change moves the stamp strictly forward — picture, reset to the leaf, picture
        // again — so the states never share a stamp, even inside one millisecond.
        let mut seen = vec![legacy.avatar_version];
        db.set_project_image_key("p_legacy", Some("avatar-new.jpg"))
            .unwrap();
        seen.push(db.project("p_legacy").unwrap().unwrap().avatar_version);
        db.set_project_image_key("p_legacy", None).unwrap();
        seen.push(db.project("p_legacy").unwrap().unwrap().avatar_version);
        db.update_project(
            "p_legacy",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(Some(2)),
            Some(Some(3)),
        )
        .unwrap();
        seen.push(db.project("p_legacy").unwrap().unwrap().avatar_version);
        db.set_project_image_key("p_legacy", Some("avatar-new.jpg"))
            .unwrap();
        seen.push(db.project("p_legacy").unwrap().unwrap().avatar_version);
        for w in seen.windows(2) {
            assert!(w[1] > w[0], "stamps must strictly increase: {seen:?}");
        }
        assert!(
            seen[1] >= now_ms() - 60_000,
            "a change stamps the clock, not the creation time: {seen:?}"
        );

        // The backfill is idempotent: a second start changes nothing.
        let before = db.project("p_legacy").unwrap().unwrap().avatar_version;
        drop(db);
        let db = Db::open(&path).expect("reopen again");
        assert_eq!(
            db.project("p_legacy").unwrap().unwrap().avatar_version,
            before
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
