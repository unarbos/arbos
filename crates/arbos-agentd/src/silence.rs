//! Silence is an outcome, not a message. In a shared chat an agent that decides not to answer
//! must post nothing (route `reply` with `reply` empty); a bubble that announces the silence
//! ("Addressed to Hezbola; staying silent. That question is for Hezbola; nothing to add from
//! me." — Arbonites, 2026-10-26) is chat noise. The gate and the prompt say so; this guard
//! enforces it: a post that only says "I am not answering" is dropped and logged.

/// Phrases that, on their own, announce silence or hand the question to someone else.
const SILENCE_PHRASES: &[&str] = &[
    "staying silent",
    "stay silent",
    "staying quiet",
    "stay quiet",
    "remain silent",
    "remaining silent",
    "keep quiet",
    "keeping quiet",
    "stay out of this",
    "staying out of this",
    "sit this one out",
    "sitting this one out",
    "sit this out",
    "nothing to add",
    "nothing more to add",
    "nothing further to add",
    "nothing from me",
    "no need for me to",
    "no reply needed from me",
    "no response needed from me",
    "no response from me",
    "not for me",
    "wasn't asked",
    "was not asked",
    "not addressed to me",
    "isn't addressed to me",
    "is not addressed to me",
    "addressed to",
    "that question is for",
    "this question is for",
    "that one is for",
    "this one is for",
    "that's for",
    "that is for",
    "this is for",
    "i'll leave this to",
    "i'll leave that to",
    "leaving this to",
    "leaving that to",
    "i'll let",
    "i will let",
    "i'll defer to",
    "deferring to",
    "defer to",
    "over to you",
    "over to",
    "your turn",
    "no comment from me",
    "not my question",
    "not my call",
    "not mine to answer",
    "i won't weigh in",
    "won't weigh in",
    "not weighing in",
    "no input from me",
    "i'll hold off",
    "holding off",
    "standing by",
    "i'll stand by",
    "listening only",
    "just listening",
    "noted, no action",
    "no action from me",
    "nothing needed from me",
    "nothing required from me",
];

/// Words that make a short line a real answer even if it also contains a silence phrase
/// ("Nothing to add — the build is green and deployed" says something).
const SUBSTANCE_HINTS: &[&str] = &[
    "http://", "https://", "```", "| ", "- ", "1.", "2.", "because", "since", "result", "error",
    "deployed", "merged", "fixed", "done", "ready", "file", "path", "/work/", "commit", "branch",
    "yes,", "no,", "yes.", "no.",
];

/// Maximum length of a post that can still be "only an announcement of silence". Longer posts
/// carry content.
const MAX_CHARS: usize = 400;

/// Does `text` only say that the agent is not answering (staying silent, deferring, "that is
/// for X", "nothing to add from me")? False for any post with substance.
pub fn is_silence_announcement(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.chars().count() > MAX_CHARS {
        return false;
    }
    let lower = t.to_lowercase();
    if !SILENCE_PHRASES.iter().any(|p| lower.contains(p)) {
        return false;
    }
    // A sentence with a link, code, list, reason or a yes/no answer is an answer.
    if SUBSTANCE_HINTS.iter().any(|h| lower.contains(h)) {
        return false;
    }
    // Every sentence must be about the silence itself (or be a bare name/handle fragment).
    let sentences: Vec<&str> = lower
        .split(['.', ';', '!', '?', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if sentences.is_empty() {
        return false;
    }
    sentences.iter().all(|s| {
        SILENCE_PHRASES.iter().any(|p| s.contains(p))
            || s.split_whitespace().count() <= 3
            || s.starts_with("that question")
            || s.starts_with("this question")
            || s.starts_with("that's a question for")
            || s.starts_with("that is a question for")
            || s.starts_with("i was only mentioned")
            || s.starts_with("i was just mentioned")
            || s.starts_with("i was mentioned")
            || s.starts_with("i'm only mentioned")
            || s.starts_with("only mentioned")
            || s.starts_with("mentioned as")
            || s.starts_with("not asked")
            || s.starts_with("i wasn't")
            || s.starts_with("no action")
            || s.starts_with("no reply")
            || s.starts_with("no response")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arbonites_post_is_dropped() {
        assert!(is_silence_announcement(
            "Addressed to @Hezbola; staying silent. That question is for Hezbola; nothing to add from me."
        ));
        assert!(is_silence_announcement(
            "Staying silent — that one's for Hezbola."
        ));
        assert!(is_silence_announcement("That question is for @hezbola."));
        assert!(is_silence_announcement("Nothing to add from me."));
        assert!(is_silence_announcement("I'll let Hezbola take this one."));
        assert!(is_silence_announcement("Not addressed to me; standing by."));
        assert!(is_silence_announcement(
            "I was only mentioned here, not asked. No reply needed from me."
        ));
    }

    #[test]
    fn real_answers_are_kept() {
        assert!(!is_silence_announcement(
            "Jacob, yes, I can reach Doug agent directly, and I've been doing it through this shared chat."
        ));
        assert!(!is_silence_announcement(
            "Nothing to add — the build is green and deployed, so we are done here."
        ));
        assert!(!is_silence_announcement(
            "No, Jacob — separate boxes. Hezbola's project folder is his own."
        ));
        assert!(!is_silence_announcement(
            "The render is at /work/01M5/out/frame120.png; that's for the deck."
        ));
        assert!(!is_silence_announcement("Yes."));
        assert!(!is_silence_announcement(""));
        assert!(!is_silence_announcement(
            "That question is for the ops team in the sense that the deploy runs on their box, but here is the answer anyway: the image is 0.1.150 and it was rolled at 14:02 UTC, see https://github.com/unarbos/arbos/actions/runs/1."
        ));
    }
}
