//! The Mac's view of the shared chat Markdown parser
//! (`arbos_core::markdown`, [feedback chat-markdown-mac-2026-10-27]): the same
//! blocks and inline runs the iPhone renders, plus what only this renderer
//! needs — typeset math runs, the named link's icon cell, URL wrap hints, and
//! the chart card as one more block kind.

use arbos_core::markdown as core;

use super::math;
use super::urlwrap;
use crate::model::LinkPreview;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Block {
    Paragraph(String),
    Heading {
        level: u8,
        text: String,
    },
    /// `> ` lines: a quote holds any block, including another quote.
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
    /// A link preview card ([feedback link-embeds-mac-2026-10-27]): the relay's
    /// `link_preview` part as one block of the bubble, under the prose that held the link.
    LinkPreview {
        part_id: String,
        preview: Box<LinkPreview>,
    },
}

impl From<core::Block> for Block {
    fn from(block: core::Block) -> Self {
        match block {
            core::Block::Paragraph(text) => Block::Paragraph(text),
            core::Block::Heading { level, text } => Block::Heading { level, text },
            core::Block::Quote(inner) => Block::Quote(inner.into_iter().map(Into::into).collect()),
            core::Block::Bullets(items) => Block::Bullets(items),
            core::Block::Numbered(items) => Block::Numbered(items),
            core::Block::Code { lang, code } => Block::Code { lang, code },
            core::Block::Table { header, rows } => Block::Table { header, rows },
            core::Block::Math { source } => Block::Math { source },
        }
    }
}

/// Split a message into blocks (the shared grammar, `docs/mac-design-rules.md`
/// § Message formatting).
pub fn split_blocks(source: &str) -> Vec<Block> {
    core::parse_blocks(source)
        .into_iter()
        .map(Into::into)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct InlineStyle {
    pub code: bool,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub link: bool,
    /// Typeset TeX: painted in the math face; `italic` marks its variables,
    /// `bold` its `\mathbf`.
    pub math: bool,
    /// 0 outside a spoiler, else the spoiler's 1-based index in its text: the
    /// run is hidden until the viewer clicks it.
    pub spoiler: u16,
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
    fn push(t: &str, out: &mut Vec<String>) {
        for span in inline_spans(t) {
            if let Some(h) = span.icon {
                if !out.contains(&h) {
                    out.push(h);
                }
            }
        }
    }
    fn walk(blocks: &[Block], out: &mut Vec<String>) {
        for block in blocks {
            match block {
                Block::Paragraph(t) | Block::Heading { text: t, .. } => push(t, out),
                Block::Quote(inner) => walk(inner, out),
                Block::Bullets(items) | Block::Numbered(items) => {
                    items.iter().for_each(|i| push(i, out))
                }
                Block::Table { header, rows } => {
                    header.iter().for_each(|c| push(c, out));
                    rows.iter().flatten().for_each(|c| push(c, out));
                }
                Block::Code { .. }
                | Block::Math { .. }
                | Block::Chart { .. }
                | Block::LinkPreview { .. } => {}
            }
        }
    }
    walk(&split_blocks(text), &mut out);
    out
}

/// Split inline markdown into styled spans: the shared runs, with math typeset
/// into its italic / upright / bold pieces, a named link's icon cell prepended
/// to its first run, and wrap hints inside addresses.
pub fn inline_spans(text: &str) -> Vec<Span> {
    let runs = core::inline_spans(text);
    let mut spans: Vec<Span> = Vec::with_capacity(runs.len());
    let mut i = 0;
    while i < runs.len() {
        let run = &runs[i];
        let style = InlineStyle {
            code: run.style.code,
            bold: run.style.bold,
            italic: run.style.italic,
            strike: run.style.strike,
            link: run.style.link,
            math: run.style.math,
            spoiler: run.style.spoiler,
        };
        if run.style.math {
            for piece in math::typeset(&run.text) {
                spans.push(Span {
                    text: piece.text,
                    style: InlineStyle {
                        italic: piece.style == math::MathStyle::Italic,
                        bold: piece.style == math::MathStyle::Bold,
                        ..style
                    },
                    href: run.href.clone(),
                    icon: None,
                });
            }
            i += 1;
            continue;
        }
        let Some(href) = run.href.as_deref().filter(|_| run.style.link) else {
            spans.push(Span {
                text: run.text.clone(),
                style,
                href: None,
                icon: None,
            });
            i += 1;
            continue;
        };
        // The runs of one link share its href; the first carries the cell.
        let mut end = i + 1;
        while end < runs.len()
            && runs[end].style.link
            && runs[end].href.as_deref() == Some(href)
            && !runs[end].style.math
        {
            end += 1;
        }
        let label: String = runs[i..end].iter().map(|r| r.text.as_str()).collect();
        let icon = (!label_is_url(&label)).then(|| link_host(href)).flatten();
        for (k, r) in runs[i..end].iter().enumerate() {
            let text = if k == 0 && icon.is_some() {
                format!("{LINK_ICON_CELL}{}", r.text)
            } else if label_is_url(&label) {
                // An address as the label wraps like a bare URL.
                urlwrap::breakable(&r.text)
            } else {
                r.text.clone()
            };
            spans.push(Span {
                text,
                style: InlineStyle {
                    code: r.style.code,
                    bold: r.style.bold,
                    italic: r.style.italic,
                    strike: r.style.strike,
                    link: true,
                    math: false,
                    spoiler: r.style.spoiler,
                },
                href: Some(href.to_owned()),
                icon: if k == 0 { icon.clone() } else { None },
            });
        }
        i = end;
    }
    spans
}

/// The text `inline_spans` renders, markers removed: what the user sees and
/// what a selection copies.
pub fn inline_plain(text: &str) -> String {
    inline_spans(text).into_iter().map(|s| s.text).collect()
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
