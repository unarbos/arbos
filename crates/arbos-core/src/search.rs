//! One matcher for every search box that lists people, agents, groups, servers
//! or channels ([feedback shared-search-matcher-mac-2026-10-26] /
//! [feedback shared-search-matcher-ios-2026-10-26]).
//!
//! Jacob, Mac v0.6.268, 2026-10-26: the "Give a person access" sheet matched
//! only on the @handle — "aff" found "Andreas F @aff" and not the people whose
//! display name starts with Aff. The earlier fix (#1477) had taught the main
//! search to look at names, handles and titles; every other picker kept its
//! own ad-hoc filter. This module is the one rule both apps call (the Mac
//! directly, the iPhone through `arbos-ffi`), so a query behaves the same in
//! the sidebar, the palette, the people pickers, the Secrets agent picker,
//! @mention completion and the Add-members sheets.
//!
//! The rule:
//! - Compare folded text: lower-case, diacritics stripped (`Zoë` = `zoe`,
//!   `José` = `jose`, `Øyvind` = `oyvind`), a leading `@` on the query dropped.
//! - A field matches when the query equals it ([`MatchKind::Exact`]), starts
//!   it ([`MatchKind::Prefix`]), or when every word of the query starts a word
//!   of the field in order ([`MatchKind::WordStart`]: `aff` finds `Affine`,
//!   `AffineTeam` and `Team Affine`; `an f` finds `Andreas F`). Words split on
//!   anything that is not a letter or digit and on a camelCase step.
//! - An entity is matched on its display name, its @handle, and a title when
//!   it has one; the best field wins, display name before handle when the
//!   kind ties, and a title hit only after every name / handle hit ([`Match`]
//!   orders itself that way).
//! - A list is ranked exact > prefix > word-start, name before handle, and is
//!   otherwise stable: rows that tie keep the order they came in.
//!
//! Message bodies are not entities and keep their own substring search.

use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// How well a field matched, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchKind {
    /// The folded field equals the folded query.
    Exact,
    /// The folded field starts with the folded query.
    Prefix,
    /// Every word of the query starts a word of the field, in order.
    WordStart,
}

/// Which field of an entity matched, best first when the kind ties.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchField {
    /// The display name (a person's name, an agent's or group's or server's name).
    Name,
    /// The @handle (a person's login, an agent's mention slug).
    Handle,
    /// A title where the entity has one (an agent's description, a server's tagline).
    Title,
}

/// The best way an entity matched a query. Orders by kind, then field — name
/// before handle — with every title hit after the name / handle hits, so a
/// `min()` over several fields (or a stable sort over a list) is the ranking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Match {
    pub kind: MatchKind,
    pub field: MatchField,
}

impl Match {
    /// What every row gets for an empty query: nothing to rank, keep the order.
    pub const EVERYTHING: Match = Match {
        kind: MatchKind::Prefix,
        field: MatchField::Name,
    };

    /// The rank as one number, 0 best: exact name 0, exact handle 1, prefix
    /// name 2, prefix handle 3, word-start name 4, word-start handle 5, then
    /// the title hits 6 (exact), 7 (prefix), 8 (word-start).
    pub fn order(self) -> u8 {
        let kind = self.kind as u8;
        match self.field {
            MatchField::Name => kind * 2,
            MatchField::Handle => kind * 2 + 1,
            MatchField::Title => 6 + kind,
        }
    }
}

impl PartialOrd for Match {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Match {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.order().cmp(&other.order())
    }
}

/// Lower-case, diacritics stripped, trimmed: the form every haystack and the
/// query are compared in. `ß`, `ø`, `æ`, `œ`, `ł`, `đ` and the dotless `ı`
/// do not decompose and are mapped by hand.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.trim().chars() {
        push_folded(c, &mut out);
    }
    out
}

fn push_folded(c: char, out: &mut String) {
    match c {
        'ß' | 'ẞ' => out.push_str("ss"),
        'ø' | 'Ø' => out.push('o'),
        'æ' | 'Æ' => out.push_str("ae"),
        'œ' | 'Œ' => out.push_str("oe"),
        'ł' | 'Ł' => out.push('l'),
        'đ' | 'Đ' => out.push('d'),
        'ı' => out.push('i'),
        _ => out.extend(
            std::iter::once(c)
                .flat_map(char::to_lowercase)
                .nfd()
                .filter(|c| !is_combining_mark(*c)),
        ),
    }
}

/// The typed query as the matcher sees it: trimmed, a leading `@` dropped so
/// "@robert" finds the handle too, folded, inner runs of whitespace collapsed
/// to one space. Empty means "show everything". Idempotent.
pub fn normalize_query(query: &str) -> String {
    let stripped = query.trim().trim_start_matches('@');
    fold(stripped)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The words of `text`, folded: split on anything that is not a letter or a
/// digit, and before a capital that follows a lower-case letter or a digit
/// (`AffineTeam` → `affine`, `team`; `robert-alan_m` → `robert`, `alan`, `m`).
pub fn words(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut prev_lower_or_digit = false;
    for c in text.chars() {
        if !c.is_alphanumeric() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            prev_lower_or_digit = false;
            continue;
        }
        if c.is_uppercase() && prev_lower_or_digit && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        push_folded(c, &mut current);
        prev_lower_or_digit = c.is_lowercase() || c.is_numeric();
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// How `text` matches `q` (already [`normalize_query`]'d, non-empty).
/// `None` when it does not.
pub fn match_text(text: &str, q: &str) -> Option<MatchKind> {
    if q.is_empty() {
        return None;
    }
    let folded = fold(text);
    if folded.is_empty() {
        return None;
    }
    if folded == q {
        return Some(MatchKind::Exact);
    }
    if folded.starts_with(q) {
        return Some(MatchKind::Prefix);
    }
    let words = words(text);
    let mut from = 0usize;
    for token in q.split_whitespace() {
        let found = words[from.min(words.len())..]
            .iter()
            .position(|w| w.starts_with(token))?;
        from += found + 1;
    }
    Some(MatchKind::WordStart)
}

/// How an entity matches `q` (already [`normalize_query`]'d): the best of its
/// display name, @handle (a leading `@` is ignored) and title. An empty query
/// matches everything ([`Match::EVERYTHING`]).
pub fn match_entity(name: &str, handle: &str, title: Option<&str>, q: &str) -> Option<Match> {
    if q.is_empty() {
        return Some(Match::EVERYTHING);
    }
    let fields = [
        (MatchField::Name, Some(name)),
        (MatchField::Handle, Some(handle.trim_start_matches('@'))),
        (MatchField::Title, title),
    ];
    fields
        .into_iter()
        .filter_map(|(field, text)| {
            let kind = match_text(text?, q)?;
            Some(Match { kind, field })
        })
        .min()
}

/// The fields an item is searched by.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fields {
    pub name: String,
    pub handle: String,
    pub title: Option<String>,
}

impl Fields {
    pub fn new(name: impl Into<String>, handle: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            handle: handle.into(),
            title: None,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        let title = title.into();
        self.title = (!title.trim().is_empty()).then_some(title);
        self
    }

    pub fn matches(&self, q: &str) -> Option<Match> {
        match_entity(&self.name, &self.handle, self.title.as_deref(), q)
    }
}

/// The items that match `q` (already [`normalize_query`]'d), best first:
/// exact > prefix > word-start, display name before handle before title, and
/// stable inside a tie. An empty query keeps every item in its order.
pub fn rank<T>(items: Vec<T>, q: &str, fields: impl Fn(&T) -> Fields) -> Vec<(T, Match)> {
    let mut out: Vec<(T, Match)> = items
        .into_iter()
        .filter_map(|item| {
            let m = fields(&item).matches(q)?;
            Some((item, m))
        })
        .collect();
    out.sort_by_key(|(_, m)| *m);
    out
}

/// `rank` without the match: the items only.
pub fn filter<T>(items: Vec<T>, q: &str, fields: impl Fn(&T) -> Fields) -> Vec<T> {
    rank(items, q, fields).into_iter().map(|(t, _)| t).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(kind: MatchKind, field: MatchField) -> Option<Match> {
        Some(Match { kind, field })
    }

    #[test]
    fn fold_drops_case_and_diacritics() {
        assert_eq!(fold("  Zoë "), "zoe");
        assert_eq!(fold("José"), "jose");
        assert_eq!(fold("Øyvind Ångström"), "oyvind angstrom");
        assert_eq!(fold("Straße"), "strasse");
        assert_eq!(fold("Łukasz"), "lukasz");
        assert_eq!(fold("Ærø"), "aero");
    }

    #[test]
    fn normalize_query_strips_at_and_collapses_space() {
        assert_eq!(normalize_query("@Aff"), "aff");
        assert_eq!(normalize_query("  andreas   F "), "andreas f");
        assert_eq!(normalize_query("@"), "");
        assert_eq!(normalize_query(&normalize_query("@Zoë  B")), "zoe b");
    }

    #[test]
    fn words_split_on_separators_and_camel_case() {
        assert_eq!(words("AffineTeam"), ["affine", "team"]);
        assert_eq!(words("robert-alan_m"), ["robert", "alan", "m"]);
        assert_eq!(words("Andreas F"), ["andreas", "f"]);
        assert_eq!(words("v2Beta"), ["v2", "beta"]);
        assert_eq!(words("ABC"), ["abc"]);
        assert!(words("  ").is_empty());
    }

    /// Jacob's case: "aff" must find the display names that start with it, not only @aff.
    #[test]
    fn aff_finds_affine_affineteam_and_the_handle() {
        assert_eq!(match_text("Affine", "aff"), Some(MatchKind::Prefix));
        assert_eq!(match_text("AffineTeam", "aff"), Some(MatchKind::Prefix));
        assert_eq!(match_text("Team Affine", "aff"), Some(MatchKind::WordStart));
        assert_eq!(match_text("aff", "aff"), Some(MatchKind::Exact));
        assert_eq!(match_text("Andreas F", "aff"), None);
        assert_eq!(
            match_entity("Andreas F", "aff", None, "aff"),
            m(MatchKind::Exact, MatchField::Handle)
        );
        assert_eq!(
            match_entity("Affine", "affine-foundation", None, "aff"),
            m(MatchKind::Prefix, MatchField::Name)
        );
    }

    #[test]
    fn no_substring_in_the_middle_of_a_word() {
        assert_eq!(match_text("Affine", "ffi"), None);
        assert_eq!(match_text("robertalanm", "alan"), None);
        assert_eq!(match_entity("Carro", "robertalanm", None, "alan"), None);
    }

    #[test]
    fn multi_word_query_matches_word_starts_in_order() {
        assert_eq!(match_text("Andreas F", "an f"), Some(MatchKind::WordStart));
        assert_eq!(match_text("Andreas F", "andreas f"), Some(MatchKind::Exact));
        assert_eq!(
            match_text("Andreas Fischer", "andreas f"),
            Some(MatchKind::Prefix)
        );
        assert_eq!(match_text("Andreas F", "f an"), None);
        assert_eq!(match_text("Sam Rivera", "sam riv"), Some(MatchKind::Prefix));
        assert_eq!(match_text("Sam Rivera", "s r"), Some(MatchKind::WordStart));
        assert_eq!(match_text("Sam Rivera", "s s"), None);
    }

    #[test]
    fn diacritics_fold_both_ways() {
        assert_eq!(
            match_text("Zoë", &normalize_query("zoe")),
            Some(MatchKind::Exact)
        );
        assert_eq!(
            match_text("zoe", &normalize_query("Zoë")),
            Some(MatchKind::Exact)
        );
        assert_eq!(
            match_text("José Álvarez", &normalize_query("alv")),
            Some(MatchKind::WordStart)
        );
        assert_eq!(
            match_entity("Øyvind", "oyvind", None, &normalize_query("øy")),
            m(MatchKind::Prefix, MatchField::Name)
        );
    }

    #[test]
    fn order_is_name_handle_pairs_then_titles() {
        let m = |kind, field| Match { kind, field };
        let orders: Vec<u8> = [
            m(MatchKind::Exact, MatchField::Name),
            m(MatchKind::Exact, MatchField::Handle),
            m(MatchKind::Prefix, MatchField::Name),
            m(MatchKind::Prefix, MatchField::Handle),
            m(MatchKind::WordStart, MatchField::Name),
            m(MatchKind::WordStart, MatchField::Handle),
            m(MatchKind::Exact, MatchField::Title),
            m(MatchKind::Prefix, MatchField::Title),
            m(MatchKind::WordStart, MatchField::Title),
        ]
        .iter()
        .map(|m| m.order())
        .collect();
        assert_eq!(orders, (0..9).collect::<Vec<u8>>());
        assert!(
            m(MatchKind::WordStart, MatchField::Handle) < m(MatchKind::Exact, MatchField::Title)
        );
    }

    #[test]
    fn name_beats_handle_beats_title_when_the_kind_ties() {
        assert_eq!(
            match_entity("Sam", "sam", None, "sam"),
            m(MatchKind::Exact, MatchField::Name)
        );
        assert_eq!(
            match_entity("Carro", "robertalanm", Some("Research"), "r"),
            m(MatchKind::Prefix, MatchField::Handle)
        );
        assert_eq!(
            match_entity("Carro", "x", Some("Research"), "res"),
            m(MatchKind::Prefix, MatchField::Title)
        );
        assert_eq!(
            match_entity("Carro", "@robertalanm", None, "robert"),
            m(MatchKind::Prefix, MatchField::Handle)
        );
    }

    #[test]
    fn exact_beats_prefix_beats_word_start_across_fields() {
        // An exact handle outranks a name that merely starts with the query.
        let exact_handle = match_entity("Andreas F", "aff", None, "aff").unwrap();
        let prefix_name = match_entity("Affine", "affine-foundation", None, "aff").unwrap();
        let word_start = match_entity("Team Affine", "team", None, "aff").unwrap();
        assert!(exact_handle < prefix_name);
        assert!(prefix_name < word_start);
    }

    #[test]
    fn empty_query_matches_everything_in_order() {
        assert_eq!(match_entity("A", "a", None, ""), Some(Match::EVERYTHING));
        let ranked = rank(vec!["b", "a", "c"], "", |s| Fields::new(*s, *s));
        assert_eq!(
            ranked.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            ["b", "a", "c"]
        );
    }

    #[test]
    fn rank_is_exact_then_prefix_then_word_start_and_stable() {
        let people = vec![
            ("Team Affine", "team-a"),
            ("AffineTeam", "affteam"),
            ("Andreas F", "aff"),
            ("Affine", "affine-foundation"),
            ("Doug", "doug"),
            ("Second Affine", "second"),
        ];
        let ranked = rank(people, "aff", |(n, h)| Fields::new(*n, *h));
        let names: Vec<&str> = ranked.iter().map(|((n, _), _)| *n).collect();
        // Exact (@aff); prefix name (AffineTeam, Affine — input order kept);
        // word-start name (Team Affine before Second Affine — input order kept).
        assert_eq!(
            names,
            [
                "Andreas F",
                "AffineTeam",
                "Affine",
                "Team Affine",
                "Second Affine"
            ]
        );
    }

    #[test]
    fn title_is_searched_only_when_present() {
        let agents = vec![
            Fields::new("Comms", "comms").with_title("Research assistant"),
            Fields::new("Research Bot", "research"),
            Fields::new("Blank", "blank").with_title("   "),
        ];
        let ranked = rank(agents, "res", |f| f.clone());
        let names: Vec<&str> = ranked.iter().map(|(f, _)| f.name.as_str()).collect();
        assert_eq!(names, ["Research Bot", "Comms"]);
        assert_eq!(ranked[1].1.field, MatchField::Title);
        // A title hit comes after every name / handle hit, whatever its kind.
        let agents = vec![
            Fields::new("Comms", "comms").with_title("Research assistant"),
            Fields::new("Affine Research", "affine-research"),
        ];
        let ranked = rank(agents, "res", |f| f.clone());
        assert_eq!(ranked[0].0.name, "Affine Research");
    }
}
