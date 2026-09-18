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
        panel::{CHAT_MIN_WIDTH, DOCUMENTS_EDITABLE, MAX_WIDTH, MIN_WIDTH, PanelTab},
        surface::{Surface, SurfaceId},
    },
    view::{
        component::{menu, menu::Menu, surface as board},
        panel::{PANEL_MIN_WINDOW, PANEL_WIDTH},
        root::{self, Arbos, Pane, TogglePanel, ZoomPanel},
    },
};
use bezel::{
    gpui::{
        AnyElement, ClickEvent, Context, DragMoveEvent, Empty, MouseButton, PathPromptOptions,
        SharedString, Window, div, prelude::*, px,
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
/// (`internal/cursor-side-panel-measured.md`): a 26 px pill in a 40 px row,
/// radius 6, icon then a 6 px gap then the label, and no minimum width — a
/// panel tab hugs its label rather than sitting in an equal cell the way the
/// window's project tabs do.
const TAB_ROW_HEIGHT: f32 = 40.;
const TAB_HEIGHT: f32 = 26.;
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
            Self::Browser => "Ask the agent to open a page",
            // Says what it does rather than what we mean to do: until the
            // kernel can refuse the agent's write to a file he has open,
            // opening one is a view (`DOCUMENTS_EDITABLE`).
            Self::File if DOCUMENTS_EDITABLE => "Open a file from this folder to edit",
            Self::File => "Read a file from this folder",
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
                .child(self.panel_tab_row(&tabs, active, window, cx))
                .children(body)
                .children(on_project.then(|| self.panel_foot(&theme, cx)))
                .into_any_element(),
        )
    }

    /// The drawer's own tab row: the tabs, the `+` right after the last of
    /// them (Cursor's, measured: 16 px after the last tab, never at the
    /// far edge — `internal/cursor-side-panel-measured.md`; F-184), and the
    /// controls that widen and close the drawer on the right.
    fn panel_tab_row(
        &self,
        tabs: &[PanelTab],
        active: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let focused = self.panel_focused(window, cx);
        let expanded = self
            .workspace
            .read(cx)
            .panel()
            .is_some_and(|panel| panel.width() >= MAX_WIDTH - 8.0);
        let expand_label = if expanded {
            "Restore panel width"
        } else {
            "Expand panel"
        };
        // The strip scrolls when the tabs outgrow it; the `+` and the two
        // controls keep their room, so the strip's ceiling is the row less
        // the three buttons and their gaps.
        let width = self
            .workspace
            .read(cx)
            .panel()
            .map(|panel| panel.width())
            .unwrap_or(crate::model::panel::MIN_WIDTH);
        let strip_max = (width - 2. * 6. - 3. * (TAB_HEIGHT + 4.) - 12.).max(TAB_HEIGHT);
        div()
            .id("panel-tabs")
            .flex_none()
            .h(px(TAB_ROW_HEIGHT))
            .w_full()
            .px(px(6.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            // The row that the tab chords will move carries the accent under
            // it. The brighter label on its front tab says the same thing, but
            // measured off a still that difference is 30 levels of grey on one
            // word — not something anyone reads at a glance, which is what
            // this has to be.
            .border_b_1()
            .border_color(if focused { theme.accent } else { theme.border })
            .child(
                div()
                    .id("panel-tab-strip")
                    .flex_none()
                    .min_w_0()
                    .max_w(px(strip_max))
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
                        .ml(px(8.))
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
            .child(div().flex_1())
            .child(
                theme
                    .ghost("panel-expand")
                    .flex_none()
                    .size(px(TAB_HEIGHT))
                    .rounded(px(TAB_RADIUS))
                    .items_center()
                    .justify_center()
                    .tooltip(move |window, cx| {
                        Tooltip::with_keystroke(expand_label, "⌘\\", window, cx)
                    })
                    .child(
                        icons::icon(icons::system::WIDGET)
                            .size(px(12.))
                            .text_color(theme.text_muted),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.zoom_panel_action(&ZoomPanel, window, cx);
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
            None => board::render(&shown, Some(&place), link, window, cx),
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
    fn take_card(&mut self, card: Card, window: &mut Window, cx: &mut Context<Self>) {
        match card {
            Card::Project => {
                self.workspace
                    .update(cx, |workspace, cx| workspace.select_panel_tab(0, cx));
            }
            Card::File => self.open_file_in_panel(cx),
            // A shell of his own, straight from the kernel (#461): it answers
            // with a row marked `by: user`, so the tab fills and the drawer
            // stays on it. No composer detour any more.
            Card::Terminal => {
                self.workspace
                    .update(cx, |workspace, cx| workspace.open_shell(cx));
            }
            // A page is still the agent's to open — the kernel has one browser
            // per agent and no frame for a client to drive it — so this asks,
            // in his words, and the row arrives when the agent obliges.
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
                    .with_description("Open a file from this folder"),
                |this, _, cx| this.open_file_in_panel(cx),
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
                    .with_description("Ask the agent to open a page"),
                |this, window, cx| this.ask_in_composer("Open a browser page.", window, cx),
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
