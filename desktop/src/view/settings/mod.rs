//! The Settings tab: a rail of sections, and the active section's body
//! centred under its title, filling the window's middle.
//!
//! A tab rather than a window (Jacob, 09-17: "settings should be a tab that
//! opens rather than a floating panel, it should be a full inlined tab"). It
//! paints the same surface as the chat, so the tab strip over it and the bar
//! under it stay one continuous surface with the content, and the window's own
//! traffic lights are the only ones on screen.
//!
//! The tab itself — its pill in the strip, what opens and closes it — belongs
//! to [`crate::view::root`]. This is only what it draws.

use crate::{
    kernel,
    model::{permission_center::Permissions, workspace::Workspace},
    view::root::{self, ShowChat},
    voice_ws,
};
use bezel::{
    gpui::{
        self, App, Context, Entity, FocusHandle, Focusable, KeyBinding, Render, Window, actions,
        div, prelude::*, px,
    },
    motion::{Fade, Painter},
    theme::{TextStyle, Theme, Typeset},
    ui::{
        icons,
        tooltip::Tooltip,
        widgets::{Layout, Scaffolding},
    },
};

actions!(arbos_settings, [CloseSettings]);

/// The key context the pane claims while it holds the keyboard, so Escape
/// leaves it for the chat. The root view's own `escape` handler answers when
/// the focus rests elsewhere — a pane that stopped being drawn dispatches
/// nothing — and both routes are wanted, because a settings surface with no way
/// out is the bug Jacob hit on the Project page.
const KEY_CONTEXT: &str = "ArbosSettings";

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", ShowChat, Some(KEY_CONTEXT))]);
}

mod general;
mod model;
mod performance;
mod permissions;
mod theme;
mod typography;

/// The section rail. The reference's 18rem is read against a 120rem panel;
/// against a window this size it would take a third of the width, so it matches
/// the right-hand panel's width instead.
const SIDEBAR_WIDTH: f32 = 200.;

/// The gap between a group and the label of the next one, and between a label
/// and the box under it.
pub(super) const GROUP_GAP: f32 = 20.;
pub(super) const LABEL_GAP: f32 = 8.;

/// The reading column's cap, `--container-content`. The body is centred in
/// whatever is left beside the rail, up to this — so a tab the width of the
/// window gives the same form more air around it, never a wider row. Nothing
/// here is stretched to fill the extra room.
const CONTENT_MAX_WIDTH: f32 = 860.;

/// Which section the rail has selected.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    General,
    Model,
    Permissions,
    Appearance,
    Performance,
}

impl Section {
    const ALL: [Self; 5] = [
        Self::General,
        Self::Model,
        Self::Permissions,
        Self::Appearance,
        Self::Performance,
    ];

    /// The section's name for the driver, so a test can assert which one the
    /// tab is on without reading a pixel.
    pub fn key(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Model => "model",
            Self::Permissions => "permissions",
            Self::Appearance => "appearance",
            Self::Performance => "performance",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Model => "Model",
            Self::Permissions => "Permissions",
            Self::Appearance => "Appearance",
            Self::Performance => "Performance",
        }
    }

    /// The line under the title, where the section needs one. It belongs to
    /// the header rather than the body: a subtitle sits with what it explains,
    /// and the gap under the whole block is the same either way.
    fn subtitle(self) -> Option<&'static str> {
        match self {
            Self::Model => Some("Who answers the chat, and with which key."),
            Self::Permissions => {
                Some("What the system lets Arbos do here. Each row asks for itself.")
            }
            Self::General | Self::Appearance | Self::Performance => None,
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::General => icons::system::SETTINGS_MINIMALISTIC,
            Self::Model => icons::system::KEY_MINIMALISTIC,
            Self::Permissions => icons::media::MICROPHONE,
            Self::Appearance => icons::system::SUN,
            Self::Performance => icons::devices::CPU,
        }
    }
}

pub struct SettingsPane {
    workspace: Entity<Workspace>,
    section: Section,
    host: model::HostPanel,
    /// The permissions rows' re-check loop is running.
    rechecking: bool,
    /// Holds the keyboard while the tab is in front, so Escape reaches
    /// [`KEY_CONTEXT`].
    focus: FocusHandle,
}

impl Focusable for SettingsPane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SettingsPane {
    pub fn new(workspace: Entity<Workspace>, section: Section, cx: &mut Context<Self>) -> Self {
        // The permission rows are the centre's; follow it.
        let center = cx.global::<Permissions>().0.clone();
        cx.observe(&center, |_, _, cx| cx.notify()).detach();
        // General compares the kernel it is talking to against the one this app
        // ships, and reading that runs the binary. Off the window's thread, so
        // the first paint of the tab does not wait for a subprocess and the row
        // does not sit on "not read yet".
        cx.background_executor()
            .spawn(async { kernel::warm_bundled_commit() })
            .detach();
        Self {
            workspace,
            section,
            host: model::HostPanel::new(cx),
            rechecking: false,
            focus: cx.focus_handle(),
        }
    }

    pub fn section(&self) -> Section {
        self.section
    }

    /// What is on this machine can change while the tab sits open — another
    /// install, a directory removed by hand — so the section's list is re-read
    /// on the way in rather than trusted from whenever it was opened.
    pub fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        if section == Section::Model {
            // config.toml may have been edited by hand or by `arbos-kernel
            // setup` since the tab opened.
            self.host.refresh();
        }
        cx.notify();
    }

    /// The tab left the front, or closed. Unlike the window this replaced, the
    /// pane outlives being looked at, so what only makes sense in front of
    /// somebody — the permission rows' poll, the microphone test — is stopped
    /// here rather than by the entity being dropped.
    pub fn went_behind(&mut self, cx: &mut Context<Self>) {
        self.stop_rechecking(cx);
    }

    pub(super) fn stop_rechecking(&mut self, cx: &mut Context<Self>) {
        if !self.rechecking {
            return;
        }
        self.rechecking = false;
        voice_ws::mic_test_stop();
        let center = cx.global::<Permissions>().0.clone();
        center.update(cx, |center, _| center.unwatch());
    }

    fn rail(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = Theme::of(cx).clone();
        let painter = Painter::of(cx);
        // A way back in words, not only a chord: Jacob opened the Project page
        // and could not find the way out of it. The tab's own close mark, ⌘1
        // and Escape all do this too. With no project open there is no chat to
        // go back to, and the tab's close mark is the way out.
        let back = self.workspace.read(cx).active.is_some().then(|| {
            theme
                .nav_row(
                    Some(icons::system::CHAT_ROUND_LINE),
                    "Back to chat",
                    false,
                    Fade::new(painter, "settings-back"),
                )
                .id("settings-back-to-chat")
                .tooltip(|window, cx| {
                    Tooltip::with_keystroke("Back to the chat (Esc)", "⌘1", window, cx)
                })
                .on_click(cx.listener(|_, _, window, cx| {
                    window.dispatch_action(Box::new(ShowChat), cx);
                }))
        });
        div()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            // The same surface as the content and the strips, with the hairline
            // the right-hand panel also keeps: a column edge, not a band.
            .bg(root::chrome_bg(&theme))
            .border_r_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(8.))
            .children(Section::ALL.into_iter().enumerate().map(|(ix, section)| {
                theme
                    .nav_row(
                        Some(section.glyph()),
                        section.title(),
                        section == self.section,
                        Fade::new(painter, format!("section-{ix}")),
                    )
                    .id(("section", ix))
                    .on_click(cx.listener(move |this, _, _, cx| this.show(section, cx)))
            }))
            .child(div().flex_1())
            .children(back)
    }
}

impl Render for SettingsPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .flex()
            .flex_row()
            // The content's own fill, as the chat column takes it: the strip
            // above and the bar below are this same surface (Jacob, 09-16).
            .bg(root::content_bg(&theme))
            .font_family(theme.font_sans.clone())
            .text_color(theme.text)
            .text_style(TextStyle::Body)
            .child(self.rail(cx))
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .px(px(32.))
                    .py(px(32.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(CONTENT_MAX_WIDTH))
                            .flex()
                            .flex_col()
                            // The header block, held off its body by the gap
                            // that separates any two groups. Nothing set this
                            // before, so the page title leaned on `group_box`'s
                            // own margin and came out with less air under it
                            // than a field label gets — and none at all in a
                            // section that opens on a label rather than a box.
                            .child(
                                div()
                                    .mb(px(GROUP_GAP))
                                    .child(theme.page_header(self.section.title(), None))
                                    .children(
                                        self.section
                                            .subtitle()
                                            .map(|copy| theme.page_subtitle(copy)),
                                    ),
                            )
                            .child(match self.section {
                                Section::General => self.general_body(cx),
                                Section::Model => self.model_body(cx),
                                Section::Permissions => self.permissions_body(cx),
                                Section::Appearance => self.appearance_body(cx),
                                Section::Performance => self.performance_body(cx),
                            }),
                    ),
            )
    }
}
