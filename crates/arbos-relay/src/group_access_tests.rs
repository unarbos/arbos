//! The reported cross-tenant chain against group chats, step by step, on a local relay.
//!
//! An outside reporter reached other people's agents on the live relay like this: project names
//! were enumerable, a group accepted a participant project its creator did not own, and the
//! victim's agent then answered inside the attacker's group. Each test below walks one step of
//! that walk and asserts the door is shut.
//!
//! Two rules decide every assertion here:
//!
//! 1. **One refusal, one wording.** A project that is not there and a project the caller may
//!    not touch answer identically. Anywhere the two differ, the difference is a lookup oracle.
//! 2. **Consent, not reachability.** An agent joins a stranger's group only when its own owner
//!    has said yes through the "Who may message X" grants (`agent_messages.rs`), which is the
//!    consent flow the product already had.

use axum::body::to_bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::Value;

use crate::auth::Principal;
use crate::groups::{
    create_group, get_group, group_reply, group_send, patch_group, CreateGroupBody, GroupReplyBody,
    GroupSendBody, PatchGroupBody,
};
use crate::people::tests_support::{person, project_of};
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};

/// Two tenants who have never met: Mallory with her agent, Victor with his.
struct Two {
    state: AppState,
    mallory: Principal,
    mallory_box: Principal,
    mallory_agent: String,
    victor: Principal,
    victor_agent: String,
}

/// Victor's agent is called `Lium` — the name is the only thing Mallory needs to know.
const VICTIM_NAME: &str = "Lium";

fn two_tenants() -> Two {
    let state = test_state(AuthMode::Github);
    let (mallory_user, mallory) = person(&state, 9301);
    let (victor_user, victor) = person(&state, 9302);
    let mallory_agent = project_of(&state, &mallory_user, "Mallory's agent");
    let victor_agent = project_of(&state, &victor_user, VICTIM_NAME);
    Two {
        state,
        mallory,
        mallory_box: Principal::Box {
            user_id: mallory_user,
        },
        mallory_agent,
        victor,
        victor_agent,
    }
}

async fn create(
    state: &AppState,
    who: &Principal,
    host: &str,
    participants: &[&str],
) -> Result<Value, (StatusCode, String)> {
    let resp = create_group(
        State(state.clone()),
        who.clone(),
        Path(host.to_owned()),
        Json(CreateGroupBody {
            title: None,
            participants: participants.iter().map(|s| (*s).to_owned()).collect(),
        }),
    )
    .await?;
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    Ok(serde_json::from_slice(&bytes).expect("group json"))
}

/// A group hosted by `host` with `participants`, made by fiat rather than through the API —
/// the roster a relay that still had the hole would have ended up with.
fn planted_group(state: &AppState, host: &str, participants: &[&str]) -> String {
    let mut roster = vec![host.to_owned()];
    roster.extend(participants.iter().map(|s| (*s).to_owned()));
    let chat = crate::db::Chat {
        id: arbos_core::ids::ulid(),
        project_id: host.to_owned(),
        title: "Group".into(),
        is_pinned: false,
        position: 1,
        created_at: arbos_core::ids::now_ms(),
        doc_epoch: 1,
        kind: "group".into(),
        participants: roster,
        forked_from: None,
    };
    state.db.insert_chat(&chat).expect("group chat");
    chat.id
}

/// Victor opens his agent to Mallory's, the way the product already lets him.
fn victor_says_yes(t: &Two) {
    t.state
        .db
        .upsert_message_grant(
            &t.victor_agent,
            &format!("project:{}", t.mallory_agent),
            true,
            None,
            "victor",
        )
        .expect("grant");
}

/// Mallory asks for `Lium` on two relays that differ in one fact: on the first, Victor's agent
/// of that name exists; on the second, nobody has taken the name. Same request, same words
/// back, or the answer is telling her which names are taken.
async fn refusal_when_victim_exists(exists: bool) -> (StatusCode, String) {
    let state = test_state(AuthMode::Github);
    let (mallory_user, mallory) = person(&state, 9401);
    let mallory_agent = project_of(&state, &mallory_user, "Mallory's agent");
    if exists {
        let (victor_user, _) = person(&state, 9402);
        project_of(&state, &victor_user, VICTIM_NAME);
    }
    create(&state, &mallory, &mallory_agent, &[VICTIM_NAME])
        .await
        .expect_err("either way, Mallory gets nothing")
}

/// Step 2 of the chain, and the hole itself: a group took any project you could name.
#[tokio::test]
async fn a_group_refuses_a_participant_its_creator_does_not_own() {
    let t = two_tenants();
    create(&t.state, &t.mallory, &t.mallory_agent, &[VICTIM_NAME])
        .await
        .expect_err("Victor's agent must not join a group Mallory made");

    assert_eq!(
        refusal_when_victim_exists(true).await,
        refusal_when_victim_exists(false).await,
        "an agent Mallory may not add must read exactly like a name nobody has taken"
    );
}

/// The same door, through `PATCH` on a group that already exists.
#[tokio::test]
async fn adding_a_participant_to_an_existing_group_checks_the_owner_too() {
    let t = two_tenants();
    let (mallory_user, _) = (t.mallory.user_id().unwrap().to_owned(), ());
    let second = project_of(&t.state, &mallory_user, "Mallory's other agent");
    let group = create(&t.state, &t.mallory, &t.mallory_agent, &[&second])
        .await
        .expect("a group of Mallory's own agents");
    let chat_id = group["id"].as_str().unwrap().to_owned();

    let refused = patch(
        &t.state,
        &t.mallory,
        &t.mallory_agent,
        &chat_id,
        &[VICTIM_NAME],
        &[],
    )
    .await
    .expect_err("Victor's agent must not be added after the fact either");
    assert_eq!(
        refused,
        (
            StatusCode::BAD_REQUEST,
            format!("unknown participant project {VICTIM_NAME:?}")
        ),
        "and is called unknown, like any name the relay will not hand over"
    );

    let roster = t
        .state
        .db
        .chat(&t.mallory_agent, &chat_id)
        .unwrap()
        .unwrap();
    assert!(
        !roster.participants.contains(&t.victor_agent),
        "the roster did not change"
    );
}

async fn patch(
    state: &AppState,
    who: &Principal,
    host: &str,
    chat: &str,
    add: &[&str],
    remove: &[&str],
) -> Result<Value, (StatusCode, String)> {
    let resp = patch_group(
        State(state.clone()),
        who.clone(),
        Path((host.to_owned(), chat.to_owned())),
        Json(PatchGroupBody {
            add: add.iter().map(|s| (*s).to_owned()).collect(),
            remove: remove.iter().map(|s| (*s).to_owned()).collect(),
            title: None,
        }),
    )
    .await?;
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    Ok(serde_json::from_slice(&bytes).expect("group json"))
}

/// The fix is a closed door, not a wall: the consent the product already has still opens it.
#[tokio::test]
async fn a_granted_agent_still_joins_and_can_be_shown_out_after_the_grant_goes() {
    let t = two_tenants();
    victor_says_yes(&t);
    // A group keeps two agents besides the host, so dropping Victor's leaves one behind.
    let mallory_user = t.mallory.user_id().unwrap().to_owned();
    let second = project_of(&t.state, &mallory_user, "Mallory's other agent");

    let group = create(
        &t.state,
        &t.mallory,
        &t.mallory_agent,
        &[VICTIM_NAME, &second],
    )
    .await
    .expect("Victor said yes, so his agent joins");
    let chat_id = group["id"].as_str().unwrap().to_owned();
    assert!(
        group["participants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p.as_str() == Some(t.victor_agent.as_str())),
        "the granted agent is on the roster"
    );

    // Victor changes his mind. His agent must still be removable, or a withdrawn grant would
    // leave it wedged in the group for good.
    t.state
        .db
        .upsert_message_grant(
            &t.victor_agent,
            &format!("project:{}", t.mallory_agent),
            false,
            None,
            "victor",
        )
        .expect("grant off");
    let after = patch(
        &t.state,
        &t.mallory,
        &t.mallory_agent,
        &chat_id,
        &[],
        &[VICTIM_NAME],
    )
    .await
    .expect("removing a participant never needs their consent");
    assert!(
        !after["participants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p.as_str() == Some(t.victor_agent.as_str())),
        "the agent is off the roster"
    );
}

/// Mallory's own second agent joins by name, which is the everyday case and must not regress.
#[tokio::test]
async fn your_own_agent_still_joins_by_name() {
    let t = two_tenants();
    let mallory_user = t.mallory.user_id().unwrap().to_owned();
    project_of(&t.state, &mallory_user, "Tern");
    // Victor has an agent of the same name, created later, which the bare `id = ? OR name = ?`
    // lookup would have preferred.
    let victor_user = t.victor.user_id().unwrap().to_owned();
    project_of(&t.state, &victor_user, "Tern");

    let group = create(&t.state, &t.mallory, &t.mallory_agent, &["Tern"])
        .await
        .expect("Mallory's own Tern");
    let roster = group["participants"].as_array().unwrap();
    for p in roster {
        let id = p.as_str().unwrap();
        let owner = t.state.db.project(id).unwrap().unwrap().user_id;
        assert_eq!(owner, mallory_user, "every participant is Mallory's own");
    }
}

/// Step 3, read: a stranger must not read a group, and must not learn it is there.
#[tokio::test]
async fn an_outsider_cannot_read_a_group_and_cannot_tell_it_exists() {
    let t = two_tenants();
    let mallory_user = t.mallory.user_id().unwrap().to_owned();
    let second = project_of(&t.state, &mallory_user, "Mallory's other agent");
    let group = create(&t.state, &t.mallory, &t.mallory_agent, &[&second])
        .await
        .expect("Mallory's group");
    let chat_id = group["id"].as_str().unwrap().to_owned();

    let real = get_group(
        State(t.state.clone()),
        t.victor.clone(),
        Path((t.mallory_agent.clone(), chat_id)),
    )
    .await
    .expect_err("Victor is nowhere near this group");
    let imaginary = get_group(
        State(t.state.clone()),
        t.victor.clone(),
        Path((
            t.mallory_agent.clone(),
            "01JUNKJUNKJUNKJUNKJUNKJUNK".to_owned(),
        )),
    )
    .await
    .expect_err("and this group does not exist at all");
    assert_eq!(real.0, StatusCode::NOT_FOUND);
    assert_eq!(
        real, imaginary,
        "a group Victor may not see reads like a group that is not there"
    );
}

/// Step 3, write — and the hole that needed no group of one's own. Any signed-in device could
/// post into any group, as the host's user or under a participant agent's name.
#[tokio::test]
async fn an_outsider_cannot_send_into_someone_elses_group() {
    let t = two_tenants();
    let mallory_user = t.mallory.user_id().unwrap().to_owned();
    let second = project_of(&t.state, &mallory_user, "Mallory's other agent");
    let group = create(&t.state, &t.mallory, &t.mallory_agent, &[&second])
        .await
        .expect("Mallory's group");
    let chat_id = group["id"].as_str().unwrap().to_owned();

    let refused = send(
        &t.state,
        &t.victor,
        &t.mallory_agent,
        &chat_id,
        "user",
        "hello from nowhere",
    )
    .await
    .expect_err("Victor's device is not on this group");
    assert_eq!(refused.0, StatusCode::NOT_FOUND, "and it is told nothing");

    let impersonation = send(
        &t.state,
        &t.victor,
        &t.mallory_agent,
        &chat_id,
        &format!("project:{second}"),
        "hello as Mallory's agent",
    )
    .await
    .expect_err("nor may it speak as an agent on the roster");
    assert_eq!(impersonation.0, StatusCode::NOT_FOUND);

    assert!(
        t.state.db.take_pushes(&second).expect("pushes").is_empty(),
        "no group_message reached the agents"
    );
}

/// A participant agent may speak in the group it joined, but only as itself.
#[tokio::test]
async fn a_participant_speaks_only_for_itself() {
    let t = two_tenants();
    victor_says_yes(&t);
    let group = create(&t.state, &t.mallory, &t.mallory_agent, &[VICTIM_NAME])
        .await
        .expect("a consented group");
    let chat_id = group["id"].as_str().unwrap().to_owned();

    // Mallory's box runs her agents; it must not post under Victor's agent's name.
    let refused = send(
        &t.state,
        &t.mallory_box,
        &t.mallory_agent,
        &chat_id,
        &format!("project:{}", t.victor_agent),
        "words in Victor's mouth",
    )
    .await
    .expect_err("Mallory's box does not speak for Victor's agent");
    assert_eq!(refused.0, StatusCode::FORBIDDEN);

    // The same claim through the reply route, which only ever checked boxes.
    let reply = group_reply(
        State(t.state.clone()),
        t.mallory.clone(),
        Path((t.mallory_agent.clone(), chat_id.clone())),
        Json(GroupReplyBody {
            from_project_id: t.victor_agent.clone(),
            text: "a reply Victor never wrote".into(),
            trace_id: None,
            depth: None,
        }),
    )
    .await
    .expect_err("Mallory's device does not file replies for Victor's agent");
    assert_eq!(reply.0, StatusCode::FORBIDDEN);

    // Her own agent's line goes through, so the route still works.
    send(
        &t.state,
        &t.mallory_box,
        &t.mallory_agent,
        &chat_id,
        &format!("project:{}", t.mallory_agent),
        "a line of my own",
    )
    .await
    .expect("a box speaks for its user's agent");
}

async fn send(
    state: &AppState,
    who: &Principal,
    host: &str,
    chat: &str,
    from: &str,
    text: &str,
) -> Result<Value, (StatusCode, String)> {
    let resp = group_send(
        State(state.clone()),
        who.clone(),
        Path((host.to_owned(), chat.to_owned())),
        Json(GroupSendBody {
            from: from.to_owned(),
            text: text.to_owned(),
            to: None,
            trace_id: None,
            depth: None,
            trace: Vec::new(),
        }),
    )
    .await?;
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    Ok(serde_json::from_slice(&bytes).expect("send json"))
}

/// The whole chain, run end to end the way the reporter ran it, against a relay that is fixed.
/// Mallory knows Victor's agent by name and nothing else.
#[tokio::test]
async fn the_reported_chain_gets_no_further_than_the_name() {
    let t = two_tenants();

    // Step 2: name the victim's agent into a group of her own. Refused, and indistinguishable
    // from naming thin air.
    assert_eq!(
        create(&t.state, &t.mallory, &t.mallory_agent, &[VICTIM_NAME])
            .await
            .expect_err("refused"),
        (
            StatusCode::BAD_REQUEST,
            format!("unknown participant project {VICTIM_NAME:?}")
        ),
    );

    // Step 3: so there is no group to read or write. The only way on from here is a roster
    // Mallory could not have built — plant one anyway, the way the live relay already holds
    // rows made while the check was missing, and send into it as the host's user, which is
    // Mallory's to do. The fan-out asks the roster again and reaches nobody.
    let planted = planted_group(&t.state, &t.mallory_agent, &[&t.victor_agent]);
    let out = send(
        &t.state,
        &t.mallory,
        &t.mallory_agent,
        &planted,
        "user",
        "answer me",
    )
    .await
    .expect("the host's own user line is accepted");
    assert_eq!(
        out["delivered"].as_array().unwrap().len(),
        0,
        "nothing was delivered"
    );
    assert_eq!(out["refused"][0]["reason"], "consent");

    // And she may not speak under Victor's agent's name either.
    let refused = send(
        &t.state,
        &t.mallory,
        &t.mallory_agent,
        &planted,
        &format!("project:{}", t.victor_agent),
        "answer me",
    )
    .await
    .expect_err("Mallory may not speak as Victor's agent");
    assert_eq!(refused.0, StatusCode::FORBIDDEN);

    assert!(
        t.state
            .db
            .take_pushes(&t.victor_agent)
            .expect("pushes")
            .is_empty(),
        "Victor's agent was never woken, so it never answered"
    );
}
