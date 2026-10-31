//! An invited account arrives with a name nobody chose, and the wire says so.
//!
//! A person invited to Arbos never reaches a sign-up form. `people::resolve_via_github` asks
//! GitHub about the login being invited and `db_people::upsert_invited_user` stores the
//! profile's `name` and `login` on a `status = 'invited'` row, so the account exists — named —
//! before its owner has typed anything. First sign-in (`db_accounts::upsert_github_user`)
//! deliberately keeps that row's name instead of refreshing it, which is right for the name
//! and wrong for the client: `/me` then returns a populated `name` that reads exactly like one
//! the person picked.
//!
//! `users.name_source` has recorded the difference since 2026-10-26 (`github` by default,
//! `user` once `PATCH /me` writes a name) and the boxes already read it through
//! `Db::user_address` — a GitHub-seeded name is a label, never an address. The relay simply
//! never told a client. These tests pin that it does now, and that the two paths which were
//! already correct stay correct: an ordinary sign-up is still `github`, and a person who has
//! named themselves is never asked a second time.
//!
//! The client rule these tests encode: ask only when `name_source == "github"`. Read
//! `name_source`, never `name` — a non-empty `name` does not mean a chosen name (the invited
//! case), and an empty one does not mean an unchosen name (the deliberate-clear case).

use axum::body::to_bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use serde_json::{json, Value};

use arbos_core::ids::ulid;

use crate::auth::Principal;
use crate::db_accounts::GithubIdentity;
use crate::signin::PatchMeBody;
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};

/// What GitHub says about a login. `name` is the profile's display name, which is what both the
/// invite path and the sign-up path copy onto the row.
fn github(id: i64, login: &str, name: Option<&str>) -> GithubIdentity {
    GithubIdentity {
        id,
        login: login.to_owned(),
        avatar_url: None,
        name: name.map(str::to_owned),
    }
}

/// A device principal for a user who has signed in, as the sign-in callback makes one.
fn device_for(state: &AppState, user_id: &str) -> Principal {
    let id = ulid();
    state
        .db
        .insert_device(&id, "iPhone", &ulid(), user_id, "github")
        .expect("device");
    Principal::Device {
        id,
        token: String::new(),
        user_id: user_id.to_owned(),
    }
}

/// Someone else invites `login`: the relay stores a named `invited` placeholder for it before
/// that person has ever opened Arbos. This is `people::resolve_via_github(create = true)` with
/// the GitHub round trip removed.
fn invited_by_someone_else(state: &AppState, gh: &GithubIdentity) -> String {
    state
        .db
        .upsert_invited_user(
            gh.id,
            &gh.login,
            gh.avatar_url.as_deref(),
            gh.name.as_deref(),
        )
        .expect("invited placeholder")
}

/// A GitHub sign-in. Returns `(user id, first sign-in)` — `true` both for a brand-new account
/// and for the first sign-in of an invited placeholder.
fn sign_in(state: &AppState, gh: &GithubIdentity) -> (String, bool) {
    let (user, created) = state.db.upsert_github_user(gh, 20.0).expect("sign-in");
    (user.id, created)
}

/// The `user` object a client actually receives from `GET /me`.
async fn me_user(state: &AppState, principal: &Principal) -> Value {
    let resp = crate::signin::me(State(state.clone()), principal.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK, "GET /me");
    body(resp).await["user"].clone()
}

async fn body(resp: Response) -> Value {
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).expect("json body")
}

/// Does a client have to ask this person to name themselves? The whole point of the field.
fn must_ask(user: &Value) -> bool {
    match user["name_source"].as_str() {
        Some(s) => s == "github",
        None => panic!("/me carries no name_source, so a client cannot tell: {user}"),
    }
}

/// The invited case, and the one that fails on today's relay: `/me` hands the client a filled-in
/// `name` that came from the invite, with nothing to mark it as unchosen.
///
/// Before `name_source` reached the wire, `must_ask` had no field to read. A client could only
/// look at `name` — which is populated here — and conclude the person had already named
/// themselves, which is exactly the repeat-onboarding limit #2469 stated rather than guessed.
#[tokio::test]
async fn an_invited_users_seeded_name_is_not_a_chosen_name() {
    let state = test_state(AuthMode::Github);
    let gh = github(4242, "dana", Some("Dana R."));

    // Someone invites Dana. The row — and the name — exist before Dana does anything.
    let placeholder = invited_by_someone_else(&state, &gh);
    assert_eq!(
        state.db.user_card(&placeholder).unwrap().unwrap().status,
        "invited"
    );

    // Dana signs in for the first time.
    let (user_id, first) = sign_in(&state, &gh);
    assert_eq!(user_id, placeholder, "the placeholder is the account");
    assert!(first, "first sign-in of an invited placeholder");

    let user = me_user(&state, &device_for(&state, &user_id)).await;
    assert_eq!(
        user["name"].as_str(),
        Some("Dana R."),
        "the seeded name is still shown as a label"
    );
    assert_eq!(
        user["name_source"].as_str(),
        Some("github"),
        "but the wire says Dana never chose it"
    );
    assert!(must_ask(&user), "so the client asks");
}

/// The option itself: an invited person sets their display name and username on first sign-in,
/// and is not asked again. Setting the handle to a value only this account holds must not trip
/// the uniqueness check against the account's own placeholder row.
#[tokio::test]
async fn an_invited_user_can_name_themselves_on_first_signin() {
    let state = test_state(AuthMode::Github);
    let gh = github(4343, "dana2", Some("Dana R."));
    invited_by_someone_else(&state, &gh);
    let (user_id, _) = sign_in(&state, &gh);
    let principal = device_for(&state, &user_id);
    assert!(must_ask(&me_user(&state, &principal).await));

    let resp = crate::signin::patch_me(
        State(state.clone()),
        principal.clone(),
        Json(PatchMeBody {
            name: Some("Dana".into()),
            login: Some("dana-r".into()),
            group_add: None,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "PATCH /me");

    // The reply a client gets back already carries the answer; no second round trip to /me.
    let patched = body(resp).await["user"].clone();
    assert_eq!(patched["name"].as_str(), Some("Dana"));
    assert_eq!(patched["login"].as_str(), Some("dana-r"));
    assert!(!must_ask(&patched), "the person has now chosen");

    let user = me_user(&state, &principal).await;
    assert_eq!(user["name_source"].as_str(), Some("user"));
    assert!(!must_ask(&user), "and is not asked again on the next open");
}

/// Regression guard: an ordinary sign-up — a GitHub account nobody invited — behaves exactly as
/// it did. Its profile name is a label too, so it is still asked; and the shape of `user` is
/// unchanged apart from the one added key, so no client loses a field it reads.
#[tokio::test]
async fn an_ordinary_signup_is_unchanged() {
    let state = test_state(AuthMode::Github);
    let gh = github(4444, "erin", Some("Erin"));

    let (user_id, created) = sign_in(&state, &gh);
    assert!(created, "a brand-new account");
    assert_eq!(
        state.db.user_card(&user_id).unwrap().unwrap().status,
        "member",
        "a sign-up is a member straight away, never invited"
    );

    let user = me_user(&state, &device_for(&state, &user_id)).await;
    assert_eq!(user["name"].as_str(), Some("Erin"));
    assert_eq!(user["login"].as_str(), Some("erin"));
    assert_eq!(user["github_id"].as_i64(), Some(4444));
    assert_eq!(
        user["name_source"].as_str(),
        Some("github"),
        "a profile name is a label on this path too"
    );

    let mut keys: Vec<&str> = user
        .as_object()
        .expect("user object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "avatar_url",
            "created_at",
            "github_id",
            "id",
            "last_login_at",
            "login",
            "name",
            "name_source",
        ],
        "`user` gains name_source and loses nothing"
    );
}

/// Regression guard: a returning user is never asked again, on any of the three ways their name
/// can look. A later sign-in refreshes the avatar and must leave the choice alone.
#[tokio::test]
async fn a_returning_user_is_never_asked_again() {
    let state = test_state(AuthMode::Github);
    let gh = github(4545, "frank", Some("Arbos"));
    let (user_id, _) = sign_in(&state, &gh);
    let principal = device_for(&state, &user_id);

    // They named themselves once, on some earlier device.
    state
        .db
        .set_user_profile(&user_id, Some("Frank"), None)
        .expect("profile");
    assert!(!must_ask(&me_user(&state, &principal).await));

    // Signing in again — a reinstall, a second Mac — keeps the choice.
    let (_, created) = sign_in(&state, &gh);
    assert!(!created, "not a first sign-in");
    let user = me_user(&state, &device_for(&state, &user_id)).await;
    assert_eq!(user["name"].as_str(), Some("Frank"));
    assert_eq!(user["name_source"].as_str(), Some("user"));
    assert!(!must_ask(&user), "a returning user is never re-onboarded");

    // Clearing the name is itself a choice: `name` goes null but the person is not asked again,
    // which is why a client must read `name_source` and not `name`.
    let resp = crate::signin::patch_me(
        State(state.clone()),
        principal.clone(),
        Json(PatchMeBody {
            name: Some(String::new()),
            login: None,
            group_add: None,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let user = me_user(&state, &principal).await;
    assert_eq!(user["name"], json!(null), "cleared to the bare @handle");
    assert_eq!(user["name_source"].as_str(), Some("user"));
    assert!(!must_ask(&user), "an empty name is not an unchosen name");
}
