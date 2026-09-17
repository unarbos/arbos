//! Bounds and output contract for chat titles.
//!
//! A single line of a few words, no label or wrapping quotes. A model
//! title is taken as given (bounded at four words). A first prompt is cut
//! at its first clause and never ends on a function word: "This project is
//! a research notebook about…" labels the chat "This project is a research
//! notebook", not "This project is a" (F-156).

const MAX_WORDS: usize = 4;
const PROMPT_MAX_WORDS: usize = 6;
const PROMPT_MAX_CHARS: usize = 40;

/// Words a label must not end on.
const TRAILING_STOP: &[&str] = &[
    "a", "an", "the", "of", "to", "in", "on", "at", "by", "for", "with", "and", "or", "is", "are",
    "be", "was", "were", "as", "that", "this", "it", "its", "into", "from", "about", "my", "our",
    "your",
];

/// Turn a model title into a sidebar label.
pub fn normalize(title: &str) -> Option<String> {
    bound(title, MAX_WORDS)
}

fn bound(title: &str, max_words: usize) -> Option<String> {
    let mut title = title.trim();
    // `get`, not a slice: byte 6 may fall inside a multibyte char.
    if title
        .get(..6)
        .is_some_and(|p| p.eq_ignore_ascii_case("title:"))
    {
        title = title[6..].trim();
    }
    let trimmed: String = title
        .trim_matches(|c: char| {
            matches!(
                c,
                ' ' | '\t' | '\r' | '\n' | '"' | '\'' | '`' | '*' | '_' | '“' | '”' | '‘' | '’'
            )
        })
        .to_string();
    let mut words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.len() > max_words {
        words.truncate(max_words);
    }
    let mut title = words.join(" ");
    while title
        .chars()
        .last()
        .is_some_and(|c| matches!(c, '.' | '!' | '?' | ';' | ':'))
    {
        title.pop();
    }
    let title = title.trim().to_string();
    (!title.is_empty()).then_some(title)
}

/// Sidebar title from the first user message. First non-blank line only,
/// then [`normalize`].
pub fn from_prompt(prompt: &str) -> Option<String> {
    let line = prompt.lines().map(str::trim).find(|l| !l.is_empty())?;
    // A slash command (`/mode haiku`, `/review`) is a setting or a skill
    // call, not what the chat is about.
    if line.starts_with('/') {
        return None;
    }
    let clause = first_clause(line);
    let mut words: Vec<&str> = clause.split_whitespace().collect();
    words.truncate(PROMPT_MAX_WORDS);
    while words.len() > 1 && words.join(" ").chars().count() > PROMPT_MAX_CHARS {
        words.pop();
    }
    while words.len() > 1
        && words
            .last()
            .is_some_and(|w| TRAILING_STOP.contains(&stem(w).as_str()))
    {
        words.pop();
    }
    let cut = words.join(" ");
    bound(&cut, PROMPT_MAX_WORDS).or_else(|| bound(line, MAX_WORDS))
}

/// The text before the first clause break, when what is before it is more
/// than one word; else the whole line.
fn first_clause(line: &str) -> &str {
    let at = line
        .char_indices()
        .find(|(i, c)| {
            matches!(c, ':' | ';' | '—' | '–' | '(' | '?' | '!')
                || (*c == '.' && line[i + 1..].starts_with(char::is_whitespace))
                || (*c == ',' && line[i + 1..].starts_with(char::is_whitespace))
                || (*c == '-'
                    && *i > 0
                    && line[..*i].ends_with(' ')
                    && line[i + 1..].starts_with(' '))
        })
        .map(|(i, _)| i);
    match at {
        Some(i) if line[..i].split_whitespace().count() >= 2 => &line[..i],
        _ => line,
    }
}

fn stem(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase()
}

/// Folder id, agent identity, or the project name — not a chat title.
pub fn is_generic(label: &str, session_id: Option<&str>) -> bool {
    let n = label.trim();
    if n.is_empty() {
        return true;
    }
    if session_id.is_some_and(|id| n == id) {
        return true;
    }
    n.eq_ignore_ascii_case("root")
        || n.eq_ignore_ascii_case("arbos")
        || n.eq_ignore_ascii_case("chat")
        || n.eq_ignore_ascii_case("new chat")
        || n.to_ascii_lowercase().starts_with("new chat ")
}

#[cfg(test)]
mod slash_tests {
    use super::*;

    #[test]
    fn a_slash_command_names_no_chat() {
        assert_eq!(from_prompt("/mode haiku"), None);
        assert_eq!(from_prompt("  /review the diff"), None);
        assert!(from_prompt("Fix the login bug").is_some());
    }

    #[test]
    fn a_prompt_title_never_ends_on_a_function_word() {
        assert_eq!(
            from_prompt("This project is a research notebook about container image formats (OCI).")
                .as_deref(),
            Some("This project is a research notebook")
        );
        assert_eq!(
            from_prompt("Use two workers: one writes docs/oci-layout.md").as_deref(),
            Some("Use two workers")
        );
        assert_eq!(
            from_prompt("Add a Decisions section to the project page with one decision").as_deref(),
            Some("Add a Decisions section")
        );
        assert_eq!(
            from_prompt("write and run bubble sort").as_deref(),
            Some("write and run bubble sort")
        );
        assert_eq!(from_prompt("Fix the").as_deref(), Some("Fix"));
        assert_eq!(
            normalize("Restructure notes into five sections now").as_deref(),
            Some("Restructure notes into five")
        );
    }
}
