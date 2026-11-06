//! The forced-job signal: a message whose own words say the work must run as a background job
//! and not inline.
//!
//! Until now nothing in agentd read that instruction. It reached the main session's model as
//! plain prose, the model obeyed it through [`crate::agent::Agent::hook_start_job`], and the
//! instruction therefore only held for a message the main session answered as its own ask.
//!
//! Jacob, Test5 round 6 (2026-11-05, agentd 0.1.193, scenario `r6lock`), bug U. Four messages
//! two seconds apart, each ending "as a background job (not inline)": `itsdangerous`,
//! `markupsafe`, `pluggy`, `more-itertools`. Two became jobs. The other two arrived while a
//! turn held the lock, took the quick lane (`status.rs`), came back from the quick call with a
//! line that could not stand — the log carries "quick reply did not end a sentence; silent
//! queue" — and were handed to `start_job_for_send`, which answers an ask with a plain
//! subagent. That subagent writes its answer straight into the chat: no job row, no job id,
//! nothing `list_jobs`, `steer_job` or `stop_job` can name. The user asked four times for a job
//! and got two.
//!
//! So the signal is read here, in code, and the two paths that could swallow it now honour it.

/// A word as it is matched: lower case, letters, digits and `_` only. Everything else
/// (brackets, commas, full stops) becomes nothing, so "(not" matches "not" and "job," matches
/// "job".
///
/// "an" is matched as "a". Every phrase in [`FORCES`] spells the article "a", and which of the
/// two a person writes is decided by the word after it — so "as **an** additional background
/// task" is the same order as "as a background task", written around the word
/// [`COUNTING_WORDS`] takes out.
fn word_key(w: &str) -> String {
    let key: String = w
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .flat_map(|c| c.to_lowercase())
        .collect();
    match key.as_str() {
        "an" => "a".to_owned(),
        _ => key,
    }
}

/// Words that only count the instance of an order and do not change the order itself: "as a
/// **second** background job" asks for a background job exactly as "as a background job" does.
///
/// Every phrase in [`FORCES`] is a run of adjacent words, so one word dropped into the middle of
/// one breaks the whole match. Jacob, Test5 round 10 (2026-11-05, agentd 0.1.195, scenario
/// `r10status`), bug AA: "As a second background job …" and "As a third background job …"
/// matched nothing, so both ran as quick-lane subagents — no job row, no job id, nothing
/// `list_jobs`, `steer_job` or `stop_job` could name. "Cancel the longest" then had nothing to
/// cancel, which is what the user saw.
///
/// Elided before matching rather than written into [`FORCES`] as more phrases, because they
/// combine: a phrase list would need every count against every cue. Elision is also why
/// "start a new job" and "as a separate job" are read as the orders they plainly are.
///
/// The counts proper are not here. They are derived by [`is_ordinal`], for the reason below.
const COUNTING_WORDS: &[&str] = &[
    "another",
    "separate",
    "additional",
    "extra",
    "further",
    "new",
    "parallel",
    "concurrent",
];

/// Ordinals whose stem changes shape before the ending, so no rule generates them. Seven words,
/// and that is all of them in English — a closed set, unlike the ordinals themselves.
const IRREGULAR_ORDINALS: &[&str] = &[
    "first", "second", "third", "fifth", "eighth", "ninth", "twelfth",
];

/// Cardinals that take a plain `th`: "four" → "fourth", "nineteen" → "nineteenth", "hundred" →
/// "hundredth". The stems rather than the ordinals, because the ordinals are what is derived.
const PLAIN_TH_CARDINALS: &[&str] = &[
    "four",
    "six",
    "seven",
    "ten",
    "eleven",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "hundred",
    "thousand",
    "million",
];

/// Tens, which form their ordinal by `y` → `ieth` ("twenty" → "twentieth") and their compounds
/// by prefixing a unit ordinal ("twenty-first").
const TENS: &[&str] = &[
    "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

/// Is this word key an ordinal — "second", "seventh", "twelfth", "twenty-third", "40th"?
///
/// Derived rather than listed, which is the whole point of this function. Bug AA was fixed in
/// `r10status` by writing "second" through "sixth" into [`COUNTING_WORDS`], and that list is
/// why it came back: Jacob's `r17status` asked for eight jobs, and the **seventh** and
/// **eighth** — "As a seventh, separate background job …" — matched nothing and ran as
/// quick-lane subagents with no job row, exactly as the second and third had a round earlier.
/// Every phrase in [`FORCES`] is a run of adjacent words, so one unlisted word in the middle of
/// one breaks the whole match.
///
/// A list of ordinals has no end to reach, so extending it to "eighth" would only have moved
/// the cliff to the ninth job. The rule has no cliff: English builds every ordinal above twelve
/// out of a stem and an ending, so the stems are listed and the ordinals are generated.
fn is_ordinal(key: &str) -> bool {
    if IRREGULAR_ORDINALS.contains(&key) {
        return true;
    }
    // "40th", "21st", "2nd". The suffix is not checked for agreement with the digits: a person
    // who writes "21th" means the twenty-first, and reading them is the job here.
    if ["st", "nd", "rd", "th"].iter().any(|end| {
        key.strip_suffix(end)
            .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
    }) {
        return true;
    }
    // "twentieth". Bare "twenty" is the cardinal and is left alone.
    if key
        .strip_suffix("ieth")
        .is_some_and(|stem| TENS.iter().any(|t| t.strip_suffix('y') == Some(stem)))
    {
        return true;
    }
    // "twenty-third", which [`word_key`] has already joined into "twentythird". One level of
    // recursion only: no tens word begins with another, so the remainder carries no tens.
    if let Some(unit) = TENS.iter().find_map(|t| key.strip_prefix(t)) {
        return is_ordinal(unit);
    }
    key.strip_suffix("th")
        .is_some_and(|stem| PLAIN_TH_CARDINALS.contains(&stem))
}

/// Is this word key one that counts an order rather than changing it, and is therefore elided?
fn counts_only(key: &str) -> bool {
    COUNTING_WORDS.contains(&key) || is_ordinal(key)
}

/// The word keys of a message, with the empty ones and the counting words dropped.
fn keys(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(word_key)
        .filter(|w| !w.is_empty() && !counts_only(w))
        .collect()
}

/// The word keys as a single space-padded line, so a phrase is matched on whole words:
/// `" install itsdangerous as a background job not inline "`.
fn matchable(keys: &[String]) -> String {
    format!(" {} ", keys.join(" "))
}

/// Phrases that name background-job execution as the instruction. Each one is an order about
/// *this* request, never a noun on its own: "background job" alone is how a person asks after
/// work that already runs, so it is not here.
const FORCES: &[&str] = &[
    "as a background job",
    "as a background task",
    "as background job",
    "as a job",
    "run it in the background",
    "run this in the background",
    "run that in the background",
    "run them in the background",
    "do it in the background",
    "do this in the background",
    "do that in the background",
    "do them in the background",
    "make it a job",
    "make this a job",
    "make that a job",
    "start a job",
    "start a background job",
    "kick it off as a job",
    "start_job",
    "not inline",
    "no inline",
    "instead of inline",
    "dont do it inline",
    "do not do it inline",
    "dont run it inline",
    "do not run it inline",
    "dont run this inline",
    "do not run this inline",
];

/// Phrases that make the message one *about* work already in flight — a question or a stop —
/// rather than a request for new work. A message that carries one of these forces nothing, so
/// the quick lane keeps answering "how is the background job going?" as the status question it
/// is.
const ABOUT_WORK_ALREADY_RUNNING: &[&str] = &[
    "stop the",
    "stop that",
    "stop it",
    "stop them",
    "cancel the",
    "cancel that",
    "cancel it",
    "kill the",
    "how is",
    "hows",
    "how are",
    "how far",
    "status of",
    "is it still",
    "is that still",
    "are they still",
    "did you",
    "what happened",
    "what jobs",
    "which job",
    "any jobs",
    "list the jobs",
];

/// Does this message order the work into a background job?
///
/// Deliberately narrow in both directions. A phrase it misses leaves the message exactly where
/// it was before this module existed, which is the old behaviour and no worse; a phrase it
/// matches wrongly starts work the user did not ask to have backgrounded, which is worse. So a
/// cue must read as an order about this request, and a message that asks after or stops running
/// work is excluded outright.
///
/// One limit is worth naming rather than hiding: "run that in the background **instead**" is a
/// correction of work already running, and the words for it are the words for a fresh order.
/// "instead" is read as a correction unless the message also says "inline" — which is how the
/// forced shape writes it ("as a background job instead of inline"), and which no correction
/// says.
///
/// A message that *opens* with a cue is an order, and [`ABOUT_WORK_ALREADY_RUNNING`] does not
/// apply to it. Bug AA, the other half of `r10status`: "As a second background job, give me the
/// status of the folder" carries "status of" and was therefore read as a question about work
/// already running, so the clearest instruction in the message — its first four words — lost to
/// three words in the middle of it. Nothing on that list reads as an order when it comes first;
/// "run that in the background instead" does open with a cue, and the "instead" rule above is
/// what holds it, which is why that rule is now asked first.
pub(crate) fn forces_a_job(text: &str) -> bool {
    let keys = keys(text);
    let line = matchable(&keys);
    if line.contains(" instead ") && !line.contains(" inline ") {
        return false;
    }
    if cue_words_at(&keys).is_none()
        && ABOUT_WORK_ALREADY_RUNNING
            .iter()
            .any(|p| line.contains(&format!(" {p} ")))
    {
        return false;
    }
    FORCES.iter().any(|p| line.contains(&format!(" {p} ")))
}

/// Words a title keeps, so a trailing bracket or comma is dropped but `more-itertools` and
/// `v1.2` stay whole.
fn title_word(w: &str) -> &str {
    w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
}

/// Words of the title, and the characters it fits in. `start_job` refuses a title over 80
/// characters, and the title is a status line a person reads, not a summary.
const TITLE_WORDS: usize = 6;
const TITLE_CHARS: usize = 72;

/// The status line for a forced job, in the user's own words: what they asked for, up to the
/// phrase that forced it. "Install itsdangerous and report its version, as a background job
/// (not inline)." becomes "Install itsdangerous and report its version".
///
/// When the forcing phrase opens the message ("As a job, install pluggy") there is nothing
/// before it, so the words after it are the title instead. A message that is nothing but the
/// instruction keeps a plain fallback.
pub(crate) fn title_for(text: &str) -> String {
    let raw: Vec<&str> = text.split_whitespace().collect();
    // Where each matched word sits among the raw ones, so a cue found here maps back to the words
    // the user wrote. The counting words `forces_a_job` elides are not matched words, and the gap
    // they leave is why the cue's span is read off this rather than counted in cue words.
    let at_raw: Vec<usize> = raw
        .iter()
        .enumerate()
        .filter(|(_, w)| {
            let k = word_key(w);
            !k.is_empty() && !counts_only(&k)
        })
        .map(|(i, _)| i)
        .collect();
    let keys: Vec<String> = at_raw.iter().map(|&i| word_key(raw[i])).collect();
    let at = (0..keys.len()).find_map(|i| cue_words_at(&keys[i..]).map(|n| (i, n)));
    let kept: &[&str] = match at {
        None => &raw,
        Some((0, n)) => raw.get(at_raw[n - 1] + 1..).unwrap_or(&[]),
        Some((i, _)) => &raw[..at_raw[i]],
    };
    let mut title = String::new();
    for w in kept.iter().map(|w| title_word(w)).filter(|w| !w.is_empty()) {
        if !title.is_empty() {
            if title.chars().count() + 1 + w.chars().count() > TITLE_CHARS {
                break;
            }
            title.push(' ');
        }
        title.push_str(w);
        if title.split_whitespace().count() >= TITLE_WORDS {
            break;
        }
    }
    let title: String = title.chars().take(TITLE_CHARS).collect();
    if title.trim().is_empty() {
        "Running this in the background".to_owned()
    } else {
        title
    }
}

/// How many words the cue phrase starting at the first of these word keys takes, when one does.
fn cue_words_at(words: &[String]) -> Option<usize> {
    FORCES.iter().find_map(|cue| {
        let cue_words: Vec<&str> = cue.split(' ').collect();
        (words.len() >= cue_words.len()
            && cue_words
                .iter()
                .zip(words.iter())
                .all(|(c, w)| *c == w.as_str()))
        .then_some(cue_words.len())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four Test5 round 6 messages, as Jacob sent them.
    #[test]
    fn the_round_six_burst_forces_a_job_every_time() {
        for pkg in ["itsdangerous", "markupsafe", "pluggy", "more-itertools"] {
            let ask =
                format!("Install {pkg} and report its version, as a background job (not inline).");
            assert!(forces_a_job(&ask), "{ask}");
        }
    }

    #[test]
    fn an_order_to_background_the_work_is_read_as_one() {
        for ask in [
            "Run the full test suite as a job",
            "run it in the background please",
            "Compare the three hosts — as a background task.",
            "Build the report. Not inline.",
            "make this a job and tell me when it lands",
            "Index the archive as a background job instead of inline",
            "As a job, install pluggy and report its version",
        ] {
            assert!(forces_a_job(ask), "{ask}");
        }
    }

    /// Bug AA's two misses, in Jacob's words. A count inside the phrase does not change the
    /// order: the second and third background job he asked for are background jobs.
    #[test]
    fn a_counted_background_job_is_still_a_background_job() {
        for ask in [
            "As a second background job, give me the status of the folder",
            "As a third background job, count the lines in that log",
            "as a 2nd background job, check the disk",
            "Run the probe again as another background job",
            "Index the archive as a separate job",
            "start a new job for the second host",
            "Compare them as an additional background task",
        ] {
            assert!(forces_a_job(ask), "{ask}");
        }
    }

    /// Bug AA's return, in Jacob's `r17status` words. He asked for eight background jobs in one
    /// burst; the seventh and eighth matched nothing, because the list of counts stopped at
    /// "sixth". Round 10 fixed the second and third by writing them down, which is precisely
    /// why the seventh and eighth broke.
    #[test]
    fn the_round_seventeen_asks_past_the_end_of_the_old_list_force_a_job() {
        for ask in [
            "As a seventh, separate background job, count the files under /etc",
            "As an eighth, separate background job, report the kernel version",
        ] {
            assert!(forces_a_job(ask), "{ask}");
        }
    }

    /// The point of deriving rather than listing: an ordinal nobody would have written down
    /// still reads as the order it is. If these needed a list entry each, the rule is not a
    /// rule and the next burst finds the new cliff wherever the list now ends.
    #[test]
    fn an_ordinal_no_list_would_have_reached_is_still_only_a_count() {
        for ask in [
            "As a twelfth background job, diff the two trees",
            "As a twentieth background job, check the disk",
            "As a twenty-third background job, tail that log",
            "As a fortieth background job, list the mounts",
            "as a 40th background job, count the lines",
            "As a hundredth background job, say hello",
        ] {
            assert!(forces_a_job(ask), "{ask}");
        }
    }

    /// Derivation earns its keep by generating, so it must not generate what it should not. A
    /// word is a count because it is an ordinal, not because it ends in the same two letters.
    #[test]
    fn a_word_that_merely_ends_in_th_is_not_a_count() {
        for word in [
            "worth",
            "month",
            "both",
            "north",
            "health",
            "growth",
            "depth",
            "path",
            "faith",
            "truth",
            "length",
            "bandwidth",
            "twenty",
            "nine",
        ] {
            assert!(
                !counts_only(word),
                "{word} is not a count and must not be elided"
            );
        }
    }

    /// The elision must not swallow a word the match needs. "Fourth" leaving a sentence is only
    /// safe while the sentence that remains says the same thing.
    #[test]
    fn eliding_a_count_does_not_invent_an_order_that_was_not_given() {
        for ask in [
            "the fourth test is failing, have a look",
            "what is the seventh line of that file",
            "read the twelfth paragraph and tell me what it says",
            "the first background job finished, what did it say",
        ] {
            assert!(!forces_a_job(ask), "{ask}");
        }
    }

    /// The count is elided, not the question. A message *about* running work still forces
    /// nothing when it carries a count — "cancel the second job" must stay a stop.
    #[test]
    fn a_counted_question_about_running_work_still_forces_nothing() {
        for ask in [
            "cancel the second background job",
            "how is the third background job going?",
            "stop the new job you started",
            "what jobs are running as a second job right now",
        ] {
            assert!(!forces_a_job(ask), "{ask}");
        }
    }

    /// The title of a counted ask is the ask, with the whole instruction — count included —
    /// gone. A title that kept "second" would read as the status line of the wrong work.
    #[test]
    fn the_title_of_a_counted_ask_drops_the_count_with_the_cue() {
        assert_eq!(
            title_for("As a second background job, check the folder"),
            "check the folder"
        );
        assert_eq!(
            title_for("Count the lines in that log, as a third background job"),
            "Count the lines in that log"
        );
    }

    /// An order that opens the message wins over a phrase in the middle of it that reads as a
    /// question about running work.
    ///
    /// The last case is the limit this rule leaves, asserted so it is a decision and not an
    /// accident: a cue in the *tail* of a message that also reads as a question still forces
    /// nothing. Widening it would make "what jobs are running as a job right now" start work,
    /// and a wrong match costs more here than a missed one — a missed one leaves the message
    /// exactly where it was before this module existed.
    #[test]
    fn an_opening_order_outranks_a_question_phrase_inside_it() {
        assert!(forces_a_job("As a job, give me the status of the disk"));
        assert!(forces_a_job("Start a job to list the jobs on host two"));
        assert!(!forces_a_job("give me the status of the disk as a job"));
    }

    /// A message about work already running is not an order to start more of it. Each of these
    /// carries a cue phrase and must still force nothing.
    #[test]
    fn a_question_or_a_stop_about_running_work_forces_nothing() {
        for ask in [
            "how is the background job going?",
            "How far along is it? Did you run it in the background?",
            "stop the job you started in the background",
            "cancel that background job",
            "what jobs are running as a job right now",
            "run that in the background instead",
        ] {
            assert!(!forces_a_job(ask), "{ask}");
        }
    }

    #[test]
    fn ordinary_messages_force_nothing() {
        for ask in [
            "what is going on now?",
            "Install itsdangerous and report its version.",
            "the job finished, thanks",
            "I like working in the background on my own machine",
            "",
        ] {
            assert!(!forces_a_job(ask), "{ask}");
        }
    }

    #[test]
    fn the_title_is_the_ask_without_the_instruction() {
        assert_eq!(
            title_for(
                "Install itsdangerous and report its version, as a background job (not inline)."
            ),
            "Install itsdangerous and report its version"
        );
        assert_eq!(
            title_for("As a job, install pluggy and report its version"),
            "install pluggy and report its version"
        );
        assert_eq!(title_for("not inline"), "Running this in the background");
        assert_eq!(title_for(""), "Running this in the background");
    }

    /// `start_job` refuses a title over 80 characters, so no message may produce one.
    #[test]
    fn a_long_ask_still_fits_the_title_limit() {
        let ask = format!("Install {} as a background job", "a".repeat(400));
        let title = title_for(&ask);
        assert!(title.chars().count() <= TITLE_CHARS, "{title}");
        assert!(title.chars().count() <= 80);
        assert!(!title.trim().is_empty());
    }
}
