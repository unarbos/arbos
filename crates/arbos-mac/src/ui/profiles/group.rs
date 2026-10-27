//! A group's pane — a people group chat or a project shared with other
//! humans (redesign 2026-10-20, mockups b–d) on the shared profile pane
//! (D93): big picture, name, member count, two tabs. **Members** starts with
//! "Add members" (people and agents) and, on people chats, **Add agent**
//! (own agents only — [feedback app-01M5WKBR85JVW1KKJH96FWZVK5]), then
//! name + handle rows. **Library** is every image, then every file shared
//! here. The picture takes a crayon on hover, the name an inline field; no
//! actions row, no presence, no badges, no Leave (that is the chat list's
//! Delete — Jacob, 2026-10-20).

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, Context, Div, ElementId, Focusable, RenderImage, SharedString, Stateful,
};

use crate::app::{AppState, ProfilePage};
use crate::model::{Person, Project};
use crate::ui::avatars;
use crate::ui::blocked;
use crate::ui::groups;
use crate::ui::icons::icon;
use crate::ui::people;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, TEXT_BODY};

use super::pane::{self, Hero, Picture, ProfilePane, Section};
use super::{
    chats_of, collect, editable_name, hero_picture, kit, list_row, HeroClick, HERO_AVATAR,
};

pub(super) const TABS: [&str; 2] = ["Members", "Library"];

/// Everyone in the space, me first, then the others by handle.
fn members(state: &AppState, space: &Project) -> Vec<Person> {
    let mut out: Vec<Person> = Vec::new();
    let me_handle = state.me_handle();
    let me_id = state.me_id();
    if !me_handle.is_empty() {
        out.push(Person {
            id: me_id,
            handle: me_handle.clone(),
            name: state
                .account
                .as_ref()
                .map(|a| a.name.clone())
                .filter(|n| !n.trim().is_empty()),
            avatar_url: state
                .account
                .as_ref()
                .map(|a| a.avatar_url.clone())
                .unwrap_or_default(),
            status: "member".into(),
            role: "member".into(),
            last_seen_at: None,
            online: true,
            avatar_color_index: None,
        });
    }
    let mut others: Vec<Person> = space
        .people
        .iter()
        .filter(|p| p.handle != me_handle)
        .cloned()
        .collect();
    others.sort_by_key(|p| p.handle.clone());
    out.extend(others);
    out
}

pub(super) fn page(
    space_id: &str,
    shell: &Shell,
    cx: &mut Context<Shell>,
) -> Option<Stateful<Div>> {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let space = state.project(space_id)?.clone();
    let tab = state.profile_tab;
    // Agent pictures through the resolver, read while `state` is borrowed (the rows below
    // take `cx` mutably for their listeners).
    let agent_images = avatars::project_images(state);
    let me_handle = state.me_handle();
    let people = members(state, &space);
    let images: Vec<Option<Arc<RenderImage>>> =
        people.iter().map(|p| state.avatar(&p.avatar_url)).collect();
    // Agents through the shared identity (#1592): the picture their bubbles, the presence
    // row and the sidebar show — uploaded picture included.
    let agent_pictures: Vec<(avatars::AgentIdentity, Option<Arc<RenderImage>>)> = space
        .agents
        .iter()
        .map(|a| {
            let ident = avatars::member_identity(Some(&space), &state.projects, a);
            let image = ident.image_key.as_deref().and_then(|url| state.avatar(url));
            (ident, image)
        })
        .collect();
    let is_people = space.is_people();
    let renaming = state.profile_editing_name && is_people;
    // A people group: any member names it or gives it a picture (relay
    // `PATCH /chats/{id}`); a shared project keeps its owner's name.
    let editable = space.is_people_group();
    // Owners remove members (relay: chat owner for people, the agent's own
    // owner for agents); the row's ✕ shows on hover, then asks once.
    let i_own_group = if is_people {
        space.owner_handle.as_deref().is_none_or(|o| o == me_handle)
    } else {
        !space.is_guest()
    };
    let armed = shell.member_remove_armed.clone();
    let blocked_handles: Vec<String> = people
        .iter()
        .filter(|p| state.is_blocked(p))
        .map(|p| p.handle.clone())
        .collect();
    let content = collect(state, &chats_of(state, &space.id));

    // ----- hero: picture with the crayon, name as an inline field ----------------
    let picture = groups::picture(&space, state, HERO_AVATAR, &theme).into_any_element();
    let sid_pic = space.id.clone();
    let pic_click = editable.then(|| {
        let f = cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
            this.open_avatar_picker(crate::ui::avatar_picker::Target::Group(sid_pic.clone()), cx);
        });
        Box::new(f) as HeroClick
    });
    // A group I cannot edit: its picture opens full-size in the media viewer.
    let view_click = (!editable)
        .then(|| space.image_url.clone())
        .flatten()
        .map(|url| {
            let name = space.name.clone();
            let f = cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                this.view_picture(&name, &url, cx);
            });
            Box::new(f) as HeroClick
        });
    let avatar = hero_picture("profile-avatar", picture, pic_click, view_click).into_any_element();
    let sid_name = space.id.clone();
    let name_click = editable.then(|| {
        let f = cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
            this.start_rename_group(sid_name.clone(), window, cx);
        });
        Box::new(f) as HeroClick
    });
    let name = editable_name(
        "profile-name",
        &space.name,
        renaming,
        &shell.agent_name_input,
        name_click,
        &theme,
    )
    .into_any_element();
    // Subtitle: the member count (the person pane shows @handle here).
    let count = people.len();
    let subtitle = format!("{count} member{}", if count == 1 { "" } else { "s" });
    let hero = Hero {
        picture: Picture::Element(avatar),
        name,
        badge: None,
        subtitle: Some(subtitle.into()),
        subtitle_color: theme.text_muted,
        subtitle_click: None,
        subtitle_element: None,
        editor: None,
        action: None,
    };

    // No share disc on a group (Jacob 2026-10-25, [feedback
    // info-pane-no-share-disc-groups-2026-10-25]: "remove this button from
    // side bar on group/"): the × alone; Add members / Add agent stay the
    // first card.
    let share: Option<HeroClick> = None;

    let body: Vec<Section> = match tab {
        0 => {
            // Mock b: "Add members" is a card of its own above the member list.
            // People chats also get "Add agent" — own agents only, one tap
            // ([feedback app-01M5WKBR85JVW1KKJH96FWZVK5]: the flow was not
            // discoverable from the chat).
            let mut add_rows: Vec<gpui::AnyElement> = Vec::new();
            let mut list: Vec<gpui::AnyElement> = Vec::new();
            let sid = space.id.clone();
            let i_own = !space.is_guest();
            if is_people || i_own {
                let sid_members = sid.clone();
                add_rows.push(
                    kit::row(
                        "profile-add-members",
                        kit::Leading::Glyph("person-plus"),
                        "Add members",
                        None,
                        None,
                        None,
                        false,
                        &theme,
                    )
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_2))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let id = sid_members.clone();
                        // A people group gets the New group picker aimed at
                        // it; a shared project keeps the one-pick invite sheet.
                        if is_people {
                            this.state.update(cx, |s, cx| s.open_add_members(id, cx));
                        } else {
                            this.state.update(cx, |s, cx| s.open_invite(id, cx));
                        }
                        let handle = this.people_input.read(cx).focus_handle(cx);
                        window.focus(&handle, cx);
                    }))
                    .into_any_element(),
                );
            }
            if is_people {
                add_rows.push(
                    kit::row(
                        "profile-add-agent",
                        kit::Leading::Glyph("sparkles"),
                        "Add agent",
                        Some("One of yours, so everyone here can ask it".into()),
                        None,
                        None,
                        false,
                        &theme,
                    )
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_2))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.members_open = false;
                        this.add_agent_open = true;
                        cx.notify();
                    }))
                    .into_any_element(),
                );
            }
            for (i, p) in people.iter().enumerate() {
                let is_me = p.handle == me_handle;
                let title = if is_me {
                    format!("{} (you)", p.display_name())
                } else {
                    p.display_name()
                };
                let key = format!("user:{}", p.id);
                if armed.as_deref() == Some(key.as_str()) {
                    let (sid, uid) = (space.id.clone(), p.id.clone());
                    list.push(
                        remove_confirm(
                            &format!("Remove @{} from {}?", p.handle, space.name),
                            move |this, cx| {
                                let (sid, uid) = (sid.clone(), uid.clone());
                                this.state.update(cx, |s, cx| {
                                    if is_people {
                                        s.remove_member(sid, uid, cx);
                                    } else {
                                        s.remove_project_member(sid, uid, cx);
                                    }
                                });
                            },
                            &theme,
                            cx,
                        )
                        .into_any_element(),
                    );
                    continue;
                }
                let handle = p.handle.clone();
                let sid = space.id.clone();
                let removable = i_own_group && !is_me;
                // Blocked ([feedback blocked-state-mac-2026-10-26]): the tag on the row.
                let is_blocked = !is_me && blocked_handles.iter().any(|h| h == &p.handle);
                let trailing: Option<gpui::AnyElement> = match (is_blocked, removable) {
                    (true, true) => Some(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(blocked::tag(&theme))
                            .child(remove_x(key.clone(), &theme, cx))
                            .into_any_element(),
                    ),
                    (true, false) => Some(blocked::tag(&theme).into_any_element()),
                    (false, true) => Some(remove_x(key.clone(), &theme, cx).into_any_element()),
                    (false, false) => None,
                };
                list.push(
                    list_row(
                        ElementId::Name(SharedString::from(format!("profile-member-{}", p.handle))),
                        people::avatar(images[i].clone(), p, kit::AVATAR, &theme)
                            .into_any_element(),
                        title,
                        format!("@{}", p.handle),
                        theme.text_muted,
                        trailing,
                        &theme,
                    )
                    .debug_selector(|| format!("profile-member-{}", p.handle))
                    // One group per row: `group_hover` looks a group up by name.
                    .group(SharedString::from(format!("member-row-{key}")))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_2))
                    // Right-click (control-click) or Shift-click: the row's menu
                    // (View profile · Remove from group), `ui/member_menu.rs`.
                    .on_mouse_down(gpui::MouseButton::Right, {
                        let (sid, key) = (sid.clone(), key.clone());
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_member_menu(sid.clone(), key.clone(), event.position, cx);
                        })
                    })
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        if event.modifiers().shift || event.modifiers().control {
                            this.open_member_menu(sid.clone(), key.clone(), event.position(), cx);
                            return;
                        }
                        let (handle, sid) = (handle.clone(), sid.clone());
                        this.push_profile(
                            ProfilePage::User {
                                handle,
                                space_id: Some(sid),
                            },
                            cx,
                        );
                    }))
                    .into_any_element(),
                );
            }
            // Agents in the chat (people groups) — or the project's own agent.
            if is_people {
                for (ai, a) in space.agents.iter().enumerate() {
                    let pid = a.project_id.clone();
                    let mine = a.owner_handle == me_handle;
                    let sub = if mine {
                        "Agent · run by you".to_string()
                    } else {
                        format!("Agent · run by @{}", a.owner_handle)
                    };
                    let key = format!("agent:{}", a.project_id);
                    if armed.as_deref() == Some(key.as_str()) {
                        let (sid, pid) = (space.id.clone(), a.project_id.clone());
                        list.push(
                            remove_confirm(
                                &format!("Remove @{} from {}?", a.handle, space.name),
                                move |this, cx| {
                                    let (sid, pid) = (sid.clone(), pid.clone());
                                    this.state.update(cx, |s, cx| s.remove_agent(sid, pid, cx));
                                },
                                &theme,
                                cx,
                            )
                            .into_any_element(),
                        );
                        continue;
                    }
                    list.push(
                        list_row(
                            ElementId::Name(SharedString::from(format!(
                                "profile-agent-{}",
                                a.project_id
                            ))),
                            {
                                let (ident, image) = &agent_pictures[ai];
                                avatars::agent_picture(ident, image.clone(), kit::AVATAR, &theme)
                                    .into_any_element()
                            },
                            a.name.clone(),
                            sub,
                            theme.text_muted,
                            // The relay lets only the agent's own owner remove it.
                            mine.then(|| remove_x(key.clone(), &theme, cx).into_any_element()),
                            &theme,
                        )
                        .group(SharedString::from(format!("member-row-{key}")))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.surface_2))
                        .on_mouse_down(gpui::MouseButton::Right, {
                            let (sid, key) = (space.id.clone(), key.clone());
                            cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.open_member_menu(sid.clone(), key.clone(), event.position, cx);
                            })
                        })
                        .on_click({
                            let sid = space.id.clone();
                            cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                if event.modifiers().shift || event.modifiers().control {
                                    this.open_member_menu(
                                        sid.clone(),
                                        key.clone(),
                                        event.position(),
                                        cx,
                                    );
                                    return;
                                }
                                let pid = pid.clone();
                                this.push_profile(ProfilePage::Agent { project_id: pid }, cx);
                            })
                        })
                        .into_any_element(),
                    );
                }
            } else {
                let pid = space.id.clone();
                let sub = if space.is_guest() {
                    format!(
                        "Agent · run by @{}",
                        space.owner_handle.clone().unwrap_or_default()
                    )
                } else {
                    "Agent · run by you".to_string()
                };
                list.push(
                    list_row(
                        "profile-agent-self",
                        avatars::project_with(
                            agent_images.get(&space.id).cloned(),
                            &space,
                            kit::AVATAR,
                            &theme,
                        )
                        .into_any_element(),
                        space.name.clone(),
                        sub,
                        theme.text_muted,
                        None,
                        &theme,
                    )
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_2))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let pid = pid.clone();
                        this.push_profile(ProfilePage::Agent { project_id: pid }, cx);
                    }))
                    .into_any_element(),
                );
            }
            let mut out = Vec::new();
            if is_people {
                if let Some(desc) = space
                    .description
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                {
                    out.push(Section::Card(vec![div()
                        .px(px(kit::INSET))
                        .py(px(12.0))
                        .text_size(px(TEXT_BODY))
                        .text_color(theme.text)
                        .child(desc.to_owned())
                        .into_any_element()]));
                }
            }
            if !add_rows.is_empty() {
                out.push(Section::Card(add_rows));
            }
            out.push(Section::Card(list));
            out
        }
        _ => {
            // Library: one list of everything shared — images, files and links by
            // day ([feedback library-list-hover-mac-2026-10-25], `profiles/library.rs`;
            // links as in the agent pane, [feedback group-library-links-2026-10-23]).
            super::library::sections(content, shell, &theme, cx)
        }
    };

    Some(pane::render(
        ProfilePane {
            page: ProfilePage::Group {
                space_id: space.id.clone(),
            },
            hero,
            tabs: &TABS,
            selected: tab,
            body,
            footer: Vec::new(),
            done: None,
            share,
        },
        shell,
        cx,
    ))
}

/// The hover-only ✕ at a member row's trailing edge; click arms the confirm.
fn remove_x(key: String, theme: &Theme, cx: &mut Context<Shell>) -> Stateful<Div> {
    div()
        .id(ElementId::Name(SharedString::from(format!("remove-{key}"))))
        .size(px(24.0))
        .flex_shrink_0()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .opacity(0.0)
        .group_hover(SharedString::from(format!("member-row-{key}")), |s| {
            s.opacity(1.0)
        })
        .hover(|s| s.bg(theme.surface_2))
        .child(icon("xmark", 11.0, theme.text_muted))
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.member_remove_armed = Some(key.clone());
            cx.notify();
        }))
}

/// "Remove @x from Group?" in the row's place: Cancel keeps, Remove runs `go`.
fn remove_confirm(
    question: &str,
    go: impl Fn(&mut Shell, &mut Context<Shell>) + 'static,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Div {
    kit::confirm_row(
        question,
        "Remove",
        kit::confirm_button("member-remove-cancel", "Cancel", false, theme).on_click(cx.listener(
            |this, _, _, cx| {
                this.member_remove_armed = None;
                cx.notify();
            },
        )),
        kit::confirm_button("member-remove-go", "Remove", true, theme).on_click(cx.listener(
            move |this, _, _, cx| {
                this.member_remove_armed = None;
                go(this, cx);
                cx.notify();
            },
        )),
        theme,
    )
}
