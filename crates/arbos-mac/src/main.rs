//! Arbos desktop client. GPUI on the zui fork; tokio for the sync sources.

mod app;
mod audio;
mod auth;
mod calls;
mod model;
mod onboarding;
mod sync;
mod ui;
mod update;

use gpui::{
    point, px, size, App, AppContext as _, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions,
    WindowBounds, WindowDecorations, WindowOptions,
};

use crate::app::AppState;
use crate::ui::composer;
use crate::ui::feedback;
use crate::ui::motion::{self, PulseClock};
use crate::ui::shell::{self, Shell};
use crate::ui::theme::{reapply_window_background, AppearancePref, Surface, Theme};
use crate::ui::widgets;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let handle = runtime.handle().clone();

    gpui_platform::application()
        .with_assets(crate::ui::icons::Assets)
        .run(move |cx: &mut App| {
            gpui_tokio::init_from_handle(cx, handle);
            // Matte is the default (D10); the saved Settings choice wins over it,
            // `ARBOS_SURFACE=matte|glass` over both (demos).
            let surface = std::env::var("ARBOS_SURFACE")
                .ok()
                .and_then(|v| Surface::parse(&v))
                .unwrap_or_else(Surface::load);
            // System is the default (D81); the saved Settings choice wins over it,
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
            let theme = Theme::of(cx).clone();
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        window_min_size: Some(size(px(900.0), px(600.0))),
                        titlebar: Some(TitlebarOptions {
                            title: Some("Arbos".into()),
                            appears_transparent: cfg!(target_os = "macos"),
                            traffic_light_position: Some(point(px(14.0), px(14.0))),
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
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(400))
                    .await;
                cx.update(|cx| {
                    let _ = window.update(cx, |_, window, _| window.refresh());
                });
            })
            .detach();
            cx.on_action(|_: &shell::Quit, cx| cx.quit());
        });
}

/// The menu bar: the app menu and Help → Send Feedback… (⌘⇧F), the Mac twin of
/// TestFlight's screenshot feedback.
fn set_menus(cx: &mut App) {
    cx.set_menus(vec![
        Menu {
            name: "Arbos".into(),
            items: vec![MenuItem::action("Quit Arbos", shell::Quit)],
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
        // Word movement: Control+Left/Right (reported) and Option+Left/Right (macOS).
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
        KeyBinding::new("enter", widgets::SubmitAnswer, Some("QuestionCard")),
        KeyBinding::new("down", widgets::NextOption, Some("QuestionCard")),
        KeyBinding::new("right", widgets::NextOption, Some("QuestionCard")),
        KeyBinding::new("up", widgets::PrevOption, Some("QuestionCard")),
        KeyBinding::new("left", widgets::PrevOption, Some("QuestionCard")),
        KeyBinding::new("space", widgets::ToggleOption, Some("QuestionCard")),
        KeyBinding::new("cmd-w", shell::CloseTab, None),
        KeyBinding::new("ctrl-w", shell::CloseTab, None),
        KeyBinding::new("cmd-t", shell::NewTab, None),
        KeyBinding::new("ctrl-t", shell::NewTab, None),
        KeyBinding::new("cmd-b", shell::ToggleRail, None),
        KeyBinding::new("ctrl-b", shell::ToggleRail, None),
        KeyBinding::new("escape", shell::Escape, None),
        KeyBinding::new("cmd-q", shell::Quit, None),
        KeyBinding::new("cmd-shift-f", feedback::SendFeedback, None),
        KeyBinding::new("ctrl-shift-f", feedback::SendFeedback, None),
        // Sheet open: Enter in the note (ComposerInput → on_submit) or ⌘⏎ / Ctrl⏎ anywhere in it.
        KeyBinding::new("cmd-enter", feedback::SubmitFeedback, Some("FeedbackSheet")),
        KeyBinding::new(
            "ctrl-enter",
            feedback::SubmitFeedback,
            Some("FeedbackSheet"),
        ),
    ]);
}
