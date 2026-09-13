//! The general section: what this copy of the app is.

use crate::{assets, view::settings::SettingsWindow, voice_ws};
use bezel::{
    gpui::{AnyElement, Context, div, img, prelude::*, px},
    theme::{TextStyle, Theme, Typeset, ink},
    ui::widgets::{Content, Controls, Scaffolding},
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// What this build is, read at compile time from `Cargo.toml` — the same
/// string the bundle carries, since the Makefile stamps `CFBundleVersion` out
/// of that file too. Nothing here can drift from what was shipped.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The commit it was built from — see `build.rs`, which is the only place that
/// can know: the app that ships has no repository to ask.
const COMMIT: &str = env!("ARBOS_COMMIT");

/// The mark over the rows. An About panel's measure — big enough to be the
/// picture of the app, small enough that the two lines under it are still what
/// the section is.
const MARK: f32 = 72.;

impl SettingsWindow {
    pub(super) fn general_body(&self, cx: &Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        div()
            .flex()
            .flex_col()
            .gap(px(super::GROUP_GAP))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.))
                    .children(assets::mark().map(|path| img(path).size(px(MARK))))
                    .child(
                        div()
                            .text_style(TextStyle::Title)
                            .text_color(theme.text)
                            .child("Arbos"),
                    )
                    .child(
                        div()
                            .text_style(TextStyle::Subheadline)
                            .text_color(theme.text_muted)
                            .child("A chat with the agent that lives here."),
                    ),
            )
            .child(
                theme
                    .group_box()
                    .child(
                        theme
                            .card_row(true)
                            .child(div().flex_1().min_w_0().child(theme.row_title("Version")))
                            .child(theme.badge(VERSION)),
                    )
                    .child(
                        theme
                            .card_row(false)
                            .child(div().flex_1().min_w_0().child(theme.row_title("Commit")))
                            .child(match commit_url() {
                                Some(url) => div()
                                    .id("commit")
                                    .cursor_pointer()
                                    .hover(|el| el.text_color(theme.accent))
                                    .child(theme.badge(COMMIT))
                                    .on_click(move |_, _, cx| cx.open_url(&url))
                                    .into_any_element(),
                                None => theme.badge(COMMIT).into_any_element(),
                            }),
                    ),
            )
            .child(self.mic_row(cx))
            .into_any_element()
    }

    /// Settings › General › Test mic: the same capture a take or a call
    /// opens, shown as a live level so "does it hear me" takes five seconds
    /// to answer. The first run is also when macOS asks for the microphone.
    fn mic_row(&self, cx: &Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let test = voice_ws::mic_test();
        let on = test.is_some();
        if on && !MIC_TICK_PENDING.swap(true, Ordering::SeqCst) {
            // The level moves on its own; redraw while the probe runs.
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(MIC_TICK).await;
                MIC_TICK_PENDING.store(false, Ordering::SeqCst);
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        let note: Option<String> = match &test {
            Some(t) => match &t.error {
                Some(e) => Some(format!("not hearing: {e}")),
                None if t.device.is_empty() => Some("opening the microphone…".into()),
                None => Some(format!("{} · {}%", t.device, (t.level * 100.0).round() as u32)),
            },
            None => None,
        };
        let level = test.as_ref().map(|t| t.level).unwrap_or(0.0);
        let bar_w = 120.0f32;
        theme
            .group_box()
            .child(
                theme
                    .card_row(true)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(theme.row_title("Test mic"))
                            .children(note.map(|n| {
                                div()
                                    .text_style(TextStyle::Caption)
                                    .text_color(theme.text_muted)
                                    .child(n)
                            })),
                    )
                    .when(on, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .w(px(bar_w))
                                .h(px(6.))
                                .rounded_full()
                                .bg(ink(0.15))
                                .child(
                                    div()
                                        .h_full()
                                        .rounded_full()
                                        .w(px(bar_w * level.clamp(0.0, 1.0)))
                                        .bg(if level > 0.02 { theme.success } else { theme.text_muted }),
                                ),
                        )
                    })
                    .child(
                        div()
                            .id("test-mic")
                            .cursor_pointer()
                            .child(theme.toggle(on))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if on {
                                    voice_ws::mic_test_stop();
                                } else {
                                    voice_ws::mic_test_start();
                                }
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
}

/// How often the Test mic row redraws while a probe runs.
const MIC_TICK: Duration = Duration::from_millis(100);
static MIC_TICK_PENDING: AtomicBool = AtomicBool::new(false);

/// The commit on the page that hosts it, where there is one to open. A build
/// that names no commit links nowhere; one built over an edited tree still
/// links at the commit underneath, which is the last thing anybody else can
/// fetch.
fn commit_url() -> Option<String> {
    let sha = COMMIT.split('-').next()?;
    (sha != "unknown").then(|| format!("{}/commit/{sha}", env!("CARGO_PKG_REPOSITORY")))
}
