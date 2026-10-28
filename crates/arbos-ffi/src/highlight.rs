//! Syntax highlighting over the FFI: the Swift code card calls
//! [`highlight_code`] and paints the runs with its theme's palette, so the
//! iPhone colours a fenced block exactly as the Mac does
//! ([feedback code-highlighting-ios-2026-10-26]).

use arbos_core::highlight as core;

/// Mirrors [`arbos_core::highlight::TokenKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum HighlightKind {
    Plain,
    Keyword,
    Function,
    String,
    Number,
    Comment,
}

impl From<core::TokenKind> for HighlightKind {
    fn from(k: core::TokenKind) -> Self {
        match k {
            core::TokenKind::Plain => Self::Plain,
            core::TokenKind::Keyword => Self::Keyword,
            core::TokenKind::Function => Self::Function,
            core::TokenKind::String => Self::String,
            core::TokenKind::Number => Self::Number,
            core::TokenKind::Comment => Self::Comment,
        }
    }
}

/// One run of the code: UTF-8 byte offsets `start..end` (contiguous, covering
/// the whole text, merged by kind).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HighlightToken {
    pub start: u32,
    pub end: u32,
    pub kind: HighlightKind,
}

/// The runs of `code` for the fence tag `lang` (empty tag → detected from the
/// code); an empty list means the block stays plain.
#[uniffi::export]
pub fn highlight_code(lang: String, code: String) -> Vec<HighlightToken> {
    core::highlight(&lang, &code)
        .into_iter()
        .map(|t| HighlightToken {
            start: t.start as u32,
            end: t.end as u32,
            kind: t.kind.into(),
        })
        .collect()
}

/// The canonical tag of the language a block is read as, or `None` for plain.
#[uniffi::export]
pub fn detect_code_language(lang: String, code: String) -> Option<String> {
    core::detect(&lang, &code).map(|l| l.tag().to_string())
}

/// Load the grammars (≈ 15 ms) — call once from a background task at launch.
#[uniffi::export]
pub fn warm_highlighter() {
    core::warm();
}
