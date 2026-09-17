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

/// Why the trajectory is not here, and what he can do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// The kernel answered that it does not know the frame. It is older than
    /// the app, which the app can say for certain rather than guess.
    KernelTooOld,
    /// Nothing came back in time: a kernel that is down, wedged, or on a link
    /// that is not carrying. Distinguished from the above because the fix is
    /// different — waiting or reopening, rather than updating.
    NoAnswer,
}

impl Unavailable {
    /// One sentence, naming the fix. Not "unavailable": he can act on this.
    fn words(&self) -> &'static str {
        match self {
            Self::KernelTooOld => {
                "This project's kernel is older than the app and cannot attach the transcript or the log. Update it from the bar, then report again to include them."
            }
            Self::NoAnswer => {
                "This project's kernel did not answer, so the transcript and the log are not attached. Your words and the screenshot will still be sent."
            }
        }
    }

    /// What a part's row says in place of a count.
    fn row_detail(&self) -> &'static str {
        match self {
            Self::KernelTooOld => "not attached — the kernel is too old",
            Self::NoAnswer => "not attached — the kernel did not answer",
        }
    }
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
    /// Why no bundle is coming, once that is known: the kernel's own refusal,
    /// or nothing said in time.
    ///
    /// Jacob's first try on his Mac showed why this has to exist. His kernel
    /// predated the `feedback` frame, the refusal was dropped, and the sheet
    /// sat on "still reading the exchange from the kernel…" while three rows
    /// read "nothing to send" — which is what a report with nothing to attach
    /// also looks like. He was one click from sending a report that looked
    /// empty, with no way to tell that anything had gone wrong.
    unavailable: Option<Unavailable>,
    /// Every outbox refused the report, so the only thing left that helps is
    /// getting his words off this screen.
    stranded: bool,
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
            unavailable: None,
            stranded: false,
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
        self.unavailable = None;
        self.stranded = false;
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
        self.unavailable = None;
        self.draft.trajectory_unavailable = None;
        cx.notify();
    }

    /// There will be no bundle, and this is why.
    ///
    /// A bundle that arrives after this is still taken: the sheet says what it
    /// knows at the time and corrects itself if the kernel turns out to be
    /// merely slow.
    pub fn no_bundle(&mut self, why: Unavailable, cx: &mut Context<Self>) {
        if !self.is_open || self.draft.bundle.is_some() {
            return;
        }
        self.awaiting = false;
        // Onto the draft as well as the sheet: the report has to carry the
        // reason, or a loop reading it sees a part he kept and did not get and
        // has nothing to explain it with.
        self.draft.trajectory_unavailable = Some(why.words().to_string());
        self.unavailable = Some(why);
        cx.notify();
    }

    /// Whether the sheet is still expecting an answer — for the timeout to ask
    /// before it declares one.
    pub fn awaiting(&self) -> bool {
        self.awaiting
    }

    /// For the driver: what the sheet says about the trajectory, if anything.
    pub fn unavailable(&self) -> Option<&'static str> {
        self.unavailable.as_ref().map(Unavailable::words)
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
        self.stranded = false;
        cx.notify();
    }

    /// Nowhere on the machine would take the report.
    ///
    /// Jacob's home directory went read-only, the outbox could not be created,
    /// and the sheet offered Close and Send again — one of which loses his words
    /// and the other of which repeats the failure. So this state offers to put
    /// his words on the clipboard. A person's words must always have somewhere
    /// to go, even when the disk has none.
    pub fn nowhere_to_save(&mut self, why: String, cx: &mut Context<Self>) {
        self.outcome = Some(Err(format!(
            "Nothing on this machine would take the report: {why}. Your words are still here — copy them out and they are not lost."
        )));
        self.stranded = true;
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

    /// His words onto the clipboard, so a report that cannot be saved anywhere
    /// still leaves with him.
    fn copy_note(&mut self, cx: &mut Context<Self>) {
        let note = self.field.read(cx).content().to_string();
        if note.trim().is_empty() {
            return;
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(note));
        self.outcome = Some(Ok(
            "Copied. Paste it wherever you like — it is out of this window and safe.".into(),
        ));
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

    /// The words under the buttons — what he actually reads after pressing
    /// Send — and whether they are good news.
    ///
    /// On the driver's surface because these are the sentences that make or
    /// break the feature's honesty, and QA had to photograph the window to
    /// check them. A promise should be assertable.
    pub fn message(&self) -> Option<(bool, String)> {
        match &self.outcome {
            Some(Ok(text)) => Some((true, text.clone())),
            Some(Err(text)) => Some((false, text.clone())),
            None => None,
        }
    }

    /// Whether a picture came, and whether it is of the window alone. The
    /// rig cannot tell a scaled window capture from a screen grab by looking.
    pub fn shot_state(&self) -> (bool, bool, Option<String>) {
        (
            self.draft.shot.is_some(),
            self.draft.shot.as_ref().is_some_and(|s| s.whole_screen),
            self.draft.shot_error.clone(),
        )
    }
}

impl Focusable for FeedbackSheet {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for FeedbackSheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_open {
            return div().into_any_element();
        }
        let viewport = window.viewport_size();
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
                "What the app knows",
                &self.desktop_detail(),
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
                    .w(px(WIDTH.min(f32::from(viewport.width) - 32.)))
                    // Never taller than the window it sits in. Without this the
                    // card grew with its contents and, centred, pushed its own
                    // footer off the screen — which is how Jacob ended up
                    // looking at a sheet whose only control was Close.
                    .max_h(px((f32::from(viewport.height) - 48.).max(240.)))
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
                    // The parts scroll; the heading, his words and the buttons
                    // do not. Whatever the report holds, Send stays on screen.
                    .child(
                        div()
                            .id("feedback-parts")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap(px(14.))
                            .child(theme.group_box().children(rows))
                            .child(self.tool_io_control(&theme, cx))
                            .child(self.provenance(&b, &theme)),
                    )
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
        // The trajectory, the log and the tail come from the kernel; the
        // screenshot and the app's own view do not. Only the first three can be
        // missing because the kernel would not answer.
        let from_kernel = matches!(part, Open::Trajectory | Open::Log | Open::Tail);
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
                        // "nothing to send" is what an empty part and a failed
                        // one both used to read as, and they are opposite
                        // facts. When the kernel has told us why, say that.
                        match (&self.unavailable, from_kernel) {
                            (Some(why), true) => why.row_detail().to_string(),
                            _ => "nothing to send".to_string(),
                        }
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
                .map(|s| feedback::session_lines(s, &b.agents))
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

    /// What the window's own state amounts to, and whether any of it disagrees
    /// with the kernel — the one thing worth reading first.
    fn desktop_detail(&self) -> String {
        let Some(state) = &self.draft.session else {
            return "nothing to send".into();
        };
        let rows = state.get("rows").and_then(|r| r.as_array()).map_or(0, Vec::len);
        let records = state
            .get("records")
            .and_then(|r| r.as_array())
            .map_or(0, Vec::len);
        let clipped = state
            .get("records_clipped")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let known: Vec<&str> = self
            .draft
            .bundle
            .as_ref()
            .map(|b| {
                b.agents
                    .iter()
                    .filter_map(|a| a.get("id").and_then(serde_json::Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        let phantom = state
            .get("rows")
            .and_then(|r| r.as_array())
            .map(|rows| {
                rows.iter()
                    .filter(|row| {
                        row.get("agent")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|a| !a.is_empty() && !known.is_empty() && !known.contains(&a))
                    })
                    .count()
            })
            .unwrap_or(0);
        let mut detail = format!("{rows} rows, {records} records on disk");
        if clipped > 0 {
            detail.push_str(&format!(", {clipped} left out for size"));
        }
        if phantom > 0 {
            detail.push_str(&format!(
                " — {phantom} the kernel has no agent for",
            ));
        }
        detail
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
        if let Some(why) = &self.unavailable {
            lines.push(why.words().into());
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
            None if !ready => ("Your words are the one thing needed.".into(), theme.text_faint),
            None => (String::new(), theme.text_faint),
        };
        // The message sits on its own line, above the buttons, and never beside
        // them.
        //
        // This is the fault Jacob hit. It used to share a row with them, in a
        // `flex_1` that would not shrink below its text, so one long sentence —
        // "Saved, and waiting: it could not be sent yet — no feedback
        // credentials at …" — pushed both buttons off the right edge of the
        // card. On his window Close was half on screen and Send was entirely
        // off it, so the sheet held his whole report and offered no way to send
        // it. Driving it on a real window put Send at x=1216 in an 820-wide
        // window, `reachable: false`; a diff would not have shown that.
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .when(!message.is_empty(), |col| {
                col.child(
                    div()
                        .id("feedback-message")
                        .w_full()
                        .max_h(px(72.))
                        .overflow_hidden()
                        .text_style(TextStyle::Caption)
                        .text_color(tint)
                        .child(SharedString::from(message)),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    // Nothing may squeeze the one row he has to be able to
                    // reach.
                    .flex_shrink_0()
                    .child(
                        theme
                            .button("Close", ButtonStyle::Ghost, None)
                            .id("feedback-close")
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                    )
                    // When nothing would take the report, the thing that helps
                    // is his words leaving this screen. Offered before Send, so
                    // it is the first thing his eye lands on.
                    .when(self.stranded, |row| {
                        row.child(
                            theme
                                .button("Copy my words", ButtonStyle::Ghost, None)
                                .id("feedback-copy")
                                .on_click(cx.listener(|this, _, _, cx| this.copy_note(cx))),
                        )
                    })
                    // Send is always here. It used to be hidden once anything
                    // settled, which left a failed send with no way to retry.
                    .child(
                        theme
                            .button(
                                if self.outcome.is_some() { "Send again" } else { "Send" },
                                ButtonStyle::Prominent,
                                None,
                            )
                            .id("feedback-send")
                            .when(!ready, |el| el.opacity(0.5))
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            )
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
