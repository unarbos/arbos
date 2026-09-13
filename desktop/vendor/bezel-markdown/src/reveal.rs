//! Text that rises into place one painted line at a time.
//!
//! The web client does this with kugiri: it asks the browser where it broke
//! the lines, wraps each one, and slides it up out of a clip. gpui hands us the
//! same facts — [`TextLayout`] knows every hard line and every soft wrap inside
//! it — so the split needs no DOM. Nothing here decides where a line breaks;
//! the text system did that already.
//!
//! [`LineReveal`] lays out one [`StyledText`] exactly as the paragraph would
//! have been laid out without it, so selection and click resolution see the
//! same [`TextLayout`]. Then, for every visual line still in motion, it sets
//! that line's bytes alone on one line, shifted down by what is left of the
//! rise, and paints it clipped to the line's box. Settled lines paint the
//! original through the same clip. When every line has settled the copies are
//! gone and the paragraph paints once, plain.
//!
//! [`RevealClock`] remembers when each line was first seen. A block that lands
//! whole staggers from its first line; a block still streaming rises one new
//! line at a time while the lines already read hold still.

use crate::doc::Part;
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Element, ElementId, GlobalElementId,
    Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, SharedString, StyledText, TextLayout,
    TextRun, Window, fill, point, size,
};
use motion::CubicBezier;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    ops::Range,
    panic::Location,
    rc::Rc,
    time::{Duration, Instant},
};

/// The web client's curve — CSS `cubic-bezier(0.23, 1, 0.32, 1)`.
const EASE: CubicBezier = CubicBezier::new(0.23, 1.0, 0.32, 1.0);

/// How a reveal moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pace {
    /// One line's rise.
    pub duration: Duration,
    /// Between one line and the next, when they land together.
    pub stagger: Duration,
    /// The stagger stops growing here, so a long block still finishes with a
    /// short one.
    pub max_delay: Duration,
    /// Past this many lines a block shows at once — a wall of text should not
    /// stage-manage itself for seconds.
    pub max_lines: usize,
    /// How far a line travels, as a share of its height. 1.0 is the full
    /// slide-up; Cursor's text barely moves, it fades in where it lands.
    pub travel: f32,
}

impl Default for Pace {
    /// Cursor's pace: a new line is readable within a quarter second, with
    /// a hint of lift rather than a slide. (`web/src/lib/reveal.ts` still
    /// carries the older, slower numbers.)
    fn default() -> Self {
        Self {
            duration: Duration::from_millis(240),
            stagger: Duration::from_millis(28),
            max_delay: Duration::from_millis(220),
            max_lines: 40,
            travel: 0.18,
        }
    }
}

/// Lines first seen this close together landed together, and stagger as one
/// run. Blocks of one document each read the clock on their own, a few
/// microseconds apart, inside one frame; a streamed line arrives well after.
const BATCH_WINDOW: Duration = Duration::from_millis(30);

#[derive(Clone, Copy)]
struct Seen {
    at: Instant,
    delay: Duration,
}

/// The lines that landed together, wherever in the document they sit. Shared
/// by every clock of one [`Reveal`], so a document that lands whole staggers
/// from its first line to its last rather than from the top of every block.
#[derive(Clone, Copy)]
struct Batch {
    at: Instant,
    count: u32,
}

/// How long after a reveal is told its text is already read that lines first
/// seen still count as read: the frame that is on screen, and no more.
const READ_WINDOW: Duration = Duration::from_millis(50);

struct Clock {
    pace: Pace,
    lines: Vec<Option<Seen>>,
    batch: Rc<RefCell<Option<Batch>>>,
    /// Lines first seen before this instant were already on screen and land
    /// where they stand.
    read_until: Option<Instant>,
}

/// When each painted line of one block was first seen.
///
/// Cloning shares the clock: the view keeps one per block across frames and
/// hands a clone to every [`LineReveal`] it renders.
#[derive(Clone)]
pub struct RevealClock(Rc<RefCell<Clock>>);

impl Default for RevealClock {
    fn default() -> Self {
        Self::new(Pace::default())
    }
}

impl RevealClock {
    pub fn new(pace: Pace) -> Self {
        Self::batched(pace, Rc::default(), None)
    }

    fn batched(
        pace: Pace,
        batch: Rc<RefCell<Option<Batch>>>,
        read_until: Option<Instant>,
    ) -> Self {
        Self(Rc::new(RefCell::new(Clock {
            pace,
            lines: Vec::new(),
            batch,
            read_until,
        })))
    }

    pub fn pace(&self) -> Pace {
        self.0.borrow().pace
    }

    /// Forget every line, so the next paint rises from the first one again.
    pub fn restart(&self) {
        self.0.borrow_mut().lines.clear();
    }

    /// True once every line seen so far has finished its rise.
    pub fn is_settled(&self) -> bool {
        let clock = self.0.borrow();
        let now = Instant::now();
        clock
            .lines
            .iter()
            .flatten()
            .all(|seen| now >= seen.at + seen.delay + clock.pace.duration)
    }

    /// Where line `ix` is in its rise right now, if it has been seen. For
    /// paint that follows a line without owning it — a wash under a word, a
    /// marker beside a row.
    pub fn peek(&self, ix: usize) -> Option<f32> {
        let clock = self.0.borrow();
        let seen = clock.lines.get(ix).copied().flatten()?;
        Some(eased(seen, clock.pace, Instant::now()))
    }

    /// Where line `ix` of `count` is in its rise at `now`, 0 to 1, eased.
    /// A line seen for the first time starts now; lines that landed together
    /// stagger after one another.
    fn progress(&self, ix: usize, count: usize, now: Instant) -> f32 {
        let mut clock = self.0.borrow_mut();
        if clock.lines.len() < count {
            clock.lines.resize(count, None);
        }
        let seen = match clock.lines[ix] {
            Some(seen) => seen,
            None => {
                let read = clock.read_until.is_some_and(|until| now < until);
                let seen = if read || count > clock.pace.max_lines {
                    Seen {
                        at: now - clock.pace.duration,
                        delay: Duration::ZERO,
                    }
                } else {
                    let mut batch = clock.batch.borrow_mut();
                    let current = match *batch {
                        Some(current) if now.duration_since(current.at) < BATCH_WINDOW => Batch {
                            at: current.at,
                            count: current.count + 1,
                        },
                        _ => Batch { at: now, count: 1 },
                    };
                    *batch = Some(current);
                    Seen {
                        at: current.at,
                        delay: (clock.pace.stagger * (current.count - 1)).min(clock.pace.max_delay),
                    }
                };
                clock.lines[ix] = Some(seen);
                seen
            }
        };
        eased(seen, clock.pace, now)
    }
}

fn eased(seen: Seen, pace: Pace, now: Instant) -> f32 {
    let Some(elapsed) = now.checked_duration_since(seen.at + seen.delay) else {
        return 0.0;
    };
    EASE.eval(elapsed.as_secs_f32() / pace.duration.as_secs_f32())
}

/// One document's reveal: a clock for every text block it has painted, and
/// how they all move. Handed to [`crate::render_revealed`]; the owner keeps
/// it across frames.
pub struct Reveal {
    pace: Pace,
    veil: Option<Hsla>,
    clocks: RefCell<BTreeMap<(usize, Part), RevealClock>>,
    batch: Rc<RefCell<Option<Batch>>>,
    read_until: Cell<Option<Instant>>,
}

impl Default for Reveal {
    fn default() -> Self {
        Self::new(Pace::default())
    }
}

impl Reveal {
    pub fn new(pace: Pace) -> Self {
        Self {
            pace,
            veil: None,
            clocks: RefCell::new(BTreeMap::new()),
            batch: Rc::default(),
            read_until: Cell::new(None),
        }
    }

    /// The text on screen now has been read: it lands where it stands, and
    /// only what arrives after this frame rises. For a reveal started on a
    /// document the reader has already seen part of.
    pub fn already_read(&self) {
        self.read_until.set(Some(Instant::now() + READ_WINDOW));
    }

    /// Fade as well as rise; see [`LineReveal::veil`].
    pub fn with_veil(mut self, background: Option<Hsla>) -> Self {
        self.veil = background;
        self
    }

    pub fn pace(&self) -> Pace {
        self.pace
    }

    pub fn veil(&self) -> Option<Hsla> {
        self.veil
    }

    /// The clock for one text part of one block, started on first sight.
    pub fn clock(&self, block: usize, part: Part) -> RevealClock {
        self.clocks
            .borrow_mut()
            .entry((block, part))
            .or_insert_with(|| {
                RevealClock::batched(self.pace, self.batch.clone(), self.read_until.get())
            })
            .clone()
    }

    /// Forget every block, so the whole document rises again.
    pub fn restart(&self) {
        self.clocks.borrow_mut().clear();
        self.batch.borrow_mut().take();
        self.read_until.set(None);
    }

    /// True once every block seen so far is at rest.
    pub fn is_settled(&self) -> bool {
        self.clocks.borrow().values().all(RevealClock::is_settled)
    }
}

/// One visual line mid-rise: its box, and the copy painted through it.
pub struct Rising {
    range: Range<usize>,
    clip: Bounds<Pixels>,
    progress: f32,
    copy: Option<AnyElement>,
}

/// The paragraph as it will paint this frame.
pub enum Frame {
    /// Every line is at rest, or motion is off: the text paints once.
    Plain,
    Rising {
        lines: Vec<Rising>,
        /// Room under each line box for its descenders.
        pad: Pixels,
    },
}

/// A [`StyledText`] whose lines rise into place. See the module docs.
pub struct LineReveal {
    text: SharedString,
    runs: Option<Vec<TextRun>>,
    main: StyledText,
    layout: TextLayout,
    clock: RevealClock,
    veil: Option<Hsla>,
}

impl LineReveal {
    /// `runs` styles the text as [`StyledText::with_runs`] would; `None`
    /// takes the window's text style whole.
    pub fn new(text: SharedString, runs: Option<Vec<TextRun>>, clock: RevealClock) -> Self {
        let main = styled(text.clone(), runs.clone());
        let layout = main.layout().clone();
        Self {
            text,
            runs,
            main,
            layout,
            clock,
            veil: None,
        }
    }

    /// Fade as well as rise. gpui has no per-paint opacity to give away, so
    /// the fade is a wash of the background colour over the line, thinning as
    /// the line settles — right on a solid ground, wrong over anything else.
    pub fn veil(mut self, background: Hsla) -> Self {
        self.veil = Some(background);
        self
    }

    /// The same layout a bare [`StyledText`] would have given, for a caller
    /// resolving selection and clicks against the text.
    pub fn layout(&self) -> &TextLayout {
        &self.layout
    }

    /// The text's visual lines, top to bottom: the bytes each one holds and
    /// the box it was painted in. Read off the layout the text system made —
    /// a hard line per newline, a soft wrap wherever it broke one.
    fn visual_lines(&self, bounds: Bounds<Pixels>) -> Vec<Line> {
        let line_height = self.layout.line_height();
        let mut lines = Vec::new();
        let mut base = 0;
        for hard in self.layout.line_layouts() {
            let glyphs = &hard.unwrapped_layout.runs;
            let mut start = 0;
            for boundary in hard.wrap_boundaries() {
                let end = glyphs[boundary.run_ix].glyphs[boundary.glyph_ix].index;
                lines.push(Line::new(
                    base + start..base + end,
                    bounds,
                    line_height,
                    lines.len(),
                ));
                start = end;
            }
            lines.push(Line::new(
                base + start..base + hard.len(),
                bounds,
                line_height,
                lines.len(),
            ));
            base += hard.len() + 1;
        }
        lines
    }

    /// How far the face reaches below its baseline. A line box is clipped
    /// this much lower than its own height, so a descender is not cut off
    /// while the line below is still on its way.
    fn descent(&self) -> Pixels {
        self.layout
            .line_layouts()
            .first()
            .map(|line| line.unwrapped_layout.descent)
            .unwrap_or_default()
    }

    /// One line's text and the runs that style it, trailing space dropped so
    /// the copy cannot wrap where the paragraph did not.
    fn excerpt(&self, range: Range<usize>) -> Option<(SharedString, Option<Vec<TextRun>>)> {
        let text = self.text.get(range.clone())?.trim_end();
        if text.is_empty() {
            return None;
        }
        let range = range.start..range.start + text.len();
        let runs = self.runs.as_ref().map(|runs| slice_runs(runs, range));
        Some((SharedString::from(text.to_string()), runs))
    }
}

/// One visual line: its bytes in the text and its box on screen.
struct Line {
    range: Range<usize>,
    clip: Bounds<Pixels>,
}

impl Line {
    fn new(range: Range<usize>, bounds: Bounds<Pixels>, line_height: Pixels, ix: usize) -> Self {
        Self {
            range,
            clip: Bounds::new(
                point(bounds.origin.x, bounds.origin.y + line_height * ix as f32),
                size(bounds.size.width, line_height),
            ),
        }
    }
}

fn styled(text: SharedString, runs: Option<Vec<TextRun>>) -> StyledText {
    match runs {
        Some(runs) => StyledText::new(text).with_runs(runs),
        None => StyledText::new(text),
    }
}

/// The part of `runs` that styles `range`, cut at both ends.
fn slice_runs(runs: &[TextRun], range: Range<usize>) -> Vec<TextRun> {
    let mut out = Vec::new();
    let mut start = 0;
    for run in runs {
        let end = start + run.len;
        let lo = start.max(range.start);
        let hi = end.min(range.end);
        if lo < hi {
            let mut cut = run.clone();
            cut.len = hi - lo;
            out.push(cut);
        }
        start = end;
    }
    out
}

impl Element for LineReveal {
    type RequestLayoutState = ();
    type PrepaintState = Frame;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        self.main.request_layout(None, None, window, cx)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Frame {
        self.main.prepaint(None, None, bounds, &mut (), window, cx);
        if cx.reduce_motion() {
            return Frame::Plain;
        }

        let lines = self.visual_lines(bounds);
        let count = lines.len();
        let now = Instant::now();
        let mut rising: Vec<Rising> = lines
            .into_iter()
            .enumerate()
            .map(|(ix, line)| Rising {
                range: line.range,
                clip: line.clip,
                progress: self.clock.progress(ix, count, now),
                copy: None,
            })
            .collect();
        if rising.iter().all(|line| line.progress >= 1.0) {
            return Frame::Plain;
        }

        // A copy of each line in motion — that line's text alone, set on one
        // line, sitting lower by what is left to rise. Only its own glyphs
        // can show through its clip; the lines around it are not in it.
        // Lines not yet started paint nothing and need none.
        let line_height = self.layout.line_height();
        let space = size(AvailableSpace::MaxContent, AvailableSpace::MaxContent);
        for line in rising
            .iter_mut()
            .filter(|line| line.progress > 0.0 && line.progress < 1.0)
        {
            let Some((text, runs)) = self.excerpt(line.range.clone()) else {
                continue;
            };
            let mut copy = styled(text, runs).into_any_element();
            let rise = line_height * self.clock.pace().travel * (1.0 - line.progress);
            copy.prepaint_as_root(
                point(line.clip.origin.x, line.clip.origin.y + rise),
                space,
                window,
                cx,
            );
            line.copy = Some(copy);
        }
        window.request_animation_frame();
        Frame::Rising {
            lines: rising,
            pad: self.descent().min(line_height * 0.3),
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        frame: &mut Frame,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Frame::Rising { lines, pad } = frame else {
            self.main
                .paint(None, None, bounds, &mut (), &mut (), window, cx);
            return;
        };
        let pad = *pad;
        let padded = |clip: Bounds<Pixels>| {
            Bounds::new(clip.origin, size(clip.size.width, clip.size.height + pad))
        };

        // The lines that have landed are a run from the top, since lines
        // start in order and rise for the same time. They are the paragraph
        // itself, seen through one box down to the last of them — one box,
        // not one per line, so no descender is cut where two lines meet.
        let landed = lines
            .iter()
            .take_while(|line| line.progress >= 1.0)
            .count();
        if let Some(last) = lines.get(landed.wrapping_sub(1)) {
            let clip = Bounds::from_corners(
                bounds.origin,
                point(bounds.right(), last.clip.bottom()),
            );
            window.with_content_mask(Some(ContentMask { bounds: padded(clip) }), |window| {
                self.main
                    .paint(None, None, bounds, &mut (), &mut (), window, cx);
            });
        }

        for line in lines[landed..].iter_mut() {
            if line.progress <= 0.0 {
                continue;
            }
            let clip = padded(line.clip);
            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                // A line landed out of turn is the paragraph seen through its
                // own box; a line in motion is its copy.
                if line.progress >= 1.0 {
                    self.main
                        .paint(None, None, bounds, &mut (), &mut (), window, cx);
                    return;
                }
                if let Some(copy) = line.copy.as_mut() {
                    copy.paint(window, cx);
                }
                if let Some(veil) = self.veil {
                    window.paint_quad(fill(clip, veil.opacity(1.0 - line.progress)));
                }
            });
        }
    }
}

impl IntoElement for LineReveal {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// Any element that rises with the first line of a clock — a list's marker
/// beside the text it belongs to, so the disc does not sit there before the
/// words. It is the clock's line 0, whether it or the text sees that line
/// first.
pub struct Rise {
    child: AnyElement,
    clock: RevealClock,
    veil: Option<Hsla>,
}

impl Rise {
    pub fn new(child: AnyElement, clock: RevealClock, veil: Option<Hsla>) -> Self {
        Self { child, clock, veil }
    }
}

impl Element for Rise {
    type RequestLayoutState = ();
    /// The clip and the progress while in motion; `None` paints plainly.
    type PrepaintState = Option<(Bounds<Pixels>, f32)>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<(Bounds<Pixels>, f32)> {
        let progress = if cx.reduce_motion() {
            1.0
        } else {
            self.clock.progress(0, 1, Instant::now())
        };
        if progress >= 1.0 {
            self.child.prepaint(window, cx);
            return None;
        }
        let rise = bounds.size.height * self.clock.pace().travel * (1.0 - progress);
        window.with_element_offset(point(Pixels::ZERO, rise), |window| {
            self.child.prepaint(window, cx)
        });
        window.request_animation_frame();
        Some((bounds, progress))
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        motion: &mut Option<(Bounds<Pixels>, f32)>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some((clip, progress)) = *motion else {
            self.child.paint(window, cx);
            return;
        };
        if progress <= 0.0 {
            return;
        }
        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
            self.child.paint(window, cx);
            if let Some(veil) = self.veil {
                window.paint_quad(fill(clip, veil.opacity(1.0 - progress)));
            }
        });
    }
}

impl IntoElement for Rise {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}
