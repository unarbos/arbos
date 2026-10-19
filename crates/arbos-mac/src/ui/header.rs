//! The conversation header, Grok Bot Mac layout: the avatar and name on the
//! left (click an agent to open its profile), then — for an agent — its chat
//! tabs ("Main", groups with their heads, "+" for a new chat, "×" archives),
//! icon buttons on the right ("…", and "add people" in a DM). Under it, one
//! optional chips row: the members of a people chat. The "…" menu holds what
//! the iPhone keeps there: New chat, Archived, Model, Members / Share, Avatar,
//! voice, Stop, Settings.

use gpui::{
    div, prelude::*, px, AnyElement, Context, Div, ElementId, Focusable as _, FontWeight,
    SharedString,
};

use crate::app::ProfilePage;
use crate::model::{CallKind, Chat, Project};
use crate::ui::avatars;
use crate::ui::callview;
use crate::ui::composer::{Composer, Picker};
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
use crate::ui::voicepicker;

pub fn render(
    shell: &Shell,
    space: &Project,
    sidebar_hidden: bool,
    cx: &mut Context<Shell>,
) -> Div {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let is_people = space.is_people();
    let in_call = shell.voice.read(cx).in_call();
    let menu_open = shell.menu_open;

    // ----- left: avatar + name (click an agent to open its profile) -----------
    let mut title = div()
        .id("header-title")
        .flex()
        .items_center()
        .gap(px(9.0))
        .h(px(34.0))
        .pl(px(4.0))
        .pr(px(10.0))
        .rounded(px(9.0))
        .min_w_0();
    if let Some(person) = space
        .person()
        .filter(|_| is_people && !space.is_people_group())
    {
        let image = state.avatar(&person.avatar_url);
        let handle = person.handle.clone();
        let sid = space.id.clone();
        title = title
            .cursor_pointer()
            .hover(|s| s.bg(theme.surface_2))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.menu_open = false;
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
        let sub = if space.pending {
            Some("Invited · opens when they sign in".to_string())
        } else if !space.agents.is_empty() {
            Some(format!(
                "@{} · {} agent{}",
                person.handle,
                space.agents.len(),
                if space.agents.len() == 1 { "" } else { "s" }
            ))
        } else {
            Some(format!("@{}", person.handle))
        };
        title = title
            .child(people::avatar(image, person, AVATAR_HEADER, &theme))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(TEXT_BODY))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .truncate()
                            .child(person.display_name()),
                    )
                    .when_some(sub, |el, s| {
                        el.child(
                            div()
                                .text_size(px(TEXT_TINY - 1.0))
                                .text_color(theme.text_muted)
                                .truncate()
                                .child(s),
                        )
                    }),
            );
    } else if space.is_people_group() {
        // A people group: stacked avatars, name, member count → group page.
        let stack: Vec<(
            crate::model::Person,
            Option<std::sync::Arc<gpui::RenderImage>>,
        )> = space
            .people
            .iter()
            .take(3)
            .map(|p| (p.clone(), state.avatar(&p.avatar_url)))
            .collect();
        let sid = space.id.clone();
        let members = space.people.len() + 1;
        title = title
            .child(people::avatar_stack(&stack, AVATAR_HEADER * 0.8, 3, &theme))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(TEXT_BODY))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .truncate()
                            .child(space.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_TINY - 1.0))
                            .text_color(theme.text_muted)
                            .truncate()
                            .child(format!(
                                "{members} members{}",
                                match space.agents.len() {
                                    0 => String::new(),
                                    n => format!(" · {n} agent{}", if n == 1 { "" } else { "s" }),
                                }
                            )),
                    ),
            )
            .cursor_pointer()
            .hover(|s| s.bg(theme.surface_2))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.menu_open = false;
                this.members_open = false;
                this.add_agent_open = false;
                let sid = sid.clone();
                this.state.update(cx, |s, cx| {
                    s.open_profile(ProfilePage::Group { space_id: sid }, cx)
                });
            }));
    } else {
        let avatar_state = state.avatar_state(&space.id);
        let streaming = state.project_is_streaming(&space.id);
        let machine = state.machine.shown_state();
        let pid = space.id.clone();
        let can_rename = !space.is_guest();
        let renaming = state
            .renaming_project
            .as_ref()
            .is_some_and(|p| p == &space.id);
        let sub = if streaming {
            Some(format!("{} is working", space.name))
        } else if space.is_guest() {
            Some(format!(
                "shared by @{}",
                space
                    .owner_handle
                    .clone()
                    .or_else(|| space.owner().map(|o| o.handle.clone()))
                    .unwrap_or_else(|| "owner".into())
            ))
        } else {
            None
        };
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
            div()
                .text_size(px(TEXT_BODY))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .truncate()
                .child(space.name.clone())
                .into_any_element()
        };
        title = title
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
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(name_el)
                    .when_some(sub, |el, s| {
                        el.child(
                            div()
                                .text_size(px(TEXT_TINY - 1.0))
                                .text_color(theme.text_muted)
                                .truncate()
                                .child(s),
                        )
                    }),
            );
        if avatar_state != avatars::AvatarState::Idle {
            PulseClock::request(cx);
        }
        // A group chat or a project with other people: the group page; my
        // own solo agent: its profile (with the tree + colour picker).
        // Double-click the name to rename (owned agents only).
        let groupish = space.is_groupish();
        title = title
            .cursor_pointer()
            .hover(|s| s.bg(theme.surface_2))
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    this.menu_open = false;
                    this.members_open = false;
                    this.add_agent_open = false;
                    let p = pid.clone();
                    if can_rename && event.click_count() >= 2 {
                        this.start_rename_agent(p, window, cx);
                        return;
                    }
                    this.state.update(cx, |s, cx| {
                        if groupish {
                            s.open_profile(ProfilePage::Group { space_id: p }, cx);
                        } else {
                            s.toggle_avatar_picker(p, cx);
                        }
                    });
                }),
            );
    }

    // ----- right: icon buttons --------------------------------------------------
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
    // Match the iPhone: no dual call / camera icons here. Start a call from
    // the composer (mic + phone) or the "…" menu; camera defaults off.
    let mut right = div().flex().items_center().gap(px(2.0));
    if is_people {
        right = right.child(
            button("header-members", "person-plus", shell.members_open, &theme).on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.members_open = !this.members_open;
                    this.add_agent_open = false;
                    this.menu_open = false;
                    cx.notify();
                }),
            ),
        );
    }
    if in_call {
        right = right.child(
            button("header-call", "phone", true, &theme)
                .bg(theme.danger_soft)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.voice.update(cx, |v, cx| v.toggle_call(cx));
                })),
        );
    }
    right = right.child(
        button("header-menu", "ellipsis", menu_open, &theme).on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.menu_open = !this.menu_open;
                this.members_open = false;
                this.add_agent_open = false;
                this.set_group_picker(false, cx);
                cx.notify();
            }),
        ),
    );

    let tabs = if is_people {
        None
    } else {
        agent_tabs(shell, space, cx)
    };
    // Sidebar toggle (⌘B): collapse when docked, bring it back when hidden
    // (docked when the window fits it, floating overlay when narrow).
    let overlay = shell.state.read(cx).sidebar_overlay;
    let sidebar_open = !sidebar_hidden || overlay;
    let sidebar_toggle = button("header-sidebar", "sidebar-left", sidebar_open, &theme).on_click(
        cx.listener(|this, _, window, cx| {
            this.menu_open = false;
            let width: f32 = window.viewport_size().width.into();
            this.state.update(cx, |s, cx| {
                let narrow = s.window_is_narrow(width);
                s.toggle_rail(narrow, cx)
            });
        }),
    );
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
                .flex()
                .items_center()
                .gap(px(4.0))
                .flex_shrink_0()
                .child(sidebar_toggle)
                .child(title.flex_shrink_0().max_w(px(300.0))),
        )
        .when_some(tabs, |el, t| el.child(t))
        .child(right.flex_shrink_0())
}

/// The agent's chat tabs, next to its name: every visible chat as a pill
/// (groups with their participants' heads), "×" on each but the pinned "Main"
/// (archives it), "+" for a new chat, and an "Archived" pill when there is
/// something in it. With multi-chat off: the primary, groups and the
/// Projects tab only, no "+", no "×", no Archived.
fn agent_tabs(shell: &Shell, space: &Project, cx: &mut Context<Shell>) -> Option<Div> {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let multi_chat = state.multi_chat;
    let tabs: Vec<(Chat, bool, bool)> = state
        .visible_tabs(&space.id)
        .into_iter()
        .map(|c| {
            (
                c.clone(),
                state.active_tab.get(&space.id) == Some(&c.id),
                state.chat_is_streaming(&c.id),
            )
        })
        .collect();
    let archived = state.archived_tabs(&space.id).len();
    if tabs.is_empty() {
        return None;
    }
    if !multi_chat && tabs.len() < 2 {
        return None;
    }
    let projects = state.projects.clone();
    // The tabs scroll; "+" and "Archived" stay put after them.
    let mut row = div()
        .id("chat-tabs")
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(6.0))
        .overflow_x_scroll();
    for (chat, active, streaming) in tabs {
        if streaming {
            PulseClock::request(cx);
        }
        let phase = PulseClock::phase(cx, 1200);
        let id = chat.id.clone();
        let close_id = chat.id.clone();
        let menu_chat_id = chat.id.clone();
        let ctrl_chat_id = chat.id.clone();
        let title = if chat.is_pinned {
            "Main".to_string()
        } else {
            chat.title.clone()
        };
        let closable = multi_chat && !chat.is_pinned;
        let mut pill = div()
            .id(ElementId::Name(SharedString::from(format!(
                "tab-{}",
                chat.id
            ))))
            .group("tab")
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(6.0))
            .h(px(28.0))
            .pl(px(11.0))
            .pr(px(if closable { 6.0 } else { 11.0 }))
            .rounded_full()
            .bg(if active {
                theme.bubble
            } else {
                theme.bubble_assistant
            })
            .text_size(px(TEXT_SMALL))
            // The active chip is a user bubble (dark in both appearances):
            // white text on it, like the bubbles themselves.
            .text_color(if active {
                theme.bubble_text()
            } else {
                theme.text_muted
            })
            .when(chat.unread && !active, |el| {
                el.font_weight(FontWeight::SEMIBOLD).text_color(theme.text)
            })
            .cursor_pointer()
            .when(!active, |el| el.hover(|s| s.text_color(theme.text)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.chat_menu = None;
                this.state.update(cx, |s, cx| s.select_tab(id.clone(), cx));
                Composer::focus(&this.composer, window, cx);
            }))
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.menu_open = false;
                    this.row_menu = None;
                    this.chat_menu = Some((menu_chat_id.clone(), event.position));
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
                    this.row_menu = None;
                    this.chat_menu = Some((ctrl_chat_id.clone(), event.position));
                    cx.notify();
                }),
            );
        if let Some(glyphs) = groups::tab_glyphs(&chat, &projects, &theme) {
            pill = pill.child(glyphs);
        }
        pill = pill.child(div().max_w(px(160.0)).truncate().child(title));
        if streaming {
            pill = pill.child(
                div()
                    .size(px(6.0))
                    .rounded_full()
                    .bg(Theme::with_alpha(theme.warn, 0.4 + 0.6 * phase)),
            );
        }
        if closable {
            pill = pill.child(
                div()
                    .id(ElementId::Name(SharedString::from(format!(
                        "tab-close-{}",
                        chat.id
                    ))))
                    .size(px(16.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.bg(theme.border_strong))
                    .child(icon(
                        "xmark",
                        9.0,
                        if active {
                            Theme::with_alpha(theme.bubble_text(), 0.8)
                        } else {
                            theme.text_muted
                        },
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.state.update(cx, |s, cx| {
                            s.close_tab(close_id.clone(), cx);
                        });
                    })),
            );
        }
        row = row.child(pill);
    }
    let mut strip = div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(row);
    if multi_chat {
        strip = strip.child(
            div()
                .id("tab-new")
                .flex_shrink_0()
                .size(px(28.0))
                .rounded_full()
                .bg(theme.bubble_assistant)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(theme.surface_2))
                .child(icon("plus", 13.0, theme.text_muted))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.state.update(cx, |s, cx| s.new_tab(cx));
                    Composer::focus(&this.composer, window, cx);
                })),
        );
        if archived > 0 {
            let open = shell.archived_open;
            strip = strip.child(
                div()
                    .id("tab-archived")
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .h(px(28.0))
                    .px(px(10.0))
                    .rounded_full()
                    .bg(if open {
                        theme.surface_2
                    } else {
                        gpui::transparent_black()
                    })
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_2).text_color(theme.text))
                    .child(icon("archivebox", 13.0, theme.text_muted))
                    .child(format!("Archived · {archived}"))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.archived_open = !this.archived_open;
                            this.menu_open = false;
                            cx.notify();
                        }),
                    ),
            );
        }
    }
    Some(strip)
}

/// The "Archived" dropdown: the agent's archived chats, each with "Unarchive".
pub fn archived_list(shell: &Shell, space: &Project, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let right = 16.0;
    let archived: Vec<Chat> = state
        .archived_tabs(&space.id)
        .into_iter()
        .cloned()
        .collect();
    let mut body = div().flex().flex_col().gap(px(1.0));
    body = body.child(
        div()
            .px(px(12.0))
            .pt(px(6.0))
            .pb(px(4.0))
            .text_size(px(TEXT_TINY))
            .text_color(theme.text_faint)
            .child(if archived.is_empty() {
                "Archived".to_string()
            } else {
                format!("Archived · {}", archived.len())
            }),
    );
    if archived.is_empty() {
        body = body.child(
            div()
                .px(px(12.0))
                .pb(px(10.0))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child("No archived chats. Close a tab with × to archive it."),
        );
    }
    for chat in archived {
        let id = chat.id.clone();
        let open_id = chat.id.clone();
        let menu_chat_id = chat.id.clone();
        let ctrl_chat_id = chat.id.clone();
        let preview = state
            .messages_of(&chat.id)
            .iter()
            .rev()
            .find(|m| !m.plain_text().trim().is_empty())
            .map(|m| crate::ui::sidebar::plain(&m.plain_text()))
            .unwrap_or_default();
        body = body.child(
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "archived-{}",
                    chat.id
                ))))
                .flex()
                .items_center()
                .gap(px(10.0))
                .h(px(44.0))
                .px(px(12.0))
                .rounded(px(9.0))
                .cursor_pointer()
                .hover(|s| s.bg(theme.surface_2))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.archived_open = false;
                    this.chat_menu = None;
                    this.state
                        .update(cx, |s, cx| s.unarchive_chat(open_id.clone(), cx));
                    Composer::focus(&this.composer, window, cx);
                }))
                .on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.menu_open = false;
                        this.row_menu = None;
                        this.chat_menu = Some((menu_chat_id.clone(), event.position));
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
                        this.row_menu = None;
                        this.chat_menu = Some((ctrl_chat_id.clone(), event.position));
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(TEXT_SMALL))
                                .text_color(theme.text)
                                .truncate()
                                .child(chat.title.clone()),
                        )
                        .when(!preview.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(px(TEXT_TINY))
                                    .text_color(theme.text_muted)
                                    .truncate()
                                    .child(preview),
                            )
                        }),
                )
                .child(
                    div()
                        .id(ElementId::Name(SharedString::from(format!(
                            "unarchive-{}",
                            chat.id
                        ))))
                        .flex_shrink_0()
                        .h(px(26.0))
                        .px(px(10.0))
                        .rounded_full()
                        .bg(theme.field)
                        .border_1()
                        .border_color(theme.field_border)
                        .flex()
                        .items_center()
                        .text_size(px(TEXT_TINY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .hover(|s| s.bg(theme.surface_2))
                        .child("Unarchive")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.archived_open = false;
                            this.state
                                .update(cx, |s, cx| s.unarchive_chat(id.clone(), cx));
                            Composer::focus(&this.composer, window, cx);
                        })),
                ),
        );
    }
    div()
        .absolute()
        .top(px(HEADER_HEIGHT - 4.0))
        .right(px(right))
        .child(glass::frosted_menu(320.0, &theme, body))
}

/// Chips under the header: agents + "Add agent" for a people chat. `None` for
/// an agent — its chat tabs live in the header (`agent_tabs`).
pub fn chips(shell: &Shell, space: &Project, cx: &mut Context<Shell>) -> Option<AnyElement> {
    if !space.is_people() {
        return None;
    }
    let theme = Theme::of(cx).clone();
    Some(people::member_chips(space, shell.add_agent_open, &theme, cx).into_any_element())
}

/// The "…" dropdown under the header's right circle.
pub fn menu(shell: &Shell, space: &Project, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let is_people = space.is_people();
    let streaming = state.project_is_streaming(&space.id);
    let voice_mode = state.voice_mode;
    let in_call = shell.voice.read(cx).in_call();
    let in_livekit = state.my_call.is_some();
    let projects_count = state.projects.iter().filter(|p| !p.is_people()).count();
    let name = if space.is_people_group() {
        space.name.clone()
    } else if is_people {
        space
            .person()
            .map(|p| format!("@{}", p.handle))
            .unwrap_or_else(|| space.name.clone())
    } else {
        space.name.clone()
    };
    let right = 16.0;

    let mut body = div().flex().flex_col().gap(px(1.0));
    if is_people {
        body = body
            .child(
                glass::menu_row("menu-members", "people", "Members & agents", false, &theme)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = false;
                        this.members_open = true;
                        cx.notify();
                    })),
            )
            .child(
                glass::menu_row(
                    "menu-add-agent",
                    "sparkles",
                    "Add one of your agents",
                    false,
                    &theme,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.menu_open = false;
                    this.add_agent_open = true;
                    cx.notify();
                })),
            );
        if let Some(person) = space.person() {
            let handle = person.handle.clone();
            let chat = space.id.clone();
            body = body.child(
                glass::menu_row(
                    "menu-block",
                    "xmark",
                    format!("Block @{}", person.handle),
                    true,
                    &theme,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.menu_open = false;
                    let h = handle.clone();
                    let c = chat.clone();
                    this.state.update(cx, |s, cx| {
                        s.block_user(h, cx);
                        s.leave_chat(c, cx);
                    });
                })),
            );
        }
    } else {
        let model = space.model.clone();
        let effort = space.effort.label();
        let multi_chat = state.multi_chat;
        if multi_chat {
            let archived = state.archived_tabs(&space.id).len();
            body = body
                .child(
                    glass::menu_row("menu-new-chat", "square-pencil", "New chat", false, &theme)
                        .child(
                            div()
                                .text_size(px(TEXT_TINY))
                                .text_color(theme.text_faint)
                                .child("⌘T"),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.menu_open = false;
                            this.state.update(cx, |s, cx| s.new_tab(cx));
                            Composer::focus(&this.composer, window, cx);
                        })),
                )
                .child(
                    glass::menu_row("menu-archived", "archivebox", "Archived", false, &theme)
                        .when(archived > 0, |el| {
                            el.child(
                                div()
                                    .text_size(px(TEXT_TINY))
                                    .text_color(theme.text_faint)
                                    .child(archived.to_string()),
                            )
                        })
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.menu_open = false;
                                this.archived_open = true;
                                cx.notify();
                            }),
                        ),
                );
        }
        if !space.is_guest() {
            body = body
                .child(
                    glass::menu_row(
                        "menu-model",
                        "cpu",
                        format!("Model · {model}"),
                        false,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = false;
                        this.composer.update(cx, |c, cx| {
                            c.open_picker = Some(Picker::Model);
                            cx.notify();
                        });
                    })),
                )
                .child(
                    glass::menu_row(
                        "menu-effort",
                        "waveform",
                        format!("Effort · {effort}"),
                        false,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = false;
                        this.composer.update(cx, |c, cx| {
                            c.open_picker = Some(Picker::Effort);
                            cx.notify();
                        });
                    })),
                )
                .child(
                    glass::menu_row(
                        "menu-voice",
                        "mic",
                        format!(
                            "Voice · {}",
                            voicepicker::current_label(space, &state.voices)
                        ),
                        false,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = false;
                        this.voice_picker_open = true;
                        this.state.update(cx, |s, _| s.fetch_voices());
                        cx.notify();
                    })),
                )
                .child({
                    // Voice API §11: may the agent look through my camera on calls.
                    let pid = space.id.clone();
                    let on = space.vision_enabled;
                    glass::menu_row(
                        "menu-vision",
                        "eye",
                        if on {
                            "Camera vision · on"
                        } else {
                            "Camera vision · off"
                        },
                        false,
                        &theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_open = false;
                        let p = pid.clone();
                        this.state
                            .update(cx, |s, cx| s.set_vision_enabled(p, !on, cx));
                    }))
                })
                .child(glass::menu_divider(&theme))
                .child(
                    glass::menu_row(
                        "menu-share",
                        "person-plus",
                        if space.guests().next().is_some() {
                            "Members"
                        } else {
                            "Share project…"
                        },
                        false,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.menu_open = false;
                        let has_guests = this
                            .state
                            .read(cx)
                            .selected()
                            .map(|p| p.guests().next().is_some())
                            .unwrap_or(false);
                        if has_guests {
                            this.members_open = true;
                            cx.notify();
                        } else {
                            let id = this.state.read(cx).selected_project.clone();
                            if let Some(id) = id {
                                this.people_input.update(cx, |i, cx| i.clear(cx));
                                this.state.update(cx, |s, cx| s.open_invite(id, cx));
                                let handle = this.people_input.read(cx).focus_handle(cx);
                                window.focus(&handle, cx);
                            }
                        }
                    })),
                )
                .child(
                    glass::menu_row("menu-avatar", "sparkles", "Avatar & colour", false, &theme)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu_open = false;
                            let id = this.state.read(cx).selected_project.clone();
                            if let Some(id) = id {
                                this.state
                                    .update(cx, |s, cx| s.toggle_avatar_picker(id, cx));
                            }
                        })),
                );
            if multi_chat && projects_count > 1 {
                body = body.child(
                    glass::menu_row(
                        "menu-group",
                        "people",
                        "Add a project to this chat",
                        false,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = false;
                        this.set_group_picker(true, cx);
                    })),
                );
            }
        } else {
            body = body.child(
                glass::menu_row("menu-members-guest", "people", "Members", false, &theme).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.menu_open = false;
                        this.members_open = true;
                        cx.notify();
                    }),
                ),
            );
        }
    }
    body = body
        .child(glass::menu_divider(&theme))
        .child(
            glass::menu_row(
                "menu-voice-mode",
                "waveform",
                if voice_mode {
                    "Voice mode · on"
                } else {
                    "Voice mode · off"
                },
                false,
                &theme,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.menu_open = false;
                this.voice.update(cx, |v, cx| v.toggle_voice_mode(cx));
            })),
        )
        .child(
            glass::menu_row(
                "menu-call",
                "phone",
                if in_livekit || in_call {
                    "Hang up"
                } else {
                    "Call"
                },
                false,
                &theme,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.menu_open = false;
                // LiveKit call first (same as the iPhone); else voice-mode hang-up /
                // start. Calls always begin audio-only; turn the camera on in-call.
                let live = this.state.read(cx).my_call.is_some();
                let voice_on = this.voice.read(cx).in_call();
                if live {
                    this.state.update(cx, |s, cx| s.leave_call(cx));
                } else if voice_on {
                    this.voice.update(cx, |v, cx| v.toggle_call(cx));
                } else if this
                    .state
                    .read(cx)
                    .selected()
                    .is_some_and(callview::callable)
                {
                    this.state
                        .update(cx, |s, cx| s.start_call_in_chat(CallKind::Voice, cx));
                }
            })),
        );
    if streaming {
        body = body.child(
            glass::menu_row("menu-stop", "stop", format!("Stop {name}"), true, &theme).on_click(
                cx.listener(|this, _, _, cx| {
                    this.menu_open = false;
                    this.state.update(cx, |s, cx| s.stop(cx));
                }),
            ),
        );
    }
    body = body
        .child(glass::menu_divider(&theme))
        .child(
            glass::menu_row(
                "menu-feedback",
                "square-pencil",
                "Send feedback",
                false,
                &theme,
            )
            .on_click(cx.listener(|this, _, window, cx| this.open_feedback(window, cx))),
        )
        .child(
            glass::menu_row("menu-settings", "gear", "Settings", false, &theme).on_click(
                cx.listener(|this, _, _, cx| {
                    this.menu_open = false;
                    this.settings_open = true;
                    cx.notify();
                }),
            ),
        );
    div()
        .absolute()
        .top(px(HEADER_HEIGHT - 4.0))
        .right(px(right))
        .child(glass::frosted_menu(250.0, &theme, body))
}
