//! The conversation bar: a quiet second strip under the project tabs.
//!
//! Hierarchy is Workspace → Project → Conversations. The dark strip above
//! this one is projects. This one is conversations: Main first, then a
//! divider, then each agent chat with a running or done mark. Overflow
//! collapses to `+N`. A `+` starts another chat in this project. The chat
//! itself has no switcher.

use crate::{
    model::session::ChildState,
    view::{
        component::{
            menu::{self, Menu},
            transcript,
        },
        root::{self, Arbos, Front, NewSession},
    },
};
use bezel::{
    gpui::{AnyElement, SharedString, Window, div, prelude::*, px},
    theme::{TextStyle, Theme, Typeset},
    ui::{icons, menu::Item, popover, tooltip::Tooltip, widgets::Buttons},
};

/// Shorter than the project strip above it, so the two rows do not read
/// as twins.
const BAR_HEIGHT: f32 = 28.;

/// How much width one agent chip is assumed to need when deciding what
/// fits and what goes into `+N`.
const CHIP_ESTIMATE: f32 = 148.;
const MAIN_ESTIMATE: f32 = 72.;
const DIVIDER_WIDTH: f32 = 17.;
const OVERFLOW_WIDTH: f32 = 36.;
const PLUS_WIDTH: f32 = 28.;

#[derive(Clone)]
struct AgentChip {
    id: u64,
    title: String,
    state: ChildState,
    since: std::time::Duration,
}

impl Arbos {
    /// The quiet row under the project tabs. Missing when Settings is in
    /// front, or when the window has no project.
    pub(crate) fn conversation_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.front() != Front::Project {
            return None;
        }
        let theme = Theme::of(cx).clone();
        let workspace = self.workspace.read(cx);
        let project = workspace.active_project()?;
        let main = project.main_session()?;
        let selected = workspace.active_id().unwrap_or(main);
        let main_working = project
            .session(main)
            .and_then(|chat| chat.busy().then(|| chat.elapsed().unwrap_or_default()));
        let agents: Vec<AgentChip> = self
            .visible_agent_rows(cx)
            .into_iter()
            .filter(|row| row.id != main)
            .filter_map(|row| {
                let chat = project.session(row.id)?;
                Some(AgentChip {
                    id: row.id,
                    title: workspace.display_label(row.id),
                    state: chat.child_state(),
                    since: chat.elapsed().unwrap_or_default(),
                })
            })
            .collect();
        let hidden = overflow_count(
            f32::from(window.viewport_size().width),
            root::toolbar_inset(window),
            agents.len(),
        );
        let shown = agents.len().saturating_sub(hidden);
        let overflow: Vec<AgentChip> = agents.iter().skip(shown).cloned().collect();
        let shown_agents = &agents[..shown];

        Some(
            div()
                .id("conversation-bar")
                .flex_none()
                .h(px(BAR_HEIGHT))
                .w_full()
                .bg(root::content_bg(&theme))
                .flex()
                .flex_row()
                .items_center()
                .pl(px(root::toolbar_inset(window)))
                .pr(px(root::HEADER_INSET))
                .gap(px(6.))
                .child(self.main_chip(main, selected == main, main_working, &theme, cx))
                .when(!agents.is_empty(), |bar| {
                    bar.child(divider(&theme)).children(
                        shown_agents
                            .iter()
                            .map(|chip| self.agent_chip(chip, selected == chip.id, &theme, cx)),
                    )
                })
                .children(self.overflow_chip(&overflow, &theme, cx))
                .child(self.new_conversation(&theme, cx))
                .into_any_element(),
        )
    }

    fn main_chip(
        &self,
        id: u64,
        selected: bool,
        working: Option<std::time::Duration>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dot = match working {
            Some(since) => transcript::spinner(since, theme.accent, cx),
            None => div()
                .flex_none()
                .size(px(6.))
                .rounded_full()
                .bg(if selected {
                    theme.text
                } else {
                    theme.text_faint
                })
                .into_any_element(),
        };
        chip("conversation-main", id, selected, theme, dot, "Main", None, cx)
    }

    fn agent_chip(
        &self,
        chip_data: &AgentChip,
        selected: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = chip_data.id;
        let glyph = match chip_data.state {
            ChildState::Working => transcript::spinner(chip_data.since, theme.text_muted, cx),
            ChildState::Asking
            | ChildState::Waiting
            | ChildState::Done => icons::icon(icons::system::SETTINGS_MINIMALISTIC)
                .size(px(11.))
                .text_color(if selected {
                    theme.text
                } else {
                    theme.text_muted
                })
                .into_any_element(),
        };
        chip(
            ("conversation-agent", id),
            id,
            selected,
            theme,
            glyph,
            &chip_data.title,
            status_label(chip_data.state),
            cx,
        )
    }

    fn overflow_chip(
        &self,
        hidden: &[AgentChip],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if hidden.is_empty() {
            return None;
        }
        let n = hidden.len();
        let open = self.menu == Some(Menu::ConversationOverflow);
        let menu = open.then(|| self.overflow_menu(hidden, cx));
        let trigger = div()
            .id("conversation-overflow")
            .flex_none()
            .h(px(22.))
            .px(px(8.))
            .rounded(px(6.))
            .flex()
            .flex_row()
            .items_center()
            .cursor_pointer()
            .text_style(TextStyle::Caption)
            .text_color(theme.text_muted)
            .hover(|el| el.bg(theme.element_hover).text_color(theme.text))
            .tooltip(|window, cx| Tooltip::text("More conversations", window, cx))
            .child(SharedString::from(format!("+{n}")))
            .children(menu)
            .on_click(cx.listener(|this, _, _, cx| {
                this.toggle_menu(Menu::ConversationOverflow, cx);
            }));
        Some(
            self.menu_press(trigger, Menu::ConversationOverflow, cx)
                .into_any_element(),
        )
    }

    fn overflow_menu(&self, hidden: &[AgentChip], cx: &mut Context<Self>) -> AnyElement {
        let rows = hidden
            .iter()
            .map(|chip| {
                let id = chip.id;
                let mut label = chip.title.clone();
                if let Some(status) = status_label(chip.state) {
                    label.push(' ');
                    label.push_str(status);
                }
                menu::row(
                    Item::action(label).with_icon(icons::system::SETTINGS_MINIMALISTIC),
                    move |this, _, cx| this.select_session(id, cx),
                )
            })
            .collect();
        popover::anchored_menu_below(
            SharedString::from("conversation-overflow-menu"),
            self.menu_card("conversation-overflow-menu", rows, cx),
            None,
        )
    }

    /// `+` on this bar starts another chat in the project in front. The
    /// `+` on the strip above opens a project tab.
    fn new_conversation(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        theme
            .ghost("conversation-new")
            .flex_none()
            .size(px(22.))
            .rounded(px(6.))
            .items_center()
            .justify_center()
            .tooltip(|window, cx| Tooltip::with_keystroke("New conversation", "⌘N", window, cx))
            .child(
                icons::icon(icons::system::PLUS)
                    .size(px(12.))
                    .text_color(theme.text_muted),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.new_session_action(&NewSession, window, cx);
            }))
            .into_any_element()
    }
}

fn chip(
    id: impl Into<bezel::gpui::ElementId>,
    session: u64,
    selected: bool,
    theme: &Theme,
    glyph: AnyElement,
    title: &str,
    status: Option<&'static str>,
    cx: &mut Context<Arbos>,
) -> AnyElement {
    // No underline. The live conversation is the stronger type, not a
    // rule under the word.
    div()
        .id(id)
        .flex_none()
        .h(px(22.))
        .max_w(px(220.))
        .px(px(8.))
        .rounded(px(6.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .cursor_pointer()
        .text_style(TextStyle::Caption)
        .when(selected, |el| el.text_color(theme.text))
        .when(!selected, |el| {
            el.text_color(theme.text_muted)
                .hover(|el| el.bg(theme.element_hover).text_color(theme.text))
        })
        .child(glyph)
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(SharedString::from(title.to_owned())),
        )
        .children(status.map(|status| {
            div()
                .flex_none()
                .text_style(TextStyle::Caption)
                .text_color(theme.text_faint)
                .child(status)
        }))
        .on_click(cx.listener(move |this, _, _, cx| this.select_session(session, cx)))
        .into_any_element()
}

fn divider(theme: &Theme) -> AnyElement {
    div()
        .id("conversation-divider")
        .flex_none()
        .w(px(1.))
        .h(px(12.))
        .bg(theme.border)
        .into_any_element()
}

fn status_label(state: ChildState) -> Option<&'static str> {
    match state {
        ChildState::Working => Some("running"),
        ChildState::Done => Some("done"),
        ChildState::Asking => Some("asking"),
        ChildState::Waiting => None,
    }
}

fn overflow_count(viewport: f32, inset: f32, agents: usize) -> usize {
    if agents == 0 {
        return 0;
    }
    let reserved = inset
        + root::HEADER_INSET
        + MAIN_ESTIMATE
        + DIVIDER_WIDTH
        + OVERFLOW_WIDTH
        + PLUS_WIDTH;
    let room = (viewport - reserved).max(0.);
    let fit = (room / CHIP_ESTIMATE).floor() as usize;
    agents.saturating_sub(fit)
}
