//! How the chat-list sidebar adapts as the user drags it narrower (Jacob
//! 2026-10-27, [feedback sidebar-narrow-wrap-2026-10-27]: "fix wrap for when
//! closing the side bar with the slider").
//!
//! Before: the column could be dragged to 200 pt, where the strip's three
//! discs no longer fit beside the traffic lights' band, the four filter
//! capsules overflowed (one clipped on the left, "Servers" off the right),
//! and the rows' time crowded the name down to "Arbo…". Now:
//!
//! * **Width breakpoints** (points, the column's full width):
//!   [`SNAP_WIDTH`] **220** is the narrowest readable layout — the strip's
//!   search · + · collapse exactly fill the band right of the lights
//!   (72 + 12 + 3 × 36 + 2 × 8 + 12), the rows keep a [`NAME_MIN`] name; the
//!   slider holds there, and dragging on past the snap point
//!   ([`SNAP_WIDTH`] − [`SNAP_HYSTERESIS`]) collapses the column to the icon
//!   rail (`ui::rail`, the rail morph) instead of drawing a broken middle
//!   state. Dragging the rail's edge back out past [`SNAP_WIDTH`] restores
//!   the width the column had (macOS's `NSSplitView` collapse behaviour),
//!   and the handle follows the pointer again once it passes that width.
//!   The two directions are mirror images (Jacob 2026-10-29, [feedback
//!   mac-sidebar-drag-open-2026-10-29]): folding, the column follows the
//!   pointer down to the snap width and past the snap point settles into
//!   the rail on the toggle's slide; unfolding, the rail morph follows the
//!   pointer up to the snap width ([`EdgeDrag::track`]) and past it settles
//!   at the restored width on the same slide, and a hand let go short of it
//!   settles back into the rail (`ui::slide`).
//! * **Rows**: the name wins over the time. From [`HANDLE_MIN`] down a DM's
//!   `@handle` goes; from [`TIME_FULL_MIN`] down the time drops to
//!   [`TIME_SMALL_SIZE`] pt; from [`TIME_SMALL_MIN`] down the time is hidden
//!   — all before the name's column goes under [`NAME_MIN`].
//! * **Filter**: the All · Agents · People · Servers capsules scroll
//!   horizontally with an edge fade when they do not fit (Grok Bot's iPhone
//!   home pattern for its filter row), centred when they do; the selected
//!   capsule is scrolled into view **once** — when the selection or the
//!   column's width changes ([`FilterPin`]) — and the strip is the user's
//!   after that, so "All" can always be scrolled back
//!   ([feedback server-strip-scroll-2026-10-28]). No icons, no overflow menu
//!   (`ui::sidebar::render_with`).
//! * **Strip**: unchanged — it fits at every width ≥ [`SNAP_WIDTH`]
//!   ([`strip_fits`]).

use crate::ui::theme::{SidebarWidth, SIDEBAR_WIDTH_MIN};

/// The narrowest width the column is drawn at. Below it the slider snaps to
/// the rail. Equals [`SIDEBAR_WIDTH_MIN`], the drag clamp.
pub const SNAP_WIDTH: f32 = SIDEBAR_WIDTH_MIN;
/// How far past the snap point the pointer must go before the column
/// collapses (the column holds at [`SNAP_WIDTH`] in between), so a hand
/// resting at the minimum does not flicker between the two layouts.
pub const SNAP_HYSTERESIS: f32 = 16.0;

/// The readable minimum for a row's name column: a dozen 15 pt medium
/// glyphs ("Sam Rivera", "Launch week").
pub const NAME_MIN: f32 = 96.0;
/// What a row spends left and right of its text: the list's 8 + 8 side
/// padding, the row's 12 + 12, the 36 pt picture and the 10 pt gap
/// (`identity::ROW_GAP`).
pub const ROW_CHROME: f32 = 8.0 + 8.0 + 12.0 + 12.0 + 36.0 + 10.0;
/// The time's footprint on the name's line (text + the 6 pt gap): "10:35 AM"
/// or "Yesterday" at `TEXT_TINY` 11.5, and at [`TIME_SMALL_SIZE`].
pub const TIME_FULL_WIDTH: f32 = 56.0;
pub const TIME_SMALL_WIDTH: f32 = 44.0;
/// The time's font size once the column is tight.
pub const TIME_SMALL_SIZE: f32 = 10.0;
/// A DM row's `@handle` beside the name (text + its 6 pt gap): the first
/// thing to go, at [`HANDLE_MIN`] (= the default 296 pt column, near enough).
pub const HANDLE_WIDTH: f32 = 50.0;
pub const HANDLE_MIN: f32 = ROW_CHROME + NAME_MIN + TIME_FULL_WIDTH + HANDLE_WIDTH;
/// Column widths at which the time still fits beside a [`NAME_MIN`] name:
/// full at ≥ **238**, small at ≥ **226**, hidden below (the snap width is 220).
pub const TIME_FULL_MIN: f32 = ROW_CHROME + NAME_MIN + TIME_FULL_WIDTH;
pub const TIME_SMALL_MIN: f32 = ROW_CHROME + NAME_MIN + TIME_SMALL_WIDTH;

/// How a row shows its time at the column's `width`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowTime {
    /// `TEXT_TINY`.
    Full,
    /// [`TIME_SMALL_SIZE`] pt.
    Small,
    /// No time: the name has the whole line.
    Hidden,
}

/// Whether a DM row shows its `@handle` beside the name at `width`.
pub fn handle_shown(width: f32) -> bool {
    width >= HANDLE_MIN
}

pub fn row_time(width: f32) -> RowTime {
    if width >= TIME_FULL_MIN {
        RowTime::Full
    } else if width >= TIME_SMALL_MIN {
        RowTime::Small
    } else {
        RowTime::Hidden
    }
}

/// The width left for a row's text at the column's `width`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn text_column(width: f32) -> f32 {
    width - ROW_CHROME
}

/// The strip's three discs fit beside the lights' band at `width`
/// (`TRAFFIC_LIGHTS_INSET` + `STRIP_INSET` + 3 discs + 2 gaps + `STRIP_INSET`).
#[cfg_attr(not(test), allow(dead_code))]
pub fn strip_fits(width: f32) -> bool {
    use crate::ui::shell::TRAFFIC_LIGHTS_INSET;
    use crate::ui::sidebar::{STRIP_BUTTON, STRIP_GAP, STRIP_INSET};
    let needed =
        TRAFFIC_LIGHTS_INSET + STRIP_INSET + 3.0 * STRIP_BUTTON + 2.0 * STRIP_GAP + STRIP_INSET;
    width + 0.5 >= needed
}

/// When the filter strip scrolls its selected capsule into view.
///
/// Affinity (Arbonites, 2026-10-28, confirmed by Scout): with Servers selected
/// in a narrow column, "All" was clipped on the left and could not be scrolled
/// back. Cause: `render` asked the scroll handle to bring the selected capsule
/// into view on **every** frame, and a scroll gesture is itself a frame — each
/// wheel tick toward "All" was undone by the next paint. The pin now fires once
/// per `(selection, column width)`; between those changes the strip holds
/// whatever offset the user scrolled to, 0 included.
#[derive(Debug, Default)]
pub struct FilterPin {
    last: std::cell::Cell<Option<(u8, i32)>>,
}

impl FilterPin {
    /// `true` when the selected capsule (`selected`, its index) should be
    /// scrolled into view this frame: the selection moved or the column was
    /// resized since the last pin. `width` is the column's width in points;
    /// `measured` says the scroll handle has been through a frame (GPUI applies
    /// a pin against the previous frame's bounds and overflow, so a pin asked
    /// for before the first frame is lost — keep asking until one can land).
    pub fn take(&self, selected: usize, width: f32, measured: bool) -> bool {
        if !measured {
            return true;
        }
        let key = (selected as u8, width.round() as i32);
        if self.last.get() == Some(key) {
            return false;
        }
        self.last.set(Some(key));
        true
    }
}

/// One drag of the column's right edge (expanded or rail). Created when the
/// drag starts; remembers the width restored by re-expanding from the rail so
/// the handle holds there until the pointer catches up.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EdgeDrag {
    /// The column's width when the gesture began (expanded), so a snap to
    /// the rail keeps *that* as the width to come back to — not the minimum
    /// the handle passed through on the way down.
    start: Option<f32>,
    restored: Option<f32>,
    /// While a gesture that began on the rail is short of the snap width:
    /// the width the rail morph is drawn at (the pointer's x, from the rail's
    /// width up to [`SNAP_WIDTH`]), so the unfold follows the hand the way
    /// the fold follows it down to the snap width.
    track: Option<f32>,
    /// The gesture has crossed the snap width once: the morph no longer
    /// follows the pointer, a further collapse settles like the toggle.
    crossed: bool,
}

/// What a pointer at `x` asks of the column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EdgeTarget {
    /// Collapse to (or stay on) the rail. `keep` is the width to save as the
    /// column's: the width the gesture started from.
    Rail { keep: f32 },
    /// Expanded at this width (clamped to the drag range).
    Expanded(f32),
    /// Nothing changes this move: the column was just restored to a width
    /// the pointer has not reached yet.
    Hold,
}

impl EdgeDrag {
    /// A gesture starting with the column `width` wide (`None` from the rail).
    pub fn begin(width: Option<f32>) -> Self {
        Self {
            start: width,
            ..Self::default()
        }
    }

    /// The rail morph's pointer-tracked width (see the field), `None` when
    /// the column draws at its own width.
    pub fn track(&self) -> Option<f32> {
        self.track
    }

    /// `x` = the pointer's distance from the window's left edge (the column
    /// docks there, so it is the width asked for); `rail` = collapsed now;
    /// `last_width` = the column's saved width.
    pub fn target(&mut self, x: f32, rail: bool, last_width: f32) -> EdgeTarget {
        self.track = None;
        if rail {
            if x >= SNAP_WIDTH {
                let restored = SidebarWidth::clamp(last_width);
                self.restored = Some(restored);
                self.crossed = true;
                EdgeTarget::Expanded(restored)
            } else {
                let rail_width = crate::ui::rail::metrics::WIDTH;
                if self.start.is_none() && !self.crossed && x > rail_width + 0.5 {
                    self.track = Some(x);
                }
                EdgeTarget::Rail { keep: last_width }
            }
        } else if x < SNAP_WIDTH - SNAP_HYSTERESIS {
            self.restored = None;
            self.crossed = true;
            EdgeTarget::Rail {
                keep: self.start.unwrap_or(last_width).max(last_width),
            }
        } else if let Some(restored) = self.restored {
            if x >= restored {
                self.restored = None;
                EdgeTarget::Expanded(SidebarWidth::clamp(x))
            } else {
                EdgeTarget::Hold
            }
        } else {
            EdgeTarget::Expanded(SidebarWidth::clamp(x))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::{SIDEBAR_WIDTH, SIDEBAR_WIDTH_MAX};

    /// The breakpoints: the snap width is the drag clamp's minimum, the strip
    /// exactly fits there and at every wider width, and the rows' name keeps
    /// its readable minimum at each time state's threshold.
    #[test]
    fn the_width_breakpoints_keep_every_part_readable() {
        assert_eq!(SNAP_WIDTH, 220.0);
        assert_eq!(SNAP_WIDTH, SIDEBAR_WIDTH_MIN);
        assert!(
            strip_fits(SNAP_WIDTH),
            "search · + · collapse fit at the snap width"
        );
        assert!(strip_fits(SIDEBAR_WIDTH));
        assert!(strip_fits(SIDEBAR_WIDTH_MAX));
        assert!(
            !strip_fits(200.0),
            "the old minimum pushed the discs onto the lights"
        );
        assert_eq!(ROW_CHROME, 86.0);
        assert_eq!(TIME_FULL_MIN, 238.0);
        assert_eq!(TIME_SMALL_MIN, 226.0);
        assert_eq!(HANDLE_MIN, 288.0);
        assert!(
            handle_shown(SIDEBAR_WIDTH),
            "the default column shows @handles"
        );
        assert!(!handle_shown(260.0));
        assert!(text_column(HANDLE_MIN) - TIME_FULL_WIDTH - HANDLE_WIDTH >= NAME_MIN);
        // At the snap width the name has the whole line and it is readable.
        assert!(text_column(SNAP_WIDTH) >= NAME_MIN);
        assert_eq!(row_time(SNAP_WIDTH), RowTime::Hidden);
        // Where the small time comes back, the name still has its minimum.
        assert!(text_column(TIME_SMALL_MIN) - TIME_SMALL_WIDTH >= NAME_MIN);
        assert_eq!(row_time(TIME_SMALL_MIN), RowTime::Small);
        assert_eq!(row_time(TIME_SMALL_MIN - 1.0), RowTime::Hidden);
        // Where the full time comes back, likewise.
        assert!(text_column(TIME_FULL_MIN) - TIME_FULL_WIDTH >= NAME_MIN);
        assert_eq!(row_time(TIME_FULL_MIN), RowTime::Full);
        assert_eq!(row_time(TIME_FULL_MIN - 1.0), RowTime::Small);
        assert_eq!(row_time(SIDEBAR_WIDTH), RowTime::Full);
        assert_eq!(TIME_SMALL_SIZE, 10.0);
        assert_eq!(crate::ui::theme::TEXT_TINY, 11.5, "the full time size");
    }

    /// Jacob's shot widths: 320 and 260 keep the full layout, 220 is the
    /// readable minimum (no time), under the snap point the rail takes over.
    #[test]
    fn the_rows_time_shrinks_then_hides_before_the_name_gives() {
        for w in [320.0, 260.0, 238.0] {
            assert_eq!(row_time(w), RowTime::Full, "{w}");
            assert!(text_column(w) - TIME_FULL_WIDTH >= NAME_MIN, "{w}");
        }
        assert!(handle_shown(320.0));
        assert!(!handle_shown(260.0), "the @handle is the first to go");
        for w in [237.0, 232.0, 226.0] {
            assert_eq!(row_time(w), RowTime::Small, "{w}");
            assert!(text_column(w) - TIME_SMALL_WIDTH >= NAME_MIN, "{w}");
        }
        for w in [225.0, 220.0] {
            assert_eq!(row_time(w), RowTime::Hidden, "{w}");
            assert!(text_column(w) >= NAME_MIN, "{w}");
        }
    }

    /// The slider's snap point: expanded, the column holds at 220 while the
    /// pointer is between 204 and 220, collapses to the rail under 204 and
    /// keeps the width the gesture started from (296) as the one to come
    /// back to; from the rail, crossing 220 restores that width and the
    /// handle holds there until the pointer passes it, then follows.
    #[test]
    fn dragging_past_the_snap_point_collapses_to_the_rail_and_back_restores_the_width() {
        let mut drag = EdgeDrag::begin(Some(296.0));
        // Expanded at 296, dragging in.
        assert_eq!(
            drag.target(260.0, false, 296.0),
            EdgeTarget::Expanded(260.0)
        );
        assert_eq!(
            drag.target(221.0, false, 260.0),
            EdgeTarget::Expanded(221.0)
        );
        assert_eq!(
            drag.target(210.0, false, 221.0),
            EdgeTarget::Expanded(SNAP_WIDTH),
            "between the snap width and the snap point the column holds at 220"
        );
        assert_eq!(
            drag.target(204.0, false, 220.0),
            EdgeTarget::Expanded(SNAP_WIDTH)
        );
        assert_eq!(
            drag.target(203.0, false, 220.0),
            EdgeTarget::Rail { keep: 296.0 },
            "past the snap point: the rail, and 296 stays the column's width"
        );
        assert_eq!(
            drag.target(150.0, true, 296.0),
            EdgeTarget::Rail { keep: 296.0 }
        );
        // Back out in the same gesture: 296 comes back at 220, holds, then follows.
        assert_eq!(drag.target(220.0, true, 296.0), EdgeTarget::Expanded(296.0));
        assert_eq!(drag.target(230.0, false, 296.0), EdgeTarget::Hold);
        assert_eq!(
            drag.target(310.0, false, 296.0),
            EdgeTarget::Expanded(310.0)
        );

        // A new gesture from the rail (collapsed by the toggle at 296): comes
        // back at 296, holds while the pointer is short of it, then follows.
        let mut drag = EdgeDrag::begin(None);
        assert_eq!(
            drag.target(100.0, true, 296.0),
            EdgeTarget::Rail { keep: 296.0 }
        );
        assert_eq!(drag.target(225.0, true, 296.0), EdgeTarget::Expanded(296.0));
        assert_eq!(drag.target(250.0, false, 296.0), EdgeTarget::Hold);
        assert_eq!(drag.target(295.0, false, 296.0), EdgeTarget::Hold);
        assert_eq!(
            drag.target(300.0, false, 296.0),
            EdgeTarget::Expanded(300.0)
        );
        assert_eq!(
            drag.target(280.0, false, 300.0),
            EdgeTarget::Expanded(280.0),
            "follows again"
        );
        // Dragging straight back under the snap point from the held state collapses.
        let mut drag = EdgeDrag::begin(None);
        assert_eq!(drag.target(225.0, true, 296.0), EdgeTarget::Expanded(296.0));
        assert_eq!(
            drag.target(190.0, false, 296.0),
            EdgeTarget::Rail { keep: 296.0 }
        );
        // A gesture that began at 240 and widened to 320 before snapping keeps 320.
        let mut drag = EdgeDrag::begin(Some(240.0));
        assert_eq!(
            drag.target(320.0, false, 240.0),
            EdgeTarget::Expanded(320.0)
        );
        assert_eq!(
            drag.target(100.0, false, 320.0),
            EdgeTarget::Rail { keep: 320.0 }
        );
        // A fresh gesture forgets the hold.
        let drag = EdgeDrag::begin(Some(296.0));
        assert_eq!(drag.restored, None);
        // The range clamp still applies.
        let mut drag = EdgeDrag::begin(Some(296.0));
        assert_eq!(
            drag.target(900.0, false, 296.0),
            EdgeTarget::Expanded(SIDEBAR_WIDTH_MAX)
        );
    }

    /// Scout's report (2026-10-29): dragging the rail open flipped straight
    /// to the unfolded column. A drag from the rail now asks the morph to
    /// follow the pointer up to the snap width — the mirror of the fold,
    /// where the column follows it down — and stops asking once the snap
    /// width is crossed (the unfold settles on the slide from there).
    #[test]
    fn a_drag_from_the_rail_tracks_the_pointer_until_the_snap_width() {
        let rail = crate::ui::rail::metrics::WIDTH;
        let mut drag = EdgeDrag::begin(None);
        assert_eq!(drag.track(), None);
        drag.target(rail - 4.0, true, 296.0);
        assert_eq!(drag.track(), None, "on the rail's own edge: the rail");
        for x in [100.0, 150.0, 219.0] {
            assert_eq!(
                drag.target(x, true, 296.0),
                EdgeTarget::Rail { keep: 296.0 }
            );
            assert_eq!(drag.track(), Some(x), "the morph follows the hand at {x}");
        }
        assert_eq!(drag.target(220.0, true, 296.0), EdgeTarget::Expanded(296.0));
        assert_eq!(drag.track(), None, "crossed: the slide takes it to 296");
        // Back under the snap point in the same gesture: the fold settles
        // like the toggle, the morph does not chase the pointer again.
        assert_eq!(
            drag.target(150.0, false, 296.0),
            EdgeTarget::Rail { keep: 296.0 }
        );
        assert_eq!(drag.track(), None);
        assert_eq!(
            drag.target(120.0, true, 296.0),
            EdgeTarget::Rail { keep: 296.0 }
        );
        assert_eq!(drag.track(), None);

        // A fold from the expanded column never tracks below the snap width:
        // it follows the pointer as a width, then settles into the rail.
        let mut drag = EdgeDrag::begin(Some(296.0));
        assert_eq!(
            drag.target(150.0, false, 296.0),
            EdgeTarget::Rail { keep: 296.0 }
        );
        assert_eq!(drag.track(), None);
        drag.target(120.0, true, 296.0);
        assert_eq!(drag.track(), None);
    }
}
