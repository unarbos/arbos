//! What a board surface looks like in the detail pane.
//!
//! The kernel `show`s a file, a canvas, or a browser. This draws that
//! surface — an image, a document, a listing — not a path and a text dump.
//! One surface at a time. The title bar names it; the body is the thing.

use crate::{
    kernel,
    model::{
        panel::OpenedBy,
        place::Place,
        surface::{Bind, Surface},
    },
};
use bezel::{
    gpui::{
        AnyElement, App, Image, ObjectFit, SharedString, StyledImage as _, Window, div, img,
        prelude::*, px,
    },
    theme::{TextStyle, Theme, Typeset},
    ui::{
        icons,
        widgets::{ButtonStyle, Buttons},
    },
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
};

/// How wide a document sits. Same column the transcript uses, so a shown
/// markdown file and a chat reply share one measure.
const DOCUMENT_MAX: f32 = 720.;

/// How much of a text file we paint. Enough for a real document; past this
/// the rest is a quiet ellipsis, not a second page of dump.
const TEXT_LIMIT: usize = 128 * 1024;

/// How many directory rows before we stop. A listing is a glance, not `ls -la`.
const DIR_LIMIT: usize = 80;

/// How many sheet rows become a table. The rest is implied.
const SHEET_ROWS: usize = 48;

/// How many lines of a process journal we show. The tail is the news.
const TAIL_LINES: usize = 200;

/// How much of the journal's end we read to find those lines.
const TAIL_BYTES: u64 = 256 * 1024;

/// How often a shown process journal is re-read.
pub const TAIL_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

/// What the title bar and the sidebar draw for this kind of card.
pub fn glyph(board_kind: &str) -> &'static str {
    match board_kind {
        "terminal" => icons::devices::TERMINAL,
        "browser" => icons::devices::GLOBAL,
        "process" => icons::devices::CPU,
        "files" | "dir" => icons::files::FOLDER,
        _ => icons::files::DOCUMENT,
    }
}

/// Whether the body changes on its own while it is shown, so the pane has
/// to come back and look again.
pub fn live(surface: &Surface) -> bool {
    matches!(surface.bind, Bind::Process { .. })
}

/// Whether the kernel of this row's place is answering. A row's state is not a
/// property of the row alone: with the link gone, nothing on screen is news,
/// and the honest word for a job we can no longer hear about is not `running`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Live,
    Lost,
}

/// What state a row is in, in one word, or none where the surface has no
/// state to be in. A tab wears this on its face, so a finished job and a
/// running one are told apart by a word rather than by a colour — and a
/// journal that has gone while its process may still be writing says so
/// (the 164 GB job, #377).
///
/// With the link lost, whatever is on disk stays on screen but nothing claims
/// to be live: a job reads `link lost`, not `running`. What ended is still
/// reported, because an exit code on disk is a fact that outlives the socket.
pub fn state_word(surface: &Surface, link: Link) -> Option<&'static str> {
    // The kernel has been asked and does not hold it. That answer outranks
    // everything below, and outlasts the link going down and coming back:
    // a row nothing is behind is not a row that might be running.
    if surface.gone {
        return Some("gone");
    }
    match &surface.bind {
        Bind::Process { done, log, .. } => match (done, link) {
            (None, Link::Lost) => Some("link lost"),
            (None, Link::Live) if log.is_file() => Some("running"),
            (None, Link::Live) => Some("no journal"),
            (Some(Some(0)), _) => Some("done"),
            (Some(Some(_)), _) => Some("failed"),
            (Some(None), _) => Some("stopped"),
        },
        // Whose shell this is, which is not the same question as who may type
        // in it. The kernel's `terminal` tool opens one page and both sides
        // can write to it, so a page the agent opened says so rather than
        // claiming to be the person's: a label that is wrong from birth is
        // worse than no label. A page of the person's own needs a frame the
        // kernel does not have yet (`docs/side-panels-design.md`, handover 2).
        // Whose shell: read from who asked, which the kernel's board frame
        // says. `owner` cannot answer it — a shell the person asked for still
        // docks under an agent, so keying on that labelled his own terminal
        // "agent's" the moment the tile could open one.
        Bind::Terminal { .. } => match (surface.by, link) {
            (_, Link::Lost) => Some("link lost"),
            (OpenedBy::User, Link::Live) => Some("yours"),
            (OpenedBy::Agent, Link::Live) => Some("agent's"),
        },
        Bind::Browser { .. } | Bind::Url(_) | Bind::Path(_) | Bind::Empty => None,
    }
}

/// Filename, page title, or host — never a full path.
pub fn title(surface: &Surface) -> String {
    if !surface.title.is_empty() && !generic_title(&surface.title, &surface.board_kind) {
        return surface.title.clone();
    }
    if let Some(name) = surface
        .path()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
    {
        return name.to_owned();
    }
    if let Bind::Url(url) | Bind::Browser { url, .. } = &surface.bind
        && let Some(host) = host_of(url)
    {
        return host.to_owned();
    }
    if surface.title.is_empty() {
        surface.board_kind.clone()
    } else {
        surface.title.clone()
    }
}

fn generic_title(title: &str, board_kind: &str) -> bool {
    title.eq_ignore_ascii_case("panel")
        || title.eq_ignore_ascii_case(board_kind)
        || matches!(
            title,
            "Terminal" | "Browser" | "Panel" | "Files" | "Canvas" | "Image"
        )
}

/// The surface's body. The caller hangs the header and, for a live terminal,
/// the existing terminal view.
pub fn render(
    surface: &Surface,
    place: Option<&Place>,
    link: Link,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = Theme::of(cx).clone();
    match &surface.bind {
        Bind::Terminal { .. } => quiet(&theme, "This terminal has no session."),
        Bind::Browser { url, shot, .. } => browser_body(&theme, url, shot.clone()),
        Bind::Process {
            log, live, done, ..
        } => process_body(
            &theme,
            log,
            live,
            *done,
            link,
            surface.status.as_deref(),
            surface.gone,
        ),
        Bind::Url(url) => open_card(&theme, Some(url), url),
        Bind::Empty => quiet(&theme, "Nothing here."),
        Bind::Path(path) => path_body(surface, place, path, window, cx),
    }
}

/// The kernel's page: where it is, the last picture of it, and a way out
/// to a real browser. The picture scales to the width and keeps its shape.
fn browser_body(theme: &Theme, url: &str, shot: Option<Arc<Image>>) -> AnyElement {
    let href = SharedString::from(url.to_owned());
    let open = href.clone();
    let bar = div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(16.))
        .py(px(8.))
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .px(px(10.))
                .py(px(4.))
                .rounded(px(6.))
                .bg(theme.surface_raised)
                .font_family(theme.font_mono.clone())
                .text_style(TextStyle::Callout)
                .text_color(theme.text_muted)
                .child(if url.is_empty() {
                    SharedString::from("about:blank")
                } else {
                    href
                }),
        )
        .when(!url.is_empty(), |bar| {
            bar.child(
                theme
                    .button("Open in browser", ButtonStyle::Ghost, None)
                    .id("surface-browser-open")
                    .on_click(move |_, _, cx| cx.open_url(&open)),
            )
        });
    let body = match shot {
        Some(shot) => div()
            .id("surface-browser-shot")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(16.))
            .py(px(16.))
            .child(img(shot).w_full().object_fit(ObjectFit::ScaleDown))
            .into_any_element(),
        None => quiet(theme, "No picture yet."),
    };
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(bar)
        .child(body)
        .into_any_element()
}

/// The tail of a job's journal, and how it ended if it has. The kernel's
/// streamed output wins when any has arrived (it is the only source on a
/// remote place); the file is read for kernels that do not stream.
fn process_body(
    theme: &Theme,
    log: &Path,
    live: &str,
    done: Option<Option<i32>>,
    link: Link,
    kernel_words: Option<&str>,
    gone: bool,
) -> AnyElement {
    let streamed = !live.is_empty() || done.is_some();
    let tail = if streamed {
        let lines: Vec<&str> = live.lines().collect();
        let skip = lines.len().saturating_sub(TAIL_LINES);
        lines[skip..].join("\n")
    } else {
        tail_lines(log)
    };
    let exit = if streamed {
        done.map(|code| match code {
            Some(code) => code.to_string(),
            None => "killed".to_owned(),
        })
    } else {
        log.parent()
            .and_then(|dir| std::fs::read_to_string(dir.join("exit")).ok())
            .map(|code| code.trim().to_owned())
            .filter(|code| !code.is_empty())
    };
    // The kernel's own words, when it has told us how this ended — it is the
    // side that knows, and after a kernel's death its replacement is the only
    // thing that can say what became of the job.
    //
    // Only for an end, and this restraint is load-bearing: `status` is a
    // snapshot from the last time we asked, and we ask on attach, not on a
    // timer. A live job's "running for 41s (pid 4812)" would therefore sit
    // there reading 41s an hour later — a clock that has stopped while
    // claiming to run, which is the family of lie this whole panel exists to
    // end. Widening this to live rows means asking on a timer first; without
    // that, showing them is worse than showing nothing.
    let ended = gone || exit.is_some() || done.is_some_and(|code| code.is_some());
    let status = match (kernel_words, ended) {
        (Some(words), true) if !words.is_empty() => words.to_owned(),
        _ => match (exit, link) {
            (Some(code), _) if code == "0" => "exited 0".to_owned(),
            (Some(code), _) if code == "killed" => "killed".to_owned(),
            (Some(code), _) => format!("exited {code}"),
            (None, _) if gone => "gone".to_owned(),
            (None, Link::Live) => "running".to_owned(),
            (None, Link::Lost) => "link lost".to_owned(),
        },
    };
    // "No output yet" is only true while there is somewhere for output to
    // appear. A job whose journal is not on disk has nowhere, and saying
    // nothing yet about a row that can never say anything is how an empty
    // screen passes for a working one (the 164 GB job's folder was gone while
    // it wrote). Checked here rather than remembered: the file either opens or
    // it does not.
    let readable = streamed || log.is_file();
    let text = match (tail.is_empty(), readable) {
        (false, _) => SharedString::from(tail),
        (true, true) => SharedString::from("(no output yet)"),
        (true, false) => SharedString::from(
            "The journal for this job is not on disk, so its output cannot be read here.",
        ),
    };
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            div()
                .id("surface-process-log")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(16.))
                .py(px(12.))
                .font_family(theme.font_mono.clone())
                .text_style(TextStyle::Callout)
                .text_color(theme.text)
                .child(text),
        )
        .child(
            div()
                .flex_none()
                .px(px(16.))
                .py(px(6.))
                .border_t_1()
                .border_color(theme.border)
                .text_style(TextStyle::Callout)
                .text_color(theme.text_faint)
                .child(SharedString::from(status)),
        )
        .into_any_element()
}

/// The last `TAIL_LINES` of a file, read from its end only — the end of a
/// journal, as the panel draws it.
fn tail_lines(path: &Path) -> String {
    let Ok(mut file) = File::open(path) else {
        return String::new();
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = size.saturating_sub(TAIL_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::with_capacity((size - start) as usize);
    if file.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(TAIL_LINES);
    lines[skip..].join("\n")
}

/// What a row seeded from the kernel's list starts with: everything the job
/// has already written, ready for the kernel's `job` frames to be appended to
/// it.
///
/// The trailing newline is the whole point of this existing beside
/// [`tail_lines`], which drops it. Without it the first delta lands on the end
/// of the last line the job wrote and two lines read as one — seen on the first
/// still of this working, as `tick 24tick 25`. A file that ends mid-line has no
/// newline to keep, and there the delta *is* the rest of that line, so the
/// question is asked of the file rather than assumed either way.
pub fn journal_prime(path: &Path) -> String {
    let text = tail_lines(path);
    if text.is_empty() || !ends_with_newline(path) {
        return text;
    }
    text + "\n"
}

/// Whether a file's last byte is a newline. Read from the end, so the size of
/// the journal does not matter.
fn ends_with_newline(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    if file.seek(SeekFrom::End(-1)).is_err() {
        return false;
    }
    let mut last = [0u8; 1];
    file.read_exact(&mut last).is_ok() && last[0] == b'\n'
}

fn path_body(
    surface: &Surface,
    place: Option<&Place>,
    path: &Path,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = Theme::of(cx).clone();
    let resolved = resolve(place, path);
    let kind = present(&surface.board_kind, Some(&resolved));
    let href = open_href(place, path, &resolved);

    if matches!(kind, Present::Files) || resolved.is_dir() {
        return files(&theme, &resolved);
    }

    if matches!(kind, Present::Image) {
        if resolved.is_file() {
            return picture(&resolved);
        }
        if let Some(url) = href.as_deref().filter(|url| url.contains("://")) {
            return picture_url(url);
        }
        return quiet(&theme, "This file is gone.");
    }

    if matches!(kind, Present::Canvas | Present::External) {
        return match href {
            Some(url) => open_card(&theme, None, &url),
            None => quiet(&theme, "This file is gone."),
        };
    }

    if !resolved.is_file() {
        return quiet(&theme, "This file is gone.");
    }

    match read_text(&resolved) {
        TextFile::Missing => quiet(&theme, "This file is gone."),
        TextFile::Binary => quiet(&theme, "Can't preview this file."),
        TextFile::Text { text, .. } if matches!(kind, Present::Diff) => {
            review(&theme, &text)
        }
        TextFile::Text { text, truncated } => {
            let source = match kind {
                // The kernel's documents open on TOML or YAML front matter
                // (`+++ owner = "root" +++`); a page reader shows the page,
                // not its header (Jacob, report 2026-09-17-14, F-144).
                Present::Markdown => fence_if_needed(None, without_front_matter(&text), truncated),
                Present::Sheet => sheet_markdown(&text, truncated),
                _ => fence_if_needed(Some(language_for(&resolved)), &text, truncated),
            };
            document(&source, window, cx)
        }
    }
}

/// The body after a `+++`/`---` front-matter block, if the text opens on one.
fn without_front_matter(text: &str) -> &str {
    for fence in ["+++", "---"] {
        if let Some(rest) = text.strip_prefix(fence)
            && let Some(end) = rest.find(&format!("\n{fence}"))
        {
            return rest[end + 1 + fence.len()..].trim_start_matches('\n');
        }
    }
    text
}

enum Present {
    Image,
    Markdown,
    Code,
    /// A unified diff, drawn as one: the tree's Review, or a patch file.
    Diff,
    Sheet,
    Files,
    Canvas,
    External,
}

fn present(board_kind: &str, path: Option<&Path>) -> Present {
    match board_kind {
        "image" => Present::Image,
        "doc" | "prompt" => Present::Markdown,
        "code" => Present::Code,
        "diff" => Present::Diff,
        "sheet" => Present::Sheet,
        "files" | "dir" => Present::Files,
        "canvas" => Present::Canvas,
        "pdf" | "browser" => Present::External,
        other => infer(other, path),
    }
}

fn infer(_kind: &str, path: Option<&Path>) -> Present {
    let Some(path) = path else {
        return Present::External;
    };
    if path.is_dir() {
        return Present::Files;
    }
    match extension(path).as_str() {
        "html" | "htm" => Present::Canvas,
        "md" | "markdown" => Present::Markdown,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp" | "avif" => Present::Image,
        "pdf" => Present::External,
        "csv" | "tsv" | "tab" => Present::Sheet,
        "diff" | "patch" => Present::Diff,
        _ => Present::Code,
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn resolve(place: Option<&Place>, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_owned();
    }
    match place {
        Some(place) if !place.is_remote() => place.path.join(path),
        _ => path.to_owned(),
    }
}

fn open_href(place: Option<&Place>, given: &Path, resolved: &Path) -> Option<String> {
    if let Some(place) = place.filter(|place| place.is_remote())
        && let Some(base) = kernel::http_base_place(place)
    {
        return Some(raw_url(&base, given));
    }
    if resolved.is_file() {
        return Some(file_url(resolved));
    }
    if given.is_absolute() && given.is_file() {
        return Some(file_url(given));
    }
    None
}

fn raw_url(base: &str, path: &Path) -> String {
    let rel = path.to_string_lossy().replace('\\', "/");
    let encoded = rel.split('/').map(encode_seg).collect::<Vec<_>>().join("/");
    format!(
        "{}/raw/{}",
        base.trim_end_matches('/'),
        encoded.trim_start_matches('/')
    )
}

fn encode_seg(seg: &str) -> String {
    let mut out = String::new();
    for byte in seg.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn file_url(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut url = String::from("file://");
    for ch in raw.chars() {
        match ch {
            ' ' => url.push_str("%20"),
            '#' => url.push_str("%23"),
            _ => url.push(ch),
        }
    }
    url
}

fn host_of(url: &str) -> Option<&str> {
    let rest = url.split("://").nth(1)?;
    rest.split('/').next().filter(|host| !host.is_empty())
}

fn quiet(theme: &Theme, line: &str) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .text_style(TextStyle::Callout)
        .text_color(theme.text_faint)
        .child(SharedString::from(line.to_owned()))
        .into_any_element()
}

/// Title is already in the header. The body is the URL, if it helps, and Open.
fn open_card(theme: &Theme, url_label: Option<&str>, href: &str) -> AnyElement {
    let href = SharedString::from(href.to_owned());
    let open = href.clone();
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(12.))
        .px(px(32.))
        .children(url_label.map(|label| {
            div()
                .max_w(px(420.))
                .truncate()
                .text_style(TextStyle::Callout)
                .text_color(theme.text_muted)
                .child(SharedString::from(label.to_owned()))
        }))
        .child(
            theme
                .button("Open", ButtonStyle::Ghost, None)
                .id("surface-open")
                .on_click(move |_, _, cx| cx.open_url(&open)),
        )
        .into_any_element()
}

fn picture(path: &Path) -> AnyElement {
    frame_image(
        img(path.to_path_buf())
            .size_full()
            .object_fit(ObjectFit::Contain),
    )
}

fn picture_url(url: &str) -> AnyElement {
    frame_image(
        img(SharedString::from(url.to_owned()))
            .size_full()
            .object_fit(ObjectFit::Contain),
    )
}

fn frame_image(image: impl IntoElement) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .px(px(32.))
        .py(px(28.))
        .child(image)
        .into_any_element()
}

/// Cursor's Review: the diff as a diff, in the document's column.
fn review(theme: &Theme, text: &str) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .justify_center()
        .child(
            div()
                .id("surface-review")
                .h_full()
                .w_full()
                .max_w(px(DOCUMENT_MAX))
                .overflow_y_scroll()
                .px(px(28.))
                .pt(px(24.))
                .pb(px(48.))
                .child(crate::view::component::transcript::review_view(theme, text)),
        )
        .into_any_element()
}

fn document(source: &str, window: &mut Window, cx: &mut App) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .justify_center()
        .child(
            div()
                .id("surface-document")
                .h_full()
                .w_full()
                .max_w(px(DOCUMENT_MAX))
                .overflow_y_scroll()
                .px(px(28.))
                .pt(px(32.))
                .pb(px(48.))
                .child(markdown::markdown(source, window, cx)),
        )
        .into_any_element()
}

fn files(theme: &Theme, path: &Path) -> AnyElement {
    let Ok(entries) = std::fs::read_dir(path) else {
        return quiet(theme, "This file is gone.");
    };
    let mut rows: Vec<(bool, String)> = entries
        .flatten()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            (dir, name)
        })
        .collect();
    rows.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .reverse()
            .then_with(|| a.1.to_ascii_lowercase().cmp(&b.1.to_ascii_lowercase()))
    });
    if rows.is_empty() {
        return quiet(theme, "Empty folder.");
    }
    let more = rows.len() > DIR_LIMIT;
    rows.truncate(DIR_LIMIT);
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .justify_center()
        .child(
            div()
                .id("surface-files")
                .h_full()
                .w_full()
                .max_w(px(DOCUMENT_MAX))
                .overflow_y_scroll()
                .px(px(28.))
                .pt(px(32.))
                .pb(px(48.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .children(rows.into_iter().map(|(dir, name)| {
                    div()
                        .text_style(TextStyle::Body)
                        .text_color(if dir { theme.text_muted } else { theme.text })
                        .child(if dir {
                            SharedString::from(format!("{name}/"))
                        } else {
                            SharedString::from(name)
                        })
                }))
                .when(more, |col| {
                    col.child(
                        div()
                            .mt(px(8.))
                            .text_style(TextStyle::Callout)
                            .text_color(theme.text_faint)
                            .child("…"),
                    )
                }),
        )
        .into_any_element()
}

enum TextFile {
    Missing,
    Binary,
    Text { text: String, truncated: bool },
}

fn read_text(path: &Path) -> TextFile {
    let Ok(file) = File::open(path) else {
        return TextFile::Missing;
    };
    let mut buf = Vec::new();
    let truncated = file
        .take(TEXT_LIMIT as u64 + 1)
        .read_to_end(&mut buf)
        .map(|n| n > TEXT_LIMIT)
        .unwrap_or(false);
    if truncated {
        buf.truncate(TEXT_LIMIT);
    }
    match String::from_utf8(buf) {
        Ok(text) => TextFile::Text { text, truncated },
        Err(_) => TextFile::Binary,
    }
}

fn fence_if_needed(language: Option<&str>, source: &str, truncated: bool) -> String {
    match language {
        None => {
            if truncated {
                format!("{source}\n\n…")
            } else {
                source.to_owned()
            }
        }
        Some(lang) => {
            let mut ticks = 3;
            while source.contains(&"`".repeat(ticks)) {
                ticks += 1;
            }
            let fence = "`".repeat(ticks);
            let mut out = format!("{fence}{lang}\n{source}");
            if !source.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&fence);
            if truncated {
                out.push_str("\n\n…");
            }
            out
        }
    }
}

fn sheet_markdown(source: &str, truncated: bool) -> String {
    let delim = if source.contains('\t') { '\t' } else { ',' };
    let lines: Vec<&str> = source.lines().take(SHEET_ROWS).collect();
    if lines.is_empty() {
        return String::new();
    }
    let cells = |line: &str| -> Vec<String> {
        line.split(delim)
            .map(|cell| cell.trim().replace('|', "\\|"))
            .collect()
    };
    let header = cells(lines[0]);
    if header.is_empty() {
        return fence_if_needed(Some("csv"), source, truncated);
    }
    let mut out = String::new();
    out.push('|');
    out.push(' ');
    out.push_str(&header.join(" | "));
    out.push_str(" |\n| ");
    out.push_str(&vec!["---"; header.len()].join(" | "));
    out.push_str(" |\n");
    for line in lines.iter().skip(1) {
        let row = cells(line);
        if row.iter().all(|cell| cell.is_empty()) {
            continue;
        }
        out.push('|');
        out.push(' ');
        out.push_str(&row.join(" | "));
        out.push_str(" |\n");
    }
    if truncated || source.lines().count() > SHEET_ROWS {
        out.push_str("\n…");
    }
    out
}

fn language_for(path: &Path) -> &'static str {
    let ext = extension(path);
    match ext.as_str() {
        "rs" => "rust",
        "py" => "python",
        "ts" => "typescript",
        "tsx" | "jsx" | "js" | "mjs" | "cjs" => "tsx",
        "json" | "jsonc" => "json",
        "go" => "go",
        "sh" | "bash" | "zsh" => "bash",
        "toml" => "toml",
        other => syntax::lang::resolve(other)
            .map(|lang| lang.name)
            .unwrap_or(""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::testing::Scratch;

    #[test]
    fn priming_keeps_the_break_the_next_delta_needs() {
        let dir = Scratch::new("journal");
        let log = dir.path().join("out.log");
        std::fs::write(&log, "tick 23\ntick 24\n").unwrap();
        // The kernel's next `job` frame is appended to this, so a lost
        // newline reads as `tick 24tick 25` — which is what the first still of
        // a seeded row showed.
        assert_eq!(journal_prime(&log), "tick 23\ntick 24\n");
    }

    #[test]
    fn a_journal_cut_mid_line_is_left_for_the_delta_to_finish() {
        let dir = Scratch::new("journal-partial");
        let log = dir.path().join("out.log");
        std::fs::write(&log, "tick 23\nhalf a li").unwrap();
        assert_eq!(
            journal_prime(&log),
            "tick 23\nhalf a li",
            "no newline to keep: the rest of that line is what comes next"
        );
    }

    #[test]
    fn nothing_to_prime_from_is_nothing() {
        let dir = Scratch::new("journal-missing");
        assert_eq!(journal_prime(&dir.path().join("gone.log")), "");
    }
}
