//! Settings → Skills (F5): the skills library as a two-pane card. Left, the
//! list with a shared badge and the origin project; right, the editor for
//! name / description / tags / when to use / body with Share, Delete, Save.
//! Edits go through `AppState::save_skill` so the fixture and the relay
//! source behave the same.

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, App, Context, Div, Entity, FocusHandle, Focusable, FontWeight,
    MouseButton, Render, Stateful, Window,
};

use crate::app::AppState;
use crate::model::{Id, Part, Skill};
use crate::ui::composer::ComposerInput;
use crate::ui::frost::{frosted, MENU_BLUR};
use crate::ui::theme::{Theme, PANEL_RADIUS, RAIL_WIDTH};

pub const CARD_WIDTH: f32 = 760.0;
const CARD_HEIGHT: f32 = 640.0;
const LIST_WIDTH: f32 = 260.0;

pub struct SkillsPanel {
    state: Entity<AppState>,
    pub open: bool,
    /// Shown as a page inside the Settings sheet (#1086) rather than as the
    /// bottom-left popover; the sheet's header carries the back button.
    pub(crate) embedded: bool,
    /// Slug of the skill in the editor; `None` while drafting a new one.
    selected: Option<String>,
    drafting: bool,
    draft_shared: bool,
    draft_origin: Option<Id>,
    name: Entity<ComposerInput>,
    description: Entity<ComposerInput>,
    tags: Entity<ComposerInput>,
    when_to_use: Entity<ComposerInput>,
    body: Entity<ComposerInput>,
    focus_handle: FocusHandle,
}

impl SkillsPanel {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |this, _, cx| {
            this.reconcile_selection(cx);
            cx.notify();
        })
        .detach();
        let field = |cx: &mut Context<Self>, placeholder: &str, multiline: bool| {
            let input = cx.new(ComposerInput::new);
            let weak = cx.weak_entity();
            input.update(cx, |input, _| {
                input.set_placeholder(placeholder.to_string());
                input.enter_inserts_newline = multiline;
                if !multiline {
                    input.on_submit = Some(Rc::new(move |_window, cx| {
                        let _ = weak.update(cx, |this, cx| this.save(cx));
                    }));
                }
            });
            cx.observe(&input, |_, _, cx| cx.notify()).detach();
            input
        };
        Self {
            name: field(cx, "Name", false),
            description: field(cx, "One line: what this skill is for", false),
            tags: field(cx, "Tags, comma separated", false),
            when_to_use: field(cx, "When should the agent reach for it?", false),
            body: field(cx, "Steps, pitfalls, examples (Markdown)", true),
            state,
            open: false,
            embedded: false,
            selected: None,
            drafting: false,
            draft_shared: false,
            draft_origin: None,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.state.update(cx, |s, cx| s.refresh_skills(cx));
        if self.selected.is_none() && !self.drafting {
            let first = self.state.read(cx).skills.first().cloned();
            match first {
                Some(skill) => self.load(&skill, cx),
                None => self.start_new(window, cx),
            }
        }
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.embedded = false;
        cx.notify();
    }

    /// Open as the bottom-left popover (not inside the Settings sheet).
    pub fn is_floating(&self) -> bool {
        self.open && !self.embedded
    }

    /// True when one of the editor fields has keyboard focus.
    pub fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.open
            && [
                &self.name,
                &self.description,
                &self.tags,
                &self.when_to_use,
                &self.body,
            ]
            .iter()
            .any(|input| input.read(cx).focus_handle(cx).is_focused(window))
    }

    fn reconcile_selection(&mut self, cx: &mut Context<Self>) {
        // A just-created skill came back from the source: adopt it.
        if self.drafting {
            let slug = arbos_core::skill::slugify(self.name.read(cx).text());
            let fresh = self
                .state
                .read(cx)
                .skills
                .iter()
                .find(|s| s.slug == slug && !s.id.is_empty())
                .cloned();
            if let Some(skill) = fresh {
                self.drafting = false;
                self.selected = Some(skill.slug);
            }
            return;
        }
        if let Some(slug) = &self.selected {
            if !self.state.read(cx).skills.iter().any(|s| &s.slug == slug) {
                self.selected = None;
                let first = self.state.read(cx).skills.first().cloned();
                if let Some(skill) = first {
                    self.load(&skill, cx);
                }
            }
        }
    }

    fn load(&mut self, skill: &Skill, cx: &mut Context<Self>) {
        self.selected = Some(skill.slug.clone());
        self.drafting = false;
        self.draft_shared = skill.shared;
        self.draft_origin = skill.origin_project_id.clone();
        let name = skill.name.clone();
        let description = skill.description.clone();
        let tags = skill.tags.join(", ");
        let when = skill.when_to_use.clone();
        let body = skill.body.clone();
        self.name
            .update(cx, |i, cx| i.set_text_from_start(name, cx));
        self.description
            .update(cx, |i, cx| i.set_text_from_start(description, cx));
        self.tags
            .update(cx, |i, cx| i.set_text_from_start(tags, cx));
        self.when_to_use
            .update(cx, |i, cx| i.set_text_from_start(when, cx));
        self.body
            .update(cx, |i, cx| i.set_text_from_start(body, cx));
        cx.notify();
    }

    fn start_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = None;
        self.drafting = true;
        self.draft_shared = false;
        self.draft_origin = self.state.read(cx).selected_project.clone();
        for input in [
            &self.name,
            &self.description,
            &self.tags,
            &self.when_to_use,
            &self.body,
        ] {
            input.update(cx, |i, cx| i.clear(cx));
        }
        let handle = self.name.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    fn current(&self, cx: &App) -> Option<Skill> {
        let slug = self.selected.as_deref()?;
        self.state
            .read(cx)
            .skills
            .iter()
            .find(|s| s.slug == slug)
            .cloned()
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).text().trim().to_string();
        if name.is_empty() {
            self.state.update(cx, |s, _| {
                s.push_failure(
                    crate::app::refused("save the skill", "A skill needs a name.").error,
                    None,
                )
            });
            cx.notify();
            return;
        }
        let existing = self.current(cx);
        let tags: Vec<String> = self
            .tags
            .read(cx)
            .text()
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        let skill = Skill {
            id: existing.as_ref().map(|s| s.id.clone()).unwrap_or_default(),
            slug: existing
                .as_ref()
                .map(|s| s.slug.clone())
                .unwrap_or_else(|| arbos_core::skill::slugify(&name)),
            name,
            description: self.description.read(cx).text().trim().to_string(),
            tags,
            when_to_use: self.when_to_use.read(cx).text().trim().to_string(),
            body: self.body.read(cx).text().to_string(),
            origin_project_id: self.draft_origin.clone(),
            shared: self.draft_shared,
            version: existing.as_ref().map(|s| s.version).unwrap_or(1),
            created_at: existing.as_ref().map(|s| s.created_at).unwrap_or(0),
            updated_at: 0,
        };
        if existing.is_none() {
            self.selected = Some(skill.slug.clone());
            self.drafting = false;
        }
        self.state.update(cx, |s, cx| s.save_skill(skill, cx));
        cx.notify();
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        if let Some(skill) = self.current(cx) {
            self.selected = None;
            self.state.update(cx, |s, cx| s.delete_skill(skill.id, cx));
            let first = self.state.read(cx).skills.first().cloned();
            if let Some(next) = first {
                self.load(&next, cx);
            }
        }
        cx.notify();
    }

    fn project_name(&self, id: Option<&str>, cx: &App) -> String {
        match id {
            Some(id) => self
                .state
                .read(cx)
                .project(id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Another project".into()),
            None => "Library".into(),
        }
    }

    fn render_list(&self, theme: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let skills = self.state.read(cx).skills.clone();
        let mut list = div()
            .id("skills-list")
            .w(px(LIST_WIDTH))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .pr(px(8.0));
        if skills.is_empty() {
            list = list.child(
                div()
                    .p(px(10.0))
                    .text_size(px(12.0))
                    .text_color(theme.text_faint)
                    .child("No skills yet. New starts one."),
            );
        }
        for (ix, skill) in skills.into_iter().enumerate() {
            let selected = self.selected.as_deref() == Some(skill.slug.as_str());
            let origin = self.project_name(skill.origin_project_id.as_deref(), cx);
            let slug = skill.slug.clone();
            let mut meta = div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(px(10.5))
                .text_color(theme.text_faint)
                .child(origin);
            if skill.shared {
                meta = meta.child(
                    div()
                        .px(px(6.0))
                        .py(px(1.0))
                        .rounded_full()
                        .bg(theme.accent_soft)
                        .text_color(theme.accent)
                        .font_weight(FontWeight::MEDIUM)
                        .child("Shared"),
                );
            }
            list = list.child(
                div()
                    .id(("skill-row", ix))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .bg(if selected {
                        theme.surface_2
                    } else {
                        gpui::transparent_black()
                    })
                    .hover(|s| s.bg(theme.surface_2))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child(skill.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.text_muted)
                            .overflow_hidden()
                            .child(skill.description.clone()),
                    )
                    .child(meta)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let skill = this
                            .state
                            .read(cx)
                            .skills
                            .iter()
                            .find(|s| s.slug == slug)
                            .cloned();
                        if let Some(skill) = skill {
                            this.load(&skill, cx);
                        }
                    })),
            );
        }
        list
    }

    fn render_editor(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let field = |label: &'static str, input: Entity<ComposerInput>, grow: bool| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .when(grow, |el| el.flex_1().min_h_0())
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.text_faint)
                        .child(label),
                )
                .child(
                    div()
                        .flex()
                        .items_start()
                        .when(grow, |el| el.flex_1().min_h(px(120.0)).overflow_hidden())
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(8.0))
                        .bg(theme.surface_2)
                        .border_1()
                        .border_color(theme.border)
                        .child(input),
                )
        };
        // The input is only as tall as its text; a click in the empty part of
        // the body box should still start typing there.
        let body_focus = self.body.read(cx).focus_handle(cx);
        let body_field = field("BODY · MARKDOWN", self.body.clone(), true).on_mouse_down(
            MouseButton::Left,
            move |_, window, cx| {
                window.focus(&body_focus, cx);
                cx.stop_propagation();
            },
        );
        let existing = self.current(cx);
        let origin = self.project_name(self.draft_origin.as_deref(), cx);
        let heading = match &existing {
            Some(s) => format!("{} · v{} · {}", s.slug, s.version, origin),
            None => format!("New skill · {origin}"),
        };
        let button = |id: &'static str, label: &'static str, primary: bool, danger: bool| {
            div()
                .id(id)
                .px(px(10.0))
                .py(px(5.0))
                .rounded(px(6.0))
                .text_size(px(12.0))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.text).text_color(theme.bg))
                .when(!primary && !danger, |el| {
                    el.text_color(theme.text_muted)
                        .hover(|s| s.bg(theme.surface_2))
                })
                .when(danger, |el| {
                    // error-ui: exception destructive-action (a Delete label, not a message)
                    el.text_color(theme.danger)
                        .hover(|s| s.bg(theme.danger_soft))
                })
                .child(label)
        };
        let share = div()
            .id("skill-share")
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .child(
                div()
                    .w(px(30.0))
                    .h(px(18.0))
                    .rounded_full()
                    .p(px(2.0))
                    .bg(if self.draft_shared {
                        theme.accent
                    } else {
                        theme.border_strong
                    })
                    .flex()
                    .when(self.draft_shared, |el| el.justify_end())
                    .child(div().size(px(14.0)).rounded_full().bg(theme.bg)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.text_muted)
                    .child(if self.draft_shared {
                        "Shared with every project"
                    } else {
                        "Only the origin project sees it"
                    }),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.draft_shared = !this.draft_shared;
                cx.notify();
            }));
        let mut footer = div()
            .flex()
            .items_center()
            .justify_between()
            .pt(px(4.0))
            .child(share);
        let mut actions = div().flex().items_center().gap(px(6.0));
        if existing.is_some() {
            actions = actions.child(
                button("skill-delete", "Delete", false, true)
                    .on_click(cx.listener(|this, _, _, cx| this.delete(cx))),
            );
        }
        actions = actions
            .child(
                button("skill-close", "Close", false, false)
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            )
            .child(
                button("skill-save", "Save", true, false)
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            );
        footer = footer.child(actions);

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.text_faint)
                    .child(heading),
            )
            .child(field("NAME", self.name.clone(), false))
            .child(field("DESCRIPTION", self.description.clone(), false))
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .child(field("TAGS", self.tags.clone(), false)),
                    )
                    .child(div().flex_1().child(field(
                        "WHEN TO USE",
                        self.when_to_use.clone(),
                        false,
                    ))),
            )
            .child(body_field)
            .child(footer)
    }
}

impl Render for SkillsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let count = self.state.read(cx).skills.len();
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(8.0))
                    .when(!self.embedded, |el| {
                        el.child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .child("Skills"),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.text_faint)
                            .child(format!(
                                "{count} in the library · the agent pulls the three most relevant into each turn"
                            )),
                    ),
            )
            .child(
                div()
                    .id("skill-new")
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .text_size(px(12.0))
                    .text_color(theme.text)
                    .bg(theme.surface_2)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.border_strong))
                    .child("+ New")
                    .on_click(cx.listener(|this, _, window, cx| this.start_new(window, cx))),
            );
        let card = div()
            .id("skills-card")
            .occlude()
            .h(px(CARD_HEIGHT))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(14.0))
            .child(header)
            .child(div().h(px(1.0)).bg(theme.border))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(14.0))
                    .child(self.render_list(&theme, cx))
                    .child(div().w(px(1.0)).h_full().bg(theme.border))
                    .child(self.render_editor(&theme, cx)),
            );
        if self.embedded {
            return card.w_full().into_any_element();
        }
        let card = card
            .w(px(CARD_WIDTH))
            .rounded(px(PANEL_RADIUS))
            .bg(theme.card())
            .border_1()
            .border_color(theme.border_strong)
            .shadow_lg();
        div()
            .absolute()
            .left(px(RAIL_WIDTH + 8.0))
            .bottom(px(14.0))
            .child(frosted(PANEL_RADIUS, MENU_BLUR, card))
            .into_any_element()
    }
}

impl Focusable for SkillsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Details drawer rows for the "Relevant skills" hidden part agentd attaches
/// to a turn (`Part::Skills`): the slugs chosen and the injected text.
pub fn details_section(parts: &[Part]) -> Option<(Vec<(String, String)>, String)> {
    parts.iter().find_map(|p| match p {
        Part::Skills { text, slugs, .. } => Some((
            vec![(
                "Selected".into(),
                if slugs.is_empty() {
                    "none matched".into()
                } else {
                    slugs.join(", ")
                },
            )],
            text.clone(),
        )),
        _ => None,
    })
}
