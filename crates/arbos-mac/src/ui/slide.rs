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
//! chat column is a flex sibling between the two, so its slot, header pill
//! and composer follow every frame, and so does the transcript: it is laid
//! out at the live width, so the bubbles move with either pane on the pane's
//! own curve instead of waiting and jumping when it settles ([feedback
//! mac-sidebar-slide-jank-2026-10-29], [feedback
//! mac-center-column-slide-2026-10-30]). The one exception is free: while
//! the rows are capped (`COLUMN_MAX_WIDTH`) at every width the column passes,
//! the live layout only re-centres them, so the transcript keeps the width it
//! had when the slide began ([`ChatSlide::start_width`]) and glides centred
//! in a clip, skipping the `list()`'s re-measure of every bubble.
//!
//! An edge drag from the rail is the one case the width follows the hand
//! instead of the clock: the rail morph is drawn at the pointer's x
//! ([`Want::sidebar_track`]) and settles from there on these same curves —
//! the drag unfolds exactly as it folds ([feedback
//! mac-sidebar-drag-open-2026-10-29]).
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

/// Rows at this chat-column width are capped (centred at `COLUMN_MAX_WIDTH`)
/// rather than filling it.
pub fn capped(center_width: f32) -> bool {
    center_width - 2.0 * COLUMN_MARGIN >= COLUMN_MAX_WIDTH - 0.5
}

/// A transcript held at its start width, centred in the live slot, while a
/// slide moves the column between widths that all cap the rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChatSlide {
    /// The column's width when the slide began.
    pub start_width: f32,
}

impl ChatSlide {
    /// The hold for a slide from `start_width` to `end_width`: only when the
    /// rows are capped at both, where it draws exactly what the live layout
    /// would. Anywhere else the transcript follows the live width.
    pub fn held(start_width: f32, end_width: f32) -> Option<ChatSlide> {
        (capped(start_width) && capped(end_width)).then_some(ChatSlide { start_width })
    }

    /// The transcript's left offset inside a slot `live_width` wide.
    pub fn offset(&self, live_width: f32) -> f32 {
        (live_width - self.start_width) / 2.0
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
    /// Set while a slide runs between widths that all cap the rows: the
    /// transcript keeps its start width, centred. `None`: the live width.
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
    /// The pointer-tracked width last drawn (`Want::sidebar_track`).
    last_sidebar_track: Option<f32>,
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
    /// An edge drag from the rail asks for this visible width (the pointer's
    /// x, `ui::sidebar_fit::EdgeDrag::track`): the rail morph follows it, no
    /// slide runs, and when it goes (the snap width crossed, or the hand let
    /// go) the sidebar settles from there on the toggle's slide.
    pub sidebar_track: Option<f32>,
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
        let track = want
            .sidebar_track
            .filter(|_| want.sidebar_shown && want.sidebar_rail && !want.narrow);

        // Where things were drawn last frame (for the chat's start width).
        let sidebar_before = match (self.last_sidebar_track, self.sidebar, self.last_sidebar) {
            (Some(tracked), _, _) => tracked,
            (None, Some(s), _) => s.value(now),
            (None, None, Some((shown, w))) => {
                if !shown {
                    0.0
                } else if self.last_sidebar_rail {
                    crate::ui::rail::metrics::WIDTH
                } else {
                    w
                }
            }
            (None, None, None) => sidebar_target,
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
        // A drag from the rail let go (or turned into the expanded column):
        // settle from the tracked width, as the toggle would from the rail.
        let track_ended = self
            .last_sidebar_track
            .is_some_and(|tracked| track.is_none() && (tracked - sidebar_target).abs() > 0.5);
        if let Some(tracked) = track {
            self.sidebar = None;
            if self.chat.is_none() {
                let start_width = want.viewport_width - sidebar_before - pane_before;
                let end_width = want.viewport_width - tracked.max(sidebar_full) - pane_before;
                self.chat = ChatSlide::held(start_width, end_width);
            }
        } else if (sidebar_changed || track_ended) && !snap && !want.narrow {
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
        self.last_sidebar_track = track;
        self.last_pane = Some((want.pane_shown, pane_full));

        // Finished slides come to rest at their target.
        if self.sidebar.is_some_and(|s| s.done(now)) {
            self.sidebar = None;
        }
        if self.pane.is_some_and(|s| s.done(now)) {
            self.pane = None;
        }
        // The sidebar's width moved under a running slide (an edge drag that
        // passed the width the unfold settles at): the slide turns towards
        // the new width from where it is, so the column never jumps.
        if let Some(s) = self.sidebar.filter(|s| (s.to - sidebar_target).abs() > 0.5) {
            self.sidebar =
                (!snap && !want.narrow).then(|| s.retarget(sidebar_target, sidebar_full, now));
        }
        // A pane slide whose target the state moved away from (a wider
        // settings page) is dropped rather than left pointing at a stale width.
        if self.pane.is_some_and(|s| (s.to - pane_target).abs() > 0.5) {
            self.pane = None;
        }
        let sidebar_visible =
            track.unwrap_or_else(|| self.sidebar.map_or(sidebar_target, |s| s.value(now)));
        let pane_visible = self.pane.map_or(pane_target, |s| s.value(now));
        let sidebar_moving = self.sidebar.is_some() || track.is_some();
        if !sidebar_moving && self.pane.is_none() {
            self.chat = None;
        }
        // Either pane moving: the held transcript only glides (centred) while
        // its rows are capped at every width the column passes — the start,
        // this frame's and the narrowest it can end at (a tracked drag may
        // still settle at the full width). Anywhere else the rows fill the
        // column, so a held layout sits still while the pane moves and jumps
        // by the whole distance at the end: the transcript is laid out at the
        // live width instead, as it is under a width drag ([feedback
        // mac-sidebar-slide-jank-2026-10-29], [feedback
        // mac-center-column-slide-2026-10-30]).
        if sidebar_moving || self.pane.is_some() {
            let narrowest_sidebar = match track {
                Some(tracked) => tracked.max(sidebar_full),
                None => sidebar_target.max(sidebar_visible),
            };
            let live = want.viewport_width - sidebar_visible - pane_visible;
            let end = want.viewport_width - narrowest_sidebar - pane_target.max(pane_visible);
            if self
                .chat
                .is_some_and(|c| !capped(c.start_width) || !capped(live) || !capped(end))
            {
                self.chat = None;
            }
        }

        Frame {
            sidebar_visible,
            sidebar_sliding: sidebar_moving,
            pane_visible,
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
            self.chat = ChatSlide::held(start_width, end_width);
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

/// The held transcript during a slide: kept at `slide.start_width`, centred
/// inside a clip that fills the live slot.
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
            sidebar_track: None,
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
        assert_eq!(f.chat, None, "884 pt fills with rows: laid out live");
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

    fn want_track(track: f32) -> Want {
        Want {
            sidebar_track: Some(track),
            ..want_rail(true)
        }
    }

    /// Scout's report, Jacob's "symmetric animations for folding the side bar
    /// in and out with the drag" ([feedback mac-sidebar-drag-open-2026-10-29]):
    /// before, a drag from the rail drew nothing until the snap width, then
    /// the unfold slide was dropped the moment the width moved under it — the
    /// column flipped open. Now the morph follows the pointer, crossing the
    /// snap width settles at the column's width on the open curve from where
    /// the hand left it, the column then chases the pointer without a jump,
    /// and a hand let go short of the snap width settles back into the rail
    /// on the close curve.
    #[test]
    fn dragging_the_rail_open_follows_the_pointer_then_settles_like_the_toggle() {
        let rail = crate::ui::rail::metrics::WIDTH;
        let t0 = Instant::now();
        let mut slides = PaneSlides::default();
        slides.step(want_rail(true), t0);
        // The hand pulls the rail's edge out: the morph is drawn at the pointer.
        let mut frame = 16;
        for x in [100.0, 140.0, 180.0, 215.0] {
            let f = slides.step(want_track(x), ms(t0, frame));
            assert!(f.sidebar_sliding, "the morph is drawn at {x}");
            assert_eq!(f.sidebar_visible, x);
            let p = f.rail_progress(SIDEBAR).expect("a rail morph");
            assert!(p > 0.0 && p < 1.0, "{p}");
            assert_eq!(f.chat, None, "it may settle at 296 (884 pt, rows fill it)");
            frame += 16;
        }
        // Past the snap width the state unfolds to 296: the open slide runs
        // from 215, not from the rail and not as a flip.
        let t1 = ms(t0, frame);
        let f = slides.step(want_rail(false), t1);
        assert!(f.sidebar_sliding);
        assert!(
            (f.sidebar_visible - 215.0).abs() < 0.5,
            "{}",
            f.sidebar_visible
        );
        let f = slides.step(want_rail(false), ms(t1, 48));
        assert!(f.sidebar_visible > 215.0 && f.sidebar_visible < SIDEBAR);
        // The pointer passes 296 mid-slide (the column now follows it): the
        // slide turns towards the new width from where it is — no jump.
        let before = f.sidebar_visible;
        let f = slides.step(
            Want {
                sidebar_width: 330.0,
                ..want_rail(false)
            },
            ms(t1, 64),
        );
        assert!(f.sidebar_sliding, "retargeted, not dropped");
        assert!(
            f.sidebar_visible >= before && f.sidebar_visible < before + 40.0,
            "{before} → {}",
            f.sidebar_visible
        );
        let f = slides.step(
            Want {
                sidebar_width: 330.0,
                ..want_rail(false)
            },
            ms(t1, 64 + OPEN_MS + 1),
        );
        assert_eq!(f.sidebar_visible, 330.0);
        assert!(!f.animating());

        // A drag let go short of the snap width: back into the rail on the
        // close curve, from where the hand left it.
        let mut slides = PaneSlides::default();
        slides.step(want_rail(true), t0);
        slides.step(want_track(160.0), ms(t0, 16));
        let f = slides.step(want_rail(true), ms(t0, 32));
        assert!(f.sidebar_sliding);
        assert!((f.sidebar_visible - 160.0).abs() < 0.5);
        let f = slides.step(want_rail(true), ms(t0, 32 + CLOSE_MS / 2));
        assert!(f.sidebar_visible > rail && f.sidebar_visible < 160.0);
        let f = slides.step(want_rail(true), ms(t0, 32 + CLOSE_MS + 1));
        assert_eq!(f.sidebar_visible, rail);
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
        // 884 → 544: the rows fill the column, so the transcript is laid out
        // at the live width and narrows with the pane.
        assert_eq!(f.chat, None);
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
        assert_eq!(f.chat, None, "544 → 884: the rows widen with the column");
        let f = slides.step(want(true, false), ms(t0, 16 + CLOSE_MS / 2));
        assert!(f.pane_visible > 0.0 && f.pane_visible < PANE);
        assert_eq!(f.chat, None);
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
        assert_eq!(f.chat.map(|c| c.start_width), Some(wide));
        let f = slides.step(w, ms(t0, 1000 + OPEN_MS));
        assert_eq!(f.sidebar_visible, SIDEBAR);
        assert!(!f.animating());
    }

    /// Scout on v0.6.350 ([feedback mac-sidebar-slide-jank-2026-10-29]): "the
    /// text bubbles teleport … sometimes chat messages lag behind". At the
    /// default 1180 pt window the rows fill the 884 pt column; the transcript
    /// used to be held at 884 against the slot's right edge, so the agent
    /// bubbles sat still while the sidebar moved and jumped the whole 296 (or
    /// 212 to the rail) when it settled. The sidebar's slides there lay the
    /// transcript out live, and so do the right pane's (Affinity and Scout,
    /// 2026-10-30, [feedback mac-center-column-slide-2026-10-30]: the pane
    /// slid but the column jumped to its new width when it settled).
    #[test]
    fn at_the_default_width_the_transcript_follows_either_moving_pane_live() {
        let t0 = Instant::now();
        let mut slides = PaneSlides::default();
        slides.step(want(true, false), t0);
        let f = slides.step(want(false, false), ms(t0, 16));
        assert!(f.sidebar_sliding);
        assert_eq!(f.chat, None);
        let f = slides.step(want(false, false), ms(t0, 16 + CLOSE_MS / 2));
        assert!(f.sidebar_sliding);
        assert_eq!(f.chat, None);
        // To the rail and back (884 ↔ 1096): the same.
        let mut slides = PaneSlides::default();
        slides.step(want_rail(false), t0);
        for i in 1..12 {
            let f = slides.step(want_rail(true), ms(t0, i * 16));
            assert_eq!(f.chat, None, "collapse frame {i}");
        }
        for i in 1..14 {
            let f = slides.step(want_rail(false), ms(t0, 400 + i * 16));
            assert_eq!(f.chat, None, "open frame {i}");
        }
        // The right pane in and out (884 ↔ 544): the same.
        let mut slides = PaneSlides::default();
        slides.step(want(true, false), t0);
        for i in 1..15 {
            let f = slides.step(want(true, true), ms(t0, i * 16));
            assert_eq!(f.chat, None, "pane open frame {i}");
        }
        for i in 1..13 {
            let f = slides.step(want(true, false), ms(t0, 400 + i * 16));
            assert_eq!(f.chat, None, "pane close frame {i}");
        }
    }

    /// A wide window holds the capped rows centred through a drag from the
    /// rail, and lets them go to the live width the frame the column the
    /// drag is heading for falls under the cap.
    #[test]
    fn a_held_transcript_is_let_go_once_the_column_falls_under_the_cap() {
        let wide = 1400.0;
        let rail = crate::ui::rail::metrics::WIDTH;
        let at = |w: Want| Want {
            viewport_width: wide,
            ..w
        };
        let t0 = Instant::now();
        let mut slides = PaneSlides::default();
        slides.step(at(want_rail(true)), t0);
        let f = slides.step(at(want_track(150.0)), ms(t0, 16));
        let chat = f.chat.expect("1316 → 1104: capped both ways, held");
        assert_eq!(chat.start_width, wide - rail);
        // Unfolded, then dragged on to 420: 980 pt, under the 992 cap.
        slides.step(at(want_rail(false)), ms(t0, 32));
        let f = slides.step(
            Want {
                sidebar_width: 420.0,
                ..at(want_rail(false))
            },
            ms(t0, 48),
        );
        assert!(f.sidebar_sliding);
        assert_eq!(f.chat, None);
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
        /// The window's width.
        width: f32,
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
            let width = self.0.width;
            let mut row = div().flex().flex_row().w(px(width)).h(px(H));
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
                    } else if f.sidebar_visible >= SIDEBAR {
                        row = row.child(sidebar());
                    } else if f.sidebar_visible > 0.0 {
                        // At rest in the rail: a column that wide.
                        row = row.child(div().h_full().w(px(f.sidebar_visible)).flex_shrink_0());
                    }
                    let live = width - f.sidebar_visible - f.pane_visible;
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
        let plain = bounds(
            cx,
            Layout {
                frame: None,
                width: W,
            },
        );
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
                width: W,
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
                    }),
                }),
                width: W,
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
                width: W,
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

    /// Scout on v0.6.350 ([feedback mac-sidebar-slide-jank-2026-10-29]):
    /// bubbles that teleport, or lag behind the sidebar; Affinity and Scout
    /// on 2026-10-30 ([feedback mac-center-column-slide-2026-10-30]): the
    /// right pane slid but the column jumped to its new width once it
    /// settled. Every frame the tracker answers for the sidebar's toggles
    /// (hide, rail, back), a drag out from the rail and the right pane's
    /// open and close puts the rows exactly where the static layout at that
    /// frame's live width does — at the default width, where the rows fill
    /// the column, and in wide windows, where they glide centred — so the
    /// column moves on the pane's own curve and nothing jumps when it ends.
    #[gpui::test]
    fn every_frame_of_a_pane_slide_lays_the_rows_out_at_the_live_width(cx: &mut TestAppContext) {
        let rail = crate::ui::rail::metrics::WIDTH;
        for width in [W, 1400.0, 1700.0] {
            let want = |shown: bool, collapsed: bool, track: Option<f32>, pane: bool| Want {
                viewport_width: width,
                sidebar_shown: shown,
                sidebar_width: SIDEBAR,
                sidebar_rail: collapsed,
                narrow: false,
                pane_shown: pane,
                pane_width: PANE,
                reduce_motion: false,
                sidebar_track: track,
            };
            let t0 = Instant::now();
            let mut script: Vec<(u64, Want)> = vec![(0, want(true, false, None, false))];
            let mut at = 0;
            let mut hold = |w: Want, frames: u64, script: &mut Vec<(u64, Want)>| {
                for _ in 0..frames {
                    at += 16;
                    script.push((at, w));
                }
            };
            hold(want(true, true, None, false), 13, &mut script);
            hold(want(true, false, None, false), 15, &mut script);
            hold(want(false, false, None, false), 13, &mut script);
            hold(want(true, false, None, false), 15, &mut script);
            hold(want(true, true, None, false), 13, &mut script);
            for x in [110.0, 150.0, 190.0, 215.0] {
                hold(want(true, true, Some(x), false), 1, &mut script);
            }
            hold(want(true, false, None, false), 15, &mut script);
            hold(want(true, false, None, true), 15, &mut script);
            hold(want(true, false, None, false), 13, &mut script);
            hold(want(true, true, None, true), 15, &mut script);
            hold(want(true, false, None, false), 15, &mut script);
            let mut slides = PaneSlides::default();
            let mut moving = 0;
            for (ms, w) in script {
                let f = slides.step(w, t0 + Duration::from_millis(ms));
                if !f.animating() {
                    continue;
                }
                moving += 1;
                let b = bounds(
                    cx,
                    Layout {
                        frame: Some(f),
                        width,
                    },
                );
                let (x, w_col, _) = b
                    .iter()
                    .find(|(n, _)| *n == "chat-column")
                    .map(|(_, v)| *v)
                    .expect("chat column drawn");
                let live = width - f.sidebar_visible - f.pane_visible;
                let col = (live - 2.0 * COLUMN_MARGIN).min(COLUMN_MAX_WIDTH);
                let want_x = f.sidebar_visible + (live - col) / 2.0;
                assert!(
                    (x - want_x).abs() <= 1.0 && (w_col - col).abs() <= 1.0,
                    "{width} pt window, sidebar at {} (rail {rail}), pane at {} at {ms} ms: \
                     rows at x {x} w {w_col}, the live layout puts them at x {want_x} w {col}",
                    f.sidebar_visible,
                    f.pane_visible
                );
            }
            assert!(moving > 100, "{moving} sliding frames checked");
        }
    }
}
