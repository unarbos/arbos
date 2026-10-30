//! The chat list: the iPhone home screen as a Mac sidebar. The top strip holds
//! three round buttons on the right — search (opens the command palette,
//! `ui::palette`; ⌘⇧F / ⌘K do the same), "+", and the collapse toggle — then
//! the iPhone home's All · Agents · People · Servers filter, then every agent,
//! person and server as one row — avatar, name, time, last line. The footer
//! is Grok Bot's: my avatar (its dot is the machine state) and name at the
//! left, and — only while an update is ready for a click — the blue
//! cloud-download disc at the right end.
//!
//! # Three-pane rule (app-wide, Jacob 2026-10-20; `docs/mac-design-rules.md`)
//!
//! Exactly three panes. **Left** always lists chats, groups and servers, and
//! its top strip holds only three controls: **+**, **search**, **minimize**
//! (collapse). Entering a server slides this pane over to that server's
//! channel list. **Centre** is always a chat. **Right** shows every
//! details / members / settings screen and closes with an X. The strip's
//! composition is pinned by [`strip_controls`] and its tests; #1491 replaced
//! the buttons with an always-open Search pill, and Jacob asked for the
//! compact round buttons back ([feedback sidebar-strip-filter-2026-10-24]).
//! The buttons are Grok Bot's size — `theme::CONTROL` 36, glyph 19, 8 apart —
//! and Search opens the centred palette instead of an inline field
//! ([feedback search-palette-2026-10-24]).

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, AnyElement, App, Context, Div, DragMoveEvent, ElementId, Focusable as _,
    FontWeight, MouseButton, Render, RenderImage, SharedString, Window,
};

use crate::app::AppState;
use crate::model::{Id, MachineState, Message, Project, Role, ServerInfo};
use crate::search;
use crate::ui::avatars;
use crate::ui::composer::Composer;
use crate::ui::frost::{edge_faded, edge_faded_sides};
use crate::ui::glass::{self, WindowDrag};
use crate::ui::groups;
use crate::ui::highlight::{self, Shape};
use crate::ui::identity;
use crate::ui::motion::PulseClock;
use crate::ui::people;
use crate::ui::sf_symbols;
use crate::ui::shell::{Shell, TRAFFIC_LIGHTS_INSET};
use crate::ui::sidebar_fit;
use crate::ui::split_handle;
use crate::ui::theme::{
    Theme, AVATAR_ROW, CONTROL, CONTROL_GAP, CONTROL_GLYPH, CONTROL_GLYPH_WIDE, HEADER_HEIGHT,
    PANEL_RADIUS, TEXT_BODY, TEXT_SMALL, TEXT_TINY,
};

/// Drag payload for the chat-list column's right-edge resize handle.
#[derive(Clone)]
pub(crate) struct SidebarResizeDrag;

/// The drag's (empty) ghost; it holds the guard that keeps the handle's
/// hairline in its drag state until GPUI drops the ghost at mouse up.
struct SidebarResizeDragGhost {
    _drag: split_handle::DragGuard,
}

impl Render for SidebarResizeDragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A pinned row being dragged to a new slot in the Pinned section ([feedback
/// sidebar-pinning-mac-2026-10-28]). The row itself moves live in the list (the
/// ghost under the pointer draws nothing); `to` is the slot under the pointer among
/// the pinned rows the current filter shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PinDragState {
    pub(crate) id: Id,
    pub(crate) to: usize,
}

/// The drag's payload (GPUI's `on_drag` value).
#[derive(Clone, Debug)]
struct PinDrag {
    id: Id,
}

struct PinDragGhost;

impl Render for PinDragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The "Pinned" label over the section: `TEXT_SMALL` medium in `text_muted` at the rows'
/// text inset (the list's 8 + the row's 12), in a 26 pt band — the voice of the
/// "Hidden chats · N ›" footer.
pub(crate) const PINNED_HEADER_H: f32 = 26.0;
/// The hairline band after the pinned rows: 1 pt `border` with 4 pt margins
/// (`glass::MENU_DIVIDER_HEIGHT`).
pub(crate) const PINNED_DIVIDER_H: f32 = glass::MENU_DIVIDER_HEIGHT;

fn pinned_header(theme: &Theme) -> gpui::Stateful<Div> {
    div()
        .id("sidebar-pinned-header")
        .debug_selector(|| "sidebar-pinned-header".into())
        .flex_shrink_0()
        .h(px(PINNED_HEADER_H))
        .px(px(12.0))
        .flex()
        .items_center()
        .text_size(px(TEXT_SMALL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text_muted)
        .child("Pinned")
}

fn pinned_divider(theme: &Theme) -> gpui::Stateful<Div> {
    div()
        .id("sidebar-pinned-divider")
        .debug_selector(|| "sidebar-pinned-divider".into())
        .flex_shrink_0()
        .h(px(1.0))
        .mx(px(12.0))
        .my(px((PINNED_DIVIDER_H - 1.0) / 2.0))
        .bg(theme.border)
}

/// The pinned row's mark before its time: SF `pin.fill` at 11 pt semibold in
/// `text_muted` — the iPhone row's glyph.
fn pin_mark(id: &Id, theme: &Theme) -> Div {
    sf_symbols::symbol(
        "pin.fill",
        11.0,
        sf_symbols::Weight::Semibold,
        13.0,
        theme.text_muted,
    )
    .debug_selector(|| format!("row-pin-{id}"))
}

/// The list as the drop target of a pin drag: every move picks the slot under the
/// pointer from the pinned cards' recorded bounds, the drop (or a mouse-up anywhere)
/// commits the previewed order through `AppState::reorder_pins`.
fn pin_drop_target(list: gpui::Stateful<Div>, cx: &mut Context<Shell>) -> gpui::Stateful<Div> {
    let weak = cx.weak_entity();
    list.on_drag_move::<PinDrag>(cx.listener(|this, ev: &DragMoveEvent<PinDrag>, _, cx| {
        let y = f32::from(ev.event.position.y);
        let Some(drag) = this.pin_drag.clone() else {
            return;
        };
        let to = this.pin_slot_at(&drag.id, y, cx);
        if to != drag.to {
            this.pin_drag = Some(PinDragState { id: drag.id, to });
            cx.notify();
        }
    }))
    .can_drop(|dragged, _, _| dragged.is::<PinDrag>())
    .on_drop(cx.listener(|this, _: &PinDrag, _, cx| this.finish_pin_drag(cx)))
    .child(
        gpui::canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let weak = weak.clone();
                window.on_mouse_event(move |_: &gpui::MouseUpEvent, _, _, cx| {
                    weak.update(cx, |this, cx| this.finish_pin_drag(cx)).ok();
                });
            },
        )
        .absolute()
        .size(px(0.0)),
    )
}

impl Shell {
    /// Which slot among the shown pinned rows the pointer's `y` is over: the number of
    /// *other* pinned cards whose centre is above it.
    pub(crate) fn pin_slot_at(&self, dragged: &Id, y: f32, cx: &Context<Shell>) -> usize {
        let state = self.state.read(cx);
        let cards = &self.morph.borrow().cards;
        state
            .pins
            .iter()
            .filter_map(|k| AppState::pinned_space_id(k))
            .filter(|id| *id != dragged.as_str())
            .filter_map(|id| cards.get(id))
            .filter(|b| f32::from(b.origin.y) + f32::from(b.size.height) / 2.0 < y)
            .count()
    }

    /// Start dragging a pinned row (its current slot is where it starts).
    pub(crate) fn begin_pin_drag(&mut self, id: Id, cx: &mut Context<Shell>) {
        let (pinned, _) = list_sections(self, cx);
        let to = pinned.iter().position(|e| e.id() == &id).unwrap_or(0);
        self.pin_drag = Some(PinDragState { id, to });
        self.row_menu = None;
        cx.notify();
    }

    /// Drop: the order the list shows becomes the pin order (through the relay).
    pub(crate) fn finish_pin_drag(&mut self, cx: &mut Context<Shell>) {
        if self.pin_drag.is_none() {
            return;
        }
        let (pinned, _) = list_sections(self, cx);
        let shown: Vec<Id> = pinned.iter().map(|e| e.id().clone()).collect();
        self.pin_drag = None;
        self.state.update(cx, |s, cx| s.reorder_pins(&shown, cx));
        cx.notify();
    }
}

/// The column's right-edge resize handle — the expanded sidebar's and the
/// rail's are the same element (`id`s differ), so one drag runs across the
/// snap between them: `split_handle::right_edge`'s invisible band and
/// hairline (col-resize cursor), the drag's start resets the gesture, every
/// move goes to `AppState::drag_sidebar_edge` with the pointer's x
/// ([feedback app-01M5MJ9Y91W9ZET71KTCE7PS65], [feedback sidebar-narrow-wrap-2026-10-27],
/// minimal look [feedback sidebar-slide-handle-minimal-2026-10-28]).
pub(crate) fn resize_handle(
    id: &'static str,
    theme: &Theme,
    drag: &split_handle::DragState,
    cx: &mut Context<Shell>,
) -> gpui::Stateful<Div> {
    // Both panes draw `border_r_1`: the hairline lands on that pixel.
    split_handle::right_edge(id, theme, drag.active(), 1.0)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_drag(SidebarResizeDrag, {
            let shell = cx.entity().downgrade();
            let drag = drag.clone();
            move |_, _, _, cx| {
                if let Some(shell) = shell.upgrade() {
                    shell.update(cx, |this, cx| {
                        this.state.update(cx, |s, _| s.begin_sidebar_edge_drag());
                    });
                }
                let guard = drag.begin();
                cx.new(|_| SidebarResizeDragGhost { _drag: guard })
            }
        })
        .on_drag_move(
            cx.listener(|this, ev: &DragMoveEvent<SidebarResizeDrag>, _, cx| {
                // The column docks at the window's left, so mouse x is the width.
                let x = f32::from(ev.event.position.x);
                this.state.update(cx, |s, cx| s.drag_sidebar_edge(x, cx));
            }),
        )
}

/// The drag strip beside the traffic lights: Grok Bot's top band, the same
/// height as the chat header (`HEADER_HEIGHT`). Its three round buttons
/// (search · + · collapse, [`STRIP_BUTTON`] discs, [`STRIP_GAP`] apart,
/// [`STRIP_INSET`] from the right edge) are centred in it, on the family's
/// line `CONTROL_LINE` = 30 — Grok's discs span 12–48 pt while its lights are
/// centred at 22 ([feedback topbar-grok-parity-2026-10-24]; the 44 band with
/// the discs on the lights' line read "too high" to Jacob). The collapse
/// toggle keeps the right end so folding the pane stays intuitive (#1491).
///
/// Native full screen keeps this exact band: nothing is added above the
/// strip and nothing moves; the lights are macOS's own and follow its
/// full-screen rules ([feedback fullscreen-traffic-lights-2026-10-24], then
/// Jacob 2026-10-24 18:3x: real buttons only; pinned by
/// `strip_tests::full_screen_adds_no_band_above_the_strip`).
pub(crate) fn top_strip(_fullscreen: bool) -> f32 {
    if cfg!(target_os = "macos") {
        HEADER_HEIGHT
    } else {
        40.0
    }
}

/// Disc size of the strip's three round buttons: the button family's
/// [`CONTROL`] — Grok Bot's 36 pt ([feedback search-palette-2026-10-24]; the
/// #1134 mock's 32 before).
pub(crate) const STRIP_BUTTON: f32 = CONTROL;
/// Space between two strip buttons (Grok: 8).
pub(crate) const STRIP_GAP: f32 = CONTROL_GAP;
/// The strip's right inset: the discs end 4 pt past the rows' card edge, as
/// Grok's do (rows are inset 8; 12 − 8).
pub(crate) const STRIP_INSET: f32 = 12.0;
/// The filter capsules under the strip: the iPhone `PaneTabs` geometry (32 pt
/// capsule, 4 pt gap) with 10 pt side padding instead of the iPhone's 16 —
/// four segments at 16 would be wider than the 296 pt default sidebar.
pub(crate) const FILTER_HEIGHT: f32 = 32.0;
pub(crate) const FILTER_GAP: f32 = 4.0;
pub(crate) const FILTER_PAD_X: f32 = 10.0;
/// Space between the strip's bottom edge and the capsules: Grok Bot's first
/// chat card starts 64 pt under the window top, 16 under its discs (bottom
/// 48). The filter is the first thing under our strip, so its capsules start
/// there: 60 + 4 ([feedback topbar-grok-parity-2026-10-24]; was 44 + 6 = 50).
pub(crate) const FILTER_TOP: f32 = 4.0;
pub(crate) const FILTER_BOTTOM: f32 = 8.0;
/// The side fade on the capsule row when it scrolls (a narrow column).
pub(crate) const FILTER_FADE_BAND: f32 = 12.0;
/// Server rows draw their picture as a rounded square (corner = this fraction
/// of the size) so they read apart from the agent and people discs.
pub(crate) const SERVER_PICTURE_CORNER: f32 = 11.0 / 36.0;

/// The footer — my avatar with the machine dot, my name — is Grok Bot's: the
/// top strip mirrored at the bottom edge. Measured on Jacob's Oct-2026 Grok
/// Mac refs (`media/group-chat-redesign/refs/1-agent-chat-topbutton-sidebar.png`
/// in the project store, 2 px/pt, window bottom at the image's last row): the
/// avatar is 72 px at x 26, rows 1574–1645 of 1670 → **36 pt** (`CONTROL`,
/// the button family), **13 pt** from the sidebar's left edge, **12 pt**
/// above the window's bottom edge, centre **30** from the bottom (`CONTROL_LINE`,
/// the strip's discs' line) — so the footer is a 60 pt band like the strip
/// ([feedback footer-avatar-grok-size-2026-10-24]). Before: 28 pt at x 18,
/// 16 above the bottom (centre already at 30).
pub(crate) const FOOTER_HEIGHT: f32 = HEADER_HEIGHT;
pub(crate) const FOOTER_AVATAR: f32 = CONTROL;
pub(crate) const FOOTER_INSET: f32 = 13.0;
pub(crate) const FOOTER_BOTTOM: f32 = (FOOTER_HEIGHT - FOOTER_AVATAR) / 2.0;
/// The hover card behind avatar + name: the rows' 8 pt side inset, so its
/// left pad is what puts the avatar at [`FOOTER_INSET`].
pub(crate) const FOOTER_ROW_INSET: f32 = 8.0;
pub(crate) const FOOTER_ROW_PAD_X: f32 = FOOTER_INSET - FOOTER_ROW_INSET;
/// The hover card's height; it is centred in the band: 60 = 8 + 44 + 8, and
/// the card itself is 4 + 36 + 4, which is what puts the avatar 12 above the
/// bottom edge.
pub(crate) const FOOTER_ROW_HEIGHT: f32 = 44.0;
pub(crate) const FOOTER_ROW_PAD_Y: f32 = FOOTER_BOTTOM - (FOOTER_ROW_HEIGHT - FOOTER_AVATAR) / 2.0;
/// The machine dot scales with the avatar (28 → 36 carried 7 / 11 to 9 / 13):
/// a 9 pt dot in a 13 pt ring of the sidebar colour, 2 pt past the avatar's
/// bottom-right edge.
/// Measured pins; the mark's ring is drawn from `marks::corner_metrics(FOOTER_AVATAR)`
/// (the same 9 / 13 / 2) — the numbers stay here for the footer tests.
#[cfg(test)]
pub(crate) const FOOTER_DOT: f32 = 9.0;
#[cfg(test)]
pub(crate) const FOOTER_DOT_RING: f32 = 13.0;
#[cfg(test)]
pub(crate) const FOOTER_DOT_OVERHANG: f32 = 2.0;

/// Grok Bot's update button at the footer's right end
/// ([feedback footer-update-button-grok-2026-10-24]): on the same ref rows
/// as the avatar, a solid blue disc at x 449–520 px — 72 px = **36** (the
/// button family), exactly under the strip's rightmost disc ("+" in Grok,
/// our collapse), so it keeps the strip's right inset; fill `#326CCB`, glyph
/// `#F5F5F5` boxed 26 × 28 px → 13 × 14 pt (`cloud-download` at 20). Grok's
/// pill sits 8 before it (`STRIP_GAP`); our hover card does the same.
pub(crate) const FOOTER_DISC: f32 = CONTROL;
pub(crate) const FOOTER_DISC_INSET: f32 = STRIP_INSET;
pub(crate) const FOOTER_DISC_GLYPH: f32 = 20.0;
/// While the disc works (download / install / relaunch) its fill pulses
/// between this alpha and 1.
pub(crate) const DISC_BUSY_ALPHA_MIN: f32 = 0.55;

/// The disc's frame: a [`FOOTER_DISC`] circle, [`STRIP_GAP`] after the hover
/// card, ending at the strip's right inset. Fill, glyph and handlers are the
/// caller's (`Shell::render_update_disc`).
pub(crate) fn footer_disc_frame(id: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .size(px(FOOTER_DISC))
        .flex_shrink_0()
        .ml(px(STRIP_GAP))
        .mr(px(FOOTER_DISC_INSET - FOOTER_ROW_INSET))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .debug_selector(move || id.into())
}

/// The footer band as `render` draws it: the hover card (avatar + name) fills
/// the width inside the rows' inset, centred in the 60 pt band; `disc` is the
/// update button when one is due.
pub(crate) fn footer_frame(row: impl IntoElement, disc: Option<impl IntoElement>) -> Div {
    div()
        .h(px(FOOTER_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .px(px(FOOTER_ROW_INSET))
        .py(px(FOOTER_ROW_PAD_Y))
        .debug_selector(|| "sidebar-footer".into())
        .child(row)
        .children(disc)
}

/// The hover card's frame: [`FOOTER_ROW_HEIGHT`] tall, [`FOOTER_ROW_PAD_X`]
/// inside, avatar then name 10 apart.
pub(crate) fn footer_row_frame() -> gpui::Stateful<Div> {
    div()
        .id("sidebar-account")
        .flex_1()
        .min_w_0()
        .h(px(FOOTER_ROW_HEIGHT))
        .px(px(FOOTER_ROW_PAD_X))
        .flex()
        .items_center()
        .gap(px(10.0))
        .rounded(px(PANEL_RADIUS - 4.0))
        .debug_selector(|| "sidebar-account".into())
}

/// One control of the top strip, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum StripControl {
    /// The round search button: opens the command palette.
    Search,
    /// The round "+" (new chat / new channel inside a server).
    New,
    /// The round collapse toggle: folds the pane.
    Collapse,
}

impl StripControl {
    /// Every control is one [`STRIP_BUTTON`] disc — the button family's size.
    pub(crate) fn size(self) -> f32 {
        STRIP_BUTTON
    }

    /// The glyph inside the disc: the plus and the magnifier draw Grok's
    /// 13–14 pt marks at [`CONTROL_GLYPH`]; the collapse rect is a wider
    /// icon and takes [`CONTROL_GLYPH_WIDE`] for the same visible box.
    pub(crate) fn glyph(self) -> (&'static str, f32) {
        match self {
            StripControl::Search => ("magnifyingglass", CONTROL_GLYPH),
            StripControl::New => ("plus", CONTROL_GLYPH),
            StripControl::Collapse => ("sidebar.left", CONTROL_GLYPH_WIDE),
        }
    }
}

/// What the strip holds, left to right (three-pane rule: exactly +, search,
/// minimize — nothing else). `show_plus` is false for a server member who
/// cannot add channels; the search is a spacer inside a server.
pub(crate) fn strip_controls(in_server: bool, show_plus: bool) -> Vec<StripControl> {
    let mut controls = Vec::with_capacity(3);
    if !in_server {
        controls.push(StripControl::Search);
    }
    if show_plus {
        controls.push(StripControl::New);
    }
    controls.push(StripControl::Collapse);
    controls
}

/// The filter row is drawn under the strip whenever the chat list is, and
/// never inside a server (the channel list has nothing to scope).
pub(crate) fn filter_shown(in_server: bool) -> bool {
    !in_server
}

/// The segment under the strip — the iPhone home's `HomeListFilter`
/// (`HomeView.swift`) with the same four labels; both apps scope the same list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ListFilter {
    #[default]
    All,
    /// My agents only — not people, not agents shared in by someone else.
    Agents,
    /// People chats: DMs and groups.
    People,
    /// Servers only.
    Servers,
}

impl ListFilter {
    pub(crate) const ALL: [ListFilter; 4] = [
        ListFilter::All,
        ListFilter::Agents,
        ListFilter::People,
        ListFilter::Servers,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            ListFilter::All => "All",
            ListFilter::Agents => "Agents",
            ListFilter::People => "People",
            ListFilter::Servers => "Servers",
        }
    }

    /// `ARBOS_SIDEBAR_FILTER=…` (rig) and the labels, case-insensitively.
    pub(crate) fn parse(s: &str) -> Option<Self> {
        ListFilter::ALL
            .into_iter()
            .find(|f| f.label().eq_ignore_ascii_case(s.trim()))
    }

    /// Whether a chat row (agent, DM, group) is in this segment.
    pub(crate) fn admits_space(self, space: &Project) -> bool {
        match self {
            ListFilter::All => true,
            ListFilter::Agents => !space.is_people() && !space.is_guest(),
            ListFilter::People => space.is_people(),
            ListFilter::Servers => false,
        }
    }

    /// Whether server rows are in this segment.
    pub(crate) fn admits_servers(self) -> bool {
        matches!(self, ListFilter::All | ListFilter::Servers)
    }

    /// Copy for an empty list under this segment (no query typed).
    pub(crate) fn empty_copy(self) -> &'static str {
        match self {
            ListFilter::All => "No conversations yet",
            ListFilter::Agents => "No agents yet",
            ListFilter::People => "No people chats yet",
            ListFilter::Servers => "No servers yet",
        }
    }

    /// The segment persists with the sidebar state (`<config>/sidebar_filter`,
    /// the label in lower case) so the rail reopens on the segment it was
    /// left on ([feedback mac-sidebar-rail-2026-10-26]). The rig's
    /// `ARBOS_SIDEBAR_FILTER` wins over the file.
    pub(crate) fn load() -> Self {
        if let Some(f) = std::env::var("ARBOS_SIDEBAR_FILTER")
            .ok()
            .and_then(|v| Self::parse(&v))
        {
            return f;
        }
        Self::load_from(&crate::sync::credentials::config_dir())
    }

    pub(crate) fn save(self) {
        self.save_to(&crate::sync::credentials::config_dir());
    }

    /// A missing or unknown value means All (today's default).
    pub(crate) fn load_from(dir: &std::path::Path) -> Self {
        std::fs::read_to_string(dir.join(FILTER_FILE))
            .ok()
            .and_then(|s| Self::parse(&s))
            .unwrap_or_default()
    }

    pub(crate) fn save_to(self, dir: &std::path::Path) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join(FILTER_FILE), self.label().to_ascii_lowercase());
    }
}

const FILTER_FILE: &str = "sidebar_filter";

/// Newest message with prose across a space's chats (archived ones too — the
/// history is still there), for the preview line.
fn latest<'a>(state: &'a AppState, space: &Project) -> Option<&'a Message> {
    let mut best: Option<&Message> = None;
    for chat in state.tabs(&space.id) {
        if let Some(m) = state
            .messages_of(&chat.id)
            .iter()
            .rev()
            // A row hidden for me (someone I blocked) never previews.
            .find(|m| !m.plain_text().trim().is_empty() && !state.hides_message(&chat.id, m))
        {
            if best.map(|b| m.created_at > b.created_at).unwrap_or(true) {
                best = Some(m);
            }
        }
    }
    best
}

pub fn last_activity(state: &AppState, space: &Project) -> u64 {
    match latest(state, space) {
        Some(m) if m.created_at > 0 => m.created_at,
        _ => state
            .tabs(&space.id)
            .iter()
            .map(|c| c.last_message_at)
            .max()
            .unwrap_or(0),
    }
}

/// Strip inline markdown markers for a one-line preview.
pub fn plain(s: &str) -> String {
    let mut t = s.to_string();
    for mark in ["**", "__", "`", "*", "_", "#"] {
        t = t.replace(mark, "");
    }
    t.trim().to_string()
}

/// `preview` for the headless tests.
#[cfg(test)]
pub(crate) fn preview_for_tests(state: &AppState, space: &Project) -> String {
    preview(state, space)
}

fn preview(state: &AppState, space: &Project) -> String {
    let Some(m) = latest(state, space) else {
        if space.is_people() {
            return if space.pending {
                "Invited · opens when they sign in".into()
            } else {
                "Say hi".into()
            };
        }
        return if state.project_is_streaming(&space.id) {
            format!("{} is working", space.name)
        } else {
            "No messages yet".into()
        };
    };
    let text = m.plain_text();
    let line = text.lines().next().unwrap_or_default();
    let who = if space.is_people() {
        people_preview_who(
            space.is_people_group(),
            state.is_mine(m),
            &m.sender,
            m.sender_handle.as_deref(),
        )
    } else if m.role == Role::User {
        "You".to_string()
    } else {
        String::new()
    };
    if who.is_empty() {
        plain(line)
    } else {
        format!("{who}: {}", plain(line))
    }
}

/// The label before a people chat's preview text: "You" for my own line, `@handle`
/// for anyone else — but not for the other person of a 1:1 chat, whose name the row
/// already carries in its own header. Naming them there printed the handle twice,
/// "@DaWizzaa" over "@DaWizzaa: …" ([feedback doubled-sender-prefix-mac-2026-10-30]).
/// A group row is headed by the group, and an agent in a 1:1 chat is not the person
/// the row names, so both keep their label.
pub(crate) fn people_preview_who(
    group: bool,
    mine: bool,
    sender: &str,
    sender_handle: Option<&str>,
) -> String {
    if mine {
        return "You".into();
    }
    if !group && sender.starts_with("user:") {
        return String::new();
    }
    sender_handle.map(|h| format!("@{h}")).unwrap_or_default()
}

/// "9:54 PM" today, "Yesterday", a weekday within the week, else "Oct 3".
pub fn time_label(ms: u64) -> String {
    use chrono::{Datelike, Local, TimeZone};
    if ms == 0 {
        return String::new();
    }
    let Some(t) = Local.timestamp_millis_opt(ms as i64).single() else {
        return String::new();
    };
    let now = Local::now();
    let today = now.date_naive();
    let day = t.date_naive();
    if day == today {
        return t.format("%-I:%M %p").to_string();
    }
    if today.pred_opt() == Some(day) {
        return "Yesterday".into();
    }
    if (today - day).num_days() < 7 {
        return t.format("%A").to_string();
    }
    if day.year() == today.year() {
        t.format("%b %-d").to_string()
    } else {
        t.format("%b %-d, %Y").to_string()
    }
}

/// "Today 9:39 PM", "Yesterday 8:02 AM", "Mon 3:02 PM", "Oct 3, 11:18 AM".
pub fn stamp_label(ms: u64) -> String {
    use chrono::{Datelike, Local, TimeZone};
    let Some(t) = Local.timestamp_millis_opt(ms as i64).single() else {
        return String::new();
    };
    let now = Local::now();
    let today = now.date_naive();
    let day = t.date_naive();
    let time = t.format("%-I:%M %p").to_string();
    if day == today {
        format!("Today {time}")
    } else if today.pred_opt() == Some(day) {
        format!("Yesterday {time}")
    } else if (today - day).num_days() < 7 {
        format!("{} {time}", t.format("%a"))
    } else if day.year() == today.year() {
        format!("{}, {time}", t.format("%b %-d"))
    } else {
        format!("{}, {time}", t.format("%b %-d, %Y"))
    }
}

/// My picture with the machine dot — the footer's 36 pt disc, drawn the same
/// at the foot of the expanded list and of the collapsed rail.
pub(crate) fn account_avatar(
    shell: &Shell,
    machine: MachineState,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Div {
    let state = shell.state.read(cx);
    let picture = match state.account.as_ref() {
        Some(a) => {
            let person = crate::model::Person {
                id: a.id.clone(),
                handle: if a.login.is_empty() {
                    a.name.clone()
                } else {
                    a.login.clone()
                },
                name: Some(a.name.clone()),
                avatar_url: a.avatar_url.clone(),
                status: String::new(),
                role: String::new(),
                last_seen_at: None,
                online: false,
                avatar_color_index: None,
            };
            people::avatar(state.avatar(&a.avatar_url), &person, FOOTER_AVATAR, theme)
        }
        None => div()
            .size(px(FOOTER_AVATAR))
            .rounded_full()
            .bg(theme.control_fill())
            .flex()
            .items_center()
            .justify_center()
            .child(sf_symbols::glyph(
                "person.fill",
                FOOTER_AVATAR / 2.0,
                theme.text,
            )),
    };
    // The machine mark in front of the picture (`ui::marks::machine_corner`, its
    // own paint layer), ringed in the sidebar colour. Never a coloured dot
    // (Jacob, 2026-10-27): the spinner while the machine wakes, the red `!` on a
    // fault, nothing at rest — then the bare picture in the same box.
    crate::ui::marks::machine_corner(
        FOOTER_AVATAR,
        picture,
        machine,
        theme.sidebar,
        |ring| ring.debug_selector(|| "dot-ring".into()),
        theme,
        cx,
    )
    .debug_selector(|| "sidebar-account-avatar".into())
}

pub(crate) struct RowView {
    pub(crate) space: Project,
    pub(crate) selected: bool,
    pub(crate) streaming: bool,
    pub(crate) unread: bool,
    /// Missed messages across the space's tabs (≥ 1 whenever `unread`).
    pub(crate) unread_count: usize,
    pub(crate) typing: bool,
    pub(crate) preview: String,
    pub(crate) time: String,
    pub(crate) person_image: Option<Arc<RenderImage>>,
    pub(crate) guests: people::PeopleWithAvatars,
    /// In the Pinned section ([feedback sidebar-pinning-mac-2026-10-28]): the row carries
    /// the muted `pin.fill` before its time.
    pub(crate) pinned: bool,
}

/// A server in the chat list (D88 rule 1): the same row as a group — disc, name, preview,
/// time, unread dot — sorted among the chats by last activity.
pub(crate) struct ServerRowView {
    pub(crate) server: ServerInfo,
    pub(crate) selected: bool,
    pub(crate) unread: bool,
    pub(crate) unread_count: usize,
    pub(crate) preview: String,
    pub(crate) time: String,
    pub(crate) pinned: bool,
}

/// One entry of the left list, chats and servers interleaved.
pub(crate) enum ListEntry {
    Chat(Box<RowView>),
    Server(Box<ServerRowView>),
}

impl ListEntry {
    /// The row's stable key: the space id or the server id.
    pub(crate) fn id(&self) -> &Id {
        match self {
            ListEntry::Chat(r) => &r.space.id,
            ListEntry::Server(r) => &r.server.id,
        }
    }

    pub(crate) fn selected(&self) -> bool {
        match self {
            ListEntry::Chat(r) => r.selected,
            ListEntry::Server(r) => r.selected,
        }
    }

    pub(crate) fn unread(&self) -> bool {
        match self {
            ListEntry::Chat(r) => r.unread,
            ListEntry::Server(r) => r.unread,
        }
    }

    pub(crate) fn unread_count(&self) -> usize {
        match self {
            ListEntry::Chat(r) => r.unread_count,
            ListEntry::Server(r) => r.unread_count,
        }
    }

    fn set_pinned(&mut self, pinned: bool) {
        match self {
            ListEntry::Chat(r) => r.pinned = pinned,
            ListEntry::Server(r) => r.pinned = pinned,
        }
    }

    /// The space's agent is working (the rail's spinner badge).
    pub(crate) fn working(&self) -> bool {
        match self {
            ListEntry::Chat(r) => r.streaming,
            ListEntry::Server(_) => false,
        }
    }

    /// The name the rail's hover tip shows.
    pub(crate) fn name(&self) -> String {
        match self {
            ListEntry::Chat(r) => row_name(&r.space),
            ListEntry::Server(r) => r.server.name.clone(),
        }
    }

    /// The row's picture at `size` — the same builders the expanded rows use
    /// (one resolver for every surface, #1912).
    pub(crate) fn picture(
        &self,
        shell: &Shell,
        size: f32,
        theme: &Theme,
        cx: &Context<Shell>,
    ) -> Div {
        match self {
            ListEntry::Chat(r) => chat_picture(r, shell, size, theme, cx),
            ListEntry::Server(r) => {
                server_row_picture(&r.server, shell.state.read(cx), size, theme)
            }
        }
    }
}

/// The chat list's entries under the current filter — the pinned rows first in the
/// user's order, then the rest newest activity first — shared by the expanded list and
/// the collapsed rail so both show the same rows in the same order.
pub(crate) fn list_entries(shell: &Shell, cx: &Context<Shell>) -> Vec<ListEntry> {
    let (mut pinned, rest) = list_sections(shell, cx);
    pinned.extend(rest);
    pinned
}

/// The list in two parts ([feedback sidebar-pinning-mac-2026-10-28]): the **Pinned**
/// section — the rows of `AppState::pins` the current filter admits, in the pin order,
/// never re-sorted by unread or activity — and the rest in today's order. While a pin
/// is being dragged, the dragged row sits at the slot under the pointer. Hidden chats
/// have no pinned part (an archived row is never pinned).
pub(crate) fn list_sections(
    shell: &Shell,
    cx: &Context<Shell>,
) -> (Vec<ListEntry>, Vec<ListEntry>) {
    let rows = unsorted_entries(shell, cx);
    let state = shell.state.read(cx);
    let mut pinned: Vec<(usize, ListEntry)> = Vec::new();
    let mut rest: Vec<ListEntry> = Vec::new();
    for entry in rows {
        match state.pin_index(entry.id()) {
            Some(i) if !shell.hidden_open => pinned.push((i, entry)),
            _ => rest.push(entry),
        }
    }
    pinned.sort_by_key(|(i, _)| *i);
    let mut pinned: Vec<ListEntry> = pinned.into_iter().map(|(_, e)| e).collect();
    if let Some(drag) = &shell.pin_drag {
        if let Some(from) = pinned.iter().position(|e| e.id() == &drag.id) {
            let row = pinned.remove(from);
            let to = drag.to.min(pinned.len());
            pinned.insert(to, row);
        }
    }
    for e in pinned.iter_mut() {
        e.set_pinned(true);
    }
    (pinned, rest)
}

fn unsorted_entries(shell: &Shell, cx: &Context<Shell>) -> Vec<ListEntry> {
    let state = shell.state.read(cx);
    let selected = state.selected_project.clone();
    let hidden_mode = shell.hidden_open;
    let shows = |archived: bool| hidden_mode == archived;
    let filter = shell.list_filter;
    let mut rows: Vec<(RowKey, ListEntry)> = Vec::new();
    for s in state.servers.iter() {
        if !filter.admits_servers() {
            break;
        }
        let Some(hit) = search::server_hit(s, "") else {
            continue;
        };
        if !shows(s.archived_at.is_some()) {
            continue;
        }
        let selected = state.open_server.as_deref() == Some(s.id.as_str())
            || state
                .selected_channel
                .as_ref()
                .is_some_and(|c| c.server_id == s.id);
        let at = state.server_last_activity(s);
        rows.push((
            (hit.kind, std::cmp::Reverse(at)),
            ListEntry::Server(Box::new(ServerRowView {
                selected,
                unread: state.space_row(s).is_some_and(|r| r.unread > 0),
                unread_count: state.space_row(s).map_or(0, |r| r.unread as usize),
                preview: hit.reason.unwrap_or_else(|| server_preview(state, s)),
                time: time_label(at),
                server: s.clone(),
                pinned: false,
            })),
        ));
    }
    for p in state.projects.iter() {
        if !filter.admits_space(p) {
            continue;
        }
        let Some(hit) = state.space_search_hit(p, "") else {
            continue;
        };
        if !shows(p.archived_at.is_some()) {
            continue;
        }
        let streaming = state.project_is_streaming(&p.id);
        // Shared projects badge the disc with the other side; a group is one
        // plain disc (redesign 2026-10-20).
        let shown_people: Vec<_> = if p.is_people_group() {
            Vec::new()
        } else if p.is_guest() {
            p.owner().cloned().into_iter().collect()
        } else {
            p.guests().cloned().collect()
        };
        let row = RowView {
            selected: selected.as_deref() == Some(p.id.as_str()),
            streaming,
            unread: state.project_unread(&p.id) > 0,
            unread_count: state
                .visible_tabs(&p.id)
                .iter()
                .map(|c| c.missed_count())
                .sum(),
            typing: !state.typing_in(&p.id).is_empty(),
            preview: hit.reason.unwrap_or_else(|| preview(state, p)),
            time: time_label(last_activity(state, p)),
            person_image: p.person().and_then(|x| state.avatar(&x.avatar_url)),
            guests: shown_people
                .into_iter()
                .map(|person| {
                    let img = state.avatar(&person.avatar_url);
                    (person, img)
                })
                .collect(),
            space: p.clone(),
            pinned: false,
        };
        rows.push((
            (hit.kind, std::cmp::Reverse(last_activity(state, p))),
            ListEntry::Chat(Box::new(row)),
        ));
    }
    rows.sort_by_key(|(key, _)| *key);
    rows.into_iter().map(|(_, e)| e).collect()
}

/// A chat row's title: the group's title, the DM partner's display name, the
/// agent's name (matches iOS PeopleRow).
fn row_name(space: &Project) -> String {
    if space.is_people_group() {
        space.name.clone()
    } else if space.is_people() {
        space
            .person()
            .map(|p| p.display_name())
            .unwrap_or_else(|| space.name.clone())
    } else {
        space.name.clone()
    }
}

/// A chat row's picture at `size`: the initials disc for an agent or a group,
/// the person's photo for a DM (with a tiny agent badge when one of my agents
/// sits in it), shared guests stacked at the corner.
fn chat_picture(
    row: &RowView,
    shell: &Shell,
    size: f32,
    theme: &Theme,
    cx: &Context<Shell>,
) -> Div {
    let space = &row.space;
    if let Some(person) = space
        .person()
        .filter(|_| space.is_people() && !space.is_people_group())
    {
        let mut pic = div()
            .relative()
            .size(px(size))
            .flex_shrink_0()
            .child(people::avatar(
                row.person_image.clone(),
                person,
                size,
                theme,
            ));
        if let Some(agent) = space.agents.first() {
            // The agent's picture through the shared identity (#1592): the one its bubbles,
            // the presence row and the member list show — uploaded picture included.
            let state = shell.state.read(cx);
            let a = avatars::member_identity(Some(space), &state.projects, agent);
            let image = a.image_key.as_deref().and_then(|url| state.avatar(url));
            pic = pic.child(
                div()
                    .absolute()
                    .right(px(-3.0))
                    .bottom(px(-3.0))
                    .p(px(2.0))
                    .rounded_full()
                    .bg(theme.bg)
                    .child(avatars::agent_picture(&a, image, size / 2.0, theme)),
            );
        }
        return pic;
    }
    let mut pic = div()
        .relative()
        .size(px(size))
        .flex_shrink_0()
        .child(groups::picture(space, shell.state.read(cx), size, theme));
    if !row.guests.is_empty() {
        pic = pic.child(
            div()
                .absolute()
                .right(px(-4.0))
                .bottom(px(-2.0))
                .child(people::avatar_stack(&row.guests, 16.0, 2, theme)),
        );
    }
    pic
}

/// "#general · Sam: hi" for a server row: its newest channel's last line.
fn server_preview(state: &AppState, server: &ServerInfo) -> String {
    let newest = state
        .matrix_snapshot
        .channels
        .iter()
        .filter(|c| c.server_id == server.id && c.last_ts > 0)
        .max_by_key(|c| c.last_ts);
    match newest {
        Some(c) => format!("{} · {}", c.name, channel_last_line(state, c)),
        None => state
            .space_row(server)
            .map(|s| s.last_text.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "No messages yet".into()),
    }
}

/// "@sam: hi" for a channel row — a person by handle, as a group's `preview`
/// labels them, "You" for my own row, an agent by name; a system line bare.
/// The Matrix row's sender name is the handle for people
/// (`arbos_matrix::message_row`), so it is never shown as if it were a name
/// ([feedback server-member-names-2026-10-28]).
fn channel_last_line(state: &AppState, row: &arbos_matrix::ChannelRow) -> String {
    let line = plain(row.last_text.lines().next().unwrap_or_default());
    let who = channel_preview_who(
        &row.last_sender,
        &row.last_sender_name,
        &format!("user:{}", state.me_id()),
    );
    if who.is_empty() {
        line
    } else {
        format!("{who}: {line}")
    }
}

/// The label before a channel preview's text, from the newest row's member
/// string and Matrix display name; `me` is my own member string.
pub(crate) fn channel_preview_who(sender: &str, sender_name: &str, me: &str) -> String {
    if sender.eq_ignore_ascii_case(me) {
        return "You".into();
    }
    if sender_name.is_empty() || sender == "system" {
        return String::new();
    }
    if sender.starts_with("user:") {
        format!("@{sender_name}")
    } else {
        sender_name.to_owned()
    }
}

/// Sort key of a row: the kind of hit (always `Title` now that the list is
/// not text-filtered), newest activity first.
type RowKey = (search::HitKind, std::cmp::Reverse<u64>);

/// The chats list's top and bottom fade (Jacob likes it, Oct 23). The band
/// sits under the strip's bottom edge: the scroll box is pulled up by the band
/// and the list pads by the same amount, so at rest the first row starts where
/// the fade ends and only rows that scroll under the strip cross it (the pane
/// rule from #1365 / #1424). The bottom band stays over the list's end.
pub const LIST_FADE_BAND: f32 = 14.0;

/// The wrapper is a flex column: the list inside keeps `flex_1 / min_h_0`, so
/// it stays bounded and scrolls. (#1497's first try used a block div here —
/// the list took its content height and painted through the account row,
/// #1528; `row_bounds_tests` pins this.)
pub(crate) fn faded_list(list: impl IntoElement) -> gpui::AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .mt(px(-LIST_FADE_BAND))
        .debug_selector(|| "sidebar-list-frame".into())
        .child(edge_faded(LIST_FADE_BAND, true, true, list))
        .into_any_element()
}

/// The expanded chat list — today's layout, unchanged by the rail
/// ([feedback mac-sidebar-rail-2026-10-26]: "do not change the expanded
/// sidebar").
pub fn render(shell: &Shell, cx: &mut Context<Shell>) -> Div {
    render_with(shell, false, cx)
}

/// `hide_morphing`: while the collapse / open slide runs, the strip's discs,
/// the rows' pictures, the selected row's fill and my footer picture are drawn
/// by `ui::rail`'s overlay on their way between the two layouts — here they
/// keep their layout slot but are not painted (`invisible`), so nothing shows
/// twice. Their resting bounds are recorded into `Shell::morph` every frame
/// they are painted, which is what the overlay interpolates from.
pub(crate) fn render_with(shell: &Shell, hide_morphing: bool, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();
    let (pinned_rows, rest_rows) = list_sections(shell, cx);
    let pinned_count = pinned_rows.len();
    let rows: Vec<ListEntry> = pinned_rows.into_iter().chain(rest_rows).collect();
    let snap = shell.morph.clone();
    let state = shell.state.read(cx);
    let machine = state.machine.shown_state();
    let sidebar_width = state.sidebar_width;
    // Inside a server the left pane shows its channels (D88 rule 2).
    let open_server = state.open_server_info().cloned();
    let open_server_admin = open_server.as_ref().is_some_and(ServerInfo::can_admin);
    let waitlist_position = state.machine.waitlist_position;

    // Sidebar archive (Jacob 2026-10-23): archived spaces leave the list and
    // sit behind the "Hidden chats · N" footer; the command palette still
    // finds them. The list itself is never filtered by text any more
    // ([feedback search-palette-2026-10-24]): every row is a plain `Title`
    // hit, ordered by activity.
    let hidden_mode = shell.hidden_open;
    let hidden_count = state.archived_space_count();
    // The All · Agents · People · Servers segment scopes the list, like the
    // iPhone home ([feedback sidebar-strip-filter-2026-10-24]).
    let filter = shell.list_filter;
    let empty = state.projects.is_empty() && state.servers.is_empty();
    let link_connecting = state.link == crate::model::Link::Connecting;
    if machine == MachineState::Waking {
        PulseClock::request(cx);
    }

    // ----- top: three round buttons on the right — search · + · collapse ------
    // Three-pane rule: the strip holds exactly +, search, minimize. Collapse
    // lives here (not the chat header) and keeps the right end so folding the
    // pane is intuitive (#1491). When the rail is hidden the header still shows
    // an expand control so the list can come back.
    let show_plus = open_server.is_none() || open_server_admin;
    let controls = strip_controls(open_server.is_some(), show_plus);
    let round = |id: &'static str, control: StripControl, theme: &Theme| {
        let (glyph, size) = control.glyph();
        debug_assert_eq!(control.size(), crate::ui::theme::CONTROL);
        glass::strip_disc(id, glyph, size, theme)
            .when(hide_morphing, |el| el.opacity(0.0))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
    };
    // The disc's resting bounds for the rail morph (`ui::rail`).
    let recorded = |control: StripControl, disc: gpui::Stateful<Div>| {
        let snap = snap.clone();
        div()
            .flex_shrink_0()
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(b) = bounds.first() {
                    snap.borrow_mut().discs.insert(control, *b);
                }
            })
            .child(disc)
    };
    let collapse = |cx: &mut Context<Shell>| {
        round("sidebar-collapse", StripControl::Collapse, &theme)
            .debug_selector(|| "sidebar-collapse".into())
            .on_click(cx.listener(|this, _, window, cx| {
                this.menu_open = false;
                let width: f32 = window.viewport_size().width.into();
                this.state.update(cx, |s, cx| {
                    let narrow = s.window_is_narrow(width);
                    s.toggle_rail(narrow, cx)
                });
            }))
    };
    let plus = |cx: &mut Context<Shell>| {
        round("sidebar-new", StripControl::New, &theme)
            .debug_selector(|| "sidebar-new".into())
            .on_click({
                let in_server = open_server.as_ref().map(|s| s.id.clone());
                cx.listener(move |this, _, window, cx| {
                    this.menu_open = false;
                    // Inside a server the "+" makes a channel (D88 rule 7; owner/admin only —
                    // the button is not drawn for members).
                    if let Some(server_id) = in_server.clone() {
                        this.open_server_sheet(
                            crate::app::ServerSheet::NewChannel { server_id },
                            window,
                            cx,
                        );
                        return;
                    }
                    this.people_input.update(cx, |i, cx| {
                        i.clear(cx);
                        i.set_placeholder("Search people");
                    });
                    this.state.update(cx, |s, cx| s.open_new_chat(cx));
                    let handle = this.people_input.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                })
            })
    };
    // The search disc opens the command palette (⌘⇧F / ⌘K do the same).
    let search_button = |cx: &mut Context<Shell>| {
        round("sidebar-search-toggle", StripControl::Search, &theme)
            .debug_selector(|| "sidebar-search".into())
            .on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx)))
    };
    let mut strip = div()
        .h(px(top_strip(shell.fullscreen)))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(STRIP_GAP))
        .pl(px(TRAFFIC_LIGHTS_INSET + STRIP_INSET))
        .pr(px(STRIP_INSET))
        .when(open_server.is_some(), |el| el.child(div().flex_1()));
    for control in controls {
        strip = match control {
            StripControl::Search => strip.child(recorded(control, search_button(cx))),
            StripControl::New => strip.child(recorded(control, plus(cx))),
            StripControl::Collapse => strip.child(recorded(control, collapse(cx))),
        };
    }
    let strip = strip.window_drag();
    // ----- filter: All · Agents · People · Servers (the iPhone home's PaneTabs) --
    // Bare labels, the selected one on a capsule, centred; no track, no border.
    // Narrow column ([feedback sidebar-narrow-wrap-2026-10-27]): the capsules
    // scroll horizontally under an edge fade when they do not fit, like Grok
    // Bot's iPhone filter row — the first and last capsule carry `auto` side
    // margins, which centre the set while there is room and resolve to 0 (a
    // left-aligned, scrollable row) once there is not; the selected capsule
    // is scrolled into view (`Shell::filter_scroll`) once per selection /
    // width — not every frame, or a scroll back toward "All" is undone by the
    // next paint (`sidebar_fit::FilterPin`, [feedback
    // server-strip-scroll-2026-10-28]).
    let filter_count = ListFilter::ALL.len();
    if let Some(ix) = ListFilter::ALL.iter().position(|f| *f == filter) {
        let measured = shell.filter_scroll.bounds().size.width > px(0.0);
        if shell.filter_pin.take(ix, sidebar_width, measured) {
            shell.filter_scroll.scroll_to_item(ix);
        }
    }
    let filter_bar = div()
        .id("sidebar-filter")
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(FILTER_GAP))
        .pt(px(FILTER_TOP))
        .pb(px(FILTER_BOTTOM))
        .px(px(8.0))
        .overflow_x_scroll()
        .track_scroll(&shell.filter_scroll)
        .children(ListFilter::ALL.into_iter().enumerate().map(|(ix, f)| {
            let on = f == filter;
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "sidebar-filter-{}",
                    f.label().to_ascii_lowercase()
                ))))
                .h(px(FILTER_HEIGHT))
                .px(px(FILTER_PAD_X))
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .map(|mut el| {
                    if ix == 0 {
                        el.style().margin.left = Some(gpui::Length::Auto);
                    }
                    if ix + 1 == filter_count {
                        el.style().margin.right = Some(gpui::Length::Auto);
                    }
                    el
                })
                .rounded_full()
                .cursor_pointer()
                // On: the bubble fill, Grok's (§ Chat-list filter); off: the
                // highlight token's washes plus the label brightening.
                .map(|el| {
                    if on {
                        highlight::washed(el, theme.filter_on_fill(), &theme)
                    } else {
                        let text = theme.text;
                        highlight::button_with(el, &theme, move |s| s.text_color(text))
                    }
                })
                .text_size(px(TEXT_BODY))
                .font_weight(if on {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if on { theme.text } else { theme.text_muted })
                .child(f.label())
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.menu_open = false;
                    this.row_menu = None;
                    this.list_filter = f;
                    f.save();
                    cx.notify();
                }))
        }));
    // ----- rows -------------------------------------------------------------
    let mut list = div()
        .id("chat-list")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .px(px(8.0))
        .pt(px(LIST_FADE_BAND + 6.0))
        .pb(px(12.0))
        .overflow_y_scroll();
    // D89: the host that runs everyone's computer is full; this account is in line. Chats and
    // people work meanwhile; agents start once the machine is ready.
    if let Some(n) = waitlist_position {
        list = list.child(
            div()
                .id("sidebar-waitlist-card")
                .flex()
                .items_center()
                .gap(px(10.0))
                .mx(px(4.0))
                .mb(px(8.0))
                .p(px(12.0))
                .rounded(px(14.0))
                .bg(theme.bubble_assistant)
                .child(
                    div()
                        .size(px(32.0))
                        .rounded_full()
                        .bg(theme.surface_2)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(sf_symbols::glyph("hourglass", 14.0, theme.text)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(TEXT_BODY))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .child(format!("You're number {n} in line for a computer")),
                        )
                        .child(
                            div()
                                .text_size(px(TEXT_SMALL))
                                .text_color(theme.text_muted)
                                .child("We'll let you know when it's ready. Chats work now."),
                        ),
                ),
        );
    }
    if empty {
        let (title, line) = if link_connecting {
            ("Loading your agents…", "Connecting to the relay.")
        } else {
            (
                "No agents yet",
                "Press + to create one. The relay sets up its workspace on your machine.",
            )
        };
        list = list.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.0))
                .pt(px(120.0))
                .px(px(20.0))
                .text_center()
                .child(
                    div()
                        .text_size(px(TEXT_BODY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(TEXT_SMALL))
                        .text_color(theme.text_muted)
                        .child(line),
                ),
        );
    } else if rows.is_empty() && !hidden_mode {
        // Nothing under this segment — the iPhone's copy.
        list = list.child(
            div()
                .id("sidebar-filter-empty")
                .pt(px(40.0))
                .text_center()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(filter.empty_copy()),
        );
    }
    if hidden_mode {
        // "Hidden chats ‹ back": the hidden rows stand in for the list.
        list = list.child(
            div()
                .id("sidebar-hidden-head")
                .flex()
                .items_center()
                .justify_between()
                .h(px(28.0))
                .px(px(12.0))
                .mb(px(4.0))
                .text_size(px(TEXT_TINY))
                .text_color(theme.text_muted)
                .child("Hidden chats")
                .child(
                    div()
                        .id("sidebar-hidden-back")
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.text))
                        .child("‹ back")
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.hidden_open = false;
                            cx.notify();
                        })),
                ),
        );
        if rows.is_empty() {
            list = list.child(
                div()
                    .pt(px(40.0))
                    .text_center()
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_muted)
                    .child("Nothing hidden."),
            );
        }
    }
    // The Pinned section ([feedback sidebar-pinning-mac-2026-10-28]): a muted label, the
    // pinned rows in the user's order, a hairline, then the rest. Zero pins → nothing of
    // this is drawn and the list is exactly as before.
    if pinned_count > 0 {
        list = list.child(pinned_header(&theme));
    }
    for (index, entry) in rows.into_iter().enumerate() {
        if pinned_count > 0 && index == pinned_count {
            list = list.child(pinned_divider(&theme));
        }
        // The row's card bounds feed the rail morph (the selected card shrinks
        // into its 54 pt tile) and the pin drag (which slot the pointer is over).
        let snap_card = snap.clone();
        let card_id = entry.id().clone();
        let row = match entry {
            ListEntry::Chat(row) => {
                render_row(*row, shell, hide_morphing, &theme, cx).into_any_element()
            }
            ListEntry::Server(row) => {
                render_server_row(*row, shell, hide_morphing, &theme, cx).into_any_element()
            }
        };
        list = list.child(
            div()
                .flex_shrink_0()
                .on_children_prepainted(move |bounds, _, _| {
                    if let Some(b) = bounds.first() {
                        snap_card.borrow_mut().cards.insert(card_id.clone(), *b);
                    }
                })
                .child(row),
        );
    }
    if pinned_count > 0 {
        list = pin_drop_target(list, cx);
    }
    if !hidden_mode && hidden_count > 0 {
        // Grok Bot's footer: muted "Hidden chats · N ›" under the list.
        list = list.child(
            div()
                .id("sidebar-hidden-footer")
                .flex()
                .items_center()
                .justify_between()
                .mt(px(8.0))
                .h(px(32.0))
                .px(px(12.0))
                .rounded(px(PANEL_RADIUS - 4.0))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .cursor_pointer()
                .map(|el| highlight::row(el, false, Shape::Own, &theme))
                .child(format!("Hidden chats · {hidden_count}"))
                .child(sf_symbols::glyph("chevron.right", 12.0, theme.text_faint))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.hidden_open = true;
                    this.menu_open = false;
                    this.row_menu = None;
                    cx.notify();
                })),
        );
    }

    // ----- bottom: the footer — my avatar + name, and the update disc when one is due --
    let who = shell.state.read(cx).account.as_ref().map_or_else(
        || "Settings".to_string(),
        |a| {
            if a.name.trim().is_empty() {
                if a.login.is_empty() {
                    "Signed in".to_string()
                } else {
                    format!("@{}", a.login)
                }
            } else {
                a.name.clone()
            }
        },
    );
    // Grok Bot's footer avatar: 36 pt at x 13, 12 above the bottom edge
    // ([feedback footer-avatar-grok-size-2026-10-24]; the constants above).
    let snap_footer = snap.clone();
    let account_row = highlight::row(
        footer_row_frame().cursor_pointer(),
        false,
        Shape::Own,
        &theme,
    )
    .child(
        div()
            .flex_shrink_0()
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(b) = bounds.first() {
                    snap_footer.borrow_mut().footer = Some(*b);
                }
            })
            .child(
                account_avatar(shell, machine, &theme, cx)
                    .when(hide_morphing, |el| el.opacity(0.0)),
            ),
    )
    .child(
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_size(px(TEXT_SMALL + 1.0))
            .text_color(theme.text)
            .child(who),
    )
    // My avatar opens my profile and every setting in the right pane
    // (three-pane rule, mockup g); a second click folds it away.
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

    // Drag handle on the right edge: slimmer or wider, and past the snap
    // point the column collapses to the rail (`ui::sidebar_fit`).
    let resize = resize_handle("sidebar-resize", &theme, &shell.split_drag, cx);

    let column = div()
        .relative()
        .h_full()
        .w(px(sidebar_width))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .bg(theme.chrome())
        .border_r_1()
        .border_color(theme.border)
        .child(strip)
        // The filter belongs to the chat list, not to a server's channel list.
        .when(filter_shown(open_server.is_some()), |el| {
            el.child(edge_faded_sides(FILTER_FADE_BAND, filter_bar))
        })
        .child(match open_server.as_ref() {
            // Inside a server the left pane shows its channels (mockup h).
            Some(server) => faded_list(server_channels(server, shell, &theme, cx)),
            None => faded_list(list),
        });
    // Grok Bot's update disc at the footer's right end, only when an update is
    // ready for a click ([feedback footer-update-button-grok-2026-10-24]).
    let update_disc = shell.render_update_disc(cx);
    column
        .child(footer_frame(account_row, update_disc))
        .child(resize)
}

/// A server's picture: the leaf picked on its Create screen (D92 amendment),
/// else the D79 disc — initials from its name, the relay's gradient index.
pub fn server_picture(server: &ServerInfo, state: &AppState, size: f32, theme: &Theme) -> Div {
    // D92 amendment: an uploaded / generated picture wins over the leaf.
    if let Some(image) = server
        .image_url
        .as_deref()
        .and_then(|url| state.avatar(url))
    {
        return div()
            .size(px(size))
            .flex_shrink_0()
            .rounded_full()
            .overflow_hidden()
            .child(
                gpui::img(crate::ui::picture_fit::fit(&image, size))
                    .size(px(size))
                    .rounded_full()
                    .object_fit(gpui::ObjectFit::Cover),
            );
    }
    if avatars::leaf::is_neutral(server.avatar_leaf) {
        return avatars::neutral_element(&server.name, size, theme);
    }
    if let (true, Some(leaf), Some(color)) = (
        avatars::leaf_avatars_on(),
        server.avatar_leaf,
        server.avatar_leaf_color,
    ) {
        return avatars::leaf_hover(
            avatars::LeafPick { leaf, color },
            size,
            avatars::leaf_hover_key(&server.id, size),
        );
    }
    avatars::element(
        &server.id,
        &server.name,
        server.avatar_color_index_effective,
        size,
        theme,
    )
}

/// A server row's picture: [`server_picture`] on a rounded square instead of
/// the disc, so servers read apart from agents and people in the one list
/// (approved mock, [feedback sidebar-strip-filter-2026-10-24]).
pub fn server_row_picture(server: &ServerInfo, state: &AppState, size: f32, theme: &Theme) -> Div {
    server_picture(server, state, size, theme).rounded(px(size * SERVER_PICTURE_CORNER))
}

/// A server row (D88 rule 1): drawn like a group row — picture, name, time, last line,
/// unread dot. No badge, no count, no disclosure. The picture is a rounded square.
/// A pinned row can be dragged to another slot of the Pinned section: the drag's start
/// records the row, the list (`pin_drop_target`) follows the pointer and commits.
fn pin_draggable(el: gpui::Stateful<Div>, id: Id, cx: &mut Context<Shell>) -> gpui::Stateful<Div> {
    let weak = cx.weak_entity();
    el.on_drag(PinDrag { id }, move |drag, _, _, cx| {
        let id = drag.id.clone();
        weak.update(cx, |this, cx| this.begin_pin_drag(id, cx)).ok();
        cx.new(|_| PinDragGhost)
    })
}

fn render_server_row(
    row: ServerRowView,
    shell: &Shell,
    hide_morphing: bool,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> gpui::Stateful<Div> {
    let picture = recorded_picture(
        shell,
        &row.server.id,
        hide_morphing,
        ListEntry::Server(Box::new(ServerRowView {
            server: row.server.clone(),
            selected: row.selected,
            unread: row.unread,
            unread_count: row.unread_count,
            preview: String::new(),
            time: String::new(),
            pinned: false,
        }))
        .picture(shell, AVATAR_ROW, theme, cx),
    );
    let ServerRowView {
        server,
        selected,
        unread,
        unread_count,
        preview,
        time,
        pinned,
    } = row;
    let id: Id = server.id.clone();
    let menu_id: Id = server.id.clone();
    let ctrl_menu_id: Id = server.id.clone();
    let drag_id: Id = server.id.clone();
    let mark_id: Id = server.id.clone();
    let lifted = shell.pin_drag.as_ref().is_some_and(|d| d.id == server.id);
    div()
        .id(ElementId::Name(SharedString::from(format!(
            "row-server-{}",
            server.id
        ))))
        .flex()
        .items_center()
        .gap(px(identity::ROW_GAP))
        .px(px(12.0))
        .py(px(11.0))
        .rounded(px(PANEL_RADIUS - 4.0))
        .map(|el| {
            if lifted {
                highlight::lifted(el, theme)
            } else {
                highlight::row(el, selected && !hide_morphing, Shape::Own, theme)
            }
        })
        .when(pinned, |el| pin_draggable(el, drag_id, cx))
        .cursor_pointer()
        .on_click(
            cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                if is_control_click(event) {
                    return;
                }
                this.menu_open = false;
                this.row_menu = None;
                this.chat_menu = None;
                this.state
                    .update(cx, |s, cx| s.enter_server(id.clone(), cx));
                Composer::focus(&this.composer, window, cx);
            }),
        )
        // Right-click / control-click: Archive (2026-10-23) — the server's own
        // page holds Leave and Delete.
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
        )
        .child(picture)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.0))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(6.0))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(TEXT_BODY))
                                .font_weight(if unread {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(theme.text)
                                .child(server.name.clone()),
                        )
                        .child(div().flex_1())
                        .when(pinned, |el| el.child(pin_mark(&mark_id, theme)))
                        .when_some(row_time_size(shell, cx), |el, size| {
                            el.child(time_el(time, size, theme))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(TEXT_SMALL))
                                .text_color(theme.text_muted)
                                .child(preview),
                        )
                        // Unread = the count pill, never a dot (Jacob, 2026-10-27).
                        .when(unread, |el| {
                            el.child(crate::ui::marks::unread_pill(unread_count, theme))
                        }),
                ),
        )
}

/// The server's channel list (D88 rule 2): `‹` back to chats, the server's disc and name
/// (click → its page in the right pane, rule 5), then one row per channel drawn like a chat
/// row with `#` in the avatar slot and a small lock on restricted ones.
fn server_channels(
    server: &ServerInfo,
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> gpui::Stateful<Div> {
    let state = shell.state.read(cx);
    let page = crate::app::ProfilePage::Server {
        server_id: server.id.clone(),
    };
    let selected_channel = state
        .selected_channel
        .as_ref()
        .filter(|c| c.server_id == server.id)
        .map(|c| c.channel_id.clone());
    let channels: Vec<(
        crate::model::ServerChannel,
        Option<arbos_matrix::ChannelRow>,
    )> = server
        .ordered_channels()
        .into_iter()
        .map(|c| (c.clone(), state.channel_row(c).cloned()))
        .collect();

    let back = div()
        .id("server-back")
        .flex()
        .items_center()
        .gap(px(10.0))
        .px(px(8.0))
        .pt(px(4.0))
        .pb(px(10.0))
        .child(
            glass::circle("server-back-circle", 30.0, theme)
                .child(sf_symbols::glyph("chevron.left", 14.0, theme.text))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.close_server_pane(cx));
                })),
        )
        .child(
            div()
                .id("server-title")
                .flex()
                .items_center()
                .gap(px(10.0))
                .min_w_0()
                .flex_1()
                .cursor_pointer()
                .child(server_picture(server, state, 26.0, theme))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(TEXT_BODY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(server.name.clone()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.menu_open = false;
                    this.state
                        .update(cx, |s, cx| s.toggle_profile(page.clone(), cx));
                })),
        );
    let mut list = div()
        .id("channel-list")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .px(px(8.0))
        .pt(px(LIST_FADE_BAND + 6.0))
        .pb(px(12.0))
        .overflow_y_scroll()
        .child(back);
    if channels.is_empty() {
        list = list.child(
            div()
                .pt(px(40.0))
                .text_center()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child("No channels yet."),
        );
    }
    for (ch, room) in channels {
        let selected = selected_channel.as_deref() == Some(ch.id.as_str());
        let unread = !selected && room.as_ref().is_some_and(|r| r.unread > 0);
        let preview = room
            .as_ref()
            .filter(|r| r.last_ts > 0)
            .map(|r| channel_last_line(state, r))
            .unwrap_or_else(|| "No messages yet".into());
        let time = room
            .as_ref()
            .filter(|r| r.last_ts > 0)
            .map(|r| time_label(r.last_ts as u64))
            .unwrap_or_default();
        let locked = ch.restricted || room.as_ref().is_some_and(|r| r.restricted);
        let server_id = server.id.clone();
        let channel_id = ch.id.clone();
        let (menu_sid, menu_cid) = (server.id.clone(), ch.id.clone());
        let (ctrl_sid, ctrl_cid) = (server.id.clone(), ch.id.clone());
        list = list.child(
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "channel-{}",
                    ch.id
                ))))
                .flex()
                .items_center()
                .gap(px(identity::ROW_GAP))
                .px(px(12.0))
                .py(px(11.0))
                .rounded(px(PANEL_RADIUS - 4.0))
                .map(|el| highlight::row(el, selected, Shape::Own, theme))
                .cursor_pointer()
                .child(
                    div()
                        .size(px(AVATAR_ROW))
                        .flex_shrink_0()
                        .rounded(px(10.0))
                        .bg(theme.surface_2)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(sf_symbols::glyph("number", 18.0, theme.text_muted)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.0))
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(TEXT_BODY))
                                        .font_weight(if unread {
                                            FontWeight::SEMIBOLD
                                        } else {
                                            FontWeight::MEDIUM
                                        })
                                        .text_color(theme.text)
                                        .child(ch.name.clone()),
                                )
                                .when(locked, |el| {
                                    el.child(sf_symbols::glyph("lock", 11.0, theme.text_faint))
                                })
                                .child(div().flex_1())
                                .when_some(row_time_size(shell, cx), |el, size| {
                                    el.child(time_el(time, size, theme))
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(TEXT_SMALL))
                                        .text_color(theme.text_muted)
                                        .child(preview),
                                )
                                .when(unread, |el| {
                                    el.child(crate::ui::marks::unread_pill(
                                        room.as_ref().map_or(1, |r| r.unread as usize),
                                        theme,
                                    ))
                                }),
                        ),
                )
                .on_click(
                    cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                        // Control-click opened the menu on mouse-down; the click stays swallowed.
                        if is_control_click(event) {
                            return;
                        }
                        this.menu_open = false;
                        this.channel_menu = None;
                        this.state.update(cx, |s, cx| {
                            s.select_channel(server_id.clone(), channel_id.clone(), cx)
                        });
                        Composer::focus(&this.composer, window, cx);
                    }),
                )
                // Right-click or control-click: the row's menu (Archive; Delete for admins),
                // the sidebar row menu's pattern (#1257/#1258).
                .on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_channel_menu(
                            menu_sid.clone(),
                            menu_cid.clone(),
                            event.position,
                            cx,
                        );
                    }),
                )
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                        if !event.modifiers.control {
                            return;
                        }
                        cx.stop_propagation();
                        this.open_channel_menu(
                            ctrl_sid.clone(),
                            ctrl_cid.clone(),
                            event.position,
                            cx,
                        );
                    }),
                ),
        );
    }
    list
}

/// The right-click menu of a channel row (D88): **Archive** (or Unarchive) for every member —
/// hides the channel from my list, the server's Channels tab brings it back — and, for owners
/// and admins, a divider and **Delete** (never on `#general`), which first asks "Delete #name?".
pub fn channel_menu(shell: &Shell, cx: &mut Context<Shell>) -> Option<Div> {
    let (server_id, channel_id, at) = shell.channel_menu.clone()?;
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let server = state.server(&server_id)?;
    let channel = server.channel(&channel_id)?.clone();
    let is_general = server.general().is_some_and(|g| g.id == channel.id);
    let can_delete = server.can_admin() && !is_general;
    let archived = channel.archived;

    if shell.channel_delete_armed && can_delete {
        let (sid, cid) = (server_id.clone(), channel_id.clone());
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(8.0))
            .child(
                div()
                    .text_size(px(TEXT_SMALL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(format!("Delete #{}?", channel.name)),
            )
            .child(
                div()
                    .text_size(px(TEXT_TINY))
                    .text_color(theme.text_muted)
                    .child("Deletes the channel and its messages for everyone in the server."),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.0))
                    .child(
                        confirm_button("channel-menu-delete-cancel", "Cancel", false, &theme)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.channel_menu = None;
                                this.channel_delete_armed = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        confirm_button("channel-menu-delete-go", "Delete", true, &theme).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.channel_menu = None;
                                this.channel_delete_armed = false;
                                let (sid, cid) = (sid.clone(), cid.clone());
                                this.state
                                    .update(cx, |s, cx| s.delete_channel(sid, cid, cx));
                            }),
                        ),
                    ),
            );
        return Some(
            div()
                .absolute()
                .left(at.x)
                .top(at.y)
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(glass::frosted_menu(260.0, &theme, body)),
        );
    }

    let (sid, cid) = (server_id.clone(), channel_id.clone());
    let mut body = div().flex().flex_col().gap(px(1.0)).child(
        glass::menu_row(
            "channel-menu-archive",
            if archived {
                "tray.and.arrow.up"
            } else {
                "archivebox"
            },
            if archived { "Unarchive" } else { "Archive" },
            false,
            &theme,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.channel_menu = None;
            let (sid, cid) = (sid.clone(), cid.clone());
            this.state
                .update(cx, |s, cx| s.archive_channel(sid, cid, !archived, cx));
        })),
    );
    if can_delete {
        body = body.child(glass::menu_divider(&theme)).child(
            glass::menu_row("channel-menu-delete", "trash", "Delete", true, &theme).on_click(
                cx.listener(|this, _, _, cx| {
                    this.channel_delete_armed = true;
                    cx.notify();
                }),
            ),
        );
    }
    Some(
        div()
            .absolute()
            .left(at.x)
            .top(at.y)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(glass::frosted_menu(200.0, &theme, body)),
    )
}

/// A row's picture wrapped for the rail morph: its resting bounds are recorded
/// under the row's id every frame it is painted; while the morph runs the
/// overlay paints it instead (`invisible` keeps the slot).
/// The rows' time size at the column's width — `None` hides it: the name
/// wins as the column narrows (`ui::sidebar_fit::row_time`,
/// [feedback sidebar-narrow-wrap-2026-10-27]).
fn row_time_size(shell: &Shell, cx: &Context<Shell>) -> Option<f32> {
    match sidebar_fit::row_time(shell.state.read(cx).sidebar_width) {
        sidebar_fit::RowTime::Full => Some(TEXT_TINY),
        sidebar_fit::RowTime::Small => Some(sidebar_fit::TIME_SMALL_SIZE),
        sidebar_fit::RowTime::Hidden => None,
    }
}

fn time_el(time: String, size: f32, theme: &Theme) -> Div {
    div()
        .flex_shrink_0()
        .text_size(px(size))
        .text_color(theme.text_muted)
        .debug_selector(|| "row-time".into())
        .child(time)
}

fn recorded_picture(shell: &Shell, id: &Id, hide_morphing: bool, picture: Div) -> Div {
    let snap = shell.morph.clone();
    let id = id.clone();
    div()
        .flex_shrink_0()
        .on_children_prepainted(move |bounds, _, _| {
            if let Some(b) = bounds.first() {
                snap.borrow_mut().rows.insert(id.clone(), *b);
            }
        })
        .child(picture.when(hide_morphing, |el| el.opacity(0.0)))
}

fn render_row(
    row: RowView,
    shell: &Shell,
    hide_morphing: bool,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    // The picture: the initials disc for an agent or a group, the person's
    // photo for a DM (with a tiny agent badge when one of my agents sits in it).
    let picture = recorded_picture(
        shell,
        &row.space.id,
        hide_morphing,
        chat_picture(&row, shell, AVATAR_ROW, theme, cx),
    );
    let RowView {
        space,
        selected,
        streaming,
        unread,
        unread_count,
        typing,
        preview,
        time,
        person_image: _,
        guests: _,
        pinned,
    } = row;
    let id: Id = space.id.clone();
    let menu_id: Id = space.id.clone();
    let ctrl_menu_id: Id = space.id.clone();
    let drag_id: Id = space.id.clone();
    let lifted = shell.pin_drag.as_ref().is_some_and(|d| d.id == space.id);
    let can_rename = !space.is_guest() && !space.is_people();
    let renaming = shell
        .state
        .read(cx)
        .renaming_project
        .as_ref()
        .is_some_and(|p| p == &space.id);
    // No pulse-clock request from a row any more: the typing pulse is gone
    // (never a coloured dot — Jacob, 2026-10-27; "typing…" is the preview), and
    // the old working-row request made the window redraw at 30 fps for nothing.
    // The spinner has its own shared 11 fps ticker (`ui::working`).

    // Groups: the chat title (custom or member handles from the relay),
    // matching iOS PeopleRow. DMs: the other person's display name.
    let name = row_name(&space);
    // The name wins over the time as the column narrows: the time shrinks,
    // then hides, and the `@handle` goes first (`ui::sidebar_fit::row_time`,
    // [feedback sidebar-narrow-wrap-2026-10-27]).
    let handle_shown = sidebar_fit::handle_shown(shell.state.read(cx).sidebar_width);
    // A group row reads like an agent row: name and preview, no member count.
    let handle = space
        .person()
        .filter(|_| space.is_people() && !space.is_people_group())
        .filter(|_| handle_shown)
        .map(|p| format!("@{}", p.handle));
    let preview = if typing {
        "typing…".to_string()
    } else {
        preview
    };

    div()
        .id(ElementId::Name(SharedString::from(format!(
            "row-{}",
            space.id
        ))))
        .flex()
        .items_center()
        .gap(px(identity::ROW_GAP))
        .px(px(12.0))
        .py(px(11.0))
        .rounded(px(PANEL_RADIUS - 4.0))
        .map(|el| {
            if lifted {
                highlight::lifted(el, theme)
            } else {
                highlight::row(el, selected && !hide_morphing, Shape::Own, theme)
            }
        })
        .when(pinned, |el| pin_draggable(el, drag_id, cx))
        .cursor_pointer()
        .on_click({
            let id = id.clone();
            cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                // Control-click is the secondary click: the mouse-down below
                // opened the row menu, and GPUI still delivers the click on
                // mouse-up — swallow it so the menu stays open.
                if is_control_click(event) {
                    return;
                }
                this.menu_open = false;
                this.row_menu = None;
                this.chat_menu = None;
                if can_rename && event.click_count() >= 2 {
                    this.state
                        .update(cx, |s, cx| s.select_project(id.clone(), cx));
                    this.start_rename_agent(id.clone(), window, cx);
                    return;
                }
                let q = this.search_input.read(cx).text().to_string();
                this.state.update(cx, |s, cx| {
                    if q.trim().is_empty() {
                        s.select_project(id.clone(), cx);
                    } else {
                        s.open_from_home_search(id.clone(), &q, cx);
                    }
                });
                Composer::focus(&this.composer, window, cx);
            })
        })
        // Grok Mac: right-click (or control-click) a row for its menu (ref 48).
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
        )
        .child(picture)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.0))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(6.0))
                        .child(if renaming {
                            div()
                                .id("sidebar-rename")
                                .min_w_0()
                                .flex_1()
                                .h(px(22.0))
                                .px(px(6.0))
                                .rounded(px(6.0))
                                .bg(theme.field)
                                .border_1()
                                .border_color(theme.field_border)
                                .text_size(px(TEXT_BODY))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .child(shell.agent_name_input.clone())
                                .into_any_element()
                        } else {
                            identity::name(name, TEXT_BODY)
                                .min_w_0()
                                .font_weight(if unread || streaming {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(if space.pending {
                                    theme.text_muted
                                } else {
                                    theme.text
                                })
                                .into_any_element()
                        })
                        .when_some(handle, |el, h| {
                            let handle_id = id.clone();
                            el.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(TEXT_TINY))
                                    .text_color(theme.text_muted)
                                    .debug_selector(move || format!("row-handle-{handle_id}"))
                                    .child(h),
                            )
                        })
                        .child(div().flex_1())
                        .when(pinned, |el| el.child(pin_mark(&id, theme)))
                        .when_some(row_time_size(shell, cx), |el, size| {
                            el.child(
                                time_el(time, size, theme)
                                    .debug_selector(|| format!("row-time-{id}")),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .debug_selector(|| format!("row-preview-line-{id}"))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(TEXT_SMALL))
                                .text_color(theme.text_muted)
                                .child(preview),
                        )
                        // The status cell under the time (Jacob, 2026-10-27): the
                        // braille spinner while the chat's agent works (one shared
                        // ticker); the unread count pill; nothing for a person typing
                        // (the preview says "typing…") or a quiet read chat. Never a
                        // coloured dot.
                        .children(row_status_cell(
                            RowStatus::of(streaming, typing, unread),
                            unread_count,
                            &id,
                            theme,
                            cx,
                        )),
                ),
        )
}

/// What the status cell under a row's time shows (Jacob, 2026-10-27,
/// [feedback working-braille-spinner-mac-2026-10-27] + [feedback
/// neon-dots-mac-2026-10-27]). One cell, one mark, never a coloured dot: a
/// person typing shows nothing in the cell (the spinner is the *agent's*
/// working sign, never a human's; the preview reads "typing…"); the spinner
/// while the chat's agent works; the unread **count pill** otherwise; a quiet
/// read chat shows nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowStatus {
    /// A person is typing: no mark in the cell (the preview carries it).
    Typing,
    /// The chat's agent is working: the braille spinner.
    Working,
    /// Unread messages: the count pill.
    Unread,
    /// Nothing.
    Quiet,
}

impl RowStatus {
    pub fn of(streaming: bool, typing: bool, unread: bool) -> Self {
        // Typing draws nothing, so it never hides the spinner or the pill.
        if streaming {
            RowStatus::Working
        } else if unread {
            RowStatus::Unread
        } else if typing {
            RowStatus::Typing
        } else {
            RowStatus::Quiet
        }
    }
}

fn row_status_cell(
    status: RowStatus,
    unread_count: usize,
    id: &Id,
    theme: &Theme,
    cx: &mut App,
) -> Option<AnyElement> {
    match status {
        RowStatus::Typing | RowStatus::Quiet => None,
        RowStatus::Working => Some(
            crate::ui::working::glyph(theme, cx)
                .debug_selector(|| format!("row-status-working-{id}"))
                .into_any_element(),
        ),
        RowStatus::Unread => Some(
            crate::ui::marks::unread_pill(unread_count, theme)
                .debug_selector(|| format!("row-status-unread-{id}"))
                .into_any_element(),
        ),
    }
}

/// The right-click menu of a sidebar row, anchored at the click. An agent
/// row (Jacob, #1258): Mute, Fork, a divider, Delete — nothing else; its
/// settings live on the agent pane. A people row: Profile, Mute, Delete; a
/// group row: Delete group.
/// What the chat-list row menu's Delete does — at once, with no "Delete …?"
/// step (Jacob, feedback remove-delete-confirms-2026-10-24; the guard is who
/// sees the item, not a second click).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowDelete {
    /// An agent and its chats leave your list.
    Agent,
    /// A DM leaves your list; a group you own is deleted for everyone.
    PeopleChat,
    /// A group you do not own: you leave it, it stays for the others.
    LeaveGroup,
}

pub fn row_delete(space: &crate::model::Project, me_handle: &str) -> RowDelete {
    if space.is_people_group() && !space.owner_handle.as_deref().is_none_or(|o| o == me_handle) {
        RowDelete::LeaveGroup
    } else if space.is_people() {
        RowDelete::PeopleChat
    } else {
        RowDelete::Agent
    }
}

fn run_row_delete(
    state: &mut crate::app::AppState,
    action: RowDelete,
    id: crate::model::Id,
    cx: &mut Context<crate::app::AppState>,
) {
    match action {
        RowDelete::Agent => state.delete_project(id, cx),
        RowDelete::PeopleChat => state.delete_people_chat(id, cx),
        RowDelete::LeaveGroup => state.leave_group(id, cx),
    }
}

pub fn row_menu(shell: &Shell, cx: &mut Context<Shell>) -> Option<Div> {
    let (space_id, at) = shell.row_menu.clone()?;
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let archived = state.is_space_archived(&space_id);
    // Pin / Unpin leads every row's menu ([feedback sidebar-pinning-mac-2026-10-28]):
    // SF `pin` / `pin.slash`, the iPhone's names. At the cap the click says so.
    let pinned = state.is_pinned(&space_id);
    let pin_row = |theme: &Theme, cx: &mut Context<Shell>| {
        let id = space_id.clone();
        glass::menu_row(
            "row-menu-pin",
            if pinned { "pin.slash" } else { "pin" },
            if pinned { "Unpin" } else { "Pin" },
            false,
            theme,
        )
        .debug_selector(|| "row-menu-pin".into())
        .on_click(cx.listener(move |this, _, _, cx| {
            this.row_menu = None;
            let id = id.clone();
            this.state.update(cx, |s, cx| s.set_pinned(id, !pinned, cx));
        }))
    };
    let archive_row = |theme: &Theme, cx: &mut Context<Shell>| {
        let id = space_id.clone();
        glass::menu_row(
            "row-menu-archive",
            if archived {
                "tray.and.arrow.up"
            } else {
                "archivebox"
            },
            if archived { "Unarchive" } else { "Archive" },
            false,
            theme,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.row_menu = None;
            let id = id.clone();
            this.state
                .update(cx, |s, cx| s.set_space_archived(id, !archived, cx));
        }))
    };
    // A server row (D88): Archive / Unarchive only — Leave and Delete live on its page.
    if state.servers.iter().any(|s| s.id == space_id) {
        let body = div()
            .flex()
            .flex_col()
            .gap(px(1.0))
            .child(pin_row(&theme, cx))
            .child(archive_row(&theme, cx));
        return Some(
            div()
                .absolute()
                .left(at.x)
                .top(at.y)
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(glass::frosted_menu(200.0, &theme, body)),
        );
    }
    let space = state.projects.iter().find(|p| p.id == space_id)?.clone();
    let muted = state.is_muted(&space.id);
    let is_people = space.is_people();
    let can_fork = !is_people && !space.shared;
    let can_delete = is_people || !space.shared;
    let person = space.person().cloned();

    let profile_id = space.id.clone();
    let profile_page = if is_people {
        match person {
            Some(p) if !space.is_people_group() => crate::app::ProfilePage::User {
                handle: p.handle.clone(),
                space_id: Some(space.id.clone()),
            },
            _ => crate::app::ProfilePage::Group {
                space_id: space.id.clone(),
            },
        }
    } else if space.shared {
        crate::app::ProfilePage::Group {
            space_id: space.id.clone(),
        }
    } else {
        crate::app::ProfilePage::Agent {
            project_id: space.id.clone(),
        }
    };
    let mute_id = space.id.clone();
    let fork_id = space.id.clone();
    let delete_id = space.id.clone();
    // A group (redesign 2026-10-20): one item, "Delete group". The owner
    // deletes it for everyone; a member leaves and the group stays.
    let is_group = space.is_people_group();
    let delete_action = row_delete(&space, &state.me_handle());

    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(1.0))
        .child(pin_row(&theme, cx));
    if is_group {
        // Mockup f: the group row's menu is "Delete group" — plus Archive (2026-10-23).
        body = body
            .child(archive_row(&theme, cx))
            .child(glass::menu_divider(&theme));
        // Jacob (feedback remove-delete-confirms-2026-10-24): no "Delete …?"
        // step — the owner deletes the group for everyone at once, a member
        // leaves it at once.
        let group_id = delete_id.clone();
        body = body.child(
            glass::menu_row("row-menu-delete", "trash", "Delete group", true, &theme).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.row_menu = None;
                    let id = group_id.clone();
                    this.state
                        .update(cx, |s, cx| run_row_delete(s, delete_action, id, cx));
                }),
            ),
        );
        return Some(
            div()
                .absolute()
                .left(at.x)
                .top(at.y)
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(glass::frosted_menu(200.0, &theme, body)),
        );
    }
    // Jacob (#1258): an agent row's menu is Mute, Fork, a divider, Delete —
    // nothing else. The profile is the header pill, New chat is the pill's
    // "+", Model / Reasoning / Share / Add to this chat / Voice mode are rows
    // on the agent pane. A people row keeps Profile.
    if is_people {
        body = body.child(
            glass::menu_row(
                "row-menu-profile",
                "person.crop.circle",
                "Profile",
                false,
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.row_menu = None;
                let (page, id) = (profile_page.clone(), profile_id.clone());
                this.state.update(cx, |s, cx| {
                    s.select_project(id, cx);
                    s.open_profile(page, cx);
                });
            })),
        );
    }
    body = body.child(
        glass::menu_row(
            "row-menu-mute",
            if muted { "bell" } else { "bell.slash" },
            if muted { "Unmute" } else { "Mute" },
            false,
            &theme,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.row_menu = None;
            let id = mute_id.clone();
            this.state.update(cx, |s, cx| s.toggle_mute(id, cx));
        })),
    );
    if can_fork {
        body = body.child(
            glass::menu_row(
                "row-menu-fork",
                "arrow.triangle.branch",
                "Fork",
                false,
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.row_menu = None;
                let id = fork_id.clone();
                this.state.update(cx, |s, cx| s.duplicate_project(id, cx));
            })),
        );
    }
    body = body.child(archive_row(&theme, cx));
    if can_delete {
        body = body.child(glass::menu_divider(&theme));
        // Deletes at once (Jacob, feedback remove-delete-confirms-2026-10-24):
        // a DM leaves the list, an agent goes with its chats; no "Delete …?" step.
        body = body.child(
            glass::menu_row("row-menu-delete", "trash", "Delete", true, &theme).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.row_menu = None;
                    let id = delete_id.clone();
                    this.state
                        .update(cx, |s, cx| run_row_delete(s, delete_action, id, cx));
                }),
            ),
        );
    }
    // Appended straight to the root: taffy places an absolute child relative
    // to its parent, so a static wrapper would push the menu off to the right.
    Some(
        div()
            .absolute()
            .left(at.x)
            .top(at.y)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(glass::frosted_menu(200.0, &theme, body)),
    )
}

/// A left click whose mouse-down carried Control (macOS secondary click).
pub fn is_control_click(event: &gpui::ClickEvent) -> bool {
    matches!(event, gpui::ClickEvent::Mouse(e) if e.down.modifiers.control)
}

/// Cancel / Delete pill for the row menu's delete confirmation.
pub(crate) fn confirm_button(
    id: &'static str,
    label: &'static str,
    danger: bool,
    theme: &Theme,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(28.0))
        .px(px(12.0))
        .rounded_full()
        .flex()
        .items_center()
        .text_size(px(TEXT_SMALL))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .when(danger, |el| {
            el.bg(theme.danger)
                .text_color(gpui::white())
                .hover(|s| s.opacity(0.88))
        })
        .when(!danger, |el| {
            highlight::washed(
                el.border_1()
                    .border_color(theme.field_border)
                    .text_color(theme.text),
                theme.field,
                theme,
            )
        })
        .child(label)
}

/// Width of the session row's context menu (`chat_menu`).
pub(crate) const CHAT_MENU_WIDTH: f32 = 180.0;

/// Right-click / control-click menu of a session row in the header's tab
/// dropdown (#1256): Rename, Fork and Delete (archive, or permanently close
/// when already archived). The caller anchors it to the row; the dropdown
/// stays open behind it with the row highlighted. Delete first swaps the
/// menu for `Delete "title"?` with Cancel / Delete, so the session being
/// removed is always named.
pub fn chat_menu(
    shell: &Shell,
    chat_id: &str,
    cx: &mut Context<Shell>,
) -> Option<gpui::AnyElement> {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let chat = state
        .chats
        .values()
        .flatten()
        .find(|c| c.id == chat_id)?
        .clone();
    let project = state.project(&chat.project_id)?.clone();
    let archived = state.archived_chats.contains(&chat.id);
    // Session fork (#1257): this chat again, inside the same agent. The
    // agent-level fork lives on the sidebar's agent row only.
    let can_fork =
        state.multi_chat && crate::app::forked_chat(&chat, &project, String::new()).is_some();
    let can_delete = !chat.is_pinned;
    let fork_id = chat.id.clone();
    let delete_id = chat.id.clone();
    let delete_archived = archived;
    let title = crate::ui::header::tab_title(&chat, &project);

    if shell.chat_delete_armed && can_delete {
        let note = if archived {
            "Removes this chat from Previous chats for good."
        } else {
            "Closes this chat. It moves to Previous chats."
        };
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(8.0))
            .child(
                div()
                    .text_size(px(TEXT_SMALL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(format!("Delete “{title}”?")),
            )
            .child(
                div()
                    .text_size(px(TEXT_TINY))
                    .text_color(theme.text_muted)
                    .child(note),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.0))
                    .child(
                        confirm_button("chat-menu-delete-cancel", "Cancel", false, &theme)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.chat_menu = None;
                                this.chat_delete_armed = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        confirm_button("chat-menu-delete-go", "Delete", true, &theme).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.chat_menu = None;
                                this.chat_delete_armed = false;
                                let id = delete_id.clone();
                                this.state.update(cx, |s, cx| {
                                    if delete_archived {
                                        s.delete_chat_tab(id, cx);
                                    } else {
                                        s.close_tab(id, cx);
                                    }
                                });
                            }),
                        ),
                    ),
            );
        return Some(glass::frosted_menu(240.0, &theme, body).into_any_element());
    }

    let mut body = div().flex().flex_col().gap(px(1.0));
    if !archived && !project.is_people() {
        // Rename: the inline field in the tab dropdown (also double-click there).
        let rename_id = chat.id.clone();
        let rename_label = title.clone();
        body = body.child(
            glass::menu_row("chat-menu-rename", "pencil", "Rename", false, &theme).on_click(
                cx.listener(move |this, _, window, cx| {
                    this.chat_menu = None;
                    this.tab_menu_open = true;
                    this.begin_tab_rename(rename_id.clone(), rename_label.clone(), window, cx);
                }),
            ),
        );
    }
    if can_fork {
        body = body.child(
            glass::menu_row(
                "chat-menu-fork",
                "arrow.triangle.branch",
                "Fork",
                false,
                &theme,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.chat_menu = None;
                this.tab_menu_open = false;
                let id = fork_id.clone();
                this.state.update(cx, |s, cx| s.fork_chat(id, cx));
                Composer::focus(&this.composer, window, cx);
            })),
        );
    }
    if can_delete {
        if can_fork || !archived {
            body = body.child(glass::menu_divider(&theme));
        }
        body = body.child(
            glass::menu_row("chat-menu-delete", "trash", "Delete", true, &theme).on_click(
                cx.listener(|this, _, _, cx| {
                    this.chat_delete_armed = true;
                    cx.notify();
                }),
            ),
        );
    }
    if !can_fork && !can_delete {
        return None;
    }
    Some(glass::frosted_menu(CHAT_MENU_WIDTH, &theme, body).into_any_element())
}

#[cfg(test)]
mod row_menu_tests {
    use super::{row_delete, RowDelete};
    use crate::model::{Project, SpaceKind};

    fn space(kind: SpaceKind, owner: Option<&str>) -> Project {
        Project {
            id: "s1".into(),
            name: "Agrippa".into(),
            kind,
            owner_handle: owner.map(str::to_owned),
            ..Default::default()
        }
    }

    /// Jacob (feedback remove-delete-confirms-2026-10-24): the row menu's
    /// Delete maps straight to one action — there is no armed / confirm state
    /// between the click and the delete.
    #[test]
    fn delete_fires_at_once() {
        assert_eq!(
            row_delete(&space(SpaceKind::Project, None), "me"),
            RowDelete::Agent
        );
        assert_eq!(
            row_delete(&space(SpaceKind::Dm, None), "me"),
            RowDelete::PeopleChat
        );
        assert_eq!(
            row_delete(&space(SpaceKind::Group, Some("me")), "me"),
            RowDelete::PeopleChat,
            "the owner deletes the group for everyone"
        );
        assert_eq!(
            row_delete(&space(SpaceKind::Group, Some("sam")), "me"),
            RowDelete::LeaveGroup,
            "a member leaves instead"
        );
    }
}

#[cfg(test)]
mod strip_tests {
    use gpui::{div, prelude::*, px, TestAppContext};

    use super::{
        strip_controls, top_strip, ListFilter, StripControl, FILTER_HEIGHT, FILTER_TOP,
        STRIP_BUTTON, STRIP_GAP, STRIP_INSET,
    };
    use crate::model::{Project, SpaceKind};
    use crate::ui::shell::TRAFFIC_LIGHTS_INSET;
    use crate::ui::theme::{
        CONTROL, CONTROL_GAP, CONTROL_GLYPH, CONTROL_GLYPH_WIDE, CONTROL_LINE, HEADER_HEIGHT,
        TRAFFIC_LIGHT_LINE, TRAFFIC_LIGHT_POSITION,
    };

    /// Grok Bot's strip, measured on `ref-grok-round-search-plus.png` (project
    /// store, 2 px/pt, window top at image row 2) — Jacob 2026-10-24: "the
    /// top bar on mac is now too high. Make it identical to the size and
    /// positioning on grokbot" ([feedback topbar-grok-parity-2026-10-24]).
    /// Points from the window's top edge.
    mod grok {
        /// Red light: x 30–57, y 32–59 px → 14 pt disc, left edge 15, centre 22.
        pub const LIGHT_LEFT: f32 = 15.0;
        pub const LIGHT_LINE: f32 = 22.0;
        /// Yellow centre x 90 px, green 136 px → 23 pt pitch.
        pub const LIGHT_PITCH: f32 = 23.0;
        /// Discs: rows 24–96 px → 36 pt, top 12, centre 30, bottom 48.
        pub const DISC_TOP: f32 = 12.0;
        pub const DISC_LINE: f32 = 30.0;
        /// First chat card at row 130 px → 64 pt: 16 under the discs.
        pub const NEXT_CONTENT_TOP: f32 = 64.0;
    }

    /// The lights sit where Grok's do, the discs are centred on Grok's line
    /// (8 pt under the lights' — they are not one line in Grok either), and
    /// the first thing under the strip starts at Grok's 64. (`HEADER_HEIGHT`
    /// is the Mac strip; `top_strip` returns it on macOS and 40 elsewhere.)
    #[test]
    fn the_strip_is_groks_height_and_the_discs_sit_on_groks_line() {
        assert_eq!(
            TRAFFIC_LIGHT_POSITION,
            grok::LIGHT_LEFT,
            "close frame x = y"
        );
        assert_eq!(TRAFFIC_LIGHT_LINE, grok::LIGHT_LINE, "lights' centre line");
        assert_eq!(CONTROL_LINE, grok::DISC_LINE, "discs' centre line");
        assert_eq!(
            CONTROL_LINE - TRAFFIC_LIGHT_LINE,
            8.0,
            "discs 8 under the lights"
        );
        if cfg!(target_os = "macos") {
            assert_eq!(top_strip(false), HEADER_HEIGHT, "the strip is the top band");
        }
        assert_eq!(
            HEADER_HEIGHT,
            2.0 * CONTROL_LINE,
            "discs centred in the band"
        );
        let disc_top = (HEADER_HEIGHT - STRIP_BUTTON) / 2.0;
        assert_eq!(disc_top, grok::DISC_TOP);
        assert_eq!(disc_top + STRIP_BUTTON, 48.0, "disc bottom");
        assert_eq!(
            HEADER_HEIGHT + FILTER_TOP,
            grok::NEXT_CONTENT_TOP,
            "filter capsules start where Grok's first card does"
        );
        assert_eq!(
            HEADER_HEIGHT + FILTER_TOP - (disc_top + STRIP_BUTTON),
            16.0,
            "gap from the discs down to the filter: Grok's 16"
        );
        // The three lights (frame right edge 15 + 2 × 23 + 14 = 75) end inside
        // the band the strip keeps clear for them.
        let lights_right = TRAFFIC_LIGHT_POSITION + 2.0 * grok::LIGHT_PITCH + 14.0;
        assert!(lights_right <= TRAFFIC_LIGHTS_INSET + STRIP_INSET);
    }

    /// Native full screen: the strip is laid out exactly as in windowed mode —
    /// no band above it, nothing pushed down. The traffic lights are macOS's
    /// own, so they follow macOS's full-screen rules (hidden until the pointer
    /// reaches the top edge); the app paints none of its own (Jacob
    /// 2026-10-24: "this should be real mac os feature not custom buttons").
    #[test]
    fn full_screen_adds_no_band_above_the_strip() {
        assert_eq!(top_strip(true), top_strip(false));
    }

    /// The strip and filter rows as `render` composes them (same div shape),
    /// laid out for real: a disc's box lands at 12–48 and the first capsule
    /// at 64, Grok's numbers.
    struct StripAndFilter;

    impl gpui::Render for StripAndFilter {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let strip = div()
                .h(px(HEADER_HEIGHT))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(STRIP_GAP))
                .pl(px(TRAFFIC_LIGHTS_INSET + STRIP_INSET))
                .pr(px(STRIP_INSET))
                .debug_selector(|| "strip".into())
                .children(strip_controls(false, true).into_iter().map(|c| {
                    let sel: &'static str = match c {
                        StripControl::Search => "disc-search",
                        StripControl::New => "disc-new",
                        StripControl::Collapse => "disc-collapse",
                    };
                    div()
                        .size(px(c.size()))
                        .flex_shrink_0()
                        .debug_selector(move || sel.into())
                }));
            let filter = div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(super::FILTER_GAP))
                .pt(px(FILTER_TOP))
                .pb(px(super::FILTER_BOTTOM))
                .px(px(8.0))
                .children(ListFilter::ALL.into_iter().map(|f| {
                    let sel: &'static str =
                        Box::leak(format!("capsule-{}", f.label()).into_boxed_str());
                    div()
                        .h(px(FILTER_HEIGHT))
                        .px(px(super::FILTER_PAD_X))
                        .debug_selector(move || sel.into())
                        .child(f.label())
                }));
            div()
                .w(px(296.0))
                .h(px(400.0))
                .flex()
                .flex_col()
                .child(strip)
                .child(filter)
        }
    }

    #[gpui::test]
    fn the_discs_and_the_filter_land_on_groks_pixels(cx: &mut TestAppContext) {
        let (_view, vcx) = cx.add_window_view(|_, _| StripAndFilter);
        vcx.run_until_parked();
        let strip = vcx.debug_bounds("strip").expect("strip drawn");
        assert_eq!(f32::from(strip.size.height), HEADER_HEIGHT);
        let mut right_edges = Vec::new();
        for sel in ["disc-search", "disc-new", "disc-collapse"] {
            let disc = vcx.debug_bounds(sel).expect(sel);
            let top: f32 = disc.origin.y.into();
            let h: f32 = disc.size.height.into();
            assert_eq!(top, grok::DISC_TOP, "{sel} top");
            assert_eq!(top + h / 2.0, grok::DISC_LINE, "{sel} centre");
            assert_eq!(h, STRIP_BUTTON, "{sel} size");
            right_edges.push(f32::from(disc.right()));
        }
        assert_eq!(
            right_edges[2],
            296.0 - STRIP_INSET,
            "collapse at the right inset"
        );
        assert_eq!(
            right_edges[1] + STRIP_GAP + STRIP_BUTTON,
            right_edges[2],
            "discs 8 apart"
        );
        let capsule = vcx.debug_bounds("capsule-All").expect("first capsule");
        let cap_top: f32 = capsule.origin.y.into();
        assert_eq!(cap_top, grok::NEXT_CONTENT_TOP, "capsule row at Grok's 64");
        assert_eq!(f32::from(capsule.size.height), FILTER_HEIGHT);
    }

    /// Three-pane rule: the chat list's strip holds exactly three controls —
    /// search, +, collapse, in that order — and nothing else (#1134; the
    /// always-open pill of #1491 is gone, and so is the #1606 inline field:
    /// search opens the palette, [feedback search-palette-2026-10-24]).
    #[test]
    fn the_strip_holds_exactly_search_plus_collapse() {
        let controls = strip_controls(false, true);
        assert_eq!(
            controls,
            vec![
                StripControl::Search,
                StripControl::New,
                StripControl::Collapse
            ]
        );
        assert_eq!(controls.len(), 3, "exactly three controls");
    }

    /// Jacob (2026-10-24): the three discs are EXACTLY Grok Bot's size, one
    /// family. Measured on `ref-grok-round-search-plus.png` at 2 px/pt: disc
    /// 72 px, glyph boxes 28 / 26 px at a 3 px stroke, 16 px between discs.
    #[test]
    fn the_three_discs_are_groks_size() {
        for control in strip_controls(false, true) {
            assert_eq!(control.size(), CONTROL, "{control:?} disc");
            assert_eq!(control.size(), 36.0, "{control:?}: Grok's 72 px / 2");
        }
        assert_eq!(STRIP_BUTTON, CONTROL);
        assert_eq!(STRIP_GAP, CONTROL_GAP);
        assert_eq!(STRIP_GAP, 8.0, "Grok's 16 px / 2 between discs");
        assert_eq!(STRIP_INSET, 12.0);
        // System symbols ([feedback sf-symbols-everywhere-mac-2026-10-28]): the
        // magnifier and plus sit in the CONTROL_GLYPH box at GLYPH_POINT_RATIO of
        // it (16 pt), medium weight — Apple's `magnifyingglass` inks ≈ 0.9 of its
        // point size (14.5, Grok's 14), `plus` ≈ 0.8 (12.9, Grok's 13); the wider
        // `sidebar.left` takes the narrower box for the same 14 pt mark.
        assert_eq!(
            StripControl::Search.glyph(),
            ("magnifyingglass", CONTROL_GLYPH)
        );
        assert_eq!(StripControl::New.glyph(), ("plus", CONTROL_GLYPH));
        assert_eq!(
            StripControl::Collapse.glyph(),
            ("sidebar.left", CONTROL_GLYPH_WIDE)
        );
        let point = crate::ui::sf_symbols::GLYPH_POINT_RATIO * CONTROL_GLYPH;
        let magnifier = 0.9 * point;
        let plus = 0.8 * point;
        let collapse = 0.95 * crate::ui::sf_symbols::GLYPH_POINT_RATIO * CONTROL_GLYPH_WIDE;
        assert!(
            (magnifier - 14.0).abs() <= 0.75,
            "magnifier mark {magnifier}"
        );
        assert!((plus - 13.0).abs() <= 0.5, "plus mark {plus}");
        assert!((collapse - 14.0).abs() <= 0.5, "collapse mark {collapse}");
        for name in ["magnifyingglass", "plus", "sidebar.left"] {
            assert!(crate::ui::sf_symbols::is_registered(name), "{name}");
        }
    }

    /// Inside a server there is no search; a member without admin rights has
    /// no "+" either — the collapse toggle always stays.
    #[test]
    fn a_server_drops_the_search_and_members_drop_the_plus() {
        assert_eq!(
            strip_controls(true, true),
            vec![StripControl::New, StripControl::Collapse]
        );
        assert_eq!(strip_controls(true, false), vec![StripControl::Collapse]);
    }

    fn agent(shared: bool) -> Project {
        Project {
            kind: SpaceKind::Project,
            shared,
            ..Project::default()
        }
    }

    /// The segment persists beside the collapsed state, so a rail left on
    /// Servers reopens on Servers ([feedback mac-sidebar-rail-2026-10-26]).
    #[test]
    fn the_filter_is_restored_from_the_config_dir() {
        let dir = std::env::temp_dir().join(format!("arbos-rail-filter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            ListFilter::load_from(&dir),
            ListFilter::All,
            "nothing saved → All"
        );
        for f in ListFilter::ALL {
            f.save_to(&dir);
            assert_eq!(ListFilter::load_from(&dir), f);
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("sidebar_filter")).unwrap(),
            "servers",
            "the label in lower case, the rig's spelling"
        );
        std::fs::write(dir.join("sidebar_filter"), "nope").unwrap();
        assert_eq!(ListFilter::load_from(&dir), ListFilter::All);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn people(kind: SpaceKind) -> Project {
        Project {
            kind,
            ..Project::default()
        }
    }

    /// The four segments scope the list like the iPhone home: All shows
    /// everything; Agents my agents only; People DMs and groups; Servers only
    /// servers (the new fourth segment).
    #[test]
    fn the_filter_scopes_agents_people_and_servers() {
        let mine = agent(false);
        let shared_in = agent(true);
        let dm = people(SpaceKind::Dm);
        let group = people(SpaceKind::Group);
        for f in [ListFilter::All, ListFilter::Servers] {
            assert!(f.admits_servers(), "{f:?} shows servers");
        }
        for f in [ListFilter::Agents, ListFilter::People] {
            assert!(!f.admits_servers(), "{f:?} hides servers");
        }
        assert!(ListFilter::All.admits_space(&mine));
        assert!(ListFilter::All.admits_space(&shared_in));
        assert!(ListFilter::All.admits_space(&dm));
        assert!(ListFilter::All.admits_space(&group));
        assert!(ListFilter::Agents.admits_space(&mine));
        assert!(!ListFilter::Agents.admits_space(&shared_in));
        assert!(!ListFilter::Agents.admits_space(&dm));
        assert!(ListFilter::People.admits_space(&dm));
        assert!(ListFilter::People.admits_space(&group));
        assert!(!ListFilter::People.admits_space(&mine));
        for p in [&mine, &shared_in, &dm, &group] {
            assert!(
                !ListFilter::Servers.admits_space(p),
                "Servers shows only servers"
            );
        }
    }

    /// The labels are the iPhone's (`HomeListFilter` in HomeView.swift), in
    /// the same order, with "Servers" fourth; the rig parses them.
    #[test]
    fn the_segments_match_the_iphone_home() {
        let labels: Vec<_> = ListFilter::ALL.iter().map(|f| f.label()).collect();
        assert_eq!(labels, ["All", "Agents", "People", "Servers"]);
        assert_eq!(ListFilter::parse("servers"), Some(ListFilter::Servers));
        assert_eq!(ListFilter::parse("nope"), None);
        assert_eq!(ListFilter::default(), ListFilter::All);
    }

    /// The filter row as `render_with` builds it in a column too narrow for
    /// the four capsules: a horizontal scroll under `scroll`, the selected
    /// capsule pinned into view through `pin` — once per selection / width,
    /// or on every frame when `pin_every_frame` (the behaviour before
    /// [feedback server-strip-scroll-2026-10-28], kept here to show the test
    /// tells the two apart).
    struct NarrowFilter {
        scroll: gpui::ScrollHandle,
        pin: crate::ui::sidebar_fit::FilterPin,
        pin_every_frame: bool,
        selected: usize,
        width: f32,
    }

    impl gpui::Render for NarrowFilter {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let measured = self.scroll.bounds().size.width > px(0.0);
            if self.pin_every_frame || self.pin.take(self.selected, self.width, measured) {
                self.scroll.scroll_to_item(self.selected);
            }
            let count = ListFilter::ALL.len();
            let row = div()
                .id("narrow-filter")
                .debug_selector(|| "filter-row".into())
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(super::FILTER_GAP))
                .px(px(8.0))
                .overflow_x_scroll()
                .track_scroll(&self.scroll)
                .children(ListFilter::ALL.into_iter().enumerate().map(|(ix, f)| {
                    let sel: &'static str =
                        Box::leak(format!("capsule-{}", f.label()).into_boxed_str());
                    div()
                        .h(px(FILTER_HEIGHT))
                        .px(px(super::FILTER_PAD_X))
                        .flex_shrink_0()
                        .map(|mut el| {
                            if ix == 0 {
                                el.style().margin.left = Some(gpui::Length::Auto);
                            }
                            if ix + 1 == count {
                                el.style().margin.right = Some(gpui::Length::Auto);
                            }
                            el
                        })
                        .debug_selector(move || sel.into())
                        .child(f.label())
                }));
            div()
                .w(px(self.width))
                .h(px(120.0))
                .flex()
                .flex_col()
                .child(row)
        }
    }

    fn narrow_filter(scroll: &gpui::ScrollHandle, pin_every_frame: bool) -> NarrowFilter {
        NarrowFilter {
            scroll: scroll.clone(),
            pin: Default::default(),
            pin_every_frame,
            selected: 3,
            // Narrower than the four capsules need: the row scrolls.
            width: 150.0,
        }
    }

    /// Affinity, Arbonites, 2026-10-28 ("if on server cannot scroll back to
    /// make 'all' visible again"; Scout confirmed): with Servers selected in
    /// a narrow column the strip opens scrolled so Servers shows (All
    /// clipped) — and after the user scrolls back, the offset must be able to
    /// reach 0 with "All" fully in view, and stay there across repaints.
    #[gpui::test]
    fn the_strip_scrolls_back_to_all_and_stays_there(cx: &mut TestAppContext) {
        let scroll = gpui::ScrollHandle::new();
        let (view, vcx) = cx.add_window_view(|_, _| narrow_filter(&scroll, false));
        // The pin lands on the frame after the one that measured the capsules.
        for _ in 0..3 {
            view.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
        let row = vcx.debug_bounds("filter-row").expect("the row");
        let all = vcx.debug_bounds("capsule-All").expect("All");
        let servers = vcx.debug_bounds("capsule-Servers").expect("Servers");
        assert!(
            f32::from(scroll.max_offset().x) > 0.0,
            "the column is narrow enough for the row to scroll"
        );
        assert!(
            f32::from(scroll.offset().x) < 0.0,
            "opens pinned to the selected capsule: offset {:?}",
            scroll.offset()
        );
        assert!(
            servers.right() <= row.right() + px(0.5),
            "Servers in view: {servers:?} in {row:?}"
        );
        assert!(all.left() < row.left(), "All clipped on the left: {all:?}");

        // The user scrolls back to the start (what the wheel / trackpad does
        // to the handle's offset), and the strip repaints.
        scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        let row = vcx.debug_bounds("filter-row").expect("the row");
        let all = vcx.debug_bounds("capsule-All").expect("All");
        assert_eq!(
            f32::from(scroll.offset().x),
            0.0,
            "the offset reaches 0 and holds across the repaint"
        );
        assert!(
            all.left() >= row.left() && all.right() <= row.right(),
            "All fully visible: {all:?} in {row:?}"
        );
        // And another frame (a hover, a notify) does not drag it back either.
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        assert_eq!(f32::from(scroll.offset().x), 0.0, "still 0 a frame later");

        // The same row pinned on every frame — the bug — snaps back to Servers.
        let scroll = gpui::ScrollHandle::new();
        let (view, vcx) = cx.add_window_view(|_, _| narrow_filter(&scroll, true));
        for _ in 0..3 {
            view.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
        assert!(f32::from(scroll.offset().x) < 0.0, "pinned to Servers");
        scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        assert!(
            f32::from(scroll.offset().x) < 0.0,
            "pinning every frame undoes the scroll back (the old behaviour)"
        );
    }

    /// The pin fires once per selection and once per width (after the handle
    /// has a frame behind it); otherwise never.
    #[test]
    fn the_pin_fires_on_selection_and_width_changes_only() {
        let pin = crate::ui::sidebar_fit::FilterPin::default();
        assert!(pin.take(3, 220.0, false), "before the first frame: ask");
        assert!(
            pin.take(3, 220.0, false),
            "…and keep asking until one can land"
        );
        assert!(pin.take(3, 220.0, true), "the first measured frame");
        assert!(!pin.take(3, 220.0, true), "a repaint");
        assert!(
            !pin.take(3, 220.2, true),
            "sub-pixel width noise is the same width"
        );
        assert!(pin.take(0, 220.0, true), "the selection moved");
        assert!(!pin.take(0, 220.0, true));
        assert!(pin.take(0, 260.0, true), "the column was resized");
        assert!(!pin.take(0, 260.0, true));
    }

    /// The filter stays in the sidebar unchanged (Jacob, search-palette item
    /// point 4): drawn whenever the chat list is, never inside a server.
    #[test]
    fn the_filter_stays_under_the_strip() {
        assert!(
            super::filter_shown(false),
            "filter drawn with the chat list"
        );
        assert!(!super::filter_shown(true), "no filter inside a server");
    }
}

#[cfg(test)]
mod footer_tests {
    use gpui::{div, prelude::*, px, TestAppContext};

    use super::{
        footer_disc_frame, footer_frame, footer_row_frame, FOOTER_AVATAR, FOOTER_BOTTOM,
        FOOTER_DISC, FOOTER_DISC_GLYPH, FOOTER_DISC_INSET, FOOTER_DOT, FOOTER_DOT_OVERHANG,
        FOOTER_DOT_RING, FOOTER_HEIGHT, FOOTER_INSET, FOOTER_ROW_HEIGHT, FOOTER_ROW_INSET,
        FOOTER_ROW_PAD_X, FOOTER_ROW_PAD_Y, STRIP_BUTTON, STRIP_GAP, STRIP_INSET,
    };
    use crate::ui::theme::{Theme, CONTROL, CONTROL_LINE, HEADER_HEIGHT};

    /// Grok Bot's footer, measured on Jacob's Oct-2026 refs
    /// (`media/group-chat-redesign/refs/1-agent-chat-topbutton-sidebar.png`,
    /// 2520 × 1670 px = 2 px/pt, window bottom at the last row) — Jacob
    /// 2026-10-24: "Make the profile picture at the bottom here the same size
    /// as it is on grok" ([feedback footer-avatar-grok-size-2026-10-24]).
    /// Points from the sidebar's left edge and the window's bottom edge.
    mod grok {
        /// Avatar: x 26–97, rows 1574–1645 px → 72 px disc.
        pub const AVATAR: f32 = 36.0;
        pub const AVATAR_LEFT: f32 = 13.0;
        /// 1670 − 1646 = 24 px under the disc.
        pub const AVATAR_BOTTOM: f32 = 12.0;
        /// Centre row 1609.5 → 60.5 px ≈ 30 pt from the bottom: the strip's
        /// discs' line, mirrored. (Grok's "Connect apps" pill and its update
        /// disc share the same 72 px rows.)
        pub const AVATAR_LINE: f32 = 30.0;
    }

    /// The footer avatar is Grok's: the button family's 36, 13 from the left,
    /// 12 above the bottom, centred on the discs' line — a 60 band like the
    /// strip. The dot scales with it (2 pt ring, 2 pt overhang).
    #[test]
    fn the_footer_avatar_is_groks_size_inset_and_line() {
        assert_eq!(FOOTER_AVATAR, grok::AVATAR);
        assert_eq!(FOOTER_AVATAR, CONTROL, "one button family");
        assert_eq!(FOOTER_INSET, grok::AVATAR_LEFT);
        assert_eq!(FOOTER_BOTTOM, grok::AVATAR_BOTTOM);
        assert_eq!(
            FOOTER_HEIGHT - FOOTER_AVATAR - 2.0 * FOOTER_BOTTOM,
            0.0,
            "centred: 12 above and below"
        );
        assert_eq!(
            FOOTER_HEIGHT / 2.0,
            grok::AVATAR_LINE,
            "centre 30 from the bottom"
        );
        assert_eq!(
            FOOTER_HEIGHT / 2.0,
            CONTROL_LINE,
            "the strip's line, mirrored"
        );
        assert_eq!(FOOTER_HEIGHT, HEADER_HEIGHT, "the strip's band, mirrored");
        assert_eq!(FOOTER_ROW_INSET + FOOTER_ROW_PAD_X, FOOTER_INSET);
        assert_eq!(
            2.0 * FOOTER_ROW_PAD_Y + FOOTER_ROW_HEIGHT,
            FOOTER_HEIGHT,
            "band = pad + card + pad"
        );
        assert_eq!(FOOTER_DOT_RING - FOOTER_DOT, 4.0, "2 pt ring");
        assert_eq!(FOOTER_DOT_OVERHANG, 2.0);
        // 28 → 36 scaled the 7 / 11 dot to 9 / 14; the ring stays 2 pt.
        assert_eq!(FOOTER_DOT, (7.0f32 * 36.0 / 28.0).round());
        // The mark's ring is drawn from `marks::corner_metrics`: the same numbers.
        let m = crate::ui::marks::corner_metrics(FOOTER_AVATAR);
        assert_eq!(
            (m.glyph, m.ring, m.overhang),
            (FOOTER_DOT, FOOTER_DOT_RING, FOOTER_DOT_OVERHANG)
        );
    }

    /// The footer as `render` frames it, laid out for real at the bottom of a
    /// sidebar column: the avatar's box lands at Grok's pixels, and so does
    /// the update disc when one is due.
    struct Column {
        with_disc: bool,
    }

    impl gpui::Render for Column {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let avatar = div()
                .relative()
                .size(px(FOOTER_AVATAR))
                .flex_shrink_0()
                .debug_selector(|| "avatar".into())
                .child(
                    div()
                        .absolute()
                        .right(px(-FOOTER_DOT_OVERHANG))
                        .bottom(px(-FOOTER_DOT_OVERHANG))
                        .size(px(FOOTER_DOT_RING))
                        .debug_selector(|| "dot-ring".into()),
                );
            let row = footer_row_frame()
                .child(avatar)
                .child(div().flex_1().min_w_0().child("Jacob Steeves"));
            let disc = self.with_disc.then(|| footer_disc_frame("disc"));
            div()
                .w(px(296.0))
                .h(px(520.0))
                .flex()
                .flex_col()
                .child(div().h(px(HEADER_HEIGHT)).flex_shrink_0())
                .child(div().flex_1().min_h_0())
                .child(footer_frame(row, disc).debug_selector(|| "footer".into()))
        }
    }

    #[gpui::test]
    fn the_footer_avatar_lands_on_groks_pixels(cx: &mut TestAppContext) {
        let (_view, vcx) = cx.add_window_view(|_, _| Column { with_disc: false });
        vcx.run_until_parked();
        let footer = vcx.debug_bounds("footer").expect("footer drawn");
        let bottom: f32 = footer.bottom().into();
        assert_eq!(bottom, 520.0, "the footer is the column's last band");
        assert_eq!(f32::from(footer.size.height), FOOTER_HEIGHT);
        let avatar = vcx.debug_bounds("avatar").expect("avatar drawn");
        let left: f32 = avatar.origin.x.into();
        let top: f32 = avatar.origin.y.into();
        let h: f32 = avatar.size.height.into();
        assert_eq!(h, grok::AVATAR, "avatar size");
        assert_eq!(left, grok::AVATAR_LEFT, "avatar left inset");
        assert_eq!(
            bottom - (top + h),
            grok::AVATAR_BOTTOM,
            "avatar above the bottom"
        );
        assert_eq!(
            bottom - (top + h / 2.0),
            grok::AVATAR_LINE,
            "avatar centre line"
        );
        let ring = vcx.debug_bounds("dot-ring").expect("dot ring drawn");
        let ring_right: f32 = ring.right().into();
        let ring_bottom: f32 = ring.bottom().into();
        assert_eq!(
            ring_right,
            left + h + FOOTER_DOT_OVERHANG,
            "dot overhangs right"
        );
        assert_eq!(
            ring_bottom,
            top + h + FOOTER_DOT_OVERHANG,
            "dot overhangs bottom"
        );
        let card = vcx
            .debug_bounds("sidebar-account")
            .expect("hover card drawn");
        let card_left: f32 = card.origin.x.into();
        assert_eq!(card_left, FOOTER_ROW_INSET, "card at the rows' inset");
        assert_eq!(f32::from(card.size.height), FOOTER_ROW_HEIGHT);
        let card_right: f32 = card.right().into();
        assert_eq!(
            card_right,
            296.0 - FOOTER_ROW_INSET,
            "no disc: the card runs to the inset"
        );
    }

    /// Grok Bot's update disc, same refs: a 72 px blue circle at x 449–520 on
    /// the avatar's rows — under the strip's rightmost disc ("+" in Grok at x
    /// 449–520 too), 8 after the pill; fill `#326CCB`, glyph box 26 × 28 px.
    mod grok_disc {
        pub const DISC: f32 = 36.0;
        pub const DISC_LINE: f32 = 30.0;
        /// The glyph: 26 × 28 px → 13 × 14 pt.
        pub const GLYPH_W: f32 = 13.0;
        pub const GLYPH_H: f32 = 14.0;
        pub const GAP_BEFORE: f32 = 8.0;
        pub const FILL: u32 = 0x326ccb;
        pub const GLYPH_FILL: u32 = 0xf5f5f5;
    }

    /// Jacob (2026-10-24): "Give us the same update button as grok also." The
    /// disc is the button family's 36 at the strip's right inset, on the
    /// footer's line; its cloud-download glyph draws Grok's 13 × 14 mark; the
    /// fills are Grok's; hover and pressed lift the fill (chip rule).
    #[test]
    fn the_update_disc_is_groks_size_colour_and_placement() {
        assert_eq!(FOOTER_DISC, grok_disc::DISC);
        assert_eq!(FOOTER_DISC, CONTROL, "one button family");
        assert_eq!(FOOTER_DISC, STRIP_BUTTON, "the strip's discs, mirrored");
        assert_eq!(
            FOOTER_DISC_INSET, STRIP_INSET,
            "under the strip's last disc"
        );
        assert_eq!(FOOTER_HEIGHT / 2.0, grok_disc::DISC_LINE);
        assert_eq!(
            STRIP_GAP,
            grok_disc::GAP_BEFORE,
            "8 after the card, as after Grok's pill"
        );
        // The glyph is drawn 15.6 × 16.2 on the 24 grid (stroke included).
        let w = (15.6 / 24.0) * FOOTER_DISC_GLYPH;
        let h = (16.2 / 24.0) * FOOTER_DISC_GLYPH;
        assert!((w - grok_disc::GLYPH_W).abs() <= 0.5, "glyph width {w}");
        assert!((h - grok_disc::GLYPH_H).abs() <= 0.5, "glyph height {h}");
        for theme in [
            Theme::dark(crate::ui::theme::Surface::DEFAULT),
            Theme::light(crate::ui::theme::Surface::DEFAULT),
        ] {
            let fill: gpui::Rgba = theme.update_disc.into();
            let glyph: gpui::Rgba = theme.on_update_disc.into();
            assert_eq!(rgb_u32(fill), grok_disc::FILL, "disc fill");
            assert_eq!(rgb_u32(glyph), grok_disc::GLYPH_FILL, "glyph fill");
            // Hover / pressed come from the highlight token: the text-colour
            // wash over the blue — brighter in dark (white wash), darker in
            // light (black wash) — one step, then one more.
            let f = crate::ui::highlight::fills(&theme);
            let hover = crate::ui::highlight::over(theme.update_disc, f.hover);
            let pressed = crate::ui::highlight::over(theme.update_disc, f.pressed);
            assert_ne!(hover, theme.update_disc, "changes under the pointer");
            if theme.is_light() {
                assert!(hover.l < theme.update_disc.l && pressed.l < hover.l);
            } else {
                assert!(hover.l > theme.update_disc.l && pressed.l > hover.l);
            }
        }
    }

    fn rgb_u32(c: gpui::Rgba) -> u32 {
        let ch = |v: f32| (v * 255.0).round() as u32;
        (ch(c.r) << 16) | (ch(c.g) << 8) | ch(c.b)
    }

    #[gpui::test]
    fn the_update_disc_lands_on_groks_pixels(cx: &mut TestAppContext) {
        let (_view, vcx) = cx.add_window_view(|_, _| Column { with_disc: true });
        vcx.run_until_parked();
        let footer = vcx.debug_bounds("footer").expect("footer drawn");
        let bottom: f32 = footer.bottom().into();
        let disc = vcx.debug_bounds("disc").expect("disc drawn");
        let top: f32 = disc.origin.y.into();
        let h: f32 = disc.size.height.into();
        let w: f32 = disc.size.width.into();
        let right: f32 = disc.right().into();
        assert_eq!(h, grok_disc::DISC, "disc size");
        assert_eq!(w, h, "a circle");
        assert_eq!(right, 296.0 - STRIP_INSET, "at the strip's right inset");
        assert_eq!(
            bottom - (top + h / 2.0),
            grok_disc::DISC_LINE,
            "on the footer's line"
        );
        // Same rows as the avatar: Grok's pill, avatar and disc share them.
        let avatar = vcx.debug_bounds("avatar").expect("avatar drawn");
        assert_eq!(
            avatar.origin.y, disc.origin.y,
            "avatar and disc on one line"
        );
        let card = vcx
            .debug_bounds("sidebar-account")
            .expect("hover card drawn");
        let card_right: f32 = card.right().into();
        let disc_left: f32 = disc.origin.x.into();
        assert_eq!(
            disc_left - card_right,
            grok_disc::GAP_BEFORE,
            "8 between card and disc"
        );
    }
}

#[cfg(test)]
mod row_bounds_tests {
    use gpui::{div, prelude::*, px, ScrollHandle, TestAppContext};

    use super::{faded_list, top_strip, LIST_FADE_BAND};

    const ROWS: usize = 12;
    const ROW_H: f32 = 58.0;

    /// The sidebar column as `render` builds it: strip, the faded scrolling
    /// list (the exact wrapper `render` uses), the account row. Twelve rows so
    /// the list scrolls.
    struct Column {
        scroll: ScrollHandle,
    }

    impl gpui::Render for Column {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let strip = div()
                .h(px(top_strip(false)))
                .flex_shrink_0()
                .debug_selector(|| "sidebar-strip".into());
            let mut list = div()
                .id("chat-list")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .px(px(8.0))
                .pt(px(LIST_FADE_BAND + 6.0))
                .pb(px(12.0))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .debug_selector(|| "chat-list".into());
            for i in 0..ROWS {
                let sel: &'static str = Box::leak(format!("row-{i}").into_boxed_str());
                list = list.child(
                    div()
                        .h(px(ROW_H))
                        .flex_shrink_0()
                        .debug_selector(move || sel.into())
                        .child(format!("Chat {i}")),
                );
            }
            // The footer exactly as `render` frames it (band + hover card).
            let account = super::footer_frame(super::footer_row_frame(), None::<gpui::Empty>)
                .debug_selector(|| "account-row".into());
            div()
                .w(px(280.0))
                .h(px(520.0))
                .flex()
                .flex_col()
                .child(strip)
                .child(faded_list(list))
                .child(account)
        }
    }

    /// Jacob (Oct 23, v0.6.180): two chats' titles painted into one slot at
    /// the list's bottom. The list must keep its bounded height: no row may
    /// paint past the list (or into the account row), and no two rows may
    /// share a y-range — at the top and scrolled to the bottom.
    #[gpui::test]
    fn rows_stay_inside_the_list_and_never_overlap(cx: &mut TestAppContext) {
        let scroll = ScrollHandle::new();
        let (view, vcx) = cx.add_window_view(|_, _| Column {
            scroll: scroll.clone(),
        });
        for pass in ["top", "bottom"] {
            if pass == "bottom" {
                scroll.scroll_to_bottom();
                view.update(vcx, |_, cx| cx.notify());
            }
            vcx.run_until_parked();
            let list = vcx.debug_bounds("chat-list").expect("list");
            let account = vcx.debug_bounds("account-row").expect("account row");
            let list_bottom: f32 = list.bottom().into();
            let account_top: f32 = account.origin.y.into();
            assert!(
                list_bottom <= account_top + 0.5,
                "{pass}: list bottom {list_bottom} runs into the account row at {account_top}"
            );
            if pass == "top" {
                // #1497: the fade band ends at the strip's bottom edge and the
                // first row starts its 6 pt under it, clear of the fade.
                let frame = vcx.debug_bounds("sidebar-list-frame").expect("frame");
                let strip = vcx.debug_bounds("sidebar-strip").expect("strip");
                let first = vcx.debug_bounds("row-0").expect("first row");
                let fade_bottom: f32 = f32::from(frame.origin.y) + LIST_FADE_BAND;
                let strip_bottom: f32 = strip.bottom().into();
                let row_top: f32 = first.origin.y.into();
                assert!(
                    (fade_bottom - strip_bottom).abs() <= 0.5,
                    "fade band ends at {fade_bottom}, strip at {strip_bottom}"
                );
                assert!(
                    (row_top - (strip_bottom + 6.0)).abs() <= 0.5,
                    "first row at {row_top}, strip + 6 = {}",
                    strip_bottom + 6.0
                );
            }
            let mut spans: Vec<(f32, f32)> = Vec::new();
            for i in 0..ROWS {
                let sel: &'static str = Box::leak(format!("row-{i}").into_boxed_str());
                let Some(b) = vcx.debug_bounds(sel) else {
                    continue;
                };
                let (top, bottom): (f32, f32) = (b.origin.y.into(), b.bottom().into());
                // A row that shows must lie within the list's box (the scroll
                // clip): nothing paints past the list.
                if bottom > f32::from(list.origin.y) && top < list_bottom {
                    spans.push((top, bottom));
                }
            }
            spans.sort_by(|a, b| a.0.total_cmp(&b.0));
            for w in spans.windows(2) {
                assert!(
                    w[1].0 >= w[0].1 - 0.5,
                    "{pass}: rows overlap ({:?} then {:?})",
                    w[0],
                    w[1]
                );
            }
            if pass == "bottom" {
                let last: &'static str = Box::leak(format!("row-{}", ROWS - 1).into_boxed_str());
                // Jacob (18:46): "you can't scroll up and down on the chats" —
                // the list must have scrolled: the last row, off-screen at the
                // top, is now fully inside the list's box.
                let b = vcx
                    .debug_bounds(last)
                    .expect("last row drawn at scroll-bottom");
                let (top, bottom): (f32, f32) = (b.origin.y.into(), b.bottom().into());
                let list_top: f32 = list.origin.y.into();
                assert!(
                    top >= list_top && bottom <= list_bottom - 12.0 + 0.5,
                    "bottom: last row {top}..{bottom} not inside the list {list_top}..{list_bottom}"
                );
                let offset: f32 = scroll.offset().y.into();
                assert!(offset < -(ROW_H * 2.0), "the list scrolled ({offset})");
            }
        }
    }
}

#[cfg(test)]
mod channel_preview_tests {
    //! A server row's "#general · …" preview labels a person as a group's preview does
    //! (`@handle:`), me as "You", an agent by name — never a bare handle as if it were a
    //! name ([feedback server-member-names-2026-10-28]; the tester's shot read "general ·
    //! A: bug").
    use super::channel_preview_who;

    #[test]
    fn the_preview_labels_people_like_a_group_does() {
        let me = "user:test_01ME";
        assert_eq!(channel_preview_who("user:test_01scout", "A", me), "@A");
        assert_eq!(channel_preview_who("user:test_01me", "affinity", me), "You");
        assert_eq!(channel_preview_who("agent:01COMMS", "Comms", me), "Comms");
        assert_eq!(channel_preview_who("system", "", me), "");
        assert_eq!(channel_preview_who("system", "launchcrew", me), "");
        assert_eq!(channel_preview_who("", "", me), "", "no row yet");
        // The fixture names its senders outright (no member string).
        assert_eq!(channel_preview_who("", "Sami", me), "Sami");
    }
}

#[cfg(test)]
mod people_preview_tests {
    //! A people row's preview names the sender only where the row's header does not
    //! ([feedback doubled-sender-prefix-mac-2026-10-30]; Jacob's shot read "@DaWizzaa"
    //! in the header and "@DaWizzaa: …" in the body).
    use futures::future::BoxFuture;
    use gpui::{AppContext as _, TestAppContext};
    use tokio::sync::mpsc;

    use super::{people_preview_who, preview_for_tests};
    use crate::app::AppState;
    use crate::model::{Chat, Message, Person, Project, SpaceKind};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::theme::{Surface, Theme};

    struct Idle;

    impl SyncSource for Idle {
        fn name(&self) -> &'static str {
            "idle"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            _commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            Box::pin(std::future::pending())
        }
    }

    fn mage() -> Person {
        serde_json::from_value(serde_json::json!({
            "id": "gh_461", "handle": "dawizzaa", "name": "DaWizzaa", "status": "active",
        }))
        .expect("person")
    }

    fn chat(room: &str, space: &str, at: u64) -> Chat {
        serde_json::from_value(serde_json::json!({
            "id": room, "project_id": space, "title": "Main", "is_pinned": true,
            "is_system": false, "unread": false, "last_message_at": at,
        }))
        .expect("chat")
    }

    fn from_mage(room: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": "m1", "chatId": room, "role": "user", "sender": "user:gh_461",
            "senderHandle": "dawizzaa", "senderName": "DaWizzaa", "status": "done",
            "createdAt": at, "completedAt": at,
            "parts": [{ "id": "m1-t", "kind": "text", "text": "Or even better, a wizard hat" }],
        }))
        .expect("message")
    }

    /// The real preview builder on two spaces that differ only in kind: DaWizzaa's
    /// line reads bare in the 1:1 row (whose header is already DaWizzaa) and keeps
    /// its "@dawizzaa:" in the group row (whose header is the group).
    #[gpui::test]
    fn the_dm_row_drops_the_prefix_and_the_group_row_keeps_it(cx: &mut TestAppContext) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(Surface::Matte));
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        let now = crate::model::now_ms();
        state.update(cx, |s, cx| {
            for (id, name, kind) in [
                ("dm", "DaWizzaa", SpaceKind::Dm),
                ("group", "Emoji names", SpaceKind::Group),
            ] {
                s.projects.push(Project {
                    id: id.into(),
                    name: name.into(),
                    kind,
                    people: vec![mage()],
                    ..Project::default()
                });
                let room = format!("room-{id}");
                s.chats
                    .insert(id.into(), vec![chat(&room, id, now - 30_000)]);
                s.messages
                    .insert(room.clone(), vec![from_mage(&room, now - 30_000)]);
            }
            cx.notify();
        });
        let (dm, group) = state.read_with(cx, |s, _| {
            let dm = s
                .projects
                .iter()
                .find(|p| p.id == "dm")
                .cloned()
                .expect("dm");
            let group = s
                .projects
                .iter()
                .find(|p| p.id == "group")
                .cloned()
                .expect("group");
            (preview_for_tests(s, &dm), preview_for_tests(s, &group))
        });
        assert_eq!(
            dm, "Or even better, a wizard hat",
            "the 1:1 row's header is DaWizzaa, so the line must not say it again"
        );
        assert_eq!(
            group, "@dawizzaa: Or even better, a wizard hat",
            "the group row's header is the group, so the line names the sender"
        );
    }

    #[test]
    fn a_one_to_one_row_does_not_repeat_the_person_its_header_names() {
        assert_eq!(
            people_preview_who(false, false, "user:gh_461", Some("dawizzaa")),
            "",
            "the DM row is already titled with this person"
        );
        assert_eq!(
            people_preview_who(true, false, "user:gh_461", Some("dawizzaa")),
            "@dawizzaa",
            "a group row is titled with the group, so it names the sender"
        );
        assert_eq!(
            people_preview_who(false, true, "user:gh_me", Some("jacob")),
            "You",
            "my own line stays mine on both"
        );
        assert_eq!(
            people_preview_who(true, true, "user:gh_me", Some("jacob")),
            "You"
        );
        assert_eq!(
            people_preview_who(false, false, "agent:01COMMS", Some("comms")),
            "@comms",
            "an agent in a 1:1 chat is not the person the row names"
        );
        assert_eq!(
            people_preview_who(false, false, "system", None),
            "",
            "no handle, no label"
        );
    }
}

#[cfg(test)]
mod status_cell_tests {
    use std::time::Duration;

    use futures::future::BoxFuture;
    use gpui::{AppContext as _, Bounds, Entity, Pixels, TestAppContext, VisualTestContext};
    use tokio::sync::mpsc;

    use super::RowStatus;
    use crate::app::AppState;
    use crate::model::{Chat, Message, Person, Project, SpaceKind};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::marks::UNREAD_PILL_H;
    use crate::ui::motion::PulseClock;
    use crate::ui::shell::Shell;
    use crate::ui::theme::{Surface, Theme};
    use crate::ui::working::{WorkingClock, CELL_H, CELL_W, FRAME_MS};

    struct Idle;

    impl SyncSource for Idle {
        fn name(&self) -> &'static str {
            "idle"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            _commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            Box::pin(std::future::pending())
        }
    }

    /// The rule as code: the agent's spinner beats unread, unread beats a
    /// person typing (which draws nothing — the spinner is never a human's
    /// sign), typing beats quiet.
    #[test]
    fn the_cell_shows_one_thing_in_this_order() {
        assert_eq!(RowStatus::of(false, true, false), RowStatus::Typing);
        assert_eq!(
            RowStatus::of(false, true, true),
            RowStatus::Unread,
            "typing draws nothing, so it hides nothing"
        );
        assert_eq!(RowStatus::of(true, true, false), RowStatus::Working);
        assert_eq!(RowStatus::of(true, false, true), RowStatus::Working);
        assert_eq!(RowStatus::of(false, false, true), RowStatus::Unread);
        assert_eq!(RowStatus::of(false, false, false), RowStatus::Quiet);
        assert_eq!(CELL_W, 8.0, "the cell is the old dot's width");
    }

    fn message(id: &str, chat: &str, role: &str, status: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "chatId": chat, "role": role, "sender": role, "status": status,
            "createdAt": at, "completedAt": at,
            "parts": [{ "id": format!("{id}-t"), "kind": "text", "text": text }],
        }))
        .expect("message")
    }

    fn chat(id: &str, project: &str, unread: bool, at: u64) -> Chat {
        serde_json::from_value(serde_json::json!({
            "id": id, "project_id": project, "title": "Main", "is_pinned": true,
            "is_system": false, "unread": unread, "last_message_at": at,
        }))
        .expect("chat")
    }

    fn person(id: &str, handle: &str, name: &str) -> Person {
        serde_json::from_value(serde_json::json!({
            "id": id, "handle": handle, "name": name, "status": "active",
        }))
        .expect("person")
    }

    /// Four rows: an agent working (`working`), an agent with unread
    /// (`unread`), a quiet read agent (`quiet`), a DM whose person is typing
    /// (`dm`). The quiet one is selected so the others keep their state.
    fn mount(
        cx: &mut TestAppContext,
    ) -> (
        tokio::runtime::Runtime,
        Entity<AppState>,
        Entity<Shell>,
        &mut VisualTestContext,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
            WorkingClock::init(cx);
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        let now = crate::model::now_ms();
        state.update(cx, |s, cx| {
            for (pid, name, kind, people) in [
                ("working", "Affine", SpaceKind::Project, vec![]),
                ("unread", "Lium", SpaceKind::Project, vec![]),
                ("quiet", "Arbos", SpaceKind::Project, vec![]),
                (
                    "dm",
                    "Sam Rivera",
                    SpaceKind::Dm,
                    vec![person("gh_2", "sam", "Sam Rivera")],
                ),
            ] {
                s.projects.push(Project {
                    id: pid.into(),
                    name: name.into(),
                    kind,
                    people,
                    ..Project::default()
                });
                let room = format!("room-{pid}");
                let at = now - 60_000;
                let mut rows = vec![
                    message(
                        "u",
                        &room,
                        "user",
                        "done",
                        "Can you look at the surf report?",
                        at,
                    ),
                    message("a", &room, "assistant", "done", "On it.", at + 1_000),
                ];
                if pid == "working" {
                    rows.push(message(
                        "w",
                        &room,
                        "assistant",
                        "streaming",
                        "",
                        now - 2_000,
                    ));
                }
                s.chats.insert(
                    pid.into(),
                    vec![chat(&room, pid, pid == "unread", at + 1_000)],
                );
                s.messages.insert(room.clone(), rows);
                if pid == "dm" {
                    // Typing marks are keyed by the space id (`typing_in(&p.id)`).
                    s.typing
                        .insert(pid.into(), vec![("sam".to_string(), now + 60_000)]);
                }
            }
            s.selected_project = Some("quiet".into());
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.run_until_parked();
        (rt, state, shell, vcx)
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    fn right(b: Bounds<Pixels>) -> f32 {
        f32::from(b.right())
    }

    fn centre_y(b: Bounds<Pixels>) -> f32 {
        f32::from(b.origin.y) + f32::from(b.size.height) / 2.0
    }

    /// The spinner sits exactly where the dot sat: its right edge is the time
    /// label's right edge (the unread dot's too), it is centred on the preview
    /// line, and it is the fixed cell. The unread row keeps its accent dot,
    /// the quiet row shows nothing, the typing DM keeps its green pulse.
    #[gpui::test]
    fn the_spinner_sits_under_the_time_where_the_dot_was(cx: &mut TestAppContext) {
        let (_rt, _state, _shell, vcx) = mount(cx);
        let spinner = vcx
            .debug_bounds("row-status-working-working")
            .expect("the working agent's row draws the spinner");
        let time = vcx
            .debug_bounds("row-time-working")
            .expect("its time label");
        let line = vcx
            .debug_bounds("row-preview-line-working")
            .expect("its preview line");
        assert_eq!(f32::from(spinner.size.width), CELL_W, "cell width");
        assert_eq!(f32::from(spinner.size.height), CELL_H, "cell height");
        assert!(
            close(right(spinner), right(time)),
            "spinner right {} = time right {}",
            right(spinner),
            right(time)
        );
        assert!(
            close(centre_y(spinner), centre_y(line)),
            "spinner centre {} = preview line centre {}",
            centre_y(spinner),
            centre_y(line)
        );
        assert!(
            f32::from(spinner.origin.y)
                > f32::from(time.origin.y) + f32::from(time.size.height) - 1.0,
            "under the time, not beside it"
        );

        let pill = vcx
            .debug_bounds("row-status-unread-unread")
            .expect("the unread agent's row shows the count pill (never a dot)");
        assert_eq!(f32::from(pill.size.height), UNREAD_PILL_H);
        assert!(
            f32::from(pill.size.width) >= UNREAD_PILL_H,
            "at least as wide as tall"
        );
        assert!(
            close(
                right(pill),
                right(vcx.debug_bounds("row-time-unread").expect("time"))
            ),
            "the pill ends under its time too"
        );
        assert!(
            close(right(pill), right(spinner)),
            "pill and spinner share the right edge under the time"
        );
        assert!(
            vcx.debug_bounds("row-status-working-unread").is_none(),
            "an unread row that is not working shows no spinner"
        );

        assert!(vcx.debug_bounds("row-status-working-quiet").is_none());
        assert!(vcx.debug_bounds("row-status-unread-quiet").is_none());

        assert!(
            vcx.debug_bounds("row-status-working-dm").is_none(),
            "a person typing never shows the agent's spinner"
        );
        assert!(
            vcx.debug_bounds("row-status-unread-dm").is_none(),
            "a person typing shows no mark in the cell (the preview says typing…)"
        );
    }

    /// One spinning row (the typing DM pulses, it does not spin), one ticker;
    /// the cell's box never moves while the glyph turns; Reduce Motion keeps
    /// the cell (still glyph) and no ticker.
    #[gpui::test]
    fn rows_share_one_ticker_and_the_cell_never_moves(cx: &mut TestAppContext) {
        let (_rt, _state, _shell, vcx) = mount(cx);
        let first = vcx
            .debug_bounds("row-status-working-working")
            .expect("spinner drawn");
        let t = vcx.update(|_, cx| WorkingClock::trace(cx));
        assert!(t.running, "spinners on screen → ticker running");
        assert_eq!(t.starts, 1, "one ticker");
        let ticks0 = t.ticks;
        for _ in 0..10 {
            vcx.executor()
                .advance_clock(Duration::from_millis(FRAME_MS));
            vcx.run_until_parked();
            let b = vcx
                .debug_bounds("row-status-working-working")
                .expect("spinner drawn");
            assert_eq!(b, first, "the cell's box is the same every frame");
        }
        let t = vcx.update(|_, cx| WorkingClock::trace(cx));
        assert_eq!(t.starts, 1, "still one ticker");
        assert_eq!(
            t.ticks - ticks0,
            10,
            "ten frames for ten periods, not twenty"
        );
        assert_eq!(
            t.drawn_last_frame, 1,
            "one spinner cell on screen: the working agent's (the DM pulses)"
        );
        // The typing DM's pulse rides the 30 fps clock, so the window draws
        // more than once per 90 ms period here; the ticker itself adds one.
        assert!(t.draws_last_tick >= 1);

        // Nobody typing: the ticker alone draws the Shell once per period.
        _state.update(vcx, |s, cx| {
            s.typing.clear();
            cx.notify();
        });
        vcx.run_until_parked();
        for _ in 0..3 {
            vcx.executor()
                .advance_clock(Duration::from_millis(FRAME_MS));
            vcx.run_until_parked();
        }
        let t = vcx.update(|_, cx| WorkingClock::trace(cx));
        assert_eq!(t.drawn_last_frame, 1);
        assert_eq!(
            t.draws_last_tick, 1,
            "the ticker alone draws the Shell once per period"
        );
        assert_eq!(t.starts, 1, "still the one ticker");

        vcx.update(|_, cx| cx.set_reduce_motion(true));
        for _ in 0..3 {
            vcx.executor()
                .advance_clock(Duration::from_millis(FRAME_MS));
            vcx.run_until_parked();
        }
        assert_eq!(
            vcx.debug_bounds("row-status-working-working"),
            Some(first),
            "Reduce Motion keeps the still glyph in the same cell"
        );
        let t = vcx.update(|_, cx| WorkingClock::trace(cx));
        assert!(
            !t.running,
            "Reduce Motion: nothing asks for frames, the ticker parks"
        );
    }

    /// Where a row's `@handle` sits against its own preview line — the one
    /// measure that is the same for every DM row whatever its name is drawn with.
    fn handle_offset(vcx: &mut VisualTestContext, id: &'static str) -> (f32, f32) {
        let handle = vcx
            .debug_bounds(match id {
                "dm" => "row-handle-dm",
                "dm-emoji" => "row-handle-dm-emoji",
                "dm-mixed" => "row-handle-dm-mixed",
                _ => unreachable!(),
            })
            .unwrap_or_else(|| panic!("{id}: the DM row shows its @handle"));
        let line = vcx
            .debug_bounds(match id {
                "dm" => "row-preview-line-dm",
                "dm-emoji" => "row-preview-line-dm-emoji",
                "dm-mixed" => "row-preview-line-dm-mixed",
                _ => unreachable!(),
            })
            .unwrap_or_else(|| panic!("{id}: its preview line"));
        (
            f32::from(line.origin.y) - f32::from(handle.origin.y),
            f32::from(handle.size.height),
        )
    }

    /// [feedback emoji-name-handle-offset-mac-2026-10-28]: a DM whose person is
    /// named with one emoji (and one with a mixed name) draws its `@handle` at
    /// the same height as a text name's — and the rows stay the same height.
    #[gpui::test]
    fn an_emoji_name_keeps_the_dm_rows_handle_where_text_names_have_it(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        let now = crate::model::now_ms();
        state.update(vcx, |s, cx| {
            for (pid, name, handle) in [
                ("dm-emoji", "🧙🏽\u{200d}♂\u{fe0f}", "dawizzaa"),
                ("dm-mixed", "Cato 🌳", "cato"),
            ] {
                s.projects.push(Project {
                    id: pid.into(),
                    name: name.into(),
                    kind: SpaceKind::Dm,
                    people: vec![person("gh_9", handle, name)],
                    ..Project::default()
                });
                let room = format!("room-{pid}");
                let at = now - 30_000;
                s.chats
                    .insert(pid.into(), vec![chat(&room, pid, false, at)]);
                s.messages.insert(
                    room.clone(),
                    vec![message("u", &room, "user", "done", "Or even better", at)],
                );
            }
            cx.notify();
        });
        for _ in 0..3 {
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
        let text = handle_offset(vcx, "dm");
        let emoji = handle_offset(vcx, "dm-emoji");
        let mixed = handle_offset(vcx, "dm-mixed");
        assert_eq!(
            emoji, text,
            "emoji name: handle offset / height as for text"
        );
        assert_eq!(
            mixed, text,
            "mixed name: handle offset / height as for text"
        );
        let row_h = |vcx: &mut VisualTestContext, sel: &'static str| {
            f32::from(vcx.debug_bounds(sel).expect(sel).size.height)
        };
        let text_h = row_h(vcx, "row-preview-line-dm");
        assert_eq!(row_h(vcx, "row-preview-line-dm-emoji"), text_h);
        assert_eq!(row_h(vcx, "row-preview-line-dm-mixed"), text_h);
    }
}

/// Sidebar pinning ([feedback sidebar-pinning-mac-2026-10-28]): the Pinned section and
/// the pin list's rules, on the real Shell (headless) and on `AppState` alone.
#[cfg(test)]
mod pin_tests {
    use std::sync::{Arc, Mutex};

    use futures::future::BoxFuture;
    use gpui::{AppContext as _, Bounds, Entity, Pixels, TestAppContext, VisualTestContext};
    use tokio::sync::mpsc;

    use super::ListFilter;
    use crate::app::AppState;
    use crate::model::{Person, Project, ServerInfo, SpaceKind};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::motion::PulseClock;
    use crate::ui::shell::Shell;
    use crate::ui::theme::{Surface, Theme};
    use crate::ui::working::WorkingClock;

    type Commands = Arc<Mutex<Option<mpsc::UnboundedReceiver<ClientCommand>>>>;

    /// Keeps the command channel so a test can read what the state sent (`SetPins`).
    struct Recording(Commands);

    impl SyncSource for Recording {
        fn name(&self) -> &'static str {
            "recording"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            *self.0.lock().expect("commands slot") = Some(commands);
            Box::pin(std::future::pending())
        }
    }

    fn person(id: &str, handle: &str, name: &str) -> Person {
        serde_json::from_value(serde_json::json!({
            "id": id, "handle": handle, "name": name, "status": "active",
        }))
        .expect("person")
    }

    /// Three agents (`arbos` selected, `lium`, `affine`), a DM (`dm`) and a server (`srv`).
    fn seed(s: &mut AppState) {
        for (pid, name, kind, people) in [
            ("arbos", "Arbos", SpaceKind::Project, vec![]),
            ("lium", "Lium", SpaceKind::Project, vec![]),
            ("affine", "Affine", SpaceKind::Project, vec![]),
            (
                "dm",
                "Sam Rivera",
                SpaceKind::Dm,
                vec![person("gh_2", "sam", "Sam Rivera")],
            ),
        ] {
            s.projects.push(Project {
                id: pid.into(),
                name: name.into(),
                kind,
                people,
                ..Project::default()
            });
        }
        s.servers.push(ServerInfo {
            id: "srv".into(),
            name: "Launch week".into(),
            ..ServerInfo::default()
        });
        s.servers_available = true;
        s.selected_project = Some("arbos".into());
    }

    fn mount(
        cx: &mut TestAppContext,
    ) -> (
        tokio::runtime::Runtime,
        Commands,
        Entity<AppState>,
        Entity<Shell>,
        &mut VisualTestContext,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
            WorkingClock::init(cx);
        });
        let commands: Commands = Arc::new(Mutex::new(None));
        let state = cx.new(|cx| AppState::new(Box::new(Recording(commands.clone())), cx));
        state.update(cx, |s, cx| {
            seed(s);
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.run_until_parked();
        // The source runs on tokio: a few turns so `Recording::run` stashes the receiver.
        for _ in 0..3 {
            rt.block_on(tokio::task::yield_now());
        }
        (rt, commands, state, shell, vcx)
    }

    fn top(b: Bounds<Pixels>) -> f32 {
        f32::from(b.origin.y)
    }

    fn pins_of(state: &Entity<AppState>, cx: &mut TestAppContext) -> Vec<String> {
        state.read_with(cx, |s, _| s.pins.clone())
    }

    fn sent_pins(commands: &Commands) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        if let Some(rx) = commands.lock().expect("commands").as_mut() {
            while let Ok(c) = rx.try_recv() {
                if let ClientCommand::SetPins { pins } = c {
                    out.push(pins);
                }
            }
        }
        out
    }

    #[gpui::test]
    fn zero_pins_draws_no_section(cx: &mut TestAppContext) {
        let (_rt, _cmds, _state, _shell, vcx) = mount(cx);
        assert!(vcx.debug_bounds("sidebar-pinned-header").is_none());
        assert!(vcx.debug_bounds("sidebar-pinned-divider").is_none());
        for id in ["arbos", "lium", "affine", "dm", "srv"] {
            assert!(
                vcx.debug_bounds(Box::leak(format!("row-pin-{id}").into_boxed_str()))
                    .is_none(),
                "{id} carries no pin mark"
            );
        }
        assert!(
            vcx.debug_bounds("row-time-arbos").is_some(),
            "the list is drawn"
        );
    }

    #[gpui::test]
    fn pinned_rows_lead_in_the_users_order_under_a_header(cx: &mut TestAppContext) {
        let (_rt, _cmds, state, _shell, vcx) = mount(cx);
        state.update(vcx, |s, cx| {
            // The DM first, then the server, then an agent: not the activity order,
            // not the kind order.
            s.pins = vec!["chat:dm".into(), "server:srv".into(), "agent:affine".into()];
            cx.notify();
        });
        vcx.run_until_parked();
        let header = vcx
            .debug_bounds("sidebar-pinned-header")
            .expect("the Pinned label");
        let divider = vcx
            .debug_bounds("sidebar-pinned-divider")
            .expect("the hairline after the pins");
        let dm = vcx.debug_bounds("row-pin-dm").expect("the DM's pin mark");
        let srv = vcx
            .debug_bounds("row-pin-srv")
            .expect("the server's pin mark");
        let affine = vcx
            .debug_bounds("row-pin-affine")
            .expect("Affine's pin mark");
        let arbos = vcx.debug_bounds("row-time-arbos").expect("Arbos, unpinned");
        let lium = vcx.debug_bounds("row-time-lium").expect("Lium, unpinned");
        assert!(top(header) < top(dm), "header above the first pin");
        assert!(
            top(dm) < top(srv) && top(srv) < top(affine),
            "the user's order"
        );
        assert!(top(affine) < top(divider), "hairline after the last pin");
        assert!(
            top(divider) < top(arbos) && top(divider) < top(lium),
            "the rest below the hairline"
        );
        assert!(vcx.debug_bounds("row-pin-arbos").is_none());
        assert!(vcx.debug_bounds("row-pin-lium").is_none());
        assert_eq!(
            f32::from(header.size.height),
            super::PINNED_HEADER_H,
            "header band"
        );
    }

    #[gpui::test]
    fn the_filter_shows_only_the_pins_it_admits(cx: &mut TestAppContext) {
        let (_rt, _cmds, state, shell, vcx) = mount(cx);
        state.update(vcx, |s, cx| {
            s.pins = vec!["server:srv".into(), "agent:lium".into()];
            cx.notify();
        });
        shell.update(vcx, |sh, cx| {
            sh.list_filter = ListFilter::Agents;
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("sidebar-pinned-header").is_some());
        assert!(
            vcx.debug_bounds("row-pin-lium").is_some(),
            "the pinned agent"
        );
        assert!(
            vcx.debug_bounds("row-pin-srv").is_none(),
            "Agents never shows the pinned server"
        );
        state.update(vcx, |s, cx| {
            s.pins = vec!["server:srv".into()];
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("sidebar-pinned-header").is_none(),
            "no pin passes the filter → no section"
        );
        assert!(vcx.debug_bounds("sidebar-pinned-divider").is_none());
        shell.update(vcx, |sh, cx| {
            sh.list_filter = ListFilter::Servers;
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("row-pin-srv").is_some(),
            "Servers shows it again"
        );
    }

    #[gpui::test]
    fn the_row_menu_leads_with_pin_then_unpin(cx: &mut TestAppContext) {
        let (_rt, _cmds, state, shell, vcx) = mount(cx);
        for id in ["lium", "dm", "srv"] {
            shell.update(vcx, |sh, cx| {
                sh.row_menu = Some((id.into(), gpui::point(gpui::px(40.0), gpui::px(200.0))));
                cx.notify();
            });
            vcx.run_until_parked();
            assert!(
                vcx.debug_bounds("row-menu-pin").is_some(),
                "{id}'s menu has the Pin row"
            );
        }
        state.update(vcx, |s, cx| s.set_pinned("lium".into(), true, cx));
        shell.update(vcx, |sh, cx| {
            sh.row_menu = Some(("lium".into(), gpui::point(gpui::px(40.0), gpui::px(200.0))));
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("row-menu-pin").is_some(),
            "Unpin in Pin's place"
        );
        assert!(state.read_with(vcx, |s, _| s.is_pinned("lium")));
    }

    #[gpui::test]
    fn pins_land_on_top_stop_at_the_cap_and_go_to_the_relay(cx: &mut TestAppContext) {
        let (_rt, cmds, state, _shell, vcx) = mount(cx);
        vcx.run_until_parked();
        state.update(vcx, |s, cx| {
            s.set_pinned("lium".into(), true, cx);
            s.set_pinned("dm".into(), true, cx);
            s.set_pinned("srv".into(), true, cx);
        });
        assert_eq!(
            pins_of(&state, vcx),
            vec!["server:srv", "chat:dm", "agent:lium"],
            "a new pin lands at the top"
        );
        assert_eq!(state.read_with(vcx, |s, _| s.pin_index("srv")), Some(0));
        assert_eq!(
            state.read_with(vcx, |s, _| s.pin_key("dm")).as_deref(),
            Some("chat:dm")
        );
        assert_eq!(state.read_with(vcx, |s, _| s.pin_key("nope")), None);

        state.update(vcx, |s, cx| {
            s.pin_cap = 3;
            s.set_pinned("affine".into(), true, cx);
        });
        assert_eq!(pins_of(&state, vcx).len(), 3, "the cap refuses the fourth");
        assert!(
            state.read_with(vcx, |s, _| s.notices.iter().any(|n| n
                .error
                .as_ref()
                .is_some_and(|e| e.sentence().contains("pin up to 3")))),
            "one sentence says why"
        );

        state.update(vcx, |s, cx| s.set_pinned("dm".into(), false, cx));
        assert_eq!(pins_of(&state, vcx), vec!["server:srv", "agent:lium"]);

        // Archiving a pinned row unpins it; deleting one does too.
        state.update(vcx, |s, cx| s.set_space_archived("srv".into(), true, cx));
        assert_eq!(pins_of(&state, vcx), vec!["agent:lium"]);
        state.update(vcx, |s, cx| s.delete_project("lium".into(), cx));
        assert!(pins_of(&state, vcx).is_empty());

        let sent = sent_pins(&cmds);
        assert_eq!(sent.len(), 6, "every change went to the relay: {sent:?}");
        assert_eq!(sent[2], vec!["server:srv", "chat:dm", "agent:lium"]);
        assert!(sent.last().is_some_and(Vec::is_empty));

        // The relay's answer is the truth.
        state.update(vcx, |s, cx| {
            s.apply(
                SyncEvent::Pins {
                    pins: vec!["agent:arbos".into()],
                    cap: 5,
                },
                cx,
            )
        });
        assert_eq!(pins_of(&state, vcx), vec!["agent:arbos"]);
        assert_eq!(state.read_with(vcx, |s, _| s.pin_cap), 5);
    }

    #[gpui::test]
    fn reordering_keeps_hidden_pins_in_place(cx: &mut TestAppContext) {
        let (_rt, _cmds, state, _shell, vcx) = mount(cx);
        state.update(vcx, |s, cx| {
            s.pins = vec![
                "agent:lium".into(),
                "server:srv".into(),
                "agent:affine".into(),
                "chat:dm".into(),
            ];
            // Under the Agents filter only Lium and Affine show; dragging Affine above Lium
            // swaps the two and leaves the server and the DM where they were.
            s.reorder_pins(&["affine".into(), "lium".into()], cx);
        });
        assert_eq!(
            pins_of(&state, vcx),
            vec!["agent:affine", "server:srv", "agent:lium", "chat:dm"]
        );
        // All filter: every pin shows; the DM dragged to the top.
        state.update(vcx, |s, cx| {
            s.reorder_pins(
                &["dm".into(), "affine".into(), "srv".into(), "lium".into()],
                cx,
            )
        });
        assert_eq!(
            pins_of(&state, vcx),
            vec!["chat:dm", "agent:affine", "server:srv", "agent:lium"]
        );
    }

    #[gpui::test]
    fn a_pin_drag_previews_the_order_and_commits_on_drop(cx: &mut TestAppContext) {
        let (_rt, cmds, state, shell, vcx) = mount(cx);
        state.update(vcx, |s, cx| {
            s.pins = vec!["agent:lium".into(), "chat:dm".into(), "server:srv".into()];
            cx.notify();
        });
        vcx.run_until_parked();
        let _ = sent_pins(&cmds);
        shell.update(vcx, |sh, cx| sh.begin_pin_drag("srv".into(), cx));
        assert_eq!(
            shell.read_with(vcx, |sh, _| sh.pin_drag.clone()),
            Some(super::PinDragState {
                id: "srv".into(),
                to: 2
            })
        );
        // The pointer over the first pinned card: the server previews at the top.
        let first = vcx.debug_bounds("row-pin-lium").expect("Lium's card");
        shell.update(vcx, |sh, cx| {
            let to = sh.pin_slot_at(&"srv".into(), top(first) - 1.0, cx);
            assert_eq!(to, 0, "above every other pinned card");
            sh.pin_drag = Some(super::PinDragState {
                id: "srv".into(),
                to,
            });
            cx.notify();
        });
        vcx.run_until_parked();
        let srv = vcx.debug_bounds("row-pin-srv").expect("the server's mark");
        let lium = vcx.debug_bounds("row-pin-lium").expect("Lium's mark");
        assert!(
            top(srv) < top(lium),
            "the dragged row shows at the slot under the pointer"
        );
        assert_eq!(
            pins_of(&state, vcx),
            vec!["agent:lium", "chat:dm", "server:srv"],
            "nothing stored until the drop"
        );
        shell.update(vcx, |sh, cx| sh.finish_pin_drag(cx));
        assert_eq!(
            pins_of(&state, vcx),
            vec!["server:srv", "agent:lium", "chat:dm"]
        );
        assert!(shell.read_with(vcx, |sh, _| sh.pin_drag.clone()).is_none());
        assert_eq!(
            sent_pins(&cmds),
            vec![vec!["server:srv", "agent:lium", "chat:dm"]]
        );
        shell.update(vcx, |sh, cx| sh.finish_pin_drag(cx));
        assert!(sent_pins(&cmds).is_empty(), "a second drop is a no-op");
    }
}
