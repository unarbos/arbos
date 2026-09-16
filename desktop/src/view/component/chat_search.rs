//! Search (⌘K): one palette over every open tab's chats and the app's own
//! actions. Cursor's shape (cycle 20, `cursor-reference/cycle-20/`): a
//! 630-wide panel a quarter of the way down, a search line, filter chips,
//! a "Recent chats" list in columns — status dot, title, project, age —
//! an actions section with its shortcuts, and a footer of key hints. Typed,
//! each hit is its title over the words that matched, with the match lit.
//!
//! Drawn here rather than through bezel's `CommandPalette`: that is one
//! filtered list on a frosted card, and Cursor's is neither.

use crate::view::panel::age;
use bezel::{
    gpui::{
        self, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, MouseButton,
        Render, SharedString, Window, div, prelude::*, px,
    },
    theme::{TextStyle, Theme, Typeset as _},
    ui::{
        input::{self, TextField},
        palette::{Confirm, Dismiss, KEY_CONTEXT, SelectNext, SelectPrevious},
    },
};
use std::time::SystemTime;

/// One chat the palette can open: which tab, which session, and what to
/// draw for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub project: usize,
    pub session: u64,
    /// The chat's title (its name, or its first words).
    pub title: String,
    /// Its first words, when they are not the title.
    pub snippet: String,
    /// The tab it lives on.
    pub tab: String,
    /// Last activity, for the age column.
    pub updated: SystemTime,
    /// A turn or a worker running now.
    pub running: bool,
}

/// An app action the palette offers, as Cursor's "Agent" section does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    NewTab,
    OpenFolder,
    ProjectPage,
    Settings,
    ReportProblem,
}

impl PaletteAction {
    const ALL: [PaletteAction; 5] = [
        PaletteAction::NewTab,
        PaletteAction::OpenFolder,
        PaletteAction::ProjectPage,
        PaletteAction::Settings,
        PaletteAction::ReportProblem,
    ];

    fn label(self) -> &'static str {
        match self {
            PaletteAction::NewTab => "New Tab",
            PaletteAction::OpenFolder => "Open Folder…",
            PaletteAction::ProjectPage => "Project Page",
            PaletteAction::Settings => "Settings",
            PaletteAction::ReportProblem => "Report a Problem…",
        }
    }

    /// The chord, in the pieces the footer draws as keys.
    fn keys(self) -> &'static [&'static str] {
        match self {
            PaletteAction::NewTab => &["⌘", "T"],
            PaletteAction::OpenFolder => &["⌘", "O"],
            PaletteAction::ProjectPage => &["⌘", "2"],
            PaletteAction::Settings => &["⌘", ","],
            PaletteAction::ReportProblem => &["⇧", "⌘", "R"],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatSearchEvent {
    /// Open this chat: switch to its tab and select it.
    Open {
        project: usize,
        session: u64,
    },
    /// Run one of the app's own actions.
    Action(PaletteAction),
    Dismiss,
}

/// Cursor's filter chips, less the two we have no list for (Files, Settings
/// are actions here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Filter {
    All,
    Chats,
    Actions,
}

impl Filter {
    const ALL: [Filter; 3] = [Filter::All, Filter::Chats, Filter::Actions];

    fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Chats => "Chats",
            Filter::Actions => "Actions",
        }
    }

    fn chats(self) -> bool {
        matches!(self, Filter::All | Filter::Chats)
    }

    fn actions(self) -> bool {
        matches!(self, Filter::All | Filter::Actions)
    }
}

/// A row the keyboard can land on, in the order drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Hit(usize),
    Action(PaletteAction),
}

/// Recent chats shown with no query, as Cursor's "Recent Agents".
const RECENT: usize = 6;
const WIDTH: f32 = 630.;

pub struct ChatSearch {
    open: bool,
    query: Entity<TextField>,
    hits: Vec<Hit>,
    filter: Filter,
    active: usize,
    focus: FocusHandle,
}

impl EventEmitter<ChatSearchEvent> for ChatSearch {}

impl ChatSearch {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| {
            TextField::new(cx)
                .with_placeholder("Search chats, actions…")
                .with_frame(false)
        });
        cx.subscribe(&query, |this, _, event: &input::FieldEvent, cx| {
            if *event == input::FieldEvent::Changed {
                this.active = 0;
                cx.notify();
            }
        })
        .detach();
        Self {
            open: false,
            query,
            hits: Vec::new(),
            filter: Filter::All,
            active: 0,
            focus: cx.focus_handle(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Put the palette up over `hits`, newest first as the caller ordered
    /// them, and give it the keyboard.
    pub fn show(&mut self, hits: Vec<Hit>, window: &mut Window, cx: &mut Context<Self>) {
        self.hits = hits;
        self.filter = Filter::All;
        self.active = 0;
        self.open = true;
        self.query.update(cx, |field, cx| field.clear(cx));
        window.focus(&self.query.focus_handle(cx), cx);
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.hits.clear();
        cx.notify();
    }

    fn query_text(&self, cx: &App) -> String {
        self.query.read(cx).content().trim().to_lowercase()
    }

    /// The rows the list draws for the query and the filter, in order.
    fn rows(&self, cx: &App) -> Vec<Row> {
        let q = self.query_text(cx);
        let mut rows = Vec::new();
        if self.filter.chats() {
            let hits = self.hits.iter().enumerate().filter(|(_, hit)| {
                q.is_empty()
                    || hit.title.to_lowercase().contains(&q)
                    || hit.snippet.to_lowercase().contains(&q)
                    || hit.tab.to_lowercase().contains(&q)
            });
            let hits: Vec<Row> = if q.is_empty() {
                hits.take(RECENT).map(|(ix, _)| Row::Hit(ix)).collect()
            } else {
                hits.map(|(ix, _)| Row::Hit(ix)).collect()
            };
            rows.extend(hits);
        }
        if self.filter.actions() {
            rows.extend(
                PaletteAction::ALL
                    .iter()
                    .filter(|action| q.is_empty() || action.label().to_lowercase().contains(&q))
                    .map(|action| Row::Action(*action)),
            );
        }
        rows
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let n = self.rows(cx).len();
        if n == 0 {
            return;
        }
        self.active = (self.active as isize + by).rem_euclid(n as isize) as usize;
        cx.notify();
    }

    fn confirm_row(&mut self, row: Row, cx: &mut Context<Self>) {
        match row {
            Row::Hit(ix) => {
                if let Some(hit) = self.hits.get(ix) {
                    let (project, session) = (hit.project, hit.session);
                    self.close(cx);
                    cx.emit(ChatSearchEvent::Open { project, session });
                }
            }
            Row::Action(action) => {
                self.close(cx);
                cx.emit(ChatSearchEvent::Action(action));
            }
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, cx);
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.rows(cx).get(self.active).copied() {
            self.confirm_row(row, cx);
        }
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        self.close(cx);
        cx.emit(ChatSearchEvent::Dismiss);
    }
}

impl Focusable for ChatSearch {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// `text` with the first occurrence of `needle` (case-insensitive) drawn in
/// the brighter ink, the rest muted — Cursor lights the matched word.
fn lit(text: &str, needle: &str, theme: &Theme) -> gpui::AnyElement {
    let at = (!needle.is_empty())
        .then(|| text.to_lowercase().find(needle))
        .flatten();
    let row = div()
        .flex()
        .flex_row()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_color(theme.text_muted);
    match at {
        Some(start) if start + needle.len() <= text.len() => {
            let end = start + needle.len();
            row.child(text[..start].to_string())
                .child(div().text_color(theme.text).child(text[start..end].to_string()))
                .child(text[end..].to_string())
                .into_any_element()
        }
        _ => row.child(text.to_string()).into_any_element(),
    }
}

fn key_caps(keys: &[&str], theme: &Theme) -> gpui::AnyElement {
    div()
        .flex()
        .flex_row()
        .gap(px(3.))
        .children(keys.iter().map(|key| {
            div()
                .px(px(5.))
                .h(px(18.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .bg(theme.element_active)
                .text_size(px(11.))
                .text_color(theme.text_faint)
                .child(key.to_string())
        }))
        .into_any_element()
}

fn section_label(text: &str, theme: &Theme) -> gpui::AnyElement {
    div()
        .px(px(12.))
        .pt(px(10.))
        .pb(px(4.))
        .text_size(px(11.))
        .text_color(theme.text_faint)
        .child(text.to_string())
        .into_any_element()
}

impl Render for ChatSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        let theme = Theme::of(cx).clone();
        let q = self.query_text(cx);
        let rows = self.rows(cx);
        let active = self.active.min(rows.len().saturating_sub(1));
        let row_base = |theme: &Theme, is_active: bool| {
            div()
                .mx(px(6.))
                .px(px(8.))
                .h(px(30.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(is_active, |el| el.bg(theme.element_active))
                .hover(|el| el.bg(theme.element_hover))
        };

        let mut list: Vec<gpui::AnyElement> = Vec::new();
        let mut hit_rows = Vec::new();
        let mut action_rows = Vec::new();
        for (position, row) in rows.iter().enumerate() {
            match row {
                Row::Hit(ix) => hit_rows.push((position, *ix)),
                Row::Action(action) => action_rows.push((position, *action)),
            }
        }
        if !hit_rows.is_empty() {
            list.push(section_label(
                if q.is_empty() { "Recent chats" } else { "Chats" },
                &theme,
            ));
            for (position, ix) in hit_rows {
                let Some(hit) = self.hits.get(ix) else {
                    continue;
                };
                let is_active = position == active;
                let row = *rows.get(position).unwrap_or(&Row::Hit(ix));
                let dot = div()
                    .size(px(6.))
                    .rounded_full()
                    .bg(if hit.running { theme.accent } else { theme.text_dim });
                let showing_snippet = !q.is_empty() && !hit.snippet.is_empty();
                let body = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .child(
                        div()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_color(theme.text)
                            .child(hit.title.clone()),
                    )
                    .when(showing_snippet, |el| {
                        el.child(div().text_size(px(12.)).child(lit(&hit.snippet, &q, &theme)))
                    });
                list.push(
                    row_base(&theme, is_active)
                        .id(SharedString::from(format!("palette-{ix}")))
                        .when(showing_snippet, |el| el.h(px(44.)))
                        .on_mouse_move(cx.listener(move |this, _, _, cx| {
                            if this.active != position {
                                this.active = position;
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| this.confirm_row(row, cx)))
                        .child(dot)
                        .child(body)
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.text_muted)
                                .whitespace_nowrap()
                                .child(hit.tab.clone()),
                        )
                        .child(
                            div()
                                .w(px(34.))
                                .text_size(px(12.))
                                .text_color(theme.text_faint)
                                .text_right()
                                .child(age(hit.updated)),
                        )
                        .into_any_element(),
                );
            }
        }
        if !action_rows.is_empty() {
            list.push(section_label("Actions", &theme));
            for (position, action) in action_rows {
                let is_active = position == active;
                let row = Row::Action(action);
                list.push(
                    row_base(&theme, is_active)
                        .id(SharedString::from(format!("palette-action-{}", position)))
                        .on_mouse_move(cx.listener(move |this, _, _, cx| {
                            if this.active != position {
                                this.active = position;
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| this.confirm_row(row, cx)))
                        .child(
                            div()
                                .flex_1()
                                .text_color(theme.text)
                                .child(action.label()),
                        )
                        .child(key_caps(action.keys(), &theme))
                        .into_any_element(),
                );
            }
        }
        if list.is_empty() {
            list.push(
                div()
                    .px(px(14.))
                    .py(px(12.))
                    .text_color(theme.text_muted)
                    .child("No matches")
                    .into_any_element(),
            );
        }

        let chips = div()
            .flex()
            .flex_row()
            .gap(px(4.))
            .px(px(12.))
            .py(px(8.))
            .children(Filter::ALL.iter().map(|filter| {
                let filter = *filter;
                let on = self.filter == filter;
                div()
                    .id(SharedString::from(format!("palette-filter-{}", filter.label())))
                    .px(px(8.))
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .when(on, |el| el.bg(theme.element_active).text_color(theme.text))
                    .when(!on, |el| el.text_color(theme.text_muted))
                    .hover(|el| el.bg(theme.element_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = filter;
                        this.active = 0;
                        cx.notify();
                    }))
                    .child(filter.label())
            }));

        let hint = |keys: &[&str], what: &str| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.))
                .child(key_caps(keys, &theme))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme.text_faint)
                        .child(what.to_string()),
                )
        };
        let footer = div()
            .flex()
            .flex_row()
            .gap(px(14.))
            .px(px(12.))
            .py(px(8.))
            .border_t_1()
            .border_color(theme.border)
            .child(hint(&["↑", "↓"], "Select"))
            .child(hint(&["↵"], "Open"))
            .child(hint(&["esc"], "Close"));

        let card = div()
            .id("chat-search-card")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .rounded(px(10.))
            .bg(theme.surface_dialog)
            .border_1()
            .border_color(theme.border)
            .shadow_lg()
            .overflow_hidden()
            .text_style(TextStyle::Body)
            .text_color(theme.text)
            .child(
                div()
                    .px(px(14.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(self.query.clone()),
            )
            .child(chips)
            .child(div().flex().flex_col().pb(px(6.)).children(list))
            .child(footer);

        // A click outside the card closes it, as the tab sheet's does; the
        // card itself keeps the press.
        div()
            .id("chat-search")
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(200.))
            .bg(Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.25,
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close(cx);
                    cx.emit(ChatSearchEvent::Dismiss);
                }),
            )
            .child(card)
            .into_any_element()
    }
}
