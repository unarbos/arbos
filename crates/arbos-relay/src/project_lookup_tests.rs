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
use axum::response::Response;
use axum::Json;
use serde_json::json;

use crate::api::{
    self, ActivityBody, ArchiveBody, AuditQuery, KeepaliveBody, PatchChatBody, StopQuery,
    TimingBody,
};
use crate::auth::Principal;
use crate::people::tests_support::{group, person, project_of};
use crate::people::{
    self, AddAgentBody, AddMemberBody, AgentSettingsBody, CreateGroupBody, InviteBody,
    ProjectMemberBody,
};
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

// ---- the same question, asked of `people.rs` -------------------------------------------------
//
// Everything above is an `api.rs` route. `people.rs` has nine of its own that take a project
// from the caller, and until #2595 one of them — `agent_remove` — answered `403` for a name
// somebody held and `404` for a free one, which is the reported oracle exactly.
//
// That was not a design failure. `own_project` and `people::project_for` were already the right
// shape and already mirrored `api::project_for`; `agent_remove` had simply stopped borrowing
// them and hand-rolled the lookup instead. It was an uncovered file, and this is the cover.

/// Everything a `people.rs` route wants before it will look at a project: Mallory, a group she
/// owns, and a bystander to name in the routes that add somebody.
///
/// The group matters because the chat-hosted routes check chat membership before they touch a
/// project, so without one they refuse for the wrong reason and prove nothing. Mallory owning it
/// is the strong case rather than the convenient one: owning a group is the most authority a
/// stranger to Victor can give herself, and it is what gets her past `can_manage_members`.
///
/// The bystander exists on both relays. A route that names Victor would differ between them for
/// a reason that has nothing to do with the project.
struct Probe {
    state: AppState,
    mallory: Principal,
    chat: String,
    bystander: String,
    bystander_handle: String,
}

fn probe(victim: bool) -> Probe {
    let (state, mallory) = relay(victim);
    let mallory_user = mallory
        .user_id()
        .expect("Mallory signs in from a device")
        .to_owned();
    let chat = group(&state, &[&mallory_user]);
    let (bystander, _) = person(&state, 9503);
    let bystander_handle = state
        .db
        .user(&bystander)
        .expect("the bystander")
        .expect("the bystander")
        .login
        .expect("a github login");
    Probe {
        state,
        mallory,
        chat,
        bystander,
        bystander_handle,
    }
}

/// [`both`], for the `people.rs` handlers.
///
/// They return a bare `Response` rather than `ApiResult`, so there is no `Err` to unwrap and the
/// refusal has to be read off the response itself. Body as well as status, and that is not belt
/// and braces: a route can answer `404` on both relays and still give the game away in the
/// sentence.
async fn both_people<F, Fut>(route: F) -> (Refusal, Refusal)
where
    F: Fn(Probe) -> Fut,
    Fut: std::future::Future<Output = Response>,
{
    let mut out = Vec::new();
    for victim in [true, false] {
        let answer = route(probe(victim)).await;
        let status = answer.status();
        let body = axum::body::to_bytes(answer.into_body(), 1 << 16)
            .await
            .expect("a refusal body");
        out.push((status, String::from_utf8_lossy(&body).into_owned()));
    }
    (out.remove(0), out.remove(0))
}

/// Every `people.rs` route that takes a project from the caller. None of them may tell Mallory
/// whether `Lium` is a name somebody has.
#[tokio::test]
async fn the_people_routes_keep_the_same_silence() {
    macro_rules! same {
        ($label:literal, $call:expr) => {{
            let (taken, free) = both_people($call).await;
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

    // Four routes that reach `own_project`: the project has to be the caller's own.
    same!("create group with an agent", |p: Probe| async move {
        people::chats_create_group(
            State(p.state),
            p.mallory,
            Json(CreateGroupBody {
                kind: "group".into(),
                members: vec![],
                agent_ids: vec![GUESS.to_owned()],
                title: Some("probe".into()),
                avatar_leaf: None,
                avatar_leaf_color: None,
                description: None,
            }),
        )
        .await
    });
    same!("add an agent to a chat", |p: Probe| async move {
        people::agent_add(
            State(p.state),
            p.mallory,
            Path(p.chat),
            Json(AddAgentBody {
                project_id: GUESS.to_owned(),
            }),
        )
        .await
    });
    same!("agent settings", |p: Probe| async move {
        people::agent_settings(
            State(p.state),
            p.mallory,
            Path((p.chat, GUESS.to_owned())),
            Json(AgentSettingsBody {
                reply_policy: None,
                restrict_guests: None,
            }),
        )
        .await
    });
    same!("add members with an agent", |p: Probe| async move {
        people::member_add(
            State(p.state),
            p.mallory,
            Path(p.chat),
            Json(AddMemberBody {
                handle: None,
                handles: vec![],
                agent_ids: vec![GUESS.to_owned()],
            }),
        )
        .await
    });

    // Four that reach `people::project_for`: the project has to be one the caller is on at all.
    // The `owner_only` these refuse with afterwards is reachable only by an invited guest, who
    // can see the project anyway — so it is the lookup, not the wording, that has to hold.
    same!("share a project", |p: Probe| async move {
        people::project_member_add(
            State(p.state),
            p.mallory,
            Path(GUESS.to_owned()),
            Json(ProjectMemberBody {
                handle: p.bystander_handle,
            }),
        )
        .await
    });
    same!("unshare a project", |p: Probe| async move {
        people::project_member_remove(
            State(p.state),
            p.mallory,
            Path((GUESS.to_owned(), p.bystander)),
        )
        .await
    });
    same!("list a project's members", |p: Probe| async move {
        people::project_members_list(State(p.state), p.mallory, Path(GUESS.to_owned())).await
    });
    same!("invite into a project", |p: Probe| async move {
        people::invite_create(
            State(p.state),
            p.mallory,
            Json(InviteBody {
                handle: p.bystander_handle,
                chat_id: None,
                project_id: Some(GUESS.to_owned()),
            }),
        )
        .await
    });
}

// The ninth route, `agent_remove`, is deliberately not above, and this comment is here so the
// gap cannot be mistaken for an oversight twice.
//
// Writing its case is what found that #2595 does not finish the job. #2595 closes the split a
// plain member sees — `can_manage_members` fails and every refusal becomes `no such project`.
// An owner or admin of any group takes a different path: `can_manage_members` passes, the target
// is nobody they moderate, `may_remove_agent` returns `Ok`, and `remove_member` then answers
//
//     taken: 404 not_found "that agent is not in this chat"
//     free:  404 not_found "no such project"
//
// Same status, different sentence, and anyone can make a group. It is the reported oracle on
// another route, which `both_people` catches only because it compares bodies and not just
// statuses. The case belongs in the same change as its fix, so it lands with that fix rather
// than sitting here red or ignored.
