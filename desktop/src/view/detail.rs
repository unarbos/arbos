//! The detail column: whichever pane is showing, and everything the turn in
//! flight stacks under it — plan, permission, queue, composer.

use crate::{
    kernel,
    model::{
        attachment::Prompt,
        project::Project,
        session::{self, ChatItem, ChatSession, Choice, Connection, PlanNode},
        settings,
    },
    view::{
        component::{composer, composer::SessionDrag, menu::Menu, surface as board, transcript},
        root::{self, Arbos, NewSession, Pane},
        sidebar::{self, Renaming},
    },
};
use bezel::{
    gpui::{
        AnyElement, App, ClickEvent, Context, ExternalPaths, FocusHandle, Focusable as _,
        SharedString, Window, div, prelude::*, px, svg,
    },
    motion::{Fade, Painter},
    theme::{TextStyle, Theme, Typeset},
    ui::{
        icons, surface,
        tooltip::Tooltip,
        widgets::{ButtonStyle, Buttons, Content, Controls},
    },
};
use cacp::schema::{
    PermissionOptionKind, SessionConfigKind, SessionConfigOptionCategory, SessionConfigOptionValue,
    SessionConfigSelectOption, SessionConfigSelectOptions, SessionModeState,
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use surface::Surfaced as _;

/// A process-journal re-read is already scheduled. One window, one clock.
static TAIL_PENDING: AtomicBool = AtomicBool::new(false);

/// What the live session can be switched between, flattened to the one shape
/// the composer draws: its config options, then its modes.
///
/// Config first, because the model is a config option and it is the one people
/// came for. Booleans are left out — a menu of two rows is a toggle wearing a
/// menu, and the agent menu has nowhere to put a real one yet.
fn switches(chat: Option<&ChatSession>, catalog: &kernel::ModelsCatalog) -> Vec<composer::Switch> {
    let mut switches: Vec<composer::Switch> = chat
        .map(|chat| {
            chat.config
                .iter()
                .filter_map(|option| {
                    let SessionConfigKind::Select(select) = &option.kind else {
                        return None;
                    };
                    let options = select_options(&select.options);
                    // A select with nothing to select is a menu with no rows.
                    (!options.is_empty()).then(|| composer::Switch {
                        id: composer::SwitchId::Config(option.id.to_string().into()),
                        name: option.name.clone().into(),
                        current: Some(select.current_value.to_string().into()),
                        options,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(chat) = chat
        && let Some(modes) = chat
            .modes
            .as_ref()
            .filter(|modes| !modes.available_modes.is_empty())
            .filter(|modes| !covered_by_config(chat, modes))
    {
        switches.push(composer::Switch {
            id: composer::SwitchId::Mode,
            name: "Mode".into(),
            current: Some(modes.current_mode_id.to_string().into()),
            options: modes
                .available_modes
                .iter()
                .map(|mode| composer::SwitchOption {
                    id: mode.id.to_string().into(),
                    name: mode.name.clone().into(),
                })
                .collect(),
        });
    }
    // Host-wide catalog from this place's HTTP gateway. Used whenever the
    // chat did not already bring a model select with rows — including when
    // there is no live chat, and when that select exists but is empty.
    if !chat.is_some_and(has_usable_model_config) {
        let current = chat
            .and_then(|chat| chat.model.clone())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| catalog.current.clone());
        switches.insert(
            0,
            composer::Switch {
                id: composer::SwitchId::Model,
                name: "Model".into(),
                current: (!current.is_empty()).then(|| current.into()),
                options: catalog
                    .models
                    .iter()
                    .map(|model| composer::SwitchOption {
                        id: model.id.clone().into(),
                        name: model.name.clone().into(),
                    })
                    .collect(),
            },
        );
    }
    switches
}

fn has_usable_model_config(chat: &ChatSession) -> bool {
    chat.config.iter().any(|option| {
        let model = option.category == Some(SessionConfigOptionCategory::Model)
            || option.id.eq_ignore_ascii_case("model");
        if !model {
            return false;
        }
        let SessionConfigKind::Select(select) = &option.kind else {
            return false;
        };
        !select_options(&select.options).is_empty()
    })
}

/// Whether the agent is already offering these modes as a config option.
///
/// Some agents report their modes twice — once through `session/new`'s `modes`
/// and again as a config option — and two switches onto one piece of state is
/// two ways to disagree about it. The config option wins: it is the general
/// mechanism, and its update is what confirms a change.
///
/// Matched on the values as well as on the category, because the category is
/// optional and an agent that leaves it off still sends the same list twice.
fn covered_by_config(chat: &ChatSession, modes: &SessionModeState) -> bool {
    chat.config.iter().any(|option| {
        if option.category == Some(SessionConfigOptionCategory::Mode) {
            return true;
        }
        let SessionConfigKind::Select(select) = &option.kind else {
            return false;
        };
        let values = select_options(&select.options);
        modes
            .available_modes
            .iter()
            .all(|mode| values.iter().any(|value| value.id.as_ref() == &*mode.id))
    })
}

/// A select's values, with a group's rows folded in beside the ungrouped ones.
/// A switch gets one flat card, and a group is a heading it has nowhere to put.
fn select_options(options: &SessionConfigSelectOptions) -> Vec<composer::SwitchOption> {
    fn one(option: &SessionConfigSelectOption) -> composer::SwitchOption {
        composer::SwitchOption {
            id: option.value.to_string().into(),
            name: option.name.clone().into(),
        }
    }
    match options {
        SessionConfigSelectOptions::Ungrouped(options) => options.iter().map(one).collect(),
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| group.options.iter().map(one))
            .collect(),
    }
}

/// One of the two answers a permission request comes down to, with the *once*
/// and *always* forms of it read as one.
struct Verdict<'a> {
    /// What the button says: the once-form's own label, whatever the checkbox
    /// is set to. Agents word the always-form as a sentence — "Yes, and allow
    /// access to repos/ and ls commands" — and a sentence is not a button.
    label: &'a str,
    once: &'a str,
    always: Option<&'a str>,
}

impl Verdict<'_> {
    /// The option id this answer sends with the checkbox in that state. An
    /// always with no form to send falls back to the once: an agent offering
    /// "allow always" and no "reject always" still has to be refusable.
    fn id(&self, always: bool) -> String {
        match always {
            true => self.always.unwrap_or(self.once).to_owned(),
            false => self.once.to_owned(),
        }
    }
}

/// The request as an alert — a yes, a no, and a checkbox — or `None` when it
/// is not one.
///
/// ACP's four option kinds are two answers times "for how long", which is the
/// macOS permission alert exactly. It holds only while the two sides account
/// for every option the agent sent: the count is what catches a second option
/// of a kind already taken, and a kind we do not know. Dropping something the
/// agent asked about is not ours to do, so anything else falls to the stack.
fn alert(options: &[Choice]) -> Option<(Verdict<'_>, Verdict<'_>)> {
    let deny = verdict(options, false)?;
    let allow = verdict(options, true)?;
    let covered = 2 + usize::from(deny.always.is_some()) + usize::from(allow.always.is_some());
    (covered == options.len()).then_some((deny, allow))
}

/// One side of the request, if the agent offered its once-form. Without one
/// there is no button to put the checkbox under.
fn verdict(options: &[Choice], allow: bool) -> Option<Verdict<'_>> {
    let (once, ever) = match allow {
        true => (
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
        ),
        false => (
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ),
    };
    let of = |kind: PermissionOptionKind| options.iter().find(move |o| o.kind == kind);
    let once = of(once)?;
    Some(Verdict {
        label: &once.name,
        once: &once.id,
        always: of(ever).map(|option| option.id.as_str()),
    })
}

impl Arbos {
    pub fn composer_focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.focus_handle(cx)
    }

    /// Put the caret in the send field after a chat is minted. The composer
    /// is drawn only over a resumable session, so a create on this tick has
    /// no field in the tree yet — wait a frame, then focus. A rename in the
    /// sidebar keeps the name field.
    pub(crate) fn focus_composer_after_create(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.renaming.is_some() {
            return;
        }
        cx.on_next_frame(window, |this, window, cx| {
            if this.renaming.is_some() {
                return;
            }
            if !this
                .workspace
                .read(cx)
                .active_session()
                .is_some_and(ChatSession::resumable)
            {
                return;
            }
            window.focus(&this.composer_focus_handle(cx), cx);
        });
    }

    /// Run `f` on the active session. Every composer action is this shape:
    /// the view knows which session is in front, the model owns it.
    fn with_active(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut ChatSession)) {
        let Some(id) = self.workspace.read(cx).active_id() else {
            return;
        };
        self.workspace
            .update(cx, |workspace, cx| workspace.with_session(id, cx, f));
    }

    pub(crate) fn submit(&mut self, text: Prompt, cx: &mut Context<Self>) {
        let Some(id) = self.workspace.read(cx).active_id() else {
            return;
        };
        if self.builtin_command(id, &text.text, cx) {
            return;
        }
        if self
            .workspace
            .read(cx)
            .active_session()
            .is_some_and(|chat| chat.questions.is_some())
        {
            self.workspace.update(cx, |workspace, cx| {
                workspace.with_session(id, cx, |chat| chat.answer_ask(&text.text, false));
            });
            return;
        }
        // A plan question picked in the strip: this send is its answer.
        let answering = self
            .workspace
            .read(cx)
            .active_session()
            .and_then(|chat| chat.answering);
        if let Some(node) = answering {
            self.workspace.update(cx, |workspace, cx| {
                workspace.with_session(id, cx, |chat| {
                    chat.plan_op(node, "answer", &text.text);
                    chat.answering = None;
                });
            });
            return;
        }
        self.workspace
            .update(cx, |workspace, cx| workspace.send(id, text, cx));
    }

    /// `/compact`, `/undo`, `/stop`, `/model <id>`, `/mode <id>`, `/pause`,
    /// `/resume`, `/fork`: the kernel's own verbs, sent as frames rather
    /// than as a prompt. True when the text was one of them.
    fn builtin_command(&mut self, id: u64, text: &str, cx: &mut Context<Self>) -> bool {
        let Some(rest) = text.trim().strip_prefix('/') else {
            return false;
        };
        let mut parts = rest.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or("").to_ascii_lowercase();
        let arg = parts.next().map(str::trim).unwrap_or("");
        match name.as_str() {
            "compact" => {
                self.workspace.update(cx, |workspace, cx| {
                    workspace.with_session(id, cx, |chat| chat.compact())
                });
                true
            }
            "undo" => {
                self.workspace.update(cx, |workspace, cx| {
                    workspace.with_session(id, cx, |chat| chat.undo_checkpoint())
                });
                true
            }
            "stop" => {
                self.cancel_turn(cx);
                true
            }
            "pause" | "resume" => {
                let paused = name == "pause";
                self.workspace.update(cx, |workspace, cx| {
                    workspace.with_session(id, cx, |chat| chat.set_paused(paused))
                });
                true
            }
            "model" if !arg.is_empty() => {
                self.workspace.update(cx, |workspace, cx| {
                    workspace.set_session_model(id, arg.to_string(), cx)
                });
                true
            }
            // The permission mode has no chip in the composer; this is the
            // one way to change it from the window.
            "mode" if matches!(arg, "auto" | "ask" | "plan") => {
                self.workspace.update(cx, |workspace, cx| {
                    workspace.set_session_mode(id, arg.to_string(), cx)
                });
                true
            }
            "fork" => {
                self.workspace
                    .update(cx, |workspace, cx| workspace.fork_session(id, cx));
                true
            }
            _ => false,
        }
    }

    pub(crate) fn cancel_turn(&mut self, cx: &mut Context<Self>) {
        if self
            .workspace
            .read(cx)
            .active_session()
            .is_some_and(|chat| chat.questions.is_some())
        {
            self.with_active(cx, |chat| chat.skip_ask());
            return;
        }
        let Some(id) = self.workspace.read(cx).active_id() else {
            return;
        };
        self.workspace
            .update(cx, |workspace, cx| workspace.cancel(id, cx));
    }

    /// Stop the running turn and send the queue plus `extra` (composer
    /// text the Force disc already took) as the next message on this chat.
    pub(crate) fn force_turn(&mut self, extra: Prompt, cx: &mut Context<Self>) {
        let Some(id) = self.workspace.read(cx).active_id() else {
            return;
        };
        self.workspace
            .update(cx, |workspace, cx| workspace.force(id, extra, cx));
    }

    /// Leftover handler. The composer chip is a model picker and never
    /// emits [`composer::ComposerEvent::Agent`].
    pub(crate) fn pick_agent(&mut self, _ix: usize, _cx: &mut Context<Self>) {}

    pub(crate) fn queue_draft_flush(&mut self, cx: &mut Context<Self>) {
        self.draft_flush = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = this.update(cx, |this, cx| this.flush_composer_draft(cx));
        });
    }

    /// Write the line in the composer to disk. Called when the window
    /// leaves the front, so a kill still has the last words.
    pub(crate) fn flush_composer_draft(&mut self, cx: &mut App) {
        let held = self.composer.read(cx).content(cx);
        let Some(id) = self.workspace.read(cx).active_id() else {
            return;
        };
        self.workspace.update(cx, |workspace, _| {
            if let Some(chat) = workspace.session_mut(id)
                && chat.draft != held
            {
                chat.draft = held;
                chat.flush();
            }
        });
    }

    /// What the composer needs from the session it is pointed at: commands,
    /// whether a turn is in flight, and the model switch.
    pub(crate) fn sync_composer(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace.read(cx);
        let kernel = settings::kernel_agent();
        let agents: Vec<composer::Agent> = vec![composer::Agent {
            name: kernel.name.clone().into(),
            icon: workspace.agent_icon(&kernel.name),
        }];
        let chat = workspace.active_session();
        let asking_other = chat.is_some_and(|chat| {
            chat.questions.as_ref().is_some_and(|prompt| {
                prompt
                    .current()
                    .is_some_and(|question| prompt.draft(&question.id).other)
            })
        });
        // Cursor's composer hint after a turn: "Send follow-up".
        let placeholder = match chat.map(|chat| &chat.connection) {
            Some(Connection::Connecting) => "connecting…",
            Some(Connection::Reconnecting(_)) => "reconnecting…",
            Some(Connection::Lost) => "reconnecting…",
            _ if asking_other => "Your answer…",
            _ if chat.is_some_and(|chat| chat.questions.is_some()) => "Add more optional details",
            _ if chat.is_some_and(|chat| chat.answering.is_some()) => {
                "Your answer to the plan question…"
            }
            // Cursor: a fresh chat invites; one with a turn asks for the next.
            _ if chat.is_some_and(|chat| chat.items.is_empty()) => "Plan, search, build anything",
            _ => "Send follow-up",
        }
        .to_owned();
        let commands = workspace.slash_commands.clone();
        let streaming = chat.is_some_and(|chat| chat.busy());
        let queued = chat.is_some_and(|chat| !chat.queue.is_empty());
        let reconnect = matches!(chat.map(|chat| &chat.connection), Some(Connection::Lost));
        let current = chat.map(|_| 0);
        let live = chat.filter(|chat| chat.live());
        let switches = switches(chat, &workspace.models);
        let model_note = workspace.models.error.clone();
        let usage = live.and_then(|chat| chat.usage);
        let next_id = chat.map(|chat| chat.id);
        let next_draft = chat.map(|chat| chat.draft.clone()).unwrap_or_default();
        let pushed = chat.is_some_and(|chat| chat.draft_pushed);
        let (old_id, held) = self
            .composer
            .update(cx, |composer, cx| (composer.bound(), composer.content(cx)));
        if pushed && old_id == next_id {
            // The model set the draft (a rewind handed the prompt back):
            // the composer takes it over whatever was typed.
            if let Some(id) = next_id {
                self.workspace.update(cx, |workspace, _| {
                    if let Some(chat) = workspace.session_mut(id) {
                        chat.draft_pushed = false;
                    }
                });
            }
            self.composer.update(cx, |composer, cx| {
                composer.take_draft(&next_draft, cx);
            });
        }
        if old_id != next_id {
            if let Some(old) = old_id {
                self.workspace.update(cx, |workspace, _| {
                    if let Some(chat) = workspace.session_mut(old) {
                        if chat.draft != held {
                            chat.draft = held;
                            chat.flush();
                        }
                    }
                });
            }
            self.composer.update(cx, |composer, cx| {
                composer.bind_session(next_id, &next_draft, cx);
            });
        }
        self.composer.update(cx, |composer, cx| {
            composer.set_placeholder(&placeholder, cx);
            composer.set_commands(&commands, cx);
            composer.set_streaming(streaming, cx);
            composer.set_queued(queued, cx);
            composer.set_reconnect(reconnect, cx);
            composer.set_agents(&agents, current, cx);
            composer.set_model_note(&model_note, cx);
            composer.set_switches(&switches, cx);
            composer.set_usage(usage, cx);
        });
        let fill = self
            .workspace
            .update(cx, |workspace, _| workspace.pending_composer.take());
        if let Some(text) = fill {
            self.composer.update(cx, |composer, cx| {
                composer.replace_text(&text, cx);
            });
        }
    }

    pub(crate) fn detail(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = Theme::of(cx).clone();
        // Any open chat keeps a send field — archived ones too. Typing
        // reconnects; hiding the card is what made a finished turn look
        // like the box had vanished.
        let showing = self.showing(cx);
        let show_composer =
            showing == Some(Pane::Chat) && self.workspace.read(cx).active_session().is_some();
        let body = match showing {
            None => self.launch(cx),
            Some(Pane::Chat) => self.conversation(window, cx),
            Some(Pane::Surface) => self.surface_pane(window, cx),
        };

        let header = show_composer.then(|| self.chat_header(&theme, window, cx));
        let empty_chat = show_composer
            && self
                .workspace
                .read(cx)
                .active_session()
                .is_some_and(|chat| chat.items.is_empty());
        let content = div()
            // An empty chat gives the composer the middle of the column:
            // the body shrinks to the top half and the composer follows.
            .when(!empty_chat, |el| el.flex_1())
            .when(empty_chat, |el| {
                el.h(px(0.)).flex_grow(1.).flex_basis(px(0.))
            })
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .children(header)
            .child(body);
        let context_row = show_composer.then(|| self.context_row(&theme, cx));

        let context_panel = show_composer
            .then(|| self.context_panel(&theme, cx))
            .flatten()
            .filter(|_| f32::from(window.viewport_size().width) >= CONTEXT_PANEL_MIN_WINDOW);
        div()
            .flex_1()
            .min_w_0()
            .relative()
            .bg(root::content_bg(&theme))
            .flex()
            .flex_row()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(content)
                    // In flow, with a floor height. An absolute overlay collapsed to
                    // 0 when the entity panicked or measured empty, and the
                    // transcript's reserved pad then read as a blank bottom.
                    .when(show_composer, |column| {
                        column.child(
                            div()
                                .flex_none()
                                .w_full()
                                .pt(px(4.))
                                .pb(px(root::COMPOSER_BOTTOM))
                                .flex()
                                .justify_center()
                                .child(
                                    div()
                                        .w_full()
                                        .max_w(px(root::CHAT_MAX_WIDTH))
                                        .px(px(root::CHAT_GUTTER))
                                        .flex()
                                        .flex_none()
                                        .flex_col()
                                        .min_h(px(root::composer_height()))
                                        .gap(px(8.))
                                        .children(self.pills(cx))
                                        .children(self.plan(cx).map(bleed))
                                        .children(self.permission(cx).map(bleed))
                                        .children(self.questions(cx).map(bleed))
                                        .children(self.queue(cx).map(bleed))
                                        .child(self.composer.clone())
                                        .children(context_row),
                                ),
                        )
                    })
                    .when(empty_chat, |column| {
                        column.child(div().flex_grow(1.).flex_basis(px(0.)))
                    })
                    // Out of flow, so folding the sidebar away costs the pane nothing:
                    // the controls float on the column rather than taking a row off it.
                    .when(!self.sidebar_open, |column| {
                        column.child(self.fold_cluster(window, cx))
                    }),
            )
            .children(context_panel)
    }
}

/// Width of the context rail on the chat's right.
/// Frame rate of the voice status bars while listening or speaking.
const VOICE_FPS: f32 = 20.0;

/// A clock for the speaking wave, so every repaint advances it.
fn voice_phase() -> Duration {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed()
}

const CONTEXT_PANEL_WIDTH: f32 = 200.;
/// Below this window width the rail is left out; the transcript comes first.
const CONTEXT_PANEL_MIN_WINDOW: f32 = 1000.;

impl Arbos {
    /// Cursor's context rail: "On ‹branch›" at the top, then one row per
    /// thing the chat has open — a browser page, a terminal, a job — each a
    /// click from view. Left out on a narrow window.
    fn context_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let workspace = self.workspace.read(cx);
        let project = workspace.active_project()?;
        let chat = workspace.active_session()?;
        let mut surfaces: Vec<_> = project
            .surfaces
            .iter()
            .filter(|surface| surface.owner == Some(chat.id))
            .collect();
        surfaces.sort_by_key(|surface| std::cmp::Reverse(surface.touched));
        let tasks = workspace.child_summaries(chat.id);
        // Cursor's rail carries standing Browser/Terminal/Files verbs; here a
        // surface exists only once the agent opens one, so an empty rail
        // would be dead space. It appears with the first surface or the
        // first sub-agent.
        if surfaces.is_empty() && tasks.is_empty() {
            return None;
        }
        let focused_child = project.focused_agent();
        let since = chat
            .live_since
            .and_then(|at| at.elapsed().ok())
            .unwrap_or_default();
        let focus = project.focus.and_then(|focus| focus.surface);
        let place_path = (project.place().host.is_none()).then(|| project.place().path.clone());
        let on = place_path
            .as_deref()
            .and_then(|path| self.branch_of(path))
            .unwrap_or_else(|| project.name());
        let rows: Vec<AnyElement> = surfaces
            .iter()
            .map(|surface| {
                let id = surface.id;
                let selected = focus == Some(id);
                div()
                    .id(("context-surface", id.0))
                    .h(px(26.))
                    .px(px(8.))
                    .rounded(px(5.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .text_style(TextStyle::Caption)
                    .text_color(if selected {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .when(selected, |el| el.bg(theme.element_active))
                    .when(!selected, |el| el.hover(|el| el.bg(theme.element_hover)))
                    .child(
                        icons::icon(board::glyph(&surface.board_kind))
                            .size(px(12.))
                            .flex_none()
                            .text_color(theme.text_muted),
                    )
                    .child(div().min_w_0().truncate().child(surface.title.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_surface(id, cx)))
                    .into_any_element()
            })
            .collect();
        let working = tasks
            .iter()
            .filter(|t| t.state == session::ChildState::Working)
            .count();
        let task_rows: Vec<AnyElement> = tasks
            .iter()
            .map(|task| {
                let id = task.id;
                let selected = focused_child == Some(id);
                let glyph: AnyElement = match task.state {
                    session::ChildState::Working => {
                        transcript::spinner(since, theme.text_muted, cx)
                    }
                    session::ChildState::Asking => icons::icon(icons::system::CHAT_ROUND_LINE)
                        .size(px(12.))
                        .text_color(theme.accent)
                        .into_any_element(),
                    session::ChildState::Waiting => div()
                        .size(px(9.))
                        .rounded_full()
                        .border_1()
                        .border_color(theme.text_faint)
                        .into_any_element(),
                    session::ChildState::Done => icons::icon(icons::status::CHECK)
                        .size(px(12.))
                        .text_color(theme.success)
                        .into_any_element(),
                };
                div()
                    .id(("context-task", id))
                    .h(px(26.))
                    .px(px(8.))
                    .rounded(px(5.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .text_style(TextStyle::Caption)
                    .text_color(match task.state {
                        session::ChildState::Done if !selected => theme.text_faint,
                        _ if selected => theme.text,
                        _ => theme.text_muted,
                    })
                    .when(selected, |el| el.bg(theme.element_active))
                    .when(!selected, |el| el.hover(|el| el.bg(theme.element_hover)))
                    .child(
                        div()
                            .flex_none()
                            .w(px(12.))
                            .flex()
                            .justify_center()
                            .child(glyph),
                    )
                    .child(div().min_w_0().truncate().child(task.title.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_session(id, cx)))
                    .into_any_element()
            })
            .collect();
        let tasks_head = (!tasks.is_empty()).then(|| {
            div()
                .h(px(22.))
                .px(px(8.))
                .mt(px(6.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_style(TextStyle::Caption)
                .text_color(theme.text_faint)
                .child("Tasks")
                .child(div().flex_1())
                .child(SharedString::from(if working > 0 {
                    format!("{working} working")
                } else {
                    format!("{}", tasks.len())
                }))
        });
        Some(
            div()
                .id("context-panel")
                .flex_none()
                .w(px(CONTEXT_PANEL_WIDTH))
                .h_full()
                .pt(px(root::HEADER_HEIGHT + 8.))
                .px(px(10.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .h(px(22.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .text_style(TextStyle::Caption)
                        .text_color(theme.text_faint)
                        .child(SharedString::from(format!("On {on}"))),
                )
                .children(rows)
                .children(tasks_head)
                .children(task_rows)
                .into_any_element(),
        )
    }

    /// Cursor's chat header: the title and the place on the left, the chat's
    /// menu on the right, on one slim line the transcript scrolls under.
    fn chat_header(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let workspace = self.workspace.read(cx);
        let Some(chat) = workspace.active_session() else {
            return div().into_any_element();
        };
        let id = chat.id;
        let closed = chat.closed;
        let title = workspace.display_label(id);
        let naming = self.renaming == Some(Renaming::Session(id)) && self.rename_in_header;
        let place = workspace.active_project().map(|project| project.name());
        let name_field = naming.then(|| self.header_name_field(window, cx));
        // Folded, the sidebar's traffic lights and fold button sit on this
        // column's left edge; the title starts after them. The cluster
        // begins at TOOLBAR_INSET and is CLUSTER_WIDTH wide, so a fixed
        // 92pt put the button over the title's first letters.
        let lead = if self.sidebar_open {
            14.
        } else {
            root::TOOLBAR_INSET + sidebar::CLUSTER_WIDTH + 10.
        };
        div()
            .id("chat-header")
            .flex_none()
            .h(px(root::HEADER_HEIGHT))
            .w_full()
            .pl(px(lead))
            .pr(px(10.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(if naming {
                div()
                    .id(("chat-header-title-name", id))
                    .min_w_0()
                    .max_w_full()
                    .text_style(TextStyle::Body)
                    .text_color(theme.text)
                    .children(name_field)
            } else {
                div()
                    .id(("chat-header-title", id))
                    .min_w_0()
                    .truncate()
                    .text_style(TextStyle::Body)
                    .text_color(theme.text)
                    .cursor_text()
                    .tooltip(|window, cx| Tooltip::text("Double-click to rename", window, cx))
                    .child(SharedString::from(title))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            cx.stop_propagation();
                            this.rename_header_title(id, window, cx);
                        }
                    }))
            })
            .children(place.map(|name| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child(
                        icons::icon(icons::files::FOLDER)
                            .size(px(11.))
                            .text_color(theme.text_faint)
                            .into_any_element(),
                    )
                    .child(div().max_w(px(200.)).truncate().child(name))
            }))
            .child(div().flex_1())
            .child(
                div()
                    .id(("chat-header-menu", id))
                    .relative()
                    .flex_none()
                    .size(px(24.))
                    .rounded(px(5.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|el| el.bg(theme.element_hover))
                    .tooltip(|window, cx| Tooltip::text("Chat actions", window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_menu_at_header(Menu::Session(id), cx)
                    }))
                    .child(
                        icons::icon(icons::system::MENU_DOTS)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .children(if self.menu_at_header {
                        self.session_menu_element(id, closed, cx)
                    } else {
                        None
                    }),
            )
            .into_any_element()
    }

    /// The line under the pill: where the agent runs on the left (Cursor's
    /// "Cloud" label; here `Local` or the remote place's ssh alias), a
    /// spinner on the right while a turn runs.
    fn context_row(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let workspace = self.workspace.read(cx);
        let since = workspace
            .active_session()
            .filter(|chat| chat.busy())
            .and_then(|chat| chat.live_since)
            .and_then(|at| at.elapsed().ok());
        let host = workspace
            .active_project()
            .and_then(|project| project.host.clone());
        let (glyph, machine) = match host {
            Some(alias) => (icons::devices::CLOUD, alias),
            None => (icons::devices::LAPTOP, "Local".to_owned()),
        };
        div()
            .id("composer-context")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .ml(px(-root::COMPOSER_PAD_X + 2.))
            .mr(px(-root::COMPOSER_PAD_X))
            .h(px(24.))
            .child(
                div()
                    .id("composer-machine")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.))
                    .pl(px(8.))
                    .max_w(px(200.))
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .tooltip(|window, cx| {
                        Tooltip::text("Where this agent runs", window, cx)
                    })
                    .child(
                        icons::icon(glyph)
                            .size(px(11.))
                            .flex_none()
                            .text_color(theme.text_faint),
                    )
                    .child(div().truncate().child(SharedString::from(machine))),
            )
            .children(self.voice_status(theme, cx))
            .child(div().flex_1())
            .children(since.map(|since| {
                div()
                    .pr(px(8.))
                    .child(transcript::spinner(since, theme.text_faint, cx))
            }))
            .into_any_element()
    }

    /// "Listening" with a level meter while the mic is open, "Speaking"
    /// with a moving wave while a reply plays, in the row under the
    /// composer. Nothing when the speech server is idle or not set up.
    fn voice_status(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::voice_ws::Phase;
        let status = crate::voice_ws::status();
        let (label, level) = match status.phase? {
            Phase::Listening => ("Listening", Some(status.level)),
            Phase::Speaking => ("Speaking", None),
            Phase::Connecting => ("Connecting to voice…", None),
            Phase::Ready | Phase::Off => return None,
        };
        Painter::of(cx).lease(VOICE_FPS, Duration::from_millis(300), cx);
        let t = voice_phase().as_secs_f32();
        let reduce = cx.reduce_motion();
        // Three bars: mic loudness when listening; a slow wave when
        // speaking (the level of a reply is not ours to know).
        let heights: [f32; 3] = match level {
            Some(l) => {
                let l = l.clamp(0.05, 1.0);
                [l * 0.7, l, l * 0.85]
            }
            None if reduce => [0.5, 0.8, 0.5],
            None => [
                0.35 + 0.35 * (t * 6.0).sin().abs(),
                0.35 + 0.55 * (t * 6.0 + 1.0).sin().abs(),
                0.35 + 0.35 * (t * 6.0 + 2.0).sin().abs(),
            ],
        };
        let reply = (!status.reply.is_empty()).then(|| {
            // One line: the reply is markdown with breaks; the row is a strip.
            let mut r = status.reply.split_whitespace().collect::<Vec<_>>().join(" ");
            if r.chars().count() > 60 {
                r = r.chars().take(60).collect::<String>() + "…";
            }
            r
        });
        Some(
            div()
                .id("composer-voice-status")
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .pl(px(8.))
                .text_style(TextStyle::Caption)
                .text_color(theme.accent)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_end()
                        .gap(px(2.))
                        .h(px(12.))
                        .children(heights.into_iter().map(|h| {
                            div()
                                .w(px(3.))
                                .h(px((12.0 * h).max(2.0)))
                                .rounded(px(1.5))
                                .bg(theme.accent)
                        })),
                )
                .child(SharedString::from(label))
                .when_some(reply, |row, reply| {
                    row.child(
                        div()
                            .max_w(px(360.))
                            .truncate()
                            .text_color(theme.text_faint)
                            .child(SharedString::from(reply)),
                    )
                })
                .into_any_element(),
        )
    }

    /// The checked-out branch of a local place, read once and kept; a
    /// checkout elsewhere shows up the next time the window opens the place.
    fn branch_of(&self, path: &std::path::Path) -> Option<String> {
        if let Some((known, branch)) = self.branch_cache.borrow().as_ref()
            && known == path
        {
            return branch.clone();
        }
        let head = std::fs::read_to_string(path.join(".git").join("HEAD")).ok();
        let branch = head.and_then(|head| {
            let head = head.trim();
            head.strip_prefix("ref: refs/heads/")
                .map(str::to_string)
                .or_else(|| (head.len() >= 7).then(|| head[..7].to_string()))
        });
        *self.branch_cache.borrow_mut() = Some((path.to_path_buf(), branch.clone()));
        branch
    }
}

/// A strip above the composer takes the composer's own plate edges: out past
/// the column gutter by the pad the composer bleeds, so the two line up.
/// GitHub pull-request URLs in a tool's output, in order of appearance.
/// `gh pr create` prints one; so does `gh pr view`. Trailing punctuation
/// from prose around the link is dropped.
/// What the two pills count for `chat`: the ids of its children with a
/// turn running, and the pull-request URLs it and its subtree opened.
///
/// PRs come from the kernel's `.arbos/prs.jsonl` when it has any (exact:
/// recorded from `gh pr create` results, whole subtree, survives restarts);
/// otherwise from scanning this chat's and its children's tool output, for
/// kernels that predate the file. The driver reports the same numbers.
pub fn pill_counts(project: &Project, chat: &ChatSession) -> (Vec<u64>, Vec<String>) {
    let mut working: Vec<u64> = project
        .sessions
        .iter()
        .filter(|c| c.parent == Some(chat.id) && c.busy())
        .map(|c| c.id)
        .collect();
    working.sort_unstable();
    let mut prs: Vec<String> = Vec::new();
    if chat.host.is_none() {
        if let Some(agent) = chat.agent_session.as_deref() {
            let place = arbos_core::Place::new(&chat.cwd);
            let all = arbos_core::load_prs(&place);
            if !all.is_empty() {
                let agents = arbos_core::list_agents(&place).unwrap_or_default();
                prs = arbos_core::prs::prs_of_tree(&all, agent, &agents)
                    .into_iter()
                    .map(|p| p.url)
                    .collect();
                return (working, prs);
            }
        }
    }
    for c in std::iter::once(chat).chain(
        project
            .sessions
            .iter()
            .filter(|c| c.parent == Some(chat.id)),
    ) {
        for item in &c.items {
            if let ChatItem::Tool { output, .. } = item {
                for url in pr_urls(output) {
                    if !prs.contains(&url) {
                        prs.push(url);
                    }
                }
            }
        }
    }
    (working, prs)
}

fn pr_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text.split(|c: char| {
        c.is_whitespace() || c == '"' || c == '\'' || c == '<' || c == '>' || c == '(' || c == ')'
    }) {
        let word = word.trim_end_matches(['.', ',', ';', ':']);
        let Some(rest) = word.strip_prefix("https://github.com/") else {
            continue;
        };
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() >= 4
            && parts[2] == "pull"
            && !parts[3].is_empty()
            && parts[3].chars().all(|c| c.is_ascii_digit())
        {
            out.push(format!(
                "https://github.com/{}/{}/pull/{}",
                parts[0], parts[1], parts[3]
            ));
        }
    }
    out
}

fn bleed(el: impl IntoElement) -> AnyElement {
    div()
        .ml(px(-root::COMPOSER_PAD_X))
        .mr(px(-root::COMPOSER_PAD_X))
        .child(el)
        .into_any_element()
}

/// The invitation's rows as one block: left-aligned so every glyph lands on the
/// same edge, and held off the line above it — `empty_state` centres its
/// children, which would otherwise centre each row on its own width.
fn letter_of(ix: usize) -> String {
    if ix < 26 {
        ((b'A' + ix as u8) as char).to_string()
    } else {
        (ix + 1).to_string()
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn make_list(rows: impl IntoIterator<Item = AnyElement>) -> impl IntoElement {
    div()
        .mt(px(14.))
        .flex()
        .flex_col()
        .items_start()
        .gap(px(8.))
        .children(rows)
}

impl Arbos {
    /// The front door, and what stands where a pane would be if one were
    /// showing: a project to open, or the first entry to make in the one that
    /// already is.
    fn launch(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.workspace.read(cx).active_project().is_some() {
            true => self.nothing_open(cx),
            false => self.no_project(cx),
        }
    }

    /// What to do when there is nothing to show: start the first chat.
    fn nothing_open(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let workspace = self.workspace.read(cx);
        let Some(ix) = workspace.active else {
            return self.no_project(cx);
        };
        let name = workspace
            .projects
            .get(ix)
            .map(|project| project.name())
            .unwrap_or_default();
        theme
            .empty_state(icons::files::FOLDER, "Nothing open", format!("in {name}"))
            .flex_1()
            .child(make_list([self.make_row(
                "session",
                "New session",
                icons::system::CHAT_ROUND_LINE,
                cx,
                move |this, window, cx| this.new_session_action(&NewSession, window, cx),
            )]))
            .into_any_element()
    }

    /// One line of the invitation: a glyph, a label, and what it makes.
    fn make_row(
        &self,
        id: &'static str,
        label: &'static str,
        glyph: &'static str,
        cx: &mut Context<Self>,
        make: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let theme = Theme::of(cx).clone();
        // An svg paints in its own `text_color` and inherits none, so the glyph
        // cannot ride the row's hover. Both halves take the row's group instead,
        // which lights them together — the group is named per row so hovering
        // one does not light the rest.
        div()
            .id(id)
            .group(id)
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .text_style(TextStyle::Callout)
            .child(
                icons::icon(glyph)
                    .size(px(14.))
                    .flex_none()
                    .text_color(theme.text_muted)
                    .group_hover(id, |el| el.text_color(theme.text)),
            )
            .child(
                div()
                    .text_color(theme.text_muted)
                    .group_hover(id, |el| el.text_color(theme.text))
                    .child(label),
            )
            .on_click(cx.listener(move |this, _, window, cx| make(this, window, cx)))
            .into_any_element()
    }

    fn surface_pane(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let workspace = self.workspace.read(cx);
        let Some(shown) = workspace.active_surface().cloned() else {
            return div().flex_1().into_any_element();
        };
        let place = workspace.active_project().map(|project| project.place());
        let glyph = board::glyph(&shown.board_kind);
        let heading = board::title(&shown);
        let id = shown.id;
        // A process journal grows on its own. Come back and read it again
        // while it is the one in front — one timer at a time, so a busy
        // window does not stack them.
        if board::live(&shown) && !TAIL_PENDING.swap(true, Ordering::SeqCst) {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(board::TAIL_EVERY).await;
                TAIL_PENDING.store(false, Ordering::SeqCst);
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        let content =
            if let Some(terminal) = shown.terminal_id().and_then(|id| self.terminals.get(id)) {
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(terminal.clone())
                    .into_any_element()
            } else {
                board::render(&shown, place.as_ref(), window, cx)
            };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .h(px(root::HEADER_HEIGHT))
                    .px(px(16.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        icons::icon(glyph)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_style(TextStyle::Body)
                            .text_color(theme.text)
                            .child(heading),
                    )
                    .child(
                        theme
                            .ghost(("surface-pane-close", id.0))
                            .p(px(4.))
                            .child(
                                icons::icon(icons::system::CLOSE)
                                    .size(px(12.))
                                    .text_color(theme.text_faint),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.show_pane(Pane::Chat, cx);
                                this.workspace.update(cx, |workspace, cx| {
                                    workspace.close_surface(id, cx);
                                });
                            })),
                    ),
            )
            .child(content)
            .into_any_element()
    }

    /// The session in front. It has one: the chat pane is named by `showing`
    /// only where a session is open in it, so the empty case is unreachable
    /// rather than a state this has to draw.
    fn conversation(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let workspace = self.workspace.read(cx);
        let Some(chat) = workspace.active_session() else {
            return div().flex_1().into_any_element();
        };
        // Nothing has been said yet, so what the session has to show for
        // itself is the directory the agent was started in.
        let inner = if chat.items.is_empty() {
            let id = chat.id;
            let title = workspace.display_label(id);
            let naming = self.renaming == Some(Renaming::Session(id)) && self.rename_heading;
            let heading = if naming {
                div()
                    .id(("chat-title-name", id))
                    .w_full()
                    .flex()
                    .justify_center()
                    .text_style(TextStyle::Title3)
                    .text_color(theme.text)
                    .child(self.heading_name_field(window, cx))
                    .into_any_element()
            } else {
                div()
                    .id(("chat-title", id))
                    .w_full()
                    .text_center()
                    .text_style(TextStyle::Title3)
                    .text_color(theme.text)
                    .child(title)
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            cx.stop_propagation();
                            this.rename_empty_title(id, window, cx);
                        }
                    }))
                    .into_any_element()
            };
            // Same column as the composer: max width + gutter, centred in
            // the pane. Title sits in the leftover height above the bar —
            // not the full window, and not a different inset.
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .relative()
                .child(
                    // Cursor's new-chat page: the title alone, just above
                    // the composer, both in the middle of the column.
                    div().absolute().inset_0().flex().justify_center().child(
                        div()
                            .w_full()
                            .max_w(px(root::CHAT_MAX_WIDTH))
                            .px(px(root::CHAT_GUTTER))
                            .h_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_end()
                            .pb(px(18.))
                            .child(heading),
                    ),
                )
                .into_any_element()
        } else {
            let id = chat.id;
            // A tool-list panic must not skip the composer sibling. The
            // transcript is inline in this render; catch it here.
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.workspace.update(cx, |workspace, cx| {
                    workspace.refresh_children(id);
                    match workspace.session(id) {
                        Some(chat) => transcript::render(chat, window, cx),
                        None => div().flex_1().into_any_element(),
                    }
                })
            })) {
                Ok(el) => el,
                Err(_) => div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_style(TextStyle::Callout)
                    .text_color(theme.text_muted)
                    .child("Transcript failed to draw.")
                    .into_any_element(),
            }
        };
        div()
            .id("conversation-drop")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .drag_over::<ExternalPaths>(|style, _, _, cx| style.bg(Theme::of(cx).element_hover))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.composer.update(cx, |composer, cx| {
                    composer.accept_paths(paths.paths().to_vec(), window, cx);
                });
                cx.stop_propagation();
            }))
            .on_drop(cx.listener(|this, drag: &SessionDrag, window, cx| {
                this.composer.update(cx, |composer, cx| {
                    composer.accept_chat_link(&drag.markdown, window, cx);
                });
            }))
            .child(inner)
            .into_any_element()
    }

    /// Cursor's chips above the composer: "Working N" for the sub-agents
    /// with a turn running, "PRs N" for the pull requests this chat and its
    /// children have opened. Both derived from the sessions on hand;
    /// neither shows at zero. Working opens the first live child; PRs opens
    /// the newest one.
    fn pills(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = Theme::of(cx).clone();
        let workspace = self.workspace.read(cx);
        let chat = workspace.active_session()?;
        let project = workspace.active_project()?;
        let (working, prs) = pill_counts(project, chat);
        if working.is_empty() && prs.is_empty() {
            return None;
        }
        let first_working = working.first().copied();
        let newest_pr = prs.last().cloned();
        let pr_list = prs.join("\n");
        let pill = |id: &'static str, glyph: AnyElement, label: String| {
            div()
                .id(id)
                .h(px(24.))
                .px(px(9.))
                .rounded_full()
                .border_1()
                .border_color(theme.border)
                .bg(theme.surface_raised.opacity(0.6))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .cursor_pointer()
                .hover(|el| el.bg(theme.element_hover))
                .text_style(TextStyle::Caption)
                .text_color(theme.text_muted)
                .child(glyph)
                .child(SharedString::from(label))
        };
        Some(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .when(!working.is_empty(), |row| {
                    row.child(
                        pill(
                            "pill-working",
                            svg()
                                .path(crate::assets::DELEGATE_ICON)
                                .size(px(12.))
                                .flex_none()
                                .text_color(theme.text_muted)
                                .into_any_element(),
                            format!("Working {}", working.len()),
                        )
                        .tooltip(|window, cx| {
                            Tooltip::text("Sub-agents with a turn running", window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(id) = first_working {
                                this.select_session(id, cx);
                            }
                        })),
                    )
                })
                .when(!prs.is_empty(), |row| {
                    let list = pr_list.clone();
                    row.child(
                        pill(
                            "pill-prs",
                            icons::icon(icons::editing::GIT_BRANCH)
                                .size(px(12.))
                                .flex_none()
                                .text_color(theme.text_muted)
                                .into_any_element(),
                            format!("PRs {}", prs.len()),
                        )
                        .tooltip(move |window, cx| Tooltip::text(list.clone(), window, cx))
                        .on_click(move |_, _, cx| {
                            if let Some(url) = &newest_pr {
                                cx.open_url(url);
                            }
                        }),
                    )
                })
                .into_any_element(),
        )
    }

    /// The agent's plan: what it holds that is not yet done. One header
    /// line — steps, standing obligations, questions, queued prompts — and,
    /// unfolded, a row per open node with its trigger and last outcome.
    /// Each row can be run now, cancelled, or (a question) answered.
    fn plan(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = Theme::of(cx).clone();
        let chat = self.workspace.read(cx).active_session()?;
        let id = chat.id;
        let nodes: Vec<PlanNode> = chat.plan_open().cloned().collect();
        let queued = chat.plan_queued();
        if nodes.is_empty() && queued == 0 {
            return None;
        }
        let answering = chat.answering;
        let steps = nodes
            .iter()
            .filter(|n| !n.standing && n.do_kind != "ask")
            .count();
        let standing = nodes.iter().filter(|n| n.standing).count();
        let asks = nodes.iter().filter(|n| n.do_kind == "ask").count();
        let failed = nodes.iter().filter(|n| n.status == "failed").count();
        let running = nodes.iter().any(|n| n.status == "active");
        let since = chat
            .live_since
            .and_then(|at| at.elapsed().ok())
            .unwrap_or_default();
        let mut parts: Vec<String> = Vec::new();
        if steps > 0 {
            parts.push(plural(steps, "step", "steps"));
        }
        if standing > 0 {
            parts.push(plural(standing, "standing", "standing"));
        }
        if asks > 0 {
            parts.push(plural(asks, "question for you", "questions for you"));
        }
        if failed > 0 {
            parts.push(plural(failed, "failed", "failed"));
        }
        if queued > 0 {
            parts.push(plural(queued, "message queued", "messages queued"));
        }
        let folded = self.plan_folded;

        let header = div()
            .id("plan-head")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.plan_folded = !this.plan_folded;
                cx.notify();
            }))
            .child(
                icons::icon(if folded {
                    icons::arrows::ALT_ARROW_RIGHT
                } else {
                    icons::arrows::ALT_ARROW_DOWN
                })
                .size(px(12.))
                .text_color(theme.text_faint),
            )
            .child(
                div()
                    .text_style(TextStyle::Callout)
                    .text_color(theme.text_muted)
                    .child("Plan"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_style(TextStyle::Caption)
                    .text_color(if failed > 0 && folded {
                        theme.danger
                    } else {
                        theme.text_faint
                    })
                    .child(SharedString::from(parts.join(" · "))),
            )
            .when(running, |el| {
                el.child(transcript::spinner(since, theme.text_faint, cx))
            })
            // Stop everything this agent has going: the turn, its standing
            // work, its children. Scheduled nodes block until run again.
            .when(standing > 0 || running, |el| {
                el.child(
                    theme
                        .ghost(SharedString::from(format!("plan-stop-{id}")))
                        .flex_none()
                        .px(px(6.))
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .rounded(px(Theme::control_radius()))
                        .tooltip(move |window, cx| {
                            bezel::ui::tooltip::Tooltip::text(
                                "Stop this agent's standing work and its children",
                                window,
                                cx,
                            )
                        })
                        .child(
                            icons::icon(icons::media::STOP)
                                .size(px(10.))
                                .text_color(theme.text_muted),
                        )
                        .child(
                            div()
                                .text_style(TextStyle::Caption)
                                .text_color(theme.text_muted)
                                .child("Stop"),
                        )
                        .on_mouse_down(bezel::gpui::MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation()
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.workspace
                                .update(cx, |workspace, cx| workspace.cancel(id, cx));
                        })),
                )
            });

        // Depth from the parent chain, so children indent under their goal.
        let mut depth: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        for n in &nodes {
            let d = if n.parent == 0 {
                0
            } else {
                depth.get(&n.parent).map(|d| d + 1).unwrap_or(0)
            };
            depth.insert(n.id, d);
        }

        let rows = (!folded).then(|| {
            div().flex().flex_col().gap(px(2.)).children(
                nodes.iter().map(|n| {
                    self.plan_row(id, n, depth[&n.id], answering == Some(n.id), &theme, cx)
                }),
            )
        });

        Some(
            div()
                .rounded(px(Theme::surface_radius()))
                .px(px(root::COMPOSER_PAD_X))
                .py(px(8.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(header)
                .children(rows)
                .surface(&theme, composer::SURFACE)
                .into_any_element(),
        )
    }

    fn plan_row(
        &self,
        chat: u64,
        n: &PlanNode,
        depth: usize,
        answering: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let node = n.id;
        let active = n.status == "active";
        let ask = n.do_kind == "ask";
        let (icon, tone) = match (n.status.as_str(), n.do_kind.as_str(), n.standing) {
            ("failed", _, _) => (icons::status::DANGER_TRIANGLE, theme.danger),
            ("blocked", _, _) => (icons::media::PAUSE, theme.warning),
            (_, "ask", _) => (icons::system::CHAT_ROUND_LINE, theme.accent),
            (_, _, true) => (icons::media::REPEAT, theme.text_faint),
            (_, "shell", _) => (icons::devices::TERMINAL, theme.text_faint),
            (_, "notify", _) => (icons::status::BELL, theme.text_faint),
            _ => (icons::editing::CHECKLIST, theme.text_faint),
        };
        let group = SharedString::from(format!("plan-row-{chat}-{node}"));
        let since = self
            .workspace
            .read(cx)
            .active_session()
            .and_then(|c| c.live_since)
            .and_then(|at| at.elapsed().ok())
            .unwrap_or_default();
        let mark = if active {
            transcript::spinner(since, theme.accent, cx).into_any_element()
        } else {
            icons::icon(icon)
                .size(px(12.))
                .text_color(tone)
                .into_any_element()
        };
        // The controls sit faint until the row is hovered, like the queue's ✕.
        let action = |key: &str, glyph: &'static str, tip: &'static str, op: &'static str| {
            let group = group.clone();
            theme
                .ghost(SharedString::from(format!("plan-{chat}-{node}-{key}")))
                .flex_none()
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .tooltip(move |window, cx| bezel::ui::tooltip::Tooltip::text(tip, window, cx))
                .child(
                    icons::icon(glyph)
                        .size(px(11.))
                        .text_color(theme.text_faint.opacity(0.35))
                        .group_hover(group, |el| el.text_color(theme.text)),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.workspace.update(cx, |workspace, cx| {
                        workspace.with_session(chat, cx, |c| c.plan_op(node, op, ""));
                    });
                    cx.notify();
                }))
        };
        let can_run = !active && !ask;
        let controls = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .when(can_run, |el| {
                el.child(action("run", icons::media::PLAY, "Run now", "run"))
            })
            .when(!active, |el| {
                el.child(action("cancel", icons::system::CLOSE, "Cancel", "cancel"))
            });
        let answer = ask.then(|| {
            let label = if answering {
                "Answering below…"
            } else {
                "Answer"
            };
            theme
                .ghost(SharedString::from(format!("plan-{chat}-{node}-answer")))
                .px(px(8.))
                .h(px(20.))
                .flex()
                .items_center()
                .rounded(px(Theme::control_radius()))
                .border_1()
                .border_color(theme.border)
                .text_style(TextStyle::Caption)
                .text_color(if answering { theme.accent } else { theme.text })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.workspace.update(cx, |workspace, cx| {
                        workspace.with_session(chat, cx, |c| {
                            c.answering = if c.answering == Some(node) {
                                None
                            } else {
                                Some(node)
                            };
                        });
                    });
                    cx.notify();
                }))
        });
        let last = (!n.last.is_empty()).then(|| {
            div()
                .min_w_0()
                .truncate()
                .text_style(TextStyle::Caption)
                .text_color(if n.status == "failed" {
                    theme.danger
                } else {
                    theme.text_faint
                })
                .child(SharedString::from(format!("last: {}", n.last)))
        });
        div()
            .group(group)
            .flex()
            .flex_col()
            .pl(px(depth as f32 * 14.))
            .py(px(1.))
            .rounded(px(4.))
            .hover(|el| el.bg(theme.element_hover))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_none()
                            .w(px(12.))
                            .flex()
                            .justify_center()
                            .child(mark),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_style(TextStyle::Callout)
                            .text_color(if active { theme.text } else { theme.text_muted })
                            .child(SharedString::from(n.goal.clone())),
                    )
                    .when(!n.when.is_empty(), |el| {
                        el.child(
                            div()
                                .flex_none()
                                .text_style(TextStyle::Caption)
                                .text_color(theme.text_faint)
                                .child(SharedString::from(n.when.clone())),
                        )
                    })
                    .children(answer)
                    .child(controls),
            )
            .children(last.map(|l| div().pl(px(20.)).child(l)))
            .into_any_element()
    }

    /// The agent's tool-authorization request.
    ///
    /// A macOS permission alert: what is being asked for, the two answers, and
    /// a checkbox saying how long the answer holds. See [`alert`] for why two
    /// buttons carry four options, and for what an agent has to ask to get the
    /// stack of rows instead.
    fn permission(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let theme = Theme::of(cx).clone();
        let chat = self.workspace.read(cx).active_session()?;
        let prompt = chat.permission.as_ref()?;
        let id = chat.id;
        let painter = Painter::of(cx);
        // One button, whichever layout it lands in. `key` is the element's and
        // the hover wash's both — the wash store is one map for the whole app,
        // so the session is in it too.
        let answer = |key: &str, option_id: String, label: &str, style| {
            let fade = Fade::new(painter, format!("permission-{id}-{key}"));
            theme
                .button(label.to_owned(), style, Some(fade))
                .id(SharedString::from(key.to_owned()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let option_id = option_id.clone();
                    this.workspace.update(cx, |workspace, cx| {
                        workspace.with_session(id, cx, |chat| chat.respond_permission(option_id));
                    });
                }))
        };
        let body = match alert(&prompt.options) {
            // How long the answer holds, then the answers — the affirmative
            // last, where macOS puts the default.
            Some((deny, allow)) => div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                // An agent offering neither *always* form has no second
                // question to ask, and the row is just the two buttons.
                .children((deny.always.is_some() || allow.always.is_some()).then(|| {
                    div()
                        .id("permission-always")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .cursor_pointer()
                        .child(theme.checkbox(prompt.always))
                        .child(
                            div()
                                .text_style(TextStyle::Callout)
                                .text_color(theme.text_muted)
                                .child("Always allow"),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.workspace.update(cx, |workspace, cx| {
                                workspace
                                    .with_session(id, cx, |chat| chat.toggle_permission_always());
                            });
                        }))
                }))
                // Whatever is on the left is on the left: the answers hold the
                // trailing edge whether or not the checkbox is there.
                .child(div().flex_1())
                .child(answer(
                    "deny",
                    deny.id(prompt.always),
                    deny.label,
                    ButtonStyle::Ghost,
                ))
                .child(answer(
                    "allow",
                    allow.id(prompt.always),
                    allow.label,
                    ButtonStyle::Prominent,
                ))
                .into_any_element(),
            // Every option the agent sent, one full-width row each. A label of
            // any length reads here, which is the whole point of stacking them.
            None => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .children(prompt.options.iter().enumerate().map(|(ix, option)| {
                    let style = match option.kind {
                        PermissionOptionKind::AllowOnce => ButtonStyle::Prominent,
                        _ => ButtonStyle::Ghost,
                    };
                    answer(
                        &format!("option-{ix}"),
                        option.id.clone(),
                        &option.name,
                        style,
                    )
                    .w_full()
                    .justify_center()
                }))
                .into_any_element(),
        };
        Some(
            div()
                .rounded(px(Theme::surface_radius()))
                .px(px(root::COMPOSER_PAD_X))
                .py(px(12.))
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_start()
                        .gap(px(8.))
                        .child(
                            icons::icon(icons::system::KEY_MINIMALISTIC)
                                .size(px(14.))
                                .flex_none()
                                // A glyph's box is its size and the line beside
                                // it is taller, so it drops to meet the text.
                                .mt(px(3.))
                                .text_color(theme.text_muted),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_style(TextStyle::Body)
                                .text_color(theme.text)
                                .child(prompt.title.clone()),
                        ),
                )
                .child(body)
                .surface(&theme, composer::SURFACE),
        )
    }

    /// The ask tool's form: one question at a time, Skip or Continue.
    fn questions(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let theme = Theme::of(cx).clone();
        let chat = self.workspace.read(cx).active_session()?;
        let prompt = chat.questions.as_ref()?;
        let id = chat.id;
        let question = prompt.current()?;
        let draft = prompt.draft(&question.id);
        let page = prompt.page;
        let count = prompt.questions.len();
        let heading = if prompt.title.is_empty() {
            "Questions".to_owned()
        } else {
            format!("Questions · {}", prompt.title)
        };
        let option_row = |key: String,
                          letter: String,
                          label: String,
                          selected: bool,
                          question_id: String,
                          option_id: Option<String>| {
            div()
                .id(SharedString::from(key))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .px(px(6.))
                .py(px(4.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(selected, |row| row.bg(theme.element_active))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.workspace.update(cx, |workspace, cx| {
                        workspace.with_session(id, cx, |chat| match &option_id {
                            Some(option_id) => chat.toggle_ask_option(&question_id, option_id),
                            None => chat.toggle_ask_other(&question_id),
                        });
                    });
                }))
                .child(
                    div()
                        .w(px(18.))
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(if selected { theme.text } else { theme.border })
                        .text_style(TextStyle::Caption)
                        .text_color(if selected {
                            theme.text
                        } else {
                            theme.text_muted
                        })
                        .child(letter),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_style(TextStyle::Callout)
                        .text_color(if selected {
                            theme.text
                        } else {
                            theme.text_muted
                        })
                        .child(label),
                )
        };
        let mut options: Vec<AnyElement> = question
            .options
            .iter()
            .enumerate()
            .map(|(ix, option)| {
                option_row(
                    format!("ask-{id}-{page}-{ix}"),
                    letter_of(ix),
                    option.label.clone(),
                    draft.selected.iter().any(|sid| sid == &option.id),
                    question.id.clone(),
                    Some(option.id.clone()),
                )
                .into_any_element()
            })
            .collect();
        options.push(
            option_row(
                format!("ask-{id}-{page}-other"),
                letter_of(question.options.len()),
                "Other…".into(),
                draft.other,
                question.id.clone(),
                None,
            )
            .into_any_element(),
        );
        let prompt_text = if question.prompt.is_empty() {
            heading.clone()
        } else {
            format!("{}. {}", page + 1, question.prompt)
        };
        Some(
            div()
                .rounded(px(Theme::surface_radius()))
                .px(px(14.))
                .py(px(12.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            icons::icon(icons::system::CHAT_ROUND_LINE)
                                .size(px(14.))
                                .flex_none()
                                .text_color(theme.text_muted),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_style(TextStyle::Callout)
                                .text_color(theme.text_muted)
                                .child(heading),
                        )
                        .when(count > 1, |row| {
                            row.child(
                                div()
                                    .id("ask-prev")
                                    .cursor_pointer()
                                    .text_style(TextStyle::Caption)
                                    .text_color(theme.text_muted)
                                    .child("↑")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let held = this.composer.read(cx).content(cx);
                                        this.workspace.update(cx, |workspace, cx| {
                                            workspace.with_session(id, cx, |chat| {
                                                chat.turn_ask_page(-1, &held);
                                            });
                                        });
                                    })),
                            )
                            .child(
                                div()
                                    .text_style(TextStyle::Caption)
                                    .text_color(theme.text_muted)
                                    .child(format!("{} of {count}", page + 1)),
                            )
                            .child(
                                div()
                                    .id("ask-next")
                                    .cursor_pointer()
                                    .text_style(TextStyle::Caption)
                                    .text_color(theme.text_muted)
                                    .child("↓")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let held = this.composer.read(cx).content(cx);
                                        this.workspace.update(cx, |workspace, cx| {
                                            workspace.with_session(id, cx, |chat| {
                                                chat.turn_ask_page(1, &held);
                                            });
                                        });
                                    })),
                            )
                        }),
                )
                .child(
                    div()
                        .text_style(TextStyle::Body)
                        .text_color(theme.text)
                        .child(prompt_text),
                )
                .child(div().flex().flex_col().gap(px(4.)).children(options))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_1())
                        .child(
                            theme
                                .button("Skip", ButtonStyle::Ghost, None)
                                .id("ask-skip")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.workspace.update(cx, |workspace, cx| {
                                        workspace.with_session(id, cx, |chat| chat.skip_ask());
                                    });
                                })),
                        )
                        .child(
                            theme
                                .button("Continue", ButtonStyle::Prominent, None)
                                .id("ask-continue")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let held = this.composer.read(cx).content(cx);
                                    this.composer.update(cx, |composer, cx| {
                                        composer
                                            .text_field()
                                            .update(cx, |field, cx| field.clear(cx));
                                    });
                                    this.workspace.update(cx, |workspace, cx| {
                                        workspace.with_session(id, cx, |chat| {
                                            chat.draft.clear();
                                            chat.answer_ask(&held, false);
                                        });
                                    });
                                })),
                        ),
                )
                .surface(&theme, composer::SURFACE),
        )
    }

    /// A pick from one of the composer's switches — the session's mode, or a
    /// config option like the model.
    pub(crate) fn switch(
        &mut self,
        id: &composer::SwitchId,
        value: &SharedString,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.workspace.read(cx).active_session().map(|chat| chat.id) else {
            return;
        };
        let value = value.to_string();
        self.workspace.update(cx, |workspace, cx| match id {
            composer::SwitchId::Mode => workspace.set_session_mode(session, value, cx),
            composer::SwitchId::Model => workspace.set_session_model(session, value, cx),
            composer::SwitchId::Config(config) => workspace.set_session_config(
                session,
                config.to_string(),
                SessionConfigOptionValue::ValueId {
                    value: value.into(),
                },
                cx,
            ),
        });
        cx.notify();
    }

    /// Prompts waiting for the in-flight turn — a steer, drawn as what it is:
    /// the message you have already written, not yet sent. The same bubble the
    /// transcript gives a sent one, held back to the muted tone, and an ✕ to
    /// take it back while it is still yours to take back.
    fn queue(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let theme = Theme::of(cx).clone();
        let chat = self.workspace.read(cx).active_session()?;
        if chat.queue.is_empty() {
            return None;
        }
        let id = chat.id;
        let busy = chat.busy();
        Some(div().flex().flex_col().w_full().gap(px(6.)).children(
            chat.queue.iter().enumerate().map(|(ix, text)| {
                // An svg paints in its own `text_color` and inherits none,
                // so the ✕ takes the bubble's group to light with it.
                let group = SharedString::from(format!("steer-{ix}"));
                div()
                    .group(group.clone())
                    .w_full()
                    .px(px(root::COMPOSER_PAD_X))
                    .py(px(9.))
                    .rounded(px(Theme::surface_radius()))
                    .bg(theme.surface_raised.opacity(0.6))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_style(TextStyle::Body)
                            .text_color(theme.text_muted)
                            .child(text.display()),
                    )
                    .when(busy && ix == 0, |row| {
                        row.child(
                            div()
                                .id("force-queue")
                                .flex_none()
                                .mt(px(2.))
                                .cursor_pointer()
                                .text_style(TextStyle::Callout)
                                .text_color(theme.text_faint)
                                .group_hover(group.clone(), |el| el.text_color(theme.text))
                                .child("Force")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.composer.update(cx, |composer, cx| {
                                        composer.force(cx);
                                    });
                                })),
                        )
                    })
                    .child(
                        div()
                            .id(("unqueue", ix))
                            .flex_none()
                            // Onto the first line's baseline, so a steer
                            // that wraps keeps its ✕ at the top.
                            .mt(px(4.))
                            .cursor_pointer()
                            .child(
                                icons::icon(icons::system::CLOSE)
                                    .size(px(12.))
                                    .text_color(theme.text_faint)
                                    .group_hover(group, |el| el.text_color(theme.text)),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.workspace.update(cx, |workspace, cx| {
                                    workspace.with_session(id, cx, |chat| chat.unqueue(ix));
                                });
                            })),
                    )
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{Choice, PermissionOptionKind, alert};

    fn choice(id: &str, kind: PermissionOptionKind) -> Choice {
        Choice {
            id: id.to_owned(),
            name: id.to_owned(),
            kind,
        }
    }

    /// The set every agent sends: two answers, one of them rememberable.
    #[test]
    fn a_yes_a_no_and_a_forever_is_an_alert() {
        let options = vec![
            choice("yes", PermissionOptionKind::AllowOnce),
            choice("yes-always", PermissionOptionKind::AllowAlways),
            choice("no", PermissionOptionKind::RejectOnce),
        ];
        let (deny, allow) = alert(&options).expect("two sides, all three covered");
        assert_eq!(allow.id(false), "yes");
        assert_eq!(allow.id(true), "yes-always");
        // No always-form to send: the refusal stands for this call either way.
        assert_eq!(deny.id(true), "no");
    }

    /// An option neither side accounts for is an option the alert would drop.
    #[test]
    fn a_kind_of_the_agents_own_falls_to_the_stack() {
        let options = vec![
            choice("yes", PermissionOptionKind::AllowOnce),
            choice("no", PermissionOptionKind::RejectOnce),
            choice("edit", PermissionOptionKind::Other("edit_first".into())),
        ];
        assert!(alert(&options).is_none());
    }

    /// So is a second option of a kind one side has already taken.
    #[test]
    fn a_repeated_kind_falls_to_the_stack() {
        let options = vec![
            choice("yes", PermissionOptionKind::AllowOnce),
            choice("yes-too", PermissionOptionKind::AllowOnce),
            choice("no", PermissionOptionKind::RejectOnce),
        ];
        assert!(alert(&options).is_none());
    }

    /// An alert needs both answers — a lone side has nothing to sit opposite.
    #[test]
    fn one_sided_falls_to_the_stack() {
        let options = vec![
            choice("yes", PermissionOptionKind::AllowOnce),
            choice("yes-always", PermissionOptionKind::AllowAlways),
        ];
        assert!(alert(&options).is_none());
    }
}
