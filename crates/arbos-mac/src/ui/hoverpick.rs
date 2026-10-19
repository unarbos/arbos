//! Hover-to-expand settings rows for the agent's profile page: Model (with a
//! search field), Reasoning and Voice. Resting the pointer on a row drops its
//! list down in place; the list folds when the pointer leaves the row and its
//! list, on Esc, or on a click elsewhere. A click on the row toggles it too
//! (trackpads). Picking a value applies it at once through the same state
//! handlers the composer and the voice sheet use; the row stays open so the
//! checkmark confirms the choice until the pointer moves on.

use gpui::{
    div, prelude::*, px, AnyElement, Context, Div, ElementId, Focusable as _, FontWeight,
    SharedString, Stateful, Window,
};

use crate::app::AgentPick;
use crate::model::{Effort, Id, Project};
use crate::sync::fixture::MODELS;
use crate::ui::composer::Composer;
use crate::ui::icons::icon;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, TEXT_BODY, TEXT_SMALL, TEXT_TINY};
use crate::ui::voicepicker;

/// How tall the model / voice lists may grow before they scroll.
const LIST_MAX_HEIGHT: f32 = 260.0;

/// One info-card row: caption over the current value, a chevron, and —
/// while `open` — `panel` dropped down under it. Hover opens, leaving closes.
pub fn row(
    caption: &str,
    value: impl Into<SharedString>,
    which: AgentPick,
    open: bool,
    panel: Option<AnyElement>,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let id = match which {
        AgentPick::Model => "agent-model-row",
        AgentPick::Reasoning => "agent-reasoning-row",
        AgentPick::Voice => "agent-voice-row",
    };
    let header = div()
        .id(ElementId::Name(SharedString::from(format!("{id}-head"))))
        .flex()
        .items_center()
        .gap(px(10.0))
        .px(px(16.0))
        .py(px(10.0))
        .cursor_pointer()
        .bg(if open {
            theme.surface_2
        } else {
            gpui::transparent_black()
        })
        .hover(|s| s.bg(theme.surface_2))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(TEXT_TINY))
                        .text_color(theme.text_muted)
                        .child(caption.to_string()),
                )
                .child(
                    div()
                        .text_size(px(TEXT_BODY))
                        .text_color(theme.text)
                        .truncate()
                        .child(value.into()),
                ),
        )
        .child(icon(
            if open { "chevron-up" } else { "chevron-down" },
            14.0,
            theme.text_faint,
        ))
        .on_click(cx.listener(move |this, _, window, cx| {
            if this.state.read(cx).agent_pick == Some(which) {
                this.close_agent_pick(Some(window), cx);
            } else {
                this.open_agent_pick(which, window, cx);
            }
        }));
    // The hover target is the row plus its dropdown, so moving into the list
    // keeps it open. No click listener here: with one, GPUI drops the hover
    // while a button is pressed inside and the list would fold mid-click.
    div()
        .id(id)
        .flex()
        .flex_col()
        .border_b_1()
        .border_color(theme.border)
        // Clicks inside stay inside: the window's "click elsewhere" handlers
        // (popovers, the profile pop-up) must not see them.
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
            if *hovered {
                this.open_agent_pick(which, window, cx);
            } else {
                this.agent_pick_hover_out(which, window, cx);
            }
        }))
        .child(header)
        .children(panel)
}

impl Shell {
    /// Drop `which` down (folding any other). Model focuses its search field,
    /// Voice asks the relay for the list.
    pub(crate) fn open_agent_pick(
        &mut self,
        which: AgentPick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.read(cx).agent_pick == Some(which) {
            return;
        }
        self.state.update(cx, |s, cx| {
            s.stop_voice_sample(cx);
            s.set_agent_pick(Some(which), cx);
        });
        match which {
            AgentPick::Model => {
                self.model_search_input.update(cx, |i, cx| i.clear(cx));
                let handle = self.model_search_input.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            AgentPick::Voice => self.state.update(cx, |s, _| s.fetch_voices()),
            AgentPick::Reasoning => {}
        }
        cx.notify();
    }

    /// Fold the open row; `true` when one was open. With a window, keyboard
    /// focus leaves the search field for the composer.
    pub(crate) fn close_agent_pick(
        &mut self,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.state.read(cx).agent_pick.is_none() {
            return false;
        }
        self.state.update(cx, |s, cx| {
            s.stop_voice_sample(cx);
            s.set_agent_pick(None, cx);
        });
        if let Some(window) = window {
            let search = self.model_search_input.read(cx).focus_handle(cx);
            if search.is_focused(window) {
                Composer::focus(&self.composer, window, cx);
            }
        }
        cx.notify();
        true
    }

    /// The pointer left `which` (row + list). Folds it — unless a model
    /// filter is being typed, which Esc, a pick or a click elsewhere ends.
    pub(crate) fn agent_pick_hover_out(
        &mut self,
        which: AgentPick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.read(cx).agent_pick != Some(which) {
            return;
        }
        if which == AgentPick::Model {
            let search = self.model_search_input.read(cx);
            if !search.text().trim().is_empty() && search.focus_handle(cx).is_focused(window) {
                return;
            }
        }
        self.close_agent_pick(Some(window), cx);
    }

    /// Enter in the model search field: the first match, if any.
    pub(crate) fn pick_first_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pid) = self.agent_pick_project(cx) else {
            return;
        };
        let query = self.model_search_input.read(cx).text().to_string();
        let first = {
            let state = self.state.read(cx);
            let current = state.project(&pid).map(|p| p.model.clone());
            model_names(&state.models, current.as_deref(), &query)
                .into_iter()
                .next()
        };
        if let Some(name) = first {
            self.state
                .update(cx, |s, cx| s.set_project_model(pid, name, cx));
            self.close_agent_pick(Some(window), cx);
        }
    }

    /// The agent whose page is open, while a settings row is dropped down.
    fn agent_pick_project(&self, cx: &Context<Self>) -> Option<Id> {
        let state = self.state.read(cx);
        state.agent_pick?;
        match &state.profile {
            Some(crate::app::ProfilePage::Agent { project_id }) => Some(project_id.clone()),
            _ => None,
        }
    }
}

/// The pickable models: the relay's list (or the fixture's), the current one
/// always present, narrowed by `query` (case-insensitive substring).
fn model_names(models: &[String], current: Option<&str>, query: &str) -> Vec<String> {
    let mut names: Vec<String> = if models.is_empty() {
        MODELS.iter().map(|m| m.to_string()).collect()
    } else {
        models.to_vec()
    };
    if let Some(current) = current.filter(|c| !c.is_empty()) {
        if !names.iter().any(|n| n == current) {
            names.insert(0, current.to_string());
        }
    }
    let q = query.trim().to_lowercase();
    if !q.is_empty() {
        names.retain(|n| n.to_lowercase().contains(&q));
    }
    names
}

fn list_frame(id: &'static str, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .mx(px(10.0))
        .mb(px(10.0))
        .rounded(px(10.0))
        .bg(theme.card())
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
}

fn item(
    id: ElementId,
    label: impl Into<SharedString>,
    selected: bool,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .min_h(px(36.0))
        .px(px(12.0))
        .text_size(px(TEXT_SMALL))
        .text_color(theme.text)
        .when(selected, |el| el.font_weight(FontWeight::MEDIUM))
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_2))
        .child(div().flex_1().min_w_0().truncate().child(label.into()))
        .when(selected, |el| el.child(icon("check", 14.0, theme.accent)))
}

/// The Model dropdown: a search field, then the matching models.
pub fn model_panel(shell: &Shell, project: &Project, cx: &mut Context<Shell>) -> AnyElement {
    let theme = Theme::of(cx).clone();
    let query = shell.model_search_input.read(cx).text().to_string();
    let names = model_names(
        &shell.state.read(cx).models,
        Some(project.model.as_str()),
        &query,
    );
    let pid = project.id.clone();
    let search = div()
        .flex()
        .items_center()
        .gap(px(7.0))
        .h(px(34.0))
        .px(px(10.0))
        .border_b_1()
        .border_color(theme.border)
        .cursor_text()
        .child(icon("search", 13.0, theme.text_muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(shell.model_search_input.clone()),
        );
    let mut list = div()
        .id("agent-model-list")
        .flex()
        .flex_col()
        .max_h(px(LIST_MAX_HEIGHT))
        .overflow_y_scroll();
    if names.is_empty() {
        list = list.child(
            div()
                .px(px(12.0))
                .py(px(10.0))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(format!("No model matches “{}”.", query.trim())),
        );
    }
    for (i, name) in names.into_iter().enumerate() {
        let selected = name == project.model;
        let p = pid.clone();
        let pick = name.clone();
        list = list.child(
            item(
                ElementId::NamedInteger("agent-model-item".into(), i as u64),
                name,
                selected,
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let (p, pick) = (p.clone(), pick.clone());
                this.state
                    .update(cx, |s, cx| s.set_project_model(p, pick, cx));
            })),
        );
    }
    list_frame("agent-model-panel", &theme)
        .child(search)
        .child(list)
        .into_any_element()
}

/// The Reasoning dropdown: Low / Medium / High.
pub fn reasoning_panel(project: &Project, cx: &mut Context<Shell>) -> AnyElement {
    let theme = Theme::of(cx).clone();
    let pid = project.id.clone();
    let mut list = list_frame("agent-reasoning-panel", &theme);
    for (i, level) in Effort::ALL.iter().enumerate() {
        let level = *level;
        let p = pid.clone();
        list = list.child(
            item(
                ElementId::NamedInteger("agent-reasoning-item".into(), i as u64),
                level.label(),
                level == project.effort,
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let p = p.clone();
                this.state
                    .update(cx, |s, cx| s.set_project_effort(p, level, cx));
            })),
        );
    }
    list.into_any_element()
}

/// The Voice dropdown: the voice sheet's list (Default first, play buttons,
/// checkmark), inline.
pub fn voice_panel(shell: &Shell, project: &Project, cx: &mut Context<Shell>) -> AnyElement {
    let theme = Theme::of(cx).clone();
    div()
        .mx(px(10.0))
        .mb(px(10.0))
        .child(voicepicker::list(
            shell,
            project,
            "agent-voice",
            LIST_MAX_HEIGHT,
            cx,
        ))
        .child(
            div()
                .pt(px(6.0))
                .px(px(4.0))
                .text_size(px(TEXT_TINY))
                .text_color(theme.text_muted)
                .child("Press play to hear a sample; a click saves."),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_filter_and_keep_the_current_one() {
        let relay: Vec<String> = ["anthropic/claude-sonnet-5.5", "openai/gpt-6.1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // No query: the relay's list, the current model first when unknown.
        let all = model_names(&relay, Some("local/llama"), "");
        assert_eq!(all[0], "local/llama");
        assert_eq!(all.len(), 3);
        // The query narrows case-insensitively on a substring.
        let some = model_names(&relay, Some("openai/gpt-6.1"), "SONNET");
        assert_eq!(some, vec!["anthropic/claude-sonnet-5.5".to_string()]);
        // Empty relay list: the fixture's models; an empty current adds nothing.
        let fallback = model_names(&[], Some(""), "");
        assert_eq!(fallback.len(), MODELS.len());
    }

    #[test]
    fn rig_hook_parses_rows() {
        assert_eq!(AgentPick::parse("model"), Some(AgentPick::Model));
        assert_eq!(AgentPick::parse("effort"), Some(AgentPick::Reasoning));
        assert_eq!(AgentPick::parse("voice"), Some(AgentPick::Voice));
        assert_eq!(AgentPick::parse("colour"), None);
    }
}
