//! A user line that corrects the agent or states a way of working — the
//! feedback the Projects post says a project learns from ("with each
//! turn of feedback, the Project learns your architecture and
//! preferences"). The kernel checks that such a turn saved something
//! (`remember`, or an edit to the context file) and reminds once when it
//! did not. Heuristic, short, and biased to the plain spellings.

/// Openers and phrases that read as a correction or a standing
/// preference. Matched on the lower-cased text.
const OPENERS: &[&str] = &[
    "no,",
    "no.",
    "no ",
    "nope",
    "not ",
    "don't ",
    "do not ",
    "never ",
    "wrong",
    "that's not",
    "that is not",
    "thats not",
    "i said",
    "i asked",
    "i told you",
    "i meant",
    "instead",
    "actually,",
    "actually ",
    "please don't",
    "please do not",
    "please stop",
];

/// Phrases anywhere in the line that read as a correction.
const CORRECTIVE: &[&str] = &[
    "not like that",
    "not that,",
    "that's wrong",
    "that is wrong",
    "you got it wrong",
    "you misunderstood",
    "don't do that",
    "do not do that",
    "stop doing",
    "rather than",
    "not what i asked",
    "not what i meant",
];

/// Phrases that state a standing way of working.
const STANDING: &[&str] = &[
    "from now on",
    "in future",
    "in the future",
    "going forward",
    "always ",
    "never ",
    "i prefer",
    "i'd prefer",
    "i would prefer",
    "my preference",
    "remember that",
    "remember:",
    "rule:",
];

/// Whether a user line reads as a correction or a stated preference. A
/// long message (a task brief that happens to say "always") is not one:
/// feedback is short.
pub fn reads_as_correction(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.chars().count() > 400 {
        return false;
    }
    let lower = t.to_lowercase();
    if OPENERS.iter().any(|o| lower.starts_with(o)) {
        // "no" as an answer to a yes/no question is not a correction.
        return !(lower.starts_with("no") && lower.trim_end_matches(['.', '!']).len() <= 3);
    }
    STANDING.iter().chain(CORRECTIVE).any(|p| lower.contains(p))
}

#[cfg(test)]
mod tests {
    use super::reads_as_correction;

    #[test]
    fn corrections_and_preferences_read_as_such_and_tasks_do_not() {
        for line in [
            "No, use the staging branch, not main.",
            "Don't touch the tests.",
            "Actually, I meant the other file.",
            "From now on, open PRs as drafts.",
            "I prefer short answers.",
            "Always run the linter before you commit.",
            "That's not what I asked — I asked for a summary.",
            "Never merge without asking me.",
            "Remember that the API lives in services/, not lib/.",
            "You misunderstood: it should be the second table.",
        ] {
            assert!(reads_as_correction(line), "{line}");
        }
        for line in [
            "Add a login page with email and password.",
            "What tests exist in this repo?",
            "no",
            "No.",
            "Run the tests and show me the output.",
            "Use the design in docs/plan.md to build the settings screen with all six sections, then open a PR.",
            &"Always ".repeat(80),
        ] {
            assert!(!reads_as_correction(line), "{line}");
        }
    }
}
