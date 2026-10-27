//! Settings → Routines (F3): list, create with presets, pause/resume, run now, history.
//! A floating panel anchored above the rail's settings button; the shell shows it when the
//! "Routines" row of the settings popover is clicked.

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, App, Context, Div, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, Render, SharedString, Window,
};

use crate::app::AppState;
use crate::model::{now_ms, Id, Routine, RoutineRun, RoutineRunStatus, RoutineTrigger};
use crate::ui::composer::ComposerInput;
use crate::ui::error_note;
use crate::ui::frost::{frosted, MENU_BLUR};
use crate::ui::highlight;
use crate::ui::theme::{Theme, PANEL_RADIUS, RAIL_WIDTH};

pub const PANEL_WIDTH: f32 = 560.0;

#[derive(Clone, Debug)]
pub enum RoutinesEvent {
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum View {
    List,
    Create,
    History(Id),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Preset {
    Daily,
    Weekdays,
    Hourly,
    EveryMinute,
    Custom,
    Webhook,
    BoxAwake,
}

impl Preset {
    const ALL: [Preset; 7] = [
        Preset::Daily,
        Preset::Weekdays,
        Preset::Hourly,
        Preset::EveryMinute,
        Preset::Custom,
        Preset::Webhook,
        Preset::BoxAwake,
    ];

    fn label(self) -> &'static str {
        match self {
            Preset::Daily => "Daily 09:00",
            Preset::Weekdays => "Weekdays 09:00",
            Preset::Hourly => "Hourly",
            Preset::EveryMinute => "Every minute",
            Preset::Custom => "Custom cron",
            Preset::Webhook => "Webhook",
            Preset::BoxAwake => "On box awake",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Preset::Daily => "preset-daily",
            Preset::Weekdays => "preset-weekdays",
            Preset::Hourly => "preset-hourly",
            Preset::EveryMinute => "preset-minute",
            Preset::Custom => "preset-custom",
            Preset::Webhook => "preset-webhook",
            Preset::BoxAwake => "preset-awake",
        }
    }

    fn trigger(self, custom_expr: &str) -> Option<RoutineTrigger> {
        let cron = |expr: &str| RoutineTrigger::Cron {
            expr: expr.to_string(),
            tz: "UTC".into(),
        };
        match self {
            Preset::Daily => Some(cron("0 9 * * *")),
            Preset::Weekdays => Some(cron("0 9 * * 1-5")),
            Preset::Hourly => Some(cron("0 * * * *")),
            Preset::EveryMinute => Some(cron("* * * * *")),
            Preset::Custom => {
                let expr = custom_expr.trim();
                (expr.split_whitespace().count() == 5 || expr.split_whitespace().count() == 6)
                    .then(|| cron(expr))
            }
            Preset::Webhook => Some(RoutineTrigger::Webhook {
                secret: String::new(),
            }),
            Preset::BoxAwake => Some(RoutineTrigger::Event {
                event: "box_awake".into(),
            }),
        }
    }
}

pub struct RoutinesPanel {
    state: Entity<AppState>,
    view: View,
    preset: Preset,
    /// Chat the prompt lands in; `None` = the pinned chat.
    chat_id: Option<Id>,
    name_input: Entity<ComposerInput>,
    prompt_input: Entity<ComposerInput>,
    cron_input: Entity<ComposerInput>,
    error: Option<String>,
    /// Shown as a page inside the Settings sheet (#1086) rather than as the
    /// bottom-left popover; the sheet's header carries the back button.
    pub(crate) embedded: bool,
    focus_handle: FocusHandle,
}

impl EventEmitter<RoutinesEvent> for RoutinesPanel {}

impl RoutinesPanel {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let name_input = cx.new(ComposerInput::new);
        let prompt_input = cx.new(ComposerInput::new);
        let cron_input = cx.new(ComposerInput::new);
        name_input.update(cx, |i, _| i.set_placeholder("Name, e.g. Morning stand-up"));
        prompt_input.update(cx, |i, _| {
            i.set_placeholder("Prompt the agent runs, e.g. Summarise open PRs waiting on me")
        });
        cron_input.update(cx, |i, _| {
            i.set_placeholder("min hour dom month dow · 0 9 * * 1-5")
        });
        {
            let weak = cx.weak_entity();
            prompt_input.update(cx, |input, _| {
                input.on_submit = Some(Rc::new(move |_window, cx| {
                    let _ = weak.update(cx, |this, cx| this.submit_create(cx));
                }));
            });
        }
        for input in [&name_input, &prompt_input, &cron_input] {
            cx.observe(input, |_, _, cx| cx.notify()).detach();
        }
        Self {
            state,
            view: View::List,
            preset: Preset::Weekdays,
            chat_id: None,
            name_input,
            prompt_input,
            cron_input,
            error: None,
            embedded: false,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Called by the shell when the panel opens: list view, fresh data.
    pub fn open(&mut self, cx: &mut Context<Self>) {
        self.view = View::List;
        self.error = None;
        self.state.update(cx, |s, cx| s.routines_refresh(cx));
        cx.notify();
    }

    /// True when one of the panel's text fields has keyboard focus (the shell must not steal it).
    pub fn has_focus(&self, window: &Window, cx: &App) -> bool {
        [&self.name_input, &self.prompt_input, &self.cron_input]
            .iter()
            .any(|i| i.read(cx).focus_handle(cx).is_focused(window))
    }

    fn submit_create(&mut self, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).text().trim().to_string();
        let prompt = self.prompt_input.read(cx).text().trim().to_string();
        let custom = self.cron_input.read(cx).text().to_string();
        if name.is_empty() {
            self.error = Some("Give the routine a name.".into());
            cx.notify();
            return;
        }
        if prompt.is_empty() {
            self.error = Some("Write the prompt the agent should run.".into());
            cx.notify();
            return;
        }
        let Some(trigger) = self.preset.trigger(&custom) else {
            self.error = Some("A cron expression has five fields: min hour dom month dow.".into());
            cx.notify();
            return;
        };
        let chat_id = match self.chat_id.clone() {
            Some(c) => c,
            None => {
                let state = self.state.read(cx);
                let Some(project_id) = state.selected_project.clone() else {
                    return;
                };
                let tabs = state.visible_tabs(&project_id);
                match tabs.iter().find(|c| c.is_pinned).or(tabs.first()) {
                    Some(c) => c.id.clone(),
                    None => return,
                }
            }
        };
        self.state.update(cx, |s, cx| {
            s.routine_create(name, prompt, chat_id, trigger, cx);
        });
        self.name_input.update(cx, |i, cx| i.clear(cx));
        self.prompt_input.update(cx, |i, cx| i.clear(cx));
        self.cron_input.update(cx, |i, cx| i.clear(cx));
        self.error = None;
        self.view = View::List;
        cx.notify();
    }

    fn button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        primary: bool,
        theme: &Theme,
    ) -> gpui::Stateful<Div> {
        div()
            .id(id)
            .flex_shrink_0()
            .px(px(9.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .text_size(px(11.5))
            .cursor_pointer()
            .when(primary, |el| el.bg(theme.text).text_color(theme.bg))
            .when(!primary, |el| {
                highlight::button(
                    el.text_color(theme.text_muted)
                        .border_1()
                        .border_color(theme.border),
                    theme,
                )
            })
            .child(label.into())
    }

    fn chip(label: impl Into<SharedString>, color: gpui::Hsla, theme: &Theme) -> Div {
        div()
            .px(px(7.0))
            .py(px(2.0))
            .rounded_full()
            .text_size(px(10.5))
            .bg(theme.surface_2)
            .text_color(color)
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(label.into())
    }

    fn header(&self, title: String, right: Vec<gpui::AnyElement>, theme: &Theme) -> Div {
        let mut row = div().flex().items_center().justify_between().child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(title),
        );
        let mut actions = div().flex().gap(px(6.0));
        for r in right {
            actions = actions.child(r);
        }
        row = row.child(actions);
        row
    }

    fn render_list(&self, cx: &mut Context<Self>) -> Div {
        let theme = Theme::of(cx).clone();
        let state = self.state.read(cx);
        let project = state.selected();
        let project_name = project.map(|p| p.name.clone()).unwrap_or_default();
        let routines: Vec<Routine> = project
            .map(|p| state.routines_of(&p.id).to_vec())
            .unwrap_or_default();
        let tabs: Vec<(Id, String)> = project
            .map(|p| {
                state
                    .tabs(&p.id)
                    .iter()
                    .map(|c| (c.id.clone(), c.title.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let now = now_ms() as i64;

        let mut list = div()
            .id("routines-list")
            .flex()
            .flex_col()
            .gap(px(6.0))
            .max_h(px(380.0))
            .overflow_y_scroll();
        if routines.is_empty() {
            list = list.child(
                div()
                    .py(px(18.0))
                    .text_size(px(12.0))
                    .text_color(theme.text_muted)
                    .text_center()
                    .child("No routines yet. A routine is a prompt that runs on a schedule, on a webhook, or when the box wakes — while you are away."),
            );
        }
        for r in routines {
            let chat_title = tabs
                .iter()
                .find(|(id, _)| id == &r.chat_id)
                .map(|(_, t)| t.clone())
                .unwrap_or_else(|| "closed chat".into());
            let timing = match (r.enabled, r.next_run_at, r.last_run_at) {
                (true, Some(next), _) => format!("next {}", rel_time(next, now)),
                (true, None, Some(last)) => format!("last {}", rel_time(last, now)),
                (true, None, None) => "never ran".to_owned(),
                (false, _, Some(last)) => format!("paused · last {}", rel_time(last, now)),
                (false, _, None) => "paused".to_owned(),
            };
            let id_toggle = r.id.clone();
            let id_run = r.id.clone();
            let id_hist = r.id.clone();
            let id_del = r.id.clone();
            let enabled = r.enabled;
            let row = div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .px(px(10.0))
                .py(px(8.0))
                .rounded(px(8.0))
                .bg(theme.surface_1)
                .border_1()
                .border_color(theme.border)
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
                                .gap(px(6.0))
                                .min_w_0()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(if enabled {
                                            theme.text
                                        } else {
                                            theme.text_muted
                                        })
                                        .child(r.name.clone()),
                                )
                                .child(Self::chip(r.trigger.label(), theme.text_muted, &theme)),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.text_faint)
                                .child(format!("{timing} · in “{chat_title}”")),
                        ),
                )
                .child(
                    self.button(
                        ElementId::Name(format!("rt-toggle-{}", r.id).into()),
                        if enabled { "On" } else { "Paused" },
                        enabled,
                        &theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |s, cx| {
                            s.routine_set_enabled(id_toggle.clone(), !enabled, cx)
                        });
                    })),
                )
                .child(
                    self.button(
                        ElementId::Name(format!("rt-run-{}", r.id).into()),
                        "Run now",
                        false,
                        &theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state
                            .update(cx, |s, cx| s.routine_run_now(id_run.clone(), cx));
                    })),
                )
                .child(
                    self.button(
                        ElementId::Name(format!("rt-hist-{}", r.id).into()),
                        "History",
                        false,
                        &theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state
                            .update(cx, |s, cx| s.routine_load_runs(id_hist.clone(), cx));
                        this.view = View::History(id_hist.clone());
                        cx.notify();
                    })),
                )
                .child(
                    div()
                        .id(ElementId::Name(format!("rt-del-{}", r.id).into()))
                        .px(px(6.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .text_size(px(11.5))
                        .text_color(theme.text_faint)
                        .cursor_pointer()
                        // error-ui: exception destructive-action (Delete on hover)
                        .hover(|s| s.text_color(theme.danger).bg(theme.danger_soft)) // hover-fill: exception danger
                        .child("Delete")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state
                                .update(cx, |s, cx| s.routine_delete(id_del.clone(), cx));
                        })),
                );
            list = list.child(row);
        }

        let header = self.header(
            format!("Routines · {project_name}"),
            vec![
                self.button("rt-new", "New routine", true, &theme)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.view = View::Create;
                        this.error = None;
                        let handle = this.name_input.read(cx).focus_handle(cx);
                        window.focus(&handle, cx);
                        cx.notify();
                    }))
                    .into_any_element(),
                self.button("rt-close", "Close", false, &theme)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(RoutinesEvent::Close)))
                    .into_any_element(),
            ],
            &theme,
        );
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(header)
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.text_faint)
                    .child("The relay scheduler wakes the box and posts the prompt into the chat as a message from the routine; replies land there like any other turn."),
            )
            .child(list)
    }

    fn render_create(&self, cx: &mut Context<Self>) -> Div {
        let theme = Theme::of(cx).clone();
        let state = self.state.read(cx);
        let tabs: Vec<(Id, String, bool)> = state
            .selected_project
            .as_deref()
            .map(|p| {
                state
                    .visible_tabs(p)
                    .iter()
                    .map(|c| (c.id.clone(), c.title.clone(), c.is_pinned))
                    .collect()
            })
            .unwrap_or_default();
        let selected_chat = self.chat_id.clone().or_else(|| {
            tabs.iter()
                .find(|(_, _, pinned)| *pinned)
                .or(tabs.first())
                .map(|(id, _, _)| id.clone())
        });

        let field = |label: &'static str, input: Entity<ComposerInput>, theme: &Theme| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.text_faint)
                        .child(label),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .min_h(px(32.0))
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded(px(8.0))
                        .bg(theme.surface_2)
                        .border_1()
                        .border_color(theme.border)
                        .child(input),
                )
        };

        let mut presets = div().flex().flex_wrap().gap(px(4.0));
        for p in Preset::ALL {
            let selected = p == self.preset;
            presets = presets.child(
                div()
                    .id(p.id())
                    .px(px(9.0))
                    .py(px(4.0))
                    .rounded_full()
                    .text_size(px(11.5))
                    .cursor_pointer()
                    // hover-fill: exception choice
                    .bg(if selected {
                        theme.accent_soft
                    } else {
                        theme.surface_2
                    })
                    .text_color(if selected {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .border_1()
                    .border_color(if selected { theme.accent } else { theme.border })
                    .hover(|s| s.border_color(theme.border_strong))
                    .child(p.label())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.preset = p;
                        if p == Preset::Custom {
                            let handle = this.cron_input.read(cx).focus_handle(cx);
                            window.focus(&handle, cx);
                        }
                        cx.notify();
                    })),
            );
        }

        let mut chats = div().flex().flex_wrap().gap(px(4.0));
        for (id, title, _) in &tabs {
            let selected = selected_chat.as_deref() == Some(id.as_str());
            let pick = id.clone();
            chats = chats.child(
                div()
                    .id(ElementId::Name(format!("rt-chat-{id}").into()))
                    .px(px(9.0))
                    .py(px(4.0))
                    .rounded_full()
                    .text_size(px(11.5))
                    .cursor_pointer()
                    // hover-fill: exception choice
                    .bg(if selected {
                        theme.accent_soft
                    } else {
                        theme.surface_2
                    })
                    .text_color(if selected {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .border_1()
                    .border_color(if selected { theme.accent } else { theme.border })
                    .child(title.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.chat_id = Some(pick.clone());
                        cx.notify();
                    })),
            );
        }

        let header = self.header(
            "New routine".into(),
            vec![self
                .button("rt-back", "Back", false, &theme)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.view = View::List;
                    this.error = None;
                    cx.notify();
                }))
                .into_any_element()],
            &theme,
        );
        // The form's note (validation, as a refusal) under the header, before the first field.
        let note = self.error.as_ref().map(|words| {
            let slot = crate::app::refused("create the routine", words);
            error_note::render(
                "routine-error",
                &slot.error,
                false,
                error_note::Style::Inline,
                &theme,
                error_note::Handlers::new(),
            )
        });
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(header)
            .children(note)
            .child(field("NAME", self.name_input.clone(), &theme))
            .child(field(
                "PROMPT · Enter creates",
                self.prompt_input.clone(),
                &theme,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme.text_faint)
                            .child("WHEN"),
                    )
                    .child(presets),
            );
        if self.preset == Preset::Custom {
            body = body.child(field(
                "CRON EXPRESSION (UTC)",
                self.cron_input.clone(),
                &theme,
            ));
        }
        if self.preset == Preset::Webhook {
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .child("The relay mints a secret URL; POST to it (any JSON body becomes trigger context) to run the routine."),
            );
        }
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.text_faint)
                        .child("CHAT"),
                )
                .child(chats),
        );
        body.child(
            div()
                .flex()
                .justify_end()
                .gap(px(6.0))
                .child(
                    self.button("rt-cancel", "Cancel", false, &theme)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.view = View::List;
                            this.error = None;
                            cx.notify();
                        })),
                )
                .child(
                    self.button("rt-create", "Create", true, &theme)
                        .on_click(cx.listener(|this, _, _, cx| this.submit_create(cx))),
                ),
        )
    }

    fn render_history(&self, routine_id: &str, cx: &mut Context<Self>) -> Div {
        let theme = Theme::of(cx).clone();
        let state = self.state.read(cx);
        let routine = state
            .selected_project
            .as_deref()
            .and_then(|p| state.routines_of(p).iter().find(|r| r.id == routine_id))
            .cloned();
        let name = routine
            .as_ref()
            .map(|r| r.name.clone())
            .unwrap_or_else(|| "Routine".into());
        let runs: Vec<RoutineRun> = state.routine_runs_of(routine_id).to_vec();
        let now = now_ms() as i64;

        let header = self.header(
            format!("History · {name}"),
            vec![self
                .button("rt-hist-back", "Back", false, &theme)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.view = View::List;
                    cx.notify();
                }))
                .into_any_element()],
            &theme,
        );
        let mut list = div()
            .id("routine-runs")
            .flex()
            .flex_col()
            .gap(px(4.0))
            .max_h(px(400.0))
            .overflow_y_scroll();
        if runs.is_empty() {
            list = list.child(
                div()
                    .py(px(14.0))
                    .text_size(px(12.0))
                    .text_color(theme.text_muted)
                    .child("No runs yet."),
            );
        }
        for run in runs.iter().take(20) {
            let color = match run.status {
                RoutineRunStatus::Done => theme.accent,
                RoutineRunStatus::Failed => theme.danger,
                RoutineRunStatus::Running | RoutineRunStatus::Queued => theme.warn,
                RoutineRunStatus::Skipped | RoutineRunStatus::Other => theme.text_faint,
            };
            let duration = match run.finished_at {
                Some(f) => format!("{:.0} s", ((f - run.started_at) as f64 / 1000.0).max(0.0)),
                None => "…".to_owned(),
            };
            let rid = routine_id.to_string();
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .bg(theme.surface_1)
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .w(px(84.0))
                        .text_size(px(11.0))
                        .text_color(theme.text_muted)
                        .child(rel_time(run.started_at, now)),
                )
                .child(Self::chip(run.status.label(), color, &theme))
                .child(
                    div()
                        .w(px(40.0))
                        .text_size(px(11.0))
                        .text_color(theme.text_faint)
                        .child(duration),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(11.5))
                        .text_color(theme.text)
                        .overflow_hidden()
                        .child(
                            run.note
                                .clone()
                                .unwrap_or_default()
                                .chars()
                                .take(72)
                                .collect::<String>(),
                        ),
                );
            if run.message_id.is_some() {
                row = row.child(
                    div()
                        .id(ElementId::Name(format!("rt-open-{}", run.id).into()))
                        .text_size(px(11.5))
                        .text_color(theme.accent)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .child("open")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state
                                .update(cx, |s, cx| s.open_chat_of_routine(&rid, cx));
                            cx.emit(RoutinesEvent::Close);
                        })),
                );
            }
            list = list.child(row);
        }
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(header)
            .child(list)
    }
}

/// "in 19 h", "5 h ago", "40 min ago", "just now".
pub fn rel_time(at_ms: i64, now_ms: i64) -> String {
    let delta = at_ms - now_ms;
    let future = delta > 0;
    let secs = delta.abs() / 1000;
    let body = if secs < 45 {
        return if future {
            "in a moment".into()
        } else {
            "just now".into()
        };
    } else if secs < 3_600 {
        format!("{} min", (secs + 30) / 60)
    } else if secs < 86_400 {
        format!("{} h", (secs + 1_800) / 3_600)
    } else {
        format!("{} d", (secs + 43_200) / 86_400)
    };
    if future {
        format!("in {body}")
    } else {
        format!("{body} ago")
    }
}

impl Focusable for RoutinesPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RoutinesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let body = match self.view.clone() {
            View::List => self.render_list(cx),
            View::Create => self.render_create(cx),
            View::History(id) => self.render_history(&id, cx),
        };
        let card = div()
            .id("routines-panel")
            .occlude()
            .flex()
            .flex_col()
            .p(px(12.0))
            .child(body);
        if self.embedded {
            return card.w_full().into_any_element();
        }
        let card = card
            .w(px(PANEL_WIDTH))
            .rounded(px(PANEL_RADIUS))
            .bg(theme.card())
            .border_1()
            .border_color(theme.border_strong)
            .shadow_lg();
        div()
            .absolute()
            .left(px(RAIL_WIDTH + 8.0))
            .bottom(px(14.0))
            .child(frosted(PANEL_RADIUS, MENU_BLUR, card))
            .into_any_element()
    }
}
