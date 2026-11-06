//! An untitled group's generated name has one form, on every surface that shows it.
//!
//! Four places turn the roster into a name a person reads, and they disagreed: a push said
//! `user41, user43` and an export of the same group said `@user41, @user43`. The agreed form
//! carries no `@`. `voice.rs` speaks its title aloud and "at user41, at user43" is not what
//! anyone says, so the bare handle is the only form that works on all four; the push title and
//! the API's DM title were already bare, so this is the existing majority rather than a fifth
//! form. No client parses a leading `@` back out of a relay-supplied title.
//!
//! Two differences this test pins but does not change. The API and push titles drop the person
//! reading them, because a group is named after the *others*; the export and voice titles name
//! the whole roster. And the Matrix room name (`matrix/worker.rs`'s `group_name`) is a fifth
//! derivation with its own form and its own order — it is the room's `m.room.name` state, so
//! changing it writes into live rooms, and it is queued separately.

use axum::body::to_bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde_json::{json, Value};

use arbos_core::ids::{now_ms, ulid};

use crate::auth::Principal;
use crate::db_people::{ChatWithMembers, PeopleChat};
use crate::export::ExportQuery;
use crate::notify::ChatRef;
use crate::people::tests_support::person;
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};

/// An untitled group of these humans; the first is the owner. `tests_support::group` titles its
/// group "Arbonites", and a titled group never reaches the derivations under test.
fn untitled_group(state: &AppState, members: &[&str]) -> String {
    let chat = PeopleChat {
        id: ulid(),
        kind: "group".into(),
        created_by: members[0].to_owned(),
        created_at: now_ms(),
        title: None,
        image_key: None,
        avatar_leaf: None,
        avatar_leaf_color: None,
        description: None,
    };
    state.db.insert_people_chat(&chat).expect("chat");
    for u in members {
        state
            .db
            .add_member(
                &chat.id,
                &format!("user:{u}"),
                "owner",
                members[0],
                &json!({}),
            )
            .expect("member");
    }
    chat.id
}

#[tokio::test]
async fn an_untitled_group_is_named_the_same_way_on_every_surface() {
    let state = test_state(AuthMode::Github);
    let (alice, alice_dev) = person(&state, 41);
    let (bob, _) = person(&state, 42);
    let (carol, _) = person(&state, 43);
    let chat = untitled_group(&state, &[&alice, &bob, &carol]);

    // The agreed form: the handles as they are stored, joined by ", ". Nothing is prefixed.
    let name_of = |handles: &[&str]| handles.join(", ");
    let (alice_h, bob_h, carol_h) = ("user41", "user42", "user43");

    // 1. The chat title in the API (`GET /chats`), as Alice's device renders it.
    let device_id = match &alice_dev {
        Principal::Device { id, .. } => id.clone(),
        _ => unreachable!("person() hands back a device principal"),
    };
    let cm = ChatWithMembers {
        chat: state.db.people_chat(&chat).unwrap().unwrap(),
        members: state.db.members_of(&chat).unwrap(),
    };
    let api = crate::people::people_chat_dto(&state, &cm, &alice, &device_id).title;

    // 2. The push title on Alice's phone.
    let push = crate::notify::chat_meta(&state, &ChatRef::people(&chat), &alice)
        .title
        .expect("a group carries a title");

    // 3. The title on the export Alice keeps.
    let resp = crate::export::export_chat(
        State(state.clone()),
        alice_dev.clone(),
        Path(chat.clone()),
        Query(ExportQuery::default()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let export = body["chat"]["title"].as_str().unwrap().to_owned();

    // 4. The name the voice agent speaks.
    let voice = crate::voice::voice_chat_title(&state, &ChatRef::people(&chat));

    // The API and push titles name the others; the export and voice titles name the roster.
    assert_eq!(api, name_of(&[bob_h, carol_h]), "API chat title");
    assert_eq!(push, name_of(&[bob_h, carol_h]), "push title");
    assert_eq!(
        export,
        name_of(&[alice_h, bob_h, carol_h]),
        "export chat title"
    );
    assert_eq!(
        voice,
        name_of(&[alice_h, bob_h, carol_h]),
        "voice spoken name"
    );

    // Said once, over all four: a generated group name carries no `@` anywhere in it.
    for (surface, title) in [
        ("API chat title", &api),
        ("push title", &push),
        ("export chat title", &export),
        ("voice spoken name", &voice),
    ] {
        assert!(
            !title.contains('@'),
            "{surface} must not prefix a handle: {title}"
        );
    }
}
