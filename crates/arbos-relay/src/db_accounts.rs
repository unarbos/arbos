//! Storage for self-serve accounts: GitHub users, sign-in flows, refresh tokens, the credit
//! ledger, and the kill switch. Same SQLite file as `db.rs`;
//! these tables are created from `Db::open` through `ACCOUNTS_SCHEMA`.
//!
//! Money is USD as REAL. A ledger row is positive for a grant and negative for a debit; the
//! balance is the sum over every kind except `machine`. `debit` refuses to take a user below zero
//! unless the user has `allow_negative` (the admin override), which is the "ledger never
//! negative" invariant. Machine time (`kind = 'machine'`) is a separate, informational bucket:
//! machines are always on (Jacob, 2026-10-15), so their cost never gates inference, never
//! consumes the grant, and never counts toward a user's daily cap.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use arbos_core::ids::now_ms;

use crate::db::Db;

pub const ACCOUNTS_SCHEMA: &str = r#"
CREATE UNIQUE INDEX IF NOT EXISTS users_github ON users(github_id) WHERE github_id IS NOT NULL;
CREATE TABLE IF NOT EXISTS auth_flows (
  state TEXT PRIMARY KEY, code_verifier TEXT NOT NULL, redirect TEXT NOT NULL,
  device_name TEXT NOT NULL, invite TEXT, created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS auth_grants (
  code_hash TEXT PRIMARY KEY, device_id TEXT NOT NULL, user_id TEXT NOT NULL,
  created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, used_at INTEGER
);
CREATE TABLE IF NOT EXISTS refresh_tokens (
  token_hash TEXT PRIMARY KEY, device_id TEXT NOT NULL, user_id TEXT NOT NULL,
  created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, rotated_at INTEGER, revoked_at INTEGER
);
CREATE INDEX IF NOT EXISTS refresh_tokens_device ON refresh_tokens(device_id);
CREATE TABLE IF NOT EXISTS credit_ledger (
  id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, at INTEGER NOT NULL,
  kind TEXT NOT NULL, usd REAL NOT NULL, ref TEXT, note TEXT, actor TEXT NOT NULL DEFAULT 'relay'
);
CREATE INDEX IF NOT EXISTS credit_ledger_user_at ON credit_ledger(user_id, at);
CREATE INDEX IF NOT EXISTS credit_ledger_at ON credit_ledger(at);
CREATE TABLE IF NOT EXISTS box_history (
  sandbox_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, snapshot TEXT, replaced_at INTEGER NOT NULL,
  delete_after INTEGER NOT NULL, deleted_at INTEGER, row_json TEXT, archived_at INTEGER,
  partial INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS machine_meter (
  user_id TEXT PRIMARY KEY, last_tick_at INTEGER NOT NULL, last_state TEXT NOT NULL
);
"#;

/// Additive columns (SQLite has no `ADD COLUMN IF NOT EXISTS`; failures mean "already there").
pub fn migrate_accounts(c: &Connection) {
    for stmt in [
        "ALTER TABLE users ADD COLUMN github_id INTEGER",
        "ALTER TABLE users ADD COLUMN login TEXT",
        "ALTER TABLE users ADD COLUMN avatar_url TEXT",
        "ALTER TABLE users ADD COLUMN name TEXT",
        "ALTER TABLE users ADD COLUMN last_login_at INTEGER",
        "ALTER TABLE users ADD COLUMN cap_unlimited INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE users ADD COLUMN allow_negative INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE devices ADD COLUMN kind TEXT NOT NULL DEFAULT 'paircode'",
        "ALTER TABLE users ADD COLUMN test INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE boxes ADD COLUMN held_stopped INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE box_history ADD COLUMN row_json TEXT",
        "ALTER TABLE box_history ADD COLUMN archived_at INTEGER",
        // 1 = the new box of a rolled-back upgrade (half-carried /work): kept so the sweeper
        // deletes it after the window, never re-adopted by a repair.
        "ALTER TABLE box_history ADD COLUMN partial INTEGER NOT NULL DEFAULT 0",
        // Who may add this person to a people group: anyone | people_i_know | nobody.
        "ALTER TABLE users ADD COLUMN group_add TEXT NOT NULL DEFAULT 'anyone'",
        // Where `name` came from: `github` (seeded from the profile at first sign-in) or `user`
        // (set through PATCH /me). Only a user-set name is how agents address the person.
        "ALTER TABLE users ADD COLUMN name_source TEXT NOT NULL DEFAULT 'github'",
        // Earlier display names (JSON array), newest last: the box rewrites memory that named
        // the owner by one of them to "the owner".
        "ALTER TABLE users ADD COLUMN former_names TEXT NOT NULL DEFAULT '[]'",
        // Per-user box provider (admin, `PUT /admin/users/{id}/box {"provider"}`): where this
        // user's NEXT box is created when it differs from ARBOS_DEFAULT_PROVIDER — scratch Daytona
        // boxes for carry tests on a gVisor-default host (ARB-339). NULL = the relay default.
        "ALTER TABLE users ADD COLUMN box_provider TEXT",
    ] {
        let _ = c.execute(stmt, []);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub github_id: Option<i64>,
    pub login: Option<String>,
    pub avatar_url: Option<String>,
    pub name: Option<String>,
    pub created_at: i64,
    pub last_login_at: Option<i64>,
    /// Daily debit cap in USD; `None` = unlimited (admin).
    pub daily_cap_usd: Option<f64>,
    pub allow_negative: bool,
    /// Made by `POST /admin/users/test`; no GitHub account behind it; deletable.
    pub test: bool,
}

/// `(login, profile name, address name, former names)` — see `Db::user_address`.
pub type UserAddress = (Option<String>, Option<String>, Option<String>, Vec<String>);

/// What GitHub tells us about the signed-in account.
#[derive(Debug, Clone)]
pub struct GithubIdentity {
    pub id: i64,
    pub login: String,
    pub avatar_url: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuthFlow {
    pub state: String,
    pub code_verifier: String,
    pub redirect: String,
    pub device_name: String,
    pub invite: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RefreshRow {
    pub device_id: String,
    pub user_id: String,
    pub expires_at: i64,
    pub rotated_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Credits {
    pub balance_usd: f64,
    pub granted_usd: f64,
    pub spent_usd: f64,
    pub spent_today_usd: f64,
    pub daily_cap_usd: Option<f64>,
    pub allow_negative: bool,
    /// Machine time (always-on box) — informational, outside the balance and caps.
    pub machine_today_usd: f64,
    pub machine_total_usd: f64,
}

/// Why a debit was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum DebitRefused {
    /// Balance would go below zero and the user has no override.
    Insufficient { balance_usd: f64, needed_usd: f64 },
    /// Today's debits already reach the user's daily cap.
    DailyCap { cap_usd: f64, spent_today_usd: f64 },
    /// The global kill switch is on.
    ReadOnly { reason: String },
}

impl std::fmt::Display for DebitRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DebitRefused::Insufficient {
                balance_usd,
                needed_usd,
            } => write!(
                f,
                "credits exhausted: balance ${balance_usd:.4}, needed ${needed_usd:.4}"
            ),
            DebitRefused::DailyCap {
                cap_usd,
                spent_today_usd,
            } => write!(
                f,
                "daily cap ${cap_usd:.2} reached (spent ${spent_today_usd:.2} today)"
            ),
            DebitRefused::ReadOnly { reason } => write!(f, "read-only: {reason}"),
        }
    }
}

/// An old sandbox kept after an upgrade, with the `boxes` row it had.
#[derive(Debug, Clone)]
pub struct RetainedBox {
    pub sandbox_id: String,
    pub snapshot: Option<String>,
    pub row: Option<crate::db::Sandbox>,
}

/// Kill switch record kept in `meta` under `kill_switch`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KillSwitch {
    pub on: bool,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub by: Option<String>,
    #[serde(default)]
    pub at: i64,
    /// `true` when tripped by the global daily cap; it clears at the next UTC day.
    #[serde(default)]
    pub auto: bool,
}

const KILL_SWITCH_KEY: &str = "kill_switch";

fn user_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    let cap: f64 = r.get(7)?;
    let unlimited: i64 = r.get(8)?;
    Ok(User {
        id: r.get(0)?,
        github_id: r.get(1)?,
        login: r.get(2)?,
        avatar_url: r.get(3)?,
        name: r.get(4)?,
        created_at: r.get(5)?,
        last_login_at: r.get(6)?,
        daily_cap_usd: if unlimited != 0 { None } else { Some(cap) },
        allow_negative: r.get::<_, i64>(9)? != 0,
        test: r.get::<_, i64>(10)? != 0,
    })
}

const USER_COLS: &str = "id, github_id, login, avatar_url, name, created_at, last_login_at, spend_cap_usd, cap_unlimited, allow_negative, test";

impl Db {
    // ---- users --------------------------------------------------------------------------------

    pub fn user(&self, id: &str) -> Result<Option<User>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {USER_COLS} FROM users WHERE id = ?1"),
                params![id],
                user_from_row,
            )
            .optional()
        })
    }

    pub fn user_by_github_id(&self, github_id: i64) -> Result<Option<User>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {USER_COLS} FROM users WHERE github_id = ?1"),
                params![github_id],
                user_from_row,
            )
            .optional()
        })
    }

    /// Insert or refresh the user behind a GitHub account. Returns `(user, created)`; the id is
    /// `gh_<github id>`, so one GitHub account is one user (and one machine) forever.
    pub fn upsert_github_user(
        &self,
        gh: &GithubIdentity,
        default_daily_cap_usd: f64,
    ) -> Result<(User, bool)> {
        let id = format!("gh_{}", gh.id);
        let now = now_ms();
        let created = self.with(|c| {
            let existing: Option<String> = c
                .query_row(
                    "SELECT status FROM users WHERE github_id = ?1",
                    params![gh.id],
                    |r| r.get(0),
                )
                .optional()?;
            match existing.as_deref() {
                Some(status) => {
                    // An `invited` placeholder (someone opened a DM with this login before it
                    // ever signed in) becomes a member now: first sign-in, with the welcome grant.
                    let first = status == "invited";
                    // Keep a picture the user uploaded via PUT /me/avatar (URL path `/avatars/…`).
                    let existing_avatar: Option<String> = c
                        .query_row(
                            "SELECT avatar_url FROM users WHERE github_id = ?1",
                            params![gh.id],
                            |r| r.get(0),
                        )
                        .optional()?
                        .flatten();
                    let avatar = if existing_avatar
                        .as_deref()
                        .is_some_and(|u| u.contains("/avatars/"))
                    {
                        existing_avatar
                    } else {
                        gh.avatar_url.clone()
                    };
                    // Keep name/handle (PATCH /me or the invited placeholder); only
                    // refresh avatar (unless custom) and last_login_at.
                    c.execute(
                        "UPDATE users SET avatar_url = ?2, last_login_at = ?3, status = 'member',
                            spend_cap_usd = CASE WHEN status = 'invited' THEN ?4 ELSE spend_cap_usd END
                         WHERE github_id = ?1",
                        params![gh.id, avatar, now, default_daily_cap_usd],
                    )?;
                    Ok(first)
                }
                None => {
                    c.execute(
                        "INSERT INTO users(id, snapshot, spend_cap_usd, created_at, github_id, login, avatar_url, name, last_login_at, status)
                         VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?3, 'member')",
                        params![id, default_daily_cap_usd, now, gh.id, gh.login, gh.avatar_url, gh.name],
                    )?;
                    Ok(true)
                }
            }
        })?;
        let user = self
            .user_by_github_id(gh.id)?
            .ok_or_else(|| anyhow!("user vanished"))?;
        Ok((user, created))
    }

    pub fn list_users(&self) -> Result<Vec<User>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {USER_COLS} FROM users ORDER BY created_at"
            ))?;
            let rows = st.query_map([], user_from_row)?;
            rows.collect()
        })
    }

    /// A test account (no GitHub behind it): `test_<ulid>`, status member, flagged `test`.
    pub fn insert_test_user(
        &self,
        id: &str,
        handle: &str,
        name: Option<&str>,
        avatar_url: Option<&str>,
        daily_cap_usd: f64,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO users(id, snapshot, spend_cap_usd, created_at, login, avatar_url, name, last_login_at, status, test)
                 VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?3, 'member', 1)",
                params![id, daily_cap_usd, now_ms(), handle, avatar_url, name],
            )
            .map(|_| ())
        })
    }

    /// Remove a test user and everything keyed to it (devices, tokens, ledger, memberships,
    /// pushes). Projects are left for the caller to refuse on. Returns false for a non-test user.
    pub fn delete_test_user(&self, id: &str) -> Result<bool> {
        self.with(|c| {
            let is_test: i64 = c
                .query_row("SELECT test FROM users WHERE id = ?1", params![id], |r| {
                    r.get(0)
                })
                .optional()?
                .unwrap_or(0);
            if is_test == 0 {
                return Ok(false);
            }
            c.execute("DELETE FROM refresh_tokens WHERE user_id = ?1", params![id])?;
            c.execute("DELETE FROM auth_grants WHERE user_id = ?1", params![id])?;
            c.execute("DELETE FROM devices WHERE user_id = ?1", params![id])?;
            c.execute("DELETE FROM credit_ledger WHERE user_id = ?1", params![id])?;
            c.execute(
                "DELETE FROM people_members WHERE member = ?1",
                params![format!("user:{id}")],
            )?;
            c.execute(
                "DELETE FROM project_members WHERE user_id = ?1",
                params![id],
            )?;
            c.execute("DELETE FROM user_pushes WHERE user_id = ?1", params![id])?;
            c.execute(
                "DELETE FROM blocks WHERE blocker = ?1 OR blocked = ?1",
                params![id],
            )?;
            c.execute("DELETE FROM machine_meter WHERE user_id = ?1", params![id])?;
            c.execute("DELETE FROM vault_items WHERE user_id = ?1", params![id])?;
            c.execute(
                "DELETE FROM secret_versions WHERE secret_id IN (SELECT id FROM secrets WHERE user_id = ?1)",
                params![id],
            )?;
            for table in [
                "secrets",
                "secret_access",
                "secret_requests",
                "user_keys",
                "user_secrets_settings",
                "user_pins",
            ] {
                c.execute(
                    &format!("DELETE FROM {table} WHERE user_id = ?1"),
                    params![id],
                )?;
            }
            c.execute("DELETE FROM users WHERE id = ?1", params![id])?;
            Ok(true)
        })
    }

    /// Custom profile picture URL (`PUT /me/avatar`); shown everywhere as `avatar_url`.
    pub fn set_user_avatar_url(&self, user_id: &str, avatar_url: &str) -> Result<()> {
        let n = self.with(|c| {
            c.execute(
                "UPDATE users SET avatar_url = ?2 WHERE id = ?1",
                params![user_id, avatar_url],
            )
        })?;
        if n == 0 {
            return Err(anyhow!("no such user"));
        }
        Ok(())
    }

    /// Back to the GitHub picture (or none): `DELETE /me/avatar`.
    pub fn clear_user_avatar_url(
        &self,
        user_id: &str,
        github_fallback: Option<&str>,
    ) -> Result<()> {
        let n = self.with(|c| {
            c.execute(
                "UPDATE users SET avatar_url = ?2 WHERE id = ?1",
                params![user_id, github_fallback],
            )
        })?;
        if n == 0 {
            return Err(anyhow!("no such user"));
        }
        Ok(())
    }

    /// Who may add this person to a people group (`anyone` | `people_i_know` | `nobody`).
    /// Missing column / missing row → `anyone` (today's open default).
    pub fn group_add_policy(&self, user_id: &str) -> Result<String> {
        self.with(|c| {
            c.query_row(
                "SELECT COALESCE(group_add, 'anyone') FROM users WHERE id = ?1",
                params![user_id],
                |r| r.get(0),
            )
            .optional()
            .map(|v| v.unwrap_or_else(|| "anyone".into()))
        })
    }

    /// `PATCH /me {group_add}` — store the group-add privacy choice.
    pub fn set_group_add_policy(&self, user_id: &str, policy: &str) -> Result<()> {
        let n = self.with(|c| {
            c.execute(
                "UPDATE users SET group_add = ?2 WHERE id = ?1",
                params![user_id, policy],
            )
        })?;
        if n == 0 {
            return Err(anyhow!("no such user"));
        }
        Ok(())
    }

    /// How agents address a person: `(login, name, address_name)`. `address_name` is the name the
    /// user set themselves (PATCH /me); a name seeded from the GitHub profile is a label, not an
    /// address — "Const" / "Arbos" were GitHub profile names, never how Jacob wanted to be called.
    pub fn user_address(&self, user_id: &str) -> Result<Option<UserAddress>> {
        self.with(|c| {
            c.query_row(
                "SELECT login, name, name_source, former_names FROM users WHERE id = ?1",
                params![user_id],
                |r| {
                    let login: Option<String> = r.get(0)?;
                    let name: Option<String> = r.get(1)?;
                    let source: String = r.get(2)?;
                    let former: String = r.get(3)?;
                    let former: Vec<String> = serde_json::from_str(&former).unwrap_or_default();
                    let address = if source == "user" { name.clone() } else { None };
                    Ok((login, name, address, former))
                },
            )
            .optional()
        })
    }

    /// Display name and/or handle (`PATCH /me`). `login` must already be validated and unique.
    /// `None` for a field leaves it unchanged; empty `name` stores NULL (clients show `@handle`).
    pub fn set_user_profile(
        &self,
        user_id: &str,
        name: Option<&str>,
        login: Option<&str>,
    ) -> Result<()> {
        if name.is_none() && login.is_none() {
            return Ok(());
        }
        let name_sql: Option<Option<&str>> =
            name.map(|s| if s.is_empty() { None } else { Some(s) });
        if let Some(new_name) = name_sql {
            // The old name joins `former_names` so boxes can retire it from memory.
            let _ = self.with(|c| {
                let prev: Option<(Option<String>, String)> = c
                    .query_row(
                        "SELECT name, former_names FROM users WHERE id = ?1",
                        params![user_id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                if let Some((Some(old), former)) = prev {
                    let old = old.trim().to_owned();
                    if !old.is_empty() && Some(old.as_str()) != new_name {
                        let mut v: Vec<String> = serde_json::from_str(&former).unwrap_or_default();
                        if !v.iter().any(|x| x == &old) {
                            v.push(old);
                        }
                        c.execute(
                            "UPDATE users SET former_names = ?2 WHERE id = ?1",
                            params![
                                user_id,
                                serde_json::to_string(&v).unwrap_or_else(|_| "[]".into())
                            ],
                        )?;
                    }
                }
                Ok(())
            });
        }
        let n = self.with(|c| match (name_sql, login) {
            (Some(name), Some(login)) => c.execute(
                "UPDATE users SET name = ?2, login = ?3, name_source = 'user' WHERE id = ?1",
                params![user_id, name, login],
            ),
            (Some(name), None) => c.execute(
                "UPDATE users SET name = ?2, name_source = 'user' WHERE id = ?1",
                params![user_id, name],
            ),
            (None, Some(login)) => c.execute(
                "UPDATE users SET login = ?2 WHERE id = ?1",
                params![user_id, login],
            ),
            (None, None) => Ok(0),
        })?;
        if n == 0 {
            return Err(anyhow!("no such user"));
        }
        Ok(())
    }

    /// Admin: daily cap (`None` = unlimited) and the negative-balance override.
    pub fn set_user_limits(
        &self,
        user_id: &str,
        daily_cap_usd: Option<Option<f64>>,
        allow_negative: Option<bool>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO users(id, created_at) VALUES (?1, ?2)",
                params![user_id, now_ms()],
            )?;
            if let Some(cap) = daily_cap_usd {
                match cap {
                    Some(v) => c.execute(
                        "UPDATE users SET spend_cap_usd = ?2, cap_unlimited = 0 WHERE id = ?1",
                        params![user_id, v],
                    )?,
                    None => c.execute(
                        "UPDATE users SET cap_unlimited = 1 WHERE id = ?1",
                        params![user_id],
                    )?,
                };
            }
            if let Some(neg) = allow_negative {
                c.execute(
                    "UPDATE users SET allow_negative = ?2 WHERE id = ?1",
                    params![user_id, neg as i64],
                )?;
            }
            Ok(())
        })
    }

    // ---- sign-in flows and grants -------------------------------------------------------------

    pub fn insert_auth_flow(&self, f: &AuthFlow, ttl_ms: i64) -> Result<()> {
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "DELETE FROM auth_flows WHERE expires_at < ?1",
                params![now],
            )?;
            c.execute(
                "INSERT INTO auth_flows(state, code_verifier, redirect, device_name, invite, created_at, expires_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![f.state, f.code_verifier, f.redirect, f.device_name, f.invite, now, now + ttl_ms],
            )
            .map(|_| ())
        })
    }

    /// Consume a flow by state (one use).
    pub fn take_auth_flow(&self, state: &str) -> Result<Option<AuthFlow>> {
        let now = now_ms();
        self.with(|c| {
            let row = c
                .query_row(
                    "SELECT state, code_verifier, redirect, device_name, invite FROM auth_flows WHERE state = ?1 AND expires_at > ?2",
                    params![state, now],
                    |r| {
                        Ok(AuthFlow {
                            state: r.get(0)?,
                            code_verifier: r.get(1)?,
                            redirect: r.get(2)?,
                            device_name: r.get(3)?,
                            invite: r.get(4)?,
                        })
                    },
                )
                .optional()?;
            c.execute("DELETE FROM auth_flows WHERE state = ?1", params![state])?;
            Ok(row)
        })
    }

    /// A signed-in device row (no long-lived token; the JWT carries the id).
    pub fn insert_device(
        &self,
        device_id: &str,
        name: &str,
        token_hash: &str,
        user_id: &str,
        kind: &str,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO devices(id, name, token_hash, created_at, user_id, kind) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![device_id, name, token_hash, now_ms(), user_id, kind],
            )
            .map(|_| ())
        })
    }

    /// `kind` of a device row (`github`, `paircode`, `reader`, …).
    pub fn device_kind(&self, device_id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT kind FROM devices WHERE id = ?1",
                params![device_id],
                |r| r.get(0),
            )
            .optional()
        })
    }

    /// Device row as `(user_id, revoked)`; `None` when the id is unknown.
    pub fn device_status(&self, device_id: &str) -> Result<Option<(String, bool)>> {
        self.with(|c| {
            c.query_row(
                "SELECT user_id, revoked_at IS NOT NULL FROM devices WHERE id = ?1",
                params![device_id],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
            )
            .optional()
        })
    }

    pub fn touch_device(&self, device_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE devices SET last_seen_at = ?2 WHERE id = ?1",
                params![device_id, now_ms()],
            )
            .map(|_| ())
        })
    }

    /// When any of the user's live devices last talked to the relay (HTTP call or presence).
    pub fn user_last_seen(&self, user_id: &str) -> Result<Option<i64>> {
        self.with(|c| {
            c.query_row(
                "SELECT MAX(last_seen_at) FROM devices WHERE user_id = ?1 AND revoked_at IS NULL",
                params![user_id],
                |r| r.get::<_, Option<i64>>(0),
            )
        })
    }

    /// Every device row of a user, signed-out ones included (admin and debug views).
    pub fn devices_of_user(&self, user_id: &str) -> Result<Vec<serde_json::Value>> {
        self.devices_of_user_filtered(user_id, true)
    }

    /// The user's signed-in devices only (`revoked_at IS NULL`) — what Settings → Devices shows.
    pub fn live_devices_of_user(&self, user_id: &str) -> Result<Vec<serde_json::Value>> {
        self.devices_of_user_filtered(user_id, false)
    }

    fn devices_of_user_filtered(
        &self,
        user_id: &str,
        include_revoked: bool,
    ) -> Result<Vec<serde_json::Value>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, name, created_at, revoked_at, last_seen_at, kind FROM devices
                 WHERE user_id = ?1 AND (?2 OR revoked_at IS NULL) ORDER BY created_at",
            )?;
            let rows = st.query_map(params![user_id, include_revoked], |r| {
                Ok(serde_json::json!({
                    "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "created_at": r.get::<_, i64>(2)?,
                    "revoked_at": r.get::<_, Option<i64>>(3)?, "last_seen_at": r.get::<_, Option<i64>>(4)?,
                    "kind": r.get::<_, String>(5)?,
                }))
            })?;
            rows.collect()
        })
    }

    /// Revoke every live GitHub device of this user that shares `name`, except
    /// `keep_id` when set. Used on re-login and when collapsing reinstalls so
    /// Settings → Devices stays one row per machine name. Returns the revoked ids.
    pub fn revoke_named_github_devices(
        &self,
        user_id: &str,
        name: &str,
        keep_id: Option<&str>,
    ) -> Result<Vec<String>> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(Vec::new());
        }
        let live = self.live_devices_of_user(user_id)?;
        let mut revoked = Vec::new();
        for d in live {
            let kind = d.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            if kind != "github" {
                continue;
            }
            let id = match d.get("id").and_then(|v| v.as_str()) {
                Some(id) => id,
                None => continue,
            };
            if keep_id == Some(id) {
                continue;
            }
            let dname = d.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            if dname != name {
                continue;
            }
            if self.revoke_device_fully(id)? {
                revoked.push(id.to_owned());
            }
        }
        Ok(revoked)
    }

    /// Collapse same-name live GitHub sessions: keep `current_id` when it is in
    /// the group, otherwise the most recently seen (else newest created); revoke
    /// the rest. Reinstalls and re-logins used to leave a row per attempt
    /// (Affinity, 2026-10-28: 3 stale iPhone rows beside the real one).
    pub fn collapse_github_device_duplicates(
        &self,
        user_id: &str,
        current_id: &str,
    ) -> Result<Vec<String>> {
        let live = self.live_devices_of_user(user_id)?;
        let mut by_name: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
        for d in live {
            if d.get("kind").and_then(|v| v.as_str()) != Some("github") {
                continue;
            }
            let name = d
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_owned();
            if name.is_empty() {
                continue;
            }
            by_name.entry(name).or_default().push(d);
        }
        let mut revoked = Vec::new();
        for group in by_name.values() {
            if group.len() < 2 {
                continue;
            }
            let keep = group
                .iter()
                .find(|d| d.get("id").and_then(|v| v.as_str()) == Some(current_id))
                .or_else(|| {
                    group.iter().max_by_key(|d| {
                        d.get("last_seen_at")
                            .and_then(|v| v.as_i64())
                            .or_else(|| d.get("created_at").and_then(|v| v.as_i64()))
                            .unwrap_or(0)
                    })
                });
            let keep_id = keep.and_then(|d| d.get("id").and_then(|v| v.as_str()));
            for d in group {
                let id = match d.get("id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => continue,
                };
                if Some(id) == keep_id {
                    continue;
                }
                if self.revoke_device_fully(id)? {
                    revoked.push(id.to_owned());
                }
            }
        }
        Ok(revoked)
    }

    /// Device ids of a user, most recently seen first — revoked ones included, since a row they
    /// wrote keeps its `deviceId` for good. Capped so a roster stays small.
    pub fn device_ids_of_user(&self, user_id: &str, limit: usize) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id FROM devices WHERE user_id = ?1
                 ORDER BY COALESCE(last_seen_at, created_at) DESC LIMIT ?2",
            )?;
            let rows = st.query_map(params![user_id, limit as i64], |r| r.get(0))?;
            rows.collect()
        })
    }

    pub fn insert_auth_grant(
        &self,
        code_hash: &str,
        device_id: &str,
        user_id: &str,
        ttl_ms: i64,
    ) -> Result<()> {
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "DELETE FROM auth_grants WHERE expires_at < ?1",
                params![now - 3_600_000],
            )?;
            c.execute(
                "INSERT INTO auth_grants(code_hash, device_id, user_id, created_at, expires_at) VALUES (?1,?2,?3,?4,?5)",
                params![code_hash, device_id, user_id, now, now + ttl_ms],
            )
            .map(|_| ())
        })
    }

    /// Redeem a grant code once: `(device_id, user_id)`.
    pub fn take_auth_grant(&self, code_hash: &str) -> Result<Option<(String, String)>> {
        let now = now_ms();
        self.with(|c| {
            let n = c.execute(
                "UPDATE auth_grants SET used_at = ?2 WHERE code_hash = ?1 AND used_at IS NULL AND expires_at > ?2",
                params![code_hash, now],
            )?;
            if n == 0 {
                return Ok(None);
            }
            c.query_row(
                "SELECT device_id, user_id FROM auth_grants WHERE code_hash = ?1",
                params![code_hash],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
    }

    // ---- refresh tokens -----------------------------------------------------------------------

    pub fn insert_refresh(
        &self,
        token_hash: &str,
        device_id: &str,
        user_id: &str,
        ttl_ms: i64,
    ) -> Result<()> {
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "INSERT INTO refresh_tokens(token_hash, device_id, user_id, created_at, expires_at) VALUES (?1,?2,?3,?4,?5)",
                params![token_hash, device_id, user_id, now, now + ttl_ms],
            )
            .map(|_| ())
        })
    }

    pub fn refresh_row(&self, token_hash: &str) -> Result<Option<RefreshRow>> {
        self.with(|c| {
            c.query_row(
                "SELECT device_id, user_id, expires_at, rotated_at, revoked_at FROM refresh_tokens WHERE token_hash = ?1",
                params![token_hash],
                |r| {
                    Ok(RefreshRow {
                        device_id: r.get(0)?,
                        user_id: r.get(1)?,
                        expires_at: r.get(2)?,
                        rotated_at: r.get(3)?,
                        revoked_at: r.get(4)?,
                    })
                },
            )
            .optional()
        })
    }

    /// Atomically claim a refresh-token row for rotation.
    ///
    /// Returns `true` when this call set `rotated_at` (the claimer must issue the new pair).
    /// Returns `false` when the row was already rotated — another refresh claimed it, or this
    /// token is being re-presented after a restart.
    pub fn mark_refresh_rotated(&self, token_hash: &str) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE refresh_tokens SET rotated_at = ?2 WHERE token_hash = ?1 AND rotated_at IS NULL",
                params![token_hash, now_ms()],
            )
            .map(|n| n > 0)
        })
    }

    /// Revoke a device and every refresh token it holds (logout, admin revoke, token reuse).
    pub fn revoke_device_fully(&self, device_id: &str) -> Result<bool> {
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "UPDATE refresh_tokens SET revoked_at = ?2 WHERE device_id = ?1 AND revoked_at IS NULL",
                params![device_id, now],
            )?;
            c.execute(
                "UPDATE devices SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
                params![device_id, now],
            )
            .map(|n| n > 0)
        })
    }

    // ---- credit ledger ------------------------------------------------------------------------

    pub fn grant_credits(&self, user_id: &str, usd: f64, note: &str, actor: &str) -> Result<()> {
        if !usd.is_finite() || usd <= 0.0 {
            return Err(anyhow!("grant must be a positive amount"));
        }
        self.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO users(id, created_at) VALUES (?1, ?2)",
                params![user_id, now_ms()],
            )?;
            c.execute(
                "INSERT INTO credit_ledger(user_id, at, kind, usd, note, actor) VALUES (?1, ?2, 'grant', ?3, ?4, ?5)",
                params![user_id, now_ms(), usd, note, actor],
            )
            .map(|_| ())
        })
    }

    /// Debits across every user since `since_ms` (the global cap).
    pub fn debited_since_all(&self, since_ms: i64) -> Result<f64> {
        self.with(|c| {
            c.query_row(
                "SELECT COALESCE(-SUM(usd), 0.0) FROM credit_ledger WHERE usd < 0 AND kind != 'machine' AND at >= ?1",
                params![since_ms],
                |r| r.get(0),
            )
        })
    }

    /// Take `usd` from the user, atomically checked against the balance (unless the user has
    /// the negative override) and the daily cap. The kill switch is checked by the caller
    /// (`credits::check`), because it is relay-wide, not per user.
    pub fn debit(
        &self,
        user_id: &str,
        usd: f64,
        kind: &str,
        reference: Option<&str>,
        start_of_day_ms: i64,
    ) -> Result<std::result::Result<f64, DebitRefused>> {
        if !usd.is_finite() || usd < 0.0 {
            return Err(anyhow!("debit must be a non-negative amount"));
        }
        self.with(|c| {
            let (cap, unlimited, allow_negative): (f64, i64, i64) = c
                .query_row(
                    "SELECT spend_cap_usd, cap_unlimited, allow_negative FROM users WHERE id = ?1",
                    params![user_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?
                .unwrap_or((0.0, 0, 0));
            let balance: f64 = c.query_row(
                "SELECT COALESCE(SUM(usd), 0.0) FROM credit_ledger WHERE user_id = ?1 AND kind != 'machine'",
                params![user_id],
                |r| r.get(0),
            )?;
            if allow_negative == 0 && balance - usd < -1e-9 {
                return Ok(Err(DebitRefused::Insufficient {
                    balance_usd: balance,
                    needed_usd: usd,
                }));
            }
            if unlimited == 0 {
                let today: f64 = c.query_row(
                    "SELECT COALESCE(-SUM(usd), 0.0) FROM credit_ledger WHERE user_id = ?1 AND usd < 0 AND kind != 'machine' AND at >= ?2",
                    params![user_id, start_of_day_ms],
                    |r| r.get(0),
                )?;
                if today >= cap {
                    return Ok(Err(DebitRefused::DailyCap {
                        cap_usd: cap,
                        spent_today_usd: today,
                    }));
                }
            }
            if usd > 0.0 {
                c.execute(
                    "INSERT INTO credit_ledger(user_id, at, kind, usd, ref) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![user_id, now_ms(), kind, -usd, reference],
                )?;
            }
            Ok(Ok(balance - usd))
        })
    }

    pub fn credits(&self, user_id: &str, start_of_day_ms: i64) -> Result<Credits> {
        let user = self.user(user_id)?;
        self.with(|c| {
            let (granted, spent): (f64, f64) = c.query_row(
                "SELECT COALESCE(SUM(CASE WHEN usd > 0 THEN usd ELSE 0 END), 0.0),
                        COALESCE(SUM(CASE WHEN usd < 0 THEN -usd ELSE 0 END), 0.0)
                 FROM credit_ledger WHERE user_id = ?1 AND kind != 'machine'",
                params![user_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let today: f64 = c.query_row(
                "SELECT COALESCE(-SUM(usd), 0.0) FROM credit_ledger WHERE user_id = ?1 AND usd < 0 AND kind != 'machine' AND at >= ?2",
                params![user_id, start_of_day_ms],
                |r| r.get(0),
            )?;
            let (machine_total, machine_today): (f64, f64) = c.query_row(
                "SELECT COALESCE(-SUM(usd), 0.0), COALESCE(-SUM(CASE WHEN at >= ?2 THEN usd ELSE 0 END), 0.0)
                 FROM credit_ledger WHERE user_id = ?1 AND kind = 'machine'",
                params![user_id, start_of_day_ms],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            Ok(Credits {
                balance_usd: granted - spent,
                granted_usd: granted,
                spent_usd: spent,
                spent_today_usd: today,
                daily_cap_usd: user.as_ref().and_then(|u| u.daily_cap_usd),
                allow_negative: user.as_ref().is_some_and(|u| u.allow_negative),
                machine_today_usd: machine_today,
                machine_total_usd: machine_total,
            })
        })
    }

    /// Machine time: recorded, never gated, never counted against grant or caps.
    pub fn record_machine_cost(
        &self,
        user_id: &str,
        usd: f64,
        reference: Option<&str>,
    ) -> Result<()> {
        if !usd.is_finite() || usd <= 0.0 {
            return Ok(());
        }
        self.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO users(id, created_at) VALUES (?1, ?2)",
                params![user_id, now_ms()],
            )?;
            c.execute(
                "INSERT INTO credit_ledger(user_id, at, kind, usd, ref) VALUES (?1, ?2, 'machine', ?3, ?4)",
                params![user_id, now_ms(), -usd, reference],
            )
            .map(|_| ())
        })
    }

    /// Relay-wide machine cost since `since_ms` (informational alert threshold).
    pub fn machine_cost_since_all(&self, since_ms: i64) -> Result<f64> {
        self.with(|c| {
            c.query_row(
                "SELECT COALESCE(-SUM(usd), 0.0) FROM credit_ledger WHERE kind = 'machine' AND at >= ?1",
                params![since_ms],
                |r| r.get(0),
            )
        })
    }

    // ---- kill switch --------------------------------------------------------------------------

    pub fn kill_switch(&self) -> Result<KillSwitch> {
        Ok(self
            .meta_get(KILL_SWITCH_KEY)?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default())
    }

    pub fn set_kill_switch(&self, ks: &KillSwitch) -> Result<()> {
        self.meta_set(KILL_SWITCH_KEY, &serde_json::to_string(ks)?)
    }

    // ---- always-on: a box the user stopped by hand stays stopped until they start it ----------

    pub fn set_box_held(&self, user_id: &str, held: bool) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE boxes SET held_stopped = ?2 WHERE user_id = ?1",
                params![user_id, held as i64],
            )
            .map(|_| ())
        })
    }

    pub fn box_held(&self, user_id: &str) -> Result<bool> {
        self.with(|c| {
            c.query_row(
                "SELECT held_stopped FROM boxes WHERE user_id = ?1",
                params![user_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map(|o| o.unwrap_or(0) != 0)
        })
    }

    /// Users who should have a running machine: everyone with a box row, plus every signed-in
    /// member without one yet.
    pub fn always_on_users(&self) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare(
                // A box row without a users row is a phantom (a project id minted as a user,
                // 2026-10-22); the sweep must never breathe life into it (#1708 follow-up).
                "SELECT b.user_id FROM boxes b JOIN users u ON u.id = b.user_id WHERE b.held_stopped = 0
                 UNION SELECT id FROM users WHERE status = 'member' AND test = 0 AND id NOT IN (SELECT user_id FROM boxes)",
            )?;
            let rows = st.query_map([], |r| r.get(0))?;
            rows.collect()
        })
    }

    // ---- box history (old sandboxes kept as fallback after an upgrade) -------------------------

    /// Keep an old sandbox as a fallback after an upgrade; `row_json` is the full `boxes` row at
    /// that time (token hash, mcp token, device id, urls) so it can be re-adopted.
    pub fn insert_box_history(&self, row: &crate::db::Sandbox, delete_after: i64) -> Result<()> {
        let sandbox_id = row.sandbox_id.clone().unwrap_or_default();
        self.with(|c| {
            c.execute(
                "INSERT INTO box_history(sandbox_id, user_id, snapshot, replaced_at, delete_after, row_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(sandbox_id) DO UPDATE SET delete_after = excluded.delete_after, deleted_at = NULL, row_json = excluded.row_json",
                params![sandbox_id, row.user_id, row.snapshot, now_ms(), delete_after, serde_json::to_string(row).unwrap_or_default()],
            )
            .map(|_| ())
        })
    }

    /// Record the new sandbox of a rolled-back upgrade so the history sweeper deletes it after
    /// `delete_after` (retrying until Daytona agrees) — never left running unrecorded. It holds a
    /// half-carried `/work`, so [`Db::latest_box_history`] never offers it for repair.
    pub fn insert_partial_box_history(
        &self,
        row: &crate::db::Sandbox,
        delete_after: i64,
    ) -> Result<()> {
        let sandbox_id = row.sandbox_id.clone().unwrap_or_default();
        self.with(|c| {
            c.execute(
                "INSERT INTO box_history(sandbox_id, user_id, snapshot, replaced_at, delete_after, row_json, partial) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)
                 ON CONFLICT(sandbox_id) DO UPDATE SET delete_after = excluded.delete_after, deleted_at = NULL, row_json = excluded.row_json, partial = 1",
                params![sandbox_id, row.user_id, row.snapshot, now_ms(), delete_after, serde_json::to_string(row).unwrap_or_default()],
            )
            .map(|_| ())
        })
    }

    #[cfg(test)]
    pub fn box_history_entry(&self, sandbox_id: &str) -> Result<Option<(i64, bool)>> {
        self.with(|c| {
            c.query_row(
                "SELECT delete_after, partial FROM box_history WHERE sandbox_id = ?1 AND deleted_at IS NULL",
                params![sandbox_id],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? == 1)),
            )
            .optional()
        })
    }

    #[cfg(test)]
    pub fn insert_box_history_at(
        &self,
        user_id: &str,
        sandbox_id: &str,
        replaced_at: i64,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO box_history(sandbox_id, user_id, snapshot, replaced_at, delete_after) VALUES (?1, ?2, NULL, ?3, ?3 + 1000)",
                params![sandbox_id, user_id, replaced_at],
            )
            .map(|_| ())
        })
    }

    /// Forget a history entry (the upgrade that made it was rolled back).
    pub fn remove_box_history(&self, sandbox_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM box_history WHERE sandbox_id = ?1",
                params![sandbox_id],
            )
            .map(|_| ())
        })
    }

    /// The newest retained sandbox of a user that has not been deleted: `(sandbox_id, snapshot, row)`.
    pub fn latest_box_history(&self, user_id: &str) -> Result<Option<RetainedBox>> {
        self.with(|c| {
            c.query_row(
                "SELECT sandbox_id, snapshot, row_json FROM box_history WHERE user_id = ?1 AND deleted_at IS NULL AND partial = 0 ORDER BY replaced_at DESC LIMIT 1",
                params![user_id],
                |r| {
                    let row: Option<String> = r.get(2)?;
                    Ok(RetainedBox {
                        sandbox_id: r.get(0)?,
                        snapshot: r.get(1)?,
                        row: row.and_then(|j| serde_json::from_str(&j).ok()),
                    })
                },
            )
            .optional()
        })
    }

    /// `(user_id, sandbox_id)` of old sandboxes past their fallback window. A sandbox a box row
    /// names again (an upgrade cut short after the old box went into history) is never due.
    pub fn box_history_due(&self) -> Result<Vec<(String, String)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT user_id, sandbox_id FROM box_history WHERE deleted_at IS NULL AND delete_after <= ?1
                   AND sandbox_id NOT IN (SELECT sandbox_id FROM boxes WHERE sandbox_id IS NOT NULL)",
            )?;
            let rows = st.query_map(params![now_ms()], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
    }

    /// Retained sandboxes to Daytona-archive now: per user, every live (not deleted, not yet archived)
    /// history row beyond the `keep` newest by `replaced_at`. The newest `keep` stay unarchived so a
    /// rollback / repair restarts in seconds; the rest sit archived (cold, no disk quota) until their
    /// fallback window ends. 2026-10-22: 308 retained 10 GiB sandboxes filled the org's 5000 GiB quota.
    pub fn box_history_to_archive(
        &self,
        user_id: Option<&str>,
        keep: usize,
    ) -> Result<Vec<(String, String)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT user_id, sandbox_id FROM (
                   SELECT user_id, sandbox_id,
                          ROW_NUMBER() OVER (PARTITION BY user_id ORDER BY replaced_at DESC, sandbox_id DESC) AS rn
                   FROM box_history WHERE deleted_at IS NULL AND archived_at IS NULL
                     AND (?1 IS NULL OR user_id = ?1)
                     AND sandbox_id NOT IN (SELECT sandbox_id FROM boxes WHERE sandbox_id IS NOT NULL)
                 ) WHERE rn > ?2 ORDER BY user_id, rn",
            )?;
            let rows = st.query_map(params![user_id, keep as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
    }

    pub fn mark_box_history_archived(&self, sandbox_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE box_history SET archived_at = ?2 WHERE sandbox_id = ?1",
                params![sandbox_id, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn mark_box_history_deleted(&self, sandbox_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE box_history SET deleted_at = ?2 WHERE sandbox_id = ?1",
                params![sandbox_id, now_ms()],
            )
            .map(|_| ())
        })
    }

    // ---- machine metering ---------------------------------------------------------------------

    /// Last metering tick for a user's box: `(at_ms, state)`.
    pub fn meter_tick(&self, user_id: &str) -> Result<Option<(i64, String)>> {
        self.with(|c| {
            c.query_row(
                "SELECT last_tick_at, last_state FROM machine_meter WHERE user_id = ?1",
                params![user_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
    }

    pub fn set_meter_tick(&self, user_id: &str, at: i64, state: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO machine_meter(user_id, last_tick_at, last_state) VALUES (?1, ?2, ?3)
                 ON CONFLICT(user_id) DO UPDATE SET last_tick_at = excluded.last_tick_at, last_state = excluded.last_state",
                params![user_id, at, state],
            )
            .map(|_| ())
        })
    }
}

#[cfg(test)]
mod box_history_archive_tests {
    use crate::db::Db;

    fn temp_db() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "arbos-boxhist-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    /// Invariant: per user, the 2 newest retained sandboxes stay unarchived (fast rollback); older ones
    /// are archived; archived and deleted rows are never returned again.
    #[test]
    fn keeps_two_newest_per_user_and_archives_the_rest() {
        let dir = temp_db();
        let db = Db::open(&dir.join("relay.db")).expect("db");
        for (i, id) in ["a-old1", "a-old2", "a-new1", "a-new2"].iter().enumerate() {
            db.insert_box_history_at("gh_a", id, 1000 + i as i64)
                .unwrap();
        }
        db.insert_box_history_at("gh_b", "b-only", 5000).unwrap();
        db.insert_box_history_at("test_x", "t-old", 100).unwrap();
        db.insert_box_history_at("test_x", "t-mid", 200).unwrap();
        db.insert_box_history_at("test_x", "t-new", 300).unwrap();

        let all = db.box_history_to_archive(None, 2).unwrap();
        assert_eq!(
            all,
            vec![
                ("gh_a".into(), "a-old2".into()),
                ("gh_a".into(), "a-old1".into()),
                ("test_x".into(), "t-old".into())
            ]
        );

        let only_a = db.box_history_to_archive(Some("gh_a"), 2).unwrap();
        assert_eq!(only_a.len(), 2);
        assert!(only_a.iter().all(|(u, _)| u == "gh_a"));

        db.mark_box_history_archived("a-old1").unwrap();
        db.mark_box_history_archived("a-old2").unwrap();
        db.mark_box_history_deleted("t-old").unwrap();
        assert!(db.box_history_to_archive(None, 2).unwrap().is_empty());

        // A new upgrade pushes one of the kept two out: it becomes the next to archive.
        db.insert_box_history_at("gh_a", "a-new3", 9000).unwrap();
        assert_eq!(
            db.box_history_to_archive(Some("gh_a"), 2).unwrap(),
            vec![("gh_a".into(), "a-new1".into())]
        );
    }
}

#[cfg(test)]
mod address_name_tests {
    use super::GithubIdentity;
    use crate::db::Db;

    /// P0 Bubble "Hello, Const" (2026-10-26): the name GitHub seeded at first sign-in is a label,
    /// not how the person is addressed. Only a name set through PATCH /me is the address, and
    /// a later GitHub sign-in must not overwrite it.
    #[test]
    fn only_a_user_set_name_is_the_address() {
        let dir = std::env::temp_dir().join(format!(
            "arbos-addr-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&dir.join("relay.db")).expect("db");
        let gh = GithubIdentity {
            id: 32490803,
            login: "const".into(),
            avatar_url: None,
            name: Some("Const".into()),
        };
        let (u, created) = db.upsert_github_user(&gh, 20.0).unwrap();
        assert!(created);
        let (login, name, address, former) = db.user_address(&u.id).unwrap().unwrap();
        assert!(former.is_empty());
        assert_eq!(login.as_deref(), Some("const"));
        assert_eq!(
            name.as_deref(),
            Some("Const"),
            "the GitHub name is kept as a label"
        );
        assert_eq!(address, None, "but it is not an address");
        db.set_user_profile(&u.id, Some("Jacob"), None).unwrap();
        let (_, name, address, former) = db.user_address(&u.id).unwrap().unwrap();
        assert_eq!(name.as_deref(), Some("Jacob"));
        assert_eq!(address.as_deref(), Some("Jacob"));
        assert_eq!(
            former,
            vec!["Const".to_owned()],
            "the old label is remembered for the boxes"
        );
        // Another GitHub sign-in keeps the user's choice.
        let (_, _) = db.upsert_github_user(&gh, 20.0).unwrap();
        let (_, _, address, _) = db.user_address(&u.id).unwrap().unwrap();
        assert_eq!(address.as_deref(), Some("Jacob"));
        // Clearing the name clears the address too; the source flips back only on a new name.
        db.set_user_profile(&u.id, Some(""), None).unwrap();
        let (_, name, _, former) = db.user_address(&u.id).unwrap().unwrap();
        assert_eq!(name, None);
        assert_eq!(former, vec!["Const".to_owned(), "Jacob".to_owned()]);
        assert!(db.user_address("gh_nobody").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod device_dedupe_tests {
    //! Settings → Devices must collapse reinstall/re-login duplicates of the same
    //! GitHub device name (Affinity, 2026-10-28). Live-only listing is covered by
    //! `signin::devices_list_tests`.

    use super::GithubIdentity;
    use crate::db::Db;
    use arbos_core::ids::ulid;

    fn open() -> (std::path::PathBuf, Db) {
        let dir = std::env::temp_dir().join(format!(
            "arbos-devices-dedupe-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&dir.join("relay.db")).expect("db");
        (dir, db)
    }

    fn user(db: &Db, github_id: i64) -> String {
        db.upsert_github_user(
            &GithubIdentity {
                id: github_id,
                login: format!("u{github_id}"),
                avatar_url: None,
                name: None,
            },
            20.0,
        )
        .unwrap()
        .0
        .id
    }

    #[test]
    fn reinstall_revokes_the_same_named_github_session() {
        let (dir, db) = open();
        let alice = user(&db, 703);
        let old = ulid();
        let phone = ulid();
        db.insert_device(&old, "iPhone", &ulid(), &alice, "github")
            .unwrap();
        db.insert_device(&phone, "Other Phone", &ulid(), &alice, "github")
            .unwrap();
        let revoked = db
            .revoke_named_github_devices(&alice, "iPhone", None)
            .unwrap();
        assert_eq!(revoked, vec![old.clone()]);
        assert!(db.device_status(&old).unwrap().unwrap().1);
        assert!(!db.device_status(&phone).unwrap().unwrap().1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn collapse_keeps_current_or_most_recent_per_name() {
        let (dir, db) = open();
        let alice = user(&db, 704);
        let stale = ulid();
        let newer = ulid();
        let other = ulid();
        db.insert_device(&stale, "Mac (Mac)", &ulid(), &alice, "github")
            .unwrap();
        db.insert_device(&newer, "Mac (Mac)", &ulid(), &alice, "github")
            .unwrap();
        db.insert_device(&other, "iPhone", &ulid(), &alice, "github")
            .unwrap();
        db.touch_device(&newer).unwrap();

        let revoked = db
            .collapse_github_device_duplicates(&alice, &other)
            .unwrap();
        assert_eq!(revoked, vec![stale.clone()]);
        let live = db.live_devices_of_user(&alice).unwrap();
        let ids: Vec<&str> = live.iter().filter_map(|d| d["id"].as_str()).collect();
        assert!(ids.contains(&newer.as_str()));
        assert!(ids.contains(&other.as_str()));
        assert!(!ids.contains(&stale.as_str()));

        // Current wins even when another same-name row was seen more recently.
        let again = ulid();
        db.insert_device(&again, "iPhone", &ulid(), &alice, "github")
            .unwrap();
        db.touch_device(&again).unwrap();
        let revoked = db
            .collapse_github_device_duplicates(&alice, &other)
            .unwrap();
        assert_eq!(revoked, vec![again]);
        assert!(!db.device_status(&other).unwrap().unwrap().1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn readers_are_not_collapsed_with_github_sign_ins() {
        let (dir, db) = open();
        let alice = user(&db, 705);
        let gh = ulid();
        let reader = ulid();
        db.insert_device(&gh, "GrokBot-Arbos", &ulid(), &alice, "github")
            .unwrap();
        db.insert_device(&reader, "GrokBot-Arbos", &ulid(), &alice, "reader")
            .unwrap();
        let revoked = db.collapse_github_device_duplicates(&alice, &gh).unwrap();
        assert!(revoked.is_empty());
        assert_eq!(db.live_devices_of_user(&alice).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
