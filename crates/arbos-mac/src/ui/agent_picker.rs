//! One agent picker for every Secrets form ([feedback secrets-agent-picker-mac-2026-10-26],
//! Jacob 2026-10-26): the agents already granted as removable chips (picture · name · ×)
//! and one "Add agent" button that drops a searchable list under the row — type to filter
//! by name or @handle, ↑ / ↓ move the highlight, ↩ adds the highlighted agent as a chip, esc
//! or a click elsewhere closes. Agents that already have access are not listed. Before this
//! every agent was a toggle chip, so a user with 16 agents saw a wall of chips (his shot).
//!
//! Used by the New secret form, Connect 1Password, "Give another agent access…" on a
//! secret's page and the same on a vault's page — the four places that pick agents in
//! Secrets share this one component (`SecretsPanel::agent_picker`); the iPhone has the same
//! shape (`AgentPickerField` + `AddAgentSheet`).

use std::rc::Rc;

use gpui::{
    anchored, deferred, div, prelude::*, px, Anchor, AnyElement, Context, Div, ElementId, Entity,
    Focusable as _, FontWeight, SharedString, Window,
};

use crate::app::AppState;
use crate::model::{Id, Project};
use crate::ui::avatars;
use crate::ui::composer::ComposerInput;
use crate::ui::icons::icon;
use crate::ui::people::agent_picker_rows;
use crate::ui::secrets::SecretsPanel;
use crate::ui::sidebar::{DISC_HOVER_LIFT, DISC_PRESSED_LIFT};
use crate::ui::theme::{Theme, TEXT_SMALL, TEXT_TINY};

/// A granted agent's chip and the Add button: 30 tall (the member-chip family).
pub const CHIP_H: f32 = 30.0;
/// The picture on a chip and in a list row.
pub const PICTURE: f32 = 20.0;
/// The chips row wraps with this gap.
pub const CHIP_GAP: f32 = 6.0;
/// The dropdown: 6 under the chips row, the content width, the hoverpick list look.
pub const DROPDOWN_GAP: f32 = 6.0;
pub const DROPDOWN_WIDTH: f32 = 308.0;
pub const DROPDOWN_RADIUS: f32 = 10.0;
pub const SEARCH_ROW_H: f32 = 34.0;
pub const ROW_H: f32 = 36.0;
pub const KEYS_ROW_H: f32 = 22.0;
/// Rows shown before the list scrolls.
pub const VISIBLE_ROWS: usize = 6;

/// Debug selectors the tests look for.
pub const ADD_SELECTOR: &str = "agent-picker-add";
pub const DROPDOWN_SELECTOR: &str = "agent-picker-dropdown";
pub const EMPTY_SELECTOR: &str = "agent-picker-empty";

/// The agents the dropdown offers for `query`: my own agents (never a guest's, never one in
/// `exclude` — the ones with access already — and never one already picked), through the
/// shared matcher every picker uses (`people::agent_picker_rows` → `arbos_core::search`:
/// name, @slug and description; exact, then prefix, then word-start; case, diacritics and a
/// leading `@` folded), best first, registry order inside a tie. No filter of its own.
pub fn candidates(
    projects: &[Project],
    picked: &[Id],
    exclude: &[Id],
    query: &str,
) -> Vec<Project> {
    let pool: Vec<Project> = projects
        .iter()
        .filter(|p| !p.is_guest() && !p.is_people())
        .filter(|p| !exclude.contains(&p.id) && !picked.contains(&p.id))
        .cloned()
        .collect();
    agent_picker_rows(pool, query)
}

/// The picker's state, one per `SecretsPanel` (the forms open one at a time).
pub struct AgentPicker {
    /// The agents granted so far (the chips), in the order they were added.
    pub picked: Vec<Id>,
    /// The dropdown is up.
    pub open: bool,
    /// The keyboard-highlighted row of the current candidates.
    pub highlight: usize,
    /// The query the highlight belongs to (a change resets it to the top).
    last_query: String,
    /// The dropdown's search field.
    pub search: Entity<ComposerInput>,
}

impl AgentPicker {
    /// Wire the search field to the owning panel: ↩ adds the highlighted agent, ↑ / ↓ move
    /// the highlight. Both run deferred (the input's rule), through `weak`.
    pub fn new(cx: &mut Context<SecretsPanel>) -> Self {
        let search = cx.new(ComposerInput::new);
        let weak = cx.weak_entity();
        search.update(cx, |input, _| {
            input.set_placeholder("Search agents");
            input.set_font_size(TEXT_SMALL);
            let w = weak.clone();
            input.on_submit = Some(Rc::new(move |_window, cx| {
                let _ = w.update(cx, |this, cx| this.agent_picker_accept(cx));
            }));
            let w = weak.clone();
            input.on_arrow = Some(Rc::new(move |delta, cx| {
                let _ = w.update(cx, |this, cx| this.agent_picker_move(delta, cx));
            }));
        });
        cx.observe(&search, |this, input, cx| {
            // A new query starts at the top of its list; a caret move alone keeps the highlight.
            let q = input.read(cx).text().to_string();
            if q != this.agent_picker.last_query {
                this.agent_picker.last_query = q;
                this.agent_picker.highlight = 0;
            }
            cx.notify();
        })
        .detach();
        Self {
            picked: Vec::new(),
            open: false,
            highlight: 0,
            last_query: String::new(),
            search,
        }
    }

    /// Everything back to empty (the form was left or submitted).
    pub fn reset(&mut self, cx: &mut Context<SecretsPanel>) {
        self.picked.clear();
        self.close(cx);
    }

    /// Fold the dropdown; the chips stay. `true` when it was open.
    pub fn close(&mut self, cx: &mut Context<SecretsPanel>) -> bool {
        let was = self.open;
        self.open = false;
        self.highlight = 0;
        self.search.update(cx, |i, cx| i.clear(cx));
        was
    }

    /// Drop the list down with the search field focused.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<SecretsPanel>) {
        self.open = true;
        self.highlight = 0;
        let handle = self.search.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// Grant `id`: a chip; the dropdown closes (one agent per visit, as the iPhone).
    pub fn add(&mut self, id: Id, cx: &mut Context<SecretsPanel>) {
        if !self.picked.contains(&id) {
            self.picked.push(id);
        }
        self.close(cx);
    }

    /// The chip's ×.
    pub fn remove(&mut self, id: &Id) {
        self.picked.retain(|p| p != id);
    }

    pub fn query(&self, cx: &gpui::App) -> String {
        self.search.read(cx).text().to_string()
    }
}

/// The chips row (granted agents + Add agent) and, while open, the dropdown under it.
/// `exclude` = the agents that already have access on this page (not offered again).
pub fn render(
    panel: &SecretsPanel,
    state: &Entity<AppState>,
    prefix: &'static str,
    exclude: &[Id],
    theme: &Theme,
    cx: &mut Context<SecretsPanel>,
) -> Div {
    let picker = &panel.agent_picker;
    let (projects, images) = {
        let st = state.read(cx);
        (st.projects.clone(), avatars::project_images(st))
    };
    let query = picker.query(cx);
    let candidates = candidates(&projects, &picker.picked, exclude, &query);
    let highlight = picker.highlight.min(candidates.len().saturating_sub(1));

    let mut row = div().flex().flex_wrap().items_center().gap(px(CHIP_GAP));
    for pid in &picker.picked {
        let Some(p) = projects.iter().find(|p| &p.id == pid) else {
            continue;
        };
        let id = p.id.clone();
        let chip_id = format!("{prefix}-agent-chip-{}", p.id);
        let sel = format!("agent-picker-chip-{}", p.id);
        let x_id = format!("{prefix}-agent-chip-x-{}", p.id);
        let x_sel = format!("agent-picker-chip-x-{}", p.id);
        row = row.child(
            div()
                .id(ElementId::Name(SharedString::from(chip_id)))
                .debug_selector(move || sel.clone())
                .h(px(CHIP_H))
                .pl(px(4.0))
                .pr(px(6.0))
                .rounded_full()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(6.0))
                .bg(theme.field)
                .border_1()
                .border_color(theme.field_border)
                .text_size(px(TEXT_SMALL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(avatars::project_with(
                    images.get(&p.id).cloned(),
                    p,
                    PICTURE,
                    theme,
                ))
                .child(p.name.clone())
                .child(
                    div()
                        .id(ElementId::Name(SharedString::from(x_id)))
                        .debug_selector(move || x_sel.clone())
                        .size(px(18.0))
                        .ml(px(-1.0))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_color(theme.text_muted)
                        .hover(|s| s.text_color(theme.text).bg(theme.surface_2))
                        .child(icon("xmark", 9.0, theme.text_muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.agent_picker.remove(&id);
                            cx.notify();
                        })),
                ),
        );
    }
    let open = picker.open;
    let hover = Theme::lighter(theme.surface_2, DISC_HOVER_LIFT);
    let pressed = Theme::lighter(theme.surface_2, DISC_PRESSED_LIFT);
    row = row.child(
        div()
            .id(ElementId::Name(SharedString::from(format!(
                "{prefix}-agent-add"
            ))))
            .debug_selector(|| ADD_SELECTOR.into())
            .h(px(CHIP_H))
            .pl(px(9.0))
            .pr(px(11.0))
            .rounded_full()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(6.0))
            .bg(if open { pressed } else { theme.surface_2 })
            .text_size(px(TEXT_SMALL))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.text)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .active(move |s| s.bg(pressed))
            .child(icon("plus", 13.0, theme.text))
            .child("Add agent")
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.agent_picker.open {
                    this.agent_picker.close(cx);
                } else {
                    this.agent_picker.open(window, cx);
                }
                cx.notify();
            })),
    );

    let mut block = div().relative().flex().flex_col().gap(px(5.0)).child(row);
    if picker.picked.is_empty() && !open {
        block = block.child(
            div()
                .debug_selector(|| EMPTY_SELECTOR.into())
                .text_size(px(TEXT_TINY))
                .text_color(theme.text_muted)
                .child("No agent can use it yet."),
        );
    }
    if open {
        block = block.child(
            div()
                .absolute()
                .left(px(0.0))
                .bottom(px(-DROPDOWN_GAP))
                .child(
                    deferred(
                        anchored()
                            .anchor(Anchor::TopLeft)
                            .position_mode(gpui::AnchoredPositionMode::Local)
                            .snap_to_window_with_margin(gpui::Edges::all(px(8.0)))
                            .child(dropdown(
                                panel,
                                prefix,
                                &candidates,
                                &images,
                                highlight,
                                &query,
                                theme,
                                cx,
                            )),
                    )
                    .with_priority(2),
                ),
        );
    }
    block
}

#[allow(clippy::too_many_arguments)]
fn dropdown(
    panel: &SecretsPanel,
    prefix: &'static str,
    candidates: &[Project],
    images: &std::collections::HashMap<String, std::sync::Arc<gpui::RenderImage>>,
    highlight: usize,
    query: &str,
    theme: &Theme,
    cx: &mut Context<SecretsPanel>,
) -> AnyElement {
    let search = div()
        .flex()
        .items_center()
        .gap(px(7.0))
        .h(px(SEARCH_ROW_H))
        .px(px(10.0))
        .border_b_1()
        .border_color(theme.border)
        .cursor_text()
        .child(icon("search", 13.0, theme.text_muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(panel.agent_picker.search.clone()),
        );
    let mut list = div()
        .id(ElementId::Name(SharedString::from(format!(
            "{prefix}-agent-list"
        ))))
        .flex()
        .flex_col()
        .max_h(px(ROW_H * VISIBLE_ROWS as f32))
        .overflow_y_scroll();
    if candidates.is_empty() {
        list = list.child(
            div()
                .px(px(12.0))
                .py(px(10.0))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(if query.trim().is_empty() {
                    "Every agent has access.".to_string()
                } else {
                    format!("No agent matches “{}”.", query.trim())
                }),
        );
    }
    for (i, p) in candidates.iter().enumerate() {
        let selected = i == highlight;
        let id = p.id.clone();
        let sel = format!("agent-picker-row-{i}");
        list = list.child(
            div()
                .id(ElementId::NamedInteger(
                    SharedString::from(format!("{prefix}-agent-row")),
                    i as u64,
                ))
                .debug_selector(move || sel.clone())
                .flex()
                .items_center()
                .gap(px(8.0))
                .min_h(px(ROW_H))
                .px(px(12.0))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text)
                .cursor_pointer()
                .when(selected, |el| el.bg(theme.surface_2))
                .hover(|s| s.bg(theme.surface_2))
                .child(avatars::project_with(
                    images.get(&p.id).cloned(),
                    p,
                    PICTURE,
                    theme,
                ))
                .child(div().child(p.name.clone()))
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(theme.text_muted)
                        .child(format!("@{}", Project::mention_slug(&p.name))),
                )
                .when(selected, |el| {
                    el.child(
                        div()
                            .ml_auto()
                            .text_size(px(11.0))
                            .text_color(theme.text_faint)
                            .child("↩"),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.agent_picker.add(id.clone(), cx);
                    cx.notify();
                })),
        );
    }
    let keys = div()
        .h(px(KEYS_ROW_H))
        .flex()
        .items_center()
        .px(px(12.0))
        .border_t_1()
        .border_color(theme.border)
        .text_size(px(11.0))
        .text_color(theme.text_faint)
        .child("↑↓ move · ↩ add · esc close");
    div()
        .id(ElementId::Name(SharedString::from(format!(
            "{prefix}-agent-dropdown"
        ))))
        .debug_selector(|| DROPDOWN_SELECTOR.into())
        .w(px(DROPDOWN_WIDTH))
        .flex()
        .flex_col()
        .rounded(px(DROPDOWN_RADIUS))
        .bg(theme.card())
        .border_1()
        .border_color(theme.border)
        .shadow_lg()
        .overflow_hidden()
        // Clicks inside stay inside: the shell's "click elsewhere" must not fold the list.
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(search)
        .child(list)
        .child(keys)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SpaceKind;

    fn agent(id: &str, name: &str) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            ..Project::default()
        }
    }

    fn fixture() -> Vec<Project> {
        let mut v = vec![
            agent("a1", "Affine"),
            agent("a2", "Arbos"),
            agent("a3", "ArbosTest"),
            agent("a4", "Doug agent"),
            agent("a5", "Hezbola"),
            agent("a6", "Arbonites"),
            agent("a7", "Zoë"),
        ];
        v.push(Project {
            id: "g1".into(),
            name: "Launch week".into(),
            kind: SpaceKind::Group,
            ..Project::default()
        });
        v.push(Project {
            id: "s1".into(),
            name: "Shared in".into(),
            shared: true,
            ..Project::default()
        });
        v
    }

    fn names(v: &[Project]) -> Vec<&str> {
        v.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn candidates_hide_granted_picked_guests_and_people_and_follow_the_query() {
        let projects = fixture();
        assert_eq!(
            names(&candidates(&projects, &[], &[], "")),
            [
                "Affine",
                "Arbos",
                "ArbosTest",
                "Doug agent",
                "Hezbola",
                "Arbonites",
                "Zoë"
            ],
            "my agents only — no group, no shared-in agent"
        );
        assert_eq!(
            names(&candidates(
                &projects,
                &["a1".into(), "a4".into()],
                &["a2".into()],
                "ar"
            )),
            ["ArbosTest", "Arbonites"],
            "“ar”: Arbos is excluded (has access), Affine / Doug are picked"
        );
        assert!(candidates(&projects, &[], &[], "nothing here").is_empty());
    }

    #[test]
    fn the_filter_is_the_shared_matcher() {
        let projects = fixture();
        assert_eq!(
            names(&candidates(&projects, &[], &[], "@doug-ag")),
            ["Doug agent"],
            "the @slug"
        );
        assert_eq!(
            names(&candidates(&projects, &[], &[], "agent")),
            ["Doug agent"],
            "a word start"
        );
        assert_eq!(
            names(&candidates(&projects, &[], &[], "zoe")),
            ["Zoë"],
            "diacritics folded"
        );
        assert_eq!(
            names(&candidates(&projects, &[], &[], "arbos")),
            ["Arbos", "ArbosTest"],
            "exact before prefix"
        );
    }
}
