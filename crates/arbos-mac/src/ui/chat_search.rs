//! The Search-in-chat bar ([feedback search-in-chat-mac-2026-10-26], Jacob
//! 2026-10-26: "bring back command F to find things in a chat by key word,
//! user name etc."). The Mac twin of the iPhone's bar (#1829): ⌘F drops it
//! under the header pill, in the header's own material (frosted on Glass and
//! Matte alike, `glass::header_surface`), at the composer's width; it holds
//! the magnifier, the pinned sender chip, the field, an offered sender chip
//! while a name is typed, the count, ‹ ›, ✕. The transcript below starts
//! `metrics::BELOW` under it — the bar pushes the rows down, the pill stays.
//!
//! What a hit looks like: every hit's words on the accent wash, the revealed
//! hit's words in Find's yellow, and the revealed message's row washed and
//! scrolled to through the palette's reveal path (`AppState::chat_search_reveal`).
//! The rules (hits, cursor, count) are `crate::chat_search`; the state is
//! `AppState::chat_search`.

use gpui::{actions, div, prelude::*, px, AnyElement, Context, Focusable, Hsla, Stateful};

use crate::app::AppState;
use crate::chat_search::Sender;
use crate::ui::avatars;
use crate::ui::glass;
use crate::ui::highlight;
use crate::ui::select::SearchMarks;
use crate::ui::sf_symbols;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, TEXT_SMALL};

actions!(
    chat_search,
    [
        /// ⌘F: open the bar (or put the cursor back in it).
        Open,
        /// Enter / › / ⌘G: the next (newer) hit, wrapping.
        Next,
        /// ⇧Enter / ‹ / ⇧⌘G: the previous (older) hit; on the oldest loaded hit
        /// of a channel, page older rows first.
        Prev,
        /// Esc / ✕: close the bar.
        Close,
        /// ⇥ in the field: pin the offered sender.
        PinSender,
    ]
);

/// Geometry every number of the bar comes from — read by the tests and the rule
/// in `docs/mac-design-rules.md`.
pub mod metrics {
    use crate::ui::header::metrics::{PILL_HEIGHT, PILL_TOP};
    use crate::ui::theme::{BUBBLE_RADIUS, COMPOSER_WIDTH, CONTROL, HEADER_HEIGHT, TEXT_BODY};

    /// 8 under the pill (the iPhone's spacing between header and bar) → 56.
    pub const GAP_UNDER_PILL: f32 = 8.0;
    pub const TOP: f32 = PILL_TOP + PILL_HEIGHT + GAP_UNDER_PILL;
    /// The button family's height.
    pub const HEIGHT: f32 = CONTROL;
    /// The iPhone bar's corner, the bubbles' radius.
    pub const RADIUS: f32 = BUBBLE_RADIUS;
    /// The composer's width: bar, rows and composer share their edges.
    pub const WIDTH: f32 = COMPOSER_WIDTH;
    /// How far the bar pushes the transcript down from where it starts
    /// without one (the band's end): the 44 of the approved mock.
    pub const PUSH: f32 = 44.0;
    /// Space between the bar and the first row — what is left of `PUSH`
    /// once the bar's own 36 and its 4 over the band's end are taken: 12.
    pub const BELOW: f32 = PUSH + HEADER_HEIGHT - TOP - HEIGHT;
    pub const INSET: f32 = 12.0;
    pub const INSET_RIGHT: f32 = 10.0;
    pub const GAP: f32 = 8.0;
    pub const GLYPH: f32 = 15.0;
    pub const FIELD_FONT: f32 = TEXT_BODY;
    pub const CHIP_HEIGHT: f32 = 24.0;
    pub const CHIP_AVATAR: f32 = 18.0;
    pub const CHIP_FONT: f32 = 12.5;
    pub const CHIP_ALPHA: f32 = 0.18;
    pub const NAV: f32 = 24.0;
    pub const NAV_GLYPH: f32 = 16.0;
    pub const CLOSE: f32 = 16.0;
}

/// Every hit's words: the accent at 30 %.
pub const WASH_ALPHA: f32 = 0.30;
/// The revealed hit's words: macOS Find's yellow, dark text on it.
pub fn find_yellow() -> Hsla {
    gpui::rgb(0xffd60a).into()
}
pub fn find_text() -> Hsla {
    gpui::rgb(0x111111).into()
}

/// What the transcript marks on one message while the bar is up.
pub fn marks_for(state: &AppState, message_id: &str, theme: &Theme) -> Option<SearchMarks> {
    let marks = state.chat_search_marks(message_id)?;
    Some(SearchMarks {
        needle: marks.needle,
        current: marks.current,
        wash: Theme::with_alpha(theme.accent, WASH_ALPHA),
        current_wash: find_yellow(),
        current_text: find_text(),
    })
}

/// The bar, when the active chat's search is open: the first thing under the
/// header, centred at the composer's width.
pub fn bar(shell: &Shell, cx: &mut Context<Shell>) -> Option<AnyElement> {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    if !state.chat_search_open() {
        return None;
    }
    let search = state.chat_search.clone()?;
    let offered = state.chat_search_offer();
    let count = state.chat_search_count();
    let has_hits = !search.hits.is_empty();
    let pages = search.searching_older;

    let mut bar = div()
        .id("chat-search-bar")
        .key_context("ChatSearch")
        .debug_selector(|| "chat-search-bar".into())
        .h(px(metrics::HEIGHT))
        .w_full()
        .max_w(px(metrics::WIDTH))
        .rounded(px(metrics::RADIUS))
        .flex()
        .items_center()
        .gap(px(metrics::GAP))
        .pl(px(metrics::INSET))
        .pr(px(metrics::INSET_RIGHT))
        .cursor_text()
        // The bar owns the pointer over its box like the palette's panel
        // ([feedback search-popup-swallows-hover-2026-10-28]): nothing under
        // it hovers, and a wheel over it never reaches the transcript.
        .occlude()
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                let handle = this.chat_search_input.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
                cx.stop_propagation();
            }),
        )
        .child(sf_symbols::glyph(
            "magnifyingglass",
            metrics::GLYPH,
            theme.text_muted,
        ));

    if let Some(sender) = search.query.sender.clone() {
        bar = bar.child(pinned_chip(&sender, &theme, cx));
    }

    let mut field = div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .gap(px(metrics::GAP))
        .text_size(px(metrics::FIELD_FONT))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(shell.chat_search_input.clone()),
        );
    if let Some(sender) = offered {
        field = field.child(offered_chip(&sender, &theme, cx));
    }
    bar = bar.child(field);

    let mut right = div().flex_shrink_0().flex().items_center().gap(px(4.0));
    if pages {
        right = right.child(
            div()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .mr(px(6.0))
                .debug_selector(|| "chat-search-status".into())
                .child("searching older…"),
        );
    }
    if !count.is_empty() {
        right = right.child(
            div()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .mr(px(6.0))
                .debug_selector(|| "chat-search-count".into())
                .child(count),
        );
    }
    right = right
        .child(
            nav(
                "chat-search-prev",
                "chevron.left",
                has_hits || pages,
                &theme,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.state.update(cx, |s, cx| s.chat_search_step(-1, cx));
            })),
        )
        .child(
            nav("chat-search-next", "chevron.right", has_hits, &theme).on_click(cx.listener(
                |this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.chat_search_step(1, cx));
                },
            )),
        )
        .child(
            div()
                .id("chat-search-close")
                .size(px(metrics::CLOSE))
                .ml(px(6.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .map(|el| highlight::washed(el, theme.text_muted, &theme))
                .debug_selector(|| "chat-search-close".into())
                .child(sf_symbols::glyph("xmark", 9.0, theme.bg))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_chat_search(window, cx);
                })),
        );
    bar = bar.child(right);

    let bar = glass::header_surface(bar, &theme);
    let bar = glass::header_frost(metrics::RADIUS, bar);
    // A click anywhere outside the bar closes it without being swallowed
    // (`Shell::click_outside`, capture phase, against these bounds).
    let bounds = shell.chat_search_bounds.clone();
    Some(
        div()
            .w_full()
            .flex()
            .justify_center()
            .mt(px(metrics::TOP - crate::ui::header::metrics::HEIGHT))
            .mb(px(metrics::BELOW))
            .px(px(16.0))
            .debug_selector(|| "chat-search-row".into())
            .on_children_prepainted(move |children, _, _| {
                bounds.set(children.first().copied());
            })
            .child(bar)
            .into_any_element(),
    )
}

/// ‹ or ›: a 24 pt hit target around a chevron; faint when there is nothing to step to.
fn nav(id: &'static str, glyph: &str, on: bool, theme: &Theme) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(metrics::NAV))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .justify_center()
        .when(on, |el| highlight::button(el.cursor_pointer(), theme))
        .debug_selector(move || id.into())
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(sf_symbols::glyph(
            glyph,
            metrics::NAV_GLYPH,
            if on { theme.text } else { theme.text_faint },
        ))
}

fn chip_base(id: &'static str, sender: &Sender, theme: &Theme) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .h(px(metrics::CHIP_HEIGHT))
        .rounded_full()
        .pl(px(3.0))
        .pr(px(8.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(metrics::CHIP_FONT))
        .font_weight(gpui::FontWeight::MEDIUM)
        .debug_selector(move || id.into())
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(avatars::element(
            &sender.key,
            &sender.name,
            None,
            metrics::CHIP_AVATAR,
            theme,
        ))
        .child(format!("From {}", sender.name))
}

/// The pinned sender: accent chip with a ✕ that unpins it.
fn pinned_chip(sender: &Sender, theme: &Theme, cx: &mut Context<Shell>) -> AnyElement {
    chip_base("chat-search-sender", sender, theme)
        .bg(Theme::with_alpha(theme.accent, metrics::CHIP_ALPHA))
        .text_color(theme.accent)
        .child(sf_symbols::glyph("xmark", 9.0, theme.accent))
        .cursor_pointer()
        .on_click(cx.listener(|this, _, window, cx| {
            this.state
                .update(cx, |s, cx| s.chat_search_unpin_sender(cx));
            let handle = this.chat_search_input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }))
        .into_any_element()
}

/// The sender the typed text names, offered: ⇥ or a click pins it.
fn offered_chip(sender: &Sender, theme: &Theme, cx: &mut Context<Shell>) -> AnyElement {
    let sender_for_click = sender.clone();
    chip_base("chat-search-offer", sender, theme)
        .bg(Theme::with_alpha(theme.text, 0.08))
        .border_1()
        .border_color(theme.border_strong)
        .text_color(theme.text)
        .child(
            div()
                .h(px(15.0))
                .px(px(4.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(theme.border_strong)
                .text_size(px(10.0))
                .text_color(theme.text_muted)
                .flex()
                .items_center()
                .child("⇥"),
        )
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, window, cx| {
            this.pin_chat_search_sender(sender_for_click.clone(), window, cx);
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use futures::future::BoxFuture;
    use gpui::{AppContext as _, Entity, Focusable, TestAppContext};
    use tokio::sync::mpsc;

    use super::metrics;
    use crate::app::AppState;
    use crate::model::{now_ms, ChannelSel, Chat, Message, Person, Project, SpaceKind};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::header::metrics::{PILL_HEIGHT, PILL_TOP};
    use crate::ui::motion::PulseClock;
    use crate::ui::shell::Shell;
    use crate::ui::theme::{Theme, BUBBLE_RADIUS, COMPOSER_WIDTH, CONTROL};

    const DM: &str = "01DMSAM0000000000000000000";
    const ROOM: &str = "!room:arbos.life";

    /// A sync source that keeps the command channel so a test can read what
    /// the state asked for (`ChannelLoadOlder`).
    struct Recording(Arc<Mutex<Option<mpsc::UnboundedReceiver<ClientCommand>>>>);

    impl SyncSource for Recording {
        fn name(&self) -> &'static str {
            "recording"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            *self.0.lock().expect("commands slot") = Some(commands);
            Box::pin(std::future::pending())
        }
    }

    fn row(chat: &str, id: &str, sender: &str, handle: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "chatId": chat, "role": "user", "sender": sender,
            "senderHandle": handle, "senderName": if handle == "sam" { "Sam Rivera" } else { "Jacob" },
            "status": "done", "createdAt": at, "completedAt": at,
            "parts": [{ "id": format!("{id}-t"), "kind": "text", "text": text }],
        }))
        .expect("message")
    }

    type Commands = Arc<Mutex<Option<mpsc::UnboundedReceiver<ClientCommand>>>>;

    /// Me (`gh_me`) in a DM with Sam: four rows, "canvas" in three of them,
    /// two of those Sam's.
    fn mount(
        cx: &mut TestAppContext,
    ) -> (
        tokio::runtime::Runtime,
        Commands,
        Entity<AppState>,
        Entity<Shell>,
        &mut gpui::VisualTestContext,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let commands: Commands = Arc::new(Mutex::new(None));
        let state = cx.new(|cx| AppState::new(Box::new(Recording(commands.clone())), cx));
        let now = now_ms();
        state.update(cx, |s, cx| {
            let sam = Person {
                id: "gh_2".into(),
                handle: "sam".into(),
                name: Some("Sam Rivera".into()),
                ..Person::default()
            };
            s.projects.push(Project {
                id: DM.into(),
                name: "sam".into(),
                kind: SpaceKind::Dm,
                people: vec![sam],
                ..Project::default()
            });
            let chat: Chat = serde_json::from_value(serde_json::json!({
                "id": DM, "project_id": DM, "title": "sam", "is_pinned": true,
                "is_system": false, "unread": false, "last_message_at": now,
            }))
            .expect("chat");
            s.chats.insert(DM.into(), vec![chat]);
            s.messages.insert(
                DM.into(),
                vec![
                    row(
                        DM,
                        "d1",
                        "user:gh_2",
                        "sam",
                        "Keep the canvas infinite.",
                        now - 90_000,
                    ),
                    row(
                        DM,
                        "d2",
                        "user:gh_me",
                        "unconst",
                        "And the export flow?",
                        now - 80_000,
                    ),
                    row(
                        DM,
                        "d3",
                        "user:gh_2",
                        "sam",
                        "Export keeps the Canvas background.",
                        now - 70_000,
                    ),
                    row(
                        DM,
                        "d4",
                        "user:gh_me",
                        "unconst",
                        "Canvas zoom stays 25–400 %.",
                        now - 60_000,
                    ),
                ],
            );
            s.selected_project = Some(DM.into());
            s.active_tab.insert(DM.into(), DM.into());
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.run_until_parked();
        settle(&state, &shell, vcx);
        (rt, commands, state, shell, vcx)
    }

    fn settle(state: &Entity<AppState>, shell: &Entity<Shell>, vcx: &mut gpui::VisualTestContext) {
        for _ in 0..3 {
            state.update(vcx, |_, cx| cx.notify());
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    fn hits(state: &Entity<AppState>, vcx: &gpui::VisualTestContext) -> Vec<String> {
        state.read_with(vcx, |s, _| {
            s.chat_search
                .as_ref()
                .map(|c| c.hits.clone())
                .unwrap_or_default()
        })
    }

    fn current(state: &Entity<AppState>, vcx: &gpui::VisualTestContext) -> Option<String> {
        state.read_with(vcx, |s, _| {
            s.chat_search
                .as_ref()
                .and_then(|c| c.current.and_then(|i| c.hits.get(i).cloned()))
        })
    }

    /// ⌘F drops the bar 8 under the pill, 36 tall, the composer's width, and
    /// pushes the first row down; Esc takes it away again and the composer
    /// has the keyboard.
    #[gpui::test]
    fn cmd_f_opens_the_bar_under_the_pill_and_esc_closes_it(cx: &mut TestAppContext) {
        let (_rt, _cmds, state, shell, vcx) = mount(cx);
        assert!(
            vcx.debug_bounds("chat-search-bar").is_none(),
            "no bar before ⌘F"
        );
        assert!(!state.read_with(&*vcx, |s, _| s.chat_search_open()));

        vcx.dispatch_action(super::Open);
        settle(&state, &shell, vcx);
        assert!(state.read_with(&*vcx, |s, _| s.chat_search_open()));
        let bar = vcx
            .debug_bounds("chat-search-bar")
            .expect("the bar is drawn");
        let header = vcx
            .debug_bounds("chat-header-bar")
            .expect("the header is drawn");
        let top: f32 = (bar.origin.y - header.origin.y).into();
        let h: f32 = bar.size.height.into();
        let w: f32 = bar.size.width.into();
        let pane_w: f32 = header.size.width.into();
        assert_eq!(top, metrics::TOP, "8 under the pill");
        assert_eq!(h, metrics::HEIGHT);
        assert_eq!(w, metrics::WIDTH.min(pane_w - 32.0), "the composer's width");
        let bar_cx: f32 = (bar.origin.x + bar.size.width / 2.0).into();
        let pane_cx: f32 = (header.origin.x + header.size.width / 2.0).into();
        assert!((bar_cx - pane_cx).abs() <= 1.0, "centred like the composer");
        // The field has the keyboard.
        let handle = shell.read_with(&*vcx, |sh, cx| {
            sh.chat_search_input.read(cx).focus_handle(cx)
        });
        let focused = vcx.update(|window, _| handle.is_focused(window));
        assert!(focused, "⌘F focuses the field");

        vcx.dispatch_action(crate::ui::shell::Escape);
        settle(&state, &shell, vcx);
        assert!(!state.read_with(&*vcx, |s, _| s.chat_search_open()));
        assert!(
            vcx.debug_bounds("chat-search-bar").is_none(),
            "Esc closes the bar"
        );
        assert!(
            state.read_with(&*vcx, |s, _| s.highlight_message.is_none()),
            "marks gone"
        );
    }

    /// ⌘F is a toggle ([feedback search-toggle-click-outside-mac-2026-10-27]):
    /// pressed on the open bar — with the field focused — it closes it like
    /// ✕ / Esc: marks gone, composer focused. ⌘⇧F toggles the palette the
    /// same way, and the two never stack.
    #[gpui::test]
    fn cmd_f_and_cmd_shift_f_toggle_their_search_closed_again(cx: &mut TestAppContext) {
        let (_rt, _cmds, state, shell, vcx) = mount(cx);
        vcx.dispatch_action(super::Open);
        settle(&state, &shell, vcx);
        vcx.simulate_input("canvas");
        settle(&state, &shell, vcx);
        assert!(state.read_with(&*vcx, |s, _| s.highlight_message.is_some()));
        let field = shell.read_with(&*vcx, |sh, cx| {
            sh.chat_search_input.read(cx).focus_handle(cx)
        });
        assert!(
            vcx.update(|window, _| field.is_focused(window)),
            "the field has the keyboard"
        );
        // ⌘F again, from inside the field: the chord reaches the shell, the bar closes.
        vcx.dispatch_action(super::Open);
        settle(&state, &shell, vcx);
        assert!(
            !state.read_with(&*vcx, |s, _| s.chat_search_open()),
            "⌘F closes the open bar"
        );
        assert!(vcx.debug_bounds("chat-search-bar").is_none());
        assert!(
            state.read_with(&*vcx, |s, _| s.highlight_message.is_none()),
            "marks cleared"
        );
        let composer = shell.read_with(&*vcx, |sh, cx| {
            sh.composer.read(cx).input.read(cx).focus_handle(cx)
        });
        assert!(
            vcx.update(|window, _| composer.is_focused(window)),
            "the composer types again"
        );

        // The palette: ⌘⇧F opens, ⌘⇧F (field focused) closes.
        vcx.dispatch_action(crate::ui::shell::OpenPalette);
        settle(&state, &shell, vcx);
        assert!(shell.read_with(&*vcx, |sh, _| sh.palette.open));
        assert!(vcx.debug_bounds("palette-panel").is_some());
        vcx.dispatch_action(crate::ui::shell::OpenPalette);
        settle(&state, &shell, vcx);
        assert!(
            !shell.read_with(&*vcx, |sh, _| sh.palette.open),
            "⌘⇧F closes the open palette"
        );
        assert!(vcx.debug_bounds("palette-panel").is_none());
        assert!(vcx.update(|window, _| composer.is_focused(window)));
    }

    /// A click outside the bar closes it and is not swallowed: the sidebar's
    /// "+" disc under that very click opens its sheet. A click inside the bar
    /// keeps it. The palette: the same, through its see-through backdrop.
    #[gpui::test]
    fn a_click_outside_closes_the_bar_or_palette_and_still_lands_on_its_target(
        cx: &mut TestAppContext,
    ) {
        let (_rt, _cmds, state, shell, vcx) = mount(cx);
        // The palette: a click in the panel keeps it; a click on the "+" disc
        // through the see-through backdrop dismisses it AND opens the sheet.
        vcx.dispatch_action(crate::ui::shell::OpenPalette);
        settle(&state, &shell, vcx);
        let field = vcx
            .debug_bounds("palette-field")
            .expect("palette field drawn");
        vcx.simulate_click(field.center(), gpui::Modifiers::default());
        settle(&state, &shell, vcx);
        assert!(
            shell.read_with(&*vcx, |sh, _| sh.palette.open),
            "a click in the panel keeps it"
        );
        let plus = vcx
            .debug_bounds("sidebar-new")
            .expect("+ disc drawn under the backdrop");
        vcx.simulate_click(plus.center(), gpui::Modifiers::default());
        settle(&state, &shell, vcx);
        assert!(
            !shell.read_with(&*vcx, |sh, _| sh.palette.open),
            "click outside dismisses the palette"
        );
        assert!(
            state.read_with(&*vcx, |s, _| s.new_chat_open),
            "the + under the click still opened"
        );
        // Sheet away, keyboard back on the composer (the sheet's field left the tree).
        state.update(vcx, |s, cx| s.close_new_chat(cx));
        vcx.update(|window, cx| {
            shell.update(cx, |sh, cx| {
                crate::ui::composer::Composer::focus(&sh.composer, window, cx)
            })
        });
        settle(&state, &shell, vcx);

        // The bar, same rule.
        vcx.dispatch_action(super::Open);
        settle(&state, &shell, vcx);
        vcx.simulate_input("canvas");
        settle(&state, &shell, vcx);
        let count = vcx.debug_bounds("chat-search-count").expect("count drawn");
        vcx.simulate_click(count.center(), gpui::Modifiers::default());
        settle(&state, &shell, vcx);
        assert!(
            state.read_with(&*vcx, |s, _| s.chat_search_open()),
            "a click in the bar keeps it"
        );
        let plus = vcx.debug_bounds("sidebar-new").expect("+ disc drawn");
        vcx.simulate_click(plus.center(), gpui::Modifiers::default());
        settle(&state, &shell, vcx);
        assert!(
            !state.read_with(&*vcx, |s, _| s.chat_search_open()),
            "click outside closes the bar"
        );
        assert!(
            state.read_with(&*vcx, |s, _| s.new_chat_open),
            "the + under the click still opened"
        );
    }

    /// Typing finds messages (not words), the count reads "n of m", Enter /
    /// ⇧Enter step and wrap, the current hit is revealed through the palette's
    /// path (highlight + reveal), and a typed name offers the sender chip that
    /// ⇥ pins — then sender AND keyword.
    #[gpui::test]
    fn typing_counts_steps_wraps_and_pins_a_sender(cx: &mut TestAppContext) {
        let (_rt, _cmds, state, shell, vcx) = mount(cx);
        vcx.dispatch_action(super::Open);
        settle(&state, &shell, vcx);
        vcx.simulate_input("canvas");
        settle(&state, &shell, vcx);
        assert_eq!(hits(&state, vcx), vec!["d1", "d3", "d4"]);
        assert_eq!(
            current(&state, vcx).as_deref(),
            Some("d4"),
            "starts on the newest hit"
        );
        assert_eq!(
            state.read_with(&*vcx, |s, _| s.chat_search_count()),
            "3 of 3"
        );
        assert!(vcx.debug_bounds("chat-search-count").is_some());
        let (highlight, reveal) = state.read_with(&*vcx, |s, _| {
            (s.highlight_message.clone(), s.reveal_message.clone())
        });
        assert_eq!(highlight.as_deref(), Some("d4"), "the palette's row wash");
        assert_eq!(
            reveal,
            Some((DM.into(), "d4".into())),
            "the palette's scroll-to"
        );
        // Marks: hit rows carry the needle, the current one is yellow.
        let (d1, d3, d2) = state.read_with(&*vcx, |s, _| {
            (
                s.chat_search_marks("d1"),
                s.chat_search_marks("d3"),
                s.chat_search_marks("d2"),
            )
        });
        assert_eq!(
            d1.as_ref().map(|m| (m.needle.as_str(), m.current)),
            Some(("canvas", false))
        );
        assert_eq!(d3.as_ref().map(|m| m.current), Some(false));
        assert!(d2.is_none(), "no mark on a row without the word");

        // ⇧Enter → older; Enter twice → wraps to the oldest.
        vcx.dispatch_action(super::Prev);
        settle(&state, &shell, vcx);
        assert_eq!(current(&state, vcx).as_deref(), Some("d3"));
        assert_eq!(
            state.read_with(&*vcx, |s, _| s.chat_search_count()),
            "2 of 3"
        );
        vcx.dispatch_action(super::Next);
        vcx.dispatch_action(super::Next);
        settle(&state, &shell, vcx);
        assert_eq!(
            current(&state, vcx).as_deref(),
            Some("d1"),
            "wrapped to the oldest"
        );
        assert_eq!(
            state.read_with(&*vcx, |s, _| s.chat_search_count()),
            "1 of 3"
        );
        vcx.dispatch_action(super::Prev);
        settle(&state, &shell, vcx);
        assert_eq!(
            current(&state, vcx).as_deref(),
            Some("d4"),
            "a chat doc is whole: ‹ wraps too"
        );

        // A name: the shared matcher offers Sam; ⇥ pins her; "canvas" then ANDs.
        shell.update(vcx, |sh, cx| {
            sh.chat_search_input.update(cx, |i, cx| i.clear(cx))
        });
        settle(&state, &shell, vcx);
        assert_eq!(
            state.read_with(&*vcx, |s, _| s.chat_search_count()),
            "",
            "nothing typed"
        );
        vcx.simulate_input("riv");
        settle(&state, &shell, vcx);
        let offered = state.read_with(&*vcx, |s, _| s.chat_search_offer().map(|p| p.name));
        assert_eq!(
            offered.as_deref(),
            Some("Sam Rivera"),
            "word-start on the display name"
        );
        assert!(vcx.debug_bounds("chat-search-offer").is_some());
        assert_eq!(
            state.read_with(&*vcx, |s, _| s.chat_search_count()),
            "No matches"
        );
        vcx.dispatch_action(super::PinSender);
        settle(&state, &shell, vcx);
        assert!(
            vcx.debug_bounds("chat-search-sender").is_some(),
            "the chip is pinned"
        );
        assert_eq!(hits(&state, vcx), vec!["d1", "d3"], "all of Sam's rows");
        assert_eq!(
            state.read_with(&*vcx, |s, _| s.chat_search_count()),
            "2 of 2"
        );
        vcx.simulate_input("background");
        settle(&state, &shell, vcx);
        assert_eq!(hits(&state, vcx), vec!["d3"], "Sam AND background");
        assert!(
            state.read_with(&*vcx, |s, _| s.chat_search_offer().is_none()),
            "no offer once pinned"
        );
    }

    /// A server channel: ‹ on the oldest loaded hit asks the relay for older
    /// rows (`ChannelLoadOlder`), the page's older hit is revealed when it
    /// lands, and "no more" turns ‹ into a wrap. State-level: the fixture has
    /// no server registry, so the bar itself is not drawn for the room.
    #[gpui::test]
    fn prev_on_the_oldest_hit_of_a_channel_pages_older_rows_then_wraps(cx: &mut TestAppContext) {
        let (rt, cmds, state, shell, vcx) = mount(cx);
        // The source runs on tokio (`Tokio::spawn`); give the current-thread
        // runtime a turn so `Recording::run` stashes the command receiver.
        for _ in 0..3 {
            rt.block_on(tokio::task::yield_now());
        }
        let now = now_ms();
        state.update(vcx, |s, cx| {
            s.channel_rows.insert(
                ROOM.into(),
                vec![
                    row(
                        ROOM,
                        "r5",
                        "user:gh_2",
                        "sam",
                        "the deck is in the folder",
                        now - 50_000,
                    ),
                    row(ROOM, "r6", "user:gh_me", "unconst", "thanks", now - 40_000),
                ],
            );
            s.selected_project = None;
            s.selected_channel = Some(ChannelSel {
                server_id: "srv".into(),
                channel_id: "ch".into(),
                room_id: Some(ROOM.into()),
            });
            assert!(s.open_chat_search(cx), "a channel is a chat: the bar opens");
            s.chat_search_set_text("deck".into(), cx);
        });
        settle(&state, &shell, vcx);
        assert_eq!(hits(&state, vcx), vec!["r5"]);
        assert_eq!(current(&state, vcx).as_deref(), Some("r5"));

        state.update(vcx, |s, cx| s.chat_search_step(-1, cx));
        settle(&state, &shell, vcx);
        let paging = state.read_with(&*vcx, |s, _| {
            s.chat_search.as_ref().map(|c| c.searching_older)
        });
        assert_eq!(paging, Some(true), "‹ on the oldest loaded hit pages");
        let asked = {
            let mut slot = cmds.lock().expect("commands");
            let rx = slot.as_mut().expect("the source holds the commands");
            let mut asked = Vec::new();
            while let Ok(c) = rx.try_recv() {
                asked.push(c);
            }
            asked
        };
        assert!(
            asked.iter().any(
                |c| matches!(c, ClientCommand::ChannelLoadOlder { room_id } if room_id == ROOM)
            ),
            "ChannelLoadOlder sent for the room: {asked:?}"
        );

        // The page lands (the timeline is whole each time): an older hit, revealed.
        state.update(vcx, |s, cx| {
            let rows = s.channel_rows.get_mut(ROOM).expect("room");
            rows.insert(
                0,
                row(ROOM, "r4", "user:gh_me", "unconst", "fine", now - 60_000),
            );
            rows.insert(
                0,
                row(
                    ROOM,
                    "r3",
                    "user:gh_2",
                    "sam",
                    "second deck slide says Q3",
                    now - 70_000,
                ),
            );
            s.chat_search_refresh(ROOM, cx);
        });
        settle(&state, &shell, vcx);
        assert_eq!(hits(&state, vcx), vec!["r3", "r5"]);
        assert_eq!(
            current(&state, vcx).as_deref(),
            Some("r3"),
            "moved on to the older hit"
        );
        assert_eq!(
            state.read_with(&*vcx, |s, _| s.chat_search_count()),
            "1 of 2"
        );
        let (paging, highlight) = state.read_with(&*vcx, |s, _| {
            (
                s.chat_search.as_ref().map(|c| c.searching_older),
                s.highlight_message.clone(),
            )
        });
        assert_eq!(paging, Some(false));
        assert_eq!(
            highlight.as_deref(),
            Some("r3"),
            "revealed through the palette's path"
        );

        // ‹ again: pages; the relay says there is nothing above → exhausted; ‹ wraps.
        state.update(vcx, |s, cx| s.chat_search_step(-1, cx));
        settle(&state, &shell, vcx);
        state.update(vcx, |s, cx| s.chat_search_history_end(ROOM, cx));
        settle(&state, &shell, vcx);
        let (paging, exhausted) = state.read_with(&*vcx, |s, _| {
            let c = s.chat_search.as_ref().expect("open");
            (c.searching_older, c.exhausted)
        });
        assert!(!paging && exhausted);
        state.update(vcx, |s, cx| s.chat_search_step(-1, cx));
        settle(&state, &shell, vcx);
        assert_eq!(
            current(&state, vcx).as_deref(),
            Some("r5"),
            "wraps to the newest"
        );
    }

    /// The numbers Jacob approved on the mock (2026-10-26 14:18 CR): 8 under
    /// the pill, the family's 36, the iPhone bar's 12 pt corner, the
    /// composer's width, and a 44 pt push on the transcript.
    #[test]
    fn the_bar_sits_eight_under_the_pill_at_the_familys_height_and_the_composers_width() {
        assert_eq!(PILL_TOP + PILL_HEIGHT, 48.0, "the pill ends at 48");
        assert_eq!(metrics::TOP, 56.0);
        assert_eq!(metrics::HEIGHT, CONTROL);
        assert_eq!(metrics::HEIGHT, 36.0);
        assert_eq!(metrics::RADIUS, BUBBLE_RADIUS);
        assert_eq!(metrics::WIDTH, COMPOSER_WIDTH);
        assert_eq!(metrics::BELOW, 12.0);
        assert_eq!(
            metrics::PUSH,
            44.0,
            "the transcript moves down 44 (approved mock)"
        );
        assert_eq!(
            metrics::TOP - (PILL_TOP + PILL_HEIGHT),
            metrics::GAP_UNDER_PILL,
            "under the pill, not over it"
        );
    }
}
