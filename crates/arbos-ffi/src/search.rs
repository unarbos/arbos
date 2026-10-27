//! The shared search matcher for Swift (`arbos_core::search`): every iPhone
//! picker and search box ranks people, agents, groups, servers and channels
//! with the exact rule the Mac uses ([feedback
//! shared-search-matcher-ios-2026-10-26]). Queries arrive as typed; these
//! functions normalize them (`normalize_query` is idempotent).

use arbos_core::search;

/// How well a field matched, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SearchMatchKind {
    Exact,
    Prefix,
    WordStart,
}

impl From<search::MatchKind> for SearchMatchKind {
    fn from(k: search::MatchKind) -> Self {
        match k {
            search::MatchKind::Exact => SearchMatchKind::Exact,
            search::MatchKind::Prefix => SearchMatchKind::Prefix,
            search::MatchKind::WordStart => SearchMatchKind::WordStart,
        }
    }
}

/// Which field matched, best first when the kind ties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SearchMatchField {
    Name,
    Handle,
    Title,
}

impl From<search::MatchField> for SearchMatchField {
    fn from(f: search::MatchField) -> Self {
        match f {
            search::MatchField::Name => SearchMatchField::Name,
            search::MatchField::Handle => SearchMatchField::Handle,
            search::MatchField::Title => SearchMatchField::Title,
        }
    }
}

/// The best way an entity matched: `kind` then `field` is the ranking.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SearchMatch {
    pub kind: SearchMatchKind,
    pub field: SearchMatchField,
    /// The rank as one number, 0 best (`arbos_core::search::Match::order`:
    /// exact / prefix / word-start on name then handle, then the title hits),
    /// so Swift can sort by it.
    pub order: u8,
}

impl From<search::Match> for SearchMatch {
    fn from(m: search::Match) -> Self {
        Self {
            kind: m.kind.into(),
            field: m.field.into(),
            order: m.order(),
        }
    }
}

/// One row to rank: display name, @handle and an optional title.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SearchCandidate {
    pub name: String,
    pub handle: String,
    pub title: Option<String>,
}

/// One ranked row: the index into the candidates given, and how it matched.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SearchRanked {
    pub index: u32,
    pub matched: SearchMatch,
}

/// Lower-case, diacritics stripped, trimmed.
#[uniffi::export]
pub fn search_fold(text: String) -> String {
    search::fold(&text)
}

/// The typed query as the matcher sees it (leading `@` dropped, folded,
/// whitespace collapsed). Empty means "show everything".
#[uniffi::export]
pub fn search_normalize_query(query: String) -> String {
    search::normalize_query(&query)
}

/// How one text (a title, a channel name) matches the typed query.
#[uniffi::export]
pub fn search_match_text(text: String, query: String) -> Option<SearchMatchKind> {
    let q = search::normalize_query(&query);
    if q.is_empty() {
        return Some(SearchMatchKind::Prefix);
    }
    search::match_text(&text, &q).map(Into::into)
}

/// How an entity matches the typed query: the best of name, @handle, title.
/// An empty query matches everything.
#[uniffi::export]
pub fn search_match(
    name: String,
    handle: String,
    title: Option<String>,
    query: String,
) -> Option<SearchMatch> {
    let q = search::normalize_query(&query);
    search::match_entity(&name, &handle, title.as_deref(), &q).map(Into::into)
}

/// The candidates that match the typed query, best first (exact > prefix >
/// word-start, name before handle before title), stable inside a tie. An
/// empty query returns every index in order.
#[uniffi::export]
pub fn search_rank(candidates: Vec<SearchCandidate>, query: String) -> Vec<SearchRanked> {
    let q = search::normalize_query(&query);
    let indexed: Vec<(u32, SearchCandidate)> = candidates
        .into_iter()
        .enumerate()
        .map(|(i, c)| (i as u32, c))
        .collect();
    search::rank(indexed, &q, |(_, c)| search::Fields {
        name: c.name.clone(),
        handle: c.handle.clone(),
        title: c.title.clone(),
    })
    .into_iter()
    .map(|((index, _), m)| SearchRanked {
        index,
        matched: m.into(),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(name: &str, handle: &str) -> SearchCandidate {
        SearchCandidate {
            name: name.into(),
            handle: handle.into(),
            title: None,
        }
    }

    #[test]
    fn rank_keeps_indexes_and_orders_like_the_core() {
        let ranked = search_rank(
            vec![
                cand("Team Affine", "team-a"),
                cand("Andreas F", "aff"),
                cand("Doug", "doug"),
                cand("Affine", "affine-foundation"),
            ],
            "@Aff".into(),
        );
        assert_eq!(
            ranked.iter().map(|r| r.index).collect::<Vec<_>>(),
            [1, 3, 0]
        );
        assert_eq!(ranked[0].matched.kind, SearchMatchKind::Exact);
        assert_eq!(ranked[0].matched.field, SearchMatchField::Handle);
        assert!(ranked[0].matched.order < ranked[1].matched.order);
        assert!(ranked[1].matched.order < ranked[2].matched.order);
    }

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(
            search_match("Zoë".into(), "zoe".into(), None, "  ".into()).map(|m| m.kind),
            Some(SearchMatchKind::Prefix)
        );
        assert_eq!(
            search_match_text("x".into(), "".into()),
            Some(SearchMatchKind::Prefix)
        );
        assert_eq!(
            search_rank(vec![cand("b", "b"), cand("a", "a")], "".into()).len(),
            2
        );
    }

    #[test]
    fn diacritics_and_multi_word_through_the_ffi() {
        let m = search_match("Zoë Álvarez".into(), "zoe".into(), None, "zoe alv".into()).unwrap();
        assert_eq!(m.kind, SearchMatchKind::Prefix);
        assert_eq!(m.field, SearchMatchField::Name);
        assert_eq!(search_fold("Øyvind".into()), "oyvind");
        assert_eq!(search_normalize_query("@Zoë  B".into()), "zoe b");
        assert!(search_match("Carro".into(), "robertalanm".into(), None, "alan".into()).is_none());
    }
}
