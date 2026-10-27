//! What a push looks like on the wire, per kind. One place decides the APNs shape so the
//! message path, the badge refresh and the (future) clear push agree on `thread-id`,
//! `collapse-id` and friends.
//!
//! APNs alert for a message (`PushKind::Message`):
//! - `aps.alert.{title, body}` — the sender (agent name / `@handle`) and the brief;
//! - `aps.thread-id` = the chat id — the lock screen groups one chat's banners;
//! - `aps.badge` = the recipient's total unread across chats;
//! - `aps.mutable-content: 1` + `aps.category` — the notification service extension turns
//!   it into a communication notification (sender avatar, intent);
//! - `apns-collapse-id` = `msg:<message_id>` — a re-send of the same message replaces the
//!   banner instead of stacking a second one.
//!
//! Background clear (`PushKind::Clear`, for the clear-on-read worker): `aps.content-available:
//! 1`, no alert, priority 5, `apns-collapse-id` = `clear:<chat_id>`; the app removes delivered
//! banners of that chat up to `upto_message_id` and sets the badge.

use serde_json::{json, Value};

use crate::notify::{ChatRef, Sender, CATEGORY};

/// The kinds of push the relay sends. The kind decides the APNs push type, priority, the
/// collapse id and the dedupe key; the payload is built by the matching constructor below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushKind {
    /// A whole message landed in a chat: an alert with the text.
    Message { chat_id: String, message_id: String },
    /// The user read something elsewhere: only the badge changes.
    Badge { total: i64 },
    /// Clear delivered banners of a chat (silent, background). `upto_message_id` = the newest
    /// message the user has seen; `None` = everything in the chat.
    Clear {
        chat_id: String,
        upto_message_id: Option<String>,
    },
}

impl PushKind {
    /// `apns-push-type`.
    pub fn push_type(&self) -> &'static str {
        match self {
            PushKind::Message { .. } | PushKind::Badge { .. } => "alert",
            PushKind::Clear { .. } => "background",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            PushKind::Message { .. } => "message",
            PushKind::Badge { .. } => "badge",
            PushKind::Clear { .. } => "clear",
        }
    }

    /// `apns-collapse-id`: later pushes with the same id replace earlier ones on the device.
    pub fn collapse_id(&self, user_id: &str) -> String {
        match self {
            PushKind::Message { message_id, .. } => format!("msg:{message_id}"),
            PushKind::Badge { .. } => format!("badge:{user_id}"),
            PushKind::Clear { chat_id, .. } => format!("clear:{chat_id}"),
        }
    }

    /// The outbox key: one row per (kind, subject, device). A second enqueue with the same key
    /// is ignored, which is what makes "each message pushes at most once per device" hold
    /// across retries, duplicate announcements and relay restarts.
    pub fn dedupe_key(&self, device_id: &str, enqueued_at: i64) -> String {
        match self {
            PushKind::Message {
                chat_id,
                message_id,
            } => format!("msg:{chat_id}:{message_id}:{device_id}"),
            // Badge refreshes are not deduplicated — each read is its own count.
            PushKind::Badge { total } => format!("badge:{device_id}:{total}:{enqueued_at}"),
            PushKind::Clear {
                chat_id,
                upto_message_id,
            } => format!(
                "clear:{chat_id}:{}:{device_id}",
                upto_message_id.as_deref().unwrap_or("*")
            ),
        }
    }

    /// Serialised for the outbox row (and back with [`PushKind::from_stored`]).
    pub fn to_stored(&self) -> Value {
        match self {
            PushKind::Message {
                chat_id,
                message_id,
            } => json!({ "kind": "message", "chat_id": chat_id, "message_id": message_id }),
            PushKind::Badge { total } => json!({ "kind": "badge", "total": total }),
            PushKind::Clear {
                chat_id,
                upto_message_id,
            } => json!({ "kind": "clear", "chat_id": chat_id, "upto_message_id": upto_message_id }),
        }
    }

    pub fn from_stored(v: &Value) -> Option<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        match v.get("kind").and_then(Value::as_str)? {
            "message" => Some(PushKind::Message {
                chat_id: s("chat_id")?,
                message_id: s("message_id")?,
            }),
            "badge" => Some(PushKind::Badge {
                total: v.get("total").and_then(Value::as_i64).unwrap_or(0),
            }),
            "clear" => Some(PushKind::Clear {
                chat_id: s("chat_id")?,
                upto_message_id: s("upto_message_id"),
            }),
            _ => None,
        }
    }
}

/// The alert for one message. `chat_title` is set for group-style chats ("sender in <chat>").
pub fn message_alert(
    chat: &ChatRef,
    sender: &Sender,
    message_id: &str,
    body: &str,
    badge: i64,
    chat_title: Option<&str>,
) -> Value {
    let mut v = json!({
        "aps": {
            "alert": { "title": sender.title(), "body": body },
            "badge": badge,
            "sound": "default",
            "thread-id": chat.chat_id,
            "category": CATEGORY,
            "mutable-content": 1,
        },
        "chat_id": chat.chat_id,
        "project_id": chat.project_label(),
        "message_id": message_id,
        "sender": sender.json(),
    });
    if let Some(t) = chat_title {
        v["chat_title"] = json!(t);
    }
    v
}

/// Badge only: the lock screen shows nothing new, the app icon count changes.
pub fn badge_only(total: i64) -> Value {
    json!({ "aps": { "badge": total } })
}

/// Silent clear for one chat. The app side (iPhone, the clear-on-read item) reads `type`,
/// `chat_id`, `upto_message_id` and `badge`. No caller on the relay yet: the clear-on-read
/// item enqueues it.
#[allow(dead_code)]
pub fn clear(chat_id: &str, upto_message_id: Option<&str>, badge: i64) -> Value {
    let mut v = json!({
        "aps": { "content-available": 1 },
        "type": "clear",
        "chat_id": chat_id,
        "badge": badge,
    });
    if let Some(m) = upto_message_id {
        v["upto_message_id"] = json!(m);
    }
    v
}

#[cfg(test)]
mod tests {
    //! Payload shape: the fields the iPhone app and its notification service extension read.

    use super::*;

    fn sender() -> Sender {
        Sender {
            kind: "user",
            id: "u1".into(),
            name: "Alice".into(),
            handle: "alice".into(),
            avatar: None,
            avatar_color: None,
            avatar_url: None,
            user_id: Some("u1".into()),
            avatar_color_index: None,
            leaf: None,
        }
    }

    #[test]
    fn a_message_alert_carries_thread_badge_mutable_content_and_category() {
        let chat = ChatRef::people("chat-1");
        let p = message_alert(&chat, &sender(), "m-9", "hello", 4, None);
        assert_eq!(p["aps"]["alert"]["title"], "@alice");
        assert_eq!(p["aps"]["alert"]["body"], "hello");
        assert_eq!(p["aps"]["thread-id"], "chat-1");
        assert_eq!(p["aps"]["badge"], 4);
        assert_eq!(p["aps"]["mutable-content"], 1);
        assert_eq!(p["aps"]["category"], CATEGORY);
        assert_eq!(p["aps"]["sound"], "default");
        assert_eq!(p["chat_id"], "chat-1");
        assert_eq!(p["project_id"], "people");
        assert_eq!(p["message_id"], "m-9");
        assert_eq!(p["sender"]["handle"], "alice");
        assert!(p.get("chat_title").is_none());

        let group = ChatRef::project("p-1", "chat-2");
        let p = message_alert(&group, &sender(), "m-1", "x", 0, Some("Research"));
        assert_eq!(p["chat_title"], "Research");
        assert_eq!(p["project_id"], "p-1");
    }

    #[test]
    fn kinds_decide_push_type_collapse_and_dedupe() {
        let msg = PushKind::Message {
            chat_id: "c".into(),
            message_id: "m".into(),
        };
        assert_eq!(msg.push_type(), "alert");
        assert_eq!(msg.collapse_id("u"), "msg:m");
        assert_eq!(msg.dedupe_key("d", 1), "msg:c:m:d");
        assert_eq!(msg.dedupe_key("d", 2), "msg:c:m:d", "time plays no part");

        let badge = PushKind::Badge { total: 3 };
        assert_eq!(badge.push_type(), "alert");
        assert_eq!(badge.collapse_id("u"), "badge:u");
        assert_ne!(badge.dedupe_key("d", 1), badge.dedupe_key("d", 2));

        let clear = PushKind::Clear {
            chat_id: "c".into(),
            upto_message_id: Some("m".into()),
        };
        assert_eq!(clear.push_type(), "background");
        assert_eq!(clear.collapse_id("u"), "clear:c");
        assert_eq!(clear.dedupe_key("d", 0), "clear:c:m:d");
        let p = super::clear("c", Some("m"), 2);
        assert_eq!(p["aps"]["content-available"], 1);
        assert!(p["aps"].get("alert").is_none());
        assert_eq!(p["type"], "clear");
        assert_eq!(p["upto_message_id"], "m");

        for k in [msg, badge, clear] {
            assert_eq!(PushKind::from_stored(&k.to_stored()).as_ref(), Some(&k));
        }
    }
}
