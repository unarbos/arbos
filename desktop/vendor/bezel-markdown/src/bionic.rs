//! Bionic reading: the front of each word set heavier than the rest.
//!
//! The eye lands on the first letters of a word and infers the rest, so
//! weighting those letters gives it somewhere to land. This runs over the
//! shaped runs *after* the marks have been flattened, so a word that a bold
//! span or a link cuts in two is still one word here.

use std::ops::Range;

use gpui::{FontWeight, TextRun};

use crate::render::Flat;

/// The weight the fixation letters are set in — markdown bold's, so the two
/// read as one face rather than a third.
pub const WEIGHT: FontWeight = FontWeight::SEMIBOLD;

/// Weight the front of every word in `flat`. Inline code is left alone: it is
/// read letter by letter, and a heavier `co`de reads as a typo. So is a
/// formula, whose face has no heavier weight to give.
pub fn apply(flat: &mut Flat) {
    let skip: Vec<Range<usize>> = flat.code.iter().chain(&flat.math).cloned().collect();
    let fixations = fixations(flat.text.as_ref(), &skip);
    if fixations.is_empty() {
        return;
    }
    flat.runs = emphasize(std::mem::take(&mut flat.runs), &fixations);
}

/// The byte ranges to weight: the first half of each word, rounded up, for
/// every word that does not touch a range in `skip`. Sorted, disjoint.
pub fn fixations(text: &str, skip: &[Range<usize>]) -> Vec<Range<usize>> {
    words(text)
        .filter(|word| !skip.iter().any(|s| s.start < word.end && word.start < s.end))
        .map(|word| {
            let letters = text[word.clone()].chars().count();
            let front = letters.div_ceil(2);
            let end = text[word.clone()]
                .char_indices()
                .nth(front)
                .map_or(word.end, |(at, _)| word.start + at);
            word.start..end
        })
        .collect()
}

/// `runs` with every range in `fixations` set to [`WEIGHT`], cut where a range
/// starts or ends inside a run. A run already at that weight or heavier keeps
/// its own. `fixations` must be sorted and disjoint, as [`fixations`] returns.
pub fn emphasize(runs: Vec<TextRun>, fixations: &[Range<usize>]) -> Vec<TextRun> {
    let mut out = Vec::with_capacity(runs.len() + 2 * fixations.len());
    let mut at = 0;
    for run in runs {
        let end = at + run.len;
        // Every edge of a fixation that falls strictly inside this run.
        let mut cuts: Vec<usize> = fixations
            .iter()
            .flat_map(|f| [f.start, f.end])
            .filter(|cut| at < *cut && *cut < end)
            .collect();
        cuts.push(end);
        cuts.sort_unstable();
        cuts.dedup();

        let mut start = at;
        for cut in cuts {
            let mut piece = run.clone();
            piece.len = cut - start;
            if covered(fixations, start) && piece.font.weight.0 < WEIGHT.0 {
                piece.font.weight = WEIGHT;
            }
            out.push(piece);
            start = cut;
        }
        at = end;
    }
    out
}

/// Whether the byte at `offset` sits inside one of the sorted `fixations`.
fn covered(fixations: &[Range<usize>], offset: usize) -> bool {
    let ix = fixations.partition_point(|f| f.end <= offset);
    fixations.get(ix).is_some_and(|f| f.start <= offset)
}

/// Each word's byte range. A word is a run of letters and digits; an
/// apostrophe inside one (`don't`) stays part of it, one at either edge (a
/// quote) does not.
fn words(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut chars = text.char_indices().peekable();
    std::iter::from_fn(move || {
        // Skip to the next word character.
        let (start, _) = chars.by_ref().find(|(_, c)| word_char(*c))?;
        let mut end = text.len();
        while let Some((at, c)) = chars.peek().copied() {
            if !word_char(c) {
                end = at;
                break;
            }
            chars.next();
        }
        Some(trim_apostrophes(text, start..end))
    })
    .filter(|word| !word.is_empty())
}

fn word_char(c: char) -> bool {
    c.is_alphanumeric() || is_apostrophe(c)
}

fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}')
}

fn trim_apostrophes(text: &str, word: Range<usize>) -> Range<usize> {
    let slice = &text[word.clone()];
    let trimmed = slice.trim_matches(is_apostrophe);
    let lead = slice.len() - slice.trim_start_matches(is_apostrophe).len();
    let start = word.start + lead;
    start..start + trimmed.len()
}
