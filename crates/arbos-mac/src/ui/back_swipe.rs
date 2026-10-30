//! Two-finger swipe right = back, the Mac trackpad's "Swipe between pages"
//! (Jacob 2026-10-29: "still too hard and feels off"; every earlier fix was
//! iPhone-only and the Mac had no back swipe at all).
//!
//! The page being dismissed follows the fingers 1:1 from the first point.
//! On lift it goes back when it travelled 30 % of its width (at least
//! [`COMMIT_MIN`]) or when the fingers were still moving right faster than
//! [`FLICK_SPEED`]; otherwise a critically damped spring brings it home in
//! [`SETTLE`]. With nothing to go back to, a chevron disc rubber-bands in
//! from the left edge instead.
//!
//! Only a gesture's own `Started` event can begin a swipe: momentum after a
//! lift, a wheel mouse and a cancelled touch all arrive as `Moved` (the GPUI
//! fork reads `phase`, never `momentumPhase`) and are ignored. A gesture is
//! claimed after [`LOCK_DISTANCE`] of travel when it goes right and
//! [`AXIS_RATIO`] × more sideways than up or down; anything else is left to
//! the scroll views, and a claimed gesture stops propagation so a vertical
//! list never turns its sideways travel into scrolling. Gestures that start
//! over a sideways scroller ([`mark_sideways_scroller`]) stay with it.
//!
//! macOS settings are followed: no swipe at all when "Swipe between pages"
//! is not set to two fingers (`NSEvent.isSwipeTrackingFromScrollEventsEnabled`),
//! and the scroll delta's sign is flipped back to finger direction when
//! natural scrolling is off. The three-finger / Magic Mouse page swipe and a
//! mouse's Back button reach GPUI as `MouseButton::Navigate(Back)`; the shell
//! sends both to the same `Shell::go_back`.

use std::cell::Cell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui::{div, prelude::*, px, Div, Hsla};

use crate::ui::glass;
use crate::ui::theme::{Theme, CONTROL, CONTROL_GLYPH};

/// Travel before the gesture is claimed or left alone (points).
pub const LOCK_DISTANCE: f32 = 10.0;
/// A claimed gesture moves at least this many times more sideways than vertically.
pub const AXIS_RATIO: f32 = 1.5;
/// Share of the dismissed page's width that commits on lift.
pub const COMMIT_FRACTION: f32 = 0.30;
/// The commit distance never drops below this on a narrow page (points).
pub const COMMIT_MIN: f32 = 80.0;
/// Rightward finger speed at lift that commits regardless of distance (pt/s).
pub const FLICK_SPEED: f32 = 400.0;
/// The finger speed at lift is measured over this much of the gesture's end.
pub const VELOCITY_WINDOW: Duration = Duration::from_millis(50);
/// Spring back (or on to the edge) after lift.
pub const SETTLE: Duration = Duration::from_millis(250);
/// Natural frequency of the settle spring (rad/s): at ω·t = 7.5 the
/// remaining distance is under half a percent, so 250 ms ends without a jump.
const SPRING_OMEGA: f32 = 30.0;
/// How far the chevron hint travels at most, and the leftward rubber band of
/// a page (points).
pub const HINT_REACH: f32 = 56.0;
const LEFT_BAND: f32 = 24.0;

/// What a back swipe dismisses, in `Shell::go_back`'s order (Escape's order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackTarget {
    /// A pushed Settings page in my pane goes back to the Settings list.
    SettingsPage,
    /// A page pushed over another in the right pane pops.
    ProfilePage,
    /// The right pane's only page: the pane closes.
    ProfileClose,
    /// A server's channel list goes back to the chat list.
    ServerChannels,
    /// The Hidden chats list goes back to the chat list.
    HiddenChats,
}

/// Where the dismissed page is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Pane,
    Sidebar,
}

impl BackTarget {
    pub fn region(self) -> Region {
        match self {
            BackTarget::SettingsPage | BackTarget::ProfilePage | BackTarget::ProfileClose => {
                Region::Pane
            }
            BackTarget::ServerChannels | BackTarget::HiddenChats => Region::Sidebar,
        }
    }
}

/// The two macOS settings the gesture follows, read when a gesture starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwipePrefs {
    /// Trackpad / Mouse › "Swipe between pages" includes scrolling with two
    /// fingers (one on a Magic Mouse).
    pub enabled: bool,
    /// Natural scrolling: the scroll delta has the fingers' sign.
    pub natural: bool,
}

impl Default for SwipePrefs {
    fn default() -> Self {
        SwipePrefs {
            enabled: true,
            natural: true,
        }
    }
}

#[cfg(all(target_os = "macos", not(test)))]
pub fn system_prefs() -> SwipePrefs {
    use objc2_app_kit::NSEvent;
    use objc2_foundation::{ns_string, NSUserDefaults};
    let defaults = NSUserDefaults::standardUserDefaults();
    let key = ns_string!("com.apple.swipescrolldirection");
    SwipePrefs {
        enabled: NSEvent::isSwipeTrackingFromScrollEventsEnabled(),
        // Unset means the factory default, which is natural scrolling.
        natural: defaults.objectForKey(key).is_none() || defaults.boolForKey(key),
    }
}

#[cfg(any(not(target_os = "macos"), test))]
pub fn system_prefs() -> SwipePrefs {
    SwipePrefs::default()
}

thread_local! {
    #[cfg(test)]
    static TEST_NOW: Cell<Option<Instant>> = const { Cell::new(None) };
    /// Set by a sideways scroller under the pointer during the current gesture.
    static OVER_SIDEWAYS: Cell<bool> = const { Cell::new(false) };
}

/// The gesture clock; tests drive it with [`set_test_now`].
pub fn now() -> Instant {
    #[cfg(test)]
    {
        if let Some(t) = TEST_NOW.with(Cell::get) {
            return t;
        }
    }
    Instant::now()
}

#[cfg(test)]
pub fn set_test_now(t: Option<Instant>) {
    TEST_NOW.with(|c| c.set(t));
}

/// A sideways-scrolling frame keeps the gestures that start over it: its
/// wheel listener (bubble phase, so after the shell's capture listener has
/// reset the mark for a new gesture) marks the gesture as its own.
pub fn mark_sideways_scroller<E: InteractiveElement>(el: E) -> E {
    el.on_scroll_wheel(|_, _, _| OVER_SIDEWAYS.with(|c| c.set(true)))
}

/// What the shell draws this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Visual {
    /// `None`: nothing to go back to, the chevron hint shows.
    pub target: Option<BackTarget>,
    /// How far the page (or the hint) is moved right, in points; a page
    /// pulled left of its rest shows a small negative rubber band.
    pub offset: f32,
    /// Lifted: the spring is running and wants animation frames.
    pub settling: bool,
}

impl Visual {
    /// The hint's opacity, 0 at rest to 1 at full reach.
    pub fn hint_opacity(&self) -> f32 {
        (self.offset / HINT_REACH).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Debug)]
enum Phase {
    Idle,
    /// Touching, not yet claimed or left alone.
    Pending {
        dx: f32,
        dy: f32,
    },
    /// Left to the scroll views until the next `Started`.
    Passed,
    Tracking {
        target: Option<BackTarget>,
        width: f32,
        dx: f32,
        /// `(time, dx)` of recent events, for the speed at lift.
        samples: VecDeque<(Instant, f32)>,
    },
    Settling {
        target: Option<BackTarget>,
        width: f32,
        from: f32,
        to: f32,
        velocity: f32,
        started: Instant,
        commit: bool,
    },
}

/// What claiming needs from the shell: what goes back and how wide it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Claim {
    pub target: Option<BackTarget>,
    pub width: f32,
}

/// The gesture state, one per window.
#[derive(Debug)]
pub struct BackSwipe {
    phase: Phase,
    /// +1 with natural scrolling, −1 without: scroll delta × sign = finger travel.
    sign: f32,
}

impl Default for BackSwipe {
    fn default() -> Self {
        BackSwipe {
            phase: Phase::Idle,
            sign: 1.0,
        }
    }
}

impl BackSwipe {
    /// A gesture's `Started` event. `allowed` is false while something modal is up.
    pub fn begin(&mut self, prefs: SwipePrefs, allowed: bool) {
        OVER_SIDEWAYS.with(|c| c.set(false));
        if matches!(self.phase, Phase::Tracking { .. } | Phase::Settling { .. }) {
            // A second touch while the page is still moving: leave it be.
            return;
        }
        self.sign = if prefs.natural { 1.0 } else { -1.0 };
        self.phase = if prefs.enabled && allowed {
            Phase::Pending { dx: 0.0, dy: 0.0 }
        } else {
            Phase::Passed
        };
    }

    /// A `Moved` event's pixel delta. `claim` is asked once, when the gesture
    /// locks to a rightward swipe. Returns whether the event is the swipe's
    /// (the caller stops its propagation).
    pub fn moved(
        &mut self,
        delta: gpui::Point<f32>,
        now: Instant,
        claim: impl FnOnce() -> Claim,
    ) -> bool {
        let fdx = delta.x * self.sign;
        match &mut self.phase {
            Phase::Pending { dx, dy } => {
                *dx += fdx;
                *dy += delta.y;
                let (dx, dy) = (*dx, *dy);
                if dx.hypot(dy) < LOCK_DISTANCE {
                    return false;
                }
                let sideways = dx > 0.0 && dx > AXIS_RATIO * dy.abs();
                if !sideways || OVER_SIDEWAYS.with(Cell::get) {
                    self.phase = Phase::Passed;
                    return false;
                }
                let Claim { target, width } = claim();
                self.phase = Phase::Tracking {
                    target,
                    width: width.max(1.0),
                    dx,
                    samples: VecDeque::from([(now, dx)]),
                };
                true
            }
            Phase::Tracking { dx, samples, .. } => {
                *dx += fdx;
                samples.push_back((now, *dx));
                while samples
                    .front()
                    .is_some_and(|(t, _)| now.saturating_duration_since(*t) > VELOCITY_WINDOW * 2)
                {
                    samples.pop_front();
                }
                true
            }
            Phase::Idle | Phase::Passed | Phase::Settling { .. } => false,
        }
    }

    /// The fingers lifted (or the pointer moved, which a lost cancel leaves
    /// as the only sign). Returns the target to go back to right away when
    /// there is no spring to run (Reduce Motion), and whether the event was
    /// the swipe's.
    pub fn ended(&mut self, now: Instant, reduce_motion: bool) -> (Option<BackTarget>, bool) {
        let (target, width, dx, samples) = match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Tracking {
                target,
                width,
                dx,
                samples,
            } => (target, width, dx, samples),
            settling @ Phase::Settling { .. } => {
                self.phase = settling;
                return (None, false);
            }
            Phase::Idle | Phase::Pending { .. } | Phase::Passed => return (None, false),
        };
        let velocity = speed(&samples, now);
        let commit = target.is_some()
            && (dx >= (COMMIT_FRACTION * width).max(COMMIT_MIN) || velocity > FLICK_SPEED);
        if reduce_motion {
            return (target.filter(|_| commit), true);
        }
        let from = rest_offset(target, width, dx);
        self.phase = Phase::Settling {
            target,
            width,
            from,
            to: if commit { width } else { 0.0 },
            velocity,
            started: now,
            commit,
        };
        (None, true)
    }

    /// Drop the gesture without going back (the page it was moving went away).
    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
    }

    /// The page the gesture is moving, if any.
    pub fn target(&self) -> Option<Option<BackTarget>> {
        match &self.phase {
            Phase::Tracking { target, .. } | Phase::Settling { target, .. } => Some(*target),
            Phase::Idle | Phase::Pending { .. } | Phase::Passed => None,
        }
    }

    /// Once per frame: a finished spring that committed hands its target back
    /// (the caller goes back in the same frame, so nothing flashes).
    pub fn finish(&mut self, now: Instant) -> Option<BackTarget> {
        let Phase::Settling {
            target,
            started,
            commit,
            ..
        } = self.phase
        else {
            return None;
        };
        if now.saturating_duration_since(started) < SETTLE {
            return None;
        }
        self.phase = Phase::Idle;
        target.filter(|_| commit)
    }

    /// Where the page (or the hint) is this frame.
    pub fn visual(&self, now: Instant) -> Option<Visual> {
        match &self.phase {
            Phase::Tracking {
                target, width, dx, ..
            } => Some(Visual {
                target: *target,
                offset: rest_offset(*target, *width, *dx),
                settling: false,
            }),
            Phase::Settling {
                target,
                width,
                from,
                to,
                velocity,
                started,
                ..
            } => {
                let t = now.saturating_duration_since(*started).as_secs_f32();
                let x = spring(*from, *to, *velocity, t);
                let limit = if target.is_some() { *width } else { HINT_REACH };
                Some(Visual {
                    target: *target,
                    offset: x.clamp(0.0, limit),
                    settling: true,
                })
            }
            Phase::Idle | Phase::Pending { .. } | Phase::Passed => None,
        }
    }
}

/// Finger travel `dx` as drawn: 1:1 for a page up to its width, a short
/// rubber band past its rest on the left, and a rubber band for the hint.
fn rest_offset(target: Option<BackTarget>, width: f32, dx: f32) -> f32 {
    match target {
        Some(_) if dx >= 0.0 => dx.min(width),
        Some(_) => -rubber_band(-dx, LEFT_BAND),
        None => rubber_band(dx.max(0.0), HINT_REACH),
    }
}

/// Apple's scroll-view rubber band: grows with `x`, never past `reach`.
pub fn rubber_band(x: f32, reach: f32) -> f32 {
    (1.0 - 1.0 / (x * 0.55 / reach + 1.0)) * reach
}

/// Finger speed over the last [`VELOCITY_WINDOW`] before `now` (pt/s, + = right).
fn speed(samples: &VecDeque<(Instant, f32)>, now: Instant) -> f32 {
    let recent: Vec<&(Instant, f32)> = samples
        .iter()
        .filter(|(t, _)| now.saturating_duration_since(*t) <= VELOCITY_WINDOW)
        .collect();
    let (Some(first), Some(last)) = (recent.first(), recent.last()) else {
        return 0.0;
    };
    let span = last.0.saturating_duration_since(first.0).as_secs_f32();
    // Events under a millisecond apart say nothing about speed.
    if span < 0.001 {
        return 0.0;
    }
    (last.1 - first.1) / span
}

/// Critically damped spring from `from` towards `to` with initial `velocity`,
/// `t` seconds in; exactly `to` from [`SETTLE`] on.
fn spring(from: f32, to: f32, velocity: f32, t: f32) -> f32 {
    if t >= SETTLE.as_secs_f32() {
        return to;
    }
    let d = from - to;
    to + (d + (velocity + SPRING_OMEGA * d) * t) * (-SPRING_OMEGA * t).exp()
}

/// A right-pane page being popped: drawn `offset` to the right inside its
/// `width`-wide slot, over the pane's own surface.
pub fn dismissing(
    width: f32,
    offset: f32,
    surface: Hsla,
    edge: f32,
    edge_color: Hsla,
    page: impl IntoElement,
) -> Div {
    div()
        .h_full()
        .w(px(width))
        .flex_shrink_0()
        .relative()
        .overflow_hidden()
        .bg(surface)
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(edge))
                .bg(edge_color),
        )
        .child(
            div()
                .h_full()
                .w(px(width))
                .flex()
                .relative()
                .left(px(offset))
                .child(page),
        )
}

/// The chevron answering a swipe with nothing to go back to: the strip's
/// disc, sliding out from under `left` (the chat column's left edge) on the
/// rubber band and fading in with it.
pub fn hint(left: f32, visual: Visual, theme: &Theme) -> Div {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(px(left - CONTROL + visual.offset))
        .flex()
        .items_center()
        .opacity(visual.hint_opacity())
        .debug_selector(|| "back-swipe-hint".into())
        .child(glass::strip_disc(
            "back-swipe-hint",
            "chevron.left",
            CONTROL_GLYPH,
            theme,
        ))
}

#[cfg(test)]
mod tests {
    //! The real `Shell` over a real `AppState`, headless, fed the trackpad's
    //! wheel events: a group's pane with Sam's page pushed over it, and a
    //! sidebar long enough to scroll.
    use std::time::{Duration, Instant};

    use futures::future::BoxFuture;
    use gpui::{
        point, px, size, AppContext, Entity, Pixels, ScrollDelta, ScrollWheelEvent, TestAppContext,
        TouchPhase, VisualTestContext,
    };
    use tokio::sync::mpsc;

    use super::set_test_now;
    use crate::app::{AppState, ProfilePage};
    use crate::model::{AccountView, Person, Project, SpaceKind};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::motion::PulseClock;
    use crate::ui::shell::Shell;
    use crate::ui::theme::Theme;

    struct Idle;

    impl SyncSource for Idle {
        fn name(&self) -> &'static str {
            "idle"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            _commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            Box::pin(std::future::pending())
        }
    }

    fn person(id: &str, handle: &str, name: &str) -> Person {
        Person {
            id: id.into(),
            handle: handle.into(),
            name: Some(name.into()),
            status: "member".into(),
            role: "member".into(),
            ..Default::default()
        }
    }

    /// Over the right pane and over the sidebar's list.
    const PANE: gpui::Point<Pixels> = point(px(1000.0), px(300.0));
    const LIST: gpui::Point<Pixels> = point(px(150.0), px(400.0));

    struct Rig<'a> {
        _rt: tokio::runtime::Runtime,
        state: Entity<AppState>,
        vcx: &'a mut VisualTestContext,
        clock: Instant,
    }

    fn mount(cx: &mut TestAppContext) -> Rig<'_> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_reduce_motion(true);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        state.update(cx, |s, cx| {
            s.account = Some(AccountView {
                id: "gh_1".into(),
                login: "unconst".into(),
                name: "Jacob".into(),
                avatar_url: String::new(),
                device_name: "test".into(),
                balance_usd: 0.0,
                spent_today_usd: 0.0,
                daily_cap_usd: None,
            });
            s.projects.push(Project {
                id: "g1".into(),
                name: "Launch week".into(),
                kind: SpaceKind::Group,
                people: vec![
                    person("gh_2", "samir", "Sam Rivera"),
                    person("gh_3", "priya", "Priya Natarajan"),
                ],
                ..Project::default()
            });
            for i in 0..40 {
                s.projects.push(Project {
                    id: format!("p{i}"),
                    name: format!("Agent {i}"),
                    ..Project::default()
                });
            }
            s.selected_project = Some("g1".into());
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.simulate_resize(size(px(1180.0), px(780.0)));
        vcx.run_until_parked();
        state.update(vcx, |s, cx| {
            s.open_profile(
                ProfilePage::Group {
                    space_id: "g1".into(),
                },
                cx,
            )
        });
        push_sam(&shell, vcx);
        let clock = Instant::now();
        set_test_now(Some(clock));
        Rig {
            _rt: rt,
            state,
            vcx,
            clock,
        }
    }

    fn push_sam(shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        shell.update(vcx, |shell, cx| {
            shell.push_profile(
                ProfilePage::User {
                    handle: "samir".into(),
                    space_id: Some("g1".into()),
                },
                cx,
            )
        });
        vcx.run_until_parked();
    }

    impl Rig<'_> {
        fn wheel(&mut self, at: gpui::Point<Pixels>, phase: TouchPhase, dx: f32, dy: f32) {
            self.vcx.simulate_event(ScrollWheelEvent {
                position: at,
                delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
                modifiers: Default::default(),
                touch_phase: phase,
            });
        }

        /// One trackpad event `ms` after the last.
        fn moved(&mut self, at: gpui::Point<Pixels>, ms: u64, dx: f32, dy: f32) {
            self.clock += Duration::from_millis(ms);
            set_test_now(Some(self.clock));
            self.wheel(at, TouchPhase::Moved, dx, dy);
        }

        /// Fingers down, `steps` sideways moves of `step` points `ms` apart.
        fn drag(&mut self, steps: usize, step: f32, ms: u64) {
            self.wheel(PANE, TouchPhase::Started, 0.0, 0.0);
            for _ in 0..steps {
                self.moved(PANE, ms, step, 0.0);
            }
        }

        fn lift(&mut self) {
            self.wheel(PANE, TouchPhase::Ended, 0.0, 0.0);
        }

        fn on_sams_page(&mut self) -> bool {
            self.state.read_with(&*self.vcx, |s, _| {
                matches!(s.profile, Some(ProfilePage::User { .. })) && s.profile_stack.len() == 1
            })
        }

        fn top(&mut self, selector: &'static str) -> f32 {
            let b = self.vcx.debug_bounds(selector).expect("drawn");
            f32::from(b.top())
        }

        fn hero_left(&mut self) -> f32 {
            let b = self
                .vcx
                .debug_bounds("hero-subtitle")
                .expect("Sam's page drawn");
            f32::from(b.left())
        }
    }

    /// The page follows the fingers 1:1; 60 pt (18 % of the 340 pt pane)
    /// lifted slowly stays, 108 pt (past 30 % = 102 pt) goes back.
    #[gpui::test]
    fn a_swipe_past_thirty_percent_goes_back(cx: &mut TestAppContext) {
        let mut rig = mount(cx);
        let rest = rig.hero_left();

        rig.drag(15, 4.0, 16);
        assert_eq!(rig.hero_left(), rest + 60.0, "1:1 with the fingers");
        rig.lift();
        assert!(rig.on_sams_page(), "60 pt at 250 pt/s springs home");
        assert_eq!(rig.hero_left(), rest);

        rig.drag(25, 4.0, 16);
        assert_eq!(rig.hero_left(), rest + 100.0);
        rig.moved(PANE, 16, 4.0, 0.0);
        rig.moved(PANE, 16, 4.0, 0.0);
        rig.lift();
        let (page, stack) = rig
            .state
            .read_with(&*rig.vcx, |s, _| (s.profile.clone(), s.profile_stack.len()));
        assert_eq!(
            page,
            Some(ProfilePage::Group {
                space_id: "g1".into()
            }),
            "back to the group's pane"
        );
        assert_eq!(stack, 0);
    }

    /// 60 pt in 40 ms (~1500 pt/s, past the 400 pt/s flick speed) goes back
    /// although it is short of 30 %.
    #[gpui::test]
    fn a_flick_goes_back(cx: &mut TestAppContext) {
        let mut rig = mount(cx);
        rig.drag(6, 10.0, 8);
        rig.lift();
        assert!(!rig.on_sams_page(), "the flick popped Sam's page");
    }

    /// Momentum after a lift (and a wheel mouse) arrive as `Moved` without a
    /// `Started`: however far right they go, nothing moves or goes back.
    #[gpui::test]
    fn momentum_never_starts_a_swipe(cx: &mut TestAppContext) {
        let mut rig = mount(cx);
        let rest = rig.hero_left();
        for _ in 0..20 {
            rig.moved(PANE, 16, 40.0, 0.0);
        }
        assert!(rig.on_sams_page());
        assert_eq!(rig.hero_left(), rest);

        // A short swipe lifted, then its momentum tail.
        rig.drag(4, 4.0, 16);
        rig.lift();
        for _ in 0..20 {
            rig.moved(PANE, 16, 40.0, 0.0);
        }
        assert!(rig.on_sams_page(), "the tail does not go back");
        assert_eq!(rig.hero_left(), rest, "nor move the page");
    }

    /// A vertical gesture is left to the scroll view under it: the sidebar
    /// list scrolls and the pushed page stays.
    #[gpui::test]
    fn a_vertical_gesture_still_scrolls(cx: &mut TestAppContext) {
        let mut rig = mount(cx);
        let before = rig.top("row-preview-line-p5");
        rig.wheel(LIST, TouchPhase::Started, 0.0, 0.0);
        for _ in 0..10 {
            rig.moved(LIST, 16, 1.0, -20.0);
        }
        rig.wheel(LIST, TouchPhase::Ended, 0.0, 0.0);
        assert!(
            rig.top("row-preview-line-p5") < before - 100.0,
            "the list scrolled"
        );
        assert!(rig.on_sams_page(), "and nothing went back");
    }
}
