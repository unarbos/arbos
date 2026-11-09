//! Daytona is retired as a box provider (Jacob, 2026-11-09). These cases pin the two halves of
//! that: no new box can be put on it, whatever an operator, a client or relay.env asks for, and a
//! `boxes` row still on it is left strictly alone and reported as retired rather than driven.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use serde_json::{json, Value};

use arbos_core::ids::now_ms;
use arbos_core::wire::BoxState;

use crate::api;
use crate::auth::Principal;
use crate::boxes::{self, Provider, RETIRED_PROVIDER};
use crate::db::Sandbox;
use crate::people::tests_support::{person, project_of};
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};

/// The JSON a handler answered with.
async fn body_of(res: Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), 4 << 20)
        .await
        .expect("a body");
    serde_json::from_slice(&bytes).expect("JSON")
}

/// A relay whose `ARBOS_DEFAULT_PROVIDER` is whatever the host happens to name.
fn relay_with_default(provider: Option<&str>) -> AppState {
    let mut state = test_state(AuthMode::Github);
    std::sync::Arc::get_mut(&mut state.cfg)
        .expect("one holder")
        .default_provider = provider.map(str::to_owned);
    state
}

/// A row of a box that was created on Daytona before the retirement and is still in the database.
fn retired_row(user_id: &str) -> Sandbox {
    Sandbox {
        user_id: user_id.into(),
        provider: RETIRED_PROVIDER.into(),
        sandbox_id: Some("92fcb3f8-old".into()),
        daytona_state: Some("started".into()),
        device_id: "box:92fcb3f8-old".into(),
        box_token_hash: "hash-of-the-old-box".into(),
        mcp_token: "mcp-old".into(),
        snapshot: Some("arbos-agent-0.1.179".into()),
        last_started_at: Some(1_000),
        last_stopped_at: None,
        last_seen_at: Some(now_ms()),
        box_url: Some("https://7788-old.proxy.daytona.works".into()),
        preview_token: Some("pv".into()),
        local_pid: None,
        migrated_from: Vec::new(),
    }
}

/// The name is not a provider any more: nothing parses it, and the message that lists the choices
/// does not offer it.
#[test]
fn the_retired_name_is_not_a_provider() {
    assert!(Provider::parse(RETIRED_PROVIDER).is_none());
    assert!(!Provider::NAMES.contains(RETIRED_PROVIDER));
    for p in [Provider::Process, Provider::Docker, Provider::Gvisor] {
        assert_eq!(Provider::parse(p.as_str()), Some(p));
        assert!(Provider::NAMES.contains(p.as_str()));
    }
}

/// Where a new box goes: the host's gVisor fleet when relay.env says so, a local process when it
/// says nothing — and a local process rather than Daytona when relay.env still names Daytona (the
/// live host's relay.env did on the day of the retirement).
#[test]
fn a_new_box_never_goes_to_the_retired_provider() {
    assert_eq!(
        boxes::default_provider(&relay_with_default(Some("gvisor"))),
        Provider::Gvisor
    );
    assert_eq!(
        boxes::default_provider(&relay_with_default(None)),
        Provider::Process
    );
    assert_eq!(
        boxes::default_provider(&relay_with_default(Some(RETIRED_PROVIDER))),
        Provider::Process
    );
}

/// The per-user override (`users.box_provider`, `PUT /admin/users/{id}/box`) cannot send a user to
/// the retired provider either: a value the relay cannot run falls back to the default.
#[test]
fn a_per_user_override_cannot_name_the_retired_provider() {
    let state = relay_with_default(Some("gvisor"));
    let (user, _) = person(&state, 9601);
    state
        .db
        .set_user_provider(&user, Some(RETIRED_PROVIDER))
        .expect("stored by hand");
    assert_eq!(boxes::provider_for_user(&state, &user), Provider::Gvisor);
}

/// `POST /projects {"provider": "daytona"}` is refused like any other name the relay cannot run,
/// and the refusal lists what it can.
#[tokio::test]
async fn creating_a_project_on_the_retired_provider_is_refused() {
    let state = relay_with_default(Some("gvisor"));
    let (_, device) = person(&state, 9602);
    let body = serde_json::from_value(json!({ "name": "demo", "provider": RETIRED_PROVIDER }))
        .expect("body");
    let (code, message) = crate::api::create_project(State(state), device, Json(body))
        .await
        .expect_err("refused");
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(message, format!("provider must be {}", Provider::NAMES));
}

/// `PUT /admin/users/{id}/box {"provider": "daytona"}` is refused too, and nothing is stored: the
/// admin route was the one way left to put a box back on Daytona by hand.
#[tokio::test]
async fn an_admin_cannot_set_a_user_back_to_the_retired_provider() {
    let state = relay_with_default(Some("gvisor"));
    let (user, _) = person(&state, 9603);
    let (code, message) = crate::api::admin_set_user_box(
        State(state.clone()),
        Principal::Admin,
        Path(user.clone()),
        Json(json!({ "provider": RETIRED_PROVIDER })),
    )
    .await
    .expect_err("refused");
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(
        message,
        format!("provider must be {} or null", Provider::NAMES)
    );
    assert_eq!(state.db.user_provider(&user).expect("read"), None);
}

/// A row left on the retired provider is recognised: the message says what it is, so an operator
/// reading a log line is not sent looking for a provider the relay never had.
#[test]
fn a_row_on_the_retired_provider_reads_as_retired() {
    let e = boxes::provider_of(&retired_row("gh_old")).expect_err("not a provider any more");
    let said = e.to_string();
    assert!(said.contains("retired"), "{said}");
    assert!(said.contains(RETIRED_PROVIDER), "{said}");
    assert!(said.contains("gh_old"), "{said}");

    let unknown = Sandbox {
        provider: "fly".into(),
        ..retired_row("gh_unknown")
    };
    assert!(boxes::provider_of(&unknown)
        .expect_err("still not a provider")
        .to_string()
        .contains("bad provider fly"));
}

/// Starting, stopping and rolling such a box all refuse, and the row keeps every field: the
/// sandbox itself is deleted by an operator step outside the relay, and until then the row is the
/// only record of which sandbox belongs to whom.
#[tokio::test]
async fn a_box_on_the_retired_provider_is_never_driven() {
    let state = relay_with_default(Some("gvisor"));
    let (user, _) = person(&state, 9604);
    let before = retired_row(&user);
    state.db.upsert_box(&before).expect("row");

    for refusal in [
        boxes::start_box(&state, &user).await.expect_err("start"),
        boxes::stop_box(&state, &user, true)
            .await
            .expect_err("stop"),
    ] {
        assert!(
            refusal.to_string().contains(RETIRED_PROVIDER),
            "{refusal:#}"
        );
    }
    let rolled = crate::upgrade::start(&state, &user, true)
        .await
        .expect_err("roll");
    assert!(rolled.to_string().contains(RETIRED_PROVIDER), "{rolled:#}");

    let after = state.db.user_box(&user).expect("read").expect("row");
    assert_eq!(after.provider, before.provider);
    assert_eq!(after.sandbox_id, before.sandbox_id);
    assert_eq!(after.snapshot, before.snapshot);
    assert_eq!(after.box_url, before.box_url);
    assert!(state.upgrading.lock().await.is_empty());
}

/// The read side, which the retirement does not change. The five rows left on the live relay
/// (Carro's box and four test or orphan rows) go through every path that enumerates or reports a
/// box: nothing panics, nothing answers an error, each path shows the box as it is, and no path
/// makes, re-provisions or rolls a replacement.
#[tokio::test]
async fn a_legacy_row_reads_through_every_path_and_is_left_alone() {
    let state = relay_with_default(Some("gvisor"));
    let (user, device) = person(&state, 9606);
    let project = project_of(&state, &user, "Carro");
    let before = retired_row(&user);
    state.db.upsert_box(&before).expect("row");

    // `GET /machine`: the provider and sandbox are reported, and the box is not offered a roll —
    // `image_outdated` is a gVisor question, so an old snapshot here is not a prompt to recreate.
    let machine = body_of(
        api::machine(State(state.clone()), device.clone())
            .await
            .expect("GET /machine answers"),
    )
    .await;
    assert_eq!(machine["provider"], json!(RETIRED_PROVIDER));
    assert_eq!(machine["sandbox_id"], json!("92fcb3f8-old"));
    assert_eq!(machine["image"]["current"], json!("arbos-agent-0.1.179"));
    assert_eq!(machine["image_outdated"], json!(false));
    assert_eq!(machine["upgrade"]["in_progress"], json!(false));

    // `GET /projects/{id}/status`: the provider view of the box, read straight off the row.
    let status = body_of(
        api::project_status(State(state.clone()), device.clone(), Path(project.clone()))
            .await
            .expect("GET /projects/{id}/status answers"),
    )
    .await;
    assert_eq!(status["provider"]["provider"], json!(RETIRED_PROVIDER));
    assert_eq!(status["provider"]["sandbox_id"], json!("92fcb3f8-old"));

    // The profile line every project listing carries.
    let listed = body_of(
        api::list_projects(State(state.clone()), device.clone())
            .await
            .expect("GET /projects answers"),
    )
    .await;
    assert_eq!(listed[0]["machine"]["class"], json!(RETIRED_PROVIDER));
    assert_eq!(listed[0]["machine"]["image_outdated"], json!(false));

    // `GET /admin/users`: the operator's enumeration, and the field the deploy worker reads to
    // leave the box out of the roll.
    let users =
        body_of(crate::admin::list_users(State(state.clone()), Principal::Admin).await).await;
    let machines: Vec<&serde_json::Value> = users
        .as_array()
        .expect("a list")
        .iter()
        .filter(|u| u["id"] == json!(user))
        .map(|u| &u["machine"])
        .collect();
    assert_eq!(machines.len(), 1);
    assert_eq!(machines[0]["provider"], json!(RETIRED_PROVIDER));
    assert_eq!(machines[0]["image_outdated"], json!(false));

    // `GET /admin/fleet`: the host's own view holds host sandboxes only, so the row is not read as
    // a resident whose container has gone missing.
    let fleet =
        body_of(crate::fleet::admin_fleet(State(state.clone()), Principal::Admin).await).await;
    assert_eq!(fleet["sandboxes"], json!([]));

    // The limbo check (#2262) has nothing to say: the row names its sandbox.
    assert!(boxes::sandboxless_live_boxes(&state).await.is_empty());

    // Every wake door: a user's message, an explicit start, and the always-on sweep that asks for
    // every signed-in user once a minute. None of them queues a wake, and none makes a new box.
    boxes::ensure_awake(&state, &user, "people").await;
    boxes::ensure_awake(&state, &user, "always-on").await;
    boxes::ensure_awake(&state, &user, "start").await;
    assert!(state.db.due_wake_jobs().expect("read").is_empty());

    let after = state.db.user_box(&user).expect("read").expect("row");
    assert_eq!(after.provider, before.provider);
    assert_eq!(after.sandbox_id, before.sandbox_id);
    assert_eq!(after.snapshot, before.snapshot);
    assert_eq!(after.box_token_hash, before.box_token_hash);
    assert_eq!(after.box_url, before.box_url);
    assert_eq!(after.preview_token, before.preview_token);
    assert!(state
        .db
        .unfinished_box_creates(now_ms() + 1)
        .expect("read")
        .is_empty());
}

/// A wake job already in the database when the relay is upgraded (a live relay has them) ends
/// instead of failing three times and leaving a box that is running reported as `error`.
#[tokio::test]
async fn a_wake_job_left_over_for_a_legacy_row_is_dropped() {
    let state = relay_with_default(Some("gvisor"));
    let (user, _) = person(&state, 9607);
    state.db.upsert_box(&retired_row(&user)).expect("row");
    state
        .db
        .upsert_wake_job(&user, "queued before the upgrade")
        .expect("job");
    assert!(!state.db.due_wake_jobs().expect("read").is_empty());

    boxes::run_wake_jobs(&state).await;

    assert!(state.db.due_wake_jobs().expect("read").is_empty());
    assert_ne!(
        state.box_state(&user).await.0,
        BoxState::Error,
        "the relay must not report a box it cannot reach as failed"
    );
}

/// A gVisor box already on the target is not rolled again, and one behind it is: the roll itself
/// still works after the provider removal.
#[tokio::test]
async fn a_gvisor_box_is_still_rolled() {
    let state = relay_with_default(Some("gvisor"));
    let (user, _) = person(&state, 9605);
    let mut sb = Sandbox {
        provider: Provider::Gvisor.as_str().into(),
        sandbox_id: Some("arbos-box-gh-new".into()),
        daytona_state: None,
        snapshot: Some(state.default_snapshot()),
        ..retired_row(&user)
    };
    state.db.upsert_box(&sb).expect("row");
    assert!(!crate::upgrade::start(&state, &user, false)
        .await
        .expect("no error"));

    sb.snapshot = Some("arbos-agent-0.0.1".into());
    state.db.upsert_box(&sb).expect("older row");
    assert!(crate::upgrade::image_outdated(&state, &sb));
}
