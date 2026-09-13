//! The composer attachment chip — reused in the transcript so a sent
//! file looks the same as it did on the card.

use bezel::{
    gpui::{Div, Image, SharedString, Stateful, div, img, prelude::*, px},
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

/// Wrap chips the way the composer tray does: a wrapping row, no empty band.
pub(crate) fn row() -> Div {
    div().w_full().flex().flex_row().flex_wrap().gap(px(6.))
}
