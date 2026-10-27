//! Arbos desktop client. GPUI on the zui fork; tokio for the sync sources.

mod app;
mod audio;
mod auth;
mod calls;
mod chat_search;
mod image_picker;
mod jobtabs;
mod model;
mod onboarding;
mod palette;
mod panic_log;
mod search;
mod search_index;
mod sign_in;
mod sync;
mod ui;
mod update;

use gpui::{
    point, px, size, App, AppContext as _, Bounds, KeyBinding, Menu, MenuItem, OsAction,
    TitlebarOptions, WindowBounds, WindowDecorations, WindowOptions,
};

use std::cell::RefCell;
use std::rc::Rc;

use crate::app::AppState;
use crate::ui::chat_search as chat_search_ui;
use crate::ui::composer;
use crate::ui::feedback;
use crate::ui::media_viewer;
use crate::ui::motion::{self, PulseClock};
use crate::ui::palette as palette_ui;
use crate::ui::shell::{self, Shell};
use crate::ui::theme::{
    reapply_window_background, AppearancePref, Surface, Theme, TRAFFIC_LIGHT_POSITION,
};
use crate::ui::transcript;
use crate::ui::widgets;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    // Before any thread or window: a panic that would end as a bare SIGABRT leaves its
    // message and backtrace in ~/Library/Logs/Arbos/panic.log (reported on the next launch).
    panic_log::install();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let handle = runtime.handle().clone();

    // `arbos://` URLs from the platform. Invite links (D88): `arbos://join/<code>` — what
    // arbos.life/s/<code> hands to the installed app — opens the Join sheet with the code.
    // `arbos://auth/callback?code=…` is the relay's answer when GitHub sign-in ran in the
    // default browser (no window to attach the system sheet to, or "Open in browser").
    // The platform delivers URLs before the app exists (cold start) or any time after; the
    // hook fills in once the state is up.
    type JoinHook = Rc<RefCell<Option<(gpui::AsyncApp, gpui::Entity<AppState>)>>>;
    let join_hook: JoinHook = Rc::new(RefCell::new(None));
    let pending_joins: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let app = gpui_platform::application().with_assets(crate::ui::icons::Assets);
    {
        let (hook, pending) = (join_hook.clone(), pending_joins.clone());
        app.on_open_urls(move |urls| {
            let codes: Vec<String> = urls
                .iter()
                .filter_map(|u| u.strip_prefix("arbos://join/"))
                .map(|c| c.trim_matches('/').to_string())
                .filter(|c| !c.is_empty())
                .collect();
            let callbacks: Vec<String> = urls
                .iter()
                .filter(|u| auth::is_callback_url(u))
                .cloned()
                .collect();
            if codes.is_empty() && callbacks.is_empty() {
                return;
            }
            match hook.borrow().as_ref() {
                Some((async_cx, state)) => {
                    for code in codes {
                        let state = state.clone();
                        async_cx.update(|cx| {
                            state.update(cx, |s, cx| s.open_join_link(code, cx));
                        });
                    }
                    for url in callbacks {
                        let state = state.clone();
                        async_cx.update(|cx| {
                            state.update(cx, |s, cx| s.sign_in_callback(&url, cx));
                        });
                    }
                }
                // A callback before the state exists has no attempt to land in; only the
                // join codes wait.
                None => pending.borrow_mut().extend(codes),
            }
        });
    }
    // The code-card grammars (≈ 15 ms) load off the main thread so the first
    // fenced block in a chat never waits for them.
    std::thread::spawn(arbos_core::highlight::warm);
    app.run(move |cx: &mut App| {
        gpui_tokio::init_from_handle(cx, handle);
        // Matte is the default (D10, D90); the saved Settings choice wins over
        // it, `ARBOS_SURFACE=matte|glass` over both (demos).
        let surface = std::env::var("ARBOS_SURFACE")
            .ok()
            .and_then(|v| Surface::parse(&v))
            .unwrap_or_else(Surface::load);
        // Dark is the default (D90); the saved Settings choice wins over it,
        // `ARBOS_APPEARANCE=system|light|dark` over both (demos, screenshots).
        let pref = std::env::var("ARBOS_APPEARANCE")
            .ok()
            .and_then(|v| AppearancePref::parse(&v))
            .unwrap_or_else(AppearancePref::load);
        cx.set_global(Theme::for_pref(pref, surface, cx));
        cx.set_reduce_motion(motion::system_reduce_motion());
        PulseClock::init(cx);
        bind_keys(cx);
        set_menus(cx);

        let source = app::init_source(cx);
        let state = cx.new(|cx| AppState::new(source, cx));

        // `ARBOS_WINDOW=x,y,w,h` places the window (demos, two instances side by side).
        let bounds = match std::env::var("ARBOS_WINDOW")
            .ok()
            .map(|v| {
                v.split(',')
                    .filter_map(|n| n.trim().parse::<f32>().ok())
                    .collect::<Vec<_>>()
            })
            .as_deref()
        {
            Some([x, y, w, h]) => Bounds::new(point(px(*x), px(*y)), size(px(*w), px(*h))),
            _ => Bounds::centered(None, size(px(1320.0), px(880.0)), cx),
        };
        *join_hook.borrow_mut() = Some((cx.to_async(), state.clone()));
        for code in pending_joins.borrow_mut().drain(..) {
            state.update(cx, |s, cx| s.open_join_link(code, cx));
        }
        let theme = Theme::of(cx).clone();
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(900.0), px(600.0))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Arbos".into()),
                        appears_transparent: cfg!(target_os = "macos"),
                        // Grok Bot's spot (lights centred 22 pt from the top);
                        // the sidebar strip's discs sit 8 pt under that line.
                        traffic_light_position: Some(point(
                            px(TRAFFIC_LIGHT_POSITION),
                            px(TRAFFIC_LIGHT_POSITION),
                        )),
                    }),
                    app_owns_titlebar_drag: cfg!(target_os = "macos"),
                    window_background: theme.window_background_appearance(),
                    window_decorations: Some(WindowDecorations::Server),
                    app_id: Some("sh.arbos.mac".into()),
                    ..Default::default()
                },
                move |window, cx| cx.new(|cx| Shell::new(state.clone(), window, cx)),
            )
            .expect("open main window");
        reapply_window_background(cx);
        // Notification clicks and foreground banners (macOS); no-op elsewhere.
        crate::ui::notify::init();
        let _ = window.update(cx, |_, _, cx| cx.activate(true));
        // Some X servers (Xvfb, no compositor) do not present the first frame
        // until an event arrives; nudge a second frame shortly after launch.
        // Rig: `ARBOS_FULLSCREEN=1` enters native full screen once the window
        // is up (screenshots without a pointer). Full screen is macOS's own:
        // the lights hide with the title bar and return on the top-edge hover.
        let enter_fullscreen = std::env::var("ARBOS_FULLSCREEN").is_ok_and(|v| v == "1");
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(400))
                .await;
            cx.update(|cx| {
                let _ = window.update(cx, |_, window, _| {
                    window.refresh();
                    if enter_fullscreen {
                        window.toggle_fullscreen();
                    }
                });
            });
        })
        .detach();
        cx.on_action(|_: &shell::Quit, cx| cx.quit());
    });
}

/// The menu bar: the app menu, Edit (Undo / Redo / Cut / Copy / Paste — ⌘V
/// reaches the chat through AppKit's `paste:` selector), and Help → Send
/// Feedback… (⌘⌥F; ⌘⇧F is the app-wide search palette since
/// [feedback search-palette-2026-10-24]).
fn set_menus(cx: &mut App) {
    cx.set_menus(vec![
        Menu {
            name: "Arbos".into(),
            items: vec![MenuItem::action("Quit Arbos", shell::Quit)],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", composer::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", composer::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", composer::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", composer::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", composer::Paste, OsAction::Paste),
                MenuItem::separator(),
                MenuItem::os_action("Select All", composer::SelectAll, OsAction::SelectAll),
            ],
            disabled: false,
        },
        Menu {
            name: "Help".into(),
            items: vec![MenuItem::action("Send Feedback…", feedback::SendFeedback)],
            disabled: false,
        },
    ]);
}

fn bind_keys(cx: &mut App) {
    let input = Some("ComposerInput");
    cx.bind_keys([
        KeyBinding::new("enter", composer::Send, input),
        KeyBinding::new("shift-enter", composer::Newline, input),
        KeyBinding::new("tab", composer::AcceptMention, input),
        KeyBinding::new("shift-tab", composer::PrevMention, input),
        KeyBinding::new("escape", composer::Escape, input),
        KeyBinding::new("backspace", composer::Backspace, input),
        KeyBinding::new("delete", composer::Delete, input),
        KeyBinding::new("alt-backspace", composer::DeleteWordBackward, input),
        KeyBinding::new("ctrl-backspace", composer::DeleteWordBackward, input),
        KeyBinding::new("alt-delete", composer::DeleteWordForward, input),
        KeyBinding::new("ctrl-delete", composer::DeleteWordForward, input),
        KeyBinding::new("left", composer::Left, input),
        KeyBinding::new("right", composer::Right, input),
        KeyBinding::new("up", composer::Up, input),
        KeyBinding::new("down", composer::Down, input),
        KeyBinding::new("shift-left", composer::SelectLeft, input),
        KeyBinding::new("shift-right", composer::SelectRight, input),
        KeyBinding::new("shift-up", composer::SelectUp, input),
        KeyBinding::new("shift-down", composer::SelectDown, input),
        // Word movement: Option+Left/Right is the macOS default. Control+Left/Right
        // (as reported) is bound too, but a stock Mac routes ^← / ^→ to Mission
        // Control ("Move left/right a space") before any app sees them; it only
        // reaches us when that system shortcut is off. Cmd+Left/Right is line start/end.
        KeyBinding::new("ctrl-left", composer::WordLeft, input),
        KeyBinding::new("ctrl-right", composer::WordRight, input),
        KeyBinding::new("alt-left", composer::WordLeft, input),
        KeyBinding::new("alt-right", composer::WordRight, input),
        KeyBinding::new("ctrl-shift-left", composer::SelectWordLeft, input),
        KeyBinding::new("ctrl-shift-right", composer::SelectWordRight, input),
        KeyBinding::new("alt-shift-left", composer::SelectWordLeft, input),
        KeyBinding::new("alt-shift-right", composer::SelectWordRight, input),
        KeyBinding::new("home", composer::Home, input),
        KeyBinding::new("end", composer::End, input),
        KeyBinding::new("shift-home", composer::SelectHome, input),
        KeyBinding::new("shift-end", composer::SelectEnd, input),
        KeyBinding::new("cmd-left", composer::Home, input),
        KeyBinding::new("cmd-right", composer::End, input),
        KeyBinding::new("cmd-shift-left", composer::SelectHome, input),
        KeyBinding::new("cmd-shift-right", composer::SelectEnd, input),
        KeyBinding::new("cmd-a", composer::SelectAll, input),
        KeyBinding::new("ctrl-a", composer::SelectAll, input),
        KeyBinding::new("cmd-v", composer::Paste, input),
        KeyBinding::new("ctrl-v", composer::Paste, input),
        KeyBinding::new("cmd-c", composer::Copy, input),
        KeyBinding::new("ctrl-c", composer::Copy, input),
        KeyBinding::new("cmd-x", composer::Cut, input),
        KeyBinding::new("ctrl-x", composer::Cut, input),
        KeyBinding::new("cmd-z", composer::Undo, input),
        KeyBinding::new("ctrl-z", composer::Undo, input),
        KeyBinding::new("cmd-shift-z", composer::Redo, input),
        KeyBinding::new("ctrl-shift-z", composer::Redo, input),
        KeyBinding::new("cmd-y", composer::Redo, input),
        KeyBinding::new("ctrl-y", composer::Redo, input),
        KeyBinding::new("enter", widgets::SubmitAnswer, Some("QuestionCard")),
        KeyBinding::new("down", widgets::NextOption, Some("QuestionCard")),
        KeyBinding::new("right", widgets::NextOption, Some("QuestionCard")),
        KeyBinding::new("up", widgets::PrevOption, Some("QuestionCard")),
        KeyBinding::new("left", widgets::PrevOption, Some("QuestionCard")),
        KeyBinding::new("space", widgets::ToggleOption, Some("QuestionCard")),
        // Text selected in a chat bubble (drag / double-click) takes focus; ⌘C copies it.
        KeyBinding::new("cmd-c", transcript::Copy, Some("Transcript")),
        KeyBinding::new("ctrl-c", transcript::Copy, Some("Transcript")),
        KeyBinding::new("cmd-w", shell::CloseTab, None),
        KeyBinding::new("ctrl-w", shell::CloseTab, None),
        KeyBinding::new("cmd-t", shell::NewTab, None),
        // ⌘⇧{ / ⌘⇧}: macOS reports these as cmd-{ / cmd-} with shift
        // cleared for punctuation — same as Zed pane tab switching.
        KeyBinding::new("cmd-{", shell::PrevTab, None),
        KeyBinding::new("cmd-}", shell::NextTab, None),
        KeyBinding::new("ctrl-{", shell::PrevTab, None),
        KeyBinding::new("ctrl-}", shell::NextTab, None),
        KeyBinding::new("ctrl-t", shell::NewTab, None),
        KeyBinding::new("cmd-b", shell::ToggleRail, None),
        KeyBinding::new("ctrl-b", shell::ToggleRail, None),
        // App-wide search (Jacob 2026-10-24): ⌘⇧F opens the command palette,
        // ⌘K is its alias (free before this).
        KeyBinding::new("cmd-shift-f", shell::OpenPalette, None),
        KeyBinding::new("ctrl-shift-f", shell::OpenPalette, None),
        KeyBinding::new("cmd-k", shell::OpenPalette, None),
        KeyBinding::new("ctrl-k", shell::OpenPalette, None),
        // In-chat search (Jacob 2026-10-26, [feedback search-in-chat-mac-2026-10-26]):
        // ⌘F opens the Search-in-chat bar (refocuses it when up); ⌘G / ⇧⌘G
        // step through the hits from anywhere; in the bar's field Enter /
        // ⇧Enter step, ⇥ pins the offered sender, Esc closes (the shell's
        // Escape). The field's own Send / Newline / AcceptMention bindings
        // are overridden inside the bar by the deeper "ChatSearch" context.
        KeyBinding::new("cmd-f", chat_search_ui::Open, None),
        KeyBinding::new("ctrl-f", chat_search_ui::Open, None),
        KeyBinding::new("cmd-g", chat_search_ui::Next, None),
        KeyBinding::new("cmd-shift-g", chat_search_ui::Prev, None),
        KeyBinding::new(
            "enter",
            chat_search_ui::Next,
            Some("ChatSearch > ComposerInput"),
        ),
        KeyBinding::new(
            "shift-enter",
            chat_search_ui::Prev,
            Some("ChatSearch > ComposerInput"),
        ),
        KeyBinding::new(
            "tab",
            chat_search_ui::PinSender,
            Some("ChatSearch > ComposerInput"),
        ),
        // Palette open: ⌘1–⌘9 open that row (↑ ↓ ⏎ Esc are the field's and the shell's).
        KeyBinding::new("cmd-1", palette_ui::Pick1, Some("Palette")),
        KeyBinding::new("cmd-2", palette_ui::Pick2, Some("Palette")),
        KeyBinding::new("cmd-3", palette_ui::Pick3, Some("Palette")),
        KeyBinding::new("cmd-4", palette_ui::Pick4, Some("Palette")),
        KeyBinding::new("cmd-5", palette_ui::Pick5, Some("Palette")),
        KeyBinding::new("cmd-6", palette_ui::Pick6, Some("Palette")),
        KeyBinding::new("cmd-7", palette_ui::Pick7, Some("Palette")),
        KeyBinding::new("cmd-8", palette_ui::Pick8, Some("Palette")),
        KeyBinding::new("cmd-9", palette_ui::Pick9, Some("Palette")),
        KeyBinding::new("escape", shell::Escape, None),
        // Media viewer: ← → page through the album (Esc closes via the shell).
        KeyBinding::new("left", media_viewer::Prev, Some("MediaViewer")),
        KeyBinding::new("right", media_viewer::Next, Some("MediaViewer")),
        KeyBinding::new("cmd-q", shell::Quit, None),
        // Send Feedback moved off ⌘⇧F when the palette took it (2026-10-24).
        KeyBinding::new("cmd-alt-f", feedback::SendFeedback, None),
        KeyBinding::new("ctrl-alt-f", feedback::SendFeedback, None),
        // Sheet open: Enter in the note (ComposerInput → on_submit) or ⌘⏎ / Ctrl⏎ anywhere in it.
        KeyBinding::new("cmd-enter", feedback::SubmitFeedback, Some("FeedbackSheet")),
        KeyBinding::new(
            "ctrl-enter",
            feedback::SubmitFeedback,
            Some("FeedbackSheet"),
        ),
    ]);
}

#[cfg(test)]
mod keymap_tests {
    //! Jacob's shortcut rules (search-palette-2026-10-24, 13:49 UTC): ⌘⇧F =
    //! app-wide search (the palette), ⌘K its alias, Send Feedback on ⌘⌥F —
    //! and ⌘F, reserved then, is in-chat search since
    //! [feedback search-in-chat-mac-2026-10-26] (Jacob 2026-10-26).
    use gpui::{Keystroke, TestAppContext};

    use super::*;

    fn actions_for(cx: &mut TestAppContext, keys: &str) -> Vec<String> {
        cx.update(|cx| {
            bind_keys(cx);
            let stroke = Keystroke::parse(keys).expect("keystroke");
            let mut names: Vec<String> = cx
                .all_bindings_for_input(&[stroke])
                .iter()
                .map(|b| b.action().name().to_string())
                .collect();
            // `bind_keys` runs once per lookup here; the table itself has each
            // binding once.
            names.dedup();
            names
        })
    }

    #[gpui::test]
    fn cmd_shift_f_and_cmd_k_open_the_palette(cx: &mut TestAppContext) {
        assert_eq!(actions_for(cx, "cmd-shift-f"), vec!["shell::OpenPalette"]);
        assert_eq!(actions_for(cx, "cmd-k"), vec!["shell::OpenPalette"]);
    }

    #[gpui::test]
    fn cmd_f_opens_in_chat_search_and_cmd_g_steps_through_it(cx: &mut TestAppContext) {
        assert_eq!(actions_for(cx, "cmd-f"), vec!["chat_search::Open"]);
        assert_eq!(actions_for(cx, "cmd-g"), vec!["chat_search::Next"]);
        assert_eq!(actions_for(cx, "cmd-shift-g"), vec!["chat_search::Prev"]);
        // Not the palette, not the old sidebar FocusSearch.
        assert!(!actions_for(cx, "cmd-f").contains(&"shell::OpenPalette".to_string()));
    }

    /// Inside the bar's field the deeper "ChatSearch" context takes Enter,
    /// ⇧Enter and ⇥ from the composer's Send / Newline / AcceptMention.
    #[gpui::test]
    fn enter_shift_enter_and_tab_in_the_bar_field_step_and_pin(cx: &mut TestAppContext) {
        cx.update(|cx| {
            bind_keys(cx);
            let mut outer = gpui::KeyContext::default();
            outer.add("ChatSearch");
            let mut inner = gpui::KeyContext::default();
            inner.add("ComposerInput");
            let stack = vec![outer, inner];
            let keymap = cx.key_bindings();
            let first = |keys: &str, stack: &[gpui::KeyContext]| {
                let stroke = Keystroke::parse(keys).expect("keystroke");
                keymap
                    .borrow()
                    .bindings_for_input(&[stroke], stack)
                    .0
                    .first()
                    .map(|b| b.action().name().to_string())
            };
            assert_eq!(first("enter", &stack).as_deref(), Some("chat_search::Next"));
            assert_eq!(
                first("shift-enter", &stack).as_deref(),
                Some("chat_search::Prev")
            );
            assert_eq!(
                first("tab", &stack).as_deref(),
                Some("chat_search::PinSender")
            );
            // The plain composer keeps its keys.
            let mut plain = gpui::KeyContext::default();
            plain.add("ComposerInput");
            assert_eq!(
                first("enter", &[plain.clone()]).as_deref(),
                Some("composer::Send")
            );
            assert_eq!(
                first("shift-enter", &[plain]).as_deref(),
                Some("composer::Newline")
            );
        });
    }

    #[gpui::test]
    fn send_feedback_moved_to_cmd_alt_f(cx: &mut TestAppContext) {
        assert_eq!(actions_for(cx, "cmd-alt-f"), vec!["feedback::SendFeedback"]);
    }

    #[gpui::test]
    fn cmd_digits_pick_palette_rows(cx: &mut TestAppContext) {
        for n in 1..=9 {
            assert_eq!(
                actions_for(cx, &format!("cmd-{n}")),
                vec![format!("palette::Pick{n}")],
                "⌘{n}"
            );
        }
    }
}
