//! Bearer-token principals (D35): paired devices (each belongs to a user), boxes (one per user,
//! serving every project of that user), the admin token.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::Response;

use arbos_core::auth::hash;

use crate::state::AppState;

#[derive(Debug, Clone)]
pub enum Principal {
    Device {
        id: String,
        token: String,
        user_id: String,
    },
    Box {
        user_id: String,
    },
    Admin,
}

impl Principal {
    pub fn device_name(&self) -> String {
        match self {
            Principal::Device { id, .. } => format!("dev:{id}"),
            Principal::Box { user_id } => format!("box:{user_id}"),
            Principal::Admin => "admin".to_owned(),
        }
    }

    /// A project is reachable by the admin, by every device of its user, and by its user's box.
    /// (Devices of other users are refused once more than one user exists on a relay.)
    pub fn can_access_project(&self, project_user_id: &str) -> bool {
        match self {
            Principal::Admin => true,
            Principal::Device { user_id, .. } | Principal::Box { user_id } => {
                user_id == project_user_id
            }
        }
    }

    /// The user this principal acts for (devices and boxes); the admin has none.
    pub fn user_id(&self) -> Option<&str> {
        match self {
            Principal::Device { user_id, .. } | Principal::Box { user_id } => Some(user_id),
            Principal::Admin => None,
        }
    }

    pub fn is_box(&self) -> bool {
        matches!(self, Principal::Box { .. })
    }

    /// The device's own bearer token (for URLs the relay hands back to that device only).
    pub fn raw_token(&self) -> Option<&str> {
        match self {
            Principal::Device { token, .. } => Some(token),
            Principal::Box { .. } | Principal::Admin => None,
        }
    }
}

/// Why a bearer was refused; `Expired` tells clients to refresh instead of signing in again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Missing,
    Invalid,
    Expired,
}

impl Refused {
    fn kind(self) -> &'static str {
        match self {
            Refused::Missing => "missing_token",
            Refused::Invalid => "invalid_token",
            Refused::Expired => "token_expired",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Refused::Missing => "missing token",
            Refused::Invalid => "invalid token",
            Refused::Expired => "access token expired; refresh it",
        }
    }
}

/// Resolve a raw token string to a principal, or say why not.
pub fn resolve_checked(state: &AppState, token: &str) -> Result<Principal, Refused> {
    if token.is_empty() {
        return Err(Refused::Missing);
    }
    if let Some(admin) = &state.cfg.admin_token {
        if token == admin {
            return Ok(Principal::Admin);
        }
    }
    if crate::signin::looks_like_jwt(token) {
        // Access tokens are bound to a device row: a revoked device's tokens die with it even
        // before they expire.
        return match crate::signin::verify_access(&state.jwt_key, token) {
            Ok(claims) => match state.db.device_status(&claims.device_id) {
                Ok(Some((user_id, false))) if user_id == claims.user_id => {
                    let _ = state.db.touch_device(&claims.device_id);
                    Ok(Principal::Device {
                        id: claims.device_id,
                        token: token.to_owned(),
                        user_id,
                    })
                }
                _ => Err(Refused::Invalid),
            },
            Err(crate::signin::AccessError::Expired) => Err(Refused::Expired),
            Err(crate::signin::AccessError::Invalid) => Err(Refused::Invalid),
        };
    }
    resolve_legacy(state, token).ok_or(Refused::Invalid)
}

/// The account id a device row carries when nothing gave it one: the column default
/// (`devices.user_id`, `db.rs`).
///
/// It means two different things depending on the relay. On a paircode relay it is *the*
/// account — one self-hosted relay serves one person, every paired machine is a session of it,
/// and the projects table defaults the same way. On a GitHub relay it is a legacy bucket: rows
/// created before the column existed were left in it, and those rows can belong to different
/// people. Anything that treats the bucket as an account has to know which relay it is on.
pub const LEGACY_USER: &str = "u_default";

/// `adv_` device tokens and `abx_` box tokens (hash lookup).
fn resolve_legacy(state: &AppState, token: &str) -> Option<Principal> {
    let h = hash(token);
    if let Ok(Some(id)) = state.db.device_by_token_hash(&h) {
        let user_id = state
            .db
            .device_user(&id)
            .ok()
            .flatten()
            .unwrap_or_else(|| LEGACY_USER.to_owned());
        return Some(Principal::Device {
            id,
            token: token.to_owned(),
            user_id,
        });
    }
    if let Ok(Some(user_id)) = state.db.user_by_box_token_hash(&h) {
        let _ = state.db.touch_box(&user_id);
        return Some(Principal::Box { user_id });
    }
    None
}

/// Token from `Authorization: Bearer …` or `?token=` (WebSocket dials, crdt-sync §10.2).
pub fn token_from_parts(parts: &Parts) -> Option<String> {
    token_from(&parts.headers, &parts.uri)
}

/// Token from `Authorization: Bearer …` and nowhere else.
///
/// A token in a query string is written down by everything it passes: the relay's access log,
/// Caddy's, any proxy in between, the `Referer` of the next request, and the shell history of
/// whoever typed the `curl`. `?token=` exists because a WebSocket dial cannot carry a header
/// (crdt-sync §10.2), and [`token_from`] offers it on every route rather than on the dials that
/// need it. A route with no WebSocket upgrade has no reason to accept it.
pub fn bearer_from_header(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
}

/// [`token_from_parts`] for a whole request (the request layer, before the handler runs).
pub fn token_from_parts_ref(req: &axum::extract::Request) -> Option<String> {
    token_from(req.headers(), req.uri())
}

fn token_from(headers: &axum::http::HeaderMap, uri: &axum::http::Uri) -> Option<String> {
    if let Some(v) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(t) = v.strip_prefix("Bearer ") {
            return Some(t.trim().to_owned());
        }
    }
    let q = uri.query()?;
    q.split('&')
        .find_map(|kv| kv.strip_prefix("token=").map(|t| t.to_owned()))
}

impl FromRequestParts<AppState> for Principal {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let tok = token_from_parts(parts).unwrap_or_default();
        let principal = resolve_checked(state, &tok).map_err(|r| {
            crate::signin::api_error(StatusCode::UNAUTHORIZED, r.kind(), r.message())
        })?;
        // A read-only device (`kind = reader`, docs/grok-bot-read-access.md) may only list and
        // export chats; every other route is refused before the handler runs.
        if let Principal::Device { id, .. } = &principal {
            let is_reader = state
                .db
                .device_kind(id)
                .ok()
                .flatten()
                .is_some_and(|k| k == crate::export::READER_KIND);
            if is_reader && !crate::export::reader_may(parts.method.as_str(), parts.uri.path()) {
                return Err(crate::signin::api_error(
                    StatusCode::FORBIDDEN,
                    "reader_only",
                    "this token can only read chats",
                ));
            }
        }
        // What the app runs (`X-Arbos-Client`): the "installed" signal for continuous delivery.
        if let Principal::Device { id, user_id, .. } = &principal {
            if let Some(h) = parts
                .headers
                .get("x-arbos-client")
                .and_then(|v| v.to_str().ok())
            {
                crate::releases::note_client(state, id, user_id, h);
            }
        }
        Ok(principal)
    }
}

/// [`Principal`], but the bearer must arrive in the `Authorization` header: a `?token=` query
/// string is refused rather than honoured. Use it on any route that does not upgrade to a
/// WebSocket, and on every route whose token is worth more than one request —
/// `POST /auth/sign-out-everywhere` can empty an account, and a bearer that can do that does
/// not belong in a log line.
pub struct HeaderPrincipal(pub Principal);

impl FromRequestParts<AppState> for HeaderPrincipal {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if bearer_from_header(&parts.headers).is_none() {
            return Err(crate::signin::api_error(
                StatusCode::UNAUTHORIZED,
                "missing token",
                "send the token in an Authorization: Bearer header; this route refuses ?token=",
            ));
        }
        // The header is present, and `token_from` prefers it, so whatever is in the query
        // string cannot be what gets resolved.
        Ok(Self(Principal::from_request_parts(parts, state).await?))
    }
}
