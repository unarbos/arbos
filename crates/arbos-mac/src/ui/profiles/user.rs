//! A person's pane (unified top bar, 2026-10-20, mockups f/g): the shared
//! read-only layout — picture, name, "@handle" subtitle, one tab pill
//! Links | Groups, the list, and a single red "Block user" row. Nothing
//! else: no last-seen line, no call / mute / search / more, no username
//! field, no Send Message, no Add members (adding people starts in groups).

use gpui::{div, prelude::*, px, Context, Div, ElementId, SharedString, Stateful};

use crate::app::ProfilePage;
use crate::model::{Person, Project};
use crate::ui::avatars;
use crate::ui::glass;
use crate::ui::groups;
use crate::ui::people;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, TEXT_BODY, TEXT_SMALL};

use super::{
    card, chats_of, collect, editable_name, editable_picture, empty, frame, hero, link_list,
    list_row, tab_bar, HERO_AVATAR,
};

const TABS: [&str; 2] = ["Links", "Groups"];

pub(super) fn page(
    handle: &str,
    _space_id: Option<&str>,
    shell: &Shell,
    cx: &mut Context<Shell>,
) -> Option<Stateful<Div>> {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let person: Person = state.person_by_handle(handle).unwrap_or_else(|| Person {
        handle: handle.to_string(),
        ..Default::default()
    });
    let me = state.me_handle() == person.handle;
    let image = state.avatar(&person.avatar_url);
    let tab = state.profile_tab;

    // Spaces we share: group chats and shared projects (the DM itself is not a group).
    let shared: Vec<Project> = state
        .projects
        .iter()
        .filter(|p| p.people.iter().any(|x| x.handle == person.handle))
        .cloned()
        .collect();
    let groups: Vec<&Project> = shared
        .iter()
        .filter(|p| p.kind != crate::model::SpaceKind::Dm)
        .collect();

    // Links this person shared, across every chat we hold.
    let all_chats: Vec<&crate::model::Chat> =
        shared.iter().flat_map(|p| chats_of(state, &p.id)).collect();
    let content = collect(state, &all_chats);
    let any_chats = !all_chats.is_empty();
    drop(all_chats);
    let by = format!("@{}", person.handle);
    let links: Vec<super::Link> = content.links.into_iter().filter(|l| l.by == by).collect();
    let group_pictures: Vec<gpui::AnyElement> = groups
        .iter()
        .map(|p| {
            if p.is_people() {
                groups::picture(p, state, 36.0, &theme).into_any_element()
            } else {
                avatars::project(p, 36.0, &theme).into_any_element()
            }
        })
        .collect();

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
    let hero = hero(
        avatar,
        name,
        &format!("@{}", person.handle),
        theme.text_muted,
        &theme,
    );

    // ----- tabs -----------------------------------------------------------------
    let tabs = tab_bar(&TABS, tab, &theme, cx);
    let body_tab = match tab {
        0 => link_list(&links, &theme, cx),
        _ => {
            if groups.is_empty() {
                empty("No groups in common yet", &theme)
            } else {
                let mut list = card(&theme);
                for (p, leading) in groups.iter().zip(group_pictures) {
                    let n = p.people.len() + 1;
                    let sub = if p.is_people() {
                        format!("{n} members · you are both in it")
                    } else {
                        format!("shared project · {n} people")
                    };
                    let pid = p.id.clone();
                    list = list.child(
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
                        })),
                    );
                }
                list
            }
        }
    };

    // ----- the one action: Block user --------------------------------------------
    let h_block = person.handle.clone();
    let block = (!me).then(|| {
        glass::group(&theme).flex_shrink_0().mx(px(16.0)).child(
            div()
                .id("profile-block")
                .flex()
                .items_center()
                .justify_center()
                .min_h(px(44.0))
                .text_size(px(TEXT_BODY))
                .text_color(theme.danger)
                .cursor_pointer()
                .hover(|s| s.bg(theme.surface_2))
                .child("Block user")
                .on_click(cx.listener(move |this, _, window, cx| {
                    let h = h_block.clone();
                    this.state.update(cx, |s, cx| s.block_user(h, cx));
                    this.close_profile(window, cx);
                })),
        )
    });

    let body = div()
        .flex()
        .flex_col()
        .pb(px(16.0))
        .child(hero)
        .child(tabs)
        .child(body_tab)
        .when(links.is_empty() && tab == 0 && any_chats, |el| {
            el.child(
                div()
                    .px(px(16.0))
                    .pb(px(12.0))
                    .text_size(px(TEXT_SMALL - 1.0))
                    .text_color(theme.text_faint)
                    .child("Links appear here as chats load."),
            )
        })
        .children(block);
    Some(frame(body, None, None, &theme, cx))
}
