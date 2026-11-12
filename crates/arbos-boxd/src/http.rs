//! The HTTP surface (docs/box-scaling.md §4.1), for the relay on the same host.
//!
//! Every request needs the bearer token. A request that fails for any reason to do with who
//! is asking gets the same bare 401.

use std::collections::BTreeMap;
use std::future::Future;
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::boxes::{valid_env, valid_version, Boxd, Error, UserId};

/// Shorter than this is not a secret anyone generated.
const MIN_TOKEN_LEN: usize = 32;

/// Read the bearer token. The file must be a plain file nobody but its owner can read.
pub fn load_token(path: &str) -> Result<String> {
    let md = std::fs::symlink_metadata(path).with_context(|| format!("token file {path}"))?;
    if !md.is_file() || md.mode() & 0o077 != 0 {
        bail!("token file {path} must be a plain file with mode 0600");
    }
    let token = std::fs::read_to_string(path)?.trim().to_owned();
    if token.len() < MIN_TOKEN_LEN {
        bail!("token file {path} holds fewer than {MIN_TOKEN_LEN} characters");
    }
    Ok(token)
}

#[derive(Clone)]
struct App {
    boxd: Arc<Boxd>,
    token: [u8; 32],
}

/// Digests are compared, and every byte of them, so neither the token's length nor where a
/// guess first differs shows in the time this takes.
fn authed(token: &[u8; 32], headers: &HeaderMap) -> bool {
    let given = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let given = Sha256::digest(given.as_bytes());
    given.iter().zip(token).fold(0, |acc, (a, b)| acc | (a ^ b)) == 0
}

async fn auth(State(app): State<App>, req: Request, next: Next) -> Response {
    if !authed(&app.token, req.headers()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(req).await
}

fn invalid() -> Response {
    let body = Json(json!({ "error": "invalid request" }));
    (StatusCode::BAD_REQUEST, body).into_response()
}

/// Run one operation to its end and answer with it. Spawned, because axum drops a handler
/// whose caller hung up, and a box must not be left half asleep.
async fn finish(op: impl Future<Output = Result<Value, Error>> + Send + 'static) -> Response {
    match tokio::spawn(op).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => {
            if e.status.is_server_error() {
                tracing::error!(status = %e.status, error = %e.msg, "request failed");
            }
            (e.status, Json(json!({ "error": e.msg }))).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "request panicked");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnsureReq {
    /// The `arbos-agent` release, a directory under `images/`.
    version: String,
    /// The box's environment, the relay's `boxes::box_env`.
    #[serde(default)]
    env: BTreeMap<String, String>,
}

async fn ensure(
    State(app): State<App>,
    Path(user): Path<String>,
    body: Result<Json<EnsureReq>, JsonRejection>,
) -> Response {
    let (Some(user), Ok(Json(req))) = (UserId::parse(&user), body) else {
        return invalid();
    };
    if !valid_version(&req.version) || !valid_env(&req.env) {
        return invalid();
    }
    finish(async move { app.boxd.ensure(&user, &req.version, &req.env).await }).await
}

async fn wake(State(app): State<App>, Path(user): Path<String>) -> Response {
    let Some(user) = UserId::parse(&user) else {
        return invalid();
    };
    finish(async move { app.boxd.wake(&user).await }).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SleepReq {
    /// Stop the box without asking agentd, and keep no checkpoint.
    #[serde(default)]
    force: bool,
}

async fn sleep(
    State(app): State<App>,
    Path(user): Path<String>,
    query: Result<Query<SleepReq>, QueryRejection>,
) -> Response {
    let (Some(user), Ok(Query(req))) = (UserId::parse(&user), query) else {
        return invalid();
    };
    finish(async move { app.boxd.sleep(&user, req.force).await }).await
}

async fn grow(State(app): State<App>, Path(user): Path<String>) -> Response {
    let Some(user) = UserId::parse(&user) else {
        return invalid();
    };
    finish(async move { app.boxd.grow(&user).await }).await
}

async fn status(State(app): State<App>) -> Response {
    finish(async move { app.boxd.status().await }).await
}

pub fn router(boxd: Arc<Boxd>, token: &str) -> Router {
    let app = App {
        boxd,
        token: Sha256::digest(token.as_bytes()).into(),
    };
    Router::new()
        .route("/v1/boxes/{user_id}/ensure", post(ensure))
        .route("/v1/boxes/{user_id}/wake", post(wake))
        .route("/v1/boxes/{user_id}/sleep", post(sleep))
        .route("/v1/boxes/{user_id}/grow", post(grow))
        .route("/v1/host/status", get(status))
        // An ensure body is a version and a few dozen short variables.
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(axum::middleware::from_fn_with_state(app.clone(), auth))
        .with_state(app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boxes::tests::boxd;
    use std::os::unix::fs::PermissionsExt;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    /// The router on a loopback port, and a client for it.
    async fn served(tag: &str) -> (String, reqwest::Client, String) {
        let (boxd, _fake, root) = boxd(tag).await;
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, router(boxd, TOKEN)).await });
        (url, reqwest::Client::new(), root)
    }

    /// No token, a wrong one, a prefix of the right one, the wrong scheme: all the same 401
    /// with nothing in it, on every route, known or not.
    #[tokio::test]
    async fn every_route_needs_the_bearer_and_fails_the_same_way() {
        let (url, http, root) = served("auth").await;
        let headers = [
            None,
            Some("Bearer wrong".to_owned()),
            Some(format!("Bearer {}", &TOKEN[..31])),
            Some(format!("Bearer {TOKEN}x")),
            Some(format!("Basic {TOKEN}")),
            Some("Bearer ".to_owned()),
        ];
        for path in ["/v1/host/status", "/v1/boxes/u_a/wake", "/nothing-here"] {
            for h in &headers {
                let mut req = http.post(format!("{url}{path}"));
                if path.ends_with("status") {
                    req = http.get(format!("{url}{path}"));
                }
                if let Some(h) = h {
                    req = req.header("authorization", h);
                }
                let r = req.send().await.unwrap();
                assert_eq!(r.status(), 401, "{path} {h:?}");
                assert_eq!(r.text().await.unwrap(), "", "{path} {h:?}");
            }
        }
        let ok = http.get(format!("{url}/v1/host/status")).bearer_auth(TOKEN);
        assert_eq!(ok.send().await.unwrap().status(), 200);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A user id outside the allowlist, a body with a field nobody defined, a version or a
    /// variable name that is not one: 400, and no box directory appears.
    #[tokio::test]
    async fn a_bad_id_or_body_is_refused_before_it_touches_a_path() {
        let (url, http, root) = served("validate").await;
        let post = |path: &str, body: Value| {
            let req = http.post(format!("{url}{path}")).bearer_auth(TOKEN);
            async move { req.json(&body).send().await.unwrap().status().as_u16() }
        };
        let good = json!({ "version": "1.0", "env": { "A": "b" } });
        // `..` itself, encoded or not, is folded away by the client before it is sent.
        for id in ["a%2fb", "a.b", "a%00", "%2e%2ex"] {
            let path = format!("/v1/boxes/{id}/ensure");
            assert_eq!(post(&path, good.clone()).await, 400, "{id}");
            assert_eq!(post(&format!("/v1/boxes/{id}/wake"), json!({})).await, 400);
        }
        for body in [
            json!({ "version": "1.0", "extra": 1 }),
            json!({ "version": "../1.0" }),
            json!({ "version": "1.0", "env": { "A=B": "c" } }),
            json!({ "version": "1.0", "env": { "A": 1 } }),
            json!({ "env": {} }),
            json!("1.0"),
        ] {
            assert_eq!(
                post("/v1/boxes/u_a/ensure", body.clone()).await,
                400,
                "{body}"
            );
        }
        assert_eq!(
            std::fs::read_dir(format!("{root}/boxes")).unwrap().count(),
            0
        );

        assert_eq!(post("/v1/boxes/u_a/ensure", good).await, 200);
        assert_eq!(post("/v1/boxes/u_a/wake", json!({})).await, 200);
        assert_eq!(post("/v1/boxes/u_b/sleep", json!({})).await, 404);
        // `force` is true, false or absent; anything else in the query stops nothing.
        for query in [
            "force=yes",
            "force=1",
            "force",
            "force=true&x=1",
            "now=true",
        ] {
            let path = format!("/v1/boxes/u_a/sleep?{query}");
            assert_eq!(post(&path, json!({})).await, 400, "{query}");
        }
        assert_eq!(post("/v1/boxes/u_a/sleep?force=true", json!({})).await, 200);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The daemon does not start on a token file others can read, a link, or a short token.
    #[test]
    fn the_token_file_must_be_private() {
        let dir = std::env::temp_dir().join(format!("arbos-boxd-token-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("token");
        let p = path.to_str().unwrap();
        let write = |text: &str, mode: u32| {
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        write(&format!("{TOKEN}\n"), 0o600);
        assert_eq!(load_token(p).unwrap(), TOKEN);
        write(TOKEN, 0o640);
        assert!(load_token(p).is_err(), "group can read it");
        write("short", 0o600);
        assert!(load_token(p).is_err());
        write(TOKEN, 0o600);
        let link = dir.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(load_token(link.to_str().unwrap()).is_err());
        assert!(load_token(dir.join("missing").to_str().unwrap()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
