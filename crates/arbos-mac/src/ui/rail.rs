//! The collapsed sidebar: Grok Bot's icon rail (Jacob 2026-10-26,
//! [feedback mac-sidebar-rail-2026-10-26]).
//!
//! Pressing the sidebar toggle no longer closes the chat list; it collapses
//! it to an 84 pt rail — one centred column: the toggle (in full screen; in a
//! window it stays beside the traffic lights in the chat header, where today's
//! lone toggle is), then **+** and **Search** stacked 8 apart on today's
//! control line, then every chat's picture (agents, people, groups, servers)
//! at 36 pt on Grok's 58 pt pitch, and my own picture alone at the bottom on
//! the footer's line. The rail follows the All · Agents · People · Servers
//! segment, which persists with the collapsed state; a server's picture opens
//! its newest channel; pictures carry the rows' badges (typing green wins over
//! the unread blue on the one corner spot) and show the name on hover.
//!
//! Measured on Jacob's `jacob-ref-grok-collapsed-rail-2.png` (3440 × 2158 px =
//! a 1720 × 1079 pt window at 2 px/pt; project store
//! `media/feedback/mac-sidebar-rail/`): hairline at x 167 px → **83.5**;
//! selected tile 107 × 108 px at (26, 94) → **54** square, radius ≈ 12;
//! avatars 72 px → **36**; pitch 116 px → **58**; the user's picture centred
//! 31 from the bottom (ours: the footer's 30); blue unread dot 16 px → 8 at
//! the picture's bottom-right 45° point. `docs/mac-design-rules.md` §
//! "Collapsed sidebar = icon rail".
//!
//! The collapse is a morph, not a clip ([`render_morph`] + [`render_overlay`]):
//! the strip's discs glide into the rail column, the rows' pictures shrink
//! into their rail slots while the text fades, the chat column re-centres —
//! on the app's slide curves (`ui::slide`: 170 ms ease-out quad closing,
//! 200 ms ease-out cubic opening). The expanded sidebar records where its
//! discs, pictures and my footer picture rest ([`MorphSnapshot`]) every frame
//! it is painted, and the overlay interpolates from there.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, Bounds, Context, Div, ElementId, Focusable as _, Pixels, SharedString,
};

use crate::model::Id;
use crate::ui::composer::Composer;
use crate::ui::frost::edge_faded;
use crate::ui::glass::{self, WindowDrag};
use crate::ui::shell::Shell;
use crate::ui::sidebar::{
    self, is_control_click, ListEntry, StripControl, FOOTER_AVATAR, FOOTER_BOTTOM,
    FOOTER_DOT_OVERHANG, FOOTER_DOT_RING, FOOTER_HEIGHT, LIST_FADE_BAND, STRIP_GAP, STRIP_INSET,
};
use crate::ui::theme::{Theme, CONTROL, TEXT_SMALL};

/// The rail's geometry (pt). Grok's numbers where Grok has them; the button
/// family's where the rail holds our discs.
pub(crate) mod metrics {
    use crate::ui::theme::{CONTROL, CONTROL_LINE};

    /// Rail width including its 1 pt right hairline (Grok: 83.5).
    pub const WIDTH: f32 = 84.0;
    /// The column's centre line.
    pub const CENTRE_X: f32 = WIDTH / 2.0;
    /// Every picture: `AVATAR_ROW` = the button family (Grok: 72 px).
    pub const AVATAR: f32 = CONTROL;
    /// The selected / hovered tile behind a picture (Grok: 107 × 108 px).
    pub const TILE: f32 = 54.0;
    pub const TILE_RADIUS: f32 = 12.0;
    /// Picture pitch (Grok: 116 px).
    pub const PITCH: f32 = 58.0;
    /// The discs' x: centred in the rail.
    pub const DISC_X: f32 = CENTRE_X - CONTROL / 2.0;
    /// The toggle keeps today's line (disc top 12, centre `CONTROL_LINE`);
    /// + and Search stack under it, `STRIP_GAP` apart.
    pub const TOGGLE_TOP: f32 = CONTROL_LINE - CONTROL / 2.0;
    pub const PLUS_TOP: f32 = TOGGLE_TOP + CONTROL + super::STRIP_GAP;
    pub const SEARCH_TOP: f32 = PLUS_TOP + CONTROL + super::STRIP_GAP;
    /// Pictures start 16 under the last disc (Grok's first card sits 16
    /// under its discs): first tile top 152 → first picture top 161.
    pub const FIRST_TILE_TOP: f32 = SEARCH_TOP + CONTROL + 16.0;
    /// The scroll box starts where the first 58 pt slot does (the 54 tile is
    /// centred in its slot, 2 above and below).
    pub const LIST_TOP: f32 = FIRST_TILE_TOP - (PITCH - TILE) / 2.0;
    /// The hover tip: 28 tall, 8 right of the rail's edge.
    pub const TIP_HEIGHT: f32 = 28.0;
    pub const TIP_GAP: f32 = 8.0;
    /// Badge discs at the picture's bottom-right corner: the unread dot in a
    /// ring of the rail's colour, the typing dot in the footer dot's ring.
    pub const UNREAD_DOT: f32 = 8.0;
    pub const UNREAD_RING: f32 = 12.0;
    pub const TYPING_DOT: f32 = 9.0;
}

/// Where the sidebar toggle lives while the sidebar is collapsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToggleSpot {
    /// A plain window: beside the traffic lights in the chat header, where
    /// today's lone toggle is (the header's 12 pt inset after the rail).
    Header,
    /// Native full screen (the lights are hidden): the rail's top-left slot.
    RailTop,
}

pub(crate) fn toggle_spot(fullscreen: bool) -> ToggleSpot {
    if fullscreen {
        ToggleSpot::RailTop
    } else {
        ToggleSpot::Header
    }
}

/// Whether the chat header draws the sidebar toggle this frame.
///
/// * The narrow band hides the sidebar outright: the header shows the toggle
///   once the sidebar has left the lights' band (as before).
/// * Collapsed to the rail: only at rest, and only in a plain window
///   ([`ToggleSpot::Header`]) — full screen keeps it in the rail, and the
///   morph's overlay carries it in between, so it is never drawn twice.
pub(crate) fn header_toggle_shown(
    sidebar_hidden: bool,
    sidebar_visible: f32,
    rail: bool,
    sliding: bool,
    fullscreen: bool,
) -> bool {
    if sidebar_hidden {
        return sidebar_visible <= crate::ui::shell::TRAFFIC_LIGHTS_INSET;
    }
    rail && !sliding && toggle_spot(fullscreen) == ToggleSpot::Header
}

/// The one badge spot at a rail picture's bottom-right corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RailBadge {
    /// Someone is typing in the chat: the green presence dot (pulsing).
    Typing,
    /// Unread messages: Grok's blue dot. Never on the selected row.
    Unread,
}

/// Typing wins over unread — the rows' own rule (`!typing && unread` for the
/// accent dot); the selected chat shows no unread dot.
pub(crate) fn badge(typing: bool, unread: bool, selected: bool) -> Option<RailBadge> {
    if typing {
        Some(RailBadge::Typing)
    } else if unread && !selected {
        Some(RailBadge::Unread)
    } else {
        None
    }
}

/// A strip disc's resting top in the rail.
pub(crate) fn disc_top(control: StripControl) -> f32 {
    match control {
        StripControl::Collapse => metrics::TOGGLE_TOP,
        StripControl::New => metrics::PLUS_TOP,
        StripControl::Search => metrics::SEARCH_TOP,
    }
}

/// The `index`-th picture's tile top in the rail (before scrolling).
pub(crate) fn tile_top(index: usize) -> f32 {
    metrics::FIRST_TILE_TOP + index as f32 * metrics::PITCH
}

/// How many pictures fit between the discs and my footer picture without
/// scrolling, in a window `height` tall (the list scrolls past it).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn capacity(height: f32) -> usize {
    let room = height - FOOTER_HEIGHT - metrics::LIST_TOP;
    (room / metrics::PITCH).floor().max(0.0) as usize
}

/// Where the expanded sidebar's morphing pieces rested when last painted
/// (window coordinates), keyed so the overlay can pair them with their rail
/// slots. Recorded by `sidebar::render_with` through `on_children_prepainted`.
#[derive(Debug, Default)]
pub(crate) struct MorphSnapshot {
    pub(crate) discs: HashMap<StripControl, Bounds<Pixels>>,
    pub(crate) rows: HashMap<Id, Bounds<Pixels>>,
    pub(crate) cards: HashMap<Id, Bounds<Pixels>>,
    pub(crate) footer: Option<Bounds<Pixels>>,
}

pub(crate) type Morph = Rc<RefCell<MorphSnapshot>>;

/// Linear interpolation of two boxes.
pub(crate) fn lerp_bounds(from: Bounds<Pixels>, to: Bounds<Pixels>, t: f32) -> Bounds<Pixels> {
    let l = |a: Pixels, b: Pixels| a + (b - a) * t;
    Bounds {
        origin: gpui::point(l(from.origin.x, to.origin.x), l(from.origin.y, to.origin.y)),
        size: gpui::size(
            l(from.size.width, to.size.width),
            l(from.size.height, to.size.height),
        ),
    }
}

fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds {
        origin: gpui::point(px(x), px(y)),
        size: gpui::size(px(w), px(h)),
    }
}

/// The rail's progress from the sidebar's visible width this frame: 0 at the
/// expanded width, 1 at the rail.
pub(crate) fn progress(visible: f32, full: f32) -> f32 {
    let span = full - metrics::WIDTH;
    if span <= f32::EPSILON {
        return 1.0;
    }
    ((full - visible) / span).clamp(0.0, 1.0)
}

/// The text's fade during the morph: gone by half-way, back from half-way.
pub(crate) fn text_alpha(t: f32) -> f32 {
    (1.0 - 2.0 * t).clamp(0.0, 1.0)
}

/// A rail disc's resting frame in window coordinates.
fn disc_rest(control: StripControl, spot: ToggleSpot) -> Bounds<Pixels> {
    match (control, spot) {
        (StripControl::Collapse, ToggleSpot::Header) => bounds(
            metrics::WIDTH + STRIP_INSET,
            metrics::TOGGLE_TOP,
            CONTROL,
            CONTROL,
        ),
        _ => bounds(metrics::DISC_X, disc_top(control), CONTROL, CONTROL),
    }
}

fn badge_element(badge: RailBadge, ring: gpui::Hsla, theme: &Theme, phase: f32) -> Div {
    let (ring_size, dot, color) = match badge {
        RailBadge::Typing => (
            FOOTER_DOT_RING,
            metrics::TYPING_DOT,
            Theme::with_alpha(theme.online, 0.35 + 0.65 * phase),
        ),
        RailBadge::Unread => (metrics::UNREAD_RING, metrics::UNREAD_DOT, theme.accent),
    };
    div()
        .absolute()
        .right(px(-FOOTER_DOT_OVERHANG))
        .bottom(px(-FOOTER_DOT_OVERHANG))
        .size(px(ring_size))
        .rounded_full()
        .bg(ring)
        .flex()
        .items_center()
        .justify_center()
        .child(div().size(px(dot)).rounded_full().bg(color))
}

/// One of the rail's three discs with the strip's own handlers.
fn rail_disc(control: StripControl, theme: &Theme, cx: &mut Context<Shell>) -> Div {
    let (glyph, size) = control.glyph();
    let id: &'static str = match control {
        StripControl::Collapse => "rail-collapse",
        StripControl::New => "rail-new",
        StripControl::Search => "rail-search",
    };
    let disc = glass::control_disc(id, glyph, size, theme)
        .debug_selector(move || id.into())
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation());
    let disc = match control {
        StripControl::Collapse => disc.on_click(cx.listener(|this, _, window, cx| {
            this.menu_open = false;
            let width: f32 = window.viewport_size().width.into();
            this.state.update(cx, |s, cx| {
                let narrow = s.window_is_narrow(width);
                s.toggle_rail(narrow, cx)
            });
        })),
        StripControl::New => disc.on_click(cx.listener(|this, _, window, cx| {
            this.menu_open = false;
            this.people_input.update(cx, |i, cx| {
                i.clear(cx);
                i.set_placeholder("Search people");
            });
            this.state.update(cx, |s, cx| s.open_new_chat(cx));
            let handle = this.people_input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        })),
        StripControl::Search => {
            disc.on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx)))
        }
    };
    div()
        .absolute()
        .left(px(metrics::DISC_X))
        .top(px(disc_top(control)))
        .child(disc)
}

/// The rail at rest.
pub(crate) fn render(shell: &Shell, cx: &mut Context<Shell>) -> gpui::Stateful<Div> {
    let theme = Theme::of(cx).clone();
    let entries = sidebar::list_entries(shell, cx);
    let spot = toggle_spot(shell.fullscreen);
    let machine = shell.state.read(cx).machine.shown_state();
    let phase = crate::ui::motion::PulseClock::phase(cx, 900);
    if entries.iter().any(ListEntry::typing) {
        crate::ui::motion::PulseClock::request(cx);
    }

    let mut column = div()
        .id("sidebar-rail")
        .relative()
        .h_full()
        .w(px(metrics::WIDTH))
        .flex_shrink_0()
        .bg(theme.chrome())
        .border_r_1()
        .border_color(theme.border)
        .debug_selector(|| "sidebar-rail".into());
    // The top band stays macOS's drag region, as the strip is.
    column = column.child(
        div()
            .absolute()
            .top_0()
            .left_0()
            .w(px(metrics::WIDTH))
            .h(px(metrics::LIST_TOP))
            .window_drag(),
    );
    if spot == ToggleSpot::RailTop {
        column = column.child(rail_disc(StripControl::Collapse, &theme, cx));
    }
    column = column
        .child(rail_disc(StripControl::New, &theme, cx))
        .child(rail_disc(StripControl::Search, &theme, cx));

    // ----- the pictures: one 58 pt slot each, a 54 tile centred in it -------------
    let hovered = shell.rail_hover.clone();
    let mut list = div()
        .id("rail-list")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .items_center()
        .overflow_y_scroll()
        .track_scroll(&shell.rail_scroll)
        .debug_selector(|| "rail-list".into());
    for (index, entry) in entries.iter().enumerate() {
        let id = entry.id().clone();
        let selected = entry.selected();
        let is_hover = hovered.as_ref() == Some(&id);
        let badge = badge(entry.typing(), entry.unread(), selected);
        let tile_fill = if selected {
            theme.surface_2
        } else if is_hover {
            Theme::with_alpha(theme.surface_2, 0.55)
        } else {
            gpui::transparent_black()
        };
        let ring = if selected || is_hover {
            theme.surface_2
        } else {
            theme.sidebar
        };
        let picture = entry.picture(shell, metrics::AVATAR, &theme, cx);
        let is_server = matches!(entry, ListEntry::Server(_));
        let click_id = id.clone();
        let hover_id = id.clone();
        let menu_id = id.clone();
        let ctrl_menu_id = id.clone();
        let tile = div()
            .id(ElementId::Name(SharedString::from(format!("rail-{id}"))))
            .size(px(metrics::TILE))
            .rounded(px(metrics::TILE_RADIUS))
            .bg(tile_fill)
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(index == 0, |el| {
                el.debug_selector(|| "rail-first-tile".into())
            })
            .child(
                div()
                    .relative()
                    .size(px(metrics::AVATAR))
                    .flex_shrink_0()
                    .when(index == 0, |el| {
                        el.debug_selector(|| "rail-first-picture".into())
                    })
                    .child(picture)
                    .when_some(badge, |el, b| {
                        el.child(badge_element(b, ring, &theme, phase))
                    }),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let now = if *hovered {
                    Some(hover_id.clone())
                } else if this.rail_hover.as_ref() == Some(&hover_id) {
                    None
                } else {
                    return;
                };
                if this.rail_hover != now {
                    this.rail_hover = now;
                    cx.notify();
                }
            }))
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    if is_control_click(event) {
                        return;
                    }
                    this.menu_open = false;
                    this.row_menu = None;
                    this.chat_menu = None;
                    this.state.update(cx, |s, cx| {
                        if is_server {
                            s.enter_server_latest(click_id.clone(), cx);
                        } else {
                            s.select_project(click_id.clone(), cx);
                        }
                    });
                    Composer::focus(&this.composer, window, cx);
                }),
            )
            // Right-click / control-click: the row's menu, as in the list.
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.menu_open = false;
                    this.chat_menu = None;
                    this.row_menu = Some((menu_id.clone(), event.position));
                    cx.notify();
                }),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    if !event.modifiers.control {
                        return;
                    }
                    cx.stop_propagation();
                    this.menu_open = false;
                    this.chat_menu = None;
                    this.row_menu = Some((ctrl_menu_id.clone(), event.position));
                    cx.notify();
                }),
            );
        list = list.child(
            div()
                .h(px(metrics::PITCH))
                .w(px(metrics::WIDTH - 1.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(tile),
        );
    }
    column = column.child(
        div()
            .absolute()
            .left_0()
            .top(px(metrics::LIST_TOP - LIST_FADE_BAND))
            .bottom(px(FOOTER_HEIGHT))
            .w(px(metrics::WIDTH - 1.0))
            .flex()
            .flex_col()
            .child(edge_faded(
                LIST_FADE_BAND,
                true,
                true,
                list.pt(px(LIST_FADE_BAND)),
            )),
    );

    // ----- my picture alone at the bottom, on the footer's line -----------------------
    let account = div()
        .id("rail-account")
        .absolute()
        .left(px(metrics::DISC_X))
        .bottom(px(FOOTER_BOTTOM))
        .cursor_pointer()
        .debug_selector(|| "rail-account".into())
        .child(sidebar::account_avatar(shell, machine, &theme, cx))
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.menu_open = false;
                let open = this.state.read(cx).profile == Some(crate::app::ProfilePage::Me);
                if open {
                    this.close_profile(window, cx);
                } else {
                    this.state.update(cx, |s, cx| {
                        s.open_profile(crate::app::ProfilePage::Me, cx);
                        s.set_profile_tab(1, cx);
                    });
                }
                cx.notify();
            }),
        );
    column.child(account)
}

/// The hovered picture's name, drawn at the root so the rail's clip never cuts
/// it: a small card 8 right of the rail, centred on the tile.
pub(crate) fn render_tip(shell: &Shell, cx: &Context<Shell>) -> Option<Div> {
    let hovered = shell.rail_hover.as_ref()?;
    let entries = sidebar::list_entries(shell, cx);
    let (index, entry) = entries
        .iter()
        .enumerate()
        .find(|(_, e)| e.id() == hovered)?;
    let theme = Theme::of(cx);
    let scroll_y: f32 = shell.rail_scroll.offset().y.into();
    let tile_centre =
        metrics::LIST_TOP + metrics::PITCH / 2.0 + index as f32 * metrics::PITCH + scroll_y;
    Some(
        div()
            .absolute()
            .left(px(metrics::WIDTH + metrics::TIP_GAP))
            .top(px(tile_centre - metrics::TIP_HEIGHT / 2.0))
            .h(px(metrics::TIP_HEIGHT))
            .px(px(10.0))
            .rounded(px(8.0))
            .bg(theme.pane_card)
            .border_1()
            .border_color(theme.border_strong)
            .shadow_md()
            .flex()
            .items_center()
            .text_size(px(TEXT_SMALL))
            .text_color(theme.text)
            .whitespace_nowrap()
            .debug_selector(|| "rail-tip".into())
            .child(entry.name()),
    )
}

/// The sidebar column while the slide runs: `width` wide this frame, holding
/// the expanded layout at its full width with the morphing pieces unpainted
/// and the rest fading ([`text_alpha`]); the pieces themselves travel in
/// [`render_overlay`].
pub(crate) fn render_morph(shell: &Shell, t: f32, width: f32, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();
    let full = shell.state.read(cx).sidebar_width;
    div()
        .h_full()
        .w(px(width.max(0.0)))
        .flex_shrink_0()
        .overflow_hidden()
        .bg(theme.chrome())
        .border_r_1()
        .border_color(theme.border)
        .child(
            div()
                .h_full()
                .w(px(full))
                .flex_shrink_0()
                .opacity(text_alpha(t))
                .child(sidebar::render_with(shell, true, cx)),
        )
}

/// The moving pieces of the collapse / open slide, over the whole window:
/// the three discs between the strip and the rail column (the toggle heads
/// for the chat header in a plain window), every row's picture between its
/// row and its rail slot (the selected row's card shrinking into its tile),
/// and my footer picture sliding to the rail's centre.
pub(crate) fn render_overlay(
    shell: &Shell,
    t: f32,
    window_height: f32,
    cx: &mut Context<Shell>,
) -> Div {
    let theme = Theme::of(cx).clone();
    let spot = toggle_spot(shell.fullscreen);
    let snap = shell.morph.borrow();
    let entries = sidebar::list_entries(shell, cx);
    let machine = shell.state.read(cx).machine.shown_state();
    let scroll_y: f32 = shell.rail_scroll.offset().y.into();
    let mut layer = div().absolute().inset_0();

    // Rows, clipped to the band both lists share (above the footer, under the strip).
    let clip_top = crate::ui::theme::HEADER_HEIGHT
        + sidebar::FILTER_TOP
        + sidebar::FILTER_HEIGHT
        + sidebar::FILTER_BOTTOM
        - LIST_FADE_BAND;
    let mut rows = div()
        .absolute()
        .left_0()
        .top(px(clip_top))
        .w(px(shell.state.read(cx).sidebar_width.max(metrics::WIDTH)))
        .h(px((window_height - clip_top - FOOTER_HEIGHT).max(0.0)))
        .overflow_hidden();
    for (index, entry) in entries.iter().enumerate() {
        let Some(from) = snap.rows.get(entry.id()).copied() else {
            continue;
        };
        let to_tile = bounds(
            metrics::CENTRE_X - metrics::TILE / 2.0,
            tile_top(index) + scroll_y,
            metrics::TILE,
            metrics::TILE,
        );
        let to = bounds(
            metrics::CENTRE_X - metrics::AVATAR / 2.0,
            tile_top(index) + (metrics::TILE - metrics::AVATAR) / 2.0 + scroll_y,
            metrics::AVATAR,
            metrics::AVATAR,
        );
        if entry.selected() {
            if let Some(card) = snap.cards.get(entry.id()).copied() {
                let b = lerp_bounds(card, to_tile, t);
                let radius = crate::ui::theme::PANEL_RADIUS - 4.0
                    + (metrics::TILE_RADIUS - (crate::ui::theme::PANEL_RADIUS - 4.0)) * t;
                rows = rows.child(
                    div()
                        .absolute()
                        .left(b.origin.x)
                        .top(b.origin.y - px(clip_top))
                        .w(b.size.width)
                        .h(b.size.height)
                        .rounded(px(radius))
                        .bg(theme.surface_2),
                );
            }
        }
        let b = lerp_bounds(from, to, t);
        rows = rows.child(
            div()
                .absolute()
                .left(b.origin.x)
                .top(b.origin.y - px(clip_top))
                .child(entry.picture(shell, metrics::AVATAR, &theme, cx)),
        );
    }
    layer = layer.child(rows);

    for control in [
        StripControl::Search,
        StripControl::New,
        StripControl::Collapse,
    ] {
        let Some(from) = snap.discs.get(&control).copied() else {
            continue;
        };
        let b = lerp_bounds(from, disc_rest(control, spot), t);
        let (glyph, size) = control.glyph();
        let id: &'static str = match control {
            StripControl::Collapse => "morph-collapse",
            StripControl::New => "morph-new",
            StripControl::Search => "morph-search",
        };
        layer = layer.child(
            div()
                .absolute()
                .left(b.origin.x)
                .top(b.origin.y)
                .child(glass::control_disc(id, glyph, size, &theme)),
        );
    }
    if let Some(from) = snap.footer {
        let to = bounds(
            metrics::DISC_X,
            window_height - FOOTER_BOTTOM - FOOTER_AVATAR,
            FOOTER_AVATAR,
            FOOTER_AVATAR,
        );
        let b = lerp_bounds(from, to, t);
        layer = layer.child(
            div()
                .absolute()
                .left(b.origin.x)
                .top(b.origin.y)
                .child(sidebar::account_avatar(shell, machine, &theme, cx)),
        );
    }
    drop(snap);
    layer
}

#[cfg(test)]
mod tests {
    use super::metrics::*;
    use super::*;
    use crate::ui::sidebar::{FOOTER_BOTTOM, FOOTER_HEIGHT, STRIP_BUTTON};
    use crate::ui::theme::{AVATAR_ROW, CONTROL_LINE, HEADER_HEIGHT};

    /// Grok Bot's rail (`jacob-ref-grok-collapsed-rail-2.png`, 2 px/pt) and
    /// the approved mock (`mock-mac-sidebar-rail-collapsed-dark.png`).
    mod grok {
        pub const RAIL: f32 = 83.5;
        pub const AVATAR: f32 = 36.0;
        pub const TILE: f32 = 54.0;
        pub const PITCH: f32 = 58.0;
        pub const USER_CENTRE_FROM_BOTTOM: f32 = 31.0;
        pub const UNREAD_DOT: f32 = 8.0;
    }

    /// The measurement table: rail 84 (Grok 83.5 + the hairline), pictures
    /// 36 on a 58 pitch in 54 tiles, the toggle on today's line, + and Search
    /// 8 under each other, pictures 16 under Search, my picture on the
    /// footer's line.
    #[test]
    fn rail_metrics_match_the_measurement_table() {
        assert!((WIDTH - grok::RAIL).abs() <= 0.5, "rail width {WIDTH}");
        assert_eq!(AVATAR, grok::AVATAR);
        assert_eq!(AVATAR, AVATAR_ROW, "the rows' picture size");
        assert_eq!(AVATAR, STRIP_BUTTON, "the button family");
        assert_eq!(TILE, grok::TILE);
        assert_eq!(TILE_RADIUS, 12.0);
        assert_eq!(PITCH, grok::PITCH);
        assert_eq!(CENTRE_X, 42.0);
        assert_eq!(DISC_X, 24.0);
        assert_eq!(TOGGLE_TOP, 12.0, "today's disc top");
        assert_eq!(TOGGLE_TOP + CONTROL / 2.0, CONTROL_LINE);
        assert_eq!(PLUS_TOP, 56.0);
        assert_eq!(SEARCH_TOP, 100.0);
        assert_eq!(PLUS_TOP - (TOGGLE_TOP + CONTROL), 8.0, "discs 8 apart");
        assert_eq!(FIRST_TILE_TOP, 152.0);
        assert_eq!(FIRST_TILE_TOP - (SEARCH_TOP + CONTROL), 16.0);
        assert_eq!(LIST_TOP, 150.0);
        let first_picture_centre = FIRST_TILE_TOP + TILE / 2.0;
        assert_eq!(first_picture_centre, 179.0);
        assert_eq!(tile_top(1) - tile_top(0), PITCH);
        // My picture: 36 tall, 12 above the bottom → centre 30 (Grok 31).
        let centre_from_bottom = FOOTER_BOTTOM + CONTROL / 2.0;
        assert_eq!(centre_from_bottom, CONTROL_LINE);
        assert!((centre_from_bottom - grok::USER_CENTRE_FROM_BOTTOM).abs() <= 1.0);
        assert_eq!(FOOTER_HEIGHT, HEADER_HEIGHT);
        assert_eq!(UNREAD_DOT, grok::UNREAD_DOT);
        assert_eq!(TYPING_DOT, super::super::sidebar::FOOTER_DOT);
    }

    /// Jacob's 1720 × 1079 window (Grok's rail-2 shot) holds 14 pictures
    /// before the rail scrolls; a 760 pt window holds 9.
    #[test]
    fn the_rail_scrolls_past_its_capacity() {
        assert_eq!(capacity(1079.0), 14);
        assert_eq!(capacity(760.0), 9);
        assert_eq!(capacity(100.0), 0);
    }

    /// Jacob 2026-10-26: in a window the toggle stays beside the traffic
    /// lights (today's spot: 12 after the rail, top 12); in full screen it is
    /// the rail's top-left slot.
    #[test]
    fn the_toggle_keeps_todays_spot_in_a_window_and_tops_the_rail_in_full_screen() {
        assert_eq!(toggle_spot(false), ToggleSpot::Header);
        assert_eq!(toggle_spot(true), ToggleSpot::RailTop);
        let header = disc_rest(StripControl::Collapse, ToggleSpot::Header);
        assert_eq!(
            f32::from(header.origin.x),
            WIDTH + STRIP_INSET,
            "x 96: 12 after the rail"
        );
        assert_eq!(f32::from(header.origin.y), 12.0);
        let rail = disc_rest(StripControl::Collapse, ToggleSpot::RailTop);
        assert_eq!(f32::from(rail.origin.x), DISC_X);
        assert_eq!(f32::from(rail.origin.y), TOGGLE_TOP);
        // + and Search never move between the two.
        for c in [StripControl::New, StripControl::Search] {
            assert_eq!(
                disc_rest(c, ToggleSpot::Header),
                disc_rest(c, ToggleSpot::RailTop)
            );
        }
    }

    /// The header's toggle: a plain window shows it at rest on the rail; full
    /// screen never (the rail has it); mid-slide never (the overlay has it);
    /// the narrow band's hidden sidebar as before.
    #[test]
    fn the_header_draws_the_toggle_only_for_a_windowed_rail_at_rest() {
        assert!(header_toggle_shown(false, WIDTH, true, false, false));
        assert!(
            !header_toggle_shown(false, WIDTH, true, false, true),
            "full screen: in the rail"
        );
        assert!(
            !header_toggle_shown(false, 190.0, true, true, false),
            "mid-slide: the overlay"
        );
        assert!(
            !header_toggle_shown(false, 296.0, false, false, false),
            "expanded: in the strip"
        );
        assert!(
            header_toggle_shown(true, 0.0, false, false, false),
            "narrow band, hidden"
        );
        assert!(!header_toggle_shown(true, 200.0, false, true, false));
    }

    /// One badge spot: typing (green) beats unread (blue); the selected chat
    /// shows no unread dot.
    #[test]
    fn typing_beats_unread_on_the_one_badge_spot() {
        assert_eq!(badge(true, true, false), Some(RailBadge::Typing));
        assert_eq!(badge(true, false, true), Some(RailBadge::Typing));
        assert_eq!(badge(false, true, false), Some(RailBadge::Unread));
        assert_eq!(badge(false, true, true), None);
        assert_eq!(badge(false, false, false), None);
    }

    /// The morph: progress from the visible width, text gone by half-way,
    /// boxes interpolated linearly.
    #[test]
    fn the_morph_runs_from_the_full_width_to_the_rail() {
        assert_eq!(progress(296.0, 296.0), 0.0);
        assert_eq!(progress(WIDTH, 296.0), 1.0);
        assert!((progress(190.0, 296.0) - 0.5).abs() < 1e-5);
        assert_eq!(text_alpha(0.0), 1.0);
        assert_eq!(text_alpha(0.5), 0.0);
        assert_eq!(text_alpha(1.0), 0.0);
        let from = bounds(20.0, 110.0, 36.0, 36.0);
        let to = bounds(24.0, 161.0, 36.0, 36.0);
        let mid = lerp_bounds(from, to, 0.5);
        assert_eq!(f32::from(mid.origin.x), 22.0);
        assert_eq!(f32::from(mid.origin.y), 135.5);
        assert_eq!(f32::from(mid.size.width), 36.0);
    }

    /// The rail as `render` lays it out, in a 1079 pt window: toggle 12,
    /// plus 56, Search 100, the first picture's top at 161 (tile 152), my
    /// picture 12 above the bottom — the mock's pixels.
    struct RailFrame;

    impl gpui::Render for RailFrame {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let disc = |sel: &'static str, top: f32| {
                div()
                    .absolute()
                    .left(px(DISC_X))
                    .top(px(top))
                    .size(px(CONTROL))
                    .debug_selector(move || sel.into())
            };
            let slot = |i: usize| {
                div()
                    .h(px(PITCH))
                    .w(px(WIDTH - 1.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(TILE))
                            .when(i == 0, |el| el.debug_selector(|| "tile-0".into()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .size(px(AVATAR))
                                    .when(i == 0, |el| el.debug_selector(|| "picture-0".into()))
                                    .when(i == 1, |el| el.debug_selector(|| "picture-1".into())),
                            ),
                    )
            };
            div()
                .relative()
                .w(px(WIDTH))
                .h(px(1079.0))
                .child(disc("toggle", TOGGLE_TOP))
                .child(disc("plus", PLUS_TOP))
                .child(disc("search", SEARCH_TOP))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(LIST_TOP))
                        .flex()
                        .flex_col()
                        .child(slot(0))
                        .child(slot(1)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(DISC_X))
                        .bottom(px(FOOTER_BOTTOM))
                        .size(px(FOOTER_AVATAR))
                        .debug_selector(|| "account".into()),
                )
        }
    }

    #[gpui::test]
    fn the_rail_lands_on_the_mocks_pixels(cx: &mut gpui::TestAppContext) {
        let (_view, vcx) = cx.add_window_view(|_, _| RailFrame);
        vcx.run_until_parked();
        let mut at = |sel: &'static str| {
            let b = vcx
                .debug_bounds(sel)
                .unwrap_or_else(|| panic!("{sel} drawn"));
            (
                f32::from(b.origin.x),
                f32::from(b.origin.y),
                f32::from(b.size.width),
                f32::from(b.size.height),
            )
        };
        assert_eq!(at("toggle"), (24.0, 12.0, 36.0, 36.0));
        assert_eq!(at("plus"), (24.0, 56.0, 36.0, 36.0));
        assert_eq!(at("search"), (24.0, 100.0, 36.0, 36.0));
        let (tx, ty, tw, th) = at("tile-0");
        assert_eq!((ty, tw, th), (152.0, 54.0, 54.0));
        assert!(
            (tx + tw / 2.0 - CENTRE_X).abs() <= 0.5,
            "tile centred at 42 ({tx})"
        );
        let (p0x, p0y, pw, _) = at("picture-0");
        assert_eq!((p0y, pw), (161.0, 36.0));
        assert!((p0x + pw / 2.0 - CENTRE_X).abs() <= 0.5);
        let (_, p1y, _, _) = at("picture-1");
        assert_eq!(p1y - p0y, PITCH);
        let (ax, ay, aw, ah) = at("account");
        assert_eq!((ax, aw, ah), (24.0, 36.0, 36.0));
        assert_eq!(1079.0 - (ay + ah), FOOTER_BOTTOM);
        assert_eq!(1079.0 - (ay + ah / 2.0), CONTROL_LINE);
    }
}
