//! Root view: the window's grid, the state the chrome owns, and the frame
//! the tab bar, the chat column and the right-hand panel are hung in.

use crate::{
    kernel,
    model::{
        permission_center::{PermissionCenter, Permissions},
        session::ChatSession,
        settings::Settings,
        state::{self, State},
        workspace::{PaneRequest, Reloaded, Workspace},
    },
    view::{
        component::{
            chat_search::{ChatSearch, ChatSearchEvent, Hit},
            composer::{Composer, ComposerEvent, VoiceState},
            menu::Menu,
            meter,
            opener::{Opener, OpenerEvent},
            permissions_sheet::{PermissionsSheet, PermissionsSheetEvent},
            tab_sheet::{TabSheet, TabSheetEvent},
        },
        naming::Renaming,
        settings::{self, Section, SettingsWindow},
    },
};
use anyhow::Result;
use bezel::{
    gpui::{
        self, AnyElement, App, Bounds, Context, Entity, FocusHandle, Focusable as _, Hsla,
        KeyBinding, PathPromptOptions, PromptLevel, Render, Task, TitlebarOptions, Window,
        WindowBounds, WindowHandle, WindowOptions, actions, div, point, prelude::*, px, size,
    },
    motion::{Fade, Painter},
    theme::{Material, TextStyle, Theme, Typeset, appearance},
    ui::{
        floating::Floating,
        icons,
        input::{FieldEvent, TextField},
        menu::Cursor,
        stats::Stats,
        widgets::{ButtonStyle, Buttons, Content},
    },
};
use std::time::{Duration, Instant};
actions!(
    arbos,
    [
        NewSession,
        NewTab,
        OpenProject,
        CloseProject,
        NextTab,
        PrevTab,
        OpenSettings,
        ShowPermissions,
        TogglePanel,
        ShowChat,
        ShowProject,
        SearchChats,
        CommitName,
        DismissName,
        DismissMenu,
        NextEntry,
        PrevEntry,
        CopySelection,
        CopyChat,
        PasteChat,
        DeleteChat,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        StartCall,
        EndCall,
        ToggleMute
    ]
);

/// Attach files to the composer without the system's file dialog: what a
/// drop does, as an action, so the driver (and a keymap) can do it too.
#[derive(Clone, PartialEq, serde::Deserialize, schemars::JsonSchema, gpui::Action)]
#[action(namespace = arbos)]
pub struct AttachPaths {
    pub paths: Vec<String>,
}

/// Claimed on the window's rest focus so Delete/Backspace archive the
/// highlighted chat when no field is in front.
const WINDOW_CONTEXT: &str = "ArbosWindow";

/// Claimed on the rename field so `enter` files the name and `escape` drops it.
const RENAME_CONTEXT: &str = "ArbosSessionName";

fn name_field_entity(heading: bool, cx: &mut Context<Arbos>) -> Entity<TextField> {
    cx.new(|cx| {
        let field = TextField::new(cx)
            .with_frame(false)
            .with_key_context(RENAME_CONTEXT)
            .with_placeholder("name this session…");
        if heading {
            field.with_metrics(TextStyle::Title3.into())
        } else {
            field
        }
    })
}

/// How thick each column's material sits. Nothing paints beneath them, so these
/// are absolute and independent: the tab bar and the panel are chrome and hold
/// no long-form text, the chat is the column whose text has to win against
/// the desktop.
const CHROME_MATERIAL: Material = Material::Thick;
const CONTENT_MATERIAL: Material = Material::UltraThick;

/// The header strip's height, measured off `../desktop`: between Cursor's 34
/// and Notion's 36, and tall enough to hold the 14px traffic lights macOS 26
/// draws without crowding them.
pub(crate) const HEADER_HEIGHT: f32 = 36.;

/// Resting height of the composer card: one idle line, the tool row, and
/// the card's own padding. The field Grows past this on wrap / Shift-Enter.
/// Transcript scroll uses this so the last turn is not buried under the bar.
pub(crate) fn composer_height() -> f32 {
    // One row: the 28px controls set the height; a one-line field sits
    // centred on them. Cursor's follow-up pill is 40px tall.
    COMPOSER_HIT + COMPOSER_PAD_TOP + COMPOSER_PAD_BOTTOM
}

/// Corner radius of the pill. Cursor's is a third of its height.
pub(crate) const COMPOSER_RADIUS: f32 = 14.;

/// Lines the field claims before it has to grow. One matches Cursor's
/// follow-up composer; wrap and Shift-Enter still add lines.
pub(crate) const COMPOSER_FIELD_MIN: f32 = 1.;

/// Padding inside the card.
pub(crate) const COMPOSER_PAD_TOP: f32 = 6.;
pub(crate) const COMPOSER_PAD_BOTTOM: f32 = 6.;
pub(crate) const COMPOSER_PAD_X: f32 = 8.;

/// Transcript and composer share this reading column. Web: `max-w-4xl`
/// on both `transcript-col` and `composer-col`.
pub(crate) const CHAT_MAX_WIDTH: f32 = 720.;
/// Cursor's chat prose: 14 px on a 23 px line (measured 14/22 on Jacob's
/// Mac, 15/25 on the Linux build; the Mac is the reference).
pub(crate) const CURSOR_PROSE_SIZE: f32 = 14.;
/// How often the working tree is re-read for the Changes pill.
const CHANGES_POLL: Duration = Duration::from_secs(4);
pub(crate) const CURSOR_PROSE_LEADING: f32 = 23.;
/// Web `px-3.5`. Cards bleed by `COMPOSER_PAD_X` so their words sit on
/// this edge, same as the answer.
pub(crate) const CHAT_GUTTER: f32 = 14.;

/// Hit target for every control on the tool row — model, files, voice, send.
pub(crate) const COMPOSER_HIT: f32 = 28.;

/// How far the floating composer stands off the column's bottom edge.
pub(crate) const COMPOSER_BOTTOM: f32 = 12.;

/// The chrome's fill — the tab bar and the panel. Opaque, it takes the
/// chrome tone: the light palette's `surface` is the grey the content
/// plane's white sits inside, and falling back to the panel would leave
/// the columns one flat sheet.
pub(crate) fn chrome_bg(theme: &Theme) -> Hsla {
    material(theme, CHROME_MATERIAL).unwrap_or(theme.surface)
}

/// The content column's fill.
pub(crate) fn content_bg(theme: &Theme) -> Hsla {
    material(theme, CONTENT_MATERIAL).unwrap_or(theme.bg)
}

/// A column's own tint at one thickness on the material ladder, or nothing
/// where the window shows no desktop to sit over. The ladder's tone is a
/// neutral scrim and carries no appearance — tinting it is what makes dark
/// glass dark.
fn material(theme: &Theme, thickness: Material) -> Option<Hsla> {
    theme.vibrancy.then(|| Hsla {
        a: thickness.opacity(),
        ..theme.vibrancy_tint()
    })
}

/// macOS traffic light diameter — AppKit owns the buttons and reports their
/// frame, so nothing here can derive it. Measured on macOS 26.
const TRAFFIC_LIGHT_SIZE: f32 = 14.;

/// Where the traffic lights go, for `TitlebarOptions::traffic_light_position`:
/// AppKit's own inset across, which is where every other window on the desktop
/// shows them, and down by half the band the header reserves for them. macOS
/// sizes the button container to `height + 2y`.
pub const TRAFFIC_LIGHT_X: f32 = 12.;
pub const TRAFFIC_LIGHT_Y: f32 = (HEADER_HEIGHT - TRAFFIC_LIGHT_SIZE) / 2.;

/// Between the lights' centres, as AppKit lays them out. Measured on macOS 26.
const TRAFFIC_LIGHT_SPACING: f32 = 23.;

/// The gap the header keeps at the window's edges, and between the lights and
/// the first control it puts past them.
pub(crate) const HEADER_INSET: f32 = 16.;

/// Where the toolbar's own controls start: clear of the three lights AppKit
/// puts down from [`TRAFFIC_LIGHT_X`], plus the gutter that clears them and the
/// strip's own inset, so the first control stands off the lights by the same
/// measure it keeps from every other edge.
pub(crate) const TOOLBAR_INSET: f32 = if cfg!(target_os = "macos") {
    TRAFFIC_LIGHT_X + 2. * TRAFFIC_LIGHT_SPACING + TRAFFIC_LIGHT_SIZE + 6. + HEADER_INSET
} else {
    HEADER_INSET
};

pub fn init(cx: &mut App) {
    crate::view::terminal::init(cx);
    crate::view::component::permissions_sheet::init(cx);
    // Cursor's chat measure, taken off its screens: prose one step above
    // the UI ladder — 14 on 23 against the ladder's 13 — and fenced code
    // as a bare plate with the copy control on hover, no language band.
    {
        let base = markdown::Typography::default();
        let scale = CURSOR_PROSE_SIZE / bezel::theme::TextStyle::Body.size();
        markdown::set_typography(
            cx,
            markdown::Typography {
                body: bezel::theme::Metrics::new(
                    bezel::theme::TextStyle::Body,
                    CURSOR_PROSE_LEADING / CURSOR_PROSE_SIZE,
                    bezel::gpui::FontWeight::NORMAL,
                )
                .scaled(scale),
                ..base
            },
        );
        markdown::set_code_band(cx, false);
    }
    cx.bind_keys([
        // A sub-chat under the project's main chat. The project has one
        // main chat, so this never makes a second root.
        KeyBinding::new("cmd-n", NewSession, None),
        // A tab is a project; both chords open the same machine-then-folder
        // picker. ⌘T is the browser's word for it, ⌘O the Mac's.
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-o", OpenProject, None),
        // ⌘W closes the tab in front, as in a browser; the window itself
        // closes on ⇧⌘W — see `menubar`.
        KeyBinding::new("cmd-w", CloseProject, None),
        // Browser tab cycling: ⇧⌘] and ⇧⌘[. macOS names the key `]` with
        // shift held, and that is what the menu draws; Linux reports the
        // shifted glyph itself and drops the shift, so `}` is the same
        // chord there.
        KeyBinding::new("cmd-shift-]", NextTab, None),
        KeyBinding::new("cmd-shift-[", PrevTab, None),
        KeyBinding::new("cmd-}", NextTab, None),
        KeyBinding::new("cmd-{", PrevTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PrevTab, None),
        // What macOS binds Preferences to in every other app.
        KeyBinding::new("cmd-,", OpenSettings, None),
        // What every app with a side panel binds it to. It is claimed
        // app-wide: the menu item carries it, so AppKit takes the chord
        // before the window is offered it, and the editor's own `cmd-b` —
        // bold — is not reached while this one is on the bar.
        KeyBinding::new("cmd-b", TogglePanel, None),
        // Call the project in front: a full-duplex conversation with its
        // main agent through the speech server. ⇧⌘C again hangs up.
        KeyBinding::new("cmd-shift-c", StartCall, None),
        KeyBinding::new("cmd-shift-m", ToggleMute, None),
        KeyBinding::new("cmd-1", ShowChat, None),
        // Cursor's Project tab sits beside the chat; ⌘2 is the next slot.
        KeyBinding::new("cmd-2", ShowProject, None),
        KeyBinding::new("cmd-k", SearchChats, None),
        // What a browser binds its zoom to. `cmd-=` first so the menu
        // draws ⌘= like Safari; `cmd-+` is the same key with shift held.
        KeyBinding::new("cmd-=", ZoomIn, None),
        KeyBinding::new("cmd-+", ZoomIn, None),
        KeyBinding::new("cmd--", ZoomOut, None),
        KeyBinding::new("cmd-0", ZoomReset, None),
        // Step between the agents the panel lists. These are what the View
        // menu carries; the bare arrows below answer where no field has them.
        KeyBinding::new("alt-cmd-down", NextEntry, None),
        KeyBinding::new("alt-cmd-up", PrevEntry, None),
        // Claimed app-wide and answered last: an editor and a field bind copy
        // on their own contexts, which gpui dispatches from the focus outward,
        // so this only runs where nothing else wanted it — which is exactly
        // where a transcript selection is the thing being copied.
        KeyBinding::new("cmd-c", CopySelection, None),
        KeyBinding::new("ctrl-c", CopyChat, None),
        KeyBinding::new("ctrl-v", PasteChat, None),
        KeyBinding::new("delete", DeleteChat, Some(WINDOW_CONTEXT)),
        KeyBinding::new("backspace", DeleteChat, Some(WINDOW_CONTEXT)),
        // On the window's rest focus only. A binding with no context is
        // the deepest match gpui knows, so a bare `up` here would beat the
        // opener's and the sheet's own arrows while their field has the
        // focus; the composer forwards its arrows itself.
        KeyBinding::new("up", PrevEntry, Some(WINDOW_CONTEXT)),
        KeyBinding::new("down", NextEntry, Some(WINDOW_CONTEXT)),
        // A context menu closes on Escape wherever the focus rests; the
        // composer forwards its own Escape here when it has nothing to close.
        KeyBinding::new("escape", DismissMenu, Some(WINDOW_CONTEXT)),
        KeyBinding::new("enter", CommitName, Some(RENAME_CONTEXT)),
        KeyBinding::new("escape", DismissName, Some(RENAME_CONTEXT)),
    ]);
    crate::view::bind_field_editing(cx, RENAME_CONTEXT, false);
}

/// Resting chat window. macOS can restore a last-used frame that is
/// smaller than `window_min_size`; clamp those so the pane is usable.
const WINDOW_WIDTH: f32 = 1100.;
const WINDOW_HEIGHT: f32 = 761.;

/// The main window's AppKit title, as `open` sets it.
const WINDOW_TITLE: &str = "Arbos";

fn restore_usable_bounds(window: &mut Window) {
    let now = window.bounds().size;
    if now.width < px(600.) || now.height < px(320.) {
        window.resize(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)));
    }
    force_usable_ns_frame();
}

/// GPUI `resize` can no-op against a saved AppKit frame (we have seen
/// 100×131). Read the real `NSWindow` frame and set it when it is unusable.
///
/// Only the visible, titled main window: gpui keeps helper windows around
/// (a 500×500 one at the origin) that must not be blown up to a full frame.
/// A window in Stage Manager's strip also reports ~100×115 — that is the
/// strip's thumbnail, not the window, and pushing a frame at it does nothing
/// useful; it is skipped by the `isOnActiveSpace`/key check below.
#[cfg(target_os = "macos")]
#[repr(C)]
struct NsPoint {
    x: f64,
    y: f64,
}
#[cfg(target_os = "macos")]
#[repr(C)]
struct NsSize {
    width: f64,
    height: f64,
}
#[cfg(target_os = "macos")]
#[repr(C)]
struct NsRect {
    origin: NsPoint,
    size: NsSize,
}

#[cfg(target_os = "macos")]
fn force_usable_ns_frame() {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};

    const YES: i8 = 1;
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let windows: *mut Object = msg_send![app, windows];
        let count: usize = msg_send![windows, count];
        for i in 0..count {
            let ns_window: *mut Object = msg_send![windows, objectAtIndex: i];
            if ns_window.is_null() {
                continue;
            }
            let visible: bool = msg_send![ns_window, isVisible];
            let title: *mut Object = msg_send![ns_window, title];
            let titled = !title.is_null() && {
                let utf8: *const std::os::raw::c_char = msg_send![title, UTF8String];
                !utf8.is_null() && std::ffi::CStr::from_ptr(utf8).to_string_lossy() == WINDOW_TITLE
            };
            if !visible || !titled {
                continue;
            }
            let frame: NsRect = msg_send![ns_window, frame];
            // A saved frame that AppKit restored off the visible screens
            // (a display that is gone, a window dragged half off the edge:
            // Jacob saw only the left 40 %) comes back to the centre of
            // the main display.
            if frame.size.width >= 600. && frame.size.height >= 320. {
                if !frame_on_a_screen(&frame) {
                    let _: () = msg_send![ns_window, center];
                }
                continue;
            }
            let screen: *mut Object = msg_send![ns_window, screen];
            let screen = if screen.is_null() {
                msg_send![class!(NSScreen), mainScreen]
            } else {
                screen
            };
            if screen.is_null() {
                continue;
            }
            let vis: NsRect = msg_send![screen, visibleFrame];
            let w = WINDOW_WIDTH as f64;
            let h = WINDOW_HEIGHT as f64;
            let next = NsRect {
                origin: NsPoint {
                    x: vis.origin.x + ((vis.size.width - w) / 2.).max(0.),
                    y: vis.origin.y + ((vis.size.height - h) / 2.).max(0.),
                },
                size: NsSize {
                    width: w,
                    height: h,
                },
            };
            let _: () = msg_send![ns_window, setFrame: next display: YES];
        }
    }
}

/// Whether at least 70 % of `frame` lies inside some screen's visible
/// area: the window can be seen and reached.
#[cfg(target_os = "macos")]
fn frame_on_a_screen(frame: &NsRect) -> bool {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};
    let area = frame.size.width * frame.size.height;
    if area <= 0. {
        return true;
    }
    unsafe {
        let screens: *mut Object = msg_send![class!(NSScreen), screens];
        if screens.is_null() {
            return true;
        }
        let count: usize = msg_send![screens, count];
        for i in 0..count {
            let screen: *mut Object = msg_send![screens, objectAtIndex: i];
            if screen.is_null() {
                continue;
            }
            let vis: NsRect = msg_send![screen, visibleFrame];
            let x0 = frame.origin.x.max(vis.origin.x);
            let y0 = frame.origin.y.max(vis.origin.y);
            let x1 = (frame.origin.x + frame.size.width).min(vis.origin.x + vis.size.width);
            let y1 = (frame.origin.y + frame.size.height).min(vis.origin.y + vis.size.height);
            if x1 > x0 && y1 > y0 && (x1 - x0) * (y1 - y0) >= 0.7 * area {
                return true;
            }
        }
    }
    false
}

/// Float the main window above other apps. For the driver only: with Stage
/// Manager on, a window whose app is not on the stage is swapped for a strip
/// thumbnail, and every capture of it comes back as that thumbnail. A
/// floating window is not staged, so the driven app can be captured and
/// clicked while the operator works elsewhere.
#[cfg(target_os = "macos")]
pub(crate) fn float_main_window() {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};
    const NS_FLOATING_WINDOW_LEVEL: i64 = 3;
    // NSWindowCollectionBehaviorCanJoinAllSpaces | ...FullScreenAuxiliary |
    // ...Stationary: not tiled, not staged, on every Space.
    const BEHAVIOR: usize = (1 << 0) | (1 << 8) | (1 << 4);
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let windows: *mut Object = msg_send![app, windows];
        let count: usize = msg_send![windows, count];
        for i in 0..count {
            let ns_window: *mut Object = msg_send![windows, objectAtIndex: i];
            if ns_window.is_null() {
                continue;
            }
            let title: *mut Object = msg_send![ns_window, title];
            if title.is_null() {
                continue;
            }
            let utf8: *const std::os::raw::c_char = msg_send![title, UTF8String];
            // Every titled window — the chat and Settings; gpui's untitled
            // helpers are left alone.
            if utf8.is_null() || std::ffi::CStr::from_ptr(utf8).to_string_lossy().is_empty() {
                continue;
            }
            let visible: bool = msg_send![ns_window, isVisible];
            if !visible {
                continue;
            }
            let _: () = msg_send![ns_window, setLevel: NS_FLOATING_WINDOW_LEVEL];
            let _: () = msg_send![ns_window, setCollectionBehavior: BEHAVIOR];
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn float_main_window() {}

#[cfg(not(target_os = "macos"))]
fn force_usable_ns_frame() {}

/// Open the workspace window. Called at launch, and again when the Dock
/// reopens an app whose window ⌘W closed.
pub fn open(settings: Settings, state: State, cx: &mut App) -> Result<WindowHandle<Arbos>> {
    let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // No strip of its own: the traffic lights sit in the nav, so the
            // window owes no titlebar above it.
            titlebar: Some(TitlebarOptions {
                title: Some("Arbos".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(TRAFFIC_LIGHT_X), px(TRAFFIC_LIGHT_Y))),
                ..Default::default()
            }),
            // Glass needs a blurred window background to blur into.
            window_background: Theme::of(cx).window_background_appearance(),
            window_min_size: Some(size(px(600.), px(320.))),
            app_id: Some("arbos-desktop".into()),
            ..Default::default()
        },
        |window, cx| {
            appearance::observe_window(window, cx).detach();
            window.resize(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)));
            cx.new(|cx| Arbos::new(settings, state, window, cx))
        },
    )?;
    // macOS can apply a saved frame after the first paint — a 100×131
    // leftover from a prior session. Push the usable size again once.
    let again = handle.clone();
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(300))
            .await;
        let _ = again.update(cx, |_, window, _| {
            window.resize(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)));
            // gpui's resize can no-op against a saved AppKit frame (a
            // 100x114 leftover has been seen after a relaunch); set the
            // NSWindow frame directly when it is still unusable.
            force_usable_ns_frame();
        });
    })
    .detach();
    Ok(handle)
}

/// Which pane the detail column shows. A property of the window, not of a
/// project; a tab click or a new tab lands on the chat (Jacob: a new or
/// empty project never opens on the Project page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Chat,
    Surface,
    /// The project page: the status page, the store's files, the context
    /// document — Cursor's Project tab, full width in the column.
    Project,
}

/// One step from `at` through `len` entries. Past either end is
/// nowhere — stay on the first or the last. A list of none, or no
/// current place in it, has nowhere to land.
fn stepped(at: Option<usize>, len: usize, step: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let Some(at) = at else {
        return None;
    };
    let next = at as isize + step;
    if next < 0 || next >= len as isize {
        return None;
    }
    Some(next as usize)
}

/// The root view. It owns no app state — only the chrome's own: whether the
/// panel is out, which pane is showing, and whichever name is being typed.
/// A live call: one per window, to the project that was in front when it
/// started. Everything spoken lands in that project's main chat.
#[derive(Debug, Clone)]
pub struct Call {
    /// The project's session id whose chat takes the `voice ·` lines.
    pub session: u64,
    pub label: String,
    pub since: std::time::Instant,
    /// Start is in flight: the button shows a spinner, End is a no-op.
    pub connecting: bool,
}

pub struct Arbos {
    pub(crate) workspace: Entity<Workspace>,
    /// The open session menu was opened from the chat header's `⋯`, so it
    /// anchors there rather than at a panel row.
    pub(crate) menu_at_header: bool,
    /// The git branch of the last local place looked at, for the row under
    /// the composer. `(place path, branch or none)`.
    pub(crate) branch_cache: std::cell::RefCell<Option<(std::path::PathBuf, Option<String>)>>,
    pub(crate) terminals:
        std::collections::HashMap<String, Entity<crate::view::terminal::TerminalPane>>,
    active_terminal: Option<String>,
    /// Whether the right-hand panel is out. ⌘B folds it away.
    pub(crate) panel_open: bool,
    /// The panel's "N archived" row is unfolded: finished workers the
    /// kernel moved to `archive/agents/` are listed, faint.
    pub(crate) archived_open: bool,
    /// The Working card the user closed (× or the pill): the root chat and
    /// the workers that were running. A new worker after that reopens it,
    /// so the next fan-out shows the card again.
    pub(crate) working_card_closed: Option<(u64, Vec<u64>)>,
    pub(crate) composer: Entity<Composer>,
    pub(crate) opener: Entity<Opener>,
    /// The sheet a tab's name, glyph and colour are set in.
    pub(crate) tab_sheet: Entity<TabSheet>,
    pub(crate) permissions_sheet: Entity<PermissionsSheet>,
    pub(crate) permission_center: Entity<PermissionCenter>,
    /// ⌘K: the palette over every open tab's chats.
    pub(crate) chat_search: Entity<ChatSearch>,
    settings_window: Option<WindowHandle<SettingsWindow>>,
    pub(crate) pane: Pane,
    pub(crate) menu: Option<Menu>,
    /// Which of the open menu's rows is live. Held here rather than in the
    /// card, which is rebuilt every frame: the pointer moves the cursor, and
    /// a cursor made afresh each paint would light nothing.
    pub(crate) menu_cursor: Cursor,
    /// Whether the press now being handled landed on the open menu's own
    /// trigger — read by [`Arbos::toggle_menu`] and nothing else.
    pub(crate) menu_pressed: bool,
    /// What the name field is attached to, and the field itself.
    pub(crate) renaming: Option<Renaming>,
    /// True when the empty-chat title is being edited, so the field lives
    /// there — not also on a panel row, which would move and restyle it.
    pub(crate) rename_heading: bool,
    /// True when the chat header's title is being edited: the field sits on
    /// the header line, Body-sized, and nowhere else.
    pub(crate) rename_in_header: bool,
    /// The heading field just appeared under a double-click. The next press
    /// on it would park a caret; take the whole title instead, then drop this.
    pub(crate) select_heading: bool,
    pub(crate) name_field: Entity<TextField>,
    meter: Entity<Stats>,
    meter_at: Floating,
    /// Where the focus rests when no field holds it — a board, a table and a
    /// transcript have none — so the bindings below always have a path here.
    focus: FocusHandle,
    /// Bumps on every mic click so a late start/stop cannot paint the
    /// opposite state.
    voice_gen: u64,
    /// Place that started the current take. Stop must hit that kernel,
    /// not whichever project is in front when the mic is pressed again.
    voice_place: Option<crate::model::place::Place>,
    /// Fn is down. A second flagsChanged for the same hold is ignored.
    fn_held: bool,
    /// Stop was asked while start was still in flight. Finish start, then stop.
    voice_want_stop: bool,
    /// The last dictated take's clock, for the driver: press → first
    /// partial, release → send. What "does it feel instant" measures.
    pub(crate) dictation: Dictation,
    /// The loop that carries the speech server's agent activity into the
    /// chat is running.
    voice_mirror_on: bool,
    /// The call in progress: which project it is for, and since when.
    pub(crate) call: Option<Call>,
    /// The tab sheet is up for the Home tab's first-launch offer; when it
    /// closes, the permissions sheet follows.
    home_offer: bool,
    /// Native Fn monitor. Lives with the window so Drop removes it.
    #[cfg(target_os = "macos")]
    _fn_monitor: Option<crate::view::fn_key::Monitor>,
    /// Debounced write of the composer line so a kill still has it.
    pub(crate) draft_flush: Task<()>,
}

impl Arbos {
    fn sync_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = self.workspace.read(cx);
        self.terminals.retain(|id, _| {
            workspace.projects.iter().any(|project| {
                project
                    .surfaces
                    .iter()
                    .any(|surface| surface.terminal_id() == Some(id.as_str()))
            })
        });
        let active = workspace.active_surface().and_then(|surface| {
            Some((
                surface.terminal_id()?.to_owned(),
                workspace.active_project()?.place(),
            ))
        });
        let active_id = active.as_ref().map(|(id, _)| id.clone());
        if let Some((id, place)) = active {
            let terminal = self.terminals.entry(id.clone()).or_insert_with(|| {
                cx.new(|cx| crate::view::terminal::TerminalPane::new(place, id, cx))
            });
            if self.active_terminal != active_id {
                window.focus(&terminal.focus_handle(cx), cx);
            }
        }
        self.active_terminal = active_id;
    }

    pub fn new(
        settings: Settings,
        state: State,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        restore_usable_bounds(window);
        let composer = cx.new(Composer::new);
        let opener = cx.new(Opener::new);
        let tab_sheet = cx.new(TabSheet::new);
        let permission_center = cx.new(|_| PermissionCenter::new(None));
        cx.set_global(Permissions(permission_center.clone()));
        let permissions_sheet = cx.new(|cx| PermissionsSheet::new(permission_center.clone(), cx));
        cx.observe(&permission_center, |_, _, cx| cx.notify())
            .detach();
        cx.subscribe_in(
            &permissions_sheet,
            window,
            |this, _, event: &PermissionsSheetEvent, window, cx| match event {
                PermissionsSheetEvent::Closed => {
                    this.workspace
                        .update(cx, |workspace, _| workspace.mark_permissions_seen());
                    this.focus_composer(window, cx);
                }
            },
        )
        .detach();
        let chat_search = cx.new(ChatSearch::new);
        cx.subscribe_in(
            &chat_search,
            window,
            |this, _, event: &ChatSearchEvent, window, cx| match event {
                ChatSearchEvent::Open { project, session } => {
                    let (project, session) = (*project, *session);
                    this.workspace.update(cx, |workspace, cx| {
                        workspace.active = Some(project);
                        workspace.select_session(session, cx);
                    });
                    this.show_pane(Pane::Chat, cx);
                    this.focus_composer(window, cx);
                }
                ChatSearchEvent::Dismiss => this.focus_composer(window, cx),
            },
        )
        .detach();
        cx.subscribe_in(
            &opener,
            window,
            |this, _, event: &OpenerEvent, window, cx| match event {
                OpenerEvent::Open(place) => {
                    let place = place.clone();
                    this.offer_store_out_of_sync(&place, window, cx);
                    this.show_pane(Pane::Chat, cx);
                    this.workspace
                        .update(cx, |workspace, cx| workspace.open_place(place, cx));
                    this.offer_tab_face(window, cx);
                }
                OpenerEvent::Browse => this.browse_local(window, cx),
                OpenerEvent::Dismiss => {}
            },
        )
        .detach();
        cx.subscribe_in(
            &tab_sheet,
            window,
            |this, _, event: &TabSheetEvent, window, cx| {
                match event {
                    TabSheetEvent::Keep(ix, identity) => {
                        let (ix, identity) = (*ix, identity.clone());
                        this.workspace
                            .update(cx, |workspace, cx| workspace.set_identity(ix, identity, cx));
                    }
                    // Skipped on the Home tab's one offer: the defaults are kept
                    // as its face, so the sheet is not offered again.
                    TabSheetEvent::Dismiss => {
                        if this.home_offer {
                            this.workspace.update(cx, |workspace, cx| {
                                if let Some(ix) = workspace.home_index()
                                    && let Some(project) = workspace.projects.get(ix)
                                    && !project.identity_saved
                                {
                                    let mut identity = project.identity.clone();
                                    identity.name = Some("Home".into());
                                    workspace.set_identity(ix, identity, cx);
                                }
                            });
                        }
                    }
                }
                if std::mem::take(&mut this.home_offer) {
                    this.permissions_once(window, cx);
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &composer,
            window,
            |this, _, event: &ComposerEvent, window, cx| match event {
                ComposerEvent::Submit(text) => {
                    // A new prompt while a reply is being read: the reply stops.
                    crate::voice_ws::interrupt();
                    this.submit(text.clone(), cx)
                }
                ComposerEvent::Queue(text) => this.queue_turn(text.clone(), cx),
                // Escape in the composer with no picker open: a menu, if one
                // is up, goes first; only then does it mean "stop the turn".
                ComposerEvent::Cancel => {
                    if this.menu.is_some() {
                        this.dismiss_menu(cx);
                    } else {
                        crate::voice_ws::interrupt();
                        this.cancel_turn(cx);
                    }
                }
                ComposerEvent::Agent(ix) => this.pick_agent(*ix, cx),
                ComposerEvent::Switch(id, value) => this.switch(id, value, cx),
                // The kernel owns modes: the chip's pick is the `/mode`
                // line, sent like any prompt (the kernel intercepts it and
                // starts no turn).
                ComposerEvent::Mode(skill) => {
                    let line = match skill {
                        Some(name) => format!("/mode {name}"),
                        None => "/mode off".to_string(),
                    };
                    this.submit(crate::model::attachment::Prompt::from(line), cx)
                }
                ComposerEvent::Voice => this.toggle_voice(cx),
                ComposerEvent::Attach => {}
                ComposerEvent::Step(step) => this.cycle_entry(*step, window, cx),
                ComposerEvent::Delete => this.delete_highlighted(cx),
            },
        )
        .detach();

        let name_field = name_field_entity(false, cx);
        let workspace = cx.new(|cx| Workspace::new(settings, state, cx));
        // The model is the only thing that says a session appeared or a turn
        // ended; the composer's placeholder, commands and busy state are all
        // read back from it rather than pushed by whoever caused the change.
        cx.observe(&workspace, |this, _, cx| {
            this.sync_composer(cx);
            cx.notify();
        })
        .detach();
        // A re-read replaced what a pane is showing — see
        // [`Workspace::reload_project`]. The card and the cell are addressed by
        // where they sit, so filing them now would file them into whatever slid
        // under the index; the edit is dropped instead, and the field with it.
        // The caret follows the document, which is a new editor entity.
        cx.subscribe_in(&workspace, window, |_, _, _: &Reloaded, _, cx| {
            cx.notify();
        })
        .detach();
        // The kernel opened a terminal, browser, or process under the chat,
        // or closed the one in front. Which pane shows is the window's to
        // decide, so the model asks and this answers — the same switch a
        // click on a panel row makes.
        cx.subscribe_in(
            &workspace,
            window,
            |this, _, request: &PaneRequest, _, cx| match request {
                PaneRequest::Surface(_) => this.show_pane(Pane::Surface, cx),
                PaneRequest::Chat => this.show_pane(Pane::Chat, cx),
            },
        )
        .detach();

        let mut this = Self {
            meter: cx.new(Stats::new),
            meter_at: Floating::new(Painter::of(cx)),
            workspace,
            terminals: Default::default(),
            active_terminal: None,
            panel_open: true,
            archived_open: false,
            working_card_closed: None,
            composer,
            opener,
            tab_sheet,
            permissions_sheet,
            permission_center,
            chat_search,
            settings_window: None,
            pane: Pane::Chat,
            menu: None,
            menu_cursor: Cursor::default(),
            menu_pressed: false,
            renaming: None,
            rename_heading: false,
            rename_in_header: false,
            select_heading: false,
            branch_cache: std::cell::RefCell::new(None),
            menu_at_header: false,
            name_field,
            focus: cx.focus_handle(),
            voice_gen: 0,
            voice_place: None,
            fn_held: false,
            voice_want_stop: false,
            dictation: Dictation::default(),
            voice_mirror_on: false,
            call: None,
            home_offer: false,
            #[cfg(target_os = "macos")]
            _fn_monitor: None,
            draft_flush: Task::ready(()),
        };
        let field = this.composer.read(cx).text_field();
        cx.subscribe(&field, |this, _, event: &FieldEvent, cx| {
            if *event == FieldEvent::Changed {
                this.queue_draft_flush(cx);
            }
        })
        .detach();
        cx.on_release(|this, cx| {
            this.flush_composer_draft(cx);
            if let Some(handle) = this.settings_window.take() {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
        })
        .detach();
        // Whatever held the focus has left the tree — the composer with the
        // chat pane, an editor with its article — and an unrendered element
        // dispatches nothing, so the window takes its focus back.
        cx.on_focus_lost(window, |this, window, cx| window.focus(&this.focus, cx))
            .detach();
        // The backstop under the watch. Coming back to the window is where a
        // dropped event costs the most and the one moment we can be sure of
        // catching, so every project is re-read on the way in — see
        // [`Workspace::reload_projects`].
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.workspace
                    .update(cx, |workspace, cx| workspace.reload_projects(cx));
            } else {
                this.flush_composer_draft(cx);
            }
        })
        .detach();
        // Entering or leaving a size — zoom, simple fullscreen — can drop the
        // blur view and the transparent titlebar. Put both back, and keep
        // Spaces fullscreen off: a style-mask change resets that too.
        cx.observe_window_bounds(window, |_, window, cx| {
            appearance::reapply_window_background(cx);
            keep_macos_glass(window);
            sync_macos_chrome(cx);
            restore_usable_bounds(window);
            cx.notify();
        })
        .detach();
        // The palette moved (Settings › Appearance, or the OS at sunset):
        // the window's own chrome follows it, so the title band the traffic
        // lights sit in is the same surface as the tab strip.
        cx.observe_global::<Theme>(|_, cx| sync_macos_chrome(cx))
            .detach();
        keep_macos_glass(window);
        sync_macos_chrome(cx);
        this.sync_composer(cx);
        // First launch: the Home tab's face — name, glyph, colour, prefilled
        // "Home", skippable — then the permissions sheet, each once. After
        // the window has painted, so they open over something rather than
        // before it.
        let home_fresh = this.workspace.read(cx).home_index().is_some_and(|ix| {
            this.workspace
                .read(cx)
                .projects
                .get(ix)
                .is_some_and(|project| !project.identity_saved)
        });
        // Cursor's Changes pill: what the project's working tree holds
        // uncommitted, read from git off the UI thread every few seconds
        // for the project in front.
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(CHANGES_POLL).await;
                let Ok(root) = this.update(cx, |this, cx| {
                    this.workspace
                        .read(cx)
                        .active_project()
                        .filter(|project| !project.is_remote())
                        .map(|project| project.path.clone())
                }) else {
                    break;
                };
                let Some(root) = root else { continue };
                let read_root = root.clone();
                let changes = cx
                    .background_executor()
                    .spawn(async move { crate::model::changes::GitChanges::read(&read_root) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.workspace.update(cx, |workspace, cx| {
                        let before = workspace.changes.get(&root).cloned();
                        match changes {
                            Some(changes) => {
                                workspace.changes.insert(root.clone(), changes);
                            }
                            None => {
                                workspace.changes.remove(&root);
                            }
                        }
                        if workspace.changes.get(&root).cloned() != before {
                            cx.notify();
                        }
                    });
                });
            }
        })
        .detach();
        if home_fresh || !this.workspace.read(cx).permissions_seen {
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
                let _ = this.update_in(cx, |this, window, cx| {
                    if home_fresh && let Some(ix) = this.workspace.read(cx).home_index() {
                        this.home_offer = true;
                        this.edit_tab(ix, window, cx);
                    } else {
                        this.permissions_once(window, cx);
                    }
                });
            })
            .detach();
        }
        // Where the caret starts. The composer is drawn only over a chat it can
        // send to, and focus on an element no frame draws is focus nowhere.
        let composer = this
            .workspace
            .read(cx)
            .active_session()
            .is_some_and(ChatSession::resumable)
            .then(|| this.composer_focus_handle(cx));
        window.focus(composer.as_ref().unwrap_or(&this.focus), cx);
        // A speech server is set up: open the session now, in the background,
        // so the first Fn press has no connect to pay.
        if crate::voice_ws::configured() {
            cx.background_executor()
                .spawn(async move {
                    let _ = crate::voice_ws::warm();
                })
                .detach();
        }
        #[cfg(target_os = "macos")]
        {
            // A speech server is configured, so the mic will be wanted:
            // ask macOS now, from this process, so the one dialog comes up
            // at start rather than mid-sentence on the first Fn press.
            if crate::voice_ws::configured() {
                crate::voice_ws::request_mic_permission();
            }
            let (tx, rx) = futures::channel::mpsc::unbounded();
            this._fn_monitor = Some(crate::view::fn_key::Monitor::start(move |down| {
                let _ = tx.unbounded_send(down);
            }));
            cx.spawn(async move |this, cx| {
                use futures::StreamExt as _;
                let mut rx = rx;
                while let Some(down) = rx.next().await {
                    let _ = this.update(cx, |this, cx| {
                        if down {
                            this.fn_down(cx);
                        } else {
                            this.fn_up(cx);
                        }
                    });
                }
            })
            .detach();
        }
        this
    }

    pub(crate) fn bind_name_field(&mut self, heading: bool, cx: &mut Context<Self>) {
        self.rename_heading = heading;
        self.name_field = name_field_entity(heading, cx);
    }

    /// ⌘N: a sub-chat under the project's main chat. It shows in the panel
    /// with the sub-agents; the main chat stays the one root.
    pub(crate) fn new_session_action(
        &mut self,
        _: &NewSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_pane(Pane::Chat, cx);
        self.workspace.update(cx, |workspace, cx| {
            workspace.new_child_session(cx);
        });
        self.focus_composer_after_create(window, cx);
    }

    /// ⌘T: a new tab, which is a project — pick the machine, then the
    /// folder. Same picker as ⌘O.
    pub(crate) fn new_tab_action(
        &mut self,
        _: &NewTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_project_action(&OpenProject, window, cx);
    }

    pub(crate) fn next_tab(&mut self, _: &NextTab, _: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(1, cx);
    }

    pub(crate) fn prev_tab(&mut self, _: &PrevTab, _: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(-1, cx);
    }

    /// Step to the neighbouring tab, wrapping at either end as a browser
    /// does.
    fn cycle_tab(&mut self, step: isize, cx: &mut Context<Self>) {
        let (at, len) = {
            let workspace = self.workspace.read(cx);
            (workspace.active, workspace.projects.len())
        };
        let (Some(at), true) = (at, len > 1) else {
            return;
        };
        let next = (at as isize + step).rem_euclid(len as isize) as usize;
        self.select_project(next, cx);
    }

    /// Copy what the transcript has selected. Bound app-wide and reached only
    /// where nothing nearer to the focus claimed the chord.
    fn copy_selection(&mut self, _: &CopySelection, _: &mut Window, cx: &mut Context<Self>) {
        self.workspace.update(cx, |workspace, cx| {
            if !workspace.copy_selection(cx) {
                workspace.copy_active_chat(cx);
            }
        });
    }

    pub(crate) fn copy_chat(&mut self, _: &CopyChat, _: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.is_some() {
            return;
        }
        self.workspace
            .update(cx, |workspace, cx| workspace.copy_active_chat(cx));
    }

    pub(crate) fn paste_chat(&mut self, _: &PasteChat, _: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.is_some() {
            return;
        }
        self.workspace
            .update(cx, |workspace, cx| workspace.paste_chat(cx));
    }

    pub(crate) fn delete_chat(&mut self, _: &DeleteChat, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_highlighted(cx);
    }

    /// Archive an open chat, or kernel-delete an archived one. Always the
    /// highlighted panel row — not whichever session last held the caret.
    fn delete_highlighted(&mut self, cx: &mut Context<Self>) {
        if self.renaming.is_some() {
            return;
        }
        let Some(id) = self.highlighted_session(cx) else {
            return;
        };
        self.delete_or_archive(id, cx);
    }

    /// The chat row with the selection wash — the one Delete should hit.
    fn highlighted_session(&self, cx: &App) -> Option<u64> {
        if self.showing(cx) != Some(Pane::Chat) {
            return None;
        }
        let workspace = self.workspace.read(cx);
        let id = workspace.active_id()?;
        workspace
            .active_project()
            .and_then(|project| project.focus)
            .is_some_and(|focus| focus.surface.is_none())
            .then_some(id)
    }

    pub(crate) fn next_entry(
        &mut self,
        _: &NextEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_entry(1, window, cx);
    }

    pub(crate) fn prev_entry(
        &mut self,
        _: &PrevEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_entry(-1, window, cx);
    }

    /// Step to the next agent the panel lists, top to bottom through the
    /// project in front: the main chat, then each sub-agent under it. The
    /// ends stay put — on the first, Up does nothing; on the last, Down
    /// does nothing.
    fn cycle_entry(&mut self, step: isize, _window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.is_some() {
            return;
        }
        // Nothing on screen is nothing to step from: the launch view is not an
        // entry, and its neighbour is not another one.
        if self.showing(cx).is_none() {
            return;
        }
        let showing = self
            .workspace
            .read(cx)
            .active_project()
            .and_then(|project| project.focused_agent());
        let list: Vec<u64> = self
            .visible_agent_rows(cx)
            .into_iter()
            .map(|row| row.id)
            .collect();
        let at = showing.and_then(|id| list.iter().position(|entry| *entry == id));
        let Some(landing) = stepped(at, list.len(), step).map(|ix| list[ix]) else {
            return;
        };
        self.select_session(landing, cx);
    }

    /// Leaving a project is the moment a half-written card has to be filed:
    /// the spot it points at belongs to the board being navigated away from.
    /// A tab click lands on the project's chat, whatever the column showed
    /// before — the way back from the Project page or a document, and the
    /// view a new tab opens on.
    pub(crate) fn select_project(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.show_pane(Pane::Chat, cx);
        self.workspace
            .update(cx, |workspace, cx| workspace.select_project(ix, cx));
    }

    pub(crate) fn close_project(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.workspace
            .update(cx, |workspace, cx| workspace.close_project(ix, cx));
    }

    /// Close the open context menu, if any.
    pub(crate) fn dismiss_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.shut_menu();
            cx.notify();
        }
    }

    fn dismiss_menu_action(&mut self, _: &DismissMenu, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.dismiss_menu(cx);
            return;
        }
        // Nothing to close: Escape leaves the Project page (or a document)
        // for the chat, as ⌘1 does.
        if self.pane != Pane::Chat {
            self.show_chat(&ShowChat, window, cx);
        }
    }

    pub(crate) fn show_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        self.pane = pane;
        cx.notify();
    }

    pub(crate) fn select_session(&mut self, id: u64, cx: &mut Context<Self>) {
        self.show_pane(Pane::Chat, cx);
        self.workspace
            .update(cx, |workspace, cx| workspace.select_session(id, cx));
    }

    pub(crate) fn select_surface(
        &mut self,
        id: crate::model::surface::SurfaceId,
        cx: &mut Context<Self>,
    ) {
        self.show_pane(Pane::Surface, cx);
        self.workspace
            .update(cx, |workspace, cx| workspace.select_surface(id, cx));
    }

    pub(crate) fn open_settings_action(
        &mut self,
        _: &OpenSettings,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings(Section::General, cx);
    }

    pub(crate) fn attach_paths_action(
        &mut self,
        action: &AttachPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths: Vec<std::path::PathBuf> =
            action.paths.iter().map(std::path::PathBuf::from).collect();
        self.composer.update(cx, |composer, cx| {
            composer.accept_paths(paths, window, cx);
        });
    }

    pub(crate) fn toggle_panel_action(
        &mut self,
        _: &TogglePanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.panel_open = !self.panel_open;
        cx.notify();
    }

    /// Step the UI font size by `by` points. Every other size is a ratio of
    /// it, so this is what "zoom" means here. Clamped to the same range
    /// the settings stepper allows.
    pub(crate) fn zoom_by(&mut self, by: f32, cx: &mut Context<Self>) {
        let size = self.workspace.read(cx).text_size;
        let next = (size + by).clamp(state::TEXT_SIZE.0, state::TEXT_SIZE.1);
        if next == size {
            return;
        }
        self.workspace
            .update(cx, |workspace, cx| workspace.set_text_size(next, cx));
        cx.notify();
    }

    pub(crate) fn zoom_in_action(&mut self, _: &ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.zoom_by(1., cx);
    }

    pub(crate) fn zoom_out_action(&mut self, _: &ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.zoom_by(-1., cx);
    }

    pub(crate) fn zoom_reset_action(
        &mut self,
        _: &ZoomReset,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let default = TextStyle::Body.size();
        if self.workspace.read(cx).text_size == default {
            return;
        }
        self.workspace
            .update(cx, |workspace, cx| workspace.set_text_size(default, cx));
        cx.notify();
    }

    /// ⌘W and the menu's Close Tab: the tab in front. A tab's own close
    /// mark names its tab; the chord has only the one in front.
    pub(crate) fn close_project_action(
        &mut self,
        _: &CloseProject,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.workspace.read(cx).active else {
            return;
        };
        self.close_project(ix, cx);
    }

    /// ⌘K and the panel's magnifier: search every open tab's chats by
    /// title and first words; Enter opens the one lit, in its tab.
    pub(crate) fn search_chats(
        &mut self,
        _: &SearchChats,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace.read(cx);
        let mut hits: Vec<Hit> = Vec::new();
        for (ix, project) in workspace.projects.iter().enumerate() {
            let tab = Workspace::tab_label(project);
            let mut chats: Vec<&ChatSession> = project
                .sessions
                .iter()
                .filter(|chat| !chat.closed)
                .collect();
            chats.sort_by(|a, b| b.updated.cmp(&a.updated));
            for chat in chats {
                let title = chat.label();
                let first = crate::model::session::first_user_text(&chat.items)
                    .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
                    .unwrap_or_default();
                let first: String = first.chars().take(72).collect();
                let label = if first.is_empty() || first == title {
                    format!("{tab} › {title}")
                } else {
                    format!("{tab} › {title} — {first}")
                };
                hits.push(Hit {
                    project: ix,
                    session: chat.id,
                    label,
                });
            }
        }
        self.dismiss_menu(cx);
        self.chat_search
            .update(cx, |search, cx| search.show(hits, window, cx));
    }

    /// The composer takes the keyboard back, when there is one to take it.
    fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let composer = self
            .workspace
            .read(cx)
            .active_session()
            .map(|_| self.composer.read(cx).focus_handle(cx));
        window.focus(composer.as_ref().unwrap_or(&self.focus), cx);
    }

    /// ⌘2 and the panel's Project header: the project page in the column.
    pub(crate) fn show_project(&mut self, _: &ShowProject, _: &mut Window, cx: &mut Context<Self>) {
        self.show_pane(Pane::Project, cx);
    }

    pub(crate) fn show_chat(&mut self, _: &ShowChat, _: &mut Window, cx: &mut Context<Self>) {
        self.workspace.update(cx, |workspace, _| {
            if let Some(project) = workspace.active_project_mut() {
                if let Some(focus) = &mut project.focus {
                    focus.surface = None;
                }
            }
        });
        self.show_pane(Pane::Chat, cx);
    }

    pub(crate) fn open_settings(&mut self, section: Section, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let had = self.settings_window.is_some();
        self.settings_window = settings::open(workspace, self.settings_window, section, cx);
        // When the window goes — Escape, ⌘W, the title bar — this window
        // comes back forward and the composer takes the keyboard, so the
        // settings never sit between the user and the chat.
        if !had
            && let Some(view) = self
                .settings_window
                .and_then(|handle| handle.entity(cx).ok())
        {
            cx.observe_release(&view, |this, _, cx| {
                this.settings_window = None;
                if let Some(main) = cx
                    .windows()
                    .into_iter()
                    .find(|w| w.downcast::<Self>().is_some())
                {
                    let _ = main.update(cx, |_, window, _| window.activate_window());
                }
                let composer = this.composer.read(cx).focus_handle(cx);
                if let Some(main) = cx
                    .windows()
                    .into_iter()
                    .find(|w| w.downcast::<Self>().is_some())
                {
                    let _ = main.update(cx, |_, window, cx| window.focus(&composer, cx));
                }
            })
            .detach();
        }
    }

    /// Mic button: start capture, or stop, put the words in the field, and send.
    /// While a speech-server session is live, what its agent does
    /// (`agent.*`, `tool.*`, `text.done`) lands in the active chat as
    /// notices, a few times a second. Ends when the session does.
    pub(crate) fn start_voice_mirror(&mut self, cx: &mut Context<Self>) {
        if self.voice_mirror_on || !crate::voice_ws::configured() {
            return;
        }
        self.voice_mirror_on = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(400))
                    .await;
                let lines = crate::voice_ws::drain_mirror();
                let live = crate::voice_ws::status().phase.is_some();
                let keep = this.update(cx, |this, cx| {
                    if !lines.is_empty() {
                        let id = this.workspace.read(cx).active_id();
                        if let Some(id) = id {
                            this.workspace.update(cx, |workspace, cx| {
                                workspace.with_session(id, cx, |chat| {
                                    for m in &lines {
                                        chat.notice(false, &mirror_line(m));
                                    }
                                });
                            });
                        }
                    }
                    if !live {
                        this.voice_mirror_on = false;
                    }
                    live
                });
                if !matches!(keep, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn toggle_voice(&mut self, cx: &mut Context<Self>) {
        if self.composer.read(cx).is_recording() || self.voice_want_stop {
            self.stop_voice(cx);
        } else {
            self.start_voice(cx);
        }
    }

    /// The Fn key, as the native monitor (or the driver) reports it.
    pub(crate) fn fn_key(&mut self, down: bool, cx: &mut Context<Self>) {
        if down {
            self.fn_down(cx);
        } else {
            self.fn_up(cx);
        }
    }

    fn fn_down(&mut self, cx: &mut Context<Self>) {
        if self.fn_held {
            return;
        }
        self.fn_held = true;
        self.dictation = Dictation {
            pressed_at: Some(Instant::now()),
            ..Dictation::default()
        };
        if self.composer.read(cx).is_recording() || self.composer.read(cx).voice_busy() {
            return;
        }
        self.start_voice(cx);
    }

    fn fn_up(&mut self, cx: &mut Context<Self>) {
        if !self.fn_held {
            return;
        }
        self.fn_held = false;
        self.dictation.released_at = Some(Instant::now());
        self.stop_voice(cx);
    }

    fn start_voice(&mut self, cx: &mut Context<Self>) {
        let Some(place) = self
            .workspace
            .read(cx)
            .active_project()
            .map(|project| project.place())
        else {
            return;
        };
        if self.composer.read(cx).is_recording() || self.composer.read(cx).voice_busy() {
            return;
        }
        self.voice_want_stop = false;
        self.voice_gen = self.voice_gen.wrapping_add(1);
        let stamp = self.voice_gen;
        let started_at = place.clone();
        self.composer.update(cx, |composer, cx| {
            composer.set_voice(VoiceState::Busy, cx);
        });
        cx.spawn(async move |this, cx| {
            let started = cx
                .background_executor()
                .spawn(async move { kernel::voice_start_place(&place) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.voice_gen != stamp {
                    return;
                }
                match started {
                    Ok(()) => {
                        this.voice_place = Some(started_at.clone());
                        this.composer.update(cx, |composer, cx| {
                            composer.set_voice(VoiceState::Recording, cx);
                        });
                        this.poll_voice(stamp, started_at, cx);
                        this.start_voice_mirror(cx);
                        if this.voice_want_stop {
                            this.stop_voice(cx);
                        }
                    }
                    Err(e) => {
                        this.voice_want_stop = false;
                        this.composer.update(cx, |composer, cx| {
                            composer.set_voice(VoiceState::Idle, cx);
                        });
                        this.voice_error(&format!("voice failed: {e:#}"), cx);
                    }
                }
            });
        })
        .detach();
    }

    /// Pull partials while the helper is listening so the composer can
    /// paint them before Stop commits the final string.
    fn poll_voice(
        &mut self,
        stamp: u64,
        place: crate::model::place::Place,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            loop {
                let live = this.update(cx, |this, _| {
                    this.voice_gen == stamp && this.voice_place.is_some()
                });
                if !matches!(live, Ok(true)) {
                    break;
                }
                let peeked = cx
                    .background_executor()
                    .spawn({
                        let place = place.clone();
                        async move { kernel::voice_peek_place(&place) }
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.voice_gen != stamp {
                        return;
                    }
                    if let Ok(text) = peeked {
                        if !text.trim().is_empty() && this.dictation.first_partial_ms.is_none() {
                            this.dictation.first_partial_ms = this
                                .dictation
                                .pressed_at
                                .map(|at| at.elapsed().as_millis() as u64);
                        }
                        this.composer.update(cx, |composer, cx| {
                            composer.set_voice_preview(&text, cx);
                        });
                    }
                });
                // Partials land within the first second; the poll keeps up
                // with them.
                cx.background_executor()
                    .timer(Duration::from_millis(80))
                    .await;
            }
        })
        .detach();
    }

    /// Stop the take. The transcript is sent, like the web board's
    /// `dictation final`.
    fn stop_voice(&mut self, cx: &mut Context<Self>) {
        let Some(place) = self.voice_place.clone().or_else(|| {
            self.workspace
                .read(cx)
                .active_project()
                .map(|project| project.place())
        }) else {
            return;
        };
        if self.composer.read(cx).voice_busy() && self.voice_place.is_none() {
            self.voice_want_stop = true;
            return;
        }
        if !self.composer.read(cx).is_recording() && self.voice_place.is_none() {
            return;
        }
        self.voice_want_stop = false;
        self.voice_gen = self.voice_gen.wrapping_add(1);
        let stamp = self.voice_gen;
        let place = self.voice_place.take().unwrap_or(place);
        self.composer.update(cx, |composer, cx| {
            composer.set_voice(VoiceState::Busy, cx);
        });
        cx.spawn(async move |this, cx| {
            let text = cx
                .background_executor()
                .spawn(async move { kernel::voice_stop_place(&place) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.voice_gen != stamp {
                    return;
                }
                let spoken = matches!(&text, Ok(t) if !t.trim().is_empty());
                // A duplex server answered the words itself (and may have
                // sent them to its own kernel): the transcript goes into the
                // composer for the record, but is neither sent nor read back.
                let server_answers =
                    crate::voice_ws::configured() && crate::voice_ws::server_answers();
                if spoken && crate::voice_ws::configured() && !server_answers {
                    // The answer to a dictated prompt is read aloud.
                    if let Some(id) = this.workspace.read(cx).active_id() {
                        this.workspace.update(cx, |workspace, cx| {
                            workspace.with_session(id, cx, |chat| chat.voice_reply = true);
                        });
                    }
                }
                let sent = matches!(&text, Ok(t) if !t.trim().is_empty());
                this.composer.update(cx, |composer, cx| {
                    composer.set_voice(VoiceState::Idle, cx);
                    if let Ok(text) = &text
                        && !text.trim().is_empty()
                    {
                        if server_answers {
                            composer.dictation_text(text, cx);
                        } else {
                            composer.dictation_final(text, cx);
                        }
                    }
                });
                if sent {
                    this.dictation.release_to_send_ms = this
                        .dictation
                        .released_at
                        .map(|at| at.elapsed().as_millis() as u64);
                }
                if let Err(e) = text {
                    this.voice_error(&format!("voice failed: {e:#}"), cx);
                }
            });
        })
        .detach();
    }

    /// A microphone or speech-server failure belongs under the mic button,
    /// not in the transcript: the conversation did not fail, the take did.
    fn voice_error(&mut self, msg: &str, cx: &mut Context<Self>) {
        let note = msg
            .strip_prefix("voice failed: ")
            .unwrap_or(msg)
            .to_string();
        // The microphone was wanted and the system said no: after "Skip for
        // now" this is what lights the dot on the gear.
        if crate::voice_ws::mic_permission().advice().is_some() {
            self.permission_center.update(cx, |center, cx| {
                center.note_needed(crate::permissions::Permission::Microphone, cx)
            });
        }
        self.composer
            .update(cx, |composer, cx| composer.set_voice_note(Some(note), cx));
    }

    // ------------------------------------------------------------------ calls

    /// Whether a call can start: a speech server is set up and a project
    /// with a main chat is in front.
    pub(crate) fn can_call(&self, cx: &App) -> bool {
        crate::voice_ws::configured() && self.workspace.read(cx).active_id().is_some()
    }

    /// ⇧⌘C and the panel's handset: start a call to the project in front,
    /// or hang up the one that is live.
    pub(crate) fn start_call_action(
        &mut self,
        _: &StartCall,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.call.is_some() {
            self.end_call(cx);
        } else {
            self.start_call(cx);
        }
    }

    pub(crate) fn end_call_action(&mut self, _: &EndCall, _: &mut Window, cx: &mut Context<Self>) {
        self.end_call(cx);
    }

    pub(crate) fn toggle_mute_action(
        &mut self,
        _: &ToggleMute,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_mute(cx);
    }

    /// Call the project in front. The gateway opens a call session to that
    /// project's main agent; from then on the mic is open, the caller's
    /// words go to the agent as `voice` messages, and the narrator's
    /// highlights are spoken and written into the chat as `voice ·` lines.
    pub(crate) fn start_call(&mut self, cx: &mut Context<Self>) {
        if self.call.is_some() {
            return;
        }
        let workspace = self.workspace.read(cx);
        let Some(session) = workspace.active_id() else {
            return;
        };
        let Some(project) = workspace.active_project() else {
            return;
        };
        if !crate::voice_ws::configured() {
            self.voice_error(
                "no speech server: set voice_url (and voice_token) in ~/.config/arbos/config.toml to call a project",
                cx,
            );
            return;
        }
        let label = Workspace::tab_label(project);
        // What the gateway is told the call is for: the tab's hub name
        // (`mac/arbos`), so a gateway on another machine can attach to this
        // kernel through the hub; a gateway serving this very kernel takes
        // the folder's name as its own.
        let hub_name = kernel::hub_project_name(&project.place());
        // Dictation, if a take is open, ends: the call owns the mic.
        if self.composer.read(cx).is_recording() {
            self.stop_voice(cx);
        }
        self.call = Some(Call {
            session,
            label: label.clone(),
            since: std::time::Instant::now(),
            connecting: true,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let started = cx
                .background_executor()
                .spawn(async move { crate::voice_ws::call_start(&hub_name) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match started {
                    Ok(()) => {
                        if let Some(call) = this.call.as_mut() {
                            call.connecting = false;
                        }
                        this.workspace.update(cx, |workspace, cx| {
                            workspace.with_session(session, cx, |chat| {
                                chat.notice(false, "voice · call started");
                            });
                        });
                        this.start_call_mirror(cx);
                    }
                    Err(e) => {
                        this.call = None;
                        this.voice_error(&format!("call failed: {e:#}"), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Hang up. The gateway closes the session; the chat keeps the record.
    pub(crate) fn end_call(&mut self, cx: &mut Context<Self>) {
        let Some(call) = self.call.take() else {
            return;
        };
        crate::voice_ws::call_end();
        let mins = call.since.elapsed().as_secs() / 60;
        let secs = call.since.elapsed().as_secs() % 60;
        self.workspace.update(cx, |workspace, cx| {
            workspace.with_session(call.session, cx, |chat| {
                chat.notice(false, &format!("voice · call ended after {mins}:{secs:02}"));
            });
        });
        cx.notify();
    }

    pub(crate) fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if self.call.is_none() {
            return;
        }
        let muted = !crate::voice_ws::status().muted;
        if let Err(e) = crate::voice_ws::call_mute(muted) {
            self.voice_error(&format!("mute failed: {e:#}"), cx);
        }
        cx.notify();
    }

    /// While the call is live: the narrator's lines land in the call's chat
    /// as `voice ·` notices, the strip repaints, and a dropped session ends
    /// the call on this side too.
    fn start_call_mirror(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let lines = crate::voice_ws::drain_mirror();
                let status = crate::voice_ws::status();
                let live = crate::voice_ws::in_call();
                let keep = this.update(cx, |this, cx| {
                    let Some(call) = this.call.clone() else {
                        return false;
                    };
                    if !lines.is_empty() {
                        this.workspace.update(cx, |workspace, cx| {
                            workspace.with_session(call.session, cx, |chat| {
                                for m in &lines {
                                    if let Some(line) = call_line(m) {
                                        chat.notice(false, &line);
                                    }
                                }
                            });
                        });
                    }
                    if let Some(e) = status.error.as_deref()
                        && !live
                    {
                        this.voice_error(&format!("call dropped: {e}"), cx);
                    }
                    if !live && !call.connecting {
                        this.call = None;
                        this.workspace.update(cx, |workspace, cx| {
                            workspace.with_session(call.session, cx, |chat| {
                                chat.notice(false, "voice · call ended");
                            });
                        });
                    }
                    cx.notify();
                    this.call.is_some()
                });
                if !matches!(keep, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn open_project_action(
        &mut self,
        _: &OpenProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let recents = self.workspace.read(cx).recents.clone();
        self.opener
            .update(cx, |opener, cx| opener.show(recents, cx));
        window.focus(&self.opener.read(cx).focus_handle(cx), cx);
    }

    fn browse_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.offer_store_out_of_sync(
                    &crate::model::place::Place::local(path.clone()),
                    window,
                    cx,
                );
                this.workspace
                    .update(cx, |workspace, cx| workspace.open_project(path, cx));
                this.offer_tab_face(window, cx);
            });
        })
        .detach();
    }

    /// A folder inside iCloud / a file-provider sync: reads of `.arbos/`
    /// there block for minutes on items the provider has not downloaded,
    /// and the kernel stalls with them. Offer to keep the store out of the
    /// sync before the kernel starts: `.arbos.nosync` beside the project
    /// (iCloud skips `*.nosync`) with `.arbos` a symlink to it, or a folder
    /// under `~/.arbos/stores/`. Nothing when the store is already out.
    fn offer_store_out_of_sync(
        &mut self,
        place: &crate::model::place::Place,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if place.is_remote() {
            return;
        }
        let path = place.path.clone();
        let Some(sync) = arbos_core::cloudsync::detect(&path) else {
            return;
        };
        if arbos_core::cloudsync::settled(&path) {
            return;
        }
        let detail = format!(
            "{} is inside {} sync. Reads of its .arbos folder can block for minutes on items {} has not downloaded, and Arbos would stall with them.\n\nKeep the store out of the sync? \"Beside the project\" renames .arbos to .arbos.nosync (which iCloud skips) and leaves .arbos as a link to it. \"In ~/.arbos/stores\" moves it out of the folder entirely.",
            path.display(),
            sync.label(),
            sync.label()
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            "This folder is synced by iCloud",
            Some(&detail),
            &[
                "Beside the project (.arbos.nosync)",
                "In ~/.arbos/stores",
                "Leave as is",
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = answer.await else {
                return;
            };
            let how = match choice {
                0 => arbos_core::cloudsync::Relocation::Nosync,
                1 => arbos_core::cloudsync::Relocation::Home,
                _ => return,
            };
            let outcome = arbos_core::cloudsync::relocate(&path, how);
            let _ = this.update_in(cx, |_this, window, cx| {
                let (title, detail) = match &outcome {
                    Ok(target) => (
                        "Store moved",
                        format!(".arbos is now a link to {}.", target.display()),
                    ),
                    Err(e) => ("Could not move the store", format!("{e:#}")),
                };
                let _ = window.prompt(PromptLevel::Info, title, Some(&detail), &["OK"], cx);
            });
        })
        .detach();
    }

    /// A folder opened for the first time has no `project.toml`: offer the
    /// sheet with the folder's own defaults filled in. A folder that has
    /// one comes back wearing it, no questions.
    /// The permissions sheet, the first time only. Skip for now marks it
    /// seen; after that only the dot on the gear says a grant is wanted.
    fn permissions_once(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.read(cx).permissions_seen {
            return;
        }
        self.show_permissions(window, cx);
    }

    /// The permissions sheet over the chat, for the open project's folder.
    pub(crate) fn show_permissions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let project = self
            .workspace
            .read(cx)
            .active_project()
            .filter(|project| !project.is_remote())
            .map(|project| project.path.clone());
        self.permission_center
            .update(cx, |center, cx| center.set_project(project, cx));
        self.permissions_sheet
            .update(cx, |sheet, cx| sheet.show(window, cx));
    }

    pub(crate) fn show_permissions_action(
        &mut self,
        _: &ShowPermissions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_permissions(window, cx);
    }

    fn offer_tab_face(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fresh = self
            .workspace
            .read(cx)
            .active_project()
            .is_some_and(|project| !project.identity_saved);
        if let (true, Some(ix)) = (fresh, self.workspace.read(cx).active) {
            self.edit_tab(ix, window, cx);
        }
    }

    /// The sheet, on the tab at `ix`: its name, glyph and colour.
    pub(crate) fn edit_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((identity, fallback)) = self.workspace.read(cx).projects.get(ix).map(|project| {
            let mut identity = project.identity.clone();
            // The Home tab's first offer comes prefilled, so Keep as-is
            // names it "Home" in its project.toml.
            if Workspace::is_home(project) && !project.identity_saved && identity.name.is_none() {
                identity.name = Some("Home".into());
            }
            (identity, Workspace::tab_label(project))
        }) else {
            return;
        };
        self.dismiss_menu(cx);
        self.tab_sheet
            .update(cx, |sheet, cx| sheet.show(ix, &identity, fallback, cx));
        window.focus(&self.tab_sheet.read(cx).focus_handle(cx), cx);
    }

    /// Which pane is on screen, as against [`Self::pane`], which is the one
    /// asked for. They part when what it points at is gone — deleted, switched
    /// off, or in a project that has none open — and whatever the project does
    /// have stands in, so a launch lands on the entry it was left on rather
    /// than on an empty conversation. The panel reads this, not the request:
    /// a row lit for a pane nobody can see is the second selection the eye
    /// finds.
    ///
    /// `None` is the launch view. A pane exists only where something is open in
    /// it, so with nothing open there is no pane to name — least of all the
    /// chat, which under the shipped defaults is itself switched off.
    pub(crate) fn showing(&self, cx: &App) -> Option<Pane> {
        if self.has_pane(self.pane, cx) {
            return Some(self.pane);
        }
        [Pane::Chat, Pane::Surface, Pane::Project]
            .into_iter()
            .find(|&pane| self.has_pane(pane, cx))
    }

    /// Whether the active project has anything open in `pane`.
    pub(crate) fn has_pane(&self, pane: Pane, cx: &App) -> bool {
        let workspace = self.workspace.read(cx);
        match pane {
            Pane::Chat => {
                workspace.active_session().is_some()
                    && workspace
                        .active_project()
                        .and_then(|project| project.focus)
                        .is_some_and(|focus| focus.surface.is_none())
            }
            Pane::Surface => workspace.active_surface().is_some(),
            // Every open project has a page, written or not.
            Pane::Project => workspace.active_project().is_some(),
        }
    }

    /// Nothing is open, so there is nowhere to send a prompt — the only thing
    /// on offer is a folder.
    pub(crate) fn no_project(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let painter = Painter::of(cx);
        theme
            .empty_state(
                icons::files::FOLDER,
                "No tab open",
                "A tab is a folder on this Mac, or host:folder over ssh.",
            )
            .flex_1()
            .child(
                theme
                    .button(
                        "New tab…",
                        ButtonStyle::Prominent,
                        Some(Fade::new(painter, "open-project-empty")),
                    )
                    .id("open-project-empty")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.new_tab_action(&NewTab, window, cx);
                    })),
            )
            .into_any_element()
    }
}

impl Render for Arbos {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_terminal(window, cx);
        let theme = Theme::of(cx).clone();
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .font_family(theme.font_sans.clone())
            .text_color(theme.text)
            .text_style(TextStyle::Body)
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::copy_chat))
            .on_action(cx.listener(Self::paste_chat))
            .on_action(cx.listener(Self::delete_chat))
            .on_action(cx.listener(Self::commit_name))
            .on_action(cx.listener(Self::dismiss_name))
            .on_action(cx.listener(Self::dismiss_menu_action))
            // Everything the menu bar names, and only under the conditions
            // that keep its items honest.
            .map(|root| self.commands(root, cx))
            // An action reaches the handlers above only through the focused
            // element's ancestors. Sized at nothing, so the pane that does hold
            // a field keeps its focus through a click anywhere else.
            .child(div().key_context(WINDOW_CONTEXT).track_focus(&self.focus))
            // The strip of tabs across the top, then the chat column with
            // the panel on its right.
            .child(self.tab_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_row()
                    .child(self.detail(window, cx))
                    .children(self.panel(window, cx)),
            )
            .children(
                self.workspace
                    .read(cx)
                    .meter
                    .then(|| meter::panel("app-meter", &self.meter_at, &self.meter, window)),
            )
            .child(self.opener.clone())
            .child(self.tab_sheet.clone())
            .child(self.permissions_sheet.clone())
            .child(self.chat_search.clone())
    }
}

/// The last dictated take's clock. `pressed_at` is the Fn press; the
/// first partial and the release-to-send times are what the driver shows
/// as `voice_latency`, so QA can measure "instant" instead of feeling it.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Dictation {
    pub pressed_at: Option<Instant>,
    pub released_at: Option<Instant>,
    pub first_partial_ms: Option<u64>,
    pub release_to_send_ms: Option<u64>,
}

/// Native Spaces fullscreen is a black desktop, so the frost has nothing to
/// blur. Stay on this Space; the green button zooms. Also put the transparent
/// titlebar back — macOS 15.3+ clears it on the way into fullscreen.
#[cfg(target_os = "macos")]
fn keep_macos_glass(_window: &Window) {
    use objc::{
        class, msg_send,
        runtime::{Object, YES},
        sel, sel_impl,
    };

    const FULL_SCREEN_PRIMARY: usize = 1 << 7;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;
    const FULL_SCREEN_NONE: usize = 1 << 9;

    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let windows: *mut Object = msg_send![app, windows];
        let count: usize = msg_send![windows, count];
        for i in 0..count {
            let ns_window: *mut Object = msg_send![windows, objectAtIndex: i];
            if ns_window.is_null() {
                continue;
            }
            let behavior: usize = msg_send![ns_window, collectionBehavior];
            if behavior & FULL_SCREEN_AUXILIARY != 0 {
                continue;
            }
            let _: () = msg_send![ns_window, setTitlebarAppearsTransparent: YES];
            let behavior = (behavior & !FULL_SCREEN_PRIMARY) | FULL_SCREEN_NONE;
            let _: () = msg_send![ns_window, setCollectionBehavior: behavior];
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn keep_macos_glass(_window: &Window) {}

/// The window's own chrome in the theme's colour. AppKit paints the title
/// band (the traffic lights' strip) from the `NSWindow`'s background colour
/// and its appearance, not from what we draw under it, so a dark palette
/// over a window left at AppKit's defaults kept a white band across the
/// top. Every window takes the chrome surface as its background — clear
/// where glass is on, so the frost still shows — and the palette's
/// appearance, so the lights and the band are drawn for dark.
#[cfg(target_os = "macos")]
fn sync_macos_chrome(cx: &App) {
    use objc::{
        class, msg_send,
        runtime::{Object, YES},
        sel, sel_impl,
    };
    let theme = Theme::of(cx);
    let chrome = chrome_bg(theme);
    let rgba = chrome.to_rgb();
    let dark = theme.appearance == bezel::theme::Appearance::Dark;
    unsafe {
        let name: *mut Object = msg_send![
            class!(NSString),
            stringWithUTF8String: if dark {
                c"NSAppearanceNameDarkAqua".as_ptr()
            } else {
                c"NSAppearanceNameAqua".as_ptr()
            }
        ];
        let appearance: *mut Object = msg_send![class!(NSAppearance), appearanceNamed: name];
        let color: *mut Object = if theme.vibrancy {
            msg_send![class!(NSColor), clearColor]
        } else {
            msg_send![
                class!(NSColor),
                colorWithSRGBRed: rgba.r as f64
                green: rgba.g as f64
                blue: rgba.b as f64
                alpha: 1.0f64
            ]
        };
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let windows: *mut Object = msg_send![app, windows];
        let count: usize = msg_send![windows, count];
        for i in 0..count {
            let ns_window: *mut Object = msg_send![windows, objectAtIndex: i];
            if ns_window.is_null() {
                continue;
            }
            let _: () = msg_send![ns_window, setAppearance: appearance];
            let _: () = msg_send![ns_window, setBackgroundColor: color];
            let _: () = msg_send![ns_window, setTitlebarAppearsTransparent: YES];
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn sync_macos_chrome(_cx: &App) {}

/// One `voice ·` line for the chat during a call: what the narrator said,
/// marked by kind so a question or a failure reads as one. Everything else
/// the mirror carries (the agent bridge) is already in the chat, which is
/// attached to the same kernel: nothing.
fn call_line(m: &crate::voice_ws::Mirror) -> Option<String> {
    let text: String = m.text.split_whitespace().collect::<Vec<_>>().join(" ");
    match m.kind.as_str() {
        // "On it." is heard, not read: the caller's own line is the record.
        "narrator.say/ack" => None,
        "narrator.say/report" => Some(format!("voice · {text}")),
        "narrator.say/ask" => Some(format!("voice · asked: {text}")),
        "narrator.say/error" => Some(format!("voice · {text}")),
        "narrator.say/detail" => Some(format!("voice · detail: {text}")),
        k if k.starts_with("narrator.say") => Some(format!("voice · {text}")),
        _ => None,
    }
}

/// One notice line for a mirrored speech-server event: who, what, words.
fn mirror_line(m: &crate::voice_ws::Mirror) -> String {
    let who = if m.agent.is_empty() {
        "voice".to_string()
    } else {
        format!("voice · {}", m.agent)
    };
    let text: String = m.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = if text.chars().count() > 240 {
        text.chars().take(240).collect::<String>() + "…"
    } else {
        text
    };
    match m.kind.as_str() {
        "text.done" => format!("{who} answered: {text}"),
        "agent.done" => format!("{who} finished: {text}"),
        "agent.turn" => format!("{who} is {text}"),
        "tool.call" => format!("{who} runs {text}"),
        "tool.result" => format!("{who} got {text}"),
        k if k.starts_with("agent.event/") => {
            format!("{who} {}: {text}", k.trim_start_matches("agent.event/"))
        }
        _ => format!("{who}: {text}"),
    }
}
