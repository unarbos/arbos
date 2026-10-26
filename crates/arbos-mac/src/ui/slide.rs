//! Pane slides (Jacob 2026-10-24): the right info pane and the left chat
//! sidebar slide in and out like Grok Bot's instead of popping
//! ([feedback right-pane-slide-2026-10-24], [feedback sidebar-slide-2026-10-24]).
//!
//! The logical state stays where it was — `AppState::profile` and
//! `AppState::rail_collapsed` flip at once — and [`PaneSlides`] derives the
//! motion from frame to frame: when a pane appears or goes, its *visible
//! width* runs from one edge to the other over the [`Motion`]'s duration and
//! curve (Grok Bot's measured 200 ms open / 170 ms close), driven by
//! `request_animation_frame` from `Shell::render`. The
//! chat column is a flex sibling between the two, so its slot follows every
//! frame — but its transcript is a `list()` whose item heights are thrown
//! away whenever its width changes, and re-measuring every bubble per frame
//! is the stutter to avoid. So while a slide runs the transcript keeps the
//! width it had when the slide began ([`ChatSlide::start_width`]) inside a
//! clip, placed by an [`Anchor`], and is laid out once more at the end at its
//! final width. Header pill and composer are cheap and follow the live width.
//!
//! Reduce Motion (`cx.reduce_motion()`): no slide is started; everything
//! snaps as before.

use std::time::{Duration, Instant};

use gpui::{div, prelude::*, px, AnyElement, Div};

use crate::ui::theme::COLUMN_MARGIN;
use crate::ui::transcript::COLUMN_MAX_WIDTH;

/// Grok Bot's right pane, measured frame by frame on xAI's own screen
/// recording (the "How I run multiple teams of Grok Bots" guide video,
/// 30 fps; `media/feedback/pane-slide/` in the project store): opening takes
/// about 200 ms with a soft start and a long tail — ease-out cubic fits it
/// as well as a lightly damped spring — and closing is quicker, ~150–175 ms
/// with no slow start, which an ease-out quad fits best. The pane translates
/// (its content moves with its edge). The sidebar toggle was not in any
/// video; it gets the same motion.
pub const OPEN_MS: u64 = 200;
pub const CLOSE_MS: u64 = 170;

/// Which way a slide goes: a pane coming in or going out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Open,
    Close,
}

impl Motion {
    pub fn duration(self) -> Duration {
        Duration::from_millis(match self {
            Motion::Open => OPEN_MS,
            Motion::Close => CLOSE_MS,
        })
    }

    /// The curve: fast off the mark, settling softly — Grok's open (cubic)
    /// and its brisker close (quad).
    pub fn ease(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Motion::Open => crate::ui::motion::ease_out_cubic(t),
            Motion::Close => 1.0 - (1.0 - t) * (1.0 - t),
        }
    }
}

/// One running slide of a visible width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slide {
    pub from: f32,
    pub to: f32,
    pub started: Instant,
    pub duration: Duration,
    pub motion: Motion,
}

impl Slide {
    /// A slide from `from` to `to` over the motion's full duration.
    pub fn new(from: f32, to: f32, now: Instant) -> Self {
        let motion = if to > from {
            Motion::Open
        } else {
            Motion::Close
        };
        Slide {
            from,
            to,
            started: now,
            duration: motion.duration(),
            motion,
        }
    }

    /// Eased progress 0..1 at `now`.
    pub fn progress(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        let t =
            now.saturating_duration_since(self.started).as_secs_f32() / self.duration.as_secs_f32();
        self.motion.ease(t)
    }

    /// The visible width at `now`.
    pub fn value(&self, now: Instant) -> f32 {
        self.from + (self.to - self.from) * self.progress(now)
    }

    pub fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= self.duration
    }

    /// A click mid-slide (collapse, then expand again): turn round from where
    /// the pane is now, with the time the remaining distance deserves — a
    /// quick reversal is quick, a full one takes the full duration.
    pub fn retarget(&self, to: f32, full: f32, now: Instant) -> Slide {
        let from = self.value(now);
        let fraction = if full.abs() < f32::EPSILON {
            1.0
        } else {
            ((to - from).abs() / full).clamp(0.0, 1.0)
        };
        let next = Slide::new(from, to, now);
        Slide {
            duration: next.motion.duration().mul_f32(fraction.max(0.05)),
            ..next
        }
    }
}

/// Which edge a pane moves on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The chat sidebar, on the window's left edge.
    Left,
    /// The info pane, on the window's right edge.
    Right,
}

/// Where the transcript (kept at its start width) sits inside the live slot
/// while a neighbour slides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// Centred in the slot: the rows glide with the slot's centre. Chosen
    /// when the rows are capped at both widths (`COLUMN_MAX_WIDTH`), so the
    /// bubbles are the same at start and end and the final re-layout moves
    /// nothing — a true reflow, with none of the measuring.
    Centre,
    /// Pinned to the slot's left edge: the right pane slides over or away
    /// from the transcript; the left-bound agent bubbles never move.
    Left,
    /// Pinned to the slot's right edge: the sidebar slides over or away from
    /// the transcript's left end.
    Right,
}

/// Rows at this chat-column width are capped (centred at `COLUMN_MAX_WIDTH`)
/// rather than filling it.
pub fn capped(center_width: f32) -> bool {
    center_width - 2.0 * COLUMN_MARGIN >= COLUMN_MAX_WIDTH - 0.5
}

/// The anchor for a slide of `side` that takes the chat column from
/// `start_width` to `end_width`.
pub fn anchor_for(side: Side, start_width: f32, end_width: f32) -> Anchor {
    if capped(start_width) && capped(end_width) {
        return Anchor::Centre;
    }
    match side {
        Side::Left => Anchor::Right,
        Side::Right => Anchor::Left,
    }
}

/// What the chat column does during a slide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChatSlide {
    /// The column's width when the slide began: the transcript stays laid
    /// out at this width until every slide has ended.
    pub start_width: f32,
    pub anchor: Anchor,
}

impl ChatSlide {
    /// The transcript's left offset inside a slot `live_width` wide.
    pub fn offset(&self, live_width: f32) -> f32 {
        match self.anchor {
            Anchor::Centre => (live_width - self.start_width) / 2.0,
            Anchor::Left => 0.0,
            Anchor::Right => live_width - self.start_width,
        }
    }
}

/// What one frame draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// The sidebar's visible width this frame (0 = gone, `full` = at rest).
    pub sidebar_visible: f32,
    /// The sidebar is mid-slide: clip it to `sidebar_visible`.
    pub sidebar_sliding: bool,
    /// The right pane's visible width this frame.
    pub pane_visible: f32,
    /// The right pane is mid-slide: clip it to `pane_visible`.
    pub pane_sliding: bool,
    /// Set while any slide runs: the transcript keeps its start width.
    pub chat: Option<ChatSlide>,
}

impl Frame {
    pub fn animating(&self) -> bool {
        self.sidebar_sliding || self.pane_sliding
    }

    /// The sidebar's visible width is between its full width and the rail's:
    /// the rail morph's progress (0 = expanded, 1 = rail). `None` when the
    /// sidebar is on its way to or from hidden (the narrow band).
    pub fn rail_progress(&self, full: f32) -> Option<f32> {
        let rail = crate::ui::rail::metrics::WIDTH;
        (self.sidebar_visible >= rail - 0.5)
            .then(|| crate::ui::rail::progress(self.sidebar_visible, full))
    }
}

/// The frame-to-frame tracker: told what the state wants each frame, it
/// answers what to draw and starts or finishes slides on changes.
#[derive(Debug, Default)]
pub struct PaneSlides {
    sidebar: Option<Slide>,
    pane: Option<Slide>,
    /// `(shown, full width)` as last drawn; `None` before the first frame.
    last_sidebar: Option<(bool, f32)>,
    /// Collapsed to the rail as last drawn.
    last_sidebar_rail: bool,
    last_pane: Option<(bool, f32)>,
    chat: Option<ChatSlide>,
}

/// What the state wants this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Want {
    pub viewport_width: f32,
    pub sidebar_shown: bool,
    pub sidebar_width: f32,
    /// The sidebar is collapsed to the icon rail (`ui::rail`): shown, but
    /// `rail::metrics::WIDTH` wide. Flipping this slides between the two
    /// widths (the rail morph) on the same curves
    /// ([feedback mac-sidebar-rail-2026-10-26]).
    pub sidebar_rail: bool,
    /// The window is in the narrow band: the sidebar is an overlay there, not
    /// a column, so its coming and going never slides.
    pub narrow: bool,
    pub pane_shown: bool,
    pub pane_width: f32,
    /// Reduce Motion: snap, never slide.
    pub reduce_motion: bool,
}

impl PaneSlides {
    /// One frame. `now` is the frame time.
    pub fn step(&mut self, want: Want, now: Instant) -> Frame {
        let sidebar_full = want.sidebar_width;
        let pane_full = want.pane_width;
        let sidebar_target = if !want.sidebar_shown {
            0.0
        } else if want.sidebar_rail {
            crate::ui::rail::metrics::WIDTH
        } else {
            sidebar_full
        };
        let pane_target = if want.pane_shown { pane_full } else { 0.0 };

        // Where things were drawn last frame (for the chat's start width).
        let sidebar_before = match (self.sidebar, self.last_sidebar) {
            (Some(s), _) => s.value(now),
            (None, Some((shown, w))) => {
                if !shown {
                    0.0
                } else if self.last_sidebar_rail {
                    crate::ui::rail::metrics::WIDTH
                } else {
                    w
                }
            }
            (None, None) => sidebar_target,
        };
        let pane_before = match (self.pane, self.last_pane) {
            (Some(s), _) => s.value(now),
            (None, Some((shown, w))) => {
                if shown {
                    w
                } else {
                    0.0
                }
            }
            (None, None) => pane_target,
        };
        let snap = want.reduce_motion;

        let sidebar_changed = self
            .last_sidebar
            .is_some_and(|(shown, _)| shown != want.sidebar_shown)
            || (self.last_sidebar.is_some()
                && want.sidebar_shown
                && self.last_sidebar_rail != want.sidebar_rail);
        if sidebar_changed && !snap && !want.narrow {
            self.start(
                Side::Left,
                sidebar_target,
                sidebar_full,
                want,
                sidebar_before,
                pane_before,
                now,
            );
        } else if snap || want.narrow {
            self.sidebar = None;
        }
        let pane_changed = self
            .last_pane
            .is_some_and(|(shown, _)| shown != want.pane_shown);
        if pane_changed && !snap {
            self.start(
                Side::Right,
                pane_target,
                pane_full,
                want,
                sidebar_before,
                pane_before,
                now,
            );
        } else if snap {
            self.pane = None;
        }
        self.last_sidebar = Some((want.sidebar_shown, sidebar_full));
        self.last_sidebar_rail = want.sidebar_rail;
        self.last_pane = Some((want.pane_shown, pane_full));

        // Finished slides come to rest at their target.
        if self.sidebar.is_some_and(|s| s.done(now)) {
            self.sidebar = None;
        }
        if self.pane.is_some_and(|s| s.done(now)) {
            self.pane = None;
        }
        // A slide whose target the state moved away from (a resize, a wider
        // settings page) is dropped rather than left pointing at a stale width.
        if self
            .sidebar
            .is_some_and(|s| (s.to - sidebar_target).abs() > 0.5)
        {
            self.sidebar = None;
        }
        if self.pane.is_some_and(|s| (s.to - pane_target).abs() > 0.5) {
            self.pane = None;
        }
        if self.sidebar.is_none() && self.pane.is_none() {
            self.chat = None;
        }

        Frame {
            sidebar_visible: self.sidebar.map_or(sidebar_target, |s| s.value(now)),
            sidebar_sliding: self.sidebar.is_some(),
            pane_visible: self.pane.map_or(pane_target, |s| s.value(now)),
            pane_sliding: self.pane.is_some(),
            chat: self.chat,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start(
        &mut self,
        side: Side,
        target: f32,
        full: f32,
        want: Want,
        sidebar_before: f32,
        pane_before: f32,
        now: Instant,
    ) {
        let slot = match side {
            Side::Left => &mut self.sidebar,
            Side::Right => &mut self.pane,
        };
        let from = match side {
            Side::Left => sidebar_before,
            Side::Right => pane_before,
        };
        *slot = Some(match *slot {
            Some(running) => running.retarget(target, full, now),
            None => Slide::new(from, target, now),
        });
        if self.chat.is_none() {
            let start_width = want.viewport_width - sidebar_before - pane_before;
            let end_width = want.viewport_width
                - match side {
                    Side::Left => target + pane_before,
                    Side::Right => sidebar_before + target,
                };
            self.chat = Some(ChatSlide {
                start_width,
                anchor: anchor_for(side, start_width, end_width),
            });
        }
    }
}

// ----- elements ----------------------------------------------------------------

/// A pane `full` wide shown through a clip `visible` wide on the window's
/// left edge: the pane keeps its layout and moves with the clip's edge (its
/// content leaves to the left), so nothing inside is re-laid out per frame.
pub fn sidebar_clip(visible: f32, full: f32, pane: impl IntoElement) -> Div {
    div()
        .h_full()
        .w(px(visible.max(0.0)))
        .flex_shrink_0()
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(px(full))
                .flex_shrink_0()
                .relative()
                .left(px(visible - full))
                .child(pane),
        )
}

/// A pane shown through a clip `visible` wide on the window's right edge:
/// the pane's left edge is the clip's left edge, so as the clip grows the
/// pane comes in from the right with its content.
pub fn pane_clip(visible: f32, pane: impl IntoElement) -> Div {
    div()
        .h_full()
        .w(px(visible.max(0.0)))
        .flex_shrink_0()
        .overflow_hidden()
        .flex()
        .child(pane)
}

/// The transcript during a slide: kept at `slide.start_width` inside a clip
/// that fills the live slot, offset by the slide's anchor.
pub fn transcript_clip(slide: ChatSlide, live_width: f32, transcript: AnyElement) -> AnyElement {
    div()
        .size_full()
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(px(slide.start_width))
                .flex_shrink_0()
                .relative()
                .left(px(slide.offset(live_width)))
                .child(transcript),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 1180.0;
    const SIDEBAR: f32 = 296.0;
    const PANE: f32 = 340.0;

    fn want(sidebar_shown: bool, pane_shown: bool) -> Want {
        Want {
            viewport_width: W,
            sidebar_shown,
            sidebar_width: SIDEBAR,
            sidebar_rail: false,
            narrow: false,
            pane_shown,
            pane_width: PANE,
            reduce_motion: false,
        }
    }

    fn want_rail(rail: bool) -> Want {
        Want {
            sidebar_rail: rail,
            ..want(true, false)
        }
    }

    /// The collapse toggle ([feedback mac-sidebar-rail-2026-10-26]): the
    /// sidebar slides from its full width to the 84 pt rail on the close
    /// curve (170 ms) and back on the open curve (200 ms); the frame reports
    /// the morph's progress for `ui::rail`.
    #[test]
    fn collapsing_slides_the_sidebar_down_to_the_rail_and_back() {
        let rail = crate::ui::rail::metrics::WIDTH;
        let t0 = Instant::now();
        let mut slides = PaneSlides::default();
        let f = slides.step(want_rail(false), t0);
        assert_eq!(f.sidebar_visible, SIDEBAR);
        assert_eq!(f.rail_progress(SIDEBAR), Some(0.0));
        // Click: the width runs 296 → 84 over CLOSE_MS.
        let f = slides.step(want_rail(true), ms(t0, 16));
        assert!(f.sidebar_sliding);
        assert_eq!(f.sidebar_visible, SIDEBAR);
        assert_eq!(f.chat.expect("chat slide").start_width, W - SIDEBAR);
        let mut last = SIDEBAR;
        for i in 1..10 {
            let f = slides.step(want_rail(true), ms(t0, 16 + i * 16));
            assert!(f.sidebar_visible <= last, "frame {i}");
            assert!(f.sidebar_visible >= rail);
            let p = f.rail_progress(SIDEBAR).expect("a rail morph, not a hide");
            assert!((0.0..=1.0).contains(&p));
            last = f.sidebar_visible;
        }
        let f = slides.step(want_rail(true), ms(t0, 16 + CLOSE_MS + 1));
        assert_eq!(f.sidebar_visible, rail);
        assert!(!f.animating());
        assert_eq!(f.rail_progress(SIDEBAR), Some(1.0));
        // Click again: 84 → 296 over OPEN_MS, from the rail's width.
        let t1 = ms(t0, 1000);
        let f = slides.step(want_rail(false), t1);
        assert!(f.sidebar_sliding);
        assert_eq!(f.sidebar_visible, rail);
        let f = slides.step(want_rail(false), ms(t1, OPEN_MS / 2));
        assert!(f.sidebar_visible > rail && f.sidebar_visible < SIDEBAR);
        let f = slides.step(want_rail(false), ms(t1, OPEN_MS + 1));
        assert_eq!(f.sidebar_visible, SIDEBAR);
        assert!(!f.animating());
    }

    /// Reduce Motion snaps the rail like everything else; a width drag while
    /// collapsed never starts a slide.
    #[test]
    fn the_rail_snaps_under_reduce_motion_and_ignores_width_drags() {
        let rail = crate::ui::rail::metrics::WIDTH;
        let t0 = Instant::now();
        let mut slides = PaneSlides::default();
        slides.step(want_rail(false), t0);
        let f = slides.step(
            Want {
                reduce_motion: true,
                ..want_rail(true)
            },
            ms(t0, 16),
        );
        assert_eq!(f.sidebar_visible, rail);
        assert!(!f.animating());
        let f = slides.step(
            Want {
                sidebar_width: 360.0,
                ..want_rail(true)
            },
            ms(t0, 32),
        );
        assert_eq!(
            f.sidebar_visible, rail,
            "the rail is the rail at any saved width"
        );
        assert!(!f.animating());
    }

    fn ms(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn both_curves_start_at_zero_end_at_one_and_only_rise() {
        for motion in [Motion::Open, Motion::Close] {
            assert_eq!(motion.ease(0.0), 0.0);
            assert_eq!(motion.ease(1.0), 1.0);
            let mut last = 0.0;
            for i in 1..=100 {
                let v = motion.ease(i as f32 / 100.0);
                assert!(v >= last, "{motion:?} fell at {i}: {v} < {last}");
                last = v;
            }
            // Ease-out: well past half way at half time.
            assert!(
                motion.ease(0.5) > 0.7,
                "{motion:?}(0.5) = {}",
                motion.ease(0.5)
            );
            assert_eq!(motion.ease(2.0), 1.0, "clamped past the end");
        }
        // Grok's numbers: the open is the softer, longer one.
        assert!(Motion::Open.ease(0.5) > Motion::Close.ease(0.5));
        assert_eq!(Motion::Open.duration(), Duration::from_millis(200));
        assert_eq!(Motion::Close.duration(), Duration::from_millis(170));
    }

    #[test]
    fn a_slide_runs_its_motion_duration_then_rests_at_its_target() {
        let t0 = Instant::now();
        let s = Slide::new(0.0, PANE, t0);
        assert_eq!(s.motion, Motion::Open);
        assert_eq!(s.value(t0), 0.0);
        assert!(!s.done(t0));
        let mid = s.value(ms(t0, OPEN_MS / 2));
        assert!(mid > 0.0 && mid < PANE, "mid {mid}");
        assert!(s.value(ms(t0, OPEN_MS / 2)) < s.value(ms(t0, OPEN_MS * 3 / 4)));
        assert!(s.done(ms(t0, OPEN_MS)));
        assert_eq!(s.value(ms(t0, OPEN_MS)), PANE);
        assert_eq!(s.value(ms(t0, OPEN_MS * 5)), PANE);
        let c = Slide::new(PANE, 0.0, t0);
        assert_eq!(c.motion, Motion::Close);
        assert!(!c.done(ms(t0, CLOSE_MS - 1)));
        assert!(c.done(ms(t0, CLOSE_MS)));
        assert_eq!(c.value(ms(t0, CLOSE_MS)), 0.0);
    }

    #[test]
    fn opening_the_right_pane_slides_it_in_and_the_tracker_rests_after_the_duration() {
        let t0 = Instant::now();
        let mut slides = PaneSlides::default();
        // First frame: nothing moves, nothing animates.
        let f = slides.step(want(true, false), t0);
        assert_eq!(f.pane_visible, 0.0);
        assert!(!f.animating());
        assert_eq!(f.chat, None);
        // The pill is clicked: the pane starts at 0 and grows.
        let f = slides.step(want(true, true), ms(t0, 16));
        assert!(f.pane_sliding);
        assert_eq!(f.pane_visible, 0.0);
        assert_eq!(f.sidebar_visible, SIDEBAR);
        assert!(!f.sidebar_sliding);
        let chat = f.chat.expect("the chat keeps its start width");
        assert_eq!(chat.start_width, W - SIDEBAR);
        // 884 → 544: the rows are no longer capped, so the pane slides over
        // the transcript from the right and the agent bubbles stay put.
        assert_eq!(chat.anchor, Anchor::Left);
        assert_eq!(chat.offset(700.0), 0.0);
        let mut last = 0.0;
        for i in 1..12 {
            let f = slides.step(want(true, true), ms(t0, 16 + i * 16));
            assert!(
                f.pane_visible >= last,
                "frame {i}: {} < {last}",
                f.pane_visible
            );
            assert!(f.pane_visible <= PANE);
            last = f.pane_visible;
        }
        // Past the duration: at rest at the full width, nothing animating,
        // the chat laid out at its final width (no clip).
        let f = slides.step(want(true, true), ms(t0, 16 + OPEN_MS + 1));
        assert_eq!(f.pane_visible, PANE);
        assert!(!f.animating());
        assert_eq!(f.chat, None);
    }

    #[test]
    fn closing_the_right_pane_slides_it_out_from_its_full_width() {
        let t0 = Instant::now();
        let mut slides = PaneSlides::default();
        slides.step(want(true, true), t0);
        let f = slides.step(want(true, false), ms(t0, 16));
        assert!(f.pane_sliding);
        assert_eq!(f.pane_visible, PANE);
        assert_eq!(f.chat.map(|c| c.start_width), Some(W - SIDEBAR - PANE));
        assert_eq!(f.chat.map(|c| c.anchor), Some(Anchor::Left));
        let f = slides.step(want(true, false), ms(t0, 16 + CLOSE_MS / 2));
        assert!(f.pane_visible > 0.0 && f.pane_visible < PANE);
        let f = slides.step(want(true, false), ms(t0, 16 + CLOSE_MS));
        assert_eq!(f.pane_visible, 0.0);
        assert!(!f.animating());
    }

    #[test]
    fn collapsing_the_sidebar_slides_it_out_and_capped_rows_glide_with_the_centre() {
        // A 1400 pt window: 1104 → 1400, the rows are capped at both widths
        // (960 + 2 × 16 = 992 ≤ 1104), so the transcript is centred and the
        // final re-layout moves nothing.
        let wide = 1400.0;
        let t0 = Instant::now();
        let mut slides = PaneSlides::default();
        let mut w = want(true, false);
        w.viewport_width = wide;
        slides.step(w, t0);
        w.sidebar_shown = false;
        let f = slides.step(w, ms(t0, 16));
        assert!(f.sidebar_sliding);
        assert_eq!(f.sidebar_visible, SIDEBAR);
        assert!(!f.pane_sliding);
        let chat = f.chat.expect("chat slide");
        assert_eq!(chat.start_width, wide - SIDEBAR);
        assert_eq!(chat.anchor, Anchor::Centre);
        assert_eq!(chat.offset(wide), SIDEBAR / 2.0);
        let f = slides.step(w, ms(t0, 16 + CLOSE_MS));
        assert_eq!(f.sidebar_visible, 0.0);
        assert!(!f.animating());
        assert_eq!(f.chat, None);
        // Expand again: from 0 back to the full width, centred again.
        w.sidebar_shown = true;
        let f = slides.step(w, ms(t0, 1000));
        assert!(f.sidebar_sliding);
        assert_eq!(f.sidebar_visible, 0.0);
        assert_eq!(f.chat.map(|c| c.anchor), Some(Anchor::Centre));
        let f = slides.step(w, ms(t0, 1000 + OPEN_MS));
        assert_eq!(f.sidebar_visible, SIDEBAR);
        assert!(!f.animating());
    }

    #[test]
    fn at_the_default_width_the_transcript_anchors_away_from_the_moving_sidebar() {
        // 1180 pt window: 884 → 1180; the rows fill the column at 884 (852 <
        // 960), so the transcript keeps its right edge and the sidebar slides
        // away from (or over) its left end.
        let t0 = Instant::now();
        let mut slides = PaneSlides::default();
        slides.step(want(true, false), t0);
        let f = slides.step(want(false, false), ms(t0, 16));
        assert_eq!(f.chat.map(|c| c.anchor), Some(Anchor::Right));
        let chat = f.chat.unwrap();
        assert_eq!(chat.start_width, W - SIDEBAR);
        assert_eq!(chat.offset(W), SIDEBAR);
        assert_eq!(chat.offset(W - SIDEBAR), 0.0);
    }

    #[test]
    fn reduce_motion_snaps_both_panes() {
        let t0 = Instant::now();
        let mut slides = PaneSlides::default();
        let mut w = want(true, false);
        w.reduce_motion = true;
        slides.step(w, t0);
        w.pane_shown = true;
        let f = slides.step(w, ms(t0, 16));
        assert_eq!(f.pane_visible, PANE);
        assert!(!f.animating());
        assert_eq!(f.chat, None);
        w.sidebar_shown = false;
        let f = slides.step(w, ms(t0, 32));
        assert_eq!(f.sidebar_visible, 0.0);
        assert!(!f.animating());
        w.pane_shown = false;
        let f = slides.step(w, ms(t0, 48));
        assert_eq!(f.pane_visible, 0.0);
        assert!(!f.animating());
    }

    #[test]
    fn the_narrow_window_overlay_never_slides() {
        let t0 = Instant::now();
        let mut slides = PaneSlides::default();
        let mut w = want(true, false);
        w.narrow = true;
        w.sidebar_shown = false;
        slides.step(w, t0);
        w.sidebar_shown = true;
        let f = slides.step(w, ms(t0, 16));
        assert!(!f.sidebar_sliding);
        assert_eq!(f.sidebar_visible, SIDEBAR);
    }

    #[test]
    fn a_click_mid_slide_turns_round_from_where_the_pane_is() {
        let t0 = Instant::now();
        let mut slides = PaneSlides::default();
        slides.step(want(true, false), t0);
        slides.step(want(true, true), ms(t0, 16));
        let half = slides.step(want(true, true), ms(t0, 16 + OPEN_MS / 2));
        let at = half.pane_visible;
        assert!(at > 0.0 && at < PANE);
        // Closed again at once: the pane comes back from `at`, not from PANE.
        let f = slides.step(want(true, false), ms(t0, 16 + OPEN_MS / 2));
        assert!(f.pane_sliding);
        assert!(
            (f.pane_visible - at).abs() < 0.5,
            "{} vs {at}",
            f.pane_visible
        );
        let f = slides.step(want(true, false), ms(t0, 16 + OPEN_MS / 2 + 16));
        assert!(f.pane_visible < at);
        // And it is home well inside a full duration.
        let f = slides.step(want(true, false), ms(t0, 16 + OPEN_MS / 2 + CLOSE_MS));
        assert_eq!(f.pane_visible, 0.0);
        assert!(!f.animating());
    }

    #[test]
    fn a_page_swap_or_a_width_drag_does_not_slide() {
        let t0 = Instant::now();
        let mut slides = PaneSlides::default();
        slides.step(want(true, true), t0);
        // The sidebar is dragged wider: shown stays true, no slide.
        let mut w = want(true, true);
        w.sidebar_width = 320.0;
        let f = slides.step(w, ms(t0, 16));
        assert!(!f.animating());
        assert_eq!(f.sidebar_visible, 320.0);
        // The Me pane opens a wider settings page: shown stays true, no slide.
        w.pane_width = 592.0;
        let f = slides.step(w, ms(t0, 32));
        assert!(!f.animating());
        assert_eq!(f.pane_visible, 592.0);
    }

    #[test]
    fn capped_follows_the_column_cap() {
        assert!(capped(COLUMN_MAX_WIDTH + 2.0 * COLUMN_MARGIN));
        assert!(capped(W));
        assert!(!capped(COLUMN_MAX_WIDTH + 2.0 * COLUMN_MARGIN - 10.0));
        assert!(!capped(W - SIDEBAR - PANE));
    }
}

#[cfg(test)]
mod layout_tests {
    //! The clips at rest are today's static layout: a sidebar clipped to its
    //! full width, a pane clipped to its full width and a transcript clipped
    //! at the live width land exactly where the bare elements do; mid-slide
    //! the clips are the visible widths and the content moves with the
    //! clip's edge.

    use gpui::{div, px, TestAppContext};

    use super::*;

    const W: f32 = 1180.0;
    const H: f32 = 600.0;
    const SIDEBAR: f32 = 296.0;
    const PANE: f32 = 340.0;

    #[derive(Clone, Copy)]
    struct Layout {
        /// `None` = the bare three-pane row as `Shell::render` composes it at
        /// rest; `Some(frame)` = the same row through the slide clips.
        frame: Option<Frame>,
    }

    struct Row(Layout);

    impl gpui::Render for Row {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let sidebar = || {
                div()
                    .h_full()
                    .w(px(SIDEBAR))
                    .flex_shrink_0()
                    .debug_selector(|| "sidebar".into())
                    .child(
                        div()
                            .absolute()
                            .left(px(84.0))
                            .top(px(12.0))
                            .size(px(36.0))
                            .debug_selector(|| "collapse-disc".into()),
                    )
            };
            let pane = || {
                div()
                    .h_full()
                    .w(px(PANE))
                    .flex_shrink_0()
                    .debug_selector(|| "pane".into())
                    .child(
                        div()
                            .absolute()
                            .right(px(10.0))
                            .top(px(15.0))
                            .size(px(30.0))
                            .debug_selector(|| "close-x".into()),
                    )
            };
            let transcript = || {
                div()
                    .size_full()
                    .debug_selector(|| "transcript".into())
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .justify_center()
                            .px(px(COLUMN_MARGIN))
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(COLUMN_MAX_WIDTH))
                                    .h(px(40.0))
                                    .debug_selector(|| "chat-column".into()),
                            ),
                    )
                    .into_any_element()
            };
            let mut row = div().flex().flex_row().w(px(W)).h(px(H));
            match self.0.frame {
                None => {
                    row = row.child(sidebar());
                    row = row.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .debug_selector(|| "chat".into())
                            .child(transcript()),
                    );
                    row = row.child(pane());
                }
                Some(f) => {
                    if f.sidebar_sliding {
                        row = row.child(sidebar_clip(f.sidebar_visible, SIDEBAR, sidebar()));
                    } else if f.sidebar_visible > 0.0 {
                        row = row.child(sidebar());
                    }
                    let live = W - f.sidebar_visible - f.pane_visible;
                    let body = match f.chat {
                        Some(c) => transcript_clip(c, live, transcript()),
                        None => transcript(),
                    };
                    row = row.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .debug_selector(|| "chat".into())
                            .child(body),
                    );
                    if f.pane_sliding {
                        row = row.child(pane_clip(f.pane_visible, pane()));
                    } else if f.pane_visible > 0.0 {
                        row = row.child(pane());
                    }
                }
            }
            row
        }
    }

    fn bounds(cx: &mut TestAppContext, layout: Layout) -> Vec<(&'static str, (f32, f32, f32))> {
        let (_view, vcx) = cx.add_window_view(|_, _| Row(layout));
        vcx.run_until_parked();
        [
            "sidebar",
            "collapse-disc",
            "chat",
            "transcript",
            "chat-column",
            "pane",
            "close-x",
        ]
        .into_iter()
        .filter_map(|name| {
            vcx.debug_bounds(name).map(|b| {
                let x: f32 = b.origin.x.into();
                let w: f32 = b.size.width.into();
                let y: f32 = b.origin.y.into();
                (name, (x.round(), w.round(), y.round()))
            })
        })
        .collect()
    }

    #[gpui::test]
    fn at_rest_the_clipped_layout_is_the_static_layout(cx: &mut TestAppContext) {
        let plain = bounds(cx, Layout { frame: None });
        // The frame the tracker answers one tick after both slides end: the
        // panes at their full widths, nothing sliding, no chat clip.
        let rest = bounds(
            cx,
            Layout {
                frame: Some(Frame {
                    sidebar_visible: SIDEBAR,
                    sidebar_sliding: false,
                    pane_visible: PANE,
                    pane_sliding: false,
                    chat: None,
                }),
            },
        );
        assert_eq!(plain, rest);
        // And the last sliding frame (clips at the full widths, the transcript
        // at the final width, centred) is the same picture.
        let last = bounds(
            cx,
            Layout {
                frame: Some(Frame {
                    sidebar_visible: SIDEBAR,
                    sidebar_sliding: true,
                    pane_visible: PANE,
                    pane_sliding: true,
                    chat: Some(ChatSlide {
                        start_width: W - SIDEBAR - PANE,
                        anchor: Anchor::Left,
                    }),
                }),
            },
        );
        assert_eq!(plain, last);
    }

    #[gpui::test]
    fn mid_slide_the_panes_move_with_their_clips_and_the_rows_keep_their_width(
        cx: &mut TestAppContext,
    ) {
        // Sidebar half out (visible 100 of 296), pane half in (visible 170 of 340),
        // the transcript kept at the pre-slide width 884 and centred.
        let chat = ChatSlide {
            start_width: W - SIDEBAR,
            anchor: Anchor::Centre,
        };
        let b = bounds(
            cx,
            Layout {
                frame: Some(Frame {
                    sidebar_visible: 100.0,
                    sidebar_sliding: true,
                    pane_visible: 170.0,
                    pane_sliding: true,
                    chat: Some(chat),
                }),
            },
        );
        let get = |name: &str| b.iter().find(|(n, _)| *n == name).map(|(_, v)| *v);
        // The sidebar's content left with its right edge: the disc at 84 inside
        // a pane shifted by -196 sits at window x -112.
        assert_eq!(get("sidebar"), Some((-196.0, SIDEBAR, 0.0)));
        assert_eq!(get("collapse-disc"), Some((-112.0, 36.0, 12.0)));
        // The chat slot runs between the two clips.
        let live = W - 100.0 - 170.0;
        assert_eq!(get("chat"), Some((100.0, live, 0.0)));
        // The transcript keeps 884 and is centred in the 910 slot.
        assert_eq!(
            get("transcript"),
            Some((
                100.0 + (live - chat.start_width) / 2.0,
                chat.start_width,
                0.0
            ))
        );
        assert_eq!(
            get("chat-column").map(|v| v.1),
            Some((chat.start_width - 2.0 * COLUMN_MARGIN).min(COLUMN_MAX_WIDTH))
        );
        // The pane's left edge is the clip's left edge; its X is clipped away
        // past the window's right edge.
        assert_eq!(get("pane"), Some((W - 170.0, PANE, 0.0)));
        assert_eq!(get("close-x"), Some((W - 170.0 + PANE - 40.0, 30.0, 15.0)));
    }
}
