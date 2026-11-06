//! The one thing a job's answer may not say: that the work was not a background job.
//!
//! Jacob, Test5 round 12 (2026-11-05, image 0.1.196, seed 508151f6a, chat
//! `01M6ME9A7EGV5PJ7DZW43AHTN5`), bug AD. The final post of job `01M6PHP14MPAGBJE83VKXBHPNS` —
//! started through `start_job`, with four `job_notes` posted in the chat — ended "the four steps
//! ran in the foreground, not as a background job". A minute earlier, at 13:07:23, the same agent
//! had answered the same question correctly: the work was "still running in the background".
//!
//! The sentence is a category error, not a wording slip. The `start_job` tool description tells
//! the main model that a worker's steps run in the foreground and that it must never brief a
//! detach, so that instruction goes into the brief; the worker reads the brief for its acceptance
//! list, scores a criterion it names "background job" as unmet, and
//! [`crate::agent::Agent::write_job_result`] hands the whole contract to the talking model as
//! notes. An instruction about how steps run *inside* a job became a claim about whether the job
//! itself was backgrounded, and an unmet acceptance criterion became a description of the user's
//! own work.
//!
//! Which is why the rule here is not a prompt. Whether the work ran as a background job is a fact
//! the box holds — the ask has a job row, with an id and its notes — so a sentence of the answer
//! that denies it is false by construction on this path, whatever led the model to write it. The
//! prompt side of the fix stops the inference (`Agent::job_backgrounding_note`); this stops the
//! words, which is the only half that can make the claim impossible rather than unlikely.
//!
//! Two limits, named rather than hidden:
//!
//! - **A denying sentence goes whole.** Where the model welds the result to the denial in one
//!   sentence ("1,200 rows are in summary.csv, though this was not a background job") the result
//!   goes with it, and the caller falls back to the worker's own words. Clause surgery was the
//!   alternative and it is worse: cutting that sentence in half either leaves "1,200 rows are in
//!   summary.csv, though", or writes new prose on the user's behalf.
//! - **A bare foreground claim stays.** "The four steps ran in the foreground" is true of the
//!   steps, and telling it apart from "this ran in the foreground" needs to know which noun the
//!   verb belongs to. It is process narration the prompt already bans and it asserts nothing
//!   false, so it is left to the prompt; only the denial is false by construction.

/// A word as it is matched: lower case, letters and digits only, so "job," matches "job",
/// "(not" matches "not" and "wasn't" matches "wasnt". Everything else — brackets, commas, an
/// em-dash standing alone — becomes an empty key and is dropped.
fn word_key(w: &str) -> String {
    w.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Words that say the thing beside them did **not** happen, or happened instead of something
/// else. A bare "no" is not here: "No problem, it ran in the background" carries one and denies
/// nothing.
const DENIES: &[&str] = &[
    "not", "never", "nothing", "wasnt", "werent", "isnt", "arent", "didnt", "doesnt", "dont",
    "hasnt", "havent", "cant", "couldnt", "wouldnt", "failed", "unmet", "rather", "instead",
    "without", "outside",
];

/// The ways an answer names backgrounding, as runs of adjacent word keys.
const BACKGROUNDED: &[&[&str]] = &[
    &["background", "job"],
    &["background", "jobs"],
    &["background", "task"],
    &["background", "tasks"],
    &["background", "work"],
    &["in", "the", "background"],
    &["backgrounded"],
];

/// How the work ran instead, as a person writes it. A denial that *follows* the mention and then
/// names one of these is the true sentence in disguise — "it ran in the background, not in the
/// foreground" — and must survive. See [`denies_backgrounding`].
const ALTERNATIVE: &[&str] = &["foreground", "inline", "inlined"];

/// Words that may sit between the denial and the mention without changing which one denies the
/// other. A closed list on purpose: any word that is not on it breaks the match, which is what
/// keeps "it didn't take long; it ran in the background" and "I did not run anything in the
/// background besides this" out.
const FILLER: &[&str] = &[
    "a",
    "an",
    "the",
    "as",
    "of",
    "than",
    "to",
    "be",
    "been",
    "being",
    "was",
    "were",
    "is",
    "are",
    "run",
    "ran",
    "runs",
    "running",
    "start",
    "started",
    "starting",
    "do",
    "did",
    "done",
    "doing",
    "it",
    "this",
    "that",
    "these",
    "those",
    "them",
    "work",
    "criterion",
    "criteria",
    "actually",
    "really",
    "technically",
    "strictly",
    "truly",
    "just",
    "quite",
    "treated",
    "handled",
    "executed",
    "detached",
];

/// How many filler words may sit between the denial and the mention. Five covers "not run as a
/// background job" and every shape round 12 produced; a longer gap is a different sentence.
const GAP: usize = 5;

/// Does a backgrounding mention start at `words[0]`? Its length, when one does.
fn mention_at(words: &[String]) -> Option<usize> {
    BACKGROUNDED.iter().find_map(|phrase| {
        (words.len() >= phrase.len()
            && phrase
                .iter()
                .zip(words.iter())
                .all(|(p, w)| *p == w.as_str()))
        .then_some(phrase.len())
    })
}

/// Does this sentence tell the reader the work was not a background job?
///
/// A denial word from [`DENIES`] and a mention from [`BACKGROUNDED`], with at most [`GAP`]
/// [`FILLER`] words between them, in either order:
///
/// - denial first ("**not** as a **background job**") is round 12's shape;
/// - mention first ("the **background job** criterion was **not** met") is the unmet criterion
///   restated, and it counts unless the denial then names how the work ran instead — "it ran in
///   the **background**, **not** in the **foreground**" is the true sentence written that way,
///   so [`ALTERNATIVE`] cancels it.
///
/// Deliberately narrow in both directions: a shape it misses leaves the sentence exactly where it
/// was before this module existed, and a shape it matches wrongly deletes words the user should
/// have read.
pub(crate) fn denies_backgrounding(text: &str) -> bool {
    let words: Vec<String> = text
        .split_whitespace()
        .map(word_key)
        .filter(|w| !w.is_empty())
        .collect();
    let filler = |i: usize| words.get(i).is_some_and(|w| FILLER.contains(&w.as_str()));
    let denial = |i: usize| words.get(i).is_some_and(|w| DENIES.contains(&w.as_str()));
    for i in 0..words.len() {
        if denial(i) {
            for at in i + 1..=i + 1 + GAP {
                if mention_at(words.get(at..).unwrap_or_default()).is_some() {
                    return true;
                }
                if !filler(at) {
                    break;
                }
            }
        }
        if let Some(len) = mention_at(&words[i..]) {
            for at in i + len..=i + len + GAP {
                if denial(at) {
                    let names_the_alternative = words
                        .iter()
                        .skip(at + 1)
                        .take(GAP)
                        .any(|w| ALTERNATIVE.contains(&w.as_str()));
                    if !names_the_alternative {
                        return true;
                    }
                    break;
                }
                if !filler(at) {
                    break;
                }
            }
        }
    }
    false
}

/// Split a line into sentences, each keeping its own closing punctuation and the space after it,
/// so what is kept rejoins byte for byte. A full stop inside a word ("summary.csv") is not a
/// boundary: only one followed by whitespace or the end of the line is.
fn sentences(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let bytes = line.as_bytes();
    for (i, c) in line.char_indices() {
        if !matches!(c, '.' | '!' | '?') {
            continue;
        }
        let mut end = i + c.len_utf8();
        // Closing quotes, brackets and markdown emphasis belong to the sentence that ends here.
        while end < bytes.len() && matches!(bytes[end], b'"' | b'\'' | b')' | b']' | b'*' | b'_') {
            end += 1;
        }
        let rest = &line[end..];
        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
            let keep_to = end + (rest.len() - rest.trim_start().len());
            out.push(&line[start..keep_to]);
            start = keep_to;
        }
    }
    if start < line.len() {
        out.push(&line[start..]);
    }
    out
}

/// `text` without the sentences that deny the work was a background job.
///
/// A blank line is a paragraph break and is kept; a line whose every sentence denied is dropped
/// whole, and so is the gap it would have left behind. An answer that was nothing but denial
/// comes back empty, which the caller reads as "the model wrote nothing usable".
pub(crate) fn strip_denials(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            out.push(line.to_owned());
            continue;
        }
        let kept: String = sentences(line)
            .into_iter()
            .filter(|s| !denies_backgrounding(s))
            .collect();
        if kept.trim().is_empty() {
            continue;
        }
        out.push(kept.trim_end().to_owned());
    }
    let mut joined = out.join("\n");
    while joined.contains("\n\n\n") {
        joined = joined.replace("\n\n\n", "\n\n");
    }
    joined.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round 12's sentence, as the talking model wrote it.
    #[test]
    fn round_twelves_sentence_is_read_as_a_denial() {
        assert!(denies_backgrounding(
            "One thing to flag: the four steps ran in the foreground, not as a background job."
        ));
    }

    #[test]
    fn the_ways_an_answer_denies_its_own_backgrounding() {
        for s in [
            "This was not a background job.",
            "It did not run as a background job.",
            "The work wasn't backgrounded.",
            "I ran the steps in the foreground rather than as a background job.",
            "The four steps ran inline instead of as a background job.",
            "This never ran in the background.",
            "It ran without a background job.",
            "Acceptance: background job — unmet.",
            "The background job criterion was not met.",
            "That was not a background task.",
        ] {
            assert!(denies_backgrounding(s), "{s}");
        }
    }

    /// The other direction, which costs more when it is wrong: each of these carries both a
    /// denial word and a mention of backgrounding, and each is a sentence the user should read.
    #[test]
    fn a_true_sentence_about_the_background_survives() {
        for s in [
            "It is still running in the background.",
            "No problem — it ran in the background and the result is below.",
            "It didn't take long; it ran in the background the whole time.",
            "I did not run anything in the background besides this.",
            "The background job you asked about is finished.",
            "It ran in the background, not in the foreground.",
            "It ran in the background rather than the foreground.",
            "Nothing failed while it ran in the background.",
            "I could not reach six of the forty-five sites.",
            "1,200 rows are in summary.csv.",
        ] {
            assert!(!denies_backgrounding(s), "{s}");
        }
    }

    /// The denial goes; the sentences around it stay, with their own punctuation and spacing.
    #[test]
    fn only_the_denying_sentence_is_dropped() {
        let answer = "All four steps finished: 1,200 rows parsed, 1,196 normalised, and the summary table is written to summary.csv. One thing to flag: the four steps ran in the foreground, not as a background job. Want the per-step counts?";
        assert_eq!(
            strip_denials(answer),
            "All four steps finished: 1,200 rows parsed, 1,196 normalised, and the summary table is written to summary.csv. Want the per-step counts?"
        );
    }

    /// A table row or a bullet is its own line, and a paragraph that was nothing but denial
    /// leaves no blank gap where it stood. The bare foreground sentence staying is the second
    /// limit in this module's header, asserted so it is a decision and not an accident.
    #[test]
    fn a_paragraph_of_denial_leaves_no_hole() {
        let answer = "The archive is summarised in summary.csv.\n\nThis did not run as a background job. It ran in the foreground.\n\n| Step | Rows |\n| --- | --- |\n| parse | 1,200 |";
        assert_eq!(
            strip_denials(answer),
            "The archive is summarised in summary.csv.\n\nIt ran in the foreground.\n\n| Step | Rows |\n| --- | --- |\n| parse | 1,200 |"
        );
    }

    /// An answer that was only a denial comes back empty, so the caller posts the worker's own
    /// words instead of a blank bubble.
    #[test]
    fn an_answer_that_was_only_denial_comes_back_empty() {
        assert!(strip_denials("This was not a background job.")
            .trim()
            .is_empty());
        assert!(strip_denials("").is_empty());
    }

    /// Nothing is touched when nothing denies, byte for byte.
    #[test]
    fn an_answer_with_no_denial_is_unchanged() {
        let answer = "1,200 rows parsed; 1,196 normalised.\n\n| Step | Rows |\n| --- | --- |\n| parse | 1,200 |\n\nWant the rejects too?";
        assert_eq!(strip_denials(answer), answer);
    }
}
