//! Phase 3, the way back: what a member's device writes into a people chat's room on the
//! homeserver (`people_native_room`) is written into the chat's Loro room too, so turning
//! `users.matrix_people_native` off — or adding an agent or an unflagged person — shows the
//! same chat, and the Loro room is never the store that is missing a message.
//!
//! The homeserver hands the relay every event of a room its users are in (the appservice
//! transaction). A message becomes the Loro row its sender's app would have written (same id,
//! sender, time, reply and parts); a removal removes the row. The relay's own mirror of a Loro
//! row comes back the same way and is known by its id, so nothing is written twice in either
//! direction.
//!
//! A reaction put on a row there is put on the Loro row, and taken off it when its event is
//! removed. What was sent before this bridge ran is read from the room once
//! ([`catch_up_once`]), through the same rules.
//!
//! Bounds. An encrypted room's events are ciphertext here: nothing is carried back, and the
//! relay is not handed the plaintext to make it so; those messages stay on the homeserver and
//! are read there again when the flag is back on.

use arbos_core::doc::UserMessageOpts;
use serde_json::{json, Value};

use super::backfill::load_doc;
use super::client::{Client, MatrixError, MxResult};
use super::Job;
use crate::notices::write_doc_as;
use crate::state::AppState;

/// The most extra parts (`life.arbos.parts`) carried with one message.
const PARTS_MAX: usize = 16;

/// The relay's ids are ULIDs; this is the widest shape taken as a message id.
fn is_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The longest reaction carried (bytes): an emoji with its modifiers
/// (`arbos_matrix::rows::REACTION_KEY_MAX`).
const REACTION_KEY_MAX: usize = 64;
/// How far back one catch-up reads a room: pages of [`CATCH_UP_PAGE`] events.
const CATCH_UP_PAGES: usize = 200;
const CATCH_UP_PAGE: usize = 100;
/// `meta` key: the one catch-up over every native chat has been queued.
const CATCH_UP_DONE: &str = "matrix_native_catch_up";

/// The device a carried row is signed with: the relay, writing a member's own words.
fn device(user_id: &str) -> String {
    format!("matrix:{user_id}")
}

/// Queue what one appservice transaction says about native people chats.
pub fn on_transaction(state: &AppState, events: &[Value]) {
    for job in events.iter().filter_map(|ev| job_for(state, ev)) {
        state.matrix.enqueue(state, job);
    }
}

/// The chat of a room whose events are carried back, with its members: an open, unencrypted
/// people chat they read on the homeserver.
fn carried_chat(state: &AppState, room_id: &str) -> Option<crate::db_people::ChatWithMembers> {
    let room = state.db.matrix_room_by_room_id(room_id).ok()??;
    if room.project_id.is_some() || room.closed_at.is_some() || room.encrypted {
        return None;
    }
    let cm = crate::people::load_chat(state, &room.chat_id).ok()?;
    (super::people_native_room(state, &cm).as_deref() == Some(room.room_id.as_str())).then_some(cm)
}

/// The person who sent an event, when they are a member who may write into the Loro room:
/// the account the homeserver says sent it, not blocked by the other person of a DM, not
/// silenced. The row path refuses those, so nothing carries them around it.
fn author(state: &AppState, cm: &crate::db_people::ChatWithMembers, ev: &Value) -> Option<String> {
    let member = state.matrix.cfg.member_of_mxid(ev["sender"].as_str()?)?;
    let user_id = member.strip_prefix("user:")?;
    (cm.members.iter().any(|m| m.member == member)
        && crate::people::sender_allowed(state, user_id, cm).is_ok())
    .then(|| user_id.to_owned())
}

/// The job an event of a room asks for, or `None`: not a people chat its members read on the
/// homeserver, encrypted, not from a member, or not a message, reaction or removal an app
/// sends.
fn job_for(state: &AppState, ev: &Value) -> Option<Job> {
    let cm = carried_chat(state, ev["room_id"].as_str()?)?;
    let chat_id = cm.chat.id.clone();
    let content = &ev["content"];
    match ev["type"].as_str()? {
        "m.room.redaction" => Some(Job::NativeRemoval {
            chat_id,
            event_id: ev["redacts"]
                .as_str()
                .or_else(|| content["redacts"].as_str())?
                .to_owned(),
        }),
        "life.arbos.reaction" if ev.get("state_key").is_none() => Some(Job::NativeReaction {
            user_id: author(state, &cm, ev)?,
            event_id: ev["event_id"].as_str()?.to_owned(),
            target: content["event_id"].as_str()?.to_owned(),
            key: content["key"]
                .as_str()
                .filter(|k| {
                    (1..=REACTION_KEY_MAX).contains(&k.len()) && !k.chars().any(char::is_control)
                })?
                .to_owned(),
            chat_id,
        }),
        "m.room.message" if ev.get("state_key").is_none() => {
            let user_id = author(state, &cm, ev)?;
            // The apps' own rule (`arbos_matrix::rows::sender_is_own`): a message that names
            // anyone else as its sender is shown to nobody, so it is carried nowhere. Nor is
            // a row of the Loro room a device carried the other way (`history.rs`).
            if content
                .get("life.arbos.sender")
                .is_some_and(|s| s.as_str() != Some(format!("user:{user_id}").as_str()))
                || content.get("life.arbos.history").is_some()
            {
                return None;
            }
            let message_id = content["life.arbos.id"].as_str().filter(|id| is_id(id))?;
            // An id this chat already has an event for is the relay's own mirror coming back.
            if content["msgtype"] != "m.text"
                || state.db.matrix_event(&chat_id, message_id).ok()?.is_some()
            {
                return None;
            }
            Some(Job::NativeMessage {
                event_id: ev["event_id"].as_str()?.to_owned(),
                user_id,
                message_id: message_id.to_owned(),
                text: content["body"].as_str()?.to_owned(),
                parts: content["life.arbos.parts"]
                    .as_array()
                    .filter(|p| p.len() <= PARTS_MAX && p.iter().all(Value::is_object))
                    .cloned(),
                reply_to: content["life.arbos.reply_to"].as_str().map(str::to_owned),
                ts: ev["origin_server_ts"].as_i64()?,
                chat_id,
            })
        }
        _ => None,
    }
}

fn db_err(e: impl std::fmt::Display) -> MatrixError {
    MatrixError::Shape(format!("db: {e}"))
}

/// Apply one of the `Native*` jobs.
pub async fn apply(state: &AppState, job: &Job) -> MxResult<()> {
    match job {
        Job::NativeRemoval { chat_id, event_id } => removal(state, chat_id, event_id).await,
        Job::NativeReaction {
            chat_id,
            event_id,
            user_id,
            target,
            key,
        } => reaction(state, chat_id, event_id, user_id, target, key).await,
        Job::NativeCatchUp { chat_id } => catch_up(state, chat_id).await,
        _ => message(state, job).await,
    }
}

/// Queue, once per relay, a read of every chat its members read on the homeserver: what they
/// sent there before the relay carried such messages back is not in the Loro room yet.
pub fn catch_up_once(state: &AppState) {
    if !matches!(state.db.meta_get(CATCH_UP_DONE), Ok(None)) {
        return;
    }
    for room in state.db.matrix_rooms_all().unwrap_or_default() {
        if carried_chat(state, &room.room_id).is_some() {
            state.matrix.enqueue(
                state,
                Job::NativeCatchUp {
                    chat_id: room.chat_id,
                },
            );
        }
    }
    if let Err(e) = state.db.meta_set(CATCH_UP_DONE, "1") {
        tracing::warn!(error = %e, "matrix catch-up: not marked done, it runs again next start");
    }
}

/// Read a chat's room back to its beginning (at most [`CATCH_UP_PAGES`] pages) and apply, in
/// the order they happened, the events a transaction would have queued. Every one of those is
/// safe to apply again, so this is too.
async fn catch_up(state: &AppState, chat_id: &str) -> MxResult<()> {
    let Some(room) = state.db.matrix_room(chat_id).map_err(db_err)? else {
        return Ok(());
    };
    let Some(cm) = carried_chat(state, &room.room_id) else {
        return Ok(());
    };
    // Only a member may read the room.
    let Some(reader) = cm
        .members
        .iter()
        .find_map(|m| state.matrix.cfg.member_mxid(&m.member))
    else {
        return Ok(());
    };
    let client = Client::new(state.http.clone(), state.matrix.cfg.clone());
    let mut events = Vec::new();
    let mut from = None;
    for _ in 0..CATCH_UP_PAGES {
        let (chunk, end) = client
            .messages_page(&room.room_id, &reader, from.as_deref(), CATCH_UP_PAGE)
            .await?;
        let done = chunk.is_empty() || end.is_none();
        events.extend(chunk);
        from = end;
        if done {
            break;
        }
    }
    for ev in events.iter().rev() {
        if let Some(job) = job_for(state, ev) {
            Box::pin(apply(state, &job)).await?;
        }
    }
    Ok(())
}

/// Whether `user_id` holds the reaction `key` on a row of the chat's Loro room; `None` when
/// the chat has no such row.
fn reacted(
    state: &AppState,
    chat_id: &str,
    message_id: &str,
    user_id: &str,
    key: &str,
) -> MxResult<Option<bool>> {
    let Some(doc) = load_doc(state, chat_id, None).map_err(db_err)? else {
        return Ok(None);
    };
    let sender = format!("user:{user_id}");
    let key = arbos_core::doc::normalize_reaction_emoji(key);
    Ok(doc
        .projected_rows()
        .iter()
        .find(|r| r["id"] == message_id)
        .map(|r| r["reactions"][key.as_str()][sender.as_str()] == true))
}

/// Put (`on`) or take off a person's reaction on a Loro row, when it is not already so.
async fn set_reaction(
    state: &AppState,
    chat_id: &str,
    message_id: &str,
    user_id: &str,
    key: &str,
    on: bool,
) -> MxResult<()> {
    if reacted(state, chat_id, message_id, user_id, key)? != Some(!on) {
        return Ok(());
    }
    let sender = format!("user:{user_id}");
    write_doc_as(state, None, chat_id, &device(user_id), |doc| {
        doc.transact(|d| d.set_reaction(message_id, key, &sender, on))?;
        Ok(())
    })
    .await
    .map(drop)
    .map_err(db_err)
}

/// Put a reaction a member's device sent on the homeserver on the Loro row of the event it
/// names. A reaction to anything that is no row here is nothing.
async fn reaction(
    state: &AppState,
    chat_id: &str,
    event_id: &str,
    user_id: &str,
    target: &str,
    key: &str,
) -> MxResult<()> {
    let Some(message_id) = state
        .db
        .matrix_message_of(chat_id, target)
        .map_err(db_err)?
    else {
        return Ok(());
    };
    // Recorded first: its removal is then known for what it is, whatever happens below.
    state
        .db
        .matrix_insert_reaction(chat_id, event_id, &message_id, user_id, key)
        .map_err(db_err)?;
    set_reaction(state, chat_id, &message_id, user_id, key, true).await
}

/// Write a message a member's device sent on the homeserver into the chat's Loro room. Safe
/// to run again: a row the chat already has is left as it is.
// The doc is rebuilt from its rows for every message (here, in `write_doc_as`, and by the
// resync that write leaves due). Fine at the pace people type; a doc kept per chat if native
// chats ever get busier than that.
async fn message(state: &AppState, job: &Job) -> MxResult<()> {
    let Job::NativeMessage {
        chat_id,
        event_id,
        user_id,
        message_id,
        text,
        parts,
        reply_to,
        ts,
    } = job
    else {
        return Ok(());
    };
    let sender = format!("user:{user_id}");
    let row = load_doc(state, chat_id, None)
        .map_err(db_err)?
        .and_then(|d| d.message(message_id));
    // Someone else's row under this id: the id is taken, and this message is not that row.
    if row
        .as_ref()
        .is_some_and(|m| m.rest["sender"] != sender.as_str())
    {
        return Ok(());
    }
    // Recorded before the row exists, so the mirror never sends the row back to the room.
    state
        .db
        .matrix_insert_event(chat_id, message_id, event_id, &sender, *ts)
        .map_err(db_err)?;
    if row.is_some() {
        return Ok(());
    }
    let card = state.db.user_card(user_id).ok().flatten();
    let opts = UserMessageOpts {
        reply_to: reply_to
            .as_deref()
            .and_then(|e| state.db.matrix_message_of(chat_id, e).ok().flatten()),
        sender: Some(sender),
        sender_handle: card
            .as_ref()
            .map(|c| c.handle.clone())
            .filter(|h| !h.is_empty())
            .or_else(|| Some(user_id.to_owned())),
        sender_name: card
            .as_ref()
            .and_then(|c| c.name.clone())
            .filter(|n| !n.is_empty()),
        sender_avatar: card.as_ref().and_then(|c| c.avatar_url.clone()),
        ..Default::default()
    };
    let device = device(user_id);
    let seq = write_doc_as(state, None, chat_id, &device, |doc| {
        doc.transact(|d| {
            d.append_user_message(message_id, text, &device, &opts)?;
            // When it was said, not when it was carried.
            d.set_message_fields(message_id, &[("createdAt", json!(ts))]);
            for part in parts.iter().flatten() {
                d.add_part_from_json(message_id, part);
            }
            Ok::<(), arbos_core::doc::CoreError>(())
        })?;
        Ok(())
    })
    .await
    .map_err(db_err)?;
    // Their own message is not unread for them.
    let _ = state.db.advance_user_cursor(user_id, chat_id, seq);
    Ok(())
}

/// Remove the Loro row of an event removed on the homeserver, which has already decided that
/// whoever asked may; a removed reaction comes off its row, unless another device of that
/// person still holds the same one. An event that is neither here is nothing to remove.
async fn removal(state: &AppState, chat_id: &str, event_id: &str) -> MxResult<()> {
    if let Some((message_id, user_id, key, held)) = state
        .db
        .matrix_take_reaction(chat_id, event_id)
        .map_err(db_err)?
    {
        if held {
            return Ok(());
        }
        return set_reaction(state, chat_id, &message_id, &user_id, &key, false).await;
    }
    let Some(message_id) = state
        .db
        .matrix_message_of(chat_id, event_id)
        .map_err(db_err)?
    else {
        return Ok(());
    };
    let has_row = load_doc(state, chat_id, None)
        .map_err(db_err)?
        .is_some_and(|d| d.find_message(&message_id).is_some());
    if !has_row {
        return Ok(());
    }
    write_doc_as(state, None, chat_id, crate::notices::RELAY_DEVICE, |doc| {
        doc.transact(|d| d.delete_message(&message_id))?;
        Ok(())
    })
    .await
    .map(drop)
    .map_err(db_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::worker::Worker;
    use crate::matrix::{Config, Matrix, Mode};
    use crate::people::tests_support::{dm, person};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    const ROOM: &str = "!native:arbos.life";

    /// A DM both members read on the homeserver, with the room the relay has for it.
    fn native_dm(encrypted: bool) -> (AppState, String, String, String) {
        let mut state = test_state(AuthMode::Github);
        state.matrix = Matrix::new(Config {
            mode: Mode::Dual,
            // Nothing listens here: a job that reaches for the homeserver fails.
            hs_url: "http://127.0.0.1:9".into(),
            ..Config::off()
        });
        let (alice, _) = person(&state, 1301);
        let (bob, _) = person(&state, 1302);
        let chat = dm(&state, &alice, &bob);
        state
            .db
            .matrix_insert_room(&chat, ROOM, None, "dm", encrypted)
            .expect("room");
        for u in [&alice, &bob] {
            assert!(state.db.set_matrix_people_native(u, true).expect("flag"));
        }
        (state, chat, alice, bob)
    }

    /// A message event as the homeserver hands it over.
    fn sent(user: &str, event_id: &str, content: Value) -> Value {
        json!({ "type": "m.room.message", "room_id": ROOM, "event_id": event_id,
                "sender": format!("@{user}:arbos.life"), "origin_server_ts": 1_700_000_000_123_i64,
                "content": content })
    }

    fn text(id: &str, body: &str) -> Value {
        json!({ "msgtype": "m.text", "body": body, "life.arbos.id": id })
    }

    fn rows(state: &AppState, chat: &str) -> Vec<Value> {
        load_doc(state, chat, None)
            .expect("doc")
            .map(|d| d.projected_rows())
            .unwrap_or_default()
    }

    async fn apply(state: &AppState, events: &[Value]) {
        let worker = Worker::new(state.clone());
        for job in events.iter().filter_map(|ev| job_for(state, ev)) {
            worker.handle(&job).await.expect("applied");
        }
    }

    #[tokio::test]
    async fn a_message_sent_on_the_homeserver_becomes_the_row_its_app_would_have_written() {
        let (state, chat, alice, bob) = native_dm(false);
        let first = sent(&alice, "$one", text("01NATIVEONE", "lunch at noon?"));
        let mut answer = text("01NATIVETWO", "noon works");
        answer["life.arbos.reply_to"] = json!("$one");
        answer["life.arbos.parts"] = json!([{ "id": "p1", "kind": "cheque", "amount": "1" }]);
        let second = sent(&bob, "$two", answer);
        apply(&state, &[first.clone(), second.clone()]).await;

        let got = rows(&state, &chat);
        assert_eq!(got.len(), 2, "{got:?}");
        let (one, two) = (&got[0], &got[1]);
        assert_eq!(one["id"], "01NATIVEONE");
        assert_eq!(one["sender"], format!("user:{alice}"));
        assert_eq!(one["role"], "user");
        assert_eq!(one["createdAt"], 1_700_000_000_123_i64);
        assert_eq!(arbos_core::doc::message_text(one), "lunch at noon?");
        assert_eq!(two["sender"], format!("user:{bob}"));
        assert_eq!(two["replyTo"], "01NATIVEONE", "the row it answers, by id");
        assert!(
            two["parts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["kind"] == "cheque"),
            "{two}"
        );
        // The mirror knows both rows by their events and sends neither back to the room.
        assert_eq!(
            state.db.matrix_event(&chat, "01NATIVEONE").unwrap(),
            Some("$one".to_owned())
        );
        let resync = crate::matrix::backfill::resync_chat(&state, &chat, None);
        assert_eq!(resync.queued, 0, "{resync:?}");

        // The homeserver sends a transaction again, and the job runs again: still two rows.
        apply(&state, &[first.clone(), second]).await;
        Worker::new(state.clone())
            .handle(&Job::NativeMessage {
                chat_id: chat.clone(),
                event_id: "$one".into(),
                user_id: alice.clone(),
                message_id: "01NATIVEONE".into(),
                text: "lunch at noon?".into(),
                parts: None,
                reply_to: None,
                ts: 1,
            })
            .await
            .expect("again");
        assert_eq!(rows(&state, &chat).len(), 2);

        // Removed on the homeserver: removed here. A removal of anything else is nothing.
        let removal = |target: &str| {
            json!({ "type": "m.room.redaction", "room_id": ROOM,
            "event_id": "$r", "sender": format!("@{alice}:arbos.life"), "redacts": target, "content": {} })
        };
        apply(&state, &[removal("$reaction"), removal("$one")]).await;
        let left = rows(&state, &chat);
        assert_eq!(left.len(), 1, "{left:?}");
        assert_eq!(left[0]["id"], "01NATIVETWO");
        apply(&state, &[removal("$one")]).await;
        assert_eq!(rows(&state, &chat).len(), 1);
    }

    #[tokio::test]
    async fn only_a_members_own_message_in_a_native_plaintext_chat_is_carried() {
        let (state, chat, alice, bob) = native_dm(false);
        let ok = sent(&alice, "$ok", text("01OK", "hi"));
        assert!(job_for(&state, &ok).is_some());
        let with = |k: &str, v: Value| {
            let mut e = ok.clone();
            e[k] = v;
            e
        };
        let saying = |content: Value| with("content", content);
        let mut claims = text("01OK", "hi");
        claims["life.arbos.sender"] = json!(format!("user:{bob}"));
        let mut own = text("01OK", "hi");
        own["life.arbos.sender"] = json!(format!("user:{alice}"));
        assert!(job_for(&state, &saying(own)).is_some(), "its own name");
        for (why, ev) in [
            ("another room", with("room_id", json!("!other:arbos.life"))),
            ("no room", with("room_id", Value::Null)),
            ("not a member", with("sender", json!("@gh_9999:arbos.life"))),
            (
                "another homeserver's user",
                with("sender", json!(format!("@{alice}:elsewhere.org"))),
            ),
            ("an agent", with("sender", json!("@_a_01jx:arbos.life"))),
            ("no sender", with("sender", Value::Null)),
            ("a state event", with("state_key", json!(""))),
            ("another type", with("type", json!("m.sticker"))),
            (
                "a row carried in from the Loro room",
                saying(
                    json!({ "msgtype": "m.text", "body": "hi", "life.arbos.id": "01OK",
                               "life.arbos.history": { "v": 1 } }),
                ),
            ),
            ("no time", with("origin_server_ts", Value::Null)),
            ("another member's name", saying(claims)),
            (
                "no id",
                saying(json!({ "msgtype": "m.text", "body": "hi" })),
            ),
            ("an id that is a path", saying(text("../x", "hi"))),
            ("an oversized id", saying(text(&"a".repeat(65), "hi"))),
            (
                "a body that is not text",
                saying(json!({ "msgtype": "m.text", "body": 7, "life.arbos.id": "01OK" })),
            ),
            (
                "an attachment",
                saying(json!({ "msgtype": "m.image", "body": "x.png", "life.arbos.id": "01OK" })),
            ),
        ] {
            assert!(job_for(&state, &ev).is_none(), "{why}");
        }
        // Parts that are not a short list of objects are left behind; the words are carried.
        let mut odd = text("01OK", "hi");
        odd["life.arbos.parts"] = json!(["x"]);
        assert!(matches!(
            job_for(&state, &saying(odd)),
            Some(Job::NativeMessage { parts: None, .. })
        ));
        // The relay's own mirror of a Loro row, coming back: known by its id.
        state
            .db
            .matrix_insert_event(&chat, "01OK", "$ok", &format!("user:{alice}"), 1)
            .expect("event");
        assert!(job_for(&state, &ok).is_none(), "the mirror's own event");

        // A row under the id that is someone else's is not overwritten, nor claimed.
        let worker = Worker::new(state.clone());
        let bobs = job_for(&state, &sent(&bob, "$bobs", text("01BOBS", "bob's words"))).unwrap();
        worker.handle(&bobs).await.expect("bob's row");
        let taken = Job::NativeMessage {
            chat_id: chat.clone(),
            event_id: "$taken".into(),
            user_id: alice.clone(),
            message_id: "01BOBS".into(),
            text: "mine now".into(),
            parts: None,
            reply_to: None,
            ts: 2,
        };
        worker.handle(&taken).await.expect("left alone");
        let got = rows(&state, &chat);
        assert_eq!(got.len(), 1);
        assert_eq!(arbos_core::doc::message_text(&got[0]), "bob's words");
        assert_eq!(
            state.db.matrix_event(&chat, "01BOBS").unwrap(),
            Some("$bobs".to_owned())
        );

        // Someone the other person of the DM has blocked writes no Loro row this way either.
        state.db.block(&bob, &alice).expect("block");
        assert!(job_for(&state, &sent(&alice, "$blocked", text("01BLOCKED", "hi"))).is_none());
        state.db.unblock(&bob, &alice).expect("unblock");
        assert!(job_for(&state, &sent(&alice, "$fine", text("01FINE", "hi"))).is_some());

        // Once a member is not flagged the chat is a Loro room again: nothing is carried.
        assert!(state
            .db
            .set_matrix_people_native(&bob, false)
            .expect("flag"));
        assert!(job_for(&state, &sent(&alice, "$late", text("01LATE", "hi"))).is_none());
    }

    #[tokio::test]
    async fn a_reaction_on_the_homeserver_is_put_on_the_loro_row_and_taken_off_with_its_event() {
        let (state, chat, alice, bob) = native_dm(false);
        apply(&state, &[sent(&alice, "$one", text("01REACTED", "lunch?"))]).await;
        let react = |user: &str, event_id: &str, content: Value| {
            json!({ "type": "life.arbos.reaction", "room_id": ROOM, "event_id": event_id,
                    "sender": format!("@{user}:arbos.life"), "origin_server_ts": 2, "content": content })
        };
        let on = |target: &str, key: &str| json!({ "event_id": target, "key": key, "m.relates_to": { "rel_type": "life.arbos.reaction" } });
        let removal = |target: &str| {
            json!({ "type": "m.room.redaction", "room_id": ROOM, "event_id": "$r",
                    "sender": format!("@{bob}:arbos.life"), "redacts": target, "content": {} })
        };
        let reactions = || rows(&state, &chat)[0]["reactions"].clone();
        let bobs = format!("user:{bob}");

        // Bob reacts from two devices; the row shows it once, and again delivered changes nothing.
        let first = react(&bob, "$r1", on("$one", "👍"));
        apply(
            &state,
            &[first.clone(), react(&bob, "$r2", on("$one", "👍")), first],
        )
        .await;
        assert_eq!(reactions(), json!({ "👍": { bobs.as_str(): true } }));
        // One device takes it back: the other still holds it. Then that one does too.
        apply(&state, &[removal("$r1")]).await;
        assert_eq!(reactions(), json!({ "👍": { bobs.as_str(): true } }));
        apply(&state, &[removal("$r2"), removal("$r2")]).await;
        assert_eq!(reactions(), json!({}));

        for (why, ev) in [
            (
                "to no row of this chat",
                react(&bob, "$r3", on("$nothing", "👍")),
            ),
            ("an essay", react(&bob, "$r4", on("$one", &"a".repeat(65)))),
            (
                "a control character",
                react(&bob, "$r5", on("$one", "a\nb")),
            ),
            ("no key", react(&bob, "$r6", json!({ "event_id": "$one" }))),
            ("from no member", react("gh_9999", "$r7", on("$one", "👍"))),
            (
                "a target only the envelope names",
                react(
                    &bob,
                    "$r8",
                    json!({ "key": "👍", "m.relates_to": { "event_id": "$one" } }),
                ),
            ),
        ] {
            apply(&state, &[ev]).await;
            assert_eq!(reactions(), json!({}), "{why}");
        }
        // Someone the other person of the DM has blocked reacts to nothing here.
        state.db.block(&alice, &bob).expect("block");
        assert!(job_for(&state, &react(&bob, "$r9", on("$one", "👍"))).is_none());
    }

    #[tokio::test]
    async fn the_one_catch_up_is_queued_for_native_plaintext_chats_only() {
        let (state, chat, _, _) = native_dm(false);
        let (sealed, sealed_chat, _, _) = native_dm(true);
        catch_up_once(&sealed);
        assert_eq!(sealed.db.matrix_outbox_stats().unwrap().pending, 0);
        assert!(sealed.db.matrix_room(&sealed_chat).unwrap().is_some());
        catch_up_once(&state);
        let due = state.db.matrix_outbox_due(10).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].job, Job::NativeCatchUp { chat_id: chat });
        state.db.matrix_outbox_done(due[0].id).unwrap();
        // Once: the next start queues nothing.
        catch_up_once(&state);
        assert_eq!(state.db.matrix_outbox_stats().unwrap().pending, 0);
    }

    #[tokio::test]
    async fn an_encrypted_chat_carries_nothing_back() {
        let (state, chat, alice, _) = native_dm(true);
        let sealed = json!({ "type": "m.room.encrypted", "room_id": ROOM, "event_id": "$s",
            "sender": format!("@{alice}:arbos.life"), "origin_server_ts": 1,
            "content": { "algorithm": "m.megolm.v1.aes-sha2", "ciphertext": "AwgA" } });
        // Nor what someone writes into it in the clear.
        let clear = sent(&alice, "$c", text("01CLEAR", "hi"));
        on_transaction(&state, &[sealed, clear]);
        assert_eq!(state.db.matrix_outbox_stats().unwrap().pending, 0);
        assert!(rows(&state, &chat).is_empty());
    }

    /// Against a real homeserver: the room is the one the worker makes, the message and its
    /// removal are sent there the way a member's device sends them, and the relay is handed
    /// the events as the homeserver stores them, on the route the homeserver delivers to.
    ///
    /// Skipped unless `ARBOS_MATRIX_TEST_HS` and `ARBOS_MATRIX_TEST_AS_TOKEN` point at a
    /// throwaway Synapse (see `matrix::worker::tests`); `ARBOS_MATRIX_TEST_SERVER_NAME` is its
    /// server name when that is not `arbos.life` (`scripts/deploy/matrix/test-synapse.sh`:
    /// `arbos.test`, whose appservice is over `@gh_.*`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_native_message_and_its_removal_reach_the_loro_room() {
        use axum::extract::{Path, Query, State};
        let (Ok(hs_url), Ok(as_token)) = (
            std::env::var("ARBOS_MATRIX_TEST_HS"),
            std::env::var("ARBOS_MATRIX_TEST_AS_TOKEN"),
        ) else {
            return;
        };
        let cfg = Config {
            mode: Mode::Dual,
            hs_url: hs_url.clone(),
            server_name: std::env::var("ARBOS_MATRIX_TEST_SERVER_NAME")
                .unwrap_or_else(|_| "arbos.life".into()),
            as_token_users: as_token.clone(),
            as_token_agents: as_token.clone(),
            as_token_bridges: as_token.clone(),
            hs_token_users: "the-homeserver".into(),
            ..Config::off()
        };
        let mut state = test_state(AuthMode::Github);
        state.matrix = Matrix::new(cfg.clone());
        // People the homeserver has not met: it keeps its users from one run to the next.
        let tag = i64::from(std::process::id()) * 10;
        let (alice, _) = person(&state, 7_000_000_000 + tag);
        let (bob, _) = person(&state, 7_000_000_001 + tag);
        let chat = dm(&state, &alice, &bob);
        for u in [&alice, &bob] {
            assert!(state.db.set_matrix_people_native(u, true).expect("flag"));
        }
        let worker = Worker::new(state.clone());
        worker
            .handle(&Job::SyncRoom {
                chat_id: chat.clone(),
                project_id: None,
            })
            .await
            .expect("the room is made");
        let room = state.db.matrix_room(&chat).expect("row").expect("room");
        let client = super::super::client::Client::new(state.http.clone(), cfg.clone());
        let (a, b) = (cfg.user_mxid(&alice), cfg.user_mxid(&bob));

        // Deliver events to the relay as the homeserver does, then run what that queued.
        let deliver = |txn: &'static str, events: Vec<Value>| {
            let state = state.clone();
            async move {
                let worker = Worker::new(state.clone());
                let mut headers = axum::http::HeaderMap::new();
                headers.insert("authorization", "Bearer the-homeserver".parse().unwrap());
                let body = json!({ "events": events }).to_string();
                let r = crate::matrix::appservice::transaction(
                    State(state.clone()),
                    headers,
                    Path(txn.to_owned()),
                    Query(Default::default()),
                    body.into(),
                )
                .await;
                assert_eq!(r.status(), axum::http::StatusCode::OK);
                // In order per chat, as the worker runs them: a carried row leaves a resync
                // due a few seconds later, and what follows waits behind it.
                for _ in 0..100 {
                    for row in state.db.matrix_outbox_due(50).expect("outbox") {
                        worker.handle(&row.job).await.expect("applied");
                        state.db.matrix_outbox_done(row.id).expect("done");
                    }
                    if state.db.matrix_outbox_stats().expect("outbox").pending == 0 {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
                panic!("the outbox did not drain");
            }
        };

        // Bob's device sends; a row the Loro room already had is mirrored the usual way.
        let sent = client
            .send(
                &room.room_id,
                &b,
                "m.room.message",
                "native-1",
                &text("01WIRENATIVE", "said on the homeserver"),
                None,
            )
            .await
            .expect("sent");
        let event = client.event(&room.room_id, &a, &sent).await.expect("event");
        assert_eq!(event["sender"], b.as_str());
        deliver("t1", vec![event.clone()]).await;
        let got = rows(&state, &chat);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0]["id"], "01WIRENATIVE");
        assert_eq!(got[0]["sender"], format!("user:{bob}"));
        assert_eq!(got[0]["createdAt"], event["origin_server_ts"]);
        assert_eq!(
            arbos_core::doc::message_text(&got[0]),
            "said on the homeserver"
        );

        // The Loro room has the row now, and the mirror does not send it to the room again:
        // the resync finds nothing, and the room holds the one message.
        let resync = crate::matrix::backfill::resync_chat(&state, &chat, None);
        assert_eq!(resync.queued, 0, "{resync:?}");
        deliver("t1", vec![event.clone()]).await;
        deliver("t2", vec![event]).await;
        assert_eq!(rows(&state, &chat).len(), 1);
        let messages = |events: Vec<Value>| {
            events
                .into_iter()
                .filter(|e| e["type"] == "m.room.message")
                .count()
        };
        let in_room = client
            .messages_back(&room.room_id, &a, 50)
            .await
            .expect("room");
        assert_eq!(messages(in_room), 1);

        // Bob removes it on the homeserver; the relay is handed the removal.
        let url = format!(
            "{hs_url}/_matrix/client/v3/rooms/{}/redact/{sent}/native-2?user_id={b}",
            room.room_id
        );
        let r = state
            .http
            .put(url)
            .bearer_auth(&as_token)
            .json(&json!({}))
            .send()
            .await
            .expect("redact");
        assert!(r.status().is_success(), "{}", r.status());
        let removal = client
            .messages_back(&room.room_id, &a, 50)
            .await
            .expect("room")
            .into_iter()
            .find(|e| e["type"] == "m.room.redaction")
            .expect("the removal is in the room");
        deliver("t3", vec![removal]).await;
        assert!(rows(&state, &chat).is_empty());

        // What was sent while no relay was listening: a message, a reaction to it and to
        // another that is then taken back. One read of the room brings the Loro room level.
        let say = |who: &str, kind: &'static str, txn: &'static str, content: Value| {
            let (client, room, who) = (client.clone(), room.room_id.clone(), who.to_owned());
            async move {
                client
                    .send(&room, &who, kind, txn, &content, None)
                    .await
                    .expect("sent")
            }
        };
        let missed = say(
            &a,
            "m.room.message",
            "missed-1",
            text("01WIREMISSED", "nobody heard"),
        )
        .await;
        let on = |key: &str| json!({ "event_id": missed, "key": key, "m.relates_to": { "rel_type": "life.arbos.reaction" } });
        say(&b, "life.arbos.reaction", "missed-2", on("👍")).await;
        let taken_back = say(&b, "life.arbos.reaction", "missed-3", on("🎉")).await;
        let url = format!(
            "{hs_url}/_matrix/client/v3/rooms/{}/redact/{taken_back}/missed-4?user_id={b}",
            room.room_id
        );
        let r = state
            .http
            .put(url)
            .bearer_auth(&as_token)
            .json(&json!({}))
            .send()
            .await;
        assert!(r.expect("redact").status().is_success());
        let catch_up = Job::NativeCatchUp {
            chat_id: chat.clone(),
        };
        for _ in 0..2 {
            Worker::new(state.clone())
                .handle(&catch_up)
                .await
                .expect("caught up");
            let got = rows(&state, &chat);
            assert_eq!(got.len(), 1, "{got:?}");
            assert_eq!(got[0]["id"], "01WIREMISSED");
            assert_eq!(got[0]["sender"], format!("user:{alice}"));
            assert_eq!(
                got[0]["reactions"],
                json!({ "👍": { format!("user:{bob}"): true } })
            );
        }
    }
}
