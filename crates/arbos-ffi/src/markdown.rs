//! The shared chat Markdown parser for Swift (`arbos_core::markdown`): the
//! iPhone splits a message into blocks and inline runs with exactly the
//! grammar the Mac renders, so `> quote`, `**bold**`, `||spoiler||` and the
//! never-mangled plain cases agree on both ([feedback
//! chat-markdown-ios-2026-10-27]).
//!
//! Quotes nest, which UniFFI enums cannot carry directly, so the block list is
//! flat and every block names its `quote_depth` (0 = not quoted): consecutive
//! blocks at depth ≥ 1 are one quote, blocks at depth ≥ 2 inside it a nested
//! one, and so on — the same tree `arbos_core::markdown::Block::Quote` holds.

use arbos_core::markdown as core;

/// One block of a message, flattened (see the module doc for `quote_depth`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum MarkdownBlock {
    Paragraph {
        quote_depth: u8,
        text: String,
    },
    Heading {
        quote_depth: u8,
        level: u8,
        text: String,
    },
    Bullets {
        quote_depth: u8,
        items: Vec<String>,
    },
    Numbered {
        quote_depth: u8,
        items: Vec<String>,
    },
    Code {
        quote_depth: u8,
        lang: String,
        code: String,
    },
    Table {
        quote_depth: u8,
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Math {
        quote_depth: u8,
        source: String,
    },
}

fn flatten(blocks: &[core::Block], depth: u8, out: &mut Vec<MarkdownBlock>) {
    for block in blocks {
        match block {
            core::Block::Paragraph(text) => out.push(MarkdownBlock::Paragraph {
                quote_depth: depth,
                text: text.clone(),
            }),
            core::Block::Heading { level, text } => out.push(MarkdownBlock::Heading {
                quote_depth: depth,
                level: *level,
                text: text.clone(),
            }),
            core::Block::Quote(inner) => flatten(inner, depth.saturating_add(1), out),
            core::Block::Bullets(items) => out.push(MarkdownBlock::Bullets {
                quote_depth: depth,
                items: items.clone(),
            }),
            core::Block::Numbered(items) => out.push(MarkdownBlock::Numbered {
                quote_depth: depth,
                items: items.clone(),
            }),
            core::Block::Code { lang, code } => out.push(MarkdownBlock::Code {
                quote_depth: depth,
                lang: lang.clone(),
                code: code.clone(),
            }),
            core::Block::Table { header, rows } => out.push(MarkdownBlock::Table {
                quote_depth: depth,
                header: header.clone(),
                rows: rows.clone(),
            }),
            core::Block::Math { source } => out.push(MarkdownBlock::Math {
                quote_depth: depth,
                source: source.clone(),
            }),
        }
    }
}

/// The blocks of `text`, in order, quotes flattened to `quote_depth`.
#[uniffi::export]
pub fn markdown_blocks(text: String) -> Vec<MarkdownBlock> {
    let mut out = Vec::new();
    flatten(&core::parse_blocks(&text), 0, &mut out);
    out
}

/// One inline run of a paragraph, heading, list item or table cell: the text
/// with its markers removed, and its styles. `math` runs carry TeX source for
/// `MathText`; `spoiler` is 0 outside a spoiler, else the 1-based index of the
/// `||…||` within the text (one tap reveals every run with that index).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MarkdownSpan {
    pub text: String,
    pub href: Option<String>,
    pub code: bool,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub link: bool,
    pub math: bool,
    pub spoiler: u16,
}

impl From<core::Span> for MarkdownSpan {
    fn from(s: core::Span) -> Self {
        Self {
            text: s.text,
            href: s.href,
            code: s.style.code,
            bold: s.style.bold,
            italic: s.style.italic,
            strike: s.style.strike,
            link: s.style.link,
            math: s.style.math,
            spoiler: s.style.spoiler,
        }
    }
}

/// The inline runs of one block's text.
#[uniffi::export]
pub fn markdown_spans(text: String) -> Vec<MarkdownSpan> {
    core::inline_spans(&text)
        .into_iter()
        .map(Into::into)
        .collect()
}

/// The text `markdown_spans` renders, markers removed (math as its source).
#[uniffi::export]
pub fn markdown_plain(text: String) -> String {
    core::inline_plain(&text)
}

/// The plain text of a whole message — what a copy of the bubble carries.
#[uniffi::export]
pub fn markdown_message_plain(text: String) -> String {
    core::blocks_plain(&core::parse_blocks(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_flatten_to_depths() {
        let blocks = markdown_blocks("> a\n>> b\n> c\nd".into());
        assert_eq!(
            blocks,
            vec![
                MarkdownBlock::Paragraph {
                    quote_depth: 1,
                    text: "a".into()
                },
                MarkdownBlock::Paragraph {
                    quote_depth: 2,
                    text: "b".into()
                },
                MarkdownBlock::Paragraph {
                    quote_depth: 1,
                    text: "c".into()
                },
                MarkdownBlock::Paragraph {
                    quote_depth: 0,
                    text: "d".into()
                },
            ]
        );
    }

    #[test]
    fn spans_carry_every_style() {
        let spans = markdown_spans("**b** _i_ ~~s~~ `c` ||sp|| [l](https://a.com) $x$".into());
        let find = |t: &str| spans.iter().find(|s| s.text == t).cloned().unwrap();
        assert!(find("b").bold);
        assert!(find("i").italic);
        assert!(find("s").strike);
        assert!(find("c").code);
        assert_eq!(find("sp").spoiler, 1);
        assert_eq!(find("l").href.as_deref(), Some("https://a.com"));
        assert!(find("x").math);
        assert_eq!(markdown_plain("**b** snake_case".into()), "b snake_case");
        assert_eq!(markdown_message_plain("> **q**\n- a".into()), "q\n• a");
    }
}
