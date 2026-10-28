//! Secret hand-off request card (`payload.kind = secret_offer`): the Grok question-card frame
//! holding a Name field (prefilled, editable), a masked Value field and a "Save to vault" row.
//! The value is typed into a local input and leaves this device once — `POST /secrets` — and
//! the card is then answered with the saved secret's name and id. It is never a draft, never a
//! message, never part of the card's payload. Mock: project store
//! `media/feedback/secret-handoff/` (Jacob, 2026-10-26).

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, App, Context, Div, Entity, FocusHandle, Focusable, FontWeight, Render,
    SharedString, Stateful, Window,
};
use serde_json::Value;

use crate::app::{AppState, HandoffState};
use crate::model::{ApprovalState, Id, Part};
use crate::ui::composer::ComposerInput;
use crate::ui::error_note;
use crate::ui::theme::{Theme, TEXT_SMALL, TEXT_TINY};
use crate::ui::widgets::{
    card_frame, card_header, option_list, option_row, CardLook, CloseHandler, RowState, ROW_HEIGHT,
    ROW_PAD_X,
};

/// What the card reads from its approval part each render.
struct View {
    state: ApprovalState,
    agent: String,
    purpose: String,
    suggested: String,
}

pub struct SecretOfferCard {
    state: Entity<AppState>,
    chat_id: Id,
    approval_id: String,
    project_id: Id,
    focus_handle: FocusHandle,
    pub(crate) name_input: Entity<ComposerInput>,
    pub(crate) value_input: Entity<ComposerInput>,
    /// The value field shows its characters (local only, before Save).
    show: bool,
    focused_once: bool,
    dismissed: bool,
}

impl SecretOfferCard {
    pub fn new(
        state: Entity<AppState>,
        chat_id: Id,
        approval_id: String,
        project_id: Id,
        suggested_name: &str,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_input = cx.new(ComposerInput::new);
        let value_input = cx.new(ComposerInput::new);
        let suggested = suggested_name.to_owned();
        let weak = cx.weak_entity();
        let value_for_enter = value_input.clone();
        name_input.update(cx, |input, cx| {
            input.set_placeholder("STRIPE_KEY");
            input.set_text(suggested, cx);
            // Enter on the name moves to the value.
            input.on_submit = Some(Rc::new(move |window: &mut Window, cx: &mut App| {
                let h = value_for_enter.read(cx).focus_handle(cx);
                window.focus(&h, cx);
            }));
        });
        value_input.update(cx, |input, _| {
            input.set_placeholder("Paste the key");
            input.set_masked(true);
            input.on_submit = Some(Rc::new(move |_: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |card, cx| card.save(cx));
            }));
        });
        cx.observe(&name_input, |_, _, cx| cx.notify()).detach();
        cx.observe(&value_input, |_, _, cx| cx.notify()).detach();
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            chat_id,
            approval_id,
            project_id,
            focus_handle: cx.focus_handle(),
            name_input,
            value_input,
            show: false,
            focused_once: false,
            dismissed: false,
        }
    }

    fn view(&self, cx: &App) -> Option<View> {
        let state = self.state.read(cx);
        state
            .messages_of(&self.chat_id)
            .iter()
            .flat_map(|m| m.parts.iter())
            .find_map(|p| match p {
                Part::Approval {
                    approval_id,
                    payload,
                    state,
                    summary,
                    ..
                } if *approval_id == self.approval_id => Some(View {
                    state: *state,
                    agent: str_of(payload, "agent"),
                    purpose: if summary.is_empty() {
                        str_of(payload, "purpose")
                    } else {
                        summary.clone()
                    },
                    suggested: str_of(payload, "name"),
                }),
                _ => None,
            })
    }

    fn name(&self, cx: &App) -> String {
        self.name_input
            .read(cx)
            .text()
            .trim()
            .to_ascii_uppercase()
            .replace([' ', '-', '.'], "_")
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let value = self.value_input.read(cx).text().to_owned();
        let name = self.name(cx);
        if value.trim().is_empty() || name.is_empty() {
            return;
        }
        let Some(view) = self.view(cx) else { return };
        let (chat_id, approval_id, project_id) = (
            self.chat_id.clone(),
            self.approval_id.clone(),
            self.project_id.clone(),
        );
        self.state.update(cx, |s, cx| {
            s.secret_handoff_save(
                chat_id,
                approval_id,
                project_id,
                name,
                value,
                view.purpose,
                cx,
            )
        });
        // Hidden again while it saves (Try again re-sends it; the field empties on success).
        self.show = false;
        self.value_input
            .update(cx, |input, _| input.set_masked(true));
        cx.notify();
    }

    fn toggle_error_details(&mut self, cx: &mut Context<Self>) {
        let id = self.approval_id.clone();
        self.state.update(cx, |s, cx| {
            if let Some(HandoffState::Failed(slot)) = s.handoff.get_mut(&id) {
                slot.toggle_details();
            }
            cx.notify();
        });
    }

    fn field(
        &self,
        id: &str,
        label: &str,
        input: &Entity<ComposerInput>,
        trailing: Option<gpui::AnyElement>,
        focused: bool,
        look: &CardLook,
    ) -> Stateful<Div> {
        let (palette, theme) = (&look.palette, look.theme);
        div()
            .id(SharedString::from(format!("{}-{id}", self.approval_id)))
            .w_full()
            .flex()
            .flex_col()
            .gap(px(1.0))
            .min_h(px(ROW_HEIGHT + 10.0))
            .px(px(ROW_PAD_X))
            .py(px(6.0))
            .bg(palette.inner)
            .when(focused, |d| {
                d.rounded(px(9.0)).border_1().border_color(theme.accent)
            })
            .child(
                div()
                    .text_size(px(TEXT_TINY))
                    .text_color(theme.text_muted)
                    .child(label.to_owned()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().flex_1().min_w_0().child(input.clone()))
                    .children(trailing),
            )
    }

    fn render_open(&mut self, look: &CardLook, cx: &mut Context<Self>) -> Div {
        let (palette, theme) = (&look.palette, look.theme);
        let has_value = !self.value_input.read(cx).text().trim().is_empty();
        let has_name = !self.name(cx).is_empty();
        let show_toggle = div()
            .id(SharedString::from(format!("{}-show", self.approval_id)))
            .cursor_pointer()
            .text_size(px(TEXT_SMALL))
            .text_color(theme.text_muted)
            .child(if self.show { "Hide" } else { "Show" })
            .on_click(cx.listener(|this, _, _, cx| {
                this.show = !this.show;
                let masked = !this.show;
                this.value_input
                    .update(cx, |input, _| input.set_masked(masked));
                cx.notify();
            }))
            .into_any_element();
        let saving = matches!(
            self.state.read(cx).handoff.get(&self.approval_id),
            Some(HandoffState::Saving)
        );
        let can_save = has_value && has_name && !saving;
        let save_row = option_row(
            format!("{}-save", self.approval_id),
            None,
            if saving { "Saving…" } else { "Save to vault" },
            if can_save {
                RowState::Open
            } else {
                RowState::Faded
            },
            true,
            look,
        )
        .font_weight(FontWeight::SEMIBOLD)
        .debug_selector(|| "secret-offer-save".into())
        .when(can_save, |r| {
            r.on_click(cx.listener(|this, _, _, cx| this.save(cx)))
        });
        option_list(palette)
            .child(self.field("name", "Name", &self.name_input.clone(), None, false, look))
            .child(self.field(
                "value",
                "Value",
                &self.value_input.clone(),
                has_value.then_some(show_toggle),
                false,
                look,
            ))
            .child(save_row)
    }

    fn render_settled(&self, view: &View, look: &CardLook) -> Div {
        let saved = matches!(view.state, ApprovalState::Approved);
        option_list(&look.palette).child(option_row(
            format!("{}-done", self.approval_id),
            saved.then(|| "✓".to_owned()),
            if saved {
                "Saved to your vault"
            } else {
                "Not saved"
            },
            if saved {
                RowState::Chosen
            } else {
                RowState::Faded
            },
            false,
            look,
        ))
    }
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned()
}

impl Focusable for SecretOfferCard {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SecretOfferCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let Some(view) = self.view(cx) else {
            return div().into_any_element();
        };
        if self.dismissed {
            return div().into_any_element();
        }
        let look = CardLook::of(&theme);
        let palette = look.palette;
        let device = self.state.read(cx).handoff.get(&self.approval_id).cloned();
        // Saved on this device counts at once; the part flips when the box settles the card.
        let saved_here = matches!(device, Some(HandoffState::Saved(_)));
        let open = view.state == ApprovalState::Pending && !saved_here;
        if open && !self.focused_once {
            self.focused_once = true;
            if self.name_input.read(cx).text().trim().is_empty() {
                let h = self.name_input.read(cx).focus_handle(cx);
                window.focus(&h, cx);
            } else {
                let h = self.value_input.read(cx).focus_handle(cx);
                window.focus(&h, cx);
            }
        }
        let name = if saved_here {
            match &device {
                Some(HandoffState::Saved(env)) => env.clone(),
                _ => view.suggested.clone(),
            }
        } else {
            self.name(cx)
        };
        let agent = if view.agent.is_empty() {
            "the agent".to_owned()
        } else {
            view.agent.clone()
        };
        let title = if open {
            format!("Give {agent} a secret")
        } else if saved_here || view.state == ApprovalState::Approved {
            format!("{name} is in your vault")
        } else {
            "Secret not saved".to_owned()
        };
        let on_close: Option<CloseHandler> = open.then(|| {
            let (state, chat_id, approval_id) = (
                self.state.clone(),
                self.chat_id.clone(),
                self.approval_id.clone(),
            );
            Box::new(cx.listener(
                move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx| {
                    this.dismissed = true;
                    state.update(cx, |s, cx| {
                        s.reject(chat_id.clone(), approval_id.clone(), None, cx)
                    });
                    cx.notify();
                },
            )) as CloseHandler
        });
        let body = if open {
            self.render_open(&look, cx)
        } else {
            let settled = View {
                state: if saved_here {
                    ApprovalState::Approved
                } else {
                    view.state
                },
                ..view
            };
            self.render_settled(&settled, &look)
        };
        if saved_here && !self.value_input.read(cx).text().is_empty() {
            // Saved: the local field forgets the value.
            self.value_input.update(cx, |input, cx| input.clear(cx));
        }
        let error = match &device {
            Some(HandoffState::Failed(slot)) if open => Some(slot.clone()),
            _ => None,
        };
        card_frame(format!("secret-offer-{}", self.approval_id), &palette)
            .debug_selector(|| "secret-offer-card".into())
            .child(card_header(
                format!("secret-offer-{}-x", self.approval_id),
                &title,
                on_close,
                &palette,
                &theme,
            ))
            .when(!open && (saved_here || view.state == ApprovalState::Approved), |c| {
                c.child(
                    div()
                        .text_size(px(TEXT_SMALL))
                        .text_color(theme.text_muted)
                        .child(format!(
                            "{agent} can use it. Where it may be used is set the first time it is used. Manage it in Settings → Secrets."
                        )),
                )
            })
            .key_context("SecretOfferCard")
            .track_focus(&self.focus_handle)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .child(body)
            .when(open, |el| {
                el.child(
                    div()
                        .px(px(1.0))
                        .text_size(px(TEXT_TINY))
                        .text_color(palette.badge_ink)
                        .child(format!(
                            "Sent encrypted from this device to your vault. {agent} gets only the name — never the value."
                        )),
                )
            })
            // The shared error note (ARB-416): sentence, Try again, Details.
            .when_some(error, |el, slot| {
                el.child(error_note::render(
                    &format!("secret-offer-{}-error", self.approval_id),
                    &slot.error,
                    slot.details_open,
                    error_note::Style::Inline,
                    &theme,
                    error_note::Handlers::new()
                        .try_again(cx.listener(|this, _, _, cx| this.save(cx)))
                        .toggle_details(cx.listener(|this, _, _, cx| this.toggle_error_details(cx))),
                ))
            })
            .into_any_element()
    }
}
