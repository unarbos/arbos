//! Bounds and output contract for chat titles.
//!
//! Same rules as Go `internal/chattitle`: a single line of at most four
//! words, no label or wrapping quotes.

const MAX_WORDS: usize = 4;

/// Turn a model title or a first user prompt into a sidebar label.
pub fn normalize(title: &str) -> Option<String> {
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
    if words.len() > MAX_WORDS {
        words.truncate(MAX_WORDS);
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
    normalize(line)
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
