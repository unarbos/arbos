//! Root view, Grok Bot layout: chat-list sidebar (the iPhone home list) ·
//! one conversation pane (floating glass header, bubbles, the composer) ·
//! and the debug Details drawer. Sheets (Settings, New Message, agent profile)
//! float centred over everything.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    actions, div, prelude::*, px, Animation, AnimationExt, AnyElement, App, Bounds, Context, Div,
    Entity, FocusHandle, Focusable, FontWeight, Pixels, Render, ScrollHandle, SharedString,
    Stateful, Window,
};

use crate::app::SignInMode;
use crate::app::{AppEvent, AppState, DetailsTarget, ProfilePage, ServerSheet};
use crate::image_picker;
use crate::model::{now_ms, CallKind, MachineState, Person};
use crate::sign_in;
use crate::sync::user_error::{ErrorClass, UserError};
use crate::sync::{ClientCommand, PictureSource};
use crate::ui::avatar_picker;
use crate::ui::avatars::{self, leaf, LeafPick};
use crate::ui::blocked;
use crate::ui::callview;
use crate::ui::channels::{ChannelsEvent, ChannelsPanel};
use crate::ui::chart_viewer;
use crate::ui::chat_search;
use crate::ui::composer::{self, Composer, ComposerEvent, ComposerInput};
use crate::ui::connect_agent;
use crate::ui::connectors::{ConnectorsEvent, ConnectorsPanel};
use crate::ui::create_entity::{self, Kind as CreateKind};
use crate::ui::dropzone;
use crate::ui::error_note;
use crate::ui::feedback::{FeedbackSheet, SendFeedback};
use crate::ui::frost::{frosted, MENU_BLUR, PANEL_BLUR};
use crate::ui::glass::{self, WindowDrag};
use crate::ui::header;
use crate::ui::highlight;
use crate::ui::local::{LocalEvent, LocalPanel};
use crate::ui::media_viewer;
use crate::ui::member_menu;
use crate::ui::message_menu;
use crate::ui::modal;
use crate::ui::motion::{ease_out_cubic, PulseClock, COMPOSER_DROP_MS, SHAKE_MS};
use crate::ui::onboarding;
use crate::ui::palette;
use crate::ui::people;
use crate::ui::profiles;
use crate::ui::profiles::machine;
use crate::ui::rail;
use crate::ui::reactions;
use crate::ui::routines::{RoutinesEvent, RoutinesPanel};
use crate::ui::secrets::{SecretsEvent, SecretsPanel};
use crate::ui::servers;
use crate::ui::sf_symbols;
use crate::ui::sidebar;
use crate::ui::sidebar_fit;
use crate::ui::skills::{self, SkillsPanel};
use crate::ui::slide::{self, ChatSlide, PaneSlides};
use crate::ui::theme::{
    reapply_window_background, AppearancePref, GlassOpacity, Surface, Theme, COLUMN_MARGIN,
    COMPOSER_BOTTOM, COMPOSER_HEIGHT, COMPOSER_WIDTH, DRAWER_WIDTH, GLASS_OPACITY_MAX,
    GLASS_OPACITY_MIN, HEADER_HEIGHT, LINE_HEIGHT, PANEL_RADIUS, TAB_BAR_HEIGHT, TEXT_BODY,
    TEXT_SMALL, TEXT_TINY,
};
use crate::ui::transcript::COLUMN_MAX_WIDTH;
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
        OpenPalette,
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
    /// The pages my pane's Settings list shows today (`profiles/me.rs`, in
    /// its order); the search index must find each of them
    /// ([feedback search-settings-mac-2026-10-26], `palette::tests`).
    #[cfg(test)]
    pub(crate) const LISTED: [SettingsPage; 8] = [
        SettingsPage::Appearance,
        SettingsPage::Notifications,
        SettingsPage::Secrets,
        SettingsPage::Devices,
        SettingsPage::ThisMac,
        SettingsPage::Machine,
        SettingsPage::Updates,
        SettingsPage::About,
    ];

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

/// What the chat column needs to know about its neighbours this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CenterGeometry {
    /// Left padding on the header bar: the traffic lights' band
    /// (`TRAFFIC_LIGHTS_INSET`) while the sidebar is hidden, the part of it
    /// the sidebar no longer covers while it slides, 0 while it is docked.
    header_inset: f32,
    /// Show the folded-sidebar expand disc in the header. During the collapse
    /// slide it appears once the inset puts it at its resting spot (72 + 12
    /// from the window edge), after the sidebar's own disc has left the frame.
    show_sidebar_toggle: bool,
    /// A slide is running: the transcript keeps `ChatSlide::start_width`
    /// inside the live slot width (`ui::slide`).
    chat_slide: Option<(ChatSlide, f32)>,
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
    /// D93 Create screen (Jacob, 2026-10-25): the draft's picture — the picker's own
    /// Upload | Generate state on a `Target::Draft`; nothing picked = the neutral initial.
    pub(crate) create_picker: avatar_picker::Picker,
    /// Create group, step 1 (picture + name) showing over the New Message backdrop.
    pub(crate) create_group_open: bool,
    /// Connect an agent (#2141): the transport picked on the Create screen.
    pub(crate) connect_transport: create_entity::Transport,
    /// The avatar picker sheet (D92 amendment) while it is open: whose picture, which tab,
    /// the pending choice.
    pub(crate) avatar_picker: Option<avatar_picker::Picker>,
    /// The Generate tab's prompt.
    pub(crate) avatar_prompt_input: Entity<ComposerInput>,
    /// The command palette's search field (⌘⇧F / ⌘K / the sidebar's Search
    /// disc; [feedback search-palette-2026-10-24]).
    pub(crate) search_input: Entity<ComposerInput>,
    /// ⌘F Search-in-chat's field ([feedback search-in-chat-mac-2026-10-26]);
    /// its text drives `AppState::chat_search`.
    pub(crate) chat_search_input: Entity<ComposerInput>,
    /// Rig: `ARBOS_CHAT_SEARCH=<text>` opens the Search-in-chat bar with that
    /// query once the first chat with rows is on screen (synthetic key events
    /// do not reach the GPUI field on the shared display). Consumed on use.
    chat_search_rig: Option<String>,
    /// The palette: open/closed, highlighted row, rows drawn last frame.
    pub(crate) palette: palette::PaletteState,
    /// Where the palette's panel was painted last frame; a mouse-down outside
    /// it closes the palette without swallowing the click (`click_outside`).
    pub(crate) palette_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Likewise the Search-in-chat bar.
    pub(crate) chat_search_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The palette's result list scroll (keeps the highlight in view).
    pub(crate) palette_scroll: ScrollHandle,
    /// The media viewer's Forward picker search field (feedback media-viewer-2026-10-23).
    pub(crate) viewer_search: Entity<ComposerInput>,
    /// Keyboard focus while the media viewer is up (← → page, Esc closes).
    viewer_focus: FocusHandle,
    /// The bubble whose "Forward to" picker last took the search field's focus
    /// (`AppState::forward_message`), so it is cleared and focused once per opening.
    forward_message_focused_for: Option<crate::model::Id>,
    /// `AppState::media_viewer_generation` the viewer was last focused for.
    viewer_focused_generation: u64,
    /// `AppState::chart_viewer_generation` the chart overlay was last focused for.
    chart_viewer_focused_generation: u64,
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
    /// Rig: `ARBOS_PEOPLE_QUERY=<text>` types into the people picker's search
    /// field once a people sheet is open (the Give-access / Add-members shot
    /// with a query, [feedback shared-search-matcher-mac-2026-10-26]).
    rig_people_query: Option<String>,
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
    /// The window is in native full screen (read at each render). The layout
    /// is the same as windowed (`sidebar::top_strip`); the traffic lights are
    /// macOS's own and follow its full-screen rules — the app paints none
    /// (Jacob 2026-10-24, [feedback topbar-grok-parity-2026-10-24]).
    pub(crate) fullscreen: bool,
    /// Flag of the header's former "…" menu (removed, feedback
    /// app-01M5ATBWSY9E1HHBKH2QQVDEZX). Nothing renders it; escapes and
    /// click-outside still clear it so leftover true never sticks.
    pub(crate) menu_open: bool,
    /// Which segment of the chat list's All · Agents · People · Servers
    /// filter is selected ([feedback sidebar-strip-filter-2026-10-24]);
    /// persisted with the rail state ([feedback mac-sidebar-rail-2026-10-26]).
    pub(crate) list_filter: sidebar::ListFilter,
    /// Where the expanded sidebar's discs, pictures and footer picture rest,
    /// recorded every frame they are painted; the rail morph reads it.
    pub(crate) morph: rail::Morph,
    /// The rail picture under the pointer (its name tip is drawn at the root).
    pub(crate) rail_hover: Option<crate::model::Id>,
    /// Whether the sidebar's edge handle is mid-drag: its hairline shows the
    /// drag state while GPUI suppresses hover styles (`ui::split_handle`).
    pub(crate) split_drag: crate::ui::split_handle::DragState,
    /// The rail's picture list scroll (the tip and the morph follow it).
    pub(crate) rail_scroll: ScrollHandle,
    /// The sidebar filter capsules' horizontal scroll (a narrow column,
    /// [feedback sidebar-narrow-wrap-2026-10-27]).
    pub(crate) filter_scroll: ScrollHandle,
    /// Scrolls the selected capsule into view once per selection / width, so
    /// the strip stays scrollable back to "All" ([feedback
    /// server-strip-scroll-2026-10-28]).
    pub(crate) filter_pin: sidebar_fit::FilterPin,
    /// Rig: `ARBOS_RIG_FULLSCREEN=1` lays the chrome out as native full
    /// screen (no layout difference since the rail keeps the lights' slot) where the platform
    /// cannot report it (headless Linux renders).
    rig_fullscreen: bool,
    /// Rig: `ARBOS_RIG_RAIL_T=0.4` freezes the collapse morph at that
    /// progress (0 expanded … 1 rail) for still frames of the transition.
    rig_rail_t: Option<f32>,
    /// The viewport width as of the last render (the rig's timed toggle needs
    /// it without a `Window`).
    last_viewport_width: f32,
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
    /// A pinned sidebar row mid-drag to a new slot ([feedback sidebar-pinning-mac-2026-10-28]).
    pub(crate) pin_drag: Option<sidebar::PinDragState>,
    /// Right-click menu on a channel row of the server's channel list (D88): server, channel,
    /// and where the click landed. Archive for everyone, Delete for owners/admins.
    pub(crate) channel_menu: Option<(
        crate::model::Id,
        crate::model::Id,
        gpui::Point<gpui::Pixels>,
    )>,
    /// The channel menu's Delete was clicked once: it shows "Delete #name?" with Cancel / Delete.
    pub(crate) channel_delete_armed: bool,
    /// The pointer over the agent pane's "Run by …" owner link (the mention
    /// chip's hover / pressed fade, [feedback agent-pane-run-by-link-mac-2026-10-25]).
    pub(crate) run_by_chip: crate::ui::mention::ChipPointer<()>,
    /// Group pane › Members: the row whose ✕ was clicked (`user:<id>` or
    /// `agent:<project>`); it shows "Remove … ?" with Cancel / Remove.
    pub(crate) member_remove_armed: Option<String>,
    /// The person an Unblock is being confirmed for ([feedback blocked-state-mac-2026-10-26]):
    /// the pane's footer row, the DM composer bar and the chat note draw the confirm in their
    /// place while this names their person; `confirm_unblock` lifts the block, `cancel_unblock`
    /// puts the row / bar back.
    pub(crate) unblock_armed: Option<Person>,
    /// Settings → Devices: the device whose Sign out is asking (Oct 23).
    pub(crate) device_revoke_armed: Option<String>,
    /// Settings → Devices: the load-failure note's Details disclosure.
    pub(crate) devices_error_details_open: bool,
    /// Settings → Updates: the failed-install note's Details disclosure.
    pub(crate) update_error_details_open: bool,
    /// The chat-level notes' Details disclosures (machine error, delivery refusal).
    pub(crate) machine_error_details_open: bool,
    pub(crate) delivery_error_details_open: bool,
    /// A member row's menu on the group pane (Shift-click / right-click): View
    /// profile, Remove from group → arms `member_remove_armed`.
    pub(crate) member_menu: Option<member_menu::MemberMenu>,
    /// The Library tab's hover, unfolded days and ⋯ menu
    /// ([feedback library-list-hover-mac-2026-10-25], `profiles/library.rs`).
    pub(crate) library: crate::ui::profiles::library::LibraryUi,
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
    /// Connected agent's Connection card: the Wake URL and its Authorization value (masked).
    pub(crate) wake_url_input: Entity<ComposerInput>,
    pub(crate) wake_auth_input: Entity<ComposerInput>,
    /// The agent whose Wake URL the two fields were last filled for.
    pub(crate) wake_form_project: Option<crate::model::Id>,
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
    /// The pane slides ([feedback right-pane-slide-2026-10-24], [feedback
    /// sidebar-slide-2026-10-24]): derived each frame from `profile` and
    /// `rail_collapsed`; `pane_page_drawn` is the page the right pane showed
    /// last, so a closing pane can keep drawing it while it slides out.
    slides: PaneSlides,
    pane_page_drawn: Option<ProfilePage>,
    pub(crate) agent_pick_reveal: bool,
    /// Rig: `ARBOS_OPEN_SETTINGS=bottom` — my pane opens scrolled to its end
    /// (Sign out) each time it fills the pane.
    settings_open_at_bottom: bool,
    /// Settings › Machine: the per-agent breakdown is unfolded (folds when the page goes).
    pub(crate) machine_breakdown_open: bool,
    /// Settings › Machine: a `GET /machine/debug` poll loop is running (`machine::POLL_MS`);
    /// the loop stops itself when the page is no longer shown.
    machine_polling: bool,
    /// Rig: `ARBOS_PROFILE_SCROLL=bottom` — a freshly opened agent pane starts scrolled to
    /// its end (Permissions · Profile sit below the fold on the 1440×900 rig display).
    profile_open_at_bottom: bool,
    /// Settings header: editing my display name and handle (`PATCH /me`).
    pub(crate) profile_edit_open: bool,
    pub(crate) name_input: Entity<ComposerInput>,
    pub(crate) handle_input: Entity<ComposerInput>,
    pub members_open: bool,
    /// Double-click rename on a chat tab: the chat being edited, and its field.
    pub(crate) renaming_tab: Option<crate::model::Id>,
    pub(crate) tab_rename_input: Entity<ComposerInput>,
    /// The tab strip's "+" dropdown: New chat + the agent's previous chats.
    pub(crate) tab_menu_open: bool,
    /// The tasks chip's pointer state (rest / hover / pressed fade, `ui::tasks`).
    pub(crate) tasks_chip: crate::ui::mention::ChipPointer<u8>,
    /// The status line's link state (`tasks::link`: rest / hover / pressed).
    pub(crate) tasks_link: crate::ui::tasks::LinkState,
    /// The tab strip's scroll row, so a new or reopened tab comes into view.
    pub(crate) tabs_scroll: ScrollHandle,
    /// Per-agent call voice picker sheet ("…" menu → Voice, profile → voice).
    pub(crate) voice_picker_open: bool,
    /// Video picture-in-picture while the call screen is collapsed: offset
    /// from the window's bottom-right (right, bottom), and the drag in flight
    /// (mouse start, offset start, moved past the click threshold).
    pub(crate) pip_offset: (f32, f32),
    pub(crate) pip_drag: Option<(gpui::Point<gpui::Pixels>, (f32, f32), bool)>,
    /// ⌘⌥F "Send feedback" sheet (self-contained in `ui::feedback`).
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
                TranscriptEvent::Unblock { person } => {
                    // A group: the confirm needs a place — the person's pane, opened over
                    // the chat with the confirm already armed on its Unblock row.
                    let page = ProfilePage::User {
                        handle: person.handle.clone(),
                        space_id: this.state.read(cx).selected_project.clone(),
                    };
                    this.state.update(cx, |s, cx| s.open_profile(page, cx));
                    this.arm_unblock(person.clone(), cx);
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
                // Rig: `…:avatar:upload=<path>` — the panel's pick, made for the shot.
                if let Some(path) = state.update(cx, |s, _| s.avatar_rig_upload.take()) {
                    if let Some(pk) = this.avatar_picker.as_mut() {
                        pk.upload = Some(path);
                        pk.reset = false;
                    }
                }
                // Rig: type the prompt and press Generate so a shot shows prompt → result.
                if let Some(prompt) = state.update(cx, |s, _| s.avatar_rig_prompt.take()) {
                    this.avatar_prompt_input
                        .update(cx, |input, cx| input.set_text(prompt, cx));
                    this.generate_avatar(cx);
                }
                // Rig: `ARBOS_RIG_VIEW=<open ms>[,<close ms>]` presses the picker's View after
                // `open ms`, then the viewer's × after `close ms` (a shot of each state).
                if let Ok(spec) = std::env::var("ARBOS_RIG_VIEW") {
                    let mut parts = spec.split(',').filter_map(|p| p.trim().parse::<u64>().ok());
                    let open_ms = parts.next().unwrap_or(1500);
                    let close_ms = parts.next();
                    cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(open_ms))
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            let picture = this.avatar_picker.as_ref().and_then(|pk| {
                                avatar_picker::viewable_picture(pk, this.state.read(cx))
                            });
                            if let Some((name, url)) = picture {
                                this.view_picture(&name, &url, cx);
                            }
                        });
                        let Some(close_ms) = close_ms else { return };
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(close_ms))
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            this.state.update(cx, |s, cx| s.close_media_viewer(cx));
                            cx.notify();
                        });
                    })
                    .detach();
                }
            }
            if let Some(done) = state.update(cx, |s, _| s.avatar_generated.take()) {
                if let Some(pk) = this.pending_picker_mut() {
                    if pk.target == done.target {
                        pk.generating = false;
                        pk.started_at = None;
                        match (done.image_key, done.url) {
                            (Some(key), Some(url)) => {
                                pk.generated = Some((key, url));
                                pk.error = None;
                            }
                            _ => {
                                pk.fail(done.error.unwrap_or_else(|| {
                                    UserError::from_text(
                                        "generate the image",
                                        "No picture came back",
                                    )
                                }));
                            }
                        }
                    }
                }
            }
            if state.read(cx).new_chat_open {
                if let Some(q) = this.rig_people_query.take() {
                    this.people_input
                        .update(cx, |input, cx| input.set_text(q, cx));
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
            crate::ui::working::WorkingClock::set_window_active(cx, active);
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
                ComposerEvent::Edit { message_id, text } => {
                    let message_id = message_id.clone();
                    let text = text.clone();
                    let Some(chat_id) = this.state.read(cx).active_chat_id() else {
                        return;
                    };
                    this.state
                        .update(cx, |s, cx| s.edit_message(chat_id, message_id, text, cx));
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
                        } else if this.state.read(cx).connect_open {
                            CreateKind::Connect
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
        {
            let weak = cx.weak_entity();
            let weak_arrow = cx.weak_entity();
            search_input.update(cx, |input, _| {
                input.set_placeholder("Search");
                input.set_font_size(palette::FIELD_FONT);
                // Return opens the highlighted row; ↑ ↓ move the highlight.
                input.on_submit = Some(Rc::new(move |window, cx| {
                    let _ = weak.update(cx, |this, cx| this.palette_open_selected(window, cx));
                }));
                input.on_arrow = Some(Rc::new(move |delta, cx| {
                    let _ = weak_arrow.update(cx, |this, cx| this.palette_step(delta, cx));
                }));
            });
        }
        let chat_search_input = cx.new(ComposerInput::new);
        chat_search_input.update(cx, |input, _| {
            input.set_placeholder("Search in chat");
            input.set_font_size(chat_search::metrics::FIELD_FONT);
        });
        // Every keystroke re-runs the search; Enter / ⇧Enter / ⇥ / Esc are
        // actions in the bar's "ChatSearch" context.
        cx.observe(&chat_search_input, |this, input, cx| {
            let typed = input.read(cx).text().to_string();
            this.state
                .update(cx, |s, cx| s.chat_search_set_text(typed, cx));
            cx.notify();
        })
        .detach();
        // Rows of the searched chat changed (a message, a channel page): the
        // hits follow, after the state's own update has finished.
        cx.subscribe(&state, |_, state, event: &AppEvent, cx| {
            if let AppEvent::TranscriptText { chat_id, .. }
            | AppEvent::TranscriptChanged { chat_id } = event
            {
                let chat_id = chat_id.clone();
                cx.defer(move |cx| {
                    state.update(cx, |s, cx| s.chat_search_refresh(&chat_id, cx));
                });
            }
        })
        .detach();
        let viewer_search = cx.new(ComposerInput::new);
        viewer_search.update(cx, |input, _| {
            input.set_placeholder("Search");
            input.set_font_size(media_viewer::PICKER_FIELD_FONT);
        });
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
        let wake_url_input = cx.new(ComposerInput::new);
        wake_url_input.update(cx, |input, _| {
            input.set_placeholder("https://…");
        });
        cx.observe(&wake_url_input, |_, _, cx| cx.notify()).detach();
        let wake_auth_input = cx.new(ComposerInput::new);
        wake_auth_input.update(cx, |input, _| {
            input.set_placeholder("Paste the Authorization header value");
            input.set_masked(true);
        });
        cx.observe(&wake_auth_input, |_, _, cx| cx.notify())
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
        // Rig: `ARBOS_SEARCH=<text>` opens the command palette with that query
        // (synthetic key events do not reach the GPUI field on the shared
        // display); `ARBOS_PALETTE=1` opens it empty (recents);
        // `ARBOS_PALETTE_SELECT=<n>` highlights row n; `ARBOS_PALETTE_OPEN=1`
        // opens that row once the rows are there (the jump-to-message shot).
        let search_query = std::env::var("ARBOS_SEARCH")
            .ok()
            .filter(|q| !q.trim().is_empty());
        if let Some(q) = &search_query {
            search_input.update(cx, |input, cx| input.set_text(q.clone(), cx));
        }
        let palette = palette::PaletteState {
            open: search_query.is_some() || std::env::var("ARBOS_PALETTE").is_ok_and(|v| v == "1"),
            cursor: crate::palette::Cursor {
                selected: std::env::var("ARBOS_PALETTE_SELECT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
            },
            open_on_rows: std::env::var("ARBOS_PALETTE_OPEN").is_ok_and(|v| v == "1"),
            ..Default::default()
        };
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
        // Rig: `ARBOS_CREATE_PICTURE=upload|generate[:<prompt>]` opens the Create sheet on that
        // option (and types the prompt into the Generate field).
        let mut create_picker = avatar_picker::Picker::new(avatar_picker::Target::Draft);
        if let Ok(spec) = std::env::var("ARBOS_CREATE_PICTURE") {
            let (option, prompt) = spec.split_once(':').unwrap_or((spec.as_str(), ""));
            if option == "generate" {
                create_picker.tab = avatar_picker::Tab::Generate;
                if !prompt.is_empty() {
                    avatar_prompt_input
                        .update(cx, |input, cx| input.set_text(prompt.to_owned(), cx));
                }
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
        // `secrets-people:<secret env>` (that secret's "Give a person access" sheet),
        // `secrets-agent:<project id or name>` (an agent's page).
        match open_settings.as_deref() {
            Some("secrets") => secrets.update(cx, |p, cx| p.show(cx)),
            Some(v) if v.starts_with("secrets:") => {
                let env = v["secrets:".len()..].to_string();
                secrets.update(cx, |p, cx| p.show_secret_env(env, cx));
            }
            Some(v) if v.starts_with("secrets-people:") => {
                let env = v["secrets-people:".len()..].to_string();
                secrets.update(cx, |p, cx| p.show_secret_people(env, cx));
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
            create_picker,
            create_group_open,
            connect_transport: create_entity::Transport::default(),
            avatar_picker: None,
            avatar_prompt_input,
            search_input,
            chat_search_input,
            chat_search_rig: std::env::var("ARBOS_CHAT_SEARCH")
                .ok()
                .filter(|q| !q.trim().is_empty()),
            palette,
            palette_bounds: Rc::new(Cell::new(None)),
            chat_search_bounds: Rc::new(Cell::new(None)),
            palette_scroll: ScrollHandle::new(),
            viewer_search,
            viewer_focus: cx.focus_handle(),
            viewer_focused_generation: 0,
            chart_viewer_focused_generation: 0,
            forward_message_focused_for: None,
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
            pin_drag: None,
            channel_menu: None,
            channel_delete_armed: false,
            run_by_chip: Default::default(),
            member_remove_armed: None,
            unblock_armed: None,
            device_revoke_armed: None,
            devices_error_details_open: false,
            update_error_details_open: false,
            machine_error_details_open: false,
            delivery_error_details_open: false,
            member_menu: None,
            library: Default::default(),
            chat_menu: None,
            chat_delete_armed: false,
            message_menu: None,
            rig_message_menu: message_menu::rig_anchor(),
            rig_forward_message: std::env::var_os("ARBOS_RIG_FORWARD_MESSAGE").is_some(),
            rig_people_query: std::env::var("ARBOS_PEOPLE_QUERY")
                .ok()
                .filter(|q| !q.trim().is_empty()),
            reaction_capsule: None,
            reaction_pick_open: false,
            reaction_pick,
            rig_reaction_capsule: reactions::rig_anchor(),
            renaming_tab: None,
            tab_rename_input,
            // Rig: `ARBOS_RIG_TABMENU=1` starts with the tab dropdown open (job tab shots).
            tab_menu_open: std::env::var("ARBOS_RIG_TABMENU").is_ok_and(|v| v == "1"),
            tasks_chip: crate::ui::mention::ChipPointer::default(),
            tasks_link: crate::ui::tasks::LinkState::default(),
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
            fullscreen: false,
            menu_open: false,
            // The saved segment; the rig's `ARBOS_SIDEBAR_FILTER=all|agents|people|servers`
            // wins at launch.
            list_filter: sidebar::ListFilter::load(),
            morph: Default::default(),
            rail_hover: None,
            split_drag: Default::default(),
            rail_scroll: ScrollHandle::new(),
            filter_scroll: ScrollHandle::new(),
            filter_pin: sidebar_fit::FilterPin::default(),
            rig_fullscreen: std::env::var("ARBOS_RIG_FULLSCREEN").is_ok_and(|v| v == "1"),
            rig_rail_t: std::env::var("ARBOS_RIG_RAIL_T")
                .ok()
                .and_then(|v| v.trim().parse::<f32>().ok())
                .map(|t| t.clamp(0.0, 1.0)),
            last_viewport_width: crate::ui::theme::NARROW_WINDOW,
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
            wake_url_input,
            wake_auth_input,
            wake_form_project: None,
            agent_name_input,
            name_click_serial: 0,
            model_search_input,
            profile_scroll: ScrollHandle::new(),
            profile_scroll_epoch: 0,
            slides: PaneSlides::default(),
            pane_page_drawn: None,
            agent_pick_reveal: false,
            // `ARBOS_OPEN_SETTINGS_BOTTOM=1` does the same for any Settings page (a Secret
            // page's People / Activity cards sit below the fold on the rig).
            machine_breakdown_open: false,
            machine_polling: false,
            settings_open_at_bottom: open_settings.as_deref() == Some("bottom")
                || std::env::var("ARBOS_OPEN_SETTINGS_BOTTOM").is_ok_and(|v| v == "1"),
            profile_open_at_bottom: std::env::var("ARBOS_PROFILE_SCROLL")
                .is_ok_and(|v| v == "bottom"),
            profile_edit_open: false,
            name_input,
            handle_input,
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
        // Rig: `ARBOS_RIG_TOGGLE_RAIL_MS=1500,3500` presses the sidebar toggle at those
        // times after launch (a recording of the collapse / open where the display
        // refuses synthetic clicks; the gate Mac, [feedback mac-sidebar-rail-2026-10-26]).
        if let Ok(spec) = std::env::var("ARBOS_RIG_TOGGLE_RAIL_MS") {
            for ms in spec.split(',').filter_map(|v| v.trim().parse::<u64>().ok()) {
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(ms))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        let width = this.last_viewport_width;
                        this.state.update(cx, |s, cx| {
                            let narrow = s.window_is_narrow(width);
                            s.toggle_rail(narrow, cx)
                        });
                    });
                })
                .detach();
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
            || focused(&self.chat_search_input)
            || focused(&self.viewer_search)
            || focused(&self.people_input)
            || focused(&self.server_input)
            || focused(&self.name_input)
            || focused(&self.handle_input)
            || focused(&self.agent_name_input)
            || focused(&self.description_input)
            || focused(&self.wake_url_input)
            || focused(&self.wake_auth_input)
            || focused(&self.avatar_prompt_input)
            || focused(&self.model_search_input)
            || focused(&self.tab_rename_input)
            || focused(&self.feedback.read(cx).note)
            || self.connectors.read(cx).input_focused(window, cx)
            || self.secrets.read(cx).input_focused(window, cx)
            || self.skills.read(cx).has_focus(window, cx)
            || (self.routines_open && self.routines.read(cx).has_focus(window, cx))
    }

    /// The focus handle a modal sheet owns while it is up (`ui::modal`): the
    /// Forward picker's search field (from a bubble or the media viewer), the
    /// viewer itself, the "Delete message?" card. Keys go there and nowhere
    /// else — `render` moves focus back every frame, so the composer behind a
    /// sheet never types (Jacob, feedback forward-sheet-focus-2026-10-24).
    pub(crate) fn modal_focus(&self, cx: &App) -> Option<FocusHandle> {
        if self.palette.open {
            return Some(self.search_input.read(cx).focus_handle(cx));
        }
        let state = self.state.read(cx);
        if state.forward_message.is_some() {
            return Some(self.viewer_search.read(cx).focus_handle(cx));
        }
        if let Some(viewer) = state.media_viewer.as_ref() {
            return Some(if viewer.forward_open {
                self.viewer_search.read(cx).focus_handle(cx)
            } else {
                self.viewer_focus.clone()
            });
        }
        if state.chart_viewer.is_some() {
            return Some(self.viewer_focus.clone());
        }
        None
    }

    /// Keep the keyboard inside the open sheet: whatever else took focus
    /// (the composer, a click, a stale handle) hands it back to the owner.
    fn enforce_modal_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(owner) = self.modal_focus(cx) {
            modal::keep_focus(&owner, window, cx);
        }
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

    // ----- Settings › Machine ([feedback machine-page-mac-2026-10-26]) ----------------

    /// Is the Machine page what the pane shows right now?
    fn machine_page_shown(&self, cx: &App) -> bool {
        self.settings_page == SettingsPage::Machine
            && self.state.read(cx).profile == Some(ProfilePage::Me)
    }

    /// Called from the Machine page's render: fetch now and keep fetching every
    /// `machine::POLL_MS` while the page is shown. The loop ends itself the first tick the page
    /// is gone (back, close, Escape, another page, the rig's launch state included).
    pub(crate) fn ensure_machine_poll(&mut self, cx: &mut Context<Self>) {
        if machine::poll_step(self.machine_polling, true) != machine::PollStep::Start {
            return;
        }
        self.machine_polling = true;
        // The first fetch happens inside the task (this is called from a render), then one
        // every `POLL_MS`.
        cx.spawn(async move |this, cx| loop {
            let keep = this.update(cx, |this, cx| {
                let on_page = this.machine_page_shown(cx);
                match machine::poll_step(this.machine_polling, on_page) {
                    machine::PollStep::Tick => {
                        this.state
                            .update(cx, |s, _| s.command(ClientCommand::MachineDebugRefresh));
                        true
                    }
                    machine::PollStep::Stop | machine::PollStep::Idle => {
                        this.machine_polling = false;
                        this.machine_breakdown_open = false;
                        false
                    }
                    machine::PollStep::Start => unreachable!("the loop is the poll"),
                }
            });
            if !keep.unwrap_or(false) {
                break;
            }
            cx.background_executor()
                .timer(std::time::Duration::from_millis(machine::POLL_MS))
                .await;
        })
        .detach();
    }

    pub(crate) fn toggle_machine_breakdown(&mut self, cx: &mut Context<Self>) {
        self.machine_breakdown_open = !self.machine_breakdown_open;
        cx.notify();
    }

    /// "Request a bigger machine": a request to Jacob through the feedback sheet, as the
    /// Credits page's top-up (a Daytona snapshot carries a fixed spec; the resize is manual).
    pub(crate) fn request_bigger_machine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let spec = self.state.read(cx).machine.spec.clone();
        self.open_feedback(window, cx);
        self.feedback.update(cx, |sheet, cx| {
            sheet.note.update(cx, |note, cx| {
                if note.text().trim().is_empty() {
                    let now = spec.map(|s| format!(" (now {s})")).unwrap_or_default();
                    note.set_text(format!("Bigger machine request{now}: "), cx);
                }
            });
        });
    }

    /// "Ask <agent> to free space": that agent's chat with the ask in the composer, ready to
    /// send (agentd's disk gate answers "free space" itself, #1641).
    pub(crate) fn ask_agent_to_free_space(
        &mut self,
        project_id: crate::model::Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            s.select_project(project_id, cx);
            s.close_profile(cx);
        });
        self.hide_settings_page(cx);
        self.composer.update(cx, |c, cx| {
            if c.input.read(cx).text().trim().is_empty() {
                c.input
                    .update(cx, |i, cx| i.set_text(machine::FREE_SPACE_ASK.into(), cx));
            }
        });
        Composer::focus(&self.composer, window, cx);
        cx.notify();
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
        // Jacob, 2026-10-25: the sheet's Upload / Generate pick travels with the create; the
        // stored leaf pair is the neutral one, never an automatic leaf.
        let picture = self.take_create_picture(cx);
        self.state.update(cx, |s, cx| {
            s.new_project_open = false;
            s.close_new_chat(cx);
            s.create_project(name, color, Some(create_entity::stored_leaf()), picture, cx);
        });
        Composer::focus(&self.composer, window, cx);
    }

    /// A Create sheet is opening: a fresh draft picture (Upload tab, no prompt).
    pub(crate) fn reset_create_picture(&mut self, cx: &mut Context<Self>) {
        self.create_picker = avatar_picker::Picker::new(avatar_picker::Target::Draft);
        self.avatar_prompt_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
    }

    /// The Create sheet's pending picture for the entity being created, and a fresh draft.
    pub(crate) fn take_create_picture(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<crate::sync::DraftPicture> {
        let picture = self.create_picker.draft_picture();
        self.reset_create_picture(cx);
        picture
    }

    /// The picker whose Generate / Upload actions and answers apply: the avatar picker sheet
    /// while it is open, else the Create sheet's draft (the two are never up together).
    pub(crate) fn pending_picker_mut(&mut self) -> Option<&mut avatar_picker::Picker> {
        Some(
            self.avatar_picker
                .as_mut()
                .unwrap_or(&mut self.create_picker),
        )
    }

    /// "New agent" in the New Message sheet: the Create New Agent screen with a fresh draft.
    pub(crate) fn open_create_agent(&mut self, cx: &mut Context<Self>) {
        self.reset_create_picture(cx);
        self.state.update(cx, |s, cx| {
            s.new_project_open = true;
            cx.notify();
        });
    }

    /// "Connect an agent" in the New Message sheet (#2141): the Create screen in its connect
    /// form (name + transport), then the key reveal.
    pub(crate) fn open_connect_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reset_create_picture(cx);
        self.create_group_open = false;
        self.connect_transport = create_entity::Transport::default();
        self.state.update(cx, |s, cx| {
            s.new_project_open = false;
            s.connect_open = true;
            cx.notify();
        });
        self.project_input.update(cx, |input, cx| {
            input.set_placeholder(CreateKind::Connect.placeholder());
            input.clear(cx);
        });
        let handle = self.project_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Open the Create group screen (step 1) over the New Message backdrop.
    pub(crate) fn open_create_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.create_group_open = true;
        self.reset_create_picture(cx);
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
            CreateKind::Connect => {
                let name = self.project_input.read(cx).text().trim().to_string();
                if name.is_empty() {
                    return;
                }
                let transport = self.connect_transport;
                self.project_input.update(cx, |input, cx| input.clear(cx));
                let color = self.new_agent_color.take();
                // The picture options are drawn but a connected agent starts with the neutral
                // initial; a picture can be set from its profile like any agent.
                let _ = self.take_create_picture(cx);
                self.state.update(cx, |s, cx| {
                    s.close_new_chat(cx);
                    s.connect_agent(name, transport.wire(), color, cx);
                });
                Composer::focus(&self.composer, window, cx);
            }
            CreateKind::Group => {
                let name = self.project_input.read(cx).text().trim().to_string();
                if name.is_empty() {
                    return;
                }
                let picture = self.take_create_picture(cx);
                self.create_group_open = false;
                self.project_input.update(cx, |input, cx| input.clear(cx));
                self.people_input.update(cx, |input, _| {
                    input.set_placeholder("Add people or agents");
                });
                self.state.update(cx, |s, cx| {
                    s.new_group_draft = Some(crate::app::GroupDraft {
                        title: name,
                        leaf: Some(create_entity::stored_leaf()),
                        picture,
                    });
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
                let picture = self.take_create_picture(cx);
                self.state.update(cx, |s, cx| {
                    s.create_server(name, Some(create_entity::stored_leaf()), picture, cx)
                });
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
            CreateKind::Connect => {
                self.state.update(cx, |s, _| s.connect_open = false);
                self.close_new_chat_sheet(window, cx);
            }
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
        let Some(pk) = self.pending_picker_mut() else {
            return;
        };
        if prompt.is_empty() || pk.generating {
            return;
        }
        let target = pk.target.clone();
        pk.generating = true;
        pk.started_at = Some(std::time::Instant::now());
        pk.error = None;
        pk.reset = false;
        self.state.update(cx, |s, cx| {
            s.generate_image(target, prompt, cx);
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
            let _ = this.update(cx, |this, cx| this.avatar_file_chosen(path, cx));
        })
        .detach();
    }

    /// The panel's pick lands in the open picker (the sheet's or a Create sheet's). The preview
    /// draws the mastered copy from the avatar cache (`file://` through the sync client), never
    /// the raw file through the GPU sampler.
    pub(crate) fn avatar_file_chosen(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.state.update(cx, |s, _| {
            s.want_avatar(avatar_picker::file_url(&path));
        });
        if let Some(pk) = self.pending_picker_mut() {
            pk.upload = Some(path);
            pk.reset = false;
            pk.error = None;
        }
        cx.notify();
    }

    /// "Set avatar": apply the sheet's pending choice and close it.
    pub(crate) fn set_avatar(&mut self, cx: &mut Context<Self>) {
        let Some(pk) = self.avatar_picker.take() else {
            return;
        };
        // The draft never reaches this sheet (`avatar_picker::sheet` draws nothing for it).
        let Some(id) = pk.target.id().cloned() else {
            return;
        };
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
                        avatar_picker::Tab::Generate => {
                            if let Some((key, _)) = pk.generated {
                                s.set_group_image_key(id, key, cx);
                            }
                        }
                    }
                }
            }
            avatar_picker::Target::Server(_) => {
                if pk.reset {
                    s.set_server_leaf(id, None, cx);
                } else {
                    match pk.tab {
                        avatar_picker::Tab::Bot => {
                            let current = s
                                .servers
                                .iter()
                                .find(|x| x.id == id)
                                .and_then(|x| {
                                    Some(LeafPick {
                                        leaf: x.avatar_leaf?,
                                        color: x.avatar_leaf_color?,
                                    })
                                })
                                .unwrap_or_else(|| leaf::pick_for(&id, None, None));
                            s.set_server_leaf(
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
                                s.set_server_image(id, PictureSource::File(path), cx);
                            }
                        }
                        avatar_picker::Tab::Generate => {
                            if let Some((key, _)) = pk.generated {
                                s.set_server_image(id, PictureSource::Key(key), cx);
                            }
                        }
                    }
                }
            }
            // My own picture: Reset is the GitHub picture again (`DELETE /me/avatar`); a file
            // goes up as today's `PUT /me/avatar`; a generated preview is set by its key.
            avatar_picker::Target::Me(_) => {
                if pk.reset {
                    s.remove_my_avatar(cx);
                } else {
                    match pk.tab {
                        avatar_picker::Tab::Upload => {
                            if let Some(path) = pk.upload {
                                s.set_my_avatar(path, cx);
                            }
                        }
                        avatar_picker::Tab::Generate => {
                            if let Some((key, _)) = pk.generated {
                                s.set_my_avatar_key(key, cx);
                            }
                        }
                        avatar_picker::Tab::Bot => {}
                    }
                }
            }
            avatar_picker::Target::Draft => {}
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
        self.reset_create_picture(cx);
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
        let picture = match sheet {
            ServerSheet::NewServer => self.take_create_picture(cx),
            ServerSheet::JoinServer | ServerSheet::NewChannel { .. } => None,
        };
        self.state.update(cx, |s, cx| match sheet {
            ServerSheet::NewServer => {
                s.create_server(text, Some(create_entity::stored_leaf()), picture, cx)
            }
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
        // The label ("Add a label") edits alone, in the hero; the name edits
        // alone too ([feedback agent-pane-details-tab-2026-10-24]).
        if description {
            self.description_input
                .update(cx, |input, cx| input.set_text(about, cx));
            self.state
                .update(cx, |s, cx| s.set_profile_editing_description(true, cx));
            let handle = self.description_input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        } else {
            self.state.update(cx, |s, cx| {
                s.profile_editing_name = true;
                cx.notify();
            });
            let handle = self.agent_name_input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
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

    /// A profile picture full-size in the media viewer (Jacob, 2026-10-25): the hero of a pane I
    /// cannot edit, or the picker's View. The picker stays up underneath — the viewer draws over
    /// it and occludes it — so its × / Esc / backdrop close only the viewer and the picker is
    /// back as it was (Jacob, 2026-10-26: the first cut dropped the picker here, so the X
    /// seemed to close both).
    /// Unblock asked for: the surface that holds `person` shows the confirm.
    pub(crate) fn arm_unblock(&mut self, person: Person, cx: &mut Context<Self>) {
        self.unblock_armed = Some(person);
        cx.notify();
    }

    pub(crate) fn cancel_unblock(&mut self, cx: &mut Context<Self>) {
        if self.unblock_armed.take().is_some() {
            cx.notify();
        }
    }

    /// The confirm's Unblock: `DELETE /blocks/{id}` through the state; the surfaces follow the
    /// relay's block list back to their plain state.
    pub(crate) fn confirm_unblock(&mut self, cx: &mut Context<Self>) {
        let Some(person) = self.unblock_armed.take() else {
            return;
        };
        self.state.update(cx, |s, cx| s.unblock_user(&person, cx));
        cx.notify();
    }

    /// The composer's stand-in for a DM with someone I blocked: the "You blocked X · Unblock"
    /// bar, or the confirm in its place while Unblock is being asked.
    fn blocked_composer(
        &self,
        person: &Person,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.unblock_armed_for(person) {
            let (cancel, go) = blocked::confirm_buttons("composer-unblock", theme);
            return div()
                .w_full()
                .rounded(px(COMPOSER_HEIGHT / 2.0))
                .bg(theme.surface_2)
                .child(blocked::confirm(
                    "composer-unblock",
                    &person.handle,
                    cancel.on_click(cx.listener(|this, _, _, cx| this.cancel_unblock(cx))),
                    go.on_click(cx.listener(|this, _, _, cx| this.confirm_unblock(cx))),
                    theme,
                ))
                .into_any_element();
        }
        let (bar, unblock) = blocked::composer_bar(person, theme);
        let person = person.clone();
        bar.child(unblock.on_click(cx.listener(move |this, _, _, cx| {
            this.arm_unblock(person.clone(), cx);
        })))
        .into_any_element()
    }

    /// Is the confirm up for this person?
    pub(crate) fn unblock_armed_for(&self, person: &Person) -> bool {
        self.unblock_armed.as_ref().is_some_and(|p| {
            (!p.id.is_empty() && p.id == person.id) || p.handle.eq_ignore_ascii_case(&person.handle)
        })
    }

    pub(crate) fn view_picture(&mut self, name: &str, url: &str, cx: &mut Context<Self>) {
        self.state
            .update(cx, |s, cx| s.open_picture_viewer(name, url, cx));
        cx.notify();
    }

    /// The crayon on my own picture: the agents' avatar picker (Generate | Upload, Reset) for
    /// my profile picture (Jacob, 2026-10-25); Set sends `PUT /me/avatar`.
    pub(crate) fn pick_my_avatar(&mut self, cx: &mut Context<Self>) {
        let Some(me) = self.state.read(cx).account.as_ref().map(|a| a.id.clone()) else {
            return;
        };
        self.open_avatar_picker(avatar_picker::Target::Me(me), cx);
    }

    /// Close the profile page (back button, click outside); the composer takes
    /// the keyboard back.
    /// The right pane's width for `page`: `profiles::SHEET_WIDTH`, wider for
    /// the two settings pages laid out past the card list (`sheet_width`).
    fn pane_width(&self, page: &ProfilePage) -> f32 {
        match page {
            ProfilePage::Me => profiles::SHEET_WIDTH.max(self.settings_page.sheet_width()),
            _ => profiles::SHEET_WIDTH,
        }
    }

    pub(crate) fn close_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_settings_page(cx);
        self.library_reset();
        self.profile_edit_open = false;
        self.member_remove_armed = None;
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
        self.reset_create_picture(cx);
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

    /// ⌘⇧F / ⌘K, or the sidebar's Search disc: the command palette opens
    /// centred over the window ([feedback search-palette-2026-10-24]). ⌘F is
    /// not bound here — Jacob keeps it for in-chat search, which does not
    /// exist yet.
    fn open_palette_action(
        &mut self,
        _: &OpenPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A toggle ([feedback search-toggle-click-outside-mac-2026-10-27],
        // Jacob 2026-10-27): ⌘⇧F on an open palette closes it, field focused or not.
        if self.palette.open {
            self.close_palette(window, cx);
        } else {
            self.open_palette(window, cx);
        }
    }

    pub(crate) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = false;
        self.close_popovers(cx);
        if !self.palette.open {
            self.search_input.update(cx, |i, cx| i.clear(cx));
            self.palette = palette::PaletteState {
                open: true,
                ..Default::default()
            };
            self.palette_scroll.set_offset(gpui::Point::default());
        }
        let handle = self.search_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Esc, a click outside, or an opened row: the palette goes away, its
    /// query is dropped and the composer takes the keyboard again.
    pub(crate) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.palette.open {
            return;
        }
        self.palette = palette::PaletteState::default();
        self.search_input.update(cx, |i, cx| i.clear(cx));
        Composer::focus(&self.composer, window, cx);
        cx.notify();
    }

    // ----- ⌘F Search-in-chat ([feedback search-in-chat-mac-2026-10-26]) ---------

    /// ⌘F: the bar drops under the header pill of the open chat and takes
    /// the keyboard; pressed again while it is up, the cursor goes back to
    /// its field (Find's convention — the key never closes the bar).
    fn open_chat_search_action(
        &mut self,
        _: &chat_search::Open,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A toggle ([feedback search-toggle-click-outside-mac-2026-10-27],
        // Jacob 2026-10-27): ⌘F on an open bar closes it like ✕ / Esc, field
        // focused or not.
        if self.state.read(cx).chat_search_open() {
            self.close_chat_search(window, cx);
        } else {
            self.open_chat_search(window, cx);
        }
    }

    /// Capture-phase mouse-down anywhere in the window: a click outside the
    /// open Search-in-chat bar closes it, a click outside the open palette's
    /// panel dismisses the palette — and the click goes on to whatever is
    /// under it (a message, a sidebar row, a button answers in the same
    /// click). Nothing here stops propagation.
    fn click_outside(
        &mut self,
        at: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inside = |b: Option<Bounds<Pixels>>| b.is_some_and(|b| b.contains(&at));
        if self.palette.open && !inside(self.palette_bounds.get()) {
            self.close_palette(window, cx);
        }
        if self.state.read(cx).chat_search_open() && !inside(self.chat_search_bounds.get()) {
            self.close_chat_search(window, cx);
        }
    }

    pub(crate) fn open_chat_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = false;
        self.close_popovers(cx);
        if self.palette.open {
            self.close_palette(window, cx);
        }
        let was_open = self.state.read(cx).chat_search_open();
        let opened = self.state.update(cx, |s, cx| s.open_chat_search(cx));
        if !opened {
            return;
        }
        if !was_open {
            self.chat_search_input.update(cx, |i, cx| i.clear(cx));
        }
        let handle = self.chat_search_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Esc / ✕: the bar goes, its marks with it; the composer types again.
    pub(crate) fn close_chat_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.close_chat_search(cx));
        self.chat_search_input.update(cx, |i, cx| i.clear(cx));
        Composer::focus(&self.composer, window, cx);
        cx.notify();
    }

    fn chat_search_close_action(
        &mut self,
        _: &chat_search::Close,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.read(cx).chat_search_open() {
            self.close_chat_search(window, cx);
        }
    }

    /// Enter / › / ⌘G.
    fn chat_search_next(&mut self, _: &chat_search::Next, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.read(cx).chat_search_open() {
            self.state.update(cx, |s, cx| s.chat_search_step(1, cx));
        }
    }

    /// ⇧Enter / ‹ / ⇧⌘G.
    fn chat_search_prev(&mut self, _: &chat_search::Prev, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.read(cx).chat_search_open() {
            self.state.update(cx, |s, cx| s.chat_search_step(-1, cx));
        }
    }

    /// ⇥ in the field: pin the sender the typed name offers.
    fn chat_search_pin_action(
        &mut self,
        _: &chat_search::PinSender,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(sender) = self.state.read(cx).chat_search_offer() {
            self.pin_chat_search_sender(sender, window, cx);
        }
    }

    pub(crate) fn pin_chat_search_sender(
        &mut self,
        sender: crate::chat_search::Sender,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state
            .update(cx, |s, cx| s.chat_search_pin_sender(sender, cx));
        self.chat_search_input.update(cx, |i, cx| i.clear(cx));
        let handle = self.chat_search_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// ↑ / ↓ in the palette field (`ComposerInput::on_arrow`).
    pub(crate) fn palette_step(&mut self, delta: i32, cx: &mut Context<Self>) {
        if !self.palette.open {
            return;
        }
        let len = self.palette.len;
        self.palette.cursor.step(delta, len);
        // The list scrolls the highlight into view on the next frame.
        self.palette.reveal = 2;
        cx.notify();
    }

    /// Return in the palette field: open the highlighted row.
    pub(crate) fn palette_open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.palette.open {
            return;
        }
        let typed = self.search_input.read(cx).text().to_string();
        let rows = palette::rows_for(self, &typed, cx);
        if let Some(row) = rows.get(self.palette.cursor.selected) {
            self.palette_open_target(row.target.clone(), window, cx);
        }
    }

    /// ⌘1–⌘9 while the palette is up: open that row.
    pub(crate) fn palette_pick_action(
        &mut self,
        action: &dyn gpui::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = palette::pick_index(action) else {
            return;
        };
        let typed = self.search_input.read(cx).text().to_string();
        let rows = palette::rows_for(self, &typed, cx);
        if let Some(row) = rows.get(index) {
            self.palette_open_target(row.target.clone(), window, cx);
        }
    }

    pub(crate) fn palette_open_target(
        &mut self,
        target: crate::palette::Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_palette(window, cx);
        self.hidden_open = false;
        match target {
            crate::palette::Target::Setting(setting) => self.open_setting(setting, window, cx),
            other => self
                .state
                .update(cx, |s, cx| s.open_palette_target(other, cx)),
        }
    }

    /// A Settings row picked in the palette ([feedback
    /// search-settings-mac-2026-10-26]): my pane straight on that page, the
    /// Secrets panel on its form, the feedback sheet, my Profile tab, or an
    /// owned agent's pane with the setting row dropped down.
    pub(crate) fn open_setting(
        &mut self,
        setting: crate::palette::SettingTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::palette::SettingTarget;
        match setting {
            SettingTarget::Page(page) => self.open_settings_page(page, window, cx),
            SettingTarget::SecretsNew => {
                self.open_settings_page(SettingsPage::Secrets, window, cx);
                self.secrets.update(cx, |p, cx| p.show_add(cx));
            }
            SettingTarget::SecretsConnect => {
                self.open_settings_page(SettingsPage::Secrets, window, cx);
                self.secrets.update(cx, |p, cx| p.show_connect(cx));
            }
            SettingTarget::Feedback => self.open_feedback(window, cx),
            SettingTarget::Profile => {
                self.hide_settings_page(cx);
                self.settings_page = SettingsPage::Root;
                self.state.update(cx, |s, cx| {
                    if s.profile != Some(ProfilePage::Me) {
                        s.open_profile(ProfilePage::Me, cx);
                    }
                    s.set_profile_tab(0, cx);
                });
                cx.notify();
            }
            SettingTarget::Agent { project_id, pick } => {
                self.hide_settings_page(cx);
                self.settings_page = SettingsPage::Root;
                self.state.update(cx, |s, cx| {
                    s.open_server = None;
                    s.select_project(project_id.clone(), cx);
                    s.open_profile(ProfilePage::Agent { project_id }, cx);
                    s.set_profile_tab(0, cx);
                    s.set_agent_pick(Some(pick), cx);
                });
                cx.notify();
            }
        }
    }

    /// Escape stops a recording memo (keeping its transcript), then closes
    /// whatever floats: menus, then sheets.
    fn escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.open {
            self.close_palette(window, cx);
            return;
        }
        if self.state.read(cx).forward_message.is_some() {
            self.state.update(cx, |s, cx| s.close_forward_message(cx));
            Composer::focus(&self.composer, window, cx);
            return;
        }
        if self.state.read(cx).chart_viewer.is_some() {
            self.state.update(cx, |s, cx| s.close_chart_viewer(cx));
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
                // Back to the avatar picker when the viewer was its View; else the composer.
                if self.avatar_picker.is_none() {
                    Composer::focus(&self.composer, window, cx);
                }
            }
            return;
        }
        if self.state.read(cx).chat_search_open() {
            self.close_chat_search(window, cx);
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
        // The Secrets forms' "Add agent" dropdown folds first; the form stays.
        if self.secrets.update(cx, |p, cx| p.close_agent_picker(cx)) {
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
            || self.tab_menu_open
            || self.row_menu.is_some()
            || self.chat_menu.is_some()
            || self.message_menu.is_some()
            || self.reaction_capsule.is_some()
            || self.state.read(cx).tasks_open.is_some();
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
        self.secrets.update(cx, |p, cx| {
            p.close_agent_picker(cx);
        });
        self.state.update(cx, |s, cx| s.close_tasks(cx));
        if self.menu_open
            || self.members_open
            || self.tab_menu_open
            || self.row_menu.is_some()
            || self.channel_menu.is_some()
            || self.chat_menu.is_some()
            || self.message_menu.is_some()
            || self.member_menu.is_some()
            || self.library.menu.is_some()
            || self.reaction_capsule.is_some()
        {
            self.menu_open = false;
            self.member_menu = None;
            self.library.menu = None;
            self.members_open = false;
            self.tab_menu_open = false;
            self.row_menu = None;
            self.channel_menu = None;
            self.channel_delete_armed = false;
            self.chat_menu = None;
            self.chat_delete_armed = false;
            self.message_menu = None;
            self.close_reaction_capsule(cx);
            cx.notify();
        }
    }

    // ----- update disc (sidebar footer, right end) --------------------------------

    /// Grok Bot's update button ([feedback footer-update-button-grok-2026-10-24]):
    /// a solid blue disc with the white cloud-download glyph at the footer's
    /// right end, the button family's size, drawn only when the update is
    /// ready for a click (`UpdateState::disc_action`). One click downloads,
    /// verifies, swaps the bundle and relaunches — or restarts now when the
    /// automatic path already installed it. Brighter under the pointer,
    /// brighter still while pressed (the chip rule, [feedback mention-chips]);
    /// pulsing and inert while it works. Replaces the "Update" pill (D42).
    pub(crate) fn render_update_disc(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let theme = Theme::of(cx).clone();
        let update = self.state.read(cx).update.clone();
        let action = update.disc_action();
        if action == crate::update::DiscAction::Hidden {
            return None;
        }
        let busy = action == crate::update::DiscAction::Busy;
        if busy {
            PulseClock::request(cx);
        }
        let fill = if busy {
            Theme::with_alpha(
                theme.update_disc,
                sidebar::DISC_BUSY_ALPHA_MIN
                    + (1.0 - sidebar::DISC_BUSY_ALPHA_MIN) * PulseClock::phase(cx, 1400),
            )
        } else {
            theme.update_disc
        };
        let glyph = theme.on_update_disc;
        Some(
            sidebar::footer_disc_frame("sidebar-update")
                .bg(fill)
                .when(!busy, |el| {
                    highlight::washed(el.cursor_pointer(), fill, &theme)
                })
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(sf_symbols::glyph(
                    "icloud.and.arrow.down",
                    sidebar::FOOTER_DISC_GLYPH,
                    glyph,
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !busy {
                        this.state.update(cx, |s, cx| s.install_update(cx));
                    }
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
            // The chat-level note (Jacob 2026-10-26): the shared error element in this slot
            // under the header, the column's width. "Can't reach your machine" with Grok's
            // reassurance as the sentence; Try again starts the machine; Details has the
            // relay's words.
            MachineState::Error => {
                let detail = machine
                    .detail
                    .clone()
                    .unwrap_or_else(|| "machine state: error".to_string());
                let error = UserError::of(
                    "",
                    ErrorClass::Refused {
                        message: Some(
                            "Can't reach your machine. Your agents are safe — it just can't be reached right now."
                                .into(),
                        ),
                    },
                    detail,
                );
                Some(
                    self.chat_note(
                        "machine-error",
                        &error,
                        self.machine_error_details_open,
                        &theme,
                        error_note::Handlers::new()
                            .try_again(cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |s, cx| s.machine_start(cx));
                            }))
                            .toggle_details(cx.listener(|this, _, _, cx| {
                                this.machine_error_details_open = !this.machine_error_details_open;
                                cx.notify();
                            })),
                    ),
                )
            }
            MachineState::Asleep
            | MachineState::Awake
            | MachineState::Stopped
            | MachineState::Unknown => None,
        }
    }

    /// A chat-level note in the slot under the header: the column's inset and width cap, 8 pt
    /// under the bar, 8 over what follows.
    fn chat_note(
        &self,
        id: &str,
        error: &UserError,
        details_open: bool,
        theme: &Theme,
        handlers: error_note::Handlers,
    ) -> Div {
        div()
            .w_full()
            .flex()
            .justify_center()
            .px(px(COLUMN_MARGIN))
            .pt(px(8.0))
            .pb(px(8.0))
            .child(
                div()
                    .w_full()
                    .max_w(px(COLUMN_MAX_WIDTH))
                    .child(error_note::render(
                        id,
                        error,
                        details_open,
                        error_note::Style::Inline,
                        theme,
                        handlers,
                    )),
            )
    }

    /// The per-room push refusal in the same slot, while the relay refuses; it clears itself
    /// on the room's next ACK (`SyncEvent::PushRefused { refused: false }`). A refusal with a
    /// `reason` says so in plain words ([feedback blocked-state-relay-2026-10-26]): "<name> has
    /// blocked you in this chat." / "Couldn't post: you've blocked <name>, who is in this chat.
    /// Unblock to post." / the mute line; anything else keeps "Messages in this chat aren't
    /// being delivered right now."
    fn render_delivery_note(&self, cx: &mut Context<Self>) -> Option<Div> {
        let theme = Theme::of(cx).clone();
        let state = self.state.read(cx);
        let chat = state.active_chat_id()?;
        let why = state.push_refused.get(&chat)?.clone();
        let error = delivery_error(&why);
        let mut handlers =
            error_note::Handlers::new().toggle_details(cx.listener(|this, _, _, cx| {
                this.delivery_error_details_open = !this.delivery_error_details_open;
                cx.notify();
            }));
        // "Unblock to post": the capsule arms the confirm for the person named
        // ([feedback blocked-state-mac-2026-10-26]); the DM's composer bar shows it.
        if let Some(handle) = why.unblock_handle() {
            let person = state.person_by_handle(handle).unwrap_or_else(|| Person {
                handle: handle.to_owned(),
                ..Person::default()
            });
            handlers = handlers.extra(
                blocked::UNBLOCK_LABEL,
                cx.listener(move |this, _, _, cx| this.arm_unblock(person.clone(), cx)),
            );
        }
        Some(self.chat_note(
            "delivery-error",
            &error,
            self.delivery_error_details_open,
            &theme,
            handlers,
        ))
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
        chat_slide: Option<(ChatSlide, f32)>,
    ) -> Stateful<Div> {
        let has_pre = !pre.is_empty();
        if has_pre {
            column = column.child(div().pt(px(header::metrics::HEIGHT)).children(pre));
        }
        // While a neighbouring pane slides, the transcript keeps the width it
        // had and is clipped/offset instead of re-measured per frame
        // (`ui::slide`); the header and composer follow the live width.
        let transcript = match chat_slide {
            Some((slide, live_width)) => slide::transcript_clip(slide, live_width, transcript),
            None => transcript,
        };
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
        // APuu1qHpbKmmpqZeUusEi0Q). The box is the bar plus the connection
        // capsule's overhang, so the capsule is drawn whole ([feedback
        // offline-chip-group-header-2026-10-27]); the bar itself stays
        // `HEIGHT` tall inside it.
        column.child(body).children(bottom).child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(header::metrics::OVERLAY_HEIGHT))
                .max_h(px(header::metrics::OVERLAY_HEIGHT))
                .overflow_hidden()
                .debug_selector(|| "chat-header-overlay".into())
                .child(header_bar),
        )
    }

    fn render_center(&self, geometry: CenterGeometry, cx: &mut Context<Self>) -> Stateful<Div> {
        let CenterGeometry {
            header_inset,
            show_sidebar_toggle,
            chat_slide,
        } = geometry;
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
        // A DM with someone I blocked has no input ([feedback blocked-state-mac-2026-10-26]):
        // "You blocked X · Unblock" stands in the composer's place, the confirm in its turn.
        let blocked_peer = project
            .as_ref()
            .filter(|p| p.is_people() && !p.is_people_group())
            .and_then(|p| p.person().cloned())
            .filter(|person| state.is_blocked(person));
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
        // Wheel over a short input (sibling of the list) still scrolls the
        // chat — same as dragging over a bubble ([feedback
        // app-01M5PDB3QYAPBPA4FJXZE1FFD0]). A tall overflowing field handles
        // the wheel itself and stops propagation so the transcript stays put
        // ([feedback app-01M5WNET9M9JY4D7W5YAZG2685]).
        let composer_slot: AnyElement = match &blocked_peer {
            Some(person) => self.blocked_composer(person, &theme, cx),
            None => self.composer.clone().into_any_element(),
        };
        let composer_block = column_row(
            div()
                .w_full()
                .debug_selector(|| "chat-composer-block".into())
                .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                    this.transcript
                        .update(cx, |t, cx| t.scroll_from_wheel(event, cx));
                }))
                .child(composer_slot)
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
                .when(header_inset > 0.0, |el| el.pl(px(header_inset)))
                .child(header::render_channel(
                    self,
                    &server,
                    &ch,
                    show_sidebar_toggle,
                    cx,
                ));
            // The Search-in-chat bar first, under the header; then the banner.
            let pre: Vec<AnyElement> = chat_search::bar(self, cx)
                .into_iter()
                .chain(
                    self.render_banner(cx)
                        .into_iter()
                        .chain(self.render_delivery_note(cx))
                        .map(IntoElement::into_any_element),
                )
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
                chat_slide,
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
            // clear them (14 pt in, three 12 pt lights, 8 pt gaps). While the
            // sidebar slides the inset is whatever of the lights' band the
            // sidebar no longer covers (`CenterGeometry`).
            .when(header_inset > 0.0, |el| el.pl(px(header_inset)))
            .child(header::render(self, &space, show_sidebar_toggle, cx));
        let floating_header = dropped;
        let mut pre: Vec<AnyElement> = Vec::new();
        // ⌘F: the Search-in-chat bar is the first thing under the header.
        if let Some(bar) = chat_search::bar(self, cx) {
            pre.push(bar);
        }
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
        if let Some(note) = self.render_delivery_note(cx) {
            pre.push(note.into_any_element());
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
                "Say hi. It lands for both of you at once.".to_string()
            } else {
                format!("Ask {name} anything. It plans, delegates, or just answers.")
            };
            let mut hero = div().flex().flex_col().items_center().gap(px(10.0));
            let (avatar_state, person_image) =
                hero_data.unwrap_or((avatars::AvatarState::Idle, None));
            let _ = avatar_state;
            // A DM shows the other person; everything else — an agent, a people group, a
            // group of agents — shows the space's own picture through `groups::picture`. A
            // people group used to fall into the DM arm (`space.person()` is just the first
            // member) and open on that member's disc instead of the group's picture
            // ([feedback group-row-avatar-mac-2026-10-27]).
            let dm_partner = space
                .person()
                .filter(|_| is_people && !space.is_people_group());
            if let Some(person) = dm_partner {
                hero = hero.child(people::avatar(person_image, person, 88.0, &theme));
            } else {
                // D92 amendment: the uploaded / generated picture wins over the leaf / initials.
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
                    .child(div().w(px(COMPOSER_WIDTH)).max_w_full().pt(px(12.0)).child(
                        match &blocked_peer {
                            Some(person) => self.blocked_composer(person, &theme, cx),
                            None => self.composer.clone().into_any_element(),
                        },
                    )),
            );
        } else {
            // Transcript above, composer pinned at the bottom. On the first
            // send the composer slides down from the centre over 220 ms using
            // a relative `top` inset (GPUI has no transform).
            // The tasks chip and drawer ride on the row while the agent has
            // background jobs open in this chat (`ui::tasks`, duplex §2.4).
            // The row floats over the transcript's bottom clearance instead of
            // sitting under it ([feedback status-row-gap-mac-2026-10-26]): the
            // last bubble keeps its 54 pt to the pill with or without the row
            // (Grok Mac), and the list is not resized when the row comes or
            // goes. Only while the list is pinned at its end — scrolled into
            // history, bubbles pass through that band (and the ↓ button sits
            // there), so the row steps aside.
            let pinned = self.transcript.read(cx).is_pinned_to_bottom();
            let presence_row = presence.filter(|_| pinned).map(|p| {
                let row = match chat_id.clone() {
                    Some(id) => {
                        let (jobs, open) = crate::ui::tasks::for_chat(self.state.read(cx), &id);
                        crate::ui::tasks::presence_row(
                            p,
                            &self.state,
                            id,
                            jobs,
                            open,
                            &self.tasks_chip,
                            self.tasks_link,
                            &theme,
                            cx,
                        )
                    }
                    None => crate::ui::presence::row(p, Some(&self.state), &theme, cx),
                };
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(crate::ui::presence::LIFT))
                    .debug_selector(|| "presence-float".into())
                    // Wheel over the row scrolls the chat, like over the pill.
                    .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                        this.transcript
                            .update(cx, |t, cx| t.scroll_from_wheel(event, cx));
                    }))
                    .child(column_row(row.into_any_element()))
            });
            let composer_row = div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .pb(px(COMPOSER_BOTTOM))
                .pt(px(6.0))
                // A zero-height anchor at the composer block's top edge: the
                // floating row hangs off it and adds nothing to the column.
                .child(div().relative().w_full().h(px(0.0)).children(presence_row))
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
                chat_slide,
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
                        // The sign-in note: under the button row and the hint, 420 wide —
                        // the shared error element in its one spot on this screen.
                        el.child(
                            div().w(px(420.0)).max_w_full().child(error_note::render(
                                "sign-in-error",
                                &err,
                                pairing.error_details_open,
                                error_note::Style::Inline,
                                &theme,
                                error_note::Handlers::new()
                                    .try_again(cx.listener(|this, _, _, cx| {
                                        this.state.update(cx, |s, cx| s.sign_in(cx));
                                    }))
                                    .toggle_details(cx.listener(|this, _, _, cx| {
                                        this.state.update(cx, |s, cx| {
                                            s.toggle_sign_in_details();
                                            cx.notify();
                                        });
                                    })),
                            )),
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
                    .border_color(theme.field_border)
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
                        el.child(div().w(px(420.0)).max_w_full().child(error_note::render(
                            "sign-in-error",
                            &err,
                            pairing.error_details_open,
                            error_note::Style::Inline,
                            &theme,
                            error_note::Handlers::new()
                                .try_again(cx.listener(|this, _, _, cx| this.submit_pairing(cx)))
                                .toggle_details(cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |s, cx| {
                                        s.toggle_sign_in_details();
                                        cx.notify();
                                    });
                                })),
                        )))
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
                            .child(sf_symbols::glyph("bell", 17.0, theme.text)),
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

    /// The one toast slot (Jacob 2026-10-27, [feedback toast-stack-mac-2026-10-27]): just
    /// below the header's profile-picture pill, centred on the chat column (between the
    /// sidebar and the pane), the newest `NOTICE_VISIBLE_MAX` toasts stacked compactly, older
    /// ones folded into one "N more" line with a × that clears the slot. Info notices are the
    /// plain card (click dismisses); failures are the shared error element as a toast — ×,
    /// Details, Try again, 10 s, the pointer on it holds it, the same failure again bumps its
    /// "×N" instead of adding a toast ([feedback error-messages-mac-2026-10-26]).
    fn render_notices(&self, left: f32, right: f32, cx: &mut Context<Self>) -> Option<Div> {
        let theme = Theme::of(cx).clone();
        let stack = self.state.read(cx).notice_stack();
        if stack.visible.is_empty() {
            return None;
        }
        PulseClock::request(cx);
        let mut column = div()
            .id("toast-slot")
            .debug_selector(|| "toast-slot".into())
            .w(px(error_note::TOAST_WIDTH))
            .flex()
            .flex_col()
            .gap(px(6.0));
        if stack.folded > 0 {
            let folded = stack.folded;
            column = column.child(
                div()
                    .id("toast-folded")
                    .debug_selector(|| "toast-folded".into())
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(12.0))
                    .h(px(24.0))
                    .rounded_full()
                    .bg(theme.surface_2)
                    .border_1()
                    .border_color(theme.border_strong)
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_muted)
                    .child(format!(
                        "{folded} more {}",
                        if folded == 1 { "notice" } else { "notices" }
                    ))
                    .child(
                        div()
                            .id("toast-folded-close")
                            .debug_selector(|| "toast-folded-close".into())
                            .size(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.7))
                            .child(sf_symbols::glyph("xmark", 12.0, theme.text_muted))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |s, cx| {
                                    s.dismiss_all_notices();
                                    cx.notify();
                                });
                            })),
                    ),
            );
        }
        for n in stack.visible {
            let nid = n.id;
            let id = format!("notice-{nid}");
            let element: gpui::AnyElement = match &n.error {
                Some(error) => {
                    let mut handlers = error_note::Handlers::new()
                        .count(n.count)
                        .toggle_details(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |s, cx| {
                                s.toggle_notice_details(nid);
                                cx.notify();
                            });
                        }))
                        .dismiss(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |s, cx| {
                                s.dismiss_notice(nid);
                                cx.notify();
                            });
                        }));
                    if n.retry.is_some() {
                        handlers = handlers.try_again(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |s, cx| {
                                s.retry_notice(nid);
                                cx.notify();
                            });
                        }));
                    }
                    if error.action() == crate::sync::user_error::Action::SignIn {
                        handlers = handlers.sign_in(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |s, cx| {
                                s.dismiss_notice(nid);
                                s.sign_in(cx);
                            });
                        }));
                    }
                    error_note::render(
                        &id,
                        error,
                        n.details_open,
                        error_note::Style::Toast,
                        &theme,
                        handlers,
                    )
                    .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                        let hovering = *hovering;
                        this.state.update(cx, |s, _| s.hold_notice(nid, hovering));
                    }))
                    .into_any_element()
                }
                None => div()
                    .id(SharedString::from(id.clone()))
                    .w_full()
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
                    .child(if n.count >= 2 {
                        format!("{} (×{})", n.text, n.count)
                    } else {
                        n.text.clone()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |s, cx| {
                            s.dismiss_notice(nid);
                            cx.notify();
                        });
                    }))
                    .into_any_element(),
            };
            column = column.child(element);
        }
        Some(
            div()
                .absolute()
                .left(px(left))
                .right(px(right))
                .top(px(header::metrics::TOAST_SLOT_TOP))
                .flex()
                .justify_center()
                .child(column),
        )
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

fn cards_spinner(theme: &Theme, cx: &mut App) -> Div {
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
        let height: f32 = window.viewport_size().height.into();
        self.last_viewport_width = width;
        self.fullscreen = window.is_fullscreen() || self.rig_fullscreen;
        crate::ui::picture_fit::note_scale_factor(window.scale_factor());
        let state = self.state.read(cx);
        let narrow = state.window_is_narrow(width);
        // The narrow band hides the sidebar (an overlay brings it back);
        // otherwise the toggle collapses it to the icon rail, never away
        // ([feedback mac-sidebar-rail-2026-10-26]).
        let sidebar_hidden = narrow;
        let rail = state.rail_collapsed && !narrow;
        let sidebar_overlay = sidebar_hidden && state.sidebar_overlay;
        let sidebar_width = state.sidebar_width;
        let details = state.details_open.clone();
        let viewer_open = state.media_viewer.is_some();
        let chart_viewer_open = state.chart_viewer.is_some();
        let chart_viewer_generation = state.chart_viewer_generation;
        let viewer_forward_open = state.media_viewer.as_ref().is_some_and(|v| v.forward_open);
        let viewer_generation = state.media_viewer_generation;
        let forward_message_open = state.forward_message.as_ref().map(|f| f.message_id.clone());
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
            .on_action(cx.listener(Self::open_palette_action))
            .on_action(cx.listener(Self::open_chat_search_action))
            .on_action(cx.listener(Self::chat_search_next))
            .on_action(cx.listener(Self::chat_search_prev))
            .on_action(cx.listener(Self::chat_search_close_action))
            .on_action(cx.listener(Self::chat_search_pin_action))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::send_feedback))
            .on_action(cx.listener(Self::paste_into_chat))
            .capture_any_mouse_down(cx.listener(
                |this, event: &gpui::MouseDownEvent, window, cx| {
                    if event.button == gpui::MouseButton::Left {
                        this.click_outside(event.position, window, cx);
                    }
                },
            ))
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

        // The pane slides ([feedback right-pane-slide-2026-10-24], [feedback
        // sidebar-slide-2026-10-24]): the tracker turns this frame's logical
        // state into visible widths; while one runs, every frame is redrawn.
        let (profile, opened) = {
            let state = self.state.read(cx);
            (state.profile.clone(), state.profile_opened)
        };
        let pane_width = profile
            .as_ref()
            .or(self.pane_page_drawn.as_ref())
            .map_or(profiles::SHEET_WIDTH, |page| self.pane_width(page));
        let frame = self.slides.step(
            slide::Want {
                viewport_width: width,
                sidebar_shown: !sidebar_hidden,
                sidebar_width,
                sidebar_rail: rail,
                narrow,
                pane_shown: profile.is_some(),
                pane_width,
                reduce_motion: cx.reduce_motion(),
            },
            std::time::Instant::now(),
        );
        let frame = match self.rig_rail_t {
            Some(t) if !sidebar_hidden => slide::Frame {
                sidebar_visible: sidebar_width - t * (sidebar_width - rail::metrics::WIDTH),
                sidebar_sliding: true,
                ..frame
            },
            _ => frame,
        };
        if frame.animating() && self.rig_rail_t.is_none() {
            window.request_animation_frame();
        }
        let live_center = width - frame.sidebar_visible - frame.pane_visible;
        // The header clears whatever of the traffic lights' band the sidebar
        // does not cover, and shows the expand disc while that band is free:
        // at rest that is 72 + the disc with the sidebar hidden, nothing with
        // it docked; mid-slide the disc holds its spot (72 + 12) from the
        // moment the sidebar's own disc has left until the sidebar is back.
        // With the rail the toggle is the rail's own (the morph's overlay
        // carries it in between), so the header draws one only for the
        // narrow band's hidden sidebar, as before
        // ([feedback sidebar-toggle-in-rail-2026-10-27]).
        let rail_progress = frame.rail_progress(sidebar_width);
        let show_sidebar_toggle = rail::header_toggle_shown(sidebar_hidden, frame.sidebar_visible);
        let geometry = CenterGeometry {
            header_inset: (TRAFFIC_LIGHTS_INSET - frame.sidebar_visible).max(0.0),
            show_sidebar_toggle,
            chat_slide: frame.chat.map(|c| (c, live_center)),
        };

        let mut morph_overlay = None;
        match (frame.sidebar_sliding, rail_progress) {
            // Collapsing to / opening from the rail: the column morphs and the
            // moving pieces travel over the window.
            (true, Some(t)) => {
                root = root.child(frosted(
                    0.0,
                    PANEL_BLUR,
                    rail::render_morph(self, t, frame.sidebar_visible, cx),
                ));
                morph_overlay = Some(rail::render_overlay(self, t, height, cx));
            }
            // Coming back from hidden (the window widened past the narrow
            // band): the clip reveals whichever layout the state wants.
            (true, None) if rail => {
                root = root.child(slide::sidebar_clip(
                    frame.sidebar_visible,
                    rail::metrics::WIDTH,
                    frosted(0.0, PANEL_BLUR, rail::render(self, cx)),
                ));
            }
            (true, None) => {
                root = root.child(slide::sidebar_clip(
                    frame.sidebar_visible,
                    sidebar_width,
                    frosted(0.0, PANEL_BLUR, sidebar::render(self, cx)),
                ));
            }
            (false, _) if sidebar_hidden => {}
            (false, _) if rail => {
                root = root.child(frosted(0.0, PANEL_BLUR, rail::render(self, cx)));
            }
            (false, _) => {
                root = root.child(frosted(0.0, PANEL_BLUR, sidebar::render(self, cx)));
            }
        }
        if !rail && self.rail_hover.is_some() {
            self.rail_hover = None;
        }
        if !sidebar_hidden && self.state.read(cx).sidebar_overlay {
            // Widened back past the band with the overlay up: it has docked.
            self.state.update(cx, |s, _| s.sidebar_overlay = false);
        }
        root = root.child(self.render_center(geometry, cx));
        if sidebar_overlay {
            root = root.child(self.render_sidebar_overlay(&theme, cx));
        }
        if let Some(target) = details {
            root = root.child(self.render_details(&target, cx));
        }
        // The right pane (three-pane rule): a flex sibling beside the chat, so
        // it never covers it; the floating menus below paint over both. A
        // pane on its way out keeps drawing the page it showed.
        let page_to_draw = profile.clone().or_else(|| {
            frame
                .pane_sliding
                .then(|| self.pane_page_drawn.clone())
                .flatten()
        });
        if profile.is_some() {
            self.pane_page_drawn = profile.clone();
        }
        if let Some(page) = page_to_draw {
            let opened = if profile.is_some() {
                opened
            } else {
                self.profile_scroll_epoch
            };
            // A page that just filled the pane starts at the top; a popped
            // one already carries its restored offset (see `back_profile`).
            if self.profile_scroll_epoch != opened {
                self.profile_scroll_epoch = opened;
                self.profile_scroll.set_offset(gpui::Point::default());
                if page == ProfilePage::Me && self.settings_open_at_bottom {
                    self.profile_scroll.scroll_to_bottom();
                }
                if matches!(page, ProfilePage::Agent { .. }) && self.profile_open_at_bottom {
                    self.profile_scroll.scroll_to_bottom();
                }
            } else if matches!(page, ProfilePage::Agent { .. }) && self.profile_open_at_bottom {
                // Rig only: the pane's rows arrive after it opened (grants, routines); keep
                // the end in view.
                self.profile_scroll.scroll_to_bottom();
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
            if page == ProfilePage::Me && self.settings_page == SettingsPage::Machine {
                self.ensure_machine_poll(cx);
            }
            let pane_width = self.pane_width(&page);
            match (profiles::pane(&page, self, cx), frame.pane_sliding) {
                (Some(pane), true) => {
                    root = root.child(slide::pane_clip(frame.pane_visible, pane));
                }
                (Some(pane), false) => root = root.child(pane),
                (None, true) => {
                    // The page's subject is gone mid-slide: an empty pane of
                    // the same width finishes the motion.
                    let blank = div()
                        .id("profile-pane")
                        .w(px(pane_width))
                        .flex_shrink_0()
                        .h_full()
                        .border_l(px(profiles::PANE_EDGE))
                        .border_color(theme.pane_edge)
                        .bg(theme.pane());
                    root = root.child(slide::pane_clip(frame.pane_visible, blank));
                }
                (None, false) => {}
            }
        }
        // The rail morph's travelling pieces and the hovered picture's name
        // tip sit over the panes, under the floating menus.
        if let Some(overlay) = morph_overlay {
            root = root.child(overlay);
        }
        if rail && !frame.sidebar_sliding {
            if let Some(tip) = rail::render_tip(self, cx) {
                root = root.child(tip);
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
            // `ARBOS_RIG_MESSAGE_MENU_FROM=someone` picks the newest message by another
            // person instead (the Reply in DM item only shows on those, #1835).
            let someone_else =
                std::env::var("ARBOS_RIG_MESSAGE_MENU_FROM").is_ok_and(|v| v == "someone");
            let target = self.state.read(cx).active_chat_id().and_then(|chat| {
                let state = self.state.read(cx);
                let me = format!("user:{}", state.me_id());
                let rows = state.messages.get(&chat)?;
                let pick = if someone_else {
                    rows.iter()
                        .rev()
                        .find(|m| m.sender.starts_with("user:") && m.sender != me)?
                } else {
                    rows.last()?
                };
                Some((chat, pick.id.clone()))
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
        if let Some(menu) = crate::ui::profiles::library::menu(self, window, cx) {
            root = root.child(menu);
        }
        if let Some(space) = people_space {
            let right = 16.0;
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
            } else if self.state.read(cx).connect_open {
                root = root.child(create_entity::sheet(self, CreateKind::Connect, cx));
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
        if let Some(reveal) = self.state.read(cx).connect_reveal.clone() {
            root = root.child(connect_agent::reveal_sheet(self, &reveal, cx));
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
            // Inline video: keep the native AVPlayerView over the picture area
            // (or tear it down when Play has not started / we paged away).
            let playing = self
                .state
                .read(cx)
                .media_viewer
                .as_ref()
                .and_then(|v| v.playing.clone());
            crate::ui::video_player::sync(window, playing.as_deref());
            root = root.child(media_viewer::overlay(self, &self.viewer_focus, cx));
        } else {
            crate::ui::video_player::sync(window, None);
            if chart_viewer_open {
                // Esc closes; focus moves into the overlay once per opening.
                if self.chart_viewer_focused_generation != chart_viewer_generation {
                    self.chart_viewer_focused_generation = chart_viewer_generation;
                    window.focus(&self.viewer_focus, cx);
                }
                root = root.child(chart_viewer::overlay(self, &self.viewer_focus, cx));
            } else if self.state.read(cx).forward_message.is_none()
                && (self.viewer_focus.is_focused(window)
                    || self
                        .viewer_search
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window))
            {
                // Closed by × / the backdrop / Forward: typing goes back to the
                // composer (unless the bubble menu's picker owns the field now).
                Composer::focus(&self.composer, window, cx);
            }
        }
        // The command palette: the topmost modal (over sheets and the viewer).
        let palette_scroll = self.palette_scroll.clone();
        if let Some(overlay) = palette::overlay(self, &palette_scroll, window, cx) {
            root = root.child(overlay);
        }
        self.enforce_modal_focus(window, cx);
        // Rig: the Search-in-chat bar with its query, once a chat with rows is up.
        if self.chat_search_rig.is_some() {
            // With `ARBOS_OPEN_SPACE` the bar waits for that space's chat, not
            // the first chat that happens to have rows.
            let wanted = std::env::var("ARBOS_OPEN_SPACE").ok();
            let state = self.state.read(cx);
            let ready = state.active_chat_id().is_some_and(|id| {
                !state.messages_of(&id).is_empty()
                    && wanted
                        .as_ref()
                        .is_none_or(|space| state.space_of_chat(&id) == *space)
            });
            if ready {
                if let Some(query) = self.chat_search_rig.take() {
                    self.open_chat_search(window, cx);
                    self.chat_search_input
                        .update(cx, |i, cx| i.set_text(query, cx));
                }
            }
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
        if let Some(notices) = self.render_notices(frame.sidebar_visible, frame.pane_visible, cx) {
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

/// The chat-level note's error for a push refusal: the refusal's sentence as the relay's
/// words (`ErrorClass::Refused`), the raw code / reason behind Details.
pub(crate) fn delivery_error(why: &crate::sync::PushRefusal) -> UserError {
    let raw = match (&why.reason, &why.by_handle) {
        (Some(reason), Some(handle)) => {
            format!(
                "the relay refused the push: {} ({reason}, @{handle})",
                why.code
            )
        }
        _ => format!("the relay refused the push: {}", why.code),
    };
    let error = UserError::of(
        "",
        ErrorClass::Offline {
            device_offline: false,
        },
        raw,
    );
    UserError {
        verb: String::new(),
        class: ErrorClass::Refused {
            message: Some(why.sentence()),
        },
        ..error
    }
}
