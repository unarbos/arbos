//! A group's pane — a people group chat or a project shared with other
//! humans (redesign 2026-10-20, mockups b–d) on the shared profile pane
//! (D93): big picture, name, member count, two tabs. **Members** starts with
//! ONE add row — "Add members", whose picker lists Agents (yours) and People
//! (Jacob 2026-10-27, [feedback add-your-agent-row-removal-mac-2026-10-27]:
//! the separate "Add agent" row is gone) — then, on a people group with a
//! description, a labelled Description row in the same card, then name +
//! handle rows. **Library** is every image, then every file shared here.
//! The picture takes a crayon on hover, the name an inline field; no
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
use crate::ui::highlight;
use crate::ui::people;
use crate::ui::sf_symbols;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, TEXT_SMALL};

use super::pane::{self, Hero, Picture, ProfilePane, Section};
use super::{
    chats_of, collect, editable_name, hero_picture, kit, list_row, HeroClick, HERO_AVATAR,
};

pub(super) const TABS: [&str; 2] = ["Members", "Library"];

/// The Members tab's one add row (element id and debug selector).
pub(crate) const ADD_MEMBERS_ID: &str = "profile-add-members";
/// The labelled Description row under it on a people group that has one.
pub(crate) const DESCRIPTION_ID: &str = "profile-description";
/// The description value's width cap: the card is 308 wide (pane 340 − 2 × 16),
/// the glyph tile + gap + "Description" take ~110; 180 leaves the title whole.
const DESCRIPTION_VALUE_MAX_W: f32 = 180.0;

/// A people group's description, trimmed; `None` when empty or not a people
/// chat (a shared project's description belongs to its agent pane).
pub(crate) fn description_of(space: &Project) -> Option<&str> {
    if !space.is_people() {
        return None;
    }
    space
        .description
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

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
    // A people group: its owner and admins name it or give it a picture (relay
    // `PATCH /chats/{id}`, owner/admin only since 2026-10-27 — members had renamed
    // Arbonites); plain members see the pane read-only. A shared project keeps its
    // owner's name.
    let editable = space.may_edit_group_settings(&me_handle);
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
        subtitle_color: theme.label_muted(),
        subtitle_click: None,
        subtitle_element: None,
        editor: None,
        action: None,
    };

    // No share disc on a group (Jacob 2026-10-25, [feedback
    // info-pane-no-share-disc-groups-2026-10-25]: "remove this button from
    // side bar on group/"): the × alone; the Add members card stays first.
    let share: Option<HeroClick> = None;

    let body: Vec<Section> = match tab {
        0 => {
            // Mock b: "Add members" is a card of its own above the member list —
            // the one way in for people AND agents (its picker has an Agents
            // section). The second "Add agent" row is gone (Jacob 2026-10-27).
            let mut add_rows: Vec<gpui::AnyElement> = Vec::new();
            let mut list: Vec<gpui::AnyElement> = Vec::new();
            let sid = space.id.clone();
            let i_own = !space.is_guest();
            if is_people || i_own {
                let sid_members = sid.clone();
                add_rows.push(
                    kit::row(
                        ADD_MEMBERS_ID,
                        kit::Leading::Glyph("person.badge.plus"),
                        "Add members",
                        None,
                        None,
                        None,
                        false,
                        &theme,
                    )
                    .debug_selector(|| ADD_MEMBERS_ID.into())
                    .cursor_pointer()
                    .group(kit::HOT)
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
            // A people group's description, as the iPhone shows it: a labelled
            // row in the first card. It used to be a bare text card above the
            // add rows — "AR" alone in a block read as a stray element (Jacob
            // 2026-10-27). The Mac has no editor for it; the iPhone's Edit
            // group sheet is the one writer.
            if let Some(desc) = description_of(&space) {
                // The value takes what the title leaves and clips at its end
                // (the iPhone row's rule); `kit::row`'s detail slot never shrinks.
                let value = div()
                    .min_w_0()
                    .max_w(px(DESCRIPTION_VALUE_MAX_W))
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.label_muted())
                    .truncate()
                    .child(desc.to_owned())
                    .into_any_element();
                add_rows.push(
                    kit::row(
                        DESCRIPTION_ID,
                        kit::Leading::Glyph("note.text"),
                        "Description",
                        None,
                        None,
                        Some(value),
                        false,
                        &theme,
                    )
                    .debug_selector(|| DESCRIPTION_ID.into())
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
                        theme.label_muted(),
                        trailing,
                        &theme,
                    )
                    .debug_selector(|| format!("profile-member-{}", p.handle))
                    // The row's group lights its pill and shows its ✕ (`group_hover`
                    // resolves the innermost group of that name).
                    .group(kit::HOT)
                    .cursor_pointer()
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
                            theme.label_muted(),
                            // The relay lets only the agent's own owner remove it.
                            mine.then(|| remove_x(key.clone(), &theme, cx).into_any_element()),
                            &theme,
                        )
                        .group(kit::HOT)
                        .cursor_pointer()
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
                        theme.label_muted(),
                        None,
                        &theme,
                    )
                    .cursor_pointer()
                    .group(kit::HOT)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let pid = pid.clone();
                        this.push_profile(ProfilePage::Agent { project_id: pid }, cx);
                    }))
                    .into_any_element(),
                );
            }
            let mut out = Vec::new();
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
    let x = div()
        .id(ElementId::Name(SharedString::from(format!("remove-{key}"))))
        .size(px(24.0))
        .flex_shrink_0()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .opacity(0.0)
        .group_hover(kit::HOT, |s| s.opacity(1.0));
    highlight::button(x, theme)
        .child(sf_symbols::glyph("xmark", 11.0, theme.label_muted()))
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

#[cfg(test)]
mod tests {
    use super::description_of;
    use crate::model::{Project, SpaceKind};

    /// The Description row is for a people group's own text; a shared project's
    /// description is its agent's and stays on the agent pane. Blank = no row.
    #[test]
    fn only_a_people_groups_description_makes_a_row() {
        let group = |desc: Option<&str>, kind: SpaceKind| Project {
            kind,
            description: desc.map(str::to_owned),
            ..Project::default()
        };
        assert_eq!(
            description_of(&group(Some("AR"), SpaceKind::Group)),
            Some("AR")
        );
        assert_eq!(
            description_of(&group(Some("  Launch week  "), SpaceKind::Group)),
            Some("Launch week")
        );
        assert_eq!(description_of(&group(Some("   "), SpaceKind::Group)), None);
        assert_eq!(description_of(&group(None, SpaceKind::Group)), None);
        assert_eq!(
            description_of(&group(Some("an agent's blurb"), SpaceKind::Project)),
            None
        );
    }
}

#[cfg(test)]
mod render_tests {
    //! The real `Shell` over a real `AppState`, headless ([feedback
    //! add-your-agent-row-removal-mac-2026-10-27]): every chat kind's info pane
    //! draws ONE add row — "Add members" — and no "Add agent" / "Add your
    //! agent" row; the people group's description is a labelled row in that
    //! card, not a bare block; the Add members picker opens with an Agents
    //! section (mine first, the other members' agents after, greyed) above
    //! People; an empty people chat has no "Add agent" control.
    use futures::future::BoxFuture;
    use gpui::{
        point, px, size, AppContext, Bounds, Entity, Modifiers, Pixels, TestAppContext,
        VisualTestContext,
    };
    use tokio::sync::mpsc;

    use super::{ADD_MEMBERS_ID, DESCRIPTION_ID};
    use crate::app::{AppState, ProfilePage};
    use crate::model::{AccountView, AgentMember, Person, Project, ServerInfo, SpaceKind};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::motion::PulseClock;
    use crate::ui::people::{AGENTS_SECTION_ID, AGENTS_SECTION_LABEL};
    use crate::ui::shell::Shell;
    use crate::ui::theme::Theme;

    /// The removed rows' element ids — nothing may draw them any more.
    const GONE: [&str; 2] = ["profile-add-agent", "empty-add-agent"];

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

    fn person(id: &str, handle: &str, name: &str) -> Person {
        Person {
            id: id.into(),
            handle: handle.into(),
            name: Some(name.into()),
            status: "member".into(),
            role: "member".into(),
            ..Default::default()
        }
    }

    fn agent_member(project_id: &str, name: &str, owner_handle: &str) -> AgentMember {
        AgentMember {
            project_id: project_id.into(),
            name: name.into(),
            handle: Project::mention_slug(name),
            owner_id: format!("owner-of-{owner_handle}"),
            owner_handle: owner_handle.into(),
            ..Default::default()
        }
    }

    /// Me (unconst) with two agents (Comms, Research); a DM with Sam; a people
    /// group "Launch week" (Sam, Priya; Comms is in it, and so is Priya's
    /// agent Ledger) with the description "AR"; a shared project of mine with
    /// Sam in it; a server I own with Sam.
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
            cx.set_reduce_motion(true);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        state.update(cx, |s, cx| {
            s.account = Some(AccountView {
                id: "gh_1".into(),
                login: "unconst".into(),
                name: "Jacob".into(),
                avatar_url: String::new(),
                device_name: "test".into(),
                balance_usd: 0.0,
                spent_today_usd: 0.0,
                daily_cap_usd: None,
            });
            s.projects.push(Project {
                id: "a-comms".into(),
                name: "Comms".into(),
                ..Project::default()
            });
            s.projects.push(Project {
                id: "a-research".into(),
                name: "Research".into(),
                ..Project::default()
            });
            s.projects.push(Project {
                id: "d1".into(),
                name: "Sam Rivera".into(),
                kind: SpaceKind::Dm,
                people: vec![person("gh_2", "samir", "Sam Rivera")],
                ..Project::default()
            });
            s.projects.push(Project {
                id: "g1".into(),
                name: "Launch week".into(),
                kind: SpaceKind::Group,
                description: Some("AR".into()),
                people: vec![
                    person("gh_2", "samir", "Sam Rivera"),
                    person("gh_3", "priya", "Priya Natarajan"),
                ],
                agents: vec![
                    agent_member("a-comms", "Comms", "unconst"),
                    agent_member("a-ledger", "Ledger", "priya"),
                ],
                ..Project::default()
            });
            s.projects.push(Project {
                id: "p-shared".into(),
                name: "Shared build".into(),
                people: vec![person("gh_2", "samir", "Sam Rivera")],
                ..Project::default()
            });
            s.servers.push(ServerInfo {
                id: "srv1".into(),
                name: "Arbonites".into(),
                my_role: "owner".into(),
                members: vec![
                    Person {
                        role: "owner".into(),
                        ..person("gh_1", "unconst", "Jacob")
                    },
                    person("gh_2", "samir", "Sam Rivera"),
                ],
                ..ServerInfo::default()
            });
            s.selected_project = Some("g1".into());
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.simulate_resize(size(px(1180.0), px(780.0)));
        vcx.run_until_parked();
        (rt, state, shell, vcx)
    }

    fn settle(shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        for _ in 0..3 {
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    fn centre(b: Bounds<Pixels>) -> gpui::Point<Pixels> {
        point(b.left() + b.size.width / 2.0, b.top() + b.size.height / 2.0)
    }

    fn open(state: &Entity<AppState>, vcx: &mut VisualTestContext, page: ProfilePage, tab: usize) {
        state.update(vcx, |s, cx| {
            s.open_profile(page, cx);
            s.set_profile_tab(tab, cx);
        });
    }

    fn assert_none_of_the_gone_rows(vcx: &mut VisualTestContext, kind: &str) {
        for id in GONE {
            assert!(
                vcx.debug_bounds(id).is_none(),
                "{kind}: `{id}` is gone for good"
            );
        }
    }

    #[gpui::test]
    fn every_chat_kinds_pane_draws_one_add_row_and_no_add_agent_row(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);

        // People group, Members tab: Add members alone (the "Add agent" row is gone).
        open(
            &state,
            vcx,
            ProfilePage::Group {
                space_id: "g1".into(),
            },
            0,
        );
        settle(&shell, vcx);
        assert!(
            vcx.debug_bounds(ADD_MEMBERS_ID).is_some(),
            "group: Add members"
        );
        assert_none_of_the_gone_rows(vcx, "group");

        // A shared project I own, shown as a group: Add members alone.
        open(
            &state,
            vcx,
            ProfilePage::Group {
                space_id: "p-shared".into(),
            },
            0,
        );
        settle(&shell, vcx);
        assert!(
            vcx.debug_bounds(ADD_MEMBERS_ID).is_some(),
            "shared project: Add members"
        );
        assert_none_of_the_gone_rows(vcx, "shared project");

        // A server I own, Members tab: its one Add members row (people only).
        open(
            &state,
            vcx,
            ProfilePage::Server {
                server_id: "srv1".into(),
            },
            1,
        );
        settle(&shell, vcx);
        assert!(
            vcx.debug_bounds(super::super::server::ADD_MEMBERS_ID)
                .is_some(),
            "server: Add members"
        );
        assert_none_of_the_gone_rows(vcx, "server");

        // A person's pane opened from a DM (the 1:1 chat's info pane): no add
        // row of any kind — and none on the same pane opened from a group.
        for space in ["d1", "g1"] {
            open(
                &state,
                vcx,
                ProfilePage::User {
                    handle: "samir".into(),
                    space_id: Some(space.into()),
                },
                0,
            );
            settle(&shell, vcx);
            assert!(
                vcx.debug_bounds(ADD_MEMBERS_ID).is_none(),
                "person pane from {space}: no Add members"
            );
            assert_none_of_the_gone_rows(vcx, "person pane");
        }
    }

    #[gpui::test]
    fn an_empty_people_chat_has_no_add_agent_control(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        for space in ["d1", "g1"] {
            state.update(vcx, |s, cx| {
                s.close_profile(cx);
                s.select_project(space.into(), cx);
            });
            settle(&shell, vcx);
            assert_none_of_the_gone_rows(vcx, "empty chat");
        }
    }

    #[gpui::test]
    fn the_description_is_a_labelled_row_in_the_add_card(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        open(
            &state,
            vcx,
            ProfilePage::Group {
                space_id: "g1".into(),
            },
            0,
        );
        settle(&shell, vcx);
        let add = vcx.debug_bounds(ADD_MEMBERS_ID).expect("Add members row");
        let desc = vcx
            .debug_bounds(DESCRIPTION_ID)
            .expect("Description row for \"AR\"");
        assert!(
            desc.top() >= add.bottom() - px(1.0) && desc.top() <= add.bottom() + px(2.0),
            "right under Add members in the same card (rows touch): add {add:?} desc {desc:?}"
        );
        assert_eq!(desc.left(), add.left(), "the same card's left edge");
        assert_eq!(
            desc.size.width, add.size.width,
            "full card width, like every row"
        );

        // No description: no row, nothing else moves.
        state.update(vcx, |s, cx| {
            if let Some(p) = s.projects.iter_mut().find(|p| p.id == "g1") {
                p.description = Some("   ".into());
            }
            cx.notify();
        });
        settle(&shell, vcx);
        assert!(vcx.debug_bounds(DESCRIPTION_ID).is_none(), "blank = no row");
        assert_eq!(
            vcx.debug_bounds(ADD_MEMBERS_ID).expect("Add members").top(),
            add.top()
        );
    }

    #[gpui::test]
    fn add_members_opens_the_picker_with_an_agents_section_first(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        open(
            &state,
            vcx,
            ProfilePage::Group {
                space_id: "g1".into(),
            },
            0,
        );
        settle(&shell, vcx);
        assert!(
            vcx.debug_bounds(AGENTS_SECTION_ID).is_none(),
            "closed: no picker"
        );

        let add = vcx.debug_bounds(ADD_MEMBERS_ID).expect("Add members row");
        vcx.simulate_click(centre(add), Modifiers::default());
        settle(&shell, vcx);

        let (open, target) = state.read_with(&*vcx, |s, _| {
            (
                s.new_chat_open && s.new_group_open,
                s.add_members_group().map(|g| g.id.clone()),
            )
        });
        assert!(open, "the New group picker, aimed at this group");
        assert_eq!(target.as_deref(), Some("g1"));

        let section = vcx
            .debug_bounds(AGENTS_SECTION_ID)
            .expect("an Agents section in the picker");
        assert_eq!(AGENTS_SECTION_LABEL, "Agents");
        let comms = vcx
            .debug_bounds("group-agent-a-comms")
            .expect("my agent in it");
        let research = vcx
            .debug_bounds("group-agent-a-research")
            .expect("my agent not yet in the chat");
        let ledger = vcx
            .debug_bounds("group-agent-a-ledger")
            .expect("Priya's agent, already in the chat, listed after mine");
        assert!(section.top() < comms.top() && section.top() < research.top());
        assert!(
            research.top() > comms.top() && ledger.top() > research.top(),
            "mine first (list order), then the other members' agents: {comms:?} {research:?} {ledger:?}"
        );
        // People come after the agents.
        let sam = vcx.debug_bounds("group-person-gh_2");
        let priya = vcx.debug_bounds("group-person-gh_3");
        for p in [sam, priya].into_iter().flatten() {
            assert!(p.top() > ledger.top(), "people under the agents: {p:?}");
        }
    }
}
