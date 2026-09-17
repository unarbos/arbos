//! The general section: what this copy of the app is.

use crate::{
    assets, build,
    kernel::{self, KernelBuild},
    model::workspace::Workspace,
    update,
    update::Updates,
    view::{settings::SettingsPane, status_bar},
};
use arbos_update::Channel;
use bezel::{
    gpui::{AnyElement, Context, SharedString, div, img, prelude::*, px},
    motion::{Fade, Painter},
    theme::{TextStyle, Theme, Typeset},
    ui::widgets::{ButtonStyle, Buttons, Content, Scaffolding},
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

impl SettingsPane {
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
            .child(self.machine_group(cx))
            .into_any_element()
    }

    /// What this copy of the app is talking to, and where its files are.
    ///
    /// The kernel rows are read off the **connection** — what a kernel said
    /// about itself in its `hello` frame. The binary the app would launch is a
    /// different fact and sits on a row of its own that says so, because the
    /// two part company in exactly the case worth seeing: a kernel from another
    /// build that was already running when the app attached. Every row answers
    /// present, absent or unknown and says which, because Jacob's window read
    /// `kernel ? ? · ?/?` for three hours while a tab was dead, and a question
    /// mark is not one of those three answers.
    fn machine_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let workspace = self.workspace.read(cx);
        let front = workspace.active_project();
        // Every *other* open place. The row above is the kernel being talked
        // to; these are the rest, so nothing is said twice.
        let others: Vec<AnyElement> = workspace
            .projects
            .iter()
            .enumerate()
            .filter(|(ix, _)| Some(*ix) != workspace.active)
            .map(|(_, project)| {
                serving_row(
                    &Workspace::tab_label(project),
                    project.kernel_build(),
                    &theme,
                )
            })
            .collect();
        let mut group = theme.group_box().child(match front {
            Some(project) => kernel_row(
                &Workspace::tab_label(project),
                project.kernel_build(),
                &theme,
            ),
            // No place open is *why* nothing is attached, and saying that is
            // kinder than a bare "not attached", which reads like a fault.
            None => row(
                "Kernel",
                Some(theme.badge("nothing attached").into_any_element()),
                vec![Line::say("No place is open, so nothing is attached.")],
                true,
                &theme,
            ),
        });
        for other in others {
            group = group.child(other);
        }
        group = group.child(launcher_row(&theme));
        for (n, (title, what, path)) in [
            (
                "Store",
                "The home place the app opens on launch.",
                Workspace::home_place().map(|place| place.path),
            ),
            (
                "Kernel config",
                "The model and the key every kernel on this machine reads.",
                Some(arbos_core::host_dir().join("config.toml")),
            ),
            (
                "App state",
                "The tabs, the appearance and the window frame this app remembers.",
                crate::model::state::path(),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            group = group.child(path_row(n, title, what, path, &theme, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(super::LABEL_GAP))
            .child(theme.field_label("This machine"))
            .child(group)
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

/// A line under a row's title. `warn` is the row's only styling decision, and
/// it means *something is wrong here*, not *this is interesting*.
struct Line {
    text: String,
    warn: bool,
}

impl Line {
    fn say(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            warn: false,
        }
    }

    fn warn(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            warn: true,
        }
    }
}

/// One row: a title with its lines under it, and whatever goes on the right.
fn row(
    title: &str,
    value: Option<AnyElement>,
    lines: Vec<Line>,
    first: bool,
    theme: &Theme,
) -> AnyElement {
    theme
        .card_row(first)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(theme.row_title(title.to_string()))
                .children(lines.into_iter().map(|line| {
                    div()
                        .mt(px(2.))
                        .text_style(TextStyle::Caption)
                        .text_color(if line.warn {
                            theme.warning
                        } else {
                            theme.text_muted
                        })
                        .child(line.text)
                })),
        )
        .children(value)
        .into_any_element()
}

/// The version to show, or the fact that the kernel did not give one. `hello`
/// always carries it, so this is the shape of the answer rather than a case
/// anybody expects to see.
fn version_badge(build: &KernelBuild, theme: &Theme) -> AnyElement {
    let version = if build.version.trim().is_empty() {
        "version not said".to_string()
    } else {
        build.version.clone()
    };
    match build.commit() {
        Some(sha) => theme.badge(format!("{version} · {sha}")),
        None => theme.badge(version),
    }
    .into_any_element()
}

/// What a kernel says about the commit it was built from, in words.
///
/// A build that recorded none reports the string `unknown`, and printing that
/// beside a version reads like a value — `kernel 0.2.0 unknown` looks like a
/// build called unknown. So it is said as a sentence instead, and the comparison
/// that needs a commit is skipped rather than guessed.
fn commit_lines(build: &KernelBuild) -> Vec<Line> {
    let mut lines = Vec::new();
    if !build.built_at.is_empty() {
        lines.push(Line::say(format!("Built {}.", build.built_at)));
    }
    let Some(running) = build.commit() else {
        lines.push(Line::say(
            "This build did not record its commit, so it cannot be compared with the kernel binary below.",
        ));
        return lines;
    };
    match kernel::bundled_commit() {
        kernel::Bundled::Sha(bundled) => {
            // Said against the binary below rather than "the build this app
            // ships": in a source tree the app and the kernel beside it are
            // often built minutes apart, and this row's claim is only ever
            // about the binary a restart would run.
            if arbos_update::kernel::same_commit(running, bundled) {
                lines.push(Line::say(
                    "The same build as the kernel binary below, which is what a restart would run.",
                ));
            } else {
                lines.push(Line::warn(
                    "A different build from the kernel binary below. Restarting it runs that one instead.",
                ));
            }
        }
        // Not "unknown" as a value: which of the two sides is unread is the
        // difference between waiting a moment and having no kernel to compare
        // against at all.
        kernel::Bundled::Unread => lines.push(Line::say(
            "The app has not read its own kernel binary yet, so the two are not compared.",
        )),
        kernel::Bundled::Unreadable => lines.push(Line::say(
            "This app cannot read the kernel binary it ships, so the two cannot be compared.",
        )),
    }
    lines
}

/// The kernel's own file being gone is the kernel's own report (`binary_gone`
/// in `hello`), not something read off the disk here and inferred.
fn replaced_line(build: &KernelBuild) -> Option<Line> {
    build.binary_gone.then(|| {
        Line::warn(
            "Its own file was replaced or moved under it, so it is running an older image than the one on disk. Restarting it runs what is there now.",
        )
    })
}

/// The kernel serving the place in front: the one this window is talking to.
fn kernel_row(place: &str, build: Option<&KernelBuild>, theme: &Theme) -> AnyElement {
    let Some(build) = build else {
        return row(
            "Kernel",
            Some(theme.badge("not attached").into_any_element()),
            vec![Line::say(format!(
                "Nothing is attached for {place}. Type in its chat and the app starts one."
            ))],
            true,
            theme,
        );
    };
    let mut lines = vec![Line::say(format!("Serving {place}."))];
    lines.extend(commit_lines(build));
    lines.extend(replaced_line(build));
    row(
        "Kernel",
        Some(version_badge(build, theme)),
        lines,
        true,
        theme,
    )
}

/// Another open place and the kernel serving it, in one line each.
fn serving_row(place: &str, build: Option<&KernelBuild>, theme: &Theme) -> AnyElement {
    let Some(build) = build else {
        return row(
            place,
            Some(theme.badge("not attached").into_any_element()),
            vec![Line::say("Nothing is attached for this place.")],
            false,
            theme,
        );
    };
    let mut lines = commit_lines(build);
    lines.extend(replaced_line(build));
    row(
        place,
        Some(version_badge(build, theme)),
        lines,
        false,
        theme,
    )
}

/// The binary a place with no kernel is started from.
///
/// Its own row, and worded so it cannot be read as the answer to the rows
/// above: this is what the app would launch, and a kernel that was already
/// running when the app attached came from somewhere else. `hello` carries no
/// path, so there is nothing here read off the connection to show.
fn launcher_row(theme: &Theme) -> AnyElement {
    let mut lines = vec![Line::say(
        "What a place with no kernel running is started from — not necessarily what is serving the places above.",
    )];
    match kernel::arbos_bin() {
        Ok(path) if path.is_absolute() => lines.push(Line::say(path.display().to_string())),
        // The last fallback is the bare name, which is whatever `PATH` finds at
        // the moment a kernel is started rather than a file this row can name.
        Ok(path) => lines.push(Line::say(format!(
            "{} — whatever PATH finds when a place opens.",
            path.display()
        ))),
        Err(_) => lines.push(Line::warn(
            "No arbos-kernel on this machine, so a place with no kernel cannot be started.",
        )),
    }
    row("Kernel binary", None, lines, false, theme)
}

/// A file or folder this app reads and writes, and a way to open it.
fn path_row(
    n: usize,
    title: &'static str,
    what: &'static str,
    path: Option<std::path::PathBuf>,
    theme: &Theme,
    cx: &mut Context<SettingsPane>,
) -> AnyElement {
    let painter = Painter::of(cx);
    let Some(path) = path else {
        return row(
            title,
            None,
            vec![
                Line::say(what),
                // A path this machine has nowhere to put is why the thing is
                // missing, and it is a different fault from an empty file.
                Line::warn("This machine has nowhere to keep it, so nothing is written."),
            ],
            false,
            theme,
        );
    };
    let shown = path.display().to_string();
    // Whether it is there yet: an app that has never saved has no state file,
    // and a row that showed the path alone would read as if it had.
    let exists = path.exists();
    let mut lines = vec![Line::say(what), Line::say(shown)];
    if !exists {
        lines.push(Line::say("Not written yet."));
    }
    row(
        title,
        Some(
            theme
                .button(
                    "Reveal",
                    ButtonStyle::Ghost,
                    Some(Fade::new(painter, format!("reveal-{n}"))),
                )
                .id(("reveal", n))
                .when(!exists, |el| el.opacity(0.5))
                .on_click(move |_, _, cx| cx.reveal_path(&path))
                .into_any_element(),
        ),
        lines,
        false,
        theme,
    )
}

/// The commit on the page that hosts it, where there is one to open. A build
/// that names no commit links nowhere; one built over an edited tree still
/// links at the commit underneath, which is the last thing anybody else can
/// fetch.
fn commit_url() -> Option<String> {
    let sha = COMMIT.split('-').next()?;
    (sha != "unknown").then(|| format!("{}/commit/{sha}", env!("CARGO_PKG_REPOSITORY")))
}
