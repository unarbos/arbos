//! #2262: a running box's row never loses its sandbox fields. gh_17856706's row was blanked when
//! a planned relay restart (SIGTERM 01:03:04 on Oct 29) cut a create between the first row write
//! and the new sandbox id; the new box then authenticated against a row that named no sandbox,
//! and rolls and always-on lost it.
//!
//! The create that was cut short belonged to the retired Daytona provider, but neither guard is
//! about a provider: the database refuses the blanking write, and the `box_creates` ledger names
//! the sandbox a create is about to make for every provider.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::Value;

use arbos_core::ids::now_ms;

use crate::auth::Principal;
use crate::boxes::{self, Provider};
use crate::db::Sandbox;
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode, UpgradeStatus};

fn live_row(user: &str) -> Sandbox {
    Sandbox {
        user_id: user.into(),
        provider: "gvisor".into(),
        sandbox_id: Some("arbos-box-old".into()),
        daytona_state: None,
        device_id: "box:arbos-box-old".into(),
        box_token_hash: "hash-of-the-running-box".into(),
        mcp_token: "mcp-old".into(),
        snapshot: Some("arbos-agent-0.1.179".into()),
        last_started_at: Some(1_000),
        last_stopped_at: None,
        last_seen_at: Some(now_ms()),
        box_url: Some("http://127.0.0.1:7788".into()),
        preview_token: None,
        local_pid: None,
        migrated_from: Vec::new(),
    }
}

/// A create that fails (here: no agentd binary to spawn) over a live row leaves the row on the box
/// the user has, and clears the create ledger so the failed attempt is not reported as cut short.
async fn failed_create_over_live_row(state: &AppState, user: &str) {
    state.db.upsert_box(&live_row(user)).expect("live row");
    boxes::create_box(state, user, Provider::Process, Some("arbos-agent-0.1.181"))
        .await
        .expect_err("no agentd binary to spawn");
}

/// The incident, replayed: before the fix the row came back as the create's placeholder — no
/// sandbox_id, snapshot, device_id, box_url or last_started_at — with a fresh token hash.
#[tokio::test]
async fn a_create_that_fails_leaves_the_live_row_whole() {
    let mut state = test_state(AuthMode::Github);
    std::sync::Arc::get_mut(&mut state.cfg)
        .expect("one holder")
        .agentd_bin = "/nonexistent/arbos-agentd".into();
    failed_create_over_live_row(&state, "gh_live").await;

    let row = state.db.user_box("gh_live").unwrap().expect("row");
    let before = live_row("gh_live");
    assert_eq!(row.sandbox_id, before.sandbox_id);
    assert_eq!(row.snapshot, before.snapshot);
    assert_eq!(row.device_id, before.device_id);
    assert_eq!(row.box_url, before.box_url);
    assert_eq!(row.last_started_at, before.last_started_at);
    assert_eq!(row.box_token_hash, before.box_token_hash);
    assert!(state
        .db
        .unfinished_box_creates(now_ms() + 1)
        .unwrap()
        .is_empty());
}

/// What a create cut short by a restart leaves behind is findable: its `box_creates` row names the
/// sandbox the provider may be running and the sandbox it was replacing, and the log-only check
/// reports it once it is stale and no create is running in this process.
#[tokio::test]
async fn a_cut_short_create_is_recorded_and_reported() {
    let state = test_state(AuthMode::Github);
    state.db.upsert_box(&live_row("gh_live")).unwrap();
    state
        .db
        .begin_box_create(
            "gh_live",
            "arbos-box-gh-live",
            "gvisor",
            Some("arbos-agent-0.1.181"),
            Some("arbos-box-old"),
        )
        .unwrap();

    let left = state.db.unfinished_box_creates(now_ms() + 1).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].sandbox_name, "arbos-box-gh-live");
    assert_eq!(left[0].replacing.as_deref(), Some("arbos-box-old"));
    assert_eq!(left[0].snapshot.as_deref(), Some("arbos-agent-0.1.181"));

    // Not stale yet: the check stays quiet. Ten minutes later it names the sandbox.
    assert!(boxes::sandboxless_live_boxes(&state).await.is_empty());
    state
        .db
        .with(|c| {
            c.execute(
                "UPDATE box_creates SET started_at = started_at - 11 * 60000",
                [],
            )
        })
        .unwrap();
    let found = boxes::sandboxless_live_boxes(&state).await;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["kind"], "unfinished_create");
    assert_eq!(found[0]["sandbox_name"], Value::from("arbos-box-gh-live"));
    assert_eq!(found[0]["provider"], Value::from("gvisor"));

    // A create running in this process is this process's business, not a finding.
    let _creating = crate::state::CreatingGuard::new(&state.restart, "gh_live");
    assert!(boxes::sandboxless_live_boxes(&state).await.is_empty());
}

/// The DB itself refuses any write that would drop a live row's sandbox (a stale write-back of a
/// row read mid-create included).
#[tokio::test]
async fn a_row_with_a_sandbox_is_never_written_without_it() {
    let state = test_state(AuthMode::Github);
    state.db.upsert_box(&live_row("gh_a")).unwrap();

    let mut blank = live_row("gh_a");
    blank.sandbox_id = None;
    blank.snapshot = None;
    blank.box_url = None;
    blank.device_id = String::new();
    blank.box_token_hash = "fresh-hash".into();
    assert!(state.db.upsert_box(&blank).is_err());
    let row = state.db.user_box("gh_a").unwrap().unwrap();
    assert_eq!(row.sandbox_id.as_deref(), Some("arbos-box-old"));
    assert_eq!(row.box_token_hash, "hash-of-the-running-box");

    // A first box's placeholder (no row yet, or a row with no sandbox) is still written.
    state
        .db
        .upsert_box(&{
            let mut b = blank.clone();
            b.user_id = "gh_new".into();
            b
        })
        .unwrap();
    assert!(state
        .db
        .user_box("gh_new")
        .unwrap()
        .unwrap()
        .sandbox_id
        .is_none());
}

/// A planned restart with an upgrade in flight answers 409 and changes nothing; while a restart
/// is under way no upgrade starts.
#[tokio::test]
async fn a_planned_restart_waits_for_box_work() {
    let state = test_state(AuthMode::Github);
    state.upgrading.lock().await.insert(
        "gh_a".into(),
        UpgradeStatus {
            phase: "pack".into(),
            since: now_ms(),
            from_snapshot: None,
            to_snapshot: "arbos-agent-0.1.181".into(),
        },
    );
    let r = crate::restart::admin_restart(
        State(state.clone()),
        Principal::Admin,
        Some(Json(crate::restart::RestartBody {
            notice_s: Some(0),
            drain_s: Some(0),
            force: None,
        })),
    )
    .await;
    assert_eq!(r.status(), StatusCode::CONFLICT);
    assert!(!state.restart.is_closing());
    assert!(!state.restart.box_work_frozen());

    state.upgrading.lock().await.clear();
    {
        let _creating = crate::state::CreatingGuard::new(&state.restart, "gh_b");
        let work = crate::restart::box_work_in_flight(&state).await;
        assert_eq!(work.len(), 1);
        assert_eq!(work[0]["kind"], "create");
    }
    assert!(crate::restart::box_work_in_flight(&state).await.is_empty());

    state.db.upsert_box(&live_row("gh_c")).unwrap();
    crate::restart::freeze_box_work(&state).await;
    let e = crate::upgrade::start(&state, "gh_c", true)
        .await
        .expect_err("frozen");
    assert!(e.to_string().contains(crate::upgrade::RESTARTING_PHRASE));
    assert!(state.upgrading.lock().await.is_empty());
}

/// The log-only check: a row without a sandbox whose box shows a sign of life is reported; a
/// row with a sandbox, or one long silent, is not. Nothing is written.
#[tokio::test]
async fn the_check_reports_live_rows_without_a_sandbox() {
    let state = test_state(AuthMode::Github);
    state.db.upsert_box(&live_row("gh_ok")).unwrap();
    let mut limbo = live_row("gh_limbo");
    limbo.sandbox_id = None;
    state.db.upsert_box(&limbo).unwrap();
    let mut silent = limbo.clone();
    silent.user_id = "gh_silent".into();
    silent.last_seen_at = Some(now_ms() - 3_600_000);
    state.db.upsert_box(&silent).unwrap();

    let found = boxes::sandboxless_live_boxes(&state).await;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["kind"], "sandboxless_live_row");
    assert_eq!(found[0]["user_id"], "gh_limbo");
    assert!(state
        .db
        .user_box("gh_limbo")
        .unwrap()
        .unwrap()
        .sandbox_id
        .is_none());
}
