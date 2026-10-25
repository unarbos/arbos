//! A person's pane (unified top bar, 2026-10-20, mockups f/g): the shared
//! read-only layout — picture, name, "@handle" subtitle, one tab pill
//! Links | Groups, the list, and a single red "Block user" row. Nothing
//! else: no last-seen line, no call / mute / search / more, no username
//! field, no Send Message, no Add members (adding people starts in groups).

use gpui::{prelude::*, Context, Div, ElementId, SharedString, Stateful};

use crate::app::ProfilePage;
use crate::model::{Person, Project};
use crate::ui::avatars;
use crate::ui::groups;
use crate::ui::people;
use crate::ui::shell::Shell;
use crate::ui::theme::Theme;

use super::pane::{self, Hero, Picture, ProfilePane, Section};
use super::{
    chats_of, collect, editable_name, editable_picture, kit, link_list, list_row, HERO_AVATAR,
};

pub(super) const TABS: [&str; 2] = ["Links", "Groups"];

/// The footer row's label for the relay's current block state (#1545).
pub(crate) fn block_row_label(blocked: bool) -> &'static str {
    if blocked {
        "Unblock"
    } else {
        "Block user"
    }
}

pub(super) fn page(
    handle: &str,
    _space_id: Option<&str>,
    shell: &Shell,
    cx: &mut Context<Shell>,
) -> Option<Stateful<Div>> {
    let theme = Theme::of(cx).clone();
    // Own everything we need from state so link_list can request favicons after.
    let (person, me, blocked, image, tab, links, groups, group_pictures) = {
        let state = shell.state.read(cx);
        let person: Person = state.person_by_handle(handle).unwrap_or_else(|| Person {
            handle: handle.to_string(),
            ..Default::default()
        });
        let me = state.me_handle() == person.handle;
        let blocked = state.is_blocked(&person);
        let image = state.avatar(&person.avatar_url);
        let tab = state.profile_tab;

        // Spaces we share: group chats and shared projects (the DM itself is not a group).
        let shared: Vec<Project> = state
            .projects
            .iter()
            .filter(|p| p.people.iter().any(|x| x.handle == person.handle))
            .cloned()
            .collect();
        let groups: Vec<Project> = shared
            .iter()
            .filter(|p| p.kind != crate::model::SpaceKind::Dm)
            .cloned()
            .collect();

        // Links this person shared, across every chat we hold.
        let all_chats: Vec<&crate::model::Chat> =
            shared.iter().flat_map(|p| chats_of(state, &p.id)).collect();
        let content = collect(state, &all_chats);
        let by = format!("@{}", person.handle);
        let links: Vec<super::Link> = content.links.into_iter().filter(|l| l.by == by).collect();
        let group_pictures: Vec<gpui::AnyElement> = groups
            .iter()
            .map(|p| {
                if p.is_people() {
                    groups::picture(p, state, kit::AVATAR, &theme).into_any_element()
                } else {
                    avatars::project(p, kit::AVATAR, &theme).into_any_element()
                }
            })
            .collect();
        (
            person,
            me,
            blocked,
            image,
            tab,
            links,
            groups,
            group_pictures,
        )
    };

    // ----- hero: picture and name read-only, @handle as the subtitle --------------
    let picture = people::avatar(image, &person, HERO_AVATAR, &theme).into_any_element();
    let avatar = editable_picture("profile-avatar", picture, None).into_any_element();
    let name = editable_name(
        "profile-name",
        &person.display_name(),
        false,
        &shell.agent_name_input,
        None,
        &theme,
    )
    .into_any_element();
    let hero = Hero {
        picture: Picture::Element(avatar),
        name,
        badge: None,
        subtitle: Some(format!("@{}", person.handle).into()),
        subtitle_color: theme.text_muted,
        subtitle_click: None,
        editor: None,
    };

    // ----- tabs: Links | Groups (the same set on the iPhone) ------------------------
    let body: Vec<Section> = match tab {
        0 => vec![Section::Custom(
            link_list(&links, shell, &theme, cx).into_any_element(),
        )],
        _ => {
            if groups.is_empty() {
                vec![Section::Empty("No groups in common yet".into())]
            } else {
                let rows = groups
                    .iter()
                    .zip(group_pictures)
                    .map(|(p, leading)| {
                        let n = p.people.len() + 1;
                        let sub = if p.is_people() {
                            format!("{n} members · you are both in it")
                        } else {
                            format!("shared project · {n} people")
                        };
                        let pid = p.id.clone();
                        list_row(
                            ElementId::Name(SharedString::from(format!("profile-group-{}", p.id))),
                            leading,
                            p.name.clone(),
                            sub,
                            theme.text_muted,
                            None,
                            &theme,
                        )
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.surface_2))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let pid = pid.clone();
                            this.state.update(cx, |s, cx| {
                                s.select_project(pid.clone(), cx);
                                s.open_profile(ProfilePage::Group { space_id: pid }, cx);
                            });
                        }))
                        .into_any_element()
                    })
                    .collect();
                vec![Section::Card(rows)]
            }
        }
    };

    // ----- the one action: Block user / Unblock ------------------------------------
    // The label comes from the relay's block list (#1545), so it is right on
    // re-open and relaunch; the pane stays open so the flip is visible.
    let footer = if me {
        Vec::new()
    } else {
        let row = if blocked {
            kit::action_row("profile-block", block_row_label(true), theme.accent, &theme)
        } else {
            kit::danger_row("profile-block", block_row_label(false), &theme)
        };
        let target = person.clone();
        vec![Section::Card(vec![row
            .on_click(cx.listener(move |this, _, _, cx| {
                let target = target.clone();
                this.state.update(cx, |s, cx| {
                    if s.is_blocked(&target) {
                        s.unblock_user(&target, cx);
                    } else {
                        s.block_user(target.handle.clone(), cx);
                    }
                });
            }))
            .into_any_element()])]
    };

    Some(pane::render(
        ProfilePane {
            page: ProfilePage::User {
                handle: handle.to_string(),
                space_id: _space_id.map(str::to_string),
            },
            hero,
            tabs: &TABS,
            selected: tab,
            body,
            footer,
            done: None,
            share: None,
        },
        shell,
        cx,
    ))
}
