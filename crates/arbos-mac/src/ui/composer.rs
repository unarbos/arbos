//! The composer, Grok Bot layout: a "+" circle for attachments, the pill
//! field ("Message <name>") with the hold-to-talk mic (transcribe), and the
//! round white button that sends text, Guides a running turn, stops when the
//! field is empty mid-turn, or starts a call when empty — same as the iPhone
//! (mic + phone; calls start audio-only). Hand-rolled multi-line text input
//! with IME (`EntityInputHandler`, zeron-study §4.6). Enter sends (or Guides
//! mid-task), Shift+Enter inserts a newline. The model and reasoning pickers
//! open from the sidebar row's control-click menu.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    actions, div, fill, img, point, prelude::*, px, relative, size, App, Bounds, ClipboardEntry,
    ClipboardItem, Context, Corners, CursorStyle, DispatchPhase, Div, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    FontWeight, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, PaintQuad, Pixels, Point,
    Render, RenderImage, Role as AriaRole, SharedString, Stateful, Style, TextAlign, TextRun,
    UTF16Selection, UnderlineStyle, Window, WrappedLine,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::app::{decode_thumbnail, AppState, ComposerMode};
use crate::model::{AttachmentKind, CallKind, Message, Part, Role};
use crate::search;
use crate::ui::callview;
use crate::ui::error_note;
use crate::ui::frost::{frosted, PANEL_BLUR};
use crate::ui::glass;
use crate::ui::highlight;
use crate::ui::icons::icon;
use crate::ui::mention::{self, Mention, MentionToken, Mentions};
use crate::ui::select::{self, word_range};
use crate::ui::theme::{
    Theme, COMPOSER_HEIGHT, LINE_HEIGHT, PANEL_RADIUS, PLUS_BUTTON, SEND_BUTTON, TEXT_BODY,
    TEXT_SMALL, TEXT_TINY,
};
use crate::ui::transcript;
use crate::ui::voice::{render_recording_row, Voice, VoiceEvent};

actions!(
    composer,
    [
        Send,
        Newline,
        Backspace,
        Delete,
        Left,
        Right,
        WordLeft,
        WordRight,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectWordLeft,
        SelectWordRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        DocumentStart,
        DocumentEnd,
        SelectDocumentStart,
        SelectDocumentEnd,
        DeleteToLineStart,
        Bold,
        Italic,
        Strike,
        DeleteWordBackward,
        DeleteWordForward,
        Paste,
        Copy,
        Cut,
        Undo,
        Redo,
        Escape,
        AcceptMention,
        PrevMention,
    ]
);

/// One restore point for ⌘Z / ⌘⇧Z in the composer field.
#[derive(Clone, PartialEq, Eq)]
struct EditSnapshot {
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    /// The mention chips (#1596) go with the text they sit in.
    tokens: Vec<MentionToken>,
}

const UNDO_LIMIT: usize = 100;

pub const MAX_LINES: usize = 10;
pub const COMPOSER_RADIUS: f32 = COMPOSER_HEIGHT / 2.0;

pub type SubmitHandler = Rc<dyn Fn(&mut Window, &mut App)>;
/// Returns true when the @-mention menu handled the key (skip caret / send).
pub type MentionKeyHandler = Rc<dyn Fn(&mut App) -> bool>;
/// ↑ (−1) / ↓ (+1) in a single-line picker field: the owner moves its list
/// highlight; the caret stays put.
pub type ArrowHandler = Rc<dyn Fn(i32, &mut App)>;
/// Files and images from a ⌘V, for the owner to stage.
pub type PasteMediaHandler = Rc<dyn Fn(Vec<PathBuf>, Vec<gpui::Image>, &mut App)>;

/// A clipboard sorted by what ⌘V should do with it.
///
/// - A Finder copy carries the paths plus a text twin (the paths joined by
///   newlines); only the paths count, the twin is never typed.
/// - Rich text (HTML, RTF) arrives as its plain-text form — GPUI reads only
///   `public.utf8-plain-text`.
/// - A copied screenshot / image carries no text.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Pasted {
    pub text: Option<String>,
    pub files: Vec<PathBuf>,
    pub images: Vec<gpui::Image>,
}

impl Pasted {
    pub(crate) fn from_clipboard(item: &ClipboardItem) -> Self {
        let mut out = Self::default();
        for entry in item.entries() {
            match entry {
                ClipboardEntry::ExternalPaths(paths) => {
                    out.files.extend(paths.paths().iter().cloned());
                }
                ClipboardEntry::Image(image) => out.images.push(image.clone()),
                ClipboardEntry::String(s) => {
                    out.text.get_or_insert_with(String::new).push_str(s.text());
                }
            }
        }
        if !out.files.is_empty() {
            out.text = None;
        }
        out
    }
}

/// File extension for a pasted image, so the staged tile and the upload see
/// a normal image file.
pub(crate) fn image_extension(format: gpui::ImageFormat) -> &'static str {
    use gpui::ImageFormat::*;
    match format {
        Png => "png",
        Jpeg => "jpg",
        Webp => "webp",
        Gif => "gif",
        Svg => "svg",
        Bmp => "bmp",
        Tiff => "tiff",
        Ico => "ico",
        Pnm => "pnm",
    }
}

/// A clipboard image becomes a real file the composer can stage and the send
/// path can upload: `<tmp>/arbos-pasted/Pasted image <date>.<ext>` (the
/// system clears its temp dir on its own).
pub(crate) fn stash_pasted_image(image: &gpui::Image, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let stamp = chrono::Local::now().format("%Y-%m-%d at %H.%M.%S");
    let ext = image_extension(image.format);
    let mut path = dir.join(format!("Pasted image {stamp}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("Pasted image {stamp} ({n}).{ext}"));
        n += 1;
    }
    std::fs::write(&path, &image.bytes)?;
    Ok(path)
}

pub(crate) fn pasted_images_dir() -> PathBuf {
    std::env::temp_dir().join("arbos-pasted")
}
const INPUT_LINE_HEIGHT: f32 = LINE_HEIGHT;
const INPUT_FONT_SIZE: f32 = TEXT_BODY;

#[derive(Clone, Debug)]
pub enum ComposerEvent {
    Send {
        text: String,
        attachments: Vec<PathBuf>,
        reply_to: Option<String>,
        /// Member ids of the committed mention chips (#1596), in text order.
        mentions: Vec<String>,
    },
    /// Inject `text` into the agent's currently running turn (Guide).
    Guide {
        text: String,
        reply_to: Option<String>,
        mentions: Vec<String>,
    },
    Stop,
}

struct Layout {
    lines: Vec<(usize, WrappedLine)>,
    bounds: Bounds<Pixels>,
    /// Where the clickable chips were painted (tests aim the pointer).
    #[cfg(test)]
    chips: Vec<(Range<usize>, Vec<Bounds<Pixels>>)>,
}

pub struct ComposerInput {
    focus_handle: FocusHandle,
    content: String,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    /// The caret sits on a soft-wrap boundary and belongs to the row *below*
    /// it (⌘← / ⇧⌘← put it there — [feedback composer-caret-shortcuts-2026-10-27]).
    /// Every other move paints a boundary caret at the end of the row above,
    /// as NSTextView's upstream affinity does.
    caret_downstream: bool,
    layout: Option<Layout>,
    is_selecting: bool,
    /// The pointer over the committed chips (hover / pressed), by chip range.
    chips: mention::ChipPointer<Range<usize>>,
    /// The cursor the chip layer last asked for while hovered (tests).
    #[cfg(test)]
    chip_cursor: CursorStyle,
    /// Vertical scroll inside the box once it reaches `MAX_LINES`.
    scroll_y: f32,
    /// Prior text states for ⌘Z (paste, word-delete, typing, cut).
    undo_stack: Vec<EditSnapshot>,
    /// States undone by ⌘Z, waiting for ⌘⇧Z.
    redo_stack: Vec<EditSnapshot>,
    /// Runs deferred, after this entity's update ends, so the owner may read
    /// and update the input freely.
    pub on_submit: Option<SubmitHandler>,
    /// Down while the `@` mention menu is open: move the highlight down.
    pub on_mention_next: Option<MentionKeyHandler>,
    /// Up / Shift+Tab while the `@` mention menu is open: move the highlight up.
    pub on_mention_prev: Option<MentionKeyHandler>,
    /// Tab or Enter while the `@` mention menu is open: accept the highlighted
    /// (or hovered) row.
    pub on_mention_accept: Option<MentionKeyHandler>,
    /// Esc while the `@` mention menu is open: close it for this token.
    pub on_mention_escape: Option<MentionKeyHandler>,
    /// Pickers (the command palette): ↑ / ↓ walk the result list instead of
    /// moving the caret in this one-line field.
    pub on_arrow: Option<ArrowHandler>,
    /// ⌘V with files or an image on the clipboard: the owner stages them.
    pub on_paste_media: Option<PasteMediaHandler>,
    /// Multi-line editors (skill bodies): Enter inserts a newline instead of
    /// submitting.
    pub enter_inserts_newline: bool,
    /// Password fields: paint `*` per byte so caret offsets still line up with `content`.
    masked: bool,
    /// Profile hero rename: paint and hit-test the name centered under the avatar.
    text_center: bool,
    /// Type size; the composer's `TEXT_BODY` unless a smaller field asks otherwise.
    font_size: f32,
    /// Known `@handle`s (people, agents, `@here`, `@agents`): paint them in the
    /// accent as soon as the full token is typed, same as bubble chips resolve.
    mentions: Rc<Mentions>,
    /// Committed mention chips (#1596), by position. Each covers `CELL` +
    /// display name in `content`; the caret never stops inside one, an edit
    /// that touches one removes it whole, and the sent text reads `@handle`.
    tokens: Vec<MentionToken>,
}

/// Separators that complete a typed `@handle` (#1596, behaviour 5): the token
/// becomes a chip when one of these is typed right after it.
fn completes_handle(c: char) -> bool {
    c.is_whitespace() || matches!(c, '.' | ',' | '!' | '?' | ':' | ';')
}

impl ComposerInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: String::new(),
            placeholder: "Do anything…".into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            caret_downstream: false,
            layout: None,
            is_selecting: false,
            chips: mention::ChipPointer::default(),
            #[cfg(test)]
            chip_cursor: CursorStyle::Arrow,
            scroll_y: 0.0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            on_submit: None,
            on_mention_next: None,
            on_mention_prev: None,
            on_mention_accept: None,
            on_mention_escape: None,
            on_arrow: None,
            on_paste_media: None,
            enter_inserts_newline: false,
            masked: false,
            text_center: false,
            font_size: INPUT_FONT_SIZE,
            mentions: Mentions::empty(),
            tokens: Vec::new(),
        }
    }

    /// Who `@handle` resolves to while typing (composer accent highlight).
    pub fn set_mentions(&mut self, mentions: Rc<Mentions>) {
        self.mentions = mentions;
    }

    pub fn set_font_size(&mut self, size: f32) {
        self.font_size = size;
    }

    pub fn set_masked(&mut self, masked: bool) {
        self.masked = masked;
    }

    pub fn set_text_center(&mut self, center: bool) {
        self.text_center = center;
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    /// The committed mention chips, in text order.
    pub fn tokens(&self) -> &[MentionToken] {
        &self.tokens
    }

    /// The text a send carries: every chip reads as `@handle`.
    pub fn sent_text(&self) -> String {
        self.plain_slice(0..self.content.len())
    }

    /// The member ids the committed chips stand for, in text order, deduplicated.
    pub fn token_ids(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in &self.tokens {
            if let Some(id) = &t.id {
                if !out.contains(id) {
                    out.push(id.clone());
                }
            }
        }
        out
    }

    /// `content[range]` with each chip inside read as `@handle` (⌘C, the send).
    fn plain_slice(&self, range: Range<usize>) -> String {
        let mut out = String::with_capacity(range.len());
        let mut pos = range.start;
        for t in &self.tokens {
            if t.range.end <= range.start || t.range.start >= range.end {
                continue;
            }
            let start = t.range.start.max(range.start);
            out.push_str(&self.content[pos..start]);
            out.push_str(&t.plain());
            pos = t.range.end.min(range.end);
        }
        out.push_str(&self.content[pos..range.end]);
        out
    }

    /// Splice `new_text` over `range`, keeping the chips right: a chip the
    /// range touches goes whole (the range grows to cover it), chips after it
    /// shift. Returns the range actually replaced.
    fn splice(&mut self, range: Range<usize>, new_text: &str) -> Range<usize> {
        let mut range = range;
        for t in &self.tokens {
            if t.range.start < range.end && t.range.end > range.start {
                range.start = range.start.min(t.range.start);
                range.end = range.end.max(t.range.end);
            }
        }
        let delta = new_text.len() as isize - range.len() as isize;
        self.tokens
            .retain(|t| t.range.end <= range.start || t.range.start >= range.end);
        for t in &mut self.tokens {
            if t.range.start >= range.end {
                t.range = ((t.range.start as isize + delta) as usize)
                    ..((t.range.end as isize + delta) as usize);
            }
        }
        self.content =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        range
    }

    /// The chip containing `offset` strictly inside it, if any.
    fn token_around(&self, offset: usize) -> Option<&MentionToken> {
        self.tokens
            .iter()
            .find(|t| t.range.start < offset && offset < t.range.end)
    }

    /// Move an offset that fell inside a chip to its nearer edge (`forward`
    /// picks the end on a tie).
    fn snap(&self, offset: usize, forward: bool) -> usize {
        match self.token_around(offset) {
            Some(t) => {
                let (a, b) = (offset - t.range.start, t.range.end - offset);
                if a < b || (a == b && !forward) {
                    t.range.start
                } else {
                    t.range.end
                }
            }
            None => offset,
        }
    }

    /// Put a chip for `name` (`@handle`, member `id`) where the open `@token`
    /// is — or at the caret when none is open — followed by one space.
    pub fn insert_mention(
        &mut self,
        handle: &str,
        name: &str,
        id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let range = match self.open_mention_range() {
            Some(r) => r,
            None => self.selected_range.clone(),
        };
        self.record_undo();
        let chip = format!("{}{name}", mention::CELL);
        let range = self.splice(range, &format!("{chip} "));
        let token = MentionToken {
            range: range.start..range.start + chip.len(),
            handle: handle.to_string(),
            name: name.to_string(),
            id,
            kind: MentionToken::kind_of(handle),
        };
        let at = self
            .tokens
            .partition_point(|t| t.range.start < token.range.start);
        self.tokens.insert(at, token);
        let caret = range.start + chip.len() + 1;
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    /// Every plain `@handle` the table knows (whole word, not shared by two
    /// entities, not already a chip) becomes a chip. Runs on Enter and after a
    /// separator is typed, so an exact handle resolves without the picker.
    pub fn resolve_exact_handles(&mut self, cx: &mut Context<Self>) -> bool {
        let found = self.mentions.find_raw(&self.content);
        let mut changed = false;
        for range in found.into_iter().rev() {
            if self
                .tokens
                .iter()
                .any(|t| t.range.start < range.end && t.range.end > range.start)
            {
                continue;
            }
            let handle = self.content[range.start + 1..range.end].to_string();
            if self.mentions.is_ambiguous(&handle) {
                continue;
            }
            let Some(m) = self.mentions.by_handle(&handle).cloned() else {
                continue;
            };
            let chip = format!("{}{}", mention::CELL, m.name);
            let caret = self.cursor_offset();
            let delta = chip.len() as isize - range.len() as isize;
            let replaced = self.splice(range.clone(), &chip);
            let token = MentionToken {
                range: replaced.start..replaced.start + chip.len(),
                kind: MentionToken::kind_of(&handle),
                handle,
                name: m.name.clone(),
                id: m.id.clone(),
            };
            let at = self
                .tokens
                .partition_point(|t| t.range.start < token.range.start);
            self.tokens.insert(at, token);
            if caret >= range.end {
                let c = (caret as isize + delta) as usize;
                self.selected_range = c..c;
            } else if caret > range.start {
                let c = replaced.start + chip.len();
                self.selected_range = c..c;
            }
            self.selection_reversed = false;
            changed = true;
        }
        if changed {
            cx.notify();
        }
        changed
    }

    pub fn set_placeholder(&mut self, text: impl Into<SharedString>) {
        self.placeholder = text.into();
    }

    pub fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.set_text_with_tokens(text, Vec::new(), cx);
    }

    /// A draft coming back with its chips (`tokens` are byte ranges in `text`).
    pub fn set_text_with_tokens(
        &mut self,
        text: String,
        tokens: Vec<MentionToken>,
        cx: &mut Context<Self>,
    ) {
        let len = text.len();
        self.content = text;
        self.tokens = tokens
            .into_iter()
            .filter(|t| t.range.end <= len && t.range.start < t.range.end)
            .collect();
        self.tokens.sort_by_key(|t| t.range.start);
        self.selected_range = len..len;
        self.selection_reversed = false;
        self.marked_range = None;
        self.scroll_y = 0.0;
        self.undo_stack.clear();
        self.redo_stack.clear();
        cx.notify();
    }

    /// Append plain text at the end (a dictated memo), keeping the chips.
    pub fn append_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let end = self.content.len();
        self.splice(end..end, text);
        let len = self.content.len();
        self.selected_range = len..len;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    /// Like `set_text`, but shows the text from its first line (form fields).
    pub fn set_text_from_start(&mut self, text: String, cx: &mut Context<Self>) {
        self.set_text(text, cx);
        self.selected_range = 0..0;
    }

    /// Select the whole contents (inline rename fields).
    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.content.clear();
        self.tokens.clear();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.scroll_y = 0.0;
        self.undo_stack.clear();
        self.redo_stack.clear();
        cx.notify();
    }

    /// Wrapped line count of the *current* text at the last laid-out width.
    /// Measured fresh here (not taken from the previous paint) so a single
    /// large insert — a paste — grows the box on the very next layout.
    pub fn line_count(&self, window: &Window, cx: &App) -> usize {
        let Some(width) = self.layout.as_ref().map(|l| l.bounds.size.width) else {
            return 1;
        };
        if self.content.is_empty() {
            return 1;
        }
        let theme = Theme::of(cx);
        let mut font = window.text_style().font();
        font.family = theme.font_sans.into();
        let run = TextRun {
            len: 0,
            font,
            color: theme.text,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let mut total = 0usize;
        for para in self.content.split('\n') {
            let runs = vec![TextRun {
                len: para.len(),
                ..run.clone()
            }];
            let wrapped = window
                .text_system()
                .shape_text(
                    SharedString::from(para.to_string()),
                    px(self.font_size),
                    &runs,
                    Some(width),
                    None,
                )
                .ok()
                .and_then(|mut v| v.pop())
                .map(|l| l.wrap_boundaries().len() + 1)
                .unwrap_or(1);
            total += wrapped;
        }
        total.max(1)
    }

    fn send(&mut self, _: &Send, window: &mut Window, cx: &mut Context<Self>) {
        if self.enter_inserts_newline {
            self.replace_text_in_range(None, "\n", window, cx);
            return;
        }
        // The mention handlers read and update this very entity (the Composer owns it), and an action
        // handler runs while the entity is leased: calling them here is a re-entrant lease, which GPUI
        // answers with a panic — every Enter crashed Mac v0.6.89/v0.6.90 (#1208). Everything the owner
        // does with the text runs deferred, after this update ends; only "is a mention open" is decided
        // here, from our own text.
        let accept = self
            .on_mention_accept
            .clone()
            .filter(|_| self.open_mention_token().is_some());
        let submit = self.on_submit.clone();
        window.defer(cx, move |window, cx| {
            if let Some(cb) = accept {
                if cb(cx) {
                    return;
                }
            }
            if let Some(cb) = submit {
                cb(window, cx);
            }
        });
    }

    fn accept_mention(&mut self, _: &AcceptMention, window: &mut Window, cx: &mut Context<Self>) {
        // Tab completes the highlighted / hovered row (same as Enter). Arrows
        // and Shift+Tab still move the highlight ([feedback
        // app-01M5WKBR85JVW1KKJH96FWZVK5]).
        self.defer_mention_key(self.on_mention_accept.clone(), window, cx);
    }

    fn prev_mention(&mut self, _: &PrevMention, window: &mut Window, cx: &mut Context<Self>) {
        self.defer_mention_key(self.on_mention_prev.clone(), window, cx);
    }

    /// Esc closes an open `@` menu; with none open the key goes on to the
    /// shell (stop a memo, close menus and sheets).
    fn escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        if !self.defer_mention_key(self.on_mention_escape.clone(), window, cx) {
            cx.propagate();
        }
    }

    /// The trailing `@token` the mention menu is open for (an `@` at a word start, no whitespace after
    /// it) — the same rule as `Composer::mention_query`, decided from our own text so no owner code
    /// runs while this entity is leased.
    pub fn open_mention_token(&self) -> Option<&str> {
        let range = self.open_mention_range()?;
        Some(&self.content[range.start + 1..range.end])
    }

    /// Bytes of the `@token` the caret sits at the end of: `@` at a word start
    /// (start of text or after whitespace), no whitespace up to the caret, not
    /// part of a chip. A chip's own `@` never counts — its cell holds a space.
    fn open_mention_range(&self) -> Option<Range<usize>> {
        if !self.selected_range.is_empty() {
            return None;
        }
        let caret = self.cursor_offset();
        let before = &self.content[..caret];
        let at = before.rfind('@')?;
        if before[at + 1..].contains(char::is_whitespace) {
            return None;
        }
        if at > 0 && !before[..at].ends_with(char::is_whitespace) {
            return None;
        }
        if self
            .tokens
            .iter()
            .any(|t| t.range.start <= at && at < t.range.end)
        {
            return None;
        }
        Some(at..caret)
    }

    /// Hand a mention-menu key to the owner, deferred (see `send`). True when a mention is open and the
    /// key was handed over; false when the caller should treat the key as plain editing.
    fn defer_mention_key(
        &self,
        cb: Option<MentionKeyHandler>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.open_mention_token().is_none() {
            return false;
        }
        let Some(cb) = cb else {
            return false;
        };
        window.defer(cx, move |_, cx| {
            let _ = cb(cx);
        });
        true
    }

    /// A picker's ↑/↓: hand the step to the owner after this update ends
    /// (same re-entrancy rule as the mention keys); `false` with no handler.
    fn defer_arrow(&self, delta: i32, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(cb) = self.on_arrow.clone() else {
            return false;
        };
        window.defer(cx, move |_, cx| cb(delta, cx));
        true
    }

    /// Complete the open `@token` with `handle`: a chip when the table knows
    /// the handle, else the plain `@handle ` text. What `Composer` does through
    /// `insert_mention` with a picker row; kept for the Enter tests.
    #[cfg(test)]
    pub fn complete_mention(&mut self, handle: &str, cx: &mut Context<Self>) {
        let Some(range) = self.open_mention_range() else {
            return;
        };
        if let Some(m) = self.mentions.by_handle(handle).cloned() {
            self.insert_mention(handle, &m.name, m.id.clone(), cx);
            return;
        }
        self.record_undo();
        let text = format!("@{handle} ");
        let range = self.splice(range, &text);
        let caret = range.start + text.len();
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            content: self.content.clone(),
            selected_range: self.selected_range.clone(),
            selection_reversed: self.selection_reversed,
            tokens: self.tokens.clone(),
        }
    }

    fn apply_snapshot(&mut self, snap: EditSnapshot) {
        self.content = snap.content;
        self.selected_range = snap.selected_range;
        self.selection_reversed = snap.selection_reversed;
        self.tokens = snap.tokens;
        self.marked_range = None;
        self.caret_downstream = false;
    }

    /// Push the current text before a user edit so ⌘Z can restore it.
    fn record_undo(&mut self) {
        let snap = self.snapshot();
        if self.undo_stack.last() == Some(&snap) {
            return;
        }
        self.undo_stack.push(snap);
        if self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(prev) = self.undo_stack.pop() else {
            return;
        };
        self.redo_stack.push(self.snapshot());
        self.apply_snapshot(prev);
        cx.notify();
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(next) = self.redo_stack.pop() else {
            return;
        };
        self.undo_stack.push(self.snapshot());
        if self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
        self.apply_snapshot(next);
        cx.notify();
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word_boundary(self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word_boundary(self.cursor_offset()), cx);
    }

    fn up(&mut self, _: &Up, window: &mut Window, cx: &mut Context<Self>) {
        if self.defer_mention_key(self.on_mention_prev.clone(), window, cx) {
            return;
        }
        if self.defer_arrow(-1, window, cx) {
            return;
        }
        if self.selected_range.is_empty() {
            self.move_to(self.offset_for_vertical_move(true), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn down(&mut self, _: &Down, window: &mut Window, cx: &mut Context<Self>) {
        if self.defer_mention_key(self.on_mention_next.clone(), window, cx) {
            return;
        }
        if self.defer_arrow(1, window, cx) {
            return;
        }
        if self.selected_range.is_empty() {
            self.move_to(self.offset_for_vertical_move(false), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_word_boundary(self.cursor_offset()), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_word_boundary(self.cursor_offset()), cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.offset_for_vertical_move(true), cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.offset_for_vertical_move(false), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    /// ⌘← / Home — NSTextView's `moveToLeftEndOfLine:`: the start of the
    /// *visual* (wrapped) line, not the paragraph.
    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.visual_line_start(self.cursor_offset()), cx);
        self.caret_downstream = true;
    }

    /// ⌘→ / End — `moveToRightEndOfLine:`: the end of the visual line (after
    /// the space the row wraps on, where the caret paints at the row's end).
    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.visual_line_end(self.cursor_offset()), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.visual_line_start(self.cursor_offset()), cx);
        self.caret_downstream = true;
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.visual_line_end(self.cursor_offset()), cx);
    }

    /// ⌘↑ — `moveToBeginningOfDocument:`.
    fn document_start(&mut self, _: &DocumentStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    /// ⌘↓ — `moveToEndOfDocument:`.
    fn document_end(&mut self, _: &DocumentEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_document_start(
        &mut self,
        _: &SelectDocumentStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(0, cx);
    }

    fn select_document_end(
        &mut self,
        _: &SelectDocumentEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.content.len(), cx);
    }

    /// ⌘⌫ — `deleteToBeginningOfLine:`: the selection if there is one, else
    /// the caret's visual line back to its start; at a row's start it takes
    /// one step back like ⌫, so the key is never dead.
    fn delete_to_line_start(
        &mut self,
        _: &DeleteToLineStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let caret = self.cursor_offset();
            let mut start = self.visual_line_start(caret);
            if start == caret {
                start = self.previous_boundary(caret);
            }
            if start == caret {
                return;
            }
            self.select_to(start, cx);
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    /// ⌘B / ⌘I / ⌘⇧X ([feedback chat-markdown-composer-2026-10-27]): wrap the
    /// selection in the chat-Markdown marker, or unwrap it when it is already
    /// wrapped; with no selection, put an empty pair around the caret so the
    /// next words are styled (a second press on the empty pair removes it).
    fn bold(&mut self, _: &Bold, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_marker("**", cx);
    }

    fn italic(&mut self, _: &Italic, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_marker("*", cx);
    }

    fn strike(&mut self, _: &Strike, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_marker("~~", cx);
    }

    pub(crate) fn toggle_marker(&mut self, marker: &str, cx: &mut Context<Self>) {
        let m = marker.len();
        let sel = self.selected_range.clone();
        if self.masked || self.content.get(sel.clone()).is_none() {
            return;
        }
        self.record_undo();
        self.marked_range = None;
        self.caret_downstream = false;
        self.selection_reversed = false;
        // The markers sit just outside the selection (or around the caret):
        // remove them.
        let before = sel
            .start
            .checked_sub(m)
            .and_then(|s| self.content.get(s..sel.start));
        let after = self.content.get(sel.end..sel.end + m);
        if before == Some(marker) && after == Some(marker) {
            self.splice(sel.end..sel.end + m, "");
            self.splice(sel.start - m..sel.start, "");
            self.selected_range = sel.start - m..sel.end - m;
            cx.notify();
            return;
        }
        // The selection holds the markers at both ends: strip them.
        let selected = &self.content[sel.clone()];
        if selected.len() >= 2 * m && selected.starts_with(marker) && selected.ends_with(marker) {
            let inner = selected[m..selected.len() - m].to_owned();
            self.splice(sel.clone(), &inner);
            self.selected_range = sel.start..sel.start + inner.len();
            cx.notify();
            return;
        }
        // Wrap: the markers hug the words, never the surrounding spaces (the
        // grammar closes a marker only against a non-space).
        let lead = selected.len() - selected.trim_start().len();
        let trail = selected.len() - selected.trim_end().len();
        let (start, end) = if lead + trail >= selected.len() {
            (sel.start, sel.end)
        } else {
            (sel.start + lead, sel.end - trail)
        };
        self.splice(end..end, marker);
        self.splice(start..start, marker);
        self.selected_range = start + m..end + m;
        cx.notify();
    }

    /// ⌫ with nothing to delete (caret at the start, no selection) is not
    /// ours: the action goes on up so the owner may act on it — the composer
    /// removes the last staged attachment when the draft is empty
    /// ([feedback composer-attachment-removal-2026-10-27]).
    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                cx.propagate();
                return;
            }
            self.select_to(prev, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    /// ⌦ at the end with nothing after the caret: same rule as `backspace`.
    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                cx.propagate();
                return;
            }
            self.select_to(next, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete_word_backward(
        &mut self,
        _: &DeleteWordBackward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let prev = self.previous_word_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                return;
            }
            self.select_to(prev, cx);
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete_word_forward(
        &mut self,
        _: &DeleteWordForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let next = self.next_word_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                return;
            }
            self.select_to(next, cx);
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        self.paste_clipboard(window, cx);
    }

    /// ⌘V / Edit → Paste: text lands at the caret; copied files and clipboard
    /// images go to the owner (`on_paste_media`), which stages them like a
    /// drop ([feedback desktop-paste-2026-10-23]). Fields without an owner
    /// hook (search, names, secrets) keep text only.
    pub(crate) fn paste_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let pasted = Pasted::from_clipboard(&item);
        if let Some(text) = pasted.text.as_deref().filter(|t| !t.is_empty()) {
            self.replace_text_in_range(None, text, window, cx);
        }
        if pasted.files.is_empty() && pasted.images.is_empty() {
            return;
        }
        // The owner reads and updates this very entity; it must run after this
        // update ends (same re-entrancy rule as `send`, #1208).
        if let Some(cb) = self.on_paste_media.clone() {
            window.defer(cx, move |_, cx| cb(pasted.files, pasted.images, cx));
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.plain_slice(self.selected_range.clone()),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.plain_slice(self.selected_range.clone()),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }
        self.is_selecting = true;
        let ix = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(ix, cx);
        } else if event.click_count == 2 && self.token_around(ix).is_some() {
            // Double-click on a chip selects the chip.
            let t = self
                .token_around(ix)
                .map(|t| t.range.clone())
                .unwrap_or(ix..ix);
            self.selected_range = t;
            self.selection_reversed = false;
            cx.notify();
        } else if event.click_count >= 3 {
            // Triple-click: select the entire chat input (macOS text-field
            // convention; matches ⌘A / select_all_text).
            self.selected_range = 0..self.content.len();
            self.selection_reversed = false;
            cx.notify();
        } else if event.click_count == 2 {
            // Double-click: select the Unicode word under the pointer —
            // same word_range the transcript bubbles use.
            self.selected_range = word_range(&self.content, ix);
            self.selection_reversed = false;
            cx.notify();
        } else {
            self.move_to(ix, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            let ix = self.index_for_mouse_position(event.position);
            self.select_to(ix, cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.caret_downstream = false;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let Some(layout) = self.layout.as_ref() else {
            return 0;
        };
        let ix = index_for_position(
            &layout.lines,
            layout.bounds,
            position,
            self.content.len(),
            self.text_center,
        );
        self.snap(ix, true)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.caret_downstream = false;
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    /// One step left: a whole chip when the caret sits at its end.
    fn previous_boundary(&self, offset: usize) -> usize {
        if let Some(t) = self.tokens.iter().find(|t| t.range.end == offset) {
            return t.range.start;
        }
        let prev = self
            .content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0);
        self.snap(prev, false)
    }

    /// One step right: a whole chip when the caret sits at its start.
    fn next_boundary(&self, offset: usize) -> usize {
        if let Some(t) = self.tokens.iter().find(|t| t.range.start == offset) {
            return t.range.end;
        }
        let next = self
            .content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len());
        self.snap(next, true)
    }

    fn previous_word_boundary(&self, offset: usize) -> usize {
        if let Some(t) = self.tokens.iter().find(|t| t.range.end == offset) {
            return t.range.start;
        }
        self.snap(previous_word_boundary(&self.content, offset), false)
    }

    fn next_word_boundary(&self, offset: usize) -> usize {
        if let Some(t) = self.tokens.iter().find(|t| t.range.start == offset) {
            return t.range.end;
        }
        self.snap(next_word_boundary(&self.content, offset), true)
    }

    fn line_start(&self, offset: usize) -> usize {
        line_start(&self.content, offset)
    }

    fn line_end(&self, offset: usize) -> usize {
        line_end(&self.content, offset)
    }

    /// Whether `offset` paints on the row below its soft-wrap boundary: only
    /// the caret carries that affinity, and only right after ⌘← / ⇧⌘←.
    fn downstream_at(&self, offset: usize) -> bool {
        self.caret_downstream && offset == self.cursor_offset()
    }

    /// Byte range of the visual (wrapped) line holding `offset`; the hard
    /// line before the first paint.
    fn visual_line(&self, offset: usize) -> Range<usize> {
        let fallback = self.line_start(offset)..self.line_end(offset);
        let Some(layout) = self.layout.as_ref() else {
            return fallback;
        };
        visual_line_range(&layout.lines, offset, self.downstream_at(offset)).unwrap_or(fallback)
    }

    /// Start of the visual line (⌘← / Home), never inside a chip.
    fn visual_line_start(&self, offset: usize) -> usize {
        self.snap(self.visual_line(offset).start, false)
    }

    /// End of the visual line (⌘→ / End), never inside a chip.
    fn visual_line_end(&self, offset: usize) -> usize {
        self.snap(self.visual_line(offset).end, true)
    }

    /// Byte offset one wrapped line above/below the caret, keeping column when
    /// the layout is known; falls back to hard newlines before the first paint.
    ///
    /// Past the first/last wrapped line, jump to document start/end (macOS
    /// text-field behaviour). Without that, a point below the last line is
    /// still hit-tested on that line and `closest_index_for_position` can
    /// return 0 — so Shift+Down selected left instead of right.
    fn offset_for_vertical_move(&self, up: bool) -> usize {
        let cursor = self.cursor_offset();
        let Some(layout) = self.layout.as_ref() else {
            return self.offset_for_hard_line_move(up);
        };
        if layout.lines.is_empty() {
            return cursor;
        }
        let Some(pos) = caret_position(
            &layout.lines,
            cursor,
            layout.bounds.size.width,
            self.text_center,
            self.caret_downstream,
        ) else {
            return self.offset_for_hard_line_move(up);
        };
        let lh = px(INPUT_LINE_HEIGHT);
        let target_y = if up { pos.y - lh } else { pos.y + lh };
        if up && target_y < px(0.0) {
            return 0;
        }
        let height = px(layout
            .lines
            .iter()
            .map(|(_, line)| f32::from(line.size(lh).height))
            .sum::<f32>());
        if !up && target_y >= height {
            return self.content.len();
        }
        let window_pos = point(layout.bounds.left() + pos.x, layout.bounds.top() + target_y);
        let ix = index_for_position(
            &layout.lines,
            layout.bounds,
            window_pos,
            self.content.len(),
            self.text_center,
        );
        self.snap(ix, !up)
    }

    fn offset_for_hard_line_move(&self, up: bool) -> usize {
        let cursor = self.cursor_offset();
        let line_start = self.line_start(cursor);
        let col = cursor - line_start;
        if up {
            if line_start == 0 {
                return 0;
            }
            let prev_end = line_start - 1;
            let prev_start = self.line_start(prev_end);
            prev_start + col.min(prev_end - prev_start)
        } else {
            let line_end = self.line_end(cursor);
            if line_end >= self.content.len() {
                return self.content.len();
            }
            let next_start = line_end + 1;
            let next_end = self.line_end(next_start);
            next_start + col.min(next_end - next_start)
        }
    }

    fn position_for_index(&self, index: usize) -> Option<Point<Pixels>> {
        let layout = self.layout.as_ref()?;
        position_for_index(
            &layout.lines,
            index,
            layout.bounds.size.width,
            self.text_center,
        )
    }
}

/// Horizontal shift so a short line sits in the middle of `bounds_width`.
fn line_center_offset(line: &WrappedLine, bounds_width: Pixels, text_center: bool) -> Pixels {
    if !text_center {
        return px(0.0);
    }
    let lh = px(INPUT_LINE_HEIGHT);
    let text_w = line.size(lh).width;
    ((bounds_width - text_w) / 2.).max(px(0.0))
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Start of the previous word (Option+Left / Option+Backspace; Control+Left
/// where macOS does not claim it for Spaces). Skips whitespace, then one run of
/// word or punctuation characters.
fn previous_word_boundary(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    let mut chars: Vec<(usize, char)> = text[..offset].char_indices().collect();
    while chars.last().is_some_and(|(_, c)| c.is_whitespace()) {
        chars.pop();
    }
    if chars.is_empty() {
        return 0;
    }
    let class = is_word_char(chars[chars.len() - 1].1);
    while chars
        .last()
        .is_some_and(|(_, c)| !c.is_whitespace() && is_word_char(*c) == class)
    {
        chars.pop();
    }
    chars.last().map(|(i, c)| i + c.len_utf8()).unwrap_or(0)
}

/// End of the next word (Option+Right / Option+Forward-Delete).
fn next_word_boundary(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    let slice = &text[offset..];
    let mut iter = slice.char_indices().peekable();
    while iter.peek().is_some_and(|(_, c)| c.is_whitespace()) {
        iter.next();
    }
    let Some(&(_, first)) = iter.peek() else {
        return text.len();
    };
    let class = is_word_char(first);
    let mut end = offset;
    for (i, c) in iter {
        if c.is_whitespace() || is_word_char(c) != class {
            return offset + i;
        }
        end = offset + i + c.len_utf8();
    }
    end
}

/// Start of the hard line (paragraph) holding `offset` — the ⌘← fallback
/// before the first paint and the ↑/↓ column keeper.
fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

/// End of the hard line (paragraph) holding `offset`.
fn line_end(text: &str, offset: usize) -> usize {
    text[offset..]
        .find('\n')
        .map(|i| offset + i)
        .unwrap_or(text.len())
}

/// Byte indices where `line` soft-wraps: each is the first byte of a new row
/// (the row above keeps the space it wrapped on).
fn wrap_indices(line: &WrappedLine) -> impl Iterator<Item = usize> + '_ {
    let runs = line.runs();
    line.wrap_boundaries()
        .iter()
        .map(move |b| runs[b.run_ix].glyphs[b.glyph_ix].index)
}

/// Byte range of the visual (wrapped) row holding `index`. An index that
/// sits on a wrap boundary belongs to the row above it (upstream, where a
/// caret paints by default) unless `downstream`.
fn visual_line_range(
    lines: &[(usize, WrappedLine)],
    index: usize,
    downstream: bool,
) -> Option<Range<usize>> {
    let (start, line) = lines
        .iter()
        .find(|(start, line)| index >= *start && index <= start + line.len())?;
    let rel = index - start;
    let mut row_start = 0;
    for wrap in wrap_indices(line) {
        let below = if downstream { rel >= wrap } else { rel > wrap };
        if below {
            row_start = wrap;
        } else {
            return Some(start + row_start..start + wrap);
        }
    }
    Some(start + row_start..start + line.len())
}

/// Where the caret paints: `position_for_index`, except that a caret with
/// downstream affinity on a wrap boundary paints at the start of the row below.
fn caret_position(
    lines: &[(usize, WrappedLine)],
    index: usize,
    bounds_width: Pixels,
    text_center: bool,
    downstream: bool,
) -> Option<Point<Pixels>> {
    let upstream = position_for_index(lines, index, bounds_width, text_center)?;
    if !downstream {
        return Some(upstream);
    }
    let (start, line) = lines
        .iter()
        .find(|(start, line)| index >= *start && index <= start + line.len())?;
    if !wrap_indices(line).any(|wrap| start + wrap == index) {
        return Some(upstream);
    }
    let ox = line_center_offset(line, bounds_width, text_center);
    Some(point(ox, upstream.y + px(INPUT_LINE_HEIGHT)))
}

/// Pixel position (relative to the text origin) of a byte index.
fn position_for_index(
    lines: &[(usize, WrappedLine)],
    index: usize,
    bounds_width: Pixels,
    text_center: bool,
) -> Option<Point<Pixels>> {
    let lh = px(INPUT_LINE_HEIGHT);
    let mut y = px(0.0);
    for (start, line) in lines {
        let end = start + line.len();
        if index >= *start && index <= end {
            let p = line.position_for_index(index - start, lh)?;
            let ox = line_center_offset(line, bounds_width, text_center);
            return Some(point(p.x + ox, p.y + y));
        }
        y += line.size(lh).height;
    }
    None
}

/// Byte index closest to a window position.
fn index_for_position(
    lines: &[(usize, WrappedLine)],
    bounds: Bounds<Pixels>,
    position: Point<Pixels>,
    content_len: usize,
    text_center: bool,
) -> usize {
    let lh = px(INPUT_LINE_HEIGHT);
    if position.y < bounds.top() {
        return 0;
    }
    let mut y = bounds.top();
    let count = lines.len();
    for (i, (start, line)) in lines.iter().enumerate() {
        let h = line.size(lh).height;
        if position.y < y + h || i + 1 == count {
            let ox = line_center_offset(line, bounds.size.width, text_center);
            let local = point(
                position.x - bounds.left() - ox,
                (position.y - y).max(px(0.0)),
            );
            let ix = match line.closest_index_for_position(local, lh) {
                Ok(ix) | Err(ix) => ix,
            };
            return (start + ix.min(line.len())).min(content_len);
        }
        y += h;
    }
    content_len
}

impl EntityInputHandler for ComposerInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        // IME commits keep `marked_range`; the checkpoint was taken when
        // composition started. Every other edit (paste, word-delete, typing)
        // records here so ⌘Z restores the prior text.
        if self.marked_range.is_none() {
            self.record_undo();
        }
        let was_marked = self.marked_range.take().is_some();
        let range = self.splice(range, new_text);
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.selection_reversed = false;
        self.caret_downstream = false;
        // `@handle` + a separator typed right after it: the handle becomes a chip
        // without the picker (behaviour 5).
        let mut chars = new_text.chars();
        if !was_marked
            && matches!((chars.next(), chars.next()), (Some(c), None) if completes_handle(c))
        {
            self.resolve_exact_handles(cx);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        // One undo step for the whole IME composition: checkpoint only at start.
        if self.marked_range.is_none() {
            self.record_undo();
        }
        let range = self.splice(range, new_text);
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let start = self.position_for_index(range.start)?;
        let end = self.position_for_index(range.end).unwrap_or(start);
        let dy = px(self.scroll_y);
        Some(Bounds::from_corners(
            point(bounds.left() + start.x, bounds.top() + start.y - dy),
            point(
                bounds.left() + end.x,
                bounds.top() + end.y + px(INPUT_LINE_HEIGHT) - dy,
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let ix = self.index_for_mouse_position(point);
        Some(self.offset_to_utf16(ix))
    }
}

struct InputElement {
    input: Entity<ComposerInput>,
}

struct Prepaint {
    lines: Vec<(usize, WrappedLine)>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
    /// Mention chips (#1596): pills under the text, pictures over it.
    pills: Vec<PaintQuad>,
    pictures: Vec<(Bounds<Pixels>, mention::Picture)>,
    /// The chips the pointer can click: the token's range, its entity and
    /// the pill rectangles (window coordinates) it covers.
    chip_hits: Vec<ChipHit>,
    /// Whether a pill is mid-fade (paint again next frame).
    fading: bool,
    /// Live quote bars: one per `> ` paragraph, in the field's left padding
    /// (painted outside the text's clip).
    bars: Vec<PaintQuad>,
    hitbox: Hitbox,
    scroll_y: f32,
}

#[derive(Clone)]
struct ChipHit {
    range: Range<usize>,
    mention: Rc<Mention>,
    pills: Vec<Bounds<Pixels>>,
}

impl ChipHit {
    fn contains(&self, position: Point<Pixels>) -> bool {
        self.pills.iter().any(|b| b.contains(&position))
    }
}

/// The clickable chip under `position`, if any.
fn chip_hit_at(hits: &[ChipHit], position: Point<Pixels>) -> Option<&ChipHit> {
    hits.iter()
        .find(|h| h.mention.page.is_some() && h.contains(position))
}

impl IntoElement for InputElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for InputElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let lines = self.input.read(cx).line_count(window, cx).min(MAX_LINES);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = px(INPUT_LINE_HEIGHT * lines as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let theme = Theme::of(cx).clone();
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let marked = input.marked_range.clone();
        let text_center = input.text_center;
        let font_size = input.font_size;
        let style = window.text_style();
        let lh = px(INPUT_LINE_HEIGHT);

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.to_string(), theme.text_faint)
        } else if input.masked {
            (
                content.chars().map(|c| "*".repeat(c.len_utf8())).collect(),
                theme.text,
            )
        } else {
            (content.clone(), theme.text)
        };
        // Known `@handle` / `@here` / `@agents` in the live text (not the
        // placeholder, not a masked field).
        let mention_ranges = if content.is_empty() || input.masked {
            Vec::new()
        } else {
            input.mentions.find_raw(&display_text)
        };
        let mention_accent = theme.accent;
        let tokens: Vec<MentionToken> = if content.is_empty() || input.masked {
            Vec::new()
        } else {
            input.tokens.clone()
        };
        let token_mentions: Vec<Option<Rc<Mention>>> = tokens
            .iter()
            .map(|t| input.mentions.mention_for(t).cloned())
            .collect();
        let now = Instant::now();
        let snap = cx.reduce_motion();
        let mut fading = false;
        let pill_alphas: Vec<f32> = tokens
            .iter()
            .map(|t| {
                let (alpha, still) = input.chips.alpha(&t.range, now, snap);
                fading |= still;
                alpha
            })
            .collect();

        let mut font = style.font();
        font.family = theme.font_sans.into();
        let base_run = TextRun {
            len: 0,
            font,
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        // One WrappedLine per '\n'-separated paragraph.
        let mut lines = Vec::new();
        let mut quote_paragraphs: Vec<usize> = Vec::new();
        let mut start = 0usize;
        for para in display_text.split('\n') {
            let mut runs = runs_for(para, start, marked.as_ref(), &base_run);
            let para_end = start + para.len();
            for range in &mention_ranges {
                if range.end <= start || range.start >= para_end {
                    continue;
                }
                let local_start = range.start.saturating_sub(start);
                let local_end = range.end.min(para_end).saturating_sub(start);
                runs = accent_runs(runs, local_start..local_end, mention_accent);
            }
            // Chips: the `@` cell transparent (the picture goes there), the name
            // in the accent — the same recolouring as bubble chips.
            for t in &tokens {
                if t.range.end <= start || t.range.start >= para_end {
                    continue;
                }
                let local = t.range.start - start..t.range.end.min(para_end) - start;
                runs = select::chip_runs(runs, local, mention::CELL.len(), mention_accent);
            }
            // Live chat-Markdown styling ([feedback chat-markdown-composer-2026-10-27]):
            // markers faint, the words between them in their style, a `> ` line
            // muted with a bar beside it — what the bubble will show.
            if !content.is_empty() && !input.masked {
                let quoted = live_styles(para, &mut runs, &theme);
                if quoted {
                    quote_paragraphs.push(lines.len());
                }
            }
            let shaped = window
                .text_system()
                .shape_text(
                    SharedString::from(para.to_string()),
                    px(font_size),
                    &runs,
                    Some(bounds.size.width),
                    None,
                )
                .ok()
                .and_then(|mut v| v.pop());
            if let Some(line) = shaped {
                lines.push((start, line));
            }
            start += para.len() + 1;
        }

        // Keep the caret inside the visible box once the text is taller than
        // MAX_LINES; everything below is painted shifted by -scroll_y.
        let total_h: f32 = lines
            .iter()
            .map(|(_, l)| f32::from(l.size(lh).height))
            .sum();
        let visible_h = f32::from(bounds.size.height);
        let mut scroll_y = input.scroll_y;
        let cursor_pos = if content.is_empty() {
            let ox = lines
                .first()
                .map(|(_, line)| line_center_offset(line, bounds.size.width, text_center))
                .unwrap_or_else(|| {
                    if text_center {
                        bounds.size.width / 2.
                    } else {
                        px(0.0)
                    }
                });
            Some(point(ox, px(0.0)))
        } else {
            caret_position(
                &lines,
                cursor,
                bounds.size.width,
                text_center,
                input.caret_downstream,
            )
        };
        if let Some(p) = cursor_pos {
            let cy = f32::from(p.y);
            if cy < scroll_y {
                scroll_y = cy;
            } else if cy + f32::from(lh) > scroll_y + visible_h {
                scroll_y = cy + f32::from(lh) - visible_h;
            }
        }
        scroll_y = scroll_y.clamp(0.0, (total_h - visible_h).max(0.0));
        let dy = px(scroll_y);

        // The quote bars: beside each `> ` paragraph, the height of its rows.
        let mut bars = Vec::new();
        let mut top = px(0.0);
        for (ix, (_, line)) in lines.iter().enumerate() {
            let h = line.size(lh).height;
            if quote_paragraphs.contains(&ix) {
                bars.push(
                    fill(
                        Bounds::new(
                            point(
                                bounds.left() - px(QUOTE_BAR_GAP + transcript::QUOTE_BAR),
                                bounds.top() + top - dy + px(QUOTE_BAR_INSET),
                            ),
                            size(px(transcript::QUOTE_BAR), h - px(QUOTE_BAR_INSET * 2.0)),
                        ),
                        Theme::with_alpha(theme.text_muted, 0.55),
                    )
                    .corner_radii(Corners::all(px(transcript::QUOTE_BAR / 2.0))),
                );
            }
            top += h;
        }

        // Chip pills (one per wrapped-line fragment) and pictures, in element
        // coordinates shifted by the scroll.
        let mut pills = Vec::new();
        let mut pictures = Vec::new();
        let mut chip_hits = Vec::new();
        for ((t, mention), alpha) in tokens.iter().zip(token_mentions).zip(pill_alphas) {
            let (Some(a), Some(b)) = (
                position_for_index(&lines, t.range.start, bounds.size.width, text_center),
                position_for_index(&lines, t.range.end, bounds.size.width, text_center),
            ) else {
                continue;
            };
            let fragments: Vec<(Pixels, Pixels, Pixels)> = if a.y == b.y {
                vec![(a.x, b.x, a.y)]
            } else {
                vec![(a.x, bounds.size.width, a.y), (px(0.0), b.x, b.y)]
            };
            let mut rects = Vec::with_capacity(fragments.len());
            for (x0, x1, top) in fragments {
                let pill = Bounds::new(
                    point(
                        bounds.left() + x0 - px(mention::PILL_PAD_X),
                        bounds.top() + top - dy - px(mention::PILL_PAD_Y),
                    ),
                    size(
                        x1 - x0 + px(mention::PILL_PAD_X * 2.0),
                        lh + px(mention::PILL_PAD_Y * 2.0),
                    ),
                );
                rects.push(pill);
                pills.push(
                    fill(pill, mention::pill_color(mention_accent, alpha))
                        .corner_radii(Corners::all(px(mention::PILL_RADIUS))),
                );
            }
            let Some(mention) = mention else {
                continue;
            };
            chip_hits.push(ChipHit {
                range: t.range.clone(),
                mention: mention.clone(),
                pills: rects,
            });
            let picture = mention.picture.clone();
            let Some(after_cell) = position_for_index(
                &lines,
                t.range.start + mention::CELL.len(),
                bounds.size.width,
                text_center,
            ) else {
                continue;
            };
            if after_cell.y != a.y {
                continue;
            }
            let cell_w = after_cell.x - a.x;
            let d = px(mention::PICTURE)
                .min(lh)
                .min(cell_w - px(mention::PICTURE_GAP));
            if d <= px(4.0) {
                continue;
            }
            pictures.push((
                Bounds::new(
                    point(
                        bounds.left() + a.x,
                        bounds.top() + a.y - dy + (lh - d) / 2.0,
                    ),
                    size(d, d),
                ),
                picture,
            ));
        }

        let mut cursor_quad = None;
        let mut selections = Vec::new();
        if content.is_empty() || selected_range.is_empty() {
            if let Some(pos) = cursor_pos {
                cursor_quad = Some(fill(
                    Bounds::new(
                        point(bounds.left() + pos.x, bounds.top() + pos.y - dy + px(1.0)),
                        size(px(1.5), lh - px(2.0)),
                    ),
                    theme.accent,
                ));
            }
        } else {
            // A selection that starts on a wrap boundary begins where the
            // row below starts (no empty sliver at the end of the row above).
            let a = caret_position(
                &lines,
                selected_range.start,
                bounds.size.width,
                text_center,
                true,
            );
            let b = position_for_index(&lines, selected_range.end, bounds.size.width, text_center);
            if let (Some(a), Some(b)) = (a, b) {
                if a.y == b.y {
                    selections.push(fill(
                        Bounds::from_corners(
                            point(bounds.left() + a.x, bounds.top() + a.y - dy),
                            point(bounds.left() + b.x, bounds.top() + b.y - dy + lh),
                        ),
                        theme.accent_soft,
                    ));
                } else {
                    selections.push(fill(
                        Bounds::from_corners(
                            point(bounds.left() + a.x, bounds.top() + a.y - dy),
                            point(bounds.right(), bounds.top() + a.y - dy + lh),
                        ),
                        theme.accent_soft,
                    ));
                    if b.y > a.y + lh {
                        selections.push(fill(
                            Bounds::from_corners(
                                point(bounds.left(), bounds.top() + a.y - dy + lh),
                                point(bounds.right(), bounds.top() + b.y - dy),
                            ),
                            theme.accent_soft,
                        ));
                    }
                    selections.push(fill(
                        Bounds::from_corners(
                            point(bounds.left(), bounds.top() + b.y - dy),
                            point(bounds.left() + b.x, bounds.top() + b.y - dy + lh),
                        ),
                        theme.accent_soft,
                    ));
                }
            }
        }

        Prepaint {
            lines,
            cursor: cursor_quad,
            selections,
            pills,
            pictures,
            chip_hits,
            fading,
            bars,
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
            scroll_y,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let lh = px(INPUT_LINE_HEIGHT);
        let scroll_y = prepaint.scroll_y;
        let focused = focus_handle.is_focused(window);
        // The bars sit in the left padding, so they get a mask one gutter wider.
        let gutter = Bounds::new(
            point(
                bounds.left() - px(QUOTE_BAR_GAP + transcript::QUOTE_BAR),
                bounds.top(),
            ),
            size(
                bounds.size.width + px(QUOTE_BAR_GAP + transcript::QUOTE_BAR),
                bounds.size.height,
            ),
        );
        window.with_content_mask(Some(gpui::ContentMask { bounds: gutter }), |window| {
            for bar in prepaint.bars.drain(..) {
                window.paint_quad(bar);
            }
        });
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for pill in prepaint.pills.drain(..) {
                window.paint_quad(pill);
            }
            for sel in prepaint.selections.drain(..) {
                window.paint_quad(sel);
            }
            let text_center = self.input.read(cx).text_center;
            let align = if text_center {
                TextAlign::Center
            } else {
                TextAlign::Left
            };
            let align_bounds = text_center.then_some(bounds);
            let mut y = bounds.top() - px(scroll_y);
            for (_, line) in &prepaint.lines {
                let h = line.size(lh).height;
                if y + h >= bounds.top() && y <= bounds.bottom() {
                    let _ =
                        line.paint(point(bounds.left(), y), lh, align, align_bounds, window, cx);
                }
                y += h;
            }
            for (bounds, picture) in prepaint.pictures.drain(..) {
                select::paint_picture(&picture, bounds, window, cx);
            }
            if focused {
                if let Some(cursor) = prepaint.cursor.take() {
                    window.paint_quad(cursor);
                }
            }
        });
        #[cfg(test)]
        let chip_rects: Vec<(Range<usize>, Vec<Bounds<Pixels>>)> = prepaint
            .chip_hits
            .iter()
            .map(|h| (h.range.clone(), h.pills.clone()))
            .collect();
        self.paint_chip_pointer(prepaint, window, cx);
        let lines = std::mem::take(&mut prepaint.lines);
        let measured: usize = lines
            .iter()
            .map(|(_, l)| l.wrap_boundaries().len() + 1)
            .sum::<usize>()
            .max(1);
        let laid_out = (f32::from(bounds.size.height) / INPUT_LINE_HEIGHT).round() as usize;
        let width_changed = self
            .input
            .read(cx)
            .layout
            .as_ref()
            .map(|l| l.bounds.size.width != bounds.size.width)
            .unwrap_or(true);
        self.input.update(cx, |input, _| {
            input.layout = Some(Layout {
                lines,
                bounds,
                #[cfg(test)]
                chips: chip_rects,
            });
            input.scroll_y = scroll_y;
        });
        // First layout after a width change, a big insert, or a programmatic
        // `set_text`: re-layout once more with the measured height. A notify is
        // ignored mid-draw; a frame request is not.
        let grew = width_changed || measured.min(MAX_LINES) != laid_out.max(1);
        if grew {
            window.request_animation_frame();
        }
    }
}

impl InputElement {
    /// The pointer over the chips (Jacob, Oct 24): a pointing hand and a
    /// brighter pill under the pointer, brighter still while pressed, and a
    /// plain click (no drag) opens the entity's page — the same as a bubble
    /// chip. The caret still lands at the chip's edge as before.
    fn paint_chip_pointer(&self, prepaint: &mut Prepaint, window: &mut Window, cx: &mut App) {
        if prepaint.fading {
            window.request_animation_frame();
        }
        let hits: Rc<Vec<ChipHit>> = Rc::new(std::mem::take(&mut prepaint.chip_hits));
        let hitbox = prepaint.hitbox.clone();
        let now = Instant::now();
        let under_pointer = hitbox
            .is_hovered(window)
            .then(|| chip_hit_at(&hits, window.mouse_position()))
            .flatten()
            .map(|h| h.range.clone());
        self.input.update(cx, |input, _| {
            input.chips.tidy(now);
            // The text moved under a still pointer: a hot chip that is no
            // longer under it cools now, not at the next mouse move.
            if under_pointer.is_none() && input.chips.hot().is_some() {
                input.chips.hover(None, now);
            }
        });
        if hits.is_empty() && self.input.read(cx).chips.hot().is_none() {
            return;
        }
        let over = under_pointer.is_some();
        if over {
            window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
        }
        #[cfg(test)]
        if hitbox.is_hovered(window) {
            self.input.update(cx, |input, _| {
                input.chip_cursor = if over {
                    CursorStyle::PointingHand
                } else {
                    CursorStyle::IBeam
                }
            });
        }

        window.on_mouse_event({
            let (hitbox, hits, input) = (hitbox.clone(), hits.clone(), self.input.clone());
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let dragging_text = event.pressed_button == Some(MouseButton::Left)
                    && input.read(cx).chips.pressed().is_none();
                let over = (hitbox.is_hovered(window) && !dragging_text)
                    .then(|| chip_hit_at(&hits, event.position))
                    .flatten()
                    .map(|h| h.range.clone());
                if input.update(cx, |input, _| input.chips.hover(over, Instant::now())) {
                    window.refresh();
                }
            }
        });
        window.on_mouse_event({
            let (hitbox, hits, input) = (hitbox.clone(), hits.clone(), self.input.clone());
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !hitbox.is_hovered(window)
                {
                    return;
                }
                let Some(hit) = chip_hit_at(&hits, event.position) else {
                    return;
                };
                let range = hit.range.clone();
                if input.update(cx, |input, _| input.chips.press(range, Instant::now())) {
                    window.refresh();
                }
            }
        });
        window.on_mouse_event({
            let input = self.input.clone();
            move |event: &MouseUpEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                let pressed = input.update(cx, |input, _| input.chips.release(Instant::now()));
                let Some(range) = pressed else {
                    return;
                };
                window.refresh();
                let hit = hits
                    .iter()
                    .find(|h| h.range == range && h.contains(event.position));
                let (Some(hit), true) = (hit, event.click_count == 1) else {
                    return;
                };
                // A drag that selected text is not a click.
                if !input.read(cx).selected_range.is_empty() {
                    return;
                }
                let mentions = input.read(cx).mentions.clone();
                mentions.open(&hit.mention, cx);
            }
        });
    }
}

/// The live quote bar's distance from the text and its vertical inset in the row.
const QUOTE_BAR_GAP: f32 = 5.0;
const QUOTE_BAR_INSET: f32 = 2.0;

/// Restyle one paragraph's runs by the chat-Markdown grammar as it is typed:
/// marker bytes (`**`, `` ` ``, `~~`, `||`, `[`, `](…)`) in the faint ink, the
/// words between them bold / italic / struck / mono on the code wash / on the
/// spoiler wash, a `> ` line's markers faint and its words muted. Returns
/// whether the paragraph is a quote line (the caller draws its bar). The runs'
/// total length never changes, so the layout is the typed text's.
fn live_styles(para: &str, runs: &mut Vec<TextRun>, theme: &Theme) -> bool {
    let quote = arbos_core::markdown::quote_marker_len(para);
    let body_from = quote.unwrap_or(0);
    if let Some(n) = quote {
        *runs = recolor(std::mem::take(runs), 0..n, theme.text_faint);
        *runs = recolor(std::mem::take(runs), n..para.len(), theme.text_muted);
    }
    let body = &para[body_from..];
    if !body.contains(['*', '_', '~', '|', '`', '[']) {
        return quote.is_some();
    }
    for range in arbos_core::markdown::inline_markers(body) {
        let range = range.start + body_from..range.end + body_from;
        *runs = recolor(std::mem::take(runs), range, theme.text_faint);
    }
    for span in arbos_core::markdown::inline_spans(body) {
        let st = span.style;
        if st == arbos_core::markdown::InlineStyle::default() || span.source.is_empty() {
            continue;
        }
        let range = span.source.start + body_from..span.source.end + body_from;
        *runs = restyle(std::mem::take(runs), range, |run| {
            if st.code {
                run.font.family = theme.font_mono.into();
                run.background_color = Some(theme.code_bg);
            }
            if st.bold {
                run.font.weight = FontWeight::BOLD;
            }
            if st.italic {
                run.font.style = gpui::FontStyle::Italic;
            }
            if st.strike {
                run.strikethrough = Some(gpui::StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(run.color),
                });
            }
            if st.spoiler != 0 {
                run.background_color = Some(transcript::spoiler_fill(theme.text, theme, true));
            }
        });
    }
    quote.is_some()
}

/// Recolour the bytes in `range`, splitting runs at its edges.
fn recolor(runs: Vec<TextRun>, range: Range<usize>, color: gpui::Hsla) -> Vec<TextRun> {
    restyle(runs, range, |run| run.color = color)
}

/// Apply `f` to the runs covering `range` (split at its edges); the total
/// length is unchanged.
fn restyle(runs: Vec<TextRun>, range: Range<usize>, f: impl Fn(&mut TextRun)) -> Vec<TextRun> {
    if range.is_empty() {
        return runs;
    }
    let mut out = Vec::with_capacity(runs.len() + 2);
    let mut pos = 0usize;
    for run in runs {
        let run_end = pos + run.len;
        let mut cursor = pos;
        for cut in [range.start, range.end, run_end] {
            if cut <= cursor || cut > run_end {
                continue;
            }
            let mut piece = TextRun {
                len: cut - cursor,
                ..run.clone()
            };
            if cursor >= range.start && cursor < range.end {
                f(&mut piece);
            }
            out.push(piece);
            cursor = cut;
        }
        pos = run_end;
    }
    out
}

fn runs_for(
    para: &str,
    para_start: usize,
    marked: Option<&Range<usize>>,
    base: &TextRun,
) -> Vec<TextRun> {
    let len = para.len();
    match marked {
        Some(m) if m.start < para_start + len && m.end > para_start => {
            let s = m.start.saturating_sub(para_start).min(len);
            let e = (m.end - para_start).min(len);
            vec![
                TextRun {
                    len: s,
                    ..base.clone()
                },
                TextRun {
                    len: e - s,
                    underline: Some(UnderlineStyle {
                        color: Some(base.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..base.clone()
                },
                TextRun {
                    len: len - e,
                    ..base.clone()
                },
            ]
            .into_iter()
            .filter(|r| r.len > 0)
            .collect()
        }
        Some(_) | None => vec![TextRun {
            len,
            ..base.clone()
        }],
    }
}

/// Recolour a known `@handle` run in the composer with the accent.
fn accent_runs(runs: Vec<TextRun>, range: Range<usize>, accent: gpui::Hsla) -> Vec<TextRun> {
    if range.is_empty() {
        return runs;
    }
    let mut out = Vec::with_capacity(runs.len() + 2);
    let mut pos = 0usize;
    for run in runs {
        let run_end = pos + run.len;
        let mut cursor = pos;
        for cut in [range.start, range.end, run_end] {
            if cut <= cursor || cut > run_end {
                continue;
            }
            let inside = cursor >= range.start && cursor < range.end;
            out.push(TextRun {
                len: cut - cursor,
                color: if inside { accent } else { run.color },
                ..run.clone()
            });
            cursor = cut;
        }
        pos = run_end;
    }
    out
}

impl Render for ComposerInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .key_context("ComposerInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::send))
            .on_action(cx.listener(Self::accept_mention))
            .on_action(cx.listener(Self::prev_mention))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_backward))
            .on_action(cx.listener(Self::delete_word_forward))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::document_start))
            .on_action(cx.listener(Self::document_end))
            .on_action(cx.listener(Self::select_document_start))
            .on_action(cx.listener(Self::select_document_end))
            .on_action(cx.listener(Self::delete_to_line_start))
            .on_action(cx.listener(Self::bold))
            .on_action(cx.listener(Self::italic))
            .on_action(cx.listener(Self::strike))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .line_height(px(INPUT_LINE_HEIGHT))
            .text_size(px(self.font_size))
            .child(InputElement { input: cx.entity() })
    }
}

impl Focusable for ComposerInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// A file staged in the composer; image paths carry a decoded thumbnail for the pill.
struct StagedAttachment {
    path: PathBuf,
    preview: Option<Arc<RenderImage>>,
    bytes: u64,
}

impl StagedAttachment {
    fn new(path: PathBuf) -> Self {
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Self {
            preview: image_preview_for(&path),
            path,
            bytes,
        }
    }

    fn file_name(&self) -> String {
        self.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("attachment")
            .to_string()
    }

    /// "156 kB" style size, like the Library's file rows.
    fn size_label(&self) -> String {
        let b = self.bytes;
        if b < 1024 {
            format!("{b} B")
        } else if b < 1024 * 1024 {
            format!("{:.0} kB", b as f64 / 1024.0)
        } else {
            format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
        }
    }
}

/// The composer card while files are pending (redesign mock e): 64 pt tiles.
pub(crate) const STAGED_TILE: f32 = 64.0;
const CARD_RADIUS: f32 = 16.0;
/// The staged strip ([feedback composer-attachment-removal-2026-10-27]): one
/// horizontal row, `STAGED_GAP` between tiles, scrolls sideways past the card's
/// width. Every tile is one rounded rect (`STAGED_RADIUS`, the theme's 1 pt
/// hairline) with the remove disc inside its top-right corner — always drawn,
/// `STAGED_REMOVE` across (the hit target), `STAGED_REMOVE_INSET` from the
/// edges, so it never reaches the neighbour and nothing is clipped.
pub(crate) const STAGED_GAP: f32 = 8.0;
pub(crate) const STAGED_RADIUS: f32 = 10.0;
pub(crate) const STAGED_REMOVE: f32 = 20.0;
pub(crate) const STAGED_REMOVE_INSET: f32 = 4.0;
/// A file chip's widest; its name truncates past this.
const STAGED_CHIP_MAX_WIDTH: f32 = 320.0;
pub(crate) const STAGED_REMOVE_LABEL: &str = "Remove attachment";

pub struct Composer {
    state: Entity<AppState>,
    pub input: Entity<ComposerInput>,
    /// F11: mic button, voice mode, spoken replies.
    voice: Entity<Voice>,
    attachments: Vec<StagedAttachment>,
    /// Which chat the input text belongs to; drafts are kept per chat.
    draft_chat: Option<crate::model::Id>,
    drafts: std::collections::HashMap<crate::model::Id, (String, Vec<MentionToken>)>,
    /// The model / effort menu, opened from the sidebar row's control-click menu.
    /// S8 shake when the user hits Cmd+W on the pinned tab; also reused for
    /// the empty-send nudge.
    pub nudge_at: Option<u64>,
    /// Highlighted row in the `@` mention menu (arrows / hover move it;
    /// Tab and Enter insert it).
    mention_selected: usize,
    /// Last `@token` query; when it changes, selection resets to the first row.
    mention_query_seen: Option<String>,
    /// The `@token` Esc closed the menu for; the menu stays closed until the
    /// token changes (#1596, behaviour 3).
    mention_dismissed: Option<String>,
    /// Rig: `ARBOS_COMPOSER_DRAFT=…` is typed into the first open chat's
    /// field once the mention table is known (`@s` leaves the picker open,
    /// `@sam deck ready?` shows the resolved chip).
    rig_draft: Option<String>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    pub fn new(state: Entity<AppState>, voice: Entity<Voice>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(ComposerInput::new);
        let weak = cx.weak_entity();
        let weak_next = weak.clone();
        let weak_prev = weak.clone();
        let weak_accept = weak.clone();
        let weak_escape = weak.clone();
        let weak_paste = weak.clone();
        input.update(cx, |input, _| {
            input.on_submit = Some(Rc::new(move |window, cx| {
                let _ = weak.update(cx, |composer, cx| composer.submit(window, cx));
            }));
            input.on_paste_media = Some(Rc::new(move |files, images, cx| {
                let _ = weak_paste.update(cx, |composer, cx| {
                    composer.paste_media(files, images, cx);
                });
            }));
            input.on_mention_next = Some(Rc::new(move |cx| {
                weak_next
                    .update(cx, |composer, cx| composer.mention_move(1, cx))
                    .unwrap_or(false)
            }));
            input.on_mention_prev = Some(Rc::new(move |cx| {
                weak_prev
                    .update(cx, |composer, cx| composer.mention_move(-1, cx))
                    .unwrap_or(false)
            }));
            input.on_mention_accept = Some(Rc::new(move |cx| {
                weak_accept
                    .update(cx, |composer, cx| composer.accept_selected_mention(cx))
                    .unwrap_or(false)
            }));
            input.on_mention_escape = Some(Rc::new(move |cx| {
                weak_escape
                    .update(cx, |composer, cx| composer.dismiss_mentions(cx))
                    .unwrap_or(false)
            }));
        });
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        cx.observe(&voice, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&voice, |this, _, event: &VoiceEvent, cx| match event {
            VoiceEvent::Transcript(text) => {
                // The memo's words land in the input; Enter confirms (§F11).
                let text = text.clone();
                this.input.update(cx, |input, cx| {
                    if input.text().is_empty() {
                        input.set_text(text, cx);
                    } else {
                        let sep = if input.text().ends_with(char::is_whitespace) {
                            ""
                        } else {
                            " "
                        };
                        input.append_text(&format!("{sep}{text}"), cx);
                    }
                });
                cx.notify();
            }
        })
        .detach();
        // Rig: `ARBOS_COMPOSER_ATTACH=/a.png:/b.pdf` stages files at launch,
        // as the "+" picker or a drop would (screenshots of the pending row).
        let attachments = std::env::var("ARBOS_COMPOSER_ATTACH")
            .ok()
            .map(|v| {
                v.split(':')
                    .map(PathBuf::from)
                    .filter(|p| p.is_file())
                    .map(StagedAttachment::new)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            state,
            input,
            voice,
            attachments,
            draft_chat: None,
            drafts: std::collections::HashMap::new(),
            nudge_at: None,
            mention_selected: 0,
            mention_query_seen: None,
            mention_dismissed: None,
            rig_draft: std::env::var("ARBOS_COMPOSER_DRAFT")
                .ok()
                .filter(|v| !v.is_empty()),
        }
    }

    /// Swap the draft when the active chat changes.
    fn sync_draft(&mut self, cx: &mut Context<Self>) {
        let active = self.state.read(cx).active_chat_id();
        if active == self.draft_chat {
            return;
        }
        let text = self.input.read(cx).text().to_string();
        let tokens = self.input.read(cx).tokens().to_vec();
        if let Some(prev) = self.draft_chat.take() {
            if text.is_empty() {
                self.drafts.remove(&prev);
            } else {
                self.drafts.insert(prev, (text, tokens));
            }
        }
        let (next, tokens) = active
            .as_ref()
            .and_then(|id| self.drafts.get(id).cloned())
            .unwrap_or_default();
        self.input
            .update(cx, |input, cx| input.set_text_with_tokens(next, tokens, cx));
        self.draft_chat = active;
        self.mention_dismissed = None;
    }

    pub fn focus(this: &Entity<Self>, window: &mut Window, cx: &mut App) {
        let handle = this.read(cx).input.read(cx).focus_handle.clone();
        window.focus(&handle, cx);
    }

    fn mode(&self, cx: &App) -> ComposerMode {
        let state = self.state.read(cx);
        state
            .active_chat_id()
            .map(|id| state.composer_mode(&id))
            .unwrap_or(ComposerMode::Send)
    }

    fn submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        match self.mode(cx) {
            // S18: typing while a card is pending still sends (the agent multitasks); the
            // yellow frame and placeholder only point at the card.
            ComposerMode::Send | ComposerMode::Answer | ComposerMode::Approve => {
                // `@handle` + Enter resolves like `@handle` + space (behaviour 5).
                self.input
                    .update(cx, |input, cx| input.resolve_exact_handles(cx));
                let text = self.input.read(cx).sent_text().trim().to_string();
                let mentions = self.input.read(cx).token_ids();
                if text.is_empty() && self.attachments.is_empty() {
                    self.nudge_at = Some(crate::model::now_ms());
                    cx.notify();
                    return;
                }
                let running = self
                    .state
                    .read(cx)
                    .active_chat_id()
                    .map(|id| self.state.read(cx).chat_is_streaming(&id))
                    .unwrap_or(false);
                let attachments: Vec<PathBuf> = std::mem::take(&mut self.attachments)
                    .into_iter()
                    .map(|a| a.path)
                    .collect();
                self.input.update(cx, |input, cx| input.clear(cx));
                let reply_to = self.state.read(cx).active_chat_id().and_then(|cid| {
                    self.state
                        .read(cx)
                        .chat_ui
                        .get(&cid)
                        .and_then(|ui| ui.reply_to.clone())
                });
                // Mid-task + typed text → Guide (steer into the live turn). Attachments still
                // queue as a normal send so the box can fetch them for the next turn.
                if running && !text.is_empty() && attachments.is_empty() {
                    cx.emit(ComposerEvent::Guide {
                        text,
                        reply_to,
                        mentions,
                    });
                } else {
                    cx.emit(ComposerEvent::Send {
                        text,
                        attachments,
                        reply_to,
                        mentions,
                    });
                }
            }
        }
        cx.notify();
    }

    fn attach(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update(cx, |composer, cx| {
                    composer.add_paths(paths, cx);
                });
            }
        })
        .detach();
    }

    /// The card's top strip while files are pending: 64 pt image tiles and
    /// file chips (icon tile · name · size) in one horizontal row that scrolls
    /// sideways when it overflows; each carries its always-visible remove
    /// disc ([feedback composer-attachment-removal-2026-10-27]).
    fn staged_row(&self, theme: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut strip = div()
            .id("staged-strip")
            .debug_selector(|| "composer-staged-strip".into())
            .flex()
            .flex_row()
            .items_start()
            .gap(px(STAGED_GAP))
            .overflow_x_scroll()
            .px(px(12.0))
            .pt(px(12.0));
        for (i, staged) in self.attachments.iter().enumerate() {
            strip = strip.child(self.staged_tile(i, staged, theme, cx));
        }
        strip
    }

    /// One staged file: a single rounded rect (`STAGED_RADIUS`, the theme's
    /// hairline) — the square thumbnail for a picture, the icon · name · size
    /// chip for anything else — with the remove disc inside its top-right
    /// corner. The disc is always drawn (trackpad and keyboard users, not
    /// hover-only), `STAGED_REMOVE` across, inset so it stays inside the
    /// tile: no overlap with the neighbour, nothing to clip.
    fn staged_tile(
        &self,
        i: usize,
        staged: &StagedAttachment,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let remove = div()
            .id(ElementId::NamedInteger("staged-remove".into(), i as u64))
            .debug_selector(move || format!("composer-remove-attachment-{i}"))
            .absolute()
            .top(px(STAGED_REMOVE_INSET))
            .right(px(STAGED_REMOVE_INSET))
            .size(px(STAGED_REMOVE))
            .rounded_full()
            .bg(Theme::with_alpha(gpui::black(), 0.62))
            .border_1()
            .border_color(Theme::with_alpha(gpui::white(), 0.4))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            // A scrim disc over a picture: black washes, not the text-wash token
            // (which would vanish on a dark thumbnail).
            .hover(|s| s.bg(Theme::with_alpha(gpui::black(), 0.85))) // hover-fill: exception overlay
            .role(AriaRole::Button)
            .aria_label(STAGED_REMOVE_LABEL)
            .tooltip(|_, cx| cx.new(|_| TextTip(STAGED_REMOVE_LABEL.into())).into())
            .child(icon("xmark", 9.0, gpui::white()))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.remove_attachment(i, cx);
            }));
        let tile = div()
            .id(ElementId::NamedInteger("staged".into(), i as u64))
            .debug_selector(move || format!("composer-staged-tile-{i}"))
            .relative()
            .flex_shrink_0()
            .h(px(STAGED_TILE))
            .rounded(px(STAGED_RADIUS))
            .bg(theme.surface_2);
        // The hairline is an overlay ring on both kinds (not a box border): the
        // picture never paints over it, and the remove disc's inset measures
        // from the tile's outer edge on every tile.
        let ring = div()
            .absolute()
            .inset_0()
            .rounded(px(STAGED_RADIUS))
            .border_1()
            .border_color(theme.border_strong);
        let tile = match staged.preview.clone() {
            // The thumbnail is decoded small and covers the square tile; the
            // renderer crops Cover through the atlas tile, so the corners are
            // the tile's own and nothing spills past it.
            Some(image) => tile.w(px(STAGED_TILE)).child(
                img(image)
                    .size(px(STAGED_TILE))
                    .rounded(px(STAGED_RADIUS))
                    .object_fit(ObjectFit::Cover),
            ),
            None => tile
                .flex()
                .items_center()
                .gap(px(10.0))
                .pl(px(10.0))
                // Room for the remove disc at the top right.
                .pr(px(STAGED_REMOVE + 2.0 * STAGED_REMOVE_INSET))
                .max_w(px(STAGED_CHIP_MAX_WIDTH))
                .child(
                    div()
                        .size(px(40.0))
                        .flex_shrink_0()
                        .rounded(px(8.0))
                        .bg(theme.surface_1)
                        .border_1()
                        .border_color(theme.border)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(
                            if is_image_path(&staged.path) {
                                "photo"
                            } else {
                                "doc"
                            },
                            16.0,
                            theme.text_muted,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(TEXT_BODY))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .truncate()
                                .child(staged.file_name()),
                        )
                        .child(
                            div()
                                .text_size(px(TEXT_TINY))
                                .text_color(theme.text_muted)
                                .whitespace_nowrap()
                                .child(staged.size_label()),
                        ),
                ),
        };
        tile.child(ring).child(remove)
    }

    /// Drop staged file `i`; the others and the typed text stay.
    fn remove_attachment(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.attachments.len() {
            self.attachments.remove(i);
            cx.notify();
        }
    }

    /// ⌫ / ⌦ reaching the composer from a field with nothing to delete: with
    /// an empty draft the last staged attachment goes (the strip's own key
    /// rule; the text field never sees a change). Returns whether one went.
    fn delete_key_on_strip(&mut self, cx: &mut Context<Self>) -> bool {
        if self.attachments.is_empty() || !self.input.read(cx).text().is_empty() {
            return false;
        }
        self.attachments.pop();
        cx.notify();
        true
    }

    #[cfg(test)]
    fn staged_paths(&self) -> Vec<PathBuf> {
        self.attachments.iter().map(|a| a.path.clone()).collect()
    }

    /// ⌘V with files (Finder copy) or an image (screenshot, copied picture):
    /// staged exactly like a drop — files as attachment chips, images as
    /// tiles. Directories are skipped like everywhere else.
    pub(crate) fn paste_media(
        &mut self,
        files: Vec<PathBuf>,
        images: Vec<gpui::Image>,
        cx: &mut Context<Self>,
    ) {
        let dir = pasted_images_dir();
        let stashed = images.iter().filter_map(|image| {
            stash_pasted_image(image, &dir)
                .map_err(|e| log::warn!("pasted image not saved: {e}"))
                .ok()
        });
        self.add_paths(files.into_iter().chain(stashed), cx);
    }

    /// Paths from the "+" picker, a drop onto the chat (pane or composer), or
    /// a ⌘V.
    pub(crate) fn add_paths(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
        cx: &mut Context<Self>,
    ) {
        for path in paths {
            if path.is_file() {
                self.attachments.push(StagedAttachment::new(path));
            }
        }
        cx.notify();
    }

    /// The `@token` being typed at the end of the input, if any.
    fn mention_query(&self, cx: &App) -> Option<String> {
        self.input
            .read(cx)
            .open_mention_token()
            .map(str::to_lowercase)
    }

    /// Who the `@token` can mean here (#1596, behaviours 1–2): people and
    /// agents of the current space, plus `@here` (everyone in this chat) and
    /// `@agents` (all agents here) for any member of a group or people chat
    /// (Jacob, 2026-10-24); a server channel keeps the owner-only collective
    /// pings (the client has no member list for a server yet). The shared
    /// matcher on the @handle and the display name (`arbos_core::search`,
    /// [feedback shared-search-matcher-mac-2026-10-26]): exact > prefix >
    /// word-start, name before handle, then alphabetical by handle; at most
    /// six rows. A handle an agent and a person both carry (#1538) tags each
    /// of those two rows.
    fn mention_candidates(&self, cx: &App) -> Vec<MentionCandidate> {
        let Some(query) = self.mention_query(cx) else {
            return Vec::new();
        };
        if self.mention_dismissed.as_deref() == Some(query.as_str()) {
            return Vec::new();
        }
        let theme = Theme::of(cx).clone();
        let state = self.state.read(cx);
        let me_handle = state.me_handle();
        let mut out: Vec<MentionCandidate> = Vec::new();
        // The chip reads `here` / `agents` (as the bubble chips do); the row
        // says `@here · everyone in this chat`.
        let collective =
            |handle: &str, description: &'static str, is_agent: bool| MentionCandidate {
                handle: handle.into(),
                name: handle.into(),
                id: Some(handle.into()),
                is_agent,
                picture: mention::initials_picture(handle, handle, None, &theme),
                tag: None,
                description: Some(description),
            };
        let collectives = |out: &mut Vec<MentionCandidate>| {
            if "here".starts_with(&query) {
                out.push(collective("here", "everyone in this chat", false));
            }
            if "agents".starts_with(&query) {
                out.push(collective("agents", "all agents here", true));
            }
        };

        if let Some((server, _)) = state.selected_channel_info() {
            if server.is_owner() {
                collectives(&mut out);
            }
            return out;
        }

        let Some(space) = state.selected() else {
            return Vec::new();
        };
        if !space.is_people() && space.people.is_empty() {
            return Vec::new();
        }
        collectives(&mut out);
        let mut members: Vec<(MentionCandidate, search::Match)> = Vec::new();
        for a in &space.agents {
            let Some(matched) = mention_match(&query, &a.handle, &a.name) else {
                continue;
            };
            members.push((
                MentionCandidate {
                    handle: a.handle.clone(),
                    name: a.name.trim().to_string(),
                    id: Some(format!("agent:{}", a.project_id)),
                    is_agent: true,
                    picture: mention::agent_member_picture(state, Some(space), a, &theme),
                    tag: None,
                    description: None,
                },
                matched,
            ));
        }
        for p in &space.people {
            if p.handle.eq_ignore_ascii_case(&me_handle) {
                continue;
            }
            let Some(matched) = mention_match(&query, &p.handle, &p.display_name()) else {
                continue;
            };
            members.push((
                MentionCandidate {
                    handle: p.handle.clone(),
                    name: p.display_name(),
                    id: Some(format!("user:{}", p.id)),
                    is_agent: false,
                    picture: mention::person_picture(state, p, &theme),
                    tag: None,
                    description: None,
                },
                matched,
            ));
        }
        // The shared matcher's order (exact > prefix > word-start, name before
        // handle), then alphabetical by handle, agents first on a tie.
        members.sort_by(|(a, a_match), (b, b_match)| {
            a_match
                .cmp(b_match)
                .then_with(|| a.handle.to_lowercase().cmp(&b.handle.to_lowercase()))
                .then_with(|| b.is_agent.cmp(&a.is_agent))
        });
        let mut members: Vec<MentionCandidate> = members.into_iter().map(|(c, _)| c).collect();
        // Shared handle: say which is which.
        let handles: Vec<String> = members.iter().map(|c| c.handle.to_lowercase()).collect();
        for c in &mut members {
            let h = c.handle.to_lowercase();
            if handles.iter().filter(|x| **x == h).count() > 1 || h == me_handle.to_lowercase() {
                c.tag = Some(if c.is_agent { "agent" } else { "person" });
            }
        }
        out.extend(members);
        if space.agents.len() == 1
            && "arbos".starts_with(&query)
            && !out.iter().any(|c| c.handle == "arbos")
        {
            let a = &space.agents[0];
            out.push(MentionCandidate {
                handle: "arbos".into(),
                name: a.name.trim().to_string(),
                id: Some(format!("agent:{}", a.project_id)),
                is_agent: true,
                picture: mention::agent_member_picture(state, Some(space), a, &theme),
                tag: Some("the agent in this chat"),
                description: None,
            });
        }
        out.truncate(MENTION_ROWS_MAX);
        out
    }

    /// Keep `mention_selected` in range for the current `@token` filter.
    fn sync_mention_selection(&mut self, candidates_len: usize, query: Option<&str>) {
        if self.mention_query_seen.as_deref() != query {
            self.mention_query_seen = query.map(str::to_string);
            self.mention_selected = 0;
        }
        if candidates_len == 0 {
            self.mention_selected = 0;
        } else if self.mention_selected >= candidates_len {
            self.mention_selected = candidates_len - 1;
        }
    }

    /// ↓ or ↑ / Shift+Tab through the `@` list. Returns true when the menu
    /// was open and handled the key.
    fn mention_move(&mut self, delta: i32, cx: &mut Context<Self>) -> bool {
        let query = self.mention_query(cx);
        let candidates = self.mention_candidates(cx);
        if candidates.is_empty() {
            self.sync_mention_selection(0, None);
            return false;
        }
        self.sync_mention_selection(candidates.len(), query.as_deref());
        let len = candidates.len() as i32;
        let next = (self.mention_selected as i32 + delta).rem_euclid(len) as usize;
        self.mention_selected = next;
        cx.notify();
        true
    }

    /// Enter accepts the highlighted `@` row. Returns true when it did.
    fn accept_selected_mention(&mut self, cx: &mut Context<Self>) -> bool {
        let query = self.mention_query(cx);
        let candidates = self.mention_candidates(cx);
        if candidates.is_empty() {
            self.sync_mention_selection(0, None);
            return false;
        }
        self.sync_mention_selection(candidates.len(), query.as_deref());
        let Some(candidate) = candidates.get(self.mention_selected).cloned() else {
            return false;
        };
        self.commit_mention(&candidate, cx);
        true
    }

    /// Put the picked row into the field as a chip (behaviour 4).
    fn commit_mention(&mut self, candidate: &MentionCandidate, cx: &mut Context<Self>) {
        let c = candidate.clone();
        self.input.update(cx, |input, cx| {
            input.insert_mention(&c.handle, &c.name, c.id.clone(), cx)
        });
        self.mention_selected = 0;
        self.mention_query_seen = None;
        self.mention_dismissed = None;
        cx.notify();
    }

    /// Esc while the menu is open: close it for this `@token`; it comes back
    /// when the token changes. Returns true when there was a menu to close.
    fn dismiss_mentions(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(query) = self.mention_query(cx) else {
            return false;
        };
        if self.mention_dismissed.as_deref() == Some(query.as_str()) {
            return false;
        }
        if self.mention_candidates(cx).is_empty() {
            return false;
        }
        self.mention_dismissed = Some(query);
        cx.notify();
        true
    }

    /// The picker (#1596; Grok Bot's group @-mention picker, launch-trailer
    /// frame 55, in the Arbos theme): a card above the pill, one row per
    /// entity — picture · name · `@handle` — the highlighted row in
    /// `surface_2`, keycap hints at the bottom.
    fn render_mentions(
        &self,
        candidates: Vec<MentionCandidate>,
        selected: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let mut menu = div()
            .id("mention-menu")
            .flex()
            .flex_col()
            .w(px(MENTION_MENU_WIDTH))
            .p(px(MENTION_MENU_PAD))
            .rounded(px(PANEL_RADIUS))
            .bg(theme.card())
            .border_1()
            .border_color(theme.border_strong)
            .shadow_lg();
        for (i, c) in candidates.into_iter().enumerate() {
            let picked = c.clone();
            let mut row = div()
                .id(ElementId::NamedInteger("mention-item".into(), i as u64))
                .flex()
                .items_center()
                .h(px(MENTION_ROW_HEIGHT))
                .gap(px(10.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .map(|el| highlight::row(el, i == selected, highlight::Shape::Own, &theme))
                // Hover owns the highlight so Tab / Enter insert that row
                // ([feedback app-01M5WKBR85JVW1KKJH96FWZVK5]).
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered && this.mention_selected != i {
                        this.mention_selected = i;
                        cx.notify();
                    }
                }))
                .child(mention_picture(&c.picture, MENTION_ROW_PICTURE, &theme))
                .child(
                    div()
                        .text_size(px(TEXT_SMALL))
                        .text_color(theme.text)
                        .whitespace_nowrap()
                        .child(match c.description {
                            Some(_) => format!("@{}", c.handle),
                            None => c.name.clone(),
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(TEXT_SMALL))
                        .text_color(theme.text_muted)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(match c.description {
                            Some(d) => format!("· {d}"),
                            None => format!("@{}", c.handle),
                        }),
                );
            if let Some(tag) = c.tag {
                row = row.child(
                    div()
                        .text_size(px(TEXT_TINY))
                        .text_color(theme.text_faint)
                        .whitespace_nowrap()
                        .child(tag),
                );
            }
            menu = menu.child(row.on_click(cx.listener(move |this, _, _, cx| {
                this.commit_mention(&picked, cx);
            })));
        }
        menu = menu.child(
            div()
                .h(px(MENTION_MENU_FOOTER))
                .flex()
                .items_center()
                .px(px(8.0))
                .text_size(px(TEXT_TINY - 1.0))
                .text_color(theme.text_faint)
                .child("↑↓ move · Tab or ↵ insert · esc close"),
        );
        div()
            .absolute()
            .bottom(px(COMPOSER_HEIGHT + MENTION_MENU_GAP))
            .left(px(0.0))
            .child(frosted(PANEL_RADIUS, PANEL_BLUR, menu))
    }
}

/// One row of the `@` picker.
#[derive(Clone, Debug)]
pub struct MentionCandidate {
    pub handle: String,
    pub name: String,
    /// `user:<id>` / `agent:<project-id>`, or the collective marker
    /// (`here` / `agents`) the source expands into the chat's members.
    pub id: Option<String>,
    pub is_agent: bool,
    pub picture: mention::Picture,
    /// `agent` / `person` when the handle is shared (#1538), or a note.
    pub tag: Option<&'static str>,
    /// A collective's row text in place of `@handle`: `@here · everyone in
    /// this chat`, `@agents · all agents here`.
    pub description: Option<&'static str>,
}

/// Picker geometry (pt), the approved mock: a 320 pt card, 32 pt rows with a
/// 22 pt picture, 8 pt above the pill.
const MENTION_MENU_WIDTH: f32 = 320.0;
const MENTION_MENU_PAD: f32 = 6.0;
const MENTION_ROW_HEIGHT: f32 = 32.0;
const MENTION_ROW_PICTURE: f32 = 22.0;
const MENTION_MENU_FOOTER: f32 = 20.0;
const MENTION_MENU_GAP: f32 = 8.0;
const MENTION_ROWS_MAX: usize = 6;

/// Does the typed `@query` match this entity? The shared matcher's verdict on
/// the @handle and the display name (`arbos_core::search`: exact > prefix >
/// word-start, name before handle, diacritics folded), `None` when neither
/// matches. An empty query matches everyone.
fn mention_match(query: &str, handle: &str, name: &str) -> Option<search::Match> {
    search::match_entity(name, handle, None, &search::normalize_query(query))
}

/// A picker row's picture as an element: photo, D79 initials, D92 leaf or the neutral disc.
fn mention_picture(picture: &mention::Picture, size: f32, theme: &Theme) -> gpui::AnyElement {
    match picture {
        mention::Picture::Image(image) => img(crate::ui::picture_fit::fit(image, size))
            .size(px(size))
            .flex_shrink_0()
            .rounded_full()
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        mention::Picture::Initials { text, top, bottom } => div()
            .size(px(size))
            .flex_shrink_0()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::linear_gradient(
                180.0,
                gpui::linear_color_stop(*top, 0.0),
                gpui::linear_color_stop(*bottom, 1.0),
            ))
            .text_size(px(size * crate::ui::avatars::INITIALS_SCALE))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(gpui::white())
            .child(text.clone())
            .into_any_element(),
        mention::Picture::Leaf(pick) => {
            crate::ui::avatars::leaf_element(*pick, size).into_any_element()
        }
        mention::Picture::Neutral { name } => {
            crate::ui::avatars::neutral_element(name, size, theme).into_any_element()
        }
    }
}

impl Render for Composer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_draft(cx);
        let theme = Theme::of(cx).clone();
        // Keep the field's known `@handle`s in step with the app (incl. `@here`).
        let mentions = Mentions::from_state(&self.state, &theme, cx);
        self.input
            .update(cx, |input, _| input.set_mentions(mentions));
        if self.rig_draft.is_some() && self.draft_chat.is_some() {
            let rig = self.rig_draft.take().unwrap_or_default();
            self.input.update(cx, |input, cx| {
                input.set_text(rig, cx);
                input.resolve_exact_handles(cx);
            });
        }
        let mode = self.mode(cx);
        let state = self.state.read(cx);
        let space = state.selected().cloned();
        let voice_mode = state.voice_mode;
        let running = state
            .active_chat_id()
            .map(|id| state.chat_is_streaming(&id))
            .unwrap_or(false);
        let name = space
            .as_ref()
            .map(|p| {
                if p.is_people_group() {
                    p.name.clone()
                } else if p.is_people() {
                    // "Message Kenobi", like the pill above (topbar-unify mock c).
                    p.person()
                        .map(|x| x.display_name())
                        .unwrap_or_else(|| p.name.clone())
                } else {
                    p.name.clone()
                }
            })
            .or_else(|| {
                state
                    .selected_channel_info()
                    .map(|(_, channel)| format!("#{}", channel.name))
            })
            .unwrap_or_else(|| "Arbos".into());
        let (recording, transcribing, in_call, voice_notice) = {
            let v = self.voice.read(cx);
            // A note about a reply shows only in that reply's chat.
            let active_chat = state.active_chat_id();
            (
                v.is_recording(),
                v.is_transcribing(),
                v.in_call(),
                v.notice_for(active_chat.as_deref()).map(str::to_owned),
            )
        };
        // The composer's note: the last send / attachment / recording failure in this chat
        // (the shared error element, directly above the field — Jacob 2026-10-26). A push
        // refusal is chat-level and lives in the slot under the header, not here.
        let send_error = state
            .active_chat_id()
            .and_then(|id| state.send_error(&id).cloned().map(|slot| (id, slot)));
        let in_livekit = state.my_call.is_some();
        let has_text = !self.input.read(cx).text().trim().is_empty();
        // In a job tab (`jobtabs`) typing steers that job.
        let job_tab = state
            .active_chat_id()
            .is_some_and(|id| crate::jobtabs::is_job_chat(&id));

        self.input.update(cx, |input, _| {
            input.set_placeholder(match mode {
                ComposerMode::Answer => format!("Answer {name}"),
                ComposerMode::Approve => "Approve or reject the card above".to_string(),
                ComposerMode::Send if transcribing => "Transcribing…".to_string(),
                ComposerMode::Send if voice_mode => "Speak or type…".to_string(),
                ComposerMode::Send if job_tab => "Steer this task".to_string(),
                // Grok Mac says "Message <bot>" (the iPhone says "Ask <bot>").
                ComposerMode::Send => format!("Message {name}"),
            });
        });

        let nudge = self
            .nudge_at
            .map(|t| crate::model::now_ms().saturating_sub(t) < 300)
            .unwrap_or(false);
        if nudge {
            crate::ui::motion::PulseClock::request(cx);
        }

        let border_color = match mode {
            _ if recording => Theme::with_alpha(theme.danger, 0.5),
            ComposerMode::Answer | ComposerMode::Approve => Theme::with_alpha(theme.warn, 0.7),
            ComposerMode::Send => {
                if nudge {
                    theme.danger
                } else {
                    theme.field_border
                }
            }
        };

        // The round primary button: a red Stop while a memo records (stops the
        // mic and transcribes what was said); Guide while a turn is running and
        // the field has text; otherwise send, stop when empty mid-turn, or
        // start / end a call when idle and empty (iPhone: phone next to the
        // mic; calls start audio-only).
        let hanging_up = in_call || in_livekit;
        let guide = running && has_text && self.attachments.is_empty();
        let action = primary_action(has_text, running, recording, guide);
        let glyph = action.glyph();
        let red = action == ButtonAction::StopMemo || (action == ButtonAction::Call && hanging_up);
        let primary_button = if action == ButtonAction::Guide {
            div()
                .id("composer-guide")
                .h(px(SEND_BUTTON))
                .px(px(14.0))
                .flex_shrink_0()
                .rounded_full()
                .bg(theme.primary)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.opacity(0.88))
                .child(
                    div()
                        .text_size(px(TEXT_TINY))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.on_primary)
                        .child("Guide"),
                )
                .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
        } else {
            div()
                .id("composer-send")
                .size(px(SEND_BUTTON))
                .flex_shrink_0()
                .rounded_full()
                .bg(if red { theme.danger } else { theme.primary })
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.opacity(0.88))
                .child(icon(
                    glyph,
                    if glyph == "stop" { 14.0 } else { 16.0 },
                    if action == ButtonAction::StopMemo {
                        gpui::white()
                    } else if red {
                        theme.text
                    } else {
                        theme.on_primary
                    },
                ))
                .on_click(cx.listener(move |this, _, window, cx| match action {
                    ButtonAction::Send | ButtonAction::Guide => this.submit(window, cx),
                    ButtonAction::Stop => cx.emit(ComposerEvent::Stop),
                    ButtonAction::StopMemo => this.voice.update(cx, |v, cx| v.stop_memo(cx)),
                    ButtonAction::Call => {
                        let live = this.state.read(cx).my_call.is_some();
                        let voice_on = this.voice.read(cx).in_call();
                        if live {
                            this.state.update(cx, |s, cx| s.leave_call(cx));
                        } else if voice_on {
                            this.voice.update(cx, |v, cx| v.toggle_call(cx));
                        } else if this
                            .state
                            .read(cx)
                            .selected()
                            .is_some_and(callview::callable)
                        {
                            // Audio-only; camera is toggled on the call screen.
                            this.state
                                .update(cx, |s, cx| s.start_call_in_chat(CallKind::Voice, cx));
                        }
                    }
                }))
        };

        // Hold to record a voice memo, or click to toggle recording. Idle it is
        // Grok's two-tone pair: grey mic next to the white phone / send
        // ([feedback app-01M5NXFCS5N79WCN2C8PM6GTKE]); while recording it stays
        // mounted, tinted red like the iPhone, so a release (hold-to-talk) or a
        // second click also finishes the memo.
        let mic_button = div()
            .id("composer-mic")
            .size(px(SEND_BUTTON))
            .flex_shrink_0()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(if recording {
                theme.danger_soft
            } else {
                theme.surface_2
            })
            .hover(|s| s.opacity(0.88))
            .child(icon(
                "mic",
                16.0,
                if recording {
                    theme.danger
                } else if transcribing {
                    theme.text_faint
                } else {
                    theme.text_muted
                },
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.voice.update(cx, |v, cx| v.mic_pressed(cx));
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.voice.update(cx, |v, cx| v.mic_released(cx));
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.voice.update(cx, |v, cx| v.mic_released(cx));
                }),
            );

        let plus = div()
            .id("composer-attach")
            .size(px(PLUS_BUTTON))
            .flex_shrink_0()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .map(|el| highlight::washed(el, theme.surface_2, &theme))
            .child(icon("plus", 16.0, theme.text_muted))
            .on_click(cx.listener(|this, _, _, cx| this.attach(cx)));

        let field_body = if recording {
            // iPhone recorder row: ✕ (discard) · red dot · timer · level bars.
            let discard = div()
                .id("composer-memo-cancel")
                .size(px(22.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .map(|el| highlight::button(el, &theme))
                .child(icon("xmark", 11.0, theme.text_muted))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.voice.update(cx, |v, cx| v.cancel_memo(cx));
                }));
            div()
                .flex_1()
                .min_w_0()
                .h(px(COMPOSER_HEIGHT - 2.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .pl(px(6.0))
                .child(discard)
                .child(render_recording_row(self.voice.read(cx), &theme))
        } else {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .pl(px(10.0))
                // In the card the row above and the buttons below set the rhythm.
                .py(px(if self.attachments.is_empty() {
                    (COMPOSER_HEIGHT - 2.0 - INPUT_LINE_HEIGHT) / 2.0
                } else {
                    0.0
                }))
                .child(self.input.clone())
        };

        // Match the iPhone: mic (transcribe) + phone (call) while the field is
        // idle and empty; send / stop / hang-up replace the phone as needed.
        let staged = !self.attachments.is_empty();
        let mut trailing = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            // In the pill the row is the pill's height; in the card the
            // circles set it and the card's padding does the rest.
            .h(px(if staged {
                SEND_BUTTON
            } else {
                COMPOSER_HEIGHT - 2.0
            }))
            .flex_shrink_0();
        if mic_mounted(has_text, recording) {
            trailing = trailing.child(mic_button);
        }
        // Always show the round button: phone when idle (like the iPhone), else
        // Stop (memo) / send / stop (turn) / hang-up.
        trailing = trailing.child(primary_button);
        let field_radius = if staged { CARD_RADIUS } else { COMPOSER_RADIUS };
        // A file dragged over the chat lights up the whole pane
        // (`shell::file_drop_target`), not this field alone.
        let field_base = div()
            .id("composer-field")
            .debug_selector(|| "composer-field".into())
            .flex_1()
            .min_w_0()
            .rounded(px(field_radius))
            .bg(theme.field)
            .border_1()
            .border_color(border_color);
        let field = if staged {
            // Redesign mock e: with files pending the pill opens into a card —
            // the tiles above the text, "+" bottom-left, mic / send bottom-right.
            field_base
                .flex()
                .flex_col()
                .child(self.staged_row(&theme, cx))
                // The body keeps the pill's own vertical centring padding; the
                // row above / the buttons below supply the rest (mock e: ≈ 13 pt
                // tile → text, text → buttons).
                .child(
                    div()
                        .flex()
                        .items_center()
                        .px(px(4.0))
                        .pt(px(12.0))
                        .child(field_body),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(12.0))
                        .pt(px(8.0))
                        .pb(px(12.0))
                        .child(plus)
                        .child(trailing),
                )
        } else {
            field_base
                .min_h(px(COMPOSER_HEIGHT))
                .flex()
                .items_end()
                .gap(px(4.0))
                .pl(px((COMPOSER_HEIGHT - 2.0 - PLUS_BUTTON) / 2.0))
                .pr(px((COMPOSER_HEIGHT - 2.0 - SEND_BUTTON) / 2.0))
                .child(
                    div()
                        .h(px(COMPOSER_HEIGHT - 2.0))
                        .flex()
                        .items_center()
                        .flex_shrink_0()
                        .child(plus),
                )
                .child(field_body)
                .child(trailing)
        };

        // Drop acceptance lives on the chat pane (`shell::file_drop_target`) so
        // a file hovered anywhere over the conversation — not only this field —
        // stages into the composer.
        let mut column = div()
            .id("composer")
            .relative()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            // The strip's key rule: ⌫ / ⌦ the field had nothing to delete for
            // (it propagates them) remove the last staged attachment while the
            // draft is empty. Handled here, on the attachments' owner, not in
            // the field's key routing.
            .on_action(cx.listener(|this, _: &Backspace, _, cx| {
                this.delete_key_on_strip(cx);
            }))
            .on_action(cx.listener(|this, _: &Delete, _, cx| {
                this.delete_key_on_strip(cx);
            }));

        // Reply preview (iPhone parity): set by the hover reply icon / the
        // bubble menu on a message. Always one fixed-height row, whatever
        // the quoted message's length ([feedback reply-preview-fixed-height]).
        if let Some(chat_id) = self.state.read(cx).active_chat_id() {
            let (reply_id, reply_ref) = {
                let state = self.state.read(cx);
                let ui = state.chat_ui.get(&chat_id);
                (
                    ui.and_then(|ui| ui.reply_to.clone()),
                    ui.and_then(|ui| ui.reply_ref.clone()),
                )
            };
            // Reply in DM arms a cross-chat reply: "Replying to Jacob in
            // Arbonites" + the snapshot the row will carry (#1835).
            let preview = match (reply_id, reply_ref) {
                (_, Some(r)) => Some(ReplyPreview::for_reply_ref(&r)),
                (Some(reply_id), None) => {
                    let state = self.state.read(cx);
                    Some(
                        state
                            .messages
                            .get(&chat_id)
                            .and_then(|list| list.iter().find(|m| m.id == reply_id))
                            .map(|m| ReplyPreview::for_message(m, &state.thumbnails))
                            .unwrap_or_else(ReplyPreview::missing),
                    )
                }
                (None, None) => None,
            };
            if let Some(preview) = preview {
                let clear_chat = chat_id.clone();
                let clear = div()
                    .id("composer-reply-clear")
                    .size(px(18.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .map(|el| highlight::button(el, &theme))
                    .child(icon("xmark", 10.0, theme.text_muted))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state
                            .update(cx, |s, cx| s.clear_reply_to(&clear_chat, cx));
                    }));
                column = column.child(reply_preview_bar(&preview, &theme, clear));
            }
        }

        // One muted line about the voice path while it is doing something (or
        // when it had to fall back: no microphone, mock relay). The link has
        // no line here: the header's connection pill is its one place
        // ([feedback connection-pill]). Overlay just above the field so the pill does
        // not jump when the note appears (a flow child above or below would
        // shift the bottom-pinned composer).
        let note = voice_notice.filter(|_| recording || transcribing || voice_mode);
        if let Some((chat, slot)) = send_error {
            let chat_for_retry = chat.clone();
            column = column.child(error_note::render(
                "composer-error",
                &slot.error,
                slot.details_open,
                error_note::Style::Inline,
                &theme,
                error_note::Handlers::new()
                    .try_again(cx.listener(move |this, _, _, cx| {
                        let chat = chat_for_retry.clone();
                        this.state.update(cx, |s, cx| s.retry_send_error(chat, cx));
                    }))
                    .toggle_details(cx.listener(move |this, _, _, cx| {
                        let chat = chat.clone();
                        this.state.update(cx, |s, cx| {
                            s.toggle_send_error_details(&chat);
                            cx.notify();
                        });
                    })),
            ));
        }
        let mut field_wrap = div().relative().w_full();
        if let Some(note) = note {
            field_wrap = field_wrap.child(
                div()
                    .absolute()
                    .left(px(16.0))
                    .right(px(16.0))
                    .top(px(-(TEXT_TINY + 6.0)))
                    .text_size(px(TEXT_TINY))
                    .text_color(theme.text_muted)
                    .child(note),
            );
        }
        column = column.child(field_wrap.child(frosted(field_radius, PANEL_BLUR, field)));

        let query = self.mention_query(cx);
        if query.is_none() {
            self.mention_dismissed = None;
        }
        let mentions = self.mention_candidates(cx);
        if mentions.is_empty() {
            self.sync_mention_selection(0, None);
        } else {
            self.sync_mention_selection(mentions.len(), query.as_deref());
            let selected = self.mention_selected;
            column = column.child(glass::popover(self.render_mentions(mentions, selected, cx)));
        }
        column
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ButtonAction {
    Send,
    /// Send as guidance while a turn is running ("Guide" pill).
    Guide,
    /// Stop the running turn.
    Stop,
    /// Stop the recording memo and transcribe it.
    StopMemo,
    Call,
}

impl ButtonAction {
    fn glyph(self) -> &'static str {
        match self {
            ButtonAction::Send => "arrow-up",
            ButtonAction::Guide => "",
            ButtonAction::Stop | ButtonAction::StopMemo => "stop",
            ButtonAction::Call => "phone",
        }
    }
}

fn is_image_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "heic")
    )
}

/// Decode a small thumbnail for a staged picture (local file only): the tile
/// at 2 px/pt plus the room `ObjectFit::Cover` may crop, never the full
/// pasted screenshot.
fn image_preview_for(path: &Path) -> Option<Arc<RenderImage>> {
    if !is_image_path(path) {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    decode_thumbnail(&bytes, STAGED_THUMBNAIL_PX).map(Arc::new)
}

/// Longest side of a staged thumbnail's bitmap: 4 × the 64 pt tile covers a
/// 2 px/pt display with a 2:1 crop to spare.
pub(crate) const STAGED_THUMBNAIL_PX: u32 = 4 * STAGED_TILE as u32;

/// The remove disc's hover tooltip: one line in the theme's small type.
struct TextTip(SharedString);

impl Render for TextTip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .bg(theme.surface_2)
            .border_1()
            .border_color(theme.border)
            .text_size(px(TEXT_TINY))
            .text_color(theme.text)
            .child(self.0.clone())
    }
}

/// What the round primary button does. A recording memo wins: the user needs
/// a visible way to stop the microphone before anything else.
fn primary_action(has_text: bool, running: bool, recording: bool, guide: bool) -> ButtonAction {
    if recording {
        ButtonAction::StopMemo
    } else if guide {
        ButtonAction::Guide
    } else if has_text {
        ButtonAction::Send
    } else if running {
        ButtonAction::Stop
    } else {
        ButtonAction::Call
    }
}

/// The mic shows while the field is empty and stays mounted for the whole
/// recording (a hold-to-talk release lands on it).
fn mic_mounted(has_text: bool, recording: bool) -> bool {
    recording || !has_text
}

/// The reply preview above the field is always this tall (Jacob, Oct 23:
/// "reply boxes should always be the same size, not fitted to the message"):
/// one row — glyph, "Replying to @handle", one truncated line of the quoted
/// message, × — whatever the original's length. 16 pt line + 2 × 8 pt.
pub const REPLY_PREVIEW_HEIGHT: f32 = 32.0;
/// The quoted line is pre-cut to this many characters; the row's own ellipsis
/// trims it to the width that is left after the label.
pub const REPLY_PREVIEW_CHARS: usize = 160;
/// A quoted picture shows as a square thumbnail this big, inside the row.
pub const REPLY_PREVIEW_THUMB: f32 = 20.0;

/// What the composer's reply preview shows for the quoted message.
#[derive(Clone)]
pub struct ReplyPreview {
    /// "@handle", or "agent" / "you" / "message" when the row has no handle.
    pub who: String,
    /// The quoted message on one line (newlines and markdown flattened,
    /// cut on a word boundary with an ellipsis); its file name when it has
    /// no prose.
    pub line: String,
    /// The quoted picture's thumbnail when the message carries one that is
    /// already decoded for the transcript.
    pub thumb: Option<Arc<RenderImage>>,
}

impl ReplyPreview {
    pub fn for_message(message: &Message, thumbnails: &HashMap<String, Arc<RenderImage>>) -> Self {
        let who = message
            .sender_handle
            .as_deref()
            .map(|h| format!("@{h}"))
            .unwrap_or_else(|| match message.role {
                Role::Assistant => "agent".into(),
                Role::User => "you".into(),
                Role::System | Role::Project => "message".into(),
            });
        let thumb = message.parts.iter().find_map(|p| match p {
            Part::Attachment { attachment, .. } if attachment.kind() == AttachmentKind::Image => {
                attachment
                    .blob_ref
                    .as_deref()
                    .and_then(|k| thumbnails.get(k).cloned())
            }
            _ => None,
        });
        Self {
            who,
            line: transcript::message_line(message, REPLY_PREVIEW_CHARS),
            thumb,
        }
    }

    /// A cross-chat reply (Reply in DM): "Jacob in Arbonites" + the snapshot.
    pub fn for_reply_ref(r: &crate::model::ReplyRef) -> Self {
        let place = r.chat_title.trim();
        let who = if place.is_empty() {
            r.who()
        } else {
            format!("{} in {place}", r.who())
        };
        Self {
            who,
            line: transcript::first_words(&transcript::one_line(&r.text), REPLY_PREVIEW_CHARS),
            thumb: None,
        }
    }

    /// The quoted message is not loaded (older than the snapshot).
    pub fn missing() -> Self {
        Self {
            who: "message".into(),
            line: String::new(),
            thumb: None,
        }
    }
}

/// The fixed-height reply preview row above the field: full composer width
/// (same edges as the message field — Jacob, Oct 21; Grok keeps bars flush),
/// [`REPLY_PREVIEW_HEIGHT`] tall, the quoted line on one line with the
/// ellipsis at its end. `clear` is the × at the right.
pub fn reply_preview_bar(
    preview: &ReplyPreview,
    theme: &Theme,
    clear: impl IntoElement,
) -> gpui::Stateful<Div> {
    let mut row = div()
        .id("composer-reply")
        .debug_selector(|| "composer-reply".into())
        .flex()
        .items_center()
        .gap(px(8.0))
        .w_full()
        .h(px(REPLY_PREVIEW_HEIGHT))
        .flex_shrink_0()
        .overflow_hidden()
        .px(px(14.0))
        .rounded(px(14.0))
        .bg(theme.bubble_assistant)
        .child(
            div()
                .flex_shrink_0()
                .child(icon("reply", 12.0, theme.text_muted)),
        )
        .child(
            div()
                .flex_shrink_0()
                .whitespace_nowrap()
                .text_size(px(TEXT_TINY))
                .text_color(theme.text_muted)
                .child(format!("Replying to {}", preview.who)),
        );
    if let Some(image) = preview.thumb.clone() {
        row = row.child(
            img(image)
                .size(px(REPLY_PREVIEW_THUMB))
                .flex_shrink_0()
                .rounded(px(4.0))
                .object_fit(ObjectFit::Cover),
        );
    }
    // A block (not a flex) slot: GPUI's text only truncates with an ellipsis when
    // its parent hands it a definite width, which a flex item's max-content
    // measure pass does not (first cut rendered a hard clip, no "…").
    row.child(
        div()
            .id("composer-reply-line")
            .debug_selector(|| "composer-reply-line".into())
            .flex_1()
            .min_w_0()
            .text_size(px(TEXT_TINY))
            .text_color(theme.text)
            .truncate()
            .child(preview.line.clone()),
    )
    .child(clear)
}

#[cfg(test)]
mod reply_preview_tests {
    //! The reply preview above the composer is one fixed-height row whatever
    //! the quoted message looks like (Jacob, 2026-10-23: a multi-paragraph
    //! markdown reply made the bar four lines tall). Rendered headless through
    //! the same `reply_preview_bar` the composer draws.
    use gpui::TestAppContext;

    use super::*;
    use crate::ui::theme::{AppearancePref, Surface};

    fn message(text: &str) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": "m1",
            "role": "assistant",
            "status": "done",
            "senderHandle": "lium",
            "parts": [{ "kind": "text", "id": "t1", "text": text }],
        }))
        .expect("message")
    }

    const ONE_LINE: &str = "Arbos, from where I sat: the authority held.";

    fn twenty_lines() -> String {
        let mut text = String::from("Arbos, from where I sat:\n\n**What worked**\n");
        for i in 1..=17 {
            text.push_str(&format!(
                "- Authority held: point {i} of the review, with detail.\n"
            ));
        }
        text
    }

    struct Bar {
        text: String,
    }

    impl gpui::Render for Bar {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = cx.global::<Theme>().clone();
            let preview = ReplyPreview::for_message(&message(&self.text), &HashMap::new());
            div()
                .w(px(852.0))
                .h(px(300.0))
                .flex()
                .flex_col()
                .child(reply_preview_bar(
                    &preview,
                    &theme,
                    div()
                        .debug_selector(|| "composer-reply-clear".into())
                        .size(px(18.0))
                        .flex_shrink_0(),
                ))
        }
    }

    struct Measured {
        bar: gpui::Bounds<Pixels>,
        line: gpui::Bounds<Pixels>,
        clear: gpui::Bounds<Pixels>,
    }

    fn measure(cx: &mut TestAppContext, text: String) -> Measured {
        cx.update(|cx| {
            if !cx.has_global::<Theme>() {
                let theme = Theme::for_pref(AppearancePref::Dark, Surface::Matte, cx);
                cx.set_global(theme);
            }
        });
        let (_view, vcx) = cx.add_window_view(|_, _| Bar { text });
        vcx.run_until_parked();
        Measured {
            bar: vcx.debug_bounds("composer-reply").expect("bar drawn"),
            line: vcx.debug_bounds("composer-reply-line").expect("line drawn"),
            clear: vcx
                .debug_bounds("composer-reply-clear")
                .expect("clear drawn"),
        }
    }

    #[gpui::test]
    fn the_bar_is_the_same_height_for_one_line_and_twenty(cx: &mut TestAppContext) {
        let short = measure(cx, ONE_LINE.to_owned());
        let long = measure(cx, twenty_lines());
        let short_h: f32 = short.bar.size.height.into();
        let long_h: f32 = long.bar.size.height.into();
        assert_eq!(short_h, REPLY_PREVIEW_HEIGHT, "one-line bar height");
        assert_eq!(long_h, REPLY_PREVIEW_HEIGHT, "twenty-line bar height");
        assert_eq!(short_h, long_h, "the bar never fits itself to the message");
        // The text slot is one line inside the bar, for either message.
        let short_line: f32 = short.line.size.height.into();
        let long_line: f32 = long.line.size.height.into();
        assert!(
            short_line > 0.0 && short_line <= REPLY_PREVIEW_HEIGHT,
            "{short_line}"
        );
        assert_eq!(
            short_line, long_line,
            "the twenty-line quote takes one line"
        );
    }

    #[gpui::test]
    fn the_quoted_line_stays_inside_the_bar_whatever_its_length(cx: &mut TestAppContext) {
        // A quoted line far wider than the bar neither widens the row nor
        // pushes the × out: the bar is the column's width and the text slot
        // ends where the × begins, with the text clipped (ellipsis) inside it.
        let m = measure(cx, twenty_lines());
        let bar_w: f32 = m.bar.size.width.into();
        assert!((bar_w - 852.0).abs() <= 1.0, "bar width {bar_w}");
        let line_right: f32 = (m.line.origin.x + m.line.size.width).into();
        let clear_left: f32 = m.clear.origin.x.into();
        let bar_right: f32 = (m.bar.origin.x + m.bar.size.width).into();
        assert!(
            line_right <= clear_left + 0.5,
            "line ends {line_right}, × at {clear_left}"
        );
        let clear_right: f32 = (m.clear.origin.x + m.clear.size.width).into();
        assert!(
            clear_right <= bar_right - 13.0,
            "× right {clear_right} inside the 14 pt padding of {bar_right}"
        );
    }

    #[test]
    fn the_quoted_text_is_one_flattened_line_with_an_ellipsis() {
        let preview = ReplyPreview::for_message(&message(&twenty_lines()), &HashMap::new());
        assert_eq!(preview.who, "@lium");
        assert!(!preview.line.contains('\n'), "{:?}", preview.line);
        assert!(!preview.line.contains("**"), "{:?}", preview.line);
        assert!(!preview.line.contains("- Authority"), "{:?}", preview.line);
        assert!(
            preview
                .line
                .starts_with("Arbos, from where I sat: What worked Authority held: point 1"),
            "{:?}",
            preview.line
        );
        assert!(preview.line.ends_with('…'), "{:?}", preview.line);
        assert!(preview.line.chars().count() <= REPLY_PREVIEW_CHARS + 1);

        let short = ReplyPreview::for_message(&message(ONE_LINE), &HashMap::new());
        assert_eq!(short.line, ONE_LINE);
        assert!(short.thumb.is_none());

        // Inline markers go, and punctuation glued to a code span stays glued.
        let code = ReplyPreview::for_message(
            &message("The draft lives in `site/launch/`; **ask** [Sam](https://x.ai) first."),
            &HashMap::new(),
        );
        assert_eq!(code.line, "The draft lives in site/launch/; ask Sam first.");
    }

    #[test]
    fn a_picture_without_prose_names_its_file() {
        let m: Message = serde_json::from_value(serde_json::json!({
            "id": "m2",
            "role": "user",
            "status": "done",
            "parts": [{ "kind": "attachment", "id": "a1",
                        "attachment": { "name": "IMG_0412.jpg", "mime": "image/jpeg", "ref": "blob/x" } }],
        }))
        .expect("message");
        let preview = ReplyPreview::for_message(&m, &HashMap::new());
        assert_eq!(preview.who, "you");
        assert_eq!(preview.line, "IMG_0412.jpg");
        assert!(preview.thumb.is_none(), "no decoded thumbnail yet");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(format: gpui::ImageFormat, bytes: &[u8]) -> gpui::Image {
        let bytes = bytes.to_vec();
        gpui::Image {
            format,
            id: gpui::hash(&bytes),
            bytes,
        }
    }

    #[test]
    fn pasted_text_is_text() {
        let p = Pasted::from_clipboard(&ClipboardItem::new_string("hello".into()));
        assert_eq!(p.text.as_deref(), Some("hello"));
        assert!(p.files.is_empty() && p.images.is_empty());
    }

    #[test]
    fn pasted_image_is_an_image_with_no_text() {
        let img = image(gpui::ImageFormat::Png, b"png");
        let p = Pasted::from_clipboard(&ClipboardItem {
            entries: vec![ClipboardEntry::Image(img.clone())],
        });
        assert_eq!(p.text, None);
        assert_eq!(p.images, vec![img]);
    }

    #[test]
    fn pasted_finder_copy_keeps_the_paths_and_drops_the_text_twin() {
        let p = Pasted::from_clipboard(&ClipboardItem {
            entries: vec![
                ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
                    [PathBuf::from("/a.pdf"), PathBuf::from("/b c.png")]
                        .into_iter()
                        .collect(),
                )),
                ClipboardEntry::String(gpui::ClipboardString::new("/a.pdf\n/b c.png".into())),
            ],
        });
        assert_eq!(p.text, None);
        assert_eq!(
            p.files,
            vec![PathBuf::from("/a.pdf"), PathBuf::from("/b c.png")]
        );
    }

    #[test]
    fn stashed_image_gets_a_dated_name_and_its_extension() {
        let dir = std::env::temp_dir().join(format!("arbos-paste-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let img = image(gpui::ImageFormat::Jpeg, b"jpeg bytes");
        let a = stash_pasted_image(&img, &dir).unwrap();
        let b = stash_pasted_image(&img, &dir).unwrap();
        let name = a.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("Pasted image "), "{name}");
        assert!(name.ends_with(".jpg"), "{name}");
        assert_ne!(a, b, "a second paste in the same second gets its own file");
        assert_eq!(std::fs::read(&a).unwrap(), b"jpeg bytes");
        assert!(is_image_path(&a));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recording_shows_a_stop_button_whatever_else_is_going_on() {
        for has_text in [false, true] {
            for running in [false, true] {
                for guide in [false, true] {
                    assert_eq!(
                        primary_action(has_text, running, true, guide),
                        ButtonAction::StopMemo
                    );
                }
                assert!(mic_mounted(has_text, true));
            }
        }
        assert_eq!(ButtonAction::StopMemo.glyph(), "stop");
    }

    #[test]
    fn idle_field_keeps_the_iphone_layout() {
        assert_eq!(
            primary_action(false, false, false, false),
            ButtonAction::Call
        );
        assert!(mic_mounted(false, false));
        assert_eq!(
            primary_action(true, false, false, false),
            ButtonAction::Send
        );
        assert!(!mic_mounted(true, false));
        assert_eq!(
            primary_action(false, true, false, false),
            ButtonAction::Stop
        );
        assert_eq!(primary_action(true, true, false, false), ButtonAction::Send);
        assert_eq!(primary_action(true, true, false, true), ButtonAction::Guide);
    }

    const SENTENCE: &str = "the quick brown fox";

    #[test]
    fn option_left_steps_one_word_not_one_line() {
        let end = SENTENCE.len();
        let before_fox = previous_word_boundary(SENTENCE, end);
        assert_eq!(&SENTENCE[before_fox..], "fox");
        assert_eq!(
            &SENTENCE[previous_word_boundary(SENTENCE, before_fox)..],
            "brown fox"
        );
        assert_eq!(
            &SENTENCE[previous_word_boundary(SENTENCE, 10)..],
            "quick brown fox"
        );
        assert_eq!(previous_word_boundary(SENTENCE, 4), 0);
        assert_eq!(previous_word_boundary(SENTENCE, 0), 0);
        assert_eq!(previous_word_boundary("fox   ", 6), 0);
        assert_eq!(previous_word_boundary(SENTENCE, end + 10), before_fox);
    }

    #[test]
    fn option_right_steps_one_word_not_one_line() {
        assert_eq!(&SENTENCE[..next_word_boundary(SENTENCE, 0)], "the");
        assert_eq!(&SENTENCE[..next_word_boundary(SENTENCE, 3)], "the quick");
        assert_eq!(&SENTENCE[..next_word_boundary(SENTENCE, 5)], "the quick");
        assert_eq!(next_word_boundary(SENTENCE, 16), SENTENCE.len());
        assert_eq!(next_word_boundary(SENTENCE, SENTENCE.len()), SENTENCE.len());
        assert_eq!(next_word_boundary("   fox", 0), 6);
    }

    #[test]
    fn word_steps_stop_at_punctuation_and_cross_newlines() {
        let text = "hello, world\nsecond line";
        assert_eq!(&text[previous_word_boundary(text, 6)..6], ",");
        assert_eq!(&text[previous_word_boundary(text, 5)..5], "hello");
        assert_eq!(&text[..next_word_boundary(text, 5)], "hello,");
        let second = text.find("second").unwrap();
        assert_eq!(
            &text[previous_word_boundary(text, second)..second],
            "world\n"
        );
        assert_eq!(&text[12..next_word_boundary(text, 12)], "\nsecond");
    }

    #[test]
    fn word_steps_respect_multibyte_characters() {
        let text = "naïve café résumé";
        let end = text.len();
        let last = previous_word_boundary(text, end);
        assert_eq!(&text[last..], "résumé");
        assert_eq!(&text[previous_word_boundary(text, last)..last], "café ");
        assert_eq!(&text[..next_word_boundary(text, 0)], "naïve");
    }

    #[test]
    fn option_backspace_removes_exactly_one_word() {
        let mut text = SENTENCE.to_string();
        let cursor = text.len();
        let prev = previous_word_boundary(&text, cursor);
        text.replace_range(prev..cursor, "");
        assert_eq!(text, "the quick brown ");
        let cursor = text.len();
        let prev = previous_word_boundary(&text, cursor);
        text.replace_range(prev..cursor, "");
        assert_eq!(text, "the quick ");
    }

    #[test]
    fn cmd_left_right_use_hard_lines() {
        let text = "first line\nsecond line";
        assert_eq!(line_start(text, 0), 0);
        assert_eq!(line_start(text, 5), 0);
        assert_eq!(line_end(text, 5), 10);
        assert_eq!(line_start(text, 11), 11);
        assert_eq!(line_start(text, text.len()), 11);
        assert_eq!(line_end(text, 11), text.len());
        assert_eq!(line_start(SENTENCE, SENTENCE.len()), 0);
        assert_eq!(line_end(SENTENCE, 0), SENTENCE.len());
    }
}

#[cfg(test)]
mod enter_tests {
    //! Typing in the composer and pressing Enter must never crash the app (#1208, Mac v0.6.89/v0.6.90).
    //! The Composer wires mention-menu callbacks into `ComposerInput` that read and update the input
    //! entity itself; the input is leased while its own action handler runs, so those callbacks have to
    //! run deferred. These tests install callbacks of exactly that shape and press the keys in a real
    //! (headless) window.
    use super::*;
    use gpui::{Action, TestAppContext, VisualTestContext};
    use std::cell::Cell;

    fn input_with_owner_callbacks(
        cx: &mut TestAppContext,
    ) -> (Entity<ComposerInput>, &mut VisualTestContext, Rc<Cell<u32>>) {
        let submitted = Rc::new(Cell::new(0u32));
        cx.update(|cx| {
            let theme = crate::ui::theme::Theme::for_pref(
                crate::ui::theme::AppearancePref::Dark,
                crate::ui::theme::Surface::Matte,
                cx,
            );
            cx.set_global(theme);
            crate::ui::motion::PulseClock::init(cx);
        });
        let (input, vcx) = cx.add_window_view(|_, cx| ComposerInput::new(cx));
        let weak = input.downgrade();
        let count = submitted.clone();
        vcx.update(|_, cx| {
            input.update(cx, |input, _| {
                input.on_submit = Some(Rc::new(move |_, _| count.set(count.get() + 1)));
                // What `Composer` does: read the input (mention query), and complete into it when a row is chosen.
                let w1 = weak.clone();
                input.on_mention_accept = Some(Rc::new(move |cx| {
                    let Some(e) = w1.upgrade() else { return false };
                    let has_at = e.read(cx).text().contains('@');
                    if has_at {
                        e.update(cx, |i, cx| i.complete_mention("sam", cx));
                    }
                    has_at
                }));
                let w2 = weak.clone();
                input.on_mention_next = Some(Rc::new(move |cx| {
                    w2.upgrade()
                        .map(|e| e.read(cx).text().contains('@'))
                        .unwrap_or(false)
                }));
                let w3 = weak.clone();
                input.on_mention_prev = Some(Rc::new(move |cx| {
                    w3.upgrade()
                        .map(|e| e.read(cx).text().contains('@'))
                        .unwrap_or(false)
                }));
            });
        });
        let handle = vcx.update(|_, cx| input.read(cx).focus_handle(cx));
        vcx.update(|window, cx| window.focus(&handle, cx));
        vcx.run_until_parked();
        (input, vcx, submitted)
    }

    #[gpui::test]
    fn typing_then_enter_submits_without_crashing(cx: &mut TestAppContext) {
        let (input, cx, submitted) = input_with_owner_callbacks(cx);
        cx.simulate_input("hello from the enter test");
        cx.dispatch_action(Send);
        cx.run_until_parked();
        assert_eq!(submitted.get(), 1, "Enter must submit the text once");
        assert_eq!(
            cx.update(|_, cx| input.read(cx).text().to_string()),
            "hello from the enter test"
        );
    }

    #[gpui::test]
    fn enter_with_an_open_mention_completes_instead_of_sending(cx: &mut TestAppContext) {
        let (input, cx, submitted) = input_with_owner_callbacks(cx);
        cx.simulate_input("hi @sa");
        cx.dispatch_action(Send);
        cx.run_until_parked();
        assert_eq!(
            submitted.get(),
            0,
            "Enter on an open @mention accepts the row, it does not send"
        );
        let text = cx.update(|_, cx| input.read(cx).text().to_string());
        assert!(
            text.starts_with("hi @sam"),
            "the mention was completed: {text:?}"
        );
    }

    #[gpui::test]
    fn tab_with_an_open_mention_completes_the_highlighted_row(cx: &mut TestAppContext) {
        // Tab inserts the highlighted handle ([feedback
        // app-01M5WKBR85JVW1KKJH96FWZVK5]); it must not only move the highlight.
        let (input, cx, submitted) = input_with_owner_callbacks(cx);
        cx.simulate_input("hi @sa");
        cx.dispatch_action(AcceptMention);
        cx.run_until_parked();
        assert_eq!(submitted.get(), 0, "Tab on an open @mention does not send");
        let text = cx.update(|_, cx| input.read(cx).text().to_string());
        assert!(
            text.starts_with("hi @sam"),
            "Tab completed the highlighted mention: {text:?}"
        );
    }

    /// Paste with an owner hook, as the chat composer has: text at the caret,
    /// files and images to the hook ([feedback desktop-paste-2026-10-23]).
    type PastedMedia = Rc<std::cell::RefCell<Vec<(Vec<PathBuf>, Vec<gpui::Image>)>>>;

    fn input_with_paste_hook(
        cx: &mut TestAppContext,
    ) -> (Entity<ComposerInput>, &mut VisualTestContext, PastedMedia) {
        let (input, vcx, _) = input_with_owner_callbacks(cx);
        let media = Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = media.clone();
        vcx.update(|_, cx| {
            input.update(cx, |input, _| {
                input.on_paste_media = Some(Rc::new(move |files, images, _| {
                    sink.borrow_mut().push((files, images));
                }));
            });
        });
        (input, vcx, media)
    }

    fn png_1x1() -> gpui::Image {
        let bytes = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x62, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE5, 0x27, 0xDE, 0xFC, 0x00, 0x00,
            0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        gpui::Image {
            format: gpui::ImageFormat::Png,
            id: gpui::hash(&bytes),
            bytes,
        }
    }

    #[gpui::test]
    fn paste_text_lands_at_the_caret(cx: &mut TestAppContext) {
        let (input, cx, media) = input_with_paste_hook(cx);
        cx.simulate_input("see ");
        cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("this".into())));
        cx.dispatch_action(Paste);
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| input.read(cx).text().to_string()),
            "see this"
        );
        assert!(media.borrow().is_empty(), "plain text stages nothing");
    }

    #[gpui::test]
    fn paste_image_stages_it_and_types_nothing(cx: &mut TestAppContext) {
        let (input, cx, media) = input_with_paste_hook(cx);
        cx.simulate_input("look");
        let image = png_1x1();
        cx.update(|_, cx| {
            cx.write_to_clipboard(ClipboardItem {
                entries: vec![ClipboardEntry::Image(image.clone())],
            })
        });
        cx.dispatch_action(Paste);
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| input.read(cx).text().to_string()), "look");
        let media = media.borrow();
        assert_eq!(media.len(), 1, "the hook ran once");
        assert!(media[0].0.is_empty());
        assert_eq!(media[0].1, vec![image]);
    }

    #[gpui::test]
    fn paste_finder_copy_stages_the_files_not_their_paths(cx: &mut TestAppContext) {
        let (input, cx, media) = input_with_paste_hook(cx);
        let paths = vec![
            PathBuf::from("/tmp/report.pdf"),
            PathBuf::from("/tmp/shot.png"),
        ];
        // What AppKit hands over for a Finder copy: the paths plus a text
        // twin with the same paths joined by newlines.
        cx.update(|_, cx| {
            cx.write_to_clipboard(ClipboardItem {
                entries: vec![
                    ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
                        paths.iter().cloned().collect(),
                    )),
                    ClipboardEntry::String(gpui::ClipboardString::new(
                        "/tmp/report.pdf\n/tmp/shot.png".into(),
                    )),
                ],
            })
        });
        cx.dispatch_action(Paste);
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| input.read(cx).text().to_string()),
            "",
            "the path list is never typed into the message"
        );
        let media = media.borrow();
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].0, paths);
        assert!(media[0].1.is_empty());
    }

    #[gpui::test]
    fn paste_without_an_owner_hook_keeps_text_only(cx: &mut TestAppContext) {
        // Search / name / secret fields: files and images are ignored, text works.
        let (input, cx, _) = input_with_owner_callbacks(cx);
        cx.update(|_, cx| {
            cx.write_to_clipboard(ClipboardItem {
                entries: vec![ClipboardEntry::Image(png_1x1())],
            })
        });
        cx.dispatch_action(Paste);
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| input.read(cx).text().to_string()), "");
        cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("abc".into())));
        cx.dispatch_action(Paste);
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| input.read(cx).text().to_string()), "abc");
    }

    #[gpui::test]
    fn mention_keys_without_a_mention_are_plain_editing(cx: &mut TestAppContext) {
        let (input, cx, submitted) = input_with_owner_callbacks(cx);
        cx.simulate_input("plain text");
        let keys: [Box<dyn Action>; 4] = [
            Box::new(Up),
            Box::new(Down),
            Box::new(AcceptMention),
            Box::new(PrevMention),
        ];
        for action in keys {
            cx.update(|window, cx| window.dispatch_action(action, cx));
            cx.run_until_parked();
        }
        assert_eq!(submitted.get(), 0);
        assert_eq!(
            cx.update(|_, cx| input.read(cx).text().to_string()),
            "plain text"
        );
    }
}

#[cfg(test)]
mod mention_token_tests {
    //! Composer mention chips (#1596): a picked row or an exact `@handle` is
    //! one token in the field — the caret skips it, ⌫ removes it whole, ⌘C and
    //! the send read `@handle`, and the send carries its member id.
    use super::*;
    use crate::app::ProfilePage;
    use crate::ui::mention::tests::stub;
    use gpui::{TestAppContext, VisualTestContext};

    fn table() -> Rc<Mentions> {
        Mentions::with_table(
            vec![
                (
                    "sam",
                    stub(
                        "Sam Rivera",
                        ProfilePage::User {
                            handle: "sam".into(),
                            space_id: None,
                        },
                    ),
                ),
                (
                    "studio",
                    stub(
                        "Studio",
                        ProfilePage::Agent {
                            project_id: "p-studio".into(),
                        },
                    ),
                ),
                // The collectives, as `Mentions::from_state` adds them.
                (
                    "here",
                    mention::Mention {
                        page: None,
                        name: "here".into(),
                        picture: mention::Picture::Initials {
                            text: "H".into(),
                            top: gpui::Hsla::default(),
                            bottom: gpui::Hsla::default(),
                        },
                        id: Some("here".into()),
                    },
                ),
                (
                    "agents",
                    mention::Mention {
                        page: None,
                        name: "agents".into(),
                        picture: mention::Picture::Initials {
                            text: "A".into(),
                            top: gpui::Hsla::default(),
                            bottom: gpui::Hsla::default(),
                        },
                        id: Some("agents".into()),
                    },
                ),
                // `@arbos` is an agent and a person (#1538).
                (
                    "arbos",
                    stub(
                        "Arbos",
                        ProfilePage::Agent {
                            project_id: "p-arbos".into(),
                        },
                    ),
                ),
                (
                    "arbos",
                    stub(
                        "Jacob",
                        ProfilePage::User {
                            handle: "arbos".into(),
                            space_id: None,
                        },
                    ),
                ),
            ],
            |_, _| {},
        )
    }

    fn input(cx: &mut TestAppContext) -> (Entity<ComposerInput>, &mut VisualTestContext) {
        cx.update(|cx| {
            let theme = crate::ui::theme::Theme::for_pref(
                crate::ui::theme::AppearancePref::Dark,
                crate::ui::theme::Surface::Matte,
                cx,
            );
            cx.set_global(theme);
            crate::ui::motion::PulseClock::init(cx);
        });
        let (input, vcx) = cx.add_window_view(|_, cx| ComposerInput::new(cx));
        vcx.update(|_, cx| {
            input.update(cx, |input, _| input.set_mentions(table()));
        });
        let handle = vcx.update(|_, cx| input.read(cx).focus_handle(cx));
        vcx.update(|window, cx| window.focus(&handle, cx));
        vcx.run_until_parked();
        (input, vcx)
    }

    fn text(input: &Entity<ComposerInput>, cx: &mut VisualTestContext) -> String {
        cx.update(|_, cx| input.read(cx).text().to_string())
    }

    fn sent(input: &Entity<ComposerInput>, cx: &mut VisualTestContext) -> (String, Vec<String>) {
        cx.update(|_, cx| {
            let i = input.read(cx);
            (i.sent_text(), i.token_ids())
        })
    }

    fn caret(input: &Entity<ComposerInput>, cx: &mut VisualTestContext) -> usize {
        cx.update(|_, cx| input.read(cx).cursor_offset())
    }

    fn chip(name: &str) -> String {
        format!("{}{name}", mention::CELL)
    }

    /// Jacob (Oct 24): the composer's chips answer the pointer like bubble
    /// chips — hand cursor, brighter pill on hover, brighter still pressed,
    /// and a plain click opens the entity's page. Plain text keeps the I-beam.
    #[gpui::test]
    fn a_composer_chip_brightens_under_the_pointer_and_opens_on_click(cx: &mut TestAppContext) {
        use crate::ui::mention::{ChipState, PILL_ALPHA, PILL_ALPHA_HOVER, PILL_ALPHA_PRESSED};
        use std::cell::RefCell;
        let opened: Rc<RefCell<Vec<ProfilePage>>> = Rc::new(RefCell::new(Vec::new()));
        let record = opened.clone();
        let (input, cx) = input(cx);
        cx.update(|_, cx| {
            input.update(cx, |i, _| {
                i.set_mentions(Mentions::with_table(
                    vec![(
                        "sam",
                        stub(
                            "Sam Rivera",
                            ProfilePage::User {
                                handle: "sam".into(),
                                space_id: None,
                            },
                        ),
                    )],
                    move |page, _| record.borrow_mut().push(page.clone()),
                ))
            })
        });
        cx.simulate_input("hi @s");
        cx.update(|_, cx| {
            input.update(cx, |i, cx| {
                i.insert_mention("sam", "Sam Rivera", Some("user:gh_sam".into()), cx)
            })
        });
        cx.simulate_input("and more words here");
        cx.run_until_parked();
        let (key, rects) =
            cx.update(|_, cx| input.read(cx).layout.as_ref().unwrap().chips[0].clone());
        assert_eq!(key, 3..3 + chip("Sam Rivera").len());
        let on_chip = rects[0].center();
        let bounds = cx.update(|_, cx| input.read(cx).layout.as_ref().unwrap().bounds);
        let on_text = point(bounds.right() - px(6.0), on_chip.y);
        let none = gpui::Modifiers::default();
        let settled = |i: &ComposerInput, snap: bool| {
            i.chips
                .alpha(&key, Instant::now() + mention::PILL_FADE, snap)
                .0
        };

        cx.simulate_mouse_move(on_text, None, none);
        cx.update(|_, cx| input.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        cx.update(|_, cx| {
            let i = input.read(cx);
            assert_eq!(i.chips.hot(), None);
            assert_eq!(i.chip_cursor, CursorStyle::IBeam, "plain text: I-beam");
        });

        cx.simulate_mouse_move(on_chip, None, none);
        cx.run_until_parked();
        cx.update(|_, cx| {
            let i = input.read(cx);
            assert_eq!(i.chips.state(&key), ChipState::Hover);
            assert_eq!(
                i.chip_cursor,
                CursorStyle::PointingHand,
                "a hand over the chip"
            );
            assert_eq!(settled(i, false), PILL_ALPHA_HOVER);
        });

        cx.simulate_mouse_down(on_chip, MouseButton::Left, none);
        cx.run_until_parked();
        cx.update(|_, cx| {
            let i = input.read(cx);
            assert_eq!(i.chips.state(&key), ChipState::Pressed);
            assert_eq!(settled(i, false), PILL_ALPHA_PRESSED);
        });

        cx.simulate_mouse_up(on_chip, MouseButton::Left, none);
        cx.run_until_parked();
        cx.update(|_, cx| {
            let i = input.read(cx);
            assert_eq!(
                i.chips.state(&key),
                ChipState::Hover,
                "released: hover again"
            );
            assert!(
                i.selected_range.is_empty() && i.token_around(i.cursor_offset()).is_none(),
                "the caret sits at the chip's edge, not inside it"
            );
        });
        assert_eq!(opened.borrow().len(), 1, "a plain click opens the page");
        assert!(matches!(&opened.borrow()[0], ProfilePage::User { handle, .. } if handle == "sam"));

        cx.simulate_mouse_move(on_text, None, none);
        cx.run_until_parked();
        cx.update(|_, cx| {
            let i = input.read(cx);
            assert_eq!(i.chips.state(&key), ChipState::Rest);
            assert_eq!(i.chip_cursor, CursorStyle::IBeam);
            assert_eq!(settled(i, false), PILL_ALPHA, "fades back to rest");
        });

        // A drag that starts on the chip and selects text is not a click.
        cx.simulate_mouse_down(on_chip, MouseButton::Left, none);
        cx.simulate_mouse_move(on_text, MouseButton::Left, none);
        cx.simulate_mouse_up(on_text, MouseButton::Left, none);
        cx.run_until_parked();
        assert_eq!(opened.borrow().len(), 1, "a drag opens nothing");
        assert!(!cx.update(|_, cx| input.read(cx).selected_range.is_empty()));
    }

    #[gpui::test]
    fn a_picked_row_is_one_chip_and_the_send_carries_its_id(cx: &mut TestAppContext) {
        let (input, cx) = input(cx);
        cx.simulate_input("hi @sa");
        assert_eq!(
            cx.update(|_, cx| input.read(cx).open_mention_token().map(str::to_string)),
            Some("sa".into())
        );
        cx.update(|_, cx| {
            input.update(cx, |i, cx| {
                i.insert_mention("sam", "Sam Rivera", Some("user:gh_sam".into()), cx)
            })
        });
        assert_eq!(text(&input, cx), format!("hi {} ", chip("Sam Rivera")));
        let tokens = cx.update(|_, cx| input.read(cx).tokens().to_vec());
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].range, 3..3 + chip("Sam Rivera").len());
        assert_eq!(
            sent(&input, cx),
            ("hi @sam ".into(), vec!["user:gh_sam".into()])
        );
        // The menu is closed: the chip's own `@` is not an open token.
        assert_eq!(
            cx.update(|_, cx| input.read(cx).open_mention_token().map(str::to_string)),
            None
        );
        // Typing goes on after the chip.
        cx.simulate_input("ok?");
        assert_eq!(sent(&input, cx).0, "hi @sam ok?");
    }

    #[gpui::test]
    fn backspace_removes_the_whole_chip_and_arrows_skip_it(cx: &mut TestAppContext) {
        let (input, cx) = input(cx);
        cx.simulate_input("hi @s");
        cx.update(|_, cx| {
            input.update(cx, |i, cx| {
                i.insert_mention("sam", "Sam Rivera", Some("user:gh_sam".into()), cx)
            })
        });
        let end = text(&input, cx).len();
        assert_eq!(caret(&input, cx), end);
        // ← over the space, then ← jumps the whole chip.
        cx.dispatch_action(Left);
        cx.run_until_parked();
        assert_eq!(caret(&input, cx), end - 1);
        cx.dispatch_action(Left);
        cx.run_until_parked();
        assert_eq!(caret(&input, cx), 3, "the caret never stops inside a chip");
        // → jumps back over it.
        cx.dispatch_action(Right);
        cx.run_until_parked();
        assert_eq!(caret(&input, cx), end - 1);
        // ⌫ at the chip's right edge removes the chip whole.
        cx.dispatch_action(Backspace);
        cx.run_until_parked();
        assert_eq!(text(&input, cx), "hi  ");
        assert!(cx.update(|_, cx| input.read(cx).tokens().is_empty()));
    }

    #[gpui::test]
    fn an_exact_handle_and_a_separator_resolve_without_the_picker(cx: &mut TestAppContext) {
        let (input, cx) = input(cx);
        cx.simulate_input("ping @sam ");
        assert_eq!(text(&input, cx), format!("ping {} ", chip("Sam Rivera")));
        assert_eq!(
            sent(&input, cx),
            ("ping @sam ".into(), vec!["user:gh_sam".into()])
        );
        // Punctuation completes it too, and case does not matter.
        cx.simulate_input("and @STUDIO?");
        assert_eq!(
            sent(&input, cx),
            (
                "ping @sam and @STUDIO?".into(),
                vec!["user:gh_sam".into(), "agent:p-studio".into()]
            )
        );
        assert!(text(&input, cx).ends_with(&format!("{}?", chip("Studio"))));
    }

    #[gpui::test]
    fn a_shared_handle_stays_plain_until_picked(cx: &mut TestAppContext) {
        let (input, cx) = input(cx);
        cx.simulate_input("@arbos can you ");
        assert_eq!(text(&input, cx), "@arbos can you ");
        assert_eq!(sent(&input, cx), ("@arbos can you ".into(), Vec::new()));
        // The picker names the person: the chip carries that id, not the agent's.
        cx.simulate_input("@ar");
        cx.update(|_, cx| {
            input.update(cx, |i, cx| {
                i.insert_mention("arbos", "Jacob", Some("user:gh_arbos".into()), cx)
            })
        });
        let (plain, ids) = sent(&input, cx);
        assert_eq!(plain, "@arbos can you @arbos ");
        assert_eq!(ids, vec!["user:gh_arbos".to_string()]);
    }

    #[gpui::test]
    fn enter_resolves_a_trailing_handle_and_unknown_handles_stay_plain(cx: &mut TestAppContext) {
        let (input, cx) = input(cx);
        cx.simulate_input("@nobody and @sam");
        let changed = cx.update(|_, cx| input.update(cx, |i, cx| i.resolve_exact_handles(cx)));
        assert!(changed);
        assert_eq!(
            text(&input, cx),
            format!("@nobody and {}", chip("Sam Rivera"))
        );
        assert_eq!(
            sent(&input, cx),
            ("@nobody and @sam".into(), vec!["user:gh_sam".into()])
        );
        assert_eq!(caret(&input, cx), text(&input, cx).len());
    }

    #[gpui::test]
    fn copy_reads_the_handle_and_a_draft_round_trips(cx: &mut TestAppContext) {
        let (input, cx) = input(cx);
        cx.simulate_input("hi @sam ok");
        cx.dispatch_action(SelectAll);
        cx.dispatch_action(Copy);
        cx.run_until_parked();
        let copied = cx.update(|_, cx| {
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default()
        });
        assert_eq!(copied, "hi @sam ok");
        // The draft keeps the chip across a chat switch.
        let (t, tokens) = cx.update(|_, cx| {
            let i = input.read(cx);
            (i.text().to_string(), i.tokens().to_vec())
        });
        cx.update(|_, cx| input.update(cx, |i, cx| i.clear(cx)));
        assert_eq!(sent(&input, cx), (String::new(), Vec::new()));
        cx.update(|_, cx| input.update(cx, |i, cx| i.set_text_with_tokens(t, tokens, cx)));
        assert_eq!(
            sent(&input, cx),
            ("hi @sam ok".into(), vec!["user:gh_sam".into()])
        );
    }

    #[gpui::test]
    fn here_and_agents_become_collective_chips_whose_marker_the_send_carries(
        cx: &mut TestAppContext,
    ) {
        let (input, cx) = input(cx);
        // Typed with a space: a chip without the picker (Jacob's iPhone bug, 2026-10-24).
        cx.simulate_input("how is the iphone now @here ");
        let tokens = cx.update(|_, cx| input.read(cx).tokens().to_vec());
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, mention::TokenKind::Collective);
        assert_eq!(tokens[0].handle, "here");
        assert_eq!(
            sent(&input, cx),
            ("how is the iphone now @here ".into(), vec!["here".into()])
        );
        assert_eq!(
            text(&input, cx),
            format!("how is the iphone now {} ", chip("here"))
        );
        // Picked from the menu as the Composer does it; Enter resolves a trailing one too.
        cx.update(|_, cx| {
            input.update(cx, |i, cx| {
                i.insert_mention("agents", "agents", Some("agents".into()), cx)
            })
        });
        cx.simulate_input("and @sam");
        cx.update(|_, cx| input.update(cx, |i, cx| i.resolve_exact_handles(cx)));
        let tokens = cx.update(|_, cx| input.read(cx).tokens().to_vec());
        assert_eq!(
            tokens.iter().map(|t| t.kind).collect::<Vec<_>>(),
            vec![
                mention::TokenKind::Collective,
                mention::TokenKind::Collective,
                mention::TokenKind::Member
            ]
        );
        assert_eq!(
            sent(&input, cx).1,
            vec!["here".to_string(), "agents".into(), "user:gh_sam".into()]
        );
    }

    #[test]
    fn candidates_match_the_handle_or_a_word_of_the_name() {
        use arbos_core::search::{Match, MatchField, MatchKind};
        let m = |kind, field| Some(Match { kind, field });
        assert_eq!(
            mention_match("", "sam", "Sam Rivera"),
            Some(Match::EVERYTHING)
        );
        assert_eq!(
            mention_match("sa", "sam", "Sam Rivera"),
            m(MatchKind::Prefix, MatchField::Name)
        );
        assert_eq!(
            mention_match("sam", "sam", "Sam Rivera"),
            m(MatchKind::Exact, MatchField::Handle)
        );
        assert_eq!(
            mention_match("riv", "sam", "Sam Rivera"),
            m(MatchKind::WordStart, MatchField::Name)
        );
        assert_eq!(
            mention_match("k", "mira-k", "Mira Kaplan"),
            m(MatchKind::WordStart, MatchField::Name)
        );
        assert_eq!(
            mention_match("Zoë", "zoe-m", "Zoe Martin"),
            m(MatchKind::Prefix, MatchField::Name)
        );
        assert_eq!(mention_match("x", "sam", "Sam Rivera"), None);
        assert_eq!(mention_match("ivera", "sam", "Sam Rivera"), None);
        assert_eq!(mention_match("am", "sam", "Sam Rivera"), None);
    }
}

#[cfg(test)]
mod caret_tests {
    //! The macOS caret chords in the composer ([feedback
    //! composer-caret-shortcuts-2026-10-27], Jacob): ⌘↑ / ⌘↓ = the draft's
    //! start / end, ⌘← / ⌘→ = the *visual* (wrapped) line's start / end, ⇧
    //! extends the selection, ⌘⌫ deletes to the visual line's start — through
    //! the real keymap (`bind_keys`) on a wrapped two-paragraph draft in a
    //! narrow headless window, and only while the composer is focused.
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    /// A 360 pt column: the first paragraph wraps into several rows.
    const WIDTH: f32 = 360.0;
    const FIRST: &str = "The quick brown fox jumps over the lazy dog while the composer \
                         wraps this long first paragraph onto several visual rows so \
                         that the caret chords have somewhere to go";
    const SECOND: &str = "second paragraph";

    struct Harness {
        input: Entity<ComposerInput>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(WIDTH))
                .h(px(600.0))
                .flex()
                .child(self.input.clone())
        }
    }

    struct Draft {
        input: Entity<ComposerInput>,
        /// Byte index where each soft-wrapped row of the first paragraph
        /// starts (row 0 starts at 0).
        rows: Vec<usize>,
        /// Start of the second paragraph.
        second: usize,
        len: usize,
    }

    fn draft(cx: &mut TestAppContext) -> (Draft, &mut VisualTestContext) {
        cx.update(|cx| {
            let theme = crate::ui::theme::Theme::for_pref(
                crate::ui::theme::AppearancePref::Dark,
                crate::ui::theme::Surface::Matte,
                cx,
            );
            cx.set_global(theme);
            crate::ui::motion::PulseClock::init(cx);
            crate::bind_keys(cx);
        });
        let (view, vcx) = cx.add_window_view(|_, cx| Harness {
            input: cx.new(ComposerInput::new),
        });
        let input = vcx.update(|_, cx| view.read(cx).input.clone());
        let text = format!("{FIRST}\n{SECOND}");
        vcx.update(|window, cx| {
            input.update(cx, |i, cx| i.set_text(text.clone(), cx));
            let handle = input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        });
        vcx.run_until_parked();
        let rows = vcx.update(|_, cx| {
            let layout = input.read(cx).layout.as_ref().expect("painted once");
            assert!(
                layout.bounds.size.width <= px(WIDTH),
                "the harness narrows the field: {:?}",
                layout.bounds.size.width
            );
            let (start, first) = &layout.lines[0];
            assert_eq!(*start, 0);
            let mut rows = vec![0usize];
            rows.extend(wrap_indices(first));
            rows
        });
        assert!(
            rows.len() >= 3,
            "the first paragraph wraps into at least three rows: {rows:?}"
        );
        let d = Draft {
            input,
            rows,
            second: FIRST.len() + 1,
            len: text.len(),
        };
        (d, vcx)
    }

    fn caret(d: &Draft, cx: &mut VisualTestContext) -> usize {
        cx.update(|_, cx| d.input.read(cx).cursor_offset())
    }

    fn selection(d: &Draft, cx: &mut VisualTestContext) -> (Range<usize>, bool) {
        cx.update(|_, cx| {
            let i = d.input.read(cx);
            (i.selected_range.clone(), i.selection_reversed)
        })
    }

    /// Which visual row (0-based, over the whole draft) the caret paints on.
    fn caret_row(d: &Draft, cx: &mut VisualTestContext) -> usize {
        cx.run_until_parked();
        cx.update(|_, cx| {
            let i = d.input.read(cx);
            let layout = i.layout.as_ref().expect("laid out");
            let p = caret_position(
                &layout.lines,
                i.cursor_offset(),
                layout.bounds.size.width,
                i.text_center,
                i.caret_downstream,
            )
            .expect("caret on a row");
            (f32::from(p.y) / INPUT_LINE_HEIGHT).round() as usize
        })
    }

    fn put_caret(d: &Draft, at: usize, cx: &mut VisualTestContext) {
        cx.update(|_, cx| d.input.update(cx, |i, cx| i.move_to(at, cx)));
        cx.run_until_parked();
    }

    fn text(d: &Draft, cx: &mut VisualTestContext) -> String {
        cx.update(|_, cx| d.input.read(cx).text().to_string())
    }

    #[gpui::test]
    fn cmd_up_and_cmd_down_go_to_the_draft_ends(cx: &mut TestAppContext) {
        let (d, cx) = draft(cx);
        let mid = d.rows[1] + 4;
        put_caret(&d, mid, cx);
        cx.simulate_keystrokes("cmd-up");
        assert_eq!(caret(&d, cx), 0, "⌘↑: the start of the whole draft");
        assert_eq!(caret_row(&d, cx), 0);
        cx.simulate_keystrokes("cmd-down");
        assert_eq!(caret(&d, cx), d.len, "⌘↓: the end of the whole draft");
        assert_eq!(
            caret_row(&d, cx),
            d.rows.len(),
            "the second paragraph's row"
        );
        // Not the paragraph: from the second paragraph ⌘↑ still goes to 0.
        put_caret(&d, d.second + 3, cx);
        cx.simulate_keystrokes("cmd-up");
        assert_eq!(caret(&d, cx), 0);
    }

    #[gpui::test]
    fn cmd_left_and_cmd_right_stop_at_the_visual_line_not_the_paragraph(cx: &mut TestAppContext) {
        let (d, cx) = draft(cx);
        // Middle of the second wrapped row of the first paragraph.
        let mid = d.rows[1] + 4;
        put_caret(&d, mid, cx);
        assert_eq!(caret_row(&d, cx), 1);

        cx.simulate_keystrokes("cmd-left");
        assert_eq!(caret(&d, cx), d.rows[1], "⌘←: this row's first byte, not 0");
        assert_eq!(
            caret_row(&d, cx),
            1,
            "the caret paints at the start of its own row, not the end of the row above"
        );

        cx.simulate_keystrokes("cmd-right");
        assert_eq!(
            caret(&d, cx),
            d.rows[2],
            "⌘→: this row's end (where the next row starts), not the paragraph's"
        );
        assert_eq!(caret_row(&d, cx), 1, "painted at the end of its row");

        // A second ⌘→ on the row's end stays on the same byte (already at the end).
        cx.simulate_keystrokes("cmd-right");
        assert_eq!(caret(&d, cx), d.rows[2]);

        // From a row start, ⌘← stays; ⌘→ goes to that row's end.
        put_caret(&d, d.rows[2], cx);
        cx.simulate_keystrokes("cmd-left");
        assert_eq!(
            caret(&d, cx),
            d.rows[1],
            "a boundary caret belongs to the row above (upstream) — ⌘← goes to that row's start"
        );

        // The second paragraph: a hard line that does not wrap.
        put_caret(&d, d.second + 3, cx);
        cx.simulate_keystrokes("cmd-left");
        assert_eq!(caret(&d, cx), d.second);
        cx.simulate_keystrokes("cmd-right");
        assert_eq!(caret(&d, cx), d.len);
    }

    #[gpui::test]
    fn shift_cmd_arrows_extend_the_selection_the_same_way(cx: &mut TestAppContext) {
        let (d, cx) = draft(cx);
        let mid = d.rows[1] + 4;

        put_caret(&d, mid, cx);
        cx.simulate_keystrokes("cmd-shift-left");
        assert_eq!(
            selection(&d, cx),
            (d.rows[1]..mid, true),
            "⇧⌘←: back to the visual row's start, caret at the moving end"
        );
        assert_eq!(caret(&d, cx), d.rows[1]);
        assert_eq!(caret_row(&d, cx), 1);

        put_caret(&d, mid, cx);
        cx.simulate_keystrokes("cmd-shift-right");
        assert_eq!(
            selection(&d, cx),
            (mid..d.rows[2], false),
            "⇧⌘→: to the row's end"
        );

        put_caret(&d, mid, cx);
        cx.simulate_keystrokes("cmd-shift-up");
        assert_eq!(
            selection(&d, cx),
            (0..mid, true),
            "⇧⌘↑: back to the draft's start"
        );

        put_caret(&d, mid, cx);
        cx.simulate_keystrokes("cmd-shift-down");
        assert_eq!(
            selection(&d, cx),
            (mid..d.len, false),
            "⇧⌘↓: on to the draft's end"
        );

        // Extending twice keeps growing from the anchor.
        put_caret(&d, mid, cx);
        cx.simulate_keystrokes("cmd-shift-right cmd-shift-down");
        assert_eq!(selection(&d, cx), (mid..d.len, false));
        cx.simulate_keystrokes("cmd-shift-up");
        assert_eq!(
            selection(&d, cx),
            (0..mid, true),
            "⇧⌘↑ from a forward selection flips it around the anchor"
        );
    }

    #[gpui::test]
    fn cmd_backspace_deletes_to_the_visual_line_start(cx: &mut TestAppContext) {
        let (d, cx) = draft(cx);
        let before = text(&d, cx);
        let at = d.rows[2] + 5;
        put_caret(&d, at, cx);
        cx.simulate_keystrokes("cmd-backspace");
        let expected = format!("{}{}", &before[..d.rows[2]], &before[at..]);
        assert_eq!(
            text(&d, cx),
            expected,
            "⌘⌫ removes this row back to its start; the rows above stay"
        );
        assert_eq!(caret(&d, cx), d.rows[2]);
        // At a row's start (⌘← put it there) it steps back one character, like
        // ⌫, so the key is never dead. Row 1's start is unchanged by the edit
        // above (greedy wrapping only looks at the text before it).
        put_caret(&d, d.rows[1] + 4, cx);
        cx.simulate_keystrokes("cmd-left");
        assert_eq!(caret(&d, cx), d.rows[1]);
        cx.simulate_keystrokes("cmd-backspace");
        assert_eq!(
            text(&d, cx),
            format!("{}{}", &expected[..d.rows[1] - 1], &expected[d.rows[1]..]),
            "one step back: the space the row above wrapped on"
        );
    }

    #[gpui::test]
    fn option_arrows_still_step_words(cx: &mut TestAppContext) {
        let (d, cx) = draft(cx);
        put_caret(&d, 0, cx);
        cx.simulate_keystrokes("alt-right");
        assert_eq!(caret(&d, cx), "The".len(), "⌥→: the end of the first word");
        cx.simulate_keystrokes("alt-right alt-left");
        assert_eq!(
            caret(&d, cx),
            "The ".len(),
            "⌥←: the start of the second word"
        );
        cx.simulate_keystrokes("alt-shift-right");
        assert_eq!(selection(&d, cx), ("The ".len().."The quick".len(), false));
    }

    /// Routing: the chords are the focused composer's; with focus elsewhere
    /// they fall through (nothing else binds them) and the draft is untouched.
    #[gpui::test]
    fn the_chords_reach_only_the_focused_composer(cx: &mut TestAppContext) {
        let (d, cx) = draft(cx);
        let mid = d.rows[1] + 4;
        put_caret(&d, mid, cx);
        cx.update(|window, _| window.blur());
        cx.run_until_parked();
        cx.simulate_keystrokes("cmd-up cmd-down cmd-left cmd-right cmd-shift-up cmd-backspace");
        assert_eq!(caret(&d, cx), mid, "unfocused: the caret did not move");
        assert_eq!(selection(&d, cx).0, mid..mid, "unfocused: nothing selected");
        assert_eq!(
            text(&d, cx),
            format!("{FIRST}\n{SECOND}"),
            "unfocused: nothing deleted"
        );

        cx.update(|window, cx| {
            let handle = d.input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("cmd-up");
        assert_eq!(caret(&d, cx), 0, "focused again: ⌘↑ moves the caret");
    }
}

#[cfg(test)]
mod format_tests {
    //! Chat-Markdown in the composer ([feedback chat-markdown-composer-2026-10-27],
    //! Jacob): ⌘B / ⌘I / ⌘⇧X wrap or unwrap the selection through the real
    //! keymap while the composer is focused, and the draft is styled as it is
    //! typed — markers faint, the words between them styled, a `> ` line muted
    //! with its bar.
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    struct Harness {
        input: Entity<ComposerInput>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(480.0))
                .h(px(300.0))
                .flex()
                .child(self.input.clone())
        }
    }

    fn field<'a>(
        cx: &'a mut TestAppContext,
        text: &str,
    ) -> (Entity<ComposerInput>, &'a mut VisualTestContext) {
        cx.update(|cx| {
            let theme = crate::ui::theme::Theme::for_pref(
                crate::ui::theme::AppearancePref::Dark,
                crate::ui::theme::Surface::Matte,
                cx,
            );
            cx.set_global(theme);
            crate::ui::motion::PulseClock::init(cx);
            crate::bind_keys(cx);
        });
        let (view, vcx) = cx.add_window_view(|_, cx| Harness {
            input: cx.new(ComposerInput::new),
        });
        let input = vcx.update(|_, cx| view.read(cx).input.clone());
        let text = text.to_owned();
        vcx.update(|window, cx| {
            input.update(cx, |i, cx| i.set_text(text, cx));
            let handle = input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        });
        vcx.run_until_parked();
        (input, vcx)
    }

    fn select(input: &Entity<ComposerInput>, range: Range<usize>, cx: &mut VisualTestContext) {
        cx.update(|_, cx| {
            input.update(cx, |i, cx| {
                i.move_to(range.start, cx);
                i.select_to(range.end, cx);
            })
        });
        cx.run_until_parked();
    }

    fn state(input: &Entity<ComposerInput>, cx: &mut VisualTestContext) -> (String, Range<usize>) {
        cx.update(|_, cx| {
            let i = input.read(cx);
            (i.text().to_string(), i.selected_range.clone())
        })
    }

    /// ⌘B wraps the selected words in `**` and keeps them selected; ⌘B again
    /// unwraps; the markers hug the words, never the spaces around them.
    #[gpui::test]
    fn cmd_b_wraps_and_unwraps_the_selection(cx: &mut TestAppContext) {
        let (input, cx) = field(cx, "make this bold please");
        select(&input, 5..9, cx);
        cx.simulate_keystrokes("cmd-b");
        assert_eq!(
            state(&input, cx),
            ("make **this** bold please".into(), 7..11)
        );
        cx.simulate_keystrokes("cmd-b");
        assert_eq!(state(&input, cx), ("make this bold please".into(), 5..9));
        // A selection that drags the spaces in: the markers still hug the word.
        select(&input, 4..10, cx);
        cx.simulate_keystrokes("cmd-b");
        assert_eq!(
            state(&input, cx),
            ("make **this** bold please".into(), 7..11)
        );
        // Selecting the markers too and pressing again strips them.
        select(&input, 5..13, cx);
        cx.simulate_keystrokes("cmd-b");
        assert_eq!(state(&input, cx), ("make this bold please".into(), 5..9));
        // ⌘Z brings the last wrap back.
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(state(&input, cx).0, "make **this** bold please");
    }

    /// ⌘I and ⌘⇧X use `*` and `~~`; the three nest.
    #[gpui::test]
    fn cmd_i_and_cmd_shift_x_wrap_with_their_markers(cx: &mut TestAppContext) {
        let (input, cx) = field(cx, "gone and soft");
        select(&input, 0..4, cx);
        cx.simulate_keystrokes("cmd-shift-x");
        assert_eq!(state(&input, cx), ("~~gone~~ and soft".into(), 2..6));
        select(&input, 13..17, cx);
        cx.simulate_keystrokes("cmd-i");
        assert_eq!(state(&input, cx), ("~~gone~~ and *soft*".into(), 14..18));
        cx.simulate_keystrokes("cmd-b");
        assert_eq!(
            state(&input, cx),
            ("~~gone~~ and ***soft***".into(), 16..20)
        );
        cx.simulate_keystrokes("cmd-b");
        assert_eq!(state(&input, cx), ("~~gone~~ and *soft*".into(), 14..18));
    }

    /// No selection: ⌘B puts an empty pair around the caret, the next words
    /// land inside; ⌘B again on the empty pair removes it.
    #[gpui::test]
    fn cmd_b_with_no_selection_opens_an_empty_pair(cx: &mut TestAppContext) {
        let (input, cx) = field(cx, "say ");
        cx.update(|_, cx| input.update(cx, |i, cx| i.move_to(4, cx)));
        cx.simulate_keystrokes("cmd-b");
        assert_eq!(state(&input, cx), ("say ****".into(), 6..6));
        cx.simulate_keystrokes("cmd-b");
        assert_eq!(state(&input, cx), ("say ".into(), 4..4));
        cx.simulate_keystrokes("cmd-b");
        cx.simulate_keystrokes("h i");
        assert_eq!(state(&input, cx).0, "say **hi**");
    }

    /// Blurred, ⌘B is the sidebar's: the draft does not change.
    #[gpui::test]
    fn blurred_cmd_b_leaves_the_draft_alone(cx: &mut TestAppContext) {
        let (input, cx) = field(cx, "plain words");
        select(&input, 0..5, cx);
        cx.update(|window, _| window.blur());
        cx.run_until_parked();
        cx.simulate_keystrokes("cmd-b");
        assert_eq!(state(&input, cx).0, "plain words");
    }

    /// Live styling: the runs of a typed line carry the grammar — markers in
    /// the faint ink, `**bold**` bold, `~~x~~` struck, `` `c` `` mono on the
    /// code wash, a `> ` line's words muted; plain text is one plain run.
    #[test]
    fn live_styles_paint_markers_faint_and_words_styled() {
        let theme = Theme::dark(crate::ui::theme::Surface::Matte);
        let base = TextRun {
            len: 0,
            font: gpui::Font {
                family: theme.font_sans.into(),
                features: Default::default(),
                fallbacks: None,
                weight: FontWeight::NORMAL,
                style: gpui::FontStyle::Normal,
            },
            color: theme.text,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs_of = |para: &str| {
            let mut runs = runs_for(para, 0, None, &base);
            let quoted = live_styles(para, &mut runs, &theme);
            assert_eq!(
                runs.iter().map(|r| r.len).sum::<usize>(),
                para.len(),
                "{para}"
            );
            (quoted, runs)
        };
        let (quoted, runs) = runs_of("a **b** `c` ~~d~~");
        assert!(!quoted);
        let pieces: Vec<(usize, bool, bool, bool, bool)> = runs
            .iter()
            .map(|r| {
                (
                    r.len,
                    r.color == theme.text_faint,
                    r.font.weight == FontWeight::BOLD,
                    r.background_color == Some(theme.code_bg),
                    r.strikethrough.is_some(),
                )
            })
            .collect();
        assert_eq!(
            pieces,
            vec![
                (2, false, false, false, false), // "a "
                (2, true, false, false, false),  // "**"
                (1, false, true, false, false),  // "b"
                (2, true, false, false, false),  // "**"
                (1, false, false, false, false), // " "
                (1, true, false, false, false),  // "`"
                (1, false, false, true, false),  // "c"
                (1, true, false, false, false),  // "`"
                (1, false, false, false, false), // " "
                (2, true, false, false, false),  // "~~"
                (1, false, false, false, true),  // "d"
                (2, true, false, false, false),  // "~~"
            ]
        );
        let (quoted, runs) = runs_of("> hello *there*");
        assert!(quoted, "a `> ` line draws its bar");
        assert_eq!(runs[0].len, 2);
        assert_eq!(runs[0].color, theme.text_faint, "the `> ` marker is faint");
        assert_eq!(
            runs[1].color, theme.text_muted,
            "the quoted words are muted"
        );
        assert!(runs.iter().any(|r| r.font.style == gpui::FontStyle::Italic));
        for plain in ["snake_case 2*3*4 a_b", ">5 not a quote", "plain words"] {
            let (quoted, runs) = runs_of(plain);
            assert!(!quoted, "{plain}");
            assert_eq!(runs.len(), 1, "{plain}: one plain run, {runs:?}");
            assert_eq!(runs[0].color, theme.text);
        }
    }
}

#[cfg(test)]
mod staged_tests {
    //! The staged-attachment strip ([feedback composer-attachment-removal-2026-10-27]):
    //! removing one keeps the others and the text, ⌫ / ⌦ on an empty draft
    //! take the last one, and the tiles' layout is pinned (one tile each, 8
    //! apart, the remove disc inside the top-right corner, nothing clipped).
    use futures::future::BoxFuture;
    use gpui::{point, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext};
    use tokio::sync::mpsc;

    use super::*;
    use crate::model::{Chat, Project, SpaceKind};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::motion::PulseClock;
    use crate::ui::shell::Shell;

    struct Idle;

    impl SyncSource for Idle {
        fn name(&self) -> &'static str {
            "idle"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            _commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            Box::pin(std::future::pending())
        }
    }

    /// A scratch dir with two pictures (a wide one and a tall one, so Cover
    /// has something to crop) and one text file.
    fn fixtures() -> (PathBuf, Vec<PathBuf>) {
        let dir = std::env::temp_dir().join(format!(
            "arbos-staged-{}-{}",
            std::process::id(),
            crate::model::now_ms()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let wide = dir.join("profile-card.png");
        image::RgbaImage::from_pixel(300, 160, image::Rgba([40, 120, 220, 255]))
            .save(&wide)
            .expect("wide png");
        let tall = dir.join("portrait.png");
        image::RgbaImage::from_pixel(120, 260, image::Rgba([220, 80, 40, 255]))
            .save(&tall)
            .expect("tall png");
        let doc = dir.join("notes.txt");
        std::fs::write(&doc, "hello").expect("txt");
        (dir, vec![wide, tall, doc])
    }

    fn mount(
        cx: &mut TestAppContext,
    ) -> (
        tokio::runtime::Runtime,
        Entity<Shell>,
        Entity<Composer>,
        &mut VisualTestContext,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        let now = crate::model::now_ms();
        state.update(cx, |s, cx| {
            s.projects.push(Project {
                id: "p1".into(),
                name: "Arbonites".into(),
                kind: SpaceKind::Group,
                ..Project::default()
            });
            let chat: Chat = serde_json::from_value(serde_json::json!({
                "id": "c1", "project_id": "p1", "title": "Arbonites", "is_pinned": true,
                "is_system": false, "unread": false, "last_message_at": now,
            }))
            .expect("chat");
            s.chats.insert("p1".into(), vec![chat]);
            s.selected_project = Some("p1".into());
            s.active_tab.insert("p1".into(), "c1".into());
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.run_until_parked();
        let composer = shell.read_with(&*vcx, |sh, _| sh.composer.clone());
        vcx.update(|window, cx| Composer::focus(&composer, window, cx));
        settle(&shell, vcx);
        (rt, shell, composer, vcx)
    }

    fn settle(shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        for _ in 0..3 {
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    fn stage(composer: &Entity<Composer>, paths: &[PathBuf], vcx: &mut VisualTestContext) {
        composer.update(vcx, |c, cx| c.add_paths(paths.iter().cloned(), cx));
    }

    fn staged(composer: &Entity<Composer>, vcx: &VisualTestContext) -> Vec<PathBuf> {
        composer.read_with(vcx, |c, _| c.staged_paths())
    }

    fn text(composer: &Entity<Composer>, vcx: &VisualTestContext) -> String {
        composer.read_with(vcx, |c, cx| c.input.read(cx).text().to_string())
    }

    fn centre(b: Bounds<Pixels>) -> gpui::Point<Pixels> {
        point(b.left() + b.size.width / 2.0, b.top() + b.size.height / 2.0)
    }

    fn f(p: Pixels) -> f32 {
        p.into()
    }

    #[gpui::test]
    fn removing_one_attachment_keeps_the_others_and_the_text(cx: &mut TestAppContext) {
        let (_rt, shell, composer, vcx) = mount(cx);
        let (_dir, paths) = fixtures();
        stage(&composer, &paths, vcx);
        vcx.simulate_input("see the card");
        settle(&shell, vcx);
        assert_eq!(staged(&composer, vcx), paths);

        let disc = vcx
            .debug_bounds("composer-remove-attachment-1")
            .expect("the middle tile's remove disc is drawn without hovering");
        vcx.simulate_click(centre(disc), Modifiers::default());
        settle(&shell, vcx);

        assert_eq!(
            staged(&composer, vcx),
            vec![paths[0].clone(), paths[2].clone()],
            "only the clicked one goes"
        );
        assert_eq!(
            text(&composer, vcx),
            "see the card",
            "the draft is untouched"
        );

        // The discs re-index: the former third tile is now the second.
        let disc = vcx
            .debug_bounds("composer-remove-attachment-1")
            .expect("second disc");
        vcx.simulate_click(centre(disc), Modifiers::default());
        settle(&shell, vcx);
        assert_eq!(staged(&composer, vcx), vec![paths[0].clone()]);
        assert_eq!(text(&composer, vcx), "see the card");
    }

    #[gpui::test]
    fn backspace_on_an_empty_draft_removes_the_last_attachment(cx: &mut TestAppContext) {
        let (_rt, shell, composer, vcx) = mount(cx);
        let (_dir, paths) = fixtures();
        stage(&composer, &paths, vcx);
        settle(&shell, vcx);

        vcx.dispatch_action(Backspace);
        settle(&shell, vcx);
        assert_eq!(
            staged(&composer, vcx),
            paths[..2].to_vec(),
            "⌫ on an empty draft takes the last attachment"
        );

        vcx.dispatch_action(Delete);
        settle(&shell, vcx);
        assert_eq!(staged(&composer, vcx), paths[..1].to_vec(), "⌦ too");

        // With text in the draft the keys edit the text, never the strip —
        // even with the caret at the start where there is nothing to delete.
        vcx.simulate_input("hi");
        vcx.dispatch_action(Backspace);
        settle(&shell, vcx);
        assert_eq!(text(&composer, vcx), "h");
        assert_eq!(staged(&composer, vcx), paths[..1].to_vec());
        vcx.dispatch_action(Home);
        vcx.dispatch_action(Backspace);
        settle(&shell, vcx);
        assert_eq!(
            text(&composer, vcx),
            "h",
            "⌫ at the start of text is a no-op"
        );
        assert_eq!(
            staged(&composer, vcx),
            paths[..1].to_vec(),
            "a non-empty draft never loses an attachment to ⌫"
        );

        // Empty the draft: the next ⌫ takes the last tile; one more does nothing.
        vcx.dispatch_action(End);
        vcx.dispatch_action(Backspace);
        settle(&shell, vcx);
        assert_eq!(text(&composer, vcx), "");
        assert_eq!(staged(&composer, vcx), paths[..1].to_vec());
        vcx.dispatch_action(Backspace);
        settle(&shell, vcx);
        assert!(staged(&composer, vcx).is_empty());
        vcx.dispatch_action(Backspace);
        settle(&shell, vcx);
        assert!(staged(&composer, vcx).is_empty());
        assert_eq!(text(&composer, vcx), "");
    }

    #[gpui::test]
    fn the_strip_lays_the_tiles_out_as_pinned(cx: &mut TestAppContext) {
        let (_rt, shell, composer, vcx) = mount(cx);
        let (_dir, paths) = fixtures();
        stage(&composer, &paths, vcx);
        settle(&shell, vcx);

        let strip = vcx
            .debug_bounds("composer-staged-strip")
            .expect("the strip is drawn");
        let tiles: Vec<Bounds<Pixels>> = [
            "composer-staged-tile-0",
            "composer-staged-tile-1",
            "composer-staged-tile-2",
        ]
        .into_iter()
        .map(|s| vcx.debug_bounds(s).expect(s))
        .collect();
        let discs: Vec<Bounds<Pixels>> = [
            "composer-remove-attachment-0",
            "composer-remove-attachment-1",
            "composer-remove-attachment-2",
        ]
        .into_iter()
        .map(|s| vcx.debug_bounds(s).expect(s))
        .collect();

        for (i, tile) in tiles.iter().enumerate() {
            assert_eq!(f(tile.size.height), STAGED_TILE, "tile {i} is 64 tall");
            if i < 2 {
                assert_eq!(
                    f(tile.size.width),
                    STAGED_TILE,
                    "picture tile {i} is square"
                );
            } else {
                assert!(
                    f(tile.size.width) > STAGED_TILE,
                    "the file chip is wider than a picture tile"
                );
            }
            // One row, tops level, nothing outside the strip (nothing to clip).
            assert_eq!(tile.top(), tiles[0].top(), "tile {i} shares the row");
            assert!(tile.left() >= strip.left() && tile.right() <= strip.right());
            assert!(tile.top() >= strip.top() && tile.bottom() <= strip.bottom());
            // The remove disc: 20 across, inset 4 from the tile's top-right,
            // fully inside its own tile.
            let disc = discs[i];
            assert_eq!(f(disc.size.width), STAGED_REMOVE);
            assert_eq!(f(disc.size.height), STAGED_REMOVE);
            assert_eq!(f(tile.right() - disc.right()), STAGED_REMOVE_INSET);
            assert_eq!(f(disc.top() - tile.top()), STAGED_REMOVE_INSET);
            assert!(disc.left() > tile.left() && disc.bottom() < tile.bottom());
        }
        for w in tiles.windows(2) {
            assert_eq!(f(w[1].left() - w[0].right()), STAGED_GAP, "8 between tiles");
        }
        for (disc, next) in discs.iter().zip(tiles.iter().skip(1)) {
            assert!(
                disc.right() < next.left(),
                "a disc never reaches its neighbour"
            );
        }
    }
}
