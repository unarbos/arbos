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

/// Build a [`MessageRow`] from one `m.room.message` event (JSON as the server sent it).
/// `display_name` resolves the sender's Matrix display name — the relay registers a person
/// under their **handle** (`matrix/worker.rs::display_name`), an agent under its name. A
/// person's row therefore carries `senderHandle` and **no** `senderName`: the apps resolve
/// the display name from their rosters (server members, people chats, the directory), the
/// way a group bubble shows it ([feedback server-member-names-2026-10-28]; before, the
/// handle was written into `senderName` too and every channel bubble read "A" for Scout).
/// An agent's row keeps its name in `senderName`.
///
/// `may_declare_sender` answers whether an mxid is entitled to speak for the room — to name
/// a sender other than itself and to supply rendered parts; pass the room's creators
/// ([`crate::client`] does). Every device holds its own homeserver access token and sends
/// straight to the homeserver, so event content is the sending member's to choose: without
/// this, a member could declare `life.arbos.sender` and have the apps draw their message as
/// the system, as the project's agent, or as another person. The relay needs the key
/// honoured in exactly two places —
/// a room creator carrying a notice or a departed member's message
/// (`matrix/spaces.rs::channel_notice`, `matrix/worker.rs` on behalf) — and otherwise writes
/// the value the sender's own mxid already implies.
pub fn message_row(
    ev: &Value,
    display_name: &dyn Fn(&str) -> Option<String>,
    may_declare_sender: &dyn Fn(&str) -> bool,
) -> Option<MessageRow> {
    if ev["type"].as_str() != Some("m.room.message") {
        return None;
    }
    let content = &ev["content"];
    let event_id = ev["event_id"].as_str()?.to_owned();
    let mxid = ev["sender"].as_str().unwrap_or_default();
    let attributed = member_of_mxid(mxid);
    // A declared sender is honoured when it only restates who the homeserver says sent the
    // event, or when the sender may speak for the room. Relay ids are lower-cased into the
    // localpart (`matrix/mod.rs::localpart`), so `user:u_01ABC` arrives as `@u_01abc` and the
    // comparison is case-insensitive; the declared spelling is kept, because that is the id
    // the rest of the app matches against.
    let sender = match content["life.arbos.sender"].as_str() {
        Some(declared)
            if declared.eq_ignore_ascii_case(&attributed) || may_declare_sender(mxid) =>
        {
            declared.to_owned()
        }
        _ => attributed,
    };
    let id = content["life.arbos.id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| event_id.clone());
    let text = content["body"].as_str().unwrap_or_default().to_owned();
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
    // Rendered parts are the room's to write, not a member's: the apps draw a `link_preview`
    // as a branded card that opens whatever destination it names, and an `approval` as the
    // native secret hand-off card with its own project id. The apps' own send path writes no
    // parts ([`crate::client::MatrixClient::send_text`]), and the relay writes them only for
    // mirrored chat rooms, which this client never surfaces.
    if may_declare_sender(mxid) {
        if let Some(extra) = content["life.arbos.parts"].as_array() {
            parts.extend(extra.iter().cloned());
        }
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

    /// No mxid may name a sender other than itself: the rule for a room whose creator is
    /// someone else (or is not known yet).
    fn nobody(_: &str) -> bool {
        false
    }

    #[test]
    fn row_from_event() {
        let ev = json!({
            "type": "m.room.message", "event_id": "$e1", "sender": "@gh_7:arbos.life", "origin_server_ts": 1700,
            "content": { "msgtype": "m.text", "body": "hi", "life.arbos.id": "01ULID" }
        });
        let row = message_row(
            &ev,
            &|m| (m == "@gh_7:arbos.life").then(|| "kenobi".to_owned()),
            &nobody,
        )
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
        let row = message_row(&agent, &|_| None, &nobody).unwrap();
        assert_eq!(row.sender, "agent:01X");
        let v: Value = serde_json::from_str(&row.json).unwrap();
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["senderHandle"], "", "agents carry no handle");
        // An agent's Matrix display name is its name: kept on the row.
        let named = message_row(&agent, &|_| Some("Comms".to_owned()), &nobody).unwrap();
        let v: Value = serde_json::from_str(&named.json).unwrap();
        assert_eq!(v["senderName"], "Comms");
        assert!(message_row(&json!({ "type": "m.room.member" }), &|_| None, &nobody).is_none());
        // A membership notice: the owner sent it, the content names the system.
        let notice = json!({
            "type": "m.room.message", "event_id": "$e3", "sender": "@gh_7:arbos.life", "origin_server_ts": 1900,
            "content": { "msgtype": "m.notice", "body": "Mira K. added Sam Rivera", "life.arbos.sender": "system", "life.arbos.id": "01N" }
        });
        let row = message_row(&notice, &|_| Some("launchcrew".to_owned()), &|m| {
            m == "@gh_7:arbos.life"
        })
        .unwrap();
        assert_eq!(row.sender, "system");
        assert_eq!(row.sender_name, "", "a system line carries no sender name");
        let v: Value = serde_json::from_str(&row.json).unwrap();
        assert_eq!(v["role"], "system");
        assert_eq!(v["senderHandle"], "");
    }

    /// A member sends with their own homeserver access token, so `content` is theirs to
    /// write. Declaring someone else's sender must not move the row away from them.
    #[test]
    fn a_member_cannot_declare_another_sender() {
        // The attacker is an ordinary member; `@gh_1` created the room.
        let creator = |m: &str| m == "@gh_1:arbos.life";
        let forged = |declared: &str| {
            json!({
                "type": "m.room.message", "event_id": "$f", "sender": "@gh_9:arbos.life",
                "origin_server_ts": 2000,
                "content": { "msgtype": "m.text", "body": "ship it", "life.arbos.sender": declared }
            })
        };

        // As the system: would draw as a centred system line nobody can reply to.
        let row = message_row(&forged("system"), &|_| None, &creator).unwrap();
        assert_eq!(
            row.sender, "user:gh_9",
            "a forged system line stays its author's"
        );
        let v: Value = serde_json::from_str(&row.json).unwrap();
        assert_eq!(v["role"], "user", "and is not given the system role");

        // As the project's agent: would draw with the agent's name and avatar.
        let row = message_row(&forged("agent:01X"), &|_| None, &creator).unwrap();
        assert_eq!(
            row.sender, "user:gh_9",
            "a forged agent reply stays its author's"
        );
        let v: Value = serde_json::from_str(&row.json).unwrap();
        assert_eq!(v["role"], "user", "and is not given the assistant role");

        // As another person: would draw as that person in everyone else's transcript, and
        // in their own it would draw as something they said themselves.
        let row = message_row(&forged("user:gh_1"), &|_| None, &creator).unwrap();
        assert_eq!(
            row.sender, "user:gh_9",
            "a forged person stays its author's"
        );
    }

    /// The two writes the relay actually makes, which the rule must not break.
    #[test]
    fn the_room_creator_still_speaks_for_the_room() {
        let creator = |m: &str| m == "@gh_1:arbos.life";

        // `matrix/spaces.rs::channel_notice`: the owner's mxid carries a system line.
        let notice = json!({
            "type": "m.room.message", "event_id": "$n", "sender": "@gh_1:arbos.life",
            "origin_server_ts": 2100,
            "content": { "msgtype": "m.notice", "body": "Mira K. added Sam Rivera", "life.arbos.sender": "system" }
        });
        let row = message_row(&notice, &|_| Some("launchcrew".to_owned()), &creator).unwrap();
        assert_eq!(row.sender, "system");

        // `matrix/worker.rs`: a member who has left is carried by the creator, with the real
        // author in content.
        let on_behalf = json!({
            "type": "m.room.message", "event_id": "$o", "sender": "@gh_1:arbos.life",
            "origin_server_ts": 2200,
            "content": { "msgtype": "m.notice", "body": "from before", "life.arbos.sender": "user:gh_4", "life.arbos.on_behalf": true }
        });
        let row = message_row(&on_behalf, &|_| None, &creator).unwrap();
        assert_eq!(
            row.sender, "user:gh_4",
            "a departed member keeps their message"
        );
    }

    /// `matrix/mod.rs::localpart` lower-cases a relay id for the localpart, so a `u_<ULID>`
    /// member's own declaration differs from the mxid only in case. It is still their own,
    /// and the declared spelling is what the rest of the app matches on.
    #[test]
    fn a_members_own_sender_survives_the_localpart_lowercasing() {
        let ev = json!({
            "type": "m.room.message", "event_id": "$u", "sender": "@u_01abcdef:arbos.life",
            "origin_server_ts": 2300,
            "content": { "msgtype": "m.text", "body": "mine", "life.arbos.sender": "user:u_01ABCDEF" }
        });
        let row = message_row(&ev, &|_| None, &nobody).unwrap();
        assert_eq!(row.sender, "user:u_01ABCDEF");
    }

    /// A member's own message renders its text and nothing else. The part kinds the apps draw
    /// carry a destination (`link_preview` opens `canonicalUrl` while showing `siteHost`) or
    /// an action (`approval` with a `secret_offer` payload draws the credential card and
    /// chooses the project its Save writes to), so a member must not be able to author them.
    #[test]
    fn a_member_cannot_inject_rendered_parts() {
        let ev = json!({
            "type": "m.room.message", "event_id": "$p", "sender": "@gh_9:arbos.life",
            "origin_server_ts": 2400,
            "content": {
                "msgtype": "m.text", "body": "look at this",
                "life.arbos.parts": [
                    { "id": "p1", "kind": "link_preview", "specVersion": 1, "url": "https://github.com/arbos/arbos", "canonicalUrl": "file:///etc/passwd", "siteHost": "github.com", "title": "arbos/arbos" },
                    { "id": "p2", "kind": "approval", "approvalId": "a1", "state": "pending", "payload": { "kind": "secret_offer", "name": "STRIPE_KEY", "projectId": "01ATTACKERPROJECT" } }
                ]
            }
        });
        let row = message_row(&ev, &|_| None, &nobody).unwrap();
        let v: Value = serde_json::from_str(&row.json).unwrap();
        assert_eq!(v["parts"][0]["text"], "look at this");
        assert_eq!(
            v["parts"].as_array().map(Vec::len),
            Some(1),
            "a member's row carries only its text: {v}"
        );
    }

    /// The relay is the room's creator, and its own rows keep their parts.
    #[test]
    fn the_room_creator_still_carries_parts() {
        let ev = json!({
            "type": "m.room.message", "event_id": "$c", "sender": "@_arbos:arbos.life",
            "origin_server_ts": 2500,
            "content": {
                "msgtype": "m.notice", "body": "a link",
                "life.arbos.parts": [{ "id": "p1", "kind": "link_preview", "specVersion": 1, "url": "https://arbos.life", "siteHost": "arbos.life" }]
            }
        });
        let row = message_row(&ev, &|_| None, &|m| m == "@_arbos:arbos.life").unwrap();
        let v: Value = serde_json::from_str(&row.json).unwrap();
        assert_eq!(v["parts"][1]["kind"], "link_preview");
    }
}
