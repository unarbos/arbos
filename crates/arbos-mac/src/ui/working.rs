//! The working spinner: the braille cycle ⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ in a fixed glyph cell,
//! drawn wherever a list row says "this agent is working" — a sidebar chat
//! row's status cell under the time, the header's tab segment, a Settings ›
//! Machine agent row (Jacob, 2026-10-27, [feedback working-braille-spinner-mac-2026-10-27];
//! replaces the small green dot).
//!
//! One clock, one ticker. Every spinner on screen reads the same frame index
//! from [`WorkingClock`] (elapsed ÷ [`FRAME_MS`] mod 10), so they all turn in
//! step and no row owns a timer. A spinner asks for the next frame by being
//! drawn ([`glyph`] marks the clock `wanted`); the ticker runs only while
//! something was drawn during the last frame, and parks while the window is
//! in the background ([`WorkingClock::set_window_active`]). GPUI coalesces a
//! tick that lands on a frame the window draws anyway (a scroll), so the
//! ticker never adds frames to one. Reduce Motion draws the still glyph
//! [`STILL`] and asks for nothing.
//!
//! The cell is a fixed box ([`CELL_W`] × [`CELL_H`]) in the monospace face,
//! the glyph centred in it: the ten frames differ only in which dots are
//! set, so the cell never changes width or height and the row's layout is
//! the same on every frame.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{div, prelude::*, px, App, Bounds, Div, Global, Pixels};

use crate::ui::theme::{Theme, TEXT_TINY};

/// The classic cycle, in order.
pub const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// One frame every 90 ms (≈ 11 fps; the spec's 80–100 ms band).
pub const FRAME_MS: u64 = 90;

/// Reduce Motion: all six dots of the frames' cell (U+283F), nothing moves —
/// the same height as the moving glyphs (⣿ U+28FF would stand taller).
pub const STILL: &str = "⠿";

/// The glyph cell: the unread dot's footprint (8 pt) wide, so the spinner's
/// right edge is the dot's right edge under the time; 12 pt tall, centred on
/// the preview line like the dot was.
pub const CELL_W: f32 = 8.0;
pub const CELL_H: f32 = 12.0;

/// The glyph's size: the row's tiny text size, so the dots read at the
/// weight of the time label above them.
pub const GLYPH_SIZE: f32 = TEXT_TINY;

/// The shared clock and the one ticker behind every spinner.
pub struct WorkingClock {
    started: Instant,
    /// A spinner was drawn since the last tick (set by [`glyph`], taken by the tick).
    wanted: bool,
    /// The ticker task is alive.
    running: bool,
    /// The window is in the foreground; the ticker parks otherwise.
    active: bool,
    /// Trace counters (`ARBOS_WORKING_TRACE=1` prints them once a second).
    starts: u64,
    ticks: u64,
    /// Cells that reached prepaint since the last tick, by position: one key per
    /// spinner on screen, the value how many times that cell was drawn.
    drawn: HashMap<(i32, i32), u32>,
    drawn_last_frame: u64,
    draws_last_tick: u32,
    trace_at: Option<Instant>,
}

impl Global for WorkingClock {}

/// Read-only view of the ticker's counters for the tests (the trace line
/// reads the fields directly).
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trace {
    /// Ticker tasks spawned so far.
    pub starts: u64,
    /// Frames the ticker asked the windows for.
    pub ticks: u64,
    /// Distinct spinner cells on screen in the last tick.
    pub drawn_last_frame: u64,
    /// How many times each of them was drawn in that tick (1 = our frame only).
    pub draws_last_tick: u32,
    /// The ticker task is alive right now.
    pub running: bool,
}

impl WorkingClock {
    pub fn init(cx: &mut App) {
        if cx.has_global::<WorkingClock>() {
            return;
        }
        cx.set_global(WorkingClock {
            started: Instant::now(),
            wanted: false,
            running: false,
            active: true,
            starts: 0,
            ticks: 0,
            drawn: HashMap::new(),
            drawn_last_frame: 0,
            draws_last_tick: 0,
            trace_at: None,
        });
    }

    /// The frame index for a clock reading: `elapsed_ms / FRAME_MS`, round the
    /// ten frames.
    pub fn frame_at(elapsed_ms: u64) -> usize {
        ((elapsed_ms / FRAME_MS) % FRAMES.len() as u64) as usize
    }

    /// The frame every spinner shows right now.
    pub fn frame(cx: &App) -> usize {
        match cx.try_global::<WorkingClock>() {
            Some(c) => Self::frame_at(c.started.elapsed().as_millis() as u64),
            None => 0,
        }
    }

    /// A spinner is being rendered: keep the ticker going (or start it).
    pub fn request(cx: &mut App) {
        Self::init(cx);
        let c = cx.global_mut::<WorkingClock>();
        c.wanted = true;
        if !c.running && c.active {
            Self::spawn(cx);
        }
    }

    /// A spinner's cell reached prepaint — it is on screen this frame (a
    /// uniform list also *renders* a measuring row it never paints, so the
    /// count is taken here, not in `request`).
    fn note_drawn(bounds: Option<&Bounds<Pixels>>, cx: &mut App) {
        Self::init(cx);
        let key = bounds
            .map(|b| (f32::from(b.origin.x) as i32, f32::from(b.origin.y) as i32))
            .unwrap_or((0, 0));
        *cx.global_mut::<WorkingClock>()
            .drawn
            .entry(key)
            .or_insert(0) += 1;
    }

    /// The Shell reports the window's activation; the ticker parks in the
    /// background and resumes when the window comes back, if a spinner still
    /// wants frames.
    pub fn set_window_active(cx: &mut App, active: bool) {
        Self::init(cx);
        let c = cx.global_mut::<WorkingClock>();
        c.active = active;
        if active && c.wanted && !c.running {
            Self::spawn(cx);
        }
    }

    #[cfg(test)]
    pub fn trace(cx: &App) -> Trace {
        match cx.try_global::<WorkingClock>() {
            Some(c) => Trace {
                starts: c.starts,
                ticks: c.ticks,
                drawn_last_frame: c.drawn_last_frame,
                draws_last_tick: c.draws_last_tick,
                running: c.running,
            },
            None => Trace {
                starts: 0,
                ticks: 0,
                drawn_last_frame: 0,
                draws_last_tick: 0,
                running: false,
            },
        }
    }

    fn spawn(cx: &mut App) {
        {
            let c = cx.global_mut::<WorkingClock>();
            c.running = true;
            c.starts += 1;
        }
        cx.spawn(async move |cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(FRAME_MS))
                .await;
            let keep = cx.update(Self::tick);
            if !keep {
                break;
            }
        })
        .detach();
    }

    /// One tick: stop when nothing was drawn since the last one or the
    /// window is in the background; else ask every window for a frame.
    fn tick(cx: &mut App) -> bool {
        let trace = std::env::var_os("ARBOS_WORKING_TRACE").is_some();
        let c = cx.global_mut::<WorkingClock>();
        let drawn = std::mem::take(&mut c.drawn);
        c.drawn_last_frame = drawn.len() as u64;
        c.draws_last_tick = drawn.values().copied().max().unwrap_or(0);
        if !c.active {
            c.running = false;
            return false;
        }
        if !std::mem::take(&mut c.wanted) {
            c.running = false;
            if trace {
                eprintln!("working spinner: ticker parked (nothing drawn)");
            }
            return false;
        }
        c.ticks += 1;
        if trace {
            let due = c
                .trace_at
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(1));
            if due {
                c.trace_at = Some(Instant::now());
                let line = format!(
                    "working spinner: ticks={} starts={} spinners={} draws/tick={} frame={}",
                    c.ticks,
                    c.starts,
                    c.drawn_last_frame,
                    c.draws_last_tick,
                    Self::frame_at(c.started.elapsed().as_millis() as u64)
                );
                eprintln!("{line}");
            }
        }
        for window in cx.windows() {
            let _ = window.update(cx, |_, window, _| window.refresh());
        }
        true
    }
}

/// The spinner's cell: fixed box, monospace face, the glyph centred, in the
/// theme's muted text colour (the time label's — never the green). Call from
/// `render`; drawing it is what keeps the ticker alive.
pub fn glyph(theme: &Theme, cx: &mut App) -> Div {
    let text = if cx.reduce_motion() {
        STILL
    } else {
        WorkingClock::request(cx);
        FRAMES[WorkingClock::frame(cx)]
    };
    div()
        .w(px(CELL_W))
        .h(px(CELL_H))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .font_family(theme.font_mono)
        .text_size(px(GLYPH_SIZE))
        .line_height(px(CELL_H))
        .text_color(theme.text_muted)
        .on_children_prepainted(|bounds, _, cx| WorkingClock::note_drawn(bounds.first(), cx))
        .debug_selector(|| "working-spinner".into())
        .child(text)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use gpui::{div, prelude::*, px, TestAppContext};

    use super::{glyph, WorkingClock, CELL_H, CELL_W, FRAMES, FRAME_MS, STILL};
    use crate::ui::theme::{Surface, Theme};

    /// The classic cycle, in order, at 80–100 ms a frame; the index walks it
    /// one step per `FRAME_MS` and wraps after ten.
    #[test]
    fn the_frames_are_the_classic_cycle_at_eighty_to_a_hundred_ms() {
        assert_eq!(FRAMES.concat(), "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏");
        assert!((80..=100).contains(&FRAME_MS), "{FRAME_MS} ms a frame");
        assert_eq!(WorkingClock::frame_at(0), 0);
        assert_eq!(WorkingClock::frame_at(FRAME_MS - 1), 0);
        assert_eq!(WorkingClock::frame_at(FRAME_MS), 1);
        assert_eq!(WorkingClock::frame_at(FRAME_MS * 9), 9);
        assert_eq!(WorkingClock::frame_at(FRAME_MS * 10), 0, "wraps after ten");
        let walk: Vec<usize> = (0..10)
            .map(|i| WorkingClock::frame_at(i * FRAME_MS + 7))
            .collect();
        assert_eq!(walk, (0..10).collect::<Vec<_>>(), "one step per frame");
        // Every frame is one braille pattern: a single scalar in U+2800–U+28FF.
        for f in FRAMES.iter().chain([&STILL]) {
            let mut chars = f.chars();
            let c = chars.next().expect("a glyph");
            assert!(chars.next().is_none(), "{f:?} is one scalar");
            assert!(('\u{2800}'..='\u{28FF}').contains(&c), "{f:?} is braille");
        }
        assert_eq!(STILL, "⠿", "Reduce Motion: the full six-dot cell");
        assert_eq!(STILL.chars().next().map(u32::from), Some(0x283F));
        for f in FRAMES {
            let c = f.chars().next().expect("a glyph");
            assert!(
                u32::from(c) <= 0x283F,
                "{f:?} stays inside the six-dot cell"
            );
        }
    }

    /// Reduce Motion: the still glyph, and no frame asked for.
    #[gpui::test]
    fn reduce_motion_draws_the_still_glyph_and_asks_for_nothing(cx: &mut TestAppContext) {
        cx.update(|cx| {
            WorkingClock::init(cx);
            cx.set_global(Theme::dark(Surface::Matte));
            cx.set_reduce_motion(true);
            let theme = Theme::of(cx).clone();
            let _ = glyph(&theme, cx);
            let t = WorkingClock::trace(cx);
            assert!(!t.running, "no ticker under Reduce Motion");
            assert_eq!(t.starts, 0);
            cx.set_reduce_motion(false);
            let _ = glyph(&theme, cx);
            let t = WorkingClock::trace(cx);
            assert!(t.running, "motion on: drawing the glyph starts the ticker");
            assert_eq!(t.starts, 1);
            assert!(FRAMES.contains(&FRAMES[WorkingClock::frame(cx)]));
        });
    }

    /// N spinners in a view: one ticker task, one frame per `FRAME_MS`
    /// whatever N is, and the ticker parks by itself once nothing draws a
    /// spinner any more; the window going to the background parks it too,
    /// and it resumes with the window.
    struct Rows {
        spinners: usize,
    }

    impl gpui::Render for Rows {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            div()
                .flex()
                .flex_col()
                .children((0..self.spinners).map(|i| {
                    div()
                        .flex()
                        .items_center()
                        .h(px(20.0))
                        .child(div().w(px(100.0)).child(format!("row {i}")))
                        .child(glyph(&theme, cx))
                }))
        }
    }

    #[gpui::test]
    fn one_ticker_for_every_visible_row_and_it_parks_when_none_is_left(cx: &mut TestAppContext) {
        cx.update(|cx| {
            WorkingClock::init(cx);
            cx.set_global(Theme::dark(Surface::Matte));
        });
        let (view, vcx) = cx.add_window_view(|_, _| Rows { spinners: 12 });
        vcx.run_until_parked();
        let t = vcx.update(|_, cx| WorkingClock::trace(cx));
        assert!(t.running, "twelve spinners drawn → the ticker runs");
        assert_eq!(t.starts, 1, "one ticker, not one per row");

        // Ten frames: ten ticks, still one ticker, twelve spinners per frame.
        for _ in 0..10 {
            vcx.executor()
                .advance_clock(Duration::from_millis(FRAME_MS));
            vcx.run_until_parked();
        }
        let t = vcx.update(|_, cx| WorkingClock::trace(cx));
        assert_eq!(t.starts, 1, "still the one ticker after ten frames");
        assert_eq!(t.ticks, 10, "one frame per FRAME_MS, not twelve");
        assert_eq!(t.drawn_last_frame, 12, "every row's cell drawn per frame");

        // No spinner left on screen → the next tick parks the ticker.
        view.update(vcx, |rows, cx| {
            rows.spinners = 0;
            cx.notify();
        });
        vcx.run_until_parked();
        vcx.executor()
            .advance_clock(Duration::from_millis(FRAME_MS));
        vcx.run_until_parked();
        vcx.executor()
            .advance_clock(Duration::from_millis(FRAME_MS));
        vcx.run_until_parked();
        let t = vcx.update(|_, cx| WorkingClock::trace(cx));
        assert!(!t.running, "nothing drawn → parked");
        let parked_ticks = t.ticks;
        vcx.executor()
            .advance_clock(Duration::from_millis(FRAME_MS * 5));
        vcx.run_until_parked();
        assert_eq!(
            vcx.update(|_, cx| WorkingClock::trace(cx)).ticks,
            parked_ticks,
            "a parked ticker asks for no frames"
        );

        // Spinners back → one new ticker; window to the background → parked;
        // window back → running again, still only tickers that were needed.
        view.update(vcx, |rows, cx| {
            rows.spinners = 3;
            cx.notify();
        });
        vcx.run_until_parked();
        let t = vcx.update(|_, cx| WorkingClock::trace(cx));
        assert!(t.running);
        assert_eq!(t.starts, 2);
        vcx.update(|_, cx| WorkingClock::set_window_active(cx, false));
        vcx.executor()
            .advance_clock(Duration::from_millis(FRAME_MS));
        vcx.run_until_parked();
        let t = vcx.update(|_, cx| WorkingClock::trace(cx));
        assert!(!t.running, "background → parked");
        let background_ticks = t.ticks;
        vcx.executor()
            .advance_clock(Duration::from_millis(FRAME_MS * 5));
        vcx.run_until_parked();
        assert_eq!(
            vcx.update(|_, cx| WorkingClock::trace(cx)).ticks,
            background_ticks,
            "no frames while in the background"
        );
        vcx.update(|_, cx| WorkingClock::set_window_active(cx, true));
        let t = vcx.update(|_, cx| WorkingClock::trace(cx));
        assert!(t.running, "foreground again → the ticker resumes");
        assert_eq!(t.starts, 3);
    }

    /// The cell is a fixed box in every frame: the glyph changes, its bounds
    /// do not, so a row never re-flows while the spinner turns.
    #[gpui::test]
    fn the_cell_keeps_its_box_across_frames(cx: &mut TestAppContext) {
        cx.update(|cx| {
            WorkingClock::init(cx);
            cx.set_global(Theme::dark(Surface::Matte));
        });
        let (_view, vcx) = cx.add_window_view(|_, _| Rows { spinners: 1 });
        vcx.run_until_parked();
        let first = vcx.debug_bounds("working-spinner").expect("cell drawn");
        assert_eq!(f32::from(first.size.width), CELL_W);
        assert_eq!(f32::from(first.size.height), CELL_H);
        for _ in 0..12 {
            vcx.executor()
                .advance_clock(Duration::from_millis(FRAME_MS));
            vcx.run_until_parked();
            let b = vcx.debug_bounds("working-spinner").expect("cell drawn");
            assert_eq!(b, first, "the cell's box is the same in every frame");
        }
    }
}
