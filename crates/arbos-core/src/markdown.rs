//! The chat Markdown grammar, shared by the Mac and the iPhone (the latter
//! through `arbos-ffi`): one parser, so a message formats the same in every
//! bubble ([feedback chat-markdown-core-2026-10-27]).
//!
//! The grammar is the subset chat apps agree on (Grok Bot, Telegram, Discord,
//! Slack), written down in `docs/mac-design-rules.md` § Message formatting:
//!
//! - Blocks: `> ` quotes (multi-line, nested), `#`…`###` headings, `-` / `*` /
//!   `+` / `•` bullets, `1.` / `1)` numbered lists, ``` fences, GFM pipe tables,
//!   `$$` / `\[` display math, paragraphs (soft newlines kept: Enter is a line
//!   break, a blank line is a paragraph break).
//! - Inline: `**bold**`, `*italic*` / `_italic_`, `***both***`,
//!   `~~strikethrough~~`, `||spoiler||`, `` `code` ``, `[text](url)`, bare
//!   `http(s)://` and `www.` links, `$…$` / `\(…\)` math, `\` escapes.
//!
//! Plain text is never mangled. Every emphasis marker opens only at a word
//! start (after a space, punctuation or the line start) and closes only at a
//! word end, so `snake_case`, `2*3*4`, `a_b`, `__init__`, `x||y` and e-mail
//! addresses stay as typed; an unmatched marker is literal; HTML is text.

/// One block of a message. Paragraph, heading, list-item and table-cell text
/// is still inline source: render it through [`inline_spans`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Block {
    Paragraph(String),
    /// `# text` … `### text` (deeper levels clamp to 3). Rendered as bolder,
    /// larger text; never an anchor.
    Heading {
        level: u8,
        text: String,
    },
    /// `> ` lines, parsed again inside: a quote holds any block, including
    /// another quote.
    Quote(Vec<Block>),
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
    /// the LaTeX between the delimiters.
    Math {
        source: String,
    },
}

/// How many `>` levels deep a block is, counting the outermost quote as 1.
pub fn quote_depth(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|b| match b {
            Block::Quote(inner) => 1 + quote_depth(inner),
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

/// Split a message into blocks.
pub fn parse_blocks(source: &str) -> Vec<Block> {
    let lines: Vec<&str> = source.lines().collect();
    parse_lines(&lines, 0)
}

/// Quotes nest at most this deep; deeper `>` runs stay literal text.
const MAX_QUOTE_DEPTH: usize = 4;

fn parse_lines(lines: &[&str], depth: usize) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut i = 0usize;

    fn flush(paragraph: &mut Vec<&str>, blocks: &mut Vec<Block>) {
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(paragraph.join("\n")));
            paragraph.clear();
        }
    }

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            flush(&mut paragraph, &mut blocks);
            i += 1;
            continue;
        }
        // Leading spaces are fine (CommonMark allows a short indent).
        if let Some(rest) = strip_fence(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let lang = rest.trim().to_string();
            let mut code = String::new();
            i += 1;
            while i < lines.len() {
                let code_line = lines[i];
                i += 1;
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
        // `> text` opens a quote; the quote is every following line that also
        // starts with `>`, parsed again with one marker removed. A `>` with
        // nothing after it only continues a quote (a blank line inside it);
        // `>5` with no space, and a lone `>`, are text.
        if depth < MAX_QUOTE_DEPTH && quote_content(trimmed).is_some_and(|t| !t.trim().is_empty()) {
            flush(&mut paragraph, &mut blocks);
            let mut inner: Vec<&str> = Vec::new();
            while i < lines.len() {
                match quote_line(lines[i].trim_end()) {
                    Some(rest) => inner.push(rest),
                    None => break,
                }
                i += 1;
            }
            while inner.last().is_some_and(|l| l.trim().is_empty()) {
                inner.pop();
            }
            blocks.push(Block::Quote(parse_lines(&inner, depth + 1)));
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
                    i += 1;
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
            let mut j = i + 1;
            while j < lines.len() {
                let line = lines[j].trim_end();
                j += 1;
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
            i = j;
            continue;
        }
        if let Some((level, text)) = heading(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading {
                level,
                text: text.to_string(),
            });
            i += 1;
            continue;
        }
        if is_table_row(trimmed) && i + 1 < lines.len() {
            let next = lines[i + 1].trim_end();
            if is_table_separator(next) {
                flush(&mut paragraph, &mut blocks);
                let header = split_table_row(trimmed);
                i += 2;
                let mut rows = Vec::new();
                while i < lines.len() {
                    let row = lines[i].trim_end();
                    if !is_table_row(row) {
                        break;
                    }
                    rows.push(split_table_row(row));
                    i += 1;
                }
                blocks.push(Block::Table { header, rows });
                continue;
            }
            // Pipe rows with no separator row are not a GFM table. Rather
            // than paint them as prose with raw `|`, they degrade to one
            // monospace block that keeps its columns and scrolls sideways.
            if is_table_row(next) {
                flush(&mut paragraph, &mut blocks);
                let mut code = trimmed.trim().to_string();
                i += 1;
                while i < lines.len() {
                    let row = lines[i].trim_end();
                    if !is_table_row(row) {
                        break;
                    }
                    code.push('\n');
                    code.push_str(row.trim());
                    i += 1;
                }
                blocks.push(Block::Code {
                    lang: String::new(),
                    code,
                });
                continue;
            }
        }
        if let Some(item) = bullet_item(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let mut items = vec![item.to_string()];
            i += 1;
            while i < lines.len() {
                match bullet_item(lines[i].trim_end()) {
                    Some(item) => {
                        items.push(item.to_string());
                        i += 1;
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
            i += 1;
            while i < lines.len() {
                match numbered_item(lines[i].trim_end()) {
                    Some(item) => {
                        items.push(item.to_string());
                        i += 1;
                    }
                    None => break,
                }
            }
            blocks.push(Block::Numbered(items));
            continue;
        }
        paragraph.push(trimmed.trim());
        i += 1;
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

/// The text after the opening ``` of a fence line, if `line` is one.
fn strip_fence(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if line.len() - t.len() > 3 {
        return None;
    }
    t.strip_prefix("```")
}

/// `line` with one quote marker removed, when it is a quote line: at most
/// three spaces, `>`, then a space (dropped), another `>` (nesting) or the end
/// of the line. `>5` (no space) is not a quote line.
fn quote_line(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if line.len() - t.len() > 3 {
        return None;
    }
    let rest = t.strip_prefix('>')?;
    if rest.is_empty() {
        return Some(rest);
    }
    if let Some(r) = rest.strip_prefix(' ') {
        return Some(r);
    }
    if rest.starts_with('>') {
        return Some(rest);
    }
    None
}

/// The text of a quote line with every nesting marker removed (`>> x` → `x`).
fn quote_content(line: &str) -> Option<&str> {
    let mut rest = quote_line(line)?;
    while let Some(inner) = quote_line(rest) {
        rest = inner;
    }
    Some(rest)
}

/// `# text`: one to six `#`, a space, then text. `#hashtag` is prose. Levels
/// deeper than three render like the third.
fn heading(line: &str) -> Option<(u8, &str)> {
    let t = line.trim_start();
    if line.len() - t.len() > 3 {
        return None;
    }
    let hashes = t.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = t[hashes..].strip_prefix(' ')?;
    let text = rest.trim();
    if text.is_empty() {
        return None;
    }
    Some((hashes.min(3) as u8, text))
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

/// `- item`, `* item`, `+ item` or `• item` (the text may be indented).
fn bullet_item(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let rest = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("+ "))
        .or_else(|| t.strip_prefix("• "))?;
    let item = rest.trim_start();
    (!item.is_empty()).then_some(item)
}

/// `1. item` or `1) item`, up to nine digits.
fn numbered_item(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let rest = &t[digits..];
    let rest = rest
        .strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))?;
    let item = rest.trim_start();
    (!item.is_empty()).then_some(item)
}

/// The styles one inline run carries. `spoiler` is 0 outside a spoiler, else
/// the 1-based index of the `||…||` the run belongs to within its text, so a
/// tap reveals the whole spoiler even when it holds several runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct InlineStyle {
    pub code: bool,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub link: bool,
    /// The run is TeX source (`text` is the LaTeX): the renderer typesets it.
    pub math: bool,
    pub spoiler: u16,
}

/// One inline run: its text (markers removed, escapes resolved), its style,
/// and the link target when `style.link`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    pub text: String,
    pub style: InlineStyle,
    pub href: Option<String>,
}

/// Split inline markdown into styled runs. Unterminated markers are shown
/// literally, so a half-streamed `**bold` does not flicker.
pub fn inline_spans(text: &str) -> Vec<Span> {
    let chars: Vec<char> = text.chars().collect();
    let mut spoilers = 0u16;
    let mut out = Vec::new();
    inline_into(
        &chars,
        0,
        chars.len(),
        InlineStyle::default(),
        None,
        &mut spoilers,
        &mut out,
    );
    out
}

/// The text [`inline_spans`] renders: markers removed, math as its source.
pub fn inline_plain(text: &str) -> String {
    inline_spans(text).into_iter().map(|s| s.text).collect()
}

/// The plain text of a block — what a copy of it carries. Math stays source.
pub fn block_plain(block: &Block) -> String {
    match block {
        Block::Paragraph(t) | Block::Heading { text: t, .. } => inline_plain(t),
        Block::Quote(inner) => blocks_plain(inner),
        Block::Bullets(items) => items
            .iter()
            .map(|i| format!("• {}", inline_plain(i)))
            .collect::<Vec<_>>()
            .join("\n"),
        Block::Numbered(items) => items
            .iter()
            .enumerate()
            .map(|(n, i)| format!("{}. {}", n + 1, inline_plain(i)))
            .collect::<Vec<_>>()
            .join("\n"),
        Block::Code { code, .. } => code.clone(),
        Block::Table { header, rows } => std::iter::once(header)
            .chain(rows.iter())
            .map(|r| {
                r.iter()
                    .map(|c| inline_plain(c))
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Block::Math { source } => source.clone(),
    }
}

/// The plain text of a whole message, blocks one newline apart.
pub fn blocks_plain(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(block_plain)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Markers an emphasis-like delimiter run may be, longest first.
const DELIMS: [(&str, Delim); 6] = [
    ("***", Delim::BoldItalic),
    ("**", Delim::Bold),
    ("~~", Delim::Strike),
    ("||", Delim::Spoiler),
    ("*", Delim::Italic),
    ("_", Delim::Italic),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Delim {
    Bold,
    Italic,
    BoldItalic,
    Strike,
    Spoiler,
}

/// Parse `chars[from..to]` into `out` with `base` applied to every run.
/// `inside_link` is the href of the enclosing `[…](…)`, when any.
fn inline_into(
    chars: &[char],
    from: usize,
    to: usize,
    base: InlineStyle,
    inside_link: Option<&str>,
    spoilers: &mut u16,
    out: &mut Vec<Span>,
) {
    let mut current = String::new();
    let mut i = from;

    let flush = |current: &mut String, out: &mut Vec<Span>| {
        if !current.is_empty() {
            out.push(Span {
                text: std::mem::take(current),
                style: base,
                href: inside_link.map(str::to_owned),
            });
        }
    };

    while i < to {
        let c = chars[i];
        // `\` + ASCII punctuation: the character itself. `\(`/`\[` that open
        // math are handled by `math_at` first.
        if c == '\\' {
            if let Some((source, end)) = math_at(chars, i, to) {
                flush(&mut current, out);
                push_math(&source, base, inside_link, out);
                i = end;
                continue;
            }
            if let Some(&next) = chars.get(i + 1).filter(|_| i + 1 < to) {
                if next.is_ascii_punctuation() {
                    current.push(next);
                    i += 2;
                    continue;
                }
            }
            current.push(c);
            i += 1;
            continue;
        }
        if c == '$' {
            if let Some((source, end)) = math_at(chars, i, to) {
                flush(&mut current, out);
                push_math(&source, base, inside_link, out);
                i = end;
                continue;
            }
        }
        if c == '`' {
            if let Some((code, end)) = code_span_at(chars, i, to) {
                flush(&mut current, out);
                out.push(Span {
                    text: code,
                    style: InlineStyle { code: true, ..base },
                    href: inside_link.map(str::to_owned),
                });
                i = end;
                continue;
            }
        }
        if c == '[' && inside_link.is_none() {
            if let Some((label_end, typed, href, end)) = link_at(chars, i, to) {
                flush(&mut current, out);
                let style = InlineStyle { link: true, ..base };
                let before = out.len();
                inline_into(chars, i + 1, label_end, style, Some(&href), spoilers, out);
                if out.len() == before {
                    // `[](url)`: the address as typed is the text.
                    out.push(Span {
                        text: typed,
                        style,
                        href: Some(href),
                    });
                }
                i = end;
                continue;
            }
        }
        if let Some((url, end)) = autolink_at(chars, i, to) {
            flush(&mut current, out);
            let href = if url.starts_with("www.") {
                format!("https://{url}")
            } else {
                url.clone()
            };
            out.push(Span {
                text: url,
                style: InlineStyle { link: true, ..base },
                href: Some(inside_link.map(str::to_owned).unwrap_or(href)),
            });
            i = end;
            continue;
        }
        if let Some((delim, len, close)) = delimiter_at(chars, i, to) {
            flush(&mut current, out);
            let mut style = base;
            match delim {
                Delim::Bold => style.bold = true,
                Delim::Italic => style.italic = true,
                Delim::BoldItalic => {
                    style.bold = true;
                    style.italic = true;
                }
                Delim::Strike => style.strike = true,
                Delim::Spoiler => {
                    *spoilers += 1;
                    style.spoiler = *spoilers;
                }
            }
            inline_into(chars, i + len, close, style, inside_link, spoilers, out);
            i = close + len;
            continue;
        }
        current.push(c);
        i += 1;
    }
    flush(&mut current, out);
}

fn push_math(source: &str, base: InlineStyle, inside_link: Option<&str>, out: &mut Vec<Span>) {
    out.push(Span {
        text: source.to_string(),
        style: InlineStyle { math: true, ..base },
        href: inside_link.map(str::to_owned),
    });
}

/// Whether the character before `i` lets a marker open there: the start of
/// the text, whitespace or punctuation — never a letter or digit, so a marker
/// inside a word (`snake_case`, `2*3`) is text.
fn opens_after(chars: &[char], i: usize) -> bool {
    i == 0 || !chars[i - 1].is_alphanumeric()
}

/// Whether the character at `i` (the one after a closing run) lets a marker
/// close there: the end of the text, whitespace or punctuation.
fn closes_before(chars: &[char], to: usize, i: usize) -> bool {
    i >= to || !chars[i].is_alphanumeric()
}

/// The emphasis-like delimiter run opening at `chars[i]`, its length and the
/// index of its closing run, when the run both opens and closes by the
/// word-boundary rules. Code spans between the two are skipped.
fn delimiter_at(chars: &[char], i: usize, to: usize) -> Option<(Delim, usize, usize)> {
    let (marker, delim) = DELIMS
        .iter()
        .find(|(m, _)| starts_with(chars, i, to, m))
        .copied()?;
    let len = marker.chars().count();
    let marker_char = chars[i];
    // The run is exactly `marker`: `****` or `~~~` is text.
    if i + len < to && chars[i + len] == marker_char {
        return None;
    }
    if !opens_after(chars, i) || (i > 0 && chars[i - 1] == marker_char) {
        return None;
    }
    let first = *chars.get(i + len).filter(|_| i + len < to)?;
    if first.is_whitespace() {
        return None;
    }
    // Find the closer: the same run, preceded by a non-space, followed by a
    // word boundary, and not glued to a longer run.
    let mut j = i + len;
    while j < to {
        if chars[j] == '`' {
            if let Some((_, end)) = code_span_at(chars, j, to) {
                j = end;
                continue;
            }
        }
        if chars[j] == '\\' && j + 1 < to && chars[j + 1].is_ascii_punctuation() {
            j += 2;
            continue;
        }
        if chars[j] == '\n' && delim != Delim::Spoiler {
            // Emphasis stays on one line of the paragraph.
            return None;
        }
        if starts_with(chars, j, to, marker)
            && j > i + len
            && !chars[j - 1].is_whitespace()
            && chars[j - 1] != marker_char
            && closes_before(chars, to, j + len)
            && !(j + len < to && chars[j + len] == marker_char)
        {
            return Some((delim, len, j));
        }
        j += 1;
    }
    None
}

/// A code span opening at `chars[i]`: its text and the index after the
/// closing run (a backtick run of the same length).
fn code_span_at(chars: &[char], i: usize, to: usize) -> Option<(String, usize)> {
    let mut n = 0;
    while i + n < to && chars[i + n] == '`' {
        n += 1;
    }
    if n == 0 {
        return None;
    }
    let mut j = i + n;
    while j < to {
        if chars[j] == '`' {
            let mut m = 0;
            while j + m < to && chars[j + m] == '`' {
                m += 1;
            }
            if m == n {
                let inner: String = chars[i + n..j].iter().collect();
                // One space each side is padding, not content (CommonMark).
                let text = if inner.len() > 2
                    && inner.starts_with(' ')
                    && inner.ends_with(' ')
                    && !inner.trim().is_empty()
                {
                    inner[1..inner.len() - 1].to_string()
                } else {
                    inner
                };
                return Some((text, j + m));
            }
            j += m;
            continue;
        }
        j += 1;
    }
    None
}

/// `[label](href)` opening at `chars[i]`: the index of the label's `]`, the
/// target as typed, the href to open and the index after the `)`. The href keeps balanced parentheses, drops
/// an optional `"title"`, and must be a link the app opens: `http(s)`,
/// `mailto`, `arbos` or `www.`; anything else leaves the text as typed.
fn link_at(chars: &[char], i: usize, to: usize) -> Option<(usize, String, String, usize)> {
    let mut j = i + 1;
    let mut depth = 0usize;
    let label_end = loop {
        if j >= to {
            return None;
        }
        match chars[j] {
            '\\' => j += 1,
            '[' => depth += 1,
            ']' if depth == 0 => break j,
            ']' => depth -= 1,
            '\n' => return None,
            _ => {}
        }
        j += 1;
    };
    if label_end + 1 >= to || chars[label_end + 1] != '(' {
        return None;
    }
    let mut k = label_end + 2;
    let mut parens = 0usize;
    let href_start = k;
    let href_end = loop {
        if k >= to {
            return None;
        }
        match chars[k] {
            '(' => parens += 1,
            ')' if parens == 0 => break k,
            ')' => parens -= 1,
            c if c.is_whitespace() => break k,
            _ => {}
        }
        k += 1;
    };
    let typed: String = chars[href_start..href_end].iter().collect();
    // After the target only spaces and a `"title"` may precede the `)`.
    let mut end = href_end;
    while end < to && chars[end] == ' ' {
        end += 1;
    }
    if end < to && matches!(chars[end], '"' | '\'') {
        let quote = chars[end];
        end += 1;
        while end < to && chars[end] != quote {
            if chars[end] == '\n' {
                return None;
            }
            end += 1;
        }
        end += 1;
        while end < to && chars[end] == ' ' {
            end += 1;
        }
    }
    if end >= to || chars[end] != ')' {
        return None;
    }
    let href = if typed.starts_with("www.") {
        format!("https://{typed}")
    } else {
        typed.clone()
    };
    if !safe_href(&href) {
        return None;
    }
    Some((label_end, typed, href, end + 1))
}

/// Link targets the apps open. Everything else (`javascript:`, `data:`,
/// relative paths) stays literal text.
pub fn safe_href(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    let web = (lower.starts_with("https://") || lower.starts_with("http://"))
        && href.len() > 8
        && !href.chars().any(char::is_whitespace);
    let mail = lower.starts_with("mailto:") && href.contains('@');
    web || mail || lower.starts_with("arbos://")
}

/// A bare URL starting at `chars[i]`: `http://`, `https://` or `www.`, up to
/// whitespace or a delimiter, trailing punctuation left to the prose.
fn autolink_at(chars: &[char], i: usize, to: usize) -> Option<(String, usize)> {
    let www = starts_with(chars, i, to, "www.");
    let http = starts_with(chars, i, to, "https://") || starts_with(chars, i, to, "http://");
    if !www && !http {
        return None;
    }
    if !opens_after(chars, i) {
        return None;
    }
    let mut end = i;
    while end < to && !chars[end].is_whitespace() && !matches!(chars[end], '<' | '>' | '"') {
        end += 1;
    }
    // Trailing sentence punctuation is prose; an unbalanced `)` too.
    while end > i {
        let last = chars[end - 1];
        if matches!(
            last,
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' | '_' | '~' | '|'
        ) {
            end -= 1;
        } else if last == ')' {
            let opens = chars[i..end].iter().filter(|c| **c == '(').count();
            let closes = chars[i..end].iter().filter(|c| **c == ')').count();
            if closes > opens {
                end -= 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    let url: String = chars[i..end].iter().collect();
    let host_len = if www { 4 } else { url.find("://")? + 3 };
    // `www.` or `https://` alone is not a link.
    if url.len() <= host_len || !url[host_len..].contains(|c: char| c.is_alphanumeric()) {
        return None;
    }
    Some((url, end))
}

fn starts_with(chars: &[char], at: usize, to: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(k, n)| at + k < to && chars[at + k] == n)
}

/// An inline math span starting at `chars[i]`: the LaTeX inside and the index
/// after the closing delimiter. `\( … \)`, `\[ … \]` and `$$ … $$` always;
/// `$ … $` by Pandoc's rule so prices stay prose: the opening `$` is not
/// glued to a word and has a non-space right after it, the closing `$` has a
/// non-space right before it and no digit after it, and the span stays on one
/// line — "$5 and $10", "US$3" and a lone "$" are text.
fn math_at(chars: &[char], i: usize, to: usize) -> Option<(String, usize)> {
    let inner = |from: usize, end: usize| -> String { chars[from..end].iter().collect() };
    let find_pair = |from: usize, a: char, b: char| -> Option<usize> {
        (from..to.saturating_sub(1)).find(|&j| chars[j] == a && chars[j + 1] == b)
    };
    if chars[i] == '\\' {
        let open = *chars.get(i + 1).filter(|_| i + 1 < to)?;
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
    if i + 1 < to && chars[i + 1] == '$' {
        let end = find_pair(i + 2, '$', '$')?;
        let source = inner(i + 2, end);
        return (!source.trim().is_empty()).then(|| (source.trim().to_string(), end + 2));
    }
    if i > 0 && chars[i - 1].is_alphanumeric() {
        return None;
    }
    let first = *chars.get(i + 1).filter(|_| i + 1 < to)?;
    if first.is_whitespace() {
        return None;
    }
    let mut j = i + 1;
    while j < to {
        match chars[j] {
            '\n' => return None,
            '$' if chars[j - 1] != '\\' => break,
            _ => j += 1,
        }
    }
    if j >= to || chars[j - 1].is_whitespace() {
        return None;
    }
    if j + 1 < to && chars[j + 1].is_ascii_digit() {
        return None;
    }
    Some((inner(i + 1, j), j + 1))
}

/// True when `text` contains at least one fenced ``` code block opener.
pub fn has_fenced_code(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim_end().trim_start().starts_with("```"))
}

/// Whether `text` holds any formatting the parser would act on — false for a
/// message that renders exactly as typed (the fast path for most chat lines).
pub fn is_plain(text: &str) -> bool {
    let blocks = parse_blocks(text);
    let plain = blocks_plain(&blocks);
    let typed: String = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    plain == typed
        && blocks.iter().all(|b| matches!(b, Block::Paragraph(_)))
        && blocks.iter().all(|b| match b {
            Block::Paragraph(t) => inline_spans(t)
                .iter()
                .all(|s| s.style == InlineStyle::default()),
            _ => false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Block {
        Block::Paragraph(text.into())
    }

    fn styles(text: &str) -> Vec<(String, InlineStyle)> {
        inline_spans(text)
            .into_iter()
            .map(|s| (s.text, s.style))
            .collect()
    }

    fn bold() -> InlineStyle {
        InlineStyle {
            bold: true,
            ..Default::default()
        }
    }
    fn italic() -> InlineStyle {
        InlineStyle {
            italic: true,
            ..Default::default()
        }
    }
    fn strike() -> InlineStyle {
        InlineStyle {
            strike: true,
            ..Default::default()
        }
    }
    fn code() -> InlineStyle {
        InlineStyle {
            code: true,
            ..Default::default()
        }
    }
    fn plain() -> InlineStyle {
        InlineStyle::default()
    }

    // ----- blocks -----

    #[test]
    fn quote_lines_form_one_quote_block_with_paragraph_breaks() {
        assert_eq!(
            parse_blocks("> hello\n> world\n>\n> second"),
            vec![Block::Quote(vec![p("hello\nworld"), p("second")])]
        );
        // Grok Bot's shape: a quote, then the reply under it.
        assert_eq!(
            parse_blocks("> hello\nnot quoted"),
            vec![Block::Quote(vec![p("hello")]), p("not quoted")]
        );
        assert_eq!(
            parse_blocks("said:\n\n> quoted\n\nreply"),
            vec![p("said:"), Block::Quote(vec![p("quoted")]), p("reply")]
        );
    }

    #[test]
    fn quotes_nest_and_hold_any_block() {
        assert_eq!(
            parse_blocks("> outer\n>> inner\n> > inner too\n> back out"),
            vec![Block::Quote(vec![
                p("outer"),
                Block::Quote(vec![p("inner\ninner too")]),
                p("back out"),
            ])]
        );
        assert_eq!(
            parse_blocks("> # Title\n> - a\n> - b\n> ```\n> x = 1\n> ```"),
            vec![Block::Quote(vec![
                Block::Heading {
                    level: 1,
                    text: "Title".into()
                },
                Block::Bullets(vec!["a".into(), "b".into()]),
                Block::Code {
                    lang: String::new(),
                    code: "x = 1".into()
                },
            ])]
        );
        assert_eq!(quote_depth(&parse_blocks("> a\n>> b\n>>> c")), 3);
        // Deeper than four stays text inside the fourth.
        let deep = parse_blocks(">>>>> five");
        assert_eq!(quote_depth(&deep), 4);
    }

    #[test]
    fn a_lone_or_glued_angle_bracket_is_prose() {
        for text in [
            ">",
            "> ",
            ">5 means more than five",
            "a > b",
            "x\n>",
            ">>",
            "5 > 3 > 1",
        ] {
            let blocks = parse_blocks(text);
            assert!(
                blocks.iter().all(|b| matches!(b, Block::Paragraph(_))),
                "{text:?} → {blocks:?}"
            );
        }
        assert_eq!(
            parse_blocks("> 5 means five"),
            vec![Block::Quote(vec![p("5 means five")])]
        );
        // Indented by up to three spaces still quotes; four is literal.
        assert_eq!(parse_blocks("   > q"), vec![Block::Quote(vec![p("q")])]);
        assert_eq!(parse_blocks("    > q"), vec![p("> q")]);
    }

    #[test]
    fn headings_need_a_space_and_clamp_at_three() {
        assert_eq!(
            parse_blocks("# One\n## Two\n### Three\n#### Four"),
            vec![
                Block::Heading {
                    level: 1,
                    text: "One".into()
                },
                Block::Heading {
                    level: 2,
                    text: "Two".into()
                },
                Block::Heading {
                    level: 3,
                    text: "Three".into()
                },
                Block::Heading {
                    level: 3,
                    text: "Four".into()
                },
            ]
        );
        assert_eq!(
            parse_blocks("#hashtag and #1 fan"),
            vec![p("#hashtag and #1 fan")]
        );
        assert_eq!(parse_blocks("#"), vec![p("#")]);
        assert_eq!(parse_blocks("# "), vec![p("#")]);
    }

    #[test]
    fn lists_need_a_marker_and_a_space() {
        assert_eq!(
            parse_blocks("- a\n* b\n+ c\n• d"),
            vec![Block::Bullets(vec![
                "a".into(),
                "b".into(),
                "c".into(),
                "d".into()
            ])]
        );
        assert_eq!(
            parse_blocks("1. one\n2) two\n10. ten"),
            vec![Block::Numbered(vec![
                "one".into(),
                "two".into(),
                "ten".into()
            ])]
        );
        assert_eq!(
            parse_blocks("-5 degrees\n*bold* start"),
            vec![p("-5 degrees\n*bold* start")]
        );
        assert_eq!(
            parse_blocks("2024. A year"),
            vec![Block::Numbered(vec!["A year".into()])]
        );
        assert_eq!(parse_blocks("- "), vec![p("-")]);
        assert_eq!(parse_blocks("1."), vec![p("1.")]);
        assert_eq!(parse_blocks("3.14 is pi"), vec![p("3.14 is pi")]);
    }

    #[test]
    fn fences_keep_code_verbatim_and_carry_the_tag() {
        assert_eq!(
            parse_blocks("before\n```python\nx = **not bold**\n> not a quote\n```\nafter"),
            vec![
                p("before"),
                Block::Code {
                    lang: "python".into(),
                    code: "x = **not bold**\n> not a quote".into()
                },
                p("after"),
            ]
        );
        // Never closed: the rest of the message is the code (streaming).
        assert_eq!(
            parse_blocks("```\nopen"),
            vec![Block::Code {
                lang: String::new(),
                code: "open".into()
            }]
        );
        assert!(has_fenced_code("a\n```\nb"));
        assert!(!has_fenced_code("a `b` c"));
    }

    #[test]
    fn tables_and_display_math_still_parse() {
        assert_eq!(
            parse_blocks("| a | b |\n|---|---|\n| 1 | 2 |"),
            vec![Block::Table {
                header: vec!["a".into(), "b".into()],
                rows: vec![vec!["1".into(), "2".into()]]
            }]
        );
        assert_eq!(
            parse_blocks("$$\nE = mc^2\n$$"),
            vec![Block::Math {
                source: "E = mc^2".into()
            }]
        );
        assert_eq!(parse_blocks("$$\nx^2\n"), vec![p("$$\nx^2")]);
    }

    #[test]
    fn paragraphs_keep_soft_newlines() {
        assert_eq!(
            parse_blocks("one\ntwo\n\nthree"),
            vec![p("one\ntwo"), p("three")]
        );
    }

    // ----- inline -----

    #[test]
    fn bold_italic_strike_and_code() {
        assert_eq!(
            styles("a **b** c"),
            vec![
                ("a ".into(), plain()),
                ("b".into(), bold()),
                (" c".into(), plain())
            ]
        );
        assert_eq!(
            styles("*i* and _i_"),
            vec![
                ("i".into(), italic()),
                (" and ".into(), plain()),
                ("i".into(), italic())
            ]
        );
        assert_eq!(styles("~~gone~~"), vec![("gone".into(), strike())]);
        assert_eq!(
            styles("***both***"),
            vec![(
                "both".into(),
                InlineStyle {
                    bold: true,
                    italic: true,
                    ..Default::default()
                }
            )]
        );
        assert_eq!(
            styles("**bold *and italic* inside**"),
            vec![
                ("bold ".into(), bold()),
                (
                    "and italic".into(),
                    InlineStyle {
                        bold: true,
                        italic: true,
                        ..Default::default()
                    }
                ),
                (" inside".into(), bold()),
            ]
        );
        assert_eq!(
            styles("run `cargo test` now"),
            vec![
                ("run ".into(), plain()),
                ("cargo test".into(), code()),
                (" now".into(), plain())
            ]
        );
        // Markers inside code are code; a double-backtick span holds a backtick.
        assert_eq!(styles("`**x**`"), vec![("**x**".into(), code())]);
        assert_eq!(styles("`` a`b ``"), vec![("a`b".into(), code())]);
        // Punctuation around markers is fine.
        assert_eq!(
            styles("(**bold**), \"*it*\"!"),
            vec![
                ("(".into(), plain()),
                ("bold".into(), bold()),
                ("), \"".into(), plain()),
                ("it".into(), italic()),
                ("\"!".into(), plain()),
            ]
        );
    }

    #[test]
    fn spoilers_are_numbered_per_text() {
        let spans = inline_spans("||one|| and ||**two**||");
        assert_eq!(spans[0].text, "one");
        assert_eq!(spans[0].style.spoiler, 1);
        assert_eq!(spans[2].text, "two");
        assert_eq!(spans[2].style.spoiler, 2);
        assert!(spans[2].style.bold);
        assert_eq!(inline_plain("||one|| and ||**two**||"), "one and two");
        // `||` as "or", or glued to a word, is text.
        assert_eq!(styles("a || b"), vec![("a || b".into(), plain())]);
        assert_eq!(styles("x||y||z"), vec![("x||y||z".into(), plain())]);
    }

    #[test]
    fn links_named_bare_and_www() {
        let spans =
            inline_spans("see [the docs](https://docs.arbos.life/a_b) or https://x.com/a_b. ok");
        assert_eq!(spans[1].text, "the docs");
        assert!(spans[1].style.link);
        assert_eq!(
            spans[1].href.as_deref(),
            Some("https://docs.arbos.life/a_b")
        );
        assert_eq!(spans[3].text, "https://x.com/a_b");
        assert_eq!(spans[3].href.as_deref(), Some("https://x.com/a_b"));
        assert_eq!(spans[4].text, ". ok");
        let www = inline_spans("go to www.example.com/path_x now");
        assert_eq!(www[1].text, "www.example.com/path_x");
        assert_eq!(
            www[1].href.as_deref(),
            Some("https://www.example.com/path_x")
        );
        // A styled label keeps the link on every run.
        let styled = inline_spans("[**bold** link](https://a.com)");
        assert!(styled
            .iter()
            .all(|s| s.href.as_deref() == Some("https://a.com")));
        assert!(styled[0].style.bold && styled[0].style.link);
        // Wikipedia-style parentheses in the target.
        let wiki = inline_spans("[x](https://en.wikipedia.org/wiki/Rust_(programming_language)) y");
        assert_eq!(
            wiki[0].href.as_deref(),
            Some("https://en.wikipedia.org/wiki/Rust_(programming_language)")
        );
        assert_eq!(wiki[1].text, " y");
        // A bare URL in parentheses leaves the closing one to the prose.
        let paren = inline_spans("(https://a.com/x)");
        assert_eq!(paren[1].text, "https://a.com/x");
        assert_eq!(paren[2].text, ")");
        // Only http(s), mailto and arbos targets are links.
        assert_eq!(
            styles("[x](javascript:alert(1))"),
            vec![("[x](javascript:alert(1))".into(), plain())]
        );
        assert_eq!(
            styles("[x](data:text/html,hi)"),
            vec![("[x](data:text/html,hi)".into(), plain())]
        );
        assert_eq!(
            styles("[x](relative/path)"),
            vec![("[x](relative/path)".into(), plain())]
        );
        assert_eq!(
            inline_spans("[mail](mailto:a@b.co)")[0].href.as_deref(),
            Some("mailto:a@b.co")
        );
        assert_eq!(
            styles("[brackets] alone (parens)"),
            vec![("[brackets] alone (parens)".into(), plain())]
        );
    }

    #[test]
    fn backslash_escapes_the_markers() {
        assert_eq!(inline_plain("\\*not italic\\*"), "*not italic*");
        assert!(inline_spans("\\*not italic\\*")
            .iter()
            .all(|s| s.style == plain()));
        assert_eq!(
            inline_plain("\\~\\~x\\~\\~ \\|\\|y\\|\\| \\`z\\` \\[a](b) \\\\ \\*\\*b\\*\\*"),
            "~~x~~ ||y|| `z` [a](b) \\ **b**"
        );
        assert_eq!(inline_plain("escaped \\$5 stays"), "escaped $5 stays");
        // A backslash before a letter or a space is itself.
        assert_eq!(
            inline_plain("C:\\path\\new file\\ x"),
            "C:\\path\\new file\\ x"
        );
        assert_eq!(parse_blocks("\\> not a quote"), vec![p("\\> not a quote")]);
        assert_eq!(inline_plain("\\> not a quote"), "> not a quote");
        assert_eq!(
            parse_blocks("\\# not a heading"),
            vec![p("\\# not a heading")]
        );
    }

    #[test]
    fn plain_text_is_never_mangled() {
        for text in [
            "snake_case_word and __init__ and some_var_name",
            "2*3*4 = 24 and 5 * 3 = 15",
            "a*b and a_b and a~b and a|b",
            "https://example.com/some_path_with_underscores_and*stars*",
            "jacob_s@example.com wrote",
            "5 > 3 and 3 < 5",
            "<b>html</b> stays <i>text</i> <script>alert(1)</script>",
            "I paid $5 and $10 for 2 items",
            "** not bold ** and * not italic *",
            "unterminated **bold and *italic and ~~strike and ||spoiler",
            "a ** b and a ~~ b and a || b",
            "trailing* and *leading",
            "**",
            "*",
            "_",
            "~~",
            "||",
            "`",
            "``",
            "[not a link]",
            "[x](y",
            "(x)[y]",
            "wow!!! really??? ...",
            "C++ and C# and F#",
            "50% off, 100% sure, #1 fan, 3.14, 1/2, a+b",
            "** spaced ** stays",
            "underscores_in_the_middle_of_words",
            "*italic*text glued after stays",
            "text glued before*italic*",
            "emoji 🎉 and 👍🏽 and ✨ stay",
            "@jacob_s and @snake_case_handle",
            "path/to/file_name.rs and ~/.config",
            "a ~ b ~ c",
        ] {
            let spans = inline_spans(text);
            assert_eq!(inline_plain(text), text, "{text:?}");
            assert!(
                spans.iter().all(|s| s.style == plain() || s.style.link),
                "{text:?} → {spans:?}"
            );
            let blocks = parse_blocks(text);
            assert_eq!(blocks, vec![p(text)], "{text:?}");
        }
        assert!(is_plain("just a line\nand another"));
        assert!(!is_plain("**bold**"));
        assert!(!is_plain("> quote"));
    }

    #[test]
    fn emphasis_does_not_cross_lines_but_spoilers_may() {
        assert_eq!(styles("*a\nb*"), vec![("*a\nb*".into(), plain())]);
        let sp = inline_spans("||a\nb||");
        assert_eq!(sp[0].text, "a\nb");
        assert_eq!(sp[0].style.spoiler, 1);
    }

    #[test]
    fn math_spans_are_source_for_the_typesetter() {
        let spans = inline_spans("runs in $O(n^2)$ time; costs $5 and $10");
        assert_eq!(spans[1].text, "O(n^2)");
        assert!(spans[1].style.math);
        assert_eq!(spans[2].text, " time; costs $5 and $10");
        assert_eq!(inline_spans("\\(x^2\\)")[0].text, "x^2");
        assert_eq!(inline_spans("$$y_1$$")[0].text, "y_1");
        // `$x$` inside code is code.
        assert_eq!(styles("`$x$`"), vec![("$x$".into(), code())]);
        // Underscores and stars inside math never become emphasis.
        assert_eq!(inline_spans("$a_1 * b_2$")[0].text, "a_1 * b_2");
    }

    #[test]
    fn block_plain_text_carries_markers_for_lists_only() {
        let blocks = parse_blocks("> **q**\n> - a\n\n1. x\n2. y");
        assert_eq!(blocks_plain(&blocks), "q\n• a\n1. x\n2. y");
    }

    // ----- property: plain text round-trips -----

    /// xorshift64*: deterministic, no dev-dependency.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
        fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
            items[self.below(items.len())]
        }
    }

    /// Words that carry marker characters inside them, where the grammar
    /// must leave them alone, mixed with ordinary words. None of these can
    /// pair with another to open a span.
    const SAFE_WORDS: &[&str] = &[
        "hello",
        "world",
        "snake_case",
        "__init__",
        "a_b_c",
        "2*3*4",
        "x*y",
        "5*5=25",
        "a~b",
        "x||y",
        "a|b",
        "C++",
        "C#",
        "#1",
        "100%",
        "3.14",
        "1/2",
        "a+b",
        "jacob_s@example.com",
        "me@x.co",
        "https://example.com/a_b*c~d",
        "www.site.com/p_q",
        "(parens)",
        "[brackets]",
        "{braces}",
        "<tag>",
        "</tag>",
        "&amp;",
        "it's",
        "\"quoted\"",
        "'single'",
        "emoji🎉",
        "👍🏽",
        "ü",
        "日本語",
        "@handle_x",
        "US$3",
        "~/.config",
        "a/b/c",
        "file_name.rs",
        "x:",
        "y;",
        "z,",
        "w.",
        "v!",
        "u?",
        "...",
        "—",
        "–",
        "5",
        "<",
        "=",
        ">=",
        "<=",
        "=>",
        "::",
        "..",
        "mid*dle",
        "mid_dle",
        "mid~dle",
        "mid|dle",
        "a~~b",
        "a**b",
        "a__b",
    ];

    /// Words that are a marker, or end or start with one: alone in a text they
    /// are still plain; two of them could pair up, so a text gets one at most.
    const EDGE_WORDS: &[&str] = &[
        ">",
        "#",
        "-",
        "+",
        "1.",
        "1)",
        "*",
        "_",
        "~",
        "|",
        "\\",
        "`",
        "**",
        "~~",
        "||",
        "$5",
        "trailing*",
        "*leading",
        "mid`dle",
        "mid$dle",
        "->",
        "trailing_",
        "_leading",
        "x~~",
        "~~x",
        "||x",
        "x||",
    ];

    /// Block markers and marker words that must not start a line to stay prose.
    fn starts_a_block(word: &str) -> bool {
        matches!(word, ">" | "#" | "-" | "+" | "1." | "1)" | "*")
    }

    /// Build a text from safe words plus at most one edge word, separated by
    /// spaces and newlines.
    fn plain_text(rng: &mut Rng) -> String {
        let mut out = String::new();
        let lines = 1 + rng.below(4);
        let edge = (rng.below(2) == 0).then(|| rng.pick(EDGE_WORDS));
        let edge_at = rng.below(32);
        let mut n = 0usize;
        for l in 0..lines {
            if l > 0 {
                out.push('\n');
            }
            let words = 1 + rng.below(8);
            for w in 0..words {
                if w > 0 {
                    out.push(' ');
                }
                let mut word = match edge {
                    Some(e) if n == edge_at => e,
                    _ => rng.pick(SAFE_WORDS),
                };
                if w == 0 && starts_a_block(word) {
                    word = "hello";
                }
                out.push_str(word);
                n += 1;
            }
        }
        out
    }

    #[test]
    fn property_plain_text_round_trips_through_the_parser() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..4_000 {
            let text = plain_text(&mut rng);
            // Every line is a paragraph line; no block structure appears.
            let blocks = parse_blocks(&text);
            assert_eq!(blocks.len(), 1, "{text:?} → {blocks:?}");
            let Block::Paragraph(src) = &blocks[0] else {
                panic!("{text:?} → {blocks:?}");
            };
            assert_eq!(src, &text, "paragraph source");
            let spans = inline_spans(src);
            let plain: String = spans.iter().map(|s| s.text.as_str()).collect();
            assert_eq!(plain, text, "{text:?} → {spans:?}");
            for s in &spans {
                let mut style = s.style;
                style.link = false;
                assert_eq!(style, InlineStyle::default(), "{text:?} → {s:?}");
                if s.style.link {
                    assert!(
                        s.text.starts_with("https://") || s.text.starts_with("www."),
                        "{text:?} → {s:?}"
                    );
                }
            }
        }
    }

    /// Any input: the parser never panics, and the rendered text only ever
    /// drops marker characters — every output character comes from the input
    /// in order (math and code spans included; list markers excluded).
    #[test]
    fn property_any_text_parses_and_only_loses_markers() {
        const ALPHABET: &[char] = &[
            'a', 'b', ' ', ' ', '\n', '*', '_', '~', '|', '`', '[', ']', '(', ')', '>', '#', '-',
            '.', '1', '$', '\\', 'h', 't', 'p', ':', '/', 'w', '"', '<', '!', '@', 'é', '🎉',
        ];
        let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
        for _ in 0..20_000 {
            let len = rng.below(40);
            let text: String = (0..len)
                .map(|_| ALPHABET[rng.below(ALPHABET.len())])
                .collect();
            let blocks = parse_blocks(&text);
            let mut rendered = String::new();
            fn collect(blocks: &[Block], out: &mut String) {
                for b in blocks {
                    match b {
                        Block::Paragraph(t) | Block::Heading { text: t, .. } => {
                            out.push_str(&inline_plain(t))
                        }
                        Block::Quote(inner) => collect(inner, out),
                        Block::Bullets(items) | Block::Numbered(items) => {
                            items.iter().for_each(|i| out.push_str(&inline_plain(i)))
                        }
                        Block::Code { code, .. } => out.push_str(code),
                        Block::Table { header, rows } => std::iter::once(header)
                            .chain(rows.iter())
                            .flatten()
                            .for_each(|c| out.push_str(&inline_plain(c))),
                        Block::Math { source } => out.push_str(source),
                    }
                }
            }
            collect(&blocks, &mut rendered);
            // Subsequence check, ignoring whitespace (the parser trims lines)
            // and the `https://` a `www.` link gains as its href, never its text.
            let mut input = text.chars().filter(|c| !c.is_whitespace());
            for c in rendered.chars().filter(|c| !c.is_whitespace()) {
                assert!(
                    input.any(|i| i == c),
                    "{text:?} rendered {rendered:?}: {c:?} is not from the input in order"
                );
            }
        }
    }
}
