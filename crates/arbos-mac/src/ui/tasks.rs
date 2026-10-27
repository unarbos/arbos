//! The tasks chip and drawer — the Mac client half of the duplex design's
//! step 3 (`docs/duplex-agent-design.md` §2.4, D3 / D7; [feedback
//! tasks-chip-drawer-mac-2026-10-25]).
//!
//! While the agent has background jobs open in the chat, the presence row
//! over the composer ("Reading tripadvisor.com") carries a small chip with
//! the count ("2 tasks"). A click on the chip opens the drawer: a popover
//! anchored over the row with one row per open job — title, live step,
//! elapsed — and **Steer** / **Stop** per job, **Stop all** for the turn and
//! every job. Steer is no command of its own: it prefills the composer as a
//! reply to the job's ask (the agent binds a correction that quotes the ask to
//! that job). Stop sends `stop { jobId }`; Stop all sends today's `stop {}`.
//!
//! Data: `AppState::jobs`, polled from `GET /projects/{id}` → `activity.jobs[]`
//! while a job row is live. No jobs → no chip, no drawer: the row is exactly
//! what it was before this module (and what it is on a relay that predates
//! the field). The box already appends "· N tasks" to the presence text; the
//! chip carries the count instead, so the words are said once on the line.
//!
//! Grok Bot has no visible counterpart for this (its status line is a plain
//! shimmer, `grok-status-line-searching.png`); the chip follows the mention
//! chip's pointer rule (accent pill at 14 / 22 / 30 % alpha, 120 ms ease-out)
//! and the drawer the frosted dropdown every other popover uses.

use std::time::Instant;

use gpui::{
    anchored, deferred, div, prelude::*, px, Anchor, AnyElement, Context, Div, Entity, FontWeight,
    MouseButton, Stateful,
};

use crate::app::AppState;
use crate::model::{Id, Job, JobState};
use crate::ui::cards;
use crate::ui::glass;
use crate::ui::icons::icon;
use crate::ui::mention::{self, ChipPointer};
use crate::ui::presence::{self, Presence};
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, TEXT_SMALL, TEXT_TINY};

/// The chip: a capsule on the presence line, `glass::chip`'s size (8 pt sides,
/// the tiny type), the mention chip's pointer alphas on the accent.
pub const CHIP_HEIGHT: f32 = 20.0;
pub const CHIP_PAD_X: f32 = 8.0;
/// The chevron after the count (up while closed: the drawer opens upward).
pub const CHIP_CHEVRON: f32 = 11.0;

/// The drawer: a frosted menu card over the row, left-aligned with the line.
pub const DRAWER_WIDTH: f32 = 400.0;
/// Air between the card and the row it hangs over.
pub const DRAWER_GAP: f32 = 6.0;
/// Header line ("Tasks" · Stop all).
pub const HEADER_HEIGHT: f32 = 32.0;
/// A job row: Grok's two-line info row (`profiles::kit::ROW_MIN_H` 41 for one
/// line, 51 for title + subtitle).
pub const ROW_HEIGHT: f32 = 51.0;
pub const ROW_PAD_X: f32 = 12.0;
/// Steer / Stop: small capsules on the row's right.
pub const BUTTON_HEIGHT: f32 = 26.0;
pub const BUTTON_PAD_X: f32 = 11.0;
pub const BUTTON_GAP: f32 = 6.0;
/// The running row's spinner (the agent pane's Coding row, 15 pt).
pub const ROW_SPINNER: f32 = 15.0;

/// The one chip's key in the shell's [`ChipPointer`].
pub const CHIP_KEY: u8 = 0;

/// What the pointer is doing to the status line's link (`link`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkState {
    #[default]
    Rest,
    Hover,
    Pressed,
}

impl LinkState {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "rest" => Some(LinkState::Rest),
            "hover" => Some(LinkState::Hover),
            "pressed" => Some(LinkState::Pressed),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            LinkState::Rest => "rest",
            LinkState::Hover => "hover",
            LinkState::Pressed => "pressed",
        }
    }
}

/// The status line as a link to the newest open job's tab ([feedback
/// subagent-job-tabs-mac-2026-10-25]): at rest today's muted, breathing words; under the
/// pointer they brighten to `text` with a hairline underline; pressed they take the accent.
/// Release selects the tab. `ARBOS_RIG_TASKS=link-hover|link-pressed` paints a state.
pub fn link(
    text: String,
    state: LinkState,
    chat_id: Id,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let state = rig_setting()
        .and_then(|v| v.strip_prefix("link-"))
        .and_then(LinkState::parse)
        .unwrap_or(state);
    let phase = crate::ui::motion::PulseClock::phase(cx, 1100);
    let (color, opacity) = match state {
        LinkState::Rest => (theme.text_muted, 0.55 + 0.45 * phase),
        LinkState::Hover => (theme.text, 1.0),
        LinkState::Pressed => (theme.accent, 1.0),
    };
    div()
        .id("presence-link")
        .min_w_0()
        .truncate()
        .text_size(px(TEXT_SMALL))
        .text_color(color)
        .opacity(opacity)
        .cursor_pointer()
        .when(state != LinkState::Rest, |el| {
            el.text_decoration_1()
                .text_decoration_color(Theme::with_alpha(color, 0.7))
        })
        .debug_selector(move || format!("presence-link-{}", state.name()))
        .child(text)
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            let next = if *hovered {
                if this.tasks_link == LinkState::Pressed {
                    LinkState::Pressed
                } else {
                    LinkState::Hover
                }
            } else {
                LinkState::Rest
            };
            if this.tasks_link != next {
                this.tasks_link = next;
                cx.notify();
            }
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.tasks_link = LinkState::Pressed;
                cx.notify();
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.tasks_link = LinkState::Rest;
                cx.notify();
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if this.tasks_link == LinkState::Pressed {
                    this.tasks_link = LinkState::Hover;
                    let chat_id = chat_id.clone();
                    this.state
                        .update(cx, |s, cx| s.open_newest_job_tab(chat_id, cx));
                    crate::ui::composer::Composer::focus(&this.composer, window, cx);
                }
                cx.notify();
            }),
        )
}

/// The presence row with the chip on it and, while `open`, the drawer hung
/// over it. `jobs` is the open chat's list (`AppState::open_jobs`); empty →
/// the plain row, no chip, no drawer.
#[allow(clippy::too_many_arguments)]
pub fn presence_row(
    presence: Presence,
    app: &Entity<AppState>,
    chat_id: Id,
    jobs: Vec<Job>,
    open: bool,
    pointer: &ChipPointer<u8>,
    link_state: LinkState,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Div {
    if jobs.is_empty() {
        return presence::row(presence, Some(app), theme, cx);
    }
    let count = jobs.len();
    let chip = chip(count, open, chat_id.clone(), pointer, theme, cx).into_any_element();
    // In a job tab the line is the job's own; elsewhere it links to the newest job's tab.
    let words = presence::shown_text(&presence, true);
    let mut row = if crate::jobtabs::is_job_chat(&chat_id) {
        presence::row_with_chip(presence, Some(app), chip, theme, cx).relative()
    } else {
        let words = link(words, link_state, chat_id.clone(), theme, cx).into_any_element();
        presence::row_with_chip_and_words(presence, Some(app), chip, words, theme, cx).relative()
    };
    if open {
        row = row.child(
            div()
                .absolute()
                .left(px(presence::INSET))
                .top(px(-DRAWER_GAP))
                .child(
                    deferred(
                        anchored()
                            .anchor(Anchor::BottomLeft)
                            .position_mode(gpui::AnchoredPositionMode::Local)
                            .snap_to_window_with_margin(gpui::Edges::all(px(8.0)))
                            .child(glass::popover(drawer(chat_id, &jobs, theme, cx))),
                    )
                    .with_priority(1),
                ),
        );
    }
    row
}

/// "2 tasks ⌃": the accent capsule that opens the drawer. Rest / hover /
/// pressed through the shell's [`ChipPointer`] (the mention chip's fade); the
/// presence row's pulse clock repaints every frame while the row is up, so
/// the fade needs no clock of its own.
pub fn chip(
    count: usize,
    open: bool,
    chat_id: Id,
    pointer: &ChipPointer<u8>,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let (alpha, _) = match rig_chip_state() {
        Some(state) => (state.alpha(), false),
        None => pointer.alpha(&CHIP_KEY, Instant::now(), cx.reduce_motion()),
    };
    let label = Job::count_label(count);
    let chevron = if open { "chevron-down" } else { "chevron-up" };
    div()
        .id("tasks-chip")
        .h(px(CHIP_HEIGHT))
        .px(px(CHIP_PAD_X))
        .flex_shrink_0()
        .rounded_full()
        .bg(mention::pill_color(theme.accent, alpha))
        .flex()
        .items_center()
        .gap(px(3.0))
        .text_size(px(TEXT_TINY))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.accent)
        .whitespace_nowrap()
        .cursor_pointer()
        .debug_selector(|| "tasks-chip".into())
        .child(label)
        .child(icon(chevron, CHIP_CHEVRON, theme.accent))
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            let key = hovered.then_some(CHIP_KEY);
            if this.tasks_chip.hover(key, Instant::now()) {
                cx.notify();
            }
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                if this.tasks_chip.press(CHIP_KEY, Instant::now()) {
                    cx.notify();
                }
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.tasks_chip.release(Instant::now());
                cx.notify();
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if this.tasks_chip.release(Instant::now()).is_some() {
                    let chat_id = chat_id.clone();
                    this.state.update(cx, |s, cx| s.toggle_tasks(chat_id, cx));
                }
                cx.notify();
            }),
        )
}

/// The card: "Tasks" · Stop all, then one row per job.
pub fn drawer(chat_id: Id, jobs: &[Job], theme: &Theme, cx: &mut Context<Shell>) -> AnyElement {
    let mut body = div().flex().flex_col().gap(px(glass::MENU_GAP));
    body = body.child(header(chat_id.clone(), theme, cx));
    body = body.child(glass::menu_divider(theme));
    for (ix, job) in jobs.iter().enumerate() {
        body = body.child(job_row(chat_id.clone(), ix, job, theme, cx));
    }
    div()
        .debug_selector(|| "tasks-drawer".into())
        .child(glass::frosted_menu(DRAWER_WIDTH, theme, body))
        .into_any_element()
}

fn header(chat_id: Id, theme: &Theme, cx: &mut Context<Shell>) -> Div {
    div()
        .h(px(HEADER_HEIGHT))
        .px(px(ROW_PAD_X))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(TEXT_SMALL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child("Tasks"),
        )
        .child(
            text_button("tasks-stop-all", "Stop all", theme.danger, theme)
                .debug_selector(|| "tasks-stop-all".into())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let chat_id = chat_id.clone();
                    this.state.update(cx, |s, cx| s.stop_all(chat_id, cx));
                })),
        )
}

/// One job: spinner (running) or a faint pause mark (queued), the title over
/// "<step> · m:ss", then Steer (running only) and Stop.
fn job_row(
    chat_id: Id,
    ix: usize,
    job: &Job,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let leading: AnyElement = match job.state {
        JobState::Running => cards::activity_spinner(ROW_SPINNER, theme, cx).into_any_element(),
        JobState::Queued => icon("pause", ROW_SPINNER, theme.text_faint).into_any_element(),
    };
    let subtitle_color = match job.state {
        JobState::Running => theme.text_muted,
        JobState::Queued => theme.text_faint,
    };
    let subtitle = format!("{} · {}", job.status_line(), job.elapsed_label());
    let mut buttons = div()
        .flex()
        .items_center()
        .gap(px(BUTTON_GAP))
        .flex_shrink_0();
    if job.can_steer() {
        let steer_job = job.clone();
        let steer_chat = chat_id.clone();
        buttons = buttons.child(
            text_button(
                gpui::ElementId::Name(format!("tasks-steer-{}", job.id).into()),
                "Steer",
                theme.text,
                theme,
            )
            .debug_selector(move || format!("tasks-steer-{ix}"))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                let chat_id = steer_chat.clone();
                let job = steer_job.clone();
                let prefilled = this
                    .state
                    .update(cx, |s, cx| s.steer_job(chat_id, &job, cx));
                if prefilled {
                    crate::ui::composer::Composer::focus(&this.composer, window, cx);
                }
            })),
        );
    }
    let stop_id = job.id.clone();
    let stop_chat = chat_id;
    buttons = buttons.child(
        text_button(
            gpui::ElementId::Name(format!("tasks-stop-{}", job.id).into()),
            "Stop",
            theme.danger,
            theme,
        )
        .debug_selector(move || format!("tasks-stop-{ix}"))
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            let chat_id = stop_chat.clone();
            let job_id = stop_id.clone();
            this.state
                .update(cx, |s, cx| s.stop_job(chat_id, &job_id, cx));
        })),
    );
    // The row itself opens the job's tab (`jobtabs`): a `surface_2` wash under the pointer
    // and a › after the title say so; the buttons stop the click before it gets here.
    let open_job = job.id.clone();
    div()
        .id(gpui::ElementId::Name(
            format!("tasks-row-{}", job.id).into(),
        ))
        .h(px(ROW_HEIGHT))
        .px(px(ROW_PAD_X))
        .rounded(px(9.0))
        .flex()
        .items_center()
        .gap(px(10.0))
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_2))
        .debug_selector(move || format!("tasks-row-{ix}"))
        .on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            let job_id = open_job.clone();
            this.state.update(cx, |s, cx| s.open_job_tab(&job_id, cx));
            crate::ui::composer::Composer::focus(&this.composer, window, cx);
        }))
        .child(div().flex_shrink_0().size(px(ROW_SPINNER)).child(leading))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(TEXT_SMALL))
                                .text_color(theme.text)
                                .debug_selector(move || format!("tasks-row-title-{ix}"))
                                .child(job.title.clone()),
                        )
                        .child(icon("chevron-right", 12.0, theme.text_faint)),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(TEXT_SMALL))
                        .text_color(subtitle_color)
                        .debug_selector(move || format!("tasks-row-status-{ix}"))
                        .child(subtitle),
                ),
        )
        .child(buttons)
}

/// A small capsule button: `surface_2` fill, 13 pt label in `color`, lifted
/// under the pointer like the strip discs (`glass::control_disc`).
fn text_button(
    id: impl Into<gpui::ElementId>,
    label: &str,
    color: gpui::Hsla,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(BUTTON_HEIGHT))
        .px(px(BUTTON_PAD_X))
        .flex_shrink_0()
        .rounded_full()
        .bg(theme.surface_2)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(TEXT_SMALL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.bg(Theme::with_alpha(theme.text, 0.14)))
        .active(|s| s.bg(Theme::with_alpha(theme.text, 0.22)))
        .child(label.to_string())
}

/// Screenshot rig: `ARBOS_RIG_TASKS=open` opens the drawer as soon as the
/// first poll lists a job (`AppState::apply`); `hover` / `pressed` paint the
/// chip in that pointer state.
pub fn rig_setting() -> Option<&'static str> {
    static RIG: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    RIG.get_or_init(|| std::env::var("ARBOS_RIG_TASKS").ok())
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

fn rig_chip_state() -> Option<mention::ChipState> {
    rig_setting().and_then(mention::ChipState::parse)
}

/// The agent's open jobs for the presence row of `chat_id`, and whether the
/// drawer is up for it — one read for the shell's render.
pub fn for_chat(state: &AppState, chat_id: &str) -> (Vec<Job>, bool) {
    let jobs = state.open_jobs(chat_id);
    let open = !jobs.is_empty() && state.tasks_open.as_deref() == Some(chat_id);
    (jobs, open)
}

#[cfg(test)]
mod job_tests {
    //! The contract's words, per state (`internal/duplex-agent-build.md`,
    //! "Drawer, per state"), and the relay payload's tolerant read.
    use super::*;

    fn job(state: JobState, step: &str, elapsed_s: u64) -> Job {
        Job {
            id: "j1".into(),
            title: "Searching for surf schools".into(),
            state,
            step: step.into(),
            elapsed_s,
            ask_message_id: Some("m1".into()),
            chat_id: Some("c1".into()),
        }
    }

    #[test]
    fn the_status_line_follows_the_contract() {
        assert_eq!(
            job(JobState::Running, "Reading tripadvisor.com", 41).status_line(),
            "Reading tripadvisor.com"
        );
        assert_eq!(job(JobState::Running, "", 3).status_line(), "Working");
        assert_eq!(job(JobState::Queued, "", 3).status_line(), "Starting");
        assert_eq!(
            job(JobState::Queued, "Waiting for Fixing the tests", 3).status_line(),
            "Waiting for Fixing the tests"
        );
        assert!(job(JobState::Running, "", 1).can_steer());
        assert!(
            !job(JobState::Queued, "", 1).can_steer(),
            "a queued job has no session to steer"
        );
    }

    #[test]
    fn elapsed_reads_as_minutes_and_seconds() {
        assert_eq!(job(JobState::Running, "", 0).elapsed_label(), "0:00");
        assert_eq!(job(JobState::Running, "", 41).elapsed_label(), "0:41");
        assert_eq!(job(JobState::Running, "", 125).elapsed_label(), "2:05");
        assert_eq!(job(JobState::Running, "", 3725).elapsed_label(), "1:02:05");
    }

    #[test]
    fn the_chip_says_the_count_and_the_line_drops_the_suffix() {
        assert_eq!(Job::count_label(1), "1 task");
        assert_eq!(Job::count_label(2), "2 tasks");
        assert_eq!(
            Job::strip_count_suffix("Reading tripadvisor.com · 2 tasks"),
            "Reading tripadvisor.com"
        );
        assert_eq!(
            Job::strip_count_suffix("Searching for surf schools"),
            "Searching for surf schools"
        );
        assert_eq!(
            Job::strip_count_suffix("Sam is typing… · Lium is working"),
            "Sam is typing… · Lium is working",
            "only a trailing count goes"
        );
    }

    #[test]
    fn the_relay_payload_reads_tolerantly() {
        let v = serde_json::json!({
            "chat_id": "c1", "text": "Reading tripadvisor.com · 2 tasks", "at": 1,
            "jobs": [
                { "id": "j1", "title": "Searching for surf schools", "state": "running",
                  "step": "Reading tripadvisor.com", "elapsed_s": 41,
                  "ask_message_id": "m1", "chat_id": "c1" },
                { "id": "j2", "title": "Fixing the tests", "state": "queued", "step": "",
                  "elapsed_s": 2 },
                { "title": "no id" },
                "garbage"
            ]
        });
        let jobs = Job::list_from_activity(Some(&v));
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].state, JobState::Running);
        assert_eq!(jobs[1].state, JobState::Queued);
        assert_eq!(jobs[1].ask_message_id, None);
        // Older relays: no `jobs`, a null activity, or no activity at all.
        assert!(Job::list_from_activity(Some(&serde_json::json!({ "text": "x" }))).is_empty());
        assert!(Job::list_from_activity(Some(&serde_json::Value::Null)).is_empty());
        assert!(Job::list_from_activity(None).is_empty());
    }
}

#[cfg(test)]
mod drawer_tests {
    //! The real `Shell` — transcript, presence row, composer — over a real
    //! `AppState`, headless, with a live job row in the chat and the relay's
    //! `activity.jobs[]` applied (the `transcript_smoke_tests` mount). No
    //! jobs → the plain row; jobs → the chip; a click → the drawer with one
    //! row per job and the contract's words; Steer → the composer replies to
    //! the ask; Stop → `stop { jobId }` for that job only; the hidden job rows
    //! never reach the transcript.
    use futures::future::BoxFuture;
    use gpui::{point, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext};
    use tokio::sync::mpsc;

    use super::*;
    use crate::model::{Chat, Message, Project, SpaceKind};
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

    fn user(id: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "chatId": "c1", "role": "user", "sender": "user", "status": "done",
            "createdAt": at, "completedAt": at,
            "parts": [{ "id": format!("{id}-t"), "kind": "text", "text": text }],
        }))
        .expect("message")
    }

    /// What agentd writes for a job (duplex §2.3): wordless, `worker`, `hidden`,
    /// streaming, its `activity` the presence line.
    fn job_row(id: &str, job_id: &str, title: &str, activity: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "chatId": "c1", "role": "assistant", "sender": "box:1",
            "status": "streaming", "createdAt": at,
            "job": { "id": job_id, "title": title, "kind": "research", "touches": [] },
            "worker": true, "hidden": true, "inReplyTo": "m1", "activity": activity,
            "parts": [{ "id": format!("{id}-s"), "kind": "status", "code": "working", "message": "Working…" }],
        }))
        .expect("message")
    }

    fn relay_jobs() -> Vec<Job> {
        vec![
            Job {
                id: "j1".into(),
                title: "Searching for surf schools".into(),
                state: JobState::Running,
                step: "Reading tripadvisor.com".into(),
                elapsed_s: 41,
                ask_message_id: Some("m1".into()),
                chat_id: Some("c1".into()),
            },
            Job {
                id: "j2".into(),
                title: "Comparing hosting prices".into(),
                state: JobState::Queued,
                step: String::new(),
                elapsed_s: 12,
                ask_message_id: Some("m1".into()),
                chat_id: Some("c1".into()),
            },
        ]
    }

    fn mount(
        cx: &mut TestAppContext,
    ) -> (
        tokio::runtime::Runtime,
        Entity<AppState>,
        Entity<Shell>,
        &mut VisualTestContext,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        let now = crate::model::now_ms();
        state.update(cx, |s, cx| {
            s.projects.push(Project {
                id: "p1".into(),
                name: "Arbos".into(),
                kind: SpaceKind::Project,
                ..Project::default()
            });
            let chat: Chat = serde_json::from_value(serde_json::json!({
                "id": "c1", "project_id": "p1", "title": "Main", "is_pinned": true,
                "is_system": false, "unread": false, "last_message_at": now,
            }))
            .expect("chat");
            s.chats.insert("p1".into(), vec![chat]);
            s.messages.insert(
                "c1".into(),
                vec![
                    user(
                        "m1",
                        "Find the 3 best surf schools near Santa Teresa.",
                        now - 60_000,
                    ),
                    job_row(
                        "m2",
                        "j1",
                        "Searching for surf schools",
                        "Reading tripadvisor.com · 2 tasks",
                        now - 41_000,
                    ),
                    job_row(
                        "m3",
                        "j2",
                        "Comparing hosting prices",
                        "Comparing hosting prices",
                        now - 12_000,
                    ),
                ],
            );
            s.selected_project = Some("p1".into());
            s.active_tab.insert("p1".into(), "c1".into());
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.run_until_parked();
        (rt, state, shell, vcx)
    }

    fn settle(shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        for _ in 0..3 {
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    fn centre(b: Bounds<Pixels>) -> gpui::Point<Pixels> {
        point(b.left() + b.size.width / 2.0, b.top() + b.size.height / 2.0)
    }

    #[gpui::test]
    fn the_row_grows_a_chip_only_once_the_relay_lists_jobs(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        settle(&shell, vcx);
        // The job rows keep the presence row up with the job's words — and are
        // not in the transcript.
        let text = vcx.debug_bounds("presence-text");
        assert!(text.is_some(), "a live job row keeps the presence row up");
        let rows: Vec<String> = shell.read_with(&*vcx, |sh, cx| {
            sh.transcript
                .read(cx)
                .rows
                .iter()
                .map(|r| r.message_id.clone())
                .collect()
        });
        assert!(
            !rows.iter().any(|id| id == "m2" || id == "m3"),
            "hidden job rows draw nothing: {rows:?}"
        );
        assert!(
            vcx.debug_bounds("tasks-chip").is_none(),
            "no listing yet, no chip"
        );
        // The relay's activity record arrives.
        state.update(vcx, |s, cx| {
            s.apply(
                SyncEvent::ProjectJobs {
                    project_id: "p1".into(),
                    jobs: relay_jobs(),
                },
                cx,
            )
        });
        settle(&shell, vcx);
        let chip = vcx
            .debug_bounds("tasks-chip")
            .expect("the chip is on the row");
        let row = vcx.debug_bounds("presence-row").expect("row");
        assert!(
            chip.left() > row.left() && chip.bottom() <= row.bottom() + px(1.0),
            "the chip sits on the presence row: chip {chip:?} row {row:?}"
        );
        assert_eq!(f32::from(chip.size.height), CHIP_HEIGHT);
        assert!(
            vcx.debug_bounds("tasks-drawer").is_none(),
            "closed until clicked"
        );
        // Nothing open any more → the chip goes.
        state.update(vcx, |s, cx| {
            s.apply(
                SyncEvent::ProjectJobs {
                    project_id: "p1".into(),
                    jobs: Vec::new(),
                },
                cx,
            )
        });
        settle(&shell, vcx);
        assert!(vcx.debug_bounds("tasks-chip").is_none(), "no jobs, no chip");
    }

    #[gpui::test]
    fn the_drawer_lists_the_jobs_with_the_contracts_words_and_actions(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        let mut commands = state.update(vcx, |s, _| s.take_command_channel());
        state.update(vcx, |s, cx| {
            s.apply(
                SyncEvent::ProjectJobs {
                    project_id: "p1".into(),
                    jobs: relay_jobs(),
                },
                cx,
            )
        });
        settle(&shell, vcx);
        let chip = vcx.debug_bounds("tasks-chip").expect("chip");
        vcx.simulate_click(centre(chip), Modifiers::default());
        settle(&shell, vcx);
        let drawer = vcx
            .debug_bounds("tasks-drawer")
            .expect("the drawer opens on a click");
        let row = vcx.debug_bounds("presence-row").expect("row");
        assert!(
            drawer.bottom() <= row.top(),
            "the drawer hangs over the row: drawer {drawer:?} row {row:?}"
        );
        assert!(vcx.debug_bounds("tasks-row-0").is_some());
        assert!(vcx.debug_bounds("tasks-row-1").is_some());
        assert!(
            vcx.debug_bounds("tasks-row-2").is_none(),
            "two jobs, two rows"
        );
        // Running: Steer + Stop; queued: Stop only.
        assert!(vcx.debug_bounds("tasks-steer-0").is_some());
        assert!(vcx.debug_bounds("tasks-stop-0").is_some());
        assert!(
            vcx.debug_bounds("tasks-steer-1").is_none(),
            "a queued job cannot be steered"
        );
        assert!(vcx.debug_bounds("tasks-stop-1").is_some());
        let open = state.read_with(&*vcx, |s, _| s.tasks_open.clone());
        assert_eq!(open.as_deref(), Some("c1"));

        // Stop the queued job: `stop { jobId }`, that job only; the row leaves at once.
        let stop = vcx.debug_bounds("tasks-stop-1").expect("stop");
        vcx.simulate_click(centre(stop), Modifiers::default());
        settle(&shell, vcx);
        let sent = commands.try_recv().expect("a command went out");
        match sent {
            ClientCommand::StopJob {
                chat_id,
                job_id,
                message_id,
            } => {
                assert_eq!(chat_id, "c1");
                assert_eq!(job_id, "j2");
                assert_eq!(message_id, "m1", "bound to the newest user message");
            }
            other => panic!("expected StopJob, got {other:?}"),
        }
        assert!(
            vcx.debug_bounds("tasks-row-1").is_none(),
            "the stopped job left the drawer"
        );
        assert!(
            vcx.debug_bounds("tasks-drawer").is_some(),
            "one job still runs"
        );

        // Steer the running job: the composer replies to its ask, the drawer closes, no command.
        let steer = vcx.debug_bounds("tasks-steer-0").expect("steer");
        vcx.simulate_click(centre(steer), Modifiers::default());
        settle(&shell, vcx);
        let reply_to = state.read_with(&*vcx, |s, _| s.chat_ui("c1").reply_to.clone());
        assert_eq!(
            reply_to.as_deref(),
            Some("m1"),
            "Steer = a reply to the job's ask"
        );
        assert!(
            vcx.debug_bounds("tasks-drawer").is_none(),
            "the drawer closed for typing"
        );
        assert!(
            commands.try_recv().is_err(),
            "Steer sends no command of its own"
        );

        // Stop all = today's Stop: `stop {}` and the chat's jobs are forgotten.
        let chip = vcx.debug_bounds("tasks-chip").expect("chip");
        vcx.simulate_click(centre(chip), Modifiers::default());
        settle(&shell, vcx);
        let all = vcx.debug_bounds("tasks-stop-all").expect("stop all");
        vcx.simulate_click(centre(all), Modifiers::default());
        settle(&shell, vcx);
        assert!(
            matches!(commands.try_recv(), Ok(ClientCommand::Stop { chat_id }) if chat_id == "c1")
        );
        assert!(vcx.debug_bounds("tasks-drawer").is_none());
        assert!(
            vcx.debug_bounds("tasks-chip").is_none(),
            "nothing open after Stop all"
        );
    }
}
