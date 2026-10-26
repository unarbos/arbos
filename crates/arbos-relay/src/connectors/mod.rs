//! Connectors (F1): the relay is the OAuth broker and token store for external services
//! (Google, Slack, GitHub, Harness Comms). Tokens are encrypted at rest and never leave this
//! process: a box asks `POST /connectors/call { provider, action, args }` and the relay makes
//! the upstream request, refreshes expired tokens, and returns a redacted JSON reply (D2, D28).
//!
//! `ARBOS_CONNECTORS_MOCK=1` answers every action with canned data and records writes/sends in
//! `connector_outbox`, so the whole flow is demonstrable without any credential.

pub mod crypto;
pub mod providers;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use arbos_core::api::{
    ConnectConnectorReplyDto, ConnectorDto, ConnectorsCatalogDto, ConnectorsListDto,
};
use arbos_core::ids::{now_ms, ulid};

use crate::auth::Principal;
use crate::db::{Connector, ConnectorTokenRow, OauthState};
use crate::state::AppState;
use providers::{ActionKind, Auth, Provider, TokenGrant};

type ApiResult = Result<Response, (StatusCode, String)>;

const OAUTH_STATE_TTL_MS: i64 = 10 * 60 * 1000;
const REFRESH_SKEW_MS: i64 = 60_000;
/// Upstream replies above this many serialized bytes are cut; the model gets a note instead.
const MAX_REPLY_BYTES: usize = 200_000;
const REDACTED: &str = "[redacted]";

fn internal(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn mock_on(state: &AppState) -> bool {
    state.cfg.connectors_mock
}

fn redirect_uri(state: &AppState) -> String {
    std::env::var("ARBOS_OAUTH_REDIRECT_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/connectors/oauth/callback", state.cfg.public_url))
}

/// Client-facing view of a connector: never includes a token.
pub fn connector_dto(c: &Connector, mock: bool) -> ConnectorDto {
    ConnectorDto {
        id: c.id.clone(),
        provider: c.provider.clone(),
        label: c.label.clone(),
        scopes: c.scopes.clone(),
        status: c.status.clone(),
        config: c.config.clone(),
        created_at: c.created_at,
        last_used_at: c.last_used_at,
        last_error: c.last_error.clone(),
        mock,
    }
}

/// Strip credentials from an upstream reply: known credential keys are blanked and any string
/// containing one of `secrets` is replaced. Keys are matched case-insensitively.
pub fn redact(v: &mut Value, secrets: &[&str]) {
    const KEYS: &[&str] = &[
        "access_token",
        "refresh_token",
        "authorization",
        "id_token",
        "client_secret",
        "api_key",
        "private_key",
    ];
    match v {
        Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                let kl = k.to_ascii_lowercase();
                if KEYS.iter().any(|x| *x == kl) {
                    *val = Value::String(REDACTED.into());
                } else {
                    redact(val, secrets);
                }
            }
        }
        Value::Array(items) => {
            for it in items {
                redact(it, secrets);
            }
        }
        Value::String(s) => {
            if secrets.iter().any(|sec| !sec.is_empty() && s.contains(sec)) {
                *s = REDACTED.into();
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

// ---- catalog and list -------------------------------------------------------------------------

pub async fn catalog(State(state): State<AppState>, _principal: Principal) -> ApiResult {
    let mock = mock_on(&state);
    let providers = Provider::ALL
        .iter()
        .map(|p| {
            let configured = match p.auth() {
                Auth::Oauth(o) => o.client().is_some(),
                Auth::Token => true,
            };
            providers::provider_dto(*p, configured, mock)
        })
        .collect();
    Ok(Json(ConnectorsCatalogDto {
        mock,
        redirect_uri: redirect_uri(&state),
        providers,
    })
    .into_response())
}

pub async fn list(State(state): State<AppState>, _principal: Principal) -> ApiResult {
    let mock = mock_on(&state);
    let items = state
        .db
        .list_connectors()
        .map_err(internal)?
        .iter()
        .map(|c| connector_dto(c, mock))
        .collect();
    Ok(Json(ConnectorsListDto { mock, items }).into_response())
}

/// Mock mode only: what would have been sent.
pub async fn outbox(State(state): State<AppState>, principal: Principal) -> ApiResult {
    if principal.is_box() {
        return Err((StatusCode::FORBIDDEN, "device only".into()));
    }
    let items = state.db.connector_outbox(100).map_err(internal)?;
    Ok(Json(json!({ "mock": mock_on(&state), "items": items })).into_response())
}

// ---- connect / callback / disconnect ----------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct ConnectBody {
    /// A pasted PAT / API key (GitHub, Slack, Harness Comms) instead of the OAuth flow.
    #[serde(default)]
    pub token: Option<String>,
    /// `harness_comms`: its current service base URL.
    #[serde(default, rename = "baseUrl")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

pub(crate) fn pkce_pair() -> (String, String) {
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    let verifier = B64URL.encode(raw);
    let challenge = B64URL.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn store_token(
    state: &AppState,
    provider: Provider,
    grant: &TokenGrant,
    config: Value,
) -> anyhow::Result<Connector> {
    let existing = state.db.connector_by_provider(provider.as_str())?;
    let id = existing.as_ref().map(|c| c.id.clone()).unwrap_or_else(ulid);
    let created_at = existing.as_ref().map(|c| c.created_at).unwrap_or(now_ms());
    let cn = Connector {
        id: id.clone(),
        provider: provider.as_str().to_owned(),
        label: grant.label.clone(),
        scopes: grant.scopes.clone(),
        status: "connected".into(),
        config,
        created_at,
        last_used_at: None,
        last_error: None,
    };
    state.db.upsert_connector(&cn)?;
    let row = ConnectorTokenRow {
        connector_id: id,
        access_token: state.relay_key.encrypt(&grant.access)?,
        refresh_token: grant
            .refresh
            .as_deref()
            .map(|r| state.relay_key.encrypt(r))
            .transpose()?,
        expires_at: grant.expires_in_s.map(|s| now_ms() + s * 1000),
        token_type: "Bearer".into(),
    };
    state.db.put_connector_token(&row)?;
    Ok(cn)
}

/// `POST /connectors/{provider}/connect`: start the OAuth code flow (`{ authUrl }`) or store a
/// pasted token (`{ token }`).
pub async fn connect(
    State(state): State<AppState>,
    principal: Principal,
    Path(provider): Path<String>,
    body: Option<Json<ConnectBody>>,
) -> ApiResult {
    if principal.is_box() {
        return Err((
            StatusCode::FORBIDDEN,
            "boxes cannot connect accounts".into(),
        ));
    }
    let p = Provider::parse(&provider).ok_or((
        StatusCode::NOT_FOUND,
        format!("unknown provider {provider}"),
    ))?;
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let mock = mock_on(&state);

    // Token path (PAT / API key).
    if let Some(token) = b.token.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let accepts = match p.auth() {
            Auth::Token => true,
            Auth::Oauth(o) => o.accepts_token,
        };
        if !accepts {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("{provider} only connects through OAuth"),
            ));
        }
        let mut config = json!({});
        if p == Provider::HarnessComms {
            let base = b
                .base_url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_owned)
                .or_else(|| std::env::var("ARBOS_HARNESS_COMMS_URL").ok())
                .filter(|u| !u.is_empty())
                .ok_or((
                    StatusCode::BAD_REQUEST,
                    "harness_comms needs baseUrl (or ARBOS_HARNESS_COMMS_URL)".to_owned(),
                ))?;
            config["baseUrl"] = json!(base.trim_end_matches('/'));
        }
        let label = b
            .label
            .clone()
            .unwrap_or_else(|| format!("{} · token", p.label()));
        let scopes = match p.auth() {
            Auth::Oauth(o) => o.scopes.iter().map(|s| s.to_string()).collect(),
            Auth::Token => vec![
                "read".into(),
                "search".into(),
                "draft".into(),
                "request_send".into(),
            ],
        };
        let grant = TokenGrant {
            access: token.to_owned(),
            refresh: None,
            expires_in_s: None,
            scopes,
            label,
        };
        let cn = store_token(&state, p, &grant, config).map_err(internal)?;
        // Verify the token with one cheap read so a typo shows up as `needs_auth` right away.
        let cn = if mock {
            cn
        } else {
            match verify_connection(&state, p, &cn).await {
                Ok(()) => cn,
                Err(e) => {
                    let _ = state
                        .db
                        .set_connector_status(&cn.id, "needs_auth", Some(&e));
                    state.db.connector(&cn.id).map_err(internal)?.unwrap_or(cn)
                }
            }
        };
        return Ok(Json(ConnectConnectorReplyDto {
            auth_url: None,
            state: None,
            redirect_uri: None,
            mock: None,
            connector: Some(connector_dto(&cn, mock)),
        })
        .into_response());
    }

    // OAuth path.
    let Auth::Oauth(oauth) = p.auth() else {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("{provider} connects with a token: POST {{ \"token\": … }}"),
        ));
    };
    let redirect = redirect_uri(&state);
    // Resolve the auth URL (and fail if the OAuth client is missing) *before*
    // writing a needs_auth placeholder — otherwise a misconfigured relay leaves
    // Settings stuck on "Needs sign-in" with a Connect button that only errors.
    let (verifier, challenge) = pkce_pair();
    let st = ulid();
    let auth_url = if mock {
        // Mock: the browser lands straight on our callback, which mints a fake token.
        format!("{redirect}?state={st}&code=mock")
    } else {
        // Keep env var names out of the client alert (Settings shows this text).
        let (client_id, _) = oauth.client().ok_or((
            StatusCode::CONFLICT,
            format!("{provider} OAuth client not configured on the relay"),
        ))?;
        let mut url = reqwest::Url::parse(oauth.auth_url).map_err(internal)?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("client_id", &client_id)
                .append_pair("redirect_uri", &redirect)
                .append_pair("response_type", "code")
                .append_pair("state", &st)
                .append_pair("code_challenge", &challenge)
                .append_pair("code_challenge_method", "S256");
            // Slack wants the user's scopes under `user_scope`; everyone else uses `scope`.
            let scope_key = if p == Provider::Slack {
                "user_scope"
            } else {
                "scope"
            };
            qp.append_pair(scope_key, &oauth.scopes.join(" "));
            for (k, v) in oauth.extra_auth_params {
                qp.append_pair(k, v);
            }
        }
        url.to_string()
    };
    state
        .db
        .put_oauth_state(
            &OauthState {
                state: st.clone(),
                provider: p.as_str().to_owned(),
                code_verifier: verifier,
                redirect_uri: redirect.clone(),
                scopes: oauth.scopes.iter().map(|s| s.to_string()).collect(),
            },
            OAUTH_STATE_TTL_MS,
        )
        .map_err(internal)?;
    // A placeholder row so the client shows "Needs sign-in" until the callback lands.
    if state
        .db
        .connector_by_provider(p.as_str())
        .map_err(internal)?
        .is_none()
    {
        state
            .db
            .upsert_connector(&Connector {
                id: ulid(),
                provider: p.as_str().to_owned(),
                label: p.label().to_owned(),
                scopes: Vec::new(),
                status: "needs_auth".into(),
                config: json!({}),
                created_at: now_ms(),
                last_used_at: None,
                last_error: None,
            })
            .map_err(internal)?;
    }
    Ok(Json(ConnectConnectorReplyDto {
        auth_url: Some(auth_url),
        state: Some(st),
        redirect_uri: Some(redirect),
        mock: Some(mock),
        connector: None,
    })
    .into_response())
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

/// Success / error HTML after Google (etc.) consent. Redirects to
/// `arbos://connectors/callback` so iOS `ASWebAuthenticationSession` (and the
/// Mac/iPhone URL scheme) bring the user back into Arbos instead of leaving
/// them on a dead Safari tab.
fn page(title: &str, body: &str) -> Response {
    done_page(title, body, None, false)
}

fn done_page(title: &str, body: &str, provider: Option<&str>, ok: bool) -> Response {
    let mut href = format!(
        "arbos://connectors/callback?ok={}",
        if ok { "1" } else { "0" }
    );
    if let Some(p) = provider {
        href.push_str("&provider=");
        href.push_str(p);
    }
    // 302 to arbos:// so ASWebAuthenticationSession closes (same hop as GitHub sign-in).
    // HTML body is a fallback if a desktop browser ignores the custom-scheme Location.
    (
        StatusCode::FOUND,
        [(header::LOCATION, href.clone())],
        Html(format!(
            "<!doctype html><meta charset=utf-8><title>Arbos · {title}</title>\
             <body style=\"font:15px system-ui;max-width:32em;margin:12vh auto;color:#222\">\
             <h2 style=\"font-weight:600\">{title}</h2><p>{body}</p>\
             <p><a href=\"{href}\">Return to Arbos</a></p>\
             <p style=\"color:#777\">You can close this tab if Arbos is already open.</p></body>"
        )),
    )
        .into_response()
}

/// `GET /connectors/oauth/callback?code=&state=`: the browser lands here after consent.
pub async fn oauth_callback(
    State(state): State<AppState>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let Some(st) = q.state.as_deref() else {
        return page("Missing state", "The redirect had no state parameter.");
    };
    let Ok(Some(os)) = state.db.take_oauth_state(st) else {
        return page(
            "Unknown or expired request",
            "Start the connection again from Arbos → Settings → Connectors.",
        );
    };
    let Some(p) = Provider::parse(&os.provider) else {
        return done_page("Unknown provider", &os.provider, Some(&os.provider), false);
    };
    let mark_error = |e: &str| {
        if let Ok(Some(c)) = state.db.connector_by_provider(p.as_str()) {
            let _ = state.db.set_connector_status(&c.id, "error", Some(e));
        }
    };
    if let Some(err) = q.error {
        let detail = q.error_description.unwrap_or_default();
        mark_error(&format!("{err} {detail}"));
        return done_page(
            "Connection refused",
            &format!("{err} {detail}"),
            Some(p.as_str()),
            false,
        );
    }
    let Some(code) = q.code.as_deref() else {
        return done_page(
            "Missing code",
            "The redirect had no authorization code.",
            Some(p.as_str()),
            false,
        );
    };

    if mock_on(&state) {
        let fake = format!("mock-{}-{}", p.as_str(), ulid().to_lowercase());
        let grant = TokenGrant {
            access: fake,
            refresh: Some("mock-refresh".into()),
            expires_in_s: Some(3600),
            scopes: os.scopes,
            label: format!("{} · mock", p.label()),
        };
        return match store_token(&state, p, &grant, json!({})) {
            Ok(_) => done_page(
                &format!("{} connected (mock)", p.label()),
                "Mock mode: a fake token was stored; every action returns canned data.",
                Some(p.as_str()),
                true,
            ),
            Err(e) => done_page(
                "Could not store the token",
                &e.to_string(),
                Some(p.as_str()),
                false,
            ),
        };
    }

    let Auth::Oauth(oauth) = p.auth() else {
        return done_page("Not an OAuth provider", p.label(), Some(p.as_str()), false);
    };
    let Some((client_id, client_secret)) = oauth.client() else {
        return done_page(
            "OAuth client not configured",
            oauth.client_id_env,
            Some(p.as_str()),
            false,
        );
    };
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", os.redirect_uri.as_str()),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("code_verifier", os.code_verifier.as_str()),
    ];
    let resp = state
        .http
        .post(oauth.token_url)
        .header("Accept", "application/json")
        .form(&form)
        .send()
        .await;
    let v: Value = match resp {
        Ok(r) => match r.json().await {
            Ok(v) => v,
            Err(e) => {
                mark_error(&e.to_string());
                return done_page(
                    "Token exchange failed",
                    "The provider's reply was not JSON.",
                    Some(p.as_str()),
                    false,
                );
            }
        },
        Err(e) => {
            mark_error(&e.to_string());
            return done_page(
                "Token exchange failed",
                &e.to_string(),
                Some(p.as_str()),
                false,
            );
        }
    };
    match providers::parse_token_response(p, &v) {
        Ok(mut grant) => {
            if grant.scopes.is_empty() {
                grant.scopes = os.scopes;
            }
            match store_token(&state, p, &grant, json!({})) {
                Ok(_) => done_page(
                    &format!("{} connected", p.label()),
                    "Arbos can now act on your behalf through the relay.",
                    Some(p.as_str()),
                    true,
                ),
                Err(e) => done_page(
                    "Could not store the token",
                    &e.to_string(),
                    Some(p.as_str()),
                    false,
                ),
            }
        }
        Err(e) => {
            mark_error(&e);
            done_page("Token exchange failed", &e, Some(p.as_str()), false)
        }
    }
}

pub async fn disconnect(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    if principal.is_box() {
        return Err((
            StatusCode::FORBIDDEN,
            "boxes cannot disconnect accounts".into(),
        ));
    }
    let ok = state.db.delete_connector(&id).map_err(internal)?;
    Ok(if ok {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
    .into_response())
}

// ---- broker -----------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CallBody {
    pub provider: String,
    pub action: String,
    #[serde(default)]
    pub args: Value,
}

/// `POST /connectors/call`: the box asks, the relay performs the upstream request with the real
/// token and returns a redacted reply.
pub async fn call(
    State(state): State<AppState>,
    _principal: Principal,
    Json(b): Json<CallBody>,
) -> ApiResult {
    let args = if b.args.is_null() { json!({}) } else { b.args };
    let v = broker_call(&state, &b.provider, &b.action, &args).await?;
    Ok(Json(v).into_response())
}

/// One cheap read to confirm a pasted token works.
async fn verify_connection(state: &AppState, p: Provider, cn: &Connector) -> Result<(), String> {
    let action = match p {
        Provider::Github => "list_repos",
        Provider::Slack => "list_channels",
        Provider::Google => "list_threads",
        Provider::HarnessComms => "status",
    };
    let v = broker_call(
        state,
        p.as_str(),
        action,
        &json!({ "per_page": 1, "limit": 1, "max": 1 }),
    )
    .await
    .map_err(|(_, e)| e)?;
    let _ = cn;
    if v["ok"].as_bool() == Some(true) {
        Ok(())
    } else {
        Err(format!(
            "verification failed: HTTP {} {}",
            v["status"],
            v["data"].to_string().chars().take(200).collect::<String>()
        ))
    }
}

/// Decrypted bearer token for a connector, refreshed first when it is about to expire.
pub(crate) async fn bearer_for(
    state: &AppState,
    p: Provider,
    cn: &Connector,
) -> Result<(String, Vec<String>), (StatusCode, String)> {
    let row = state.db.connector_token(&cn.id).map_err(internal)?.ok_or((
        StatusCode::CONFLICT,
        format!("{} has no token; connect it again", p.as_str()),
    ))?;
    let mut access = state
        .relay_key
        .decrypt(&row.access_token)
        .map_err(internal)?;
    let refresh = row
        .refresh_token
        .as_deref()
        .map(|r| state.relay_key.decrypt(r))
        .transpose()
        .map_err(internal)?;
    let expiring = row
        .expires_at
        .is_some_and(|t| t - REFRESH_SKEW_MS <= now_ms());
    if expiring {
        if let (Auth::Oauth(oauth), Some(rt)) = (p.auth(), refresh.as_deref()) {
            if let Some((client_id, client_secret)) = oauth.client() {
                let form = [
                    ("grant_type", "refresh_token"),
                    ("refresh_token", rt),
                    ("client_id", client_id.as_str()),
                    ("client_secret", client_secret.as_str()),
                ];
                let reply: Result<Value, String> = async {
                    let r = state
                        .http
                        .post(oauth.token_url)
                        .header("Accept", "application/json")
                        .form(&form)
                        .send()
                        .await
                        .map_err(|e| e.to_string())?;
                    r.json::<Value>().await.map_err(|e| e.to_string())
                }
                .await;
                match reply.and_then(|v| providers::parse_token_response(p, &v)) {
                    Ok(grant) => {
                        let new_refresh = grant.refresh.or(refresh.clone());
                        let _ = state.db.put_connector_token(&ConnectorTokenRow {
                            connector_id: cn.id.clone(),
                            access_token: state
                                .relay_key
                                .encrypt(&grant.access)
                                .map_err(internal)?,
                            refresh_token: new_refresh
                                .as_deref()
                                .map(|r| state.relay_key.encrypt(r))
                                .transpose()
                                .map_err(internal)?,
                            expires_at: grant.expires_in_s.map(|s| now_ms() + s * 1000),
                            token_type: row.token_type.clone(),
                        });
                        let _ = state.db.set_connector_status(&cn.id, "connected", None);
                        access = grant.access;
                    }
                    Err(e) => {
                        let _ = state
                            .db
                            .set_connector_status(&cn.id, "needs_auth", Some(&e));
                        return Err((
                            StatusCode::CONFLICT,
                            format!("{}: token refresh failed ({e}); reconnect", p.as_str()),
                        ));
                    }
                }
            }
        }
    }
    let mut secrets = vec![access.clone()];
    if let Some(r) = refresh {
        secrets.push(r);
    }
    Ok((access, secrets))
}

/// The broker proper, shared by the HTTP handler and the invariant test.
pub async fn broker_call(
    state: &AppState,
    provider: &str,
    action: &str,
    args: &Value,
) -> Result<Value, (StatusCode, String)> {
    let p = Provider::parse(provider).ok_or((
        StatusCode::NOT_FOUND,
        format!("unknown provider {provider}"),
    ))?;
    let spec = p.action(action).ok_or((
        StatusCode::NOT_FOUND,
        format!("{provider} has no action {action}"),
    ))?;
    let missing = spec.missing(args);
    if !missing.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("{provider}.{action}: missing args {}", missing.join(", ")),
        ));
    }
    let cn = state
        .db
        .connector_by_provider(p.as_str())
        .map_err(internal)?
        .ok_or((
            StatusCode::NOT_FOUND,
            format!("{provider} is not connected (Settings → Connectors)"),
        ))?;
    let mock = mock_on(state);
    if !mock && cn.status != "connected" {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "{provider} is {}{}; reconnect it in Settings → Connectors",
                cn.status,
                cn.last_error
                    .as_deref()
                    .map(|e| format!(" ({e})"))
                    .unwrap_or_default()
            ),
        ));
    }
    let _ = state.db.touch_connector(&cn.id);

    if mock {
        if spec.kind.needs_approval() {
            let _ = state
                .db
                .record_connector_outbox(p.as_str(), spec.name, args);
        }
        let mut data = providers::mock_response(p, spec, args);
        redact(&mut data, &[]);
        return Ok(
            json!({ "ok": true, "mock": true, "status": 200, "provider": p.as_str(), "action": spec.name, "kind": spec.kind.as_str(), "data": data }),
        );
    }

    let (bearer, secrets) = bearer_for(state, p, &cn).await?;
    let up =
        providers::build(p, spec, args, &cn.config).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let mut req = state
        .http
        .request(up.method, &up.url)
        .bearer_auth(&bearer)
        .header("User-Agent", "arbos-relay")
        .query(&up.query);
    for (k, v) in &up.headers {
        req = req.header(*k, v);
    }
    if let Some(body) = &up.json {
        req = req.json(body);
    }
    let resp = req.send().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("{provider} unreachable: {e}"),
        )
    })?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let mut data: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    if status.is_success() {
        data = providers::shape_response(p, spec.name, data);
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = state.db.set_connector_status(
            &cn.id,
            "needs_auth",
            Some(&format!("{provider} answered 401")),
        );
    }
    // Slack reports errors as 200 + ok:false.
    let slack_failed =
        p == Provider::Slack && data.get("ok").and_then(Value::as_bool) == Some(false);
    let secret_refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
    redact(&mut data, &secret_refs);
    if data.to_string().len() > MAX_REPLY_BYTES {
        let s = data.to_string();
        let cut: String = s.chars().take(MAX_REPLY_BYTES / 2).collect();
        data = json!({ "truncated": true, "bytes": s.len(), "text": cut });
    }
    let kind: ActionKind = spec.kind;
    Ok(json!({
        "ok": status.is_success() && !slack_failed, "status": status.as_u16(), "provider": p.as_str(),
        "action": spec.name, "kind": kind.as_str(), "data": data,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::state::Config;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;
    use tokio::sync::{Mutex, RwLock};

    fn test_state(mock: bool) -> AppState {
        let dir = std::env::temp_dir().join(format!("arbos-connectors-test-{}", ulid()));
        let db = Db::open(&dir.join("relay.db")).expect("db");
        let cfg = Config {
            public_url: "http://127.0.0.1:7701".into(),
            data_dir: dir,
            admin_token: None,
            openrouter_key: None,
            anthropic_key: None,
            openai_key: None,
            openai_base: String::new(),
            openai_images_base: String::new(),
            anthropic_base: String::new(),
            daytona_snapshot: String::new(),
            daytona_dockerfile: None,
            bootstrap_dir: None,
            agentd_bin: "arbos-agentd".into(),
            local_box_image: String::new(),
            box_idle_grace_ms: 0,
            default_provider: None,
            fleet: crate::fleet::FleetCfg::default(),
            connectors_mock: mock,
            channels_mock: true,
            image_mock: false,
            avatar_retry_backoff_ms: Vec::new(),
            avatar_attempt_timeout: std::time::Duration::from_secs(40),
            avatar_total_timeout: std::time::Duration::from_secs(75),
            voice_mock: true,
            openai_audio_base: String::new(),
            vault_mock: false,
            auth_mode: crate::state::AuthMode::Paircode,
            github_client_id: None,
            github_client_secret: None,
            auth_redirects: Vec::new(),
            default_grant_usd: 100.0,
            default_daily_cap_usd: 20.0,
            global_daily_cap_usd: 500.0,
            global_machine_daily_cap_usd: 400.0,
            always_on: false,
            machine_usd_per_hour: 0.332,
            machine_stopped_usd_per_day: 0.013,
            releases_repo: "unarbos/arbos".into(),
            site_url: "https://arbos.life".into(),
            avatar_slugs: crate::avatar::DEFAULT_SLUGS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            avatar_colors: crate::avatar::DEFAULT_COLORS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            multi_chat: false,
            openai_direct_base: String::new(),
            openai_route: crate::state::OpenAiRoute::Auto,
        };
        AppState {
            db,
            cfg: Arc::new(cfg),
            http: reqwest::Client::new(),
            daytona: None,
            rooms: Arc::new(RwLock::new(HashMap::new())),
            box_states: Arc::new(RwLock::new(HashMap::new())),
            local_children: Arc::new(Mutex::new(HashMap::new())),
            box_conns: Arc::new(Mutex::new(HashMap::new())),
            wake_started: Arc::new(Mutex::new(HashMap::new())),
            last_client_push: Arc::new(Mutex::new(HashMap::new())),
            stale_bounces: Arc::new(Mutex::new(HashMap::new())),
            conn_seq: Arc::new(AtomicU64::new(0)),
            relay_key: Arc::new(crypto::RelayKey::derive("test-key")),
            jwt_key: Arc::new(crypto::RelayKey::derive("test-key").subkey("jwt")),
            download_cache: Arc::new(Mutex::new(None)),
            upgrading: Arc::new(Mutex::new(HashMap::new())),
            upgrade_refused: Arc::new(Mutex::new(HashMap::new())),
            local_conns: Arc::new(Mutex::new(HashMap::new())),
            refresh_grace: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            push: Arc::new(crate::apns::Pusher::Capture(tokio::sync::Mutex::new(
                Vec::new(),
            ))),
            focus: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            fallback_sent: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            livekit: None,
            presence_seen: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            activity: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            box_http_seen: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            turn_running: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            evicted: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            fleet_pressure: Arc::new(tokio::sync::Mutex::new(None)),
            voice: Arc::new(crate::voice::VoiceCfg::default()),
            voice_conns: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            default_snapshot: Arc::new(std::sync::RwLock::new(String::new())),
            started_at: std::time::Instant::now(),
            restart: Arc::new(crate::state::RestartSignal::default()),
            voice_dispatched: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            voice_worker: Arc::new(tokio::sync::Mutex::new(None)),
            voice_inbox: crate::voice::VoiceInbox::default(),
            image_health: Arc::new(tokio::sync::Mutex::new(None)),
            client_seen: Arc::new(std::sync::Mutex::new(HashMap::new())),
            matrix: crate::matrix::Matrix::new(crate::matrix::Config::off()),
        }
    }

    /// Invariant (F1): a connector token never appears in a `/connectors/call` reply, in the
    /// connector listing, or in the environment a box is started with; credential keys and any
    /// string carrying the token are redacted.
    #[tokio::test]
    async fn token_never_leaves_the_relay() {
        let state = test_state(true);
        let secret = "ghp_SECRETTOKEN1234567890abcdefXYZ";
        let refresh = "refresh-SECRET-0987654321";
        let grant = TokenGrant {
            access: secret.into(),
            refresh: Some(refresh.into()),
            expires_in_s: Some(3600),
            scopes: vec!["repo".into()],
            label: "GitHub · test".into(),
        };
        let cn = store_token(&state, Provider::Github, &grant, json!({})).expect("store");

        // 1. Encrypted at rest: the raw token is not in the row.
        let row = state.db.connector_token(&cn.id).unwrap().unwrap();
        assert!(!row.access_token.contains(secret));
        assert_eq!(state.relay_key.decrypt(&row.access_token).unwrap(), secret);

        // 2. The broker reply (mock path here; the live path runs the same `redact`).
        let reply = broker_call(&state, "github", "list_repos", &json!({}))
            .await
            .expect("call");
        let text = reply.to_string();
        assert!(!text.contains(secret) && !text.contains(refresh), "{text}");

        // 3. The listing.
        let listing = serde_json::to_string(&connector_dto(&cn, true)).unwrap();
        assert!(!listing.contains(secret) && !listing.contains(refresh));

        // 4. Redaction of an upstream reply that echoes credentials in keys and strings.
        let mut echoed = json!({
            "access_token": secret,
            "refresh_token": refresh,
            "Authorization": format!("Bearer {secret}"),
            "nested": { "headers": [ { "name": "authorization", "value": format!("Bearer {secret}") }, format!("token={secret}") ] },
            "fine": "hello",
        });
        redact(&mut echoed, &[secret, refresh]);
        let text = echoed.to_string();
        assert!(!text.contains(secret) && !text.contains(refresh), "{text}");
        assert_eq!(echoed["access_token"], REDACTED);
        assert_eq!(echoed["refresh_token"], REDACTED);
        assert_eq!(echoed["Authorization"], REDACTED);
        assert_eq!(echoed["fine"], "hello");

        // 5. The box environment carries only the box's own relay token.
        let project = crate::db::Project {
            id: "p1".into(),
            name: "p1".into(),
            icon: "P".into(),
            instructions: String::new(),
            models: json!({}),
            state: "awake".into(),
            created_at: 0,
            last_active_at: 0,
            spend_cap_daily: 25.0,
            color: "#000".into(),
            user_id: "u_default".into(),
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
        };
        let env = crate::boxes::box_env(
            &state,
            &project.user_id,
            "abx_box",
            "amc_mcp",
            "http://relay",
        );
        for (k, v) in &env {
            assert!(
                !v.contains(secret) && !v.contains(refresh),
                "{k} leaks a connector token"
            );
        }
    }
}
