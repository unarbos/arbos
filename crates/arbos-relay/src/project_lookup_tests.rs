//! Step 1 of the reported chain: `GET /projects/{name}/pushes` told a caller which project
//! names were taken. A name nobody had answered `404 no such project`; a name someone else
//! had answered `403 not your project`. That difference is a list of every agent on the relay,
//! and the names were all the reporter needed to go on to the group routes.
//!
//! Every route here is asked the same question twice, on two relays that differ in one fact:
//! on the first, Victor owns an agent called `Lium`; on the second, nobody does. The answers
//! must match, word for word and status for status.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::json;

use crate::api::{
    self, ActivityBody, ArchiveBody, AuditQuery, KeepaliveBody, PatchChatBody, StopQuery,
    TimingBody,
};
use crate::auth::Principal;
use crate::people::tests_support::{person, project_of};
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};

/// The name Mallory is guessing at.
const GUESS: &str = "Lium";

/// A relay with Mallory and her agent on it, and Victor's `Lium` only when `victim` is set.
fn relay(victim: bool) -> (AppState, Principal) {
    let state = test_state(AuthMode::Github);
    let (mallory_user, mallory) = person(&state, 9501);
    project_of(&state, &mallory_user, "Mallory's agent");
    if victim {
        let (victor_user, _) = person(&state, 9502);
        project_of(&state, &victor_user, GUESS);
    }
    (state, mallory)
}

type Refusal = (StatusCode, String);

/// Ask one route for `Lium` on both relays and hand back the two refusals.
async fn both<F, Fut>(route: F) -> (Refusal, Refusal)
where
    F: Fn(AppState, Principal, String) -> Fut,
    Fut: std::future::Future<Output = crate::api::ApiResult>,
{
    let mut out = Vec::new();
    for victim in [true, false] {
        let (state, mallory) = relay(victim);
        let answer = route(state, mallory, GUESS.to_owned()).await;
        out.push(answer.expect_err("Mallory gets nothing back, either way"));
    }
    (out.remove(0), out.remove(0))
}

/// The route the reporter used.
#[tokio::test]
async fn pushes_does_not_say_whether_the_name_is_taken() {
    let (taken, free) =
        both(|s, p, id| async move { api::take_pushes(State(s), p, Path(id)).await }).await;
    assert_eq!(
        taken.0,
        StatusCode::NOT_FOUND,
        "a project Mallory may not read is simply not found"
    );
    assert_eq!(
        taken, free,
        "`pushes` must answer a taken name exactly as it answers a free one"
    );
}

/// And every sibling that takes a project in its path, so none of them is the next `pushes`.
#[tokio::test]
async fn the_sibling_routes_keep_the_same_silence() {
    macro_rules! same {
        ($label:literal, $call:expr) => {{
            let (taken, free) = both($call).await;
            assert_eq!(
                taken.0,
                StatusCode::NOT_FOUND,
                concat!($label, ": a project Mallory may not touch is not found")
            );
            assert_eq!(
                taken, free,
                concat!($label, ": taken and free names must read alike")
            );
        }};
    }

    same!("usage", |s, p, id| async move {
        api::usage(State(s), p, Path(id)).await
    });
    same!("status", |s, p, id| async move {
        api::project_status(State(s), p, Path(id)).await
    });
    same!("audit (read)", |s, p, id| async move {
        api::list_audit(State(s), p, Path(id), Query(AuditQuery { limit: None })).await
    });
    same!("audit (write)", |s, p, id| async move {
        api::record_audit(State(s), p, Path(id), Json(json!({ "kind": "probe" }))).await
    });
    same!("timings", |s, p, id| async move {
        api::record_timing(
            State(s),
            p,
            Path(id),
            Json(TimingBody {
                kind: "probe".into(),
                ms: 1,
                detail: None,
            }),
        )
        .await
    });
    same!("activity", |s, p, id| async move {
        api::set_activity(
            State(s),
            p,
            Path(id),
            Json(ActivityBody {
                chat_id: "c".into(),
                text: None,
                jobs: None,
            }),
        )
        .await
    });
    same!("keepalive", |s, p, id| async move {
        api::keepalive(
            State(s),
            p,
            Path(id),
            Json(KeepaliveBody {
                turn_running: false,
            }),
        )
        .await
    });
    same!("archive", |s, p, id| async move {
        api::archive_project(State(s), p, Path(id), Json(ArchiveBody { archived: true })).await
    });
    same!("wake", |s, p, id| async move {
        api::wake(State(s), p, Path(id)).await
    });
    same!("stop", |s, p, id| async move {
        api::stop(State(s), p, Path(id), Query(StopQuery { force: false })).await
    });
    same!("chat rename", |s, p, id| async move {
        api::patch_chat(
            State(s),
            p,
            Path((id, "c".to_owned())),
            Json(PatchChatBody {
                title: "probe".into(),
            }),
        )
        .await
    });
    same!("chat close", |s, p, id| async move {
        api::delete_chat(State(s), p, Path((id, "c".to_owned()))).await
    });
}

/// The grant card names other people's agents on purpose, so it is the one place a lookup
/// reaches across owners. It takes an id, never a name — a name would make it a directory.
#[tokio::test]
async fn a_grant_names_another_agent_by_id_and_never_by_name() {
    let state = test_state(AuthMode::Github);
    let (mallory_user, mallory) = person(&state, 9601);
    let mine = project_of(&state, &mallory_user, "Mallory's agent");
    let (victor_user, _) = person(&state, 9602);
    let victor_agent = project_of(&state, &victor_user, GUESS);

    let by_name = crate::agent_messages::put_grant(
        State(state.clone()),
        mallory.clone(),
        Path(mine.clone()),
        Json(crate::agent_messages::GrantBody {
            grantee: format!("project:{GUESS}"),
            enabled: Some(true),
            note: None,
        }),
    )
    .await
    .expect_err("a name tells Mallory nothing");
    assert_eq!(by_name.0, StatusCode::NOT_FOUND);

    // The id Mallory already holds still works, which is what the card is for.
    crate::agent_messages::put_grant(
        State(state.clone()),
        mallory,
        Path(mine),
        Json(crate::agent_messages::GrantBody {
            grantee: format!("project:{victor_agent}"),
            enabled: Some(true),
            note: None,
        }),
    )
    .await
    .expect("an exact id is how a grant names an agent");
}

/// Mallory's own agent answers to its own name even when a stranger's agent, touched more
/// recently, shares it. Without this the uniform refusal would start eating real requests.
#[tokio::test]
async fn your_own_name_wins_over_a_strangers() {
    let state = test_state(AuthMode::Github);
    let (mallory_user, mallory) = person(&state, 9701);
    let mine = project_of(&state, &mallory_user, "Tern");
    let (victor_user, _) = person(&state, 9702);
    let theirs = project_of(&state, &victor_user, "Tern");
    assert_ne!(mine, theirs);

    let resolved = crate::api::project_for(&state, &mallory, "Tern").expect("Mallory's own Tern");
    assert_eq!(
        resolved.id, mine,
        "the caller's own agent, not the stranger's"
    );
}
