//! One agent, one picture, one name — on every surface of every client.
//!
//! A people-chat row from an agent carries `sender: "agent:<project id>"`, a
//! `senderHandle` (`@slug`) and a `senderName`. The chat roster carries the
//! agent's project card (leaf picks, uploaded picture, display name). The
//! viewer's own project list carries the same card for agents they own. Before
//! this module each surface picked its own subset of those sources — the
//! iPhone bubble read only the viewer's projects and fell back to initials
//! seeded by the handle, while the working row read the roster and drew the
//! leaf; the old working line printed the raw handle (#1592). The rule below
//! is what the Mac, the iPhone (through `arbos-ffi`) and the relay roster all
//! run, so no surface guesses.
//!
//! Precedence, first hit wins: a card whose id is the sender's project id →
//! a card whose `@slug` is the row's handle → a card whose name slugs to that
//! handle → a card whose `@slug` is the slug of the row's name → no card: the
//! id seeds the automatic leaf (D92) and colour (D79) and the row's own name
//! is shown. Callers order the cards: the open chat's roster first (it is the
//! relay's current truth for that chat), then the viewer's projects, then
//! other chats' rosters.

use super::leaf::{self, LeafPick};

/// What a roster row or a project record knows about an agent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentCard {
    /// Project id (`01M5…`), no prefix.
    pub id: String,
    /// Display name as the owner typed it.
    pub name: String,
    /// `@slug`, empty when the source has none (derived from the name then).
    pub handle: String,
    /// D92 picks (`None` = automatic).
    pub leaf: Option<u8>,
    pub leaf_color: Option<u8>,
    /// D79 override (`None` = derived from the id).
    pub color_index: Option<u8>,
    /// Uploaded / generated picture: a blob key in the project's slot.
    pub image_key: Option<String>,
}

/// The row / event being drawn: who sent it, as the doc says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentQuery<'a> {
    /// `agent:<id>`, `project:<id>` or a bare id.
    pub sender: &'a str,
    /// The row's `senderHandle`, if any.
    pub handle: Option<&'a str>,
    /// The row's `senderName`, if any.
    pub name: Option<&'a str>,
}

/// The one answer every surface draws from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentIdentity {
    /// Bare project id; seeds the leaf and the colour.
    pub id: String,
    /// What a caption, a presence line or a chip shows. Never the bare handle while a
    /// name is known anywhere.
    pub name: String,
    /// `@slug` (without the `@`).
    pub handle: String,
    /// The leaf in effect (owner's picks or the automatic pair).
    pub leaf: LeafPick,
    /// D79 colour index when the leaf set is off.
    pub color_index: u8,
    /// The picture that wins over the leaf, when the agent has one.
    pub image_key: Option<String>,
    /// `true` when a card answered; `false` when only the row's own fields did.
    pub known: bool,
}

/// `agent:<id>` / `project:<id>` → `<id>`; a bare id passes through.
pub fn sender_id(sender: &str) -> &str {
    sender
        .strip_prefix("agent:")
        .or_else(|| sender.strip_prefix("project:"))
        .unwrap_or(sender)
}

/// The `@slug` an agent answers to: its name trimmed, lower-cased, runs of
/// whitespace to one hyphen. The relay (`people::agent_slug`), agentd and both
/// clients use this one.
pub fn agent_slug(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

fn same_handle(a: &str, b: &str) -> bool {
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

/// Pick the card for `query` from `cards` (ordered by the caller) and build the identity.
pub fn resolve<'c>(
    query: &AgentQuery<'_>,
    cards: impl IntoIterator<Item = &'c AgentCard>,
) -> AgentIdentity {
    let cards: Vec<&AgentCard> = cards.into_iter().collect();
    let id = sender_id(query.sender);
    let handle = query.handle.map(str::trim).filter(|h| !h.is_empty());
    let name = query.name.map(str::trim).filter(|n| !n.is_empty());
    let hit = cards
        .iter()
        .find(|c| !id.is_empty() && c.id == id)
        .or_else(|| {
            handle.and_then(|h| {
                cards
                    .iter()
                    .find(|c| same_handle(&c.handle, h))
                    .or_else(|| cards.iter().find(|c| same_handle(&agent_slug(&c.name), h)))
            })
        })
        .or_else(|| {
            name.and_then(|n| {
                let s = agent_slug(n);
                cards
                    .iter()
                    .find(|c| same_handle(&c.handle, &s) || same_handle(&agent_slug(&c.name), &s))
            })
        })
        .copied();
    match hit {
        Some(card) => {
            let card_handle = if card.handle.is_empty() {
                agent_slug(&card.name)
            } else {
                card.handle.clone()
            };
            AgentIdentity {
                id: card.id.clone(),
                name: display_name(Some(&card.name), handle),
                handle: card_handle,
                leaf: leaf::pick_for(&card.id, card.leaf, card.leaf_color),
                color_index: super::index_for(&card.id, card.color_index),
                image_key: card.image_key.clone().filter(|k| !k.is_empty()),
                known: true,
            }
        }
        None => {
            // No card: the bare id seeds the picture; a row without an id (an older box's
            // handle-only attribution) seeds it with the handle, else the name, so the same
            // row always draws the same leaf on every surface.
            let seed = if !id.is_empty() {
                id
            } else {
                handle.or(name).unwrap_or(query.sender)
            };
            AgentIdentity {
                id: seed.to_string(),
                name: display_name(name, handle),
                handle: handle
                    .map(str::to_string)
                    .or_else(|| name.map(agent_slug))
                    .unwrap_or_default(),
                leaf: leaf::default_pick(seed),
                color_index: super::color_index(seed),
                image_key: None,
                known: false,
            }
        }
    }
}

/// The words a surface shows for an agent: the display name when there is one; the
/// `@handle` only when nothing else is known; "Agent" when not even that.
pub fn display_name(name: Option<&str>, handle: Option<&str>) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => match handle.map(str::trim).filter(|h| !h.is_empty()) {
            Some(h) => format!("@{}", h.trim_start_matches('@')),
            None => "Agent".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, name: &str) -> AgentCard {
        AgentCard {
            id: id.into(),
            name: name.into(),
            handle: agent_slug(name),
            ..Default::default()
        }
    }

    #[test]
    fn sender_prefixes_are_stripped_and_bare_ids_pass() {
        assert_eq!(sender_id("agent:01M5A"), "01M5A");
        assert_eq!(sender_id("project:01M5A"), "01M5A");
        assert_eq!(sender_id("01M5A"), "01M5A");
        assert_eq!(sender_id(""), "");
    }

    #[test]
    fn slug_is_the_relay_rule() {
        assert_eq!(agent_slug("TestAgent"), "testagent");
        assert_eq!(agent_slug("  Rail  Layout Bot "), "rail-layout-bot");
        assert_eq!(agent_slug("GigChadAgent"), "gigchadagent");
    }

    /// The table: (sender, handle, name) against a roster and a project list, and what every
    /// surface must draw. The bubble and the presence row ask with the same row, so they get
    /// the same answer by construction.
    #[test]
    fn resolve_table() {
        let roster = [
            AgentCard {
                image_key: Some("avatar-1.jpg".into()),
                leaf: Some(3),
                leaf_color: Some(7),
                ..card("01TEST", "TestAgent")
            },
            card("01HOME", "Home"),
        ];
        let mine = [card("01MINE", "Mine"), card("01HOME", "Home (stale)")];
        let cards = || roster.iter().chain(mine.iter());

        // 1. Sender id wins, whatever the handle / name on the row say.
        let by_id = resolve(
            &AgentQuery {
                sender: "agent:01TEST",
                handle: Some("old-handle"),
                name: Some("Old Name"),
            },
            cards(),
        );
        assert_eq!(by_id.name, "TestAgent");
        assert_eq!(by_id.handle, "testagent");
        assert_eq!(by_id.leaf, LeafPick { leaf: 3, color: 7 });
        assert_eq!(by_id.image_key.as_deref(), Some("avatar-1.jpg"));
        assert!(by_id.known);

        // 2. The roster is asked before my projects (the relay's current truth for the chat).
        let home = resolve(
            &AgentQuery {
                sender: "agent:01HOME",
                handle: None,
                name: None,
            },
            cards(),
        );
        assert_eq!(home.name, "Home");

        // 3. A row without an id but with the handle lands on the same card.
        let by_handle = resolve(
            &AgentQuery {
                sender: "",
                handle: Some("TestAgent"),
                name: None,
            },
            cards(),
        );
        assert_eq!(by_handle.id, "01TEST");
        assert_eq!(by_handle.name, "TestAgent");

        // 4. A row with only a name slugs it and matches the card's handle.
        let by_name = resolve(
            &AgentQuery {
                sender: "",
                handle: None,
                name: Some(" testAgent "),
            },
            cards(),
        );
        assert_eq!(by_name.id, "01TEST");

        // 5. The bubble (keyed by `agent:<id>`) and the presence row (keyed by the handle)
        //    agree: one identity.
        let bubble = resolve(
            &AgentQuery {
                sender: "agent:01TEST",
                handle: Some("testagent"),
                name: Some("TestAgent"),
            },
            cards(),
        );
        let presence = resolve(
            &AgentQuery {
                sender: "testagent",
                handle: Some("testagent"),
                name: Some("TestAgent"),
            },
            cards(),
        );
        assert_eq!(bubble, presence);

        // 6. Unknown agent: the bare id seeds the picture (not `agent:<id>`), the row's name
        //    is shown, never the handle.
        let unknown = resolve(
            &AgentQuery {
                sender: "agent:01GHOST",
                handle: Some("ghost"),
                name: Some("Ghost Writer"),
            },
            cards(),
        );
        assert!(!unknown.known);
        assert_eq!(unknown.id, "01GHOST");
        assert_eq!(unknown.name, "Ghost Writer");
        assert_eq!(unknown.handle, "ghost");
        assert_eq!(unknown.leaf, leaf::default_pick("01GHOST"));
        assert_ne!(unknown.leaf, leaf::default_pick("agent:01GHOST"));
        assert_eq!(unknown.color_index, super::super::color_index("01GHOST"));
        assert_eq!(unknown.image_key, None);

        // 7. Unknown and nameless: the handle, marked as one; nothing at all: "Agent".
        let handle_only = resolve(
            &AgentQuery {
                sender: "agent:01GHOST",
                handle: Some("ghost"),
                name: None,
            },
            cards(),
        );
        assert_eq!(handle_only.name, "@ghost");
        let nothing = resolve(
            &AgentQuery {
                sender: "agent:01GHOST",
                handle: None,
                name: None,
            },
            cards(),
        );
        assert_eq!(nothing.name, "Agent");
        assert_eq!(nothing.handle, "");

        // 7b. No id at all (an older box's handle-only row): the handle seeds the picture, so
        //     the bubble and the row still draw one leaf.
        let handle_seeded = resolve(
            &AgentQuery {
                sender: "",
                handle: Some("ghost"),
                name: Some("Ghost Writer"),
            },
            cards(),
        );
        assert_eq!(handle_seeded.id, "ghost");
        assert_eq!(handle_seeded.leaf, leaf::default_pick("ghost"));

        // 8. A card without a handle gets one from its name; an empty image key is no picture.
        let no_handle = [AgentCard {
            handle: String::new(),
            image_key: Some(String::new()),
            ..card("01X", "Rail Layout Bot")
        }];
        let r = resolve(
            &AgentQuery {
                sender: "agent:01X",
                handle: None,
                name: None,
            },
            &no_handle,
        );
        assert_eq!(r.handle, "rail-layout-bot");
        assert_eq!(r.image_key, None);
    }

    #[test]
    fn display_name_never_shows_a_bare_handle_when_a_name_exists() {
        assert_eq!(
            display_name(Some("TestAgent"), Some("testagent")),
            "TestAgent"
        );
        assert_eq!(display_name(Some("  "), Some("testagent")), "@testagent");
        assert_eq!(display_name(None, Some("@testagent")), "@testagent");
        assert_eq!(display_name(None, None), "Agent");
    }
}
