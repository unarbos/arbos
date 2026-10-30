//! The collapsed sidebar: Grok Bot's icon rail (Jacob 2026-10-26,
//! [feedback mac-sidebar-rail-2026-10-26]).
//!
//! Pressing the sidebar toggle no longer closes the chat list; it collapses
//! it to an 84 pt rail — one centred column: the toggle, **+** and **Search**
//! stacked 8 apart under the traffic lights' slot (Jacob 2026-10-27,
//! [feedback sidebar-toggle-in-rail-2026-10-27]: "the tab button should also
//! be in the side bar" — before, a plain window kept it floating in the chat
//! header), then every chat's picture (agents, people, groups, servers)
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
    div, prelude::*, px, App, Bounds, Context, Div, ElementId, Focusable as _, Pixels, SharedString,
};

use crate::model::Id;
use crate::ui::composer::Composer;
use crate::ui::frost::edge_faded;
use crate::ui::glass::{self, WindowDrag};
use crate::ui::highlight;
use crate::ui::shell::Shell;
use crate::ui::sidebar::{
    self, is_control_click, ListEntry, StripControl, FOOTER_AVATAR, FOOTER_BOTTOM, FOOTER_HEIGHT,
    LIST_FADE_BAND, STRIP_GAP,
};
use crate::ui::theme::{Theme, CONTROL, TEXT_SMALL};

/// The rail's geometry (pt). Grok's numbers where Grok has them; the button
/// family's where the rail holds our discs.
pub(crate) mod metrics {
    use crate::ui::theme::{CONTROL, TRAFFIC_LIGHT_SLOT};

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
    /// The traffic lights' slot at the top of the rail: the 44 pt title-bar
    /// container GPUI sizes around AppKit's buttons (`TRAFFIC_LIGHT_SLOT`:
    /// 14 pt discs centred at 22, spanning x 15–75 — the rail's whole width
    /// but 9 pt). Reserved and empty of our controls, so a disc never sits
    /// under the buttons or inside their tracking areas; all window drag.
    /// Like the expanded strip, the rail moves nothing in native full screen
    /// (the lights hide, the slot stays).
    pub const LIGHTS_SLOT: f32 = TRAFFIC_LIGHT_SLOT;
    /// One disc plus the strip's gap: the column's pitch between discs.
    pub const DISC_PITCH: f32 = CONTROL + super::STRIP_GAP;
    /// The toggle, + and Search stack under the lights' slot, `STRIP_GAP`
    /// apart from it and from each other — the strip's order read top-down
    /// ([feedback sidebar-toggle-in-rail-2026-10-27]: the toggle lives in the
    /// rail with the other discs; before, a plain window kept it in the chat
    /// header, and the rail's top slot — on the lights' line — hid it under
    /// the buttons whenever that layout was drawn in a window).
    pub const TOGGLE_TOP: f32 = LIGHTS_SLOT + super::STRIP_GAP;
    pub const PLUS_TOP: f32 = TOGGLE_TOP + DISC_PITCH;
    pub const SEARCH_TOP: f32 = PLUS_TOP + DISC_PITCH;
    /// Pictures start 16 under the last disc (Grok's first card sits 16
    /// under its discs): first tile top 192 → first picture top 201.
    pub const FIRST_TILE_TOP: f32 = SEARCH_TOP + CONTROL + 16.0;
    /// The scroll box starts where the first 58 pt slot does (the 54 tile is
    /// centred in its slot, 2 above and below).
    pub const LIST_TOP: f32 = FIRST_TILE_TOP - (PITCH - TILE) / 2.0;
    /// The hover tip: 28 tall, 8 right of the rail's edge.
    pub const TIP_HEIGHT: f32 = 28.0;
    pub const TIP_GAP: f32 = 8.0;
    // Badges at the picture's bottom-right corner (`tile_badge` through
    // `ui::marks::corner`): the working spinner in a ring of the tile's colour,
    // or the unread count pill (`ui::marks`) — never a coloured dot.
}

/// Whether the chat header draws the sidebar toggle this frame: only for the
/// narrow band, which hides the sidebar outright — the header shows the
/// toggle once the sidebar has left the lights' band (as before). Collapsed
/// to the rail the toggle is the rail's own ([`metrics::TOGGLE_TOP`]), and
/// the morph's overlay carries it in between, so the header never draws it
/// ([feedback sidebar-toggle-in-rail-2026-10-27]; before, a plain window kept
/// it in the header at rest).
pub(crate) fn header_toggle_shown(sidebar_hidden: bool, sidebar_visible: f32) -> bool {
    sidebar_hidden && sidebar_visible <= crate::ui::shell::TRAFFIC_LIGHTS_INSET
}

/// The one badge spot at a rail picture's bottom-right corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RailBadge {
    /// The space's agent is working: the braille spinner in a ring.
    Working,
    /// Unread messages: the count pill. Never on the selected row.
    Unread,
}

/// Typing wins over unread — the rows' own rule (`!typing && unread` for the
/// accent dot); the selected chat shows no unread dot.
pub(crate) fn badge(working: bool, unread: bool, selected: bool) -> Option<RailBadge> {
    if working {
        Some(RailBadge::Working)
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

/// The rail list's scroll offset as its tiles land this frame. GPUI adds wheel
/// deltas to the shared offset unclamped and clamps them only in the list's
/// own prepaint, so a read during render runs past either end while the user
/// keeps scrolling there ([feedback mac-rail-hover-label-overscroll-2026-10-29]).
pub(crate) fn painted_scroll_y(scroll: &gpui::ScrollHandle) -> f32 {
    let y: f32 = scroll.offset().y.into();
    let max: f32 = scroll.max_offset().y.into();
    y.clamp(-max.max(0.0), 0.0)
}

/// The hover tip's top beside the `index`-th picture, centred on its tile.
pub(crate) fn tip_top(index: usize, scroll_y: f32) -> f32 {
    metrics::LIST_TOP + metrics::PITCH / 2.0 + index as f32 * metrics::PITCH + scroll_y
        - metrics::TIP_HEIGHT / 2.0
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

/// Top of the list frame the morph's travelling pictures are clipped to and
/// edge-faded in at progress `t`: the expanded list's frame (under the filter
/// segment) at 0, the rail list's (under Search) at 1, on the pictures' own
/// linear path. Both lists fade their rows over `LIST_FADE_BAND` at the
/// frame's top and bottom, so the pictures keep that fade the whole way and
/// the rail at rest adds none ([feedback mac-sidebar-slide-jank-2026-10-29]:
/// it used to appear the frame the fold ended, and vanish the frame an unfold
/// began).
pub(crate) fn morph_list_top(t: f32) -> f32 {
    let expanded = crate::ui::theme::HEADER_HEIGHT
        + sidebar::FILTER_TOP
        + sidebar::FILTER_HEIGHT
        + sidebar::FILTER_BOTTOM
        - LIST_FADE_BAND;
    let rail = metrics::LIST_TOP - LIST_FADE_BAND;
    expanded + (rail - expanded) * t.clamp(0.0, 1.0)
}

/// The text's fade during the morph: gone by half-way, back from half-way.
pub(crate) fn text_alpha(t: f32) -> f32 {
    (1.0 - 2.0 * t).clamp(0.0, 1.0)
}

/// A rail disc's resting frame in window coordinates.
fn disc_rest(control: StripControl) -> Bounds<Pixels> {
    bounds(metrics::DISC_X, disc_top(control), CONTROL, CONTROL)
}

/// Where macOS draws the three window buttons in a plain window (GPUI's
/// `traffic_light_position` frames: x 15–75, y 15–29): the box every control
/// of ours keeps clear. Hidden by macOS in native full screen.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn lights_frame() -> Bounds<Pixels> {
    use crate::ui::theme::{TRAFFIC_LIGHT_BUTTON, TRAFFIC_LIGHT_PITCH, TRAFFIC_LIGHT_POSITION};
    bounds(
        TRAFFIC_LIGHT_POSITION,
        TRAFFIC_LIGHT_POSITION,
        2.0 * TRAFFIC_LIGHT_PITCH + TRAFFIC_LIGHT_BUTTON,
        TRAFFIC_LIGHT_BUTTON,
    )
}

/// A tile's badge for `ui::marks::corner` (Jacob, 2026-10-27: never a coloured
/// dot): the braille spinner in a ring of the tile's colour while the space's
/// agent works, the unread count pill otherwise. `ring` is the tile's fill.
fn tile_badge(
    badge: RailBadge,
    unread_count: usize,
    ring: gpui::Hsla,
    theme: &Theme,
    cx: &mut App,
) -> Div {
    match badge {
        RailBadge::Working => crate::ui::marks::ring_disc(
            crate::ui::marks::corner_metrics(metrics::AVATAR).ring,
            ring,
        )
        .child(crate::ui::working::glyph(theme, cx)),
        RailBadge::Unread => crate::ui::marks::unread_pill(unread_count, theme)
            .border_2()
            .border_color(ring),
    }
}

/// One of the rail's three discs with the strip's own handlers.
fn rail_disc(control: StripControl, theme: &Theme, cx: &mut Context<Shell>) -> Div {
    let (glyph, size) = control.glyph();
    let id: &'static str = match control {
        StripControl::Collapse => "rail-collapse",
        StripControl::New => "rail-new",
        StripControl::Search => "rail-search",
    };
    let disc = glass::strip_disc(id, glyph, size, theme)
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
    let machine = shell.state.read(cx).machine.shown_state();

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
    // The top band stays macOS's drag region, as the strip is; the lights'
    // slot above the discs is all drag.
    column = column.child(
        div()
            .absolute()
            .top_0()
            .left_0()
            .w(px(metrics::WIDTH))
            .h(px(metrics::LIST_TOP))
            .window_drag(),
    );
    column = column
        .child(rail_disc(StripControl::Collapse, &theme, cx))
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
        let badge = badge(entry.working(), entry.unread(), selected);
        let unread_count = entry.unread_count();
        // The highlight token's selected / hover steps (`ui::highlight`); the
        // tile is its own pill (Grok's 54 pt, radius 12), hover tracked here
        // because the name tip beside the rail follows it.
        let fills = highlight::fills(&theme);
        let tile_fill = if selected {
            fills.selected
        } else if is_hover {
            fills.hover
        } else {
            gpui::transparent_black()
        };
        // The badge's ring is the tile's colour as painted on the sidebar.
        let ring = highlight::over(theme.sidebar, tile_fill);
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
                match badge {
                    // The badge in front of the picture, in its own paint layer
                    // (`ui::marks::corner`); the bare picture keeps the same box.
                    Some(b) => crate::ui::marks::corner(
                        metrics::AVATAR,
                        picture,
                        tile_badge(b, unread_count, ring, &theme, cx),
                        crate::ui::marks::CORNER_OVERHANG,
                    ),
                    None => crate::ui::marks::plain(metrics::AVATAR, picture),
                }
                .when(index == 0, |el| {
                    el.debug_selector(|| "rail-first-picture".into())
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
    // The rail's right edge is the same resize handle as the sidebar's:
    // dragging it out past the snap width restores the column at its last
    // width ([feedback sidebar-narrow-wrap-2026-10-27]).
    column.child(account).child(sidebar::resize_handle(
        "rail-resize",
        &theme,
        &shell.split_drag,
        cx,
    ))
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
    let scroll_y = painted_scroll_y(&shell.rail_scroll);
    Some(
        div()
            .absolute()
            .left(px(metrics::WIDTH + metrics::TIP_GAP))
            .top(px(tip_top(index, scroll_y)))
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
/// the three discs between the strip and the rail column, every row's
/// picture between its row and its rail slot (the selected row's card
/// shrinking into its tile), and my footer picture sliding to the rail's
/// centre.
pub(crate) fn render_overlay(
    shell: &Shell,
    t: f32,
    window_height: f32,
    cx: &mut Context<Shell>,
) -> Div {
    let theme = Theme::of(cx).clone();
    let snap = shell.morph.borrow();
    let entries = sidebar::list_entries(shell, cx);
    let machine = shell.state.read(cx).machine.shown_state();
    let scroll_y = painted_scroll_y(&shell.rail_scroll);
    let mut layer = div().absolute().inset_0();

    // Rows, clipped to the list frame between the two lists' and faded at
    // its edges the way both lists are.
    let clip_top = morph_list_top(t);
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
    layer = layer.child(edge_faded(LIST_FADE_BAND, true, true, rows));

    for control in [
        StripControl::Search,
        StripControl::New,
        StripControl::Collapse,
    ] {
        let Some(from) = snap.discs.get(&control).copied() else {
            continue;
        };
        let b = lerp_bounds(from, disc_rest(control), t);
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
                .child(glass::strip_disc(id, glyph, size, &theme)),
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
    use crate::ui::sidebar::{FOOTER_BOTTOM, FOOTER_HEIGHT, STRIP_BUTTON, STRIP_INSET};
    use crate::ui::theme::{AVATAR_ROW, CONTROL_LINE, HEADER_HEIGHT};

    /// Grok Bot's rail (`jacob-ref-grok-collapsed-rail-2.png`, 2 px/pt) and
    /// the approved mock (`mock-mac-sidebar-rail-collapsed-dark.png`).
    mod grok {
        pub const RAIL: f32 = 83.5;
        pub const AVATAR: f32 = 36.0;
        pub const TILE: f32 = 54.0;
        pub const PITCH: f32 = 58.0;
        pub const USER_CENTRE_FROM_BOTTOM: f32 = 31.0;
    }

    /// The measurement table: rail 84 (Grok 83.5 + the hairline), pictures
    /// 36 on a 58 pitch in 54 tiles, the lights' 44 pt slot left to macOS,
    /// the toggle, + and Search stacked 8 apart under it, pictures 16 under
    /// Search, my picture on the footer's line.
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
        assert_eq!(LIGHTS_SLOT, 44.0, "GPUI's title-bar container: 14 + 2 × 15");
        assert_eq!(DISC_PITCH, 44.0);
        assert_eq!(TOGGLE_TOP, 52.0, "the toggle: first disc under the lights");
        assert_eq!(PLUS_TOP, 96.0);
        assert_eq!(SEARCH_TOP, 140.0);
        assert_eq!(
            TOGGLE_TOP - LIGHTS_SLOT,
            8.0,
            "slot → toggle: the discs' gap"
        );
        assert_eq!(PLUS_TOP - (TOGGLE_TOP + CONTROL), 8.0, "discs 8 apart");
        assert_eq!(SEARCH_TOP - (PLUS_TOP + CONTROL), 8.0, "discs 8 apart");
        assert_eq!(FIRST_TILE_TOP, 192.0);
        assert_eq!(FIRST_TILE_TOP - (SEARCH_TOP + CONTROL), 16.0);
        assert_eq!(LIST_TOP, 190.0);
        let first_picture_centre = FIRST_TILE_TOP + TILE / 2.0;
        assert_eq!(first_picture_centre, 219.0);
        assert_eq!(tile_top(1) - tile_top(0), PITCH);
        // My picture: 36 tall, 12 above the bottom → centre 30 (Grok 31).
        let centre_from_bottom = FOOTER_BOTTOM + CONTROL / 2.0;
        assert_eq!(centre_from_bottom, CONTROL_LINE);
        assert!((centre_from_bottom - grok::USER_CENTRE_FROM_BOTTOM).abs() <= 1.0);
        assert_eq!(FOOTER_HEIGHT, HEADER_HEIGHT);
    }

    /// Jacob's 1720 × 1079 window (Grok's rail-2 shot) holds 14 pictures
    /// before the rail scrolls; a 760 pt window holds 8.
    #[test]
    fn the_rail_scrolls_past_its_capacity() {
        assert_eq!(capacity(1079.0), 14);
        assert_eq!(capacity(760.0), 8);
        assert_eq!(capacity(100.0), 0);
    }

    /// Jacob 2026-10-27: the toggle lives in the rail with the other discs —
    /// same column (x 24), same size, the first slot under the traffic
    /// lights — in a window and in full screen alike; + and Search follow
    /// under it. Before (v0.6.278), a plain window kept the toggle in the
    /// chat header at x 96.
    #[test]
    fn the_toggle_rests_in_the_rail_under_the_lights() {
        let toggle = disc_rest(StripControl::Collapse);
        assert_eq!(f32::from(toggle.origin.x), DISC_X);
        assert_eq!(f32::from(toggle.origin.y), TOGGLE_TOP);
        assert_eq!(f32::from(toggle.size.width), CONTROL);
        assert!(
            f32::from(toggle.origin.x) < WIDTH,
            "inside the rail, not the header"
        );
        // Under the lights' slot, never on it.
        assert!(f32::from(toggle.origin.y) >= LIGHTS_SLOT);
        let plus = disc_rest(StripControl::New);
        let search = disc_rest(StripControl::Search);
        for d in [plus, search] {
            assert_eq!(d.origin.x, toggle.origin.x, "one column");
            assert_eq!(d.size, toggle.size, "one size");
        }
        assert_eq!(f32::from(plus.origin.y - toggle.origin.y), DISC_PITCH);
        assert_eq!(f32::from(search.origin.y - plus.origin.y), DISC_PITCH);
    }

    /// The traffic lights' slot, per state (Jacob 2026-10-27: the toggle was
    /// half hidden under the buttons). macOS draws the buttons at x 15–75,
    /// y 15–29, vertically centred in GPUI's 44 pt title-bar container; the
    /// rail's discs all rest below that container, the expanded strip's all
    /// rest right of the buttons' band, and the layout is the same in a
    /// window, in native full screen (buttons hidden, slot kept), during the
    /// morph (the discs travel between the two clear regions) and whether or
    /// not the window is key (AppKit only re-tints the buttons).
    #[test]
    fn no_control_shares_the_traffic_lights_frame_in_any_state() {
        use crate::ui::shell::TRAFFIC_LIGHTS_INSET;
        use crate::ui::theme::{TRAFFIC_LIGHT_LINE, TRAFFIC_LIGHT_SLOT};
        let lights = lights_frame();
        assert_eq!(f32::from(lights.origin.y), 15.0);
        assert_eq!(f32::from(lights.bottom()), 29.0);
        assert_eq!(f32::from(lights.right()), 75.0);
        assert_eq!(TRAFFIC_LIGHT_LINE, 22.0, "Grok's lights' line");
        assert_eq!(
            TRAFFIC_LIGHT_LINE,
            TRAFFIC_LIGHT_SLOT / 2.0,
            "the buttons are vertically centred in their slot"
        );
        assert!(f32::from(lights.bottom()) <= LIGHTS_SLOT);
        assert!(
            f32::from(lights.right()) <= WIDTH,
            "inside the rail's width"
        );
        // Collapsed (rail): every disc below the slot, in a window and in full screen alike.
        for control in [
            StripControl::Collapse,
            StripControl::New,
            StripControl::Search,
        ] {
            let d = disc_rest(control);
            assert!(!d.intersects(&lights), "{control:?} on the lights");
            assert!(
                f32::from(d.origin.y) >= LIGHTS_SLOT,
                "{control:?} inside the title-bar container"
            );
        }
        // The rail's drag band covers the slot (LIST_TOP ≥ the slot).
        let drag_band = std::hint::black_box(LIST_TOP);
        assert!(drag_band >= LIGHTS_SLOT);
        // Expanded (strip): the discs start right of the lights' band; the
        // strip is the same band in full screen (`top_strip` ignores it).
        let first_strip_disc_x = TRAFFIC_LIGHTS_INSET + STRIP_INSET;
        assert!(first_strip_disc_x >= f32::from(lights.right()));
        let strip_disc = bounds(first_strip_disc_x, 12.0, CONTROL, CONTROL);
        assert!(!strip_disc.intersects(&lights));
        assert_eq!(
            crate::ui::sidebar::top_strip(true),
            crate::ui::sidebar::top_strip(false)
        );
        // Mid-morph: each disc travelling from its strip slot (the default
        // 296 pt sidebar: Collapse at the right end, + and Search 44 before)
        // to its rail slot never crosses the buttons' box — it has dropped
        // below the slot before it reaches their column.
        let strip_right = crate::ui::theme::SIDEBAR_WIDTH - STRIP_INSET;
        for (control, right_edge) in [
            (StripControl::Collapse, strip_right),
            (StripControl::New, strip_right - DISC_PITCH),
            (StripControl::Search, strip_right - 2.0 * DISC_PITCH),
        ] {
            let from = bounds(right_edge - CONTROL, 12.0, CONTROL, CONTROL);
            let to = disc_rest(control);
            for step in 0..=20 {
                let b = lerp_bounds(from, to, step as f32 / 20.0);
                assert!(
                    !b.intersects(&lights),
                    "{control:?} morph step {step} on the lights"
                );
            }
        }
    }

    /// The header's toggle is only the narrow band's (hidden sidebar): never
    /// for the rail at rest or mid-slide, never for the expanded sidebar.
    #[test]
    fn the_header_draws_the_toggle_only_for_the_hidden_sidebar() {
        assert!(
            !header_toggle_shown(false, WIDTH),
            "rail at rest: the rail has it"
        );
        assert!(
            !header_toggle_shown(false, 190.0),
            "mid-slide: the overlay has it"
        );
        assert!(!header_toggle_shown(false, 296.0), "expanded: in the strip");
        assert!(header_toggle_shown(true, 0.0), "narrow band, hidden");
        assert!(header_toggle_shown(true, 72.0));
        assert!(!header_toggle_shown(true, 200.0), "still leaving the band");
    }

    /// One badge spot: the working spinner beats the unread pill; the selected
    /// chat shows no unread pill. No typing badge (never a coloured dot).
    #[test]
    fn working_beats_unread_on_the_one_badge_spot() {
        assert_eq!(badge(true, true, false), Some(RailBadge::Working));
        assert_eq!(badge(true, false, true), Some(RailBadge::Working));
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

    /// Scout on v0.6.350 ([feedback mac-sidebar-slide-jank-2026-10-29]): "it
    /// folds in and the second it is folded in it just overlays the blur".
    /// The travelling pictures are faded in a list frame that starts as the
    /// expanded list's (its fade band ending at the filter segment's foot) and
    /// lands on the rail list's (ending at the first tile's slot), moving
    /// with the pictures — so the fade is on them from the first frame of a
    /// fold to the last of an unfold, never added or dropped at either end.
    #[test]
    fn the_morph_fades_its_pictures_in_a_frame_that_moves_with_them() {
        let expanded_band_foot = crate::ui::theme::HEADER_HEIGHT
            + sidebar::FILTER_TOP
            + sidebar::FILTER_HEIGHT
            + sidebar::FILTER_BOTTOM;
        assert_eq!(morph_list_top(0.0) + LIST_FADE_BAND, expanded_band_foot);
        assert_eq!(morph_list_top(1.0) + LIST_FADE_BAND, LIST_TOP);
        // On the pictures' linear path (`lerp_bounds`), clamped at both ends.
        let mid = morph_list_top(0.5);
        assert!((mid - (morph_list_top(0.0) + morph_list_top(1.0)) / 2.0).abs() < 1e-4);
        assert_eq!(morph_list_top(-1.0), morph_list_top(0.0));
        assert_eq!(morph_list_top(2.0), morph_list_top(1.0));
        // The first picture keeps the same place relative to the frame's top
        // band all the way: inside the frame, below the band, at both ends.
        assert!(tile_top(0) >= morph_list_top(1.0) + LIST_FADE_BAND);
    }

    /// The rail as `render` lays it out, in a 1079 pt window: toggle 52,
    /// plus 96, Search 140, the first picture's top at 201 (tile 192), my
    /// picture 12 above the bottom — the mock's column, one slot down since
    /// the toggle joined it under the lights' 44 pt slot.
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
                .child(disc("magnifyingglass", SEARCH_TOP))
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
        assert_eq!(at("toggle"), (24.0, 52.0, 36.0, 36.0));
        assert_eq!(at("plus"), (24.0, 96.0, 36.0, 36.0));
        assert_eq!(at("magnifyingglass"), (24.0, 140.0, 36.0, 36.0));
        let (tx, ty, tw, th) = at("tile-0");
        assert_eq!((ty, tw, th), (192.0, 54.0, 54.0));
        assert!(
            (tx + tw / 2.0 - CENTRE_X).abs() <= 0.5,
            "tile centred at 42 ({tx})"
        );
        let (p0x, p0y, pw, _) = at("picture-0");
        assert_eq!((p0y, pw), (201.0, 36.0));
        assert!((p0x + pw / 2.0 - CENTRE_X).abs() <= 0.5);
        let (_, p1y, _, _) = at("picture-1");
        assert_eq!(p1y - p0y, PITCH);
        let (ax, ay, aw, ah) = at("account");
        assert_eq!((ax, aw, ah), (24.0, 36.0, 36.0));
        assert_eq!(1079.0 - (ay + ah), FOOTER_BOTTOM);
        assert_eq!(1079.0 - (ay + ah / 2.0), CONTROL_LINE);
    }

    const SCROLL_TILES: usize = 20;
    const SCROLL_WINDOW: f32 = 760.0;

    /// A scrolling rail list (more pictures than fit) with the hover tip
    /// placed the way [`render_tip`] places it.
    struct RailScrollFrame {
        scroll: gpui::ScrollHandle,
        hovered: usize,
    }

    impl gpui::Render for RailScrollFrame {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let mut list = div()
                .id("rail-list")
                .absolute()
                .left_0()
                .top(px(LIST_TOP))
                .bottom(px(FOOTER_HEIGHT))
                .w(px(WIDTH - 1.0))
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .track_scroll(&self.scroll);
            for i in 0..SCROLL_TILES {
                list = list.child(
                    div()
                        .h(px(PITCH))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .size(px(TILE))
                                .debug_selector(move || format!("tile-{i}")),
                        ),
                );
            }
            div()
                .relative()
                .w(px(WIDTH + 200.0))
                .h(px(SCROLL_WINDOW))
                .child(list)
                .child(
                    div()
                        .absolute()
                        .left(px(WIDTH + TIP_GAP))
                        .top(px(tip_top(self.hovered, painted_scroll_y(&self.scroll))))
                        .w(px(80.0))
                        .h(px(TIP_HEIGHT))
                        .debug_selector(|| "tip".into()),
                )
        }
    }

    fn wheel(vcx: &mut gpui::VisualTestContext, dy: f32) {
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(px(CENTRE_X), px(400.0)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(dy))),
            modifiers: Default::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
    }

    fn centre_y(vcx: &mut gpui::VisualTestContext, sel: &'static str) -> f32 {
        let b = vcx
            .debug_bounds(sel)
            .unwrap_or_else(|| panic!("{sel} drawn"));
        f32::from(b.origin.y) + f32::from(b.size.height) / 2.0
    }

    /// Scout, 2026-10-29: with the list already at its top or bottom, more
    /// scrolling carried the hover name away from its picture while the
    /// pictures stayed put. The tip stays centred on its tile through
    /// wheel events past either end, in the frame each event draws.
    #[gpui::test]
    fn the_hover_tip_stays_on_its_tile_when_scrolling_past_the_ends(cx: &mut gpui::TestAppContext) {
        let (view, vcx) = cx.add_window_view(|_, _| RailScrollFrame {
            scroll: gpui::ScrollHandle::new(),
            hovered: 2,
        });
        vcx.run_until_parked();
        assert_eq!(centre_y(vcx, "tip"), centre_y(vcx, "tile-2"));

        for _ in 0..3 {
            wheel(vcx, 240.0);
            assert_eq!(
                centre_y(vcx, "tile-2"),
                tile_top(2) + TILE / 2.0,
                "top: list unmoved"
            );
            assert_eq!(
                centre_y(vcx, "tip"),
                centre_y(vcx, "tile-2"),
                "top: tip on its tile"
            );
        }

        let last = SCROLL_TILES - 1;
        view.update(vcx, |frame, cx| {
            frame.hovered = last;
            frame
                .scroll
                .set_offset(gpui::point(px(0.0), px(-100_000.0)));
            cx.notify();
        });
        vcx.run_until_parked();
        let resting = centre_y(vcx, "tile-19");
        assert_eq!(centre_y(vcx, "tip"), resting);
        assert!(
            resting < SCROLL_WINDOW - FOOTER_HEIGHT,
            "the last tile is in view at the bottom ({resting})"
        );
        for _ in 0..3 {
            wheel(vcx, -240.0);
            assert_eq!(centre_y(vcx, "tile-19"), resting, "bottom: list unmoved");
            assert_eq!(centre_y(vcx, "tip"), resting, "bottom: tip on its tile");
        }
    }
}
