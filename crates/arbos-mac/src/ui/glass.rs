//! The iPhone app's glass controls, in GPUI: the glass circle (header
//! buttons), the glass pill (header title), grouped cards and rows for the
//! sheets, menu rows, and the centered sheet scaffold every modal uses.

use gpui::{
    div, prelude::*, px, AnyElement, BoxShadow, Div, ElementId, FontWeight, Global,
    InteractiveElement, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, SharedString,
    Stateful, Window, WindowControlArea,
};

use crate::ui::frost::{frosted, frosted_always, MENU_BLUR, PANEL_BLUR};
use crate::ui::highlight;
use crate::ui::icons::icon;
use crate::ui::profiles::kit;
use crate::ui::sf_symbols;
use crate::ui::theme::{Theme, CONTROL, PANEL_RADIUS, TEXT_BODY, TEXT_SMALL, TEXT_TINY};

/// Drag this region to move the window.
///
/// macOS windows open with `app_owns_titlebar_drag`, so AppKit will not move
/// the window itself. Mark the title-strip (and any other chrome the user
/// should pull) with this; a left-drag calls [`gpui::Window::start_window_move`],
/// a double-click does what the title bar does (zoom, per the system setting).
/// Interactive children should `cx.stop_propagation()` on mouse-down so a
/// click on a button does not start a move.
/// How far the pointer travels (pt) after a press before the window moves.
/// A press that stays put is a click for whatever is under it (a bubble in
/// the transcript's top fade), so the drag catchment can overlap content.
pub const WINDOW_DRAG_SLOP: f32 = 4.0;

/// The press that may become a window move. A global, not a per-render cell:
/// element listeners belong to the frame that rendered them, and anything
/// animating (a leaf avatar's sway, a streaming dot) re-renders every frame,
/// so a cell created inside `window_drag()` lost the press between mouse-down
/// and the first mouse-move and the window could not be dragged at all
/// (v0.6.11x–v0.6.131, [feedback mac-window-drag-hotfix]).
#[derive(Default)]
pub struct WindowDragPress {
    /// Where the left button went down inside a drag region.
    pub start: Option<Point<Pixels>>,
    /// Window moves requested so far (tests read this).
    pub moves: u32,
    /// Tests: record the request instead of asking the platform.
    pub record_only: bool,
}

impl Global for WindowDragPress {}

impl WindowDragPress {
    fn request_move(&mut self, window: &Window) {
        self.start = None;
        self.moves += 1;
        if !self.record_only {
            window.start_window_move();
        }
    }
}

pub trait WindowDrag: InteractiveElement + Sized {
    fn window_drag(self) -> Self {
        self.window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |event: &MouseDownEvent, window, cx| {
                let press = cx.default_global::<WindowDragPress>();
                if event.click_count == 2 {
                    press.start = None;
                    if !press.record_only {
                        window.titlebar_double_click();
                    }
                } else {
                    press.start = Some(event.position);
                }
            })
            .on_mouse_up(MouseButton::Left, |_, _, cx| {
                cx.default_global::<WindowDragPress>().start = None;
            })
            .on_mouse_up_out(MouseButton::Left, |_, _, cx| {
                cx.default_global::<WindowDragPress>().start = None;
            })
            .on_mouse_down_out(|_, _, cx| {
                cx.default_global::<WindowDragPress>().start = None;
            })
            .on_mouse_move(|event: &MouseMoveEvent, window, cx| {
                let press = cx.default_global::<WindowDragPress>();
                if let Some(start) = press.start {
                    let dx: f32 = (event.position.x - start.x).into();
                    let dy: f32 = (event.position.y - start.y).into();
                    if dx * dx + dy * dy >= WINDOW_DRAG_SLOP * WINDOW_DRAG_SLOP {
                        press.request_move(window);
                    }
                }
            })
    }
}

impl<E: InteractiveElement> WindowDrag for E {}

/// Glyph size for the right pane's close `×` in a [`CONTROL`] disc: the xmark
/// spans 12 of the 24 grid, so 22 paints Grok Bot's 11 pt mark (measured on
/// `grok-right-pane-details.png`, 2 px/pt: × 22 × 22 px, disc 72 px).
pub const PANE_CLOSE_GLYPH: f32 = 22.0;
/// Glyph size for the right pane's share disc: the `share` icon spans 16 of
/// 24, so 21 paints Grok's 14 pt mark (28 × 28 px in the same shot).
pub const PANE_SHARE_GLYPH: f32 = 21.0;

/// One disc of the round-button family (Jacob 2026-10-24, "one button
/// family"): the sidebar strip's search · + · collapse, the folded-sidebar
/// toggle in the chat header and the right pane's share and close. [`CONTROL`]
/// 36 pt, `surface_2` fill, the glyph in `text`, the hover lift. Grok Bot's
/// `#1F1F1F` disc with an `#E0E0E0` glyph.
pub fn control_disc(
    id: impl Into<ElementId>,
    glyph: &str,
    glyph_size: f32,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(CONTROL))
        .flex_shrink_0()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .map(|el| highlight::washed(el, theme.surface_2, theme))
        .child(icon(glyph, glyph_size, theme.text))
}

/// What a chat-header control is made of ([feedback mac-header-glass-2026-10-25],
/// [feedback mac-header-pill-glass-matte-2026-10-26]; the iPhone's
/// `HeaderGlassBackground`).
///
/// Jacob's reference is Grok Bot's header: frosted pills, the transcript
/// visibly blurred through them, a light rim — over a flat dark background.
/// #1830 gave the header that frost on Glass and left Matte solid; Jacob's
/// Mac is on Matte, so he still saw a flat `bubble_assistant` pill
/// (2026-10-26). The header pill and disc are frosted glass on BOTH surfaces
/// now: Matte means no window vibrancy, not a flat header. Nothing else of
/// the glass restyle moves with it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeaderStyle {
    /// White-alpha over a dark underlay (dark), a translucent white or grey
    /// (light): the list shows through.
    pub fill: gpui::Hsla,
    /// The 1 pt light rim.
    pub rim: gpui::Hsla,
    /// The specular line along the top edge.
    pub highlight: gpui::Hsla,
}

#[cfg(test)]
impl HeaderStyle {
    /// Frosted glass: a see-through fill with a visible rim. Read by the
    /// tests on every chat kind × surface × appearance.
    pub fn is_frosted(&self) -> bool {
        self.fill.a < 1.0 && self.rim.a > 0.0
    }
}

/// The pure part of a header control's look, for the tests.
pub fn header_style(theme: &Theme) -> HeaderStyle {
    HeaderStyle {
        fill: theme.header_glass(),
        rim: theme.header_glass_rim(),
        highlight: theme.header_glass_highlight(),
    }
}

/// Fill a header control (the name pill, the header disc): the glass fill,
/// an inset 1 pt rim, a 1 pt specular line along the top edge and a 1 pt
/// lift — the "glass plate" of `docs/glass-surface-directions.md` — on Glass
/// and on Matte alike.
pub fn header_surface<E: Styled>(el: E, theme: &Theme) -> E {
    let style = header_style(theme);
    el.bg(style.fill).shadow(vec![
        BoxShadow::new(px(0.0), px(0.0), style.rim)
            .spread_radius(px(1.0))
            .inset(),
        BoxShadow::new(px(0.0), px(1.0), style.highlight).inset(),
        BoxShadow::new(px(0.0), px(1.0), gpui::hsla(0.0, 0.0, 0.0, 0.28)).blur_radius(px(3.0)),
    ])
}

/// The blur under a header control ([feedback
/// mac-header-pill-see-through-2026-10-26]): light enough that a line of
/// transcript scrolling under the pill stays legible through it — Grok Bot's
/// pill softens the text behind, it does not erase it. The menus' 16 pt
/// (`MENU_BLUR`) turned it to mush, and with nothing behind it the pill then
/// read as a solid chip (Jacob's 14:15 CR shot of v0.6.275).
pub const HEADER_BLUR: f32 = 5.0;

/// Blur the list under a header control (its rounded bounds), whatever the
/// surface.
pub fn header_frost(radius: f32, el: impl IntoElement) -> AnyElement {
    frosted_always(radius, HEADER_BLUR, el).into_any_element()
}

/// A glass circle: field fill at 85 %, hairline rim. Content centred.
pub fn circle(id: impl Into<ElementId>, size: f32, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(size))
        .flex_shrink_0()
        .rounded_full()
        .border_1()
        .border_color(theme.field_border)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .map(|el| highlight::washed(el, theme.glass_fill(), theme))
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
        .border_1()
        .border_color(theme.field_border)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(TEXT_SMALL))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text)
        .cursor_pointer()
        .map(|el| highlight::washed(el, theme.field, theme))
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
///
/// `flex_shrink_0` is required: `overflow_hidden` makes the automatic
/// minimum height zero, so inside a scrolling flex column the card would
/// otherwise shrink and clip its rows (Settings, New Message, profiles).
pub fn group(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .rounded(px(PANEL_RADIUS))
        .bg(theme.surface_1)
        .overflow_hidden()
}

/// One row in a grouped card: label left, value right. Not interactive.
pub fn row(label: impl Into<SharedString>, theme: &Theme) -> Div {
    row_frame(theme)
        .child(div().flex_1().min_w_0().child(label.into()))
        .child(kit::hairline_below(theme))
}

/// [`row`] that reacts: the highlight pill under it (`ui::highlight`, inset
/// 4, concentric with the [`group`] card), pointer cursor, and the row is its
/// pill's group — the Machine page's breakdown row, a secret's switch row.
pub fn hot_row(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    highlight::row(
        row_frame(theme).id(id).cursor_pointer(),
        false,
        highlight::Shape::Nested(PANEL_RADIUS),
        theme,
    )
    .child(div().flex_1().min_w_0().child(label.into()))
    .child(kit::hairline_below(theme))
}

fn row_frame(theme: &Theme) -> Div {
    // The pane kit's metrics (profiles::kit): 48 pt rows, 16 pt inset. The
    // hairline is the kit's one divider, hung 1 pt below the row: between
    // rows it lands on the next row's first pixel, under the last row the
    // card's clip swallows it. A bottom border inside the row drew a stray
    // line under a one-row card (Settings › Notifications, Jacob Oct 21).
    div()
        .relative()
        .flex()
        .items_center()
        .gap(px(12.0))
        .min_h(px(48.0))
        .px(px(kit::INSET))
        .py(px(6.0))
        .text_size(px(TEXT_BODY))
        .text_color(theme.text)
}

/// Apple-style icon tile for settings rows: 30 px rounded square, muted
/// monochrome fill, white glyph (D80).
pub fn icon_tile(glyph: &str, theme: &Theme) -> Div {
    icon_tile_sized(glyph, 30.0, theme)
}

/// The settings tile at another size (the palette's 24 pt row picture);
/// radius and glyph scale with it.
pub fn icon_tile_sized(glyph: &str, size: f32, theme: &Theme) -> Div {
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded(px(size * 0.25))
        .bg(match theme.appearance {
            crate::ui::theme::Appearance::Dark => gpui::Hsla::from(gpui::rgb(0x5c5c60)),
            crate::ui::theme::Appearance::Light => gpui::Hsla::from(gpui::rgb(0x6e6e73)),
        })
        .flex()
        .items_center()
        .justify_center()
        .child(icon(glyph, size * 0.5, gpui::white()))
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

/// Geometry of a [`frosted_menu`] / [`menu_card`], shared with the code that
/// places a menu inside the window (`message_menu::menu_size`): one row,
/// the gap between children, the card's padding and border, a divider's
/// full height (1 pt line + 4 pt margins).
pub const MENU_ROW_HEIGHT: f32 = 36.0;
pub const MENU_GAP: f32 = 1.0;
pub const MENU_CARD_PADDING: f32 = 6.0;
/// The row is its own highlight pill inside the plate's padding: concentric
/// with the plate ([`PANEL_RADIUS`] − `MENU_CARD_PADDING`), as every row's
/// highlight is with its card (`ui::highlight`).
pub const MENU_ROW_RADIUS: f32 = PANEL_RADIUS - MENU_CARD_PADDING;
pub const MENU_CARD_BORDER: f32 = 1.0;
pub const MENU_DIVIDER_HEIGHT: f32 = 9.0;

/// Every menu glyph is a system SF Symbol at the label's point size
/// (`TEXT_SMALL`), regular weight, in a fixed slot so rows align — the same
/// names the iPhone passes to `Image(systemName:)` ([feedback
/// menu-sf-symbols-2026-10-27]; `sf_symbols` for the rendering path).
pub const MENU_SYMBOL_PT: f32 = TEXT_SMALL;
pub const MENU_SYMBOL_SLOT: f32 = 20.0;

/// Menu row (the "…" menu): `symbol` is an SF Symbol name registered in
/// `sf_symbols::FALLBACKS`, then the label.
pub fn menu_row(
    id: impl Into<ElementId>,
    symbol: &'static str,
    label: impl Into<SharedString>,
    danger: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let color = if danger { theme.danger } else { theme.text };
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.0))
        .h(px(MENU_ROW_HEIGHT))
        .px(px(12.0))
        .rounded(px(MENU_ROW_RADIUS))
        .text_size(px(TEXT_SMALL))
        .text_color(color)
        .cursor_pointer()
        .map(|el| highlight::row(el, false, highlight::Shape::Own, theme))
        .child(sf_symbols::symbol(
            symbol,
            MENU_SYMBOL_PT,
            sf_symbols::Weight::Regular,
            MENU_SYMBOL_SLOT,
            color,
        ))
        .child(div().flex_1().min_w_0().truncate().child(label.into()))
}

/// Hitbox behaviour for every floating popover / menu / sheet: block clicks
/// **and** wheel/trackpad scroll into the view behind it. GPUI's
/// [`InteractiveElement::occlude`] (`HitboxBehavior::BlockMouse`) — not
/// `block_mouse_except_scroll`, which would still let the transcript scroll.
fn floating_layer_hitbox_behavior() -> gpui::HitboxBehavior {
    gpui::HitboxBehavior::BlockMouse
}

/// Wrap a floating popover so clicks and wheel/trackpad events inside it
/// never reach the view behind (e.g. the chat transcript) or the root's
/// "click elsewhere closes popovers" handler.
pub fn popover(child: impl IntoElement) -> Div {
    debug_assert_eq!(
        floating_layer_hitbox_behavior(),
        gpui::HitboxBehavior::BlockMouse
    );
    div()
        // Occlude: GPUI otherwise delivers scroll to every hitbox under the
        // pointer, so a wheel over this menu would also move the transcript.
        .occlude()
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .child(child)
}

/// Shared vertical scroll area for inside a popover, menu, or sheet. Prefer
/// this over a bare `overflow_y_scroll` so floating layers share one
/// container. Pair with [`popover`] / [`frosted_menu`] / [`sheet`] (they
/// occlude so the wheel never chains into the view behind).
pub fn contained_y_scroll(id: impl Into<ElementId>) -> Stateful<Div> {
    div().id(id).flex().flex_col().overflow_y_scroll()
}

pub fn menu_divider(theme: &Theme) -> Div {
    let line = 1.0;
    div()
        .h(px(line))
        .my(px((MENU_DIVIDER_HEIGHT - line) / 2.0))
        .bg(theme.border)
}

/// A floating surface's glass plate on Glass ([feedback
/// mac-glass-contrast-2026-10-26]): the card tint plus the header's 1 pt rim
/// and top highlight, over the lift shadow it already had. On Matte the
/// card keeps its solid fill and plain border.
pub fn plate<E: Styled>(el: E, theme: &Theme) -> E {
    let el = el
        .bg(theme.card())
        .border_1()
        .border_color(theme.border_strong);
    if !theme.is_frost() {
        return el.shadow_lg();
    }
    el.shadow(vec![
        BoxShadow::new(px(0.0), px(0.0), theme.plate_rim())
            .spread_radius(px(1.0))
            .inset(),
        BoxShadow::new(px(0.0), px(1.0), theme.plate_highlight()).inset(),
        BoxShadow::new(px(0.0), px(8.0), gpui::hsla(0.0, 0.0, 0.0, 0.28)).blur_radius(px(24.0)),
    ])
}

/// A dropdown card under a header control: frosted, rounded, shadowed.
pub fn menu_card(width: f32, theme: &Theme) -> Div {
    let card = div()
        .w(px(width))
        .flex()
        .flex_col()
        .gap(px(MENU_GAP))
        .p(px(MENU_CARD_PADDING))
        .rounded(px(PANEL_RADIUS));
    plate(card, theme)
}

/// Frosted dropdown used by the tab "+" menu and sidebar row menus. Occludes
/// so a wheel over the menu never scrolls the list or transcript behind it,
/// even when the caller did not wrap with [`popover`].
pub fn frosted_menu(width: f32, theme: &Theme, body: Div) -> impl IntoElement {
    div()
        .occlude()
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .child(frosted(
            PANEL_RADIUS,
            MENU_BLUR,
            menu_card(width, theme).child(body),
        ))
}

/// Centered sheet with a dimmed backdrop: the iPhone's `.sheet`. `header`
/// is the title row (left control, centred title, right control).
///
/// A mouse-down inside the card stays inside it: the root's "click elsewhere"
/// handler never sees it (so a field in the sheet keeps the keyboard), and
/// neither does the returned backdrop — a sheet that closes on a click
/// outside puts its handler on the backdrop.
pub fn sheet(width: f32, theme: &Theme, header: Div, body: impl IntoElement) -> Div {
    let card = div()
        .w(px(width))
        .max_w_full()
        .max_h_full()
        .flex()
        .flex_col()
        .rounded(px(PANEL_RADIUS + 6.0));
    let card = plate(card, theme)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(header)
        .child(body);
    // The backdrop is modal: nothing under it is hovered, clicked, or
    // scrolled (GPUI delivers mouse/wheel events to every hitbox under the
    // pointer otherwise — same containment as [`popover`]).
    div()
        .absolute()
        .inset_0()
        .occlude()
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
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
        .border_1()
        .border_color(theme.field_border)
        .flex()
        .items_center()
        .text_size(px(TEXT_SMALL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text)
        .cursor_pointer()
        .map(|el| highlight::washed(el, theme.glass_fill(), theme))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Front-to-back layers under the pointer. A layer with `contains_scroll`
    /// (popover / menu / sheet via occlude) consumes the wheel; layers behind
    /// it are not scroll targets — the same rule GPUI's hit-test applies for
    /// `BlockMouse`.
    fn scroll_hit_layers<'a>(
        front_to_back: impl IntoIterator<Item = (bool /* contains_scroll */, &'a str)>,
    ) -> Vec<&'a str> {
        let mut hit = Vec::new();
        for (contains_scroll, name) in front_to_back {
            hit.push(name);
            if contains_scroll {
                break;
            }
        }
        hit
    }

    #[test]
    fn floating_layer_blocks_scroll_not_only_clicks() {
        // `BlockMouseExceptScroll` would still let the transcript move under
        // the + tab menu; popovers must use full `BlockMouse` (occlude).
        assert_eq!(
            floating_layer_hitbox_behavior(),
            gpui::HitboxBehavior::BlockMouse
        );
        assert_ne!(
            floating_layer_hitbox_behavior(),
            gpui::HitboxBehavior::BlockMouseExceptScroll
        );
    }

    #[test]
    fn wheel_over_popover_does_not_move_transcript() {
        // Regression for popover-scroll-bleed-2026-10-20: a wheel over the
        // agent chat's + tab menu (New chat + Previous chats) must not scroll
        // the transcript behind it.
        let hit = scroll_hit_layers([(true, "popover"), (false, "transcript")]);
        assert_eq!(hit, vec!["popover"]);
        assert!(!hit.contains(&"transcript"));
    }

    #[test]
    fn wheel_over_menu_does_not_chain_into_underlying_scroll() {
        let hit = scroll_hit_layers([(true, "frosted_menu"), (false, "sidebar_list")]);
        assert_eq!(hit, vec!["frosted_menu"]);
    }
}

#[cfg(test)]
mod drag_tests {
    use gpui::{point, px, Modifiers, MouseButton, MouseDownEvent, Pixels, Point, TestAppContext};

    use super::*;

    /// The app's top chrome at its real metrics: sidebar strip (296 × 44),
    /// chat header (44) with a centred non-drag pill and, when the sidebar is
    /// folded, a 28 pt toggle after the 72 pt traffic-light inset; right pane
    /// bar (340 × 44) with a 30 pt close circle. Each bar is a drag region.
    struct TopChrome {
        collapsed: bool,
    }

    const SIDEBAR: f32 = 296.0;
    const PANE: f32 = 340.0;
    const BAR: f32 = 44.0;
    const WIDTH: f32 = 1180.0;

    impl gpui::Render for TopChrome {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let stop = |el: Stateful<Div>| {
                el.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            };
            let mut row = div().flex().w(px(WIDTH)).h(px(BAR));
            if !self.collapsed {
                row = row.child(
                    div()
                        .id("strip")
                        .w(px(SIDEBAR))
                        .h(px(BAR))
                        .flex()
                        .items_center()
                        .justify_end()
                        .px(px(12.0))
                        .child(stop(div().id("plus").size(px(28.0))))
                        .window_drag(),
                );
            }
            let mut header = div()
                .id("header")
                .flex_1()
                .h(px(BAR))
                .flex()
                .items_center()
                .justify_between()
                .pl(px(if self.collapsed { 72.0 + 12.0 } else { 12.0 }))
                .pr(px(10.0))
                .window_drag();
            header = header
                .child(div().w(px(40.0)).flex().when(self.collapsed, |el| {
                    el.child(stop(div().id("toggle").size(px(28.0))))
                }))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .justify_center()
                        .child(stop(div().id("pill").w(px(120.0)).h(px(34.0)))),
                )
                .child(div().w(px(40.0)));
            row = row.child(header).child(
                div()
                    .id("pane-bar")
                    .w(px(PANE))
                    .h(px(BAR))
                    .flex()
                    .items_center()
                    .justify_end()
                    .px(px(10.0))
                    .child(stop(div().id("close").size(px(30.0))))
                    .window_drag(),
            );
            let _ = cx;
            div().size_full().child(row)
        }
    }

    fn press_and_move(cx: &mut TestAppContext, collapsed: bool, at: Point<Pixels>) -> u32 {
        cx.update(|cx| {
            cx.set_global(WindowDragPress {
                record_only: true,
                ..Default::default()
            })
        });
        let (view, vcx) = cx.add_window_view(|_, _| TopChrome { collapsed });
        vcx.run_until_parked();
        vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        // An animating avatar re-renders every frame between the press and
        // the first move: the drag must survive a fresh frame.
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        vcx.simulate_mouse_move(
            point(at.x + px(12.0), at.y + px(3.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| cx.default_global::<WindowDragPress>().moves)
    }

    #[gpui::test]
    fn empty_top_strips_drag_the_window_in_all_three_panes(cx: &mut TestAppContext) {
        // Sidebar strip, left of the + button.
        assert_eq!(
            press_and_move(cx, false, point(px(60.0), px(22.0))),
            1,
            "sidebar strip"
        );
        // Chat header, between the sidebar edge and the pill.
        assert_eq!(
            press_and_move(cx, false, point(px(SIDEBAR + 80.0), px(22.0))),
            1,
            "header"
        );
        // Right pane bar, left of the close circle.
        assert_eq!(
            press_and_move(cx, false, point(px(WIDTH - PANE + 60.0), px(22.0))),
            1,
            "pane bar"
        );
    }

    #[gpui::test]
    fn the_header_drags_with_the_sidebar_folded_too(cx: &mut TestAppContext) {
        // Right of the toggle, left of the pill.
        assert_eq!(
            press_and_move(cx, true, point(px(160.0), px(22.0))),
            1,
            "folded header"
        );
        assert_eq!(
            press_and_move(cx, true, point(px(WIDTH - PANE + 60.0), px(22.0))),
            1,
            "pane bar, folded"
        );
    }

    #[gpui::test]
    fn controls_do_not_drag(cx: &mut TestAppContext) {
        // The pill sits centred in the header: header spans SIDEBAR..WIDTH-PANE.
        let header_centre = (SIDEBAR + (WIDTH - PANE)) / 2.0;
        assert_eq!(
            press_and_move(cx, false, point(px(header_centre), px(22.0))),
            0,
            "pill"
        );
        assert_eq!(
            press_and_move(cx, false, point(px(SIDEBAR - 12.0 - 14.0), px(22.0))),
            0,
            "+ button"
        );
        assert_eq!(
            press_and_move(cx, true, point(px(72.0 + 12.0 + 14.0), px(22.0))),
            0,
            "folded toggle"
        );
    }

    #[gpui::test]
    fn a_short_jitter_is_a_click_not_a_move(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(WindowDragPress {
                record_only: true,
                ..Default::default()
            })
        });
        let (_view, vcx) = cx.add_window_view(|_, _| TopChrome { collapsed: false });
        vcx.run_until_parked();
        let at = point(px(SIDEBAR + 80.0), px(22.0));
        vcx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        vcx.simulate_mouse_move(
            point(at.x + px(1.0), at.y + px(1.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        vcx.run_until_parked();
        assert_eq!(
            vcx.update(|_, cx| cx.default_global::<WindowDragPress>().moves),
            0
        );
    }
}
