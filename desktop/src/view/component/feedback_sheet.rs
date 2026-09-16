//! The review sheet: the one thing Jacob touches when something looks wrong.
//!
//! It opens on the exchange he is looking at, asks for his words, and shows
//! **everything that will leave his machine** — a row per part, each one
//! expandable into lines a person can read, each one removable with a single
//! click. Send is the only thing that sends.
//!
//! Two rules the sheet exists to keep, and neither is negotiable:
//!
//! - **Nothing goes silently.** He sees the parts before Send, and a part he
//!   removes is recorded as removed, so a loop reading the report can tell
//!   his choice from a fault.
//! - **What he sees is readable.** His consent is to a category — "my code
//!   may go" — and that is not consent to sending something he cannot see. So
//!   every part is drawn as lines ([`crate::feedback::event_lines`] and its
//!   siblings), never as the JSON that actually travels. A wall of JSON in a
//!   dialog shows him nothing.
//!
//! Credentials are stripped by shape whichever way the tool-argument control
//! is set. That is not this sheet's politeness — the kernel redacts before
//! the bundle is ever handed over, so there is no path through this file that
//! could send a key. "My code may go" was never "my keys may go".

use crate::feedback::{self, Draft, Parts};
use bezel::{
    gpui::{
        self, AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
        KeyBinding, MouseButton, Render, SharedString, Window, actions, div, prelude::*, px,
    },
    theme::{TextStyle, Theme, Typeset},
    ui::{
        input::TextField,
        tooltip::Tooltip,
        widgets::{ButtonStyle, Buttons, Scaffolding},
    },
};

actions!(arbos_feedback_sheet, [SendReport, DismissReport]);

const KEY_CONTEXT: &str = "ArbosFeedbackSheet";
const WIDTH: f32 = 620.;
/// How tall an opened part may grow before it scrolls. Enough to read a turn
/// without the sheet becoming the window.
const OPEN_MAX: f32 = 220.;

pub fn init(cx: &mut App) {
    crate::view::bind_field_editing(cx, KEY_CONTEXT, false);
    let ctx = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("cmd-enter", SendReport, ctx),
        KeyBinding::new("escape", DismissReport, ctx),
    ]);
}

/// Which part's lines are open. One at a time: the sheet is for scanning, and
/// four open parts is the JSON wall again in another shape.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Open {
    None,
    Trajectory,
    Log,
    Tail,
    Session,
    Screenshot,
}

pub enum FeedbackSheetEvent {
    /// Write this report to the outbox. The workspace owns the disk and the
    /// delivery; the sheet owns what is in it.
    Send(Box<Draft>),
    Dismissed,
}

pub struct FeedbackSheet {
    field: Entity<TextField>,
    draft: Draft,
    /// Which agent and line the report is about, for the ask to the kernel.
    anchor: Option<(String, Option<u64>)>,
    /// Waiting for the kernel's answer. His words are typed meanwhile, so the
    /// wait is never in his way.
    awaiting: bool,
    open: Open,
    /// Set once the report is on disk: what to tell him, and whether it is
    /// waiting for the network.
    outcome: Option<Result<String, String>>,
    pub is_open: bool,
    focus: FocusHandle,
}

impl EventEmitter<FeedbackSheetEvent> for FeedbackSheet {}

impl FeedbackSheet {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| {
            TextField::new(cx)
                .with_frame(true)
                .with_key_context(KEY_CONTEXT)
                .with_placeholder("What went wrong?")
        });
        Self {
            field,
            draft: Draft::new(Parts::default()),
            anchor: None,
            awaiting: false,
            open: Open::None,
            outcome: None,
            is_open: false,
            focus: cx.focus_handle(),
        }
    }

    /// Open the sheet. `agent` and `seq` name the exchange he is looking at;
    /// with no `seq` the kernel takes the last exchange he opened, which is
    /// the right answer when he came in from the menu rather than from a
    /// turn's footer.
    pub fn show(
        &mut self,
        agent: Option<String>,
        seq: Option<u64>,
        parts: Parts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.draft = Draft::new(parts);
        self.anchor = agent.map(|a| (a, seq));
        self.awaiting = self.anchor.is_some();
        self.open = Open::None;
        self.outcome = None;
        self.is_open = true;
        self.field.update(cx, |field, cx| field.set_content("", cx));
        // The field takes the caret, not the sheet: he should be able to
        // type the moment it opens, since the whole point is one second.
        let field = self.field.focus_handle(cx);
        window.focus(&field, cx);
        cx.notify();
    }

    /// What the report should ask the kernel for, once the sheet is open.
    pub fn wanted(&self) -> Option<(String, Option<u64>, u32)> {
        self.anchor
            .as_ref()
            .map(|(agent, seq)| (agent.clone(), *seq, feedback::TAIL_LINES))
    }

    /// The kernel answered.
    pub fn take_bundle(&mut self, bundle: feedback::Bundle, cx: &mut Context<Self>) {
        if !self.is_open {
            return;
        }
        self.draft.bundle = Some(bundle);
        self.awaiting = false;
        cx.notify();
    }

    /// The picture arrived, or did not.
    pub fn take_shot(&mut self, shot: Result<feedback::Shot, String>, cx: &mut Context<Self>) {
        match shot {
            // Scaling happens in the capture, so a picture that arrives here
            // already fits. It used to be refused for size instead, which meant
            // no report from a Retina Mac carried one at all (F-101).
            Ok(shot) => self.draft.shot = Some(shot),
            Err(why) => self.draft.shot_error = Some(why),
        }
        cx.notify();
    }

    /// The app's own view of the chat, for the case where the drawing and the
    /// transcript disagree — which is the bug, not a detail.
    pub fn take_session(&mut self, session: serde_json::Value, cx: &mut Context<Self>) {
        self.draft.session = Some(session);
        cx.notify();
    }

    pub fn settled(&mut self, outcome: Result<String, String>, cx: &mut Context<Self>) {
        self.outcome = Some(outcome);
        cx.notify();
    }

    fn send(&mut self, _: &SendReport, _: &mut Window, cx: &mut Context<Self>) {
        self.submit(cx);
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        self.draft.note = self.field.read(cx).content().to_string();
        if !self.draft.ready() {
            return;
        }
        cx.emit(FeedbackSheetEvent::Send(Box::new(self.draft.clone())));
        cx.notify();
    }

    fn dismiss_action(&mut self, _: &DismissReport, _: &mut Window, cx: &mut Context<Self>) {
        self.dismiss(cx);
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if !self.is_open {
            return;
        }
        self.is_open = false;
        cx.emit(FeedbackSheetEvent::Dismissed);
        cx.notify();
    }

    fn toggle(&mut self, part: Open, cx: &mut Context<Self>) {
        self.open = if self.open == part { Open::None } else { part };
        cx.notify();
    }

    /// The parts as they stand, for the workspace to remember his answer to
    /// the tool-argument control.
    pub fn parts(&self) -> Parts {
        self.draft.parts
    }
}

impl Focusable for FeedbackSheet {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for FeedbackSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_open {
            return div().into_any_element();
        }
        let theme = Theme::of(cx).clone();
        let b = self.draft.bundle.clone().unwrap_or_default();
        let parts = self.draft.parts;

        let heading = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_style(TextStyle::Title3)
                    .text_color(theme.text)
                    .child("Report a problem"),
            )
            .child(
                div()
                    .text_style(TextStyle::Subheadline)
                    .text_color(theme.text_muted)
                    .child(
                        "Everything below is what will be sent. Remove anything you would rather keep.",
                    ),
            );

        let rows: Vec<AnyElement> = vec![
            self.part_row(
                Open::Screenshot,
                "Screenshot",
                &self.shot_detail(),
                parts.screenshot && self.draft.shot.is_some(),
                self.draft.shot.is_some(),
                &theme,
                cx,
            ),
            self.part_row(
                Open::Trajectory,
                "What the agent did",
                &format!(
                    "{} lines, {} tool calls, {} failed",
                    b.events.len(),
                    b.tool_calls(),
                    b.failed_calls()
                ),
                parts.trajectory,
                !b.events.is_empty(),
                &theme,
                cx,
            ),
            self.part_row(
                Open::Log,
                "Kernel log",
                &format!("{} lines around this exchange", b.log.len()),
                parts.log,
                !b.log.is_empty(),
                &theme,
                cx,
            ),
            self.part_row(
                Open::Tail,
                "Earlier in this chat",
                &format!("{} lines before it", b.tail.len()),
                parts.tail && parts.trajectory,
                !b.tail.is_empty() && parts.trajectory,
                &theme,
                cx,
            ),
            self.part_row(
                Open::Session,
                "What the app drew",
                "the window's own view, to compare with the transcript",
                parts.session && self.draft.session.is_some(),
                self.draft.session.is_some(),
                &theme,
                cx,
            ),
        ];

        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::send))
            .on_action(cx.listener(Self::dismiss_action))
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
                cx.listener(|this, _, _, cx| this.dismiss(cx)),
            )
            .child(
                div()
                    .id("feedback-sheet")
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
                    .child(heading)
                    // Named so the rig can put the caret back after
                    // clicking a row; typing already works, since the
                    // field takes the focus when the sheet opens.
                    .child(div().id("feedback-note").child(self.field.clone()))
                    .child(theme.group_box().children(rows))
                    .child(self.tool_io_control(&theme, cx))
                    .child(self.provenance(&b, &theme))
                    .child(self.footer(&theme, cx)),
            )
            .into_any_element()
    }
}

impl FeedbackSheet {
    /// One part: its name, what it amounts to, a way to look at it, and a way
    /// to take it out.
    #[allow(clippy::too_many_arguments)]
    fn part_row(
        &self,
        part: Open,
        label: &str,
        detail: &str,
        included: bool,
        available: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = format!("{part:?}").to_lowercase();
        let open = self.open == part && included && available;

        let head = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .w(px(160.))
                    .text_style(TextStyle::Body)
                    .text_color(if included && available {
                        theme.text
                    } else {
                        theme.text_faint
                    })
                    .child(SharedString::from(label.to_string())),
            )
            .child(
                div()
                    .flex_1()
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_muted)
                    .child(SharedString::from(if !available {
                        "nothing to send".to_string()
                    } else if included {
                        detail.to_string()
                    } else {
                        "removed — the report will say you removed it".to_string()
                    })),
            )
            .when(available && included, |row| {
                row.child(
                    div()
                        .id(SharedString::from(format!("feedback-open-{id}")))
                        .cursor_pointer()
                        .rounded(px(4.))
                        .px(px(6.))
                        .hover(|el| el.bg(theme.element_hover))
                        .text_style(TextStyle::Caption)
                        .text_color(theme.text_muted)
                        .tooltip(|window, cx| {
                            Tooltip::text("See exactly what this sends", window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(part, cx)))
                        .child(SharedString::from(if open { "hide" } else { "show" })),
                )
            })
            .when(available, |row| {
                row.child(
                    div()
                        .id(SharedString::from(format!("feedback-cut-{id}")))
                        .cursor_pointer()
                        .rounded(px(4.))
                        .p(px(3.))
                        .hover(|el| el.bg(theme.element_hover))
                        .tooltip(move |window, cx| {
                            Tooltip::text(
                                if included {
                                    "Remove this"
                                } else {
                                    "Put it back"
                                },
                                window,
                                cx,
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.cut(part, cx);
                        }))
                        .child(
                            div()
                                .text_style(TextStyle::Caption)
                                .text_color(if included {
                                    theme.text_muted
                                } else {
                                    theme.accent
                                })
                                .child(SharedString::from(if included { "✕" } else { "undo" })),
                        ),
                )
            });

        if !open {
            return head.into_any_element();
        }

        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(head)
            .child(self.preview(part, theme))
            .into_any_element()
    }

    /// The part's own contents, as lines. Never the JSON: he cannot scan JSON,
    /// and a consent given to something unreadable is not consent.
    fn preview(&self, part: Open, theme: &Theme) -> AnyElement {
        if part == Open::Screenshot {
            return match &self.draft.shot {
                Some(shot) => div()
                    .max_h(px(OPEN_MAX))
                    .child(
                        div()
                            .text_style(TextStyle::Caption)
                            .text_color(theme.text_muted)
                            .child(SharedString::from(format!(
                                "{}×{}, {} — {}",
                                shot.width,
                                shot.height,
                                feedback::bytes_human(shot.bytes.len() as u64),
                                if shot.whole_screen {
                                    "the whole display: this desktop gave no way to photograph the window alone, so anything else on screen is in it"
                                } else {
                                    "this window only, never the whole screen"
                                }
                            ))),
                    )
                    .into_any_element(),
                None => div().into_any_element(),
            };
        }

        let b = self.draft.bundle.clone().unwrap_or_default();
        let mut lines = match part {
            Open::Trajectory => {
                let mut events = b.events.clone();
                if !self.draft.parts.tool_io {
                    feedback::strip_tool_io(&mut events);
                }
                feedback::event_lines(&events)
            }
            Open::Tail => {
                let mut tail = b.tail.clone();
                if !self.draft.parts.tool_io {
                    feedback::strip_tool_io(&mut tail);
                }
                feedback::event_lines(&tail)
            }
            Open::Log => feedback::log_lines(&b.log),
            Open::Session => self
                .draft
                .session
                .as_ref()
                .map(feedback::session_lines)
                .unwrap_or_default(),
            Open::None | Open::Screenshot => vec![],
        };
        // A part with hundreds of lines is scannable at its ends, not in the
        // middle, and the sheet says how much it is not showing.
        let total = lines.len();
        if total > 40 {
            let tail = lines.split_off(total - 20);
            lines.truncate(20);
            lines.push(format!("… {} more lines, all of them sent …", total - 40));
            lines.extend(tail);
        }

        div()
            .id(SharedString::from(format!("feedback-preview-{part:?}")))
            .max_h(px(OPEN_MAX))
            .overflow_y_scroll()
            .p(px(8.))
            .rounded(px(4.))
            .bg(theme.surface_raised)
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(lines.into_iter().map(|line| {
                div()
                    .font_family(theme.font_mono.clone())
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_muted)
                    .child(SharedString::from(line))
            }))
            .into_any_element()
    }

    fn cut(&mut self, part: Open, cx: &mut Context<Self>) {
        let p = &mut self.draft.parts;
        match part {
            Open::Screenshot => p.screenshot = !p.screenshot,
            Open::Trajectory => p.trajectory = !p.trajectory,
            Open::Log => p.log = !p.log,
            Open::Tail => p.tail = !p.tail,
            Open::Session => p.session = !p.session,
            Open::None => {}
        }
        cx.notify();
    }

    fn shot_detail(&self) -> String {
        match (&self.draft.shot, &self.draft.shot_error) {
            // A whole-screen capture leads with that, because it is the one
            // thing about the picture he may want to act on: his other windows
            // are in it, and the design says they are not the report.
            (Some(shot), _) if shot.whole_screen => format!(
                "your whole screen, not only Arbos — {}×{}, {}",
                shot.width,
                shot.height,
                feedback::bytes_human(shot.bytes.len() as u64)
            ),
            (Some(shot), _) => format!(
                "{}×{}, {}",
                shot.width,
                shot.height,
                feedback::bytes_human(shot.bytes.len() as u64)
            ),
            (None, Some(why)) => format!("no picture: {why}"),
            (None, None) => "taking it…".into(),
        }
    }

    /// Jacob's own decision, kept as a control rather than a constant: the
    /// arguments his agents passed and the glance at what came back. He said
    /// yes to sending them, so this starts on — and it is here so he can say
    /// no to one particular report without changing his mind in general.
    fn tool_io_control(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let on = self.draft.parts.tool_io;
        let calls = self.draft.tool_io_count();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .id("feedback-tool-io")
                    .cursor_pointer()
                    .rounded(px(4.))
                    .px(px(6.))
                    .py(px(3.))
                    .border_1()
                    .border_color(if on { theme.accent } else { theme.border })
                    .hover(|el| el.bg(theme.element_hover))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.draft.parts.tool_io = !this.draft.parts.tool_io;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .text_style(TextStyle::Caption)
                            .text_color(if on { theme.accent } else { theme.text_muted })
                            .child(SharedString::from(if on {
                                "Sending tool arguments and output"
                            } else {
                                "Tool arguments and output removed"
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child(SharedString::from({
                        let s = if calls == 1 { "call" } else { "calls" };
                        if on {
                            format!(
                                "{calls} {s} carry what they were given and a glance at what came back — including file contents"
                            )
                        } else {
                            format!(
                                "{calls} {s} will say what ran and whether it failed, with no file contents and no paths"
                            )
                        }
                    })),
            )
            .into_any_element()
    }

    /// Where the report came from and what was taken out of it on the way.
    /// The credential line is the important one: it holds whichever way the
    /// control above is set, because the kernel strips keys by shape before
    /// this sheet ever sees them.
    fn provenance(&self, b: &feedback::Bundle, theme: &Theme) -> AnyElement {
        let removed = b.credentials_removed();
        let kernel = |k: &str| {
            b.kernel
                .get(k)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?")
                .to_string()
        };
        let mut lines = vec![format!(
            "{} · kernel {} {} · {}/{}",
            crate::build::version_label(),
            kernel("version"),
            kernel("git_sha"),
            kernel("os"),
            kernel("arch"),
        )];
        if removed > 0 {
            lines.push(format!(
                "{removed} credential{} removed before this reached the sheet — always, whatever you choose above",
                if removed == 1 { "" } else { "s" }
            ));
        }
        // His own words are the one part no kernel has seen, so if he has
        // pasted a key into them, say so here rather than only in the report.
        // It goes out redacted either way; he may want to know he did it.
        let in_his_words = self.draft.note_redaction().total();
        if in_his_words > 0 {
            lines.push(format!(
                "{in_his_words} credential{} in your own words will be removed too",
                if in_his_words == 1 { "" } else { "s" }
            ));
        }
        if b.truncated {
            lines.push("this exchange was long, so its middle was left out".into());
        }
        if self.awaiting {
            lines.push("still reading the exchange from the kernel…".into());
        }
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(lines.into_iter().map(|line| {
                div()
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child(SharedString::from(line))
            }))
            .into_any_element()
    }

    fn footer(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let ready = !self.field.read(cx).content().trim().is_empty();
        let (message, tint) = match &self.outcome {
            Some(Ok(text)) => (text.clone(), theme.success),
            Some(Err(why)) => (why.clone(), theme.danger),
            None if !ready => (
                "Your words are the one thing needed.".into(),
                theme.text_faint,
            ),
            None => (String::new(), theme.text_faint),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .text_style(TextStyle::Caption)
                    .text_color(tint)
                    .child(SharedString::from(message)),
            )
            .child(
                theme
                    .button("Close", ButtonStyle::Ghost, None)
                    .id("feedback-close")
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
            )
            .when(self.outcome.is_none(), |row| {
                row.child(
                    theme
                        .button("Send", ButtonStyle::Prominent, None)
                        .id("feedback-send")
                        .when(!ready, |el| el.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feedback::Bundle;
    use serde_json::json;

    /// The sheet's own rule, checked without a window: cutting the trajectory
    /// takes its tail with it, and the report says so.
    #[test]
    fn cutting_a_part_is_recorded_rather_than_hidden() {
        let mut draft = Draft::new(Parts::default());
        draft.note = "the sidebar draws twice".into();
        draft.bundle = Some(Bundle {
            events: vec![json!({"kind": "tool", "name": "bash", "args": {"command": "ls"}})],
            tail: vec![json!({"kind": "wake", "wake": "user", "text": "before"})],
            log: vec![json!({"ts": 1, "event": "turn.start"})],
            ..Default::default()
        });
        draft.parts.log = false;
        let report = draft.report("id", 0);
        assert_eq!(report["included"]["log"], json!(false));
        assert_eq!(report["included"]["trajectory"], json!(true));
        assert_eq!(report["included"]["tool_io"], json!(true), "Jacob said yes");
    }
}
