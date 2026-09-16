//! The composer attachment chip — reused in the transcript so a sent
//! file looks the same as it did on the card.

use bezel::{
    gpui::{Div, Image, ObjectFit, SharedString, Stateful, div, img, prelude::*, px},
    theme::{TextStyle, Theme, Typeset},
    ui::icons,
};
use std::sync::Arc;

/// One file or image chip: thumbnail or document glyph, then the name.
/// The composer adds an ✕; history does not.
pub(crate) fn chip(
    id: impl Into<bezel::gpui::ElementId>,
    name: impl Into<SharedString>,
    preview: Option<Arc<Image>>,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.))
        .h(px(if preview.is_some() { 54. } else { 22. }))
        .px(px(7.))
        .rounded(px(Theme::control_radius()))
        .bg(theme.element_hover)
        .child(match preview {
            Some(preview) => img(preview)
                .size(px(44.))
                .with_fallback(|| div().size(px(44.)).into_any_element())
                .into_any_element(),
            None => icons::icon(icons::files::DOCUMENT)
                .size(px(11.))
                .text_color(theme.text_faint)
                .into_any_element(),
        })
        .child(
            div()
                .max_w(px(140.))
                .truncate()
                .text_style(TextStyle::Caption)
                .text_color(theme.text_muted)
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
