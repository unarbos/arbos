//! Search in the open chat ([feedback search-in-chat-mac-2026-10-26], Jacob
//! 2026-10-26: "bring back command F to find things in a chat by key word,
//! user name etc."). The pure part: what the bar's query means, which loaded
//! messages it hits, where the cursor goes on ‹ › / Enter / ⇧Enter, what the
//! count says, and which bytes of a rendered text to mark.
//!
//! A query is a keyword (folded like every other search in the app —
//! `arbos_core::search::fold`) and an optional pinned sender; a message hits
//! when it is from that sender AND its text contains the keyword (an empty
//! keyword with a pinned sender = all of that sender's messages). Hits keep
//! the transcript's order (1 = the oldest loaded) like Find; the cursor starts
//! on the newest hit, ‹ walks older, › newer, both wrap, and ‹ on the oldest
//! loaded hit asks for older rows when the chat may have some
//! ([`Step::PageOlder`]). While typing, a name or @handle that matches someone
//! in the chat is offered as a sender chip (the shared matcher of #1918,
//! `arbos_core::search::match_entity`); ⇥ or a click pins it.
//!
//! The view lives in `ui::chat_search`; the state in `AppState::chat_search`.

use std::ops::Range;

use arbos_core::search::{fold, match_entity, normalize_query, Match};

use crate::model::Id;

/// Someone who posted in the chat: the chip's name and handle, and the key
/// a message's sender is compared by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sender {
    /// Stable identity (`user:<id>`, `agent:<id>`, a handle) — never shown.
    pub key: String,
    pub name: String,
    /// Without the `@`; may be empty (an agent without a slug).
    pub handle: String,
    pub avatar_url: Option<String>,
}

/// One loaded message as the search sees it.
#[derive(Clone, Debug)]
pub struct Row {
    pub id: Id,
    /// Plain text, markdown stripped, whitespace collapsed (the palette index's form).
    pub text: String,
    pub sender: Sender,
}

/// The bar's query, resolved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// Folded, trimmed; empty = no keyword.
    pub keyword: String,
    pub sender: Option<Sender>,
}

impl Query {
    pub fn new(typed: &str, sender: Option<Sender>) -> Self {
        Self {
            keyword: keyword(typed),
            sender,
        }
    }

    /// Nothing to search for: no keyword and no pinned sender.
    pub fn is_empty(&self) -> bool {
        self.keyword.is_empty() && self.sender.is_none()
    }

    pub fn hits_row(&self, row: &Row) -> bool {
        if self.is_empty() {
            return false;
        }
        if let Some(s) = &self.sender {
            if row.sender.key != s.key {
                return false;
            }
        }
        self.keyword.is_empty() || fold(&row.text).contains(&self.keyword)
    }
}

/// The typed text as a keyword: folded and trimmed, inner whitespace kept
/// as one space. A leading `@` stays — as a keyword "@sam" is the literal
/// mention; the sender offer below drops it.
pub fn keyword(typed: &str) -> String {
    fold(typed).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The ids of the rows the query hits, in `rows` order (oldest first).
pub fn hits(rows: &[Row], query: &Query) -> Vec<Id> {
    rows.iter()
        .filter(|r| query.hits_row(r))
        .map(|r| r.id.clone())
        .collect()
}

/// Everyone who posted, in first-appearance order, one entry per key.
pub fn senders(rows: &[Row]) -> Vec<Sender> {
    let mut out: Vec<Sender> = Vec::new();
    for r in rows {
        if !out.iter().any(|s| s.key == r.sender.key) {
            out.push(r.sender.clone());
        }
    }
    out
}

/// The sender the typed text names, if any: best match by the shared
/// matcher's order (exact > prefix > word-start, name before handle). Only
/// while no sender is pinned yet and something is typed.
pub fn offer_sender<'a>(typed: &str, senders: &'a [Sender]) -> Option<&'a Sender> {
    let q = normalize_query(typed);
    if q.is_empty() {
        return None;
    }
    senders
        .iter()
        .filter_map(|s| match_entity(&s.name, &s.handle, None, &q).map(|m| (m, s)))
        .min_by_key(|(m, _): &(Match, &Sender)| m.order())
        .map(|(_, s)| s)
}

/// Where the cursor goes on a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// To this hit (wrapping included).
    To(usize),
    /// ‹ on the oldest loaded hit (or with no hits yet) and the chat may hold
    /// older rows: fetch a page, then continue.
    PageOlder,
    /// Nothing to step through.
    Nothing,
}

/// `delta` −1 = older (‹ / ⇧Enter / ⇧⌘G), +1 = newer (› / Enter / ⌘G).
pub fn step(current: Option<usize>, len: usize, delta: i32, may_have_older: bool) -> Step {
    if len == 0 {
        return if may_have_older && delta < 0 {
            Step::PageOlder
        } else {
            Step::Nothing
        };
    }
    let Some(cur) = current else {
        return Step::To(len - 1);
    };
    if delta < 0 {
        if cur == 0 {
            if may_have_older {
                Step::PageOlder
            } else {
                Step::To(len - 1)
            }
        } else {
            Step::To(cur - 1)
        }
    } else if cur + 1 >= len {
        Step::To(0)
    } else {
        Step::To(cur + 1)
    }
}

/// Where the cursor starts when the hits change: the newest hit (what a chat
/// scrolled to its end is closest to), or `None` without hits.
pub fn initial_cursor(len: usize) -> Option<usize> {
    len.checked_sub(1)
}

/// Keep the cursor on the same message after the hit list changed (a row
/// arrived, older rows paged in); the newest hit when it is gone.
pub fn keep_cursor(hits: &[Id], was_on: Option<&Id>) -> Option<usize> {
    was_on
        .and_then(|id| hits.iter().position(|h| h == id))
        .or_else(|| initial_cursor(hits.len()))
}

/// "3 of 7", "No matches", or "" while there is nothing to search for.
pub fn count_label(query: &Query, current: Option<usize>, len: usize) -> String {
    if query.is_empty() {
        return String::new();
    }
    match (current, len) {
        (_, 0) => "No matches".to_owned(),
        (Some(i), n) => format!("{} of {}", i + 1, n),
        (None, n) => format!("{n} matches"),
    }
}

/// Byte ranges of `text` whose folded form contains `needle` (folded,
/// non-empty). Folding can change a char's length (`ß` → `ss`, a stripped
/// accent), so the folded string is built char by char with a map back to
/// the original bytes; a range always starts and ends on a char boundary.
pub fn hit_ranges(text: &str, needle: &str) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut folded = String::with_capacity(text.len());
    // For every byte of `folded`, the original byte offset of the char it came from.
    let mut origin: Vec<usize> = Vec::with_capacity(text.len());
    for (at, c) in text.char_indices() {
        let before = folded.len();
        folded.push_str(&fold(&c.to_string()));
        origin.extend(std::iter::repeat_n(at, folded.len() - before));
    }
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut from = 0usize;
    while from < folded.len() {
        let Some(pos) = folded[from..].find(needle) else {
            break;
        };
        let start_f = from + pos;
        let end_f = start_f + needle.len();
        let start = origin[start_f];
        // The whole original char that produced the hit's last folded byte.
        let end = next_char(text, origin[end_f - 1]);
        match out.last_mut() {
            // A fold that grew (ß → ss) can make two hits share a char: one range.
            Some(last) if last.end > start => last.end = last.end.max(end),
            _ => out.push(start..end),
        }
        from = end_f;
    }
    out
}

fn next_char(text: &str, at: usize) -> usize {
    text[at..]
        .chars()
        .next()
        .map(|c| at + c.len_utf8())
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sender(key: &str, name: &str, handle: &str) -> Sender {
        Sender {
            key: key.into(),
            name: name.into(),
            handle: handle.into(),
            avatar_url: None,
        }
    }

    fn rows() -> Vec<Row> {
        let me = sender("user:me", "Jacob Steeves", "unarbos");
        let affine = sender("agent:a1", "Affine", "affine");
        let sam = sender("user:sam", "Sam Rivera", "sam");
        vec![
            Row {
                id: "m1".into(),
                text: "Summarise yesterday's design review.".into(),
                sender: me.clone(),
            },
            Row {
                id: "m2".into(),
                text: "Three decisions: keep the canvas infinite, move export to the share sheet."
                    .into(),
                sender: affine.clone(),
            },
            Row {
                id: "m3".into(),
                text: "What were the open questions?".into(),
                sender: me.clone(),
            },
            Row {
                id: "m4".into(),
                text:
                    "Whether the Canvas grid snaps, and whether export keeps the canvas background."
                        .into(),
                sender: affine.clone(),
            },
            Row {
                id: "m5".into(),
                text: "Snap at 8. Export keeps the background.".into(),
                sender: me,
            },
            Row {
                id: "m6".into(),
                text: "Noted — the cánvas spec is updated.".into(),
                sender: affine,
            },
            Row {
                id: "m7".into(),
                text: "Sending the deck in a sec.".into(),
                sender: sam,
            },
        ]
    }

    #[test]
    fn keyword_hits_are_case_and_accent_insensitive_and_count_messages_not_words() {
        let q = Query::new("Canvas", None);
        assert_eq!(hits(&rows(), &q), vec!["m2", "m4", "m6"]);
        // m4 holds the word twice and is one hit.
        assert_eq!(count_label(&q, initial_cursor(3), 3), "3 of 3");
    }

    #[test]
    fn a_pinned_sender_filters_and_ands_with_the_keyword() {
        let affine = senders(&rows())
            .into_iter()
            .find(|s| s.name == "Affine")
            .unwrap();
        let only_sender = Query::new("", Some(affine.clone()));
        assert_eq!(hits(&rows(), &only_sender), vec!["m2", "m4", "m6"]);
        let both = Query::new("export", Some(affine));
        assert_eq!(
            hits(&rows(), &both),
            vec!["m2", "m4"],
            "export from me (m5) is not a hit"
        );
        assert!(Query::new("  ", None).is_empty());
        assert!(hits(&rows(), &Query::default()).is_empty());
    }

    #[test]
    fn typing_a_name_or_handle_offers_the_sender_through_the_shared_matcher() {
        let all = senders(&rows());
        assert_eq!(all.len(), 3, "one entry per sender, first appearance order");
        assert_eq!(all[0].name, "Jacob Steeves");
        assert_eq!(
            offer_sender("aff", &all).map(|s| s.name.as_str()),
            Some("Affine")
        );
        assert_eq!(
            offer_sender("@sam", &all).map(|s| s.name.as_str()),
            Some("Sam Rivera")
        );
        assert_eq!(
            offer_sender("rivera", &all).map(|s| s.name.as_str()),
            Some("Sam Rivera"),
            "word start"
        );
        assert_eq!(
            offer_sender("Jacob S", &all).map(|s| s.handle.as_str()),
            Some("unarbos")
        );
        assert_eq!(offer_sender("canvas", &all), None);
        assert_eq!(offer_sender("", &all), None);
        // A whole name beats a handle prefix of someone else: "sam" is Sam's
        // handle exactly, not a prefix of anything on Jacob.
        assert_eq!(
            offer_sender("sam", &all).map(|s| s.name.as_str()),
            Some("Sam Rivera")
        );
    }

    #[test]
    fn next_and_prev_wrap_and_prev_on_the_oldest_pages_older_when_there_may_be_more() {
        // Four hits, cursor on the newest to start.
        assert_eq!(initial_cursor(4), Some(3));
        assert_eq!(
            step(Some(3), 4, 1, false),
            Step::To(0),
            "next on the newest wraps to the oldest"
        );
        assert_eq!(
            step(Some(0), 4, -1, false),
            Step::To(3),
            "prev on the oldest wraps to the newest"
        );
        assert_eq!(step(Some(2), 4, -1, false), Step::To(1));
        assert_eq!(step(Some(2), 4, 1, false), Step::To(3));
        // A channel with older rows on the relay: prev on the oldest pages instead of wrapping.
        assert_eq!(step(Some(0), 4, -1, true), Step::PageOlder);
        assert_eq!(
            step(None, 0, -1, true),
            Step::PageOlder,
            "no hits yet, older rows may hold one"
        );
        assert_eq!(step(None, 0, -1, false), Step::Nothing);
        assert_eq!(step(None, 0, 1, true), Step::Nothing, "next never pages");
        assert_eq!(
            step(None, 4, 1, false),
            Step::To(3),
            "no cursor yet: land on the newest"
        );
    }

    #[test]
    fn the_cursor_stays_on_its_message_when_older_rows_arrive() {
        let before: Vec<Id> = vec!["m2".into(), "m4".into()];
        let after: Vec<Id> = vec!["m0".into(), "m2".into(), "m4".into()];
        assert_eq!(
            keep_cursor(&after, before.first()),
            Some(1),
            "m2 moved down one"
        );
        assert_eq!(
            keep_cursor(&after, Some(&"gone".to_owned())),
            Some(2),
            "lost → the newest"
        );
        assert_eq!(keep_cursor(&[], None), None);
    }

    #[test]
    fn the_count_reads_like_find() {
        let q = Query::new("canvas", None);
        assert_eq!(count_label(&q, Some(1), 4), "2 of 4");
        assert_eq!(count_label(&q, None, 0), "No matches");
        assert_eq!(
            count_label(&Query::default(), None, 0),
            "",
            "nothing typed, nothing said"
        );
    }

    #[test]
    fn hit_ranges_map_folded_matches_back_to_the_original_bytes() {
        let text = "Keep the Canvas infinite; the cánvas too — and Straße.";
        let ranges = hit_ranges(text, "canvas");
        let words: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(words, vec!["Canvas", "cánvas"]);
        // A fold that grows (ß → ss) still ends on the right char.
        let ranges = hit_ranges(text, "strasse");
        assert_eq!(&text[ranges[0].clone()], "Straße");
        assert!(hit_ranges(text, "").is_empty());
        assert!(hit_ranges("", "x").is_empty());
        // Every range is on char boundaries.
        for r in hit_ranges("ééé canvas ééé", "canvas") {
            assert!(
                "ééé canvas ééé".is_char_boundary(r.start)
                    && "ééé canvas ééé".is_char_boundary(r.end)
            );
        }
    }
}
