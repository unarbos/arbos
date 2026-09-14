//! Link chips: a pull request, an agent, a document — as Cursor draws them
//! in prose, a small glyph and a short label (`⛓ #139`, `⚙ chat doors PR`,
//! `📄 SWE-bench loop`) instead of a raw URL or bracket text.
//!
//! Done on the markdown before it is parsed: a link's label is rewritten to
//! carry the glyph, a bare pull-request URL becomes a link with `#N` as its
//! label. The target is untouched, so a click still opens the same thing,
//! and code spans and fences are left as written.

/// A pull request.
pub const PR: &str = "⛓";
/// An agent — a chat, a worker.
pub const AGENT: &str = "⚙";
/// A document or file.
pub const DOC: &str = "📄";

/// What a link points at, for the glyph it gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pr,
    Agent,
    Doc,
    Other,
}

/// The glyph for a target, or none for a target that gets no chip.
pub fn glyph(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Pr => Some(PR),
        Kind::Agent => Some(AGENT),
        Kind::Doc => Some(DOC),
        Kind::Other => None,
    }
}

/// Classify a link target: a GitHub pull request, an agent (`agents/<id>`,
/// `.arbos/agents/<id>`, `arbos://chat/…`), a document (`.md`, `.txt`,
/// `docs/`), or anything else.
pub fn classify(target: &str) -> Kind {
    let t = target.trim();
    if pr_number(t).is_some() {
        return Kind::Pr;
    }
    let rel = t.strip_prefix("./").unwrap_or(t);
    let rel = rel.strip_prefix(".arbos/").unwrap_or(rel);
    if rel.starts_with("agents/")
        || t.starts_with("arbos://chat/")
        || t.starts_with("arbos://agent/")
    {
        return Kind::Agent;
    }
    let path = rel.split(['?', '#']).next().unwrap_or(rel);
    if path.ends_with(".md")
        || path.ends_with(".txt")
        || path.starts_with("docs/")
        || path.contains("/docs/")
    {
        return Kind::Doc;
    }
    Kind::Other
}

/// `#139` from a GitHub pull-request URL.
pub fn pr_number(url: &str) -> Option<u64> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() >= 4 && parts[2] == "pull" {
        return parts[3]
            .trim_end_matches(['.', ',', ';', ':', ')'])
            .parse::<u64>()
            .ok();
    }
    None
}

/// Whether a label already carries a chip glyph.
fn chipped(label: &str) -> bool {
    [PR, AGENT, DOC]
        .iter()
        .any(|g| label.trim_start().starts_with(g))
}

/// The label a link should show: the glyph, then the words — or `#N` for a
/// pull request whose label is the URL itself or empty.
pub fn label(kind: Kind, label: &str, target: &str) -> String {
    let words = label.trim();
    if chipped(words) {
        return words.to_string();
    }
    let Some(glyph) = glyph(kind) else {
        return words.to_string();
    };
    let words = match kind {
        Kind::Pr => {
            let generic = words.is_empty()
                || words.starts_with("http://")
                || words.starts_with("https://")
                || words == target;
            match (generic, pr_number(target)) {
                (true, Some(n)) => format!("#{n}"),
                _ => words.to_string(),
            }
        }
        Kind::Agent | Kind::Doc => {
            if words.is_empty() || words == target {
                target
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(target)
                    .trim_end_matches(".md")
                    .to_string()
            } else {
                words.to_string()
            }
        }
        Kind::Other => words.to_string(),
    };
    format!("{glyph} {words}")
}

/// Rewrite the links in markdown `text` to chips. Fenced code and inline
/// code spans are left alone.
pub fn dress(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut fenced = false;
    for (n, line) in text.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            out.push_str(line);
            continue;
        }
        if fenced {
            out.push_str(line);
            continue;
        }
        // Code spans stay as written; only the prose between them is dressed.
        let mut in_code = false;
        for (i, piece) in line.split('`').enumerate() {
            if i > 0 {
                out.push('`');
            }
            if in_code {
                out.push_str(piece);
            } else {
                out.push_str(&dress_prose(piece));
            }
            in_code = !in_code;
        }
    }
    out
}

/// One run of prose: markdown links get their chip label; bare pull-request
/// URLs become `[⛓ #N](url)`.
fn dress_prose(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut done = 0; // dressed up to here
    let mut scan = 0; // looking for `[` from here
    while let Some(open) = text[scan..].find('[').map(|i| scan + i) {
        // `![alt](img)` is an image, not a link; a `[` that opens no link
        // (a checkbox, a citation) is skipped over.
        if text[..open].ends_with('!') {
            scan = open + 1;
            continue;
        }
        let Some((label_end, target_end)) = link_bounds(&text[open..]) else {
            scan = open + 1;
            continue;
        };
        let label = &text[open + 1..open + label_end];
        let target = &text[open + label_end + 2..open + target_end];
        out.push_str(&dress_urls(&text[done..open]));
        out.push('[');
        out.push_str(&self::label(classify(target), label, target));
        out.push_str("](");
        out.push_str(target);
        out.push(')');
        done = open + target_end + 1;
        scan = done;
    }
    out.push_str(&dress_urls(&text[done..]));
    out
}

/// For `s` starting at `[`: the offset of the `]` that closes the label and
/// the offset of the `)` that closes the target, when it is a link.
fn link_bounds(s: &str) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    let mut depth = 0usize;
    let mut label_end = None;
    for (i, &b) in bytes.iter().enumerate().skip(1) {
        match b {
            b'[' => depth += 1,
            b']' if depth == 0 => {
                if bytes.get(i + 1) == Some(&b'(') {
                    label_end = Some(i);
                    break;
                }
                return None;
            }
            b']' => depth -= 1,
            b'\n' => return None,
            _ => {}
        }
    }
    let label_end = label_end?;
    let close = s[label_end + 2..].find(')')? + label_end + 2;
    // A target with a space in it is not a link, unless the space starts a
    // title: `[a](b "t")`.
    let target = &s[label_end + 2..close];
    if target.contains(char::is_whitespace) && !target.contains(" \"") {
        return None;
    }
    Some((label_end, close))
}

/// Bare pull-request URLs in a run of prose that holds no markdown links.
fn dress_urls(text: &str) -> String {
    if !text.contains("github.com/") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 8);
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let core = word.trim_end_matches(['.', ',', ';', ':', ')']);
        let tail = &word[core.len()..];
        match pr_number(core) {
            Some(n) if core.starts_with("http") => {
                out.push_str(&format!("[{PR} #{n}]({core}){tail}"));
            }
            _ => out.push_str(word),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_request_url_becomes_a_numbered_chip() {
        assert_eq!(
            dress("Fix PR https://github.com/unarbos/arbos/pull/139. Done."),
            "Fix PR [⛓ #139](https://github.com/unarbos/arbos/pull/139). Done."
        );
        assert_eq!(
            dress("see [the PR](https://github.com/o/r/pull/7) now"),
            "see [⛓ the PR](https://github.com/o/r/pull/7) now"
        );
        assert_eq!(
            dress("[https://github.com/o/r/pull/7](https://github.com/o/r/pull/7)"),
            "[⛓ #7](https://github.com/o/r/pull/7)"
        );
    }

    #[test]
    fn agents_and_documents_get_their_glyphs_and_code_is_left_alone() {
        assert_eq!(
            dress("- [ ] [chat doors](agents/chat-doors) — pending"),
            "- [ ] [⚙ chat doors](agents/chat-doors) — pending"
        );
        assert_eq!(
            dress("Living doc: [SWE-bench loop](docs/swebench-loop.md)."),
            "Living doc: [📄 SWE-bench loop](docs/swebench-loop.md)."
        );
        assert_eq!(
            dress("`https://github.com/o/r/pull/7` stays"),
            "`https://github.com/o/r/pull/7` stays"
        );
        assert_eq!(
            dress("```\nhttps://github.com/o/r/pull/7\n```"),
            "```\nhttps://github.com/o/r/pull/7\n```"
        );
        assert_eq!(
            dress("[⛓ #7](https://github.com/o/r/pull/7)"),
            "[⛓ #7](https://github.com/o/r/pull/7)"
        );
        assert_eq!(
            dress("[plain](https://example.com/x)"),
            "[plain](https://example.com/x)"
        );
    }
}
