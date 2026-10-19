//! The composer, Grok Bot layout: a "+" circle for attachments, the pill
//! field ("Message <name>") with the hold-to-talk mic (transcribe), and the
//! round white button that sends text, Guides a running turn, stops when the
//! field is empty mid-turn, or starts a call when empty — same as the iPhone
//! (mic + phone; calls start audio-only). Hand-rolled multi-line text input
//! with IME (`EntityInputHandler`, zeron-study §4.6). Enter sends (or Guides
//! mid-task), Shift+Enter inserts a newline. The model and reasoning pickers
//! open from the header's "…" menu.

use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    actions, div, fill, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem, Context,
    CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    ExternalPaths, FocusHandle, Focusable, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    Render, SharedString, Style, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window,
    WrappedLine,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::app::{AppState, ComposerMode};
use crate::model::{CallKind, Effort};
use crate::sync::fixture::MODELS;
use crate::ui::callview;
use crate::ui::frost::{frosted, MENU_BLUR, PANEL_BLUR};
use crate::ui::glass;
use crate::ui::icons::icon;
use crate::ui::theme::{
    Theme, COMPOSER_HEIGHT, LINE_HEIGHT, PANEL_RADIUS, PLUS_BUTTON, SEND_BUTTON, TEXT_BODY,
    TEXT_TINY,
};
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
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        Paste,
        Copy,
        Cut,
        Escape,
        AcceptMention,
    ]
);

pub const MAX_LINES: usize = 10;
pub const COMPOSER_RADIUS: f32 = COMPOSER_HEIGHT / 2.0;

pub type SubmitHandler = Rc<dyn Fn(&mut Window, &mut App)>;
const INPUT_LINE_HEIGHT: f32 = LINE_HEIGHT;
const INPUT_FONT_SIZE: f32 = TEXT_BODY;

#[derive(Clone, Debug)]
pub enum ComposerEvent {
    Send {
        text: String,
        attachments: Vec<PathBuf>,
        reply_to: Option<String>,
    },
    /// Inject `text` into the agent's currently running turn (Guide).
    Guide {
        text: String,
        reply_to: Option<String>,
    },
    Stop,
}

struct Layout {
    lines: Vec<(usize, WrappedLine)>,
    bounds: Bounds<Pixels>,
}

pub struct ComposerInput {
    focus_handle: FocusHandle,
    content: String,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    layout: Option<Layout>,
    is_selecting: bool,
    /// Vertical scroll inside the box once it reaches `MAX_LINES`.
    scroll_y: f32,
    /// Runs deferred, after this entity's update ends, so the owner may read
    /// and update the input freely.
    pub on_submit: Option<SubmitHandler>,
    /// Tab with an `@` token under the caret (D72 mention completion).
    pub on_tab: Option<SubmitHandler>,
    /// Multi-line editors (skill bodies): Enter inserts a newline instead of
    /// submitting.
    pub enter_inserts_newline: bool,
    /// Password fields: paint `*` per byte so caret offsets still line up with `content`.
    masked: bool,
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
            layout: None,
            is_selecting: false,
            scroll_y: 0.0,
            on_submit: None,
            on_tab: None,
            enter_inserts_newline: false,
            masked: false,
        }
    }

    pub fn set_masked(&mut self, masked: bool) {
        self.masked = masked;
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_placeholder(&mut self, text: impl Into<SharedString>) {
        self.placeholder = text.into();
    }

    pub fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        let len = text.len();
        self.content = text;
        self.selected_range = len..len;
        self.selection_reversed = false;
        self.marked_range = None;
        self.scroll_y = 0.0;
        cx.notify();
    }

    /// Like `set_text`, but shows the text from its first line (form fields).
    pub fn set_text_from_start(&mut self, text: String, cx: &mut Context<Self>) {
        self.set_text(text, cx);
        self.selected_range = 0..0;
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.content.clear();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.scroll_y = 0.0;
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
                    px(INPUT_FONT_SIZE),
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
        if let Some(cb) = self.on_submit.clone() {
            window.defer(cx, move |window, cx| cb(window, cx));
        }
    }

    fn accept_mention(&mut self, _: &AcceptMention, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(cb) = self.on_tab.clone() {
            window.defer(cx, move |window, cx| cb(window, cx));
        }
    }

    /// Replace the trailing `@token` with `@handle ` (mention completion).
    pub fn complete_mention(&mut self, handle: &str, cx: &mut Context<Self>) {
        let Some(at) = self.content.rfind('@') else {
            return;
        };
        if self.content[at + 1..].contains(char::is_whitespace) {
            return;
        }
        self.content.truncate(at);
        self.content.push('@');
        self.content.push_str(handle);
        self.content.push(' ');
        let len = self.content.len();
        self.selected_range = len..len;
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

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.offset_for_vertical_move(true), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
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

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_start(self.cursor_offset()), cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_end(self.cursor_offset()), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_start(self.cursor_offset()), cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_end(self.cursor_offset()), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                return;
            }
            self.select_to(prev, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                return;
            }
            self.select_to(next, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
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
        index_for_position(&layout.lines, layout.bounds, position, self.content.len())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
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

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    fn line_start(&self, offset: usize) -> usize {
        self.content[..offset]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0)
    }

    fn line_end(&self, offset: usize) -> usize {
        self.content[offset..]
            .find('\n')
            .map(|i| offset + i)
            .unwrap_or(self.content.len())
    }

    /// Byte offset one wrapped line above/below the caret, keeping column when
    /// the layout is known; falls back to hard newlines before the first paint.
    fn offset_for_vertical_move(&self, up: bool) -> usize {
        let cursor = self.cursor_offset();
        let Some(layout) = self.layout.as_ref() else {
            return self.offset_for_hard_line_move(up);
        };
        if layout.lines.is_empty() {
            return cursor;
        }
        let Some(pos) = position_for_index(&layout.lines, cursor) else {
            return self.offset_for_hard_line_move(up);
        };
        let lh = px(INPUT_LINE_HEIGHT);
        let target_y = if up { pos.y - lh } else { pos.y + lh };
        let window_pos = point(layout.bounds.left() + pos.x, layout.bounds.top() + target_y);
        index_for_position(&layout.lines, layout.bounds, window_pos, self.content.len())
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
        position_for_index(&layout.lines, index)
    }
}

/// Pixel position (relative to the text origin) of a byte index.
fn position_for_index(lines: &[(usize, WrappedLine)], index: usize) -> Option<Point<Pixels>> {
    let lh = px(INPUT_LINE_HEIGHT);
    let mut y = px(0.0);
    for (start, line) in lines {
        let end = start + line.len();
        if index >= *start && index <= end {
            let p = line.position_for_index(index - start, lh)?;
            return Some(point(p.x, p.y + y));
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
            let local = point(position.x - bounds.left(), (position.y - y).max(px(0.0)));
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
        self.content =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
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
        self.content =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
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
    scroll_y: f32,
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
        let mut start = 0usize;
        for para in display_text.split('\n') {
            let runs = runs_for(para, start, marked.as_ref(), &base_run);
            let shaped = window
                .text_system()
                .shape_text(
                    SharedString::from(para.to_string()),
                    px(INPUT_FONT_SIZE),
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
            Some(point(px(0.0), px(0.0)))
        } else {
            position_for_index(&lines, cursor)
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
            let a = position_for_index(&lines, selected_range.start);
            let b = position_for_index(&lines, selected_range.end);
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
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for sel in prepaint.selections.drain(..) {
                window.paint_quad(sel);
            }
            let mut y = bounds.top() - px(scroll_y);
            for (_, line) in &prepaint.lines {
                let h = line.size(lh).height;
                if y + h >= bounds.top() && y <= bounds.bottom() {
                    let _ = line.paint(
                        point(bounds.left(), y),
                        lh,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
                y += h;
            }
            if focused {
                if let Some(cursor) = prepaint.cursor.take() {
                    window.paint_quad(cursor);
                }
            }
        });
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
            input.layout = Some(Layout { lines, bounds });
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
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .line_height(px(INPUT_LINE_HEIGHT))
            .text_size(px(INPUT_FONT_SIZE))
            .child(InputElement { input: cx.entity() })
    }
}

impl Focusable for ComposerInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picker {
    Model,
    Effort,
}

pub struct Composer {
    state: Entity<AppState>,
    pub input: Entity<ComposerInput>,
    /// F11: mic button, voice mode, spoken replies.
    voice: Entity<Voice>,
    attachments: Vec<PathBuf>,
    /// Which chat the input text belongs to; drafts are kept per chat.
    draft_chat: Option<crate::model::Id>,
    drafts: std::collections::HashMap<crate::model::Id, String>,
    /// The model / effort menu, opened from the header's "…" menu.
    pub open_picker: Option<Picker>,
    /// S8 shake when the user hits Cmd+W on the pinned tab; also reused for
    /// the empty-send nudge.
    pub nudge_at: Option<u64>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    pub fn new(state: Entity<AppState>, voice: Entity<Voice>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(ComposerInput::new);
        let weak = cx.weak_entity();
        let weak_tab = weak.clone();
        input.update(cx, |input, _| {
            input.on_submit = Some(Rc::new(move |window, cx| {
                let _ = weak.update(cx, |composer, cx| composer.submit(window, cx));
            }));
            input.on_tab = Some(Rc::new(move |_window, cx| {
                let _ = weak_tab.update(cx, |composer, cx| composer.accept_first_mention(cx));
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
                    let existing = input.text().trim_end().to_string();
                    let next = if existing.is_empty() {
                        text
                    } else {
                        format!("{existing} {text}")
                    };
                    input.set_text(next, cx);
                });
                cx.notify();
            }
        })
        .detach();
        Self {
            state,
            input,
            voice,
            attachments: Vec::new(),
            draft_chat: None,
            drafts: std::collections::HashMap::new(),
            open_picker: None,
            nudge_at: None,
        }
    }

    /// Swap the draft when the active chat changes.
    fn sync_draft(&mut self, cx: &mut Context<Self>) {
        let active = self.state.read(cx).active_chat_id();
        if active == self.draft_chat {
            return;
        }
        let text = self.input.read(cx).text().to_string();
        if let Some(prev) = self.draft_chat.take() {
            if text.is_empty() {
                self.drafts.remove(&prev);
            } else {
                self.drafts.insert(prev, text);
            }
        }
        let next = active
            .as_ref()
            .and_then(|id| self.drafts.get(id).cloned())
            .unwrap_or_default();
        self.input.update(cx, |input, cx| input.set_text(next, cx));
        self.draft_chat = active;
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
                let text = self.input.read(cx).text().trim().to_string();
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
                let attachments = std::mem::take(&mut self.attachments);
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
                    cx.emit(ComposerEvent::Guide { text, reply_to });
                } else {
                    cx.emit(ComposerEvent::Send {
                        text,
                        attachments,
                        reply_to,
                    });
                }
            }
        }
        self.open_picker = None;
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

    /// Paths from the "+" picker or a drop onto the chat box.
    fn add_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            if path.is_file() {
                self.attachments.push(path);
            }
        }
        cx.notify();
    }

    /// The `@token` being typed at the end of the input, if any.
    fn mention_query(&self, cx: &App) -> Option<String> {
        let text = self.input.read(cx).text();
        let at = text.rfind('@')?;
        let token = &text[at + 1..];
        if token.contains(char::is_whitespace) {
            return None;
        }
        // Only at a word start: "@" preceded by nothing or whitespace.
        if at > 0 && !text[..at].ends_with(char::is_whitespace) {
            return None;
        }
        Some(token.to_lowercase())
    }

    /// People and agents of the current space matching the `@token`:
    /// (handle, label, is_agent).
    fn mention_candidates(&self, cx: &App) -> Vec<(String, String, bool)> {
        let Some(query) = self.mention_query(cx) else {
            return Vec::new();
        };
        let state = self.state.read(cx);
        let Some(space) = state.selected() else {
            return Vec::new();
        };
        if !space.is_people() && space.people.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<(String, String, bool)> = Vec::new();
        for a in &space.agents {
            if a.handle.starts_with(&query) {
                out.push((
                    a.handle.clone(),
                    format!("{} · owned by @{}", a.name, a.owner_handle),
                    true,
                ));
            }
        }
        for p in &space.people {
            if p.handle.to_lowercase().starts_with(&query) {
                out.push((p.handle.clone(), p.name.clone().unwrap_or_default(), false));
            }
        }
        if space.agents.len() == 1
            && "arbos".starts_with(&query)
            && !out.iter().any(|(h, _, _)| h == "arbos")
        {
            out.push(("arbos".into(), "the agent in this chat".into(), true));
        }
        out.truncate(6);
        out
    }

    fn accept_first_mention(&mut self, cx: &mut Context<Self>) {
        if let Some((handle, _, _)) = self.mention_candidates(cx).into_iter().next() {
            self.input
                .update(cx, |input, cx| input.complete_mention(&handle, cx));
            cx.notify();
        }
    }

    fn render_mentions(
        &self,
        candidates: Vec<(String, String, bool)>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let mut menu = div()
            .id("mention-menu")
            .flex()
            .flex_col()
            .min_w(px(260.0))
            .p(px(6.0))
            .rounded(px(PANEL_RADIUS))
            .bg(theme.card())
            .border_1()
            .border_color(theme.border_strong)
            .shadow_lg();
        for (i, (handle, label, is_agent)) in candidates.into_iter().enumerate() {
            let h = handle.clone();
            menu = menu.child(
                div()
                    .id(ElementId::NamedInteger("mention-item".into(), i as u64))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .when(i == 0, |el| el.bg(theme.surface_2))
                    .hover(|s| s.bg(theme.surface_2))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(theme.text)
                            .child(format!("@{handle}")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.0))
                            .text_color(theme.text_faint)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme.text_faint)
                            .child(if is_agent { "agent" } else { "person" }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.input
                            .update(cx, |input, cx| input.complete_mention(&h, cx));
                        cx.notify();
                    })),
            );
        }
        menu = menu.child(
            div()
                .px(px(8.0))
                .pt(px(2.0))
                .text_size(px(10.5))
                .text_color(theme.text_faint)
                .child("Tab completes the first match"),
        );
        div()
            .absolute()
            .bottom(px(COMPOSER_HEIGHT + 12.0))
            .left(px(8.0))
            .child(frosted(PANEL_RADIUS, PANEL_BLUR, menu))
    }

    fn render_picker_menu(&self, picker: Picker, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let state = self.state.read(cx);
        let (model, effort) = state
            .selected()
            .map(|p| (p.model.clone(), p.effort))
            .unwrap_or((MODELS[0].to_string(), Effort::High));
        let mut menu = div()
            .id("picker-menu")
            .flex()
            .flex_col()
            .min_w(px(220.0))
            .max_h(px(320.0))
            .overflow_y_scroll()
            .p(px(4.0))
            .rounded(px(PANEL_RADIUS))
            .bg(theme.card())
            .border_1()
            .border_color(theme.border_strong)
            .shadow_lg();
        match picker {
            Picker::Model => {
                let mut names: Vec<String> = if state.models.is_empty() {
                    MODELS.iter().map(|m| m.to_string()).collect()
                } else {
                    state.models.clone()
                };
                if !names.contains(&model) {
                    names.insert(0, model.clone());
                }
                for (i, name) in names.iter().enumerate() {
                    let selected = *name == model;
                    let name_owned = name.to_string();
                    menu = menu.child(
                        div()
                            .id(ElementId::NamedInteger("model-item".into(), i as u64))
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(6.0))
                            .text_size(px(13.0))
                            .text_color(theme.text)
                            .hover(|s| s.bg(theme.surface_2))
                            .cursor_pointer()
                            .child(name.clone())
                            .when(selected, |el| {
                                el.child(div().text_color(theme.accent).child("✓"))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state
                                    .update(cx, |s, cx| s.set_model(name_owned.clone(), cx));
                                this.open_picker = None;
                                cx.notify();
                            })),
                    );
                }
            }
            Picker::Effort => {
                for (i, level) in Effort::ALL.iter().enumerate() {
                    let selected = *level == effort;
                    let level = *level;
                    menu = menu.child(
                        div()
                            .id(ElementId::NamedInteger("effort-item".into(), i as u64))
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(6.0))
                            .text_size(px(13.0))
                            .text_color(theme.text)
                            .hover(|s| s.bg(theme.surface_2))
                            .cursor_pointer()
                            .child(level.label())
                            .when(selected, |el| {
                                el.child(div().text_color(theme.accent).child("✓"))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |s, cx| s.set_effort(level, cx));
                                this.open_picker = None;
                                cx.notify();
                            })),
                    );
                }
            }
        }
        // Opens upward, above the field.
        div()
            .absolute()
            .bottom(px(COMPOSER_HEIGHT + 12.0))
            .right(px(0.0))
            .child(frosted(PANEL_RADIUS, MENU_BLUR, menu))
    }
}

impl Render for Composer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_draft(cx);
        let theme = Theme::of(cx).clone();
        let mode = self.mode(cx);
        let state = self.state.read(cx);
        let space = state.selected().cloned();
        // A people chat has no model of its own: its agents run on their owners' settings.
        let link_full = state.link == crate::model::Link::Reconnecting;
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
                    p.person()
                        .map(|x| format!("@{}", x.handle))
                        .unwrap_or_else(|| p.name.clone())
                } else {
                    p.name.clone()
                }
            })
            .unwrap_or_else(|| "Arbos".into());
        let (recording, transcribing, in_call, voice_notice) = {
            let v = self.voice.read(cx);
            (
                v.is_recording(),
                v.is_transcribing(),
                v.in_call(),
                v.notice().map(str::to_owned),
            )
        };
        let in_livekit = state.my_call.is_some();
        let has_text = !self.input.read(cx).text().trim().is_empty();

        self.input.update(cx, |input, _| {
            input.set_placeholder(match mode {
                ComposerMode::Answer => format!("Answer {name}"),
                ComposerMode::Approve => "Approve or reject the card above".to_string(),
                ComposerMode::Send if transcribing => "Transcribing…".to_string(),
                ComposerMode::Send if voice_mode => "Speak or type…".to_string(),
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
            ComposerMode::Answer | ComposerMode::Approve => Theme::with_alpha(theme.warn, 0.7),
            ComposerMode::Send => {
                if nudge {
                    theme.danger
                } else {
                    theme.field_border
                }
            }
        };

        // The round primary button: Guide while a turn is running and the field
        // has text; otherwise send, stop when empty mid-turn, or start / end a
        // call when idle and empty (iPhone: phone next to the mic; calls start
        // audio-only).
        let hanging_up = in_call || in_livekit;
        let guide = running && has_text && self.attachments.is_empty();
        let (glyph, action): (&str, ButtonAction) = if guide {
            ("", ButtonAction::Guide)
        } else if has_text {
            ("arrow-up", ButtonAction::Send)
        } else if running {
            ("stop", ButtonAction::Stop)
        } else {
            ("phone", ButtonAction::Call)
        };
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
                .bg(if action == ButtonAction::Call && hanging_up {
                    theme.danger
                } else {
                    theme.primary
                })
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.opacity(0.88))
                .child(icon(
                    glyph,
                    if glyph == "stop" { 14.0 } else { 16.0 },
                    if action == ButtonAction::Call && hanging_up {
                        theme.text
                    } else {
                        theme.on_primary
                    },
                ))
                .on_click(cx.listener(move |this, _, window, cx| match action {
                    ButtonAction::Send | ButtonAction::Guide => this.submit(window, cx),
                    ButtonAction::Stop => cx.emit(ComposerEvent::Stop),
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
        // Grok Mac's filled round mic (black on light, white on dark).
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
            } else if transcribing {
                theme.surface_2
            } else {
                theme.primary
            })
            .hover(|s| s.opacity(0.88))
            .child(icon(
                if recording { "stop" } else { "mic" },
                16.0,
                if recording {
                    theme.danger
                } else if transcribing {
                    theme.text_faint
                } else {
                    theme.on_primary
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
            .bg(theme.surface_2)
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(theme.border_strong))
            .child(icon("plus", 16.0, theme.text_muted))
            .on_click(cx.listener(|this, _, _, cx| this.attach(cx)));

        let field_body = if recording {
            div()
                .flex_1()
                .min_w_0()
                .h(px(COMPOSER_HEIGHT - 2.0))
                .flex()
                .items_center()
                .pl(px(10.0))
                .child(render_recording_row(self.voice.read(cx), &theme))
        } else {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .pl(px(10.0))
                .py(px((COMPOSER_HEIGHT - 2.0 - INPUT_LINE_HEIGHT) / 2.0))
                .child(self.input.clone())
        };

        let drop_border = theme.accent;
        let drop_fill = theme.accent_soft;
        let mut field = div()
            .id("composer-field")
            .flex_1()
            .min_w_0()
            .min_h(px(COMPOSER_HEIGHT))
            .flex()
            .items_end()
            .gap(px(4.0))
            .pl(px((COMPOSER_HEIGHT - 2.0 - PLUS_BUTTON) / 2.0))
            .pr(px((COMPOSER_HEIGHT - 2.0 - SEND_BUTTON) / 2.0))
            .rounded(px(COMPOSER_RADIUS))
            .bg(theme.field)
            .border_1()
            .border_color(border_color)
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.border_color(drop_border).bg(drop_fill)
            })
            .child(
                div()
                    .h(px(COMPOSER_HEIGHT - 2.0))
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .child(plus),
            )
            .child(field_body);
        // Match the iPhone: mic (transcribe) + phone (call) while the field is
        // idle and empty; send / stop / hang-up replace the phone as needed.
        let mut trailing = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .h(px(COMPOSER_HEIGHT - 2.0))
            .flex_shrink_0();
        if !has_text && !recording {
            trailing = trailing.child(mic_button);
        }
        // Always show the round button: phone when idle (like the iPhone), else
        // send / stop / hang-up.
        trailing = trailing.child(primary_button);
        field = field.child(trailing);

        let mut column = div()
            .id("composer")
            .relative()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .can_drop(|dragged, _, _| dragged.is::<ExternalPaths>())
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.add_paths(paths.paths().iter().cloned(), cx);
            }));

        // Reply chip (iPhone parity): set by the hover reply icon on a message.
        if let Some(chat_id) = self.state.read(cx).active_chat_id() {
            let reply_id = self
                .state
                .read(cx)
                .chat_ui
                .get(&chat_id)
                .and_then(|ui| ui.reply_to.clone());
            if let Some(reply_id) = reply_id {
                let (who, preview) =
                    self.state
                        .read(cx)
                        .messages
                        .get(&chat_id)
                        .and_then(|list| list.iter().find(|m| m.id == reply_id))
                        .map(|m| {
                            let who =
                                m.sender_handle
                                    .as_deref()
                                    .map(|h| format!("@{h}"))
                                    .unwrap_or_else(|| match m.role {
                                        crate::model::Role::Assistant => "agent".into(),
                                        crate::model::Role::User => "you".into(),
                                        crate::model::Role::System
                                        | crate::model::Role::Project => "message".into(),
                                    });
                            let mut preview = m.plain_text();
                            if preview.chars().count() > 72 {
                                preview = preview.chars().take(72).collect::<String>() + "…";
                            }
                            (who, preview)
                        })
                        .unwrap_or_else(|| ("message".into(), String::new()));
                let clear_chat = chat_id.clone();
                column = column.child(
                    div()
                        .id("composer-reply")
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(14.0))
                        .py(px(8.0))
                        .ml(px(PLUS_BUTTON + 12.0))
                        .rounded(px(14.0))
                        .bg(theme.bubble_assistant)
                        .child(icon("reply", 12.0, theme.text_muted))
                        .child(
                            div()
                                .text_size(px(TEXT_TINY))
                                .text_color(theme.text_muted)
                                .child(format!("Replying to {who}")),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(TEXT_TINY))
                                .text_color(theme.text)
                                .truncate()
                                .child(preview),
                        )
                        .child(
                            div()
                                .id("composer-reply-clear")
                                .size(px(18.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.surface_2))
                                .child(icon("xmark", 10.0, theme.text_muted))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.state
                                        .update(cx, |s, cx| s.clear_reply_to(&clear_chat, cx));
                                })),
                        ),
                );
            }
        }

        if !self.attachments.is_empty() {
            let mut chips = div().flex().flex_wrap().gap(px(6.0)).pl(px(8.0));
            for (i, path) in self.attachments.iter().enumerate() {
                let name_owned = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("attachment")
                    .to_string();
                let is_image = matches!(
                    path.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.to_lowercase())
                        .as_deref(),
                    Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "heic")
                );
                chips = chips.child(
                    div()
                        .id(ElementId::NamedInteger("attachment".into(), i as u64))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded_full()
                        .bg(theme.bubble_assistant)
                        .text_size(px(TEXT_TINY))
                        .text_color(theme.text)
                        .cursor_pointer()
                        .child(icon(
                            if is_image { "photo" } else { "doc" },
                            12.0,
                            theme.text,
                        ))
                        .child(div().max_w(px(220.0)).truncate().child(name_owned))
                        .child(icon("xmark", 9.0, theme.text_muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if i < this.attachments.len() {
                                this.attachments.remove(i);
                            }
                            cx.notify();
                        })),
                );
            }
            column = column.child(chips);
        }

        column = column.child(frosted(COMPOSER_RADIUS, PANEL_BLUR, field));

        // One muted line about the voice path while it is doing something (or
        // when it had to fall back: no microphone, mock relay), or the link.
        let note = voice_notice
            .filter(|_| recording || transcribing || voice_mode)
            .or_else(|| {
                link_full.then(|| {
                    "Reconnecting… messages queue locally and sync when the relay is back."
                        .to_string()
                })
            });
        if let Some(note) = note {
            column = column.child(
                div()
                    .pl(px(16.0))
                    .text_size(px(TEXT_TINY))
                    .text_color(theme.text_muted)
                    .child(note),
            );
        }

        if let Some(picker) = self.open_picker {
            column = column.child(glass::popover(self.render_picker_menu(picker, cx)));
        }
        let mentions = self.mention_candidates(cx);
        if !mentions.is_empty() {
            column = column.child(glass::popover(self.render_mentions(mentions, cx)));
        }
        column
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonAction {
    Send,
    Guide,
    Stop,
    Call,
}
