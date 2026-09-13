//! [`Doc`] → gpui elements.
//!
//! Numbers drive layout (sizes, line heights, paddings — the constants here);
//! colors are paint, read from [`Theme`]. Blocks are a flat list, so nesting is
//! left padding rather than nested containers, and the gap between two blocks
//! is decided by the pair: list items sit tight, everything else breathes.
//!
//! Ported from zeronsh/comet (MIT) and rebuilt against the flat block model.

use std::{cell::RefCell, ops::Range, rc::Rc};

use gpui::{
    AnyElement, App, BorderStyle, Bounds, CursorStyle, ElementId, FontStyle, FontWeight, HighlightStyle,
    Hsla, ObjectFit, Pixels, Point, SharedString, StrikethroughStyle, StyledImage as _,
    StyledText, TextLayout, TextRun, UnderlineStyle, Window, canvas, div, font, img, point,
    prelude::*, px, quad, size,
};
use theme::{TextStyle, Theme, Typeset};

use crate::{
    bionic, block,
    doc::{Align, Block, BlockKind, Doc, Form, Mark, Part, Text},
    math, preview,
    reveal::{LineReveal, Reveal, RevealClock, Rise},
    select::{Cursor, Selection},
    typography::Typography,
};

/// Space between two ordinary blocks, and the tighter space inside a list.
const BLOCK_GAP: f32 = 12.0;
const LIST_GAP: f32 = 8.0;
/// One indent level. Wide enough to clear a marker and read as a level.
const INDENT_WIDTH: f32 = 22.0;
/// The marker column of a list row. Cursor: the marker sits a little in
/// from the paragraph edge and the words begin 20px in.
const MARKER_WIDTH: f32 = 14.0;
const MARKER_GAP: f32 = 2.0;
const LIST_INSET: f32 = 6.0;
/// A fence's height is `lines × the code leading` plus this padding.
const CODE_PADDING_X: f32 = 12.0;
const CODE_PADDING_Y: f32 = 10.0;
/// What a fence with no info string calls itself, in its header and in a
/// picker — one spelling, so the label and the menu row cannot disagree.
pub const PLAIN_LANGUAGE: &str = "Plain";
/// Width of the caret. Wider than a hairline, because it has to read at a
/// glance against the text it sits in.
const CARET_WIDTH: f32 = 1.5;
/// Inline code's wash is a rounded quad painted under the glyphs: a run's
/// `background_color` can only ever be a square box.
const INLINE_CODE_RADIUS: f32 = 4.5;
const INLINE_CODE_PAD_X: f32 = 2.0;
const INLINE_CODE_INSET_Y: f32 = 2.0;
/// A mention's chip — the same quad-under-glyphs trick as inline code, with
/// more room and an outline so the two do not read as the same thing.
const CHIP_PAD_X: f32 = 4.0;
const CHIP_INSET_Y: f32 = 1.0;
/// A chip with a block to itself is a real element rather than a wash, so it
/// has room for the favicon the inline one cannot hold.
const CHIP_BLOCK_PAD_X: f32 = 8.0;
const CHIP_BLOCK_PAD_Y: f32 = 3.0;
const CHIP_ICON: f32 = 15.0;
/// Bookmark metrics. Notion's card: 180px of image beside the text, and a
/// height that fits a title, two lines of blurb and a footer. A cover moves
/// that image above the text and gives it the card's full width.
const CARD_HEIGHT: f32 = 116.0;
const CARD_IMAGE_WIDTH: f32 = 180.0;
const CARD_COVER_HEIGHT: f32 = 200.0;
const CARD_PADDING: f32 = 14.0;
const CARD_BORDER: f32 = 1.0;
const CARD_ICON: f32 = 16.0;
const CARD_COVER: f32 = 44.0;
/// Image metrics.
const IMAGE_EMPTY_HEIGHT: f32 = 52.0;
const CAPTION_GAP: f32 = 4.0;
/// What an image with no URL yet says, and what its caption says while empty.
const IMAGE_EMPTY: &str = "Add an image";
const CAPTION_HINT: &str = "Write a caption";
/// A display formula is set larger than the prose around it, as TeX sets
/// display style, with room above and below.
const MATH_SCALE: f32 = 1.15;
const MATH_PADDING_Y: f32 = 6.0;
/// Table metrics. The design is frameless: hairlines between rows are the only
/// chrome — no outer box, no header fill, no rounding.
const TABLE_CELL_PADDING: f32 = 8.0;
const TABLE_DIVIDER: f32 = 1.0;
/// Floor for a column's max-content share, so a short column ("1k") beside a
/// prose column keeps a readable width.
const TABLE_MIN_COLUMN_CONTENT: f32 = 48.0;
/// Narrowest a column wraps down to before the table scrolls instead.
const TABLE_MIN_COLUMN_WIDTH: f32 = 96.0;

/// What an image's one authored string is doing on the page.
///
/// SwiftUI keeps three things apart — `accessibilityLabel` for a reader that
/// cannot see, `.help` for the pointer, and a caption you compose out of a
/// `Text` under the picture. Markdown has one slot for all three, so this says
/// which of them it is playing here rather than in the document, where it is
/// the same string either way.
///
/// A named choice rather than a `bool`, so a surface that wants a third answer
/// gets a variant instead of a second flag.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Caption {
    /// Under the picture, where a caret can sit in it. The editor's shape.
    #[default]
    Shown,
    /// Kept by the document and painted nowhere — a picture on its own.
    Hidden,
}

/// A range the caller wants washed, and which of the three washes it gets.
///
/// A comment thread is what asks for this, and none of what it *says* is here:
/// the caller keeps the thread and hands over the range, the way it hands over
/// a [`crate::Preview`]. A closed set rather than a color, so the environment
/// keeps deciding the paint.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Annotation {
    /// A thread still waiting on someone.
    #[default]
    Open,
    /// Answered, and kept for the record.
    Resolved,
    /// The one whose thread the reader has in front of them.
    Active,
}

impl Annotation {
    fn wash(self, theme: &Theme) -> Hsla {
        match self {
            Self::Open => theme.warning.opacity(0.20),
            Self::Resolved => theme.warning.opacity(0.08),
            Self::Active => theme.warning.opacity(0.38),
        }
    }
}

/// What an editor paints over a document.
///
/// One value rather than six parameters, and the reason it is public: the
/// caret, the selection, the comment washes and the layout sink all arrive
/// together or not at all, and a read-only [`render`] sets none of them.
#[derive(Clone)]
pub struct Editing<'a> {
    /// The caret and what it has selected. `None` paints neither — a document
    /// nobody is editing.
    pub selection: Option<Selection>,
    /// The blink's lit half. A caret painted on every frame reads as frozen,
    /// and the phase belongs to whoever owns the focus.
    pub caret_on: bool,
    /// Filled as the document paints, for a caller resolving clicks against it.
    pub layouts: Option<&'a BlockLayouts>,
    /// Ranges washed under the text, in the order given.
    pub annotations: &'a [(Selection, Annotation)],
    /// Shown on the caret's block while it holds nothing.
    pub placeholder: Option<SharedString>,
    pub caption: Caption,
    /// What to set the document in. `None` takes the installed
    /// [`Typography`] — a caller sizing one document apart from the rest
    /// passes [`Typography::scaled`], and one setting prose for [`bionic`]
    /// reading passes [`Typography::bionic`].
    pub typography: Option<Typography>,
}

impl Default for Editing<'_> {
    fn default() -> Self {
        Self {
            selection: None,
            // Lit, so that a caller setting a selection and nothing else gets a
            // caret rather than a mystery.
            caret_on: true,
            layouts: None,
            annotations: &[],
            placeholder: None,
            caption: Caption::default(),
            typography: None,
        }
    }
}

/// Where each block's text landed, recorded as it painted.
///
/// A caret has to be placeable by pointer, and only paint knows where a glyph
/// ended up. An editor hands one of these in, the renderer fills it, and the
/// next click resolves against it. Read-only callers pass nothing and pay
/// nothing.
#[derive(Clone, Default)]
pub struct BlockLayouts(Rc<RefCell<Frames>>);

#[derive(Default)]
struct Frames {
    texts: Vec<Painted>,
    /// Each block's whole box, which a text layout does not give: a rule and
    /// an image hold no text at all, and a gutter handle still has to find them.
    blocks: Vec<(usize, Bounds<Pixels>)>,
    /// A fenced block's language label, which a host may want to hang a
    /// picker on.
    languages: Vec<(usize, Bounds<Pixels>)>,
    /// An image block's picture, which is not its block: the block runs the
    /// full column and carries the caption, and a resize handle belongs on the
    /// edge of the picture itself.
    pictures: Vec<(usize, Bounds<Pixels>)>,
}

/// One shaped run and the slice of its part it covers.
///
/// A paragraph is one entry over all of its text; a code block is one entry per
/// line. The range is what lets both resolve a click the same way — the layout
/// answers in its own coordinates and the base puts the answer back into the
/// part's.
struct Painted {
    block: usize,
    part: Part,
    range: Range<usize>,
    layout: TextLayout,
}

impl BlockLayouts {
    /// The position under `point`.
    ///
    /// Falls back to the nearest text vertically, so clicking the margin
    /// beside a line — or below the last one — still lands somewhere useful
    /// rather than doing nothing.
    pub fn hit(&self, point: Point<Pixels>) -> Option<Cursor> {
        let entries = &self.0.borrow().texts;
        let cursor = |painted: &Painted| {
            let (Ok(offset) | Err(offset)) = painted.layout.index_for_position(point);
            Cursor::new(
                painted.block,
                painted.part,
                painted.range.start + offset.min(painted.range.len()),
            )
        };
        if let Some(painted) = entries
            .iter()
            .find(|painted| painted.layout.bounds().contains(&point))
        {
            return Some(cursor(painted));
        }
        entries
            .iter()
            .min_by_key(|painted| {
                let bounds = painted.layout.bounds();
                let above = (bounds.origin.y - point.y).abs();
                let below = (bounds.origin.y + bounds.size.height - point.y).abs();
                f32::from(above.min(below)) as i64
            })
            .map(cursor)
    }

    /// Where a position painted last frame, and how tall its line is.
    ///
    /// Vertical motion is geometry rather than arithmetic on line numbers, so
    /// a wrapped row and a hard newline are the same case and neither needs
    /// counting — the rule `ui::TextField` arrived at.
    pub fn position(&self, at: Cursor) -> Option<(Point<Pixels>, Pixels)> {
        let entries = &self.0.borrow().texts;
        let painted = entries.iter().find(|painted| {
            painted.block == at.block
                && painted.part == at.part
                && painted.range.start <= at.offset
                && at.offset <= painted.range.end
        })?;
        let point = painted
            .layout
            .position_for_index(at.offset - painted.range.start)?;
        Some((point, painted.layout.line_height()))
    }

    /// The position one painted row above or below `at`, and the row it landed
    /// on. Walks the recorded runs in paint order — which is document order.
    ///
    /// Two things make this refuse to be a hit test. The gap between blocks
    /// belongs to no run, so a probe there answers with whichever run is
    /// nearest — and at a boundary that is the block being *left*, whose bottom
    /// edge is zero pixels away while the next block's top is a whole gap. And
    /// `from` is passed in rather than derived from `at`, because an offset at
    /// a soft wrap belongs to two rows and `position_for_index` always answers
    /// with the first: derive it and every step down recomputes the same row.
    pub fn step_row(
        &self,
        at: Cursor,
        from: Point<Pixels>,
        down: bool,
    ) -> Option<(Cursor, Pixels)> {
        let entries = &self.0.borrow().texts;
        let ix = entries.iter().position(|painted| {
            painted.block == at.block
                && painted.part == at.part
                && painted.range.start <= at.offset
                && at.offset <= painted.range.end
        })?;
        let here = &entries[ix];
        let line = here.layout.line_height();
        let index_at = |painted: &Painted, y: Pixels| {
            let (Ok(offset) | Err(offset)) = painted.layout.index_for_position(point(from.x, y));
            (
                Cursor::new(
                    painted.block,
                    painted.part,
                    painted.range.start + offset.min(painted.range.len()),
                ),
                y,
            )
        };

        // A wrapped paragraph is one run holding several rows, so try to stay
        // inside it before looking for a neighbour.
        let bounds = here.layout.bounds();
        let target = if down { from.y + line } else { from.y - line };
        if target >= bounds.origin.y && target < bounds.origin.y + bounds.size.height {
            return Some(index_at(here, target));
        }

        let next = match down {
            true => entries.get(ix + 1)?,
            false => entries.get(ix.checked_sub(1)?)?,
        };
        // Enter the neighbour on the row facing the one just left.
        let bounds = next.layout.bounds();
        let row = match down {
            true => bounds.origin.y,
            false => bounds.origin.y + bounds.size.height - next.layout.line_height(),
        };
        Some(index_at(next, row))
    }

    /// Whether `point` is inside painted text.
    ///
    /// [`Self::hit`] answers with the nearest run wherever it is asked, which
    /// is what a click wants and what a *pointer* must not have: an I-beam
    /// belongs where a caret would land, not everywhere an editor's box
    /// reaches.
    pub fn over_text(&self, point: Point<Pixels>) -> bool {
        self.0
            .borrow()
            .texts
            .iter()
            .any(|painted| painted.layout.bounds().contains(&point))
    }

    /// The block under `point`, for a gutter handle and a drop target.
    pub fn block_at(&self, point: Point<Pixels>) -> Option<usize> {
        let blocks = &self.0.borrow().blocks;
        blocks
            .iter()
            .find(|(_, bounds)| bounds.contains(&point))
            .or_else(|| {
                blocks.iter().min_by_key(|(_, bounds)| {
                    let above = (bounds.origin.y - point.y).abs();
                    let below = (bounds.origin.y + bounds.size.height - point.y).abs();
                    f32::from(above.min(below)) as i64
                })
            })
            .map(|(ix, _)| *ix)
    }

    /// Where a block's first painted row sits, and how tall that row is — what
    /// a mark in the gutter has to line up with.
    ///
    /// [`Self::block_bounds`] is not that: it spans every row the block holds,
    /// and a heading's single row is taller than a paragraph's, so anything
    /// placed from the top of the box rides above the text it points at.
    /// `None` for a block that paints no text at all, a rule being the one
    /// that does.
    pub fn first_row(&self, ix: usize) -> Option<(Pixels, Pixels)> {
        let texts = &self.0.borrow().texts;
        let painted = texts.iter().find(|painted| painted.block == ix)?;
        Some((
            painted.layout.bounds().origin.y,
            painted.layout.line_height(),
        ))
    }

    /// Where a block painted last frame, in window coordinates.
    pub fn block_bounds(&self, ix: usize) -> Option<Bounds<Pixels>> {
        self.0
            .borrow()
            .blocks
            .iter()
            .find(|(block, _)| *block == ix)
            .map(|(_, bounds)| *bounds)
    }

    /// Where a fenced block's language label painted — the box a host hangs its
    /// picker on. Recorded rather than derived: the word's width is the text
    /// system's answer, and padding arithmetic would be wrong the first time
    /// any of it changed.
    pub fn language_bounds(&self, ix: usize) -> Option<Bounds<Pixels>> {
        self.0
            .borrow()
            .languages
            .iter()
            .find(|(block, _)| *block == ix)
            .map(|(_, bounds)| *bounds)
    }

    /// Where an image block's picture painted, which
    /// [`BlockLayouts::block_bounds`] does not give: that box spans the column
    /// and takes in the caption, so a handle placed from it sits off the edge
    /// of any picture narrower than the page.
    pub fn picture_bounds(&self, ix: usize) -> Option<Bounds<Pixels>> {
        self.0
            .borrow()
            .pictures
            .iter()
            .find(|(block, _)| *block == ix)
            .map(|(_, bounds)| *bounds)
    }

    fn record(&self, block: usize, part: Part, range: Range<usize>, layout: TextLayout) {
        self.0.borrow_mut().texts.push(Painted {
            block,
            part,
            range,
            layout,
        });
    }

    fn record_block(&self, ix: usize, bounds: Bounds<Pixels>) {
        self.0.borrow_mut().blocks.push((ix, bounds));
    }

    fn record_language(&self, ix: usize, bounds: Bounds<Pixels>) {
        self.0.borrow_mut().languages.push((ix, bounds));
    }

    fn record_picture(&self, ix: usize, bounds: Bounds<Pixels>) {
        self.0.borrow_mut().pictures.push((ix, bounds));
    }

    fn clear(&self) {
        let mut frames = self.0.borrow_mut();
        frames.texts.clear();
        frames.blocks.clear();
        frames.languages.clear();
        frames.pictures.clear();
    }
}

/// What the editor needs painted into one text: which text it is, where the
/// caret sits, and where to record the layout a click resolves against.
///
/// One bundle rather than four parameters threaded through every block arm —
/// a read-only render builds it with no caret and no sink, and pays nothing.
#[derive(Clone, Copy)]
struct Overlay<'a> {
    block: usize,
    part: Part,
    selection: Option<Selection>,
    caret_on: bool,
    layouts: Option<&'a BlockLayouts>,
    /// Ranges washed under the text, in the order the caller gave them.
    annotations: &'a [(Selection, Annotation)],
    /// Shown on the caret's block while it holds nothing. The renderer is the
    /// only thing that knows where that text sits, so the string comes to it.
    placeholder: Option<&'a SharedString>,
    caption: Caption,
    reveal: Option<&'a Reveal>,
    /// Whether prose is weighted for [`bionic`] reading.
    bionic: bool,
}

impl<'a> Overlay<'a> {
    fn at(self, part: Part) -> Self {
        Self { part, ..self }
    }

    fn here(&self) -> Cursor {
        Cursor::new(self.block, self.part, 0)
    }

    /// The caret to paint: where it is, and only on the blink's lit half.
    ///
    /// Separate from [`Self::caret`] because the blink must not reach anything
    /// but the quad — a block whose paint depends on holding the caret would
    /// otherwise swap itself out twice a second.
    fn caret_painted(&self) -> Option<usize> {
        self.caret_on.then(|| self.caret()).flatten()
    }

    /// The caret's byte offset, if the head is in *this* text.
    fn caret(&self) -> Option<usize> {
        self.selection
            .map(|selection| selection.head)
            .filter(|head| head.block == self.block && head.part == self.part)
            .map(|head| head.offset)
    }

    /// The selected slice of this text, clipped to it.
    fn selected(&self, len: usize) -> Option<Range<usize>> {
        self.clip(self.selection?, len)
    }

    /// The annotated slices of this text, already resolved to their paint —
    /// the wash goes into a `move` closure that the theme does not travel into.
    fn annotated(&self, len: usize, theme: &Theme) -> Vec<(Range<usize>, Hsla)> {
        self.annotations
            .iter()
            .filter_map(|(range, kind)| Some((self.clip(*range, len)?, kind.wash(theme))))
            .collect()
    }

    /// A range clipped to this text, and `None` when it does not reach it.
    ///
    /// The comparison is on `(block, part)` alone: a range covers this text
    /// entirely when it starts before and ends after, and the offsets only
    /// matter at the two ends.
    fn clip(&self, selection: Selection, len: usize) -> Option<Range<usize>> {
        if selection.is_collapsed() {
            return None;
        }
        let (start, end) = selection.ordered();
        let here = self.here();
        let (first, last) = (
            Cursor::new(start.block, start.part, 0),
            Cursor::new(end.block, end.part, 0),
        );
        if here < first || here > last {
            return None;
        }
        let from = if here == first { start.offset } else { 0 };
        let to = if here == last { end.offset } else { len };
        (from < to).then_some(from..to.min(len))
    }

    /// Whether a block painting something a caret cannot enter — a rule, a
    /// picture — falls inside the selection, and so should show that it is
    /// going to be taken.
    fn covers_block(&self) -> bool {
        let Some(selection) = self.selection.filter(|s| !s.is_collapsed()) else {
            return false;
        };
        let (start, end) = selection.ordered();
        start.block < self.block && self.block < end.block
    }
}

/// Parse and render in one step — the common case for read-only content.
pub fn markdown(source: &str, window: &mut Window, cx: &mut App) -> AnyElement {
    render(&crate::parse(source), Caption::default(), window, cx)
}

/// Render a document.
pub fn render(doc: &Doc, caption: Caption, window: &mut Window, cx: &mut App) -> AnyElement {
    render_with(
        doc,
        Editing {
            caption,
            ..Editing::default()
        },
        window,
        cx,
    )
}

/// Render a document with a caret and a selection in it.
///
/// Both are paint-time concerns and nothing else: they read their positions off
/// the shaped text's own layout handle, the same way the inline-code wash does,
/// so nothing about layout depends on where the caret sits. An editor supplies
/// the selection and owns the focus and the keys; painting a caret and a few
/// quads is not worth a second renderer.
pub fn render_with(doc: &Doc, editing: Editing, window: &mut Window, cx: &mut App) -> AnyElement {
    render_inner(doc, editing, None, window, cx)
}

/// [`render_with`], with the prose rising into place one painted line at a
/// time. `reveal` is the document's clocks; the caller keeps it across frames.
pub fn render_revealed(
    doc: &Doc,
    editing: Editing,
    reveal: &Reveal,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    render_inner(doc, editing, Some(reveal), window, cx)
}

fn render_inner(
    doc: &Doc,
    editing: Editing,
    reveal: Option<&Reveal>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Editing {
        selection,
        caret_on,
        layouts,
        annotations,
        placeholder,
        caption,
        typography,
    } = editing;
    // Refilled every frame, in paint order — and emptied in *prepaint*, not
    // here. An editor reads last frame's positions while building this frame's
    // tree (a menu anchored at the caret, a handle beside a block), and
    // clearing at build time takes them away before it can. Placed first in the
    // column so it runs ahead of every recorder below it.
    let reset = layouts.map(|layouts| {
        let layouts = layouts.clone();
        canvas(move |_, _, _| layouts.clear(), |_, _, _, _| ())
            .absolute()
            .size(px(0.0))
    });
    // Cloned once so the theme is readable while `cx` stays free for the
    // element state the copy button needs.
    let theme = Theme::of(cx).clone();
    let typography = typography.unwrap_or_else(|| Typography::of(cx));
    let mut column = div()
        .flex()
        .flex_col()
        .font_family(theme.font_sans.clone())
        .children(reset);

    for (ix, block) in doc.blocks.iter().enumerate() {
        let gap = match doc.blocks.get(ix.wrapping_sub(1)) {
            None => 0.0,
            Some(previous) if tight(previous, block) => LIST_GAP,
            Some(_) => BLOCK_GAP,
        };
        let overlay = Overlay {
            block: ix,
            part: Part::Body,
            selection,
            caret_on,
            layouts,
            annotations,
            placeholder: placeholder.as_ref(),
            caption,
            reveal,
            bionic: typography.bionic,
        };
        // The block's own box, recorded for a gutter handle and a drop target.
        // A rule and an image hold no text, so a layout would not find them.
        let frame = layouts.map(|layouts| {
            let layouts = layouts.clone();
            canvas(
                move |bounds, _, _| layouts.record_block(ix, bounds),
                |_, _, _, _| (),
            )
            .absolute()
            .size_full()
        });
        column = column.child(
            // The indent sits on the outside and the recorder on the inside,
            // so what is recorded is the box the block's text actually
            // occupies. Recorded outside the padding, every level answered
            // with the same left edge, and a gutter handle placed from it
            // stayed at the margin while the block it belongs to moved right.
            div()
                .mt(px(gap))
                .pl(px(block.indent as f32 * INDENT_WIDTH))
                .child(
                    div()
                        .w_full()
                        .relative()
                        .children(frame)
                        // What a caret cannot enter still has to show it is
                        // inside the selection, or a rule between two
                        // paragraphs looks untouched right up until it
                        // disappears.
                        .when(overlay.covers_block() && block.opaque(), |el| {
                            el.rounded(px(4.0)).bg(theme.selection)
                        })
                        .child(block_element(
                            block,
                            overlay,
                            &typography,
                            &theme,
                            window,
                            cx,
                        )),
                ),
        );
    }

    column.into_any_element()
}

/// Whether two adjacent blocks belong to the same list and should sit close.
fn tight(previous: &Block, next: &Block) -> bool {
    let marker = |block: &Block| {
        matches!(
            block.kind,
            BlockKind::Bullet(_) | BlockKind::Ordered { .. } | BlockKind::Task { .. }
        )
    };
    marker(previous) && (marker(next) || next.indent > previous.indent)
}

fn block_element(
    block: &Block,
    overlay: Overlay,
    typography: &Typography,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let body = overlay.at(Part::Body);
    match &block.kind {
        BlockKind::Paragraph(text) => text_element(
            text,
            typography.body.size(),
            typography.body.line_height(),
            FontWeight::NORMAL,
            body,
            theme,
        ),
        BlockKind::Heading { level, text } => {
            let heading = typography.heading(*level);
            text_element(
                text,
                heading.size(),
                heading.line_height(),
                heading.weight,
                body,
                theme,
            )
        }
        BlockKind::Bullet(text) => {
            marker_row(disc(typography, theme), text, body, typography, theme)
        }
        BlockKind::Ordered { number, text } => marker_row(
            div()
                .flex_none()
                .w(px(MARKER_WIDTH))
                .text_size(px(typography.body.size()))
                .line_height(px(typography.body.line_height()))
                .text_color(theme.text)
                .child(SharedString::from(format!("{number}.")))
                .into_any_element(),
            text,
            body,
            typography,
            theme,
        ),
        BlockKind::Task { checked, text } => marker_row(
            checkbox(*checked, typography, theme),
            text,
            body,
            typography,
            theme,
        ),
        BlockKind::Quote(text) => div()
            .border_l_2()
            .border_color(theme.border_strong)
            .pl(px(12.0))
            .pr(px(10.0))
            .py(px(2.0))
            .text_color(theme.text_muted)
            .child(text_element(
                text,
                typography.body.size(),
                typography.body.line_height(),
                FontWeight::NORMAL,
                body,
                theme,
            ))
            .into_any_element(),
        BlockKind::Code { language, code } => {
            let overlay = overlay.at(Part::Code);
            // The caret in the fence gives the source back. A painted block is
            // still an editable one, and typing into it otherwise edits what
            // the reader cannot see.
            let painted = overlay
                .caret()
                .is_none()
                .then(|| block::render(language.as_deref(), &code.text, window, cx))
                .flatten();
            match painted {
                // Painted, there is no text under the selection to carry it —
                // the wash an opaque block gets at the container comes here.
                Some(element) => div()
                    .when(overlay.covers_block(), |el| {
                        el.rounded(px(4.0)).bg(theme.selection)
                    })
                    .child(element)
                    .into_any_element(),
                None => code_block(
                    language.as_deref(),
                    &code.text,
                    overlay,
                    typography,
                    theme,
                    window,
                    cx,
                ),
            }
        }
        BlockKind::Image { url, alt, width } => image(url, alt, *width, overlay, typography, theme),
        BlockKind::Bookmark { url, form } => {
            bookmark(overlay.block, url, *form, typography, theme, cx)
        }
        BlockKind::Table {
            align,
            header,
            rows,
        } => table(align, header, rows, overlay, typography, theme, window),
        // Centred in the column like TeX's display style. Atomic: the frame
        // recorded around every block is what a click finds, and there is no
        // text layout to record because there is no caret to place.
        BlockKind::Math { tex } => div()
            .w_full()
            .flex()
            .justify_center()
            .py(px(MATH_PADDING_Y))
            .child(math::typeset(
                &tex.text,
                typography.body.size() * MATH_SCALE,
                theme.text,
            ))
            .into_any_element(),
        BlockKind::Rule => div()
            .h(px(1.0))
            .w_full()
            .bg(theme.border)
            .into_any_element(),
    }
}

/// A real 5px disc rather than the "•" glyph, which reads too small at body size.
fn disc(typography: &Typography, theme: &Theme) -> AnyElement {
    div()
        .flex_none()
        .w(px(MARKER_WIDTH))
        .h(px(typography.body.line_height()))
        .flex()
        .items_center()
        .child(
            div()
                .ml(px(1.0))
                .w(px(5.0))
                .h(px(5.0))
                .rounded_full()
                .bg(theme.text_faint),
        )
        .into_any_element()
}

fn checkbox(checked: bool, typography: &Typography, theme: &Theme) -> AnyElement {
    let mut box_ = div()
        .w(px(13.0))
        .h(px(13.0))
        .rounded(px(3.5))
        .border_1()
        .flex()
        .items_center()
        .justify_center();
    box_ = if checked {
        box_.bg(theme.solid)
            .border_color(theme.solid)
            .text_style(TextStyle::Caption)
            .text_color(theme.on_solid)
            .child("✓")
    } else {
        box_.border_color(theme.border_strong)
    };

    div()
        .flex_none()
        .w(px(MARKER_WIDTH))
        .h(px(typography.body.line_height()))
        .flex()
        .items_center()
        .child(box_)
        .into_any_element()
}

fn marker_row(
    marker: AnyElement,
    text: &Text,
    overlay: Overlay,
    typography: &Typography,
    theme: &Theme,
) -> AnyElement {
    // The marker rises with the row's first line, or it sits there alone
    // before the words arrive.
    let marker = match overlay.reveal {
        Some(reveal) => Rise::new(
            marker,
            reveal.clock(overlay.block, overlay.part),
            reveal.veil(),
        )
        .into_any_element(),
        None => marker,
    };
    div()
        .flex()
        .flex_row()
        .pl(px(LIST_INSET))
        .gap(px(MARKER_GAP))
        .child(marker)
        .child(div().flex_1().min_w_0().child(text_element(
            text,
            typography.body.size(),
            typography.body.line_height(),
            FontWeight::NORMAL,
            overlay,
            theme,
        )))
        .into_any_element()
}

/// Inline content flattened for shaping: one string, its runs, and the ranges
/// that need painting underneath (link clicks, inline-code washes, chips).
pub struct Flat {
    pub text: SharedString,
    pub runs: Vec<TextRun>,
    /// Bold / italic / strike / link / code colour — applied on the inherited
    /// face.
    pub highlights: Vec<(Range<usize>, HighlightStyle)>,
    /// Monospace family on inline code, the math face on a formula. Same
    /// inherited face otherwise.
    pub fonts: Vec<(Range<usize>, SharedString)>,
    pub links: Vec<(Range<usize>, String)>,
    pub code: Vec<Range<usize>>,
    pub chips: Vec<Range<usize>>,
    /// Inline formulas — set in [`math::FONT`], and left alone by [`bionic`].
    pub math: Vec<Range<usize>>,
}

/// Marks are ranges, gpui wants consecutive runs — so cut the text at every
/// mark boundary and ask which marks cover each piece.
pub fn flatten(text: &Text, base_weight: FontWeight, theme: &Theme) -> Flat {
    let mut cuts: Vec<usize> = text
        .marks
        .iter()
        .flat_map(|span| [span.range.start, span.range.end])
        .chain([0, text.text.len()])
        .filter(|cut| *cut <= text.text.len())
        .collect();
    cuts.sort_unstable();
    cuts.dedup();

    let mut runs = Vec::new();
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut fonts: Vec<(Range<usize>, SharedString)> = Vec::new();
    let mut links: Vec<(Range<usize>, String)> = Vec::new();
    let mut code: Vec<Range<usize>> = Vec::new();
    let mut chips: Vec<Range<usize>> = Vec::new();
    let mut maths: Vec<Range<usize>> = Vec::new();

    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let covering = text
            .marks
            .iter()
            .filter(|span| span.range.start <= start && span.range.end >= end);

        let (mut bold, mut italic, mut mono, mut strike) = (false, false, false, false);
        let (mut chip, mut formula) = (false, false);
        let mut link = None;
        for span in covering {
            match &span.mark {
                Mark::Bold => bold = true,
                Mark::Italic => italic = true,
                Mark::Strike => strike = true,
                Mark::Code => mono = true,
                Mark::Math { .. } => formula = true,
                Mark::Mention { url, .. } => {
                    chip = true;
                    link = Some(url.clone());
                }
                Mark::Link(url) | Mark::Image(url) => link = Some(url.clone()),
            }
        }

        if mono {
            match code.last_mut() {
                Some(range) if range.end == start => range.end = end,
                _ => code.push(start..end),
            }
        }
        if formula {
            match maths.last_mut() {
                Some(range) if range.end == start => range.end = end,
                _ => maths.push(start..end),
            }
        }
        if chip {
            match chips.last_mut() {
                Some(range) if range.end == start => range.end = end,
                _ => chips.push(start..end),
            }
        }
        if let Some(url) = &link {
            match links.last_mut() {
                Some((range, last)) if range.end == start && last == url => range.end = end,
                _ => links.push((start..end, url.clone())),
            }
        }

        // A formula is already italic where it should be — its letters are the
        // mathematical italic code points — so the face stays upright, and at
        // the prose weight: the math face has one.
        let family: SharedString = if mono {
            theme.font_mono.clone()
        } else if formula {
            SharedString::new_static(math::FONT)
        } else {
            theme.font_sans.clone()
        };
        let mut face = font(family.clone());
        face.weight = if bold && base_weight.0 < FontWeight::SEMIBOLD.0 && !formula {
            FontWeight::SEMIBOLD
        } else {
            base_weight
        };
        face.style = if italic && !formula {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };

        runs.push(TextRun {
            len: end - start,
            font: face,
            // Links take the accent and no underline, as Cursor's do; a chip
            // carries its own wash and keeps the body colour.
            color: if link.is_some() && !chip {
                theme.accent
            } else if mono {
                theme.code_text
            } else {
                theme.text
            },
            background_color: None,
            underline: None,
            strikethrough: strike.then_some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(theme.text_muted),
            }),
        });

        // Highlights ride on the parent face.
        let mut highlight = HighlightStyle::default();
        let mut marked = false;
        if bold && base_weight.0 < FontWeight::SEMIBOLD.0 {
            highlight.font_weight = Some(FontWeight::SEMIBOLD);
            marked = true;
        }
        if italic {
            highlight.font_style = Some(FontStyle::Italic);
            marked = true;
        }
        if strike {
            highlight.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(theme.text_muted),
            });
            marked = true;
        }
        if link.is_some() && !chip {
            highlight.underline = Some(UnderlineStyle {
                color: Some(theme.text_muted),
                thickness: px(1.0),
                wavy: false,
            });
            marked = true;
        }
        if mono {
            highlight.color = Some(theme.code_text);
            marked = true;
        }
        if mono || formula {
            match fonts.last_mut() {
                Some((range, last)) if range.end == start && last.as_ref() == family.as_ref() => {
                    range.end = end;
                }
                _ => fonts.push((start..end, family.clone())),
            }
        }
        if marked {
            match highlights.last_mut() {
                Some((range, last)) if range.end == start && *last == highlight => {
                    range.end = end;
                }
                _ => highlights.push((start..end, highlight)),
            }
        }
    }

    Flat {
        text: text.text.clone().into(),
        runs,
        highlights,
        fonts,
        links,
        code,
        chips,
        math: maths,
    }
}

/// Paint a highlighted line only when the runs cover it exactly.
/// `StyledText::with_runs` panics in debug on a mismatch.
fn styled_line(line: &str, runs: Vec<TextRun>) -> StyledText {
    let text = SharedString::from(line.to_string());
    match covering(line, runs) {
        Some(runs) => StyledText::new(text).with_runs(runs),
        None => StyledText::new(text),
    }
}

/// The runs, when they cover the line exactly; otherwise nothing, and the
/// line takes the window's text style whole.
fn covering(line: &str, runs: Vec<TextRun>) -> Option<Vec<TextRun>> {
    let covered: usize = runs.iter().map(|run| run.len).sum();
    (covered == line.len() && !runs.is_empty()).then_some(runs)
}

fn text_element(
    text: &Text,
    size: f32,
    line_height: f32,
    weight: FontWeight,
    overlay: Overlay,
    theme: &Theme,
) -> AnyElement {
    let mut flat = flatten(text, weight, theme);
    if overlay.bionic {
        bionic::apply(&mut flat);
    }
    painted_text(flat, text.text.len(), size, line_height, overlay, theme)
}

/// Shaped inline content with the editing overlay under it: the selection, the
/// caret, the inline-code wash, and the layout a click resolves against.
///
/// Takes a [`Flat`] rather than a [`Text`] because a table has to shape every
/// cell to measure the columns before it can paint one.
fn painted_text(
    flat: Flat,
    len: usize,
    size: f32,
    line_height: f32,
    overlay: Overlay,
    theme: &Theme,
) -> AnyElement {
    let (ix, part) = (overlay.block, overlay.part);
    let (caret, selected) = (overlay.caret_painted(), overlay.selected(len));
    let span = 0..len;
    // Only where the caret already is, and only while there is nothing to
    // read: a hint on every empty block would be a page of grey.
    let hint = overlay
        .placeholder
        // The caret's own presence, not the blink's phase — a hint that came
        // and went twice a second would be unreadable.
        .filter(|_| len == 0 && overlay.caret().is_some())
        .map(|hint| {
            div()
                .absolute()
                .text_color(theme.text_faint)
                .child(hint.clone())
        });
    let runs = flat.runs;
    let clock = overlay.reveal.map(|reveal| reveal.clock(ix, part));
    let (styled, layout) = match (overlay.reveal, clock.clone()) {
        (Some(reveal), Some(clock)) => {
            let runs = covering(flat.text.as_ref(), runs);
            let mut rising = LineReveal::new(flat.text.clone(), runs, clock);
            if let Some(veil) = reveal.veil() {
                rising = rising.veil(veil);
            }
            let layout = rising.layout().clone();
            (rising.into_any_element(), layout)
        }
        _ => {
            let styled = styled_line(flat.text.as_ref(), runs);
            let layout = styled.layout().clone();
            (styled.into_any_element(), layout)
        }
    };

    let wash = theme.code_wash;
    let code_ranges = flat.code;
    let chip_wash = theme.element_hover;
    let chip_edge = theme.border;
    let chip_ranges = flat.chips;
    let caret_color = theme.caret;
    let selection_color = theme.selection;
    let annotated = overlay.annotated(len, theme);
    let layouts = overlay.layouts.cloned();
    let underlay = canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            if let Some(layouts) = &layouts {
                layouts.record(ix, part, span.clone(), layout.clone());
            }
            for (range, wash) in &annotated {
                for rect in range_rects(&layout, range, 0.0, 0.0) {
                    window.paint_quad(quad(
                        rect,
                        px(2.0),
                        *wash,
                        px(0.0),
                        gpui::transparent_black(),
                        BorderStyle::default(),
                    ));
                }
            }
            if let Some(range) = &selected {
                for rect in range_rects(&layout, range, 0.0, 0.0) {
                    window.paint_quad(quad(
                        rect,
                        px(2.0),
                        selection_color,
                        px(0.0),
                        gpui::transparent_black(),
                        BorderStyle::default(),
                    ));
                }
            }
            if let Some(offset) = caret
                && let Some(head) = layout.position_for_index(offset)
            {
                window.paint_quad(quad(
                    caret_quad(head, size, layout.line_height()),
                    px(0.0),
                    caret_color,
                    px(0.0),
                    gpui::transparent_black(),
                    BorderStyle::default(),
                ));
            }
            for range in &code_ranges {
                for rect in range_rects(&layout, range, INLINE_CODE_PAD_X, INLINE_CODE_INSET_Y) {
                    with_rise(clock.as_ref(), &layout, rect, window, |rect, window| {
                        window.paint_quad(quad(
                            rect,
                            px(INLINE_CODE_RADIUS),
                            wash,
                            px(0.0),
                            gpui::transparent_black(),
                            BorderStyle::default(),
                        ));
                    });
                }
            }
            for range in &chip_ranges {
                for rect in range_rects(&layout, range, CHIP_PAD_X, CHIP_INSET_Y) {
                    with_rise(clock.as_ref(), &layout, rect, window, |rect, window| {
                        window.paint_quad(quad(
                            rect,
                            px(Theme::control_radius()),
                            chip_wash,
                            px(1.0),
                            chip_edge,
                            BorderStyle::Solid,
                        ));
                    });
                }
            }
        },
    )
    .absolute()
    .size_full();

    div()
        .text_size(px(size))
        .line_height(px(line_height))
        .relative()
        .child(underlay)
        .children(hint)
        .child(styled)
        .into_any_element()
}

/// The caret's quad: the text's own size, centred in the line box.
///
/// The leading is not the caret's to take. A document is set with air around
/// its lines, and a caret filling all of it reads as a second, larger font
/// standing where the text should be.
fn caret_quad(head: Point<Pixels>, size: f32, line_height: Pixels) -> Bounds<Pixels> {
    let inset = (line_height - px(size)) / 2.0;
    Bounds::new(
        head + point(px(0.0), inset),
        gpui::size(px(CARET_WIDTH), px(size)),
    )
}

/// Paint a quad under the text where its line has risen to. A wash that sat
/// still while its words rose through it would show the shape of the code
/// before the code. With no clock, or once the line has landed, `paint` gets
/// the rect as it is.
fn with_rise(
    clock: Option<&RevealClock>,
    layout: &TextLayout,
    rect: Bounds<Pixels>,
    window: &mut Window,
    paint: impl FnOnce(Bounds<Pixels>, &mut Window),
) {
    let line_height = layout.line_height();
    let top = layout.bounds().origin.y;
    let ix = f32::from((rect.center().y - top) / line_height).floor().max(0.0) as usize;
    let progress = clock.and_then(|clock| clock.peek(ix)).unwrap_or(1.0);
    if progress >= 1.0 {
        return paint(rect, window);
    }
    if progress <= 0.0 {
        return;
    }
    let rise = line_height * (1.0 - progress);
    let clip = Bounds::new(
        point(rect.origin.x, top + line_height * ix as f32),
        gpui::size(rect.size.width, line_height),
    );
    let mut risen = rect;
    risen.origin.y += rise;
    window.with_content_mask(Some(gpui::ContentMask { bounds: clip }), |window| {
        paint(risen, window)
    });
}

/// The rectangles a byte range occupies, one per visual row.
fn range_rects(
    layout: &gpui::TextLayout,
    range: &Range<usize>,
    pad_x: f32,
    inset_y: f32,
) -> Vec<Bounds<Pixels>> {
    let mut rects = Vec::new();
    let line_height = layout.line_height();
    let mut cursor = range.start;
    // Walk one visual row at a time. A wrapped range has no direct row query,
    // so the last index still on this row is found by bisection.
    let mut guard = 0;
    while cursor < range.end && guard < 256 {
        guard += 1;
        let Some(head) = layout.position_for_index(cursor) else {
            break;
        };
        let (row_end, next) = match layout.position_for_index(range.end) {
            Some(tail) if tail.y == head.y => (range.end, range.end),
            _ => {
                let (mut low, mut high) = (cursor, range.end);
                while high - low > 1 {
                    let mid = low + (high - low) / 2;
                    match layout.position_for_index(mid) {
                        Some(probe) if probe.y == head.y => low = mid,
                        _ => high = mid,
                    }
                }
                (low, high)
            }
        };
        if let Some(tail) = layout.position_for_index(row_end)
            && tail.x > head.x
        {
            rects.push(Bounds::new(
                point(head.x - px(pad_x), head.y + px(inset_y)),
                size(
                    tail.x - head.x + px(2.0 * pad_x),
                    line_height - px(2.0 * inset_y),
                ),
            ));
        }
        cursor = next.max(cursor + 1);
    }
    rects
}

fn code_block(
    language: Option<&str>,
    code: &str,
    overlay: Overlay,
    typography: &Typography,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let ix = overlay.block;
    // Per line, so the block's height is exactly `lines × line height`.
    // Highlighting recolors runs only — layout does not move, so a build with
    // no highlighter installed paints the same block in one plain run.
    let spans = crate::highlight::spans(cx, language, code);
    let mono = font(theme.font_mono.clone());
    let run = |len: usize, color: Hsla| TextRun {
        len,
        font: mono.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    // Each line's own layout, with the slice of the code it covers — the caret
    // and a click both resolve through these.
    let mut rows: Vec<(Range<usize>, TextLayout)> = Vec::new();
    let mut offset = 0usize;
    let lines: Vec<AnyElement> = code
        .split('\n')
        .map(|line| {
            let start = offset;
            offset += line.len() + 1;
            let mut runs = Vec::new();
            // Runs are measured within the line; spans are byte ranges over the
            // whole block, so every span is clipped to the line and rebased.
            let mut pos = 0usize;
            if let Some(spans) = &spans {
                let end = start + line.len();
                for (range, kind) in spans.iter().filter(|(r, _)| r.end > start && r.start < end) {
                    let s = range.start.clamp(start, end) - start;
                    let e = range.end.min(end) - start;
                    if s > pos {
                        runs.push(run(s - pos, theme.text));
                    }
                    runs.push(run(e - s, theme.syntax.color(*kind)));
                    pos = e;
                }
            }
            if pos < line.len() {
                runs.push(run(line.len() - pos, theme.text));
            }
            if runs.is_empty() {
                runs.push(run(0, theme.text));
            }
            let styled = styled_line(line, runs);
            rows.push((start..start + line.len(), styled.layout().clone()));
            styled.into_any_element()
        })
        .collect();

    let caret = overlay.caret_painted();
    let selected = overlay.selected(code.len());
    let sink = overlay.layouts.cloned();
    let code_size = typography.code.size();
    let annotated = overlay.annotated(code.len(), theme);
    let (caret_color, selection_color) = (theme.caret, theme.selection);
    let underlay = canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            for (span, layout) in &rows {
                if let Some(sink) = &sink {
                    sink.record(ix, Part::Code, span.clone(), layout.clone());
                }
                for (range, wash) in &annotated {
                    let (from, to) = (range.start.max(span.start), range.end.min(span.end));
                    if from < to {
                        for rect in
                            range_rects(layout, &(from - span.start..to - span.start), 0.0, 0.0)
                        {
                            window.paint_quad(quad(
                                rect,
                                px(2.0),
                                *wash,
                                px(0.0),
                                gpui::transparent_black(),
                                BorderStyle::default(),
                            ));
                        }
                    }
                }
                if let Some(range) = &selected {
                    let (from, to) = (range.start.max(span.start), range.end.min(span.end));
                    if from < to {
                        for rect in
                            range_rects(layout, &(from - span.start..to - span.start), 0.0, 0.0)
                        {
                            window.paint_quad(quad(
                                rect,
                                px(2.0),
                                selection_color,
                                px(0.0),
                                gpui::transparent_black(),
                                BorderStyle::default(),
                            ));
                        }
                    }
                }
                if let Some(offset) = caret.filter(|at| span.contains(at) || *at == span.end)
                    && let Some(head) = layout.position_for_index(offset - span.start)
                {
                    window.paint_quad(quad(
                        caret_quad(head, code_size, layout.line_height()),
                        px(0.0),
                        caret_color,
                        px(0.0),
                        gpui::transparent_black(),
                        BorderStyle::default(),
                    ));
                }
            }
        },
    )
    .absolute()
    .size_full();

    // Bleed by the card's own pad so the plate shares edges with a
    // composer / prompt that does the same (`-mx` + matching `px`).
    div()
        .w_full()
        .ml(px(-CODE_PADDING_X))
        .mr(px(-CODE_PADDING_X))
        .rounded(px(Theme::panel_radius()))
        .bg(theme.ink(0.035))
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .relative()
        // The band is unconditional: it is where the copy button already floats,
        // and where a host puts its language control — which needs somewhere to
        // sit on a block that has no language yet.
        .child(
            div()
                .relative()
                .flex()
                .flex_row()
                .items_center()
                .px(px(CODE_PADDING_X))
                .py(px(5.0))
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.ink(0.02))
                .text_style(TextStyle::Subheadline)
                .text_color(match language {
                    Some(_) => theme.text_muted,
                    None => theme.text_faint,
                })
                // The label's own box, not the band's: a host hanging a picker
                // here wants it around the word, and only the word knows how
                // wide the word is.
                .child(
                    div()
                        .relative()
                        .children(overlay.layouts.map(|layouts| {
                            let layouts = layouts.clone();
                            canvas(
                                move |bounds, _, _| layouts.record_language(ix, bounds),
                                |_, _, _, _| (),
                            )
                            .absolute()
                            .size_full()
                        }))
                        .child(SharedString::from(
                            language.unwrap_or("text").to_string(),
                        )),
                ),
        )
        .child(
            div()
                .id(ElementId::named_usize("md-code", ix))
                .overflow_x_scroll()
                // Without it a scroll down the page turns sideways the moment
                // the pointer crosses a code block: gpui remaps input to
                // whichever axis a container can scroll.
                .restrict_scroll_to_axis()
                .relative()
                .px(px(CODE_PADDING_X))
                .py(px(CODE_PADDING_Y))
                .text_size(px(typography.code.size()))
                .line_height(px(typography.code.line_height()))
                .whitespace_nowrap()
                .child(underlay)
                .children(lines),
        )
        .child(copy_button(code, ix, theme, window, cx))
        .into_any_element()
}

fn copy_glyph(theme: &Theme) -> impl IntoElement {
    let line = theme.text_muted;
    div()
        .relative()
        .size(px(12.))
        .child(
            div()
                .absolute()
                .right(px(0.))
                .top(px(0.))
                .size(px(8.))
                .rounded(px(1.5))
                .border_1()
                .border_color(line),
        )
        .child(
            div()
                .absolute()
                .left(px(0.))
                .bottom(px(0.))
                .size(px(8.))
                .rounded(px(1.5))
                .border_1()
                .border_color(line)
                .bg(theme.ink(0.035)),
        )
}

/// A copy button that owns its own feedback.
///
/// The state is the element's, not the caller's: a component library cannot ask
/// every host to thread a handler and a "which block is showing Copied" index
/// through its render tree just to put a button on a code block. It resets when
/// the pointer leaves, which needs no clock.
fn copy_button(
    code: &str,
    ix: usize,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let copied = window.use_keyed_state(ElementId::named_usize("md-copied", ix), cx, |_, _| false);
    let showing = *copied.read(cx);
    let text: SharedString = code.to_string().into();

    div()
        .id(ElementId::named_usize("md-copy", ix))
        .absolute()
        .top(px(3.0))
        .right(px(5.0))
        .h(px(20.0))
        .px(px(6.0))
        .rounded(px(5.0))
        .flex()
        .items_center()
        .cursor_pointer()
        .text_style(TextStyle::Caption)
        .text_color(theme.text_muted)
        .hover(|el| el.bg(theme.element_hover))
        .child(if showing {
            div()
                .text_color(theme.text_muted)
                .child("✓")
                .into_any_element()
        } else {
            // Cursor: icon, not the word "Copy".
            copy_glyph(theme).into_any_element()
        })
        .on_click({
            let copied = copied.clone();
            move |_, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.to_string()));
                copied.update(cx, |state, cx| {
                    *state = true;
                    cx.notify();
                });
            }
        })
        .on_hover(move |hovering, _, cx| {
            if !*hovering && *copied.read(cx) {
                copied.update(cx, |state, cx| {
                    *state = false;
                    cx.notify();
                });
            }
        })
        .into_any_element()
}

/// A picture and the caption under it, which is the alt text a caret can reach.
///
/// The caption row appears when there is something to read or somewhere to
/// type, so a document being read is not a column of pictures each trailing a
/// blank line. With no URL yet the picture is a dashed row instead — the shape
/// the slash menu makes, waiting to be told what to show.
fn image(
    url: &str,
    alt: &Text,
    width: Option<u32>,
    overlay: Overlay,
    typography: &Typography,
    theme: &Theme,
) -> AnyElement {
    let hint = SharedString::new_static(CAPTION_HINT);
    let overlay = Overlay {
        placeholder: Some(&hint),
        ..overlay.at(Part::Caption)
    };
    let picture = if url.is_empty() {
        div()
            .h(px(IMAGE_EMPTY_HEIGHT))
            .flex()
            .items_center()
            .px(px(CARD_PADDING))
            .rounded(px(Theme::button_radius()))
            .border_1()
            .border_dashed()
            .border_color(theme.border)
            .text_size(px(typography.body.size()))
            .text_color(theme.text_muted)
            .child(IMAGE_EMPTY)
    } else {
        // A URL is fetched; anything else is a file, and gpui reads one only
        // from a `PathBuf` — handed a string it looks for an asset built into
        // the binary and paints nothing.
        let picture = match url.contains("://") {
            true => img(SharedString::from(url.to_string())),
            false => img(std::path::PathBuf::from(url)),
        };
        let box_ = div()
            .relative()
            .rounded(px(Theme::button_radius()))
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .children(overlay.layouts.map(|layouts| {
                let layouts = layouts.clone();
                let ix = overlay.block;
                canvas(
                    move |bounds, _, _| layouts.record_picture(ix, bounds),
                    |_, _, _, _| (),
                )
                .absolute()
                .size_full()
            }));
        match width {
            // A stated width is the box's: it hugs, so the border is around
            // the picture rather than around the column beside it, and the
            // picture fills what the box settled on — which `max_w_full`
            // holds inside the page however wide the width was written.
            Some(width) => box_
                .self_start()
                .max_w_full()
                .w(px(width as f32))
                .child(picture.w(px(width as f32)).max_w_full()),
            // Unstated, the picture scales itself against the column, which
            // is a percentage and so needs a box that spans one to measure.
            None => box_.child(picture.max_w_full()),
        }
    };
    div()
        .flex()
        .flex_col()
        .gap(px(CAPTION_GAP))
        .child(picture)
        // An empty caption still paints while the caret is in it, or there
        // would be nothing to type into and no hint saying so.
        .when(
            overlay.caption == Caption::Shown && (!alt.is_empty() || overlay.caret().is_some()),
            |el| {
                el.child(text_element(
                    alt,
                    typography.caption.size(),
                    typography.caption.line_height(),
                    FontWeight::NORMAL,
                    overlay,
                    theme,
                ))
            },
        )
        .into_any_element()
}

/// A bookmark, in Notion's proportions: a fixed-height row with the text on the
/// left and an image panel of a fixed width on the right, all of it one click
/// target. [`Form::Embed`] turns the row into a column and gives the image the
/// card's full width instead, and [`Form::Chip`] is neither — a pill of favicon
/// and title, which is what an inline mention would be if shaped text had
/// anywhere to put a picture.
///
/// The row is a fixed height with its footer pinned to the bottom, because a
/// preview resolves *after* the card has painted — a blurb arriving into a box
/// that grows would shove every block below it down the page. An embed's cover
/// holds that height, so its text hugs.
fn bookmark(
    ix: usize,
    url: &str,
    form: Form,
    typography: &Typography,
    theme: &Theme,
    cx: &App,
) -> AnyElement {
    let preview = preview::of(cx, url).unwrap_or_default();
    let host = SharedString::from(preview::host(url).to_string());
    let label = preview.label.clone().unwrap_or_else(|| host.clone());
    let title = preview
        .title
        .clone()
        .unwrap_or_else(|| SharedString::from(url.to_string()));

    // Owned, because the image panel's fallback outlives this call: gpui asks
    // for the replacement element only once the fetch has failed.
    let (icon, muted, wash) = (preview.icon.clone(), theme.text_muted, theme.element_hover);
    let site = host.clone();
    let mark = move |size: f32| {
        let host = site.clone();
        match icon.clone() {
            Some(icon) => img(icon)
                .size(px(size))
                .rounded(px(size / 4.0))
                .with_fallback(move || initial(&host, size, muted, wash))
                .into_any_element(),
            None => initial(&host, size, muted, wash),
        }
    };

    if form == Form::Chip {
        let open = url.to_string();
        let pill = div()
            .id(ElementId::named_usize("md-chip", ix))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(CHIP_BLOCK_PAD_X))
            .py(px(CHIP_BLOCK_PAD_Y))
            .rounded(px(Theme::control_radius()))
            .border_1()
            .border_color(theme.border)
            .bg(theme.element_hover)
            .text_size(px(typography.body.size()))
            .line_height(px(typography.body.line_height()))
            .text_color(theme.text)
            .cursor(CursorStyle::PointingHand)
            .hover(|el| el.bg(theme.element_active))
            .on_click(move |_, _, cx| cx.open_url(&open))
            .child(mark(CHIP_ICON))
            // The host, not the URL, when nothing has resolved it: a chip is
            // the short form, and a raw URL in a pill is the long one.
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(preview.title.unwrap_or(label)),
            );
        // A block's own box is `display: block`, where a pill would take the
        // whole width. One flex row around it is what lets it hug its label.
        return div().flex().flex_row().child(pill).into_any_element();
    }

    let words = div()
        .flex()
        .flex_col()
        .min_w_0()
        .px(px(CARD_PADDING))
        .py(px(CARD_PADDING - 2.0))
        .child(
            div()
                .truncate()
                .text_size(px(typography.body.size()))
                .line_height(px(typography.body.line_height()))
                .text_color(theme.text)
                .child(title),
        )
        .children(preview.description.map(|blurb| {
            div()
                .line_clamp(2)
                .text_size(px(typography.card.size()))
                .line_height(px(typography.card.line_height()))
                .text_color(theme.text_muted)
                .child(blurb)
        }))
        .child(
            div()
                .mt_auto()
                .pt(px(6.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(px(typography.card.size()))
                .text_color(theme.text_muted)
                .child(mark(CARD_ICON))
                .child(div().truncate().child(label)),
        );

    let picture = corners(div(), form)
        .bg(theme.surface)
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .child(match preview.image {
            Some(image) => corners(img(image).size_full().object_fit(ObjectFit::Cover), form)
                .with_fallback(move || mark(CARD_COVER))
                .into_any_element(),
            None => mark(CARD_COVER),
        });

    let open = url.to_string();
    let card = div()
        .id(ElementId::named_usize("md-bookmark", ix))
        .flex()
        .w_full()
        .overflow_hidden()
        .rounded(px(Theme::button_radius()))
        .border(px(CARD_BORDER))
        .border_color(theme.border)
        .bg(theme.surface_card)
        .cursor(CursorStyle::PointingHand)
        .hover(|el| el.bg(theme.element_hover))
        .on_click(move |_, _, cx| cx.open_url(&open));

    if form == Form::Embed {
        card.flex_col()
            .child(picture.w_full().h(px(CARD_COVER_HEIGHT)))
            .child(words.w_full())
    } else {
        card.h(px(CARD_HEIGHT))
            .child(words.flex_1())
            .child(picture.flex_none().w(px(CARD_IMAGE_WIDTH)).h_full())
    }
    .into_any_element()
}

/// The card's corners, on the panel that reaches them: a content mask is a
/// rectangle, so a picture paints square over a rounded card unless it carries
/// the radius itself, concentric inside the card's border.
fn corners<T: Styled>(element: T, form: Form) -> T {
    let corner = px(Theme::inset_radius(Theme::button_radius(), CARD_BORDER));
    match form {
        Form::Embed => element.rounded_t(corner),
        _ => element.rounded_r(corner),
    }
}

/// The mark a site gets before anyone has fetched its favicon: its host's first
/// letter, which is a placeholder no icon set has to ship.
fn initial(host: &str, size: f32, color: Hsla, wash: Hsla) -> AnyElement {
    div()
        .flex_none()
        .size(px(size))
        .rounded(px(size / 4.0))
        .bg(wash)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(size * 0.55))
        .text_color(color)
        .child(SharedString::from(
            host.chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string(),
        ))
        .into_any_element()
}

/// A GFM table.
///
/// Columns are content-proportional with a per-column floor: each cell is
/// shaped unwrapped to get its max-content width, and the flex resolution does
/// the rest. When even the floors no longer fit, the table scrolls sideways
/// rather than crushing every column into per-character wrapping.
fn table(
    align: &[Align],
    header: &[Text],
    rows: &[Vec<Text>],
    overlay: Overlay,
    typography: &Typography,
    theme: &Theme,
    window: &mut Window,
) -> AnyElement {
    let ix = overlay.block;
    let all: Vec<&[Text]> = std::iter::once(header)
        .filter(|row| !row.is_empty())
        .chain(rows.iter().map(|row| row.as_slice()))
        .collect();
    let columns = all.iter().map(|row| row.len()).max().unwrap_or(0);
    if columns == 0 {
        return gpui::Empty.into_any_element();
    }
    let has_header = !header.is_empty();

    let text_system = window.text_system();
    let mut flats: Vec<Vec<Option<Flat>>> = Vec::with_capacity(all.len());
    let mut content = vec![0.0f32; columns];
    for (r, row) in all.iter().enumerate() {
        let weight = if has_header && r == 0 {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        };
        let mut out = Vec::with_capacity(columns);
        for (c, natural) in content.iter_mut().enumerate() {
            let Some(cell) = row.get(c) else {
                out.push(None);
                continue;
            };
            let mut flat = flatten(cell, weight, theme);
            if overlay.bionic {
                bionic::apply(&mut flat);
            }
            if !flat.text.is_empty() {
                let width = f32::from(
                    text_system
                        .shape_line(
                            flat.text.clone(),
                            px(typography.body.size()),
                            &flat.runs,
                            None,
                        )
                        .width(),
                );
                *natural = natural.max(width);
            }
            out.push(Some(flat));
        }
        flats.push(out);
    }

    let naturals: Vec<f32> = content
        .iter()
        .map(|width| width.max(TABLE_MIN_COLUMN_CONTENT) + 2.0 * TABLE_CELL_PADDING)
        .collect();
    let minimums: Vec<f32> = naturals
        .iter()
        .map(|natural| natural.min(TABLE_MIN_COLUMN_WIDTH))
        .collect();
    let hairline = theme.hairline(0.10);

    // Cursor's table: a grid. Hairlines between rows and between cells, a
    // shaded header row, and a rounded border around the whole.
    let mut inner = div()
        .flex()
        .flex_col()
        .w_full()
        .min_w(px(minimums.iter().sum::<f32>()))
        .rounded(px(6.0))
        .border_1()
        .border_color(hairline)
        .overflow_hidden();
    for (r, row) in flats.into_iter().enumerate() {
        if r > 0 {
            inner = inner.child(div().flex_none().h(px(TABLE_DIVIDER)).w_full().bg(hairline));
        }
        let mut row_el = div().flex().flex_row();
        if has_header && r == 0 {
            row_el = row_el.bg(theme.element_hover);
        }
        for (c, cell) in row.into_iter().enumerate() {
            let mut cell_el = div()
                .flex_grow(naturals[c])
                .flex_shrink(naturals[c])
                .flex_basis(px(0.0))
                .min_w(px(minimums[c]))
                .px(px(TABLE_CELL_PADDING))
                .py(px(TABLE_CELL_PADDING - 2.0))
                .when(c > 0, |cell| cell.border_l_1().border_color(hairline))
                .text_size(px(typography.body.size()))
                .line_height(px(typography.body.line_height()));
            cell_el = match align.get(c).copied().unwrap_or_default() {
                Align::Left => cell_el,
                Align::Center => cell_el.text_center(),
                Align::Right => cell_el.text_right(),
            };
            if let Some(flat) = cell {
                // `all` drops an empty header, so a table without one starts at
                // part row 1 — row 0 is the header slot whether or not it is
                // filled.
                let row = if has_header { r } else { r + 1 };
                let len = flat.text.len();
                cell_el = cell_el.child(painted_text(
                    flat,
                    len,
                    typography.body.size(),
                    typography.body.line_height(),
                    overlay.at(Part::Cell { row, column: c }),
                    theme,
                ));
            }
            row_el = row_el.child(cell_el);
        }
        inner = inner.child(row_el);
    }

    div()
        .id(ElementId::named_usize("md-table", ix))
        .w_full()
        .overflow_x_scroll()
        .restrict_scroll_to_axis()
        .child(inner)
        .into_any_element()
}
