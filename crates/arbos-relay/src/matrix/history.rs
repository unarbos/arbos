//! Phase 3, the history a chat had before its members read it on the homeserver.
//!
//! A people chat's earlier rows live in its Loro room. In an unencrypted room the mirror has
//! already written them, each as its sender, except the rows of people who have since left:
//! those the creator carried, and a row that names someone other than the account that sent
//! it is shown to nobody (`arbos_matrix::rows::sender_is_own`). In an encrypted room the relay
//! wrote nothing at all, because it holds no keys.
//!
//! So the relay vouches and a member's device carries. `GET /chats/{id}/matrix/history` hands
//! a member the next rows that are missing from the room, each signed with the relay's
//! history key over the room, the chat, the row's id, sender, time and content. The device
//! sends them into the room (encrypted, where the room is) and tells the relay which event
//! each became (`POST`). Every member's app shows such a row as its signed sender's and at
//! its signed time, and only with a signature of the key it pinned, made for that very room:
//! a member can withhold history but not write any.
//!
//! History is not news: before a row is handed out, the relay tells the homeserver, for
//! every member, that an event marked as carried history is neither counted as unread nor
//! announced (a push rule on what stays outside the ciphertext: that it is a carried row).
//!
//! One device carries a chat at a time (a short lease), in batches, oldest first. A row is
//! offered until some device's event for it is recorded, so a carry that stops half way is
//! taken up where it stopped, by any member's device. Two events for one row can still exist
//! (a device that sent and never said so); the apps draw one row per id and sender.

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::STANDARD_NO_PAD as B64;
use base64::Engine;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::doc::message_text;
use arbos_core::ids::now_ms;

use super::backfill::{carried_parts, load_doc};
use super::client::Client;
use crate::auth::Principal;
use crate::signin::api_error;
use crate::state::AppState;

/// The schema of a signed row; part of what is signed.
const VERSION: u32 = 1;
/// The most rows one request hands out, and one report names.
const BATCH: usize = 50;
/// What a carried row's event says outside the ciphertext (`m.relates_to.rel_type`), and
/// the id of the push rule over it (`arbos_matrix::rows::HISTORY`).
const HISTORY_REL: &str = "life.arbos.history";
/// How long one device has a chat's carry to itself after its last request.
const LEASE_MS: i64 = 60_000;

/// One row of a chat as it was said, with everything the signature covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    /// `user:<id>` | `agent:<project>` | `system`.
    pub sender: String,
    /// The sender's handle (a person) or name (an agent) when it was said; may be empty.
    pub handle: String,
    pub ts: i64,
    pub text: String,
    /// The id of the row it answers, or empty.
    pub reply_to: String,
    /// The extra parts as JSON text (an array), or empty. Text, so that what is signed is
    /// bytes and not a JSON value two programs could spell differently.
    pub parts: String,
}

/// The bytes the relay signs for `row` in `room_id`, the room of `chat_id`. Every field is
/// length-prefixed, so no two rows share an encoding. `arbos_matrix::rows::history_bytes` is
/// the same function on the reading side; the test vector below is in both.
pub fn signing_bytes(key: &[u8], room_id: &str, chat_id: &str, row: &Row) -> Vec<u8> {
    let mut out = b"arbos.history".to_vec();
    out.extend_from_slice(&VERSION.to_be_bytes());
    let mut field = |bytes: &[u8]| {
        out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        out.extend_from_slice(bytes);
    };
    field(key);
    field(room_id.as_bytes());
    field(chat_id.as_bytes());
    field(row.id.as_bytes());
    field(row.sender.as_bytes());
    field(row.handle.as_bytes());
    field(&row.ts.to_be_bytes());
    field(row.text.as_bytes());
    field(row.reply_to.as_bytes());
    field(row.parts.as_bytes());
    out
}

/// The key the relay signs history with: Ed25519, from the relay key, so it is the same after
/// a restart and on every relay process that shares the key.
pub struct HistoryKey(Ed25519KeyPair);

impl HistoryKey {
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self(Ed25519KeyPair::from_seed_unchecked(seed).expect("a 32-byte seed is a key"))
    }

    fn of(state: &AppState) -> Self {
        Self::from_seed(&state.relay_key.subkey("matrix-history"))
    }

    /// The public half, as the apps pin it (`GET /me/matrix` → `history_key`).
    pub fn public(&self) -> String {
        B64.encode(self.0.public_key().as_ref())
    }

    fn sign(&self, room_id: &str, chat_id: &str, row: &Row) -> String {
        let bytes = signing_bytes(self.0.public_key().as_ref(), room_id, chat_id, row);
        B64.encode(self.0.sign(&bytes).as_ref())
    }
}

/// `history_key` of `GET /me/matrix`.
pub fn public_key(state: &AppState) -> String {
    HistoryKey::of(state).public()
}

/// The rows of a chat's Loro room that are missing from its homeserver room, oldest first.
///
/// Missing from an encrypted room: every finished row no event is recorded for. Missing from
/// an unencrypted one: a row the mirror wrote for someone who is no longer a member, which the
/// creator carried unsigned (or which the apps would attribute to an account that has left);
/// the mirror writes the rest itself.
// The doc is rebuilt from its rows on every request: fine for a chat's one-time carry, and a
// doc kept between the batches of one lease if chats ever have histories that make it slow.
fn pending(state: &AppState, chat_id: &str, encrypted: bool) -> anyhow::Result<Vec<Row>> {
    let Some(doc) = load_doc(state, chat_id, None)? else {
        return Ok(Vec::new());
    };
    let recorded: HashMap<String, bool> = state.db.matrix_events_carried(chat_id)?;
    let members: HashSet<String> = state
        .db
        .members_of(chat_id)?
        .into_iter()
        .map(|m| m.member)
        .collect();
    let mut out = Vec::new();
    for row in doc.projected_rows() {
        if matches!(row["status"].as_str(), Some("queued" | "streaming")) {
            continue;
        }
        let Some(id) = row["id"].as_str() else {
            continue;
        };
        let sender = match row["sender"].as_str() {
            Some(s) if s.starts_with("user:") || s.starts_with("agent:") => s,
            _ => "system",
        };
        let departed = sender != "system" && !members.contains(sender);
        let missing = match recorded.get(id) {
            None => encrypted,
            Some(false) => departed,
            Some(true) => false,
        };
        let text = message_text(&row);
        let parts = carried_parts(&row);
        if !missing || (text.trim().is_empty() && parts.is_none()) {
            continue;
        }
        let name = if sender.starts_with("agent:") {
            &row["senderName"]
        } else {
            &row["senderHandle"]
        };
        out.push(Row {
            id: id.to_owned(),
            sender: sender.to_owned(),
            handle: name.as_str().unwrap_or_default().to_owned(),
            ts: row["createdAt"].as_i64().unwrap_or(0),
            text,
            reply_to: row["replyTo"].as_str().unwrap_or_default().to_owned(),
            parts: parts.map(|p| p.to_string()).unwrap_or_default(),
        });
    }
    Ok(out)
}

/// The caller of a history route, and the chat's room.
struct Carrier {
    user_id: String,
    device_id: String,
    room_id: String,
    encrypted: bool,
    /// Every member of the chat.
    members: Vec<String>,
}

/// The chat's room when the caller is a member whose apps read the chat there. Anything else
/// is one answer: a chat that is not carried says nothing about whether it exists.
fn carried_room(
    state: &AppState,
    principal: &Principal,
    chat_id: &str,
) -> Result<Carrier, Box<Response>> {
    let refused = || {
        Box::new(api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no such chat",
        ))
    };
    let Principal::Device { id: device, .. } = principal else {
        return Err(refused());
    };
    let (me, cm) = crate::people::member_chat(state, principal, chat_id).map_err(|_| refused())?;
    let room_id = super::people_native_room(state, &cm).ok_or_else(refused)?;
    let room = state
        .db
        .matrix_room(chat_id)
        .ok()
        .flatten()
        .filter(|r| r.room_id == room_id)
        .ok_or_else(refused)?;
    Ok(Carrier {
        user_id: me,
        device_id: device.clone(),
        room_id,
        encrypted: room.encrypted,
        members: cm
            .members
            .iter()
            .filter_map(|m| m.user_id().map(str::to_owned))
            .collect(),
    })
}

/// Tell the homeserver, for each of `members` it has not been told for since this relay
/// started, that a carried row is no new message.
async fn quiet(state: &AppState, members: &[String]) -> super::client::MxResult<()> {
    let client = Client::new(state.http.clone(), state.matrix.cfg.clone());
    for user_id in members {
        let known = |set: &HashSet<String>| set.contains(user_id);
        if known(
            &state
                .matrix
                .history_quiet
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
        ) {
            continue;
        }
        client
            .quiet_rel_type(&state.matrix.cfg.user_mxid(user_id), HISTORY_REL)
            .await?;
        state
            .matrix
            .history_quiet
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(user_id.clone());
    }
    Ok(())
}

/// Whether `device` has (or now takes) the chat's carry.
fn lease(state: &AppState, chat_id: &str, device: &str) -> bool {
    let now = now_ms();
    let mut leases = state
        .matrix
        .history_leases
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    leases.retain(|_, (_, until)| *until > now);
    match leases.get(chat_id) {
        Some((holder, _)) if holder != device => false,
        _ => {
            leases.insert(chat_id.to_owned(), (device.to_owned(), now + LEASE_MS));
            true
        }
    }
}

#[derive(Deserialize)]
pub struct PageQuery {
    #[serde(default)]
    limit: Option<usize>,
}

/// `GET /chats/{id}/matrix/history[?limit=]` → `{v, key, room_id, rows, more}`: the next rows
/// for this device to carry into the room. `rows` empty with `more` true means another device
/// is carrying; ask again later.
pub async fn page(
    State(state): State<AppState>,
    principal: Principal,
    Path(chat_id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Response {
    let Carrier {
        device_id,
        room_id,
        encrypted,
        members,
        ..
    } = match carried_room(&state, &principal, &chat_id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let mut rows = match pending(&state, &chat_id, encrypted) {
        Ok(r) => r,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let key = HistoryKey::of(&state);
    let limit = q.limit.unwrap_or(BATCH).clamp(1, BATCH);
    let more = rows.len() > limit;
    if !rows.is_empty() && !lease(&state, &chat_id, &device_id) {
        return Json(json!({ "v": VERSION, "key": key.public(), "room_id": room_id, "rows": [], "more": true }))
            .into_response();
    }
    // No row leaves before every member's account knows it is not a new message.
    if !rows.is_empty() {
        if let Err(e) = quiet(&state, &members).await {
            return api_error(StatusCode::BAD_GATEWAY, "homeserver", e.to_string());
        }
    }
    rows.truncate(limit);
    let rows: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({ "id": r.id, "sender": r.sender, "handle": r.handle, "ts": r.ts, "text": r.text,
                    "reply_to": r.reply_to, "parts": r.parts, "sig": key.sign(&room_id, &chat_id, r) })
        })
        .collect();
    Json(json!({ "v": VERSION, "key": key.public(), "room_id": room_id, "rows": rows, "more": more }))
        .into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Carried {
    id: String,
    event_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    carried: Vec<Carried>,
}

/// `POST /chats/{id}/matrix/history` `{carried: [{id, event_id}]}` → `{recorded}`: the events
/// this device sent for rows it was handed. Each is recorded once the homeserver confirms it:
/// an event of this room, sent by the caller, for a row that was missing. A report of anything
/// else is not an error, it is just not recorded, and the row is offered again.
pub async fn report(
    State(state): State<AppState>,
    principal: Principal,
    Path(chat_id): Path<String>,
    Json(body): Json<Report>,
) -> Response {
    let Carrier {
        user_id: me,
        device_id: device,
        room_id,
        encrypted,
        ..
    } = match carried_room(&state, &principal, &chat_id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if body.carried.len() > BATCH {
        return api_error(StatusCode::BAD_REQUEST, "bad_request", "too many rows");
    }
    let missing: HashMap<String, Row> = match pending(&state, &chat_id, encrypted) {
        Ok(rows) => rows.into_iter().map(|r| (r.id.clone(), r)).collect(),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let client = Client::new(state.http.clone(), state.matrix.cfg.clone());
    let mxid = state.matrix.cfg.user_mxid(&me);
    let mut recorded = 0;
    for c in &body.carried {
        let Some(row) = missing.get(&c.id) else {
            continue;
        };
        if !c.event_id.starts_with('$') || c.event_id.len() > 255 {
            continue;
        }
        let Ok(ev) = client.event(&room_id, &mxid, &c.event_id).await else {
            continue;
        };
        // In an encrypted room the relay sees who sent an event and nothing of what it says.
        let is_carry = if encrypted {
            ev["type"] == "m.room.encrypted"
        } else {
            ev["type"] == "m.room.message"
                && ev["content"]["life.arbos.id"] == row.id.as_str()
                && ev["content"]["life.arbos.history"].is_object()
        };
        if ev["sender"] != mxid.as_str() || ev["room_id"] != room_id.as_str() || !is_carry {
            continue;
        }
        match state
            .db
            .matrix_record_carried(&chat_id, &row.id, &c.event_id, &row.sender, row.ts)
        {
            Ok(()) => recorded += 1,
            Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
        }
    }
    // Still this device's turn while it keeps reporting.
    lease(&state, &chat_id, &device);
    Json(json!({ "recorded": recorded })).into_response()
}

/// How many rows of a chat its room is still missing (the shadow compare's parity figure).
pub fn missing(state: &AppState, chat_id: &str, encrypted: bool) -> Vec<String> {
    pending(state, chat_id, encrypted)
        .map(|rows| rows.into_iter().map(|r| r.id).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::{Config, Matrix, Mode};
    use crate::notices::write_doc;
    use crate::people::tests_support::{group, person};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;
    use arbos_core::doc::UserMessageOpts;

    const ROOM: &str = "!history:arbos.life";

    /// The same vector as `arbos_matrix::rows` (`a_carried_row_is_the_relays_word`): the two
    /// sides agree on the bytes and on what a signature over them looks like.
    #[test]
    fn the_signed_bytes_are_the_ones_the_apps_check() {
        let key = HistoryKey::from_seed(&[7u8; 32]);
        assert_eq!(key.public(), "6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iw");
        let row = Row {
            id: "01HISTORY".into(),
            sender: "user:gh_8".into(),
            handle: "mira".into(),
            ts: 1_700_000_000_123,
            text: "see you at noon".into(),
            reply_to: String::new(),
            parts: String::new(),
        };
        assert_eq!(
            key.sign("!r:arbos.test", "01CHAT", &row),
            "DA2DHf8+cNdovLKYvOsViEsQuPrDcVDMyLgmSAsErXD3Ubl1JFRzfszRh+Kr9Ov5G5sZcjkn0OuRzyOckTytDQ"
        );
        // Every field moves the bytes, and no two fields can trade a character.
        let bytes = |room: &str, chat: &str, r: &Row| signing_bytes(b"k", room, chat, r);
        let base = bytes("!r", "c", &row);
        let mut other = row.clone();
        other.id = "01HISTOR".into();
        other.sender = "Yuser:gh_8".into();
        assert_ne!(base, bytes("!r", "c", &other));
        assert_ne!(base, bytes("!r2", "c", &row));
        assert_ne!(base, bytes("!r", "c2", &row));
        for change in [
            |r: &mut Row| r.handle.push('x'),
            |r: &mut Row| r.ts += 1,
            |r: &mut Row| r.text.push('x'),
            |r: &mut Row| r.reply_to.push('x'),
            |r: &mut Row| r.parts.push('x'),
        ] {
            let mut r = row.clone();
            change(&mut r);
            assert_ne!(base, bytes("!r", "c", &r));
        }
    }

    /// A group of three flagged people with a room, and two rows said in its Loro room: one
    /// by a member who then left.
    async fn carried_group(encrypted: bool) -> (AppState, String, [(String, Principal); 3]) {
        let cfg = Config {
            mode: Mode::Dual,
            // Nothing answers as the homeserver here.
            hs_url: "http://127.0.0.1:9".into(),
            ..Config::off()
        };
        let (state, chat, people) = group_with_history(cfg, 1401).await;
        for (u, _) in &people {
            // As if the homeserver had been told for each already.
            let mut told = state.matrix.history_quiet.lock().unwrap();
            told.insert(u.clone());
        }
        state
            .db
            .matrix_insert_room(&chat, ROOM, None, "group", encrypted)
            .expect("room");
        (state, chat, people)
    }

    /// Three flagged people (GitHub ids from `first`) in a group whose Loro room has two rows,
    /// one by the third, who then left. No room yet.
    async fn group_with_history(
        cfg: Config,
        first: i64,
    ) -> (AppState, String, [(String, Principal); 3]) {
        let mut state = test_state(AuthMode::Github);
        state.matrix = Matrix::new(cfg);
        let people = [first, first + 1, first + 2].map(|n| person(&state, n));
        let ids: Vec<&str> = people.iter().map(|(u, _)| u.as_str()).collect();
        let chat = group(&state, &ids);
        for u in &ids {
            assert!(state.db.set_matrix_people_native(u, true).expect("flag"));
        }
        for (id, who, text) in [
            ("01FIRST", ids[2], "I was here"),
            ("01SECOND", ids[0], "so was I"),
        ] {
            let opts = UserMessageOpts {
                sender: Some(format!("user:{who}")),
                sender_handle: Some(format!("handle-{who}")),
                ..Default::default()
            };
            write_doc(&state, None, &chat, |doc| {
                doc.transact(|d| d.append_user_message(id, text, "dev", &opts))?;
                Ok(())
            })
            .await
            .expect("row");
        }
        state
            .db
            .remove_member(&chat, &format!("user:{}", ids[2]), false)
            .expect("left");
        (state, chat, people)
    }

    async fn get(state: &AppState, who: &Principal, chat: &str) -> (StatusCode, Value) {
        let r = page(
            State(state.clone()),
            who.clone(),
            Path(chat.to_owned()),
            Query(PageQuery { limit: None }),
        )
        .await;
        let status = r.status();
        let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn an_encrypted_room_is_missing_every_row_and_a_plain_one_the_departed_members() {
        let (state, chat, people) = carried_group(true).await;
        let (status, body) = get(&state, &people[0].1, &chat).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["room_id"], ROOM);
        assert_eq!(body["key"], public_key(&state));
        let rows = body["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2, "{body}");
        assert_eq!(rows[0]["id"], "01FIRST");
        assert_eq!(rows[0]["sender"], format!("user:{}", people[2].0));
        assert_eq!(rows[0]["handle"], format!("handle-{}", people[2].0));
        assert_eq!(rows[0]["text"], "I was here");
        assert_eq!(body["more"], false);
        // Once a device's event for a row is recorded the row is not offered again.
        state
            .db
            .matrix_record_carried(&chat, "01FIRST", "$one", "user:x", 1)
            .unwrap();
        let (_, body) = get(&state, &people[0].1, &chat).await;
        assert_eq!(body["rows"].as_array().unwrap().len(), 1);
        assert_eq!(body["rows"][0]["id"], "01SECOND");

        // Unencrypted: the mirror wrote both rows; the one whose sender left is carried again,
        // signed, and only until it has been.
        let (state, chat, people) = carried_group(false).await;
        assert!(get(&state, &people[0].1, &chat).await.1["rows"]
            .as_array()
            .unwrap()
            .is_empty());
        for (id, who) in [("01FIRST", &people[2].0), ("01SECOND", &people[0].0)] {
            state
                .db
                .matrix_insert_event(&chat, id, &format!("${id}"), &format!("user:{who}"), 1)
                .unwrap();
        }
        let (_, body) = get(&state, &people[0].1, &chat).await;
        let rows = body["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{body}");
        assert_eq!(rows[0]["id"], "01FIRST");
        state
            .db
            .matrix_record_carried(&chat, "01FIRST", "$carried", "user:x", 1)
            .unwrap();
        assert!(get(&state, &people[0].1, &chat).await.1["rows"]
            .as_array()
            .unwrap()
            .is_empty());
        // The mirror's own event for the row is still the one a read receipt is put on.
        assert_eq!(
            state.db.matrix_event(&chat, "01FIRST").unwrap(),
            Some("$01FIRST".to_owned())
        );
    }

    #[tokio::test]
    async fn only_a_member_whose_apps_read_the_chat_there_is_handed_its_history() {
        let (state, chat, people) = carried_group(true).await;
        let refused = |r: (StatusCode, Value)| {
            assert_eq!(r.0, StatusCode::NOT_FOUND, "{}", r.1);
            assert!(r.1["rows"].is_null());
        };
        // Someone who left, someone who was never in it, the admin token, a box.
        refused(get(&state, &people[2].1, &chat).await);
        let (_, stranger) = person(&state, 1499);
        refused(get(&state, &stranger, &chat).await);
        refused(get(&state, &Principal::Admin, &chat).await);
        let as_box = Principal::Box {
            user_id: people[0].0.clone(),
        };
        refused(get(&state, &as_box, &chat).await);
        refused(get(&state, &people[0].1, "01NOSUCHCHAT").await);
        // A report is refused the same way, and one that names no missing row records nothing.
        let post = |who: Principal, carried: Value| {
            report(
                State(state.clone()),
                who,
                Path(chat.clone()),
                Json(serde_json::from_value(json!({ "carried": carried })).unwrap()),
            )
        };
        let said = json!([{ "id": "01FIRST", "event_id": "$x" }]);
        assert_eq!(
            post(stranger, said.clone()).await.status(),
            StatusCode::NOT_FOUND
        );
        // The homeserver does not answer here, so nothing is confirmed and nothing recorded.
        let r = post(people[0].1.clone(), said).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(state.db.matrix_event(&chat, "01FIRST").unwrap(), None);
        let many: Vec<Value> = (0..=BATCH)
            .map(|i| json!({ "id": format!("m{i}"), "event_id": "$x" }))
            .collect();
        assert_eq!(
            post(people[0].1.clone(), json!(many)).await.status(),
            StatusCode::BAD_REQUEST
        );

        // One device carries at a time: the second is told to come back.
        let (_, other_device) = (
            (),
            Principal::Device {
                id: "another-device".into(),
                token: String::new(),
                user_id: people[1].0.clone(),
            },
        );
        let (_, first) = get(&state, &people[0].1, &chat).await;
        assert_eq!(first["rows"].as_array().unwrap().len(), 2);
        let (_, second) = get(&state, &other_device, &chat).await;
        assert!(second["rows"].as_array().unwrap().is_empty());
        assert_eq!(second["more"], true);

        // A member the homeserver could not be told about: no row leaves the relay.
        state.matrix.history_leases.lock().unwrap().clear();
        state.matrix.history_quiet.lock().unwrap().clear();
        let (status, body) = get(&state, &people[0].1, &chat).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(body["rows"].is_null(), "{body}");

        // A chat one of whose members is not flagged lives in its Loro room: nothing to carry.
        assert!(state
            .db
            .set_matrix_people_native(&people[1].0, false)
            .unwrap());
        refused(get(&state, &people[0].1, &chat).await);
    }

    /// Against a real homeserver (see `matrix::native::tests`): the room is the encrypted one
    /// the worker makes, the members' accounts are told before a row is handed out, and a
    /// row is recorded as carried only for an event the homeserver says the caller sent there.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn history_leaves_once_the_homeserver_is_told_and_is_recorded_once_it_confirms() {
        use crate::matrix::worker::Worker;
        use crate::matrix::Job;
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
            e2ee: true,
            ..Config::off()
        };
        let first = 7_100_000_000 + i64::from(std::process::id()) * 10;
        let (state, chat, people) = group_with_history(cfg.clone(), first).await;
        Worker::new(state.clone())
            .handle(&Job::SyncRoom {
                chat_id: chat.clone(),
                project_id: None,
            })
            .await
            .expect("the room is made");
        let room = state.db.matrix_room(&chat).expect("row").expect("room");
        assert!(room.encrypted);
        let client = Client::new(state.http.clone(), cfg.clone());
        let [a, b] = [0, 1].map(|i| cfg.user_mxid(&people[i].0));

        let (status, body) = get(&state, &people[0].1, &chat).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["room_id"], room.room_id.as_str());
        assert_eq!(body["rows"].as_array().unwrap().len(), 2, "{body}");
        // Both members' accounts now hold the rule under which a carried row is no news.
        for mxid in [&a, &b] {
            let url = format!(
                "{hs_url}/_matrix/client/v3/pushrules/global/override/{HISTORY_REL}?user_id={mxid}"
            );
            let r = state.http.get(url).bearer_auth(&as_token).send().await;
            let rule: Value = r.expect("asked").json().await.expect("a rule");
            assert_eq!(rule["actions"], json!([]), "{rule}");
            assert_eq!(rule["conditions"][0]["value"], HISTORY_REL, "{rule}");
        }

        // What a member's device sends: ciphertext, marked as a carried row and nothing else.
        let sealed = json!({ "algorithm": "m.megolm.v1.aes-sha2", "ciphertext": "AwgA",
            "sender_key": "k", "session_id": "s", "device_id": "D",
            "m.relates_to": { "rel_type": HISTORY_REL } });
        let send = |who: &str, txn: &'static str| {
            let (client, room, who, sealed) = (
                client.clone(),
                room.room_id.clone(),
                who.to_owned(),
                sealed.clone(),
            );
            async move {
                client
                    .send(&room, &who, "m.room.encrypted", txn, &sealed, None)
                    .await
                    .expect("sent")
            }
        };
        let (alices, bobs) = (send(&a, "h01FIRST").await, send(&b, "h01SECOND").await);
        let post = |carried: Value| {
            let (state, who, chat) = (state.clone(), people[0].1.clone(), chat.clone());
            async move {
                let r = report(
                    State(state),
                    who,
                    Path(chat),
                    Json(serde_json::from_value(json!({ "carried": carried })).unwrap()),
                )
                .await;
                let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
                serde_json::from_slice::<Value>(&body).unwrap()["recorded"].clone()
            }
        };
        // Her own event for a missing row counts; someone else's, one for a row that is not
        // missing, and one the room does not have, do not.
        let said = json!([
            { "id": "01FIRST", "event_id": alices },
            { "id": "01SECOND", "event_id": bobs },
            { "id": "01NOSUCHROW", "event_id": alices },
            { "id": "01SECOND", "event_id": "$nothing" },
        ]);
        assert_eq!(post(said).await, 1);
        assert_eq!(
            state.db.matrix_event(&chat, "01FIRST").unwrap(),
            Some(alices.clone())
        );
        let (_, body) = get(&state, &people[0].1, &chat).await;
        assert_eq!(body["rows"].as_array().unwrap().len(), 1, "{body}");
        assert_eq!(body["rows"][0]["id"], "01SECOND");

        // The parity check names the row still waiting, and nothing once it is carried.
        let diff = |rep: crate::matrix::shadow::Report| rep.diffs[0].missing.clone();
        let rep = crate::matrix::shadow::run(&state, Some(&chat), false).await;
        assert_eq!(diff(rep), ["01SECOND"]);
        // (Her own transaction id: the homeserver keeps one set of them for the appservice.)
        let own = send(&a, "h01SECOND-hers").await;
        assert_eq!(
            post(json!([{ "id": "01SECOND", "event_id": own }])).await,
            1
        );
        let rep = crate::matrix::shadow::run(&state, Some(&chat), false).await;
        assert!(rep.diffs[0].ok, "{:?}", rep.diffs[0]);
    }
}
