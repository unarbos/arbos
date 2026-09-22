//! The right-hand panel: a live view of the project's `.arbos/`. Agents
//! and their sub-agents as a tree (a click puts that chat in the column),
//! the processes those agents started, the resources they hold, and the
//! project page: the status the main chat keeps in `notes.md`, rendered
//! the way Cursor's Projects page is, plus the context document. Nothing
//! else but a settings button at the bottom.

use crate::{
    model::{
        panel::OpenedBy,
        session::{ChatItem, ChatSession, ChildState},
        store_view::{FileKind, PageBlock, PageItem, ProjectPage, Resource, StoreFile, Target},
        surface::{Surface, SurfaceId, SurfaceKind},
        workspace::Workspace,
    },
    view::{
        component::{composer::SessionDrag, menu::Menu, surface as board, transcript},
        root::{Arbos, TogglePanel},
    },
};
use bezel::{
    gpui::{
        AnyElement, App, ClickEvent, Context, Div, FontWeight, Hsla, Render, SharedString,
        Stateful, Window, div, prelude::*, px,
    },
    theme::{TextStyle, Theme, Typeset},
    ui::{icons, popover, tooltip::Tooltip, widgets::Buttons},
};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

/// The glyph a store file wears, by kind.
pub(crate) fn file_glyph(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Markdown => icons::files::DOCUMENT,
        FileKind::Image => icons::system::WIDGET,
        FileKind::Other => icons::files::FOLDER_WITH_FILES,
    }
}

/// How long ago, in the coarsest unit that still says something:
/// `now`, `4m`, `2h`, `3d`.
pub(crate) fn age(at: SystemTime) -> String {
    let secs = at.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        s if s < 60 => "now".into(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// The panel's width. Cursor's right panel runs 280–320 at a 1728 window.
pub(crate) const PANEL_WIDTH: f32 = 280.;

/// Below this window width the panel is left out; the chat comes first.
/// Under this width the panel folds away and the chat takes the column:
/// a 900-pt window (Mac cycle 11's small-window still) keeps a readable
/// transcript instead of a 620-pt one beside a full panel.
pub(crate) const PANEL_MIN_WINDOW: f32 = 1000.;

/// A row's height, and the step each level of the tree indents by.
const ROW_HEIGHT: f32 = 26.;
const TREE_STEP: f32 = 14.;

/// Padding inside the panel, and between its sections.
const PAD_X: f32 = 10.;
const SECTION_GAP: f32 = 14.;

/// How large the project page is drawn: in the panel's caption size, or
/// at reading size for the page in the column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageScale {
    Panel,
    Page,
}

impl PageScale {
    fn text(self) -> TextStyle {
        match self {
            Self::Panel => TextStyle::Caption,
            Self::Page => TextStyle::Body,
        }
    }

    /// A `##` heading; `###` steps one size down.
    fn heading(self, level: u8) -> TextStyle {
        match (self, level >= 3) {
            (Self::Panel, _) => TextStyle::Caption,
            (Self::Page, false) => TextStyle::Title3,
            (Self::Page, true) => TextStyle::Callout,
        }
    }

    fn glyph(self) -> f32 {
        match self {
            Self::Panel => 12.,
            Self::Page => 14.,
        }
    }

    /// A row's vertical padding: Cursor's page rows breathe.
    fn row_py(self) -> f32 {
        match self {
            Self::Panel => 3.,
            Self::Page => 8.,
        }
    }

    /// The left inset a row and a heading share.
    fn inset(self) -> f32 {
        match self {
            Self::Panel => 8.,
            Self::Page => 0.,
        }
    }
}

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
    /// Cursor's task-list rows read "title — summary": the step while it
    /// works, its last words once done. None for the main chat.
    summary: Option<String>,
    /// The main chat's "waiting on <worker> — <step>" while a worker is
    /// live (kernel #366). The row draws whom it waits on, dim; the step is
    /// on the worker's row under it and on the chat's live line.
    waiting: Option<String>,
    /// The worker cannot write: the same mark as on its line in the chat.
    readonly: bool,
}

/// A worker the kernel archived that this window never had a row for:
/// known only by its folder under `archive/agents/`.
struct ArchivedOnly {
    id: String,
    title: String,
    /// The worker's last words, from its archived transcript — the same
    /// line an archived row with a chat here shows after its title.
    summary: Option<String>,
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

/// Who holds a standing obligation: an attached chat (by its id here), or
/// an agent known only by its kernel id (read off `subscriptions/`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Holder {
    Session(u64),
    Kernel(String),
}

/// A standing obligation one of the agents holds: `every 1h · next 15:04`.
struct StandingLine {
    agent: Holder,
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

    /// The rows the keyboard steps through: what the panel shows. Archived
    /// workers sit behind the "N archived" row, so while it is folded the
    /// arrows step over them, as the eye does.
    pub(crate) fn visible_agent_rows(&self, cx: &App) -> Vec<AgentRow> {
        let mut rows = self.agent_rows(cx);
        if !self.archived_open {
            let workspace = self.workspace.read(cx);
            if let Some(project) = workspace.active_project() {
                rows.retain(|row| {
                    row.depth == 0
                        || project
                            .session(row.id)
                            .is_none_or(|chat| !(chat.closed || chat.agent_gone()))
                });
            }
        }
        rows
    }

    /// The project tab's body: a live view of this project's `.arbos/` —
    /// its agents and their workers, the processes they started, the
    /// resources they hold, and the project page. The drawer around it,
    /// and its other tabs, are in [`crate::view::drawer`].
    pub(crate) fn panel_store_body(
        &self,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
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
                let archived = n != 0 && (chat.closed || chat.agent_gone());
                // An archived worker's folder moved with its name; a row
                // that never learned it reads the name from the archive.
                let title = match (&chat.name, chat.agent_gone(), &chat.agent_session) {
                    (None, true, Some(sid)) if !remote => archived_title(&place.path, sid)
                        .unwrap_or_else(|| workspace.display_label(row.id)),
                    _ => workspace.display_label(row.id),
                };
                Some(AgentLine {
                    id: row.id,
                    depth: row.depth,
                    title,
                    state: chat.child_state(),
                    main: n == 0,
                    // Closed here, or moved to `archive/agents/` by the
                    // kernel once its done was read: out of the live list.
                    archived,
                    // A kernel child has no flight to time; the shared clock
                    // keeps its spinner turning.
                    since: chat.elapsed().unwrap_or_else(transcript::live_phase),
                    summary: (n != 0).then(|| row_summary(chat)).flatten(),
                    waiting: (n == 0).then(|| chat.waiting.clone()).flatten(),
                    readonly: chat.readonly,
                })
            })
            .collect();
        let (agents, archived): (Vec<AgentLine>, Vec<AgentLine>) =
            agents.into_iter().partition(|line| !line.archived);
        let archived_only = (!remote)
            .then(|| archived_only(&place.path, &project.sessions))
            .unwrap_or_default();
        let archived_count = archived.len() + archived_only.len();
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
            .filter(|surface| matches!(surface.kind, SurfaceKind::Terminal | SurfaceKind::Process))
            .map(|surface| line_of(surface))
            .collect();
        let resources: Vec<SurfaceLine> = surfaces
            .iter()
            .filter(|surface| matches!(surface.kind, SurfaceKind::Browser | SurfaceKind::Panel))
            .map(|surface| line_of(surface))
            .collect();
        // Standing work: the `subscriptions/` files when the store has
        // them (the Cursor-model kernel); else what attached chats report
        // in their plan (an older kernel, or a remote place).
        let standing: Vec<StandingLine> = if !store.standing_known {
            project
                .sessions
                .iter()
                .filter(|chat| !chat.closed)
                .flat_map(|chat| {
                    chat.plan_open()
                        .filter(|node| node.standing)
                        .map(move |node| StandingLine {
                            agent: Holder::Session(chat.id),
                            goal: node.goal.clone(),
                            when: node.when.clone(),
                        })
                })
                .collect()
        } else {
            store
                .standing
                .iter()
                .map(|sub| StandingLine {
                    agent: Holder::Kernel(sub.agent.clone()),
                    goal: if sub.paused {
                        format!("{} (paused)", sub.label)
                    } else {
                        sub.label.clone()
                    },
                    when: sub.when.clone(),
                })
                .collect()
        };

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
        if archived_count > 0 {
            body = body.child(self.archived_head(archived_count, &theme, cx));
            if self.archived_open {
                body = body
                    .children(
                        archived
                            .into_iter()
                            .map(|line| self.agent_row(line, focused, &theme, cx)),
                    )
                    .children(
                        archived_only
                            .into_iter()
                            .map(|line| archived_only_row(line, &theme)),
                    );
            }
        }
        if !processes.is_empty() {
            body = body
                .child(section_head("Processes", None, &theme))
                .children(
                    processes
                        .into_iter()
                        .map(|line| self.surface_row(line, focused_surface, &theme, cx)),
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
        // The project page stays in this panel. A click must not put it
        // over the chat (#629).
        body = body
            .child(section_head("Project", None, &theme))
            .child(self.project_page(store.page.as_ref(), remote, PageScale::Panel, &theme, cx))
            .children(self.files_rows(&store.files, &theme, cx));
        if !standing.is_empty() {
            body = body
                .child(page_heading(2, "Standing", true, PageScale::Panel, &theme))
                .children(
                    standing
                        .into_iter()
                        .enumerate()
                        .map(|(n, line)| self.standing_row(n as u64, line, &theme, cx)),
                );
        }

        Some(body.into_any_element())
    }

    /// The project's name, where it lives, and the branch checked out there.
    /// The handset is on the composer, beside the mic, not up here.
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(name.to_string())),
                    ),
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
    /// "N archived": the workers the kernel moved out of the live tree, one
    /// row that unfolds them, faint with their checks, below the live ones.
    fn archived_head(&self, count: usize, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let open = self.archived_open;
        row(("panel-archived", 0), 1, false, theme)
            .child(glyph_box(
                icons::icon(if open {
                    icons::arrows::ALT_ARROW_DOWN
                } else {
                    icons::arrows::ALT_ARROW_RIGHT
                })
                .size(px(11.))
                .text_color(theme.text_faint)
                .into_any_element(),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.text_faint)
                    .child(SharedString::from(format!("{count} archived"))),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.archived_open = !this.archived_open;
                cx.notify();
            }))
            .into_any_element()
    }

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
        let waiting_on = line.waiting.is_some();
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
                    .flex()
                    .flex_row()
                    .items_baseline()
                    // While the main chat waits on a worker the step is the
                    // news: the title gives way to it, not the other way.
                    .child(
                        div()
                            .when(waiting_on, |el| el.min_w_0().flex_shrink(1.))
                            .when(!waiting_on, |el| el.flex_none())
                            .truncate()
                            .text_color(tint)
                            .child(title),
                    )
                    .when(line.readonly, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .ml(px(4.))
                                .child(transcript::readonly_mark(theme, 10.)),
                        )
                    })
                    .when_some(line.summary, |el, summary| {
                        el.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.text_faint)
                                .child(SharedString::from(format!(" — {summary}"))),
                        )
                    })
                    .when_some(line.waiting, |el, waiting| {
                        // "· waiting on Slow builder": whom the main chat
                        // waits on. The worker's own row, right under it,
                        // carries the step and its clock; the chat's live
                        // line carries it too. The panel is too narrow to
                        // say the step a third time.
                        let who = waiting
                            .split_once(" — ")
                            .map_or(waiting.as_str(), |(who, _)| who)
                            .to_string();
                        el.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.text_faint)
                                .child(SharedString::from(format!(" · {who}"))),
                        )
                    }),
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
            // Into the drawer's tab row, exactly as a click on its tab does:
            // one thing a row can mean, whichever list it was clicked in.
            .on_click(cx.listener(move |this, _, window, cx| this.show_surface(id, window, cx)))
            .into_any_element()
    }

    /// A standing obligation: the goal, and when it next fires. A click
    /// opens the agent that holds it.
    fn standing_row(
        &self,
        n: u64,
        line: StandingLine,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let holder = line.agent;
        row(("panel-standing", n), 0, false, theme)
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
            .on_click(cx.listener(move |this, _, _, cx| match &holder {
                Holder::Session(id) => this.select_session(*id, cx),
                Holder::Kernel(kernel_id) => this.open_worker(kernel_id.clone(), cx),
            }))
            .into_any_element()
    }

    /// The chat of the agent with this kernel id: the main chat for root,
    /// else the worker's own (adopted under the main chat if it was not
    /// yet shown).
    fn open_worker(&mut self, kernel_id: String, cx: &mut Context<Self>) {
        self.workspace.update(cx, |workspace, cx| {
            let Some(main) = workspace.active_project().and_then(|p| p.main_session()) else {
                return;
            };
            if kernel_id == "root" {
                workspace.select_session(main, cx);
                return;
            }
            if let Some(id) = workspace.ensure_child_agent(main, kernel_id, cx) {
                workspace.select_session(id, cx);
            }
        });
    }

    /// The store's files under the page: the context document first, then
    /// the rest. They stay in this panel.
    fn files_rows(
        &self,
        files: &[StoreFile],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if files.is_empty() {
            return Vec::new();
        }
        let mut rows = vec![page_heading(2, "Files", true, PageScale::Panel, theme)];
        rows.extend(
            files
                .iter()
                .enumerate()
                .map(|(n, file)| self.file_row(("panel-file", n as u64), file, theme, cx)),
        );
        rows
    }

    /// One file: its kind's glyph, its name, and how long ago it changed,
    /// dim at the right. A click opens it as a tab of this panel.
    pub(crate) fn file_row(
        &self,
        id: (&'static str, u64),
        file: &StoreFile,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = file.path.clone();
        let title = file.name.clone();
        row(id, 0, false, theme)
            .child(glyph_box(
                icons::icon(file_glyph(file.kind))
                    .size(px(12.))
                    .text_color(if file.pinned {
                        theme.accent
                    } else {
                        theme.text_muted
                    })
                    .into_any_element(),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.text_muted)
                    .child(SharedString::from(if file.pinned {
                        "Context".to_string()
                    } else {
                        file.name.clone()
                    })),
            )
            .children(file.modified.map(|at| {
                div()
                    .flex_none()
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child(SharedString::from(age(at)))
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_store_file(path.clone(), &title, cx);
            }))
            .into_any_element()
    }

    /// The status page as Cursor's Projects page draws it: tldr bullets,
    /// section headings, checkbox rows whose label is the link and whose
    /// readout sits dim under it. With nothing written yet, an invitation
    /// to start it through the main chat.
    pub(crate) fn project_page(
        &self,
        page: Option<&ProjectPage>,
        remote: bool,
        scale: PageScale,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(page) = page.filter(|page| !page.is_empty()) else {
            return div()
                .px(px(scale.inset()))
                .py(px(4.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .text_style(scale.text())
                .text_color(theme.text_faint)
                .child("Nothing on the project page yet.")
                .when(!remote, |el| {
                    el.child(
                        div()
                            .id("panel-start-page")
                            .self_start()
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .hover(|el| el.text_color(theme.accent_strong))
                            .child("Start the page…")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.invite_page(cx);
                            })),
                    )
                })
                .into_any_element();
        };
        let (tldr_id, item_id): (&'static str, &'static str) = match scale {
            PageScale::Panel => ("panel-tldr-item", "panel-page-item"),
            PageScale::Page => ("page-tldr-item", "page-item"),
        };
        let mut body = div().flex().flex_col();
        if !page.tldr.is_empty() {
            body = body.child(
                div()
                    .id(match scale {
                        PageScale::Panel => "panel-tldr",
                        PageScale::Page => "page-tldr",
                    })
                    .mx(px(scale.inset() / 2.))
                    .mb(px(6.))
                    .px(px(4.))
                    .py(px(2.))
                    .rounded(px(5.))
                    .bg(theme.element_hover)
                    .flex()
                    .flex_col()
                    .children(page.tldr.iter().enumerate().map(|(n, item)| {
                        self.page_item((tldr_id, n as u64), item, scale, theme, cx)
                    })),
            );
        }
        for (n, block) in page.blocks.iter().enumerate() {
            body = body.child(match block {
                PageBlock::Heading { level, text } => {
                    page_heading(*level, text, n > 0, scale, theme)
                }
                PageBlock::Item(item) => {
                    self.page_item((item_id, n as u64), item, scale, theme, cx)
                }
            });
        }
        body.into_any_element()
    }

    /// One item: its checkbox (or a dot for a tldr bullet), the label as a
    /// link, and the readout dim under it.
    fn page_item(
        &self,
        id: (&'static str, u64),
        item: &PageItem,
        scale: PageScale,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let g = scale.glyph();
        let glyph: AnyElement = match item.done {
            Some(true) => icons::icon(icons::status::CHECK)
                .size(px(g))
                .text_color(theme.success)
                .into_any_element(),
            // Cursor's open item: a hollow circle.
            Some(false) => div()
                .size(px(g - 2.))
                .rounded_full()
                .border_1()
                .border_color(theme.text_faint)
                .into_any_element(),
            None => div()
                .size(px(4.))
                .rounded_full()
                .bg(theme.text_faint)
                .into_any_element(),
        };
        let label_tint = match (item.done, item.target.is_some()) {
            (Some(true), _) => theme.text_faint,
            (_, true) => theme.text,
            (_, false) => theme.text_muted,
        };
        let target = item.target.clone();
        let label_text = item.label.clone();
        let label = div()
            .id((id.0, id.1.wrapping_mul(2)))
            .min_w_0()
            .truncate()
            .text_color(label_tint)
            // Cursor's page: the label in bold, the readout plain and dim.
            .when(scale == PageScale::Page && item.done.is_some(), |el| {
                el.font_weight(FontWeight::SEMIBOLD)
            })
            .when(target.is_some(), |el| {
                el.cursor_pointer()
                    .hover(|el| el.text_color(theme.accent))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(target) = target.clone() {
                            this.follow_page_link(&target, &label_text, cx);
                        }
                    }))
            })
            .child(SharedString::from(chip_label(item)));
        div()
            .id(id)
            .flex_none()
            .pl(px(scale.inset() + TREE_STEP * f32::from(item.depth)))
            .pr(px(scale.inset()))
            .py(px(scale.row_py()))
            .flex()
            .flex_row()
            .items_start()
            .gap(px(if scale == PageScale::Page { 10. } else { 8. }))
            .text_style(scale.text())
            .when(!item.prose, |row| {
                row.child(
                    div()
                        .flex_none()
                        .w(px(g))
                        .pt(px(if scale == PageScale::Page { 4. } else { 3. }))
                        .flex()
                        .justify_center()
                        .child(glyph),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(label)
                    .when(!item.readout.is_empty(), |el| {
                        el.child(
                            div()
                                .line_clamp(2)
                                .text_color(theme.text_faint)
                                .child(SharedString::from(plain_links(&item.readout))),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Where a page link goes: a URL to the browser (an `arbos://` chat
    /// link stays in the app), a worker to its chat, a file to the column.
    fn follow_page_link(&mut self, target: &Target, label: &str, cx: &mut Context<Self>) {
        match target {
            Target::Url(url) if url.starts_with("arbos://") => {
                self.workspace.update(cx, |workspace, cx| {
                    workspace.open_chat_link(url, cx);
                });
            }
            Target::Url(url) => cx.open_url(url),
            Target::Worker(kernel_id) => self.open_worker(kernel_id.clone(), cx),
            Target::File(path) => self.open_store_file(path.clone(), label, cx),
        }
    }

    /// A file of the store in the column, as a document panel under the
    /// main chat.
    pub(crate) fn open_store_file(&mut self, path: PathBuf, title: &str, cx: &mut Context<Self>) {
        let title = title.to_owned();
        // The viewer picks its presenter by the kind: a picture as a
        // picture, markdown as a document, anything else by its extension.
        let ext = path
            .extension()
            .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let kind = match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "avif" => "image",
            "md" | "markdown" | "txt" | "" => "doc",
            _ => "file",
        };
        self.workspace.update(cx, |workspace, cx| {
            let Some(main) = workspace.active_project().and_then(|p| p.main_session()) else {
                return;
            };
            workspace.open_shown(
                main,
                path.display().to_string(),
                title,
                kind.into(),
                None,
                None,
                OpenedBy::User,
                cx,
            );
        });
    }

    /// Ask the main chat to start the page: the prompt lands in the
    /// composer, and sending it is the person's call.
    fn invite_page(&mut self, cx: &mut Context<Self>) {
        let prompt = "Fill in .arbos/docs/project-context.md for this project (goal, constraints, decisions, resources) and start .arbos/notes.md with the workstreams you know of. Ask me what you need to know first.";
        // From the Project page the composer is out of sight: go to the
        // chat first, so the link visibly does something.
        self.show_pane(crate::view::root::Pane::Chat, cx);
        self.workspace.update(cx, |workspace, cx| {
            if let Some(main) = workspace.active_project().and_then(|p| p.main_session()) {
                workspace.select_session(main, cx);
            }
            workspace.edit_in_composer(prompt.to_string(), cx);
        });
    }

    /// The control that hides the panel and brings it back, on the window
    /// tab strip. It is the last control on the strip: the four-box that
    /// widened the panel is gone (Jacob, 09-18); ⌘\ still widens it.
    #[allow(dead_code)]
    pub(crate) fn panel_toggle(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let open = self
            .workspace
            .read(cx)
            .panel()
            .is_some_and(|panel| panel.open);
        let label = if open { "Hide panel" } else { "Show panel" };
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
            .on_click(
                cx.listener(|this, _, window, cx| {
                    this.toggle_panel_action(&TogglePanel, window, cx)
                }),
            )
            .into_any_element()
    }
}

fn push_children(rows: &mut Vec<AgentRow>, workspace: &Workspace, parent: u64, depth: u8) {
    for child in workspace.child_summaries(parent) {
        rows.push(AgentRow {
            id: child.id,
            depth,
        });
        push_children(rows, workspace, child.id, depth.saturating_add(1));
    }
}

/// The title an archived worker's `agent.md` carries (`title:`, else
/// `name:`), read from `archive/agents/<id>/`.
fn archived_title(path: &std::path::Path, id: &str) -> Option<String> {
    let agent_md = arbos_core::Place::new(path)
        .arbos()
        .join("archive")
        .join("agents")
        .join(id)
        .join("agent.md");
    let text = std::fs::read_to_string(agent_md).ok()?;
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key))
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    field("title:").or_else(|| field("name:"))
}

/// Archived workers this window has no row for: folders under
/// `.arbos/archive/agents/` whose id no session here carries. Titled from
/// their `agent.md` (`title:`, else `name:`, else the folder).
fn archived_only(path: &std::path::Path, sessions: &[ChatSession]) -> Vec<ArchivedOnly> {
    let dir = arbos_core::Place::new(path)
        .arbos()
        .join("archive")
        .join("agents");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<ArchivedOnly> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let id = entry.file_name().to_string_lossy().into_owned();
            if sessions
                .iter()
                .any(|chat| chat.agent_session.as_deref() == Some(id.as_str()))
            {
                return None;
            }
            let agent_md =
                std::fs::read_to_string(entry.path().join("agent.md")).unwrap_or_default();
            let field = |key: &str| {
                agent_md
                    .lines()
                    .find_map(|line| line.strip_prefix(key))
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned)
            };
            let title = field("title:")
                .or_else(|| field("name:"))
                .unwrap_or_else(|| id.clone());
            let summary = archived_summary(&entry.path().join("transcript.jsonl"));
            Some(ArchivedOnly { id, title, summary })
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// The last thing an archived worker said, first line, as the row shows
/// it after the title — read from the archive when no chat here holds it.
fn archived_summary(transcript: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(transcript).ok()?;
    let last = text.lines().rev().find_map(|line| {
        let event: arbos_core::Event = serde_json::from_str(line).ok()?;
        match event.kind {
            arbos_core::EventKind::Assistant { text, .. } if !text.trim().is_empty() => Some(text),
            _ => None,
        }
    })?;
    let line = last
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))?
        .trim_start_matches(['-', '*', ' '])
        .to_string();
    let mut phrase: String = line.chars().take(90).collect();
    if line.chars().count() > 90 {
        phrase.push('…');
    }
    Some(phrase)
}

/// A row for an archived worker with no chat here: its title, a check,
/// its last words, nothing to click — the history is the kernel's
/// (`grep scope=history`). Same indent as the archived rows that do have
/// a chat: one list under "N archived", not a second step with no header
/// over it (F-183, cycle 39 d17).
fn archived_only_row(line: ArchivedOnly, theme: &Theme) -> AnyElement {
    div()
        .flex_none()
        .h(px(ROW_HEIGHT))
        .pl(px(8. + TREE_STEP))
        .pr(px(8.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .text_style(TextStyle::Caption)
        .child(glyph_box(
            icons::icon(icons::status::CHECK)
                .size(px(12.))
                .text_color(theme.text_faint)
                .into_any_element(),
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_baseline()
                .child(
                    div()
                        .flex_none()
                        .truncate()
                        .text_color(theme.text_faint)
                        .child(SharedString::from(line.title)),
                )
                .when_some(line.summary, |el, summary| {
                    el.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.text_faint)
                            .child(SharedString::from(format!(" — {summary}"))),
                    )
                }),
        )
        .into_any_element()
}

/// A page item's label as prose shows it: the chip glyph for what it
/// points at (`⚙` a worker, `⛓` a pull request, `📄` a document), then
/// the words.
fn chip_label(item: &PageItem) -> String {
    use crate::view::chips::{self, Kind};
    let kind = match &item.target {
        Some(Target::Worker(_)) => Kind::Agent,
        Some(Target::Url(url)) => chips::classify(url),
        Some(Target::File(path)) => chips::classify(&path.to_string_lossy()),
        None => Kind::Other,
    };
    match chips::glyph(kind) {
        Some(glyph) if !item.label.starts_with(glyph) => format!("{glyph} {}", item.label),
        _ => item.label.clone(),
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

/// A `##` or `###` heading of the page: a faint caption, the deeper one
/// indented a step.
pub(crate) fn page_heading(
    level: u8,
    text: &str,
    gap_above: bool,
    scale: PageScale,
    theme: &Theme,
) -> AnyElement {
    let sub = level >= 3;
    let page = scale == PageScale::Page;
    div()
        .flex_none()
        .pl(px(scale.inset() + if sub && !page { TREE_STEP } else { 0. }))
        .pr(px(scale.inset()))
        .pt(px(match (page, gap_above) {
            (true, true) => 28.,
            (true, false) => 8.,
            (false, true) => 8.,
            (false, false) => 2.,
        }))
        .pb(px(if page { 10. } else { 2. }))
        .text_style(scale.heading(level))
        .when(page, |el| el.font_weight(FontWeight::SEMIBOLD))
        .text_color(match (page, sub) {
            (true, false) => theme.text,
            (true, true) => theme.text_muted,
            (false, true) => theme.text_faint,
            (false, false) => theme.text_muted,
        })
        .truncate()
        .child(SharedString::from(text.to_owned()))
        .into_any_element()
}

/// The pill every clickable row sits in. Cursor's rows: 26 tall, 5px
/// corners, the hover wash, and the selection wash on the one in front.
fn row(id: (&'static str, u64), depth: u8, selected: bool, theme: &Theme) -> Stateful<Div> {
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

/// A worker row's em-dash summary: the live step while it works, else the
/// first line of its last words. Cut to a phrase; the row truncates the rest.
fn row_summary(chat: &ChatSession) -> Option<String> {
    let text = match chat.child_state() {
        ChildState::Working => chat.current_step()?,
        ChildState::Asking | ChildState::Waiting => return None,
        ChildState::Done => chat.items.iter().rev().find_map(|item| match item {
            ChatItem::Agent(text) if !text.trim().is_empty() => Some(text.clone()),
            _ => None,
        })?,
    };
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))?
        .trim_start_matches(['-', '*', ' '])
        .to_string();
    let mut phrase: String = line.chars().take(90).collect();
    if line.chars().count() > 90 {
        phrase.push('…');
    }
    Some(phrase)
}

/// A readout's markdown links as words: "[PR 1](https://…)" → "PR 1". The
/// row is one dim line; the link itself is on the page.
fn plain_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match (after.find("]("), after.find(')')) {
            (Some(close), Some(end)) if end > close => {
                out.push_str(&after[..close]);
                rest = &after[end + 1..];
            }
            _ => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}
