//! The chat list: the iPhone home screen as a Mac sidebar. A top bar with the
//! account circle (its dot is the machine state), search and "+", then every
//! agent and person as one row — tree avatar, name, time, last line. The
//! Update pill sits at the bottom only while a newer build exists.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, Context, Div, ElementId, Focusable as _, FontWeight, RenderImage,
    SharedString,
};

use crate::app::AppState;
use crate::model::{Id, MachineState, Message, Project, Role};
use crate::ui::avatars;
use crate::ui::composer::Composer;
use crate::ui::frost::edge_faded;
use crate::ui::glass::{self, WindowDrag};
use crate::ui::icons::icon;
use crate::ui::motion::PulseClock;
use crate::ui::people;
use crate::ui::shell::Shell;
use crate::ui::theme::{
    Theme, AVATAR_ROW, PANEL_RADIUS, SIDEBAR_WIDTH, TEXT_BODY, TEXT_SMALL, TEXT_TINY,
};

/// The drag strip beside the traffic lights (the "+" sits in it on the right).
fn top_strip() -> f32 {
    if cfg!(target_os = "macos") {
        38.0
    } else {
        34.0
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

pub fn render(shell: &Shell, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let selected = state.selected_project.clone();
    let query = shell.search_input.read(cx).text().trim().to_lowercase();
    let machine = state.machine.shown_state();

    let mut rows: Vec<(u64, RowView)> = Vec::new();
    for p in state.projects.iter() {
        if !query.is_empty() {
            let hay = format!(
                "{} {}",
                p.name.to_lowercase(),
                p.person()
                    .map(|x| x.handle.to_lowercase())
                    .unwrap_or_default()
            );
            if !hay.contains(&query) {
                continue;
            }
        }
        let streaming = state.project_is_streaming(&p.id);
        let shown_people: Vec<_> = if p.is_guest() {
            p.owner().cloned().into_iter().collect()
        } else {
            p.guests().cloned().collect()
        };
        let row = RowView {
            selected: selected.as_deref() == Some(p.id.as_str()),
            streaming,
            unread: state.project_unread(&p.id) > 0,
            typing: state.typing_in(&p.id).is_some(),
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
        rows.push((last_activity(state, p), row));
    }
    rows.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    let empty = state.projects.is_empty();
    let link_connecting = state.link == crate::model::Link::Connecting;
    if machine == MachineState::Waking {
        PulseClock::request(cx);
    }

    // ----- top: "+" beside the traffic lights, then the search pill ------------
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
        .on_click(cx.listener(|this, _, window, cx| {
            this.menu_open = false;
            this.settings_open = false;
            this.people_input.update(cx, |i, cx| i.clear(cx));
            this.state.update(cx, |s, cx| s.open_new_chat(cx));
            let handle = this.people_input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }));
    let strip = div()
        .h(px(top_strip()))
        .flex_shrink_0()
        .flex()
        .items_end()
        .justify_end()
        .px(px(12.0))
        .pb(px(2.0))
        .child(plus)
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
        .pt(px(6.0))
        .pb(px(12.0))
        .overflow_y_scroll();
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
    } else if rows.is_empty() {
        list = list.child(
            div()
                .pt(px(40.0))
                .text_center()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(format!("Nothing matches “{query}”.")),
        );
    }
    for (_, row) in rows {
        list = list.child(render_row(row, shell, &theme, cx));
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
        .child(icon("gear", 14.0, theme.text_faint))
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.settings_open = !this.settings_open;
                this.menu_open = false;
                cx.notify();
            }),
        );

    let mut column = div()
        .relative()
        .h_full()
        .w(px(SIDEBAR_WIDTH))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .bg(theme.chrome())
        .border_r_1()
        .border_color(theme.border)
        .child(strip)
        .child(search)
        .child(edge_faded(14.0, true, true, list));
    if let Some(update) = shell.render_update_button(cx) {
        column = column.child(div().px(px(12.0)).pb(px(8.0)).child(update));
    }
    column.child(account_row)
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
    if streaming || typing || avatar_state != avatars::AvatarState::Idle {
        PulseClock::request(cx);
    }
    let phase = PulseClock::phase(cx, 900);

    // The picture: a coloured tree for an agent, the person's photo for a DM
    // (with a tiny tree badge when one of my agents sits in it).
    let picture: Div = if let Some(person) = space.person().filter(|_| space.is_people()) {
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
            .child(avatars::project(&space, AVATAR_ROW, theme));
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
    let handle = if space.is_people_group() {
        let n = space.people.len() + 1;
        Some(format!("{n} member{}", if n == 1 { "" } else { "s" }))
    } else {
        space
            .person()
            .filter(|_| space.is_people())
            .map(|p| format!("@{}", p.handle))
    };
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
        .gap(px(10.0))
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
                this.menu_open = false;
                this.row_menu = None;
                this.chat_menu = None;
                if can_rename && event.click_count() >= 2 {
                    this.state
                        .update(cx, |s, cx| s.select_project(id.clone(), cx));
                    this.start_rename_agent(id.clone(), window, cx);
                    return;
                }
                this.state
                    .update(cx, |s, cx| s.select_project(id.clone(), cx));
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
                        .when(streaming || typing, |el| {
                            el.child(div().size(px(7.0)).rounded_full().bg(Theme::with_alpha(
                                if typing { theme.online } else { theme.warn },
                                0.35 + 0.65 * phase,
                            )))
                        })
                        .when(!streaming && !typing && unread, |el| {
                            el.child(div().size(px(8.0)).rounded_full().bg(theme.accent))
                        }),
                ),
        )
}

/// The right-click menu of a sidebar row (Grok Mac, ref 48): profile, mute,
/// new chat, fork, delete — anchored at the click.
pub fn row_menu(shell: &Shell, cx: &mut Context<Shell>) -> Option<Div> {
    let (space_id, at) = shell.row_menu.clone()?;
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
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
    let chat_id = space.id.clone();
    let fork_id = space.id.clone();
    let delete_id = space.id.clone();
    let delete_people = is_people;

    let mut body = div().flex().flex_col().gap(px(1.0));
    body = body.child(
        glass::menu_row(
            "row-menu-profile",
            "person",
            if is_people {
                "Profile"
            } else {
                "Agent profile"
            },
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
    if !is_people {
        body = body.child(glass::menu_divider(&theme)).child(
            glass::menu_row(
                "row-menu-new-chat",
                "square-pencil",
                "New chat",
                false,
                &theme,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.row_menu = None;
                let id = chat_id.clone();
                this.state.update(cx, |s, cx| {
                    s.select_project(id, cx);
                    s.new_tab(cx);
                });
                Composer::focus(&this.composer, window, cx);
            })),
        );
    }
    if can_fork || can_delete {
        body = body.child(glass::menu_divider(&theme));
    }
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
    if can_delete {
        body = body.child(
            glass::menu_row("row-menu-delete", "trash", "Delete", true, &theme).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.row_menu = None;
                    let id = delete_id.clone();
                    this.state.update(cx, |s, cx| {
                        if delete_people {
                            s.delete_people_chat(id, cx);
                        } else {
                            s.delete_project(id, cx);
                        }
                    });
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

/// Right-click / control-click menu on a chat tab or an archived chat row:
/// Fork (the agent) and Delete (archive, or permanently close when already archived).
pub fn chat_menu(shell: &Shell, cx: &mut Context<Shell>) -> Option<Div> {
    let (chat_id, at) = shell.chat_menu.clone()?;
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
    let can_fork = !project.shared && !project.is_people();
    let can_delete = !chat.is_pinned;
    let fork_id = project.id.clone();
    let delete_id = chat.id.clone();
    let delete_archived = archived;

    let mut body = div().flex().flex_col().gap(px(1.0));
    if can_fork {
        body = body.child(
            glass::menu_row("chat-menu-fork", "branch", "Fork", false, &theme).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.chat_menu = None;
                    this.archived_open = false;
                    let id = fork_id.clone();
                    this.state.update(cx, |s, cx| s.duplicate_project(id, cx));
                }),
            ),
        );
    }
    if can_delete {
        body = body.child(
            glass::menu_row("chat-menu-delete", "trash", "Delete", true, &theme).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.chat_menu = None;
                    this.archived_open = false;
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
        );
    }
    if !can_fork && !can_delete {
        return None;
    }
    Some(
        div()
            .absolute()
            .left(at.x)
            .top(at.y)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(glass::frosted_menu(180.0, &theme, body)),
    )
}
