//! The chat list: the iPhone home screen as a Mac sidebar. A top bar with the
//! collapse control and "+", search, then every agent and person as one row —
//! avatar, name, time, last line. The account circle (its dot is the machine
//! state) and gear sit at the bottom; the Update pill only while a newer build
//! exists.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, Context, CursorStyle, Div, DragMoveEvent, ElementId, Focusable as _,
    FontWeight, MouseButton, Render, RenderImage, SharedString, Window,
};

use crate::app::AppState;
use crate::model::{Id, MachineState, Message, Project, Role, ServerInfo};
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
    Theme, AVATAR_ROW, HEADER_HEIGHT, PANEL_RADIUS, SIDEBAR_RESIZE_HANDLE, TEXT_BODY, TEXT_SMALL,
    TEXT_TINY,
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
/// same height as the chat header ([feedback mac-header-height-grok]). The
/// collapse control sits right of the lights — at the header toggle's exact
/// spot, so folding the pane never moves it — and + / search keep the right
/// end. Controls are centred on the lights' line: macOS draws them at 20.75 pt
/// in this window, so the 44 band is centred 1 pt high (Grok: 21.75 / 21.75)
/// ([feedback mac-collapsed-toggle-inline]).
fn top_strip() -> f32 {
    if cfg!(target_os = "macos") {
        HEADER_HEIGHT
    } else {
        40.0
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

fn server_matches_query(server: &ServerInfo, query: &str) -> bool {
    server.name.to_lowercase().contains(query)
        || server
            .channels
            .iter()
            .any(|c| c.name.to_lowercase().contains(query))
}

/// The chats list's top and bottom fade (Jacob likes it, Oct 23). The band
/// sits under the strip's bottom edge: the scroll box is pulled up by the band
/// and the list pads by the same amount, so at rest the first row starts where
/// the fade ends and only rows that scroll under the strip cross it (the pane
/// rule from #1365 / #1424). The bottom band stays over the list's end.
pub const LIST_FADE_BAND: f32 = 14.0;

fn faded_list(list: impl IntoElement) -> gpui::AnyElement {
    div()
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
    let query = shell.search_input.read(cx).text().trim().to_lowercase();
    let machine = state.machine.shown_state();
    let sidebar_width = state.sidebar_width;
    // Inside a server the left pane shows its channels (D88 rule 2).
    let open_server = state.open_server_info().cloned();
    let open_server_admin = open_server.as_ref().is_some_and(ServerInfo::can_admin);
    let waitlist_position = state.machine.waitlist_position;

    let open_archived = state.open_archived;
    let archived_count = state.archived_space_count();
    let mut rows: Vec<(u64, ListEntry)> = Vec::new();
    // Servers stay on the main list (and while searching); the Archived folder
    // only holds agents and people chats.
    if !open_archived {
        for s in state.servers.iter() {
            if !query.is_empty() && !server_matches_query(s, &query) {
                continue;
            }
            let selected = state.open_server.as_deref() == Some(s.id.as_str())
                || state
                    .selected_channel
                    .as_ref()
                    .is_some_and(|c| c.server_id == s.id);
            let at = state.server_last_activity(s);
            rows.push((
                at,
                ListEntry::Server(Box::new(ServerRowView {
                    selected,
                    unread: state.space_row(s).is_some_and(|r| r.unread > 0),
                    preview: server_preview(state, s),
                    time: time_label(at),
                    server: s.clone(),
                })),
            ));
        }
    }
    for p in state.projects.iter() {
        let archived = state.is_space_archived(&p.id);
        // Main list: hide archived until search. Archived folder: only those.
        if open_archived {
            if !archived {
                continue;
            }
        } else if archived && query.is_empty() {
            continue;
        }
        // Name, @handle, chat title, or any loaded message text.
        if !query.is_empty() && !state.space_matches_query(p, &query) {
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
            preview: preview(state, p),
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
        rows.push((last_activity(state, p), ListEntry::Chat(Box::new(row))));
    }
    rows.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    let empty = state.projects.is_empty() && state.servers.is_empty();
    let link_connecting = state.link == crate::model::Link::Connecting;
    if machine == MachineState::Waking {
        PulseClock::request(cx);
    }

    // ----- top: collapse + "+" beside the traffic lights, then search ----------
    // Collapse lives here (not the chat header). When the rail is hidden the
    // header still shows an expand control so the list can come back.
    let collapse = div()
        .id("sidebar-collapse")
        .size(px(28.0))
        .flex_shrink_0()
        .rounded(px(7.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .bg(theme.surface_2)
        .hover(|s| s.bg(theme.surface_2))
        .child(icon("sidebar-left", 17.0, theme.text))
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(|this, _, window, cx| {
            this.menu_open = false;
            let width: f32 = window.viewport_size().width.into();
            this.state.update(cx, |s, cx| {
                let narrow = s.window_is_narrow(width);
                s.toggle_rail(narrow, cx)
            });
        }));
    let plus = div()
        .id("sidebar-new")
        .size(px(28.0))
        .flex_shrink_0()
        .rounded(px(7.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_2))
        .child(icon("plus", 17.0, theme.text_muted))
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
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
        });
    let show_plus = open_server.is_none() || open_server_admin;
    // The magnifier shows the search field below (and keeps it while a query
    // is typed); Escape or the clear dot fold it (three-pane rule: the strip
    // holds exactly +, search, minimize).
    let search_shown = shell.search_open || !query.is_empty();
    let search_toggle = div()
        .id("sidebar-search-toggle")
        .size(px(28.0))
        .flex_shrink_0()
        .rounded(px(7.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .bg(if search_shown {
            theme.surface_2
        } else {
            gpui::transparent_black()
        })
        .hover(|s| s.bg(theme.surface_2))
        .child(icon(
            "search",
            16.0,
            if search_shown {
                theme.text
            } else {
                theme.text_muted
            },
        ))
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(|this, _, window, cx| {
            this.menu_open = false;
            if this.search_open {
                this.search_open = false;
                this.search_input.update(cx, |i, cx| i.clear(cx));
                Composer::focus(&this.composer, window, cx);
            } else {
                this.search_open = true;
                let handle = this.search_input.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            cx.notify();
        }));
    let strip = div()
        .h(px(top_strip()))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(2.0))
        .pl(px(TRAFFIC_LIGHTS_INSET + 12.0))
        .pr(px(12.0))
        .pb(px(2.0))
        .child(collapse)
        .child(div().flex_1())
        .when(show_plus, |el| el.child(plus))
        .child(search_toggle)
        .window_drag();
    let search = div()
        .id("sidebar-search")
        .mx(px(12.0))
        .mt(px(6.0))
        .mb(px(6.0))
        .h(px(32.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(7.0))
        // Grok Mac: a flat tinted field (#F0F0F0 light / #242423 dark), no rim.
        .rounded(px(8.0))
        .bg(theme.surface_2)
        .cursor_text()
        .child(icon("search", 14.0, theme.text_muted))
        .child(div().flex_1().min_w_0().child(shell.search_input.clone()))
        .when(!query.is_empty(), |el| {
            el.child(
                div()
                    .id("sidebar-search-clear")
                    .size(px(16.0))
                    .rounded_full()
                    .bg(theme.text_faint)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(icon("xmark", 8.0, theme.bg))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.search_input.update(cx, |i, cx| i.clear(cx));
                        this.search_open = false;
                        Composer::focus(&this.composer, window, cx);
                        cx.notify();
                    })),
            )
        })
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                let handle = this.search_input.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }),
        );
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
    } else if rows.is_empty() && (query.is_empty() && archived_count == 0 || !query.is_empty()) {
        list = list.child(
            div()
                .pt(px(40.0))
                .text_center()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(format!("Nothing matches “{query}”.")),
        );
    }
    // Archived folder at the top of the chat list (when something is archived).
    if query.is_empty() && archived_count > 0 {
        list = list.child(archived_folder_row(archived_count, &theme, cx));
    }
    for (_, entry) in rows {
        list = match entry {
            ListEntry::Chat(row) => list.child(render_row(*row, shell, &theme, cx)),
            ListEntry::Server(row) => list.child(render_server_row(*row, &theme, cx)),
        };
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
        .when(
            search_shown && open_server.is_none() && !open_archived,
            |el| el.child(search),
        )
        .child(if let Some(server) = open_server.as_ref() {
            // Inside a server the left pane shows its channels (mockup h).
            faded_list(server_channels(server, shell, &theme, cx))
        } else if open_archived {
            faded_list(archived_folder(shell, &theme, cx))
        } else {
            faded_list(list)
        });
    if let Some(update) = shell.render_update_button(cx) {
        column = column.child(div().px(px(12.0)).pb(px(8.0)).child(update));
    }
    column.child(account_row).child(resize)
}

/// A server's picture: the leaf picked on its Create screen (D92 amendment),
/// else the D79 disc — initials from its name, the relay's gradient index.
pub fn server_picture(server: &ServerInfo, size: f32, theme: &Theme) -> Div {
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

/// A server row (D88 rule 1): drawn exactly like a group row — disc, name, time, last line,
/// unread dot. No badge, no count, no disclosure.
fn render_server_row(
    row: ServerRowView,
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
    let id: Id = server.id.clone();
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
        .on_click(cx.listener(move |this, _, window, cx| {
            this.menu_open = false;
            this.row_menu = None;
            this.chat_menu = None;
            this.state
                .update(cx, |s, cx| s.enter_server(id.clone(), cx));
            Composer::focus(&this.composer, window, cx);
        }))
        .child(
            div()
                .relative()
                .size(px(AVATAR_ROW))
                .flex_shrink_0()
                .child(server_picture(&server, AVATAR_ROW, theme)),
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

/// Folder row at the top of the chat list: opens the Archived pane.
fn archived_folder_row(
    count: usize,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> gpui::Stateful<Div> {
    div()
        .id("sidebar-archived-folder")
        .flex()
        .items_center()
        .gap(px(identity::ROW_GAP))
        .px(px(12.0))
        .py(px(11.0))
        .rounded(px(PANEL_RADIUS - 4.0))
        .cursor_pointer()
        .hover(|s| s.bg(Theme::with_alpha(theme.surface_2, 0.55)))
        .on_click(cx.listener(|this, _, _, cx| {
            this.menu_open = false;
            this.row_menu = None;
            this.chat_menu = None;
            this.state.update(cx, |s, cx| s.enter_archived(cx));
        }))
        .child(
            div()
                .size(px(AVATAR_ROW))
                .flex_shrink_0()
                .rounded_full()
                .bg(theme.surface_2)
                .flex()
                .items_center()
                .justify_center()
                .child(icon("archive", 18.0, theme.text_muted)),
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
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .child("Archived"),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(TEXT_TINY))
                                .text_color(theme.text_muted)
                                .child(format!("{count}")),
                        ),
                )
                .child(
                    div()
                        .text_size(px(TEXT_SMALL))
                        .text_color(theme.text_muted)
                        .child(if count == 1 {
                            "1 chat".into()
                        } else {
                            format!("{count} chats")
                        }),
                ),
        )
}

/// Inside the Archived folder: back to the chat list, then every archived space.
fn archived_folder(shell: &Shell, theme: &Theme, cx: &mut Context<Shell>) -> gpui::Stateful<Div> {
    let rows: Vec<(u64, RowView)> = {
        let state = shell.state.read(cx);
        let selected = state.selected_project.clone();
        let mut rows: Vec<(u64, RowView)> = state
            .projects
            .iter()
            .filter(|p| state.is_space_archived(&p.id))
            .map(|p| {
                let shown_people: Vec<_> = if p.is_people_group() {
                    Vec::new()
                } else if p.is_guest() {
                    p.owner().cloned().into_iter().collect()
                } else {
                    p.guests().cloned().collect()
                };
                let row = RowView {
                    selected: selected.as_deref() == Some(p.id.as_str()),
                    streaming: state.project_is_streaming(&p.id),
                    unread: state.project_unread(&p.id) > 0,
                    typing: !state.typing_in(&p.id).is_empty(),
                    preview: preview(state, p),
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
                (last_activity(state, p), row)
            })
            .collect();
        rows.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        rows
    };

    let back = div()
        .id("archived-back")
        .flex()
        .items_center()
        .gap(px(10.0))
        .px(px(8.0))
        .pt(px(4.0))
        .pb(px(10.0))
        .child(
            glass::circle("archived-back-circle", 30.0, theme)
                .child(icon("chevron-left", 14.0, theme.text))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.close_archived(cx));
                })),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .size(px(26.0))
                        .rounded_full()
                        .bg(theme.surface_2)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("archive", 14.0, theme.text_muted)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(TEXT_BODY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child("Archived"),
                ),
        );

    let mut list = div()
        .id("archived-list")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .px(px(8.0))
        .pt(px(LIST_FADE_BAND + 6.0))
        .pb(px(12.0))
        .overflow_y_scroll()
        .child(back);
    if rows.is_empty() {
        list = list.child(
            div()
                .pt(px(40.0))
                .text_center()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child("No archived chats"),
        );
    }
    for (_, row) in rows {
        list = list.child(render_row(row, shell, theme, cx));
    }
    list
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
                .child(server_picture(server, 26.0, theme))
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
            pic = pic.child(
                div()
                    .absolute()
                    .right(px(-3.0))
                    .bottom(px(-3.0))
                    .p(px(2.0))
                    .rounded_full()
                    .bg(theme.bg)
                    .child(people::agent_glyph(agent, 18.0, theme)),
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
    let space = state.projects.iter().find(|p| p.id == space_id)?.clone();
    let muted = state.is_muted(&space.id);
    let archived = state.is_space_archived(&space.id);
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
    let archive_id = space.id.clone();
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
        // Group row: Archive / Unarchive, then Delete group.
        body = body.child(
            glass::menu_row(
                "row-menu-archive",
                if archived { "tray-up" } else { "archive" },
                if archived { "Unarchive" } else { "Archive" },
                false,
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.row_menu = None;
                let id = archive_id.clone();
                let next = !archived;
                this.state
                    .update(cx, |s, cx| s.set_space_archived(id, next, cx));
            })),
        );
        body = body.child(glass::menu_divider(&theme));
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
    body = body.child(
        glass::menu_row(
            "row-menu-archive",
            if archived { "tray-up" } else { "archive" },
            if archived { "Unarchive" } else { "Archive" },
            false,
            &theme,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.row_menu = None;
            let id = archive_id.clone();
            let next = !archived;
            this.state
                .update(cx, |s, cx| s.set_space_archived(id, next, cx));
        })),
    );
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
mod fade_tests {
    use gpui::{div, prelude::*, px, TestAppContext};

    use super::{faded_list, top_strip, LIST_FADE_BAND};

    /// The sidebar's shape: the strip, then the faded list whose first row
    /// carries a selector. Mirrors `render`'s column.
    struct Column;

    impl gpui::Render for Column {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let strip = div()
                .h(px(top_strip()))
                .flex_shrink_0()
                .debug_selector(|| "sidebar-strip".into());
            let list = div()
                .id("chat-list")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .px(px(8.0))
                .pt(px(LIST_FADE_BAND + 6.0))
                .pb(px(12.0))
                .overflow_y_scroll()
                .child(
                    div()
                        .h(px(58.0))
                        .debug_selector(|| "first-row".into())
                        .child("Arbonites"),
                )
                .child(div().h(px(900.0)));
            div()
                .w(px(280.0))
                .h(px(600.0))
                .flex()
                .flex_col()
                .child(strip)
                .child(faded_list(list))
        }
    }

    #[gpui::test]
    fn the_fade_band_ends_at_the_strip_and_the_first_row_is_clear_at_rest(cx: &mut TestAppContext) {
        let (_view, vcx) = cx.add_window_view(|_, _| Column);
        vcx.run_until_parked();
        let strip = vcx.debug_bounds("sidebar-strip").expect("strip");
        let frame = vcx.debug_bounds("sidebar-list-frame").expect("list frame");
        let row = vcx.debug_bounds("first-row").expect("first row");
        let strip_bottom: f32 = strip.bottom().into();
        let frame_top: f32 = frame.origin.y.into();
        let fade_bottom = frame_top + LIST_FADE_BAND;
        let row_top: f32 = row.origin.y.into();
        assert!(
            fade_bottom <= strip_bottom + 0.5,
            "fade band bottom {fade_bottom} must sit at or above the strip's bottom {strip_bottom}"
        );
        assert!(
            row_top >= fade_bottom - 0.5,
            "first row top {row_top} is inside the fade (ends {fade_bottom})"
        );
        // And the row still starts its 6 pt under the strip, as before.
        assert!(
            (row_top - (strip_bottom + 6.0)).abs() <= 0.5,
            "row at strip + 6 ({row_top})"
        );
    }
}
