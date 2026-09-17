//! Text you can select with the pointer and copy out of.
//!
//! The two hard halves are already here: [`render_with`] paints a [`Selection`]
//! it is handed, and fills a [`BlockLayouts`] whose [`BlockLayouts::hit`] turns
//! a point back into a [`Cursor`]. What was missing is the *gesture* — press,
//! drag, release — which belonged to whoever owns the selection, and until now
//! only an editor ever did.
//!
//! This is that gesture and nothing else. Which item holds the selection, and
//! what copying means, stay the caller's.

use crate::{
    BlockLayouts, Cursor, Doc, Editing, Reveal, Selection, Typography, render_revealed,
    render_with,
};
use gpui::{
    AnyElement, Context, CursorStyle, ElementId, MouseButton, MouseDownEvent, MouseMoveEvent,
    Window, div, prelude::*,
};
use std::rc::Rc;

/// What the pointer did over the text.
pub enum Pointer {
    /// Pressed here — the start of a selection.
    Down(Cursor),
    /// Double-clicked text — select the whole document.
    SelectAll,
    /// Moved here with the button still down.
    Move(Cursor),
    /// Let go. Whatever the selection had become is what it is.
    Up,
    /// Moved here with no button down — `None` when the pointer left the
    /// text. For the hand and the underline over a link.
    Hover(Option<Cursor>),
}

/// Render `doc` with `selection` painted in it, reporting what the pointer does.
///
/// `dragging` is the caller's: a move only extends a selection that a press
/// started, and which item that press landed in is not something one block of
/// text can know.
///
/// `bionic` weights the front of each word — see [`crate::bionic`]. The
/// caller's too, because it is a reading aid for someone else's words.
///
/// Releasing is answered twice over — on the text and off it — because a drag
/// that ends past the edge of a paragraph is the ordinary way to select to the
/// end of one.
#[expect(
    clippy::too_many_arguments,
    reason = "a document, its selection, and a gesture"
)]
pub fn render<V: 'static>(
    id: impl Into<ElementId>,
    doc: &Doc,
    layouts: &BlockLayouts,
    selection: Option<Selection>,
    dragging: bool,
    hover_link: Option<Selection>,
    reveal: Option<&Reveal>,
    bionic: bool,
    window: &mut Window,
    cx: &mut Context<V>,
    on_pointer: impl Fn(&mut V, Pointer, &mut Context<V>) + 'static,
) -> AnyElement {
    let on_pointer = Rc::new(on_pointer);
    let (down, moved, up, off, left) = (
        on_pointer.clone(),
        on_pointer.clone(),
        on_pointer.clone(),
        on_pointer.clone(),
        on_pointer,
    );
    let (at_down, at_move, at_leave) = (layouts.clone(), layouts.clone(), layouts.clone());
    let hover_marks: Vec<(Selection, crate::render::Annotation)> = hover_link
        .into_iter()
        .map(|sel| (sel, crate::render::Annotation::LinkHover))
        .collect();
    div()
        .id(id)
        // The hand over a link, the I-beam over words: a URL in an answer
        // read as plain text until it was clicked (Jacob, report -31).
        .cursor(if hover_link.is_some() {
            CursorStyle::PointingHand
        } else {
            CursorStyle::IBeam
        })
        // gpui reports "not hovered" for this box while the pointer is
        // over its own painted text (the canvas child holds the hitbox),
        // so a leave counts only when the pointer is off the text itself.
        .on_hover(cx.listener(move |view, hovering: &bool, window, cx| {
            if !*hovering && at_leave.hit(window.mouse_position()).is_none() {
                left(view, Pointer::Hover(None), cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, event: &MouseDownEvent, _, cx| {
                if event.click_count >= 2 {
                    down(view, Pointer::SelectAll, cx);
                    cx.stop_propagation();
                } else if let Some(cursor) = at_down.hit(event.position) {
                    down(view, Pointer::Down(cursor), cx);
                }
            }),
        )
        .on_mouse_move(cx.listener(move |view, event: &MouseMoveEvent, _, cx| {
            if dragging {
                if let Some(cursor) = at_move.hit(event.position) {
                    moved(view, Pointer::Move(cursor), cx);
                }
            } else {
                moved(view, Pointer::Hover(at_move.hit(event.position)), cx);
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |view, _, _, cx| up(view, Pointer::Up, cx)),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(move |view, _, _, cx| off(view, Pointer::Up, cx)),
        )
        .child({
            let editing = Editing {
                selection,
                // Read-only text has no caret. Without this a collapsed
                // selection — every press that starts one — would blink an
                // insertion point in text nobody can type into.
                caret_on: false,
                layouts: Some(layouts),
                annotations: &hover_marks,
                // `None` when off, so the installed set is read at paint the
                // way it is everywhere else.
                typography: bionic.then(|| Typography::of(cx).bionic(true)),
                ..Editing::default()
            };
            match reveal {
                Some(reveal) => render_revealed(doc, editing, reveal, window, cx),
                None => render_with(doc, editing, window, cx),
            }
        })
        .into_any_element()
}

/// The text a selection covers, as it would be pasted.
///
/// [`Doc::spans`] answers in parts — a paragraph, a cell, a line of a fence —
/// and a newline between them is what puts a multi-block selection back
/// together.
pub fn copied(doc: &Doc, selection: Selection) -> String {
    doc.spans(selection)
        .into_iter()
        .filter_map(|(at, range)| {
            let text = &doc.blocks.get(at.block)?.text_at(at.part)?.text;
            text.get(range).map(str::to_owned)
        })
        .collect::<Vec<_>>()
        .join("\n")
}
