//! The one row/card kit every right pane draws with (Jacob, 2026-10-20:
//! "make the settings, user and group panels consistent"). Settings cards,
//! member lists, group lists, file lists, Block user and Sign out all come
//! from here, so a metric lives in exactly one place:
//!
//! * card: `CARD_RADIUS` corners, `pane_card` fill (dark: the D80 grey, the
//!   iPhone's `PaneCard`; light: a grey card on the white pane), 16 pt side
//!   margins, a `section_title` over it where Grok labels a card;
//! * row: 41 pt minimum (51 with a subtitle), 16 pt inset, 12 pt gap,
//!   hairline under the label;
//! * leading: a 30 pt icon tile (settings), a 36 pt avatar (people, agents,
//!   groups) or nothing;
//! * detail: `TEXT_SMALL` muted, right-aligned; subtitle: `TEXT_SMALL` muted
//!   under the title; chevron 13 pt faint when the row pushes a page;
//! * danger row: centred red label, same height.

use gpui::{
    div, prelude::*, px, AnyElement, Div, ElementId, FontWeight, Hsla, SharedString, Stateful,
};

use crate::ui::glass;
use crate::ui::icons::icon;
use crate::ui::identity;
use crate::ui::theme::{Theme, TEXT_BODY, TEXT_SMALL, TEXT_TINY};

/// Row height, side inset, gap between leading and text. Grok Bot's pane
/// cards ([feedback info-pane-chrome-2026-10-24], `grok-right-pane-details.png`
/// at 2 px/pt): a one-line row is 41 pt (the Coding row, 584–665 px), a
/// two-line row 51 (the three Routines rows, 306 px / 3); ours were 48 / 58.
/// With the rows' 6 pt vertical padding a title + subtitle row lands on 51 by
/// itself, so the minimum only carries the one-line rows.
pub const ROW_MIN_H: f32 = 41.0;
/// Card inset from the pane's edges: Grok's 31 px / 32 px → 16 (ours was 16).
pub const INSET: f32 = 16.0;
pub const GAP: f32 = identity::ROW_GAP;
/// Card corners: Grok's fill reaches the card's left edge 21 px (10.5 pt)
/// under its top row → radius 12 (ours was `PANEL_RADIUS` 14).
pub const CARD_RADIUS: f32 = 12.0;
/// Card → next card (Jacob's mocks: 12–14 between cards).
pub const CARD_GAP: f32 = 14.0;
/// Section label ("Coding", "Routines"): its text starts 28 pt from the pane's
/// edge (Grok: 56 px) — the card inset plus this; box padding above / below
/// so a label's glyphs sit 26 under the card before it (14 + 10 + ~3 into
/// the box) and the next card starts 10 under its glyphs (Grok: 26 / 10–11).
pub const SECTION_TITLE_INDENT: f32 = 12.0;
pub const SECTION_TITLE_TOP: f32 = 10.0;
pub const SECTION_TITLE_BOTTOM: f32 = 8.0;
/// The label's line box (the app's 21 pt line would float the 13 pt glyphs
/// 4 pt lower than Grok's).
pub const SECTION_TITLE_LINE: f32 = 16.0;
/// The avatar size on people / agent / group rows.
pub const AVATAR: f32 = 36.0;
/// The hairline starts under the label: inset + leading + gap.
fn separator_inset(leading: &Leading) -> f32 {
    match leading {
        Leading::None => INSET,
        Leading::Tile(_) => INSET + 30.0 + GAP,
        Leading::Avatar(_) | Leading::Glyph(_) => INSET + AVATAR + GAP,
    }
}

/// What sits at a row's leading edge.
pub enum Leading {
    None,
    /// An Apple-style 30 pt icon tile (settings rows).
    Tile(&'static str),
    /// A 36 pt picture (people, agents, groups, files).
    Avatar(AnyElement),
    /// A 36 pt box with a glyph in it (Add members, files).
    Glyph(&'static str),
}

/// The grouped card: one fill, one radius, one margin.
pub fn card(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .mx(px(INSET))
        .mb(px(CARD_GAP))
        .rounded(px(CARD_RADIUS))
        .bg(theme.pane_card)
        .overflow_hidden()
}

/// The one separator every grouped row uses: a 1 pt hairline hung 1 pt
/// *below* the row's box (`.relative()` on the row). Between rows it lands on
/// the next row's first pixel; under the last row the card's clip swallows it,
/// and nothing sits above the first — separators only between rows, inside
/// the card (Jacob, 2026-10-21). Drawn as a bottom border it would stay inside
/// the box and show under a card's last row.
pub fn hairline_below(inset: f32, theme: &Theme) -> Div {
    div()
        .absolute()
        .left(px(inset))
        .right(px(0.0))
        .bottom(px(-1.0))
        .h(px(1.0))
        .bg(theme.border)
}

/// What a grouped section is made of, in order: rows with exactly one
/// separator between each neighbouring pair — none before the first, none
/// after the last. Pure, so it can be tested without a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionPart {
    Row(usize),
    Separator,
}

pub fn section_parts(rows: usize) -> Vec<SectionPart> {
    let mut parts = Vec::with_capacity(rows.saturating_mul(2));
    for i in 0..rows {
        if i > 0 {
            parts.push(SectionPart::Separator);
        }
        parts.push(SectionPart::Row(i));
    }
    parts
}

/// A grouped card whose separators are explicit elements placed by
/// [`section_parts`]: rows that carry no hairline of their own (plain
/// content, custom rows) get exactly `rows - 1` lines, inside the card.
pub fn section(rows: Vec<AnyElement>, inset: f32, theme: &Theme) -> Div {
    let mut rows: Vec<Option<AnyElement>> = rows.into_iter().map(Some).collect();
    let mut out = card(theme);
    for part in section_parts(rows.len()) {
        out = match part {
            SectionPart::Row(i) => out.children(rows[i].take()),
            SectionPart::Separator => out.child(div().ml(px(inset)).h(px(1.0)).bg(theme.border)),
        };
    }
    out
}

fn leading_el(leading: Leading, theme: &Theme) -> Option<AnyElement> {
    match leading {
        Leading::None => None,
        Leading::Tile(glyph) => Some(glass::icon_tile(glyph, theme).into_any_element()),
        Leading::Avatar(el) => Some(identity::slot(AVATAR, el).into_any_element()),
        Leading::Glyph(glyph) => Some(
            div()
                .size(px(AVATAR))
                .flex_shrink_0()
                .rounded(px(10.0))
                .bg(theme.surface_2)
                .flex()
                .items_center()
                .justify_center()
                .child(icon(glyph, 18.0, theme.text_muted))
                .into_any_element(),
        ),
    }
}

/// One row: leading, title (+ optional subtitle), optional right detail,
/// optional trailing element, chevron when it pushes a page. The hairline
/// sits under the label, 1 pt below the row: between rows it lands on the
/// next row's first pixel, under the last row the card's clip swallows it
/// (D80 / the iPhone's `PaneCardLayout`: no line under the last row).
#[allow(clippy::too_many_arguments)]
pub fn row(
    id: impl Into<ElementId>,
    leading: Leading,
    title: impl Into<SharedString>,
    subtitle: Option<SharedString>,
    detail: Option<SharedString>,
    trailing: Option<AnyElement>,
    chevron: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let sep = separator_inset(&leading);
    // Mock b: names on people / agent / file rows are medium; mock g: settings
    // labels are regular.
    let title_weight = match leading {
        Leading::Avatar(_) | Leading::Glyph(_) => FontWeight::MEDIUM,
        Leading::None | Leading::Tile(_) => FontWeight::NORMAL,
    };
    let leading = leading_el(leading, theme);
    div()
        .id(id)
        .relative()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(GAP))
        .min_h(px(ROW_MIN_H))
        .px(px(INSET))
        .py(px(6.0))
        .text_size(px(TEXT_BODY))
        .text_color(theme.text)
        .children(leading)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(
                    div()
                        .truncate()
                        .font_weight(title_weight)
                        .child(title.into()),
                )
                .when_some(subtitle, |el, s| {
                    el.child(
                        div()
                            .text_size(px(TEXT_SMALL))
                            .text_color(theme.text_muted)
                            .truncate()
                            .child(s),
                    )
                }),
        )
        .when_some(detail, |el, d| {
            el.child(
                div()
                    .flex_shrink_0()
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_muted)
                    .child(d),
            )
        })
        .children(trailing)
        .when(chevron, |el| {
            el.child(icon("chevron-right", 13.0, theme.text_faint))
        })
        .child(hairline_below(sep, theme))
}

/// A row that pushes a page: tile, label, detail, chevron, hover.
pub fn nav_row(
    id: impl Into<ElementId>,
    glyph: &'static str,
    title: impl Into<SharedString>,
    detail: Option<String>,
    theme: &Theme,
) -> Stateful<Div> {
    row(
        id,
        Leading::Tile(glyph),
        title,
        None,
        detail.map(SharedString::from),
        None,
        true,
        theme,
    )
    .cursor_pointer()
    .hover(|s| s.bg(theme.surface_2))
}

/// Label left, value right (not interactive).
pub fn value_row(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    value: impl Into<SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    row(
        id,
        Leading::None,
        title,
        None,
        Some(value.into()),
        None,
        false,
        theme,
    )
}

/// The destructive action at the end of a pane (Block user, Sign out,
/// Remove picture): a centred red label in its own card.
pub fn danger_row(id: impl Into<ElementId>, label: &str, theme: &Theme) -> Stateful<Div> {
    action_row(id, label, theme.danger, theme)
}

/// A centred one-label action in its own card, in the given colour — the
/// non-destructive twin of `danger_row` (Unblock in the accent colour).
pub fn action_row(
    id: impl Into<ElementId>,
    label: &str,
    color: Hsla,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .min_h(px(ROW_MIN_H))
        .px(px(INSET))
        .text_size(px(TEXT_BODY))
        .text_color(color)
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_2))
        .child(label.to_string())
}

/// Grey explanatory line under a card.
pub fn footnote(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .flex_shrink_0()
        .px(px(INSET * 2.0))
        .mt(px(-8.0))
        .mb(px(14.0))
        .text_size(px(TEXT_TINY))
        .text_color(theme.text_muted)
        .child(text.into())
}

/// "Coding", "Routines", "Images · 23" over a card: Grok's section label —
/// `TEXT_SMALL` muted, indented past the card's edge, its own padding so the
/// card before and after it land at Grok's distances (see the constants).
pub fn section_title(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .flex_shrink_0()
        .px(px(INSET + SECTION_TITLE_INDENT))
        .pt(px(SECTION_TITLE_TOP))
        .pb(px(SECTION_TITLE_BOTTOM))
        .text_size(px(TEXT_SMALL))
        .line_height(px(SECTION_TITLE_LINE))
        .text_color(theme.text_muted)
        .debug_selector(|| "section-title".into())
        .child(text.into())
}

/// Centered grey line for an empty list.
pub fn empty(text: &str, theme: &Theme) -> Div {
    div()
        .flex_shrink_0()
        .px(px(INSET))
        .py(px(28.0))
        .flex()
        .justify_center()
        .text_size(px(TEXT_SMALL))
        .text_color(theme.text_muted)
        .child(text.to_string())
}

/// A confirm pill: Cancel (quiet) or the red action.
pub fn confirm_button(
    id: &'static str,
    label: &'static str,
    danger: bool,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(28.0))
        .px(px(12.0))
        .rounded_full()
        .flex()
        .items_center()
        .text_size(px(TEXT_SMALL))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .when(danger, |el| {
            el.bg(theme.danger)
                .text_color(gpui::white())
                .hover(|s| s.opacity(0.88))
        })
        .when(!danger, |el| {
            el.bg(theme.field)
                .border_1()
                .border_color(theme.field_border)
                .text_color(theme.text)
                .hover(|s| s.bg(theme.surface_2))
        })
        .child(label)
}

/// "Remove @x from Group?" in a row's place: the question, then Cancel / the action.
pub fn confirm_row(
    question: &str,
    action: &'static str,
    cancel: Stateful<Div>,
    go: Stateful<Div>,
    theme: &Theme,
) -> Div {
    let _ = action;
    div()
        .relative()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .px(px(INSET))
        .py(px(10.0))
        .child(hairline_below(INSET, theme))
        .child(
            div()
                .text_size(px(TEXT_SMALL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(question.to_string()),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(6.0))
                .child(cancel)
                .child(go),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_row_section_has_no_separator() {
        let parts = section_parts(1);
        assert_eq!(parts, vec![SectionPart::Row(0)]);
    }

    #[test]
    fn a_three_row_section_has_exactly_two_separators_between_rows() {
        let parts = section_parts(3);
        assert_eq!(
            parts,
            vec![
                SectionPart::Row(0),
                SectionPart::Separator,
                SectionPart::Row(1),
                SectionPart::Separator,
                SectionPart::Row(2),
            ]
        );
        assert!(parts.first() != Some(&SectionPart::Separator));
        assert!(parts.last() != Some(&SectionPart::Separator));
    }

    #[test]
    fn separators_are_always_rows_minus_one() {
        for n in 0..12 {
            let seps = section_parts(n)
                .iter()
                .filter(|p| **p == SectionPart::Separator)
                .count();
            assert_eq!(seps, n.saturating_sub(1), "{n} rows");
        }
    }

    /// Headless GPUI render: a 1-row and a 3-row section draw without a panic
    /// and keep their part counts (the structure the window paints from).
    #[gpui::test]
    fn one_and_three_row_sections_render_headless(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let theme = crate::ui::theme::Theme::for_pref(
                crate::ui::theme::AppearancePref::Dark,
                crate::ui::theme::Surface::Matte,
                cx,
            );
            cx.set_global(theme);
        });
        struct Sections;
        impl gpui::Render for Sections {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                let theme = Theme::of(cx).clone();
                let row = |label: &str| {
                    div()
                        .h(px(ROW_MIN_H))
                        .px(px(INSET))
                        .child(label.to_string())
                        .into_any_element()
                };
                div()
                    .flex()
                    .flex_col()
                    .child(section(vec![row("Only")], INSET, &theme))
                    .child(section(
                        vec![row("One"), row("Two"), row("Three")],
                        INSET,
                        &theme,
                    ))
            }
        }
        let (_view, vcx) = cx.add_window_view(|_, _| Sections);
        vcx.run_until_parked();
        assert_eq!(section_parts(1).len(), 1);
        assert_eq!(section_parts(3).len(), 5);
    }
}
