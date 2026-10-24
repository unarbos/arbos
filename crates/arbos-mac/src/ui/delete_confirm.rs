//! "Delete message?" — the confirm the bubble menu's Delete opens instead of
//! deleting at once (feedback app-01M5MXQ90MEEWF6C8W090VCZHF, Jacob). One
//! card over a veil: red trash glyph and title, the quoted bubble (sender ·
//! time · first words), one line on what happens, then Cancel (Esc) and the
//! red Delete (Return). Delete is always "for everyone": the relay applies the
//! #1526 rule and the menu only offered it where that rule holds. On someone
//! else's message (a moderator) the title names the sender.

use gpui::{
    actions, div, prelude::*, px, AnyElement, Context, Div, FocusHandle, FontWeight, MouseButton,
    Stateful,
};

use crate::app::DeleteConfirm;
use crate::ui::icons::icon;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, TEXT_SMALL, TEXT_TINY};
use crate::ui::transcript::message_line;

actions!(delete_confirm, [ConfirmDelete]);

/// Card width (mock-mac: 400 pt).
pub const CARD_WIDTH: f32 = 400.0;
/// Pill height for Cancel / Delete.
pub const BUTTON_HEIGHT: f32 = 30.0;
/// The quoted bubble shows this many characters before "…".
const QUOTE_CHARS: usize = 120;

/// What the card says about one message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copy {
    pub title: String,
    pub body: String,
}

/// Title and body for a delete: your own message gets the plain title; a
/// moderator deleting someone else's sees whose it is.
pub fn copy_for(mine: bool, sender: &str) -> Copy {
    let title = if mine || sender.trim().is_empty() {
        "Delete message?".to_owned()
    } else {
        format!("Delete {}’s message?", sender.trim())
    };
    Copy {
        title,
        body: "This removes it from the chat for everyone. This can’t be undone.".to_owned(),
    }
}

/// The open confirm, appended to the shell's root; `focus` takes the keyboard
/// so Return confirms and Esc cancels.
pub fn overlay(shell: &Shell, focus: &FocusHandle, cx: &mut Context<Shell>) -> Option<AnyElement> {
    let confirm: DeleteConfirm = shell.state.read(cx).delete_confirm.clone()?;
    let theme = Theme::of(cx).clone();
    let (mine, sender, time, quote) = {
        let state = shell.state.read(cx);
        let message = state
            .messages_of(&confirm.chat_id)
            .iter()
            .find(|m| m.id == confirm.message_id)
            .cloned();
        match message {
            Some(m) => {
                // A project chat's own agent writes bare `agent` / `assistant`
                // rows: name it after the chat's agent, not "Agent".
                let sender = if matches!(m.sender.as_str(), "agent" | "assistant" | "project") {
                    state
                        .project(&state.space_of_chat(&confirm.chat_id))
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| state.viewer_sender_name(&m))
                } else {
                    state.viewer_sender_name(&m)
                };
                (
                    state.is_mine(&m),
                    sender,
                    crate::ui::media_viewer::when_label(m.created_at),
                    message_line(&m, QUOTE_CHARS),
                )
            }
            None => (true, String::new(), String::new(), String::new()),
        }
    };
    let copy = copy_for(mine, &sender);
    let shown_sender = if mine { "You".to_owned() } else { sender };

    let close = shell.state.clone();
    let close_bg = shell.state.clone();
    let confirm_state = shell.state.clone();
    let confirm_key = shell.state.clone();

    let quote_card = div()
        .flex()
        .gap(px(10.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(10.0))
        .bg(theme.surface_2)
        .child(
            div()
                .w(px(3.0))
                .flex_shrink_0()
                .rounded(px(2.0))
                .bg(theme.text_faint),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(px(TEXT_TINY + 0.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .child(shown_sender),
                        )
                        .child(
                            div()
                                .text_size(px(TEXT_TINY))
                                .text_color(theme.text_muted)
                                .child(time),
                        ),
                )
                .child(
                    div()
                        .text_size(px(TEXT_TINY + 0.5))
                        .text_color(theme.text)
                        .child(quote),
                ),
        );

    let buttons = div()
        .flex()
        .justify_end()
        .items_center()
        .gap(px(10.0))
        .child(
            ghost_button("delete-confirm-cancel", "Cancel", "esc", &theme).on_click(
                move |_, _, cx| {
                    cx.stop_propagation();
                    close.update(cx, |s, cx| s.close_delete_confirm(cx));
                },
            ),
        )
        .child(
            danger_button("delete-confirm-delete", "Delete", "⏎", &theme).on_click(
                move |_, _, cx| {
                    cx.stop_propagation();
                    confirm_state.update(cx, |s, cx| s.confirm_delete(cx));
                },
            ),
        );

    let card = div()
        .id("delete-confirm-card")
        .key_context("DeleteConfirm")
        .track_focus(focus)
        .on_action(move |_: &ConfirmDelete, _, cx| {
            confirm_key.update(cx, |s, cx| s.confirm_delete(cx));
        })
        .w(px(CARD_WIDTH))
        .rounded(px(14.0))
        .bg(theme.surface_1)
        .border_1()
        .border_color(theme.border_strong)
        .shadow_lg()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .p(px(20.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(icon("trash", 16.0, theme.danger))
                .child(
                    div()
                        .text_size(px(TEXT_BODY_TITLE))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(copy.title),
                ),
        )
        .child(quote_card)
        .child(
            div()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(copy.body),
        )
        .child(div().h(px(6.0)))
        .child(buttons);

    Some(
        div()
            .id("delete-confirm-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(Theme::with_alpha(gpui::black(), 0.35))
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
                close_bg.update(cx, |s, cx| s.close_delete_confirm(cx));
            })
            .child(card)
            .into_any_element(),
    )
}

/// 15 pt title.
const TEXT_BODY_TITLE: f32 = 15.0;

fn pill(id: &'static str, label: &str, hint: &str, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(BUTTON_HEIGHT))
        .px(px(14.0))
        .rounded(px(8.0))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .cursor_pointer()
        .text_size(px(TEXT_SMALL))
        .font_weight(FontWeight::MEDIUM)
        .child(label.to_owned())
        .child(
            div()
                .text_size(px(10.0))
                .text_color(Theme::with_alpha(theme.text, 0.55))
                .child(hint.to_owned()),
        )
}

fn ghost_button(id: &'static str, label: &str, hint: &str, theme: &Theme) -> Stateful<Div> {
    pill(id, label, hint, theme)
        .text_color(theme.text_muted)
        .hover(|s| s.bg(theme.surface_2))
}

fn danger_button(id: &'static str, label: &str, hint: &str, theme: &Theme) -> Stateful<Div> {
    pill(id, label, hint, theme)
        .bg(theme.danger)
        .text_color(gpui::white())
        .hover(|s| s.opacity(0.9))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_message_gets_the_plain_title() {
        let c = copy_for(true, "Jacob Steeves");
        assert_eq!(c.title, "Delete message?");
        assert!(c.body.contains("for everyone"));
    }

    #[test]
    fn a_moderator_sees_whose_message_it_is() {
        assert_eq!(
            copy_for(false, "Sam Rivera").title,
            "Delete Sam Rivera’s message?"
        );
        // No name to show: fall back to the plain title rather than "Delete ’s message?".
        assert_eq!(copy_for(false, "  ").title, "Delete message?");
    }

    #[test]
    fn card_matches_the_mock() {
        assert_eq!(CARD_WIDTH, 400.0);
        assert_eq!(BUTTON_HEIGHT, 30.0);
    }
}
