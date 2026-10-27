//! Syntax highlighting for fenced code blocks, shared by the Mac app and (through
//! `arbos-ffi`) the iPhone app, so both colour the same code the same way
//! ([feedback code-highlighting-mac-2026-10-26] / [feedback code-highlighting-ios-2026-10-26]).
//!
//! Grammars: syntect's default Sublime pack (one 360 KB dump in the binary, loaded
//! on first use — [`warm`] does it off the main thread) for Python, JavaScript,
//! Rust, Go, shell, JSON, SQL, C, C++, YAML, HTML, CSS and Markdown; TypeScript
//! rides on the JavaScript grammar; Swift (not in the pack) has a small lexer here.
//! Every scope collapses to six token kinds — the six colours Grok Bot's code cards
//! use — and the apps map kinds to their theme's palette.
//!
//! Language: the fence tag ([`language_for_tag`]), else the first line (shebang,
//! `<!DOCTYPE`), else a keyword score over the code, else plain text.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

/// How a run of code is painted. The order is the palette's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// Identifiers, operators, brackets, and `in` / `and` / `or` / `is` / `as`.
    Plain,
    /// Control and declaration keywords, storage types, `true` / `false` / `None`.
    Keyword,
    /// Function and macro names at definition and call, type names, tag attributes.
    Function,
    /// String literals including their quotes and escapes.
    String,
    /// Numeric literals.
    Number,
    /// Comments (painted italic).
    Comment,
}

/// One run of `code`: `start..end` are byte offsets, runs are contiguous and
/// cover the whole text, adjacent runs of one kind are merged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

/// The languages the apps colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Python,
    JavaScript,
    TypeScript,
    Swift,
    Rust,
    Go,
    Shell,
    Json,
    Sql,
    C,
    Cpp,
    Yaml,
    Html,
    Css,
    Markdown,
}

impl Language {
    pub const ALL: [Language; 15] = [
        Language::Python,
        Language::JavaScript,
        Language::TypeScript,
        Language::Swift,
        Language::Rust,
        Language::Go,
        Language::Shell,
        Language::Json,
        Language::Sql,
        Language::C,
        Language::Cpp,
        Language::Yaml,
        Language::Html,
        Language::Css,
        Language::Markdown,
    ];

    /// The canonical fence tag.
    pub fn tag(self) -> &'static str {
        match self {
            Language::Python => "python",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
            Language::Swift => "swift",
            Language::Rust => "rust",
            Language::Go => "go",
            Language::Shell => "bash",
            Language::Json => "json",
            Language::Sql => "sql",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Yaml => "yaml",
            Language::Html => "html",
            Language::Css => "css",
            Language::Markdown => "markdown",
        }
    }

    /// The syntect syntax scope that paints this language; `None` for the lexer.
    fn scope(self) -> Option<&'static str> {
        Some(match self {
            Language::Python => "source.python",
            Language::JavaScript | Language::TypeScript => "source.js",
            Language::Swift => return None,
            Language::Rust => "source.rust",
            Language::Go => "source.go",
            Language::Shell => "source.shell.bash",
            Language::Json => "source.json",
            Language::Sql => "source.sql",
            Language::C => "source.c",
            Language::Cpp => "source.c++",
            Language::Yaml => "source.yaml",
            Language::Html => "text.html.basic",
            Language::Css => "source.css",
            Language::Markdown => "text.html.markdown",
        })
    }
}

/// The language a fence tag names (`py`, `ts`, `sh`, `yml`, `c++` …), case-insensitive.
pub fn language_for_tag(tag: &str) -> Option<Language> {
    let t = tag.trim().to_ascii_lowercase();
    let t = t.split([' ', '{', ':', ',']).next().unwrap_or("");
    Some(match t {
        "python" | "py" | "py3" | "python3" | "pyi" => Language::Python,
        "javascript" | "js" | "jsx" | "mjs" | "cjs" | "node" => Language::JavaScript,
        "typescript" | "ts" | "tsx" | "mts" | "cts" => Language::TypeScript,
        "swift" => Language::Swift,
        "rust" | "rs" => Language::Rust,
        "go" | "golang" => Language::Go,
        "bash" | "sh" | "shell" | "zsh" | "fish" | "console" | "shellscript" | "terminal" => {
            Language::Shell
        }
        "json" | "jsonc" | "json5" => Language::Json,
        "sql" | "mysql" | "postgres" | "postgresql" | "pgsql" | "sqlite" | "plsql" | "tsql" => {
            Language::Sql
        }
        "c" | "h" => Language::C,
        "cpp" | "c++" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "objective-c++" => Language::Cpp,
        "yaml" | "yml" => Language::Yaml,
        "html" | "htm" | "xhtml" | "xml" | "svg" | "vue" => Language::Html,
        "css" | "scss" | "less" => Language::Css,
        "markdown" | "md" | "mdx" => Language::Markdown,
        _ => return None,
    })
}

/// The language of a fenced block: the tag, else the first line, else a keyword
/// score over the code (needs a clear winner), else `None` — plain text.
pub fn detect(tag: &str, code: &str) -> Option<Language> {
    if let Some(lang) = language_for_tag(tag) {
        return Some(lang);
    }
    if let Some(lang) = language_for_first_line(code) {
        return Some(lang);
    }
    language_by_keywords(code)
}

/// Shebangs and document openers.
fn language_for_first_line(code: &str) -> Option<Language> {
    let first = code.lines().find(|l| !l.trim().is_empty())?.trim();
    if let Some(rest) = first.strip_prefix("#!") {
        let l = rest.to_ascii_lowercase();
        return if l.contains("python") {
            Some(Language::Python)
        } else if l.contains("node") || l.contains("deno") || l.contains("bun") {
            Some(Language::JavaScript)
        } else if l.contains("swift") {
            Some(Language::Swift)
        } else if l.contains("sh") {
            Some(Language::Shell)
        } else {
            None
        };
    }
    let l = first.to_ascii_lowercase();
    if l.starts_with("<!doctype html") || l.starts_with("<html") || l.starts_with("<?xml") {
        return Some(Language::Html);
    }
    if l.starts_with("<?php") {
        return None;
    }
    None
}

/// Keyword and shape clues per language. Each hit scores one; the best language
/// wins with at least two hits and a one-point lead.
fn language_by_keywords(code: &str) -> Option<Language> {
    let trimmed = code.trim();
    if trimmed.is_empty() {
        return None;
    }
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(trimmed).is_ok()
    {
        return Some(Language::Json);
    }
    let words: Vec<&str> = code
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '#' || c == '!' || c == '$'))
        .filter(|w| !w.is_empty())
        .collect();
    let has = |w: &str| words.contains(&w);
    let has_ci = |w: &str| words.iter().any(|x| x.eq_ignore_ascii_case(w));
    let contains = |s: &str| code.contains(s);
    let mut scores: Vec<(Language, u32)> = Vec::new();
    let mut score = |lang: Language, clues: &[bool]| {
        let n = clues.iter().filter(|c| **c).count() as u32;
        scores.push((lang, n));
    };
    score(
        Language::Python,
        &[
            has("def"),
            has("elif"),
            has("self"),
            has("None"),
            has("import") && !contains(";") && !contains("{"),
            contains("print("),
            has("lambda"),
            code.lines().any(|l| l.trim_end().ends_with(':')),
        ],
    );
    score(
        Language::JavaScript,
        &[
            has("const") || has("let"),
            contains("=>"),
            has("function"),
            contains("console."),
            contains("==="),
            has("var"),
            contains("document."),
            has("null") || has("undefined"),
        ],
    );
    score(
        Language::TypeScript,
        &[
            has("interface"),
            contains(": string") || contains(": number") || contains(": boolean"),
            contains("=>"),
            has("const") || has("let"),
            has("type") && contains(" = {"),
            contains("<T>"),
            has("readonly"),
            has("enum"),
        ],
    );
    score(
        Language::Swift,
        &[
            has("func"),
            has("let") || has("var"),
            has("guard"),
            contains("\\("),
            has("struct") && contains("var "),
            contains("->") && has("func"),
            has("nil"),
            has("import") && (has("Foundation") || has("SwiftUI") || has("UIKit")),
        ],
    );
    score(
        Language::Rust,
        &[
            has("fn"),
            has("let") && has("mut"),
            contains("println!"),
            contains("::"),
            has("impl"),
            contains("->") && has("fn"),
            has("pub"),
            contains("&str") || contains("Vec<") || contains("Option<"),
        ],
    );
    score(
        Language::Go,
        &[
            has("func"),
            contains(":="),
            contains("fmt."),
            has("package"),
            has("nil") && has("err"),
            has("go") && has("func"),
            has("struct") && has("type"),
            contains("import ("),
        ],
    );
    score(
        Language::Shell,
        &[
            has("echo"),
            has("fi") || has("done") || has("esac"),
            contains("$1") || contains("$@") || contains("${"),
            contains(" | "),
            has("sudo") || has("cd") || has("ls") || has("grep") || has("export"),
            contains("&&"),
            has("then") || has("do"),
            code.lines().any(|l| l.trim_start().starts_with("$ ")),
        ],
    );
    score(
        Language::Sql,
        &[
            has_ci("select"),
            has_ci("from"),
            has_ci("where"),
            has_ci("insert") || has_ci("update") || has_ci("delete"),
            has_ci("join"),
            has_ci("table"),
            has_ci("group") && has_ci("by"),
            contains("--"),
        ],
    );
    score(
        Language::C,
        &[
            contains("#include"),
            contains("printf("),
            contains("int main"),
            has("malloc") || has("free") || has("sizeof"),
            has("struct") && contains(";"),
            contains("char *") || contains("char*"),
            has("return") && contains(";") && !contains("::"),
            has("void"),
        ],
    );
    score(
        Language::Cpp,
        &[
            contains("#include <iostream>")
                || contains("#include <vector>")
                || contains("#include <string>"),
            contains("std::"),
            contains("cout") || contains("cin"),
            has("class") && contains("::"),
            has("template"),
            has("nullptr"),
            has("namespace"),
            contains("->") && contains("::"),
        ],
    );
    score(
        Language::Yaml,
        &[
            code.lines()
                .filter(|l| {
                    let t = l.trim_start();
                    let key = t.split(':').next().unwrap_or("");
                    !key.is_empty()
                        && key
                            .chars()
                            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                        && (t[key.len()..].starts_with(": ") || t[key.len()..] == *":")
                })
                .count()
                >= 2,
            code.lines().any(|l| l.trim_start().starts_with("- ")),
            !contains("{") && !contains(";"),
            contains("---"),
        ],
    );
    score(
        Language::Html,
        &[
            contains("</"),
            contains("<div") || contains("<span") || contains("<p>") || contains("<a "),
            contains("<html") || contains("<body") || contains("<head"),
            contains("/>"),
            contains("class=\""),
            contains("<script") || contains("<style"),
            contains("<h1") || contains("<ul") || contains("<li"),
            contains("href="),
        ],
    );
    score(
        Language::Css,
        &[
            code.lines().any(|l| l.trim_end().ends_with('{'))
                && code.lines().any(|l| l.trim_end().ends_with(';')),
            contains("color:")
                || contains("margin:")
                || contains("padding:")
                || contains("display:"),
            contains("px;") || contains("rem;") || contains("em;"),
            contains("@media") || contains("@import"),
            code.lines()
                .any(|l| l.trim_start().starts_with('.') || l.trim_start().starts_with('#')),
            contains("font-") || contains("background"),
            contains(":hover") || contains("::before"),
            contains("!important"),
        ],
    );
    score(
        Language::Markdown,
        &[
            code.lines()
                .any(|l| l.starts_with("# ") || l.starts_with("## ")),
            code.lines()
                .filter(|l| l.starts_with("- ") || l.starts_with("* "))
                .count()
                >= 2,
            contains("**"),
            contains("](http"),
            code.lines().any(|l| l.starts_with("> ")),
            contains("```"),
            code.lines().any(|l| l.starts_with("| ")),
            code.lines().any(|l| l.starts_with("1. ")),
        ],
    );
    scores.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let (best, top) = scores[0];
    let runner_up = scores[1].1;
    (top >= 2 && top > runner_up).then_some(best)
}

type Cache = HashMap<(String, String), Vec<Token>>;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Load the grammars now (≈ 15 ms); call from a background thread at launch so the
/// first code card never waits for it.
pub fn warm() {
    let _ = syntax_set();
}

/// Blocks longer than this are left plain: a pasted log is not worth a regex pass.
pub const MAX_HIGHLIGHT_BYTES: usize = 64 * 1024;

/// The tokens of `code` for the language `tag` names (or, with an empty / unknown
/// tag, the language [`detect`] finds). Empty when the code stays plain. Results
/// are cached per (tag, code).
pub fn highlight(tag: &str, code: &str) -> Vec<Token> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (tag.to_string(), code.to_string());
    if let Some(hit) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }
    let tokens = highlight_uncached(tag, code);
    if let Ok(mut c) = cache.lock() {
        if c.len() > 512 {
            c.clear();
        }
        c.insert(key, tokens.clone());
    }
    tokens
}

fn highlight_uncached(tag: &str, code: &str) -> Vec<Token> {
    if code.len() > MAX_HIGHLIGHT_BYTES {
        return Vec::new();
    }
    let Some(lang) = detect(tag, code) else {
        return Vec::new();
    };
    highlight_as(lang, code)
}

/// The tokens of `code` read as `lang`.
pub fn highlight_as(lang: Language, code: &str) -> Vec<Token> {
    let tokens = match lang.scope() {
        Some(scope) => {
            let ss = syntax_set();
            let Some(syntax) = ss.find_syntax_by_scope(Scope::new(scope).expect("valid scope"))
            else {
                return Vec::new();
            };
            tokens_from_syntect(ss, syntax, code, lang)
        }
        None => lexer::tokens(code, &lexer::SWIFT),
    };
    merge(tokens, code.len())
}

/// Words every grammar would call a keyword but Grok paints plain.
const PLAIN_WORDS: [&str; 5] = ["in", "and", "or", "is", "as"];

fn tokens_from_syntect(
    ss: &SyntaxSet,
    syntax: &SyntaxReference,
    code: &str,
    lang: Language,
) -> Vec<Token> {
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out = Vec::new();
    let mut offset = 0usize;
    let html = matches!(lang, Language::Html);
    for line in code.split_inclusive('\n') {
        // The "newlines" grammars expect every line to end in `\n`; the block's
        // last line usually does not. Parse it with one and clamp the offsets.
        let owned;
        let parsed: &str = if line.ends_with('\n') {
            line
        } else {
            owned = format!("{line}\n");
            &owned
        };
        let Ok(ops) = state.parse_line(parsed, ss) else {
            // A grammar error on one line: the rest of the block stays plain.
            out.push(Token {
                start: offset,
                end: code.len(),
                kind: TokenKind::Plain,
            });
            return out;
        };
        let mut last = 0usize;
        for (i, op) in ops {
            let i = i.min(line.len());
            if i > last {
                out.push(Token {
                    start: offset + last,
                    end: offset + i,
                    kind: classify(stack.as_slice(), &line[last..i], html),
                });
            }
            let _ = stack.apply(&op);
            last = i;
        }
        if last < line.len() {
            out.push(Token {
                start: offset + last,
                end: offset + line.len(),
                kind: classify(stack.as_slice(), &line[last..], html),
            });
        }
        offset += line.len();
    }
    out
}

/// Six colours from a Sublime scope stack (innermost scope decides, comments and
/// strings win over anything nested in them).
fn classify(stack: &[Scope], text: &str, html: bool) -> TokenKind {
    let names: Vec<String> = stack.iter().map(|s| s.build_string()).collect();
    if names.iter().any(|n| n.starts_with("comment")) {
        return TokenKind::Comment;
    }
    // YAML keys are tagged inside a plain-string scope: the key wins.
    if names.iter().any(|n| n.starts_with("entity.name.tag")) {
        return if html {
            TokenKind::Keyword
        } else {
            TokenKind::Function
        };
    }
    if names.iter().any(|n| {
        n.starts_with("string")
            || n.starts_with("constant.character.escape")
            || n.starts_with("constant.other.placeholder")
            || n.starts_with("markup.raw")
    }) {
        return TokenKind::String;
    }
    let word = text.trim();
    let alphabetic = !word.is_empty() && word.chars().all(|c| c.is_alphabetic() || c == '_');
    // A name token carries its function scope above the generic-name scope.
    let named_function = names.iter().any(|n| {
        n.starts_with("entity.name.function")
            || n.starts_with("variable.function")
            || n.starts_with("support.function")
    });
    for n in names.iter().rev() {
        if n.starts_with("constant.numeric") {
            return TokenKind::Number;
        }
        if n.starts_with("constant.language") {
            return TokenKind::Keyword;
        }
        if n.starts_with("entity.name.function")
            || n.starts_with("variable.function")
            || n.starts_with("support.function")
            || n.starts_with("support.macro")
            || n.starts_with("entity.name.type")
            || n.starts_with("entity.name.class")
            || n.starts_with("entity.name.struct")
            || n.starts_with("entity.name.enum")
            || n.starts_with("entity.other.inherited-class")
            || n.starts_with("entity.other.attribute-name")
            || n.starts_with("support.type")
            || n.starts_with("support.class")
            || n.starts_with("markup.underline.link")
        {
            return TokenKind::Function;
        }
        if n.starts_with("entity.name.tag") {
            return if html {
                TokenKind::Keyword
            } else {
                TokenKind::Function
            };
        }
        if n.starts_with("entity.name.section")
            || n.starts_with("markup.heading")
            || n.starts_with("markup.bold")
            || n.starts_with("markup.italic")
        {
            return TokenKind::Keyword;
        }
        if n.starts_with("keyword.operator") {
            return if alphabetic && !PLAIN_WORDS.contains(&word) {
                TokenKind::Keyword
            } else {
                TokenKind::Plain
            };
        }
        if n.starts_with("keyword") || n.starts_with("storage") {
            return if PLAIN_WORDS.contains(&word) {
                TokenKind::Plain
            } else {
                TokenKind::Keyword
            };
        }
        if n.starts_with("meta.generic-name") || n.starts_with("meta.qualified-name") {
            let in_call = names.iter().any(|n| n.starts_with("meta.function-call"))
                && !names
                    .iter()
                    .any(|n| n.starts_with("meta.function-call.arguments"));
            return if in_call || named_function {
                TokenKind::Function
            } else {
                TokenKind::Plain
            };
        }
        if n.starts_with("variable") || n.starts_with("punctuation") {
            return TokenKind::Plain;
        }
    }
    TokenKind::Plain
}

/// Contiguous, merged, covering `0..len`.
fn merge(tokens: Vec<Token>, len: usize) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut pos = 0usize;
    for t in tokens {
        if t.start > pos {
            push_merged(
                &mut out,
                Token {
                    start: pos,
                    end: t.start,
                    kind: TokenKind::Plain,
                },
            );
        }
        if t.end > t.start {
            push_merged(&mut out, t);
        }
        pos = pos.max(t.end);
    }
    if pos < len {
        push_merged(
            &mut out,
            Token {
                start: pos,
                end: len,
                kind: TokenKind::Plain,
            },
        );
    }
    out
}

fn push_merged(out: &mut Vec<Token>, t: Token) {
    if let Some(last) = out.last_mut() {
        if last.kind == t.kind && last.end == t.start {
            last.end = t.end;
            return;
        }
    }
    out.push(t);
}

/// A table-driven lexer for languages the Sublime pack lacks (Swift).
pub mod lexer {
    use super::{Token, TokenKind};

    pub struct Table {
        pub keywords: &'static [&'static str],
        pub constants: &'static [&'static str],
        pub line_comment: &'static str,
        pub block_comment: Option<(&'static str, &'static str)>,
        /// Words before a name that make it a definition (`func`, `class`).
        pub definers: &'static [&'static str],
    }

    pub const SWIFT: Table = Table {
        keywords: &[
            "func",
            "let",
            "var",
            "if",
            "else",
            "guard",
            "return",
            "for",
            "in",
            "while",
            "repeat",
            "switch",
            "case",
            "default",
            "break",
            "continue",
            "struct",
            "class",
            "enum",
            "protocol",
            "extension",
            "import",
            "init",
            "deinit",
            "self",
            "Self",
            "super",
            "static",
            "private",
            "public",
            "internal",
            "fileprivate",
            "open",
            "final",
            "override",
            "mutating",
            "throws",
            "throw",
            "try",
            "catch",
            "async",
            "await",
            "defer",
            "where",
            "some",
            "any",
            "typealias",
            "associatedtype",
            "inout",
            "lazy",
            "weak",
            "unowned",
            "subscript",
            "operator",
            "do",
            "fallthrough",
            "rethrows",
            "indirect",
            "convenience",
            "required",
            "optional",
            "dynamic",
            "nonisolated",
            "actor",
            "is",
            "as",
            "willSet",
            "didSet",
            "get",
            "set",
        ],
        constants: &["true", "false", "nil"],
        line_comment: "//",
        block_comment: Some(("/*", "*/")),
        definers: &[
            "func",
            "class",
            "struct",
            "enum",
            "protocol",
            "extension",
            "actor",
        ],
    };

    pub fn tokens(code: &str, table: &Table) -> Vec<Token> {
        let bytes = code.as_bytes();
        let mut out = Vec::new();
        let mut i = 0usize;
        let push = |start: usize, end: usize, kind: TokenKind, out: &mut Vec<Token>| {
            if end > start {
                out.push(Token { start, end, kind });
            }
        };
        while i < bytes.len() {
            let rest = &code[i..];
            if rest.starts_with(table.line_comment) {
                let end = rest.find('\n').map(|e| i + e).unwrap_or(code.len());
                push(i, end, TokenKind::Comment, &mut out);
                i = end;
                continue;
            }
            if let Some((open, close)) = table.block_comment {
                if let Some(after) = rest.strip_prefix(open) {
                    let end = after
                        .find(close)
                        .map(|e| i + open.len() + e + close.len())
                        .unwrap_or(code.len());
                    push(i, end, TokenKind::Comment, &mut out);
                    i = end;
                    continue;
                }
            }
            let c = rest.chars().next().unwrap();
            if c == '"' {
                let triple = rest.starts_with("\"\"\"");
                let end = if triple {
                    rest[3..]
                        .find("\"\"\"")
                        .map(|e| i + 3 + e + 3)
                        .unwrap_or(code.len())
                } else {
                    let mut j = i + 1;
                    while j < bytes.len() && bytes[j] != b'"' && bytes[j] != b'\n' {
                        if bytes[j] == b'\\' {
                            j += 1;
                        }
                        j += 1;
                    }
                    (j + 1).min(code.len())
                };
                push(i, end, TokenKind::String, &mut out);
                i = end;
                continue;
            }
            if c.is_ascii_digit() {
                let mut j = i;
                while j < bytes.len()
                    && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'.' || bytes[j] == b'_')
                {
                    j += 1;
                }
                push(i, j, TokenKind::Number, &mut out);
                i = j;
                continue;
            }
            if c.is_alphabetic() || c == '_' {
                let mut j = i;
                for ch in rest.chars() {
                    if ch.is_alphanumeric() || ch == '_' {
                        j += ch.len_utf8();
                    } else {
                        break;
                    }
                }
                let word = &code[i..j];
                let next = code[j..].trim_start_matches(' ').chars().next();
                let before = code[..i].trim_end();
                let prev_word = before
                    .rsplit(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .next()
                    .unwrap_or("");
                let after_definer = table.definers.contains(&prev_word);
                let kind = if table.constants.contains(&word) {
                    TokenKind::Keyword
                } else if table.keywords.contains(&word) {
                    if super::PLAIN_WORDS.contains(&word) {
                        TokenKind::Plain
                    } else {
                        TokenKind::Keyword
                    }
                } else if after_definer
                    || next == Some('(')
                    || word.chars().next().is_some_and(char::is_uppercase)
                {
                    TokenKind::Function
                } else {
                    TokenKind::Plain
                };
                push(i, j, kind, &mut out);
                i = j;
                continue;
            }
            i += c.len_utf8();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lang: &str, code: &str) -> Vec<(String, TokenKind)> {
        highlight(lang, code)
            .into_iter()
            .map(|t| (code[t.start..t.end].to_string(), t.kind))
            .collect()
    }

    fn kind_of(lang: &str, code: &str, needle: &str) -> TokenKind {
        kind_at(lang, code, needle, 0)
    }

    /// The kind of the `nth` occurrence of `needle` (one token must cover it whole).
    fn kind_at(lang: &str, code: &str, needle: &str, nth: usize) -> TokenKind {
        let start = code
            .match_indices(needle)
            .nth(nth)
            .map(|(i, _)| i)
            .unwrap_or_else(|| panic!("{needle:?} #{nth} in {code:?}"));
        let end = start + needle.len();
        let tokens = highlight(lang, code);
        let t = tokens
            .iter()
            .find(|t| t.start <= start && end <= t.end)
            .unwrap_or_else(|| panic!("no token covers {needle:?}: {tokens:?}"));
        t.kind
    }

    const BUBBLE_SORT: &str = "def bubble_sort(arr):\n    n = len(arr)\n    for i in range(n):\n        swapped = False\n        for j in range(0, n - i - 1):\n            if arr[j] > arr[j + 1]:\n                arr[j], arr[j + 1] = arr[j + 1], arr[j]\n                swapped = True\n        # If no two elements swapped, list is already sorted\n        if not swapped:\n            break\n    return arr\n\n# Example usage:\nitems = [64, 34, 25, 12, 22, 11, 90]\nprint(bubble_sort(items))  # Output: [11, 12, 22, 25, 34, 64, 90]";

    /// Jacob's bubble-sort card, token by token as Grok paints it.
    #[test]
    fn python_bubble_sort_matches_grok() {
        use TokenKind::*;
        let k = |n: &str| kind_of("python", BUBBLE_SORT, n);
        assert_eq!(k("def"), Keyword);
        assert_eq!(k("bubble_sort"), Function, "definition name");
        assert_eq!(kind_of("python", BUBBLE_SORT, "arr):"), Plain);
        assert_eq!(k("len"), Function, "builtin call");
        assert_eq!(k("range"), Function);
        assert_eq!(k("for"), Keyword);
        assert_eq!(k(" in "), Plain, "Grok paints `in` plain");
        assert_eq!(k("False"), Keyword);
        assert_eq!(k("True"), Keyword);
        assert_eq!(k("not"), Keyword);
        assert_eq!(k("break"), Keyword);
        assert_eq!(k("return"), Keyword);
        assert_eq!(k("64"), Number);
        assert_eq!(k("# If no two"), Comment);
        assert_eq!(k("# Output: [11"), Comment);
        assert_eq!(k("print"), Function);
        assert_eq!(
            kind_at("python", BUBBLE_SORT, "bubble_sort", 1),
            Function,
            "user function call"
        );
        assert_eq!(k("swapped = "), Plain);
        // Contiguous and covering.
        let tokens = highlight("python", BUBBLE_SORT);
        assert_eq!(tokens[0].start, 0);
        assert_eq!(tokens.last().unwrap().end, BUBBLE_SORT.len());
        for w in tokens.windows(2) {
            assert_eq!(w[0].end, w[1].start);
            assert!(w[0].kind != w[1].kind, "merged: {:?}", w);
        }
    }

    #[test]
    fn every_language_classifies_its_landmarks() {
        use TokenKind::*;
        type Case = (
            &'static str,
            &'static str,
            &'static [(&'static str, TokenKind)],
        );
        let cases: &[Case] = &[
            (
                "js",
                "const add = (a, b) => a + b;\nfunction greet(name) { return `hi ${name}`; }\nconsole.log(greet(\"x\"), true, null, 42); // done",
                &[("const", Keyword), ("add", Function), ("function", Keyword), ("greet", Function), ("return", Keyword), ("`hi ", String), ("log", Function), ("\"x\"", String), ("true", Keyword), ("null", Keyword), ("42", Number), ("// done", Comment), ("a + b", Plain)],
            ),
            (
                "ts",
                "interface P { x: number }\nconst f = (p: P): string => `v${p.x}`;",
                &[("const", Keyword), ("`v", String)],
            ),
            (
                "swift",
                "import Foundation\n\nstruct Greeter {\n    let name: String\n    func greet(times: Int) -> String {\n        // say hi\n        guard times > 0 else { return \"\" }\n        return \"Hi \\(name) \\(times)\"\n    }\n}\nprint(Greeter(name: \"Jacob\").greet(times: 2.5))",
                &[("import", Keyword), ("struct", Keyword), ("Greeter", Function), ("let", Keyword), ("String", Function), ("func", Keyword), ("greet", Function), ("// say hi", Comment), ("guard", Keyword), ("\"Hi \\(name) \\(times)\"", String), ("print", Function), ("2.5", Number), ("times > ", Plain)],
            ),
            (
                "rust",
                "fn main() {\n    let mut v: Vec<u32> = vec![1, 2];\n    println!(\"hi {}\", v.len());\n    if v.is_empty() { return; } // end\n}",
                &[("fn", Keyword), ("main", Function), ("let", Keyword), ("mut", Keyword), ("u32", Keyword), ("vec!", Function), ("1", Number), ("println!", Function), ("\"hi {}\"", String), ("len", Function), ("if", Keyword), ("is_empty", Function), ("return", Keyword), ("// end", Comment)],
            ),
            (
                "go",
                "package main\n\nfunc main() {\n    x := len(os.Args)\n    fmt.Println(\"hi\", x, true) // out\n}",
                &[("package", Keyword), ("func", Keyword), ("len", Function), ("Println", Function), ("\"hi\"", String), ("true", Keyword), ("// out", Comment), ("x :=", Plain)],
            ),
            (
                "bash",
                "#!/bin/bash\n# count\nfor f in *.txt; do\n  echo \"file $f\" | grep -c foo\ndone",
                &[("#!/bin/bash", Comment), ("for", Keyword), (" in ", Plain), ("do", Keyword), ("echo", Function), ("\"file ", String), ("grep", Function), ("# count", Comment), ("done", Keyword)],
            ),
            (
                "json",
                "{\n  \"name\": \"arbos\",\n  \"count\": 3,\n  \"ok\": true,\n  \"none\": null\n}",
                &[("\"name\"", String), ("\"arbos\"", String), ("3", Number), ("true", Keyword), ("null", Keyword), ("{", Plain)],
            ),
            (
                "sql",
                "SELECT name, COUNT(*) FROM users WHERE age > 21 GROUP BY name; -- top",
                &[("SELECT", Keyword), ("COUNT", Function), ("FROM", Keyword), ("WHERE", Keyword), ("21", Number), ("GROUP BY", Keyword), ("-- top", Comment), ("users", Plain)],
            ),
            (
                "c",
                "#include <stdio.h>\nint main(void) {\n    printf(\"%d\\n\", 42); // hi\n    return 0;\n}",
                &[("#include", Keyword), ("<stdio.h>", String), ("int", Keyword), ("main", Function), ("void", Keyword), ("printf", Function), ("\"%d\\n\"", String), ("42", Number), ("// hi", Comment), ("return", Keyword)],
            ),
            (
                "cpp",
                "#include <iostream>\nint main() {\n    std::vector<int> v{1};\n    std::cout << \"hi\" << std::endl; /* c */\n    return 0;\n}",
                &[("#include", Keyword), ("int", Keyword), ("main", Function), ("\"hi\"", String), ("/* c */", Comment), ("return", Keyword), ("1", Number)],
            ),
            (
                "yaml",
                "name: arbos\nversion: 1.2\nflags:\n  - fast\n  - debug: true  # yes",
                &[("name", Function), ("arbos", String), ("1.2", Number), ("flags", Function), ("fast", String), ("true", Keyword), ("# yes", Comment)],
            ),
            (
                "html",
                "<div class=\"a\">Hi <b>there</b></div>\n<!-- note -->\n<style>.a { color: red; }</style>",
                &[("div", Keyword), ("class", Function), ("\"a\"", String), ("Hi ", Plain), ("<!-- note -->", Comment), ("color", Function), ("style", Keyword)],
            ),
            (
                "css",
                ".card { color: #fff; margin: 4px 0; } /* box */\n@media (max-width: 600px) { .card { display: none; } }",
                &[("card", Function), ("color", Function), ("4", Number), ("/* box */", Comment), ("margin", Function), ("display", Function)],
            ),
            (
                "markdown",
                "# Title\nSome **bold** and `code` and [link](http://x).\n- item",
                &[("Title", Keyword), ("bold", Keyword), ("code", String), ("http://x", Function), ("Some ", Plain), ("item", Plain)],
            ),
        ];
        for (lang, code, expect) in cases {
            for (needle, kind) in expect.iter() {
                assert_eq!(
                    kind_of(lang, code, needle),
                    *kind,
                    "{lang}: {needle:?}\n{:#?}",
                    kinds(lang, code)
                );
            }
        }
    }

    #[test]
    fn tags_aliases_and_fallbacks() {
        assert_eq!(language_for_tag("py"), Some(Language::Python));
        assert_eq!(language_for_tag("Python3"), Some(Language::Python));
        assert_eq!(language_for_tag("ts"), Some(Language::TypeScript));
        assert_eq!(language_for_tag("c++"), Some(Language::Cpp));
        assert_eq!(language_for_tag("yml"), Some(Language::Yaml));
        assert_eq!(language_for_tag("sh"), Some(Language::Shell));
        assert_eq!(language_for_tag("shell"), Some(Language::Shell));
        assert_eq!(language_for_tag("json {title}"), Some(Language::Json));
        assert_eq!(language_for_tag("brainfuck"), None);
        assert_eq!(language_for_tag(""), None);

        // The tag wins over the content.
        assert_eq!(detect("rust", "def f():\n  pass"), Some(Language::Rust));
        // First line.
        assert_eq!(
            detect("", "#!/usr/bin/env python3\nprint(1)"),
            Some(Language::Python)
        );
        assert_eq!(detect("", "#!/bin/sh\nls"), Some(Language::Shell));
        assert_eq!(
            detect("", "#!/usr/bin/env node\nx"),
            Some(Language::JavaScript)
        );
        assert_eq!(
            detect("", "<!DOCTYPE html>\n<p>x</p>"),
            Some(Language::Html)
        );
        // Keyword score.
        assert_eq!(detect("", BUBBLE_SORT), Some(Language::Python));
        assert_eq!(
            detect("", "const x = 1;\nconsole.log(x === 1);"),
            Some(Language::JavaScript)
        );
        assert_eq!(
            detect(
                "",
                "fn main() {\n    let mut x = 1;\n    println!(\"{}\", x);\n}"
            ),
            Some(Language::Rust)
        );
        assert_eq!(
            detect(
                "",
                "package main\nfunc main() {\n  x := 1\n  fmt.Println(x)\n}"
            ),
            Some(Language::Go)
        );
        assert_eq!(detect("", "func greet(name: String) -> String {\n    guard !name.isEmpty else { return \"\" }\n    return \"Hi \\(name)\"\n}"), Some(Language::Swift));
        assert_eq!(
            detect("", "SELECT id FROM users WHERE active = 1"),
            Some(Language::Sql)
        );
        assert_eq!(
            detect("", "{\"a\": [1, 2], \"b\": null}"),
            Some(Language::Json)
        );
        assert_eq!(
            detect("", "name: arbos\nversion: 1\nflags:\n  - fast"),
            Some(Language::Yaml)
        );
        assert_eq!(
            detect("", "<div class=\"a\">\n  <p>Hi</p>\n</div>"),
            Some(Language::Html)
        );
        assert_eq!(
            detect("", ".a {\n  color: red;\n  margin: 0 4px;\n}"),
            Some(Language::Css)
        );
        assert_eq!(
            detect(
                "",
                "#include <stdio.h>\nint main(void) {\n  printf(\"hi\");\n  return 0;\n}"
            ),
            Some(Language::C)
        );
        assert_eq!(
            detect(
                "",
                "#include <iostream>\nint main() {\n  std::cout << \"hi\";\n  return 0;\n}"
            ),
            Some(Language::Cpp)
        );
        assert_eq!(
            detect("", "echo hello\nls -la | grep foo && cd /tmp"),
            Some(Language::Shell)
        );
        assert_eq!(
            detect("", "# Title\n\n- one\n- two\n\nSome **bold** text."),
            Some(Language::Markdown)
        );
        // Plain prose stays plain: no colours.
        assert_eq!(detect("", "Just a sentence with no code in it."), None);
        assert!(highlight("", "Just a sentence with no code in it.").is_empty());
        assert!(highlight("python", "").is_empty());
        // Too long: plain.
        let huge = "x = 1\n".repeat(MAX_HIGHLIGHT_BYTES / 4);
        assert!(highlight("python", &huge).is_empty());
    }

    #[test]
    fn the_cache_returns_the_same_tokens() {
        let a = highlight("python", BUBBLE_SORT);
        let b = highlight("python", BUBBLE_SORT);
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }
}
