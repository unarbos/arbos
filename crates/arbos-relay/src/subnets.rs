//! What every agent can look up about a Bittensor subnet: its name, the repository it names on
//! chain, and a brief on mining it written from that repository's docs.
//!
//! The rows are written by `arbos-subnets`, a program that runs beside the relay and holds
//! `ARBOS_SUBNETS_TOKEN`. That token opens these routes and nothing else on the relay, because
//! its holder reads strangers' repositories; with it unset nothing can be written. Any signed-in
//! device or box reads, through the agent's `bittensor_subnet` tool. A row is public knowledge
//! about a public chain, the same for every user.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::signin::api_error;
use crate::state::AppState;

pub const SUBNETS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS subnets (
  netuid INTEGER PRIMARY KEY, name TEXT NOT NULL, repo TEXT NOT NULL, commit_sha TEXT NOT NULL,
  brief TEXT NOT NULL, updated_at INTEGER NOT NULL
);
"#;

const MAX_BRIEF_CHARS: usize = 8_000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubnetBody {
    pub name: String,
    pub repo: String,
    pub commit: String,
    pub brief: String,
}

impl SubnetBody {
    /// The writer is ours, but it relays chain text and a model's output: hold both to shape.
    fn valid(&self) -> bool {
        let plain = |s: &str| !s.chars().any(char::is_control);
        self.name.chars().count() <= 64
            && plain(&self.name)
            && self.repo.len() <= 250
            && self
                .repo
                .strip_prefix("https://github.com/")
                .is_some_and(|p| {
                    !p.is_empty()
                        && p.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))
                })
            && self.commit.len() == 40
            && self.commit.bytes().all(|b| b.is_ascii_hexdigit())
            && (1..=MAX_BRIEF_CHARS).contains(&self.brief.chars().count())
    }
}

impl crate::db::Db {
    /// Every subnet without its brief, or one with it.
    pub fn subnets(&self, netuid: Option<u16>) -> anyhow::Result<Vec<Value>> {
        self.with(|c| {
            let mut q = c.prepare(
                "SELECT netuid, name, repo, commit_sha, updated_at, brief FROM subnets
                 WHERE ?1 IS NULL OR netuid = ?1 ORDER BY netuid",
            )?;
            let rows = q.query_map(params![netuid], |r| {
                let mut v = json!({
                    "netuid": r.get::<_, u16>(0)?, "name": r.get::<_, String>(1)?,
                    "repo": r.get::<_, String>(2)?, "commit": r.get::<_, String>(3)?,
                    "updated_at": r.get::<_, i64>(4)?,
                });
                if netuid.is_some() {
                    v["brief"] = json!(r.get::<_, String>(5)?);
                }
                Ok(v)
            })?;
            rows.collect::<Result<_, _>>()
        })
    }

    pub fn set_subnet(&self, netuid: u16, b: Option<&SubnetBody>) -> anyhow::Result<()> {
        self.with(|c| {
            match b {
                Some(b) => c.execute(
                    "INSERT INTO subnets(netuid, name, repo, commit_sha, brief, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(netuid) DO UPDATE SET name = excluded.name, repo = excluded.repo,
                       commit_sha = excluded.commit_sha, brief = excluded.brief,
                       updated_at = excluded.updated_at",
                    params![
                        netuid,
                        b.name,
                        b.repo,
                        b.commit,
                        b.brief,
                        chrono::Utc::now().timestamp_millis()
                    ],
                )?,
                None => c.execute("DELETE FROM subnets WHERE netuid = ?1", params![netuid])?,
            };
            Ok(())
        })
    }
}

/// Whether `headers` carry the writer's token. Compared as digests, so the time taken says
/// nothing about how much of a guess was right.
fn is_writer(configured: Option<&str>, headers: &HeaderMap) -> bool {
    match (configured, crate::auth::bearer_from_header(headers)) {
        (Some(want), Some(got)) if !want.is_empty() => {
            Sha256::digest(want.as_bytes()) == Sha256::digest(got.as_bytes())
        }
        _ => false,
    }
}

fn writer(headers: &HeaderMap) -> bool {
    is_writer(
        std::env::var("ARBOS_SUBNETS_TOKEN").ok().as_deref(),
        headers,
    )
}

/// A reader is the writer or anyone the relay knows. The header only: no WebSocket dials here.
fn reader(state: &AppState, headers: &HeaderMap) -> bool {
    writer(headers)
        || crate::auth::bearer_from_header(headers)
            .is_some_and(|t| crate::auth::resolve_checked(state, &t).is_ok())
}

fn refused() -> Response {
    api_error(StatusCode::UNAUTHORIZED, "unauthorized", "not signed in")
}

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!(error = %e, "subnet storage");
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "db",
        "something went wrong",
    )
}

/// `GET /subnets` → `{subnets: [{netuid, name, repo, commit, updated_at}]}`.
pub async fn list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !reader(&state, &headers) {
        return refused();
    }
    match state.db.subnets(None) {
        Ok(rows) => Json(json!({ "subnets": rows })).into_response(),
        Err(e) => internal(e),
    }
}

/// `GET /subnets/{netuid}` → the row with its `brief`, or 404.
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(netuid): Path<u16>,
) -> Response {
    if !reader(&state, &headers) {
        return refused();
    }
    match state.db.subnets(Some(netuid)).map(|mut rows| rows.pop()) {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "not_found", "no such subnet"),
        Err(e) => internal(e),
    }
}

/// `PUT /subnets/{netuid} {name, repo, commit, brief}` → 204. The writer only.
pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(netuid): Path<u16>,
    Json(b): Json<SubnetBody>,
) -> Response {
    if !writer(&headers) {
        return refused();
    }
    if !b.valid() {
        return api_error(StatusCode::BAD_REQUEST, "bad_subnet", "not a subnet row");
    }
    match state.db.set_subnet(netuid, Some(&b)) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal(e),
    }
}

/// `DELETE /subnets/{netuid}` → 204. The writer only.
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(netuid): Path<u16>,
) -> Response {
    if !writer(&headers) {
        return refused();
    }
    match state.db.set_subnet(netuid, None) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bearer(t: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("authorization", format!("Bearer {t}").parse().unwrap());
        h
    }

    fn row() -> SubnetBody {
        SubnetBody {
            name: "lium.io".into(),
            repo: "https://github.com/Datura-ai/lium-io".into(),
            commit: "a".repeat(40),
            brief: "## What the subnet does".into(),
        }
    }

    #[test]
    fn only_the_configured_token_writes_and_an_unset_one_never_does() {
        assert!(is_writer(Some("s3cret"), &bearer("s3cret")));
        assert!(!is_writer(Some("s3cret"), &bearer("s3cre")));
        assert!(!is_writer(Some("s3cret"), &HeaderMap::new()));
        assert!(!is_writer(None, &bearer("s3cret")));
        assert!(!is_writer(Some(""), &bearer("")));
    }

    #[test]
    fn a_row_is_held_to_its_shape() {
        assert!(row().valid());
        let bad = [
            SubnetBody {
                name: "a\nb".into(),
                ..row()
            },
            SubnetBody {
                name: "n".repeat(65),
                ..row()
            },
            SubnetBody {
                repo: "https://evil.io/o/r".into(),
                ..row()
            },
            SubnetBody {
                repo: "https://github.com/o/r?x".into(),
                ..row()
            },
            SubnetBody {
                commit: "abc".into(),
                ..row()
            },
            SubnetBody {
                commit: "z".repeat(40),
                ..row()
            },
            SubnetBody {
                brief: String::new(),
                ..row()
            },
            SubnetBody {
                brief: "b".repeat(MAX_BRIEF_CHARS + 1),
                ..row()
            },
        ];
        for b in bad {
            assert!(!b.valid(), "{b:?}");
        }
    }

    #[tokio::test]
    async fn anyone_the_relay_knows_reads_what_only_the_writer_may_change() {
        let state = crate::state::test_support::test_state(crate::state::AuthMode::Github);
        // The admin token stands for any caller the relay knows. It is not the writer.
        let known = bearer("adm_test");
        let put_as = |h: HeaderMap| put(State(state.clone()), h, Path(51), Json(row()));
        assert_eq!(
            put_as(known.clone()).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert!(state.db.subnets(None).unwrap().is_empty());
        state.db.set_subnet(51, Some(&row())).unwrap();
        state.db.set_subnet(51, Some(&row())).unwrap();
        let get_as = |h: HeaderMap, n: u16| get(State(state.clone()), h, Path(n));
        assert_eq!(
            get_as(HeaderMap::new(), 51).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            get_as(bearer("nobody"), 51).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(get_as(known.clone(), 51).await.status(), StatusCode::OK);
        assert_eq!(
            get_as(known.clone(), 52).await.status(),
            StatusCode::NOT_FOUND
        );
        let listed = state.db.subnets(None).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(
            listed[0].get("brief").is_none(),
            "the list carries no brief"
        );
        assert_eq!(state.db.subnets(Some(51)).unwrap()[0]["brief"], row().brief);
        let gone = delete(State(state.clone()), known, Path(51)).await;
        assert_eq!(gone.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(state.db.subnets(None).unwrap().len(), 1);
        state.db.set_subnet(51, None).unwrap();
        assert!(state.db.subnets(None).unwrap().is_empty());
    }
}
