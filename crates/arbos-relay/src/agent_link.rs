//! Agent access links (2026-10-31; design `docs/agent-access-link-spec.md` in the store): a
//! person hands an outside agent — Claude Code, a Grok Bot, anything that can fetch a URL — one
//! link that lets it read their account and, if they allowed it, speak in their chats **as
//! them**. It is not a member of anything and it is not the user's own device; it is a
//! revocable, expiring grant to a third party.
//!
//! The two credentials, and why there are two:
//!
//! - the **open secret**, 24 URL-safe base64 characters, is the whole URL the owner copies. It
//!   is single-use and lives 15 minutes. Fetching it returns the instruction sheet, and the
//!   sheet is the only place the bearer ever appears.
//! - the **bearer**, `arbl_` + 40 hex, authenticates every later call and **never transits a
//!   URL**. The owner never sees it and is never asked to carry it: the agent collects its own
//!   credential. One thing to copy, and the thing copied is dead the moment it is used.
//!
//! Owner side (ordinary device bearer): `POST /me/agent-links`, `GET /me/agent-links`,
//! `GET /me/agent-links/{id}/activity`, `DELETE /me/agent-links/{id}`.
//! Public: `GET /agent-link/open/{secret}`. Agent side (bearer `arbl_…`): `GET /agent-link/me`.
//!
//! A link token reaches **no** ordinary route. It is not a [`crate::auth::Principal`]: `arbl_`
//! does not start with `eyJ`, so `resolve_checked` sends it to `resolve_legacy`, which looks in
//! `devices` and `boxes` and finds nothing. Default-deny falls out of code that already exists,
//! and the handlers here are the only ones that take a [`LinkPrincipal`].

use arbos_core::auth::hash;
use arbos_core::ids::{now_ms, ulid};
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use rand::RngCore;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Principal;
use crate::db::Db;
use crate::export::READER_KIND;
use crate::signin::api_error;
use crate::state::AppState;

/// Tables for agent access links, run from [`Db::open`] after the main `SCHEMA`, exactly as
/// `crate::outside::SCHEMA` is. Additive: no existing table or column is touched, there is no
/// migration and no backfill, and an older binary rolled back over these ignores them because it
/// never names them in any query.
pub const SCHEMA: &str = r#"
-- One agent access link. `open_hash` is the only copy of the single-use secret in the URL, and
-- `token_hash` the only copy of the bearer. `token_hash` is NULL until the link is opened: only
-- the hash is ever stored, so the bearer cannot be minted at creation and read back later — it
-- is made at the moment it is handed to the agent, and before that no bearer for this link
-- exists anywhere. `open_hash` is kept after the link is opened (`opened_at`) so a second fetch
-- can be told "already opened" rather than "no such link", which is the signal that somebody
-- got there before the agent did.
CREATE TABLE IF NOT EXISTS agent_links (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL,
  label TEXT NOT NULL DEFAULT '',
  scope TEXT NOT NULL,                      -- 'read' | 'reply'
  token_hash TEXT UNIQUE,
  open_hash TEXT NOT NULL UNIQUE,
  open_expires_at INTEGER NOT NULL,
  chats TEXT,                               -- NULL = every chat; JSON array = only those
  created_at INTEGER NOT NULL,
  created_by_device TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  opened_at INTEGER,
  revoked_at INTEGER,
  last_seen_at INTEGER,
  first_write_at INTEGER
);
CREATE INDEX IF NOT EXISTS agent_links_user ON agent_links(user_id, created_at);

-- Durable stream events for one user, read by an open SSE stream from its cursor. Only written
-- while that user has a live link; capped per user and by age.
CREATE TABLE IF NOT EXISTS link_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id TEXT NOT NULL,
  chat_id TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  payload TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS link_events_user ON link_events(user_id, id);
CREATE INDEX IF NOT EXISTS link_events_at ON link_events(created_at);

-- Everything a link did. Written only by the relay and reachable by no client, which is what
-- makes it the authority for "via agent link" attribution: the `deviceId` on a chat row carries
-- the same mark and is client-writable on the room socket path, so the row label is a
-- convenience and this table is the evidence.
CREATE TABLE IF NOT EXISTS link_audit (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  link_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  at INTEGER NOT NULL,
  action TEXT NOT NULL,                     -- created|opened|read|message|reaction|typing|
                                            -- stream_open|stream_close|refused|revoked|expired
  chat_id TEXT,
  message_id TEXT,
  ip TEXT,                                  -- reported by the front proxy; not authenticated
  user_agent TEXT,                          -- sent by the caller; not authenticated
  detail TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS link_audit_link ON link_audit(link_id, at);
CREATE INDEX IF NOT EXISTS link_audit_user ON link_audit(user_id, at);
CREATE INDEX IF NOT EXISTS link_audit_message ON link_audit(message_id);
"#;

/// Bearer prefix; the rest is 40 hex characters (160 bits), the house shape for a third-party
/// credential (`outside::KEY_PREFIX`).
pub const TOKEN_PREFIX: &str = "arbl_";
/// How long the URL in the copy sheet works before it is worthless, opened or not.
const OPEN_TTL_MS: i64 = 15 * 60 * 1000;
/// Shortest, default and longest life of a link.
const TTL_MIN_MS: i64 = 3_600_000;
const TTL_DEFAULT_MS: i64 = 7 * 24 * 3_600_000;
const TTL_MAX_MS: i64 = 30 * 24 * 3_600_000;
/// Longest label the owner can give a link.
const LABEL_MAX: usize = 60;
/// Most links one account may hold at once, and most it may mint in a day: a stolen device
/// cannot mint a fleet of links and spread a read budget across them.
const LIVE_LINKS_MAX: i64 = 5;
const MINTS_PER_DAY: i64 = 10;
/// Requests a link may make in a minute, every endpoint together. Half of ASP's 240: an outside
/// agent is the owner's standing integration, a link is a short-lived grant to a stranger.
const REQUESTS_PER_MIN: i64 = 120;
/// Audit rows one `GET …/activity` call returns.
const ACTIVITY_DEFAULT: i64 = 100;
const ACTIVITY_MAX: i64 = 500;

/// What a link may do. There are exactly two values and a link's scope never changes: to widen
/// one, revoke it and mint another. No code path raises a link's permission, so there is no
/// scope escalation to defend against — only the question of whether the agent should write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Read every endpoint and watch the stream.
    Read,
    /// Everything [`Scope::Read`] may do, plus posting messages, replies and reactions as the user.
    Reply,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Reply => "reply",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "read" => Some(Scope::Read),
            "reply" => Some(Scope::Reply),
            _ => None,
        }
    }
}

/// One agent access link row.
#[derive(Debug, Clone)]
pub struct AgentLink {
    pub id: String,
    pub user_id: String,
    pub label: String,
    pub scope: Scope,
    /// `None` = every chat of the account; `Some(list)` = only those. An empty list is refused
    /// at mint, so `Some` always means at least one chat and can never read as "no filter".
    pub chats: Option<Vec<String>>,
    pub created_at: i64,
    pub expires_at: i64,
    pub open_expires_at: i64,
    pub opened_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub last_seen_at: Option<i64>,
    pub first_write_at: Option<i64>,
}

impl AgentLink {
    /// Live = not revoked and not past its expiry. Checked on every single request: there is no
    /// token cache and no JWT here, so a revoked link's next call is refused rather than staying
    /// good until some signature expires.
    pub fn live(&self, now: i64) -> bool {
        self.revoked_at.is_none() && self.expires_at > now
    }
}

// ---- storage ------------------------------------------------------------------------------------

impl Db {
    fn link_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AgentLink> {
        let scope: String = r.get("scope")?;
        let chats: Option<String> = r.get("chats")?;
        Ok(AgentLink {
            id: r.get("id")?,
            user_id: r.get("user_id")?,
            label: r.get("label")?,
            // A row whose scope is unreadable is the weaker of the two, never the stronger.
            scope: Scope::parse(&scope).unwrap_or(Scope::Read),
            chats: chats.and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok()),
            created_at: r.get("created_at")?,
            expires_at: r.get("expires_at")?,
            open_expires_at: r.get("open_expires_at")?,
            opened_at: r.get("opened_at")?,
            revoked_at: r.get("revoked_at")?,
            last_seen_at: r.get("last_seen_at")?,
            first_write_at: r.get("first_write_at")?,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_agent_link(
        &self,
        id: &str,
        user_id: &str,
        label: &str,
        scope: Scope,
        open_hash: &str,
        chats: Option<&[String]>,
        created_by_device: &str,
        ttl_ms: i64,
    ) -> anyhow::Result<()> {
        let now = now_ms();
        let chats_json = chats.map(|c| serde_json::to_string(c).unwrap_or_else(|_| "null".into()));
        self.with(|c| {
            c.execute(
                "INSERT INTO agent_links(id, user_id, label, scope, open_hash,
                     open_expires_at, chats, created_at, created_by_device, expires_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    id,
                    user_id,
                    label,
                    scope.as_str(),
                    open_hash,
                    now + OPEN_TTL_MS,
                    chats_json,
                    now,
                    created_by_device,
                    now + ttl_ms
                ],
            )
            .map(|_| ())
        })
    }

    pub fn agent_link_by_token_hash(&self, h: &str) -> anyhow::Result<Option<AgentLink>> {
        self.with(|c| {
            c.query_row(
                "SELECT * FROM agent_links WHERE token_hash = ?1",
                params![h],
                Self::link_from_row,
            )
            .optional()
        })
    }

    pub fn agent_link_by_open_hash(&self, h: &str) -> anyhow::Result<Option<AgentLink>> {
        self.with(|c| {
            c.query_row(
                "SELECT * FROM agent_links WHERE open_hash = ?1",
                params![h],
                Self::link_from_row,
            )
            .optional()
        })
    }

    pub fn agent_link(&self, id: &str) -> anyhow::Result<Option<AgentLink>> {
        self.with(|c| {
            c.query_row(
                "SELECT * FROM agent_links WHERE id = ?1",
                params![id],
                Self::link_from_row,
            )
            .optional()
        })
    }

    /// Every link of a user, newest first, revoked and expired ones included.
    pub fn agent_links_of_user(&self, user_id: &str) -> anyhow::Result<Vec<AgentLink>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT * FROM agent_links WHERE user_id = ?1 ORDER BY created_at DESC, id DESC",
            )?;
            let rows = st.query_map(params![user_id], Self::link_from_row)?;
            rows.collect()
        })
    }

    pub fn live_agent_link_count(&self, user_id: &str) -> anyhow::Result<i64> {
        let now = now_ms();
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM agent_links
                 WHERE user_id = ?1 AND revoked_at IS NULL AND expires_at > ?2",
                params![user_id, now],
                |r| r.get(0),
            )
        })
    }

    /// Spend the open secret and record the bearer that fetch is being given, in one statement.
    /// Returns `true` only for the call that spent it, so two fetches racing cannot both be
    /// handed a bearer.
    ///
    /// The two halves are one `UPDATE` rather than two on purpose. Done separately, a failure
    /// between them leaves a link marked opened with no bearer: nobody can authenticate as it
    /// and nobody can open it again, so the owner's only recovery is to notice and mint another.
    /// That is the same shape as the Matrix defect in #2460 — a success path that leaves state
    /// no later call can reach.
    pub fn open_agent_link(&self, id: &str, token_hash: &str) -> anyhow::Result<bool> {
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "UPDATE agent_links SET opened_at = ?2, token_hash = ?3
                 WHERE id = ?1 AND opened_at IS NULL",
                params![id, now, token_hash],
            )
            .map(|n| n > 0)
        })
    }

    pub fn touch_agent_link(&self, id: &str) -> anyhow::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE agent_links SET last_seen_at = ?2 WHERE id = ?1",
                params![id, now_ms()],
            )
            .map(|_| ())
        })
    }

    /// Revoke one link. `true` when this call was the one that closed it.
    pub fn revoke_agent_link(&self, id: &str) -> anyhow::Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE agent_links SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
                params![id, now_ms()],
            )
            .map(|n| n > 0)
        })
    }

    /// Revoke every live link of an account. Returns the ids actually closed, for the audit.
    pub fn revoke_agent_links_of_user(&self, user_id: &str) -> anyhow::Result<Vec<String>> {
        let now = now_ms();
        self.with(|c| {
            let mut st =
                c.prepare("SELECT id FROM agent_links WHERE user_id = ?1 AND revoked_at IS NULL")?;
            let ids: Vec<String> = st
                .query_map(params![user_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for id in &ids {
                c.execute(
                    "UPDATE agent_links SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
                    params![id, now],
                )?;
            }
            Ok(ids)
        })
    }

    /// Append one audit row. Written only here, inside the relay; no client can reach this table.
    #[allow(clippy::too_many_arguments)]
    pub fn note_link_audit(
        &self,
        link_id: &str,
        user_id: &str,
        action: &str,
        chat_id: Option<&str>,
        message_id: Option<&str>,
        ip: Option<&str>,
        user_agent: Option<&str>,
        detail: &Value,
    ) -> anyhow::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO link_audit(link_id, user_id, at, action, chat_id, message_id, ip,
                     user_agent, detail)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    link_id,
                    user_id,
                    now_ms(),
                    action,
                    chat_id,
                    message_id,
                    ip,
                    user_agent,
                    detail.to_string()
                ],
            )
            .map(|_| ())
        })
    }

    /// Audit rows of one link, newest first, optionally older than `before` (an audit row id).
    pub fn link_audit_rows(
        &self,
        link_id: &str,
        before: Option<i64>,
        limit: i64,
    ) -> anyhow::Result<Vec<Value>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, at, action, chat_id, message_id, ip, user_agent, detail
                 FROM link_audit WHERE link_id = ?1 AND (?2 IS NULL OR id < ?2)
                 ORDER BY id DESC LIMIT ?3",
            )?;
            let rows = st.query_map(params![link_id, before, limit], |r| {
                let detail: String = r.get(7)?;
                Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "at": r.get::<_, i64>(1)?,
                    "action": r.get::<_, String>(2)?,
                    "chat_id": r.get::<_, Option<String>>(3)?,
                    "message_id": r.get::<_, Option<String>>(4)?,
                    "ip": r.get::<_, Option<String>>(5)?,
                    "user_agent": r.get::<_, Option<String>>(6)?,
                    "detail": serde_json::from_str::<Value>(&detail).unwrap_or(Value::Null),
                }))
            })?;
            rows.collect()
        })
    }
}

// ---- minting ------------------------------------------------------------------------------------

/// A bearer; only its hash is stored.
fn mint_token() -> String {
    let mut bytes = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{TOKEN_PREFIX}{}", hex::encode(bytes))
}

/// The single-use secret that *is* the URL: 18 bytes as 24 URL-safe base64 characters.
fn mint_open_secret() -> String {
    let mut bytes = [0u8; 18];
    rand::thread_rng().fill_bytes(&mut bytes);
    B64URL.encode(bytes)
}

fn relay_of(state: &AppState) -> String {
    state.cfg.public_url.trim_end_matches('/').to_owned()
}

/// The one response in the whole relay that carries a link's bearer must not cross plain HTTP.
/// Loopback is allowed so a self-hoster can develop against `http://127.0.0.1:…`.
fn transport_is_safe(public_url: &str) -> bool {
    public_url.starts_with("https://")
        || public_url.starts_with("http://127.0.0.1")
        || public_url.starts_with("http://localhost")
        || public_url.starts_with("http://[::1]")
}

#[derive(Deserialize, Default)]
pub struct MintBody {
    #[serde(default)]
    pub label: Option<String>,
    pub scope: String,
    #[serde(default)]
    pub ttl_hours: Option<i64>,
    /// Chats this link may reach. Absent or `null` = every chat; a list = only those. An empty
    /// list is refused rather than read as "no filter".
    #[serde(default)]
    pub chats: Option<Vec<String>>,
}

/// `POST /me/agent-links` — mint a link and return the URL **once**.
///
/// The answer carries no bearer. The owner copies one URL, the agent fetches it, and the agent
/// collects its own credential from that one response: the person is never asked to transport a
/// secret, and the thing they did paste is worthless the moment it is used.
pub async fn mint(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    body: Option<Json<MintBody>>,
) -> Response {
    let Principal::Device {
        id: device_id,
        user_id,
        ..
    } = &principal
    else {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        );
    };
    if !transport_is_safe(&state.cfg.public_url) {
        return api_error(
            StatusCode::CONFLICT,
            "insecure_transport",
            "this relay's public URL is plain HTTP; an agent link would hand its key over an \
             unencrypted connection",
        );
    }
    let Some(Json(b)) = body else {
        return api_error(StatusCode::BAD_REQUEST, "bad_scope", "name a scope");
    };
    let Some(scope) = Scope::parse(&b.scope) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_scope",
            "scope is \"read\" or \"reply\"",
        );
    };
    let ttl_ms = match b.ttl_hours {
        None => TTL_DEFAULT_MS,
        Some(h) => {
            let ms = h.saturating_mul(3_600_000);
            if !(TTL_MIN_MS..=TTL_MAX_MS).contains(&ms) {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "bad_ttl",
                    "a link lives between 1 hour and 30 days",
                );
            }
            ms
        }
    };
    let chats = match &b.chats {
        None => None,
        Some(list) if list.is_empty() => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_chats",
                "name at least one chat, or leave chats out for all of them",
            )
        }
        Some(list) => Some(list.clone()),
    };
    let label = b
        .label
        .map(|s| s.trim().chars().take(LABEL_MAX).collect::<String>())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Agent link".to_owned());

    match state.db.live_agent_link_count(user_id) {
        Ok(n) if n >= LIVE_LINKS_MAX => {
            return api_error(
                StatusCode::CONFLICT,
                "too_many_links",
                format!("{LIVE_LINKS_MAX} links at once is the most; revoke one first"),
            )
        }
        Err(e) => return db_error("live link count", e),
        _ => {}
    }
    if !state
        .db
        .rate_check_and_note(
            &format!("linkmint:{user_id}"),
            "linkmint",
            24 * 3_600_000,
            MINTS_PER_DAY,
        )
        .unwrap_or(true)
    {
        return rate_limited(
            &state,
            &format!("linkmint:{user_id}"),
            "linkmint",
            24 * 3_600_000,
        );
    }

    let id = ulid();
    let secret = mint_open_secret();
    if let Err(e) = state.db.insert_agent_link(
        &id,
        user_id,
        &label,
        scope,
        &hash(&secret),
        chats.as_deref(),
        device_id,
        ttl_ms,
    ) {
        return db_error("insert link", e);
    }
    let _ = state.db.note_link_audit(
        &id,
        user_id,
        "created",
        None,
        None,
        client_ip(&headers).as_deref(),
        user_agent(&headers).as_deref(),
        &json!({ "scope": scope.as_str(), "label": label, "chats": chats }),
    );
    tracing::info!(user = %user_id, link = %id, scope = scope.as_str(), "agent link minted");

    let Ok(Some(link)) = state.db.agent_link(&id) else {
        return api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db",
            "the link was written but could not be read back",
        );
    };
    let mut row = link_json(&link);
    row["url"] = json!(format!("{}/agent-link/open/{secret}", relay_of(&state)));
    row["url_note"] = json!(
        "Opens once, within 15 minutes. Give it to the agent; the agent collects its own key."
    );
    (StatusCode::CREATED, Json(row)).into_response()
}

// ---- the owner's list -----------------------------------------------------------------------------

/// How a row in the owner's list came to exist.
///
/// `export` rows are the earlier generation of the same idea: `export::mint_reader` has minted a
/// long-lived read-only `adv_` bearer since before this feature, stored as a `devices` row with
/// `kind = 'reader'`. They are listed here, and revoked here, so the account has **one** place
/// that means "tokens I gave an agent" — but they are not pretended to be links. They never
/// expire, they carry no audit, their media URLs are bearer-less for an hour, and their secret
/// passed through the owner's hands. The list says so per row rather than flattening them.
fn link_json(link: &AgentLink) -> Value {
    let now = now_ms();
    json!({
        "id": link.id,
        "origin": "link",
        "label": link.label,
        "scope": link.scope.as_str(),
        "created_at": link.created_at,
        "expires_at": link.expires_at,
        "opened_at": link.opened_at,
        "revoked_at": link.revoked_at,
        "last_seen_at": link.last_seen_at,
        "first_write_at": link.first_write_at,
        "chats": link.chats,
        "live": link.live(now),
        "state": if link.revoked_at.is_some() {
            "revoked"
        } else if link.expires_at <= now {
            "expired"
        } else if link.opened_at.is_some() {
            "in_use"
        } else {
            "waiting"
        },
    })
}

fn export_token_json(row: &Value) -> Value {
    json!({
        "id": row["id"],
        "origin": "export",
        "label": row["name"],
        "scope": "read",
        "created_at": row["created_at"],
        // An export token has no expiry, was never "opened", and keeps no audit of its own.
        "expires_at": Value::Null,
        "opened_at": Value::Null,
        "revoked_at": row["revoked_at"],
        "last_seen_at": row["last_seen_at"],
        "first_write_at": Value::Null,
        "chats": Value::Null,
        "live": row["revoked_at"].is_null(),
        "state": if row["revoked_at"].is_null() { "in_use" } else { "revoked" },
        "note": "This token predates agent links: it never expires and keeps no activity record.",
    })
}

/// Live export reader tokens of an account, in the list's shape.
fn export_tokens_of(state: &AppState, user_id: &str) -> Vec<Value> {
    state
        .db
        .live_devices_of_user(user_id)
        .unwrap_or_default()
        .iter()
        .filter(|d| d["kind"] == READER_KIND)
        .map(export_token_json)
        .collect()
}

/// `GET /me/agent-links` — every agent token of the account: the links and the export reader
/// tokens that predate them. No bearer and no URL appears here, on any row, ever.
pub async fn list(State(state): State<AppState>, principal: Principal) -> Response {
    let Some(user_id) = device_user(&principal) else {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        );
    };
    let links = match state.db.agent_links_of_user(&user_id) {
        Ok(list) => list,
        Err(e) => return db_error("list links", e),
    };
    let mut rows: Vec<Value> = links.iter().map(link_json).collect();
    rows.extend(export_tokens_of(&state, &user_id));
    Json(rows).into_response()
}

/// `DELETE /me/agent-links/{id}` — revoke one of the caller's agent tokens, of either origin.
/// Immediate: the next request the holder makes is refused, because every request re-reads the
/// row rather than trusting a cached decision.
///
/// An export token is a `devices` row, so this route could be an oblique way to sign a real app
/// out. It cannot: the row must belong to the caller **and** its `kind` must be `reader`.
pub async fn revoke(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(user_id) = device_user(&principal) else {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        );
    };
    match state.db.agent_link(&id) {
        Ok(Some(link)) if link.user_id == user_id => {
            match state.db.revoke_agent_link(&id) {
                Ok(_) => {}
                Err(e) => return db_error("revoke link", e),
            }
            let _ = state.db.note_link_audit(
                &id,
                &user_id,
                "revoked",
                None,
                None,
                client_ip(&headers).as_deref(),
                user_agent(&headers).as_deref(),
                &json!({ "by": "owner" }),
            );
            tracing::info!(user = %user_id, link = %id, "agent link revoked");
            return StatusCode::NO_CONTENT.into_response();
        }
        // Somebody else's link looks like no link at all: no enumeration oracle.
        Ok(Some(_)) => return not_found(),
        Ok(None) => {}
        Err(e) => return db_error("look up link", e),
    }
    // Not a link: it may be one of the export reader tokens this list also owns.
    let is_reader = state
        .db
        .device_kind(&id)
        .ok()
        .flatten()
        .is_some_and(|k| k == READER_KIND);
    let owned = state.db.device_status(&id).ok().flatten();
    match owned {
        Some((owner, _)) if owner == user_id && is_reader => {
            match crate::signin::revoke_device_everywhere(&state, &id).await {
                Ok(_) => {
                    tracing::info!(user = %user_id, device = %id, "export reader token revoked");
                    StatusCode::NO_CONTENT.into_response()
                }
                Err(e) => db_error("revoke reader token", e),
            }
        }
        _ => not_found(),
    }
}

/// `GET /me/agent-links/{id}/activity?limit=&before=` — what this link did.
pub async fn activity(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Query(q): Query<ActivityQuery>,
) -> Response {
    let Some(user_id) = device_user(&principal) else {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        );
    };
    match state.db.agent_link(&id) {
        Ok(Some(link)) if link.user_id == user_id => {
            let limit = q.limit.unwrap_or(ACTIVITY_DEFAULT).clamp(1, ACTIVITY_MAX);
            match state.db.link_audit_rows(&id, q.before, limit) {
                Ok(rows) => {
                    Json(json!({ "link": link_json(&link), "events": rows })).into_response()
                }
                Err(e) => db_error("read activity", e),
            }
        }
        Ok(Some(_)) | Ok(None) => {
            // An export token is a real row of this account, and it honestly has no activity to
            // show: nothing records what a reader bearer read. Saying so beats an empty list that
            // reads as "it did nothing".
            let is_reader = state
                .db
                .device_kind(&id)
                .ok()
                .flatten()
                .is_some_and(|k| k == READER_KIND);
            let mine = state.db.device_status(&id).ok().flatten();
            match mine {
                Some((owner, _)) if owner == user_id && is_reader => Json(json!({
                    "events": [],
                    "note": "This token predates agent links. The relay kept no record of what \
                             it read; only links carry an activity log.",
                }))
                .into_response(),
                _ => not_found(),
            }
        }
        Err(e) => db_error("look up link for activity", e),
    }
}

#[derive(Deserialize, Default)]
pub struct ActivityQuery {
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub before: Option<i64>,
}

// ---- opening the link ------------------------------------------------------------------------------

/// The instruction sheet, with the canonical host rewritten to this relay's public URL and the
/// bearer substituted in. The asset itself holds no credential, which its test asserts.
const SHEET: &str = include_str!("../assets/agent-link-llms.txt");
const CANONICAL_RELAY: &str = "https://relay.arbos.life";
const TOKEN_PLACEHOLDER: &str = "@@TOKEN@@";

/// `GET /agent-link/open/{secret}` — the instruction sheet, once.
///
/// This is the only response in the relay that carries a link's bearer. It is single-use: the
/// first successful fetch spends the secret, and every later one is told the link was already
/// opened. That turns a leaked URL into either a dead string (if the agent got there first) or a
/// loud failure the owner can act on (if it did not), instead of a silently shared account.
pub async fn open_sheet(
    State(state): State<AppState>,
    Path(secret): Path<String>,
    headers: HeaderMap,
) -> Response {
    // One indexed lookup by hash, exactly as every other bearer in the relay is resolved. An
    // unknown secret and a spent one therefore cost the same work, so neither the answer nor the
    // time it took distinguishes "wrong secret" from "already used".
    let link = match state.db.agent_link_by_open_hash(&hash(&secret)) {
        Ok(Some(link)) => link,
        Ok(None) => return not_found(),
        Err(e) => return db_error("resolve open secret", e),
    };
    let now = now_ms();
    if link.revoked_at.is_some() || link.expires_at <= now {
        return api_error(
            StatusCode::GONE,
            "already_opened",
            "this link is no longer live; ask the owner for a new one",
        );
    }
    if link.open_expires_at <= now {
        return api_error(
            StatusCode::GONE,
            "open_expired",
            "this link was not opened within 15 minutes of being made; ask the owner for a new one",
        );
    }
    // The bearer is handed over only to the fetch that spends the secret. Two racing fetches
    // cannot both win: the update is the claim.
    let token = mint_token();
    match state.db.open_agent_link(&link.id, &hash(&token)) {
        Ok(true) => {}
        Ok(false) => {
            let _ = state.db.note_link_audit(
                &link.id,
                &link.user_id,
                "refused",
                None,
                None,
                client_ip(&headers).as_deref(),
                user_agent(&headers).as_deref(),
                &json!({ "why": "already_opened" }),
            );
            return api_error(
                StatusCode::GONE,
                "already_opened",
                "this link was already opened; ask the owner for a new one",
            );
        }
        Err(e) => return db_error("spend open secret", e),
    }
    let _ = state.db.note_link_audit(
        &link.id,
        &link.user_id,
        "opened",
        None,
        None,
        client_ip(&headers).as_deref(),
        user_agent(&headers).as_deref(),
        &json!({}),
    );
    tracing::info!(user = %link.user_id, link = %link.id, "agent link opened");

    let body = SHEET
        .replace(CANONICAL_RELAY, &relay_of(&state))
        .replace(TOKEN_PLACEHOLDER, &token);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

// ---- the agent's principal ---------------------------------------------------------------------------

/// An authenticated agent access link (bearer `arbl_…`). Only handlers in this module take one,
/// so a link token cannot reach a route that was never written for it: Axum refuses the request
/// before any handler body runs.
#[derive(Debug, Clone)]
pub struct LinkPrincipal {
    pub link: AgentLink,
}

impl FromRequestParts<AppState> for LinkPrincipal {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // The header and nowhere else (spec §3.2 deviation 1a). `auth::token_from_parts` — what
        // every other extractor in the relay uses, `outside::AgentPrincipal` included — falls
        // back to `?token=` on **every** route, not only on the WebSocket dials that need it.
        // Building on it here would undo the reason this feature chose SSE over a WebSocket in
        // the first place, and would put the bearer in relay logs, proxy logs, `Referer`
        // headers and shell history. So: `bearer_from_header`, and a token in the query string
        // is told where it belongs rather than quietly ignored.
        let Some(tok) = crate::auth::bearer_from_header(&parts.headers) else {
            return Err(if has_query_token(parts) {
                token_in_url()
            } else {
                invalid_token()
            });
        };
        if !tok.starts_with(TOKEN_PREFIX) {
            return Err(invalid_token());
        }
        let link = state
            .db
            .agent_link_by_token_hash(&hash(&tok))
            .ok()
            .flatten()
            .ok_or_else(invalid_token)?;
        if !link.live(now_ms()) {
            return Err(invalid_token());
        }
        if !state
            .db
            .rate_check_and_note(
                &format!("link:{}", link.id),
                "link",
                60_000,
                REQUESTS_PER_MIN,
            )
            .unwrap_or(true)
        {
            let key = format!("link:{}", link.id);
            return Err(rate_limited(state, &key, "link", 60_000));
        }
        let _ = state.db.touch_agent_link(&link.id);
        Ok(LinkPrincipal { link })
    }
}

/// `GET /agent-link/me` — who this link speaks for, what it may do, and when it dies.
pub async fn me(State(state): State<AppState>, who: LinkPrincipal) -> Response {
    let link = who.link;
    let card = state.db.user_card(&link.user_id).ok().flatten();
    Json(json!({
        "user": {
            "id": link.user_id,
            "handle": card.as_ref().map(|c| c.handle.clone()).unwrap_or_default(),
            "name": card.and_then(|c| c.name).unwrap_or_default(),
        },
        "scope": link.scope.as_str(),
        "label": link.label,
        "expires_at": link.expires_at,
        "chats": link.chats,
        "limits": { "requests_per_min": REQUESTS_PER_MIN },
    }))
    .into_response()
}

// ---- small shared pieces ---------------------------------------------------------------------------

fn device_user(principal: &Principal) -> Option<String> {
    match principal {
        Principal::Device { user_id, .. } => Some(user_id.clone()),
        Principal::Box { .. } | Principal::Admin => None,
    }
}

fn not_found() -> Response {
    api_error(StatusCode::NOT_FOUND, "not_found", "no such agent link")
}

fn invalid_token() -> Response {
    api_error(
        StatusCode::UNAUTHORIZED,
        "invalid_token",
        "this agent link is not valid; it may have been revoked or expired",
    )
}

/// A storage failure the caller can do nothing about. The real error goes to the log, where the
/// operator can read it; the caller is told only that the call failed. Review of #2460 found
/// rusqlite's text reaching clients on the devices route, which hands table and column names to
/// anyone who can provoke an error — and here the caller may be a stranger's agent.
fn db_error(what: &str, e: impl std::fmt::Display) -> Response {
    tracing::error!(error = %e, op = what, "agent link: storage failed");
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "db",
        "the relay could not complete that; try again",
    )
}

/// Does the request carry a `token=` query parameter? Only used to turn a silent 401 into one
/// that says what the caller did wrong — the value is never read and never resolved.
fn has_query_token(parts: &Parts) -> bool {
    parts
        .uri
        .query()
        .is_some_and(|q| q.split('&').any(|kv| kv.starts_with("token=")))
}

/// Refusal for a bearer sent in the URL. An agent that gets this has already leaked the token
/// into whatever logged the request, so the message tells it to mint a new link rather than
/// retry — the token it just sent should be treated as burnt.
fn token_in_url() -> Response {
    api_error(
        StatusCode::UNAUTHORIZED,
        "token_in_url",
        "send the token in an Authorization: Bearer header; a token in the query string is refused, and one that was sent there is in the logs now — ask the owner for a new link",
    )
}

fn rate_limited(state: &AppState, key: &str, kind: &str, window_ms: i64) -> Response {
    let retry = state.db.rate_retry_s(key, kind, window_ms).unwrap_or(60);
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, retry.to_string())],
        Json(json!({"error": {"type": "rate_limited", "message": "too many requests; wait and try again"}})),
    )
        .into_response()
}

/// The caller's address as the front proxy reported it. **Not authenticated**: nothing stops a
/// caller that reaches the relay directly from inventing the header, so an audit row's `ip` is
/// evidence about what was claimed, never proof of where a request came from.
fn client_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().chars().take(64).collect())
        .filter(|s: &String| !s.is_empty())
}

fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().chars().take(200).collect())
        .filter(|s: &String| !s.is_empty())
}

#[cfg(test)]
mod tests {
    //! The lifecycle, and the three properties the design rests on: the owner copies one thing
    //! and that thing dies on use, the bearer never travels in a URL, and a revoked link is
    //! refused on its very next call.

    use axum::body::to_bytes;
    use axum::extract::FromRequestParts;

    use super::*;
    use crate::auth::{resolve_checked, Refused};
    use crate::people::tests_support::person;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    async fn text_of(r: Response) -> (StatusCode, String) {
        let status = r.status();
        let bytes = to_bytes(r.into_body(), 1 << 22).await.expect("body");
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    async fn json_of(r: Response) -> (StatusCode, Value) {
        let (status, body) = text_of(r).await;
        (status, serde_json::from_str(&body).unwrap_or(Value::Null))
    }

    fn owner(state: &AppState, github_id: i64) -> (String, Principal) {
        person(state, github_id)
    }

    /// Mint a link and hand back `(row, the URL's secret)`.
    async fn mint_link(state: &AppState, who: &Principal, body: Value) -> (Value, String) {
        let b: MintBody = serde_json::from_value(body).expect("mint body");
        let r = mint(
            State(state.clone()),
            who.clone(),
            HeaderMap::new(),
            Some(Json(b)),
        )
        .await;
        let (status, row) = json_of(r).await;
        assert_eq!(status, StatusCode::CREATED, "mint: {row}");
        let url = row["url"].as_str().expect("a url").to_owned();
        let secret = url.rsplit('/').next().expect("a secret").to_owned();
        (row, secret)
    }

    /// Fetch the sheet and pull the bearer out of it, the way an agent would.
    async fn open_and_take_bearer(state: &AppState, secret: &str) -> String {
        let r = open_sheet(
            State(state.clone()),
            Path(secret.to_owned()),
            HeaderMap::new(),
        )
        .await;
        let (status, sheet) = text_of(r).await;
        assert_eq!(status, StatusCode::OK, "open: {sheet}");
        sheet
            .split_whitespace()
            .find(|w| w.starts_with(TOKEN_PREFIX))
            .map(|w| w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_'))
            .expect("the sheet carries the bearer")
            .to_owned()
    }

    fn parts(uri: &str, bearer: Option<&str>) -> Parts {
        let mut b = axum::http::Request::builder().uri(uri);
        if let Some(t) = bearer {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        b.body(()).expect("request").into_parts().0
    }

    /// Jacob's requirement: *"It should just be one thing you copy to give to your agent."*
    /// The owner copies a URL and is never handed a secret to carry; the agent collects its own.
    #[tokio::test]
    async fn the_owner_copies_one_url_and_never_sees_the_bearer() {
        let state = test_state(AuthMode::Github);
        let (user, who) = owner(&state, 5001);
        let (row, secret) = mint_link(&state, &who, json!({ "scope": "read" })).await;

        let minted = serde_json::to_string(&row).expect("json");
        assert!(
            !minted.contains(TOKEN_PREFIX),
            "the mint response carries no bearer: {minted}"
        );
        assert_eq!(row["state"], "waiting");
        assert!(
            state
                .db
                .agent_link_by_token_hash(&hash("anything"))
                .unwrap()
                .is_none()
                && state
                    .db
                    .with(|c| c.query_row(
                        "SELECT token_hash IS NULL FROM agent_links WHERE id = ?1",
                        params![row["id"].as_str().unwrap()],
                        |r| r.get::<_, bool>(0)
                    ))
                    .unwrap(),
            "no bearer exists anywhere until the link is opened"
        );

        let bearer = open_and_take_bearer(&state, &secret).await;
        assert!(bearer.starts_with(TOKEN_PREFIX));

        // And the owner's list never shows it, before or after.
        let (status, rows) = json_of(list(State(state.clone()), who.clone()).await).await;
        assert_eq!(status, StatusCode::OK);
        let listed = serde_json::to_string(&rows).expect("json");
        assert!(
            !listed.contains(TOKEN_PREFIX) && !listed.contains(&secret),
            "the list carries neither the bearer nor the open secret: {listed}"
        );
        assert_eq!(rows[0]["state"], "in_use");
        assert_eq!(rows[0]["user_hint"], Value::Null, "no stray fields");
        let _ = user;
    }

    /// The copied URL is worth one fetch. A leak after the agent used it is a leak of a dead
    /// string; a leak before it is a loud failure the owner can act on.
    #[tokio::test]
    async fn the_url_opens_once_and_the_second_fetch_says_somebody_got_there_first() {
        let state = test_state(AuthMode::Github);
        let (_user, who) = owner(&state, 5002);
        let (_row, secret) = mint_link(&state, &who, json!({ "scope": "read" })).await;

        let bearer = open_and_take_bearer(&state, &secret).await;

        let r = open_sheet(State(state.clone()), Path(secret.clone()), HeaderMap::new()).await;
        let (status, body) = json_of(r).await;
        assert_eq!(status, StatusCode::GONE);
        assert_eq!(body["error"]["type"], "already_opened");

        // The first fetch's bearer is unharmed: the second fetch is refused, not a revocation.
        let mut p = parts("/agent-link/me", Some(&bearer));
        assert!(LinkPrincipal::from_request_parts(&mut p, &state)
            .await
            .is_ok());

        // An unknown secret is a 404, and a spent one is a 410: "wrong" and "used" are
        // different answers, which is the whole point of keeping `open_hash` after the open.
        let r = open_sheet(
            State(state.clone()),
            Path("not-a-real-secret".to_owned()),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    /// Spec §3.2 deviation 1a. `auth::token_from_parts` would accept this, which is why the
    /// extractor does not use it: SSE was chosen over a WebSocket precisely to keep the bearer
    /// out of URLs, and that argument is worthless if the relay reads one from a query string.
    #[tokio::test]
    async fn a_bearer_in_the_query_string_is_refused_and_told_why() {
        let state = test_state(AuthMode::Github);
        let (_user, who) = owner(&state, 5003);
        let (_row, secret) = mint_link(&state, &who, json!({ "scope": "read" })).await;
        let bearer = open_and_take_bearer(&state, &secret).await;

        let mut p = parts(&format!("/agent-link/me?token={bearer}"), None);
        let refused = LinkPrincipal::from_request_parts(&mut p, &state)
            .await
            .expect_err("a token in the url is refused");
        let (status, body) = json_of(refused).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            body["error"]["type"], "token_in_url",
            "the agent is told where the token belongs, not just that it failed"
        );

        // The same token in the header is fine, and a query string alongside it changes nothing.
        let mut p = parts("/agent-link/me?chat=abc", Some(&bearer));
        assert!(LinkPrincipal::from_request_parts(&mut p, &state)
            .await
            .is_ok());
    }

    /// No cache, no JWT: the row is read on every call, so revoking is immediate rather than
    /// eventual. Same for an expiry that has passed.
    #[tokio::test]
    async fn a_revoked_or_expired_link_is_refused_on_its_very_next_call() {
        let state = test_state(AuthMode::Github);
        let (_user, who) = owner(&state, 5004);
        let (row, secret) = mint_link(&state, &who, json!({ "scope": "reply" })).await;
        let id = row["id"].as_str().expect("id").to_owned();
        let bearer = open_and_take_bearer(&state, &secret).await;

        let mut p = parts("/agent-link/me", Some(&bearer));
        assert!(LinkPrincipal::from_request_parts(&mut p, &state)
            .await
            .is_ok());

        let r = revoke(
            State(state.clone()),
            who.clone(),
            Path(id.clone()),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);

        let mut p = parts("/agent-link/me", Some(&bearer));
        assert!(
            LinkPrincipal::from_request_parts(&mut p, &state)
                .await
                .is_err(),
            "the next call after a revoke is refused"
        );

        // Expiry is the same check, from the other side.
        let (row2, secret2) = mint_link(&state, &who, json!({ "scope": "read" })).await;
        let id2 = row2["id"].as_str().expect("id").to_owned();
        let bearer2 = open_and_take_bearer(&state, &secret2).await;
        state
            .db
            .with(|c| {
                c.execute(
                    "UPDATE agent_links SET expires_at = ?2 WHERE id = ?1",
                    params![id2, now_ms() - 1],
                )
            })
            .expect("expire");
        let mut p = parts("/agent-link/me", Some(&bearer2));
        assert!(LinkPrincipal::from_request_parts(&mut p, &state)
            .await
            .is_err());
    }

    /// Default-deny by construction: the link token is not a `Principal` at all, so a route
    /// that does not take a `LinkPrincipal` cannot be reached with one. No allowlist to keep
    /// in step with the route table.
    #[tokio::test]
    async fn a_link_bearer_is_not_a_principal_and_reaches_no_ordinary_route() {
        let state = test_state(AuthMode::Github);
        let (_user, who) = owner(&state, 5005);
        let (_row, secret) = mint_link(&state, &who, json!({ "scope": "reply" })).await;
        let bearer = open_and_take_bearer(&state, &secret).await;

        assert_eq!(
            resolve_checked(&state, &bearer).unwrap_err(),
            Refused::Invalid,
            "an arbl_ token is in neither devices.token_hash nor boxes.token_hash"
        );
    }

    /// Two relay-side ideas both mean "read-only agent token". The list is one place and the
    /// revoke is one path, and the older kind is labelled rather than disguised.
    #[tokio::test]
    async fn export_reader_tokens_are_listed_and_revoked_here_without_pretending_to_be_links() {
        let state = test_state(AuthMode::Github);
        let (user, who) = owner(&state, 5006);
        let reader_id = ulid();
        let reader_token = format!("adv_{}", ulid());
        state
            .db
            .insert_device(
                &reader_id,
                "Grok Bot",
                &hash(&reader_token),
                &user,
                READER_KIND,
            )
            .expect("reader");
        let (_row, _secret) = mint_link(&state, &who, json!({ "scope": "read" })).await;

        let (_status, rows) = json_of(list(State(state.clone()), who.clone()).await).await;
        let rows = rows.as_array().expect("a list");
        assert_eq!(rows.len(), 2);
        let export = rows
            .iter()
            .find(|r| r["origin"] == "export")
            .expect("the reader token is listed");
        assert_eq!(export["label"], "Grok Bot");
        assert_eq!(export["scope"], "read");
        assert_eq!(export["expires_at"], Value::Null, "it never expires");
        assert!(
            export["note"]
                .as_str()
                .unwrap_or_default()
                .contains("predates"),
            "the row says what it is rather than passing as a link"
        );

        // Its activity is honestly empty rather than a lie by omission.
        let (status, act) = json_of(
            activity(
                State(state.clone()),
                who.clone(),
                Path(reader_id.clone()),
                Query(ActivityQuery::default()),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(act["note"]
            .as_str()
            .unwrap_or_default()
            .contains("kept no record"));

        // And revoking it here works, through the same route.
        let r = revoke(
            State(state.clone()),
            who.clone(),
            Path(reader_id.clone()),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            resolve_checked(&state, &reader_token).unwrap_err(),
            Refused::Invalid
        );
    }

    /// The revoke route reaches `devices` rows, so it has to be unable to sign a real app out.
    #[tokio::test]
    async fn the_revoke_route_cannot_be_used_to_sign_a_real_device_out() {
        let state = test_state(AuthMode::Github);
        let (user, who) = owner(&state, 5007);
        let mac_id = ulid();
        let mac_token = format!("adv_{}", ulid());
        state
            .db
            .insert_device(&mac_id, "Alice's Mac", &hash(&mac_token), &user, "github")
            .expect("device");

        let r = revoke(
            State(state.clone()),
            who.clone(),
            Path(mac_id.clone()),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(
            r.status(),
            StatusCode::NOT_FOUND,
            "an ordinary device is not an agent token, whoever owns it"
        );
        assert!(resolve_checked(&state, &mac_token).is_ok());
    }

    /// Somebody else's link answers like a link that does not exist, so the route cannot be
    /// used to find out which ids are real.
    #[tokio::test]
    async fn another_account_cannot_see_revoke_or_probe_a_link() {
        let state = test_state(AuthMode::Github);
        let (_alice, alice) = owner(&state, 5008);
        let (_bob, bob) = owner(&state, 5009);
        let (row, _secret) = mint_link(&state, &alice, json!({ "scope": "read" })).await;
        let id = row["id"].as_str().expect("id").to_owned();

        for r in [
            revoke(
                State(state.clone()),
                bob.clone(),
                Path(id.clone()),
                HeaderMap::new(),
            )
            .await,
            activity(
                State(state.clone()),
                bob.clone(),
                Path(id.clone()),
                Query(ActivityQuery::default()),
            )
            .await,
        ] {
            assert_eq!(r.status(), StatusCode::NOT_FOUND);
        }
        assert!(
            state.db.agent_link(&id).unwrap().unwrap().live(now_ms()),
            "Alice's link is untouched"
        );
        let (_status, rows) = json_of(list(State(state.clone()), bob).await).await;
        assert_eq!(rows.as_array().expect("a list").len(), 0);
    }

    /// A stolen device must not be able to mint a fleet of links and spread a read budget
    /// across them, and a malformed ask must not produce a link that reads as "no filter".
    #[tokio::test]
    async fn minting_is_capped_and_a_bad_shape_is_refused() {
        let state = test_state(AuthMode::Github);
        let (_user, who) = owner(&state, 5010);

        for _ in 0..LIVE_LINKS_MAX {
            mint_link(&state, &who, json!({ "scope": "read" })).await;
        }
        let over = mint(
            State(state.clone()),
            who.clone(),
            HeaderMap::new(),
            Some(Json(
                serde_json::from_value(json!({ "scope": "read" })).unwrap(),
            )),
        )
        .await;
        let (status, body) = json_of(over).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["type"], "too_many_links");

        let bad = [
            (json!({ "scope": "admin" }), "bad_scope"),
            (json!({ "scope": "read", "ttl_hours": 0 }), "bad_ttl"),
            (json!({ "scope": "read", "ttl_hours": 24 * 90 }), "bad_ttl"),
            (json!({ "scope": "read", "chats": [] }), "bad_chats"),
        ];
        for (body, want) in bad {
            let r = mint(
                State(state.clone()),
                who.clone(),
                HeaderMap::new(),
                Some(Json(serde_json::from_value(body.clone()).unwrap())),
            )
            .await;
            let (status, got) = json_of(r).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(got["error"]["type"], want, "{body}");
        }
    }

    /// A link is a grant the account made to a third party, so signing out everywhere has to
    /// take it with the sessions — that requirement is why #2460 landed first.
    #[tokio::test]
    async fn signing_out_everywhere_takes_the_links_too() {
        let state = test_state(AuthMode::Github);
        let (user, who) = owner(&state, 5011);
        let device_token = format!("adv_{}", ulid());
        let Principal::Device { id: device_id, .. } = &who else {
            unreachable!("person() hands back a device")
        };
        state
            .db
            .with(|c| {
                c.execute(
                    "UPDATE devices SET token_hash = ?2 WHERE id = ?1",
                    params![device_id, hash(&device_token)],
                )
            })
            .expect("re-token");
        let (row, secret) = mint_link(&state, &who, json!({ "scope": "reply" })).await;
        let id = row["id"].as_str().expect("id").to_owned();
        let bearer = open_and_take_bearer(&state, &secret).await;

        let caller = resolve_checked(&state, &device_token).expect("sign in");
        let r = crate::signin::sign_out_everywhere(
            State(state.clone()),
            crate::auth::HeaderPrincipal(caller),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);

        let mut p = parts("/agent-link/me", Some(&bearer));
        assert!(
            LinkPrincipal::from_request_parts(&mut p, &state)
                .await
                .is_err(),
            "the link died with the sessions"
        );
        assert!(state
            .db
            .agent_link(&id)
            .unwrap()
            .unwrap()
            .revoked_at
            .is_some());
        let audit = state.db.link_audit_rows(&id, None, 50).expect("audit");
        assert!(
            audit
                .iter()
                .any(|e| e["action"] == "revoked" && e["detail"]["by"] == "sign_out_everywhere"),
            "the audit says who closed it: {audit:?}"
        );
        let _ = user;
    }

    /// The asset in the repository holds no credential, and the sheet the agent reads holds
    /// exactly one — with the canonical host rewritten to whichever relay served it.
    #[tokio::test]
    async fn the_instruction_sheet_carries_the_bearer_and_the_asset_does_not() {
        assert!(
            !SHEET.contains(TOKEN_PREFIX),
            "the checked-in sheet carries no token"
        );
        assert!(
            SHEET.contains(TOKEN_PLACEHOLDER),
            "the sheet has somewhere to put one"
        );

        let state = test_state(AuthMode::Github);
        let (_user, who) = owner(&state, 5012);
        let (_row, secret) = mint_link(&state, &who, json!({ "scope": "read" })).await;
        let r = open_sheet(State(state.clone()), Path(secret), HeaderMap::new()).await;
        assert_eq!(
            r.headers()
                .get(header::CACHE_CONTROL)
                .map(|v| v.to_str().unwrap()),
            Some("no-store"),
            "the one response carrying a bearer is never cached"
        );
        let (_status, sheet) = text_of(r).await;
        assert!(
            !sheet.contains(TOKEN_PLACEHOLDER),
            "the placeholder is filled"
        );
        assert!(
            !sheet.contains(CANONICAL_RELAY) && sheet.contains("http://127.0.0.1:7707"),
            "the sheet names the relay that served it"
        );
        assert_eq!(
            sheet.matches(TOKEN_PREFIX).count(),
            1,
            "the bearer appears once, where the agent is told to store it"
        );
    }

    /// A mint over plain HTTP would hand a long-lived key to an agent over an unencrypted
    /// connection. Loopback is exempt so a self-hoster can develop.
    #[tokio::test]
    async fn a_plain_http_relay_will_not_mint_a_link() {
        assert!(transport_is_safe("https://relay.arbos.life"));
        assert!(transport_is_safe("http://127.0.0.1:7707"));
        assert!(transport_is_safe("http://localhost:7707"));
        assert!(!transport_is_safe("http://relay.example.com"));

        let mut state = test_state(AuthMode::Github);
        let (_user, who) = owner(&state, 5013);
        let mut cfg = (*state.cfg).clone();
        cfg.public_url = "http://relay.example.com".into();
        state.cfg = std::sync::Arc::new(cfg);
        let r = mint(
            State(state.clone()),
            who,
            HeaderMap::new(),
            Some(Json(
                serde_json::from_value(json!({ "scope": "read" })).unwrap(),
            )),
        )
        .await;
        let (status, body) = json_of(r).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["type"], "insecure_transport");
    }

    /// Only the owner's own signed-in device mints, lists and revokes. A box serves the
    /// account's projects; it does not get to hand the account out.
    #[tokio::test]
    async fn a_box_cannot_mint_list_or_revoke() {
        let state = test_state(AuthMode::Github);
        let (user, _who) = owner(&state, 5014);
        let as_box = Principal::Box {
            user_id: user.clone(),
        };
        for r in [
            mint(
                State(state.clone()),
                as_box.clone(),
                HeaderMap::new(),
                Some(Json(
                    serde_json::from_value(json!({ "scope": "read" })).unwrap(),
                )),
            )
            .await,
            list(State(state.clone()), as_box.clone()).await,
            revoke(State(state.clone()), as_box, Path(ulid()), HeaderMap::new()).await,
        ] {
            assert_eq!(r.status(), StatusCode::FORBIDDEN);
        }
    }

    /// Every endpoint shares one per-minute budget, keyed by link, so an agent cannot spread a
    /// flood across routes.
    #[tokio::test]
    async fn one_link_shares_one_request_budget_across_every_endpoint() {
        let state = test_state(AuthMode::Github);
        let (_user, who) = owner(&state, 5015);
        let (_row, secret) = mint_link(&state, &who, json!({ "scope": "read" })).await;
        let bearer = open_and_take_bearer(&state, &secret).await;

        for i in 0..REQUESTS_PER_MIN {
            let mut p = parts("/agent-link/me", Some(&bearer));
            assert!(
                LinkPrincipal::from_request_parts(&mut p, &state)
                    .await
                    .is_ok(),
                "call {i} is within budget"
            );
        }
        let mut p = parts("/agent-link/me", Some(&bearer));
        let refused = LinkPrincipal::from_request_parts(&mut p, &state)
            .await
            .expect_err("over budget");
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(
            refused.headers().contains_key(header::RETRY_AFTER),
            "an agent is told how long to wait"
        );
    }
}
