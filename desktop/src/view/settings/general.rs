//! The general section: what this copy of the app is.

use crate::{
    assets, build, update,
    update::Updates,
    view::{settings::SettingsWindow, status_bar},
};
use arbos_update::Channel;
use bezel::{
    gpui::{AnyElement, Context, SharedString, div, img, prelude::*, px},
    theme::{TextStyle, Theme, Typeset},
    ui::widgets::{Content, Scaffolding},
};

/// What this build is, read at compile time from `Cargo.toml` — the same
/// string the bundle carries, since the Makefile stamps `CFBundleVersion` out
/// of that file too. Nothing here can drift from what was shipped.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The commit it was built from — see `build.rs`, which is the only place that
/// can know: the app that ships has no repository to ask.
const COMMIT: &str = build::COMMIT;

/// The mark over the rows. An About panel's measure — big enough to be the
/// picture of the app, small enough that the two lines under it are still what
/// the section is.
const MARK: f32 = 72.;

impl SettingsWindow {
    pub(super) fn general_body(&self, cx: &mut Context<Self>) -> AnyElement {
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
                    // The one line to read out when asked "which build": the
                    // same badge the settings gear's tooltip carries.
                    .child(
                        theme
                            .card_row(false)
                            .child(div().flex_1().min_w_0().child(theme.row_title("Build")))
                            .child(theme.badge(build::badge())),
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
            .child(self.updates_group(cx))
            .into_any_element()
    }

    /// `Checked 4m ago`, or that nobody has looked yet.
    fn update_last_checked(&self, cx: &Context<Self>) -> String {
        let updates = cx.global::<Updates>().0.read(cx);
        status_bar::last_checked(updates.checked())
    }

    /// Why the last check failed, when it did. The bar says *that* it failed;
    /// this is the only place that says why without a pointer resting on a
    /// control.
    fn update_failure(&self, cx: &Context<Self>) -> Option<String> {
        cx.global::<Updates>()
            .0
            .read(cx)
            .checked()
            .and_then(|checked| checked.failed.clone())
    }

    /// Which builds this machine follows, and a way to look now.
    ///
    /// The control that acts on this lives in the bar along the bottom of the
    /// window; this is where the choice behind it is made, because a channel
    /// is a decision taken once and the button is pressed often.
    fn updates_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let chosen = update::channel_of(&self.workspace.read(cx).settings);
        theme
            .group_box()
            .child(
                theme
                    .card_row(true)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(theme.row_title("Updates"))
                            .child(
                                div()
                                    .text_style(TextStyle::Caption)
                                    .text_color(theme.text_muted)
                                    .child(chosen.describe()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(2.))
                            .p(px(2.))
                            .rounded(px(Theme::control_radius()))
                            .bg(theme.input_bg)
                            .children(Channel::ALL.into_iter().map(|channel| {
                                let selected = channel == chosen;
                                div()
                                    .id(match channel {
                                        Channel::Stable => "update-channel-stable",
                                        Channel::Dev => "update-channel-dev",
                                    })
                                    .px(px(10.))
                                    .py(px(3.))
                                    .rounded(px(Theme::control_radius() - 1.))
                                    .text_style(TextStyle::Caption)
                                    .when(selected, |el| {
                                        el.bg(theme.surface_raised).text_color(theme.text)
                                    })
                                    .when(!selected, |el| {
                                        el.cursor_pointer()
                                            .text_color(theme.text_muted)
                                            .hover(|el| el.bg(theme.element_hover))
                                    })
                                    .child(match channel {
                                        Channel::Stable => "Stable",
                                        Channel::Dev => "Dev",
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.workspace.update(cx, |workspace, cx| {
                                            workspace.set_update_channel(channel, cx);
                                        });
                                        cx.notify();
                                    }))
                            })),
                    ),
            )
            // Written down where no pointer is needed to read it. The bar's
            // tooltip says the same thing, and a tooltip needs the pointer to
            // rest on the control — which puts it out of reach of anything
            // driving the app, and out of mind for anybody who is not already
            // suspicious that updates have stopped arriving.
            .child(
                theme
                    .card_row(false)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(theme.row_title("Last checked"))
                            .children(self.update_failure(cx).map(|why| {
                                div()
                                    .id("update-last-check-why")
                                    .text_style(TextStyle::Caption)
                                    .text_color(theme.text_muted)
                                    .child(why)
                            })),
                    )
                    .child(
                        div()
                            .id("update-last-checked")
                            .child(theme.badge(self.update_last_checked(cx))),
                    ),
            )
            .child(
                theme
                    .card_row(false)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(theme.row_title("This build"))
                            .child(
                                div()
                                    .text_style(TextStyle::Caption)
                                    .text_color(theme.text_muted)
                                    .child(match update::built_in_key_present() {
                                        true => SharedString::from(
                                            "Updates are checked against Arbos's signing key.",
                                        ),
                                        // A build made before anybody set the
                                        // key up. It can never install an
                                        // update, and saying so here is kinder
                                        // than a button that fails.
                                        false => SharedString::from(
                                            "This build carries no update key, so it cannot \
                                             install an update.",
                                        ),
                                    }),
                            ),
                    )
                    .child(theme.badge(build::version_label())),
            )
            .into_any_element()
    }
}

/// The commit on the page that hosts it, where there is one to open. A build
/// that names no commit links nowhere; one built over an edited tree still
/// links at the commit underneath, which is the last thing anybody else can
/// fetch.
fn commit_url() -> Option<String> {
    let sha = COMMIT.split('-').next()?;
    (sha != "unknown").then(|| format!("{}/commit/{sha}", env!("CARGO_PKG_REPOSITORY")))
}
