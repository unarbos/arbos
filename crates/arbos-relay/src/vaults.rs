//! Connected vaults (D83 §3, Phase 2): secrets that live elsewhere and are used by reference.
//! v1 provider: **1Password** through the pinned `op` CLI on the relay host. The service-account
//! token is stored as ciphertext under the user's DEK (AAD = vault id), is handed to `op` through
//! the environment of one child process at a time, and never reaches a box. `op://` references
//! resolve on the relay only, inside `/egress`, with a short in-memory cache so a vault stays
//! under the plan's limits (the Arbos account is Individual: 1,000 reads/h per token, 1,000
//! requests/day for the account — decision 10). Vault-level Access with per-item host pinning.
//!
//! Env: `ARBOS_OP_BIN` (default `op`), `ARBOS_OP_CACHE_TTL_SECS` (resolved values; default 300),
//! `ARBOS_OP_ITEMS_TTL_SECS` (cached item list; default 3600), `ARBOS_OP_CONFIG_DIR` (default
//! `<data>/op`), `ARBOS_OP_READS_PER_HOUR` / `ARBOS_OP_READS_PER_DAY` (plan limits; defaults
//! 1000 / 1000). `ARBOS_OP_DISABLED=1` turns the provider off (`providers: []`).
//!
//! Remote flag `ARBOS_FEATURE_VAULT_CONNECT` (default **off**, Jacob 2026-10-21): while off the
//! relay advertises no provider (`providers: []`, `features.vault_connect: false` on `GET /secrets`
//! and `GET /me`) and `POST /vaults` answers 403 `feature_off`, so no app shows the Connect
//! 1Password flow. Vaults already connected keep working (list, resolve, access, token, refresh,
//! disconnect). Flip with `ARBOS_FEATURE_VAULT_CONNECT=1` in the relay env; no client release.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::api::{
    ProjectSecretItemDto, ProjectVaultSummaryDto, PushEventDto, SecretAccessDto,
    SecretAccessEnvelopeDto, SecretAccessScopeDto, SecretRequestPushPayloadDto, VaultDetailDto,
    VaultDto, VaultEnvelopeDto, VaultItemHostDto, VaultItemsEnvelopeDto, VaultLimitsDto,
};
use arbos_core::ids::{now_ms, ulid};

use crate::auth::Principal;
use crate::db_secrets::{Access, AuditEntry, AuditQuery, SecretRequest, Vault, REQUEST_PENDING_MS};
use crate::secrets::{
    self, audit, clean_hosts, device_user, err, internal, request_json, user_dek, ApiErr,
    ApiResult, Refusal, ACTIVITY_WINDOW_MS,
};
use crate::state::AppState;
use crate::vault::host_allowed;

pub const OP_SCHEME: &str = "op://";
const OP_TIMEOUT: Duration = Duration::from_secs(25);
/// Refuse before the plan refuses us: leave this much of each window unused.
const LIMIT_HEADROOM: f64 = 0.95;

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn op_bin() -> String {
    std::env::var("ARBOS_OP_BIN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "op".into())
}

fn provider_enabled() -> bool {
    !std::env::var("ARBOS_OP_DISABLED").is_ok_and(|v| v == "1")
}

/// `ARBOS_FEATURE_VAULT_CONNECT=1|true|on` lets users connect new vaults. Default off.
fn connect_enabled() -> bool {
    std::env::var("ARBOS_FEATURE_VAULT_CONNECT").is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        )
    })
}

/// `features.vault_connect`: may users connect new vaults on this relay.
/// Push when a vault `op://` secret is requested (always carries `vault_id`).
pub(crate) fn secret_request_push(
    request_id: impl Into<String>,
    project_id: impl Into<String>,
    project_name: impl Into<String>,
    name: impl Into<String>,
    host: Option<String>,
    use_kind: impl Into<String>,
    vault_id: impl Into<String>,
) -> PushEventDto {
    PushEventDto::SecretRequest(SecretRequestPushPayloadDto {
        request_id: request_id.into(),
        project_id: project_id.into(),
        project_name: project_name.into(),
        name: name.into(),
        host,
        use_kind: use_kind.into(),
        vault_id: Some(vault_id.into()),
    })
}

pub(crate) fn vault_connect() -> bool {
    provider_enabled() && connect_enabled()
}

/// Remote feature flags the apps read (`GET /secrets` and `GET /me` → `features`): the one
/// object from `crate::features`, so every flag ships on both endpoints.
#[allow(dead_code)] // kept as the documented alias; handlers use `crate::features::dto()`.
pub(crate) fn features() -> Value {
    crate::features::features()
}

fn cache_ttl() -> Duration {
    Duration::from_secs(env_u64("ARBOS_OP_CACHE_TTL_SECS", 300))
}

fn items_ttl_ms() -> i64 {
    env_u64("ARBOS_OP_ITEMS_TTL_SECS", 3600) as i64 * 1000
}

fn reads_per_hour() -> i64 {
    env_u64("ARBOS_OP_READS_PER_HOUR", 1000) as i64
}

fn reads_per_day() -> i64 {
    env_u64("ARBOS_OP_READS_PER_DAY", 1000) as i64
}

/// Providers this relay can connect (`GET /secrets` → `providers[]`).
pub(crate) fn providers(_state: &AppState) -> Vec<&'static str> {
    if provider_enabled() && connect_enabled() {
        vec!["onepassword"]
    } else {
        Vec::new()
    }
}

// ---- the `op` CLI -----------------------------------------------------------------------------

#[derive(Debug)]
enum OpError {
    BadToken,
    RateLimited,
    NotFound,
    Other(String),
}

impl OpError {
    fn classify(stderr: &str) -> Self {
        let s = stderr.to_ascii_lowercase();
        if s.contains("rate limit") || s.contains("rate-limit") || s.contains("429") {
            OpError::RateLimited
        } else if s.contains("isn't an item")
            || s.contains("not found")
            || s.contains("no item")
            || s.contains("doesn't exist")
            || s.contains("more than one item")
        {
            OpError::NotFound
        } else if s.contains("token")
            || s.contains("credentials")
            || s.contains("unauthor")
            || s.contains("401")
            || s.contains("403")
            || s.contains("session")
            || s.contains("invalid")
        {
            OpError::BadToken
        } else {
            OpError::Other(stderr.trim().chars().take(300).collect())
        }
    }

    fn api(&self) -> ApiErr {
        match self {
            OpError::BadToken => err(
                StatusCode::BAD_REQUEST,
                "bad_token",
                "That token didn't work. Check it was copied whole.",
            ),
            OpError::RateLimited => err(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "1Password rate limit reached — try again later.",
            ),
            OpError::NotFound => err(
                StatusCode::NOT_FOUND,
                "not_found",
                "no such item in the vault",
            ),
            OpError::Other(m) => err(
                StatusCode::BAD_GATEWAY,
                "provider_error",
                format!("1Password: {m}"),
            ),
        }
    }
}

/// Run one `op` command with the token in its environment only. stdout as text; stderr classified.
async fn op(state: &AppState, token: &str, args: &[&str]) -> Result<String, OpError> {
    let cfg_dir = std::env::var("ARBOS_OP_CONFIG_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| state.cfg.data_dir.join("op"));
    // `op` refuses a config dir anyone else can read.
    let _ = std::fs::create_dir_all(&cfg_dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&cfg_dir, std::fs::Permissions::from_mode(0o700));
    }
    let mut cmd = tokio::process::Command::new(op_bin());
    cmd.args(args)
        .env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into()),
        )
        .env("HOME", &cfg_dir)
        .env("OP_CONFIG_DIR", &cfg_dir)
        .env("OP_SERVICE_ACCOUNT_TOKEN", token)
        .env("OP_FORMAT", "json")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let out = tokio::time::timeout(OP_TIMEOUT, cmd.output())
        .await
        .map_err(|_| OpError::Other("op timed out".into()))?
        .map_err(|e| OpError::Other(format!("could not run op: {e}")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        // Drop `op`'s "Using configuration at non-standard location" notice before classifying.
        let stderr = String::from_utf8_lossy(&out.stderr);
        let meaningful: String = stderr
            .lines()
            .filter(|l| !l.starts_with("Using configuration at"))
            .collect::<Vec<_>>()
            .join("\n");
        Err(OpError::classify(&meaningful))
    }
}

/// Fields the agent can reference per 1Password category, without a per-item read.
fn fields_for_category(category: &str) -> Vec<&'static str> {
    match category.to_ascii_uppercase().as_str() {
        "LOGIN" => vec!["username", "password"],
        "API_CREDENTIAL" => vec!["credential", "username"],
        "PASSWORD" => vec!["password"],
        "SECURE_NOTE" => vec!["notesPlain"],
        "DATABASE" => vec!["username", "password", "hostname", "port", "database"],
        "SSH_KEY" => vec!["private_key", "public_key"],
        "CREDIT_CARD" => vec!["number", "cvv"],
        _ => vec!["password", "credential", "notesPlain"],
    }
}

/// `op vault list` → `[{id, name}]`.
async fn list_external_vaults(state: &AppState, token: &str) -> Result<Vec<Value>, OpError> {
    let out = op(state, token, &["vault", "list", "--format", "json"]).await?;
    let v: Value = serde_json::from_str(&out).map_err(|e| OpError::Other(e.to_string()))?;
    Ok(v.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|x| json!({ "id": x["id"], "name": x["name"] }))
        .collect())
}

/// `op item list --vault <id>` for every external vault → cached items, names only.
async fn list_items(
    state: &AppState,
    token: &str,
    external: &[Value],
) -> Result<Vec<Value>, OpError> {
    let mut items = Vec::new();
    for ev in external {
        let Some(vid) = ev["id"].as_str() else {
            continue;
        };
        let vname = ev["name"].as_str().unwrap_or(vid);
        let out = op(
            state,
            token,
            &["item", "list", "--vault", vid, "--format", "json"],
        )
        .await?;
        let v: Value = serde_json::from_str(&out).map_err(|e| OpError::Other(e.to_string()))?;
        for it in v.as_array().cloned().unwrap_or_default() {
            let title = it["title"].as_str().unwrap_or("").to_owned();
            let category = it["category"].as_str().unwrap_or("").to_owned();
            let urls: Vec<String> = it["urls"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|u| u["href"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let fields = fields_for_category(&category);
            let primary = fields.first().copied().unwrap_or("password");
            items.push(json!({
                "id": it["id"], "title": title, "category": category, "vault": { "id": vid, "name": vname },
                "ref": format!("{OP_SCHEME}{vname}/{title}/{primary}"),
                "fields": fields, "urls": urls,
            }));
        }
    }
    Ok(items)
}

type ValueCache = Mutex<HashMap<(String, String), (Instant, String)>>;

/// Resolved values, keyed by `(vault_id, reference)`, for `ARBOS_OP_CACHE_TTL_SECS`.
fn cache() -> &'static ValueCache {
    static C: OnceLock<ValueCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_flush(vault_id: &str) {
    cache()
        .lock()
        .expect("op cache")
        .retain(|(v, _), _| v != vault_id);
}

/// One plan-aware provider read, counted per vault.
async fn read_counted(
    state: &AppState,
    v: &Vault,
    token: &str,
    args: &[&str],
) -> Result<String, ApiErr> {
    let (hour, day) = state.db.vault_reads(&v.id).map_err(internal)?;
    if hour as f64 >= reads_per_hour() as f64 * LIMIT_HEADROOM
        || day as f64 >= reads_per_day() as f64 * LIMIT_HEADROOM
    {
        let secs = if hour as f64 >= reads_per_hour() as f64 * LIMIT_HEADROOM {
            3600 - (now_ms() / 1000) % 3600
        } else {
            86_400 - (now_ms() / 1000) % 86_400
        };
        return Err(ApiErr(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited".into(),
            format!(
                "1Password rate limit reached — try again in {} min.",
                (secs / 60).max(1)
            ),
        ));
    }
    let _ = state.db.count_vault_read(&v.id);
    op(state, token, args).await.map_err(|e| {
        if matches!(e, OpError::BadToken | OpError::RateLimited) {
            let mut v2 = v.clone();
            v2.status = if matches!(e, OpError::BadToken) {
                "token_invalid"
            } else {
                "rate_limited"
            }
            .into();
            v2.last_error = Some(e.api().2.clone());
            let _ = state.db.upsert_vault(&v2);
        }
        e.api()
    })
}

/// Decrypt the vault's token for one provider call.
fn vault_token(state: &AppState, v: &Vault) -> Result<String, ApiErr> {
    let enc = v.token_enc.as_deref().ok_or_else(|| {
        err(
            StatusCode::CONFLICT,
            "token_invalid",
            "this vault has no token; replace it",
        )
    })?;
    let dek = user_dek(state, &v.user_id).map_err(internal)?;
    dek.decrypt(v.id.as_bytes(), enc).map_err(internal)
}

/// Resolve one `op://…` reference for a vault (cache first). The value stays in this process.
async fn resolve(state: &AppState, v: &Vault, reference: &str) -> Result<String, ApiErr> {
    let key = (v.id.clone(), reference.to_owned());
    if let Some((at, val)) = cache().lock().expect("op cache").get(&key) {
        if at.elapsed() < cache_ttl() {
            return Ok(val.clone());
        }
    }
    let token = vault_token(state, v)?;
    let out = read_counted(state, v, &token, &["read", "--no-newline", reference]).await?;
    cache()
        .lock()
        .expect("op cache")
        .insert(key, (Instant::now(), out.clone()));
    Ok(out)
}

/// The vault an `op://<vault>/<item>/<field>` reference belongs to, by external name or id.
fn vault_for_ref<'a>(vaults: &'a [Vault], reference: &str) -> Option<&'a Vault> {
    let rest = reference.strip_prefix(OP_SCHEME)?;
    let vault_seg = rest.split('/').next()?.trim();
    vaults.iter().find(|v| {
        v.external_vaults.as_array().is_some_and(|a| {
            a.iter().any(|e| {
                e["name"]
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(vault_seg))
                    || e["id"].as_str() == Some(vault_seg)
            })
        })
    })
}

/// `op://…` references in `s`, in order (stops at whitespace, quotes and JSON punctuation).
pub fn find_refs(s: &str) -> Vec<String> {
    // Vault and item names may hold spaces (1Password allows them); the field ends at
    // whitespace, a quote or JSON punctuation.
    let is_term =
        |c: char| c == '\n' || matches!(c, '"' | '\'' | '<' | '>' | ',' | ';' | '}' | ']' | '\\');
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find(OP_SCHEME) {
        let after = &rest[i + OP_SCHEME.len()..];
        let mut segs: Vec<&str> = Vec::new();
        let mut cursor = after;
        let mut consumed = 0;
        for n in 0..3 {
            let end = if n < 2 {
                cursor.find(|c: char| c == '/' || is_term(c))
            } else {
                cursor.find(|c: char| c.is_whitespace() || c == '/' || is_term(c))
            }
            .unwrap_or(cursor.len());
            let seg = cursor[..end].trim();
            if seg.is_empty() || (n < 2 && !cursor[end..].starts_with('/')) {
                break;
            }
            segs.push(seg);
            let step = (end + usize::from(n < 2)).min(cursor.len());
            consumed += step;
            cursor = &cursor[step..];
        }
        if segs.len() == 3 {
            let token = format!("{OP_SCHEME}{}/{}/{}", segs[0], segs[1], segs[2]);
            if !out.contains(&token) {
                out.push(token);
            }
        }
        rest = &after[consumed.max(1).min(after.len())..];
    }
    out
}

// ---- JSON (Dto builders; wire JSON unchanged) --------------------------------------------------

fn to_json<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).expect("vault dto")
}

pub(crate) fn vault_dto(state: &AppState, v: &Vault) -> VaultDto {
    let (hour, day) = state.db.vault_reads(&v.id).unwrap_or((0, 0));
    let agents: Vec<SecretAccessDto> = state
        .db
        .accesses_of_target("vault", &v.id)
        .unwrap_or_default()
        .iter()
        .map(|a| access_dto(state, a))
        .collect();
    VaultDto {
        id: v.id.clone(),
        provider: v.provider.clone(),
        name: v.name.clone(),
        external_vaults: v.external_vaults.clone(),
        items_count: v.items_count,
        items_refreshed_at: v.items_refreshed_at,
        token_expires_at: v.token_expires_at,
        limits: VaultLimitsDto {
            reads_per_hour: reads_per_hour(),
            reads_used_hour: hour,
            reads_per_day: reads_per_day(),
            reads_used_day: day,
            cache_ttl_secs: cache_ttl().as_secs() as i64,
        },
        status: v.status.clone(),
        last_error: v.last_error.clone(),
        last_refreshed_at: v.last_refreshed_at,
        created_at: v.created_at,
        agents,
        // Cached names only (title, category, fields, urls): small, and the Mac shows them inline.
        items: Value::Array(items_json(state, v)),
        env_items: state
            .db
            .vault_item_envs(&v.id)
            .map(|e| e.len() as i64)
            .unwrap_or(0),
    }
}

#[allow(dead_code)] // Value form; HTTP handlers return DTOs, fixtures still use this.
pub(crate) fn vault_json(state: &AppState, v: &Vault) -> Value {
    to_json(vault_dto(state, v))
}

/// The cached items with the owner's env mapping on each: `env` (the name agents get the real
/// value under, or the name they would get) and `inject_env` (whether they do).
pub(crate) fn items_json(state: &AppState, v: &Vault) -> Vec<Value> {
    let envs = state.db.vault_item_envs(&v.id).unwrap_or_default();
    v.items
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut it| {
            let r = item_base(it["ref"].as_str().unwrap_or(""));
            match envs.iter().find(|(i, _)| *i == r) {
                Some((_, env)) => {
                    it["env"] = json!(env);
                    it["inject_env"] = json!(true);
                }
                None => {
                    it["env"] = json!(secrets::env_from_name(it["title"].as_str().unwrap_or("")));
                    it["inject_env"] = json!(false);
                }
            }
            it
        })
        .collect()
}

/// Variable names for one vault item's fields: `ENV` for the main field (credential, password,
/// note, key…), `ENV_FIELD` for the rest (`username`, `hostname`…).
pub(crate) fn item_env_vars(env: &str, fields: &[String]) -> Vec<(String, String)> {
    const PRIMARY: [&str; 6] = [
        "credential",
        "password",
        "notesPlain",
        "private_key",
        "number",
        "value",
    ];
    let primary = PRIMARY
        .iter()
        .find(|p| fields.iter().any(|f| f == *p))
        .map(|p| (*p).to_owned())
        .or_else(|| fields.first().cloned());
    fields
        .iter()
        .map(|f| {
            let name = if Some(f) == primary.as_ref() {
                env.to_owned()
            } else {
                format!("{env}_{}", f.to_ascii_uppercase())
            };
            (f.clone(), name)
        })
        .collect()
}

pub(crate) fn access_dto(state: &AppState, a: &Access) -> SecretAccessDto {
    let name = state
        .db
        .project(&a.project_id)
        .ok()
        .flatten()
        .map(|p| p.name)
        .unwrap_or_else(|| a.project_id.clone());
    let pinned: Vec<VaultItemHostDto> = state
        .db
        .vault_access_hosts(&a.id)
        .unwrap_or_default()
        .into_iter()
        .map(|(r, h)| VaultItemHostDto {
            item_ref: r,
            host: h,
        })
        .collect();
    SecretAccessDto {
        project_id: a.project_id.clone(),
        name,
        access_id: a.id.clone(),
        enabled: a.enabled,
        note: a.note.clone(),
        hosts: a.hosts.clone(),
        scope: SecretAccessScopeDto {
            kind: a.scope_kind.clone(),
            chat_id: a.scope_chat_id.clone(),
        },
        created_from: a.created_from.clone(),
        created_at: a.created_at,
        last_used_at: a.last_used_at,
        disabled_at: a.disabled_at,
        item_hosts: Some(pinned),
    }
}

#[allow(dead_code)] // Value form of [`access_dto`].
fn access_json(state: &AppState, a: &Access) -> Value {
    to_json(access_dto(state, a))
}

/// `GET /secrets` → `vaults[]`.
pub(crate) fn list_dto(state: &AppState, user_id: &str) -> Vec<VaultDto> {
    state
        .db
        .list_vaults(user_id)
        .unwrap_or_default()
        .iter()
        .map(|v| vault_dto(state, v))
        .collect()
}

#[allow(dead_code)] // Value form for callers that still want raw JSON arrays.
pub(crate) fn list_json(state: &AppState, user_id: &str) -> Vec<Value> {
    list_dto(state, user_id).into_iter().map(to_json).collect()
}

/// One vault-level access as an item of the agent profile page.
pub(crate) fn access_item_dto(
    state: &AppState,
    owner: &str,
    a: &Access,
) -> Option<ProjectSecretItemDto> {
    let v = state.db.vault(owner, &a.target_id).ok().flatten()?;
    let access = access_dto(state, a);
    Some(ProjectSecretItemDto {
        project_id: access.project_id,
        name: access.name,
        access_id: access.access_id,
        enabled: access.enabled,
        note: access.note,
        hosts: access.hosts,
        scope: access.scope,
        created_from: access.created_from,
        created_at: access.created_at,
        last_used_at: access.last_used_at,
        disabled_at: access.disabled_at,
        item_hosts: access.item_hosts,
        target_kind: Some("vault".into()),
        secret: None,
        vault: Some(ProjectVaultSummaryDto {
            id: v.id,
            provider: v.provider,
            name: v.name,
            items_count: v.items_count,
            status: v.status,
        }),
    })
}

#[allow(dead_code)] // Value form of [`access_item_dto`].
pub(crate) fn access_item_json(state: &AppState, owner: &str, a: &Access) -> Option<Value> {
    access_item_dto(state, owner, a).map(to_json)
}

/// Items of connected vaults this project may use (names only), one per cached item.
pub(crate) fn box_items(state: &AppState, user_id: &str, project_id: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for v in state.db.list_vaults(user_id).unwrap_or_default() {
        let access = state.db.access(project_id, "vault", &v.id).ok().flatten();
        let enabled = access.as_ref().is_some_and(|a| a.enabled);
        let envs = state.db.vault_item_envs(&v.id).unwrap_or_default();
        for it in v.items.as_array().cloned().unwrap_or_default() {
            let r = it["ref"].as_str().unwrap_or("").to_owned();
            let hosts: Vec<String> = access
                .as_ref()
                .map(|a| state.db.vault_item_hosts(&a.id, &r).unwrap_or_default())
                .unwrap_or_default();
            let fields: Vec<String> = it["fields"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|f| f.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let base = r
                .rsplit_once('/')
                .map(|(b, _)| b.to_owned())
                .unwrap_or(r.clone());
            let placeholders: serde_json::Map<String, Value> = fields
                .iter()
                .map(|f| (f.clone(), Value::String(format!("{base}/{f}"))))
                .collect();
            let env = envs
                .iter()
                .find(|(i, _)| *i == item_base(&r))
                .map(|(_, e)| e.clone());
            out.push(json!({
                "id": format!("{}:{}", v.id, it["id"].as_str().unwrap_or("")), "name": it["title"], "env": Value::Null,
                "inject_env": env.is_some(), "real_env": env,
                "real_vars": env.as_deref().map(|e| item_env_vars(e, &fields).into_iter().map(|(_, n)| n).collect::<Vec<_>>()),
                "kind": it["category"].as_str().map(|c| c.to_ascii_lowercase()).unwrap_or_default(),
                "note": "", "hosts": hosts, "ref": r, "fields": fields, "placeholders": placeholders,
                "vault_name": v.name, "vault_id": v.id, "access_id": access.as_ref().map(|a| a.id.clone()),
                "enabled": enabled && v.status == "ok", "ask_every_time": false, "expires_at": v.token_expires_at,
                "scope": access.as_ref().map(|a| json!({ "kind": a.scope_kind, "chat_id": a.scope_chat_id })),
                "urls": it["urls"],
                // The owner's note on the vault Access is the policy for every item in it.
                "use_notes": access.as_ref().map(|a| a.note.trim()).filter(|n| !n.is_empty()).map(|n| vec![n.to_owned()]).unwrap_or_default(),
            }));
        }
    }
    out
}

/// Real values of the connected-vault items this project's box puts in its environment: the
/// owner mapped the item to an env name, the agent's vault Access is on, the token is healthy.
/// Resolved here (cache first); a failed read skips the item and is logged, never fatal.
pub(crate) async fn box_env_items(state: &AppState, user_id: &str, project_id: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for v in state.db.list_vaults(user_id).unwrap_or_default() {
        if v.status != "ok" {
            continue;
        }
        let Some(access) = state
            .db
            .access(project_id, "vault", &v.id)
            .ok()
            .flatten()
            .filter(|a| a.enabled)
        else {
            continue;
        };
        let envs = state.db.vault_item_envs(&v.id).unwrap_or_default();
        if envs.is_empty() {
            continue;
        }
        for it in v.items.as_array().cloned().unwrap_or_default() {
            let base = item_base(it["ref"].as_str().unwrap_or(""));
            let Some((_, env)) = envs.iter().find(|(i, _)| *i == base) else {
                continue;
            };
            let fields: Vec<String> = it["fields"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|f| f.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let mut vars = serde_json::Map::new();
            for (field, name) in item_env_vars(env, &fields) {
                match resolve(state, &v, &format!("{base}/{field}")).await {
                    Ok(value) => {
                        vars.insert(name, Value::String(value));
                    }
                    Err(e) => {
                        tracing::warn!(vault = %v.name, item = %base, field, error = ?e.1, "vault item env: read failed");
                    }
                }
            }
            if vars.is_empty() {
                continue;
            }
            audit(
                state,
                user_id,
                AuditEntry {
                    project_id: Some(project_id.to_owned()),
                    access_id: Some(access.id.clone()),
                    use_kind: Some("env".into()),
                    detail: json!({ "ref": base, "env": env, "vars": vars.keys().collect::<Vec<_>>() }),
                    ..entry("injected", &v, &format!("agent:{project_id}"))
                },
            );
            out.push(json!({ "id": format!("{}:{}", v.id, it["id"].as_str().unwrap_or("")), "name": it["title"],
                "env": env, "kind": it["category"].as_str().map(|c| c.to_ascii_lowercase()).unwrap_or_default(),
                "ref": base, "vault_name": v.name, "vars": vars }));
        }
    }
    out
}

#[derive(Deserialize)]
pub struct ItemEnvBody {
    #[serde(rename = "ref")]
    pub reference: String,
    pub inject_env: bool,
    /// The env name; defaults to one derived from the item's title.
    #[serde(default)]
    pub env: Option<String>,
}

/// `PUT /vaults/{id}/items/env { ref, inject_env, env? }` → `{ vault, items }`: put one item's
/// real value into the environment of every agent with Access to this vault (or take it out).
pub async fn put_item_env(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<ItemEnvBody>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let v = load_vault(&state, &user_id, &id)?;
    let base = item_base(b.reference.trim());
    let item = v
        .items
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|it| item_base(it["ref"].as_str().unwrap_or("")) == base)
        })
        .cloned()
        .ok_or_else(|| {
            err(
                StatusCode::NOT_FOUND,
                "not_found",
                "no such item in this vault",
            )
        })?;
    let title = item["title"].as_str().unwrap_or("").to_owned();
    let env = if b.inject_env {
        let wanted = b
            .env
            .as_deref()
            .filter(|e| !e.trim().is_empty())
            .unwrap_or(&title);
        let (_, env) = secrets::validate_name(wanted)?;
        if state
            .db
            .secret_by_name(&user_id, &env)
            .map_err(internal)?
            .is_some()
            || state
                .db
                .vault_item_env_owner(&user_id, &env)
                .map_err(internal)?
                .is_some_and(|(vid, r)| vid != v.id || r != base)
        {
            return Err(err(
                StatusCode::CONFLICT,
                "already_exists",
                format!("${env} is already another secret's name."),
            ));
        }
        state
            .db
            .set_vault_item_env(&v.id, &base, &env)
            .map_err(internal)?;
        Some(env)
    } else {
        state
            .db
            .clear_vault_item_env(&v.id, &base)
            .map_err(internal)?;
        None
    };
    audit(
        &state,
        &user_id,
        AuditEntry {
            detail: json!({ "ref": base, "item": title, "env": env }),
            ..entry(
                if env.is_some() { "env_on" } else { "env_off" },
                &v,
                &format!("user:{user_id}"),
            )
        },
    );
    for a in state
        .db
        .accesses_of_target("vault", &v.id)
        .unwrap_or_default()
    {
        after_access_change(&state, &v, &a);
    }
    Ok(Json(VaultItemsEnvelopeDto {
        vault: vault_dto(&state, &v),
        items: Value::Array(items_json(&state, &v)),
    })
    .into_response())
}

pub(crate) fn admin_status(state: &AppState) -> Value {
    let paths = std::env::var("PATH").unwrap_or_default();
    let found = std::env::split_paths(&paths).any(|d| d.join(op_bin()).is_file())
        || std::path::Path::new(&op_bin()).is_file();
    json!({ "onepassword": { "available": provider_enabled() && found, "connect_enabled": connect_enabled(), "op_bin": op_bin(),
        "cache_ttl_secs": cache_ttl().as_secs(), "items_ttl_secs": items_ttl_ms() / 1000,
        "reads_per_hour": reads_per_hour(), "reads_per_day": reads_per_day(),
        "vaults": state.db.with(|c| c.query_row("SELECT COUNT(*) FROM vaults WHERE disconnected_at IS NULL", [], |r| r.get::<_, i64>(0))).unwrap_or(0) } })
}

fn entry(action: &str, v: &Vault, by: &str) -> AuditEntry {
    AuditEntry {
        action: action.into(),
        target_kind: "vault".into(),
        target_id: v.id.clone(),
        target_name: v.name.clone(),
        by: by.into(),
        detail: json!({}),
        ..Default::default()
    }
}

fn push_box(state: &AppState, project_id: &str, kind: &str, payload: Value) {
    let _ = state.db.enqueue_push(project_id, kind, &payload);
}

fn after_access_change(state: &AppState, v: &Vault, a: &Access) {
    push_box(
        state,
        &a.project_id,
        "secret_access_changed",
        json!({ "access_id": a.id, "ref": format!("{OP_SCHEME}{}/", v.name), "env": Value::Null, "name": v.name,
            "kind": "vault", "enabled": a.enabled, "hosts": a.hosts }),
    );
}

// ---- device: connect, read, token, refresh, access, disconnect -----------------------------

#[derive(Deserialize)]
pub struct ConnectBody {
    #[serde(default = "default_provider")]
    pub provider: String,
    pub token: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub agents: Vec<secrets::AgentPick>,
}

fn default_provider() -> String {
    "onepassword".into()
}

/// Validate a token with the provider and read what it can see. Returns `(external vaults, items)`.
async fn probe(state: &AppState, token: &str) -> Result<(Vec<Value>, Vec<Value>), ApiErr> {
    let external = list_external_vaults(state, token)
        .await
        .map_err(|e| e.api())?;
    if external.is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "no_vaults",
            "This token can read no vaults.",
        ));
    }
    let items = list_items(state, token, &external)
        .await
        .map_err(|e| e.api())?;
    Ok((external, items))
}

/// `POST /vaults` → 201 `{ vault }`. The token is checked with `op vault list` + one
/// `op item list` per vault, then stored encrypted; the box never sees it.
pub async fn connect(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<ConnectBody>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    if !connect_enabled() {
        return Err(err(
            StatusCode::FORBIDDEN,
            "feature_off",
            "Connecting a vault is turned off on this relay.",
        ));
    }
    if !provider_enabled() || b.provider != "onepassword" {
        return Err(err(
            StatusCode::NOT_IMPLEMENTED,
            "provider_unavailable",
            format!(
                "{}: connected vaults of this kind are not available on this relay",
                b.provider
            ),
        ));
    }
    let token = b.token.trim().to_owned();
    if token.len() < 20 || token.contains(char::is_whitespace) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_token",
            "That token didn't work. Check it was copied whole.",
        ));
    }
    let (external, items) = probe(&state, &token).await?;
    let default_name = external
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let name = b
        .name
        .map(|n| n.trim().chars().take(80).collect::<String>())
        .filter(|n| !n.is_empty())
        .unwrap_or(default_name);
    let dek = user_dek(&state, &user_id).map_err(internal)?;
    let id = ulid();
    let now = now_ms();
    let v = Vault {
        id: id.clone(),
        user_id: user_id.clone(),
        provider: "onepassword".into(),
        name,
        token_enc: Some(dek.encrypt(id.as_bytes(), &token).map_err(internal)?),
        external_vaults: Value::Array(external),
        items_count: items.len() as i64,
        items: Value::Array(items),
        items_refreshed_at: Some(now),
        token_expires_at: None,
        status: "ok".into(),
        last_error: None,
        last_refreshed_at: Some(now),
        created_at: now,
        disconnected_at: None,
    };
    state.db.upsert_vault(&v).map_err(internal)?;
    // The probe made 1 + n reads; count them so the Vault page is honest.
    for _ in 0..=v.external_vaults.as_array().map(Vec::len).unwrap_or(0) {
        let _ = state.db.count_vault_read(&v.id);
    }
    audit(
        &state,
        &user_id,
        AuditEntry {
            detail: json!({ "provider": v.provider, "vaults": v.external_vaults.as_array().map(Vec::len).unwrap_or(0), "items": v.items_count }),
            ..entry("vault_connected", &v, &format!("user:{user_id}"))
        },
    );
    for pick in &b.agents {
        if let Some(a) = grant_access(
            &state,
            &user_id,
            &v,
            &pick.project_id,
            pick.note.as_deref(),
            true,
            "settings",
        )? {
            after_access_change(&state, &v, &a);
        }
    }
    Ok((
        StatusCode::CREATED,
        Json(VaultEnvelopeDto {
            vault: vault_dto(&state, &v),
        }),
    )
        .into_response())
}

fn grant_access(
    state: &AppState,
    user_id: &str,
    v: &Vault,
    project_id: &str,
    note: Option<&str>,
    enabled: bool,
    created_from: &str,
) -> Result<Option<Access>, ApiErr> {
    let Some(p) = state.db.project(project_id).map_err(internal)? else {
        return Err(err(StatusCode::NOT_FOUND, "not_found", "no such agent"));
    };
    if p.user_id != user_id {
        return Err(err(StatusCode::FORBIDDEN, "not_owner", "not your agent"));
    }
    let existing = state.db.access(&p.id, "vault", &v.id).map_err(internal)?;
    let was_on = existing.as_ref().is_some_and(|a| a.enabled);
    let now = now_ms();
    let a = Access {
        id: existing.as_ref().map(|a| a.id.clone()).unwrap_or_else(ulid),
        user_id: user_id.to_owned(),
        project_id: p.id.clone(),
        target_kind: "vault".into(),
        target_id: v.id.clone(),
        note: note
            .map(|n| n.chars().take(500).collect())
            .or_else(|| existing.as_ref().map(|a| a.note.clone()))
            .unwrap_or_default(),
        hosts: existing
            .as_ref()
            .map(|a| a.hosts.clone())
            .unwrap_or_default(),
        scope_kind: existing
            .as_ref()
            .map(|a| a.scope_kind.clone())
            .unwrap_or_else(|| "all".into()),
        scope_chat_id: existing.as_ref().and_then(|a| a.scope_chat_id.clone()),
        enabled,
        created_from: existing
            .as_ref()
            .map(|a| a.created_from.clone())
            .unwrap_or_else(|| created_from.to_owned()),
        created_at: existing.as_ref().map(|a| a.created_at).unwrap_or(now),
        disabled_at: if enabled {
            None
        } else {
            existing.as_ref().and_then(|a| a.disabled_at).or(Some(now))
        },
        last_used_at: existing.as_ref().and_then(|a| a.last_used_at),
    };
    state.db.upsert_access(&a).map_err(internal)?;
    if was_on != enabled || existing.is_none() {
        audit(
            state,
            user_id,
            AuditEntry {
                project_id: Some(p.id.clone()),
                access_id: Some(a.id.clone()),
                detail: json!({ "note": a.note, "created_from": created_from }),
                ..entry(
                    if enabled { "access_on" } else { "access_off" },
                    v,
                    &format!("user:{user_id}"),
                )
            },
        );
    }
    Ok(Some(a))
}

fn load_vault(state: &AppState, user_id: &str, id: &str) -> Result<Vault, ApiErr> {
    state
        .db
        .vault(user_id, id)
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such vault"))
}

/// `GET /vaults/{id}` → `{ vault, items, activity }` (items from the cache; refreshed when stale).
pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let mut v = load_vault(&state, &user_id, &id)?;
    if v.items_refreshed_at
        .is_none_or(|t| now_ms() - t > items_ttl_ms())
        && v.status == "ok"
    {
        if let Ok(v2) = refresh_items(&state, &v).await {
            v = v2;
        }
    }
    let activity = secrets::activity(
        &state,
        &user_id,
        AuditQuery {
            target_id: Some(v.id.clone()),
            since: Some(now_ms() - ACTIVITY_WINDOW_MS),
            limit: 50,
            ..Default::default()
        },
    );
    Ok(Json(VaultDetailDto {
        vault: vault_dto(&state, &v),
        items: Value::Array(items_json(&state, &v)),
        activity,
    })
    .into_response())
}

/// Re-read the item list (1 + n provider reads) and clear the value cache.
async fn refresh_items(state: &AppState, v: &Vault) -> Result<Vault, ApiErr> {
    let token = vault_token(state, v)?;
    let external =
        match read_counted(state, v, &token, &["vault", "list", "--format", "json"]).await {
            Ok(out) => serde_json::from_str::<Value>(&out)
                .ok()
                .and_then(|x| x.as_array().cloned())
                .unwrap_or_default()
                .iter()
                .map(|x| json!({ "id": x["id"], "name": x["name"] }))
                .collect::<Vec<_>>(),
            Err(e) => return Err(e),
        };
    let mut items = Vec::new();
    for ev in &external {
        let _ = state.db.count_vault_read(&v.id);
        items.extend(
            list_items(state, &token, std::slice::from_ref(ev))
                .await
                .map_err(|e| e.api())?,
        );
    }
    let mut v2 = v.clone();
    v2.external_vaults = Value::Array(external);
    v2.items_count = items.len() as i64;
    v2.items = Value::Array(items);
    v2.items_refreshed_at = Some(now_ms());
    v2.last_refreshed_at = Some(now_ms());
    v2.status = "ok".into();
    v2.last_error = None;
    state.db.upsert_vault(&v2).map_err(internal)?;
    cache_flush(&v.id);
    audit(
        state,
        &v.user_id,
        AuditEntry {
            detail: json!({ "items": v2.items_count }),
            ..entry("vault_refreshed", &v2, "system")
        },
    );
    Ok(v2)
}

/// `POST /vaults/{id}/refresh` → `{ vault }`.
pub async fn refresh(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let v = load_vault(&state, &user_id, &id)?;
    let v2 = refresh_items(&state, &v).await?;
    Ok(Json(VaultEnvelopeDto {
        vault: vault_dto(&state, &v2),
    })
    .into_response())
}

#[derive(Deserialize)]
pub struct TokenBody {
    pub token: String,
}

/// `PUT /vaults/{id}/token` `{ token }` → `{ vault }` (validated first; cache flushed).
pub async fn replace_token(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<TokenBody>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let v = load_vault(&state, &user_id, &id)?;
    let token = b.token.trim().to_owned();
    let (external, items) = probe(&state, &token).await?;
    let dek = user_dek(&state, &user_id).map_err(internal)?;
    let mut v2 = v.clone();
    v2.token_enc = Some(dek.encrypt(v.id.as_bytes(), &token).map_err(internal)?);
    v2.external_vaults = Value::Array(external);
    v2.items_count = items.len() as i64;
    v2.items = Value::Array(items);
    v2.items_refreshed_at = Some(now_ms());
    v2.last_refreshed_at = Some(now_ms());
    v2.status = "ok".into();
    v2.last_error = None;
    state.db.upsert_vault(&v2).map_err(internal)?;
    cache_flush(&v.id);
    audit(
        &state,
        &user_id,
        AuditEntry {
            detail: json!({ "token_replaced": true }),
            ..entry("vault_refreshed", &v2, &format!("user:{user_id}"))
        },
    );
    Ok(Json(VaultEnvelopeDto {
        vault: vault_dto(&state, &v2),
    })
    .into_response())
}

#[derive(Deserialize, Default)]
pub struct AccessBody {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub note: Option<String>,
}

fn default_true() -> bool {
    true
}

/// `PUT /vaults/{id}/access/{project_id}` `{ enabled, note? }` → `{ access }`.
pub async fn put_access(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, project_id)): Path<(String, String)>,
    body: Option<Json<AccessBody>>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let b = body.map(|b| b.0).unwrap_or_default();
    let v = load_vault(&state, &user_id, &id)?;
    let a = grant_access(
        &state,
        &user_id,
        &v,
        &project_id,
        b.note.as_deref(),
        b.enabled,
        "settings",
    )?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such agent"))?;
    after_access_change(&state, &v, &a);
    Ok(Json(SecretAccessEnvelopeDto {
        access: access_dto(&state, &a),
    })
    .into_response())
}

/// `DELETE /vaults/{id}` → 204. Token ciphertext and cached items go, every access is removed,
/// the cache is flushed; the audit stays.
pub async fn disconnect(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let v = load_vault(&state, &user_id, &id)?;
    let accesses = state
        .db
        .accesses_of_target("vault", &v.id)
        .unwrap_or_default();
    state.db.disconnect_vault(&user_id, &id).map_err(internal)?;
    cache_flush(&v.id);
    audit(
        &state,
        &user_id,
        AuditEntry {
            detail: json!({ "agents": accesses.len() }),
            ..entry("vault_disconnected", &v, &format!("user:{user_id}"))
        },
    );
    for mut a in accesses {
        a.enabled = false;
        after_access_change(&state, &v, &a);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---- the rule for `op://` references ------------------------------------------------------------

/// What one `op://` use was allowed by.
pub(crate) struct OpAllowed {
    pub vault: Vault,
    pub access: Option<Access>,
    pub once: Option<SecretRequest>,
    pub pin_host: bool,
}

/// Access on, scope matches, the item's pinned hosts include `host` (or none yet → pin), vault
/// token healthy. `host` empty skips the host check (a request without a host).
#[allow(clippy::result_large_err)]
pub(crate) fn decide_op_use(
    state: &AppState,
    user_id: &str,
    reference: &str,
    project_id: &str,
    host: &str,
    use_kind: &str,
    chat_id: Option<&str>,
) -> Result<OpAllowed, (Refusal, Option<Vault>)> {
    let vaults = state.db.list_vaults(user_id).unwrap_or_default();
    let Some(v) = vault_for_ref(&vaults, reference).cloned() else {
        return Err((Refusal::NoSuch, None));
    };
    if v.status != "ok" || v.token_enc.is_none() {
        return Err((Refusal::Expired, Some(v)));
    }
    let item_ref = item_base(reference);
    let access = state.db.access(project_id, "vault", &v.id).ok().flatten();
    if let Some(once) = state
        .db
        .allowed_once_for(project_id, &v.id, Some(host), use_kind)
        .ok()
        .flatten()
        .filter(|r| item_base(&r.target_ref) == item_ref)
    {
        return Ok(OpAllowed {
            vault: v,
            access,
            once: Some(once),
            pin_host: false,
        });
    }
    let Some(a) = access else {
        return Err((Refusal::NoAccess, Some(v)));
    };
    if !a.enabled {
        return Err((Refusal::TurnedOff, Some(v)));
    }
    if a.scope_kind == "chat" && a.scope_chat_id.as_deref() != chat_id {
        return Err((Refusal::WrongChat, Some(v)));
    }
    let mut hosts = state
        .db
        .vault_item_hosts(&a.id, &item_ref)
        .unwrap_or_default();
    hosts.extend(a.hosts.iter().cloned());
    if hosts.is_empty() {
        return Ok(OpAllowed {
            vault: v,
            access: Some(a),
            once: None,
            pin_host: !host.is_empty(),
        });
    }
    if !host.is_empty() && !host_allowed(host, &hosts) {
        return Err((Refusal::HostNotAllowed(hosts), Some(v)));
    }
    Ok(OpAllowed {
        vault: v,
        access: Some(a),
        once: None,
        pin_host: false,
    })
}

/// `op://vault/item/field` → `op://vault/item` (the pin is per item, not per field).
fn item_base(reference: &str) -> String {
    let parts: Vec<&str> = reference.splitn(5, '/').collect();
    if parts.len() >= 5 {
        format!("{}/{}/{}", parts[0], parts[2], parts[3]).replacen("op:/", "op://", 1)
    } else {
        reference.to_owned()
    }
}

fn short_ref(reference: &str) -> String {
    reference
        .strip_prefix(OP_SCHEME)
        .map(|r| {
            let mut it = r.split('/');
            let vault = it.next().unwrap_or("");
            let item = it.next().unwrap_or("");
            format!("{item} from {vault}")
        })
        .unwrap_or_else(|| reference.to_owned())
}

/// After an allowed use: spend a one-time grant, pin the first host, audit.
pub(crate) fn after_op_use(
    state: &AppState,
    reference: &str,
    project_id: &str,
    host: &str,
    use_kind: &str,
    chat_id: Option<&str>,
    allowed: &OpAllowed,
) {
    if let Some(once) = &allowed.once {
        let _ = state.db.consume_allowed_once(&once.id);
    }
    let v = &allowed.vault;
    if allowed.pin_host {
        if let Some(a) = &allowed.access {
            let _ = state
                .db
                .pin_vault_item_host(&a.id, &item_base(reference), host);
            let agent = state
                .db
                .project(project_id)
                .ok()
                .flatten()
                .map(|p| p.name)
                .unwrap_or_else(|| "The agent".into());
            push_box(
                state,
                project_id,
                "secret_notice",
                json!({ "ref": reference, "env": Value::Null, "host": host, "chat_id": chat_id,
                    "text": format!("{agent} used {} at {host}.", short_ref(reference)) }),
            );
            audit(
                state,
                &v.user_id,
                AuditEntry {
                    project_id: Some(project_id.to_owned()),
                    chat_id: chat_id.map(str::to_owned),
                    access_id: Some(a.id.clone()),
                    host: Some(host.to_owned()),
                    detail: json!({ "pinned": true, "ref": item_base(reference) }),
                    ..entry("access_on", v, &format!("agent:{project_id}"))
                },
            );
        }
    }
    if let Some(a) = &allowed.access {
        let _ = state.db.with(|c| {
            c.execute(
                "UPDATE secret_access SET last_used_at = ?2 WHERE id = ?1",
                rusqlite::params![a.id, now_ms()],
            )
        });
    }
    audit(
        state,
        &v.user_id,
        AuditEntry {
            project_id: Some(project_id.to_owned()),
            chat_id: chat_id.map(str::to_owned),
            request_id: allowed.once.as_ref().map(|r| r.id.clone()),
            access_id: allowed.access.as_ref().map(|a| a.id.clone()),
            use_kind: Some(use_kind.to_owned()),
            host: Some(host.to_owned()),
            detail: json!({ "ref": item_base(reference) }),
            ..entry("used", v, &format!("agent:{project_id}"))
        },
    );
}

fn audit_op_refusal(
    state: &AppState,
    user_id: &str,
    reference: &str,
    v: Option<&Vault>,
    project_id: &str,
    host: &str,
    why: &Refusal,
) {
    audit(
        state,
        user_id,
        AuditEntry {
            action: "refused".into(),
            target_kind: "vault".into(),
            target_id: v.map(|v| v.id.clone()).unwrap_or_default(),
            target_name: v
                .map(|v| v.name.clone())
                .unwrap_or_else(|| short_ref(reference)),
            project_id: Some(project_id.to_owned()),
            use_kind: Some("header".into()),
            host: Some(host.to_owned()),
            by: format!("agent:{project_id}"),
            detail: json!({ "reason": why.kind(), "ref": item_base(reference) }),
            ..Default::default()
        },
    )
}

/// `/egress` step for `op://` references: `(reference → value)` to substitute, refusals, values
/// to redact, and the uses to settle after the request goes out.
pub(crate) struct OpPlan {
    pub values: Vec<(String, String)>,
    pub refused: Vec<(String, String)>,
    pub used: Vec<(String, OpAllowed)>,
}

pub(crate) async fn plan_egress(
    state: &AppState,
    user_id: &str,
    project_id: &str,
    host: &str,
    chat_id: Option<&str>,
    texts: &[&str],
) -> OpPlan {
    let mut plan = OpPlan {
        values: Vec::new(),
        refused: Vec::new(),
        used: Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    for t in texts {
        for r in find_refs(t) {
            if !seen.insert(r.clone()) {
                continue;
            }
            match decide_op_use(state, user_id, &r, project_id, host, "header", chat_id) {
                Ok(allowed) => match resolve(state, &allowed.vault, &r).await {
                    Ok(value) => {
                        plan.values.push((r.clone(), value));
                        plan.used.push((r, allowed));
                    }
                    Err(e) => plan.refused.push((r, e.2)),
                },
                Err((why, v)) => {
                    let name = v
                        .as_ref()
                        .map(|v| v.name.clone())
                        .unwrap_or_else(|| short_ref(&r));
                    let msg = match &why {
                        Refusal::NoSuch => format!("{r}: no connected vault holds this reference."),
                        Refusal::NoAccess => {
                            format!("{r}: this agent has no access to vault {name}.")
                        }
                        Refusal::Expired => format!("{r}: vault {name} needs a new token."),
                        other => other.message(&short_ref(&r), host),
                    };
                    audit_op_refusal(state, user_id, &r, v.as_ref(), project_id, host, &why);
                    plan.refused.push((r, msg));
                }
            }
        }
    }
    plan
}

/// `POST /box/secrets/request` with an `op://` reference: standing access → 200; else a card.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn box_request(
    state: &AppState,
    user_id: &str,
    project_id: &str,
    reference: &str,
    purpose: &str,
    use_kind: &str,
    host: Option<String>,
    chat_id: &str,
) -> Result<Response, ApiErr> {
    let vaults = state.db.list_vaults(user_id).map_err(internal)?;
    let Some(v) = vault_for_ref(&vaults, reference).cloned() else {
        return Err(err(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("{reference}: no connected vault holds this reference"),
        ));
    };
    let host_s = host.clone().unwrap_or_default();
    let chat = if chat_id.is_empty() {
        None
    } else {
        Some(chat_id)
    };
    if let Ok(allowed) = decide_op_use(
        state, user_id, reference, project_id, &host_s, use_kind, chat,
    ) {
        if allowed.once.is_none() {
            if let Some(a) = allowed.access.clone() {
                let now = now_ms();
                let rq = SecretRequest {
                    id: ulid(),
                    user_id: user_id.to_owned(),
                    project_id: project_id.to_owned(),
                    chat_id: chat_id.to_owned(),
                    target_kind: "vault".into(),
                    target_id: Some(v.id.clone()),
                    target_ref: reference.to_owned(),
                    purpose: purpose.chars().take(400).collect(),
                    use_kind: use_kind.to_owned(),
                    host: host.clone(),
                    status: "allowed_always".into(),
                    decided_by: Some(format!("rule:{}", a.id)),
                    created_at: now,
                    decided_at: Some(now),
                    expires_at: i64::MAX,
                    asked_by: None,
                };
                state.db.insert_secret_request(&rq).map_err(internal)?;
                if allowed.pin_host {
                    after_op_use(
                        state, reference, project_id, &host_s, use_kind, chat, &allowed,
                    );
                }
                return Ok(Json(json!({ "request": request_json(state, &rq), "status": "allowed_always", "access_id": a.id })).into_response());
            }
        }
    }
    if let Some(rq) = state
        .db
        .pending_secret_request_for(project_id, reference, use_kind)
        .map_err(internal)?
    {
        return Ok((StatusCode::CREATED, Json(json!({ "request": request_json(state, &rq), "status": "pending", "resumed": true }))).into_response());
    }
    let now = now_ms();
    let rq = SecretRequest {
        id: ulid(),
        user_id: user_id.to_owned(),
        project_id: project_id.to_owned(),
        chat_id: chat_id.to_owned(),
        target_kind: "vault".into(),
        target_id: Some(v.id.clone()),
        target_ref: reference.to_owned(),
        purpose: purpose.chars().take(400).collect(),
        use_kind: use_kind.to_owned(),
        host: host.clone(),
        status: "pending".into(),
        decided_by: None,
        created_at: now,
        decided_at: None,
        expires_at: now + REQUEST_PENDING_MS,
        asked_by: None,
    };
    state.db.insert_secret_request(&rq).map_err(internal)?;
    audit(
        state,
        user_id,
        AuditEntry {
            project_id: Some(project_id.to_owned()),
            chat_id: chat.map(str::to_owned),
            request_id: Some(rq.id.clone()),
            use_kind: Some(use_kind.to_owned()),
            host: host.clone(),
            detail: json!({ "purpose": rq.purpose, "ref": item_base(reference) }),
            ..entry("requested", &v, &format!("agent:{project_id}"))
        },
    );
    let agent = state
        .db
        .project(project_id)
        .ok()
        .flatten()
        .map(|p| p.name)
        .unwrap_or_default();
    let _ = state.db.push_user_event(
        user_id,
        &secret_request_push(
            rq.id.clone(),
            project_id.to_owned(),
            agent,
            short_ref(reference),
            host.clone(),
            use_kind.to_owned(),
            v.id.clone(),
        ),
        None,
    );
    Ok((
        StatusCode::CREATED,
        Json(json!({ "request": request_json(state, &rq), "status": "pending" })),
    )
        .into_response())
}

/// A card decision on an `op://` request: `chat`/`always` turn the vault Access on (creating it
/// with the purpose as note) and pin the request's host for that item.
pub(crate) async fn decide_vault_request(
    state: &AppState,
    user_id: &str,
    rq: &SecretRequest,
    status: &str,
) -> Result<Option<Access>, ApiErr> {
    let Some(vid) = rq.target_id.as_deref() else {
        return Ok(None);
    };
    let v = load_vault(state, user_id, vid)?;
    audit(
        state,
        user_id,
        AuditEntry {
            project_id: Some(rq.project_id.clone()),
            chat_id: Some(rq.chat_id.clone()),
            request_id: Some(rq.id.clone()),
            use_kind: Some(rq.use_kind.clone()),
            host: rq.host.clone(),
            detail: json!({ "decision": status, "ref": item_base(&rq.target_ref) }),
            ..entry("decided", &v, &format!("card:{}", rq.id))
        },
    );
    if !matches!(status, "allowed_chat" | "allowed_always") {
        return Ok(None);
    }
    let mut a = grant_access(
        state,
        user_id,
        &v,
        &rq.project_id,
        Some(&rq.purpose),
        true,
        &format!("card:{}", rq.id),
    )?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such agent"))?;
    if status == "allowed_chat" {
        a.scope_kind = "chat".into();
        a.scope_chat_id = Some(rq.chat_id.clone());
    } else {
        a.scope_kind = "all".into();
        a.scope_chat_id = None;
    }
    state.db.upsert_access(&a).map_err(internal)?;
    if let Some(h) = rq.host.as_deref() {
        let _ = state
            .db
            .pin_vault_item_host(&a.id, &item_base(&rq.target_ref), h);
    }
    after_access_change(state, &v, &a);
    Ok(Some(a))
}

/// Hosts a device may pin or widen for one agent × item: `PUT /vaults/{id}/access/{project_id}/hosts`
/// `{ ref, hosts }`.
#[derive(Deserialize)]
pub struct ItemHostsBody {
    #[serde(rename = "ref")]
    pub reference: String,
    pub hosts: Vec<String>,
}

pub async fn put_item_hosts(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, project_id)): Path<(String, String)>,
    Json(b): Json<ItemHostsBody>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let v = load_vault(&state, &user_id, &id)?;
    let a = state
        .db
        .access(&project_id, "vault", &v.id)
        .map_err(internal)?
        .ok_or_else(|| {
            err(
                StatusCode::NOT_FOUND,
                "not_found",
                "no access for this agent",
            )
        })?;
    let hosts = clean_hosts(&b.hosts)?;
    let base = item_base(&b.reference);
    state
        .db
        .with(|c| {
            c.execute(
                "DELETE FROM vault_access_hosts WHERE access_id = ?1 AND item_ref = ?2",
                rusqlite::params![a.id, base],
            )
        })
        .map_err(internal)?;
    for h in &hosts {
        state
            .db
            .pin_vault_item_host(&a.id, &base, h)
            .map_err(internal)?;
    }
    audit(
        &state,
        &user_id,
        AuditEntry {
            project_id: Some(project_id.clone()),
            access_id: Some(a.id.clone()),
            detail: json!({ "ref": base, "hosts": hosts }),
            ..entry("updated", &v, &format!("user:{user_id}"))
        },
    );
    Ok(Json(SecretAccessEnvelopeDto {
        access: access_dto(&state, &a),
    })
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_env_vars_name_the_main_field_plainly() {
        let api = ["credential".to_owned(), "username".to_owned()];
        assert_eq!(
            item_env_vars("STRIPE", &api),
            vec![
                ("credential".to_owned(), "STRIPE".to_owned()),
                ("username".to_owned(), "STRIPE_USERNAME".to_owned())
            ]
        );
        let note = ["notesPlain".to_owned()];
        assert_eq!(
            item_env_vars("RUNBOOK", &note),
            vec![("notesPlain".to_owned(), "RUNBOOK".to_owned())]
        );
        let db = [
            "username".to_owned(),
            "password".to_owned(),
            "hostname".to_owned(),
        ];
        let names: Vec<String> = item_env_vars("PG", &db)
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        assert_eq!(names, vec!["PG_USERNAME", "PG", "PG_HOSTNAME"]);
    }

    #[test]
    fn refs_are_found_and_pinned_per_item() {
        let text = r#"{"Authorization":"Bearer op://Arbos — Research/Stripe/credential","x":"op://Arbos — Research/Postgres/url"} op://short"#;
        let refs = find_refs(text);
        assert_eq!(
            refs,
            vec![
                "op://Arbos — Research/Stripe/credential".to_owned(),
                "op://Arbos — Research/Postgres/url".to_owned()
            ]
        );
        assert_eq!(
            item_base("op://Arbos — Research/Stripe/credential"),
            "op://Arbos — Research/Stripe"
        );
        assert_eq!(item_base("op://v/i"), "op://v/i");
        assert_eq!(
            short_ref("op://Arbos — Research/Stripe/credential"),
            "Stripe from Arbos — Research"
        );
        assert_eq!(fields_for_category("LOGIN"), vec!["username", "password"]);
        assert_eq!(fields_for_category("api_credential")[0], "credential");
    }

    #[test]
    fn op_errors_are_classified() {
        assert!(matches!(
            OpError::classify("[ERROR] 429 rate limit exceeded"),
            OpError::RateLimited
        ));
        assert!(matches!(
            OpError::classify("[ERROR] invalid service account token"),
            OpError::BadToken
        ));
        assert!(matches!(
            OpError::classify("[ERROR] 2026/10/18 failed to DecodeSACredentials: illegal base64"),
            OpError::BadToken
        ));
        assert!(matches!(
            OpError::classify("[ERROR] \"Stripe\" isn't an item in the \"X\" vault"),
            OpError::NotFound
        ));
        assert_eq!(
            OpError::classify("[ERROR] 429 rate limit exceeded").api().1,
            "rate_limited"
        );
    }
}
