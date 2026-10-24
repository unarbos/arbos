//! Sign in with GitHub (self-serve Arbos): OAuth authorization code + PKCE, short-lived JWT
//! access tokens, rotating refresh tokens, `/me`, and the download redirect.
//!
//! Flow for a Mac or iPhone app (ASWebAuthenticationSession-compatible):
//!   1. open `GET /auth/github/start?redirect=arbos://auth/callback&device_name=…`;
//!   2. GitHub sends the browser to `GET /auth/github/callback?code&state`; the relay signs
//!      the user in and redirects to `<redirect>?code=<one-time grant>`;
//!   3. the app posts the grant to `POST /auth/token` and gets `{access_token, refresh_token,
//!      expires_in, device_id, user}`; `POST /auth/refresh` rotates both later.
//!
//! The access token is an HS256 JWT `{sub: user_id, dev: device_id, jti, iat, exp}` and goes
//! wherever the old `adv_` device token went (`Authorization: Bearer`, `?token=` on WebSockets).
//! Provider config is env only: `ARBOS_GITHUB_CLIENT_ID` / `ARBOS_GITHUB_CLIENT_SECRET` (the
//! `ARBOS_GITHUB_OAUTH_CLIENT_*` spellings work too), `ARBOS_PUBLIC_URL`, `ARBOS_JWT_KEY` (optional).

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;

use arbos_core::api::{
    AccountUserDto, ApiErrorBodyDto, ApiErrorDto, CreditsDto, MeDeviceDto, MeLimitsDto,
    MeMachineDto, MePrivacyDto, MeResponseDto, MeUserResponseDto, PeopleInviteAcceptedPayloadDto,
    PushEventDto, TokenResponseDto,
};
use arbos_core::auth::hash;
use arbos_core::ids::{now_ms, ulid};

use crate::auth::Principal;
use crate::blobs::not_an_image;
use crate::db_accounts::{AuthFlow, GithubIdentity};
use crate::image_sniff::{check_avatar, ImageKind};
use crate::state::{AppState, AuthMode};

pub const ACCESS_TTL_S: i64 = 3600;
const REFRESH_TTL_MS: i64 = 30 * 24 * 3600 * 1000;
const FLOW_TTL_MS: i64 = 10 * 60 * 1000;
const GRANT_TTL_MS: i64 = 60 * 1000;
const REFRESH_PREFIX: &str = "arf_";
const DOWNLOAD_CACHE_MS: i64 = 5 * 60 * 1000;

type HmacSha256 = Hmac<Sha256>;

/// `{"error":{"type":…,"message":…}}` with a status; the shape every client branches on.
pub(crate) fn api_error(status: StatusCode, kind: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(ApiErrorDto {
            error: ApiErrorBodyDto {
                type_: kind.to_owned(),
                message: message.into(),
            },
        }),
    )
        .into_response()
}

/// 302 Found: the redirect browsers and ASWebAuthenticationSession expect from OAuth hops.
fn found(url: &str) -> Response {
    (StatusCode::FOUND, [(header::LOCATION, url.to_owned())]).into_response()
}

// ---- JWT (HS256) --------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    pub user_id: String,
    pub device_id: String,
    pub exp: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessError {
    Expired,
    Invalid,
}

fn sign(key: &[u8; 32], data: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac key");
    mac.update(data.as_bytes());
    B64URL.encode(mac.finalize().into_bytes())
}

pub fn mint_access(key: &[u8; 32], user_id: &str, device_id: &str, ttl_s: i64) -> String {
    let now = now_ms() / 1000;
    let header = B64URL.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = B64URL.encode(
        json!({ "sub": user_id, "dev": device_id, "jti": ulid(), "iat": now, "exp": now + ttl_s })
            .to_string(),
    );
    let signing_input = format!("{header}.{payload}");
    let sig = sign(key, &signing_input);
    format!("{signing_input}.{sig}")
}

/// Does this bearer look like one of our JWTs (three base64url parts)?
pub fn looks_like_jwt(token: &str) -> bool {
    token.starts_with("eyJ") && token.split('.').count() == 3
}

pub fn verify_access(key: &[u8; 32], token: &str) -> Result<Claims, AccessError> {
    let mut parts = token.split('.');
    let (Some(h), Some(p), Some(s), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(AccessError::Invalid);
    };
    let expected = sign(key, &format!("{h}.{p}"));
    let given = B64URL.decode(s).map_err(|_| AccessError::Invalid)?;
    let expected_bytes = B64URL.decode(&expected).map_err(|_| AccessError::Invalid)?;
    if given.len() != expected_bytes.len()
        || given
            .iter()
            .zip(expected_bytes.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            != 0
    {
        return Err(AccessError::Invalid);
    }
    let payload: Value =
        serde_json::from_slice(&B64URL.decode(p).map_err(|_| AccessError::Invalid)?)
            .map_err(|_| AccessError::Invalid)?;
    let exp = payload
        .get("exp")
        .and_then(Value::as_i64)
        .ok_or(AccessError::Invalid)?;
    let claims = Claims {
        user_id: payload
            .get("sub")
            .and_then(Value::as_str)
            .ok_or(AccessError::Invalid)?
            .to_owned(),
        device_id: payload
            .get("dev")
            .and_then(Value::as_str)
            .ok_or(AccessError::Invalid)?
            .to_owned(),
        exp,
    };
    if exp * 1000 <= now_ms() {
        return Err(AccessError::Expired);
    }
    Ok(claims)
}

// ---- token issue --------------------------------------------------------------------------------

fn user_dto(state: &AppState, user_id: &str) -> AccountUserDto {
    match state.db.user(user_id).ok().flatten() {
        Some(u) => AccountUserDto {
            id: u.id,
            github_id: Some(u.github_id),
            login: Some(u.login),
            avatar_url: Some(u.avatar_url),
            name: Some(u.name),
            created_at: Some(u.created_at),
            last_login_at: Some(u.last_login_at),
        },
        None => AccountUserDto {
            id: user_id.to_owned(),
            github_id: None,
            login: None,
            avatar_url: None,
            name: None,
            created_at: None,
            last_login_at: None,
        },
    }
}

#[allow(dead_code)] // kept for callers that still want Value
fn user_json(state: &AppState, user_id: &str) -> Value {
    serde_json::to_value(user_dto(state, user_id)).expect("AccountUserDto")
}

/// Access + refresh pair for a device. The refresh token is stored hashed, bound to the device.
pub(crate) fn issue_tokens(
    state: &AppState,
    user_id: &str,
    device_id: &str,
) -> anyhow::Result<TokenResponseDto> {
    let access = mint_access(&state.jwt_key, user_id, device_id, ACCESS_TTL_S);
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    let refresh = format!("{REFRESH_PREFIX}{}", hex::encode(raw));
    state
        .db
        .insert_refresh(&hash(&refresh), device_id, user_id, REFRESH_TTL_MS)?;
    let _ = state.db.touch_device(device_id);
    Ok(TokenResponseDto {
        access_token: access,
        token_type: "Bearer".into(),
        expires_in: ACCESS_TTL_S,
        refresh_token: refresh,
        device_id: device_id.to_owned(),
        user: user_dto(state, user_id),
    })
}

/// Wire JSON for a token pair (token + refresh grace cache).
pub(crate) fn token_response_json(dto: &TokenResponseDto) -> Value {
    serde_json::to_value(dto).expect("TokenResponseDto")
}

// ---- GitHub OAuth -------------------------------------------------------------------------------

fn github_mode(state: &AppState) -> Result<(String, String), Box<Response>> {
    if state.cfg.auth_mode != AuthMode::Github {
        return Err(Box::new(api_error(
            StatusCode::CONFLICT,
            "auth_mode",
            "this relay uses pairing codes (ARBOS_AUTH_MODE=paircode); GitHub sign-in is off",
        )));
    }
    match (&state.cfg.github_client_id, &state.cfg.github_client_secret) {
        (Some(id), Some(secret)) => Ok((id.clone(), secret.clone())),
        _ => Err(Box::new(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "oauth_unconfigured",
            "ARBOS_GITHUB_CLIENT_ID / ARBOS_GITHUB_CLIENT_SECRET are not set",
        ))),
    }
}

fn callback_url(state: &AppState) -> String {
    format!(
        "{}/auth/github/callback",
        state.cfg.public_url.trim_end_matches('/')
    )
}

/// Where the relay may send the browser after sign-in: the apps' custom scheme, a loopback
/// listener, the relay's own origin, or an operator-listed prefix.
pub(crate) fn redirect_allowed(state: &AppState, redirect: &str) -> bool {
    if redirect == "arbos://auth/callback" || redirect.starts_with("arbos://auth/callback?") {
        return true;
    }
    let loopback = ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"]
        .iter()
        .any(|p| redirect.starts_with(p));
    if loopback {
        return true;
    }
    let own = state.cfg.public_url.trim_end_matches('/');
    if redirect == own || redirect.starts_with(&format!("{own}/")) {
        return true;
    }
    state
        .cfg
        .auth_redirects
        .iter()
        .any(|p| !p.is_empty() && redirect.starts_with(p))
}

fn with_query(redirect: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = redirect.to_owned();
    for (i, (k, v)) in pairs.iter().enumerate() {
        out.push(if i == 0 && !redirect.contains('?') {
            '?'
        } else {
            '&'
        });
        out.push_str(k);
        out.push('=');
        out.push_str(&urlencode(v));
    }
    out
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Deserialize)]
pub struct StartQuery {
    pub redirect: String,
    #[serde(default)]
    pub device_name: Option<String>,
}

/// `GET /auth/github/start?redirect=…&device_name=…` → 302 to GitHub.
pub async fn github_start(State(state): State<AppState>, Query(q): Query<StartQuery>) -> Response {
    let (client_id, _) = match github_mode(&state) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if !redirect_allowed(&state, &q.redirect) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_redirect",
            "redirect must be arbos://auth/callback, a loopback URL, or this relay's origin",
        );
    }
    let (verifier, challenge) = crate::connectors::pkce_pair();
    let mut raw = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut raw);
    let flow = AuthFlow {
        state: B64URL.encode(raw),
        code_verifier: verifier,
        redirect: q.redirect,
        device_name: q
            .device_name
            .map(|s| s.trim().chars().take(80).collect::<String>())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "device".to_owned()),
        invite: None,
    };
    if let Err(e) = state.db.insert_auth_flow(&flow, FLOW_TTL_MS) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    let url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope=read%3Auser&state={}&code_challenge={}&code_challenge_method=S256",
        urlencode(&client_id),
        urlencode(&callback_url(&state)),
        urlencode(&flow.state),
        urlencode(&challenge),
    );
    found(&url)
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

async fn github_identity(
    state: &AppState,
    client_id: &str,
    client_secret: &str,
    code: &str,
    verifier: &str,
) -> anyhow::Result<GithubIdentity> {
    let token: Value = state
        .http
        .post("https://github.com/login/oauth/access_token")
        .header(header::ACCEPT, "application/json")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("redirect_uri", &callback_url(state)),
            ("code_verifier", verifier),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let access = token
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "github token exchange: {}",
                token
                    .get("error_description")
                    .or(token.get("error"))
                    .and_then(Value::as_str)
                    .unwrap_or("no access_token")
            )
        })?;
    let user: Value = state
        .http
        .get("https://api.github.com/user")
        .bearer_auth(access)
        .header(header::ACCEPT, "application/vnd.github+json")
        .header(header::USER_AGENT, "arbos-relay")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(GithubIdentity {
        id: user
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow::anyhow!("github user has no id"))?,
        login: user
            .get("login")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        avatar_url: user
            .get("avatar_url")
            .and_then(Value::as_str)
            .map(str::to_owned),
        name: user.get("name").and_then(Value::as_str).map(str::to_owned),
    })
}

/// `GET /auth/github/callback?code&state` (GitHub → relay). Ends with a redirect to the app:
/// `<redirect>?code=<grant>` or `<redirect>?error=…&message=…`.
pub async fn github_callback(
    State(state): State<AppState>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let (client_id, client_secret) = match github_mode(&state) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let Some(flow_state) = q.state.as_deref() else {
        return api_error(StatusCode::BAD_REQUEST, "bad_state", "missing state");
    };
    let flow = match state.db.take_auth_flow(flow_state) {
        Ok(Some(f)) => f,
        Ok(None) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_state",
                "unknown or expired sign-in attempt; start again",
            )
        }
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let back = |kind: &str, message: &str| -> Response {
        found(&with_query(
            &flow.redirect,
            &[("error", kind), ("message", message)],
        ))
    };
    if let Some(err) = q.error.as_deref() {
        let msg = q.error_description.as_deref().unwrap_or(err);
        return back(
            if err == "access_denied" {
                "oauth_denied"
            } else {
                "oauth_failed"
            },
            msg,
        );
    }
    let Some(code) = q.code.as_deref() else {
        return back("oauth_failed", "GitHub sent no code");
    };
    let gh = match github_identity(
        &state,
        &client_id,
        &client_secret,
        code,
        &flow.code_verifier,
    )
    .await
    {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!(error = %e, "github sign-in failed");
            return back("oauth_failed", &e.to_string());
        }
    };
    let (user, created) = match state
        .db
        .upsert_github_user(&gh, state.cfg.default_daily_cap_usd)
    {
        Ok(v) => v,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    if created {
        if let Err(e) = state.db.grant_credits(
            &user.id,
            state.cfg.default_grant_usd,
            "welcome grant",
            "relay",
        ) {
            tracing::warn!(error = %e, user = %user.id, "welcome grant failed");
        }
        tracing::info!(user = %user.id, login = %gh.login, "new account");
    }
    if state.cfg.always_on {
        // Always-on: the machine starts at sign-in (created now when the user has none).
        crate::boxes::ensure_awake(&state, &user.id, "signin").await;
    }
    // Invite links: every open invite addressed to this GitHub login is now accepted; the
    // inviters hear about it. The DM / project membership already exists for the placeholder.
    if let Ok(accepted) = state.db.accept_invites_for_login(&gh.login, &user.id) {
        for inv in accepted {
            let _ = state.db.push_user_event(
                &inv.inviter_user_id,
                &PushEventDto::PeopleInviteAccepted(PeopleInviteAcceptedPayloadDto {
                    code: inv.code,
                    handle: gh.login.clone(),
                    user_id: user.id.clone(),
                    target_kind: inv.target_kind,
                    target_id: inv.target_id,
                }),
                None,
            );
        }
    }
    let device_id = ulid();
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    if let Err(e) = state.db.insert_device(
        &device_id,
        &flow.device_name,
        &hash(&format!("jwt:{device_id}:{}", hex::encode(salt))),
        &user.id,
        "github",
    ) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    let mut raw = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut raw);
    let grant = B64URL.encode(raw);
    if let Err(e) = state
        .db
        .insert_auth_grant(&hash(&grant), &device_id, &user.id, GRANT_TTL_MS)
    {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    found(&with_query(&flow.redirect, &[("code", &grant)]))
}

#[derive(Deserialize)]
pub struct TokenBody {
    pub grant_code: String,
}

/// `POST /auth/token {"grant_code"}` → tokens. One use, 60 s.
pub async fn token(State(state): State<AppState>, Json(b): Json<TokenBody>) -> Response {
    match state.db.take_auth_grant(&hash(b.grant_code.trim())) {
        Ok(Some((device_id, user_id))) => match issue_tokens(&state, &user_id, &device_id) {
            Ok(dto) => Json(dto).into_response(),
            Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
        },
        Ok(None) => api_error(
            StatusCode::UNAUTHORIZED,
            "bad_grant",
            "unknown, used, or expired grant code",
        ),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct RefreshBody {
    pub refresh_token: String,
}

/// How long a just-rotated refresh token still answers with the pair it was rotated into.
/// Several requests of one app can be refused at the same instant and each try to refresh;
/// inside this window they all get the one new pair. Outside it a reused token is a replay.
pub const REFRESH_GRACE_MS: i64 = 30_000;

/// `POST /auth/refresh {"refresh_token"}` → new pair; the old refresh token is dead after
/// `REFRESH_GRACE_MS`. A token that was rotated longer ago is a replay: the whole device is revoked.
pub async fn refresh(State(state): State<AppState>, Json(b): Json<RefreshBody>) -> Response {
    let h = hash(b.refresh_token.trim());
    {
        let mut grace = state.refresh_grace.lock().await;
        let now = now_ms();
        grace.retain(|_, (at, _)| now - *at < REFRESH_GRACE_MS);
        if let Some((_, pair)) = grace.get(&h) {
            tracing::debug!("refresh token re-presented within the grace window; same pair");
            // Cached copy was built from TokenResponseDto (same as issue_tokens).
            return Json(pair.clone()).into_response();
        }
    }
    let row = match state.db.refresh_row(&h) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return api_error(
                StatusCode::UNAUTHORIZED,
                "bad_refresh",
                "unknown refresh token",
            )
        }
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    if row.revoked_at.is_some() {
        return api_error(
            StatusCode::UNAUTHORIZED,
            "bad_refresh",
            "refresh token revoked",
        );
    }
    if row.rotated_at.is_some() {
        tracing::warn!(device = %row.device_id, "refresh token replayed; revoking the device");
        let _ = state.db.revoke_device_fully(&row.device_id);
        return api_error(
            StatusCode::UNAUTHORIZED,
            "bad_refresh",
            "refresh token was already used; sign in again",
        );
    }
    if row.expires_at <= now_ms() {
        return api_error(
            StatusCode::UNAUTHORIZED,
            "bad_refresh",
            "refresh token expired",
        );
    }
    match state.db.device_status(&row.device_id) {
        Ok(Some((_, false))) => {}
        _ => return api_error(StatusCode::UNAUTHORIZED, "bad_refresh", "device revoked"),
    }
    if let Err(e) = state.db.mark_refresh_rotated(&h) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    match issue_tokens(&state, &row.user_id, &row.device_id) {
        Ok(dto) => {
            let pair = token_response_json(&dto);
            state.refresh_grace.lock().await.insert(h, (now_ms(), pair));
            Json(dto).into_response()
        }
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `POST /auth/logout` (Bearer): this device and its refresh tokens are revoked.
pub async fn logout(State(state): State<AppState>, principal: Principal) -> Response {
    match principal {
        Principal::Device { id, .. } => match state.db.revoke_device_fully(&id) {
            Ok(_) => {
                crate::matrix::servers::revoke_device(&state, &id).await;
                StatusCode::NO_CONTENT.into_response()
            }
            Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
        },
        Principal::Box { .. } | Principal::Admin => {
            api_error(StatusCode::FORBIDDEN, "forbidden", "devices only")
        }
    }
}

/// `GET /auth/devices` — the caller's devices (this one marked `current`).
pub async fn devices_list(State(state): State<AppState>, principal: Principal) -> Response {
    let Principal::Device { id, user_id, .. } = &principal else {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        );
    };
    match state.db.devices_of_user(user_id) {
        Ok(mut list) => {
            for d in &mut list {
                let current = d["id"].as_str() == Some(id.as_str());
                d["current"] = json!(current);
            }
            Json(list).into_response()
        }
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `DELETE /auth/devices/{id}` — revoke one of the caller's devices (its access and refresh
/// tokens die at once); the admin may revoke any device.
pub async fn device_revoke(
    State(state): State<AppState>,
    principal: Principal,
    axum::extract::Path(target): axum::extract::Path<String>,
) -> Response {
    let owner = match state.db.device_status(&target) {
        Ok(Some((owner, _))) => owner,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "not_found", "no such device"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let allowed = match &principal {
        Principal::Admin => true,
        Principal::Device { user_id, .. } => *user_id == owner,
        Principal::Box { .. } => false,
    };
    if !allowed {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "not your device");
    }
    match state.db.revoke_device_fully(&target) {
        Ok(_) => {
            crate::matrix::servers::revoke_device(&state, &target).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// Max JPEG for `PUT /me/avatar` (512×512 at high quality is well under this).
const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;

fn safe_avatar_user_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn avatar_file(state: &AppState, user_id: &str) -> std::path::PathBuf {
    state
        .cfg
        .data_dir
        .join("avatars")
        .join(format!("{user_id}.jpg"))
}

/// `PATCH /me {name?, login?, group_add?}`: profile and/or privacy. At least one field required.
#[derive(Deserialize)]
pub struct PatchMeBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub login: Option<String>,
    /// Who may add me to groups: `anyone` | `people_i_know` | `nobody`.
    #[serde(default)]
    pub group_add: Option<String>,
}

fn parse_group_add(raw: &str) -> Option<&'static str> {
    match raw.trim() {
        "anyone" => Some("anyone"),
        "people_i_know" => Some("people_i_know"),
        "nobody" => Some("nobody"),
        _ => None,
    }
}

fn privacy_dto(state: &AppState, user_id: &str) -> MePrivacyDto {
    MePrivacyDto {
        group_add: state
            .db
            .group_add_policy(user_id)
            .unwrap_or_else(|_| "anyone".into()),
    }
}

fn valid_handle(handle: &str) -> bool {
    !handle.is_empty()
        && handle.len() <= 39
        && handle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !handle.starts_with('-')
        && !handle.ends_with('-')
}

pub async fn patch_me(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<PatchMeBody>,
) -> Response {
    let user_id = match &principal {
        Principal::Device { user_id, .. } => user_id.clone(),
        Principal::Box { .. } | Principal::Admin => {
            return api_error(StatusCode::FORBIDDEN, "forbidden", "device token required")
        }
    };
    if body.name.is_none() && body.login.is_none() && body.group_add.is_none() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "provide name, login, and/or group_add",
        );
    }

    // Trim and cap the display name; empty clears it (clients show @handle).
    let name: Option<String> = body.name.as_ref().map(|s| {
        let t = s.trim();
        if t.len() > 80 {
            t.chars().take(80).collect()
        } else {
            t.to_owned()
        }
    });

    let login = match body.login.as_ref() {
        None => None,
        Some(raw) => {
            let handle = raw.trim().trim_start_matches('@').to_owned();
            if !valid_handle(&handle) {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "bad_handle",
                    "handle must be 1–39 letters, numbers, or hyphens",
                );
            }
            if let Ok(Some(other)) = state.db.user_by_handle(&handle) {
                let other_id = other.id.as_str();
                if other_id != user_id {
                    return api_error(
                        StatusCode::CONFLICT,
                        "handle_taken",
                        "that handle is already taken",
                    );
                }
            }
            Some(handle)
        }
    };

    let group_add = match body.group_add.as_deref() {
        None => None,
        Some(raw) => match parse_group_add(raw) {
            Some(v) => Some(v),
            None => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "bad_group_add",
                    "group_add must be anyone, people_i_know, or nobody",
                )
            }
        },
    };

    if name.is_some() || login.is_some() {
        if let Err(e) = state
            .db
            .set_user_profile(&user_id, name.as_deref(), login.as_deref())
        {
            return if e.to_string() == "no such user" {
                api_error(StatusCode::NOT_FOUND, "not_found", "no such user")
            } else {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string())
            };
        }
    }
    if let Some(policy) = group_add {
        if let Err(e) = state.db.set_group_add_policy(&user_id, policy) {
            return if e.to_string() == "no such user" {
                api_error(StatusCode::NOT_FOUND, "not_found", "no such user")
            } else {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string())
            };
        }
    }
    Json(MeUserResponseDto {
        user: user_dto(&state, &user_id),
        avatar_url: None,
    })
    .into_response()
}

/// `PUT /me/avatar`: raw JPEG body → public `{public_url}/avatars/{user_id}.jpg?v=…`.
pub async fn put_avatar(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let user_id = match &principal {
        Principal::Device { user_id, .. } => user_id.clone(),
        Principal::Box { .. } | Principal::Admin => {
            return api_error(StatusCode::FORBIDDEN, "forbidden", "device token required")
        }
    };
    if !safe_avatar_user_id(&user_id) {
        return api_error(StatusCode::BAD_REQUEST, "bad_user", "bad user id");
    }
    // Header and bytes must both say "image" (the shared gate); the file is then served as
    // `{user}.jpg`, so of the image types only a JPEG is stored here — the apps re-encode.
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    match check_avatar(content_type, &body) {
        Ok(ImageKind::Jpeg) => {}
        Ok(other) => {
            return api_error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "not_jpeg",
                format!(
                    "Profile pictures are stored as JPEG; this is a {other}. Re-encode it first"
                ),
            );
        }
        Err(why) => return not_an_image(&why),
    }
    if body.len() > MAX_AVATAR_BYTES {
        return api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            "avatar must be at most 2 MB",
        );
    }
    let path = avatar_file(&state, &user_id);
    if let Some(parent) = path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string());
        }
    }
    if let Err(e) = tokio::fs::write(&path, &body).await {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string());
    }
    let v = now_ms();
    let avatar_url = format!(
        "{}/avatars/{user_id}.jpg?v={v}",
        state.cfg.public_url.trim_end_matches('/')
    );
    if let Err(e) = state.db.set_user_avatar_url(&user_id, &avatar_url) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    Json(MeUserResponseDto {
        user: user_dto(&state, &user_id),
        avatar_url: Some(Some(avatar_url)),
    })
    .into_response()
}

/// `DELETE /me/avatar`: drop the uploaded picture; the GitHub avatar (by github_id, GitHub's
/// stable URL) shows again, or the D79 coloured initials when there is none.
pub async fn delete_avatar(State(state): State<AppState>, principal: Principal) -> Response {
    let user_id = match &principal {
        Principal::Device { user_id, .. } => user_id.clone(),
        Principal::Box { .. } | Principal::Admin => {
            return api_error(StatusCode::FORBIDDEN, "forbidden", "device token required")
        }
    };
    if !safe_avatar_user_id(&user_id) {
        return api_error(StatusCode::BAD_REQUEST, "bad_user", "bad user id");
    }
    let _ = tokio::fs::remove_file(avatar_file(&state, &user_id)).await;
    let github = state
        .db
        .user(&user_id)
        .ok()
        .flatten()
        .and_then(|u| u.github_id)
        .map(|id| format!("https://avatars.githubusercontent.com/u/{id}?v=4"));
    match state.db.clear_user_avatar_url(&user_id, github.as_deref()) {
        Ok(()) => Json(MeUserResponseDto {
            user: user_dto(&state, &user_id),
            avatar_url: Some(github),
        })
        .into_response(),
        Err(e) if e.to_string() == "no such user" => {
            api_error(StatusCode::NOT_FOUND, "not_found", "no such user")
        }
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `GET /avatars/{user_id}.jpg` — public (same role as a GitHub CDN avatar URL).
pub async fn get_avatar(State(state): State<AppState>, Path(file): Path<String>) -> Response {
    let user_id = file.strip_suffix(".jpg").unwrap_or(file.as_str());
    if !safe_avatar_user_id(user_id) {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such avatar");
    }
    let path = avatar_file(&state, user_id);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg")),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=86400"),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => api_error(StatusCode::NOT_FOUND, "not_found", "no such avatar"),
    }
}

/// `GET /me`: who am I, what can I spend, where is my machine.
pub async fn me(State(state): State<AppState>, principal: Principal) -> Response {
    let (user_id, device) = match &principal {
        Principal::Device { id, user_id, .. } => (user_id.clone(), Some(id.clone())),
        Principal::Box { user_id } => (user_id.clone(), None),
        Principal::Admin => {
            return api_error(StatusCode::FORBIDDEN, "forbidden", "device token required")
        }
    };
    let credits = match state
        .db
        .credits(&user_id, crate::proxy::start_of_utc_day_ms())
    {
        Ok(c) => c,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let ks = state.db.kill_switch().unwrap_or_default();
    let (bs, since) = state.box_state(&user_id).await;
    let sb = state.db.user_box(&user_id).ok().flatten();
    let device = device.and_then(|id| {
        state
            .db
            .devices_of_user(&user_id)
            .ok()?
            .into_iter()
            .find(|d| d.get("id").and_then(Value::as_str) == Some(id.as_str()))
            .and_then(|d| {
                Some(MeDeviceDto {
                    id: d.get("id")?.as_str()?.to_owned(),
                    name: d.get("name")?.as_str()?.to_owned(),
                    created_at: d.get("created_at")?.as_i64()?,
                    revoked_at: d.get("revoked_at").and_then(Value::as_i64),
                    last_seen_at: d.get("last_seen_at").and_then(Value::as_i64),
                    kind: d.get("kind")?.as_str()?.to_owned(),
                })
            })
    });
    // D89: a host that is full waitlists new machines; `signup` tells the apps so, and a
    // waitlisted user sees their place in line instead of a machine.
    let fleet = crate::fleet::fleet_enabled(&state);
    let waitlist_position = if fleet {
        state.db.waitlist_position(&user_id).ok().flatten()
    } else {
        None
    };
    let signup = if fleet && sb.is_none() && !crate::fleet::capacity(&state).await.has_room {
        "waitlist"
    } else {
        "open"
    };
    Json(MeResponseDto {
        user: user_dto(&state, &user_id),
        device,
        credits: CreditsDto {
            balance_usd: credits.balance_usd,
            granted_usd: credits.granted_usd,
            spent_usd: credits.spent_usd,
            spent_today_usd: credits.spent_today_usd,
            daily_cap_usd: credits.daily_cap_usd,
            allow_negative: credits.allow_negative,
            machine_today_usd: credits.machine_today_usd,
            machine_total_usd: credits.machine_total_usd,
        },
        limits: MeLimitsDto {
            read_only: ks.on,
            read_only_reason: ks.reason,
            signup: signup.to_owned(),
            auth_mode: state.cfg.auth_mode.as_str().to_owned(),
        },
        features: crate::features::dto(),
        machine: MeMachineDto {
            exists: sb.is_some(),
            state: bs,
            since,
            provider: sb.as_ref().map(|s| s.provider.clone()),
            sandbox_id: sb.as_ref().and_then(|s| s.sandbox_id.clone()),
            always_on: state.cfg.always_on,
            held: state.db.box_held(&user_id).unwrap_or(false),
            waitlisted: waitlist_position.is_some(),
            waitlist_position: waitlist_position.map(|p| p as i64),
            evicted: crate::fleet::is_evicted(&state, &user_id).await,
        },
        privacy: privacy_dto(&state, &user_id),
    })
    .into_response()
}

/// `GET /download/mac` → 302 to the newest **stable** DMG (the `latest` release's `latest.txt`
/// names the version; the asset is `Arbos-<v>-macos-universal.dmg` on `v<v>`), falling back to
/// the newest edge DMG when no stable release exists. `ARBOS_DOWNLOAD_CHANNEL=edge` forces
/// edge. Cached for five minutes.
pub async fn download_mac(State(state): State<AppState>) -> Response {
    let repo = state.cfg.releases_repo.trim_matches('/');
    let now = now_ms();
    if let Some((at, url)) = state.download_cache.lock().await.clone() {
        if now - at < DOWNLOAD_CACHE_MS {
            return found(&url);
        }
    }
    let fetch_version = |url: String| {
        let http = state.http.clone();
        async move {
            match http.get(&url).send().await {
                Ok(r) if r.status().is_success() => {
                    let v = r.text().await.unwrap_or_default().trim().to_owned();
                    (!v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.'))
                        .then_some(v)
                }
                _ => None,
            }
        }
    };
    let prefer_edge = std::env::var("ARBOS_DOWNLOAD_CHANNEL").is_ok_and(|c| c.trim() == "edge");
    let mut url = None;
    if !prefer_edge {
        if let Some(v) = fetch_version(format!(
            "https://github.com/{repo}/releases/latest/download/latest.txt"
        ))
        .await
        {
            url = Some(format!(
                "https://github.com/{repo}/releases/download/v{v}/Arbos-{v}-macos-universal.dmg"
            ));
        }
    }
    if url.is_none() {
        if let Some(v) = fetch_version(format!(
            "https://github.com/{repo}/releases/download/edge/latest.txt"
        ))
        .await
        {
            url = Some(format!(
                "https://github.com/{repo}/releases/download/edge-{v}/Arbos-{v}-macos-universal.dmg"
            ));
        }
    }
    let Some(url) = url else {
        tracing::warn!("no release version could be read; sending the releases page");
        return found(&format!("https://github.com/{repo}/releases"));
    };
    *state.download_cache.lock().await = Some((now, url.clone()));
    found(&url)
}

#[cfg(test)]
mod avatar_tests {
    //! `PUT /me/avatar` takes a JPEG that says it is one; anything else is refused with a
    //! plain message, whatever the client called the file.

    use axum::extract::State;
    use axum::http::{header, HeaderMap, HeaderValue, StatusCode};

    use super::*;
    use crate::image_sniff::samples;
    use crate::people::tests_support::person;
    use crate::state::test_support::test_state;

    async fn put(state: &AppState, who: &Principal, mime: Option<&str>, bytes: &[u8]) -> Response {
        let mut headers = HeaderMap::new();
        if let Some(m) = mime {
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(m).unwrap());
        }
        put_avatar(
            State(state.clone()),
            who.clone(),
            headers,
            Bytes::copy_from_slice(bytes),
        )
        .await
    }

    async fn error_message(r: Response) -> String {
        let body = axum::body::to_bytes(r.into_body(), 1 << 16).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    #[tokio::test]
    async fn profile_picture_must_be_a_jpeg_by_type_and_bytes() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 41);

        // A model file sent with a JPEG header: the bytes are not a picture.
        let r = put(&state, &alice_dev, Some("image/jpeg"), samples::ONNX).await;
        assert_eq!(r.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let msg = error_message(r).await;
        assert!(msg.contains("not an image"), "{msg}");

        // A model file with its honest type.
        let r = put(
            &state,
            &alice_dev,
            Some("application/octet-stream"),
            samples::ONNX,
        )
        .await;
        assert_eq!(r.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        // A JPEG with no Content-Type at all: the type is part of the contract.
        let r = put(&state, &alice_dev, None, samples::JPEG).await;
        assert_eq!(r.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        // A real PNG: an image, but this endpoint stores JPEG only (the apps re-encode).
        let r = put(&state, &alice_dev, Some("image/png"), samples::PNG).await;
        assert_eq!(r.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let msg = error_message(r).await;
        assert!(msg.contains("JPEG"), "{msg}");

        assert!(
            !avatar_file(&state, &alice).exists(),
            "nothing was written by the refused uploads"
        );
        assert!(state
            .db
            .user(&alice)
            .unwrap()
            .unwrap()
            .avatar_url
            .as_deref()
            .is_none_or(|u| !u.contains("/avatars/")));

        // A JPEG that says it is one: stored and published.
        let r = put(&state, &alice_dev, Some("image/jpeg"), samples::JPEG).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(avatar_file(&state, &alice).exists());
        let url = state
            .db
            .user(&alice)
            .unwrap()
            .unwrap()
            .avatar_url
            .unwrap_or_default();
        assert!(url.contains(&format!("/avatars/{alice}.jpg")), "{url}");
    }
}
