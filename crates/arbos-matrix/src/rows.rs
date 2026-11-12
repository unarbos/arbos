//! What the apps render, built from room state and events. [`MessageRow`] is the chat-doc
//! message shape (`crdt-sync.md` §6.2: `id`, `role`, `sender`, `status`, `parts`, `createdAt`)
//! so the existing transcript draws a channel like any chat; [`ChannelRow`] / [`SpaceRow`] feed
//! the left list.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// One message as the transcript expects it (JSON in `json`, the keys the apps already read).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageRow {
    pub event_id: String,
    /// `life.arbos.id` when the relay or an app wrote it, else the event id.
    pub id: String,
    /// `user:<id>` | `agent:<project>` | `system`.
    pub sender: String,
    /// The sender's Matrix display name: the relay registers people under their
    /// **handle** and agents under their name, so for a `user:` row this is the
    /// handle (the apps look the display name up in their rosters); empty for
    /// a system line.
    pub sender_name: String,
    pub text: String,
    pub created_at: i64,
    /// This device cannot decrypt the event (yet): `text` is [`LOCKED`], never the ciphertext.
    pub locked: bool,
    /// This device's own send, shown before the homeserver has it ([`echo_row`]); the event
    /// replaces it. Its `event_id` is not an event's.
    #[serde(default)]
    pub pending: bool,
    /// Full row for the transcript (same shape as a chat-doc message).
    pub json: String,
}

/// A channel in the left list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelRow {
    pub room_id: String,
    /// The relay's channel id (`life.arbos.chat.chat_id`).
    pub channel_id: String,
    pub server_id: String,
    pub name: String,
    /// Only owner/admins post (lock in the list).
    pub restricted: bool,
    /// Can this device post here (its power level meets `events_default`).
    pub can_post: bool,
    pub unread: u64,
    pub last_text: String,
    /// The newest row's `sender` (`user:<id>` | `agent:<project>` | `system`), so the
    /// list can label a person the way a group's preview does.
    pub last_sender: String,
    /// The newest row's [`MessageRow::sender_name`] (a person's handle).
    pub last_sender_name: String,
    pub last_ts: i64,
    pub order: String,
}

/// A server (Space) in the left list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpaceRow {
    pub room_id: String,
    pub server_id: String,
    pub name: String,
    pub member_count: u64,
    pub unread: u64,
    pub last_text: String,
    /// See [`ChannelRow::last_sender`].
    pub last_sender: String,
    pub last_sender_name: String,
    pub last_ts: i64,
    pub channel_ids: Vec<String>,
}

/// A DM or group in the left list (phase 3: people chats read from the homeserver).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeopleRoomRow {
    pub room_id: String,
    /// The relay's chat id (`life.arbos.chat.chat_id`, the same in the room's create event).
    pub chat_id: String,
    /// `dm` | `group`.
    pub kind: String,
    /// Messages here are end-to-end encrypted; not knowing counts as encrypted.
    pub encrypted: bool,
    pub unread: u64,
    pub mentions: u64,
    pub last_text: String,
    /// See [`ChannelRow::last_sender`].
    pub last_sender: String,
    pub last_sender_name: String,
    pub last_ts: i64,
}

/// `life.arbos.chat` as the relay writes it on a chat's room (`matrix/worker.rs::chat_state`,
/// and `close_room`'s marker). Anything else in there is not a state this build knows how to
/// read, so the room is not listed rather than half understood.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // every field is type-checked; only some are read
struct ChatState {
    chat_id: String,
    kind: String,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    created_by: Option<String>,
    #[serde(default)]
    created_at: Option<i64>,
    #[serde(default)]
    epoch: Option<i64>,
    #[serde(default)]
    closed_at: Option<i64>,
}

/// The relay's ids are ULIDs; this is the widest shape an app will take as a chat id.
fn is_chat_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// `(chat_id, kind)` when a room is an open people chat: `state` is its `life.arbos.chat`
/// content, `create` its `m.room.create` content. Any member with the power to send state can
/// rewrite `life.arbos.chat`; nobody can rewrite the create event, so a room is the chat the
/// relay created it as (`creation_content.life.arbos.chat_id`) or it is not listed. A project
/// chat (`project_id`) is not a people chat, whatever its kind.
pub fn people_chat(state: &Value, create: &Value) -> Option<(String, String)> {
    let s: ChatState = serde_json::from_value(state.clone()).ok()?;
    let people = matches!(s.kind.as_str(), "dm" | "group") && s.project_id.is_none();
    let created_as = create["life.arbos.chat_id"].as_str() == Some(s.chat_id.as_str());
    (people && s.closed_at.is_none() && is_chat_id(&s.chat_id) && created_as)
        .then_some((s.chat_id, s.kind))
}

/// `@gh_1:arbos.life` → `user:gh_1`; `@_a_<ulid>` → `agent:<ULID>`; anything else → `system`.
pub fn member_of_mxid(mxid: &str) -> String {
    let local = mxid
        .trim_start_matches('@')
        .split(':')
        .next()
        .unwrap_or_default();
    if let Some(p) = local.strip_prefix("_a_") {
        format!("agent:{}", p.to_ascii_uppercase())
    } else if local.starts_with('_') || local.is_empty() {
        "system".to_owned()
    } else {
        format!("user:{local}")
    }
}

/// What a message this device cannot decrypt (yet) reads as.
pub const LOCKED: &str = "🔒 Waiting for this message's key";

/// Events that become transcript rows: messages, and encrypted events that did not decrypt.
pub fn is_message(ev: &Value) -> bool {
    matches!(
        ev["type"].as_str(),
        Some("m.room.message" | "m.room.encrypted")
    )
}

/// The one message an encrypted room shows although it was not encrypted: the relay's system
/// line (`matrix/spaces.rs::channel_notice`), which says nothing the member list does not.
fn is_system_notice(ev: &Value) -> bool {
    let c = &ev["content"];
    c["msgtype"] == "m.notice"
        && c["life.arbos.sender"] == "system"
        && c["life.arbos.parts"].is_null()
}

/// Whether an event becomes a row. In an encrypted room a message that did not arrive
/// encrypted is whatever the homeserver (or any member) chose to write, so it is dropped
/// rather than drawn like a message its sender's device vouched for.
pub fn shows(ev: &Value, room_encrypted: bool, decrypted: bool) -> bool {
    is_message(ev)
        && !is_redacted(ev)
        && (!room_encrypted
            || decrypted
            || ev["type"] == "m.room.encrypted"
            || is_system_notice(ev))
}

/// Whether a people-chat event says nothing about its sender that the homeserver does not:
/// `life.arbos.sender` is absent, names the Matrix `sender`, or marks the relay's system
/// line. A row's author in a people chat is the account that sent the event, so a message
/// carried for someone else (`life.arbos.on_behalf`, a past member's backfilled history) or
/// one that simply names another member is not drawn.
// Owner decision (spec §7 phase 3 backfill): dropped rather than attributed to whoever
// carried it; the backfill slice decides how a past member's history is shown.
pub fn sender_is_own(ev: &Value) -> bool {
    match ev["content"].get("life.arbos.sender") {
        None => true,
        Some(claimed) => {
            is_system_notice(ev)
                || claimed.as_str() == ev["sender"].as_str().map(member_of_mxid).as_deref()
        }
    }
}

/// A removed event as the homeserver hands it out afterwards: its content stripped.
fn is_redacted(ev: &Value) -> bool {
    ev["unsigned"]["redacted_because"].is_object()
}

/// The longest reaction an app sends or draws (bytes): an emoji with its modifiers, not text.
pub const REACTION_KEY_MAX: usize = 64;

pub fn is_reaction_key(key: &str) -> bool {
    (1..=REACTION_KEY_MAX).contains(&key.len()) && !key.chars().any(char::is_control)
}

/// One member's reaction on a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    /// The row reacted to, by event id.
    pub target: String,
    pub key: String,
    /// Who reacted: the account that sent the event ([`member_of_mxid`]), nothing it says.
    pub sender: String,
}

/// `(the reaction's own event id, the reaction)` when `ev` is a well-formed `m.reaction`.
pub fn reaction(ev: &Value) -> Option<(String, Reaction)> {
    let rel = &ev["content"]["m.relates_to"];
    let key = rel["key"].as_str().filter(|k| is_reaction_key(k))?;
    (ev["type"] == "m.reaction" && rel["rel_type"] == "m.annotation").then_some(())?;
    Some((
        ev["event_id"].as_str()?.to_owned(),
        Reaction {
            target: rel["event_id"].as_str()?.to_owned(),
            key: key.to_owned(),
            sender: member_of_mxid(ev["sender"].as_str()?),
        },
    ))
}

/// The event an `m.room.redaction` removes (top level before room version 11, in the content
/// from it on).
pub fn redacts(ev: &Value) -> Option<String> {
    (ev["type"] == "m.room.redaction").then_some(())?;
    ev["redacts"]
        .as_str()
        .or_else(|| ev["content"]["redacts"].as_str())
        .map(str::to_owned)
}

/// The event id a row answers (`replyToEvent`, from the event's `m.in_reply_to`).
pub fn reply_target(row: &MessageRow) -> Option<String> {
    row.json.contains("\"replyToEvent\"").then_some(())?;
    serde_json::from_str::<Value>(&row.json).ok()?["replyToEvent"]
        .as_str()
        .map(str::to_owned)
}

/// What [`echo_row`] puts before the local id to make a pending row's `event_id`; no event id
/// starts with it (they start with `$`).
const ECHO: &str = "~";

/// The row of a message this device is sending, as it will read once the homeserver has it:
/// `content` is the event content, `local_id` its `life.arbos.id`.
pub fn echo_row(me: &str, content: &Value, local_id: &str, now_ms: i64) -> Option<MessageRow> {
    let ev = json!({ "type": "m.room.message", "event_id": format!("{ECHO}{local_id}"),
                     "sender": me, "origin_server_ts": now_ms, "content": content });
    let mut row = message_row(&ev, &|_| None)?;
    let mut v: Value = serde_json::from_str(&row.json).ok()?;
    v["pending"] = json!(true);
    row.json = v.to_string();
    row.pending = true;
    Some(row)
}

/// Write what the room's other events say about a row into its JSON: `reactions` in the
/// chat-doc's projected shape (`emoji → { member → true }`) and `replyTo`, the id of the row
/// it answers.
pub fn decorate(
    row: &mut MessageRow,
    reactions: &std::collections::BTreeMap<&str, std::collections::BTreeMap<&str, bool>>,
    reply_to: Option<&str>,
) {
    let Ok(mut v) = serde_json::from_str::<Value>(&row.json) else {
        return;
    };
    v["reactions"] = json!(reactions);
    if let Some(id) = reply_to {
        v["replyTo"] = json!(id);
    }
    row.json = v.to_string();
}

/// Build a [`MessageRow`] from one `m.room.message` event (JSON as the server sent it).
/// `display_name` resolves the sender's Matrix display name — the relay registers a person
/// under their **handle** (`matrix/worker.rs::display_name`), an agent under its name. A
/// person's row therefore carries `senderHandle` and **no** `senderName`: the apps resolve
/// the display name from their rosters (server members, people chats, the directory), the
/// way a group bubble shows it ([feedback server-member-names-2026-10-28]; before, the
/// handle was written into `senderName` too and every channel bubble read "A" for Scout).
/// An agent's row keeps its name in `senderName`.
pub fn message_row(
    ev: &Value,
    display_name: &dyn Fn(&str) -> Option<String>,
) -> Option<MessageRow> {
    if !is_message(ev) {
        return None;
    }
    let locked = ev["type"] == "m.room.encrypted";
    let content = &ev["content"];
    let event_id = ev["event_id"].as_str()?.to_owned();
    let mxid = ev["sender"].as_str().unwrap_or_default();
    let sender = content["life.arbos.sender"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| member_of_mxid(mxid));
    let id = content["life.arbos.id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| event_id.clone());
    let text = if locked {
        LOCKED.to_owned()
    } else {
        content["body"].as_str().unwrap_or_default().to_owned()
    };
    let created_at = ev["origin_server_ts"].as_i64().unwrap_or(0);
    let role = if sender.starts_with("agent:") {
        "assistant"
    } else if sender == "system" {
        "system"
    } else {
        "user"
    };
    // A system line (membership notice, #1386) is nobody's message: no sender
    // name, so the channel list shows "Mira K. added Sam Rivera", not
    // "launchcrew: Mira K. added Sam Rivera" (the owner only carried it).
    let sender_name = if role == "system" {
        String::new()
    } else {
        display_name(mxid).unwrap_or_else(|| {
            mxid.trim_start_matches('@')
                .split(':')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
    };
    let person = sender.starts_with("user:");
    let handle = if person {
        sender_name.clone()
    } else {
        String::new()
    };
    let mut parts = vec![json!({ "id": format!("{event_id}#text"), "kind": "text", "text": text })];
    if let Some(extra) = content["life.arbos.parts"].as_array().filter(|_| !locked) {
        parts.extend(extra.iter().cloned());
    }
    let mut row = json!({
        "id": id,
        "eventId": event_id,
        "role": role,
        "sender": sender,
        "senderHandle": handle,
        "status": "done",
        "createdAt": created_at,
        "completedAt": created_at,
        "parts": parts,
        "reactions": {},
    });
    if !person {
        row["senderName"] = json!(sender_name);
    }
    // Nothing from an undecrypted event's envelope.
    if let Some(e) = content["m.relates_to"]["m.in_reply_to"]["event_id"]
        .as_str()
        .filter(|_| !locked)
    {
        row["replyToEvent"] = json!(e);
    }
    Some(MessageRow {
        event_id,
        id,
        sender,
        sender_name,
        text,
        created_at,
        locked,
        pending: false,
        json: row.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mxid_to_member() {
        assert_eq!(member_of_mxid("@gh_12:arbos.life"), "user:gh_12");
        assert_eq!(member_of_mxid("@_a_01jabc:arbos.life"), "agent:01JABC");
        assert_eq!(member_of_mxid("@_arbos:arbos.life"), "system");
    }

    #[test]
    fn row_from_event() {
        let ev = json!({
            "type": "m.room.message", "event_id": "$e1", "sender": "@gh_7:arbos.life", "origin_server_ts": 1700,
            "content": { "msgtype": "m.text", "body": "hi", "life.arbos.id": "01ULID" }
        });
        let row = message_row(&ev, &|m| {
            (m == "@gh_7:arbos.life").then(|| "kenobi".to_owned())
        })
        .unwrap();
        assert_eq!(row.id, "01ULID");
        assert_eq!(row.sender, "user:gh_7");
        assert_eq!(
            row.sender_name, "kenobi",
            "the Matrix display name = the handle"
        );
        let v: Value = serde_json::from_str(&row.json).unwrap();
        assert_eq!(v["role"], "user");
        assert_eq!(v["parts"][0]["text"], "hi");
        assert_eq!(v["senderHandle"], "kenobi");
        // A person's display name is not the handle: the row leaves it to the app's roster
        // ([feedback server-member-names-2026-10-28]).
        assert!(
            v.get("senderName").is_none(),
            "no senderName on a person's row: {v}"
        );
        let agent = json!({
            "type": "m.room.message", "event_id": "$e2", "sender": "@_a_01x:arbos.life", "origin_server_ts": 1800,
            "content": { "msgtype": "m.text", "body": "done" }
        });
        let row = message_row(&agent, &|_| None).unwrap();
        assert_eq!(row.sender, "agent:01X");
        let v: Value = serde_json::from_str(&row.json).unwrap();
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["senderHandle"], "", "agents carry no handle");
        // An agent's Matrix display name is its name: kept on the row.
        let named = message_row(&agent, &|_| Some("Comms".to_owned())).unwrap();
        let v: Value = serde_json::from_str(&named.json).unwrap();
        assert_eq!(v["senderName"], "Comms");
        assert!(message_row(&json!({ "type": "m.room.member" }), &|_| None).is_none());
        // Undecryptable: a locked row in its place, never the ciphertext.
        let enc = json!({
            "type": "m.room.encrypted", "event_id": "$e4", "sender": "@gh_7:arbos.life", "origin_server_ts": 2000,
            "content": { "algorithm": "m.megolm.v1.aes-sha2", "ciphertext": "AwgA…", "life.arbos.parts": [{"kind": "x"}] }
        });
        let row = message_row(&enc, &|_| None).unwrap();
        assert!(row.locked);
        assert_eq!(row.sender, "user:gh_7");
        let v: Value = serde_json::from_str(&row.json).unwrap();
        assert_eq!(
            v["parts"].as_array().unwrap().len(),
            1,
            "nothing from the envelope"
        );
        // A membership notice: the owner sent it, the content names the system.
        let notice = json!({
            "type": "m.room.message", "event_id": "$e3", "sender": "@gh_7:arbos.life", "origin_server_ts": 1900,
            "content": { "msgtype": "m.notice", "body": "Mira K. added Sam Rivera", "life.arbos.sender": "system", "life.arbos.id": "01N" }
        });
        let row = message_row(&notice, &|_| Some("launchcrew".to_owned())).unwrap();
        assert_eq!(row.sender, "system");
        assert_eq!(row.sender_name, "", "a system line carries no sender name");
        let v: Value = serde_json::from_str(&row.json).unwrap();
        assert_eq!(v["role"], "system");
        assert_eq!(v["senderHandle"], "");
        // Anyone can type the locked row's words; that does not make a row locked.
        let typed = json!({
            "type": "m.room.message", "event_id": "$e5", "sender": "@gh_7:arbos.life",
            "content": { "msgtype": "m.text", "body": LOCKED }
        });
        assert!(!message_row(&typed, &|_| None).unwrap().locked);
    }

    #[test]
    fn a_people_chat_is_the_room_the_relay_created_for_it() {
        let state = json!({ "chat_id": "01JCHAT", "kind": "dm", "project_id": null,
            "created_by": "@gh_1:arbos.life", "created_at": 1700, "epoch": 1 });
        let create = json!({ "room_version": "12", "life.arbos.chat_id": "01JCHAT" });
        assert_eq!(
            people_chat(&state, &create),
            Some(("01JCHAT".to_owned(), "dm".to_owned()))
        );
        let with = |k: &str, v: Value| {
            let mut s = state.clone();
            s[k] = v;
            s
        };
        assert!(people_chat(&with("kind", json!("group")), &create).is_some());
        for (why, s, c) in [
            ("a channel", with("kind", json!("channel")), create.clone()),
            (
                "a project chat",
                with("project_id", json!("01JP")),
                create.clone(),
            ),
            ("closed", with("closed_at", json!(1800)), create.clone()),
            (
                "an unknown field",
                with("server_id", json!("x")),
                create.clone(),
            ),
            (
                "a chat id of the wrong type",
                with("chat_id", json!(7)),
                create.clone(),
            ),
            (
                "an empty chat id",
                with("chat_id", json!("")),
                json!({ "life.arbos.chat_id": "" }),
            ),
            (
                "a chat id that is a path",
                with("chat_id", json!("../x")),
                json!({ "life.arbos.chat_id": "../x" }),
            ),
            (
                "an oversized chat id",
                with("chat_id", json!("a".repeat(65))),
                json!({ "life.arbos.chat_id": "a".repeat(65) }),
            ),
            // State rewritten to another chat's id: the create event still names the first.
            (
                "another chat's id",
                with("chat_id", json!("01JOTHER")),
                create.clone(),
            ),
            (
                "no id in the create event",
                state.clone(),
                json!({ "room_version": "12" }),
            ),
            ("no state", Value::Null, create.clone()),
        ] {
            assert_eq!(people_chat(&s, &c), None, "{why}");
        }
    }

    #[test]
    fn a_people_row_is_from_the_account_that_sent_it() {
        let ev = |sender: Value, content: Value| json!({ "type": "m.room.message", "event_id": "$e", "sender": sender, "content": content });
        let mine = json!("@gh_7:arbos.life");
        for (why, e, own) in [
            (
                "no claim",
                ev(mine.clone(), json!({ "msgtype": "m.text", "body": "hi" })),
                true,
            ),
            (
                "the relay's mirror names the sender itself",
                ev(
                    mine.clone(),
                    json!({ "body": "hi", "life.arbos.sender": "user:gh_7" }),
                ),
                true,
            ),
            (
                "another member's name",
                ev(
                    mine.clone(),
                    json!({ "body": "hi", "life.arbos.sender": "user:gh_8" }),
                ),
                false,
            ),
            (
                "carried for a past member",
                ev(
                    mine.clone(),
                    json!({ "body": "hi", "life.arbos.sender": "user:gh_8", "life.arbos.on_behalf": true }),
                ),
                false,
            ),
            (
                "an agent's name",
                ev(
                    mine.clone(),
                    json!({ "body": "hi", "life.arbos.sender": "agent:01X" }),
                ),
                false,
            ),
            (
                "the system line",
                ev(
                    mine.clone(),
                    json!({ "msgtype": "m.notice", "body": "a added b", "life.arbos.sender": "system" }),
                ),
                true,
            ),
            (
                "a message dressed as the system",
                ev(
                    mine.clone(),
                    json!({ "msgtype": "m.text", "body": "x", "life.arbos.sender": "system" }),
                ),
                false,
            ),
            (
                "a claim that is not a string",
                ev(mine.clone(), json!({ "life.arbos.sender": 7 })),
                false,
            ),
            (
                "a null claim",
                ev(mine.clone(), json!({ "life.arbos.sender": null })),
                false,
            ),
            (
                "no sender at all",
                ev(Value::Null, json!({ "life.arbos.sender": "user:gh_7" })),
                false,
            ),
            // Undecryptable: the envelope carries no claim, the row is the sender's locked row.
            (
                "a locked event",
                json!({ "type": "m.room.encrypted", "sender": "@gh_7:arbos.life", "content": { "ciphertext": "x" } }),
                true,
            ),
        ] {
            assert_eq!(sender_is_own(&e), own, "{why}");
        }
    }

    #[test]
    fn reactions_removals_and_replies_are_read_from_the_event_not_its_claims() {
        let react = |content: Value| json!({ "type": "m.reaction", "event_id": "$r", "sender": "@gh_7:arbos.life", "content": content });
        let rel = |key: Value| json!({ "m.relates_to": { "rel_type": "m.annotation", "event_id": "$m", "key": key } });
        assert_eq!(
            reaction(&react(rel(json!("👍")))),
            Some((
                "$r".to_owned(),
                Reaction {
                    target: "$m".into(),
                    key: "👍".into(),
                    sender: "user:gh_7".into()
                }
            ))
        );
        // Who reacted is the account that sent it, whatever the content adds.
        let mut claims = rel(json!("👍"));
        claims["life.arbos.sender"] = json!("user:gh_8");
        assert_eq!(reaction(&react(claims)).unwrap().1.sender, "user:gh_7");
        for (why, ev) in [
            ("an empty key", react(rel(json!("")))),
            ("a key that is not a string", react(rel(json!(7)))),
            (
                "an essay",
                react(rel(json!("a".repeat(REACTION_KEY_MAX + 1)))),
            ),
            ("a control character", react(rel(json!("a\nb")))),
            (
                "another relation",
                react(
                    json!({ "m.relates_to": { "rel_type": "m.thread", "event_id": "$m", "key": "👍" } }),
                ),
            ),
            (
                "no target",
                react(json!({ "m.relates_to": { "rel_type": "m.annotation", "key": "👍" } })),
            ),
            // A removed reaction comes back with its content stripped.
            ("removed", react(json!({}))),
            (
                "a message dressed as a reaction",
                json!({ "type": "m.room.message", "event_id": "$r", "sender": "@gh_7:arbos.life", "content": rel(json!("👍")) }),
            ),
        ] {
            assert_eq!(reaction(&ev), None, "{why}");
        }
        assert_eq!(
            redacts(&json!({ "type": "m.room.redaction", "redacts": "$m", "content": {} })),
            Some("$m".to_owned())
        );
        assert_eq!(
            redacts(&json!({ "type": "m.room.redaction", "content": { "redacts": "$m" } })),
            Some("$m".to_owned()),
            "room version 11 on"
        );
        assert_eq!(
            redacts(
                &json!({ "type": "m.room.message", "redacts": "$m", "content": { "redacts": "$m" } })
            ),
            None
        );
        // A removed message is not a row, encrypted or not.
        let gone = json!({ "type": "m.room.message", "content": {}, "unsigned": { "redacted_because": { "type": "m.room.redaction" } } });
        assert!(!shows(&gone, false, false));
        let mut gone_locked = gone.clone();
        gone_locked["type"] = json!("m.room.encrypted");
        assert!(!shows(&gone_locked, true, false));
        let answer = |relates: Value, kind: &str| {
            let ev = json!({ "type": kind, "event_id": "$a", "sender": "@gh_7:arbos.life",
                             "content": { "body": "\"replyToEvent\"", "m.relates_to": relates } });
            reply_target(&message_row(&ev, &|_| None).unwrap())
        };
        let to = |id: Value| json!({ "m.in_reply_to": { "event_id": id } });
        assert_eq!(
            answer(to(json!("$m")), "m.room.message"),
            Some("$m".to_owned())
        );
        assert_eq!(answer(to(json!(7)), "m.room.message"), None);
        assert_eq!(answer(json!({}), "m.room.message"), None);
        // An undecrypted event's envelope says nothing the row repeats.
        assert_eq!(answer(to(json!("$m")), "m.room.encrypted"), None);
    }

    #[test]
    fn a_pending_row_reads_like_the_message_it_becomes() {
        let content =
            json!({ "msgtype": "m.text", "body": "on my way", "life.arbos.id": "01LOCAL" });
        let mut row = echo_row("@gh_7:arbos.life", &content, "01LOCAL", 1700).unwrap();
        assert!(row.pending && !row.locked);
        assert_eq!(
            (row.id.as_str(), row.sender.as_str(), row.text.as_str()),
            ("01LOCAL", "user:gh_7", "on my way")
        );
        assert!(!row.event_id.starts_with('$'), "not an event id");
        let v: Value = serde_json::from_str(&row.json).unwrap();
        assert_eq!(v["pending"], true);
        let sent = json!({ "type": "m.room.message", "event_id": "$e", "sender": "@gh_7:arbos.life",
                           "origin_server_ts": 1800, "content": content });
        let sent = message_row(&sent, &|_| None).unwrap();
        assert!(!sent.pending);
        assert_eq!(
            (sent.id.as_str(), sent.sender.as_str()),
            ("01LOCAL", "user:gh_7")
        );
        // Reactions and the row it answers are written in the chat-doc's shape.
        let reactions = [("👍", [("user:gh_8", true)].into())].into();
        decorate(&mut row, &reactions, Some("01TARGET"));
        let v: Value = serde_json::from_str(&row.json).unwrap();
        assert_eq!(v["reactions"], json!({ "👍": { "user:gh_8": true } }));
        assert_eq!(v["replyTo"], "01TARGET");
        assert_eq!(v["parts"][0]["text"], "on my way");
    }

    #[test]
    fn an_encrypted_room_shows_no_cleartext_message() {
        let clear =
            json!({ "type": "m.room.message", "content": { "msgtype": "m.text", "body": "hi" } });
        let notice = json!({ "type": "m.room.message",
            "content": { "msgtype": "m.notice", "body": "Mira K. added Sam Rivera", "life.arbos.sender": "system" } });
        // A "system" line that carries parts (a cheque card, say) is not the relay's notice.
        let dressed = json!({ "type": "m.room.message",
            "content": { "msgtype": "m.notice", "body": "x", "life.arbos.sender": "system", "life.arbos.parts": [{ "kind": "x" }] } });
        let locked = json!({ "type": "m.room.encrypted", "content": {} });
        // (event, room encrypted, decrypted) -> shown
        for (ev, encrypted, decrypted, shown) in [
            (&clear, false, false, true),
            (&clear, true, true, true),
            (&clear, true, false, false),
            (&dressed, true, false, false),
            (&notice, true, false, true),
            (&locked, true, false, true),
            (&json!({ "type": "m.room.member" }), false, false, false),
        ] {
            assert_eq!(shows(ev, encrypted, decrypted), shown, "{ev}");
        }
    }
}
