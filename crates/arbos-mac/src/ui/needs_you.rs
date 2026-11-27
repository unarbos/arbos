//! "Needs you": every card still waiting on me, across the chats this Mac holds, in one
//! sheet. Newest first; each row names the agent and chat, says how long ago, and carries
//! the chat's own card (`approval_card`, `SecretOfferCard`, `QuestionCard`), so an answer
//! here takes the same path as an answer in the chat. The row's title opens the chat at
//! the card. A plain question has no card (it is answered from the composer): its row
//! shows the question and its title opens the chat.
//!
//! Keys: ↑ ↓ move the highlight, ⏎ opens the highlighted row's chat, Esc closes (the
//! shell's). A card takes its own keys once it is clicked, as in the chat.

use std::collections::HashMap;

use gpui::{
    div, prelude::*, px, AnyElement, App, Context, Entity, FocusHandle, Focusable, FontWeight,
    KeyDownEvent, ScrollHandle, Window,
};

use crate::app::AppState;
use crate::model::{ago, ApprovalState, Id, Message, Part, QuestionState, Role, Widget};
use crate::ui::approve::{approval_card, ApprovalView};
use crate::ui::highlight::{self, Shape};
use crate::ui::modal::modal_overlay;
use crate::ui::secret_card::SecretOfferCard;
use crate::ui::theme::{Theme, PANEL_RADIUS, TEXT_SMALL, TEXT_TINY};
use crate::ui::widgets::QuestionCard;

/// What a row is waiting for.
#[derive(Clone, Debug, PartialEq)]
pub enum What {
    /// An approval card: approve / reject, a secret request's answers, or a secret hand-off.
    Approval(ApprovalView),
    /// A typed question card.
    Question { question_id: String, widget: Widget },
    /// A plain question, answered from the chat's composer.
    Plain { question: String },
}

/// One waiting card.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub chat_id: Id,
    pub message_id: Id,
    /// When the message carrying the card was created (ms).
    pub at: u64,
    pub what: What,
}

/// Everything waiting on me in `messages` (chat id → rows), newest first.
///
/// Deliberately simple: a scan of every part of every held chat per call. Fine for the
/// chats one Mac holds; keep a count per chat on message sync if it ever shows in a profile.
pub fn waiting(messages: &HashMap<Id, Vec<Message>>) -> Vec<Item> {
    let mut out = Vec::new();
    for (chat_id, list) in messages {
        for (ix, m) in list.iter().enumerate() {
            // A plain question is open only while it is the chat's last word
            // (`AppState::chat_needs_answer`); an older one was talked past.
            let last = ix + 1 == list.len() && m.role == Role::Assistant && !m.is_streaming();
            for p in &m.parts {
                let what = match p {
                    Part::Approval {
                        approval_id,
                        title,
                        summary,
                        risk,
                        payload,
                        state: ApprovalState::Pending,
                        decided_by,
                        reason,
                        grant,
                        ..
                    } => What::Approval(ApprovalView {
                        approval_id: approval_id.clone(),
                        title: title.clone(),
                        summary: summary.clone(),
                        risk: *risk,
                        state: ApprovalState::Pending,
                        payload: payload.clone(),
                        decided_by: decided_by.clone(),
                        reason: reason.clone(),
                        grant: grant.clone(),
                        agent: None,
                    }),
                    Part::Question {
                        question_id,
                        widget: Some(widget),
                        state: QuestionState::Open,
                        ..
                    } => What::Question {
                        question_id: question_id.clone(),
                        widget: widget.clone(),
                    },
                    Part::Question {
                        question,
                        widget: None,
                        state: QuestionState::Open,
                        ..
                    } if last => What::Plain {
                        question: question.clone(),
                    },
                    _ => continue,
                };
                out.push(Item {
                    chat_id: chat_id.clone(),
                    message_id: m.id.clone(),
                    at: m.created_at,
                    what,
                });
            }
        }
    }
    // The map has no order of its own: ties fall back to the ids so rows never swap.
    out.sort_by(|a, b| {
        b.at.cmp(&a.at)
            .then_with(|| (&a.chat_id, &a.message_id).cmp(&(&b.chat_id, &b.message_id)))
    });
    out
}

/// "Arbos · Refunds": the agent (or person, or group) and the chat's title.
fn who(state: &AppState, chat_id: &str) -> String {
    let space = state.project_of_chat(chat_id).map(|p| p.name.clone());
    let title = state
        .chats
        .values()
        .chain(state.job_tabs.values())
        .flatten()
        .find(|c| c.id == chat_id)
        .map(|c| c.title.trim().to_owned())
        .filter(|t| !t.is_empty());
    match (space, title) {
        (Some(space), Some(title)) if space != title => format!("{space} · {title}"),
        (Some(name), _) | (None, Some(name)) => name,
        (None, None) => "A chat".to_owned(),
    }
}

pub struct NeedsYou {
    state: Entity<AppState>,
    open: bool,
    /// The row ↑ ↓ highlight and ⏎ opens.
    cursor: usize,
    scroll: ScrollHandle,
    focus_handle: FocusHandle,
    questions: HashMap<String, Entity<QuestionCard>>,
    secrets: HashMap<String, Entity<SecretOfferCard>>,
}

impl NeedsYou {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |this, _, cx| {
            if this.open {
                cx.notify();
            }
        })
        .detach();
        Self {
            state,
            open: false,
            cursor: 0,
            scroll: ScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            questions: HashMap::new(),
            secrets: HashMap::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.cursor = 0;
        window.focus(&self.focus_handle, cx);
        // A stateful card takes the keyboard the first time it draws (its rule in the
        // chat); in a list the sheet keeps it, so ↑ ↓ ⏎ work until a card is clicked.
        let focus = self.focus_handle.clone();
        window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.questions.clear();
        self.secrets.clear();
        cx.notify();
    }

    /// The row's title: the chat, scrolled to the message carrying the card.
    fn jump(&mut self, item: &Item, cx: &mut Context<Self>) {
        let (chat_id, message_id) = (item.chat_id.clone(), item.message_id.clone());
        self.state.update(cx, |s, cx| {
            let space_id = s.space_of_chat(&chat_id);
            s.open_palette_target(
                crate::palette::Target::Message {
                    space_id,
                    chat_id,
                    message_id,
                },
                cx,
            );
        });
        self.close(cx);
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let items = waiting(&self.state.read(cx).messages);
        match event.keystroke.key.as_str() {
            "down" => self.cursor = (self.cursor + 1).min(items.len().saturating_sub(1)),
            "up" => self.cursor = self.cursor.saturating_sub(1),
            "enter" => {
                if let Some(item) = items.get(self.cursor) {
                    self.jump(item, cx);
                }
            }
            _ => return,
        }
        self.scroll.scroll_to_item(self.cursor);
        cx.stop_propagation();
        cx.notify();
    }

    /// The chat's own card for `item`, wired to the chat's own actions.
    fn card(
        &mut self,
        ix: usize,
        item: &Item,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chat_id = item.chat_id.clone();
        let state = self.state.clone();
        match &item.what {
            What::Plain { question } => div()
                .px(px(10.0))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text)
                .child(question.clone())
                .child(
                    div()
                        .text_size(px(TEXT_TINY))
                        .text_color(theme.text_faint)
                        .child("Answer in the chat"),
                )
                .into_any_element(),
            What::Question {
                question_id,
                widget,
            } => self
                .questions
                .entry(question_id.clone())
                .or_insert_with(|| {
                    cx.new(|cx| QuestionCard::new(state, chat_id, question_id.clone(), widget, cx))
                })
                .clone()
                .into_any_element(),
            What::Approval(view) => {
                let project = self.state.read(cx).project_of_chat(&chat_id);
                let (project_id, agent) = project
                    .map(|p| (p.id.clone(), p.name.clone()))
                    .unwrap_or_default();
                let field = |key: &str| view.payload.get(key).and_then(serde_json::Value::as_str);
                let approval_id = view.approval_id.clone();
                // The hand-off card is stateful (its masked field), as in the transcript.
                if field("kind") == Some("secret_offer") {
                    let project_id = field("projectId").map_or(project_id, str::to_owned);
                    let suggested = field("name").unwrap_or("");
                    return self
                        .secrets
                        .entry(approval_id.clone())
                        .or_insert_with(|| {
                            cx.new(|cx| {
                                SecretOfferCard::new(
                                    state,
                                    chat_id,
                                    approval_id,
                                    project_id,
                                    suggested,
                                    cx,
                                )
                            })
                        })
                        .clone()
                        .into_any_element();
                }
                let mut view = view.clone();
                view.agent = Some(agent);
                let (state_a, chat_a, id_a) = (state.clone(), chat_id.clone(), approval_id.clone());
                let (state_r, chat_r, id_r) = (state.clone(), chat_id.clone(), approval_id.clone());
                approval_card(
                    &format!("needs-you-card-{ix}"),
                    &view,
                    theme,
                    move |cx| {
                        state_a.update(cx, |s, cx| s.approve(chat_a.clone(), id_a.clone(), cx));
                    },
                    move |cx| {
                        state_r
                            .update(cx, |s, cx| s.reject(chat_r.clone(), id_r.clone(), None, cx));
                    },
                    move |choice, cx| {
                        state.update(cx, |s, cx| {
                            s.answer_approval(chat_id.clone(), approval_id.clone(), choice, cx)
                        });
                    },
                )
                .into_any_element()
            }
        }
    }
}

impl Focusable for NeedsYou {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for NeedsYou {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let items = waiting(&self.state.read(cx).messages);
        self.cursor = self.cursor.min(items.len().saturating_sub(1));

        let mut list = div()
            .id("needs-you-list")
            .flex()
            .flex_col()
            .gap(px(10.0))
            .max_h(px(520.0))
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        if items.is_empty() {
            list = list.child(
                div()
                    .py(px(24.0))
                    .text_center()
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_muted)
                    .debug_selector(|| "needs-you-empty".into())
                    .child("Nothing is waiting on you"),
            );
        }
        for (ix, item) in items.iter().enumerate() {
            let who = who(self.state.read(cx), &item.chat_id);
            let target = item.clone();
            let title = div()
                .id(("needs-you-title", ix))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .h(px(28.0))
                .px(px(10.0))
                .rounded(px(PANEL_RADIUS - 4.0))
                .text_size(px(TEXT_SMALL))
                .cursor_pointer()
                .map(|el| highlight::row(el, ix == self.cursor, Shape::Own, &theme))
                .debug_selector(move || format!("needs-you-title-{ix}"))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(who),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(TEXT_TINY))
                        .text_color(theme.text_faint)
                        .child(ago(item.at as i64)),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.jump(&target, cx)));
            let card = self.card(ix, item, &theme, cx);
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .child(title)
                    .child(card),
            );
        }

        let card = div()
            .w(px(560.0))
            .max_w_full()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(18.0))
            .rounded(px(PANEL_RADIUS))
            .bg(theme.card())
            .border_1()
            .border_color(theme.border_strong)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(15.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child("Needs you"),
                    )
                    .when(!items.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.text_faint)
                                .child("↑↓ · ⏎ open chat"),
                        )
                    }),
            )
            .child(list);

        let weak = cx.weak_entity();
        div()
            .absolute()
            .inset_0()
            .key_context("NeedsYou")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key))
            .child(modal_overlay(
                "needs-you",
                move |_, cx| {
                    let _ = weak.update(cx, |this, cx| this.close(cx));
                },
                card,
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(id: &str, at: u64, parts: serde_json::Value) -> Message {
        serde_json::from_value(json!({
            "id": id, "chatId": "c", "role": "assistant", "sender": "agent", "status": "done",
            "createdAt": at, "parts": parts,
        }))
        .expect("message")
    }

    fn approval(state: &str) -> serde_json::Value {
        json!({ "id": "p", "kind": "approval", "approvalId": "ap", "title": "Send it",
                "state": state, "payload": {} })
    }

    fn question(state: &str, typed: bool) -> serde_json::Value {
        let mut q = json!({ "id": "q", "kind": "question", "questionId": "q1",
                            "question": "Which one?", "state": state });
        if typed {
            q["widget"] = json!({ "type": "choice", "options": [{ "id": "a", "label": "A" }] });
        }
        q
    }

    /// Pending approvals, open typed questions and a chat's last plain question are listed,
    /// newest first across chats; decided, answered and talked-past ones are not.
    #[test]
    fn waiting_lists_only_open_cards_newest_first() {
        let mut messages = HashMap::new();
        messages.insert(
            "c1".to_string(),
            vec![
                message("old-approval", 10, json!([approval("pending")])),
                message("decided", 50, json!([approval("approved")])),
                message("talked-past", 60, json!([question("open", false)])),
                message("answered", 70, json!([question("answered", true)])),
                message("last-plain", 80, json!([question("open", false)])),
            ],
        );
        messages.insert(
            "c2".to_string(),
            vec![
                message("typed", 40, json!([question("open", true)])),
                message("expired", 90, json!([approval("expired")])),
            ],
        );
        let got = waiting(&messages);
        let ids: Vec<(&str, &str)> = got
            .iter()
            .map(|i| (i.chat_id.as_str(), i.message_id.as_str()))
            .collect();
        assert_eq!(
            ids,
            [
                ("c1", "last-plain"),
                ("c2", "typed"),
                ("c1", "old-approval")
            ]
        );
        assert!(matches!(got[0].what, What::Plain { .. }));
        assert!(matches!(got[1].what, What::Question { .. }));
        assert!(matches!(got[2].what, What::Approval(_)));

        // Answering one takes it off the list.
        messages.get_mut("c1").expect("c1")[0] =
            message("old-approval", 10, json!([approval("rejected")]));
        assert_eq!(waiting(&messages).len(), 2);
    }
}

#[cfg(test)]
mod draw_tests {
    //! The sheet over a real `AppState` in a headless window: it lists a card from each of
    //! two chats, ↓ ⏎ opens the highlighted row's chat, an answered card leaves the list,
    //! and an empty list says so.
    use futures::future::BoxFuture;
    use gpui::TestAppContext;
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;
    use crate::model::{Chat, Project};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::motion::PulseClock;

    struct Idle;

    impl SyncSource for Idle {
        fn name(&self) -> &'static str {
            "idle"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            _commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            Box::pin(std::future::pending())
        }
    }

    struct Host(Entity<NeedsYou>);

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.0.clone())
        }
    }

    fn row(chat: &str, id: &str, at: u64, part: serde_json::Value) -> Message {
        serde_json::from_value(json!({
            "id": id, "chatId": chat, "role": "assistant", "sender": "agent", "status": "done",
            "createdAt": at, "parts": [part],
        }))
        .expect("message")
    }

    #[gpui::test]
    fn the_sheet_lists_opens_and_empties(cx: &mut TestAppContext) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            PulseClock::init(cx);
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        let t = 1_700_000_000_000u64;
        state.update(cx, |s, _| {
            s.projects.push(Project {
                id: "p1".into(),
                name: "Arbos".into(),
                ..Project::default()
            });
            let chat = |id: &str, title: &str| -> Chat {
                serde_json::from_value(json!({
                    "id": id, "project_id": "p1", "title": title, "is_pinned": false,
                    "is_system": false, "unread": false, "last_message_at": t,
                }))
                .expect("chat")
            };
            s.chats
                .insert("p1".into(), vec![chat("c1", "Mail"), chat("c2", "Trip")]);
            s.messages.insert(
                "c1".into(),
                vec![row(
                    "c1",
                    "m-approval",
                    t,
                    json!({ "id": "p", "kind": "approval", "approvalId": "ap", "title": "Send it",
                            "state": "pending", "payload": {} }),
                )],
            );
            s.messages.insert(
                "c2".into(),
                vec![row(
                    "c2",
                    "m-question",
                    t + 1_000,
                    json!({ "id": "q", "kind": "question", "questionId": "q1",
                            "question": "Which one?", "state": "open",
                            "widget": { "type": "choice", "options": [{ "id": "a", "label": "A" }] } }),
                )],
            );
            s.selected_project = Some("p1".into());
            s.active_tab.insert("p1".into(), "c2".into());
        });
        let sheet = cx.new(|cx| NeedsYou::new(state.clone(), cx));
        let (_host, vcx) = cx.add_window_view(|_, _| Host(sheet.clone()));
        sheet.update_in(vcx, |s, window, cx| s.open(window, cx));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("needs-you-title-0").is_some());
        assert!(vcx.debug_bounds("needs-you-title-1").is_some());
        assert!(vcx.debug_bounds("needs-you-empty").is_none());

        // The older card (the approval in "Mail") is the second row: ↓ ⏎ opens its chat.
        vcx.simulate_keystrokes("down enter");
        vcx.run_until_parked();
        assert!(!sheet.read_with(vcx, |s, _| s.is_open()));
        state.read_with(vcx, |s, _| {
            assert_eq!(s.active_tab.get("p1").map(String::as_str), Some("c1"));
        });

        // Answered through the card's own path, the question leaves the list.
        sheet.update_in(vcx, |s, window, cx| s.open(window, cx));
        state.update(vcx, |s, cx| {
            s.answer_question("c2".into(), "q1".into(), json!("a"), cx)
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("needs-you-title-0").is_some());
        assert!(vcx.debug_bounds("needs-you-title-1").is_none());

        // The box settles the approval: nothing is left, and the sheet says so.
        state.update(vcx, |s, cx| {
            s.messages.insert(
                "c1".into(),
                vec![row(
                    "c1",
                    "m-approval",
                    t,
                    json!({ "id": "p", "kind": "approval", "approvalId": "ap", "title": "Send it",
                            "state": "approved", "payload": {} }),
                )],
            );
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("needs-you-empty").is_some());
    }
}
