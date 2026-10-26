//! A small markdown block splitter. One transcript row = one block
//! (zeron-study §4.1). Inline: `code`, **bold**, *italic*. Enough for agent
//! replies; swap for pulldown-cmark when tables and links are needed.

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Block {
    Paragraph(String),
    Heading {
        level: u8,
        text: String,
    },
    Bullets(Vec<String>),
    Numbered(Vec<String>),
    Code {
        lang: String,
        code: String,
    },
    /// GitHub-style pipe table: header row, then body rows.
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

pub fn split_blocks(source: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut lines = source.lines().peekable();
    let mut paragraph: Vec<&str> = Vec::new();

    fn flush(paragraph: &mut Vec<&str>, blocks: &mut Vec<Block>) {
        if !paragraph.is_empty() {
            // Keep soft newlines (chat-style): a single Enter is a line break,
            // not a space. Blank lines still split paragraphs above.
            blocks.push(Block::Paragraph(paragraph.join("\n")));
            paragraph.clear();
        }
    }

    while let Some(line) = lines.next() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            flush(&mut paragraph, &mut blocks);
            continue;
        }
        // Leading spaces are fine (iOS parity; CommonMark allows a short indent).
        if let Some(rest) = trimmed.trim_start().strip_prefix("```") {
            flush(&mut paragraph, &mut blocks);
            let lang = rest.trim().to_string();
            let mut code = String::new();
            for code_line in lines.by_ref() {
                if code_line.trim_end().trim_start().starts_with("```") {
                    break;
                }
                if !code.is_empty() {
                    code.push('\n');
                }
                code.push_str(code_line);
            }
            blocks.push(Block::Code { lang, code });
            continue;
        }
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count().min(6) as u8;
            let text = trimmed[level as usize..].trim().to_string();
            if !text.is_empty() {
                flush(&mut paragraph, &mut blocks);
                blocks.push(Block::Heading { level, text });
                continue;
            }
        }
        if is_table_row(trimmed) {
            if let Some(next) = lines.peek() {
                if is_table_separator(next.trim_end()) {
                    flush(&mut paragraph, &mut blocks);
                    let header = split_table_row(trimmed);
                    lines.next();
                    let mut rows = Vec::new();
                    while let Some(row) = lines.peek() {
                        let row = row.trim_end();
                        if !is_table_row(row) {
                            break;
                        }
                        rows.push(split_table_row(row));
                        lines.next();
                    }
                    blocks.push(Block::Table { header, rows });
                    continue;
                }
                // Pipe rows with no separator row are not a GFM table. Rather
                // than paint them as prose with raw `|`, they degrade to one
                // monospace block that keeps its columns and scrolls sideways.
                if is_table_row(next.trim_end()) {
                    flush(&mut paragraph, &mut blocks);
                    let mut code = trimmed.trim().to_string();
                    while let Some(row) = lines.peek() {
                        let row = row.trim_end();
                        if !is_table_row(row) {
                            break;
                        }
                        code.push('\n');
                        code.push_str(row.trim());
                        lines.next();
                    }
                    blocks.push(Block::Code {
                        lang: String::new(),
                        code,
                    });
                    continue;
                }
            }
        }
        if let Some(item) = bullet_item(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let mut items = vec![item.to_string()];
            while let Some(next) = lines.peek() {
                match bullet_item(next.trim_end()) {
                    Some(item) => {
                        items.push(item.to_string());
                        lines.next();
                    }
                    None => break,
                }
            }
            blocks.push(Block::Bullets(items));
            continue;
        }
        if let Some(item) = numbered_item(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let mut items = vec![item.to_string()];
            while let Some(next) = lines.peek() {
                match numbered_item(next.trim_end()) {
                    Some(item) => {
                        items.push(item.to_string());
                        lines.next();
                    }
                    None => break,
                }
            }
            blocks.push(Block::Numbered(items));
            continue;
        }
        paragraph.push(trimmed.trim());
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

fn is_table_row(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.ends_with('|') && t.len() > 2
}

fn is_table_separator(line: &str) -> bool {
    let t = line.trim();
    is_table_row(t)
        && t.chars()
            .all(|c| c == '|' || c == '-' || c == ':' || c == ' ')
        && t.contains('-')
}

fn split_table_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let inner = &t[1..t.len() - 1];
    inner.split('|').map(|c| c.trim().to_string()).collect()
}

fn bullet_item(line: &str) -> Option<&str> {
    let t = line.trim_start();
    t.strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("• "))
}

fn numbered_item(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let rest = &t[digits..];
    rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct InlineStyle {
    pub code: bool,
    pub bold: bool,
    pub italic: bool,
    pub link: bool,
}

/// One inline run: text, style, and the link target when `style.link`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: InlineStyle,
    pub href: Option<String>,
}

type Styled = (String, InlineStyle, Option<String>);

/// Split inline markdown into styled spans. Unterminated markers are shown
/// literally so a half-streamed `**bold` does not flicker. Links:
/// `[text](url)` and bare `https://…`.
pub fn inline_spans(text: &str) -> Vec<Span> {
    inline_styled(text)
        .into_iter()
        .map(|(text, style, href)| Span { text, style, href })
        .collect()
}

/// The text `inline_spans` renders, markers removed: what the user sees and
/// what a selection copies.
pub fn inline_plain(text: &str) -> String {
    inline_styled(text)
        .into_iter()
        .map(|(text, _, _)| text)
        .collect()
}

fn inline_styled(text: &str) -> Vec<Styled> {
    let mut spans: Vec<Styled> = Vec::new();
    let style = InlineStyle::default();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    let push = |current: &mut String, style: InlineStyle, spans: &mut Vec<Styled>| {
        if !current.is_empty() {
            spans.push((std::mem::take(current), style, None));
        }
    };

    while i < chars.len() {
        let c = chars[i];
        if c == '[' {
            if let Some(close) = find_from(&chars, i + 1, &[']']) {
                if close + 1 < chars.len() && chars[close + 1] == '(' {
                    if let Some(end) = find_from(&chars, close + 2, &[')']) {
                        push(&mut current, style, &mut spans);
                        let label: String = chars[i + 1..close].iter().collect();
                        let href: String = chars[close + 2..end].iter().collect();
                        spans.push((
                            label,
                            InlineStyle {
                                link: true,
                                ..style
                            },
                            Some(href),
                        ));
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        if c == 'h' && (slice_starts(&chars, i, "https://") || slice_starts(&chars, i, "http://")) {
            let end = (i..chars.len())
                .find(|&j| chars[j].is_whitespace() || chars[j] == ')' || chars[j] == '"')
                .unwrap_or(chars.len());
            let mut url_end = end;
            while url_end > i && matches!(chars[url_end - 1], '.' | ',' | ';' | ':') {
                url_end -= 1;
            }
            push(&mut current, style, &mut spans);
            let url: String = chars[i..url_end].iter().collect();
            spans.push((
                url.clone(),
                InlineStyle {
                    link: true,
                    ..style
                },
                Some(url),
            ));
            i = url_end;
            continue;
        }
        if c == '`' {
            if let Some(end) = find_from(&chars, i + 1, &['`']) {
                push(&mut current, style, &mut spans);
                let code: String = chars[i + 1..end].iter().collect();
                spans.push((
                    code,
                    InlineStyle {
                        code: true,
                        ..style
                    },
                    None,
                ));
                i = end + 1;
                continue;
            }
        }
        if c == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            if let Some(end) = find_pair(&chars, i + 2) {
                push(&mut current, style, &mut spans);
                let inner: String = chars[i + 2..end].iter().collect();
                for (t, s, href) in inline_styled(&inner) {
                    spans.push((
                        t,
                        InlineStyle {
                            bold: true,
                            ..merge(style, s)
                        },
                        href,
                    ));
                }
                i = end + 2;
                continue;
            }
        }
        if c == '*' || c == '_' {
            let prev_ws = i == 0 || chars[i - 1].is_whitespace();
            if prev_ws {
                if let Some(end) = find_from(&chars, i + 1, &[c]) {
                    if end > i + 1 {
                        push(&mut current, style, &mut spans);
                        let inner: String = chars[i + 1..end].iter().collect();
                        spans.push((
                            inner,
                            InlineStyle {
                                italic: true,
                                ..style
                            },
                            None,
                        ));
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        current.push(c);
        i += 1;
    }
    push(&mut current, style, &mut spans);
    spans
}

fn merge(a: InlineStyle, b: InlineStyle) -> InlineStyle {
    InlineStyle {
        code: a.code || b.code,
        bold: a.bold || b.bold,
        italic: a.italic || b.italic,
        link: a.link || b.link,
    }
}

/// Display polish for assistant bubbles already in the doc: insert a missing space
/// after `.`/`!`/`?` when the next character is uppercase ("have.Checking" →
/// "have. Checking"), between a lowercase run and an uppercase sentence start
/// ("findingsDon't" → "findings Don't"), and drop a trailing short tool-narration
/// sentence ("Checking the latest…") so the bubble stays prose-only.
///
/// Fenced ``` code blocks are left untouched so identifiers like `obj.Value`
/// stay intact inside the fence.
pub fn polish_assistant_prose(text: &str) -> String {
    let without_tail = strip_trailing_tool_narration(text);
    polish_outside_fences(&without_tail, fix_glued_sentence_spaces)
}

/// Run `polish` on prose only; copy fenced code blocks through unchanged.
fn polish_outside_fences(text: &str, polish: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut lines = text.lines().peekable();
    let mut prose: Vec<&str> = Vec::new();
    let ends_with_newline = text.ends_with('\n');

    let flush_prose = |prose: &mut Vec<&str>, out: &mut String| {
        if prose.is_empty() {
            return;
        }
        let chunk = prose.join("\n");
        prose.clear();
        let polished = polish(&chunk);
        if polished.is_empty() {
            return;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&polished);
    };

    while let Some(line) = lines.next() {
        if line.trim_end().trim_start().starts_with("```") {
            flush_prose(&mut prose, &mut out);
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(line);
            out.push('\n');
            for code_line in lines.by_ref() {
                out.push_str(code_line);
                out.push('\n');
                if code_line.trim_end().trim_start().starts_with("```") {
                    break;
                }
            }
            continue;
        }
        prose.push(line);
    }
    flush_prose(&mut prose, &mut out);
    if ends_with_newline && !out.ends_with('\n') {
        out.push('\n');
    } else if !ends_with_newline && out.ends_with('\n') && !text.is_empty() {
        out.pop();
    }
    out
}

/// True when `text` contains at least one fenced ``` code block opener.
pub fn has_fenced_code(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim_end().trim_start().starts_with("```"))
}

/// True when `chars[i]` (lowercase) is glued to an uppercase letter that starts a new
/// sentence/word ("findingsDon't"), not mid-camelCase ("GitHub", "iPhone").
fn is_glued_word_break(chars: &[char], i: usize) -> bool {
    let Some(next) = chars.get(i + 1) else {
        return false;
    };
    if !chars[i].is_lowercase() || !next.is_uppercase() {
        return false;
    }
    let continues = match chars.get(i + 2) {
        None => true,
        Some(n) => n.is_lowercase() || *n == '\'',
    };
    if !continues {
        return false;
    }
    let mut run_start = i;
    while run_start > 0 && chars[run_start - 1].is_lowercase() {
        run_start -= 1;
    }
    if run_start > 0 && chars[run_start - 1].is_ascii_alphabetic() {
        return false;
    }
    (i + 1 - run_start) >= 2
}

fn fix_glued_sentence_spaces(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        out.push(c);
        if matches!(c, '.' | '!' | '?') {
            let abbrev =
                c == '.' && i >= 2 && chars[i - 1].is_ascii_lowercase() && chars[i - 2] == '.';
            if !abbrev {
                if let Some(next) = chars.get(i + 1) {
                    if next.is_uppercase() {
                        out.push(' ');
                    }
                }
            }
        } else if is_glued_word_break(&chars, i) {
            out.push(' ');
        }
        i += 1;
    }
    out
}

fn strip_trailing_tool_narration(text: &str) -> String {
    let (body, nl) = match text.strip_suffix('\n') {
        Some(b) => match b.strip_suffix('\r') {
            Some(b2) => (b2, "\r\n"),
            None => (b, "\n"),
        },
        None => (text, ""),
    };
    let trimmed = body.trim_end();
    if trimmed.is_empty() {
        return String::new();
    }
    // Walk back to the last sentence boundary (period/!/?), including glued ".Word".
    let bytes = trimmed.as_bytes();
    let mut split_at: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'.' || b == b'!' || b == b'?' {
            let abbrev =
                b == b'.' && i >= 2 && bytes[i - 1].is_ascii_lowercase() && bytes[i - 2] == b'.';
            if !abbrev {
                let after = &trimmed[i + 1..];
                if after.is_empty() {
                    break;
                }
                if after.starts_with(' ')
                    || after.starts_with('\n')
                    || after.starts_with('\r')
                    || after.chars().next().is_some_and(|c| c.is_uppercase())
                {
                    split_at = Some(i + 1);
                }
            }
        }
        i += if b < 0x80 {
            1
        } else if b >> 5 == 0b110 {
            2
        } else if b >> 4 == 0b1110 {
            3
        } else {
            4
        };
    }
    let Some(at) = split_at else {
        return if is_short_tool_narration(trimmed) {
            String::new()
        } else {
            format!("{trimmed}{nl}")
        };
    };
    let head = trimmed[..at].trim_end();
    let tail = trimmed[at..].trim_start();
    if is_short_tool_narration(tail) {
        format!("{head}{nl}")
    } else {
        format!("{trimmed}{nl}")
    }
}

fn is_short_tool_narration(sentence: &str) -> bool {
    let l = sentence
        .trim()
        .trim_start_matches(['*', '_', '#', '>', '-', ' '])
        .to_ascii_lowercase();
    if l.is_empty() || l.chars().count() > 90 {
        return false;
    }
    [
        "running ",
        "executing ",
        "calling ",
        "invoking ",
        "let me check",
        "let me run",
        "let me look",
        "i'll check",
        "i will check",
        "i'll run",
        "i will run",
        "checking ",
        "reading the file",
        "first, i",
        "first i",
        "now i'll",
        "now i will",
    ]
    .iter()
    .any(|p| l.starts_with(p))
}

fn slice_starts(chars: &[char], at: usize, needle: &str) -> bool {
    let n: Vec<char> = needle.chars().collect();
    at + n.len() <= chars.len() && chars[at..at + n.len()] == n[..]
}

fn find_from(chars: &[char], from: usize, needles: &[char]) -> Option<usize> {
    (from..chars.len()).find(|&i| needles.contains(&chars[i]))
}

fn find_pair(chars: &[char], from: usize) -> Option<usize> {
    (from..chars.len().saturating_sub(1)).find(|&i| chars[i] == '*' && chars[i + 1] == '*')
}
