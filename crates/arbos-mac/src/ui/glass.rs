//! The iPhone app's glass controls, in GPUI: the glass circle (header
//! buttons), the glass pill (header title), grouped cards and rows for the
//! sheets, menu rows, and the centered sheet scaffold every modal uses.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, Div, ElementId, FontWeight, InteractiveElement, MouseButton, SharedString,
    Stateful, WindowControlArea,
};

use crate::ui::frost::{frosted, MENU_BLUR, PANEL_BLUR};
use crate::ui::icons::icon;
use crate::ui::theme::{Theme, PANEL_RADIUS, TEXT_BODY, TEXT_SMALL, TEXT_TINY};

/// Drag this region to move the window.
///
/// macOS windows open with `app_owns_titlebar_drag`, so AppKit will not move
/// the window itself. Mark the title-strip (and any other chrome the user
/// should pull) with this; a left-drag calls [`gpui::Window::start_window_move`].
/// Interactive children should `cx.stop_propagation()` on mouse-down so a
/// click on a button does not start a move.
pub trait WindowDrag: InteractiveElement + Sized {
    fn window_drag(self) -> Self {
        let pending = Rc::new(Cell::new(false));
        self.window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, {
                let pending = pending.clone();
                move |_, _, _| pending.set(true)
            })
            .on_mouse_up(MouseButton::Left, {
                let pending = pending.clone();
                move |_, _, _| pending.set(false)
            })
            .on_mouse_up_out(MouseButton::Left, {
                let pending = pending.clone();
                move |_, _, _| pending.set(false)
            })
            .on_mouse_down_out({
                let pending = pending.clone();
                move |_, _, _| pending.set(false)
            })
            .on_mouse_move({
                let pending = pending.clone();
                move |_, window, _| {
                    if pending.get() {
                        pending.set(false);
                        window.start_window_move();
                    }
                }
            })
    }
}

impl<E: InteractiveElement> WindowDrag for E {}

/// A glass circle: field fill at 85 %, hairline rim. Content centred.
pub fn circle(id: impl Into<ElementId>, size: f32, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(size))
        .flex_shrink_0()
        .rounded_full()
        .bg(theme.glass_fill())
        .border_1()
        .border_color(theme.field_border)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_2))
}

/// Filled primary button (white on dark): the iPhone CTA.
pub fn primary_button(id: impl Into<ElementId>, label: &str, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(38.0))
        .px(px(18.0))
        .rounded_full()
        .bg(theme.primary)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(TEXT_SMALL))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.on_primary)
        .cursor_pointer()
        .hover(|s| s.opacity(0.88))
        .child(label.to_string())
}

/// Quiet pill button (field fill, rim): Cancel / Deny / secondary actions.
pub fn quiet_button(id: impl Into<ElementId>, label: &str, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(38.0))
        .px(px(18.0))
        .rounded_full()
        .bg(theme.field)
        .border_1()
        .border_color(theme.field_border)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(TEXT_SMALL))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text)
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_2))
        .child(label.to_string())
}

/// Small capsule chip ("On Arbos", "owner", "3 agents").
pub fn chip(text: impl Into<SharedString>, color: gpui::Hsla) -> Div {
    div()
        .px(px(8.0))
        .py(px(2.0))
        .rounded_full()
        .bg(Theme::with_alpha(color, 0.16))
        .text_size(px(TEXT_TINY))
        .text_color(color)
        .flex_shrink_0()
        .whitespace_nowrap()
        .child(text.into())
}

/// Section label above a grouped card ("ACCOUNT").
pub fn section_label(text: &str, theme: &Theme) -> Div {
    div()
        .px(px(16.0))
        .pb(px(6.0))
        .text_size(px(TEXT_TINY))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text_muted)
        .child(text.to_uppercase())
}

/// A grouped card: rows separated by hairlines, 14 px corners.
pub fn group(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(PANEL_RADIUS))
        .bg(theme.surface_1)
        .overflow_hidden()
}

/// One row in a grouped card: label left, value right. Not interactive.
pub fn row(label: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .min_h(px(44.0))
        .px(px(16.0))
        .py(px(10.0))
        .border_b_1()
        .border_color(theme.border)
        .text_size(px(TEXT_BODY))
        .text_color(theme.text)
        .child(div().flex_1().min_w_0().child(label.into()))
}

/// Apple-style icon tile for settings rows: 30 px rounded square, muted
/// monochrome fill, white glyph (D80).
pub fn icon_tile(glyph: &str, theme: &Theme) -> Div {
    div()
        .size(px(30.0))
        .flex_shrink_0()
        .rounded(px(7.5))
        .bg(match theme.appearance {
            crate::ui::theme::Appearance::Dark => gpui::Hsla::from(gpui::rgb(0x5c5c60)),
            crate::ui::theme::Appearance::Light => gpui::Hsla::from(gpui::rgb(0x6e6e73)),
        })
        .flex()
        .items_center()
        .justify_center()
        .child(icon(glyph, 15.0, gpui::white()))
}

/// A tappable row with an Apple-style icon tile in front of the label (D80 settings).
pub fn nav_row_icon(
    id: impl Into<ElementId>,
    glyph: &str,
    label: impl Into<SharedString>,
    detail: Option<String>,
    theme: &Theme,
) -> Stateful<Div> {
    nav_row_with(id, Some(glyph), label, detail, theme)
}

fn nav_row_with(
    id: impl Into<ElementId>,
    glyph: Option<&str>,
    label: impl Into<SharedString>,
    detail: Option<String>,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(10.0))
        .min_h(px(44.0))
        .px(px(16.0))
        .border_b_1()
        .border_color(theme.border)
        .text_size(px(TEXT_BODY))
        .text_color(theme.text)
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_2))
        .when_some(glyph, |el, g| el.child(icon_tile(g, theme)))
        .child(div().flex_1().min_w_0().child(label.into()))
        .when_some(detail, |el, d| {
            el.child(
                div()
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_muted)
                    .child(d),
            )
        })
        .child(icon("chevron-right", 13.0, theme.text_faint))
}

/// Segmented choice inside a row: `[Dark] Light`.
pub fn segment(id: impl Into<ElementId>, label: &str, on: bool, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(28.0))
        .px(px(12.0))
        .rounded_full()
        .flex()
        .items_center()
        .text_size(px(TEXT_SMALL))
        .font_weight(if on {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .bg(if on {
            theme.bubble
        } else {
            gpui::transparent_black()
        })
        // The on-segment sits on the user-bubble colour (black in light), so it
        // takes the bubble's text colour, not the page text colour.
        .text_color(if on {
            theme.bubble_text()
        } else {
            theme.text_muted
        })
        .cursor_pointer()
        .when(!on, |el| el.hover(|s| s.text_color(theme.text)))
        .child(label.to_string())
}

/// Menu row (the "…" menu): glyph, label, optional trailing value.
pub fn menu_row(
    id: impl Into<ElementId>,
    glyph: &str,
    label: impl Into<SharedString>,
    danger: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let color = if danger { theme.danger } else { theme.text };
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(10.0))
        .h(px(36.0))
        .px(px(12.0))
        .rounded(px(9.0))
        .text_size(px(TEXT_SMALL))
        .text_color(color)
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_2))
        .child(icon(glyph, 15.0, color))
        .child(div().flex_1().min_w_0().truncate().child(label.into()))
}

/// Wrap a floating popover so clicks inside it never reach the root's
/// "click elsewhere closes popovers" handler.
pub fn popover(child: impl IntoElement) -> Div {
    div()
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(child)
}

pub fn menu_divider(theme: &Theme) -> Div {
    div().h(px(1.0)).my(px(4.0)).bg(theme.border)
}

/// A dropdown card under a header control: frosted, rounded, shadowed.
pub fn menu_card(width: f32, theme: &Theme) -> Div {
    div()
        .w(px(width))
        .flex()
        .flex_col()
        .gap(px(1.0))
        .p(px(6.0))
        .rounded(px(PANEL_RADIUS))
        .bg(theme.card())
        .border_1()
        .border_color(theme.border_strong)
        .shadow_lg()
}

pub fn frosted_menu(width: f32, theme: &Theme, body: Div) -> impl IntoElement {
    frosted(PANEL_RADIUS, MENU_BLUR, menu_card(width, theme).child(body))
}

/// Centered sheet with a dimmed backdrop: the iPhone's `.sheet`. `header`
/// is the title row (left control, centred title, right control).
pub fn sheet(width: f32, theme: &Theme, header: Div, body: impl IntoElement) -> Div {
    let card = div()
        .w(px(width))
        .max_w_full()
        .max_h_full()
        .flex()
        .flex_col()
        .rounded(px(PANEL_RADIUS + 6.0))
        .bg(theme.card())
        .border_1()
        .border_color(theme.border_strong)
        .shadow_lg()
        .child(header)
        .child(body);
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .p(px(24.0))
        .bg(Theme::with_alpha(theme.bg, 0.55))
        .child(frosted(PANEL_RADIUS + 6.0, PANEL_BLUR, card))
}

/// Sheet header: `[left] Title [right]`.
pub fn sheet_header(
    title: &str,
    left: Option<Stateful<Div>>,
    right: Option<Stateful<Div>>,
    theme: &Theme,
) -> Div {
    div()
        .relative()
        .h(px(60.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_between()
        .px(px(14.0))
        // The centred title paints first so the corner buttons stay clickable.
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(TEXT_BODY + 1.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(title.to_string()),
        )
        .child(div().min_w(px(72.0)).children(left))
        .child(div().min_w(px(72.0)).flex().justify_end().children(right))
}

/// Text button for sheet headers ("Cancel", "Done"): a glass pill.
pub fn sheet_button(id: impl Into<ElementId>, label: &str, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(34.0))
        .px(px(14.0))
        .rounded_full()
        .bg(theme.glass_fill())
        .border_1()
        .border_color(theme.field_border)
        .flex()
        .items_center()
        .text_size(px(TEXT_SMALL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text)
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_2))
        .child(label.to_string())
}

/// A macOS-style switch (read-only picture; the row around it is the control).
pub fn switch(on: bool, theme: &Theme) -> Div {
    div()
        .flex_shrink_0()
        .w(px(38.0))
        .h(px(22.0))
        .rounded_full()
        .bg(if on {
            theme.online
        } else {
            theme.border_strong
        })
        .flex()
        .items_center()
        .px(px(2.0))
        .when(on, |el| el.justify_end())
        .child(
            div()
                .size(px(18.0))
                .rounded_full()
                .bg(gpui::white())
                .shadow_sm(),
        )
}
