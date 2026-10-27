//! The one hover / selected / pressed highlight on the Mac (Jacob, 2026-10-27,
//! [feedback row-hover-style-2026-10-27]): "a subtle inset highlight — a soft
//! low-contrast rounded pill inset a few points from the container edges,
//! corner radius matching the container's inner radius, never a hard
//! rectangle; selected = a slightly stronger fill of the same shape; pressed =
//! one step more" — Grok Bot's list rows. Before this, every list drew its
//! own: a `surface_2` box over the whole row, square, flush with the card.
//!
//! Three shapes, one set of fills (`docs/mac-design-rules.md` § Rows):
//!
//! * [`row`] with [`Shape::Nested`] — a full-bleed row inside a rounded card,
//!   sheet, menu plate or dropdown: the pill is a layer under the content,
//!   inset [`INSET`] from the row's box on every side, its radius the card's
//!   minus the inset ([`inner_radius`]: concentric with the card, never under
//!   [`MIN_RADIUS`]). The row's own box never gets a fill.
//! * [`row`] with [`Shape::Own`] — a row that already is the pill, measured on
//!   Grok (sidebar rows: inset 8, radius 10; palette rows; menu rows): the
//!   fills go on the row's own rounded box.
//! * [`button`] — a transparent rounded control (an icon disc, a text button, a
//!   tab): the same hover / pressed washes on its own shape. [`washed`] is the
//!   filled control's rule (the footer disc, pills on `surface_2` or `field`):
//!   the wash composited over the fill, a step under the hand, one more down.
//!
//! The fills are washes of the text colour, so one number reads the same on
//! the chat, the sidebar, a card, a frosted plate and on Glass. No other file
//! sets a hover or pressed background: `tests::no_hover_or_pressed_fill_outside_the_token`
//! scans `src/ui`; a line that must (a drag handle, an overlay on a photo)
//! says why with `// hover-fill: exception <kind>`.

use gpui::{div, prelude::*, px, Div, Hsla, SharedString, Stateful};

use crate::ui::theme::{Surface, Theme};

/// The pill's inset from the row's box (= from the card's edges) on all four
/// sides. Grok's rows sit a few points in from their card.
pub const INSET: f32 = 4.0;
/// The pill never gets sharper than this, whatever the card's radius.
pub const MIN_RADIUS: f32 = 6.0;
/// The radius a [`Shape::Own`] layer would take (unused by rows that are
/// their own pill, kept as the reference: Grok's sidebar row, `PANEL_RADIUS - 4`).
pub const FREE_RADIUS: f32 = 10.0;
/// The group name every highlighted row carries; the pill layer listens to it.
pub const GROUP: &str = "row-highlight";
/// The layer's debug selector, so a headless render can measure the pill.
pub const LAYER_SELECTOR: &str = "row-highlight-layer";

/// The pill's radius inside a container with the given corner radius:
/// concentric (container minus [`INSET`]), floored at [`MIN_RADIUS`].
pub fn inner_radius(container_radius: f32) -> f32 {
    (container_radius - INSET).max(MIN_RADIUS)
}

/// What the highlight sits in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// A full-bleed row inside a rounded container of this radius: the pill is
    /// a layer inset [`INSET`], radius [`inner_radius`].
    Nested(f32),
    /// The row's own rounded box is the pill (it already has its margins and
    /// radius); the fills go on the row itself.
    Own,
}

/// Hover, selected and pressed fills — one step apart, the same shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fills {
    pub hover: Hsla,
    pub selected: Hsla,
    pub pressed: Hsla,
}

impl Fills {
    /// The resting fill of a row: the selected step or nothing.
    pub fn rest(&self, selected: bool) -> Hsla {
        if selected {
            self.selected
        } else {
            gpui::transparent_black()
        }
    }

    /// The fill under the pointer: a selected row stays at its step.
    pub fn hovered(&self, selected: bool) -> Hsla {
        if selected {
            self.selected
        } else {
            self.hover
        }
    }
}

/// The alphas (hover, selected, pressed) of the text-colour wash per surface
/// and appearance. Glass is a touch stronger: the frosted backdrop is busier.
pub fn alphas(surface: Surface, light: bool) -> (f32, f32, f32) {
    match (surface, light) {
        (Surface::Matte, false) => (0.055, 0.09, 0.13),
        (Surface::Matte, true) => (0.045, 0.08, 0.12),
        (Surface::Glass, false) => (0.08, 0.12, 0.16),
        (Surface::Glass, true) => (0.06, 0.10, 0.14),
    }
}

/// The token for a theme.
pub fn fills(theme: &Theme) -> Fills {
    let (hover, selected, pressed) = alphas(theme.surface, theme.is_light());
    Fills {
        hover: Theme::with_alpha(theme.text, hover),
        selected: Theme::with_alpha(theme.text, selected),
        pressed: Theme::with_alpha(theme.text, pressed),
    }
}

/// The pill layer: absolute, inset, rounded, lit by the row's group. Add it
/// as the row's **first** child (it paints under what follows).
pub fn layer(selected: bool, shape: Shape, theme: &Theme) -> Stateful<Div> {
    let f = fills(theme);
    layer_with(f.rest(selected), f.hovered(selected), shape, theme)
}

/// The pill already lit at its hover step: for a row whose hover the view
/// tracks itself (the Library row arms a preview after a delay and draws the
/// armed row from state, also headless in the rig).
pub fn armed_layer(shape: Shape, theme: &Theme) -> Stateful<Div> {
    let f = fills(theme);
    layer_with(f.hover, f.hover, shape, theme)
}

fn layer_with(rest: Hsla, hovered: Hsla, shape: Shape, theme: &Theme) -> Stateful<Div> {
    let f = fills(theme);
    let radius = match shape {
        Shape::Nested(r) => inner_radius(r),
        Shape::Own => FREE_RADIUS,
    };
    div()
        .id(LAYER_SELECTOR)
        .debug_selector(|| LAYER_SELECTOR.into())
        .absolute()
        .inset(px(INSET))
        .rounded(px(radius))
        .bg(rest)
        .group_hover(GROUP, move |s| s.bg(hovered))
        .group_active(GROUP, move |s| s.bg(f.pressed))
}

/// Make `el` a highlighted row. Call it on the row's frame **before** its
/// content children are added: for [`Shape::Nested`] the pill layer becomes
/// the first child and the row the layer's group; for [`Shape::Own`] the
/// fills go on the row's own box.
pub fn row(el: Stateful<Div>, selected: bool, shape: Shape, theme: &Theme) -> Stateful<Div> {
    let f = fills(theme);
    match shape {
        Shape::Own => el
            .bg(f.rest(selected))
            .hover(move |s| s.bg(f.hovered(selected)))
            .active(move |s| s.bg(f.pressed)),
        Shape::Nested(_) => el
            .relative()
            .group(SharedString::from(GROUP))
            .child(layer(selected, shape, theme)),
    }
}

/// A transparent rounded control: the hover and pressed washes on its own
/// shape (an icon disc, a text button, a tab segment, a small ×).
pub fn button(el: Stateful<Div>, theme: &Theme) -> Stateful<Div> {
    let f = fills(theme);
    el.hover(move |s| s.bg(f.hover))
        .active(move |s| s.bg(f.pressed))
}

/// [`button`] with more hover styling (a glyph that brightens, a rim that
/// appears): GPUI keeps one hover refinement per element, so the extra goes
/// in with the wash.
pub fn button_with(
    el: Stateful<Div>,
    theme: &Theme,
    extra: impl FnOnce(gpui::StyleRefinement) -> gpui::StyleRefinement + 'static,
) -> Stateful<Div> {
    let f = fills(theme);
    el.hover(move |s| extra(s).bg(f.hover))
        .active(move |s| s.bg(f.pressed))
}

/// A segment of a pill switch (the pane's Details · Library tabs, the header's
/// chat tabs, the sidebar filter): the "on" segment is a filled control on
/// `on_fill` ([`washed`]), the others transparent with the washes.
pub fn segment(el: Stateful<Div>, on: bool, on_fill: Hsla, theme: &Theme) -> Stateful<Div> {
    if on {
        washed(el, on_fill, theme)
    } else {
        button(el, theme)
    }
}

/// A filled control (a disc or pill on `surface_2`, a `field` pill, the
/// footer's blue update disc): `base` at rest, the token's hover / pressed
/// wash composited over it under the pointer and while down — the same two
/// steps as a row, as an opaque colour. In dark this is the old chip rule
/// (the fill a shade lighter); in light the fill goes a shade darker, where
/// lifting `#E8E8E8` toward white used to vanish on the `#F6F6F6` sidebar.
pub fn washed(el: Stateful<Div>, base: Hsla, theme: &Theme) -> Stateful<Div> {
    let f = fills(theme);
    let hover = over(base, f.hover);
    let pressed = over(base, f.pressed);
    el.bg(base)
        .hover(move |s| s.bg(hover))
        .active(move |s| s.bg(pressed))
}

/// `wash` (translucent) composited over `base` the way GPUI paints it — a
/// straight sRGB "over" — as one colour. An opaque base gives an opaque
/// result; a translucent one (the glass circle's 85 % field) keeps most of
/// its translucency.
pub fn over(base: Hsla, wash: Hsla) -> Hsla {
    let (b, w) = (base.to_rgb(), wash.to_rgb());
    let a = w.a + b.a * (1.0 - w.a);
    if a <= 0.0 {
        return gpui::transparent_black();
    }
    let mix = |wc: f32, bc: f32| (wc * w.a + bc * b.a * (1.0 - w.a)) / a;
    gpui::Rgba {
        r: mix(w.r, b.r),
        g: mix(w.g, b.g),
        b: mix(w.b, b.b),
        a,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::{AppearancePref, Surface};
    use std::path::{Path, PathBuf};

    #[test]
    fn the_pill_is_concentric_with_its_card_and_never_sharper_than_the_floor() {
        assert_eq!(inner_radius(12.0), 8.0, "kit card 12 → 8");
        assert_eq!(inner_radius(14.0), 10.0, "PANEL_RADIUS 14 → 10");
        assert_eq!(inner_radius(10.0), MIN_RADIUS, "dropdown 10 → floor");
        assert_eq!(inner_radius(0.0), MIN_RADIUS);
        let inset = INSET;
        assert!((2.0..=8.0).contains(&inset), "a few points, not a margin");
    }

    #[test]
    fn selected_is_one_step_over_hover_and_pressed_one_more_on_every_surface() {
        for surface in [Surface::Matte, Surface::Glass] {
            for light in [false, true] {
                let (h, s, p) = alphas(surface, light);
                assert!(
                    h > 0.0 && h < s && s < p && p <= 0.2,
                    "{surface:?} light={light}"
                );
            }
        }
        // Glass reads a touch stronger than Matte, step for step.
        for light in [false, true] {
            let m = alphas(Surface::Matte, light);
            let g = alphas(Surface::Glass, light);
            assert!(g.0 > m.0 && g.1 > m.1 && g.2 > m.2);
        }
    }

    #[test]
    fn the_fills_are_washes_of_the_text_colour() {
        for theme in [
            Theme::dark(Surface::Matte),
            Theme::light(Surface::Matte),
            Theme::dark(Surface::Glass),
            Theme::light(Surface::Glass),
        ] {
            let f = fills(&theme);
            for c in [f.hover, f.selected, f.pressed] {
                assert_eq!((c.h, c.s, c.l), (theme.text.h, theme.text.s, theme.text.l));
                assert!(c.a > 0.0 && c.a < 1.0);
            }
            assert_eq!(f.rest(false).a, 0.0);
            assert_eq!(f.rest(true), f.selected);
            assert_eq!(f.hovered(true), f.selected, "a selected row keeps its step");
            assert_eq!(f.hovered(false), f.hover);
        }
    }

    // ----- the drift guard ----------------------------------------------------

    fn sources() -> Vec<(PathBuf, String)> {
        fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
            for entry in std::fs::read_dir(dir).expect("ui dir") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).expect("read source");
                    out.push((path, text));
                }
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let mut out = Vec::new();
        walk(&root, &mut out);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn rel(path: &Path) -> String {
        path.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
            .unwrap_or(path)
            .display()
            .to_string()
    }

    /// The reasons a line may set a hover / pressed background by hand. A
    /// marker with any other word fails the lint like a missing one.
    const EXCEPTION_KINDS: &[&str] = &[
        // A resize / drag handle: a hairline that lights, not a row.
        "handle",
        // A control painted over a photo or video (white / black washes).
        "overlay",
        // A row the sibling item removes ([feedback add-agent-row-removal]);
        // not restyled so the two branches merge (coordinator, 2026-10-27).
        "removed-by-sibling",
        // Code-card copy button: its own four-state copied / hover / down look.
        "code-card",
        // A destructive label that turns red under the pointer (`danger_soft`).
        "danger",
        // The approval card's option rows: the card's own palette (its
        // keyboard highlight and hover are one colour, `CardPalette.highlight`).
        "card-option",
        // A chosen option in a form — a picker chip, a segmented control's
        // on segment with its own palette — not a list row's selection.
        "choice",
    ];

    fn marker(line: &str) -> Option<&str> {
        let idx = line.find("// hover-fill: exception ")?;
        let kind = line[idx + "// hover-fill: exception ".len()..]
            .split_whitespace()
            .next()
            .unwrap_or("");
        Some(kind)
    }

    /// The "Add agent" / "Add your agent" rows and the add-agent popover are
    /// being removed by a sibling item ([feedback add-agent-row-removal],
    /// 2026-10-27); their lines stay as they are so the two branches merge.
    /// Drop this once that item has landed.
    fn removed_by_sibling(lines: &[&str], i: usize) -> bool {
        let from = i.saturating_sub(16);
        lines[from..=i].iter().any(|l| {
            l.contains("\"Add agent\"")
                || l.contains("\"Add your agent\"")
                || l.contains("add-agent-")
        })
    }

    fn excepted(lines: &[&str], i: usize) -> Result<bool, String> {
        let here = marker(lines[i]);
        let above = (i > 0).then(|| marker(lines[i - 1].trim_start())).flatten();
        match here.or(above) {
            None => Ok(removed_by_sibling(lines, i)),
            Some(kind) if EXCEPTION_KINDS.contains(&kind) => Ok(true),
            Some(kind) => Err(format!("unknown exception kind '{kind}'")),
        }
    }

    /// A hover / pressed / selected background set by hand: `.hover(|s| s.bg(`,
    /// `.active(|s| s.bg(`, `group_hover(…, |s| s.bg(`, and the state fills
    /// `.when(selected | is_selected | hovered | open | i == selected, |el| el.bg(`.
    fn is_hand_fill(line: &str) -> bool {
        let flat: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        // The text of a call's arguments: from `head` to its matching `)` (or
        // the line's end when the call continues on the next line).
        let call_args = |head: &str| -> Option<String> {
            let at = flat.find(head)?;
            let body = &flat[at + head.len()..];
            let mut depth = 1usize;
            let mut end = body.len();
            for (i, c) in body.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            Some(body[..end].to_owned())
        };
        let closure_fill = |head: &str| {
            call_args(head).is_some_and(|args| args.contains('|') && args.contains(".bg("))
        };
        if closure_fill(".hover(")
            || closure_fill(".active(")
            || closure_fill("group_hover(")
            || closure_fill("group_active(")
        {
            return true;
        }
        let state_heads = [
            ".when(selected,",
            ".when(is_selected,",
            ".when(hovered,",
            ".when(open,",
            ".when(i==selected,",
            ".when(menu_here,",
        ];
        if state_heads
            .iter()
            .any(|h| call_args(h).is_some_and(|args| args.contains(".bg(")))
        {
            return true;
        }
        // `.bg(if selected { surface_2 } else { transparent })` and friends —
        // a selected / open / hovered fill chosen inline.
        [
            ".bg(ifselected",
            ".bg(ifis_selected",
            ".bg(ifis_active",
            ".bg(ifhovered",
            ".bg(ifopen",
            ".bg(ifmenu_here",
        ]
        .iter()
        .any(|h| flat.contains(h))
    }

    /// Every hover, pressed and selected fill in `src/ui` comes from this file
    /// ([`row`], [`button`], [`button_with`], [`segment`], [`washed`]). A line that
    /// sets one by hand fails the gate unless it (or the line above) carries
    /// `// hover-fill: exception <kind>` with a kind from [`EXCEPTION_KINDS`].
    #[test]
    fn no_hover_or_pressed_fill_outside_the_token() {
        let mut bad = Vec::new();
        for (path, text) in sources() {
            if path.ends_with("highlight.rs") {
                continue;
            }
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if !is_hand_fill(line) {
                    continue;
                }
                match excepted(&lines, i) {
                    Ok(true) => {}
                    Ok(false) => bad.push(format!("{}:{}: {}", rel(&path), i + 1, line.trim())),
                    Err(why) => {
                        bad.push(format!("{}:{}: {why}: {}", rel(&path), i + 1, line.trim()))
                    }
                }
            }
        }
        assert!(
            bad.is_empty(),
            "hover / pressed / selected fills come from ui::highlight (row / button / segment / washed); \
             a line that must set its own says why with `// hover-fill: exception <kind>`:\n{}",
            bad.join("\n")
        );
    }

    #[test]
    fn the_lint_recognises_the_hand_fill_shapes() {
        assert!(is_hand_fill(".hover(|s| s.bg(theme.surface_2))"));
        assert!(is_hand_fill(".hover(|st| st.bg(theme.surface_2))"));
        assert!(is_hand_fill(".hover(move |s| s.bg(hover))"));
        assert!(is_hand_fill(
            ".hover(|s| s.text_color(theme.text).bg(theme.surface_2))"
        ));
        assert!(is_hand_fill(".bg(if selected && !hide_morphing {"));
        assert!(is_hand_fill(
            ".active(|s| s.bg(Theme::with_alpha(theme.text, 0.22)))"
        ));
        assert!(is_hand_fill(".when(selected, |el| el.bg(theme.surface_2))"));
        assert!(is_hand_fill(".when(hovered, |el| el.bg(theme.surface_2))"));
        assert!(is_hand_fill(
            ".group_hover(group, |s| s.bg(theme.surface_2))"
        ));
        assert!(!is_hand_fill(".hover(|s| s.opacity(0.85))"));
        assert!(!is_hand_fill(".hover(|s| s.text_color(theme.text))"));
        assert!(!is_hand_fill(".group_hover(group, |s| s.opacity(1.0))"));
        assert!(!is_hand_fill(
            ".when(selected, |el| el.font_weight(FontWeight::MEDIUM))"
        ));
        assert!(!is_hand_fill(
            "highlight::row(el, selected, Shape::Nested(12.0), theme)"
        ));
        assert!(
            excepted(
                &["// hover-fill: exception handle", ".hover(|s| s.bg(x))"],
                1
            ) == Ok(true)
        );
        assert!(excepted(&[".hover(|s| s.bg(x)) // hover-fill: exception overlay"], 0) == Ok(true));
        assert!(excepted(&[".hover(|s| s.bg(x)) // hover-fill: exception because"], 0).is_err());
    }

    // ----- headless render: the pill's geometry ------------------------------

    struct Probe {
        shape: Shape,
        selected: bool,
    }

    impl gpui::Render for Probe {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            let frame = div()
                .id("probe-row")
                .debug_selector(|| "probe-row".into())
                .flex()
                .items_center()
                .h(px(44.0))
                .px(px(16.0));
            let row = row(frame, self.selected, self.shape, &theme).child("Members");
            div().w(px(320.0)).h(px(200.0)).flex().flex_col().child(
                div()
                    .rounded(px(12.0))
                    .overflow_hidden()
                    .debug_selector(|| "probe-card".into())
                    .child(row),
            )
        }
    }

    /// The pill under a nested row is inset [`INSET`] from the row's box on
    /// all four sides — never flush with the card — and the row itself has
    /// no background of its own.
    #[gpui::test]
    fn the_nested_pill_sits_inset_from_every_edge_of_the_row(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let theme = Theme::for_pref(AppearancePref::Dark, Surface::Matte, cx);
            cx.set_global(theme);
        });
        let f = |p: gpui::Pixels| -> f32 { p.into() };
        for selected in [false, true] {
            let (_view, vcx) = cx.add_window_view(|_, _| Probe {
                shape: Shape::Nested(12.0),
                selected,
            });
            vcx.run_until_parked();
            let row_b = vcx.debug_bounds("probe-row").expect("row");
            let pill = vcx.debug_bounds(LAYER_SELECTOR).expect("pill layer");
            assert_eq!(f(pill.origin.x) - f(row_b.origin.x), INSET, "left inset");
            assert_eq!(f(pill.origin.y) - f(row_b.origin.y), INSET, "top inset");
            assert_eq!(f(row_b.right()) - f(pill.right()), INSET, "right inset");
            assert_eq!(f(row_b.bottom()) - f(pill.bottom()), INSET, "bottom inset");
            let card = vcx.debug_bounds("probe-card").expect("card");
            assert!(
                f(pill.origin.x) > f(card.origin.x),
                "never flush with the card"
            );
        }
    }
}
