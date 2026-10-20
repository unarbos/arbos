//! The conversation header (unified top bar, Jacob 2026-10-20, mockups
//! `topbar-unify/`): one centred pill for every chat — avatar + name, the
//! same control for agents, people and groups. Click the name → that chat's
//! right pane. An agent with more than one tab folds them into the pill as a
//! second hit region, "Name · Tab ⌄": its dropdown lists the tabs (current
//! checked, last activity, × on hover closes), the previous chats, then
//! New tab ⌘T. Nothing else lives in the bar: no @handle line, no agent
//! chips, no add-person button, no member heads or counts. Calls start from
//! the composer; adding people or agents lives in the right pane's Members.

use gpui::{
    anchored, deferred, div, prelude::*, px, AnyElement, Context, Div, Edges, ElementId,
    FontWeight, SharedString, Stateful,
};

use crate::app::ProfilePage;
use crate::model::{Chat, Project};
use crate::ui::avatars;
use crate::ui::composer::Composer;
use crate::ui::glass::{self, WindowDrag};
use crate::ui::groups;
use crate::ui::icons::icon;
use crate::ui::motion::PulseClock;
use crate::ui::people;
use crate::ui::shell::Shell;
use crate::ui::sidebar::machine_dot;
use crate::ui::theme::{
    Theme, AVATAR_HEADER, CONTROL, HEADER_HEIGHT, TEXT_BODY, TEXT_SMALL, TEXT_TINY,
};

/// The pinned chat reads "Main" until it is renamed away from the agent's name.
pub(crate) fn tab_title(chat: &Chat, space: &Project) -> String {
    if chat.is_pinned && (chat.title.is_empty() || chat.title == space.name || chat.title == "Main")
    {
        "Main".to_string()
    } else {
        chat.title.clone()
    }
}

pub fn render(
    shell: &Shell,
    space: &Project,
    sidebar_hidden: bool,
    cx: &mut Context<Shell>,
) -> Div {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let is_people = space.is_people();

    // ----- the pill: avatar + name (click → the right pane) --------------------
    let mut pill = div()
        .id("header-title")
        .flex()
        .items_center()
        .h(px(34.0))
        .pl(px(4.0))
        .pr(px(10.0))
        .rounded_full()
        .bg(theme.bubble_assistant)
        .min_w_0()
        // The header is a window_drag region: a mouse-down that bubbles to it
        // arms a window move, and the slightest pointer jitter before the
        // mouse-up turns the click into a drag that never completes — the
        // second click of a double-click most of all (#1068). Stop it here and
        // do what the root would have on the way.
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.close_popovers(cx);
                if this.renaming_tab.is_some() {
                    this.commit_tab_rename(window, cx);
                }
            }),
        );

    let name_label = |text: String, theme: &Theme| {
        div()
            .text_size(px(TEXT_BODY))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.text)
            .truncate()
            .child(text)
    };
    // The name part: its own hit region so the tab segment can sit beside it.
    let mut name_part = div()
        .id("header-name")
        .flex()
        .items_center()
        .gap(px(9.0))
        .h_full()
        .min_w_0()
        .cursor_pointer()
        .rounded_full()
        .hover(|s| s.bg(theme.surface_2))
        .pr(px(4.0));

    if let Some(person) = space
        .person()
        .filter(|_| is_people && !space.is_people_group())
    {
        let image = state.avatar(&person.avatar_url);
        let handle = person.handle.clone();
        let sid = space.id.clone();
        name_part = name_part
            .child(people::avatar(image, person, AVATAR_HEADER, &theme))
            .child(name_label(person.display_name(), &theme))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.members_open = false;
                this.add_agent_open = false;
                let (handle, sid) = (handle.clone(), sid.clone());
                this.state.update(cx, |s, cx| {
                    s.open_profile(
                        ProfilePage::User {
                            handle,
                            space_id: Some(sid),
                        },
                        cx,
                    )
                });
            }));
    } else if space.is_people_group() {
        let sid = space.id.clone();
        name_part = name_part
            .child(groups::picture(space, state, AVATAR_HEADER, &theme))
            .child(name_label(space.name.clone(), &theme))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.members_open = false;
                this.add_agent_open = false;
                let sid = sid.clone();
                this.state.update(cx, |s, cx| {
                    s.open_profile(ProfilePage::Group { space_id: sid }, cx)
                });
            }));
    } else {
        let avatar_state = state.avatar_state(&space.id);
        let machine = state.machine.shown_state();
        let pid = space.id.clone();
        let can_rename = !space.is_guest();
        let renaming = state
            .renaming_project
            .as_ref()
            .is_some_and(|p| p == &space.id);
        let name_el = if renaming {
            div()
                .id("header-rename")
                .w(px(160.0))
                .h(px(22.0))
                .px(px(6.0))
                .rounded(px(6.0))
                .bg(theme.field)
                .border_1()
                .border_color(theme.field_border)
                .text_size(px(TEXT_BODY))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(shell.agent_name_input.clone())
                .into_any_element()
        } else {
            name_label(space.name.clone(), &theme).into_any_element()
        };
        if avatar_state != avatars::AvatarState::Idle {
            PulseClock::request(cx);
        }
        // A project with other people: the group pane; my own agent: its
        // profile. Double-click the name to rename (owned agents only) — so
        // a single click opens the page only after the double-click window
        // (#1068).
        let groupish = space.is_groupish();
        name_part = name_part
            .child(
                div()
                    .relative()
                    .size(px(AVATAR_HEADER))
                    .flex_shrink_0()
                    .child(avatars::project(space, AVATAR_HEADER, &theme))
                    .child(
                        div()
                            .absolute()
                            .right(px(-2.0))
                            .bottom(px(-2.0))
                            .size(px(10.0))
                            .rounded_full()
                            .bg(theme.bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .size(px(6.0))
                                    .rounded_full()
                                    .bg(machine_dot(machine, &theme)),
                            ),
                    ),
            )
            .child(name_el)
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    this.members_open = false;
                    this.add_agent_open = false;
                    let p = pid.clone();
                    if !can_rename {
                        this.open_from_name(p, groupish, cx);
                    } else if event.click_count() >= 2 {
                        this.start_rename_agent(p, window, cx);
                    } else {
                        this.name_clicked(p, groupish, cx);
                    }
                }),
            );
    }
    pill = pill.child(name_part);

    // ----- tabs fold into the pill: "· Tab ⌄" with the dropdown ---------------
    if !is_people {
        if let Some(segment) = tab_segment(shell, space, cx) {
            pill = pill.child(segment);
        }
    }

    // ----- the bar: equal side slots keep the pill centred ---------------------
    let button = |id: &'static str, glyph: &str, on: bool, theme: &Theme| {
        div()
            .id(id)
            .size(px(CONTROL))
            .flex_shrink_0()
            .rounded(px(8.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(if on {
                theme.surface_2
            } else {
                gpui::transparent_black()
            })
            .hover(|s| s.bg(theme.surface_2))
            .child(icon(
                glyph,
                17.0,
                if on { theme.text } else { theme.text_muted },
            ))
    };
    // Expand only when the rail is hidden (collapsed or too narrow). Collapse
    // lives next to "+" in the sidebar.
    let sidebar_toggle = sidebar_hidden.then(|| {
        let overlay = shell.state.read(cx).sidebar_overlay;
        button("header-sidebar", "sidebar-left", overlay, &theme).on_click(cx.listener(
            |this, _, window, cx| {
                let width: f32 = window.viewport_size().width.into();
                this.state.update(cx, |s, cx| {
                    let narrow = s.window_is_narrow(width);
                    s.toggle_rail(narrow, cx)
                });
            },
        ))
    });
    let side = px(CONTROL + 8.0);
    div()
        .h(px(HEADER_HEIGHT))
        .flex_shrink_0()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .pl(px(12.0))
        .pr(px(10.0))
        .border_b_1()
        .border_color(theme.border)
        .window_drag()
        .child(
            div()
                .w(side)
                .flex_shrink_0()
                .flex()
                .items_center()
                .children(sidebar_toggle),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .justify_center()
                .child(pill.flex_shrink_0().max_w(px(460.0))),
        )
        .child(div().w(side).flex_shrink_0())
}

/// "· Tab ⌄" inside the pill for an agent with more than one tab (a single
/// tab shows the name only). Click toggles the dropdown, which hangs off it.
fn tab_segment(shell: &Shell, space: &Project, cx: &mut Context<Shell>) -> Option<Stateful<Div>> {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let tabs = state.visible_tabs(&space.id);
    if tabs.len() < 2 {
        return None;
    }
    let active = state.active_tab.get(&space.id).cloned();
    let current = tabs
        .iter()
        .find(|c| Some(&c.id) == active.as_ref())
        .or(tabs.first())
        .map(|c| tab_title(c, space))
        .unwrap_or_else(|| "Main".into());
    let streaming = active
        .as_deref()
        .is_some_and(|id| state.chat_is_streaming(id));
    if streaming {
        PulseClock::request(cx);
    }
    let phase = PulseClock::phase(cx, 1200);
    let open = shell.tab_menu_open;
    Some(
        div()
            .id("header-tab")
            .relative()
            .flex()
            .items_center()
            .gap(px(5.0))
            .h(px(26.0))
            .ml(px(2.0))
            .pl(px(9.0))
            .pr(px(7.0))
            .rounded_full()
            .bg(if open {
                theme.surface_2
            } else {
                theme.surface_1
            })
            .text_size(px(TEXT_SMALL))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.text)
            .cursor_pointer()
            .hover(|s| s.bg(theme.surface_2))
            .child(
                div()
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_faint)
                    .child("·"),
            )
            .child(div().max_w(px(160.0)).truncate().child(current))
            .when(streaming, |el| {
                el.child(
                    div()
                        .size(px(6.0))
                        .rounded_full()
                        .bg(Theme::with_alpha(theme.warn, 0.4 + 0.6 * phase)),
                )
            })
            .child(icon("chevron-down", 11.0, theme.text_muted))
            // Mouse-down, not click: the root closes every popover on
            // mouse-down, so a click handler could never toggle this one shut.
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.menu_open = false;
                    if this.renaming_tab.is_some() {
                        this.commit_tab_rename(window, cx);
                    }
                    this.tab_menu_open = !this.tab_menu_open;
                    cx.notify();
                }),
            )
            .when(open, |el| {
                // Hangs under the segment: an absolute wrapper places it (taffy
                // positions it relative to the segment), the anchored child
                // keeps it inside the window, deferred paints it over the chat.
                el.child(
                    div().absolute().left(px(-8.0)).top(px(34.0)).child(
                        deferred(
                            anchored()
                                .position_mode(gpui::AnchoredPositionMode::Local)
                                .snap_to_window_with_margin(Edges::all(px(8.0)))
                                .child(glass::popover(tab_dropdown(shell, space, cx))),
                        )
                        .with_priority(1),
                    ),
                )
            }),
    )
}

/// The dropdown (mockup b): every visible tab — current checked, its last
/// activity, × on hover (closes; the pinned Main cannot) — then the previous
/// (archived) chats, a separator and New tab ⌘T.
fn tab_dropdown(shell: &Shell, space: &Project, cx: &mut Context<Shell>) -> impl IntoElement {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let multi_chat = state.multi_chat;
    let active = state.active_tab.get(&space.id).cloned();
    let tabs: Vec<Chat> = state.visible_tabs(&space.id).into_iter().cloned().collect();
    let mut previous: Vec<Chat> = state
        .archived_tabs(&space.id)
        .into_iter()
        .cloned()
        .collect();
    previous.sort_by_key(|c| std::cmp::Reverse(c.last_message_at));
    let projects = state.projects.clone();
    let renaming = shell.renaming_tab.clone();

    let mut body = div().flex().flex_col().gap(px(1.0));
    let mut list = div()
        .id("tab-menu-tabs")
        .flex()
        .flex_col()
        .gap(px(1.0))
        .max_h(px(360.0))
        .overflow_y_scroll();
    for chat in tabs {
        let is_active = Some(&chat.id) == active.as_ref();
        let editing = renaming.as_ref() == Some(&chat.id);
        let closable = multi_chat && !chat.is_pinned && !editing;
        let title = tab_title(&chat, space);
        let id = chat.id.clone();
        let close_id = chat.id.clone();
        let menu_chat_id = chat.id.clone();
        let ctrl_chat_id = chat.id.clone();
        let rename_id = chat.id.clone();
        let rename_label = title.clone();
        let key = chat.id.clone();
        let mut row = div()
            .id(ElementId::Name(SharedString::from(format!(
                "tab-menu-{}",
                chat.id
            ))))
            .group(SharedString::from(format!("tab-row-{key}")))
            .flex()
            .items_center()
            .gap(px(10.0))
            .h(px(36.0))
            .pl(px(10.0))
            .pr(px(8.0))
            .rounded(px(9.0))
            .text_size(px(TEXT_SMALL))
            .text_color(theme.text)
            .bg(if is_active {
                theme.surface_2
            } else {
                gpui::transparent_black()
            })
            .cursor_pointer()
            .hover(|s| s.bg(theme.surface_2))
            .child(
                div()
                    .w(px(14.0))
                    .flex_shrink_0()
                    .flex()
                    .justify_center()
                    .when(is_active, |el| el.child(icon("check", 12.0, theme.text))),
            );
        if editing {
            row = row
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(24.0))
                        .px(px(6.0))
                        .rounded(px(6.0))
                        .bg(theme.field)
                        .border_1()
                        .border_color(theme.field_border)
                        .child(shell.tab_rename_input.clone()),
                );
            list = list.child(row);
            continue;
        }
        if let Some(glyphs) = groups::tab_glyphs(&chat, &projects, &theme) {
            row = row.child(glyphs);
        }
        row = row
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(chat.unread && !is_active, |el| {
                        el.font_weight(FontWeight::SEMIBOLD)
                    })
                    .child(title),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(TEXT_TINY))
                    .text_color(theme.text_faint)
                    .child(crate::ui::sidebar::time_label(chat.last_message_at)),
            );
        if closable {
            row = row.child(
                div()
                    .id(ElementId::Name(SharedString::from(format!(
                        "tab-close-{}",
                        chat.id
                    ))))
                    .size(px(18.0))
                    .flex_shrink_0()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(0.0)
                    .group_hover(SharedString::from(format!("tab-row-{key}")), |s| {
                        s.opacity(1.0)
                    })
                    .hover(|s| s.bg(theme.border_strong))
                    .child(icon("xmark", 9.0, theme.text_muted))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.state.update(cx, |s, cx| {
                            s.close_tab(close_id.clone(), cx);
                        });
                    })),
            );
        } else {
            row = row.child(div().size(px(18.0)).flex_shrink_0());
        }
        row = row
            // Right-click / control-click: the chat menu (Rename, Fork, Delete).
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.tab_menu_open = false;
                    this.row_menu = None;
                    this.chat_menu = Some((menu_chat_id.clone(), event.position));
                    cx.notify();
                }),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    if event.modifiers.control {
                        this.tab_menu_open = false;
                        this.row_menu = None;
                        this.chat_menu = Some((ctrl_chat_id.clone(), event.position));
                        cx.notify();
                        return;
                    }
                    if event.click_count >= 2 && this.renaming_tab.as_ref() != Some(&rename_id) {
                        this.begin_tab_rename(rename_id.clone(), rename_label.clone(), window, cx);
                    }
                }),
            )
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    if event.modifiers().control
                        || event.click_count() >= 2
                        || this.renaming_tab.as_ref() == Some(&id)
                    {
                        return;
                    }
                    this.tab_menu_open = false;
                    this.chat_menu = None;
                    this.state.update(cx, |s, cx| s.select_tab(id.clone(), cx));
                    Composer::focus(&this.composer, window, cx);
                }),
            );
        list = list.child(row);
    }
    body = body.child(list);

    if !previous.is_empty() {
        body = body.child(glass::menu_divider(&theme)).child(
            div()
                .px(px(12.0))
                .pt(px(4.0))
                .pb(px(2.0))
                .text_size(px(TEXT_TINY))
                .text_color(theme.text_faint)
                .child("Previous chats"),
        );
        let mut list = div()
            .id("tab-menu-previous")
            .flex()
            .flex_col()
            .gap(px(1.0))
            .max_h(px(240.0))
            .overflow_y_scroll();
        for chat in previous {
            let id = chat.id.clone();
            let menu_chat_id = chat.id.clone();
            list = list.child(
                div()
                    .id(ElementId::Name(SharedString::from(format!(
                        "tab-menu-previous-{}",
                        chat.id
                    ))))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .h(px(36.0))
                    .pl(px(34.0))
                    .pr(px(12.0))
                    .rounded(px(9.0))
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_2))
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.tab_menu_open = false;
                            this.row_menu = None;
                            this.chat_menu = Some((menu_chat_id.clone(), event.position));
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(chat.title.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(TEXT_TINY))
                            .text_color(theme.text_faint)
                            .child(crate::ui::sidebar::time_label(chat.last_message_at)),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.tab_menu_open = false;
                        let id = id.clone();
                        this.state.update(cx, |s, cx| s.unarchive_chat(id, cx));
                        Composer::focus(&this.composer, window, cx);
                    })),
            );
        }
        body = body.child(list);
    }

    if multi_chat {
        body = body.child(glass::menu_divider(&theme)).child(
            glass::menu_row("tab-menu-new", "plus", "New tab", false, &theme)
                .child(
                    div()
                        .text_size(px(TEXT_TINY))
                        .text_color(theme.text_faint)
                        .child("⌘T"),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.tab_menu_open = false;
                    this.open_new_tab(window, cx);
                })),
        );
    }
    glass::frosted_menu(280.0, &theme, body)
}

/// Nothing under the header any more (2026-10-20): agents of a DM or group
/// sit in the right pane's Members list.
pub fn chips(_shell: &Shell, _space: &Project, _cx: &mut Context<Shell>) -> Option<AnyElement> {
    None
}
