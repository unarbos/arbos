//! Question cards (design §10 "question widgets"): one [`QuestionCard`] view
//! per `question` part. Five widget types — choice, multi-select, text, date,
//! form — each answered from the card with the mouse or the keyboard (Enter
//! submits, arrows move, Space toggles). An answered card stays in the
//! transcript with its value.
//!
//! The look is Grok Bot's question card and nothing else: one rounded card in
//! the assistant-bubble grey, a short bold question, a thin grey × top-right,
//! and a white (dark: slightly lighter) inner list of rows, each with a small
//! grey lettered badge. No icons, chips, coloured buttons or hints. The frame
//! helpers at the bottom ([`card_frame`], [`card_header`], [`option_list`],
//! [`option_row`]) are shared with the approval card.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use gpui::{
    actions, div, prelude::*, px, App, Context, Div, Entity, FocusHandle, Focusable, FontWeight,
    Hsla, Render, SharedString, Stateful, Window,
};
use serde_json::{json, Value};

use arbos_core::widget::{add_days, check_date, today_iso};

use crate::app::AppState;
use crate::model::{ChoiceOption, FieldKind, FormField, Id, Part, QuestionState, Widget};
use crate::ui::composer::ComposerInput;
use crate::ui::error_note;
use crate::ui::icons::icon;
use crate::ui::theme::{Appearance, Theme, TEXT_SMALL};

actions!(
    widgets,
    [SubmitAnswer, NextOption, PrevOption, ToggleOption]
);

/// Live snapshot of the part this card renders.
#[derive(Clone, Debug, PartialEq)]
struct PartView {
    question: String,
    widget: Widget,
    answer: Option<Value>,
    state: QuestionState,
}

pub struct QuestionCard {
    state: Entity<AppState>,
    chat_id: Id,
    question_id: String,
    focus_handle: FocusHandle,
    /// Keyboard cursor over the options (choice / multi-select).
    highlighted: usize,
    selected: BTreeSet<String>,
    text_input: Option<Entity<ComposerInput>>,
    form_inputs: BTreeMap<String, Entity<ComposerInput>>,
    form_choice: BTreeMap<String, String>,
    form_bool: BTreeMap<String, bool>,
    error: Option<String>,
    focused_once: bool,
    /// × was clicked: the card hides for this session (the question stays open on the box;
    /// the composer still answers it).
    dismissed: bool,
}

impl QuestionCard {
    pub fn new(
        state: Entity<AppState>,
        chat_id: Id,
        question_id: String,
        widget: &Widget,
        cx: &mut Context<Self>,
    ) -> Self {
        let weak = cx.weak_entity();
        let submit_from_input = move |_: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |card, cx| card.submit(cx));
        };
        let make_input = |placeholder: &str, cx: &mut Context<Self>| {
            let input = cx.new(ComposerInput::new);
            let placeholder = placeholder.to_string();
            let on_submit = submit_from_input.clone();
            input.update(cx, |input, _| {
                input.set_placeholder(placeholder);
                input.on_submit = Some(Rc::new(on_submit));
            });
            cx.observe(&input, |_, _, cx| cx.notify()).detach();
            input
        };
        let mut text_input = None;
        let mut form_inputs = BTreeMap::new();
        let mut form_bool = BTreeMap::new();
        match widget {
            Widget::Choice { .. } | Widget::MultiSelect { .. } => {}
            Widget::Text { placeholder, .. } => {
                text_input = Some(make_input(
                    placeholder.as_deref().unwrap_or("Type your answer…"),
                    cx,
                ));
            }
            Widget::Date { .. } => {
                text_input = Some(make_input("yyyy-mm-dd", cx));
            }
            Widget::Form { fields } => {
                for f in fields {
                    match f.kind {
                        FieldKind::Text => {
                            form_inputs.insert(f.id.clone(), make_input(&f.label, cx));
                        }
                        FieldKind::Number => {
                            form_inputs.insert(f.id.clone(), make_input("0", cx));
                        }
                        FieldKind::Date => {
                            form_inputs.insert(f.id.clone(), make_input("yyyy-mm-dd", cx));
                        }
                        FieldKind::Bool => {
                            form_bool.insert(f.id.clone(), false);
                        }
                        FieldKind::Choice => {}
                    }
                }
            }
        }
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            chat_id,
            question_id,
            focus_handle: cx.focus_handle(),
            highlighted: 0,
            selected: BTreeSet::new(),
            text_input,
            form_inputs,
            form_choice: BTreeMap::new(),
            form_bool,
            error: None,
            focused_once: false,
            dismissed: false,
        }
    }

    fn part(&self, cx: &App) -> Option<PartView> {
        let state = self.state.read(cx);
        state.messages_of(&self.chat_id).iter().find_map(|m| {
            m.parts.iter().find_map(|p| match p {
                Part::Question {
                    question_id,
                    question,
                    widget: Some(widget),
                    answer,
                    state,
                    ..
                } if *question_id == self.question_id => Some(PartView {
                    question: question.clone(),
                    widget: widget.clone(),
                    answer: answer.clone(),
                    state: *state,
                }),
                _ => None,
            })
        })
    }

    fn options_len(&self, cx: &App) -> usize {
        match self.part(cx).map(|p| p.widget) {
            Some(Widget::Choice { options }) | Some(Widget::MultiSelect { options }) => {
                options.len()
            }
            Some(Widget::Text { .. })
            | Some(Widget::Date { .. })
            | Some(Widget::Form { .. })
            | None => 0,
        }
    }

    fn next(&mut self, _: &NextOption, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.options_len(cx);
        if n > 0 {
            self.highlighted = (self.highlighted + 1) % n;
            cx.notify();
        }
    }

    fn prev(&mut self, _: &PrevOption, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.options_len(cx);
        if n > 0 {
            self.highlighted = (self.highlighted + n - 1) % n;
            cx.notify();
        }
    }

    fn toggle(&mut self, _: &ToggleOption, _: &mut Window, cx: &mut Context<Self>) {
        let Some(part) = self.part(cx) else {
            return;
        };
        if let Widget::MultiSelect { options } = &part.widget {
            if let Some(o) = options.get(self.highlighted) {
                self.toggle_id(o.id.clone(), cx);
            }
        }
    }

    fn toggle_id(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.selected.remove(&id) {
            self.selected.insert(id);
        }
        self.error = None;
        cx.notify();
    }

    fn submit_action(&mut self, _: &SubmitAnswer, _: &mut Window, cx: &mut Context<Self>) {
        let Some(part) = self.part(cx) else {
            return;
        };
        // Enter on a choice card picks the highlighted option.
        if let Widget::Choice { options } = &part.widget {
            if let Some(o) = options.get(self.highlighted) {
                self.answer(json!(o.id), cx);
            }
            return;
        }
        self.submit(cx);
    }

    /// Collect the draft into a value, validate it, and send it.
    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(part) = self.part(cx) else {
            return;
        };
        if part.state == QuestionState::Answered {
            return;
        }
        let value = match &part.widget {
            Widget::Choice { options } => match options.get(self.highlighted) {
                Some(o) => Ok(json!(o.id)),
                None => Err("Pick one".to_string()),
            },
            Widget::MultiSelect { options } => {
                let picked: Vec<&str> = options
                    .iter()
                    .filter(|o| self.selected.contains(&o.id))
                    .map(|o| o.id.as_str())
                    .collect();
                if picked.is_empty() {
                    Err("Pick at least one".to_string())
                } else {
                    Ok(json!(picked))
                }
            }
            Widget::Text { .. } => {
                let text = self.input_text(&self.text_input, cx);
                if text.is_empty() {
                    Err("Type an answer".to_string())
                } else {
                    Ok(json!(text))
                }
            }
            Widget::Date { min, max } => {
                let text = self.input_text(&self.text_input, cx);
                check_date(&text, min.as_deref(), max.as_deref()).map(|d| json!(d))
            }
            Widget::Form { fields } => self.collect_form(fields, cx),
        };
        match value {
            Ok(v) => self.answer(v, cx),
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    fn collect_form(&self, fields: &[FormField], cx: &App) -> Result<Value, String> {
        let mut out = serde_json::Map::new();
        for f in fields {
            let v = match f.kind {
                FieldKind::Text => {
                    let t = self.input_text(&self.form_inputs.get(&f.id).cloned(), cx);
                    (!t.is_empty()).then(|| json!(t))
                }
                FieldKind::Number => {
                    let t = self.input_text(&self.form_inputs.get(&f.id).cloned(), cx);
                    if t.is_empty() {
                        None
                    } else {
                        if let Ok(i) = t.parse::<i64>() {
                            Some(json!(i))
                        } else {
                            let n: f64 = t
                                .parse()
                                .map_err(|_| format!("{}: not a number", f.label))?;
                            Some(json!(n))
                        }
                    }
                }
                FieldKind::Date => {
                    let t = self.input_text(&self.form_inputs.get(&f.id).cloned(), cx);
                    if t.is_empty() {
                        None
                    } else {
                        Some(json!(
                            check_date(&t, None, None).map_err(|e| format!("{}: {e}", f.label))?
                        ))
                    }
                }
                FieldKind::Choice => self.form_choice.get(&f.id).map(|id| json!(id)),
                FieldKind::Bool => Some(json!(self.form_bool.get(&f.id).copied().unwrap_or(false))),
            };
            match v {
                Some(v) => {
                    out.insert(f.id.clone(), v);
                }
                None if f.required => return Err(format!("{} is required", f.label)),
                None => {}
            }
        }
        Ok(Value::Object(out))
    }

    fn input_text(&self, input: &Option<Entity<ComposerInput>>, cx: &App) -> String {
        input
            .as_ref()
            .map(|i| i.read(cx).text().trim().to_string())
            .unwrap_or_default()
    }

    fn answer(&mut self, value: Value, cx: &mut Context<Self>) {
        self.error = None;
        let chat_id = self.chat_id.clone();
        let qid = self.question_id.clone();
        self.state
            .update(cx, |s, cx| s.answer_question(chat_id, qid, value, cx));
        cx.notify();
    }

    fn set_date(&mut self, iso: String, cx: &mut Context<Self>) {
        if let Some(input) = &self.text_input {
            input.update(cx, |i, cx| i.set_text(iso, cx));
        }
        self.error = None;
        cx.notify();
    }

    // ---- rendering ----------------------------------------------------------------------------

    fn render_rows(
        &self,
        options: &[ChoiceOption],
        multi: bool,
        picked: &BTreeSet<String>,
        open: bool,
        look: &CardLook,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut list = option_list(&look.palette);
        for (ix, o) in options.iter().enumerate() {
            let selected = picked.contains(&o.id);
            let state = if selected {
                RowState::Chosen
            } else if !open {
                RowState::Faded
            } else if ix == self.highlighted {
                RowState::Highlighted
            } else {
                RowState::Open
            };
            let id = o.id.clone();
            let row = option_row(
                format!("{}-opt-{}", self.question_id, o.id),
                Some(letter(ix)),
                &o.label,
                state,
                ix > 0,
                look,
            )
            .when(open, |el| {
                el.on_click(cx.listener(move |this, _, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    this.highlighted = ix;
                    if multi {
                        this.toggle_id(id.clone(), cx);
                    } else {
                        this.answer(json!(id), cx);
                    }
                }))
            });
            list = list.child(row);
        }
        list
    }

    fn render_input_box(&self, input: &Entity<ComposerInput>, palette: &CardPalette) -> Div {
        div()
            .flex()
            .w_full()
            .min_h(px(ROW_HEIGHT))
            .items_center()
            .px(px(ROW_PAD_X))
            .py(px(6.0))
            .bg(palette.inner)
            .child(input.clone())
    }

    /// The last row of a card that needs a Send: no badge, the label in the body weight.
    fn send_row(&self, look: &CardLook, cx: &mut Context<Self>) -> Stateful<Div> {
        option_row(
            format!("{}-submit", self.question_id),
            None,
            "Send",
            RowState::Open,
            true,
            look,
        )
        .font_weight(FontWeight::SEMIBOLD)
        .on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
    }

    fn render_answered(&self, view: &PartView, look: &CardLook, cx: &mut Context<Self>) -> Div {
        let (palette, theme) = (&look.palette, look.theme);
        let answer = view.answer.clone().unwrap_or(Value::Null);
        match &view.widget {
            Widget::Choice { options } | Widget::MultiSelect { options } => {
                let picked: BTreeSet<String> = match &answer {
                    Value::Array(a) => a
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                    Value::String(s) => BTreeSet::from([s.clone()]),
                    _ => BTreeSet::new(),
                };
                self.render_rows(options, false, &picked, false, look, cx)
            }
            Widget::Text { .. } | Widget::Date { .. } | Widget::Form { .. } => {
                let text = view.widget.answer_text(&answer);
                let text = text
                    .strip_prefix("Answer:")
                    .unwrap_or(&text)
                    .trim()
                    .to_string();
                option_list(palette).child(
                    div()
                        .min_h(px(ROW_HEIGHT))
                        .px(px(ROW_PAD_X + 2.0))
                        .py(px(8.0))
                        .text_size(px(ROW_TEXT))
                        .line_height(px(ROW_TEXT + 5.0))
                        .text_color(theme.text)
                        .child(text),
                )
            }
        }
    }

    fn render_open(&self, view: &PartView, look: &CardLook, cx: &mut Context<Self>) -> Div {
        let (palette, theme) = (&look.palette, look.theme);
        match &view.widget {
            Widget::Choice { options } => {
                self.render_rows(options, false, &BTreeSet::new(), true, look, cx)
            }
            Widget::MultiSelect { options } => self
                .render_rows(options, true, &self.selected, true, look, cx)
                .child(self.send_row(look, cx)),
            Widget::Text { .. } => {
                let input = self.text_input.clone().expect("text widget has an input");
                option_list(palette)
                    .child(self.render_input_box(&input, palette))
                    .child(self.send_row(look, cx))
            }
            Widget::Date { min, max } => {
                let input = self.text_input.clone().expect("date widget has an input");
                let today = today_iso();
                let quick: Vec<(&str, Option<String>)> = vec![
                    ("Today", Some(today.clone())),
                    ("Tomorrow", add_days(&today, 1)),
                    ("In a week", add_days(&today, 7)),
                    ("In a month", add_days(&today, 30)),
                ];
                let mut list = option_list(palette);
                let mut ix = 0;
                for (label, iso) in quick {
                    let Some(iso) = iso else { continue };
                    let within = check_date(&iso, min.as_deref(), max.as_deref()).is_ok();
                    let row = option_row(
                        format!("{}-quick-{label}", self.question_id),
                        Some(letter(ix)),
                        label,
                        if within {
                            RowState::Open
                        } else {
                            RowState::Faded
                        },
                        ix > 0,
                        look,
                    )
                    .when(within, |el| {
                        el.on_click(cx.listener(move |this, _, _, cx| {
                            this.set_date(iso.clone(), cx);
                            this.submit(cx);
                        }))
                    });
                    list = list.child(row);
                    ix += 1;
                }
                list.child(
                    div()
                        .border_t_1()
                        .border_color(palette.hairline)
                        .child(self.render_input_box(&input, palette)),
                )
                .child(self.send_row(look, cx))
            }
            Widget::Form { fields } => {
                let mut list = option_list(palette);
                for (ix, f) in fields.iter().enumerate() {
                    let label = div()
                        .w(px(120.0))
                        .flex_shrink_0()
                        .text_size(px(ROW_TEXT))
                        .text_color(theme.text_muted)
                        .child(if f.required {
                            format!("{} *", f.label)
                        } else {
                            f.label.clone()
                        });
                    let control: Div = match f.kind {
                        FieldKind::Text | FieldKind::Number | FieldKind::Date => {
                            match self.form_inputs.get(&f.id) {
                                Some(input) => div().flex_1().child(input.clone()),
                                None => div(),
                            }
                        }
                        FieldKind::Choice => {
                            let picked: Option<&String> = self.form_choice.get(&f.id);
                            let mut row = div().flex().flex_wrap().gap(px(6.0));
                            for o in &f.options {
                                let fid = f.id.clone();
                                let oid = o.id.clone();
                                let on = picked == Some(&o.id);
                                row = row.child(
                                    pill(
                                        format!("{}-{}-{}", self.question_id, f.id, o.id),
                                        &o.label,
                                        on,
                                        palette,
                                        theme,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.form_choice.insert(fid.clone(), oid.clone());
                                            this.error = None;
                                            cx.notify();
                                        },
                                    )),
                                );
                            }
                            row
                        }
                        FieldKind::Bool => {
                            let on = self.form_bool.get(&f.id).copied().unwrap_or(false);
                            let fid = f.id.clone();
                            div().child(
                                pill(
                                    format!("{}-{}-bool", self.question_id, f.id),
                                    if on { "Yes" } else { "No" },
                                    on,
                                    palette,
                                    theme,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        let v = this.form_bool.entry(fid.clone()).or_insert(false);
                                        *v = !*v;
                                        cx.notify();
                                    },
                                )),
                            )
                        }
                    };
                    list = list.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .min_h(px(ROW_HEIGHT))
                            .px(px(ROW_PAD_X + 2.0))
                            .py(px(6.0))
                            .when(ix > 0, |el| el.border_t_1().border_color(palette.hairline))
                            .child(label)
                            .child(control),
                    );
                }
                list.child(self.send_row(look, cx))
            }
        }
    }
}

impl Focusable for QuestionCard {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for QuestionCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let Some(view) = self.part(cx) else {
            return div().into_any_element();
        };
        if self.dismissed {
            return div().into_any_element();
        }
        let look = CardLook::of(&theme);
        let palette = look.palette;
        let open = view.state == QuestionState::Open;
        // The newest open card takes the keyboard once, so Enter/arrows work at once.
        if open && !self.focused_once {
            self.focused_once = true;
            match &self.text_input {
                Some(input) => {
                    let h = input.read(cx).focus_handle(cx);
                    window.focus(&h, cx);
                }
                None => {
                    if let Some(first) = self.form_inputs.values().next() {
                        let h = first.read(cx).focus_handle(cx);
                        window.focus(&h, cx);
                    } else {
                        window.focus(&self.focus_handle, cx);
                    }
                }
            }
        }

        let body = if open {
            self.render_open(&view, &look, cx)
        } else {
            self.render_answered(&view, &look, cx)
        };

        let on_close: Option<CloseHandler> = open.then(|| {
            Box::new(cx.listener(
                |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx| {
                    this.dismissed = true;
                    cx.notify();
                },
            )) as CloseHandler
        });

        let card =
            card_frame(format!("question-{}", self.question_id), &palette).child(card_header(
                format!("question-{}-x", self.question_id),
                &view.question,
                on_close,
                &palette,
                &theme,
            ));

        card.key_context("QuestionCard")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::submit_action))
            .on_action(cx.listener(Self::next))
            .on_action(cx.listener(Self::prev))
            .on_action(cx.listener(Self::toggle))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    // The shell's root handler hands focus back to the
                    // composer on any click; a click inside a card keeps it
                    // here (the inner input took it already, or the card does).
                    cx.stop_propagation();
                    if this.text_input.is_none() && this.form_inputs.is_empty() {
                        window.focus(&this.focus_handle, cx);
                        cx.notify();
                    }
                }),
            )
            .child(body)
            // Grok's hint under an open card (measured on the real iPhone app).
            .when(open, |el| {
                el.child(
                    div()
                        .px(px(1.0))
                        .text_size(px(TEXT_SMALL))
                        .text_color(palette.badge_ink)
                        .child("Or answer in the chat below"),
                )
            })
            .when_some(self.error.clone(), |el, e| {
                // The card's note: the shared element with the validation as its sentence.
                let slot = crate::app::refused("send your answer", &e);
                el.child(error_note::render(
                    "question-card-error",
                    &slot.error,
                    false,
                    error_note::Style::Inline,
                    &theme,
                    error_note::Handlers::new(),
                ))
            })
            .into_any_element()
    }
}

// ---- the Grok Bot question card frame, shared with the approval card ------------------------

/// Outer card radius (Grok's inline cards sit at the bubble radius plus a little).
pub const CARD_RADIUS: f32 = 18.0;
/// Inner list radius.
pub const LIST_RADIUS: f32 = 11.0;
pub const CARD_PAD: f32 = 12.0;
pub const ROW_HEIGHT: f32 = 34.0;
pub const ROW_PAD_X: f32 = 9.0;
pub const ROW_TEXT: f32 = 13.5;
pub const TITLE_TEXT: f32 = 14.5;
pub const BADGE: f32 = 20.0;

/// The card's colours for the current appearance: light = the assistant-bubble grey with a
/// white list; dark (measured on the real Grok Bot app) = the #202020 card with the page
/// colour (#141414) inset as the list, card-coloured badges and separators, #5D5D60 ink.
#[derive(Clone, Copy, Debug)]
pub struct CardPalette {
    pub outer: Hsla,
    pub inner: Hsla,
    pub badge: Hsla,
    pub badge_ink: Hsla,
    pub close: Hsla,
    pub hairline: Hsla,
    pub highlight: Hsla,
}

impl CardPalette {
    pub fn of(theme: &Theme) -> Self {
        match theme.appearance {
            Appearance::Dark => Self {
                outer: theme.bubble_assistant,
                inner: theme.bg,
                badge: theme.bubble_assistant,
                badge_ink: hex(0x5d5d60),
                close: hex(0x5d5d60),
                hairline: theme.bubble_assistant,
                highlight: Theme::with_alpha(gpui::white(), 0.05),
            },
            Appearance::Light => Self {
                outer: theme.bubble_assistant,
                inner: gpui::white(),
                badge: hex(0xf1f1f3),
                badge_ink: hex(0xb4b4b8),
                close: hex(0xb4b4b8),
                hairline: Theme::with_alpha(gpui::black(), 0.06),
                highlight: Theme::with_alpha(gpui::black(), 0.035),
            },
        }
    }
}

fn hex(v: u32) -> Hsla {
    gpui::rgb(v).into()
}

/// Palette plus theme, passed together so row helpers stay short.
#[derive(Clone, Copy)]
pub struct CardLook<'a> {
    pub palette: CardPalette,
    pub theme: &'a Theme,
}

impl<'a> CardLook<'a> {
    pub fn of(theme: &'a Theme) -> Self {
        Self {
            palette: CardPalette::of(theme),
            theme,
        }
    }
}

/// `A`, `B`, … for the row badges; past `Z` the number.
pub fn letter(ix: usize) -> String {
    if ix < 26 {
        char::from(b'A' + ix as u8).to_string()
    } else {
        (ix + 1).to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    Open,
    /// Keyboard cursor.
    Highlighted,
    /// The picked answer on an answered (or multi-select) card.
    Chosen,
    /// Not picked on an answered card, or out of bounds.
    Faded,
}

/// A click on the card's ×.
pub type CloseHandler = Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static>;

/// The outer card: bubble grey, generous radius; the header row goes in first.
/// Grok: a card is as wide as the assistant block — it fills the agent
/// bubble's slot (`transcript::assistant_slot`), which carries the cap.
pub fn card_frame(id: impl Into<SharedString>, palette: &CardPalette) -> Stateful<Div> {
    div()
        .id(id.into())
        .w_full()
        .max_w_full()
        .my(px(4.0))
        .flex()
        .flex_col()
        .gap(px(9.0))
        .p(px(CARD_PAD))
        .rounded(px(CARD_RADIUS))
        .bg(palette.outer)
}

/// Bold question, thin grey × on the right (none when the card is settled).
pub fn card_header(
    close_id: impl Into<SharedString>,
    title: &str,
    on_close: Option<CloseHandler>,
    palette: &CardPalette,
    theme: &Theme,
) -> Div {
    let mut header = div().flex().items_start().gap(px(10.0)).child(
        div()
            .flex_1()
            .min_w_0()
            .pl(px(1.0))
            .text_size(px(TITLE_TEXT))
            .line_height(px(TITLE_TEXT + 4.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.text)
            .child(title.to_string()),
    );
    if let Some(on_close) = on_close {
        header = header.child(
            div()
                .id(close_id.into())
                .flex_shrink_0()
                .size(px(18.0))
                .mt(px(0.5))
                .mr(px(-1.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.0))
                .cursor_pointer()
                // hover-fill: exception card-option
                .hover(|s| s.bg(palette.highlight))
                .child(icon("xmark", 13.0, palette.close))
                .on_click(move |e, window, cx| on_close(e, window, cx)),
        );
    }
    header
}

/// The white (dark: lighter) inner list that holds the rows.
pub fn option_list(palette: &CardPalette) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .rounded(px(LIST_RADIUS))
        .bg(palette.inner)
        .overflow_hidden()
}

/// One row: lettered badge, plain label, a hairline above when not the first.
pub fn option_row(
    id: impl Into<SharedString>,
    badge: Option<String>,
    label: &str,
    state: RowState,
    hairline_above: bool,
    look: &CardLook,
) -> Stateful<Div> {
    let (palette, theme) = (&look.palette, look.theme);
    let ink = match state {
        RowState::Open | RowState::Highlighted | RowState::Chosen => theme.text,
        RowState::Faded => theme.text_faint,
    };
    let (badge_bg, badge_ink) = match state {
        RowState::Chosen => (theme.text, palette.inner),
        RowState::Open | RowState::Highlighted | RowState::Faded => {
            (palette.badge, palette.badge_ink)
        }
    };
    let clickable = matches!(state, RowState::Open | RowState::Highlighted);
    div()
        .id(id.into())
        .flex()
        .items_center()
        .gap(px(10.0))
        .min_h(px(ROW_HEIGHT))
        .px(px(ROW_PAD_X))
        .py(px(6.0))
        .when(hairline_above, |el| {
            el.border_t_1().border_color(palette.hairline)
        })
        .when(state == RowState::Highlighted, |el| {
            el.bg(palette.highlight)
        })
        .when(clickable, |el| {
            // hover-fill: exception card-option
            el.cursor_pointer().hover(|s| s.bg(palette.highlight))
        })
        .when_some(badge, |el, letter| {
            el.child(
                div()
                    .flex_shrink_0()
                    .size(px(BADGE))
                    .rounded(px(5.0))
                    .bg(badge_bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.5))
                    .text_color(badge_ink)
                    .child(letter),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(ROW_TEXT))
                .line_height(px(ROW_TEXT + 5.0))
                .text_color(ink)
                .child(label.to_string()),
        )
}

/// Small grey pill for form choices / yes-no (inside a form row).
fn pill(
    id: impl Into<SharedString>,
    label: &str,
    on: bool,
    palette: &CardPalette,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .px(px(9.0))
        .py(px(3.0))
        .rounded(px(6.0))
        .text_size(px(12.0))
        .bg(if on { theme.text } else { palette.badge })
        .text_color(if on { palette.inner } else { theme.text })
        .cursor_pointer()
        .child(label.to_string())
}
