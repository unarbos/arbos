//! The few non-prose rows the transcript still shows: the working line
//! (S4/S6/S17 collapsed into one indicator), the ask-user card (S8) and the
//! error chip (S10). Tool detail lives in the Details drawer.

use gpui::{div, prelude::*, px, App, Div, FontWeight, SharedString, Stateful};

use crate::model::StatusKind;
use crate::ui::motion::PulseClock;
use crate::ui::theme::{Theme, PANEL_RADIUS};

pub fn spinner(theme: &Theme, cx: &App) -> Div {
    let phase = PulseClock::phase(cx, 900);
    div()
        .size(px(8.0))
        .rounded_full()
        .bg(Theme::with_alpha(theme.accent, 0.35 + 0.65 * phase))
}

/// Grok Bot's activity spinner — the blue eight-spoke dial that steps round
/// while an agent works (measured 15 pt, `#6098F5`-ish, on the Coding row of
/// `grok-right-pane-details.png`). One step per eighth of the clock's period.
pub fn activity_spinner(size: f32, theme: &Theme, cx: &App) -> gpui::Svg {
    let step = (PulseClock::ramp(cx, 800) * 8.0).floor() / 8.0;
    crate::ui::icons::icon("spinner", size, theme.accent).with_transformation(
        gpui::Transformation::rotate(gpui::radians(step * std::f32::consts::TAU)),
    )
}

/// The chat's single working indicator: a pulsing dot that breathes a short
/// shimmer line beside it. Amber while the message is queued for a sleeping
/// box. No words.
pub fn indicator(queued: bool, theme: &Theme, cx: &App) -> Div {
    let phase = PulseClock::phase(cx, 1100);
    let base = if queued { theme.warn } else { theme.accent };
    div()
        .w_full()
        .py(px(6.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .size(px(8.0))
                .rounded_full()
                .bg(Theme::with_alpha(base, 0.35 + 0.65 * phase)),
        )
        .child(
            div()
                .h(px(2.0))
                .w(px(28.0 + 36.0 * phase))
                .rounded_full()
                .bg(Theme::with_alpha(base, 0.25 + 0.35 * (1.0 - phase))),
        )
}

pub fn status_row(code: StatusKind, text: &str, live: bool, theme: &Theme, cx: &App) -> Div {
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

/// S10: red-tinted chip with the short message and Retry.
pub fn error_chip(
    row_id: &str,
    code: &str,
    message: &str,
    theme: &Theme,
    on_retry: impl Fn(&mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(SharedString::from(row_id.to_string()))
        .my(px(4.0))
        .flex()
        .items_center()
        .gap(px(10.0))
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(PANEL_RADIUS))
        .bg(theme.danger_soft)
        .border_1()
        .border_color(Theme::with_alpha(theme.danger, 0.4))
        .child(
            div()
                .text_color(theme.danger)
                .text_size(px(12.0))
                .child("✗"),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(12.5))
                .text_color(theme.text)
                .child(message.to_string()),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(theme.text_faint)
                .child(code.to_string()),
        )
        .child(
            div()
                .id(SharedString::from(format!("{row_id}-retry")))
                .text_size(px(12.0))
                .text_color(theme.accent)
                .cursor_pointer()
                .child("Retry")
                .on_click(move |_, _, cx| on_retry(cx)),
        )
}
