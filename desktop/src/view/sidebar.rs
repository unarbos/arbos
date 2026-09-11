//! The projects sidebar: a folding heading per project, and every session,
//! article and table in it. The window's grid lives in [`crate::view::root`];
//! this draws on it.

use crate::{
    model::surface::{Child, SurfaceId},
    view::{
        component::{
            composer::SessionDrag,
            menu::{self, Menu},
            surface as board, transcript,
        },
        root::{self, CommitName, Cydonia, DismissName, NewSession, OpenProject, Pane},
        settings::Section,
    },
};
use bezel::{
    gpui::{
        self, AnyElement, App, Bounds, ClickEvent, Context, Div, DragMoveEvent, Empty, Entity,
        Focusable as _, Hsla, MouseButton, MouseDownEvent, Pixels, Point, ScrollStrategy,
        SharedString, Stateful, TextRun, UniformListDecoration, Window, canvas, div, fill, font,
        point, prelude::*, px, size, svg, uniform_list,
    },
    theme::{TextStyle, Theme, Typeset},
    ui::{
        icons, input,
        menu::Item,
        popover,
        surface::Surfaced as _,
        tooltip::Tooltip,
        widgets::{Buttons, Layout},
    },
};
use std::{
    ops::Range,
    time::{Duration, SystemTime},
};

/// What the sidebar needs of a session to draw its row, read out of the model
/// before the row is built. A turn in flight puts a braille spinner in the
/// mark's place.
struct SessionRow {
    id: u64,
    label: String,
    icon: Option<SharedString>,
    delegate: bool,
    /// How long the turn has been running, or `None` when nothing is in flight.
    working: Option<Duration>,
    archived: bool,
    /// The agent holds a standing obligation: the clock will fire it.
    standing: bool,
    /// The agent has a question parked for the user.
    asking: bool,
    /// When the chat last changed, for the age at the row's right edge.
    updated: SystemTime,
}

/// One line of the sidebar. An address, not content: the label behind it is
/// read when the row is built, which [`uniform_list`] only does for the rows on
/// screen.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    Project(usize),
    Session {
        project: usize,
        id: u64,
        /// Indented under a focused agent — a child agent, not a root.
        nested: bool,
    },
    Surface {
        project: usize,
        id: SurfaceId,
    },
}

/// What the sidebar's name field is attached to. One field for all of them,
/// because only one row can be being named at a time. Each entry is held by
/// what identifies it — a file, a session, a table's key — never by an index:
/// that moves the moment a neighbour is made or dropped, and the field would
/// follow it onto whichever entry slid underneath.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Renaming {
    Session(u64),
    /// The project's place string — the same key state.toml uses.
    Project(String),
}

/// What an entry's row is written in: the one on screen at full strength, one
/// put away a step back from the rest.
pub(crate) fn tint(selected: bool, archived: bool, theme: &Theme) -> Hsla {
    match (selected, archived) {
        (true, _) => theme.text,
        (false, true) => theme.text_faint,
        (false, false) => theme.text_muted,
    }
}

/// A project on its way to another place in the list. The index is safe to
/// carry: nothing reorders the list while a drag is in flight.
#[derive(Clone)]
pub(crate) struct ProjectDrag(usize);

/// Where a dragged chat will land. `before` is the row it sits in
/// front of; `None` is after the last in that list. `archived` is
/// which list: the open chats, or the ones the header toggle shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionDrop {
    before: Option<u64>,
    /// The list row the line is drawn above.
    at: usize,
    nested: bool,
    /// Drop into the archived list. False is the open list.
    archived: bool,
}

/// What rides under the cursor while a project is being carried.
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

/// The wash a row paints, and — with the 1px either side of it that used to be
/// the column's gap — the pitch the list lays every row out at. One height for
/// headings and rows alike, because [`uniform_list`] measures a single row and
/// gives every other one the same.
const ROW_PILL: f32 = 30.;
pub(crate) const ROW_HEIGHT: f32 = ROW_PILL + 2.;

/// How far the pinned heading's glass runs past the band it is seen in, and is
/// clipped away.
///
/// A lens bends what is behind it within [`bezel::theme::SurfaceSpec::rim`] of
/// its own edge, and lights the edge itself. On a bar one row tall that is the
/// whole of it: two bands and two hairlines, reading as a line ruled along the
/// top and the bottom. Run the glass out past the clip and only its middle —
/// the flat blur — is left in view.
const PINNED_BLEED: f32 = 20.;

/// Where the project heading's name starts: the pill's margin and padding,
/// its 14px mark, and the 4px gap. Every trunk hangs from a name, so this
/// is the tree's origin.
const HEAD_NAME_X: f32 = root::SIDEBAR_GUTTER + 6. + 14. + 4.;
/// One step of the tree: a child's name sits this far past its parent's.
/// Room for the trunk under the parent's first letter, the tick, the
/// child's mark, and the gap before its name.
const TREE_STEP: f32 = 28.;
/// The trunk runs this far in from the parent's name — under its first
/// letter rather than on its edge.
const TREE_TRUNK: f32 = 3.;
/// Every row's mark, root or child, and the gap to its name. One size, so
/// the steps come out even.
const ROW_MARK: f32 = 12.;
const ROW_GAP: f32 = 6.;

/// Where a row's name starts. Depth 0 is a root chat.
/// A first-level chat's name sits on the repository name's x, as Cursor's
/// do; each level below steps in by `TREE_STEP`.
fn tree_name_x(depth: u8) -> f32 {
    HEAD_NAME_X + depth as f32 * TREE_STEP
}

/// Where a row's mark starts: the tick lands here.
fn tree_mark_x(depth: u8) -> f32 {
    tree_name_x(depth) - ROW_MARK - ROW_GAP
}

/// A row's pill margin: the mark sits one gutter of padding inside it.
fn tree_inset(depth: u8) -> f32 {
    tree_mark_x(depth) - root::SIDEBAR_GUTTER
}

/// The trunk a row at `depth` hangs from: under its parent's name.
fn tree_trunk_x(depth: u8) -> f32 {
    let parent = if depth == 0 {
        HEAD_NAME_X
    } else {
        tree_name_x(depth - 1)
    };
    parent + TREE_TRUNK
}

/// Where a row stands in the tree, so it can draw its own connector and
/// the trunks of the ancestors still running past it.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Guide {
    /// 0 is a root chat under a project heading.
    depth: u8,
    /// The last child of its parent: `└──`, not `├──`.
    last: bool,
    /// Bit `a` set: the ancestor at depth `a` has later siblings, so its
    /// trunk runs the full height of this row.
    trunks: u32,
}

impl Guide {
    fn child(self, n: usize, count: usize) -> Self {
        let trunks = if self.last {
            self.trunks
        } else {
            self.trunks | (1 << self.depth)
        };
        Self {
            depth: self.depth + 1,
            last: n + 1 == count,
            trunks,
        }
    }
}

/// The `├──` / `└──` a row hangs from, plus the trunks of ancestors that
/// continue below it. Painted across the row's full pitch so the lines meet
/// between rows.
/// "3m", "6h", "2d": the coarsest unit that still says something, as
/// Cursor's sidebar puts it. Under a minute reads "now".
fn age_label(at: SystemTime) -> String {
    let secs = at.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        s if s < 60 => "now".to_owned(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

fn tree_lines(guide: Guide, color: Hsla) -> AnyElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let left = bounds.origin.x;
            let top = bounds.origin.y;
            let height = bounds.size.height;
            let mid = top + height / 2.;
            let rule = |x: Pixels, y: Pixels, w: Pixels, h: Pixels, window: &mut Window| {
                window.paint_quad(fill(Bounds::new(point(x, y), size(w, h)), color));
            };
            for level in 0..guide.depth {
                if guide.trunks & (1 << level) != 0 {
                    rule(left + px(tree_trunk_x(level)), top, px(1.), height, window);
                }
            }
            let x = left + px(tree_trunk_x(guide.depth));
            let end = if guide.last { mid } else { top + height };
            rule(x, top, px(1.), end - top, window);
            let tick = tree_mark_x(guide.depth) - tree_trunk_x(guide.depth);
            rule(x, mid, px(tick), px(1.), window);
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// Hit box for the heading's trailing controls (`+`, archive). The
/// glyph stays 12px; this is the clickable square so a miss does not
/// fold the project. Fits the 30px heading pill.
const HEAD_CONTROL: f32 = 28.;

fn nested_row(
    id: impl Into<gpui::ElementId>,
    group: &'static str,
    selected: bool,
    depth: u8,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .group(group)
        .h(px(ROW_PILL))
        .ml(px(tree_inset(depth)))
        .mr(px(root::SIDEBAR_GUTTER))
        .px(px(root::SIDEBAR_GUTTER))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(ROW_GAP))
        .rounded(px(Theme::control_radius()))
        .cursor_pointer()
        .when(selected, |el| el.bg(theme.element_active))
        // Only off the open row: the hover wash is the weaker rung, and
        // painting it over the selection would dim what the pointer is on.
        .when(!selected, |el| el.hover(|el| el.bg(theme.element_hover)))
}

/// The inset [`Buttons::control_group`] holds its controls at, mirrored here
/// because the height below is measured from it and bezel keeps it private.
const CLUSTER_PAD: f32 = 2.;

/// The floating cluster's height, half of which is the pill's radius: a ghost
/// button's box — a 14pt glyph in 4pt of padding — inside that inset.
const CLUSTER_HEIGHT: f32 = 14. + 2. * 4. + 2. * CLUSTER_PAD;

/// A row's name. Agents use body; children use caption so the step down
/// the tree is a size you can see, not only an indent.
///
/// The line height is what the field pins itself to: left to gpui's default
/// the label's box is φ×13, and renaming would resize the row under the
/// name being typed.
fn row_label(name: String, tint: Hsla, nested: bool) -> AnyElement {
    let style = if nested {
        TextStyle::Caption
    } else {
        TextStyle::Body
    };
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_style(style)
        .line_height(px(if nested { style.line_height() } else { 18. }))
        .text_color(tint)
        .child(name)
        .into_any_element()
}

/// The heading of the project whose entries are under the scroll, held at the
/// top of the list while they pass beneath it.
///
/// A decoration rather than a child of the column, because this is the one
/// place the scroll offset for the frame being drawn is known. Read off the
/// handle in `render` it would be the offset of the frame before, and the
/// heading would lag the rows it belongs to by one.
struct PinnedHead(Entity<Cydonia>);

impl UniformListDecoration for PinnedHead {
    fn compute(
        &self,
        visible: Range<usize>,
        _bounds: Bounds<Pixels>,
        scroll: Point<Pixels>,
        item_height: Pixels,
        _count: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.0.update(cx, |this, cx| {
            this.pinned_head(visible.start, scroll.y, item_height, cx)
        })
    }
}

impl Cydonia {
    pub(crate) fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = Theme::of(cx).clone();
        let lines = self.lines(cx);
        let count = lines.len();
        div()
            .flex_none()
            .w(px(self.sidebar_width))
            .h_full()
            .bg(root::sidebar_bg(&theme))
            // Drawn ON the column, not left as a gap between two: a bare strip
            // between them would be raw desktop at full strength, a bright line
            // the height of the window.
            .border_r_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            // The fold out at the trailing edge: the lights float in the
            // leading half of the strip, which is what leaves nothing there to
            // pad them clear of.
            .child(
                div()
                    .flex_none()
                    .h(px(root::HEADER_HEIGHT))
                    .pr(px(8.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .child(self.fold_toggle(theme.text_faint, cx)),
            )
            .child(self.quick_actions(&theme, cx))
            .child(
                div()
                    .id("project-drop")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .on_drop(cx.listener(|this, drag: &ProjectDrag, _, cx| {
                        this.drop_project(drag.0, cx);
                    }))
                    .on_drop(cx.listener(|this, drag: &SessionDrag, _, cx| {
                        this.drop_session(drag.id, cx);
                    }))
                    .child(
                        uniform_list(
                            "project-list",
                            count,
                            cx.processor(move |this, range: Range<usize>, _, cx| {
                                range
                                    .filter_map(|ix| {
                                        lines
                                            .get(ix)
                                            .copied()
                                            .map(|(row, guide)| this.sidebar_row(row, guide, cx))
                                    })
                                    .collect()
                            }),
                        )
                        .track_scroll(&self.rail)
                        .with_decoration(PinnedHead(cx.entity()))
                        .size_full()
                        .on_drop(cx.listener(|this, drag: &ProjectDrag, _, cx| {
                            this.drop_project(drag.0, cx);
                        }))
                        .on_drop(cx.listener(
                            |this, drag: &SessionDrag, _, cx| {
                                this.drop_session(drag.id, cx);
                            },
                        )),
                    )
                    .children(self.drop_line(cx))
                    .children(self.session_drop_line(cx)),
            )
            .child(
                div()
                    .flex_none()
                    .mx(px(8.))
                    .mb(px(8.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        theme
                            .ghost("settings")
                            .px(px(8.))
                            .py(px(6.))
                            // The mark alone, like the folder on this line.
                            // What it opens is said in the tooltip.
                            .tooltip(|window, cx| {
                                Tooltip::with_keystroke("Settings", "⌘,", window, cx)
                            })
                            .child(
                                icons::icon(icons::system::SETTINGS_MINIMALISTIC)
                                    .size(px(14.))
                                    .text_color(theme.text),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_settings(Section::General, cx)
                            })),
                    )
                    .child(
                        theme
                            .ghost("open-project")
                            .px(px(8.))
                            .py(px(6.))
                            .tooltip(|window, cx| {
                                Tooltip::with_keystroke("Open project", "⌘O", window, cx)
                            })
                            .child(
                                icons::icon(icons::files::FOLDER)
                                    .size(px(14.))
                                    .text_color(theme.text),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_project_action(&OpenProject, window, cx);
                            })),
                    ),
            )
    }

    /// The fold toggle once the sidebar is away, as a glass pill over the
    /// content. Out of flow and hugging the one control it holds: a band would
    /// take a row off every pane to carry a single button, and the column under
    /// it is what the button is for. Only the fold — adding a project acts on
    /// the list you are looking at, and with the list gone it is chrome for
    /// somewhere you are not. Its tone is the strong one, because the plate
    /// floats over whatever the pane shows, which can be a picture we did not
    /// choose.
    pub(crate) fn fold_cluster(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        theme
            .control_group()
            .absolute()
            .top(px((root::HEADER_HEIGHT - CLUSTER_HEIGHT) / 2.))
            // Full screen takes the lights away, and the room they needed
            // would be left as a hole. Simple fullscreen is the same chrome.
            .left(px(
                if window.is_fullscreen() || window.is_simple_fullscreen() {
                    root::HEADER_INSET
                } else {
                    root::TOOLBAR_INSET
                },
            ))
            .h(px(CLUSTER_HEIGHT))
            // A pill, where the group's own corner is cut for a row of square
            // buttons. Before the glass, which reads the corners off the box.
            .rounded(px(CLUSTER_HEIGHT / 2.))
            .items_center()
            .child(self.fold_toggle(theme.text, cx))
            // The same glass bezel's own floating bar mounts on. Its
            // `control_bar` is the shipped container, and it refuses this case
            // on purpose: a fixed 56pt tall, and sized by its caller rather
            // than by what it holds.
            .surface(&theme, theme.popover_surface)
            .into_any_element()
    }

    /// The control that folds the sidebar away and brings it back. It belongs
    /// to whichever column runs along the window's left edge, so it changes
    /// strip across the collapse — and takes that strip's tone with it.
    fn fold_toggle(&self, tint: Hsla, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = Theme::of(cx).clone();
        let label = if self.sidebar_open {
            "Hide sidebar"
        } else {
            "Show sidebar"
        };
        theme
            .ghost("toggle-sidebar")
            .p(px(4.))
            .tooltip(move |window, cx| Tooltip::text(label, window, cx))
            .child(
                icons::icon(icons::system::SIDEBAR_MINIMALISTIC_LEFT)
                    .size(px(14.))
                    .text_color(tint),
            )
            .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)))
    }

    /// The heading held at the top of the list, and where to hold it.
    ///
    /// Measured in the list's own space — the decoration is laid out over the
    /// whole run of rows, so `y` here is counted from the first of them rather
    /// than from the top of what is on screen.
    fn pinned_head(
        &self,
        first: usize,
        scroll: Pixels,
        item_height: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = self.rows(cx);
        let head = |row: &Row| matches!(row, Row::Project(_));
        let Some(at) = rows
            .get(..=first)
            .and_then(|above| above.iter().rposition(head))
        else {
            return Empty.into_any_element();
        };
        let Row::Project(ix) = rows[at] else {
            return Empty.into_any_element();
        };
        // The next heading pushes this one out rather than sliding under it,
        // which is what keeps two of them from ever reading as one block.
        let next = rows[at + 1..]
            .iter()
            .position(head)
            .map(|after| item_height * (at + 1 + after) - item_height);
        let rest = item_height * at;
        let y = next.map_or(-scroll, |limit| (-scroll).min(limit));
        // Above its own place there is nothing to hold: the row itself is on
        // screen, in the list, where it belongs.
        if y <= rest {
            return Empty.into_any_element();
        }
        div()
            .size_full()
            .relative()
            .child(
                // Stateful, and so an id scope of its own: the copy inside
                // carries the same ids as the row it stands for.
                //
                // The band the glass is seen in, and what clips it to one row.
                div()
                    .id("pinned-head")
                    .absolute()
                    .top(y)
                    .left_0()
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .overflow_hidden()
                    .child(self.project_head(ix, true, cx)),
            )
            .into_any_element()
    }

    /// One project's heading: the name, and on hover a fold chevron,
    /// an archive toggle when the project has put chats away, and a
    /// `+` that opens a chat.
    ///
    /// `pinned` is the copy [`Cydonia::pinned_head`] holds at the top of the
    /// list. It gives up the pill for the column's full width, and takes the
    /// glass the floating cluster below it is cut from — a heading with rows
    /// running under it has to be read against whatever is passing.
    /// Cursor's sidebar opens with a short list of verbs — New Chat, then
    /// what the app can start — and a faint "Repositories" label over the
    /// folders. The verbs are the same actions ⌘N and ⌘O run.
    fn quick_actions(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let row = |id: &'static str, icon: &'static str, label: &'static str, theme: &Theme| {
            div()
                .id(id)
                .h(px(ROW_PILL))
                .mx(px(8.))
                .px(px(6.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .rounded(px(Theme::control_radius()))
                .cursor_pointer()
                .text_style(TextStyle::Body)
                .text_color(theme.text_muted)
                .hover(|el| el.bg(theme.element_hover).text_color(theme.text))
                .child(
                    icons::icon(icon)
                        .size(px(14.))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .child(label)
        };
        div()
            .flex_none()
            .flex()
            .flex_col()
            .pb(px(2.))
            .child(
                row("quick-new-chat", icons::editing::PEN, "New Chat", theme).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.new_session_action(&root::NewSession, window, cx)
                    }),
                ),
            )
            .child(
                row(
                    "quick-open-folder",
                    icons::files::FOLDER_WITH_FILES,
                    "Open Folder",
                    theme,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_project_action(&root::OpenProject, window, cx)
                })),
            )
            .child(
                div()
                    .h(px(22.))
                    .mt(px(8.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child("Repositories"),
            )
            .into_any_element()
    }

    fn project_head(&self, ix: usize, pinned: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let (name, key, expanded, archive_open, has_archived) =
            match self.workspace.read(cx).projects.get(ix) {
                Some(project) => (
                    project.name(),
                    project.place().encode(),
                    project.expanded,
                    project.archive_open,
                    project.has_archived(),
                ),
                None => return Empty.into_any_element(),
            };
        let hovered = self.hovering_head == Some((ix, pinned));
        let naming = self.renaming == Some(Renaming::Project(key.clone()));
        let carried = SharedString::from(name.clone());
        let title = if naming {
            self.name_field(cx)
        } else {
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_style(TextStyle::Body)
                .child(name)
                .into_any_element()
        };
        let head = div()
            .id(("project", ix))
            .group("project-head")
            // Pinned it runs edge to edge, and past the band it shows in at
            // the top and the bottom — see [`PINNED_BLEED`]. The label keeps
            // the x the pill's own margin and padding put it at.
            .when(pinned, |el| {
                el.absolute()
                    .top(px(-PINNED_BLEED))
                    .left_0()
                    .right_0()
                    .h(px(ROW_HEIGHT + 2. * PINNED_BLEED))
                    .px(px(8. + 6.))
            })
            .when(!pinned, |el| {
                el.relative()
                    .mx(px(8.))
                    .px(px(6.))
                    .h(px(ROW_PILL))
                    .rounded(px(Theme::control_radius()))
            })
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .cursor_pointer()
            // On the head, not the label: a name's colour is fixed when
            // its text is laid out, and only this div is stateful enough
            // to carry the hover that far.
            .text_color(theme.text)
            .when(pinned, |el| el.hover(|el| el.text_color(theme.text)))
            .when(!pinned, |el| {
                el.hover(|el| el.text_color(theme.text).bg(theme.element_hover))
            })
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let next = hovered.then_some((ix, pinned));
                if *hovered {
                    if this.hovering_head != next {
                        this.hovering_head = next;
                        cx.notify();
                    }
                } else if this.hovering_head == Some((ix, pinned)) {
                    this.hovering_head = None;
                    cx.notify();
                }
            }))
            .child(self.project_mark(ix, expanded, hovered, &theme, cx))
            .child(title)
            .child(self.project_archive(ix, hovered, archive_open, has_archived, &theme, cx))
            .child(self.project_add(ix, hovered, &theme, cx))
            .children(self.project_menu(ix, cx))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| this.toggle_menu(Menu::Project(ix), cx)),
            )
            // The name folds the chats, same as the hover chevron.
            // Expanding also focuses the project. A press on the pinned
            // copy is a press on where it came from: the list goes back
            // to the heading, rather than folding away what you are reading.
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.click_count() >= 2 {
                    this.rename_project(ix, window, cx);
                    return;
                }
                if this.renaming == Some(Renaming::Project(key.clone())) {
                    return;
                }
                this.open_project(ix, pinned, cx);
            }))
            // Carried by its heading, and dropped on the heading it is to sit
            // in front of. Nothing else in the column is draggable: what the
            // entries are ordered by is when they were last written.
            .on_drag(ProjectDrag(ix), move |_, _, _, cx| {
                let carried = carried.clone();
                cx.new(|_| Carried(carried))
            })
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ProjectDrag>, _, cx| {
                    this.track_project_drag(event.event.position.y, cx);
                }),
            )
            .on_drop(cx.listener(|this, drag: &ProjectDrag, _, cx| {
                this.drop_project(drag.0, cx);
            }));
        // Its own menu opens on the press rather than the click, so the note
        // has to be here too — read stale, a right press would swallow.
        let head = self.menu_press(head, Menu::Project(ix), cx);
        match pinned {
            // The same token the cluster at the foot of the column mounts on,
            // so the two glasses in the sidebar move together.
            true => head
                .surface(&theme, theme.popover_surface)
                .into_any_element(),
            false => head.into_any_element(),
        }
    }

    /// Left of the name: empty until hover, then the fold chevron.
    /// The box stays so the name does not jump.
    fn project_mark(
        &self,
        ix: usize,
        expanded: bool,
        hovered: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mark = div()
            .id(("project-mark", ix))
            .flex_none()
            .size(px(14.))
            .flex()
            .items_center()
            .justify_center();
        if hovered {
            return mark
                .child(theme.disclosure(expanded))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_project(ix, cx);
                }))
                .into_any_element();
        }
        // At rest, Cursor's repository rows lead with an open folder.
        mark.child(
            icons::icon(if expanded {
                icons::files::FOLDER_WITH_FILES
            } else {
                icons::files::FOLDER
            })
            .size(px(13.))
            .text_color(theme.text_muted),
        )
        .into_any_element()
    }

    /// Trailing `+`. Same box when idle so the name does not jump.
    fn project_add(
        &self,
        ix: usize,
        hovered: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !hovered {
            return div().flex_none().size(px(HEAD_CONTROL)).into_any_element();
        }
        theme
            .ghost(("project-add", ix))
            .flex_none()
            .size(px(HEAD_CONTROL))
            .flex()
            .items_center()
            .justify_center()
            .tooltip(|window, cx| Tooltip::with_keystroke("New session", "⌘N", window, cx))
            .child(
                icons::icon(icons::system::PLUS)
                    .size(px(12.))
                    .text_color(theme.text),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.select_project(ix, cx);
                this.new_session_action(&NewSession, window, cx);
            }))
            .into_any_element()
    }

    /// Archive tray next to `+`. Hidden until this project has put a
    /// chat away. On hover like `+`; stays up while the list is shown
    /// so it can be turned off. A drop here archives. The click does
    /// not fold the project and does not open a chat.
    fn project_archive(
        &self,
        ix: usize,
        hovered: bool,
        archive_open: bool,
        has_archived: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !has_archived {
            return Empty.into_any_element();
        }
        let show = hovered || archive_open || cx.has_active_drag();
        let tint = if archive_open {
            theme.text
        } else {
            theme.text_muted
        };
        let label = if archive_open {
            "Hide archived"
        } else {
            "Show archived"
        };
        theme
            .ghost(("project-archive", ix))
            .flex_none()
            .size(px(HEAD_CONTROL))
            .flex()
            .items_center()
            .justify_center()
            .when(archive_open, |el| el.bg(theme.element_active))
            .tooltip(move |window, cx| Tooltip::text(label, window, cx))
            .child(if show {
                icons::icon(icons::files::ARCHIVE_MINIMALISTIC)
                    .size(px(12.))
                    .text_color(tint)
                    .into_any_element()
            } else {
                div().size(px(12.)).into_any_element()
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_archive(ix, cx);
            }))
            .on_drop(cx.listener(move |this, drag: &SessionDrag, _, cx| {
                this.drop_session_on_archive(drag.id, ix, cx);
            }))
            .into_any_element()
    }

    /// Everything open in one project — the lines the sidebar draws under
    /// its head. A keyboard step walks every project's entries as one list.
    ///
    /// Roots stay in the user's rank. Archived rows sink. A running turn
    /// does not move anyone.
    pub(crate) fn entries(&self, project: usize, cx: &App) -> Vec<(Row, Guide)> {
        let workspace = self.workspace.read(cx);
        let Some(open) = workspace.projects.get(project) else {
            return Vec::new();
        };
        let mut entries: Vec<(bool, i64, u64, Row)> = open
            .roots()
            .map(|chat| {
                (
                    chat.closed,
                    chat.rank,
                    chat.id,
                    Row::Session {
                        project,
                        id: chat.id,
                        nested: false,
                    },
                )
            })
            .collect();
        entries.sort_by_key(|(archived, rank, id, _)| (*archived, *rank, *id));
        let split = entries.iter().position(|(archived, ..)| *archived);
        let live = split.unwrap_or(entries.len());
        // Closed roots, only while the header toggle is on. No
        // Archived label — that read as a chat.
        let archived = split.map(|at| &entries[at..]).unwrap_or(&[]);
        let shown = if open.archive_open {
            live + archived.len()
        } else {
            live
        };
        let mut rows: Vec<(Row, Guide)> = Vec::new();
        for (n, (_, _, _, row)) in entries[..shown].iter().enumerate() {
            let guide = Guide {
                depth: 0,
                last: n + 1 == shown,
                trunks: 0,
            };
            rows.push((*row, guide));
            if let Row::Session { id, .. } = *row {
                self.push_children(&mut rows, project, id, guide, open);
            }
        }
        rows
    }

    /// Every open child under `parent`, then their children. The folder
    /// listing is the tree.
    fn push_children(
        &self,
        rows: &mut Vec<(Row, Guide)>,
        project: usize,
        parent: u64,
        guide: Guide,
        open: &crate::model::project::Project,
    ) {
        let children = open.children(parent);
        let count = children.len();
        for (n, child) in children.into_iter().enumerate() {
            let guide = guide.child(n, count);
            match child {
                Child::Agent(id) => {
                    rows.push((
                        Row::Session {
                            project,
                            id,
                            nested: true,
                        },
                        guide,
                    ));
                    self.push_children(rows, project, id, guide, open);
                }
                Child::Surface(id) => rows.push((Row::Surface { project, id }, guide)),
            }
        }
    }

    /// Every line the sidebar shows, in order, with where each stands in the
    /// tree. Addresses only: a project with a thousand articles costs a
    /// thousand `Row`s here and reads a title for none of them.
    pub(crate) fn lines(&self, cx: &App) -> Vec<(Row, Guide)> {
        let mut rows = Vec::new();
        for p in 0..self.workspace.read(cx).projects.len() {
            rows.push((Row::Project(p), Guide::default()));
            if self.workspace.read(cx).projects[p].expanded {
                rows.extend(self.entries(p, cx));
            }
        }
        rows
    }

    /// [`Self::lines`] without the tree geometry — what a keyboard step walks.
    pub(crate) fn rows(&self, cx: &App) -> Vec<Row> {
        self.lines(cx).into_iter().map(|(row, _)| row).collect()
    }

    /// Open what a row points at — what a keyboard step does with its landing.
    /// The pointer never comes through here: each row carries its own
    /// `on_click`, which needs no [`Row`] to know what it is.
    pub(crate) fn open_row(&mut self, row: Row, _window: &mut Window, cx: &mut Context<Self>) {
        match row {
            Row::Project(ix) => self.select_project(ix, cx),
            Row::Session { id, .. } => self.select_session(id, cx),
            Row::Surface { id, .. } => self.select_surface(id, cx),
        }
    }

    /// Take the list back to where a project starts, heading and all.
    fn scroll_to_project(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(at) = self
            .rows(cx)
            .iter()
            .position(|row| *row == Row::Project(ix))
        else {
            return;
        };
        self.rail.scroll_to_item(at, ScrollStrategy::Top);
        cx.notify();
    }

    /// Scroll the rail to a row, if it is not already on screen. `Nearest`
    /// rather than `Top`: a step to the neighbour below should move the list by
    /// a row, not throw the one you came from off the top of it.
    pub(crate) fn reveal(&mut self, row: Row, cx: &Context<Self>) {
        if let Some(ix) = self.rows(cx).iter().position(|at| *at == row) {
            self.rail.scroll_to_item(ix, ScrollStrategy::Nearest);
        }
    }

    /// One line, built when the list scrolls it into view. The box around it is
    /// what holds the pitch: the row inside paints the wash, and the pixel
    /// either side of it is the gap between two.
    fn sidebar_row(&self, row: Row, guide: Guide, cx: &mut Context<Self>) -> AnyElement {
        let inner = match row {
            Row::Project(ix) => self.project_head(ix, false, cx),
            Row::Session { project, id, .. } => match self.session_of(project, id, cx) {
                Some(session) => self.session_row(session, guide.depth, cx),
                None => Empty.into_any_element(),
            },
            Row::Surface { project, id } => self.surface_row(project, id, guide.depth, cx),
        };
        // The connector runs under the pill, across the row's whole pitch,
        // so one row's trunk meets the next.
        // Cursor draws no tree under a repository; the guide starts where
        // a chat has children of its own.
        let lines = (!matches!(row, Row::Project(_)) && guide.depth > 1)
            .then(|| tree_lines(guide, Theme::of(cx).border_strong));
        div()
            .h(px(ROW_HEIGHT))
            .py(px(1.))
            .relative()
            .children(lines)
            .on_drop(cx.listener(|this, drag: &ProjectDrag, _, cx| {
                this.drop_project(drag.0, cx);
            }))
            .on_drop(cx.listener(|this, drag: &SessionDrag, _, cx| {
                this.drop_session(drag.id, cx);
            }))
            .child(inner)
            .into_any_element()
    }

    /// What the sidebar needs of a session, read when its row comes on screen.
    fn session_of(&self, project: usize, id: u64, cx: &Context<Self>) -> Option<SessionRow> {
        let workspace = self.workspace.read(cx);
        let chat = workspace.projects.get(project)?.session(id)?;
        Some(SessionRow {
            id: chat.id,
            label: workspace.display_label(chat.id),
            icon: if chat.is_delegate() {
                Some(crate::assets::DELEGATE_ICON.into())
            } else {
                workspace.agent_icon(&chat.entry.name)
            },
            delegate: chat.is_delegate(),
            working: chat.busy().then(|| chat.elapsed().unwrap_or_default()),
            archived: chat.closed,
            standing: chat.plan_open().any(|n| n.standing),
            asking: chat.plan_open().any(|n| n.do_kind == "ask"),
            updated: chat.updated,
        })
    }

    fn toggle_archive(&mut self, project: usize, cx: &mut Context<Self>) {
        self.workspace.update(cx, |workspace, cx| {
            if let Some(open) = workspace.projects.get_mut(project) {
                open.archive_open = !open.archive_open;
            }
            cx.notify();
        });
    }

    /// The header toggle is the old Archived drop slot: land here and
    /// the chat is put away, and the list opens so the landing shows.
    fn drop_session_on_archive(&mut self, from: u64, project: usize, cx: &mut Context<Self>) {
        self.session_drop = None;
        self.menu = None;
        let belongs = self
            .workspace
            .read(cx)
            .projects
            .get(project)
            .is_some_and(|open| open.session(from).is_some());
        if !belongs {
            return;
        }
        let before = self
            .workspace
            .read(cx)
            .projects
            .get(project)
            .and_then(first_archived_root);
        self.workspace.update(cx, |workspace, cx| {
            if let Some(open) = workspace.projects.get_mut(project) {
                open.expanded = true;
            }
            workspace.place_session(from, before, true, cx);
        });
    }

    /// Menus address a project by its place in the list, so the one open when
    /// it moves would be pointing at whichever project slid underneath.
    fn item_height(&self) -> f32 {
        self.rail
            .0
            .borrow()
            .last_item_size
            .map(|size| f32::from(size.item.height))
            .filter(|height| *height > 0.)
            .unwrap_or(ROW_HEIGHT)
    }

    fn track_project_drag(&mut self, pointer_y: Pixels, cx: &mut Context<Self>) {
        let bounds = self.rail.0.borrow().base_handle.bounds();
        if bounds.size.height <= px(0.) {
            return;
        }
        let local = f32::from(pointer_y - bounds.origin.y);
        let scroll = f32::from(self.rail.0.borrow().base_handle.offset().y);
        let slot = self.project_slot(local - scroll, cx);
        if self.drop_slot != Some(slot) {
            self.drop_slot = Some(slot);
            cx.notify();
        }
    }

    /// Which gap the pointer is over. `0` is above the first project, `n` is
    /// below the last — including the empty space under the list.
    fn project_slot(&self, content_y: f32, cx: &App) -> usize {
        let n = self.workspace.read(cx).projects.len();
        if n == 0 {
            return 0;
        }
        let rows = self.rows(cx);
        let item_h = self.item_height();
        let mut slot = n;
        for (ix, row) in rows.iter().enumerate() {
            let Row::Project(project) = row else {
                continue;
            };
            let mid = (ix as f32 + 0.5) * item_h;
            if content_y < mid {
                return *project;
            }
            slot = *project + 1;
        }
        slot
    }

    fn drop_line(&self, cx: &App) -> Option<AnyElement> {
        if !cx.has_active_drag() {
            return None;
        }
        let slot = self.drop_slot?;
        let theme = Theme::of(cx).clone();
        let rows = self.rows(cx);
        let item_h = self.item_height();
        let content_y = rows
            .iter()
            .position(|row| matches!(row, Row::Project(project) if *project == slot))
            .map(|ix| ix as f32 * item_h)
            .unwrap_or(rows.len() as f32 * item_h);
        let y = content_y + f32::from(self.rail.0.borrow().base_handle.offset().y);
        Some(
            div()
                .absolute()
                .top(px(y - 1.))
                .left(px(10.))
                .right(px(10.))
                .h(px(2.))
                .rounded(px(1.))
                .bg(theme.text)
                .into_any_element(),
        )
    }

    fn drop_project(&mut self, from: usize, cx: &mut Context<Self>) {
        let Some(to) = self.drop_slot.take() else {
            return;
        };
        self.menu = None;
        self.workspace
            .update(cx, |workspace, cx| workspace.move_project(from, to, cx));
    }

    fn track_session_drag(&mut self, from: u64, pointer_y: Pixels, cx: &mut Context<Self>) {
        let bounds = self.rail.0.borrow().base_handle.bounds();
        if bounds.size.height <= px(0.) {
            return;
        }
        let local = f32::from(pointer_y - bounds.origin.y);
        let scroll = f32::from(self.rail.0.borrow().base_handle.offset().y);
        let slot = self.session_slot(from, local - scroll, cx);
        if self.session_drop != slot {
            self.session_drop = slot;
            cx.notify();
        }
    }

    /// Which gap the pointer is over. Roots can land in either list:
    /// among open chats, or among archived rows while the header
    /// toggle is on. Dropping on the header toggle archives — that
    /// slot is not a list row. Nested chats stay among their siblings.
    /// Top half of a row is before it; bottom half is after. A pointer
    /// over another project, or off the list, is no slot.
    fn session_slot(&self, from: u64, content_y: f32, cx: &App) -> Option<SessionDrop> {
        let workspace = self.workspace.read(cx);
        let project_ix = workspace
            .projects
            .iter()
            .position(|project| project.session(from).is_some())?;
        let moved = workspace.projects.get(project_ix)?.session(from)?;
        let parent = moved.parent;
        let closed = moved.closed;
        let nested = parent.is_some();
        let last_project = project_ix + 1 == workspace.projects.len();

        let rows = self.rows(cx);
        let item_h = self.item_height();
        let start = rows
            .iter()
            .position(|row| matches!(row, Row::Project(p) if *p == project_ix))?;
        let end = rows[start + 1..]
            .iter()
            .position(|row| matches!(row, Row::Project(_)))
            .map(|i| start + 1 + i)
            .unwrap_or(rows.len());
        let top = start as f32 * item_h;
        let bottom = end as f32 * item_h;
        let in_project = content_y >= top && content_y < bottom;
        let in_tail = last_project && content_y >= bottom;
        if !in_project && !in_tail {
            return None;
        }

        if nested {
            return self.session_slot_siblings(
                parent, closed, content_y, start, end, item_h, &rows, project_ix, &workspace,
            );
        }

        let project = workspace.projects.get(project_ix)?;
        let mut opens: Vec<(usize, u64)> = Vec::new();
        let mut archived: Vec<(usize, u64)> = Vec::new();
        for (offset, row) in rows[start..end].iter().enumerate() {
            let ix = start + offset;
            match row {
                Row::Session {
                    id,
                    project: p,
                    nested: is_child,
                } if *p == project_ix && !*is_child => {
                    let Some(chat) = project.session(*id) else {
                        continue;
                    };
                    if chat.parent.is_some() {
                        continue;
                    }
                    if chat.closed {
                        archived.push((ix, *id));
                    } else {
                        opens.push((ix, *id));
                    }
                }
                _ => {}
            }
        }

        let archive_at = archived.first().map(|(ix, _)| *ix);
        let into_archive = archive_at.is_some_and(|ix| content_y >= ix as f32 * item_h);
        if into_archive {
            for (ix, id) in &archived {
                let mid = (*ix as f32 + 0.5) * item_h;
                if content_y < mid {
                    return Some(SessionDrop {
                        before: Some(*id),
                        at: *ix,
                        nested: false,
                        archived: true,
                    });
                }
            }
            return Some(SessionDrop {
                before: None,
                at: archived.last().map(|(ix, _)| *ix + 1).unwrap_or(end),
                nested: false,
                archived: true,
            });
        }

        for (ix, id) in &opens {
            let mid = (*ix as f32 + 0.5) * item_h;
            if content_y < mid {
                return Some(SessionDrop {
                    before: Some(*id),
                    at: *ix,
                    nested: false,
                    archived: false,
                });
            }
        }
        Some(SessionDrop {
            before: None,
            at: archive_at
                .or_else(|| opens.last().map(|(ix, _)| *ix + 1))
                .unwrap_or(end),
            nested: false,
            archived: false,
        })
    }

    /// Same-list slots for a nested chat. It cannot cross into or
    /// out of the archived list.
    fn session_slot_siblings(
        &self,
        parent: Option<u64>,
        closed: bool,
        content_y: f32,
        start: usize,
        end: usize,
        item_h: f32,
        rows: &[Row],
        project_ix: usize,
        workspace: &crate::model::workspace::Workspace,
    ) -> Option<SessionDrop> {
        let mut candidates: Vec<(usize, Option<u64>)> = Vec::new();
        for (offset, row) in rows[start..end].iter().enumerate() {
            let ix = start + offset;
            match row {
                Row::Session { id, project, .. } if *project == project_ix => {
                    let Some(chat) = workspace.projects[project_ix].session(*id) else {
                        continue;
                    };
                    if chat.parent == parent && chat.closed == closed {
                        candidates.push((ix, Some(*id)));
                    }
                }
                _ => {}
            }
        }
        for (ix, before) in &candidates {
            let mid = (*ix as f32 + 0.5) * item_h;
            if content_y < mid {
                return Some(SessionDrop {
                    before: *before,
                    at: *ix,
                    nested: true,
                    archived: closed,
                });
            }
        }
        let at = match candidates.last() {
            Some((ix, _)) => *ix + 1,
            None => end,
        };
        Some(SessionDrop {
            before: None,
            at,
            nested: true,
            archived: closed,
        })
    }

    fn session_drop_line(&self, cx: &App) -> Option<AnyElement> {
        if !cx.has_active_drag() {
            return None;
        }
        let slot = self.session_drop?;
        let theme = Theme::of(cx).clone();
        let item_h = self.item_height();
        let content_y = slot.at as f32 * item_h;
        let y = content_y + f32::from(self.rail.0.borrow().base_handle.offset().y);
        // The same inset the rows sit at.
        let left = tree_inset(if slot.nested { 1 } else { 0 });
        Some(
            div()
                .absolute()
                .top(px(y))
                .left(px(left))
                .right(px(root::SIDEBAR_GUTTER))
                .h(px(1.))
                .bg(theme.text)
                .into_any_element(),
        )
    }

    fn drop_session(&mut self, from: u64, cx: &mut Context<Self>) {
        let Some(slot) = self.session_drop.take() else {
            return;
        };
        self.menu = None;
        self.workspace.update(cx, |workspace, cx| {
            workspace.place_session(from, slot.before, slot.archived, cx);
        });
    }

    /// Fold or unfold the chats under this heading. The hover
    /// chevron comes through here; the name uses [`Self::open_project`].
    fn toggle_project(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.workspace.update(cx, |workspace, cx| {
            if let Some(project) = workspace.projects.get_mut(ix) {
                project.expanded = !project.expanded;
            }
            cx.notify();
        });
    }

    /// Focus this project. The list heading toggles the chats under
    /// it — expand and select, or collapse and leave it in front.
    /// The pinned copy always opens and scrolls back to the heading.
    fn open_project(&mut self, ix: usize, pinned: bool, cx: &mut Context<Self>) {
        self.workspace.update(cx, |workspace, cx| {
            if let Some(project) = workspace.projects.get_mut(ix) {
                if pinned {
                    project.expanded = true;
                } else {
                    project.expanded = !project.expanded;
                }
            }
            workspace.select_project(ix, cx);
        });
        if pinned {
            self.scroll_to_project(ix, cx);
        }
    }

    /// What a press on the heading opens. Removing closes the tab — the
    /// directory and everything in it stays where it is.
    fn project_menu(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.menu != Some(Menu::Project(ix)) {
            return None;
        }
        let rows = vec![
            menu::row(
                Item::action("Rename").with_icon(icons::editing::PEN_NEW_SQUARE),
                move |this, window, cx| this.rename_project(ix, window, cx),
            ),
            menu::row(
                Item::action("Remove project").with_icon(icons::files::TRASH_BIN_MINIMALISTIC),
                move |this, _, cx| this.close_project(ix, cx),
            ),
        ];
        let id = SharedString::from(format!("project-menu-{ix}"));
        Some(popover::anchored_menu_below(
            id.clone(),
            self.menu_card(id, rows, cx),
            None,
        ))
    }

    /// One session: its mark and its name.
    fn session_row(&self, session: SessionRow, depth: u8, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let nested = depth > 0;
        let id = session.id;
        let archived = session.archived;
        let workspace = self.workspace.read(cx);
        let selected = self.showing(cx) == Some(Pane::Chat)
            && workspace.active_id() == Some(id)
            && workspace
                .active_project()
                .and_then(|project| project.focus)
                .is_some_and(|focus| focus.surface.is_none());
        let naming = self.renaming == Some(Renaming::Session(id))
            && !self.rename_heading
            && !self.rename_in_header;
        let tint = tint(selected, archived, &theme);
        // The agent's own mark, in the label's colour rather than any of its
        // own: every icon the registry publishes is a `currentColor` glyph, so
        // tinting is the only colour it will ever have. While a turn is in
        // flight a braille spinner stands in its place.
        let mark_size = px(ROW_MARK);
        let mark = if let Some(since) = session.working.filter(|_| !session.delegate) {
            transcript::spinner(since, tint, cx)
        } else {
            match session.icon {
                Some(path) => svg()
                    .path(path)
                    .size(mark_size)
                    .flex_none()
                    .text_color(tint)
                    .into_any_element(),
                None => Empty.into_any_element(),
            }
        };

        let title = SharedString::from(session.label.as_str());
        let label = match naming {
            true => self.name_field(cx),
            false => row_label(session.label, tint, nested),
        };
        let link = (!naming)
            .then(|| self.workspace.read(cx).chat_link(id))
            .flatten();

        let row = nested_row(("session", id), "session-row", selected, depth, &theme)
            .child(
                div()
                    .flex_none()
                    .size(mark_size)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(mark),
            )
            .child(label)
            .children(
                session
                    .working
                    .filter(|_| session.delegate)
                    .map(|since| transcript::spinner(since, tint, cx)),
            )
            // A question for you outranks a standing clock; one glyph only.
            .when(session.asking, |el| {
                el.child(
                    icons::icon(icons::system::CHAT_ROUND_LINE)
                        .size(px(11.))
                        .flex_none()
                        .text_color(theme.accent),
                )
            })
            .when(session.standing && !session.asking, |el| {
                el.child(
                    icons::icon(icons::media::REPEAT)
                        .size(px(11.))
                        .flex_none()
                        .text_color(theme.text_faint),
                )
            })
            // Cursor: how long ago, at the right, faint — and under the
            // pointer the age gives way to `⋯`, which opens the row's menu.
            // Live rows show the spinner instead.
            .when(!naming && session.working.is_none(), |el| {
                el.child(
                    div()
                        .flex_none()
                        .relative()
                        .h(px(18.))
                        .min_w(px(24.))
                        .flex()
                        .items_center()
                        .justify_end()
                        .child(
                            div()
                                .text_style(TextStyle::Caption)
                                .text_color(theme.text_faint)
                                .group_hover("session-row", |el| el.invisible())
                                .child(SharedString::from(age_label(session.updated))),
                        )
                        .child(
                            div()
                                .id(("session-dots", id))
                                .absolute()
                                .right_0()
                                .size(px(18.))
                                .rounded(px(4.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .invisible()
                                .group_hover("session-row", |el| el.visible())
                                .hover(|el| el.bg(theme.element_active))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_menu(Menu::Session(id), cx);
                                }))
                                .child(
                                    icons::icon(icons::system::MENU_DOTS)
                                        .size(px(12.))
                                        .text_color(theme.text_muted),
                                ),
                        ),
                )
            })
            .children(self.session_menu(id, archived, cx))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| this.toggle_menu(Menu::Session(id), cx)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if event.modifiers.control {
                        cx.stop_propagation();
                        this.select_session(id, cx);
                        this.toggle_menu(Menu::Session(id), cx);
                    }
                }),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.click_count() >= 2 {
                    this.rename_session(id, window, cx);
                    return;
                }
                if this.renaming == Some(Renaming::Session(id)) {
                    return;
                }
                this.select_session(id, cx);
            }));
        let markdown: SharedString = link.unwrap_or_default().into();
        let this = cx.entity();
        let row = self.menu_press(row, Menu::Session(id), cx);
        row.on_drag(SessionDrag { id, markdown }, move |_, _, _, cx| {
            this.update(cx, |this, _| {
                this.drop_slot = None;
                this.session_drop = None;
            });
            cx.new(|_| Carried(title.clone()))
        })
        .on_drag_move(
            cx.listener(move |this, event: &DragMoveEvent<SessionDrag>, _, cx| {
                this.track_session_drag(id, event.event.position.y, cx);
            }),
        )
        .on_drop(cx.listener(move |this, drag: &SessionDrag, _, cx| {
            this.drop_session(drag.id, cx);
        }))
        .into_any_element()
    }

    fn surface_row(
        &self,
        project: usize,
        id: SurfaceId,
        depth: u8,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let workspace = self.workspace.read(cx);
        let Some(surface) = workspace
            .projects
            .get(project)
            .and_then(|open| open.surface(id))
        else {
            return Empty.into_any_element();
        };
        let selected = workspace
            .active_project()
            .and_then(|open| open.focus)
            .is_some_and(|focus| focus.surface == Some(id));
        let tint = tint(selected, false, &theme);
        let glyph = board::glyph(&surface.board_kind);
        nested_row(("surface", id.0), "surface-row", selected, depth, &theme)
            .child(
                div()
                    .flex_none()
                    .size(px(ROW_MARK))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icons::icon(glyph).size(px(ROW_MARK)).text_color(tint)),
            )
            .child(row_label(surface.title.clone(), tint, true))
            .child(self.surface_close(id, selected, &theme, cx))
            .on_click(cx.listener(move |this, _, _, cx| this.select_surface(id, cx)))
            .into_any_element()
    }

    fn surface_close(
        &self,
        id: SurfaceId,
        selected: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        theme
            .ghost(("surface-close", id.0))
            .flex_none()
            .p(px(1.))
            .when(!selected, |el| {
                el.invisible().group_hover("surface-row", |el| el.visible())
            })
            .tooltip(|window, cx| Tooltip::text("Close", window, cx))
            .child(
                icons::icon(icons::files::TRASH_BIN_MINIMALISTIC)
                    .size(px(10.))
                    .text_color(theme.text_faint),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.workspace
                    .update(cx, |workspace, cx| workspace.close_surface(id, cx));
            }))
            .into_any_element()
    }

    /// Copy, fork, archive, delete — a press on the row, not icons on it.
    /// The row's menu, unless it was opened from the chat header.
    fn session_menu(&self, id: u64, archived: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.menu_at_header {
            return None;
        }
        self.session_menu_element(id, archived, cx)
    }

    /// The chat's menu, anchored below whatever holds it.
    pub(crate) fn session_menu_element(
        &self,
        id: u64,
        archived: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.menu != Some(Menu::Session(id)) {
            return None;
        }
        let archive_label = if archived { "Unarchive" } else { "Archive" };
        let archive_icon = if archived {
            icons::files::ARCHIVE_UP_MINIMALISTIC
        } else {
            icons::files::ARCHIVE_MINIMALISTIC
        };
        let rows = vec![
            menu::row(
                Item::action("Copy").with_icon(icons::files::COPY),
                move |this, _, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| workspace.copy_chat_link(id, cx));
                },
            ),
            menu::row(
                Item::action("Fork").with_icon(icons::editing::GIT_BRANCH),
                move |this, _, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| workspace.fork_session(id, cx));
                },
            ),
            menu::row(
                Item::action(archive_label).with_icon(archive_icon),
                move |this, _, cx| {
                    this.workspace.update(cx, |workspace, cx| {
                        workspace.archive_session(id, !archived, cx);
                    });
                },
            ),
            menu::row(
                Item::action("Delete")
                    .with_icon(icons::files::TRASH_BIN_MINIMALISTIC)
                    .with_keystroke("⌫"),
                move |this, _, cx| this.delete_or_archive(id, cx),
            ),
        ];
        let key = SharedString::from(format!("session-menu-{id}"));
        Some(popover::anchored_menu_below(
            key.clone(),
            self.menu_card(key, rows, cx),
            None,
        ))
    }

    /// Open chat: put it in the archive. Archived chat: remove it.
    /// Then light the next row in that same list so Delete can fire again.
    pub(crate) fn delete_or_archive(&mut self, id: u64, cx: &mut Context<Self>) {
        let next = self.neighbor_after_remove(id, cx);
        let archived = self
            .workspace
            .read(cx)
            .session(id)
            .is_some_and(|chat| chat.closed);
        self.workspace.update(cx, |workspace, cx| {
            if archived {
                workspace.delete_session(id, cx);
            } else {
                workspace.archive_session(id, true, cx);
            }
        });
        if let Some(next) = next {
            self.select_session(next, cx);
        }
    }

    /// Who should be in front after `id` leaves its section. Next row
    /// in the same list, or the one above if it was last. Archiving the
    /// last open chat keeps that chat — it is now the archived row.
    /// Deleting the last archived chat falls back to the last open one.
    fn neighbor_after_remove(&self, id: u64, cx: &App) -> Option<u64> {
        let workspace = self.workspace.read(cx);
        let project = workspace
            .projects
            .iter()
            .find(|project| project.session(id).is_some())?;
        let chat = project.session(id)?;
        let closed = chat.closed;
        let nested = !project.is_root(chat);
        let parent = chat.parent;
        let mut ids: Vec<(i64, u64)> = project
            .sessions
            .iter()
            .filter(|sibling| {
                sibling.closed == closed
                    && if nested {
                        sibling.parent == parent
                    } else {
                        project.is_root(sibling)
                    }
            })
            .map(|sibling| (sibling.rank, sibling.id))
            .collect();
        ids.sort_by_key(|key| *key);
        let ids: Vec<u64> = ids.into_iter().map(|(_, sid)| sid).collect();
        let at = ids.iter().position(|&sid| sid == id)?;
        if let Some(&below) = ids.get(at + 1) {
            return Some(below);
        }
        if at > 0 {
            return Some(ids[at - 1]);
        }
        if nested {
            return parent.filter(|&parent| project.session(parent).is_some());
        }
        if !closed {
            return None;
        }
        let mut open: Vec<(i64, u64)> = project
            .roots()
            .filter(|chat| !chat.closed)
            .map(|chat| (chat.rank, chat.id))
            .collect();
        open.sort_by_key(|key| *key);
        open.last().map(|(_, sid)| *sid)
    }

    /// Put the name field on an entry's row, whichever kind it is. Each is
    /// addressed by what identifies it, so the field cannot slide onto its
    /// neighbour if the list reorders under it.
    fn rename_project(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self
            .workspace
            .read(cx)
            .projects
            .get(ix)
            .map(|project| project.place().encode())
        else {
            return;
        };
        self.start_rename(Renaming::Project(key), false, window, cx);
    }

    pub(crate) fn rename_session(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.start_rename(Renaming::Session(id), false, window, cx);
    }

    pub(crate) fn rename_empty_title(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_heading = true;
        self.start_rename(Renaming::Session(id), true, window, cx);
        // The heading is painted under the activating double-click. That
        // press reaches the new field and parks a caret in the word, so
        // the same-tick SelectAll in `start_rename` loses. Frame callbacks
        // run before the draw, so the first one still sees a tree without
        // the field and its SelectAll would miss too: let that frame paint
        // the field, then select on the next one and drop the flag.
        cx.on_next_frame(window, |_, window, cx| {
            cx.on_next_frame(window, |this, window, cx| {
                this.select_name_field(window, cx);
                this.select_heading = false;
            });
        });
    }

    /// Rename from the chat header: the same field, Body-sized, in the
    /// title's place on the header line. Reached by a double-click on the
    /// title, like the empty-chat heading.
    pub(crate) fn rename_header_title(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_heading = true;
        self.start_rename(Renaming::Session(id), false, window, cx);
        self.rename_in_header = true;
        // Same two-frame wait as `rename_empty_title`: the activating press
        // lands on the new field first and would park a caret in the word.
        cx.on_next_frame(window, |_, window, cx| {
            cx.on_next_frame(window, |this, window, cx| {
                this.select_name_field(window, cx);
                this.select_heading = false;
            });
        });
    }

    /// Same field as the chat header's title: Body, hugging the text, so
    /// the header line does not shift when editing starts.
    pub(crate) fn header_name_field(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = self.name_field.read(cx).content().to_string();
        let width = name_width(&text, TextStyle::Body, window, cx);
        self.name_field_frame(false, TextStyle::Body, px(18.), Some(width), cx)
    }

    /// The field, in the row's place. It carries its own press: `TextField`
    /// does not focus itself, and a press that reached the row would open what
    /// is being named out from under the name.
    pub(crate) fn name_field(&self, cx: &mut Context<Self>) -> AnyElement {
        self.name_field_frame(true, TextStyle::Body, px(18.), None, cx)
    }

    /// Same field, set as the empty-chat title: Title3, hugging the text,
    /// so the heading does not jump left or drop a size.
    pub(crate) fn heading_name_field(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = self.name_field.read(cx).content().to_string();
        let width = name_width(&text, TextStyle::Title3, window, cx);
        self.name_field_frame(
            false,
            TextStyle::Title3,
            px(TextStyle::Title3.line_height()),
            Some(width),
            cx,
        )
    }

    fn name_field_frame(
        &self,
        fill_row: bool,
        style: TextStyle,
        line: Pixels,
        width: Option<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut frame = div()
            .flex()
            .items_center()
            .text_style(style)
            .line_height(line)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    window.focus(&this.name_field.read(cx).focus_handle(cx), cx);
                    if this.select_heading {
                        this.select_heading = false;
                        window.dispatch_action(Box::new(input::SelectAll), cx);
                    }
                }),
            )
            // Pressing anywhere else is finishing, not abandoning — the name
            // typed is the name meant. `escape` is what discards.
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.commit_name(&CommitName, window, cx);
            }))
            .child(self.name_field.clone());
        if fill_row {
            frame = frame.flex_1().min_w_0();
        }
        if let Some(width) = width {
            frame = frame.w(width).max_w_full();
        }
        frame.into_any_element()
    }

    fn start_rename(
        &mut self,
        what: Renaming,
        heading: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace.read(cx);
        let label = match &what {
            Renaming::Session(id) => workspace.display_label(*id),
            Renaming::Project(key) => workspace
                .projects
                .iter()
                .find(|project| project.place().encode() == *key)
                .map(crate::model::project::Project::name)
                .unwrap_or_default(),
        };
        self.bind_name_field(heading, cx);
        self.rename_in_header = false;
        self.name_field
            .update(cx, |field, cx| field.set_content(label, cx));
        self.renaming = Some(what);
        window.focus(&self.name_field.read(cx).focus_handle(cx), cx);
        // `set_content` parks the caret at the end. Select after the
        // field is focused and in the tree, or the action misses it.
        cx.spawn_in(window, async move |this, cx| {
            let _ = this.update_in(cx, |this, window, cx| {
                this.select_name_field(window, cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn select_name_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.name_field.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        window.dispatch_action(Box::new(input::SelectAll), cx);
    }

    pub(crate) fn commit_name(&mut self, _: &CommitName, _: &mut Window, cx: &mut Context<Self>) {
        let Some(what) = self.renaming.take() else {
            return;
        };
        self.rename_heading = false;
        self.rename_in_header = false;
        self.select_heading = false;
        let name = self.name_field.read(cx).content().to_string();
        self.workspace.update(cx, |workspace, cx| match what {
            Renaming::Session(id) => workspace.rename_session(id, name, cx),
            Renaming::Project(key) => workspace.rename_project(&key, name, cx),
        });
        cx.notify();
    }

    pub(crate) fn dismiss_name(&mut self, _: &DismissName, _: &mut Window, cx: &mut Context<Self>) {
        self.renaming = None;
        self.rename_heading = false;
        self.rename_in_header = false;
        self.select_heading = false;
        cx.notify();
    }
}

/// Width of the empty-chat title, plus a sliver for the caret, so the
/// field stays where the idle heading sat.
fn name_width(text: &str, style: TextStyle, window: &mut Window, cx: &App) -> Pixels {
    let theme = Theme::of(cx);
    let shown = if text.is_empty() { " " } else { text };
    let size = px(style.painted());
    let run = TextRun {
        len: shown.len(),
        font: font(theme.font_sans.clone()),
        color: theme.text,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(shown.to_string().into(), size, &[run], None)
        .width()
        + px(2.)
}

/// The archived root that sits first by rank — the row a drop on
/// the header archive toggle sits in front of.
fn first_archived_root(project: &crate::model::project::Project) -> Option<u64> {
    project
        .roots()
        .filter(|chat| chat.closed)
        .min_by_key(|chat| (chat.rank, chat.id))
        .map(|chat| chat.id)
}
