//! Text selection for the chat transcript: drag, double-click (word),
//! triple-click (paragraph), shift-click, ⌘C. One selection range per
//! **bubble**: byte offsets into the bubble's rendered plain text, which runs
//! through every paragraph, list, table and code block of the message. Each
//! text element knows the bubble it belongs to and where its own text starts
//! in that plain text, so a drag that leaves one paragraph continues in the
//! next (#1389).
//!
//! [`TextSelection`] is the model (an entity the transcript observes);
//! [`Selectable::bubble`] scopes it to one bubble; [`SelectableText`] wraps a
//! [`StyledText`] and wires the mouse to the model from inside its own paint,
//! where the text layout is guaranteed to exist.

use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    linear_color_stop, linear_gradient, point, px, size, App, Bounds, Context, Corners,
    CursorStyle, DispatchPhase, Element, ElementId, Entity, FocusHandle, FontWeight,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString,
    StyledText, TextAlign, TextLayout, TextRun, TransformationMatrix, Window,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::ui::avatars;
use crate::ui::mention::{self, Mention, Mentions};

/// Ignore pointer noise until the drag travels this far before choosing
/// text selection vs. vertical scroll (same idea as reply-swipe slop).
const SELECTION_DRAG_SLOP: f32 = 12.0;

/// What one click, two clicks or three clicks select, and what a drag that
/// started with them extends by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Granularity {
    Character,
    Word,
    Paragraph,
}

/// How far the pointer is from a text element: vertical first, then
/// horizontal, both 0 when the pointer is inside it. While dragging, every
/// element of the bubble proposes the index nearest to the pointer and the
/// smallest distance wins, so the gap between two paragraphs (or two table
/// cells) still extends the selection.
pub type Distance = (f32, f32);

#[derive(Clone, Debug)]
struct Active {
    key: SharedString,
    /// The bubble's plain text when the selection was made; copy reads from
    /// this so a streaming bubble cannot shift the offsets under the user.
    text: SharedString,
    /// The fixed end: the pressed index, or the pressed word / paragraph.
    anchor: Range<usize>,
    /// The moving end, already snapped to the granularity.
    head: Range<usize>,
    granularity: Granularity,
    dragging: bool,
}

impl Active {
    fn range(&self) -> Range<usize> {
        self.anchor.start.min(self.head.start)..self.anchor.end.max(self.head.end)
    }
}

/// The transcript's one selection. Holds the focus handle that receives ⌘C:
/// a non-empty selection takes keyboard focus (AppKit behaviour); an empty
/// one leaves it where it was, so a stray click never strands the composer.
pub struct TextSelection {
    pub focus_handle: FocusHandle,
    model: Model,
    /// The pointer over the transcript's mention chips (hover / pressed), by
    /// bubble key and the chip's byte range in that bubble's plain text.
    pub chips: mention::ChipPointer<ChipKey>,
    /// The cursor the last painted bubble text asked for (tests).
    #[cfg(test)]
    pub(crate) cursor: CursorStyle,
}

/// Names one chip across frames: the bubble and the chip's range in the
/// bubble's plain text.
pub type ChipKey = (SharedString, Range<usize>);

impl TextSelection {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            model: Model::default(),
            chips: mention::ChipPointer::default(),
            #[cfg(test)]
            cursor: CursorStyle::Arrow,
        }
    }

    /// The selected text, if anything is selected.
    pub fn selected_text(&self) -> Option<String> {
        self.model.selected_text()
    }

    /// Drop a settled (not dragging) selection; `true` when something changed.
    pub fn clear_settled(&mut self) -> bool {
        self.model.clear_settled()
    }

    /// Drop any selection (drag or settled). Used when a row gesture
    /// (swipe-right-to-reply) takes over the pointer.
    pub fn clear(&mut self) {
        self.model.active = None;
        self.model.proposal = None;
    }

    /// `true` while a drag has committed to a non-empty text range (so the
    /// transcript must not steal the pointer for drag-scroll).
    pub fn is_selecting(&self) -> bool {
        self.model
            .active
            .as_ref()
            .is_some_and(|a| a.dragging && !a.range().is_empty())
    }

    /// Screenshot rig (`ARBOS_RIG_SELECT`): select part of bubble `key` as a
    /// gesture would — `all`, a drag `<from>..<to>` (byte offsets of `text`),
    /// a double-click `word@<i>` or a triple-click `para@<i>`. The GUI
    /// session of the EC2 Mac drops posted pointer events, so the rig asks.
    /// `false` when the spec does not parse.
    pub fn rig_select(&mut self, key: SharedString, text: SharedString, spec: &str) -> bool {
        let spec = spec.trim();
        let (index, clicks, to) = if spec == "all" {
            (0, 1, Some(text.len()))
        } else if let Some((from, to)) = spec.split_once("..") {
            let (Ok(from), Ok(to)) = (from.trim().parse(), to.trim().parse()) else {
                return false;
            };
            (from, 1, Some(to))
        } else if let Some(i) = spec.strip_prefix("word@") {
            let Ok(i) = i.trim().parse() else {
                return false;
            };
            (i, 2, None)
        } else if let Some(i) = spec.strip_prefix("para@") {
            let Ok(i) = i.trim().parse() else {
                return false;
            };
            (i, 3, None)
        } else {
            return false;
        };
        self.model.begin(key, text, index, clicks, false);
        if let Some(to) = to {
            self.model.extend_to(to);
        }
        self.model.finish_drag();
        true
    }

    #[cfg(test)]
    pub(crate) fn model_for_tests(&mut self) -> &mut Model {
        &mut self.model
    }
}

/// The selection itself, free of GPUI handles so it can be unit-tested.
#[derive(Default, Debug)]
pub(crate) struct Model {
    active: Option<Active>,
    /// The pointer position of the mouse-move being dispatched and the best
    /// distance proposed for it so far (see [`Distance`]).
    proposal: Option<(Point<Pixels>, Distance)>,
}

impl Model {
    pub(crate) fn selected_text(&self) -> Option<String> {
        let active = self.active.as_ref()?;
        let range = active.range();
        if range.is_empty() {
            return None;
        }
        active.text.get(range).map(str::to_string)
    }

    /// `true` while a mouse drag that started in bubble `key` is in progress.
    fn is_dragging_in(&self, key: &str) -> bool {
        self.active
            .as_ref()
            .is_some_and(|a| a.dragging && a.key.as_ref() == key)
    }

    fn has_range(&self) -> bool {
        self.active.as_ref().is_some_and(|a| !a.range().is_empty())
    }

    fn clear_settled(&mut self) -> bool {
        match &self.active {
            Some(a) if !a.dragging => {
                self.active = None;
                true
            }
            _ => false,
        }
    }

    /// Mouse down at byte `index` of the bubble's `text`: one click arms a
    /// drag, shift extends, two clicks take the word, three the paragraph.
    pub(crate) fn begin(
        &mut self,
        key: SharedString,
        text: SharedString,
        index: usize,
        click_count: usize,
        shift: bool,
    ) {
        self.proposal = None;
        if shift {
            if let Some(a) = self.active.as_mut() {
                if a.key == key && a.text == text {
                    a.head = snap(&text, index, a.granularity);
                    a.dragging = true;
                    return;
                }
            }
        }
        let granularity = match click_count {
            0 | 1 => Granularity::Character,
            2 => Granularity::Word,
            _ => Granularity::Paragraph,
        };
        let anchor = snap(&text, index, granularity);
        self.active = Some(Active {
            key,
            text,
            head: anchor.clone(),
            anchor,
            granularity,
            dragging: true,
        });
    }

    /// Drag to byte `index`; `true` when the range changed.
    pub(crate) fn extend_to(&mut self, index: usize) -> bool {
        let Some(a) = self.active.as_mut() else {
            return false;
        };
        let head = snap(&a.text, index, a.granularity);
        if head == a.head {
            return false;
        }
        a.head = head;
        true
    }

    /// One text element's answer to a mouse move at `position`: the index it
    /// would extend to, and how far the pointer is from it. The nearest
    /// element of the same move wins; `true` when the range changed.
    fn propose(&mut self, position: Point<Pixels>, index: usize, distance: Distance) -> bool {
        match self.proposal {
            Some((at, best)) if at == position && best <= distance => return false,
            _ => self.proposal = Some((position, distance)),
        }
        self.extend_to(index)
    }

    /// Mouse up: `true` when the gesture ended as a plain click (no range).
    /// An empty selection is kept (not cleared) so every element's mouse-up
    /// sees the same answer; it is nothing to copy or draw.
    pub(crate) fn finish_drag(&mut self) -> bool {
        self.proposal = None;
        let Some(a) = self.active.as_mut() else {
            return true;
        };
        a.dragging = false;
        a.range().is_empty()
    }
}

/// The per-render snapshot of the selection, plus what a text block needs to
/// draw its highlight and wire its mouse events.
#[derive(Clone)]
pub struct Selectable {
    entity: Entity<TextSelection>,
    current: Option<(SharedString, SharedString, Range<usize>)>,
    color: Hsla,
    /// Who `@name` runs resolve to, and the accent the chips take (name
    /// text; the pill is the accent at `mention::PILL_ALPHA`, brighter under
    /// the pointer).
    mentions: Rc<Mentions>,
    chip_accent: Hsla,
}

impl Selectable {
    pub fn new(entity: &Entity<TextSelection>, color: Hsla, cx: &App) -> Self {
        let current = entity.read(cx).model.active.as_ref().and_then(|a| {
            let range = a.range();
            (!range.is_empty()).then(|| (a.key.clone(), a.text.clone(), range))
        });
        Self {
            entity: entity.clone(),
            current,
            color,
            mentions: Mentions::empty(),
            chip_accent: Hsla::default(),
        }
    }

    /// Mention chips: `@Name` runs of a known entity draw as a chip (`accent`
    /// text on an accent pill) and open its page on click.
    pub fn with_mentions(mut self, mentions: Rc<Mentions>, accent: Hsla) -> Self {
        self.mentions = mentions;
        self.chip_accent = accent;
        self
    }

    /// The same selection drawn in another colour (dark bubbles need more).
    pub fn with_color(&self, color: Hsla) -> Self {
        Self {
            color,
            ..self.clone()
        }
    }

    /// Scope to one bubble: `key` names it, `text` is its whole rendered
    /// plain text (every block, in order). The bubble's text elements are
    /// then built with [`BubbleSelectable::text`] at their byte offset.
    pub fn bubble(
        &self,
        key: impl Into<SharedString>,
        text: impl Into<SharedString>,
    ) -> BubbleSelectable {
        let key = key.into();
        let text = text.into();
        let range = match &self.current {
            Some((k, t, range)) if *k == key && *t == text => Some(range.clone()),
            _ => None,
        };
        BubbleSelectable {
            entity: self.entity.clone(),
            key,
            text,
            range,
            color: self.color,
            mentions: self.mentions.clone(),
            chip_accent: self.chip_accent,
        }
    }
}

/// [`Selectable`] scoped to one bubble.
#[derive(Clone)]
pub struct BubbleSelectable {
    entity: Entity<TextSelection>,
    key: SharedString,
    text: SharedString,
    /// The current selection when it lives in this bubble.
    range: Option<Range<usize>>,
    color: Hsla,
    mentions: Rc<Mentions>,
    chip_accent: Hsla,
}

/// One mention chip inside a [`SelectableText`]: the `@Name` run (element-local
/// bytes), what it shows and where a click goes.
pub struct Chip {
    pub range: Range<usize>,
    /// Bytes of the picture cell at the start of the run (`@` or `@` + hair space).
    pub cell: usize,
    pub mention: Rc<Mention>,
    pub mentions: Rc<Mentions>,
    /// The accent the pill is tinted with (see [`mention::pill_color`]).
    pub accent: Hsla,
    /// The chip's name across frames (bubble, range in the bubble's text).
    pub key: ChipKey,
}

impl Chip {
    /// A chip whose click opens a page. Collective `@here` / `@agents` have
    /// none and stay still under the pointer.
    fn clickable(&self) -> bool {
        self.mention.page.is_some()
    }
}

impl BubbleSelectable {
    /// A selectable text element whose `text` starts at byte `at` of the
    /// bubble's plain text. `runs` cover `text` exactly; the selection
    /// highlight is painted as run backgrounds so it follows soft wraps.
    pub fn text(
        &self,
        at: usize,
        text: impl Into<SharedString>,
        runs: Vec<TextRun>,
    ) -> SelectableText {
        let text = text.into();
        debug_assert_eq!(
            self.text.get(at..at + text.len()),
            Some(text.as_ref()),
            "selectable text must be the bubble's plain text at its offset"
        );
        // Mention chips first: the `@` glyph goes transparent (its cell holds
        // the picture), the name takes the accent; the selection highlight
        // then splits those runs like any other.
        let found = self.mentions.find(&text);
        let accent = self.chip_accent;
        let mut chips = Vec::with_capacity(found.len());
        let mut runs = runs;
        for (range, mention) in found {
            let cell = if text[range.clone()].starts_with(mention::CELL) {
                mention::CELL.len()
            } else {
                1
            };
            runs = chip_runs(runs, range.clone(), cell, accent);
            chips.push(Chip {
                key: (self.key.clone(), at + range.start..at + range.end),
                range,
                cell,
                mention,
                mentions: self.mentions.clone(),
                accent,
            });
        }
        let runs = match &self.range {
            Some(range) => highlight(runs, local_range(range, at, text.len()), self.color),
            None => runs,
        };
        SelectableText {
            styled: StyledText::new(text.clone()).with_runs(runs),
            key: self.key.clone(),
            bubble_text: self.text.clone(),
            at,
            links: Vec::new(),
            chips,
            selection: self.entity.clone(),
        }
    }
}

/// Recolour the `@Name` run for a chip: the `@` (first byte) transparent, the
/// name in `accent`, no underline; everything else untouched.
pub fn chip_runs(
    runs: Vec<TextRun>,
    range: Range<usize>,
    cell: usize,
    accent: Hsla,
) -> Vec<TextRun> {
    let mut out = Vec::with_capacity(runs.len() + 3);
    let mut pos = 0usize;
    let at_end = range.start + cell;
    for run in runs {
        let run_end = pos + run.len;
        let mut cursor = pos;
        for cut in [range.start, at_end, range.end, run_end] {
            if cut <= cursor || cut > run_end {
                continue;
            }
            let color = if cursor >= range.start && cursor < at_end {
                gpui::transparent_black()
            } else if cursor >= at_end && cursor < range.end {
                accent
            } else {
                run.color
            };
            let in_chip = cursor >= range.start && cursor < range.end;
            out.push(TextRun {
                len: cut - cursor,
                color,
                underline: if in_chip { None } else { run.underline },
                ..run.clone()
            });
            cursor = cut;
        }
        pos = run_end;
    }
    out
}

/// The part of a bubble-wide `range` that falls inside the element starting
/// at `at` with `len` bytes, in the element's own offsets (empty when none).
pub fn local_range(range: &Range<usize>, at: usize, len: usize) -> Range<usize> {
    let start = range.start.clamp(at, at + len) - at;
    let end = range.end.clamp(at, at + len) - at;
    start..end.max(start)
}

/// Snap `index` to the unit the gesture selects by.
fn snap(text: &str, index: usize, granularity: Granularity) -> Range<usize> {
    let index = clamp_to_char_boundary(text, index);
    match granularity {
        Granularity::Character => index..index,
        Granularity::Word => word_range(text, index),
        Granularity::Paragraph => paragraph_range(text, index),
    }
}

fn clamp_to_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// The Unicode word (UAX #29 segment) containing `index`; the segment before
/// it when `index` is at the very end.
pub fn word_range(text: &str, index: usize) -> Range<usize> {
    let mut last = 0..0;
    for (start, word) in text.split_word_bound_indices() {
        last = start..start + word.len();
        if index < last.end {
            return last;
        }
    }
    last
}

/// The hard-newline-delimited paragraph containing `index` (macOS triple-click).
pub fn paragraph_range(text: &str, index: usize) -> Range<usize> {
    let index = index.min(text.len());
    let start = text[..index].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = text[index..]
        .find('\n')
        .map(|i| index + i)
        .unwrap_or(text.len());
    start..end
}

/// Split `runs` at the selection edges and paint the inside with `color`.
pub fn highlight(runs: Vec<TextRun>, range: Range<usize>, color: Hsla) -> Vec<TextRun> {
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
                background_color: if inside {
                    Some(color)
                } else {
                    run.background_color
                },
                ..run.clone()
            });
            cursor = cut;
        }
        pos = run_end;
    }
    out
}

/// Byte index nearest to a window position, clamped to the text when the
/// pointer is outside it (dragging past an edge keeps selecting).
fn closest_index(layout: &TextLayout, position: Point<Pixels>) -> usize {
    let bounds = layout.bounds();
    let line_height = layout.line_height();
    let len = layout.len();
    if position.y < bounds.top() {
        return 0;
    }
    let lines = layout.line_layouts();
    let count = lines.len();
    let mut origin = bounds.origin;
    let mut start = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let height = line.size(line_height).height;
        if position.y <= origin.y + height || i + 1 == count {
            // A local y of exactly `height` would index one wrapped line too far.
            let y = (position.y - origin.y).clamp(px(0.0), height - px(0.5));
            let local = point(position.x - origin.x, y);
            let ix = match line.closest_index_for_position(local, line_height) {
                Ok(ix) | Err(ix) => ix,
            };
            return (start + ix.min(line.len())).min(len);
        }
        origin.y += height;
        start += line.len() + 1;
    }
    len
}

/// How far `position` is from `bounds` (see [`Distance`]).
pub fn distance_to(bounds: &Bounds<Pixels>, position: Point<Pixels>) -> Distance {
    let gap = |low: Pixels, high: Pixels, p: Pixels| -> f32 {
        if p < low {
            f32::from(low - p)
        } else if p > high {
            f32::from(p - high)
        } else {
            0.0
        }
    };
    (
        gap(bounds.top(), bounds.bottom(), position.y),
        gap(bounds.left(), bounds.right(), position.x),
    )
}

/// What a drag at `position` should extend to inside one element: the index
/// under the pointer when it is at the element's height, else the element's
/// start (pointer above) or end (pointer below).
fn drag_index(layout: &TextLayout, position: Point<Pixels>, distance: Distance) -> usize {
    let bounds = layout.bounds();
    if distance.0 > 0.0 {
        if position.y < bounds.top() {
            0
        } else {
            layout.len()
        }
    } else {
        closest_index(layout, position)
    }
}

/// A [`StyledText`] the user can select from. Registers its own mouse
/// listeners during paint (the same way `InteractiveText` does).
pub struct SelectableText {
    /// The bubble this element belongs to, its whole plain text, and where
    /// this element's text starts in it.
    key: SharedString,
    bubble_text: SharedString,
    at: usize,
    styled: StyledText,
    links: Vec<(Range<usize>, String)>,
    chips: Vec<Chip>,
    selection: Entity<TextSelection>,
}

impl SelectableText {
    /// Clickable link ranges (element-local): a plain click (no drag) inside
    /// one opens it.
    pub fn with_links(mut self, links: Vec<(Range<usize>, String)>) -> Self {
        self.links = links;
        self
    }
}

impl IntoElement for SelectableText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectableText {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.styled.request_layout(None, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.styled
            .prepaint(None, inspector_id, bounds, state, window, cx);
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.styled.layout().clone();
        let chips: Rc<Vec<Chip>> = Rc::new(std::mem::take(&mut self.chips));
        let selection = self.selection.clone();
        let now = Instant::now();
        let snap = cx.reduce_motion();
        let mut fading = false;
        selection.update(cx, |s, _| s.chips.tidy(now));
        for chip in chips.iter() {
            let (alpha, still) = selection.read(cx).chips.alpha(&chip.key, now, snap);
            fading |= still;
            paint_chip_pill(
                &layout,
                chip,
                mention::pill_color(chip.accent, alpha),
                window,
            );
        }
        if fading {
            window.request_animation_frame();
        }
        self.styled
            .paint(None, inspector_id, bounds, state, &mut (), window, cx);
        for chip in chips.iter() {
            paint_chip_picture(&layout, chip, window, cx);
        }
        let links = std::mem::take(&mut self.links);
        let focus_handle = selection.read(cx).focus_handle.clone();
        let (key, bubble_text, at) = (self.key.clone(), self.bubble_text.clone(), self.at);

        let over_link = hitbox.is_hovered(window)
            && layout
                .index_for_position(window.mouse_position())
                .is_ok_and(|ix| {
                    links.iter().any(|(r, _)| r.contains(&ix))
                        || chips.iter().any(|c| c.clickable() && c.range.contains(&ix))
                });
        let cursor = if over_link {
            CursorStyle::PointingHand
        } else {
            CursorStyle::IBeam
        };
        window.set_cursor_style(cursor, hitbox);
        #[cfg(test)]
        if hitbox.is_hovered(window) {
            selection.update(cx, |s, _| s.cursor = cursor);
        }

        // Hover: the chip under the pointer brightens (and the one it left
        // fades back). Only this element's chips are touched, so a bubble the
        // pointer is not over never clears a neighbour's hover.
        window.on_mouse_event({
            let (hitbox, layout, selection, chips) = (
                hitbox.clone(),
                layout.clone(),
                selection.clone(),
                chips.clone(),
            );
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                // A drag that began off a chip selects text; chips it
                // crosses stay at rest.
                let dragging_text = event.pressed_button == Some(MouseButton::Left)
                    && selection.read(cx).chips.pressed().is_none();
                let over = (hitbox.is_hovered(window) && !dragging_text)
                    .then(|| chip_at(&chips, &layout, event.position))
                    .flatten()
                    .map(|c| c.key.clone());
                let mine = selection
                    .read(cx)
                    .chips
                    .hot()
                    .is_some_and(|k| chips.iter().any(|c| c.key == *k));
                if over.is_none() && !mine {
                    return;
                }
                if selection.update(cx, |s, _| s.chips.hover(over, Instant::now())) {
                    window.refresh();
                }
            }
        });

        // Press origin for axis lock: a vertical pan with no range yet yields
        // to transcript drag-scroll instead of selecting across lines.
        let press_at = Rc::new(std::cell::Cell::new(Point::default()));

        window.on_mouse_event({
            let (hitbox, layout, selection, focus_handle) = (
                hitbox.clone(),
                layout.clone(),
                selection.clone(),
                focus_handle.clone(),
            );
            let (key, bubble_text, chips) = (key.clone(), bubble_text.clone(), chips.clone());
            let press_at = press_at.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                // Control-click is the bubble's copy; right-click its menu.
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || event.modifiers.control
                    || !hitbox.is_hovered(window)
                {
                    return;
                }
                press_at.set(event.position);
                let index = at + closest_index(&layout, event.position);
                if let Some(chip) = chip_at(&chips, &layout, event.position) {
                    if selection.update(cx, |s, _| s.chips.press(chip.key.clone(), Instant::now()))
                    {
                        window.refresh();
                    }
                }
                let selected = selection.update(cx, |s, cx| {
                    s.model.begin(
                        key.clone(),
                        bubble_text.clone(),
                        index,
                        event.click_count,
                        event.modifiers.shift,
                    );
                    cx.notify();
                    s.model.has_range()
                });
                // Only a real selection takes focus from the composer; a plain
                // press propagates so the shell can close menus as usual.
                if selected {
                    window.focus(&focus_handle, cx);
                    cx.stop_propagation();
                }
            }
        });

        window.on_mouse_event({
            let (layout, selection, focus_handle, key) = (
                layout.clone(),
                selection.clone(),
                focus_handle.clone(),
                key.clone(),
            );
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !selection.read(cx).model.is_dragging_in(&key)
                {
                    return;
                }
                if event.pressed_button != Some(MouseButton::Left) {
                    selection.update(cx, |s, cx| {
                        s.model.finish_drag();
                        cx.notify();
                    });
                    return;
                }
                // Vertical-dominant pan with no range yet: yield so the
                // transcript can drag-scroll ([feedback app-01M5PDB3QYAPBPA4FJXZE1FFD0]).
                // Double/triple-click already has a range and keeps selecting.
                if !selection.read(cx).model.has_range() {
                    let start = press_at.get();
                    let dx: f32 = (event.position.x - start.x).into();
                    let dy: f32 = (event.position.y - start.y).into();
                    if dx.abs() + dy.abs() < SELECTION_DRAG_SLOP {
                        return;
                    }
                    if dy.abs() > dx.abs() * 1.15 {
                        selection.update(cx, |s, cx| {
                            s.clear();
                            cx.notify();
                        });
                        return;
                    }
                }
                let distance = distance_to(&layout.bounds(), event.position);
                let index = at + drag_index(&layout, event.position, distance);
                let selected = selection.update(cx, |s, cx| {
                    if s.model.propose(event.position, index, distance) {
                        cx.notify();
                    }
                    s.model.has_range()
                });
                if selected && !focus_handle.is_focused(window) {
                    window.focus(&focus_handle, cx);
                }
            }
        });

        window.on_mouse_event({
            let hitbox = hitbox.clone();
            move |event: &MouseUpEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !selection.read(cx).model.is_dragging_in(&key)
                {
                    return;
                }
                let plain_click = selection.update(cx, |s, cx| {
                    let plain = s.model.finish_drag();
                    if s.chips.release(Instant::now()).is_some() {
                        window.refresh();
                    }
                    cx.notify();
                    plain
                });
                if plain_click && event.click_count == 1 && hitbox.is_hovered(window) {
                    if let Some(chip) = chip_at(&chips, &layout, event.position) {
                        chip.mentions.open(&chip.mention, cx);
                    } else if let Some((_, href)) = layout
                        .index_for_position(event.position)
                        .ok()
                        .and_then(|ix| links.iter().find(|(r, _)| r.contains(&ix)))
                    {
                        cx.open_url(href);
                    }
                }
            }
        });
    }
}

/// The clickable chip under `position`, if the pointer is on one of its glyphs.
fn chip_at<'a>(
    chips: &'a [Chip],
    layout: &TextLayout,
    position: Point<Pixels>,
) -> Option<&'a Chip> {
    let ix = layout.index_for_position(position).ok()?;
    chips
        .iter()
        .find(|c| c.clickable() && c.range.contains(&ix))
}

/// The chip's pill: one rounded quad per line fragment the `@Name` run
/// occupies, `PILL_PAD_X` wider than the glyphs on each side and
/// `PILL_PAD_Y` taller above and below the line box. Painted before the text.
fn paint_chip_pill(layout: &TextLayout, chip: &Chip, color: Hsla, window: &mut Window) {
    for (start, end, top) in chip_fragments(layout, &chip.range) {
        let line_h = layout.line_height();
        let bounds = Bounds::new(
            point(
                start - px(mention::PILL_PAD_X),
                top - px(mention::PILL_PAD_Y),
            ),
            size(
                end - start + px(mention::PILL_PAD_X * 2.0),
                line_h + px(mention::PILL_PAD_Y * 2.0),
            ),
        );
        window.paint_quad(gpui::quad(
            bounds,
            Corners::all(px(mention::PILL_RADIUS)),
            color,
            gpui::Edges::all(px(0.0)),
            gpui::transparent_black(),
            gpui::BorderStyle::default(),
        ));
    }
}

/// The entity's picture, a `PICTURE` pt disc centred in the `@` glyph's cell
/// (which the run paints transparent). Painted over the text.
fn paint_chip_picture(layout: &TextLayout, chip: &Chip, window: &mut Window, cx: &mut App) {
    let Some(at) = layout.position_for_index(chip.range.start) else {
        return;
    };
    let Some(after_at) = layout.position_for_index(chip.range.start + chip.cell) else {
        return;
    };
    if after_at.y != at.y {
        return;
    }
    let line_h = layout.line_height();
    // The disc sits at the cell's left and never reaches the name: it is
    // the cell minus the gap when that is smaller than the full picture.
    let cell_w = after_at.x - at.x;
    let d = px(mention::PICTURE)
        .min(line_h)
        .min(cell_w - px(mention::PICTURE_GAP));
    if d <= px(4.0) {
        return;
    }
    let origin = point(at.x, at.y + (line_h - d) / 2.0);
    paint_picture(
        &chip.mention.picture,
        Bounds::new(origin, size(d, d)),
        window,
        cx,
    );
}

/// The disc a chip's `@` cell holds — a photo, D79 initials or a D92 leaf —
/// filling `bounds` (square). Shared by bubble chips and composer tokens.
pub fn paint_picture(
    picture: &mention::Picture,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let origin = bounds.origin;
    let d = bounds.size.width;
    let radii = Corners::all(d / 2.0);
    match picture {
        mention::Picture::Image(image) => {
            let _ = window.paint_image(bounds, radii, image.clone(), 0, false);
        }
        mention::Picture::Initials { text, top, bottom } => {
            window.paint_quad(gpui::quad(
                bounds,
                radii,
                linear_gradient(
                    180.0,
                    linear_color_stop(*top, 0.0),
                    linear_color_stop(*bottom, 1.0),
                ),
                gpui::Edges::all(px(0.0)),
                gpui::transparent_black(),
                gpui::BorderStyle::default(),
            ));
            let font_size = d * avatars::INITIALS_SCALE;
            let mut font = window.text_style().font();
            font.weight = FontWeight::SEMIBOLD;
            let run = TextRun {
                len: text.len(),
                font,
                color: gpui::white(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window.text_system().shape_line(
                SharedString::from(text.clone()),
                font_size,
                &[run],
                None,
            );
            let x = origin.x + (d - line.width) / 2.0;
            let _ = line.paint(point(x, origin.y), d, TextAlign::Left, None, window, cx);
        }
        mention::Picture::Leaf(pick) => {
            window.paint_quad(gpui::quad(
                bounds,
                radii,
                avatars::leaf_disc(pick.color),
                gpui::Edges::all(px(0.0)),
                gpui::transparent_black(),
                gpui::BorderStyle::default(),
            ));
            let side = d * arbos_core::avatar::leaf::LEAF_SCALE;
            let leaf = Bounds::new(
                point(origin.x + (d - side) / 2.0, origin.y + (d - side) / 2.0),
                size(side, side),
            );
            let _ = window.paint_svg(
                leaf,
                avatars::leaf_asset(pick.leaf),
                None,
                TransformationMatrix::unit(),
                avatars::leaf_ink(pick.color),
                cx,
            );
        }
    }
}

/// The (start x, end x, line top) of each wrapped-line fragment a byte range
/// covers. A range on one line is one fragment; one that soft-wraps gives the
/// tail of the first line and the head of the next.
fn chip_fragments(layout: &TextLayout, range: &Range<usize>) -> Vec<(Pixels, Pixels, Pixels)> {
    let (Some(start), Some(end)) = (
        layout.position_for_index(range.start),
        layout.position_for_index(range.end),
    ) else {
        return Vec::new();
    };
    if start.y == end.y {
        return vec![(start.x, end.x, start.y)];
    }
    let bounds = layout.bounds();
    vec![
        (start.x, bounds.right(), start.y),
        (bounds.left(), end.x, end.y),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, FontStyle, FontWeight};

    fn run(len: usize) -> TextRun {
        TextRun {
            len,
            font: gpui::Font {
                family: "Test".into(),
                features: Default::default(),
                fallbacks: None,
                weight: FontWeight::NORMAL,
                style: FontStyle::Normal,
            },
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    #[test]
    fn double_click_takes_the_word_under_the_pointer() {
        let text = "double-clicking a word";
        assert_eq!(&text[word_range(text, 0)], "double");
        assert_eq!(&text[word_range(text, 8)], "clicking");
        assert_eq!(&text[word_range(text, 18)], "word");
        // At the very end: the last word, not an empty range.
        assert_eq!(&text[word_range(text, text.len())], "word");
        // On the gap between words: the gap itself (macOS behaviour).
        assert_eq!(&text[word_range(text, 15)], " ");
    }

    #[test]
    fn word_boundaries_are_unicode_aware() {
        let text = "héllo wörld — naïve café";
        assert_eq!(&text[word_range(text, 1)], "héllo");
        let w = text.find("wörld").unwrap();
        assert_eq!(&text[word_range(text, w + 2)], "wörld");
        let c = text.find("café").unwrap();
        assert_eq!(&text[word_range(text, c)], "café");
        // Apostrophes stay inside the word.
        let text = "it's fine";
        assert_eq!(&text[word_range(text, 2)], "it's");
    }

    #[test]
    fn triple_click_takes_the_paragraph() {
        let text = "first line\nsecond line\nthird";
        assert_eq!(&text[paragraph_range(text, 3)], "first line");
        assert_eq!(&text[paragraph_range(text, 12)], "second line");
        assert_eq!(&text[paragraph_range(text, text.len())], "third");
        assert_eq!(paragraph_range("one", 1), 0..3);
    }

    #[test]
    fn snap_lands_on_char_boundaries() {
        let text = "héllo";
        // Byte 2 is inside the two-byte 'é'.
        assert_eq!(snap(text, 2, Granularity::Character), 1..1);
        assert_eq!(snap(text, 99, Granularity::Character), 6..6);
    }

    #[test]
    fn highlight_splits_runs_at_the_selection_edges() {
        let runs = vec![run(5), run(5)];
        let color = Hsla {
            h: 0.5,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        };
        let out = highlight(runs, 3..7, color);
        let lens: Vec<usize> = out.iter().map(|r| r.len).collect();
        assert_eq!(lens, vec![3, 2, 2, 3]);
        let shaded: Vec<bool> = out.iter().map(|r| r.background_color.is_some()).collect();
        assert_eq!(shaded, vec![false, true, true, false]);
        assert_eq!(out.iter().map(|r| r.len).sum::<usize>(), 10);
    }

    #[test]
    fn highlight_keeps_untouched_runs_and_empty_ranges() {
        let runs = vec![run(4)];
        assert_eq!(highlight(runs.clone(), 2..2, Hsla::default()).len(), 1);
        let out = highlight(runs, 0..4, Hsla::default());
        assert_eq!(out.len(), 1);
        assert!(out[0].background_color.is_some());
    }

    #[test]
    fn drag_from_a_double_click_extends_by_whole_words() {
        let text: SharedString = "alpha beta gamma".into();
        let mut sel = Model::default();
        sel.begin("k".into(), text.clone(), 7, 2, false);
        assert_eq!(sel.selected_text().as_deref(), Some("beta"));
        assert!(sel.extend_to(13));
        assert_eq!(sel.selected_text().as_deref(), Some("beta gamma"));
        // Dragging back before the anchor keeps the anchor word.
        assert!(sel.extend_to(1));
        assert_eq!(sel.selected_text().as_deref(), Some("alpha beta"));
        assert!(!sel.finish_drag());
        // A settled selection clears; a plain single click leaves nothing.
        assert!(sel.clear_settled());
        sel.begin("k".into(), text, 3, 1, false);
        assert!(sel.finish_drag());
        assert_eq!(sel.selected_text(), None);
        assert!(!sel.has_range());
    }

    #[test]
    fn shift_click_extends_the_existing_selection() {
        let text: SharedString = "alpha beta gamma".into();
        let mut sel = Model::default();
        sel.begin("k".into(), text.clone(), 0, 2, false);
        sel.finish_drag();
        sel.begin("k".into(), text.clone(), 12, 1, true);
        assert_eq!(sel.selected_text().as_deref(), Some("alpha beta gamma"));
        // Shift-click in another bubble starts over there.
        sel.begin("other".into(), "zeta".into(), 1, 1, true);
        assert_eq!(sel.selected_text(), None);
    }

    /// Jacob's report (#1389): a drag that starts in one paragraph and ends
    /// three paragraphs down selects everything in between, end to end.
    #[test]
    fn drag_spans_paragraphs_end_to_end() {
        let paragraphs = [
            "First paragraph.",
            "Second one, longer.",
            "Third.",
            "Fourth and last.",
        ];
        let text: SharedString = paragraphs.join("\n").into();
        let mut sel = Model::default();
        sel.begin("bubble".into(), text.clone(), 0, 1, false);
        assert!(sel.extend_to(text.len()));
        assert_eq!(sel.selected_text().as_deref(), Some(text.as_ref()));
        assert!(!sel.finish_drag());

        // From the middle of the second paragraph into the middle of the
        // fourth: the whole third paragraph and both line breaks come along.
        let p2 = text.find("Second").unwrap();
        let p4 = text.find("Fourth").unwrap();
        sel.begin("bubble".into(), text.clone(), p2 + 7, 1, false);
        sel.extend_to(p4 + 6);
        assert_eq!(
            sel.selected_text().as_deref(),
            Some("one, longer.\nThird.\nFourth")
        );
        // Dragging back above the anchor flips the range the same way.
        sel.extend_to(3);
        assert_eq!(
            sel.selected_text().as_deref(),
            Some("st paragraph.\nSecond ")
        );
    }

    /// Double-click selects one word, triple-click one paragraph — never a
    /// neighbouring paragraph, even though the bubble is one text now.
    #[test]
    fn double_and_triple_click_stay_inside_their_paragraph() {
        let text: SharedString = "Alpha beta.\nGamma delta.\nEpsilon.".into();
        let gamma = text.find("Gamma").unwrap();
        let mut sel = Model::default();
        sel.begin("b".into(), text.clone(), gamma + 2, 2, false);
        assert_eq!(sel.selected_text().as_deref(), Some("Gamma"));
        sel.finish_drag();
        sel.begin("b".into(), text.clone(), gamma + 8, 3, false);
        assert_eq!(sel.selected_text().as_deref(), Some("Gamma delta."));
        // A triple-click drag extends by whole paragraphs.
        sel.extend_to(text.len());
        assert_eq!(
            sel.selected_text().as_deref(),
            Some("Gamma delta.\nEpsilon.")
        );
    }

    /// Two paragraphs answer the same mouse move; the nearer one wins no
    /// matter which answers first, and the next move starts afresh.
    #[test]
    fn nearest_element_wins_a_move() {
        let text: SharedString = "one\ntwo\nthree".into();
        let mut sel = Model::default();
        sel.begin("b".into(), text.clone(), 0, 1, false);
        let p = point(px(10.0), px(50.0));
        // Pointer in the gap: "two" (below, 4 px away) ends at 7; "one"
        // (above, 6 px away) ends at 3.
        assert!(sel.propose(p, 3, (6.0, 0.0)));
        assert!(sel.propose(p, 7, (4.0, 0.0)));
        assert_eq!(sel.selected_text().as_deref(), Some("one\ntwo"));
        // The farther one, arriving later, is ignored.
        assert!(!sel.propose(p, 3, (6.0, 0.0)));
        assert_eq!(sel.selected_text().as_deref(), Some("one\ntwo"));
        // Same vertical distance: the horizontally nearer one wins (cells).
        let q = point(px(10.0), px(80.0));
        assert!(sel.propose(q, 13, (0.0, 12.0)));
        assert!(sel.propose(q, 9, (0.0, 2.0)));
        assert_eq!(sel.selected_text().as_deref(), Some("one\ntwo\nt"));
    }

    #[gpui::test]
    fn rig_select_specs_act_like_the_gestures(cx: &mut gpui::TestAppContext) {
        let text: SharedString = "Alpha beta.\nGamma delta.".into();
        let sel = cx.new(TextSelection::new);
        let pick = |cx: &mut gpui::TestAppContext, spec: &str| {
            sel.update(cx, |s, _| {
                assert!(s.rig_select("k".into(), text.clone(), spec), "{spec}");
                s.selected_text()
            })
        };
        assert_eq!(pick(cx, "all").as_deref(), Some(text.as_ref()));
        assert_eq!(pick(cx, "2..8").as_deref(), Some("pha be"));
        assert_eq!(pick(cx, "word@13").as_deref(), Some("Gamma"));
        assert_eq!(pick(cx, "para@13").as_deref(), Some("Gamma delta."));
        sel.update(cx, |s, _| {
            assert!(!s.rig_select("k".into(), text.clone(), "nonsense"));
        });
    }

    /// A bubble whose `text` starts with a chip, e.g. `@Comms says hi`.
    struct ChipBubble {
        selection: Entity<TextSelection>,
        mentions: Rc<Mentions>,
        text: SharedString,
    }

    /// Comms (an agent with a page) and the collective `here` (no page).
    fn chip_table(opened: &Rc<std::cell::RefCell<Vec<crate::app::ProfilePage>>>) -> Rc<Mentions> {
        use crate::app::ProfilePage;
        let record = opened.clone();
        let mut here = crate::ui::mention::tests::stub("here", ProfilePage::Me);
        here.page = None;
        Mentions::with_table(
            vec![
                (
                    "comms",
                    crate::ui::mention::tests::stub(
                        "Comms",
                        ProfilePage::Agent {
                            project_id: "p-comms".into(),
                        },
                    ),
                ),
                ("here", here),
            ],
            move |page, _| record.borrow_mut().push(page.clone()),
        )
    }

    impl gpui::Render for ChipBubble {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::{div, InteractiveElement as _, ParentElement as _, Styled as _};
            let text = self.text.clone();
            let selectable = Selectable::new(&self.selection, Hsla::default(), cx)
                .with_mentions(self.mentions.clone(), gpui::blue());
            let bubble = selectable.bubble("m1", text.clone());
            div()
                .w(px(400.0))
                .h(px(100.0))
                .p(px(20.0))
                .text_size(px(15.0))
                .child(
                    div()
                        .debug_selector(|| "chip-text".into())
                        .child(bubble.text(0, text.clone(), vec![run(text.len())])),
                )
        }
    }

    #[gpui::test]
    fn a_click_on_a_chip_opens_its_page_and_a_click_past_it_does_not(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::app::ProfilePage;
        use std::cell::RefCell;
        let opened: Rc<RefCell<Vec<ProfilePage>>> = Rc::new(RefCell::new(Vec::new()));
        let mentions = chip_table(&opened);
        let (_view, vcx) = cx.add_window_view(|_, cx| ChipBubble {
            selection: cx.new(TextSelection::new),
            mentions,
            text: "@Comms says hi".into(),
        });
        vcx.run_until_parked();
        let bounds = vcx.debug_bounds("chip-text").expect("text drawn");
        let y = bounds.origin.y + bounds.size.height / 2.0;
        // Inside "@Comms": the second glyph.
        let on_chip = point(bounds.origin.x + px(12.0), y);
        vcx.simulate_click(on_chip, gpui::Modifiers::default());
        vcx.run_until_parked();
        assert_eq!(opened.borrow().len(), 1, "one click, one page");
        assert!(matches!(
            &opened.borrow()[0],
            ProfilePage::Agent { project_id } if project_id == "p-comms"
        ));
        // Far right of the line, past "says hi": plain text, nothing opens.
        let past = point(bounds.right() - px(4.0), y);
        vcx.simulate_click(past, gpui::Modifiers::default());
        vcx.run_until_parked();
        assert_eq!(
            opened.borrow().len(),
            1,
            "a click past the chip opens nothing"
        );
    }

    /// Jacob (Oct 24): a chip must look clickable — pointing hand and a
    /// brighter pill under the pointer, brighter still while pressed; a chip
    /// with no page (`@here`) and plain text (`@nobody`) stay as they are.
    #[gpui::test]
    fn a_chip_brightens_under_the_pointer_and_while_pressed(cx: &mut gpui::TestAppContext) {
        use crate::ui::mention::{ChipState, PILL_ALPHA, PILL_ALPHA_HOVER, PILL_ALPHA_PRESSED};
        use std::cell::RefCell;
        let opened = Rc::new(RefCell::new(Vec::new()));
        let mentions = chip_table(&opened);
        let selection = cx.update(|cx| cx.new(TextSelection::new));
        let (view, vcx) = cx.add_window_view(|_, _| ChipBubble {
            selection: selection.clone(),
            mentions,
            text: "@Comms says hi".into(),
        });
        vcx.run_until_parked();
        let bounds = vcx.debug_bounds("chip-text").expect("text drawn");
        let y = bounds.origin.y + bounds.size.height / 2.0;
        let on_chip = point(bounds.origin.x + px(12.0), y);
        let on_text = point(bounds.right() - px(4.0), y);
        let key: ChipKey = ("m1".into(), 0..6);
        let none = gpui::Modifiers::default();
        let settled = |s: &TextSelection, snap: bool| {
            s.chips
                .alpha(&key, Instant::now() + mention::PILL_FADE, snap)
                .0
        };

        // Plain text first: nothing hot, an I-beam. (The real cursor follows
        // the pointer without a repaint; the test's record needs one.)
        vcx.simulate_mouse_move(on_text, None, none);
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        selection.read_with(&*vcx, |s, _| {
            assert_eq!(s.chips.hot(), None);
            assert_eq!(s.cursor, CursorStyle::IBeam);
            assert_eq!(settled(s, true), PILL_ALPHA);
        });

        vcx.simulate_mouse_move(on_chip, None, none);
        vcx.run_until_parked();
        selection.read_with(&*vcx, |s, _| {
            assert_eq!(s.chips.state(&key), ChipState::Hover);
            assert_eq!(s.cursor, CursorStyle::PointingHand, "a hand over the chip");
            assert_eq!(settled(s, false), PILL_ALPHA_HOVER);
            assert_eq!(settled(s, true), PILL_ALPHA_HOVER, "reduced motion snaps");
        });

        vcx.simulate_mouse_down(on_chip, MouseButton::Left, none);
        vcx.run_until_parked();
        selection.read_with(&*vcx, |s, _| {
            assert_eq!(s.chips.state(&key), ChipState::Pressed);
            assert_eq!(settled(s, false), PILL_ALPHA_PRESSED);
        });

        vcx.simulate_mouse_up(on_chip, MouseButton::Left, none);
        vcx.run_until_parked();
        selection.read_with(&*vcx, |s, _| {
            assert_eq!(
                s.chips.state(&key),
                ChipState::Hover,
                "released: back to hover"
            );
            assert_eq!(settled(s, false), PILL_ALPHA_HOVER);
        });
        assert_eq!(
            opened.borrow().len(),
            1,
            "the press-and-release opened the page"
        );

        vcx.simulate_mouse_move(on_text, None, none);
        vcx.run_until_parked();
        selection.read_with(&*vcx, |s, _| {
            assert_eq!(s.chips.state(&key), ChipState::Rest);
            assert_eq!(s.cursor, CursorStyle::IBeam);
            assert_eq!(settled(s, false), PILL_ALPHA, "fades back to rest");
        });
    }

    #[gpui::test]
    fn chips_without_a_page_and_unknown_handles_stay_still(cx: &mut gpui::TestAppContext) {
        use std::cell::RefCell;
        for text in ["@here says hi", "@nobody says hi"] {
            let opened = Rc::new(RefCell::new(Vec::new()));
            let mentions = chip_table(&opened);
            let selection = cx.update(|cx| cx.new(TextSelection::new));
            let (view, vcx) = cx.add_window_view(|_, _| ChipBubble {
                selection: selection.clone(),
                mentions,
                text: text.into(),
            });
            vcx.run_until_parked();
            let bounds = vcx.debug_bounds("chip-text").expect("text drawn");
            let y = bounds.origin.y + bounds.size.height / 2.0;
            let on_at = point(bounds.origin.x + px(12.0), y);
            vcx.simulate_mouse_move(on_at, None, gpui::Modifiers::default());
            vcx.simulate_click(on_at, gpui::Modifiers::default());
            view.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
            selection.read_with(&*vcx, |s, _| {
                assert_eq!(s.chips.hot(), None, "{text}: nothing lights up");
                assert_eq!(s.cursor, CursorStyle::IBeam, "{text}: no hand");
            });
            assert!(opened.borrow().is_empty(), "{text}: nothing opens");
        }
    }

    #[test]
    fn chip_runs_hide_the_at_and_colour_the_name() {
        let runs = chip_runs(vec![run(14)], 0..6, 1, gpui::blue());
        let lens: Vec<usize> = runs.iter().map(|r| r.len).collect();
        assert_eq!(lens, vec![1, 5, 8]);
        assert_eq!(runs[0].color, gpui::transparent_black());
        assert_eq!(runs[1].color, gpui::blue());
        assert_eq!(runs[2].color, Hsla::default());
        // A chip in the middle of a run keeps the text before it.
        let runs = chip_runs(vec![run(20)], 4..10, 1, gpui::blue());
        let lens: Vec<usize> = runs.iter().map(|r| r.len).collect();
        assert_eq!(lens, vec![4, 1, 5, 10]);
        // With the hair-space cell (4 bytes), the transparent run is 4 long.
        let runs = chip_runs(vec![run(20)], 0..9, 4, gpui::blue());
        let lens: Vec<usize> = runs.iter().map(|r| r.len).collect();
        assert_eq!(lens, vec![4, 5, 11]);
    }

    #[test]
    fn local_range_clips_the_bubble_range_to_one_element() {
        // Element at 10..20 of the bubble.
        assert_eq!(local_range(&(0..30), 10, 10), 0..10);
        assert_eq!(local_range(&(12..15), 10, 10), 2..5);
        assert_eq!(local_range(&(0..5), 10, 10), 0..0);
        assert_eq!(local_range(&(25..30), 10, 10), 10..10);
        assert_eq!(local_range(&(15..40), 10, 10), 5..10);
    }
}
