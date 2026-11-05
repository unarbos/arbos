//! Search-in-chat for Swift ([`arbos_core::chat_search`]): the rules the Mac's
//! ⌘F bar has had since [feedback search-in-chat-mac-2026-10-26], so the iPhone
//! can filter a chat by who wrote a message without a second copy of them.
//!
//! Jacob, 2026-10-26: "bring back command F to find things in a chat by key
//! word, user name etc." The phone got the keyword half and not the name half.
//!
//! Only the sender half is exported here. Stepping, the count and the hit marks
//! belong to the phone's next release of this surface, and exporting them before
//! anything calls them would be dead bindings.
//!
//! What folds where, deliberately: a **sender** is matched as an entity, through
//! the same `match_entity` every picker, @mention completion and the home
//! search's name half on the phone already uses, so the chip cannot rank a name
//! differently from the rest of the app. A **message body** is not an entity and
//! keeps the phone's own folding (`HomeSearch.fold`, which is also
//! width-insensitive), so [`chat_search_row_hits`] takes the keyword's verdict
//! rather than re-deciding it here.

use arbos_core::chat_search;

/// Someone who posted in the chat: what the chip shows, and the key a message's
/// sender is compared by.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ChatSearchSender {
    /// Stable identity (`user:<id>`, `agent:<id>`, a handle) — never shown.
    pub key: String,
    pub name: String,
    /// Without the `@`; may be empty (an agent without a slug).
    pub handle: String,
    pub avatar_url: Option<String>,
}

impl From<ChatSearchSender> for chat_search::Sender {
    fn from(s: ChatSearchSender) -> Self {
        Self {
            key: s.key,
            name: s.name,
            handle: s.handle,
            avatar_url: s.avatar_url,
        }
    }
}

impl From<chat_search::Sender> for ChatSearchSender {
    fn from(s: chat_search::Sender) -> Self {
        Self {
            key: s.key,
            name: s.name,
            handle: s.handle,
            avatar_url: s.avatar_url,
        }
    }
}

/// Everyone who posted in the chat, one entry per `key`, in first-appearance
/// order: what a typed name is offered from. Pass one entry per loaded row.
#[uniffi::export]
pub fn chat_search_senders(senders: Vec<ChatSearchSender>) -> Vec<ChatSearchSender> {
    chat_search::first_by_key(senders.into_iter().map(Into::into))
        .into_iter()
        .map(Into::into)
        .collect()
}

/// The sender the typed text names, if any — best match by the shared entity
/// matcher (exact > prefix > word-start, display name before @handle). `None`
/// while nothing is typed, or when no one in the chat is named that.
///
/// With `pinned` set there is no offer at all, however the typed text reads:
/// the only way to change a pinned sender is to remove the chip. That is the
/// Mac's behaviour and the question of whether typing should swap it instead is
/// open, so it is asked in the shared rules and not answered here.
#[uniffi::export]
pub fn chat_search_offer(
    typed: String,
    senders: Vec<ChatSearchSender>,
    pinned: Option<ChatSearchSender>,
) -> Option<ChatSearchSender> {
    let pool: Vec<chat_search::Sender> = senders.into_iter().map(Into::into).collect();
    let pinned: Option<chat_search::Sender> = pinned.map(Into::into);
    chat_search::offer_sender(&typed, &pool, pinned.as_ref())
        .cloned()
        .map(Into::into)
}

/// Does one row hit, given whether its text holds the keyword?
///
/// The composition is the shared rule: a pinned sender ANDs with the keyword, a
/// pinned sender with an empty keyword takes every one of that sender's
/// messages, and an empty query hits nothing. `keyword_in_text` stays the
/// caller's because the phone folds message bodies its own way; `sender_key` is
/// compared as an identity and never folded.
#[uniffi::export]
pub fn chat_search_row_hits(
    keyword: String,
    sender_key: Option<String>,
    row_sender_key: String,
    keyword_in_text: bool,
) -> bool {
    chat_search::row_hits(&keyword, sender_key.as_deref(), &row_sender_key, || {
        keyword_in_text
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sender(key: &str, name: &str, handle: &str) -> ChatSearchSender {
        ChatSearchSender {
            key: key.into(),
            name: name.into(),
            handle: handle.into(),
            avatar_url: None,
        }
    }

    fn posted() -> Vec<ChatSearchSender> {
        vec![
            sender("user:me", "Jacob Steeves", "unarbos"),
            sender("agent:a1", "Affine", "affine"),
            sender("user:me", "Jacob Steeves", "unarbos"),
            sender("user:sam", "Sam Rivera", "sam"),
            sender("agent:a1", "Affine", "affine"),
        ]
    }

    /// Everyone who posted, one entry per key, in first-appearance order — the
    /// order the chip list is offered in.
    #[test]
    fn senders_are_deduplicated_by_key_in_first_appearance_order() {
        let who = chat_search_senders(posted());
        assert_eq!(
            who.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(),
            ["user:me", "agent:a1", "user:sam"]
        );
        assert_eq!(who[2].name, "Sam Rivera");
        assert!(chat_search_senders(vec![]).is_empty());
    }

    /// The offer comes out of the shared entity matcher, so the chip ranks a
    /// typed name exactly as every picker on the phone does.
    #[test]
    fn a_typed_name_or_handle_offers_one_sender_through_the_shared_matcher() {
        let who = chat_search_senders(posted());
        let offered =
            |typed: &str| chat_search_offer(typed.to_owned(), who.clone(), None).map(|s| s.name);
        assert_eq!(offered("aff").as_deref(), Some("Affine"));
        assert_eq!(offered("@sam").as_deref(), Some("Sam Rivera"));
        assert_eq!(
            offered("rivera").as_deref(),
            Some("Sam Rivera"),
            "word start"
        );
        assert_eq!(offered("Jacob S").as_deref(), Some("Jacob Steeves"));
        // Diacritics fold on a name the way they do everywhere else.
        let zoe = vec![sender("user:z", "Zoë Álvarez", "zoe")];
        assert_eq!(
            chat_search_offer("zoe alv".to_owned(), zoe, None).map(|s| s.key),
            Some("user:z".to_owned())
        );
        // A word nobody is named is not an offer, and neither is nothing typed.
        assert_eq!(offered("canvas"), None);
        assert_eq!(offered(""), None);
        assert_eq!(offered("   "), None);
        // Nothing is offered once a sender is pinned, so the chip cannot be
        // swapped by typing — the Mac's behaviour, asked in the shared rules.
        assert_eq!(
            chat_search_offer("aff".to_owned(), who.clone(), Some(who[2].clone())),
            None
        );
    }

    /// The composition, reached through the FFI: AND with a keyword, every row
    /// of a pinned sender without one, nothing at all when neither is set.
    #[test]
    fn a_rows_verdict_is_the_senders_and_the_keywords_together() {
        let hits = |keyword: &str, pinned: Option<&str>, row: &str, in_text: bool| {
            chat_search_row_hits(
                keyword.to_owned(),
                pinned.map(str::to_owned),
                row.to_owned(),
                in_text,
            )
        };
        assert!(!hits("", None, "user:sam", true), "nothing to search for");
        assert!(hits("canvas", None, "user:sam", true));
        assert!(!hits("canvas", None, "user:sam", false));
        assert!(
            hits("", Some("user:sam"), "user:sam", false),
            "a pinned sender with no keyword takes all of their messages"
        );
        assert!(!hits("", Some("user:sam"), "user:me", true));
        assert!(hits("canvas", Some("user:sam"), "user:sam", true));
        assert!(!hits("canvas", Some("user:sam"), "user:sam", false));
        assert!(!hits("canvas", Some("user:sam"), "user:me", true));
        // By key, never by what the chip shows.
        assert!(!hits("", Some("user:sam"), "Sam Rivera", true));
    }

    /// The records cross the boundary without losing a field.
    #[test]
    fn a_sender_survives_the_round_trip() {
        let mut s = sender("user:sam", "Sam Rivera", "sam");
        s.avatar_url = Some("https://example.invalid/a.png".into());
        let core: chat_search::Sender = s.clone().into();
        assert_eq!(ChatSearchSender::from(core), s);
    }
}
