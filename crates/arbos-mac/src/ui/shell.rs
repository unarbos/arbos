//! Root view, Grok Bot layout: chat-list sidebar (the iPhone home list) ·
//! one conversation pane (floating glass header, bubbles, the composer) ·
//! and the debug Details drawer. Sheets (Settings, New Message, agent profile)
//! float centred over everything.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    actions, div, prelude::*, px, Animation, AnimationExt, AnyElement, App, Context, Div, Entity,
    FocusHandle, Focusable, FontWeight, Render, ScrollHandle, SharedString, Stateful, Window,
};

use crate::app::SignInMode;
use crate::app::{AppState, DetailsTarget, ProfilePage, ServerSheet};
use crate::image_picker;
use crate::model::{now_ms, CallKind, MachineState};
use crate::sign_in;
use crate::sync::PictureSource;
use crate::ui::avatar_picker;
use crate::ui::avatars::{self, leaf, LeafPick};
use crate::ui::callview;
use crate::ui::channels::{ChannelsEvent, ChannelsPanel};
use crate::ui::composer::{self, Composer, ComposerEvent, ComposerInput};
use crate::ui::connectors::{ConnectorsEvent, ConnectorsPanel};
use crate::ui::create_entity::{self, Kind as CreateKind};
use crate::ui::delete_confirm;
use crate::ui::dropzone;
use crate::ui::feedback::{FeedbackSheet, SendFeedback};
use crate::ui::frost::{frosted, MENU_BLUR, PANEL_BLUR};
use crate::ui::glass::{self, WindowDrag};
use crate::ui::header;
use crate::ui::icons::icon;
use crate::ui::local::{LocalEvent, LocalPanel};
use crate::ui::media_viewer;
use crate::ui::member_menu;
use crate::ui::message_menu;
use crate::ui::motion::{ease_out_cubic, PulseClock, COMPOSER_DROP_MS, SHAKE_MS};
use crate::ui::onboarding;
use crate::ui::people;
use crate::ui::profiles;
use crate::ui::reactions;
use crate::ui::routines::{RoutinesEvent, RoutinesPanel};
use crate::ui::secrets::{SecretsEvent, SecretsPanel};
use crate::ui::servers;
use crate::ui::sidebar;
use crate::ui::skills::{self, SkillsPanel};
use crate::ui::theme::{
    reapply_window_background, AppearancePref, GlassOpacity, Surface, Theme, COLUMN_MARGIN,
    COMPOSER_BOTTOM, COMPOSER_WIDTH, DRAWER_WIDTH, GLASS_OPACITY_MAX, GLASS_OPACITY_MIN,
    HEADER_HEIGHT, LINE_HEIGHT, PANEL_RADIUS, TAB_BAR_HEIGHT, TEXT_BODY, TEXT_SMALL, TEXT_TINY,
};
use crate::ui::transcript::{column_row, Transcript, TranscriptEvent};
use crate::ui::voice::Voice;
use crate::ui::voicepicker;

actions!(
    shell,
    [
        CloseTab,
        NewTab,
        PrevTab,
        NextTab,
        ToggleRail,
        FocusSearch,
        Escape,
        Quit
    ]
);

/// Left inset for the conversation header while the sidebar is hidden: the window's traffic lights.
/// The expand control (when the rail is folded) sits at this inset beside the lights.
pub(crate) const TRAFFIC_LIGHTS_INSET: f32 = 72.0;

/// What the shared name field (`agent_name_input`) renames when Return is pressed.
#[derive(Clone, Copy)]
enum RenameKind {
    Agent,
    Group,
    Server,
}

/// How long a single click on the header name waits for a second one before
/// it opens the profile (#1068). Shorter than the system double-click
/// interval (500 ms by default) so the page does not feel late; a slower
/// double-click still renames — its second click lands on the page's
/// backdrop, which hands it to the rename (`rename_from_backdrop`).
const NAME_CLICK_DELAY: Duration = Duration::from_millis(250);

/// Width of my pane's Settings pages that fit the pane; the two wider panels
/// (`sheet_width`) grow the pane for their page.
const SETTINGS_WIDTH: f32 = crate::ui::profiles::SHEET_WIDTH;

/// What my pane's Settings tab shows (#1086, D80): the card list, or a pushed
/// page inside the pane with a back button — the iPhone's `SettingsPage`
/// navigation. Credits / Machine / Appearance /
/// Notifications / Updates / About are the former inline sections; the rest
/// are the Apps & tools panels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SettingsPage {
    #[default]
    Root,
    /// Hidden from Settings until credits UI ships again.
    #[allow(dead_code)]
    Credits,
    Machine,
    Appearance,
    Notifications,
    Updates,
    About,
    Connectors,
    /// Hidden from Settings until routines UI ships again.
    #[allow(dead_code)]
    Routines,
    /// Hidden from Settings until channels UI ships again.
    #[allow(dead_code)]
    Channels,
    /// Hidden from Settings until skills UI ships again.
    #[allow(dead_code)]
    Skills,
    ThisMac,
    Secrets,
    /// Signed-in devices of my account: list + sign out another (Oct 23).
    Devices,
}

impl SettingsPage {
    pub(crate) fn title(self) -> &'static str {
        match self {
            SettingsPage::Root => "Settings",
            SettingsPage::Credits => "Credits",
            SettingsPage::Machine => "Machine",
            SettingsPage::Appearance => "Appearance",
            SettingsPage::Notifications => "Notifications",
            SettingsPage::Updates => "Updates",
            SettingsPage::About => "About",
            SettingsPage::Connectors => "Connected apps",
            SettingsPage::Routines => "Routines",
            SettingsPage::Channels => "Channels",
            SettingsPage::Skills => "Skills",
            SettingsPage::ThisMac => "This Mac",
            SettingsPage::Secrets => "Secrets",
            SettingsPage::Devices => "Devices",
        }
    }

    /// True for pages drawn as content inside the sheet (not a panel entity).
    pub(crate) fn is_content(self) -> bool {
        matches!(
            self,
            SettingsPage::Credits
                | SettingsPage::Machine
                | SettingsPage::Appearance
                | SettingsPage::Notifications
                | SettingsPage::Updates
                | SettingsPage::About
                | SettingsPage::Devices
        )
    }

    /// The pane grows for the two pages laid out wider than the card list.
    pub(crate) fn sheet_width(self) -> f32 {
        match self {
            SettingsPage::Root
            | SettingsPage::Credits
            | SettingsPage::Machine
            | SettingsPage::Appearance
            | SettingsPage::Notifications
            | SettingsPage::Updates
            | SettingsPage::About
            | SettingsPage::Connectors
            | SettingsPage::Channels
            | SettingsPage::ThisMac
            | SettingsPage::Secrets
            | SettingsPage::Devices => SETTINGS_WIDTH,
            SettingsPage::Routines => crate::ui::routines::PANEL_WIDTH + 32.0,
            SettingsPage::Skills => skills::CARD_WIDTH + 32.0,
        }
    }
}

pub struct Shell {
    pub(crate) state: Entity<AppState>,
    pub(crate) transcript: Entity<Transcript>,
    pub(crate) composer: Entity<Composer>,
    /// F11: call strip + mic/TTS owner.
    pub(crate) voice: Entity<Voice>,
    pairing_input: Entity<ComposerInput>,
    pub(crate) project_input: Entity<ComposerInput>,
    /// D79: the swatch picked for the agent being created; `None` = automatic.
    pub(crate) new_agent_color: Option<u8>,
    /// D93 Create screen: the leaf and colour clicked; `None` = the sheet's default pick.
    pub(crate) create_leaf: Option<u8>,
    pub(crate) create_leaf_color: Option<u8>,
    /// Create group, step 1 (picture + name) showing over the New Message backdrop.
    pub(crate) create_group_open: bool,
    /// The avatar picker sheet (D92 amendment) while it is open: whose picture, which tab,
    /// the pending choice.
    pub(crate) avatar_picker: Option<avatar_picker::Picker>,
    /// The Generate tab's prompt.
    pub(crate) avatar_prompt_input: Entity<ComposerInput>,
    /// Sidebar search field.
    pub(crate) search_input: Entity<ComposerInput>,
    /// The media viewer's Forward picker search field (feedback media-viewer-2026-10-23).
    pub(crate) viewer_search: Entity<ComposerInput>,
    /// Keyboard focus while the media viewer is up (← → page, Esc closes).
    viewer_focus: FocusHandle,
    /// The bubble whose "Forward to" picker last took the search field's focus
    /// (`AppState::forward_message`), so it is cleared and focused once per opening.
    forward_message_focused_for: Option<crate::model::Id>,
    /// Keyboard owner of the "Delete message?" confirm (Return / Esc).
    delete_confirm_focus: FocusHandle,
    /// The message whose confirm last took that focus, so it is focused once per opening.
    delete_confirm_focused_for: Option<crate::model::Id>,
    /// `AppState::media_viewer_generation` the viewer was last focused for.
    viewer_focused_generation: u64,
    pub(crate) connectors: Entity<ConnectorsPanel>,
    pub(crate) skills: Entity<SkillsPanel>,
    pub(crate) local: Entity<LocalPanel>,
    focus_handle: FocusHandle,
    /// Which page my pane's Settings tab shows (#1086): the card list, or a
    /// pushed page with `‹` back.
    pub(crate) settings_page: SettingsPage,
    /// The message menu over a bubble (`message_menu`): (chat, message, the click).
    pub(crate) message_menu: Option<(
        crate::model::Id,
        crate::model::Id,
        gpui::Point<gpui::Pixels>,
    )>,
    /// `ARBOS_RIG_MESSAGE_MENU=x,y` (screenshot rig): opens the menu once the
    /// open chat has a message; cleared when it has.
    rig_message_menu: Option<gpui::Point<gpui::Pixels>>,
    /// `ARBOS_RIG_FORWARD_MESSAGE=1` (screenshot rig): opens the "Forward to"
    /// picker for the newest message of the open chat; cleared once it has.
    rig_forward_message: bool,
    /// `ARBOS_RIG_DELETE_CONFIRM=1` (screenshot rig): opens the "Delete
    /// message?" confirm for the newest message of the open chat; cleared once it has.
    rig_delete_confirm: bool,
    /// The reaction capsule over a bubble (`reactions`): (chat, message, the smiley's click).
    pub(crate) reaction_capsule: Option<(
        crate::model::Id,
        crate::model::Id,
        gpui::Point<gpui::Pixels>,
    )>,
    /// The capsule's "pick any emoji" field is showing (system picker open).
    pub(crate) reaction_pick_open: bool,
    pub(crate) reaction_pick: Entity<ComposerInput>,
    /// `ARBOS_RIG_REACTION_CAPSULE=x,y` (screenshot rig): opens the capsule for
    /// the newest message of the open chat at that window position.
    rig_reaction_capsule: Option<gpui::Point<gpui::Pixels>>,
    pub(crate) settings_scroll: ScrollHandle,
    pub(crate) routines: Entity<RoutinesPanel>,
    pub(crate) routines_open: bool,
    pub(crate) channels: Entity<ChannelsPanel>,
    pub(crate) secrets: Entity<SecretsPanel>,
    shake_started_at: Option<u64>,
    /// A Finder drag is over the chat pane: the whole pane lights up
    /// (`ui::dropzone`). Set from the pane's own drag-move bounds check, so it
    /// does not depend on GPUI hover state; cleared on drop, exit and release.
    file_drop_over: bool,
    /// Rig: `ARBOS_FIXTURE_DRAG_OVER=1` holds the highlight on without a real
    /// drag (synthetic pointer events do not reach the GUI session).
    fixture_drag_over: bool,
    /// True while the sign-in screen is up; its clearing focuses the composer.
    signing_in: bool,
    /// Flag of the header's former "…" menu (removed, feedback
    /// app-01M5ATBWSY9E1HHBKH2QQVDEZX). Nothing renders it; escapes and
    /// click-outside still clear it so leftover true never sticks.
    pub(crate) menu_open: bool,
    /// The chat list's top strip: whether the round search button has
    /// expanded into its field (click or ⌘F), and which segment of the
    /// All · Agents · People · Servers filter is selected
    /// ([feedback sidebar-strip-filter-2026-10-24]).
    pub(crate) search_strip: sidebar::SearchStrip,
    pub(crate) list_filter: sidebar::ListFilter,
    /// Sidebar: the "Hidden chats" list is showing instead of the chat list
    /// (Jacob 2026-10-23; the footer opens it, ‹ back returns).
    pub(crate) hidden_open: bool,
    /// D88: the name/invite/channel field of the server sheets and the inline channel rename
    /// on the server page; `channel_rename` is the channel being renamed there.
    pub(crate) server_input: Entity<ComposerInput>,
    pub(crate) channel_rename: Option<crate::model::Id>,
    /// Server pane › Settings › New link options (owner): 7-day expiry, single use.
    pub(crate) invite_expires: bool,
    pub(crate) invite_single_use: bool,
    /// Channel page › Settings: the channel whose topic is being edited in `server_input`.
    pub(crate) channel_topic_edit: Option<crate::model::Id>,
    /// D88: the "Admins only post" toggle of the new-channel sheet.
    pub(crate) new_channel_restricted: bool,
    /// D88: a member row's ✕ was clicked on the server page: Cancel / Remove / Ban.
    pub(crate) server_member_armed: Option<crate::model::Id>,
    /// D88: "Leave server" / "Delete server" was clicked once; the row asks to confirm.
    pub(crate) server_leave_armed: bool,
    /// Right-click menu on a sidebar row: the space and where the click landed.
    pub(crate) row_menu: Option<(crate::model::Id, gpui::Point<gpui::Pixels>)>,
    /// Right-click menu on a channel row of the server's channel list (D88): server, channel,
    /// and where the click landed. Archive for everyone, Delete for owners/admins.
    pub(crate) channel_menu: Option<(
        crate::model::Id,
        crate::model::Id,
        gpui::Point<gpui::Pixels>,
    )>,
    /// The channel menu's Delete was clicked once: it shows "Delete #name?" with Cancel / Delete.
    pub(crate) channel_delete_armed: bool,
    /// The row menu's Delete was clicked once: it now shows "Delete …?" with
    /// Cancel / Delete instead of the action list (iPhone confirmationDialog).
    pub(crate) row_delete_armed: bool,
    /// The agent pane's "Delete agent" was clicked once: the footer row asks
    /// to confirm (D93 footer, #1318).
    pub(crate) agent_delete_armed: bool,
    /// Group pane › Members: the row whose ✕ was clicked (`user:<id>` or
    /// `agent:<project>`); it shows "Remove … ?" with Cancel / Remove.
    pub(crate) member_remove_armed: Option<String>,
    /// Settings → Devices: the device whose Sign out is asking (Oct 23).
    pub(crate) device_revoke_armed: Option<String>,
    /// A member row's menu on the group pane (Shift-click / right-click): View
    /// profile, Remove from group → arms `member_remove_armed`.
    pub(crate) member_menu: Option<member_menu::MemberMenu>,
    /// Right-click menu on a session row of the header's tab dropdown: the
    /// chat whose row carries it (#1256: anchored to the row, the dropdown
    /// stays open with the row highlighted).
    pub(crate) chat_menu: Option<crate::model::Id>,
    /// The session menu's Delete was clicked once: it shows `Delete "title"?`
    /// with Cancel / Delete instead of the action list.
    pub(crate) chat_delete_armed: bool,
    /// People chats (D72): the @search field, the header pickers.
    pub people_input: Entity<ComposerInput>,
    /// Agent profile page: inline description editor.
    pub(crate) description_input: Entity<ComposerInput>,
    /// Double-click rename: the inline name field for an agent.
    pub(crate) agent_name_input: Entity<ComposerInput>,
    /// Header name (#1068): a single click opens the profile only after
    /// `NAME_CLICK_DELAY`; every later click and a rename bump this, and the
    /// deferred open fires only if still current.
    name_click_serial: u64,
    /// Agent page: the filter field in the Model dropdown (`ui::hoverpick`).
    pub(crate) model_search_input: Entity<ComposerInput>,
    /// The right pane body's scroll (every page): a dropped-down agent row is
    /// scrolled into view once (`agent_pick_reveal`), a page pushed over
    /// another starts at the top, and `‹` restores the page underneath.
    /// `profile_scroll_epoch` mirrors `AppState::profile_opened`.
    pub(crate) profile_scroll: ScrollHandle,
    profile_scroll_epoch: u64,
    pub(crate) agent_pick_reveal: bool,
    /// Rig: `ARBOS_OPEN_SETTINGS=bottom` — my pane opens scrolled to its end
    /// (Sign out) each time it fills the pane.
    settings_open_at_bottom: bool,
    /// Settings header: editing my display name and handle (`PATCH /me`).
    pub(crate) profile_edit_open: bool,
    pub(crate) name_input: Entity<ComposerInput>,
    pub(crate) handle_input: Entity<ComposerInput>,
    pub add_agent_open: bool,
    pub members_open: bool,
    /// Double-click rename on a chat tab: the chat being edited, and its field.
    pub(crate) renaming_tab: Option<crate::model::Id>,
    pub(crate) tab_rename_input: Entity<ComposerInput>,
    /// The tab strip's "+" dropdown: New chat + the agent's previous chats.
    pub(crate) tab_menu_open: bool,
    /// The tab strip's scroll row, so a new or reopened tab comes into view.
    pub(crate) tabs_scroll: ScrollHandle,
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
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let open_settings = std::env::var("ARBOS_OPEN_SETTINGS").ok().filter(|v| {
            v == "1"
                || v == "bottom"
                || v == "connectors"
                || v == "notifications"
                || v == "machine"
                || v == "updates"
                || v == "about"
                || v == "appearance"
                || v == "credits"
                || v == "devices"
                || v.starts_with("secrets")
        });
        let settings_scroll = ScrollHandle::new();
        let transcript = cx.new(|cx| Transcript::new(state.clone(), cx));
        let reaction_pick = reactions::new_pick_input(cx);
        cx.subscribe(
            &transcript,
            |this, _, event: &TranscriptEvent, cx| match event {
                TranscriptEvent::MessageMenu {
                    chat_id,
                    message_id,
                    position,
                } => {
                    this.close_popovers(cx);
                    this.message_menu = Some((chat_id.clone(), message_id.clone(), *position));
                    cx.notify();
                }
                TranscriptEvent::ReactionCapsule {
                    chat_id,
                    message_id,
                    position,
                } => {
                    this.open_reaction_capsule(chat_id.clone(), message_id.clone(), *position, cx);
                }
                TranscriptEvent::Reply {
                    chat_id,
                    message_id,
                } => {
                    this.close_popovers(cx);
                    this.state.update(cx, |s, cx| {
                        s.set_reply_to(chat_id.clone(), message_id.clone(), cx)
                    });
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
        cx.observe(&state, |this, state, cx| {
            // The Add members picker (from a group pane or the rig hook) searches
            // agents too; Cancel / Add put "Search people" back.
            // An invite link opened the Join sheet: the code goes into its field.
            if let Some(code) = state.update(cx, |s, _| s.pending_join_code.take()) {
                this.server_input
                    .update(cx, |input, cx| input.set_text(code, cx));
            }
            // The avatar picker: a rig request to open it, and the Generate tab's answer.
            if let Some((target, tab)) = state.update(cx, |s, _| s.avatar_picker_request.take()) {
                this.open_avatar_picker(target, cx);
                if let Some(pk) = this.avatar_picker.as_mut() {
                    pk.tab = tab;
                }
            }
            if let Some(done) = state.update(cx, |s, _| s.avatar_generated.take()) {
                if let Some(pk) = this.avatar_picker.as_mut() {
                    if pk.target.id() == &done.project_id {
                        pk.generating = false;
                        match (done.image_key, done.url) {
                            (Some(key), Some(url)) => {
                                pk.generated = Some((key, url));
                                pk.error = None;
                            }
                            _ => {
                                pk.error =
                                    done.error.or_else(|| Some("No picture came back".into()))
                            }
                        }
                    }
                }
            }
            let st = state.read(cx);
            if st.add_members_secret.is_some() {
                this.people_input
                    .update(cx, |input, _| input.set_placeholder("Search people"));
            } else if st.add_members_server.is_some() {
                this.people_input
                    .update(cx, |input, _| input.set_placeholder("Add people"));
            } else if st.add_members_target.is_some() {
                this.people_input
                    .update(cx, |input, _| input.set_placeholder("Add people or agents"));
            }
            cx.notify();
        })
        .detach();
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
                ComposerEvent::Send {
                    text,
                    attachments,
                    reply_to,
                    mentions,
                } => {
                    let text = text.clone();
                    let attachments = attachments.clone();
                    let reply_to = reply_to.clone();
                    let mentions = mentions.clone();
                    this.state.update(cx, |s, cx| {
                        s.send(text, attachments, reply_to, mentions, cx)
                    });
                }
                ComposerEvent::Guide {
                    text,
                    reply_to,
                    mentions,
                } => {
                    let text = text.clone();
                    let reply_to = reply_to.clone();
                    let mentions = mentions.clone();
                    this.state
                        .update(cx, |s, cx| s.guide(text, reply_to, mentions, cx));
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
                input.set_placeholder("Name your agent");
                input.set_text_center(true);
                // Shared by Create agent and Create group (D93). Enter must match
                // the sheet's action button: agent → create, group → Next (picker).
                input.on_submit = Some(Rc::new(move |window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        let kind = if this.create_group_open {
                            CreateKind::Group
                        } else {
                            CreateKind::Agent
                        };
                        this.submit_create(kind, window, cx);
                    });
                }));
            });
        }
        let avatar_prompt_input = cx.new(ComposerInput::new);
        {
            let weak = cx.weak_entity();
            avatar_prompt_input.update(cx, |input, _| {
                input.set_placeholder("A fox reading by a lamp");
                input.on_submit = Some(Rc::new(move |_, cx| {
                    let _ = weak.update(cx, |this, cx| this.generate_avatar(cx));
                }));
            });
        }
        cx.observe(&avatar_prompt_input, |_, _, cx| cx.notify())
            .detach();
        let search_input = cx.new(ComposerInput::new);
        search_input.update(cx, |input, _| input.set_placeholder("Search"));
        let viewer_search = cx.new(ComposerInput::new);
        viewer_search.update(cx, |input, _| input.set_placeholder("Search"));
        cx.observe(&viewer_search, |_, _, cx| cx.notify()).detach();
        cx.observe(&pairing_input, |_, _, cx| cx.notify()).detach();
        cx.observe(&project_input, |_, _, cx| cx.notify()).detach();
        cx.observe(&search_input, |_, _, cx| cx.notify()).detach();
        let description_input = cx.new(ComposerInput::new);
        description_input.update(cx, |input, _| {
            input.set_placeholder("What does this agent do?")
        });
        cx.observe(&description_input, |_, _, cx| cx.notify())
            .detach();
        let agent_name_input = cx.new(ComposerInput::new);
        {
            let weak = cx.weak_entity();
            agent_name_input.update(cx, |input, _| {
                input.set_placeholder("Agent name");
                input.on_submit = Some(Rc::new(move |window, cx| {
                    let _ = weak.update(cx, |this, cx| this.commit_rename_agent(window, cx));
                }));
            });
        }
        cx.observe(&agent_name_input, |_, _, cx| cx.notify())
            .detach();
        let model_search_input = cx.new(ComposerInput::new);
        {
            let weak = cx.weak_entity();
            model_search_input.update(cx, |input, _| {
                input.set_placeholder("Search models");
                input.on_submit = Some(Rc::new(move |window, cx| {
                    let _ = weak.update(cx, |this, cx| this.pick_first_model(window, cx));
                }));
            });
        }
        cx.observe(&model_search_input, |_, _, cx| cx.notify())
            .detach();
        let name_input = cx.new(ComposerInput::new);
        {
            let weak = cx.weak_entity();
            name_input.update(cx, |input, _| {
                input.set_placeholder("Display name");
                input.on_submit = Some(Rc::new(move |_window, cx| {
                    let _ = weak.update(cx, |this, cx| this.save_profile(cx));
                }));
            });
        }
        cx.observe(&name_input, |_, _, cx| cx.notify()).detach();
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
        let tab_rename_input = cx.new(ComposerInput::new);
        {
            let weak = cx.weak_entity();
            tab_rename_input.update(cx, |input, _| {
                input.set_placeholder("Chat name");
                input.on_submit = Some(Rc::new(move |window, cx| {
                    let _ = weak.update(cx, |this, cx| this.commit_tab_rename(window, cx));
                }));
            });
        }
        cx.observe(&tab_rename_input, |_, _, cx| cx.notify())
            .detach();
        let server_input = cx.new(ComposerInput::new);
        {
            let weak = cx.weak_entity();
            server_input.update(cx, |input, _| {
                input.set_placeholder("Server name");
                input.on_submit = Some(Rc::new(move |window, cx| {
                    let _ = weak.update(cx, |this, cx| this.submit_server_input(window, cx));
                }));
            });
        }
        cx.observe(&server_input, |_, _, cx| cx.notify()).detach();
        let people_input = cx.new(ComposerInput::new);
        {
            let weak = cx.entity().downgrade();
            people_input.update(cx, |input, _| {
                input.set_placeholder("Search people");
                // Grok Mac's search placeholder is ~14 pt (ref 57).
                input.set_font_size(people::SEARCH_FONT_SIZE);
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
        // "New agent" form expanded; `ARBOS_OPEN_NEW_CHAT=1` the plain sheet
        // (`ARBOS_OPEN_NEW_GROUP=1` lives in app.rs: it needs the fixture's
        // projects to pick members from).
        if std::env::var("ARBOS_OPEN_NEW_CHAT").is_ok_and(|v| v == "1") {
            state.update(cx, |s, cx| s.open_new_chat(cx));
        }
        // Rig: `ARBOS_SEARCH=<text>` opens the sidebar search with that query
        // (synthetic key events do not reach the GPUI field on the shared display).
        let search_query = std::env::var("ARBOS_SEARCH")
            .ok()
            .filter(|q| !q.trim().is_empty());
        if let Some(q) = &search_query {
            search_input.update(cx, |input, cx| input.set_text(q.clone(), cx));
        }
        if std::env::var("ARBOS_OPEN_NEW_AGENT").is_ok_and(|v| v == "1") {
            state.update(cx, |s, cx| {
                s.open_new_chat(cx);
                s.new_project_open = true;
                cx.notify();
            });
            // `ARBOS_NEW_AGENT_NAME=…` prefills the name (the "named" state on the rig).
            if let Ok(name) = std::env::var("ARBOS_NEW_AGENT_NAME") {
                project_input.update(cx, |input, cx| input.set_text(name, cx));
            }
        }
        // Rig: `ARBOS_OPEN_CREATE=group|server` opens the D93 Create screen for that entity
        // (group = step 1; `ARBOS_OPEN_NEW_GROUP=1` is step 2); `ARBOS_NEW_AGENT_NAME` prefills.
        let open_create = std::env::var("ARBOS_OPEN_CREATE").ok();
        let create_group_open = open_create.as_deref() == Some("group");
        if create_group_open {
            state.update(cx, |s, cx| s.open_new_chat(cx));
            project_input.update(cx, |input, cx| {
                input.set_placeholder(CreateKind::Group.placeholder());
                if let Ok(name) = std::env::var("ARBOS_NEW_AGENT_NAME") {
                    input.set_text(name, cx);
                }
            });
        }
        if open_create.as_deref() == Some("server") {
            state.update(cx, |s, cx| s.open_server_sheet(ServerSheet::NewServer, cx));
            server_input.update(cx, |input, cx| {
                input.set_placeholder(CreateKind::Server.placeholder());
                if let Ok(name) = std::env::var("ARBOS_NEW_AGENT_NAME") {
                    input.set_text(name, cx);
                }
            });
        }
        let connectors = cx.new(|cx| ConnectorsPanel::new(state.clone(), cx));
        // Rig: `ARBOS_OPEN_SETTINGS=connectors` — the Connectors page inside the sheet.
        if open_settings.as_deref() == Some("connectors") {
            connectors.update(cx, |p, cx| {
                p.show(cx);
                p.embedded = true;
            });
        }
        cx.observe(&connectors, |_, _, cx| cx.notify()).detach();
        cx.subscribe(
            &connectors,
            |this, _, event: &ConnectorsEvent, cx| match event {
                ConnectorsEvent::Back => this.settings_back(cx),
            },
        )
        .detach();
        let local = cx.new(|cx| LocalPanel::new(state.clone(), cx));
        cx.observe(&local, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&local, |this, _, event: &LocalEvent, cx| match event {
            LocalEvent::Back => this.settings_back(cx),
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
        // Those rig pages open inside the Settings sheet too (#1086).
        if secrets.read(cx).is_open() {
            secrets.update(cx, |p, _| p.embedded = true);
        }
        cx.observe(&secrets, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&secrets, |this, _, event: &SecretsEvent, cx| match event {
            SecretsEvent::Back => this.settings_back(cx),
        })
        .detach();
        let routines = cx.new(|cx| RoutinesPanel::new(state.clone(), cx));
        cx.subscribe(
            &routines,
            |this, _, event: &RoutinesEvent, cx| match event {
                RoutinesEvent::Close => {
                    this.routines_open = false;
                    if this.routines.read(cx).embedded {
                        // "Close" on the Settings › Routines page: back to the list.
                        this.settings_back(cx);
                    }
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
                ChannelsEvent::Back => this.settings_back(cx),
            },
        )
        .detach();
        let skills = cx.new(|cx| SkillsPanel::new(state.clone(), cx));
        cx.observe(&skills, |_, _, cx| cx.notify()).detach();
        let feedback = cx.new(|cx| FeedbackSheet::new(state.clone(), cx));
        cx.observe(&feedback, |_, _, cx| cx.notify()).detach();

        let shell = Self {
            state,
            transcript,
            composer,
            voice,
            pairing_input,
            project_input,
            new_agent_color: None,
            create_leaf: None,
            create_leaf_color: None,
            create_group_open,
            avatar_picker: None,
            avatar_prompt_input,
            search_input,
            viewer_search,
            viewer_focus: cx.focus_handle(),
            viewer_focused_generation: 0,
            forward_message_focused_for: None,
            delete_confirm_focus: cx.focus_handle(),
            delete_confirm_focused_for: None,
            connectors,
            skills,
            local,
            focus_handle: cx.focus_handle(),
            // `ARBOS_OPEN_SETTINGS=1|bottom|connectors|secrets…` opens my pane's
            // Settings tab at launch (`bottom` scrolled to its end, the others on
            // that page) — rig; the pane itself is opened below.
            settings_page: match open_settings.as_deref() {
                Some("connectors") => SettingsPage::Connectors,
                // Rig: `ARBOS_OPEN_SETTINGS=notifications|machine|updates|about|appearance|credits`
                // lands on that subpage.
                Some("notifications") => SettingsPage::Notifications,
                Some("machine") => SettingsPage::Machine,
                Some("updates") => SettingsPage::Updates,
                Some("about") => SettingsPage::About,
                Some("appearance") => SettingsPage::Appearance,
                Some("credits") => SettingsPage::Credits,
                Some("devices") => SettingsPage::Devices,
                Some(v) if v.starts_with("secrets") => SettingsPage::Secrets,
                _ => SettingsPage::Root,
            },
            settings_scroll,
            row_menu: None,
            channel_menu: None,
            channel_delete_armed: false,
            row_delete_armed: false,
            agent_delete_armed: false,
            member_remove_armed: None,
            device_revoke_armed: None,
            member_menu: None,
            chat_menu: None,
            chat_delete_armed: false,
            message_menu: None,
            rig_message_menu: message_menu::rig_anchor(),
            rig_forward_message: std::env::var_os("ARBOS_RIG_FORWARD_MESSAGE").is_some(),
            rig_delete_confirm: std::env::var_os("ARBOS_RIG_DELETE_CONFIRM").is_some(),
            reaction_capsule: None,
            reaction_pick_open: false,
            reaction_pick,
            rig_reaction_capsule: reactions::rig_anchor(),
            renaming_tab: None,
            tab_rename_input,
            tab_menu_open: false,
            tabs_scroll: ScrollHandle::new(),
            voice_picker_open: false,
            pip_offset: (24.0, 96.0),
            pip_drag: None,
            routines,
            routines_open: false,
            channels,
            secrets,
            shake_started_at: None,
            file_drop_over: false,
            fixture_drag_over: std::env::var("ARBOS_FIXTURE_DRAG_OVER").is_ok_and(|v| v == "1"),
            signing_in: false,
            menu_open: false,
            // A rig query (`ARBOS_SEARCH`) shows the field open, as a typed one would.
            search_strip: sidebar::SearchStrip {
                open: search_query.is_some()
                    || std::env::var("ARBOS_SEARCH_OPEN").is_ok_and(|v| v == "1"),
            },
            // Rig: `ARBOS_SIDEBAR_FILTER=all|agents|people|servers` selects a segment at launch.
            list_filter: std::env::var("ARBOS_SIDEBAR_FILTER")
                .ok()
                .and_then(|v| sidebar::ListFilter::parse(&v))
                .unwrap_or_default(),
            hidden_open: false,
            server_input,
            channel_rename: None,
            invite_expires: false,
            invite_single_use: false,
            channel_topic_edit: None,
            new_channel_restricted: false,
            server_member_armed: None,
            server_leave_armed: false,
            people_input,
            description_input,
            agent_name_input,
            name_click_serial: 0,
            model_search_input,
            profile_scroll: ScrollHandle::new(),
            profile_scroll_epoch: 0,
            agent_pick_reveal: false,
            // `ARBOS_OPEN_SETTINGS_BOTTOM=1` does the same for any Settings page (a Secret
            // page's People / Activity cards sit below the fold on the rig).
            settings_open_at_bottom: open_settings.as_deref() == Some("bottom")
                || std::env::var("ARBOS_OPEN_SETTINGS_BOTTOM").is_ok_and(|v| v == "1"),
            profile_edit_open: false,
            name_input,
            handle_input,
            add_agent_open: false,
            members_open: false,
            feedback,
        };
        if open_settings.is_some() {
            shell.state.update(cx, |s, cx| {
                s.open_profile(ProfilePage::Me, cx);
                s.set_profile_tab(1, cx);
            });
            if shell.settings_page == SettingsPage::Devices {
                shell.state.update(cx, |s, cx| s.devices_refresh(cx));
            }
        }
        shell
    }

    /// Double-click a chat tab: turn its label into an editable field.
    pub(crate) fn begin_tab_rename(
        &mut self,
        chat_id: crate::model::Id,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_popovers(cx);
        self.renaming_tab = Some(chat_id);
        self.tab_rename_input.update(cx, |input, cx| {
            input.set_text(label, cx);
            input.select_all_text(cx);
        });
        let handle = self.tab_rename_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    pub(crate) fn commit_tab_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat_id) = self.renaming_tab.take() else {
            return;
        };
        let title = self.tab_rename_input.read(cx).text().to_string();
        self.tab_rename_input
            .update(cx, |input, cx| input.clear(cx));
        self.state
            .update(cx, |s, cx| s.retitle_chat(chat_id, title, cx));
        Composer::focus(&self.composer, window, cx);
        cx.notify();
    }

    pub(crate) fn cancel_tab_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming_tab.take().is_none() {
            return;
        }
        self.tab_rename_input
            .update(cx, |input, cx| input.clear(cx));
        Composer::focus(&self.composer, window, cx);
        cx.notify();
    }

    fn send_feedback(&mut self, _: &SendFeedback, window: &mut Window, cx: &mut Context<Self>) {
        self.feedback.update(cx, |sheet, cx| sheet.open(window, cx));
    }

    /// Edit → Paste / ⌘V when focus is not already on a text field (e.g. after
    /// selecting a bubble): land the clipboard in the chat composer.
    fn paste_into_chat(
        &mut self,
        _: &composer::Paste,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A focused ComposerInput (chat, search, secrets, …) already handles
        // Paste; running again here would insert the text twice.
        if self.text_field_focused(window, cx) {
            return;
        }
        Composer::focus(&self.composer, window, cx);
        let input = self.composer.read(cx).input.clone();
        input.update(cx, |input, cx| input.paste_clipboard(window, cx));
    }

    /// True when a one-line or chat text field owns the keyboard.
    fn text_field_focused(&self, window: &Window, cx: &App) -> bool {
        let focused =
            |input: &Entity<ComposerInput>| input.read(cx).focus_handle(cx).is_focused(window);
        focused(&self.composer.read(cx).input)
            || focused(&self.pairing_input)
            || focused(&self.project_input)
            || focused(&self.search_input)
            || focused(&self.viewer_search)
            || focused(&self.people_input)
            || focused(&self.server_input)
            || focused(&self.name_input)
            || focused(&self.handle_input)
            || focused(&self.agent_name_input)
            || focused(&self.description_input)
            || focused(&self.avatar_prompt_input)
            || focused(&self.model_search_input)
            || focused(&self.tab_rename_input)
            || focused(&self.feedback.read(cx).note)
            || self.connectors.read(cx).input_focused(window, cx)
            || self.secrets.read(cx).input_focused(window, cx)
            || self.skills.read(cx).has_focus(window, cx)
            || (self.routines_open && self.routines.read(cx).has_focus(window, cx))
    }

    pub(crate) fn open_feedback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = false;
        self.close_settings(cx);
        self.feedback.update(cx, |sheet, cx| sheet.open(window, cx));
    }

    // ----- settings navigation (#1086) -------------------------------------------

    /// Settings › Apps & tools row: push that page inside the sheet.
    pub(crate) fn open_settings_page(
        &mut self,
        page: SettingsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.hide_settings_page(cx);
        match page {
            SettingsPage::Root
            | SettingsPage::Credits
            | SettingsPage::Machine
            | SettingsPage::Appearance
            | SettingsPage::Notifications
            | SettingsPage::Updates
            | SettingsPage::About => {}
            SettingsPage::Devices => self.state.update(cx, |s, cx| s.devices_refresh(cx)),
            SettingsPage::Connectors => self.connectors.update(cx, |p, cx| {
                p.show(cx);
                p.embedded = true;
            }),
            SettingsPage::Routines => {
                self.routines_open = true;
                self.routines.update(cx, |p, cx| {
                    p.open(cx);
                    p.embedded = true;
                });
            }
            SettingsPage::Channels => self.channels.update(cx, |p, cx| {
                p.show(cx);
                p.embedded = true;
            }),
            SettingsPage::Skills => self.skills.update(cx, |p, cx| {
                p.open(window, cx);
                p.embedded = true;
            }),
            SettingsPage::ThisMac => self.local.update(cx, |p, cx| {
                p.show(cx);
                p.embedded = true;
            }),
            SettingsPage::Secrets => self.secrets.update(cx, |p, cx| {
                p.show(cx);
                p.embedded = true;
            }),
        }
        self.settings_page = page;
        self.state.update(cx, |s, cx| {
            if s.profile != Some(ProfilePage::Me) {
                s.open_profile(ProfilePage::Me, cx);
            }
            s.set_profile_tab(1, cx);
        });
        cx.notify();
    }

    /// Close whichever page is embedded in the sheet (its entity stops rendering).
    fn hide_settings_page(&mut self, cx: &mut Context<Self>) {
        match self.settings_page {
            SettingsPage::Root
            | SettingsPage::Credits
            | SettingsPage::Machine
            | SettingsPage::Appearance
            | SettingsPage::Notifications
            | SettingsPage::Updates
            | SettingsPage::About
            | SettingsPage::Devices => {}
            SettingsPage::Connectors => self.connectors.update(cx, |p, cx| p.hide(cx)),
            SettingsPage::Routines => {
                self.routines_open = false;
                self.routines.update(cx, |p, _| p.embedded = false);
            }
            SettingsPage::Channels => self.channels.update(cx, |p, cx| p.hide(cx)),
            SettingsPage::Skills => self.skills.update(cx, |p, cx| p.close(cx)),
            SettingsPage::ThisMac => self.local.update(cx, |p, cx| p.hide(cx)),
            SettingsPage::Secrets => self.secrets.update(cx, |p, cx| p.hide(cx)),
        }
        self.settings_page = SettingsPage::Root;
    }

    /// The pane's `‹` on a pushed page: back to the Settings list.
    pub(crate) fn settings_back(&mut self, cx: &mut Context<Self>) {
        self.hide_settings_page(cx);
        self.state.update(cx, |s, cx| {
            if s.profile != Some(ProfilePage::Me) {
                s.open_profile(ProfilePage::Me, cx);
            }
            s.set_profile_tab(1, cx);
        });
        cx.notify();
    }

    /// Escape / sign out: my pane goes, pushed page included.
    pub(crate) fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.hide_settings_page(cx);
        self.state.update(cx, |s, cx| {
            if s.profile == Some(ProfilePage::Me) {
                s.close_profile(cx);
            }
        });
        cx.notify();
    }

    /// My pane is up (the Settings window's successor).
    fn settings_pane_open(&self, cx: &App) -> bool {
        self.state.read(cx).profile == Some(ProfilePage::Me)
    }

    /// True while the embedded page's panel is still open; a page that closed
    /// itself (Skills › Close) falls back to the root list.
    fn settings_page_alive(&self, cx: &App) -> bool {
        match self.settings_page {
            SettingsPage::Root
            | SettingsPage::Credits
            | SettingsPage::Machine
            | SettingsPage::Appearance
            | SettingsPage::Notifications
            | SettingsPage::Updates
            | SettingsPage::About
            | SettingsPage::Devices => true,
            SettingsPage::Connectors => self.connectors.read(cx).is_open(),
            SettingsPage::Routines => self.routines_open,
            SettingsPage::Channels => self.channels.read(cx).is_open(),
            SettingsPage::Skills => self.skills.read(cx).open,
            SettingsPage::ThisMac => self.local.read(cx).is_open(),
            SettingsPage::Secrets => self.secrets.read(cx).is_open(),
        }
    }

    /// Settings › Credits: the feedback sheet, pre-addressed as a top-up request.
    pub(crate) fn request_top_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        // Create group reuses this field; never create an agent from a group sheet.
        if self.create_group_open {
            self.submit_create(CreateKind::Group, window, cx);
            return;
        }
        let name = self.project_input.read(cx).text().trim().to_string();
        if name.is_empty() {
            return;
        }
        self.project_input.update(cx, |input, cx| input.clear(cx));
        let color = self.new_agent_color.take();
        // D92: the Create sheet always shows a definite leaf + colour; send that pair so the
        // new agent looks exactly like its preview.
        let pick = create_entity::current_pick(self, CreateKind::Agent, cx);
        self.create_leaf = None;
        self.create_leaf_color = None;
        self.state.update(cx, |s, cx| {
            s.new_project_open = false;
            s.close_new_chat(cx);
            s.create_project(name, color, Some(pick), cx);
        });
        Composer::focus(&self.composer, window, cx);
    }

    /// Open the Create group screen (step 1) over the New Message backdrop.
    pub(crate) fn open_create_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.create_group_open = true;
        self.create_leaf = None;
        self.create_leaf_color = None;
        // Agent create wins the sheet if both flags are set; clear it so New
        // group never opens as Create New Agent.
        self.state.update(cx, |s, _| s.new_project_open = false);
        self.project_input.update(cx, |input, cx| {
            input.set_placeholder(CreateKind::Group.placeholder());
            input.clear(cx);
        });
        let handle = self.project_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// The Create screen's action: agents create, groups go on to the member
    /// picker (step 2) with the name + leaf kept as the draft, servers create.
    pub(crate) fn submit_create(
        &mut self,
        kind: CreateKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match kind {
            CreateKind::Agent => self.submit_project(window, cx),
            CreateKind::Group => {
                let name = self.project_input.read(cx).text().trim().to_string();
                if name.is_empty() {
                    return;
                }
                let pick = create_entity::current_pick(self, kind, cx);
                self.create_group_open = false;
                self.create_leaf = None;
                self.create_leaf_color = None;
                self.project_input.update(cx, |input, cx| input.clear(cx));
                self.people_input.update(cx, |input, _| {
                    input.set_placeholder("Add people or agents");
                });
                self.state.update(cx, |s, cx| {
                    s.new_group_draft = Some((name, pick));
                    s.open_new_group(cx);
                });
                let handle = self.people_input.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            CreateKind::Server => {
                let name = self.server_input.read(cx).text().trim().to_string();
                if name.is_empty() {
                    return;
                }
                let pick = create_entity::current_pick(self, kind, cx);
                self.state
                    .update(cx, |s, cx| s.create_server(name, Some(pick), cx));
            }
        }
    }

    /// × / Cancel / click outside on a Create screen.
    pub(crate) fn cancel_create(
        &mut self,
        kind: CreateKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match kind {
            CreateKind::Agent | CreateKind::Group => self.close_new_chat_sheet(window, cx),
            CreateKind::Server => self.close_server_sheet(window, cx),
        }
    }

    /// The avatar picker (D92 amendment) over the window, for an agent or a group I can edit.
    pub(crate) fn open_avatar_picker(
        &mut self,
        target: avatar_picker::Target,
        cx: &mut Context<Self>,
    ) {
        self.avatar_prompt_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.avatar_picker = Some(avatar_picker::Picker::new(target));
        cx.notify();
    }

    pub(crate) fn close_avatar_picker(&mut self, cx: &mut Context<Self>) {
        self.avatar_picker = None;
        cx.notify();
    }

    /// Generate tab: ask the relay for a picture from the prompt (preview only; Set applies).
    pub(crate) fn generate_avatar(&mut self, cx: &mut Context<Self>) {
        let prompt = self.avatar_prompt_input.read(cx).text().trim().to_string();
        let Some(pk) = self.avatar_picker.as_mut() else {
            return;
        };
        if prompt.is_empty() || pk.generating {
            return;
        }
        let avatar_picker::Target::Agent(project_id) = pk.target.clone() else {
            return;
        };
        pk.generating = true;
        pk.error = None;
        pk.reset = false;
        self.state.update(cx, |s, cx| {
            s.generate_project_image(project_id, prompt, cx);
        });
        cx.notify();
    }

    /// Upload tab: the image-only panel; the choice previews until Set.
    pub(crate) fn choose_avatar_file(&mut self, cx: &mut Context<Self>) {
        let rx = image_picker::pick_image(cx, "Choose");
        cx.spawn(async move |this, cx| {
            let Ok(Some(path)) = rx.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(pk) = this.avatar_picker.as_mut() {
                    pk.upload = Some(path);
                    pk.reset = false;
                    pk.error = None;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// "Set avatar": apply the sheet's pending choice and close it.
    pub(crate) fn set_avatar(&mut self, cx: &mut Context<Self>) {
        let Some(pk) = self.avatar_picker.take() else {
            return;
        };
        let id = pk.target.id().clone();
        self.state.update(cx, |s, cx| match &pk.target {
            avatar_picker::Target::Agent(_) => {
                if pk.reset {
                    s.set_avatar_leaf(id, None, None, cx);
                } else {
                    match pk.tab {
                        avatar_picker::Tab::Bot => {
                            let current = s
                                .projects
                                .iter()
                                .find(|p| p.id == id)
                                .map(crate::ui::avatars::project_leaf)
                                .unwrap_or_else(|| leaf::pick_for(&id, None, None));
                            s.set_avatar_leaf(
                                id,
                                Some(pk.leaf.unwrap_or(current.leaf)),
                                Some(pk.color.unwrap_or(current.color)),
                                cx,
                            );
                        }
                        avatar_picker::Tab::Generate => {
                            if let Some((key, _)) = pk.generated {
                                s.set_project_image(id, PictureSource::Key(key), cx);
                            }
                        }
                        avatar_picker::Tab::Upload => {
                            if let Some(path) = pk.upload {
                                s.set_project_image(id, PictureSource::File(path), cx);
                            }
                        }
                    }
                }
            }
            avatar_picker::Target::Group(_) => {
                if pk.reset {
                    s.set_group_leaf(id, None, cx);
                } else {
                    match pk.tab {
                        avatar_picker::Tab::Bot => {
                            let current = s
                                .projects
                                .iter()
                                .find(|p| p.id == id)
                                .map(crate::ui::avatars::project_leaf)
                                .unwrap_or_else(|| leaf::pick_for(&id, None, None));
                            s.set_group_leaf(
                                id,
                                Some(LeafPick {
                                    leaf: pk.leaf.unwrap_or(current.leaf),
                                    color: pk.color.unwrap_or(current.color),
                                }),
                                cx,
                            );
                        }
                        avatar_picker::Tab::Upload => {
                            if let Some(path) = pk.upload {
                                s.set_group_image(id, path, cx);
                            }
                        }
                        avatar_picker::Tab::Generate => {}
                    }
                }
            }
        });
        cx.notify();
    }

    /// Double-click an agent's name: load it into the rename field and focus.
    pub(crate) fn start_rename_agent(
        &mut self,
        project_id: crate::model::Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = {
            let state = self.state.read(cx);
            let Some(p) = state.project(&project_id) else {
                return;
            };
            if p.is_guest() || p.is_people() {
                return;
            }
            p.name.clone()
        };
        self.menu_open = false;
        self.row_menu = None;
        self.channel_menu = None;
        // A pending single-click open of the header name must not land on
        // top of the rename.
        self.name_click_serial = self.name_click_serial.wrapping_add(1);
        self.agent_name_input.update(cx, |input, cx| {
            input.set_text_center(false);
            input.set_text(name, cx);
        });
        self.state
            .update(cx, |s, cx| s.start_renaming(project_id, cx));
        let handle = self.agent_name_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// A single click on the header name of an agent that can be renamed:
    /// the profile opens once the double-click window has passed without a
    /// second click (#1068). Opening it at once put the page's backdrop under
    /// the second click, which closed the page instead of renaming.
    pub(crate) fn name_clicked(
        &mut self,
        project_id: crate::model::Id,
        groupish: bool,
        cx: &mut Context<Self>,
    ) {
        self.name_click_serial = self.name_click_serial.wrapping_add(1);
        let serial = self.name_click_serial;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NAME_CLICK_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.name_click_serial != serial {
                    return;
                }
                this.open_from_name(project_id, groupish, cx);
            });
        })
        .detach();
    }

    /// The header name's page: the group page for a shared project, else the
    /// agent's own profile (a second click on the name folds it away).
    pub(crate) fn open_from_name(
        &mut self,
        project_id: crate::model::Id,
        groupish: bool,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            if groupish {
                s.toggle_profile(
                    ProfilePage::Group {
                        space_id: project_id,
                    },
                    cx,
                );
            } else {
                s.toggle_avatar_picker(project_id, cx);
            }
            s.profile_from_name = s.profile.is_some();
        });
    }

    /// Enter in the name field: sidebar / header rename, or the agent page's.
    pub(crate) fn commit_rename_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (project_id, kind) = {
            let state = self.state.read(cx);
            match (&state.renaming_project, &state.profile) {
                (Some(id), _) => (id.clone(), RenameKind::Agent),
                (None, Some(ProfilePage::Agent { project_id })) if state.profile_editing_name => {
                    (project_id.clone(), RenameKind::Agent)
                }
                (None, Some(ProfilePage::Group { space_id })) if state.profile_editing_name => {
                    (space_id.clone(), RenameKind::Group)
                }
                (None, Some(ProfilePage::Server { server_id })) if state.profile_editing_name => {
                    (server_id.clone(), RenameKind::Server)
                }
                _ => return,
            }
        };
        let name = self.agent_name_input.read(cx).text().to_string();
        self.agent_name_input
            .update(cx, |input, _| input.set_text_center(false));
        self.state.update(cx, |s, cx| match kind {
            RenameKind::Agent => s.set_project_name(project_id, name, cx),
            RenameKind::Group => s.set_group_title(project_id, name, cx),
            RenameKind::Server => s.set_server_name(project_id, name, cx),
        });
        Composer::focus(&self.composer, window, cx);
        cx.notify();
    }

    /// The server page's name, clicked by an owner/admin: an inline field in its place
    /// (Return commits, Escape cancels) — the group page's flow (D88 rule 5).
    pub(crate) fn start_rename_server(
        &mut self,
        server_id: crate::model::Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .state
            .read(cx)
            .server(&server_id)
            .filter(|s| s.can_admin())
            .map(|s| s.name.clone())
        else {
            return;
        };
        self.agent_name_input.update(cx, |input, cx| {
            input.set_text_center(true);
            input.set_text(name, cx);
        });
        self.state.update(cx, |s, cx| {
            s.profile_editing_name = true;
            cx.notify();
        });
        let handle = self.agent_name_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Open one of the server sheets (D88 rules 7 and 8) with its field focused.
    pub(crate) fn open_server_sheet(
        &mut self,
        sheet: ServerSheet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = match &sheet {
            ServerSheet::NewServer => CreateKind::Server.placeholder(),
            ServerSheet::JoinServer => "Invite link or code",
            ServerSheet::NewChannel { .. } => "channel-name",
        };
        self.menu_open = false;
        self.row_menu = None;
        self.channel_menu = None;
        self.channel_rename = None;
        self.channel_topic_edit = None;
        self.new_channel_restricted = false;
        self.server_input.update(cx, |input, cx| {
            input.clear(cx);
            input.set_placeholder(placeholder);
        });
        self.people_input
            .update(cx, |input, _| input.set_placeholder("Search people"));
        self.state.update(cx, |s, cx| {
            s.close_new_chat(cx);
            s.open_server_sheet(sheet, cx);
        });
        let handle = self.server_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Cancel / click outside / Escape on a server sheet.
    pub(crate) fn close_server_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.server_input.update(cx, |input, cx| input.clear(cx));
        self.state.update(cx, |s, cx| {
            s.close_server_sheet(cx);
        });
        Composer::focus(&self.composer, window, cx);
        cx.notify();
    }

    /// Return in the server field: the open sheet's primary action, or the inline channel
    /// rename on the server page.
    pub(crate) fn submit_server_input(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let text = self.server_input.read(cx).text().trim().to_string();
        // The inline channel rename / topic edit lives on the server page or the channel page.
        let page_server = match self.state.read(cx).profile.clone() {
            Some(ProfilePage::Server { server_id })
            | Some(ProfilePage::Channel { server_id, .. }) => Some(server_id),
            _ => None,
        };
        if let Some(channel_id) = self.channel_rename.take() {
            let Some(server_id) = page_server else { return };
            self.server_input.update(cx, |input, cx| input.clear(cx));
            self.state.update(cx, |s, cx| {
                s.set_channel_name(server_id, channel_id, text, cx);
            });
            cx.notify();
            return;
        }
        if let Some(channel_id) = self.channel_topic_edit.take() {
            let Some(server_id) = page_server else { return };
            self.server_input.update(cx, |input, cx| input.clear(cx));
            self.state.update(cx, |s, cx| {
                s.set_channel_topic(server_id, channel_id, text, cx);
            });
            cx.notify();
            return;
        }
        let Some(sheet) = self.state.read(cx).server_sheet.clone() else {
            return;
        };
        let restricted = self.new_channel_restricted;
        // Enter in the Create screen: same as its action button.
        let server_pick = create_entity::current_pick(self, CreateKind::Server, cx);
        self.state.update(cx, |s, cx| match sheet {
            ServerSheet::NewServer => s.create_server(text, Some(server_pick), cx),
            ServerSheet::JoinServer => s.join_server(text, cx),
            ServerSheet::NewChannel { server_id } => {
                s.create_channel(server_id, text, restricted, cx)
            }
        });
    }

    /// Server page, Channels tab: a channel name clicked by an owner/admin becomes a field.
    pub(crate) fn start_rename_channel(
        &mut self,
        channel_id: crate::model::Id,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.channel_topic_edit = None;
        self.channel_rename = Some(channel_id);
        self.server_input.update(cx, |input, cx| {
            input.set_placeholder("channel-name");
            input.set_text(name, cx);
            input.select_all_text(cx);
        });
        let handle = self.server_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Channel page › Settings: the Topic row becomes a field (owner/admin).
    pub(crate) fn start_edit_channel_topic(
        &mut self,
        channel_id: crate::model::Id,
        topic: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.channel_rename = None;
        self.channel_topic_edit = Some(channel_id);
        self.server_input.update(cx, |input, cx| {
            input.set_placeholder("What this channel is for");
            input.set_text(topic, cx);
            input.select_all_text(cx);
        });
        let handle = self.server_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    pub(crate) fn cancel_rename_channel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.channel_rename.take().is_some() || self.channel_topic_edit.take().is_some() {
            self.server_input.update(cx, |input, cx| input.clear(cx));
            Composer::focus(&self.composer, window, cx);
            cx.notify();
        }
    }

    pub(crate) fn cancel_rename_agent(&mut self, cx: &mut Context<Self>) {
        self.agent_name_input
            .update(cx, |input, _| input.set_text_center(false));
        self.state.update(cx, |s, cx| s.cancel_renaming(cx));
        cx.notify();
    }

    /// Double-click the agent page's name (`description: false`): the name
    /// becomes a field inside the sheet (owned agents only). `description:
    /// true` also opens the about field (kept for the former Edit button).
    pub(crate) fn start_edit_agent(
        &mut self,
        project_id: crate::model::Id,
        description: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (name, about) = {
            let state = self.state.read(cx);
            let Some(p) = state.project(&project_id) else {
                return;
            };
            if p.is_guest() || p.is_people() {
                return;
            }
            (p.name.clone(), p.description.clone().unwrap_or_default())
        };
        self.menu_open = false;
        self.row_menu = None;
        self.channel_menu = None;
        self.agent_name_input.update(cx, |input, cx| {
            input.set_text_center(true);
            input.set_text(name, cx);
        });
        if description {
            self.description_input
                .update(cx, |input, cx| input.set_text(about, cx));
        }
        self.state.update(cx, |s, cx| {
            s.profile_editing_name = true;
            if description {
                s.set_profile_editing_description(true, cx);
            }
            cx.notify();
        });
        let handle = self.agent_name_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// The group pane's name, clicked: an inline field in its place (Return
    /// commits, Escape cancels).
    pub(crate) fn start_rename_group(
        &mut self,
        space_id: crate::model::Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .state
            .read(cx)
            .project(&space_id)
            .filter(|p| p.is_people_group())
            .map(|p| p.name.clone())
        else {
            return;
        };
        self.agent_name_input.update(cx, |input, cx| {
            input.set_text_center(true);
            input.set_text(name, cx);
        });
        self.state.update(cx, |s, cx| {
            s.profile_editing_name = true;
            cx.notify();
        });
        let handle = self.agent_name_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// The crayon on my own picture: pick an image (images only in the panel),
    /// `PUT /me/avatar`.
    pub(crate) fn pick_my_avatar(&mut self, cx: &mut Context<Self>) {
        let rx = image_picker::pick_image(cx, "Choose");
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let Ok(Some(path)) = rx.await else {
                return;
            };
            cx.update(|cx| {
                state.update(cx, |s, cx| s.set_my_avatar(path, cx));
            });
        })
        .detach();
    }

    /// Close the profile page (back button, click outside); the composer takes
    /// the keyboard back.
    pub(crate) fn close_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_settings_page(cx);
        self.profile_edit_open = false;
        self.member_remove_armed = None;
        self.agent_delete_armed = false;
        self.server_member_armed = None;
        self.server_leave_armed = false;
        self.channel_rename = None;
        self.channel_topic_edit = None;
        self.state.update(cx, |s, cx| {
            s.close_profile(cx);
        });
        Composer::focus(&self.composer, window, cx);
    }

    /// A member row in a group's pane (or an owner on an agent's): push the
    /// person's page over the open one, which keeps its tab and body scroll.
    pub(crate) fn push_profile(&mut self, page: ProfilePage, cx: &mut Context<Self>) {
        let scroll = -f32::from(self.profile_scroll.offset().y);
        self.member_remove_armed = None;
        self.state.update(cx, |s, cx| {
            let depth = s.profile_stack.len();
            s.push_profile(page, cx);
            // The page just covered is the new top frame; only the shell knows
            // how far its body was scrolled.
            if s.profile_stack.len() > depth {
                if let Some(frame) = s.profile_stack.last_mut() {
                    frame.scroll = scroll;
                }
            }
        });
    }

    /// The pane's `‹`: back to the page underneath, its tab and scroll as left
    /// (closes the pane when nothing is underneath).
    pub(crate) fn back_profile(&mut self, cx: &mut Context<Self>) {
        self.member_remove_armed = None;
        let (scroll, epoch) = self.state.update(cx, |s, cx| {
            let scroll = s.back_profile(cx);
            (scroll, s.profile_opened)
        });
        if let Some(scroll) = scroll {
            // Mark the epoch seen so render keeps this offset instead of
            // starting the page at the top like a fresh one.
            self.profile_scroll_epoch = epoch;
            self.profile_scroll
                .set_offset(gpui::point(px(0.0), px(-scroll)));
        }
        cx.notify();
    }

    /// Close the New Message sheet (Cancel, click outside); drops an in-progress
    /// New agent / New group form and returns the keyboard to the composer.
    pub(crate) fn close_new_chat_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.people_input
            .update(cx, |input, _| input.set_placeholder("Search people"));
        self.create_leaf = None;
        self.create_leaf_color = None;
        self.create_group_open = false;
        self.project_input.update(cx, |input, cx| {
            input.set_placeholder(CreateKind::Agent.placeholder());
            input.clear(cx);
        });
        self.state.update(cx, |s, cx| {
            s.new_project_open = false;
            s.new_group_draft = None;
            s.close_new_chat(cx);
        });
        Composer::focus(&self.composer, window, cx);
    }

    /// The agent page's "Done": save whichever of the name / description
    /// fields are open (unchanged text is not sent) and leave edit mode.
    pub(crate) fn finish_edit_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (project_id, renaming, editing, current) = {
            let state = self.state.read(cx);
            let Some(ProfilePage::Agent { project_id }) = state.profile.clone() else {
                return;
            };
            let current = state
                .project(&project_id)
                .and_then(|p| p.description.clone())
                .unwrap_or_default();
            (
                project_id,
                state.profile_editing_name,
                state.profile_editing_description,
                current,
            )
        };
        if renaming {
            let name = self.agent_name_input.read(cx).text().to_string();
            self.agent_name_input
                .update(cx, |input, _| input.set_text_center(false));
            self.state
                .update(cx, |s, cx| s.set_project_name(project_id.clone(), name, cx));
        }
        if editing {
            let text = self.description_input.read(cx).text().to_string();
            self.state.update(cx, |s, cx| {
                if text.trim() == current.trim() {
                    s.set_profile_editing_description(false, cx);
                } else {
                    s.set_description(project_id, text, cx);
                }
            });
        }
        Composer::focus(&self.composer, window, cx);
        cx.notify();
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

    /// ⌘⇧{ and ⌘⇧}: the previous / next tab in dropdown order, wrapping.
    fn step_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let next = {
            let state = self.state.read(cx);
            let Some(project_id) = state.selected_project.clone() else {
                return;
            };
            let tabs = state.visible_tabs(&project_id);
            if tabs.len() < 2 {
                return;
            }
            let active = state.active_tab.get(&project_id);
            let ix = tabs.iter().position(|c| Some(&c.id) == active).unwrap_or(0) as isize;
            let n = tabs.len() as isize;
            tabs[((ix + delta).rem_euclid(n)) as usize].id.clone()
        };
        self.tab_menu_open = false;
        self.state.update(cx, |s, cx| s.select_tab(next, cx));
        Composer::focus(&self.composer, window, cx);
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.step_tab(-1, window, cx);
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.step_tab(1, window, cx);
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_new_tab(window, cx);
    }

    /// ⌘T / "+" / "New chat": a new chat after the last tab, scrolled into
    /// view, with the composer focused.
    pub(crate) fn open_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.new_tab(cx));
        self.reveal_active_tab(cx);
        Composer::focus(&self.composer, window, cx);
    }

    /// Scroll the tab strip so the selected agent's active tab is visible.
    pub(crate) fn reveal_active_tab(&self, cx: &Context<Self>) {
        let state = self.state.read(cx);
        let Some(project_id) = state.selected_project.as_deref() else {
            return;
        };
        let active = state.active_tab.get(project_id);
        if let Some(ix) = state
            .visible_tabs(project_id)
            .iter()
            .position(|c| Some(&c.id) == active)
        {
            self.tabs_scroll.scroll_to_item(ix);
        }
    }

    fn toggle_rail(&mut self, _: &ToggleRail, window: &mut Window, cx: &mut Context<Self>) {
        let width: f32 = window.viewport_size().width.into();
        self.state.update(cx, |s, cx| {
            let narrow = s.window_is_narrow(width);
            s.toggle_rail(narrow, cx)
        });
    }

    /// ⌘F: the round search button in the chat list's strip expands into its
    /// field and takes focus ([feedback sidebar-strip-filter-2026-10-24]).
    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.open_search(window, cx);
    }

    pub(crate) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = false;
        self.search_strip.focus();
        let handle = self.search_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// The search field folds back into its round button; a typed query is
    /// dropped and the composer takes focus again.
    pub(crate) fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_strip.fold();
        self.search_input.update(cx, |i, cx| i.clear(cx));
        Composer::focus(&self.composer, window, cx);
        cx.notify();
    }

    /// Escape stops a recording memo (keeping its transcript), then closes
    /// whatever floats: menus, then sheets.
    fn escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.read(cx).forward_message.is_some() {
            self.state.update(cx, |s, cx| s.close_forward_message(cx));
            Composer::focus(&self.composer, window, cx);
            return;
        }
        if self.state.read(cx).delete_confirm.is_some() {
            self.state.update(cx, |s, cx| s.close_delete_confirm(cx));
            Composer::focus(&self.composer, window, cx);
            return;
        }
        if let Some(viewer) = self.state.read(cx).media_viewer.as_ref() {
            // Esc closes the Forward picker first, then the viewer.
            if viewer.forward_open {
                self.state
                    .update(cx, |s, cx| s.media_viewer_toggle_forward(cx));
                window.focus(&self.viewer_focus, cx);
            } else {
                self.state.update(cx, |s, cx| s.close_media_viewer(cx));
                Composer::focus(&self.composer, window, cx);
            }
            return;
        }
        if self.voice.update(cx, |v, cx| v.escape(cx)) {
            Composer::focus(&self.composer, window, cx);
            return;
        }
        if self.renaming_tab.is_some() {
            self.cancel_tab_rename(window, cx);
            return;
        }
        if self.channel_rename.is_some() || self.channel_topic_edit.is_some() {
            self.cancel_rename_channel(window, cx);
            return;
        }
        if self.state.read(cx).server_sheet.is_some() {
            self.close_server_sheet(window, cx);
            return;
        }
        if self.member_menu.is_some() || self.member_remove_armed.is_some() {
            self.member_menu = None;
            self.member_remove_armed = None;
            cx.notify();
            return;
        }
        if self.server_member_armed.is_some() || self.server_leave_armed {
            self.server_member_armed = None;
            self.server_leave_armed = false;
            cx.notify();
            return;
        }
        if self
            .search_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
        {
            // Escape in the Search field clears it, folds it back into its
            // round button and returns to the composer.
            self.close_search(window, cx);
            return;
        }
        let editing_agent = {
            let s = self.state.read(cx);
            s.renaming_project.is_some()
                || (s.profile.is_some()
                    && (s.profile_editing_name || s.profile_editing_description))
        };
        if editing_agent {
            // The agent page's edit mode (name and/or description fields) ends
            // without saving; the page itself stays up.
            self.cancel_rename_agent(cx);
            self.state
                .update(cx, |s, cx| s.set_profile_editing_description(false, cx));
            Composer::focus(&self.composer, window, cx);
            return;
        }
        if self.close_agent_pick(Some(window), cx) {
            return;
        }
        // A session row's context menu closes alone; the dropdown under it
        // stays (standard menu behaviour).
        if self.chat_menu.is_some() {
            self.chat_menu = None;
            self.chat_delete_armed = false;
            cx.notify();
            return;
        }
        let floating = self.menu_open
            || self.members_open
            || self.add_agent_open
            || self.tab_menu_open
            || self.row_menu.is_some()
            || self.chat_menu.is_some()
            || self.message_menu.is_some()
            || self.reaction_capsule.is_some();
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
        if self.profile_edit_open {
            // Escape while editing my name/handle drops the edit; the pane stays.
            self.profile_edit_open = false;
            Composer::focus(&self.composer, window, cx);
            cx.notify();
            return;
        }
        let mut closed = false;
        if self.settings_pane_open(cx) && self.settings_page != SettingsPage::Root {
            // Escape on a pushed page goes back first, like the iPhone's swipe.
            self.settings_back(cx);
            return;
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

    /// Settings → Appearance → Theme. The pick is always written, even when it
    /// equals the unsaved default, so it survives a later default change (D90).
    pub(crate) fn pick_appearance(&mut self, pref: AppearancePref, cx: &mut Context<Self>) {
        pref.save();
        let surface = Theme::of(cx).surface;
        self.set_theme(pref, surface, cx);
    }

    /// Settings → Appearance → Surface. Same persistence rule as the theme.
    pub(crate) fn pick_surface(&mut self, surface: Surface, cx: &mut Context<Self>) {
        surface.save();
        let pref = Theme::of(cx).pref;
        self.set_theme(pref, surface, cx);
    }

    fn set_theme(&mut self, pref: AppearancePref, surface: Surface, cx: &mut Context<Self>) {
        cx.set_global(Theme::for_pref(pref, surface, cx));
        reapply_window_background(cx);
        cx.notify();
    }

    /// Appearance → Opacity: set the glass shell tint from a track click / drag.
    pub(crate) fn set_glass_opacity_at(
        &mut self,
        x: gpui::Pixels,
        bounds: gpui::Bounds<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let width = f32::from(bounds.size.width).max(1.0);
        let frac = ((f32::from(x) - f32::from(bounds.left())) / width).clamp(0.0, 1.0);
        let raw = GLASS_OPACITY_MIN + frac * (GLASS_OPACITY_MAX - GLASS_OPACITY_MIN);
        let value = (raw * 100.0).round() / 100.0;
        self.set_glass_opacity(value, cx);
    }

    pub(crate) fn set_glass_opacity(&mut self, value: f32, cx: &mut Context<Self>) {
        let value = GlassOpacity::clamp(value);
        if (Theme::of(cx).glass_opacity - value).abs() < 0.005 {
            return;
        }
        GlassOpacity::save(value);
        let mut theme = Theme::of(cx).clone();
        theme.glass_opacity = value;
        cx.set_global(theme);
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

    /// Right-click / control-click on a session row of the tab dropdown
    /// (#1256): its menu opens anchored to that row; the dropdown stays open
    /// and the row stays highlighted until the menu closes.
    pub(crate) fn open_chat_menu(&mut self, chat_id: crate::model::Id, cx: &mut Context<Self>) {
        self.menu_open = false;
        self.row_menu = None;
        self.channel_menu = None;
        self.row_delete_armed = false;
        self.tab_menu_open = true;
        self.chat_menu = Some(chat_id);
        self.chat_delete_armed = false;
        cx.notify();
    }

    /// Right-click / control-click on a channel row (D88): its menu, anchored at the click.
    pub(crate) fn open_channel_menu(
        &mut self,
        server_id: crate::model::Id,
        channel_id: crate::model::Id,
        at: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.menu_open = false;
        self.row_menu = None;
        self.row_delete_armed = false;
        self.chat_menu = None;
        self.channel_menu = Some((server_id, channel_id, at));
        self.channel_delete_armed = false;
        cx.notify();
    }

    /// Shift-click / right-click on a member row of the group pane: its menu at the click.
    pub(crate) fn open_member_menu(
        &mut self,
        space_id: crate::model::Id,
        key: String,
        at: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.menu_open = false;
        self.row_menu = None;
        self.channel_menu = None;
        self.chat_menu = None;
        self.message_menu = None;
        self.close_reaction_capsule(cx);
        self.member_remove_armed = None;
        self.member_menu = Some(member_menu::MemberMenu { space_id, key, at });
        cx.notify();
    }

    /// Everything that floats closes on a click elsewhere.
    pub(crate) fn close_popovers(&mut self, cx: &mut Context<Self>) {
        self.close_agent_pick(None, cx);
        if self.menu_open
            || self.members_open
            || self.add_agent_open
            || self.tab_menu_open
            || self.row_menu.is_some()
            || self.channel_menu.is_some()
            || self.chat_menu.is_some()
            || self.message_menu.is_some()
            || self.member_menu.is_some()
            || self.reaction_capsule.is_some()
        {
            self.menu_open = false;
            self.member_menu = None;
            self.members_open = false;
            self.add_agent_open = false;
            self.tab_menu_open = false;
            self.row_menu = None;
            self.channel_menu = None;
            self.row_delete_armed = false;
            self.agent_delete_armed = false;
            self.channel_menu = None;
            self.channel_delete_armed = false;
            self.chat_menu = None;
            self.chat_delete_armed = false;
            self.message_menu = None;
            self.close_reaction_capsule(cx);
            cx.notify();
        }
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
            .child({
                let width = self.state.read(cx).sidebar_width;
                div()
                    .id("sidebar-overlay-panel")
                    .absolute()
                    .left_0()
                    .top_0()
                    .h_full()
                    .w(px(width))
                    .bg(theme.shell())
                    .border_r_1()
                    .border_color(theme.border)
                    .shadow_lg()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(frosted(0.0, PANEL_BLUR, sidebar::render(self, cx)))
            })
    }

    // ----- conversation pane ---------------------------------------------------

    /// The one chat scaffold every kind uses — agent, DM, group, server channel
    /// (Jacob, Oct 22: "match the server design and the group chat design"):
    /// `pre` (call bar, banner, chips) starts under the bar; the transcript
    /// runs up under the floating header and fades over `metrics::FADE_BAND`;
    /// a transparent drag strip continues `metrics::DRAG_OVERHANG` below the
    /// bar; the header is appended last so its pill, toggle and drag band keep
    /// their hitboxes on top. `bottom` is the composer (or the channel's quiet
    /// line). Geometry lives in `header::metrics`; `header::header_tests`
    /// checks it for every kind.
    fn chat_frame(
        mut column: Stateful<Div>,
        header_bar: Div,
        pre: Vec<AnyElement>,
        transcript: AnyElement,
        bottom: Vec<AnyElement>,
    ) -> Stateful<Div> {
        let has_pre = !pre.is_empty();
        if has_pre {
            column = column.child(div().pt(px(header::metrics::HEIGHT)).children(pre));
        }
        let body = div().flex_1().min_h_0().relative().child(transcript).child(
            div()
                .absolute()
                .top(px(if has_pre {
                    0.0
                } else {
                    header::metrics::HEIGHT
                }))
                .left_0()
                .right_0()
                .h(px(header::metrics::DRAG_OVERHANG))
                .window_drag(),
        );
        // Cap height and clip: a stretched overlay over a long transcript would
        // otherwise centre the pill mid-pane ([feedback
        // local-relay-unarbos-20261024065516774-20261024065609]; iPhone twin
        // APuu1qHpbKmmpqZeUusEi0Q).
        column.child(body).children(bottom).child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(header::metrics::HEIGHT))
                .max_h(px(header::metrics::HEIGHT))
                .overflow_hidden()
                .child(header_bar),
        )
    }

    fn render_center(&self, sidebar_hidden: bool, cx: &mut Context<Self>) -> Stateful<Div> {
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
        // The one presence row over the composer: who is typing (people) or
        // working (agents), pictures + one line, left-bound in the chat column
        // ([feedback typing-indicator-left-2026-10-23]).
        let presence = chat_id
            .as_deref()
            .zip(project.as_ref())
            .and_then(|(id, space)| crate::ui::presence::gather(state, id, space, &theme));
        // D88: a channel in the centre — the same transcript, the unified pill with the
        // server's disc and `# name`, and the composer (or rule 9's one quiet line).
        let channel = state
            .selected_channel_info()
            .map(|(server, ch)| (server.clone(), ch.clone(), state.channel_can_post()));
        if animating || shake.is_some() || presence.is_some() {
            PulseClock::request(cx);
        }

        // The composer and the transcript rows share one column (margin and
        // cap, `transcript::column_row`): Grok's agent bubble starts exactly
        // at the composer's left edge. Before this the rows sat in their own
        // narrower column (21 pt margin, 820 cap) while the pill had 16 and
        // 876, which put the bubble 21–28 pt right of the pill's edge
        // ([feedback agent-bubble-inset-2026-10-22]). The pill itself has
        // not moved.
        // Wheel over the input (sibling of the list) still scrolls the chat —
        // same as dragging over a bubble ([feedback app-01M5PDB3QYAPBPA4FJXZE1FFD0]).
        let composer_block = column_row(
            div()
                .w_full()
                .debug_selector(|| "chat-composer-block".into())
                .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                    this.transcript
                        .update(cx, |t, cx| t.scroll_from_wheel(event, cx));
                }))
                .child(self.composer.clone())
                .into_any_element(),
        );

        // Stateful so the pane can own the file-drop hitbox (anywhere over the chat).
        let mut column = div()
            .id("chat-pane")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(theme.chat_wash());

        if let Some((server, ch, can_post)) = channel {
            // D88 rule 3: the channel is a chat like any other — the same
            // floating header, fade band and drag catchment (`chat_frame`).
            let header_bar = div()
                .relative()
                .when(sidebar_hidden, |el| el.pl(px(TRAFFIC_LIGHTS_INSET)))
                .child(header::render_channel(
                    self,
                    &server,
                    &ch,
                    sidebar_hidden,
                    cx,
                ));
            let pre: Vec<AnyElement> = self
                .render_banner(cx)
                .into_iter()
                .map(IntoElement::into_any_element)
                .collect();
            let bottom = div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .pb(px(COMPOSER_BOTTOM))
                .pt(px(6.0));
            let bottom = if can_post {
                bottom.child(composer_block)
            } else {
                bottom.pb(px(COMPOSER_BOTTOM + 10.0)).child(
                    div()
                        .text_size(px(TEXT_SMALL))
                        .text_color(theme.text_muted)
                        .child("Only admins post here"),
                )
            };
            let column = Self::chat_frame(
                column,
                header_bar,
                pre,
                self.transcript.clone().into_any_element(),
                vec![bottom.into_any_element()],
            );
            return self.file_drop_target(column, &theme, cx);
        }

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
        // The header. With a transcript it floats over the list top like Grok's
        // (the list runs under it and fades out through the band, see below),
        // so it is appended last as an absolute overlay; everything else in the
        // column is laid out from the top as before.
        let header_bar = div()
            .relative()
            .left(px(shake_offset))
            // Without the sidebar the traffic lights sit over this header:
            // clear them (14 pt in, three 12 pt lights, 8 pt gaps).
            .when(sidebar_hidden, |el| el.pl(px(TRAFFIC_LIGHTS_INSET)))
            .child(header::render(self, &space, sidebar_hidden, cx));
        let floating_header = dropped;
        let mut pre: Vec<AnyElement> = Vec::new();
        let mut header_bar = Some(header_bar);
        if !floating_header {
            column = column.child(header_bar.take().expect("the header is built once"));
        }
        if let Some(chips) = header::chips(self, &space, cx) {
            pre.push(chips);
        }
        // Calls v1: my call (bar, or the full screen below), or a live call to join.
        let my_call = self.state.read(cx).my_call.clone();
        let call_here = self.state.read(cx).calls_in_chat.get(&space.id).cloned();
        let screen_open = my_call
            .as_ref()
            .map(|c| c.expanded && c.call.space_id() == space.id)
            .unwrap_or(false);
        if let Some(active) = my_call.as_ref().filter(|_| !screen_open) {
            pre.push(callview::bar(self, active, cx).into_any_element());
        } else if let Some(call) = call_here.filter(|c| {
            c.is_live()
                && my_call
                    .as_ref()
                    .map(|m| m.call.call_id != c.call_id)
                    .unwrap_or(true)
        }) {
            pre.push(callview::join_bar(&call, cx).into_any_element());
        }
        if let Some(banner) = self.render_banner(cx) {
            pre.push(banner.into_any_element());
        }
        if screen_open {
            if let Some(active) = my_call.as_ref() {
                return column
                    .children(pre)
                    .child(callview::screen(self, active, cx));
            }
        }

        if !dropped {
            column = column.children(pre);
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
            } else {
                // A group of agents only: its own picture, else the initials disc.
                let state = self.state.read(cx);
                hero = hero.child(crate::ui::groups::picture(&space, state, 88.0, &theme));
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
            let presence_row = presence
                .map(|p| column_row(crate::ui::presence::row(p, &theme, cx).into_any_element()));
            let composer_row = div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .pb(px(COMPOSER_BOTTOM))
                .pt(px(6.0))
                .children(presence_row)
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
            column = Self::chat_frame(
                column,
                header_bar
                    .take()
                    .expect("the transcript header is built once"),
                pre,
                self.transcript.clone().into_any_element(),
                vec![self.voice.clone().into_any_element(), composer_row],
            );
        }
        self.file_drop_target(column, &theme, cx)
    }

    /// Finder / file drops land anywhere over the conversation (transcript,
    /// empty hero, header or composer — the sidebar and right pane are not
    /// part of it), and the whole pane lights up while the drag is over it
    /// ([feedback chat-drop-highlight-2026-10-23], `ui::dropzone`).
    fn file_drop_target(
        &self,
        column: Stateful<Div>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let over = self.fixture_drag_over || (self.file_drop_over && cx.has_active_drag());
        dropzone::target(
            column,
            over,
            theme,
            cx,
            Self::set_file_drop_over,
            |this, paths, _, cx| {
                this.composer.update(cx, |c, cx| {
                    c.add_paths(paths.paths().iter().cloned(), cx);
                });
            },
        )
    }

    fn set_file_drop_over(&mut self, over: bool, cx: &mut Context<Self>) {
        if self.file_drop_over != over {
            self.file_drop_over = over;
            cx.notify();
        }
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
    /// Settings header: double-click the name to edit display name and handle.
    pub(crate) fn start_profile_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (name, handle) = {
            let state = self.state.read(cx);
            state
                .account
                .as_ref()
                .map(|a| (a.name.clone(), a.login.clone()))
                .unwrap_or_default()
        };
        self.name_input.update(cx, |input, cx| {
            input.set_text_center(true);
            input.set_text(name, cx);
        });
        self.handle_input
            .update(cx, |input, cx| input.set_text(handle, cx));
        self.profile_edit_open = true;
        let focus = self.name_input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn save_profile(&mut self, cx: &mut Context<Self>) {
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
                let flow = &pairing.flow;
                let busy = flow.busy();
                // The pill: "Sign in with GitHub" → "Opening GitHub sign-in…" →
                // "Waiting for GitHub…" → "Finishing sign-in…"; after 60 s with
                // no answer "GitHub did not answer." with Retry / Open in browser.
                let button = glass::primary_button("sign-in-github", flow.label(), &theme)
                    .h(px(40.0))
                    .px(px(22.0))
                    .text_size(px(TEXT_BODY))
                    .when(busy, |el| el.opacity(0.7))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.sign_in(cx));
                    }));
                let action_button = |id: &'static str, label: &str, theme: &Theme| {
                    glass::quiet_button(id, label, theme)
                        .h(px(40.0))
                        .px(px(18.0))
                        .text_size(px(TEXT_BODY))
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                };
                let mut row = div()
                    .mt(px(18.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(button);
                for action in flow.actions() {
                    row = row.child(match action {
                        sign_in::Action::Cancel => {
                            action_button("sign-in-cancel", "Cancel", &theme).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.state.update(cx, |s, cx| s.cancel_sign_in(cx));
                                },
                            ))
                        }
                        sign_in::Action::Retry => action_button("sign-in-retry", "Retry", &theme)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |s, cx| s.sign_in(cx));
                            })),
                        sign_in::Action::OpenInBrowser => {
                            action_button("sign-in-browser", "Open in browser", &theme).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |s, cx| s.sign_in_in_browser(cx));
                                }),
                            )
                        }
                    });
                }
                screen
                    .child(row)
                    .when_some(flow.hint(), |el, hint| {
                        el.child(
                            div()
                                .max_w(px(420.0))
                                .text_center()
                                .text_size(px(TEXT_SMALL))
                                .text_color(theme.text_muted)
                                .child(hint),
                        )
                    })
                    .when_some(flow.error.clone(), |el, err| {
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
        // Click dismisses — download status / HTTP errors must not stick around.
        let mut stack = div()
            .absolute()
            .right(px(16.0))
            .top(px(HEADER_HEIGHT + 12.0))
            .flex()
            .flex_col()
            .gap(px(6.0));
        for n in notices {
            let at = n.at;
            stack = stack.child(
                div()
                    .id(SharedString::from(format!("notice-{at}")))
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
                    .cursor_pointer()
                    .child(n.text)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |s, cx| {
                            s.dismiss_notice(at);
                            cx.notify();
                        });
                    })),
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

pub(crate) fn fmt_uptime(ms: u64) -> String {
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
        let viewer_open = state.media_viewer.is_some();
        let viewer_forward_open = state.media_viewer.as_ref().is_some_and(|v| v.forward_open);
        let viewer_generation = state.media_viewer_generation;
        let forward_message_open = state.forward_message.as_ref().map(|f| f.message_id.clone());
        let delete_confirm_open = state.delete_confirm.as_ref().map(|d| d.message_id.clone());
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
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::toggle_rail))
            .on_action(cx.listener(Self::focus_search))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::send_feedback))
            .on_action(cx.listener(Self::paste_into_chat))
            // Clicks on chrome must not strand keyboard focus: unless another
            // text field took it, the composer keeps it. Floating menus close.
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.close_popovers(cx);
                    // A click anywhere else drops a pending header-name open.
                    this.name_click_serial = this.name_click_serial.wrapping_add(1);
                    if this.feedback.read(cx).is_open() {
                        return;
                    }
                    // Click outside an inline tab rename commits it (the editing
                    // pill stops propagation, so this only runs for elsewhere).
                    if this.renaming_tab.is_some() {
                        this.commit_tab_rename(window, cx);
                        return;
                    }
                    // Likewise a click outside an inline channel rename (its row stops
                    // propagation too) — that one cancels.
                    if this.channel_rename.is_some() || this.channel_topic_edit.is_some() {
                        this.cancel_rename_channel(window, cx);
                        return;
                    }
                    let pairing = this.pairing_input.read(cx).focus_handle(cx);
                    let project = this.project_input.read(cx).focus_handle(cx);
                    let search = this.search_input.read(cx).focus_handle(cx);
                    let shown = this.state.read(cx).new_project_open;
                    let people = this.people_input.read(cx).focus_handle(cx);
                    if pairing.is_focused(window)
                        || (shown && project.is_focused(window))
                        || search.is_focused(window)
                        || (this.state.read(cx).new_chat_open && people.is_focused(window))
                        || this.connectors.read(cx).input_focused(window, cx)
                        || this.skills.read(cx).has_focus(window, cx)
                        || this.secrets.read(cx).input_focused(window, cx)
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
        // The right pane (three-pane rule): a flex sibling beside the chat, so
        // it never covers it; the floating menus below paint over both.
        let (profile, opened) = {
            let state = self.state.read(cx);
            (state.profile.clone(), state.profile_opened)
        };
        if let Some(page) = profile {
            // A page that just filled the pane starts at the top; a popped
            // one already carries its restored offset (see `back_profile`).
            if self.profile_scroll_epoch != opened {
                self.profile_scroll_epoch = opened;
                self.profile_scroll.set_offset(gpui::Point::default());
                if page == ProfilePage::Me && self.settings_open_at_bottom {
                    self.profile_scroll.scroll_to_bottom();
                }
            } else if page == ProfilePage::Me
                && self.settings_open_at_bottom
                && std::env::var("ARBOS_OPEN_SETTINGS_BOTTOM").is_ok_and(|v| v == "1")
            {
                // Rig only: a Settings subpage whose rows arrive after the pane opened (the
                // Secret page waits for the secrets list) keeps its end in view.
                self.profile_scroll.scroll_to_bottom();
            }
            if page == ProfilePage::Me && !self.settings_page_alive(cx) {
                self.settings_page = SettingsPage::Root;
            }
            if let Some(pane) = profiles::pane(&page, self, cx) {
                root = root.child(pane);
            }
        }
        if let Some(menu) = sidebar::row_menu(self, cx) {
            root = root.child(menu);
        }
        // Rig: the hook asked for the selected channel's row menu.
        if let Some(armed) = self.state.update(cx, |s, _| s.rig_channel_menu.take()) {
            if let Some(c) = self.state.read(cx).selected_channel.clone() {
                self.channel_menu =
                    Some((c.server_id, c.channel_id, gpui::point(px(150.0), px(135.0))));
                self.channel_delete_armed = armed;
            }
        }
        if let Some(menu) = sidebar::channel_menu(self, cx) {
            root = root.child(menu);
        }
        if let Some(at) = self.rig_message_menu {
            let target = self.state.read(cx).active_chat_id().and_then(|chat| {
                let state = self.state.read(cx);
                let last = state.messages.get(&chat)?.last()?.id.clone();
                Some((chat, last))
            });
            if let Some((chat, message)) = target {
                self.rig_message_menu = None;
                self.message_menu = Some((chat, message, at));
            }
        }
        if let Some(menu) = message_menu::render(self, window, cx) {
            root = root.child(menu);
        }
        if self.rig_forward_message {
            let target = self.state.read(cx).active_chat_id().and_then(|chat| {
                let state = self.state.read(cx);
                let last = state.messages.get(&chat)?.last()?.id.clone();
                Some((chat, last))
            });
            if let Some((chat, message)) = target {
                self.rig_forward_message = false;
                self.state
                    .update(cx, |s, cx| s.open_forward_message(chat, message, cx));
            }
        }
        if let Some(picker) = media_viewer::message_forward_overlay(self, cx) {
            // The picker's search field takes the keyboard while it is up (same
            // field as the viewer's picker; cleared on each opening).
            if self.forward_message_focused_for != forward_message_open {
                self.forward_message_focused_for = forward_message_open;
                self.viewer_search.update(cx, |input, cx| input.clear(cx));
            }
            let search = self.viewer_search.read(cx).focus_handle(cx);
            if !search.is_focused(window) {
                window.focus(&search, cx);
            }
            root = root.child(picker);
        } else if self.forward_message_focused_for.is_some() {
            self.forward_message_focused_for = None;
            if !viewer_open {
                Composer::focus(&self.composer, window, cx);
            }
        }
        if let Some(at) = self.rig_reaction_capsule {
            let target = self.state.read(cx).active_chat_id().and_then(|chat| {
                let state = self.state.read(cx);
                let last = state.messages.get(&chat)?.last()?.id.clone();
                Some((chat, last))
            });
            if let Some((chat, message)) = target {
                self.rig_reaction_capsule = None;
                self.reaction_capsule = Some((chat, message, at));
            }
        }
        if let Some(capsule) = reactions::render(self, window, cx) {
            root = root.child(capsule);
        }
        if self.rig_delete_confirm {
            let target = self.state.read(cx).active_chat_id().and_then(|chat| {
                let state = self.state.read(cx);
                let last = state.messages.get(&chat)?.last()?.id.clone();
                Some((chat, last))
            });
            if let Some((chat, message)) = target {
                self.rig_delete_confirm = false;
                self.state
                    .update(cx, |s, cx| s.open_delete_confirm(chat, message, cx));
            }
        }
        if let Some(confirm) = delete_confirm::overlay(self, &self.delete_confirm_focus, cx) {
            // Return / Esc go to the card while it is up; focus moves there once per opening.
            if self.delete_confirm_focused_for != delete_confirm_open {
                self.delete_confirm_focused_for = delete_confirm_open.clone();
                window.focus(&self.delete_confirm_focus, cx);
            }
            root = root.child(confirm);
        } else if self.delete_confirm_focused_for.is_some() {
            self.delete_confirm_focused_for = None;
            if self.delete_confirm_focus.is_focused(window) {
                Composer::focus(&self.composer, window, cx);
            }
        }
        // Rig: the hook asked for the first other member's row menu on the group pane.
        if let Some(armed) = self.state.update(cx, |s, _| s.rig_member_menu.take()) {
            let target = {
                let state = self.state.read(cx);
                match state.profile.clone() {
                    Some(ProfilePage::Group { space_id }) => state
                        .project(&space_id)
                        .and_then(|p| p.people.first().map(|m| format!("user:{}", m.id)))
                        .map(|key| (space_id, key)),
                    _ => None,
                }
            };
            if let Some((space_id, key)) = target {
                if armed {
                    self.member_remove_armed = Some(key);
                } else {
                    self.member_menu = Some(member_menu::MemberMenu {
                        space_id,
                        key,
                        at: gpui::point(px(980.0), px(330.0)),
                    });
                }
            }
        }
        if let Some(menu) = member_menu::render(self, window, cx) {
            root = root.child(menu);
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
            let right = 16.0;
            if self.members_open {
                root = root.child(glass::popover(people::project_members_popover(
                    &project,
                    &self.state,
                    right,
                    cx,
                )));
            }
        }
        // The feature panels float bottom-left only when opened outside Settings
        // (agent profile → Secrets); from Settings they render inside the sheet.
        if self.connectors.read(cx).is_floating() {
            root = root.child(self.connectors.clone());
        }
        if self.local.read(cx).is_floating() {
            root = root.child(self.local.clone());
        }
        if self.secrets.read(cx).is_floating() {
            root = root.child(self.secrets.clone());
        }
        if self.routines_open && !self.routines.read(cx).embedded {
            root = root.child(self.routines.clone());
        }
        if self.channels.read(cx).is_floating() {
            root = root.child(self.channels.clone());
        }
        if self.skills.read(cx).is_floating() {
            root = root.child(self.skills.clone());
        }
        if self.voice_picker_open {
            if let Some(space) = selected.as_ref().filter(|s| !s.is_people()) {
                root = root.child(voicepicker::sheet(self, space, cx));
            }
        }
        if let Some(picker) = self.avatar_picker.clone() {
            root = root.child(avatar_picker::sheet(self, &picker, cx));
        }
        if new_chat_open {
            // D93: "New agent" / "New group" open the Create screen over the same backdrop
            // (a group goes on to the member picker as step 2).
            if self.state.read(cx).new_project_open {
                root = root.child(create_entity::sheet(self, CreateKind::Agent, cx));
            } else if self.create_group_open {
                root = root.child(create_entity::sheet(self, CreateKind::Group, cx));
            } else {
                root = root.child(people::new_chat_sheet(&self.state, &self.people_input, cx));
            }
        }
        if let Some(sheet) = self.state.read(cx).server_sheet.clone() {
            root = root.child(match sheet {
                ServerSheet::NewServer => create_entity::sheet(self, CreateKind::Server, cx),
                other => servers::sheet(self, &other, cx),
            });
        }
        if self.feedback.read(cx).is_open() {
            root = root.child(self.feedback.clone());
        }
        if viewer_open {
            // Focus moves into the viewer once per opening (← → page, Esc
            // closes); the Forward picker's search field takes it while it is up.
            if self.viewer_focused_generation != viewer_generation {
                self.viewer_focused_generation = viewer_generation;
                self.viewer_search.update(cx, |input, cx| input.clear(cx));
                window.focus(&self.viewer_focus, cx);
            }
            let search = self.viewer_search.read(cx).focus_handle(cx);
            if viewer_forward_open {
                if !search.is_focused(window) {
                    window.focus(&search, cx);
                }
            } else if search.is_focused(window) {
                window.focus(&self.viewer_focus, cx);
            }
            root = root.child(media_viewer::overlay(self, &self.viewer_focus, cx));
        } else if self.viewer_focus.is_focused(window)
            || self
                .viewer_search
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        {
            // Closed by × / the backdrop / Forward: typing goes back to the composer.
            Composer::focus(&self.composer, window, cx);
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
pub(crate) fn human_bytes(bytes: i64) -> String {
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
