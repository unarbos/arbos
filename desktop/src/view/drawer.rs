//! The side panel: the drawer down the right of the window, and its own row
//! of tabs.
//!
//! The chat keeps the left. Everything the person or an agent opens that is
//! not a conversation — a terminal, a job's output, a page, a document — is a
//! tab in here, beside the permanent first tab that is the project's
//! `.arbos/` view ([`crate::view::panel`]).
//!
//! Two rows of tabs are now on screen, and one pair of chords drives both:
//! `⌘T`, `⌘⇧{` and `⌘⇧}` act on whichever row has the focus. So the row with
//! the focus says so plainly — its tab in front is filled and lit, the other
//! row's is flat — and [`crate::view::root::Arbos::panel_focused`] is what
//! decides, not a guess about where the mouse last was.

use crate::{
    model::{
        panel::PanelTab,
        surface::{Surface, SurfaceId},
    },
    view::{
        component::surface as board,
        panel::{PANEL_MIN_WINDOW, PANEL_WIDTH},
        root::{self, Arbos, Pane, TogglePanel},
    },
};
use bezel::{
    gpui::{
        AnyElement, ClickEvent, Context, MouseButton, PathPromptOptions, SharedString, Window, div,
        prelude::*, px,
    },
    theme::{TextStyle, Theme, Typeset},
    ui::{icons, tooltip::Tooltip, widgets::Buttons},
};

/// The tab row's height and its pills, as the window's own tab strip draws
/// them: one row of equal pills, the live one shaded.
const TAB_HEIGHT: f32 = 24.;
const TAB_RADIUS: f32 = 6.;
const TAB_MIN_WIDTH: f32 = 84.;
const TAB_MAX_WIDTH: f32 = 180.;

/// How much of the window the drawer may take. The chat is this app's centre
/// of gravity and never goes below a readable measure, so a wide drawer on a
/// small window is clamped rather than honoured.
const CHAT_MIN_WIDTH: f32 = 560.;

/// What the four cards on an empty tab offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Card {
    Project,
    Terminal,
    Browser,
    File,
}

impl Card {
    const ALL: [Card; 4] = [Card::Project, Card::Terminal, Card::Browser, Card::File];

    fn title(self) -> &'static str {
        match self {
            Self::Project => "Project",
            Self::Terminal => "Terminal",
            Self::Browser => "Browser",
            Self::File => "File",
        }
    }

    /// What the card does, in the words of what will happen. The two that ask
    /// the agent say so: it opens them, and they appear here as tabs.
    fn detail(self) -> &'static str {
        match self {
            Self::Project => "Agents, processes and the project page",
            Self::Terminal => "Ask the agent for a shell here",
            Self::Browser => "Ask the agent to open a page",
            Self::File => "Open a file from this folder",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Project => icons::files::FOLDER,
            Self::Terminal => icons::devices::TERMINAL,
            Self::Browser => icons::devices::GLOBAL,
            Self::File => icons::files::DOCUMENT,
        }
    }
}

impl Arbos {
    /// The drawer, or nothing when it is closed or the window is too narrow
    /// to give it room.
    pub(crate) fn panel(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = Theme::of(cx).clone();
        let workspace = self.workspace.read(cx);
        let panel = workspace.panel()?;
        let viewport = f32::from(window.viewport_size().width);
        if !panel.open || viewport < PANEL_MIN_WINDOW {
            return None;
        }
        let tabs: Vec<PanelTab> = panel.tabs().to_vec();
        let active = panel.active();
        // The chat's measure wins: a drawer that would squeeze it below
        // `CHAT_MIN_WIDTH` is narrowed instead.
        let width = panel
            .width()
            .min((viewport - CHAT_MIN_WIDTH).max(PANEL_WIDTH));
        let on_project = tabs.get(active) == Some(&PanelTab::Project);
        let body = match tabs.get(active).copied().unwrap_or(PanelTab::Project) {
            PanelTab::Project => self.panel_store_body(window, cx),
            PanelTab::Surface(id) => self.panel_surface_body(id, window, cx),
            PanelTab::New(_) => Some(self.panel_cards(&theme, cx)),
        };
        Some(
            div()
                .id("panel")
                .key_context(root::PANEL_CONTEXT)
                .track_focus(&self.panel_focus)
                .flex_none()
                .w(px(width))
                .h_full()
                .bg(root::chrome_bg(&theme))
                .border_l_1()
                .border_color(theme.border)
                .flex()
                .flex_col()
                // A click anywhere in the drawer is what gives it the focus,
                // and with it the tab chords. Nothing else moves the focus
                // here: no frame, no agent, no tab opening by itself.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.focus_panel(window, cx)),
                )
                .child(self.panel_tab_row(&tabs, active, window, cx))
                .children(body)
                .children(on_project.then(|| self.panel_foot(&theme, cx)))
                .into_any_element(),
        )
    }

    /// The drawer's own tab row: `+` on the left, the tabs after it, and the
    /// control that closes the drawer on the right.
    fn panel_tab_row(
        &self,
        tabs: &[PanelTab],
        active: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let focused = self.panel_focused(window, cx);
        div()
            .id("panel-tabs")
            .flex_none()
            .h(px(root::HEADER_HEIGHT))
            .w_full()
            .px(px(6.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .border_b_1()
            .border_color(theme.border)
            .child(
                theme
                    .ghost("panel-new-tab")
                    .flex_none()
                    .size(px(TAB_HEIGHT))
                    .rounded(px(TAB_RADIUS))
                    .items_center()
                    .justify_center()
                    .tooltip(|window, cx| {
                        Tooltip::with_keystroke("New panel tab", "⌘T", window, cx)
                    })
                    .child(
                        icons::icon(icons::system::PLUS)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.focus_panel(window, cx);
                        this.workspace
                            .update(cx, |workspace, cx| workspace.new_panel_tab(cx));
                    })),
            )
            .child(
                div()
                    .id("panel-tab-strip")
                    .flex_1()
                    .min_w_0()
                    .overflow_x_scroll()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .children(tabs.iter().enumerate().map(|(at, tab)| {
                        self.panel_tab(at, *tab, at == active, focused, &theme, cx)
                    })),
            )
            .child(
                theme
                    .ghost("panel-close")
                    .flex_none()
                    .size(px(TAB_HEIGHT))
                    .rounded(px(TAB_RADIUS))
                    .items_center()
                    .justify_center()
                    .tooltip(|window, cx| Tooltip::with_keystroke("Close panel", "⌘B", window, cx))
                    .child(
                        icons::icon(icons::system::CLOSE)
                            .size(px(12.))
                            .text_color(theme.text_muted),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_panel_action(&TogglePanel, window, cx);
                    })),
            )
            .into_any_element()
    }

    /// One tab. The tab in front is filled; it is *lit* only while the drawer
    /// has the focus, which is how a person can tell which row `⌘⇧}` is about
    /// to move without pressing it.
    fn panel_tab(
        &self,
        at: usize,
        tab: PanelTab,
        active: bool,
        focused: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group = SharedString::from(format!("panel-tab-{at}"));
        let workspace = self.workspace.read(cx);
        let (label, glyph, state) = match tab {
            PanelTab::Project => ("Project".to_string(), icons::files::FOLDER, None),
            PanelTab::New(_) => ("New tab".to_string(), icons::system::PLUS, None),
            PanelTab::Surface(id) => match workspace
                .active_project()
                .and_then(|project| project.surface(id))
            {
                Some(surface) => (
                    board::title(surface),
                    board::glyph(&surface.board_kind),
                    board::state_word(surface),
                ),
                // A tab is dropped the moment its surface goes, so this is
                // unreachable; drawn as gone rather than as nothing, because
                // a row the window cannot account for must never read live.
                None => ("gone".to_string(), icons::system::CLOSE, Some("gone")),
            },
        };
        let closable = at != 0;
        let close = theme
            .ghost(("panel-tab-close", at))
            .flex_none()
            .size(px(16.))
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .when(!active, |el| {
                el.invisible().group_hover(group.clone(), |el| el.visible())
            })
            .tooltip(|window, cx| Tooltip::text("Close tab", window, cx))
            .child(
                icons::icon(icons::system::CLOSE)
                    .size(px(11.))
                    .text_color(theme.text_muted),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.workspace
                    .update(cx, |workspace, cx| workspace.close_panel_tab(at, cx));
            }));
        div()
            .id(("panel-tab", at))
            .group(group)
            .flex_none()
            .h(px(TAB_HEIGHT))
            .min_w(px(TAB_MIN_WIDTH))
            .max_w(px(TAB_MAX_WIDTH))
            .pl(px(8.))
            .pr(px(4.))
            .rounded(px(TAB_RADIUS))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .text_style(TextStyle::Callout)
            .when(active, |el| el.bg(theme.element_active))
            .when(active && focused, |el| el.text_color(theme.text))
            .when(active && !focused, |el| el.text_color(theme.text_muted))
            .when(!active, |el| {
                el.text_color(theme.text_muted)
                    .hover(|el| el.bg(theme.element_hover).text_color(theme.text))
            })
            .child(
                icons::icon(glyph)
                    .size(px(13.))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(label)),
            )
            // The state a row is in, on its face and in a word: `running`,
            // `exit 0`, `stopped`, `gone`. Never a colour on its own.
            .children(state.map(|word| {
                div()
                    .flex_none()
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child(SharedString::from(word))
            }))
            .children(closable.then_some(close))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.focus_panel(window, cx);
                this.workspace
                    .update(cx, |workspace, cx| workspace.select_panel_tab(at, cx));
            }))
            .into_any_element()
    }

    /// A surface's body, with no header of its own: the tab is its title.
    fn panel_surface_body(
        &self,
        id: SurfaceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let workspace = self.workspace.read(cx);
        let project = workspace.active_project()?;
        let shown: Surface = project.surface(id)?.clone();
        let place = project.place();
        self.tail_again(&shown, cx);
        let body = match shown.terminal_id().and_then(|id| self.terminals.get(id)) {
            Some(terminal) => div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(terminal.clone())
                .into_any_element(),
            None => board::render(&shown, Some(&place), window, cx),
        };
        Some(
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .flex()
                .flex_col()
                .child(body)
                .into_any_element(),
        )
    }

    /// An empty tab: the four things a tab can hold. Two of them the window
    /// opens itself; the other two are the agent's to open, and their cards
    /// say so and put the request in the composer rather than pretending to
    /// do it here.
    fn panel_cards(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("panel-cards")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(
                Card::ALL
                    .into_iter()
                    .map(|card| self.panel_card(card, theme, cx)),
            )
            .into_any_element()
    }

    fn panel_card(&self, card: Card, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id(("panel-card", card as usize))
            .flex_none()
            .w_full()
            .p(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(theme.border)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .cursor_pointer()
            .hover(|el| el.bg(theme.element_hover))
            .child(
                icons::icon(card.glyph())
                    .size(px(16.))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_style(TextStyle::Body)
                            .text_color(theme.text)
                            .child(card.title()),
                    )
                    .child(
                        div()
                            .text_style(TextStyle::Caption)
                            .text_color(theme.text_faint)
                            .child(card.detail()),
                    ),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.take_card(card, window, cx)))
            .into_any_element()
    }

    /// What a card does when it is pressed.
    fn take_card(&mut self, card: Card, window: &mut Window, cx: &mut Context<Self>) {
        match card {
            Card::Project => {
                self.workspace
                    .update(cx, |workspace, cx| workspace.select_panel_tab(0, cx));
            }
            Card::File => self.open_file_in_panel(cx),
            // The kernel owns the shell and the page: it has no frame yet for
            // a client to ask for either, so the honest thing a card can do
            // is put the words in the composer for him to send. When the
            // kernel opens it, it arrives as a tab here.
            Card::Terminal => self.ask_in_composer("Open a terminal in this folder.", window, cx),
            Card::Browser => self.ask_in_composer("Open a browser page.", window, cx),
        }
    }

    /// Put a request in the composer and leave sending it to him — the same
    /// path the chat's pencil uses.
    fn ask_in_composer(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.show_pane(Pane::Chat, cx);
        self.workspace.update(cx, |workspace, cx| {
            workspace.edit_in_composer(text.to_string(), cx);
        });
        self.focus_composer(window, cx);
    }

    /// Cursor's Open File, into the tab in front: the system picker, then the
    /// file as a document.
    fn open_file_in_panel(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let title = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                this.open_store_file(path, &title, cx);
            });
        })
        .detach();
    }
}
