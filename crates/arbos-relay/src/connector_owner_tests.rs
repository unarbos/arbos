//! Connector ownership (ARB-connector-owner): a connector holds one person's Gmail, Slack or
//! GitHub credentials, so only its owner may list it, spend it, delete it or overwrite it.
//!
//! Every test here is written as the attack, not as the new behaviour: each one drives one
//! user's principal at another user's connector, and each one passes on the relay that shipped
//! before this fix (the handler bound the caller as `_principal` and the lookups had no owner
//! predicate). They are the regression evidence, so they go through the handlers and raw SQL
//! only — no helper that exists only after the fix — and they still compile against the old
//! connector code.

#![cfg(test)]

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use serde_json::{json, Value};

use arbos_core::ids::{now_ms, ulid};

use crate::auth::Principal;
use crate::connectors::{self, ConnectBody};
use crate::db::Db;
use crate::people::tests_support::person;
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};

async fn body_json(r: Response) -> Value {
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 22)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

/// Connect a provider with a pasted token (mock mode: no upstream call). Returns the new
/// connector's id.
async fn connect_with_token(
    state: &AppState,
    who: &Principal,
    provider: &str,
    token: &str,
) -> String {
    let r = connectors::connect(
        State(state.clone()),
        who.clone(),
        Path(provider.to_owned()),
        Some(Json(ConnectBody {
            token: Some(token.to_owned()),
            base_url: None,
            label: None,
        })),
    )
    .await
    .expect("connect");
    body_json(r).await["connector"]["id"]
        .as_str()
        .expect("connector id")
        .to_owned()
}

async fn list_ids(state: &AppState, who: &Principal) -> Vec<String> {
    let r = connectors::list(State(state.clone()), who.clone())
        .await
        .expect("list");
    body_json(r).await["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|c| c["id"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// The reported finding: `POST /connectors/call` discarded the principal, so any authenticated
/// caller reached the newest connector of a provider — somebody else's Gmail, Slack or GitHub.
#[tokio::test]
async fn another_users_principal_cannot_spend_a_connector() {
    let state = test_state(AuthMode::Github);
    let (_, alice) = person(&state, 9101);
    let (_, bob) = person(&state, 9102);
    connect_with_token(&state, &alice, "github", "ghp_alice_not_real").await;

    let refused = connectors::call(
        State(state.clone()),
        bob.clone(),
        Json(
            serde_json::from_value(json!({ "provider": "github", "action": "list_repos" }))
                .unwrap(),
        ),
    )
    .await;
    let (code, _) = refused.expect_err("bob must not reach alice's github connector");
    assert_eq!(code, StatusCode::NOT_FOUND, "bob has no github connector");

    // Alice's own call still works: the fix refuses the stranger, not the owner.
    let mine = connectors::call(
        State(state.clone()),
        alice,
        Json(
            serde_json::from_value(json!({ "provider": "github", "action": "list_repos" }))
                .unwrap(),
        ),
    )
    .await
    .expect("alice's own call");
    assert_eq!(body_json(mine).await["ok"], json!(true));
}

/// `GET /connectors` listed every connector on the relay, so one user saw who else had
/// connected what (labels included).
#[tokio::test]
async fn the_listing_shows_only_the_callers_connectors() {
    let state = test_state(AuthMode::Github);
    let (_, alice) = person(&state, 9103);
    let (_, bob) = person(&state, 9104);
    let alice_id = connect_with_token(&state, &alice, "github", "ghp_alice_not_real").await;

    assert_eq!(list_ids(&state, &bob).await, Vec::<String>::new());
    assert_eq!(list_ids(&state, &alice).await, vec![alice_id]);
}

/// `DELETE /connectors/{id}` took any id, so one user could disconnect another user's account.
#[tokio::test]
async fn another_user_cannot_delete_a_connector() {
    let state = test_state(AuthMode::Github);
    let (_, alice) = person(&state, 9105);
    let (_, bob) = person(&state, 9106);
    let alice_id = connect_with_token(&state, &alice, "github", "ghp_alice_not_real").await;

    let refused = connectors::disconnect(State(state.clone()), bob, Path(alice_id.clone())).await;
    match refused {
        Err((code, _)) => assert_eq!(code, StatusCode::NOT_FOUND),
        Ok(r) => assert_eq!(
            r.status(),
            StatusCode::NOT_FOUND,
            "bob must not delete alice's connector"
        ),
    }
    assert_eq!(list_ids(&state, &alice).await, vec![alice_id]);
}

/// Connecting looked up the row by provider alone, so a second user's `connect` overwrote the
/// first user's connector — replacing their stored token with one of the attacker's choosing.
#[tokio::test]
async fn another_users_connect_does_not_overwrite_a_connector() {
    let state = test_state(AuthMode::Github);
    let (_, alice) = person(&state, 9107);
    let (_, bob) = person(&state, 9108);
    let alice_id = connect_with_token(&state, &alice, "github", "ghp_alice_not_real").await;
    let bob_id = connect_with_token(&state, &bob, "github", "ghp_bob_not_real").await;

    assert_ne!(alice_id, bob_id, "each user gets their own connector row");
    assert_eq!(list_ids(&state, &alice).await, vec![alice_id]);
    assert_eq!(list_ids(&state, &bob).await, vec![bob_id]);
}

/// The OAuth callback carries no bearer token. It used to attach the returned tokens to
/// whichever connector matched the provider; now the consumed state row says whose flow it was.
#[tokio::test]
async fn the_oauth_callback_credits_the_user_who_started_the_flow() {
    let state = test_state(AuthMode::Github);
    let (_, alice) = person(&state, 9109);
    let (_, bob) = person(&state, 9110);

    let r = connectors::connect(
        State(state.clone()),
        alice.clone(),
        Path("google".into()),
        None,
    )
    .await
    .expect("start oauth");
    let st = body_json(r).await["state"]
        .as_str()
        .expect("state")
        .to_owned();
    let done = connectors::oauth_callback(
        State(state.clone()),
        Query(serde_json::from_value(json!({ "code": "mock", "state": st })).unwrap()),
    )
    .await;
    assert_eq!(done.status(), StatusCode::FOUND, "callback completed");

    assert_eq!(list_ids(&state, &bob).await, Vec::<String>::new());
    let r = connectors::list(State(state.clone()), alice)
        .await
        .expect("list");
    let items = body_json(r).await;
    let google = items["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|c| c["provider"] == json!("google"))
        .expect("alice owns the google connector");
    assert_eq!(google["status"], json!("connected"));
}

/// A connector whose owner is unknown (the ownership migration could not attribute it) fails
/// closed: it is listed to nobody and spends for nobody, so a legacy row cannot become a free
/// pass for whoever asks first.
#[tokio::test]
async fn an_unattributed_connector_works_for_nobody() {
    let state = test_state(AuthMode::Github);
    let (_, alice) = person(&state, 9111);
    let id = ulid();
    state
        .db
        .with(|c| {
            c.execute(
                "INSERT INTO connectors(id, provider, label, scopes, status, config, created_at)
                 VALUES (?1, 'github', 'GitHub · legacy', '[\"repo\"]', 'connected', '{}', ?2)",
                rusqlite::params![id, now_ms()],
            )
        })
        .expect("legacy connector row");

    assert_eq!(list_ids(&state, &alice).await, Vec::<String>::new());
    let refused = connectors::call(
        State(state.clone()),
        alice,
        Json(
            serde_json::from_value(json!({ "provider": "github", "action": "list_repos" }))
                .unwrap(),
        ),
    )
    .await;
    let (code, _) = refused.expect_err("an ownerless connector is nobody's to spend");
    assert_eq!(code, StatusCode::NOT_FOUND);
}

/// The backfill: a relay with exactly one user has only one possible owner for the connectors
/// that exist, so those rows are attributed to that user and keep working over the upgrade.
#[tokio::test]
async fn the_backfill_attributes_rows_on_a_single_user_relay() {
    let dir = std::env::temp_dir().join(format!("arbos-connector-backfill-{}", ulid()));
    let path = dir.join("relay.db");
    legacy_relay(&path, &["u_only"]);

    let db = Db::open(&path).expect("open and migrate");
    assert_eq!(owners_of_connectors(&db), vec![Some("u_only".to_owned())]);
    assert_eq!(owners_of_tokens(&db), vec![Some("u_only".to_owned())]);
}

/// The same backfill on a relay with two users leaves every legacy row unattributed. Either
/// user is plausible and neither is provable, and attaching one user's inbox to the other is
/// worse than a connector that stops until a human says whose it is.
#[tokio::test]
async fn the_backfill_refuses_to_guess_on_a_multi_user_relay() {
    let dir = std::env::temp_dir().join(format!("arbos-connector-backfill-{}", ulid()));
    let path = dir.join("relay.db");
    legacy_relay(&path, &["u_one", "u_two"]);

    let db = Db::open(&path).expect("open and migrate");
    assert_eq!(owners_of_connectors(&db), vec![None]);
    assert_eq!(owners_of_tokens(&db), vec![None]);
}

/// How an unattributed row is recovered: a human names the owner, once. This is the only test
/// here that cannot run against the pre-fix relay, because assigning an owner is new.
#[tokio::test]
async fn an_admin_can_assign_an_unattributed_connector() {
    let dir = std::env::temp_dir().join(format!("arbos-connector-backfill-{}", ulid()));
    let path = dir.join("relay.db");
    legacy_relay(&path, &["u_one", "u_two"]);
    let db = Db::open(&path).expect("open and migrate");

    assert!(db
        .set_connector_owner("conn_legacy", "u_two")
        .expect("assign"));
    assert_eq!(owners_of_connectors(&db), vec![Some("u_two".to_owned())]);
    assert_eq!(owners_of_tokens(&db), vec![Some("u_two".to_owned())]);

    // Assigning again never moves an owned connector to someone else.
    assert!(!db
        .set_connector_owner("conn_legacy", "u_one")
        .expect("reassign"));
    assert_eq!(owners_of_connectors(&db), vec![Some("u_two".to_owned())]);
}

/// A relay database as it looked before connectors had owners: the pre-fix tables, one
/// connector with its token, and `users` rows for the given ids.
fn legacy_relay(path: &std::path::Path, users: &[&str]) {
    std::fs::create_dir_all(path.parent().expect("dir")).expect("mkdir");
    let c = rusqlite::Connection::open(path).expect("open legacy db");
    c.execute_batch(
        "CREATE TABLE users (
           id TEXT PRIMARY KEY, snapshot TEXT, spend_cap_usd REAL NOT NULL DEFAULT 25.0, created_at INTEGER NOT NULL
         );
         CREATE TABLE connectors (
           id TEXT PRIMARY KEY, provider TEXT NOT NULL, label TEXT NOT NULL DEFAULT '', scopes TEXT NOT NULL DEFAULT '[]',
           status TEXT NOT NULL DEFAULT 'needs_auth', config TEXT NOT NULL DEFAULT '{}', created_at INTEGER NOT NULL,
           last_used_at INTEGER, last_error TEXT
         );
         CREATE TABLE connector_tokens (
           connector_id TEXT PRIMARY KEY, access_token TEXT NOT NULL, refresh_token TEXT, expires_at INTEGER,
           token_type TEXT NOT NULL DEFAULT 'Bearer', updated_at INTEGER NOT NULL
         );
         INSERT INTO connectors(id, provider, label, scopes, status, config, created_at)
           VALUES ('conn_legacy', 'google', 'Google · legacy', '[]', 'connected', '{}', 1);
         INSERT INTO connector_tokens(connector_id, access_token, updated_at)
           VALUES ('conn_legacy', 'ciphertext', 1);",
    )
    .expect("legacy schema");
    for u in users {
        c.execute(
            "INSERT INTO users(id, created_at) VALUES (?1, 1)",
            rusqlite::params![u],
        )
        .expect("legacy user");
    }
}

fn owners_of_connectors(db: &Db) -> Vec<Option<String>> {
    owners(db, "SELECT user_id FROM connectors ORDER BY id")
}

fn owners_of_tokens(db: &Db) -> Vec<Option<String>> {
    owners(
        db,
        "SELECT user_id FROM connector_tokens ORDER BY connector_id",
    )
}

fn owners(db: &Db, sql: &str) -> Vec<Option<String>> {
    db.with(|c| {
        let mut st = c.prepare(sql)?;
        let rows = st.query_map([], |r| r.get::<_, Option<String>>(0))?;
        rows.collect()
    })
    .expect("owner column")
}
