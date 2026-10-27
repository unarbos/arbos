//! Where a URL in a bubble may wrap ([feedback link-click-mac-2026-10-27]).
//!
//! GPUI's line wrapper knows words and punctuation, not URLs. Its rule: a
//! word character (letters, digits, `-` `_` `.` `:` `=` `#` `%` …) may start a
//! line only after a space; any other character may start a line when the
//! character before it is also not a word character. In `https://host/a/b`
//! that leaves exactly one break opportunity before the path — the second
//! slash of `://`, because the first slash precedes it — so a URL that does
//! not fit wraps as `https:/` | `/host/a/b` (Jacob's bubble, Mac v0.6.296),
//! and a path slash after a letter is never a break at all, so a long path
//! is cut mid-word wherever the width runs out.
//!
//! The wrapper is pinned upstream, so the text tells it where to break with
//! two invisible, zero-width characters:
//!
//! - [`HOLD`] (U+034F COMBINING GRAPHEME JOINER) between the two slashes of
//!   `://`: it is in a block the wrapper counts as word characters, so the
//!   second slash no longer follows a non-word character and `https://host`
//!   stays whole.
//! - [`BREAK`] (U+200B ZERO WIDTH SPACE) before a path `/`, a `?` or an `&`
//!   that follows a word character: the slash now follows a non-word
//!   character and may start the next line, so a URL breaks *before* its
//!   separators — the Chicago rule — and the line that continues it begins
//!   with `/`, `?` or `&`, which says "this is still the address".
//!
//! Both are default-ignorable code points: Core Text and the Linux shaper
//! draw nothing for them and give them no advance. They live in the rendered
//! text and in the bubble's plain text (so selection offsets agree), and
//! [`plain`] strips them from anything that leaves the app (⌘C).

/// Keeps the two characters around it on one line (see the module note).
pub const HOLD: char = '\u{034F}';
/// Lets the character after it start a line (see the module note).
pub const BREAK: char = '\u{200B}';

/// `url` with its wrap hints: `https://host` whole, a break allowed before
/// every path `/`, `?` and `&` that follows a word character (never before a
/// trailing one). A URL that is not `scheme://…` is returned as it is.
pub fn breakable(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let mut out = String::with_capacity(url.len() + 8);
    out.push_str(&url[..scheme_end]);
    out.push_str(":/");
    out.push(HOLD);
    out.push('/');
    let rest = &url[scheme_end + 3..];
    let mut chars = rest.char_indices().peekable();
    let mut prev = '/';
    while let Some((i, c)) = chars.next() {
        let last = chars.peek().is_none();
        let separator = matches!(c, '/' | '?' | '&');
        // Not inside the host: the first separator after it starts the path.
        if separator && !last && is_word_char(prev) && i > 0 {
            out.push(BREAK);
        }
        out.push(c);
        prev = c;
    }
    out
}

/// Literal text (a typed bubble) with every bare `http(s)://…` URL made
/// [`breakable`]. A URL runs to the next whitespace, `)` or `"`, less any
/// trailing `.` `,` `;` `:` — the same reading the bubbles give it.
pub fn hint_bare_urls(text: &str) -> String {
    if !text.contains("://") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(start) = find_url_start(rest) {
        out.push_str(&rest[..start]);
        let end = bare_url_end(rest, start);
        out.push_str(&breakable(&rest[start..end]));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Where the next `http://` or `https://` begins in `text`.
fn find_url_start(text: &str) -> Option<usize> {
    let https = text.find("https://");
    let http = text.find("http://");
    match (https, http) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The byte after the bare URL that starts at `start`.
pub fn bare_url_end(text: &str, start: usize) -> usize {
    let tail = &text[start..];
    let stop = tail
        .char_indices()
        .find(|(_, c)| c.is_whitespace() || *c == ')' || *c == '"')
        .map(|(i, _)| i)
        .unwrap_or(tail.len());
    let trimmed = tail[..stop].trim_end_matches(['.', ',', ';', ':']);
    start + trimmed.len()
}

/// `text` without the wrap hints — what copy and paste should carry.
pub fn plain(text: &str) -> String {
    if !text.contains([HOLD, BREAK]) {
        return text.to_string();
    }
    text.chars().filter(|c| *c != HOLD && *c != BREAK).collect()
}

/// GPUI's `LineWrapper::is_word_char` for the characters a URL can hold:
/// ASCII letters and digits plus the punctuation it keeps attached to a word.
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '-' | '_' | '.' | '\'' | '$' | '%' | '@' | '#' | '^' | '~' | ',' | '=' | ':' | ';'
        )
        || c == HOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    const JACOB: &str =
        "https://treatment-perfume-marble-tournaments.trycloudflare.com/jacob/competitors.html";

    #[test]
    fn the_scheme_and_host_hold_and_the_path_may_break_before_its_slashes() {
        let b = breakable(JACOB);
        assert_eq!(plain(&b), JACOB, "nothing visible changes");
        assert_eq!(
            b,
            format!(
                "https:/{HOLD}/treatment-perfume-marble-tournaments.trycloudflare.com\
                 {BREAK}/jacob{BREAK}/competitors.html"
            )
        );
    }

    #[test]
    fn query_separators_may_start_a_line_and_a_trailing_slash_may_not() {
        assert_eq!(
            breakable("https://a.io/p?x=1&y=2"),
            format!("https:/{HOLD}/a.io{BREAK}/p{BREAK}?x=1{BREAK}&y=2")
        );
        assert_eq!(
            breakable("https://a.io/"),
            format!("https:/{HOLD}/a.io/"),
            "a lone slash never starts a line"
        );
        assert_eq!(
            breakable("http://a.io"),
            format!("http:/{HOLD}/a.io"),
            "a bare host only holds"
        );
    }

    #[test]
    fn a_double_slash_in_the_path_breaks_before_its_first_slash() {
        // The first slash gets the hint; the second already follows a
        // non-word character and needs none.
        assert_eq!(
            breakable("https://a.io/x//y"),
            format!("https:/{HOLD}/a.io{BREAK}/x{BREAK}//y")
        );
    }

    #[test]
    fn literal_text_hints_every_bare_url_and_nothing_else() {
        let typed = "see https://a.io/x/y, and (http://b.io/z) now.";
        let hinted = hint_bare_urls(typed);
        assert_eq!(plain(&hinted), typed);
        assert_eq!(
            hinted,
            format!(
                "see https:/{HOLD}/a.io{BREAK}/x{BREAK}/y, and (http:/{HOLD}/b.io{BREAK}/z) now."
            )
        );
        assert_eq!(hint_bare_urls("nothing typed"), "nothing typed");
        assert_eq!(bare_url_end("x https://a.io/p. y", 2), 16);
    }

    /// One run over `len` bytes in the window's font.
    fn run_over(window: &gpui::Window, len: usize) -> gpui::TextRun {
        gpui::TextRun {
            len,
            font: window.text_style().font(),
            color: gpui::Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    /// The lines GPUI's wrapper makes of `text` at `width`, 15 pt.
    fn wrapped_lines(vcx: &mut gpui::VisualTestContext, text: &str, width: f32) -> Vec<String> {
        use gpui::{px, SharedString};
        let text: SharedString = text.to_string().into();
        vcx.update(|window, _| {
            let run = run_over(window, text.len());
            let lines = window
                .text_system()
                .shape_text(text.clone(), px(15.0), &[run], Some(px(width)), None)
                .expect("shaped");
            assert_eq!(lines.len(), 1, "one paragraph");
            let line = &lines[0];
            let mut starts: Vec<usize> = line
                .wrap_boundaries
                .iter()
                .map(|b| line.unwrapped_layout.runs[b.run_ix].glyphs[b.glyph_ix].index)
                .collect();
            starts.insert(0, 0);
            starts.push(text.len());
            starts
                .windows(2)
                .map(|w| text[w[0]..w[1]].to_string())
                .collect()
        })
    }

    /// Through GPUI's own wrapper (pinned upstream; this test is the alarm
    /// if a bump changes its rule): the raw URL breaks as `https:/` |
    /// `/host…` at every bubble width it does not fit — Jacob's shot — while
    /// the hinted URL keeps `https://host` whole and continues only at a `/`.
    #[gpui::test]
    fn gpuis_wrapper_breaks_the_hinted_url_before_its_slashes(cx: &mut gpui::TestAppContext) {
        let vcx = cx.add_empty_window();
        let whole = wrapped_lines(vcx, JACOB, 10_000.0);
        assert_eq!(whole.len(), 1, "fits on one line when wide enough");
        let full_width = vcx.update(|window, _| {
            let run = run_over(window, JACOB.len());
            window
                .text_system()
                .layout_line(JACOB, gpui::px(15.0), &[run], None)
                .width
        });
        let full = f32::from(full_width);
        assert!(full > 300.0, "the URL is wide at 15 pt: {full}");

        // The host fits but the whole URL does not: GPUI's rule puts the
        // break at the only opportunity it sees — the second slash of `://`.
        let narrow = full * 0.8;
        let raw = wrapped_lines(vcx, JACOB, narrow);
        assert!(raw.len() >= 2, "{raw:?}");
        assert_eq!(raw[0], "https:/", "the shipped wrap (v0.6.296)");

        let hinted = wrapped_lines(vcx, &breakable(JACOB), narrow);
        assert!(hinted.len() >= 2, "{hinted:?}");
        assert!(
            plain(&hinted[0])
                .starts_with("https://treatment-perfume-marble-tournaments.trycloudflare.com"),
            "scheme and host stay whole: {hinted:?}"
        );
        for line in &hinted[1..] {
            assert!(
                line.starts_with('/'),
                "a continuation starts at a slash: {hinted:?}"
            );
        }
        assert_eq!(plain(&hinted.concat()), JACOB, "the text is the URL");

        // At every width a bubble can have, no line of the hinted URL ends
        // with `https:/` — the break Jacob saw.
        let mut width = 100.0;
        while width < full {
            for line in wrapped_lines(vcx, &breakable(JACOB), width) {
                assert!(
                    !plain(&line).ends_with("https:/"),
                    "never between `https:/` and `/` (width {width}): {line:?}"
                );
            }
            width += 20.0;
        }
    }

    #[test]
    fn other_text_is_untouched_and_plain_is_the_inverse() {
        for s in ["no url here", "mailto:x@y.z", "", "a/b/c"] {
            assert_eq!(breakable(s), s);
            assert_eq!(plain(s), s);
        }
        assert_eq!(plain(&format!("a{HOLD}b{BREAK}c")), "abc");
    }
}
