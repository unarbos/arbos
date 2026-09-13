//! Arbos desktop — Arbos look, Rust kernel (`arbos-kernel serve`).

use anyhow::Result;
use bezel::{
    gpui::App,
    gpui_platform,
    theme::{self, Tint, appearance},
    ui::{self, focus, input},
};
use arbos_desktop::{
    assets, memory,
    model::{settings, state, workspace},
    reading,
    view::{
        component::{composer, opener, tab_sheet},
        menubar, root,
    },
};

fn main() -> Result<()> {
    arbos_desktop::kernel::install_shutdown();
    let _tunnels = arbos_desktop::kernel::TunnelGuard;
    let settings = settings::load().unwrap_or_else(|err| {
        eprintln!("settings: {err:#}; using defaults");
        settings::Settings::default()
    });
    let state = state::restore();
    let app = gpui_platform::application().with_assets(assets::Assets);
    // The Dock icon and a second launch both land here. ⌘W leaves the app
    // running with no window, as it does in every other mac app, so this is
    // the way back to one.
    app.on_reopen(|cx| {
        if cx
            .windows()
            .iter()
            .any(|window| window.downcast::<root::Arbos>().is_some())
        {
            return;
        }
        // ⌘W of the chat window used to leave Settings holding a dead
        // Workspace. Drop those leftovers before a new one is made.
        for window in cx.windows() {
            if let Some(handle) = window.downcast::<arbos_desktop::view::settings::SettingsWindow>() {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
        }
        let settings = settings::load().unwrap_or_else(|err| {
            eprintln!("settings: {err:#}; using defaults");
            settings::Settings::default()
        });
        let _ = root::open(settings, state::restore(), cx);
    });
    app.run(move |cx: &mut App| {
        if let Err(err) = ui::register_fonts(cx) {
            eprintln!("font registration failed: {err:?}");
        }
        // Cursor's colours, registered before the first palette is built.
        arbos_desktop::view::palette::install(cx);
        appearance::init(state.appearance, cx);
        // Before the window is opened: it reads its background appearance
        // on the way up, and vibrancy is what decides that.
        workspace::apply_transparency(state.reduce_transparency, cx);
        workspace::apply_tint(Tint::new(state.hue, state.chroma), cx);
        input::set_caret_blink(state.cursor_blink, cx);
        theme::set_base_text_size(state.text_size, cx);
        reading::set_bionic(state.bionic_reading, cx);
        markdown::set_highlighter(
            cx,
            |language, code| syntax::highlight(code, language),
            syntax::lang::LANGS.iter().map(|lang| lang.name),
        );
        memory::init(settings.cover_memory * 1_000_000, cx);
        input::init(cx);
        focus::init(cx);
        composer::init(cx);
        opener::init(cx);
        tab_sheet::init(cx);
        editor::init(cx);
        root::init(cx);
        // Last: it reads every binding above off the keymap to put the
        // shortcuts beside its items.
        menubar::init(cx);

        let window = root::open(settings, state, cx).expect("failed to open window");
        // Test harness: `ARBOS_DRIVER=1` / `ARBOS_DRIVER_SOCKET=…` opens the
        // control socket a driver program clicks and types through.
        if arbos_desktop::driver::socket_path().is_some()
            && let Err(err) = arbos_desktop::driver::start(window, cx)
        {
            eprintln!("driver: {err:#}");
        }
        cx.activate(true);
    });
    Ok(())
}
