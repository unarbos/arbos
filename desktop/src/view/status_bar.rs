//! The bar across the bottom of the window: settings on the left, and beside
//! it what build this is — or, when there is a newer one, a blue Update
//! button.
//!
//! Cursor's is the shape being matched. It sits at the bottom left, it is
//! always there, and it is quiet until it has something to say. Resting, it is
//! the version in faint text and nothing else. When a build is waiting it
//! becomes a filled blue control that reads `Update` — not a tinted pill or a
//! dot on an icon, because the whole point is that it cannot be missed.
//!
//! The four states it is judged on:
//!
//! | state | what it looks like |
//! | --- | --- |
//! | resting, up to date | `0.2.0 (1877)` in faint text; hovering lifts it |
//! | update waiting | a filled blue `⭳ Update`; hovering lightens the plate |
//! | updating | the same plate with the download filling it left to right, reading `Updating… 42%`, then `Installing…`, then `Restarting…` |
//! | failed | the plate goes to the danger colour and reads `Update failed`; the reason is in the tooltip, and a click tries again |
//!
//! Nothing here decides anything. [`crate::update::Updater`] holds the state
//! and does the work; this draws it.

use crate::{
    build,
    update::{Checked, State, Updater},
    view::{root::Arbos, settings::Section},
};
use arbos_update::Channel;
use bezel::{
    gpui::{AnyElement, Context, Entity, FontWeight, Hsla, div, hsla, prelude::*, px},
    theme::{TextStyle, Theme, Typeset},
    ui::{icons, tooltip::Tooltip, widgets::Buttons},
};

/// The strip's height. The tab bar is 36; the bar under the window is meant to
/// read as chrome rather than as another row of content, so it is shorter.
const HEIGHT: f32 = 28.;

/// The inset either end, matching the panel's.
const PAD_X: f32 = 8.;

impl Arbos {
    /// The bar. Always drawn: it is where the version lives, and a control
    /// that appears only when it has news is a control nobody learns.
    pub(crate) fn status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        div()
            .id("status-bar")
            .flex_none()
            .w_full()
            .h(px(HEIGHT))
            .px(px(PAD_X))
            // No tray behind it and no rule over it: the gear and the
            // version float on the chat's surface, legible by their own
            // muted tone and the spacing above (Jacob, 09-16).
            .bg(crate::view::root::chrome_bg(&theme))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .child(self.status_bar_settings(&theme, cx))
            // Before the update control: a kernel from another build is
            // already losing work, where an update merely waiting is not.
            .children(self.status_bar_stranger(&theme, cx))
            .child(self.status_bar_update(&theme, cx))
            .into_any_element()
    }

    /// The gear. The same control the panel's foot carries, in the place
    /// Cursor keeps it, so it is reachable with the panel folded away.
    fn status_bar_settings(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        // Something the user tried needed a permission that is not granted: a
        // dot on the gear, and nothing louder.
        let wants_permission = self.permission_center.read(cx).wants_attention();
        theme
            .ghost("status-bar-settings")
            .px(px(6.))
            .py(px(4.))
            .tooltip(|window, cx| Tooltip::with_keystroke("Settings", "⌘,", window, cx))
            .child(
                div()
                    .relative()
                    .child(
                        icons::icon(icons::system::SETTINGS_MINIMALISTIC)
                            .size(px(13.))
                            .text_color(theme.text_muted),
                    )
                    .when(wants_permission, |el| {
                        el.child(
                            div()
                                .id("status-bar-settings-dot")
                                .absolute()
                                .top(px(-2.))
                                .right(px(-3.))
                                .size(px(6.))
                                .rounded_full()
                                .bg(theme.warning),
                        )
                    }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if wants_permission {
                    this.show_permissions(window, cx);
                } else {
                    this.open_settings(Section::General, cx);
                }
            }))
            .into_any_element()
    }

    /// A kernel serving an open place that is not the build this app ships.
    ///
    /// This comes before the update control, because it is worse news. An
    /// update that is available costs nothing to ignore; a kernel from another
    /// build is *already* failing — on 2026-09-17 one was 223 commits behind,
    /// rejected frames it had never heard of, and lost five workers' reports
    /// and a feedback sheet without a single error reaching the screen.
    ///
    /// The quiet one has already been dealt with: a stranger with nothing
    /// running in it is stopped and replaced at attach. What reaches here is
    /// the one that needs a person, because something is running inside it.
    fn status_bar_stranger(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        // Asked on every frame; the looking happens on a timer inside.
        let places: Vec<_> = self
            .workspace
            .read(cx)
            .projects
            .iter()
            .map(|project| project.place().clone())
            .collect();
        self.updater
            .update(cx, |updater, cx| updater.look_for_strangers(places, cx));
        let found = self.updater.read(cx).strangers().first()?.clone();
        let place = found.place.clone();
        // Capitalised: `say` gives the sentence, this is the start of one.
        let what = found.reason.say(&place.title());
        let tooltip = format!(
            "{}{}.\n\n\
             It will not understand everything this window sends it: work can finish and \n\
             never be reported. {}.\n\n\
             Click to stop and restart it on this build. Anything running in it ends the \n\
             way the stop button ends it.",
            what[..1].to_uppercase(),
            &what[1..],
            found.gate.say(),
        );
        Some(
            plate(
                cx,
                Plate {
                    id: "status-bar-stranger-kernel",
                    label: found.reason.headline().into(),
                    icon: None,
                    fill: theme.warning,
                    progress: None,
                    tooltip: Some(tooltip),
                    action: Action::RestartKernel(place),
                },
            )
            .into_any_element(),
        )
    }

    /// The stranger plate's click: stop the kernel serving `place` and let
    /// the app start one on this build. Off the main thread — the stop
    /// waits on a process. The word on what happened goes on the place's
    /// root chat, where the person is looking, not only in the bar: the
    /// sockets drop and reconnect on their own, and the plate goes when the
    /// next look finds no stranger.
    fn restart_stranger(&mut self, place: crate::model::place::Place, cx: &mut Context<Self>) {
        let title = place.title();
        self.workspace.update(cx, |workspace, cx| {
            workspace.notice_on_root(
                &place,
                false,
                &format!("Restarting the kernel for {title} on this build; anything it was running is being stopped."),
                cx,
            );
        });
        cx.spawn(async move |this, cx| {
            let target = place.clone();
            let outcome = cx
                .background_executor()
                .spawn(async move { crate::kernel::restart_kernel(&target) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.updater
                    .update(cx, |updater, cx| updater.forget_strangers(cx));
                let (failed, text) = match outcome {
                    Ok(()) => (
                        false,
                        format!("Kernel for {title} restarted on this build ({}).", build::badge()),
                    ),
                    Err(err) => (true, format!("Could not restart the kernel for {title}: {err:#}")),
                };
                this.workspace.update(cx, |workspace, cx| {
                    workspace.notice_on_root(&place, failed, &text, cx);
                });
            });
        })
        .detach();
    }

    /// The version, or the button.
    fn status_bar_update(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        // Settings owns the choice and this owns the asking, so the two are
        // reconciled here rather than through a third thing that watches the
        // file. `set_channel` does nothing when it has not moved.
        let chosen = crate::update::channel_of(&self.workspace.read(cx).settings);
        self.updater
            .update(cx, |updater, cx| updater.set_channel(chosen, cx));

        // Taken out of the entity before anything else borrows `cx`.
        let updater = self.updater.read(cx);
        let (state, channel, trouble, at, checked) = (
            updater.state().clone(),
            updater.channel(),
            updater.can_install(),
            updater.installed_at(),
            updater.checked().cloned(),
        );
        match &state {
            State::Idle | State::Checking | State::Unreachable { .. } => quiet(
                theme,
                Resting {
                    state: state.clone(),
                    channel,
                    trouble,
                    at,
                    checked,
                },
                cx,
            ),
            State::Ready(update) => plate(
                cx,
                Plate {
                    id: "status-bar-update",
                    label: "Update".into(),
                    icon: Some(icons::files::DOWNLOAD),
                    fill: theme.accent,
                    progress: None,
                    tooltip: Some(format!(
                        "Update to {}{}",
                        update.version.human(),
                        match update.notes.trim() {
                            "" => String::new(),
                            notes => format!(" — {notes}"),
                        }
                    )),
                    action: Action::Install,
                },
            ),
            State::Downloading { got, total, .. } => {
                let fraction = match total {
                    0 => 0.,
                    total => (*got as f32 / *total as f32).clamp(0., 1.),
                };
                plate(
                    cx,
                    Plate {
                        id: "status-bar-updating",
                        label: format!("Updating… {}%", (fraction * 100.).round() as u32),
                        icon: None,
                        fill: theme.accent,
                        progress: Some(fraction),
                        tooltip: Some(format!("{} of {}", megabytes(*got), megabytes(*total))),
                        action: Action::None,
                    },
                )
            }
            State::Installing(_) => plate(
                cx,
                Plate {
                    id: "status-bar-installing",
                    label: "Installing…".into(),
                    icon: None,
                    fill: theme.accent,
                    // Unpacking and moving are quick and have no honest
                    // fraction; a bar that jumped to full and sat there would
                    // be a lie.
                    progress: Some(1.),
                    tooltip: Some("Putting the new build in place".into()),
                    action: Action::None,
                },
            ),
            State::Restarting => plate(
                cx,
                Plate {
                    id: "status-bar-restarting",
                    label: "Restarting…".into(),
                    icon: None,
                    fill: theme.accent,
                    progress: Some(1.),
                    tooltip: Some("Arbos is reopening with your tabs and chats".into()),
                    action: Action::None,
                },
            ),
            State::Failed { why, update } => plate(
                cx,
                Plate {
                    id: "status-bar-update-failed",
                    label: "Update failed".into(),
                    icon: None,
                    fill: theme.danger,
                    progress: None,
                    tooltip: Some(format!("{why}\n\nArbos is unchanged. Click to try again.",)),
                    action: match update.is_some() {
                        true => Action::Install,
                        false => Action::None,
                    },
                },
            ),
        }
    }
}

/// Everything the resting control needs to know.
struct Resting {
    state: State,
    channel: Channel,
    trouble: Option<String>,
    at: Option<std::path::PathBuf>,
    checked: Option<Checked>,
}

/// Resting: what build this is, and a click to look for a newer one.
///
/// Three things rest here and they must not look alike. Up to date is the
/// version alone. Checking is the same, dimmer. A check that could not reach
/// the channel says so in words beside the version — quietly, because nothing
/// is broken, but *visibly*, because folding it into the quiet state is how an
/// app that has silently lost its channel passes for one that is current.
///
/// Each carries its own element id, so what the bar is showing can be read off
/// the element tree instead of a colour or a tooltip. A pointer warp raises no
/// hover event in this UI, which makes anything that appears only on hover
/// untestable — so the id is the assertion, and the tooltip only ever repeats
/// what is already somewhere else.
fn quiet(theme: &Theme, resting: Resting, cx: &mut Context<Arbos>) -> AnyElement {
    let Resting {
        state,
        channel,
        trouble,
        at,
        checked,
    } = resting;
    let version = build::version_label();
    let unreachable = matches!(state, State::Unreachable { .. });
    let (id, tint) = match &state {
        State::Unreachable { .. } => ("status-bar-unreachable", theme.text_muted),
        State::Checking => ("status-bar-checking", theme.text_dim),
        _ => ("status-bar-version", theme.text_faint),
    };

    let last = last_checked(checked.as_ref());
    let tooltip = match (&trouble, &state) {
        // A build that cannot update itself says so here rather than offering
        // a button that would fail at the last step.
        (Some(why), _) => format!("Arbos {version}\n\n{why}"),
        (None, State::Unreachable { why }) => format!(
            "Could not reach the {} channel.\n{why}\n\n{last}\nClick to try again.",
            channel.as_str()
        ),
        _ => format!(
            "Arbos {version} — up to date on the {} channel.\n{last}\nClick to check again.{}",
            channel.as_str(),
            // Which copy this is. The first question worth answering when
            // ⌘Space opens the wrong Arbos, or none.
            match &at {
                Some(at) => format!("\n\n{}", at.display()),
                None => String::new(),
            }
        ),
    };

    theme
        .ghost(id)
        .px(px(6.))
        .py(px(3.))
        .gap(px(4.))
        .text_style(TextStyle::Caption)
        .text_color(tint)
        .tooltip(move |window, cx| Tooltip::text(tooltip.clone(), window, cx))
        .child(version)
        // In words, not a colour: a muted dot would be invisible to anybody
        // not looking for it, and unreadable to anything driving the app.
        .when(unreachable, |el| {
            el.child(
                div()
                    .id("status-bar-unreachable-note")
                    .text_color(theme.text_muted)
                    .child("· check failed"),
            )
        })
        .on_click(cx.listener(|this, _, _, cx| {
            this.updater.update(cx, |updater, cx| updater.check(cx));
        }))
        .into_any_element()
}

/// `Checked 4m ago`, `Checked 4m ago — it failed`, or that nobody has looked.
///
/// Shown in Settings as its own row as well as here, because a tooltip needs a
/// pointer resting on a control to appear and so cannot be the only place this
/// is written down.
pub(crate) fn last_checked(checked: Option<&Checked>) -> String {
    match checked {
        None => "Not checked yet.".into(),
        Some(checked) => {
            let ago = crate::view::panel::age(checked.at);
            match &checked.failed {
                None => format!("Checked {ago} ago."),
                Some(_) => format!("Checked {ago} ago — it failed."),
            }
        }
    }
}

/// Everything the control looks like when it has something to say.
/// What a click on the plate does. One control, two jobs — and they must not
/// be confused, because one installs a new app and the other ends whatever a
/// kernel is running.
#[derive(Clone)]
enum Action {
    /// Nothing; the plate is showing progress.
    None,
    /// Download and install the update being offered.
    Install,
    /// Stop the kernel serving this place and let the app start it again on
    /// this build.
    RestartKernel(crate::model::place::Place),
}

struct Plate {
    /// The element id. One per state rather than one for the control, so what
    /// the bar is showing can be asserted without a pointer or a colour.
    id: &'static str,
    label: String,
    icon: Option<&'static str>,
    /// The plate's colour: the accent for an update, danger for a failure.
    fill: Hsla,
    /// `0.0` to `1.0`, drawn as the plate filling left to right. `None` for a
    /// plate that is not working on anything.
    progress: Option<f32>,
    tooltip: Option<String>,
    action: Action,
}

/// The filled control.
///
/// bezel ships `Ghost`, `Prominent` and `Destructive`, and `Prominent` is a
/// white plate — right for a dialog's confirm, wrong for this. Cursor's update
/// button is its one blue, so this is built from `theme.accent`, which
/// `view::palette` paints Cursor's blue.
fn plate(cx: &mut Context<Arbos>, plate: Plate) -> AnyElement {
    let Plate {
        id,
        label,
        icon,
        fill,
        progress,
        tooltip,
        action,
    } = plate;
    let clickable = !matches!(action, Action::None);
    let ink = on_plate(fill);
    div()
        .id(id)
        .relative()
        .overflow_hidden()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.))
        .h(px(20.))
        .px(px(8.))
        .rounded(px(Theme::control_radius()))
        // The plate under everything. While a download is running the same
        // colour is laid over it at full strength up to the fraction done, so
        // the button *is* the progress bar rather than growing one.
        .bg(dim(fill, 0.45))
        .when_some(progress, |el, fraction| {
            el.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .bottom_0()
                    .w(bezel::gpui::relative(fraction))
                    .bg(fill),
            )
        })
        .when(progress.is_none(), |el| el.bg(fill))
        .when(clickable, |el| {
            el.cursor_pointer()
                // Lighter on hover, the way Cursor's does. `opacity` on the
                // whole control rather than a second background, so the label
                // moves with the plate instead of washing out against it.
                .hover(|el| el.opacity(0.88))
                .active(|el| el.opacity(0.78))
        })
        .children(
            icon.map(|icon| {
                div().child(icons::icon(icon).size(px(12.)).flex_none().text_color(ink))
            }),
        )
        .child(
            div()
                .relative()
                .text_style(TextStyle::Caption)
                .font_weight(FontWeight::MEDIUM)
                .text_color(ink)
                .child(label),
        )
        .when_some(tooltip, |el, tooltip| {
            el.tooltip(move |window, cx| Tooltip::text(tooltip.clone(), window, cx))
        })
        .when(clickable, |el| {
            el.on_click(cx.listener(move |this, _, _, cx| match action.clone() {
                Action::None => {}
                // The stranger's plate ran the update path instead of its
                // own action (F-125, cycle 25): a click meant to end one
                // kernel installed a new app.
                Action::RestartKernel(place) => this.restart_stranger(place, cx),
                Action::Install => {
                    // Every place this app knows a kernel for, not only the
                    // tabs that happen to be open.
                    //
                    // The open projects alone are not enough, and that gap
                    // did real harm: after an update on 2026-09-17 a kernel
                    // from a closed tab kept running from the deleted old
                    // bundle, and the new app attached to it and sent frames
                    // it had never heard of. Recents are where those kernels
                    // are — a place stops being a tab long before its kernel
                    // stops running.
                    let workspace = this.workspace.read(cx);
                    let mut places: Vec<_> = workspace
                        .projects
                        .iter()
                        .map(|project| project.place().clone())
                        .collect();
                    for recent in &workspace.recents {
                        if !places.contains(recent) {
                            places.push(recent.clone());
                        }
                    }
                    this.updater
                        .update(cx, |updater, cx| updater.install(places, cx));
                }
            }))
        })
        .into_any_element()
}

/// A label that reads on a plate.
///
/// `theme.on_accent` is bezel's answer for `theme.accent_strong`, which is a
/// near-neutral plate; `view::palette` paints `accent` an actual blue and
/// leaves the other two alone, so the pair would not match. Choosing from the
/// plate's own lightness is right whichever colour it is given, which also
/// keeps the failed state legible without a second token.
fn on_plate(fill: Hsla) -> Hsla {
    match fill.l > 0.5 {
        true => hsla(0., 0., 0.08, 1.),
        false => hsla(0., 0., 1., 1.),
    }
}

/// The same colour, held back — the unfilled part of the plate while a
/// download runs.
fn dim(color: Hsla, by: f32) -> Hsla {
    hsla(color.h, color.s, color.l, color.a * by)
}

/// `48.2 MB`. Bytes are not a thing to read while waiting.
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.)
}

/// Redraw the bar when the updater moves.
pub(crate) fn observe(arbos: &mut Context<Arbos>, updater: &Entity<Updater>) {
    arbos.observe(updater, |_, _, cx| cx.notify()).detach();
}
