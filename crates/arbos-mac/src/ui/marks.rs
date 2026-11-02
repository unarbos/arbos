//! Status marks that are not coloured dots (Jacob, 2026-10-27, [feedback
//! neon-dots-mac-2026-10-27]: "replace all neon glimmer icons like this in the
//! app with braille spinners always, on a matter of principle never use
//! colored dots"). Activity → the braille spinner (`ui::working`); unread →
//! the count pill; a fault → the red `exclamation` glyph; a resting state
//! (awake, asleep, read) → nothing. The source lint (`lint_tests`) refuses a
//! new small round `div` filled with a status colour anywhere in this crate.

use gpui::{
    div, prelude::*, px, AnyElement, App, Bounds, Div, Element, ElementId, FontWeight,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Window,
};

use crate::model::MachineState;
use crate::ui::sf_symbols;
use crate::ui::theme::Theme;
use crate::ui::working;

/// The count pill: an accent capsule with white digits (Grok Bot's unread
/// mark). `UNREAD_PILL_H` tall, at least as wide as it is tall, "99+" past 99.
pub const UNREAD_PILL_H: f32 = 16.0;
pub const UNREAD_PILL_TEXT: f32 = 10.5;

pub fn unread_label(count: usize) -> String {
    if count > 99 {
        "99+".to_string()
    } else {
        count.max(1).to_string()
    }
}

pub fn unread_pill(count: usize, theme: &Theme) -> Div {
    div()
        .h(px(UNREAD_PILL_H))
        .min_w(px(UNREAD_PILL_H))
        .px(px(5.0))
        .rounded_full()
        .bg(theme.accent)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(UNREAD_PILL_TEXT))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(gpui::white())
        .debug_selector(|| "unread-pill".into())
        .child(unread_label(count))
}

/// The unread-mention mark: [`unread_pill`]'s capsule with `@` where the
/// digits go (Jacob, 2026-10-31). It carries no number — the count lives on
/// the transcript's jump capsule, where it cannot be read as arithmetic
/// against the message count beside it.
///
/// `@` is typed, not an icon: the symbol pack has no `at` glyph.
pub fn mention_pill(theme: &Theme) -> Div {
    div()
        .h(px(UNREAD_PILL_H))
        .min_w(px(UNREAD_PILL_H))
        .px(px(5.0))
        .rounded_full()
        .bg(theme.accent)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(UNREAD_PILL_TEXT))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(gpui::white())
        .debug_selector(|| "mention-pill".into())
        .child("@")
}

/// What a machine state shows instead of its old coloured dot: the spinner
/// while it wakes, the red `!` glyph on a fault, nothing at rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineMark {
    Working,
    Fault,
    None,
}

pub fn machine_mark(state: MachineState) -> MachineMark {
    match state {
        MachineState::Waking => MachineMark::Working,
        MachineState::Error => MachineMark::Fault,
        MachineState::Asleep
        | MachineState::Awake
        | MachineState::Stopped
        | MachineState::Unknown => MachineMark::None,
    }
}

/// The fault glyph at `size` (the red exclamation, the error note's mark).
pub fn fault_glyph(size: f32, theme: &Theme) -> gpui::Div {
    sf_symbols::glyph("exclamationmark.circle", size, theme.danger)
}

/// The machine mark drawn: `None` when the state is at rest, so a corner slot
/// can collapse entirely.
pub fn machine_mark_element(
    state: MachineState,
    glyph_size: f32,
    theme: &Theme,
    cx: &mut App,
) -> Option<AnyElement> {
    match machine_mark(state) {
        MachineMark::Working => Some(working::glyph(theme, cx).into_any_element()),
        MachineMark::Fault => Some(fault_glyph(glyph_size, theme).into_any_element()),
        MachineMark::None => None,
    }
}

/// The mark's badge on a picture: how far past the picture's bottom-right
/// corner it hangs, and the ring (a disc of the surface colour) around it.
/// Sizes follow the picture: the glyph a quarter of it (6–12), the ring 2 pt
/// wider on each side — Grok's 12-on-48 badge, 9-on-36 on the footer disc.
pub const CORNER_OVERHANG: f32 = 2.0;
pub const CORNER_RING_WIDTH: f32 = 2.0;
pub const CORNER_GLYPH_MIN: f32 = 6.0;
pub const CORNER_GLYPH_MAX: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CornerMetrics {
    pub glyph: f32,
    pub ring: f32,
    pub overhang: f32,
}

pub fn corner_metrics(picture: f32) -> CornerMetrics {
    let glyph = (picture / 4.0)
        .round()
        .clamp(CORNER_GLYPH_MIN, CORNER_GLYPH_MAX);
    CornerMetrics {
        glyph,
        ring: glyph + 2.0 * CORNER_RING_WIDTH,
        overhang: CORNER_OVERHANG,
    }
}

/// `picture` in a `size` square, bare — the same box a marked picture has,
/// so a row's layout does not move when the mark comes and goes.
pub fn plain(size: f32, picture: impl IntoElement) -> Div {
    div()
        .relative()
        .size(px(size))
        .flex_shrink_0()
        .child(picture)
}

/// `picture` in a `size` square with the machine mark for `state` hanging
/// past its bottom-right corner in a ring of `ring_color`, **in front of** the
/// picture; the bare picture when the state is at rest. `ring` lets the
/// caller refine the ring disc (a hover fill, a debug selector).
pub fn machine_corner(
    size: f32,
    picture: impl IntoElement,
    state: MachineState,
    ring_color: Hsla,
    ring: impl FnOnce(Div) -> Div,
    theme: &Theme,
    cx: &mut App,
) -> Div {
    let m = corner_metrics(size);
    match machine_mark_element(state, m.glyph, theme, cx) {
        Some(mark) => corner(
            size,
            picture,
            ring(ring_disc(m.ring, ring_color).child(mark)),
            m.overhang,
        ),
        None => plain(size, picture),
    }
}

/// A ring disc of the surface colour, the mark centred in it.
pub fn ring_disc(size: f32, color: Hsla) -> Div {
    div()
        .size(px(size))
        .rounded_full()
        .bg(color)
        .flex()
        .items_center()
        .justify_center()
}

/// `picture` in a `size` square with `overlay` hanging `overhang` past its
/// bottom-right corner, painted in a layer of its own so it lands in front of
/// the picture whatever kind of primitive the picture is (a photo is a
/// polychrome sprite and is drawn after plain quads inside one layer — a
/// sibling div would end up under it; see [`Above`]).
pub fn corner(
    size: f32,
    picture: impl IntoElement,
    overlay: impl IntoElement,
    overhang: f32,
) -> Div {
    plain(size, picture).child(above(
        div()
            .absolute()
            .right(px(-overhang))
            .bottom(px(-overhang))
            .child(overlay),
    ))
}

/// `child`, painted in a layer of its own: above everything the enclosing
/// layer paints under it. Inside one GPUI layer every primitive shares one
/// draw order and the renderer batches equal orders by kind (quads, then
/// paths / text / icons, then photos), so a mark drawn as a sibling *after* a
/// photo still lands under it; a fresh layer gets an order above everything
/// already painted under its bounds. The child is laid out as if it stood
/// here itself.
pub fn above(child: impl IntoElement) -> Above {
    Above {
        child: child.into_any_element(),
    }
}

pub struct Above {
    child: AnyElement,
}

impl IntoElement for Above {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Above {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.paint_layer(bounds, |window| self.child.paint(window, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_mark_is_spinner_fault_or_nothing() {
        assert_eq!(machine_mark(MachineState::Waking), MachineMark::Working);
        assert_eq!(machine_mark(MachineState::Error), MachineMark::Fault);
        for s in [
            MachineState::Awake,
            MachineState::Asleep,
            MachineState::Stopped,
            MachineState::Unknown,
        ] {
            assert_eq!(machine_mark(s), MachineMark::None, "{s:?} shows no mark");
        }
    }

    #[test]
    fn the_corner_mark_scales_with_the_picture() {
        assert_eq!(
            corner_metrics(48.0).glyph,
            12.0,
            "Grok's 12 on a 48 pt row picture"
        );
        assert_eq!(
            corner_metrics(36.0).glyph,
            9.0,
            "9 on the 36 pt footer disc"
        );
        assert_eq!(corner_metrics(22.0).glyph, 6.0, "never under 6");
        assert_eq!(corner_metrics(96.0).glyph, 12.0, "never over 12");
        let m = corner_metrics(36.0);
        assert_eq!(m.ring, m.glyph + 2.0 * CORNER_RING_WIDTH);
        assert_eq!(m.overhang, CORNER_OVERHANG);
    }

    #[test]
    fn the_unread_label_caps_at_ninety_nine() {
        assert_eq!(unread_label(1), "1");
        assert_eq!(unread_label(0), "1", "a lit flag with no count reads 1");
        assert_eq!(unread_label(99), "99");
        assert_eq!(unread_label(100), "99+");
    }
}

/// The source lint: no coloured status dot may be drawn in this crate. A
/// "status dot" is a small round `div` (`.size(px(≤ 12))` + `.rounded_full()`)
/// filled with a status colour (`online`, `warn`, `danger`, `accent`, a machine
/// dot colour, or a pulsing alpha of one). Legend swatches and other
/// non-status discs carry `// status-dot: exception <why>` on the `.bg(` line.
#[cfg(test)]
mod lint_tests {
    use std::path::Path;

    const SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src");

    fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read src") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs")
                && path.file_name().is_some_and(|n| n != "marks.rs")
            {
                // (this file holds the lint and its sample dot)
                out.push(path);
            }
        }
    }

    fn small_size(line: &str) -> bool {
        let Some(ix) = line.find(".size(px(") else {
            return false;
        };
        let rest = &line[ix + ".size(px(".len()..];
        let num: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        num.parse::<f32>().is_ok_and(|n| n <= 12.0)
    }

    fn status_fill(line: &str) -> bool {
        let Some(ix) = line.find(".bg(") else {
            return false;
        };
        let fill = &line[ix..];
        [
            "theme.online",
            "theme.warn",
            "theme.danger",
            "theme.accent",
            "machine_dot(",
            "theme.unread",
        ]
        .iter()
        .any(|needle| fill.contains(needle))
    }

    pub(super) fn offenders() -> Vec<String> {
        let mut files = Vec::new();
        rust_files(Path::new(SRC), &mut files);
        files.sort();
        let mut found = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).expect("read");
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if !line.contains(".rounded_full()") {
                    continue;
                }
                let lo = i.saturating_sub(4);
                let hi = (i + 5).min(lines.len());
                let window = &lines[lo..hi];
                let sized = window.iter().any(|l| small_size(l));
                let filled = window.iter().enumerate().find(|(_, l)| status_fill(l));
                let Some((_, fill_line)) = filled else {
                    continue;
                };
                if !sized {
                    continue;
                }
                if fill_line.contains("status-dot: exception") {
                    continue;
                }
                let rel = file.strip_prefix(SRC).unwrap_or(&file).display();
                found.push(format!("{rel}:{}: {}", i + 1, line.trim()));
            }
        }
        found
    }

    #[test]
    fn no_coloured_status_dot_is_drawn_in_this_crate() {
        let found = offenders();
        assert!(
            found.is_empty(),
            "coloured status dots (Jacob 2026-10-27: never; use ui::working / ui::marks, or mark a \
             non-status disc `// status-dot: exception <why>` on its .bg( line):\n{}",
            found.join("\n")
        );
    }

    #[test]
    fn the_lint_catches_a_dot() {
        let sample = [
            "div()",
            "    .size(px(8.0))",
            "    .rounded_full()",
            "    .bg(theme.online)",
        ];
        assert!(super::lint_tests::small_size(sample[1]));
        assert!(super::lint_tests::status_fill(sample[3]));
        assert!(!super::lint_tests::small_size("    .size(px(30.0))"));
        assert!(!super::lint_tests::status_fill("    .bg(theme.surface_2)"));
    }
}
