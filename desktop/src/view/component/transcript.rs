//! The transcript — one zone per turn: the question, the work it took, the
//! answer.
//!
//! The zone split is a `rposition`: **the answer is the prose after the last
//! tool call or thought; everything before it is interim.** That one rule is
//! what stops a model's thinking-out-loud being presented as its reply.

use crate::{
    model::{
        attachment::{MessageImage, Prompt, UserMessage},
        session::{Artifact, ArtifactKind, ChatItem, ChatSession, PlanNode, ToolStatus},
        workspace::Workspace,
    },
    reading,
    view::{component::attachment, root},
};
use bezel::{
    gpui::{
        AnyElement, ClipboardItem, Context, Empty, Hsla, Pixels, ScrollHandle, SharedString,
        StyledText, TextRun, Window, canvas, div, font, img, point, prelude::*, px, rgb, svg,
    },
    motion::Painter,
    theme::{HighlightKind, TextStyle, Theme, Typeset, ink},
    ui::{
        icons,
        scroll::{FOLLOW_SLACK, at_bottom},
        tooltip::Tooltip,
        widgets::{Buttons, Layout, Status, Takeover},
    },
};
use cacp::schema::ToolKind;
use markdown::{
    BlockLayouts, Cursor, Doc, Mark, Reveal, Selection,
    selectable::{self, Pointer},
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    ops::Range,
    rc::Rc,
    time::{Duration, Instant},
};

/// Web `transcript-col`: `px-3.5 py-4`, `space-y-2`.
const PAD: f32 = 16.;
const ITEM_GAP: f32 = 8.;
const ROW_GAP: f32 = 6.;
const CARD_PAD_X: f32 = 12.;
const CARD_PAD_Y: f32 = 6.;
const CARD_RADIUS: f32 = 8.;
const PROMPT_PAD_Y: f32 = 10.;
/// The user's card: Cursor's ~10 px corners, and no wider than most of the
/// reading column so the answer under it reads as a reply.
const PROMPT_RADIUS: f32 = 10.;
/// Jacob's Mac reference (`chat-2026-09-14/03`, 2x): the bubble is 483 pt
/// of a 689 pt column — 70 % — with 12 pt side padding and a 10 pt radius.
const PROMPT_MAX_WIDTH: f32 = (root::CHAT_MAX_WIDTH - 2. * root::CHAT_GUTTER) * 0.70;
const PROMPT_PAD_X: f32 = 12.;
/// Web diff/terminal: `text-[11.5px] leading-[1.5]`.
const MONO_SIZE: f32 = 11.5;
const MONO_LEAD: f32 = 17.;

/// The TUI's braille frames, and the 80ms tick the web composer uses with them.
// ASCII only. Braille cells fall back to a tofu or a ⋮ in the UI font,
// which reads as a menu, not a spinner.
const BRAILLE: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BRAILLE_TICK_MS: u128 = 80;
const BRAILLE_FPS: f32 = 12.5;
const BRAILLE_LEASE: Duration = Duration::from_millis(300);
/// How long the footer copy glyph stays a check after a click.
const COPY_FLASH: Duration = Duration::from_millis(1250);

/// A label as one shaped run on the UI face, in `color`.
fn spaced_label(text: impl AsRef<str>, color: Hsla, theme: &Theme) -> StyledText {
    let text = text.as_ref();
    let run = TextRun {
        len: text.len(),
        font: font(theme.font_sans.clone()),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    StyledText::new(SharedString::from(text.to_string())).with_runs(vec![run])
}

/// One sweep of the shimmer, left to right and off the end.
const SHIMMER_PERIOD_MS: u128 = 1800;
const SHIMMER_FPS: f32 = 30.;

/// Cursor's working label: muted text with a brighter band sliding across
/// it, over and over, while something runs. Each character is its own run
/// so the band can fall between letters. `since` sets the phase, so a row
/// that re-renders keeps its place in the sweep. Under Reduce Motion the
/// label sits still in the muted colour.
/// A shimmering line for another view (the kickoff view's "Setting up
/// environment"): the same paint as the heartbeat's words.
pub(crate) fn shimmer_line<V: 'static>(
    text: &str,
    since: Duration,
    theme: &Theme,
    cx: &mut Context<V>,
) -> AnyElement {
    shimmer_label(text, since, theme, cx).into_any_element()
}

fn shimmer_label<V: 'static>(
    text: impl AsRef<str>,
    since: Duration,
    theme: &Theme,
    cx: &mut Context<V>,
) -> StyledText {
    let text = text.as_ref();
    let base = theme.text_faint;
    if cx.reduce_motion() {
        return spaced_label(text, theme.text_muted, theme);
    }
    Painter::of(cx).lease(SHIMMER_FPS, BRAILLE_LEASE, cx);
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len().max(1) as f32;
    // The band runs from just before the first letter to just past the last.
    let phase = (since.as_millis() % SHIMMER_PERIOD_MS) as f32 / SHIMMER_PERIOD_MS as f32;
    let centre = phase * (n + 10.) - 5.;
    let runs = chars
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let d = (i as f32 - centre).abs();
            // A wide, soft band: half its width is five letters, and it
            // never quite reaches full body-text brightness.
            let lift = (1. - d / 5.).clamp(0., 1.);
            let lift = 0.85 * lift * lift * (3. - 2. * lift);
            TextRun {
                len: ch.len_utf8(),
                font: font(theme.font_sans.clone()),
                color: bezel::theme::mix(base, theme.text, lift),
                background_color: None,
                underline: None,
                strikethrough: None,
            }
        })
        .collect();
    StyledText::new(SharedString::from(text.to_string())).with_runs(runs)
}

/// A card body's scroll handle and the clamped offset it last had.
#[derive(Clone, Default)]
struct ScrollBox {
    handle: ScrollHandle,
    last: Rc<Cell<Pixels>>,
}

/// Browser scroll chaining. gpui hands the wheel to every scroll container
/// under the pointer, so a card body and the transcript both moved. The box's
/// own handler runs first; this listener then checks whether it moved the
/// box (clamped, since gpui only clamps at the next prepaint) and, if so,
/// keeps the event from reaching the page. At the box's end the page scrolls.
fn chain_scroll(
    el: bezel::gpui::Stateful<bezel::gpui::Div>,
    scroll: ScrollBox,
) -> bezel::gpui::Stateful<bezel::gpui::Div> {
    let ScrollBox { handle, last } = scroll;
    el.track_scroll(&handle).on_scroll_wheel(move |_, _, cx| {
        let max = handle.max_offset().y;
        let now = handle.offset().y.clamp(-max, px(0.));
        let moved = now != last.get();
        last.set(now);
        if moved {
            cx.stop_propagation();
        }
    })
}

/// Stick-to-bottom, with an explicit unpin for a rail jump. Bezel's
/// `FollowState` has no unpin: a click that leaves the end while content is
/// still growing is treated as "keep following" and snaps back.
#[derive(Clone)]
struct Follow(Rc<Cell<(bool, Pixels)>>);

impl Default for Follow {
    fn default() -> Self {
        Self(Rc::new(Cell::new((true, px(0.0)))))
    }
}

impl Follow {
    fn unpin(&self) {
        let (_, last) = self.0.get();
        self.0.set((false, last));
    }
}

/// Where a session's scrollback sits and which of its zones are open — view
/// state, per session, so switching back finds the transcript as it was left.
#[derive(Default)]
pub struct State {
    scroll: ScrollHandle,
    follow: Follow,
    /// Keyed by the turn's first item index.
    work: HashMap<usize, Takeover>,
    /// A thought the person folded or unfolded by hand, and which way.
    /// Absent means the default: open while it streams, folded once done.
    thoughts: HashMap<usize, bool>,
    /// Tool items whose output is showing, by item index.
    output: HashSet<usize>,
    /// Low-signal groups whose member names are showing, keyed by the first
    /// item index in the group.
    groups: HashSet<usize>,
    /// Which item's text is selected and what of it. One at a time — a press
    /// in another item is what clears the last, the same way a page of prose
    /// has one selection however many paragraphs it holds.
    selection: Option<(usize, Selection)>,
    /// Whether the pointer is down and dragging the selection's head about.
    dragging: bool,
    /// What each item painted, so a press can be resolved against what is on
    /// screen rather than against the source.
    ///
    /// Behind a cell because the transcript is drawn from `&ChatSession`: the
    /// layouts are refilled by the renderer every frame, and an item drawn for
    /// the first time has to be able to put its own in.
    layouts: RefCell<HashMap<usize, BlockLayouts>>,
    /// Card bodies that scroll on their own, by item index: the handle and
    /// where it was last seen, which is what tells the wheel whether the box
    /// moved or the page should.
    boxes: RefCell<HashMap<usize, ScrollBox>>,
    /// Each prose item's parsed markdown, kept while its text stays the same.
    docs: Memo<Doc>,
    /// Each open tool output's syntax spans, likewise.
    spans: Memo<Spans>,
    /// The answer streaming in rises into place one painted line at a time,
    /// and this is its clock, by item index. Kept while the lines settle and
    /// dropped after, so a settled item paints plainly. A transcript read
    /// back from disk starts none: what was already said shows at rest.
    reveals: RefCell<HashMap<usize, Rc<Reveal>>>,
    /// When each turn's copy control last flashed a check, keyed by the
    /// turn's first item index.
    copy_flash: HashMap<usize, Instant>,
}

/// What the highlighter says about one text: `None` when it has no grammar
/// for the language.
type Spans = Option<Vec<(Range<usize>, HighlightKind)>>;

/// What an item's text was last made into, held until that text changes.
///
/// The transcript is rebuilt on every frame, and a scroll is a frame every
/// few milliseconds. Parsing every message again on each one is what made a
/// long chat crawl; this is the one place that work is remembered. Keyed by
/// item index and checked against the source text, so the message still
/// streaming in is re-made as it grows and everything settled is not.
///
/// A cell, like `layouts`, because the renderer only has `&ChatSession`.
struct Memo<T>(RefCell<HashMap<usize, (String, Rc<T>)>>);

impl<T> Default for Memo<T> {
    fn default() -> Self {
        Self(RefCell::new(HashMap::new()))
    }
}

impl<T> Memo<T> {
    /// The value for item `ix` made from `text`, built only when `text` is
    /// not what it was built from last time.
    fn get(&self, ix: usize, text: &str, build: impl FnOnce(&str) -> T) -> Rc<T> {
        let mut map = self.0.borrow_mut();
        if let Some((source, value)) = map.get(&ix)
            && source == text
        {
            return value.clone();
        }
        let value = Rc::new(build(text));
        map.insert(ix, (text.to_owned(), value.clone()));
        value
    }

    /// Whether item `ix` has been built at all.
    fn has(&self, ix: usize) -> bool {
        self.0.borrow().contains_key(&ix)
    }
}

impl State {
    /// The parsed markdown of item `ix`, whose prose is `text`.
    fn doc(&self, ix: usize, text: &str) -> Rc<Doc> {
        // Links go in as chips — `⛓ #139`, `⚙ chat doors`, `📄 notes` — the
        // targets as written, so a click still opens the same thing.
        self.docs.get(ix, text, |text| {
            markdown::parse(&crate::view::chips::dress(text))
        })
    }

    /// The reveal for item `ix`: started while the item is `live` — the
    /// answer streaming in — and kept only until its lines have landed.
    fn reveal(&self, ix: usize, live: bool) -> Option<Rc<Reveal>> {
        let mut reveals = self.reveals.borrow_mut();
        if live {
            return Some(
                reveals
                    .entry(ix)
                    .or_insert_with(|| {
                        // An item this state has already drawn was on screen
                        // at rest — after switching back to a chat mid-turn,
                        // say. What is there stays put; only what arrives
                        // from here rises.
                        let reveal = Reveal::default();
                        if self.docs.has(ix) {
                            reveal.already_read();
                        }
                        Rc::new(reveal)
                    })
                    .clone(),
            );
        }
        match reveals.get(&ix) {
            Some(reveal) if !reveal.is_settled() => Some(reveal.clone()),
            Some(_) => {
                reveals.remove(&ix);
                None
            }
            None => None,
        }
    }

    /// Cursor: a thought streams open, then folds to `Thought 10s`. A
    /// click flips whichever state is on screen, and that choice sticks —
    /// one folded by hand while streaming stays folded when it settles.
    fn thought_open(&self, ix: usize, done: bool) -> bool {
        self.thoughts.get(&ix).copied().unwrap_or(!done)
    }

    fn toggle_thought(&mut self, ix: usize, done: bool) {
        let open = self.thought_open(ix, done);
        self.thoughts.insert(ix, !open);
    }

    fn toggle_group(&mut self, start: usize) {
        if !self.groups.insert(start) {
            self.groups.remove(&start);
        }
    }

    /// The layout store for item `ix`, made on the first frame it is drawn.
    fn layouts(&self, ix: usize) -> BlockLayouts {
        self.layouts.borrow_mut().entry(ix).or_default().clone()
    }

    /// The scroll state of item `ix`'s card body, made when first drawn.
    fn scroll_box(&self, ix: usize) -> ScrollBox {
        self.boxes.borrow_mut().entry(ix).or_default().clone()
    }

    /// What `ix` has selected, if it is the item holding the selection.
    fn selection(&self, ix: usize) -> Option<Selection> {
        self.selection
            .filter(|(item, _)| *item == ix)
            .map(|(_, selection)| selection)
    }

    /// Answer the pointer over item `ix`. A press starts a selection there and
    /// drops whatever another item held; a move drags its head.
    ///
    /// The first release after a press, on a link, returns the URL to open —
    /// including a click that jittered but stayed on that one link. A later
    /// Up does not: `bezel-markdown` reports release twice (on the text and
    /// off it), and both would otherwise open a tab.
    pub fn point(&mut self, ix: usize, text: &str, pointer: Pointer) -> Option<String> {
        match pointer {
            Pointer::Down(cursor) => {
                self.selection = Some((ix, Selection::at(cursor)));
                self.dragging = true;
                None
            }
            // Double-click: the whole message, and the gesture is over — the
            // release that follows must neither shrink it nor open a link.
            Pointer::SelectAll => {
                self.selection = Some((ix, Selection::all(&self.doc(ix, text))));
                self.dragging = false;
                None
            }
            Pointer::Move(cursor) => {
                if !self.dragging {
                    return None;
                }
                if let Some((item, selection)) = self.selection.filter(|(item, _)| *item == ix) {
                    self.selection = Some((item, selection.extend_to(cursor)));
                }
                None
            }
            Pointer::Up => {
                if !self.dragging {
                    return None;
                }
                self.dragging = false;
                let (_, selection) = self.selection.filter(|(item, _)| *item == ix)?;
                click_url(&self.doc(ix, text), selection)
            }
        }
    }

    /// What is selected, as it would be pasted, or nothing when a press
    /// collapsed without a drag behind it.
    pub fn copied(&self, chat: &ChatSession) -> Option<String> {
        let (ix, selection) = self.selection?;
        let item = chat.items.get(ix)?;
        // The prompt card is set from the escaped text; copy from the same
        // document, so the offsets line up (the escapes do not paste).
        let shown: std::borrow::Cow<str> = match item {
            ChatItem::User(message) => plain_markdown(&message.text).into(),
            other => item_text(other)?.into(),
        };
        let doc = self.doc(ix, &shown);
        let text = selectable::copied(&doc, selection);
        (!text.is_empty()).then_some(text)
    }
}

/// The URL a click should open: a caret on a link, or both ends of a
/// selection sitting on the same link (a press that jittered).
fn click_url(doc: &Doc, selection: Selection) -> Option<String> {
    let (url, head_range) = link_at(doc, selection.head)?;
    if selection.is_collapsed() {
        return Some(url);
    }
    let (other, anchor_range) = link_at(doc, selection.anchor)?;
    if url != other
        || selection.head.block != selection.anchor.block
        || selection.head.part != selection.anchor.part
        || head_range != anchor_range
    {
        return None;
    }
    Some(url)
}

fn link_at(doc: &Doc, cursor: Cursor) -> Option<(String, Range<usize>)> {
    let text = doc.blocks.get(cursor.block)?.text_at(cursor.part)?;
    let offset = cursor.offset;
    text.marks.iter().rev().find_map(|span| {
        if offset < span.range.start || offset > span.range.end {
            return None;
        }
        match &span.mark {
            Mark::Link(url) | Mark::Mention { url, .. } | Mark::Image(url) => {
                openable(url).map(|url| (url, span.range.clone()))
            }
            _ => None,
        }
    })
}

fn openable(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    if url.starts_with("arbos://") || markdown::is_url(url) || url.starts_with("mailto:") {
        return Some(url.to_string());
    }
    if let Some(rest) = url.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    if url.contains('.') && !url.contains(' ') && !url.starts_with('#') && !url.starts_with('/') {
        return Some(format!("https://{url}"));
    }
    None
}

/// The prose of an item, for the two kinds that carry any.
fn item_text(item: &ChatItem) -> Option<&str> {
    match item {
        ChatItem::User(message) => Some(&message.text),
        ChatItem::Agent(text) | ChatItem::From { text, .. } => Some(text),
        ChatItem::Thinking { text, .. } => Some(text),
        _ => None,
    }
}

/// A question and the answer it drew.
struct Turn {
    range: Range<usize>,
    /// Where the interim half ends and the reply begins.
    answer_from: usize,
}

/// Start a turn at every question. The leading chunk of a session has none —
/// a connection that failed before the first prompt is still something to show.
fn turns(items: &[ChatItem]) -> Vec<Turn> {
    let mut turns = Vec::new();
    let mut start = 0;
    for ix in 1..=items.len() {
        // A worker's report right after the wake it caused is that
        // segment's first line, not a boundary of its own.
        let report_in_segment = ix < items.len()
            && matches!(items[ix], ChatItem::From { .. })
            && matches!(items[ix - 1], ChatItem::Wake { .. });
        if ix < items.len()
            && (!matches!(
                items[ix],
                ChatItem::User(_) | ChatItem::From { .. } | ChatItem::Wake { .. }
            ) || inline_user(items, ix)
                || report_in_segment)
        {
            continue;
        }
        let interim =
            |item: &ChatItem| matches!(item, ChatItem::Tool { .. } | ChatItem::Thinking { .. });
        let answer_from = items[start..ix]
            .iter()
            .rposition(interim)
            .map_or(start, |last| start + last + 1);
        turns.push(Turn {
            range: start..ix,
            answer_from: answer_from.max(start + 1),
        });
        start = ix;
    }
    turns
}

/// The assistant reply that belongs to this step — Agent prose after the
/// tools and thoughts, not the chat name and not earlier turns.
/// The answer text of the turn whose prompt sits at `first`, for a vote's
/// record. Empty when the turn has no answer yet.
pub fn answer_of(items: &[ChatItem], first: usize) -> Option<String> {
    let turn = turns(items).into_iter().find(|t| t.range.start == first)?;
    turn_answer(items, &turn)
}

fn turn_answer(items: &[ChatItem], turn: &Turn) -> Option<String> {
    let parts: Vec<&str> = (turn.answer_from..turn.range.end)
        .filter_map(|ix| match items.get(ix) {
            Some(ChatItem::Agent(text)) if !text.is_empty() => Some(text.as_str()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("\n\n"))
}

/// The kernel's "project page not updated" reminder (a `nudge` event, or
/// the notice older kernels wrote): one dim line with a ↻ glyph, the
/// reason only — the instruction half is the agent's to act on, not the
/// reader's. No strip, no retry.
fn page_nudge(text: &str, theme: &Theme) -> AnyElement {
    // "project page not updated last turn: a worker was started or
    // reported and .arbos/notes.md did not change — update it" is the
    // kernel's whole sentence; the reader needs the first clause, as a
    // sentence of its own.
    let clause = text
        .split(" — ")
        .next()
        .unwrap_or(text)
        .split(':')
        .next()
        .unwrap_or(text)
        .trim();
    let mut shown = String::with_capacity(clause.len());
    let mut chars = clause.chars();
    if let Some(first) = chars.next() {
        shown.extend(first.to_uppercase());
        shown.push_str(chars.as_str());
    }
    let shown = if shown.starts_with("Project page not updated") {
        "Project page not updated this turn".to_string()
    } else {
        shown
    };
    div()
        .self_start()
        .w_full()
        .max_w(px(root::CHAT_MAX_WIDTH))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .child(
            icons::icon(icons::media::REPEAT)
                .size(px(11.))
                .text_color(theme.text_faint),
        )
        .child(
            div()
                .text_style(TextStyle::Caption)
                .text_color(theme.text_faint)
                .child(SharedString::from(shown)),
        )
        .into_any_element()
}

/// What the session has to say for itself. Cursor keeps failures as a
/// muted line, not a full-width danger banner. ChatView ErrorCard puts
/// Retry on a failed turn — same resend of the last user prompt.
fn notice(
    chat: &ChatSession,
    ix: usize,
    text: &str,
    failed: bool,
    theme: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    if !failed && text == "done" {
        return div().into_any_element();
    }
    if !failed && crate::model::session::is_page_nudge(text) {
        return page_nudge(text, theme);
    }
    // The kernel turned an answer down ("answer refused: no question is
    // pending"): that belongs to the question's card, as one faint line in
    // its shape, not a free line with Retry.
    if let Some(reason) = text.strip_prefix("answer refused:") {
        return asked_line("answer", &format!("refused — {}", reason.trim()), theme);
    }
    // A failure's raw wording — a provider's refusal, two model ids, a
    // documentation link — is never the line (F-76); Cursor shows a short
    // sentence and keeps the detail behind a disclosure. The same for the
    // kernel's "switched to <model> for this turn: <the whole reason>",
    // Jacob's first line on a new project. The disclosure is there
    // whenever the short line dropped something.
    let shown = if failed {
        short_error(text)
    } else {
        short_notice(text)
    };
    let detail = (shown.trim() != text.trim()).then(|| text.trim().to_string());
    let open = detail.is_some() && chat.transcript.groups.contains(&ix);
    let retry = failed.then(|| last_user_prompt(&chat.items)).flatten();
    let can_retry = retry.is_some() && !chat.busy();
    let id = chat.id;
    let row = div()
        .self_start()
        .w_full()
        .max_w(px(root::CHAT_MAX_WIDTH))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .text_style(TextStyle::Callout)
        .text_color(theme.text_muted)
        // In a flex box the words need a shrinkable cell to wrap in; without
        // it a long notice ("mode: ask — …") ran off the right edge (Mac
        // cycle 11).
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(spaced_label(shown, theme.text_muted, theme)),
        )
        .when(detail.is_some(), |row| {
            row.child(
                div()
                    .id(SharedString::from(format!("notice-details-{id}-{ix}")))
                    .flex_none()
                    .cursor_pointer()
                    .rounded(px(4.))
                    .px(px(6.))
                    .py(px(2.))
                    .hover(|el| el.bg(theme.element_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.with_session(id, cx, |chat| chat.transcript.toggle_group(ix));
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(3.))
                            .text_style(TextStyle::Caption)
                            .text_color(theme.text_faint)
                            .child("Details")
                            .child(
                                icons::icon(if open {
                                    icons::arrows::ALT_ARROW_DOWN
                                } else {
                                    icons::arrows::ALT_ARROW_RIGHT
                                })
                                .size(px(10.))
                                .text_color(theme.text_faint),
                            ),
                    ),
            )
        })
        .when_some(retry, |row, prompt| {
            row.child(
                div()
                    .id(SharedString::from(format!("retry-notice-{id}-{ix}")))
                    .flex_none()
                    .cursor_pointer()
                    .rounded(px(4.))
                    .px(px(6.))
                    .py(px(2.))
                    .when(can_retry, |el| {
                        el.hover(|el| el.bg(theme.element_hover))
                            .active(|el| el.bg(theme.element_active))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(id, prompt.clone(), cx);
                            }))
                    })
                    .opacity(if can_retry { 1. } else { 0.5 })
                    .child(
                        div()
                            .text_style(TextStyle::Caption)
                            .text_color(theme.text_faint)
                            .child("Retry"),
                    ),
            )
        });
    match detail.filter(|_| open) {
        Some(detail) => div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(row)
            .child(
                div()
                    .id(SharedString::from(format!("notice-detail-{id}-{ix}")))
                    .w_full()
                    .max_w(px(root::CHAT_MAX_WIDTH))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .bg(theme.surface_raised)
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child(SharedString::from(detail)),
            )
            .into_any_element(),
        None => row.into_any_element(),
    }
}

fn last_user_prompt(items: &[ChatItem]) -> Option<Prompt> {
    items.iter().rev().find_map(|item| match item {
        ChatItem::User(message) => {
            let prompt = message.to_prompt();
            (!prompt.is_empty()).then_some(prompt)
        }
        _ => None,
    })
}

/// Cursor errors are one short line, not the kernel's full wrap.
/// A notice that is not a failure, cut to its news: "switched to
/// <model> for this turn" without the provider's paragraph after the colon;
/// anything else that runs long, to its first sentence. The whole text
/// stays behind the Details disclosure.
fn short_notice(text: &str) -> String {
    let text = text.trim();
    // "google/gemini-2.5-flash: connection failed: error sending request
    // for url (…) — retrying in 2.4s (attempt 4/5)": the attempt is the
    // news; the URL and the model id are the detail.
    if crate::model::session::is_retry_line(text) {
        let attempt = text
            .split("(attempt ")
            .nth(1)
            .and_then(|rest| rest.split(')').next())
            .unwrap_or("?");
        let what = if text.contains("connection failed") {
            "Connection failed"
        } else {
            "The model did not answer"
        };
        return format!("{what}; retrying (attempt {attempt}).");
    }
    if let Some(rest) = text.strip_prefix("switched to ")
        && let Some((model, _)) = rest.split_once(" for this turn")
    {
        let model = model.rsplit('/').next().unwrap_or(model);
        return format!("Switched to {model} for this turn.");
    }
    if text.chars().count() > 160 {
        let first = text
            .split_inclusive(['.', ':'])
            .next()
            .unwrap_or(text)
            .trim_end_matches(':')
            .trim();
        return shorten(first, 100);
    }
    text.to_owned()
}

fn short_error(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    // A provider turning the request down — a policy block, a bad key, a
    // quota, a rate limit: the model did not answer, and the reason is a
    // sentence, not a paragraph with model ids and a documentation link
    // (Jacob's first line on a new project, F-76).
    if lower.contains("policy violation")
        || lower.contains("has been blocked")
        || lower.contains("content_policy")
    {
        return "The model provider refused the request.".into();
    }
    if lower.contains("no api key") || lower.contains("invalid api key") || lower.contains("401") {
        return "No working model key on this kernel.".into();
    }
    if lower.contains("rate limit") || lower.contains("429") || lower.contains("quota") {
        return "The model provider is rate-limiting requests.".into();
    }
    if lower.contains("cut off") || lower.contains("mid-stream") {
        return "Answer was cut off. Send the message again.".into();
    }
    if lower.contains("connection failed") {
        return "Connection failed.".into();
    }
    if lower.contains("attach writer closed") {
        return "Stopped.".into();
    }
    // The kernel's rewind refusal names the agent id and a version note;
    // the reader needs only what to do.
    if lower.starts_with("rewind:") && lower.contains("no checkpoints yet") {
        return "Nothing to rewind to yet: checkpoints are written when a turn starts.".into();
    }
    if lower.starts_with("rewind:") && lower.contains("checkpoint") {
        return "Rewind failed: no checkpoint for that turn.".into();
    }
    // The kernel's job line: "job j9 exited with code 1 after 1s — `pm2
    // status` — log: …". Cursor names the command and the code, no
    // backticks and no log path.
    if text.starts_with("job ") && text.contains("exited with code") {
        let code = text
            .split("exited with code ")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or("?");
        let command = text
            .split('`')
            .nth(1)
            .map(|cmd| shorten(cmd.trim(), 60))
            .unwrap_or_else(|| "command".into());
        return format!("{command} exited with code {code}");
    }
    let text = text
        .strip_prefix("turn failed: ")
        .or_else(|| text.strip_prefix("Internal error — "))
        .unwrap_or(text);
    let text = text
        .rsplit_once(" — ")
        .map(|(_, rest)| rest)
        .unwrap_or(text);
    shorten(text.trim(), 80)
}

fn message_images(images: &[MessageImage]) -> impl Iterator<Item = AnyElement> + '_ {
    images.iter().map(|image| {
        let (width, height) = image.display_size();
        match image.preview() {
            Some(preview) => img(preview)
                .w(px(width))
                .max_w_full()
                .h(px(height))
                .rounded(px(Theme::control_radius()))
                .into_any_element(),
            None => div().child("Image unavailable").into_any_element(),
        }
    })
}

/// Widest an artifact card grows. Two fit side by side in the column.
const ARTIFACT_W: f32 = 296.;
const ARTIFACT_H: f32 = 180.;

/// Files a tool made for the user — screenshots, clips — as cards in a
/// wrapping row: the picture (a clip shows its last frame with a play
/// badge), the file name, and the tool's measurements. Click opens the
/// file with the system's viewer.
fn artifacts_row(
    chat: &ChatSession,
    ix: usize,
    files: &[Artifact],
    theme: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let id = chat.id;
    div()
        .id(SharedString::from(format!("artifacts-{id}-{ix}")))
        .w_full()
        .max_w(px(root::CHAT_MAX_WIDTH))
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(8.))
        .children(
            files
                .iter()
                .enumerate()
                .map(|(n, file)| artifact_card(id, ix, n, file, theme, cx)),
        )
        .into_any_element()
}

fn artifact_card(
    id: u64,
    ix: usize,
    n: usize,
    file: &Artifact,
    theme: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let path = file.path.clone();
    let preview = file.thumb.as_ref().and_then(MessageImage::preview);
    let (w, h) = file
        .thumb
        .as_ref()
        .map(|thumb| {
            let (tw, th) = thumb.display_size();
            let scale = (ARTIFACT_W / tw.max(1.))
                .min(ARTIFACT_H / th.max(1.))
                .min(1.);
            (tw * scale, th * scale)
        })
        .unwrap_or((ARTIFACT_W, 72.));
    let is_clip = file.kind == ArtifactKind::Video;
    let picture = div()
        .relative()
        .w(px(w))
        .h(px(h))
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.surface_raised)
        .rounded_t(px(Theme::control_radius()))
        .overflow_hidden()
        .child(match preview {
            Some(preview) => img(preview).w(px(w)).h(px(h)).into_any_element(),
            None => div()
                .text_style(TextStyle::Caption)
                .text_color(theme.text_faint)
                .child(if is_clip { "Recording" } else { "Image" })
                .into_any_element(),
        })
        .when(is_clip, |el| {
            el.child(
                div()
                    .absolute()
                    .left(px(w / 2. - 16.))
                    .top(px(h / 2. - 16.))
                    .size(px(32.))
                    .rounded_full()
                    .bg(rgb(0x000000).opacity(0.55))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        icons::icon(icons::media::PLAY_BOLD)
                            .size(px(14.))
                            .text_color(rgb(0xffffff)),
                    ),
            )
        });
    let caption = div()
        .w_full()
        .px(px(8.))
        .py(px(5.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .min_w(px(0.))
                .flex_1()
                .font_family(theme.font_mono.clone())
                .text_size(px(MONO_SIZE))
                .line_height(px(MONO_LEAD))
                .text_color(theme.text_muted)
                .truncate()
                .child(SharedString::from(file.name.clone())),
        )
        .when(!file.caption.is_empty(), |row| {
            row.child(
                div()
                    .flex_none()
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child(SharedString::from(file.caption.clone())),
            )
        });
    div()
        .id(SharedString::from(format!("artifact-{id}-{ix}-{n}")))
        .flex()
        .flex_col()
        .w(px(w.max(160.)))
        .border_1()
        .border_color(theme.border)
        .rounded(px(Theme::control_radius()))
        .overflow_hidden()
        .cursor_pointer()
        .hover(|el| el.bg(theme.element_hover))
        .child(picture)
        .child(caption)
        .on_click(cx.listener(move |_, _, _, _| open_external(&path)))
        .into_any_element()
}

/// Hand a file to the system viewer. Fire and forget: a missing viewer
/// shows the OS's own message, not ours.
fn open_external(path: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Web prompt-card: the plate bleeds by its own padding so the words
/// share the left edge with the answer. `-mx-3 px-3 py-2.5`.
fn user_prompt(
    chat: &ChatSession,
    ix: usize,
    message: &UserMessage,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let id = chat.id;
    let prompt = message.to_prompt();
    let (slash, body) = split_slash(&message.text);
    let group = SharedString::from(format!("prompt-{id}-{ix}"));
    // Cursor's user bubble: a card at the column's right edge, flush with
    // the composer's, no wider than most of the column. The edit pencil
    // sits outside the card, to its left, and shows on hover — inside, it
    // widened the card and moved the text's right edge (Jacob's still).
    let card = div()
        .id(group.clone())
        .max_w(px(PROMPT_MAX_WIDTH))
        .px(px(PROMPT_PAD_X))
        .py(px(PROMPT_PAD_Y))
        .rounded(px(PROMPT_RADIUS))
        // Cursor's prompt card: one step up from the page (#212121 on its
        // #161514), no border, no shadow.
        .bg(theme.ink(0.06))
        .flex()
        .flex_row()
        .items_end()
        .gap(px(ROW_GAP))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.))
                .when(message.has_attachments(), |el| {
                    el.child(user_attachments(message, theme))
                })
                .when(!message.described.is_empty(), |el| {
                    el.child(described_note(message, theme))
                })
                .when_some(slash, |el, cmd| {
                    el.child(
                        div()
                            .text_style(TextStyle::Body)
                            .text_color(theme.text_muted)
                            .child(SharedString::from(cmd)),
                    )
                })
                .when(!body.is_empty(), |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap(px(6.))
                            // Spoken on a call: a small microphone leads the line.
                            .when(message.channel == "voice", |row| {
                                row.child(
                                    div()
                                        .id(SharedString::from(format!("prompt-voice-{id}-{ix}")))
                                        .flex_none()
                                        .mt(px(3.))
                                        .tooltip(|window, cx| {
                                            Tooltip::text("Spoken on a call", window, cx)
                                        })
                                        .child(
                                            icons::icon(icons::media::MICROPHONE)
                                                .size(px(12.))
                                                .text_color(theme.text_muted),
                                        ),
                                )
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .text_style(TextStyle::Body)
                                    .text_color(theme.text)
                                    // Cursor shows the prompt as typed —
                                    // backticks and stars stay characters,
                                    // nothing is set as code or bold.
                                    .child(prose(chat, ix, &plain_markdown(body), window, cx)),
                            ),
                    )
                }),
        );
    let pencil = div()
        .id(SharedString::from(format!("replay-prompt-{id}-{ix}")))
        .flex_none()
        .size(px(24.))
        .mb(px(4.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        // Hidden until the pointer is over the row, then faint beside the
        // card's bottom-left corner; the card's own width never changes.
        .invisible()
        .group_hover(group.clone(), |el| el.visible())
        .hover(|el| el.bg(theme.element_hover))
        .tooltip(move |window, cx| Tooltip::text("Edit and resubmit from here", window, cx))
        .on_click(cx.listener(move |this, _, _, cx| {
            if prompt.is_empty() {
                return;
            }
            this.edit_in_composer(prompt.text.clone(), cx);
        }))
        .child(
            icons::icon(icons::editing::PEN)
                .size(px(12.))
                .text_color(theme.text_faint),
        );
    // A row with the card at its end: the card takes its content's width
    // up to PROMPT_MAX_WIDTH and its right edge is the composer's — the
    // composer's plate bleeds past the column gutter by its own pad.
    div()
        .group(group)
        .w_full()
        .relative()
        .left(px(root::COMPOSER_PAD_X))
        .flex()
        .flex_row()
        .justify_end()
        .items_end()
        .gap(px(6.))
        .child(pencil)
        .child(card.flex_none())
        .into_any_element()
}

fn split_slash(text: &str) -> (Option<String>, &str) {
    let trim = text.trim_start();
    if !trim.starts_with('/') {
        return (None, text);
    }
    let cmd = trim.split_whitespace().next().unwrap_or("").to_owned();
    let rest = trim.get(cmd.len()..).unwrap_or("").trim_start();
    (Some(cmd), rest)
}

/// Composer chips for a sent message: files and images above the text,
/// same card as the tray, without the ✕.
fn user_attachments(message: &UserMessage, theme: &Theme) -> AnyElement {
    if !message.has_attachments() {
        return div().into_any_element();
    }
    attachment::row()
        .children(message.files.iter().enumerate().map(|(ix, file)| {
            attachment::chip(("history-file", ix), file.name.clone(), None, theme)
                .into_any_element()
        }))
        .children(message.images.iter().enumerate().map(|(ix, image)| {
            attachment::chip(
                ("history-image", ix),
                image.label().to_string(),
                image.preview(),
                theme,
            )
            .into_any_element()
        }))
        .into_any_element()
}

/// The turn's model could not see the attached image(s); another model
/// put them into words. A paperclip and a dim caption inside the card —
/// hover for which model and what it said. Never a line of the transcript.
fn described_note(message: &UserMessage, theme: &Theme) -> AnyElement {
    let n = message.described.len();
    let model = message
        .described
        .first()
        .map(|d| d.model.clone())
        .unwrap_or_default();
    let caption: SharedString = if n == 1 {
        format!("image described by {model}").into()
    } else {
        format!("{n} images described by {model}").into()
    };
    let tip: SharedString = message
        .described
        .iter()
        .map(|d| {
            let name = std::path::Path::new(&d.path)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(d.path.as_str());
            format!(
                "{name} — described by {}:\n{}",
                d.model,
                shorten(d.text.trim(), 400)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
        .into();
    div()
        .id("described-images")
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .text_style(TextStyle::Caption)
        .text_color(theme.text_faint)
        .cursor_default()
        .tooltip(move |window, cx| Tooltip::text(tip.clone(), window, cx))
        .child(
            icons::icon(icons::files::PAPERCLIP)
                .size(px(11.))
                .text_color(theme.text_faint)
                .into_any_element(),
        )
        .child(caption)
        .into_any_element()
}

/// A message from outside this window. The name is the whole signal: this
/// is not you, and not the agent answering you.
fn from_block(
    chat: &ChatSession,
    ix: usize,
    who: &str,
    text: &str,
    images: &[MessageImage],
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    if let Some((ok, words)) = done_report(text) {
        return worker_card(chat, ix, who, ok, &words, theme, window, cx);
    }
    div()
        .self_start()
        .max_w(px(520.))
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(
            div()
                .text_style(TextStyle::Caption)
                .text_color(theme.text_faint)
                // A sub-agent's title, not its folder id.
                .child(SharedString::from(chat.who_label(who))),
        )
        .child(
            div()
                .text_style(TextStyle::Body)
                .text_color(theme.text)
                .child(prose(chat, ix, text, window, cx)),
        )
        .children(message_images(images))
        .into_any_element()
}

/// The kernel's done file for a worker, as it lands in the parent's chat:
/// `Turn ended. Last words: … (transcript: …)`, or `Turn ended badly.
/// …`. The verdict and the words, without the file pointer.
fn done_report(text: &str) -> Option<(bool, String)> {
    let text = text.trim();
    let (ok, rest) = if let Some(rest) = text.strip_prefix("Turn ended. Last words:") {
        (true, rest)
    } else if let Some(rest) = text.strip_prefix("Turn ended badly. Last words:") {
        (false, rest)
    } else {
        return None;
    };
    let mut words = rest.trim().to_string();
    if let Some(at) = words.rfind("(transcript:") {
        words.truncate(at);
    }
    Some((ok, words.trim().trim_end_matches('…').trim().to_string()))
}

/// A worker's completion, as Cursor draws it: the worker's last words as
/// one dim line in the flow — no box, no header — that opens the worker.
fn worker_card(
    chat: &ChatSession,
    ix: usize,
    who: &str,
    ok: bool,
    words: &str,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let child = chat
        .children
        .iter()
        .find(|c| c.kernel_id.as_deref() == Some(who))
        .map(|c| c.id);
    // Cursor's line for a finished sub-agent: its chip, "done —", its
    // last words. The chip is a link to the worker's chat (`agents/<id>`
    // dresses as an agent chip), so the name is the way in.
    let name = chat.who_label(who);
    let head = format!(
        "[{name}](agents/{who}) {}",
        if ok { "done" } else { "ended badly" }
    );
    let words = if words.is_empty() {
        head
    } else {
        format!("{head} — {words}")
    };
    div()
        .id(("worker-line", chat.id * 100_000 + ix as u64))
        .self_start()
        .w_full()
        .max_w(px(root::CHAT_MAX_WIDTH))
        .text_style(TextStyle::Callout)
        .text_color(if ok { theme.text_muted } else { theme.danger })
        .when(child.is_some(), |el| el.cursor_pointer())
        .child(prose(chat, ix, &words, window, cx))
        .when_some(child, |el, id| {
            el.on_mouse_down(
                bezel::gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.select_session(id, cx)
                }),
            )
        })
        .into_any_element()
}

/// A subscription the agent set up in this turn, as a small card under
/// the work: what stands now and when it fires. The Project panel keeps
/// the full list; this is the moment it was made.
fn subscription_cards(chat: &ChatSession, body: Range<usize>, theme: &Theme) -> Vec<AnyElement> {
    chat.items[body]
        .iter()
        .filter_map(|item| match item {
            ChatItem::Tool {
                label,
                status: ToolStatus::Success,
                output,
                ..
            } if label.starts_with("subscribe") && !label.starts_with("subscribe remove") => {
                let line = output
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or("")
                    .split(". ")
                    .next()
                    .unwrap_or("")
                    .trim_end_matches('.')
                    .to_string();
                (!line.is_empty() && line.starts_with("Subscribed")).then_some(line)
            }
            _ => None,
        })
        .map(|line| {
            div()
                .self_start()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .rounded(px(Theme::surface_radius()))
                .border_1()
                .border_color(theme.border)
                .bg(theme.surface_raised)
                .px(px(10.))
                .py(px(6.))
                .child(
                    icons::icon(icons::media::REPEAT)
                        .size(px(12.))
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .text_style(TextStyle::Caption)
                        .text_color(theme.text_faint)
                        .child("standing"),
                )
                .child(
                    div()
                        .text_style(TextStyle::Callout)
                        .text_color(theme.text)
                        .child(SharedString::from(line)),
                )
                .into_any_element()
        })
        .collect()
}

/// The kernel ids of the sub-agents a turn's body spawned — the `spawn`
/// calls in `body` and the child each one made.
fn spawned_in(items: &[ChatItem], body: Range<usize>) -> Vec<String> {
    items[body]
        .iter()
        .filter_map(|item| match item {
            ChatItem::Tool {
                child_session: Some(child),
                ..
            } => Some(child.clone()),
            _ => None,
        })
        .collect()
}

/// The parent's view of the sub-agents one turn spawned — Cursor's inline
/// status lines, one per worker, in the flow of the conversation: `1 Working
/// Reading project context and secrets inventory` while it runs, the step
/// text moving as the worker moves; `Asking …`, `Waiting …`; a finished
/// worker whose report is on the page below says nothing here, one without
/// a report reads `Done  <title>`. Dim, no box, no glyph; the press opens
/// the worker.
/// The mark after a worker that cannot write: it reads and reports, nothing
/// more. A project whose workers all carry it is a project that will
/// produce no code (F-56), and that should be visible while it runs.
pub(crate) fn readonly_mark(theme: &Theme, size: f32) -> AnyElement {
    div()
        .id("readonly-mark")
        .flex_none()
        .child(
            icons::icon(icons::system::MAGNIFER)
                .size(px(size))
                .text_color(theme.text_faint),
        )
        .tooltip(|window, cx| {
            Tooltip::text("read-only: reads and reports, writes nothing", window, cx)
        })
        .into_any_element()
}

/// The kind chip's word, or none: a plain writing worker carries no kind,
/// and a coordinator's line already reads as a sub-project.
pub(crate) fn kind_chip_text(kind: Option<&str>) -> Option<&str> {
    kind.filter(|k| !k.is_empty() && *k != "coordinator" && *k != "code" && *k != "default")
}

/// A low-contrast chip with the worker's kind, the model chip's weight.
pub(crate) fn kind_chip(kind: &str, theme: &Theme) -> AnyElement {
    div()
        .flex_none()
        .px(px(5.))
        .rounded(px(4.))
        .bg(theme.surface_raised)
        .text_style(TextStyle::Caption)
        .text_color(theme.text_faint)
        .child(SharedString::from(kind.to_string()))
        .into_any_element()
}

fn children_lines(
    chat: &ChatSession,
    spawned: &[String],
    theme: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    use crate::model::session::ChildState;
    let children: Vec<_> = chat
        .children
        .iter()
        .filter(|c| {
            c.kernel_id
                .as_deref()
                .is_some_and(|id| spawned.iter().any(|s| s == id))
        })
        .collect();
    let working = children
        .iter()
        .filter(|c| c.state == ChildState::Working)
        .count();
    let reported: Vec<&str> = chat
        .items
        .iter()
        .filter_map(|item| match item {
            ChatItem::From { who, text, .. } if done_report(text).is_some() => Some(who.as_str()),
            _ => None,
        })
        .collect();
    let mut first_working = true;
    let rows: Vec<AnyElement> = children
        .iter()
        .filter_map(|child| {
            let id = child.id;
            let (verb, rest, tone) = match child.state {
                ChildState::Working => {
                    let verb = if first_working && working > 1 {
                        format!("{working} Working")
                    } else if first_working {
                        "1 Working".to_string()
                    } else {
                        "Working".to_string()
                    };
                    first_working = false;
                    // One worker: "1 Working  <step>", as Jacob's Cursor
                    // still reads. Several: each line names its worker
                    // before the step, so three "Reading project context"
                    // lines are not one worker said thrice.
                    let rest = match (&child.step, working > 1) {
                        (Some(step), true) => format!("{} · {step}", child.title),
                        (Some(step), false) => step.clone(),
                        (None, _) => child.title.clone(),
                    };
                    (verb, rest, theme.text_muted)
                }
                ChildState::Asking => ("Asking".to_string(), child.title.clone(), theme.accent),
                ChildState::Waiting => {
                    ("Waiting".to_string(), child.title.clone(), theme.text_faint)
                }
                ChildState::Done => {
                    if child
                        .kernel_id
                        .as_deref()
                        .is_some_and(|k| reported.contains(&k))
                    {
                        return None;
                    }
                    ("Done".to_string(), child.title.clone(), theme.text_faint)
                }
            };
            Some(
                div()
                    .id(("child-line", id))
                    .self_start()
                    .max_w_full()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .gap(px(6.))
                    .py(px(2.))
                    .cursor_pointer()
                    .text_style(TextStyle::Body)
                    .text_size(px(root::CURSOR_PROSE_SIZE))
                    .child(
                        div()
                            .flex_none()
                            .text_color(tone)
                            .child(SharedString::from(verb)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.text_faint)
                            .child(SharedString::from(rest)),
                    )
                    .when(child.readonly, |el| el.child(readonly_mark(theme, 12.)))
                    .when_some(kind_chip_text(child.agent_kind.as_deref()), |el, kind| {
                        el.child(kind_chip(kind, theme))
                    })
                    // On the press, as a source list selects: while the turn
                    // streams, the transcript grows and scrolls between a
                    // press and its release.
                    .on_mouse_down(
                        bezel::gpui::MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.select_session(id, cx)
                        }),
                    )
                    .into_any_element(),
            )
        })
        .collect();
    // A worker the kernel has archived and this window never had a session
    // for (a transcript read back after a relaunch): still one line, "Done"
    // and its name, so the fan-out reads whole. Nothing to open.
    let known: Vec<&str> = children
        .iter()
        .filter_map(|c| c.kernel_id.as_deref())
        .collect();
    let gone: Vec<AnyElement> = spawned
        .iter()
        .filter(|id| !known.contains(&id.as_str()) && !reported.contains(&id.as_str()))
        .map(|id| {
            div()
                .self_start()
                .flex()
                .flex_row()
                .items_baseline()
                .gap(px(6.))
                .py(px(2.))
                .text_style(TextStyle::Body)
                .text_size(px(root::CURSOR_PROSE_SIZE))
                .text_color(theme.text_faint)
                .child("Done")
                .child(SharedString::from(sentence_case(
                    &crate::model::session::worker_name(id).unwrap_or_else(|| id.clone()),
                )))
                .into_any_element()
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .children(rows)
        .children(gone)
        .into_any_element()
}

/// One message, selectable. The transcript's two prose items — what you asked
/// and what came back — are the same element, because copying the one is the
/// same act as copying the other.
///
/// The session id rides in the closure rather than the item: a pointer event
/// arrives at the workspace, which holds every session, and the transcript on
/// screen is only one of them.
fn prose(
    chat: &ChatSession,
    ix: usize,
    text: &str,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let id = chat.id;
    // Only the answer still arriving rises. Everything before it — an earlier
    // reply, a peer's message, a transcript read back — is already read.
    let live = chat.streaming && live_answer(&chat.items) == Some(ix);
    let reveal = chat.transcript.reveal(ix, live);
    // A reading aid for words that came *to* you. Your own prompt is read
    // back as you typed it.
    let bionic = reading::bionic(cx) && !matches!(chat.items.get(ix), Some(ChatItem::User(_)));
    selectable::render(
        ("transcript-prose", ix),
        &chat.transcript.doc(ix, text),
        &chat.transcript.layouts(ix),
        chat.transcript.selection(ix),
        chat.transcript.dragging,
        reveal.as_deref(),
        bionic,
        window,
        cx,
        move |workspace, pointer, cx| {
            let mut url = None;
            workspace.with_session(id, cx, |chat| {
                let Some(item) = chat.items.get(ix) else {
                    return;
                };
                // The prompt card was set from the escaped text; the hit
                // test must read the same document.
                let shown: std::borrow::Cow<str> = match item {
                    ChatItem::User(message) => plain_markdown(&message.text).into(),
                    other => match item_text(other) {
                        Some(text) => text.into(),
                        None => return,
                    },
                };
                url = chat.transcript.point(ix, &shown, pointer);
            });
            // `arbos://` stays in the app; anything else is the browser's.
            match url {
                Some(url) if url.starts_with("arbos://") => {
                    workspace.open_chat_link(&url, cx);
                }
                Some(url) => cx.open_url(&url),
                None => {}
            }
        },
    )
}

/// The agent text a turn in flight is appending to: the last of its kind,
/// and after the prompt that started the turn. Before the answer's first
/// chunk lands, the last agent text is the previous answer, which is read.
fn live_answer(items: &[ChatItem]) -> Option<usize> {
    let answer = items
        .iter()
        .rposition(|item| matches!(item, ChatItem::Agent(_)))?;
    let prompt = items
        .iter()
        .rposition(|item| matches!(item, ChatItem::User(_)))?;
    (answer > prompt).then_some(answer)
}

/// Fence tag for a path in a tool title or its output — `acp.rs` is rust.
fn code_language(parts: &[&str]) -> Option<&'static str> {
    let mut found = None;
    for part in parts {
        for path in path_candidates(part) {
            if let Some(lang) = language_for_path(&path) {
                found = Some(lang);
            }
        }
    }
    found
}

fn path_candidates(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('`') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('`') else {
            break;
        };
        found.push(rest[..end].to_owned());
        rest = &rest[end + 1..];
    }
    for token in text.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'')) {
        let token =
            token.trim_matches(|c: char| matches!(c, ',' | ';' | ':' | ')' | '(' | '[' | ']'));
        if token.contains('.') {
            found.push(token.to_owned());
        }
    }
    found
}

fn language_for_path(path: &str) -> Option<&'static str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "ts" => Some("typescript"),
        "tsx" | "jsx" | "js" | "mjs" | "cjs" => Some("tsx"),
        "json" | "jsonc" => Some("json"),
        "go" => Some("go"),
        "html" | "htm" => Some("html"),
        "css" => Some("css"),
        "sh" | "bash" | "zsh" => Some("bash"),
        "toml" => Some("toml"),
        other => syntax::lang::resolve(other).map(|lang| lang.name),
    }
}

fn file_tool(kind: ToolKind) -> bool {
    matches!(
        kind,
        ToolKind::Edit | ToolKind::Read | ToolKind::Delete | ToolKind::Move
    )
}

/// Reads and searches are noise in a long turn — Cursor folds them. Writes,
/// shell, web, and anything that changes state stay first-class rows.
fn explore_tool(kind: ToolKind) -> bool {
    matches!(kind, ToolKind::Read | ToolKind::Search)
}

/// Kind used for grouping. History written before kinds were classified
/// still coalesces from the verb in the label (`read foo.rs`).
fn explore_kind(kind: ToolKind, label: &str) -> Option<ToolKind> {
    if explore_tool(kind) {
        return Some(kind);
    }
    if kind == ToolKind::Execute && shell_search(label) {
        return Some(ToolKind::Search);
    }
    match label.split_whitespace().next().unwrap_or(label) {
        "read" | "show" => Some(ToolKind::Read),
        "grep" | "find" | "ls" | "tgrep" | "glob" => Some(ToolKind::Search),
        "bash" | "run" | "exec" if shell_search(label) => Some(ToolKind::Search),
        _ => None,
    }
}

/// `grep` / `rg` via bash still count as Cursor searches, not commands.
fn shell_search(label: &str) -> bool {
    tool_rest(label)
        .split(|c: char| c.is_whitespace() || matches!(c, '|' | ';' | '&' | '(' | ')'))
        .any(|word| {
            matches!(
                word.trim_start_matches('-'),
                "grep" | "rg" | "tgrep" | "egrep" | "fgrep" | "find" | "ag" | "ack"
            )
        })
}

/// Reads and searches always stack. Same-file edits and same-command
/// shells stack too — Cursor's "Ran 4 commands". Distinct titles stay
/// their own line.
fn coalesce_kind(kind: ToolKind, label: &str) -> Option<ToolKind> {
    if let Some(kind) = explore_kind(kind, label) {
        return Some(kind);
    }
    match kind {
        ToolKind::Edit | ToolKind::Execute => Some(kind),
        _ => match label.split_whitespace().next().unwrap_or(label) {
            "edit" | "write" | "apply_patch" => Some(ToolKind::Edit),
            "bash" | "run" | "exec" => Some(ToolKind::Execute),
            _ => None,
        },
    }
}

/// Cursor's one-line tool titles: "Read foo.rs L40-69", "Ran Find cargo's…".
fn display_title(kind: ToolKind, label: &str, output: &str, running: bool) -> String {
    let leaf = tool_leaf(label);
    if let Some(title) = waited_title(label, output) {
        return title;
    }
    if let Some(query) = find_query(label) {
        let verb = if running { "Running" } else { "Ran" };
        return format!("{verb} Find {}", shorten(&query, 40));
    }
    match kind {
        ToolKind::Read => {
            let verb = if running { "Reading" } else { "Read" };
            match line_span(label).or_else(|| line_span(output)) {
                Some((a, b)) if a == b => format!("{verb} {leaf} L{a}"),
                Some((a, b)) => format!("{verb} {leaf} L{a}-{b}"),
                None => format!("{verb} {leaf}"),
            }
        }
        ToolKind::Edit => format!("{} {leaf}", if running { "Editing" } else { "Edited" }),
        ToolKind::Execute => {
            let cmd = clean_shell(&tool_rest(label));
            let verb = if running { "Running" } else { "Ran" };
            if cmd.is_empty() || is_job_log(&cmd) {
                format!("{verb} command")
            } else {
                format!("{verb} {}", shorten(&cmd, 72))
            }
        }
        ToolKind::Search => {
            let rest = search_rest(label);
            let first = label.split_whitespace().next();
            if first == Some("ls") {
                format!("Listed {rest}")
            } else if shell_search(label) || matches!(first, Some("grep" | "tgrep" | "rg")) {
                format!("Grepped {rest}")
            } else {
                format!("Searched {rest}")
            }
        }
        ToolKind::Fetch => format!("Fetched {leaf}"),
        ToolKind::Delete => format!("Deleted {leaf}"),
        _ => label.to_owned(),
    }
}

/// ChatView SummaryRow: muted verb + 11.5px mono argument.
fn display_parts(
    kind: ToolKind,
    label: &str,
    output: &str,
    running: bool,
) -> (String, Option<String>) {
    display_parts_for(kind, label, output, running, false)
}

/// A refused call never reads as an empty step: the row says `refused:`
/// and the error's first line; the fold body holds the whole error.
fn display_parts_for(
    kind: ToolKind,
    label: &str,
    output: &str,
    running: bool,
    failed: bool,
) -> (String, Option<String>) {
    if failed {
        let first = output
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("no result");
        let (verb, arg) = display_parts_for(kind, label, "", false, false);
        let mut detail = arg.unwrap_or_default();
        if !detail.is_empty() {
            detail.push_str(" · ");
        }
        detail.push_str("refused: ");
        detail.push_str(&shorten(first, 90));
        return (verb, Some(detail));
    }
    if waited_title(label, output).is_some() {
        return (display_title(kind, label, output, running), None);
    }
    if let Some(query) = find_query(label) {
        let verb = if running { "Running Find" } else { "Ran Find" };
        return (verb.to_owned(), Some(shorten(&query, 40)));
    }
    match kind {
        ToolKind::Read => {
            let leaf = tool_leaf(label);
            let verb = if running { "Reading" } else { "Read" };
            let arg = match line_span(label).or_else(|| line_span(output)) {
                Some((a, b)) if a == b => format!("{leaf} L{a}"),
                Some((a, b)) => format!("{leaf} L{a}-{b}"),
                None => leaf,
            };
            (verb.to_owned(), Some(arg).filter(|arg| !arg.is_empty()))
        }
        ToolKind::Search => {
            let rest = search_rest(label);
            let first = label.split_whitespace().next();
            if first == Some("ls") {
                let arg = if rest.is_empty() || rest == "ls" {
                    ".".to_owned()
                } else {
                    rest
                };
                return ("Listed".to_owned(), Some(arg));
            }
            let verb = if shell_search(label) || matches!(first, Some("grep" | "tgrep" | "rg")) {
                "Grepped"
            } else {
                "Searched"
            };
            (verb.to_owned(), Some(rest).filter(|arg| !arg.is_empty()))
        }
        _ => (display_title(kind, label, output, running), None),
    }
}

fn mono_label(text: impl AsRef<str>, color: Hsla, theme: &Theme) -> StyledText {
    let text = text.as_ref().to_string();
    let len = text.len();
    let styled = StyledText::new(SharedString::from(text));
    if len == 0 {
        styled
    } else {
        styled.with_runs(vec![TextRun {
            len,
            font: font(theme.font_mono.clone()),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }])
    }
}

/// Cursor: `Waited for Running \`path\` in shell` — await, or a
/// backgrounded bash that only reported the job.
fn waited_title(label: &str, output: &str) -> Option<String> {
    let first = label.split_whitespace().next().unwrap_or("");
    let target = if first == "await" {
        let rest = tool_rest(label);
        if rest.is_empty() {
            "command".to_owned()
        } else {
            rest
        }
    } else if output.contains("Running as job") {
        let cmd = clean_shell(&tool_rest(label));
        if cmd.is_empty() || is_job_log(&cmd) {
            return None;
        }
        cmd.rsplit(['/', ' '])
            .next()
            .unwrap_or(cmd.as_str())
            .to_owned()
    } else {
        return None;
    };
    Some(format!(
        "Waited for Running `{}` in shell",
        shorten(&target, 36)
    ))
}

/// `find **/*.pl` or a shell `find …` — Cursor says `Ran Find …`.
fn find_query(label: &str) -> Option<String> {
    let first = label.split_whitespace().next().unwrap_or("");
    if first == "find" {
        let rest = tool_rest(label);
        return Some(if rest.is_empty() {
            "files".to_owned()
        } else {
            rest
        });
    }
    let cmd = clean_shell(&tool_rest(label));
    let head = cmd.split_whitespace().next().unwrap_or("");
    if matches!(first, "bash" | "run" | "exec") && head == "find" {
        let rest = cmd.split_once(char::is_whitespace)?.1.trim();
        return Some(if rest.is_empty() {
            "files".to_owned()
        } else {
            rest.to_owned()
        });
    }
    None
}

fn tool_rest(label: &str) -> String {
    label
        .split_once(char::is_whitespace)
        .map(|(_, rest)| rest.trim())
        .filter(|rest| !rest.is_empty())
        .unwrap_or(label)
        .to_owned()
}

fn search_rest(label: &str) -> String {
    let rest = clean_shell(&tool_rest(label));
    let stripped = rest
        .split_once(char::is_whitespace)
        .and_then(|(verb, tail)| {
            matches!(
                verb,
                "grep" | "rg" | "tgrep" | "egrep" | "fgrep" | "find" | "ag" | "ack"
            )
            .then_some(tail.trim())
        })
        .unwrap_or(rest.as_str());
    // Cursor: "Grepped display_title( in transcript.rs"
    let title = match stripped.split_once(char::is_whitespace) {
        Some((pattern, path))
            if !path.is_empty() && !pattern.starts_with('-') && !path.starts_with('-') =>
        {
            format!("{pattern} in {}", tool_leaf(path))
        }
        _ => stripped.to_owned(),
    };
    shorten(&title, 56)
}

fn clean_shell(cmd: &str) -> String {
    let mut s = cmd.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s = s[1..s.len() - 1].trim();
    }
    s.trim_start_matches('|').trim().to_owned()
}

fn is_job_log(path: &str) -> bool {
    let leaf = path.rsplit(['/', '\\']).next().unwrap_or(path);
    leaf == "out.log" || leaf == "err.log" || path.contains("/jobs/")
}

fn shorten(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let short: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

/// `L40-69` in the label, or numbered lines in a read dump (`40|` / `40:`).
fn line_span(text: &str) -> Option<(u32, u32)> {
    if let Some(rest) = text.split(" L").nth(1) {
        let rest = rest.trim();
        if let Some((a, b)) = rest.split_once('-') {
            let a = a
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>();
            let b = b
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>();
            if let (Ok(a), Ok(b)) = (a.parse(), b.parse()) {
                return Some((a, b));
            }
        } else {
            let a = rest
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>();
            if let Ok(a) = a.parse() {
                return Some((a, a));
            }
        }
    }
    let mut first = None;
    let mut last = None;
    for line in text.lines() {
        let trim = line.trim_start();
        let digits = trim.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            continue;
        }
        let rest = &trim[digits..];
        if !rest.starts_with(['|', ':', ' ']) {
            continue;
        }
        let Ok(n) = trim[..digits].parse::<u32>() else {
            continue;
        };
        if first.is_none() {
            first = Some(n);
        }
        last = Some(n);
    }
    Some((first?, last.unwrap_or(first?)))
}

/// How many diff rows the open edit card shows. The TypeScript DiffCard
/// uses the same cap.
const DIFF_MAX_LINES: usize = 40;

#[derive(Clone, Copy, PartialEq)]
enum DiffKind {
    Add,
    Del,
    Ctx,
    Gap,
}

struct DiffRow {
    kind: DiffKind,
    num: Option<u32>,
    text: String,
}

/// Cursor's edit card: header `Edited name +N −M`, then an editor
/// body — line numbers, `+`/`−`, syntax, a wash only on the changed line.
fn diff_card(
    theme: &Theme,
    id: u64,
    ix: usize,
    label: &str,
    output: &str,
    running: bool,
    failed: bool,
    open: bool,
    scroll: ScrollBox,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let rows = parse_diff(output);
    let add = rows.iter().filter(|row| row.kind == DiffKind::Add).count();
    let del = rows.iter().filter(|row| row.kind == DiffKind::Del).count();
    let more = rows.len() > DIFF_MAX_LINES;
    let shown: Vec<DiffRow> = rows.into_iter().take(DIFF_MAX_LINES).collect();
    let name = tool_leaf(label);
    let title = format!("Edited {name}");
    let source: String = shown
        .iter()
        .map(|row| row.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let spans: Spans = language_for_path(&name).and_then(|lang| syntax::highlight(&source, lang));
    let digits = shown
        .iter()
        .filter_map(|row| row.num)
        .max()
        .unwrap_or(0)
        .to_string()
        .len()
        .max(2);
    let num_w = digits as f32 * 7.2;
    let chevron = if open {
        icons::arrows::ALT_ARROW_DOWN
    } else {
        icons::arrows::ALT_ARROW_RIGHT
    };
    // Same plate edges as the prompt, fences, and composer.
    div()
        .w_full()
        .ml(px(-root::COMPOSER_PAD_X))
        .mr(px(-root::COMPOSER_PAD_X))
        .rounded(px(CARD_RADIUS))
        .bg(ink(0.04))
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .id(("diff-card", ix))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .px(px(CARD_PAD_X))
                .py(px(CARD_PAD_Y))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.with_session(id, cx, |chat| {
                        if !chat.transcript.output.insert(ix) {
                            chat.transcript.output.remove(&ix);
                        }
                    });
                }))
                .child(
                    div()
                        .min_w(px(0.))
                        .flex_1()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(theme.text)
                        .child(spaced_label(title, theme.text, theme)),
                )
                .when(!running && !failed && (add + del > 0), |row| {
                    row.child(diff_badge(theme, add, del))
                })
                .when(running, |row| {
                    row.child(spinner(Duration::ZERO, theme.text_faint, cx))
                })
                .child(
                    icons::icon(chevron)
                        .size(px(12.))
                        .text_color(theme.text_faint),
                ),
        )
        .when(open && !failed, |el| {
            el.child(
                chain_scroll(
                    div()
                        .id(("diff-body", ix))
                        .max_h(px(224.))
                        .overflow_y_scroll(),
                    scroll,
                )
                .py(px(4.))
                .flex()
                .flex_col()
                .bg(diff_editor_bg(theme))
                .children({
                    let mut offset = 0usize;
                    shown
                        .into_iter()
                        .map(|row| {
                            let start = offset;
                            offset += row.text.len() + 1;
                            diff_row(theme, row, start, &spans, num_w)
                        })
                        .collect::<Vec<_>>()
                })
                .when(more, |body| {
                    body.child(
                        div()
                            .px(px(CARD_PAD_X))
                            .py(px(2.))
                            .font_family(theme.font_mono.clone())
                            .text_size(px(MONO_SIZE))
                            .line_height(px(MONO_LEAD))
                            .text_color(theme.text_faint)
                            .child("⋯"),
                    )
                }),
            )
        })
        .into_any_element()
}

fn diff_editor_bg(theme: &Theme) -> Hsla {
    if theme.appearance.is_light() {
        theme.bg
    } else {
        theme.surface_card
    }
}

fn diff_row(theme: &Theme, row: DiffRow, start: usize, spans: &Spans, num_w: f32) -> AnyElement {
    if row.kind == DiffKind::Gap {
        return div()
            .px(px(CARD_PAD_X))
            .py(px(2.))
            .font_family(theme.font_mono.clone())
            .text_size(px(MONO_SIZE))
            .line_height(px(MONO_LEAD))
            .text_color(theme.text_faint)
            .child("⋯")
            .into_any_element();
    }
    let (bg, sign, sign_color, tone): (Option<Hsla>, &str, Hsla, Hsla) = match row.kind {
        DiffKind::Add => (
            Some(diff_add_bg(theme)),
            "+",
            diff_add_mark(theme),
            theme.text,
        ),
        DiffKind::Del => (
            Some(diff_del_bg(theme)),
            "-",
            diff_del_mark(theme),
            theme.text,
        ),
        DiffKind::Ctx => (None, " ", theme.text_faint, theme.text_faint),
        DiffKind::Gap => unreachable!("gap painted above"),
    };
    let num = row.num.map(|n| n.to_string()).unwrap_or_default();
    let code = highlighted_diff_line(&row.text, start, spans, theme, tone);
    let line = div()
        .w_full()
        .flex()
        .flex_row()
        .items_start()
        .px(px(8.))
        .py(px(1.))
        .gap(px(8.))
        .font_family(theme.font_mono.clone())
        .text_size(px(MONO_SIZE))
        .line_height(px(MONO_LEAD))
        .child(
            div()
                .w(px(num_w))
                .flex_none()
                .text_color(theme.text_faint)
                .child(SharedString::from(num)),
        )
        .child(
            div()
                .w(px(10.))
                .flex_none()
                .text_color(sign_color)
                .child(SharedString::from(sign.to_string())),
        )
        .child(div().min_w(px(0.)).flex_1().child(code));
    match bg {
        Some(bg) => line.bg(bg).into_any_element(),
        None => line.into_any_element(),
    }
}

fn highlighted_diff_line(
    line: &str,
    start: usize,
    spans: &Spans,
    theme: &Theme,
    fallback: Hsla,
) -> AnyElement {
    if line.is_empty() {
        return div()
            .text_color(fallback)
            .child(SharedString::from(" "))
            .into_any_element();
    }
    let mut runs = Vec::new();
    let mut pos = 0usize;
    if let Some(spans) = spans {
        let end = start + line.len();
        for (range, kind) in spans.iter().filter(|(r, _)| r.end > start && r.start < end) {
            let s = range.start.clamp(start, end) - start;
            let e = range.end.min(end) - start;
            if e <= s || e <= pos {
                continue;
            }
            let s = s.max(pos);
            if s > pos {
                runs.push(TextRun {
                    len: s - pos,
                    font: font(theme.font_mono.clone()),
                    color: fallback,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                });
            }
            runs.push(TextRun {
                len: e - s,
                font: font(theme.font_mono.clone()),
                color: theme.syntax.color(*kind),
                background_color: None,
                underline: None,
                strikethrough: None,
            });
            pos = e;
        }
    }
    if pos < line.len() {
        runs.push(TextRun {
            len: line.len() - pos,
            font: font(theme.font_mono.clone()),
            color: fallback,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    styled_line(line, runs, fallback)
}

/// Kernel `Details.diff`: `+693 text` / `-693 text` / ` 693 text`, plus
/// unified diffs and the hashline snippet the model sees.
/// ChatView write card: every line of the new file is an add. Old history
/// often has `wrote /path` and no `ToolRec.diff` — read the file we wrote.
fn write_preview_from_output(output: &str) -> Option<String> {
    let path = output.lines().next()?.trim().strip_prefix("wrote ")?;
    let body = std::fs::read_to_string(path).ok()?;
    if body.is_empty() {
        return None;
    }
    Some(
        body.lines()
            .enumerate()
            .map(|(i, line)| format!("+{} {line}", i + 1))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn parse_diff(src: &str) -> Vec<DiffRow> {
    let details = src
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter(|line| numbered_diff_line(line) || diff_gap(line))
        .count();
    let total = src.lines().filter(|line| !line.trim().is_empty()).count();
    if details > 0 && details * 2 >= total {
        return parse_numbered_diff(src);
    }
    if has_unified_diff(src) {
        return parse_unified_diff(src);
    }
    parse_hashline_diff(src)
}

fn diff_gap(line: &str) -> bool {
    let trim = line.trim();
    trim == "..."
        || trim == "…"
        || (trim.contains('.') && trim.chars().all(|c| c == '.' || c.is_whitespace()))
}

fn numbered_diff_line(line: &str) -> bool {
    let Some(sign) = line.chars().next() else {
        return false;
    };
    if !matches!(sign, '+' | '-' | ' ') {
        return false;
    }
    let rest = line[sign.len_utf8()..].trim_start();
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0 && (rest.len() == digits || rest.as_bytes().get(digits) == Some(&b' '))
}

fn parse_numbered_diff(src: &str) -> Vec<DiffRow> {
    src.lines()
        .filter_map(|line| {
            if diff_gap(line) {
                return Some(DiffRow {
                    kind: DiffKind::Gap,
                    num: None,
                    text: String::new(),
                });
            }
            let sign = line.chars().next()?;
            let kind = match sign {
                '+' => DiffKind::Add,
                '-' => DiffKind::Del,
                ' ' => DiffKind::Ctx,
                _ => return None,
            };
            let rest = line[sign.len_utf8()..].trim_start();
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            let num = rest[..digits].parse().ok();
            let text = rest[digits..].strip_prefix(' ').unwrap_or("").to_owned();
            Some(DiffRow { kind, num, text })
        })
        .collect()
}

fn parse_unified_diff(src: &str) -> Vec<DiffRow> {
    let mut old = 0u32;
    let mut new = 0u32;
    let mut rows = Vec::new();
    for line in src.lines() {
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff ") {
            continue;
        }
        if let Some(hunk) = line.strip_prefix("@@") {
            if let Some((o, n)) = hunk_starts(hunk) {
                old = o;
                new = n;
            }
            continue;
        }
        if let Some(text) = line.strip_prefix('+') {
            rows.push(DiffRow {
                kind: DiffKind::Add,
                num: Some(new),
                text: text.to_owned(),
            });
            new += 1;
        } else if let Some(text) = line.strip_prefix('-') {
            rows.push(DiffRow {
                kind: DiffKind::Del,
                num: Some(old),
                text: text.to_owned(),
            });
            old += 1;
        } else if let Some(text) = line.strip_prefix(' ') {
            rows.push(DiffRow {
                kind: DiffKind::Ctx,
                num: Some(old),
                text: text.to_owned(),
            });
            old += 1;
            new += 1;
        }
    }
    rows
}

fn hunk_starts(hunk: &str) -> Option<(u32, u32)> {
    let mut old = None;
    let mut new = None;
    for part in hunk.split_whitespace() {
        if let Some(rest) = part.strip_prefix('-') {
            old = rest.split(',').next().and_then(|n| n.parse().ok());
        } else if let Some(rest) = part.strip_prefix('+') {
            new = rest.split(',').next().and_then(|n| n.parse().ok());
        }
    }
    Some((old?, new?))
}

fn parse_hashline_diff(src: &str) -> Vec<DiffRow> {
    hashline_lines(src)
        .filter_map(|line| {
            let trim = line.trim_start();
            let digits = trim.bytes().take_while(u8::is_ascii_digit).count();
            let num = trim[..digits].parse().ok();
            let rest = trim.split_once('|')?.1;
            Some(DiffRow {
                kind: DiffKind::Ctx,
                num,
                text: rest.to_owned(),
            })
        })
        .collect()
}

/// Cursor's file view: a short snippet under "Read foo.rs L40-69".
fn read_peek(theme: &Theme, label: &str, output: &str, take: usize) -> AnyElement {
    let lines: Vec<&str> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(take)
        .collect();
    if lines.is_empty() {
        return div().into_any_element();
    }
    let more = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
        > lines.len();
    let leaf = tool_leaf(label);
    div()
        .ml(px(16.))
        .mb(px(4.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(6.))
        .bg(ink(0.06))
        .flex()
        .flex_col()
        .gap(px(1.))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .child(
                    icons::icon(icons::files::DOCUMENT)
                        .size(px(11.))
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .text_style(TextStyle::Callout)
                        .text_color(theme.text_muted)
                        .child(SharedString::from(leaf)),
                ),
        )
        .children(lines.into_iter().map(|line| {
            let clean = clean_read_line(line);
            let short: String = clean.chars().take(88).collect();
            div()
                .font_family(theme.font_mono.clone())
                .text_style(TextStyle::Callout)
                .text_color(theme.text_muted)
                .child(SharedString::from(short))
                .into_any_element()
        }))
        .when(more, |el| {
            el.child(
                div()
                    .text_style(TextStyle::Callout)
                    .text_color(theme.text_faint)
                    .child("…"),
            )
        })
        .into_any_element()
}

/// Kernel dumps look like `2161:zbs|    let x`. Cursor shows `2161  let x`.
fn clean_read_line(line: &str) -> String {
    let trim = line.trim_start();
    let Some((meta, rest)) = trim.split_once('|') else {
        return line.to_owned();
    };
    let num: String = meta.chars().take_while(|c| c.is_ascii_digit()).collect();
    if num.is_empty() {
        return rest.trim_start().to_owned();
    }
    format!("{num:>4}  {}", rest.trim_start())
}

/// The web TerminalCard: command in the header, dim tail under it.
fn terminal_card(
    theme: &Theme,
    id: u64,
    ix: usize,
    label: &str,
    output: &str,
    running: bool,
    failed: bool,
    open: bool,
    scroll: ScrollBox,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let cmd = clean_shell(&tool_rest(label));
    let cmd = if cmd.is_empty() || is_job_log(&cmd) {
        String::new()
    } else {
        cmd
    };
    let take = 12;
    let lines: Vec<String> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != cmd && *line != "(no output)")
        .take(take)
        .map(|line| {
            let short: String = line.chars().take(88).collect();
            if line.chars().count() > 88 {
                format!("{short}…")
            } else {
                short
            }
        })
        .collect();
    // One line, like Cursor. A heredoc command carries its own newlines.
    let title: String = display_title(ToolKind::Execute, label, output, running)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // Same plate edges as the prompt, fences, and composer.
    div()
        .w_full()
        .ml(px(-root::COMPOSER_PAD_X))
        .mr(px(-root::COMPOSER_PAD_X))
        .rounded(px(CARD_RADIUS))
        .bg(ink(0.04))
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .id(("term-card", ix))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .px(px(CARD_PAD_X))
                .py(px(CARD_PAD_Y))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.with_session(id, cx, |chat| {
                        if !chat.transcript.output.insert(ix) {
                            chat.transcript.output.remove(&ix);
                        }
                    });
                }))
                .child(
                    icons::icon(icons::devices::TERMINAL)
                        .size(px(13.))
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .min_w(px(0.))
                        .flex_1()
                        .text_size(px(MONO_SIZE))
                        .line_height(px(MONO_LEAD))
                        .text_color(theme.text_faint)
                        .child(spaced_label(shorten(&title, 72), theme.text_faint, theme)),
                )
                .when(running, |row| {
                    row.child(spinner(Duration::ZERO, theme.text_faint, cx))
                }),
        )
        .when(open && (!lines.is_empty() || failed), |el| {
            el.child(
                chain_scroll(
                    div()
                        .id(("term-body", ix))
                        .max_h(px(176.))
                        .overflow_y_scroll(),
                    scroll,
                )
                .border_t_1()
                .border_color(theme.border)
                .px(px(CARD_PAD_X))
                .py(px(8.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(lines.into_iter().map(|line| {
                    div()
                        .font_family(theme.font_mono.clone())
                        .text_size(px(MONO_SIZE))
                        .line_height(px(MONO_LEAD))
                        .text_color(theme.text)
                        .child(SharedString::from(line))
                        .into_any_element()
                })),
            )
        })
        .into_any_element()
}

/// One entry of a turn's timeline, in Cursor's order: a folded thought,
/// a paragraph the model said mid-turn, or a run of tool calls (with the
/// thoughts between them) that folds to one summary line.
enum Seg {
    Thought(usize),
    Prose(usize),
    Run(Range<usize>),
    Other(usize),
}

/// Cut the interim half into segments. Thoughts that lead a run stand on
/// their own (`Thought 10s`); thoughts between two tool calls belong to
/// the run they interrupt.
fn segments(items: &[ChatItem], body: Range<usize>) -> Vec<Seg> {
    let mut segs = Vec::new();
    let mut ix = body.start;
    while ix < body.end {
        match &items[ix] {
            ChatItem::Agent(text) => {
                if !text.trim().is_empty() {
                    segs.push(Seg::Prose(ix));
                }
                ix += 1;
            }
            ChatItem::Thinking { .. } | ChatItem::Tool { .. } => {
                let start = ix;
                while ix < body.end
                    && matches!(items[ix], ChatItem::Thinking { .. } | ChatItem::Tool { .. })
                {
                    ix += 1;
                }
                let mut head = start;
                while head < ix && matches!(items[head], ChatItem::Thinking { .. }) {
                    segs.push(Seg::Thought(head));
                    head += 1;
                }
                if head < ix {
                    segs.push(Seg::Run(head..ix));
                }
            }
            // Artifacts are for the user, not part of the work: the turn
            // zone paints them below the fold whether it is open or not.
            ChatItem::Artifacts(_) => ix += 1,
            _ => {
                segs.push(Seg::Other(ix));
                ix += 1;
            }
        }
    }
    segs
}

struct WorkStats {
    tools: usize,
    files: usize,
    edits: usize,
    searches: usize,
    commands: usize,
    /// The kernel's own calls — plan items added or checked, workers
    /// spawned — which Cursor would fold into the run line rather than
    /// list as rows.
    plans: usize,
    todos: usize,
    spawns: usize,
    add: usize,
    del: usize,
    first_file: Option<String>,
    first_edit: Option<String>,
    /// The model's own words for the first command that had some (`bash`'s
    /// description): "Ran List repo contents and recent commits".
    first_desc: Option<String>,
    /// What the last tool of the range does — the verb a live header leads with.
    last_kind: Option<ToolKind>,
    /// Seconds the turn's tools and thoughts took, added up: the settled
    /// "Worked …" figure for records that predate the stamped wall time.
    secs: u64,
    /// The last tool's title, for a live run with nothing else to count
    /// (an `ask`, a `say`): "Ask Coffee or tea?" beats "Working".
    last_label: Option<String>,
}

/// Cursor: the timeline is on screen while the turn runs, then folds to
/// one line above the answer. A click takes over from there.
/// Cursor leaves the newest turn's timeline open — "Worked 6s ⌄" with its
/// Thought / Explored / Edited lines — until the next prompt folds it.
fn auto_work_open(items: &[ChatItem], first: usize, running: bool) -> bool {
    running
        || turns(items)
            .last()
            .is_some_and(|turn| turn.range.start == first)
}

fn work_stats(items: &[ChatItem], body: Range<usize>) -> WorkStats {
    let mut stats = WorkStats {
        tools: 0,
        files: 0,
        edits: 0,
        searches: 0,
        commands: 0,
        add: 0,
        del: 0,
        first_file: None,
        first_desc: None,
        first_edit: None,
        last_kind: None,
        secs: 0,
        last_label: None,
        plans: 0,
        todos: 0,
        spawns: 0,
    };
    let mut edited: HashSet<String> = HashSet::new();
    for item in &items[body] {
        if let ChatItem::Thinking {
            secs: Some(secs), ..
        } = item
        {
            stats.secs += u64::from(*secs);
        }
        if let ChatItem::Tool {
            kind,
            label,
            output,
            diff,
            secs,
            desc,
            ..
        } = item
        {
            if is_status_call(label) {
                continue;
            }
            stats.tools += 1;
            if stats.first_desc.is_none() {
                stats.first_desc = desc.clone();
            }
            stats.secs += u64::from(secs.unwrap_or(0));
            stats.last_label = Some(label.clone());
            let kind = coalesce_kind(*kind, label).unwrap_or(*kind);
            stats.last_kind = Some(kind);
            match kind {
                ToolKind::Read | ToolKind::Delete | ToolKind::Move => {
                    stats.files += 1;
                    if stats.first_file.is_none() {
                        stats.first_file = Some(tool_leaf(label));
                    }
                }
                ToolKind::Edit => {
                    let leaf = tool_leaf(label);
                    if edited.insert(leaf.clone()) {
                        stats.edits += 1;
                    }
                    if stats.first_edit.is_none() {
                        stats.first_edit = Some(leaf);
                    }
                    let counted = diff
                        .as_deref()
                        .filter(|text| !text.trim().is_empty())
                        .unwrap_or(output);
                    if let Some((add, del)) = diff_counts(counted) {
                        stats.add += add;
                        stats.del += del;
                    }
                }
                ToolKind::Search => stats.searches += 1,
                ToolKind::Execute => stats.commands += 1,
                _ => match label.split_whitespace().next().unwrap_or(label) {
                    "plan" => stats.plans += 1,
                    "todo" => stats.todos += 1,
                    "spawn" => stats.spawns += 1,
                    _ => {}
                },
            }
        }
    }
    stats
}

fn tool_leaf(label: &str) -> String {
    let rest = label
        .split_once(char::is_whitespace)
        .map(|(_, rest)| rest.trim())
        .filter(|rest| !rest.is_empty())
        .unwrap_or(label);
    rest.rsplit(['/', '\\']).next().unwrap_or(rest).to_owned()
}

/// The glyph for a tool's category — what the ACP `kind` is for.
fn tool_icon(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => icons::files::DOCUMENT,
        ToolKind::Edit => icons::editing::PEN,
        ToolKind::Delete => icons::files::TRASH_BIN_MINIMALISTIC,
        ToolKind::Move => icons::arrows::ARROW_RIGHT,
        ToolKind::Search => icons::system::MAGNIFER,
        ToolKind::Execute => icons::devices::TERMINAL,
        ToolKind::Think => icons::devices::CPU,
        ToolKind::Fetch => icons::devices::GLOBAL,
        ToolKind::SwitchMode => icons::system::TUNING,
        _ => icons::system::WIDGET,
    }
}

/// The transcript of one session, rendered from the model that owns it —
/// expanding a work section or a tool's output writes back through `cx`.
/// `tail` is what belongs at the end of the conversation right now — a
/// question the agent is asking, an approval it needs — drawn as cards in
/// the flow, in the reading column, not pinned over the composer.
pub fn render(
    chat: &ChatSession,
    changes: Option<crate::model::changes::GitChanges>,
    head: Option<AnyElement>,
    tail: Vec<AnyElement>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let id = chat.id;
    let turns = turns(&chat.items);
    let last = turns.len().saturating_sub(1);
    let mut zones: Vec<AnyElement> = Vec::new();
    // Cursor's Project chat opens on the project itself: icon, name, one
    // line, and the way to its page. A subagent's chat has none.
    if let Some(head) = head {
        zones.push(head);
    }
    // The reading column is the composer's: `CHAT_MAX_WIDTH` less its
    // gutter on each side. Each turn centres itself in the scroller.
    let column = root::CHAT_MAX_WIDTH - 2. * root::CHAT_GUTTER;
    let theme = Theme::of(cx).clone();
    let mut last_day: Option<i64> = None;
    // Copy, fork, thumbs once per response: a worker's report wakes the
    // agent into another turn, and a footer under each of those read as
    // three replies to one prompt. In each run of turns between two
    // prompts, the last one with an answer carries the footer.
    let mut footer_at: Vec<bool> = vec![false; turns.len()];
    {
        let mut run_start = 0;
        for position in 0..=turns.len() {
            let starts_run = position == turns.len()
                || (position > run_start
                    && matches!(
                        chat.items.get(turns[position].range.start),
                        Some(ChatItem::User(_))
                    ));
            if starts_run {
                if let Some(answered) = (run_start..position)
                    .rev()
                    .find(|p| turn_answer(&chat.items, &turns[*p]).is_some())
                {
                    footer_at[answered] = true;
                }
                run_start = position;
            }
        }
    }
    // Workers still running count as the turn still going: no footer yet.
    let workers_busy_now = chat
        .children
        .iter()
        .any(|c| c.state == crate::model::session::ChildState::Working);
    for (position, turn) in turns.iter().enumerate() {
        let running = position == last && chat.busy();
        let footer = footer_at[position] && !(position == last && workers_busy_now);
        // Cursor's Agents chat draws no date line over a turn; the footer's
        // "2m ago" is the only clock. The divider stays for a day crossing
        // inside one conversation, where "Yesterday" earns its line.
        // The kickoff turn has no prompt; its clock is when it was asked for.
        let kickoff_at = (turn.range.start == 0
            && !matches!(chat.items.first(), Some(ChatItem::User(_))))
        .then(|| chat.kickoff_at)
        .flatten()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64);
        let sent_at = match chat.items.get(turn.range.start) {
            Some(ChatItem::User(message)) => message.sent_at.filter(|at| *at > 0),
            Some(ChatItem::Wake { at, .. }) => *at,
            _ => kickoff_at,
        };
        if let Some(at) = sent_at {
            let day = local_day(at);
            // Cursor's Project chat draws "Today 11:35 PM" over the first
            // turn; a subagent's chat does not. Both draw one at a day
            // crossing.
            let first_of_root = last_day.is_none() && chat.parent.is_none();
            if first_of_root || (last_day.is_some_and(|previous| previous != day)) {
                zones.push(
                    div()
                        .w_full()
                        .max_w(px(column))
                        .self_center()
                        .child(day_divider(at, &theme))
                        .into_any_element(),
                );
            }
            last_day = Some(day);
        }
        zones.push(
            div()
                .w_full()
                .max_w(px(column))
                .self_center()
                .child(zone(chat, turn, running, footer, window, cx))
                .into_any_element(),
        );
    }
    // Cursor's "2 Files Changed · Review" card under the last answer: the
    // working tree's uncommitted files with their line counts, while the
    // chat is idle. The main chat of a local repository only.
    let workers_busy = chat
        .children
        .iter()
        .any(|c| c.state == crate::model::session::ChildState::Working);
    // Cursor's card follows a turn that changed files: an edit of its own,
    // or a worker (whose edits land in the tree, not in this chat's items).
    // A turn that only ran a test or read a file gets none.
    let newest_worked = turns.last().is_some_and(|turn| {
        let stats = work_stats(&chat.items, turn.range.clone());
        stats.edits > 0
            || stats.spawns > 0
            || chat.items[turn.range.clone()]
                .iter()
                .any(|item| matches!(item, ChatItem::From { .. }))
            || !spawned_in(&chat.items, turn.range.clone()).is_empty()
    });
    if !chat.busy()
        && !workers_busy
        && newest_worked
        && chat.parent.is_none()
        && chat.host.is_none()
        && !turns.is_empty()
        && let Some(changes) = changes.as_ref().filter(|c| !c.is_empty())
    {
        zones.push(
            div()
                .w_full()
                .max_w(px(column))
                .self_center()
                .child(files_changed_card(chat.cwd.clone(), changes, &theme, cx))
                .into_any_element(),
        );
    }
    for node in chat.plan_open().filter(|n| n.do_kind == "ask") {
        zones.push(
            div()
                .w_full()
                .max_w(px(column))
                .self_center()
                .child(ask_card(chat, node, &theme, cx))
                .into_any_element(),
        );
    }
    if let Some(card) = approve_card(chat, &theme, cx) {
        zones.push(
            div()
                .w_full()
                .max_w(px(column))
                .self_center()
                .child(card)
                .into_any_element(),
        );
    }
    for card in tail {
        zones.push(
            div()
                .w_full()
                .max_w(px(column))
                .self_center()
                .mt(px(8.))
                .child(card)
                .into_any_element(),
        );
    }
    // The drop target is a column flex, so `flex_1` here is a real height.
    // The rail is absolute on this pane — not a flex sibling — so it cannot
    // shift the centred column or collapse the scroll viewport.
    //
    // The scroller spans the whole pane, sidebar to sidebar: the wheel works
    // in the margins beside the column, not only over the words.
    div()
        .flex_1()
        .min_h_0()
        .relative()
        .child(
            div()
                .absolute()
                .inset_0()
                .child(
                    // Turns are the scroll container's own children:
                    // the rail addresses what gpui indexes.
                    div()
                        .id(("transcript", id))
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&chat.transcript.scroll)
                        // The wheel decides the follow: up while a turn
                        // streams unpins (Cursor lets you read back and
                        // shows ↓); back to the end pins again. Without
                        // this, growing content kept the old pin and
                        // snapped the reader to the bottom.
                        .on_scroll_wheel({
                            let handle = chat.transcript.scroll.clone();
                            let follow = chat.transcript.follow.clone();
                            move |_, _, _| {
                                let max = handle.max_offset().y;
                                let now = handle.offset().y.clamp(-max, px(0.));
                                follow.0.set((at_bottom(max, now, FOLLOW_SLACK), max));
                            }
                        })
                        .px(px(root::CHAT_GUTTER))
                        .pt(px(PAD))
                        .pb(px(PAD))
                        .flex()
                        .flex_col()
                        .children(zones),
                )
                .child(stick(&chat.transcript.scroll, &chat.transcript.follow)),
        )
        .child(rail(
            SharedString::from(format!("transcript-rail-{id}")),
            &chat.transcript.scroll,
            &chat.transcript.follow,
            &turns,
            &chat.items,
        ))
        .children({
            let theme = Theme::of(cx).clone();
            jump_to_end(
                id,
                &chat.transcript.scroll,
                &chat.transcript.follow,
                &theme,
                cx,
            )
        })
        .into_any_element()
}

/// An answer bubble a window from before this one wrote under its folded
/// question (the same words as the card's answer): drawn nowhere now.
/// A card that does not open a turn of its own: a steer typed into the
/// running turn (Cursor keeps the turn whole, the card inside its work), or
/// an older window's echo of an answered ask.
fn inline_user(items: &[ChatItem], ix: usize) -> bool {
    matches!(&items[ix], ChatItem::User(m) if m.steer) || is_ask_echo(items, ix)
}

fn is_ask_echo(items: &[ChatItem], ix: usize) -> bool {
    ix > 0
        && matches!(
            (&items[ix - 1], &items[ix]),
            (ChatItem::Asked { answer, .. }, ChatItem::User(message))
                if !answer.is_empty() && message.text.trim() == answer.trim()
        )
}

/// A question once answered: the card folded to one faint line, the
/// chosen answer (or "skipped") at its end — Cursor keeps the pick inside
/// the collapsed card, not in a bubble of yours.
fn asked_line(question: &str, answer: &str, theme: &Theme) -> AnyElement {
    div()
        .self_start()
        .w_full()
        .max_w(px(520.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .child(
            icons::icon(icons::system::CHAT_ROUND_LINE)
                .size(px(11.))
                .text_color(theme.text_faint),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_style(TextStyle::Caption)
                .text_color(theme.text_faint)
                .child(SharedString::from(format!(
                    "Question · {}",
                    question.trim()
                ))),
        )
        .child(
            div()
                .flex_none()
                .text_style(TextStyle::Caption)
                .text_color(if answer.is_empty() {
                    theme.text_faint
                } else {
                    theme.text_muted
                })
                .child(if answer.is_empty() {
                    "skipped".to_string()
                } else {
                    let mut short: String = answer.trim().chars().take(60).collect();
                    if answer.trim().chars().count() > 60 {
                        short.push('…');
                    }
                    short
                }),
        )
        .into_any_element()
}

/// Plan mode with approval: the agent wrote its checklist read-only and
/// the turn is over. One card at the end of the conversation — how many
/// steps stand — with Approve and run, which switches the chat to auto and
/// starts the work.
fn approve_card(
    chat: &ChatSession,
    theme: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<AnyElement> {
    let plan_mode = chat
        .modes
        .as_ref()
        .is_some_and(|m| m.current_mode_id.to_string() == "plan");
    if !plan_mode || chat.busy() {
        return None;
    }
    let steps = chat
        .plan_open()
        .filter(|n| !n.standing && n.do_kind != "ask")
        .count();
    if steps == 0 {
        return None;
    }
    let id = chat.id;
    Some(
        div()
            .mt(px(8.))
            .rounded(px(Theme::surface_radius()))
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface_raised)
            .px(px(12.))
            .py(px(10.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(
                icons::icon(icons::editing::CHECKLIST)
                    .size(px(12.))
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_style(TextStyle::Callout)
                    .text_color(theme.text)
                    .child(SharedString::from(format!(
                        "Plan ready · {}",
                        plural(steps, "step", "steps")
                    ))),
            )
            .child(
                theme
                    .ghost(SharedString::from(format!("plan-approve-{id}")))
                    .flex_none()
                    .px(px(8.))
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .rounded(px(Theme::control_radius()))
                    .border_1()
                    .border_color(theme.border)
                    .tooltip(|window, cx| {
                        Tooltip::text("Switch to auto and run the checklist", window, cx)
                    })
                    .child(
                        icons::icon(icons::media::PLAY)
                            .size(px(10.))
                            .text_color(theme.text),
                    )
                    .child(
                        div()
                            .text_style(TextStyle::Caption)
                            .text_color(theme.text)
                            .child("Approve and run"),
                    )
                    .on_click(cx.listener(move |workspace, _, _, cx| {
                        workspace.approve_plan(id, cx);
                    })),
            )
            .into_any_element(),
    )
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A question the agent parked for you — an `ask` node — as a card at the
/// turn it belongs to, after the last one. Answer points the composer's next
/// send at it (the placeholder says so); ✕ drops the question.
fn ask_card(
    chat: &ChatSession,
    node: &PlanNode,
    theme: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let id = chat.id;
    let ask = node.id;
    let answering = chat.answering == Some(ask);
    let group = SharedString::from(format!("ask-card-{id}-{ask}"));
    let head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .child(
            icons::icon(icons::system::CHAT_ROUND_LINE)
                .size(px(12.))
                .text_color(theme.accent),
        )
        .child(
            div()
                .flex_1()
                .text_style(TextStyle::Caption)
                .text_color(theme.text_faint)
                .child("Question for you"),
        )
        .child(
            theme
                .ghost(SharedString::from(format!("ask-drop-{id}-{ask}")))
                .flex_none()
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .tooltip(|window, cx| Tooltip::text("Drop this question", window, cx))
                .child(
                    icons::icon(icons::system::CLOSE)
                        .size(px(11.))
                        .text_color(theme.text_faint.opacity(0.35))
                        .group_hover(group.clone(), |el| el.text_color(theme.text)),
                )
                .on_click(cx.listener(move |workspace, _, _, cx| {
                    workspace.with_session(id, cx, |c| c.plan_op(ask, "cancel", ""));
                    cx.notify();
                })),
        );
    let answer = theme
        .ghost(SharedString::from(format!("ask-answer-{id}-{ask}")))
        .px(px(8.))
        .h(px(22.))
        .flex()
        .items_center()
        .rounded(px(Theme::control_radius()))
        .border_1()
        .border_color(theme.border)
        .text_style(TextStyle::Caption)
        .text_color(if answering { theme.accent } else { theme.text })
        .child(if answering {
            "Answering below…"
        } else {
            "Answer"
        })
        .on_click(cx.listener(move |workspace, _, _, cx| {
            workspace.with_session(id, cx, |c| {
                c.answering = if c.answering == Some(ask) {
                    None
                } else {
                    Some(ask)
                };
            });
            cx.notify();
        }));
    div()
        .group(group)
        .mt(px(8.))
        .rounded(px(Theme::surface_radius()))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface_raised)
        .px(px(12.))
        .py(px(10.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(head)
        .child(
            div()
                .text_style(TextStyle::Body)
                .text_color(theme.text)
                .child(SharedString::from(node.goal.clone())),
        )
        .child(div().flex().flex_row().child(answer))
        .into_any_element()
}

/// Cursor's `↓` disc, low and centred over the transcript, while the view
/// is scrolled away from the end. A click goes back to the bottom and pins.
fn jump_to_end(
    id: u64,
    handle: &ScrollHandle,
    follow: &Follow,
    theme: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<AnyElement> {
    let (pinned, _) = follow.0.get();
    if pinned || handle.max_offset().y <= px(0.5) {
        return None;
    }
    let handle = handle.clone();
    let follow = follow.clone();
    Some(
        div()
            .absolute()
            .bottom(px(10.))
            .left_0()
            .right(px(RAIL_INSET + MARK))
            .flex()
            .justify_center()
            .child(
                div()
                    .id(("jump-to-end", id))
                    .size(px(28.))
                    .rounded_full()
                    .bg(theme.surface_raised)
                    .border_1()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|el| el.bg(theme.surface_raised_hover))
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let max = handle.max_offset().y;
                        handle.set_offset(point(handle.offset().x, -max));
                        follow.0.set((true, max));
                        window.request_animation_frame();
                        cx.notify();
                    }))
                    .child(
                        icons::icon(icons::arrows::ARROW_DOWN)
                            .size(px(13.))
                            .text_color(theme.text_muted),
                    ),
            )
            .into_any_element(),
    )
}

/// Same geometry as `bezel::ui::scroll::rail` — a 16×2 mark, 10px between,
/// 12px in from the pane's right edge.
const MARK: f32 = 16.0;
const MARK_THICK: f32 = 2.0;
const MARK_GAP: f32 = 10.0;
const RAIL_INSET: f32 = 12.0;
/// Clickable pad around each mark. The painted dash stays 2px; the pad is
/// what a pointer can actually catch.
const MARK_HIT: f32 = MARK_THICK + MARK_GAP;

/// One dash per turn. A press jumps the transcript to that turn and unpins
/// stick-to-bottom, the same way a scrollbar click should feel.
///
/// Quiet until wanted: Cursor's chat has no rail, and on a long project a
/// dozen dashes down the right edge read as a second scrollbar (F-59). Only
/// the turn in view is drawn, faintly; the rest appear when the pointer is
/// over the rail's strip, where the jumps are.
fn rail(
    id: SharedString,
    handle: &ScrollHandle,
    follow: &Follow,
    turns: &[Turn],
    items: &[ChatItem],
) -> AnyElement {
    let count = turns.len();
    if count == 0 {
        return Empty.into_any_element();
    }
    let at = visible_turn(handle, count);
    let strip = SharedString::from(format!("{id}-strip"));
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .right(px(RAIL_INSET))
        .w(px(MARK))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .group(strip.clone())
        .children((0..count).map(|ix| {
            let strip = strip.clone();
            let handle = handle.clone();
            let follow = follow.clone();
            let tip = turn_label(items, &turns[ix], ix);
            let group = SharedString::from(format!("{id}-{ix}"));
            div()
                .id(group.clone())
                .group(group.clone())
                .w(px(MARK))
                .h(px(MARK_HIT))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .occlude()
                .tooltip(move |window, cx| Tooltip::text(tip.clone(), window, cx))
                .on_click(move |_, window, _| {
                    follow.unpin();
                    jump_to(&handle, ix, count);
                    window.refresh();
                })
                .child(
                    div()
                        .w(px(MARK))
                        .h(px(MARK_THICK))
                        .rounded_full()
                        .bg(if ix == at { ink(0.35) } else { ink(0.0) })
                        .group_hover(strip, move |mark| {
                            mark.bg(if ix == at { ink(0.6) } else { ink(0.2) })
                        })
                        .group_hover(group, |mark| mark.bg(ink(0.45))),
                )
        }))
        .into_any_element()
}

/// Bezel's follow canvas, reading our [`Follow`] so a rail click can unpin.
fn stick(handle: &ScrollHandle, follow: &Follow) -> AnyElement {
    let handle = handle.clone();
    let follow = follow.clone();
    canvas(
        move |_, window, _| {
            let max_offset = handle.max_offset().y;
            let offset = handle.offset().y;
            let (was_pinned, last_max) = follow.0.get();

            let pinned = if (max_offset - last_max).abs() > px(0.5) {
                was_pinned
            } else {
                at_bottom(max_offset, offset, FOLLOW_SLACK)
            };

            if pinned && (offset + max_offset).abs() > px(0.5) {
                handle.set_offset(point(handle.offset().x, -max_offset));
                window.request_animation_frame();
            }
            follow.0.set((pinned, max_offset));
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
    .into_any_element()
}

fn jump_to(handle: &ScrollHandle, ix: usize, count: usize) {
    if let Some(bounds) = handle.bounds_for_item(ix) {
        let y = (handle.bounds().top() - bounds.top()).clamp(-handle.max_offset().y, px(0.0));
        handle.set_offset(point(handle.offset().x, y));
        return;
    }
    // Bounds are a frame late the first time; ask gpui, and fall back to an
    // even split of the overflow so the jump still lands somewhere honest.
    handle.scroll_to_top_of_item(ix);
    let max = handle.max_offset().y;
    if max > px(0.0) && count > 1 {
        let t = ix as f32 / (count - 1) as f32;
        handle.set_offset(point(handle.offset().x, -max * t));
    }
}

fn visible_turn(handle: &ScrollHandle, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    if handle.bounds_for_item(0).is_some() {
        return handle.top_item().min(count - 1);
    }
    let max = handle.max_offset().y;
    if max <= px(0.0) {
        return count - 1;
    }
    let t = (handle.offset().y.clamp(-max, px(0.0)).abs() / max).clamp(0.0, 1.0);
    ((t * (count - 1) as f32).round() as usize).min(count - 1)
}

fn turn_label(items: &[ChatItem], turn: &Turn, ix: usize) -> SharedString {
    let n = ix + 1;
    let preview = items
        .get(turn.range.start)
        .and_then(item_text)
        .and_then(|text| text.lines().find(|line| !line.trim().is_empty()))
        .map(str::trim);
    match preview {
        Some(line) => {
            let short: String = line.chars().take(48).collect();
            if line.chars().count() > 48 {
                SharedString::from(format!("{short}…"))
            } else {
                SharedString::from(short)
            }
        }
        None => SharedString::from(format!("Turn {n}")),
    }
}

fn zone(
    chat: &ChatSession,
    turn: &Turn,
    running: bool,
    footer: bool,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = Theme::of(cx).clone();
    let first = turn.range.start;
    // The kickoff turn (Cursor's "Setting up environment") opens the
    // transcript with no prompt: its first item is work, not a card, and
    // it folds to "Worked Ns" once settled — Cursor shows no detail of it.
    let kickoff_turn = first == 0
        && chat.kickoff_at.is_some()
        && !matches!(
            chat.items.first(),
            Some(ChatItem::User(_) | ChatItem::From { .. })
        );
    // A worker's report under the wake it caused is the segment's first
    // line, drawn under the header where Cursor draws its "<agent> done —
    // …" line, not inside the fold.
    let report = (matches!(chat.items.get(first), Some(ChatItem::Wake { .. }))
        && matches!(chat.items.get(first + 1), Some(ChatItem::From { .. })))
    .then_some(first + 1);
    let body_start = if kickoff_turn {
        first
    } else {
        (first + 1 + usize::from(report.is_some())).min(turn.range.end)
    };
    let body = body_start..turn.answer_from.max(body_start);
    let stats = work_stats(&chat.items, body.clone());
    let auto_open = auto_work_open(&chat.items, first, running) && !kickoff_turn;
    let open = chat
        .transcript
        .work
        .get(&first)
        .copied()
        .unwrap_or_default()
        .get(auto_open);

    let mut zone = div().flex().flex_col().gap(px(ITEM_GAP)).pb(px(PAD));
    match chat.items.get(first) {
        Some(ChatItem::User(message)) => {
            zone = zone.child(user_prompt(chat, first, message, &theme, window, cx));
        }
        Some(ChatItem::From { who, text, images }) => {
            zone = zone.child(from_block(
                chat, first, who, text, images, &theme, window, cx,
            ));
        }
        Some(ChatItem::Notice { text, failed }) => {
            zone = zone.child(notice(chat, first, text, *failed, &theme, cx));
        }
        _ => {}
    }
    // Cursor's timeline: Thought 10s / prose / a folded run of tools /
    // prose / … while the turn runs. Once it settles the whole thing
    // folds to one summary line above the answer.
    let segs = segments(&chat.items, body.clone());
    let mut live_fold_shown = false;
    // A wake segment always carries its "Worked Ns" line, as Cursor's do,
    // even when the coordinator only read the report and moved on.
    let foldable = segs
        .iter()
        .any(|seg| matches!(seg, Seg::Run(_) | Seg::Prose(_)))
        || matches!(chat.items.get(first), Some(ChatItem::Wake { .. }));
    let mut header_drawn = false;
    let mut open = open;
    if !running && foldable {
        match work_header(chat.id, first, &stats, open, running, chat, cx) {
            Some(header) => {
                zone = zone.child(header);
                header_drawn = true;
            }
            // No headline to fold under: the body stands as it is.
            None => open = true,
        }
    }
    // Cursor's live headline over the timeline: "Working <step> ⌄" — the
    // step the agent named, else the kernel's; the rows of work under it.
    let live_headline = running && foldable && !kickoff_turn;
    if live_headline {
        let step = chat
            .status
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| chat.current_step())
            .unwrap_or_else(|| "Planning next moves".to_string());
        zone = zone.child(
            fold_row(
                &theme,
                "work",
                first,
                "Working".to_string(),
                step,
                None,
                true,
                true,
                cx,
            )
            .into_any_element(),
        );
        header_drawn = true;
        open = true;
    }
    // Cursor's Project chat (the root) shows no tool calls at all — the
    // coordinator's quick reads and commands are hidden work; its checklist
    // (`plan`, Cursor's TodoWrite) shows as a card. A worker's chat shows
    // every call.
    let project_style = chat.parent.is_none();
    // The worker's report that woke this segment, under its header.
    if let Some(ix) = report
        && let Some(ChatItem::From { who, text, images }) = chat.items.get(ix)
    {
        zone = zone.child(from_block(chat, ix, who, text, images, &theme, window, cx));
    }
    if open || !foldable {
        let last_run = segs.iter().rposition(|seg| matches!(seg, Seg::Run(_)));
        // A run still taking calls already shows a live line; a second
        // "Working" under it would say the same thing twice.
        // The run still taking calls is the last one with nothing but
        // inline cards (a steer's bubble) after it: the steer must not turn
        // "Running" into "Ran" before any output arrives.
        let tail_is_run = last_run.is_some_and(|at| {
            segs[at + 1..]
                .iter()
                .all(|seg| matches!(seg, Seg::Other(_)))
        });
        live_fold_shown = (running && tail_is_run) || live_headline;
        // Cursor's kickoff shows one shimmering line and nothing of the
        // work until it ends.
        let segs: Vec<Seg> = if kickoff_turn && running {
            Vec::new()
        } else {
            segs
        };
        if kickoff_turn && running {
            live_fold_shown = false;
        }
        let kids: Vec<AnyElement> = segs
            .iter()
            .enumerate()
            .map(|(n, seg)| match seg {
                Seg::Thought(ix) => thought(chat, *ix, false, window, cx),
                Seg::Prose(ix) => {
                    let ChatItem::Agent(text) = &chat.items[*ix] else {
                        return div().into_any_element();
                    };
                    div()
                        .w_full()
                        .max_w(px(root::CHAT_MAX_WIDTH))
                        .text_style(TextStyle::Body)
                        .text_color(theme.text)
                        .child(prose(chat, *ix, text, window, cx))
                        .into_any_element()
                }
                Seg::Run(range) if project_style && !own_calls(&chat.items, range.clone()) => {
                    // The root's kernel calls — spawn, say, plan, status —
                    // are the worker lines, the page, the live step; the
                    // run itself draws nothing but the checklist it wrote.
                    match todo_card(chat, range.clone(), &theme) {
                        Some(card) => card,
                        None => div().into_any_element(),
                    }
                }
                // The root's own quick call (a read-only command, a read)
                // shows as Cursor's Project chat shows it: "Running 1
                // command ⌄", then "Ran <what> ⌄" with the command card and
                // its output; the next prompt folds it under "Worked".
                Seg::Run(range) => {
                    // Present tense only on the run still taking calls.
                    let live = running && last_run == Some(n) && tail_is_run;
                    run_fold(chat, range.clone(), live, window, cx)
                }
                Seg::Other(ix) => work_other(chat, *ix, &theme, window, cx),
            })
            .collect();
        if !kids.is_empty() {
            zone = zone.child(div().flex().flex_col().gap(px(ITEM_GAP)).children(kids));
        }
    }
    // Cursor's sub-agent lines under the status: "1 Working  <task>" per
    // live child, and a check for each one that finished. Under the turn
    // that spawned them, once; the panel keeps the whole tree.
    let mut spawned = spawned_in(&chat.items, body.clone());
    // A `spawn wait=true` names its child only when it returns; until then
    // the worker is running under this turn with no record to hang from.
    // The running turn takes every child no turn has named yet.
    if running {
        let named = spawned_in(&chat.items, 0..chat.items.len());
        for child in &chat.children {
            if let Some(id) = &child.kernel_id
                && !named.contains(id)
                && !spawned.contains(id)
            {
                spawned.push(id.clone());
            }
        }
    }
    if !spawned.is_empty() && !chat.children.is_empty() {
        zone = zone.child(children_lines(chat, &spawned, &theme, cx));
    }
    // Standing work the turn set up: a small card at the moment it was made.
    zone = zone.children(subscription_cards(chat, body.clone(), &theme));
    // Screenshots and clips the work produced stay in view when the work
    // folds: they are what the user asked to see.
    for ix in body.clone() {
        if let ChatItem::Artifacts(files) = &chat.items[ix] {
            zone = zone.child(artifacts_row(chat, ix, files, &theme, cx));
        }
    }
    // ChatView `group/msg`: copy sits on the answer, hidden until
    // the pointer is over that answer. Retry lives on error cards only.
    let msg = SharedString::from(format!("msg-{first}"));
    let mut tail = div()
        .id(msg.clone())
        .group(msg.clone())
        .flex()
        .flex_col()
        .gap(px(ITEM_GAP));
    let mut has_tail = false;
    // The tail starts where the body ended: the report line under a wake
    // segment's header is drawn above, not again here.
    for ix in turn.answer_from.max(body_start)..turn.range.end {
        // The interruption is on the fold line already; once is enough.
        if header_drawn
            && let ChatItem::Notice { text, .. } = &chat.items[ix]
            && crate::model::session::is_interrupt_notice(text)
        {
            continue;
        }
        has_tail = true;
        // The turn's own end: say how long it had run before the cut.
        if let ChatItem::Notice { text, failed } = &chat.items[ix]
            && crate::model::session::is_interrupt_notice(text)
        {
            let secs = chat.items[..=first]
                .iter()
                .rev()
                .find_map(|item| match item {
                    ChatItem::User(message) => message.worked_secs,
                    _ => None,
                });
            let line = match secs {
                Some(s) if s > 0 => format!(
                    "{text} · after {}",
                    since(Duration::from_secs(u64::from(s)))
                ),
                _ => text.clone(),
            };
            tail = tail.child(notice(chat, ix, &line, *failed, &theme, cx));
            continue;
        }
        tail = tail.child(match &chat.items[ix] {
            ChatItem::Agent(text) => div()
                .self_start()
                .w_full()
                .max_w(px(root::CHAT_MAX_WIDTH))
                .text_style(TextStyle::Body)
                .text_color(theme.text)
                .child(prose(chat, ix, text, window, cx))
                .into_any_element(),
            ChatItem::From { who, text, images } => {
                from_block(chat, ix, who, text, images, &theme, window, cx)
            }
            ChatItem::Notice { text, failed } => notice(chat, ix, text, *failed, &theme, cx),
            ChatItem::Nudge(text) => page_nudge(text, &theme),
            ChatItem::Artifacts(files) => artifacts_row(chat, ix, files, &theme, cx),
            ChatItem::Asked { question, answer } => asked_line(question, answer, &theme),
            _ => div().into_any_element(),
        });
    }
    // Copy, fork, thumbs and "2m ago" belong to a finished turn. While
    // the turn still runs — a long model call with nothing streaming, a
    // job in flight — an interim reply already has words, and the footer
    // under them read as "done" next to a live Stop button.
    if !running
        && footer
        && let Some(answer) = turn_answer(&chat.items, turn)
    {
        has_tail = true;
        // Always shown, faint, as Cursor's are: copy, then fork.
        tail = tail.child(turn_footer(chat, first, answer, &theme, cx));
    }
    if has_tail {
        zone = zone.child(tail);
    }
    // Web WorkingIndicator: a sent prompt must not sit in silence. The
    // line is there the same frame the user message lands — before the
    // first thought token, and again whenever the turn is deciding its
    // next move with nothing streaming.
    if running && !live_fold_shown {
        if let Some(label) = heartbeat_label(chat, turn) {
            zone = zone.child(heartbeat(&theme, label, chat, cx));
        }
    }
    zone.into_any_element()
}

/// Quiet row under a step: ChatView shows only the copy control on hover.
fn turn_footer(
    chat: &ChatSession,
    turn: usize,
    answer: String,
    theme: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let id = chat.id;
    let showing = chat
        .transcript
        .copy_flash
        .get(&turn)
        .is_some_and(|at| at.elapsed() < COPY_FLASH);
    let glyph = if showing {
        icons::status::CHECK
    } else {
        icons::files::COPY
    };
    let row = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_start()
        .gap(px(6.))
        .pt(px(6.))
        .pb(px(2.));
    let copy = div()
        .id(SharedString::from(format!("copy-turn-{id}-{turn}")))
        .cursor_pointer()
        .rounded(px(4.))
        .p(px(3.))
        .hover(|el| el.bg(theme.element_hover))
        .active(|el| el.bg(theme.element_active))
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(answer.clone()));
            this.with_session(id, cx, |chat| {
                chat.transcript.copy_flash.insert(turn, Instant::now());
            });
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(COPY_FLASH).await;
                let _ = this.update(cx, |this, cx| {
                    this.with_session(id, cx, |chat| {
                        if chat
                            .transcript
                            .copy_flash
                            .get(&turn)
                            .is_some_and(|at| at.elapsed() >= COPY_FLASH)
                        {
                            chat.transcript.copy_flash.remove(&turn);
                        }
                    });
                });
            })
            .detach();
        }))
        .child(
            icons::icon(glyph)
                .size(px(12.))
                .text_color(theme.text_faint),
        );
    let msg_for_fork = SharedString::from(format!("msg-{turn}"));
    let fork = div()
        .id(SharedString::from(format!("fork-turn-{id}-{turn}")))
        // Cursor's Project and subagent footers have no fork; the
        // classic chat's does. Ours shows when the pointer is over
        // the answer, so the feature stays a reach away.
        .invisible()
        .group_hover(msg_for_fork, |el| el.visible())
        .cursor_pointer()
        .rounded(px(4.))
        .p(px(3.))
        .hover(|el| el.bg(theme.element_hover))
        .active(|el| el.bg(theme.element_active))
        .tooltip(|window, cx| Tooltip::text("Fork chat from here", window, cx))
        .on_click(cx.listener(move |this, _, _, cx| this.fork_session(id, cx)))
        .child(
            icons::icon(icons::editing::GIT_BRANCH)
                .size(px(12.))
                .text_color(theme.text_faint),
        );
    // Cursor keeps the checkpoint restore on the prompt card, not here; the
    // footer's copy shows only while the pointer is over the answer.
    let msg = SharedString::from(format!("msg-{turn}"));
    let rewind = div()
        .id(SharedString::from(format!("rewind-turn-{id}-{turn}")))
        .invisible()
        .group_hover(msg, |el| el.visible())
        .cursor_pointer()
        .rounded(px(4.))
        .p(px(3.))
        .hover(|el| el.bg(theme.element_hover))
        .active(|el| el.bg(theme.element_active))
        .tooltip(|window, cx| {
            Tooltip::text(
                "Rewind here: chat and files back to before this prompt",
                window,
                cx,
            )
        })
        .on_click(cx.listener(move |this, _, _, cx| this.rewind_turn(id, turn, cx)))
        .child(
            icons::icon(icons::system::RESTART)
                .size(px(12.))
                .text_color(theme.text_faint),
        );
    // Cursor's order: thumbs up, thumbs down, copy, fork, then "Just now".
    let (vote, sent_at) = match chat.items.get(turn) {
        Some(ChatItem::User(message)) => (message.feedback, message.sent_at),
        _ => (None, None),
    };
    let thumb = |up: bool, cx: &mut Context<Workspace>| {
        let value: i8 = if up { 1 } else { -1 };
        let lit = vote == Some(value);
        let (name, path, tip) = if up {
            ("up", crate::assets::THUMBS_UP_ICON, "Good answer")
        } else {
            ("down", crate::assets::THUMBS_DOWN_ICON, "Bad answer")
        };
        div()
            .id(SharedString::from(format!("vote-{name}-{id}-{turn}")))
            .cursor_pointer()
            .rounded(px(4.))
            .p(px(3.))
            .hover(|el| el.bg(theme.element_hover))
            .active(|el| el.bg(theme.element_active))
            .tooltip(move |window, cx| Tooltip::text(tip, window, cx))
            .on_click(cx.listener(move |this, _, _, cx| this.vote_turn(id, turn, value, cx)))
            .child(svg().path(path).size(px(12.)).text_color(if lit {
                theme.accent
            } else {
                theme.text_faint
            }))
    };
    let row = row
        .child(thumb(true, cx))
        .child(thumb(false, cx))
        .child(copy)
        .child(fork)
        .child(rewind)
        .when_some(sent_at.and_then(relative_time), |row, when| {
            row.child(
                div()
                    .id(SharedString::from(format!("turn-time-{id}-{turn}")))
                    .pl(px(4.))
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child(SharedString::from(when)),
            )
        });
    row.into_any_element()
}

/// Seconds east of UTC for the local clock at `at_secs`, from the C
/// library's `localtime_r` — the one source that knows this machine's zone
/// and its summer time.
fn local_offset_secs(at_secs: i64) -> i64 {
    let t: libc::time_t = at_secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    if ok { tm.tm_gmtoff as i64 } else { 0 }
}

/// The local calendar day of an instant, as days since the epoch.
fn local_day(at_ms: i64) -> i64 {
    let secs = at_ms / 1000;
    (secs + local_offset_secs(secs)).div_euclid(86_400)
}

/// "Today 4:22 PM", "Yesterday 9:10 AM", "Sep 12, 4:22 PM" — the line
/// Cursor draws over the first message of a day.
fn day_label(at_ms: i64) -> String {
    let secs = at_ms / 1000;
    let local = secs + local_offset_secs(secs);
    let day = local.div_euclid(86_400);
    let today = local_day(arbos_core::now_ms());
    let tod = local.rem_euclid(86_400);
    let (h24, m) = (tod / 3600, (tod % 3600) / 60);
    let (h12, ampm) = match h24 {
        0 => (12, "AM"),
        1..=11 => (h24, "AM"),
        12 => (12, "PM"),
        _ => (h24 - 12, "PM"),
    };
    let clock = format!("{h12}:{m:02} {ampm}");
    if day == today {
        format!("Today {clock}")
    } else if day == today - 1 {
        format!("Yesterday {clock}")
    } else {
        let (y, mo, d) = civil_from_days(day);
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let this_year = civil_from_days(today).0;
        if y == this_year {
            format!("{} {d}, {clock}", MONTHS[(mo - 1) as usize])
        } else {
            format!("{} {d}, {y}, {clock}", MONTHS[(mo - 1) as usize])
        }
    }
}

fn day_divider(at_ms: i64, theme: &Theme) -> AnyElement {
    div()
        .w_full()
        .flex()
        .justify_center()
        .pt(px(4.))
        .pb(px(10.))
        .child(
            div()
                .text_style(TextStyle::Caption)
                .text_color(theme.text_faint)
                .child(SharedString::from(day_label(at_ms))),
        )
        .into_any_element()
}

/// "Just now", "2m ago", "3h ago", "Yesterday", "3d ago", then a date.
/// `None` for a missing or absurd time (an old transcript with `ts: 0`).
fn relative_time(at_ms: i64) -> Option<String> {
    if at_ms <= 0 {
        return None;
    }
    let now = arbos_core::now_ms();
    let secs = (now - at_ms).max(0) / 1000;
    Some(match secs {
        s if s < 60 => "Just now".to_owned(),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3600),
        s if s < 2 * 86_400 => "Yesterday".to_owned(),
        s if s < 7 * 86_400 => format!("{}d ago", s / 86_400),
        _ => {
            let days = at_ms / 86_400_000;
            let (y, m, d) = civil_from_days(days);
            const MONTHS: [&str; 12] = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];
            let this_year = civil_from_days(now / 86_400_000).0;
            if y == this_year {
                format!("{} {d}", MONTHS[(m - 1) as usize])
            } else {
                format!("{} {d}, {y}", MONTHS[(m - 1) as usize])
            }
        }
    })
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The settled turn's one line above the answer: everything that happened,
/// folded. Click opens the timeline.
fn work_header(
    id: u64,
    turn: usize,
    stats: &WorkStats,
    open: bool,
    running: bool,
    chat: &ChatSession,
    cx: &mut Context<Workspace>,
) -> Option<AnyElement> {
    let theme = Theme::of(cx).clone();
    // Live: the clock since the turn began. Settled: the turn's wall time
    // stamped on its prompt; for records that predate the stamp, what the
    // tools and thoughts took.
    let stamped = match chat.items.get(turn) {
        Some(ChatItem::User(message)) => message.worked_secs.map(u64::from),
        Some(ChatItem::Wake { secs, .. }) => secs.map(u64::from),
        // The kickoff turn opens the transcript with no prompt.
        _ if turn == 0 => chat.kickoff_secs.map(u64::from),
        _ => None,
    };
    let elapsed = chat
        .elapsed()
        .filter(|_| running)
        .unwrap_or_else(|| Duration::from_secs(stamped.unwrap_or(stats.secs)));
    let (mut verb, rest) = work_summary(stats, running, elapsed, true);
    // A turn that was cut short says so on its one line, with the time it
    // had run, instead of a bare "Worked" over nothing.
    if !running && let Some(label) = interrupt_label_of(&chat.items, turn) {
        verb = label;
    }
    // Nothing to say — no time, no tools worth a word: Cursor draws no
    // line at all; the caller shows the body as it is.
    if verb == "Worked" && rest.is_empty() {
        return None;
    }
    // Cursor's headline is "Worked 12s ⌄" alone; the +3 −3 sits on the
    // "Edited 2 files" run line inside the fold.
    Some(
        fold_row(&theme, "work", turn, verb, rest, None, false, open, cx)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.with_session(id, cx, |chat| {
                    let running = chat.busy();
                    let auto = auto_work_open(&chat.items, turn, running);
                    chat.transcript.work.entry(turn).or_default().toggle(auto);
                });
            }))
            .into_any_element(),
    )
}

/// The interruption notice of the turn whose prompt is at `first`, if the
/// turn ended that way: "Stopped by you" or "Interrupted: …".
fn interrupt_label_of(items: &[ChatItem], first: usize) -> Option<String> {
    let turn = turns(items).into_iter().find(|t| t.range.start == first)?;
    items[turn.range.clone()]
        .iter()
        .rev()
        .find_map(|item| match item {
            ChatItem::Notice { text, .. } if crate::model::session::is_interrupt_notice(text) => {
                Some(text.clone())
            }
            _ => None,
        })
}

/// One run of tool calls as Cursor shows it: `Editing foo.rs, explored 7
/// files, 4 searches, ran 1 command +8 −1`. Closed by default; open, the
/// calls and the thoughts between them follow in order.
fn run_fold(
    chat: &ChatSession,
    range: Range<usize>,
    live: bool,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = Theme::of(cx).clone();
    let id = chat.id;
    let key = range.start;
    let stats = work_stats(&chat.items, range.clone());
    let (verb, rest) = work_summary(&stats, live, Duration::ZERO, false);
    // Cursor's run lines start with a capital: "Explored 2 files, 2 searches".
    let verb = {
        let mut chars = verb.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => verb,
        }
    };
    let diff = (stats.add + stats.del > 0).then_some((stats.add, stats.del));
    // A run with nothing to say for itself — a spawn, a call the stats do
    // not count — has no line; Cursor shows the rows, never a bare
    // "Worked" over them.
    let bare = !live && verb == "Worked" && rest.is_empty();
    // The headline of a turn without a stamped time already says what
    // this run did; one run under it would say it twice. Show the rows.
    let bare = bare || (!live && headline_describes(chat, range.start));
    let open = bare || chat.transcript.groups.contains(&key);
    div()
        .flex()
        .flex_col()
        .gap(px(ITEM_GAP))
        .when(!bare, |el| {
            el.child(
                fold_row(&theme, "run", key, verb, rest, diff, live, open, cx).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.with_session(id, cx, |chat| chat.transcript.toggle_group(key));
                    }),
                ),
            )
        })
        .when(open, |el| {
            el.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(ITEM_GAP))
                    .children(range.map(|ix| match &chat.items[ix] {
                        ChatItem::Thinking { .. } => thought(chat, ix, false, window, cx),
                        // A `status` call is the live step, not a row (#185).
                        ChatItem::Tool { label, .. } if is_status_call(label) => {
                            div().into_any_element()
                        }
                        // The root's kernel calls are drawn elsewhere (worker
                        // lines, the panel's page): no row for them here.
                        ChatItem::Tool { label, .. }
                            if chat.parent.is_none() && is_kernel_call(label) =>
                        {
                            div().into_any_element()
                        }
                        // A worker's `todo` call is its checklist card
                        // (Cursor's TodoWrite in a classic chat), not a
                        // bare "todo" row.
                        // A worker's `plan` writes its own notes file, so in
                        // its chat that call is a checklist card as well.
                        ChatItem::Tool { label, .. }
                            if is_todo_call(label)
                                || (chat.parent.is_some() && is_plan_call(label)) =>
                        {
                            let theme = Theme::of(cx).clone();
                            todo_card(chat, ix..ix + 1, &theme)
                                .unwrap_or_else(|| tool(chat, ix, false, cx))
                        }
                        ChatItem::Tool { .. } => tool(chat, ix, false, cx),
                        _ => div().into_any_element(),
                    })),
            )
        })
        .into_any_element()
}

/// Whether the turn holding item `ix` draws a headline that is the run
/// description itself (no time to say "Worked 12s") and that turn has this
/// one run — the case where a run line would repeat the headline.
fn headline_describes(chat: &ChatSession, ix: usize) -> bool {
    let Some(turn) = turns(&chat.items)
        .into_iter()
        .find(|t| t.range.contains(&ix))
    else {
        return false;
    };
    let stamped = match chat.items.get(turn.range.start) {
        Some(ChatItem::User(message)) => message.worked_secs.unwrap_or(0),
        Some(ChatItem::Wake { secs, .. }) => secs.unwrap_or(0),
        _ if turn.range.start == 0 => chat.kickoff_secs.unwrap_or(0),
        _ => 0,
    };
    if stamped > 0 {
        return false;
    }
    let body = (turn.range.start + 1).min(turn.range.end)..turn.answer_from;
    let runs = segments(&chat.items, body.clone())
        .iter()
        .filter(|seg| matches!(seg, Seg::Run(_)))
        .count();
    runs == 1 && work_stats(&chat.items, body).secs == 0
}

/// Cursor's fold line: the verb a shade brighter than what follows, the
/// `+N −M` badge, a spinner while live, and a chevron that shows on hover.
fn fold_row(
    theme: &Theme,
    name: &'static str,
    key: usize,
    verb: String,
    rest: String,
    diff: Option<(usize, usize)>,
    live: bool,
    open: bool,
    cx: &mut Context<Workspace>,
) -> bezel::gpui::Stateful<bezel::gpui::Div> {
    let group = SharedString::from(format!("{name}-{key}"));
    // Cursor sets "Worked 23s", "Running 1 command", "Ran …" all in the
    // prose size (14 px on the Mac, the bubble's own): one size for the
    // work lines, none a step smaller.
    let style = TextStyle::Body;
    div()
        .id((name, key))
        .group(group.clone())
        .self_start()
        .max_w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(ROW_GAP))
        .py(px(2.))
        .rounded(px(Theme::control_radius()))
        .cursor_pointer()
        .hover(|el| el.bg(theme.element_hover))
        .child(
            div()
                .text_style(style)
                .text_size(px(root::CURSOR_PROSE_SIZE))
                .text_color(theme.text_muted)
                .child(if live {
                    shimmer_label(verb, live_phase(), theme, cx)
                } else {
                    // Settled: Cursor paints "Worked" a shade brighter than
                    // the "23s" after it.
                    spaced_label(verb, theme.text_muted, theme)
                }),
        )
        .when(!rest.is_empty(), |el| {
            el.child(
                div()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_style(style)
                    .text_size(px(root::CURSOR_PROSE_SIZE))
                    .text_color(theme.text_faint)
                    .child(spaced_label(rest, theme.text_faint, theme)),
            )
        })
        .when_some(diff, |el, (add, del)| el.child(diff_badge(theme, add, del)))
        .child(
            // The turn's headline wears its chevron ("Worked 6m 44s ›"); a run
            // inside the opened timeline shows one on hover.
            div()
                .when(name != "work", |el| {
                    el.invisible().group_hover(group, |el| el.visible())
                })
                .child(Layout::disclosure(theme, open)),
        )
}

/// A clock every live shimmer shares, so rows sweep together.
pub(crate) fn live_phase() -> Duration {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed()
}

/// Cursor's fold label. Finished: "Worked 21s", as the Agents window
/// puts it — what was done is one click away in the fold. Live:
/// "Editing transcript.rs, 4 searches, ran 5 commands", present tense, so
/// the row says what is happening right now. Returned as (verb, rest) so the
/// verb can paint brighter than the rest.
fn work_summary(
    stats: &WorkStats,
    running: bool,
    elapsed: Duration,
    headline: bool,
) -> (String, String) {
    // The turn's one settled line says how long; a run inside the opened
    // timeline still says what it did.
    // A settled turn with a known time is "Worked 23s"; one without (a
    // record older than the stamp, a worker's report) says what it did
    // instead — Cursor never shows a bare "Worked".
    if !running && headline && elapsed.as_secs() > 0 {
        return ("Worked".to_owned(), since(elapsed));
    }
    let mut parts: Vec<String> = Vec::new();
    if stats.edits > 0 {
        let what = match (stats.edits, stats.first_edit.as_deref()) {
            (1, Some(file)) => file.to_owned(),
            (n, _) => format!("{n} files"),
        };
        parts.push(format!("edited {what}"));
    }
    if stats.files > 0 {
        let what = match (stats.files, stats.first_file.as_deref()) {
            (1, Some(file)) => file.to_owned(),
            (n, _) => format!("{n} files"),
        };
        parts.push(format!("explored {what}"));
    }
    if stats.searches > 0 {
        if parts.is_empty() {
            parts.push(format!(
                "searched {} {}",
                stats.searches,
                count_word(stats.searches, "time", "times")
            ));
        } else {
            parts.push(format!(
                "{} {}",
                stats.searches,
                count_word(stats.searches, "search", "searches")
            ));
        }
    }
    if stats.commands > 0 {
        // One command that described itself: Cursor's "Ran List repo
        // contents and recent commits". Several, or none described: the count.
        // Live it counts ("Running 1 command"); the words come once it
        // settles ("Ran List repo contents and recent commits"), as in Cursor.
        match (&stats.first_desc, stats.commands, stats.tools, running) {
            (Some(desc), 1, 1, false) => parts.push(format!("ran {desc}")),
            _ => parts.push(format!(
                "ran {} {}",
                stats.commands,
                count_word(stats.commands, "command", "commands")
            )),
        }
    }
    if stats.spawns > 0 {
        parts.push(format!(
            "started {} {}",
            stats.spawns,
            count_word(stats.spawns, "worker", "workers")
        ));
    }
    if stats.plans > 0 {
        parts.push(
            format!(
                "updated the project page {}",
                if stats.plans == 1 {
                    String::new()
                } else {
                    format!("{} times", stats.plans)
                }
            )
            .trim_end()
            .to_string(),
        );
    }
    if stats.todos > 0 {
        parts.push("kept its checklist".to_string());
    }
    if parts.is_empty() {
        return if running {
            match stats.last_label.as_deref() {
                Some(label) if !label.trim().is_empty() => {
                    let (verb, rest) = label.split_once(' ').unwrap_or((label, ""));
                    (verb.to_owned(), rest.to_owned())
                }
                _ => ("Working".to_owned(), String::new()),
            }
        } else {
            // With no time and nothing to describe there is no line; the
            // caller drops the header and shows the body open.
            (
                "Worked".to_owned(),
                if elapsed.as_secs() == 0 {
                    String::new()
                } else {
                    since(elapsed)
                },
            )
        };
    }
    let first = parts.remove(0);
    let (verb, arg) = first.split_once(' ').unwrap_or((first.as_str(), ""));
    let verb = match (verb, running) {
        ("edited", true) => "Editing",
        ("edited", false) => "Edited",
        ("explored", true) => "Exploring",
        ("explored", false) => "Explored",
        ("searched", true) => "Searching",
        ("searched", false) => "Searched",
        ("ran", true) => "Running",
        ("ran", false) => "Ran",
        ("started", true) => "Starting",
        ("started", false) => "Started",
        ("updated", true) => "Updating",
        ("updated", false) => "Updated",
        (other, _) => other,
    };
    let mut rest = arg.to_owned();
    if !parts.is_empty() {
        rest = format!("{rest}, {}", parts.join(", "));
    }
    (verb.to_owned(), rest)
}

fn count_word(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        one.to_owned()
    } else {
        many.to_owned()
    }
}

fn work_other(
    chat: &ChatSession,
    ix: usize,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    match &chat.items[ix] {
        ChatItem::Thinking { text, .. } => div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(6.))
            .text_style(TextStyle::Callout)
            .text_color(theme.text_muted.opacity(0.7))
            .child(
                icons::icon(icons::devices::CPU)
                    .size(px(12.))
                    .text_color(theme.text_faint),
            )
            .child(prose(chat, ix, text, window, cx))
            .into_any_element(),
        // Mid-turn chatter is not the answer. Cursor only paints the
        // reply after the last tool. This text still lives in `answer_from`.
        ChatItem::Agent(_) => div().into_any_element(),
        ChatItem::From { who, text, images } => {
            from_block(chat, ix, who, text, images, theme, window, cx)
        }
        ChatItem::Notice { text, failed } => notice(chat, ix, text, *failed, theme, cx),
        ChatItem::Artifacts(files) => artifacts_row(chat, ix, files, theme, cx),
        // A steer's card, inside the turn it steered.
        ChatItem::User(message) if message.steer => {
            user_prompt(chat, ix, message, theme, window, cx)
        }
        ChatItem::Asked { question, answer } => asked_line(question, answer, theme),
        _ => div().into_any_element(),
    }
}

/// Cursor shows one "Thought 14s" line per turn, even if the kernel
/// streamed several thought blocks.
/// Streamed thoughts stay open. Finished ones collapse to "Thought 14s".
fn thought(
    chat: &ChatSession,
    ix: usize,
    first: bool,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let _ = first;
    let ChatItem::Thinking { done, secs, .. } = &chat.items[ix] else {
        return div().into_any_element();
    };
    let marked_done = *done;
    let done = marked_done || !chat.busy();
    let secs = if marked_done {
        *secs
    } else if done {
        Some(0)
    } else {
        None
    };
    thought_line(chat, ix, done, secs, window, cx)
}

fn thought_line(
    chat: &ChatSession,
    ix: usize,
    done: bool,
    secs: Option<u32>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = Theme::of(cx).clone();
    let ChatItem::Thinking { text, .. } = &chat.items[ix] else {
        return div().into_any_element();
    };
    let id = chat.id;
    // A streaming thought opens its tail in a worker's chat; the root
    // (Cursor's Project chat) keeps to the "Thinking" line unless clicked.
    let folded = done || chat.parent.is_none();
    let open = chat.transcript.thought_open(ix, folded);
    let live = chat
        .thought_elapsed()
        .or_else(|| chat.elapsed())
        .unwrap_or_default();
    let (verb, time) = thought_label(done, secs, live);
    let has_body = !text.trim().is_empty();
    let group = SharedString::from(format!("thought-{ix}"));
    // Cursor: `Thought 10s` folded; a live thought streams its tail in the
    // dropdown, and an opened one shows the whole text.
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .id(("thought", ix))
                .group(group.clone())
                .self_start()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(ROW_GAP))
                .py(px(2.))
                .rounded(px(Theme::control_radius()))
                .cursor_pointer()
                .hover(|el| el.bg(theme.element_hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.with_session(id, cx, |chat| {
                        chat.transcript.toggle_thought(ix, folded);
                    });
                }))
                .child(
                    div()
                        .text_style(TextStyle::Body)
                        .text_size(px(root::CURSOR_PROSE_SIZE))
                        .text_color(theme.text_muted)
                        // Live, "Thinking" shimmers as Cursor's does; settled,
                        // the line is one faint colour.
                        .child(if done {
                            spaced_label(verb, theme.text_faint, &theme)
                        } else {
                            shimmer_label(verb, live, &theme, cx)
                        }),
                )
                .when(!time.is_empty(), |row| {
                    row.child(
                        div()
                            .text_style(TextStyle::Callout)
                            .text_color(theme.text_faint)
                            .child(spaced_label(time, theme.text_faint, &theme)),
                    )
                })
                .when(has_body, |row| {
                    row.child(
                        div()
                            .invisible()
                            .group_hover(group, |el| el.visible())
                            .child(Layout::disclosure(&theme, open)),
                    )
                }),
        )
        .when(open && has_body && !done, |el| {
            let tail: Vec<&str> = text.lines().collect();
            let skip = tail.len().saturating_sub(THOUGHT_TAIL_LINES);
            el.child(
                div()
                    .mt(px(2.))
                    .pl(px(CARD_PAD_X))
                    .border_l_1()
                    .border_color(theme.border)
                    .text_style(TextStyle::Callout)
                    .text_color(theme.text_muted)
                    .children(
                        tail.into_iter()
                            .skip(skip)
                            .map(|line| div().child(spaced_label(line, theme.text_muted, &theme))),
                    ),
            )
        })
        .when(open && has_body && done, |el| {
            el.child(
                div()
                    .mt(px(4.))
                    .text_style(TextStyle::Body)
                    .text_color(theme.text)
                    .child(prose(chat, ix, text, window, cx)),
            )
        })
        .into_any_element()
}

/// One tool call, and what it printed.
fn tool(chat: &ChatSession, ix: usize, first: bool, cx: &mut Context<Workspace>) -> AnyElement {
    let theme = Theme::of(cx).clone();
    let ChatItem::Tool {
        kind,
        label,
        status,
        output,
        diff,
        child_session,
        ..
    } = &chat.items[ix]
    else {
        return div().into_any_element();
    };
    let id = chat.id;
    let reconstructed;
    let peek = {
        let stored = diff
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .unwrap_or(output);
        if parse_diff(stored).is_empty() {
            reconstructed = write_preview_from_output(output);
            reconstructed.as_deref().unwrap_or(stored)
        } else {
            stored
        }
    };
    let edit_body = !parse_diff(peek).is_empty();
    let failed = *status == ToolStatus::Failure;
    let child = child_session.clone();
    let explore = explore_kind(*kind, label);
    let display_kind = explore.unwrap_or(*kind);
    // Web DiffCard / TerminalCard start open.
    let default_open =
        (display_kind == ToolKind::Edit && edit_body) || display_kind == ToolKind::Execute;
    let open = chat.transcript.output.contains(&ix) != default_open;
    let language = (explore.is_none() && file_tool(*kind))
        .then(|| code_language(&[label, output]))
        .flatten();
    let tone = theme.text_muted;
    let meta = (*status == ToolStatus::Running).then(|| {
        Painter::of(cx).lease(BRAILLE_FPS, BRAILLE_LEASE, cx);
        SharedString::from(
            spinner_frame(chat.elapsed().unwrap_or_default(), cx.reduce_motion()).to_string(),
        )
    });
    let diff = (display_kind == ToolKind::Edit)
        .then(|| diff_counts(peek))
        .flatten();
    // ChatView SummaryRow (read / ls / find / grep) has no chevron.
    // Reads still open a cheap peek on click. Searches stay title-only.
    // Reads get a cheap peek, not a highlighter — a 400-line file dump
    // is what froze the machine. Searches stay title-only.
    let file_view = display_kind == ToolKind::Read && !output.trim().is_empty();
    let show_output = (explore.is_none() || file_view || failed) && !output.is_empty();
    let _ = first;
    if display_kind == ToolKind::Edit && edit_body {
        return diff_card(
            &theme,
            id,
            ix,
            label,
            peek,
            *status == ToolStatus::Running,
            failed,
            open,
            chat.transcript.scroll_box(ix),
            cx,
        );
    }
    if display_kind == ToolKind::Execute {
        return terminal_card(
            &theme,
            id,
            ix,
            label,
            output,
            *status == ToolStatus::Running,
            failed,
            open,
            chat.transcript.scroll_box(ix),
            cx,
        );
    }
    div()
        .child(
            tool_row(
                &theme,
                tool_icon(display_kind),
                display_parts_for(
                    display_kind,
                    label,
                    output,
                    *status == ToolStatus::Running,
                    failed,
                ),
                tone,
                meta,
                diff,
                None,
            )
            .hover(|el| el.bg(theme.element_hover))
            .id(("tool", ix))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(session) = child.as_ref() {
                    if let Some(child_id) = this.ensure_child_agent(id, session.clone(), cx) {
                        this.select_session(child_id, cx);
                    }
                    return;
                }
                this.with_session(id, cx, |chat| {
                    if !chat.transcript.output.insert(ix) {
                        chat.transcript.output.remove(&ix);
                    }
                });
            })),
        )
        // A closed row is one line. Peeking output out of every closed
        // step is what made a thirty-call turn scroll for pages: the file
        // dump, diff, or command tail shows up when the row is opened.
        .when(open && file_view, |el| {
            el.child(read_peek(&theme, label, output, 16))
        })
        .when(
            open && show_output
                && !file_view
                && !matches!(display_kind, ToolKind::Execute | ToolKind::Edit),
            |el| {
                el.child(match language {
                    Some(language) => {
                        // The language comes from the label, which does not
                        // change for a given call, so the output text alone is
                        // the key.
                        let spans = chat
                            .transcript
                            .spans
                            .get(ix, output, |text| syntax::highlight(text, language));
                        highlighted_output(
                            ("tool-output", ix),
                            output,
                            &spans,
                            chat.transcript.scroll_box(ix),
                            &theme,
                        )
                    }
                    None => theme
                        .step_output(("tool-output", ix), output.clone())
                        .into_any_element(),
                })
            },
        )
        .into_any_element()
}

/// Same bones as [`Status::step_row`]. Titles stay on the muted face.
fn tool_row(
    theme: &Theme,
    _icon: &'static str,
    title: (impl AsRef<str>, Option<String>),
    tone: Hsla,
    meta: Option<SharedString>,
    diff: Option<(usize, usize)>,
    expanded: Option<bool>,
) -> bezel::gpui::Div {
    let (verb, arg) = title;
    let arg = arg
        .as_deref()
        .map(str::trim)
        .filter(|arg| !arg.is_empty())
        .map(str::to_owned);
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .py(px(2.))
        .cursor_pointer()
        .child(
            div()
                .flex_none()
                .text_style(TextStyle::Callout)
                .text_color(tone)
                .child(spaced_label(verb.as_ref(), tone, theme)),
        )
        .when_some(arg, |row, arg| {
            row.child(
                div()
                    .min_w(px(0.))
                    .flex_1()
                    .font_family(theme.font_mono.clone())
                    .text_size(px(MONO_SIZE))
                    .line_height(px(MONO_LEAD))
                    .text_color(tone.opacity(0.8))
                    .child(mono_label(arg, tone.opacity(0.8), theme)),
            )
        })
        .when_some(diff, |row, (add, del)| {
            row.child(diff_badge(theme, add, del))
        })
        .child(
            div()
                .ml_auto()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .when_some(meta, |cluster, meta| {
                    cluster.child(
                        div()
                            .font_family(theme.font_mono.clone())
                            .text_style(TextStyle::Body)
                            .text_color(theme.text_muted)
                            .child(meta),
                    )
                })
                .when_some(expanded, |cluster, expanded| {
                    cluster.child(Layout::disclosure(theme, expanded))
                }),
        )
}

/// Cursor paints edit size as a quiet +N −M next to the file name.
/// Kernel `edit` often prints `edited path` plus a hashline snippet, not
/// a unified diff — count those snippet lines as the shown hunk.
fn diff_counts(output: &str) -> Option<(usize, usize)> {
    let rows = parse_diff(output);
    if !rows.is_empty() {
        let add = rows.iter().filter(|row| row.kind == DiffKind::Add).count();
        let del = rows.iter().filter(|row| row.kind == DiffKind::Del).count();
        if add + del > 0 {
            return Some((add, del));
        }
    }
    let mut add = 0usize;
    let mut del = 0usize;
    for line in output.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            add += 1;
        } else if line.starts_with('-') {
            del += 1;
        }
    }
    (add + del > 0).then_some((add, del))
}

fn hashline_lines(output: &str) -> impl Iterator<Item = &str> {
    output.lines().filter(|line| {
        let trim = line.trim_start();
        let digits = trim.bytes().take_while(u8::is_ascii_digit).count();
        digits > 0 && trim[digits..].contains('|')
    })
}

fn has_unified_diff(output: &str) -> bool {
    output.lines().any(|line| {
        (line.starts_with('+') || line.starts_with('-'))
            && !line.starts_with("+++")
            && !line.starts_with("---")
    })
}

fn diff_add_bg(theme: &Theme) -> Hsla {
    if theme.appearance.is_light() {
        theme.diff_add.opacity(0.08)
    } else {
        rgb(0x1e2b24).into()
    }
}

fn diff_del_bg(theme: &Theme) -> Hsla {
    if theme.appearance.is_light() {
        theme.diff_del.opacity(0.08)
    } else {
        rgb(0x33201f).into()
    }
}

fn diff_add_mark(theme: &Theme) -> Hsla {
    if theme.appearance.is_light() {
        theme.diff_add
    } else {
        rgb(0x3fb950).into()
    }
}

fn diff_del_mark(theme: &Theme) -> Hsla {
    if theme.appearance.is_light() {
        theme.diff_del
    } else {
        rgb(0xf85149).into()
    }
}

fn diff_badge(theme: &Theme, add: usize, del: usize) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .gap(px(6.))
        .font_family(theme.font_mono.clone())
        .text_size(px(11.))
        .line_height(px(15.))
        .when(add > 0, |el| {
            el.child(
                div()
                    .text_color(diff_add_mark(theme))
                    .child(SharedString::from(format!("+{add}"))),
            )
        })
        .when(del > 0, |el| {
            el.child(
                div()
                    .text_color(diff_del_mark(theme))
                    .child(SharedString::from(format!("−{del}"))),
            )
        })
        .into_any_element()
}

/// Tool output painted with the file's highlighter — rust keywords, go
/// types — not one muted grey for every language. `spans` is what the
/// highlighter said about `text`, remembered by the caller across frames.
fn highlighted_output(
    id: impl Into<bezel::gpui::ElementId>,
    text: &str,
    spans: &Spans,
    scroll: ScrollBox,
    theme: &Theme,
) -> AnyElement {
    let mono = font(theme.font_mono.clone());
    let run = |len: usize, color: Hsla| TextRun {
        len,
        font: mono.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut offset = 0usize;
    let lines: Vec<AnyElement> = text
        .split('\n')
        .map(|line| {
            let start = offset;
            offset += line.len() + 1;
            let mut runs = Vec::new();
            let mut pos = 0usize;
            if let Some(spans) = &spans {
                let end = start + line.len();
                for (range, kind) in spans.iter().filter(|(r, _)| r.end > start && r.start < end) {
                    let s = range.start.clamp(start, end) - start;
                    let e = range.end.min(end) - start;
                    if e <= s || e <= pos {
                        continue;
                    }
                    let s = s.max(pos);
                    if s > pos {
                        runs.push(run(s - pos, theme.text));
                    }
                    runs.push(run(e - s, theme.syntax.color(*kind)));
                    pos = e;
                }
            }
            if pos < line.len() {
                runs.push(run(line.len() - pos, theme.text));
            }
            styled_line(line, runs, theme.text)
        })
        .collect();
    chain_scroll(div().id(id).max_h(px(256.)).overflow_y_scroll(), scroll)
        .border_t_1()
        .border_color(theme.border)
        .px(px(10.))
        .py(px(6.))
        .font_family(theme.font_mono.clone())
        .text_style(TextStyle::Callout)
        .children(lines)
        .into_any_element()
}

/// Paint a highlighted line only when the runs cover it exactly.
/// `StyledText::with_runs` panics in debug on a mismatch, and that used
/// to take the whole detail column — composer included — with it.
fn styled_line(line: &str, runs: Vec<TextRun>, fallback: Hsla) -> AnyElement {
    let covered: usize = runs.iter().map(|run| run.len).sum();
    if covered == line.len() && !runs.is_empty() {
        return StyledText::new(SharedString::from(line.to_string()))
            .with_runs(runs)
            .into_any_element();
    }
    div()
        .text_color(fallback)
        .child(SharedString::from(line.to_string()))
        .into_any_element()
}

/// One braille cell, advanced from how long the turn has been running.
/// The lease keeps the window ticking while this is on screen.
fn spinner_frame(since: Duration, reduce_motion: bool) -> &'static str {
    if reduce_motion {
        "···"
    } else {
        BRAILLE[(since.as_millis() / BRAILLE_TICK_MS) as usize % BRAILLE.len()]
    }
}

/// One braille cell, advanced from how long the turn has been running.
/// The lease keeps the window ticking while this is on screen.
pub fn spinner<V: 'static>(since: Duration, color: Hsla, cx: &mut Context<V>) -> AnyElement {
    Painter::of(cx).lease(BRAILLE_FPS, BRAILLE_LEASE, cx);
    div()
        .text_style(TextStyle::Callout)
        .text_color(color)
        .child(spinner_frame(since, cx.reduce_motion()))
        .into_any_element()
}

/// The same cell for a row drawn from another view's state: the caller's
/// painter keeps that view ticking.
pub fn spinner_with(
    painter: Painter,
    since: Duration,
    color: Hsla,
    cx: &mut bezel::gpui::App,
) -> AnyElement {
    painter.lease(BRAILLE_FPS, BRAILLE_LEASE, cx);
    div()
        .text_style(TextStyle::Callout)
        .text_color(color)
        .child(spinner_frame(since, cx.reduce_motion()))
        .into_any_element()
}

/// How long a streaming tail may sit still before the heartbeat comes
/// back. Web `STALE_TAIL_MS`: long enough to ignore between-token
/// pauses, short enough that a tool-argument gap does not look frozen.
const STALE_TAIL_MS: u128 = 1000;

/// Web WorkingIndicator copy. A fresh prompt is "Planning next moves";
/// a lull mid-turn is "Working".
fn heartbeat_label(chat: &ChatSession, turn: &Turn) -> Option<String> {
    // The kernel says the model is thinking in silence: always show it,
    // whatever the last item is.
    if chat.working.is_some() {
        return Some("Thinking".to_string());
    }
    // The kickoff turn (no prompt of the user's, the chat's first) reads
    // as Cursor's "Setting up environment" whatever step the kernel derives.
    let kickoff = turn.range.start == 0
        && chat.kickoff_at.is_some()
        && !matches!(chat.items.first(), Some(ChatItem::User(_)));
    // The agent named its step (Cursor's UpdateCurrentStep on the
    // timeline: "Copying stills to artifacts"): that is the line.
    if let Some(step) = chat
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && !kickoff
    {
        return Some(step.to_string());
    }
    let last = chat.items.get(turn.range.start..turn.range.end)?.last()?;
    let tool_running = matches!(
        last,
        ChatItem::Tool {
            status: ToolStatus::Running,
            ..
        }
    );
    let live = match last {
        ChatItem::Thinking {
            done: false, text, ..
        } if !text.trim().is_empty() => true,
        ChatItem::Tool {
            status: ToolStatus::Running,
            ..
        } => true,
        ChatItem::Agent(text) if !text.is_empty() => true,
        _ => false,
    };
    let stale = live
        && !tool_running
        && chat
            .updated
            .elapsed()
            .ok()
            .is_some_and(|d| d.as_millis() >= STALE_TAIL_MS);
    if live && !stale {
        return None;
    }
    if tool_running {
        return None;
    }
    Some(
        match last {
            _ if kickoff => "Setting up environment",
            ChatItem::User(_) => "Planning next moves",
            _ => "Working",
        }
        .to_string(),
    )
}

/// Web: spinner + shimmering label under the last item while the turn
/// is live and nothing else is moving.
fn heartbeat(
    theme: &Theme,
    label: String,
    chat: &ChatSession,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let since = chat.elapsed().unwrap_or_default();
    // A silent model call: the braille spinner and "Thinking for 42s",
    // ticking, so a minute of thought never looks like a dead turn.
    if let Some(thinking) = chat.thinking_for() {
        Painter::of(cx).lease(2.0, Duration::from_millis(1100), cx);
        let text = format!("Thinking for {}", since_short(thinking));
        return div()
            .id("thinking-heartbeat")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(ROW_GAP))
            .py(px(2.))
            .child(spinner(since, theme.text_muted, cx))
            .child(
                div()
                    .text_style(TextStyle::Body)
                    .text_size(px(root::CURSOR_PROSE_SIZE))
                    .text_color(theme.text_muted)
                    .child(shimmer_label(text, since, theme, cx)),
            )
            .into_any_element();
    }
    // A turn that has produced nothing for a while — a kickoff whose
    // provider is not answering (Jacob waited 98 s on a silent shimmer and
    // gave up, F-77) — says how long, and past a minute what to do. The
    // clock ticks, so it never reads as frozen.
    let quiet = since >= STALL_CLOCK_AFTER;
    let stalled = since >= STALL_HINT_AFTER;
    if quiet {
        Painter::of(cx).lease(2.0, Duration::from_millis(1100), cx);
    }
    let label = if quiet {
        format!("{label} · {}", since_short(since))
    } else {
        label
    };
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .py(px(2.))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(ROW_GAP))
                .child(
                    div()
                        .text_style(TextStyle::Body)
                        .text_size(px(root::CURSOR_PROSE_SIZE))
                        .text_color(theme.text_muted)
                        .child(shimmer_label(label, since, theme, cx)),
                ),
        )
        .when(stalled, |el| {
            el.child(
                div()
                    .id("stall-hint")
                    .text_style(TextStyle::Callout)
                    .text_color(theme.text_faint)
                    .child(SharedString::from(format!(
                        "Nothing has arrived in {}. Stop to try again, or check the model key in Settings.",
                        since_short(since)
                    ))),
            )
        })
        .into_any_element()
}

/// When a silent turn's heartbeat starts showing its clock, and when it
/// adds the hint.
const STALL_CLOCK_AFTER: Duration = Duration::from_secs(20);
const STALL_HINT_AFTER: Duration = Duration::from_secs(60);

/// `42s`, `1m 05s`: the thinking clock.
pub(crate) fn since_short(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// Cursor: `Thinking` while it streams, `Thought 10s` once it is done.
/// Returned as (verb, time) so the time can paint fainter.
fn thought_label(done: bool, secs: Option<u32>, _live: Duration) -> (String, String) {
    if !done {
        return ("Thinking".to_owned(), String::new());
    }
    // Cursor: "Thought briefly" under a few seconds, "Thought for 12s" past.
    let time = match secs {
        Some(s) if s >= 5 => format!("for {}", since(Duration::from_secs(u64::from(s)))),
        _ => "briefly".to_owned(),
    };
    ("Thought".to_owned(), time)
}

/// How many trailing lines of a streaming thought show in its dropdown.
const THOUGHT_TAIL_LINES: usize = 6;

/// A turn's age, in the coarsest unit that still says something.
fn since(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs < 60 {
        true => format!("{secs}s"),
        false => format!("{}m {}s", secs / 60, secs % 60),
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    #[test]
    fn a_steer_stays_inside_the_turn_it_steered() {
        let mut steer = UserMessage::from("Also print the date at the end.".to_string());
        steer.steer = true;
        let items = vec![
            ChatItem::User(UserMessage::from("Run the loop.".to_string())),
            ChatItem::Tool {
                id: "t1".into(),
                kind: ToolKind::Execute,
                label: "bash for i in 1 2 3".into(),
                status: ToolStatus::Success,
                output: "step 1".into(),
                diff: None,
                child_session: None,
                secs: None,
                desc: None,
            },
            ChatItem::User(steer),
            ChatItem::Agent("Done, with the date.".into()),
            ChatItem::User(UserMessage::from("Thanks.".to_string())),
        ];
        let turns = turns(&items);
        assert_eq!(turns.len(), 2, "a steer opens no turn of its own");
        assert_eq!(turns[0].range, 0..4);
        assert_eq!(turns[1].range, 4..5);
    }

    #[test]
    fn a_settled_run_folds_and_a_click_pins_it_open() {
        // Twelve thoughts that lead the turn each stand on their own line;
        // a tool call after them starts the run they lead into.
        let mut items: Vec<ChatItem> = (0..12)
            .map(|i| ChatItem::Thinking {
                text: format!("step {i}"),
                done: true,
                secs: Some(1),
            })
            .collect();
        items.push(ChatItem::Tool {
            id: "t1".into(),
            kind: ToolKind::Read,
            label: "read a.rs".into(),
            status: ToolStatus::Success,
            output: String::new(),
            diff: None,
            child_session: None,
            secs: None,
            desc: None,
        });
        let segs = segments(&items, 0..items.len());
        assert_eq!(
            segs.iter()
                .filter(|seg| matches!(seg, Seg::Thought(_)))
                .count(),
            12
        );
        assert!(matches!(segs.last(), Some(Seg::Run(range)) if range.start == 12));
        // Cursor: the timeline shows while the turn runs and stays open on
        // the newest turn; an older turn folds once it settles, and from
        // there a click owns the fold.
        assert!(auto_work_open(&items, 0, true));
        assert!(
            auto_work_open(&items, 0, false),
            "the newest turn stays open"
        );
        let mut older = items.clone();
        older.push(ChatItem::User("Next".to_string().into()));
        assert!(!auto_work_open(&older, 0, false), "an older turn folds");
        let mut state = State::default();
        assert!(!state.groups.contains(&0));
        state.toggle_group(0);
        assert!(
            state.groups.contains(&0),
            "opening the fold pins the run open"
        );
        state.toggle_group(0);
        assert!(!state.groups.contains(&0));
    }

    #[test]
    fn thinking_starts_open_and_respects_manual_collapse() {
        let mut state = State::default();
        let mut items = vec![
            ChatItem::User("Question".to_string().into()),
            ChatItem::Thinking {
                text: "First token".into(),
                done: false,
                secs: None,
            },
        ];
        assert_eq!(turns(&items)[0].answer_from, 2);
        assert!(state.thought_open(1, false));
        state.toggle_thought(1, false);
        let ChatItem::Thinking { text, done, .. } = &mut items[1] else {
            unreachable!()
        };
        text.push_str(" and more streamed text");
        assert!(!state.thought_open(1, false));
        *done = true;
        assert!(!state.thought_open(1, true));
        state.toggle_thought(1, true);
        assert!(state.thought_open(1, true));
        assert!(state.thought_open(2, false));
        assert!(!state.output.contains(&1));
    }

    struct SelectionView {
        state: State,
        paints: usize,
    }

    impl gpui::Render for SelectionView {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            self.paints += 1;
            div()
                .debug_selector(|| "selection-test".into())
                .child(selectable::render(
                    "selection-test",
                    &markdown::parse("First paragraph.\n\nSecond paragraph."),
                    &self.state.layouts(0),
                    self.state.selection(0),
                    self.state.dragging,
                    None,
                    false,
                    window,
                    cx,
                    |view, pointer, cx| {
                        view.state
                            .point(0, "First paragraph.\n\nSecond paragraph.", pointer);
                        cx.notify();
                    },
                ))
        }
    }

    #[gpui::test]
    fn rendered_double_click_selects_and_repaints(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            bezel::theme::appearance::init(bezel::theme::appearance::AppearanceMode::default(), cx);
        });
        let (view, cx) = cx.add_window_view(|_, _| SelectionView {
            state: State::default(),
            paints: 0,
        });
        cx.run_until_parked();
        let before = view.read_with(cx, |v, _| v.paints);
        let bounds = cx
            .debug_bounds("selection-test")
            .expect("selectable message is rendered");
        let position = bounds.origin + point(px(12.), px(12.));
        cx.simulate_mouse_move(position, None, gpui::Modifiers::default());
        cx.simulate_event(gpui::MouseDownEvent {
            position,
            button: gpui::MouseButton::Left,
            modifiers: gpui::Modifiers::default(),
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_mouse_up(
            position,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        view.read_with(cx, |v, _| {
            assert!(v.paints > before);
            let selected = v
                .state
                .selection(0)
                .expect("double-click reached selection handler");
            assert_eq!(
                selectable::copied(
                    &markdown::parse("First paragraph.\n\nSecond paragraph."),
                    selected
                ),
                "First paragraph.\nSecond paragraph."
            );
        });
    }

    #[test]
    fn image_only_message_renders_images_and_starts_a_turn() {
        use crate::model::attachment::{MessageImage, UserMessage};
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(8, 4)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let image = MessageImage::from_part(
            &serde_json::json!({"image":{"data":STANDARD.encode(bytes.into_inner())}}),
        )
        .unwrap();
        let items = vec![
            ChatItem::User("first".to_string().into()),
            ChatItem::Agent("answer".into()),
            ChatItem::User(UserMessage {
                text: String::new(),
                images: vec![image.clone(), image],
                described: Vec::new(),
                steer: false,
                files: Vec::new(),
                worked_secs: None,
                channel: String::new(),
                sent_at: None,
                feedback: None,
            }),
            ChatItem::Agent("two pictures".into()),
        ];
        let sections = turns(&items);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[1].range, 2..4);
        assert_eq!(item_text(&items[2]), Some(""));
        let ChatItem::User(message) = &items[2] else {
            unreachable!()
        };
        assert_eq!(message_images(&message.images).count(), 2);
        assert_eq!(message_images(&[]).count(), 0);
    }

    #[test]
    fn double_click_selects_every_block_of_the_message() {
        let text =
            "# Heading\n\nHello **world**.\n\n```rust\nlet x = 1;\n```\n\nLast 日本語 paragraph.";
        let mut state = State::default();
        state.point(3, text, Pointer::SelectAll);
        let selection = state.selection(3).unwrap();
        assert_eq!(
            selectable::copied(&markdown::parse(text), selection),
            "Heading\nHello world.\nlet x = 1;\nLast 日本語 paragraph."
        );
        assert!(!state.dragging);
        assert!(state.selection(2).is_none());
    }

    #[test]
    fn double_click_selection_survives_motion_and_release() {
        let text = "Select this whole message.";
        let mut state = State::default();
        state.point(0, text, Pointer::Down(Cursor::default()));
        state.point(0, text, Pointer::SelectAll);
        let selected = state.selection(0);
        state.point(0, text, Pointer::Move(Cursor::default()));
        assert!(state.point(0, text, Pointer::Up).is_none());
        assert!(state.point(0, text, Pointer::Up).is_none());
        assert_eq!(state.selection(0), selected);
    }

    #[test]
    fn double_click_does_not_open_selected_link() {
        let text = "[another chat](arbos://chat/123)";
        let mut state = State::default();
        state.point(0, text, Pointer::SelectAll);
        assert!(state.point(0, text, Pointer::Up).is_none());
        assert_eq!(
            selectable::copied(&markdown::parse(text), state.selection(0).unwrap()),
            "another chat"
        );
    }

    #[test]
    fn single_click_and_drag_still_select_a_range() {
        let text = "hello world";
        let mut state = State::default();
        state.point(0, text, Pointer::SelectAll);
        state.point(1, text, Pointer::Down(Cursor::default()));
        state.point(
            1,
            text,
            Pointer::Move(Cursor {
                offset: 5,
                ..Cursor::default()
            }),
        );
        state.point(1, text, Pointer::Up);
        assert!(state.selection(0).is_none());
        assert_eq!(
            selectable::copied(&markdown::parse(text), state.selection(1).unwrap()),
            "hello"
        );
    }
}

/// Cursor's card under an answer that changed files: "2 Files Changed" with
/// Review on the right, then one row per file — its glyph, name, and
/// `+N −M`. A row opens that file's diff; Review opens the whole tree's.
fn files_changed_card(
    root: std::path::PathBuf,
    changes: &crate::model::changes::GitChanges,
    theme: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let n = changes.files.len();
    let head = format!("{n} File{} Changed", if n == 1 { "" } else { "s" });
    let review_root = root.clone();
    let mut card = div()
        .id("files-changed")
        .w_full()
        .mt(px(4.))
        .rounded(px(Theme::surface_radius()))
        .border_1()
        .border_color(theme.border)
        .bg(theme.ink(0.02))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .px(px(12.))
                .py(px(8.))
                .border_b_1()
                .border_color(theme.border)
                .text_style(TextStyle::Callout)
                .child(
                    div()
                        .flex_1()
                        .text_color(theme.text)
                        .child(SharedString::from(head)),
                )
                .child(
                    div()
                        .id("files-changed-review")
                        .cursor_pointer()
                        .text_color(theme.text_muted)
                        .hover(|el| el.text_color(theme.text))
                        .child("Review")
                        .on_mouse_down(
                            bezel::gpui::MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.review_changes(&review_root, None, cx);
                            }),
                        ),
                ),
        );
    const SHOWN: usize = 8;
    for (ix, file) in changes.files.iter().take(SHOWN).enumerate() {
        let path = file.path.clone();
        let review_root = root.clone();
        let name = std::path::Path::new(&file.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.path.clone());
        card = card.child(
            div()
                .id(("files-changed-row", ix))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .px(px(12.))
                .py(px(5.))
                .cursor_pointer()
                .hover(|el| el.bg(theme.element_hover))
                .text_style(TextStyle::Callout)
                .child(
                    icons::icon(icons::files::DOCUMENT)
                        .size(px(12.))
                        .flex_none()
                        .text_color(theme.text_faint),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text)
                        .child(SharedString::from(name)),
                )
                .child(crate::view::detail::diff_marks(theme, file.add, file.del))
                .on_mouse_down(
                    bezel::gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.review_changes(&review_root, Some(&path), cx);
                    }),
                ),
        );
    }
    if changes.files.len() > SHOWN {
        let more = changes.files.len() - SHOWN;
        let review_root = root.clone();
        card = card.child(
            div()
                .id("files-changed-more")
                .px(px(12.))
                .py(px(5.))
                .cursor_pointer()
                .text_style(TextStyle::Caption)
                .text_color(theme.text_faint)
                .hover(|el| el.text_color(theme.text_muted))
                .child(SharedString::from(format!("+{more} more")))
                .on_mouse_down(
                    bezel::gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, cx| this.review_changes(&review_root, None, cx)),
                ),
        );
    }
    card.into_any_element()
}

/// The text with markdown's marks escaped, so it renders as it was typed:
/// a prompt is the user's words, not a document. Line breaks stay.
pub(crate) fn plain_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for (n, line) in text.split('\n').enumerate() {
        if n > 0 {
            // Two spaces before the break: a hard line break in CommonMark.
            out.push_str("  \n");
        }
        let trimmed = line.trim_start();
        let lead = &line[..line.len() - trimmed.len()];
        out.push_str(lead);
        let mut chars = trimmed.chars().peekable();
        let mut at_start = true;
        while let Some(c) = chars.next() {
            match c {
                '`' | '*' | '_' | '~' | '[' | ']' | '\\' => {
                    out.push('\\');
                    out.push(c);
                }
                '#' | '>' | '-' | '+' if at_start => {
                    out.push('\\');
                    out.push(c);
                }
                '0'..='9' if at_start => {
                    // "1. " would start a list; keep the digits, escape the dot.
                    out.push(c);
                    while let Some(d) = chars.peek().copied().filter(char::is_ascii_digit) {
                        out.push(d);
                        chars.next();
                    }
                    if chars.peek() == Some(&'.') {
                        out.push('\\');
                    }
                }
                _ => out.push(c),
            }
            at_start = false;
        }
    }
    out
}

/// The kernel's `status` tool: what the agent says it is doing, carried
/// by the worker line and the panel. Never a row of its own.
pub(crate) fn is_status_call(label: &str) -> bool {
    label
        .split_whitespace()
        .next()
        .is_some_and(|first| first == "status")
}

/// Cursor's TodoWrite card, from the root's `todo` calls in a run (the
/// thread's own checklist, `agents/root/todo.md`): the list the kernel
/// echoed back, one row per item with its box. The last call's echo is the
/// list's state. `plan` edits the project page, which the panel shows.
fn todo_card(chat: &ChatSession, range: Range<usize>, theme: &Theme) -> Option<AnyElement> {
    let key = range.start;
    let (title, items) = range
        .rev()
        .filter_map(|ix| match &chat.items[ix] {
            ChatItem::Tool { label, output, .. }
                if is_todo_call(label) || (chat.parent.is_some() && is_plan_call(label)) =>
            {
                plan_items(output)
            }
            _ => None,
        })
        .next()?;
    if items.is_empty() {
        return None;
    }
    let done = items.iter().filter(|(d, _)| *d).count();
    let mut card = div()
        .id(("todo-card", key))
        .w_full()
        .max_w(px(root::CHAT_MAX_WIDTH))
        .rounded(px(Theme::surface_radius()))
        .border_1()
        .border_color(theme.border)
        .bg(theme.ink(0.02))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .px(px(12.))
                .py(px(7.))
                .border_b_1()
                .border_color(theme.border)
                .text_style(TextStyle::Callout)
                .child(
                    div()
                        .flex_1()
                        .text_color(theme.text)
                        .child(SharedString::from(title)),
                )
                .child(
                    div()
                        .text_color(theme.text_faint)
                        .child(SharedString::from(format!("{done}/{}", items.len()))),
                ),
        );
    for (done, text) in items {
        card = card.child(
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(8.))
                .px(px(12.))
                .py(px(4.))
                .text_style(TextStyle::Callout)
                .child(if done {
                    icons::icon(icons::status::CHECK)
                        .size(px(12.))
                        .flex_none()
                        .mt(px(3.))
                        .text_color(theme.success)
                        .into_any_element()
                } else {
                    // Cursor's open item: a hollow circle.
                    div()
                        .size(px(10.))
                        .flex_none()
                        .mt(px(4.))
                        .rounded_full()
                        .border_1()
                        .border_color(theme.text_faint)
                        .into_any_element()
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(if done { theme.text_faint } else { theme.text })
                        .child(SharedString::from(text)),
                ),
        );
    }
    Some(card.into_any_element())
}

/// The kernel names a call by its tool and first argument: `todo`,
/// `todo add`, `todo check 2`.
fn is_todo_call(label: &str) -> bool {
    label == "todo" || label.starts_with("todo ")
}

/// A call to the kernel about the project rather than to the world: it
/// shows as a worker line, the page, a checklist or the live step, never as
/// a tool row of the root's.
fn is_kernel_call(label: &str) -> bool {
    matches!(
        label.split_whitespace().next().unwrap_or(label),
        "spawn"
            | "say"
            | "plan"
            | "todo"
            | "status"
            | "subscribe"
            | "remember"
            | "agents"
            | "transcript"
            | "ask"
    )
}

/// Whether a run holds a call of the root's own — a command, a read, a
/// search — that Cursor's Project chat would show, as against kernel calls
/// only (which draw as worker lines and cards).
fn own_calls(items: &[ChatItem], range: Range<usize>) -> bool {
    items[range].iter().any(|item| {
        matches!(item, ChatItem::Tool { label, .. } if !is_kernel_call(label) && !is_status_call(label))
    })
}

fn is_plan_call(label: &str) -> bool {
    label == "plan" || label.starts_with("plan ")
}

/// The kernel's echo of a checklist after a `todo` (or `plan`) call:
/// "## Maintenance" then lines "[ ] 1 - [ ] [label](target) — readout".
/// Returns the section title and (done, words) per item.
fn plan_items(output: &str) -> Option<(String, Vec<(bool, String)>)> {
    let mut title = "Todos".to_string();
    let mut items = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if let Some(heading) = line.strip_prefix("## ") {
            title = heading.trim().to_string();
            continue;
        }
        let (done, rest) = if let Some(rest) = line.strip_prefix("[x] ") {
            (true, rest)
        } else if let Some(rest) = line.strip_prefix("[ ] ") {
            (false, rest)
        } else {
            continue;
        };
        // "1 - [ ] [label](target) — readout": drop the index and the inner box.
        let rest = rest
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start();
        let rest = rest.strip_prefix("- ").unwrap_or(rest);
        let rest = rest
            .strip_prefix("[ ] ")
            .or_else(|| rest.strip_prefix("[x] "))
            .unwrap_or(rest);
        let words = strip_link(rest);
        if !words.is_empty() {
            items.push((done, words));
        }
    }
    (!items.is_empty()).then_some((title, items))
}

/// "[label](target) — readout" → "label — readout".
fn strip_link(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match (after.find("]("), after.find(')')) {
            (Some(close), Some(end)) if end > close => {
                out.push_str(&after[..close]);
                rest = &after[end + 1..];
            }
            _ => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// "set explainer" → "Set explainer": a worker's folder name as a title.
fn sentence_case(words: &str) -> String {
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
