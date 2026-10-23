//! Sidebar search: one ranked match model for every row (feedback
//! `search-name-matching-2026-10-23`). Jacob typed "carro" and the person
//! Carro (@robertalanm) did not appear — the filter looked at the DM's relay
//! name (the handle) and never at the person's display name — while the group
//! "Testing" did, because any loaded message body counted with no visible
//! reason. Now a row appears only when the query matches what the row can
//! show (name, @handle, title, a member, a message) and a hit that is not the
//! row's own name says why on its second line ("Member · Carro @robertalanm").
//!
//! Matching folds case and diacritics ("Zoe" finds "Zoë", "jose" finds "José")
//! and ignores a leading `@`. Ranking: people (display name / handle), then
//! chats by title, then chats by a member's name, then message bodies.
//! The iPhone has the same model in `HomeSearch.swift`.

use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

use crate::model::{Id, Project, ServerInfo};

/// What the row matched on, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HitKind {
    /// A person's display name or @handle (DM rows).
    Person,
    /// The row's own title: agent name, group title, server or channel name, a chat title.
    Title,
    /// Someone (or an agent) inside a group / shared project.
    Member,
    /// A loaded message body.
    Message,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub kind: HitKind,
    /// Shown on the row's second line when the name alone does not explain the hit.
    pub reason: Option<String>,
    /// The message to scroll to (`Message` hits): (chat id, message id).
    pub message: Option<(Id, Id)>,
}

impl SearchHit {
    fn plain(kind: HitKind) -> Self {
        Self {
            kind,
            reason: None,
            message: None,
        }
    }

    fn with_reason(kind: HitKind, reason: String) -> Self {
        Self {
            kind,
            reason: Some(reason),
            message: None,
        }
    }
}

/// One message body a space can be found by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageText {
    pub chat_id: Id,
    pub message_id: Id,
    pub created_at: u64,
    pub text: String,
}

/// Lower-case, diacritics stripped, trimmed: the form every haystack and the
/// query are compared in.
pub fn fold(s: &str) -> String {
    s.trim()
        .chars()
        .flat_map(char::to_lowercase)
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect()
}

/// The typed query as the matcher sees it: folded, a leading `@` dropped so
/// "@robert" finds the handle too. Empty means "show everything".
pub fn normalize_query(query: &str) -> String {
    fold(query.trim().trim_start_matches('@'))
}

fn contains(hay: &str, q: &str) -> bool {
    fold(hay).contains(q)
}

/// How a DM / group / agent row matches `q` (already normalized, non-empty).
/// `chats` are the space's (id, title) pairs; `messages` is asked for only
/// when nothing else matched (it walks every loaded message).
pub fn space_hit<'a>(
    space: &Project,
    me_handle: &str,
    chats: impl IntoIterator<Item = (&'a str, &'a str)>,
    messages: impl FnOnce() -> Vec<MessageText>,
    q: &str,
) -> Option<SearchHit> {
    if q.is_empty() {
        return Some(SearchHit::plain(HitKind::Title));
    }
    let is_dm = space.is_people() && !space.is_people_group();
    if is_dm {
        if let Some(p) = space.person() {
            if contains(&p.display_name(), q) || contains(&p.handle, q) {
                return Some(SearchHit::plain(HitKind::Person));
            }
        }
    } else if contains(&space.name, q) {
        return Some(SearchHit::plain(HitKind::Title));
    }
    if !is_dm {
        if let Some(p) = space
            .people
            .iter()
            .filter(|p| p.handle != me_handle)
            .find(|p| contains(&p.display_name(), q) || contains(&p.handle, q))
        {
            let name = p.display_name();
            let reason = if name == p.handle {
                format!("Member · @{}", p.handle)
            } else {
                format!("Member · {name} @{}", p.handle)
            };
            return Some(SearchHit::with_reason(HitKind::Member, reason));
        }
        if let Some(a) = space.agents.iter().find(|a| contains(&a.name, q)) {
            return Some(SearchHit::with_reason(
                HitKind::Member,
                format!("Agent · {}", a.name),
            ));
        }
    }
    if let Some((_, title)) = chats
        .into_iter()
        .find(|(_, title)| !title.trim().is_empty() && contains(title, q))
    {
        return Some(SearchHit::with_reason(
            HitKind::Title,
            format!("Chat · {}", title.trim()),
        ));
    }
    message_hit(messages(), q)
}

/// The newest message whose prose contains `q`, with the snippet as the reason.
pub fn message_hit(messages: Vec<MessageText>, q: &str) -> Option<SearchHit> {
    let best = messages
        .into_iter()
        .filter_map(|m| snippet(&m.text, q).map(|s| (m, s)))
        .max_by_key(|(m, _)| m.created_at)?;
    let (m, snippet) = best;
    Some(SearchHit {
        kind: HitKind::Message,
        reason: Some(format!("Message · {snippet}")),
        message: Some((m.chat_id, m.message_id)),
    })
}

/// A server row: its name, or one of its channels (named in the reason).
pub fn server_hit(server: &ServerInfo, q: &str) -> Option<SearchHit> {
    if q.is_empty() || contains(&server.name, q) {
        return Some(SearchHit::plain(HitKind::Title));
    }
    server
        .channels
        .iter()
        .find(|c| contains(&c.name, q))
        .map(|c| SearchHit::with_reason(HitKind::Title, format!("#{}", c.name)))
}

/// Up to this many characters of context before the match in a snippet.
const SNIPPET_BEFORE: usize = 18;
/// Up to this many characters after the match.
const SNIPPET_AFTER: usize = 60;

/// The part of `text` around the first occurrence of `q` (folded), one line,
/// with ellipses where it was cut; `None` when `q` does not occur.
pub fn snippet(text: &str, q: &str) -> Option<String> {
    if q.is_empty() {
        return None;
    }
    // Fold char by char, remembering which original char each folded char
    // came from, so a hit in the folded text maps back to the original.
    let chars: Vec<char> = text.chars().collect();
    let mut folded = String::new();
    let mut origin: Vec<usize> = Vec::new();
    for (i, c) in chars.iter().enumerate() {
        for f in std::iter::once(*c)
            .flat_map(char::to_lowercase)
            .nfd()
            .filter(|c| !is_combining_mark(*c))
        {
            folded.push(f);
            origin.push(i);
        }
    }
    let start_byte = folded.find(q)?;
    let start_fc = folded[..start_byte].chars().count();
    let q_fc = q.chars().count();
    let end_fc = (start_fc + q_fc).min(origin.len());
    let start = origin[start_fc];
    let end = if end_fc == 0 {
        start
    } else {
        origin[end_fc - 1] + 1
    };
    let from = start.saturating_sub(SNIPPET_BEFORE);
    let to = (end + SNIPPET_AFTER).min(chars.len());
    let mut out = String::new();
    if from > 0 {
        out.push('…');
    }
    out.extend(
        chars[from..to]
            .iter()
            .map(|c| if c.is_whitespace() { ' ' } else { *c }),
    );
    if to < chars.len() {
        out.push('…');
    }
    Some(out.split_whitespace().collect::<Vec<_>>().join(" "))
}

#[cfg(test)]
mod tests {
    //! Jacob, Mac, 2026-10-23: "carro" showed only the group Testing, not the
    //! person Carro; "robertalan" showed Carro (handle) and Testing again.
    use super::*;
    use crate::model::{AgentMember, Person, SpaceKind};

    fn person(handle: &str, name: Option<&str>) -> Person {
        Person {
            id: format!("gh_{handle}"),
            handle: handle.into(),
            name: name.map(str::to_string),
            ..Default::default()
        }
    }

    fn dm(handle: &str, name: Option<&str>) -> Project {
        Project {
            id: format!("dm-{handle}"),
            // The relay names a DM by the other side's handle.
            name: handle.into(),
            kind: SpaceKind::Dm,
            people: vec![person(handle, name)],
            ..Default::default()
        }
    }

    fn group(title: &str, members: Vec<Person>) -> Project {
        Project {
            id: format!("group-{title}"),
            name: title.into(),
            kind: SpaceKind::Group,
            people: members,
            ..Default::default()
        }
    }

    fn agent(name: &str) -> Project {
        Project {
            id: format!("agent-{name}"),
            name: name.into(),
            ..Default::default()
        }
    }

    fn msg(chat: &str, id: &str, at: u64, text: &str) -> MessageText {
        MessageText {
            chat_id: chat.into(),
            message_id: id.into(),
            created_at: at,
            text: text.into(),
        }
    }

    fn no_chats() -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }

    #[test]
    fn display_name_finds_the_person() {
        let carro = dm("robertalanm", Some("Carro"));
        let q = normalize_query("carro");
        let hit = space_hit(&carro, "jacob", no_chats(), Vec::new, &q).expect("Carro is found");
        assert_eq!(hit.kind, HitKind::Person);
        assert_eq!(hit.reason, None, "the row's own name explains the hit");
    }

    #[test]
    fn handle_prefix_finds_the_person_with_or_without_at() {
        let carro = dm("robertalanm", Some("Carro"));
        for typed in ["robertalan", "@robertalan", "ROBERTALAN", " robertalanm "] {
            let hit = space_hit(
                &carro,
                "jacob",
                no_chats(),
                Vec::new,
                &normalize_query(typed),
            );
            assert_eq!(hit.map(|h| h.kind), Some(HitKind::Person), "{typed:?}");
        }
    }

    #[test]
    fn unrelated_group_does_not_appear() {
        let testing = group("Testing", vec![person("kenobijon", Some("Kenobi"))]);
        let messages = || vec![msg("c1", "m1", 10, "Missed call from Kenobi")];
        assert_eq!(
            space_hit(
                &testing,
                "jacob",
                no_chats(),
                messages,
                &normalize_query("carro")
            ),
            None
        );
        assert_eq!(
            space_hit(
                &testing,
                "jacob",
                no_chats(),
                messages,
                &normalize_query("robertalan")
            ),
            None
        );
    }

    #[test]
    fn group_with_the_person_as_member_says_so() {
        let testing = group(
            "Testing",
            vec![
                person("kenobijon", Some("Kenobi")),
                person("robertalanm", Some("Carro")),
            ],
        );
        let hit = space_hit(
            &testing,
            "jacob",
            no_chats(),
            Vec::new,
            &normalize_query("carro"),
        )
        .unwrap();
        assert_eq!(hit.kind, HitKind::Member);
        assert_eq!(hit.reason.as_deref(), Some("Member · Carro @robertalanm"));
        let by_handle = space_hit(
            &testing,
            "jacob",
            no_chats(),
            Vec::new,
            &normalize_query("robertalan"),
        )
        .unwrap();
        assert_eq!(
            by_handle.reason.as_deref(),
            Some("Member · Carro @robertalanm")
        );
    }

    #[test]
    fn my_own_name_does_not_match_every_group() {
        let testing = group(
            "Testing",
            vec![person("jacob", Some("Jacob Steeves")), person("sam", None)],
        );
        assert_eq!(
            space_hit(
                &testing,
                "jacob",
                no_chats(),
                Vec::new,
                &normalize_query("jacob")
            ),
            None
        );
        let hit = space_hit(
            &testing,
            "jacob",
            no_chats(),
            Vec::new,
            &normalize_query("sam"),
        )
        .unwrap();
        assert_eq!(
            hit.reason.as_deref(),
            Some("Member · @sam"),
            "no name → handle only"
        );
    }

    #[test]
    fn message_body_hit_names_the_message() {
        let testing = group("Testing", vec![person("kenobijon", Some("Kenobi"))]);
        let messages = || {
            vec![
                msg("c1", "old", 10, "ask carro about the deck"),
                msg(
                    "c1",
                    "new",
                    20,
                    "Carro said the launch is Tuesday, so plan for it",
                ),
            ]
        };
        let hit = space_hit(
            &testing,
            "jacob",
            no_chats(),
            messages,
            &normalize_query("carro"),
        )
        .unwrap();
        assert_eq!(hit.kind, HitKind::Message);
        assert_eq!(
            hit.message,
            Some(("c1".into(), "new".into())),
            "newest match wins"
        );
        assert_eq!(
            hit.reason.as_deref(),
            Some("Message · Carro said the launch is Tuesday, so plan for it")
        );
    }

    #[test]
    fn ranking_people_then_titles_then_members_then_messages() {
        let carro = dm("robertalanm", Some("Carro"));
        let carro_group = group("Carro's launch", vec![]);
        let with_carro = group("Testing", vec![person("robertalanm", Some("Carro"))]);
        let mentions = agent("Research");
        let q = normalize_query("carro");
        let kinds: Vec<HitKind> = [
            space_hit(
                &mentions,
                "jacob",
                no_chats(),
                || vec![msg("c", "m", 1, "ping carro")],
                &q,
            ),
            space_hit(&with_carro, "jacob", no_chats(), Vec::new, &q),
            space_hit(&carro_group, "jacob", no_chats(), Vec::new, &q),
            space_hit(&carro, "jacob", no_chats(), Vec::new, &q),
        ]
        .into_iter()
        .map(|h| h.unwrap().kind)
        .collect();
        let mut sorted = kinds.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            vec![
                HitKind::Person,
                HitKind::Title,
                HitKind::Member,
                HitKind::Message
            ]
        );
        assert_eq!(kinds, sorted.into_iter().rev().collect::<Vec<_>>());
    }

    #[test]
    fn case_and_diacritics_fold_both_ways() {
        let zoe = dm("zoe-m", Some("Zoë Martín"));
        for typed in ["zoe", "ZOË", "martin", "Martín", "zoë martin"] {
            let hit = space_hit(&zoe, "jacob", no_chats(), Vec::new, &normalize_query(typed));
            assert_eq!(hit.map(|h| h.kind), Some(HitKind::Person), "{typed:?}");
        }
        let jose = dm("jose", Some("Jose"));
        assert!(space_hit(
            &jose,
            "jacob",
            no_chats(),
            Vec::new,
            &normalize_query("José")
        )
        .is_some());
    }

    #[test]
    fn agent_rows_match_name_members_and_chat_titles() {
        let research = agent("Research");
        let q = normalize_query("research");
        assert_eq!(
            space_hit(&research, "jacob", no_chats(), Vec::new, &q).map(|h| h.kind),
            Some(HitKind::Title)
        );
        let chats = vec![("c1", "Interface research"), ("c2", "Budget")];
        let hit = space_hit(&agent("Arbos"), "jacob", chats, Vec::new, &q).unwrap();
        assert_eq!(hit.kind, HitKind::Title);
        assert_eq!(hit.reason.as_deref(), Some("Chat · Interface research"));
        let mut shared = agent("Studio");
        shared.shared = true;
        shared.people = vec![person("sam", Some("Sam Rivera"))];
        let hit = space_hit(
            &shared,
            "jacob",
            no_chats(),
            Vec::new,
            &normalize_query("rivera"),
        )
        .unwrap();
        assert_eq!(hit.reason.as_deref(), Some("Member · Sam Rivera @sam"));
        let mut with_agent = group("Launch week", vec![]);
        with_agent.agents = vec![AgentMember {
            name: "Arbos".into(),
            ..Default::default()
        }];
        let hit = space_hit(
            &with_agent,
            "jacob",
            no_chats(),
            Vec::new,
            &normalize_query("arbos"),
        )
        .unwrap();
        assert_eq!(hit.reason.as_deref(), Some("Agent · Arbos"));
    }

    #[test]
    fn empty_query_shows_everything_and_nothing_matches_gibberish() {
        let carro = dm("robertalanm", Some("Carro"));
        assert!(space_hit(&carro, "jacob", no_chats(), Vec::new, &normalize_query("")).is_some());
        assert!(space_hit(&carro, "jacob", no_chats(), Vec::new, &normalize_query("@")).is_some());
        assert_eq!(
            space_hit(
                &carro,
                "jacob",
                no_chats(),
                Vec::new,
                &normalize_query("zzqx")
            ),
            None
        );
    }

    #[test]
    fn servers_match_name_or_channel() {
        let mut server = ServerInfo {
            name: "Bittensor".into(),
            ..Default::default()
        };
        server.channels = vec![crate::model::ServerChannel {
            name: "general".into(),
            ..Default::default()
        }];
        assert_eq!(
            server_hit(&server, &normalize_query("bitt"))
                .unwrap()
                .reason,
            None
        );
        assert_eq!(
            server_hit(&server, &normalize_query("gener"))
                .unwrap()
                .reason
                .as_deref(),
            Some("#general")
        );
        assert_eq!(server_hit(&server, &normalize_query("carro")), None);
    }

    #[test]
    fn snippets_cut_around_the_match() {
        let long = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty twenty-one twenty-two";
        let s = snippet(long, "ten").unwrap();
        assert!(s.starts_with('…'), "{s}");
        assert!(s.contains("ten"), "{s}");
        assert!(s.ends_with('…'), "{s}");
        assert_eq!(
            snippet("Zoë\nwas here", "zoe").as_deref(),
            Some("Zoë was here")
        );
        assert_eq!(snippet("plain", "x"), None);
    }
}
