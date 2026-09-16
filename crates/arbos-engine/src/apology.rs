//! After a kernel nudge ("Your reply was empty…"), some models open the
//! next reply by apologising for the hiccup — "Sorry — empty reply on my
//! side, nothing blocking." — and the correction becomes the first words
//! the user reads (Jacob's phone, 2026-09-16: he answered "Why?"). The
//! nudge was for the model; the user saw neither the blank nor the note.
//! A leading sentence that talks about the empty reply or the nudge is
//! cut; what follows is the answer.

/// Words a sentence about the model's own hiccup uses. The sentence has
/// to open like an apology *and* name the hiccup: "Sorry, the tests fail"
/// is about the user's request and stays.
const OPENERS: &[&str] = &[
    "sorry",
    "apolog",
    "my apologies",
    "oops",
    "pardon",
    "excuse",
    "it seems my",
    "it looks like my",
    "my previous",
    "my last",
    "that was an empty",
    "that was a blank",
];

const ABOUT_THE_HICCUP: &[&str] = &[
    "empty reply",
    "empty response",
    "empty message",
    "empty step",
    "empty turn",
    "blank reply",
    "blank response",
    "blank message",
    "reply was empty",
    "response was empty",
    "message was empty",
    "came out empty",
    "sent nothing",
    "returned nothing",
    "hiccup",
    "glitch on my",
    "nothing blocking",
    "the nudge",
    "kernel note",
    "kernel's note",
];

/// `content` without a leading apology about the model's own empty reply.
/// Only the first sentence or the first short paragraph is looked at;
/// anything after it is kept as written. Returns the input unchanged when
/// the opening is about something else.
pub fn strip_empty_reply_apology(content: &str) -> String {
    let trimmed = content.trim_start();
    let first_para_end = trimmed.find("\n\n").unwrap_or(trimmed.len());
    let first_para = &trimmed[..first_para_end];
    // The unit cut: the first sentence, or the whole first paragraph when
    // it is one short line.
    let sentence_end = first_para
        .char_indices()
        .find(|(i, c)| {
            matches!(c, '.' | '!' | '?')
                && first_para[i + c.len_utf8()..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
        })
        .map(|(i, c)| i + c.len_utf8());
    let unit_end = match sentence_end {
        Some(e) if e < first_para.len() => e,
        _ => first_para_end,
    };
    let unit = &trimmed[..unit_end];
    if unit.chars().count() > 240 {
        return content.to_string();
    }
    let lower = unit.to_ascii_lowercase();
    let lead: String = lower
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .chars()
        .take(40)
        .collect();
    let opens = OPENERS.iter().any(|o| lead.starts_with(o));
    let about = ABOUT_THE_HICCUP.iter().any(|w| lower.contains(w));
    if !(opens && about) {
        return content.to_string();
    }
    trimmed[unit_end..]
        .trim_start_matches(['\n', ' ', '\t'])
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::strip_empty_reply_apology as strip;

    #[test]
    fn a_leading_apology_about_the_empty_reply_goes_and_the_answer_stays() {
        assert_eq!(
            strip(
                "Sorry — empty reply on my side, nothing blocking. The build passes on both targets."
            ),
            "The build passes on both targets."
        );
        assert_eq!(
            strip("Apologies for the blank response!\n\nHere is the summary:\n- one\n- two"),
            "Here is the summary:\n- one\n- two"
        );
        assert_eq!(
            strip("My previous message came out empty. The tests pass."),
            "The tests pass."
        );
        assert_eq!(
            strip("Oops, that was an empty step on my part. The file is at docs/a.md."),
            "The file is at docs/a.md."
        );
    }

    #[test]
    fn an_apology_about_the_users_request_stays() {
        let s = "Sorry, the tests still fail on main. Two cases assert the old rounding.";
        assert_eq!(strip(s), s);
        let s = "Sorry for the delay — the build took eight minutes.";
        assert_eq!(strip(s), s);
        let s = "The reply was empty because the file has no content. Nothing to do.";
        assert_eq!(strip(s), s, "not an apology opener");
        let s = "Done.";
        assert_eq!(strip(s), s);
        assert_eq!(strip(""), "");
    }

    #[test]
    fn a_long_opening_is_left_alone() {
        let long = format!("Sorry about the empty reply; {}.", "x".repeat(300));
        assert_eq!(strip(&long), long);
    }
}
