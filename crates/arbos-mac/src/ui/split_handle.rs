//! Split / resize handles (Jacob 2026-10-28, [feedback
//! sidebar-slide-handle-minimal-2026-10-28]: "the slide highlight on the side
//! bar is wrong. Should make this much more minimal").
//!
//! macOS's `NSSplitView` divider: what the pointer grabs is wide, what the
//! eye sees is a hairline. Before this the sidebar's handle lit its whole
//! [`HIT_WIDTH`] band in `primary` at 45 % on hover — a thick light bar down
//! the sidebar's edge (#2082's tester feedback was about a fidgety grab, not a
//! faint line; the fix had read it the other way). Now:
//!
//! * **Hit area**: [`HIT_WIDTH`] points, invisible, inside the pane's edge
//!   (`col-resize` cursor). The pane's own separator (`border` on the sidebar
//!   and the rail, `pane_edge` on the info pane) is the only thing drawn at
//!   rest.
//! * **Indicator**: a [`LINE_WIDTH`] hairline flush with the edge, over the
//!   separator — nothing at idle, a low-contrast brighten on hover
//!   ([`HOVER_ALPHA`] of the text colour), one step more while dragging
//!   ([`DRAG_ALPHA`]). Never a fill of the hit band, never a colour.
//! * **Every handle** goes through [`right_edge`]; the sidebar's and the
//!   rail's are the same element (`ui::sidebar::resize_handle`), the right
//!   info pane has a fixed width and so no handle — only the hairline rule
//!   applies there.
//!
//! The drag's visual state comes from [`DragState`]: the handle raises it
//! when the gesture's ghost view is built, and the guard the ghost holds
//! lowers it when GPUI drops the ghost at mouse up — hover styles are
//! suppressed during a drag, so the hairline would otherwise go dark while
//! the column follows the pointer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gpui::{div, prelude::*, px, Div, Hsla, SharedString, Stateful};

use crate::ui::theme::Theme;

/// What the pointer grabs: the drag band's width in points, inside the pane's
/// edge. Generous so the gesture is easy to start ([feedback
/// app-01M5MWRQZ8TM4SS0Q25KSYYVV1]); never drawn.
pub const HIT_WIDTH: f32 = 10.0;
/// What is drawn, at most: a one-point hairline on the pane's edge (macOS's
/// split-view divider is 1 pt).
pub const LINE_WIDTH: f32 = 1.0;
/// The hairline under the pointer: a wash of the text colour, a step above
/// the pane's `border` (white 8 % / black 10 %).
pub const HOVER_ALPHA: f32 = 0.18;
/// The hairline while the drag runs: one step more, still low contrast.
pub const DRAG_ALPHA: f32 = 0.30;

/// Debug selectors of the two boxes a headless render can measure.
pub const HIT_SELECTOR: &str = "split-handle-hit";
pub const LINE_SELECTOR: &str = "split-handle-line";

/// The handle's visual state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandleState {
    /// Nothing drawn: the pane's own separator is the line.
    Idle,
    /// The pointer is over the hit band.
    Hovered,
    /// The gesture is running.
    Dragging,
}

/// The hairline's alpha (of the text colour) in `state`; `None` draws nothing.
pub fn line_alpha(state: HandleState) -> Option<f32> {
    match state {
        HandleState::Idle => None,
        HandleState::Hovered => Some(HOVER_ALPHA),
        HandleState::Dragging => Some(DRAG_ALPHA),
    }
}

/// The hairline's colour in `state`, on `theme`.
pub fn line_color(theme: &Theme, state: HandleState) -> Option<Hsla> {
    line_alpha(state).map(|a| Theme::with_alpha(theme.text, a))
}

/// Where a right-edge handle sits in a pane `width` wide, in the pane's own
/// x: the hit band `[hit.0, hit.1)` and the drawn line `[line.0, line.1)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub hit: (f32, f32),
    pub line: (f32, f32),
}

/// The geometry of a handle on the right edge of a pane `width` wide.
pub fn right_edge_frame(width: f32) -> Frame {
    Frame {
        hit: (width - HIT_WIDTH, width),
        line: (width - LINE_WIDTH, width),
    }
}

/// Whether one of the app's split handles is mid-drag. Cloned into the
/// handle; [`DragState::begin`] raises it and returns the guard that lowers
/// it when dropped — hold the guard in the drag's ghost view.
#[derive(Clone, Default, Debug)]
pub struct DragState(Arc<AtomicBool>);

/// Lowers the [`DragState`] it came from when dropped.
#[derive(Debug)]
pub struct DragGuard(Arc<AtomicBool>);

impl DragState {
    pub fn active(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub fn begin(&self) -> DragGuard {
        self.0.store(true, Ordering::Relaxed);
        DragGuard(Arc::clone(&self.0))
    }
}

impl Drop for DragGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

/// The handle on the right edge of a `relative` pane whose own right border
/// is `edge_border` wide: an invisible [`HIT_WIDTH`] band with the col-resize
/// cursor, and inside it, flush with the pane's outer edge, the
/// [`LINE_WIDTH`] hairline — transparent at rest, [`HOVER_ALPHA`] under the
/// pointer (a group-hover on the band), [`DRAG_ALPHA`] while `dragging`. An
/// absolute child sits inside its parent's border (the padding box), so the
/// band is pushed out by `edge_border`: the hairline lands **on** the pane's
/// separator pixel, brightening it, instead of drawing a second line beside
/// it. The caller adds the drag handlers.
pub fn right_edge(
    id: &'static str,
    theme: &Theme,
    dragging: bool,
    edge_border: f32,
) -> Stateful<Div> {
    let group: SharedString = format!("split-handle-{id}").into();
    let state = if dragging {
        HandleState::Dragging
    } else {
        HandleState::Idle
    };
    let at_rest = line_color(theme, state).unwrap_or(gpui::transparent_black());
    let hovered = line_color(theme, HandleState::Hovered).unwrap_or(at_rest);
    // The band's own geometry: the line at the band's far edge.
    let frame = right_edge_frame(HIT_WIDTH);
    let line = div()
        .absolute()
        .top_0()
        .left(px(frame.line.0 - frame.hit.0))
        .h_full()
        .w(px(frame.line.1 - frame.line.0))
        .bg(at_rest)
        // hover-fill: exception handle
        .group_hover(group.clone(), move |s| s.bg(hovered))
        .debug_selector(|| LINE_SELECTOR.into());
    div()
        .id(id)
        .group(group)
        .absolute()
        .top_0()
        .right(px(-edge_border))
        .h_full()
        .w(px(frame.hit.1 - frame.hit.0))
        .cursor(gpui::CursorStyle::ResizeColumn)
        .debug_selector(|| HIT_SELECTOR.into())
        .child(line)
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, Window};

    use super::*;
    use crate::ui::theme::Surface;

    /// The rule in numbers: a generous grab, a hairline look.
    #[test]
    fn the_hit_band_is_generous_and_the_line_is_a_hairline() {
        assert!(
            (8.0..=10.0).contains(&HIT_WIDTH),
            "hit band {HIT_WIDTH}: 8–10 pt grabs easily without eating the list"
        );
        assert_eq!(LINE_WIDTH, 1.0, "macOS split-view divider: 1 pt");
        let f = right_edge_frame(296.0);
        assert_eq!(f.hit, (286.0, 296.0));
        assert_eq!(f.line, (295.0, 296.0));
        let (grab, seen) = (f.hit.1 - f.hit.0, f.line.1 - f.line.0);
        assert!(
            grab >= 8.0 * seen,
            "the grab ({grab}) is many times the line ({seen})"
        );
        assert!(
            f.hit.0 <= f.line.0 && f.line.1 <= f.hit.1,
            "the line sits inside the hit band, flush with the edge"
        );
    }

    /// Idle draws nothing; hover brightens a little; the drag one step more;
    /// both stay low-contrast washes, never a colour.
    #[test]
    fn the_line_is_dark_at_rest_and_brightens_in_two_steps() {
        assert_eq!(line_alpha(HandleState::Idle), None);
        let hover = line_alpha(HandleState::Hovered).expect("hover draws the line");
        let drag = line_alpha(HandleState::Dragging).expect("drag draws the line");
        assert!(hover < drag, "drag is the stronger step ({hover} < {drag})");
        assert!(drag <= 0.35, "still subtle while dragging ({drag})");
        assert!(hover >= 0.12, "visible over the 8 % border ({hover})");
        for theme in [Theme::dark(Surface::Matte), Theme::light(Surface::Matte)] {
            assert_eq!(line_color(&theme, HandleState::Idle), None);
            let c = line_color(&theme, HandleState::Hovered).unwrap();
            assert_eq!((c.h, c.s, c.l), (theme.text.h, theme.text.s, theme.text.l));
            assert_eq!(c.a, HOVER_ALPHA);
        }
    }

    /// The flag follows the ghost's life: up at `begin`, down when the guard
    /// (held by the ghost view) drops.
    #[test]
    fn the_drag_state_lowers_itself_when_the_ghost_drops() {
        let state = DragState::default();
        assert!(!state.active());
        let guard = state.begin();
        assert!(state.active());
        let other = state.clone();
        assert!(other.active(), "a clone reads the same flag");
        drop(guard);
        assert!(!state.active());
        assert!(!other.active());
    }

    struct Pane {
        dragging: bool,
    }

    impl Render for Pane {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::dark(Surface::Matte);
            div()
                .relative()
                .w(px(296.0))
                .h(px(600.0))
                .border_r_1()
                .border_color(theme.border)
                .child(right_edge("test-split", &theme, self.dragging, 1.0))
        }
    }

    /// Headless render of a pane with the sidebar's 1 pt right border: the
    /// band the pointer gets is [`HIT_WIDTH`] wide and full height; the box
    /// that can show a colour is [`LINE_WIDTH`] wide, inside the band, and
    /// lands exactly on the pane's border pixel (the pane's outer edge).
    #[gpui::test]
    fn the_rendered_hit_band_is_wide_and_the_drawn_line_is_one_point(
        cx: &mut gpui::TestAppContext,
    ) {
        for dragging in [false, true] {
            let (_view, vcx) = cx.add_window_view(|_, _| Pane { dragging });
            vcx.run_until_parked();
            let mut at = |sel: &'static str| {
                let b = vcx
                    .debug_bounds(sel)
                    .unwrap_or_else(|| panic!("{sel} drawn"));
                (
                    f32::from(b.origin.x),
                    f32::from(b.origin.y),
                    f32::from(b.size.width),
                    f32::from(b.size.height),
                )
            };
            let (hx, hy, hw, hh) = at(HIT_SELECTOR);
            let (lx, ly, lw, lh) = at(LINE_SELECTOR);
            let f = right_edge_frame(296.0);
            assert_eq!(
                (hx, hw),
                (f.hit.0, HIT_WIDTH),
                "hit band (dragging {dragging})"
            );
            assert_eq!(
                (lx, lw),
                (f.line.0, LINE_WIDTH),
                "line (dragging {dragging})"
            );
            assert_eq!((hy, hh), (0.0, 600.0), "hit band is full height");
            assert_eq!((ly, lh), (0.0, 600.0), "line is full height");
            assert_eq!(hx + hw, 296.0, "band flush with the pane's outer edge");
            assert_eq!(
                (lx, lx + lw),
                (295.0, 296.0),
                "line on the pane's 1 pt border, not beside it"
            );
            assert!(lx >= hx, "line inside the band");
            assert!(
                lw * 8.0 <= hw,
                "what is seen ({lw}) is a fraction of what is grabbed ({hw})"
            );
        }
    }
}
