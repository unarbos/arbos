//! The tab bar across the top of the window: one tab per open project, a
//! `+` that opens the machine-then-folder picker, and the traffic-light
//! inset on macOS. A tab is a project — a folder on a machine — and closing
//! it closes the project; its `.arbos/` stays, so opening the folder again
//! brings it back as it was.

use crate::{
    assets,
    model::workspace::Workspace,
    view::{
        component::transcript,
        root::{self, Arbos, NewTab},
    },
};
use bezel::{
    gpui::{AnyElement, ClickEvent, SharedString, div, prelude::*, px},
    theme::{TextStyle, Theme, Typeset},
    ui::{icons, tooltip::Tooltip, widgets::Buttons},
};

/// A tab's width. Cursor's editor tabs hug their label up to a cap; a
/// floor keeps a one-letter folder from becoming a sliver.
const TAB_MIN_WIDTH: f32 = 96.;
const TAB_MAX_WIDTH: f32 = 200.;

/// The tab itself is shorter than the strip it stands in, so the active
/// one reads as a card set on the strip's bottom edge.
const TAB_HEIGHT: f32 = 28.;

/// Corner radius of the tab card. Same as the composer's controls.
const TAB_RADIUS: f32 = 6.;

/// What the bar needs of a project to draw its tab, read out of the model
/// before the tab is built.
struct Tab {
    ix: usize,
    label: String,
    remote: bool,
    /// Whether this is the home tab — `~/.arbos`, the folder the app lands
    /// on. It gets the house glyph.
    home: bool,
    /// A turn is running in one of its agents: the glyph gives way to the
    /// spinner, with how long the earliest turn has been going.
    working: Option<std::time::Duration>,
    /// An agent has a question parked for the user.
    asking: bool,
}

impl Arbos {
    /// The strip: tabs from the left, `+` after the last, and the rest of
    /// the width free for the window to be dragged by.
    pub(crate) fn tab_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let workspace = self.workspace.read(cx);
        let active = workspace.active;
        let tabs: Vec<Tab> = workspace
            .projects
            .iter()
            .enumerate()
            .map(|(ix, project)| {
                let working = project
                    .sessions
                    .iter()
                    .filter(|chat| !chat.closed && chat.busy())
                    .filter_map(|chat| chat.elapsed())
                    .max();
                let asking = project
                    .sessions
                    .iter()
                    .filter(|chat| !chat.closed)
                    .any(|chat| chat.plan_open().any(|n| n.do_kind == "ask"));
                Tab {
                    ix,
                    label: Workspace::tab_label(project),
                    remote: project.is_remote(),
                    home: Workspace::is_home(project),
                    working,
                    asking,
                }
            })
            .collect();
        div()
            .id("tab-bar")
            .flex_none()
            .h(px(root::HEADER_HEIGHT))
            .w_full()
            .bg(root::chrome_bg(&theme))
            .border_b_1()
            .border_color(theme.border)
            .flex()
            .flex_row()
            .items_end()
            .pl(px(root::TOOLBAR_INSET - 8.))
            .pr(px(root::HEADER_INSET))
            .gap(px(2.))
            .children(tabs.into_iter().map(|tab| {
                let selected = active == Some(tab.ix);
                self.tab(tab, selected, &theme, cx)
            }))
            .child(
                theme
                    .ghost("new-tab")
                    .flex_none()
                    .size(px(TAB_HEIGHT - 4.))
                    .mb(px(4.))
                    .ml(px(2.))
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

    /// One tab. The active one takes the content plane's tone and sits
    /// flush on the strip's bottom edge so it reads as part of the page
    /// below; the rest stay on the strip and light on hover. A close mark
    /// shows on hover and on the active tab.
    fn tab(&self, tab: Tab, active: bool, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let ix = tab.ix;
        let group = SharedString::from(format!("tab-{ix}"));
        let glyph: AnyElement = match tab.working {
            Some(since) => transcript::spinner(since, theme.text_muted, cx),
            None => {
                let path = if tab.remote {
                    assets::REMOTE_ICON
                } else if tab.home {
                    assets::HOME_ICON
                } else {
                    icons::files::FOLDER
                };
                icons::icon(path)
                    .size(px(12.))
                    .text_color(if active {
                        theme.text_muted
                    } else {
                        theme.text_faint
                    })
                    .into_any_element()
            }
        };
        let close = theme
            .ghost(("tab-close", ix))
            .flex_none()
            .size(px(18.))
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
        div()
            .id(("tab", ix))
            .group(group)
            .flex_none()
            .h(px(TAB_HEIGHT))
            .min_w(px(TAB_MIN_WIDTH))
            .max_w(px(TAB_MAX_WIDTH))
            .pl(px(10.))
            .pr(px(6.))
            .rounded_tl(px(TAB_RADIUS))
            .rounded_tr(px(TAB_RADIUS))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(7.))
            .cursor_pointer()
            .text_style(TextStyle::Callout)
            .when(active, |el| {
                el.bg(root::content_bg(theme))
                    .text_color(theme.text)
                    // The card's own outline, minus the bottom edge, so it
                    // opens into the page below.
                    .border_1()
                    .border_b_0()
                    .border_color(theme.border)
                    // One pixel over the strip's rule to hide it under the card.
                    .mb(px(-1.))
                    .pb(px(1.))
            })
            .when(!active, |el| {
                el.mb(px(4.))
                    .rounded(px(TAB_RADIUS))
                    .text_color(theme.text_muted)
                    .hover(|el| el.bg(theme.element_hover).text_color(theme.text))
            })
            .child(div().flex_none().w(px(12.)).flex().justify_center().child(glyph))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(tab.label)),
            )
            .when(tab.asking, |el| {
                el.child(
                    div()
                        .flex_none()
                        .size(px(6.))
                        .rounded_full()
                        .bg(theme.accent),
                )
            })
            .child(close)
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if event.is_middle_click() {
                    this.close_project(ix, cx);
                } else {
                    this.select_project(ix, cx);
                }
            }))
            .into_any_element()
    }
}
