//! The detail column: whichever pane is showing, and everything the turn in
//! flight stacks under it — plan, permission, queue, composer.

use crate::{
    kernel,
    model::{
        attachment::Prompt,
        project::Project,
        session::{ChatItem, ChatSession, Choice, Connection, PlanNode},
        settings,
    },
    view::{
        component::{composer, composer::SessionDrag, menu::Menu, surface as board, transcript},
        naming::Renaming,
        root::{self, Arbos, NewSession, Pane},
    },
};
use bezel::{
    gpui::{
        AnyElement, App, ClickEvent, Context, ExternalPaths, FocusHandle, Focusable as _,
        SharedString, Window, div, img, prelude::*, px, svg,
    },
    motion::{Fade, Painter},
    theme::{TextStyle, Theme, Typeset},
    ui::{
        icons, surface,
        tooltip::Tooltip,
        widgets::{ButtonStyle, Buttons, Content},
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
                    vision: None,
                    free: false,
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
                        vision: Some(model.sees_images()),
                        free: model.free,
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
            vision: None,
            free: false,
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
        // Cursor's approval card takes Enter as "Run ↵": an empty send while
        // one is parked allows the call.
        if text.text.trim().is_empty() && text.attachments.is_empty() {
            let allow = self
                .workspace
                .read(cx)
                .active_session()
                .and_then(|chat| chat.permission.as_ref())
                .and_then(|prompt| {
                    prompt
                        .options
                        .iter()
                        .find(|o| o.kind == PermissionOptionKind::AllowOnce)
                        .map(|o| o.id.clone())
                });
            if let Some(option_id) = allow {
                self.workspace.update(cx, |workspace, cx| {
                    workspace.with_session(id, cx, |chat| chat.respond_permission(option_id));
                });
                return;
            }
        }
        // "stop" while the chat works is the Stop button, not a follow-up.
        let busy = self
            .workspace
            .read(cx)
            .active_session()
            .is_some_and(|chat| chat.busy());
        if busy && text.attachments.is_empty() && arbos_core::is_stop_word(&text.text) {
            // The kernel's `interrupted` line becomes the "Stopped by you"
            // notice, so nothing is said twice.
            self.workspace
                .update(cx, |workspace, cx| workspace.cancel(id, cx));
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
    /// than as a prompt; `/voice <words>` goes to the speech server. True
    /// when the text was one of them.
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
            // `/voice <words>`: to the speech server's own model over its
            // text channel, not to this chat's agent. It answers aloud; the
            // answer and what its agent does come back as notices.
            "voice" if !arg.is_empty() => {
                let sent = crate::voice_ws::text_input(arg);
                let line = match &sent {
                    Ok(()) => format!("voice ← {arg}"),
                    Err(e) => format!("voice failed: {e:#}"),
                };
                let failed = sent.is_err();
                self.workspace.update(cx, |workspace, cx| {
                    workspace.with_session(id, cx, |chat| chat.notice(failed, &line));
                });
                if !failed {
                    self.start_voice_mirror(cx);
                }
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

    /// Hold the composer's text for the next turn on this chat; the kernel
    /// keeps it and runs it when the turn in flight ends.
    pub(crate) fn queue_turn(&mut self, text: Prompt, cx: &mut Context<Self>) {
        let Some(id) = self.workspace.read(cx).active_id() else {
            return;
        };
        self.workspace
            .update(cx, |workspace, cx| workspace.queue_next(id, text, cx));
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
            // Cursor's subagent chat takes no follow-ups; a worker the
            // kernel archived is that here.
            _ if chat.is_some_and(|chat| chat.agent_gone()) => "Follow-ups aren't available for this worker",
            // Cursor: a fresh chat invites; one with a turn asks for the next.
            _ if chat.is_some_and(|chat| chat.items.is_empty()) => "Plan, search, build anything",
            _ => "Send follow-up",
        }
        .to_owned();
        let commands = workspace.slash_commands.clone();
        let streaming = chat.is_some_and(|chat| chat.busy());
        let reconnect = matches!(chat.map(|chat| &chat.connection), Some(Connection::Lost));
        let current = chat.map(|_| 0);
        let live = chat.filter(|chat| chat.live());
        let switches = switches(chat, &workspace.models);
        let model_note = workspace.models.error.clone();
        // The pinned mode, from agent.md (local places), and the skills on
        // offer for the chip's list.
        let (mode_skill, skills) = match live.filter(|chat| chat.host.is_none()) {
            Some(chat) => {
                let place = arbos_core::Place::new(&chat.cwd);
                let pinned = chat
                    .agent_session
                    .as_deref()
                    .and_then(|sid| kernel::agent_skill(&place, sid));
                let skills = if pinned.is_some() {
                    kernel::skill_names(&place)
                } else {
                    Vec::new()
                };
                (pinned, skills)
            }
            None => (None, Vec::new()),
        };
        let usage = live.and_then(|chat| chat.usage);
        let next_id = chat.map(|chat| chat.id);
        let next_draft = chat.map(|chat| chat.draft.clone()).unwrap_or_default();
        let pushed = chat.is_some_and(|chat| chat.draft_pushed);
        let (old_id, held) = self
            .composer
            .update(cx, |composer, cx| (composer.bound(), composer.content(cx)));
        if pushed && old_id == next_id {
            // The model set the draft: the composer takes it over whatever
            // was typed.
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
            composer.set_reconnect(reconnect, cx);
            composer.set_agents(&agents, current, cx);
            composer.set_model_note(&model_note, cx);
            composer.set_switches(&switches, cx);
            composer.set_mode_skill(mode_skill.clone(), skills.clone(), cx);
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
            Some(Pane::Project) => self.project_view(window, cx),
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
                                        .children(self.provider_offer(cx).map(bleed))
                                        .children(self.held(cx).map(bleed))
                                        .children(self.queue(cx).map(bleed))
                                        .children(self.live_view(cx).map(bleed))
                                        .child(self.composer.clone())
                                        .children(context_row),
                                ),
                        )
                    })
                    .when(empty_chat, |column| {
                        column.child(div().flex_grow(1.).flex_basis(px(0.)))
                    }),
            )
    }
}

/// Frame rate of the voice status bars while listening or speaking.
const VOICE_FPS: f32 = 20.0;

/// A clock for the speaking wave, so every repaint advances it.
fn voice_phase() -> Duration {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed()
}

impl Arbos {
    /// Cursor's chat header: the chat's place in the tree on the left — its
    /// parents as crumbs, then its title — the chat's menu and the panel
    /// toggle on the right, on one slim line the transcript scrolls under.
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
        // A sub-agent in front is shown under its parents, Cursor's way:
        // `Main › Review edge cases`, each crumb a click back up the tree.
        let crumbs: Vec<(u64, String)> = workspace
            .active_project()
            .map(|project| {
                let mut path = project.path_to(id);
                path.pop();
                path.into_iter()
                    .map(|up| (up, workspace.display_label(up)))
                    .collect()
            })
            .unwrap_or_default();
        // The main chat is what the tab names; its title would say the same
        // thing twice, so only a sub-agent's title is drawn here, after
        // the crumbs that lead back to it.
        let titled = !crumbs.is_empty();
        let name_field = (naming && titled).then(|| self.header_name_field(window, cx));
        div()
            .id("chat-header")
            .flex_none()
            .h(px(root::HEADER_HEIGHT))
            .w_full()
            .pl(px(14.))
            .pr(px(10.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .children(crumbs.into_iter().flat_map(|(up, label)| {
                [
                    div()
                        .id(("chat-header-crumb", up))
                        .flex_none()
                        .max_w(px(180.))
                        .truncate()
                        .text_style(TextStyle::Body)
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|el| el.text_color(theme.text))
                        .child(SharedString::from(label))
                        .on_click(cx.listener(move |this, _, _, cx| this.select_session(up, cx)))
                        .into_any_element(),
                    div()
                        .flex_none()
                        .text_style(TextStyle::Body)
                        .text_color(theme.text_faint)
                        .child("›")
                        .into_any_element(),
                ]
            }))
            .when(titled && naming, |el| {
                el.child(
                    div()
                        .id(("chat-header-title-name", id))
                        .min_w_0()
                        .max_w_full()
                        .text_style(TextStyle::Body)
                        .text_color(theme.text)
                        .children(name_field),
                )
            })
            .when(titled && !naming, |el| {
                el.child(
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
                        })),
                )
            })
            .child(div().flex_1())
            .child(
                self.menu_press(
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
                        .hover(|el| el.bg(theme.element_hover)),
                    Menu::Session(id),
                    cx,
                )
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
            .child(self.panel_toggle(cx))
            .into_any_element()
    }

    /// The line under the pill: where the agent runs on the left (Cursor's
    /// "Cloud" label; here `Local` or the remote place's ssh alias), a
    /// spinner on the right while a turn runs.
    fn context_row(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let workspace = self.workspace.read(cx);
        // Cursor keeps a small ring at the row's end; ours turns while any
        // agent of the project works — the root, or only its workers.
        let since = workspace.active_project().and_then(|project| {
            project
                .sessions
                .iter()
                .filter(|chat| chat.busy())
                .map(|chat| chat.elapsed().unwrap_or_else(transcript::live_phase))
                .max()
        });
        let host = workspace
            .active_project()
            .and_then(|project| project.host.clone());
        // Cursor's two pills under the composer: the branch checked out in the
        // project, and where the agent runs — "This Mac" on a Mac, "This
        // Computer" elsewhere, the remote's alias when it is one.
        let branch = workspace
            .active_project()
            .filter(|project| !project.is_remote())
            .and_then(|project| self.branch_of(&project.path));
        let here = if cfg!(target_os = "macos") { "This Mac" } else { "This Computer" };
        let (glyph, mut machine) = match host {
            Some(alias) => (icons::devices::CLOUD, alias),
            None => (icons::devices::LAPTOP, here.to_owned()),
        };
        // A kernel that dropped, or a start still being tried: say what the
        // window is doing about it.
        if let Some(chat) = workspace.active_session() {
            match (&chat.connection, chat.reconnect_at) {
                (Connection::Lost, Some(at)) => {
                    let left = at
                        .saturating_duration_since(std::time::Instant::now())
                        .as_secs();
                    machine = format!(
                        "{machine} · reconnecting, try {} in {left}s",
                        chat.reconnect_attempt
                    );
                    Painter::of(cx).lease(1.0, Duration::from_millis(1100), cx);
                }
                (Connection::Lost, None) if chat.reconnect_attempt > 0 => {
                    machine = format!("{machine} · connection lost");
                }
                (Connection::Connecting, _) if chat.reconnect_attempt > 0 => {
                    machine = format!("{machine} · reconnecting, try {}…", chat.reconnect_attempt);
                }
                _ => {}
            }
        }
        div()
            .id("composer-context")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .ml(px(-root::COMPOSER_PAD_X + 2.))
            .mr(px(-root::COMPOSER_PAD_X))
            .h(px(24.))
            // Cursor's pills: `⑂ master ⌄` then `▭ This Mac ⌄`. The branch is
            // the project's to change (a terminal, a tool call); the machine
            // is the tab's — the pill opens the picker for a new one.
            .children(branch.map(|branch| {
                div()
                    .id("composer-branch")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .pl(px(8.))
                    .pr(px(4.))
                    .max_w(px(220.))
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .tooltip(|window, cx| Tooltip::text("Branch checked out in this project", window, cx))
                    .child(
                        icons::icon(icons::editing::GIT_BRANCH)
                            .size(px(11.))
                            .flex_none()
                            .text_color(theme.text_faint),
                    )
                    .child(div().truncate().child(SharedString::from(branch)))
                    .child(
                        icons::icon(icons::arrows::ALT_ARROW_DOWN)
                            .size(px(9.))
                            .flex_none()
                            .text_color(theme.text_faint),
                    )
            }))
            .child(
                div()
                    .id("composer-machine")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .pl(px(8.))
                    .pr(px(4.))
                    .max_w(px(260.))
                    .rounded(px(Theme::control_radius()))
                    .cursor_pointer()
                    .hover(|el| el.bg(theme.element_hover))
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .tooltip(|window, cx| Tooltip::with_keystroke("Where the agent runs. Open another machine or folder", "⌘T", window, cx))
                    .on_click(cx.listener(|this, _, window, cx| this.new_tab_action(&crate::view::root::NewTab, window, cx)))
                    .child(
                        icons::icon(glyph)
                            .size(px(11.))
                            .flex_none()
                            .text_color(theme.text_faint),
                    )
                    .child(div().truncate().child(SharedString::from(machine)))
                    .child(
                        icons::icon(icons::arrows::ALT_ARROW_DOWN)
                            .size(px(9.))
                            .flex_none()
                            .text_color(theme.text_faint),
                    ),
            )
            .children(self.try_live_button(theme, cx))
            .children(if self.call.is_some() {
                self.call_strip(theme, cx)
            } else {
                self.voice_status(theme, cx)
            })
            .child(div().flex_1())
            .children(since.map(|since| {
                div()
                    .pr(px(8.))
                    .child(transcript::spinner(since, theme.text_faint, cx))
            }))
            .into_any_element()
    }

    /// The call, in the row under the composer: an orb that breathes with
    /// the mic while the caller talks and waves while Arbos speaks, the live
    /// words (the caller's partial line, then the narrator's last line),
    /// Mute, End. The composer above stays usable: typed words go to the
    /// same agent as `text`.
    fn call_strip(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::voice_ws::Phase;
        let call = self.call.as_ref()?;
        let status = crate::voice_ws::status();
        Painter::of(cx).lease(VOICE_FPS, Duration::from_millis(300), cx);
        let t = voice_phase().as_secs_f32();
        let reduce = cx.reduce_motion();
        let speaking = status.phase == Some(Phase::Speaking);
        let listening = matches!(status.phase, Some(Phase::Listening | Phase::Ready));
        // The orb: 14 px at rest; the mic level swells it while the caller
        // talks, a slow wave while Arbos does. Muted: hollow.
        let swell = if reduce {
            0.0
        } else if speaking {
            0.25 * (t * 5.0).sin().abs()
        } else if listening && !status.muted {
            (status.level * 0.6).min(0.5)
        } else {
            0.0
        };
        let size = 12.0 + 8.0 * swell;
        let orb_color = if call.connecting {
            theme.text_faint
        } else if speaking {
            theme.accent
        } else {
            theme.success
        };
        // The devices — `speaker · mic · level` — are the orb's tooltip, so
        // a silent call is never a mystery and the strip stays one line of
        // words. Only the mic's failure shows inline, in red.
        let mic_error = status.mic_error.clone();
        let mut parts: Vec<String> = Vec::new();
        if !status.speaker_device.is_empty() {
            parts.push(format!("speaker: {}", status.speaker_device));
        }
        if !status.mic_device.is_empty() {
            parts.push(format!("mic: {}", status.mic_device));
            parts.push(format!("level {}%", (status.level * 100.0).round() as u32));
        }
        let devices = parts.join(" · ");
        let mic_line = match &mic_error {
            Some(e) => {
                let e: String = e.split_whitespace().collect::<Vec<_>>().join(" ");
                let e: String = if e.chars().count() > 70 {
                    e.chars().take(70).collect::<String>() + "…"
                } else {
                    e
                };
                format!("mic: {e}")
            }
            None => String::new(),
        };
        let orb = div()
            .id("call-orb")
            .flex_none()
            .w(px(20.))
            .h(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .when(!devices.is_empty(), |el| {
                let devices = devices.clone();
                el.tooltip(move |window, cx| Tooltip::text(devices.clone(), window, cx))
            })
            .child(
                div()
                    .size(px(size))
                    .rounded_full()
                    .when(!status.muted, |el| el.bg(orb_color))
                    .when(status.muted, |el| {
                        el.border_2().border_color(theme.text_faint)
                    }),
            );
        let label = if call.connecting {
            "Calling…".to_string()
        } else if speaking {
            "Arbos".to_string()
        } else if status.muted {
            "Muted".to_string()
        } else {
            "Listening".to_string()
        };
        // One line of live words: what the caller is saying now, else the
        // last thing the narrator said.
        let words = if !status.text.trim().is_empty() && !speaking {
            format!("You · {}", status.text.trim())
        } else if !status.reply.trim().is_empty() && speaking {
            format!("Arbos · {}", status.reply.trim())
        } else if !status.last_said.is_empty() {
            format!("Arbos · {}", status.last_said)
        } else {
            String::new()
        };
        let words: String = words.split_whitespace().collect::<Vec<_>>().join(" ");
        let words: String = if words.chars().count() > 90 {
            words.chars().take(90).collect::<String>() + "…"
        } else {
            words
        };
        let muted = status.muted;
        let mute = theme
            .ghost("call-mute")
            .flex_none()
            .h(px(20.))
            .px(px(6.))
            .rounded(px(4.))
            .items_center()
            .gap(px(4.))
            .text_style(TextStyle::Caption)
            .text_color(if muted {
                theme.danger
            } else {
                theme.text_muted
            })
            .when(muted, |el| el.bg(theme.danger.opacity(0.12)))
            .tooltip(move |window, cx| {
                Tooltip::with_keystroke(if muted { "Unmute" } else { "Mute" }, "⇧⌘M", window, cx)
            })
            .child(
                icons::icon(if muted {
                    icons::media::VOLUME_MUTE
                } else {
                    icons::media::MICROPHONE
                })
                .size(px(11.))
                .text_color(if muted {
                    theme.danger
                } else {
                    theme.text_muted
                }),
            )
            .child(if muted { "Unmute" } else { "Mute" })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx)));
        let end = theme
            .ghost("call-end")
            .flex_none()
            .h(px(20.))
            .px(px(6.))
            .rounded(px(4.))
            .items_center()
            .gap(px(4.))
            .text_style(TextStyle::Caption)
            .text_color(theme.danger)
            .bg(theme.danger.opacity(0.12))
            .tooltip(|window, cx| Tooltip::with_keystroke("End call", "⇧⌘C", window, cx))
            .child(
                svg()
                    .path(crate::assets::PHONE_OFF_ICON)
                    .size(px(11.))
                    .text_color(theme.danger),
            )
            .child("End")
            .on_click(cx.listener(|this, _, _, cx| this.end_call(cx)));
        Some(
            div()
                .id("composer-call-strip")
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .pl(px(8.))
                .min_w_0()
                .text_style(TextStyle::Caption)
                .text_color(theme.text_muted)
                .child(orb)
                .child(SharedString::from(label))
                .when(!words.is_empty(), |row| {
                    row.child(
                        div()
                            .id("call-words")
                            .max_w(px(420.))
                            .truncate()
                            .text_color(theme.text_faint)
                            .child(SharedString::from(words)),
                    )
                })
                .when(!mic_line.is_empty(), |row| {
                    row.child(
                        div()
                            .id("call-mic")
                            .flex_none()
                            .max_w(px(260.))
                            .truncate()
                            .text_color(if mic_error.is_some() {
                                theme.danger
                            } else {
                                theme.text_faint
                            })
                            .child(SharedString::from(mic_line)),
                    )
                })
                .child(mute)
                .child(end)
                .into_any_element(),
        )
    }

    /// Try Live (A-02): the toggle in the row under the composer. Open, it
    /// reads "Live" and closes the view.
    fn try_live_button(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let workspace = self.workspace.read(cx);
        let chat = workspace.active_session()?;
        if !chat.live() {
            return None;
        }
        // The screen a local agent works on is this one: the button would
        // open a view of the window it sits in. It is for a remote place —
        // or a view already open, which needs its way back.
        let remote = workspace
            .active_project()
            .is_some_and(|project| project.host.is_some());
        if !remote && !chat.live_open {
            return None;
        }
        let id = chat.id;
        let open = chat.live_open;
        Some(
            div()
                .id("try-live")
                .flex_none()
                .ml(px(8.))
                .px(px(6.))
                .h(px(20.))
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded(px(Theme::control_radius()))
                .cursor_pointer()
                .text_style(TextStyle::Caption)
                .text_color(if open { theme.text } else { theme.text_faint })
                .when(open, |el| el.bg(theme.element_active))
                .hover(|el| el.bg(theme.element_hover))
                .tooltip(move |window, cx| {
                    Tooltip::text(
                        if open {
                            "Close the live view"
                        } else {
                            "Try Live: watch the screen this agent works on, wherever it runs"
                        },
                        window,
                        cx,
                    )
                })
                .child(
                    icons::icon(icons::devices::LAPTOP)
                        .size(px(11.))
                        .text_color(if open { theme.text } else { theme.text_faint }),
                )
                .child(if open { "Live" } else { "Try Live" })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| workspace.toggle_live(id, cx));
                }))
                .into_any_element(),
        )
    }

    /// The live view: the latest frame of the agent's screen, its machine,
    /// and how old the frame is. An error from the kernel (no display, no
    /// capture tool on that machine) shows in the frame's place.
    fn live_view(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let theme = Theme::of(cx).clone();
        let chat = self.workspace.read(cx).active_session()?;
        if !chat.live_open {
            return None;
        }
        let id = chat.id;
        let screen = chat.live_screen.clone();
        let caption = match &screen {
            None => "Live · asking the kernel for the screen…".to_string(),
            Some(s) => {
                let age = s.at.elapsed().as_secs();
                match &s.error {
                    Some(_) => format!("Live · {} · no frame", s.machine),
                    None => format!(
                        "Live · {} · {}×{} · {}",
                        s.machine,
                        s.width,
                        s.height,
                        if age == 0 {
                            "now".to_string()
                        } else {
                            format!("{age}s ago")
                        }
                    ),
                }
            }
        };
        let width = 640.0_f32;
        let height = screen
            .as_ref()
            .filter(|s| s.width > 0 && s.height > 0)
            .map(|s| (width * s.height as f32 / s.width as f32).clamp(120.0, 480.0))
            .unwrap_or(240.0);
        let body: AnyElement = match screen.as_ref() {
            Some(s) if s.error.is_some() => div()
                .p(px(12.))
                .text_style(TextStyle::Caption)
                .text_color(theme.text_muted)
                .child(s.error.clone().unwrap_or_default())
                .into_any_element(),
            Some(s) if s.image.is_some() => img(s.image.clone().unwrap())
                .w(px(width))
                .h(px(height))
                .rounded(px(Theme::control_radius()))
                .into_any_element(),
            _ => div()
                .w(px(width))
                .h(px(height))
                .flex()
                .items_center()
                .justify_center()
                .child(transcript::spinner(Duration::ZERO, theme.text_faint, cx))
                .into_any_element(),
        };
        Some(
            div()
                .id("live-view")
                .w_full()
                .px(px(root::COMPOSER_PAD_X))
                .py(px(8.))
                .rounded(px(Theme::surface_radius()))
                .bg(theme.surface_raised.opacity(0.7))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .text_style(TextStyle::Caption)
                                .text_color(theme.text_muted)
                                .child(SharedString::from(caption)),
                        )
                        .child(
                            div()
                                .id("live-close")
                                .cursor_pointer()
                                .text_style(TextStyle::Caption)
                                .text_color(theme.text_faint)
                                .hover(|el| el.text_color(theme.text))
                                .child("Close")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.workspace
                                        .update(cx, |workspace, cx| workspace.toggle_live(id, cx));
                                })),
                        ),
                )
                .child(body),
        )
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
            let mut r = status
                .reply
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
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
    pub(crate) fn branch_of(&self, path: &std::path::Path) -> Option<String> {
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
    // A worker just spawned and not yet on its first token counts too
    // while the parent's turn runs: Cursor lists it under Working from the
    // spawn. An idle sub-chat under an idle parent is nobody's work.
    let parent_busy = chat.busy();
    let mut working: Vec<u64> = project
        .sessions
        .iter()
        .filter(|c| {
            c.parent == Some(chat.id)
                && match c.child_state() {
                    crate::model::session::ChildState::Working => true,
                    crate::model::session::ChildState::Waiting => parent_busy,
                    crate::model::session::ChildState::Asking
                    | crate::model::session::ChildState::Done => false,
                }
        })
        .map(|c| c.id)
        .collect();
    working.sort_unstable();
    let mut prs: Vec<String> = Vec::new();
    if chat.host.is_none() {
        if let Some(agent) = chat.agent_session.as_deref() {
            let place = arbos_core::Place::new(&chat.cwd);
            let all = arbos_core::load_prs(&place);
            if !all.is_empty() {
                // The kernel's live agent list places each PR under its
                // opener; a worker the kernel has archived is no longer in
                // it, so this window's own tree names the descendants too.
                let agents = arbos_core::list_agents(&place).unwrap_or_default();
                let mut descendants: Vec<&str> = Vec::new();
                let mut frontier = vec![chat.id];
                while let Some(id) = frontier.pop() {
                    for c in project.sessions.iter().filter(|c| c.parent == Some(id)) {
                        if let Some(a) = c.agent_session.as_deref() {
                            descendants.push(a);
                        }
                        frontier.push(c.id);
                    }
                }
                let tree = arbos_core::prs::prs_of_tree(&all, agent, &agents);
                for p in tree.iter().chain(
                    all.iter()
                        .filter(|p| descendants.contains(&p.agent.as_str())),
                ) {
                    if !prs.contains(&p.url) {
                        prs.push(p.url.clone());
                    }
                }
                if !prs.is_empty() {
                    return (working, prs);
                }
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
            // What the agent is asking of the user right now goes at the end
            // of the conversation, where it was asked.
            let tail: Vec<AnyElement> = [
                self.permission(cx).map(IntoElement::into_any_element),
                self.questions(cx).map(IntoElement::into_any_element),
            ]
            .into_iter()
            .flatten()
            .collect();
            let mut head = self.chat_project_head(cx);
            // A tool-list panic must not skip the composer sibling. The
            // transcript is inline in this render; catch it here.
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.workspace.update(cx, |workspace, cx| {
                    workspace.refresh_children(id);
                    match workspace.session(id) {
                        Some(chat) => {
                            let changes = workspace.changes.get(&chat.cwd).cloned();
                            let head = head.take();
                            transcript::render(chat, changes, head, tail, window, cx)
                        }
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
        // A subagent's chat in Cursor carries no pills; they are the project's.
        if chat.parent.is_some() {
            return None;
        }
        let (working, prs) = pill_counts(project, chat);
        // Cursor's Changes pill: the working tree's uncommitted lines, and
        // beside it Commit & Push. Only for a local repository with changes.
        let tree = (!project.is_remote())
            .then(|| workspace.changes.get(&project.path))
            .flatten()
            .cloned();
        let changes = tree.clone().filter(|changes| !changes.is_empty());
        // A clean tree with commits the upstream lacks: Cursor's "Push".
        let ahead = tree
            .as_ref()
            .filter(|tree| tree.is_empty() && tree.ahead > 0)
            .map(|tree| tree.ahead);
        // A turn stopped mid-way: Cursor offers "Continue Working".
        let stopped = !chat.busy()
            && chat
                .items
                .iter()
                .rev()
                .take_while(|item| !matches!(item, crate::model::session::ChatItem::User(_)))
                .any(|item| matches!(item, crate::model::session::ChatItem::Notice { text, .. } if crate::model::session::is_interrupt_notice(text)));
        let has_agents = project.sessions.iter().any(|c| c.parent == Some(chat.id));
        if working.is_empty() && !has_agents && prs.is_empty() && changes.is_none() && ahead.is_none() && !stopped {
            return None;
        }
        let root = project.path.clone();
        let main_id = chat.id;
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
        // Cursor's Working card: above the pills while workers run, one
        // row per worker with its spinner, Stop All on the right. It opens
        // by itself at fan-out; × puts it away until the next one; the
        // Working pill brings it back.
        let closed = self
            .working_card_closed
            .as_ref()
            .is_some_and(|(id, ids)| *id == main_id && working.iter().all(|w| ids.contains(w)));
        let card_open = !working.is_empty() && !closed;
        let pill_ids = working.clone();
        let workers: Vec<(u64, String, Duration, bool)> = working
            .iter()
            .filter_map(|id| project.sessions.iter().find(|c| c.id == *id))
            .map(|c| (c.id, c.label(), c.elapsed().unwrap_or_default(), true))
            .collect();
        // Cursor's pill once the workers are done reads "Agents" and opens
        // the same list, a check per finished worker.
        let mut agents: Vec<(u64, String, Duration, bool)> = project
            .sessions
            .iter()
            .filter(|c| c.parent == Some(main_id))
            .map(|c| (c.id, c.label(), Duration::ZERO, c.busy()))
            .collect();
        agents.sort_by_key(|(id, ..)| *id);
        let agents_open = working.is_empty() && !agents.is_empty() && self.agents_card_open == Some(main_id);
        let row = div()
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
                            this.working_card_closed = if closed {
                                None
                            } else {
                                Some((main_id, pill_ids.clone()))
                            };
                            cx.notify();
                        })),
                    )
                })
                .when(working.is_empty() && has_agents, |row| {
                    row.child(
                        pill(
                            "pill-agents",
                            svg()
                                .path(crate::assets::DELEGATE_ICON)
                                .size(px(12.))
                                .flex_none()
                                .text_color(theme.text_muted)
                                .into_any_element(),
                            "Agents".to_string(),
                        )
                        .tooltip(|window, cx| Tooltip::text("This chat's sub-agents", window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.agents_card_open = if this.agents_card_open == Some(main_id) {
                                None
                            } else {
                                Some(main_id)
                            };
                            cx.notify();
                        })),
                    )
                })
                .when_some(changes, |row, changes| {
                    let (add, del) = (changes.add(), changes.del());
                    let root_review = root.clone();
                    let root_commit = root.clone();
                    let files = changes.files.len();
                    row.child(
                        pill(
                            "pill-changes",
                            icons::icon(icons::editing::GIT_BRANCH)
                                .size(px(12.))
                                .flex_none()
                                .text_color(theme.text_muted)
                                .into_any_element(),
                            "Changes".to_string(),
                        )
                        .child(diff_marks(&theme, add, del))
                        .tooltip(move |window, cx| {
                            Tooltip::text(
                                format!("{files} file{} changed and not committed. Click to review the diff", if files == 1 { "" } else { "s" }),
                                window,
                                cx,
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.review_changes(&root_review, None, cx);
                        })),
                    )
                    .child(
                        pill(
                            "pill-commit",
                            icons::icon(icons::status::CHECK)
                                .size(px(12.))
                                .flex_none()
                                .text_color(theme.text_muted)
                                .into_any_element(),
                            "Commit & Push".to_string(),
                        )
                        .child(
                            icons::icon(icons::arrows::ALT_ARROW_DOWN)
                                .size(px(9.))
                                .flex_none()
                                .text_color(theme.text_faint),
                        )
                        .tooltip(|window, cx| {
                            Tooltip::text("Ask the agent to commit every change with a clear message and push", window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let _ = &root_commit;
                            this.workspace.update(cx, |workspace, cx| {
                                workspace.send(
                                    main_id,
                                    "Commit all current changes with a clear, conventional commit message and push the branch.".to_string(),
                                    cx,
                                );
                            });
                        })),
                    )
                })
                .when(stopped, |row| {
                    row.child(
                        pill(
                            "pill-continue",
                            icons::icon(icons::arrows::ALT_ARROW_RIGHT)
                                .size(px(12.))
                                .flex_none()
                                .text_color(theme.text_muted)
                                .into_any_element(),
                            "Continue Working".to_string(),
                        )
                        .tooltip(|window, cx| Tooltip::text("Pick the stopped turn back up", window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.workspace.update(cx, |workspace, cx| {
                                workspace.send(main_id, "Continue where you stopped.".to_string(), cx);
                            });
                        })),
                    )
                })
                .when_some(ahead, |row, ahead| {
                    row.child(
                        pill(
                            "pill-push",
                            icons::icon(icons::arrows::ARROW_UP)
                                .size(px(12.))
                                .flex_none()
                                .text_color(theme.text_muted)
                                .into_any_element(),
                            "Push".to_string(),
                        )
                        .tooltip(move |window, cx| {
                            Tooltip::text(format!("{ahead} commit{} not yet pushed", if ahead == 1 { "" } else { "s" }), window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.workspace.update(cx, |workspace, cx| {
                                workspace.send(main_id, "Push the current branch to its remote.".to_string(), cx);
                            });
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
                });
        let card = if card_open {
            Some(self.working_card(&workers, main_id, true, &theme, cx))
        } else if agents_open {
            Some(self.working_card(&agents, main_id, false, &theme, cx))
        } else {
            None
        };
        Some(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(8.))
                .children(card)
                .child(row)
                .into_any_element(),
        )
    }

    /// The card over the pills during a fan-out: "Working" and "Stop All ×"
    /// on one line, then a braille spinner and the worker's name per row.
    /// A row opens the worker; Stop All cancels every running one.
    fn working_card(
        &self,
        workers: &[(u64, String, Duration, bool)],
        main_id: u64,
        live: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ids: Vec<u64> = workers.iter().map(|(id, ..)| *id).collect();
        let close_ids = ids.clone();
        let mut card = div()
            .id("working-card")
            .w_full()
            .rounded(px(Theme::surface_radius()))
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface_raised.opacity(0.6))
            .flex()
            .flex_col()
            .py(px(6.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px(px(12.))
                    .h(px(28.))
                    .text_style(TextStyle::Callout)
                    .child(
                        div()
                            .flex_1()
                            .text_color(theme.text_muted)
                            .child(if live { "Working" } else { "Agents" }),
                    )
                    .when(live, |head| head.child(
                        div()
                            .id("working-stop-all")
                            .px(px(4.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .text_color(theme.text_muted)
                            .hover(|el| el.text_color(theme.text))
                            .child("Stop All")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let ids = ids.clone();
                                this.workspace.update(cx, |workspace, cx| {
                                    for id in ids {
                                        workspace.cancel(id, cx);
                                    }
                                });
                            })),
                    ))
                    .child(
                        div()
                            .id("working-card-close")
                            .ml(px(6.))
                            .size(px(18.))
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_color(theme.text_muted)
                            .hover(|el| el.text_color(theme.text).bg(theme.element_hover))
                            .child(
                                icons::icon(icons::system::CLOSE)
                                    .size(px(11.))
                                    .text_color(theme.text_muted),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if live {
                                    this.working_card_closed = Some((main_id, close_ids.clone()));
                                } else {
                                    this.agents_card_open = None;
                                }
                                cx.notify();
                            })),
                    ),
            );
        for (id, label, since, running) in workers {
            let (id, since, running) = (*id, *since, *running);
            card = card.child(
                div()
                    .id(("working-row", id))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .mx(px(6.))
                    .px(px(6.))
                    .h(px(29.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|el| el.bg(theme.element_hover))
                    .text_style(TextStyle::Callout)
                    .text_color(theme.text)
                    .child(if running {
                        transcript::spinner(since, theme.text_muted, cx)
                    } else {
                        icons::icon(icons::status::CHECK)
                            .size(px(12.))
                            .flex_none()
                            .text_color(theme.success)
                            .into_any_element()
                    })
                    .child(SharedString::from(label.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_session(id, cx))),
            );
        }
        card.into_any_element()
    }

    /// Cursor's Project chat header, over the root chat's first turn: the
    /// project's glyph in its colour, its name, one line about it, and
    /// "View Project Page". A worker's chat draws none.
    fn chat_project_head(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = Theme::of(cx).clone();
        let workspace = self.workspace.read(cx);
        let chat = workspace.active_session()?;
        if chat.parent.is_some() {
            return None;
        }
        let project = workspace.active_project()?;
        let name = crate::model::workspace::Workspace::tab_label(project);
        let glyph = project.identity.glyph();
        let color = project.identity.hsla();
        let line = crate::model::store_view::StoreView::read(&project.path)
            .page
            .as_ref()
            .and_then(|page| page.tldr.first().map(|item| item.label.clone()))
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| {
                format!("Tracks the decisions, agent work, and follow-through needed to move {name} forward.")
            });
        Some(
            div()
                .id("project-head")
                .w_full()
                .max_w(px(root::CHAT_MAX_WIDTH - 2. * root::CHAT_GUTTER))
                .self_center()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .pt(px(28.))
                .pb(px(24.))
                .child(
                    div()
                        .size(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icons::icon(glyph).size(px(24.)).text_color(color)),
                )
                .child(
                    div()
                        .text_style(TextStyle::Headline)
                        .text_color(theme.text)
                        .child(SharedString::from(name)),
                )
                .child(
                    div()
                        .max_w(px(360.))
                        .text_style(TextStyle::Callout)
                        .text_color(theme.text_muted)
                        .text_center()
                        .child(SharedString::from(line)),
                )
                .child(
                    div()
                        .id("project-head-page")
                        .mt(px(6.))
                        .cursor_pointer()
                        .text_style(TextStyle::Callout)
                        .text_color(theme.accent)
                        .child("View Project Page")
                        .on_click(cx.listener(|this, _, _, cx| this.show_pane(Pane::Project, cx))),
                )
                .into_any_element(),
        )
    }

    /// Open the working tree's diff (one file, or all of it) in the column
    /// as a code view — Cursor's Review.
    pub(crate) fn review_changes(&mut self, root: &std::path::Path, file: Option<&str>, cx: &mut Context<Self>) {
        self.workspace
            .update(cx, |workspace, cx| workspace.review_changes(root, file, cx));
    }

    /// The kernel behind this chat has no model key, and this window has
    /// one: offer it. The key belongs to the user, not the machine — it
    /// goes over the connection that is already authenticated, and the
    /// kernel keeps it 0600 (or in memory for "this session only").
    fn provider_offer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = Theme::of(cx).clone();
        let chat = self.workspace.read(cx).active_session()?;
        let provider = chat.provider_missing.clone()?;
        let id = chat.id;
        let mine = arbos_core::Host::load()
            .ok()
            .and_then(|h| h.api_key())
            .is_some();
        let label = match provider.as_str() {
            "openrouter" => "OpenRouter",
            "openai" => "OpenAI",
            other => other,
        }
        .to_string();
        let button = |key: &'static str,
                      text: String,
                      tip: &'static str,
                      remember: bool,
                      cx: &mut Context<Self>| {
            theme
                .ghost(SharedString::from(format!("{key}-{id}")))
                .flex_none()
                .px(px(8.))
                .h(px(22.))
                .flex()
                .items_center()
                .rounded(px(Theme::control_radius()))
                .tooltip(move |window, cx| bezel::ui::tooltip::Tooltip::text(tip, window, cx))
                .child(
                    div()
                        .text_style(TextStyle::Caption)
                        .text_color(theme.text)
                        .child(SharedString::from(text)),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.workspace.update(cx, |workspace, cx| {
                        workspace.with_session(id, cx, |chat| chat.offer_key(remember));
                    });
                }))
        };
        let row = div()
            .id("provider-offer")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(
                icons::icon(icons::status::DANGER_TRIANGLE)
                    .size(px(12.))
                    .text_color(theme.warning),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_style(TextStyle::Callout)
                    .text_color(theme.text_muted)
                    .child(SharedString::from(if mine {
                        format!("This machine has no {label} key.")
                    } else {
                        format!(
                            "This machine has no {label} key, and neither does this window (Settings › Model)."
                        )
                    })),
            )
            .when(mine, |row| {
                row.child(button(
                    "provider-offer-remember",
                    format!("Use my {label} key on this machine"),
                    "Saved in that machine's ~/.config/arbos/config.toml, readable by its owner only",
                    true,
                    cx,
                ))
                .child(button(
                    "provider-offer-session",
                    "This session only".to_string(),
                    "Kept in that kernel's memory; gone when it stops",
                    false,
                    cx,
                ))
            });
        Some(
            div()
                .rounded(px(Theme::surface_radius()))
                .px(px(root::COMPOSER_PAD_X))
                .py(px(8.))
                .child(row)
                .surface(&theme, composer::SURFACE)
                .into_any_element(),
        )
    }

    /// Under the composer only what is about this send: the follow-ups the
    /// kernel holds for this chat. Goals and standing work are the Project
    /// panel's; a parked question is a card in the transcript.
    fn held(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = Theme::of(cx).clone();
        let chat = self.workspace.read(cx).active_session()?;
        let id = chat.id;
        let queued: Vec<PlanNode> = chat
            .plan
            .iter()
            .filter(|n| n.inbox && n.status == "pending" && n.do_kind != "steer")
            .cloned()
            .collect();
        self.followups(id, &queued, &theme, cx)
    }

    /// Messages the kernel holds for this chat that have not run yet
    /// ("Send follow-up" while a turn was busy). One header, a row per
    /// message with its words, and three ways out: send it into the
    /// running turn now, take it back into the composer, or drop it.
    fn followups(
        &self,
        id: u64,
        queued: &[PlanNode],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if queued.is_empty() {
            return None;
        }
        let header = div()
            .id("followups-head")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .child(
                icons::icon(icons::system::CHAT_ROUND_LINE)
                    .size(px(12.))
                    .text_color(theme.text_faint),
            )
            .child(
                div()
                    .text_style(TextStyle::Callout)
                    .text_color(theme.text_muted)
                    .child(SharedString::from(plural(
                        queued.len(),
                        "follow-up queued",
                        "follow-ups queued",
                    ))),
            )
            .child(
                div()
                    .flex_1()
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child("runs when this turn ends"),
            );
        let rows = queued.iter().map(|n| {
            let node = n.id;
            let text = n.goal.clone();
            let group = SharedString::from(format!("followup-{id}-{node}"));
            let control =
                |key: &str,
                 label: &'static str,
                 tip: &'static str,
                 cx: &mut Context<Self>,
                 act: fn(&mut Self, u64, u64, String, &mut Context<Self>)| {
                    let text = text.clone();
                    div()
                        .id(SharedString::from(format!("followup-{key}-{id}-{node}")))
                        .flex_none()
                        .cursor_pointer()
                        .px(px(4.))
                        .rounded(px(4.))
                        .text_style(TextStyle::Caption)
                        .text_color(theme.text_faint)
                        .hover(|el| el.bg(theme.element_hover).text_color(theme.text))
                        .tooltip(move |window, cx| {
                            bezel::ui::tooltip::Tooltip::text(tip, window, cx)
                        })
                        .child(label)
                        .on_click(
                            cx.listener(move |this, _, _, cx| {
                                act(this, id, node, text.clone(), cx)
                            }),
                        )
                };
            div()
                .group(group)
                .flex()
                .flex_row()
                .items_start()
                .gap(px(8.))
                .pl(px(18.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_style(TextStyle::Body)
                        .text_color(theme.text_muted)
                        .child(SharedString::from(text.clone())),
                )
                .child(control(
                    "send",
                    "Send now",
                    "Into the running turn now, at its next step",
                    cx,
                    |this, id, node, text, cx| {
                        this.workspace.update(cx, |workspace, cx| {
                            workspace.with_session(id, cx, |chat| chat.plan_op(node, "cancel", ""));
                            workspace.send(id, text, cx);
                        });
                    },
                ))
                .child(control(
                    "edit",
                    "Edit",
                    "Take it back into the composer",
                    cx,
                    |this, id, node, text, cx| {
                        this.workspace.update(cx, |workspace, cx| {
                            workspace.with_session(id, cx, |chat| {
                                chat.plan_op(node, "cancel", "");
                                chat.draft = text;
                                chat.draft_pushed = true;
                            });
                        });
                    },
                ))
                .child(control(
                    "remove",
                    "Remove",
                    "Drop this follow-up",
                    cx,
                    |this, id, node, _text, cx| {
                        this.workspace.update(cx, |workspace, cx| {
                            workspace.with_session(id, cx, |chat| chat.plan_op(node, "cancel", ""));
                        });
                    },
                ))
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
                .surface(theme, composer::SURFACE)
                .into_any_element(),
        )
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
            // Cursor's approval row, trailing edge: "Skip" as plain words,
            // "Always Run" when the agent offers a standing allow, "Run ↵"
            // as the default. Enter in the empty composer is the same Run.
            Some((deny, allow)) => div()
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(px(8.))
                .child(answer("deny", deny.id(false), "Skip", ButtonStyle::Ghost))
                .children(allow.always.map(|always| {
                    answer("allow-always", always.to_owned(), "Always Run", ButtonStyle::Ghost)
                }))
                .child(answer("allow", allow.id(false), "Run ↵", ButtonStyle::Prominent))
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
                .border_1()
                .border_color(theme.border)
                .bg(theme.surface_raised)
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
                .child(body),
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
        // One question: the card is the question; a header that repeats
        // it is one more thing to read. Several: the set's title heads them.
        let heading = if count <= 1 || prompt.title.is_empty() {
            if count <= 1 {
                "Question".to_owned()
            } else {
                "Questions".to_owned()
            }
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
            prompt.title.clone()
        } else if count <= 1 {
            question.prompt.clone()
        } else {
            format!("{}. {}", page + 1, question.prompt)
        };
        Some(
            div()
                .rounded(px(Theme::surface_radius()))
                .border_1()
                .border_color(theme.border)
                .bg(theme.surface_raised)
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
                ),
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

    /// Prompts typed before the socket was up, waiting on the wire — drawn as
    /// what they are: the message you have already written, not yet sent.
    /// The same bubble the transcript gives a sent one, held back to the
    /// muted tone, and an ✕ to take it back while it is still yours to take
    /// back. A turn in flight holds nothing here: those words steer it.
    fn queue(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let theme = Theme::of(cx).clone();
        let chat = self.workspace.read(cx).active_session()?;
        let waiting: Vec<_> = chat.waiting().collect();
        if waiting.is_empty() {
            return None;
        }
        let id = chat.id;
        Some(div().flex().flex_col().w_full().gap(px(6.)).children(
            waiting.into_iter().enumerate().map(|(ix, text)| {
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
                    .child(
                        div()
                            .id(("edit-queue", ix))
                            .flex_none()
                            .mt(px(2.))
                            .cursor_pointer()
                            .text_style(TextStyle::Callout)
                            .text_color(theme.text_faint)
                            .group_hover(group.clone(), |el| el.text_color(theme.text))
                            .child("Edit")
                            .tooltip(|window, cx| {
                                bezel::ui::tooltip::Tooltip::text(
                                    "Take this back into the composer",
                                    window,
                                    cx,
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.workspace.update(cx, |workspace, cx| {
                                    workspace.with_session(id, cx, |chat| chat.edit_queued(ix));
                                });
                            })),
                    )
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

/// Cursor's `+6 −1`: added in green, removed in red, each only when nonzero.
pub(crate) fn diff_marks(theme: &Theme, add: u32, del: u32) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .text_style(TextStyle::Caption)
        .when(add > 0, |el| {
            el.child(div().text_color(theme.diff_add).child(SharedString::from(format!("+{add}"))))
        })
        .when(del > 0, |el| {
            el.child(div().text_color(theme.diff_del).child(SharedString::from(format!("\u{2212}{del}"))))
        })
        .into_any_element()
}
