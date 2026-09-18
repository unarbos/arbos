//! Root view: the window's grid, the state the chrome owns, and the frame
//! the tab bar, the chat column and the right-hand panel are hung in.

use crate::{
    kernel,
    model::{
        panel::{Panel, PanelTab},
        permission_center::{PermissionCenter, Permissions},
        session::ChatSession,
        settings::Settings,
        state::{self, State},
        surface::SurfaceId,
        workspace::{PaneRequest, PtyWrote, Reloaded, Workspace},
    },
    view::{
        component::{
            chat_search::{ChatSearch, ChatSearchEvent, Hit, PaletteAction},
            composer::{Composer, ComposerEvent, VoiceState},
            feedback_sheet::{FeedbackSheet, FeedbackSheetEvent, Unavailable},
            menu::Menu,
            meter,
            opener::{Opener, OpenerEvent},
            permissions_sheet::{PermissionsSheet, PermissionsSheetEvent},
            tab_sheet::{TabSheet, TabSheetEvent},
        },
        naming::Renaming,
        settings::{CloseSettings, Section, SettingsPane},
        terminal::{TerminalEvent, TerminalPane},
    },
};
use anyhow::Result;
use bezel::{
    gpui::{
        self, AnyElement, App, Bounds, Context, Entity, FocusHandle, Focusable as _, Hsla,
        KeyBinding, PathPromptOptions, Pixels, PromptLevel, Render, Task, TitlebarOptions, Window,
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
        ReportProblem,
        TogglePanel,
        ZoomPanel,
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

/// Report a problem with one answer: the thumbs-down under it opens the
/// review sheet anchored on that exchange. `seq` is the transcript line of
/// the prompt that began the turn; without one the sheet takes the latest
/// exchange, as Help › Report a Problem… does.
#[derive(Clone, PartialEq, serde::Deserialize, schemars::JsonSchema, gpui::Action)]
#[action(namespace = arbos)]
pub struct ReportProblemAt {
    pub seq: Option<u64>,
}

/// Claimed on the window's rest focus so Delete/Backspace archive the
/// highlighted chat when no field is in front.
const WINDOW_CONTEXT: &str = "ArbosWindow";

/// Claimed on the rename field so `enter` files the name and `escape` drops it.
const RENAME_CONTEXT: &str = "ArbosSessionName";

/// The side panel's key context. Nothing is bound on it: it exists so the
/// panel's own focus is a fact the tab chords can read.
pub(crate) const PANEL_CONTEXT: &str = "ArbosPanel";

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

/// How thick the window's one material sits. Nothing paints beneath it, so
/// this is absolute: thick enough for the chat's text to win against the
/// desktop, and the chrome takes the same so the strips are not bands.
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
/// Cursor's chat prose is San Francisco at 14 px on a 23 px line (measured
/// 14/22 on Jacob's Mac). Ours is Inter (`crate::fonts`), whose lowercase
/// is 7 % taller at the same size: x-height 0.546 em against SF's 0.508.
/// 13 px Inter puts the lowercase where Cursor's is — rendered at 2x, 15 px
/// x-height and 20 px caps against SF 14's 14.2 and 19.7; at 14 px Inter
/// it would be 16 and 21, almost a pixel over at 1x. The 23 px line box is
/// kept: the line pitch is what the eye compares across the two windows.
pub(crate) const CURSOR_PROSE_SIZE: f32 = 13.;
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

/// The chrome's fill — the tab bar, the bar under the window, the panel,
/// and on macOS the title band. One surface with the content: Jacob, from
/// his Mac (09-16), "the top bar and the bottom bar can't be seen as a
/// separation" — Cursor's strips are the chat's own background with the
/// controls floating in it, no tray, no edge; the separation is spacing.
/// (Before this the chrome took the thinner material and, without
/// vibrancy, the light palette's `surface` grey around the content's white.)
pub(crate) fn chrome_bg(theme: &Theme) -> Hsla {
    content_bg(theme)
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

/// Leading room on the tab strip. In a Space the traffic lights hide with
/// the menu bar; keeping their inset would be an empty hole at the top.
pub(crate) fn toolbar_inset(window: &Window) -> f32 {
    if window.is_fullscreen() || window.is_simple_fullscreen() {
        HEADER_INSET
    } else {
        TOOLBAR_INSET
    }
}

pub fn init(cx: &mut App) {
    crate::view::terminal::init(cx);
    crate::view::component::feedback_sheet::init(cx);
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
    bezel::ui::tree::init(cx);
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
        // Report a problem, from wherever he is. The moment he notices is
        // the moment he will use it, so it answers app-wide rather than only
        // where a turn footer happens to be on screen.
        KeyBinding::new("cmd-shift-r", ReportProblem, None),
        // What every app with a side panel binds it to. It is claimed
        // app-wide: the menu item carries it, so AppKit takes the chord
        // before the window is offered it, and the editor's own `cmd-b` —
        // bold — is not reached while this one is on the bar.
        KeyBinding::new("cmd-b", TogglePanel, None),
        KeyBinding::new("cmd-\\", ZoomPanel, None),
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
        // And in the side panel, where Escape is one of its three ways back.
        // An action reaches a handler only through the focused element's own
        // ancestors, and the panel's focus is not under `WINDOW_CONTEXT`, so
        // without this line Escape in the drawer went nowhere at all.
        KeyBinding::new("escape", DismissMenu, Some(PANEL_CONTEXT)),
        KeyBinding::new("enter", CommitName, Some(RENAME_CONTEXT)),
        KeyBinding::new("escape", DismissName, Some(RENAME_CONTEXT)),
    ]);
    crate::view::bind_field_editing(cx, RENAME_CONTEXT, false);
}

/// The window's resting size: what an un-maximized window comes back to,
/// and the floor a restored frame is clamped to — macOS can hand back a
/// last-used frame smaller than `window_min_size`.
const WINDOW_WIDTH: f32 = 1100.;
const WINDOW_HEIGHT: f32 = 761.;
/// The title the macOS window-restore check matches; only that check
/// reads it, so Linux would otherwise warn it is unused (#486 deleted it
/// on that warning and broke the macOS build).
#[cfg(target_os = "macos")]
const WINDOW_TITLE: &str = "Arbos";

fn restore_usable_bounds(window: &mut Window) {
    // Native fullscreen owns the frame. Pushing a saved size at it
    // during the Space transition fights AppKit.
    if window.is_fullscreen() || window.is_simple_fullscreen() {
        return;
    }
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
            // NSWindowStyleMaskFullScreen. A Space already owns this
            // window; do not write a windowed frame over it.
            const STYLE_MASK_FULL_SCREEN: usize = 1 << 14;
            let style_mask: usize = msg_send![ns_window, styleMask];
            if style_mask & STYLE_MASK_FULL_SCREEN != 0 {
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

/// Fill the screen the window came up on, unless it already fills it.
///
/// Opening with [`WindowBounds::Maximized`] is an ask, and a window
/// manager may drop it: an X11 one ignores the `_NET_WM_STATE` message
/// sent before the window is mapped, and the window then stands at its
/// resting size. Asking again once it is on screen is what makes the
/// first launch filled on every platform. A window that is already filled
/// — or in a fullscreen of its own — is left alone, because the ask is a
/// toggle and would shrink it.
fn fill_screen(window: &mut Window, cx: &App) {
    if window.is_fullscreen() || window.is_simple_fullscreen() {
        return;
    }
    let frame = window.bounds();
    let centre = point(
        frame.origin.x + frame.size.width / 2.,
        frame.origin.y + frame.size.height / 2.,
    );
    let Some(screen) = cx
        .displays()
        .into_iter()
        .map(|display| display.bounds())
        .find(|screen| screen.contains(&centre))
    else {
        return;
    };
    // Not "the same size": a maximized window stops short of the menu
    // bar, the dock, or a desktop panel, so a filled one is merely most
    // of the screen.
    let filled = f32::from(frame.size.width) >= 0.9 * f32::from(screen.size.width)
        && f32::from(frame.size.height) >= 0.8 * f32::from(screen.size.height);
    if !filled {
        window.zoom_window();
    }
}

/// Whether at least 70 % of `frame` lies on one of the displays gpui
/// knows: the window can be seen and reached. Every platform.
fn frame_mostly_on_a_display(frame: &Bounds<Pixels>, cx: &App) -> bool {
    let area = f32::from(frame.size.width) * f32::from(frame.size.height);
    if area <= 0. {
        return false;
    }
    cx.displays().iter().any(|display| {
        let screen = display.bounds();
        let x0 = f32::from(frame.origin.x).max(f32::from(screen.origin.x));
        let y0 = f32::from(frame.origin.y).max(f32::from(screen.origin.y));
        let x1 = (f32::from(frame.origin.x) + f32::from(frame.size.width))
            .min(f32::from(screen.origin.x) + f32::from(screen.size.width));
        let y1 = (f32::from(frame.origin.y) + f32::from(frame.size.height))
            .min(f32::from(screen.origin.y) + f32::from(screen.size.height));
        x1 > x0 && y1 > y0 && (x1 - x0) * (y1 - y0) >= 0.7 * area
    })
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
    // Where the window was last time, when most of that frame is still on
    // a screen; otherwise centred on the main display (a fresh install, a
    // display that is gone, a frame dragged off the edge — Jacob saw only
    // the left 40 % of one).
    let bounds = state
        .frame
        .map(|[x, y, w, h]| Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w.max(600.)), px(h.max(320.))),
        })
        .filter(|frame| frame_mostly_on_a_display(frame, cx))
        .unwrap_or_else(|| Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx));
    let handle = cx.open_window(
        WindowOptions {
            // Filled screen on open (Jacob, 09-18). `bounds` is what the
            // window goes back to when it is un-maximized: the frame it
            // was left at, else the resting size. A maximized window is
            // never written back as that frame (see `observe_window_bounds`),
            // so every launch starts filled.
            window_bounds: Some(WindowBounds::Maximized(bounds)),
            // No strip of its own: the traffic lights sit in the nav, so the
            // window owes no titlebar above it.
            titlebar: Some(TitlebarOptions {
                title: Some("Arbos".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(TRAFFIC_LIGHT_X), px(TRAFFIC_LIGHT_Y))),
                ..Default::default()
            }),
            // We draw the tab strip; AppKit must not reserve a second title
            // band or delay clicks on it. In a Space that leftover band is
            // the gap #542 left.
            app_owns_titlebar_drag: true,
            // Glass needs a blurred window background to blur into.
            window_background: Theme::of(cx).window_background_appearance(),
            window_min_size: Some(size(px(600.), px(320.))),
            app_id: Some("arbos-desktop".into()),
            ..Default::default()
        },
        |window, cx| {
            appearance::observe_window(window, cx).detach();
            cx.new(|cx| Arbos::new(settings, state, window, cx))
        },
    )?;
    // macOS can apply a saved frame after the first paint — a 100×131
    // leftover from a prior session. Repair that once, and fill the screen
    // if the ask above did not take.
    let again = handle.clone();
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(300))
            .await;
        let _ = again.update(cx, |_, window, cx| {
            restore_usable_bounds(window);
            fill_screen(window, cx);
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
    /// Kept so old requests compile. The page lives in the right panel
    /// and never takes this column.
    Project,
}

/// Which tab of the strip the window's middle draws. Everywhere else in the
/// app a tab is a project; Settings is the one tab that is not, so this is the
/// one place that says which kind is in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Front {
    /// The project `Workspace::active` points at, in whichever [`Pane`] it was
    /// left on.
    Project,
    /// The Settings tab, filling the width.
    Settings,
}

/// The Settings tab while it is in the strip: the pane that draws it, and
/// whether it is the tab in front.
///
/// One field holds both, so nothing can claim Settings is in front while no
/// tab holds it: closing the tab is dropping this whole value, and the project
/// underneath is untouched — which is why leaving Settings needs no memory of
/// where to go back to.
pub(crate) struct SettingsTab {
    pub(crate) pane: Entity<SettingsPane>,
    front: bool,
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

/// How long after opening a window activation still counts as the launch.
const LAUNCH_SETTLE: std::time::Duration = std::time::Duration::from_secs(5);

/// One OS notification the window asked the platform to show.
#[derive(Debug, Clone)]
pub(crate) struct PostedNotification {
    pub at: i64,
    pub title: String,
    pub body: String,
    /// The command could not be started; `None` when it was.
    pub error: Option<String>,
}

pub struct Arbos {
    pub(crate) workspace: Entity<Workspace>,
    /// Whether this window is the active one on the desktop: a kernel
    /// notification for a chat the person is looking at is seen at once;
    /// one for a chat they are not goes to the OS as a notification.
    pub(crate) window_active: bool,
    /// The person has done something in this window since it opened — a
    /// press, a key, or coming back to it after leaving. Until then a chat
    /// restored in front is on screen but not yet looked at, and its unseen
    /// replies keep their dot (qal-j03): a reply that landed while the app
    /// was shut is exactly the one a person needs telling about.
    pub(crate) touched: bool,
    /// When the window opened: activations in its first seconds are the
    /// launch settling (X11 focuses a new window more than once), not a
    /// return to it.
    launched_at: std::time::Instant,
    /// OS notifications this window posted (#293), newest last, capped:
    /// what the driver shows a test so "an alert was posted" is a fact it
    /// can read and check against the daemon, not a belief.
    pub(crate) notifications_posted: Vec<PostedNotification>,
    /// The open session menu was opened from the chat header's `⋯`, so it
    /// anchors there rather than at a panel row.
    pub(crate) menu_at_header: bool,
    /// The git branch of the last local place looked at, for the project
    /// panel header. `(place path, branch or none)`. The composer no
    /// longer shows a branch chip.
    pub(crate) branch_cache: std::cell::RefCell<Option<(std::path::PathBuf, Option<String>)>>,
    pub(crate) terminals: std::collections::HashMap<String, Entity<TerminalPane>>,
    active_terminal: Option<String>,
    /// Text files open in the drawer. Keyed by the resolved path so a
    /// second open of the same file keeps the buffer.
    pub(crate) file_editors:
        std::collections::HashMap<std::path::PathBuf, Entity<crate::view::file_editor::FileDoc>>,
    /// The side panel's own focus. Two rows of tabs are on screen and one
    /// pair of chords drives both — `⌘T`, `⌘⇧{`, `⌘⇧}` act on the panel's
    /// tabs while this holds the focus and on the window's projects
    /// otherwise — so which row is lit and which row moves are the same
    /// fact, read from here.
    pub(crate) panel_focus: FocusHandle,
    /// The panel's "N archived" row is unfolded: finished workers the
    /// kernel moved to `archive/agents/` are listed, faint.
    pub(crate) archived_open: bool,
    /// The root chat whose idle "Agents" card is open (Cursor's Agents pill
    /// once the workers are done).
    pub(crate) agents_card_open: Option<u64>,
    pub(crate) composer: Entity<Composer>,
    pub(crate) opener: Entity<Opener>,
    /// The sheet a tab's name, glyph and colour are set in.
    pub(crate) tab_sheet: Entity<TabSheet>,
    pub(crate) feedback_sheet: Entity<FeedbackSheet>,
    /// What the last outbox pass found. Read by the driver so the loop can
    /// assert the state he is in without photographing the window for it.
    pub(crate) feedback_outbox: OutboxState,
    /// Whether any chat held a live kernel on the last look — the edge
    /// `drain_on_reconnect` watches for.
    was_connected: bool,
    /// The exchange the open report is about — chat id and the prompt's
    /// `seq` — so a sent report can leave its mark on that prompt's footer.
    report_anchor: Option<(u64, u64)>,
    pub(crate) permissions_sheet: Entity<PermissionsSheet>,
    pub(crate) permission_center: Entity<PermissionCenter>,
    /// ⌘K: the palette over every open tab's chats.
    pub(crate) chat_search: Entity<ChatSearch>,
    /// The Settings tab, or nothing when it is not open. It is not remembered
    /// across a launch: a tab in this strip is a place with a kernel and a
    /// chat, `state.toml` restores those, and a relaunch that landed on a
    /// preferences form instead of the work would be the wrong side of the
    /// trade — ⌘, is one chord away.
    pub(crate) settings_tab: Option<SettingsTab>,
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
    /// What the bar along the bottom draws: which build this is, and whether
    /// the channel has a newer one. See [`crate::update`].
    pub(crate) updater: Entity<crate::update::Updater>,
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
        let active = {
            let workspace = self.workspace.read(cx);
            self.terminals.retain(|id, _| {
                workspace.projects.iter().any(|project| {
                    project
                        .surfaces
                        .iter()
                        .any(|surface| surface.terminal_id() == Some(id.as_str()))
                })
            });
            // Which terminal wants a live pane: the one the side panel is
            // showing, or the one in the column when a tab has been zoomed
            // there. Before the drawer existed only the column could hold
            // one, and a terminal opened into the panel drew "This terminal
            // has no session" for ever.
            let in_panel = match workspace.panel().map(Panel::active_tab) {
                Some(PanelTab::Surface(id)) => workspace
                    .active_project()
                    .and_then(|project| project.surface(id))
                    .and_then(|surface| surface.terminal_id())
                    .map(str::to_owned),
                Some(PanelTab::Project | PanelTab::New(_)) | None => None,
            };
            in_panel.or_else(|| {
                workspace
                    .active_surface()
                    .and_then(|surface| surface.terminal_id())
                    .map(str::to_owned)
            })
        };
        let active_id = active.clone();
        let zoomed = self.pane == Pane::Surface;
        if let Some(id) = active {
            let pane = match self.terminals.get(&id) {
                Some(pane) => pane.clone(),
                None => {
                    let pane = cx.new(|cx| TerminalPane::new(id.clone(), cx));
                    // The keys go out on the connection the window already
                    // holds. Nothing of the pane's own reaches the kernel.
                    cx.subscribe(&pane, |this, pane, event: &TerminalEvent, cx| {
                        let TerminalEvent::Input(bytes) = event;
                        let page = pane.read(cx).id().to_owned();
                        let bytes = bytes.clone();
                        this.workspace
                            .update(cx, |workspace, _| workspace.pty_send(&page, bytes));
                    })
                    .detach();
                    self.terminals.insert(id.clone(), pane.clone());
                    pane
                }
            };
            self.feed_terminal(&id, cx);
            // The column takes the caret with it, since zooming a terminal is
            // an act of sitting down at it. In the drawer nothing takes the
            // focus but a click, as everywhere else here.
            if zoomed && self.active_terminal != active_id {
                window.focus(&pane.focus_handle(cx), cx);
            }
        }
        self.active_terminal = active_id;
        self.sync_file_editors(cx);
    }

    /// Hand a pane everything its shell has written since it last looked.
    ///
    /// Called when the pane is built — where the bytes handed over are the
    /// whole scrollback, prompt and all, written before the tab existed —
    /// and on every `PtyWrote` after that. Only the pane repaints: the
    /// output of a running build must not redraw the window around it.
    pub(crate) fn feed_terminal(&mut self, page: &str, cx: &mut Context<Self>) {
        let Some(pane) = self.terminals.get(page).cloned() else {
            return;
        };
        let cursor = pane.read(cx).cursor();
        let (held, live, note) = {
            let workspace = self.workspace.read(cx);
            (
                workspace.pty_since(page, cursor),
                workspace.pty_live(page),
                workspace.pty_note(page),
            )
        };
        pane.update(cx, |pane, cx| {
            if let Some((bytes, cursor)) = held {
                pane.feed(&bytes, cursor, cx);
            }
            pane.set_state(live, note, cx);
        });
    }

    fn sync_file_editors(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace.read(cx);
        let needed: Vec<std::path::PathBuf> = workspace
            .projects
            .iter()
            .flat_map(|project| {
                project.surfaces.iter().filter_map(|surface| {
                    let path = surface.path()?;
                    let resolved = if path.is_absolute() {
                        path.to_path_buf()
                    } else {
                        project.path.join(path)
                    };
                    crate::view::file_editor::is_editable(&resolved).then_some(resolved)
                })
            })
            .collect();
        let dropping: Vec<_> = self
            .file_editors
            .keys()
            .filter(|path| !needed.iter().any(|held| held == *path))
            .cloned()
            .collect();
        if !dropping.is_empty() {
            let workspace = self.workspace.read(cx);
            for path in &dropping {
                workspace.claim_path(path, false);
            }
        }
        self.file_editors
            .retain(|path, _| needed.iter().any(|held| held == path));
        let workspace = self.workspace.clone();
        for path in needed {
            self.file_editors.entry(path.clone()).or_insert_with(|| {
                let workspace = workspace.clone();
                cx.new(|cx| crate::view::file_editor::FileDoc::new(path, workspace, cx))
            });
        }
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
        let feedback_sheet = cx.new(FeedbackSheet::new);
        cx.subscribe_in(
            &feedback_sheet,
            window,
            |this, sheet, event: &FeedbackSheetEvent, window, cx| match event {
                FeedbackSheetEvent::Send(draft) => this.write_report(sheet.clone(), draft, cx),
                FeedbackSheetEvent::Dismissed => this.focus_composer(window, cx),
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
                // The palette's actions are the menubar's, dispatched so
                // the one handler each has stays the one handler.
                ChatSearchEvent::Action(action) => {
                    let action: Box<dyn gpui::Action> = match action {
                        PaletteAction::NewTab => Box::new(NewTab),
                        PaletteAction::OpenFolder => Box::new(OpenProject),
                        PaletteAction::ProjectPage => Box::new(ShowProject),
                        PaletteAction::Settings => Box::new(OpenSettings),
                        PaletteAction::ReportProblem => Box::new(ReportProblem),
                    };
                    window.dispatch_action(action, cx);
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
                // The handset beside the mic: the same toggle ⇧⌘C is.
                ComposerEvent::Call => this.start_call_action(&StartCall, window, cx),
                ComposerEvent::Attach => {}
                ComposerEvent::Step(step) => this.cycle_entry(*step, window, cx),
                ComposerEvent::Delete => this.delete_highlighted(cx),
            },
        )
        .detach();

        let name_field = name_field_entity(false, cx);
        // Read before the settings are handed to the workspace, and used to
        // start the bar's updater below.
        let channel = crate::update::channel_of(&settings);
        let workspace = cx.new(|cx| Workspace::new(settings, state, cx));
        // The model is the only thing that says a session appeared or a turn
        // ended; the composer's placeholder, commands and busy state are all
        // read back from it rather than pushed by whoever caused the change.
        // A shell's output goes to its own pane and no further. Everything
        // else the model reports comes through `observe` below and repaints
        // the window; a terminal streaming a build would repaint it a
        // hundred times a second for output nothing else on screen shows.
        cx.subscribe(&workspace, |this, _, event: &PtyWrote, cx| {
            this.feed_terminal(&event.0, cx);
        })
        .detach();
        cx.observe(&workspace, |this, _, cx| {
            this.sync_composer(cx);
            this.reap_notifications(cx);
            this.collect_feedback(cx);
            this.drain_on_reconnect(cx);
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
                // A surface belongs in the right panel. Putting it in the
                // column covered the chat — the same fault as the Project
                // page (#629).
                PaneRequest::Surface(_) => {}
                PaneRequest::Chat => this.set_pane(Pane::Chat, cx),
            },
        )
        .detach();

        // The updater is the window's, so closing the window stops it looking.
        let updater = cx.new(move |cx| crate::update::Updater::new(channel, cx));
        crate::view::status_bar::observe(cx, &updater);
        // Settings is its own window and shows what the updater knows, so the
        // entity is reachable from there the way the permission centre is.
        cx.set_global(crate::update::Updates(updater.clone()));

        let mut this = Self {
            meter: cx.new(Stats::new),
            meter_at: Floating::new(Painter::of(cx)),
            updater,
            workspace,
            terminals: Default::default(),
            file_editors: Default::default(),
            active_terminal: None,
            window_active: true,
            notifications_posted: Vec::new(),
            touched: false,
            launched_at: std::time::Instant::now(),
            panel_focus: cx.focus_handle(),
            archived_open: false,
            agents_card_open: None,
            composer,
            opener,
            tab_sheet,
            feedback_sheet,
            feedback_outbox: OutboxState::default(),
            was_connected: false,
            report_anchor: None,
            permissions_sheet,
            permission_center,
            chat_search,
            settings_tab: None,
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
            // The Settings tab goes with the window, and what it had running —
            // the permission poll, a microphone test — goes with it.
            if let Some(tab) = this.settings_tab.take() {
                tab.pane.update(cx, |pane, cx| pane.went_behind(cx));
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
            this.window_active = window.is_window_active();
            if this.window_active {
                this.workspace
                    .update(cx, |workspace, cx| workspace.reload_projects(cx));
                // Coming back to the window is looking at its chat; the
                // launch's own activations are not.
                if this.launched_at.elapsed() > LAUNCH_SETTLE {
                    this.touched = true;
                }
                this.reap_notifications(cx);
            } else {
                this.flush_composer_draft(cx);
            }
        })
        .detach();
        // Entering or leaving a size — zoom, native fullscreen — can drop the
        // blur view and the transparent titlebar. Put both back, and keep
        // Spaces fullscreen on: a style-mask change resets that too.
        cx.observe_window_bounds(window, |this, window, cx| {
            appearance::reapply_window_background(cx);
            keep_macos_glass(window);
            sync_macos_chrome(cx);
            restore_usable_bounds(window);
            // Remember the frame for the next launch (not a fullscreen one).
            if let WindowBounds::Windowed(bounds) = window.window_bounds() {
                let frame = [
                    f32::from(bounds.origin.x),
                    f32::from(bounds.origin.y),
                    f32::from(bounds.size.width),
                    f32::from(bounds.size.height),
                ];
                this.workspace
                    .update(cx, |workspace, _| workspace.set_frame(frame));
            }
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
        // The outbox, from launch onwards. A report he made before quitting, or
        // while the link was down, goes now — and then every minute, which is
        // what makes "it will go by itself" true rather than hopeful. The pass
        // is a directory listing when there is nothing to do, and each report
        // has its own widening delay, so a machine that is away is not
        // hammered.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            loop {
                let alive = this
                    .update(cx, |this, cx| this.drain_feedback(false, cx))
                    .is_ok();
                if !alive {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(60))
                    .await;
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

    /// ⌘T: a new tab. Which row of tabs it lands in follows the focus — the
    /// side panel's when the panel has it, the window's projects otherwise.
    /// The lit tab row is the one that answers, and it is lit off the same
    /// focus this reads, so what the chord will do is on screen before it is
    /// pressed.
    pub(crate) fn new_tab_action(
        &mut self,
        _: &NewTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.panel_focused(window, cx) {
            self.workspace
                .update(cx, |workspace, cx| workspace.new_panel_tab(cx));
            return;
        }
        self.open_project_action(&OpenProject, window, cx);
    }

    pub(crate) fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(1, window, cx);
    }

    pub(crate) fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(-1, window, cx);
    }

    /// Step to the neighbouring tab, wrapping at either end as a browser
    /// does — the side panel's own row when the panel has the focus, and the
    /// window's strip otherwise. That strip's ring is the projects in their
    /// order, then Settings when it is open, because a tab the cycle cannot
    /// reach is not a tab.
    fn cycle_tab(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel_focused(window, cx) {
            self.workspace
                .update(cx, |workspace, cx| workspace.step_panel_tab(step, cx));
            return;
        }
        let projects = self.workspace.read(cx).projects.len();
        // The Settings slot's index, when the strip holds one: past the last
        // project, which is where the strip draws it.
        let settings = self.settings_tab.is_some().then_some(projects);
        let slots = projects + usize::from(settings.is_some());
        let at = match self.front() {
            Front::Settings => settings,
            Front::Project => self.workspace.read(cx).active,
        };
        let (Some(at), true) = (at, slots > 1) else {
            return;
        };
        let next = (at as isize + step).rem_euclid(slots as isize) as usize;
        if settings == Some(next) {
            self.show_settings(window, cx);
        } else {
            self.select_project(next, cx);
        }
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
        // A row is highlighted only where it can be seen. With the panel
        // shut, ⌫ on an emptied composer archived the chat being typed in
        // (one key past the last letter of a draft; F-205) — Cursor's
        // composer never deletes a chat.
        if !workspace.panel().is_some_and(|panel| panel.open) {
            return None;
        }
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

    fn dismiss_menu_action(
        &mut self,
        _: &DismissMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.menu.is_some() {
            self.dismiss_menu(cx);
            return;
        }
        // The side panel is the next thing Escape closes: it is the one of
        // the three ways back that needs no keystroke to be learned first
        // (the Project page shipped without one, and he told us he could not
        // close it).
        if self.panel_focused(window, cx)
            && self
                .workspace
                .read(cx)
                .panel()
                .is_some_and(|panel| panel.open)
        {
            self.workspace
                .update(cx, |workspace, cx| workspace.set_panel_open(false, cx));
            self.focus_composer(window, cx);
            return;
        }
        // Nothing to close: Escape leaves the Settings tab, the Project page or
        // a document for the chat, as ⌘1 does. The pane binds `escape` on its
        // own key context as well, and both are wanted: this one answers when
        // the focus has come back to the window (a pane that stopped being
        // drawn dispatches nothing), that one when the pane itself holds it.
        let leaving = match self.front() {
            Front::Settings => true,
            Front::Project => self.pane != Pane::Chat,
        };
        if leaving {
            self.show_chat(&ShowChat, window, cx);
        }
    }

    /// Show a pane of the project in front, and leave the Settings tab if it
    /// was the tab showing. Every caller is a person asking to see something in
    /// the column — a tab, a chat, a surface, the project page — and none of
    /// them means it to happen behind Settings.
    pub(crate) fn show_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        self.leave_settings(cx);
        self.set_pane(pane, cx);
    }

    /// Set the pane without touching which tab is in front. The kernel's own
    /// [`PaneRequest`] takes this route: an agent opening a terminal must not
    /// pull a person out of the settings they are reading.
    pub(crate) fn set_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        self.pane = pane;
        cx.notify();
    }

    pub(crate) fn select_session(&mut self, id: u64, cx: &mut Context<Self>) {
        self.show_pane(Pane::Chat, cx);
        self.workspace
            .update(cx, |workspace, cx| workspace.select_session(id, cx));
    }

    /// A click on a surface, wherever it was clicked: it comes to the front
    /// of the side panel and the drawer opens with it.
    pub(crate) fn show_surface(
        &mut self,
        id: SurfaceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace
            .update(cx, |workspace, cx| workspace.show_surface(id, true, cx));
        self.focus_panel(window, cx);
    }

    /// ⌘\\ and the four-box: grow the drawer to the space the window can
    /// spare, or return it to the default. An older build put the tab in
    /// front into the chat column, which cloned a Terminal over the
    /// conversation; if that pane is still showing, this key gives the
    /// chat back first.
    pub(crate) fn zoom_panel_action(
        &mut self,
        _: &ZoomPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pane == Pane::Surface {
            self.show_chat(&ShowChat, window, cx);
            return;
        }
        if self
            .workspace
            .read(cx)
            .panel()
            .is_none_or(|panel| !panel.open)
        {
            self.workspace
                .update(cx, |workspace, cx| workspace.set_panel_open(true, cx));
        }
        let viewport = f32::from(window.viewport_size().width);
        let available =
            (viewport - crate::model::panel::CHAT_MIN_WIDTH).max(crate::model::panel::MIN_WIDTH);
        self.workspace.update(cx, |workspace, cx| {
            workspace.toggle_panel_expand(available, cx)
        });
    }

    pub(crate) fn open_settings_action(
        &mut self,
        _: &OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings(Section::General, window, cx);
    }

    /// Kernel notifications (#293), sorted the way Cursor sorts them: one
    /// for the chat the person is looking at, in a focused window, is seen
    /// the moment it arrives (`seen` goes to the kernel, every client's
    /// badge drops); any other — window unfocused, another project's tab,
    /// another chat of this project — becomes an OS notification and stays
    /// on the tab's badge until that chat is opened.
    pub(crate) fn reap_notifications(&mut self, cx: &mut Context<Self>) {
        let window_active = self.window_active;
        let touched = self.touched;
        let mut post: Vec<(String, String)> = Vec::new();
        self.workspace.update(cx, |workspace, _| {
            let active = workspace.active_id();
            let active_ix = workspace.active;
            for (ix, project) in workspace.projects.iter_mut().enumerate() {
                for chat in &mut project.sessions {
                    let looking = window_active
                        && touched
                        && Some(ix) == active_ix
                        && Some(chat.id) == active;
                    if looking {
                        if !chat.unseen.is_empty() {
                            chat.mark_seen();
                        }
                        continue;
                    }
                    for note in chat.to_notify.drain(..) {
                        post.push((note.title, note.body));
                    }
                }
            }
        });
        for (title, body) in post {
            let result = crate::notify_os::post(&title, &body);
            self.notifications_posted.push(PostedNotification {
                at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
                title,
                body,
                error: result.err(),
            });
            if self.notifications_posted.len() > 50 {
                self.notifications_posted.remove(0);
            }
        }
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

    /// ⌘B: open or close the side panel of the project in front. Opening it
    /// gives it the focus, so the tab chords act on its row at once — he
    /// asked for the drawer, so the drawer is what he is driving.
    pub(crate) fn toggle_panel_action(
        &mut self,
        _: &TogglePanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace
            .update(cx, |workspace, cx| workspace.toggle_panel(cx));
        let open = self
            .workspace
            .read(cx)
            .panel()
            .is_some_and(|panel| panel.open);
        if open {
            self.focus_panel(window, cx);
        } else {
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    /// Whether the side panel holds the focus — which row of tabs `⌘T` and
    /// `⌘⇧{ }` act on, and which row is drawn lit.
    pub(crate) fn panel_focused(&self, window: &Window, cx: &App) -> bool {
        self.panel_focus.contains_focused(window, cx)
    }

    /// Give the panel the focus. Only a person's own action calls this.
    pub(crate) fn focus_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.panel_focus, cx);
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
    /// ⌘W closes the tab in front, and Settings is a tab.
    pub(crate) fn close_project_action(
        &mut self,
        _: &CloseProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.front() {
            Front::Settings => self.close_settings(window, cx),
            Front::Project => {
                let Some(ix) = self.workspace.read(cx).active else {
                    return;
                };
                self.close_project(ix, cx);
            }
        }
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
                let snippet: String = if first == title {
                    String::new()
                } else {
                    first.chars().take(90).collect()
                };
                hits.push(Hit {
                    project: ix,
                    session: chat.id,
                    title,
                    snippet,
                    tab: tab.clone(),
                    updated: chat.updated,
                    running: chat.busy(),
                });
            }
        }
        self.dismiss_menu(cx);
        self.chat_search
            .update(cx, |search, cx| search.show(hits, window, cx));
    }

    /// The composer takes the keyboard back, when there is one to take it.
    pub(crate) fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let composer = self
            .workspace
            .read(cx)
            .active_session()
            .map(|_| self.composer.read(cx).focus_handle(cx));
        window.focus(composer.as_ref().unwrap_or(&self.focus), cx);
    }

    /// ⌘2 and the panel's Project header: the project page in the right
    /// panel. Never the main column — that covered the chat (#629).
    pub(crate) fn show_project(
        &mut self,
        _: &ShowProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace
            .update(cx, |workspace, cx| workspace.select_panel_tab(0, cx));
        self.focus_panel(window, cx);
    }

    /// ⌘1: the chat, whatever else is open — the way back from the Settings
    /// tab as much as from the Project page or the side panel, and the one
    /// that always works.
    pub(crate) fn show_chat(&mut self, _: &ShowChat, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace.update(cx, |workspace, _| {
            if let Some(project) = workspace.active_project_mut() {
                if let Some(focus) = &mut project.focus {
                    focus.surface = None;
                }
            }
        });
        self.show_pane(Pane::Chat, cx);
        // The composer takes the keyboard either way, so the next keystroke
        // lands in the chat: the Settings tab stays in the strip where its
        // close mark is, and the side panel gives the tab chords back to the
        // window's own strip. Unconditional because "⌘1 goes to the chat" has
        // to mean the caret too, or the drawer keeps answering ⌘T.
        self.focus_composer(window, cx);
    }

    /// ⌘, the gear, and the menu item: open the Settings tab on `section`, or
    /// bring the open one forward. Never a second one — two Settings tabs
    /// would be two views of one preference file, and the strip would hold a
    /// tab whose twin already answers to the same chord.
    pub(crate) fn open_settings(
        &mut self,
        section: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &self.settings_tab {
            Some(tab) => {
                let pane = tab.pane.clone();
                pane.update(cx, |pane, cx| pane.show(section, cx));
            }
            None => {
                let workspace = self.workspace.clone();
                let pane = cx.new(|cx| SettingsPane::new(workspace, section, cx));
                self.settings_tab = Some(SettingsTab { pane, front: false });
            }
        }
        self.dismiss_menu(cx);
        self.show_settings(window, cx);
    }

    /// Put the Settings tab in front. The pane takes the keyboard, so Escape
    /// reaches its own key context.
    pub(crate) fn show_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = &mut self.settings_tab else {
            return;
        };
        tab.front = true;
        let focus = tab.pane.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Leave the Settings tab without closing it: Escape, ⌘1, a click on a
    /// project tab, ⌃Tab, the rail's own row. It keeps its place in the strip,
    /// where its close mark is.
    fn leave_settings(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = &mut self.settings_tab else {
            return;
        };
        if !tab.front {
            return;
        }
        tab.front = false;
        let pane = tab.pane.clone();
        pane.update(cx, |pane, cx| pane.went_behind(cx));
        cx.notify();
    }

    /// Close the tab: its close mark, ⌘W with it in front, and the driver's
    /// `arbos_settings::CloseSettings`. Whatever is underneath comes back —
    /// the chat of the project in front, or, when Settings was the only tab
    /// left, the launch view with its folder button.
    pub(crate) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.settings_tab.take() else {
            return;
        };
        tab.pane.update(cx, |pane, cx| pane.went_behind(cx));
        self.focus_composer(window, cx);
        cx.notify();
    }

    pub(crate) fn close_settings_action(
        &mut self,
        _: &CloseSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_settings(window, cx);
    }

    /// Which tab the window's middle draws.
    pub(crate) fn front(&self) -> Front {
        match &self.settings_tab {
            Some(tab) if tab.front => Front::Settings,
            _ => Front::Project,
        }
    }

    /// Mic button: start capture, or stop, put the words in the field, and send.
    /// While a speech-server session is live, what its agent does
    /// (`agent.*`, `tool.*`, `text.done`) lands in the active chat as
    /// notices, a few times a second. Ends when the session does.
    ///
    /// A live call owns the same queue (`start_call_mirror`). This loop
    /// must not run then: it writes to whichever tab is in front, so Home
    /// or another project would get the spoken rows.
    pub(crate) fn start_voice_mirror(&mut self, cx: &mut Context<Self>) {
        if self.voice_mirror_on
            || self.call.is_some()
            || crate::voice_ws::in_call()
            || !crate::voice_ws::configured()
        {
            return;
        }
        self.voice_mirror_on = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(400))
                    .await;
                let keep = this.update(cx, |this, cx| {
                    if this.call.is_some() || crate::voice_ws::in_call() {
                        this.voice_mirror_on = false;
                        return false;
                    }
                    let lines = crate::voice_ws::drain_mirror();
                    let live = crate::voice_ws::status().phase.is_some();
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
                // Dictation is typing by voice: the words land in the field
                // and wait for Enter, and the answer is read, not spoken —
                // as Cursor's mic does. It used to send at once and have the
                // reply read aloud (Jacob, report 2026-09-17-19: "the message
                // is immediately sent rather than just appearing in the chat
                // box … the response is spoken, this is the wrong way").
                // A call (the phone control) is where speech answers speech.
                let sent = false;
                this.composer.update(cx, |composer, cx| {
                    composer.set_voice(VoiceState::Idle, cx);
                    if let Ok(text) = &text
                        && !text.trim().is_empty()
                    {
                        composer.dictation_text(text, cx);
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
        let Some(project) = workspace.active_project() else {
            return;
        };
        // That project's own chat — not a worker under it, and not Home
        // unless Home is the tab in front (the call is then Home's).
        let Some(session) = project.main_session().or_else(|| workspace.active_id()) else {
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
        // What the gateway is told the call is for: this tab's folder path
        // with the machine and folder name the hub knows it by. The path is
        // what binds the call — never another folder's agent, whatever the
        // gateway was started with.
        let mut target = kernel::call_target(&project.place(), &label);
        if let Some(chat) = workspace.session(session) {
            target.context = call_context(chat);
        }
        // No machine name and a speech server elsewhere: the gateway would
        // refuse (`project_not_on_hub`) — say so now, in the chat, with what
        // to do, instead of dialing. A gateway on this computer can still
        // reach the folder by its path.
        if target.machine.is_none() && !crate::voice_ws::gateway_is_local() {
            let why = format!("voice · call refused: {}", crate::voice_ws::NOT_ON_HUB);
            self.workspace.update(cx, |workspace, cx| {
                workspace.with_session(session, cx, |chat| chat.notice(true, &why));
            });
            self.voice_error(
                &format!("call refused: {}", crate::voice_ws::NOT_ON_HUB),
                cx,
            );
            return;
        }
        // Dictation, if a take is open, ends: the call owns the mic.
        if self.composer.read(cx).is_recording() {
            self.stop_voice(cx);
        }
        // Stop the dictation mirror so it cannot steal call frames onto
        // the front tab (Home, another project) while this call is live.
        self.voice_mirror_on = false;
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
                .spawn(async move { crate::voice_ws::call_start(&target) })
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
                        // The refusal in the chat too, where the caller looks
                        // (`project_not_on_hub: … put the hub url … then call again`).
                        this.workspace.update(cx, |workspace, cx| {
                            workspace.with_session(session, cx, |chat| {
                                chat.notice(true, &format!("voice · call refused: {e:#}"));
                            });
                        });
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

    /// While the call is live: spoken lines land only in the call's project
    /// chat (`call.session`). The dictation mirror is off, so Home and other
    /// projects cannot receive them. A dropped session ends the call here.
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
                                    if m.kind == "caller.said" {
                                        chat.voice_prompt(&m.text);
                                    } else if let Some(line) = call_line(m) {
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

    /// Report a problem: open the review sheet on the chat in front, ask the
    /// kernel for the exchange behind it, and take a picture of this window.
    ///
    /// Nothing is sent here. The sheet shows him every part first and Send is
    /// the only thing that writes anything.
    pub(crate) fn report_problem(
        &mut self,
        _: &ReportProblem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_report(None, window, cx);
    }

    /// The thumbs-down under an answer: Cursor's 👎 asks what was wrong, and
    /// so does ours — the same sheet, anchored on that exchange. The vote
    /// itself is already recorded by the time this runs; dismissing the
    /// sheet leaves a plain 👎 a plain 👎.
    pub(crate) fn report_problem_at(
        &mut self,
        action: &ReportProblemAt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_report(action.seq, window, cx);
    }

    fn open_report(&mut self, seq: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        let (id, agent, place) = {
            let workspace = self.workspace.read(cx);
            let id = workspace.active_id();
            let session = workspace.active_session();
            (
                id,
                session.and_then(|chat| chat.agent_session.clone()),
                workspace
                    .active_project()
                    .map(|project| arbos_core::Place::new(project.path.clone())),
            )
        };
        // His last answer to the tool-argument control, so he does not decide
        // it again every time.
        let parts = place
            .as_ref()
            .map(crate::feedback::load_parts)
            .unwrap_or_default();
        self.report_anchor = id.zip(seq);
        self.feedback_sheet.update(cx, |sheet, cx| {
            sheet.show(agent, seq, parts, window, cx);
            // Everything the window knows about this place, not only the chat
            // in front: the rows and the facts behind them, the records on
            // disk, the tabs and the focus. A report that carries one side of a
            // disagreement cannot show it.
            let workspace = self.workspace.read(cx);
            let mut view = workspace
                .active_project()
                .map(|project| {
                    // The store, not the path: a remote place's records live in
                    // its local sidecar, and its path is the far machine's.
                    workspace.desktop_state(&project.store(), crate::feedback::DESKTOP_STATE_BUDGET)
                })
                .unwrap_or(serde_json::Value::Null);
            if let Some(obj) = view.as_object_mut()
                && let Some(chat) = workspace.active_session()
            {
                obj.insert("drawn".into(), chat.drawn_view());
            }
            sheet.take_session(view, cx);
        });
        if let Some(id) = id {
            self.workspace.update(cx, |workspace, cx| {
                workspace.with_session(id, cx, |chat| {
                    chat.request_feedback(seq, crate::feedback::TAIL_LINES)
                });
            });
        }
        // And a clock on the ask. A kernel that predates the frame refuses and
        // is caught above; one that is down, wedged, or on a link that is not
        // carrying says nothing at all, and the sheet must not wait on it in
        // silence. Two and a half seconds is long enough for a local socket and
        // short enough that he is still looking at the sheet when it answers.
        let waiting_sheet = self.feedback_sheet.clone();
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(2_500))
                .await;
            let _ = waiting_sheet.update(cx, |sheet, cx| {
                if sheet.awaiting() {
                    sheet.no_bundle(Unavailable::NoAnswer, cx);
                }
            });
        })
        .detach();
        // The capture runs off the UI thread: it shells out, and the window
        // must keep drawing — a frozen window is not the window he is
        // complaining about.
        let size = window.viewport_size();
        let (w, h) = (f32::from(size.width), f32::from(size.height));
        let sheet = self.feedback_sheet.clone();
        cx.spawn(async move |_, cx| {
            let shot = cx
                .background_executor()
                .spawn(async move {
                    crate::feedback::capture_window(w, h).map_err(|e| format!("{e:#}"))
                })
                .await;
            let _ = sheet.update(cx, |sheet, cx| sheet.take_shot(shot, cx));
        })
        .detach();
    }

    /// Read the far side's own account over ssh, off the UI thread.
    ///
    /// `exit 2` is a real answer and is shown as one: there is no Arbos place in
    /// that folder, so there is nothing to attach — a different fact from "the
    /// kernel would not answer", pointing at a different thing to do. Jacob's
    /// `ArbosLife:~` tab had no store there until this afternoon.
    fn collect_over_ssh(&mut self, host: String, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let sheet = self.feedback_sheet.clone();
        cx.spawn(async move |_, cx| {
            let got = cx
                .background_executor()
                .spawn(async move {
                    crate::kernel::feedback_over_ssh(&host, &path, crate::feedback::TAIL_LINES)
                })
                .await;
            let _ = sheet.update(cx, |sheet, cx| match got {
                Ok(crate::kernel::FarSide::Bundle(line)) => {
                    match crate::agent::acp::feedback_bundle_from_json(&line) {
                        Some(bundle) => sheet.take_bundle_over_ssh(bundle, cx),
                        None => sheet.no_bundle(Unavailable::NoAnswer, cx),
                    }
                }
                Ok(crate::kernel::FarSide::NoPlace(why)) => {
                    sheet.no_bundle(Unavailable::NoPlaceThere(why), cx)
                }
                Err(_) => sheet.no_bundle(Unavailable::NoAnswer, cx),
            });
        })
        .detach();
    }

    /// The kernel answered a `feedback` ask: hand it to the sheet.
    fn collect_feedback(&mut self, cx: &mut Context<Self>) {
        if !self.feedback_sheet.read(cx).is_open {
            return;
        }
        let Some(id) = self.workspace.read(cx).active_id() else {
            return;
        };
        // Look before taking: this runs inside the workspace's observer,
        // and `with_session` notifies the workspace, so taking from a chat
        // that holds nothing observed itself forever — the window froze
        // the moment the sheet opened (found driving it on the rig).
        if !self
            .workspace
            .read(cx)
            .session(id)
            .is_some_and(|chat| chat.feedback.is_some() || chat.feedback_error.is_some())
        {
            return;
        }
        let mut taken = None;
        let mut refused = None;
        self.workspace.update(cx, |workspace, cx| {
            workspace.with_session(id, cx, |chat| {
                taken = chat.take_feedback();
                refused = chat.take_feedback_error();
            });
        });
        if let Some(bundle) = taken {
            self.feedback_sheet
                .update(cx, |sheet, cx| sheet.take_bundle(*bundle, cx));
        } else if refused.is_some() {
            // The kernel said it does not know the frame, which means it
            // predates it. Certain, not guessed.
            self.feedback_sheet.update(cx, |sheet, cx| {
                sheet.no_bundle(Unavailable::KernelTooOld, cx)
            });
        }
    }

    /// A kernel came back: try the outbox at once rather than waiting for the
    /// minute to turn. This is the moment "it will go by itself when the link
    /// is back" names, so it should not take a minute to honour.
    ///
    /// An edge, not a level: draining on every observer run while connected
    /// would shell out to the kernel several times a second.
    fn drain_on_reconnect(&mut self, cx: &mut Context<Self>) {
        let connected = self
            .workspace
            .read(cx)
            .projects
            .iter()
            .flat_map(|project| project.sessions.iter())
            .any(|chat| chat.connected());
        if connected && !self.was_connected {
            self.was_connected = true;
            self.drain_feedback(false, cx);
        } else if !connected {
            self.was_connected = false;
        }
    }

    /// Send: write the report to the outbox on his own disk. On disk is what
    /// "sent" means here — delivery reads the outbox, so a report made with
    /// the network down is already safe and goes out when the link returns.
    fn write_report(
        &mut self,
        sheet: Entity<FeedbackSheet>,
        draft: &crate::feedback::Draft,
        cx: &mut Context<Self>,
    ) {
        // The host as well as the path: a remote place's path is not a local
        // path, and staging a report inside one is what stranded his.
        let host = self
            .workspace
            .read(cx)
            .active_project()
            .and_then(|project| project.host.clone());
        let Some(place) = self
            .workspace
            .read(cx)
            .active_project()
            .map(|project| arbos_core::Place::new(project.path.clone()))
        else {
            sheet.update(cx, |sheet, cx| {
                sheet.settled(Err("no project open to file this against".into()), cx)
            });
            return;
        };
        crate::feedback::save_parts(&place, &draft.parts);
        let id = crate::feedback::new_id(arbos_core::now_ms());
        let written =
            match crate::feedback::write(&place, host.as_deref(), draft, &id, arbos_core::now_ms())
            {
                Ok(written) => written,
                Err(e) => {
                    // Every outbox refused it. His words are still in the field and
                    // must not die there, so the sheet offers to put them on the
                    // clipboard rather than a button that repeats the failure.
                    sheet.update(cx, |sheet, cx| {
                        sheet.nowhere_to_save(format!("{e:#}"), cx);
                    });
                    return;
                }
            };
        // The thumbs-down he pressed now reads as reported.
        if let Some((chat_id, seq)) = self.report_anchor.take() {
            self.workspace.update(cx, |workspace, cx| {
                workspace.with_session(chat_id, cx, |chat| chat.mark_reported(seq, &id));
            });
        }
        // On disk is what Send means, so say so now and carry it the rest of
        // the way behind him. Delivery shells out to the kernel and talks to
        // the hub; neither belongs on the thread drawing the window.
        let address = self.workspace.read(cx).settings.feedback.address.clone();
        sheet.update(cx, |sheet, cx| {
            sheet.settled(
                Ok({
                    // Where it went, when that is not where it usually goes.
                    // A report saved somewhere unexpected is only honest if it
                    // says so.
                    let where_ = match written.elsewhere {
                        Some(why) => format!(" Saved to {why} — {}.", written.dir.display()),
                        None => String::new(),
                    };
                    if address.trim().is_empty() {
                        format!(
                            "Saved. It has nowhere to go yet — this machine has no feedback address — so it waits on disk.{where_} Reference {id}."
                        )
                    } else {
                        format!(
                            "Sent. It reaches an agent within fifteen minutes, and you will be told which build carries the fix.{where_} Reference {id}."
                        )
                    }
                }),
                cx,
            )
        });
        self.drain_feedback(true, cx);
    }

    /// Carry every waiting report to the store it goes to, off the UI thread.
    ///
    /// `tell_him` says whether a failure should be spoken: it should when he has
    /// just pressed Send and is looking at the sheet, and should not when this
    /// is the timer doing its rounds behind him.
    ///
    /// Every open project, not only the one in front: a report filed in one
    /// project while another is on screen is still his report.
    ///
    /// QA found this path had exactly one call site — Send — so "it will go by
    /// itself when the link is back" only came true if he happened to report
    /// something else later (qal-j06). The wording was a promise the code did
    /// not keep, which is the one thing this feature cannot afford, so the
    /// callers now are Send, launch, a kernel coming back, and a timer.
    fn drain_feedback(&mut self, tell_him: bool, cx: &mut Context<Self>) {
        let (address, hub_home) = {
            let feedback = &self.workspace.read(cx).settings.feedback;
            (feedback.address.clone(), feedback.hub_home.clone())
        };
        if address.trim().is_empty() {
            return;
        }
        // Every outbox that holds reports, whatever is open. Walking the open
        // tabs meant a report from a project he had closed was never retried,
        // and a report staged outside a project — because his home went
        // read-only — belongs to no tab at all.
        let mut roots = crate::feedback::known_outboxes();
        for project in &self.workspace.read(cx).projects {
            let root = crate::feedback::outbox(&arbos_core::Place::new(project.path.clone()));
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        if roots.is_empty() {
            return;
        }
        let sheet = self.feedback_sheet.clone();
        cx.spawn(async move |this, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    let home = std::path::Path::new(&hub_home);
                    let now = arbos_core::now_ms();
                    roots
                        .iter()
                        .flat_map(|root| {
                            crate::feedback::deliver_pending(root, &address, home, now)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            if results.is_empty() {
                return;
            }
            let why = results.iter().find_map(|(_, state)| match state {
                crate::feedback::Delivery::Waiting {
                    last_error: Some(why),
                    ..
                } => Some(why.clone()),
                _ => None,
            });
            let sent = results
                .iter()
                .filter(|(_, state)| matches!(state, crate::feedback::Delivery::Sent { .. }))
                .count();
            // Kept where the driver can read it, so the loop can assert the
            // state Jacob is in rather than photographing the window for it.
            let _ = this.update(cx, |this, cx| {
                this.feedback_outbox = OutboxState {
                    waiting: results.len() - sent,
                    sent_this_run: sent,
                    last_error: why.clone(),
                    at: Some(Instant::now()),
                };
                cx.notify();
            });
            // Only a failure is worth saying to him, and only while he is
            // looking: a delivered report already read as sent when it hit the
            // disk, and the timer must not talk over whatever he is doing.
            if tell_him && let Some(why) = why {
                let _ = sheet.update(cx, |sheet, cx| {
                    sheet.settled(
                        Err(format!(
                            "Saved, and waiting: it could not be sent yet — {why}. It will go by itself when the link is back."
                        )),
                        cx,
                    )
                });
            }
        })
        .detach();
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
        // The Project page and a surface stay in the right panel. Asking
        // for either in the column used to cover the chat (#629).
        let asked = match self.pane {
            Pane::Chat => Pane::Chat,
            Pane::Surface | Pane::Project => Pane::Chat,
        };
        if self.has_pane(asked, cx) {
            return Some(asked);
        }
        [Pane::Chat]
            .into_iter()
            .find(|&pane| self.has_pane(pane, cx))
    }

    /// Whether the active project has anything open in `pane`.
    pub(crate) fn has_pane(&self, pane: Pane, cx: &App) -> bool {
        let workspace = self.workspace.read(cx);
        match pane {
            // A surface in the panel must not hide the chat. The old
            // column-takeover used `focus.surface` to mean the middle of
            // the window was a document; that is gone (#629).
            Pane::Chat => workspace.active_session().is_some(),
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
            // The first press or key in the window: from here the chat in
            // front counts as looked at (see `touched`).
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                if !this.touched {
                    this.touched = true;
                    this.reap_notifications(cx);
                }
            }))
            .on_key_down(cx.listener(|this, _, _, cx| {
                if !this.touched {
                    this.touched = true;
                    this.reap_notifications(cx);
                }
            }))
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
            // The strip of tabs across the top, then whichever tab is in
            // front: a project — the chat column with the panel on its right —
            // or Settings, which is not a project and so fills the width.
            .child(self.tab_bar(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_row()
                    .map(|row| match self.front() {
                        // No panel beside it: the panel is a view of a
                        // project's `.arbos/`, and Settings has none.
                        Front::Settings => {
                            row.children(self.settings_tab.as_ref().map(|tab| tab.pane.clone()))
                        }
                        Front::Project => row
                            .child(self.detail(window, cx))
                            .children(self.panel(window, cx)),
                    }),
            )
            // Under everything, the width of the window: settings and the
            // update control, where Cursor keeps them.
            .child(self.status_bar(cx))
            .children(
                self.workspace
                    .read(cx)
                    .meter
                    .then(|| meter::panel("app-meter", &self.meter_at, &self.meter, window)),
            )
            .child(self.opener.clone())
            .child(self.tab_sheet.clone())
            .child(self.feedback_sheet.clone())
            .child(self.permissions_sheet.clone())
            .child(self.chat_search.clone())
    }
}

/// Where the feedback outbox stands, for the driver to read and the window to
/// draw. Its own type rather than a tuple because the parity loop asserts on
/// these names.
#[derive(Debug, Default, Clone)]
pub(crate) struct OutboxState {
    /// Reports written and not yet delivered.
    pub waiting: usize,
    /// How many went on the last pass.
    pub sent_this_run: usize,
    /// Why the last attempt did not go, in the kernel's own words.
    pub last_error: Option<String>,
    pub at: Option<Instant>,
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

/// Put the transparent titlebar back — macOS 15.3+ clears it on a
/// style-mask change — and keep the window off native Spaces, so the green
/// button and View › Enter Full Screen fill the screen on this Space with
/// the three traffic lights still in the content (Jacob, 09-18) and the
/// wallpaper still behind the glass.
///
/// The title stays hidden and the view fills the window; a window that is
/// in a Space anyway (one AppKit restored) still gets its safe-area inset
/// zeroed, so no band is left across the top.
#[cfg(target_os = "macos")]
fn keep_macos_glass(window: &Window) {
    use objc::{
        class, msg_send,
        runtime::{Object, YES},
        sel, sel_impl,
    };

    const FULL_SCREEN_PRIMARY: usize = 1 << 7;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;
    const FULL_SCREEN_NONE: usize = 1 << 9;
    const STYLE_MASK_FULL_SCREEN: usize = 1 << 14;
    const STYLE_MASK_FULL_SIZE_CONTENT: usize = 1 << 15;
    const TITLE_HIDDEN: usize = 1;

    #[repr(C)]
    struct NsEdgeInsets {
        top: f64,
        left: f64,
        bottom: f64,
        right: f64,
    }

    let in_space = window.is_fullscreen();
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
            let _: () = msg_send![ns_window, setTitleVisibility: TITLE_HIDDEN];
            let style_mask: usize = msg_send![ns_window, styleMask];
            if style_mask & STYLE_MASK_FULL_SIZE_CONTENT == 0 {
                let _: () = msg_send![
                    ns_window,
                    setStyleMask: style_mask | STYLE_MASK_FULL_SIZE_CONTENT
                ];
            }
            let behavior = (behavior & !FULL_SCREEN_PRIMARY) | FULL_SCREEN_NONE;
            let _: () = msg_send![ns_window, setCollectionBehavior: behavior];
            if in_space || style_mask & STYLE_MASK_FULL_SCREEN != 0 {
                let content: *mut Object = msg_send![ns_window, contentView];
                if !content.is_null() {
                    let zero = NsEdgeInsets {
                        top: 0.,
                        left: 0.,
                        bottom: 0.,
                        right: 0.,
                    };
                    let _: () = msg_send![content, setAdditionalSafeAreaInsets: zero];
                }
            }
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
            let _: () = msg_send![ns_window, setTitleVisibility: 1usize];
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
        // GPT Live's own words, including short small talk ("hey"). The
        // narrator's "On it." is `narrator.say/ack` above, not this.
        "model.reply" => Some(format!("voice · {text}")),
        _ => None,
    }
}

/// What the call starts knowing: the last lines of this chat, clipped, and
/// the sub-agents, so the narrator and the speech model can answer "what
/// were we doing" before the first new turn. Tool lines are the label and
/// state only — never the output or the diff.
fn call_context(chat: &crate::model::session::ChatSession) -> crate::voice_ws::CallContext {
    use crate::model::session::{ChatItem, ChildState, ToolStatus};
    use crate::voice_ws::{CallContext, ContextAgent, ContextLine};
    const LINES: usize = 40;
    const CLIP: usize = 400;
    let clip = |text: &str| -> String {
        let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.chars().count() > CLIP {
            flat.chars().take(CLIP).collect::<String>() + "…"
        } else {
            flat
        }
    };
    let mut recent: Vec<ContextLine> = chat
        .items
        .iter()
        .rev()
        .filter_map(|item| match item {
            ChatItem::User(m) if !m.text.trim().is_empty() => Some(ContextLine {
                role: "user".into(),
                text: clip(&m.text),
            }),
            ChatItem::Agent(text) if !text.trim().is_empty() => Some(ContextLine {
                role: "assistant".into(),
                text: clip(text),
            }),
            ChatItem::From { who, text, .. } if !text.trim().is_empty() => Some(ContextLine {
                role: "worker".into(),
                text: clip(&format!("{who}: {text}")),
            }),
            ChatItem::Tool { label, status, .. } => {
                let state = match status {
                    ToolStatus::Running => "running",
                    ToolStatus::Success => "done",
                    ToolStatus::Failure => "failed",
                };
                Some(ContextLine {
                    role: "tool".into(),
                    text: clip(&format!("{label} ({state})")),
                })
            }
            ChatItem::Notice { text, .. } if !text.trim().is_empty() => Some(ContextLine {
                role: "notice".into(),
                text: clip(text),
            }),
            ChatItem::Asked { question, answer } => {
                let line = if answer.trim().is_empty() {
                    format!("asked: {question}")
                } else {
                    format!("asked: {question} → {answer}")
                };
                Some(ContextLine {
                    role: "asked".into(),
                    text: clip(&line),
                })
            }
            ChatItem::Thinking { text, done, .. } if !done && !text.trim().is_empty() => {
                Some(ContextLine {
                    role: "thinking".into(),
                    text: clip(text),
                })
            }
            _ => None,
        })
        .take(LINES)
        .collect();
    recent.reverse();
    let agents = chat
        .children
        .iter()
        .map(|c| ContextAgent {
            name: c.kernel_id.clone().unwrap_or_else(|| c.title.clone()),
            state: match c.state {
                ChildState::Working => "working",
                ChildState::Asking => "asking",
                ChildState::Waiting => "waiting",
                ChildState::Done => "done",
            }
            .into(),
            step: c.step.clone(),
        })
        .collect();
    CallContext {
        recent,
        agents,
        running: chat.busy(),
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
