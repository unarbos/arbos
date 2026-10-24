//! The chat list: the iPhone home screen as a Mac sidebar. The top strip holds
//! three round buttons on the right — search (opens the command palette,
//! `ui::palette`; ⌘⇧F / ⌘K do the same), "+", and the collapse toggle — then
//! the iPhone home's All · Agents · People · Servers filter, then every agent,
//! person and server as one row — avatar, name, time, last line. The account
//! circle (its dot is the machine state) sits at the bottom; the Update pill
//! only while a newer build exists.
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
    div, prelude::*, px, Context, CursorStyle, Div, DragMoveEvent, ElementId, Focusable as _,
    FontWeight, MouseButton, Render, RenderImage, SharedString, Window,
};

use crate::app::AppState;
use crate::model::{Id, MachineState, Message, Project, Role, ServerInfo};
use crate::search;
use crate::ui::avatars;
use crate::ui::composer::Composer;
use crate::ui::frost::edge_faded;
use crate::ui::glass::{self, WindowDrag};
use crate::ui::groups;
use crate::ui::icons::icon;
use crate::ui::identity;
use crate::ui::motion::PulseClock;
use crate::ui::people;
use crate::ui::shell::{Shell, TRAFFIC_LIGHTS_INSET};
use crate::ui::theme::{
    Theme, AVATAR_ROW, CONTROL, CONTROL_GAP, CONTROL_GLYPH, CONTROL_GLYPH_WIDE, HEADER_HEIGHT,
    PANEL_RADIUS, SIDEBAR_RESIZE_HANDLE, TEXT_BODY, TEXT_SMALL, TEXT_TINY,
};

/// Drag payload for the chat-list column's right-edge resize handle.
#[derive(Clone)]
struct SidebarResizeDrag;

struct SidebarResizeDragGhost;

impl Render for SidebarResizeDragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The drag strip beside the traffic lights: Grok Mac's 44 pt top band, the
/// same height as the chat header ([feedback mac-header-height-grok]). Its
/// three round buttons (search · + · collapse, [`STRIP_BUTTON`] discs,
/// [`STRIP_GAP`] apart, [`STRIP_INSET`] from the right edge) are centred on
/// the lights' line: macOS draws them at 20.75 pt in this window, so the 44
/// band is centred 1 pt high (Grok: 21.75 / 21.75). The collapse toggle keeps
/// the right end so folding the pane stays intuitive (#1491).
///
/// Native full screen keeps this exact band: the shell paints the traffic
/// lights into it (`traffic_lights::overlay`) and nothing is added above the
/// strip ([feedback fullscreen-traffic-lights-2026-10-24]; pinned by
/// `traffic_lights::tests::full_screen_adds_no_band_above_the_strip`).
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
/// Server rows draw their picture as a rounded square (corner = this fraction
/// of the size) so they read apart from the agent and people discs.
pub(crate) const SERVER_PICTURE_CORNER: f32 = 11.0 / 36.0;

/// One control of the top strip, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
            StripControl::Search => ("search", CONTROL_GLYPH),
            StripControl::New => ("plus", CONTROL_GLYPH),
            StripControl::Collapse => ("sidebar-left", CONTROL_GLYPH_WIDE),
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
}

/// Newest message with prose across a space's chats (archived ones too — the
/// history is still there), for the preview line.
fn latest<'a>(state: &'a AppState, space: &Project) -> Option<&'a Message> {
    let mut best: Option<&Message> = None;
    for chat in state.tabs(&space.id) {
        if let Some(m) = state
            .messages_of(&chat.id)
            .iter()
            .rev()
            .find(|m| !m.plain_text().trim().is_empty())
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
        if state.is_mine(m) {
            "You".to_string()
        } else {
            m.sender_handle
                .as_deref()
                .map(|h| format!("@{h}"))
                .unwrap_or_default()
        }
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

pub fn machine_dot(state: MachineState, theme: &Theme) -> gpui::Hsla {
    match state {
        MachineState::Awake => theme.online,
        MachineState::Waking => theme.warn,
        MachineState::Error => theme.danger,
        MachineState::Asleep | MachineState::Stopped | MachineState::Unknown => theme.text_faint,
    }
}

struct RowView {
    space: Project,
    selected: bool,
    streaming: bool,
    unread: bool,
    typing: bool,
    preview: String,
    time: String,
    person_image: Option<Arc<RenderImage>>,
    guests: people::PeopleWithAvatars,
    avatar_state: avatars::AvatarState,
}

/// A server in the chat list (D88 rule 1): the same row as a group — disc, name, preview,
/// time, unread dot — sorted among the chats by last activity.
struct ServerRowView {
    server: ServerInfo,
    selected: bool,
    unread: bool,
    preview: String,
    time: String,
}

/// One entry of the left list, chats and servers interleaved.
enum ListEntry {
    Chat(Box<RowView>),
    Server(Box<ServerRowView>),
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
        Some(c) => format!("{} · {}", c.name, channel_last_line(c)),
        None => state
            .space_row(server)
            .map(|s| s.last_text.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "No messages yet".into()),
    }
}

/// "Sam: hi" for a channel row.
fn channel_last_line(row: &arbos_matrix::ChannelRow) -> String {
    let line = plain(row.last_text.lines().next().unwrap_or_default());
    if row.last_sender_name.is_empty() {
        line
    } else {
        format!("{}: {line}", row.last_sender_name)
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

pub fn render(shell: &Shell, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let selected = state.selected_project.clone();
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
    let shows = |archived: bool| hidden_mode == archived;
    // The All · Agents · People · Servers segment scopes the list, like the
    // iPhone home ([feedback sidebar-strip-filter-2026-10-24]).
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
                preview: hit.reason.unwrap_or_else(|| server_preview(state, s)),
                time: time_label(at),
                server: s.clone(),
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
            avatar_state: state.avatar_state(&p.id),
            space: p.clone(),
        };
        rows.push((
            (hit.kind, std::cmp::Reverse(last_activity(state, p))),
            ListEntry::Chat(Box::new(row)),
        ));
    }
    rows.sort_by_key(|(key, _)| *key);
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
        div()
            .id(id)
            .size(px(control.size()))
            .flex_shrink_0()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(theme.surface_2)
            .hover(|s| s.bg(Theme::with_alpha(theme.text, 0.14)))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(icon(glyph, size, theme.text))
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
        .pb(px(2.0))
        .when(open_server.is_some(), |el| el.child(div().flex_1()));
    for control in controls {
        strip = match control {
            StripControl::Search => strip.child(search_button(cx)),
            StripControl::New => strip.child(plus(cx)),
            StripControl::Collapse => strip.child(collapse(cx)),
        };
    }
    let strip = strip.window_drag();
    // ----- filter: All · Agents · People · Servers (the iPhone home's PaneTabs) --
    // Bare labels, the selected one on a capsule, centred; no track, no border.
    let filter_bar = div()
        .id("sidebar-filter")
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(FILTER_GAP))
        .pt(px(6.0))
        .pb(px(8.0))
        .px(px(8.0))
        .children(ListFilter::ALL.into_iter().map(|f| {
            let on = f == filter;
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "sidebar-filter-{}",
                    f.label().to_ascii_lowercase()
                ))))
                .h(px(FILTER_HEIGHT))
                .px(px(FILTER_PAD_X))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .bg(if on {
                    theme.bubble_assistant
                } else {
                    gpui::transparent_black()
                })
                .hover(move |s| if on { s } else { s.text_color(theme.text) })
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
                        .child(icon("hourglass", 14.0, theme.text)),
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
    for (_, entry) in rows {
        list = match entry {
            ListEntry::Chat(row) => list.child(render_row(*row, shell, &theme, cx)),
            ListEntry::Server(row) => list.child(render_server_row(*row, shell, &theme, cx)),
        };
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
                .hover(|s| s.bg(theme.surface_2))
                .child(format!("Hidden chats · {hidden_count}"))
                .child(icon("chevron-right", 12.0, theme.text_faint))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.hidden_open = true;
                    this.menu_open = false;
                    this.row_menu = None;
                    cx.notify();
                })),
        );
    }

    // ----- bottom: Update pill (only when a newer build exists) and the account row --
    let (who, person_for_avatar, account_image) = {
        let s = shell.state.read(cx);
        match s.account.clone() {
            Some(a) => {
                let who = if a.name.trim().is_empty() {
                    if a.login.is_empty() {
                        "Signed in".to_string()
                    } else {
                        format!("@{}", a.login)
                    }
                } else {
                    a.name.clone()
                };
                let person = crate::model::Person {
                    id: a.id.clone(),
                    handle: if a.login.is_empty() {
                        who.clone()
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
                let img = s.avatar(&a.avatar_url);
                (who, Some(person), img)
            }
            None => ("Settings".to_string(), None, None),
        }
    };
    let account_row = div()
        .id("sidebar-account")
        .mx(px(8.0))
        .mb(px(8.0))
        .h(px(44.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(10.0))
        .rounded(px(PANEL_RADIUS - 4.0))
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_2))
        .child(
            div()
                .relative()
                .size(px(28.0))
                .flex_shrink_0()
                .child(match &person_for_avatar {
                    Some(p) => people::avatar(account_image, p, 28.0, &theme),
                    None => div()
                        .size(px(28.0))
                        .rounded_full()
                        .bg(theme.surface_2)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("person", 14.0, theme.text)),
                })
                .child(
                    div()
                        .absolute()
                        .right(px(-2.0))
                        .bottom(px(-2.0))
                        .size(px(11.0))
                        .rounded_full()
                        .bg(theme.sidebar)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().size(px(7.0)).rounded_full().bg(Theme::with_alpha(
                            machine_dot(machine, &theme),
                            if machine == MachineState::Waking {
                                0.4 + 0.6 * PulseClock::phase(cx, 1100)
                            } else {
                                1.0
                            },
                        ))),
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

    // Drag handle on the right edge: a slim hit target (col-resize cursor) that
    // lets the user make the chat list slimmer or wider
    // ([feedback app-01M5MJ9Y91W9ZET71KTCE7PS65]).
    let resize = div()
        .id("sidebar-resize")
        .absolute()
        .top_0()
        .right_0()
        .h_full()
        .w(px(SIDEBAR_RESIZE_HANDLE))
        .cursor(CursorStyle::ResizeColumn)
        .hover(|s| s.bg(Theme::with_alpha(theme.primary, 0.45)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_drag(SidebarResizeDrag, |_, _, _, cx| {
            cx.new(|_| SidebarResizeDragGhost)
        })
        .on_drag_move(
            cx.listener(|this, ev: &DragMoveEvent<SidebarResizeDrag>, _, cx| {
                // The column docks at the window's left, so mouse x is the width.
                let width = f32::from(ev.event.position.x);
                this.state
                    .update(cx, |s, cx| s.set_sidebar_width(width, cx));
            }),
        );

    let mut column = div()
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
            el.child(filter_bar)
        })
        .child(match open_server.as_ref() {
            // Inside a server the left pane shows its channels (mockup h).
            Some(server) => faded_list(server_channels(server, shell, &theme, cx)),
            None => faded_list(list),
        });
    if let Some(update) = shell.render_update_button(cx) {
        column = column.child(div().px(px(12.0)).pb(px(8.0)).child(update));
    }
    column.child(account_row).child(resize)
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
                gpui::img(image)
                    .size(px(size))
                    .rounded_full()
                    .object_fit(gpui::ObjectFit::Cover),
            );
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
fn render_server_row(
    row: ServerRowView,
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> gpui::Stateful<Div> {
    let ServerRowView {
        server,
        selected,
        unread,
        preview,
        time,
    } = row;
    let picture = server_row_picture(&server, shell.state.read(cx), AVATAR_ROW, theme);
    let id: Id = server.id.clone();
    let menu_id: Id = server.id.clone();
    let ctrl_menu_id: Id = server.id.clone();
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
        .bg(if selected {
            theme.surface_2
        } else {
            gpui::transparent_black()
        })
        .hover(|s| {
            s.bg(if selected {
                theme.surface_2
            } else {
                Theme::with_alpha(theme.surface_2, 0.55)
            })
        })
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
                this.row_delete_armed = false;
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
                this.row_delete_armed = false;
                cx.notify();
            }),
        )
        .child(
            div()
                .relative()
                .size(px(AVATAR_ROW))
                .flex_shrink_0()
                .child(picture),
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
                                .child(server.name.clone()),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(TEXT_TINY))
                                .text_color(theme.text_muted)
                                .child(time),
                        ),
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
                            el.child(div().size(px(8.0)).rounded_full().bg(theme.accent))
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
                .child(icon("chevron-left", 14.0, theme.text))
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
            .map(channel_last_line)
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
                .bg(if selected {
                    theme.surface_2
                } else {
                    gpui::transparent_black()
                })
                .hover(|s| {
                    s.bg(if selected {
                        theme.surface_2
                    } else {
                        Theme::with_alpha(theme.surface_2, 0.55)
                    })
                })
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
                        .child(icon("hash", 18.0, theme.text_muted)),
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
                                .when(locked, |el| el.child(icon("lock", 11.0, theme.text_faint)))
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_size(px(TEXT_TINY))
                                        .text_color(theme.text_muted)
                                        .child(time),
                                ),
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
                                    el.child(div().size(px(8.0)).rounded_full().bg(theme.accent))
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
            if archived { "tray-up" } else { "archive" },
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

fn render_row(
    row: RowView,
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let RowView {
        space,
        selected,
        streaming,
        unread,
        typing,
        preview,
        time,
        person_image,
        guests,
        avatar_state,
    } = row;
    let id: Id = space.id.clone();
    let menu_id: Id = space.id.clone();
    let ctrl_menu_id: Id = space.id.clone();
    let can_rename = !space.is_guest() && !space.is_people();
    let renaming = shell
        .state
        .read(cx)
        .renaming_project
        .as_ref()
        .is_some_and(|p| p == &space.id);
    if typing || avatar_state != avatars::AvatarState::Idle {
        PulseClock::request(cx);
    }
    let phase = PulseClock::phase(cx, 900);

    // The picture: the initials disc for an agent or a group, the person's
    // photo for a DM (with a tiny agent badge when one of my agents sits in it).
    let picture: Div = if let Some(person) = space
        .person()
        .filter(|_| space.is_people() && !space.is_people_group())
    {
        let mut pic = div()
            .relative()
            .size(px(AVATAR_ROW))
            .flex_shrink_0()
            .child(people::avatar(person_image, person, AVATAR_ROW, theme));
        if let Some(agent) = space.agents.first() {
            // The agent's picture through the shared identity (#1592): the one its bubbles,
            // the presence row and the member list show — uploaded picture included.
            let state = shell.state.read(cx);
            let a = avatars::member_identity(Some(&space), &state.projects, agent);
            let image = a.image_key.as_deref().and_then(|url| state.avatar(url));
            pic = pic.child(
                div()
                    .absolute()
                    .right(px(-3.0))
                    .bottom(px(-3.0))
                    .p(px(2.0))
                    .rounded_full()
                    .bg(theme.bg)
                    .child(avatars::agent_picture(&a, image, 18.0, theme)),
            );
        }
        pic
    } else {
        let mut pic = div()
            .relative()
            .size(px(AVATAR_ROW))
            .flex_shrink_0()
            .child(groups::picture(
                &space,
                shell.state.read(cx),
                AVATAR_ROW,
                theme,
            ));
        if !guests.is_empty() {
            pic = pic.child(
                div()
                    .absolute()
                    .right(px(-4.0))
                    .bottom(px(-2.0))
                    .child(people::avatar_stack(&guests, 16.0, 2, theme)),
            );
        }
        pic
    };

    // Groups: the chat title (custom or member handles from the relay),
    // matching iOS PeopleRow. DMs: the other person's display name.
    let name = if space.is_people_group() {
        space.name.clone()
    } else if space.is_people() {
        space
            .person()
            .map(|p| p.display_name())
            .unwrap_or_else(|| space.name.clone())
    } else {
        space.name.clone()
    };
    // A group row reads like an agent row: name and preview, no member count.
    let handle = space
        .person()
        .filter(|_| space.is_people() && !space.is_people_group())
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
        .bg(if selected {
            theme.surface_2
        } else {
            gpui::transparent_black()
        })
        // Hover is a lighter tint of the selected fill in both modes (in light,
        // the old white hover read brighter than the selected row).
        .hover(|s| {
            s.bg(if selected {
                theme.surface_2
            } else {
                Theme::with_alpha(theme.surface_2, 0.55)
            })
        })
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
                this.row_delete_armed = false;
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
                this.row_delete_armed = false;
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
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(TEXT_BODY))
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
                                .child(name)
                                .into_any_element()
                        })
                        .when_some(handle, |el, h| {
                            el.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(TEXT_TINY))
                                    .text_color(theme.text_muted)
                                    .child(h),
                            )
                        })
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(TEXT_TINY))
                                .text_color(theme.text_muted)
                                .child(time),
                        ),
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
                        // No "agent is working" dot on rows (Jacob, 2026-10-22): the
                        // working state lives in the header / activity line. A person
                        // typing keeps its green pulse; unread keeps its accent dot.
                        .when(typing, |el| {
                            el.child(
                                div()
                                    .size(px(7.0))
                                    .rounded_full()
                                    .bg(Theme::with_alpha(theme.online, 0.35 + 0.65 * phase)),
                            )
                        })
                        .when(!typing && unread, |el| {
                            el.child(div().size(px(8.0)).rounded_full().bg(theme.accent))
                        }),
                ),
        )
}

/// The right-click menu of a sidebar row, anchored at the click. An agent
/// row (Jacob, #1258): Mute, Fork, a divider, Delete — nothing else; its
/// settings live on the agent pane. A people row: Profile, Mute, Delete; a
/// group row: Delete group.
pub fn row_menu(shell: &Shell, cx: &mut Context<Shell>) -> Option<Div> {
    let (space_id, at) = shell.row_menu.clone()?;
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let archived = state.is_space_archived(&space_id);
    let archive_row = |theme: &Theme, cx: &mut Context<Shell>| {
        let id = space_id.clone();
        glass::menu_row(
            "row-menu-archive",
            if archived { "tray-up" } else { "archive" },
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
    let delete_people = is_people;
    // A group (redesign 2026-10-20): one item, "Delete group". The owner
    // deletes it for everyone; a member leaves and the group stays.
    let is_group = space.is_people_group();
    let me_handle = state.me_handle();
    let owns_group = is_group && space.owner_handle.as_deref().is_none_or(|o| o == me_handle);
    let leave_instead = is_group && !owns_group;

    // Delete is destructive: the first click swaps the menu for an iPhone-style
    // "Delete “Name”?" with Cancel / Delete; Esc or a click elsewhere cancels.
    if shell.row_delete_armed && can_delete {
        let title = if is_people {
            space
                .person()
                .filter(|_| !space.is_people_group())
                .map(|p| p.display_name())
                .unwrap_or_else(|| space.name.clone())
        } else {
            space.name.clone()
        };
        let note = if leave_instead {
            "You leave the group. It stays for everyone else."
        } else if is_group {
            "Deletes the group for everyone in it."
        } else if is_people {
            "Removes this chat from your list."
        } else {
            "Removes this agent and its chats from your list."
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
                        confirm_button("row-menu-delete-cancel", "Cancel", false, &theme).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.row_menu = None;
                                this.row_delete_armed = false;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        confirm_button("row-menu-delete-go", "Delete", true, &theme).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.row_menu = None;
                                this.row_delete_armed = false;
                                let id = delete_id.clone();
                                this.state.update(cx, |s, cx| {
                                    if leave_instead {
                                        s.leave_group(id, cx);
                                    } else if delete_people {
                                        s.delete_people_chat(id, cx);
                                    } else {
                                        s.delete_project(id, cx);
                                    }
                                });
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
                .child(glass::frosted_menu(240.0, &theme, body)),
        );
    }

    let mut body = div().flex().flex_col().gap(px(1.0));
    if is_group {
        // Mockup f: the group row's menu is "Delete group" — plus Archive (2026-10-23).
        body = body
            .child(archive_row(&theme, cx))
            .child(glass::menu_divider(&theme));
        body = body.child(
            glass::menu_row("row-menu-delete", "trash", "Delete group", true, &theme).on_click(
                cx.listener(|this, _, _, cx| {
                    this.row_delete_armed = true;
                    cx.notify();
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
            glass::menu_row("row-menu-profile", "person", "Profile", false, &theme).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.row_menu = None;
                    let (page, id) = (profile_page.clone(), profile_id.clone());
                    this.state.update(cx, |s, cx| {
                        s.select_project(id, cx);
                        s.open_profile(page, cx);
                    });
                }),
            ),
        );
    }
    body = body.child(
        glass::menu_row(
            "row-menu-mute",
            if muted { "bell" } else { "bell-slash" },
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
            glass::menu_row("row-menu-fork", "branch", "Fork", false, &theme).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.row_menu = None;
                    let id = fork_id.clone();
                    this.state.update(cx, |s, cx| s.duplicate_project(id, cx));
                }),
            ),
        );
    }
    body = body.child(archive_row(&theme, cx));
    if can_delete {
        body = body.child(glass::menu_divider(&theme));
        body = body.child(
            glass::menu_row("row-menu-delete", "trash", "Delete", true, &theme).on_click(
                cx.listener(|this, _, _, cx| {
                    this.row_delete_armed = true;
                    cx.notify();
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
            el.bg(theme.field)
                .border_1()
                .border_color(theme.field_border)
                .text_color(theme.text)
                .hover(|s| s.bg(theme.surface_2))
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
            glass::menu_row("chat-menu-rename", "square-pencil", "Rename", false, &theme).on_click(
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
            glass::menu_row("chat-menu-fork", "branch", "Fork", false, &theme).on_click(
                cx.listener(move |this, _, window, cx| {
                    this.chat_menu = None;
                    this.tab_menu_open = false;
                    let id = fork_id.clone();
                    this.state.update(cx, |s, cx| s.fork_chat(id, cx));
                    Composer::focus(&this.composer, window, cx);
                }),
            ),
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
mod strip_tests {
    use super::{strip_controls, ListFilter, StripControl, STRIP_BUTTON, STRIP_GAP, STRIP_INSET};
    use crate::model::{Project, SpaceKind};
    use crate::ui::theme::{CONTROL, CONTROL_GAP, CONTROL_GLYPH, CONTROL_GLYPH_WIDE};

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
        // The 24-grid icons: search and plus at CONTROL_GLYPH draw Grok's
        // 14 / 13 pt marks within 1 px (0.75 × 19 = 14.25, 0.667 × 19 = 12.7);
        // the collapse rect (20/24 of the grid) takes the narrower size for
        // the same 14 pt box (0.833 × 17 = 14.2).
        assert_eq!(StripControl::Search.glyph(), ("search", CONTROL_GLYPH));
        assert_eq!(StripControl::New.glyph(), ("plus", CONTROL_GLYPH));
        assert_eq!(
            StripControl::Collapse.glyph(),
            ("sidebar-left", CONTROL_GLYPH_WIDE)
        );
        let magnifier = 0.75 * CONTROL_GLYPH;
        let plus = (16.0 / 24.0) * CONTROL_GLYPH;
        let collapse = (20.0 / 24.0) * CONTROL_GLYPH_WIDE;
        assert!((magnifier - 14.0).abs() <= 0.5, "magnifier box {magnifier}");
        assert!((plus - 13.0).abs() <= 0.5, "plus box {plus}");
        assert!((collapse - 14.0).abs() <= 0.5, "collapse box {collapse}");
        let stroke = 2.0 / 24.0 * CONTROL_GLYPH;
        assert!((stroke - 1.5).abs() <= 0.25, "stroke {stroke} (Grok 1.5)");
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
            let account = div()
                .h(px(44.0))
                .flex_shrink_0()
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
