//! The command palette (feedback `search-palette-2026-10-24`, Jacob): the
//! sidebar's Search disc, ⌘⇧F and ⌘K open one centred modal over the window
//! — Grok Bot's palette, Spotlight-shaped. A rounded dark panel: the search
//! field on top, then the results (avatar · name · subtitle · ⌘1–⌘9) in the
//! sections `crate::palette` arranges. ↑↓ move the highlight, Return opens
//! it, Esc or a click outside closes. The panel sits on the shared
//! [`crate::ui::modal`] backdrop, so the chat behind it gets no wheel, no
//! click and no key (the field owns the keyboard through `Shell::modal_focus`).
//!
//! App-wide ([feedback search-palette-app-wide-2026-10-24]): the rows come from
//! `AppState::palette_entries` — every chat, person, server, channel, agent,
//! and from the local `crate::search_index` every message, file and link on
//! this Mac, whatever chat is open.
//!
//! Geometry from `ref-grok-command-palette.png` (2 px/pt): panel 1116 px wide
//! → 558; field row 98 → 49 with a 17 pt "Search"; rows 98 tall, 4 apart,
//! inset 20 from the panel → 49 / 2 / 10, radius 24 → 12; avatar 44–48 →
//! 24; name ≈ 13.5, subtitle ≈ 12.5; shortcut badge 53 × 38 → 27 × 19,
//! 16 px → 8 from the row's right edge.

use gpui::{
    actions, div, prelude::*, px, AnyElement, Context, Div, ElementId, Focusable as _, FontWeight,
    ScrollHandle, Stateful, Window,
};

use crate::palette::{arrange, flatten, shortcut_label, Cursor, Entry, Kind, Section, Target};
use crate::search;
use crate::ui::glass;
use crate::ui::groups;
use crate::ui::highlight;
use crate::ui::identity;
use crate::ui::link_tile;
use crate::ui::modal;
use crate::ui::people;
use crate::ui::sf_symbols;
use crate::ui::shell::Shell;
use crate::ui::sidebar;
use crate::ui::theme::Theme;

actions!(
    palette,
    [Pick1, Pick2, Pick3, Pick4, Pick5, Pick6, Pick7, Pick8, Pick9]
);

/// Panel width (Grok: 558 pt).
pub const WIDTH: f32 = 558.0;
pub const RADIUS: f32 = 12.0;
/// The search row on top of the panel and its type size.
pub const FIELD_ROW: f32 = 49.0;
pub const FIELD_FONT: f32 = 17.0;
/// The magnifier in the field: 17 pt draws Grok's 12.5 pt glyph box.
pub const FIELD_GLYPH: f32 = 17.0;
/// Left inset of the magnifier in the field row.
pub const FIELD_INSET: f32 = 16.5;
/// Space between the magnifier and the typed text.
pub const FIELD_GAP: f32 = 8.0;
/// The list: inset from the panel, padding over the first row.
pub const LIST_INSET: f32 = 10.0;
pub const LIST_PAD_TOP: f32 = 8.0;
pub const LIST_PAD_BOTTOM: f32 = 10.0;
/// One result row, the gap between two, the highlight's corner.
pub const ROW_HEIGHT: f32 = 49.0;
pub const ROW_GAP: f32 = 2.0;
pub const ROW_RADIUS: f32 = 12.0;
pub const ROW_PAD_X: f32 = 7.0;
/// The picture in a row and its gap to the text.
pub const AVATAR: f32 = 24.0;
pub const AVATAR_GAP: f32 = 9.0;
pub const TITLE_FONT: f32 = 13.5;
pub const SUBTITLE_FONT: f32 = 12.5;
/// The ⌘n badge at the row's right edge.
pub const BADGE_W: f32 = 27.0;
pub const BADGE_H: f32 = 19.0;
pub const BADGE_RADIUS: f32 = 6.0;
pub const BADGE_FONT: f32 = 11.5;
pub const BADGE_INSET: f32 = 8.0;
/// Section caption ("People & agents") over a group of rows.
pub const SECTION_FONT: f32 = 11.5;
pub const SECTION_HEIGHT: f32 = 24.0;
/// Where the panel hangs: this fraction of the window height from the top
/// (at least [`MIN_TOP`]), never taller than [`MAX_HEIGHT_FRACTION`] of it.
pub const TOP_FRACTION: f32 = 0.12;
pub const MIN_TOP: f32 = 48.0;
pub const MAX_HEIGHT_FRACTION: f32 = 0.72;

/// The palette's open/closed state and highlight, owned by the shell.
#[derive(Clone, Debug, Default)]
pub struct PaletteState {
    pub open: bool,
    pub cursor: Cursor,
    /// Rows drawn by the last frame (what ↑↓ and ⌘n index into).
    pub len: usize,
    /// The query the highlight was computed for: typing resets it to the top.
    pub last_query: String,
    /// Frames left in which to scroll the highlight into view (keys, the
    /// rig): two, because GPUI clamps a scroll offset against the previous
    /// frame's content size, so the first attempt after rows appear is lost.
    pub reveal: u8,
    /// Rig (`ARBOS_PALETTE_OPEN=1`): open the highlighted row as soon as the
    /// rows are there — the "a message hit jumps to its chat" shot.
    pub open_on_rows: bool,
}

/// ⌘1–⌘9 → row index.
pub fn pick_index(action: &dyn gpui::Action) -> Option<usize> {
    let name = action.name();
    name.strip_prefix("palette::Pick")
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| (1..=9).contains(n))
        .map(|n| n - 1)
}

/// The rows for `typed`, flattened in list order.
pub(crate) fn rows_for(shell: &Shell, typed: &str, cx: &Context<Shell>) -> Vec<Entry> {
    let query = search::normalize_query(typed);
    let entries = shell.state.read(cx).palette_entries(&query);
    flatten(&arrange(entries, query.is_empty()))
        .into_iter()
        .cloned()
        .collect()
}

/// Draw the open palette over the shell root (`None` while closed).
pub fn overlay(
    shell: &mut Shell,
    scroll: &ScrollHandle,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Option<AnyElement> {
    if !shell.palette.open {
        return None;
    }
    let theme = Theme::of(cx).clone();
    let typed = shell.search_input.read(cx).text().to_string();
    let query = search::normalize_query(&typed);
    let arranged = {
        let entries = shell.state.read(cx).palette_entries(&query);
        arrange(entries, query.is_empty())
    };
    let rows = flatten(&arranged);
    // The links section's tiles want the relay's icon for hosts without a bundled brand mark
    // ([feedback link-icons-mac-adopt-2026-10-27]); asked once per host per session.
    let wants: Vec<String> = rows
        .iter()
        .filter_map(|e| match &e.target {
            Target::Link(url) if link_tile::Brand::for_url(url).is_none() => {
                Some(crate::ui::profiles::link_icon_host(url))
            }
            _ => None,
        })
        .collect();
    shell.state.update(cx, |s, _| {
        for host in wants {
            s.want_link_icon(host);
        }
    });
    if shell.palette.last_query != query {
        shell.palette.last_query = query.clone();
        // Typing resets the highlight to the top; the very first frame keeps
        // a preset one (the rig's `ARBOS_PALETTE_SELECT`).
        if shell.palette.len != 0 {
            shell.palette.cursor = Cursor::default();
        }
        shell.palette.reveal = 2;
    }
    shell.palette.len = rows.len();
    shell.palette.cursor.clamp(rows.len());
    let selected = shell.palette.cursor.selected;

    let viewport = window.viewport_size();
    let height: f32 = viewport.height.into();
    let top = (height * TOP_FRACTION).max(MIN_TOP);
    let max_h = (height * MAX_HEIGHT_FRACTION).max(FIELD_ROW + ROW_HEIGHT * 2.0);
    if shell.palette.reveal > 0 && !rows.is_empty() {
        shell.palette.reveal -= 1;
        reveal_row(scroll, &arranged, selected, max_h);
        if shell.palette.reveal > 0 {
            cx.notify();
        }
    }
    if shell.palette.open_on_rows && !rows.is_empty() {
        shell.palette.open_on_rows = false;
        let weak = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            let _ = weak.update(cx, |this, cx| this.palette_open_selected(window, cx));
        });
    }

    let field = div()
        .id("palette-field")
        .flex_shrink_0()
        .h(px(FIELD_ROW))
        .pl(px(FIELD_INSET))
        .pr(px(FIELD_INSET))
        .flex()
        .items_center()
        .gap(px(FIELD_GAP))
        .cursor_text()
        .debug_selector(|| "palette-field".into())
        .child(sf_symbols::glyph(
            "magnifyingglass",
            FIELD_GLYPH,
            theme.text_muted,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .text_size(px(FIELD_FONT))
                .child(shell.search_input.clone()),
        )
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                let handle = this.search_input.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
                cx.stop_propagation();
            }),
        );
    let hairline = div().flex_shrink_0().h(px(1.0)).bg(theme.border);

    let mut list = glass::contained_y_scroll("palette-list")
        .flex_1()
        .min_h_0()
        .track_scroll(scroll)
        .px(px(LIST_INSET))
        .pt(px(LIST_PAD_TOP))
        .pb(px(LIST_PAD_BOTTOM))
        .gap(px(ROW_GAP))
        .debug_selector(|| "palette-list".into());
    if rows.is_empty() {
        list = list.child(
            div()
                .h(px(ROW_HEIGHT))
                .px(px(ROW_PAD_X + 4.0))
                .flex()
                .items_center()
                .text_size(px(TITLE_FONT))
                .text_color(theme.text_muted)
                .child(if query.is_empty() {
                    "No chats yet — start one with +.".to_string()
                } else {
                    format!("Nothing matches “{}”.", typed.trim())
                }),
        );
    }
    let mut index = 0usize;
    for (section, entries) in &arranged {
        if *section != Section::Recent {
            list = list.child(section_caption(*section, &theme));
        }
        for entry in entries {
            list = list.child(row(shell, entry, index, index == selected, &theme, cx));
            index += 1;
        }
    }

    // One material ([feedback search-popup-single-layer-2026-10-28]): the
    // popup plate — frosted on Glass, the flat card on Matte — is the only
    // fill between the chat and the rows. The rows are transparent; the
    // highlighted one is a tint on top (`highlight::row`). No dim behind,
    // no second border: `glass::popup_plate`.
    // The panel owns the pointer over its whole box ([feedback
    // search-popup-swallows-hover-2026-10-28], Scout: "it also traces
    // movements through the search popup into the chat window"; Jacob: the
    // result list scrolled the chat behind it). `occlude` takes every hitbox
    // under the panel out of the hit test — no row behind it hovers, no rail
    // appears, no wheel reaches the transcript, also at the list's end; the
    // wheel stop is the belt to that brace. Clicks inside still land on the
    // rows and the field; a click outside is the shell's capture-phase
    // `click_outside`, which never looks at hitboxes under the pointer.
    let panel = div()
        .id("palette-panel")
        .key_context("Palette")
        .w(px(WIDTH))
        .max_w_full()
        .max_h(px(max_h))
        .flex()
        .flex_col()
        .rounded(px(RADIUS))
        .overflow_hidden()
        .occlude()
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .debug_selector(|| "palette-panel".into())
        .on_action(cx.listener(|this, a: &Pick1, w, cx| this.palette_pick_action(a, w, cx)))
        .on_action(cx.listener(|this, a: &Pick2, w, cx| this.palette_pick_action(a, w, cx)))
        .on_action(cx.listener(|this, a: &Pick3, w, cx| this.palette_pick_action(a, w, cx)))
        .on_action(cx.listener(|this, a: &Pick4, w, cx| this.palette_pick_action(a, w, cx)))
        .on_action(cx.listener(|this, a: &Pick5, w, cx| this.palette_pick_action(a, w, cx)))
        .on_action(cx.listener(|this, a: &Pick6, w, cx| this.palette_pick_action(a, w, cx)))
        .on_action(cx.listener(|this, a: &Pick7, w, cx| this.palette_pick_action(a, w, cx)))
        .on_action(cx.listener(|this, a: &Pick8, w, cx| this.palette_pick_action(a, w, cx)))
        .on_action(cx.listener(|this, a: &Pick9, w, cx| this.palette_pick_action(a, w, cx)))
        .child(field)
        .child(hairline)
        .child(list);
    let panel = glass::popup_plate(RADIUS, &theme, panel);

    // Spotlight hangs high, not mid-window: the shared backdrop hangs the
    // panel `top` under the window edge.
    // Click-outside dismissal lives in `Shell::click_outside` (capture phase,
    // against these bounds), so the click also reaches what is under it.
    let bounds = shell.palette_bounds.clone();
    let panel = div()
        .on_children_prepainted(move |children, _, _| {
            bounds.set(children.first().copied());
        })
        .child(panel);
    Some(
        modal::modal_overlay_top_passthrough("palette-backdrop", top, panel)
            .debug_selector(|| "palette-backdrop".into())
            .into_any_element(),
    )
}

/// Scroll the list so row `selected` is inside the visible part (the panel's
/// list box is `max_h` minus the field row, the hairline and the paddings).
fn reveal_row(
    scroll: &ScrollHandle,
    arranged: &[(Section, Vec<Entry>)],
    selected: usize,
    max_h: f32,
) {
    let mut y = 0.0;
    let mut index = 0usize;
    let mut row_top = None;
    for (section, entries) in arranged {
        if *section != Section::Recent {
            y += SECTION_HEIGHT + ROW_GAP;
        }
        for _ in entries {
            if index == selected {
                row_top = Some(y);
            }
            y += ROW_HEIGHT + ROW_GAP;
            index += 1;
        }
    }
    let Some(row_top) = row_top else {
        return;
    };
    let visible = max_h - FIELD_ROW - 1.0 - LIST_PAD_TOP - LIST_PAD_BOTTOM;
    let current: f32 = -f32::from(scroll.offset().y);
    let next = if row_top < current {
        row_top
    } else if row_top + ROW_HEIGHT > current + visible {
        row_top + ROW_HEIGHT - visible
    } else {
        return;
    };
    scroll.set_offset(gpui::point(px(0.0), px(-next.max(0.0))));
}

fn section_caption(section: Section, theme: &Theme) -> Div {
    div()
        .h(px(SECTION_HEIGHT))
        .flex_shrink_0()
        .px(px(ROW_PAD_X + 4.0))
        .flex()
        .items_end()
        .pb(px(4.0))
        .text_size(px(SECTION_FONT))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text_faint)
        .child(section.label())
}

/// One result row: picture, name over subtitle, the ⌘n badge on the first nine.
fn row(
    shell: &Shell,
    entry: &Entry,
    index: usize,
    selected: bool,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let target = entry.target.clone();
    let mut row = div()
        .id(ElementId::NamedInteger("palette-row".into(), index as u64))
        .h(px(ROW_HEIGHT))
        .flex_shrink_0()
        .px(px(ROW_PAD_X))
        .rounded(px(ROW_RADIUS))
        .flex()
        .items_center()
        .gap(px(AVATAR_GAP))
        .cursor_pointer()
        .debug_selector(move || format!("palette-row-{index}"))
        .map(|el| highlight::row(el, selected, highlight::Shape::Own, theme))
        .child(picture(shell, entry, theme, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(
                    // A title over a subtitle ("@handle") lifts an emoji-only
                    // name onto the baseline; a lone title stays centred.
                    if entry.subtitle.is_empty() {
                        div()
                            .text_size(px(TITLE_FONT))
                            .truncate()
                            .child(entry.title.clone())
                    } else {
                        identity::name(entry.title.clone(), TITLE_FONT)
                    }
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text),
                )
                .when(!entry.subtitle.is_empty(), |el| {
                    el.child(
                        div()
                            .text_size(px(SUBTITLE_FONT))
                            .text_color(theme.text_muted)
                            .truncate()
                            .child(entry.subtitle.clone()),
                    )
                }),
        )
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.palette_open_target(target.clone(), window, cx);
        }));
    if let Some(label) = shortcut_label(index) {
        row = row.child(
            div()
                .flex_shrink_0()
                .w(px(BADGE_W))
                .h(px(BADGE_H))
                .mr(px(BADGE_INSET - ROW_PAD_X))
                .rounded(px(BADGE_RADIUS))
                .bg(Theme::with_alpha(theme.text, 0.07))
                .border_1()
                .border_color(Theme::with_alpha(theme.text, 0.08))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(BADGE_FONT))
                .text_color(theme.text_muted)
                .debug_selector(move || format!("palette-shortcut-{index}"))
                .child(label),
        );
    }
    row
}

/// The row's picture: the chat's own disc, the server's rounded square, a
/// `#` tile for a channel, the thumbnail or a glyph tile for content.
fn picture(shell: &Shell, entry: &Entry, theme: &Theme, cx: &Context<Shell>) -> AnyElement {
    let state = shell.state.read(cx);
    let tile = |glyph: &'static str| {
        div()
            .size(px(AVATAR))
            .flex_shrink_0()
            .rounded(px(AVATAR * sidebar::SERVER_PICTURE_CORNER))
            .bg(theme.surface_2)
            .flex()
            .items_center()
            .justify_center()
            .child(sf_symbols::glyph(glyph, AVATAR * 0.58, theme.text_muted))
    };
    match &entry.target {
        Target::Setting(setting) => {
            if let crate::palette::SettingTarget::Agent { project_id, .. } = setting {
                if let Some(space) = state.project(project_id) {
                    return groups::picture(space, state, AVATAR, theme).into_any_element();
                }
            }
            // The Settings list's own tile, at the palette's picture size.
            glass::icon_tile_sized(crate::palette::target_glyph(setting), AVATAR, theme)
                .into_any_element()
        }
        Target::Space(id) | Target::Message { space_id: id, .. } => {
            let Some(space) = state.project(id) else {
                return tile("person.fill").into_any_element();
            };
            groups::chat_picture(space, state, AVATAR, theme).into_any_element()
        }
        Target::Server(id) | Target::Channel { server_id: id, .. } => {
            let Some(server) = state.server(id) else {
                return tile("number").into_any_element();
            };
            if entry.kind == Kind::Channel {
                tile("number").into_any_element()
            } else {
                sidebar::server_picture(server, state, AVATAR, theme).into_any_element()
            }
        }
        Target::Media {
            chat_id,
            message_id,
            part_id,
        } => {
            let thumb = state
                .messages_of(chat_id)
                .iter()
                .find(|m| &m.id == message_id)
                .and_then(|m| {
                    m.parts.iter().find_map(|p| match p {
                        crate::model::Part::Attachment { id, attachment } if id == part_id => {
                            attachment
                                .blob_ref
                                .as_ref()
                                .and_then(|k| state.thumbnails.get(k).cloned())
                        }
                        _ => None,
                    })
                });
            match thumb {
                Some(img) => div()
                    .size(px(AVATAR))
                    .flex_shrink_0()
                    .rounded(px(AVATAR * sidebar::SERVER_PICTURE_CORNER))
                    .overflow_hidden()
                    .child(
                        gpui::img(img)
                            .size(px(AVATAR))
                            .object_fit(gpui::ObjectFit::Cover),
                    )
                    .into_any_element(),
                None => tile(if entry.kind == Kind::Media {
                    "photo"
                } else {
                    "doc"
                })
                .into_any_element(),
            }
        }
        Target::ChannelMessage { .. } => tile("number").into_any_element(),
        Target::Person(handle) => {
            // A person met in a group or the directory: their photo / initials
            // disc, as the member list draws it.
            let person = state
                .projects
                .iter()
                .flat_map(|p| p.people.iter().chain(p.owner_person.iter()))
                .chain(state.people_results.iter())
                .find(|p| &p.handle == handle)
                .cloned();
            match person {
                Some(p) => {
                    let image = state.avatar(&p.avatar_url);
                    people::avatar(image, &p, AVATAR, theme).into_any_element()
                }
                None => tile("person.fill").into_any_element(),
            }
        }
        Target::Link(url) => {
            // The one link tile: brand mark, the relay's icon, or the host's letter.
            let icon_host = crate::ui::profiles::link_icon_host(url);
            let host = crate::ui::profiles::link_host(url);
            let icon = link_tile::icon_for(url, &host, state.link_icon(&icon_host));
            link_tile::tile(icon, AVATAR, theme).into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    //! The panel's shape at its real metrics: a chat column with a scrolling
    //! transcript and a composer behind, the palette (field + list) on the
    //! shared modal backdrop over it. What Jacob asked for: a wheel over the
    //! palette never moves the transcript, typing lands only in the palette
    //! field, a click outside closes, the rows and badges are the measured
    //! sizes.
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{
        point, px, Focusable, Pixels, ScrollDelta, ScrollHandle, ScrollWheelEvent, TestAppContext,
        TouchPhase,
    };

    use super::*;
    use crate::ui::composer::ComposerInput;
    use crate::ui::theme::{AppearancePref, Surface};

    const W: f32 = 1180.0;
    const H: f32 = 780.0;
    const ROWS: usize = 14;

    struct Scene {
        transcript: ScrollHandle,
        list: ScrollHandle,
        open: bool,
        /// Set by a click on the target under the backdrop (a stand-in for a
        /// message / sidebar row / button).
        hit: Rc<Cell<bool>>,
        composer: gpui::Entity<ComposerInput>,
        field: gpui::Entity<ComposerInput>,
        selected: usize,
    }

    impl gpui::Render for Scene {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            if self.open {
                modal::keep_focus(&self.field.read(cx).focus_handle(cx), window, cx);
            }
            let mut messages = div().flex().flex_col();
            for i in 0..60 {
                messages =
                    messages.child(div().h(px(40.0)).flex_shrink_0().child(format!("m {i}")));
            }
            let transcript = div()
                .id("transcript")
                .absolute()
                .inset_0()
                .overflow_y_scroll()
                .track_scroll(&self.transcript)
                .debug_selector(|| "transcript".into())
                .child(messages);
            let composer = div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(44.0))
                .child(self.composer.clone());
            let hit = self.hit.clone();
            let target = div()
                .id("under-target")
                .absolute()
                .left(px(10.0))
                .bottom(px(60.0))
                .size(px(40.0))
                .debug_selector(|| "under-target".into())
                .on_click(move |_, _, _| hit.set(true));
            let mut root = div()
                .relative()
                .w(px(W))
                .h(px(H))
                .child(transcript)
                .child(composer)
                .child(target);
            if self.open {
                let field = div()
                    .h(px(FIELD_ROW))
                    .flex_shrink_0()
                    .pl(px(FIELD_INSET))
                    .flex()
                    .items_center()
                    .gap(px(FIELD_GAP))
                    .debug_selector(|| "palette-field".into())
                    .child(sf_symbols::glyph(
                        "magnifyingglass",
                        FIELD_GLYPH,
                        theme.text_muted,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(FIELD_FONT))
                            .child(self.field.clone()),
                    );
                let mut list = glass::contained_y_scroll("palette-list")
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.list)
                    .px(px(LIST_INSET))
                    .pt(px(LIST_PAD_TOP))
                    .pb(px(LIST_PAD_BOTTOM))
                    .gap(px(ROW_GAP))
                    .debug_selector(|| "palette-list".into());
                for i in 0..ROWS {
                    let sel: &'static str = Box::leak(format!("palette-row-{i}").into_boxed_str());
                    let mut row = div()
                        .h(px(ROW_HEIGHT))
                        .flex_shrink_0()
                        .px(px(ROW_PAD_X))
                        .rounded(px(ROW_RADIUS))
                        .flex()
                        .items_center()
                        .gap(px(AVATAR_GAP))
                        .debug_selector(move || sel.into())
                        .when(i == self.selected, |el| {
                            el.bg(highlight::fills(&theme).selected)
                        })
                        .child(div().size(px(AVATAR)).rounded_full().bg(theme.surface_2))
                        .child(div().flex_1().child(format!("Row {i}")));
                    if let Some(label) = shortcut_label(i) {
                        let bsel: &'static str =
                            Box::leak(format!("palette-shortcut-{i}").into_boxed_str());
                        row = row.child(
                            div()
                                .w(px(BADGE_W))
                                .h(px(BADGE_H))
                                .mr(px(BADGE_INSET - ROW_PAD_X))
                                .debug_selector(move || bsel.into())
                                .child(label),
                        );
                    }
                    list = list.child(row);
                }
                let panel = div()
                    .w(px(WIDTH))
                    .max_h(px(H * MAX_HEIGHT_FRACTION))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .debug_selector(|| "palette-panel".into())
                    .child(field)
                    .child(div().h(px(1.0)))
                    .child(list);
                root = root.child(modal::modal_overlay_top_passthrough(
                    "palette-backdrop",
                    (H * TOP_FRACTION).max(MIN_TOP),
                    panel,
                ));
            }
            root
        }
    }

    fn setup(
        cx: &mut TestAppContext,
        open: bool,
    ) -> (gpui::Entity<Scene>, &mut gpui::VisualTestContext) {
        cx.update(|cx| {
            if !cx.has_global::<Theme>() {
                let theme = Theme::for_pref(AppearancePref::Dark, Surface::Matte, cx);
                cx.set_global(theme);
            }
        });
        let (view, vcx) = cx.add_window_view(|window, cx| {
            let composer = cx.new(ComposerInput::new);
            let field = cx.new(ComposerInput::new);
            let handle = composer.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
            Scene {
                transcript: ScrollHandle::new(),
                list: ScrollHandle::new(),
                open,
                hit: Rc::new(Cell::new(false)),
                composer,
                field,
                selected: 0,
            }
        });
        vcx.run_until_parked();
        (view, vcx)
    }

    fn wheel(vcx: &mut gpui::VisualTestContext, at: gpui::Point<Pixels>, dy: f32) {
        vcx.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.0), px(dy))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        vcx.run_until_parked();
    }

    /// Jacob: scrolling inside the palette must not scroll the chat behind.
    /// Over the list, over the field and over the backdrop the transcript
    /// stays; the palette's own list scrolls.
    #[gpui::test]
    fn a_wheel_over_the_palette_does_not_move_the_transcript(cx: &mut TestAppContext) {
        let (view, vcx) = setup(cx, true);
        let list = vcx.debug_bounds("palette-list").expect("list");
        let field = vcx.debug_bounds("palette-field").expect("field");
        let before: f32 = view.read_with(vcx, |s, _| s.transcript.offset().y.into());
        wheel(vcx, list.center(), -160.0);
        wheel(vcx, field.center(), -160.0);
        wheel(vcx, point(px(30.0), px(H - 30.0)), -160.0);
        let after: f32 = view.read_with(vcx, |s, _| s.transcript.offset().y.into());
        assert_eq!(after, before, "the transcript moved under the palette");
        let own: f32 = view.read_with(vcx, |s, _| s.list.offset().y.into());
        assert!(own < -1.0, "the palette's own list did not scroll ({own})");
    }

    /// Control: with the palette closed the same wheel scrolls the chat.
    #[gpui::test]
    fn the_chat_scrolls_when_the_palette_is_closed(cx: &mut TestAppContext) {
        let (view, vcx) = setup(cx, false);
        wheel(vcx, point(px(W / 2.0), px(H / 2.0)), -160.0);
        let after: f32 = view.read_with(vcx, |s, _| s.transcript.offset().y.into());
        assert!(after < -1.0, "the transcript did not scroll ({after})");
    }

    /// Jacob: keys never reach the composer while the palette is up.
    #[gpui::test]
    fn typing_goes_only_to_the_palette_field(cx: &mut TestAppContext) {
        let (view, vcx) = setup(cx, true);
        vcx.simulate_input("carro");
        vcx.run_until_parked();
        let (composer, field) = view.read_with(vcx, |s, cx| {
            (
                s.composer.read(cx).text().to_owned(),
                s.field.read(cx).text().to_owned(),
            )
        });
        assert_eq!(field, "carro");
        assert_eq!(composer, "", "the composer behind the palette typed");
    }

    /// The backdrop is see-through for the mouse ([feedback
    /// search-toggle-click-outside-mac-2026-10-27]): a click outside the panel
    /// reaches the target under it in the same click (the shell closes the
    /// palette from its capture-phase `click_outside`; `ui::chat_search::tests`
    /// covers that on the real shell). A click inside the panel stays inside.
    #[gpui::test]
    fn a_click_outside_the_panel_reaches_what_is_under_the_backdrop(cx: &mut TestAppContext) {
        let (view, vcx) = setup(cx, true);
        let panel = vcx.debug_bounds("palette-panel").expect("panel");
        vcx.simulate_click(panel.center(), Default::default());
        vcx.run_until_parked();
        assert!(
            !view.read_with(vcx, |s, _| s.hit.get()),
            "a click in the panel leaked under it"
        );
        let target = vcx
            .debug_bounds("under-target")
            .expect("target under the backdrop");
        vcx.simulate_click(target.center(), Default::default());
        vcx.run_until_parked();
        assert!(
            view.read_with(vcx, |s, _| s.hit.get()),
            "the click outside was swallowed"
        );
    }

    /// The measured geometry: panel 558 wide hanging at 12 % of the height,
    /// field row 49, rows 49 tall and 2 apart, ⌘ badges 27 × 19 on the first
    /// nine rows only, 8 pt from the row's right edge.
    #[gpui::test]
    fn the_panel_rows_and_badges_are_groks_sizes(cx: &mut TestAppContext) {
        let (_view, vcx) = setup(cx, true);
        let panel = vcx.debug_bounds("palette-panel").expect("panel");
        let pw: f32 = panel.size.width.into();
        let ptop: f32 = panel.origin.y.into();
        let pleft: f32 = panel.origin.x.into();
        assert_eq!(pw, WIDTH);
        assert!((ptop - H * TOP_FRACTION).abs() <= 0.5, "panel top {ptop}");
        assert!(
            (pleft - (W - WIDTH) / 2.0).abs() <= 0.5,
            "panel centred: left {pleft}"
        );
        let field = vcx.debug_bounds("palette-field").expect("field");
        assert_eq!(f32::from(field.size.height), FIELD_ROW);
        let r0 = vcx.debug_bounds("palette-row-0").expect("row 0");
        let r1 = vcx.debug_bounds("palette-row-1").expect("row 1");
        assert_eq!(f32::from(r0.size.height), ROW_HEIGHT);
        let pitch: f32 = (r1.origin.y - r0.origin.y).into();
        assert_eq!(pitch, ROW_HEIGHT + ROW_GAP);
        let list = vcx.debug_bounds("palette-list").expect("list");
        let row_left: f32 = (r0.origin.x - list.origin.x).into();
        assert_eq!(row_left, LIST_INSET);
        let badge = vcx.debug_bounds("palette-shortcut-0").expect("⌘1");
        assert_eq!(f32::from(badge.size.width), BADGE_W);
        assert_eq!(f32::from(badge.size.height), BADGE_H);
        let badge_right: f32 = (r0.right() - badge.right()).into();
        assert_eq!(badge_right, BADGE_INSET);
        assert!(vcx.debug_bounds("palette-shortcut-8").is_some(), "⌘9 drawn");
        assert!(
            vcx.debug_bounds("palette-shortcut-9").is_none(),
            "no badge after ⌘9"
        );
    }

    #[test]
    fn cmd_digits_map_to_rows() {
        assert_eq!(pick_index(&Pick1), Some(0));
        assert_eq!(pick_index(&Pick9), Some(8));
        assert_eq!(pick_index(&crate::ui::shell::Escape), None);
    }
}

#[cfg(test)]
mod shell_tests {
    //! The popup over the real shell ([feedback
    //! search-popup-swallows-hover-2026-10-28]; Scout 2026-10-28: "it also
    //! traces movements through the search popup into the chat window";
    //! Jacob: "scrolling the popup's result list also scrolls the chat behind
    //! it"). A DM long enough that bubbles fill the column under the panel,
    //! and enough chats that the palette's own list overflows: the pointer
    //! over the panel hovers nothing behind it, the wheel over it — also at
    //! the end of its list — leaves the transcript where it was, a click on
    //! a row still opens it, and the ⌘F bar swallows the wheel the same way.
    use futures::future::BoxFuture;
    use gpui::{
        point, px, AppContext as _, Entity, Pixels, Point, ScrollDelta, ScrollWheelEvent,
        TestAppContext, TouchPhase,
    };
    use tokio::sync::mpsc;

    use crate::app::AppState;
    use crate::model::{now_ms, Chat, Message, Person, Project, SpaceKind};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::motion::PulseClock;
    use crate::ui::shell::Shell;
    use crate::ui::theme::{Surface, Theme};

    /// Chats in the sidebar and the palette's Recent list: more than fit the
    /// panel at its 72 % cap, so the list scrolls.
    const CHATS: usize = 16;
    /// Rows in the open DM: enough to fill the column top to bottom.
    const ROWS: usize = 40;

    struct Quiet;

    impl SyncSource for Quiet {
        fn name(&self) -> &'static str {
            "quiet"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            _commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            Box::pin(std::future::pending())
        }
    }

    fn dm_id(n: usize) -> String {
        format!("01DM{n:02}000000000000000000000")
    }

    fn message(chat: &str, n: usize, at: u64) -> Message {
        let (sender, handle) = if n % 2 == 0 {
            ("user:gh_2", "sam")
        } else {
            ("user:gh_me", "unconst")
        };
        serde_json::from_value(serde_json::json!({
            "id": format!("{chat}-m{n}"), "chatId": chat, "role": "user", "sender": sender,
            "senderHandle": handle, "senderName": if handle == "sam" { "Sam Rivera" } else { "Jacob" },
            "status": "done", "createdAt": at, "completedAt": at,
            "parts": [{ "id": format!("{chat}-m{n}-t"), "kind": "text",
                        "text": format!("Line {n} of a long thread about the launch page and what a seat includes.") }],
        }))
        .expect("message")
    }

    fn mount(
        cx: &mut TestAppContext,
        surface: Surface,
    ) -> (
        tokio::runtime::Runtime,
        Entity<AppState>,
        Entity<Shell>,
        &mut gpui::VisualTestContext,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(surface));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let state = cx.new(|cx| AppState::new(Box::new(Quiet), cx));
        let now = now_ms();
        state.update(cx, |s, cx| {
            for n in 0..CHATS {
                let id = dm_id(n);
                let person = Person {
                    id: format!("gh_{}", n + 2),
                    handle: format!("person{n}"),
                    name: Some(format!("Person {n}")),
                    ..Person::default()
                };
                s.projects.push(Project {
                    id: id.clone(),
                    name: format!("person{n}"),
                    kind: SpaceKind::Dm,
                    people: vec![person],
                    ..Project::default()
                });
                let last = now - (n as u64) * 60_000;
                let chat: Chat = serde_json::from_value(serde_json::json!({
                    "id": id, "project_id": id, "title": format!("person{n}"), "is_pinned": false,
                    "is_system": false, "unread": false, "last_message_at": last,
                }))
                .expect("chat");
                s.chats.insert(id.clone(), vec![chat]);
                let rows = if n == 0 { ROWS } else { 1 };
                s.messages.insert(
                    id.clone(),
                    (0..rows)
                        .map(|m| message(&id, m, last - ((rows - m) as u64) * 1_000))
                        .collect(),
                );
                s.active_tab.insert(id.clone(), id.clone());
            }
            s.selected_project = Some(dm_id(0));
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.run_until_parked();
        settle(&state, &shell, vcx);
        (rt, state, shell, vcx)
    }

    fn settle(state: &Entity<AppState>, shell: &Entity<Shell>, vcx: &mut gpui::VisualTestContext) {
        for _ in 0..3 {
            state.update(vcx, |_, cx| cx.notify());
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    fn hovered(shell: &Entity<Shell>, vcx: &gpui::VisualTestContext) -> Option<String> {
        shell.read_with(vcx, |sh, cx| sh.transcript.read(cx).hovered_bubble())
    }

    fn scroll(shell: &Entity<Shell>, vcx: &gpui::VisualTestContext) -> (usize, f32) {
        shell.read_with(vcx, |sh, cx| sh.transcript.read(cx).scroll_position())
    }

    fn own_offset(shell: &Entity<Shell>, vcx: &gpui::VisualTestContext) -> f32 {
        shell.read_with(vcx, |sh, _| sh.palette_scroll.offset().y.into())
    }

    fn wheel(vcx: &mut gpui::VisualTestContext, at: Point<Pixels>, dy: f32) {
        vcx.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.0), px(dy))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        vcx.run_until_parked();
    }

    fn open_palette(
        state: &Entity<AppState>,
        shell: &Entity<Shell>,
        vcx: &mut gpui::VisualTestContext,
    ) {
        vcx.dispatch_action(crate::ui::shell::OpenPalette);
        settle(state, shell, vcx);
        assert!(shell.read_with(&*vcx, |sh, _| sh.palette.open));
    }

    /// The chat column's transcript area: the header's width, from under the
    /// header to the composer.
    fn transcript_area(vcx: &mut gpui::VisualTestContext) -> gpui::Bounds<Pixels> {
        let header = vcx.debug_bounds("chat-header-bar").expect("header drawn");
        let composer = vcx
            .debug_bounds("chat-composer-block")
            .expect("composer drawn");
        gpui::Bounds::from_corners(
            point(header.origin.x, header.bottom()),
            point(header.right(), composer.origin.y),
        )
    }

    /// A point on the panel that has a transcript row under it: a row in the
    /// middle of the list, inside the chat column.
    fn point_over_a_row(vcx: &mut gpui::VisualTestContext) -> Point<Pixels> {
        let row = vcx.debug_bounds("palette-row-4").expect("row 4 drawn");
        let transcript = transcript_area(vcx);
        let at = row.center();
        assert!(
            transcript.contains(&at),
            "the panel row at {at:?} is not over the chat column {transcript:?}"
        );
        at
    }

    /// Scout: the pointer over the popup hovers the bubble behind — its rail
    /// and time appear. With the panel occluding, nothing under it hovers;
    /// the same point with the palette closed hovers the bubble (so the
    /// check is live).
    #[gpui::test]
    fn the_pointer_over_the_panel_hovers_nothing_behind_it(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx, Surface::Glass);
        open_palette(&state, &shell, vcx);
        let at = point_over_a_row(vcx);
        // Approach from outside the panel, as a hand would.
        vcx.simulate_mouse_move(point(at.x, px(2.0)), None, Default::default());
        vcx.run_until_parked();
        vcx.simulate_mouse_move(at, None, Default::default());
        settle(&state, &shell, vcx);
        assert_eq!(
            hovered(&shell, vcx),
            None,
            "a bubble under the panel hovered"
        );

        // Control: the palette closed, the same point hovers a bubble.
        vcx.dispatch_action(crate::ui::shell::OpenPalette);
        settle(&state, &shell, vcx);
        assert!(!shell.read_with(&*vcx, |sh, _| sh.palette.open));
        vcx.simulate_mouse_move(point(at.x, px(2.0)), None, Default::default());
        vcx.run_until_parked();
        vcx.simulate_mouse_move(at, None, Default::default());
        settle(&state, &shell, vcx);
        assert!(
            hovered(&shell, vcx).is_some(),
            "with the palette closed the same point hovers a bubble"
        );
    }

    /// Jacob: the result list and the chat scrolled together. A wheel over
    /// the list scrolls the list only; at the list's end the wheel is
    /// consumed, not forwarded; over the field and the panel's edge the same.
    #[gpui::test]
    fn the_wheel_over_the_panel_never_moves_the_transcript_also_at_the_lists_end(
        cx: &mut TestAppContext,
    ) {
        let (_rt, state, shell, vcx) = mount(cx, Surface::Glass);
        // Control first: with nothing over it, the wheel moves the chat.
        let at_rest = scroll(&shell, vcx);
        let column = transcript_area(vcx);
        wheel(vcx, column.center(), 160.0);
        settle(&state, &shell, vcx);
        assert_ne!(
            scroll(&shell, vcx),
            at_rest,
            "the control wheel moved the chat"
        );

        open_palette(&state, &shell, vcx);
        let before = scroll(&shell, vcx);
        let list = vcx.debug_bounds("palette-list").expect("list");
        let field = vcx.debug_bounds("palette-field").expect("field");
        let panel = vcx.debug_bounds("palette-panel").expect("panel");
        wheel(vcx, list.center(), -160.0);
        settle(&state, &shell, vcx);
        assert!(
            own_offset(&shell, vcx) < -1.0,
            "the palette's own list scrolled ({})",
            own_offset(&shell, vcx)
        );
        assert_eq!(
            scroll(&shell, vcx),
            before,
            "the transcript moved under the list"
        );

        // To the end of the list and past it: still nothing behind moves.
        for _ in 0..12 {
            wheel(vcx, list.center(), -400.0);
        }
        settle(&state, &shell, vcx);
        let at_end = own_offset(&shell, vcx);
        wheel(vcx, list.center(), -160.0);
        settle(&state, &shell, vcx);
        assert_eq!(own_offset(&shell, vcx), at_end, "the list is at its end");
        assert_eq!(
            scroll(&shell, vcx),
            before,
            "the wheel chained into the transcript at the end"
        );
        wheel(vcx, list.center(), 160.0);
        wheel(vcx, field.center(), 160.0);
        wheel(
            vcx,
            point(panel.right() - px(2.0), panel.bottom() - px(2.0)),
            160.0,
        );
        settle(&state, &shell, vcx);
        assert_eq!(
            scroll(&shell, vcx),
            before,
            "the transcript moved under the panel"
        );
    }

    /// Clicks on results still work with the panel occluding: a click on a
    /// row opens that chat and closes the palette.
    #[gpui::test]
    fn a_click_on_a_row_still_opens_it(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx, Surface::Glass);
        open_palette(&state, &shell, vcx);
        let row = vcx.debug_bounds("palette-row-1").expect("row 1");
        vcx.simulate_click(row.center(), Default::default());
        settle(&state, &shell, vcx);
        assert!(
            !shell.read_with(&*vcx, |sh, _| sh.palette.open),
            "the palette closed"
        );
        let selected = state.read_with(&*vcx, |s, _| s.selected_project.clone());
        assert_eq!(
            selected.as_deref(),
            Some(dm_id(1).as_str()),
            "row 1 opened its chat"
        );
    }

    /// The ⌘F bar: a wheel over it leaves the transcript where it was.
    #[gpui::test]
    fn the_wheel_over_the_chat_search_bar_never_moves_the_transcript(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx, Surface::Matte);
        vcx.dispatch_action(crate::ui::chat_search::Open);
        settle(&state, &shell, vcx);
        let bar = vcx.debug_bounds("chat-search-bar").expect("bar drawn");
        let before = scroll(&shell, vcx);
        wheel(vcx, bar.center(), 160.0);
        wheel(vcx, bar.center(), -160.0);
        settle(&state, &shell, vcx);
        assert_eq!(
            scroll(&shell, vcx),
            before,
            "the transcript moved under the bar"
        );
    }

    /// Layering ([feedback search-popup-single-layer-2026-10-28]) over the
    /// real shell: the panel is drawn once, inside the see-through backdrop,
    /// and no second plate or dim sits between the chat and the rows.
    #[gpui::test]
    fn the_panel_is_drawn_once_on_a_backdrop_that_does_not_dim(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx, Surface::Glass);
        open_palette(&state, &shell, vcx);
        let backdrop = vcx.debug_bounds("palette-backdrop").expect("backdrop");
        let panel = vcx.debug_bounds("palette-panel").expect("panel");
        let viewport = vcx.update(|window, _| window.viewport_size());
        assert_eq!(backdrop.size, viewport, "the backdrop spans the window");
        assert!(backdrop.contains(&panel.center()));
        assert_eq!(crate::ui::modal::PASSTHROUGH_BACKDROP_ALPHA, 0.0);
        let material = vcx.update(|_, cx| crate::ui::glass::popup_material(Theme::of(cx)));
        assert!(!material.dims_window && material.blur > 0.0 && material.fill.a > 0.0);
    }
}
