//! The Blocked state, one look everywhere (Jacob, 2026-10-26, [feedback
//! blocked-state-mac-2026-10-26]: "Today you can't tell in the app that you've blocked
//! someone"): a **Blocked** tag (the DM header pill, member rows), the pane hero's **Blocked**
//! chip, the **Unblock** confirm every surface asks before it lifts a block ("Unblock @x?
//! They'll be able to message you again." · Cancel · Unblock), and the bar that stands in for
//! the composer of a DM with someone I blocked ("You blocked X · Unblock"). The block list
//! itself is the relay's (`AppState::blocked`, `GET /blocks`); nothing here keeps a local flag.
//!
//! Tokens: `danger` ink on `danger_soft` for the tag and chip; the kit's confirm buttons; the
//! composer bar is `COMPOSER_HEIGHT` tall on `surface_2` with the composer's radius.

use gpui::{div, prelude::*, px, Div, FontWeight, SharedString, Stateful};

use crate::model::Person;
use crate::ui::profiles::kit;
use crate::ui::sf_symbols;
use crate::ui::theme::{Theme, COMPOSER_HEIGHT, TEXT_SMALL, TEXT_TINY};

/// The small capsule after a name: 16 pt tall, 10.5 pt semibold.
pub const TAG_HEIGHT: f32 = 16.0;
pub const TAG_TEXT: f32 = 10.5;
/// The hero chip under the @handle: 22 pt tall, the tiny type, the glyph before the word.
pub const CHIP_HEIGHT: f32 = 22.0;
pub const CHIP_GLYPH: f32 = 12.0;

pub const TAG_LABEL: &str = "Blocked";
pub const UNBLOCK_LABEL: &str = "Unblock";

/// The question the confirm asks and the line under it.
pub fn confirm_question(handle: &str) -> String {
    format!("Unblock @{handle}?")
}

pub const CONFIRM_NOTE: &str = "They'll be able to message you and see you in groups again.";

/// "You blocked X" for the composer bar.
pub fn composer_line(person: &Person) -> String {
    format!("You blocked {}", person.display_name())
}

/// The capsule tag: a DM header pill after the name, a member row's trailing slot.
pub fn tag(theme: &Theme) -> Div {
    div()
        .h(px(TAG_HEIGHT))
        .px(px(6.0))
        .flex_shrink_0()
        .rounded_full()
        .bg(theme.danger_soft)
        .flex()
        .items_center()
        .text_size(px(TAG_TEXT))
        .line_height(px(TAG_HEIGHT))
        .font_weight(FontWeight::SEMIBOLD)
        // error-ui: exception status mark (the Blocked tag is a state, not a failure)
        .text_color(theme.danger)
        .debug_selector(|| "blocked-tag".into())
        .child(TAG_LABEL)
}

/// The pane hero's chip under the @handle: glyph + "Blocked".
pub fn chip(theme: &Theme) -> Div {
    div()
        .h(px(CHIP_HEIGHT))
        .pl(px(8.0))
        .pr(px(10.0))
        .rounded_full()
        .bg(theme.danger_soft)
        .flex()
        .items_center()
        .gap(px(5.0))
        .text_size(px(TEXT_TINY))
        .font_weight(FontWeight::SEMIBOLD)
        // error-ui: exception status mark (the Blocked chip is a state, not a failure)
        .text_color(theme.danger)
        .debug_selector(|| "blocked-chip".into())
        .child(sf_symbols::glyph("nosign", CHIP_GLYPH, theme.danger))
        .child(TAG_LABEL)
}

/// The confirm, in a row's or the bar's place: the question, the note, Cancel · Unblock.
/// `id` prefixes the buttons (`{id}-cancel`, `{id}-go`).
pub fn confirm(
    id: &'static str,
    handle: &str,
    cancel: Stateful<Div>,
    go: Stateful<Div>,
    theme: &Theme,
) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .px(px(kit::INSET))
        .py(px(10.0))
        .debug_selector(move || format!("{id}-confirm"))
        .child(
            div()
                .text_size(px(TEXT_SMALL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(confirm_question(handle)),
        )
        .child(
            div()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(CONFIRM_NOTE),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(6.0))
                .pt(px(2.0))
                .child(cancel)
                .child(go),
        )
}

/// The confirm's two buttons with the shared ids.
pub fn confirm_buttons(id: &'static str, theme: &Theme) -> (Stateful<Div>, Stateful<Div>) {
    let cancel_id: &'static str = Box::leak(format!("{id}-cancel").into_boxed_str());
    let go_id: &'static str = Box::leak(format!("{id}-go").into_boxed_str());
    (
        kit::confirm_button(cancel_id, "Cancel", false, theme)
            .debug_selector(move || cancel_id.to_string()),
        kit::confirm_button(go_id, UNBLOCK_LABEL, true, theme)
            .debug_selector(move || go_id.to_string()),
    )
}

/// "N messages hidden because you blocked @x" for the transcript.
pub fn hidden_line(count: usize, who: &str) -> String {
    let n = if count == 1 {
        "1 message".to_string()
    } else {
        format!("{count} messages")
    };
    format!("{n} hidden because you blocked {who}")
}

/// The transcript's folded line for a run of a blocked person's messages (Jacob
/// 2026-10-26: hidden from the blocker alone): the sentence in the stamp's muted type, then
/// the Unblock link the caller wires (it opens the shell's confirm). `who` is the display
/// name, else `@handle`.
pub fn hidden_row(row_id: &str, count: usize, who: &str, theme: &Theme) -> (Div, Stateful<Div>) {
    let line = div()
        .debug_selector(|| "hidden-row".into())
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(TEXT_SMALL))
        .text_color(theme.text_muted)
        .child(sf_symbols::glyph("nosign", 12.0, theme.text_muted))
        .child(hidden_line(count, who))
        .child("·");
    let unblock = div()
        .id(SharedString::from(format!("{row_id}#unblock")))
        .debug_selector(|| "hidden-row-unblock".into())
        .text_color(theme.accent)
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|s| s.opacity(0.8))
        .child(UNBLOCK_LABEL);
    (line, unblock)
}

/// The composer's stand-in for a DM with someone I blocked: "You blocked X" with an Unblock
/// capsule at the right, the composer's height and radius.
pub fn composer_bar(person: &Person, theme: &Theme) -> (Div, Stateful<Div>) {
    let unblock = div()
        .id("composer-unblock")
        .debug_selector(|| "composer-unblock".into())
        .h(px(28.0))
        .px(px(12.0))
        .rounded_full()
        .bg(theme.field)
        .border_1()
        .border_color(theme.field_border)
        .flex()
        .items_center()
        .text_size(px(TEXT_SMALL))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.accent)
        .cursor_pointer()
        .hover(|s| s.opacity(0.85))
        .child(UNBLOCK_LABEL);
    let bar = div()
        .w_full()
        .h(px(COMPOSER_HEIGHT))
        .pl(px(16.0))
        .pr(px(8.0))
        .rounded(px(COMPOSER_HEIGHT / 2.0))
        .bg(theme.surface_2)
        .flex()
        .items_center()
        .gap(px(10.0))
        .debug_selector(|| "composer-blocked".into())
        .child(sf_symbols::glyph("nosign", 14.0, theme.text_muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .debug_selector(|| "composer-blocked-text".into())
                .child(SharedString::from(composer_line(person))),
        );
    (bar, unblock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words() {
        let sam = Person {
            handle: "sam".into(),
            name: Some("Sam Rivera".into()),
            ..Person::default()
        };
        assert_eq!(composer_line(&sam), "You blocked Sam Rivera");
        assert_eq!(confirm_question("sam"), "Unblock @sam?");
        let bare = Person {
            handle: "sam".into(),
            ..Person::default()
        };
        assert_eq!(composer_line(&bare), "You blocked sam");
        assert_eq!(
            hidden_line(1, "@sam"),
            "1 message hidden because you blocked @sam"
        );
        assert_eq!(
            hidden_line(3, "Sam Rivera"),
            "3 messages hidden because you blocked Sam Rivera"
        );
    }
}

#[cfg(test)]
mod render_tests {
    //! The real `Shell` over a real `AppState`, headless: a DM with Sam (blocked), a group
    //! with Sam and Priya. Every surface says Blocked, every Unblock asks first, the DM's
    //! composer is the "You blocked Sam · Unblock" bar, and the chat note names the block.
    use futures::future::BoxFuture;
    use gpui::{
        point, px, size, AppContext, Bounds, Entity, Modifiers, Pixels, TestAppContext,
        VisualTestContext,
    };
    use tokio::sync::mpsc;

    use crate::app::{AppState, ProfilePage};
    use crate::model::{AccountView, Chat, Person, Project, SpaceKind};
    use crate::sync::{ClientCommand, PushRefusal, SyncEvent, SyncSource};
    use crate::ui::motion::PulseClock;
    use crate::ui::profiles::user::{MESSAGE_ID, UNBLOCK_ID};
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

    fn sam() -> Person {
        person("gh_2", "sam", "Sam Rivera")
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
        let now = crate::model::now_ms();
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
                name: "sam".into(),
                kind: SpaceKind::Dm,
                people: vec![sam()],
                ..Project::default()
            });
            s.projects.push(Project {
                id: "g1".into(),
                name: "Launch week".into(),
                kind: SpaceKind::Group,
                people: vec![sam(), person("gh_3", "priya", "Priya Natarajan")],
                ..Project::default()
            });
            let chat = |id: &str, title: &str| -> Chat {
                serde_json::from_value(serde_json::json!({
                    "id": id, "project_id": id, "title": title, "is_pinned": true,
                    "is_system": false, "unread": false, "last_message_at": now,
                }))
                .expect("chat")
            };
            s.chats.insert("d1".into(), vec![chat("d1", "sam")]);
            s.chats.insert("g1".into(), vec![chat("g1", "Launch week")]);
            // A line from Sam, so the DM draws the transcript layout (not the empty hero).
            let hello: crate::model::Message = serde_json::from_value(serde_json::json!({
                "id": "m1", "chatId": "d1", "role": "user", "sender": "user:gh_2",
                "senderHandle": "sam", "senderName": "Sam Rivera", "status": "done",
                "createdAt": now - 60_000, "completedAt": now - 60_000,
                "parts": [{ "id": "m1-t", "kind": "text", "text": "hey" }],
            }))
            .expect("message");
            s.messages.insert("d1".into(), vec![hello]);
            s.active_tab.insert("d1".into(), "d1".into());
            s.active_tab.insert("g1".into(), "g1".into());
            s.blocked = vec![sam()];
            s.selected_project = Some("d1".into());
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.simulate_resize(size(px(1180.0), px(780.0)));
        vcx.run_until_parked();
        (rt, state, shell, vcx)
    }

    fn settle(state: &Entity<AppState>, shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        for _ in 0..3 {
            state.update(vcx, |_, cx| cx.notify());
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    fn centre(b: Bounds<Pixels>) -> gpui::Point<Pixels> {
        point(b.left() + b.size.width / 2.0, b.top() + b.size.height / 2.0)
    }

    fn click(
        selector: &'static str,
        state: &Entity<AppState>,
        shell: &Entity<Shell>,
        vcx: &mut VisualTestContext,
    ) {
        let b = vcx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("`{selector}` is drawn"));
        vcx.simulate_click(centre(b), Modifiers::default());
        settle(state, shell, vcx);
    }

    fn open_pane(state: &Entity<AppState>, vcx: &mut VisualTestContext, handle: &str, space: &str) {
        state.update(vcx, |s, cx| {
            s.open_profile(
                ProfilePage::User {
                    handle: handle.into(),
                    space_id: Some(space.into()),
                },
                cx,
            );
        });
    }

    fn unblocks(commands: &mut mpsc::UnboundedReceiver<ClientCommand>) -> Vec<String> {
        std::iter::from_fn(|| commands.try_recv().ok())
            .filter_map(|c| match c {
                ClientCommand::UnblockUser { handle, .. } => Some(handle),
                _ => None,
            })
            .collect()
    }

    /// The pane: Blocked chip under the @handle, Unblock pill instead of Message, the
    /// footer row reads Unblock; the pill opens the confirm in the row's place, Cancel puts
    /// the row back, Unblock sends `DELETE /blocks/{id}` once; unblocked → chip gone, Message.
    #[gpui::test]
    fn the_person_pane_says_blocked_and_unblock_asks_first(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        let mut commands = state.update(vcx, |s, _| s.take_command_channel());
        open_pane(&state, vcx, "sam", "g1");
        settle(&state, &shell, vcx);
        let chip = vcx.debug_bounds("blocked-chip").expect("Blocked chip");
        let handle = vcx.debug_bounds("hero-subtitle").expect("@handle line");
        assert!(
            chip.top() >= handle.bottom(),
            "the chip sits under the @handle"
        );
        assert!(
            vcx.debug_bounds(MESSAGE_ID).is_none(),
            "no Message pill while blocked"
        );
        let pill = vcx.debug_bounds(UNBLOCK_ID).expect("Unblock pill");
        assert!(pill.top() >= chip.bottom(), "the pill is under the chip");
        assert!(
            vcx.debug_bounds("profile-unblock-confirm").is_none(),
            "no confirm yet"
        );

        click(UNBLOCK_ID, &state, &shell, vcx);
        assert!(
            vcx.debug_bounds("profile-unblock-confirm").is_some(),
            "the pill asks first"
        );
        assert!(
            unblocks(&mut commands).is_empty(),
            "nothing sent before the confirm"
        );
        click("profile-unblock-cancel", &state, &shell, vcx);
        assert!(
            vcx.debug_bounds("profile-unblock-confirm").is_none(),
            "Cancel puts the row back"
        );
        assert!(unblocks(&mut commands).is_empty());

        click(UNBLOCK_ID, &state, &shell, vcx);
        click("profile-unblock-go", &state, &shell, vcx);
        assert_eq!(
            unblocks(&mut commands),
            vec!["sam".to_string()],
            "one DELETE /blocks"
        );
        assert!(vcx.debug_bounds("profile-unblock-confirm").is_none());
        // The relay's list comes back without Sam: the pane is plain again.
        state.update(vcx, |s, cx| {
            s.blocked.clear();
            cx.notify();
        });
        settle(&state, &shell, vcx);
        assert!(
            vcx.debug_bounds("blocked-chip").is_none(),
            "chip gone once unblocked"
        );
        assert!(vcx.debug_bounds(UNBLOCK_ID).is_none());
        assert!(vcx.debug_bounds(MESSAGE_ID).is_some(), "Message is back");
    }

    /// The DM: the header pill carries the Blocked tag and the composer is the bar — "You
    /// blocked Sam Rivera" with Unblock; Unblock → confirm in the bar's place; Cancel → bar;
    /// Unblock → the command; the relay's list without Sam → the composer is back, no tag.
    #[gpui::test]
    fn the_dm_header_tags_and_the_composer_is_the_blocked_bar(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        let mut commands = state.update(vcx, |s, _| s.take_command_channel());
        settle(&state, &shell, vcx);
        let tag = vcx
            .debug_bounds("blocked-tag")
            .expect("Blocked tag on the DM pill");
        let pill = vcx.debug_bounds("chat-header-pill").expect("header pill");
        assert!(
            tag.left() >= pill.left() && tag.right() <= pill.right(),
            "the tag is inside the pill: {tag:?} in {pill:?}"
        );
        let bar = vcx
            .debug_bounds("composer-blocked")
            .expect("the blocked bar");
        let block = vcx
            .debug_bounds("chat-composer-block")
            .expect("composer block");
        assert!(
            (f32::from(bar.size.height) - crate::ui::theme::COMPOSER_HEIGHT).abs() < 0.5,
            "the bar is the composer's height: {bar:?}"
        );
        assert!(bar.top() >= block.top() && bar.bottom() <= block.bottom() + px(0.5));
        assert!(
            vcx.debug_bounds("composer-field").is_none(),
            "no input while blocked"
        );

        click("composer-unblock", &state, &shell, vcx);
        assert!(
            vcx.debug_bounds("composer-unblock-confirm").is_some(),
            "asks first"
        );
        assert!(vcx.debug_bounds("composer-blocked").is_none());
        click("composer-unblock-cancel", &state, &shell, vcx);
        assert!(
            vcx.debug_bounds("composer-blocked").is_some(),
            "Cancel brings the bar back"
        );
        assert!(unblocks(&mut commands).is_empty());
        click("composer-unblock", &state, &shell, vcx);
        click("composer-unblock-go", &state, &shell, vcx);
        assert_eq!(unblocks(&mut commands), vec!["sam".to_string()]);
        state.update(vcx, |s, cx| {
            s.blocked.clear();
            cx.notify();
        });
        settle(&state, &shell, vcx);
        assert!(
            vcx.debug_bounds("composer-blocked").is_none(),
            "the composer is back"
        );
        assert!(
            vcx.debug_bounds("composer-field").is_some(),
            "the field is back"
        );
        assert!(
            vcx.debug_bounds("blocked-tag").is_none(),
            "no tag once unblocked"
        );
    }

    /// The group's member list: Sam's row carries the Blocked tag, Priya's does not.
    #[gpui::test]
    fn the_member_list_tags_the_blocked_member(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        state.update(vcx, |s, cx| {
            s.selected_project = Some("g1".into());
            s.open_profile(
                ProfilePage::Group {
                    space_id: "g1".into(),
                },
                cx,
            );
        });
        settle(&state, &shell, vcx);
        let sam_row = vcx.debug_bounds("profile-member-sam").expect("Sam's row");
        let priya_row = vcx
            .debug_bounds("profile-member-priya")
            .expect("Priya's row");
        let tag = vcx
            .debug_bounds("blocked-tag")
            .expect("one Blocked tag in the list");
        assert!(
            tag.top() >= sam_row.top() && tag.bottom() <= sam_row.bottom(),
            "the tag is on Sam's row: {tag:?} in {sam_row:?}"
        );
        assert!(
            tag.top() >= priya_row.bottom() || tag.bottom() <= priya_row.top(),
            "and not on Priya's"
        );
        state.update(vcx, |s, cx| {
            s.blocked.clear();
            cx.notify();
        });
        settle(&state, &shell, vcx);
        assert!(vcx.debug_bounds("blocked-tag").is_none());
    }

    /// The chat-level note for a refused push says who stands in the way; the
    /// `blocks_member` line offers Unblock, which opens the DM bar's confirm.
    #[gpui::test]
    fn the_refusal_note_names_the_block_and_offers_unblock(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        settle(&state, &shell, vcx);
        state.update(vcx, |s, cx| {
            s.apply(
                SyncEvent::PushRefused {
                    chat_id: "d1".into(),
                    refused: true,
                    why: PushRefusal {
                        code: "not_available".into(),
                        reason: Some("blocks_member".into()),
                        by_handle: Some("sam".into()),
                        by_name: Some("Sam Rivera".into()),
                    },
                },
                cx,
            );
        });
        settle(&state, &shell, vcx);
        assert!(
            vcx.debug_bounds("delivery-error").is_some(),
            "the note is up"
        );
        let why = state.read_with(vcx, |s, _| s.push_refused.get("d1").cloned().unwrap());
        assert_eq!(
            why.sentence(),
            "Couldn't post: you've blocked Sam Rivera, who is in this chat. Unblock to post."
        );
        click("delivery-error-unblock", &state, &shell, vcx);
        assert!(
            vcx.debug_bounds("composer-unblock-confirm").is_some(),
            "Unblock on the note opens the confirm"
        );
        // They blocked me: no Unblock to offer.
        state.update(vcx, |s, cx| {
            s.apply(
                SyncEvent::PushRefused {
                    chat_id: "d1".into(),
                    refused: true,
                    why: PushRefusal {
                        code: "not_available".into(),
                        reason: Some("blocked_by".into()),
                        by_handle: Some("sam".into()),
                        by_name: Some("Sam Rivera".into()),
                    },
                },
                cx,
            );
            s.blocked.clear();
            cx.notify();
        });
        shell.update(vcx, |sh, cx| sh.cancel_unblock(cx));
        settle(&state, &shell, vcx);
        let why = state.read_with(vcx, |s, _| s.push_refused.get("d1").cloned().unwrap());
        assert_eq!(why.sentence(), "Sam Rivera has blocked you in this chat.");
        assert!(
            vcx.debug_bounds("delivery-error-unblock").is_none(),
            "nothing to unblock"
        );
        // The room's next ACK clears the note.
        state.update(vcx, |s, cx| {
            s.apply(
                SyncEvent::PushRefused {
                    chat_id: "d1".into(),
                    refused: false,
                    why: PushRefusal::default(),
                },
                cx,
            );
        });
        settle(&state, &shell, vcx);
        assert!(vcx.debug_bounds("delivery-error").is_none());
    }
}

#[cfg(test)]
mod hidden_tests {
    //! A group block hides the blocked person's rows from the blocker alone (Jacob
    //! 2026-10-26): the transcript folds each run into one line with Unblock, the sidebar
    //! preview, unread dot, search and Library skip those rows, and unblocking reveals them.
    use std::collections::{HashMap, HashSet};

    use futures::future::BoxFuture;
    use gpui::{
        point, px, size, AppContext, Bounds, Entity, Modifiers, Pixels, TestAppContext,
        VisualTestContext,
    };
    use tokio::sync::mpsc;

    use crate::app::{AppState, ProfilePage};
    use crate::model::{AccountView, Chat, Message, Person, Project, SpaceKind};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::mention::Mentions;
    use crate::ui::motion::PulseClock;
    use crate::ui::shell::Shell;
    use crate::ui::theme::Theme;
    use crate::ui::transcript::{build_rows_hiding, RowKind, SpeechRows};

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

    fn row(id: &str, sender: &str, handle: &str, name: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "chatId": "g1", "role": "user", "sender": sender,
            "senderHandle": handle, "senderName": name, "status": "done",
            "createdAt": at, "completedAt": at,
            "parts": [{ "id": format!("{id}-t"), "kind": "text", "text": text }],
        }))
        .expect("message")
    }

    fn messages(now: u64) -> Vec<Message> {
        vec![
            row(
                "m1",
                "user:gh_3",
                "priya",
                "Priya Natarajan",
                "morning all",
                now - 50_000,
            ),
            row("m2", "user:gh_2", "sam", "Sam Rivera", "hey", now - 40_000),
            row(
                "m3",
                "user:gh_2",
                "sam",
                "Sam Rivera",
                "anyone around?",
                now - 30_000,
            ),
            row(
                "m4",
                "user:gh_3",
                "priya",
                "Priya Natarajan",
                "here",
                now - 20_000,
            ),
            row("m5", "user:gh_2", "sam", "Sam Rivera", "cool", now - 10_000),
        ]
    }

    /// Pure: Sam's two runs fold into two Hidden rows (2 then 1), Priya's rows stay.
    #[test]
    fn a_blocked_senders_runs_fold_into_hidden_rows() {
        let (spoken, failed) = (HashSet::new(), HashMap::new());
        let speech = SpeechRows {
            enabled: false,
            spoken: &spoken,
            failed: &failed,
            voice_mode: false,
        };
        let hidden: HashSet<String> = ["user:gh_2".to_string()].into_iter().collect();
        let rows = build_rows_hiding(
            &messages(1_700_000_000_000),
            false,
            speech,
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            true,
            &hidden,
        );
        let shape: Vec<String> = rows
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::User { .. } => Some(format!("user:{}", r.message_id)),
                RowKind::Hidden { count, handle, .. } => Some(format!("hidden:{handle}x{count}")),
                _ => None,
            })
            .collect();
        assert_eq!(
            shape,
            vec!["user:m1", "hidden:samx2", "user:m4", "hidden:samx1"],
            "{shape:?}"
        );
        // Nothing hidden: every row is a bubble.
        let rows = build_rows_hiding(
            &messages(1_700_000_000_000),
            false,
            speech,
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            true,
            &HashSet::new(),
        );
        assert_eq!(
            rows.iter()
                .filter(|r| matches!(r.kind, RowKind::User { .. }))
                .count(),
            5
        );
        assert!(!rows
            .iter()
            .any(|r| matches!(r.kind, RowKind::Hidden { .. })));
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
        let now = crate::model::now_ms();
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
                id: "g1".into(),
                name: "Launch week".into(),
                kind: SpaceKind::Group,
                people: vec![
                    person("gh_2", "sam", "Sam Rivera"),
                    person("gh_3", "priya", "Priya Natarajan"),
                ],
                blocked_senders: vec!["user:gh_2".into()],
                my_role: None,
                ..Project::default()
            });
            let chat: Chat = serde_json::from_value(serde_json::json!({
                "id": "g1", "project_id": "g1", "title": "Launch week", "is_pinned": true,
                "is_system": false, "unread": false, "last_message_at": now,
            }))
            .expect("chat");
            s.chats.insert("g1".into(), vec![chat]);
            s.active_tab.insert("g1".into(), "g1".into());
            s.messages.insert("g1".into(), messages(now));
            s.blocked = vec![person("gh_2", "sam", "Sam Rivera")];
            s.selected_project = Some("g1".into());
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.simulate_resize(size(px(1180.0), px(780.0)));
        vcx.run_until_parked();
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

    /// The real shell: Sam's bubbles are folded, Priya's drawn; the sidebar preview is
    /// Priya's last line, not Sam's; the folded line's Unblock opens Sam's pane with the
    /// confirm armed; once the relay's list clears, Sam's bubbles are back.
    #[gpui::test]
    fn the_blocker_sees_folded_lines_and_unblock_reveals(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        settle(&state, &shell, vcx);
        let bubbles = shell.read_with(vcx, |sh, cx| {
            sh.transcript
                .read(cx)
                .rows
                .iter()
                .filter(|r| matches!(r.kind, RowKind::User { .. }))
                .map(|r| r.message_id.clone())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            bubbles,
            vec!["m1".to_string(), "m4".to_string()],
            "only Priya's bubbles"
        );
        assert!(
            vcx.debug_bounds("hidden-row").is_some(),
            "a folded line is drawn"
        );
        let preview = state.read_with(vcx, |s, _| {
            let space = s.projects[0].clone();
            crate::ui::sidebar::preview_for_tests(s, &space)
        });
        assert_eq!(
            preview, "@priya: here",
            "the preview skips Sam's newest row"
        );
        // The folded line's Unblock: Sam's pane opens with the confirm up.
        let link = vcx
            .debug_bounds("hidden-row-unblock")
            .expect("Unblock on the fold");
        vcx.simulate_click(centre(link), Modifiers::default());
        settle(&state, &shell, vcx);
        state.read_with(vcx, |s, _| {
            assert!(matches!(
                &s.profile,
                Some(ProfilePage::User { handle, .. }) if handle == "sam"
            ));
        });
        assert!(
            vcx.debug_bounds("profile-unblock-confirm").is_some(),
            "the pane opens on the confirm"
        );
        // The relay's list comes back without Sam, and the hint is gone: all five bubbles.
        state.update(vcx, |s, cx| {
            s.blocked.clear();
            s.projects[0].blocked_senders.clear();
            cx.notify();
        });
        shell.update(vcx, |sh, cx| sh.cancel_unblock(cx));
        settle(&state, &shell, vcx);
        let bubbles = shell.read_with(vcx, |sh, cx| {
            sh.transcript
                .read(cx)
                .rows
                .iter()
                .filter(|r| matches!(r.kind, RowKind::User { .. }))
                .count()
        });
        assert_eq!(bubbles, 5, "unblocked: every row is back");
        assert!(vcx.debug_bounds("hidden-row").is_none());
    }

    /// Arrivals from a blocked sender neither light the unread dot nor notify; a third
    /// member's do.
    #[gpui::test]
    fn a_hidden_arrival_leaves_the_dot_dark(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        state.update(vcx, |s, cx| {
            // Another chat is on screen, so arrivals here count as unread.
            s.selected_project = None;
            cx.notify();
        });
        settle(&state, &shell, vcx);
        let now = crate::model::now_ms();
        state.update(vcx, |s, cx| {
            let mut m = s.messages.get("g1").cloned().unwrap();
            m.push(row("m6", "user:gh_2", "sam", "Sam Rivera", "psst", now));
            s.apply(
                SyncEvent::ChatSnapshot {
                    chat_id: "g1".into(),
                    messages: m,
                },
                cx,
            );
        });
        settle(&state, &shell, vcx);
        state.read_with(vcx, |s, _| {
            let chat = &s.chats["g1"][0];
            assert!(!chat.unread, "Sam's row is hidden for me: no dot");
        });
        state.update(vcx, |s, cx| {
            let mut m = s.messages.get("g1").cloned().unwrap();
            m.push(row(
                "m7",
                "user:gh_3",
                "priya",
                "Priya Natarajan",
                "hi!",
                now + 1,
            ));
            s.apply(
                SyncEvent::ChatSnapshot {
                    chat_id: "g1".into(),
                    messages: m,
                },
                cx,
            );
        });
        settle(&state, &shell, vcx);
        state.read_with(vcx, |s, _| {
            assert!(s.chats["g1"][0].unread, "Priya's row lights it");
        });
    }
}
