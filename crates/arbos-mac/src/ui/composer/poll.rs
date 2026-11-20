//! The composer's poll form (`docs/polls.md`): `+` → "Poll" opens this card above the field, in
//! a people chat. A question, two to ten options, a switch for multiple answers, a switch for
//! showing who voted, how long it runs, Send.
//!
//! The bounds are `arbos_core::poll`'s, the same ones the relay checks a pushed poll against:
//! Send stays faded until [`Poll::new`] would accept what the fields hold.

use std::rc::Rc;

use arbos_core::poll::{Poll, OPTIONS_MAX, OPTIONS_MIN};
use gpui::{div, prelude::*, px, AnyElement, App, Context, Entity, Focusable, FontWeight, Window};

use super::{Composer, ComposerInput};
use crate::ui::glass;
use crate::ui::highlight;
use crate::ui::sf_symbols;
use crate::ui::theme::{Theme, PANEL_RADIUS, TEXT_SMALL};

const WIDTH: f32 = 320.0;
const FIELD_H: f32 = 30.0;
const HOUR_MS: i64 = 60 * 60 * 1000;
/// How long a poll runs: the row's label and the time from now, `None` until its author
/// closes it. A click on the row steps to the next.
const DURATIONS: [(&str, Option<i64>); 4] = [
    ("No time limit", None),
    ("Closes in 1 hour", Some(HOUR_MS)),
    ("Closes in 1 day", Some(24 * HOUR_MS)),
    ("Closes in 1 week", Some(7 * 24 * HOUR_MS)),
];
/// The step after the last of [`DURATIONS`]: a number of hours typed into a field.
const CUSTOM: usize = DURATIONS.len();

/// A custom duration as typed: whole hours, from one to the week a poll may run.
fn custom_duration(hours: &str) -> Option<i64> {
    let hours: i64 = hours.trim().parse().ok()?;
    (1..=arbos_core::poll::DURATION_MAX_MS / HOUR_MS)
        .contains(&hours)
        .then_some(hours * HOUR_MS)
}

pub struct PollDraft {
    question: Entity<ComposerInput>,
    options: Vec<Entity<ComposerInput>>,
    multiple: bool,
    /// "Show who voted". Off: the card shows counts only.
    pub(super) show_voters: bool,
    /// Index into [`DURATIONS`], or [`CUSTOM`].
    pub(super) duration: usize,
    /// The hours field shown under the duration row at [`CUSTOM`].
    custom_hours: Entity<ComposerInput>,
}

impl Composer {
    /// Polls live in people chats that exist: not an agent's own chat, not a DM still unsent.
    pub(super) fn can_poll(&self, cx: &App) -> bool {
        let state = self.state.read(cx);
        state.active_chat_id().is_some_and(|chat| {
            !crate::draft_dm::is_draft(&chat)
                && state.project_of_chat(&chat).is_some_and(|p| p.is_people())
        })
    }

    fn poll_input(&self, placeholder: String, cx: &mut Context<Self>) -> Entity<ComposerInput> {
        let input = cx.new(ComposerInput::new);
        let (on_enter, on_escape) = (cx.weak_entity(), cx.weak_entity());
        input.update(cx, |input, _| {
            input.set_placeholder(placeholder);
            input.set_font_size(TEXT_SMALL);
            input.on_submit = Some(Rc::new(move |_: &mut Window, cx: &mut App| {
                let _ = on_enter.update(cx, |composer, cx| composer.send_poll(cx));
            }));
            input.on_mention_escape = Some(Rc::new(move |cx| {
                on_escape
                    .update(cx, |composer, cx| composer.close_poll(cx))
                    .is_ok()
            }));
        });
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        input
    }

    pub(super) fn open_poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_fun(cx);
        let question = self.poll_input("Ask a question".into(), cx);
        let options = (1..=OPTIONS_MIN)
            .map(|n| self.poll_input(format!("Option {n}"), cx))
            .collect();
        let custom_hours = self.poll_input("Hours, 1 to 168".into(), cx);
        window.focus(&question.read(cx).focus_handle(cx), cx);
        self.poll = Some(PollDraft {
            question,
            options,
            multiple: false,
            show_voters: false,
            duration: 0,
            custom_hours,
        });
        cx.notify();
    }

    fn close_poll(&mut self, cx: &mut Context<Self>) {
        self.poll = None;
        cx.notify();
    }

    /// Take one option row out and renumber the placeholders of the rest.
    fn remove_poll_option(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(draft) = self.poll.as_mut() else {
            return;
        };
        if draft.options.len() <= OPTIONS_MIN || ix >= draft.options.len() {
            return;
        }
        draft.options.remove(ix);
        for (n, input) in draft.options.iter().enumerate() {
            input.update(cx, |i, _| i.set_placeholder(format!("Option {}", n + 1)));
        }
        cx.notify();
    }

    /// How long the drafted poll runs: `Some(None)` with no time limit, `None` while the
    /// custom hours field does not hold a duration.
    fn drafted_duration(&self, cx: &App) -> Option<Option<i64>> {
        let draft = self.poll.as_ref()?;
        match DURATIONS.get(draft.duration) {
            Some((_, closes_in)) => Some(*closes_in),
            None => custom_duration(draft.custom_hours.read(cx).text()).map(Some),
        }
    }

    fn drafted_poll(&self, cx: &App) -> Option<(String, Vec<String>, bool)> {
        let draft = self.poll.as_ref()?;
        let question = draft.question.read(cx).text().to_owned();
        let options: Vec<String> = draft
            .options
            .iter()
            .map(|o| o.read(cx).text().to_owned())
            .collect();
        let closes_in = self.drafted_duration(cx)?;
        Poll::new(&question, &options, draft.multiple, false, closes_in)
            .map(|_| (question, options, draft.multiple))
    }

    fn send_poll(&mut self, cx: &mut Context<Self>) {
        let Some((question, options, multiple)) = self.drafted_poll(cx) else {
            return;
        };
        let show_voters = self.poll.as_ref().is_some_and(|d| d.show_voters);
        let closes_in = self.drafted_duration(cx).flatten();
        let Some(chat) = self.state.read(cx).active_chat_id() else {
            return;
        };
        let sent = self.state.update(cx, |s, cx| {
            s.send_poll(
                chat,
                &question,
                &options,
                multiple,
                show_voters,
                closes_in,
                cx,
            )
        });
        if sent {
            self.close_poll(cx);
        }
    }

    #[cfg(test)]
    pub(super) fn fill_poll_for_test(
        &mut self,
        question: &str,
        options: &[&str],
        cx: &mut Context<Self>,
    ) {
        let draft = self.poll.as_ref().expect("the form is open");
        draft
            .question
            .update(cx, |i, cx| i.set_text(question.to_owned(), cx));
        for (input, label) in draft.options.iter().zip(options) {
            input.update(cx, |i, cx| i.set_text((*label).to_owned(), cx));
        }
    }

    pub(super) fn render_poll(&self, draft: &PollDraft, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let field = |input: &Entity<ComposerInput>| {
            div()
                .h(px(FIELD_H))
                .px(px(10.0))
                .rounded(px(8.0))
                .bg(theme.field_fill())
                .border_1()
                .border_color(theme.field_border)
                .flex()
                .items_center()
                .cursor_text()
                .text_size(px(TEXT_SMALL))
                .child(div().flex_1().min_w_0().child(input.clone()))
        };
        let quiet_row = |id: &'static str, lead: gpui::Div, label: &'static str| {
            div()
                .id(id)
                .h(px(FIELD_H))
                .px(px(6.0))
                .rounded(px(8.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .cursor_pointer()
                .map(|el| highlight::button(el, &theme))
                .child(lead)
                .child(
                    div()
                        .text_size(px(TEXT_SMALL))
                        .text_color(theme.text)
                        .child(label),
                )
        };
        // A switch: a box, ticked when on.
        let tick_box = |on: bool| {
            div()
                .size(px(14.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(theme.field_border)
                .flex()
                .items_center()
                .justify_center()
                .when(on, |el| {
                    el.bg(theme.primary).child(sf_symbols::glyph(
                        "checkmark",
                        9.0,
                        theme.on_primary,
                    ))
                })
        };
        let ready = self.drafted_poll(cx).is_some();
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(TEXT_SMALL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child("Poll"),
            )
            .child(
                div()
                    .id("poll-close")
                    .size(px(24.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .map(|el| highlight::button(el, &theme))
                    .child(sf_symbols::glyph("xmark", 11.0, theme.text_muted))
                    .on_click(cx.listener(|this, _, _, cx| this.close_poll(cx))),
            );
        div()
            .id("poll-form")
            .debug_selector(|| "composer-poll-form".into())
            .flex()
            .flex_col()
            .gap(px(8.0))
            .w(px(WIDTH))
            .p(px(12.0))
            .rounded(px(PANEL_RADIUS))
            .bg(theme.card())
            .border_1()
            .border_color(theme.border_strong)
            .shadow_lg()
            // Floats over the transcript: its own hitbox blocks the rows beneath.
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(field(&draft.question))
            .children(draft.options.iter().enumerate().map(|(ix, input)| {
                // Rows past the two a poll needs can be taken back out.
                field(input).when(draft.options.len() > OPTIONS_MIN, |el| {
                    el.child(
                        div()
                            .id(("poll-remove-option", ix))
                            .size(px(20.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .map(|el| highlight::button(el, &theme))
                            .child(sf_symbols::glyph("xmark", 9.0, theme.text_muted))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.remove_poll_option(ix, cx)),
                            ),
                    )
                })
            }))
            .when(draft.options.len() < OPTIONS_MAX, |el| {
                el.child(
                    quiet_row(
                        "poll-add-option",
                        sf_symbols::glyph("plus", 14.0, theme.text_muted),
                        "Add option",
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        let n = this.poll.as_ref().map_or(0, |d| d.options.len()) + 1;
                        let input = this.poll_input(format!("Option {n}"), cx);
                        window.focus(&input.read(cx).focus_handle(cx), cx);
                        if let Some(draft) = this.poll.as_mut() {
                            draft.options.push(input);
                        }
                        cx.notify();
                    })),
                )
            })
            .child(
                quiet_row(
                    "poll-multiple",
                    tick_box(draft.multiple),
                    "Allow multiple answers",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(draft) = this.poll.as_mut() {
                        draft.multiple = !draft.multiple;
                    }
                    cx.notify();
                })),
            )
            .child(
                quiet_row(
                    "poll-show-voters",
                    tick_box(draft.show_voters),
                    "Show who voted",
                )
                .debug_selector(|| "composer-poll-show-voters".into())
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(draft) = this.poll.as_mut() {
                        draft.show_voters = !draft.show_voters;
                    }
                    cx.notify();
                })),
            )
            .child(
                quiet_row(
                    "poll-duration",
                    sf_symbols::glyph("clock", 14.0, theme.text_muted),
                    DURATIONS
                        .get(draft.duration)
                        .map_or("Closes in a number of hours", |d| d.0),
                )
                .debug_selector(|| "composer-poll-duration".into())
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(draft) = this.poll.as_mut() {
                        draft.duration = (draft.duration + 1) % (CUSTOM + 1);
                    }
                    cx.notify();
                })),
            )
            .when(draft.duration == CUSTOM, |el| {
                el.child(field(&draft.custom_hours).debug_selector(|| "composer-poll-hours".into()))
            })
            .child(
                glass::primary_button("poll-send", "Send poll", &theme)
                    .debug_selector(|| "composer-poll-send".into())
                    .when(!ready, |el| el.opacity(0.4).cursor_default())
                    .on_click(cx.listener(|this, _, _, cx| this.send_poll(cx))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_custom_duration_is_whole_hours_up_to_a_week() {
        assert_eq!(custom_duration(" 3 "), Some(3 * HOUR_MS));
        assert_eq!(
            custom_duration("168"),
            Some(arbos_core::poll::DURATION_MAX_MS)
        );
        for bad in ["", "0", "169", "-2", "1.5", "two"] {
            assert_eq!(custom_duration(bad), None, "{bad}");
        }
    }
}
