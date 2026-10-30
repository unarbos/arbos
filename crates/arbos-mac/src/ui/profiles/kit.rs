//! The one row/card kit every right pane draws with (Jacob, 2026-10-20:
//! "make the settings, user and group panels consistent"). Settings cards,
//! member lists, group lists, file lists, Block user and Sign out all come
//! from here, so a metric lives in exactly one place:
//!
//! * card: `CARD_RADIUS` corners, `pane_card` fill (dark: the D80 grey, the
//!   iPhone's `PaneCard`; light: a grey card on the white pane), 16 pt side
//!   margins, a `section_title` over it where Grok labels a card;
//! * row: 41 pt minimum (51 with a subtitle), 16 pt inset, 12 pt gap,
//!   hairline between rows inset 16 at both ends — the row's own padding
//!   (Jacob, 2026-10-26: a divider that starts under the label but runs to
//!   the card's right edge reads lopsided);
//! * leading: a 30 pt icon tile (settings), a 36 pt avatar (people, agents,
//!   groups) or nothing;
//! * detail: `TEXT_SMALL` muted, right-aligned; subtitle: `TEXT_SMALL` muted
//!   under the title; chevron 13 pt faint when the row pushes a page;
//! * danger row: centred red label, same height.

use gpui::{
    div, prelude::*, px, AnyElement, BoxShadow, Div, ElementId, FontWeight, Hsla, SharedString,
    Stateful,
};

use crate::ui::glass;
use crate::ui::highlight::{self, Shape};
use crate::ui::identity;
use crate::ui::sf_symbols;
use crate::ui::theme::{Theme, CONTROL, TEXT_BODY, TEXT_SMALL, TEXT_TINY};

/// The group a clickable row joins so its highlight pill lights: every
/// [`row`] carries the pill as its first child; `.group(HOT)` (plus
/// `cursor_pointer()`) is what makes it react. A value row never joins.
pub const HOT: &str = highlight::GROUP;

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
/// The divider between two grouped rows stops this far from the card's
/// edges — **the same at both ends**, and it is the row's content padding
/// ([feedback mac-settings-card-dividers-2026-10-26]). Before, the line
/// started under the label (past the icon tile or avatar) and ran to the
/// card's right edge; on the agent's Settings card Jacob read that as
/// lopsided. One number for every grouped list, whatever leads the row.
pub const DIVIDER_INSET: f32 = INSET;

/// `(leading, trailing)` inset of a grouped row's divider. Pure, so the rule
/// — equal at both ends and equal to the row padding — is testable without
/// a window; [`hairline_below`] and [`section`] draw from it.
pub fn divider_insets() -> (f32, f32) {
    (DIVIDER_INSET, DIVIDER_INSET)
}

/// The debug selector every shared divider carries, so a headless render
/// can measure it against its card.
pub const DIVIDER_SELECTOR: &str = "grouped-row-divider";

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
        .bg(theme.pane_card_fill())
        // The Glass rim: an inset shadow, so it costs the rows no layout
        // (a border would shrink the card's content box by 1 pt a side);
        // transparent on Matte.
        .shadow(vec![BoxShadow::new(
            px(0.0),
            px(0.0),
            theme.pane_card_rim(),
        )
        .spread_radius(px(1.0))
        .inset()])
        .overflow_hidden()
}

/// The one separator every grouped row uses: a 1 pt hairline hung 1 pt
/// *below* the row's box (`.relative()` on the row), inset [`DIVIDER_INSET`]
/// from both card edges. Between rows it lands on the next row's first pixel;
/// under the last row the card's clip swallows it, and nothing sits above the
/// first — separators only between rows, inside the card (Jacob, 2026-10-21).
/// Drawn as a bottom border it would stay inside the box and show under a
/// card's last row. No per-caller inset: every grouped list in the app draws
/// the same line, so no card can drift back to a lopsided one.
pub fn hairline_below(theme: &Theme) -> Div {
    let (leading, trailing) = divider_insets();
    div()
        .absolute()
        .left(px(leading))
        .right(px(trailing))
        .bottom(px(-1.0))
        .h(px(1.0))
        .bg(theme.border)
        .debug_selector(|| DIVIDER_SELECTOR.into())
}

/// The same line as an in-flow element, for a card that lays its own rows
/// out and puts the separator between two of them itself (the New message
/// and New channel sheets, [`section`]). Same insets, same colour, same
/// selector — a card never draws a divider of its own.
pub fn divider(theme: &Theme) -> Div {
    let (leading, trailing) = divider_insets();
    div()
        .flex_shrink_0()
        .ml(px(leading))
        .mr(px(trailing))
        .h(px(1.0))
        .bg(theme.border)
        .debug_selector(|| DIVIDER_SELECTOR.into())
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
/// content, custom rows) get exactly `rows - 1` lines, inside the card, at
/// the same insets as [`hairline_below`].
pub fn section(rows: Vec<AnyElement>, theme: &Theme) -> Div {
    let mut rows: Vec<Option<AnyElement>> = rows.into_iter().map(Some).collect();
    let mut out = card(theme);
    for part in section_parts(rows.len()) {
        out = match part {
            SectionPart::Row(i) => out.children(rows[i].take()),
            SectionPart::Separator => out.child(divider(theme)),
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
                .bg(theme.control_fill())
                .flex()
                .items_center()
                .justify_center()
                .child(sf_symbols::glyph(glyph, 18.0, theme.label_muted()))
                .into_any_element(),
        ),
    }
}

/// One row: leading, title (+ optional subtitle), optional right detail,
/// optional trailing element, chevron when it pushes a page. The hairline
/// hangs 1 pt below the row at the row's padding on both sides: between rows
/// it lands on the next row's first pixel, under the last row the card's clip
/// swallows it (D80 / the iPhone's `PaneCardLayout`: no line under the last
/// row). The highlight pill ([`highlight::row`], inset 4, radius
/// `CARD_RADIUS - 4`) is the row's first child; it lights for rows that join
/// [`HOT`].
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
    row_state(
        id, leading, title, subtitle, detail, trailing, chevron, false, theme,
    )
}

/// [`row`] with a selected state: the pill at its selected step at rest (the
/// open hoverpick anchor, the server pane's current channel).
#[allow(clippy::too_many_arguments)]
pub fn row_state(
    id: impl Into<ElementId>,
    leading: Leading,
    title: impl Into<SharedString>,
    subtitle: Option<SharedString>,
    detail: Option<SharedString>,
    trailing: Option<AnyElement>,
    chevron: bool,
    selected: bool,
    theme: &Theme,
) -> Stateful<Div> {
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
        // First child: paints under the content; lights only for `HOT` rows.
        .child(highlight::layer(
            selected,
            Shape::Nested(CARD_RADIUS),
            theme,
        ))
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
                            .text_color(theme.label_muted())
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
                    .text_color(theme.label_muted())
                    .child(d),
            )
        })
        .children(trailing)
        .when(chevron, |el| {
            el.child(sf_symbols::glyph(
                "chevron.right",
                13.0,
                theme.label_faint(),
            ))
        })
        .child(hairline_below(theme))
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
    .group(HOT)
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
    let frame = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .min_h(px(ROW_MIN_H))
        .px(px(INSET))
        .text_size(px(TEXT_BODY))
        .text_color(color)
        .cursor_pointer();
    highlight::row(frame, false, Shape::Nested(CARD_RADIUS), theme).child(label.to_string())
}

/// The hero's action pill ("Message" on a person's pane,
/// [feedback profile-message-button-mac-2026-10-25]): one of the Grok button
/// family — [`CONTROL`] 36 tall, a capsule on the discs' `surface_2` fill,
/// SF Symbol (the iPhone's `PanePillButton(glyph:)` name, semibold at the
/// label's size) and 13 pt semibold label in `text`; hover and pressed are the
/// highlight token's two steps over the fill (`highlight::washed`).
pub const PILL_PAD_X: f32 = 18.0;
pub const PILL_GLYPH: f32 = TEXT_SMALL;
pub const PILL_GLYPH_SLOT: f32 = 18.0;
pub const PILL_GAP: f32 = 7.0;

pub fn pill_button(
    id: &'static str,
    symbol: &'static str,
    label: &str,
    theme: &Theme,
) -> Stateful<Div> {
    let frame = div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .h(px(CONTROL))
        .px(px(PILL_PAD_X))
        .flex_shrink_0()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(PILL_GAP))
        .text_size(px(TEXT_SMALL))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text)
        .cursor_pointer();
    highlight::washed(frame, theme.control_fill(), theme)
        .child(sf_symbols::symbol(
            symbol,
            PILL_GLYPH,
            sf_symbols::Weight::Semibold,
            PILL_GLYPH_SLOT,
            theme.text,
        ))
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
        .text_color(theme.label_muted())
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
        .text_color(theme.label_muted())
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
        .text_color(theme.label_muted())
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
            highlight::washed(
                el.border_1()
                    .border_color(theme.field_border)
                    .text_color(theme.text),
                theme.field_fill(),
                theme,
            )
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
        .child(hairline_below(theme))
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

    /// [feedback mac-settings-card-dividers-2026-10-26]: the divider is inset
    /// the same at both ends, and that inset is the row's content padding.
    /// Before, it started under the label and ran to the card's right edge.
    #[test]
    fn the_divider_is_inset_equally_at_both_ends_by_the_row_padding() {
        let (leading, trailing) = divider_insets();
        assert_eq!(leading, trailing, "leading inset == trailing inset");
        assert_eq!(leading, INSET, "divider inset == row content padding");
        assert_eq!(DIVIDER_INSET, INSET);
    }

    /// Every grouped card draws the kit's divider, never one of its own.
    /// Scans the Mac UI sources for the hand-rolled idioms this fix removed:
    /// a hairline with only a left margin (`.ml(px(..)) … .bg(theme.border)`),
    /// an absolute hairline pinned `left(..)` / `right(0)`, the 0.5 pt sheet
    /// line, and a list row's `border_b_1()` under `i + 1 < count`.
    #[test]
    fn no_card_outside_the_kit_draws_its_own_divider() {
        let ui = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let mut files = Vec::new();
        collect_rs(&ui, &mut files);
        assert!(files.len() > 20, "found {} ui sources", files.len());
        let mut offenders = Vec::new();
        for path in files {
            if path.ends_with("profiles/kit.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("read ui source");
            let flat: String = src.chars().filter(|c| !c.is_whitespace()).collect();
            let rel = path
                .strip_prefix(&ui)
                .unwrap_or(&path)
                .display()
                .to_string();
            for (idx, _) in flat.match_indices(".bg(theme.border)") {
                let back = &flat[idx.saturating_sub(140)..idx];
                let left_only = back.contains(".ml(px(") && !back.contains(".mr(px(");
                let pinned = back.contains(".left(px(") && back.contains(".right(px(0.0))");
                let half_point = back.contains("h(px(0.5))");
                if left_only || pinned || half_point {
                    offenders.push(format!("{rel}: hairline at offset {idx}"));
                }
            }
            if flat.contains("<count,|el|{el.border_b_1()")
                || flat.contains("<count,|el|el.border_b_1()")
            {
                offenders.push(format!("{rel}: list row with its own bottom border"));
            }
        }
        assert!(
            offenders.is_empty(),
            "cards must use kit::hairline_below / kit::divider:\n{}",
            offenders.join("\n")
        );
    }

    fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read ui dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                collect_rs(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    #[derive(Clone, Copy)]
    enum Probe {
        TileRow,
        AvatarRow,
        GlassRow,
        Section,
    }

    struct DividerCard(Probe);

    impl gpui::Render for DividerCard {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            let plain = |label: &str| {
                div()
                    .h(px(ROW_MIN_H))
                    .px(px(INSET))
                    .child(label.to_string())
                    .into_any_element()
            };
            let kit_row = |leading: Leading| {
                row(
                    "probe",
                    leading,
                    "Model",
                    None,
                    Some("Low".into()),
                    None,
                    true,
                    &theme,
                )
                .into_any_element()
            };
            let card = match self.0 {
                Probe::TileRow => card(&theme).child(kit_row(Leading::Tile("gearshape"))),
                Probe::AvatarRow => card(&theme).child(kit_row(Leading::Avatar(
                    div().size(px(AVATAR)).into_any_element(),
                ))),
                Probe::GlassRow => card(&theme).child(glass::row("Credits", &theme)),
                Probe::Section => section(vec![plain("One"), plain("Two")], &theme),
            };
            div()
                .w(px(320.0))
                .h(px(300.0))
                .flex()
                .flex_col()
                .child(card.debug_selector(|| "divider-probe-card".into()))
        }
    }

    /// Headless render of every divider the kit draws — under a tile row, an
    /// avatar row, a `glass::row` and between `section` rows: each line stops
    /// `INSET` short of the card's left edge **and** of its right edge, and is
    /// 1 pt tall. Before this fix the tile row's line started at 58 and the
    /// rest at 16, and all of them ran to the card's right edge.
    #[gpui::test]
    fn every_shared_divider_stops_at_the_row_padding_on_both_sides(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let theme = crate::ui::theme::Theme::for_pref(
                crate::ui::theme::AppearancePref::Dark,
                crate::ui::theme::Surface::Matte,
                cx,
            );
            cx.set_global(theme);
        });
        let f = |p: gpui::Pixels| -> f32 { p.into() };
        for (name, probe) in [
            ("tile row", Probe::TileRow),
            ("avatar row", Probe::AvatarRow),
            ("glass row", Probe::GlassRow),
            ("section", Probe::Section),
        ] {
            let (_view, vcx) = cx.add_window_view(|_, _| DividerCard(probe));
            vcx.run_until_parked();
            let card = vcx.debug_bounds("divider-probe-card").expect("card");
            let line = vcx.debug_bounds(DIVIDER_SELECTOR).expect("divider");
            let leading = f(line.origin.x) - f(card.origin.x);
            let trailing = f(card.right()) - f(line.right());
            assert_eq!(leading, INSET, "{name}: leading inset");
            assert_eq!(trailing, INSET, "{name}: trailing inset");
            assert_eq!(leading, trailing, "{name}: symmetric");
            assert_eq!(f(line.size.height), 1.0, "{name}: 1 pt line");
        }
    }

    /// A member row (avatar, name over `@handle`) for one name.
    struct MemberRow(&'static str);

    impl gpui::Render for MemberRow {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            let subtitle = div()
                .debug_selector(|| "member-handle".into())
                .child("@dawizzaa")
                .into_any_element();
            div().w(px(340.0)).h(px(120.0)).child(
                card(&theme).debug_selector(|| "member-card".into()).child(
                    row(
                        "member",
                        Leading::Avatar(div().size(px(AVATAR)).into_any_element()),
                        self.0,
                        Some("@dawizzaa".into()),
                        None,
                        Some(subtitle),
                        false,
                        &theme,
                    )
                    .debug_selector(|| "member-row".into()),
                ),
            )
        }
    }

    /// [feedback emoji-name-handle-offset-mac-2026-10-28]: a member row whose
    /// name is one emoji is as tall as a text name's row, its `@handle` sits at
    /// the same place in it ([feedback emoji-name-baseline-2026-10-29]: no lift).
    #[gpui::test]
    fn an_emoji_member_name_keeps_the_row_and_its_handle_in_place(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let theme = crate::ui::theme::Theme::for_pref(
                crate::ui::theme::AppearancePref::Dark,
                crate::ui::theme::Surface::Matte,
                cx,
            );
            cx.set_global(theme);
        });
        let lay = |cx: &mut gpui::TestAppContext, name: &'static str| {
            let (_view, vcx) = cx.add_window_view(|_, _| MemberRow(name));
            vcx.run_until_parked();
            let row = vcx.debug_bounds("member-row").expect("row");
            let handle = vcx.debug_bounds("member-handle").expect("handle");
            let divider = vcx.debug_bounds(DIVIDER_SELECTOR).expect("divider");
            (
                row.size,
                handle.origin.y - row.origin.y,
                divider.origin.y - row.origin.y,
            )
        };
        let text = lay(cx, "Sam Rivera");
        let emoji = lay(cx, "🧙🏽\u{200d}♂\u{fe0f}");
        let mixed = lay(cx, "Cato 🌳");
        assert_eq!(emoji.0, text.0, "row size");
        assert_eq!(emoji.1, text.1, "handle offset in the row");
        assert_eq!(emoji.2, text.2, "divider offset in the row");
        assert_eq!(mixed.0, text.0, "row size (mixed)");
        assert_eq!(mixed.1, text.1, "handle offset in the row (mixed)");
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
                    .child(section(vec![row("Only")], &theme))
                    .child(section(vec![row("One"), row("Two"), row("Three")], &theme))
            }
        }
        let (_view, vcx) = cx.add_window_view(|_, _| Sections);
        vcx.run_until_parked();
        assert_eq!(section_parts(1).len(), 1);
        assert_eq!(section_parts(3).len(), 5);
    }
}
