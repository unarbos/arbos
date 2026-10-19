//! Root view, Grok Bot layout: chat-list sidebar (the iPhone home list) ·
//! one conversation pane (floating glass header, bubbles, the composer) ·
//! the debug Details drawer. Sheets (Settings, New Message, agent profile)
//! float centred over everything.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    actions, div, prelude::*, px, Animation, AnimationExt, App, Context, Div, Entity, FocusHandle,
    Focusable, FontWeight, Render, ScrollHandle, SharedString, Stateful, Window,
};

use crate::app::SignInMode;
use crate::app::{AppState, DetailsTarget};
use crate::model::{now_ms, CallKind, Link, MachineState};
use crate::ui::avatars;
use crate::ui::callview;
use crate::ui::channels::{ChannelsEvent, ChannelsPanel};
use crate::ui::composer::{Composer, ComposerEvent, ComposerInput};
use crate::ui::connectors::{ConnectorsEvent, ConnectorsPanel};
use crate::ui::feedback::{FeedbackSheet, SendFeedback};
use crate::ui::frost::{frosted, MENU_BLUR, PANEL_BLUR};
use crate::ui::glass::{self, WindowDrag};
use crate::ui::groups;
use crate::ui::header;
use crate::ui::icons::icon;
use crate::ui::local::{LocalEvent, LocalPanel};
use crate::ui::motion::{ease_out_cubic, PulseClock, COMPOSER_DROP_MS, SHAKE_MS};
use crate::ui::onboarding;
use crate::ui::people;
use crate::ui::profiles;
use crate::ui::routines::{RoutinesEvent, RoutinesPanel};
use crate::ui::secrets::{SecretsEvent, SecretsPanel};
use crate::ui::sidebar;
use crate::ui::skills::SkillsPanel;
use crate::ui::theme::{
    reapply_window_background, AppearancePref, Surface, Theme, COLUMN_MARGIN, COMPOSER_WIDTH,
    DRAWER_WIDTH, HEADER_HEIGHT, LINE_HEIGHT, PANEL_RADIUS, PLUS_BUTTON, SIDEBAR_WIDTH,
    TAB_BAR_HEIGHT, TEXT_BODY, TEXT_SMALL, TEXT_TINY,
};
use crate::ui::transcript::{Transcript, TranscriptEvent, COLUMN_MAX_WIDTH};
use crate::ui::voice::Voice;
use crate::ui::voicepicker;

actions!(shell, [CloseTab, NewTab, ToggleRail, Escape, Quit]);

/// Left inset for the conversation header while the sidebar is hidden: the window's traffic lights.
const TRAFFIC_LIGHTS_INSET: f32 = 72.0;

/// Which "…" menu row is expanded on hover (model / effort / voice).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuSubmenu {
    Model,
    Effort,
    Voice,
}

pub struct Shell {
    pub(crate) state: Entity<AppState>,
    transcript: Entity<Transcript>,
    pub(crate) composer: Entity<Composer>,
    /// F11: call strip + mic/TTS owner.
    pub(crate) voice: Entity<Voice>,
    pairing_input: Entity<ComposerInput>,
    pub(crate) project_input: Entity<ComposerInput>,
    /// D79: the swatch picked for the agent being created; `None` = automatic.
    pub(crate) new_agent_color: Option<u8>,
    /// Sidebar search field.
    pub(crate) search_input: Entity<ComposerInput>,
    connectors: Entity<ConnectorsPanel>,
    skills: Entity<SkillsPanel>,
    local: Entity<LocalPanel>,
    focus_handle: FocusHandle,
    pub(crate) settings_open: bool,
    /// Emoji react menu over a bubble: (chat, message, where it was clicked).
    react_menu: Option<(
        crate::model::Id,
        crate::model::Id,
        gpui::Point<gpui::Pixels>,
    )>,
    settings_scroll: ScrollHandle,
    routines: Entity<RoutinesPanel>,
    routines_open: bool,
    channels: Entity<ChannelsPanel>,
    /// F6: the "add a project to this chat" picker (from the "…" menu).
    group_picker_open: bool,
    pub(crate) secrets: Entity<SecretsPanel>,
    shake_started_at: Option<u64>,
    /// True while the sign-in screen is up; its clearing focuses the composer.
    signing_in: bool,
    /// The header's "…" menu.
    pub(crate) menu_open: bool,
    /// Expanded hover submenu inside the "…" menu.
    pub(crate) menu_submenu: Option<MenuSubmenu>,
    /// Search field inside the Model hover submenu.
    pub(crate) model_search_input: Entity<ComposerInput>,
    /// Double-click the agent's name in the header: inline rename.
    pub(crate) renaming_agent: bool,
    pub(crate) agent_rename_input: Entity<ComposerInput>,
    /// Right-click menu on a sidebar row: the space and where the click landed.
    pub(crate) row_menu: Option<(crate::model::Id, gpui::Point<gpui::Pixels>)>,
    /// People chats (D72): the @search field, the header pickers.
    pub people_input: Entity<ComposerInput>,
    /// Agent profile page: inline description editor.
    pub(crate) description_input: Entity<ComposerInput>,
    /// Settings header: editing my display name and handle (`PATCH /me`).
    profile_edit_open: bool,
    name_input: Entity<ComposerInput>,
    handle_input: Entity<ComposerInput>,
    pub add_agent_open: bool,
    pub members_open: bool,
    /// The agent's "Archived" chats list (header strip / "…" menu).
    pub(crate) archived_open: bool,
    /// Per-agent call voice picker sheet ("…" menu → Voice, profile → voice).
    pub(crate) voice_picker_open: bool,
    /// Video picture-in-picture while the call screen is collapsed: offset
    /// from the window's bottom-right (right, bottom), and the drag in flight
    /// (mouse start, offset start, moved past the click threshold).
    pub(crate) pip_offset: (f32, f32),
    pub(crate) pip_drag: Option<(gpui::Point<gpui::Pixels>, (f32, f32), bool)>,
    /// ⌘⇧F "Send feedback" sheet (self-contained in `ui::feedback`).
    feedback: Entity<FeedbackSheet>,
}

impl Shell {
    pub(crate) fn app_state(&self) -> &Entity<AppState> {
        &self.state
    }

    pub(crate) fn set_group_picker(&mut self, open: bool, cx: &mut Context<Self>) {
        self.group_picker_open = open;
        cx.notify();
    }

    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let open_settings = std::env::var("ARBOS_OPEN_SETTINGS")
            .ok()
            .filter(|v| v == "1" || v == "bottom" || v == "connectors" || v.starts_with("secrets"));
        let settings_scroll = ScrollHandle::new();
        if open_settings.as_deref() == Some("bottom") {
            settings_scroll.scroll_to_bottom();
        }
        let transcript = cx.new(|cx| Transcript::new(state.clone(), cx));
        cx.subscribe(
            &transcript,
            |this, _, event: &TranscriptEvent, cx| match event {
                TranscriptEvent::ReactMenu {
                    chat_id,
                    message_id,
                    position,
                } => {
                    this.close_popovers(cx);
                    this.react_menu = Some((chat_id.clone(), message_id.clone(), *position));
                    cx.notify();
                }
            },
        )
        .detach();
        let voice = cx.new(|cx| Voice::new(state.clone(), cx));
        let composer = cx.new(|cx| Composer::new(state.clone(), voice.clone(), cx));
        {
            // Keystrokes in a people chat → presence "typing" (throttled in AppState).
            let composer_input = composer.read(cx).input.clone();
            cx.observe(&composer_input, |this, input, cx| {
                if input.read(cx).text().trim().is_empty() {
                    return;
                }
                let chat = this.state.read(cx).active_chat_id();
                if let Some(chat) = chat {
                    this.state.update(cx, |s, _| s.typed_in(&chat));
                }
            })
            .detach();
        }
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self::follow_system_appearance(window);
        cx.observe(&voice, |_, _, cx| cx.notify()).detach();
        cx.observe(&composer, |_, _, cx| cx.notify()).detach();
        cx.observe_window_activation(window, |this, window, cx| {
            let active = window.is_window_active();
            this.state
                .update(cx, |s, cx| s.set_window_active(active, cx));
        })
        .detach();
        cx.subscribe(
            &composer,
            |this, _, event: &ComposerEvent, cx| match event {
                ComposerEvent::Send(text, attachments) => {
                    let text = text.clone();
                    let attachments = attachments.clone();
                    this.state.update(cx, |s, cx| s.send(text, attachments, cx));
                }
                ComposerEvent::Stop => this.state.update(cx, |s, cx| s.stop(cx)),
            },
        )
        .detach();
        Composer::focus(&composer, window, cx);

        let pairing_input = cx.new(ComposerInput::new);
        {
            let weak = cx.weak_entity();
            pairing_input.update(cx, |input, _| {
                input.set_placeholder("One-time pairing code");
                input.on_submit = Some(Rc::new(move |_window, cx| {
                    let _ = weak.update(cx, |this, cx| this.submit_pairing(cx));
                }));
            });
        }
        let project_input = cx.new(ComposerInput::new);
        {
            let weak = cx.weak_entity();
            project_input.update(cx, |input, _| {
                input.set_placeholder("Agent name");
                input.on_submit = Some(Rc::new(move |window, cx| {
                    let _ = weak.update(cx, |this, cx| this.submit_project(window, cx));
                }));
            });
        }
        let search_input = cx.new(ComposerInput::new);
        search_input.update(cx, |input, _| input.set_placeholder("Search"));
        cx.observe(&pairing_input, |_, _, cx| cx.notify()).detach();
        cx.observe(&project_input, |_, _, cx| cx.notify()).detach();
        cx.observe(&search_input, |_, _, cx| cx.notify()).detach();
        let description_input = cx.new(ComposerInput::new);
        description_input.update(cx, |input, _| {
            input.set_placeholder("What does this agent do?")
        });
        cx.observe(&description_input, |_, _, cx| cx.notify())
            .detach();
        let name_input = cx.new(ComposerInput::new);
        name_input.update(cx, |input, _| input.set_placeholder("Display name"));
        cx.observe(&name_input, |_, _, cx| cx.notify()).detach();
        let model_search_input = cx.new(ComposerInput::new);
        model_search_input.update(cx, |input, _| input.set_placeholder("Search models"));
        cx.observe(&model_search_input, |_, _, cx| cx.notify())
            .detach();
        let agent_rename_input = cx.new(ComposerInput::new);
        {
            let weak = cx.weak_entity();
            agent_rename_input.update(cx, |input, _| {
                input.set_placeholder("Agent name");
                input.on_submit = Some(Rc::new(move |window, cx| {
                    let _ = weak.update(cx, |this, cx| this.commit_agent_rename(window, cx));
                }));
            });
        }
        cx.observe(&agent_rename_input, |_, _, cx| cx.notify())
            .detach();
        let handle_input = cx.new(ComposerInput::new);
        {
            let weak = cx.weak_entity();
            handle_input.update(cx, |input, _| {
                input.set_placeholder("handle");
                input.on_submit = Some(Rc::new(move |_window, cx| {
                    let _ = weak.update(cx, |this, cx| this.save_profile(cx));
                }));
            });
        }
        cx.observe(&handle_input, |_, _, cx| cx.notify()).detach();
        let people_input = cx.new(ComposerInput::new);
        {
            let weak = cx.entity().downgrade();
            people_input.update(cx, |input, _| {
                input.set_placeholder("Search people");
                input.on_submit = Some(Rc::new(move |_window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        let typed = this.people_input.read(cx).text().trim().to_string();
                        let pick = {
                            let s = this.state.read(cx);
                            s.people_results
                                .first()
                                .map(|p| p.handle.clone())
                                .or_else(|| (!typed.is_empty()).then_some(typed.clone()))
                        };
                        if let Some(handle) = pick {
                            this.state.update(cx, |s, cx| s.pick_person(handle, cx));
                        }
                    });
                }));
            });
        }
        cx.observe(&people_input, |this, input, cx| {
            let text = input.read(cx).text().trim().to_string();
            if this.state.read(cx).new_chat_open && this.state.read(cx).people_query != text {
                this.state.update(cx, |s, cx| s.search_people(text, cx));
            }
            cx.notify();
        })
        .detach();
        // Rig: `ARBOS_OPEN_NEW_AGENT=1` opens the New Message sheet with the
        // "New agent" form expanded.
        if std::env::var("ARBOS_OPEN_NEW_AGENT").is_ok_and(|v| v == "1") {
            state.update(cx, |s, cx| {
                s.open_new_chat(cx);
                s.new_project_open = true;
                cx.notify();
            });
        }
        let connectors = cx.new(|cx| ConnectorsPanel::new(state.clone(), cx));
        if open_settings.as_deref() == Some("connectors") {
            connectors.update(cx, |p, cx| p.show(cx));
        }
        cx.observe(&connectors, |_, _, cx| cx.notify()).detach();
        cx.subscribe(
            &connectors,
            |this, _, event: &ConnectorsEvent, cx| match event {
                ConnectorsEvent::Back => {
                    this.settings_open = true;
                    cx.notify();
                }
            },
        )
        .detach();
        let local = cx.new(|cx| LocalPanel::new(state.clone(), cx));
        cx.observe(&local, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&local, |this, _, event: &LocalEvent, cx| match event {
            LocalEvent::Back => {
                this.settings_open = true;
                cx.notify();
            }
        })
        .detach();
        let secrets = cx.new(|cx| SecretsPanel::new(state.clone(), cx));
        // Rig: `ARBOS_OPEN_SETTINGS=secrets` (the list), `secrets:<secret env>` (one secret),
        // `secrets-agent:<project id or name>` (an agent's page).
        match open_settings.as_deref() {
            Some("secrets") => secrets.update(cx, |p, cx| p.show(cx)),
            Some(v) if v.starts_with("secrets:") => {
                let env = v["secrets:".len()..].to_string();
                secrets.update(cx, |p, cx| p.show_secret_env(env, cx));
            }
            Some(v) if v.starts_with("secrets-agent:") => {
                let pid = v["secrets-agent:".len()..].to_string();
                secrets.update(cx, |p, cx| p.show_agent(pid, cx));
            }
            Some("secrets-add") => secrets.update(cx, |p, cx| p.show_add(cx)),
            Some("secrets-connect") => secrets.update(cx, |p, cx| p.show_connect(cx)),
            Some("secrets-vault") => secrets.update(cx, |p, cx| p.show_first_vault(cx)),
            _ => {}
        }
        cx.observe(&secrets, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&secrets, |this, _, event: &SecretsEvent, cx| match event {
            SecretsEvent::Back => {
                this.settings_open = true;
                cx.notify();
            }
        })
        .detach();
        let routines = cx.new(|cx| RoutinesPanel::new(state.clone(), cx));
        cx.subscribe(
            &routines,
            |this, _, event: &RoutinesEvent, cx| match event {
                RoutinesEvent::Close => {
                    this.routines_open = false;
                    cx.notify();
                }
            },
        )
        .detach();
        let channels = cx.new(|cx| ChannelsPanel::new(state.clone(), cx));
        cx.observe(&channels, |_, _, cx| cx.notify()).detach();
        cx.subscribe(
            &channels,
            |this, _, event: &ChannelsEvent, cx| match event {
                ChannelsEvent::Back => {
                    this.settings_open = true;
                    cx.notify();
                }
            },
        )
        .detach();
        let skills = cx.new(|cx| SkillsPanel::new(state.clone(), cx));
        cx.observe(&skills, |_, _, cx| cx.notify()).detach();
        let feedback = cx.new(|cx| FeedbackSheet::new(state.clone(), cx));
        cx.observe(&feedback, |_, _, cx| cx.notify()).detach();

        Self {
            state,
            transcript,
            composer,
            voice,
            pairing_input,
            project_input,
            new_agent_color: None,
            search_input,
            connectors,
            skills,
            local,
            focus_handle: cx.focus_handle(),
            // `ARBOS_OPEN_SETTINGS=1|bottom|connectors` opens the sheet at launch
            // (`bottom` scrolled to its end, `connectors` the Connectors panel) — rig.
            settings_open: open_settings
                .as_deref()
                .is_some_and(|v| v != "connectors" && !v.starts_with("secrets")),
            settings_scroll,
            row_menu: None,
            react_menu: None,
            group_picker_open: false,
            archived_open: false,
            voice_picker_open: false,
            pip_offset: (24.0, 96.0),
            pip_drag: None,
            routines,
            routines_open: false,
            channels,
            secrets,
            shake_started_at: None,
            signing_in: false,
            menu_open: false,
            menu_submenu: None,
            model_search_input,
            renaming_agent: false,
            agent_rename_input,
            people_input,
            description_input,
            profile_edit_open: false,
            name_input,
            handle_input,
            add_agent_open: false,
            members_open: false,
            feedback,
        }
    }

    /// Double-click the agent's name in the header: show an inline rename field.
    pub(crate) fn start_agent_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(space) = self.state.read(cx).selected().cloned() else {
            return;
        };
        if space.is_people() || space.is_guest() || space.is_groupish() {
            return;
        }
        let name = space.name.clone();
        // A double-click also fires a single-click that may have opened the profile.
        self.state.update(cx, |s, cx| {
            s.close_profile(cx);
        });
        self.renaming_agent = true;
        self.menu_open = false;
        self.menu_submenu = None;
        self.agent_rename_input
            .update(cx, |input, cx| input.set_text(name, cx));
        let handle = self.agent_rename_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    pub(crate) fn commit_agent_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.renaming_agent {
            return;
        }
        let name = self.agent_rename_input.read(cx).text().trim().to_string();
        self.renaming_agent = false;
        if !name.is_empty() {
            self.state.update(cx, |s, cx| s.set_project_name(name, cx));
        }
        Composer::focus(&self.composer, window, cx);
        cx.notify();
    }

    pub(crate) fn cancel_agent_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.renaming_agent {
            return;
        }
        self.renaming_agent = false;
        Composer::focus(&self.composer, window, cx);
        cx.notify();
    }

    fn send_feedback(&mut self, _: &SendFeedback, window: &mut Window, cx: &mut Context<Self>) {
        self.feedback.update(cx, |sheet, cx| sheet.open(window, cx));
    }

    pub(crate) fn open_feedback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = false;
        self.settings_open = false;
        self.feedback.update(cx, |sheet, cx| sheet.open(window, cx));
    }

    /// Settings › Credits: the feedback sheet, pre-addressed as a top-up request.
    fn request_top_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_feedback(window, cx);
        self.feedback.update(cx, |sheet, cx| {
            sheet.note.update(cx, |note, cx| {
                if note.text().trim().is_empty() {
                    note.set_text("Top-up request: ".into(), cx);
                }
            });
        });
    }

    fn submit_pairing(&mut self, cx: &mut Context<Self>) {
        let code = self.pairing_input.read(cx).text().trim().to_string();
        if code.is_empty() {
            return;
        }
        self.state.update(cx, |s, cx| s.pair(code, cx));
    }

    pub(crate) fn submit_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.project_input.read(cx).text().trim().to_string();
        if name.is_empty() {
            return;
        }
        self.project_input.update(cx, |input, cx| input.clear(cx));
        let color = self.new_agent_color.take();
        self.state.update(cx, |s, cx| {
            s.new_project_open = false;
            s.close_new_chat(cx);
            s.create_project(name, color, cx);
        });
        Composer::focus(&self.composer, window, cx);
    }

    fn close_tab(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        let active = self.state.read(cx).active_chat_id();
        if let Some(chat_id) = active {
            let closed = self.state.update(cx, |s, cx| s.close_tab(chat_id, cx));
            if !closed {
                self.shake_started_at = Some(now_ms());
                cx.notify();
            }
        }
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.new_tab(cx));
        Composer::focus(&self.composer, window, cx);
    }

    fn toggle_rail(&mut self, _: &ToggleRail, window: &mut Window, cx: &mut Context<Self>) {
        let width: f32 = window.viewport_size().width.into();
        self.state.update(cx, |s, cx| {
            let narrow = s.window_is_narrow(width);
            s.toggle_rail(narrow, cx)
        });
    }

    /// Escape closes whatever floats: menus, then sheets.
    fn escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming_agent {
            self.cancel_agent_rename(window, cx);
            return;
        }
        let floating = self.menu_open
            || self.members_open
            || self.add_agent_open
            || self.group_picker_open
            || self.row_menu.is_some()
            || self.react_menu.is_some();
        self.close_popovers(cx);
        if floating {
            return;
        }
        if self.feedback.read(cx).is_open() {
            self.feedback.update(cx, |sheet, cx| sheet.close(cx));
            Composer::focus(&self.composer, window, cx);
            cx.notify();
            return;
        }
        if self.state.read(cx).sidebar_overlay {
            self.state.update(cx, |s, cx| s.close_sidebar_overlay(cx));
            return;
        }
        let mut closed = false;
        if self.settings_open {
            self.settings_open = false;
            closed = true;
        }
        self.state.update(cx, |s, cx| {
            if s.new_chat_open {
                s.new_project_open = false;
                s.close_new_chat(cx);
                closed = true;
            }
            if s.close_profile(cx) {
                closed = true;
            }
        });
        if closed {
            Composer::focus(&self.composer, window, cx);
            cx.notify();
        }
    }

    fn set_theme(&mut self, pref: AppearancePref, surface: Surface, cx: &mut Context<Self>) {
        let current = Theme::of(cx);
        if current.pref != pref {
            pref.save();
        }
        if current.surface != surface {
            surface.save();
        }
        cx.set_global(Theme::for_pref(pref, surface, cx));
        reapply_window_background(cx);
        cx.notify();
    }

    /// System follows macOS: re-resolve the theme when its appearance changes (D81).
    fn follow_system_appearance(window: &mut Window) {
        window
            .observe_window_appearance(|window, cx| {
                let theme = Theme::of(cx);
                let system = window.appearance();
                crate::ui::theme::note_system_appearance(system);
                if theme.pref != AppearancePref::System {
                    return;
                }
                let wanted = AppearancePref::System.resolve(system);
                if wanted != theme.appearance {
                    let surface = theme.surface;
                    cx.set_global(Theme::for_pref(AppearancePref::System, surface, cx));
                    reapply_window_background(cx);
                    window.refresh();
                }
            })
            .detach();
    }

    /// Everything that floats closes on a click elsewhere.
    fn close_popovers(&mut self, cx: &mut Context<Self>) {
        if self.menu_open
            || self.members_open
            || self.add_agent_open
            || self.group_picker_open
            || self.archived_open
            || self.row_menu.is_some()
            || self.react_menu.is_some()
            || self.menu_submenu.is_some()
        {
            self.menu_open = false;
            self.menu_submenu = None;
            self.members_open = false;
            self.add_agent_open = false;
            self.group_picker_open = false;
            self.archived_open = false;
            self.row_menu = None;
            self.react_menu = None;
            cx.notify();
        }
        if self.composer.read(cx).open_picker.is_some() {
            self.composer.update(cx, |c, cx| {
                c.open_picker = None;
                cx.notify();
            });
        }
    }

    // ----- react menu -----------------------------------------------------------------
    /// The quick-react bar (iPhone parity: the long-press palette) at the click.
    fn react_menu(&self, cx: &mut Context<Self>) -> Option<Div> {
        let (chat_id, message_id, at) = self.react_menu.clone()?;
        let theme = Theme::of(cx).clone();
        let me = format!("user:{}", self.state.read(cx).me_id());
        let held: Vec<String> = self
            .state
            .read(cx)
            .messages
            .get(&chat_id)
            .and_then(|list| list.iter().find(|m| m.id == message_id))
            .map(|m| {
                m.reaction_list()
                    .into_iter()
                    .filter(|(_, s)| s.contains(&me))
                    .map(|(e, _)| e)
                    .collect()
            })
            .unwrap_or_default();
        let mut bar = div().flex().items_center().gap(px(2.0)).p(px(4.0));
        for emoji in crate::model::REACTION_PALETTE {
            let on = held.iter().any(|e| e == emoji);
            let (chat, msg) = (chat_id.clone(), message_id.clone());
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("react-{emoji}")))
                    .size(px(32.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .when(on, |el| el.bg(Theme::with_alpha(theme.accent, 0.22)))
                    .hover(|s| s.bg(theme.surface_2))
                    .text_size(px(18.0))
                    .child(emoji)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.react_menu = None;
                        let (chat, msg) = (chat.clone(), msg.clone());
                        this.state.update(cx, |s, cx| {
                            s.toggle_reaction(chat, msg, emoji.to_string(), cx)
                        });
                        cx.notify();
                    })),
            );
        }
        // Appended straight to the root (see sidebar::row_menu): sits just above the click.
        Some(
            div()
                .absolute()
                .left(at.x - px(16.0))
                .top(at.y - px(48.0))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .rounded_full()
                        .bg(theme.card())
                        .border_1()
                        .border_color(theme.border_strong)
                        .shadow_lg()
                        .child(bar),
                ),
        )
    }

    // ----- update pill (sidebar bottom) ------------------------------------------

    /// The Update button (D42): absent while up to date; appears the moment
    /// the feed has a newer build; one click downloads, verifies, swaps the
    /// bundle, and relaunches. It sits at the bottom of the chat list.
    pub(crate) fn render_update_button(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let theme = Theme::of(cx).clone();
        let update = self.state.read(cx).update.clone();
        if !update.button_visible() {
            return None;
        }
        let busy = update.busy();
        let failed = matches!(update.phase, crate::update::UpdatePhase::Failed { .. });
        if busy {
            PulseClock::request(cx);
        }
        let phase = if busy {
            PulseClock::phase(cx, 1400)
        } else {
            1.0
        };
        let bg = if failed {
            theme.danger_soft
        } else {
            Theme::with_alpha(theme.primary, 0.75 + 0.25 * phase)
        };
        let fg = if failed { theme.text } else { theme.on_primary };
        let label = update.button_label();
        Some(
            div()
                .id("rail-update")
                .w_full()
                .h(px(38.0))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.0))
                .rounded_full()
                .bg(bg)
                .text_color(fg)
                .text_size(px(TEXT_SMALL))
                .font_weight(FontWeight::SEMIBOLD)
                .when(!busy, |el| el.cursor_pointer().hover(|s| s.opacity(0.88)))
                .child(icon(if failed { "xmark" } else { "arrow-down" }, 14.0, fg))
                .child(label)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.install_update(cx));
                })),
        )
    }

    // ----- banners (S11–S14) ------------------------------------------

    fn render_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        let theme = Theme::of(cx).clone();
        let state = self.state.read(cx);
        let link = state.link;
        let machine = state.machine.clone();
        state.selected()?;

        let banner = |bg: gpui::Hsla, fg: gpui::Hsla| {
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(10.0))
                .px(px(16.0))
                .py(px(6.0))
                .bg(bg)
                .text_size(px(12.0))
                .text_color(fg)
        };

        if link == Link::Reconnecting {
            return Some(
                banner(theme.surface_1, theme.text_muted)
                    .child(cards_spinner(&theme, cx))
                    .child("Reconnecting… the transcript stays readable from your local replica."),
            );
        }

        match machine.shown_state() {
            MachineState::Waking => {
                PulseClock::request(cx);
                let detail = machine
                    .detail
                    .clone()
                    .unwrap_or_else(|| "Starting your machine…".to_string());
                let mut b = banner(theme.surface_1, theme.text)
                    .child(cards_spinner(&theme, cx))
                    .child(detail);
                if let Some(progress) = machine.progress {
                    b = b.child(
                        div()
                            .flex_1()
                            .h(px(4.0))
                            .rounded_full()
                            .bg(theme.border)
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(theme.accent)
                                    .w(gpui::relative(progress.clamp(0.02, 1.0))),
                            ),
                    );
                } else {
                    b = b.child(div().flex_1());
                }
                Some(
                    b.child(
                        div()
                            .text_color(theme.text_faint)
                            .child("messages are queued"),
                    ),
                )
            }
            // Grok (refs 69/70): no red strip — a quiet block with a small
            // monitor glyph, a short title, the reason in muted text and a
            // capsule "Retry".
            MachineState::Error => {
                let detail = machine.detail.clone().unwrap_or_else(|| {
                    "Your agents are safe — their machine just can't be reached right now."
                        .to_string()
                });
                Some(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .px(px(COLUMN_MARGIN))
                        .pt(px(12.0))
                        .pb(px(8.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .text_size(px(TEXT_SMALL))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .child(icon("window", 13.0, theme.text_muted))
                                .child("Can't reach your machine"),
                        )
                        .child(
                            div()
                                .text_size(px(TEXT_TINY + 0.5))
                                .text_color(theme.text_muted)
                                .max_w(px(480.0))
                                .child(detail),
                        )
                        .child(
                            div().pt(px(4.0)).flex().child(
                                glass::quiet_button("banner-retry", "Retry", &theme)
                                    .h(px(30.0))
                                    .px(px(14.0))
                                    .bg(theme.surface_2)
                                    .border_color(gpui::transparent_black())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state.update(cx, |s, cx| s.machine_start(cx));
                                    })),
                            ),
                        ),
                )
            }
            MachineState::Asleep
            | MachineState::Awake
            | MachineState::Stopped
            | MachineState::Unknown => None,
        }
    }

    // ----- sidebar overlay (narrow window) -------------------------------------

    /// The sidebar floating over the chat when the window is too narrow to
    /// dock it: a dimmed scrim (click closes) and the usual sidebar at the left.
    fn render_sidebar_overlay(&self, theme: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("sidebar-overlay")
            .absolute()
            .inset_0()
            .bg(Theme::with_alpha(theme.bg, 0.45))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.close_sidebar_overlay(cx));
                }),
            )
            .child(
                div()
                    .id("sidebar-overlay-panel")
                    .absolute()
                    .left_0()
                    .top_0()
                    .h_full()
                    .w(px(SIDEBAR_WIDTH))
                    .bg(theme.shell())
                    .border_r_1()
                    .border_color(theme.border)
                    .shadow_lg()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(frosted(0.0, PANEL_BLUR, sidebar::render(self, cx))),
            )
    }

    // ----- conversation pane ---------------------------------------------------

    fn render_center(&self, sidebar_hidden: bool, cx: &mut Context<Self>) -> Div {
        let theme = Theme::of(cx).clone();
        let state = self.state.read(cx);
        let project = state.selected().cloned();
        let chat_id = state.active_chat_id();
        let ui = chat_id
            .as_deref()
            .map(|id| state.chat_ui(id))
            .unwrap_or_default();
        let has_messages = chat_id
            .as_deref()
            .map(|id| !state.messages_of(id).is_empty())
            .unwrap_or(false);

        let dropped = ui.composer_dropped || has_messages;
        let drop_elapsed = ui
            .drop_started_at
            .map(|t| now_ms().saturating_sub(t))
            .unwrap_or(COMPOSER_DROP_MS + 1);
        let animating = dropped && drop_elapsed < COMPOSER_DROP_MS;
        let shake = self
            .shake_started_at
            .map(|t| now_ms().saturating_sub(t))
            .filter(|dt| *dt < SHAKE_MS);

        // Everything the empty-state hero needs, read before `cx` is borrowed mutably.
        let hero_data = project.as_ref().map(|space| {
            (
                state.avatar_state(&space.id),
                space
                    .person()
                    .filter(|_| space.is_people())
                    .and_then(|p| state.avatar(&p.avatar_url)),
            )
        });
        let typing = chat_id.as_deref().and_then(|id| state.typing_in(id));
        if animating || shake.is_some() {
            PulseClock::request(cx);
        }

        let composer_block = div()
            .w_full()
            .max_w(px(COMPOSER_WIDTH.max(COLUMN_MAX_WIDTH + 2.0 * PLUS_BUTTON)))
            .child(self.composer.clone());

        let mut column = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full();

        let Some(space) = project else {
            // Nothing selected: the window title strip stays draggable.
            return column
                .child(
                    div()
                        .h(px(HEADER_HEIGHT))
                        .w_full()
                        .window_drag(),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(8.0))
                        .px(px(28.0))
                        .child(
                            div()
                                .text_size(px(TEXT_BODY + 2.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .child("Pick a chat"),
                        )
                        .child(
                            div()
                                .text_size(px(TEXT_SMALL))
                                .text_color(theme.text_muted)
                                .text_center()
                                .child(
                                    "Choose an agent or a person on the left, or press + for a new one.",
                                ),
                        ),
                );
        };

        let shake_offset = shake
            .map(|dt| ((dt as f32 / SHAKE_MS as f32) * std::f32::consts::PI * 4.0).sin() * 3.0)
            .unwrap_or(0.0);
        column = column.child(
            div()
                .relative()
                .left(px(shake_offset))
                // Without the sidebar the traffic lights sit over this header:
                // clear them (14 pt in, three 12 pt lights, 8 pt gaps).
                .when(sidebar_hidden, |el| el.pl(px(TRAFFIC_LIGHTS_INSET)))
                .child(header::render(self, &space, sidebar_hidden, cx)),
        );
        if let Some(chips) = header::chips(self, &space, cx) {
            column = column.child(chips);
        }
        // Calls v1: my call (bar, or the full screen below), or a live call to join.
        let my_call = self.state.read(cx).my_call.clone();
        let call_here = self.state.read(cx).calls_in_chat.get(&space.id).cloned();
        let screen_open = my_call
            .as_ref()
            .map(|c| c.expanded && c.call.space_id() == space.id)
            .unwrap_or(false);
        if let Some(active) = my_call.as_ref().filter(|_| !screen_open) {
            column = column.child(callview::bar(self, active, cx));
        } else if let Some(call) = call_here.filter(|c| {
            c.is_live()
                && my_call
                    .as_ref()
                    .map(|m| m.call.call_id != c.call_id)
                    .unwrap_or(true)
        }) {
            column = column.child(callview::join_bar(&call, cx));
        }
        if let Some(banner) = self.render_banner(cx) {
            column = column.child(banner);
        }
        if screen_open {
            if let Some(active) = my_call.as_ref() {
                return column.child(callview::screen(self, active, cx));
            }
        }

        if !dropped {
            // S1/S2: empty chat — the agent's picture and a line, composer centred.
            let is_people = space.is_people();
            let name = if space.is_people_group() {
                space.name.clone()
            } else if is_people {
                space
                    .person()
                    .map(|p| p.display_name())
                    .unwrap_or_else(|| space.name.clone())
            } else {
                space.name.clone()
            };
            let hint = if is_people {
                "Say hi. It lands for both of you at once; Add agent brings one of yours in."
                    .to_string()
            } else {
                format!("Ask {name} anything. It plans, delegates, or just answers.")
            };
            let mut hero = div().flex().flex_col().items_center().gap(px(10.0));
            let (avatar_state, person_image) =
                hero_data.unwrap_or((avatars::AvatarState::Idle, None));
            let _ = avatar_state;
            if !is_people {
                hero = hero.child(avatars::project(&space, 88.0, &theme));
            } else if let Some(person) = space.person() {
                hero = hero.child(people::avatar(person_image, person, 88.0, &theme));
            }
            column = column.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(14.0))
                    .px(px(28.0))
                    .child(hero)
                    .child(
                        div()
                            .text_size(px(TEXT_BODY + 5.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_SMALL))
                            .text_color(theme.text_muted)
                            .text_center()
                            .max_w(px(460.0))
                            .child(hint),
                    )
                    .child(
                        div()
                            .w(px(COMPOSER_WIDTH))
                            .max_w_full()
                            .pt(px(12.0))
                            .child(self.composer.clone()),
                    ),
            );
        } else {
            // Transcript above, composer pinned at the bottom. On the first
            // send the composer slides down from the centre over 220 ms using
            // a relative `top` inset (GPUI has no transform).
            let body = div()
                .flex_1()
                .min_h_0()
                .relative()
                .child(self.transcript.clone());
            if typing.is_some() {
                PulseClock::request(cx);
            }
            let composer_row = div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .px(px(COLUMN_MARGIN))
                .pb(px(14.0))
                .pt(px(6.0))
                .when_some(typing, |el, handle| {
                    el.child(people::typing_line(&handle, &theme))
                })
                .child(composer_block);
            let composer_row = if animating {
                composer_row
                    .relative()
                    .with_animation(
                        "composer-drop",
                        Animation::new(Duration::from_millis(COMPOSER_DROP_MS))
                            .with_easing(ease_out_cubic),
                        |el, t| el.top(px(-240.0 * (1.0 - t))),
                    )
                    .into_any_element()
            } else {
                composer_row.into_any_element()
            };
            // F11 call strip renders above the composer only during a call.
            column = column
                .child(body)
                .child(self.voice.clone())
                .child(composer_row);
        }
        column
    }

    fn render_details(&self, target: &DetailsTarget, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let state = self.state.read(cx);
        let asleep = state.box_state().map(|s| s.is_asleep()).unwrap_or(false);
        let mut sections: Vec<(String, Vec<(String, String)>)> = Vec::new();
        let mut preview: Option<String> = None;
        let mut blob_key: Option<String> = None;
        let title = match target {
            DetailsTarget::Message {
                chat_id,
                message_id,
            } => {
                if let Some(m) = state
                    .messages_of(chat_id)
                    .iter()
                    .find(|m| &m.id == message_id)
                {
                    let mut rows = Vec::new();
                    rows.push(("Message".into(), m.id.clone()));
                    rows.push(("Status".into(), format!("{:?}", m.status)));
                    rows.push((
                        "Routing decision".into(),
                        m.decision
                            .clone()
                            .or(m.turn.map(|t| format!("{t:?}").to_lowercase()))
                            .unwrap_or_else(|| "—".into()),
                    ));
                    if let Some(model) = &m.model {
                        rows.push(("Model".into(), model.clone()));
                    }
                    if let Some(s) = &m.subagent_id {
                        rows.push(("Subagent".into(), s.clone()));
                    }
                    if let Some(r) = &m.in_reply_to {
                        rows.push(("In reply to".into(), r.clone()));
                    }
                    sections.push(("Turn".into(), rows));
                    let usage = m.usage.clone().unwrap_or(serde_json::Value::Null);
                    let mut tokens = json_rows(&usage);
                    if tokens.is_empty() {
                        tokens.push(("Tokens".into(), "not reported yet".into()));
                    }
                    sections.push(("Context pack · tokens".into(), tokens));
                    let pulls: Vec<(String, String)> = m
                        .tool_parts()
                        .filter_map(|p| match p {
                            crate::model::Part::Tool {
                                call,
                                output_summary,
                                output_ref,
                                ..
                            } => Some((
                                format!("{} {}", call.name, call.call_input_short()),
                                match output_ref {
                                    Some(r) => format!("{output_summary} · blob {r}"),
                                    None => output_summary.clone(),
                                },
                            )),
                            _ => None,
                        })
                        .collect();
                    sections.push(("Pull log · tool calls".into(), pulls));
                    let steps: Vec<(String, String)> = m
                        .parts
                        .iter()
                        .chain(m.hidden_parts.iter())
                        .filter_map(|p| match p {
                            crate::model::Part::Status { code, message, .. } => {
                                Some((format!("{code:?}").to_lowercase(), message.clone()))
                            }
                            _ => None,
                        })
                        .collect();
                    if !steps.is_empty() {
                        sections.push(("Steps · status events".into(), steps));
                    }
                    if let Some(name) = &m.subagent_name {
                        sections.push(("Subagent".into(), vec![("Name".into(), name.clone())]));
                    }
                    // S18: one audit row per card, requested → decided (feature `approve`).
                    let approvals: Vec<(String, String)> = m
                        .parts
                        .iter()
                        .chain(m.hidden_parts.iter())
                        .filter_map(|p| match p {
                            crate::model::Part::Approval {
                                approval_id,
                                title,
                                risk,
                                state,
                                decided_by,
                                decided_at,
                                reason,
                                ..
                            } => {
                                let mut detail = format!(
                                    "approval_requested · {} · {} · {}",
                                    risk.label(),
                                    title,
                                    approval_id
                                );
                                if *state != crate::model::ApprovalState::Pending {
                                    detail.push_str(&format!(
                                        "\napproval_decided · {}{}{}",
                                        state.label().to_lowercase(),
                                        decided_by
                                            .as_deref()
                                            .map(|d| format!(" · by {d}"))
                                            .unwrap_or_default(),
                                        reason
                                            .as_deref()
                                            .filter(|r| !r.trim().is_empty())
                                            .map(|r| format!(" · {r}"))
                                            .unwrap_or_default()
                                    ));
                                    if let Some(at) = decided_at {
                                        let mins =
                                            crate::model::now_ms().saturating_sub(*at) / 60_000;
                                        detail.push_str(&format!(" · {mins} min ago"));
                                    }
                                }
                                Some((state.label().to_string(), detail))
                            }
                            _ => None,
                        })
                        .collect();
                    if !approvals.is_empty() {
                        sections.push(("Approvals · audit".into(), approvals));
                    }
                    if let Some(c) = &m.return_contract {
                        sections.push(("Return contract".into(), json_rows(c)));
                    }
                    if let Some((rows, text)) = crate::ui::skills::details_section(&m.hidden_parts)
                    {
                        sections.push(("Relevant skills (injected)".into(), rows));
                        preview = Some(text);
                    }
                    for p in m.parts.iter().chain(m.hidden_parts.iter()) {
                        if let crate::model::Part::SubagentResult {
                            text,
                            status,
                            duration_ms,
                            ..
                        } = p
                        {
                            sections.push((
                                "Subagent result (raw)".into(),
                                vec![
                                    (
                                        "Status".into(),
                                        status.clone().unwrap_or_else(|| "—".into()),
                                    ),
                                    (
                                        "Duration".into(),
                                        duration_ms
                                            .map(|d| format!("{:.1} s", d as f64 / 1000.0))
                                            .unwrap_or_else(|| "—".into()),
                                    ),
                                ],
                            ));
                            let mut head: String = text.chars().take(4_000).collect();
                            if text.len() > head.len() {
                                head.push_str("\n…");
                            }
                            preview = Some(head);
                        }
                    }
                }
                if let Some(doc) = state.selected_doc() {
                    if !doc.threads.is_empty() {
                        sections.push((
                            "Threads · T1 index".into(),
                            doc.threads
                                .iter()
                                .take(6)
                                .map(|t| (format!("{} · {}", t.name, t.status), t.summary.clone()))
                                .collect(),
                        ));
                    }
                }
                "Turn details".to_string()
            }
            DetailsTarget::Blob(key) => {
                blob_key = Some(key.clone());
                let mut rows = vec![("Blob".into(), key.clone())];
                match state.blobs.get(key) {
                    Some(blob) => {
                        rows.push(("Type".into(), blob.mime.clone()));
                        rows.push(("Size".into(), format!("{} bytes", blob.bytes.len())));
                        if let Some(p) = &blob.saved_to {
                            rows.push(("Saved to".into(), p.display().to_string()));
                        }
                        if blob.mime.starts_with("text/")
                            || blob.mime.contains("json")
                            || std::str::from_utf8(&blob.bytes).is_ok()
                        {
                            let text = String::from_utf8_lossy(&blob.bytes);
                            let mut head: String = text.chars().take(4_000).collect();
                            if text.len() > head.len() {
                                head.push_str("\n…");
                            }
                            preview = Some(head);
                        } else {
                            preview = Some("(binary — use Save to download)".into());
                        }
                    }
                    None => {
                        rows.push((
                            "Full output".into(),
                            if asleep {
                                "needs the box awake; the relay mirror is tried first".into()
                            } else {
                                "fetching from GET /blob/…".into()
                            },
                        ));
                    }
                }
                sections.push(("Blob".into(), rows));
                "Details".to_string()
            }
        };

        let mut drawer = div()
            .w(px(DRAWER_WIDTH))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.card())
            .child(
                div()
                    .h(px(TAB_BAR_HEIGHT))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(12.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .text_size(px(12.5))
                    .text_color(theme.text)
                    .child(title)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .when_some(blob_key.clone(), |el, key| {
                                let key2 = key.clone();
                                el.child(
                                    div()
                                        .id("details-save")
                                        .text_size(px(12.0))
                                        .text_color(theme.accent)
                                        .cursor_pointer()
                                        .child("Save")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.state.update(cx, |s, cx| {
                                                s.fetch_blob(key2.clone(), true, cx)
                                            });
                                        })),
                                )
                            })
                            .child(
                                div()
                                    .id("details-close")
                                    .text_color(theme.text_faint)
                                    .cursor_pointer()
                                    .child("×")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state.update(cx, |s, cx| s.close_details(cx));
                                    })),
                            ),
                    ),
            );
        let mut body = div()
            .id("details-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .p(px(12.0));
        for (heading, rows) in sections {
            let mut sec = div().flex().flex_col().gap(px(8.0)).child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.text_faint)
                    .child(heading.to_uppercase()),
            );
            if rows.is_empty() {
                sec = sec.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.text_faint)
                        .child("—"),
                );
            }
            for (k, v) in rows {
                sec = sec.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.text_muted)
                                .child(k),
                        )
                        .child(div().text_size(px(12.5)).text_color(theme.text).child(v)),
                );
            }
            body = body.child(sec);
        }
        if let Some(text) = preview {
            body = body.child(
                div()
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .bg(theme.code_bg)
                    .border_1()
                    .border_color(theme.border)
                    .font_family(theme.font_mono)
                    .text_size(px(11.5))
                    .line_height(px(16.0))
                    .text_color(theme.text)
                    .child(text),
            );
        }
        drawer = drawer.child(body);
        frosted(0.0, PANEL_BLUR, drawer)
    }

    // ----- profile edit ---------------------------------------------------------------
    /// Settings header pencil: load the current name and handle into the fields.
    fn start_profile_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (name, handle) = {
            let state = self.state.read(cx);
            state
                .account
                .as_ref()
                .map(|a| (a.name.clone(), a.login.clone()))
                .unwrap_or_default()
        };
        self.name_input
            .update(cx, |input, cx| input.set_text(name, cx));
        self.handle_input
            .update(cx, |input, cx| input.set_text(handle, cx));
        self.profile_edit_open = true;
        let focus = self.name_input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn save_profile(&mut self, cx: &mut Context<Self>) {
        if !self.profile_edit_open {
            return;
        }
        let name = self.name_input.read(cx).text().trim().to_string();
        let handle = self
            .handle_input
            .read(cx)
            .text()
            .trim()
            .trim_start_matches('@')
            .to_string();
        if handle.is_empty() {
            self.state.update(cx, |s, cx| {
                s.push_notice("Choose a handle.");
                cx.notify();
            });
            return;
        }
        self.profile_edit_open = false;
        self.state
            .update(cx, |s, cx| s.update_profile(name, handle, cx));
        cx.notify();
    }

    // ----- settings sheet -----------------------------------------------------------

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let state = self.state.read(cx);
        let source = state.source_name;
        let link = state.link;
        let device = state.device_id.clone();
        let show_details = state.show_details;
        let account = state.account.clone();
        let account_image = account.as_ref().and_then(|a| state.avatar(&a.avatar_url));
        let notify_on = state.notifier.enabled;
        let skill_count = state.skills.len();
        let secrets_value = if state.secrets.loaded {
            state.secrets.row_value()
        } else {
            String::new()
        };
        let store_usage = state.store_usage;
        let update = state.update.clone();
        let update_failed = matches!(update.phase, crate::update::UpdatePhase::Failed { .. });
        let update_action: Option<(&'static str, &'static str, bool)> = match &update.phase {
            crate::update::UpdatePhase::Available(_)
            | crate::update::UpdatePhase::Failed {
                available: Some(_), ..
            } => Some(("update-install", "Install and relaunch", true)),
            crate::update::UpdatePhase::Idle
            | crate::update::UpdatePhase::UpToDate
            | crate::update::UpdatePhase::Failed {
                available: None, ..
            } => Some(("update-check", "Check now", false)),
            crate::update::UpdatePhase::Checking
            | crate::update::UpdatePhase::Downloading { .. }
            | crate::update::UpdatePhase::Installing(_)
            | crate::update::UpdatePhase::Restarting(_) => None,
        };
        let machine = state.machine.clone();
        let shown = machine.shown_state();
        if shown == MachineState::Waking {
            PulseClock::request(cx);
        }
        let glass_ok = Theme::glass_available();

        let value = |text: String, theme: &Theme| {
            div()
                .flex_shrink_0()
                .text_size(px(TEXT_BODY))
                .text_color(theme.text_muted)
                .child(text)
        };
        let link_button = |id: &'static str, label: &'static str, danger: bool, theme: &Theme| {
            div()
                .id(id)
                .flex_shrink_0()
                .text_size(px(TEXT_BODY))
                .text_color(if danger { theme.danger } else { theme.accent })
                .cursor_pointer()
                .hover(|s| s.opacity(0.8))
                .child(label)
        };
        let footnote = |text: String, theme: &Theme| {
            div()
                .px(px(16.0))
                .pt(px(6.0))
                .text_size(px(TEXT_TINY))
                .text_color(theme.text_muted)
                .child(text)
        };
        let section = |label: &str, group: Div, note: Option<Div>, theme: &Theme| {
            div()
                .flex()
                .flex_col()
                .child(glass::section_label(label, theme))
                .child(group)
                .children(note)
        };

        let mut body = div()
            .id("settings-body")
            .flex()
            .flex_col()
            .gap(px(22.0))
            .px(px(16.0))
            .pb(px(20.0))
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.settings_scroll);

        // Profile header (D80, Apple's account screen): big avatar, bold name, grey handle.
        let settings_group = |theme: &Theme| glass::group(theme).bg(theme.bubble_assistant);
        let mut sign_out: Option<Div> = None;
        if let Some(account) = account.clone() {
            let who = if account.login.is_empty() {
                "Signed in".to_string()
            } else {
                format!("@{}", account.login)
            };
            let person = crate::model::Person {
                id: account.id.clone(),
                handle: account.login.clone(),
                name: Some(account.name.clone()),
                avatar_url: account.avatar_url.clone(),
                status: String::new(),
                role: String::new(),
                last_seen_at: None,
                online: false,
                avatar_color_index: None,
            };
            let mut header = div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.0))
                .pt(px(6.0))
                .pb(px(2.0))
                .child(div().pb(px(8.0)).child(people::avatar(
                    account_image,
                    &person,
                    88.0,
                    &theme,
                )));
            if self.profile_edit_open {
                // iPhone parity: name and handle are editable (`PATCH /me`).
                let field = |input: Entity<ComposerInput>, theme: &Theme| {
                    div()
                        .w(px(260.0))
                        .px(px(10.0))
                        .py(px(7.0))
                        .rounded(px(10.0))
                        .bg(theme.field)
                        .border_1()
                        .border_color(theme.field_border)
                        .text_size(px(TEXT_BODY))
                        .child(input)
                };
                header = header
                    .child(field(self.name_input.clone(), &theme))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(TEXT_BODY))
                                    .text_color(theme.text_muted)
                                    .child("@"),
                            )
                            .child(field(self.handle_input.clone(), &theme)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .pt(px(6.0))
                            .child(
                                glass::quiet_button("profile-edit-cancel", "Cancel", &theme)
                                    .h(px(32.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.profile_edit_open = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                glass::primary_button("profile-edit-save", "Save", &theme)
                                    .h(px(32.0))
                                    .on_click(cx.listener(|this, _, _, cx| this.save_profile(cx))),
                            ),
                    );
            } else {
                header = header
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(22.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.text)
                                    .child(if account.name.trim().is_empty() {
                                        who.clone()
                                    } else {
                                        account.name.clone()
                                    }),
                            )
                            .child(
                                div()
                                    .id("profile-edit")
                                    .size(px(22.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.surface_2))
                                    .child(icon("square-pencil", 14.0, theme.text_muted))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.start_profile_edit(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_BODY))
                            .text_color(theme.text_muted)
                            .child(who.clone()),
                    );
            }
            body = body.child(header);
            if account.balance_usd > 0.0 || account.daily_cap_usd.is_some() {
                let mut group = settings_group(&theme).child(
                    glass::row("Credits", &theme)
                        .child(value(format!("${:.2} left", account.balance_usd), &theme)),
                );
                group = group.child(glass::row("Today", &theme).child(value(
                    match account.daily_cap_usd {
                        Some(cap) => {
                            format!("${:.2} of ${cap:.0} cap", account.spent_today_usd)
                        }
                        None => format!("${:.2}", account.spent_today_usd),
                    },
                    &theme,
                )));
                // iPhone parity: during the beta a top-up is a note to the relay.
                group = group.child(glass::row("", &theme).border_b_0().child(
                    link_button("credits-top-up", "Request a top-up", false, &theme).on_click(
                        cx.listener(|this, _, window, cx| this.request_top_up(window, cx)),
                    ),
                ));
                body = body.child(section(
                    "Account",
                    group,
                    Some(footnote(
                        "Credits pay for your agents' model use. Machine time is billed separately. During the beta, usage credit is included — request a top-up when you run low.".into(),
                        &theme,
                    )),
                    &theme,
                ));
            }
            sign_out = Some(
                settings_group(&theme).child(
                    div()
                        .id("sign-out")
                        .flex()
                        .items_center()
                        .justify_center()
                        .min_h(px(44.0))
                        .text_size(px(TEXT_BODY))
                        .text_color(theme.danger)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.surface_2))
                        .child("Sign out")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings_open = false;
                            this.state.update(cx, |s, cx| s.sign_out(cx));
                        })),
                ),
            );
        }

        // Machine.
        let dot = sidebar::machine_dot(shown, &theme);
        let uptime = machine
            .since
            .filter(|_| shown == MachineState::Awake)
            .map(|since| fmt_uptime(now_ms().saturating_sub(since)))
            .unwrap_or_else(|| "—".into());
        let mut group = settings_group(&theme)
            .child(
                glass::row("State", &theme).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .child(div().size(px(8.0)).rounded_full().bg(Theme::with_alpha(
                            dot,
                            if shown == MachineState::Waking {
                                0.4 + 0.6 * PulseClock::phase(cx, 1100)
                            } else {
                                1.0
                            },
                        )))
                        .child(value(shown.label().to_string(), &theme)),
                ),
            )
            .child(glass::row("Since", &theme).child(value(uptime, &theme)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .px(px(16.0))
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_size(px(TEXT_BODY))
                            .text_color(theme.text)
                            .child("Spec"),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_SMALL))
                            .text_color(theme.text_muted)
                            .child(format!(
                                "{}{}",
                                machine
                                    .spec
                                    .clone()
                                    .unwrap_or_else(|| "spec unknown".into()),
                                machine
                                    .class
                                    .as_deref()
                                    .map(|c| format!(" · {c}"))
                                    .unwrap_or_default()
                            )),
                    ),
            )
            .child(
                glass::row(
                    if machine.always_on {
                        "Machine time today"
                    } else {
                        "Spend today"
                    },
                    &theme,
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_end()
                        .child(value(
                            if machine.always_on {
                                format!("${:.2}", machine.spend_today_usd)
                            } else {
                                format!(
                                    "${:.2} of ${:.0}",
                                    machine.spend_today_usd, machine.spend_cap_usd
                                )
                            },
                            &theme,
                        ))
                        .when(machine.always_on, |el| {
                            el.child(
                                div()
                                    .text_size(px(TEXT_TINY))
                                    .text_color(theme.text_muted)
                                    .child("Not taken from credits"),
                            )
                        }),
                ),
            );
        if let Some(detail) = machine.detail.clone() {
            group = group.child(footnote(detail, &theme).pb(px(8.0)));
        }
        if machine.always_on {
            group = group.child(glass::row("", &theme).border_b_0().child(
                link_button("machine-restart", "Restart machine", false, &theme).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.machine_restart(cx));
                    }),
                ),
            ));
        } else if machine.state.is_down() {
            group = group.child(glass::row("", &theme).border_b_0().child(
                link_button("machine-start", "Start machine", false, &theme).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.machine_start(cx));
                    },
                )),
            ));
        } else {
            group = group.child(glass::row("", &theme).border_b_0().child(
                link_button("machine-stop", "Stop machine", true, &theme).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.machine_stop(cx));
                    },
                )),
            ));
        }
        body = body.child(section(
            "Machine",
            group,
            Some(footnote(
                "Your one cloud computer, always on. Agents are workspaces on it. Machine time \
                 is billed separately from your credits."
                    .into(),
                &theme,
            )),
            &theme,
        ));

        // Appearance.
        let seg_row = |label: &str, segs: Div, theme: &Theme| {
            glass::row(label.to_string(), theme).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .p(px(2.0))
                    .rounded_full()
                    .bg(theme.field)
                    .border_1()
                    .border_color(theme.field_border)
                    .child(segs),
            )
        };
        let group = settings_group(&theme)
            .child(seg_row(
                "Theme",
                div().flex().gap(px(2.0)).children(
                    [
                        (AppearancePref::System, "mode-system"),
                        (AppearancePref::Light, "mode-light"),
                        (AppearancePref::Dark, "mode-dark"),
                    ]
                    .into_iter()
                    .map(|(pref, id)| {
                        glass::segment(id, pref.label(), theme.pref == pref, &theme).on_click(
                            cx.listener(move |this, _, _, cx| {
                                let s = Theme::of(cx).surface;
                                this.set_theme(pref, s, cx);
                            }),
                        )
                    }),
                ),
                &theme,
            ))
            .child(
                seg_row(
                    "Surface",
                    div()
                        .flex()
                        .gap(px(2.0))
                        .child(
                            glass::segment(
                                "surface-matte",
                                "Matte",
                                theme.surface == Surface::Matte,
                                &theme,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                let a = Theme::of(cx).pref;
                                this.set_theme(a, Surface::Matte, cx);
                            })),
                        )
                        .child(
                            glass::segment(
                                "surface-glass",
                                "Glass",
                                theme.surface == Surface::Glass,
                                &theme,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                let a = Theme::of(cx).pref;
                                this.set_theme(a, Surface::Glass, cx);
                            })),
                        ),
                    &theme,
                )
                .border_b_0(),
            );
        body = body.child(section(
            "Appearance",
            group,
            Some(footnote(
                if glass_ok {
                    "Glass: window vibrancy and frosted panels.".into()
                } else {
                    "Glass here: frosted panels only; window vibrancy needs macOS.".into()
                },
                &theme,
            )),
            &theme,
        ));

        // Notifications.
        let group = settings_group(&theme).child(
            seg_row(
                "Notifications",
                div()
                    .flex()
                    .gap(px(2.0))
                    .child(
                        glass::segment("notify-on", "On", notify_on, &theme).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.state
                                    .update(cx, |s, cx| s.set_notifications_enabled(true, cx));
                            },
                        )),
                    )
                    .child(
                        glass::segment("notify-off", "Off", !notify_on, &theme).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.state
                                    .update(cx, |s, cx| s.set_notifications_enabled(false, cx));
                            }),
                        ),
                    ),
                &theme,
            )
            .border_b_0(),
        );
        body = body.child(section(
            "Notifications",
            group,
            Some(footnote(
                "A banner with the sender's picture when an agent finishes or a person writes, \
                 only while you are not looking."
                    .into(),
                &theme,
            )),
            &theme,
        ));

        // Updates — status, Check now / Install, and Download when self-update failed.
        let mut actions = div().flex().items_center().gap(px(14.0));
        if let Some((id, label, installs)) = update_action {
            actions = actions.child(link_button(id, label, false, &theme).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.state.update(cx, |s, cx| {
                        if installs {
                            s.install_update(cx);
                        } else {
                            s.check_for_updates(cx);
                        }
                    });
                },
            )));
        }
        if update_failed {
            // Self-update could not finish here (read-only copy, unwritable
            // folder…): the newest build is one click away in the browser.
            actions = actions.child(
                link_button("settings-update-download", "Download", false, &theme).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.open_download_page(cx));
                    }),
                ),
            );
        }
        let group = settings_group(&theme).child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .min_h(px(44.0))
                .px(px(16.0))
                .py(px(10.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(TEXT_BODY))
                        .text_color(if update_failed {
                            theme.danger
                        } else {
                            theme.text
                        })
                        .child(update.status_line()),
                )
                .child(actions),
        );
        body = body.child(section("Updates", group, None, &theme));

        // More: the feature panels.
        let group = settings_group(&theme)
            .child(
                glass::nav_row_icon("settings-connectors", "link", "Connectors", None, &theme)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings_open = false;
                        this.connectors.update(cx, |p, cx| p.show(cx));
                        cx.notify();
                    })),
            )
            .child(
                glass::nav_row_icon("settings-routines", "sparkles", "Routines", None, &theme)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings_open = false;
                        this.routines_open = true;
                        this.routines.update(cx, |p, cx| p.open(cx));
                        cx.notify();
                    })),
            )
            .child(
                glass::nav_row_icon("settings-channels", "globe", "Channels", None, &theme)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings_open = false;
                        this.channels.update(cx, |p, cx| p.show(cx));
                        cx.notify();
                    })),
            )
            .child(
                glass::nav_row_icon(
                    "settings-skills",
                    "cpu",
                    "Skills",
                    Some(format!("{skill_count} in library")),
                    &theme,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.settings_open = false;
                    this.skills.update(cx, |p, cx| p.open(window, cx));
                    cx.notify();
                })),
            )
            .child(
                glass::nav_row_icon("settings-local", "window", "This Mac", None, &theme).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.settings_open = false;
                        this.local.update(cx, |p, cx| p.show(cx));
                        cx.notify();
                    }),
                ),
            )
            .child(
                glass::nav_row_icon(
                    "settings-secrets",
                    "key",
                    "Secrets",
                    Some(secrets_value),
                    &theme,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.settings_open = false;
                    this.secrets.update(cx, |p, cx| p.show(cx));
                    cx.notify();
                })),
            )
            .child(
                glass::nav_row_icon(
                    "settings-feedback",
                    "square-pencil",
                    "Send feedback",
                    Some("⌘⇧F".into()),
                    &theme,
                )
                .border_b_0()
                .on_click(cx.listener(|this, _, window, cx| this.open_feedback(window, cx))),
            );
        body = body.child(section("Apps & tools", group, None, &theme));

        // Debug + about.
        let group = settings_group(&theme)
            .child(seg_row(
                "Turn details",
                div()
                    .flex()
                    .gap(px(2.0))
                    .child(
                        glass::segment("details-off", "Off", !show_details, &theme).on_click(
                            cx.listener(|this, _, _, cx| {
                                if this.state.read(cx).show_details {
                                    this.state.update(cx, |s, cx| s.toggle_show_details(cx));
                                }
                            }),
                        ),
                    )
                    .child(
                        glass::segment("details-on", "On", show_details, &theme).on_click(
                            cx.listener(|this, _, _, cx| {
                                if !this.state.read(cx).show_details {
                                    this.state.update(cx, |s, cx| s.toggle_show_details(cx));
                                }
                            }),
                        ),
                    ),
                &theme,
            ))
            .child(
                glass::row("Version", &theme)
                    .child(value(update.current_version.to_string(), &theme)),
            )
            .child(glass::row("On this Mac", &theme).child(value(
                if store_usage.1 == 0 {
                    "Nothing saved yet".to_string()
                } else {
                    format!(
                        "{} · {} chat{}",
                        human_bytes(store_usage.0),
                        store_usage.1,
                        if store_usage.1 == 1 { "" } else { "s" }
                    )
                },
                &theme,
            )))
            .child(glass::row("Relay", &theme).border_b_0().child(value(
                format!(
                    "{source} · {}{}",
                    format!("{link:?}").to_lowercase(),
                    device.map(|d| format!(" · {d}")).unwrap_or_default()
                ),
                &theme,
            )));
        body = body.child(section(
            "Help & about",
            group,
            Some(footnote(
                "Turn details adds a \"Show details\" line under each reply with the model, \
                 tools and tokens."
                    .into(),
                &theme,
            )),
            &theme,
        ));
        if let Some(sign_out) = sign_out {
            body = body.child(sign_out);
        }

        let header = glass::sheet_header(
            "Settings",
            None,
            Some(
                glass::sheet_button("settings-done", "Done", &theme).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.settings_open = false;
                        Composer::focus(&this.composer, window, cx);
                        cx.notify();
                    },
                )),
            ),
            &theme,
        );
        glass::sheet(480.0, &theme, header, body.max_h(px(720.0)))
    }

    // ----- sign-in -------------------------------------------------------------------

    /// First launch: the mark, one sentence, "Sign in with GitHub". The
    /// pairing-code entry only appears when the relay runs in paircode mode.
    fn render_pairing(&self, pairing: &crate::app::Pairing, cx: &mut Context<Self>) -> Div {
        let theme = Theme::of(cx).clone();
        // Grok Mac sign-in (ref 60): the mark inline with a large wordmark, a
        // two-line tagline in the text colour, one black pill.
        let mark_image = self.state.read(cx).mark.clone();
        let mark = div()
            .size(px(56.0))
            .flex()
            .items_center()
            .justify_center()
            .map(|el| match mark_image {
                Some(image) => el.child(
                    gpui::img(image)
                        .size(px(56.0))
                        .object_fit(gpui::ObjectFit::Contain),
                ),
                None => el
                    .rounded(px(14.0))
                    .bg(theme.text)
                    .text_size(px(26.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.bg)
                    .child("Y"),
            });
        let wordmark = div().flex().items_center().gap(px(14.0)).child(mark).child(
            div()
                .text_size(px(44.0))
                .line_height(px(52.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child("Arbos"),
        );
        let screen = div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(14.0))
            .px(px(28.0))
            .window_drag()
            .child(wordmark)
            .child(
                div()
                    .pt(px(18.0))
                    .text_size(px(TEXT_BODY + 2.0))
                    .line_height(px(LINE_HEIGHT + 3.0))
                    .text_color(theme.text)
                    .text_center()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child("Your team of always-on agents")
                    .child("that finish the work."),
            );
        match pairing.mode {
            SignInMode::GitHub => {
                let busy = pairing.busy;
                let button = glass::primary_button(
                    "sign-in-github",
                    if busy {
                        "Waiting for GitHub…"
                    } else {
                        "Sign in with GitHub"
                    },
                    &theme,
                )
                .mt(px(18.0))
                .h(px(40.0))
                .px(px(22.0))
                .text_size(px(TEXT_BODY))
                .when(busy, |el| el.opacity(0.7))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.sign_in(cx));
                }));
                screen
                    .child(button)
                    .when_some(pairing.error.clone(), |el, err| {
                        el.child(
                            div()
                                .max_w(px(420.0))
                                .text_center()
                                .text_size(px(TEXT_SMALL))
                                .text_color(theme.danger)
                                .child(err),
                        )
                    })
            }
            SignInMode::PairCode => {
                let input_row = div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .h(px(44.0))
                    .pl(px(16.0))
                    .pr(px(5.0))
                    .rounded_full()
                    .bg(theme.field)
                    .border_1()
                    .border_color(if pairing.error.is_some() {
                        theme.danger
                    } else {
                        theme.field_border
                    })
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(self.pairing_input.clone())
                    .child(
                        glass::primary_button(
                            "pair-submit",
                            if pairing.busy { "Pairing…" } else { "Pair" },
                            &theme,
                        )
                        .h(px(34.0))
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, _, cx| this.submit_pairing(cx))),
                    );
                screen
                    .child(
                        div()
                            .text_size(px(TEXT_TINY))
                            .text_color(theme.text_faint)
                            .text_center()
                            .child(format!(
                                "{} uses pairing codes. Enter the one-time code from its log (or `arbosctl pair-code`).",
                                pairing.relay_url
                            )),
                    )
                    .child(
                        div()
                            .w(px(420.0))
                            .max_w_full()
                            .pt(px(4.0))
                            .child(frosted(22.0, PANEL_BLUR, input_row)),
                    )
                    .when_some(pairing.error.clone(), |el, err| {
                        el.child(
                            div()
                                .text_size(px(TEXT_SMALL))
                                .text_color(theme.danger)
                                .child(err),
                        )
                    })
            }
        }
    }

    /// One-time explainer before macOS asks for notification permission: a
    /// Grok-style card in the top-right corner of the conversation.
    fn render_notify_prompt(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.state.read(cx).notify_prompt {
            return None;
        }
        let theme = Theme::of(cx).clone();
        let card = div()
            .w(px(360.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(16.0))
            .rounded(px(PANEL_RADIUS + 6.0))
            .bg(theme.card())
            .border_1()
            .border_color(theme.border_strong)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        glass::circle("notify-glyph", 40.0, &theme)
                            .child(icon("bell", 17.0, theme.text)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(TEXT_BODY))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .child("Get told when someone writes"),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_SMALL))
                                    .text_color(theme.text_muted)
                                    .child("Banners with the sender's picture, even in the back."),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(TEXT_TINY))
                    .text_color(theme.text_muted)
                    .child("macOS will ask you to allow it next."),
            )
            .child(
                div()
                    .flex()
                    .gap(px(10.0))
                    .child(
                        glass::quiet_button("notify-later", "Not now", &theme)
                            .flex_1()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |s, cx| s.dismiss_notify_prompt(cx));
                            })),
                    )
                    .child(
                        glass::primary_button("notify-allow", "Allow notifications", &theme)
                            .flex_1()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |s, cx| s.allow_notifications(cx));
                            })),
                    ),
            );
        Some(
            div()
                .absolute()
                .right(px(16.0))
                .top(px(HEADER_HEIGHT + 8.0))
                .child(frosted(PANEL_RADIUS + 6.0, MENU_BLUR, card)),
        )
    }

    fn render_notices(&self, cx: &mut Context<Self>) -> Option<Div> {
        let theme = Theme::of(cx).clone();
        let notices = self.state.read(cx).live_notices();
        if notices.is_empty() {
            return None;
        }
        PulseClock::request(cx);
        // Top right, under the header: never over the composer's send / mic
        // button or an error banner above it (the busy end of the column).
        let mut stack = div()
            .absolute()
            .right(px(16.0))
            .top(px(HEADER_HEIGHT + 12.0))
            .flex()
            .flex_col()
            .gap(px(6.0));
        for n in notices {
            stack = stack.child(
                div()
                    .max_w(px(380.0))
                    .px(px(14.0))
                    .py(px(10.0))
                    .rounded(px(PANEL_RADIUS))
                    .bg(theme.surface_2)
                    .border_1()
                    .border_color(theme.border_strong)
                    .shadow_md()
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text)
                    .child(n.text),
            );
        }
        Some(stack)
    }
}

/// Flatten a JSON object one level ("cost.total") for the drawer.
fn json_rows(v: &serde_json::Value) -> Vec<(String, String)> {
    fn scalar(v: &serde_json::Value) -> String {
        match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => "—".into(),
            serde_json::Value::Number(n) => match n.as_f64() {
                Some(f) if f.fract() != 0.0 => format!("{f:.6}").trim_end_matches('0').to_string(),
                _ => n.to_string(),
            },
            other => other.to_string(),
        }
    }
    match v {
        serde_json::Value::Object(map) => {
            let mut rows = Vec::new();
            for (k, v) in map {
                match v {
                    serde_json::Value::Object(inner) => {
                        for (ik, iv) in inner {
                            rows.push((format!("{k}.{ik}"), scalar(iv)));
                        }
                    }
                    other => rows.push((k.clone(), scalar(other))),
                }
            }
            rows
        }
        serde_json::Value::Null => Vec::new(),
        other => vec![("value".into(), scalar(other))],
    }
}

fn fmt_uptime(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn cards_spinner(theme: &Theme, cx: &App) -> Div {
    crate::ui::cards::spinner(theme, cx)
}

impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let width: f32 = window.viewport_size().width.into();
        let state = self.state.read(cx);
        let sidebar_hidden = state.rail_collapsed || state.window_is_narrow(width);
        let sidebar_overlay = sidebar_hidden && state.sidebar_overlay;
        let details = state.details_open.clone();
        let pairing = state.pairing.clone();
        let new_chat_open = state.new_chat_open;
        let people_space = state.selected().filter(|p| p.is_people()).cloned();
        let project_space = state.selected().filter(|p| !p.is_people()).cloned();
        let selected = state.selected().cloned();

        let mut root = div()
            .id("shell-root")
            .key_context("Shell")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::toggle_rail))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::send_feedback))
            // Clicks on chrome must not strand keyboard focus: unless another
            // text field took it, the composer keeps it. Floating menus close.
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.close_popovers(cx);
                    if this.feedback.read(cx).is_open() {
                        return;
                    }
                    let pairing = this.pairing_input.read(cx).focus_handle(cx);
                    let project = this.project_input.read(cx).focus_handle(cx);
                    let search = this.search_input.read(cx).focus_handle(cx);
                    let shown = this.state.read(cx).new_project_open;
                    let people = this.people_input.read(cx).focus_handle(cx);
                    let rename = this.agent_rename_input.read(cx).focus_handle(cx);
                    let model_q = this.model_search_input.read(cx).focus_handle(cx);
                    if pairing.is_focused(window)
                        || (shown && project.is_focused(window))
                        || search.is_focused(window)
                        || (this.state.read(cx).new_chat_open && people.is_focused(window))
                        || this.connectors.read(cx).input_focused(window, cx)
                        || this.skills.read(cx).has_focus(window, cx)
                        || this.secrets.read(cx).input_focused(window, cx)
                        || (this.renaming_agent && rename.is_focused(window))
                        || (this.menu_submenu == Some(MenuSubmenu::Model)
                            && model_q.is_focused(window))
                    {
                        return;
                    }
                    if this.routines_open && this.routines.read(cx).has_focus(window, cx) {
                        return;
                    }
                    if this.profile_edit_open
                        && (this.name_input.read(cx).focus_handle(cx).is_focused(window)
                            || this
                                .handle_input
                                .read(cx)
                                .focus_handle(cx)
                                .is_focused(window))
                    {
                        return;
                    }
                    if this.renaming_agent {
                        this.commit_agent_rename(window, cx);
                        return;
                    }
                    if this.state.read(cx).pairing.is_none() {
                        Composer::focus(&this.composer, window, cx);
                    }
                }),
            )
            .relative()
            .flex()
            .flex_row()
            .size_full()
            .bg(theme.shell())
            .text_color(theme.text)
            .font_family(theme.font_sans)
            .text_size(px(TEXT_BODY));

        if let Some(pairing) = pairing {
            self.signing_in = true;
            return root
                .child(self.render_pairing(&pairing, cx))
                .into_any_element();
        }
        if self.signing_in {
            // Landed after sign-in: the composer takes the keyboard.
            self.signing_in = false;
            Composer::focus(&self.composer, window, cx);
        }

        if !sidebar_hidden {
            root = root.child(frosted(0.0, PANEL_BLUR, sidebar::render(self, cx)));
            if self.state.read(cx).sidebar_overlay {
                // Widened back past the band with the overlay up: it has docked.
                self.state.update(cx, |s, _| s.sidebar_overlay = false);
            }
        }
        root = root.child(self.render_center(sidebar_hidden, cx));
        if sidebar_overlay {
            root = root.child(self.render_sidebar_overlay(&theme, cx));
        }
        if let Some(target) = details {
            root = root.child(self.render_details(&target, cx));
        }
        if self.menu_open {
            if let Some(space) = selected.as_ref() {
                root = root.child(glass::popover(header::menu(self, space, cx)));
            }
        }
        if self.archived_open {
            if let Some(space) = selected.as_ref() {
                root = root.child(glass::popover(header::archived_list(self, space, cx)));
            }
        }
        if let Some(menu) = sidebar::row_menu(self, cx) {
            root = root.child(menu);
        }
        if let Some(menu) = self.react_menu(cx) {
            root = root.child(menu);
        }
        if self.group_picker_open {
            root = root.child(glass::popover(groups::picker(
                &self.state,
                TAB_BAR_HEIGHT - 4.0,
                16.0,
                cx,
            )));
        }
        if let Some(space) = people_space {
            let right = 16.0;
            if self.add_agent_open {
                root = root.child(glass::popover(people::add_agent_picker(
                    &space,
                    &self.state,
                    right,
                    cx,
                )));
            }
            if self.members_open {
                root = root.child(glass::popover(people::members_popover(
                    &space,
                    &self.state,
                    right,
                    cx,
                )));
            }
        } else if let Some(project) = project_space {
            if self.members_open {
                root = root.child(glass::popover(people::project_members_popover(
                    &project,
                    &self.state,
                    16.0,
                    cx,
                )));
            }
        }
        if let Some(page) = self.state.read(cx).profile.clone() {
            if let Some(sheet) = profiles::sheet(&page, self, cx) {
                root = root.child(sheet);
            }
        }
        if self.connectors.read(cx).is_open() {
            root = root.child(self.connectors.clone());
        }
        if self.local.read(cx).is_open() {
            root = root.child(self.local.clone());
        }
        if self.secrets.read(cx).is_open() {
            root = root.child(self.secrets.clone());
        }
        if self.routines_open {
            root = root.child(self.routines.clone());
        }
        if self.channels.read(cx).is_open() {
            root = root.child(self.channels.clone());
        }
        if self.skills.read(cx).open {
            root = root.child(self.skills.clone());
        }
        if self.settings_open {
            root = root.child(self.render_settings(cx));
        }
        if self.voice_picker_open {
            if let Some(space) = selected.as_ref().filter(|s| !s.is_people()) {
                root = root.child(voicepicker::sheet(self, space, cx));
            }
        }
        if new_chat_open {
            root = root.child(people::new_chat_sheet(
                self,
                &self.state,
                &self.people_input,
                cx,
            ));
        }
        if self.feedback.read(cx).is_open() {
            root = root.child(self.feedback.clone());
        }
        // Collapsed video call: the camera keeps showing in a draggable tile.
        let collapsed_video = self
            .state
            .read(cx)
            .my_call
            .clone()
            .filter(|c| !c.expanded && c.call.kind == CallKind::Video);
        if let Some(active) = collapsed_video.as_ref() {
            root = root.child(callview::pip(self, active, cx));
        }
        if self.pip_drag.is_some() {
            root = root
                .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, window, cx| {
                    let Some((start, (r0, b0), moved)) = this.pip_drag else {
                        return;
                    };
                    let dx = f32::from(ev.position.x - start.x);
                    let dy = f32::from(ev.position.y - start.y);
                    let moved = moved || dx.abs() > 4.0 || dy.abs() > 4.0;
                    let size = window.viewport_size();
                    let max_r = f32::from(size.width) - callview::PIP_W - 8.0;
                    let max_b = f32::from(size.height) - callview::PIP_H - 8.0;
                    let r = (r0 - dx).clamp(8.0, max_r.max(8.0));
                    let b = (b0 - dy).clamp(8.0, max_b.max(8.0));
                    this.pip_offset = (r, b);
                    this.pip_drag = Some((start, (r0, b0), moved));
                    cx.notify();
                }))
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        if let Some((_, _, moved)) = this.pip_drag.take() {
                            if !moved {
                                // A plain click: back to the full call screen.
                                this.state.update(cx, |s, cx| s.toggle_call_expanded(cx));
                            }
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_up_out(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.pip_drag = None;
                        cx.notify();
                    }),
                );
        }
        let incoming = self.state.read(cx).incoming_call.clone();
        if let Some((call, sender)) = incoming.as_ref() {
            root = root.child(callview::ring_card(self, call, sender, cx));
        }
        if let Some(prompt) = self.render_notify_prompt(cx) {
            root = root.child(prompt);
        }
        if let Some(notices) = self.render_notices(cx) {
            root = root.child(notices);
        }
        // D82: first sign-in — the terminal screen covers the shell until the
        // computer, agent and first chat exist, then morphs into the greeting.
        if let Some(onboarding) = self.state.read(cx).onboarding.clone() {
            root = root.child(onboarding::render(
                self,
                &self.state,
                &onboarding,
                window,
                cx,
            ));
        }
        root.into_any_element()
    }
}

/// "12.4 MB" for the on-device store line.
fn human_bytes(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    if b < 1_000.0 {
        format!("{bytes} B")
    } else if b < 1_000_000.0 {
        format!("{:.0} KB", b / 1_000.0)
    } else if b < 1_000_000_000.0 {
        format!("{:.1} MB", b / 1_000_000.0)
    } else {
        format!("{:.2} GB", b / 1_000_000_000.0)
    }
}
