//! The few non-prose rows the transcript still shows: the working line
//! (S4/S6/S17 collapsed into one indicator), the ask-user card (S8) and the
//! error chip (S10). Tool detail lives in the Details drawer.

use gpui::{div, prelude::*, px, App, Div, FontWeight, SharedString, Stateful};

use crate::model::StatusKind;
use crate::sync::user_error::{ErrorClass, UserError};
use crate::ui::error_note;
use crate::ui::motion::PulseClock;
use crate::ui::theme::{Theme, PANEL_RADIUS};

/// The small working mark in banners: the braille spinner (never a pulsing
/// dot — Jacob, 2026-10-27).
pub fn spinner(theme: &Theme, cx: &mut App) -> Div {
    crate::ui::working::glyph(theme, cx)
}

/// Grok Bot's activity spinner — the blue eight-spoke dial that steps round
/// while an agent works (measured 15 pt, `#6098F5`-ish, on the Coding row of
/// `grok-right-pane-details.png`). One step per eighth of the clock's period.
pub fn activity_spinner(size: f32, theme: &Theme, cx: &App) -> gpui::Svg {
    let step = (PulseClock::ramp(cx, 800) * 8.0).floor() / 8.0;
    // icon: custom — Grok's eight-spoke dial with fading spokes, rotated per step; SF has no
    // multi-opacity spinner glyph (`rays` is uniform), so this stays the pack's one custom path.
    crate::ui::icons::icon("spinner", size, theme.accent).with_transformation(
        gpui::Transformation::rotate(gpui::radians(step * std::f32::consts::TAU)),
    )
}

/// The chat's single working indicator: the braille spinner on its own row
/// (Jacob, 2026-10-27 — no pulsing dot, no shimmer line). While the message
/// is queued for a sleeping box the spinner is followed by nothing either;
/// the status row's words say "waiting for the machine".
pub fn indicator(queued: bool, theme: &Theme, cx: &mut App) -> Div {
    let _ = queued;
    div()
        .w_full()
        .py(px(6.0))
        .flex()
        .items_center()
        .child(crate::ui::working::glyph(theme, cx))
}

pub fn status_row(code: StatusKind, text: &str, live: bool, theme: &Theme, cx: &mut App) -> Div {
    match code {
        StatusKind::AskUser => div()
            .w_full()
            .py(px(4.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .py(px(10.0))
                    .rounded(px(PANEL_RADIUS))
                    .bg(theme.card())
                    .border_l_2()
                    .border_color(theme.warn)
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.warn)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("WAITING FOR YOU"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(13.5))
                            .text_color(theme.text)
                            .child(text.to_string()),
                    ),
            ),
        StatusKind::Deciding
        | StatusKind::Working
        | StatusKind::Step
        | StatusKind::Queued
        | StatusKind::Done
        | StatusKind::Failed
        | StatusKind::Stopped
        | StatusKind::Handoff
        | StatusKind::WaitingApproval
        | StatusKind::Note
        | StatusKind::Other => {
            if live {
                indicator(false, theme, cx)
            } else {
                div()
                    .w_full()
                    .py(px(4.0))
                    .text_size(px(12.0))
                    .text_color(theme.text_faint)
                    .italic()
                    .child(text.to_string())
            }
        }
    }
}

/// S10: an agent turn that failed — the shared error element in the transcript's flow (the
/// agent's own sentence, the error code behind Details, Try again re-runs the turn).
pub fn error_chip(
    row_id: &str,
    code: &str,
    message: &str,
    theme: &Theme,
    on_retry: impl Fn(&mut App) + 'static,
) -> Stateful<Div> {
    let error = UserError::of(
        "",
        ErrorClass::Refused {
            message: Some(if message.trim().is_empty() {
                "The agent's turn failed.".to_owned()
            } else {
                message.to_owned()
            }),
        },
        code.to_owned(),
    );
    let error = UserError {
        class: ErrorClass::Unknown,
        ..error
    };
    let error = if message.trim().is_empty() {
        error
    } else {
        UserError {
            class: ErrorClass::Refused {
                message: Some(message.to_owned()),
            },
            ..error
        }
    };
    div()
        .id(SharedString::from(row_id.to_string()))
        .my(px(4.0))
        .w_full()
        .child(error_note::render(
            &format!("{row_id}-error"),
            &error,
            false,
            error_note::Style::Inline,
            theme,
            error_note::Handlers::new().try_again(move |_, _, cx| on_retry(cx)),
        ))
}
