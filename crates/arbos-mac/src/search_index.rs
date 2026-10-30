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

/// Only what the chat exchanged is searchable ([feedback
/// search-chat-only-mac-2026-10-30], the twin of the Library's rule — Jacob,
/// 2026-10-25): a hidden job row (duplex §2.3) or an unnamed worker's spawn row
/// is the agent's own machinery — the files it read, the pages it fetched — and
/// the transcript draws neither, so the palette must not offer them. Links the
/// agent typed in a **visible** reply stay indexed, unlike the Library's list:
/// they are in the chat, and the URL is part of the indexed prose anyway, so
/// dropping the link row would hide the direct result while leaving the same
/// page one section above.
///
/// Keep this the same predicate as `ui::profiles::shared_in_chat`; the two
/// should be folded into one helper once both land on main.
fn shared_in_chat(m: &Message) -> bool {
    !m.hidden && !m.worker
}

/// System lines never count, nor does anything the chat did not exchange; a
/// message with no prose, no attachment and no link is not indexed at all.
fn index_message(chat_id: &str, m: &Message, names: &dyn SenderName) -> Option<Indexed> {
    if m.role == Role::System || !shared_in_chat(m) {
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

    fn row(v: serde_json::Value) -> Message {
        serde_json::from_value(v).expect("message")
    }

    #[test]
    fn only_rows_the_chat_exchanged_are_shared() {
        let mut m = msg("m", 1, Role::Assistant, "hi");
        assert!(shared_in_chat(&m));
        m.worker = true;
        assert!(!shared_in_chat(&m), "an unnamed worker's row");
        m.worker = false;
        m.hidden = true;
        assert!(!shared_in_chat(&m), "a hidden job row");
    }

    /// The Library's rule on this surface ([feedback
    /// search-chat-only-mac-2026-10-30], the twin of #1819): a hidden job row
    /// and an unnamed worker's spawn row are the agent's own machinery — the
    /// files it read, the pages it fetched — and the transcript draws neither,
    /// so the palette must not find them. What the chat exchanged stays
    /// findable, the agent's visible prose links included (they are in the
    /// chat, and the same URL is already searchable as message text).
    #[test]
    fn the_agents_machinery_is_never_indexed_but_the_chat_stays_searchable() {
        let job = row(serde_json::json!({
            "id": "j", "chatId": "c", "role": "assistant", "sender": "agent:01PROJ",
            "status": "streaming", "createdAt": 1, "hidden": true, "worker": true,
            "job": { "id": "job1", "title": "Reading the repo" },
            "parts": [
                { "id": "j-t", "kind": "text", "text": "Read https://github.com/unarbos/arbos/blob/main/crates/arbos-mac/src/app.rs" },
                { "id": "j-a", "kind": "attachment", "attachment": { "name": "app.rs", "mime": "text/plain", "ref": "blob-1" } }
            ],
        }));
        let worker = row(serde_json::json!({
            "id": "w", "chatId": "c", "role": "assistant", "sender": "agent:01PROJ",
            "status": "done", "createdAt": 2, "worker": true, "subagentName": "reader",
            "turn": { "kind": "spawn" },
            "parts": [{ "id": "w-t", "kind": "text", "text": "Fetched https://docs.rs/gpui/layout" }],
        }));
        let reply = row(serde_json::json!({
            "id": "a", "chatId": "c", "role": "assistant", "sender": "agent:01PROJ",
            "status": "done", "createdAt": 3,
            "parts": [
                { "id": "a-t", "kind": "text", "text": "Nothing to change. Source: https://arbos.life/notes" },
                { "id": "a-a", "kind": "attachment", "attachment": { "name": "signups.png", "mime": "image/png", "ref": "blob-3", "kind": "image" } }
            ],
        }));
        let sent = row(serde_json::json!({
            "id": "u", "chatId": "c", "role": "user", "sender": "user:gh_2",
            "status": "done", "createdAt": 4,
            "parts": [
                { "id": "u-t", "kind": "text", "text": "Deck and the page: https://arbos.life/launch" },
                { "id": "u-a", "kind": "attachment", "attachment": { "name": "deck-v3.pdf", "mime": "application/pdf", "ref": "blob-2" } }
            ],
        }));

        let mut ix = SearchIndex::default();
        ix.replace_chat("c", &[job.clone(), worker, reply, sent], &Names);

        assert!(
            ix.attachments(&normalize_query("app.rs"), 10).is_empty(),
            "a file the agent read is not a file in the chat: {:?}",
            ix.attachments(&normalize_query("app.rs"), 10)
        );
        assert!(
            ix.links(&normalize_query("github.com"), 10).is_empty(),
            "a page a job row fetched is not a link the chat shared: {:?}",
            ix.links(&normalize_query("github.com"), 10)
        );
        assert!(
            ix.links(&normalize_query("docs.rs"), 10).is_empty(),
            "nor is a worker row's page: {:?}",
            ix.links(&normalize_query("docs.rs"), 10)
        );
        assert!(
            ix.messages(&normalize_query("Reading the repo"), 10)
                .is_empty()
                && ix.messages(&normalize_query("Fetched"), 10).is_empty(),
            "a row the transcript never draws cannot be jumped to, so it is not a Messages hit"
        );

        // What the chat exchanged is still there.
        assert_eq!(
            ix.attachments(&normalize_query("deck-v3"), 10).len(),
            1,
            "the file a person sent"
        );
        assert_eq!(
            ix.attachments(&normalize_query("signups"), 10).len(),
            1,
            "the picture the agent shared"
        );
        assert_eq!(
            ix.links(&normalize_query("arbos.life"), 10).len(),
            2,
            "the person's link and the agent's visible prose link: {:?}",
            ix.links(&normalize_query("arbos.life"), 10)
        );
        assert_eq!(
            ix.messages(&normalize_query("Nothing to change"), 10).len(),
            1,
            "the agent's visible reply"
        );

        // A row that turns hidden later leaves the index on its next upsert.
        let mut visible = job;
        visible.hidden = false;
        visible.worker = false;
        ix.upsert("c", &visible, &Names);
        assert_eq!(ix.attachments(&normalize_query("app.rs"), 10).len(), 1);
        ix.upsert(
            "c",
            &{
                let mut m = visible;
                m.hidden = true;
                m.worker = true;
                m
            },
            &Names,
        );
        assert!(ix.attachments(&normalize_query("app.rs"), 10).is_empty());
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
