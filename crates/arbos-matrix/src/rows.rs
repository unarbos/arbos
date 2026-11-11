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
        && (!room_encrypted
            || decrypted
            || ev["type"] == "m.room.encrypted"
            || is_system_notice(ev))
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
    Some(MessageRow {
        event_id,
        id,
        sender,
        sender_name,
        text,
        created_at,
        locked,
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
