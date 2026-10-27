//! One behaviour for every picture or name of a person or an agent in the
//! chat ([feedback profile-pane-toggle-mac-2026-10-26]; Jacob 2026-10-26:
//! "clicking the agent or user in the chat should open their user profile
//! right bar. Clicking it again should close it. Same if you click the user
//! profile picture to open the side bar").
//!
//! The targets — a sender name inside a bubble, the disc beside it, an
//! @mention chip (people and agents), the chat header pill, a status-row
//! picture — all route through [`AppState::toggle_profile`]: open the
//! profile in the right pane, close it when that same person or agent is
//! showing (whichever target opened it), swap it in place for someone else.
//! Every target shows the pointing hand and carries the same accessible
//! label, so VoiceOver and the headless tests read one thing everywhere.

use gpui::{prelude::*, App, Entity, Role, SharedString};

use crate::app::{AppState, ProfilePage};

/// The accessible label of a target: what one click will do.
pub fn label(name: &str, showing: bool) -> String {
    if showing {
        "Close profile".to_string()
    } else {
        format!("Open {}'s profile", name.trim())
    }
}

/// The hand, the button role and the label — no click. `showing` is
/// `AppState::profile_shows(page)`. For a target that has its own click
/// handler (the header pill's double-click rename) but must still read and
/// feel like every other one.
pub fn mark<E>(el: E, name: &str, showing: bool) -> E
where
    E: StatefulInteractiveElement + Styled,
{
    el.cursor_pointer()
        .role(Role::Button)
        .aria_label(SharedString::from(label(name, showing)))
}

/// The whole behaviour: [`mark`] plus a click that toggles `page` through the
/// one shell action.
pub fn attach<E>(el: E, state: &Entity<AppState>, page: ProfilePage, name: &str, cx: &App) -> E
where
    E: StatefulInteractiveElement + Styled,
{
    let el = mark(el, name, state.read(cx).profile_shows(&page));
    let state = state.clone();
    el.on_click(move |_, _, cx| {
        let page = page.clone();
        state.update(cx, |s, cx| s.toggle_profile(page, cx));
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_says_what_the_click_does() {
        assert_eq!(label("Allan", false), "Open Allan's profile");
        assert_eq!(label(" Affine ", false), "Open Affine's profile");
        assert_eq!(label("Allan", true), "Close profile");
    }

    #[test]
    fn a_person_is_the_same_subject_from_any_chat() {
        let from_dm = ProfilePage::User {
            handle: "allan".into(),
            space_id: Some("dm-1".into()),
        };
        let from_chip = ProfilePage::User {
            handle: "Allan".into(),
            space_id: None,
        };
        let other = ProfilePage::User {
            handle: "sami".into(),
            space_id: Some("dm-1".into()),
        };
        assert!(from_dm.same_subject(&from_chip));
        assert!(!from_dm.same_subject(&other));
        let agent = ProfilePage::Agent {
            project_id: "p-1".into(),
        };
        let agent_again = ProfilePage::Agent {
            project_id: "p-1".into(),
        };
        let agent_other = ProfilePage::Agent {
            project_id: "p-2".into(),
        };
        assert!(agent.same_subject(&agent_again));
        assert!(!agent.same_subject(&agent_other));
        assert!(!agent.same_subject(&from_dm));
    }

    /// Every chat target routes through `AppState::toggle_profile` (through
    /// `profile_target::attach` or the header's own listener): no target in
    /// the transcript, the mention table or the status row opens a profile
    /// by another road — so every one of them carries the hand, the button
    /// role and the label `mark` sets.
    #[test]
    fn no_chat_target_opens_a_profile_by_another_road() {
        let src = |rel: &str| {
            std::fs::read_to_string(
                concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui/").to_string() + rel,
            )
            .unwrap_or_else(|e| panic!("{rel}: {e}"))
        };
        for (file, forbidden) in [
            ("transcript.rs", &["s.push_profile(", "s.open_profile("][..]),
            ("presence.rs", &["s.push_profile(", "s.open_profile("][..]),
            ("mention.rs", &["s.push_profile(", "s.open_profile("][..]),
            ("header.rs", &["s.push_profile(", "s.open_profile("][..]),
        ] {
            let text = src(file);
            for needle in forbidden {
                assert!(
                    !text.contains(needle),
                    "{file} still calls `{needle}`: route the click through profile_target::attach / AppState::toggle_profile"
                );
            }
        }
    }
}

#[cfg(test)]
mod render_tests {
    //! The real `Shell` over a real `AppState`, headless: Allan and the agent
    //! Affine in the Arbonites group (Allan typing, his last bubble mentions
    //! @Affine), plus a DM with Allan for the header pill. Every target
    //! opens its profile, a second click on it — or on any other target for
    //! the same person — closes the pane, and a click on someone else swaps
    //! the page in place with nothing on the back stack.
    use futures::future::BoxFuture;
    use gpui::{
        point, prelude::*, px, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext,
    };
    use tokio::sync::mpsc;

    use crate::app::{AppState, ProfilePage};
    use crate::model::{now_ms, AgentMember, Chat, Message, Person, Project, SpaceKind};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::motion::PulseClock;
    use crate::ui::shell::Shell;
    use crate::ui::theme::Theme;

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

    const GROUP: &str = "01GROUPARBONITES0000000000";
    const DM: &str = "01DMWITHALLAN000000000000";
    const AGENT: &str = "01PROJAFFINE0000000000000";

    fn allan() -> Person {
        Person {
            id: "gh_2".into(),
            handle: "allan".into(),
            name: Some("Allan".into()),
            ..Person::default()
        }
    }

    fn affine() -> AgentMember {
        AgentMember {
            project_id: AGENT.into(),
            name: "Affine".into(),
            handle: "affine".into(),
            owner_handle: "sami".into(),
            avatar_leaf: Some(5),
            avatar_leaf_color: Some(2),
            ..Default::default()
        }
    }

    fn person_row(id: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "chatId": GROUP, "role": "user", "sender": "user:gh_2",
            "senderHandle": "allan", "senderName": "Allan", "status": "done",
            "createdAt": at, "completedAt": at,
            "parts": [{ "id": format!("{id}-t"), "kind": "text", "text": text }],
        }))
        .expect("message")
    }

    fn agent_row(id: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "chatId": GROUP, "role": "assistant", "sender": format!("agent:{AGENT}"),
            "senderHandle": "affine", "senderName": "Affine", "status": "done",
            "createdAt": at, "completedAt": at + 1_000,
            "parts": [{ "id": format!("{id}-t"), "kind": "text", "text": text }],
        }))
        .expect("message")
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
            // The pane snaps instead of sliding: a click measured mid-slide lands beside its
            // target (the gate's `--workspace` run, several binaries at once, caught this).
            cx.set_reduce_motion(true);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        let now = now_ms();
        state.update(cx, |s, cx| {
            s.projects.push(Project {
                id: GROUP.into(),
                name: "Arbonites".into(),
                kind: SpaceKind::Group,
                people: vec![allan()],
                agents: vec![affine()],
                ..Project::default()
            });
            s.projects.push(Project {
                id: DM.into(),
                name: "allan".into(),
                kind: SpaceKind::Dm,
                people: vec![allan()],
                ..Project::default()
            });
            let chat = |id: &str, title: &str| -> Chat {
                serde_json::from_value(serde_json::json!({
                    "id": id, "project_id": id, "title": title, "is_pinned": true,
                    "is_system": false, "unread": false, "last_message_at": now,
                }))
                .expect("chat")
            };
            s.chats.insert(GROUP.into(), vec![chat(GROUP, "Arbonites")]);
            s.chats.insert(DM.into(), vec![chat(DM, "allan")]);
            s.messages.insert(
                GROUP.into(),
                vec![
                    agent_row("a0", "Here is the chart you asked for.", now - 120_000),
                    person_row("a1", "@affine I did not see the chart", now - 60_000),
                ],
            );
            s.messages
                .insert(DM.into(), vec![person_row("d0", "hey", now - 30_000)]);
            // Allan is typing in the group: his face on the status row.
            s.typing
                .insert(GROUP.into(), vec![("allan".into(), now + 600_000)]);
            s.selected_project = Some(GROUP.into());
            s.active_tab.insert(GROUP.into(), GROUP.into());
            s.active_tab.insert(DM.into(), DM.into());
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.run_until_parked();
        settle(&state, &shell, vcx);
        (rt, state, shell, vcx)
    }

    fn settle(state: &Entity<AppState>, shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        for _ in 0..4 {
            state.update(vcx, |_, cx| cx.notify());
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    fn centre(b: Bounds<Pixels>) -> gpui::Point<Pixels> {
        point(b.left() + b.size.width / 2.0, b.top() + b.size.height / 2.0)
    }

    fn bounds(vcx: &mut VisualTestContext, selector: &str) -> Bounds<Pixels> {
        let key: &'static str = Box::leak(selector.to_string().into_boxed_str());
        vcx.debug_bounds(key)
            .unwrap_or_else(|| panic!("`{selector}` is drawn"))
    }

    /// Click `selector`'s centre, or — for the @Affine chip — the second glyph
    /// of the last bubble's text.
    fn click(
        selector: &str,
        state: &Entity<AppState>,
        shell: &Entity<Shell>,
        vcx: &mut VisualTestContext,
    ) {
        let at = if selector == "chip" {
            let text = bounds(vcx, "bubble-text");
            point(text.left() + px(12.0), text.top() + px(11.0))
        } else {
            centre(bounds(vcx, selector))
        };
        vcx.simulate_mouse_move(at, None, Modifiers::default());
        vcx.run_until_parked();
        vcx.simulate_click(at, Modifiers::default());
        settle(state, shell, vcx);
    }

    fn open(state: &Entity<AppState>, vcx: &mut VisualTestContext) -> Option<ProfilePage> {
        state.read_with(vcx, |s, _| s.profile.clone())
    }

    fn depth(state: &Entity<AppState>, vcx: &mut VisualTestContext) -> usize {
        state.read_with(vcx, |s, _| s.profile_stack.len())
    }

    type Shows = fn(Option<ProfilePage>) -> bool;

    fn is_allan(page: Option<ProfilePage>) -> bool {
        matches!(page, Some(ProfilePage::User { handle, .. }) if handle == "allan")
    }

    fn is_affine(page: Option<ProfilePage>) -> bool {
        matches!(page, Some(ProfilePage::Agent { project_id }) if project_id == AGENT)
    }

    /// Each target kind — the person's name and disc, the agent's name and
    /// disc, the @mention chip, the status-row picture — opens its profile
    /// with nothing underneath, and a second click on the same target closes
    /// the pane.
    #[gpui::test]
    fn every_target_opens_and_a_second_click_closes(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        assert!(open(&state, vcx).is_none(), "nothing open to start");
        let targets: [(&str, Shows); 6] = [
            ("who#a1#user", is_allan),
            ("disc#a1#user", is_allan),
            ("presence-picture-0", is_allan),
            ("who#a0#a0-t.0", is_affine),
            ("disc#a0#a0-t.0", is_affine),
            ("chip", is_affine),
        ];
        for (selector, shows) in targets {
            click(selector, &state, &shell, vcx);
            assert!(
                shows(open(&state, vcx)),
                "{selector}: one click opens the profile, got {:?}",
                open(&state, vcx)
            );
            assert_eq!(
                depth(&state, vcx),
                0,
                "{selector}: nothing on the back stack"
            );
            assert!(
                vcx.debug_bounds("profile-back").is_none(),
                "{selector}: no ‹ disc, the page stands alone"
            );
            click(selector, &state, &shell, vcx);
            assert!(
                open(&state, vcx).is_none(),
                "{selector}: the second click closes the pane"
            );
        }
    }

    /// Any target for the same person closes a pane another target opened:
    /// the chip is Affine's name is Affine's disc; Allan's name is his disc
    /// is his face on the status row.
    #[gpui::test]
    fn any_target_for_the_same_profile_closes_it(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        for (opener, closer) in [
            ("who#a1#user", "disc#a1#user"),
            ("disc#a1#user", "presence-picture-0"),
            ("presence-picture-0", "who#a1#user"),
            ("chip", "who#a0#a0-t.0"),
            ("who#a0#a0-t.0", "disc#a0#a0-t.0"),
            ("disc#a0#a0-t.0", "chip"),
        ] {
            click(opener, &state, &shell, vcx);
            assert!(open(&state, vcx).is_some(), "{opener} opened");
            click(closer, &state, &shell, vcx);
            assert!(
                open(&state, vcx).is_none(),
                "{closer} closes what {opener} opened"
            );
        }
    }

    /// A different person or agent swaps the page in place: no push, no ‹.
    #[gpui::test]
    fn a_different_person_swaps_the_page_in_place(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        click("who#a1#user", &state, &shell, vcx);
        assert!(is_allan(open(&state, vcx)));
        click("who#a0#a0-t.0", &state, &shell, vcx);
        assert!(is_affine(open(&state, vcx)), "Affine replaced Allan");
        assert_eq!(depth(&state, vcx), 0, "swapped, not pushed");
        assert!(vcx.debug_bounds("profile-back").is_none(), "no ‹ disc");
        click("presence-picture-0", &state, &shell, vcx);
        assert!(is_allan(open(&state, vcx)), "Allan back in place");
        assert_eq!(depth(&state, vcx), 0);
        click("chip", &state, &shell, vcx);
        assert!(is_affine(open(&state, vcx)));
        click("chip", &state, &shell, vcx);
        assert!(open(&state, vcx).is_none(), "and the chip closes it");
    }

    /// The DM header pill is a target too: it opens Allan, closes him, and
    /// closes a page his bubble opened (the chip's page has no space, the
    /// pill's has — the same person either way).
    #[gpui::test]
    fn the_header_pill_toggles_the_same_person_as_the_bubbles(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        state.update(vcx, |s, cx| {
            s.selected_project = Some(DM.into());
            cx.notify();
        });
        settle(&state, &shell, vcx);
        click("chat-header-pill", &state, &shell, vcx);
        assert!(
            matches!(
                open(&state, vcx),
                Some(ProfilePage::User { handle, space_id }) if handle == "allan" && space_id.as_deref() == Some(DM)
            ),
            "the pill opens the DM's person: {:?}",
            open(&state, vcx)
        );
        click("chat-header-pill", &state, &shell, vcx);
        assert!(open(&state, vcx).is_none(), "the pill closes him again");
        // Opened from a chip-shaped page (no space): the pill still reads it as his.
        state.update(vcx, |s, cx| {
            s.open_profile(
                ProfilePage::User {
                    handle: "Allan".into(),
                    space_id: None,
                },
                cx,
            );
        });
        settle(&state, &shell, vcx);
        state.read_with(vcx, |s, _| {
            assert!(s.profile_shows(&ProfilePage::User {
                handle: "allan".into(),
                space_id: Some(DM.into()),
            }));
        });
        click("chat-header-pill", &state, &shell, vcx);
        assert!(
            open(&state, vcx).is_none(),
            "same person, whichever target opened him: the pill closes the pane"
        );
    }

    /// The label follows the pane: "Open Allan's profile" while closed,
    /// "Close profile" while his page shows, and back once another page took
    /// its place.
    #[gpui::test]
    fn the_label_tracks_whose_page_is_showing(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        let allan_page = ProfilePage::User {
            handle: "allan".into(),
            space_id: Some(GROUP.into()),
        };
        let label_for = |state: &Entity<AppState>, vcx: &mut VisualTestContext| {
            state.read_with(vcx, |s, _| {
                super::label("Allan", s.profile_shows(&allan_page))
            })
        };
        assert_eq!(label_for(&state, vcx), "Open Allan's profile");
        click("who#a1#user", &state, &shell, vcx);
        assert_eq!(label_for(&state, vcx), "Close profile");
        click("who#a0#a0-t.0", &state, &shell, vcx);
        assert_eq!(
            label_for(&state, vcx),
            "Open Allan's profile",
            "Affine's page is up, Allan's targets offer to open his"
        );
    }
}
