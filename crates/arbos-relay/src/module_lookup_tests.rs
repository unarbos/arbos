//! The same silence, in the modules that keep their own project lookup instead of going through
//! `api::project_for`. Each of these answered a name someone else held differently from a name
//! nobody held, which is the enumeration oracle the reporter used on `pushes` (reported
//! 2026-11-02), just spelt with a different status line.
//!
//! Every route is asked the same question on two relays that differ in one fact: on the first,
//! Victor owns an agent called `Lium`; on the second, nobody does. The answers must match.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
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

/// `POST /skills` files a skill under the project the caller names, and every later ownership
/// question about that skill is answered from that one field. It resolved the name with the
/// plain lookup, which is not scoped to the caller, so the name went to whichever `Tern` was
/// active last. Mallory, who has her own `Tern` and is a guest of Victor's, filed her skill in
/// Victor's library — where his box both reads it and may edit it.
///
/// This one needs no name/id collision, so narrowing which strings a name may match does not
/// reach it. Two live projects sharing a name is enough.
#[tokio::test]
async fn a_skill_is_filed_under_the_callers_own_agent() {
    const NAME: &str = "Tern";
    let state = test_state(AuthMode::Github);
    let (mallory_user, mallory) = person(&state, 9831);
    let mine = project_of(&state, &mallory_user, NAME);
    let (victor_user, _) = person(&state, 9832);
    let theirs = project_of(&state, &victor_user, NAME);
    state
        .db
        .add_project_member(&theirs, &mallory_user, &victor_user)
        .expect("Mallory is a guest of Victor's Tern");

    // The plain lookup breaks a name tie on `last_active_at`, and the clock counts whole
    // milliseconds, so Victor's row is touched until it is the newer of the two.
    loop {
        state.db.set_project_state(&theirs, "awake").expect("touch");
        if state.db.project(NAME).expect("lookup").expect("a row").id == theirs {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }

    crate::skills::create(
        State(state.clone()),
        mallory,
        HeaderMap::new(),
        Json(crate::skills::SkillBody {
            name: Some("Mallory's notes".into()),
            slug: Some("mallory-notes".into()),
            origin_project_id: Some(NAME.to_owned()),
            ..Default::default()
        }),
    )
    .await
    .expect("Mallory may file a skill under her own agent");

    let filed = state
        .db
        .skill("mallory-notes")
        .expect("read")
        .expect("the skill was written");
    assert_eq!(
        filed.origin_project_id.as_deref(),
        Some(mine.as_str()),
        "Mallory's skill belongs to Mallory's Tern, not the stranger's"
    );
}

// ---------------------------------------------------------------------------------------------
// The handler modules this file did not reach the first time (swept 2026-11-02).
//
// The sweep asked every route that takes a project id or name from the caller the same question
// the tests above ask, on the same two relays. Routes whose answers already match are pinned
// here. The ones that do not are listed at the foot of the file with the answers they gave; they
// want a source change, which is not this file's to make.
// ---------------------------------------------------------------------------------------------

/// Like [`both`], but keeps the body as well as the status.
///
/// Status alone is not a strong enough comparison for this question. `GET
/// /projects/{file}/avatar.jpg` answers 404 either way and still gives the game away, because
/// one 404 says "no picture" and the other says "no such agent". A route is only silent when
/// both halves of the answer match.
async fn both_answers<F, Fut>(route: F) -> ((StatusCode, String), (StatusCode, String))
where
    F: Fn(AppState, Principal, String) -> Fut,
    Fut: std::future::Future<Output = Response>,
{
    let mut out = Vec::new();
    for victim in [true, false] {
        let (state, mallory) = relay(victim);
        let answer = route(state, mallory, GUESS.to_owned()).await;
        let status = answer.status();
        let body = axum::body::to_bytes(answer.into_body(), 64 * 1024)
            .await
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        out.push((status, body));
    }
    (out.remove(0), out.remove(0))
}

macro_rules! same_answer {
    ($label:literal, $call:expr) => {{
        let (taken, free) = both_answers($call).await;
        assert_eq!(
            taken, free,
            concat!(
                $label,
                ": a name someone holds must read like a name nobody holds"
            )
        );
    }};
}

/// The routes that settle access before they look a project up. A stranger is refused the same
/// way whether the name is Victor's or nobody's, because `AppState::allows` answers `false` for
/// a project that is not there just as it does for one that is not yours — so the refusal that
/// reaches the caller cannot be used to tell the two apart.
///
/// This is the shape worth keeping. It is also why these are cheap to pin and worth pinning:
/// moving the lookup above the gate is a one-line edit that would read like a tidy-up.
#[tokio::test]
async fn the_gated_project_routes_keep_the_same_silence() {
    same_answer!("voice (get)", |s, p, id| async move {
        crate::voice::get_agent_settings(State(s), p, Path(id)).await
    });
    same_answer!("notify (project chat)", |s, p, id| async move {
        crate::notify::notify_project_chat(
            State(s),
            p,
            Path((id, "c1".to_owned())),
            Json(
                serde_json::from_value(serde_json::json!({ "message_id": "m1", "text": "hi" }))
                    .expect("NotifyBody"),
            ),
        )
        .await
    });
    same_answer!("calls (start)", |s, p, id| async move {
        crate::calls::start_project_call(
            State(s),
            p,
            Path((id, "c1".to_owned())),
            HeaderMap::new(),
            None,
        )
        .await
    });
}

/// `POST /projects/{id}/local/{device}` wants a box token, and the refusal for the missing token
/// sits one line below the lookup. A device probe would be turned away there and never reach the
/// branch this file is about, so the caller here is Mallory's own box: the most privileged
/// principal who is still a stranger to Victor.
#[tokio::test]
async fn running_an_op_on_a_computer_does_not_say_whose_name_is_taken() {
    same_answer!("local (exec)", |s, p, id| async move {
        let as_a_box = Principal::Box {
            user_id: p.user_id().unwrap_or_default().to_owned(),
        };
        crate::local::exec(
            State(s),
            as_a_box,
            Path((id, "d1".to_owned())),
            Json(
                serde_json::from_value(serde_json::json!({ "op": "list_dir", "path": "/" }))
                    .expect("LocalOp"),
            ),
        )
        .await
        .into_response()
    });
}

/// The group routes go through `api::lookup_project`, which was built for exactly this and
/// collapses "no such host", "no such chat", "not a group" and "nothing to do with you" into one
/// answer. Pinned because `groups.rs` reaches that helper through two different doors
/// (`project_for` on the way in, `open_group` afterwards) and only one of them is obvious.
#[tokio::test]
async fn the_group_routes_keep_the_same_silence() {
    same_answer!("groups (create)", |s, p, id| async move {
        crate::groups::create_group(
            State(s),
            p,
            Path(id),
            Json(
                serde_json::from_value(serde_json::json!({ "participants": ["x"] }))
                    .expect("CreateGroupBody"),
            ),
        )
        .await
        .into_response()
    });
    same_answer!("groups (get)", |s, p, id| async move {
        crate::groups::get_group(State(s), p, Path((id, "c1".to_owned())))
            .await
            .into_response()
    });
}

// ---------------------------------------------------------------------------------------------
// Still open, measured 2026-11-02 with the harness above. Not asserted here, because the
// assertion is the fix's to add and a red test in `main` helps nobody.
//
// Eighteen routes answer `403 not your project` for a name Victor holds and `404 no such
// project` for one nobody holds. They come from six lookups:
//
//   goals::authorize       — list, create, get, patch, delete, append_log      (6 routes)
//   routines::authorize    — list, create, patch, delete, run_now, runs,
//                            report_run                                        (7 routes)
//   inspect::project_inspect                                                   (1 route)
//   inspect::project_job                                                       (1 route)
//   images::avatar_generate                                                    (1 route)
//   secrets::project_secrets  (403 "not your agent" / 404 "no such agent")     (1 route)
//
// All six are the same two statements in the same order: a bare `state.db.project(caller_input)`
// that 404s on a miss, then a separate refusal on a hit. `goals::authorize` and
// `routines::authorize` are character-for-character the same function, so thirteen of the
// eighteen routes are two edits.
//
// `GET /projects/{file}/avatar.jpg` (`images::project_avatar`) belongs to the same family and
// wants a different answer. It takes no principal at all, and it answers 404 "no picture" for a
// name someone holds against 404 "no such agent" for one nobody does — so the oracle is open to
// anyone on the internet, not just to a signed-in stranger. Wording will not close it: an agent
// that has a picture answers with the picture. What closes it is matching on the id only
// (`db::project_by_id`), which is what that method's own comment already asks for.
//
// Two routes could not be measured rather than being found clean: `browser::browser_stream` and
// `rooms::ws_room` take a `WebSocketUpgrade`, which cannot be built without a real upgradable
// connection, so a harness that calls handlers directly cannot reach them. Read rather than run,
// `browser_stream` has the leaking shape (lookup, then `allows`) and `ws_room` has the safe one
// (`allows`, then lookup).
// ---------------------------------------------------------------------------------------------
