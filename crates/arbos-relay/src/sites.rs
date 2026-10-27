//! Agent-hosted sites (`arbos.life/<prefix>/<app>/`, design #2060): the relay is the registry and
//! the origin of truth; the edge (Cloudflare R2 + one Worker) serves. No box and no relay host
//! is in the serving path.
//!
//! Publish, from the box (bearer = box token, so the owner is the token's user — never a body
//! field): `POST /sites/{app}/begin` with the file list → the hashes still missing;
//! `PUT /sites/{app}/blob/{sha}` raw bytes for each; `POST /sites/{app}/commit` → the version
//! is recorded, pushed to R2 when configured (`ARBOS_SITES_R2_*`), audited, and the public URL
//! returned. The box then reads the site back over the public route before it tells anyone.
//!
//! Owner: `GET /sites`, `DELETE /sites/{app}`, `DELETE /sites?all=1` (device or box token).
//! Admin: `GET /admin/sites`, `POST /admin/sites/{user}/{app}/takedown|restore`,
//! `DELETE /admin/sites/{user}/{app}` (purge), `GET /admin/sites/events`,
//! `PUT /admin/sites/quota/{user}`. `arbosctl admin sites …` wraps these.
//!
//! Blobs are content-addressed (`<data>/sites/blobs/<aa>/<sha>`; R2 `blobs/<sha>`), so a
//! republish ships only what changed and identical files are stored once for everyone. The edge
//! reads `sites/<prefix>/<app>/manifest.json` (path → sha, content type, status).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use hmac::{Hmac, Mac};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use arbos_core::ids::now_ms;
use arbos_core::sites::{
    check_app, check_files, check_html, check_path, check_subdomain, subdomain_for_handle,
    ManifestEntry, Route, Scheme, SiteFile, SiteManifest, DEFAULT_MAX_SITES,
    DEFAULT_MAX_USER_BYTES, MAX_FILES, MAX_FILE_BYTES, MAX_PUBLISHES_PER_DAY,
    MAX_PUBLISHES_PER_HOUR, MAX_SITE_BYTES,
};

use crate::auth::Principal;
use crate::signin::api_error;
use crate::state::AppState;

/// A begin→commit session that saw no commit within this long is dropped.
const PENDING_TTL_MS: i64 = 60 * 60 * 1000;
/// After an explicit prefix move the old name 301s to the new one, and stays unclaimable, this long.
const MOVE_REDIRECT_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// Operator switches, read once from the environment.
pub struct SitesCfg {
    /// URL layout + apex (`ARBOS_SITES_SCHEME` = subdomain | path | two-level, default
    /// `subdomain` — Jacob 2026-10-27; `ARBOS_SITES_DOMAIN`, default the host of `ARBOS_SITE_URL`).
    pub route: Route,
    /// `ARBOS_SITES_PUBLISH=0` refuses new publishes (sites stay up).
    pub publish_enabled: bool,
    /// Shown on refusal pages and in `list_sites` so users know where reports go.
    pub abuse_email: Option<String>,
    pub r2: Option<S3Target>,
}

/// An S3-compatible bucket (Cloudflare R2) reached with SigV4.
#[derive(Clone)]
pub struct S3Target {
    pub endpoint: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
}

fn env_nonempty(k: &str) -> Option<String> {
    std::env::var(k)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

pub fn cfg(state: &AppState) -> &'static SitesCfg {
    static CFG: OnceLock<SitesCfg> = OnceLock::new();
    CFG.get_or_init(|| {
        let r2 = match (
            env_nonempty("ARBOS_SITES_R2_BUCKET"),
            env_nonempty("ARBOS_SITES_R2_ENDPOINT"),
            env_nonempty("ARBOS_SITES_R2_ACCESS_KEY_ID"),
            env_nonempty("ARBOS_SITES_R2_SECRET_ACCESS_KEY"),
        ) {
            (Some(bucket), Some(endpoint), Some(access_key), Some(secret_key)) => Some(S3Target {
                endpoint: endpoint.trim_end_matches('/').to_owned(),
                bucket,
                access_key,
                secret_key,
                region: env_nonempty("ARBOS_SITES_R2_REGION").unwrap_or_else(|| "auto".into()),
            }),
            _ => None,
        };
        if r2.is_none() {
            tracing::info!(
                "sites: no ARBOS_SITES_R2_* configured; publishes stay relay-local (not live)"
            );
        }
        let scheme = env_nonempty("ARBOS_SITES_SCHEME")
            .and_then(|s| Scheme::parse(&s))
            .unwrap_or(Scheme::Subdomain);
        let domain = env_nonempty("ARBOS_SITES_DOMAIN").unwrap_or_else(|| {
            state
                .cfg
                .site_url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .split('/')
                .next()
                .unwrap_or("arbos.life")
                .to_owned()
        });
        SitesCfg {
            route: Route::new(scheme, &domain),
            publish_enabled: std::env::var("ARBOS_SITES_PUBLISH")
                .map(|v| v != "0")
                .unwrap_or(true),
            abuse_email: env_nonempty("ARBOS_SITES_ABUSE_EMAIL"),
            r2,
        }
    })
}

// ---- storage ------------------------------------------------------------------------------------

fn blobs_dir(state: &AppState) -> PathBuf {
    state.cfg.data_dir.join("sites").join("blobs")
}

fn blob_path(state: &AppState, sha: &str) -> PathBuf {
    blobs_dir(state).join(&sha[..2]).join(sha)
}

fn sha_ok(sha: &str) -> bool {
    sha.len() == 64
        && sha
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SiteRow {
    pub user_id: String,
    pub app: String,
    pub prefix: String,
    pub status: String,
    pub version: i64,
    pub bytes: i64,
    pub files: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub published_at: Option<i64>,
    pub edge_synced_at: Option<i64>,
    pub takedown_reason: Option<String>,
    pub takedown_by: Option<String>,
    #[serde(skip)]
    pub manifest: String,
}

const SITE_COLS: &str = "user_id, app, prefix, status, version, manifest, bytes, files, created_at, updated_at, published_at, edge_synced_at, takedown_reason, takedown_by";

fn site_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SiteRow> {
    Ok(SiteRow {
        user_id: r.get(0)?,
        app: r.get(1)?,
        prefix: r.get(2)?,
        status: r.get(3)?,
        version: r.get(4)?,
        manifest: r.get(5)?,
        bytes: r.get(6)?,
        files: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
        published_at: r.get(10)?,
        edge_synced_at: r.get(11)?,
        takedown_reason: r.get(12)?,
        takedown_by: r.get(13)?,
    })
}

struct Quota {
    max_sites: i64,
    max_bytes: i64,
    publish_blocked: bool,
}

impl crate::db::Db {
    fn site(&self, user_id: &str, app: &str) -> anyhow::Result<Option<SiteRow>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {SITE_COLS} FROM sites WHERE user_id = ?1 AND app = ?2"),
                params![user_id, app],
                site_from_row,
            )
            .optional()
        })
    }

    fn user_sites(&self, user_id: &str) -> anyhow::Result<Vec<SiteRow>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {SITE_COLS} FROM sites WHERE user_id = ?1 ORDER BY app"
            ))?;
            let rows = st.query_map(params![user_id], site_from_row)?;
            rows.collect()
        })
    }

    fn all_sites(&self) -> anyhow::Result<Vec<SiteRow>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {SITE_COLS} FROM sites ORDER BY updated_at DESC"
            ))?;
            let rows = st.query_map([], site_from_row)?;
            rows.collect()
        })
    }

    fn site_quota(&self, user_id: &str) -> anyhow::Result<Quota> {
        self.with(|c| {
            let q: Option<(Option<i64>, Option<i64>, i64)> = c
                .query_row(
                    "SELECT max_sites, max_bytes, publish_blocked FROM site_quotas WHERE user_id = ?1",
                    params![user_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let (ms, mb, blocked) = q.unwrap_or((None, None, 0));
            Ok(Quota {
                max_sites: ms.unwrap_or(DEFAULT_MAX_SITES),
                max_bytes: mb.unwrap_or(DEFAULT_MAX_USER_BYTES),
                publish_blocked: blocked != 0,
            })
        })
    }

    /// The prefix a user already claimed, if any.
    fn site_prefix_of(&self, user_id: &str) -> anyhow::Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT prefix FROM site_prefixes WHERE user_id = ?1",
                params![user_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
        })
    }

    /// Who holds `prefix`: as a current claim, or as a name moved away from within the redirect
    /// window (still unclaimable so the 301 keeps working).
    fn site_prefix_holder(&self, prefix: &str) -> anyhow::Result<Option<String>> {
        self.with(|c| {
            if let Some(u) = c
                .query_row(
                    "SELECT user_id FROM site_prefixes WHERE prefix = ?1",
                    params![prefix],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                return Ok(Some(u));
            }
            c.query_row(
                "SELECT user_id FROM site_prefixes WHERE moved_from = ?1 AND moved_at >= ?2",
                params![prefix, now_ms() - MOVE_REDIRECT_MS],
                |r| r.get::<_, String>(0),
            )
            .optional()
        })
    }

    /// Claim the user's subdomain once, at first publish, from the login. Later handle renames do
    /// not move it (`site_prefix_move` does, explicitly). Errors carry the reason for the agent.
    fn site_prefix_claim(
        &self,
        user_id: &str,
        login: Option<&str>,
    ) -> anyhow::Result<Result<String, String>> {
        if let Some(p) = self.site_prefix_of(user_id)? {
            return Ok(Ok(p));
        }
        let Some(login) = login.filter(|l| !l.trim().is_empty()) else {
            return Ok(Err("this account has no site address yet: addresses come from the owner's GitHub login, so the owner must sign in with GitHub first".to_owned()));
        };
        let sub = match subdomain_for_handle(login) {
            Ok(s) => s,
            Err(e) => return Ok(Err(e.0)),
        };
        if let Some(holder) = self.site_prefix_holder(&sub)? {
            if holder != user_id {
                return Ok(Err(format!(
                    "the address `{sub}` is already claimed by another account; an administrator can assign a different one"
                )));
            }
        }
        self.with(|c| {
            c.execute(
                "INSERT INTO site_prefixes(user_id, prefix, created_at) VALUES (?1, ?2, ?3)",
                params![user_id, sub, now_ms()],
            )?;
            Ok(())
        })?;
        Ok(Ok(sub))
    }

    /// Explicit move (admin): the new name must be free; the old one 301s for 30 days.
    fn site_prefix_move(
        &self,
        user_id: &str,
        new_prefix: &str,
    ) -> anyhow::Result<Result<Option<String>, String>> {
        if let Err(e) = check_subdomain(new_prefix) {
            return Ok(Err(e.0));
        }
        if let Some(holder) = self.site_prefix_holder(new_prefix)? {
            if holder != user_id {
                return Ok(Err(format!("`{new_prefix}` is claimed by another account")));
            }
        }
        let old = self.site_prefix_of(user_id)?;
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "INSERT INTO site_prefixes(user_id, prefix, created_at, moved_from, moved_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(user_id) DO UPDATE SET prefix = excluded.prefix, moved_from = excluded.moved_from, moved_at = excluded.moved_at",
                params![user_id, new_prefix, now, old, old.as_ref().map(|_| now)],
            )?;
            c.execute(
                "UPDATE sites SET prefix = ?2, updated_at = ?3, edge_synced_at = NULL WHERE user_id = ?1",
                params![user_id, new_prefix, now],
            )?;
            Ok(())
        })?;
        Ok(Ok(old))
    }

    #[allow(clippy::too_many_arguments)]
    fn site_event(
        &self,
        user_id: &str,
        app: &str,
        kind: &str,
        actor: &str,
        version: Option<i64>,
        bytes: Option<i64>,
        files: Option<i64>,
        detail: Option<&str>,
    ) -> anyhow::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO site_events(at, user_id, app, kind, actor, version, bytes, files, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![now_ms(), user_id, app, kind, actor, version, bytes, files, detail],
            )?;
            Ok(())
        })
    }

    fn site_publishes_since(&self, user_id: &str, since: i64) -> anyhow::Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM site_events WHERE user_id = ?1 AND kind = 'publish' AND at >= ?2",
                params![user_id, since],
                |r| r.get(0),
            )
        })
    }

    fn site_blob_known(&self, sha: &str) -> anyhow::Result<Option<(i64, Option<i64>)>> {
        self.with(|c| {
            c.query_row(
                "SELECT size, edge_pushed_at FROM site_blobs WHERE sha256 = ?1",
                params![sha],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
    }

    fn site_blob_record(&self, sha: &str, size: i64) -> anyhow::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO site_blobs(sha256, size, created_at) VALUES (?1, ?2, ?3)",
                params![sha, size, now_ms()],
            )?;
            Ok(())
        })
    }

    fn site_blob_pushed(&self, sha: &str) -> anyhow::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE site_blobs SET edge_pushed_at = ?2 WHERE sha256 = ?1",
                params![sha, now_ms()],
            )?;
            Ok(())
        })
    }
}

// ---- helpers --------------------------------------------------------------------------------------

fn db_err(e: anyhow::Error) -> Response {
    api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string())
}

fn refused(msg: impl Into<String>) -> Response {
    api_error(StatusCode::UNPROCESSABLE_ENTITY, "site_rule", msg)
}

/// Who may publish: a box (the agent) or a device (future Settings UI) acting for a user.
fn owner_of(principal: &Principal) -> Result<String, Box<Response>> {
    match principal.user_id() {
        Some(u) => Ok(u.to_owned()),
        None => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a user's box or device token is required",
        ))),
    }
}

fn admin_only(principal: &Principal) -> Result<(), Box<Response>> {
    match principal {
        Principal::Admin => Ok(()),
        _ => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "admin token required",
        ))),
    }
}

fn actor_name(principal: &Principal) -> &'static str {
    match principal {
        Principal::Box { .. } => "box",
        Principal::Device { .. } => "owner",
        Principal::Admin => "admin",
    }
}

fn site_json(state: &AppState, s: &SiteRow) -> Value {
    let c = cfg(state);
    json!({
        "app": s.app,
        "prefix": s.prefix,
        "status": s.status,
        "version": s.version,
        "bytes": s.bytes,
        "files": s.files,
        "public_url": c.route.public_url(&s.prefix, &s.app),
        "published_at": s.published_at,
        "updated_at": s.updated_at,
        "edge_synced_at": s.edge_synced_at,
        "takedown_reason": s.takedown_reason,
        "user_id": s.user_id,
    })
}

fn limits_json() -> Value {
    json!({
        "max_site_bytes": MAX_SITE_BYTES,
        "max_file_bytes": MAX_FILE_BYTES,
        "max_files": MAX_FILES,
        "allowed_extensions": arbos_core::sites::allowed_extensions(),
    })
}

/// The prefix a user publishes under (claimed now if this is their first publish), or the
/// refusal that says why there is none.
fn prefix_for(state: &AppState, user_id: &str) -> Result<String, Box<Response>> {
    let user = state.db.user(user_id).map_err(|e| Box::new(db_err(e)))?;
    let login = user.as_ref().and_then(|u| u.login.as_deref());
    match state
        .db
        .site_prefix_claim(user_id, login)
        .map_err(|e| Box::new(db_err(e)))?
    {
        Ok(p) => Ok(p),
        Err(reason) => Err(Box::new(api_error(
            StatusCode::CONFLICT,
            "no_prefix",
            reason,
        ))),
    }
}

// ---- box API: begin / blob / commit --------------------------------------------------------------

#[derive(Deserialize)]
pub struct BeginBody {
    pub files: Vec<SiteFile>,
}

/// `POST /sites/{app}/begin`: validate the file list and the quotas, remember the session, and
/// answer the hashes the relay does not hold yet.
pub async fn begin(
    State(state): State<AppState>,
    principal: Principal,
    Path(app): Path<String>,
    Json(body): Json<BeginBody>,
) -> Response {
    let user_id = match owner_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let c = cfg(&state);
    if !c.publish_enabled {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "publish_off",
            "publishing is paused by the operator; existing sites stay up",
        );
    }
    if let Err(e) = check_app(&app) {
        return refused(e.0);
    }
    if let Err(e) = check_files(&body.files) {
        return refused(e.0);
    }
    let quota = match state.db.site_quota(&user_id) {
        Ok(q) => q,
        Err(e) => return db_err(e),
    };
    if quota.publish_blocked {
        return api_error(
            StatusCode::FORBIDDEN,
            "publish_blocked",
            "publishing is blocked for this account by an administrator",
        );
    }
    let prefix = match prefix_for(&state, &user_id) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let now = now_ms();
    for (window, cap, name) in [
        (3_600_000, MAX_PUBLISHES_PER_HOUR, "hour"),
        (86_400_000, MAX_PUBLISHES_PER_DAY, "day"),
    ] {
        match state.db.site_publishes_since(&user_id, now - window) {
            Ok(n) if n >= cap => {
                return api_error(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited",
                    format!("{cap} publishes per {name} reached; try again later"),
                )
            }
            Ok(_) => {}
            Err(e) => return db_err(e),
        }
    }
    let existing = match state.db.user_sites(&user_id) {
        Ok(s) => s,
        Err(e) => return db_err(e),
    };
    let is_new = !existing.iter().any(|s| s.app == app);
    let live_count = existing
        .iter()
        .filter(|s| s.status != "unpublished")
        .count() as i64;
    if is_new && live_count >= quota.max_sites {
        return api_error(
            StatusCode::FORBIDDEN,
            "quota_sites",
            format!(
                "this account already has {live_count} sites (limit {}); unpublish one first",
                quota.max_sites
            ),
        );
    }
    let new_bytes: i64 = body.files.iter().map(|f| f.size as i64).sum();
    let other_bytes: i64 = existing
        .iter()
        .filter(|s| s.app != app && s.status != "unpublished")
        .map(|s| s.bytes)
        .sum();
    if other_bytes + new_bytes > quota.max_bytes {
        return api_error(
            StatusCode::FORBIDDEN,
            "quota_bytes",
            format!(
                "publishing this site would put the account at {:.1} MB of {} MB; shrink it or unpublish another site",
                (other_bytes + new_bytes) as f64 / (1024.0 * 1024.0),
                quota.max_bytes / (1024 * 1024)
            ),
        );
    }
    let mut missing = Vec::new();
    for f in &body.files {
        let have = match state.db.site_blob_known(&f.sha256) {
            Ok(Some((size, _))) => size as u64 == f.size && blob_path(&state, &f.sha256).is_file(),
            Ok(None) => false,
            Err(e) => return db_err(e),
        };
        if !have && !missing.contains(&f.sha256) {
            missing.push(f.sha256.clone());
        }
    }
    let files_json = serde_json::to_string(&body.files).unwrap_or_default();
    if let Err(e) = state.db.with(|c| {
        c.execute(
            "DELETE FROM site_pending WHERE created_at < ?1",
            params![now - PENDING_TTL_MS],
        )?;
        c.execute(
            "INSERT OR REPLACE INTO site_pending(user_id, app, files, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![user_id, app, files_json, now],
        )?;
        Ok(())
    }) {
        return db_err(e);
    }
    let _ = state.db.site_event(
        &user_id,
        &app,
        "begin",
        actor_name(&principal),
        None,
        Some(new_bytes),
        Some(body.files.len() as i64),
        Some(&format!("missing {}", missing.len())),
    );
    Json(json!({
        "prefix": prefix,
        "app": app,
        "public_url": c.route.public_url(&prefix, &app),
        "scheme": c.route.scheme.as_str(),
        "domain": c.route.domain,
        "missing": missing,
        "edge": c.r2.is_some(),
        "limits": limits_json(),
    }))
    .into_response()
}

fn pending_files(
    state: &AppState,
    user_id: &str,
    app: &str,
) -> Result<Vec<SiteFile>, Box<Response>> {
    let raw: Option<(String, i64)> = state
        .db
        .with(|c| {
            c.query_row(
                "SELECT files, created_at FROM site_pending WHERE user_id = ?1 AND app = ?2",
                params![user_id, app],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
        .map_err(|e| Box::new(db_err(e)))?;
    match raw {
        Some((files, at)) if at >= now_ms() - PENDING_TTL_MS => serde_json::from_str(&files)
            .map_err(|e| {
                Box::new(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "pending",
                    e.to_string(),
                ))
            }),
        _ => Err(Box::new(api_error(
            StatusCode::CONFLICT,
            "no_session",
            "no publish in progress for this site: call begin first (sessions last one hour)",
        ))),
    }
}

/// `PUT /sites/{app}/blob/{sha}`: one file's bytes, announced in `begin`; verified by hash and size.
pub async fn put_blob(
    State(state): State<AppState>,
    principal: Principal,
    Path((app, sha)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let user_id = match owner_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    if check_app(&app).is_err() || !sha_ok(&sha) {
        return refused("bad app name or hash");
    }
    let files = match pending_files(&state, &user_id, &app) {
        Ok(f) => f,
        Err(r) => return *r,
    };
    let Some(announced) = files.iter().find(|f| f.sha256 == sha) else {
        return refused("this hash was not announced in begin");
    };
    if body.len() as u64 != announced.size || body.len() as u64 > MAX_FILE_BYTES {
        return refused(format!(
            "`{}`: {} bytes received, {} announced",
            announced.path,
            body.len(),
            announced.size
        ));
    }
    let actual = hex::encode(Sha256::digest(&body));
    if actual != sha {
        return refused(format!(
            "`{}`: bytes do not match the announced sha256",
            announced.path
        ));
    }
    if let Some(ct) = check_path(&announced.path)
        .ok()
        .filter(|ct| ct.starts_with("text/html"))
    {
        let _ = ct;
        if let Err(e) = check_html(&announced.path, &String::from_utf8_lossy(&body)) {
            let _ = state.db.site_event(
                &user_id,
                &app,
                "refused",
                actor_name(&principal),
                None,
                None,
                None,
                Some(&e.0),
            );
            return refused(e.0);
        }
    }
    let path = blob_path(&state, &sha);
    if let Some(dir) = path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(dir).await {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string());
        }
    }
    let tmp = path.with_extension(format!("tmp-{}", now_ms()));
    if let Err(e) = tokio::fs::write(&tmp, &body).await {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string());
    }
    if let Err(e) = tokio::fs::rename(&tmp, &path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string());
    }
    if let Err(e) = state.db.site_blob_record(&sha, body.len() as i64) {
        return db_err(e);
    }
    Json(json!({ "sha256": sha, "size": body.len(), "stored": true })).into_response()
}

/// `POST /sites/{app}/commit`: every announced blob must be present; the manifest becomes the
/// live version, goes to the edge when configured, and the public URL comes back.
pub async fn commit(
    State(state): State<AppState>,
    principal: Principal,
    Path(app): Path<String>,
) -> Response {
    let user_id = match owner_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    if check_app(&app).is_err() {
        return refused("bad app name");
    }
    let files = match pending_files(&state, &user_id, &app) {
        Ok(f) => f,
        Err(r) => return *r,
    };
    if let Err(e) = check_files(&files) {
        return refused(e.0);
    }
    let prefix = match prefix_for(&state, &user_id) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let mut entries = BTreeMap::new();
    let mut missing = Vec::new();
    for f in &files {
        let ct = match check_path(&f.path) {
            Ok(ct) => ct,
            Err(e) => return refused(e.0),
        };
        if !blob_path(&state, &f.sha256).is_file() {
            missing.push(f.path.clone());
            continue;
        }
        entries.insert(
            f.path.clone(),
            ManifestEntry {
                sha256: f.sha256.clone(),
                size: f.size,
                content_type: ct.to_owned(),
            },
        );
    }
    if !missing.is_empty() {
        return api_error(
            StatusCode::CONFLICT,
            "blobs_missing",
            format!("upload these files first: {}", missing.join(", ")),
        );
    }
    let existing = match state.db.site(&user_id, &app) {
        Ok(s) => s,
        Err(e) => return db_err(e),
    };
    if let Some(s) = &existing {
        if s.status == "down" {
            return api_error(
                StatusCode::FORBIDDEN,
                "taken_down",
                format!(
                    "this site was taken down by an administrator{}; it cannot be republished",
                    s.takedown_reason
                        .as_deref()
                        .map(|r| format!(" ({r})"))
                        .unwrap_or_default()
                ),
            );
        }
    }
    let version = existing.as_ref().map(|s| s.version + 1).unwrap_or(1);
    let now = now_ms();
    let bytes: i64 = files.iter().map(|f| f.size as i64).sum();
    let manifest = SiteManifest {
        prefix: prefix.clone(),
        app: app.clone(),
        version,
        status: "live".into(),
        published_at: now,
        files: entries,
        reason: None,
    };
    let manifest_json = serde_json::to_string(&manifest).unwrap_or_default();
    let nfiles = files.len() as i64;
    if let Err(e) = state.db.with(|c| {
        c.execute(
            "INSERT INTO sites(user_id, app, prefix, status, version, manifest, bytes, files, created_at, updated_at, published_at)
             VALUES (?1, ?2, ?3, 'live', ?4, ?5, ?6, ?7, ?8, ?8, ?8)
             ON CONFLICT(user_id, app) DO UPDATE SET status = 'live', version = excluded.version, manifest = excluded.manifest,
               bytes = excluded.bytes, files = excluded.files, updated_at = excluded.updated_at, published_at = excluded.published_at,
               edge_synced_at = NULL, prefix = excluded.prefix",
            params![user_id, app, prefix, version, manifest_json, bytes, nfiles, now],
        )?;
        c.execute(
            "INSERT OR REPLACE INTO site_versions(user_id, app, version, manifest, bytes, files, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![user_id, app, version, manifest_json, bytes, nfiles, now],
        )?;
        c.execute(
            "DELETE FROM site_pending WHERE user_id = ?1 AND app = ?2",
            params![user_id, app],
        )?;
        Ok(())
    }) {
        return db_err(e);
    }
    let _ = state.db.site_event(
        &user_id,
        &app,
        "publish",
        actor_name(&principal),
        Some(version),
        Some(bytes),
        Some(nfiles),
        None,
    );
    let c = cfg(&state);
    let (edge, edge_error) = match &c.r2 {
        Some(r2) => match push_manifest_to_edge(&state, r2, &manifest).await {
            Ok(()) => {
                if let Err(e) = push_user_index(&state, r2, &user_id).await {
                    tracing::warn!(error = %e, user = %user_id, "sites: index push failed");
                }
                let _ = state.db.with(|c| {
                    c.execute(
                        "UPDATE sites SET edge_synced_at = ?3 WHERE user_id = ?1 AND app = ?2",
                        params![user_id, app, now_ms()],
                    )?;
                    Ok(())
                });
                (true, None)
            }
            Err(e) => {
                tracing::warn!(error = %e, user = %user_id, app = %app, "sites: edge push failed");
                let _ = state.db.site_event(
                    &user_id,
                    &app,
                    "edge_error",
                    "relay",
                    Some(version),
                    None,
                    None,
                    Some(&e.to_string()),
                );
                (false, Some(e.to_string()))
            }
        },
        None => (
            false,
            Some("public route not configured on this relay".to_owned()),
        ),
    };
    Json(json!({
        "app": app,
        "prefix": prefix,
        "version": version,
        "bytes": bytes,
        "files": nfiles,
        "public_url": c.route.public_url(&prefix, &app),
        "user_url": c.route.user_url(&prefix),
        "edge": edge,
        "edge_error": edge_error,
        "index_sha256": manifest.files.get("index.html").map(|e| e.sha256.clone()),
    }))
    .into_response()
}

// ---- owner API --------------------------------------------------------------------------------------

/// `GET /sites`: the caller's sites.
pub async fn list(State(state): State<AppState>, principal: Principal) -> Response {
    let user_id = match owner_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let rows = match state.db.user_sites(&user_id) {
        Ok(r) => r,
        Err(e) => return db_err(e),
    };
    let prefix = state.db.site_prefix_of(&user_id).ok().flatten();
    let quota = state.db.site_quota(&user_id).ok();
    let c = cfg(&state);
    Json(json!({
        "prefix": prefix,
        "user_url": prefix.as_deref().and_then(|p| c.route.user_url(p)),
        "scheme": c.route.scheme.as_str(),
        "domain": c.route.domain,
        "edge": c.r2.is_some(),
        "abuse_email": c.abuse_email,
        "sites": rows.iter().filter(|s| s.status != "unpublished").map(|s| site_json(&state, s)).collect::<Vec<_>>(),
        "quota": quota.map(|q| json!({ "max_sites": q.max_sites, "max_bytes": q.max_bytes, "publish_blocked": q.publish_blocked })),
        "limits": limits_json(),
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct UnpublishQuery {
    #[serde(default)]
    pub all: Option<String>,
}

async fn unpublish_one(
    state: &AppState,
    s: &SiteRow,
    actor: &str,
    kind: &str,
    reason: Option<&str>,
) -> Result<(), anyhow::Error> {
    let now = now_ms();
    let new_status = if kind == "takedown" {
        "down"
    } else {
        "unpublished"
    };
    state.db.with(|c| {
        c.execute(
            "UPDATE sites SET status = ?3, updated_at = ?4, takedown_reason = ?5, takedown_by = ?6, edge_synced_at = NULL WHERE user_id = ?1 AND app = ?2",
            params![s.user_id, s.app, new_status, now, reason, if kind == "takedown" { Some(actor) } else { None }],
        )?;
        Ok(())
    })?;
    let _ = state.db.site_event(
        &s.user_id,
        &s.app,
        kind,
        actor,
        Some(s.version),
        None,
        None,
        reason,
    );
    if let Some(r2) = &cfg(state).r2 {
        let key = manifest_key(&s.prefix, &s.app);
        let res = if new_status == "down" {
            let mut m: SiteManifest = serde_json::from_str(&s.manifest)?;
            m.status = "down".into();
            m.reason = reason.map(str::to_owned);
            m.files.clear();
            r2.put(
                &key,
                serde_json::to_vec(&m)?,
                "application/json",
                "no-cache",
            )
            .await
        } else {
            r2.delete(&key).await
        };
        match res {
            Ok(()) => {
                let _ = state.db.with(|c| {
                    c.execute(
                        "UPDATE sites SET edge_synced_at = ?3 WHERE user_id = ?1 AND app = ?2",
                        params![s.user_id, s.app, now_ms()],
                    )?;
                    Ok(())
                });
                let _ = push_user_index(state, r2, &s.user_id).await;
            }
            Err(e) => {
                let _ = state.db.site_event(
                    &s.user_id,
                    &s.app,
                    "edge_error",
                    "relay",
                    Some(s.version),
                    None,
                    None,
                    Some(&e.to_string()),
                );
                return Err(e);
            }
        }
    }
    Ok(())
}

/// `DELETE /sites/{app}`: the owner takes a site down.
pub async fn unpublish(
    State(state): State<AppState>,
    principal: Principal,
    Path(app): Path<String>,
) -> Response {
    let user_id = match owner_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let Some(s) = state.db.site(&user_id, &app).ok().flatten() else {
        return api_error(StatusCode::NOT_FOUND, "no_site", "no such site");
    };
    if s.status == "down" {
        return api_error(
            StatusCode::FORBIDDEN,
            "taken_down",
            "this site was taken down by an administrator; only they can change it",
        );
    }
    match unpublish_one(&state, &s, actor_name(&principal), "unpublish", None).await {
        Ok(()) => {
            Json(json!({ "app": app, "status": "unpublished", "edge": cfg(&state).r2.is_some() }))
                .into_response()
        }
        Err(e) => api_error(
            StatusCode::BAD_GATEWAY,
            "edge",
            format!("the site is unpublished on the relay but the edge did not confirm: {e}"),
        ),
    }
}

/// `DELETE /sites?all=1`: the owner's kill switch — every site of theirs comes down.
pub async fn unpublish_all(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<UnpublishQuery>,
) -> Response {
    let user_id = match owner_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    if q.all.as_deref() != Some("1") && q.all.as_deref() != Some("true") {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "pass ?all=1 to take every site down",
        );
    }
    let rows = match state.db.user_sites(&user_id) {
        Ok(r) => r,
        Err(e) => return db_err(e),
    };
    let mut done = Vec::new();
    let mut failed = Vec::new();
    for s in rows.iter().filter(|s| s.status == "live") {
        match unpublish_one(&state, s, actor_name(&principal), "unpublish", Some("all")).await {
            Ok(()) => done.push(s.app.clone()),
            Err(e) => failed.push(json!({ "app": s.app, "error": e.to_string() })),
        }
    }
    Json(json!({ "unpublished": done, "failed": failed })).into_response()
}

// ---- admin API --------------------------------------------------------------------------------------

/// `GET /admin/sites`: every site with its owner.
pub async fn admin_list(State(state): State<AppState>, principal: Principal) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let rows = match state.db.all_sites() {
        Ok(r) => r,
        Err(e) => return db_err(e),
    };
    let mut out = Vec::with_capacity(rows.len());
    for s in &rows {
        let mut v = site_json(&state, s);
        let login = state
            .db
            .user(&s.user_id)
            .ok()
            .flatten()
            .and_then(|u| u.login);
        v["login"] = json!(login);
        out.push(v);
    }
    let c = cfg(&state);
    Json(json!({ "sites": out, "edge": c.r2.is_some(), "publish_enabled": c.publish_enabled, "scheme": c.route.scheme.as_str(), "domain": c.route.domain }))
        .into_response()
}

#[derive(Deserialize)]
pub struct MoveBody {
    pub prefix: String,
}

/// `POST /admin/sites/move/{user} {prefix}`: the explicit address move. The old address keeps
/// answering 301 for 30 days (`sites/<old>/redirect.json` at the edge) and cannot be claimed
/// meanwhile; every site of the user is re-pushed under the new prefix.
pub async fn admin_move(
    State(state): State<AppState>,
    principal: Principal,
    Path(user_id): Path<String>,
    Json(body): Json<MoveBody>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let new_prefix = body.prefix.trim().to_ascii_lowercase();
    let old = match state.db.site_prefix_move(&user_id, &new_prefix) {
        Ok(Ok(old)) => old,
        Ok(Err(reason)) => return refused(reason),
        Err(e) => return db_err(e),
    };
    let _ = state.db.site_event(
        &user_id,
        "*",
        "move",
        "admin",
        None,
        None,
        None,
        Some(&format!(
            "{} -> {new_prefix}",
            old.as_deref().unwrap_or("-")
        )),
    );
    let c = cfg(&state);
    let mut edge_errors = Vec::new();
    if let Some(r2) = &c.r2 {
        if let Some(old) = &old {
            let redirect = json!({
                "from": old,
                "to": new_prefix,
                "to_url": c.route.user_url(&new_prefix),
                "until": now_ms() + MOVE_REDIRECT_MS,
            });
            if let Err(e) = r2
                .put(
                    &format!("sites/{old}/redirect.json"),
                    serde_json::to_vec(&redirect).unwrap_or_default(),
                    "application/json",
                    "no-cache",
                )
                .await
            {
                edge_errors.push(e.to_string());
            }
        }
        if let Ok(rows) = state.db.user_sites(&user_id) {
            for s in rows.iter().filter(|s| s.status != "unpublished") {
                if let Ok(mut m) = serde_json::from_str::<SiteManifest>(&s.manifest) {
                    m.prefix = new_prefix.clone();
                    if let Err(e) = push_manifest_to_edge(&state, r2, &m).await {
                        edge_errors.push(format!("{}: {e}", s.app));
                    }
                    if let Some(old) = &old {
                        let _ = r2.delete(&manifest_key(old, &s.app)).await;
                    }
                }
            }
        }
        if let Err(e) = push_user_index(&state, r2, &user_id).await {
            edge_errors.push(e.to_string());
        }
    }
    Json(json!({
        "user_id": user_id,
        "prefix": new_prefix,
        "moved_from": old,
        "user_url": c.route.user_url(&new_prefix),
        "redirect_until": old.as_ref().map(|_| now_ms() + MOVE_REDIRECT_MS),
        "edge_errors": edge_errors,
    }))
    .into_response()
}

#[derive(Deserialize, Default)]
pub struct TakedownBody {
    #[serde(default)]
    pub reason: Option<String>,
}

/// `POST /admin/sites/{user}/{app}/takedown {reason}`: the edge answers 410 within its cache TTL;
/// the owner cannot republish until `restore`.
pub async fn admin_takedown(
    State(state): State<AppState>,
    principal: Principal,
    Path((user_id, app)): Path<(String, String)>,
    body: Option<Json<TakedownBody>>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let Some(s) = state.db.site(&user_id, &app).ok().flatten() else {
        return api_error(StatusCode::NOT_FOUND, "no_site", "no such site");
    };
    let reason = body
        .and_then(|b| b.0.reason)
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| "taken down by Arbos".to_owned());
    match unpublish_one(&state, &s, "admin", "takedown", Some(&reason)).await {
        Ok(()) => {
            Json(json!({ "user_id": user_id, "app": app, "status": "down", "reason": reason }))
                .into_response()
        }
        Err(e) => api_error(
            StatusCode::BAD_GATEWAY,
            "edge",
            format!("taken down on the relay but the edge did not confirm: {e}"),
        ),
    }
}

/// `POST /admin/sites/{user}/{app}/restore`: back to `live` with its last manifest.
pub async fn admin_restore(
    State(state): State<AppState>,
    principal: Principal,
    Path((user_id, app)): Path<(String, String)>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let Some(s) = state.db.site(&user_id, &app).ok().flatten() else {
        return api_error(StatusCode::NOT_FOUND, "no_site", "no such site");
    };
    let mut m: SiteManifest = match serde_json::from_str(&s.manifest) {
        Ok(m) => m,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "manifest", e.to_string()),
    };
    m.status = "live".into();
    m.reason = None;
    if let Err(e) = state.db.with(|c| {
        c.execute(
            "UPDATE sites SET status = 'live', updated_at = ?3, takedown_reason = NULL, takedown_by = NULL, edge_synced_at = NULL WHERE user_id = ?1 AND app = ?2",
            params![user_id, app, now_ms()],
        )?;
        Ok(())
    }) {
        return db_err(e);
    }
    let _ = state.db.site_event(
        &user_id,
        &app,
        "restore",
        "admin",
        Some(s.version),
        None,
        None,
        None,
    );
    if let Some(r2) = &cfg(&state).r2 {
        if let Err(e) = push_manifest_to_edge(&state, r2, &m).await {
            return api_error(
                StatusCode::BAD_GATEWAY,
                "edge",
                format!("restored on the relay but the edge did not confirm: {e}"),
            );
        }
        let _ = push_user_index(&state, r2, &user_id).await;
        let _ = state.db.with(|c| {
            c.execute(
                "UPDATE sites SET edge_synced_at = ?3 WHERE user_id = ?1 AND app = ?2",
                params![user_id, app, now_ms()],
            )?;
            Ok(())
        });
    }
    Json(json!({ "user_id": user_id, "app": app, "status": "live" })).into_response()
}

/// `DELETE /admin/sites/{user}/{app}`: forget the site; blobs stay (content-addressed, shared).
pub async fn admin_purge(
    State(state): State<AppState>,
    principal: Principal,
    Path((user_id, app)): Path<(String, String)>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let Some(s) = state.db.site(&user_id, &app).ok().flatten() else {
        return api_error(StatusCode::NOT_FOUND, "no_site", "no such site");
    };
    if let Some(r2) = &cfg(&state).r2 {
        if let Err(e) = r2.delete(&manifest_key(&s.prefix, &s.app)).await {
            return api_error(StatusCode::BAD_GATEWAY, "edge", e.to_string());
        }
    }
    if let Err(e) = state.db.with(|c| {
        c.execute(
            "DELETE FROM sites WHERE user_id = ?1 AND app = ?2",
            params![user_id, app],
        )?;
        c.execute(
            "DELETE FROM site_versions WHERE user_id = ?1 AND app = ?2",
            params![user_id, app],
        )?;
        c.execute(
            "DELETE FROM site_pending WHERE user_id = ?1 AND app = ?2",
            params![user_id, app],
        )?;
        Ok(())
    }) {
        return db_err(e);
    }
    let _ = state.db.site_event(
        &user_id,
        &app,
        "purge",
        "admin",
        Some(s.version),
        None,
        None,
        None,
    );
    Json(json!({ "user_id": user_id, "app": app, "purged": true })).into_response()
}

#[derive(Deserialize)]
pub struct EventsQuery {
    #[serde(default)]
    pub since: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// `GET /admin/sites/events?since=<ms>&limit=`: the audit trail, oldest first.
pub async fn admin_events(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<EventsQuery>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let since = q.since.unwrap_or(0);
    let limit = q.limit.unwrap_or(500).clamp(1, 5000);
    let rows: anyhow::Result<Vec<Value>> = state.db.with(|c| {
        let mut st = c.prepare(
            "SELECT id, at, user_id, app, kind, actor, version, bytes, files, detail FROM site_events WHERE at >= ?1 ORDER BY at ASC, id ASC LIMIT ?2",
        )?;
        let rows = st.query_map(params![since, limit], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "at": r.get::<_, i64>(1)?, "user_id": r.get::<_, String>(2)?,
                "app": r.get::<_, String>(3)?, "kind": r.get::<_, String>(4)?, "actor": r.get::<_, String>(5)?,
                "version": r.get::<_, Option<i64>>(6)?, "bytes": r.get::<_, Option<i64>>(7)?,
                "files": r.get::<_, Option<i64>>(8)?, "detail": r.get::<_, Option<String>>(9)?,
            }))
        })?;
        rows.collect()
    });
    match rows {
        Ok(events) => Json(json!({ "events": events })).into_response(),
        Err(e) => db_err(e),
    }
}

#[derive(Deserialize, Default)]
pub struct QuotaBody {
    pub max_sites: Option<i64>,
    pub max_bytes: Option<i64>,
    pub publish_blocked: Option<bool>,
}

/// `PUT /admin/sites/quota/{user}`: raise or lower one account's ceilings, or block its publishing.
pub async fn admin_set_quota(
    State(state): State<AppState>,
    principal: Principal,
    Path(user_id): Path<String>,
    Json(body): Json<QuotaBody>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let cur = match state.db.site_quota(&user_id) {
        Ok(q) => q,
        Err(e) => return db_err(e),
    };
    let max_sites = body.max_sites.unwrap_or(cur.max_sites);
    let max_bytes = body.max_bytes.unwrap_or(cur.max_bytes);
    let blocked = body.publish_blocked.unwrap_or(cur.publish_blocked);
    if let Err(e) = state.db.with(|c| {
        c.execute(
            "INSERT INTO site_quotas(user_id, max_sites, max_bytes, publish_blocked) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id) DO UPDATE SET max_sites = excluded.max_sites, max_bytes = excluded.max_bytes, publish_blocked = excluded.publish_blocked",
            params![user_id, max_sites, max_bytes, blocked as i64],
        )?;
        Ok(())
    }) {
        return db_err(e);
    }
    let _ = state.db.site_event(
        &user_id,
        "*",
        "quota",
        "admin",
        None,
        Some(max_bytes),
        Some(max_sites),
        Some(if blocked { "publish_blocked" } else { "ok" }),
    );
    Json(json!({ "user_id": user_id, "max_sites": max_sites, "max_bytes": max_bytes, "publish_blocked": blocked })).into_response()
}

// ---- edge push (R2 over S3 SigV4) -----------------------------------------------------------------

fn manifest_key(prefix: &str, app: &str) -> String {
    format!("sites/{prefix}/{app}/manifest.json")
}

/// `sites/<prefix>/index.json`: the user's live sites, for the landing the Worker shows at
/// `https://<prefix>.<domain>/`.
async fn push_user_index(state: &AppState, r2: &S3Target, user_id: &str) -> anyhow::Result<()> {
    let Some(prefix) = state.db.site_prefix_of(user_id)? else {
        return Ok(());
    };
    let c = cfg(state);
    let rows = state.db.user_sites(user_id)?;
    let live: Vec<Value> = rows
        .iter()
        .filter(|s| s.status == "live")
        .map(|s| {
            json!({
                "app": s.app,
                "url": c.route.public_url(&s.prefix, &s.app),
                "published_at": s.published_at,
                "version": s.version,
            })
        })
        .collect();
    let index = json!({ "prefix": prefix, "sites": live, "updated_at": now_ms() });
    r2.put(
        &format!("sites/{prefix}/index.json"),
        serde_json::to_vec(&index)?,
        "application/json",
        "no-cache",
    )
    .await
}

/// Upload every blob the edge does not have yet, then the manifest (last, so a half-pushed
/// version is never visible).
async fn push_manifest_to_edge(
    state: &AppState,
    r2: &S3Target,
    m: &SiteManifest,
) -> anyhow::Result<()> {
    for (path, e) in &m.files {
        let pushed = state
            .db
            .site_blob_known(&e.sha256)?
            .and_then(|(_, at)| at)
            .is_some();
        if pushed {
            continue;
        }
        let bytes = tokio::fs::read(blob_path(state, &e.sha256)).await?;
        r2.put(
            &format!("blobs/{}", e.sha256),
            bytes,
            &e.content_type,
            "public, max-age=31536000, immutable",
        )
        .await
        .map_err(|err| anyhow::anyhow!("{path}: {err}"))?;
        state.db.site_blob_pushed(&e.sha256)?;
    }
    r2.put(
        &manifest_key(&m.prefix, &m.app),
        serde_json::to_vec(m)?,
        "application/json",
        "no-cache",
    )
    .await
}

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac key");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let keep = b.is_ascii_alphanumeric()
            || b == b'-'
            || b == b'_'
            || b == b'.'
            || b == b'~'
            || (keep_slash && b == b'/');
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl S3Target {
    fn host(&self) -> String {
        self.endpoint
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_owned()
    }

    async fn request(
        &self,
        method: &str,
        key: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
        cache_control: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();
        let payload_hash = hex::encode(Sha256::digest(&body));
        let canonical_uri = format!(
            "/{}/{}",
            uri_encode(&self.bucket, false),
            uri_encode(key, true)
        );
        let host = self.host();
        let mut headers: Vec<(String, String)> = vec![
            ("host".into(), host.clone()),
            ("x-amz-content-sha256".into(), payload_hash.clone()),
            ("x-amz-date".into(), amz_date.clone()),
        ];
        if let Some(ct) = content_type {
            headers.push(("content-type".into(), ct.to_owned()));
        }
        if let Some(cc) = cache_control {
            headers.push(("cache-control".into(), cc.to_owned()));
        }
        headers.sort();
        let canonical_headers: String = headers
            .iter()
            .map(|(k, v)| format!("{k}:{}\n", v.trim()))
            .collect();
        let signed_headers = headers
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>()
            .join(";");
        let canonical_request = format!(
            "{method}\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
        );
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );
        let k_date = hmac(
            format!("AWS4{}", self.secret_key).as_bytes(),
            date.as_bytes(),
        );
        let k_region = hmac(&k_date, self.region.as_bytes());
        let k_service = hmac(&k_region, b"s3");
        let k_signing = hmac(&k_service, b"aws4_request");
        let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            self.access_key
        );
        let url = format!("{}{canonical_uri}", self.endpoint);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?;
        let mut req = client
            .request(method.parse()?, &url)
            .header("authorization", authorization)
            .header("x-amz-content-sha256", payload_hash)
            .header("x-amz-date", amz_date);
        if let Some(ct) = content_type {
            req = req.header("content-type", ct);
        }
        if let Some(cc) = cache_control {
            req = req.header("cache-control", cc);
        }
        let resp = req.body(body).send().await?;
        let status = resp.status();
        if status.is_success() || (method == "DELETE" && status == reqwest::StatusCode::NOT_FOUND) {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "R2 {method} {key}: HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        )
    }

    pub async fn put(
        &self,
        key: &str,
        body: Vec<u8>,
        content_type: &str,
        cache_control: &str,
    ) -> anyhow::Result<()> {
        self.request("PUT", key, body, Some(content_type), Some(cache_control))
            .await
    }

    pub async fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.request("DELETE", key, Vec::new(), None, None).await
    }
}
