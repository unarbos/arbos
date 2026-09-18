//! The composer's attachment tokens and thumbnails — reused in the
//! transcript so a sent file looks the same as it did on the card.

use bezel::{
    gpui::{Div, Image, ObjectFit, SharedString, Stateful, div, img, prelude::*, px},
    theme::{TextStyle, Theme, Typeset},
    ui::icons,
};
use std::sync::Arc;

/// A file attachment as Cursor sets it: `≡ name` in the link colour, in
/// the text row itself, no plate — a reference in the sentence rather than
/// a chip above it (`cursor-reference/composer-attachments/`, cycle 21;
/// closed cycle 39, F-109). The same token leads the sent bubble's text.
/// `line` is the height of the text line it sits on — the field's own,
/// or the prose's wider lead — so the name centres on the words beside it.
pub(crate) fn token(
    id: impl Into<bezel::gpui::ElementId>,
    name: impl Into<SharedString>,
    line: f32,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(3.))
        .h(px(line))
        .child(
            icons::icon(icons::editing::LIST)
                .size(px(12.))
                .text_color(theme.accent),
        )
        .child(
            div()
                .max_w(px(200.))
                .truncate()
                .text_style(TextStyle::Body)
                .text_color(theme.accent)
                .child(name.into()),
        )
}

/// An image attachment as Cursor draws it: the picture alone, rounded, no
/// name — a thumbnail is its own label. `height` is the tray's 64 or the
/// sent card's 40; the width follows the picture up to `max_width`.
pub(crate) fn thumb(
    id: impl Into<bezel::gpui::ElementId>,
    preview: Option<Arc<Image>>,
    height: f32,
    max_width: f32,
    theme: &Theme,
) -> Stateful<Div> {
    let radius = px(8.);
    div()
        .id(id)
        .h(px(height))
        .max_w(px(max_width))
        .rounded(radius)
        .overflow_hidden()
        .bg(theme.element_hover)
        .child(match preview {
            Some(preview) => img(preview)
                .h(px(height))
                .max_w(px(max_width))
                .object_fit(ObjectFit::Cover)
                .rounded(radius)
                .with_fallback(move || div().size(px(height)).into_any_element())
                .into_any_element(),
            None => div().size(px(height)).into_any_element(),
        })
}

/// Wrap chips the way the composer tray does: a wrapping row, no empty band.
pub(crate) fn row() -> Div {
    div().w_full().flex().flex_row().flex_wrap().gap(px(6.))
}
