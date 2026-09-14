//! The macOS menu bar: the tree, the commands only it names, and the wiring
//! that decides which of them are live.
//!
//! An item carries an action and a name, never a shortcut. `set_menus` reads
//! the equivalent off the keymap, so the `bind_keys` in each view's `init`
//! stays the one place a chord is written — at the price of two rules:
//!
//! * every `init` runs before this module does, or an item is built for an
//!   action whose binding is not registered yet and shows no shortcut at all;
//! * an action bound inside a key context does not belong here. AppKit claims
//!   a key equivalent before gpui sees the keystroke and dispatches it straight
//!   at the focused path with the context unread, so an item for the composer's
//!   `enter` would make `enter` mean "send" in every field in the app.
//!
//! Which items are live is not written here either. macOS validates each one
//! against [`bezel::gpui::App::is_action_available`] on every open and before
//! every key equivalent, so an item greys itself exactly when nothing in the
//! focused path handles its action — which is what [`Arbos::commands`] is
//! for, and why a greyed item's shortcut still reaches the keymap underneath.

use crate::view::root::{
    Arbos, CloseProject, EndCall, NewSession, NewTab, NextEntry, NextTab, OpenProject,
    OpenSettings, PrevEntry, PrevTab, SearchChats, ShowPermissions, ShowProject, StartCall,
    ToggleMute, TogglePanel, ZoomIn, ZoomOut, ZoomReset,
};
use bezel::{
    gpui::{
        self, App, Context, Div, KeyBinding, Menu, MenuItem, OsAction, Window, actions, prelude::*,
    },
    ui::input,
};

actions!(
    arbos,
    [
        CloseWindow,
        Hide,
        HideOthers,
        Minimize,
        Quit,
        ShowAll,
        ToggleFullScreen,
        Zoom
    ]
);

pub fn init(cx: &mut App) {
    // A nib gives an app these; there is no nib here, and an item whose action
    // nothing has bound shows no shortcut and answers to none — ⌘Q included.
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        // ⌘W is the tab's — see `root::init`; the window closes on the
        // browser's chord for it.
        KeyBinding::new("cmd-shift-w", CloseWindow, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
    ]);

    cx.on_action(|_: &Quit, cx: &mut App| {
        crate::kernel::shutdown_tunnels();
        cx.quit();
    });
    // Session files are written on their own thread. This runs after the
    // windows are gone — after the composer's last draft flush — and holds
    // the process until what was queued has landed, however the quit came.
    cx.on_app_quit(|_| async { crate::model::record::settle() })
        .detach();
    cx.on_action(|_: &Hide, cx: &mut App| cx.hide());
    cx.on_action(|_: &HideOthers, cx: &mut App| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx: &mut App| cx.unhide_other_apps());
    cx.on_action(|_: &Minimize, cx: &mut App| front(cx, |window| window.minimize_window()));
    cx.on_action(|_: &Zoom, cx: &mut App| front(cx, |window| window.zoom_window()));
    // Native Spaces fullscreen is a black desktop — the frost dies. Simple
    // fullscreen stays on this Space so the wallpaper is still behind the glass.
    cx.on_action(|_: &ToggleFullScreen, cx: &mut App| {
        front(cx, |window| {
            #[cfg(target_os = "macos")]
            window.toggle_simple_fullscreen();
            #[cfg(not(target_os = "macos"))]
            window.toggle_fullscreen();
        })
    });
    // The workspace goes with its window. Transcripts are on disk and
    // resumable; tunnels this process opened are torn down on ⌘Q. The
    // kernel stays up, and the Dock opens the window again.
    cx.on_action(|_: &CloseWindow, cx: &mut App| {
        let closing_main = cx
            .active_window()
            .and_then(|window| window.downcast::<Arbos>())
            .is_some();
        if closing_main {
            crate::kernel::shutdown_tunnels();
            for window in cx.windows() {
                if let Some(handle) = window.downcast::<crate::view::settings::SettingsWindow>() {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                }
            }
        }
        front(cx, |window| window.remove_window());
    });

    // Both are handled in the workspace window as well. A global handler runs
    // only once every element in the focused path has declined, so these are
    // what answers when the window in front is the settings one.
    cx.on_action(|_: &OpenProject, cx: &mut App| {
        workspace(cx, |this, window, cx| {
            this.open_project_action(&OpenProject, window, cx)
        })
    });
    cx.on_action(|_: &OpenSettings, cx: &mut App| {
        workspace(cx, |this, window, cx| {
            this.open_settings_action(&OpenSettings, window, cx)
        })
    });
    // Zoom is one size for the whole app, so it answers from any window —
    // settings included, where the stepper sits next to it.
    cx.on_action(|_: &ZoomIn, cx: &mut App| workspace_quiet(cx, |this, cx| this.zoom_by(1., cx)));
    cx.on_action(|_: &ZoomOut, cx: &mut App| workspace_quiet(cx, |this, cx| this.zoom_by(-1., cx)));
    cx.on_action(|_: &ZoomReset, cx: &mut App| {
        workspace_quiet(cx, |this, cx| {
            let default = bezel::theme::TextStyle::Body.size();
            this.workspace
                .update(cx, |workspace, cx| workspace.set_text_size(default, cx));
            cx.notify();
        })
    });

    cx.set_menus(menus());
}

/// The tree.
///
/// No About: an about panel is a window this app does not have, and an item
/// that opens a web page in its place is not one.
fn menus() -> Vec<Menu> {
    vec![
        // Titled for the unbundled binary alone — a bundle takes the first
        // menu's name from `CFBundleName`, which is this same lowercase word.
        Menu::new("Arbos").items([
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::action("Permissions…", ShowPermissions),
            MenuItem::separator(),
            MenuItem::action("Hide Arbos", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Arbos", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Tab", NewTab),
            MenuItem::action("New Sub-chat", NewSession),
            MenuItem::separator(),
            MenuItem::action("Open Project…", OpenProject),
            MenuItem::action("Close Tab", CloseProject),
            MenuItem::separator(),
            MenuItem::action("Close Window", CloseWindow),
        ]),
        // The text field's actions, which the article's editor does not answer
        // to: it keeps a vocabulary of its own that this crate cannot name. Its
        // ⌘C is its own and reaches it, because macOS leaves a greyed item's
        // key equivalent alone — the item beside it is what goes dim.
        //
        // `os_action` puts them on the responder chain under the names AppKit
        // knows, so cut and paste work in the open panel a project is picked
        // from as well as in our own fields.
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
            MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
            MenuItem::separator(),
            MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("View").items([
            MenuItem::action("Toggle Panel", TogglePanel),
            MenuItem::action("Show Project", ShowProject),
            MenuItem::action("Search Chats…", SearchChats),
            MenuItem::separator(),
            // A call to the project in front, through the speech server.
            MenuItem::action("Call Project", StartCall),
            MenuItem::action("End Call", EndCall),
            MenuItem::action("Mute", ToggleMute),
            MenuItem::separator(),
            // Drawn ⇧⌘] and ⇧⌘[, which is why those are bound first: the
            // `ctrl-tab` pair these also answer to is a chord gpui cannot
            // hand macOS, and an item that named it would teach ⌃T.
            MenuItem::action("Next Tab", NextTab),
            MenuItem::action("Previous Tab", PrevTab),
            MenuItem::separator(),
            MenuItem::action("Next Agent", NextEntry),
            MenuItem::action("Previous Agent", PrevEntry),
            MenuItem::separator(),
            MenuItem::action("Actual Size", ZoomReset),
            MenuItem::action("Zoom In", ZoomIn),
            MenuItem::action("Zoom Out", ZoomOut),
            MenuItem::separator(),
            MenuItem::action("Enter Full Screen", ToggleFullScreen),
        ]),
        // Named exactly this: `setWindowsMenu:` is hung off the title, and it
        // is what appends the window list under whatever we put in it.
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
        ]),
    ]
}

/// Run `f` on the window in front. A window command means whichever window
/// that is: ⌘M over settings minimises settings.
fn front(cx: &mut App, f: impl FnOnce(&mut Window)) {
    if let Some(window) = cx.active_window() {
        let _ = window.update(cx, |_, window, _| f(window));
    }
}

/// Run `f` on the workspace window, brought forward first. Looked up rather
/// than held: ⌘W closes that window and the Dock opens another, so a handle
/// taken at launch outlives the window it names.
fn workspace(cx: &mut App, f: impl FnOnce(&mut Arbos, &mut Window, &mut Context<Arbos>)) {
    let Some(handle) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<Arbos>())
    else {
        return;
    };
    let _ = handle.update(cx, |this, window, cx| {
        window.activate_window();
        f(this, window, cx);
    });
}

/// Run `f` on the workspace window without bringing it forward. For a
/// setting that shows everywhere at once, the window in front stays there.
fn workspace_quiet(cx: &mut App, f: impl FnOnce(&mut Arbos, &mut Context<Arbos>)) {
    let Some(handle) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<Arbos>())
    else {
        return;
    };
    let _ = handle.update(cx, |this, _, cx| f(this, cx));
}

impl Arbos {
    /// Hang the menu's commands on the root, each under the condition that
    /// makes it mean something — a board cannot be started in a window with no
    /// project open, so with none there is nothing here to handle `NewBoard`
    /// and macOS greys the item.
    ///
    /// The window's own editing actions are not here: they are bound inside a
    /// key context, no menu names them, and they are wanted for as long as the
    /// field they belong to is up.
    pub(crate) fn commands(&self, root: Div, cx: &mut Context<Self>) -> Div {
        let workspace = self.workspace.read(cx);
        let project = workspace.active.is_some();
        let entries = self.showing(cx).is_some();

        root.on_action(cx.listener(Self::toggle_panel_action))
            .on_action(cx.listener(Self::start_call_action))
            .on_action(cx.listener(Self::end_call_action))
            .on_action(cx.listener(Self::toggle_mute_action))
            .on_action(cx.listener(Self::open_project_action))
            .on_action(cx.listener(Self::new_tab_action))
            .on_action(cx.listener(Self::open_settings_action))
            .on_action(cx.listener(Self::show_permissions_action))
            .on_action(cx.listener(Self::attach_paths_action))
            .on_action(cx.listener(Self::show_chat))
            .on_action(cx.listener(Self::show_project))
            .on_action(cx.listener(Self::search_chats))
            .on_action(cx.listener(Self::zoom_in_action))
            .on_action(cx.listener(Self::zoom_out_action))
            .on_action(cx.listener(Self::zoom_reset_action))
            .when(project, |root| {
                root.on_action(cx.listener(Self::close_project_action))
                    .on_action(cx.listener(Self::new_session_action))
                    .on_action(cx.listener(Self::next_tab))
                    .on_action(cx.listener(Self::prev_tab))
            })
            .when(entries, |root| {
                root.on_action(cx.listener(Self::next_entry))
                    .on_action(cx.listener(Self::prev_entry))
                    .on_action(cx.listener(Self::copy_chat))
                    .on_action(cx.listener(Self::paste_chat))
                    .on_action(cx.listener(Self::delete_chat))
            })
    }
}
