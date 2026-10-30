//! Text selection for the chat transcript: drag, double-click (word),
//! triple-click (paragraph), shift-click, ⌘C. A selection is a pair of
//! positions, each a byte offset into one **bubble's** rendered plain text
//! (which runs through every paragraph, list, table and code block of the
//! message) plus that bubble's place in the transcript. Each text element
//! knows the bubble it belongs to and where its own text starts in it, so a
//! drag that leaves one paragraph continues in the next (#1389), and a drag
//! that leaves one bubble continues in the next bubble, up or down, like
//! Grok's (Jacob, 2026-10-24, [feedback bubble-selection-2026-10-24]): the
//! bubbles between the two ends are selected whole, and ⌘C copies them in
//! transcript order whichever way the drag went.
//!
//! [`TextSelection`] is the model (an entity the transcript observes);
//! [`Selectable::bubble`] scopes it to one bubble; [`SelectableText`] wraps a
//! [`StyledText`] and wires the mouse to the model from inside its own paint,
//! where the text layout is guaranteed to exist.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    linear_color_stop, linear_gradient, point, px, size, App, Bounds, Context, Corners,
    CursorStyle, DispatchPhase, Element, ElementId, Entity, FocusHandle, FontWeight,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, RenderImage,
    SharedString, StyledText, TextAlign, TextLayout, TextRun, TransformationMatrix, Window,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::ui::avatars;
use crate::ui::mention::{self, Mention, Mentions};

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

/// One bubble as the selection sees it: where it sits in the transcript
/// (`order` — its first row's index, shared by every block row of the
/// message), its key, and its rendered plain text at the time of the gesture
/// (copy reads from this, so a streaming bubble cannot shift the offsets
/// under the user).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BubbleAt {
    pub order: usize,
    pub key: SharedString,
    pub text: SharedString,
}

impl BubbleAt {
    /// Transcript order: by row, then by key for two bubbles on one row.
    fn rank(&self) -> (usize, &str) {
        (self.order, self.key.as_ref())
    }
}

/// One end of the selection: a snapped byte range inside one bubble.
#[derive(Clone, Debug)]
struct Pos {
    at: BubbleAt,
    range: Range<usize>,
}

impl Pos {
    fn same_bubble(&self, other: &Pos) -> bool {
        self.at.key == other.at.key
    }
}

/// The two ends of a selection in transcript order, with the byte offsets in
/// their bubbles; `start` and `end` name the same bubble when the selection
/// lives in one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: BubbleAt,
    pub start_index: usize,
    pub end: BubbleAt,
    pub end_index: usize,
}

impl Span {
    /// The part of bubble `at` this span covers, in that bubble's own bytes:
    /// all of it between the ends, a tail at the start, a head at the end.
    pub fn range_in(&self, at: &BubbleAt) -> Option<Range<usize>> {
        let same_start = at.key == self.start.key;
        let same_end = at.key == self.end.key;
        if same_start && same_end {
            return Some(self.start_index..self.end_index);
        }
        if same_start {
            return Some(self.start_index..at.text.len());
        }
        if same_end {
            return Some(0..self.end_index);
        }
        (self.start.rank() < at.rank() && at.rank() < self.end.rank()).then(|| 0..at.text.len())
    }

    /// `true` when the two ends are one bubble (plain ⌘C can copy from the
    /// span's own texts).
    pub fn one_bubble(&self) -> bool {
        self.start.key == self.end.key
    }
}

#[derive(Clone, Debug)]
struct Active {
    /// The fixed end: the pressed index, or the pressed word / paragraph.
    anchor: Pos,
    /// The moving end, already snapped to the granularity.
    head: Pos,
    granularity: Granularity,
    dragging: bool,
}

impl Active {
    /// The ends in transcript order.
    fn ends(&self) -> (&Pos, &Pos) {
        let (a, h) = (&self.anchor, &self.head);
        if a.same_bubble(h) {
            if h.range.start < a.range.start {
                (h, a)
            } else {
                (a, h)
            }
        } else if h.at.rank() < a.at.rank() {
            (h, a)
        } else {
            (a, h)
        }
    }

    /// Nothing is selected: both ends in one bubble at one point.
    fn is_empty(&self) -> bool {
        self.anchor.same_bubble(&self.head) && self.range().is_empty()
    }

    /// The range inside the one bubble both ends share (callers check).
    fn range(&self) -> Range<usize> {
        let (a, h) = (&self.anchor.range, &self.head.range);
        a.start.min(h.start)..a.end.max(h.end)
    }

    fn span(&self) -> Span {
        let (start, end) = self.ends();
        if start.same_bubble(end) {
            let range = self.range();
            Span {
                start: start.at.clone(),
                start_index: range.start,
                end: end.at.clone(),
                end_index: range.end,
            }
        } else {
            Span {
                start: start.at.clone(),
                start_index: start.range.start,
                end: end.at.clone(),
                end_index: end.range.end,
            }
        }
    }
}

/// The transcript's one selection. Holds the focus handle that receives ⌘C:
/// a non-empty selection takes keyboard focus (AppKit behaviour); an empty
/// one leaves it where it was, so a stray click never strands the composer.
pub struct TextSelection {
    pub focus_handle: FocusHandle,
    model: Model,
    /// How the last left-button release on bubble text ended: `Some(true)` a
    /// plain click (press and release at one point), `Some(false)` a drag;
    /// `None` from the next press on. Every text element's mouse-up listener
    /// runs for one release, last painted first, and the first one finishes
    /// the gesture — the element under the pointer may come later and reads
    /// the shared answer here instead of finding no gesture left to finish
    /// (Jacob, 2026-10-27: links in a bubble with text after them never opened).
    release: Option<bool>,
    /// The pointer over the transcript's mention chips (hover / pressed), by
    /// bubble key and the chip's byte range in that bubble's plain text.
    pub chips: mention::ChipPointer<ChipKey>,
    /// Screenshot rig (`ARBOS_RIG_CHIP=hover|pressed`): every chip paints in
    /// that pointer state. The EC2 Mac's VNC session cannot hold a button
    /// down for a capture, so the rig asks.
    rig_chip: Option<mention::ChipState>,
    /// The `||spoilers||` this viewer has clicked open, by bubble key and the
    /// spoiler's start in the bubble's plain text. Per viewer, per launch:
    /// nothing is stored ([feedback chat-markdown-mac-2026-10-27]).
    revealed: Rc<HashSet<SpoilerKey>>,
    /// The cursor the last painted bubble text asked for (tests).
    #[cfg(test)]
    pub(crate) cursor: CursorStyle,
    /// Every link a click opened, in order (tests).
    #[cfg(test)]
    pub(crate) opened: Vec<String>,
}

/// Names one spoiler across frames: the bubble and where the spoiler's text
/// starts in the bubble's plain text.
pub type SpoilerKey = (SharedString, usize);

/// Names one chip across frames: the bubble and the chip's range in the
/// bubble's plain text.
pub type ChipKey = (SharedString, Range<usize>);

impl TextSelection {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            model: Model::default(),
            release: None,
            chips: mention::ChipPointer::default(),
            rig_chip: std::env::var("ARBOS_RIG_CHIP")
                .ok()
                .and_then(|s| mention::ChipState::parse(&s)),
            revealed: Rc::new(HashSet::new()),
            #[cfg(test)]
            cursor: CursorStyle::Arrow,
            #[cfg(test)]
            opened: Vec::new(),
        }
    }

    /// Show or hide one spoiler for this viewer.
    pub fn toggle_spoiler(&mut self, key: SpoilerKey, cx: &mut Context<Self>) {
        let revealed = Rc::make_mut(&mut self.revealed);
        if !revealed.remove(&key) {
            revealed.insert(key);
        }
        cx.notify();
    }

    /// Whether the viewer has opened the spoiler at `key`.
    #[cfg(test)]
    pub fn spoiler_revealed(&self, key: &SpoilerKey) -> bool {
        self.revealed.contains(key)
    }

    /// The selected text when the selection lives in one bubble. A selection
    /// across bubbles needs the texts in between, which only the transcript
    /// has: see [`TextSelection::span`].
    pub fn selected_text(&self) -> Option<String> {
        self.model.selected_text()
    }

    /// The two ends of a non-empty selection, in transcript order.
    pub fn span(&self) -> Option<Span> {
        self.model.span()
    }

    /// Drop a settled (not dragging) selection; `true` when something changed.
    pub fn clear_settled(&mut self) -> bool {
        self.model.clear_settled()
    }

    /// Drop any selection (drag or settled).
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
            .is_some_and(|a| a.dragging && !a.is_empty())
    }

    /// `true` from a press on bubble text until the button comes up: the
    /// pointer belongs to the selection, whichever way it moves (Grok: a drag
    /// from the bottom of a message upward selects; the transcript must not
    /// take it for drag-scroll — [feedback bubble-selection-2026-10-24]).
    pub fn is_dragging(&self) -> bool {
        self.model.is_dragging()
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

    /// Screenshot rig: a drag from the second word of bubble `from` to the
    /// second-to-last word of bubble `to` (two messages, the end bubbles cut).
    pub fn rig_select_across(&mut self, from: BubbleAt, to: BubbleAt) {
        let second_word = |t: &str| t.find(' ').map(|i| i + 1).unwrap_or(0);
        let before_last_word = |t: &str| t.trim_end().rfind(' ').unwrap_or(t.len());
        let (start, end) = (second_word(&from.text), before_last_word(&to.text));
        self.model.begin_at(from, start, 1, false);
        self.model.extend_in(&to, end);
        self.model.finish_drag();
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
    /// The selected text when both ends are in one bubble.
    pub(crate) fn selected_text(&self) -> Option<String> {
        let active = self.active.as_ref()?;
        if !active.anchor.same_bubble(&active.head) {
            return None;
        }
        let range = active.range();
        if range.is_empty() {
            return None;
        }
        active.anchor.at.text.get(range).map(str::to_string)
    }

    pub(crate) fn span(&self) -> Option<Span> {
        let active = self.active.as_ref()?;
        (!active.is_empty()).then(|| active.span())
    }

    /// `true` while a mouse drag is in progress (in any bubble).
    fn is_dragging(&self) -> bool {
        self.active.as_ref().is_some_and(|a| a.dragging)
    }

    /// `true` when the gesture's press landed in bubble `key` at a byte inside
    /// `span` — the element whose link or chip a plain click may open.
    fn pressed_in(&self, key: &SharedString, span: &Range<usize>) -> bool {
        self.active
            .as_ref()
            .is_some_and(|a| a.anchor.at.key == *key && span.contains(&a.anchor.range.start))
    }

    fn has_range(&self) -> bool {
        self.active.as_ref().is_some_and(|a| !a.is_empty())
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

    /// Mouse down at byte `index` of bubble `at`: one click arms a drag,
    /// shift extends, two clicks take the word, three the paragraph.
    pub(crate) fn begin_at(&mut self, at: BubbleAt, index: usize, click_count: usize, shift: bool) {
        self.proposal = None;
        if shift {
            if let Some(a) = self.active.as_mut() {
                let range = snap(&at.text, index, a.granularity);
                a.head = Pos { at, range };
                a.dragging = true;
                return;
            }
        }
        let granularity = match click_count {
            0 | 1 => Granularity::Character,
            2 => Granularity::Word,
            _ => Granularity::Paragraph,
        };
        let range = snap(&at.text, index, granularity);
        let anchor = Pos { at, range };
        self.active = Some(Active {
            head: anchor.clone(),
            anchor,
            granularity,
            dragging: true,
        });
    }

    /// [`Model::begin_at`] for a bubble at transcript order 0 (one-bubble
    /// callers and tests).
    pub(crate) fn begin(
        &mut self,
        key: SharedString,
        text: SharedString,
        index: usize,
        click_count: usize,
        shift: bool,
    ) {
        self.begin_at(
            BubbleAt {
                order: 0,
                key,
                text,
            },
            index,
            click_count,
            shift,
        )
    }

    /// Drag to byte `index` of bubble `at`; `true` when the range changed.
    pub(crate) fn extend_in(&mut self, at: &BubbleAt, index: usize) -> bool {
        let Some(a) = self.active.as_mut() else {
            return false;
        };
        let range = snap(&at.text, index, a.granularity);
        if a.head.at == *at && range == a.head.range {
            return false;
        }
        a.head = Pos {
            at: at.clone(),
            range,
        };
        true
    }

    /// Drag to byte `index` of the bubble the drag started in.
    pub(crate) fn extend_to(&mut self, index: usize) -> bool {
        let Some(at) = self.active.as_ref().map(|a| a.anchor.at.clone()) else {
            return false;
        };
        self.extend_in(&at, index)
    }

    /// One text element's answer to a mouse move at `position`: the bubble it
    /// belongs to, the index it would extend to, and how far the pointer is
    /// from it. The nearest element of the same move wins — across bubbles
    /// too; `true` when the range changed.
    fn propose_in(
        &mut self,
        position: Point<Pixels>,
        at: &BubbleAt,
        index: usize,
        distance: Distance,
    ) -> bool {
        match self.proposal {
            Some((p, best)) if p == position && best <= distance => return false,
            _ => self.proposal = Some((position, distance)),
        }
        self.extend_in(at, index)
    }

    /// [`Model::propose_in`] for the bubble the drag started in.
    #[cfg(test)]
    fn propose(&mut self, position: Point<Pixels>, index: usize, distance: Distance) -> bool {
        let Some(at) = self.active.as_ref().map(|a| a.anchor.at.clone()) else {
            return false;
        };
        self.propose_in(position, &at, index, distance)
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
        a.is_empty()
    }
}

/// The site pictures the bubbles' named links draw in their icon cell
/// ([`crate::ui::markdown::LINK_ICON_CELL`]), by host: `Some(image)` once the
/// relay answered with one, `None` when the site has none (a neutral rounded
/// square stands in); a host that is still loading is absent and draws the
/// same placeholder.
#[derive(Default)]
pub struct LinkIcons {
    pub by_host: HashMap<String, Option<Arc<RenderImage>>>,
}

impl LinkIcons {
    pub fn empty() -> Rc<Self> {
        Rc::new(Self::default())
    }

    pub fn picture(&self, host: &str) -> Option<Arc<RenderImage>> {
        self.by_host.get(host).cloned().flatten()
    }
}

/// The icon drawn before a named link, in points (Grok Bot's is 16 at its 15 pt body).
pub const LINK_ICON: f32 = 14.0;
/// The gap the icon keeps to the link text, inside the cell.
pub const LINK_ICON_GAP: f32 = 4.0;

/// One named link's icon cell inside a [`SelectableText`]: the cell's bytes
/// (element-local) and the picture, when the site has one.
pub struct LinkIcon {
    pub range: Range<usize>,
    pub picture: Option<Arc<RenderImage>>,
}

/// The per-render snapshot of the selection, plus what a text block needs to
/// draw its highlight and wire its mouse events.
#[derive(Clone)]
pub struct Selectable {
    entity: Entity<TextSelection>,
    /// The selection as it stands this frame (both ends), for the highlights.
    current: Option<Span>,
    /// Transcript order of the bubbles built from this (the row index of the
    /// message's first row); see [`Selectable::ordered`].
    order: usize,
    color: Hsla,
    /// Who `@name` runs resolve to, and the accent the chips take (name
    /// text; the pill is the accent at `mention::PILL_ALPHA`, brighter under
    /// the pointer).
    mentions: Rc<Mentions>,
    chip_accent: Hsla,
    /// Site pictures for the named links, and the placeholder's colour.
    link_icons: Rc<LinkIcons>,
    icon_placeholder: Hsla,
    /// Search-in-chat marks for this bubble's words, while the bar is up.
    marks: Option<Rc<SearchMarks>>,
    /// The spoilers open this frame.
    revealed: Rc<HashSet<SpoilerKey>>,
}

/// What ⌘F Search-in-chat paints on a hit's words ([feedback
/// search-in-chat-mac-2026-10-26]): every hit's words on the accent wash,
/// the revealed hit's in Find's yellow with dark text.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchMarks {
    /// Folded keyword (`chat_search::hit_ranges`); empty = sender-only query, no word marks.
    pub needle: String,
    pub current: bool,
    pub wash: Hsla,
    pub current_wash: Hsla,
    pub current_text: Hsla,
}

impl Selectable {
    pub fn new(entity: &Entity<TextSelection>, color: Hsla, cx: &App) -> Self {
        let selection = entity.read(cx);
        let current = selection.model.span();
        let revealed = selection.revealed.clone();
        Self {
            entity: entity.clone(),
            current,
            order: 0,
            color,
            mentions: Mentions::empty(),
            chip_accent: Hsla::default(),
            link_icons: LinkIcons::empty(),
            icon_placeholder: Hsla::default(),
            marks: None,
            revealed,
        }
    }

    /// The words a Search-in-chat query hits are marked in every text of
    /// the bubbles built from this.
    pub fn with_marks(mut self, marks: Option<SearchMarks>) -> Self {
        self.marks = marks.map(Rc::new);
        self
    }

    /// Named links draw their site's picture (or `placeholder`, a neutral
    /// rounded square) in their icon cell.
    pub fn with_link_icons(mut self, icons: Rc<LinkIcons>, placeholder: Hsla) -> Self {
        self.link_icons = icons;
        self.icon_placeholder = placeholder;
        self
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

    /// Where in the transcript the bubbles built from this sit: a drag that
    /// crosses bubbles orders its two ends by this (then by key).
    pub fn ordered(mut self, order: usize) -> Self {
        self.order = order;
        self
    }

    /// Scope to one bubble: `key` names it, `text` is its whole rendered
    /// plain text (every block, in order). The bubble's text elements are
    /// then built with [`BubbleSelectable::text`] at their byte offset.
    pub fn bubble(
        &self,
        key: impl Into<SharedString>,
        text: impl Into<SharedString>,
    ) -> BubbleSelectable {
        let at = BubbleAt {
            order: self.order,
            key: key.into(),
            text: text.into(),
        };
        // An end bubble whose text changed since the gesture (streaming) is
        // not highlighted: its offsets no longer mean the same glyphs.
        let range = self.current.as_ref().and_then(|span| {
            let fresh = (at.key != span.start.key || at.text == span.start.text)
                && (at.key != span.end.key || at.text == span.end.text);
            fresh
                .then(|| span.range_in(&at))
                .flatten()
                .filter(|r| !r.is_empty())
        });
        BubbleSelectable {
            entity: self.entity.clone(),
            at,
            range,
            color: self.color,
            mentions: self.mentions.clone(),
            chip_accent: self.chip_accent,
            link_icons: self.link_icons.clone(),
            icon_placeholder: self.icon_placeholder,
            marks: self.marks.clone(),
            revealed: self.revealed.clone(),
        }
    }
}

/// [`Selectable`] scoped to one bubble.
#[derive(Clone)]
pub struct BubbleSelectable {
    entity: Entity<TextSelection>,
    at: BubbleAt,
    /// The part of the current selection that falls in this bubble.
    range: Option<Range<usize>>,
    color: Hsla,
    mentions: Rc<Mentions>,
    chip_accent: Hsla,
    link_icons: Rc<LinkIcons>,
    icon_placeholder: Hsla,
    marks: Option<Rc<SearchMarks>>,
    revealed: Rc<HashSet<SpoilerKey>>,
}

impl BubbleSelectable {
    /// Whether the spoiler whose text starts at byte `at` of this bubble's
    /// plain text is open for the viewer.
    pub fn spoiler_revealed(&self, at: usize) -> bool {
        self.revealed.contains(&(self.at.key.clone(), at))
    }
}

/// One mention chip inside a [`SelectableText`]: the `@Name` run (element-local
/// bytes), what it shows and where a click goes.
pub struct Chip {
    pub range: Range<usize>,
    /// How the run divides up: the pad cells and the picture cell.
    pub text: mention::ChipText,
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
            self.at.text.get(at..at + text.len()),
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
            let chip_text = mention::chip_text(&text[range.clone()]);
            runs = chip_runs(runs, range.clone(), chip_text.cell, accent);
            chips.push(Chip {
                key: (self.at.key.clone(), at + range.start..at + range.end),
                range,
                text: chip_text,
                mention,
                mentions: self.mentions.clone(),
                accent,
            });
        }
        // Search-in-chat: the hit words on the wash, the revealed hit's in
        // Find's yellow; the selection highlight then splits them like any run.
        let runs = match self.marks.as_deref() {
            Some(marks) if !marks.needle.is_empty() => {
                let (bg, fg) = if marks.current {
                    (marks.current_wash, Some(marks.current_text))
                } else {
                    (marks.wash, None)
                };
                crate::chat_search::hit_ranges(&text, &marks.needle)
                    .into_iter()
                    .fold(runs, |runs, range| mark(runs, range, bg, fg))
            }
            _ => runs,
        };
        let runs = match &self.range {
            Some(range) => highlight(runs, local_range(range, at, text.len()), self.color),
            None => runs,
        };
        SelectableText {
            styled: StyledText::new(text.clone()).with_runs(runs),
            bubble: self.at.clone(),
            span: at..at + text.len(),
            at,
            links: Vec::new(),
            spoilers: Vec::new(),
            chips,
            icons: Vec::new(),
            link_icons: self.link_icons.clone(),
            icon_placeholder: self.icon_placeholder,
            selection: self.entity.clone(),
        }
    }
}

/// Recolour the `@Name` run for a chip: the `@` (first byte) transparent, the
/// name in `accent`, no underline and no run background — the pill is the
/// chip's only background (a code span's box behind the name stacked a second,
/// square one inside it); everything else untouched.
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
                background_color: if in_chip { None } else { run.background_color },
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
    mark(runs, range, color, None)
}

/// Split `runs` at `range` and paint the inside: `background` behind it and,
/// when given, `color` on its glyphs (a search hit's words; the selection
/// highlight keeps the glyph colour).
pub fn mark(
    runs: Vec<TextRun>,
    range: Range<usize>,
    background: Hsla,
    color: Option<Hsla>,
) -> Vec<TextRun> {
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
                    Some(background)
                } else {
                    run.background_color
                },
                color: match color {
                    Some(c) if inside => c,
                    _ => run.color,
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
    /// The bubble this element belongs to (with its whole plain text), and
    /// where this element's text starts in it.
    bubble: BubbleAt,
    at: usize,
    /// `at..at + text.len()`: the bubble bytes this element covers.
    span: Range<usize>,
    styled: StyledText,
    links: Vec<(Range<usize>, String)>,
    /// Spoiler ranges (element-local) and each one's key: a plain click
    /// inside one shows or hides it.
    spoilers: Vec<(Range<usize>, SpoilerKey)>,
    chips: Vec<Chip>,
    /// Named links' icon cells, with the site picture when it is known.
    icons: Vec<LinkIcon>,
    link_icons: Rc<LinkIcons>,
    icon_placeholder: Hsla,
    selection: Entity<TextSelection>,
}

impl SelectableText {
    /// Clickable link ranges (element-local): a plain click (no drag) inside
    /// one opens it.
    pub fn with_links(mut self, links: Vec<(Range<usize>, String)>) -> Self {
        self.links = links;
        self
    }

    /// Spoiler ranges (element-local) with the byte each spoiler starts at
    /// in the bubble's plain text: a plain click toggles the spoiler.
    pub fn with_spoilers(mut self, spoilers: Vec<(Range<usize>, usize)>) -> Self {
        let key = self.bubble.key.clone();
        self.spoilers = spoilers
            .into_iter()
            .map(|(range, start)| (range, (key.clone(), start)))
            .collect();
        self
    }

    /// The icon cells of the named links (element-local byte range of each
    /// cell, the site's host): drawn with the host's picture from the bubble's
    /// [`LinkIcons`], else the placeholder.
    pub fn with_icons(mut self, icons: Vec<(Range<usize>, String)>) -> Self {
        let pictures = self.link_icons.clone();
        self.icons = icons
            .into_iter()
            .map(|(range, host)| LinkIcon {
                range,
                picture: pictures.picture(&host),
            })
            .collect();
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
        // The text may have moved under a still pointer (the pane opening
        // reflows the bubbles): a hot chip of ours that is no longer under
        // the pointer cools now, not at the next mouse move.
        let under_pointer = hitbox
            .is_hovered(window)
            .then(|| chip_at(&chips, &layout, window.mouse_position()))
            .flatten()
            .map(|c| c.key.clone());
        let (key, span) = (self.bubble.key.clone(), self.span.clone());
        selection.update(cx, |s, _| {
            s.chips.tidy(now);
            // Ours: a chip in this element, or a chip this bubble no longer
            // has at that byte range (the text changed).
            let mine = s
                .chips
                .hot()
                .is_some_and(|(k, range)| *k == key && span.contains(&range.start));
            if mine && under_pointer.is_none() {
                s.chips.hover(None, now);
            }
        });
        let rig = selection.read(cx).rig_chip;
        for chip in chips.iter() {
            let (alpha, still) = match rig {
                Some(state) if chip.clickable() => (state.alpha(), false),
                _ => selection.read(cx).chips.alpha(&chip.key, now, snap),
            };
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
        for icon in &self.icons {
            paint_link_icon(&layout, icon, self.icon_placeholder, window);
        }
        let links = std::mem::take(&mut self.links);
        let spoilers = std::mem::take(&mut self.spoilers);
        let focus_handle = selection.read(cx).focus_handle.clone();
        let (bubble, at) = (self.bubble.clone(), self.at);
        let key = bubble.key.clone();

        let over_link = hitbox.is_hovered(window)
            && layout
                .index_for_position(window.mouse_position())
                .is_ok_and(|ix| {
                    links.iter().any(|(r, _)| r.contains(&ix))
                        || spoilers.iter().any(|(r, _)| r.contains(&ix))
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
            let (key, span) = (key.clone(), span.clone());
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
                    .is_some_and(|(k, range)| *k == key && span.contains(&range.start));
                if over.is_none() && !mine {
                    return;
                }
                if selection.update(cx, |s, _| s.chips.hover(over, Instant::now())) {
                    window.refresh();
                }
            }
        });

        window.on_mouse_event({
            let (hitbox, layout, selection, focus_handle) = (
                hitbox.clone(),
                layout.clone(),
                selection.clone(),
                focus_handle.clone(),
            );
            let (bubble, chips) = (bubble.clone(), chips.clone());
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                // A new press: the last release's answer is spent.
                if selection.read(cx).release.is_some() {
                    selection.update(cx, |s, _| s.release = None);
                }
                // Control-click is the bubble's copy; right-click its menu.
                if event.modifiers.control || !hitbox.is_hovered(window) {
                    return;
                }
                let index = at + closest_index(&layout, event.position);
                if let Some(chip) = chip_at(&chips, &layout, event.position) {
                    if selection.update(cx, |s, _| s.chips.press(chip.key.clone(), Instant::now()))
                    {
                        window.refresh();
                    }
                }
                let selected = selection.update(cx, |s, cx| {
                    s.model.begin_at(
                        bubble.clone(),
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

        // A drag that started on bubble text — this bubble's or another's —
        // proposes its nearest index on every move, whichever way the pointer
        // went: up, down, into the next bubble. (The old axis lock that handed
        // a vertical pan to drag-scroll made a bottom-up drag select nothing;
        // the transcript now leaves a press on text to the selection.)
        window.on_mouse_event({
            let (layout, selection, focus_handle, bubble) = (
                layout.clone(),
                selection.clone(),
                focus_handle.clone(),
                bubble.clone(),
            );
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !selection.read(cx).model.is_dragging() {
                    return;
                }
                if event.pressed_button != Some(MouseButton::Left) {
                    selection.update(cx, |s, cx| {
                        s.model.finish_drag();
                        cx.notify();
                    });
                    return;
                }
                let distance = distance_to(&layout.bounds(), event.position);
                let index = at + drag_index(&layout, event.position, distance);
                let selected = selection.update(cx, |s, cx| {
                    if s.model.propose_in(event.position, &bubble, index, distance) {
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
            let (bubble, span) = (bubble.clone(), span.clone());
            move |event: &MouseUpEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                // The first element to see the release finishes the gesture
                // and keeps the answer; every element after it reads that.
                let Some(plain_click) = selection.update(cx, |s, cx| {
                    if s.model.is_dragging() {
                        let plain = s.model.finish_drag();
                        s.release = Some(plain);
                        if s.chips.release(Instant::now()).is_some() {
                            window.refresh();
                        }
                        cx.notify();
                    }
                    s.release
                }) else {
                    return;
                };
                // A plain click opens what is under the pointer, in the
                // element the press began in (the one whose text it is).
                if plain_click
                    && event.click_count == 1
                    && hitbox.is_hovered(window)
                    && selection.read(cx).model.pressed_in(&bubble.key, &span)
                {
                    let index = layout.index_for_position(event.position).ok();
                    if let Some(chip) = chip_at(&chips, &layout, event.position) {
                        chip.mentions.open(&chip.mention, cx);
                    } else if let Some((_, key)) =
                        index.and_then(|ix| spoilers.iter().find(|(r, _)| r.contains(&ix)))
                    {
                        // A hidden spoiler opens; its link (if any) waits for
                        // the next click, once the text can be read.
                        let key = key.clone();
                        selection.update(cx, |s, cx| s.toggle_spoiler(key, cx));
                    } else if let Some((_, href)) =
                        index.and_then(|ix| links.iter().find(|(r, _)| r.contains(&ix)))
                    {
                        #[cfg(test)]
                        selection.update(cx, |s, _| s.opened.push(href.clone()));
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

/// The chip's pill: one rounded quad per line fragment the chip's run
/// occupies, `PILL_PAD_Y` taller than the line box above and below.
///
/// Across the line the pill stays inside the run, because the run reserves a
/// [`mention::PAD`] cell at each end for it; only an end with no pad cell
/// falls back to the painted `PILL_PAD_X` inset. A wrapped chip takes its
/// leading inset on the first fragment and its trailing one on the last, so
/// neither half reaches past the edge of its line. Painted before the text.
fn paint_chip_pill(layout: &TextLayout, chip: &Chip, color: Hsla, window: &mut Window) {
    let fragments = chip_fragments(layout, &chip.range);
    let last = fragments.len().saturating_sub(1);
    for (i, (start, end, top)) in fragments.into_iter().enumerate() {
        let line_h = layout.line_height();
        let lead = if i == 0 && chip.text.lead == 0 {
            px(mention::PILL_PAD_X)
        } else {
            px(0.0)
        };
        let trail = if i == last && chip.text.tail == 0 {
            px(mention::PILL_PAD_X)
        } else {
            px(0.0)
        };
        let bounds = Bounds::new(
            point(start - lead, top - px(mention::PILL_PAD_Y)),
            size(
                end - start + lead + trail,
                line_h + px(mention::PILL_PAD_Y * 2.0),
            ),
        );
        #[cfg(test)]
        PAINTED_CHIPS.with(|p| {
            p.borrow_mut().push(PaintedChip::Pill {
                bounds,
                text: (start, end),
                line: (top, line_h),
            })
        });
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
/// The site's picture in a named link's icon cell: a rounded square
/// [`LINK_ICON`] tall (smaller when the cell or the line is), left-aligned in
/// the cell so [`LINK_ICON_GAP`] stays before the name; the placeholder is the
/// same square in a neutral colour. Nothing is drawn when the cell wrapped.
fn paint_link_icon(layout: &TextLayout, icon: &LinkIcon, placeholder: Hsla, window: &mut Window) {
    let Some(at) = layout.position_for_index(icon.range.start) else {
        return;
    };
    let Some(after) = layout.position_for_index(icon.range.end) else {
        return;
    };
    if after.y != at.y {
        return;
    }
    let line_h = layout.line_height();
    let cell_w = after.x - at.x;
    let d = px(LINK_ICON).min(line_h).min(cell_w - px(LINK_ICON_GAP));
    if d <= px(4.0) {
        return;
    }
    let bounds = Bounds::new(point(at.x, at.y + (line_h - d) / 2.0), size(d, d));
    let radii = Corners::all(d * 0.25);
    #[cfg(test)]
    PAINTED_LINK_ICONS.with(|n| n.set(n.get() + 1));
    match &icon.picture {
        Some(image) => {
            let _ = window.paint_image(bounds, radii, image.clone(), 0, false);
        }
        None => window.paint_quad(gpui::quad(
            bounds,
            radii,
            placeholder,
            gpui::Edges::all(px(0.0)),
            gpui::transparent_black(),
            gpui::BorderStyle::default(),
        )),
    }
}

#[cfg(test)]
thread_local! {
    /// How many link icon cells were painted (picture or placeholder), for the
    /// headless render tests.
    pub static PAINTED_LINK_ICONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn paint_chip_picture(layout: &TextLayout, chip: &Chip, window: &mut Window, cx: &mut App) {
    let Some(at) = layout.position_for_index(chip.range.start + chip.text.lead) else {
        return;
    };
    let Some(after_at) = layout.position_for_index(chip.range.start + chip.text.cell) else {
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
    #[cfg(test)]
    PAINTED_CHIPS.with(|p| {
        p.borrow_mut().push(PaintedChip::Disc {
            bounds: Bounds::new(origin, size(d, d)),
            name_x: after_at.x,
        })
    });
    paint_picture(
        &chip.mention.picture,
        Bounds::new(origin, size(d, d)),
        window,
        cx,
    );
}

/// What a chip painted, for the headless render tests: each pill (one per
/// line fragment) with the `@Name` run's span and the line box it sits on,
/// and the disc with where the name starts.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub enum PaintedChip {
    Pill {
        bounds: Bounds<Pixels>,
        text: (Pixels, Pixels),
        line: (Pixels, Pixels),
    },
    Disc {
        bounds: Bounds<Pixels>,
        name_x: Pixels,
    },
}

#[cfg(test)]
thread_local! {
    pub static PAINTED_CHIPS: std::cell::RefCell<Vec<PaintedChip>> = const { std::cell::RefCell::new(Vec::new()) };
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
        mention::Picture::Neutral { name } => {
            let (disc, ink) = avatars::neutral_colors(crate::ui::theme::Theme::of(cx));
            window.paint_quad(gpui::quad(
                bounds,
                radii,
                disc,
                gpui::Edges::all(px(0.0)),
                gpui::transparent_black(),
                gpui::BorderStyle::default(),
            ));
            let text = arbos_core::avatar::leaf::initial(name);
            let font_size = d * avatars::INITIALS_SCALE;
            let mut font = window.text_style().font();
            font.weight = FontWeight::SEMIBOLD;
            let run = TextRun {
                len: text.len(),
                font,
                color: ink,
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
        // Shift-click in a later bubble extends across to it (macOS / Grok).
        sel.begin_at(at(1, "other", "zeta"), 1, 1, true);
        let span = sel.span().expect("a span across two bubbles");
        assert_eq!(span.start.key.as_ref(), "k");
        assert_eq!(span.start_index, 0);
        assert_eq!(span.end.key.as_ref(), "other");
        assert_eq!(
            span.end_index, 4,
            "the double-click's word granularity still applies"
        );
        assert_eq!(sel.selected_text(), None, "one-bubble copy has no answer");
    }

    fn at(order: usize, key: &str, text: &str) -> BubbleAt {
        BubbleAt {
            order,
            key: key.to_string().into(),
            text: text.to_string().into(),
        }
    }

    /// Jacob (2026-10-24, [feedback bubble-selection-2026-10-24]): a drag
    /// from one message into the next selects the end of the first, the whole
    /// of anything between, and the start of the last — and the same drag
    /// made from the bottom up gives the same span.
    #[test]
    fn a_drag_across_bubbles_spans_them_in_transcript_order() {
        let (a, b, c) = (
            at(0, "a", "First message here."),
            at(3, "b", "Second, in the middle."),
            at(7, "c", "Third and last one."),
        );
        // Top down: press at "message" in a, drag to after "Third" in c.
        let mut sel = Model::default();
        sel.begin_at(a.clone(), 6, 1, false);
        assert!(sel.is_dragging());
        assert!(!sel.has_range());
        let p = point(px(10.0), px(300.0));
        assert!(sel.propose_in(p, &c, 5, (0.0, 0.0)));
        assert!(sel.has_range());
        let down = sel.span().expect("span");
        assert_eq!(
            down,
            Span {
                start: a.clone(),
                start_index: 6,
                end: c.clone(),
                end_index: 5
            }
        );
        assert_eq!(down.range_in(&a), Some(6..a.text.len()));
        assert_eq!(
            down.range_in(&b),
            Some(0..b.text.len()),
            "the middle bubble, whole"
        );
        assert_eq!(down.range_in(&c), Some(0..5));
        assert_eq!(down.range_in(&at(9, "d", "after")), None);
        assert!(!down.one_bubble());
        assert!(!sel.finish_drag(), "not a plain click");

        // Bottom up: press after "Third" in c, drag up to "message" in a.
        let mut sel = Model::default();
        sel.begin_at(c.clone(), 5, 1, false);
        assert!(sel.propose_in(p, &a, 6, (0.0, 0.0)));
        assert_eq!(sel.span().expect("span"), down, "same span either way");
        // Back into the anchor's own bubble: an ordinary one-bubble range.
        assert!(sel.propose_in(point(px(10.0), px(310.0)), &c, 0, (0.0, 0.0)));
        assert_eq!(sel.selected_text().as_deref(), Some("Third"));
    }

    /// Two bubbles rendered one above the other; a real pointer drag from
    /// the end of the lower one up into the upper one selects across both,
    /// bottom-up (the gesture Jacob found broken).
    struct TwoBubbles {
        selection: Entity<TextSelection>,
    }

    const UPPER: &str = "The upper message, first in the chat.";
    const LOWER: &str = "The lower message, sent after it.";

    impl gpui::Render for TwoBubbles {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::{div, InteractiveElement as _, ParentElement as _, Styled as _};
            let selectable = Selectable::new(&self.selection, Hsla::default(), cx);
            let upper = selectable.clone().ordered(0).bubble("upper", UPPER);
            let lower = selectable.ordered(1).bubble("lower", LOWER);
            div()
                .w(px(500.0))
                .h(px(200.0))
                .p(px(20.0))
                .text_size(px(15.0))
                .flex()
                .flex_col()
                .gap(px(30.0))
                .child(
                    div()
                        .debug_selector(|| "upper-text".into())
                        .child(upper.text(0, UPPER, vec![run(UPPER.len())])),
                )
                .child(
                    div()
                        .debug_selector(|| "lower-text".into())
                        .child(lower.text(0, LOWER, vec![run(LOWER.len())])),
                )
        }
    }

    #[gpui::test]
    fn a_bottom_up_drag_selects_across_two_rendered_bubbles(cx: &mut gpui::TestAppContext) {
        let selection = cx.update(|cx| cx.new(TextSelection::new));
        let (_view, vcx) = cx.add_window_view(|_, _| TwoBubbles {
            selection: selection.clone(),
        });
        vcx.run_until_parked();
        let upper = vcx.debug_bounds("upper-text").expect("upper drawn");
        let lower = vcx.debug_bounds("lower-text").expect("lower drawn");
        let mid = |b: Bounds<Pixels>| b.origin.y + b.size.height / 2.0;
        // Press inside the lower line, drag straight up (a vertical move — no
        // sideways motion first) into the upper line, release.
        let press = point(lower.left() + px(90.0), mid(lower));
        let over_gap = point(lower.left() + px(90.0), lower.top() - px(10.0));
        let target = point(upper.left() + px(40.0), mid(upper));
        vcx.simulate_mouse_down(press, MouseButton::Left, gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(selection.read_with(&*vcx, |s, _| s.is_dragging()));
        vcx.simulate_mouse_move(
            over_gap,
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        vcx.run_until_parked();
        vcx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        vcx.run_until_parked();
        vcx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        vcx.run_until_parked();
        let span = selection
            .read_with(&*vcx, |s, _| s.span())
            .expect("a selection across the two bubbles");
        assert_eq!(
            span.start.key.as_ref(),
            "upper",
            "starts in the upper bubble"
        );
        assert_eq!(span.end.key.as_ref(), "lower", "ends in the lower bubble");
        assert!(span.start_index > 0 && span.start_index < UPPER.len());
        assert!(span.end_index > 0 && span.end_index < LOWER.len());
        assert!(!selection.read_with(&*vcx, |s, _| s.is_dragging()));
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

    /// The pane opening reflows the bubble: a chip that moves out from under
    /// a still pointer cools at the next paint, not at the next mouse move.
    #[gpui::test]
    fn a_chip_that_moves_away_from_a_still_pointer_cools(cx: &mut gpui::TestAppContext) {
        use crate::ui::mention::ChipState;
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
        let on_chip = point(
            bounds.origin.x + px(12.0),
            bounds.origin.y + bounds.size.height / 2.0,
        );
        vcx.simulate_mouse_move(on_chip, None, gpui::Modifiers::default());
        vcx.run_until_parked();
        selection.read_with(&*vcx, |s, _| {
            assert_eq!(s.chips.state(&("m1".into(), 0..6)), ChipState::Hover);
        });
        // The text changes under the pointer: the chip is now further right.
        view.update(vcx, |v, cx| {
            v.text = "moved along: @Comms".into();
            cx.notify();
        });
        vcx.run_until_parked();
        selection.read_with(&*vcx, |s, _| {
            assert_eq!(s.chips.hot(), None, "nothing under the pointer any more");
            assert_eq!(s.chips.state(&("m1".into(), 13..19)), ChipState::Rest);
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

    /// Jacob's bubble (2026-10-27, Mac v0.6.296): a bare URL as its own
    /// paragraph, then a second paragraph. The URL is long enough to wrap in
    /// the bubble; the link run and its click range cover the whole URL.
    const LINK_URL: &str =
        "https://treatment-perfume-marble-tournaments.trycloudflare.com/jacob/competitors.html";
    const LINK_TAIL: &str = "I checked it from outside and it loads. No zip or file card needed.";

    /// Two paragraphs of one bubble, as the transcript builds them: one
    /// `Selectable::bubble` with the whole plain text, one [`SelectableText`]
    /// per paragraph starting at its offset in it. The second paragraph is
    /// painted after the first.
    struct LinkBubble {
        selection: Entity<TextSelection>,
        width: Pixels,
    }

    impl gpui::Render for LinkBubble {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::{div, InteractiveElement as _, ParentElement as _, Styled as _};
            let plain = format!("{LINK_URL}\n{LINK_TAIL}");
            let selectable = Selectable::new(&self.selection, Hsla::default(), cx);
            let bubble = selectable.bubble("m1", plain);
            let url: SharedString = LINK_URL.into();
            let tail: SharedString = LINK_TAIL.into();
            div()
                .w(self.width)
                .p(px(20.0))
                .text_size(px(15.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div().debug_selector(|| "link-text".into()).child(
                        bubble
                            .text(0, url.clone(), vec![run(url.len())])
                            .with_links(vec![(0..url.len(), LINK_URL.to_string())]),
                    ),
                )
                .child(
                    div()
                        .debug_selector(|| "tail-text".into())
                        .child(bubble.text(url.len() + 1, tail.clone(), vec![run(tail.len())])),
                )
        }
    }

    /// The bubble at `width`: its selection, the window, the link paragraph's
    /// bounds and the line height.
    fn link_bubble(
        cx: &mut gpui::TestAppContext,
        width: f32,
    ) -> (
        Entity<TextSelection>,
        &mut gpui::VisualTestContext,
        Bounds<Pixels>,
        Pixels,
    ) {
        let selection = cx.update(|cx| cx.new(TextSelection::new));
        let (_view, vcx) = cx.add_window_view(|_, _| LinkBubble {
            selection: selection.clone(),
            width: px(width),
        });
        vcx.run_until_parked();
        let bounds = vcx.debug_bounds("link-text").expect("link text drawn");
        let line_h = vcx.update(|window, _| window.line_height());
        (selection, vcx, bounds, line_h)
    }

    fn opened(selection: &Entity<TextSelection>, vcx: &gpui::VisualTestContext) -> Vec<String> {
        selection.read_with(vcx, |s, _| s.opened.clone())
    }

    /// Jacob (2026-10-27, P0, Mac v0.6.296): a click on a link in a bubble
    /// that has more text after it opened nothing. Every paragraph's mouse-up
    /// ran in turn and the last-painted one (the tail) finished the gesture
    /// first, so the link's own paragraph saw no press to act on. The answer
    /// of the release is shared now: the paragraph under the pointer opens
    /// its link.
    #[gpui::test]
    fn a_click_on_a_link_opens_it_when_text_follows_it(cx: &mut gpui::TestAppContext) {
        let (selection, vcx, bounds, line_h) = link_bubble(cx, 1200.0);
        assert!(
            bounds.size.height < line_h * 1.5,
            "a wide bubble keeps the URL on one line: {bounds:?}"
        );
        let y = bounds.origin.y + bounds.size.height / 2.0;
        vcx.simulate_click(
            point(bounds.origin.x + px(40.0), y),
            gpui::Modifiers::default(),
        );
        vcx.run_until_parked();
        assert_eq!(opened(&selection, vcx), vec![LINK_URL.to_string()]);
        assert_eq!(cx.opened_url().as_deref(), Some(LINK_URL));
    }

    /// The wrapped link: every line of the URL is the link. A click on each
    /// line opens it once; a click on the tail paragraph opens nothing.
    #[gpui::test]
    fn a_wrapped_link_opens_from_every_line(cx: &mut gpui::TestAppContext) {
        let (selection, vcx, bounds, line_h) = link_bubble(cx, 300.0);
        let lines = (bounds.size.height / line_h).round() as usize;
        assert!(
            lines >= 3,
            "the URL wraps onto at least three lines: {lines}"
        );
        let x = bounds.origin.x + px(30.0);
        for line in 0..lines {
            let y = bounds.origin.y + line_h * (line as f32 + 0.5);
            vcx.simulate_click(point(x, y), gpui::Modifiers::default());
            vcx.run_until_parked();
            assert_eq!(
                opened(&selection, vcx).len(),
                line + 1,
                "line {line} of {lines} opens the link"
            );
        }
        assert!(opened(&selection, vcx).iter().all(|u| u == LINK_URL));
        let tail = vcx.debug_bounds("tail-text").expect("tail drawn");
        vcx.simulate_click(
            point(tail.origin.x + px(30.0), tail.origin.y + line_h / 2.0),
            gpui::Modifiers::default(),
        );
        vcx.run_until_parked();
        assert_eq!(
            opened(&selection, vcx).len(),
            lines,
            "the tail is plain text"
        );
    }

    /// A drag that starts on the link selects text and opens nothing; a
    /// release over the link after a press elsewhere opens nothing either.
    #[gpui::test]
    fn a_drag_over_a_link_selects_and_does_not_open(cx: &mut gpui::TestAppContext) {
        let (selection, vcx, bounds, line_h) = link_bubble(cx, 300.0);
        let none = gpui::Modifiers::default();
        let on_link = point(bounds.origin.x + px(30.0), bounds.origin.y + line_h / 2.0);
        let further = point(bounds.origin.x + px(120.0), bounds.origin.y + line_h / 2.0);
        vcx.simulate_mouse_down(on_link, MouseButton::Left, none);
        vcx.simulate_mouse_move(further, Some(MouseButton::Left), none);
        vcx.simulate_mouse_up(further, MouseButton::Left, none);
        vcx.run_until_parked();
        assert!(
            selection.read_with(&*vcx, |s, _| s.selected_text().is_some()),
            "the drag selected text"
        );
        assert!(opened(&selection, vcx).is_empty(), "a drag is not a click");

        // Press on the tail, release on the link: no click on the link.
        let tail = vcx.debug_bounds("tail-text").expect("tail drawn");
        let on_tail = point(tail.origin.x + px(30.0), tail.origin.y + line_h / 2.0);
        vcx.simulate_mouse_down(on_tail, MouseButton::Left, none);
        vcx.simulate_mouse_move(on_link, Some(MouseButton::Left), none);
        vcx.simulate_mouse_up(on_link, MouseButton::Left, none);
        vcx.run_until_parked();
        assert!(
            opened(&selection, vcx).is_empty(),
            "the press was not on the link"
        );
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

    /// A chip inside a code span: the pill is its only background — the code
    /// box stays on the text around it, not behind the name.
    #[test]
    fn chip_runs_drop_the_run_background_inside_the_chip() {
        let mut code = run(20);
        code.background_color = Some(gpui::red());
        let runs = chip_runs(vec![code], 4..10, 1, gpui::blue());
        let backgrounds: Vec<bool> = runs.iter().map(|r| r.background_color.is_some()).collect();
        assert_eq!(backgrounds, vec![true, false, false, true]);
    }

    /// A bubble line at the chat's type size and line height, chips and all.
    struct ChipLine {
        selection: Entity<TextSelection>,
        mentions: Rc<Mentions>,
        text: SharedString,
    }

    impl gpui::Render for ChipLine {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use crate::ui::theme::{CHAT_BODY, CHAT_LINE_HEIGHT};
            use gpui::{div, ParentElement as _, Styled as _};
            let text = self.text.clone();
            let selectable = Selectable::new(&self.selection, Hsla::default(), cx)
                .with_mentions(self.mentions.clone(), gpui::blue());
            let bubble = selectable.bubble("m1", text.clone());
            div()
                .w(px(480.0))
                .p(px(20.0))
                .text_size(px(CHAT_BODY))
                .line_height(px(CHAT_LINE_HEIGHT))
                .child(bubble.text(0, text.clone(), vec![run(text.len())]))
        }
    }

    /// Geometry snapshot of what a bubble paints for its chips
    /// ([feedback mention-chip-highlight-2026-10-28], Jacob: "odd pfp
    /// highlights … clearly not centered"): one pill per chip, as wide as the
    /// disc and the name plus the same inset at both ends, the disc and the
    /// pill on the line's centre, #1533's gap between disc and name kept.
    #[gpui::test]
    fn each_chip_paints_one_pill_centred_on_its_picture_and_name(cx: &mut gpui::TestAppContext) {
        use std::cell::RefCell;
        let opened = Rc::new(RefCell::new(Vec::new()));
        let mentions = chip_table(&opened);
        let text: SharedString = mentions.display("@comms says hi to @here").into();
        PAINTED_CHIPS.with(|p| p.borrow_mut().clear());
        let (_view, vcx) = cx.add_window_view(|_, cx| ChipLine {
            selection: cx.new(TextSelection::new),
            mentions,
            text,
        });
        vcx.run_until_parked();
        // Every frame paints again; one copy of each is the snapshot.
        let mut painted: Vec<PaintedChip> = Vec::new();
        for p in PAINTED_CHIPS.with(|p| p.borrow().clone()) {
            if !painted.contains(&p) {
                painted.push(p);
            }
        }
        let pills: Vec<_> = painted
            .iter()
            .filter_map(|p| match p {
                PaintedChip::Pill { bounds, text, line } => Some((*bounds, *text, *line)),
                PaintedChip::Disc { .. } => None,
            })
            .collect();
        let discs: Vec<_> = painted
            .iter()
            .filter_map(|p| match p {
                PaintedChip::Disc { bounds, name_x } => Some((*bounds, *name_x)),
                PaintedChip::Pill { .. } => None,
            })
            .collect();
        assert_eq!(pills.len(), 2, "one pill per chip: {painted:#?}");
        assert_eq!(discs.len(), 2, "one disc per chip: {painted:#?}");
        let close = |a: Pixels, b: Pixels, what: &str| {
            assert!(
                (f32::from(a) - f32::from(b)).abs() < 0.01,
                "{what}: {a:?} vs {b:?}"
            );
        };
        let mid_y = |b: &Bounds<Pixels>| b.origin.y + b.size.height / 2.0;
        for ((pill, (start, end), (top, line_h)), (disc, name_x)) in pills.iter().zip(&discs) {
            // Scout, 2026-10-30: the pill used to be painted `PILL_PAD_X` past
            // the run on each side, over the word beside it. It now stays
            // inside the run, whose pad cells are its inset.
            close(pill.left(), *start, "the pill starts at the chip's run");
            close(pill.right(), *end, "the pill ends at the chip's run");
            assert!(
                disc.left() > pill.left(),
                "the pad cell holds the picture clear of the pill's edge"
            );
            assert!(
                f32::from(*name_x - disc.right()) >= mention::PICTURE_GAP - 0.01,
                "the disc keeps its gap to the name"
            );
            close(
                pill.size.height,
                *line_h + px(mention::PILL_PAD_Y * 2.0),
                "pill height",
            );
            close(
                mid_y(pill),
                *top + *line_h / 2.0,
                "the pill on the line's centre",
            );
            close(mid_y(disc), mid_y(pill), "the disc on the pill's centre");
            assert!(
                disc.size.height <= px(mention::PICTURE),
                "the disc is at most the picture size"
            );
        }
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
