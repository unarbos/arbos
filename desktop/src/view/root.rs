//! Root view: the window's grid, the state the chrome owns, and the frame
//! the sidebar and the chat column are hung in.

use crate::{
    kernel,
    model::{
        session::ChatSession,
        settings::Settings,
        state::{self, State},
        workspace::{PaneRequest, Reloaded, Workspace},
    },
    view::{
        component::{
            composer::{Composer, ComposerEvent, VoiceState},
            menu::Menu,
            meter,
            opener::{Opener, OpenerEvent},
        },
        settings::{self, Section, SettingsWindow},
        sidebar::{Renaming, Row, SessionDrop},
    },
};
use anyhow::Result;
use bezel::{
    gpui::{
        self, AnyElement, App, Axis, Bounds, Context, DragMoveEvent, Empty, Entity, FocusHandle,
        Focusable as _, Hsla, KeyBinding, PathPromptOptions, Render, Task, TitlebarOptions,
        UniformListScrollHandle, Window, WindowBounds, WindowHandle, WindowOptions, actions, div,
        point, prelude::*, px, size,
    },
    motion::{Fade, Painter},
    theme::{Material, TextStyle, Theme, Typeset, appearance},
    ui::{
        floating::Floating,
        icons,
        input::{FieldEvent, TextField},
        menu::Cursor,
        stats::Stats,
        widgets::{ButtonStyle, Buttons, Content, Layout, SPLIT_HANDLE_HIT, SplitDrag, SplitStyle},
    },
};
use std::time::Duration;
actions!(
    arbos,
    [
        NewSession,
        OpenProject,
        CloseProject,
        OpenSettings,
        ToggleSidebar,
        ShowChat,
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
        ZoomReset
    ]
);

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

/// Cursor's Agents sidebar measures 267pt at a 1728pt window.
const SIDEBAR_WIDTH: f32 = 260.;
const SIDEBAR_WIDTH_MIN: f32 = 180.;
const SIDEBAR_WIDTH_MAX: f32 = 420.;

/// The sidebar's gutter: a row's outer margin, and the padding inside it.
pub(crate) const SIDEBAR_GUTTER: f32 = 8.;

/// How thick each column's material sits. Nothing paints beneath them, so these
/// are absolute and independent: the sidebar is chrome and holds no long-form
/// text, the panel is the column whose text has to win against the desktop.
const SIDEBAR_MATERIAL: Material = Material::Thick;
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
/// Web `px-3.5`. Cards bleed by `COMPOSER_PAD_X` so their words sit on
/// this edge, same as the answer.
pub(crate) const CHAT_GUTTER: f32 = 14.;

/// Hit target for every control on the tool row — model, files, voice, send.
pub(crate) const COMPOSER_HIT: f32 = 28.;

/// How far the floating composer stands off the column's bottom edge.
pub(crate) const COMPOSER_BOTTOM: f32 = 12.;

/// The sidebar's fill. Opaque, it takes the chrome tone: the light palette's
/// `surface` is the grey the content plane's white sits inside, and falling
/// back to the panel would leave the two columns one flat sheet.
pub(crate) fn sidebar_bg(theme: &Theme) -> Hsla {
    material(theme, SIDEBAR_MATERIAL).unwrap_or(theme.surface)
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
    cx.bind_keys([
        KeyBinding::new("cmd-n", NewSession, None),
        KeyBinding::new("cmd-o", OpenProject, None),
        // What macOS binds Preferences to in every other app.
        KeyBinding::new("cmd-,", OpenSettings, None),
        // What every app with a sidebar binds it to. It is claimed app-wide:
        // the menu item carries it, so AppKit takes the chord before the
        // window is offered it, and the editor's own `cmd-b` — bold — is not
        // reached while this one is on the bar.
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-1", ShowChat, None),
        // What a browser binds its zoom to. `cmd-=` first so the menu
        // draws ⌘= like Safari; `cmd-+` is the same key with shift held.
        KeyBinding::new("cmd-=", ZoomIn, None),
        KeyBinding::new("cmd-+", ZoomIn, None),
        KeyBinding::new("cmd--", ZoomOut, None),
        KeyBinding::new("cmd-0", ZoomReset, None),
        // Bound ahead of the `tab` pair below because the menu draws the first
        // chord a command was given, and `tab` is the one it cannot draw: gpui
        // has no macOS key equivalent for it, so AppKit is handed the word
        // where the API takes one character and shows ⌃T. These are what the
        // View menu carries.
        KeyBinding::new("alt-cmd-right", NextEntry, None),
        KeyBinding::new("alt-cmd-left", PrevEntry, None),
        // What a browser binds its tabs to. Global, because the point is to
        // move between documents without taking the hand out of the editor —
        // where `tab` itself is indent.
        KeyBinding::new("ctrl-tab", NextEntry, None),
        KeyBinding::new("ctrl-shift-tab", PrevEntry, None),
        // Claimed app-wide and answered last: an editor and a field bind copy
        // on their own contexts, which gpui dispatches from the focus outward,
        // so this only runs where nothing else wanted it — which is exactly
        // where a transcript selection is the thing being copied.
        KeyBinding::new("cmd-c", CopySelection, None),
        KeyBinding::new("ctrl-c", CopyChat, None),
        KeyBinding::new("ctrl-v", PasteChat, None),
        KeyBinding::new("delete", DeleteChat, Some(WINDOW_CONTEXT)),
        KeyBinding::new("backspace", DeleteChat, Some(WINDOW_CONTEXT)),
        KeyBinding::new("up", PrevEntry, None),
        KeyBinding::new("down", NextEntry, None),
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
fn force_usable_ns_frame() {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};

    #[repr(C)]
    struct NsPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    struct NsSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    struct NsRect {
        origin: NsPoint,
        size: NsSize,
    }

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
            if frame.size.width >= 600. && frame.size.height >= 320. {
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
/// project — switching projects must not teleport you to another pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Chat,
    Surface,
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

/// The root view. It owns no app state — only the chrome's own: how wide the
/// sidebar is, which pane is showing, and whichever card is being written.
pub struct Arbos {
    pub(crate) workspace: Entity<Workspace>,
    /// The open session menu was opened from the chat header's `⋯`, so it
    /// anchors there rather than at the sidebar row.
    pub(crate) menu_at_header: bool,
    /// The git branch of the last local place looked at, for the row under
    /// the composer. `(place path, branch or none)`.
    pub(crate) branch_cache: std::cell::RefCell<Option<(std::path::PathBuf, Option<String>)>>,
    pub(crate) terminals:
        std::collections::HashMap<String, Entity<crate::view::terminal::TerminalPane>>,
    active_terminal: Option<String>,
    pub(crate) sidebar_open: bool,
    pub(crate) sidebar_width: f32,
    pub(crate) composer: Entity<Composer>,
    pub(crate) opener: Entity<Opener>,
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
    /// there — not also in the sidebar row, which would move and restyle it.
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
    /// The rail's scroll. A step taken from the keyboard has to bring its
    /// landing into view; the list does not scroll itself.
    pub(crate) rail: UniformListScrollHandle,
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
    /// The plan strip above the composer shows one summary line only.
    pub(crate) plan_folded: bool,
    /// Native Fn monitor. Lives with the window so Drop removes it.
    #[cfg(target_os = "macos")]
    _fn_monitor: Option<crate::view::fn_key::Monitor>,
    /// Where a dragged project will land: `0` is the top, `len` is after
    /// the last. `None` when nothing is being carried.
    pub(crate) drop_slot: Option<usize>,
    /// Where a dragged chat will land among its siblings. `None` when
    /// no chat is being carried, or the pointer has left the list.
    pub(crate) session_drop: Option<SessionDrop>,
    /// Which project heading the pointer is on, and whether it is the
    /// pinned copy. The chevron and `+` mount only then — an invisible
    /// control would still steal the click.
    pub(crate) hovering_head: Option<(usize, bool)>,
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
        cx.subscribe(&opener, |this, _, event: &OpenerEvent, cx| match event {
            OpenerEvent::Open(place) => {
                let place = place.clone();
                this.workspace
                    .update(cx, |workspace, cx| workspace.open_place(place, cx));
            }
            OpenerEvent::Browse => this.browse_local(cx),
            OpenerEvent::Dismiss => {}
        })
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
                ComposerEvent::Force(text) => this.force_turn(text.clone(), cx),
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
        // click on the sidebar row makes.
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
            sidebar_open: true,
            sidebar_width: SIDEBAR_WIDTH,
            composer,
            opener,
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
            rail: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            voice_gen: 0,
            voice_place: None,
            fn_held: false,
            voice_want_stop: false,
            plan_folded: false,
            #[cfg(target_os = "macos")]
            _fn_monitor: None,
            drop_slot: None,
            session_drop: None,
            hovering_head: None,
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
            restore_usable_bounds(window);
            cx.notify();
        })
        .detach();
        keep_macos_glass(window);
        this.sync_composer(cx);
        // Where the caret starts. The composer is drawn only over a chat it can
        // send to, and focus on an element no frame draws is focus nowhere.
        let composer = this
            .workspace
            .read(cx)
            .active_session()
            .is_some_and(ChatSession::resumable)
            .then(|| this.composer_focus_handle(cx));
        window.focus(composer.as_ref().unwrap_or(&this.focus), cx);
        #[cfg(target_os = "macos")]
        {
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

    pub(crate) fn new_session_action(
        &mut self,
        _: &NewSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_pane(Pane::Chat, cx);
        self.workspace.update(cx, |workspace, cx| {
            if let Some(entry) = workspace.preferred_agent() {
                workspace.new_session(entry, None, cx);
            }
        });
        self.focus_composer_after_create(window, cx);
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
    /// highlighted sidebar row — not whichever session last held the caret.
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

    /// Step to the next chat or board the sidebar draws, top to bottom
    /// across every project. Headings, `+`, and the archive toggle are
    /// skipped. The ends stay put — first visible chat, Up does nothing;
    /// last visible chat, Down does nothing.
    fn cycle_entry(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.is_some() {
            return;
        }
        // Nothing on screen is nothing to step from: the launch view is not an
        // entry, and its neighbour is not another one.
        let Some(pane) = self.showing(cx) else {
            return;
        };
        let workspace = self.workspace.read(cx);
        let Some(project) = workspace.active else {
            return;
        };
        let Some(open) = workspace.projects.get(project) else {
            return;
        };
        let showing = match pane {
            Pane::Chat => open.focused_agent().map(|id| Row::Session {
                project,
                id,
                nested: open.session(id).is_some_and(|chat| !open.is_root(chat)),
            }),
            Pane::Surface => open
                .focus
                .and_then(|focus| focus.surface.map(|id| Row::Surface { project, id })),
        };
        // Same order as drawn. A project with no chats is skipped because
        // it contributes no Session or Surface rows.
        let list: Vec<Row> = self
            .rows(cx)
            .into_iter()
            .filter(|row| matches!(row, Row::Session { .. } | Row::Surface { .. }))
            .collect();
        let at = showing.and_then(|row| list.iter().position(|entry| *entry == row));
        let Some(landing) = stepped(at, list.len(), step).map(|ix| list[ix]) else {
            return;
        };
        self.open_row(landing, window, cx);
        self.reveal(landing, cx);
    }

    /// Leaving a project is the moment a half-written card has to be filed:
    /// the spot it points at belongs to the board being navigated away from.
    pub(crate) fn select_project(&mut self, ix: usize, cx: &mut Context<Self>) {
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

    fn dismiss_menu_action(&mut self, _: &DismissMenu, _: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_menu(cx);
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

    pub(crate) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        cx.notify();
    }

    pub(crate) fn toggle_sidebar_action(
        &mut self,
        _: &ToggleSidebar,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_sidebar(cx);
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

    /// The menu's Close Project. The sidebar names a project by the row it was
    /// pressed on; the menu bar has only the one in front.
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
        self.settings_window = settings::open(workspace, self.settings_window, section, cx);
    }

    /// Mic button: start capture, or stop, put the words in the field, and send.
    fn toggle_voice(&mut self, cx: &mut Context<Self>) {
        if self.composer.read(cx).is_recording() || self.voice_want_stop {
            self.stop_voice(cx);
        } else {
            self.start_voice(cx);
        }
    }

    fn fn_down(&mut self, cx: &mut Context<Self>) {
        if self.fn_held {
            return;
        }
        self.fn_held = true;
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
                        this.composer.update(cx, |composer, cx| {
                            composer.set_voice_preview(&text, cx);
                        });
                    }
                });
                cx.background_executor()
                    .timer(Duration::from_millis(200))
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
                let server_answers = crate::voice_ws::configured() && crate::voice_ws::server_answers();
                if spoken && crate::voice_ws::configured() && !server_answers {
                    // The answer to a dictated prompt is read aloud.
                    if let Some(id) = this.workspace.read(cx).active_id() {
                        this.workspace.update(cx, |workspace, cx| {
                            workspace.with_session(id, cx, |chat| chat.voice_reply = true);
                        });
                    }
                }
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
                if let Err(e) = text {
                    this.voice_error(&format!("voice failed: {e:#}"), cx);
                }
            });
        })
        .detach();
    }

    fn voice_error(&mut self, msg: &str, cx: &mut Context<Self>) {
        if let Some(id) = self.workspace.read(cx).active_id() {
            self.workspace.update(cx, |workspace, cx| {
                workspace.with_session(id, cx, |chat| chat.notice(true, msg));
            });
        }
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

    fn browse_local(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.workspace
                    .update(cx, |workspace, cx| workspace.open_project(path, cx));
            });
        })
        .detach();
    }

    /// Which pane is on screen, as against [`Self::pane`], which is the one
    /// asked for. They part when what it points at is gone — deleted, switched
    /// off, or in a project that has none open — and whatever the project does
    /// have stands in, so a launch lands on the entry it was left on rather
    /// than on an empty conversation. The sidebar reads this, not the request:
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
        [Pane::Chat, Pane::Surface]
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
                "No project open",
                "A folder on this Mac, or host:folder over ssh.",
            )
            .flex_1()
            .child(
                theme
                    .button(
                        "Open project…",
                        ButtonStyle::Prominent,
                        Some(Fade::new(painter, "open-project-empty")),
                    )
                    .id("open-project-empty")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_project_action(&OpenProject, window, cx);
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
            .flex_row()
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
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<SplitDrag>, _, cx| {
                    this.sidebar_width = f32::from(event.event.position.x)
                        .clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX);
                    cx.notify();
                }),
            )
            // An action reaches the handlers above only through the focused
            // element's ancestors. Sized at nothing, so the pane that does hold
            // a field keeps its focus through a click anywhere else.
            .child(div().key_context(WINDOW_CONTEXT).track_focus(&self.focus))
            .when(self.sidebar_open, |root| root.child(self.sidebar(cx)))
            .child(self.detail(window, cx))
            // Rides on the seam between the sidebar and the detail column
            // rather than sitting in flow, so neither gives up a column.
            .when(self.sidebar_open, |root| {
                root.child(
                    theme
                        .split_handle(Axis::Horizontal, SplitStyle::Line { dragging: false })
                        .id("sidebar-split")
                        .absolute()
                        .top_0()
                        .left(px(self.sidebar_width - SPLIT_HANDLE_HIT / 2.))
                        .on_drag(SplitDrag, |_, _, _, cx| cx.new(|_| Empty)),
                )
            })
            .children(
                self.workspace
                    .read(cx)
                    .meter
                    .then(|| meter::panel("app-meter", &self.meter_at, &self.meter, window)),
            )
            .child(self.opener.clone())
    }
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
