//! The same silence, in the modules that keep their own project lookup instead of going through
//! `api::project_for`. Each of these answered a name someone else held differently from a name
//! nobody held, which is the enumeration oracle the reporter used on `pushes` (reported
//! 2026-11-02), just spelt with a different status line.
//!
//! Every route is asked the same question on two relays that differ in one fact: on the first,
//! Victor owns an agent called `Lium`; on the second, nobody does. The answers must match.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;

use crate::auth::Principal;
use crate::people::tests_support::{person, project_of};
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};

/// The name Mallory is guessing at.
const GUESS: &str = "Lium";

/// A relay with Mallory and her own agent, and Victor's `Lium` only when `victim` is set.
fn relay(victim: bool) -> (AppState, Principal) {
    let state = test_state(AuthMode::Github);
    let (mallory_user, mallory) = person(&state, 9801);
    project_of(&state, &mallory_user, "Mallory's agent");
    if victim {
        let (victor_user, _) = person(&state, 9802);
        project_of(&state, &victor_user, GUESS);
    }
    (state, mallory)
}

/// Ask one route for `Lium` on both relays and hand back the two statuses.
async fn both<F, Fut>(route: F) -> (StatusCode, StatusCode)
where
    F: Fn(AppState, Principal, String) -> Fut,
    Fut: std::future::Future<Output = Response>,
{
    let mut out = Vec::new();
    for victim in [true, false] {
        let (state, mallory) = relay(victim);
        out.push(route(state, mallory, GUESS.to_owned()).await.status());
    }
    (out.remove(0), out.remove(0))
}

macro_rules! same {
    ($label:literal, $call:expr) => {{
        let (taken, free) = both($call).await;
        assert_eq!(
            taken,
            StatusCode::NOT_FOUND,
            concat!($label, ": a project Mallory may not touch is not found")
        );
        assert_eq!(
            taken, free,
            concat!($label, ": taken and free names must read alike")
        );
    }};
}

/// `people`'s own two lookups, reached through the four routes that use them.
#[tokio::test]
async fn the_project_member_routes_keep_the_same_silence() {
    same!("members (add)", |s, p, id| async move {
        crate::people::project_member_add(
            State(s),
            p,
            Path(id),
            Json(crate::people::ProjectMemberBody {
                handle: "someone".into(),
            }),
        )
        .await
    });
    same!("members (list)", |s, p, id| async move {
        crate::people::project_members_list(State(s), p, Path(id)).await
    });
    same!("members (remove)", |s, p, id| async move {
        crate::people::project_member_remove(State(s), p, Path((id, "u_other".to_owned()))).await
    });
}

/// `POST /chats/{id}/agents` used to say "you can only add or change your own Project's agent"
/// for an agent that existed and "no such project" for one that did not. Mallory has to be a
/// member of a chat to get this far, so the chat here is her own.
#[tokio::test]
async fn adding_an_agent_to_a_chat_does_not_say_whose_name_is_taken() {
    let mut answers = Vec::new();
    for victim in [true, false] {
        let state = test_state(AuthMode::Github);
        let (mallory_user, mallory) = person(&state, 9811);
        project_of(&state, &mallory_user, "Mallory's agent");
        if victim {
            let (victor_user, _) = person(&state, 9812);
            project_of(&state, &victor_user, GUESS);
        }
        let chat = crate::people::tests_support::group(&state, &[&mallory_user]);
        answers.push(
            crate::people::agent_add(
                State(state),
                mallory,
                Path(chat),
                Json(crate::people::AddAgentBody {
                    project_id: GUESS.to_owned(),
                }),
            )
            .await
            .status(),
        );
    }
    assert_eq!(
        answers[0],
        StatusCode::NOT_FOUND,
        "Victor's agent is not Mallory's to add, and she is not told it exists"
    );
    assert_eq!(answers[0], answers[1], "taken and free names read alike");
}

/// `PUT /projects/{id}/voice` answered 404 for a name nobody had and 403 for a name someone
/// else had. (`GET` was already safe: it checks access before it looks anything up.)
#[tokio::test]
async fn voice_settings_do_not_say_whose_name_is_taken() {
    same!("voice (put)", |s, p, id| async move {
        crate::voice::put_agent_settings(
            State(s),
            p,
            Path(id),
            Json(crate::voice::AgentSettingsBody {
                chattiness: None,
                wake_name: None,
                vision_enabled: None,
            }),
        )
        .await
    });
}

/// Mallory's own agent still answers to its own name even when a stranger's agent, touched more
/// recently, shares it. Without this the uniform refusal would start eating real requests.
#[tokio::test]
async fn your_own_agent_still_answers_to_its_own_name() {
    let state = test_state(AuthMode::Github);
    let (mallory_user, mallory) = person(&state, 9821);
    let mine = project_of(&state, &mallory_user, "Tern");
    let (victor_user, _) = person(&state, 9822);
    let theirs = project_of(&state, &victor_user, "Tern");
    assert_ne!(mine, theirs);

    let answer = crate::voice::put_agent_settings(
        State(state),
        mallory,
        Path("Tern".to_owned()),
        Json(crate::voice::AgentSettingsBody {
            chattiness: Some("quiet".into()),
            wake_name: None,
            vision_enabled: None,
        }),
    )
    .await;
    assert_eq!(
        answer.status(),
        StatusCode::OK,
        "Mallory's own Tern, not the stranger's"
    );
}
