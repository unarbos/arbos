//! Click-to-expand settings rows for the agent's profile page: Model (with a
//! search field), Reasoning and Voice. A click on the row opens its choice
//! list in place; the list folds on Esc, a click elsewhere, a second click on
//! the row, or after a value is picked. Picking a value applies it at once
//! through the same state handlers the composer and the voice sheet use.

use gpui::{
    div, prelude::*, px, AnyElement, Bounds, Context, Div, ElementId, Focusable as _, FontWeight,
    Pixels, SharedString, Stateful, Window,
};

use crate::app::AgentPick;
use crate::model::{Effort, Id, Project};
use crate::sync::fixture::MODELS;
use crate::ui::composer::Composer;
use crate::ui::highlight::{self, Shape};
use crate::ui::profiles::kit::{self, Leading};
use crate::ui::sf_symbols;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, TEXT_SMALL, TEXT_TINY};
use crate::ui::voicepicker;

/// How tall the model / voice lists may grow before they scroll.
const LIST_MAX_HEIGHT: f32 = 260.0;
/// The dropped-down list's corners; its rows' highlight is concentric.
pub(crate) const LIST_RADIUS: f32 = 10.0;

/// One info-card row: caption over the current value, a chevron, and —
/// while `open` — `panel` dropped down under it. A click toggles it open.
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
    // The pane kit's row: label left, the current value right, a chevron that
    // turns up while the list is open.
    let header = kit::row_state(
        ElementId::Name(SharedString::from(format!("{id}-head"))),
        Leading::None,
        caption.to_string(),
        None,
        Some(value.into()),
        open.then(|| sf_symbols::glyph("chevron.up", 13.0, theme.text_faint).into_any_element()),
        !open,
        open,
        theme,
    )
    .cursor_pointer()
    .group(kit::HOT)
    .on_click(cx.listener(move |this, _, window, cx| {
        if this.state.read(cx).agent_pick == Some(which) {
            this.close_agent_pick(Some(window), cx);
        } else {
            this.open_agent_pick(which, window, cx);
        }
    }));
    // Clicks inside stay inside: the window's "click elsewhere" handlers
    // (popovers, the profile pop-up) must not see them. No hover listener —
    // the list opens only on a click, so moving the pointer across rows does
    // not change what is open.
    //
    // The header's hung hairline separates it from the panel while open, and
    // from the next row while closed. When open, hang one more under the
    // whole block so the next row still gets a card-style divider (the pane
    // card no longer interleaves section separators on kit rows).
    div()
        .id(id)
        .relative()
        .flex()
        .flex_col()
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(header)
        .children(panel)
        .when(open, |el| el.child(kit::hairline_below(theme)))
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
        self.agent_pick_reveal = true;
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

    /// Prepaint of the info card holding the rows: the first frame after a
    /// row opened, scroll the sheet body just enough for its list to show
    /// (never pushing the row's own header off the top). Once, so a later
    /// manual scroll is left alone.
    pub(crate) fn reveal_pane_row(
        &mut self,
        row: Option<&Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.agent_pick_reveal {
            return;
        }
        let Some(row) = row else {
            return;
        };
        self.agent_pick_reveal = false;
        let view = self.profile_scroll.bounds();
        let overflow = row.bottom() - view.bottom();
        let room = row.top() - view.top();
        let shift = overflow.min(room);
        if shift > px(0.5) {
            let mut offset = self.profile_scroll.offset();
            offset.y -= shift;
            self.profile_scroll.set_offset(offset);
            // This runs mid-draw, where `refresh` / `notify` are ignored:
            // ask for the redraw once this frame is out.
            let shell = cx.entity();
            window.on_next_frame(move |window, cx| {
                shell.update(cx, |_, cx| cx.notify());
                window.refresh();
            });
        }
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
        .rounded(px(LIST_RADIUS))
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
    let frame = div()
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
        .cursor_pointer();
    highlight::row(frame, false, Shape::Nested(LIST_RADIUS), theme)
        .child(div().flex_1().min_w_0().truncate().child(label.into()))
        .when(selected, |el| {
            el.child(sf_symbols::glyph("checkmark", 14.0, theme.accent))
        })
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
        .child(sf_symbols::glyph("magnifyingglass", 13.0, theme.text_muted))
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
            .on_click(cx.listener(move |this, _, window, cx| {
                let (p, pick) = (p.clone(), pick.clone());
                this.state
                    .update(cx, |s, cx| s.set_project_model(p, pick, cx));
                this.close_agent_pick(Some(window), cx);
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
            .on_click(cx.listener(move |this, _, window, cx| {
                let p = p.clone();
                this.state
                    .update(cx, |s, cx| s.set_project_effort(p, level, cx));
                this.close_agent_pick(Some(window), cx);
            })),
        );
    }
    list.into_any_element()
}

/// The Voice dropdown: the call-voice picker's list (Default first, play
/// buttons, checkmark), inline — the same list as the voice sheet and the
/// iPhone's `VoicePickerView`. A pick saves and folds the row.
pub fn voice_panel(shell: &Shell, project: &Project, cx: &mut Context<Shell>) -> AnyElement {
    let theme = Theme::of(cx).clone();
    // 6 pt under the row's hairline so the first voice is never tucked under it.
    div()
        .mx(px(10.0))
        .mt(px(6.0))
        .mb(px(10.0))
        .child(voicepicker::list(
            shell,
            project,
            "agent-voice",
            LIST_MAX_HEIGHT,
            true,
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
