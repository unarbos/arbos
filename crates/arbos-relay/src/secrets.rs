//! Secrets (D83, `docs/spec-secrets.md`): named values an agent may use but never read.
//!
//! Three words: a **Secret** (name, note, write-only value, hosts), **Access** (one switch per
//! secret × agent, with a note), and **Pause all secrets** (the kill switch). The relay is the
//! authority at the moment of use: `/egress` swaps a placeholder for the value only when Access
//! is on, the host is allowed and the user is not paused; values live as ciphertext under a
//! per-user DEK wrapped by the relay key (§7.2), are decrypted inside one request and dropped,
//! and are redacted from every reply. Every event lands in the append-only audit log (§7.4).
//!
//! The F8 `/vault/*` endpoints in `vault.rs` are thin shims over this model for one release.

use std::collections::{BTreeMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::api::{
    DecideSecretResponseDto, MessageRedactedPayloadDto, ProjectSecretItemDto,
    ProjectSecretSummaryDto, ProjectSecretsDto, PushEventDto, SecretAccessDto,
    SecretAccessEnvelopeDto, SecretAccessScopeDto, SecretActivityDto, SecretActivityPageDto,
    SecretDetailDto, SecretDirectorDto, SecretDto, SecretEnvelopeDto, SecretExpiringPayloadDto,
    SecretRequestDto, SecretRequestPushPayloadDto, SecretRequestTargetDto, SecretRequestsDto,
    SecretSourceDto, SecretsOverviewDto, SecretsPausedDto, SecretsPausedPayloadDto,
};
use arbos_core::ids::{now_ms, ulid};

use crate::auth::Principal;
use crate::connectors::crypto::Dek;
use crate::db_secrets::{
    Access, AuditEntry, AuditQuery, Secret, SecretAudit, SecretDirector, SecretRequest,
    SecretVersion, SecretsSettings, REQUEST_PENDING_MS,
};
use crate::signin::api_error;
use crate::state::AppState;
use crate::vault::{host_allowed, PLACEHOLDER_SCHEME};

pub const KINDS: [&str; 3] = ["key", "login", "text"];
pub const USES: [&str; 3] = ["header", "env", "browser_fill"];
/// Window after a decision in which `browser_fill` may fetch login fields.
pub const FILL_WINDOW_MS: i64 = 10 * 60 * 1000;
/// Activity shown in the UI (kept forever, D76 Q6).
pub const ACTIVITY_WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// `secret_expiring` goes out this long before `expires_at`.
const EXPIRING_NOTICE_MS: i64 = 3 * 24 * 60 * 60 * 1000;
pub const ARBOS_VAULT_NAME: &str = "Arbos";

/// A refused call: [`arbos_core::api::ApiErrorDto`] with its status. Small, so `Result` stays
/// cheap to pass around (clippy `result_large_err`).
#[derive(Debug)]
pub struct ApiErr(pub StatusCode, pub String, pub String);

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        api_error(self.0, &self.1, self.2)
    }
}

pub(crate) type ApiResult = Result<Response, ApiErr>;

pub(crate) fn err(status: StatusCode, kind: &str, msg: impl Into<String>) -> ApiErr {
    ApiErr(status, kind.to_owned(), msg.into())
}

pub(crate) fn internal(e: impl std::fmt::Display) -> ApiErr {
    err(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
}

fn not_found() -> ApiErr {
    err(StatusCode::NOT_FOUND, "not_found", "no such secret")
}

/// The user a device call acts for; boxes and the admin token have no Settings.
pub(crate) fn device_user(principal: &Principal) -> Result<String, ApiErr> {
    match principal {
        Principal::Device { user_id, .. } => Ok(user_id.clone()),
        Principal::Box { .. } | Principal::Admin => Err(err(
            StatusCode::FORBIDDEN,
            "forbidden",
            "device token required",
        )),
    }
}

/// `(user, project)` for a box call: `X-Arbos-Project` must belong to the box's user.
pub(crate) fn box_project(
    state: &AppState,
    principal: &Principal,
    headers: &HeaderMap,
) -> Result<(String, String), ApiErr> {
    let Principal::Box { user_id } = principal else {
        return Err(err(
            StatusCode::FORBIDDEN,
            "forbidden",
            "box token required (the agent asks, the user decides)",
        ));
    };
    let id = headers
        .get("x-arbos-project")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "X-Arbos-Project header required",
            )
        })?;
    let p = state
        .db
        .project(id)
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_request", "no such project"))?;
    if p.user_id != *user_id {
        return Err(err(StatusCode::FORBIDDEN, "not_owner", "not your project"));
    }
    Ok((user_id.clone(), p.id))
}

// ---- names, env names, hosts -------------------------------------------------------------------

/// `stripe live` → `STRIPE_LIVE`; a leading digit gets a `_`.
pub fn env_from_name(name: &str) -> String {
    let mut s = String::new();
    for ch in name.trim().chars() {
        s.push(if ch.is_ascii_alphanumeric() {
            ch.to_ascii_uppercase()
        } else {
            '_'
        });
    }
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    let s = s.trim_matches('_').to_owned();
    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("_{s}")
    } else {
        s
    }
}

/// 1–64 chars: letters, digits, space, `_`, `-`, `.`; must leave a non-empty env name.
pub fn validate_name(name: &str) -> Result<(String, String), ApiErr> {
    let name = name.trim();
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ' '));
    let env = env_from_name(name);
    if !ok || env.is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_name",
            "Letters, numbers and _ (1–64 characters).",
        ));
    }
    Ok((name.to_owned(), env))
}

pub fn clean_hosts(hosts: &[String]) -> Result<Vec<String>, ApiErr> {
    let mut out: Vec<String> = Vec::new();
    for h in hosts {
        let h = h.trim().to_ascii_lowercase();
        let h = h
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or("")
            .split(':')
            .next()
            .unwrap_or("")
            .to_owned();
        if h.is_empty() {
            continue;
        }
        let body = h.strip_prefix("*.").unwrap_or(&h);
        if body.is_empty()
            || !body
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "bad_host",
                format!("{h}: a host looks like api.example.com or *.example.com"),
            ));
        }
        if !out.contains(&h) {
            out.push(h);
        }
    }
    Ok(out)
}

pub fn placeholder(env: &str, field: &str) -> String {
    format!("{PLACEHOLDER_SCHEME}{env}/{field}")
}

/// Field names per kind; `login` carries the sign-in URL as metadata, not a secret.
fn fields_for_kind(kind: &str, b: &Value) -> Result<serde_json::Map<String, Value>, ApiErr> {
    let mut out = serde_json::Map::new();
    let get = |k: &str| b.get(k).and_then(Value::as_str).map(str::to_owned);
    match kind {
        "key" | "text" => {
            let v = get("value")
                .or_else(|| get("text"))
                .filter(|v| !v.is_empty())
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_request", "value required"))?;
            out.insert("value".into(), Value::String(v));
        }
        "login" => {
            let f = b.get("fields").unwrap_or(b);
            let fget = |k: &str| f.get(k).and_then(Value::as_str).map(str::to_owned);
            let password = fget("password").filter(|v| !v.is_empty()).ok_or_else(|| {
                err(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "login needs a password",
                )
            })?;
            out.insert("password".into(), Value::String(password));
            if let Some(u) = fget("username").filter(|v| !v.is_empty()) {
                out.insert("username".into(), Value::String(u));
            }
            if let Some(u) = fget("url").filter(|v| !v.is_empty()) {
                out.insert("url".into(), Value::String(u));
            }
        }
        _ => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "bad_request",
                format!("kind must be one of {}", KINDS.join(", ")),
            ))
        }
    }
    Ok(out)
}

// ---- key material -----------------------------------------------------------------------------

/// The user's DEK, created and wrapped on first use.
pub(crate) fn user_dek(state: &AppState, user_id: &str) -> anyhow::Result<Dek> {
    if let Some(w) = state.db.user_dek_wrapped(user_id)? {
        return state.relay_key.unwrap_dek(&w);
    }
    let dek = Dek::random();
    state
        .db
        .insert_user_dek(user_id, &state.relay_key.wrap_dek(&dek)?)?;
    // Another request may have won the insert: always read back the stored one.
    let w = state
        .db
        .user_dek_wrapped(user_id)?
        .ok_or_else(|| anyhow::anyhow!("dek vanished"))?;
    state.relay_key.unwrap_dek(&w)
}

fn aad(secret_id: &str, version: i64) -> Vec<u8> {
    format!("{secret_id}|{version}").into_bytes()
}

fn encrypt_fields(
    dek: &Dek,
    secret_id: &str,
    version: i64,
    fields: &Value,
) -> anyhow::Result<String> {
    dek.encrypt(&aad(secret_id, version), &fields.to_string())
}

/// Decrypted fields (`field → value`) of one version. Stays inside this process.
fn decrypt_fields(dek: &Dek, v: &SecretVersion) -> anyhow::Result<BTreeMap<String, String>> {
    let plain = dek.decrypt(&aad(&v.secret_id, v.version), &v.fields_enc)?;
    let val: Value = serde_json::from_str(&plain)?;
    Ok(val
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect()
        })
        .unwrap_or_default())
}

/// Current fields plus every value still live in a previous version (for redaction).
pub(crate) struct Plain {
    pub current: BTreeMap<String, String>,
    pub all_values: Vec<String>,
}

pub(crate) fn plain_of(state: &AppState, dek: &Dek, s: &Secret) -> anyhow::Result<Plain> {
    let versions = state.db.secret_versions(&s.id)?;
    let mut current = BTreeMap::new();
    let mut all_values = Vec::new();
    for v in &versions {
        let f = decrypt_fields(dek, v)?;
        for (k, val) in &f {
            if (k == "value" || k == "password") && !all_values.contains(val) {
                all_values.push(val.clone());
            }
        }
        if v.version == s.current_version {
            current = f;
        }
    }
    Ok(Plain {
        current,
        all_values,
    })
}

/// Field names of the current version, for listings. Logins expose `username` as metadata.
fn field_names(kind: &str) -> Vec<&'static str> {
    match kind {
        "login" => vec!["username", "password"],
        _ => vec!["value"],
    }
}

// ---- audit and pushes -------------------------------------------------------------------------

pub(crate) fn audit(state: &AppState, user_id: &str, e: AuditEntry) {
    if let Err(e) = state.db.append_secret_audit(user_id, &e) {
        tracing::warn!(error = %e, "secret audit append failed");
    }
}

fn entry(action: &str, s: &Secret, by: &str) -> AuditEntry {
    AuditEntry {
        action: action.into(),
        target_kind: "secret".into(),
        target_id: s.id.clone(),
        target_name: s.env.clone(),
        by: by.into(),
        detail: json!({}),
        ..Default::default()
    }
}

fn push_box(state: &AppState, project_id: &str, kind: &str, payload: Value) {
    let _ = state.db.enqueue_push(project_id, kind, &payload);
}

fn push_user(state: &AppState, user_id: &str, event: PushEventDto) {
    let _ = state.db.push_user_event(user_id, &event, None);
}

/// Push when a project secret is requested (no `vault_id`; the secrets.rs form).
pub(crate) fn secret_request_push(
    request_id: impl Into<String>,
    project_id: impl Into<String>,
    project_name: impl Into<String>,
    name: impl Into<String>,
    host: Option<String>,
    use_kind: impl Into<String>,
) -> PushEventDto {
    PushEventDto::SecretRequest(SecretRequestPushPayloadDto {
        request_id: request_id.into(),
        project_id: project_id.into(),
        project_name: project_name.into(),
        name: name.into(),
        host,
        use_kind: use_kind.into(),
        vault_id: None,
    })
}

// ---- JSON shapes (Dto builders; wire JSON unchanged) ------------------------------------------

fn to_json<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).expect("secrets dto")
}

pub(crate) fn access_dto(state: &AppState, a: &Access) -> SecretAccessDto {
    let name = state
        .db
        .project(&a.project_id)
        .ok()
        .flatten()
        .map(|p| p.name)
        .unwrap_or_else(|| a.project_id.clone());
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
        item_hosts: None,
    }
}

#[allow(dead_code)] // Value form; HTTP handlers return DTOs, fixtures still use this.
fn access_json(state: &AppState, a: &Access) -> Value {
    to_json(access_dto(state, a))
}

/// Handle (no `@`) and display name of a user, for People rows and "asked by" lines.
pub(crate) fn user_handle_name(state: &AppState, user_id: &str) -> (String, Option<String>) {
    let card = state.db.user_card(user_id).ok().flatten();
    let handle = card
        .as_ref()
        .map(|c| c.handle.trim_start_matches('@').to_owned())
        .unwrap_or_default();
    let name = card
        .as_ref()
        .and_then(|c| c.name.clone())
        .filter(|n| !n.is_empty());
    (handle, name)
}

/// One People row for Settings / the box: id, handle, name, switch, scope, and the usage note
/// the agent sees.
pub(crate) fn director_dto(state: &AppState, d: &SecretDirector) -> SecretDirectorDto {
    let card = state.db.user_card(&d.user_id).ok().flatten();
    let (handle, name) = user_handle_name(state, &d.user_id);
    let avatar_url = card.as_ref().and_then(|c| c.avatar_url.clone());
    SecretDirectorDto {
        user_id: d.user_id.clone(),
        handle,
        name,
        avatar_url,
        note: d.note.clone(),
        created_at: d.created_at,
        enabled: Some(d.enabled),
        disabled_at: d.disabled_at,
        scope: Some(SecretAccessScopeDto {
            kind: d.scope_kind.clone(),
            chat_id: d.scope_chat_id.clone(),
        }),
        last_asked_at: d.last_asked_at,
        created_from: Some(d.created_from.clone()),
    }
}

pub(crate) fn director_json(state: &AppState, d: &SecretDirector) -> Value {
    to_json(director_dto(state, d))
}

pub(crate) fn secret_dto(state: &AppState, s: &Secret) -> SecretDto {
    let agents: Vec<SecretAccessDto> = state
        .db
        .accesses_of_target("secret", &s.id)
        .unwrap_or_default()
        .iter()
        .map(|a| access_dto(state, a))
        .collect();
    let directors: Vec<SecretDirectorDto> = state
        .db
        .secret_directors(&s.id)
        .unwrap_or_default()
        .iter()
        .map(|d| director_dto(state, d))
        .collect();
    let username = if s.kind == "login" {
        user_dek(state, &s.user_id)
            .ok()
            .and_then(|dek| plain_of(state, &dek, s).ok())
            .and_then(|p| p.current.get("username").cloned())
    } else {
        None
    };
    SecretDto {
        id: s.id.clone(),
        name: s.name.clone(),
        env: s.env.clone(),
        kind: s.kind.clone(),
        note: s.note.clone(),
        hosts: s.hosts.clone(),
        ask_every_time: s.ask_every_time,
        inject_env: s.inject_env,
        expires_at: s.expires_at,
        version: s.current_version,
        created_at: s.created_at,
        updated_at: s.updated_at,
        last_used_at: s.last_used_at,
        username,
        vault_name: ARBOS_VAULT_NAME.to_owned(),
        secret_ref: placeholder(
            &s.env,
            if s.kind == "login" {
                "password"
            } else {
                "value"
            },
        ),
        agents,
        people: directors.clone(),
        directors,
        others_ask: Some(s.others_ask.clone()),
        source: Some(secret_source_dto(state, s)),
    }
}

/// `source` block on a secret: `settings`, or `chat` + the agent it was handed to.
fn secret_source_dto(state: &AppState, s: &Secret) -> SecretSourceDto {
    let project_name = s
        .source_project_id
        .as_deref()
        .and_then(|p| state.db.project(p).ok().flatten())
        .map(|p| p.name);
    SecretSourceDto {
        kind: s.source_kind.clone(),
        project_id: s.source_project_id.clone(),
        project_name,
        chat_id: s.source_chat_id.clone(),
    }
}

#[allow(dead_code)] // Value form; HTTP handlers return DTOs, fixtures still use this.
pub(crate) fn secret_json(state: &AppState, s: &Secret) -> Value {
    to_json(secret_dto(state, s))
}

fn secret_envelope(state: &AppState, s: &Secret) -> SecretEnvelopeDto {
    SecretEnvelopeDto {
        secret: secret_dto(state, s),
    }
}

pub(crate) fn request_dto(state: &AppState, r: &SecretRequest) -> SecretRequestDto {
    let asked = r.asked_by.as_deref().map(|u| user_handle_name(state, u));
    let project_name = state
        .db
        .project(&r.project_id)
        .ok()
        .flatten()
        .map(|p| p.name);
    SecretRequestDto {
        id: r.id.clone(),
        project_id: r.project_id.clone(),
        project_name,
        chat_id: r.chat_id.clone(),
        target: SecretRequestTargetDto {
            kind: r.target_kind.clone(),
            id: r.target_id.clone(),
            target_ref: r.target_ref.clone(),
        },
        name: r.target_ref.clone(),
        purpose: r.purpose.clone(),
        use_kind: r.use_kind.clone(),
        host: r.host.clone(),
        status: r.status.clone(),
        decided_by: r.decided_by.clone(),
        created_at: r.created_at,
        decided_at: r.decided_at,
        expires_at: r.expires_at,
        asked_by: r.asked_by.clone(),
        asked_by_handle: asked.as_ref().map(|(h, _)| h.clone()),
        asked_by_name: asked.and_then(|(_, n)| n),
    }
}

pub(crate) fn request_json(state: &AppState, r: &SecretRequest) -> Value {
    to_json(request_dto(state, r))
}

pub(crate) fn activity_dto(a: &SecretAudit, project_name: Option<String>) -> SecretActivityDto {
    SecretActivityDto {
        id: a.id,
        user_id: a.user_id.clone(),
        at: a.at,
        action: a.action.clone(),
        target_kind: a.target_kind.clone(),
        target_id: a.target_id.clone(),
        target_name: a.target_name.clone(),
        name: a.target_name.clone(),
        project_id: a.project_id.clone(),
        project_name,
        chat_id: a.chat_id.clone(),
        request_id: a.request_id.clone(),
        access_id: a.access_id.clone(),
        use_kind: a.use_kind.clone(),
        host: a.host.clone(),
        by: a.by.clone(),
        detail: a.detail.clone(),
    }
}

pub(crate) fn activity(state: &AppState, user_id: &str, q: AuditQuery) -> Vec<SecretActivityDto> {
    state
        .db
        .list_audit(user_id, &q)
        .unwrap_or_default()
        .iter()
        .map(|a| {
            let project_name = a
                .project_id
                .as_deref()
                .and_then(|p| state.db.project(p).ok().flatten())
                .map(|p| p.name);
            activity_dto(a, project_name)
        })
        .collect()
}

fn project_secret_summary(s: &Secret) -> ProjectSecretSummaryDto {
    ProjectSecretSummaryDto {
        id: s.id.clone(),
        name: s.name.clone(),
        env: s.env.clone(),
        kind: s.kind.clone(),
        note: s.note.clone(),
        hosts: s.hosts.clone(),
        last_used_at: s.last_used_at,
        vault_name: ARBOS_VAULT_NAME.to_owned(),
        inject_env: s.inject_env,
    }
}

fn project_secret_item(state: &AppState, a: &Access, s: &Secret) -> ProjectSecretItemDto {
    let access = access_dto(state, a);
    ProjectSecretItemDto {
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
        item_hosts: None,
        target_kind: None,
        secret: Some(project_secret_summary(s)),
        vault: None,
    }
}

pub(crate) fn paused_dto(settings: &SecretsSettings) -> SecretsPausedDto {
    SecretsPausedDto {
        paused: settings.paused,
        paused_at: settings.paused_at,
        paused_by: settings.paused_by.clone(),
    }
}

// ---- the one rule: Access on, host allowed, not paused ---------------------------------------

/// Why a use was refused (strings from spec §8.8, never a value).
#[derive(Debug)]
pub enum Refusal {
    Paused,
    NoSuch,
    NoAccess,
    TurnedOff,
    WrongChat,
    HostNotAllowed(Vec<String>),
    Expired,
    AskEveryTime,
    /// A non-owner asked the agent and is not on this secret's People list (row on, scope fits).
    NotDirector,
    /// Same, but the secret's `others_ask` is `card`: the agent must call `secrets_request` so
    /// the owner gets a card naming who asked.
    AskOwnerFirst,
}

impl Refusal {
    pub fn message(&self, name: &str, host: &str) -> String {
        match self {
            Refusal::Paused => "Secrets are paused by the owner.".to_owned(),
            Refusal::NoSuch => format!("{name}: no such secret — see secrets_list."),
            Refusal::NoAccess => {
                format!("{name}: no access for this agent — call secrets_request.")
            }
            Refusal::TurnedOff => format!("{name}: access was turned off by the owner."),
            Refusal::WrongChat => {
                format!("{name}: access is for another chat — call secrets_request.")
            }
            Refusal::HostNotAllowed(allowed) => {
                format!(
                    "{name}: not allowed at {host} (allowed: {}).",
                    allowed.join(", ")
                )
            }
            Refusal::Expired => format!("{name}: expired."),
            Refusal::AskEveryTime => {
                format!("{name}: asks every time — call secrets_request first.")
            }
            Refusal::NotDirector => format!(
                "{name}: the person asking you is not on this secret's People list. Only the owner, or people the owner added under People on the secret's page, may ask for it. Tell them plainly, and do not retry with this secret for them."
            ),
            Refusal::AskOwnerFirst => format!(
                "{name}: the person asking you is not on this secret's People list — call secrets_request first so the owner gets a card, then wait for the answer."
            ),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Refusal::Paused => "paused",
            Refusal::NoSuch => "not_found",
            Refusal::NoAccess | Refusal::TurnedOff | Refusal::WrongChat | Refusal::AskEveryTime => {
                "no_access"
            }
            Refusal::NotDirector | Refusal::AskOwnerFirst => "not_allowed_person",
            Refusal::HostNotAllowed(_) => "host_not_allowed",
            Refusal::Expired => "expired",
        }
    }
}

/// May `asked_by` ask for `s` in `chat_id`? The owner always may. Anyone else needs a People
/// row that is on and whose scope covers the chat. `None` means no other person is asking
/// (owner 1:1 chat, or asker unknown) — allowed.
pub(crate) fn may_direct(
    state: &AppState,
    s: &Secret,
    asked_by: Option<&str>,
    chat_id: Option<&str>,
) -> bool {
    match asked_by.map(str::trim).filter(|u| !u.is_empty()) {
        None => true,
        Some(uid) if uid == s.user_id => true,
        Some(uid) => state
            .db
            .is_secret_director(&s.id, uid, chat_id)
            .unwrap_or(false),
    }
}

/// The refusal for a person who is not on the list: refuse flat, or send the agent to ask.
pub(crate) fn not_listed(s: &Secret) -> Refusal {
    if s.others_ask == "card" {
        Refusal::AskOwnerFirst
    } else {
        Refusal::NotDirector
    }
}

/// A use the rule allowed, and what it was allowed by.
pub struct Allowed {
    pub access: Option<Access>,
    /// The `allowed_once` request spent by this use.
    pub once: Option<SecretRequest>,
    /// No host was known yet: pin `host` on the access and tell the owner.
    pub pin_host: bool,
}

/// Check one use of `s` by `project_id` at `host` in `chat_id`. `use_kind` is `header | env |
/// browser_fill`. `asked_by` is the human directing this turn (`None` = owner / unknown).
/// Pure except for the lookups (access, allowed_once, directors).
pub(crate) fn decide_use(
    state: &AppState,
    s: &Secret,
    project_id: &str,
    host: &str,
    use_kind: &str,
    chat_id: Option<&str>,
    asked_by: Option<&str>,
) -> Result<Allowed, Refusal> {
    if s.expires_at.is_some_and(|t| t < now_ms()) {
        return Err(Refusal::Expired);
    }
    let access = state.db.access(project_id, "secret", &s.id).ok().flatten();
    let asker = asked_by.map(str::trim).filter(|u| !u.is_empty());
    let once = state
        .db
        .allowed_once_for(project_id, &s.id, Some(host), use_kind)
        .ok()
        .flatten()
        // A once-grant given for one person's ask is spent only by that person's ask.
        .filter(|r| r.asked_by.is_none() || r.asked_by.as_deref() == asker);
    // Owner already approved this specific request card — that decision stands.
    if let Some(once) = once {
        return Ok(Allowed {
            access,
            once: Some(once),
            pin_host: false,
        });
    }
    if !may_direct(state, s, asked_by, chat_id) {
        return Err(not_listed(s));
    }
    let Some(a) = access else {
        return Err(Refusal::NoAccess);
    };
    if !a.enabled {
        return Err(Refusal::TurnedOff);
    }
    if a.scope_kind == "chat" && a.scope_chat_id.as_deref() != chat_id {
        return Err(Refusal::WrongChat);
    }
    if s.ask_every_time {
        return Err(Refusal::AskEveryTime);
    }
    let mut hosts = s.hosts.clone();
    hosts.extend(a.hosts.iter().cloned());
    if hosts.is_empty() {
        return Ok(Allowed {
            access: Some(a),
            once: None,
            pin_host: !host.is_empty(),
        });
    }
    // No host yet (a request without one, a login fill): the host is checked at use.
    if !host.is_empty() && !host_allowed(host, &hosts) {
        return Err(Refusal::HostNotAllowed(hosts));
    }
    Ok(Allowed {
        access: Some(a),
        once: None,
        pin_host: false,
    })
}

/// After an allowed use: spend a one-time grant, pin a first host, touch timestamps, audit.
/// `asked_by` is the person whose ask this use served (None = the owner); it lands on the
/// audit row and stamps their People row.
#[allow(clippy::too_many_arguments)]
pub(crate) fn after_use(
    state: &AppState,
    s: &Secret,
    project_id: &str,
    host: &str,
    use_kind: &str,
    chat_id: Option<&str>,
    asked_by: Option<&str>,
    allowed: &Allowed,
) {
    let asked_by = asked_by
        .map(str::trim)
        .filter(|u| !u.is_empty() && *u != s.user_id);
    if let Some(uid) = asked_by {
        let _ = state.db.touch_secret_director(&s.id, uid);
    }
    if let Some(once) = &allowed.once {
        let _ = state.db.consume_allowed_once(&once.id);
    }
    if allowed.pin_host {
        if let Some(a) = &allowed.access {
            let mut a = a.clone();
            a.hosts.push(host.to_owned());
            let _ = state.db.upsert_access(&a);
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
                json!({ "ref": placeholder(&s.env, "value"), "env": s.env, "host": host, "chat_id": chat_id,
                    "text": format!("{agent} used {} at {host}.", s.env) }),
            );
            audit(
                state,
                &s.user_id,
                AuditEntry {
                    project_id: Some(project_id.to_owned()),
                    chat_id: chat_id.map(str::to_owned),
                    access_id: Some(a.id.clone()),
                    host: Some(host.to_owned()),
                    detail: json!({ "pinned": true }),
                    ..entry("access_on", s, &format!("agent:{project_id}"))
                },
            );
            spawn_remount(state.clone(), s.user_id.clone());
        }
    }
    let _ = state.db.touch_secret(&s.user_id, &s.id, project_id);
    audit(
        state,
        &s.user_id,
        AuditEntry {
            project_id: Some(project_id.to_owned()),
            chat_id: chat_id.map(str::to_owned),
            request_id: allowed.once.as_ref().map(|r| r.id.clone()),
            access_id: allowed.access.as_ref().map(|a| a.id.clone()),
            use_kind: Some(use_kind.to_owned()),
            host: Some(host.to_owned()),
            detail: asked_by_detail(state, asked_by),
            ..entry("used", s, &format!("agent:{project_id}"))
        },
    );
}

/// `{ asked_by, asked_by_handle }` for an audit row, or `{}` when the owner asked.
pub(crate) fn asked_by_detail(state: &AppState, asked_by: Option<&str>) -> Value {
    match asked_by {
        Some(uid) => {
            let (handle, _) = user_handle_name(state, uid);
            json!({ "asked_by": uid, "asked_by_handle": handle })
        }
        None => json!({}),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn audit_refusal(
    state: &AppState,
    user_id: &str,
    name: &str,
    secret: Option<&Secret>,
    project_id: &str,
    host: &str,
    use_kind: &str,
    asked_by: Option<&str>,
    why: &Refusal,
) {
    let mut detail = asked_by_detail(state, asked_by.filter(|u| *u != user_id));
    detail["reason"] = json!(why.kind());
    audit(
        state,
        user_id,
        AuditEntry {
            action: "refused".into(),
            target_kind: "secret".into(),
            target_id: secret.map(|s| s.id.clone()).unwrap_or_default(),
            target_name: secret
                .map(|s| s.env.clone())
                .unwrap_or_else(|| name.to_owned()),
            project_id: Some(project_id.to_owned()),
            use_kind: Some(use_kind.to_owned()),
            host: Some(host.to_owned()),
            by: format!("agent:{project_id}"),
            detail,
            ..Default::default()
        },
    );
}

// ---- Daytona mirror (D28) -----------------------------------------------------------------------

/// Recompute the user's sandbox env: every secret with at least one enabled access and a known
/// host set, as Daytona Secrets (`ENV` for one field, `ENV_FIELD` for logins); `{}` while
/// paused. Best effort; the relay `/egress` path is the authority regardless.
pub(crate) async fn remount_daytona(state: &AppState, user_id: &str) -> Option<String> {
    let dt = state.daytona.as_ref()?;
    let sb = state.db.user_box(user_id).ok().flatten()?;
    if sb.provider != "daytona" {
        return None;
    }
    let sandbox_id = sb.sandbox_id?;
    let paused = state.db.secrets_settings(user_id).ok()?.paused;
    let mut mounts = Vec::new();
    if !paused {
        let dek = user_dek(state, user_id).ok()?;
        for mut s in state.db.list_secrets(user_id).ok()? {
            if s.expires_at.is_some_and(|t| t < now_ms()) {
                continue;
            }
            let accesses = state.db.accesses_of_target("secret", &s.id).ok()?;
            let mut hosts = s.hosts.clone();
            let mut any_on = false;
            for a in accesses.iter().filter(|a| a.enabled) {
                any_on = true;
                hosts.extend(a.hosts.iter().cloned());
            }
            hosts.dedup();
            if !any_on || hosts.is_empty() {
                continue;
            }
            let Ok(plain) = plain_of(state, &dek, &s) else {
                continue;
            };
            let mut ids = s.daytona_secrets.as_object().cloned().unwrap_or_default();
            let mut changed = false;
            for (field, value) in &plain.current {
                if field == "url" || field == "username" {
                    continue;
                }
                let secret_name = format!("arbos-{}-{}", s.id.to_lowercase(), field);
                let outcome = match ids.get(field).and_then(Value::as_str) {
                    Some(existing) => dt
                        .update_secret(existing, value, &hosts)
                        .await
                        .map(|_| existing.to_owned()),
                    None => dt.create_secret(&secret_name, value, &hosts).await,
                };
                match outcome {
                    Ok(sid) => {
                        if ids.get(field).and_then(Value::as_str) != Some(sid.as_str()) {
                            ids.insert(field.clone(), Value::String(sid));
                            changed = true;
                        }
                        let env = if field == "value" {
                            s.env.clone()
                        } else {
                            format!("{}_{}", s.env, field.to_ascii_uppercase())
                        };
                        let mut m = serde_json::Map::new();
                        m.insert(env, Value::String(secret_name));
                        mounts.push(m);
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, secret = %s.env, "daytona secret mirror failed");
                    }
                }
            }
            if changed {
                s.daytona_secrets = Value::Object(ids);
                let _ = state.db.update_secret(&s);
            }
        }
    }
    match dt.update_secrets(&sandbox_id, mounts).await {
        Ok(_) => Some("mounted".into()),
        Err(e) => {
            tracing::warn!(error = %e, "daytona updateSecrets failed");
            Some(format!("updateSecrets failed: {e}"))
        }
    }
}

pub(crate) fn spawn_remount(state: AppState, user_id: String) {
    tokio::spawn(async move {
        remount_daytona(&state, &user_id).await;
    });
}

/// Tell the box about one access change and refresh the Daytona mount.
fn after_access_change(state: &AppState, s: &Secret, a: &Access) {
    push_box(
        state,
        &a.project_id,
        "secret_access_changed",
        json!({ "access_id": a.id, "ref": placeholder(&s.env, if s.kind == "login" { "password" } else { "value" }),
            "env": s.env, "name": s.name, "kind": s.kind, "enabled": a.enabled, "inject_env": s.inject_env,
            "hosts": s.hosts.iter().chain(a.hosts.iter()).collect::<Vec<_>>() }),
    );
    spawn_remount(state.clone(), s.user_id.clone());
}

/// A People row or the Others-who-ask switch changed: every agent with Access on the secret
/// re-reads its grants (the box keeps the list for its own pre-check, ≤ 5 s like an Access flip).
fn after_people_change(state: &AppState, s: &Secret) {
    for a in state
        .db
        .accesses_of_target("secret", &s.id)
        .unwrap_or_default()
    {
        push_box(
            state,
            &a.project_id,
            "secret_access_changed",
            json!({ "access_id": a.id, "ref": placeholder(&s.env, if s.kind == "login" { "password" } else { "value" }),
                "env": s.env, "name": s.name, "kind": s.kind, "enabled": a.enabled, "inject_env": s.inject_env,
                "people": true }),
        );
    }
}

// ---- device: list, create, read, update, value, delete -----------------------------------------

/// `GET /secrets` → `{ paused, vaults, secrets, requests_pending, requests, activity, providers, features }`.
/// `requests` are the pending cards, so the Secrets root can show "Waiting for you" from one call.
pub async fn list(State(state): State<AppState>, principal: Principal) -> ApiResult {
    let user_id = device_user(&principal)?;
    let settings = state.db.secrets_settings(&user_id).map_err(internal)?;
    let secrets: Vec<SecretDto> = state
        .db
        .list_secrets(&user_id)
        .map_err(internal)?
        .iter()
        .map(|s| secret_dto(&state, s))
        .collect();
    let pending_items: Vec<SecretRequestDto> = state
        .db
        .list_secret_requests(&user_id, None, 50)
        .unwrap_or_default()
        .iter()
        .filter(|r| r.status == "pending")
        .map(|r| request_dto(&state, r))
        .collect();
    let pending = pending_items.len() as i64;
    let recent = activity(
        &state,
        &user_id,
        AuditQuery {
            since: Some(now_ms() - ACTIVITY_WINDOW_MS),
            limit: 20,
            ..Default::default()
        },
    );
    Ok(Json(SecretsOverviewDto {
        paused: settings.paused,
        paused_at: settings.paused_at,
        paused_by: settings.paused_by,
        vaults: crate::vaults::list_dto(&state, &user_id),
        providers: crate::vaults::providers(&state)
            .into_iter()
            .map(str::to_owned)
            .collect(),
        features: crate::features::dto(),
        secrets,
        requests_pending: pending,
        requests: pending_items,
        activity: recent,
    })
    .into_response())
}

#[derive(Deserialize, Default)]
pub struct AgentPick {
    pub project_id: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct CreateBody {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub fields: Option<Value>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub hosts: Option<Vec<String>>,
    #[serde(default)]
    pub ask_every_time: Option<bool>,
    #[serde(default)]
    pub inject_env: Option<bool>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub agents: Vec<AgentPick>,
    /// Where the value was given: absent = the Settings form; `{ kind: "chat", project_id,
    /// chat_id }` = the request card in a chat (the client posts the value here, never into
    /// the chat; the agent gets the name back through the card).
    #[serde(default)]
    pub source: Option<SourceBody>,
}

#[derive(Deserialize, Default, Clone)]
pub struct SourceBody {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub chat_id: Option<String>,
}

/// The kind a form implies when the body does not say: login fields → login, `text` → text.
fn infer_kind(b: &CreateBody) -> String {
    if let Some(k) = b.kind.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        return match k {
            "api_key" | "secret" => "key".to_owned(),
            other => other.to_owned(),
        };
    }
    if b.fields
        .as_ref()
        .is_some_and(|f| f.get("password").is_some())
    {
        "login".to_owned()
    } else if b.text.is_some() || b.value.as_deref().is_some_and(|v| v.contains('\n')) {
        "text".to_owned()
    } else {
        "key".to_owned()
    }
}

/// `POST /secrets` → 201 `{ secret }`. The value is encrypted at once and never returned.
pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<CreateBody>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let (name, env) = validate_name(&b.name)?;
    let kind = infer_kind(&b);
    if !KINDS.contains(&kind.as_str()) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            format!("kind must be one of {}", KINDS.join(", ")),
        ));
    }
    let body_value = json!({ "value": b.value, "text": b.text, "fields": b.fields });
    let fields = fields_for_kind(&kind, &body_value)?;
    if state
        .db
        .secret_by_name(&user_id, &name)
        .map_err(internal)?
        .is_some()
        || state
            .db
            .secret_by_name(&user_id, &env)
            .map_err(internal)?
            .is_some()
    {
        return Err(err(
            StatusCode::CONFLICT,
            "already_exists",
            format!("You already have a secret called {env}."),
        ));
    }
    let hosts = clean_hosts(b.hosts.as_deref().unwrap_or(&[]))?;
    let dek = user_dek(&state, &user_id).map_err(internal)?;
    let now = now_ms();
    let id = ulid();
    let s = Secret {
        id: id.clone(),
        user_id: user_id.clone(),
        name,
        env,
        kind,
        note: b.note.unwrap_or_default().chars().take(500).collect(),
        hosts,
        ask_every_time: b.ask_every_time.unwrap_or(false) && !b.inject_env.unwrap_or(false),
        expires_at: b.expires_at,
        current_version: 1,
        created_at: now,
        updated_at: now,
        last_used_at: None,
        daytona_secrets: json!({}),
        // Real value in the environment is a choice the owner makes; it also turns off
        // Ask every time, which has nothing to ask once the value is just there.
        inject_env: b.inject_env.unwrap_or(false),
        others_ask: "refuse".into(),
        source_kind: "settings".into(),
        source_project_id: None,
        source_chat_id: None,
    };
    let (s, created_from) = match b.source.as_ref().filter(|src| src.kind == "chat") {
        Some(src) => {
            // Only the user's own agent can be the one it was handed to.
            let pid = src
                .project_id
                .as_deref()
                .and_then(|p| state.db.project(p).ok().flatten())
                .filter(|p| p.user_id == user_id)
                .map(|p| p.id);
            (
                Secret {
                    source_kind: "chat".into(),
                    source_project_id: pid,
                    source_chat_id: src.chat_id.clone(),
                    ..s
                },
                "chat",
            )
        }
        None => (s, "settings"),
    };
    let v1 = SecretVersion {
        secret_id: id.clone(),
        version: 1,
        fields_enc: encrypt_fields(&dek, &id, 1, &Value::Object(fields)).map_err(internal)?,
        created_at: now,
        retire_at: None,
    };
    state.db.insert_secret(&s, &v1).map_err(internal)?;
    audit(
        &state,
        &user_id,
        AuditEntry {
            detail: json!({ "kind": s.kind, "hosts": s.hosts, "source": s.source_kind, "project_id": s.source_project_id, "chat_id": s.source_chat_id }),
            ..entry("created", &s, &format!("user:{user_id}"))
        },
    );
    for pick in &b.agents {
        if let Some(a) = grant_access(
            &state,
            &user_id,
            &s,
            &pick.project_id,
            pick.note.as_deref(),
            None,
            true,
            created_from,
        )? {
            after_access_change(&state, &s, &a);
        }
    }
    Ok((StatusCode::CREATED, Json(secret_envelope(&state, &s))).into_response())
}

/// Create or update the access row for `(secret, project)`; audits on/off. `Ok(None)` when the
/// project is not the user's.
#[allow(clippy::too_many_arguments)]
fn grant_access(
    state: &AppState,
    user_id: &str,
    s: &Secret,
    project_id: &str,
    note: Option<&str>,
    hosts: Option<Vec<String>>,
    enabled: bool,
    created_from: &str,
) -> Result<Option<Access>, ApiErr> {
    let Some(p) = state.db.project(project_id).map_err(internal)? else {
        return Err(err(StatusCode::NOT_FOUND, "not_found", "no such agent"));
    };
    if p.user_id != user_id {
        return Err(err(StatusCode::FORBIDDEN, "not_owner", "not your agent"));
    }
    let existing = state.db.access(&p.id, "secret", &s.id).map_err(internal)?;
    let was_on = existing.as_ref().is_some_and(|a| a.enabled);
    let now = now_ms();
    let a = Access {
        id: existing.as_ref().map(|a| a.id.clone()).unwrap_or_else(ulid),
        user_id: user_id.to_owned(),
        project_id: p.id.clone(),
        target_kind: "secret".into(),
        target_id: s.id.clone(),
        note: note
            .map(|n| n.chars().take(500).collect())
            .or_else(|| existing.as_ref().map(|a| a.note.clone()))
            .unwrap_or_default(),
        hosts: hosts
            .or_else(|| existing.as_ref().map(|a| a.hosts.clone()))
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
                detail: json!({ "note": a.note, "hosts": a.hosts, "created_from": created_from }),
                ..entry(
                    if enabled { "access_on" } else { "access_off" },
                    s,
                    &format!("user:{user_id}"),
                )
            },
        );
    }
    Ok(Some(a))
}

/// `GET /secrets/{id}` → `{ secret, activity }`.
pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let s = state
        .db
        .secret(&user_id, &id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let act = activity(
        &state,
        &user_id,
        AuditQuery {
            target_id: Some(s.id.clone()),
            since: Some(now_ms() - ACTIVITY_WINDOW_MS),
            limit: 50,
            ..Default::default()
        },
    );
    Ok(Json(SecretDetailDto {
        secret: secret_dto(&state, &s),
        activity: act,
    })
    .into_response())
}

#[derive(Deserialize, Default)]
pub struct PatchBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub hosts: Option<Vec<String>>,
    #[serde(default)]
    pub ask_every_time: Option<bool>,
    /// "Real value in the agent's environment" (D83 decision 2 as decided). On and Ask every
    /// time are exclusive: turning either on turns the other off.
    #[serde(default)]
    pub inject_env: Option<bool>,
    /// `refuse | card`: what a non-listed person's ask does (spec §7.7).
    #[serde(default)]
    pub others_ask: Option<String>,
    /// `null` clears; absent keeps.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub expires_at: Option<Option<i64>>,
}

fn deserialize_double_option<'de, D>(d: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<i64>::deserialize(d)?))
}

/// `PATCH /secrets/{id}` → `{ secret }`.
pub async fn patch(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<PatchBody>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let mut s = state
        .db
        .secret(&user_id, &id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    if let Some(name) = b.name.as_deref() {
        let (name, env) = validate_name(name)?;
        for other in [&name, &env] {
            if let Some(o) = state.db.secret_by_name(&user_id, other).map_err(internal)? {
                if o.id != s.id {
                    return Err(err(
                        StatusCode::CONFLICT,
                        "already_exists",
                        format!("You already have a secret called {env}."),
                    ));
                }
            }
        }
        s.name = name;
        s.env = env;
    }
    if let Some(note) = b.note {
        s.note = note.chars().take(500).collect();
    }
    let hosts_changed = b.hosts.is_some();
    if let Some(h) = b.hosts.as_deref() {
        s.hosts = clean_hosts(h)?;
    }
    if let Some(a) = b.ask_every_time {
        s.ask_every_time = a;
        if a {
            s.inject_env = false;
        }
    }
    let inject_changed = b.inject_env.is_some_and(|i| i != s.inject_env);
    if let Some(i) = b.inject_env {
        s.inject_env = i;
        if i {
            s.ask_every_time = false;
        }
    }
    if let Some(e) = b.expires_at {
        s.expires_at = e;
    }
    let others_changed = b.others_ask.as_deref().is_some_and(|o| o != s.others_ask);
    if let Some(o) = b.others_ask.as_deref() {
        s.others_ask = match o {
            "card" | "ask" | "ask_me" => "card".to_owned(),
            "refuse" => "refuse".to_owned(),
            _ => {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "others_ask must be refuse or card",
                ))
            }
        };
    }
    state.db.update_secret(&s).map_err(internal)?;
    audit(
        &state,
        &user_id,
        AuditEntry {
            detail: json!({ "hosts": s.hosts, "ask_every_time": s.ask_every_time, "inject_env": s.inject_env, "expires_at": s.expires_at, "others_ask": s.others_ask }),
            ..entry(
                if inject_changed {
                    if s.inject_env {
                        "env_on"
                    } else {
                        "env_off"
                    }
                } else {
                    "updated"
                },
                &s,
                &format!("user:{user_id}"),
            )
        },
    );
    if hosts_changed || inject_changed || others_changed {
        for a in state
            .db
            .accesses_of_target("secret", &s.id)
            .unwrap_or_default()
        {
            after_access_change(&state, &s, &a);
        }
    }
    Ok(Json(secret_envelope(&state, &s)).into_response())
}

/// `PUT /secrets/{id}/value` `{ value | text | fields }` → `{ secret }`: version n+1; version n
/// keeps resolving for 24 h; Daytona mirrors refreshed; the box drops its cached placeholder env.
pub async fn replace_value(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<Value>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let s = state
        .db
        .secret(&user_id, &id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let fields = fields_for_kind(&s.kind, &b)?;
    let dek = user_dek(&state, &user_id).map_err(internal)?;
    let next = s.current_version + 1;
    let enc = encrypt_fields(&dek, &s.id, next, &Value::Object(fields)).map_err(internal)?;
    let got = state
        .db
        .add_secret_version(&user_id, &s.id, &enc)
        .map_err(internal)?;
    if got != next {
        return Err(err(
            StatusCode::CONFLICT,
            "conflict",
            "the value changed under you; reload and try again",
        ));
    }
    let s = state
        .db
        .secret(&user_id, &id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    audit(
        &state,
        &user_id,
        AuditEntry {
            detail: json!({ "version": s.current_version }),
            ..entry("value_replaced", &s, &format!("user:{user_id}"))
        },
    );
    for a in state
        .db
        .accesses_of_target("secret", &s.id)
        .unwrap_or_default()
    {
        push_box(
            &state,
            &a.project_id,
            "secret_value_replaced",
            json!({ "ref": placeholder(&s.env, "value"), "env": s.env }),
        );
    }
    spawn_remount(state.clone(), user_id.clone());
    Ok(Json(secret_envelope(&state, &s)).into_response())
}

/// `DELETE /secrets/{id}` → 204. Ciphertext, versions and accesses go; audit stays.
pub async fn delete(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let s = state
        .db
        .secret(&user_id, &id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let accesses = state
        .db
        .accesses_of_target("secret", &s.id)
        .unwrap_or_default();
    state.db.delete_secret(&user_id, &id).map_err(internal)?;
    audit(
        &state,
        &user_id,
        AuditEntry {
            detail: json!({ "agents": accesses.len() }),
            ..entry("deleted", &s, &format!("user:{user_id}"))
        },
    );
    for a in accesses {
        let mut off = a.clone();
        off.enabled = false;
        after_access_change(&state, &s, &off);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---- device: access ----------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct AccessBody {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub hosts: Option<Vec<String>>,
    /// `{ "kind": "all" | "chat", "chat_id"? }`.
    #[serde(default)]
    pub scope: Option<Value>,
}

fn default_true() -> bool {
    true
}

/// `PUT /secrets/{id}/access/{project_id}` → `{ access }` (create or update; idempotent).
pub async fn put_access(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, project_id)): Path<(String, String)>,
    body: Option<Json<AccessBody>>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let b = body.map(|b| b.0).unwrap_or_default();
    let s = state
        .db
        .secret(&user_id, &id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let hosts = match b.hosts.as_deref() {
        Some(h) => Some(clean_hosts(h)?),
        None => None,
    };
    let mut a = grant_access(
        &state,
        &user_id,
        &s,
        &project_id,
        b.note.as_deref(),
        hosts,
        b.enabled,
        "settings",
    )?
    .ok_or_else(not_found)?;
    if let Some(scope) = b.scope.as_ref() {
        let kind = scope.get("kind").and_then(Value::as_str).unwrap_or("all");
        a.scope_kind = if kind == "chat" { "chat" } else { "all" }.into();
        a.scope_chat_id = if kind == "chat" {
            scope
                .get("chat_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        } else {
            None
        };
        state.db.upsert_access(&a).map_err(internal)?;
    }
    after_access_change(&state, &s, &a);
    Ok(Json(SecretAccessEnvelopeDto {
        access: access_dto(&state, &a),
    })
    .into_response())
}

/// `DELETE /secrets/{id}/access/{project_id}` → 204: off, and gone from the lists.
pub async fn delete_access(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, project_id)): Path<(String, String)>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let s = state
        .db
        .secret(&user_id, &id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let Some(existing) = state
        .db
        .access(&project_id, "secret", &s.id)
        .map_err(internal)?
    else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    if let Some(mut a) = state
        .db
        .delete_access(&user_id, &existing.id)
        .map_err(internal)?
    {
        a.enabled = false;
        audit(
            &state,
            &user_id,
            AuditEntry {
                project_id: Some(a.project_id.clone()),
                access_id: Some(a.id.clone()),
                ..entry("access_off", &s, &format!("user:{user_id}"))
            },
        );
        after_access_change(&state, &s, &a);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize, Default)]
pub struct DirectorBody {
    /// Optional: resolve the person by GitHub handle when the path has no user id yet.
    #[serde(default)]
    pub handle: Option<String>,
    /// Constraints the agent must honour when this person asks
    /// (e.g. "only 1k usd of credit usage per day").
    #[serde(default)]
    pub note: Option<String>,
    /// The row's switch (absent = leave as is; a new row is on).
    #[serde(default)]
    pub enabled: Option<bool>,
    /// `{ "kind": "all" | "chat", "chat_id"? }` (absent = leave as is; a new row is `all`).
    #[serde(default)]
    pub scope: Option<Value>,
}

/// Create or update one People row from Settings (`PUT …/people/{user_id}` and the legacy
/// `PUT …/directors/{user_id}`): note, switch, scope. The owner cannot add themselves.
fn upsert_person(
    state: &AppState,
    user_id: &str,
    s: &Secret,
    person_id: &str,
    b: DirectorBody,
    handle_for_audit: Option<&str>,
) -> Result<(), ApiErr> {
    let now = now_ms();
    let existing = state
        .db
        .secret_director(&s.id, person_id)
        .map_err(internal)?;
    let mut d = existing
        .clone()
        .unwrap_or_else(|| SecretDirector::new(&s.id, person_id, "", now));
    if let Some(n) = b.note {
        d.note = n.chars().take(500).collect();
    }
    if let Some(sc) = b.scope.as_ref() {
        let kind = sc.get("kind").and_then(Value::as_str).unwrap_or("all");
        d.scope_kind = if kind == "chat" { "chat" } else { "all" }.into();
        d.scope_chat_id = if kind == "chat" {
            sc.get("chat_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .filter(|c| !c.is_empty())
        } else {
            None
        };
    }
    let was_on = existing.as_ref().is_none_or(|e| !e.enabled);
    if let Some(on) = b.enabled {
        d.enabled = on;
        d.disabled_at = if on { None } else { Some(now) };
    }
    state.db.upsert_secret_director(&d).map_err(internal)?;
    after_people_change(state, s);
    let flipped = existing.as_ref().map(|e| e.enabled) != Some(d.enabled);
    if existing.is_none() || flipped || was_on {
        audit(
            state,
            user_id,
            AuditEntry {
                detail: json!({ "person_user_id": person_id, "handle": handle_for_audit, "note": d.note, "scope": d.scope_kind }),
                ..entry(
                    if d.enabled { "person_on" } else { "person_off" },
                    s,
                    &format!("user:{user_id}"),
                )
            },
        );
    }
    Ok(())
}

/// `PUT /secrets/{id}/people/{user_id}` (alias `…/directors/{user_id}`) → `{ secret }`.
/// Body may carry `note`, `enabled`, `scope`. The owner cannot add themselves (they always may ask).
pub async fn put_director(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, director_user_id)): Path<(String, String)>,
    body: Option<Json<DirectorBody>>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let b = body.map(|b| b.0).unwrap_or_default();
    let s = state
        .db
        .secret(&user_id, &id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let person_id = director_user_id.trim().to_owned();
    if person_id.is_empty() || person_id == user_id {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "pick someone other than yourself; you already may ask for every secret you own",
        ));
    }
    if state.db.user(&person_id).map_err(internal)?.is_none() {
        return Err(err(
            StatusCode::NOT_FOUND,
            "not_found",
            "no such person on Arbos",
        ));
    }
    let (handle, _) = user_handle_name(&state, &person_id);
    upsert_person(&state, &user_id, &s, &person_id, b, Some(&handle))?;
    Ok(Json(secret_envelope(&state, &s)).into_response())
}

/// `POST /secrets/{id}/people` (alias `…/directors`) → `{ secret }` with body `{ handle, note? }`
/// for the people picker.
pub async fn post_director(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<DirectorBody>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let s = state
        .db
        .secret(&user_id, &id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let handle = b
        .handle
        .as_deref()
        .map(|h| h.trim().trim_start_matches('@'))
        .filter(|h| !h.is_empty())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_request", "handle is required"))?
        .to_owned();
    let person = state
        .db
        .user_by_handle(&handle)
        .map_err(internal)?
        .ok_or_else(|| {
            err(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("@{handle} is not on Arbos yet"),
            )
        })?;
    let person_id = person.id.clone();
    if person_id.is_empty() || person_id == user_id {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "pick someone other than yourself; you already may ask for every secret you own",
        ));
    }
    // Adding from the picker turns the row on and sets the note (empty note overwrites).
    let body = DirectorBody {
        handle: None,
        note: Some(b.note.unwrap_or_default()),
        enabled: Some(true),
        scope: b.scope,
    };
    upsert_person(&state, &user_id, &s, &person_id, body, Some(&handle))?;
    Ok(Json(secret_envelope(&state, &s)).into_response())
}

/// `DELETE /secrets/{id}/people/{user_id}` (alias `…/directors/{user_id}`) → `{ secret }`.
pub async fn delete_director(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, director_user_id)): Path<(String, String)>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let s = state
        .db
        .secret(&user_id, &id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let person_id = director_user_id.trim().to_owned();
    let (handle, _) = user_handle_name(&state, &person_id);
    let removed = state
        .db
        .delete_secret_director(&s.id, &person_id)
        .map_err(internal)?;
    if removed {
        after_people_change(&state, &s);
        audit(
            &state,
            &user_id,
            AuditEntry {
                detail: json!({ "person_user_id": person_id, "handle": handle, "removed": true }),
                ..entry("person_off", &s, &format!("user:{user_id}"))
            },
        );
    }
    Ok(Json(secret_envelope(&state, &s)).into_response())
}

/// `POST /secrets/{id}/revoke-all` → `{ secret }` (every access off).
pub async fn revoke_all(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let s = state
        .db
        .secret(&user_id, &id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let were_on = state
        .db
        .disable_accesses_of_target(&user_id, "secret", &s.id)
        .map_err(internal)?;
    for mut a in were_on {
        a.enabled = false;
        audit(
            &state,
            &user_id,
            AuditEntry {
                project_id: Some(a.project_id.clone()),
                access_id: Some(a.id.clone()),
                detail: json!({ "revoke_all": true }),
                ..entry("access_off", &s, &format!("user:{user_id}"))
            },
        );
        after_access_change(&state, &s, &a);
    }
    Ok(Json(secret_envelope(&state, &s)).into_response())
}

/// How long a box may fetch scrub values after the owner taps Scrub from context.
const SCRUB_WINDOW_MS: i64 = 10 * 60 * 1000;

/// `POST /secrets/{id}/scrub` → `{ secret }`: find this value in every agent's context
/// (chats, sessions, logs) and replace each copy with a redaction. Values never leave in the
/// push; each box fetches them once through `GET /box/secrets/{id}/scrub` within the window.
pub async fn scrub(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let s = state
        .db
        .secret(&user_id, &id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let dek = user_dek(&state, &user_id).map_err(internal)?;
    let plain = plain_of(&state, &dek, &s).map_err(internal)?;
    let value_count = plain.all_values.len();
    audit(
        &state,
        &user_id,
        AuditEntry {
            detail: json!({ "values": value_count }),
            ..entry("scrubbed", &s, &format!("user:{user_id}"))
        },
    );
    // Every agent of this owner: a paste into any chat can leak the value.
    for p in state.db.projects_of_user(&user_id).unwrap_or_default() {
        push_box(
            &state,
            &p.id,
            "secret_scrub",
            json!({ "secret_id": s.id, "env": s.env, "ref": placeholder(&s.env, "value") }),
        );
    }
    Ok(Json(secret_envelope(&state, &s)).into_response())
}

/// `POST /box/secrets/from-message` body: the "save this" path (spec: secret hand-off).
#[derive(Deserialize, Default)]
pub struct FromMessageBody {
    #[serde(default)]
    pub chat_id: String,
    #[serde(default)]
    pub message_id: String,
    /// The chat is a people chat (the project is a member), not the agent's own.
    #[serde(default)]
    pub people_chat: bool,
    /// Wanted name; absent → the suggestion for the key's shape (`STRIPE_KEY`), `_2` on a clash.
    #[serde(default)]
    pub name: Option<String>,
    /// Becomes the note and the access note.
    #[serde(default)]
    pub purpose: Option<String>,
    /// `sk_l…3xA1` — picks one token when the message holds several; never the value.
    #[serde(default)]
    pub hint: Option<String>,
}

/// A name nobody has yet: `name`, else `name_2`, `name_3`, …
fn free_name(state: &AppState, user_id: &str, wanted: &str) -> Result<(String, String), ApiErr> {
    let (name, env) = validate_name(wanted)?;
    let taken = |n: &str| -> Result<bool, ApiErr> {
        Ok(state
            .db
            .secret_by_name(user_id, n)
            .map_err(internal)?
            .is_some())
    };
    if !taken(&name)? && !taken(&env)? {
        return Ok((name, env));
    }
    for i in 2..100 {
        let (n, e) = validate_name(&format!("{name}_{i}"))?;
        if !taken(&n)? && !taken(&e)? {
            return Ok((n, e));
        }
    }
    Err(err(
        StatusCode::CONFLICT,
        "already_exists",
        format!("You already have a secret called {env}."),
    ))
}

/// `POST /box/secrets/from-message` → 201 `{ secret, message_id, redacted: true }`.
///
/// The one place a value leaves a chat for the vault. The relay reads the message **once**
/// from the room doc, takes the token that looks like a key (or the one the hint names),
/// encrypts it into the owner's vault, grants the asking agent access, and rewrites the row
/// into the `secret_redacted` ghost ("Secret saved as NAME") — one update every device and the
/// box import — then replaces the room's stored history with a shallow checkpoint so no row
/// or checkpoint keeps the old text. Boxes get `secret_scrub` (sessions, memory, logs) and
/// the owner's devices `message_redacted` (search, notifications, local snapshot).
pub async fn box_from_message(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Json(b): Json<FromMessageBody>,
) -> ApiResult {
    let (user_id, project_id) = box_project(&state, &principal, &headers)?;
    if b.chat_id.trim().is_empty() || b.message_id.trim().is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "chat_id and message_id required",
        ));
    }
    let settings = state.db.secrets_settings(&user_id).map_err(internal)?;
    if settings.paused {
        return Err(err(
            StatusCode::FORBIDDEN,
            "paused",
            Refusal::Paused.message("", ""),
        ));
    }
    let room_project: Option<&str> = if b.people_chat {
        if !crate::people::room_access(&state, &principal, &b.chat_id) {
            return Err(err(
                StatusCode::FORBIDDEN,
                "forbidden",
                "not a member of that chat",
            ));
        }
        None
    } else {
        Some(project_id.as_str())
    };
    let doc = crate::matrix::backfill::load_doc(&state, &b.chat_id, room_project)
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such chat"))?;
    let msg = doc
        .message(&b.message_id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such message"))?;
    if msg.role != "user" {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "only a person's message can be saved",
        ));
    }
    // The value goes into the owner's vault, so it must be the owner's own message.
    let sender = msg
        .rest
        .get("sender")
        .and_then(Value::as_str)
        .unwrap_or("user");
    if sender != "user" && sender != format!("user:{user_id}") {
        return Err(err(
            StatusCode::FORBIDDEN,
            "not_owner_message",
            "only the owner's own message can go into the owner's vault",
        ));
    }
    let text = msg.text();
    let found = arbos_core::redact::find_key_like(&text);
    let pick = match b.hint.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
        Some(h) => found
            .iter()
            .find(|k| arbos_core::redact::matches_hint(k.value(&text), h)),
        None => match found.len() {
            0 => None,
            1 => found.first(),
            _ => found
                .iter()
                .find(|k| k.confident)
                .filter(|_| found.iter().filter(|k| k.confident).count() == 1),
        },
    };
    let Some(pick) = pick else {
        return Err(err(
            StatusCode::NOT_FOUND,
            if found.is_empty() {
                "no_key_found"
            } else {
                "ambiguous"
            },
            if found.is_empty() {
                "nothing in that message looks like a key"
            } else {
                "several tokens look like keys; say which with a hint"
            },
        ));
    };
    let value = pick.value(&text).to_owned();
    let wanted = b
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(pick.suggested_name);
    let (name, env) = free_name(&state, &user_id, wanted)?;
    let purpose: String = b
        .purpose
        .unwrap_or_default()
        .trim()
        .chars()
        .take(500)
        .collect();
    let dek = user_dek(&state, &user_id).map_err(internal)?;
    let now = now_ms();
    let id = ulid();
    let kind = if value.contains('\n') { "text" } else { "key" };
    let fields = fields_for_kind(kind, &json!({ "value": value, "text": value }))?;
    drop(value);
    let s = Secret {
        id: id.clone(),
        user_id: user_id.clone(),
        name,
        env,
        kind: kind.to_owned(),
        note: purpose.clone(),
        hosts: Vec::new(),
        ask_every_time: false,
        expires_at: None,
        current_version: 1,
        created_at: now,
        updated_at: now,
        last_used_at: None,
        daytona_secrets: json!({}),
        inject_env: false,
        others_ask: "refuse".into(),
        source_kind: "chat".into(),
        source_project_id: Some(project_id.clone()),
        source_chat_id: Some(b.chat_id.clone()),
    };
    let v1 = SecretVersion {
        secret_id: id.clone(),
        version: 1,
        fields_enc: encrypt_fields(&dek, &id, 1, &Value::Object(fields)).map_err(internal)?,
        created_at: now,
        retire_at: None,
    };
    state.db.insert_secret(&s, &v1).map_err(internal)?;
    audit(
        &state,
        &user_id,
        AuditEntry {
            detail: json!({ "kind": s.kind, "source": "chat", "project_id": project_id, "chat_id": b.chat_id, "message_id": b.message_id }),
            ..entry("created", &s, &format!("agent:{project_id}"))
        },
    );
    if let Some(a) = grant_access(
        &state,
        &user_id,
        &s,
        &project_id,
        (!purpose.is_empty()).then_some(purpose.as_str()),
        None,
        true,
        "chat",
    )? {
        after_access_change(&state, &s, &a);
    }

    // The row becomes the ghost; then the stored history forgets the old text.
    let redaction = arbos_core::doc::SecretRedaction {
        secret_id: s.id.clone(),
        name: s.name.clone(),
        env: s.env.clone(),
        saved_by: format!("user:{user_id}"),
        agent: Some(project_id.clone()),
    };
    let message_id = b.message_id.clone();
    crate::notices::write_doc(&state, room_project, &b.chat_id, move |doc| {
        anyhow::ensure!(
            doc.redact_message_as_secret(&message_id, &redaction),
            "message vanished"
        );
        Ok(())
    })
    .await
    .map_err(internal)?;
    crate::notices::compact_shallow(&state, room_project, &b.chat_id).map_err(internal)?;
    audit(
        &state,
        &user_id,
        AuditEntry {
            detail: json!({ "values": 1, "chat_id": b.chat_id, "message_id": b.message_id, "compacted": true }),
            ..entry("scrubbed", &s, &format!("agent:{project_id}"))
        },
    );
    // Every box of the owner scrubs its sessions, memory and logs (the value was in a chat
    // any of its agents may have read); the owner's devices drop search / notification copies.
    for p in state.db.projects_of_user(&user_id).unwrap_or_default() {
        push_box(
            &state,
            &p.id,
            "secret_scrub",
            json!({ "secret_id": s.id, "env": s.env, "ref": placeholder(&s.env, "value"), "chat_id": b.chat_id, "message_id": b.message_id }),
        );
    }
    push_user(
        &state,
        &user_id,
        PushEventDto::MessageRedacted(MessageRedactedPayloadDto {
            chat_id: b.chat_id.clone(),
            project_id: room_project.map(str::to_owned),
            message_id: b.message_id.clone(),
            secret_id: s.id.clone(),
            name: s.name.clone(),
        }),
    );
    let mut body = to_json(secret_envelope(&state, &s));
    body["message_id"] = json!(b.message_id);
    body["redacted"] = json!(true);
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

/// `GET /projects/{id}/secrets` → `{ paused, items, requests, activity }` for the agent profile.
/// Owner or guest may read.
pub async fn project_secrets(
    State(state): State<AppState>,
    principal: Principal,
    Path(project_id): Path<String>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let p = state
        .db
        .project(&project_id)
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such agent"))?;
    let member =
        p.user_id == user_id || state.db.is_project_member(&p.id, &user_id).unwrap_or(false);
    if !member {
        return Err(err(StatusCode::FORBIDDEN, "forbidden", "not your agent"));
    }
    let owner = p.user_id.clone();
    let settings = state.db.secrets_settings(&owner).map_err(internal)?;
    let mut items = Vec::new();
    for a in state.db.accesses_of_project(&p.id).map_err(internal)? {
        if a.target_kind == "secret" {
            if let Some(s) = state.db.secret(&owner, &a.target_id).map_err(internal)? {
                items.push(project_secret_item(&state, &a, &s));
            }
        } else if let Some(v) = crate::vaults::access_item_dto(&state, &owner, &a) {
            items.push(v);
        }
    }
    let requests: Vec<SecretRequestDto> = state
        .db
        .list_secret_requests(&owner, Some(&p.id), 50)
        .map_err(internal)?
        .iter()
        .map(|r| request_dto(&state, r))
        .collect();
    let act = activity(
        &state,
        &owner,
        AuditQuery {
            project_id: Some(p.id.clone()),
            since: Some(now_ms() - ACTIVITY_WINDOW_MS),
            limit: 50,
            ..Default::default()
        },
    );
    Ok(Json(ProjectSecretsDto {
        paused: settings.paused,
        owner: owner == user_id,
        items,
        requests,
        activity: act,
    })
    .into_response())
}

// ---- device: kill switch, activity, requests ---------------------------------------------------

#[derive(Deserialize)]
pub struct PauseBody {
    pub paused: bool,
}

/// Flip the kill switch for a user: relay refuses at once; boxes hear within 5 s; Daytona
/// mounts within 15 s.
/// The pause `POST /machine/stop` set (D83 decision 8, `paused_by: stop_machine`) is the kill
/// switch for the downtime, not a standing choice: `POST /machine/start` lifts it. A pause the
/// user set themselves (`paused_by: user`) stays (#1664: every restart used to leave all
/// secrets paused and the agent said "paused by the owner").
pub(crate) async fn resume_after_stop(state: &AppState, user_id: &str) -> bool {
    let settings = state.db.secrets_settings(user_id).unwrap_or_default();
    if settings.paused && settings.paused_by.as_deref() == Some("stop_machine") {
        set_paused(state, user_id, false, "start_machine").await;
        return true;
    }
    false
}

pub(crate) async fn set_paused(
    state: &AppState,
    user_id: &str,
    paused: bool,
    by: &str,
) -> SecretsPausedDto {
    let before = state.db.secrets_settings(user_id).unwrap_or_default();
    let settings = state
        .db
        .set_secrets_paused(user_id, paused, by)
        .unwrap_or_default();
    if before.paused != paused {
        audit(
            state,
            user_id,
            AuditEntry {
                action: if paused { "paused" } else { "unpaused" }.into(),
                target_kind: "user".into(),
                target_id: user_id.to_owned(),
                target_name: "all secrets".into(),
                by: by.to_owned(),
                detail: json!({}),
                ..Default::default()
            },
        );
    }
    for p in state.db.projects_of_user(user_id).unwrap_or_default() {
        push_box(state, &p.id, "secrets_paused", json!({ "paused": paused }));
    }
    push_user(
        state,
        user_id,
        PushEventDto::SecretsPaused(SecretsPausedPayloadDto { paused }),
    );
    remount_daytona(state, user_id).await;
    paused_dto(&settings)
}

/// `PUT /me/secrets/settings` `{ paused }`.
pub async fn put_settings(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<PauseBody>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    Ok(Json(set_paused(&state, &user_id, b.paused, "user").await).into_response())
}

#[derive(Deserialize, Default)]
pub struct ActivityQuery {
    #[serde(default)]
    pub secret_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub vault_id: Option<String>,
    #[serde(default)]
    pub before: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// `GET /secrets/activity?secret_id=&project_id=&vault_id=&before=&limit=` → `{ items, next_before }`.
pub async fn list_activity(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<ActivityQuery>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let items = activity(
        &state,
        &user_id,
        AuditQuery {
            target_id: q.secret_id.or(q.vault_id),
            project_id: q.project_id,
            before: q.before,
            since: None,
            limit,
        },
    );
    let next_before = if items.len() as i64 == limit {
        items.last().map(|a| a.at)
    } else {
        None
    };
    Ok(Json(SecretActivityPageDto { items, next_before }).into_response())
}

/// `GET /secrets/requests` → `{ items }` (pending first).
pub async fn list_requests(State(state): State<AppState>, principal: Principal) -> ApiResult {
    let user_id = device_user(&principal)?;
    let items: Vec<SecretRequestDto> = state
        .db
        .list_secret_requests(&user_id, None, 100)
        .map_err(internal)?
        .iter()
        .map(|r| request_dto(&state, r))
        .collect();
    Ok(Json(SecretRequestsDto { items }).into_response())
}

#[derive(Deserialize)]
pub struct DecideBody {
    /// `once | chat | always | deny`.
    pub decision: String,
}

/// The four card answers → request status.
fn status_for(decision: &str) -> Option<&'static str> {
    match decision {
        "once" | "allow_once" | "approve" => Some("allowed_once"),
        "chat" | "allow_chat" => Some("allowed_chat"),
        "always" | "allow_always" | "approved" => Some("allowed_always"),
        "deny" | "denied" | "reject" => Some("denied"),
        _ => None,
    }
}

/// Apply a decision: `chat`/`always` create (or re-enable) the Access with the request's
/// purpose as the note and its host pinned; `once` is a 5-minute grant; `deny` is final.
pub(crate) async fn decide(
    state: &AppState,
    user_id: &str,
    request_id: &str,
    decision: &str,
    decided_by: &str,
) -> Result<(SecretRequest, Option<Access>), ApiErr> {
    let status = status_for(decision).ok_or_else(|| {
        err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "decision must be once, chat, always or deny",
        )
    })?;
    let rq = state
        .db
        .decide_secret_request(user_id, request_id, status, decided_by)
        .map_err(internal)?
        .ok_or_else(|| {
            err(
                StatusCode::CONFLICT,
                "not_pending",
                "request is not pending (already decided, expired, or not yours)",
            )
        })?;
    let mut access = None;
    let secret = match (rq.target_kind.as_str(), rq.target_id.as_deref()) {
        ("secret", Some(id)) => state.db.secret(user_id, id).map_err(internal)?,
        _ => None,
    };
    if let Some(s) = &secret {
        audit(
            state,
            user_id,
            AuditEntry {
                project_id: Some(rq.project_id.clone()),
                chat_id: Some(rq.chat_id.clone()),
                request_id: Some(rq.id.clone()),
                use_kind: Some(rq.use_kind.clone()),
                host: rq.host.clone(),
                detail: json!({ "decision": status }),
                ..entry("decided", s, decided_by)
            },
        );
        if matches!(status, "allowed_chat" | "allowed_always") {
            let hosts = rq.host.clone().map(|h| vec![h]);
            let mut a = grant_access(
                state,
                user_id,
                s,
                &rq.project_id,
                Some(&rq.purpose),
                None,
                true,
                &format!("card:{}", rq.id),
            )?
            .ok_or_else(not_found)?;
            if let Some(h) = hosts {
                for host in h {
                    if !a.hosts.contains(&host) && !s.hosts.contains(&host) {
                        a.hosts.push(host);
                    }
                }
            }
            if status == "allowed_chat" {
                a.scope_kind = "chat".into();
                a.scope_chat_id = Some(rq.chat_id.clone());
            } else {
                a.scope_kind = "all".into();
                a.scope_chat_id = None;
            }
            state.db.upsert_access(&a).map_err(internal)?;
            after_access_change(state, s, &a);
            access = Some(a);
            // The ask came from a person who is not the owner: the answer also puts them on
            // the People list — for this chat, or standing (Jacob, 2026-10-23: "Always allow
            // @person" adds them).
            if let Some(person) = rq
                .asked_by
                .as_deref()
                .filter(|p| *p != user_id && *p != arbos_core::UNKNOWN_ASKER)
            {
                let now = now_ms();
                let existing = state.db.secret_director(&s.id, person).map_err(internal)?;
                let is_new = existing.is_none();
                let mut d = existing.unwrap_or_else(|| SecretDirector::new(&s.id, person, "", now));
                d.enabled = true;
                d.disabled_at = None;
                if status == "allowed_chat" {
                    // A standing row stays standing; a new or chat row covers this chat now.
                    if is_new || d.scope_kind == "chat" {
                        d.scope_kind = "chat".into();
                        d.scope_chat_id = Some(rq.chat_id.clone());
                    }
                } else {
                    d.scope_kind = "all".into();
                    d.scope_chat_id = None;
                }
                if is_new {
                    d.created_from = format!("card:{}", rq.id);
                }
                state.db.upsert_secret_director(&d).map_err(internal)?;
                after_people_change(state, s);
                let (handle, _) = user_handle_name(state, person);
                audit(
                    state,
                    user_id,
                    AuditEntry {
                        request_id: Some(rq.id.clone()),
                        chat_id: Some(rq.chat_id.clone()),
                        detail: json!({ "person_user_id": person, "handle": handle, "scope": d.scope_kind }),
                        ..entry("person_on", s, decided_by)
                    },
                );
            }
        }
    } else if rq.target_kind == "vault" {
        access = crate::vaults::decide_vault_request(state, user_id, &rq, status).await?;
    }
    push_box(
        state,
        &rq.project_id,
        "secret_decided",
        json!({ "request_id": rq.id, "status": rq.status, "access_id": access.as_ref().map(|a| a.id.clone()),
            "ref": rq.target_ref, "use": rq.use_kind, "host": rq.host }),
    );
    Ok((rq, access))
}

/// `POST /secrets/requests/{id}/decide { decision }` → `{ request, access? }`.
pub async fn decide_request(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<DecideBody>,
) -> ApiResult {
    let user_id = device_user(&principal)?;
    let (rq, access) = decide(&state, &user_id, &id, &b.decision, &format!("card:{id}")).await?;
    Ok(Json(DecideSecretResponseDto {
        request: request_dto(&state, &rq),
        access: access.map(|a| access_dto(&state, &a)),
    })
    .into_response())
}

// ---- box ---------------------------------------------------------------------------------------

/// What this project may use right now, names only.
pub(crate) fn box_items(state: &AppState, user_id: &str, project_id: &str) -> Vec<Value> {
    let mut items = Vec::new();
    for s in state.db.list_secrets(user_id).unwrap_or_default() {
        let access = state.db.access(project_id, "secret", &s.id).ok().flatten();
        let fields: Vec<&str> = field_names(&s.kind);
        let placeholders: BTreeMap<String, String> = fields
            .iter()
            .map(|f| ((*f).to_owned(), placeholder(&s.env, f)))
            .collect();
        let mut hosts = s.hosts.clone();
        if let Some(a) = &access {
            hosts.extend(a.hosts.iter().cloned());
        }
        hosts.dedup();
        let enabled = access.as_ref().is_some_and(|a| a.enabled)
            && !s.expires_at.is_some_and(|t| t < now_ms());
        let people_rows = state.db.secret_directors(&s.id).unwrap_or_default();
        // The box only hears about rows that are on; an off row is as if it were not there.
        let directors: Vec<Value> = people_rows
            .iter()
            .filter(|d| d.enabled)
            .map(|d| director_json(state, d))
            .collect();
        // Constraints the agent must honour when a listed person asks: secret note + each
        // person's note (e.g. "only 1k usd of credit usage per day").
        let mut use_notes: Vec<String> = Vec::new();
        if !s.note.trim().is_empty() {
            use_notes.push(s.note.trim().to_owned());
        }
        for d in people_rows.iter().filter(|d| d.enabled) {
            let note = d.note.trim();
            if note.is_empty() {
                continue;
            }
            let who = state
                .db
                .user_card(&d.user_id)
                .ok()
                .flatten()
                .map(|c| {
                    c.name
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| c.handle.trim_start_matches('@').to_owned())
                })
                .unwrap_or_else(|| d.user_id.clone());
            use_notes.push(format!("When {who} asks you: {note}"));
        }
        let who_may_direct = if s.others_ask == "card" {
            "the owner, and the people listed (rows that are on). Anyone else: call secrets_request first — the owner gets a card naming who asked."
        } else {
            "the owner, and the people listed (rows that are on). Anyone else cannot use this secret; tell them so."
        };
        items.push(json!({
            "id": s.id, "name": s.name, "env": s.env, "kind": s.kind, "note": s.note, "hosts": hosts,
            "ref": placeholder(&s.env, if s.kind == "login" { "password" } else { "value" }),
            "fields": fields, "placeholders": placeholders, "vault_name": ARBOS_VAULT_NAME,
            "access_id": access.as_ref().map(|a| a.id.clone()), "enabled": enabled,
            "ask_every_time": s.ask_every_time, "inject_env": s.inject_env, "expires_at": s.expires_at,
            "scope": access.as_ref().map(|a| json!({ "kind": a.scope_kind, "chat_id": a.scope_chat_id })),
            "directors": directors,
            "people": directors,
            "others_ask": s.others_ask,
            "use_notes": use_notes,
            "who_may_direct": who_may_direct,
        }));
    }
    items.extend(crate::vaults::box_items(state, user_id, project_id));
    items
}

/// `GET /box/secrets/env` → `{ paused, items: [{ id, name, env, kind, vault_name, vars: { NAME: value } }] }`:
/// the real values this project's box puts in its environment — secrets whose owner switched
/// "Real value in the agent's environment" on and gave this agent Access, plus connected-vault
/// items the owner mapped to an env name (resolved here, never in the box). Empty while paused.
/// Every value handed over is audited as `injected` (D83 decision 2 as decided 2026-10-18).
pub async fn box_env(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
) -> ApiResult {
    let (user_id, project_id) = box_project(&state, &principal, &headers)?;
    let paused = state
        .db
        .secrets_settings(&user_id)
        .map(|s| s.paused)
        .unwrap_or(false);
    let mut items = Vec::new();
    if !paused {
        let dek = user_dek(&state, &user_id).map_err(internal)?;
        for s in state.db.list_secrets(&user_id).map_err(internal)? {
            if !s.inject_env || s.expires_at.is_some_and(|t| t < now_ms()) {
                continue;
            }
            let Some(a) = state
                .db
                .access(&project_id, "secret", &s.id)
                .map_err(internal)?
                .filter(|a| a.enabled)
            else {
                continue;
            };
            let plain = plain_of(&state, &dek, &s).map_err(internal)?;
            let vars = env_vars(&s.env, &plain.current);
            audit(
                &state,
                &user_id,
                AuditEntry {
                    project_id: Some(project_id.clone()),
                    access_id: Some(a.id.clone()),
                    use_kind: Some("env".into()),
                    detail: json!({ "vars": vars.keys().collect::<Vec<_>>() }),
                    ..entry("injected", &s, &format!("agent:{project_id}"))
                },
            );
            items.push(
                json!({ "id": s.id, "name": s.name, "env": s.env, "kind": s.kind,
                "vault_name": ARBOS_VAULT_NAME, "vars": vars }),
            );
        }
        items.extend(crate::vaults::box_env_items(&state, &user_id, &project_id).await);
    }
    Ok(Json(json!({ "paused": paused, "items": items })).into_response())
}

/// `ENV` for a one-field secret, `ENV_FIELD` for logins; `url` is metadata, not a variable.
pub(crate) fn env_vars(env: &str, fields: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let vars: Vec<(&String, &String)> = fields.iter().filter(|(k, _)| *k != "url").collect();
    vars.into_iter()
        .map(|(k, v)| {
            let name = if k == "value" || fields.len() == 1 {
                env.to_owned()
            } else {
                format!("{env}_{}", k.to_ascii_uppercase())
            };
            (name, v.clone())
        })
        .collect()
}

/// `GET /box/secrets` → `{ paused, items }`.
pub async fn box_list(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
) -> ApiResult {
    let (user_id, project_id) = box_project(&state, &principal, &headers)?;
    let paused = state
        .db
        .secrets_settings(&user_id)
        .map(|s| s.paused)
        .unwrap_or(false);
    Ok(
        Json(json!({ "paused": paused, "items": box_items(&state, &user_id, &project_id) }))
            .into_response(),
    )
}

/// `GET /box/secrets/{id}/scrub` → `{ env, values }` for a just-requested scrub. The values
/// exist only so the box can replace them in chats, sessions and logs; they are not granted
/// for use. Requires a `scrubbed` audit row within [`SCRUB_WINDOW_MS`].
pub async fn box_scrub(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let (user_id, _project_id) = box_project(&state, &principal, &headers)?;
    let s = state
        .db
        .secret(&user_id, &id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let recent = state
        .db
        .list_audit(
            &user_id,
            &AuditQuery {
                target_id: Some(s.id.clone()),
                since: Some(now_ms() - SCRUB_WINDOW_MS),
                limit: 20,
                ..Default::default()
            },
        )
        .map_err(internal)?;
    if !recent.iter().any(|a| a.action == "scrubbed") {
        return Err(err(
            StatusCode::FORBIDDEN,
            "forbidden",
            "no scrub in progress for this secret",
        ));
    }
    let dek = user_dek(&state, &user_id).map_err(internal)?;
    let plain = plain_of(&state, &dek, &s).map_err(internal)?;
    Ok(Json(json!({ "env": s.env, "values": plain.all_values })).into_response())
}

#[derive(Deserialize)]
pub struct BoxRequestBody {
    #[serde(default, alias = "ref")]
    pub reference: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub purpose: String,
    #[serde(rename = "use", default)]
    pub use_kind: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default, alias = "chatId")]
    pub chat_id: String,
    /// Human directing this turn (`user_id`). Absent = owner / unknown.
    #[serde(default, alias = "askedBy")]
    pub asked_by: Option<String>,
}

/// `POST /box/secrets/request` → 201 `{ request }` (a card is needed) or 200
/// `{ request, status: "allowed_always", access_id }` when an Access already covers it.
pub async fn box_request(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Json(b): Json<BoxRequestBody>,
) -> ApiResult {
    let (user_id, project_id) = box_project(&state, &principal, &headers)?;
    let use_kind = b.use_kind.as_deref().unwrap_or("header").trim().to_owned();
    if !USES.contains(&use_kind.as_str()) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            format!("use must be one of {}", USES.join(", ")),
        ));
    }
    let settings = state.db.secrets_settings(&user_id).map_err(internal)?;
    if settings.paused {
        return Err(err(
            StatusCode::FORBIDDEN,
            "paused",
            Refusal::Paused.message("", ""),
        ));
    }
    let host = b
        .host
        .as_deref()
        .map(|h| clean_hosts(&[h.to_owned()]))
        .transpose()?
        .and_then(|h| h.into_iter().next());
    let name = b
        .reference
        .as_deref()
        .or(b.name.as_deref())
        .unwrap_or("")
        .trim()
        .to_owned();
    if name.starts_with("op://") {
        return crate::vaults::box_request(
            &state,
            &user_id,
            &project_id,
            &name,
            &b.purpose,
            &use_kind,
            host,
            &b.chat_id,
        )
        .await;
    }
    let name = name
        .strip_prefix(PLACEHOLDER_SCHEME)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(&name)
        .to_owned();
    let Some(s) = state.db.secret_by_name(&user_id, &name).map_err(internal)? else {
        return Err(err(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("{name}: no such secret (the user adds secrets in Settings → Secrets)"),
        ));
    };
    if use_kind == "browser_fill" && s.kind != "login" {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            format!("{}: browser_fill needs a Login", s.env),
        ));
    }
    let chat_id = if b.chat_id.is_empty() {
        None
    } else {
        Some(b.chat_id.as_str())
    };
    let asked_by = b
        .asked_by
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty());
    // A person who is not on the People list: refuse flat (default), or card the owner.
    if !may_direct(&state, &s, asked_by, chat_id) && s.others_ask != "card" {
        let why = Refusal::NotDirector;
        audit_refusal(
            &state,
            &user_id,
            &s.env,
            Some(&s),
            &project_id,
            host.as_deref().unwrap_or(""),
            &use_kind,
            asked_by,
            &why,
        );
        return Err(err(
            StatusCode::FORBIDDEN,
            why.kind(),
            why.message(&s.env, host.as_deref().unwrap_or("")),
        ));
    }
    // Standing access covers it: no card, but a decided request row so `values` has a window.
    if !s.ask_every_time {
        if let Ok(allowed) = decide_use(
            &state,
            &s,
            &project_id,
            host.as_deref().unwrap_or(""),
            &use_kind,
            chat_id,
            asked_by,
        ) {
            if allowed.once.is_none() {
                if let Some(a) = &allowed.access {
                    let now = now_ms();
                    let rq = SecretRequest {
                        id: ulid(),
                        user_id: user_id.clone(),
                        project_id: project_id.clone(),
                        chat_id: b.chat_id.clone(),
                        target_kind: "secret".into(),
                        target_id: Some(s.id.clone()),
                        target_ref: s.env.clone(),
                        purpose: b.purpose.chars().take(400).collect(),
                        use_kind: use_kind.clone(),
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
                        after_use(
                            &state,
                            &s,
                            &project_id,
                            host.as_deref().unwrap_or(""),
                            &use_kind,
                            chat_id,
                            asked_by,
                            &Allowed {
                                access: Some(a.clone()),
                                once: None,
                                pin_host: true,
                            },
                        );
                    }
                    return Ok(Json(json!({ "request": request_json(&state, &rq), "status": "allowed_always", "access_id": a.id, "item": box_item(&state, &s, Some(a)) }))
                        .into_response());
                }
            }
        }
    }
    // Resume an undecided request for the same thing instead of a second card.
    if let Some(rq) = state
        .db
        .pending_secret_request_for(&project_id, &s.env, &use_kind)
        .map_err(internal)?
    {
        return Ok((StatusCode::CREATED, Json(json!({ "request": request_json(&state, &rq), "status": "pending", "resumed": true })))
            .into_response());
    }
    let now = now_ms();
    let rq = SecretRequest {
        id: ulid(),
        user_id: user_id.clone(),
        project_id: project_id.clone(),
        chat_id: b.chat_id.clone(),
        target_kind: "secret".into(),
        target_id: Some(s.id.clone()),
        target_ref: s.env.clone(),
        purpose: b.purpose.chars().take(400).collect(),
        use_kind: use_kind.clone(),
        host: host.clone(),
        status: "pending".into(),
        decided_by: None,
        created_at: now,
        decided_at: None,
        expires_at: now + REQUEST_PENDING_MS,
        asked_by: asked_by.filter(|u| *u != user_id).map(str::to_owned),
    };
    state.db.insert_secret_request(&rq).map_err(internal)?;
    let mut detail = asked_by_detail(&state, rq.asked_by.as_deref());
    detail["purpose"] = json!(rq.purpose);
    audit(
        &state,
        &user_id,
        AuditEntry {
            project_id: Some(project_id.clone()),
            chat_id: chat_id.map(str::to_owned),
            request_id: Some(rq.id.clone()),
            use_kind: Some(use_kind.clone()),
            host: host.clone(),
            detail,
            ..entry("requested", &s, &format!("agent:{project_id}"))
        },
    );
    let agent = state
        .db
        .project(&project_id)
        .ok()
        .flatten()
        .map(|p| p.name)
        .unwrap_or_default();
    push_user(
        &state,
        &user_id,
        secret_request_push(
            rq.id.clone(),
            project_id.clone(),
            agent,
            s.env.clone(),
            host.clone(),
            use_kind.clone(),
        ),
    );
    Ok((
        StatusCode::CREATED,
        Json(json!({ "request": request_json(&state, &rq), "status": "pending" })),
    )
        .into_response())
}

fn box_item(state: &AppState, s: &Secret, access: Option<&Access>) -> Value {
    box_items(
        state,
        &s.user_id,
        access.map(|a| a.project_id.as_str()).unwrap_or(""),
    )
    .into_iter()
    .find(|v| v.get("id").and_then(Value::as_str) == Some(s.id.as_str()))
    .unwrap_or(Value::Null)
}

/// `GET /box/secrets/requests/{id}` → `{ request }` (own project only).
pub async fn box_request_status(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let (_user_id, project_id) = box_project(&state, &principal, &headers)?;
    let rq = state
        .db
        .secret_request(&id)
        .map_err(internal)?
        .filter(|r| r.project_id == project_id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such request"))?;
    Ok(Json(json!({ "request": request_json(&state, &rq) })).into_response())
}

/// `POST /box/secrets/requests/{id}/values` → login fields for one allowed `browser_fill`
/// request within ten minutes of the decision. The only path on which a value leaves the relay
/// in clear; agentd types it and forgets it.
pub async fn box_request_values(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let (user_id, project_id) = box_project(&state, &principal, &headers)?;
    if state
        .db
        .secrets_settings(&user_id)
        .map_err(internal)?
        .paused
    {
        return Err(err(
            StatusCode::FORBIDDEN,
            "paused",
            Refusal::Paused.message("", ""),
        ));
    }
    let rq = state
        .db
        .secret_request(&id)
        .map_err(internal)?
        .filter(|r| r.project_id == project_id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such request"))?;
    if rq.use_kind != "browser_fill" {
        return Err(err(
            StatusCode::FORBIDDEN,
            "forbidden",
            "values are only released for browser_fill requests; use /egress",
        ));
    }
    if !rq.is_allowed() {
        return Err(err(
            StatusCode::CONFLICT,
            "not_pending",
            format!("request is {}", rq.status),
        ));
    }
    if rq.decided_at.is_some_and(|t| now_ms() - t > FILL_WINDOW_MS) {
        return Err(err(
            StatusCode::CONFLICT,
            "window_closed",
            "approval window closed; ask again",
        ));
    }
    let s = rq
        .target_id
        .as_deref()
        .and_then(|i| state.db.secret(&user_id, i).ok().flatten())
        .ok_or_else(|| err(StatusCode::GONE, "not_found", "the secret was deleted"))?;
    let host = rq.host.clone().unwrap_or_default();
    // The standing rule still applies at the moment of use. The asker was already judged when
    // the owner answered the card (`asked_by` is not re-checked here).
    let allowed = match decide_use(
        &state,
        &s,
        &project_id,
        &host,
        "browser_fill",
        Some(rq.chat_id.as_str()),
        None,
    ) {
        Ok(a) => a,
        Err(
            Refusal::NoAccess
            | Refusal::AskEveryTime
            | Refusal::NotDirector
            | Refusal::AskOwnerFirst,
        ) if rq.status == "allowed_once" || rq.status == "allowed_chat" => Allowed {
            access: None,
            once: None,
            pin_host: false,
        },
        Err(why) => {
            audit_refusal(
                &state,
                &user_id,
                &s.env,
                Some(&s),
                &project_id,
                &host,
                "browser_fill",
                rq.asked_by.as_deref(),
                &why,
            );
            return Err(err(
                StatusCode::FORBIDDEN,
                why.kind(),
                why.message(&s.env, &host),
            ));
        }
    };
    let dek = user_dek(&state, &user_id).map_err(internal)?;
    let plain = plain_of(&state, &dek, &s).map_err(internal)?;
    after_use(
        &state,
        &s,
        &project_id,
        &host,
        "browser_fill",
        Some(rq.chat_id.as_str()),
        rq.asked_by.as_deref(),
        &allowed,
    );
    Ok(Json(
        json!({ "name": s.env, "kind": s.kind, "allowed_hosts": s.hosts, "fields": plain.current }),
    )
    .into_response())
}

// ---- egress resolution -------------------------------------------------------------------------

/// Everything `/egress` needs for one request: usable items with plaintext, refusals by
/// placeholder key, and every value (current and previous) to redact from the reply.
pub(crate) struct EgressPlan {
    pub items: Vec<crate::vault::Resolved>,
    pub refused: Vec<(String, String)>,
    pub redact: Vec<String>,
    /// `(secret, allowed)` to settle after the request goes out.
    pub used: Vec<(Secret, Allowed)>,
}

/// Decide every placeholder in `texts` for `project_id` at `host`. Values are decrypted only
/// for secrets the rule allows. `asked_by` is the human directing this turn.
pub(crate) fn plan_egress(
    state: &AppState,
    user_id: &str,
    project_id: &str,
    host: &str,
    chat_id: Option<&str>,
    asked_by: Option<&str>,
    texts: &[&str],
) -> Result<EgressPlan, ApiErr> {
    let mut plan = EgressPlan {
        items: Vec::new(),
        refused: Vec::new(),
        redact: Vec::new(),
        used: Vec::new(),
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut dek: Option<Dek> = None;
    for t in texts {
        for (env, field) in crate::vault::find_placeholders(t) {
            if !seen.insert(env.clone()) {
                continue;
            }
            if env.starts_with("op:") {
                continue;
            }
            let Some(s) = state.db.secret_by_name(user_id, &env).map_err(internal)? else {
                plan.refused.push((
                    format!("{env}/{field}"),
                    Refusal::NoSuch.message(&env, host),
                ));
                audit_refusal(
                    state,
                    user_id,
                    &env,
                    None,
                    project_id,
                    host,
                    "header",
                    asked_by,
                    &Refusal::NoSuch,
                );
                continue;
            };
            match decide_use(state, &s, project_id, host, "header", chat_id, asked_by) {
                Ok(allowed) => {
                    if dek.is_none() {
                        dek = Some(user_dek(state, user_id).map_err(internal)?);
                    }
                    let plain =
                        plain_of(state, dek.as_ref().expect("dek"), &s).map_err(internal)?;
                    for v in &plain.all_values {
                        if !plan.redact.contains(v) {
                            plan.redact.push(v.clone());
                        }
                    }
                    let mut fields = plain.current;
                    // Logins: the password is the secret; the username is metadata but
                    // substitutable too (basic auth), the URL is not.
                    fields.remove("url");
                    plan.items.push(crate::vault::Resolved {
                        name: s.env.clone(),
                        allowed_hosts: vec![host.to_owned()],
                        fields,
                        approved: true,
                    });
                    plan.used.push((s, allowed));
                }
                Err(why) => {
                    plan.refused
                        .push((format!("{env}/{field}"), why.message(&s.env, host)));
                    audit_refusal(
                        state,
                        user_id,
                        &s.env,
                        Some(&s),
                        project_id,
                        host,
                        "header",
                        asked_by,
                        &why,
                    );
                }
            }
        }
    }
    Ok(plan)
}

// ---- admin -------------------------------------------------------------------------------------

/// `GET /admin/secrets` → counts per user, paused users, provider status.
pub async fn admin_status(State(state): State<AppState>, principal: Principal) -> ApiResult {
    if !matches!(principal, Principal::Admin) {
        return Err(err(StatusCode::FORBIDDEN, "forbidden", "admin only"));
    }
    let users: Vec<Value> = state
        .db
        .secrets_counts()
        .map_err(internal)?
        .into_iter()
        .map(|(u, n, on)| json!({ "user_id": u, "secrets": n, "accesses_on": on }))
        .collect();
    Ok(Json(json!({
        "users": users,
        "paused": state.db.paused_user_ids().unwrap_or_default(),
        "kek_from_env": crate::connectors::crypto::RelayKey::from_env(),
        "providers": crate::vaults::admin_status(&state),
    }))
    .into_response())
}

#[derive(Deserialize, Default)]
pub struct AdminPauseBody {
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub all: bool,
    #[serde(default = "default_true")]
    pub paused: bool,
}

/// `POST /admin/secrets/pause { user_id | all, paused? }`.
pub async fn admin_pause(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<AdminPauseBody>,
) -> ApiResult {
    if !matches!(principal, Principal::Admin) {
        return Err(err(StatusCode::FORBIDDEN, "forbidden", "admin only"));
    }
    let users: Vec<String> = if b.all {
        state
            .db
            .secrets_counts()
            .map_err(internal)?
            .into_iter()
            .map(|(u, _, _)| u)
            .chain(state.db.paused_user_ids().unwrap_or_default())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    } else {
        b.user_id.into_iter().collect()
    };
    let mut out = Vec::new();
    for u in users {
        let v = set_paused(&state, &u, b.paused, "admin").await;
        out.push(json!({ "user_id": u, "paused": v.paused }));
    }
    Ok(Json(json!({ "users": out })).into_response())
}

// ---- F8 migration and sweeper ------------------------------------------------------------------

/// One-shot: `vault_items` → secrets (version 1, re-encrypted under the owner's DEK), approved
/// `vault_requests` → Access rows, pending → requests, the rest → audit. Keyed in `meta`.
pub fn migrate_f8(state: &AppState) -> anyhow::Result<()> {
    const KEY: &str = "secrets_f8_migrated";
    if state.db.meta_get(KEY)?.is_some() {
        return Ok(());
    }
    let mut moved = 0;
    let users: Vec<String> = state.db.vault_item_users()?;
    for user_id in users {
        let dek = user_dek(state, &user_id)?;
        for it in state.db.list_vault_items(&user_id)? {
            let plain = match state.relay_key.decrypt(&it.fields_enc) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, item = %it.name, "F8 migration: item did not decrypt; skipped");
                    continue;
                }
            };
            let fields: Value = serde_json::from_str(&plain).unwrap_or(json!({}));
            let kind = if it.kind == "login" { "login" } else { "key" };
            let (name, env) = match validate_name(&it.name) {
                Ok(v) => v,
                Err(_) => (it.name.clone(), env_from_name(&it.name)),
            };
            if state.db.secret_by_name(&user_id, &env)?.is_some() {
                continue;
            }
            let now = now_ms();
            let s = Secret {
                id: it.id.clone(),
                user_id: user_id.clone(),
                name,
                env,
                kind: kind.into(),
                note: String::new(),
                hosts: it.allowed_hosts.clone(),
                ask_every_time: false,
                expires_at: None,
                current_version: 1,
                created_at: it.created_at,
                updated_at: now,
                last_used_at: it.last_used_at,
                daytona_secrets: it.daytona_secrets.clone(),
                inject_env: false,
                others_ask: "refuse".into(),
                source_kind: "settings".into(),
                source_project_id: None,
                source_chat_id: None,
            };
            let v1 = SecretVersion {
                secret_id: s.id.clone(),
                version: 1,
                fields_enc: encrypt_fields(&dek, &s.id, 1, &fields)?,
                created_at: it.created_at,
                retire_at: None,
            };
            state.db.insert_secret(&s, &v1)?;
            moved += 1;
            for rq in state.db.vault_requests_for_item(&it.id)? {
                match rq.status.as_str() {
                    "approved" => {
                        let a = Access {
                            id: ulid(),
                            user_id: user_id.clone(),
                            project_id: rq.project_id.clone(),
                            target_kind: "secret".into(),
                            target_id: s.id.clone(),
                            note: rq.purpose.clone(),
                            hosts: vec![],
                            scope_kind: "all".into(),
                            scope_chat_id: None,
                            enabled: true,
                            created_from: format!("card:{}", rq.id),
                            created_at: rq.decided_at.unwrap_or(rq.created_at),
                            disabled_at: None,
                            last_used_at: None,
                        };
                        state.db.upsert_access(&a)?;
                    }
                    "pending" => {
                        let now = now_ms();
                        state.db.insert_secret_request(&SecretRequest {
                            id: rq.id.clone(),
                            user_id: user_id.clone(),
                            project_id: rq.project_id.clone(),
                            chat_id: rq.chat_id.clone(),
                            target_kind: "secret".into(),
                            target_id: Some(s.id.clone()),
                            target_ref: s.env.clone(),
                            purpose: rq.purpose.clone(),
                            use_kind: rq.use_kind.clone(),
                            host: None,
                            status: "pending".into(),
                            decided_by: None,
                            created_at: rq.created_at,
                            decided_at: None,
                            expires_at: now + REQUEST_PENDING_MS,
                            asked_by: None,
                        })?;
                    }
                    other => audit(
                        state,
                        &user_id,
                        AuditEntry {
                            project_id: Some(rq.project_id.clone()),
                            request_id: Some(rq.id.clone()),
                            use_kind: Some(rq.use_kind.clone()),
                            detail: json!({ "decision": other, "migrated": true }),
                            ..entry("decided", &s, "system")
                        },
                    ),
                }
            }
            audit(
                state,
                &user_id,
                AuditEntry {
                    detail: json!({ "migrated_from": "vault_items" }),
                    ..entry("created", &s, "system")
                },
            );
        }
    }
    state.db.meta_set(KEY, "1")?;
    if moved > 0 {
        tracing::info!(moved, "F8 vault items migrated to secrets");
    }
    Ok(())
}

/// Every minute: secrets past `expires_at` lose their accesses (once, audited), and the owner
/// hears three days ahead (`secret_expiring`, once).
pub async fn spawn_sweeper(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            sweep_once(&state).await;
        }
    });
}

async fn sweep_once(state: &AppState) {
    let now = now_ms();
    for s in state.db.expired_secrets().unwrap_or_default() {
        let were_on = state
            .db
            .disable_accesses_of_target(&s.user_id, "secret", &s.id)
            .unwrap_or_default();
        if were_on.is_empty() {
            continue;
        }
        audit(
            state,
            &s.user_id,
            AuditEntry {
                detail: json!({ "agents": were_on.len() }),
                ..entry("expired", &s, "system")
            },
        );
        for mut a in were_on {
            a.enabled = false;
            after_access_change(state, &s, &a);
        }
        push_user(
            state,
            &s.user_id,
            PushEventDto::SecretExpiring(SecretExpiringPayloadDto {
                secret_id: s.id.clone(),
                name: s.env.clone(),
                expires_at: s.expires_at,
                expired: true,
            }),
        );
    }
    // Three-day notice, once per secret (the audit row is the marker).
    for (user_id, _, _) in state.db.secrets_counts().unwrap_or_default() {
        for s in state.db.list_secrets(&user_id).unwrap_or_default() {
            let Some(t) = s.expires_at else { continue };
            if t < now || t - now > EXPIRING_NOTICE_MS {
                continue;
            }
            let q = AuditQuery {
                target_id: Some(s.id.clone()),
                limit: 50,
                ..Default::default()
            };
            let already = state
                .db
                .list_audit(&user_id, &q)
                .unwrap_or_default()
                .iter()
                .any(|a| {
                    a.action == "expiring"
                        && a.detail.get("expires_at").and_then(Value::as_i64) == Some(t)
                });
            if already {
                continue;
            }
            audit(
                state,
                &user_id,
                AuditEntry {
                    detail: json!({ "expires_at": t }),
                    ..entry("expiring", &s, "system")
                },
            );
            push_user(
                state,
                &user_id,
                PushEventDto::SecretExpiring(SecretExpiringPayloadDto {
                    secret_id: s.id.clone(),
                    name: s.env.clone(),
                    expires_at: Some(t),
                    expired: false,
                }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Chat, Project};
    use crate::db_secrets::Vault;
    use crate::people::tests_support::person;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;
    use arbos_core::api::{
        DecideSecretResponseDto, ProjectSecretsDto, SecretAccessEnvelopeDto, SecretsOverviewDto,
    };

    const FIXED_AT: i64 = 1_700_000_000_000;

    fn fixtures_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/api")
    }

    fn load_fixture(name: &str) -> Value {
        let path = fixtures_dir().join(name);
        let raw =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        serde_json::from_str(&raw).expect("fixture json")
    }

    fn assert_builder(name: &str, got: Value) {
        let path = fixtures_dir().join(name);
        if !path.exists() || std::env::var_os("UPDATE_API_FIXTURES").is_some() {
            let pretty = serde_json::to_string_pretty(&got).expect("pretty");
            std::fs::write(&path, format!("{pretty}\n")).expect("write fixture");
        }
        let want = load_fixture(name);
        assert_eq!(got, want, "fixture {name}");
    }

    fn insert_project(state: &AppState, owner: &str, id: &str, name: &str) {
        state
            .db
            .insert_project(&Project {
                id: id.into(),
                name: name.into(),
                icon: "R".into(),
                instructions: String::new(),
                models: json!({}),
                state: "hibernated_light".into(),
                created_at: FIXED_AT,
                last_active_at: FIXED_AT,
                spend_cap_daily: 25.0,
                color: "#000".into(),
                user_id: owner.to_owned(),
                workspace_state: "stopped".into(),
                avatar: None,
                avatar_color: None,
                description: String::new(),
                primary_chat_id: None,
                avatar_color_index: None,
                avatar_leaf: None,
                avatar_leaf_color: None,
                image_key: None,
                avatar_version: 0,
            })
            .unwrap();
        state
            .db
            .insert_chat(&Chat {
                id: format!("chat_{id}"),
                project_id: id.into(),
                title: name.into(),
                is_pinned: true,
                position: 0,
                created_at: FIXED_AT,
                doc_epoch: 1,
                kind: "chat".into(),
                participants: Vec::new(),
                forked_from: None,
            })
            .unwrap();
    }

    fn insert_key_secret(
        state: &AppState,
        user_id: &str,
        id: &str,
        name: &str,
        env: &str,
    ) -> Secret {
        let dek = user_dek(state, user_id).unwrap();
        let fields = json!({ "value": "fixture-not-a-real-key" });
        let s = Secret {
            id: id.into(),
            user_id: user_id.into(),
            name: name.into(),
            env: env.into(),
            kind: "key".into(),
            note: "Fixture key for agent egress tests.".into(),
            hosts: vec!["api.fixture.example".into()],
            ask_every_time: false,
            expires_at: None,
            current_version: 1,
            created_at: FIXED_AT,
            updated_at: FIXED_AT,
            last_used_at: Some(FIXED_AT + 1_000),
            daytona_secrets: json!({}),
            inject_env: false,
            others_ask: "refuse".into(),
            source_kind: "settings".into(),
            source_project_id: None,
            source_chat_id: None,
        };
        let v1 = SecretVersion {
            secret_id: s.id.clone(),
            version: 1,
            fields_enc: encrypt_fields(&dek, &s.id, 1, &fields).unwrap(),
            created_at: FIXED_AT,
            retire_at: None,
        };
        state.db.insert_secret(&s, &v1).unwrap();
        s
    }

    fn insert_login_secret(state: &AppState, user_id: &str) -> Secret {
        let dek = user_dek(state, user_id).unwrap();
        let fields = json!({
            "username": "fixture.login",
            "password": "fixture-not-a-real-password",
            "url": "https://login.fixture.example"
        });
        let s = Secret {
            id: "sec_login_fixture".into(),
            user_id: user_id.into(),
            name: "Fixture Login".into(),
            env: "FIXTURE_LOGIN".into(),
            kind: "login".into(),
            note: "Login metadata only; password stays encrypted.".into(),
            hosts: vec!["login.fixture.example".into()],
            ask_every_time: false,
            expires_at: None,
            current_version: 1,
            created_at: FIXED_AT,
            updated_at: FIXED_AT,
            last_used_at: None,
            daytona_secrets: json!({}),
            inject_env: false,
            others_ask: "refuse".into(),
            source_kind: "settings".into(),
            source_project_id: None,
            source_chat_id: None,
        };
        let v1 = SecretVersion {
            secret_id: s.id.clone(),
            version: 1,
            fields_enc: encrypt_fields(&dek, &s.id, 1, &fields).unwrap(),
            created_at: FIXED_AT,
            retire_at: None,
        };
        state.db.insert_secret(&s, &v1).unwrap();
        s
    }

    #[test]
    fn secrets_wire_fixtures() {
        let state = test_state(AuthMode::Github);
        let (owner, _) = person(&state, 501);
        let (director_id, _) = person(&state, 502);
        insert_project(&state, &owner, "proj_secrets_fixture", "Research");
        let secret = insert_key_secret(
            &state,
            &owner,
            "sec_fixture",
            "Fixture Stripe",
            "FIXTURE_STRIPE_KEY",
        );
        let login = insert_login_secret(&state, &owner);
        state
            .db
            .upsert_access(&Access {
                id: "acc_secret_fixture".into(),
                user_id: owner.clone(),
                project_id: "proj_secrets_fixture".into(),
                target_kind: "secret".into(),
                target_id: secret.id.clone(),
                note: "May charge test fixtures only.".into(),
                hosts: vec!["api.fixture.example".into()],
                scope_kind: "all".into(),
                scope_chat_id: None,
                enabled: true,
                created_from: "settings".into(),
                created_at: FIXED_AT,
                disabled_at: None,
                last_used_at: Some(FIXED_AT + 2_000),
            })
            .unwrap();
        state
            .db
            .upsert_secret_director(&SecretDirector {
                last_asked_at: Some(FIXED_AT + 3_000),
                ..SecretDirector::new(&secret.id, &director_id, "Only fixture spend.", FIXED_AT)
            })
            .unwrap();

        let request = SecretRequest {
            id: "req_secret_fixture".into(),
            user_id: owner.clone(),
            project_id: "proj_secrets_fixture".into(),
            chat_id: "chat_proj_secrets_fixture".into(),
            target_kind: "secret".into(),
            target_id: Some(secret.id.clone()),
            target_ref: secret.env.clone(),
            purpose: "Call the fixture API once.".into(),
            use_kind: "header".into(),
            host: Some("api.fixture.example".into()),
            status: "pending".into(),
            decided_by: None,
            created_at: FIXED_AT,
            decided_at: None,
            expires_at: FIXED_AT + REQUEST_PENDING_MS,
            asked_by: None,
        };
        state.db.insert_secret_request(&request).unwrap();

        let vault = Vault {
            id: "vault_fixture".into(),
            user_id: owner.clone(),
            provider: "onepassword".into(),
            name: "Fixture 1Password".into(),
            token_enc: None,
            external_vaults: json!([{ "id": "ev_fixture", "name": "Fixture Vault" }]),
            items: json!([{
                "ref": "op://Fixture/Login/password",
                "id": "item_fixture",
                "title": "Fixture Login Item",
                "category": "Login",
                "fields": ["username", "password"],
                "urls": ["https://login.fixture.example"]
            }]),
            items_count: 1,
            items_refreshed_at: Some(FIXED_AT),
            token_expires_at: Some(FIXED_AT + 86_400_000),
            status: "ok".into(),
            last_error: None,
            last_refreshed_at: Some(FIXED_AT),
            created_at: FIXED_AT,
            disconnected_at: None,
        };
        state.db.upsert_vault(&vault).unwrap();
        state
            .db
            .upsert_access(&Access {
                id: "acc_vault_fixture".into(),
                user_id: owner.clone(),
                project_id: "proj_secrets_fixture".into(),
                target_kind: "vault".into(),
                target_id: vault.id.clone(),
                note: "Everything in the fixture vault.".into(),
                hosts: vec![],
                scope_kind: "all".into(),
                scope_chat_id: None,
                enabled: true,
                created_from: "settings".into(),
                created_at: FIXED_AT,
                disabled_at: None,
                last_used_at: None,
            })
            .unwrap();

        let reason = SecretAudit {
            id: 1,
            user_id: owner.clone(),
            at: FIXED_AT,
            action: "refused".into(),
            target_kind: "secret".into(),
            target_id: secret.id.clone(),
            target_name: secret.env.clone(),
            project_id: Some("proj_secrets_fixture".into()),
            chat_id: Some("chat_proj_secrets_fixture".into()),
            request_id: None,
            access_id: Some("acc_secret_fixture".into()),
            use_kind: Some("header".into()),
            host: Some("evil.fixture.example".into()),
            by: "agent:proj_secrets_fixture".into(),
            detail: json!({ "reason": "host_not_allowed" }),
        };
        let decision = SecretAudit {
            id: 2,
            user_id: owner.clone(),
            at: FIXED_AT + 60_000,
            action: "decided".into(),
            target_kind: "secret".into(),
            target_id: secret.id.clone(),
            target_name: secret.env.clone(),
            project_id: Some("proj_secrets_fixture".into()),
            chat_id: Some("chat_proj_secrets_fixture".into()),
            request_id: Some(request.id.clone()),
            access_id: None,
            use_kind: Some("header".into()),
            host: Some("api.fixture.example".into()),
            by: format!("card:{}", request.id),
            detail: json!({ "decision": "allowed_always" }),
        };

        assert_builder("secret_json.json", secret_json(&state, &secret));
        assert_builder("secret_login_json.json", secret_json(&state, &login));
        assert_builder("secret_request_json.json", request_json(&state, &request));
        assert_builder(
            "secret_activity_reason.json",
            to_json(activity_dto(&reason, Some("Research".into()))),
        );
        assert_builder(
            "secret_activity_decision.json",
            to_json(activity_dto(&decision, Some("Research".into()))),
        );
        assert_builder(
            "secret_access_json.json",
            access_json(
                &state,
                &state
                    .db
                    .access("proj_secrets_fixture", "secret", &secret.id)
                    .unwrap()
                    .unwrap(),
            ),
        );
        assert_builder("vault_json.json", crate::vaults::vault_json(&state, &vault));

        let overview = SecretsOverviewDto {
            paused: false,
            paused_at: None,
            paused_by: None,
            vaults: vec![crate::vaults::vault_dto(&state, &vault)],
            providers: crate::vaults::providers(&state)
                .into_iter()
                .map(str::to_owned)
                .collect(),
            features: crate::features::dto(),
            secrets: vec![secret_dto(&state, &secret), secret_dto(&state, &login)],
            requests_pending: 1,
            requests: vec![request_dto(&state, &request)],
            activity: vec![
                activity_dto(&reason, Some("Research".into())),
                activity_dto(&decision, Some("Research".into())),
            ],
        };
        assert_builder("secrets_overview.json", to_json(overview));

        let mut items = vec![project_secret_item(
            &state,
            &state
                .db
                .access("proj_secrets_fixture", "secret", &secret.id)
                .unwrap()
                .unwrap(),
            &secret,
        )];
        items.push(
            crate::vaults::access_item_dto(
                &state,
                &owner,
                &state
                    .db
                    .access("proj_secrets_fixture", "vault", &vault.id)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap(),
        );
        assert_builder(
            "project_secrets_json.json",
            to_json(ProjectSecretsDto {
                paused: false,
                owner: true,
                items,
                requests: vec![request_dto(&state, &request)],
                activity: vec![
                    activity_dto(&reason, Some("Research".into())),
                    activity_dto(&decision, Some("Research".into())),
                ],
            }),
        );

        let decided = SecretRequest {
            status: "allowed_always".into(),
            decided_by: Some(format!("card:{}", request.id)),
            decided_at: Some(FIXED_AT + 60_000),
            ..request.clone()
        };
        assert_builder(
            "decide_secret_response.json",
            to_json(DecideSecretResponseDto {
                request: request_dto(&state, &decided),
                access: Some(access_dto(
                    &state,
                    &state
                        .db
                        .access("proj_secrets_fixture", "secret", &secret.id)
                        .unwrap()
                        .unwrap(),
                )),
            }),
        );
        assert_builder(
            "secrets_paused.json",
            to_json(paused_dto(&SecretsSettings {
                paused: true,
                paused_at: Some(FIXED_AT),
                paused_by: Some("user".into()),
            })),
        );
        let _ = SecretAccessEnvelopeDto {
            access: access_dto(
                &state,
                &state
                    .db
                    .access("proj_secrets_fixture", "secret", &secret.id)
                    .unwrap()
                    .unwrap(),
            ),
        };
        let _ = director_id;
    }

    #[test]
    fn env_vars_follow_the_placeholder_names() {
        let key: BTreeMap<String, String> = [("value".to_owned(), "sk_live_1".to_owned())].into();
        assert_eq!(
            env_vars("STRIPE_KEY", &key),
            [("STRIPE_KEY".to_owned(), "sk_live_1".to_owned())].into()
        );
        let login: BTreeMap<String, String> = [
            ("url".to_owned(), "https://github.com/login".to_owned()),
            ("username".to_owned(), "unarbos".to_owned()),
            ("password".to_owned(), "hunter22".to_owned()),
        ]
        .into();
        let vars = env_vars("GITHUB_LOGIN", &login);
        assert_eq!(
            vars.get("GITHUB_LOGIN_USERNAME").map(String::as_str),
            Some("unarbos")
        );
        assert_eq!(
            vars.get("GITHUB_LOGIN_PASSWORD").map(String::as_str),
            Some("hunter22")
        );
        assert!(!vars.values().any(|v| v.contains("github.com")));
    }

    #[test]
    fn env_names_follow_the_spec() {
        assert_eq!(env_from_name("STRIPE_KEY"), "STRIPE_KEY");
        assert_eq!(env_from_name("stripe live"), "STRIPE_LIVE");
        assert_eq!(env_from_name("GitHub login"), "GITHUB_LOGIN");
        assert_eq!(env_from_name("demo-api.key"), "DEMO_API_KEY");
        assert_eq!(env_from_name("1password"), "_1PASSWORD");
        assert_eq!(env_from_name("  __x__ "), "X");
        assert!(validate_name("").is_err());
        assert!(validate_name("has/slash").is_err());
        assert!(validate_name("ok name").is_ok());
    }

    #[test]
    fn hosts_are_cleaned_and_checked() {
        let h = clean_hosts(&[
            "https://api.stripe.com/v1".into(),
            "API.STRIPE.COM".into(),
            "*.acme.internal:8443".into(),
            " ".into(),
        ])
        .unwrap();
        assert_eq!(h, vec!["api.stripe.com", "*.acme.internal"]);
        assert!(clean_hosts(&["bad host".into()]).is_err());
        assert!(clean_hosts(&["evil.example/../x".into()]).is_ok());
    }

    #[test]
    fn refusal_strings_never_carry_values() {
        let allowed = vec!["api.stripe.com".to_owned()];
        assert_eq!(
            Refusal::HostNotAllowed(allowed).message("STRIPE_KEY", "evil.example"),
            "STRIPE_KEY: not allowed at evil.example (allowed: api.stripe.com)."
        );
        assert_eq!(
            Refusal::TurnedOff.message("STRIPE_KEY", ""),
            "STRIPE_KEY: access was turned off by the owner."
        );
        assert_eq!(
            Refusal::Paused.message("x", "y"),
            "Secrets are paused by the owner."
        );
        assert!(Refusal::NotDirector
            .message("LIUM_ALI", "")
            .contains("not on this secret's People list"));
        assert!(Refusal::AskOwnerFirst
            .message("LIUM_ALI", "")
            .contains("secrets_request first"));
        assert_eq!(Refusal::NotDirector.kind(), "not_allowed_person");
        assert_eq!(status_for("always"), Some("allowed_always"));
        assert_eq!(status_for("nope"), None);
    }

    /// #1664: the pause set by `POST /machine/stop` is lifted by `POST /machine/start`; a pause
    /// the user set stays.
    #[tokio::test]
    async fn start_lifts_the_stop_machine_pause_but_not_the_users() {
        let state = test_state(AuthMode::Github);
        let (owner, _) = person(&state, 701);
        set_paused(&state, &owner, true, "stop_machine").await;
        assert!(state.db.secrets_settings(&owner).unwrap().paused);
        assert!(resume_after_stop(&state, &owner).await);
        let after = state.db.secrets_settings(&owner).unwrap();
        assert!(!after.paused, "{after:?}");
        assert!(!resume_after_stop(&state, &owner).await, "nothing to lift");
        set_paused(&state, &owner, true, "user").await;
        assert!(
            !resume_after_stop(&state, &owner).await,
            "a user pause stays"
        );
        assert!(state.db.secrets_settings(&owner).unwrap().paused);
    }

    /// Owner, agent with standing Access, and three people: Sam (row on), Samir (row off),
    /// Jules (no row). Returns (state, owner, secret, sam, samir, jules).
    fn people_setup() -> (AppState, String, Secret, String, String, String) {
        let state = test_state(AuthMode::Github);
        let (owner, _) = person(&state, 601);
        let (sam, _) = person(&state, 602);
        let (samir, _) = person(&state, 603);
        let (jules, _) = person(&state, 604);
        insert_project(&state, &owner, "proj_people", "GigChadAgent");
        let secret = insert_key_secret(&state, &owner, "sec_people", "Engy", "ENGY_API_KEY");
        state
            .db
            .upsert_access(&Access {
                id: "acc_people".into(),
                user_id: owner.clone(),
                project_id: "proj_people".into(),
                target_kind: "secret".into(),
                target_id: secret.id.clone(),
                note: String::new(),
                hosts: vec!["api.engy.com".into()],
                scope_kind: "all".into(),
                scope_chat_id: None,
                enabled: true,
                created_from: "settings".into(),
                created_at: FIXED_AT,
                disabled_at: None,
                last_used_at: None,
            })
            .unwrap();
        state
            .db
            .upsert_secret_director(&SecretDirector::new(
                &secret.id,
                &sam,
                "reports only",
                FIXED_AT,
            ))
            .unwrap();
        state
            .db
            .upsert_secret_director(&SecretDirector {
                enabled: false,
                disabled_at: Some(FIXED_AT + 1),
                ..SecretDirector::new(&secret.id, &samir, "", FIXED_AT)
            })
            .unwrap();
        (state, owner, secret, sam, samir, jules)
    }

    fn verdict(state: &AppState, s: &Secret, asked_by: Option<&str>) -> Result<(), String> {
        decide_use(
            state,
            s,
            "proj_people",
            "api.engy.com",
            "header",
            Some("chat_arbonites"),
            asked_by,
        )
        .map(|_| ())
        .map_err(|r| r.kind().to_owned())
    }

    #[test]
    fn people_list_decides_who_may_ask() {
        let (state, owner, secret, sam, samir, jules) = people_setup();
        // Refuse (default): owner and Sam pass; Samir (row off) and Jules (no row) are refused flat.
        assert!(verdict(&state, &secret, None).is_ok());
        assert!(verdict(&state, &secret, Some(&owner)).is_ok());
        assert!(verdict(&state, &secret, Some(&sam)).is_ok());
        assert_eq!(
            verdict(&state, &secret, Some(&samir)),
            Err("not_allowed_person".into())
        );
        assert_eq!(
            verdict(&state, &secret, Some(&jules)),
            Err("not_allowed_person".into())
        );
        assert!(matches!(not_listed(&secret), Refusal::NotDirector));

        // Ask me: the same people are refused, but the agent is sent to ask first.
        let mut card = secret.clone();
        card.others_ask = "card".into();
        state.db.update_secret(&card).unwrap();
        assert!(verdict(&state, &card, Some(&sam)).is_ok());
        assert!(matches!(not_listed(&card), Refusal::AskOwnerFirst));
        assert!(verdict(&state, &card, Some(&jules)).is_err());

        // A chat-scoped row covers only its chat.
        state
            .db
            .upsert_secret_director(&SecretDirector {
                scope_kind: "chat".into(),
                scope_chat_id: Some("chat_other".into()),
                ..SecretDirector::new(&secret.id, &jules, "", FIXED_AT)
            })
            .unwrap();
        assert!(verdict(&state, &card, Some(&jules)).is_err());
        assert!(state
            .db
            .is_secret_director(&secret.id, &jules, Some("chat_other"))
            .unwrap());

        // The box hears only rows that are on, and the switch.
        let items = box_items(&state, &owner, "proj_people");
        let item = items
            .iter()
            .find(|v| v["env"] == "ENGY_API_KEY")
            .expect("item");
        assert_eq!(item["others_ask"], "card");
        let listed: Vec<&str> = item["people"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["user_id"].as_str())
            .collect();
        assert!(listed.contains(&sam.as_str()));
        assert!(listed.contains(&jules.as_str()));
        assert!(!listed.contains(&samir.as_str()));
    }

    #[test]
    fn a_used_row_names_who_asked() {
        let (state, owner, secret, sam, _samir, _jules) = people_setup();
        let allowed = decide_use(
            &state,
            &secret,
            "proj_people",
            "api.engy.com",
            "header",
            Some("chat_arbonites"),
            Some(&sam),
        )
        .unwrap();
        after_use(
            &state,
            &secret,
            "proj_people",
            "api.engy.com",
            "header",
            Some("chat_arbonites"),
            Some(&sam),
            &allowed,
        );
        let rows = state
            .db
            .list_audit(
                &owner,
                &crate::db_secrets::AuditQuery {
                    target_id: Some(secret.id.clone()),
                    project_id: None,
                    before: None,
                    since: None,
                    limit: 10,
                },
            )
            .unwrap();
        let used = rows.iter().find(|a| a.action == "used").expect("used row");
        assert_eq!(used.detail["asked_by"], sam);
        assert_eq!(used.detail["asked_by_handle"], "user602");
        let row = state.db.secret_director(&secret.id, &sam).unwrap().unwrap();
        assert!(row.last_asked_at.is_some());
    }

    #[tokio::test]
    async fn always_allow_on_a_persons_card_adds_them_to_the_list() {
        let (state, owner, secret, _sam, _samir, jules) = people_setup();
        let rq = SecretRequest {
            id: "req_jules".into(),
            user_id: owner.clone(),
            project_id: "proj_people".into(),
            chat_id: "chat_arbonites".into(),
            target_kind: "secret".into(),
            target_id: Some(secret.id.clone()),
            target_ref: secret.env.clone(),
            purpose: "rent a GPU box".into(),
            use_kind: "header".into(),
            host: Some("api.engy.com".into()),
            status: "pending".into(),
            decided_by: None,
            created_at: FIXED_AT,
            decided_at: None,
            expires_at: now_ms() + REQUEST_PENDING_MS,
            asked_by: Some(jules.clone()),
        };
        state.db.insert_secret_request(&rq).unwrap();
        let dto = request_dto(&state, &rq);
        assert_eq!(dto.asked_by_handle.as_deref(), Some("user604"));

        // Allow for this chat → a chat-scoped row.
        let (decided, _) = decide(&state, &owner, "req_jules", "chat", "card:req_jules")
            .await
            .unwrap();
        assert_eq!(decided.status, "allowed_chat");
        let row = state
            .db
            .secret_director(&secret.id, &jules)
            .unwrap()
            .unwrap();
        assert!(row.enabled);
        assert_eq!(row.scope_kind, "chat");
        assert_eq!(row.scope_chat_id.as_deref(), Some("chat_arbonites"));
        assert_eq!(row.created_from, "card:req_jules");
        assert!(verdict(&state, &secret, Some(&jules)).is_ok());

        // Always allow → standing.
        let mut rq2 = rq.clone();
        rq2.id = "req_jules_2".into();
        state.db.insert_secret_request(&rq2).unwrap();
        decide(&state, &owner, "req_jules_2", "always", "card:req_jules_2")
            .await
            .unwrap();
        let row = state
            .db
            .secret_director(&secret.id, &jules)
            .unwrap()
            .unwrap();
        assert_eq!(row.scope_kind, "all");
    }

    #[tokio::test]
    async fn a_people_change_reaches_every_agent_with_access() {
        let (state, owner, secret, _sam, samir, jules) = people_setup();
        let _ = state.db.take_pushes("proj_people");
        // Settings: flip Samir's row on.
        let (handle, _) = user_handle_name(&state, &samir);
        upsert_person(
            &state,
            &owner,
            &secret,
            &samir,
            DirectorBody {
                enabled: Some(true),
                ..Default::default()
            },
            Some(&handle),
        )
        .unwrap();
        let pushes = state.db.take_pushes("proj_people").unwrap();
        assert_eq!(pushes.len(), 1, "{pushes:?}");
        assert_eq!(pushes[0].1, "secret_access_changed");
        assert_eq!(pushes[0].2["env"], "ENGY_API_KEY");
        assert_eq!(pushes[0].2["people"], true);
        // A card answer that adds a person pushes too.
        state
            .db
            .insert_secret_request(&SecretRequest {
                id: "req_push".into(),
                user_id: owner.clone(),
                project_id: "proj_people".into(),
                chat_id: "chat_arbonites".into(),
                target_kind: "secret".into(),
                target_id: Some(secret.id.clone()),
                target_ref: secret.env.clone(),
                purpose: "x".into(),
                use_kind: "header".into(),
                host: Some("api.engy.com".into()),
                status: "pending".into(),
                decided_by: None,
                created_at: FIXED_AT,
                decided_at: None,
                expires_at: now_ms() + REQUEST_PENDING_MS,
                asked_by: Some(jules.clone()),
            })
            .unwrap();
        decide(&state, &owner, "req_push", "always", "card:req_push")
            .await
            .unwrap();
        let kinds: Vec<String> = state
            .db
            .take_pushes("proj_people")
            .unwrap()
            .into_iter()
            .map(|(_, k, _)| k)
            .collect();
        assert!(
            kinds.iter().any(|k| k == "secret_access_changed"),
            "{kinds:?}"
        );
    }

    #[test]
    fn a_once_grant_is_bound_to_its_asker() {
        let (state, owner, secret, sam, _samir, jules) = people_setup();
        let mut card = secret.clone();
        card.others_ask = "card".into();
        state.db.update_secret(&card).unwrap();
        state
            .db
            .insert_secret_request(&SecretRequest {
                id: "req_once".into(),
                user_id: owner.clone(),
                project_id: "proj_people".into(),
                chat_id: "chat_arbonites".into(),
                target_kind: "secret".into(),
                target_id: Some(card.id.clone()),
                target_ref: card.env.clone(),
                purpose: "once".into(),
                use_kind: "header".into(),
                host: Some("api.engy.com".into()),
                status: "allowed_once".into(),
                decided_by: Some("card:req_once".into()),
                created_at: now_ms(),
                decided_at: Some(now_ms()),
                expires_at: now_ms() + crate::db_secrets::ALLOW_ONCE_MS,
                asked_by: Some(jules.clone()),
            })
            .unwrap();
        // Sam's ask does not spend Jules's once-grant (Sam passes on her own row anyway);
        // Jules's does.
        let sam_ok = verdict(&state, &card, Some(&sam));
        assert!(sam_ok.is_ok());
        let jules_ok = decide_use(
            &state,
            &card,
            "proj_people",
            "api.engy.com",
            "header",
            Some("chat_arbonites"),
            Some(&jules),
        )
        .unwrap();
        assert!(jules_ok.once.is_some());
        let owner_use = decide_use(
            &state,
            &card,
            "proj_people",
            "api.engy.com",
            "header",
            Some("chat_arbonites"),
            None,
        )
        .unwrap();
        assert!(owner_use.once.is_none());
    }

    // ---- secret hand-off: "save this" (POST /box/secrets/from-message) -------------------------

    const HANDOFF_KEY: &str = concat!(
        "sk_",
        "live_51HxQ9mT3vK2pLnR8wZ4eYc7bD1fG6hJ0kM9nP5qS2tU8vW3xA1"
    );

    fn holds(bytes: &[u8], needle: &str) -> bool {
        let n = needle.as_bytes();
        bytes.windows(n.len()).any(|w| w == n)
    }

    /// Every byte the relay keeps for a room: the checkpoint and every row.
    fn room_bytes(state: &AppState, room: &str) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some((_, cp)) = state.db.checkpoint_bytes(room).unwrap() {
            out.extend(cp);
        }
        for r in state.db.rows_after(room, 0, None, 10_000).unwrap() {
            out.extend(r.bytes);
        }
        out
    }

    /// Owner + project + a room with two user messages; the first holds the key. Returns
    /// `(state, owner, project_id, chat_id, room_name)`.
    async fn handoff_setup(text: &str) -> (AppState, String, String, String, String) {
        let state = test_state(AuthMode::Github);
        let (owner, _) = person(&state, 701);
        insert_project(&state, &owner, "proj_handoff", "Arbos");
        let chat_id = "chat_handoff".to_owned();
        let room = arbos_core::wire::room_name("proj_handoff", &chat_id);
        let sender = format!("user:{owner}");
        let text = text.to_owned();
        crate::notices::write_doc(&state, Some("proj_handoff"), &chat_id, move |doc| {
            let opts = arbos_core::doc::UserMessageOpts {
                sender: Some(sender),
                ..Default::default()
            };
            doc.append_user_message("m_key", &text, "dev1", &opts)?;
            doc.append_user_message("m_plain", "and run the refunds report", "dev1", &opts)?;
            Ok(())
        })
        .await
        .unwrap();
        (state, owner, "proj_handoff".into(), chat_id, room)
    }

    fn box_headers(project: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-arbos-project", project.parse().unwrap());
        h
    }

    async fn from_message(
        state: &AppState,
        owner: &str,
        project: &str,
        body: FromMessageBody,
    ) -> Result<(StatusCode, Value), ApiErr> {
        let res = box_from_message(
            State(state.clone()),
            Principal::Box {
                user_id: owner.to_owned(),
            },
            box_headers(project),
            Json(body),
        )
        .await?;
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        Ok((
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        ))
    }

    /// `ARBOS_HANDOFF_KEEP=<dir>` (scripts/secrets/handoff-invariant-check.sh): keep the relay's
    /// data dir and a TRACE journal of the run there, for the grep that follows.
    fn keep_dir() -> Option<std::path::PathBuf> {
        std::env::var_os("ARBOS_HANDOFF_KEEP").map(std::path::PathBuf::from)
    }

    fn journal_guard(
        keep: &Option<std::path::PathBuf>,
    ) -> Option<tracing::subscriber::DefaultGuard> {
        let dir = keep.as_ref()?;
        std::fs::create_dir_all(dir).ok()?;
        let file = std::fs::File::create(dir.join("relay-journal.log")).ok()?;
        let sub = tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(file))
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .finish();
        Some(tracing::subscriber::set_default(sub))
    }

    fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
        let _ = std::fs::create_dir_all(to);
        if let Ok(rd) = std::fs::read_dir(from) {
            for e in rd.flatten() {
                let p = e.path();
                let dest = to.join(e.file_name());
                if p.is_dir() {
                    copy_tree(&p, &dest);
                } else {
                    let _ = std::fs::copy(&p, &dest);
                }
            }
        }
    }

    #[tokio::test]
    async fn save_this_moves_the_value_into_the_vault_and_out_of_every_relay_byte() {
        let keep = keep_dir();
        let _journal = journal_guard(&keep);
        let text = format!("here is the stripe key {HANDOFF_KEY} save this");
        let (state, owner, project, chat, room) = handoff_setup(&text).await;
        tracing::debug!(chat = %chat, "handoff check: message with the key written");
        assert!(
            holds(&room_bytes(&state, &room), HANDOFF_KEY),
            "setup: the row holds the key"
        );
        let _ = state.db.take_pushes(&project);

        let (status, body) = from_message(
            &state,
            &owner,
            &project,
            FromMessageBody {
                chat_id: chat.clone(),
                message_id: "m_key".into(),
                purpose: Some("the refunds report".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["secret"]["name"], "STRIPE_KEY");
        assert_eq!(body["secret"]["env"], "STRIPE_KEY");
        assert_eq!(body["secret"]["source"]["kind"], "chat");
        assert_eq!(body["secret"]["source"]["project_id"], project);
        assert_eq!(body["secret"]["source"]["project_name"], "Arbos");
        assert_eq!(body["redacted"], true);
        assert!(
            !body.to_string().contains(HANDOFF_KEY),
            "the answer never carries the value"
        );

        // the vault has it, encrypted, and it decrypts to the exact token
        let id = body["secret"]["id"].as_str().unwrap();
        let s = state.db.secret(&owner, id).unwrap().unwrap();
        let dek = user_dek(&state, &owner).unwrap();
        let plain = plain_of(&state, &dek, &s).unwrap();
        assert_eq!(plain.current["value"], HANDOFF_KEY);
        assert_eq!(s.note, "the refunds report");
        assert_eq!(s.source_chat_id.as_deref(), Some(chat.as_str()));
        let access = state.db.access(&project, "secret", id).unwrap().unwrap();
        assert!(access.enabled);
        assert_eq!(access.created_from, "chat");
        assert_eq!(access.note, "the refunds report");

        // the row is the ghost, the other message untouched
        let doc = crate::matrix::backfill::load_doc(&state, &chat, Some(&project))
            .unwrap()
            .unwrap();
        let m = doc.message("m_key").unwrap();
        assert_eq!(m.parts.len(), 1);
        assert_eq!(m.parts[0]["kind"], "secret_redacted");
        assert_eq!(m.parts[0]["name"], "STRIPE_KEY");
        assert_eq!(m.parts[0]["savedBy"], format!("user:{owner}"));
        assert_eq!(m.parts[0]["agent"], project);
        assert_eq!(m.text(), "");
        assert_eq!(
            doc.message("m_plain").unwrap().text(),
            "and run the refunds report"
        );

        // and no byte the relay stores for the room holds the key any more
        let st = state.db.room_state(&room).unwrap();
        assert_eq!(st.row_count, 0, "rows folded into the shallow checkpoint");
        assert!(st.checkpoint_seq >= 1);
        assert!(!holds(&room_bytes(&state, &room), HANDOFF_KEY));
        // …nor the database files themselves (main file + WAL), the way the e2e check greps them
        for name in ["relay.db", "relay.db-wal"] {
            let p = state.cfg.data_dir.join(name);
            if let Ok(bytes) = std::fs::read(&p) {
                assert!(
                    !holds(&bytes, HANDOFF_KEY),
                    "{} still holds the key",
                    p.display()
                );
            }
        }

        // audit: created + scrubbed, no value in detail
        let acts = state
            .db
            .list_audit(
                &owner,
                &AuditQuery {
                    target_id: Some(id.to_owned()),
                    limit: 50,
                    ..Default::default()
                },
            )
            .unwrap();
        let kinds: Vec<&str> = acts.iter().map(|a| a.action.as_str()).collect();
        assert!(
            kinds.contains(&"created")
                && kinds.contains(&"scrubbed")
                && kinds.contains(&"access_on"),
            "{kinds:?}"
        );
        for a in &acts {
            assert!(!a.detail.to_string().contains(HANDOFF_KEY));
        }

        // pushes: the box scrubs, the devices drop their copies
        let pushes = state.db.take_pushes(&project).unwrap();
        let kinds: Vec<&str> = pushes.iter().map(|p| p.1.as_str()).collect();
        assert!(kinds.contains(&"secret_access_changed"), "{kinds:?}");
        assert!(kinds.contains(&"secret_scrub"), "{kinds:?}");
        for p in &pushes {
            assert!(!p.2.to_string().contains(HANDOFF_KEY));
        }
        let user_pushes: Vec<String> = state
            .db
            .with(|c| {
                let mut st =
                    c.prepare("SELECT kind FROM user_pushes WHERE delivered_at IS NULL")?;
                let v = st
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(v)
            })
            .unwrap();
        assert!(
            user_pushes.contains(&"message_redacted".to_owned()),
            "{user_pushes:?}"
        );
        if let Some(dir) = keep {
            let _ = state.db.flush_after_scrub();
            copy_tree(&state.cfg.data_dir, &dir.join("data"));
            let _ = std::fs::write(
                dir.join("README.txt"),
                "relay data dir after the save-this path (relay.db, -wal, blobs/…) + relay-journal.log (TRACE)\n",
            );
        }
    }

    #[tokio::test]
    async fn save_this_needs_a_key_a_hint_for_two_and_a_free_name() {
        // nothing key-like → 404 no_key_found
        let (state, owner, project, chat, _) = handoff_setup("just prose here").await;
        let e = from_message(
            &state,
            &owner,
            &project,
            FromMessageBody {
                chat_id: chat.clone(),
                message_id: "m_key".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!((e.0, e.1.as_str()), (StatusCode::NOT_FOUND, "no_key_found"));

        // two confident keys → ambiguous without a hint; the hint picks one
        let gh = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let (state, owner, project, chat, room) =
            handoff_setup(&format!("stripe {HANDOFF_KEY} and github {gh}")).await;
        let e = from_message(
            &state,
            &owner,
            &project,
            FromMessageBody {
                chat_id: chat.clone(),
                message_id: "m_key".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(e.1, "ambiguous");
        let hint = arbos_core::redact::key_hint(gh);
        let (status, body) = from_message(
            &state,
            &owner,
            &project,
            FromMessageBody {
                chat_id: chat.clone(),
                message_id: "m_key".into(),
                hint: Some(hint),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["secret"]["name"], "GITHUB_TOKEN");
        let bytes = room_bytes(&state, &room);
        assert!(
            !holds(&bytes, gh) && !holds(&bytes, HANDOFF_KEY),
            "the whole row went, both tokens with it"
        );

        // a taken name gets _2
        let (state, owner, project, chat, _) =
            handoff_setup(&format!("{HANDOFF_KEY} save this")).await;
        insert_key_secret(&state, &owner, "sec_taken", "STRIPE_KEY", "STRIPE_KEY");
        let (_, body) = from_message(
            &state,
            &owner,
            &project,
            FromMessageBody {
                chat_id: chat.clone(),
                message_id: "m_key".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(body["secret"]["name"], "STRIPE_KEY_2");

        // someone else's message never goes into the owner's vault; an agent row neither
        let (state, owner, project, chat, _) = handoff_setup(HANDOFF_KEY).await;
        let (guest, _) = person(&state, 702);
        let guest_sender = format!("user:{guest}");
        crate::notices::write_doc(&state, Some(&project), &chat, move |doc| {
            doc.append_user_message(
                "m_guest",
                HANDOFF_KEY,
                "dev2",
                &arbos_core::doc::UserMessageOpts {
                    sender: Some(guest_sender),
                    ..Default::default()
                },
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let e = from_message(
            &state,
            &owner,
            &project,
            FromMessageBody {
                chat_id: chat.clone(),
                message_id: "m_guest".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(e.1, "not_owner_message");
        let e = from_message(
            &state,
            &owner,
            &project,
            FromMessageBody {
                chat_id: chat.clone(),
                message_id: "nope".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(e.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_card_save_records_where_the_secret_came_from() {
        let state = test_state(AuthMode::Github);
        let (owner, principal) = person(&state, 703);
        insert_project(&state, &owner, "proj_card", "Arbos");
        let res = create(
            State(state.clone()),
            principal,
            Json(CreateBody {
                name: "STRIPE_KEY".into(),
                value: Some(HANDOFF_KEY.into()),
                note: Some("refunds report".into()),
                agents: vec![AgentPick {
                    project_id: "proj_card".into(),
                    note: Some("refunds report".into()),
                }],
                source: Some(SourceBody {
                    kind: "chat".into(),
                    project_id: Some("proj_card".into()),
                    chat_id: Some("chat_1".into()),
                }),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["secret"]["source"]["kind"], "chat");
        assert_eq!(body["secret"]["source"]["project_name"], "Arbos");
        assert_eq!(body["secret"]["source"]["chat_id"], "chat_1");
        assert!(!bytes
            .windows(HANDOFF_KEY.len())
            .any(|w| w == HANDOFF_KEY.as_bytes()));
        let id = body["secret"]["id"].as_str().unwrap();
        let access = state.db.access("proj_card", "secret", id).unwrap().unwrap();
        assert_eq!(access.created_from, "chat");
        // a settings save stays `settings`
        let (owner2, principal2) = person(&state, 704);
        let res = create(
            State(state.clone()),
            principal2,
            Json(CreateBody {
                name: "OTHER".into(),
                value: Some("v".repeat(20)),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["secret"]["source"]["kind"], "settings");
        assert!(state.db.secret_by_name(&owner2, "OTHER").unwrap().is_some());
    }
}
