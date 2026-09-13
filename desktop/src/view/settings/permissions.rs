//! Settings › Permissions: one row per thing the system must allow, with
//! what it is for, where it stands, and a button that raises the real
//! prompt — or opens the pane when the system will not ask again. Shown on
//! first launch too. The rows re-read their status every second while the
//! section is up, so a grant made in System Settings shows without a
//! restart.

use crate::{
    permissions::{Permission, Requested, Status},
    view::settings::{self, SettingsWindow},
    voice_ws,
};
use bezel::{
    gpui::{AnyElement, Context, Hsla, SharedString, div, prelude::*, px},
    motion::Painter,
    theme::{TextStyle, Theme, Typeset},
    ui::widgets::{ButtonStyle, Buttons, Content, Scaffolding},
};
use std::time::Duration;

/// How often the rows re-read the system while the section shows.
const RECHECK: Duration = Duration::from_secs(1);

/// The level bar's width and the frame rate it moves at while the mic
/// test runs.
const LEVEL_WIDTH: f32 = 120.;
const LEVEL_FPS: f32 = 20.;

impl SettingsWindow {
    pub(super) fn permissions_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        self.keep_rechecking(cx);
        let project = self
            .workspace
            .read(cx)
            .active_project()
            .filter(|project| !project.is_remote())
            .map(|project| project.path.clone());
        let rows: Vec<(Permission, Status)> = Permission::applicable()
            .iter()
            .map(|permission| (*permission, permission.status(project.as_deref())))
            .collect();
        let mut group = theme.group_box();
        for (n, (permission, status)) in rows.iter().enumerate() {
            group = group.child(self.permission_row(n, *permission, status, &theme, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(settings::GROUP_GAP))
            .child(group)
            .child(self.mic_test_group(&theme, cx))
            .child(
                div()
                    .text_style(TextStyle::Subheadline)
                    .text_color(theme.text_faint)
                    .child(if cfg!(target_os = "macos") {
                        "Grants are filed under the bundle life.arbos.desktop. A build run from the source tree asks under its own path and does not keep them."
                    } else {
                        "On Linux the desktop portal asks for the screen on first use; the microphone needs a capture program and a device."
                    }),
            )
            .into_any_element()
    }

    /// One row: title and purpose on the left; the status badge and the
    /// button on the right. Granted rows carry no button.
    fn permission_row(
        &self,
        n: usize,
        permission: Permission,
        status: &Status,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (label, tint): (String, Hsla) = match status {
            Status::Granted => ("Granted".into(), theme.success),
            Status::NotAsked => ("Not asked".into(), theme.text_muted),
            Status::Denied => ("Denied".into(), theme.danger),
            Status::Unavailable(_) => ("Unavailable".into(), theme.text_faint),
        };
        let note = match status {
            Status::Unavailable(why) => Some(why.clone()),
            Status::Denied => permission
                .settings_url()
                .map(|_| "The system will not ask again; the switch is in System Settings.".to_string()),
            Status::Granted | Status::NotAsked => None,
        };
        let button_label = match status {
            Status::Granted => None,
            Status::Denied => permission.settings_url().map(|_| "Open System Settings…"),
            Status::NotAsked => Some("Request"),
            Status::Unavailable(_) => permission.settings_url().map(|_| "Open System Settings…"),
        };
        theme
            .card_row(n == 0)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(theme.row_title(permission.title()))
                    .child(
                        div()
                            .mt(px(4.))
                            .text_style(TextStyle::Subheadline)
                            .text_color(theme.text_muted)
                            .child(permission.purpose()),
                    )
                    .children(note.map(|note| {
                        div()
                            .mt(px(2.))
                            .text_style(TextStyle::Caption)
                            .text_color(theme.text_faint)
                            .child(SharedString::from(note))
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .id(("permission-status", n))
                            .text_color(tint)
                            .child(theme.badge(label)),
                    )
                    .children(button_label.map(|label| {
                        theme
                            .button(label, ButtonStyle::Prominent, None)
                            .id(("permission-request", n))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.request(permission, cx);
                            }))
                    })),
            )
            .into_any_element()
    }

    /// Raise the prompt, or open the pane; the next re-check shows the result.
    fn request(&mut self, permission: Permission, cx: &mut Context<Self>) {
        let project = self
            .workspace
            .read(cx)
            .active_project()
            .filter(|project| !project.is_remote())
            .map(|project| project.path.clone());
        let mut opened: Option<String> = None;
        let outcome = permission.request(project.as_deref(), &mut |url| opened = Some(url.to_owned()));
        if let (Requested::OpenedSettings, Some(url)) = (&outcome, opened) {
            cx.open_url(&url);
        }
        cx.notify();
    }

    /// While this section shows, re-read every second: a grant made in
    /// System Settings, or a prompt answered, lands without a restart.
    fn keep_rechecking(&mut self, cx: &mut Context<Self>) {
        if self.rechecking {
            return;
        }
        self.rechecking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(RECHECK).await;
                let live = this.update(cx, |this, cx| {
                    let on = this.section == settings::Section::Permissions;
                    if on {
                        cx.notify();
                    } else {
                        this.rechecking = false;
                    }
                    on
                });
                if !matches!(live, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// The microphone, heard: a level bar while a test take runs, through
    /// the same capture the composer's mic and hold-Fn dictation use once
    /// they share the in-process path. Until then the test opens a speech
    /// session, so it needs the speech server configured.
    fn mic_test_group(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let status = voice_ws::status();
        let live = status.phase.is_some_and(|phase| phase != voice_ws::Phase::Off);
        if live {
            Painter::of(cx).lease(LEVEL_FPS, Duration::from_millis(300), cx);
        }
        let configured = voice_ws::configured();
        let detail: String = match (&status.mic_error, live) {
            (Some(err), _) => err.clone(),
            (None, true) if !status.mic_device.is_empty() => format!("Listening on {}.", status.mic_device),
            (None, true) => "Listening.".into(),
            (None, false) if !configured => {
                "Needs the speech server: set voice_url in the kernel's config.".into()
            }
            (None, false) => "Press Test and say something; the bar shows what the mic hears.".into(),
        };
        let level = status.level.clamp(0., 1.);
        theme
            .group_box()
            .child(
                theme
                    .card_row(true)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(theme.row_title("Test the microphone"))
                            .child(
                                div()
                                    .mt(px(4.))
                                    .text_style(TextStyle::Subheadline)
                                    .text_color(if status.mic_error.is_some() {
                                        theme.danger
                                    } else {
                                        theme.text_muted
                                    })
                                    .child(SharedString::from(detail)),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(12.))
                            .child(
                                div()
                                    .id("mic-level")
                                    .w(px(LEVEL_WIDTH))
                                    .h(px(6.))
                                    .rounded_full()
                                    .bg(theme.element_hover)
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded_full()
                                            .w(px(LEVEL_WIDTH * level))
                                            .bg(if live { theme.success } else { theme.text_faint }),
                                    ),
                            )
                            .child(
                                theme
                                    .button(
                                        if live { "Stop" } else { "Test" },
                                        if live {
                                            ButtonStyle::Ghost
                                        } else {
                                            ButtonStyle::Prominent
                                        },
                                        None,
                                    )
                                    .id("mic-test")
                                    .when(!configured && !live, |el| el.opacity(0.5))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        if live {
                                            let _ = voice_ws::stop();
                                        } else if configured {
                                            let _ = voice_ws::start();
                                        }
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
