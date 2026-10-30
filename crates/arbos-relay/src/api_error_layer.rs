//! One response layer: plain-text `ApiResult` errors become [`ApiErrorDto`].
//!
//! Handlers that still return `Err((StatusCode, String))` stay untouched; this layer rewrites
//! the wire body when the response is 4xx/5xx `text/plain`. JSON, HTML, binary, `/proxy/` and
//! `/_matrix/` passthroughs, and browser invite pages are left alone.

use std::time::Instant;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;

use arbos_core::api::{ApiErrorBodyDto, ApiErrorDto};

use crate::auth::Principal;
use crate::state::AppState;

/// Max bytes we will read from a plain-text error body before giving up and passing it through.
const PLAIN_ERROR_BODY_LIMIT: usize = 1024 * 1024;

/// Map an HTTP status to the `error.type` string used when converting a plain-text body.
pub(crate) fn type_for_status(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 => "bad_request",
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not_found",
        409 => "conflict",
        410 => "gone",
        413 => "too_large",
        429 => "too_many_requests",
        503 => "unavailable",
        code if (500..600).contains(&code) => "internal",
        code if (400..500).contains(&code) => "bad_request",
        _ => "internal",
    }
}

/// Paths whose bodies must stay as the upstream (or browser) sent them.
fn is_passthrough_path(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    path.starts_with("/proxy/")
        || path == "/proxy"
        || path.starts_with("/_matrix/")
        || path == "/_matrix"
        // Invite landings are pages meant for browsers (`/i/…`, `/s/…`).
        || path.starts_with("/i/")
        || path == "/i"
        || path.starts_with("/s/")
        || path == "/s"
}

fn content_type_is_plain_text(res: &Response) -> bool {
    res.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/plain"))
}

/// Rewrite a finished response when it is a plain-text 4xx/5xx on a normal API path; also
/// returns the plain-text message it converted (for the request log).
pub(crate) async fn convert_plain_error(path: &str, res: Response) -> (Response, Option<String>) {
    let status = res.status();
    if !(status.is_client_error() || status.is_server_error()) {
        return (res, None);
    }
    if is_passthrough_path(path) || !content_type_is_plain_text(&res) {
        return (res, None);
    }

    let (mut parts, body) = res.into_parts();
    let bytes = match axum::body::to_bytes(body, PLAIN_ERROR_BODY_LIMIT).await {
        Ok(b) => b,
        Err(_) => {
            // Body was too large or broken — restore an empty plain response with the same status.
            parts.headers.insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("text/plain"),
            );
            return (Response::from_parts(parts, Body::empty()), None);
        }
    };
    let message = String::from_utf8_lossy(&bytes).into_owned();
    let dto = ApiErrorDto {
        error: ApiErrorBodyDto {
            type_: type_for_status(status).to_owned(),
            message: message.clone(),
        },
    };
    let mut out = (status, Json(dto)).into_response();
    // Keep every other original header; only content-type and length change.
    for (name, value) in parts.headers.iter() {
        if name != header::CONTENT_TYPE && name != header::CONTENT_LENGTH {
            out.headers_mut().insert(name.clone(), value.clone());
        }
    }
    (out, Some(message))
}

/// A request slower than this is logged (`slow request`), whatever its status.
pub(crate) const SLOW_REQUEST_MS: u128 = 5_000;

/// Routes that are slow by design — long-polls and streams — and never make a `slow request`
/// line: the Matrix client sync (sliding sync waits up to 30 s), the voice worker's call inbox
/// (≤ 25 s), inference proxy streams, and the WebSocket upgrades (`/room`, `/local`), whose
/// response is the socket's lifetime. The first hour on the relay was 62 lines of `/_matrix/
/// client/…/sync` at 30 005 ms and nothing else.
fn slow_by_design(path: &str) -> bool {
    path.starts_with("/_matrix/")
        || path.starts_with("/proxy/")
        || path == "/proxy"
        || path.starts_with("/room/")
        || path.starts_with("/local/")
        || (path.starts_with("/voice/calls/") && path.ends_with("/inbox"))
}

/// Who made a request, for the log: `user:<id>` / `box:<user>` / `admin`, `-` when the token
/// resolves to nothing. Resolved only for the few requests that get a line.
fn caller_label(state: &AppState, token: Option<&str>) -> String {
    match token.and_then(|t| crate::auth::resolve_checked(state, t).ok()) {
        Some(Principal::Device { user_id, .. }) => format!("user:{user_id}"),
        Some(Principal::Box { user_id }) => format!("box:{user_id}"),
        Some(Principal::Admin) => "admin".to_owned(),
        None => "-".to_owned(),
    }
}

/// Axum middleware: run the inner service, then maybe rewrite a plain-text error body. Every
/// 5xx and every request over [`SLOW_REQUEST_MS`] gets one journal line with method, path,
/// status, latency and caller — the relay keeps no access log, and a `/box/projects` 500 or
/// stall used to leave no trace (#2120).
pub(crate) async fn rewrite_plain_api_errors(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let token = crate::auth::token_from_parts_ref(&req);
    let res = next.run(req).await;
    let status = res.status();
    let (res, message) = convert_plain_error(&path, res).await;
    let ms = started.elapsed().as_millis();
    if status.is_server_error() {
        tracing::error!(
            %method, %path, status = status.as_u16(), ms,
            caller = %caller_label(&state, token.as_deref()),
            error = message.as_deref().unwrap_or("-"),
            "request failed"
        );
    } else if ms > SLOW_REQUEST_MS && !slow_by_design(&path) {
        tracing::warn!(
            %method, %path, status = status.as_u16(), ms,
            caller = %caller_label(&state, token.as_deref()),
            "slow request"
        );
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use serde_json::{json, Value};

    use crate::people::tests_support::person;
    use crate::signin::api_error;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;
    use crate::{api, goals, routines};

    fn fixtures_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/api")
    }

    fn load_fixture(name: &str) -> Value {
        let path = fixtures_dir().join(name);
        let raw =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        serde_json::from_str(&raw).expect("fixture json")
    }

    fn assert_fixture(name: &str, got: Value) {
        let path = fixtures_dir().join(name);
        if !path.exists() || std::env::var_os("UPDATE_API_FIXTURES").is_some() {
            let pretty = serde_json::to_string_pretty(&got).expect("pretty");
            std::fs::write(&path, format!("{pretty}\n")).expect("write fixture");
        }
        assert_eq!(got, load_fixture(name), "fixture {name}");
    }

    async fn body_json(res: Response) -> Value {
        let bytes = axum::body::to_bytes(res.into_body(), PLAIN_ERROR_BODY_LIMIT)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("json")
    }

    async fn body_text(res: Response) -> String {
        let bytes = axum::body::to_bytes(res.into_body(), PLAIN_ERROR_BODY_LIMIT)
            .await
            .expect("body");
        String::from_utf8(bytes.to_vec()).expect("utf8")
    }

    async fn through_layer(path: &str, res: Response) -> Response {
        convert_plain_error(path, res).await.0
    }

    #[tokio::test]
    async fn plain_text_from_api_goals_routines_become_api_error_dto() {
        let state = test_state(AuthMode::Github);
        let (_alice, dev) = person(&state, 1);

        // api.rs: missing project → plain "no such project"
        let api_err = api::get_project(
            axum::extract::State(state.clone()),
            dev.clone(),
            axum::extract::Path("missing-project".into()),
        )
        .await
        .expect_err("missing project");
        assert_eq!(api_err.0, StatusCode::NOT_FOUND);
        assert_eq!(api_err.1, "no such project");
        let converted = through_layer("/projects/missing-project", api_err.into_response()).await;
        assert_eq!(converted.status(), StatusCode::NOT_FOUND);
        let v = body_json(converted).await;
        assert_eq!(
            v,
            json!({"error": {"type": "not_found", "message": "no such project"}})
        );
        assert_fixture("api_error_converted.json", v);

        // goals.rs: missing project → same plain shape
        let goals_err = goals::get(
            axum::extract::State(state.clone()),
            dev.clone(),
            axum::extract::Path(("no-project".into(), "no-goal".into())),
        )
        .await
        .expect_err("missing project");
        assert_eq!(goals_err.0, StatusCode::NOT_FOUND);
        assert_eq!(goals_err.1, "no such project");
        let converted = through_layer(
            "/projects/no-project/goals/no-goal",
            goals_err.into_response(),
        )
        .await;
        assert_eq!(converted.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            body_json(converted).await,
            json!({"error": {"type": "not_found", "message": "no such project"}})
        );

        // routines.rs: missing project → plain "no such project"
        let routines_err = routines::list(
            axum::extract::State(state),
            dev,
            axum::extract::Path("no-project".into()),
        )
        .await
        .expect_err("missing project");
        assert_eq!(routines_err.0, StatusCode::NOT_FOUND);
        assert_eq!(routines_err.1, "no such project");
        let converted = through_layer(
            "/projects/no-project/routines",
            routines_err.into_response(),
        )
        .await;
        assert_eq!(converted.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            body_json(converted).await,
            json!({"error": {"type": "not_found", "message": "no such project"}})
        );
    }

    #[tokio::test]
    async fn existing_api_error_json_is_unchanged() {
        let want = json!({
            "error": {
                "type": "token_expired",
                "message": "access token expired; refresh it"
            }
        });
        let original = api_error(
            StatusCode::UNAUTHORIZED,
            "token_expired",
            "access token expired; refresh it",
        );
        assert_eq!(original.status(), StatusCode::UNAUTHORIZED);
        let ct = original
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        assert!(ct.starts_with("application/json"), "{ct}");
        let converted = through_layer("/me", original).await;
        assert_eq!(converted.status(), StatusCode::UNAUTHORIZED);
        let ct2 = converted
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        assert!(ct2.starts_with("application/json"), "{ct2}");
        assert_eq!(body_json(converted).await, want);
    }

    #[tokio::test]
    async fn proxy_plain_error_from_upstream_is_unchanged() {
        let upstream = (StatusCode::BAD_GATEWAY, "upstream: connection reset").into_response();
        let converted = through_layer("/proxy/v1/chat/completions", upstream).await;
        assert_eq!(converted.status(), StatusCode::BAD_GATEWAY);
        assert!(content_type_is_plain_text(&converted));
        assert_eq!(body_text(converted).await, "upstream: connection reset");
    }

    #[tokio::test]
    async fn ok_text_response_is_unchanged() {
        let ok = (StatusCode::OK, "hello plain").into_response();
        let converted = through_layer("/healthz", ok).await;
        assert_eq!(converted.status(), StatusCode::OK);
        assert_eq!(body_text(converted).await, "hello plain");
    }

    #[test]
    fn long_polls_and_streams_are_slow_by_design() {
        assert!(slow_by_design(
            "/_matrix/client/unstable/org.matrix.simplified_msc3575/sync"
        ));
        assert!(slow_by_design("/voice/calls/01ABC/inbox"));
        assert!(slow_by_design("/proxy/v1/chat/completions"));
        assert!(slow_by_design("/room/people/01ABC"));
        assert!(!slow_by_design("/box/projects"));
        assert!(!slow_by_design("/voice/calls/01ABC/events"));
        assert!(!slow_by_design("/chats/01ABC/messages"));
    }

    #[test]
    fn status_type_mapping() {
        assert_eq!(type_for_status(StatusCode::BAD_REQUEST), "bad_request");
        assert_eq!(type_for_status(StatusCode::UNAUTHORIZED), "unauthorized");
        assert_eq!(type_for_status(StatusCode::FORBIDDEN), "forbidden");
        assert_eq!(type_for_status(StatusCode::NOT_FOUND), "not_found");
        assert_eq!(type_for_status(StatusCode::CONFLICT), "conflict");
        assert_eq!(type_for_status(StatusCode::GONE), "gone");
        assert_eq!(type_for_status(StatusCode::PAYLOAD_TOO_LARGE), "too_large");
        assert_eq!(
            type_for_status(StatusCode::TOO_MANY_REQUESTS),
            "too_many_requests"
        );
        assert_eq!(
            type_for_status(StatusCode::SERVICE_UNAVAILABLE),
            "unavailable"
        );
        assert_eq!(
            type_for_status(StatusCode::INTERNAL_SERVER_ERROR),
            "internal"
        );
        assert_eq!(type_for_status(StatusCode::BAD_GATEWAY), "internal");
        assert_eq!(type_for_status(StatusCode::PAYMENT_REQUIRED), "bad_request");
    }
}
