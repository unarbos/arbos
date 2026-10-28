//! A person's pane (unified top bar, 2026-10-20, mockups f/g): the shared
//! read-only layout — picture, name, "@handle" subtitle, the one **Message**
//! pill under them (Jacob 2026-10-25, [feedback profile-message-button-mac-2026-10-25]:
//! opens the 1:1 DM, or starts it through the same `/chats/dm` path the New
//! Message sheet uses; never on your own pane, never when you blocked them),
//! one tab pill Links | Groups, the list, and a single red "Block user" row.
//! No Add members and no "Add your agent" here (Jacob 2026-10-27, [feedback
//! add-your-agent-row-removal-mac-2026-10-27]): members — people and agents —
//! are added from a group's Add members; `space_id` only says which chat the
//! pane was opened from.

use gpui::{div, prelude::*, px, Context, Div, ElementId, SharedString, Stateful};

use crate::app::ProfilePage;
use crate::model::{Person, Project};
use crate::ui::avatars;
use crate::ui::blocked;
use crate::ui::groups;
use crate::ui::people;
use crate::ui::shell::Shell;
use crate::ui::theme::Theme;

use super::pane::{self, Hero, Picture, ProfilePane, Section};
use super::{
    chats_of, collect, editable_name, hero_picture, kit, link_list, list_row, HeroClick,
    HERO_AVATAR,
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

/// The hero's Message pill: its element id / debug selector and label.
pub(crate) const MESSAGE_ID: &str = "profile-message";
pub(crate) const MESSAGE_LABEL: &str = "Message";
/// The hero's Unblock pill once I blocked them ([feedback blocked-state-mac-2026-10-26]).
pub(crate) const UNBLOCK_ID: &str = "profile-unblock-pill";

/// Does this person's pane carry the Message pill? Not on my own pane and
/// not once I blocked them (the relay refuses the DM either way; the other
/// direction — they blocked me — is not in any payload the client reads, so
/// that case still shows the pill and the relay's "not available" answer).
pub(crate) fn shows_message(me: bool, blocked: bool) -> bool {
    !me && !blocked
}

pub(super) fn page(
    handle: &str,
    space_id: Option<&str>,
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
                    avatars::project_in(state, p, kit::AVATAR, &theme).into_any_element()
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
    // A picture opens full-size in the media viewer; initials open nothing.
    let view_click = (!person.avatar_url.is_empty()).then(|| {
        let (name, url) = (person.display_name(), person.avatar_url.clone());
        let f = cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
            this.view_picture(&name, &url, cx);
        });
        Box::new(f) as HeroClick
    });
    let avatar = hero_picture("profile-avatar", picture, None, view_click).into_any_element();
    let name = editable_name(
        "profile-name",
        &person.display_name(),
        false,
        &shell.agent_name_input,
        None,
        &theme,
    )
    .into_any_element();
    // The one primary action: open the DM we have, or start one through the
    // same path as the New Message sheet (`AppState::start_dm` finds the
    // existing people space first, else `POST /chats/dm` and selects it when
    // the registry lists it).
    // Blocked ([feedback blocked-state-mac-2026-10-26]): the hero says so — a Blocked chip
    // under the @handle — and its one action is Unblock (the confirm takes the footer row's
    // place); otherwise the Message pill.
    let action = if blocked && !me {
        let target = person.clone();
        Some(
            kit::pill_button(UNBLOCK_ID, "nosign", blocked::UNBLOCK_LABEL, &theme)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.arm_unblock(target.clone(), cx);
                }))
                .into_any_element(),
        )
    } else {
        shows_message(me, blocked).then(|| {
            let handle = person.handle.clone();
            kit::pill_button(MESSAGE_ID, "bubble.left", MESSAGE_LABEL, &theme)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let handle = handle.clone();
                    this.state.update(cx, |s, cx| s.start_dm(handle, cx));
                }))
                .into_any_element()
        })
    };
    // The chip rides in the hero's under-the-subtitle slot (`editor`), above the pill.
    let chip = blocked.then(|| {
        div()
            .pt(px(2.0))
            .child(blocked::chip(&theme))
            .into_any_element()
    });
    let hero = Hero {
        picture: Picture::Element(avatar),
        name,
        badge: None,
        subtitle: Some(format!("@{}", person.handle).into()),
        subtitle_color: theme.text_muted,
        subtitle_click: None,
        subtitle_element: None,
        editor: chip,
        action,
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
                        .group(kit::HOT)
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

    // ----- actions: Block / Unblock ----------------------------------------------
    // The Block label comes from the relay's block list (#1545), so it is right
    // on re-open and relaunch; the pane stays open so the flip is visible.
    let mut footer: Vec<Section> = Vec::new();
    if !me {
        let target = person.clone();
        let row = if blocked && shell.unblock_armed_for(&person) {
            // Unblock asked for: the confirm in the row's place.
            let (cancel, go) = blocked::confirm_buttons("profile-unblock", &theme);
            blocked::confirm(
                "profile-unblock",
                &person.handle,
                cancel.on_click(cx.listener(|this, _, _, cx| this.cancel_unblock(cx))),
                go.on_click(cx.listener(|this, _, _, cx| this.confirm_unblock(cx))),
                &theme,
            )
            .into_any_element()
        } else if blocked {
            kit::action_row("profile-block", block_row_label(true), theme.accent, &theme)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.arm_unblock(target.clone(), cx);
                }))
                .into_any_element()
        } else {
            kit::danger_row("profile-block", block_row_label(false), &theme)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let handle = target.handle.clone();
                    this.state.update(cx, |s, cx| s.block_user(handle, cx));
                }))
                .into_any_element()
        };
        footer.push(Section::Card(vec![row]));
    }

    Some(pane::render(
        ProfilePane {
            page: ProfilePage::User {
                handle: handle.to_string(),
                space_id: space_id.map(str::to_string),
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

#[cfg(test)]
mod tests {
    use super::{shows_message, MESSAGE_LABEL};

    /// The pill is for other people I have not blocked — never my own pane,
    /// never once I blocked them ([feedback profile-message-button-mac-2026-10-25]).
    #[test]
    fn the_message_pill_is_for_other_unblocked_people() {
        assert!(shows_message(false, false));
        assert!(!shows_message(true, false), "not on my own pane");
        assert!(!shows_message(false, true), "not once I blocked them");
        assert!(!shows_message(true, true));
        assert_eq!(MESSAGE_LABEL, "Message");
    }
}

#[cfg(test)]
mod render_tests {
    //! The real `Shell` over a real `AppState`, headless: a DM with Sam, a
    //! group with Sam and Priya (no DM with Priya yet). Sam's pane carries the
    //! Message pill and a click opens the DM; Priya's click starts one through
    //! `start_dm`; my own pane and a blocked person's pane have no pill.
    use futures::future::BoxFuture;
    use gpui::{
        point, px, size, AppContext, Bounds, Entity, Modifiers, Pixels, TestAppContext,
        VisualTestContext,
    };
    use tokio::sync::mpsc;

    use super::MESSAGE_ID;
    use crate::app::{AppState, ProfilePage};
    use crate::model::{AccountView, Person, Project, SpaceKind};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::motion::PulseClock;
    use crate::ui::shell::Shell;
    use crate::ui::theme::{Theme, CONTROL};

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
                people: vec![
                    person("gh_2", "samir", "Sam Rivera"),
                    person("gh_3", "priya", "Priya Natarajan"),
                    person("gh_4", "dawizzaa", "🧙🏽\u{200d}♂\u{fe0f}"),
                    person("gh_5", "cato", "Cato 🌳"),
                ],
                ..Project::default()
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

    fn open(state: &Entity<AppState>, vcx: &mut VisualTestContext, handle: &str) {
        state.update(vcx, |s, cx| {
            s.open_profile(
                ProfilePage::User {
                    handle: handle.into(),
                    space_id: Some("g1".into()),
                },
                cx,
            );
        });
    }

    #[gpui::test]
    fn the_pill_sits_under_the_handle_and_opens_the_existing_dm(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        open(&state, vcx, "samir");
        settle(&shell, vcx);
        let pill = vcx
            .debug_bounds(MESSAGE_ID)
            .expect("Message pill on Sam's pane");
        let handle = vcx.debug_bounds("hero-subtitle").expect("@handle line");
        assert_eq!(
            f32::from(pill.size.height),
            CONTROL,
            "the button family's height"
        );
        assert!(
            pill.top() > handle.bottom(),
            "under the @handle: {pill:?} vs {handle:?}"
        );
        let pane_centre = handle.left() + handle.size.width / 2.0;
        let pill_centre = pill.left() + pill.size.width / 2.0;
        assert!(
            (f32::from(pane_centre) - f32::from(pill_centre)).abs() < 1.0,
            "centred with the hero"
        );

        vcx.simulate_click(centre(pill), Modifiers::default());
        settle(&shell, vcx);
        let selected = state.read_with(&*vcx, |s, _| s.selected_project.clone());
        assert_eq!(
            selected.as_deref(),
            Some("d1"),
            "the DM we already have opens"
        );
    }

    #[gpui::test]
    fn the_pill_starts_a_dm_when_there_is_none(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        open(&state, vcx, "priya");
        settle(&shell, vcx);
        let pill = vcx
            .debug_bounds(MESSAGE_ID)
            .expect("Message pill on Priya's pane");
        vcx.simulate_click(centre(pill), Modifiers::default());
        settle(&shell, vcx);
        let (awaiting, busy, selected) = state.read_with(&*vcx, |s, _| {
            (
                s.awaiting_dm_handle(),
                s.people_busy,
                s.selected_project.clone(),
            )
        });
        assert_eq!(
            awaiting.as_deref(),
            Some("priya"),
            "start_dm asked the relay"
        );
        assert!(busy);
        assert_eq!(
            selected.as_deref(),
            Some("g1"),
            "nothing to select until the relay lists the DM"
        );
    }

    /// [feedback emoji-name-handle-offset-mac-2026-10-28]: the hero's `@handle`
    /// line and the Message pill sit where they sit for a text name when the
    /// name is one emoji (or text with an emoji); only the emoji-only name is
    /// drawn lifted onto the baseline.
    #[gpui::test]
    fn an_emoji_name_keeps_the_handle_line_where_a_text_name_has_it(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        let mut seen = Vec::new();
        for (handle, lifted) in [("samir", false), ("dawizzaa", true), ("cato", false)] {
            open(&state, vcx, handle);
            settle(&shell, vcx);
            let line = vcx
                .debug_bounds("hero-subtitle")
                .unwrap_or_else(|| panic!("@{handle} line"));
            let pill = vcx
                .debug_bounds(MESSAGE_ID)
                .unwrap_or_else(|| panic!("Message pill on @{handle}'s pane"));
            assert_eq!(
                vcx.debug_bounds(crate::ui::identity::LIFTED_SELECTOR)
                    .is_some(),
                lifted,
                "@{handle}: lifted name = {lifted}"
            );
            seen.push((handle, line.origin.y, line.size.height, pill.origin.y));
        }
        let (_, text_y, text_h, text_pill) = seen[0];
        for (handle, y, h, pill) in &seen[1..] {
            assert_eq!(
                *y, text_y,
                "@{handle}: the handle line's top is a text name's"
            );
            assert_eq!(
                *h, text_h,
                "@{handle}: the handle line's height is a text name's"
            );
            assert_eq!(
                *pill, text_pill,
                "@{handle}: the pill under it is where a text name has it"
            );
        }
    }

    #[gpui::test]
    fn no_pill_on_my_own_pane_or_a_blocked_persons(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        open(&state, vcx, "unconst");
        settle(&shell, vcx);
        assert!(vcx.debug_bounds(MESSAGE_ID).is_none(), "not on my own pane");

        state.update(vcx, |s, cx| {
            s.blocked = vec![person("gh_3", "priya", "Priya Natarajan")];
            cx.notify();
        });
        open(&state, vcx, "priya");
        settle(&shell, vcx);
        assert!(
            vcx.debug_bounds(MESSAGE_ID).is_none(),
            "not once I blocked them"
        );

        state.update(vcx, |s, cx| {
            s.blocked.clear();
            cx.notify();
        });
        settle(&shell, vcx);
        assert!(
            vcx.debug_bounds(MESSAGE_ID).is_some(),
            "back once unblocked"
        );
    }
}
