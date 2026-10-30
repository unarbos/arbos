//! Chat doc on Loro, shaped as `crdt-sync.md` §6.2:
//!
//! ```text
//! meta:      Map { chatId, projectId, schemaVersion, epoch }
//! messages:  List<Map { id, role, ..., parts: List<Map { kind, text: Text, ... }> }>
//! commands:  List<Map { id, kind, payload, issuedBy, issuedAt, status }>
//! ```
//!
//! A thin reader over [`arbos_core::doc::ChatDoc`]: every write goes through the
//! core writers (inside `core.transact`). Reading goes through `get_deep_value` →
//! JSON → [`Message`], so the schema stays additive.

use std::collections::BTreeSet;

use loro::event::DiffEvent;
use loro::{Container, LoroList, LoroMap, LoroValue, ValueOrContainer};

use crate::model::{Id, Message};

/// What a root-level diff touched, mapped to message indexes.
#[derive(Debug, Default)]
pub struct Touched {
    /// The `messages` list itself changed (insert/delete): re-read everything.
    pub list_changed: bool,
    pub messages: BTreeSet<usize>,
}

pub struct ChatDoc {
    pub chat_id: Id,
    pub core: arbos_core::doc::ChatDoc,
}

impl ChatDoc {
    pub fn new(chat_id: &str, project_id: &str) -> Self {
        let core = arbos_core::doc::ChatDoc::new(None, false);
        core.init_meta(chat_id, project_id, 1);
        Self {
            chat_id: chat_id.to_string(),
            core,
        }
    }

    /// Fixture-side session fork (#1257): the source transcript under a new
    /// chat id. Falls back to an empty doc when the snapshot does not load.
    pub fn forked_from_snapshot(chat_id: &str, project_id: &str, snapshot: &[u8]) -> Self {
        let Ok(core) = arbos_core::doc::ChatDoc::from_snapshot(None, false, snapshot) else {
            return Self::new(chat_id, project_id);
        };
        core.init_meta(chat_id, project_id, 1);
        Self {
            chat_id: chat_id.to_string(),
            core,
        }
    }

    /// A replica with no local `meta`: the box writes it and it arrives by sync.
    pub fn empty(chat_id: &str) -> Self {
        Self {
            chat_id: chat_id.to_string(),
            core: arbos_core::doc::ChatDoc::new(None, false),
        }
    }

    pub fn messages(&self) -> LoroList {
        self.core.doc.get_list("messages")
    }

    pub fn len(&self) -> usize {
        self.messages().len()
    }

    pub fn index_of(&self, message_id: &str) -> Option<usize> {
        let list = self.messages();
        (0..list.len()).find(|&ix| {
            message_map(&list, ix)
                .and_then(|m| m.get("id"))
                .and_then(|v| match v {
                    ValueOrContainer::Value(LoroValue::String(s)) => Some(s.as_str() == message_id),
                    ValueOrContainer::Value(_) | ValueOrContainer::Container(_) => None,
                })
                .unwrap_or(false)
        })
    }

    pub fn read_message(&self, ix: usize) -> Option<Message> {
        let map = message_map(&self.messages(), ix)?;
        let mut json = serde_json::to_value(map.get_deep_value()).ok()?;
        arbos_core::doc::project_hidden_parts(&mut json);
        arbos_core::doc::project_reactions(&mut json);
        serde_json::from_value(json).ok()
    }

    pub fn read_all(&self) -> Vec<Message> {
        (0..self.len())
            .filter_map(|ix| self.read_message(ix))
            .collect()
    }

    /// Map a root diff to message indexes (delegates to `arbos_core::doc::touched`).
    pub fn touched(event: &DiffEvent<'_>) -> Touched {
        let t = arbos_core::doc::touched(event);
        Touched {
            list_changed: t.list_changed,
            messages: t.messages,
        }
    }
}

fn message_map(list: &LoroList, ix: usize) -> Option<LoroMap> {
    container_map(list.get(ix))
}

fn container_map(v: Option<ValueOrContainer>) -> Option<LoroMap> {
    match v {
        Some(ValueOrContainer::Container(Container::Map(m))) => Some(m),
        Some(ValueOrContainer::Container(_)) | Some(ValueOrContainer::Value(_)) | None => None,
    }
}

#[cfg(test)]
mod core_writer_tests {
    use super::*;
    use crate::model::{Part, Role};
    use arbos_core::doc::{UserAttachment, UserMessageOpts};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn message_from_core_row(row: &serde_json::Value) -> Message {
        let mut json = row.clone();
        arbos_core::doc::project_hidden_parts(&mut json);
        arbos_core::doc::project_reactions(&mut json);
        serde_json::from_value(json).expect("core row deserializes as Message")
    }

    #[test]
    fn core_client_writers_deserialize_as_mac_message() {
        let core = arbos_core::doc::ChatDoc::new(None, false);
        let plain = "plain text for mac";
        core.transact(|d| {
            d.append_user_message(
                "m1",
                plain,
                "dev:mac",
                &UserMessageOpts {
                    attachments: vec![UserAttachment {
                        name: "a.png".into(),
                        mime: "image/png".into(),
                        kind: Some("image".into()),
                        blob_ref: Some("ref1".into()),
                        url: None,
                        source: None,
                    }],
                    reply_to: Some("other".into()),
                    ..UserMessageOpts::default()
                },
            )
            .unwrap();
            d.append_system_line("m2", "Call ended · 1 min", "dev:mac", Some("call-9"))
                .unwrap();
            d.append_greeting("m3", "Hi, I'm Arbos.", "dev:mac", "proj", "Arbos")
                .unwrap();
            d.set_reaction("m1", "heart", "user:1", true).unwrap();
        });

        let rows = core.to_json()["messages"].as_array().cloned().unwrap();
        assert_eq!(rows.len(), 3);

        let user = message_from_core_row(&rows[0]);
        assert_eq!(user.plain_text(), plain);
        assert_eq!(user.role, Role::User);
        assert_eq!(user.reply_to.as_deref(), Some("other"));
        assert!(user
            .reactions
            .get("heart")
            .unwrap()
            .get("user:1")
            .copied()
            .unwrap());
        assert_eq!(user.parts.len(), 2);

        let sys = message_from_core_row(&rows[1]);
        assert_eq!(sys.plain_text(), "Call ended · 1 min");
        assert_eq!(sys.role, Role::System);
        assert_eq!(sys.call_id.as_deref(), Some("call-9"));

        let greet = message_from_core_row(&rows[2]);
        assert_eq!(greet.plain_text(), "Hi, I'm Arbos.");
        assert!(greet.greeting);
        assert_eq!(greet.sender, "agent:proj");
    }

    /// Tester bug ([feedback picture-reply-quote-mac-2026-10-28]): a picture sent as a
    /// reply — no words at all — must still leave the relay row with its `replyTo`,
    /// so every device (sender's and recipient's) draws the quote.
    #[test]
    fn wordless_picture_reply_row_keeps_reply_to() {
        let core = arbos_core::doc::ChatDoc::new(None, false);
        core.transact(|d| {
            d.append_user_message(
                "m1",
                "",
                "dev:mac",
                &UserMessageOpts {
                    attachments: vec![UserAttachment {
                        name: "IMG_0042.jpeg".into(),
                        mime: "image/jpeg".into(),
                        kind: Some("image".into()),
                        blob_ref: Some("blob-42".into()),
                        url: None,
                        source: None,
                    }],
                    reply_to: Some("original".into()),
                    ..UserMessageOpts::default()
                },
            )
            .unwrap();
        });
        let rows = core.to_json()["messages"].as_array().cloned().unwrap();
        assert_eq!(rows[0]["replyTo"], "original");
        let user = message_from_core_row(&rows[0]);
        assert_eq!(user.reply_to.as_deref(), Some("original"));
        assert!(user.plain_text().is_empty());
        assert!(matches!(
            &user.parts[0],
            Part::Attachment { attachment, .. } if attachment.blob_ref.as_deref() == Some("blob-42")
        ));
    }

    #[test]
    fn mac_style_send_via_core_transact_is_one_commit() {
        let mac = ChatDoc::empty("chat1");
        let commits = Arc::new(AtomicUsize::new(0));
        let commits_cb = commits.clone();
        let _sub = mac.core.doc.subscribe_local_update(Box::new(move |_| {
            commits_cb.fetch_add(1, Ordering::SeqCst);
            true
        }));
        mac.core.transact(|d| {
            d.append_user_message("m1", "hello", "dev:mac", &UserMessageOpts::default())
                .unwrap();
            d.push_command("send", serde_json::json!({ "messageId": "m1" }), "dev:mac");
            d.push_command(
                "retitle",
                serde_json::json!({ "title": "hello" }),
                "dev:mac",
            );
        });
        assert_eq!(commits.load(Ordering::SeqCst), 1);
        assert_eq!(mac.len(), 1);
        assert_eq!(mac.core.doc.get_list("commands").len(), 2);
    }
}
