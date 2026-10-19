//! Text selection for the chat transcript: drag, double-click (word),
//! triple-click (paragraph), shift-click, ⌘C. Selection lives in one text
//! block at a time — a paragraph, heading, list item, code block or user
//! bubble — as byte offsets into that block's rendered plain text.
//!
//! [`TextSelection`] is the model (an entity the transcript observes);
//! [`SelectableText`] wraps a [`StyledText`] and wires the mouse to the model
//! from inside its own paint, where the text layout is guaranteed to exist.

use std::ops::Range;

use gpui::{
    point, px, App, Bounds, Context, CursorStyle, DispatchPhase, Element, ElementId, Entity,
    FocusHandle, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    SharedString, StyledText, TextLayout, TextRun, Window,
};
use unicode_segmentation::UnicodeSegmentation;

/// What one click, two clicks or three clicks select, and what a drag that
/// started with them extends by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Granularity {
    Character,
    Word,
    Paragraph,
}

#[derive(Clone, Debug)]
struct Active {
    key: SharedString,
    /// The block's text when the selection was made; copy reads from this so
    /// a streaming block cannot shift the offsets under the user.
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
}

impl TextSelection {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            model: Model::default(),
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
}

/// The selection itself, free of GPUI handles so it can be unit-tested.
#[derive(Default, Debug)]
struct Model {
    active: Option<Active>,
}

impl Model {
    fn selected_text(&self) -> Option<String> {
        let active = self.active.as_ref()?;
        let range = active.range();
        if range.is_empty() {
            return None;
        }
        active.text.get(range).map(str::to_string)
    }

    /// `true` while a mouse drag that started in `key` is in progress.
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

    /// Mouse down at byte `index` of `text`: one click arms a drag, shift
    /// extends, two clicks take the word, three the paragraph.
    fn begin(
        &mut self,
        key: SharedString,
        text: SharedString,
        index: usize,
        click_count: usize,
        shift: bool,
    ) {
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
    fn extend_to(&mut self, index: usize) -> bool {
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

    /// Mouse up: `true` when the gesture ended as a plain click (no range).
    fn finish_drag(&mut self) -> bool {
        let Some(a) = self.active.as_mut() else {
            return true;
        };
        a.dragging = false;
        if a.range().is_empty() {
            self.active = None;
            return true;
        }
        false
    }
}

/// The per-render snapshot of the selection, plus what a text block needs to
/// draw its highlight and wire its mouse events.
#[derive(Clone)]
pub struct Selectable {
    entity: Entity<TextSelection>,
    current: Option<(SharedString, SharedString, Range<usize>)>,
    color: Hsla,
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
        }
    }

    /// The same selection drawn in another colour (dark bubbles need more).
    pub fn with_color(&self, color: Hsla) -> Self {
        Self {
            color,
            ..self.clone()
        }
    }

    /// A selectable text block. `runs` cover `text` exactly; the selection
    /// highlight is painted as run backgrounds so it follows soft wraps.
    pub fn text(
        &self,
        key: impl Into<SharedString>,
        text: impl Into<SharedString>,
        runs: Vec<TextRun>,
    ) -> SelectableText {
        let key = key.into();
        let text = text.into();
        let runs = match &self.current {
            Some((k, t, range)) if *k == key && *t == text => {
                highlight(runs, range.clone(), self.color)
            }
            _ => runs,
        };
        SelectableText {
            styled: StyledText::new(text.clone()).with_runs(runs),
            key,
            text,
            links: Vec::new(),
            selection: self.entity.clone(),
        }
    }
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

/// A [`StyledText`] the user can select from. Registers its own mouse
/// listeners during paint (the same way `InteractiveText` does).
pub struct SelectableText {
    key: SharedString,
    text: SharedString,
    styled: StyledText,
    links: Vec<(Range<usize>, String)>,
    selection: Entity<TextSelection>,
}

impl SelectableText {
    /// Clickable link ranges: a plain click (no drag) inside one opens it.
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
        self.styled
            .paint(None, inspector_id, bounds, state, &mut (), window, cx);
        let layout = self.styled.layout().clone();
        let links = std::mem::take(&mut self.links);
        let selection = self.selection.clone();
        let focus_handle = selection.read(cx).focus_handle.clone();
        let (key, text) = (self.key.clone(), self.text.clone());

        let over_link = hitbox.is_hovered(window)
            && layout
                .index_for_position(window.mouse_position())
                .is_ok_and(|ix| links.iter().any(|(r, _)| r.contains(&ix)));
        window.set_cursor_style(
            if over_link {
                CursorStyle::PointingHand
            } else {
                CursorStyle::IBeam
            },
            hitbox,
        );

        window.on_mouse_event({
            let (hitbox, layout, selection, focus_handle) = (
                hitbox.clone(),
                layout.clone(),
                selection.clone(),
                focus_handle.clone(),
            );
            let (key, text) = (key.clone(), text.clone());
            move |event: &MouseDownEvent, phase, window, cx| {
                // Control-click is the bubble's copy; right-click its react menu.
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || event.modifiers.control
                    || !hitbox.is_hovered(window)
                {
                    return;
                }
                let index = closest_index(&layout, event.position);
                let selected = selection.update(cx, |s, cx| {
                    s.model.begin(
                        key.clone(),
                        text.clone(),
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
                let index = closest_index(&layout, event.position);
                let selected = selection.update(cx, |s, cx| {
                    if s.model.extend_to(index) {
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
                    cx.notify();
                    plain
                });
                if plain_click && event.click_count == 1 && hitbox.is_hovered(window) {
                    if let Ok(ix) = layout.index_for_position(event.position) {
                        if let Some((_, href)) = links.iter().find(|(r, _)| r.contains(&ix)) {
                            cx.open_url(href);
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{FontStyle, FontWeight};

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
    }

    #[test]
    fn shift_click_extends_the_existing_selection() {
        let text: SharedString = "alpha beta gamma".into();
        let mut sel = Model::default();
        sel.begin("k".into(), text.clone(), 0, 2, false);
        sel.finish_drag();
        sel.begin("k".into(), text.clone(), 12, 1, true);
        assert_eq!(sel.selected_text().as_deref(), Some("alpha beta gamma"));
        // Shift-click in another block starts over there.
        sel.begin("other".into(), "zeta".into(), 1, 1, true);
        assert_eq!(sel.selected_text(), None);
    }
}
