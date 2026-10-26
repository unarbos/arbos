//! The command palette's result model (feedback `search-palette-2026-10-24`,
//! Jacob): ⌘⇧F / ⌘K / the sidebar's Search disc open one centred palette over
//! the window — Grok Bot's — that finds everything: agents, people (DMs),
//! groups, servers and their channels, and the files, pictures and links
//! shared in chats. The ranked name/handle matching of #1477
//! ([`crate::search`]) stays the matcher for chat rows; this module decides
//! what the rows are, how they group and in what order they come.
//!
//! Sections, top to bottom: **People & agents** (a DM, a person met in a group
//! or a shared project, an agent — found by name / handle), **Chats** (groups;
//! a member, a chat title), **Servers & channels**, **Messages** (message text
//! anywhere on this Mac, from the `crate::search_index`; a row jumps to the
//! message in its chat), **Files, media & links**. Inside a section: a name
//! that the shared matcher ranks best (exact, then a name that starts with
//! the query, then one where a word starts with it — `arbos_core::search`),
//! then indirect hits, newest activity first. With no query the palette shows **Recent** — every
//! chat and server by activity — and the first nine rows carry ⌘1–⌘9, like
//! Grok's. App-wide whatever chat is open ([feedback
//! search-palette-app-wide-2026-10-24]).

use crate::model::{Id, Project};
use crate::search::{HitKind, Match, SearchHit};

/// What a row is; decides its glyph and its section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Agent,
    Person,
    Group,
    Server,
    Channel,
    /// A message whose text matched (any chat or channel).
    Message,
    Media,
    File,
    Link,
}

/// The group a row is listed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Recent,
    People,
    Chats,
    Servers,
    Messages,
    Content,
}

impl Section {
    pub const WITH_QUERY: [Section; 5] = [
        Section::People,
        Section::Chats,
        Section::Servers,
        Section::Messages,
        Section::Content,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Recent => "Recent",
            Section::People => "People & agents",
            Section::Chats => "Chats",
            Section::Servers => "Servers & channels",
            Section::Messages => "Messages",
            Section::Content => "Files, media & links",
        }
    }
}

/// How well a row's own name matched: decides the order inside a section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    /// The name, @handle or title matched through the shared matcher
    /// (`arbos_core::search`): exact > prefix > word-start, name before handle.
    Direct(Match),
    /// A message body, file name or link that contains the query.
    Contains,
    /// Something other than the name matched: a member, a chat title.
    Indirect,
}

impl Rank {
    /// Every row's rank on an empty query (nothing to order by but activity).
    pub const EVERYTHING: Rank = Rank::Direct(Match::EVERYTHING);

    /// Rank a name against the normalized query.
    pub fn of(name: &str, q: &str) -> Rank {
        Rank::of_entity(name, "", None, q)
    }

    /// Rank an entity (display name, @handle, title) against the normalized query.
    pub fn of_entity(name: &str, handle: &str, title: Option<&str>, q: &str) -> Rank {
        crate::search::match_entity(name, handle, title, q)
            .map(Rank::Direct)
            .unwrap_or(Rank::Indirect)
    }
}

/// What opening a row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Open the chat (agent, DM, group).
    Space(Id),
    /// Open the chat and scroll to the message that matched.
    Message {
        space_id: Id,
        chat_id: Id,
        message_id: Id,
    },
    /// Enter the server (its `#general`).
    Server(Id),
    /// Open one channel of a server.
    Channel { server_id: Id, channel_id: Id },
    /// Open one channel scrolled to a message.
    ChannelMessage {
        server_id: Id,
        channel_id: Id,
        message_id: Id,
    },
    /// Start (or open) the DM with a person met in a group or a shared project.
    Person(String),
    /// Open a picture / video / file in the media viewer.
    Media {
        chat_id: Id,
        message_id: Id,
        part_id: Id,
    },
    /// Open a link in the browser.
    Link(String),
}

/// One palette row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    pub title: String,
    pub subtitle: String,
    /// Newest activity (ms); orders rows inside a section.
    pub when: u64,
    /// How the row matched: by its name (prefix / contains) or indirectly (a
    /// member, a chat title). Name hits come first in a section.
    pub rank: Rank,
    pub target: Target,
}

impl Entry {
    /// Which section this row lists under when a query is typed.
    pub fn section(&self) -> Section {
        match (self.kind, self.rank) {
            (Kind::Agent | Kind::Person, Rank::Indirect) => Section::Chats,
            (Kind::Agent | Kind::Person, _) => Section::People,
            (Kind::Group, _) => Section::Chats,
            (Kind::Server | Kind::Channel, _) => Section::Servers,
            (Kind::Message, _) => Section::Messages,
            (Kind::Media | Kind::File | Kind::Link, _) => Section::Content,
        }
    }
}

/// A chat row from the #1477 matcher's verdict: a DM reads as the person
/// (display name over @handle), a group as "Group · N members", an agent as
/// its description; a hit beyond the row's own name keeps the matcher's
/// reason as the subtitle and counts as indirect (listed under Chats).
pub fn space_entry(space: &Project, hit: SearchHit, when: u64, query: &str) -> Entry {
    let kind = if space.is_people_group() {
        Kind::Group
    } else if space.is_people() {
        Kind::Person
    } else {
        Kind::Agent
    };
    let person = space.person().filter(|_| kind == Kind::Person);
    let title = match person {
        Some(p) => p.display_name(),
        None => space.name.clone(),
    };
    let own_line = match (kind, person) {
        (Kind::Person, Some(p)) => format!("@{}", p.handle),
        (Kind::Group, _) => group_subtitle(space.people.len()),
        _ => space
            .description
            .clone()
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| "Agent".into()),
    };
    let rank = if query.is_empty() {
        Rank::EVERYTHING
    } else if matches!(hit.kind, HitKind::Member | HitKind::Message)
        || hit
            .reason
            .as_deref()
            .is_some_and(|r| r.starts_with("Chat · "))
    {
        Rank::Indirect
    } else {
        // The matcher's own verdict on the name / @handle / title.
        Rank::Direct(hit.matched)
    };
    let target = match &hit.message {
        Some((chat_id, message_id)) => Target::Message {
            space_id: space.id.clone(),
            chat_id: chat_id.clone(),
            message_id: message_id.clone(),
        },
        None => Target::Space(space.id.clone()),
    };
    Entry {
        kind,
        title,
        subtitle: hit.reason.unwrap_or(own_line),
        when,
        rank,
        target,
    }
}

/// Rows per section with a query (the list scrolls, but a long tail of
/// message hits would bury the servers and files).
pub const SECTION_CAP: usize = 8;
/// Rows on the empty query (recent chats and servers by activity).
pub const RECENT_CAP: usize = 24;
/// How many rows carry a ⌘1–⌘9 shortcut.
pub const SHORTCUTS: usize = 9;

/// Group and order `entries` for the list. Empty `query`: one **Recent**
/// section by activity. Otherwise the four sections in order, each with its
/// direct (name) hits first, newest first, capped at [`SECTION_CAP`].
pub fn arrange(mut entries: Vec<Entry>, query_is_empty: bool) -> Vec<(Section, Vec<Entry>)> {
    if query_is_empty {
        entries.retain(|e| {
            matches!(
                e.kind,
                Kind::Agent | Kind::Person | Kind::Group | Kind::Server
            ) && !matches!(e.target, Target::Person(_))
        });
        entries.sort_by_key(|e| std::cmp::Reverse(e.when));
        entries.truncate(RECENT_CAP);
        return if entries.is_empty() {
            Vec::new()
        } else {
            vec![(Section::Recent, entries)]
        };
    }
    let mut out: Vec<(Section, Vec<Entry>)> = Vec::new();
    for section in Section::WITH_QUERY {
        let mut rows: Vec<Entry> = entries
            .iter()
            .filter(|e| e.section() == section)
            .cloned()
            .collect();
        if rows.is_empty() {
            continue;
        }
        rows.sort_by_key(|e| (e.rank, std::cmp::Reverse(e.when)));
        rows.truncate(SECTION_CAP);
        out.push((section, rows));
    }
    out
}

/// The rows in list order, flattened across sections (what ↑↓ and ⌘1–9 walk).
pub fn flatten(arranged: &[(Section, Vec<Entry>)]) -> Vec<&Entry> {
    arranged.iter().flat_map(|(_, rows)| rows.iter()).collect()
}

/// "⌘1" … "⌘9" for the first nine rows; `None` after that.
pub fn shortcut_label(index: usize) -> Option<String> {
    (index < SHORTCUTS).then(|| format!("⌘{}", index + 1))
}

/// Keyboard state of the open palette: which row is highlighted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub selected: usize,
}

impl Cursor {
    /// ↑ / ↓ by `delta` over `len` rows: clamps at both ends (Grok's palette
    /// does not wrap).
    pub fn step(&mut self, delta: i32, len: usize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        let next = self.selected as i64 + delta as i64;
        self.selected = next.clamp(0, len as i64 - 1) as usize;
    }

    /// The list changed (typing): keep the highlight inside it. An empty
    /// list leaves it alone — the rows may simply not have loaded yet (the
    /// fixture's first frame), and a preset highlight must survive that.
    pub fn clamp(&mut self, len: usize) {
        if len > 0 && self.selected >= len {
            self.selected = len - 1;
        }
    }
}

/// "Server · 3 channels · 12 members".
pub fn server_subtitle(channels: usize, members: u64) -> String {
    let mut s = format!(
        "Server · {channels} channel{}",
        if channels == 1 { "" } else { "s" }
    );
    if members > 0 {
        s.push_str(&format!(
            " · {members} member{}",
            if members == 1 { "" } else { "s" }
        ));
    }
    s
}

/// "Group · 4 members".
pub fn group_subtitle(members: usize) -> String {
    format!(
        "Group · {members} member{}",
        if members == 1 { "" } else { "s" }
    )
}

/// The host of a link for its subtitle ("github.com"), the URL itself when
/// it has none.
pub fn link_host(url: &str) -> String {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.strip_prefix("www.").unwrap_or(host);
    if host.is_empty() {
        url.to_string()
    } else {
        host.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: Kind, title: &str, when: u64, indirect: bool) -> Entry {
        Entry {
            kind,
            title: title.into(),
            subtitle: String::new(),
            when,
            rank: if indirect {
                Rank::Indirect
            } else {
                Rank::Contains
            },
            target: Target::Space(format!("id-{title}")),
        }
    }

    fn fixture() -> Vec<Entry> {
        vec![
            entry(Kind::Agent, "Arbos", 50, false),
            entry(Kind::Person, "Carro", 40, false),
            entry(Kind::Group, "Testing", 60, false),
            // A DM found through a message body: a Chats row, not a person row.
            entry(Kind::Person, "Sam", 70, true),
            entry(Kind::Server, "Launch Crew", 30, false),
            Entry {
                target: Target::Channel {
                    server_id: "s1".into(),
                    channel_id: "c1".into(),
                },
                ..entry(Kind::Channel, "#general", 80, false)
            },
            Entry {
                target: Target::Media {
                    chat_id: "ch".into(),
                    message_id: "m".into(),
                    part_id: "p".into(),
                },
                ..entry(Kind::File, "plan.pdf", 90, false)
            },
            Entry {
                target: Target::Link("https://www.github.com/unarbos/arbos".into()),
                ..entry(Kind::Link, "github.com/unarbos/arbos", 20, false)
            },
            // A message-text hit from a chat that is not open: its own section.
            Entry {
                target: Target::Message {
                    space_id: "g".into(),
                    chat_id: "c".into(),
                    message_id: "m9".into(),
                },
                ..entry(Kind::Message, "Arbonites", 85, false)
            },
            // A person met in a group, no DM yet: People, opens a DM.
            Entry {
                target: Target::Person("dawizzaa".into()),
                ..entry(Kind::Person, "@DaWizzaa", 15, false)
            },
        ]
    }

    /// People/agents → chats → servers/channels → files/media/links, each
    /// section newest first; a server *and* its channel both appear.
    #[test]
    fn sections_come_in_jacobs_order_with_a_channel_and_a_file() {
        let arranged = arrange(fixture(), false);
        let sections: Vec<Section> = arranged.iter().map(|(s, _)| *s).collect();
        assert_eq!(
            sections,
            vec![
                Section::People,
                Section::Chats,
                Section::Servers,
                Section::Messages,
                Section::Content
            ]
        );
        let titles = |s: Section| -> Vec<String> {
            arranged
                .iter()
                .find(|(x, _)| *x == s)
                .map(|(_, rows)| rows.iter().map(|r| r.title.clone()).collect())
                .unwrap_or_default()
        };
        assert_eq!(titles(Section::People), ["Arbos", "Carro", "@DaWizzaa"]);
        assert_eq!(titles(Section::Chats), ["Testing", "Sam"]);
        assert_eq!(titles(Section::Messages), ["Arbonites"]);
        assert_eq!(titles(Section::Servers), ["#general", "Launch Crew"]);
        assert_eq!(
            titles(Section::Content),
            ["plan.pdf", "github.com/unarbos/arbos"]
        );
        assert!(flatten(&arranged)
            .iter()
            .any(|e| matches!(e.target, Target::Channel { .. })));
        assert!(flatten(&arranged)
            .iter()
            .any(|e| matches!(e.target, Target::Media { .. })));
    }

    /// Empty query: recent chats and servers by activity — no files, links,
    /// messages or DM-less people —, ⌘1–⌘9 on the first nine rows and nothing after.
    #[test]
    fn the_empty_query_shows_recents_with_shortcuts() {
        let arranged = arrange(fixture(), true);
        assert_eq!(arranged.len(), 1);
        assert_eq!(arranged[0].0, Section::Recent);
        let titles: Vec<&str> = arranged[0].1.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Sam", "Testing", "Arbos", "Carro", "Launch Crew"]);
        assert_eq!(shortcut_label(0).as_deref(), Some("⌘1"));
        assert_eq!(shortcut_label(8).as_deref(), Some("⌘9"));
        assert_eq!(shortcut_label(9), None);
    }

    /// A name hit outranks an indirect hit inside a section, whatever the age.
    #[test]
    fn name_hits_come_before_member_and_message_hits() {
        let rows = vec![
            entry(Kind::Group, "Old by name", 10, false),
            entry(Kind::Group, "New by member", 99, true),
        ];
        let arranged = arrange(rows, false);
        let titles: Vec<&str> = arranged[0].1.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Old by name", "New by member"]);
    }

    #[test]
    fn a_section_is_capped() {
        let rows: Vec<Entry> = (0..20)
            .map(|i| entry(Kind::Person, &format!("p{i}"), i, false))
            .collect();
        let arranged = arrange(rows, false);
        assert_eq!(arranged[0].1.len(), SECTION_CAP);
        assert_eq!(arranged[0].1[0].title, "p19", "newest first");
    }

    /// ↑↓ clamp at the ends; a shorter list pulls the highlight back in.
    #[test]
    fn the_cursor_steps_and_clamps() {
        let mut c = Cursor::default();
        c.step(-1, 5);
        assert_eq!(c.selected, 0);
        c.step(1, 5);
        c.step(1, 5);
        assert_eq!(c.selected, 2);
        c.step(10, 5);
        assert_eq!(c.selected, 4);
        c.clamp(2);
        assert_eq!(c.selected, 1);
        c.clamp(0);
        assert_eq!(c.selected, 1, "an empty list keeps the preset highlight");
        c.step(1, 0);
        assert_eq!(c.selected, 0, "stepping over nothing lands on the top");
    }

    /// Jacob's #1477 case through the palette: "carro" finds the DM by the
    /// person's display name (the relay names the DM by the handle), reads
    /// "Carro / @robertalanm" and lists under People & agents.
    #[test]
    fn display_name_search_finds_the_person_row() {
        use crate::model::{Person, SpaceKind};
        use crate::search::{normalize_query, space_hit};
        let carro = Project {
            id: "dm-1".into(),
            name: "robertalanm".into(),
            kind: SpaceKind::Dm,
            people: vec![Person {
                id: "gh_1".into(),
                handle: "robertalanm".into(),
                name: Some("Carro".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let q = normalize_query("carro");
        let hit = space_hit(&carro, "jacob", Vec::<(&str, &str)>::new(), Vec::new, &q)
            .expect("found by display name");
        let entry = space_entry(&carro, hit, 5, &q);
        assert_eq!(entry.kind, Kind::Person);
        assert_eq!(entry.title, "Carro");
        assert_eq!(entry.subtitle, "@robertalanm");
        assert_eq!(
            entry.rank,
            Rank::Direct(Match {
                kind: arbos_core::search::MatchKind::Exact,
                field: arbos_core::search::MatchField::Name
            }),
            "\"carro\" is the name"
        );
        assert_eq!(entry.section(), Section::People);
        assert_eq!(entry.target, Target::Space("dm-1".into()));

        // The same person inside a group: a member hit, listed under Chats
        // with the matcher's reason.
        let testing = Project {
            id: "g-1".into(),
            name: "Testing".into(),
            kind: SpaceKind::Group,
            people: carro.people.clone(),
            ..Default::default()
        };
        let hit = space_hit(&testing, "jacob", Vec::<(&str, &str)>::new(), Vec::new, &q)
            .expect("found by member");
        let entry = space_entry(&testing, hit, 9, &q);
        assert_eq!(entry.kind, Kind::Group);
        assert_eq!(entry.rank, Rank::Indirect);
        assert_eq!(entry.section(), Section::Chats);
        assert_eq!(entry.subtitle, "Member · Carro @robertalanm");

        // A message hit opens the chat at that message.
        let messages = || {
            vec![crate::search::MessageText {
                chat_id: "c1".into(),
                message_id: "m1".into(),
                created_at: 10,
                text: "the plan.pdf from Carro".into(),
            }]
        };
        let hit = space_hit(
            &testing,
            "jacob",
            Vec::<(&str, &str)>::new(),
            messages,
            &normalize_query("plan"),
        )
        .expect("found by message");
        let entry = space_entry(&testing, hit, 9, "plan");
        assert_eq!(
            entry.target,
            Target::Message {
                space_id: "g-1".into(),
                chat_id: "c1".into(),
                message_id: "m1".into()
            }
        );
        assert!(entry.subtitle.starts_with("Message · "));

        // No query: a plain recent row, never indirect.
        let hit = space_hit(&testing, "jacob", Vec::<(&str, &str)>::new(), Vec::new, "")
            .expect("everything matches the empty query");
        assert_eq!(space_entry(&testing, hit, 1, "").rank, Rank::EVERYTHING);
    }

    /// Rank per type (Jacob): a name that starts with the query beats one
    /// where only a later word starts with it, whatever their age; indirect
    /// hits last. Through the shared matcher a handle hit sorts after a name
    /// hit of the same kind, and a substring inside a word is no hit.
    #[test]
    fn prefix_beats_word_start_beats_indirect() {
        use arbos_core::search::{MatchField, MatchKind};
        let direct = |kind, field| Rank::Direct(Match { kind, field });
        assert_eq!(
            Rank::of("Scout", "sc"),
            direct(MatchKind::Prefix, MatchField::Name)
        );
        assert_eq!(
            Rank::of("Affinity Scout", "sc"),
            direct(MatchKind::WordStart, MatchField::Name)
        );
        assert_eq!(Rank::of("Doug", "sc"), Rank::Indirect);
        assert_eq!(Rank::of("Affinity", "ffin"), Rank::Indirect);
        assert_eq!(
            Rank::of_entity("Carro", "robertalanm", None, "rob"),
            direct(MatchKind::Prefix, MatchField::Handle)
        );
        assert!(
            direct(MatchKind::Exact, MatchField::Handle)
                < direct(MatchKind::Prefix, MatchField::Name)
        );
        assert!(
            direct(MatchKind::Prefix, MatchField::Name)
                < direct(MatchKind::Prefix, MatchField::Handle)
        );
        assert!(
            direct(MatchKind::WordStart, MatchField::Handle)
                < direct(MatchKind::Exact, MatchField::Title)
        );
        assert!(direct(MatchKind::WordStart, MatchField::Title) < Rank::Contains);
        assert!(Rank::Contains < Rank::Indirect);
        let rows = vec![
            Entry {
                rank: Rank::of("Affinity Scout", "sc"),
                ..entry(Kind::Agent, "Affinity Scout", 99, false)
            },
            Entry {
                rank: Rank::of("Scout", "sc"),
                ..entry(Kind::Agent, "Scout", 1, false)
            },
        ];
        let arranged = arrange(rows, false);
        let titles: Vec<&str> = arranged[0].1.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Scout", "Affinity Scout"]);
    }

    #[test]
    fn subtitles_read_naturally() {
        assert_eq!(server_subtitle(1, 1), "Server · 1 channel · 1 member");
        assert_eq!(server_subtitle(3, 0), "Server · 3 channels");
        assert_eq!(group_subtitle(4), "Group · 4 members");
        assert_eq!(
            link_host("https://www.github.com/unarbos/arbos?x=1"),
            "github.com"
        );
        assert_eq!(link_host("http://relay.arbos.life"), "relay.arbos.life");
    }
}
