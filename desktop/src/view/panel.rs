//! The right-hand panel: a live view of the project's `.arbos/`. Agents
//! and their sub-agents as a tree (a click puts that chat in the column),
//! the processes those agents started, the resources they hold, and the
//! project's goals and notes. Nothing else but a settings button at the
//! bottom.

use crate::{
    model::{
        session::ChildState,
        store_view::{Note, Resource},
        surface::{Surface, SurfaceId, SurfaceKind},
        workspace::Workspace,
    },
    view::{
        component::{composer::SessionDrag, menu::Menu, surface as board, transcript},
        root::{self, Arbos, NewSession, TogglePanel},
        settings::Section,
    },
};
use bezel::{
    gpui::{
        AnyElement, App, ClickEvent, Context, Div, Hsla, Render, SharedString, Stateful, Window,
        div, prelude::*, px,
    },
    theme::{TextStyle, Theme, Typeset},
    ui::{icons, popover, tooltip::Tooltip, widgets::Buttons},
};
use std::time::Duration;

/// The panel's width. Cursor's right panel runs 280–320 at a 1728 window.
pub(crate) const PANEL_WIDTH: f32 = 280.;

/// Below this window width the panel is left out; the chat comes first.
pub(crate) const PANEL_MIN_WINDOW: f32 = 900.;

/// A row's height, and the step each level of the tree indents by.
const ROW_HEIGHT: f32 = 26.;
const TREE_STEP: f32 = 14.;

/// Padding inside the panel, and between its sections.
const PAD_X: f32 = 10.;
const SECTION_GAP: f32 = 14.;

/// One agent on the tree, as the panel and a keyboard step walk it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AgentRow {
    pub id: u64,
    pub depth: u8,
}

/// What a row needs of its agent, read out of the model before the row
/// is built.
struct AgentLine {
    id: u64,
    depth: u8,
    title: String,
    state: ChildState,
    /// The first root: the project's main chat.
    main: bool,
    archived: bool,
    /// How long its turn has run, for the spinner.
    since: Duration,
}

/// What rides under the cursor while an agent is being carried.
struct Carried(SharedString);

impl Render for Carried {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        popover::popover_card(&theme)
            .px(px(10.))
            .py(px(4.))
            .text_style(TextStyle::Callout)
            .text_color(theme.text)
            .child(self.0.clone())
    }
}

/// One row under Processes or Resources.
struct SurfaceLine {
    id: SurfaceId,
    title: String,
    glyph: &'static str,
}

/// A standing obligation one of the agents holds: `every 1h · next 15:04`.
struct StandingLine {
    agent: u64,
    goal: String,
    when: String,
}

impl Arbos {
    /// Every open agent in the project in front, roots first by rank and
    /// each one's children under it — the order the panel draws and the
    /// keyboard steps.
    pub(crate) fn agent_rows(&self, cx: &App) -> Vec<AgentRow> {
        let workspace = self.workspace.read(cx);
        let Some(project) = workspace.active_project() else {
            return Vec::new();
        };
        let mut roots: Vec<(i64, u64)> = project
            .roots()
            .filter(|chat| !chat.closed)
            .map(|chat| (chat.rank, chat.id))
            .collect();
        roots.sort();
        let mut rows = Vec::new();
        for (_, id) in roots {
            rows.push(AgentRow { id, depth: 0 });
            push_children(&mut rows, workspace, id, 1);
        }
        rows
    }

    /// The panel, or nothing on a window too narrow to give it room.
    pub(crate) fn panel(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.panel_open || f32::from(window.viewport_size().width) < PANEL_MIN_WINDOW {
            return None;
        }
        let theme = Theme::of(cx).clone();
        let workspace = self.workspace.read(cx);
        let project = workspace.active_project()?;
        let focused = project.focused_agent();
        let focused_surface = project.focus.and_then(|focus| focus.surface);
        let name = Workspace::tab_label(project);
        let (glyph, tint) = (project.identity.glyph(), project.identity.hsla());
        let place = project.place();
        let where_ = place.encode();
        let branch = (!place.is_remote())
            .then(|| self.branch_of(&place.path))
            .flatten();
        let store = project.store_view.clone();
        let remote = project.is_remote();

        let agents: Vec<AgentLine> = self
            .agent_rows(cx)
            .into_iter()
            .enumerate()
            .filter_map(|(n, row)| {
                let chat = project.session(row.id)?;
                Some(AgentLine {
                    id: row.id,
                    depth: row.depth,
                    title: workspace.display_label(row.id),
                    state: chat.child_state(),
                    main: n == 0,
                    archived: chat.closed,
                    // A kernel child has no flight to time; the shared clock
                    // keeps its spinner turning.
                    since: chat.elapsed().unwrap_or_else(transcript::live_phase),
                })
            })
            .collect();
        let working = agents
            .iter()
            .filter(|line| line.state == ChildState::Working)
            .count();

        let mut surfaces: Vec<_> = project.surfaces.iter().collect();
        surfaces.sort_by_key(|surface| std::cmp::Reverse(surface.touched));
        let line_of = |surface: &Surface| SurfaceLine {
            id: surface.id,
            title: board::title(surface),
            glyph: board::glyph(&surface.board_kind),
        };
        let processes: Vec<SurfaceLine> = surfaces
            .iter()
            .filter(|surface| {
                matches!(surface.kind, SurfaceKind::Terminal | SurfaceKind::Process)
            })
            .map(|surface| line_of(surface))
            .collect();
        let resources: Vec<SurfaceLine> = surfaces
            .iter()
            .filter(|surface| matches!(surface.kind, SurfaceKind::Browser | SurfaceKind::Panel))
            .map(|surface| line_of(surface))
            .collect();
        let standing: Vec<StandingLine> = project
            .sessions
            .iter()
            .filter(|chat| !chat.closed)
            .flat_map(|chat| {
                chat.plan_open()
                    .filter(|node| node.standing)
                    .map(move |node| StandingLine {
                        agent: chat.id,
                        goal: node.goal.clone(),
                        when: node.when.clone(),
                    })
            })
            .collect();

        let mut body = div()
            .id("panel-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(PAD_X))
            .pb(px(SECTION_GAP))
            .flex()
            .flex_col()
            .child(self.panel_head(&name, &where_, branch.as_deref(), glyph, tint, &theme))
            .child(section_head(
                "Agents",
                (working > 0).then(|| format!("{working} working")),
                &theme,
            ))
            .children(
                agents
                    .into_iter()
                    .map(|line| self.agent_row(line, focused, &theme, cx)),
            );
        if !processes.is_empty() || !standing.is_empty() {
            body = body
                .child(section_head("Processes", None, &theme))
                .children(
                    processes
                        .into_iter()
                        .map(|line| self.surface_row(line, focused_surface, &theme, cx)),
                )
                .children(
                    standing
                        .into_iter()
                        .map(|line| self.standing_row(line, &theme, cx)),
                );
        }
        if !resources.is_empty() || !store.resources.is_empty() {
            body = body
                .child(section_head("Resources", None, &theme))
                .children(
                    resources
                        .into_iter()
                        .map(|line| self.surface_row(line, focused_surface, &theme, cx)),
                )
                .children(store_rows(&store.resources, &theme));
        }
        body = body
            .child(section_head("Goals", None, &theme))
            .child(self.note_block(store.goals.as_ref(), true, remote, &theme, cx));
        if let Some(notes) = &store.notes {
            body = body
                .child(section_head("Notes", None, &theme))
                .child(self.note_block(Some(notes), false, remote, &theme, cx));
        }

        Some(
            div()
                .id("panel")
                .flex_none()
                .w(px(PANEL_WIDTH))
                .h_full()
                .bg(root::chrome_bg(&theme))
                .border_l_1()
                .border_color(theme.border)
                .flex()
                .flex_col()
                .child(body)
                .child(self.panel_foot(&theme, cx))
                .into_any_element(),
        )
    }

    /// The project's name, where it lives, and the branch checked out there.
    fn panel_head(
        &self,
        name: &str,
        where_: &str,
        branch: Option<&str>,
        glyph: &'static str,
        tint: Hsla,
        theme: &Theme,
    ) -> AnyElement {
        div()
            .flex_none()
            .pt(px(12.))
            .px(px(8.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .text_style(TextStyle::Body)
                    .text_color(theme.text)
                    .child(icons::icon(glyph).size(px(13.)).text_color(tint))
                    .child(div().min_w_0().truncate().child(SharedString::from(name.to_string()))),
            )
            .child(
                div()
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .truncate()
                    .child(SharedString::from(match branch {
                        Some(branch) => format!("{where_} · {branch}"),
                        None => where_.to_string(),
                    })),
            )
            .into_any_element()
    }

    /// One agent: its state glyph, its title, indented by its depth. The
    /// one in front takes the selection wash; a finished sub-agent fades.
    fn agent_row(
        &self,
        line: AgentLine,
        focused: Option<u64>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = line.id;
        let selected = focused == Some(id);
        let glyph: AnyElement = match line.state {
            ChildState::Working => transcript::spinner(line.since, theme.text_muted, cx),
            ChildState::Asking => icons::icon(icons::system::CHAT_ROUND_LINE)
                .size(px(12.))
                .text_color(theme.accent)
                .into_any_element(),
            ChildState::Waiting if line.main => icons::icon(icons::system::CHAT_ROUND_LINE)
                .size(px(12.))
                .text_color(theme.text_muted)
                .into_any_element(),
            ChildState::Waiting => div()
                .size(px(9.))
                .rounded_full()
                .border_1()
                .border_color(theme.text_faint)
                .into_any_element(),
            ChildState::Done if line.main => icons::icon(icons::system::CHAT_ROUND_LINE)
                .size(px(12.))
                .text_color(theme.text_muted)
                .into_any_element(),
            ChildState::Done => icons::icon(icons::status::CHECK)
                .size(px(12.))
                .text_color(theme.success)
                .into_any_element(),
        };
        let tint = match (selected, line.state, line.main) {
            (true, ..) => theme.text,
            (false, ChildState::Done, false) => theme.text_faint,
            _ => theme.text_muted,
        };
        let title = SharedString::from(line.title);
        let carried = title.clone();
        let markdown: SharedString = self
            .workspace
            .read(cx)
            .chat_link(id)
            .unwrap_or_default()
            .into();
        // The menu anchors under the row unless the header opened it.
        let menu = (!self.menu_at_header)
            .then(|| self.session_menu_element(id, line.archived, cx))
            .flatten();
        let row = row(("panel-agent", id), line.depth, selected, theme)
            .relative()
            .child(glyph_box(glyph))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(tint)
                    .child(title),
            )
            .when(line.main, |el| {
                el.child(
                    div()
                        .flex_none()
                        .text_style(TextStyle::Caption)
                        .text_color(theme.text_faint)
                        .child("main"),
                )
            })
            .children(menu)
            // One click opens the chat; two name it, in the header's field;
            // a secondary click opens its menu.
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.select_session(id, cx);
                if event.click_count() >= 2 {
                    this.rename_header_title(id, window, cx);
                }
            }))
            .on_aux_click(cx.listener(move |this, _, _, cx| {
                this.toggle_menu(Menu::Session(id), cx);
            }));
        // Dragged onto the composer, the chat lands as a chip carrying its
        // link — the way to hand one agent's thread to another.
        self.menu_press(row, Menu::Session(id), cx)
            .on_drag(SessionDrag { markdown }, move |_, _, _, cx| {
                cx.new(|_| Carried(carried.clone()))
            })
            .into_any_element()
    }

    /// A terminal, job, page or panel one of the agents opened.
    fn surface_row(
        &self,
        line: SurfaceLine,
        focused: Option<SurfaceId>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = line.id;
        let selected = focused == Some(id);
        row(("panel-surface", id.0), 0, selected, theme)
            .child(glyph_box(
                icons::icon(line.glyph)
                    .size(px(12.))
                    .text_color(theme.text_muted)
                    .into_any_element(),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if selected {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .child(SharedString::from(line.title)),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.select_surface(id, cx)))
            .into_any_element()
    }

    /// A standing obligation: the goal, and when it next fires. A click
    /// opens the agent that holds it.
    fn standing_row(&self, line: StandingLine, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let agent = line.agent;
        row(("panel-standing", agent), 0, false, theme)
            .child(glyph_box(
                icons::icon(icons::media::REPEAT)
                    .size(px(12.))
                    .text_color(theme.text_muted)
                    .into_any_element(),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.text_muted)
                    .child(SharedString::from(line.goal)),
            )
            .when(!line.when.is_empty(), |el| {
                el.child(
                    div()
                        .flex_none()
                        .text_style(TextStyle::Caption)
                        .text_color(theme.text_faint)
                        .child(SharedString::from(line.when)),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| this.select_session(agent, cx)))
            .into_any_element()
    }

    /// A goals or notes file's first lines, or — for goals, with none
    /// written — an invitation to set them through the main chat.
    fn note_block(
        &self,
        note: Option<&Note>,
        goals: bool,
        remote: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lines = note.map(|note| note.lines.clone()).unwrap_or_default();
        if lines.is_empty() {
            let (hint, verb) = if goals {
                ("No goals yet.", "Set goals…")
            } else {
                ("Nothing written yet.", "Add a note…")
            };
            return div()
                .px(px(8.))
                .py(px(4.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .text_style(TextStyle::Caption)
                .text_color(theme.text_faint)
                .child(hint)
                .when(!remote, |el| {
                    el.child(
                        div()
                            .id(if goals { "panel-set-goals" } else { "panel-add-note" })
                            .self_start()
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .hover(|el| el.text_color(theme.accent_strong))
                            .child(verb)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.invite_note(goals, cx);
                            })),
                    )
                })
                .into_any_element();
        }
        let more = note.map(|note| note.more).unwrap_or_default();
        div()
            .px(px(8.))
            .py(px(4.))
            .flex()
            .flex_col()
            .gap(px(3.))
            .text_style(TextStyle::Caption)
            .text_color(theme.text_muted)
            .children(lines.into_iter().enumerate().map(|(n, line)| {
                div()
                    .line_clamp(2)
                    .when(line.heading, |el| {
                        el.text_color(theme.text_faint).when(n > 0, |el| el.mt(px(4.)))
                    })
                    .child(SharedString::from(line.text))
            }))
            .when(more > 0, |el| {
                el.child(
                    div()
                        .text_color(theme.text_faint)
                        .child(SharedString::from(format!("{more} more"))),
                )
            })
            .into_any_element()
    }

    /// Ask the main chat to write the file: the prompt lands in the
    /// composer, and sending it is the person's call.
    fn invite_note(&mut self, goals: bool, cx: &mut Context<Self>) {
        let prompt = if goals {
            "Fill in .arbos/GOALS.md for this project: the goal, constraints, decisions, and current focus. Ask me what you need to know first."
        } else {
            "Start .arbos/notes.md with what you know about this project so far."
        };
        self.workspace.update(cx, |workspace, cx| {
            if let Some(main) = workspace.active_project().and_then(|p| p.main_session()) {
                workspace.select_session(main, cx);
            }
            workspace.edit_in_composer(prompt.to_string(), cx);
        });
    }

    /// The bottom strip: settings on the left, a sub-chat on the right.
    fn panel_foot(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_none()
            .h(px(40.))
            .px(px(PAD_X))
            .border_t_1()
            .border_color(theme.border)
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                theme
                    .ghost("settings")
                    .px(px(8.))
                    .py(px(6.))
                    .tooltip(|window, cx| Tooltip::with_keystroke("Settings", "⌘,", window, cx))
                    .child(
                        icons::icon(icons::system::SETTINGS_MINIMALISTIC)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_settings(Section::General, cx)
                    })),
            )
            .child(
                theme
                    .ghost("new-subchat")
                    .px(px(8.))
                    .py(px(6.))
                    .tooltip(|window, cx| {
                        Tooltip::with_keystroke("New sub-chat", "⌘N", window, cx)
                    })
                    .child(
                        icons::icon(icons::system::PLUS)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.new_session_action(&NewSession, window, cx)
                    })),
            )
            .into_any_element()
    }

    /// The control that hides the panel and brings it back, for the chat
    /// header's right edge.
    pub(crate) fn panel_toggle(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let label = if self.panel_open {
            "Hide panel"
        } else {
            "Show panel"
        };
        theme
            .ghost("toggle-panel")
            .flex_none()
            .size(px(24.))
            .items_center()
            .justify_center()
            .tooltip(move |window, cx| Tooltip::with_keystroke(label, "⌘B", window, cx))
            .child(
                icons::icon(icons::system::SIDEBAR_MINIMALISTIC)
                    .size(px(14.))
                    .text_color(theme.text_muted),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.toggle_panel_action(&TogglePanel, window, cx)
            }))
            .into_any_element()
    }
}

fn push_children(
    rows: &mut Vec<AgentRow>,
    workspace: &Workspace,
    parent: u64,
    depth: u8,
) {
    for child in workspace.child_summaries(parent) {
        rows.push(AgentRow {
            id: child.id,
            depth,
        });
        push_children(rows, workspace, child.id, depth.saturating_add(1));
    }
}

/// A section's label line, with a count or state at its right edge.
fn section_head(label: &'static str, aside: Option<String>, theme: &Theme) -> AnyElement {
    div()
        .flex_none()
        .h(px(22.))
        .mt(px(SECTION_GAP))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(6.))
        .text_style(TextStyle::Caption)
        .text_color(theme.text_faint)
        .child(label)
        .child(div().flex_1())
        .children(aside.map(SharedString::from))
        .into_any_element()
}

/// The pill every clickable row sits in. Cursor's rows: 26 tall, 5px
/// corners, the hover wash, and the selection wash on the one in front.
fn row(
    id: (&'static str, u64),
    depth: u8,
    selected: bool,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(ROW_HEIGHT))
        .pl(px(8. + TREE_STEP * f32::from(depth)))
        .pr(px(8.))
        .rounded(px(5.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .text_style(TextStyle::Caption)
        .when(selected, |el| el.bg(theme.element_active))
        .when(!selected, |el| el.hover(|el| el.bg(theme.element_hover)))
}

/// A fixed-width box for a row's glyph so titles line up whatever the mark.
fn glyph_box(glyph: AnyElement) -> AnyElement {
    div()
        .flex_none()
        .w(px(12.))
        .flex()
        .justify_center()
        .child(glyph)
        .into_any_element()
}

/// What the store holds, folder by folder: `3 agents`, `2 skills`.
fn store_rows(resources: &[Resource], theme: &Theme) -> Vec<AnyElement> {
    resources
        .iter()
        .map(|resource| {
            div()
                .flex_none()
                .h(px(ROW_HEIGHT))
                .px(px(8.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .text_style(TextStyle::Caption)
                .text_color(theme.text_muted)
                .child(glyph_box(
                    icons::icon(icons::system::WIDGET)
                        .size(px(12.))
                        .text_color(theme.text_muted)
                        .into_any_element(),
                ))
                .child(SharedString::from(format!(
                    "{} {}",
                    resource.count, resource.label
                )))
                .into_any_element()
        })
        .collect()
}
