//! A small markdown block splitter. One transcript row = one block
//! (zeron-study §4.1). Inline: `code`, **bold**, *italic*, links, and TeX
//! math (`$…$`, `\(…\)`; `$$…$$` / `\[…\]` as a block). Enough for agent
//! replies; swap for pulldown-cmark when more is needed.

use super::math;
use super::urlwrap;

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
    /// Display math on its own line(s): `$$ … $$` or `\[ … \]`. `source` is
    /// the LaTeX between the delimiters ([feedback math-render-mac-2026-10-26]).
    Math {
        source: String,
    },
    /// A chart bubble's card ([feedback chart-bubble-mac-2026-10-26]): not markdown, but one
    /// block of the agent's bubble like a code card. `spec` is the `chart` part's JSON (the
    /// validated `arbos_core::chart::Chart`), `table` its markdown table (what a copy carries).
    Chart {
        part_id: String,
        spec: String,
        table: String,
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
        if let Some((opener, closer)) = display_math_delimiters(trimmed) {
            let rest = &trimmed.trim_start()[opener.len()..];
            if let Some(inner) = rest.trim_end().strip_suffix(closer) {
                if !inner.trim().is_empty() {
                    flush(&mut paragraph, &mut blocks);
                    blocks.push(Block::Math {
                        source: inner.trim().to_string(),
                    });
                    continue;
                }
            }
            // `$$` opening a multi-line block: everything up to the closing
            // line. Never closed, the lines are prose as typed.
            let mut body: Vec<&str> = Vec::new();
            if !rest.trim().is_empty() {
                body.push(rest.trim());
            }
            let mut consumed: Vec<&str> = Vec::new();
            let mut closed = false;
            while let Some(next) = lines.peek() {
                let line = next.trim_end();
                lines.next();
                if line.trim().is_empty() {
                    continue;
                }
                consumed.push(line);
                if let Some(inner) = line.trim().strip_suffix(closer) {
                    if !inner.trim().is_empty() {
                        body.push(inner.trim());
                    }
                    closed = true;
                    break;
                }
                body.push(line.trim());
            }
            if closed {
                flush(&mut paragraph, &mut blocks);
                blocks.push(Block::Math {
                    source: body.join("\n"),
                });
            } else {
                paragraph.push(trimmed.trim());
                paragraph.extend(consumed.iter().map(|l| l.trim()));
            }
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

/// The opener a display-math line starts with and the closer that ends it.
fn display_math_delimiters(line: &str) -> Option<(&'static str, &'static str)> {
    let t = line.trim_start();
    if t.starts_with("$$") {
        Some(("$$", "$$"))
    } else if t.starts_with("\\[") {
        Some(("\\[", "\\]"))
    } else {
        None
    }
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
    /// Typeset TeX: painted in the math face; `italic` marks its variables,
    /// `bold` its `\mathbf`.
    pub math: bool,
}

/// One inline run: text, style, and the link target when `style.link`. A named link
/// (`[Name](https://…)`, not a bare URL) starts with [`LINK_ICON_CELL`] and names the
/// site in `icon`: the renderer draws the site's small picture in that cell (Jacob,
/// 2026-10-25: every link in a reply carries its favicon, as in Grok Bot).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: InlineStyle,
    pub href: Option<String>,
    pub icon: Option<String>,
}

/// What a named link's text starts with: an em space and a three-per-em space, the
/// cell a 14 pt icon and its gap fit in at body size. Copies carry it as whitespace.
pub const LINK_ICON_CELL: &str = "\u{2003}\u{2004}";

/// The host of `href` when it is an http(s) URL a site icon can be fetched for.
pub fn link_host(href: &str) -> Option<String> {
    let rest = href
        .strip_prefix("https://")
        .or_else(|| href.strip_prefix("http://"))?;
    let host = rest
        .split(['/', '?', '#'])
        .next()?
        .rsplit('@')
        .next()?
        .split(':')
        .next()?
        .trim()
        .to_ascii_lowercase();
    let ok = host.contains('.')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !host
            .split('.')
            .all(|l| l.chars().all(|c| c.is_ascii_digit()));
    ok.then_some(host)
}

/// Whether a link label is itself a URL (then it gets no icon cell: the address is the text).
fn label_is_url(label: &str) -> bool {
    let l = label.trim();
    l.starts_with("http://") || l.starts_with("https://") || l.starts_with("www.")
}

/// Every site an icon is wanted for in `text`'s rendered blocks (named links in prose,
/// headings, lists and table cells; never inside code fences), each host once, in order.
pub fn link_hosts(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |t: &str| {
        for span in inline_spans(t) {
            if let Some(h) = span.icon {
                if !out.contains(&h) {
                    out.push(h);
                }
            }
        }
    };
    for block in split_blocks(text) {
        match block {
            Block::Paragraph(t) | Block::Heading { text: t, .. } => push(&t),
            Block::Bullets(items) | Block::Numbered(items) => items.iter().for_each(|i| push(i)),
            Block::Table { header, rows } => {
                header.iter().for_each(|c| push(c));
                rows.iter().flatten().for_each(|c| push(c));
            }
            Block::Code { .. } | Block::Math { .. } | Block::Chart { .. } => {}
        }
    }
    out
}

type Styled = (String, InlineStyle, Option<String>, Option<String>);

/// Split inline markdown into styled spans. Unterminated markers are shown
/// literally so a half-streamed `**bold` does not flicker. Links:
/// `[text](url)` and bare `https://…`.
pub fn inline_spans(text: &str) -> Vec<Span> {
    inline_styled(text)
        .into_iter()
        .map(|(text, style, href, icon)| Span {
            text,
            style,
            href,
            icon,
        })
        .collect()
}

/// The text `inline_spans` renders, markers removed: what the user sees and
/// what a selection copies.
pub fn inline_plain(text: &str) -> String {
    inline_styled(text)
        .into_iter()
        .map(|(text, _, _, _)| text)
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
            spans.push((std::mem::take(current), style, None, None));
        }
    };

    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && chars.get(i + 1) == Some(&'$') {
            current.push('$');
            i += 2;
            continue;
        }
        if let Some((source, end)) = math_at(&chars, i) {
            push(&mut current, style, &mut spans);
            for run in math::typeset(&source) {
                spans.push((
                    run.text,
                    InlineStyle {
                        math: true,
                        italic: run.style == math::MathStyle::Italic,
                        bold: run.style == math::MathStyle::Bold,
                        ..style
                    },
                    None,
                    None,
                ));
            }
            i = end;
            continue;
        }
        if c == '[' {
            if let Some(close) = find_from(&chars, i + 1, &[']']) {
                if close + 1 < chars.len() && chars[close + 1] == '(' {
                    if let Some(end) = find_from(&chars, close + 2, &[')']) {
                        push(&mut current, style, &mut spans);
                        let label: String = chars[i + 1..close].iter().collect();
                        let href: String = chars[close + 2..end].iter().collect();
                        let icon = (!label_is_url(&label)).then(|| link_host(&href)).flatten();
                        let text = match &icon {
                            Some(_) => format!("{LINK_ICON_CELL}{label}"),
                            // An address as the label wraps like a bare URL.
                            None if label_is_url(&label) => urlwrap::breakable(&label),
                            None => label,
                        };
                        spans.push((
                            text,
                            InlineStyle {
                                link: true,
                                ..style
                            },
                            Some(href),
                            icon,
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
            // The text carries the wrap hints; the target is the address itself.
            spans.push((
                urlwrap::breakable(&url),
                InlineStyle {
                    link: true,
                    ..style
                },
                Some(url),
                None,
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
                for (t, s, href, icon) in inline_styled(&inner) {
                    spans.push((
                        t,
                        InlineStyle {
                            bold: true,
                            ..merge(style, s)
                        },
                        href,
                        icon,
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
        math: a.math || b.math,
    }
}

/// An inline math span starting at `chars[i]`: the LaTeX inside and the index
/// after the closing delimiter. `\( … \)`, `\[ … \]` and `$$ … $$` always;
/// `$ … $` by Pandoc's rule so prices stay prose: the opening `$` is not
/// glued to a word and has a non-space right after it, the closing `$` has a
/// non-space right before it and no digit after it, and the span stays on one
/// line — "$5 and $10", "US$3" and a lone "$" are text.
fn math_at(chars: &[char], i: usize) -> Option<(String, usize)> {
    let inner = |from: usize, to: usize| -> String { chars[from..to].iter().collect() };
    let find_pair = |from: usize, a: char, b: char| -> Option<usize> {
        (from..chars.len().saturating_sub(1)).find(|&j| chars[j] == a && chars[j + 1] == b)
    };
    if chars[i] == '\\' {
        let open = *chars.get(i + 1)?;
        let close = match open {
            '(' => ')',
            '[' => ']',
            _ => return None,
        };
        let end = find_pair(i + 2, '\\', close)?;
        let source = inner(i + 2, end);
        return (!source.trim().is_empty()).then(|| (source.trim().to_string(), end + 2));
    }
    if chars[i] != '$' {
        return None;
    }
    if chars.get(i + 1) == Some(&'$') {
        let end = find_pair(i + 2, '$', '$')?;
        let source = inner(i + 2, end);
        return (!source.trim().is_empty()).then(|| (source.trim().to_string(), end + 2));
    }
    if i > 0 && chars[i - 1].is_alphanumeric() {
        return None;
    }
    let first = *chars.get(i + 1)?;
    if first.is_whitespace() {
        return None;
    }
    let mut j = i + 1;
    while j < chars.len() {
        match chars[j] {
            '\n' => return None,
            '$' if chars[j - 1] != '\\' => break,
            _ => j += 1,
        }
    }
    if j >= chars.len() || chars[j - 1].is_whitespace() {
        return None;
    }
    if chars.get(j + 1).is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((inner(i + 1, j), j + 1))
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

#[cfg(test)]
mod math_tests {
    use super::*;

    fn plain(text: &str) -> String {
        inline_plain(text)
    }

    fn math_spans(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut current = String::new();
        for span in inline_spans(text) {
            if span.style.math {
                current.push_str(&span.text);
            } else if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
        out
    }

    /// Jacob's bubble-sort reply (2026-10-26): the complexity is typeset, the prose and the
    /// `swapped` code chip around it survive.
    #[test]
    fn big_o_renders_as_math() {
        let text = "It runs in $O(n^2)$ time in the worst and average cases, and $O(n)$ time when the array is already sorted thanks to the `swapped` flag. Space complexity is $O(1)$.";
        assert_eq!(math_spans(text), vec!["O(n²)", "O(n)", "O(1)"]);
        assert_eq!(
            plain(text),
            "It runs in O(n²) time in the worst and average cases, and O(n) time when the array is already sorted thanks to the swapped flag. Space complexity is O(1)."
        );
        let spans = inline_spans(text);
        let o = spans.iter().find(|s| s.text == "O").expect("the O run");
        assert!(o.style.math && o.style.italic, "{o:?}");
        let n2 = spans.iter().find(|s| s.text == "²)").expect("the ²) run");
        assert!(n2.style.math && !n2.style.italic, "{n2:?}");
        assert!(spans.iter().any(|s| s.style.code && s.text == "swapped"));
    }

    #[test]
    fn prices_and_lone_dollars_stay_prose() {
        for text in [
            "costs $5 and $10",
            "$5",
            "US$3 and US$4",
            "a lone $ sign",
            "unmatched $x here",
            "no $ x$ math",
            "from $5 to $10, then $20",
        ] {
            assert!(math_spans(text).is_empty(), "{text}");
            assert_eq!(plain(text), text);
        }
        assert_eq!(plain("escaped \\$5 stays"), "escaped $5 stays");
        assert_eq!(math_spans("`$x$` in code"), Vec::<String>::new());
        assert!(inline_spans("`$x$` in code")
            .iter()
            .any(|s| s.style.code && s.text == "$x$"));
    }

    #[test]
    fn other_delimiters_and_display_blocks() {
        assert_eq!(math_spans("so \\(x^2\\) and $$y_1$$ too"), vec!["x²", "y₁"]);
        assert_eq!(plain("so \\(x^2\\) and $$y_1$$ too"), "so x² and y₁ too");
        assert_eq!(
            split_blocks("Sum:\n\n$$\n\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}\n$$\n\nDone."),
            vec![
                Block::Paragraph("Sum:".into()),
                Block::Math {
                    source: "\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}".into()
                },
                Block::Paragraph("Done.".into()),
            ]
        );
        assert_eq!(
            split_blocks("$$ E = mc^2 $$"),
            vec![Block::Math {
                source: "E = mc^2".into()
            }]
        );
        assert_eq!(
            split_blocks("\\[\nx^2\n\\]"),
            vec![Block::Math {
                source: "x^2".into()
            }]
        );
        // Never closed: prose as typed.
        assert_eq!(
            split_blocks("$$\nx^2\n"),
            vec![Block::Paragraph("$$\nx^2".into())]
        );
        assert_eq!(
            link_hosts("$$ x $$ and [a](https://a.com)"),
            vec!["a.com".to_string()]
        );
    }
}
