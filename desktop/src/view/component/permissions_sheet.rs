//! The permissions sheet: a modal over the chat — centred, the column
//! dimmed behind it — with one row per permission, one **Enable all** that
//! asks for them in sequence off the UI thread, the microphone test inline,
//! and **Skip for now**, which never nags again beyond a dot on the gear
//! when something the user tries needs a grant. Rows draw from
//! [`PermissionCenter`], which Settings › Permissions shares, so both say
//! the same thing and both turn green by themselves once a switch is
//! flipped in System Settings.

use crate::{
    model::permission_center::{PermissionCenter, Phase, Row},
    permissions::{Permission, Status},
    view::component::transcript,
    voice_ws,
};
use bezel::{
    gpui::{
        self, AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
        KeyBinding, MouseButton, Render, SharedString, Window, actions, div, prelude::*, px,
    },
    motion::Painter,
    theme::{TextStyle, Theme, Typeset},
    ui::{
        tooltip::Tooltip,
        widgets::{ButtonStyle, Buttons, Content, Scaffolding},
    },
};
use std::time::Duration;

actions!(arbos_permissions_sheet, [SkipPermissions]);

const KEY_CONTEXT: &str = "ArbosPermissionsSheet";
const WIDTH: f32 = 560.;
const LEVEL_WIDTH: f32 = 96.;
const LEVEL_FPS: f32 = 20.;

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", SkipPermissions, Some(KEY_CONTEXT))]);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionsSheetEvent {
    /// Closed — by Skip for now, Escape, Done, or a click outside.
    Closed,
}

pub struct PermissionsSheet {
    center: Entity<PermissionCenter>,
    open: bool,
    focus: FocusHandle,
}

impl EventEmitter<PermissionsSheetEvent> for PermissionsSheet {}

impl PermissionsSheet {
    pub fn new(center: Entity<PermissionCenter>, cx: &mut Context<Self>) -> Self {
        cx.observe(&center, |_, _, cx| cx.notify()).detach();
        Self {
            center,
            open: false,
            focus: cx.focus_handle(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            return;
        }
        self.open = true;
        self.center.update(cx, |center, cx| center.watch(cx));
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.center.update(cx, |center, _| center.unwatch());
        voice_ws::mic_test_stop();
        cx.emit(PermissionsSheetEvent::Closed);
        cx.notify();
    }

    fn skip(&mut self, _: &SkipPermissions, _: &mut Window, cx: &mut Context<Self>) {
        self.close(cx);
    }
}

impl Focusable for PermissionsSheet {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PermissionsSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        let theme = Theme::of(cx).clone();
        let painter = Painter::of(cx);
        let center = self.center.clone();
        let (rows, settled, enabling) = {
            let c = center.read(cx);
            (c.rows.clone(), c.all_settled(), c.enabling_all)
        };
        let row_elements: Vec<AnyElement> = rows
            .iter()
            .enumerate()
            .map(|(n, row)| permission_row(n, row, &center, painter, &theme, cx))
            .collect();
        let mic = mic_test_row(painter, &theme, cx);
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child(if settled && rows.iter().all(|row| row.status == Status::Granted) {
                        "Everything Arbos uses is allowed."
                    } else if settled {
                        "Nothing more to ask for here."
                    } else if cfg!(target_os = "macos") {
                        "Each one is macOS's own prompt. Where it will not ask, the row opens the pane."
                    } else {
                        "The desktop portal asks for the screen on first use; the microphone needs a capture program."
                    }),
            )
            .child(
                theme
                    .button(
                        if settled { "Done" } else { "Skip for now" },
                        ButtonStyle::Ghost,
                        None,
                    )
                    .id("permissions-skip")
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            )
            .when(!settled, |el| {
                el.child(
                    theme
                        .button(
                            if enabling { "Enabling…" } else { "Enable all" },
                            ButtonStyle::Prominent,
                            None,
                        )
                        .id("permissions-enable-all")
                        .when(enabling, |el| el.opacity(0.6))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.center.update(cx, |center, cx| center.enable_all(cx));
                        })),
                )
            });
        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::skip))
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.45,
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close(cx)),
            )
            .child(
                div()
                    .id("permissions-sheet")
                    .w(px(WIDTH))
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .p(px(20.))
                    .rounded(px(Theme::surface_radius() + 4.))
                    .bg(theme.surface_dialog)
                    .border_1()
                    .border_color(theme.border)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_style(TextStyle::Title3)
                                    .text_color(theme.text)
                                    .child("Permissions"),
                            )
                            .child(
                                div()
                                    .text_style(TextStyle::Subheadline)
                                    .text_color(theme.text_muted)
                                    .child("What the system lets Arbos do here. Each row asks for itself; Enable all asks for them one after another."),
                            ),
                    )
                    .child(theme.group_box().children(row_elements))
                    .child(mic)
                    .child(footer),
            )
            .into_any_element()
    }
}

/// One permission: its name and purpose, where it stands, and the one
/// thing to press. Shared with Settings › Permissions.
pub fn permission_row(
    n: usize,
    row: &Row,
    center: &Entity<PermissionCenter>,
    painter: Painter,
    theme: &Theme,
    cx: &mut App,
) -> AnyElement {
    let permission = row.permission;
    let waiting = matches!(row.phase, Phase::Requesting { .. } | Phase::Prompted { .. });
    let (label, tint): (&str, Hsla) = match (&row.status, &row.phase) {
        (Status::Granted, _) => ("Granted", theme.success),
        (_, Phase::Requesting { .. }) => ("Asking…", theme.text_muted),
        (_, Phase::Prompted { .. }) => ("Waiting for your answer", theme.text_muted),
        (_, Phase::NeedsSettings) => ("Needs System Settings", theme.warning),
        (Status::NotAsked, _) => ("Not asked", theme.text_muted),
        (Status::Denied, _) => ("Denied", theme.danger),
        (Status::Unavailable(_), _) => ("Unavailable", theme.text_faint),
    };
    let note: Option<String> = match (&row.status, &row.phase) {
        (Status::Unavailable(why), _) => Some(why.clone()),
        (Status::Granted, _) => None,
        (_, Phase::NeedsSettings) => Some(if cfg!(target_os = "macos") {
            "macOS won't prompt for this; enable Arbos here.".to_string()
        } else {
            "Turn it on in the system's settings.".to_string()
        }),
        _ => None,
    };
    let since = match &row.phase {
        Phase::Requesting { since } | Phase::Prompted { since } => Some(*since),
        _ => None,
    };
    let status: AnyElement = match since {
        Some(since) => div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .child(transcript::spinner_with(
                painter,
                since.elapsed().max(Duration::from_millis(1)),
                theme.text_muted,
                cx,
            ))
            .child(
                div()
                    .id(("permission-status", n))
                    .text_style(TextStyle::Caption)
                    .text_color(tint)
                    .child(label),
            )
            .into_any_element(),
        None => div()
            .id(("permission-status", n))
            .text_color(tint)
            .child(theme.badge(label))
            .into_any_element(),
    };
    let granted = row.status == Status::Granted;
    let unavailable = matches!(row.status, Status::Unavailable(_));
    let needs_settings = matches!(row.phase, Phase::NeedsSettings) || row.status == Status::Denied;
    let can_ask = !granted && !unavailable && !waiting && !needs_settings;
    let mut buttons = div().flex().flex_row().items_center().gap(px(6.));
    if can_ask {
        let center = center.clone();
        buttons = buttons.child(
            theme
                .button("Request", ButtonStyle::Prominent, None)
                .id(("permission-request", n))
                .on_click(move |_, _, cx| {
                    center.update(cx, |center, cx| center.request(permission, cx));
                }),
        );
    }
    if needs_settings && !granted && permission.settings_url().is_some() {
        let center = center.clone();
        buttons = buttons.child(
            theme
                .button("Open System Settings", ButtonStyle::Prominent, None)
                .id(("permission-settings", n))
                .on_click(move |_, _, cx| {
                    center.update(cx, |center, cx| center.open_settings(permission, cx));
                }),
        );
    }
    if permission == Permission::ScreenRecording && !granted && !unavailable && !waiting {
        let center = center.clone();
        buttons = buttons.child(
            theme
                .button("Try a capture now", ButtonStyle::Ghost, None)
                .id(("permission-capture", n))
                .tooltip(|window, cx| {
                    Tooltip::text(
                        "Sequoia lists an app under Screen Recording only after it has tried to capture",
                        window,
                        cx,
                    )
                })
                .on_click(move |_, _, cx| {
                    center.update(cx, |center, cx| center.try_capture(cx));
                }),
        );
    }
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
                        .mt(px(2.))
                        .text_style(TextStyle::Subheadline)
                        .text_color(theme.text_muted)
                        .child(permission.purpose()),
                )
                .children(note.map(|note| {
                    div()
                        .mt(px(2.))
                        .text_style(TextStyle::Caption)
                        .text_color(if unavailable { theme.text_faint } else { theme.warning })
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
                .child(status)
                .child(buttons),
        )
        .into_any_element()
}

/// The microphone heard: the in-process probe with a level bar, inline in
/// the sheet. Shared with Settings › Permissions.
pub fn mic_test_row(painter: Painter, theme: &Theme, cx: &mut App) -> AnyElement {
    let test = voice_ws::mic_test();
    let live = test.is_some();
    if live {
        painter.lease(LEVEL_FPS, Duration::from_millis(300), cx);
    }
    let error = test.as_ref().and_then(|t| t.error.clone());
    let detail: String = match &test {
        Some(t) => match &t.error {
            Some(err) => format!("Not hearing: {err}"),
            None if t.device.is_empty() => "Opening the microphone…".into(),
            None => format!("Listening on {}.", t.device),
        },
        None => match voice_ws::mic_permission().advice() {
            Some(advice) => advice.to_string(),
            None => "Press Test and say something; the bar shows what the mic hears.".into(),
        },
    };
    let level = test.as_ref().map(|t| t.level).unwrap_or(0.).clamp(0., 1.);
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
                                .mt(px(2.))
                                .text_style(TextStyle::Subheadline)
                                .text_color(if error.is_some() { theme.danger } else { theme.text_muted })
                                .child(SharedString::from(detail)),
                        ),
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
                                        .bg(if live && level > 0.02 {
                                            theme.success
                                        } else {
                                            theme.text_faint
                                        }),
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
                                .on_click(move |_, _, cx| {
                                    if live {
                                        voice_ws::mic_test_stop();
                                    } else {
                                        voice_ws::mic_test_start();
                                    }
                                    cx.refresh_windows();
                                }),
                        ),
                ),
        )
        .into_any_element()
}
