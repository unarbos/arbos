//! The tab bar across the top of the window: one tab per open project, the
//! Settings tab after them when it is open, a `+` that opens the
//! machine-then-folder picker, and the traffic-light inset on macOS. A tab is
//! a project — a folder on a machine — and closing it closes the project; its
//! `.arbos/` stays, so opening the folder again brings it back as it was.
//!
//! Settings is the one tab that is not a project, and it reads as one: the
//! gear in the muted tone every other piece of chrome takes, rather than a
//! project's own glyph in its own colour, and none of the marks a project
//! carries — no spinner, no asking dot, no unseen badge, because nothing runs
//! in it. It sits last, after every project, and no tab in this strip can be
//! dragged, so it cannot be reordered either.

use crate::{
    model::workspace::Workspace,
    view::{
        component::{
            menu::{self, Menu},
            transcript,
        },
        root::{self, Arbos, Front, NewTab},
    },
};
use bezel::{
    gpui::{AnyElement, ClickEvent, Hsla, SharedString, Window, div, prelude::*, px},
    theme::{TextStyle, Theme, Typeset},
    ui::{icons, menu::Item, popover, tooltip::Tooltip, widgets::Buttons},
};

/// A tab's width. Cursor's tabs hug their label up to a cap; a floor
/// keeps a one-letter folder from becoming a sliver.
const TAB_MIN_WIDTH: f32 = 96.;
const TAB_MAX_WIDTH: f32 = 200.;

/// Every tab, and the `+` beside them, is one pill of this height, set on
/// the strip's centre line — the same line the traffic lights sit on
/// (`TRAFFIC_LIGHT_Y` centres them in `HEADER_HEIGHT`). The active tab
/// differs in fill and contrast only, never in size or position: Cursor's
/// tab strip is a row of equal pills, the live one shaded.
const TAB_HEIGHT: f32 = 24.;

/// Corner radius of the pill. Cursor's tabs and the composer's controls.
const TAB_RADIUS: f32 = 6.;

/// What the bar needs of a project to draw its tab, read out of the model
/// before the tab is built.
struct Tab {
    ix: usize,
    label: String,
    /// The face from `project.toml`: the glyph and the colour it takes.
    glyph: &'static str,
    color: Hsla,
    /// A turn is running in one of its agents: the glyph gives way to the
    /// spinner, with how long the earliest turn has been going.
    working: Option<std::time::Duration>,
    /// An agent has a question parked for the user.
    asking: bool,
    /// Kernel notifications for this project nobody has looked at yet.
    unseen: usize,
}

impl Arbos {
    /// The strip: one pill per open project, then `+`. Conversations sit
    /// on the quieter bar under this one.
    pub(crate) fn tab_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let workspace = self.workspace.read(cx);
        let active = workspace.active;
        let tabs: Vec<Tab> = workspace
            .projects
            .iter()
            .enumerate()
            .map(|(ix, project)| {
                // Any agent in the project, the main chat or a sub-agent
                // the kernel drives: a kernel child has no flight of its
                // own to time, so a busy one takes the shared clock.
                let working = project
                    .sessions
                    .iter()
                    .filter(|chat| !chat.closed && chat.busy())
                    .map(|chat| chat.elapsed().unwrap_or_else(transcript::live_phase))
                    .max();
                let asking = project
                    .sessions
                    .iter()
                    .filter(|chat| !chat.closed)
                    .any(|chat| chat.plan_open().any(|n| n.do_kind == "ask"));
                let unseen = project
                    .sessions
                    .iter()
                    .filter(|chat| !chat.closed)
                    .map(|chat| chat.unseen.len())
                    .sum();
                Tab {
                    ix,
                    label: Workspace::tab_label(project),
                    glyph: project.identity.glyph(),
                    color: project.identity.hsla(),
                    working,
                    asking,
                    unseen,
                }
            })
            .collect();
        // One row, one centre line. On macOS the traffic lights are drawn
        // by AppKit into this same band, centred by `TRAFFIC_LIGHT_Y`; the
        // first tab starts at `TOOLBAR_INSET`, clear of them.
        div()
            .id("tab-bar")
            .flex_none()
            .h(px(root::HEADER_HEIGHT))
            .w_full()
            // No rule under it: the strip is the chat's own surface and the
            // tabs float in it, as Cursor's do (Jacob, 09-16).
            .bg(root::chrome_bg(&theme))
            .flex()
            .flex_row()
            .items_center()
            .pl(px(root::toolbar_inset(window)))
            .pr(px(root::HEADER_INSET))
            .gap(px(4.))
            .children(tabs.into_iter().map(|tab| {
                let selected = active == Some(tab.ix) && self.front() == Front::Project;
                self.tab(tab, selected, &theme, cx)
            }))
            .children(self.settings_tab_pill(&theme, cx))
            .child(
                theme
                    .ghost("new-tab")
                    .flex_none()
                    .size(px(TAB_HEIGHT))
                    .rounded(px(TAB_RADIUS))
                    .items_center()
                    .justify_center()
                    .tooltip(|window, cx| Tooltip::with_keystroke("New tab", "⌘T", window, cx))
                    .child(
                        icons::icon(icons::system::PLUS)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.new_tab_action(&NewTab, window, cx);
                    })),
            )
            .child(div().flex_1().h_full())
            .into_any_element()
    }

    /// One tab: a pill the height of every other. The active one is
    /// filled and its label at full strength; the rest are plain on the
    /// strip and light on hover. The project's glyph, in its colour, gives
    /// way to the spinner while any of its agents works and comes back
    /// when they rest; a dot at its corner says one is asking. A close
    /// mark shows on hover and on the active tab. Double-click opens the
    /// sheet; a secondary click, the menu.
    fn tab(&self, tab: Tab, active: bool, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let ix = tab.ix;
        let group = SharedString::from(format!("tab-{ix}"));
        let glyph: AnyElement = match tab.working {
            Some(since) => transcript::spinner(since, tab.color, cx),
            None => icons::icon(tab.glyph)
                .size(px(13.))
                .text_color(tab.color)
                .into_any_element(),
        };
        let close = theme
            .ghost(("tab-close", ix))
            .flex_none()
            .size(px(16.))
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .when(!active, |el| {
                el.invisible().group_hover(group.clone(), |el| el.visible())
            })
            .tooltip(|window, cx| Tooltip::with_keystroke("Close tab", "⌘W", window, cx))
            .child(
                icons::icon(icons::system::CLOSE)
                    .size(px(11.))
                    .text_color(theme.text_muted),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.close_project(ix, cx);
            }));
        let menu = self.tab_menu_element(ix, cx);
        let pill = div()
            .id(("tab", ix))
            .group(group)
            .relative()
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
            .when(active, |el| {
                el.bg(theme.element_active).text_color(theme.text)
            })
            .when(!active, |el| {
                el.text_color(theme.text_muted)
                    .hover(|el| el.bg(theme.element_hover).text_color(theme.text))
            })
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(px(14.))
                    .h(px(14.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(glyph)
                    // Cursor's badge: a small dot at the glyph's corner when
                    // the project wants the person — a question waiting, or
                    // a reply, failure or notice nobody has looked at (#293).
                    .when(tab.asking || tab.unseen > 0, |el| {
                        el.child(
                            div()
                                .absolute()
                                .right(px(-2.))
                                .bottom(px(-2.))
                                .size(px(6.))
                                .rounded_full()
                                .bg(theme.accent)
                                .border_1()
                                .border_color(root::chrome_bg(theme)),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(tab.label)),
            )
            .child(close)
            .children(menu)
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.click_count() >= 2 {
                    this.edit_tab(ix, window, cx);
                } else {
                    this.select_project(ix, cx);
                }
            }))
            // The other buttons: a secondary click opens the menu, the
            // middle one closes the tab, as in a browser.
            .on_aux_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if event.is_middle_click() {
                    this.close_project(ix, cx);
                } else {
                    this.toggle_menu(Menu::Tab(ix), cx);
                }
            }));
        self.menu_press(pill, Menu::Tab(ix), cx).into_any_element()
    }

    /// The Settings tab's pill, when it is open: the same shape and size as
    /// every other tab — Cursor's strip is a row of equal pills, the live one
    /// shaded — with the gear where a project's glyph goes and a close mark
    /// beside its name. Clicking it brings Settings forward; the mark closes
    /// it, as ⌘W does with it in front.
    fn settings_tab_pill(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.settings_tab.as_ref()?;
        let active = self.front() == Front::Settings;
        let group = SharedString::from("tab-settings");
        let close = theme
            .ghost("tab-settings-close")
            .flex_none()
            .size(px(16.))
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .when(!active, |el| {
                el.invisible().group_hover(group.clone(), |el| el.visible())
            })
            .tooltip(|window, cx| Tooltip::with_keystroke("Close Settings", "⌘W", window, cx))
            .child(
                icons::icon(icons::system::CLOSE)
                    .size(px(11.))
                    .text_color(theme.text_muted),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.close_settings(window, cx);
            }));
        Some(
            div()
                .id("tab-settings")
                .group(group)
                .relative()
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
                .when(active, |el| {
                    el.bg(theme.element_active).text_color(theme.text)
                })
                .when(!active, |el| {
                    el.text_color(theme.text_muted)
                        .hover(|el| el.bg(theme.element_hover).text_color(theme.text))
                })
                .child(
                    div()
                        .flex_none()
                        .w(px(14.))
                        .h(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            icons::icon(icons::system::SETTINGS_MINIMALISTIC)
                                .size(px(13.))
                                .text_color(theme.text_muted),
                        ),
                )
                .child(div().flex_1().min_w_0().truncate().child("Settings"))
                .child(close)
                // Forward on whichever section it was left on, not back to
                // General: a tab click is not a fresh open.
                .on_click(cx.listener(|this, _, window, cx| {
                    this.show_settings(window, cx);
                }))
                .into_any_element(),
        )
    }

    /// The tab's menu: its face, then closing it.
    fn tab_menu_element(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.menu != Some(Menu::Tab(ix)) {
            return None;
        }
        let rows = vec![
            menu::row(
                Item::action("Edit Tab…").with_icon(icons::editing::PEN),
                move |this, window, cx| this.edit_tab(ix, window, cx),
            ),
            menu::row(
                Item::action("Close Tab")
                    .with_icon(icons::system::CLOSE)
                    .with_keystroke("⌘W"),
                move |this, _, cx| this.close_project(ix, cx),
            ),
        ];
        let key = SharedString::from(format!("tab-menu-{ix}"));
        Some(popover::anchored_menu_below(
            key.clone(),
            self.menu_card(key, rows, cx),
            None,
        ))
    }
}
