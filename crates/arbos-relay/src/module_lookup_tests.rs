//! The same silence, in the modules that keep their own project lookup instead of going through
//! `api::project_for`. Each of these answered a name someone else held differently from a name
//! nobody held, which is the enumeration oracle the reporter used on `pushes` (reported
//! 2026-11-02), just spelt with a different status line.
//!
//! Every route is asked the same question on two relays that differ in one fact: on the first,
//! Victor owns an agent called `Lium`; on the second, nobody does. The answers must match.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
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

/// One route's whole answer: the status line and the body it was sent with.
///
/// The body is half the answer, not decoration. `GET /projects/{file}/avatar.jpg` refuses a name
/// someone holds and a name nobody holds with the same 404 and tells them apart in the message —
/// "no picture" against "no such agent" — so a comparison that stops at the status passes while
/// the oracle is wide open. Every assertion in this file compares both halves.
type Answer = (StatusCode, String);

async fn read_answer(response: Response) -> Answer {
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    (status, body)
}

/// Ask one route for `Lium` on both relays and hand back the two answers.
async fn both<F, Fut>(route: F) -> (Answer, Answer)
where
    F: Fn(AppState, Principal, String) -> Fut,
    Fut: std::future::Future<Output = Response>,
{
    let mut out = Vec::new();
    for victim in [true, false] {
        let (state, mallory) = relay(victim);
        out.push(read_answer(route(state, mallory, GUESS.to_owned()).await).await);
    }
    (out.remove(0), out.remove(0))
}

/// `same!("label", route)` requires the two answers to match. The three-argument form also pins
/// which refusal they settle on, for the routes where that is part of the contract.
macro_rules! same {
    ($label:literal, $call:expr) => {{
        let (taken, free) = both($call).await;
        assert_eq!(
            taken, free,
            concat!($label, ": taken and free names must read alike")
        );
    }};
    ($label:literal, $call:expr, $status:expr) => {{
        let (taken, free) = both($call).await;
        assert_eq!(
            taken.0, $status,
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
    same!(
        "members (add)",
        |s, p, id| async move {
            crate::people::project_member_add(
                State(s),
                p,
                Path(id),
                Json(crate::people::ProjectMemberBody {
                    handle: "someone".into(),
                }),
            )
            .await
        },
        StatusCode::NOT_FOUND
    );
    same!(
        "members (list)",
        |s, p, id| async move { crate::people::project_members_list(State(s), p, Path(id)).await },
        StatusCode::NOT_FOUND
    );
    same!(
        "members (remove)",
        |s, p, id| async move {
            crate::people::project_member_remove(State(s), p, Path((id, "u_other".to_owned())))
                .await
        },
        StatusCode::NOT_FOUND
    );
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
            read_answer(
                crate::people::agent_add(
                    State(state),
                    mallory,
                    Path(chat),
                    Json(crate::people::AddAgentBody {
                        project_id: GUESS.to_owned(),
                    }),
                )
                .await,
            )
            .await,
        );
    }
    assert_eq!(
        answers[0].0,
        StatusCode::NOT_FOUND,
        "Victor's agent is not Mallory's to add, and she is not told it exists"
    );
    assert_eq!(answers[0], answers[1], "taken and free names read alike");
}

/// `PUT /projects/{id}/voice` answered 404 for a name nobody had and 403 for a name someone
/// else had. (`GET` was already safe: it checks access before it looks anything up.)
#[tokio::test]
async fn voice_settings_do_not_say_whose_name_is_taken() {
    same!(
        "voice (put)",
        |s, p, id| async move {
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
        },
        StatusCode::NOT_FOUND
    );
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

/// The routes that settle access before they look a project up. A stranger is refused the same
/// way whether the name is Victor's or nobody's, because `AppState::allows` answers `false` for
/// a project that is not there just as it does for one that is not yours — so the refusal that
/// reaches the caller cannot be used to tell the two apart.
///
/// This is the shape worth keeping. It is also why these are cheap to pin and worth pinning:
/// moving the lookup above the gate is a one-line edit that would read like a tidy-up.
#[tokio::test]
async fn the_gated_project_routes_keep_the_same_silence() {
    same!("voice (get)", |s, p, id| async move {
        crate::voice::get_agent_settings(State(s), p, Path(id)).await
    });
    same!("notify (project chat)", |s, p, id| async move {
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
    same!("calls (start)", |s, p, id| async move {
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
    same!("local (exec)", |s, p, id| async move {
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
    same!("groups (create)", |s, p, id| async move {
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
    same!("groups (get)", |s, p, id| async move {
        crate::groups::get_group(State(s), p, Path((id, "c1".to_owned())))
            .await
            .into_response()
    });
}

/// `GET /outside-agents/{id}` and the five routes beside it (`patch`, `revoke`, `rotate`,
/// `wake/test`, `log`) all resolve through `outside::owned`, which does split its answers: a
/// connected agent of Victor's is `403 not your agent` and an unconnected id is `404 no such
/// connected agent`.
///
/// It is pinned by *name* rather than by id, which is the whole point. `Db::outside_agent` reads
/// `WHERE project_id = ?1` and does not match on the name, so a guessed name never reaches the
/// split and both relays answer alike — the namespace that made the project oracle worth
/// reporting is not enumerable here. What this test defends is that property, not the wording:
/// route `owned` through anything that resolves a name (`Db::project` does) and this goes red.
#[tokio::test]
async fn the_outside_agent_routes_are_not_reachable_by_name() {
    let mut answers = Vec::new();
    for victim in [true, false] {
        let state = test_state(AuthMode::Github);
        let (mallory_user, mallory) = person(&state, 9841);
        project_of(&state, &mallory_user, "Mallory's agent");
        if victim {
            let (victor_user, _) = person(&state, 9842);
            let victors = project_of(&state, &victor_user, GUESS);
            state
                .db
                .insert_outside_agent(&victors, &victor_user, "grok", "deadbeef")
                .expect("Victor's Lium is a connected agent");
        }
        answers.push(
            read_answer(
                crate::outside::get(State(state), mallory, Path(GUESS.to_owned()))
                    .await
                    .into_response(),
            )
            .await,
        );
    }
    assert_eq!(
        answers[0].0,
        StatusCode::NOT_FOUND,
        "a connected agent Mallory does not own is not found by name"
    );
    assert_eq!(answers[0], answers[1], "taken and free names read alike");
}

/// `POST /chats/{id}/notify {project_id}` lets a box say which of its agents spoke, and the
/// project named there is the caller's own string. A device never reaches that lookup — only the
/// box arm of `notify::sender_for` performs it — so the caller here is a box whose agent sits in
/// the chat: the most privileged principal who is still a stranger to Victor.
///
/// Getting that setup wrong is how this test would go quietly useless. Without the `agent:` row
/// the box is turned away by `room_access` with `not in this chat`, identically on both relays,
/// and the assertion below passes without the lookup ever running. It is asserted on the refusal
/// `sender_for` gives, not merely on the two answers matching, so that it fails rather than
/// passes if the setup stops reaching it.
#[tokio::test]
async fn naming_someone_elses_agent_as_the_speaker_does_not_say_whose_name_is_taken() {
    let mut answers = Vec::new();
    for victim in [true, false] {
        let state = test_state(AuthMode::Github);
        let (mallory_user, _) = person(&state, 9861);
        let mine = project_of(&state, &mallory_user, "Mallory's agent");
        if victim {
            let (victor_user, _) = person(&state, 9862);
            project_of(&state, &victor_user, GUESS);
        }
        let chat = crate::people::tests_support::group(&state, &[&mallory_user]);
        state
            .db
            .add_member(
                &chat,
                &format!("agent:{mine}"),
                "agent",
                &mallory_user,
                &serde_json::json!({}),
            )
            .expect("Mallory's agent is in her chat, so her box may speak there");
        let as_a_box = Principal::Box {
            user_id: mallory_user.clone(),
        };
        answers.push(
            read_answer(
                crate::notify::notify_people_chat(
                    State(state),
                    as_a_box,
                    Path(chat),
                    Json(
                        serde_json::from_value(serde_json::json!({
                            "message_id": "m1", "text": "hi", "project_id": GUESS,
                        }))
                        .expect("NotifyBody"),
                    ),
                )
                .await,
            )
            .await,
        );
    }
    assert!(
        answers[0].1.contains("not this box's agent"),
        "the box must have reached `sender_for`, not been stopped at the chat gate: {:?}",
        answers[0]
    );
    assert_eq!(answers[0], answers[1], "taken and free names read alike");
}

/// The seventeen routes the sweep measured as one family, now answering a name they hide like a
/// name nobody holds. One assertion per lookup is the right grain: the lookup, not the route, is
/// what decides the answer. Each pins the refusal as well as the match, because an equality
/// between two refusals says nothing on its own — two identical answers from a route that was
/// never reached would pass just as happily.
///
/// What each lookup asks now:
///
///   `goals` (6 routes) and `routines` (7) — `api::resolve_project_id`, the helper the routes
///   that already answered alike go through; both copies of `authorize` are gone.
///   `inspect` rows and job (2) — `inspect::project_and_scope`, which keeps the way in for
///   someone who shares a chat with the agent and hides every other caller.
///   `images::avatar_generate` (1) — `api::project_owned_by`.
///   `secrets::project_secrets` (1) — `secrets::visible_agent`, answering in that module's own
///   noun: "no such agent" rather than "no such project".
#[tokio::test]
async fn the_modules_with_their_own_lookup_keep_the_same_silence() {
    same!(
        "goals (list)",
        |s, p, id| async move {
            crate::goals::list(State(s), p, Path(id))
                .await
                .unwrap_or_else(|e| e.into_response())
        },
        StatusCode::NOT_FOUND
    );
    same!(
        "routines (list)",
        |s, p, id| async move {
            crate::routines::list(State(s), p, Path(id))
                .await
                .unwrap_or_else(|e| e.into_response())
        },
        StatusCode::NOT_FOUND
    );
    same!(
        "inspect (rows)",
        |s, p, id| async move {
            crate::inspect::project_inspect(State(s), p, Path(id), Query(HashMap::new())).await
        },
        StatusCode::NOT_FOUND
    );
    same!(
        "inspect (job)",
        |s, p, id| async move {
            crate::inspect::project_job(
                State(s),
                p,
                Path((id, "j1".to_owned())),
                Query(HashMap::new()),
            )
            .await
        },
        StatusCode::NOT_FOUND
    );
    same!(
        "avatar (generate)",
        |s, p, id| async move {
            crate::images::avatar_generate(
                State(s),
                p,
                Path(id),
                Json(crate::images::AvatarGenerateBody {
                    prompt: "a leaf".into(),
                }),
            )
            .await
        },
        StatusCode::NOT_FOUND
    );
    same!(
        "secrets (agent profile)",
        |s, p, id| async move {
            crate::secrets::project_secrets(State(s), p, Path(id))
                .await
                .unwrap_or_else(|e| e.into_response())
        },
        StatusCode::NOT_FOUND
    );
}

/// A guest still reads the agent they were invited to, and its owner still reads their own. The
/// uniform refusal above hides other people's agents; it must not start hiding the caller's.
#[tokio::test]
async fn the_people_inside_an_agent_still_read_it() {
    let state = test_state(AuthMode::Github);
    let (victor_user, victor) = person(&state, 9891);
    let project = project_of(&state, &victor_user, GUESS);
    let (guest_user, guest) = person(&state, 9892);
    state
        .db
        .add_project_member(&project, &guest_user, &victor_user)
        .expect("Victor invites a guest");

    for (who, principal) in [("the owner", victor), ("a guest", guest)] {
        crate::goals::list(
            State(state.clone()),
            principal.clone(),
            Path(GUESS.to_owned()),
        )
        .await
        .unwrap_or_else(|e| panic!("{who} reads the goals of their own agent: {e:?}"));
        crate::secrets::project_secrets(State(state.clone()), principal, Path(GUESS.to_owned()))
            .await
            .unwrap_or_else(|e| panic!("{who} reads the secrets card of their own agent: {e:?}"));
    }
}

// ---------------------------------------------------------------------------------------------
// What the sweep of 2026-11-02 found, and how firmly.
//
// Three kinds of entry below, which are three different claims and should not be read as one:
// MEASURED LEAK (run against both relays, answers differ), MEASURED AND NOT ENUMERABLE (answers
// differ, but only for input a stranger cannot guess), and UNMEASURED (read, never run).
//
// -- MEASURED LEAK: nineteen routes, eight lookups ----------------------------------------------
//
// Eighteen of the nineteen are closed and asserted here now:
// `the_modules_with_their_own_lookup_keep_the_same_silence` (seventeen routes, six lookups) and
// `watching_a_browser_does_not_say_whose_name_is_taken` (one route, at the foot with the rest of
// the socket work). What each lookup asks instead is in those two tests' own notes.
//
// The seventeen were one family — `403 not your project` for a name Victor holds, `404 no such
// project` for one nobody holds — spread over six lookups: `goals::authorize` (6 routes),
// `routines::authorize` (7), `inspect::project_inspect` (1), `inspect::project_job` (1),
// `images::avatar_generate` (1), `secrets::project_secrets` (1, in that module's noun: 403 "not
// your agent" against 404 "no such agent"). Every one of them was the same two statements in the
// same order — a bare `state.db.project(caller_input)` that 404s on a miss, then a separate
// refusal on a hit.
//
// The eighteenth was `GET /projects/{id}/browser/stream` (`browser::browser_stream`), measured
// over a real socket by the harness at the foot of this file: `403` with an **empty body** for a
// name Victor holds against `404 no such project` for one nobody holds. The empty body was a
// bare `StatusCode::FORBIDDEN`, which the error layer leaves alone, so that route's refusal had
// no JSON envelope and was alone in the API in that; it is settled in the same change.
//
// The nineteenth is still open here, and wants a different remedy:
//
// `GET /projects/{file}/avatar.jpg` (`images::project_avatar`) is measured too, and wants a
// different remedy. It takes no principal at all, and answers 404 "no picture" for a name
// someone holds against 404 "no such agent" for one nobody does — so the oracle is open to
// anyone on the internet, not just to a signed-in stranger, and it is invisible to any check
// that compares statuses. Wording will not close it: an agent that has a picture answers with
// the picture. What closes it is matching on the id only (`db::project_by_id`), which is what
// that method's own comment already asks for. That is an API-shape change, not a message change.
//
// -- MEASURED, AND NOT ENUMERABLE ---------------------------------------------------------------
//
// `outside::owned` splits its answers (403 "not your agent" against 404 "no such connected
// agent") when asked by project *id*. Recorded rather than reported, for the reason the test
// above pins: `Db::outside_agent` matches on `project_id` alone, ids are ULIDs, and there is no
// name path into it — so there is no namespace to walk. This is the same judgement as
// `people::member_chat`, which splits on chat ids for the same harmless reason. Worth writing
// down so nobody rediscovers either and files it as new.
//
// -- UNMEASURED: READ, NOT RUN ------------------------------------------------------------------
//
// Empty. The two routes that sat here — `browser::browser_stream` and `rooms::ws_room` — are now
// measured over a real socket by the harness further down, and have moved: `ws_room` is silent
// and pinned by a test, `browser_stream` is a measured leak and is listed above.
//
// Both readings turned out to be right, which is not the point. They were guesses, and a guess
// that happens to be correct is still not evidence; `browser_stream` in particular would have
// gone to whoever fixes it as "suspicious shape, unverified" rather than "403 with an empty body
// against 404 no such project, measured". Keep this heading even while it is empty, so the next
// route that cannot be reached has somewhere honest to sit.
// ---------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------
// A router-level harness, for the two routes that cannot be called directly.
//
// `browser::browser_stream` and `rooms::ws_room` take a `WebSocketUpgrade`, and that extractor
// refuses (`ConnectionNotUpgradable`) unless the request arrived over a connection hyper can
// actually hand over. Calling the handler with a hand-built `Parts` does not work and cannot be
// made to work, so these two sat in this file as read-but-never-run while everything else was
// measured. This serves them over a real loopback socket instead and dials them with a real
// client, which is the only way to find out what they do.
// ---------------------------------------------------------------------------------------------

/// The two upgrading routes, wrapped in the error layer the real app wraps them in.
///
/// Deliberately not the whole application router, which is built inline inside `main` and cannot
/// be reached from a test without reshaping `main.rs`. The error layer is here because it is the
/// difference between a handler's answer and the answer a client sees — it is what turns a plain
/// `(404, "no such project")` into the JSON envelope — and this file compares the bytes.
fn upgrading_routes(state: AppState) -> axum::Router {
    axum::Router::new()
        .route(
            "/projects/{id}/browser/stream",
            axum::routing::get(crate::browser::browser_stream),
        )
        .route(
            "/room/{project}/{chat}",
            axum::routing::get(crate::rooms::ws_room),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::api_error_layer::rewrite_plain_api_errors,
        ))
        .with_state(state)
}

async fn serve(state: AppState) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback port");
    let addr = listener.local_addr().expect("addr");
    let app = upgrading_routes(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

/// A device token that will actually authenticate. `tests_support::person` leaves the token
/// empty, which is fine for a direct call and useless over a socket.
fn token_for(state: &AppState, user_id: &str) -> String {
    let token = format!("adv_{}", arbos_core::ids::ulid());
    state
        .db
        .insert_device(
            &arbos_core::ids::ulid(),
            "harness",
            &arbos_core::auth::hash(&token),
            user_id,
            "github",
        )
        .expect("device");
    token
}

/// Dial one route and report what came back: `101` when the upgrade completed, otherwise the
/// refusal the client was handed.
///
/// A transport failure panics rather than being folded into the answer. "The relay never
/// answered" and "the relay refused" are different facts, and a harness that returns the first
/// as if it were the second is the trap this whole file exists to avoid.
async fn dial(addr: std::net::SocketAddr, path: &str, token: &str) -> (u16, String) {
    let url = format!("ws://{addr}{path}?token={token}");
    match tokio_tungstenite::connect_async(url).await {
        Ok((stream, _)) => {
            drop(stream);
            (101, String::new())
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => {
            let status = r.status().as_u16();
            let body = r
                .body()
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default();
            (status, body)
        }
        Err(e) => panic!("harness never reached the relay: {e}"),
    }
}

/// Before any websocket measurement is worth reading, the harness has to be shown to reach the
/// inside of the handlers. A socket that refuses every request looks exactly like a relay that
/// refuses every request, and a differential between two such refusals is vacuous: two matching
/// 403s prove nothing unless something pins which 403.
///
/// So this asserts four separate things, each of which is a way the harness could be lying:
/// that an upgrade can complete at all, that a non-upgrading answer can still come from deep
/// inside a handler, that a route the router does not serve is distinguishable from a handler's
/// own 404, and that the token is really being read.
#[tokio::test]
async fn the_websocket_harness_reaches_the_handlers() {
    let state = test_state(AuthMode::Github);
    let (mallory_user, _) = person(&state, 9871);
    let mine = project_of(&state, &mallory_user, "Mallory's agent");
    let token = token_for(&state, &mallory_user);
    let addr = serve(state).await;

    let upgraded = dial(addr, &format!("/room/{mine}/project"), &token).await;
    assert_eq!(
        upgraded.0, 101,
        "the harness must be able to complete a real upgrade, or every refusal it reports is \
         unexplained: {upgraded:?}"
    );

    let deep = dial(addr, &format!("/projects/{mine}/browser/stream"), &token).await;
    assert!(
        deep.1.contains("no sandbox"),
        "a non-upgrading answer must still come from inside the handler, past the branch these \
         tests are about: {deep:?}"
    );

    let unserved = dial(addr, "/nope/nope", &token).await;
    assert_eq!(
        unserved,
        (404, String::new()),
        "a route the router does not serve must be tellable apart from a handler's own 404"
    );

    let anonymous = dial(addr, &format!("/projects/{mine}/browser/stream"), "").await;
    assert_eq!(
        anonymous.0, 401,
        "the token must really be read: {anonymous:?}"
    );
}

/// Measured at last, rather than guessed from the shape: `GET /room/{project}/{chat}` settles
/// access before it looks the project up, so a stranger is refused identically whether the name
/// is Victor's or nobody's.
///
/// Asserted on the refusal `ws_room` actually gives, not merely on the two answers matching. If
/// the harness ever stops reaching the handler, both relays will agree on something else and
/// this must fail rather than pass.
#[tokio::test]
async fn opening_a_room_does_not_say_whose_name_is_taken() {
    let mut answers = Vec::new();
    for victim in [true, false] {
        let state = test_state(AuthMode::Github);
        let (mallory_user, _) = person(&state, 9881);
        project_of(&state, &mallory_user, "Mallory's agent");
        if victim {
            let (victor_user, _) = person(&state, 9882);
            project_of(&state, &victor_user, GUESS);
        }
        let token = token_for(&state, &mallory_user);
        let addr = serve(state).await;
        answers.push(dial(addr, &format!("/room/{GUESS}/project"), &token).await);
    }
    assert!(
        answers[0].1.contains("not your project"),
        "the dial must have reached `ws_room`'s own refusal: {:?}",
        answers[0]
    );
    assert_eq!(answers[0], answers[1], "taken and free names read alike");
}

/// `GET /projects/{id}/browser/stream` asked `allows` one line after it looked the project up,
/// which is the eighteenth route of this family and the only one that had to be measured over a
/// socket: `403` with an empty body for a name Victor holds against `404 no such project` for
/// one nobody holds. It now resolves through `api::project_for`, like the routes above.
///
/// The pin is on the free relay, not the taken one. A name nobody holds answered `no such
/// project` before this change and answers it after, so it anchors the assertion; the taken
/// side's answer is what the change moves. Without a pin the equality would be vacuous — a dial
/// that stopped reaching the handler would have both relays agreeing on some other refusal.
#[tokio::test]
async fn watching_a_browser_does_not_say_whose_name_is_taken() {
    let mut answers = Vec::new();
    for victim in [true, false] {
        let state = test_state(AuthMode::Github);
        let (mallory_user, _) = person(&state, 9893);
        project_of(&state, &mallory_user, "Mallory's agent");
        if victim {
            let (victor_user, _) = person(&state, 9894);
            project_of(&state, &victor_user, GUESS);
        }
        let token = token_for(&state, &mallory_user);
        let addr = serve(state).await;
        answers.push(dial(addr, &format!("/projects/{GUESS}/browser/stream"), &token).await);
    }
    assert!(
        answers[1].1.contains("no such project"),
        "the dial must have reached the route's own refusal for a free name: {:?}",
        answers[1]
    );
    assert_eq!(answers[0], answers[1], "taken and free names read alike");
}
