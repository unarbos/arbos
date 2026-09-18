//! The side panel: the drawer down the right of the window, and its own row
//! of tabs.
//!
//! The chat keeps the left. Everything the person or an agent opens that is
//! not a conversation — a terminal, a job's output, a page, a document — is a
//! tab in here, beside the permanent first tab that is the project's
//! `.arbos/` view ([`crate::view::panel`]).
//!
//! The panel's tabs sit on the window's own tab strip, not a second band
//! under it. `⌘T`, `⌘⇧{` and `⌘⇧}` act on whichever row has the focus. The
//! row with the focus says so plainly — its tab in front is filled and
//! lit, the other row's is flat — and
//! [`crate::view::root::Arbos::panel_focused`] is what decides, not a
//! guess about where the mouse last was.

use crate::{
    model::{
        panel::{CHAT_MIN_WIDTH, DOCUMENTS_EDITABLE, MAX_WIDTH, MIN_WIDTH, PanelTab},
        surface::{Surface, SurfaceId},
    },
    view::{
        component::{menu, menu::Menu, surface as board},
        panel::{PANEL_MIN_WINDOW, PANEL_WIDTH},
        root::{self, Arbos},
    },
};
use bezel::{
    gpui::{
        AnyElement, ClickEvent, Context, DragMoveEvent, Empty, MouseButton, SharedString, Window,
        div, prelude::*, px,
    },
    theme::{TextStyle, Theme, Typeset},
    ui::{
        icons,
        menu::Item,
        popover,
        tooltip::Tooltip,
        widgets::{Buttons, Layout, SPLIT_HANDLE_HIT, SplitStyle},
    },
};

/// The payload of a drag on the drawer's divider. Its own type so a drag
/// of a settings slider or another split cannot move this edge.
struct PanelSplit;

/// The tab row's pills, measured off Cursor's side panel
/// (`internal/cursor-side-panel-measured.md`): a 26 px pill, radius 6,
/// icon then a 6 px gap then the label, and no minimum width — a panel tab
/// hugs its label rather than sitting in an equal cell the way the window's
/// project tabs do. They sit on the window's own tab strip now, so the
/// height matches that row (24) rather than a second 40 px band.
const TAB_HEIGHT: f32 = 24.;
const TAB_RADIUS: f32 = 6.;
const TAB_MAX_WIDTH: f32 = 180.;

/// One tile of the empty tab, and the gap between them: Cursor's are about
/// 110 by 80, 16 apart.
const CARD_WIDTH: f32 = 110.;
const CARD_HEIGHT: f32 = 80.;
const CARD_GAP: f32 = 16.;

/// What the four cards on an empty tab offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Card {
    Project,
    Terminal,
    Browser,
    File,
}

impl Card {
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
            Self::Terminal => "A shell of your own, in this folder",
            Self::Browser => "A browser page of your own",
            Self::File if DOCUMENTS_EDITABLE => "The project's folder; a file opens to edit",
            Self::File => "The project's folder as a tree",
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
        // The chat's measure wins over the drawer's: one that would squeeze it
        // below `CHAT_MIN_WIDTH` is narrowed instead. The range a width may
        // hold is the model's business (`Panel::width`).
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
                .relative()
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
                .on_drag_move(
                    cx.listener(|this, event: &DragMoveEvent<PanelSplit>, window, cx| {
                        this.drag_panel_width(event, window, cx);
                    }),
                )
                .child(self.panel_split(&theme, cx))
                .children(body)
                .children(on_project.then(|| self.panel_foot(&theme, cx)))
                .into_any_element(),
        )
    }

    /// The panel's own tabs and the `+` after them, for the window tab
    /// strip. Close is the strip's panel toggle; the expand grid and the
    /// header X are gone.
    pub(crate) fn panel_tab_pills(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let workspace = self.workspace.read(cx);
        let Some(panel) = workspace.panel() else {
            return div().into_any_element();
        };
        let tabs: Vec<PanelTab> = panel.tabs().to_vec();
        let active = panel.active();
        let focused = self.panel_focused(window, cx);
        div()
            .id("panel-tabs")
            .flex_none()
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .child(
                div()
                    .id("panel-tab-strip")
                    .flex_none()
                    .min_w_0()
                    .max_w(px(360.))
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
                self.menu_press(
                    theme
                        .ghost("panel-new-tab")
                        .relative()
                        .flex_none()
                        .ml(px(4.))
                        .size(px(TAB_HEIGHT))
                        .rounded(px(TAB_RADIUS))
                        .items_center()
                        .justify_center()
                        .tooltip(|window, cx| {
                            Tooltip::with_keystroke("Open a panel", "⌘T", window, cx)
                        })
                        .child(
                            icons::icon(icons::system::PLUS)
                                .size(px(14.))
                                .text_color(theme.text_muted),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.focus_panel(window, cx);
                            this.toggle_menu(Menu::PanelNew, cx);
                        })),
                    Menu::PanelNew,
                    cx,
                )
                .children(self.panel_new_menu(cx)),
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
        let link = workspace.panel_link();
        let (label, glyph, state) = match tab {
            PanelTab::Project => ("Project".to_string(), Some(icons::files::FOLDER), None),
            // No glyph: the `+` that made it is two pills to the left, and a
            // second one on the tab reads as a second control.
            PanelTab::New(_) => ("New tab".to_string(), None, None),
            PanelTab::Surface(id) => match workspace
                .active_project()
                .and_then(|project| project.surface(id))
            {
                Some(surface) => (
                    board::title(surface),
                    Some(board::glyph(&surface.board_kind)),
                    board::state_word(surface, link),
                ),
                // A tab is dropped the moment its surface goes, so this is
                // unreachable; drawn as gone rather than as nothing, because
                // a row the window cannot account for must never read live.
                None => ("gone".to_string(), Some(icons::system::CLOSE), Some("gone")),
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
            // The × comes with the pointer, on the front tab too: Cursor's
            // tabs show it on hover only, and the tab does not widen for
            // it (`cursor-side-panel-measured.md`, the tab row).
            .invisible()
            .group_hover(group.clone(), |el| el.visible())
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
            .children(glyph.map(|glyph| {
                icons::icon(glyph)
                    .size(px(13.))
                    .flex_none()
                    .text_color(theme.text_muted)
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(label)),
            )
            // The state it is in, in a word rather than a colour:
            // `board::state_word` is the one place those words are decided.
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
        let link = workspace.panel_link();
        self.tail_again(&shown, cx);
        let body = match shown.terminal_id().and_then(|id| self.terminals.get(id)) {
            Some(terminal) => div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(terminal.clone())
                .into_any_element(),
            None => {
                if let Some(path) = shown.path() {
                    let resolved = if path.is_absolute() {
                        path.to_path_buf()
                    } else {
                        place.path.join(path)
                    };
                    if shown.board_kind == "files" || shown.board_kind == "dir" || resolved.is_dir()
                    {
                        return Some(self.file_tree_body(&resolved, window, cx));
                    }
                    if crate::view::file_editor::is_editable(&resolved) {
                        if let Some(editor) = self.file_editors.get(&resolved) {
                            return Some(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .min_w_0()
                                    .child(editor.clone())
                                    .into_any_element(),
                            );
                        }
                    }
                }
                board::render(&shown, Some(&place), link, window, cx)
            }
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

    /// An empty tab: the four things a tab can hold, as Cursor's own empty
    /// panel draws them — a 2×2 of tiles, an icon over a label, sitting low
    /// in the panel rather than centred. Two of them the window opens itself;
    /// the other two are the agent's to open, and their tiles say so and put
    /// the request in the composer rather than pretending to do it here.
    fn panel_cards(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("panel-cards")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(CARD_GAP))
            // Cursor's tiles sit at about 60% of the panel's height — down
            // near the composer's line rather than in the middle.
            .pb(px(CARD_HEIGHT))
            .child(self.panel_card_row([Card::Project, Card::Browser], theme, cx))
            .child(self.panel_card_row([Card::Terminal, Card::File], theme, cx))
            .into_any_element()
    }

    fn panel_card_row(
        &self,
        cards: [Card; 2],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .gap(px(CARD_GAP))
            .children(
                cards
                    .into_iter()
                    .map(|card| self.panel_card(card, theme, cx)),
            )
            .into_any_element()
    }

    fn panel_card(&self, card: Card, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id(("panel-card", card as usize))
            .flex_none()
            .w(px(CARD_WIDTH))
            .h(px(CARD_HEIGHT))
            .rounded(px(8.))
            .border_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .cursor_pointer()
            .hover(|el| el.bg(theme.element_hover))
            .tooltip(move |window, cx| Tooltip::text(card.detail(), window, cx))
            .child(
                icons::icon(card.glyph())
                    .size(px(18.))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .text_style(TextStyle::Callout)
                    .text_color(theme.text)
                    .child(card.title()),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.take_card(card, window, cx)))
            .into_any_element()
    }

    /// What a card does when it is pressed.
    fn take_card(&mut self, card: Card, _window: &mut Window, cx: &mut Context<Self>) {
        match card {
            Card::Project => {
                self.workspace
                    .update(cx, |workspace, cx| workspace.select_panel_tab(0, cx));
            }
            Card::File => {
                self.workspace
                    .update(cx, |workspace, cx| workspace.open_files_tree(cx));
            }
            // A shell of his own, straight from the kernel (#461): it answers
            // with a row marked `by: user`, so the tab fills and the drawer
            // stays on it. No composer detour any more.
            Card::Terminal => {
                self.workspace
                    .update(cx, |workspace, cx| workspace.open_shell(cx));
            }
            Card::Browser => {
                self.workspace.update(cx, |workspace, cx| {
                    workspace.open_browser(None, cx);
                });
            }
        }
    }

    /// The divider on the drawer's left edge: a 9 px grab strip over the
    /// hairline the panel already paints, so a drag widens or narrows it.
    fn panel_split(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        theme
            .split_handle(bezel::gpui::Axis::Horizontal, SplitStyle::Ghost)
            .id("panel-split")
            .absolute()
            .left(px(-SPLIT_HANDLE_HIT / 2.))
            .top(px(0.))
            .h_full()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    let Some(width) = this.workspace.read(cx).panel().map(|panel| panel.width())
                    else {
                        return;
                    };
                    this.workspace.update(cx, |workspace, cx| {
                        workspace.set_panel_width(width, true, cx);
                    });
                }),
            )
            .on_drag(PanelSplit, |_, _, _, cx| cx.new(|_| Empty))
            .into_any_element()
    }

    /// Follow the pointer: the drawer's width is the space from the pointer
    /// to the window's right edge, held so the chat never drops under its
    /// reading measure.
    fn drag_panel_width(
        &mut self,
        event: &DragMoveEvent<PanelSplit>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = f32::from(window.viewport_size().width);
        let pointer = f32::from(event.event.position.x);
        let max = (viewport - CHAT_MIN_WIDTH).min(MAX_WIDTH).max(MIN_WIDTH);
        let width = (viewport - pointer).clamp(MIN_WIDTH, max);
        self.workspace.update(cx, |workspace, cx| {
            workspace.set_panel_width(width, false, cx)
        });
    }

    /// The `+` pull-down: a files browser first, then the other things a
    /// tab can hold. `⌘T` still opens an empty tab; this is the ask.
    fn panel_new_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.menu != Some(Menu::PanelNew) {
            return None;
        }
        let rows = vec![
            menu::row(
                Item::action("Browse files…")
                    .with_icon(icons::files::FOLDER_WITH_FILES)
                    .with_description("The project's folder as a tree"),
                |this, _, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| workspace.open_files_tree(cx));
                },
            ),
            menu::row(
                Item::action("Terminal")
                    .with_icon(icons::devices::TERMINAL)
                    .with_description("A shell of your own, in this folder"),
                |this, _, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| workspace.open_shell(cx));
                },
            ),
            menu::row(
                Item::action("Browser")
                    .with_icon(icons::devices::GLOBAL)
                    .with_description("A browser page of your own"),
                |this, _, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| workspace.open_browser(None, cx));
                },
            ),
            menu::row(
                Item::action("Project")
                    .with_icon(icons::files::FOLDER)
                    .with_description("Agents, processes and the project page"),
                |this, _, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| workspace.select_panel_tab(0, cx));
                },
            ),
            menu::row(Item::Separator, |_, _, _| {}),
            menu::row(
                Item::action("New tab")
                    .with_icon(icons::system::PLUS)
                    .with_keystroke("⌘T"),
                |this, _, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| workspace.new_panel_tab(cx));
                },
            ),
        ];
        Some(popover::anchored_menu_below(
            "panel-new-menu",
            self.menu_card("panel-new-menu", rows, cx),
            None,
        ))
    }
}
