//! The composer's poll form (`docs/polls.md`): `+` → "Poll" opens this card above the field, in
//! a people chat. A question, two to ten options, one switch for multiple answers, Send.
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

pub struct PollDraft {
    question: Entity<ComposerInput>,
    options: Vec<Entity<ComposerInput>>,
    multiple: bool,
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
        window.focus(&question.read(cx).focus_handle(cx), cx);
        self.poll = Some(PollDraft {
            question,
            options,
            multiple: false,
        });
        cx.notify();
    }

    fn close_poll(&mut self, cx: &mut Context<Self>) {
        self.poll = None;
        cx.notify();
    }

    /// The poll the fields hold, when it is one the relay would take.
    fn drafted_poll(&self, cx: &App) -> Option<(String, Vec<String>, bool)> {
        let draft = self.poll.as_ref()?;
        let question = draft.question.read(cx).text().to_owned();
        let options: Vec<String> = draft
            .options
            .iter()
            .map(|o| o.read(cx).text().to_owned())
            .collect();
        Poll::new(&question, &options, draft.multiple).map(|_| (question, options, draft.multiple))
    }

    fn send_poll(&mut self, cx: &mut Context<Self>) {
        let Some((question, options, multiple)) = self.drafted_poll(cx) else {
            return;
        };
        let Some(chat) = self.state.read(cx).active_chat_id() else {
            return;
        };
        let sent = self.state.update(cx, |s, cx| {
            s.send_poll(chat, &question, &options, multiple, cx)
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
        // The switch: a box, ticked when several answers are allowed.
        let tick_box = div()
            .size(px(14.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(theme.field_border)
            .flex()
            .items_center()
            .justify_center()
            .when(draft.multiple, |el| {
                el.bg(theme.primary)
                    .child(sf_symbols::glyph("checkmark", 9.0, theme.on_primary))
            });
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
            .children(draft.options.iter().map(field))
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
                quiet_row("poll-multiple", tick_box, "Allow multiple answers").on_click(
                    cx.listener(|this, _, _, cx| {
                        if let Some(draft) = this.poll.as_mut() {
                            draft.multiple = !draft.multiple;
                        }
                        cx.notify();
                    }),
                ),
            )
            .child(
                glass::primary_button("poll-send", "Send poll", &theme)
                    .debug_selector(|| "composer-poll-send".into())
                    .when(!ready, |el| el.opacity(0.4).cursor_default())
                    .on_click(cx.listener(|this, _, _, cx| this.send_poll(cx))),
            )
            .into_any_element()
    }
}
