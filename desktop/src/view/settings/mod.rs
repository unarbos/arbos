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
        self, App, Context, Div, Entity, FocusHandle, Focusable, KeyBinding, Render, SharedString,
        Window, actions, div, prelude::*, px,
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

/// The gap between a group and the caption of the next one, and between a
/// caption and the rows under it.
pub(super) const GROUP_GAP: f32 = 20.;
pub(super) const LABEL_GAP: f32 = 8.;

/// A row's own padding above and below its text. Provisional: the parity loop
/// has live Cursor and is measuring the real one — see
/// `internal/features-inbox/2026-09-17-cursor-settings-interior-measurements-ask.md`.
const ROW_PAD_Y: f32 = 10.;

/// Between a row's label and its description.
const ROW_LINE_GAP: f32 = 3.;

/// A line under a row's label. `Warn` is the only styling choice a row makes,
/// and it means *something here is wrong*, not *this is interesting*.
pub(super) enum Tone {
    Plain,
    Warn,
}

pub(super) struct Line {
    pub text: SharedString,
    pub tone: Tone,
}

impl Line {
    pub(super) fn say(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Plain,
        }
    }

    pub(super) fn warn(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Warn,
        }
    }
}

/// One settings row, Cursor's shape (Jacob, 09-17: "settings should feel more
/// like this"): the label on the left with its description under it, and the
/// control hard right.
///
/// No box, and no horizontal inset — the label lines up with the section
/// heading above it, and the hairline that divides it from the row before runs
/// the width of the column. `first` carries no hairline; the last row carries
/// none after it, because a rule with nothing under it is drawing the edge of a
/// box we deliberately do not have.
pub(super) fn row(first: bool, theme: &Theme) -> Div {
    div()
        .py(px(ROW_PAD_Y))
        .when(!first, |el| el.border_t_1().border_color(theme.border))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(16.))
}

/// A row whose control is too wide to sit hard right: the label and its
/// description above, the control on its own line under them.
///
/// Cursor's rows carry small controls — a toggle, a dropdown, a button — and a
/// row is read along its width, so a control that eats half the column leaves
/// the description wrapping in what is left. At 640px the API key's field and
/// buttons wrapped its instructions to four lines; this is the honest variant
/// for the two rows that are not a toggle.
pub(super) fn stacked_row(first: bool, theme: &Theme) -> Div {
    div()
        .py(px(ROW_PAD_Y))
        .when(!first, |el| el.border_t_1().border_color(theme.border))
        .flex()
        .flex_col()
        .gap(px(10.))
}

/// A row's left side: what the setting is, and what it does.
pub(super) fn label_block(label: impl Into<SharedString>, lines: Vec<Line>, theme: &Theme) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(theme.row_title(label.into()))
        .children(lines.into_iter().map(|line| {
            div()
                .mt(px(ROW_LINE_GAP))
                .text_style(TextStyle::Subheadline)
                .text_color(match line.tone {
                    Tone::Plain => theme.text_muted,
                    Tone::Warn => theme.warning,
                })
                .child(line.text)
        }))
}

/// The container for a run of rows: nothing around it. Cursor's groups are
/// held apart by space and a caption, not by a border — a box around three
/// rows is a box drawn to say "these three go together", which the caption
/// already says.
pub(super) fn rows() -> Div {
    div().flex().flex_col()
}

/// The muted words over a run of rows — `Colors`, `Updates`, `This machine`.
/// Quieter than the section heading and quieter than a row's own label, so it
/// groups without competing.
pub(super) fn caption(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .text_style(TextStyle::Caption)
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text_muted)
        .child(text.into())
}

/// A caption with its rows under it, which is every group in every section.
pub(super) fn group(caption_text: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(LABEL_GAP))
        .child(caption(caption_text, theme))
}

/// The content column's width, and how far it sits from the rail.
///
/// Left-aligned rather than centred, which puts it left of centre in a wide
/// window — Cursor's shape, and the reason is that a row whose label is on the
/// left and whose control is hard right is read along its width, so the width
/// has to be a fixed measure rather than whatever the window is. Centring it
/// also moved the whole form when the panel opened.
///
/// Both provisional until the parity loop measures Cursor's.
const CONTENT_WIDTH: f32 = 640.;
const CONTENT_INSET: f32 = 32.;

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
        //
        // At the top, where Cursor puts its own Back and where the eye starts,
        // rather than at the foot where it was — a way out is worth finding
        // before the list, not after it.
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
            .children(back)
            // Space under the way out, so it reads as its own thing above the
            // list rather than as a sixth section.
            .child(div().h(px(GROUP_GAP)).flex_none())
            // One run, no bands. Cursor's rail has about thirteen items in four
            // bands; five in a row already read as one band, and bands here
            // would be space around arbitrary splits. A sixth and seventh
            // section will split on their own.
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
        // No search field. In Cursor it filters a long list; over five sections
        // it is decoration, and one that matches section names while a person
        // types a setting's name ("kernel", "key") is worse than none. It earns
        // its place when it can search row labels, or when the sections outgrow
        // one screen each.
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
                    .px(px(CONTENT_INSET))
                    .py(px(CONTENT_INSET))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .w(px(CONTENT_WIDTH))
                            .max_w_full()
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
