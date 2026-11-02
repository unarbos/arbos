//! The unwritten precondition of the uniform-404 work, written down.
//!
//! Those changes make every project route answer a name it may not serve with one indivisible
//! `404 no such project`, so the relay cannot be asked which agent names are taken. The grant
//! card is the deliberate exception: `agent_messages::grant_dto` resolves a `project:{id}`
//! grantee straight to that project's `name` and `@handle`, for any caller, with no permission
//! check. `project_lookup_tests` says why that is safe — the card takes an id and never a name,
//! because "a name would make it a directory".
//!
//! Safe, that is, **only while a device cannot get hold of another owner's project id.** The id
//! is the whole of the protection, and nothing enforced it. Anyone adding an id to a cross-owner
//! payload — a listing, a search result, an error string, a push — hands over the one input the
//! card turns into a name, and every uniform 404 in the relay keeps passing while they do it.
//!
//! So a stranger's device is given everything it can legitimately ask for, and **every string in
//! every answer** is scanned for the one id that would complete the chain. Scanning the whole
//! response rather than the fields that look like ids is the method, not fussiness: the id would
//! arrive in whichever field nobody thought to check, and an id in an error message spends the
//! same as an id in a body.
//!
//! Skills are the only place a device's request can return a row another owner created, so they
//! are what the sweep covers today. `answers` is a table on purpose: a new cross-owner route is
//! one line, and that line is cheaper to add than this file is to rediscover.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::Json;
use serde_json::{json, Value};
use std::collections::HashMap;

use arbos_core::ids::{now_ms, ulid};
use arbos_core::skill::Skill;

use crate::auth::Principal;
use crate::people::tests_support::{person, project_of};
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};

/// Victor's agent, named so a hit in the output is unmistakable and names its own field.
const VICTORS_AGENT: &str = "ZEBRAVICTORAGENT";

type ApiResult = Result<Response, (StatusCode, String)>;

/// Every string in a JSON tree, with the path it sits at.
fn strings(v: &Value, at: String, out: &mut Vec<(String, String)>) {
    match v {
        Value::String(s) => out.push((at, s.clone())),
        Value::Array(xs) => {
            for (i, x) in xs.iter().enumerate() {
                strings(x, format!("{at}[{i}]"), out);
            }
        }
        Value::Object(m) => {
            for (k, x) in m {
                strings(x, format!("{at}.{k}"), out);
            }
        }
        _ => {}
    }
}

/// Where `needle` shows up in `body`, by path. A refusal is scanned like any other answer.
fn sightings(label: &str, body: &Value, needle: &str) -> Vec<String> {
    let mut all = Vec::new();
    strings(body, String::new(), &mut all);
    all.iter()
        .filter(|(_, text)| text.contains(needle))
        .map(|(path, text)| format!("{label}{path} = {text:?}"))
        .collect()
}

/// The body if the route answered, the status and message if it refused. Both get scanned.
async fn answer(r: ApiResult) -> Value {
    match r {
        Ok(resp) => {
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
                .await
                .expect("body");
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        }
        Err((status, message)) => json!({ "status": status.as_u16(), "message": message }),
    }
}

struct Scene {
    state: AppState,
    /// Owns `VICTORS_AGENT`, one shared skill and one private one.
    victors_project: String,
    shared_skill: String,
    private_skill: String,
    /// A device with no tie to Victor at all.
    stranger: Principal,
}

fn scene() -> Scene {
    let state = test_state(AuthMode::Github);
    let (victor, _) = person(&state, 9801);
    let victors_project = project_of(&state, &victor, VICTORS_AGENT);
    let (mallory, stranger) = person(&state, 9802);
    project_of(&state, &mallory, "Mallory's agent");
    let shared_skill = skill(&state, &victors_project, "victors-shared", true);
    let private_skill = skill(&state, &victors_project, "victors-private", false);
    Scene {
        state,
        victors_project,
        shared_skill,
        private_skill,
        stranger,
    }
}

fn skill(state: &AppState, origin: &str, slug: &str, shared: bool) -> String {
    let now = now_ms();
    let s = Skill {
        id: ulid(),
        slug: slug.to_owned(),
        name: slug.to_owned(),
        body: "how Victor does it".into(),
        origin_project_id: Some(origin.to_owned()),
        shared,
        version: 1,
        created_at: now,
        updated_at: now,
        ..Skill::default()
    };
    state.db.insert_skill(&s).expect("skill");
    s.id
}

async fn searched(state: &AppState, who: &Principal, headers: &HeaderMap, q: &str) -> Value {
    answer(
        crate::skills::list(
            State(state.clone()),
            who.clone(),
            headers.clone(),
            Query(HashMap::from([("q".to_owned(), q.to_owned())])),
        )
        .await,
    )
    .await
}

/// Every answer a stranger's device can get out of the cross-owner surface. Add a route here
/// rather than writing a second test for it.
async fn answers(s: &Scene, who: &Principal, headers: &HeaderMap) -> Vec<(String, Value)> {
    let mut out = vec![(
        "GET /skills ".to_owned(),
        answer(
            crate::skills::list(
                State(s.state.clone()),
                who.clone(),
                headers.clone(),
                Query(HashMap::new()),
            )
            .await,
        )
        .await,
    )];
    out.push((
        "GET /skills/index ".to_owned(),
        answer(crate::skills::index(State(s.state.clone()), who.clone(), headers.clone()).await)
            .await,
    ));
    // Including a search for the id itself: a matcher and a filter are usually written by
    // different people, and a search is the classic way a scoped row comes back anyway.
    for q in [VICTORS_AGENT, "victors", &s.victors_project] {
        out.push((
            format!("GET /skills?q={q} "),
            searched(&s.state, who, headers, q).await,
        ));
    }
    for (what, id) in [("shared", &s.shared_skill), ("private", &s.private_skill)] {
        out.push((
            format!("GET /skills/{what} "),
            answer(
                crate::skills::get(
                    State(s.state.clone()),
                    who.clone(),
                    headers.clone(),
                    Path(id.clone()),
                )
                .await,
            )
            .await,
        ));
    }
    out
}

/// The invariant. A device that owns nothing of Victor's must not be handed the one string the
/// grant card converts into his agent's name.
#[tokio::test]
async fn a_strangers_device_is_never_handed_another_owners_project_id() {
    let s = scene();
    let headers = HeaderMap::new();
    let mut hits = Vec::new();
    for (label, body) in answers(&s, &s.stranger, &headers).await {
        hits.extend(sightings(&label, &body, &s.victors_project));
        // The name is the thing the id buys, so a route handing it over directly is the same
        // failure arriving early.
        hits.extend(sightings(&label, &body, VICTORS_AGENT));
    }
    assert!(
        hits.is_empty(),
        "a stranger's device was handed what the grant card turns into a name:\n{hits:#?}"
    );
}

/// The chain the invariant above does **not** close, pinned so it cannot quietly widen.
///
/// `SkillScope::Project` is `s.shared = 1 OR s.origin_project_id = ?1`, so a box reads every
/// shared skill on the relay together with the `origin_project_id` saying whose it is. That is
/// attribution on an explicitly shared row, and defensible by itself. `owner_only` then refuses
/// the box at the grant card, which is also defensible by itself.
///
/// The two together are not. A box reports to its own owner, and that step leaves the relay
/// without leaving the system: the owner ends up holding a foreign project id, the owner has a
/// device, and `grant_dto` turns an id into a name and a handle for any device with no
/// permission check. That is the enumeration path the uniform 404 exists to prevent, assembled
/// out of two parts that each look fine where they are written.
///
/// There is a second reason to pin it here rather than leave it to the reader. `skills.rs`
/// opens by saying a skill "is never shared across *people*", while the predicate above is
/// relay-wide for a box. One of those is wrong and the file does not say which — which is the
/// condition under which someone later edits the code to match the comment, or the comment to
/// match the code, not knowing a security property hangs on the answer.
///
/// Nothing here fixes it — `skills.rs` is held elsewhere. What these assertions buy is that the
/// chain cannot get *shorter* without a test failing: the origin must not reach an unshared
/// skill, and the card must keep refusing the box.
#[tokio::test]
async fn a_box_reads_a_foreign_project_id_and_only_the_last_step_is_refused() {
    let s = scene();
    let (mallory, _) = person(&s.state, 9803);
    let mallorys_project = project_of(&s.state, &mallory, "Mallory's other agent");
    let agent = Principal::Box {
        user_id: mallory.clone(),
    };
    let mut headers = HeaderMap::new();
    headers.insert("x-arbos-project", mallorys_project.parse().unwrap());

    let listed = answer(
        crate::skills::list(
            State(s.state.clone()),
            agent.clone(),
            headers.clone(),
            Query(HashMap::new()),
        )
        .await,
    )
    .await;
    // Step one of the chain, asserted so that closing it is a deliberate act and not a
    // side effect: today a box is told whose shared skill it is.
    assert!(
        !sightings("", &listed, &s.victors_project).is_empty(),
        "a box no longer learns whose shared skill it is — if that was on purpose, the chain \
         in this test's doc comment is closed at its first step and this assertion should go \
         with it: {listed}"
    );

    // Attribution on a shared row only. An unshared skill stays out of the listing entirely,
    // so its origin never reaches the box either.
    let private = answer(
        crate::skills::get(
            State(s.state.clone()),
            agent.clone(),
            headers.clone(),
            Path(s.private_skill.clone()),
        )
        .await,
    )
    .await;
    assert!(
        sightings("", &private, &s.victors_project).is_empty(),
        "an unshared skill told a box whose it is: {private}"
    );

    // The last step, and the only one the relay refuses: the card is a device route. This is
    // where the chain stops inside the relay, not where it stops.
    let refused = crate::agent_messages::put_grant(
        State(s.state.clone()),
        agent,
        Path(mallorys_project),
        Json(crate::agent_messages::GrantBody {
            grantee: format!("project:{}", s.victors_project),
            enabled: Some(true),
            note: None,
        }),
    )
    .await
    .expect_err("a box does not grant");
    assert_eq!(
        refused.0,
        StatusCode::FORBIDDEN,
        "a box may read the foreign id but may not spend it at the card itself"
    );
}
