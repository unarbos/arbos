//! The command palette's local index (feedback `search-palette-app-wide-2026-10-24`,
//! Jacob: "make this search everything app wide"). Every chat doc that is on
//! this Mac — agent chats, DMs, groups, server channels — is indexed as it
//! arrives and re-indexed on every change (`SyncEvent::ChatSnapshot`,
//! `MessageUpsert`, a channel timeline), so a keystroke in the palette scans
//! memory and never asks the relay. Per message: the prose, the attachments
//! (name + title) and the `http(s)` links, each with its folded form
//! (`crate::search::fold`) computed once, on the first query that needs it.
//!
//! Queries return the newest hits first with a snippet around the match; the
//! palette turns them into rows that jump to the message in its chat.

use std::cell::OnceCell;
use std::collections::HashMap;

use crate::model::{AttachmentKind, Id, Message, Part, Role};
use crate::search::{fold, snippet};

/// One message as the index keeps it.
#[derive(Debug)]
struct Indexed {
    id: Id,
    created_at: u64,
    /// Markdown markers stripped, whitespace collapsed to single spaces.
    text: String,
    folded: OnceCell<String>,
    /// "Mira", "@sam", "You" — whoever the palette should name.
    sender: String,
    attachments: Vec<IndexedAttachment>,
    links: Vec<IndexedLink>,
}

#[derive(Debug)]
struct IndexedAttachment {
    part_id: Id,
    name: String,
    title: String,
    folded: OnceCell<String>,
    kind: AttachmentKind,
    size_label: String,
}

#[derive(Debug)]
struct IndexedLink {
    url: String,
    folded: OnceCell<String>,
}

impl Indexed {
    fn folded(&self) -> &str {
        self.folded.get_or_init(|| fold(&self.text))
    }
}

impl IndexedAttachment {
    fn folded(&self) -> &str {
        self.folded
            .get_or_init(|| fold(&format!("{} {}", self.name, self.title)))
    }
}

impl IndexedLink {
    fn folded(&self) -> &str {
        self.folded.get_or_init(|| fold(&self.url))
    }
}

/// A message whose prose contains the query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageHit {
    pub chat_id: Id,
    pub message_id: Id,
    pub created_at: u64,
    pub sender: String,
    pub snippet: String,
}

/// An attachment whose name or title contains the query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentHit {
    pub chat_id: Id,
    pub message_id: Id,
    pub part_id: Id,
    pub created_at: u64,
    pub sender: String,
    pub name: String,
    pub kind: AttachmentKind,
    pub size_label: String,
}

/// A link whose URL contains the query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkHit {
    pub chat_id: Id,
    pub message_id: Id,
    pub created_at: u64,
    pub sender: String,
    pub url: String,
}

/// Who a message is from, as the palette names it. The owner passes the
/// resolver so the index stays free of `AppState`.
pub trait SenderName {
    fn sender_name(&self, chat_id: &str, message: &Message) -> String;
}

#[derive(Debug, Default)]
pub struct SearchIndex {
    chats: HashMap<Id, Vec<Indexed>>,
}

impl SearchIndex {
    /// Replace everything known about `chat_id` (a snapshot, a channel page).
    pub fn replace_chat(&mut self, chat_id: &str, messages: &[Message], names: &dyn SenderName) {
        let rows: Vec<Indexed> = messages
            .iter()
            .filter_map(|m| index_message(chat_id, m, names))
            .collect();
        if rows.is_empty() {
            self.chats.remove(chat_id);
        } else {
            self.chats.insert(chat_id.to_string(), rows);
        }
    }

    /// One message arrived or changed (streaming deltas included).
    pub fn upsert(&mut self, chat_id: &str, message: &Message, names: &dyn SenderName) {
        let row = index_message(chat_id, message, names);
        let list = self.chats.entry(chat_id.to_string()).or_default();
        match (list.iter().position(|r| r.id == message.id), row) {
            (Some(i), Some(row)) => list[i] = row,
            (Some(i), None) => {
                list.remove(i);
            }
            (None, Some(row)) => list.push(row),
            (None, None) => {}
        }
    }

    pub fn remove_chat(&mut self, chat_id: &str) {
        self.chats.remove(chat_id);
    }

    /// How many chats and messages the index holds.
    #[cfg(test)]
    pub fn size(&self) -> (usize, usize) {
        (self.chats.len(), self.chats.values().map(Vec::len).sum())
    }

    /// Messages whose prose contains `q` (already `normalize_query`'d,
    /// non-empty), newest first, at most `limit`.
    pub fn messages(&self, q: &str, limit: usize) -> Vec<MessageHit> {
        if q.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<MessageHit> = Vec::new();
        for (chat_id, rows) in &self.chats {
            for r in rows {
                if !r.folded().contains(q) {
                    continue;
                }
                let Some(snippet) = snippet(&r.text, q) else {
                    continue;
                };
                hits.push(MessageHit {
                    chat_id: chat_id.clone(),
                    message_id: r.id.clone(),
                    created_at: r.created_at,
                    sender: r.sender.clone(),
                    snippet,
                });
            }
        }
        hits.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.chat_id.cmp(&b.chat_id))
                .then_with(|| a.message_id.cmp(&b.message_id))
        });
        hits.truncate(limit);
        hits
    }

    /// Attachments whose name or title contains `q`, newest first.
    pub fn attachments(&self, q: &str, limit: usize) -> Vec<AttachmentHit> {
        if q.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<AttachmentHit> = Vec::new();
        for (chat_id, rows) in &self.chats {
            for r in rows {
                for a in &r.attachments {
                    if !a.folded().contains(q) {
                        continue;
                    }
                    hits.push(AttachmentHit {
                        chat_id: chat_id.clone(),
                        message_id: r.id.clone(),
                        part_id: a.part_id.clone(),
                        created_at: r.created_at,
                        sender: r.sender.clone(),
                        name: if a.title.is_empty() {
                            a.name.clone()
                        } else {
                            a.title.clone()
                        },
                        kind: a.kind,
                        size_label: a.size_label.clone(),
                    });
                }
            }
        }
        hits.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.part_id.cmp(&b.part_id))
        });
        hits.truncate(limit);
        hits
    }

    /// Links whose URL contains `q`, newest first, one row per URL.
    pub fn links(&self, q: &str, limit: usize) -> Vec<LinkHit> {
        if q.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<LinkHit> = Vec::new();
        for (chat_id, rows) in &self.chats {
            for r in rows {
                for l in &r.links {
                    if !l.folded().contains(q) {
                        continue;
                    }
                    hits.push(LinkHit {
                        chat_id: chat_id.clone(),
                        message_id: r.id.clone(),
                        created_at: r.created_at,
                        sender: r.sender.clone(),
                        url: l.url.clone(),
                    });
                }
            }
        }
        hits.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.url.cmp(&b.url))
        });
        hits.dedup_by(|a, b| a.url == b.url);
        hits.truncate(limit);
        hits
    }
}

/// System lines never count; a message with no prose, no attachment and no
/// link is not indexed at all.
fn index_message(chat_id: &str, m: &Message, names: &dyn SenderName) -> Option<Indexed> {
    if m.role == Role::System {
        return None;
    }
    let text = crate::ui::sidebar::plain(&m.plain_text());
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut attachments = Vec::new();
    let mut links = Vec::new();
    for part in &m.parts {
        match part {
            Part::Attachment { id, attachment } => attachments.push(IndexedAttachment {
                part_id: id.clone(),
                name: attachment.name.clone(),
                title: attachment.title().to_string(),
                folded: OnceCell::new(),
                kind: attachment.kind(),
                size_label: attachment.size_label(),
            }),
            Part::Text { text, .. } => {
                for url in crate::ui::profiles::urls_in(text) {
                    links.push(IndexedLink {
                        url,
                        folded: OnceCell::new(),
                    });
                }
            }
            _ => {}
        }
    }
    if text.is_empty() && attachments.is_empty() && links.is_empty() {
        return None;
    }
    Some(Indexed {
        id: m.id.clone(),
        created_at: m.created_at,
        text,
        folded: OnceCell::new(),
        sender: names.sender_name(chat_id, m),
        attachments,
        links,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Attachment;
    use crate::search::normalize_query;

    struct Names;

    impl SenderName for Names {
        fn sender_name(&self, _: &str, m: &Message) -> String {
            match m.role {
                Role::User => "You".into(),
                _ => "Agent".into(),
            }
        }
    }

    fn msg(id: &str, at: u64, role: Role, text: &str) -> Message {
        let role = match role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            Role::Project => "project",
        };
        serde_json::from_value(serde_json::json!({
            "id": id,
            "chatId": "chat",
            "role": role,
            "sender": "user:me",
            "status": "done",
            "createdAt": at,
            "parts": [{ "id": format!("{id}-t"), "kind": "text", "text": text }],
        }))
        .expect("message")
    }

    fn with_file(mut m: Message, part: &str, name: &str, title: &str) -> Message {
        m.parts.push(Part::Attachment {
            id: part.into(),
            attachment: Attachment {
                name: name.into(),
                mime: "application/pdf".into(),
                blob_ref: None,
                bytes: Some(2048),
                kind: None,
                title: Some(title.into()),
                chart_part: None,
                url: None,
                source: None,
            },
        });
        m
    }

    fn index() -> SearchIndex {
        let mut ix = SearchIndex::default();
        ix.replace_chat(
            "chat-kenobi",
            &[
                msg("k1", 10, Role::User, "Hi Kenobi, how is the gate?"),
                msg("k2", 20, Role::Assistant, "Green since noon."),
            ],
            &Names,
        );
        ix.replace_chat(
            "chat-arbonites",
            &[
                msg(
                    "a1",
                    30,
                    Role::User,
                    "The **zebra** budget is in https://docs.example.com/zebra.",
                ),
                with_file(
                    msg("a2", 40, Role::Assistant, "Here is the deck."),
                    "p-deck",
                    "q3-review.pdf",
                    "Zebra review deck",
                ),
                msg("a3", 50, Role::System, "zebra joined"),
            ],
            &Names,
        );
        ix
    }

    /// Jacob's check: from inside the Kenobi DM, a word that exists only in
    /// another chat's message is found, newest first, with a snippet.
    #[test]
    fn a_word_from_another_chat_is_found() {
        let ix = index();
        let hits = ix.messages(&normalize_query("Zebra"), 10);
        assert_eq!(hits.len(), 1, "the system line never counts: {hits:?}");
        assert_eq!(hits[0].chat_id, "chat-arbonites");
        assert_eq!(hits[0].message_id, "a1");
        assert!(
            hits[0].snippet.contains("zebra budget"),
            "{}",
            hits[0].snippet
        );
        assert_eq!(hits[0].sender, "You");
        assert!(ix.messages("nothing-here", 10).is_empty());
        assert!(ix.messages("", 10).is_empty(), "no query, no message rows");
    }

    #[test]
    fn attachments_and_links_are_found_by_name_title_or_url() {
        let ix = index();
        let files = ix.attachments(&normalize_query("deck"), 10);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].part_id, "p-deck");
        assert_eq!(files[0].name, "Zebra review deck");
        assert_eq!(files[0].size_label, "2 KB");
        assert_eq!(
            ix.attachments("review.pdf", 10).len(),
            1,
            "the file name too"
        );
        let links = ix.links(&normalize_query("docs.example"), 10);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "https://docs.example.com/zebra");
    }

    /// Newest first across chats; the limit cuts the tail.
    #[test]
    fn hits_come_newest_first_across_chats() {
        let ix = index();
        let hits = ix.messages("e", 10);
        let order: Vec<&str> = hits.iter().map(|h| h.message_id.as_str()).collect();
        assert_eq!(order, ["a2", "a1", "k2", "k1"]);
        assert_eq!(ix.messages("e", 2).len(), 2);
    }

    /// A snapshot replaces the chat; an upsert changes one message in place;
    /// a message that lost its prose leaves the index.
    #[test]
    fn the_index_follows_doc_changes() {
        let mut ix = index();
        assert_eq!(ix.size(), (2, 4));
        ix.upsert("chat-kenobi", &msg("k3", 60, Role::User, "zebra!"), &Names);
        assert_eq!(ix.messages("zebra", 10).len(), 2);
        ix.upsert(
            "chat-kenobi",
            &msg("k3", 61, Role::User, "never mind"),
            &Names,
        );
        assert_eq!(ix.messages("zebra", 10).len(), 1, "edited away");
        ix.upsert("chat-kenobi", &msg("k1", 10, Role::User, ""), &Names);
        assert_eq!(ix.size(), (2, 5 - 1));
        ix.replace_chat("chat-kenobi", &[], &Names);
        assert_eq!(ix.size(), (1, 2));
        ix.remove_chat("chat-arbonites");
        assert_eq!(ix.size(), (0, 0));
    }

    /// Case and diacritics fold the way the sidebar search does.
    #[test]
    fn matching_folds_case_and_diacritics() {
        let mut ix = SearchIndex::default();
        ix.replace_chat(
            "c",
            &[msg("m", 1, Role::User, "Zoë sent the résumé")],
            &Names,
        );
        assert_eq!(ix.messages(&normalize_query("ZOE"), 5).len(), 1);
        assert_eq!(ix.messages(&normalize_query("resume"), 5).len(), 1);
    }
}
