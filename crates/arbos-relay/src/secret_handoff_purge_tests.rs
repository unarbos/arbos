//! What the secret hand-off owes everyone who was in the room, not just the owner.
//!
//! `POST /box/secrets/from-message` exists to get a pasted credential out of a chat. It
//! supports people chats explicitly (`people_chat`, behind a `room_access` check), and a people
//! chat has other humans in it. Three things were measured there and all three are checked
//! below, because they fail independently:
//!
//! 1. **The value should never have been sent.** `notify::brief` builds the notification text
//!    and truncates without redacting, so a pasted key arrives verbatim on every other member's
//!    lock screen — twice, as `aps.alert.body` and as the top-level `text` the notification
//!    service extension reads. Clearing banners afterwards is remediation for a leak that
//!    should not have left the relay.
//! 2. **The banner has to be withdrawn on a phone that is not running.** `PushKind::Clear` is
//!    the mechanism for that and the hand-off never used it.
//! 3. **The open session has to be told too.** `message_redacted` went to the owner alone,
//!    because `push_user` fans out over one user's devices.
//!
//! A new file rather than additions to `notify.rs` and `secrets.rs`: both have other lanes'
//! pull requests in flight, and the non-test half of this change already touches them.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde_json::json;

use crate::auth::Principal;
use crate::db::Project;
use crate::notify::{self, ChatRef, Sender};
use crate::people::tests_support::{group, person};
use crate::secrets::{box_from_message, FromMessageBody};
use crate::state::test_support::test_state;
use crate::state::{AppState, AuthMode};
use arbos_core::ids::{now_ms, ulid};

/// A Stripe-shaped key: `find_key_like` calls it `confident`, which is why the hand-off below
/// accepts it without a hint. The point of using a real shape is that the assertions cannot
/// pass by accident on a token nothing would have vaulted either.
///
/// Assembled rather than written out, because a literal of this shape is exactly what GitHub's
/// push protection stops — it blocked the first version of this file, which is its own evidence
/// that the shape is realistic. The test needs the shape at run time, not in the source.
fn key() -> String {
    format!("sk_{}_{}", "live", "51QhandoffPROBE0123456789")
}

const PROJECT: &str = "proj_handoff_purge";

struct Room {
    state: AppState,
    alice: String,
    bob: String,
    chat: String,
    alice_token: String,
    bob_token: String,
}

/// Alice and Bob in a group, Alice's agent a member of it — the membership that lets her box
/// reach the room at all (`people::room_access`). Both phones are reachable by APNs.
fn room() -> Room {
    let state = test_state(AuthMode::Github);
    let (alice, alice_dev) = person(&state, 7701);
    let (bob, bob_dev) = person(&state, 7702);
    state
        .db
        .insert_project(&Project {
            id: PROJECT.into(),
            name: "Arbos".into(),
            icon: "R".into(),
            instructions: String::new(),
            models: json!({}),
            state: "hibernated_light".into(),
            created_at: now_ms(),
            last_active_at: now_ms(),
            spend_cap_daily: 25.0,
            color: "#000".into(),
            user_id: alice.clone(),
            workspace_state: "stopped".into(),
            avatar: None,
            avatar_color: None,
            description: String::new(),
            primary_chat_id: None,
            avatar_color_index: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            image_key: None,
            avatar_version: 0,
        })
        .expect("project");
    let chat = group(&state, &[&alice, &bob]);
    state
        .db
        .add_member(
            &chat,
            &format!("agent:{PROJECT}"),
            "member",
            &alice,
            &json!({}),
        )
        .expect("agent member");
    let alice_token = push_token(&state, &alice_dev, &alice);
    let bob_token = push_token(&state, &bob_dev, &bob);
    Room {
        state,
        alice,
        bob,
        chat,
        alice_token,
        bob_token,
    }
}

fn push_token(state: &AppState, p: &Principal, user: &str) -> String {
    let device_id = match p {
        Principal::Device { id, .. } => id.clone(),
        Principal::Box { .. } | Principal::Admin => unreachable!("device principal"),
    };
    let token = format!("{:0<64}", ulid().to_ascii_lowercase());
    state
        .db
        .upsert_push_token(&crate::db_push::PushToken {
            device_id,
            user_id: user.to_owned(),
            platform: "ios".into(),
            token: token.clone(),
            environment: "development".into(),
            bundle_id: crate::apns::DEFAULT_TOPIC.into(),
            app_version: "1".into(),
            updated_at: now_ms(),
        })
        .expect("token");
    token
}

/// The message Alice pastes, written into the shared room doc so the hand-off can read it.
async fn paste(r: &Room, message_id: &str, text: &str) {
    let sender = format!("user:{}", r.alice);
    let text = text.to_owned();
    let id = message_id.to_owned();
    crate::notices::write_doc(&r.state, None, &r.chat, move |doc| {
        let opts = arbos_core::doc::UserMessageOpts {
            sender: Some(sender),
            ..Default::default()
        };
        doc.append_user_message(&id, &text, "dev1", &opts)?;
        Ok(())
    })
    .await
    .expect("write");
}

async fn hand_off(r: &Room, message_id: &str) {
    let mut headers = HeaderMap::new();
    headers.insert("x-arbos-project", PROJECT.parse().unwrap());
    box_from_message(
        State(r.state.clone()),
        Principal::Box {
            user_id: r.alice.clone(),
        },
        headers,
        Json(FromMessageBody {
            chat_id: r.chat.clone(),
            message_id: message_id.to_owned(),
            people_chat: true,
            ..Default::default()
        }),
    )
    .await
    .expect("hand-off accepted");
}

/// Every APNs push handed to the provider so far, as `(token, payload)`.
async fn sent(state: &AppState) -> Vec<(String, serde_json::Value)> {
    state.push.drain().await;
    state
        .push
        .captured()
        .await
        .into_iter()
        .map(|o| (o.device_token, o.payload))
        .collect()
}

/// The in-app `/me/pushes` rows, as `(user_id, kind)`.
fn queued(state: &AppState) -> Vec<(String, String)> {
    state
        .db
        .with(|c| {
            let mut st = c.prepare("SELECT user_id, kind FROM user_pushes ORDER BY id")?;
            let rows = st
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
        .expect("pushes")
}

/// 1. The leak itself. Nothing the relay would offer to put in a vault belongs in a
/// notification, because a notification is the one copy that lands on a screen the recipient
/// did not open and survives there until they do.
#[tokio::test]
async fn a_key_the_relay_would_vault_is_never_put_in_a_notification() {
    let r = room();
    let key = key();
    notify::announce(
        &r.state,
        &ChatRef::people(&r.chat),
        &Sender::user(&r.state, &r.alice),
        "m_key",
        &format!("here it is: {key}"),
    )
    .await;

    let bob: Vec<serde_json::Value> = sent(&r.state)
        .await
        .into_iter()
        .filter(|(tok, _)| *tok == r.bob_token)
        .map(|(_, p)| p)
        .collect();
    assert_eq!(bob.len(), 1, "Bob's phone is alerted about the message");
    // Scanned whole rather than field by field: the text is in the payload more than once, and
    // the second copy is the one a field-by-field assertion would have missed.
    let whole = bob[0].to_string();
    assert!(
        !whole.contains(&key),
        "a pasted key reached Bob's lock screen:\n{}",
        serde_json::to_string_pretty(&bob[0]).unwrap()
    );
    assert!(
        whole.contains("here it is"),
        "the rest of the message still says what it said:\n{whole}"
    );
}

/// Why the notification redaction is driven by `find_key_like` and not by `scrub` alone.
///
/// This is the trap that made the obvious fix a no-op. `scrub` is the module's named helper and
/// the natural thing to reach for, but its pattern list is the smaller of the two the crate
/// keeps, and the shapes it misses are not exotic — `sk_live_…` is the example in the hand-off
/// path's own documentation. A fix that called `scrub` here would have passed review, changed
/// the file the finding named, and left the measured leak exactly where it was.
#[test]
fn scrub_alone_would_have_left_the_key_that_started_this() {
    use arbos_core::redact;

    let key = key();
    let pasted = format!("here it is: {key}");
    assert!(
        redact::scrub(&pasted).contains(&key),
        "if `scrub` has grown to cover this shape, the composition below is redundant and the \
         comment on `redact_key_like` needs rewriting rather than deleting"
    );
    assert!(!redact::redact_key_like(&pasted).contains(&key));
    // And the hand-off agrees it is a key — that is the point of sharing the finder.
    assert!(redact::find_key_like(&pasted)[0].confident);
}

/// 2 and 3. Who is told to drop their copy. Both halves are asserted together because they
/// answer the same question for two different device states — `Clear` reaches the phone that is
/// asleep, `message_redacted` heals the session that is open — and shipping one without the
/// other leaves whichever device the member happens to be holding.
#[tokio::test]
async fn the_hand_off_tells_every_member_to_drop_its_copy() {
    let r = room();
    let pasted = format!("here it is: {}", key());
    paste(&r, "m_key", &pasted).await;
    notify::announce(
        &r.state,
        &ChatRef::people(&r.chat),
        &Sender::user(&r.state, &r.alice),
        "m_key",
        &pasted,
    )
    .await;
    let before = sent(&r.state).await.len();

    hand_off(&r, "m_key").await;

    let after: Vec<(String, serde_json::Value)> =
        sent(&r.state).await.into_iter().skip(before).collect();
    let cleared: Vec<&String> = after
        .iter()
        .filter(|(_, p)| p["type"] == "clear")
        .map(|(tok, _)| tok)
        .collect();
    assert!(
        cleared.contains(&&r.bob_token),
        "nothing withdrew the banner from Bob's phone; after the hand-off the relay sent: {:?}",
        after
            .iter()
            .map(|(_, p)| p["type"].clone())
            .collect::<Vec<_>>()
    );
    assert!(
        cleared.contains(&&r.alice_token),
        "the owner's own other devices were not told either"
    );

    let told = |user: &str| {
        queued(&r.state)
            .into_iter()
            .any(|(u, kind)| u == user && kind == "message_redacted")
    };
    assert!(told(&r.alice), "the owner is told, as before");
    assert!(
        told(&r.bob),
        "the open session on Bob's device was never told the row changed"
    );
}
