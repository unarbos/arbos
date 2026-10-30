//! The chat column body: a bottom-aligned `list()` with one row per markdown
//! block (zeron-study §4.1–§4.4). Rows are diffed by `(id, version)` and
//! spliced; streaming text fades in through the veil.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    actions, canvas, div, list, prelude::*, px, Animation, AnimationExt, AnyElement, App, Bounds,
    BoxShadow, Context, Div, Entity, FontWeight, ListAlignment, ListScrollEvent, ListState,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, SharedString,
    Stateful, TextRun, Window,
};

use crate::app::{AppEvent, AppState, DetailsTarget};
use crate::model::{
    now_ms, Attachment, AttachmentKind, Chat, Id, LinkPreview, Message, MessageStatus, Part,
    Person, Project, ReplyRef, Role, SpaceKind, StatusKind, TurnKind,
};
use crate::ui::approve::{approval_card, ApprovalView};
use crate::ui::attachments;
use crate::ui::avatars;
use crate::ui::cards;
use crate::ui::chat_search;
use crate::ui::frost::edge_faded;
use crate::ui::glass::WindowDragPress;
use crate::ui::highlight;
use crate::ui::identity;
use crate::ui::markdown::{
    self, inline_plain, inline_spans, polish_assistant_prose, split_blocks, Block,
};
use crate::ui::math;
use crate::ui::mention::{self, Mentions};
use crate::ui::motion::PulseClock;
use crate::ui::people;
use crate::ui::profile_target;
use crate::ui::secret_card::SecretOfferCard;
use crate::ui::select::{BubbleAt, BubbleSelectable, LinkIcons, Selectable, Span, TextSelection};
use crate::ui::sf_symbols;
use crate::ui::theme::{
    Theme, BUBBLE_PAD_X, BUBBLE_PAD_Y, BUBBLE_RADIUS, CHAT_BODY, CHAT_LINE_HEIGHT, COLUMN_MARGIN,
    COMPOSER_HEIGHT, COMPOSER_WIDTH, LINE_HEIGHT, PLUS_BUTTON, TEXT_SMALL, TEXT_TINY,
};
use crate::ui::urlwrap;
use crate::ui::veil::Veil;
use crate::ui::widgets::QuestionCard;
use serde_json::Value;

actions!(transcript, [Copy]);

/// Where one of my own bubbles is on its way. No caption says it (Jacob,
/// 2026-10-24: "remove these 'sent' items, just use coloring"), and nothing
/// after delivery dims it (Jacob, 2026-10-25: delivered bubbles that waited
/// for a read receipt "show up too dim, I think they have not been sent").
/// See docs/mac-design-rules.md § Own-bubble delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Delivery {
    /// Not yet acknowledged by the relay: the only dimmed state.
    Pending,
    /// The relay refused it: full opacity, red mark + Retry beside the bubble.
    Failed,
    /// Acknowledged by the relay: the full own-bubble style, at once. Read
    /// receipts never change how my bubble looks (agents send none, members
    /// on older builds send none, a member who has not opened the chat sends
    /// none — a bubble waiting for one reads as unsent).
    Delivered,
}

/// Whole-bubble opacity of a pending bubble (white text on `#5c5c5c` /
/// `#111111`: 3.4 : 1 dark, 3.6 : 1 light — transient, until the relay's ACK).
pub const BUBBLE_ALPHA_PENDING: f32 = 0.5;

impl Delivery {
    pub fn opacity(self) -> f32 {
        match self {
            Delivery::Pending => BUBBLE_ALPHA_PENDING,
            Delivery::Failed | Delivery::Delivered => 1.0,
        }
    }
}

const OVERDRAW_PX: f32 = 320.0;
/// One column for every chat kind: the rows and the composer are centred
/// in the pane with the same side margin ([`COLUMN_MARGIN`]) and the same
/// width cap, so the agent bubble's left edge is the composer's left edge
/// and my bubble's right edge is the composer's right edge, at any pane
/// width. Grok Mac (refs 57/58, 2 px/pt): both at 16 pt from the pane. The
/// cap is the composer pill at its widest — the 904 pt input plus the two
/// 28 pt circles = Grok's 960 ([feedback mac-sidebar-rail-2026-10-26]; 876
/// before) — so the pill itself does not move; up to a 992 pt pane the
/// margins bound both.
pub const COLUMN_MAX_WIDTH: f32 = COMPOSER_WIDTH + 2.0 * PLUS_BUTTON;
/// Agent bubble cap, measured from Grok Mac on two pane widths (ref 57:
/// 728 pt pane, 612 pt bubble; ref 58: 424 pt pane, 310 pt bubble): the
/// bubble ends a fixed 84 pt before the column's right edge (100 pt from
/// the pane), not at a fixed width. The hover rail (reply · copy · react ·
/// time) lives in that gap, so hovering never re-wraps the text. Only the
/// sent/seen label sits inside the bubble; the time is hover-only
/// (docs/mac-design-rules.md).
pub const ASSISTANT_RIGHT_GAP: f32 = 84.0;
/// Agent bubble ceiling, measured on Jacob's Grok Bot shot of 2026-10-24
/// (`grok-chat-1.png` / `grok-chat-2-research.png`, 2 px/pt, a 1728 pt
/// window): every wrapped agent bubble ends at 1120 px = **560 pt** in a
/// 960 pt composer column (58 % of it) — the list bubble is content-sized
/// narrower, nothing is wider. Ours filled the column minus the gap (792 at
/// the then 876 pt cap, 90 %), which is what Jacob saw as "Arbos bubbles are
/// wider" ([feedback bubble-width-2026-10-24]). The cap is the smaller of
/// the two rules ([`assistant_cap`]): narrow panes keep Grok's fixed gap
/// (refs 57/58: 612 in a 696 pill, 310 in 392), wide panes stop at 560.
pub const ASSISTANT_MAX_WIDTH: f32 = 560.0;

/// The widest an agent bubble (or a question / approval card in its slot) can
/// be in a slot `slot_width` wide: the slot minus [`ASSISTANT_RIGHT_GAP`],
/// never more than [`ASSISTANT_MAX_WIDTH`].
pub fn assistant_cap(slot_width: f32) -> f32 {
    (slot_width - ASSISTANT_RIGHT_GAP).min(ASSISTANT_MAX_WIDTH)
}
/// My own bubble's share of the column in an agent chat (parity it.36): the
/// right edge on the composer's, at most 72 % of the column.
pub const OWN_BUBBLE_FRACTION: f32 = 0.72;
/// People chats — DM, group, channel, a project with people in it — take
/// more of the width on both sides (Jacob, 2026-10-26, [feedback
/// chat-density-mac-2026-10-26]: "use more of the screen both ways, left and
/// right per chat bubble"): my bubble may reach 80 % of the column from the
/// right, an incoming bubble (a person's or an agent's) 80 % of the slot after
/// the sender lane from the left. The 20 % that stays free on the far side
/// keeps a bubble reading as mine / theirs, and leaves the hover rail its
/// room. Grok's 560 pt ceiling stays an agent-chat rule: Grok has no group
/// chats to measure, and a group bubble should not be narrower than a
/// person's beside it (560 vs 655 at the 960 column before this).
pub const PEOPLE_BUBBLE_FRACTION: f32 = 0.80;
/// The share of its parent my own bubble may take.
pub fn own_bubble_fraction(people: bool) -> f32 {
    if people {
        PEOPLE_BUBBLE_FRACTION
    } else {
        OWN_BUBBLE_FRACTION
    }
}
/// The widest an incoming bubble in a people chat can be in a slot
/// `slot_width` wide (the row after the sender lane) — what [`people_slot`]
/// draws, for the layout tests.
#[cfg(test)]
pub(crate) fn people_cap(slot_width: f32) -> f32 {
    slot_width * PEOPLE_BUBBLE_FRACTION
}
/// A new time stamp line whenever messages are this far apart.
const STAMP_GAP_MS: u64 = 60 * 60 * 1000;
/// Grouped messages (Jacob, 2026-10-28, [feedback grouped-messages-mac-2026-10-28]):
/// consecutive messages from one sender this close together form one **run** —
/// the name and the disc show once, on the run's first message; the later
/// messages sit under its text with the avatar column empty and the 6 pt
/// in-run gap between them. The run breaks when the sender changes, when this
/// window passes between two messages, at a date stamp or a system line, and
/// at a bubble that carries a reply quote (it starts a run of its own). People
/// and agents group alike, my own messages too. Mirrored by the iPhone's
/// `Transcript.runWindowMs` (the row builders live in the two clients, not in
/// `arbos_core`): keep the two in step.
pub const RUN_WINDOW_MS: u64 = 2 * 60 * 1000;
/// Vertical rhythm measured from Grok Bot's Mac app (parity it.36, 2 px/pt):
/// consecutive bubbles of one run sit 6 pt apart, a new run opens 20 pt, a time
/// stamp has 20 pt above and 24 below, and the last message keeps 54 pt clear
/// of the composer.
pub(crate) const ROW_GAP_HALF: f32 = 3.0;
pub(crate) const TURN_GAP_EXTRA: f32 = 20.0 - 2.0 * ROW_GAP_HALF;
/// The gap between two bubbles of one run, and the gap a new run opens — the
/// numbers the rhythm tests pin.
#[cfg(test)]
pub(crate) const RUN_GAP: f32 = 2.0 * ROW_GAP_HALF;
#[cfg(test)]
pub(crate) const TURN_GAP: f32 = TURN_GAP_EXTRA + 2.0 * ROW_GAP_HALF;

/// The tail of the sender run the row builder is in: who spoke last and when
/// (`created_at`, ms). `None` after a date stamp, a system line or a hidden fold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunTail {
    pub sender: String,
    pub at: u64,
}

/// Whether a message from `sender` at `at` continues the run `tail` ends: the
/// same sender, within [`RUN_WINDOW_MS`] of the run's last message, and no
/// reply quote on the new bubble (`quoted`). Rows without a clock never group.
pub(crate) fn continues_run(tail: Option<&RunTail>, sender: &str, at: u64, quoted: bool) -> bool {
    let Some(tail) = tail else {
        return false;
    };
    !quoted
        && tail.sender == sender
        && at > 0
        && tail.at > 0
        && at.abs_diff(tail.at) <= RUN_WINDOW_MS
}

/// Whether the message's bubble draws a reply quote — a person's `replyRef` /
/// `replyTo`; an agent's `replyTo`, else `inReplyTo` in an attributed chat (a
/// people-chat reply or another Project's row, the fallback the row builder
/// uses). A quoted bubble names its sender and starts its own run.
fn carries_quote(message: &Message) -> bool {
    let non_empty = |id: Option<&str>| id.is_some_and(|id| !id.is_empty());
    match message.role {
        Role::User => {
            message
                .reply_ref
                .as_ref()
                .is_some_and(|r| !r.chat_id.is_empty() && !r.message_id.is_empty())
                || non_empty(message.reply_to.as_deref())
        }
        Role::Assistant | Role::Project => {
            let in_people =
                message.sender.starts_with("agent:") && message.asked_by_handle.is_some();
            let attributed = message.role == Role::Project || in_people;
            non_empty(message.reply_to.as_deref())
                || (attributed && non_empty(message.in_reply_to.as_deref()))
        }
        Role::System => false,
    }
}
/// Measured glyph-to-bubble, like the reference: the stamp's 13 pt line box
/// carries ~4 pt of its own above and ~7 below the glyphs (measured).
const STAMP_PAD_TOP: f32 = 20.0 - ROW_GAP_HALF - 4.0;
const STAMP_PAD_BOTTOM: f32 = 24.0 - ROW_GAP_HALF - 7.0;
/// Markdown rhythm inside a bubble, measured on Jacob's Grok Bot shot of
/// 2026-10-24 (`grok-chat-1.png`, 2 px/pt; [feedback bubble-markdown-2026-10-24]):
/// list items sit on a 26 pt pitch and blocks (paragraph → heading → list →
/// paragraph) on a 32 pt pitch over Grok's 22 pt text line
/// (`CHAT_LINE_HEIGHT`) — 4 pt between items, 10 between blocks (5 above
/// and 5 below each block row). Arbos had 3 and 6 on a 20 pt line.
const LIST_ITEM_GAP: f32 = 4.0;
const BLOCK_GAP_HALF: f32 = 5.0;
/// Grok's bullet is a 4.5 pt disc in the muted text colour, its centre 8.5 pt
/// from the block's left edge, the item text starting at 20 pt (Arbos drew the
/// font's "•" — a 3 pt dot 2 pt in — with the text at 24).
const LIST_INDENT: f32 = 20.0;
/// Two-digit numbered markers ("10.") need a wider gutter.
const LIST_INDENT_WIDE: f32 = 26.0;
const BULLET_DOT: f32 = 4.5;
const BULLET_CENTRE: f32 = 8.5;
/// Clearance above the composer when pinned (54 pt glyph-to-pill, minus the
/// composer row's own 6 pt top padding and the last row's half-gap). Kept as a
/// trailing list row — not as container padding — so scrolling bubbles reach
/// the real edge of the input instead of hitting an opaque black strip. The
/// presence row (who is typing / working) floats inside this clearance while
/// the list is pinned (`presence::LIFT`), so it never adds to it.
const BOTTOM_INSET: f32 = 54.0 - ROW_GAP_HALF - 6.0;
/// The last bubble's bottom edge → the composer pill's top edge when pinned.
#[cfg(test)]
pub(crate) const LAST_BUBBLE_TO_COMPOSER: f32 = ROW_GAP_HALF + BOTTOM_INSET + 6.0;
/// People chats (redesign mocks a/f): a 24 pt sender disc at the bubble's
/// bottom-left, 8 pt from it; captions and earlier blocks keep the lane free.
/// The avatar column (Jacob, 2026-10-23 15:20, [feedback
/// typing-indicator-left-2026-10-23]): every round picture in the chat — the
/// sender disc beside a people-chat bubble, the pictures on the presence row
/// over the composer — is the composer's `+` disc: the same size, centred on
/// it. `AVATAR_COLUMN_INSET` is the pill's rim + pad (1 + 7) from the chat
/// column's left edge (= the composer's left edge, #1374), so the disc spans
/// the `+` exactly (24–52 from the pane, centre 38). `SENDER_DISC_GAP` lands
/// the bubble body on the composer's text start (rim 1 + pad 7 + `+` 28 + 4 +
/// 10 = 50 from the column edge, 66 from the pane). Agent chats keep #1374's
/// no-avatar rule (body at the composer edge). Shared with
/// `feedback/sender-name-in-bubble-2026-10-23`, which builds on these names.
pub const AVATAR_COLUMN_INSET: f32 = 1.0 + (COMPOSER_HEIGHT - 2.0 - PLUS_BUTTON) / 2.0;
pub const SENDER_DISC: f32 = PLUS_BUTTON;
pub const SENDER_DISC_GAP: f32 = 14.0;
pub const SENDER_LANE: f32 = AVATAR_COLUMN_INSET + SENDER_DISC + SENDER_DISC_GAP;
/// Messages for Mac fades a row's hover-only affordances (time, react glyph)
/// in and out rather than snapping them.
const HOVER_FADE_MS: f32 = 150.0;
/// The fade band under the floating header (one number for every chat kind).
pub const HEADER_FADE_BAND: f32 = crate::ui::header::metrics::FADE_BAND;
/// How long the hover copy glyph stays a checkmark after a successful copy.
const COPY_FEEDBACK_MS: u64 = 2_000;
/// Ignore pointer noise until a click-drag travels this far before it is a
/// scroll (iPhone minimumDistance).
const DRAG_SCROLL_SLOP: f32 = 12.0;

/// Which bubble the pointer is over, and the one it just left, with the
/// moment each happened: the fade is computed from those at render time.
///
/// Keyed by **bubble**, not by transcript row: an agent reply is one row per
/// markdown block, and Grok shows the rail while the pointer is anywhere on
/// the bubble (Jacob, 2026-10-24, [feedback bubble-hover-rail-2026-10-24]).
/// `rows` remembers which rows of a bubble the pointer is over right now, so
/// crossing from one block to the next — whichever order GPUI reports the
/// leave and the enter — never fades the rail out.
#[derive(Default)]
struct HoverFade {
    over: Option<(String, Instant)>,
    left: Option<(String, Instant)>,
    rows: HashMap<String, HashSet<String>>,
}

/// Click-drag vertical pan that scrolls the list (iPhone ScrollView parity).
/// Armed on a press that is not on bubble text (a press on text belongs to
/// the selection, whichever way it then moves); locks once the pan is
/// clearly vertical. There is no swipe-to-reply on the Mac: dragging a
/// message must never turn it into a reply (Jacob, 2026-10-24, [feedback
/// bubble-selection-2026-10-24]); the iPhone keeps its own gesture.
struct DragScroll {
    start: Point<Pixels>,
    last: Point<Pixels>,
    locked: bool,
}

impl HoverFade {
    /// Record that the pointer entered or left `row_id`, one of the rows of
    /// bubble `key`; `true` when the bubble's hover state changed.
    fn set(&mut self, key: &str, row_id: &str, hovered: bool, now: Instant) -> bool {
        let rows = self.rows.entry(key.to_string()).or_default();
        if hovered {
            rows.insert(row_id.to_string());
        } else {
            rows.remove(row_id);
        }
        let hovered = !rows.is_empty();
        if !hovered {
            self.rows.remove(key);
        }
        let is_over = self.over.as_ref().is_some_and(|(id, _)| id == key);
        if hovered == is_over {
            return false;
        }
        if hovered {
            if let Some((prev, _)) = self.over.take() {
                self.left = Some((prev, now));
            }
            // Re-entering mid-fade starts the fade-in from the current level.
            let head_start = match &self.left {
                Some((id, at)) if id == key => {
                    let remaining = 1.0 - Self::progress(*at, now);
                    Duration::from_secs_f32(remaining * HOVER_FADE_MS / 1000.0)
                }
                _ => Duration::ZERO,
            };
            self.left = None;
            self.over = Some((key.to_string(), now - head_start));
        } else {
            self.over = None;
            self.left = Some((key.to_string(), now));
        }
        true
    }

    fn progress(since: Instant, now: Instant) -> f32 {
        (now.saturating_duration_since(since).as_millis() as f32 / HOVER_FADE_MS).clamp(0.0, 1.0)
    }

    /// Opacity for `row_id`'s affordances now, and whether it is still moving.
    fn opacity(&self, row_id: &str, now: Instant) -> (f32, bool) {
        match (&self.over, &self.left) {
            (Some((id, at)), _) if id == row_id => {
                let t = Self::progress(*at, now);
                (crate::ui::motion::ease_out_cubic(t), t < 1.0)
            }
            (_, Some((id, at))) if id == row_id => {
                let t = Self::progress(*at, now);
                (1.0 - crate::ui::motion::ease_out_cubic(t), t < 1.0)
            }
            _ => (0.0, false),
        }
    }
}

/// Grok's quote header inside a reply bubble: speech-bubble glyph, then the
/// replied-to message's first words in quotes. Clicking it reveals the original.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Quote {
    /// The message replied to (`replyTo` on the row, or `replyRef.messageId`).
    pub message_id: Id,
    /// Its first line, cut to [`QUOTE_CHARS`] characters with an ellipsis.
    pub text: String,
    /// Reply in DM (#1835): the original lives in another chat — its address
    /// and the "Replying to <who> in <chat>" line. `None` for an in-chat reply.
    pub source: Option<QuoteSource>,
}

/// Where a cross-chat quote came from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct QuoteSource {
    pub chat_id: Id,
    /// "Replying to Jacob in Arbonites".
    pub label: String,
}

/// The quote header shows this many characters of the original before "…".
pub const QUOTE_CHARS: usize = 64;

impl Quote {
    /// The quote for a message that replies to `reply_to`; a reply whose original is not
    /// loaded (older than the snapshot) still shows a header so the reference is never lost.
    pub fn for_reply(reply_to: &str, messages: &[Message]) -> Self {
        let text = messages
            .iter()
            .find(|m| m.id == reply_to)
            .map(quote_text)
            .unwrap_or_else(|| "Earlier message".to_owned());
        Self {
            message_id: reply_to.to_owned(),
            text,
            source: None,
        }
    }

    /// The quote for a cross-chat reply: drawn from the row's own snapshot, so
    /// it needs nothing from the source chat.
    pub fn for_reply_ref(r: &ReplyRef) -> Self {
        let text = if r.text.trim().is_empty() {
            "Message".to_owned()
        } else {
            first_words(&one_line(&r.text), QUOTE_CHARS)
        };
        Self {
            message_id: r.message_id.clone(),
            text,
            source: Some(QuoteSource {
                chat_id: r.chat_id.clone(),
                label: reply_ref_label(r),
            }),
        }
    }
}

/// "Replying to Jacob in Arbonites" — what a cross-chat quote says above the text.
pub fn reply_ref_label(r: &ReplyRef) -> String {
    let place = r.chat_title.trim();
    if place.is_empty() {
        format!("Replying to {}", r.who())
    } else {
        format!("Replying to {} in {place}", r.who())
    }
}

/// A whole message on one line, for quote headers and the composer's reply
/// preview: markdown blocks (headings, bullets, fences, tables) and inline
/// markers (`**`, `` ` ``, links) are dropped, every newline becomes a space,
/// whitespace is collapsed. Attachments are not part of it — callers name the
/// file when there is no prose.
pub fn one_line(text: &str) -> String {
    let mut out = String::new();
    one_line_blocks(split_blocks(text), &mut out);
    out
}

fn one_line_blocks(blocks: Vec<Block>, out: &mut String) {
    for block in blocks {
        let pieces: Vec<String> = match block {
            Block::Paragraph(text) | Block::Heading { text, .. } => vec![text],
            Block::Quote(inner) => {
                one_line_blocks(inner, out);
                continue;
            }
            Block::Bullets(items) | Block::Numbered(items) => items,
            Block::Code { code, .. } => vec![code],
            Block::Table { header, rows } => header
                .into_iter()
                .chain(rows.into_iter().flatten())
                .collect(),
            Block::Math { source } => vec![math::plain(&source)],
            Block::Chart { spec, .. } => vec![crate::ui::chart_card::one_line_title(&spec)],
            Block::LinkPreview { preview, .. } => vec![preview.one_line()],
        };
        for piece in pieces {
            // Spans are joined before splitting so "`site/launch/`;" stays one
            // word — a span boundary is a style change, not a space.
            let plain: String = inline_spans(&piece).into_iter().map(|s| s.text).collect();
            for word in plain.split_whitespace() {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(word);
            }
        }
    }
}

/// `line` cut to `max_chars` on a word boundary with an ellipsis; shorter
/// lines come back as they are.
pub fn first_words(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_owned();
    }
    let mut cut: String = line.chars().take(max_chars).collect();
    if let Some(ix) = cut.rfind(' ') {
        if ix > max_chars / 2 {
            cut.truncate(ix);
        }
    }
    format!("{}…", cut.trim_end_matches([',', ';', ':', '.']))
}

/// The message flattened to one line ([`one_line`]) and cut to `max_chars`;
/// an attachment-only message names its file, an empty one says "Message".
pub fn message_line(message: &Message, max_chars: usize) -> String {
    let line = one_line(&message.plain_text());
    let line = if line.is_empty() {
        match message.parts.iter().find_map(|p| match p {
            Part::Attachment { attachment, .. } => Some(attachment.name.clone()),
            _ => None,
        }) {
            Some(name) => name,
            None => return "Message".to_owned(),
        }
    } else {
        line
    };
    first_words(&line, max_chars)
}

/// The forwarded bubble's header (Telegram / iMessage): forward glyph, then
/// "Forwarded from <name>", one line, in the quote header's muted colour.
pub(crate) fn forwarded_header(by: String, text_color: gpui::Hsla) -> Stateful<Div> {
    let muted = Theme::with_alpha(text_color, 0.72);
    div()
        .id(SharedString::from(format!("forwarded-from-{by}")))
        .flex()
        .items_center()
        .gap(px(6.0))
        .min_w_0()
        .child(sf_symbols::glyph("arrowshape.turn.up.right", 12.0, muted))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(TEXT_SMALL))
                .font_weight(FontWeight::MEDIUM)
                .line_height(px(LINE_HEIGHT - 4.0))
                .text_color(muted)
                .child(format!("{}{by}", crate::app::FORWARDED_FROM_PREFIX)),
        )
}

/// First words of a message for its quote header: the whole message on one
/// line (newlines and markdown flattened), cut on a word boundary; attachments
/// name their file.
pub fn quote_text(message: &Message) -> String {
    message_line(message, QUOTE_CHARS)
}

/// Who sent an attachment row that is not mine, for the avatar column and the name
/// (the same identity the message's bubble resolves).
#[derive(Clone, Debug)]
pub enum AttachmentSender {
    /// Another person in a people chat.
    Person(Person),
    /// An agent in a people chat (or another Project): its `sender` seed, resolved
    /// at render time like a bubble's lane.
    Agent { seed: String },
}

#[derive(Clone, Debug)]
pub enum RowKind {
    User {
        text: String,
        /// Caption for prompts not typed by the user (a routine, another project).
        sender_label: Option<String>,
        /// Another person's bubble carries their name inside it, at the top, when
        /// the chat names its senders (group / channel, see [`names_senders`]) and
        /// the message starts a run (Grok Bot's group style for the run).
        named: bool,
        /// Another person in a people chat (D72): left bubble with their
        /// avatar and handle instead of my right bubble.
        from: Option<Person>,
        /// Draw `from`'s disc on the avatar column: the run's first message only; a
        /// message that continues the run keeps the column empty
        /// ([feedback grouped-messages-mac-2026-10-28]).
        disc: bool,
        /// "9:41 PM", revealed at the bubble's outer edge on hover only.
        time: Option<String>,
        /// My own messages: pending → failed / sent → seen, shown as the
        /// bubble's opacity (plus a red glyph when failed). `None` on others'
        /// bubbles.
        delivery: Option<Delivery>,
        /// Set when this message replies to another: Grok's quote header
        /// sits inside the bubble above the text.
        quote: Option<Quote>,
        /// Bubble menu → Forward: "Forwarded from <name>" header above the
        /// text (feedback app-01M5MXRZ7609S70P9S9P2RZDDF).
        forwarded_from: Option<String>,
        /// Secret hand-off: the message's value moved into the vault and the row is the
        /// "Secret saved as NAME" ghost — muted fill, key glyph, no text to select or copy.
        /// The secret's name.
        ghost: Option<String>,
        /// The relay's link preview for this message, drawn as a card under the text
        /// (or as the whole bubble when the text was only the link): `(part id, preview)`
        /// ([feedback link-embeds-mac-2026-10-27]).
        preview: Option<Box<(String, LinkPreview)>>,
    },
    Markdown {
        block: Block,
        live: bool,
        /// The whole bubble's rendered plain text and where this block's text
        /// starts in it: text selection runs through every block of a message
        /// as one range (#1389).
        bubble: BubbleText,
        /// Where this block sits in its message's bubble (first/last round
        /// the corners; `fill` makes a multi-block bubble span the cap width
        /// so every block has the same edge).
        first: bool,
        last: bool,
        fill: bool,
        /// "9:41 PM", revealed in the hover rail of the last block on hover only.
        time: Option<String>,
        /// In a people chat the message has a sender row above it: every block
        /// leaves the avatar lane free and the last one draws the sender's disc
        /// at the bubble's bottom-left (redesign mocks a/f). The sender seed.
        lane: Option<String>,
        /// People / group reply: quote of the user message this answers
        /// (`replyTo`, else `inReplyTo`). Only set on the first block of the bubble.
        quote: Option<Quote>,
        /// First block of a message whose sender is named, in a chat that names
        /// its senders: the agent's name sits inside this bubble, at the top.
        named: bool,
        /// The message starts its run: its last block draws the disc. A message
        /// that continues the run keeps the lane empty on every block.
        disc: bool,
    },
    /// A finished reply with no words, cards or attachments: one plain line in the
    /// agent's bubble. `name` is the row's `senderName`; the agent id in `seed` resolves
    /// the name when the row carries none (older boxes), like the sender header does.
    NoAnswer {
        seed: String,
        name: Option<String>,
        time: Option<String>,
        lane: Option<String>,
        /// People / group reply: quote of the user message this would have answered.
        quote: Option<Quote>,
        named: bool,
        /// The message starts its run (draws the disc), as on a `Markdown` row.
        disc: bool,
    },
    /// Centered muted time line ("Today 9:39 PM").
    Stamp {
        label: String,
    },
    /// A run of messages from someone I blocked, folded into one muted line ("3 messages
    /// hidden because you blocked @sam · Unblock") — the blocker alone sees this; everyone
    /// else sees the messages ([feedback blocked-state-mac-2026-10-26], Jacob 2026-10-26).
    Hidden {
        count: usize,
        /// The sender key (`user:<id>`) and what the row knows of them.
        sender: String,
        handle: String,
        name: Option<String>,
    },
    /// Emoji reaction chips under a bubble (iPhone parity): `(emoji, senders)`.
    Reactions {
        reactions: Vec<(String, Vec<String>)>,
        /// Under my right-hand bubble (chips align right).
        mine: bool,
        /// The bubble sits after the sender's avatar lane (people chats: another
        /// person, or an agent in a group), so the chips start at its left edge.
        lane: bool,
    },
    /// Grok's hand-off line: "Messaged ◉ A" / "N messages with ◉ A, ◉ B and ◉ C".
    Handoff {
        names: Vec<String>,
        total: u32,
    },
    /// Agent-to-agent messaging: this reply went back to the agent that wrote in —
    /// "Replied to ◉ hjkb", the same line style as a hand-off.
    RepliedTo {
        name: String,
    },
    /// An image or file in the chat (agent or user). `mine` right-aligns my sends.
    /// A picture is a message like any other ([feedback forward-attachments-mac-2026-10-27],
    /// Jacob's Scout case): someone else's carries their disc on the avatar column and,
    /// when the chat names its senders and the sender changed, their name above the card.
    Attachment {
        attachment: Attachment,
        /// The `parts[]` id: the viewer opens on this picture of the album.
        part_id: Id,
        mine: bool,
        /// Who sent it when it is not mine in a people chat: the disc beside the card
        /// (the message's bubble has the same one). `None` = my own, or an agent's in
        /// its own 1:1 chat (no avatar column there).
        sender: Option<AttachmentSender>,
        /// The sender's name above the card (first card of the message only; the
        /// message's text bubble, if any, then carries no name).
        named: bool,
        /// Draw the disc: on the last row of a message that starts its run — its text
        /// bubble when it has one, else its last card; earlier cards and every card of
        /// a message that continues the run keep the lane free.
        disc: bool,
        /// The message's clock label, hover-only like a bubble's.
        time: Option<String>,
        /// The message's "Forwarded from …" line, drawn on the card.
        forwarded_from: Option<String>,
        /// A reply's quote header above the first card of the message — a picture
        /// reply quotes like a text one ([feedback picture-reply-quote-mac-2026-10-28]);
        /// the caption bubble under it then carries none.
        quote: Option<Quote>,
    },
    /// S8: the agent asked a question.
    AskUser {
        text: String,
    },
    /// S18: draft-and-approve card.
    Approval {
        view: ApprovalView,
        /// People chat: the card sits after the sender's avatar lane, like its bubble.
        lane: bool,
    },
    /// Error: one line, red glyph, Retry.
    /// A typed question widget (design §10): rendered by a [`QuestionCard`] view.
    Question {
        question_id: String,
        /// People chat: the card sits after the sender's avatar lane, like its bubble.
        lane: bool,
    },
    Error {
        code: String,
        message: String,
    },
    /// Tiny inline glyph (■ stopped, ✗ failed) — no words.
    Glyph {
        glyph: &'static str,
    },
    /// Debug-only "Show details" disclosure under a finished reply.
    Details,
    /// F11: small speaker under a reply; click speaks it (again).
    Speaker {
        /// The speech is already cached (instant replay) vs. will be synthesised.
        cached: bool,
        /// The last attempt to speak this reply failed; the row is a retry.
        failed: bool,
    },
    /// Empty height at the end of the list so the last bubble sits clear of
    /// the composer when pinned; scrolls away so mid-thread bubbles can reach
    /// the input's real edge (no opaque padding strip).
    BottomInset,
}

/// Which replies get a speaker row (F11).
#[derive(Clone, Copy)]
pub struct SpeechRows<'a> {
    /// Relay flag `features.voice_mode`; off = no speaker rows at all.
    pub enabled: bool,
    pub spoken: &'a HashSet<Id>,
    /// Replies whose speech failed (reason): their speaker row offers a retry.
    pub failed: &'a HashMap<Id, String>,
    pub voice_mode: bool,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub id: String,
    pub version: u64,
    pub message_id: Id,
    pub kind: RowKind,
}

/// A block's place in its bubble's plain text (see [`bubble_plain`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BubbleText {
    /// The bubble's whole rendered plain text, every block in order.
    pub text: SharedString,
    /// Byte offset of this block's plain text inside `text`.
    pub at: usize,
}

impl BubbleText {
    /// A bubble made of this one block.
    pub fn single(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            at: 0,
        }
    }

    /// One entry per block: the shared bubble text and each block's offset.
    pub fn for_blocks(blocks: &[Block], literal: bool) -> Vec<Self> {
        let text: SharedString = bubble_plain(blocks, literal).into();
        block_offsets(blocks, literal)
            .into_iter()
            .map(|at| Self {
                text: text.clone(),
                at,
            })
            .collect()
    }
}

/// Stick-to-bottom velocity spring (zeron-study §4.3). Each frame it moves a
/// fraction of the remaining distance to the end, damped, so streamed text
/// glides instead of jumping; a user wheel-up breaks the pin.
#[derive(Default)]
struct StickSpring {
    velocity: f32,
}

const SPRING_STIFFNESS: f32 = 0.22;
const SPRING_DAMPING: f32 = 0.55;
pub const STICK_THRESHOLD_PX: f32 = 70.0;
/// How far above the end the user must be before the ↓ button shows.
const JUMP_BUTTON_AFTER_PX: f32 = 160.0;
/// ↓ from this close to the end (in viewport heights), with every row between
/// already laid out, glides on the spring; from further away it lands on the
/// newest page at once ([`Transcript::jump_to_latest`]).
const JUMP_GLIDE_MAX_VIEWPORTS: f32 = 2.5;
/// The landing: the newest page settles in over this long, rising
/// [`JUMP_ARRIVE_RISE_PX`] and fading from [`JUMP_ARRIVE_FROM_ALPHA`] — a
/// short "arrived" cue instead of a crawl, Grok Mac's own timing class.
const JUMP_ARRIVE_MS: u64 = 150;
const JUMP_ARRIVE_RISE_PX: f32 = 24.0;
const JUMP_ARRIVE_FROM_ALPHA: f32 = 0.35;

/// The hover rail's buttons: Grok Mac's 24 pt hit target, 16 pt glyph, 2 pt apart.
pub const RAIL_BUTTON: f32 = 24.0;
pub const RAIL_GLYPH: f32 = 16.0;
pub const RAIL_GAP: f32 = 2.0;
/// The Glass capsule's inset on every side of the rail's buttons. The same
/// inset all round keeps each button's round hover wash inside the capsule's
/// rim and concentric with its rounded ends ([feedback hover-bar-highlight-mac-2026-10-29]).
pub const RAIL_PLATE_PAD: f32 = 2.0;

/// A reaction chip under a bubble (iPhone `reactionChips`: 14 pt emoji, 12 pt
/// medium count, 8 pt side pads, 4 pt vertical pads, 1 pt rim). The height is
/// fixed rather than derived from the emoji's line box: the glyph comes from
/// a fallback font whose metrics must never size the chip, and nothing about
/// the pointer may change the box ([feedback reaction-pill-hover-jitter-2026-10-28]).
pub const REACTION_CHIP_HEIGHT: f32 = 26.0;
pub const REACTION_CHIP_PAD_X: f32 = 8.0;
pub const REACTION_CHIP_EMOJI: f32 = 14.0;
pub const REACTION_CHIP_COUNT: f32 = 12.0;

/// Right-click or the rail's ⋯: the shell opens the message menu there.
/// The rail's smiley: the shell opens the reaction capsule there.
/// Hover reply: the shell sets the composer's reply target.
pub enum TranscriptEvent {
    MessageMenu {
        chat_id: Id,
        message_id: Id,
        position: gpui::Point<gpui::Pixels>,
    },
    ReactionCapsule {
        chat_id: Id,
        message_id: Id,
        position: gpui::Point<gpui::Pixels>,
    },
    Reply {
        chat_id: Id,
        message_id: Id,
    },
    /// The hidden-run line's Unblock link ([feedback blocked-state-mac-2026-10-26]): the
    /// shell opens its Unblock confirm for this person.
    Unblock {
        person: Person,
    },
}

impl gpui::EventEmitter<TranscriptEvent> for Transcript {}

pub struct Transcript {
    state: Entity<AppState>,
    chat_id: Option<Id>,
    list: ListState,
    pub(crate) rows: Vec<Row>,
    /// Per-message built rows (#1405 / feedback app-01M5Z3QPDA5BRE84SAPADXX05V,
    /// addendum app-01M5Z3T4F4KAMTWA7YKW3CP58H): a send or streaming upsert
    /// rebuilds one message, not the whole transcript — keeps scrolling and
    /// reply/menu gestures responsive after send (Mac has no swipe-to-reply).
    row_cache: MessageRowCache,
    /// Who `@handle` resolves to in this chat's bubbles (mention chips),
    /// rebuilt with the rows.
    mentions: Rc<Mentions>,
    /// Site pictures for the bubbles' named links (`AppState::link_icons`).
    link_icons: Rc<LinkIcons>,
    /// Whether the right pane was open at the last sync: its opening or
    /// closing changes the column width, and the cached row layouts must go.
    pane_open: bool,
    veils: HashMap<String, Veil>,
    /// One stateful card per typed question, keyed by question id; dropped with its row.
    cards: HashMap<String, Entity<QuestionCard>>,
    /// Secret hand-off request cards by approval id (the masked-field card keeps state).
    secret_cards: HashMap<String, Entity<SecretOfferCard>>,
    pinned_to_bottom: bool,
    /// When ↓ last landed on the newest page from far away: drives the
    /// [`JUMP_ARRIVE_MS`] settle of the list (rise + fade), then cleared.
    arrived_at: Option<Instant>,
    /// Row layouts the list has asked for, in total (every `render_row`
    /// call: visible rows and the overdraw). The budget the jump test holds.
    pub(crate) rows_laid_out: u64,
    /// Reveal request (`AppState::reveal_nonce`) we already scrolled for.
    revealed: Option<u64>,
    spring: StickSpring,
    /// Set by the scroll callback; the next frame re-pins when the user
    /// stopped within `STICK_THRESHOLD_PX` of the end.
    repin_check_pending: bool,
    hover: HoverFade,
    /// Text selected in a bubble (drag / double-click / triple-click); ⌘C copies it.
    selection: Entity<TextSelection>,
    /// Messages whose sender differs from the one before: their first row opens
    /// the wider turn gap.
    turn_starts: HashSet<Id>,
    /// The open chat names its senders (group / channel, [`names_senders`]):
    /// others' bubbles carry the name inside and the time after the text.
    multi_party: bool,
    /// The open chat is a people chat (DM / group / channel / shared project):
    /// bubbles take [`PEOPLE_BUBBLE_FRACTION`] of their side ([`own_bubble_fraction`]).
    people: bool,
    /// Message whose hover copy glyph is showing a checkmark, and when it started.
    copied_feedback: Option<(Id, Instant)>,
    /// The code card Copy pill reading "Copied" right now (its id, when).
    code_copied: Option<(SharedString, Instant)>,
    /// `ARBOS_RIG_SELECT=all|<from>..<to>|word@<i>|para@<i>` (screenshot rig):
    /// once the open chat shows a settled agent bubble, select that much of
    /// the newest one — the drag / double-click / triple-click the EC2 Mac's
    /// GUI session cannot receive from posted events. Not before
    /// `ARBOS_RIG_SELECT_AFTER_MS` (so a reply typed over VNC can land
    /// first). Cleared when applied.
    rig_select: Option<(String, Instant)>,
    /// Vertical click-drag scroll over bubbles / empty rows.
    drag_scroll: Option<DragScroll>,
    /// Where every painted row and the list's viewport were in the last
    /// frame, recorded during prepaint. The rail of a multi-row bubble is
    /// placed from these: the list's own state cannot be queried from inside
    /// its render (`ListState::bounds_for_item` borrows the `RefCell` the
    /// list holds while it lays its items out — v0.6.224 crashed on launch,
    /// [feedback bubble-rail-crash-hotfix-2026-10-25]).
    measured: Rc<std::cell::RefCell<Measured>>,
}

/// Last-frame geometry, by row id, plus the transcript's viewport.
#[derive(Default)]
struct Measured {
    rows: HashMap<String, Bounds<Pixels>>,
    viewport: Bounds<Pixels>,
}

/// Hover tip on a reaction chip: who used that emoji (iPhone `reactionSenders`).
/// Floats above the list so the row's height never changes on hover.
struct ReactionSendersTip(SharedString);

impl Render for ReactionSendersTip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .bg(theme.surface_2)
            .border_1()
            .border_color(theme.border)
            .text_size(px(TEXT_TINY))
            .text_color(theme.text)
            .child(self.0.clone())
    }
}

impl Transcript {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Bottom, px(OVERDRAW_PX));
        let weak = cx.weak_entity();
        list.set_scroll_handler(move |event: &ListScrollEvent, _, cx| {
            let _ = weak.update(cx, |this, _| this.handle_scroll(event));
        });
        let selection = cx.new(TextSelection::new);
        cx.observe(&selection, |_, _, cx| cx.notify()).detach();
        cx.observe(&state, |this, _, cx| this.sync(cx)).detach();
        cx.subscribe(&state, |this, _, event: &AppEvent, cx| match event {
            AppEvent::TranscriptText { chat_id, .. } | AppEvent::TranscriptChanged { chat_id } => {
                if this.chat_id.as_deref() == Some(chat_id.as_str()) {
                    this.sync(cx);
                }
            }
            AppEvent::VoiceTranscript { .. }
            | AppEvent::VoiceSpeech { .. }
            | AppEvent::ReplaySpeech { .. } => {}
        })
        .detach();
        Self {
            state,
            chat_id: None,
            list,
            rows: Vec::new(),
            row_cache: MessageRowCache::default(),
            mentions: Mentions::empty(),
            link_icons: LinkIcons::empty(),
            pane_open: false,
            veils: HashMap::new(),
            cards: HashMap::new(),
            secret_cards: HashMap::new(),
            pinned_to_bottom: true,
            arrived_at: None,
            rows_laid_out: 0,
            revealed: None,
            spring: StickSpring::default(),
            repin_check_pending: false,
            hover: HoverFade::default(),
            selection,
            turn_starts: HashSet::new(),
            multi_party: false,
            people: false,
            copied_feedback: None,
            code_copied: None,
            rig_select: std::env::var("ARBOS_RIG_SELECT").ok().map(|spec| {
                let after = std::env::var("ARBOS_RIG_SELECT_AFTER_MS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(after + 50))
                        .await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
                (spec, Instant::now() + Duration::from_millis(after))
            }),
            drag_scroll: None,
            measured: Rc::default(),
        }
    }

    /// Apply `ARBOS_RIG_SELECT` to the newest settled agent bubble, once.
    /// `across` selects from the second word of the bubble before it to the
    /// second-to-last word of that bubble — a drag across two messages.
    fn apply_rig_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((spec, not_before)) = self.rig_select.clone() else {
            return;
        };
        if Instant::now() < not_before {
            return;
        }
        if spec.trim() == "across" {
            let bubbles = self.bubble_texts(cx);
            let n = bubbles.len();
            if n < 2 {
                return;
            }
            let at = |(order, key, text): &(usize, String, String)| BubbleAt {
                order: *order,
                key: key.clone().into(),
                text: text.clone().into(),
            };
            let (from, to) = (at(&bubbles[n - 2]), at(&bubbles[n - 1]));
            self.rig_select = None;
            let focus = self.selection.read(cx).focus_handle.clone();
            self.selection.update(cx, |s, cx| {
                s.rig_select_across(from, to);
                cx.notify();
            });
            window.focus(&focus, cx);
            return;
        }
        let target = self.rows.iter().rev().find_map(|row| match &row.kind {
            RowKind::Markdown {
                bubble,
                live: false,
                ..
            } => Some((row.message_id.clone(), bubble.text.clone())),
            _ => None,
        });
        let Some((message_id, text)) = target else {
            return;
        };
        self.rig_select = None;
        let focus = self.selection.read(cx).focus_handle.clone();
        let applied = self.selection.update(cx, |s, cx| {
            let ok = s.rig_select(format!("{message_id}#text").into(), text, &spec);
            cx.notify();
            ok
        });
        if applied {
            window.focus(&focus, cx);
        }
    }

    /// The transcript order of the bubble row `ix` belongs to: the index of
    /// the message's first row, shared by every block row of one reply.
    fn message_order(&self, ix: usize) -> usize {
        let Some(row) = self.rows.get(ix) else {
            return ix;
        };
        let mut first = ix;
        while first > 0 && self.rows[first - 1].message_id == row.message_id {
            first -= 1;
        }
        first
    }

    /// The selectable bubbles of the transcript in order — `(order, key,
    /// plain text)`, the same key and text each row hands its
    /// `Selectable::bubble` — so a selection across bubbles can be copied with
    /// the bubbles in between, in transcript order.
    fn bubble_texts(&self, cx: &Context<Self>) -> Vec<(usize, String, String)> {
        let mut out: Vec<(usize, String, String)> = Vec::new();
        for (ix, row) in self.rows.iter().enumerate() {
            let order = self.message_order(ix);
            let entry = match &row.kind {
                RowKind::Markdown { bubble, .. } => {
                    Some((format!("{}#text", row.message_id), bubble.text.to_string()))
                }
                RowKind::NoAnswer { seed, name, .. } => {
                    let hint = name.as_deref().unwrap_or("Agent");
                    let (resolved, _, _, _) = self.agent_identity(seed, hint, cx);
                    let who = if resolved == "Agent" || resolved.trim().is_empty() {
                        "The agent".to_owned()
                    } else {
                        resolved
                    };
                    Some((
                        format!("{}#text", row.message_id),
                        format!("{who} didn't answer. Ask again."),
                    ))
                }
                RowKind::User { text, .. } => {
                    Some((format!("{}.u", row.id), user_bubble_plain(text)))
                }
                RowKind::Stamp { label } => Some((format!("{}.s", row.id), inline_plain(label))),
                _ => None,
            };
            if let Some((key, text)) = entry {
                // Block rows of one reply share one bubble: keep it once.
                if out.last().is_some_and(|(_, k, _)| *k == key) {
                    continue;
                }
                out.push((order, key, text));
            }
        }
        out
    }

    /// ⌘C with text selected: one bubble's slice, or — across bubbles — the
    /// tail of the first, every bubble in between whole, the head of the
    /// last, in transcript order whichever way the drag went, one blank line
    /// between messages (what Grok's copy gives).
    fn copy_selection(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let span = self.selection.read(cx).span();
        if let Some(message_id) = span
            .as_ref()
            .and_then(|span| self.whole_bubble_with_attachments(span, cx))
        {
            self.copy_message(&message_id, cx);
            return;
        }
        let text = {
            let selection = self.selection.read(cx);
            match span {
                Some(span) if !span.one_bubble() => copy_across(&span, &self.bubble_texts(cx)),
                _ => selection.selected_text(),
            }
        };
        if let Some(text) = text {
            // The URLs' wrap hints stay in the app.
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(urlwrap::plain(&text)));
        }
    }

    /// The row around a bubble: tracks the pointer so the time and react glyph
    /// can fade, and reports their opacity for this frame. With Reduce Motion
    /// the fade collapses to the snap it replaced.
    ///
    /// `key` names the bubble the row belongs to: every block row of one agent
    /// reply shares its message's key, so the rail shows and stays while the
    /// pointer is anywhere on the bubble; single-row bubbles use their row id.
    fn hover_row(
        &self,
        row_id: &str,
        key: &str,
        now: Instant,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> (Stateful<Div>, f32) {
        let (opacity, moving) = self.hover.opacity(key, now);
        let opacity = if cx.reduce_motion() {
            opacity.round()
        } else {
            if moving {
                window.request_animation_frame();
            }
            opacity
        };
        let (id, key) = (row_id.to_string(), key.to_string());
        let row = div()
            .id(SharedString::from(format!("{row_id}#row")))
            .w_full()
            .flex()
            .items_center()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if this.hover.set(&key, &id, *hovered, Instant::now()) {
                    cx.notify();
                }
            }));
        (row, opacity)
    }

    /// The hover key of a multi-block agent bubble (every `RowKind::Markdown`
    /// row of one message).
    fn bubble_hover_key(message_id: &Id) -> String {
        format!("{message_id}#bubble")
    }

    /// Where the hover rail of the bubble whose last block is row `ix` sits:
    /// the vertical centre of the whole bubble, in pixels from that row's
    /// top. `None` for a one-row bubble (its row centres the rail itself) or
    /// before the list has laid the rows out.
    fn bubble_rail_centre(&self, ix: usize) -> Option<Pixels> {
        let message_id = &self.rows.get(ix)?.message_id;
        let is_block = |i: usize| {
            self.rows.get(i).is_some_and(|r| {
                r.message_id == *message_id && matches!(r.kind, RowKind::Markdown { .. })
            })
        };
        let mut first = ix;
        while first > 0 && is_block(first - 1) {
            first -= 1;
        }
        if first == ix {
            return None;
        }
        let measured = self.measured.borrow();
        let last = *measured.rows.get(&self.rows[ix].id)?;
        let rows: Vec<Option<Bounds<Pixels>>> = (first..ix)
            .map(|i| measured.rows.get(&self.rows[i].id).copied())
            .collect();
        let top_inset = ROW_GAP_HALF
            + if self.is_turn_start(first) {
                TURN_GAP_EXTRA
            } else {
                0.0
            };
        Some(rail_centre(
            &rows,
            last,
            measured.viewport,
            (px(top_inset), px(ROW_GAP_HALF)),
        ))
    }

    /// An invisible, out-of-flow child that records where its parent was
    /// painted this frame (runs in prepaint with the parent's bounds): a row
    /// under `Some(row id)`, the transcript's viewport under `None`.
    fn measure_into(
        measured: &Rc<std::cell::RefCell<Measured>>,
        key: Option<String>,
    ) -> impl IntoElement {
        let measured = measured.clone();
        canvas(
            move |bounds, _, _| {
                let mut m = measured.borrow_mut();
                match key {
                    Some(id) => {
                        m.rows.insert(id, bounds);
                    }
                    None => m.viewport = bounds,
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }

    /// The list sits at its end (new rows glide into view). The shell draws the
    /// presence row over the bottom clearance only then: scrolled into history,
    /// bubbles pass through that band and the ↓ button takes it.
    pub fn is_pinned_to_bottom(&self) -> bool {
        self.pinned_to_bottom
    }

    /// Where the list drew row `row_id` in the last frame (its whole frame:
    /// the bubble plus its gap padding), if it was on screen.
    #[cfg(test)]
    pub(crate) fn row_frame(&self, row_id: &str) -> Option<Bounds<Pixels>> {
        self.measured.borrow().rows.get(row_id).copied()
    }

    /// The bubble the pointer is over right now (its hover key) — what the
    /// hover rail and the time are drawn for. `None` when nothing is hovered.
    #[cfg(test)]
    pub(crate) fn hovered_bubble(&self) -> Option<String> {
        self.hover.over.as_ref().map(|(key, _)| key.clone())
    }

    /// Where the list is scrolled to (item index, pixels into it), for a
    /// test to compare before and after an input.
    #[cfg(test)]
    pub(crate) fn scroll_position(&self) -> (usize, f32) {
        let top = self.list.logical_scroll_top();
        (top.item_ix, f32::from(top.offset_in_item))
    }

    /// The first row of a message from a new sender opens the turn gap; a
    /// stamp between them already carries its own spacing.
    fn is_turn_start(&self, ix: usize) -> bool {
        let Some(row) = self.rows.get(ix) else {
            return false;
        };
        ix.checked_sub(1)
            .and_then(|i| self.rows.get(i))
            .is_some_and(|prev| {
                prev.message_id != row.message_id
                    && !matches!(prev.kind, RowKind::Stamp { .. } | RowKind::BottomInset)
                    && self.turn_starts.contains(&row.message_id)
            })
    }

    /// Runs inside the list's own scroll callback while `ListState` is
    /// mutably borrowed, so it must not touch the list. Record the intent
    /// and let the next frame (`step_spring`) read pixel offsets.
    fn handle_scroll(&mut self, event: &ListScrollEvent) {
        // `is_scrolled == false` means the bottom-aligned list snapped to its
        // end: the user is back at the tail. `is_scrolled == true` means they
        // left that natural end (wheel, trackpad, or scrollbar). Unpin so a
        // new incoming row cannot spring them back while they read history —
        // the parent wheel listener alone was not enough (scrollbar / list
        // scroll paths never cleared the pin).
        if event.is_scrolled {
            self.pinned_to_bottom = false;
            self.spring.velocity = 0.0;
        } else {
            self.pinned_to_bottom = true;
            self.spring.velocity = 0.0;
        }
    }

    /// Wheel/trackpad over the transcript: scrolling up (positive delta,
    /// toward older messages) breaks the pin at once; scrolling down asks the
    /// next frame to re-pin when within `STICK_THRESHOLD_PX` of the end.
    pub(crate) fn on_wheel(&mut self, event: &gpui::ScrollWheelEvent, cx: &mut Context<Self>) {
        let dy = f32::from(event.delta.pixel_delta(px(20.0)).y);
        if dy > 0.0 {
            self.pinned_to_bottom = false;
            self.spring.velocity = 0.0;
            self.repin_check_pending = false;
        } else if dy < 0.0 {
            self.repin_check_pending = true;
        }
        cx.notify();
    }

    /// Wheel over the composer (sibling of this list): move the transcript
    /// the same way a wheel over the bubbles would ([feedback
    /// app-01M5PDB3QYAPBPA4FJXZE1FFD0]).
    pub fn scroll_from_wheel(&mut self, event: &gpui::ScrollWheelEvent, cx: &mut Context<Self>) {
        let dy = f32::from(event.delta.pixel_delta(px(20.0)).y);
        // List::scroll uses `scroll_top - delta.y`; scroll_by adds to offset.
        if dy != 0.0 {
            self.list.scroll_by(px(-dy));
        }
        self.on_wheel(event, cx);
    }

    /// Arm a possible vertical drag-scroll (bubbles / empty rows).
    fn begin_drag_scroll(&mut self, start: Point<Pixels>) {
        self.drag_scroll = Some(DragScroll {
            start,
            last: start,
            locked: false,
        });
    }

    /// Trackpad / mouse move while a vertical drag-scroll may be in progress.
    fn on_drag_scroll_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.drag_scroll.is_none() {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag_scroll = None;
            return;
        }
        // Text selection has committed: leave the pointer alone.
        if self.selection.read(cx).is_selecting() {
            self.drag_scroll = None;
            return;
        }

        let (dx, dy_from_start, locked) = {
            let drag = self.drag_scroll.as_ref().expect("checked above");
            let dx: f32 = (event.position.x - drag.start.x).into();
            let dy: f32 = (event.position.y - drag.start.y).into();
            (dx, dy, drag.locked)
        };
        if !locked {
            if dx.abs() + dy_from_start.abs() < DRAG_SCROLL_SLOP {
                return;
            }
            // Horizontal-dominant: not a scroll.
            if dx.abs() > dy_from_start.abs() * 1.15 {
                self.drag_scroll = None;
                return;
            }
            // Vertical: take the gesture.
            self.selection.update(cx, |s, cx| {
                s.clear();
                cx.notify();
            });
            if let Some(drag) = self.drag_scroll.as_mut() {
                drag.locked = true;
            }
        }

        let last = self
            .drag_scroll
            .as_ref()
            .map(|d| d.last)
            .expect("drag_scroll still armed");
        let step: f32 = (event.position.y - last.y).into();
        if let Some(drag) = self.drag_scroll.as_mut() {
            drag.last = event.position;
        }
        if step.abs() < 0.5 {
            return;
        }
        // Finger down → content follows → older messages (lower scroll offset).
        self.list.scroll_by(px(-step));
        if step > 0.0 {
            self.pinned_to_bottom = false;
            self.spring.velocity = 0.0;
            self.repin_check_pending = false;
        } else {
            self.repin_check_pending = true;
        }
        cx.notify();
    }

    fn end_drag_scroll(&mut self) {
        self.drag_scroll = None;
    }

    /// The ↓ button (and anything else that wants "back to the newest row"):
    /// re-pin, and land on the newest page.
    ///
    /// The stick spring moves a fraction of the *known* distance to the end
    /// each frame, and the list only knows the height of rows it has laid
    /// out — a row never scrolled past counts as zero. From deep in a long
    /// chat the known distance is therefore never more than one overdraw
    /// ahead, so the spring crawled through the whole history one page per
    /// frame, laying out every row on the way (Jacob, 2026-10-25: 1 682 rows
    /// took 11.8 s, [feedback jump-to-latest-instant-mac-2026-10-25]). Now the
    /// spring only glides when the end is close and every row between is
    /// already laid out; otherwise the list goes straight to its end — the
    /// same path a chat opens on — so only the newest page is laid out, and a
    /// [`JUMP_ARRIVE_MS`] settle marks the landing.
    pub fn jump_to_latest(&mut self, cx: &mut Context<Self>) {
        self.pinned_to_bottom = true;
        self.repin_check_pending = false;
        self.spring.velocity = 0.0;
        if !self.end_is_near_and_laid_out() {
            self.snap_to_end(!cx.reduce_motion());
        }
        cx.notify();
    }

    /// Whether the spring can glide to the end without laying out history:
    /// the known distance is under [`JUMP_GLIDE_MAX_VIEWPORTS`] and no row
    /// from the viewport to the end is still unmeasured.
    fn end_is_near_and_laid_out(&self) -> bool {
        let max = f32::from(self.list.max_offset_for_scrollbar().y);
        let cur = -f32::from(self.list.scroll_px_offset_for_scrollbar().y);
        let viewport = f32::from(self.list.viewport_bounds().size.height).max(1.0);
        if max - cur > JUMP_GLIDE_MAX_VIEWPORTS * viewport {
            return false;
        }
        let top = self.list.logical_scroll_top().item_ix;
        // `bounds_for_item` answers only for rows the list has measured.
        (top..self.rows.len()).all(|ix| self.list.bounds_for_item(ix).is_some())
    }

    /// Put the list at its end: the next layout walks back from the newest
    /// row and measures one page, nothing above it. `settle` plays the
    /// arrival cue (not on a chat open, not under Reduce Motion).
    fn snap_to_end(&mut self, settle: bool) {
        self.list.scroll_to_end();
        self.spring.velocity = 0.0;
        self.arrived_at = settle.then(Instant::now);
    }

    /// Progress of the landing cue, `None` once it is over (or never started).
    fn arrive_progress(&mut self) -> Option<f32> {
        let started = self.arrived_at?;
        let t = started.elapsed().as_secs_f32() / (JUMP_ARRIVE_MS as f32 / 1000.0);
        if t >= 1.0 {
            self.arrived_at = None;
            return None;
        }
        Some(crate::ui::motion::ease_out_cubic(t))
    }

    /// One spring step per frame while pinned and not yet at the end.
    fn step_spring(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        if self.repin_check_pending {
            self.repin_check_pending = false;
            let max = f32::from(self.list.max_offset_for_scrollbar().y);
            let cur = -f32::from(self.list.scroll_px_offset_for_scrollbar().y);
            if max - cur <= STICK_THRESHOLD_PX {
                self.pinned_to_bottom = true;
            }
        }
        if !self.pinned_to_bottom {
            return;
        }
        let max = f32::from(self.list.max_offset_for_scrollbar().y);
        let cur = -f32::from(self.list.scroll_px_offset_for_scrollbar().y);
        let remaining = max - cur;
        // At the end (or anchored past it, the frame after `snap_to_end`
        // before the list has laid that page out): nothing to glide.
        if remaining <= 0.5 {
            self.spring.velocity = 0.0;
            return;
        }
        self.spring.velocity =
            self.spring.velocity * (1.0 - SPRING_DAMPING) + remaining * SPRING_STIFFNESS;
        let step = if self.spring.velocity.abs() > remaining.abs() {
            remaining
        } else {
            self.spring.velocity
        };
        if step.abs() >= 0.25 {
            self.list.scroll_by(px(step));
            window.request_animation_frame();
        }
    }

    pub fn sync(&mut self, cx: &mut Context<Self>) {
        let theme = Theme::of(cx).clone();
        self.mentions = Mentions::from_state(&self.state, &theme, cx);
        self.link_icons = Rc::new(LinkIcons {
            by_host: self.state.read(cx).link_icons.clone(),
        });
        let state = self.state.read(cx);
        let chat_id = state.active_chat_id();
        let chat_changed = chat_id != self.chat_id;
        let pane_open = state.profile.is_some();
        let pane_changed = pane_open != self.pane_open;
        self.pane_open = pane_open;
        if chat_changed {
            self.drag_scroll = None;
        }
        let messages: Vec<Message> = chat_id
            .as_deref()
            .map(|id| state.messages_of(id).to_vec())
            .unwrap_or_default();
        let show_details = state.show_details;
        let speech = SpeechRows {
            enabled: state.voice_mode_available(),
            spoken: &state.spoken,
            failed: &state.speech_failed,
            voice_mode: state.voice_mode,
        };
        let me_id = state.me_id();
        let thumb_keys: HashSet<&str> = state.thumbnails.keys().map(|k| k.as_str()).collect();
        let multi_party = names_senders(
            state.selected(),
            state.active_chat(),
            state.selected_channel.is_some(),
        );
        let people = people_chat(
            state.selected(),
            state.active_chat(),
            state.selected_channel.is_some(),
        );
        // A group, a DM or a server channel: every row there names who wrote it, so a
        // wordless one that does not is a blank line from no one. A shared project chat's
        // agent rows say a bare `agent` by design.
        let people_space = state.selected_channel.is_some()
            || state
                .selected()
                .is_some_and(|p| matches!(p.kind, SpaceKind::Group | SpaceKind::Dm));
        if chat_changed {
            self.row_cache.clear();
        }
        self.chat_id = chat_id;
        // Clone the Rc so the row-cache mut borrow does not conflict.
        let mentions = Rc::clone(&self.mentions);
        // People I blocked: their rows fold into one hidden line for me (the relay's
        // `blockedSenders` hint, else my block list by id) — groups only; a DM with them has
        // the composer bar instead ([feedback blocked-state-mac-2026-10-26]).
        let hidden_senders: HashSet<String> = state
            .selected()
            .filter(|p| p.is_people_group())
            .map(|p| state.hidden_senders_in(p))
            .unwrap_or_default();
        let new_rows = build_rows_with_cache(
            &messages,
            show_details,
            speech,
            &me_id,
            &thumb_keys,
            &mentions,
            multi_party,
            people_space,
            state.link_previews,
            &mut self.row_cache,
            &hidden_senders,
        );
        self.multi_party = multi_party;
        self.people = people;
        // A new sender run opens the turn gap: a sender change, more than
        // `RUN_WINDOW_MS` since the run's last message, a quoted reply, a system
        // line or a date stamp in between. Messages inside a run — a person's
        // quick lines, an agent's follow-up bubbles, my own — sit 6 pt apart
        // (Jacob, 2026-10-28, [feedback grouped-messages-mac-2026-10-28];
        // supersedes #1447's gap on every agent message).
        self.turn_starts = self.row_cache.run_starts.clone();

        // Veils: observe text growth of live markdown rows.
        for row in &new_rows {
            if let RowKind::Markdown {
                block, live: true, ..
            } = &row.kind
            {
                let len = block_text_len(block);
                let veil = self
                    .veils
                    .entry(row.id.clone())
                    .or_insert_with(|| Veil::seeded(0));
                veil.observe(len);
            }
        }
        self.veils
            .retain(|id, _| new_rows.iter().any(|r| r.id == *id));
        self.cards.retain(|qid, _| {
            new_rows.iter().any(
                |r| matches!(&r.kind, RowKind::Question { question_id, .. } if question_id == qid),
            )
        });

        if chat_changed {
            self.rows = new_rows;
            self.list.reset(self.rows.len());
            self.pinned_to_bottom = true;
            self.revealed = None;
            // Open at the bottom: the same landing as ↓, without the cue.
            self.snap_to_end(false);
            // `ARBOS_SCROLL_UP=1`: open at the top instead (screenshot rig; the
            // GUI session accepts no synthetic wheel events).
            if std::env::var("ARBOS_SCROLL_UP").is_ok_and(|v| v == "1")
                && self.state.read(cx).reveal_message.is_none()
            {
                self.pinned_to_bottom = false;
                self.list.scroll_to(gpui::ListOffset {
                    item_ix: 0,
                    offset_in_item: px(0.0),
                });
            }
            self.reveal_hit(cx);
            cx.notify();
            return;
        }

        let prefix = self
            .rows
            .iter()
            .zip(new_rows.iter())
            .take_while(|(a, b)| a.id == b.id && a.version == b.version)
            .count();
        if prefix == self.rows.len() && prefix == new_rows.len() {
            if pane_changed {
                // Same rows, new column width: measure them again.
                self.list.reset(self.rows.len());
                if self.pinned_to_bottom {
                    self.list.scroll_to_end();
                }
                cx.notify();
            }
            self.reveal_hit(cx);
            return;
        }
        let old_len = self.rows.len();
        self.rows = new_rows;
        self.list.splice(prefix..old_len, self.rows.len() - prefix);
        if crate::model::trace_rows() {
            eprintln!(
                "rows: transcript chat={} rows={} changed_from={prefix} at={}",
                self.chat_id.as_deref().unwrap_or(""),
                self.rows.len(),
                crate::model::now_ms()
            );
        }
        // Reading history: a stale pin (scrollbar / list scroll that never
        // cleared it) must not spring to the new end when rows land below.
        let max = f32::from(self.list.max_offset_for_scrollbar().y);
        let cur = -f32::from(self.list.scroll_px_offset_for_scrollbar().y);
        if max - cur > STICK_THRESHOLD_PX {
            self.pinned_to_bottom = false;
            self.spring.velocity = 0.0;
        }
        self.reveal_hit(cx);
        // While pinned the spring glides to the new end on the next frames.
        cx.notify();
    }

    /// A home-search hit or a quote click asked to land on a message. Scroll
    /// once per request, when its row exists.
    fn reveal_hit(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let Some((chat, msg)) = state.reveal_message.clone() else {
            self.revealed = None;
            return;
        };
        let nonce = state.reveal_nonce;
        if self.chat_id.as_deref() != Some(chat.as_str()) {
            return;
        }
        if self.revealed == Some(nonce) {
            return;
        }
        let Some(ix) = self.rows.iter().position(|r| {
            r.message_id == msg && !matches!(r.kind, RowKind::Stamp { .. } | RowKind::BottomInset)
        }) else {
            return;
        };
        self.pinned_to_bottom = false;
        self.list.scroll_to(gpui::ListOffset {
            item_ix: ix,
            offset_in_item: px(0.0),
        });
        self.revealed = Some(nonce);
        cx.notify();
    }

    /// The message a selection bubble key (`<message>#text`, `<row>.u`, see
    /// `bubble_texts`) belongs to.
    fn message_of_bubble(&self, key: &str) -> Option<Id> {
        self.rows.iter().find_map(|row| {
            let row_key = match &row.kind {
                RowKind::Markdown { .. } | RowKind::NoAnswer { .. } => {
                    format!("{}#text", row.message_id)
                }
                RowKind::User { .. } => format!("{}.u", row.id),
                _ => return None,
            };
            (row_key == key).then(|| row.message_id.clone())
        })
    }

    /// ⌘C with a whole bubble selected (triple-click, select-all in it) on a message
    /// that carries pictures or files: the whole message, pictures included, is what
    /// the user means ([feedback copy-image-messages-mac-2026-10-27]). `Some(id)`
    /// names that message; `None` keeps the plain text copy.
    fn whole_bubble_with_attachments(
        &self,
        span: &crate::ui::select::Span,
        cx: &App,
    ) -> Option<Id> {
        if !span.one_bubble() {
            return None;
        }
        let whole = span.start_index == 0
            && span.end_index >= span.start.text.trim_end().len()
            && !span.start.text.trim().is_empty();
        if !whole {
            return None;
        }
        let message_id = self.message_of_bubble(&span.start.key)?;
        let chat_id = self.chat_id.as_ref()?;
        self.state
            .read(cx)
            .messages
            .get(chat_id)
            .and_then(|list| list.iter().find(|m| m.id == message_id))
            .and_then(|m| m.body())
            .filter(|b| !b.attachments.is_empty())
            .map(|_| message_id)
    }

    /// Copy of a message — the bubble menu's Copy, control-click, ⌘C on a whole
    /// bubble: the whole message through `AppState::copy_message` (its text and
    /// every picture / file it carries, [feedback copy-image-messages-mac-2026-10-27];
    /// before, only `plain_text()` went out and a picture message copied nothing).
    /// The bubble reads "Copied" for [`COPY_FEEDBACK_MS`] when it has anything to copy.
    pub(crate) fn copy_message(&mut self, message_id: &Id, cx: &mut Context<Self>) {
        let Some(chat_id) = self.chat_id.clone() else {
            return;
        };
        let has_body = self
            .state
            .read(cx)
            .messages
            .get(&chat_id)
            .and_then(|list| list.iter().find(|m| m.id == *message_id))
            .is_some_and(|m| m.body().is_some());
        self.state
            .update(cx, |s, cx| s.copy_message(&chat_id, message_id, cx));
        if !has_body {
            return;
        }
        self.copied_feedback = Some((message_id.clone(), Instant::now()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(COPY_FEEDBACK_MS))
                .await;
            let _ =
                this.update(cx, |this, cx| {
                    if this.copied_feedback.as_ref().is_some_and(|(_, at)| {
                        at.elapsed() >= Duration::from_millis(COPY_FEEDBACK_MS)
                    }) {
                        this.copied_feedback = None;
                        cx.notify();
                    }
                });
        })
        .detach();
    }

    /// A code card's Copy pill: the block's text to the clipboard and the pill
    /// reads "Copied" for [`CODE_COPY_FEEDBACK_MS`].
    pub(crate) fn copy_code(&mut self, pill: SharedString, code: String, cx: &mut Context<Self>) {
        copy_code_to_clipboard(&mut self.code_copied, pill, code, cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(CODE_COPY_FEEDBACK_MS))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.code_copied.as_ref().is_some_and(|(_, at)| {
                    at.elapsed() >= Duration::from_millis(CODE_COPY_FEEDBACK_MS)
                }) {
                    this.code_copied = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The code cards' Copy wiring for this frame.
    /// `code_copy` for one message: Remove preview is offered where the relay would let this
    /// user delete the message (the same #1526 rule the bubble menu uses).
    fn code_copy_for(&self, message_id: &Id, cx: &Context<Self>) -> CodeCopy {
        let mut copy = self.code_copy(cx);
        copy.moderates = self.chat_id.as_ref().is_some_and(|chat| {
            let state = self.state.read(cx);
            let space_id = state.space_of_chat(chat);
            match (
                state.project(&space_id),
                state.messages_of(chat).iter().find(|m| &m.id == message_id),
            ) {
                (Some(space), Some(message)) => crate::model::may_delete_message(
                    space,
                    &state.me_id(),
                    &state.me_handle(),
                    message,
                ),
                _ => false,
            }
        });
        copy
    }

    fn code_copy(&self, cx: &Context<Self>) -> CodeCopy {
        let this = cx.entity().downgrade();
        let hover = this.clone();
        let save_state = self.state.clone();
        let expand_state = self.state.clone();
        let chat_id = self.chat_id.clone();
        let links = {
            // The preview pictures this chat has decoded (keys `lp-…`), snapshotted here so
            // the cards need no app context at paint time.
            let thumbs: Rc<HashMap<String, std::sync::Arc<gpui::RenderImage>>> = Rc::new(
                self.state
                    .read(cx)
                    .thumbnails
                    .iter()
                    .filter(|(k, _)| k.starts_with("lp-") || k.starts_with("li-"))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            );
            // The relay's host icons (`AppState::link_icon`), same snapshot idea.
            let host_icons: Rc<
                HashMap<
                    String,
                    (
                        std::sync::Arc<gpui::RenderImage>,
                        crate::ui::link_tile::Tone,
                    ),
                >,
            > = Rc::new({
                let state = self.state.read(cx);
                state
                    .link_icons
                    .keys()
                    .filter_map(|h| {
                        state
                            .link_icon(h)
                            .map(|(img, tone)| (h.clone(), (img.clone(), tone)))
                    })
                    .collect()
            });
            let open_state = self.state.clone();
            let remove_state = self.state.clone();
            let remove_chat = self.chat_id.clone();
            Rc::new(crate::ui::link_card::LinkUi {
                thumbnail: Rc::new(move |key| thumbs.get(key).cloned()),
                host_icon: Rc::new(move |host| host_icons.get(host).cloned()),
                on_open: Rc::new(move |url, _, cx| {
                    open_state.update(cx, |s, cx| s.open_link(&url, cx));
                }),
                on_remove: Rc::new(move |message_id, _, cx| {
                    if let Some(chat) = remove_chat.clone() {
                        remove_state
                            .update(cx, |s, cx| s.remove_link_preview(chat, message_id, cx));
                    }
                }),
            })
        };
        CodeCopy {
            copied: copied_pill(&self.code_copied, Instant::now()),
            links: Some(links),
            moderates: false,
            on_copy: Some(Rc::new(move |id, code, _, cx| {
                let _ = this.update(cx, |t, cx| t.copy_code(id, code, cx));
            })),
            chart: Some(Rc::new(crate::ui::chart_card::ChartUi {
                on_hover: Rc::new(move |_, cx| {
                    let _ = hover.update(cx, |_, cx| cx.notify());
                }),
                on_save: Rc::new(move |spec, title, _, cx| {
                    crate::ui::chart_card::save_png(&spec, &title, &save_state, cx);
                }),
                on_expand: Rc::new(move |part_id, spec, _, cx| {
                    expand_state.update(cx, |s, cx| {
                        s.open_chart_viewer(
                            crate::ui::chart_viewer::ChartViewer {
                                chat_id: chat_id.clone(),
                                part_id,
                                spec,
                                data_view: false,
                                copied_at: None,
                            },
                            cx,
                        )
                    });
                }),
            })),
        }
    }

    /// Right-click → the message menu (Copy / Reply / Delete, `message_menu`).
    /// Control-click → copy the message text.
    fn menu_target(
        &self,
        el: Stateful<Div>,
        message_id: &Id,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let Some(chat_id) = self.chat_id.clone() else {
            return el;
        };
        let (right_chat, right_msg) = (chat_id, message_id.clone());
        let ctrl_msg = message_id.clone();
        el.on_mouse_down(
            gpui::MouseButton::Right,
            cx.listener(move |_, event: &gpui::MouseDownEvent, _, cx| {
                cx.stop_propagation();
                cx.emit(TranscriptEvent::MessageMenu {
                    chat_id: right_chat.clone(),
                    message_id: right_msg.clone(),
                    position: event.position,
                });
            }),
        )
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                if !event.modifiers.control {
                    return;
                }
                cx.stop_propagation();
                this.copy_message(&ctrl_msg, cx);
            }),
        )
    }

    /// Grok's quote header at the top of a reply bubble: speech-bubble glyph,
    /// then the original's first words in quotes, one line, truncated. Click
    /// → scroll to the original and highlight it for a moment.
    fn quote_header(
        &self,
        row_id: &str,
        quote: Quote,
        text_color: gpui::Hsla,
        cx: &App,
    ) -> Stateful<Div> {
        let state = self.state.clone();
        let chat_id = self.chat_id.clone();
        let target = quote.message_id.clone();
        let muted = Theme::with_alpha(text_color, 0.72);
        // Reply in DM (#1835): the quote names its chat; it is a link only when
        // the viewer can open that chat (a member), else just the words.
        let source = quote.source.clone();
        let linkable = match &source {
            Some(src) => self.state.read(cx).is_member_of_chat(&src.chat_id),
            None => true,
        };
        let quote_line = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .min_w_0()
            .child(sf_symbols::glyph("bubble.left", 12.0, muted))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(TEXT_SMALL))
                    .line_height(px(LINE_HEIGHT - 4.0))
                    .text_color(muted)
                    .child(format!("“{}”", quote.text)),
            );
        let selector = format!("reply-quote-{row_id}");
        let source_selector = format!("reply-quote-source-{row_id}");
        div()
            .id(SharedString::from(format!("{row_id}#quote")))
            .debug_selector(move || selector.clone())
            .flex()
            .flex_col()
            .gap(px(2.0))
            .min_w_0()
            .when(linkable, |el| el.cursor_pointer().hover(|s| s.opacity(0.8)))
            .when_some(source.as_ref(), |el, src| {
                el.child(
                    div()
                        .debug_selector(move || source_selector.clone())
                        .min_w_0()
                        .truncate()
                        .text_size(px(TEXT_TINY))
                        .line_height(px(LINE_HEIGHT - 6.0))
                        .text_color(muted)
                        .child(src.label.clone()),
                )
            })
            .child(quote_line)
            .when(linkable, |el| {
                el.on_click(move |_, _, cx| match (&source, chat_id.clone()) {
                    (Some(src), _) => {
                        let r = ReplyRef {
                            chat_id: src.chat_id.clone(),
                            message_id: target.clone(),
                            ..ReplyRef::default()
                        };
                        state.update(cx, |s, cx| {
                            s.reveal_reply_ref(&r, cx);
                        });
                    }
                    (None, Some(chat_id)) => {
                        let target = target.clone();
                        state.update(cx, |s, cx| s.reveal_reply(chat_id, target, cx));
                    }
                    (None, None) => {}
                })
            })
    }

    /// The hover rail beside a bubble (fades with the row), in Grok's order:
    /// smiley (the reaction capsule) · reply · ⋯ (the message menu: Copy,
    /// Reply, Delete). After a copy from the menu the ⋯ shows a checkmark
    /// for a moment.
    /// The red "! Retry" beside one of my bubbles the relay refused: a click
    /// re-sends the text (and reply-to) as a new message and drops the failed row.
    fn failed_retry(
        &self,
        row_id: &str,
        message_id: &Id,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let state = self.state.clone();
        let chat_id = self.chat_id.clone();
        let message_id = message_id.clone();
        failed_glyph(theme)
            .id(SharedString::from(format!("{row_id}#retry")))
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, _, cx| {
                let (Some(chat_id), message_id) = (chat_id.clone(), message_id.clone()) else {
                    return;
                };
                state.update(cx, |s, cx| s.resend_failed(chat_id, message_id, cx));
            }))
    }

    fn hover_rail(
        &self,
        row_id: &str,
        message_id: &Id,
        opacity: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let chat_id = self.chat_id.clone();
        let (react_chat, react_msg) = (chat_id.clone(), message_id.clone());
        let (more_chat, more_msg) = (chat_id.clone(), message_id.clone());
        let reply_msg = message_id.clone();
        let just_copied = self.copied_feedback.as_ref().is_some_and(|(id, at)| {
            id == message_id && at.elapsed() < Duration::from_millis(COPY_FEEDBACK_MS)
        });
        // Keep the checkmark readable even if the pointer leaves mid-feedback.
        let more_opacity = if just_copied { 1.0 } else { opacity };
        let more_glyph = if just_copied { "checkmark" } else { "ellipsis" };
        // On Glass the three sit in one capsule over the desktop; the capsule
        // fades as a whole, so the buttons inside carry only what differs
        // from it (the smiley and reply keep fading under a held checkmark).
        let plate = theme.rail_plate();
        let glyph_color = if plate.is_some() {
            theme.text
        } else {
            theme.text_muted
        };
        let (group_opacity, opacity, more_opacity) = match plate {
            Some(_) if just_copied => (1.0, opacity, 1.0),
            Some(_) => (opacity, 1.0, 1.0),
            None => (1.0, opacity, more_opacity),
        };
        let more_color = if just_copied {
            theme.online
        } else {
            glyph_color
        };
        let btn = |id: SharedString, glyph: &'static str, opacity: f32, color: gpui::Hsla| {
            div()
                .id(id)
                .debug_selector(move || format!("rail-button#{glyph}"))
                // A fixed box; the hover wash is paint only (`highlight::button`).
                .size(px(RAIL_BUTTON))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .opacity(opacity)
                .map(|el| highlight::button(el, theme))
                .child(sf_symbols::glyph(glyph, RAIL_GLYPH, color))
        };
        div()
            .flex()
            .items_center()
            .gap(px(RAIL_GAP))
            .when_some(plate, |el, plate| {
                el.debug_selector(|| "rail-plate".into())
                    .h(px(RAIL_BUTTON + 2.0 * RAIL_PLATE_PAD))
                    .p(px(RAIL_PLATE_PAD))
                    .rounded_full()
                    .opacity(group_opacity)
                    // Every row carries a rail; only a revealed one paints its plate.
                    .when(group_opacity > 0.0, |el| {
                        el.bg(highlight::over(plate, theme.control_fill()))
                            .shadow(vec![
                                BoxShadow::new(px(0.0), px(0.0), theme.pane_card_rim())
                                    .spread_radius(px(1.0))
                                    .inset(),
                                BoxShadow::new(px(0.0), px(1.0), gpui::hsla(0.0, 0.0, 0.0, 0.28))
                                    .blur_radius(px(3.0)),
                            ])
                    })
            })
            .child(
                btn(
                    SharedString::from(format!("{row_id}#react")),
                    "face.smiling",
                    opacity,
                    glyph_color,
                )
                .on_click(cx.listener(
                    move |_, event: &gpui::ClickEvent, _, cx| {
                        if let Some(chat_id) = react_chat.clone() {
                            cx.emit(TranscriptEvent::ReactionCapsule {
                                chat_id,
                                message_id: react_msg.clone(),
                                position: event.position(),
                            });
                        }
                    },
                )),
            )
            .child(
                btn(
                    SharedString::from(format!("{row_id}#reply")),
                    "arrowshape.turn.up.left",
                    opacity,
                    glyph_color,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    if let Some(chat_id) = chat_id.clone() {
                        cx.emit(TranscriptEvent::Reply {
                            chat_id,
                            message_id: reply_msg.clone(),
                        });
                    }
                })),
            )
            .child(
                btn(
                    SharedString::from(format!("{row_id}#more")),
                    more_glyph,
                    more_opacity,
                    more_color,
                )
                .on_click(cx.listener(
                    move |_, event: &gpui::ClickEvent, _, cx| {
                        if let Some(chat_id) = more_chat.clone() {
                            cx.emit(TranscriptEvent::MessageMenu {
                                chat_id,
                                message_id: more_msg.clone(),
                                position: event.position(),
                            });
                        }
                    },
                )),
            )
    }

    /// A sender row's agent, resolved like the chip: a shared agent's reply may
    /// carry only its handle, so name, disc seed and colour come from the chat's
    /// member list or my projects. Returns (name, disc seed, colour index, project id).
    fn agent_identity(
        &self,
        seed: &str,
        name: &str,
        cx: &Context<Self>,
    ) -> (String, String, Option<u8>, Option<Id>) {
        let a = self.agent_identity_full(seed, name, cx);
        (a.name, a.seed, a.color_index, a.project_id)
    }

    /// `agent_identity` plus the D92 leaf pair in effect — the shared rule
    /// (`avatars::agent_identity`, #1592): the open chat's roster, then my projects, then
    /// every other roster; the row's own name when no card knows the agent. The presence
    /// row asks the same function with the same row, so the two can never disagree.
    fn agent_identity_full(&self, seed: &str, name: &str, cx: &Context<Self>) -> AgentIdentity {
        let state = self.state.read(cx);
        let hint =
            (name != "Agent" && name != "Project" && !name.trim().is_empty()).then_some(name);
        let a = avatars::agent_identity(state.selected(), &state.projects, seed, None, hint);
        let had_id = seed.starts_with("agent:") || seed.starts_with("project:");
        // D92 amendment: the agent's uploaded / generated picture, once the loader has it.
        let image = a.image_key.as_deref().and_then(|url| state.avatar(url));
        // A neutral agent (no leaf, Jacob 2026-10-25) carries the stored pair on, so every
        // disc drawn from this identity goes through `avatars::agent`'s neutral branch.
        let leaf = if a.neutral {
            let n = avatars::neutral_pick();
            (Some(n.leaf), Some(n.color))
        } else {
            (Some(a.leaf.leaf), Some(a.leaf.color))
        };
        AgentIdentity {
            name: a.name,
            seed: a.id.clone(),
            color_index: Some(a.color_index),
            leaf,
            image,
            project_id: (a.known || had_id).then_some(a.id),
        }
    }

    /// Grok's centred agent line: `lead` then "◉ A, ◉ B and ◉ C" with 16 pt discs. A name that
    /// is one of my agents is a link to that agent's chat (the "open Doug" affordance).
    fn agent_line(
        &self,
        lead: &str,
        names: &[String],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        let mut line = div()
            .flex()
            .items_center()
            .gap(px(5.0))
            .text_size(px(TEXT_SMALL))
            .text_color(theme.text_muted)
            .child(lead.to_owned());
        let n = names.len();
        for (i, name) in names.iter().enumerate() {
            let mine = state
                .projects
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(name) && p.kind == SpaceKind::Project);
            let glyph: AnyElement = match mine {
                Some(space) => {
                    crate::ui::avatars::project_in(state, space, 16.0, theme).into_any_element()
                }
                // Not one of my projects (a subagent): initials on the name's gradient.
                None => {
                    crate::ui::avatars::element(name, name, None, 16.0, theme).into_any_element()
                }
            };
            let mut who = div()
                .id(SharedString::from(format!("agent-line-{lead}-{i}-{name}")))
                .flex()
                .items_center()
                .gap(px(5.0))
                .child(glyph)
                .child(div().child(name.clone()));
            if let Some(space) = mine {
                let pid = space.id.clone();
                let state_entity = self.state.clone();
                who = who
                    .cursor_pointer()
                    .hover(|el| el.text_color(theme.text))
                    .on_click(move |_, _, cx| {
                        let pid = pid.clone();
                        state_entity.update(cx, |s, cx| s.select_project(pid, cx));
                    });
            }
            line = line.child(who);
            if n >= 2 && i + 2 == n {
                line = line.child(div().child("and"));
            } else if i + 2 < n {
                line = line.child(div().ml(px(-5.0)).child(","));
            }
        }
        div()
            .w_full()
            .flex()
            .justify_center()
            .pt(px(10.0))
            .pb(px(6.0))
            .child(line)
            .into_any_element()
    }

    /// The disc and (when `named`) the name label for someone else's attachment row —
    /// a person's or an agent's, with the same colours and profile targets as their
    /// bubble ([feedback forward-attachments-mac-2026-10-27]).
    fn attachment_sender(
        &self,
        row_id: &str,
        sender: &AttachmentSender,
        named: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> (AnyElement, Option<AnyElement>) {
        match sender {
            AttachmentSender::Person(person) => {
                let state = self.state.read(cx);
                let image = state.avatar(&person.avatar_url);
                let color = state.sender_accent(person, theme);
                let space_id = self.chat_id.as_ref().and_then(|c| {
                    state.chats.iter().find_map(|(pid, tabs)| {
                        tabs.iter().any(|t| &t.id == c).then(|| pid.clone())
                    })
                });
                let page = crate::app::ProfilePage::User {
                    handle: person.handle.clone(),
                    space_id,
                };
                let disc = profile_target::attach(
                    div()
                        .id(SharedString::from(format!("{row_id}#disc")))
                        .debug_selector(|| format!("disc#{row_id}"))
                        .flex_shrink_0()
                        .child(people::avatar(image, person, SENDER_DISC, theme)),
                    &self.state,
                    page.clone(),
                    &person.display_name(),
                    cx,
                )
                .into_any_element();
                let name = named.then(|| {
                    profile_target::attach(
                        self.sender_name_label(row_id, person.display_name(), color, false),
                        &self.state,
                        page,
                        &person.display_name(),
                        cx,
                    )
                    .into_any_element()
                });
                (disc, name)
            }
            AttachmentSender::Agent { seed } => {
                let a = self.agent_identity_full(seed, "Agent", cx);
                let color =
                    avatars::agent_accent(&a.seed, a.color_index, a.leaf.0, a.leaf.1, theme);
                let disc = div()
                    .id(SharedString::from(format!("{row_id}#disc")))
                    .debug_selector(|| format!("disc#{row_id}"))
                    .flex_shrink_0()
                    .child(self.agent_disc(seed, cx))
                    .when_some(a.project_id.clone(), |el, pid| {
                        profile_target::attach(
                            el,
                            &self.state,
                            crate::app::ProfilePage::Agent { project_id: pid },
                            &a.name,
                            cx,
                        )
                    })
                    .into_any_element();
                let name = named.then(|| {
                    self.sender_name_label(row_id, a.name.clone(), color, true)
                        .when_some(a.project_id.clone(), |el, pid| {
                            profile_target::attach(
                                el,
                                &self.state,
                                crate::app::ProfilePage::Agent { project_id: pid },
                                &a.name,
                                cx,
                            )
                        })
                        .into_any_element()
                });
                (disc, name)
            }
        }
    }

    /// The 24 pt disc for an agent's message in a people chat.
    fn agent_disc(&self, seed: &str, cx: &Context<Self>) -> Div {
        let theme = Theme::of(cx).clone();
        let a = self.agent_identity_full(seed, "Agent", cx);
        if let Some(image) = a.image {
            return div()
                .size(px(SENDER_DISC))
                .flex_shrink_0()
                .rounded_full()
                .overflow_hidden()
                .child(
                    gpui::img(crate::ui::picture_fit::fit(&image, SENDER_DISC))
                        .size(px(SENDER_DISC))
                        .rounded_full()
                        .object_fit(gpui::ObjectFit::Cover),
                );
        }
        avatars::agent(
            // avatar-lint: allow (the resolver's identity, decoded above)
            &a.seed,
            &a.name,
            a.color_index,
            a.leaf.0,
            a.leaf.1,
            SENDER_DISC,
            &theme,
        )
    }

    /// The sender's name inside a group / channel bubble (Jacob's Discord-style
    /// reference, 2026-10-23): semibold in the sender's identity colour, with a
    /// small `agent` tag for agents. The caller adds the click → profile.
    fn sender_name_label(
        &self,
        row_id: &str,
        name: String,
        color: gpui::Hsla,
        agent: bool,
    ) -> Stateful<Div> {
        div()
            .id(SharedString::from(format!("{row_id}#who")))
            .debug_selector(|| format!("who#{row_id}"))
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(
                identity::name(name, CHAT_BODY)
                    .line_height(px(CHAT_LINE_HEIGHT))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color),
            )
            .when(agent, |el| {
                el.child(
                    div()
                        .h(px(16.0))
                        .px(px(5.0))
                        .flex()
                        .items_center()
                        .rounded(px(4.0))
                        .bg(Theme::with_alpha(color, 0.16))
                        .text_size(px(10.0))
                        .line_height(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color)
                        .child("agent"),
                )
            })
    }

    /// Display name for a reaction sender id (`user:…` / `agent:…`), matching
    /// the iPhone's `reactionSenderLabel` ("You" for self). Shown in the chip's
    /// floating tip so the reaction row never grows on hover.
    fn reaction_sender_label(&self, sender: &str, me: &str, cx: &App) -> String {
        if sender == me {
            return "You".into();
        }
        if let Some(m) = self.mentions.by_id(sender) {
            return m.name.clone();
        }
        let state = self.state.read(cx);
        if let Some(id) = sender.strip_prefix("agent:") {
            if let Some(p) = state.project(id) {
                return p.name.clone();
            }
            for space in state.projects.iter().filter(|p| p.is_people()) {
                if let Some(a) = space.agents.iter().find(|a| a.project_id == id) {
                    return a.name.clone();
                }
            }
        }
        if let Some(id) = sender.strip_prefix("user:") {
            if id == state.me_id() {
                return "You".into();
            }
            if let Some(p) = state
                .projects
                .iter()
                .flat_map(|p| p.people.iter())
                .chain(state.people_results.iter())
                .chain(state.people_directory.iter())
                .find(|p| p.id == id)
            {
                return p.display_name();
            }
        }
        sender
            .split_once(':')
            .map(|(_, bare)| bare)
            .filter(|b| !b.is_empty())
            .unwrap_or(sender)
            .to_string()
    }

    /// One assistant bubble block (the Markdown row and the no-answer line share it).
    #[allow(clippy::too_many_arguments)]
    fn markdown_bubble(
        &mut self,
        row_id: &str,
        message_id: &Id,
        block: Block,
        bubble_text: BubbleText,
        live: bool,
        first: bool,
        last: bool,
        fill: bool,
        time: Option<String>,
        lane: Option<String>,
        quote: Option<Quote>,
        named: bool,
        disc: bool,
        rail_centre: Option<Pixels>,
        now: Instant,
        theme: &Theme,
        selectable: &Selectable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let veil = if live {
            self.veils.get(row_id).cloned()
        } else {
            None
        };
        let fading = veil.as_ref().map(|v| v.is_active(now)).unwrap_or(false);
        if live || fading {
            PulseClock::request(cx);
        }
        // One assistant bubble, drawn block by block: the first block
        // rounds the top, the last the bottom, the rest run straight.
        // People-chat agent bubbles get the same coloured left-edge
        // sliver as person messages, from the agent's disc colour.
        let identity = lane
            .as_deref()
            .map(|seed| self.agent_identity_full(seed, "Agent", cx));
        let agent_accent = identity
            .as_ref()
            .map(|a| avatars::agent_accent(&a.seed, a.color_index, a.leaf.0, a.leaf.1, theme));
        // Group / channel: the agent's name inside its first bubble, in its
        // colour, with the `agent` tag; the time only appears in the hover rail.
        // The name and the disc are profile targets: one click opens the
        // agent's pane, a second closes it (`ui::profile_target`).
        let name_el = match (&identity, agent_accent) {
            (Some(a), Some(color)) if named => {
                let pid = a.project_id.clone();
                let name = a.name.clone();
                Some(
                    self.sender_name_label(row_id, a.name.clone(), color, true)
                        .when_some(pid, |el, pid| {
                            profile_target::attach(
                                el,
                                &self.state,
                                crate::app::ProfilePage::Agent { project_id: pid },
                                &name,
                                cx,
                            )
                        })
                        .into_any_element(),
                )
            }
            _ => None,
        };
        let r = px(BUBBLE_RADIUS);
        let quote_header = quote.filter(|_| first).map(|q| {
            self.quote_header(row_id, q, theme.text, cx)
                .into_any_element()
        });
        let bubble = self
            .menu_target(
                div().id(SharedString::from(format!("{}#bubble", row_id))),
                message_id,
                cx,
            )
            .debug_selector(|| "chat-bubble-assistant".into())
            // The cap is the slot's width (the column minus the 84 pt gap,
            // see `ASSISTANT_RIGHT_GAP`), so it follows the pane.
            .max_w_full()
            // A flex item's auto min-width is its content's; without
            // this the bubble keeps its width when the right pane opens
            // and runs under it instead of re-wrapping.
            .min_w_0()
            .relative()
            .when(fill, |el| el.w_full())
            .px(px(BUBBLE_PAD_X))
            .when(first, |el| el.pt(px(BUBBLE_PAD_Y)).rounded_t(r))
            .when(!first, |el| el.pt(px(BLOCK_GAP_HALF)))
            .when(last, |el| el.pb(px(BUBBLE_PAD_Y)).rounded_b(r))
            .when(!last, |el| el.pb(px(BLOCK_GAP_HALF)))
            .bg(theme.bubble_assistant)
            .when_some(agent_accent, |el, color| {
                el.border_l_2().border_color(Theme::with_alpha(color, 0.7))
            })
            .child(sender_bubble_column(
                name_el,
                quote_header,
                render_block(
                    row_id,
                    &block,
                    bubble_text.at,
                    veil.as_ref(),
                    now,
                    theme,
                    &selectable.bubble(format!("{message_id}#text"), bubble_text.text),
                    &self.code_copy_for(message_id, cx),
                ),
            ));
        // Every block row of the reply shares one hover key, so the rail shows
        // while the pointer is anywhere on the bubble, not only its last block.
        let (row_el, reveal) =
            self.hover_row(row_id, &Self::bubble_hover_key(message_id), now, window, cx);
        // Hover: the rail (smiley · reply · ⋯) and the bubble's time, drawn by
        // the last block of a finished reply. Nothing of it exists at rest.
        let hover_actions =
            (last && !live).then(|| self.hover_rail(row_id, message_id, reveal, theme, cx));
        let hover_time = (last && !live)
            .then(|| hover_time(time, reveal, theme).map(|t| t.ml(px(8.0))))
            .flatten();
        // The rail hangs off the bubble's right edge, out of the flow: it
        // lives in the column's 84 pt gap and never narrows the bubble. It is
        // centred on the whole bubble (all of its block rows), like Grok's.
        let hover_rail = (hover_actions.is_some() || hover_time.is_some()).then(|| {
            hover_rail_frame(rail_centre)
                .children(hover_actions)
                .children(hover_time)
        });
        let bubble = bubble.children(hover_rail);
        // People chats: the sender's disc at the bubble's bottom-left on the
        // last block of the run's first message; earlier blocks, and every
        // block of a message that continues the run, keep the lane free
        // (mocks a/f; grouped messages, Jacob 2026-10-28).
        let disc = lane.as_deref().map(|seed| {
            if last && disc {
                // The disc is the same profile target as the name.
                let (agent_name, _, _, agent_pid) = self.agent_identity(seed, "Agent", cx);
                div()
                    .id(SharedString::from(format!("{row_id}#disc")))
                    .debug_selector(|| format!("disc#{row_id}"))
                    .flex_shrink_0()
                    .child(self.agent_disc(seed, cx))
                    .when_some(agent_pid, |el, pid| {
                        profile_target::attach(
                            el,
                            &self.state,
                            crate::app::ProfilePage::Agent { project_id: pid },
                            &agent_name,
                            cx,
                        )
                    })
                    .into_any_element()
            } else {
                div().w(px(SENDER_DISC)).flex_shrink_0().into_any_element()
            }
        });
        row_el
            .justify_start()
            .items_end()
            .when(first, |el| el.pt(px(ROW_GAP_HALF)))
            .when(last, |el| el.pb(px(ROW_GAP_HALF)))
            .when(lane.is_some(), |el| {
                el.pl(px(AVATAR_COLUMN_INSET)).gap(px(SENDER_DISC_GAP))
            })
            .children(disc)
            .child(incoming_slot(bubble, lane.is_some()))
            .into_any_element()
    }

    /// One row of the list. Building a row's element from a message the code did not expect
    /// must not take the app down (2026-10-26: Mac 0.6.268 aborted at every launch on the
    /// restored transcript): a panic here is caught, logged by the panic hook, and the row
    /// shows a short note instead.
    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.render_row_inner(ix, window, cx)
        }));
        match result {
            Ok(element) => element,
            Err(_) => {
                let theme = Theme::of(cx).clone();
                log::error!(target: "arbos_mac::diagnostic", "transcript row {ix} skipped after a panic");
                div()
                    .w_full()
                    .px(px(BUBBLE_PAD_X))
                    .py(px(6.0))
                    .text_size(px(12.0))
                    .text_color(theme.text_faint)
                    .child("This message could not be shown.")
                    .into_any_element()
            }
        }
    }

    fn render_row_inner(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.rows_laid_out += 1;
        let Some(row) = self.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        let theme = Theme::of(cx).clone();
        let now = Instant::now();
        let row_frame_id = row.id.clone();
        // Every block row of a message shares its bubble's transcript order,
        // so a selection across bubbles knows which end comes first.
        let selectable = Selectable::new(&self.selection, theme.accent_soft, cx)
            .with_mentions(self.mentions.clone(), theme.accent)
            .with_link_icons(self.link_icons.clone(), link_icon_placeholder(&theme))
            // ⌘F Search-in-chat: this message's hit words get their marks.
            .with_marks(chat_search::marks_for(
                self.state.read(cx),
                &row.message_id,
                &theme,
            ))
            .ordered(self.message_order(ix));
        let inner: AnyElement = match row.kind {
            RowKind::User {
                text,
                sender_label,
                from,
                named,
                disc,
                time,
                delivery,
                quote,
                forwarded_from,
                ghost,
                preview,
            } => {
                // My bubble is dark in both appearances: white text, a
                // stronger highlight than the soft one on the grey bubbles.
                let (text_color, bubble_selectable) = if from.is_some() {
                    (theme.text, selectable.clone())
                } else {
                    (
                        theme.bubble_text(),
                        selectable.with_color(Theme::with_alpha(theme.accent, 0.45)),
                    )
                };
                // A forward carries its "Forwarded from" header where a reply
                // would carry its quote (a forward never quotes).
                let quote_header = match forwarded_from {
                    Some(by) => Some(forwarded_header(by, text_color)),
                    None => quote.map(|q| self.quote_header(&row.id, q, text_color, cx)),
                };
                // Triple-backtick fences render as code cards (same as agent
                // replies); other text stays plain so "# heading" is still literal.
                // Bare `http(s)://…` URLs are still accented and clickable.
                let code_copy = self.code_copy_for(&row.message_id, cx);
                let link_only = text.is_empty() && preview.is_some();
                let body = match &ghost {
                    Some(name) => secret_ghost_body(name, from.as_ref(), &theme),
                    None if link_only => div().into_any_element(),
                    None => user_message_body(
                        &row.id,
                        &text,
                        text_color,
                        &theme,
                        &bubble_selectable,
                        &code_copy,
                    ),
                };
                // The link preview card under the text — or as the bubble's whole body
                // when the message was only the link ([feedback link-embeds-mac-2026-10-27]).
                let body = match &preview {
                    Some(p) => {
                        let card = crate::ui::link_card::card(
                            &row.id,
                            &row.message_id,
                            &p.1,
                            &theme,
                            code_copy.links.as_ref(),
                            crate::ui::link_card::Place {
                                own: from.is_none(),
                                removable: code_copy.moderates,
                            },
                        );
                        let mut col = div().flex().flex_col().w_full();
                        if !link_only {
                            col = col.child(body).child(div().h(px(8.0)));
                        }
                        col.child(card).into_any_element()
                    }
                    None => body,
                };
                // Someone else's bubble in a group / channel: their name inside
                // it (their colour); the time is hover-only; my own bubble carries
                // its sent/seen label in the footer.
                let (image, color) = match &from {
                    Some(person) => {
                        let state = self.state.read(cx);
                        (
                            state.avatar(&person.avatar_url),
                            Some(state.sender_accent(person, &theme)),
                        )
                    }
                    None => (None, None),
                };
                // The name and the disc are one profile target (`ui::profile_target`):
                // a click opens the person's pane, a second one closes it.
                let person_page = |this: &Self, person: &Person, cx: &Context<Self>| {
                    let sid = this.chat_id.as_ref().and_then(|c| {
                        this.state.read(cx).chats.iter().find_map(|(pid, tabs)| {
                            tabs.iter().any(|t| &t.id == c).then(|| pid.clone())
                        })
                    });
                    crate::app::ProfilePage::User {
                        handle: person.handle.clone(),
                        space_id: sid,
                    }
                };
                let name_el = match (&from, color) {
                    (Some(person), Some(color)) if named => Some(
                        profile_target::attach(
                            self.sender_name_label(&row.id, person.display_name(), color, false),
                            &self.state,
                            person_page(self, person, cx),
                            &person.display_name(),
                            cx,
                        )
                        .into_any_element(),
                    ),
                    _ => None,
                };
                let bubble = self
                    .menu_target(
                        div().id(SharedString::from(format!("{}#bubble", row.id))),
                        &row.message_id,
                        cx,
                    )
                    // Someone else's bubble takes the people share of the lane beside
                    // the disc; mine takes its share of the column (people chats wider).
                    .max_w(relative_width(if from.is_some() {
                        PEOPLE_BUBBLE_FRACTION
                    } else {
                        own_bubble_fraction(self.people)
                    }))
                    .px(px(BUBBLE_PAD_X))
                    .py(px(BUBBLE_PAD_Y))
                    // A card-only bubble keeps a tight even inset around the card.
                    .when(link_only, |b| b.px(px(6.0)).py(px(6.0)))
                    .rounded(px(BUBBLE_RADIUS))
                    .bg(theme.bubble)
                    .text_color(theme.bubble_text())
                    .text_size(px(CHAT_BODY))
                    .line_height(px(CHAT_LINE_HEIGHT))
                    .child(sender_bubble_column(
                        name_el,
                        quote_header.map(|q| q.into_any_element()),
                        body,
                    ))
                    // The ghost: the surface fill with a hairline, muted ink — a row that
                    // says a value was here, not a message to read.
                    .when(ghost.is_some(), |b| {
                        b.bg(theme.surface_2)
                            .border_1()
                            .border_color(theme.border_strong)
                            .text_color(theme.text_muted)
                            .debug_selector(|| "secret-ghost".into())
                    });
                if let Some(person) = from {
                    // Someone else: left-aligned, their avatar beside the bubble on
                    // the avatar column. A small coloured left-edge sliver marks a
                    // person message so it reads apart from an agent bubble: the
                    // dominant colour of their picture, else their D79 disc colour.
                    // Must not early-return: the match arm still needs the
                    // COLUMN_MARGIN wrapper (same padding as agent bubbles).
                    let color = color.unwrap_or(theme.text_muted);
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_start()
                        .py(px(ROW_GAP_HALF))
                        .child({
                            let (row_el, reveal) =
                                self.hover_row(&row.id, &row.id, now, window, cx);
                            let actions =
                                self.hover_rail(&row.id, &row.message_id, reveal, &theme, cx);
                            // The disc on the run's first message only; a message
                            // that continues the run keeps the column empty so its
                            // bubble still aligns under the text (grouped messages).
                            let disc_el = if disc {
                                // The disc is the same profile target as the name.
                                profile_target::attach(
                                    div()
                                        .id(SharedString::from(format!("{}#disc", row.id)))
                                        .debug_selector(|| format!("disc#{}", row.id))
                                        .flex_shrink_0()
                                        .child(people::avatar(image, &person, SENDER_DISC, &theme)),
                                    &self.state,
                                    person_page(self, &person, cx),
                                    &person.display_name(),
                                    cx,
                                )
                                .into_any_element()
                            } else {
                                div().w(px(SENDER_DISC)).flex_shrink_0().into_any_element()
                            };
                            row_el
                                .justify_start()
                                .items_end()
                                .pl(px(AVATAR_COLUMN_INSET))
                                .gap(px(SENDER_DISC_GAP))
                                .child(disc_el)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .child(
                                            bubble
                                                .debug_selector(|| {
                                                    format!("person-bubble#{}", row.id)
                                                })
                                                .bg(theme.bubble_assistant)
                                                .text_color(theme.text)
                                                .border_l_2()
                                                .border_color(Theme::with_alpha(color, 0.7)),
                                        )
                                        .child(actions.ml(px(6.0)))
                                        .children(
                                            hover_time(time, reveal, &theme).map(|t| t.ml(px(8.0))),
                                        ),
                                )
                        })
                        .into_any_element()
                } else {
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(3.0))
                        .py(px(ROW_GAP_HALF))
                        .when_some(sender_label, |el, label| {
                            el.child(
                                div()
                                    .pr(px(6.0))
                                    .text_size(px(TEXT_TINY))
                                    .text_color(theme.text_muted)
                                    .child(label),
                            )
                        })
                        .child({
                            // My bubble sits right, so its outer edge (and the
                            // hover-only time) is on the left. Its delivery state is
                            // its opacity: pending < sent < seen; failed is full with
                            // a red glyph at the outer edge (no captions).
                            let (row_el, reveal) =
                                self.hover_row(&row.id, &row.id, now, window, cx);
                            let actions =
                                self.hover_rail(&row.id, &row.message_id, reveal, &theme, cx);
                            let delivery = delivery.unwrap_or(Delivery::Delivered);
                            row_el
                                .justify_end()
                                .children(hover_time(time, reveal, &theme).map(|t| t.mr(px(8.0))))
                                .child(actions.mr(px(6.0)))
                                .children((delivery == Delivery::Failed).then(|| {
                                    self.failed_retry(&row.id, &row.message_id, &theme, cx)
                                        .mr(px(8.0))
                                }))
                                .children(
                                    (delivery == Delivery::Pending)
                                        .then(|| pending_glyph(&theme).mr(px(8.0))),
                                )
                                .child(own_bubble(
                                    // One selector per element: a ghost keeps "secret-ghost".
                                    bubble.when(ghost.is_none(), |b| {
                                        b.debug_selector(|| format!("own-bubble#{}", row.id))
                                    }),
                                    delivery,
                                ))
                        })
                        .into_any_element()
                }
            }
            RowKind::Markdown {
                block,
                live,
                bubble,
                first,
                last,
                fill,
                time,
                lane,
                quote,
                named,
                disc,
            } => {
                let rail_centre = (last && !live)
                    .then(|| self.bubble_rail_centre(ix))
                    .flatten();
                self.markdown_bubble(
                    &row.id,
                    &row.message_id,
                    block,
                    bubble,
                    live,
                    first,
                    last,
                    fill,
                    time,
                    lane,
                    quote,
                    named,
                    disc,
                    rail_centre,
                    now,
                    &theme,
                    &selectable,
                    window,
                    cx,
                )
            }
            RowKind::NoAnswer {
                seed,
                name,
                time,
                lane,
                quote,
                named,
                disc,
            } => {
                let hint = name.as_deref().unwrap_or("Agent");
                let (resolved, _, _, _) = self.agent_identity(&seed, hint, cx);
                let who = if resolved == "Agent" || resolved.trim().is_empty() {
                    "The agent".to_owned()
                } else {
                    resolved
                };
                let text = format!("{who} didn't answer. Ask again.");
                let block = Block::Paragraph(text.clone());
                self.markdown_bubble(
                    &row.id,
                    &row.message_id,
                    block,
                    BubbleText::single(text),
                    false,
                    true,
                    true,
                    false,
                    time,
                    lane,
                    quote,
                    named,
                    disc,
                    None,
                    now,
                    &theme,
                    &selectable,
                    window,
                    cx,
                )
            }
            RowKind::Hidden {
                count,
                sender: _,
                handle,
                name,
            } => {
                let who = name
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| format!("@{handle}"));
                let person = Person {
                    handle: handle.clone(),
                    ..Person::default()
                };
                let hidden_person = self
                    .state
                    .read(cx)
                    .person_by_handle(&handle)
                    .unwrap_or(person);
                let (line, unblock) = crate::ui::blocked::hidden_row(&row.id, count, &who, &theme);
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .pt(px(STAMP_PAD_TOP))
                    .pb(px(STAMP_PAD_BOTTOM))
                    .child(line.child(unblock.on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(TranscriptEvent::Unblock {
                            person: hidden_person.clone(),
                        });
                    }))))
                    .into_any_element()
            }
            RowKind::Stamp { label } => div()
                .w_full()
                .flex()
                .justify_center()
                .pt(px(STAMP_PAD_TOP))
                .pb(px(STAMP_PAD_BOTTOM))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                // System / time lines can carry a tracking URL; paint and open
                // it like a chat link.
                .child(styled_text(
                    &label,
                    theme.text_muted,
                    &theme,
                    None,
                    now,
                    0,
                    &selectable.bubble(format!("{}.s", row.id), inline_plain(&label)),
                    0,
                ))
                .into_any_element(),
            RowKind::Reactions {
                reactions,
                mine,
                lane,
            } => {
                // iPhone parity: small capsules under the bubble, mine tinted;
                // click toggles my reaction. Hover shows who used that emoji
                // in a floating tip (iPhone `reactionSenders`) — never an
                // in-flow names row, which grew/shrunk the message and made
                // the list jump while the pointer sat on the chip
                // ([feedback app-01M61440FP54FHVZ69QWEVD844]).
                let me = format!("user:{}", self.state.read(cx).me_id());
                let chat_id = self.chat_id.clone().unwrap_or_default();
                let chips_selector: SharedString = format!("reactions#{}", row.message_id).into();
                let mut chips = div()
                    .debug_selector(move || chips_selector.to_string())
                    .flex()
                    .flex_wrap()
                    .when(mine, |el| el.justify_end())
                    .gap(px(4.0));
                for (emoji, senders) in &reactions {
                    let on = senders.contains(&me);
                    let (state, chat, message_id, e) = (
                        self.state.clone(),
                        chat_id.clone(),
                        row.message_id.clone(),
                        emoji.clone(),
                    );
                    let names: SharedString = senders
                        .iter()
                        .map(|s| self.reaction_sender_label(s, &me, cx))
                        .collect::<Vec<_>>()
                        .join(", ")
                        .into();
                    let accent_soft = Theme::with_alpha(theme.accent, 0.22);
                    let accent_edge = Theme::with_alpha(theme.accent, 0.55);
                    // Hover is paint only (the token's wash over the chip's own
                    // fill): the border, the pads, the fixed height and the
                    // count's weight are the same at rest and under the pointer,
                    // so the chip's box never moves ([feedback
                    // reaction-pill-hover-jitter-2026-10-28], `reaction_chip_tests`).
                    let fill = if on {
                        accent_soft
                    } else {
                        theme.bubble_assistant
                    };
                    let selector: SharedString = format!("reaction-chip#{emoji}").into();
                    chips = chips.child(
                        div()
                            .id(SharedString::from(format!("{}#{}", row.id, emoji)))
                            .debug_selector(move || selector.to_string())
                            .flex()
                            .items_center()
                            .gap(px(3.0))
                            .h(px(REACTION_CHIP_HEIGHT))
                            .px(px(REACTION_CHIP_PAD_X))
                            .rounded_full()
                            .border_1()
                            .border_color(if on { accent_edge } else { theme.border })
                            .cursor_pointer()
                            .map(|el| highlight::washed(el, fill, &theme))
                            .tooltip(move |_, cx| {
                                cx.new(|_| ReactionSendersTip(names.clone())).into()
                            })
                            .child(
                                div()
                                    .text_size(px(REACTION_CHIP_EMOJI))
                                    .line_height(px(REACTION_CHIP_EMOJI))
                                    .child(emoji.clone()),
                            )
                            .when(senders.len() > 1, |el| {
                                el.child(
                                    div()
                                        .text_size(px(REACTION_CHIP_COUNT))
                                        .line_height(px(REACTION_CHIP_COUNT))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.text_muted)
                                        .child(senders.len().to_string()),
                                )
                            })
                            .on_click(move |_, _, cx| {
                                let (chat, message_id, e) =
                                    (chat.clone(), message_id.clone(), e.clone());
                                state
                                    .update(cx, |s, cx| s.toggle_reaction(chat, message_id, e, cx));
                            }),
                    );
                }
                div()
                    .debug_selector(|| "reaction-row".into())
                    .w_full()
                    .flex()
                    .when(mine, |el| el.justify_end())
                    .when(!mine, |el| el.justify_start())
                    // The bubble's own left edge: after the avatar lane when its row
                    // has one (`SENDER_LANE`, grouped continuations included).
                    .when(lane && !mine, |el| el.pl(px(SENDER_LANE)))
                    .mt(px(-2.0))
                    .pb(px(ROW_GAP_HALF))
                    .child(chips)
                    .into_any_element()
            }
            RowKind::Handoff { names, total } => {
                // "Messaged ◉ A" while only the brief went out; "N messages with
                // ◉ A, ◉ B and ◉ C" once replies came back (Grok refs 13/58).
                let lead = if total <= names.len() as u32 {
                    "Messaged".to_string()
                } else {
                    format!("{total} messages with")
                };
                self.agent_line(&lead, &names, &theme, cx)
            }
            RowKind::RepliedTo { name } => self.agent_line("Replied to", &[name], &theme, cx),
            RowKind::Attachment {
                attachment,
                part_id,
                mine,
                sender,
                named,
                disc,
                time,
                forwarded_from,
                quote,
            } => {
                // The blob `ref`, or `url:<url>` for a picked GIF that lives with its
                // provider (ARB-544) — the sync layer fetches either.
                let preview = match attachment.preview_key() {
                    None => attachments::Preview::Unavailable,
                    Some(k) => {
                        let state = self.state.read(cx);
                        match state.card_thumbnail(&k) {
                            Some(image) => attachments::Preview::Ready(image),
                            None if state.thumbnails_failed.contains(&k) => {
                                attachments::Preview::Unavailable
                            }
                            None => attachments::Preview::Loading,
                        }
                    }
                };
                // Click → the media viewer on this attachment (feedback media-viewer-2026-10-23).
                let open: Option<attachments::OpenFn> = self.chat_id.clone().map(|chat_id| {
                    let state = self.state.clone();
                    let message_id = row.message_id.clone();
                    Rc::new(move |cx: &mut App| {
                        state.update(cx, |s, cx| {
                            s.open_media_in_chat(&chat_id, &message_id, &part_id, cx)
                        });
                    }) as attachments::OpenFn
                });
                // A reply's quote header above the first card, the same one a text
                // bubble carries ([feedback picture-reply-quote-mac-2026-10-28]).
                let quote_header = quote.map(|q| {
                    self.quote_header(&row.id, q, theme.text, cx)
                        .into_any_element()
                });
                // Same reply · copy rail and right-click menu as text bubbles
                // (feedback app-01M5N8ZQVWC63V54HM70D9SJ4H): an image / file
                // row was a bare card with no way to reply.
                let card = self
                    .menu_target(
                        div().id(SharedString::from(format!("{}#media", row.id))),
                        &row.message_id,
                        cx,
                    )
                    .child(attachments::card(
                        &row.id,
                        &attachment,
                        preview,
                        forwarded_from.as_deref(),
                        quote_header,
                        open,
                        self.state.clone(),
                        &theme,
                    ));
                let (row_el, reveal) = self.hover_row(&row.id, &row.id, now, window, cx);
                let actions = self.hover_rail(&row.id, &row.message_id, reveal, &theme, cx);
                if mine {
                    // My media sits right; the rail is on its outer (left) edge.
                    div()
                        .w_full()
                        .py(px(4.0))
                        .flex()
                        .flex_col()
                        .items_end()
                        .child(
                            row_el
                                .justify_end()
                                .items_center()
                                .children(hover_time(time, reveal, &theme).map(|t| t.mr(px(8.0))))
                                .child(actions.mr(px(6.0)))
                                .child(card),
                        )
                        .into_any_element()
                } else if let Some(sender) = sender {
                    // Someone else's picture / file in a people chat: their disc on the
                    // avatar column and, on the message's first card when the run changes
                    // sender, their name above it — the same identity, colour and profile
                    // target as their bubble ([feedback forward-attachments-mac-2026-10-27]).
                    let (disc_el, name_el) =
                        self.attachment_sender(&row.id, &sender, named, &theme, cx);
                    // Earlier cards of the message keep the lane free; the disc sits on
                    // the last row (markdown_bubble's rule).
                    let disc_el = if disc {
                        disc_el
                    } else {
                        div().w(px(SENDER_DISC)).flex_shrink_0().into_any_element()
                    };
                    div()
                        .w_full()
                        .py(px(4.0))
                        .flex()
                        .flex_col()
                        .items_start()
                        .child(
                            row_el
                                .justify_start()
                                .items_end()
                                .pl(px(AVATAR_COLUMN_INSET))
                                .gap(px(SENDER_DISC_GAP))
                                .child(disc_el)
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        .children(name_el.map(|n| div().pl(px(6.0)).child(n)))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .child(card)
                                                .child(actions.ml(px(6.0)))
                                                .children(
                                                    hover_time(time, reveal, &theme)
                                                        .map(|t| t.ml(px(8.0))),
                                                ),
                                        ),
                                ),
                        )
                        .into_any_element()
                } else {
                    // An agent's attachment in its own 1:1 chat: no avatar column there.
                    div()
                        .w_full()
                        .py(px(4.0))
                        .flex()
                        .flex_col()
                        .items_start()
                        .child(
                            row_el
                                .justify_start()
                                .items_center()
                                .child(card)
                                .child(actions.ml(px(6.0)))
                                .children(hover_time(time, reveal, &theme).map(|t| t.ml(px(8.0)))),
                        )
                        .into_any_element()
                }
            }
            RowKind::AskUser { text } => {
                cards::status_row(StatusKind::AskUser, &text, false, &theme, cx).into_any_element()
            }
            RowKind::Question { question_id, lane } => {
                let Some(chat_id) = self.chat_id.clone() else {
                    return div().into_any_element();
                };
                let widget = self
                    .state
                    .read(cx)
                    .messages_of(&chat_id)
                    .iter()
                    .find_map(|m| {
                        m.parts.iter().find_map(|p| match p {
                            Part::Question {
                                question_id: qid,
                                widget: Some(w),
                                ..
                            } if *qid == question_id => Some(w.clone()),
                            _ => None,
                        })
                    });
                let Some(widget) = widget else {
                    return div().into_any_element();
                };
                let state = self.state.clone();
                let card = self
                    .cards
                    .entry(question_id.clone())
                    .or_insert_with(|| {
                        cx.new(|cx| QuestionCard::new(state, chat_id, question_id, &widget, cx))
                    })
                    .clone();
                card_row(card, lane).into_any_element()
            }
            RowKind::Approval { mut view, lane } => {
                let chat_id = self.chat_id.clone().unwrap_or_default();
                // Secret hand-off request card: a stateful card with the masked field.
                if view.payload.get("kind").and_then(serde_json::Value::as_str)
                    == Some("secret_offer")
                {
                    let project_id = view
                        .payload
                        .get("projectId")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| {
                            self.state
                                .read(cx)
                                .project_of_chat(&chat_id)
                                .map(|p| p.id.clone())
                        })
                        .unwrap_or_default();
                    let suggested = view
                        .payload
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let state = self.state.clone();
                    let approval_id = view.approval_id.clone();
                    let card = self
                        .secret_cards
                        .entry(approval_id.clone())
                        .or_insert_with(|| {
                            cx.new(|cx| {
                                SecretOfferCard::new(
                                    state,
                                    chat_id.clone(),
                                    approval_id,
                                    project_id,
                                    &suggested,
                                    cx,
                                )
                            })
                        })
                        .clone();
                    return card_row(card, lane).into_any_element();
                }
                // #1544: the standing rows name the agent ("Always allow Doug in /tmp").
                view.agent = self
                    .state
                    .read(cx)
                    .project_of_chat(&chat_id)
                    .map(|p| p.name.clone());
                let approval_id = view.approval_id.clone();
                let (state_a, chat_a, id_a) =
                    (self.state.clone(), chat_id.clone(), approval_id.clone());
                let (state_r, chat_r, id_r) =
                    (self.state.clone(), chat_id.clone(), approval_id.clone());
                let (state_s, chat_s, id_s) = (self.state.clone(), chat_id, approval_id);
                let card = approval_card(
                    &row.id,
                    &view,
                    &theme,
                    move |cx| {
                        state_a.update(cx, |s, cx| s.approve(chat_a.clone(), id_a.clone(), cx));
                    },
                    move |cx| {
                        state_r
                            .update(cx, |s, cx| s.reject(chat_r.clone(), id_r.clone(), None, cx));
                    },
                    move |choice, cx| {
                        state_s.update(cx, |s, cx| {
                            s.answer_approval(chat_s.clone(), id_s.clone(), choice, cx)
                        });
                    },
                );
                card_row(card, lane).into_any_element()
            }
            RowKind::Error { code, message } => {
                let state = self.state.clone();
                let chat_id = self.chat_id.clone().unwrap_or_default();
                let message_id = row.message_id.clone();
                cards::error_chip(&row.id, &code, &message, &theme, move |cx| {
                    state.update(cx, |s, cx| s.retry(chat_id.clone(), message_id.clone(), cx));
                })
                .into_any_element()
            }
            RowKind::Glyph { glyph } => div()
                .w_full()
                .py(px(2.0))
                .text_size(px(10.0))
                .text_color(theme.text_muted)
                .child(glyph)
                .into_any_element(),
            RowKind::Details => {
                let state = self.state.clone();
                let chat_id = self.chat_id.clone().unwrap_or_default();
                let message_id = row.message_id.clone();
                div()
                    .id(SharedString::from(row.id.clone()))
                    .w_full()
                    .pb(px(4.0))
                    .text_size(px(11.0))
                    .text_color(theme.text_faint)
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.text_muted))
                    .child("Show details ▸")
                    .on_click(move |_, _, cx| {
                        state.update(cx, |s, cx| {
                            s.open_details(
                                DetailsTarget::Message {
                                    chat_id: chat_id.clone(),
                                    message_id: message_id.clone(),
                                },
                                cx,
                            )
                        });
                    })
                    .into_any_element()
            }
            RowKind::Speaker { cached, failed } => {
                let state = self.state.clone();
                let message_id = row.message_id.clone();
                // The failure lives under the reply it belongs to, as a retry.
                let label = speaker_label(cached, failed);
                div()
                    .id(SharedString::from(row.id.clone()))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .w(px(if failed { 220.0 } else { 96.0 }))
                    .pb(px(ROW_GAP_HALF))
                    .text_size(px(11.0))
                    .text_color(theme.text_faint)
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.text_muted))
                    .child(div().text_size(px(12.0)).child("▶"))
                    .child(label)
                    .on_click(move |_, _, cx| {
                        state.update(cx, |s, cx| s.replay_speech(message_id.clone(), cx));
                    })
                    .into_any_element()
            }
            RowKind::BottomInset => {
                // No chrome — just the clearance height. Return early so the
                // shared column padding / highlight wrapper does not paint a
                // strip over scrolling bubbles.
                return div().w_full().h(px(BOTTOM_INSET)).into_any_element();
            }
        };
        let highlighted =
            self.state.read(cx).highlight_message.as_deref() == Some(row.message_id.as_str());
        let turn_start = self.is_turn_start(ix);
        let measure = Self::measure_into(&self.measured, Some(row_frame_id.clone()));
        div()
            .id(SharedString::from(format!("{row_frame_id}#frame")))
            .relative()
            .w_full()
            .child(measure)
            .child(
                column_row(inner)
                    .when(turn_start, |el| el.pt(px(TURN_GAP_EXTRA)))
                    .when(highlighted, |el| {
                        el.bg(Theme::with_alpha(theme.accent, 0.14))
                    }),
            )
            .into_any_element()
    }
}

/// The one row frame every chat kind's transcript row sits in: the column's
/// side margin, centred, capped at [`COLUMN_MAX_WIDTH`] — the composer's own
/// geometry (`shell::render_center`), so bubbles, captions, the working line
/// and the input share their left and right edges.
pub fn column_row(inner: AnyElement) -> Div {
    div()
        .w_full()
        .flex()
        .justify_center()
        .px(px(COLUMN_MARGIN))
        .child(
            div()
                .w_full()
                .max_w(px(COLUMN_MAX_WIDTH))
                .debug_selector(|| "chat-column".into())
                .child(inner),
        )
}

/// Where an agent bubble goes in its row: the rest of the row after the
/// sender lane, minus Grok's fixed [`ASSISTANT_RIGHT_GAP`], and never wider
/// than [`ASSISTANT_MAX_WIDTH`] ([`assistant_cap`]). The bubble's
/// `max_w_full` is the inner cap frame's width; the hover rail, absolute at
/// the bubble's right edge, lands in the gap after it.
fn assistant_slot(bubble: impl IntoElement) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .pr(px(ASSISTANT_RIGHT_GAP))
        .flex()
        .items_center()
        .debug_selector(|| "chat-bubble-slot".into())
        .child(
            div()
                .w_full()
                .max_w(px(ASSISTANT_MAX_WIDTH))
                .min_w_0()
                .flex()
                .items_center()
                .debug_selector(|| "chat-bubble-cap".into())
                .child(bubble),
        )
}

/// Where an incoming bubble goes in a people-chat row: the rest of the row
/// after the sender lane, the bubble at most [`PEOPLE_BUBBLE_FRACTION`] of it
/// ([`people_cap`]). The hover rail, absolute at the bubble's right edge,
/// lands in the free 20 %.
fn people_slot(bubble: impl IntoElement) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .debug_selector(|| "chat-bubble-slot".into())
        .child(
            div()
                .w_full()
                .max_w(relative_width(PEOPLE_BUBBLE_FRACTION))
                .min_w_0()
                .flex()
                .items_center()
                .debug_selector(|| "chat-bubble-cap".into())
                .child(bubble),
        )
}

/// The slot an incoming bubble sits in: the people slot when the row carries
/// a sender lane, Grok's agent slot otherwise.
fn incoming_slot(bubble: impl IntoElement, lane: bool) -> Div {
    if lane {
        people_slot(bubble)
    } else {
        assistant_slot(bubble)
    }
}

/// A question / approval card is a transcript row like a bubble (#1374, #1440 follow-up): the
/// same left edge as an incoming bubble — after the sender's avatar lane in a people chat — and
/// the same cap width (`assistant_slot`, or the people slot after a lane). The lane is left
/// empty: the card belongs to the agent's run and the disc already sits on its bubble.
pub fn card_row(card: impl IntoElement, lane: bool) -> Div {
    div()
        .w_full()
        .flex()
        .when(lane, |el| el.pl(px(SENDER_LANE)))
        .debug_selector(|| "chat-card-row".into())
        .child(incoming_slot(card, lane))
}

fn relative_width(fraction: f32) -> gpui::DefiniteLength {
    gpui::relative(fraction)
}

impl Render for Transcript {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.step_spring(window, cx);
        let theme = Theme::of(cx).clone();
        // Grok Mac (ref 58): a 40 pt ↓ circle centred above the composer once
        // the user has scrolled up; clicking re-pins and lands on the newest
        // page (`jump_to_latest`).
        let max = f32::from(self.list.max_offset_for_scrollbar().y);
        let cur = -f32::from(self.list.scroll_px_offset_for_scrollbar().y);
        let show_jump = !self.pinned_to_bottom && max - cur > JUMP_BUTTON_AFTER_PX;
        // The landing cue: the newest page rises into place and brightens.
        let arrive = self.arrive_progress();
        if arrive.is_some() {
            window.request_animation_frame();
        }
        let (arrive_rise, arrive_alpha) = arrive.map_or((0.0, 1.0), |e| {
            (
                (1.0 - e) * JUMP_ARRIVE_RISE_PX,
                JUMP_ARRIVE_FROM_ALPHA + (1.0 - JUMP_ARRIVE_FROM_ALPHA) * e,
            )
        });
        self.apply_rig_select(window, cx);
        // A selection lives as long as the transcript keeps keyboard focus:
        // clicking into the composer (or anywhere else) drops the highlight,
        // so a highlight always means ⌘C will copy it.
        let focus = self.selection.read(cx).focus_handle.clone();
        if !focus.is_focused(window) {
            self.selection.update(cx, |s, cx| {
                if s.clear_settled() {
                    cx.notify();
                }
            });
        }
        div()
            .relative()
            .size_full()
            .key_context("Transcript")
            .track_focus(&focus)
            .child(Self::measure_into(&self.measured, None))
            .on_action(cx.listener(Self::copy_selection))
            .on_scroll_wheel(cx.listener(|this, event, _, cx| this.on_wheel(event, cx)))
            // Drag-scroll: move/up live on the transcript so a drag that
            // leaves the row still finishes (iPhone parity). A press on bubble
            // text is the selection's (its listener ran first and armed the
            // drag), so the pointer is left to it — up, down or across
            // bubbles — and never scrolls the list instead.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if event.modifiers.control || event.modifiers.shift {
                        return;
                    }
                    if this.selection.read(cx).is_dragging() {
                        return;
                    }
                    // The header and its drag catchment float over the list: a
                    // press there is the window's to move. While AppKit drags the
                    // window the pointer keeps jittering against the lagging frame,
                    // and as a drag-scroll each jitter broke or restored the pin,
                    // so the presence row blinked ([feedback
                    // mac-typing-line-drag-flicker-2026-10-29]).
                    if cx
                        .try_global::<WindowDragPress>()
                        .is_some_and(|press| press.start.is_some())
                    {
                        return;
                    }
                    this.begin_drag_scroll(event.position);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                this.on_drag_scroll_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _cx| {
                    this.end_drag_scroll();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _cx| {
                    this.end_drag_scroll();
                }),
            )
            // Grok's header band (Jacob, Oct 22 12:49 CR, superseding the opaque
            // band): the list runs up under the floating pill and fades out
            // over `HEADER_FADE_BAND` — Grok's text is gone at the window edge
            // and fully readable ~50 pt down ([feedback mac-header-fade-grok]).
            .child(
                div()
                    .relative()
                    .size_full()
                    .top(px(arrive_rise))
                    .opacity(arrive_alpha)
                    .child(edge_faded(
                        HEADER_FADE_BAND,
                        true,
                        false,
                        list(self.list.clone(), cx.processor(Self::render_row)).size_full(),
                    )),
            )
            .when(show_jump, |el| {
                el.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(12.0))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .id("jump-to-latest")
                                .size(px(40.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .border_1()
                                .border_color(theme.border_strong)
                                .shadow_md()
                                .cursor_pointer()
                                .map(|el| highlight::washed(el, theme.card(), &theme))
                                .child(sf_symbols::glyph("arrow.down", 15.0, theme.text))
                                .on_click(cx.listener(|this, _, _, cx| this.jump_to_latest(cx)))
                                .with_animation(
                                    "jump-in",
                                    Animation::new(Duration::from_millis(180)),
                                    |el, t| el.opacity(t),
                                ),
                        ),
                )
            })
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// Who an agent lane belongs to: display name, disc seed, D79 colour, D92
/// leaf pair in effect, and the project id when the lane names one.
struct AgentIdentity {
    name: String,
    seed: String,
    color_index: Option<u8>,
    leaf: (Option<u8>, Option<u8>),
    image: Option<std::sync::Arc<gpui::RenderImage>>,
    project_id: Option<Id>,
}

fn hash_version<T: Hash>(value: &T, live: bool) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h);
    (h.finish() << 1) | (live as u64)
}

/// Text of the speaker row under a reply (F11). A failure is said under the
/// reply it belongs to and reads as a retry; it never becomes a global line.
pub fn speaker_label(cached: bool, failed: bool) -> &'static str {
    if failed {
        "Could not play — try again"
    } else if cached {
        "Replay"
    } else {
        "Speak"
    }
}

fn block_text_len(block: &Block) -> usize {
    match block {
        Block::Paragraph(t) | Block::Heading { text: t, .. } => t.len(),
        Block::Quote(inner) => inner.iter().map(|b| block_text_len(b) + 1).sum(),
        Block::Bullets(items) | Block::Numbered(items) => items.iter().map(|s| s.len() + 1).sum(),
        Block::Code { code, .. } => code.len(),
        Block::Table { header, rows } => {
            header.iter().map(|s| s.len() + 1).sum::<usize>()
                + rows.iter().flatten().map(|s| s.len() + 1).sum::<usize>()
        }
        Block::Math { source } => source.len(),
        Block::Chart { table, .. } => table.len(),
        Block::LinkPreview { preview, .. } => preview.title.len(),
    }
}

/// "Routine · Morning stand-up" for a prompt a routine injected; `None` for the
/// user's own messages (`sender` empty or `user`).
fn sender_caption(message: &Message) -> Option<String> {
    let sender = message.sender.as_str();
    if sender.is_empty() || sender == "user" || sender.starts_with("user:") {
        return None;
    }
    if let Some(id) = sender.strip_prefix("routine:") {
        let name = message.routine_name.as_deref().unwrap_or(id);
        return Some(format!("Routine · {name}"));
    }
    Some(sender.replace(':', " · "))
}

/// The bubble's own clock ("9:41 PM"), the iPhone's drag-to-reveal time.
/// UTType-style labels some clients leave as the only text on a media-only send.
fn is_generic_media_label(text: &str) -> bool {
    matches!(
        text.trim().to_ascii_lowercase().as_str(),
        "image" | "photo" | "video" | "attachment" | "(attachment)"
    )
}

fn clock_label(ms: u64) -> Option<String> {
    use chrono::{Local, TimeZone};
    if ms == 0 {
        return None;
    }
    Local
        .timestamp_millis_opt(ms as i64)
        .single()
        .map(|t| t.format("%-I:%M %p").to_string())
}

/// Where my own bubble is: pending until the relay ACKs, failed when it
/// refused, delivered otherwise. `None` for someone else's bubble.
pub fn delivery_state(message: &Message, is_mine: bool) -> Option<Delivery> {
    if !is_mine {
        return None;
    }
    if message.pending {
        return Some(Delivery::Pending);
    }
    if message.status == MessageStatus::Error {
        return Some(Delivery::Failed);
    }
    Some(Delivery::Delivered)
}

/// Grok keeps the rail inside the chat: a bubble taller than the pane gets
/// its rail at the centre of the visible part, this far in from the list's
/// edges (the top edge is the header's fade band, where nothing is readable).
const RAIL_EDGE_INSET: f32 = 8.0;

/// The hover rail's vertical centre for a multi-row bubble, measured from the
/// top of its last row: the middle of the whole bubble — the rows above it
/// (`None` for one the list has not laid out, which only happens above the
/// viewport) plus `last` — clamped so the rail stays inside `viewport` (Grok,
/// Jacob's grok-chat-2 ref: the ☺ ↩ ⋯ sit at the table bubble's mid-height).
///
/// `insets` are the row padding outside the bubble: above its first row
/// (the half gap, plus the turn gap when the message opens a turn) and
/// below its last.
fn rail_centre(
    rows_above: &[Option<Bounds<Pixels>>],
    last: Bounds<Pixels>,
    viewport: Bounds<Pixels>,
    insets: (Pixels, Pixels),
) -> Pixels {
    let mut top = last.top();
    let mut unknown = false;
    for row in rows_above {
        match row {
            Some(b) => top = top.min(b.top()),
            None => unknown = true,
        }
    }
    top += insets.0;
    if unknown {
        // Rows above the scroll top are not measured: the bubble starts at
        // least there, and only the visible part matters for the clamp below.
        top = top.min(viewport.top());
    }
    let bottom = last.bottom() - insets.1;
    let centre = (top + bottom) / 2.0;
    let half = px(RAIL_BUTTON / 2.0 + RAIL_EDGE_INSET);
    let low = viewport.top() + px(HEADER_FADE_BAND) + half;
    let high = viewport.bottom() - half;
    let centre = clamp_or_keep(centre, low, high);
    // Never outside the bubble itself (a short bubble at the very edge). The bounds come
    // from the list's measured rows, which can be of two layout generations while rows
    // reflow (a row inserted above, a table that grew, a scroll): the span may then be
    // shorter than the button or even inverted — `Ord::clamp` would panic on it
    // (`assertion failed: min <= max`; Mac 0.6.268 aborted on exactly that, 2026-10-26).
    let centre = clamp_or_keep(
        centre,
        top + px(RAIL_BUTTON / 2.0),
        bottom - px(RAIL_BUTTON / 2.0),
    );
    centre - last.top()
}

/// `value.clamp(low, high)` when the bounds are in order, `value` itself otherwise — the
/// rail may sit a few points off on a frame whose measurements are mid-reflow, never panic.
fn clamp_or_keep(value: Pixels, low: Pixels, high: Pixels) -> Pixels {
    if high < low {
        value
    } else {
        value.clamp(low, high)
    }
}

/// The frame that hangs the rail (and the hover time) off a bubble's right
/// edge, out of the flow, in the column's [`ASSISTANT_RIGHT_GAP`]: centred on
/// the bubble's own height, or — for a bubble drawn as several rows — at
/// `centre` pixels from this row's top ([`rail_centre`]).
fn hover_rail_frame(centre: Option<Pixels>) -> Div {
    let frame = div()
        .absolute()
        .left(relative_width(1.0))
        .pl(px(6.0))
        .flex()
        .items_center()
        .debug_selector(|| "bubble-rail".into());
    match centre {
        Some(c) => frame.top(c - px(RAIL_BUTTON / 2.0)).h(px(RAIL_BUTTON)),
        None => frame.top_0().bottom_0(),
    }
}

/// The 11 pt muted time beside a bubble at the row's current hover opacity.
/// Nothing is rendered at rest: the time exists only while the row is hovered
/// or fading out (docs/mac-design-rules.md: timestamps are never always-on).
fn hover_time(time: Option<String>, opacity: f32, theme: &Theme) -> Option<Div> {
    if opacity <= 0.0 {
        return None;
    }
    time.map(|label| {
        div()
            .flex_shrink_0()
            .whitespace_nowrap()
            .text_size(px(11.0))
            .text_color(theme.text_muted)
            .opacity(opacity)
            .debug_selector(|| "bubble-time".into())
            .child(label)
    })
}

/// My own bubble at its delivery opacity (see [`Delivery::opacity`]). Applied
/// to the bubble itself, never a wrapper: the bubble's cap is
/// `relative_width(own_bubble_fraction(..))` of its parent, and a wrapper that is itself
/// content-sized shrinks that cap (v0.6.223 wrapped own bubbles a word early).
fn own_bubble<E: Styled>(bubble: E, delivery: Delivery) -> E {
    bubble.opacity(delivery.opacity())
}

/// The small clock beside a bubble the relay has not ACKed yet (with the
/// 0.5 opacity): "on its way", never "not sent".
fn pending_glyph(theme: &Theme) -> Div {
    div()
        .flex_shrink_0()
        .size(px(RAIL_GLYPH))
        .flex()
        .items_center()
        .justify_center()
        .debug_selector(|| "own-bubble-pending-glyph".into())
        .child(sf_symbols::glyph("hourglass", 12.0, theme.text_muted))
}

/// The mark beside a bubble the relay refused: the red glyph, always visible, outside the
/// bubble on its outer edge (Jacob's own-bubble rule). The words and Try again are the
/// composer's note ([feedback error-messages-mac-2026-10-26]); [`Transcript::failed_retry`]
/// still makes the mark itself clickable.
fn failed_glyph(theme: &Theme) -> Div {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .debug_selector(|| "own-bubble-failed-glyph".into())
        // error-ui: exception failed-bubble-mark (the glyph is the mark, not a message)
        .child(sf_symbols::glyph(
            "exclamationmark.circle",
            RAIL_GLYPH,
            theme.danger,
        ))
}

/// A text block that is really machine output (a JSON object/array) is not
/// prose; it belongs in the Details drawer, never in the transcript.
fn is_json_block(block: &Block) -> bool {
    let text = match block {
        Block::Paragraph(t) => t.as_str(),
        Block::Code { lang, code } if lang.is_empty() || lang.eq_ignore_ascii_case("json") => {
            code.as_str()
        }
        Block::Code { .. }
        | Block::Heading { .. }
        | Block::Quote(_)
        | Block::Bullets(_)
        | Block::Numbered(_)
        | Block::Table { .. }
        | Block::Math { .. }
        | Block::Chart { .. }
        | Block::LinkPreview { .. } => return false,
    };
    let t = text.trim();
    (t.starts_with('{') || t.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(t).is_ok()
}

/// Built rows for one message, keyed by a render version so a streaming
/// upsert rebuilds only that message (#1405 /
/// feedback app-01M5Z3QPDA5BRE84SAPADXX05V,
/// addendum app-01M5Z3T4F4KAMTWA7YKW3CP58H).
#[derive(Default)]
pub struct MessageRowCache {
    entries: HashMap<Id, CachedMessageRows>,
    /// How many messages were built (not served from the cache) on the
    /// last [`build_rows_with_cache`] call. Tests assert a streaming edit
    /// rebuilds one message, not the whole transcript.
    pub built: usize,
    /// The messages that start a sender run on the last call (see
    /// [`RUN_WINDOW_MS`]): their first row opens the turn gap. Rebuilt on
    /// every call, cache hits included.
    pub run_starts: HashSet<Id>,
}

struct CachedMessageRows {
    version: u64,
    rows: Vec<Row>,
    last_stamp_at: u64,
    run_tail: Option<RunTail>,
    run_start: bool,
}

impl MessageRowCache {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.built = 0;
        self.run_starts.clear();
    }
}

/// Fingerprint of the fields that change a message's rows: text, parts,
/// status, completed time, reactions, and the stamp / sender-run context
/// (plus speech / details / thumbs / width-class / link-card flags).
#[allow(clippy::too_many_arguments)]
fn message_render_version(
    message: &Message,
    last_stamp_at: u64,
    continues_run: bool,
    is_last_reply: bool,
    show_details: bool,
    speech: &SpeechRows<'_>,
    me_id: &str,
    thumb_keys: &HashSet<&str>,
    names_senders: bool,
    // Width class: multi-party layout changes who gets a name caption.
    width_class: u8,
    link_previews: bool,
) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    message.id.hash(&mut h);
    std::mem::discriminant(&message.role).hash(&mut h);
    message.sender.hash(&mut h);
    std::mem::discriminant(&message.status).hash(&mut h);
    message.pending.hash(&mut h);
    message.created_at.hash(&mut h);
    message.completed_at.hash(&mut h);
    message.hidden.hash(&mut h);
    message.worker.hash(&mut h);
    message.greeting.hash(&mut h);
    message.waiting_user.hash(&mut h);
    message.waiting_approval.hash(&mut h);
    message.plain_text().hash(&mut h);
    for (emoji, people) in &message.reactions {
        emoji.hash(&mut h);
        for (who, on) in people {
            who.hash(&mut h);
            on.hash(&mut h);
        }
    }
    // Parts: Debug covers every variant; thumb presence is mixed in so a
    // late-decoded preview reloads the attachment row.
    for p in &message.parts {
        format!("{p:?}").hash(&mut h);
        if let Part::Attachment { attachment, .. } = p {
            attachment
                .blob_ref
                .as_deref()
                .is_some_and(|k| thumb_keys.contains(k))
                .hash(&mut h);
        }
    }
    message.sender_name.hash(&mut h);
    message.sender_handle.hash(&mut h);
    message.sender_avatar.hash(&mut h);
    message.sender_avatar_color.hash(&mut h);
    message.sender_icon.hash(&mut h);
    message.in_reply_to.hash(&mut h);
    message.reply_to.hash(&mut h);
    message.forwarded_from.hash(&mut h);
    message.agent_reply_to.hash(&mut h);
    message.asked_by_handle.hash(&mut h);
    message.owner_handle.hash(&mut h);
    message.routine_name.hash(&mut h);
    message.activity.hash(&mut h);
    format!("{:?}", message.turn).hash(&mut h);
    format!("{:?}", message.reply_ref).hash(&mut h);
    format!("{:?}", message.job).hash(&mut h);
    last_stamp_at.hash(&mut h);
    continues_run.hash(&mut h);
    is_last_reply.hash(&mut h);
    show_details.hash(&mut h);
    speech.enabled.hash(&mut h);
    speech.voice_mode.hash(&mut h);
    speech.spoken.contains(&message.id).hash(&mut h);
    speech.failed.contains_key(&message.id).hash(&mut h);
    me_id.hash(&mut h);
    names_senders.hash(&mut h);
    width_class.hash(&mut h);
    // Whether the card draws, not the setting itself: the Link previews toggle and Remove
    // preview (`preview: "off"`) rebuild only the rows that carry a card.
    link_preview_of(message, link_previews)
        .is_some()
        .hash(&mut h);
    h.finish()
}

/// The transcript shows only what the agent said and made: prose, files,
/// questions, errors. Everything the agent did (tool calls, status events,
/// JSON, usage) is hidden; the Details drawer has it. One indicator per chat
/// pulses under the last message while anything runs.
///
/// `thumb_keys` are blob refs that already have a decoded preview; they are
/// mixed into row versions so a late thumbnail reloads the picture row.
/// `names_senders` landed from a parallel feedback; keep it a plain arg
/// rather than a one-off options struct.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub fn build_rows(
    messages: &[Message],
    show_details: bool,
    speech: SpeechRows<'_>,
    me_id: &str,
    thumb_keys: &HashSet<&str>,
    mentions: &Mentions,
    names_senders: bool,
) -> Vec<Row> {
    build_rows_hiding(
        messages,
        show_details,
        speech,
        me_id,
        thumb_keys,
        mentions,
        names_senders,
        &HashSet::new(),
    )
}

/// [`build_rows`] with the senders I blocked (`user:<id>` keys): their messages fold into
/// one [`RowKind::Hidden`] line per run instead of bubbles, and carry no date stamp.
/// Production sync goes through [`build_rows_with_cache`] (same fold, plus the row cache).
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub fn build_rows_hiding(
    messages: &[Message],
    show_details: bool,
    speech: SpeechRows<'_>,
    me_id: &str,
    thumb_keys: &HashSet<&str>,
    mentions: &Mentions,
    names_senders: bool,
    hidden_senders: &HashSet<String>,
) -> Vec<Row> {
    let mut cache = MessageRowCache::default();
    build_rows_with_cache(
        messages,
        show_details,
        speech,
        me_id,
        thumb_keys,
        mentions,
        names_senders,
        false,
        true,
        &mut cache,
        hidden_senders,
    )
}

/// Like [`build_rows`], but reuses per-message rows whose render version is
/// unchanged. A streaming upsert then rebuilds one message, not all of them.
/// Blocked senders fold into [`RowKind::Hidden`] the same way as [`build_rows_hiding`].
/// `link_previews` is Settings → Chat → Link previews (`AppState::link_previews`).
#[allow(clippy::too_many_arguments)]
pub fn build_rows_with_cache(
    messages: &[Message],
    show_details: bool,
    speech: SpeechRows<'_>,
    me_id: &str,
    thumb_keys: &HashSet<&str>,
    mentions: &Mentions,
    names_senders: bool,
    people_space: bool,
    link_previews: bool,
    cache: &mut MessageRowCache,
    hidden_senders: &HashSet<String>,
) -> Vec<Row> {
    cache.built = 0;
    let mine = |m: &Message| {
        m.sender == "user" || m.sender.is_empty() || m.sender == format!("user:{me_id}")
    };
    let mut rows = Vec::new();
    let now = now_ms();
    // One indicator while a recent stream or open hand-off is live; stale
    // `streaming` rows (dead box / leftover under a later user line) do not count.
    let running = Message::chat_is_actively_working(messages, now);
    let mut queued = false;
    let last_reply = messages
        .iter()
        .rposition(|m| m.role == Role::Assistant && !m.is_streaming());
    let mut last_stamp_at: u64 = 0;
    // The sender run the builder is in (who spoke last, when): a message that
    // continues it draws no name and no disc and sits in the tight in-run gap; a
    // date stamp, a system line or a hidden fold ends it ([feedback
    // grouped-messages-mac-2026-10-28]). Names only in chats that name their
    // senders at all (`names_senders`).
    let mut run_tail: Option<RunTail> = None;
    cache.run_starts.clear();
    // The stamp pushed for the current message, with the state it replaced. A
    // message that then draws nothing — a hidden job / worker row, a stream
    // with no words yet — takes its stamp back, so no date line hangs over
    // empty space above the status row ([feedback status-row-gap-mac-2026-10-26]).
    let mut pending_stamp: Option<(usize, u64, Option<RunTail>)> = None;
    let width_class = u8::from(names_senders);
    let mut keep: HashSet<Id> = HashSet::new();
    for (mix, message) in messages.iter().enumerate() {
        keep.insert(message.id.clone());
        retract_dangling_stamp(
            &mut rows,
            &mut pending_stamp,
            &mut last_stamp_at,
            &mut run_tail,
        );
        // Someone I blocked: fold the run into one hidden line, no bubble, no stamp.
        // Cross-message fold — do not serve from the per-message row cache.
        if message.role == Role::User && hidden_senders.contains(&message.sender) {
            match rows.last_mut() {
                Some(Row {
                    kind: RowKind::Hidden { count, sender, .. },
                    version,
                    ..
                }) if *sender == message.sender => {
                    *count += 1;
                    *version = version.wrapping_add(1);
                }
                _ => rows.push(Row {
                    version: hash_version(&message.id, false),
                    id: format!("{}#hidden", message.id),
                    message_id: message.id.clone(),
                    kind: RowKind::Hidden {
                        count: 1,
                        sender: message.sender.clone(),
                        handle: message
                            .sender_handle
                            .clone()
                            .unwrap_or_else(|| "someone".into()),
                        name: message.sender_name.clone(),
                    },
                }),
            }
            run_tail = None;
            continue;
        }
        if people_space && message.is_senderless_blank() {
            continue;
        }
        let is_last_reply = last_reply == Some(mix);
        // Does this message continue the run? Decided before the stamp below:
        // a stamp only lands when the hour gap passed, which is past the run
        // window anyway.
        let continues = continues_run(
            run_tail.as_ref(),
            &message.sender,
            message.created_at,
            carries_quote(message),
        );
        let run_start = !continues;
        let version = message_render_version(
            message,
            last_stamp_at,
            continues,
            is_last_reply,
            show_details,
            &speech,
            me_id,
            thumb_keys,
            names_senders,
            width_class,
            link_previews,
        );
        if let Some(hit) = cache.entries.get(&message.id) {
            if hit.version == version {
                rows.extend(hit.rows.iter().cloned());
                last_stamp_at = hit.last_stamp_at;
                run_tail = hit.run_tail.clone();
                if hit.run_start {
                    cache.run_starts.insert(message.id.clone());
                }
                pending_stamp = None;
                continue;
            }
        }
        cache.built += 1;
        let start = rows.len();
        let streaming = message.is_streaming();
        if message.created_at > 0 && message.created_at.saturating_sub(last_stamp_at) > STAMP_GAP_MS
        {
            pending_stamp = Some((rows.len(), last_stamp_at, run_tail.take()));
            last_stamp_at = message.created_at;
            let label = crate::ui::sidebar::stamp_label(message.created_at);
            rows.push(Row {
                version: hash_version(&label, false),
                id: format!("{}#stamp", message.id),
                message_id: message.id.clone(),
                kind: RowKind::Stamp { label },
            });
        }
        let rows_before_message = rows.len();
        match message.role {
            Role::User => {
                // Secret hand-off: a redacted row carries one `secret_redacted` part and no
                // text; it draws as the ghost bubble ("Secret saved as NAME").
                let ghost = message.parts.iter().find_map(|p| match p {
                    Part::SecretRedacted { name, env, .. } => Some(if name.is_empty() {
                        env.clone()
                    } else {
                        name.clone()
                    }),
                    _ => None,
                });
                // `@handle` → `@Display Name` so the chip shows the name and a copy
                // carries it (mention chips, Oct 23).
                let mut text = match &ghost {
                    Some(name) => format!("Secret saved as {name}"),
                    None => mentions.display(&message.plain_text()),
                };
                // The link preview card; a message that was only the link shows the card
                // alone (iMessage's rule), the full URL stays one hover away on the card.
                let preview = ghost
                    .is_none()
                    .then(|| link_preview_of(message, link_previews))
                    .flatten()
                    .map(|(id, p)| Box::new((id.clone(), p.clone())));
                if let Some(p) = &preview {
                    if is_link_only(&text, &p.1) {
                        text = String::new();
                    }
                }
                let attachment_parts: Vec<(String, Attachment)> = message
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Attachment { id, attachment } => {
                            Some((id.clone(), attachment.clone()))
                        }
                        _ => None,
                    })
                    .collect();
                let has_image = attachment_parts
                    .iter()
                    .any(|(_, a)| a.kind() == AttachmentKind::Image);
                let from = if message.sender.starts_with("user:") && !mine(message) {
                    Some(Person {
                        id: message.sender.trim_start_matches("user:").to_string(),
                        handle: message
                            .sender_handle
                            .clone()
                            .unwrap_or_else(|| "someone".into()),
                        name: message.sender_name.clone(),
                        avatar_url: message.sender_avatar.clone().unwrap_or_default(),
                        status: "member".into(),
                        role: String::new(),
                        last_seen_at: None,
                        online: false,
                        avatar_color_index: None,
                    })
                } else {
                    None
                };
                let sender_label = if from.is_some() {
                    None
                } else {
                    sender_caption(message)
                };
                let named = names_senders && from.is_some() && run_start;
                // Image preview / file card rows (same cards as agent attachments).
                let is_mine = from.is_none();
                // A media-viewer forward's whole text is the "Forwarded from …" line:
                // the card's hint, not a bubble of its own.
                let legacy_hint = (!attachment_parts.is_empty())
                    .then(|| attachments::forwarded_from(&text).map(str::to_owned))
                    .flatten();
                // A bubble-menu forward carries `forwardedFrom`; with attachments the
                // label sits once, on the first card, and the caption bubble under it
                // stays plain ([feedback forward-attachments-mac-2026-10-27]).
                let forwarded_by = message
                    .forwarded_from
                    .as_deref()
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_owned);
                let forwarded = legacy_hint.clone().or_else(|| {
                    (!attachment_parts.is_empty())
                        .then(|| {
                            forwarded_by
                                .as_deref()
                                .map(|by| format!("{}{by}", crate::app::FORWARDED_FROM_PREFIX))
                        })
                        .flatten()
                });
                // Skip a bare "Image"/"Photo" bubble when the attachment already shows it.
                let show_text = !text.is_empty()
                    && !(has_image && is_generic_media_label(&text))
                    && legacy_hint.is_none();
                // A cross-chat reply (Reply in DM) quotes its own snapshot; an in-chat
                // reply quotes the row it points at. A forward never quotes. With
                // attachments the quote sits once, above the first card, so a picture
                // reply keeps its reply — with or without a caption ([feedback
                // picture-reply-quote-mac-2026-10-28]: a wordless picture had no bubble
                // to carry it and the reply vanished).
                let quote = if forwarded_by.is_some() || legacy_hint.is_some() {
                    None
                } else {
                    message
                        .reply_ref
                        .as_ref()
                        .filter(|r| !r.chat_id.is_empty() && !r.message_id.is_empty())
                        .map(Quote::for_reply_ref)
                        .or_else(|| {
                            message
                                .reply_to
                                .as_deref()
                                .filter(|id| !id.is_empty())
                                .map(|id| Quote::for_reply(id, messages))
                        })
                };
                for (ix, (part_id, attachment)) in attachment_parts.iter().enumerate() {
                    let forwarded = forwarded.clone().filter(|_| ix == 0);
                    let quote = quote.clone().filter(|_| ix == 0);
                    let has_thumb = attachment
                        .blob_ref
                        .as_deref()
                        .is_some_and(|k| thumb_keys.contains(k));
                    // Someone else's picture carries their disc like their bubble, and
                    // their name on the first card when the run changes sender.
                    let sender = from.clone().map(AttachmentSender::Person);
                    let card_named = named && ix == 0;
                    let disc = run_start && !show_text && ix + 1 == attachment_parts.len();
                    rows.push(Row {
                        version: hash_version(
                            &(
                                &attachment.name,
                                &attachment.title,
                                attachment.bytes,
                                &attachment.blob_ref,
                                is_mine,
                                has_thumb,
                                &forwarded,
                                from.as_ref().map(|p| &p.handle),
                                card_named,
                                disc,
                                &quote,
                            ),
                            false,
                        ),
                        id: format!("{}#{}", message.id, part_id),
                        message_id: message.id.clone(),
                        kind: RowKind::Attachment {
                            attachment: attachment.clone(),
                            part_id: part_id.clone(),
                            mine: is_mine,
                            sender,
                            named: card_named,
                            disc,
                            time: clock_label(message.created_at),
                            forwarded_from: forwarded.clone(),
                            quote,
                        },
                    });
                }
                if show_text || attachment_parts.is_empty() || preview.is_some() {
                    // The quote is on the first card when the message has any.
                    let quote = quote.filter(|_| attachment_parts.is_empty());
                    let delivery = delivery_state(message, is_mine);
                    // The label is on the first card when the message has any.
                    let forwarded_from = forwarded_by.filter(|_| attachment_parts.is_empty());
                    rows.push(Row {
                        version: hash_version(
                            &(
                                &text,
                                &sender_label,
                                from.as_ref().map(|p| &p.handle),
                                &quote,
                                delivery,
                                &forwarded_from,
                                &ghost,
                                &preview,
                                run_start,
                                named,
                            ),
                            false,
                        ),
                        id: format!("{}#user", message.id),
                        message_id: message.id.clone(),
                        kind: RowKind::User {
                            text,
                            sender_label,
                            from,
                            // The first card already names the sender.
                            named: named && attachment_parts.is_empty(),
                            disc: run_start,
                            time: clock_label(message.created_at),
                            delivery,
                            quote,
                            forwarded_from,
                            ghost,
                            preview,
                        },
                    });
                }
                if message.status == MessageStatus::Queued {
                    queued = true;
                }
                push_reactions(&mut rows, message, is_mine, !is_mine);
            }
            Role::System => {
                // Call lines and membership notices ("Jacob added Home, hjkb and
                // Doug", #1349): one centred muted line, never a bubble. Like a
                // date stamp it breaks the sender run — the next bubble names its
                // sender again (Telegram / Grok Bot).
                // A membership line is worded from its `notice` facts and the live
                // roster, so the actor reads by their current name; the stored words
                // are the fallback when someone named is no longer known here.
                let text = message
                    .notice
                    .as_ref()
                    .and_then(|n| n.text(|kind, id| mentions.name_of(kind, id)))
                    .unwrap_or_else(|| message.plain_text());
                let line = text.lines().next().unwrap_or_default().trim().to_string();
                if !line.is_empty() {
                    rows.push(Row {
                        version: hash_version(&line, false),
                        id: format!("{}#system", message.id),
                        message_id: message.id.clone(),
                        kind: RowKind::Stamp { label: line },
                    });
                    run_tail = None;
                }
            }
            Role::Assistant | Role::Project => {
                // A background job's row (duplex §2.3, `job` + `hidden`) is wordless
                // by design: its status parts feed the presence line and the tasks
                // drawer, never a bubble — not while it runs, not the "no answer"
                // stock line when it ends. The job's outcome is the agent's own
                // reply bubble (or the "Stopped: …" line).
                if message.hidden || message.job.is_some() {
                    continue;
                }
                if matches!(
                    message.turn,
                    Some(TurnKind::New | TurnKind::Spawn | TurnKind::Wake)
                ) {
                    // The box writes `subagentName` on the row; older fixtures only
                    // name it on the spawn tool call.
                    let named = message.subagent_name.clone().or_else(|| {
                        message.parts.iter().find_map(|p| match p {
                            Part::Tool {
                                subagent_name: Some(n),
                                ..
                            } => Some(n.clone()),
                            _ => None,
                        })
                    });
                    // A background worker's spawn row draws nothing (`worker`,
                    // [feedback hide-worker-rows-and-tz-2026-10-24]); its activity
                    // still feeds the working line through `Message::current_activity`.
                    if let Some(name) = named.filter(|_| !message.worker) {
                        let count = message.handoff_messages.unwrap_or(1).max(1);
                        // Consecutive hand-offs merge into one line.
                        if let Some(Row {
                            kind: RowKind::Handoff { names, total },
                            version,
                            ..
                        }) = rows.last_mut()
                        {
                            if !names.contains(&name) {
                                names.push(name);
                            }
                            *total += count;
                            *version = hash_version(&(&*names, *total), false);
                        } else {
                            rows.push(Row {
                                version: hash_version(&(&name, count), false),
                                id: format!("{}#handoff", message.id),
                                message_id: message.id.clone(),
                                kind: RowKind::Handoff {
                                    names: vec![name],
                                    total: count,
                                },
                            });
                        }
                    }
                }
                let mut lane: Option<String> = None;
                let in_people =
                    message.sender.starts_with("agent:") && message.asked_by_handle.is_some();
                // Multi-party chats (group / channel / shared): every agent bubble sits
                // on the avatar column like the other participants. People-chat replies
                // also carry asked_by_handle; a subagent-result row used to ship as bare
                // `sender: agent` with no asker (#1604) — once attributed as `agent:<id>`
                // it still needs the lane even if asked_by_handle is missing.
                let agent_speaker = in_people
                    || message.role == Role::Project
                    || message.sender.starts_with("project:")
                    || (names_senders && message.sender.starts_with("agent:"));
                // Quote the message this answers: `replyTo` is the platform reply the
                // box sets by the #1521 rule (always in groups / channels / people chats
                // and for a row another agent sent; in a 1:1 only when answering an
                // earlier message than the latest). Rows from older boxes in attributed
                // chats fall back to `inReplyTo` (#1447); a plain 1:1 answer to the
                // latest message carries no quote.
                let attributed = message.role == Role::Project || in_people;
                let reply_quote = message
                    .reply_to
                    .as_deref()
                    .filter(|id| !id.is_empty())
                    .or_else(|| {
                        attributed
                            .then(|| message.in_reply_to.as_deref().filter(|id| !id.is_empty()))
                            .flatten()
                    })
                    .map(|id| Quote::for_reply(id, messages));
                if agent_speaker {
                    // A people-chat agent (or another Project, F6) speaks in its own lane;
                    // the disc on the last block names it. No meta line — "answering
                    // @x · owned by @y" moved to the profile pane (Jacob, 2026-10-22).
                    lane = Some(message.sender.clone());
                }
                // Group / channel: the agent's name sits inside its first bubble (the
                // name resolves at render time from the seed, like the disc) when the
                // message starts a run. Agents group like people (Jacob, 2026-10-28,
                // supersedes #1447's name-on-every-reply): a reply that quotes the ask
                // starts its own run and is named; a quick follow-up within the window
                // is not. A stream with no words yet has no bubble, so it shows no name
                // either (the presence row says who is working).
                let sender_named = names_senders && lane.is_some() && run_start;
                let last_text_part = message
                    .parts
                    .iter()
                    .rposition(|p| matches!(p, Part::Text { .. }));
                // Blocks across every text part form one bubble.
                // Polish assistant prose on display so glued ".Word" and a trailing
                // tool-narration sentence (already in the doc) still read as chat.
                let polished: Vec<(String, String)> = message
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Text { id, text } => {
                            Some((id.clone(), mentions.display(&polish_assistant_prose(text))))
                        }
                        _ => None,
                    })
                    .collect();
                let chart_parts = message
                    .parts
                    .iter()
                    .filter(|p| matches!(p, Part::Chart { .. }))
                    .count();
                let link_preview =
                    link_preview_of(message, link_previews).map(|(id, _)| id.clone());
                let total_blocks: usize = polished
                    .iter()
                    .map(|(_, text)| {
                        split_blocks(text)
                            .into_iter()
                            .filter(|b| !is_json_block(b))
                            .count()
                    })
                    .sum::<usize>()
                    + chart_parts
                    + usize::from(link_preview.is_some());
                let fill = total_blocks > 1;
                // Every block of every text part is one bubble for selection.
                let bubble_blocks: Vec<Block> = polished
                    .iter()
                    .flat_map(|(_, text)| {
                        split_blocks(text).into_iter().filter(|b| !is_json_block(b))
                    })
                    .collect();
                let bubble_texts = BubbleText::for_blocks(&bubble_blocks, false);
                let mut block_ix = 0usize;
                let mut had_prose = false;
                // The message's last card that draws (a chart's fallback picture does not).
                let last_card_part = message.parts.iter().rposition(|p| match p {
                    Part::Attachment { attachment, .. } => !attachment
                        .chart_part
                        .as_deref()
                        .is_some_and(|cp| message.has_chart_part(cp)),
                    _ => false,
                });
                // The sender's name goes on the message's first row — a picture that
                // comes before the prose takes it, and the first text block then does
                // not repeat it ([feedback forward-attachments-mac-2026-10-27]).
                let mut name_taken = false;
                // Error parts wait until the prose is known: under a real answer they stay
                // the chip with Retry; on an answer with no words they become the words.
                let mut errors: Vec<(String, String, String)> = Vec::new();
                let rows_before_parts = rows.len();
                for (pix, part) in message.parts.iter().enumerate() {
                    match part {
                        Part::Text { id, .. } => {
                            let text = polished
                                .iter()
                                .find(|(pid, _)| pid == id)
                                .map(|(_, t)| t.as_str())
                                .unwrap_or("");
                            if text.trim().is_empty() {
                                continue;
                            }
                            let blocks: Vec<Block> = split_blocks(text)
                                .into_iter()
                                .filter(|b| !is_json_block(b))
                                .collect();
                            let is_last_text = Some(pix) == last_text_part;
                            let n = blocks.len();
                            for (bix, block) in blocks.into_iter().enumerate() {
                                had_prose = true;
                                let live = streaming && is_last_text && bix + 1 == n;
                                let first = block_ix == 0;
                                // While streaming, the newest block is the bottom for now.
                                let last = block_ix + 1 == total_blocks || (live && streaming);
                                let bubble =
                                    bubble_texts.get(block_ix).cloned().unwrap_or_else(|| {
                                        BubbleText::single(block_plain(&block, false))
                                    });
                                let named = sender_named && first && !name_taken;
                                name_taken |= named;
                                rows.push(Row {
                                    version: hash_version(
                                        &(
                                            &block,
                                            first,
                                            last,
                                            fill,
                                            &reply_quote,
                                            named,
                                            run_start,
                                        ),
                                        live,
                                    ),
                                    id: format!("{}#{}.{}", message.id, id, bix),
                                    message_id: message.id.clone(),
                                    kind: RowKind::Markdown {
                                        block,
                                        live,
                                        bubble,
                                        first,
                                        last,
                                        fill,
                                        time: clock_label(message.created_at),
                                        lane: lane.clone(),
                                        quote: if first { reply_quote.clone() } else { None },
                                        named,
                                        disc: run_start,
                                    },
                                });
                                block_ix += 1;
                            }
                        }
                        // A chart bubble's card: one block of the bubble, like a code card
                        // ([feedback chart-bubble-mac-2026-10-26]).
                        Part::Chart {
                            id, chart, table, ..
                        } => {
                            had_prose = true;
                            let first = block_ix == 0;
                            let last = block_ix + 1 == total_blocks;
                            let block = Block::Chart {
                                part_id: id.clone(),
                                spec: chart.to_string(),
                                table: table.clone(),
                            };
                            let bubble = BubbleText::single(block_plain(&block, false));
                            let named = sender_named && first && !name_taken;
                            name_taken |= named;
                            rows.push(Row {
                                version: hash_version(
                                    &(&block, first, last, fill, &reply_quote, named, run_start),
                                    false,
                                ),
                                id: format!("{}#{}", message.id, id),
                                message_id: message.id.clone(),
                                kind: RowKind::Markdown {
                                    block,
                                    live: false,
                                    bubble,
                                    first,
                                    last,
                                    fill,
                                    time: clock_label(message.created_at),
                                    lane: lane.clone(),
                                    quote: if first { reply_quote.clone() } else { None },
                                    named,
                                    disc: run_start,
                                },
                            });
                            block_ix += 1;
                        }
                        // The link preview card: one block under the prose
                        // ([feedback link-embeds-mac-2026-10-27]); hidden once the sender
                        // removed it (`preview: "off"`) or the switch is off.
                        Part::LinkPreview { id, preview }
                            if link_preview.as_deref() == Some(id.as_str()) =>
                        {
                            had_prose = true;
                            let first = block_ix == 0;
                            let last = block_ix + 1 == total_blocks;
                            let block = Block::LinkPreview {
                                part_id: id.clone(),
                                preview: preview.clone(),
                            };
                            let bubble = BubbleText::single(block_plain(&block, false));
                            let named = sender_named && first;
                            rows.push(Row {
                                version: hash_version(
                                    &(&block, first, last, fill, &reply_quote, named, run_start),
                                    false,
                                ),
                                id: format!("{}#{}", message.id, id),
                                message_id: message.id.clone(),
                                kind: RowKind::Markdown {
                                    block,
                                    live: false,
                                    bubble,
                                    first,
                                    last,
                                    fill,
                                    time: clock_label(message.created_at),
                                    lane: lane.clone(),
                                    quote: if first { reply_quote.clone() } else { None },
                                    named,
                                    disc: run_start,
                                },
                            });
                            block_ix += 1;
                        }
                        Part::LinkPreview { .. } => {}
                        // The chart's fallback picture: an older app shows it, this one
                        // draws the chart instead.
                        Part::Attachment { attachment, .. }
                            if attachment
                                .chart_part
                                .as_deref()
                                .is_some_and(|cp| message.has_chart_part(cp)) => {}
                        Part::Attachment { id, attachment } => {
                            let has_thumb = attachment
                                .blob_ref
                                .as_deref()
                                .is_some_and(|k| thumb_keys.contains(k));
                            // The agent's picture sits in its lane with its disc, and
                            // names it when it is the message's first row (Scout's
                            // photo in Arbonites drew as a bare picture before).
                            let card_named = sender_named && !name_taken;
                            name_taken |= card_named;
                            // The disc goes on the last row of a message that starts its
                            // run: its text bubble when it has words, else its last card.
                            let disc =
                                run_start && total_blocks == 0 && Some(pix) == last_card_part;
                            rows.push(Row {
                                version: hash_version(
                                    &(
                                        &attachment.name,
                                        &attachment.title,
                                        attachment.bytes,
                                        &attachment.blob_ref,
                                        has_thumb,
                                        &lane,
                                        card_named,
                                        disc,
                                    ),
                                    false,
                                ),
                                id: format!("{}#{}", message.id, id),
                                message_id: message.id.clone(),
                                kind: RowKind::Attachment {
                                    attachment: attachment.clone(),
                                    part_id: id.clone(),
                                    mine: false,
                                    sender: lane
                                        .clone()
                                        .map(|seed| AttachmentSender::Agent { seed }),
                                    named: card_named,
                                    disc,
                                    time: clock_label(message.created_at),
                                    forwarded_from: None,
                                    quote: None,
                                },
                            })
                        }
                        // Legacy `waiting_user` row: only when no `question` part carries it.
                        Part::Status {
                            id,
                            code: StatusKind::AskUser,
                            message: text,
                        } if !message.has_question_part() => rows.push(Row {
                            version: hash_version(text, false),
                            id: format!("{}#{}", message.id, id),
                            message_id: message.id.clone(),
                            kind: RowKind::AskUser { text: text.clone() },
                        }),
                        Part::Question {
                            id,
                            question_id,
                            question,
                            widget,
                            answer,
                            state,
                            ..
                        } => match widget {
                            Some(_) => rows.push(Row {
                                version: hash_version(
                                    &(question, answer.as_ref().map(Value::to_string), *state),
                                    false,
                                ),
                                id: format!("{}#{}", message.id, id),
                                message_id: message.id.clone(),
                                kind: RowKind::Question {
                                    question_id: question_id.clone(),
                                    lane: lane.is_some(),
                                },
                            }),
                            None => rows.push(Row {
                                version: hash_version(&(question, *state), false),
                                id: format!("{}#{}", message.id, id),
                                message_id: message.id.clone(),
                                kind: RowKind::AskUser {
                                    text: question.clone(),
                                },
                            }),
                        },
                        // S18: the card renders; its `waiting_approval` status row is implied.
                        Part::Approval {
                            id,
                            approval_id,
                            title,
                            summary,
                            risk,
                            payload,
                            state,
                            decided_by,
                            reason,
                            grant,
                            ..
                        } => {
                            let view = ApprovalView {
                                approval_id: approval_id.clone(),
                                title: title.clone(),
                                summary: summary.clone(),
                                risk: *risk,
                                state: *state,
                                payload: payload.clone(),
                                decided_by: decided_by.clone(),
                                reason: reason.clone(),
                                grant: grant.clone(),
                                agent: None,
                            };
                            rows.push(Row {
                                version: hash_version(
                                    &(approval_id, title, state.label(), decided_by, reason, grant),
                                    false,
                                ),
                                id: format!("{}#{}", message.id, id),
                                message_id: message.id.clone(),
                                kind: RowKind::Approval {
                                    view,
                                    lane: lane.is_some(),
                                },
                            });
                        }
                        Part::Status {
                            code: StatusKind::Queued,
                            ..
                        } => {
                            if streaming {
                                queued = true;
                            }
                        }
                        Part::Error {
                            id,
                            code,
                            message: text,
                        } => errors.push((id.clone(), code.clone(), text.clone())),
                        // Everything the agent did, not said: Details drawer only.
                        Part::Status { .. }
                        | Part::Tool { .. }
                        | Part::SubagentResult { .. }
                        | Part::Reasoning { .. }
                        | Part::Diff { .. }
                        | Part::File { .. }
                        | Part::Browser { .. }
                        | Part::Skills { .. }
                        | Part::SecretRedacted { .. }
                        | Part::Unknown => {}
                    }
                }
                // A finished reply that shows nothing — no prose, no card, no attachment —
                // would leave only the sender header (Lium in the "Testing" group,
                // 2026-10-21: six "answering @…" lines over nothing). Say so in the bubble,
                // plainly, like the limit announcements: the error part's own line when the
                // box wrote one, else a stock line.
                let shows_nothing = rows.len() == rows_before_parts && !had_prose;
                let finished = matches!(message.status, MessageStatus::Done | MessageStatus::Error);
                // A row that only did work (tool calls, a hand-off) may legitimately end
                // without words — its answer came as a progress bubble; only a row that holds
                // nothing but empty text and status gets the stock line.
                let only_empty = message.parts.iter().all(|p| {
                    matches!(
                        p,
                        Part::Text { .. } | Part::Status { .. } | Part::Error { .. }
                    )
                });
                let failed = !errors.is_empty() || matches!(message.status, MessageStatus::Error);
                // A hand-off row (a subagent brief, an agent-to-agent send) is its own line
                // ("Messaged ◉ X"); it never gets the stock line, even with no parts (#1466).
                let handoff_row = message.subagent_name.is_some()
                    && matches!(
                        message.turn,
                        Some(TurnKind::New | TurnKind::Spawn | TurnKind::Wake)
                    );
                // The stock line is for a turn that said NOTHING. A turn that answered with a
                // `send_message` bubble and ended on the fresh result row that bubble opened
                // (wordless, "Working…") must not get "didn't answer" under its own answer
                // (Jacob, "run bubble sort", 2026-10-25). Another visible row of the same turn
                // = an assistant row answering the same user message with words or a card.
                let turn_answered_elsewhere = !failed
                    && message.in_reply_to.as_deref().is_some_and(|ask| {
                        messages.iter().any(|m| {
                            m.id != message.id
                                && m.role == Role::Assistant
                                && m.in_reply_to.as_deref() == Some(ask)
                                && (!m.plain_text().trim().is_empty()
                                    || m.parts.iter().any(|p| {
                                        matches!(
                                            p,
                                            Part::Question { .. }
                                                | Part::Approval { .. }
                                                | Part::File { .. }
                                                | Part::Attachment { .. }
                                        )
                                    }))
                        })
                    });
                if shows_nothing
                    && finished
                    && !streaming
                    && !handoff_row
                    && !turn_answered_elsewhere
                    && (failed || only_empty)
                {
                    let line = errors
                        .first()
                        .map(|(_, _, text)| text.trim())
                        .filter(|t| !t.is_empty())
                        .map(str::to_owned);
                    let kind = match line {
                        Some(line) => RowKind::Markdown {
                            bubble: BubbleText::single(inline_plain(&line)),
                            block: Block::Paragraph(line),
                            live: false,
                            first: true,
                            last: true,
                            fill: false,
                            time: clock_label(message.created_at),
                            lane: lane.clone(),
                            quote: reply_quote.clone(),
                            named: sender_named,
                            disc: run_start,
                        },
                        None => RowKind::NoAnswer {
                            seed: message.sender.clone(),
                            name: message.sender_name.clone().filter(|n| !n.trim().is_empty()),
                            time: clock_label(message.created_at),
                            lane: lane.clone(),
                            quote: reply_quote.clone(),
                            named: sender_named,
                            disc: run_start,
                        },
                    };
                    rows.push(Row {
                        version: hash_version(
                            &(
                                &message.sender,
                                &message.sender_name,
                                errors.first().map(|(_, _, text)| text),
                                &reply_quote,
                                sender_named,
                                run_start,
                            ),
                            false,
                        ),
                        id: format!("{}#failed", message.id),
                        message_id: message.id.clone(),
                        kind,
                    });
                } else {
                    for (id, code, text) in &errors {
                        rows.push(Row {
                            version: hash_version(&(code, text), false),
                            id: format!("{}#{}", message.id, id),
                            message_id: message.id.clone(),
                            kind: RowKind::Error {
                                code: code.clone(),
                                message: text.clone(),
                            },
                        });
                    }
                }
                // Stopped turns leave one tiny glyph, no words.
                let glyph = match message.status {
                    MessageStatus::Cancelled => Some("■"),
                    MessageStatus::Error
                    | MessageStatus::Done
                    | MessageStatus::Queued
                    | MessageStatus::Streaming => None,
                };
                if let Some(glyph) = glyph {
                    rows.push(Row {
                        version: 1,
                        id: format!("{}#glyph", message.id),
                        message_id: message.id.clone(),
                        kind: RowKind::Glyph { glyph },
                    });
                }
                if !streaming {
                    push_reactions(&mut rows, message, false, lane.is_some());
                }
                // Speaker (F11): every reply whose speech is cached or failed,
                // plus the newest finished reply while voice mode is on.
                let cached = speech.spoken.contains(&message.id);
                let failed = speech.failed.contains_key(&message.id);
                if speech.enabled
                    && !streaming
                    && message.role == Role::Assistant
                    && had_prose
                    && (cached || failed || (speech.voice_mode && last_reply == Some(mix)))
                {
                    rows.push(Row {
                        version: hash_version(&(cached, failed), false),
                        id: format!("{}#speaker", message.id),
                        message_id: message.id.clone(),
                        kind: RowKind::Speaker { cached, failed },
                    });
                }
                if show_details && !streaming && message.role == Role::Assistant && had_prose {
                    rows.push(Row {
                        version: 1,
                        id: format!("{}#details", message.id),
                        message_id: message.id.clone(),
                        kind: RowKind::Details,
                    });
                }
                // Agent-to-agent messaging: the reply went back to the agent that wrote in.
                if let Some(name) = message
                    .agent_reply_to
                    .as_deref()
                    .filter(|n| !streaming && !n.trim().is_empty())
                {
                    rows.push(Row {
                        version: hash_version(&name, false),
                        id: format!("{}#replied", message.id),
                        message_id: message.id.clone(),
                        kind: RowKind::RepliedTo {
                            name: name.to_owned(),
                        },
                    });
                }
            }
        }
        // A message that drew rows is the run's new tail; one that drew nothing
        // (a hidden job, a stream with no words yet) neither starts nor continues
        // a run, and is never a turn start.
        let drew = rows.len() > rows_before_message;
        if drew {
            run_tail = Some(RunTail {
                sender: message.sender.clone(),
                at: message.created_at,
            });
            if run_start {
                cache.run_starts.insert(message.id.clone());
            }
        }
        // Finalize a no-content message's stamp now so the cached slice is
        // complete (same outcome as retract on the next turn).
        if !drew {
            retract_dangling_stamp(
                &mut rows,
                &mut pending_stamp,
                &mut last_stamp_at,
                &mut run_tail,
            );
        }
        cache.entries.insert(
            message.id.clone(),
            CachedMessageRows {
                version,
                rows: rows[start..].to_vec(),
                last_stamp_at,
                run_tail: run_tail.clone(),
                run_start: drew && run_start,
            },
        );
    }
    retract_dangling_stamp(
        &mut rows,
        &mut pending_stamp,
        &mut last_stamp_at,
        &mut run_tail,
    );
    cache.entries.retain(|id, _| keep.contains(id));
    // Who is working is the presence row over the composer (`ui::presence`),
    // not a transcript row: one notice per agent, never a caption plus a line.
    let _ = (running, queued, last_stamp_at, run_tail);
    // Trailing clearance: scrolls with the list so mid-thread bubbles can
    // reach the composer, while the pinned end still keeps 54 pt above it.
    if !rows.is_empty() {
        rows.push(Row {
            version: 1,
            id: "chat#bottom-inset".into(),
            message_id: String::new(),
            kind: RowKind::BottomInset,
        });
    }
    rows
}

/// A time stamp that ended up as the last row drew a line over nothing: the
/// message it introduced added no row. Take it back and restore the stamp
/// clock and the sender run to what they were before it.
fn retract_dangling_stamp(
    rows: &mut Vec<Row>,
    pending: &mut Option<(usize, u64, Option<RunTail>)>,
    last_stamp_at: &mut u64,
    run_tail: &mut Option<RunTail>,
) {
    if let Some((ix, at, tail)) = pending.take() {
        if rows.len() == ix + 1 && matches!(rows[ix].kind, RowKind::Stamp { .. }) {
            rows.pop();
            *last_stamp_at = at;
            *run_tail = tail;
        }
    }
}

/// One chips row under a message that carries reactions. `mine` and `lane` are
/// the ones its bubble was laid out with, so the chips share its edge.
fn push_reactions(rows: &mut Vec<Row>, message: &Message, mine: bool, lane: bool) {
    let reactions = message.reaction_list();
    if reactions.is_empty() {
        return;
    }
    rows.push(Row {
        version: hash_version(&(&reactions, mine, lane), false),
        id: format!("{}#reactions", message.id),
        message_id: message.id.clone(),
        kind: RowKind::Reactions {
            reactions,
            mine,
            lane,
        },
    });
}

// ---------------------------------------------------------------------------
// Markdown rendering with the veil
// ---------------------------------------------------------------------------

fn sans_font(theme: &Theme) -> gpui::Font {
    gpui::Font {
        family: theme.font_sans.into(),
        features: Default::default(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: gpui::FontStyle::Normal,
    }
}

/// One selectable text element of a block and the non-selectable text the
/// bubble shows before it: a list marker, a cell separator, the line break
/// between items. The bubble's plain text is every segment's `lead + text`,
/// blocks joined by one newline ([`bubble_plain`]); a selection is a byte
/// range of that text, so copy includes the markers the user dragged over.
struct Segment {
    lead: String,
    text: String,
}

/// The selectable segments of `block`, in paint order. `literal` renders the
/// text as typed (user bubbles); otherwise inline markdown markers are
/// removed, as `styled_text` paints it.
fn block_segments(block: &Block, literal: bool) -> Vec<Segment> {
    let inline = |t: &str| {
        if literal {
            // As typed, except that bare URLs carry their wrap hints (as
            // `plain_text` draws them).
            urlwrap::hint_bare_urls(t)
        } else {
            inline_plain(t)
        }
    };
    let seg = |lead: String, text: String| Segment { lead, text };
    match block {
        Block::Paragraph(text) | Block::Heading { text, .. } => {
            vec![seg(String::new(), inline(text))]
        }
        // A quote's blocks follow one another one newline apart, as the
        // bubble's blocks do.
        Block::Quote(inner) => inner
            .iter()
            .enumerate()
            .flat_map(|(i, b)| {
                let mut segs = block_segments(b, literal);
                if i > 0 {
                    if let Some(first) = segs.first_mut() {
                        first.lead.insert(0, '\n');
                    }
                }
                segs
            })
            .collect(),
        Block::Bullets(items) => items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                seg(
                    format!("{}• ", if i == 0 { "" } else { "\n" }),
                    inline(item),
                )
            })
            .collect(),
        Block::Numbered(items) => items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                seg(
                    format!("{}{}. ", if i == 0 { "" } else { "\n" }, i + 1),
                    inline(item),
                )
            })
            .collect(),
        Block::Table { header, rows } => {
            let cols = header.len().max(1);
            let mut out = Vec::new();
            for (ri, row) in std::iter::once(header).chain(rows.iter()).enumerate() {
                for ci in 0..cols {
                    let lead = match (ri, ci) {
                        (0, 0) => "",
                        (_, 0) => "\n",
                        _ => " | ",
                    };
                    let cell = row.get(ci).map(String::as_str).unwrap_or("");
                    out.push(seg(lead.to_string(), inline(cell)));
                }
            }
            out
        }
        Block::Code { code, .. } => vec![seg(String::new(), code.clone())],
        Block::Math { source } => vec![seg(
            String::new(),
            if literal {
                source.clone()
            } else {
                math::plain(source)
            },
        )],
        // A copy of the bubble carries the chart's data as its markdown table.
        Block::Chart { table, .. } => vec![seg(String::new(), table.clone())],
        // A copy of the bubble carries the card as its title and site.
        Block::LinkPreview { preview, .. } => vec![seg(String::new(), preview.one_line())],
    }
}

/// The plain text a user bubble is selectable as — what `user_message_body`
/// hands its `Selectable::bubble`: the text as typed, or its blocks when it
/// carries a fenced code block.
fn user_bubble_plain(text: &str) -> String {
    let blocks = split_blocks(text);
    if is_plain_paragraph(&blocks) {
        urlwrap::hint_bare_urls(text)
    } else {
        bubble_plain(&blocks, false)
    }
}

/// The clipboard text of a selection that spans bubbles: the start bubble
/// from the pressed index, every bubble strictly between the ends (from
/// `bubbles`, the transcript's `(order, key, text)` list), and the end
/// bubble up to its index — in transcript order, one blank line between
/// messages. The two ends use the texts the gesture saw.
fn copy_across(span: &Span, bubbles: &[(usize, String, String)]) -> Option<String> {
    let head = span.start.text.get(span.start_index..)?;
    let tail = span.end.text.get(..span.end_index)?;
    let start = (span.start.order, span.start.key.as_ref());
    let end = (span.end.order, span.end.key.as_ref());
    let mut parts = vec![head.to_string()];
    for (order, key, text) in bubbles {
        let rank = (*order, key.as_str());
        if rank > start && rank < end {
            parts.push(text.clone());
        }
    }
    parts.push(tail.to_string());
    Some(
        parts
            .into_iter()
            .filter(|p| !p.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
    )
}

/// The rendered plain text of one block (what a full selection of it copies).
pub fn block_plain(block: &Block, literal: bool) -> String {
    block_segments(block, literal)
        .iter()
        .map(|s| format!("{}{}", s.lead, s.text))
        .collect()
}

/// The rendered plain text of a whole bubble: its blocks, one newline apart.
pub fn bubble_plain(blocks: &[Block], literal: bool) -> String {
    blocks
        .iter()
        .map(|b| block_plain(b, literal))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Byte offset of each block's plain text inside [`bubble_plain`].
pub fn block_offsets(blocks: &[Block], literal: bool) -> Vec<usize> {
    let mut at = 0usize;
    blocks
        .iter()
        .map(|b| {
            let here = at;
            at += block_plain(b, literal).len() + 1;
            here
        })
        .collect()
}

/// Selectable user-bubble text: literal (no bold/italic/heading paint) except
/// bare `http(s)://…` URLs, which get accent + underline and open on click.
/// `at` is where `text` starts in `bubble`'s plain text.
fn plain_text(
    text: &str,
    color: gpui::Hsla,
    theme: &Theme,
    bubble: &BubbleSelectable,
    at: usize,
) -> AnyElement {
    let font = sans_font(theme);
    let mut full = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut links: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    let mut plain = String::new();

    let flush_plain = |plain: &mut String,
                       full: &mut String,
                       runs: &mut Vec<TextRun>,
                       font: &gpui::Font,
                       color: gpui::Hsla| {
        if plain.is_empty() {
            return;
        }
        runs.push(TextRun {
            len: plain.len(),
            font: font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
        full.push_str(plain);
        plain.clear();
    };

    while i < chars.len() {
        let is_url = chars[i] == 'h'
            && (slice_starts_at(&chars, i, "https://") || slice_starts_at(&chars, i, "http://"));
        if is_url {
            flush_plain(&mut plain, &mut full, &mut runs, &font, color);
            let end = (i..chars.len())
                .find(|&j| chars[j].is_whitespace() || chars[j] == ')' || chars[j] == '"')
                .unwrap_or(chars.len());
            let mut url_end = end;
            while url_end > i && matches!(chars[url_end - 1], '.' | ',' | ';' | ':') {
                url_end -= 1;
            }
            let url: String = chars[i..url_end].iter().collect();
            let start = full.len();
            // `text` already carries the wrap hints; the target never does.
            links.push((start..start + url.len(), urlwrap::plain(&url)));
            runs.push(TextRun {
                len: url.len(),
                font: font.clone(),
                color: theme.accent,
                background_color: None,
                underline: Some(gpui::UnderlineStyle {
                    color: Some(Theme::with_alpha(theme.accent, 0.6)),
                    thickness: px(1.0),
                    wavy: false,
                }),
                strikethrough: None,
            });
            full.push_str(&url);
            i = url_end;
            continue;
        }
        plain.push(chars[i]);
        i += 1;
    }
    flush_plain(&mut plain, &mut full, &mut runs, &font, color);

    bubble
        .text(at, full, runs)
        .with_links(links)
        .into_any_element()
}

fn slice_starts_at(chars: &[char], at: usize, needle: &str) -> bool {
    let n: Vec<char> = needle.chars().collect();
    at + n.len() <= chars.len() && chars[at..at + n.len()] == n[..]
}

/// User / people-chat bubble body: plain prose, or prose mixed with fenced
/// ``` code cards when the message contains triple backticks.
/// Secret hand-off ghost row body: key glyph, "Secret saved as **NAME**" (another person's
/// row: "<Name> saved a secret as **NAME**"). Nothing here is the value.
fn secret_ghost_body(name: &str, from: Option<&Person>, theme: &Theme) -> AnyElement {
    let lead = match from {
        Some(p) => format!("{} saved a secret as ", p.display_name()),
        None => "Secret saved as ".to_owned(),
    };
    div()
        .flex()
        .items_center()
        .gap(px(7.0))
        .child(sf_symbols::glyph("key", 13.0, theme.text_muted))
        .child(
            div()
                .flex()
                .items_baseline()
                .child(div().text_color(theme.text_muted).child(lead))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(name.to_owned()),
                ),
        )
        .into_any_element()
}

/// The message's link preview, when Settings → Chat → Link previews is on (`enabled`,
/// `AppState::link_previews`) and the sender has not removed it ([feedback
/// link-embeds-mac-2026-10-27]).
pub fn link_preview_of(message: &Message, enabled: bool) -> Option<(&Id, &LinkPreview)> {
    if !enabled {
        return None;
    }
    message.link_preview()
}

/// Was the message nothing but the link the card stands for? Then the card is the bubble.
pub fn is_link_only(text: &str, preview: &LinkPreview) -> bool {
    let t = text.trim();
    !t.is_empty()
        && !t.contains(char::is_whitespace)
        && (t == preview.url.trim()
            || t == preview.canonical_url.trim()
            || t.trim_end_matches('/') == preview.url.trim().trim_end_matches('/'))
}

fn user_message_body(
    row_id: &str,
    text: &str,
    color: gpui::Hsla,
    theme: &Theme,
    selectable: &Selectable,
    copy: &CodeCopy,
) -> AnyElement {
    let key = format!("{row_id}.u");
    let blocks = split_blocks(text);
    // A typed message with no formatting at all stays one plain element: the
    // text as typed, bare URLs linked ([`user_bubble_plain`]).
    if is_plain_paragraph(&blocks) {
        let text = urlwrap::hint_bare_urls(text);
        let bubble = selectable.bubble(key, text.clone());
        return plain_text(&text, color, theme, &bubble, 0);
    }
    let bubble = selectable.bubble(key, bubble_plain(&blocks, false));
    let offsets = block_offsets(&blocks, false);
    // Same as `render_block`: no `min_w_0`, so the quote header above never
    // sets this column's width.
    let mut col = div().flex().flex_col().gap(px(6.0)).w_full();
    for (i, block) in blocks.iter().enumerate() {
        let id = format!("{row_id}.u{i}");
        col = col.child(render_block_in(
            &id,
            block,
            color,
            offsets[i],
            None,
            Instant::now(),
            theme,
            &bubble,
            copy,
        ));
    }
    col.into_any_element()
}

/// Whether a typed message renders exactly as typed: one paragraph whose
/// inline runs carry no style (links aside — `plain_text` draws those).
fn is_plain_paragraph(blocks: &[Block]) -> bool {
    match blocks {
        [Block::Paragraph(text)] => inline_spans(text).iter().all(|s| {
            let mut style = s.style;
            style.link = false;
            style == markdown::InlineStyle::default() && s.icon.is_none()
        }),
        [] => true,
        _ => false,
    }
}

/// Inline-markdown prose as one selectable element. `veil_offset` is where
/// `text` starts in the block's source (for the streaming veil); `at` is
/// where its rendered text starts in `bubble`'s plain text.
#[allow(clippy::too_many_arguments)]
fn styled_text(
    text: &str,
    base_color: gpui::Hsla,
    theme: &Theme,
    veil: Option<&Veil>,
    now: Instant,
    veil_offset: usize,
    bubble: &BubbleSelectable,
    at: usize,
) -> AnyElement {
    let (full, runs, links, icons, spoilers) = inline_runs(text, base_color, theme, at, &|start| {
        bubble.spoiler_revealed(start)
    });
    let runs = match veil {
        Some(veil) => apply_veil(runs, &veil.spans(now), veil_offset),
        None => runs,
    };
    bubble
        .text(at, full, runs)
        .with_links(links)
        .with_icons(icons)
        .with_spoilers(spoilers)
        .into_any_element()
}

/// Grok Bot's **bold** and headings are bold (700), not semibold: on Jacob's
/// 2026-10-24 shot the "Surf gear" heading of the surf list is a wider,
/// heavier cut than Arbos's semibold ([feedback bubble-markdown-2026-10-24]).
const STRONG_WEIGHT: FontWeight = FontWeight::BOLD;

/// The neutral rounded square a named link shows while its site's icon loads
/// or when the site has none: the muted text colour at low alpha.
pub fn link_icon_placeholder(theme: &Theme) -> gpui::Hsla {
    Theme::with_alpha(theme.text_muted, 0.28)
}

/// Clickable link ranges of a rendered text (element-local bytes → href).
type Links = Vec<(std::ops::Range<usize>, String)>;
/// Icon cells of the named links (element-local bytes of the cell → site host).
type Icons = Vec<(std::ops::Range<usize>, String)>;
/// Spoiler ranges (element-local bytes → where the spoiler starts in the bubble).
type Spoilers = Vec<(std::ops::Range<usize>, usize)>;

/// A hidden spoiler's fill: a solid pill in the muted ink; open, a faint wash.
pub fn spoiler_fill(color: gpui::Hsla, theme: &Theme, revealed: bool) -> gpui::Hsla {
    let ink = muted_of(color, theme);
    Theme::with_alpha(ink, if revealed { 0.16 } else { 0.9 })
}

/// The rendered text, its styled runs, its link ranges, its icon cells and
/// its spoiler ranges for inline markdown `text` (`**bold**`, `*italic*`,
/// `~~strike~~`, `||spoiler||`, `` `code` ``, links). A named link's cell
/// ([`markdown::LINK_ICON_CELL`]) is an unpainted run — the site's picture
/// goes there at paint time — and the link's click range covers cell and
/// name. A spoiler the viewer has not opened (`revealed(start)` is false for
/// its start byte in the bubble's plain text) paints as a solid pill with its
/// text invisible; `at` is where `text` starts in the bubble's plain text.
fn inline_runs(
    text: &str,
    base_color: gpui::Hsla,
    theme: &Theme,
    at: usize,
    revealed: &dyn Fn(usize) -> bool,
) -> (String, Vec<TextRun>, Links, Icons, Spoilers) {
    let spans = inline_spans(text);
    let mut full = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut links: Links = Vec::new();
    let mut icons: Icons = Vec::new();
    let mut spoilers: Spoilers = Vec::new();
    // Where each spoiler's first run starts (element-local), by its index.
    let mut spoiler_starts: HashMap<u16, usize> = HashMap::new();
    let sans = sans_font(theme);
    let mono = gpui::Font {
        family: theme.font_mono.into(),
        ..sans.clone()
    };
    for span in spans {
        let style = span.style;
        let mut font = if style.math {
            math_font(theme, style.italic, style.bold)
        } else if style.code {
            mono.clone()
        } else {
            sans.clone()
        };
        if style.bold && !style.math {
            font.weight = STRONG_WEIGHT;
        }
        if style.italic && !style.math {
            font.style = gpui::FontStyle::Italic;
        }
        let start = full.len();
        if let Some(href) = span.href.clone() {
            links.push((start..start + span.text.len(), href));
        }
        let mut text_len = span.text.len();
        if let Some(host) = span.icon.as_ref() {
            let cell = markdown::LINK_ICON_CELL.len();
            if span.text.starts_with(markdown::LINK_ICON_CELL) {
                icons.push((start..start + cell, host.clone()));
                runs.push(TextRun {
                    len: cell,
                    font: font.clone(),
                    color: gpui::transparent_black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                });
                text_len -= cell;
            }
        }
        let mut color = if style.link { theme.accent } else { base_color };
        let mut background = style.code.then_some(theme.code_bg);
        let mut underline = style.link.then(|| gpui::UnderlineStyle {
            color: Some(Theme::with_alpha(theme.accent, 0.6)),
            thickness: px(1.0),
            wavy: false,
        });
        if style.spoiler != 0 {
            let first = *spoiler_starts.entry(style.spoiler).or_insert(start);
            let open = revealed(at + first);
            spoilers.push((start..start + span.text.len(), at + first));
            background = Some(spoiler_fill(base_color, theme, open));
            if !open {
                color = gpui::transparent_black();
                underline = None;
            }
        }
        runs.push(TextRun {
            len: text_len,
            font,
            color,
            background_color: background,
            underline,
            strikethrough: style.strike.then(|| gpui::StrikethroughStyle {
                thickness: px(1.0),
                color: Some(color),
            }),
        });
        full.push_str(&span.text);
    }
    (full, runs, links, icons, spoilers)
}

/// Split runs at chunk boundaries and multiply alpha in (zeron veil.rs).
fn apply_veil(
    runs: Vec<TextRun>,
    spans: &[(std::ops::Range<usize>, f32)],
    offset: usize,
) -> Vec<TextRun> {
    if spans.is_empty() {
        return runs;
    }
    let mut out = Vec::new();
    let mut pos = 0usize;
    for run in runs {
        let run_range = pos..pos + run.len;
        let mut cursor = run_range.start;
        // Boundaries inside this run.
        let mut cuts: Vec<usize> = spans
            .iter()
            .flat_map(|(r, _)| [r.start.saturating_sub(offset), r.end.saturating_sub(offset)])
            .filter(|c| *c > run_range.start && *c < run_range.end)
            .collect();
        cuts.sort_unstable();
        cuts.dedup();
        cuts.push(run_range.end);
        for cut in cuts {
            if cut <= cursor {
                continue;
            }
            let mid = cursor;
            let alpha = spans
                .iter()
                .find(|(r, _)| {
                    let s = r.start.saturating_sub(offset);
                    let e = r.end.saturating_sub(offset);
                    mid >= s && mid < e
                })
                .map(|(_, a)| *a)
                .unwrap_or(1.0);
            let mut color = run.color;
            color.a *= alpha;
            out.push(TextRun {
                len: cut - cursor,
                color,
                ..run.clone()
            });
            cursor = cut;
        }
        pos = run_range.end;
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn render_block(
    row_id: &str,
    block: &Block,
    at: usize,
    veil: Option<&Veil>,
    now: Instant,
    theme: &Theme,
    bubble: &BubbleSelectable,
    copy: &CodeCopy,
) -> AnyElement {
    render_block_in(
        row_id, block, theme.text, at, veil, now, theme, bubble, copy,
    )
}

/// The quote bar's width and the gap to the quoted text (Grok Bot: a thin
/// muted bar, the text inset and muted).
pub const QUOTE_BAR: f32 = 3.0;
pub const QUOTE_GAP: f32 = 10.0;

/// A muted version of the bubble's text colour: the theme's muted ink on the
/// agent's grey bubble, the same ink at lower alpha on my accent bubble.
fn muted_of(color: gpui::Hsla, theme: &Theme) -> gpui::Hsla {
    if color == theme.text {
        theme.text_muted
    } else {
        Theme::with_alpha(color, 0.72)
    }
}

/// One block in `color` (the bubble's ink: `theme.text` on an agent's bubble,
/// the white of my own).
#[allow(clippy::too_many_arguments)]
fn render_block_in(
    row_id: &str,
    block: &Block,
    color: gpui::Hsla,
    at: usize,
    veil: Option<&Veil>,
    now: Instant,
    theme: &Theme,
    bubble: &BubbleSelectable,
    copy: &CodeCopy,
) -> AnyElement {
    // No `min_w_0` here. Inside a flex-column bubble (a quote header stacked
    // over the text, #1447) taffy sizes the column from its items, and an
    // item with `min-width: 0` contributes nothing to that width — the bubble
    // took the quote header's width and wrapped the body one word per line
    // (Jacob, 2026-10-23, v0.6.175). With the minimum left auto the text's
    // unwrapped width sizes the bubble, up to the column cap; it still shrinks
    // with the bubble (a flex item's automatic minimum is main-axis only, and
    // the bubble itself keeps `min_w_0`). `layout_tests::a_quoted_bubble_…`.
    let base = div()
        .w_full()
        .text_color(color)
        .text_size(px(CHAT_BODY))
        .line_height(px(CHAT_LINE_HEIGHT));
    // Where each selectable element's text starts in the bubble's plain text.
    let segments = block_segments(block, false);
    let mut starts = Vec::with_capacity(segments.len());
    let mut pos = at;
    for s in &segments {
        pos += s.lead.len();
        starts.push(pos);
        pos += s.text.len();
    }
    let muted = muted_of(color, theme);
    match block {
        Block::Paragraph(text) => base
            .child(styled_text(
                text, color, theme, veil, now, 0, bubble, starts[0],
            ))
            .into_any_element(),
        Block::Heading { level, text } => {
            let size = match level {
                1 => 20.0,
                2 => 17.0,
                _ => 15.0,
            };
            base.text_size(px(size))
                .font_weight(STRONG_WEIGHT)
                .pt(px(6.0))
                .debug_selector(move || format!("heading-{level}"))
                .child(styled_text(
                    text, color, theme, veil, now, 0, bubble, starts[0],
                ))
                .into_any_element()
        }
        // `> quoted`: a thin bar, the blocks inset and muted; a quote inside
        // a quote draws its own bar inside this one.
        Block::Quote(inner) => {
            let mut col = div().flex_1().min_w_0().flex().flex_col().gap(px(6.0));
            let mut inner_at = at;
            for (i, b) in inner.iter().enumerate() {
                let id = format!("{row_id}.q{i}");
                col = col.child(render_block_in(
                    &id, b, muted, inner_at, None, now, theme, bubble, copy,
                ));
                inner_at += block_plain(b, false).len() + 1;
            }
            base.flex()
                .items_stretch()
                .gap(px(QUOTE_GAP))
                .debug_selector(|| "quote-block".into())
                .child(
                    div()
                        .w(px(QUOTE_BAR))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(Theme::with_alpha(muted, 0.55))
                        .debug_selector(|| "quote-bar".into()),
                )
                .child(col)
                .into_any_element()
        }
        Block::Bullets(items) | Block::Numbered(items) => {
            let numbered = matches!(block, Block::Numbered(_));
            let mut col = base.flex().flex_col().gap(px(LIST_ITEM_GAP));
            let mut offset = 0usize;
            // Two-digit markers ("10.") need a wider gutter than a dot.
            let indent = if numbered && items.len() >= 10 {
                LIST_INDENT_WIDE
            } else {
                LIST_INDENT
            };
            for (i, item) in items.iter().enumerate() {
                let marker = div()
                    .w(px(indent))
                    .h(px(CHAT_LINE_HEIGHT))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .text_color(muted)
                    .debug_selector(move || format!("list-marker-{i}"));
                let marker = if numbered {
                    marker.child(format!("{}.", i + 1))
                } else {
                    // Grok's bullet: a real disc, not the font's "•" glyph.
                    marker.child(
                        div()
                            .ml(px(BULLET_CENTRE - BULLET_DOT / 2.0))
                            .size(px(BULLET_DOT))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(muted)
                            .debug_selector(move || format!("list-bullet-{i}")),
                    )
                };
                col = col.child(
                    div()
                        .flex()
                        .debug_selector(move || format!("list-item-{i}"))
                        .child(marker)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .debug_selector(move || format!("list-text-{i}"))
                                .child(styled_text(
                                    item, color, theme, veil, now, offset, bubble, starts[i],
                                )),
                        ),
                );
                offset += item.len() + 1;
            }
            col.into_any_element()
        }
        Block::Table { header, rows } => {
            let cols = header.len().max(1);
            let widths = table_column_widths(header, rows);
            // A cell is inline markdown like any paragraph: `[Name](url)` is a
            // clickable link, `**bold**` is bold ([feedback
            // markdown-tables-mac-2026-10-25]: the Lima table showed raw
            // `[…](https://…)` in every first column).
            let cell = |text: &str, head: bool, ri: usize, i: usize, start: usize| {
                let color = if head { color } else { muted };
                let (full, mut runs, links, icons, spoilers) =
                    inline_runs(text, color, theme, start, &|s| bubble.spoiler_revealed(s));
                if head {
                    for run in &mut runs {
                        if run.font.weight < FontWeight::SEMIBOLD {
                            run.font.weight = FontWeight::SEMIBOLD;
                        }
                    }
                }
                div()
                    .flex_grow(1.0)
                    .flex_shrink_0()
                    .flex_basis(px(widths[i]))
                    .min_w_0()
                    .px(px(10.0))
                    .py(px(6.0))
                    .when(i > 0, |el| el.border_l_1().border_color(theme.border))
                    .text_size(px(12.5))
                    .text_color(color)
                    .debug_selector(move || format!("table-cell-r{ri}-c{i}"))
                    .child(
                        bubble
                            .text(start, full, runs)
                            .with_links(links)
                            .with_icons(icons)
                            .with_spoilers(spoilers),
                    )
            };
            // Taffy sizes a nested flex column from its cells' text, not their
            // bases, so the table names its width: the bases' sum, never under
            // the frame (`min_w_full`); the cells' `flex_grow` shares any slack.
            let total: f32 = widths.iter().sum();
            let mut table = div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .w(px(total))
                .min_w_full()
                .debug_selector(|| "table".into());
            let mut head_row = div().flex().bg(theme.code_bg);
            for (i, h) in header.iter().enumerate() {
                head_row = head_row.child(cell(h, true, 0, i, starts[i]));
            }
            table = table.child(head_row);
            for (ri, row) in rows.iter().enumerate() {
                let mut r = div()
                    .flex()
                    .border_t_1()
                    .border_color(theme.border)
                    .when(ri % 2 == 1, |el| {
                        el.bg(Theme::with_alpha(theme.code_bg, 0.4))
                    });
                for i in 0..cols {
                    r = r.child(cell(
                        row.get(i).map(String::as_str).unwrap_or(""),
                        false,
                        ri + 1,
                        i,
                        starts[(ri + 1) * cols + i],
                    ));
                }
                table = table.child(r);
            }
            // Wider than the bubble (many columns, long cells): the table keeps
            // its column widths and scrolls sideways inside the frame instead
            // of squeezing every cell to a word per line.
            sideways_scroll(
                div()
                    .id(SharedString::from(format!("{row_id}#table")))
                    .w_full()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme.border),
            )
            .child(table)
            .into_any_element()
        }
        Block::Code { lang, code } => code_fence_card(
            row_id, lang, code, color, theme, bubble, starts[0], veil, now, copy,
        ),
        Block::Chart { part_id, spec, .. } => {
            crate::ui::chart_card::card(row_id, part_id, spec, theme, copy)
        }
        // A link preview card under the prose: an agent's or another person's bubble, so
        // never "own"; removable when this chat is mine to moderate.
        Block::LinkPreview { preview, .. } => {
            let message_id = row_id.split('#').next().unwrap_or(row_id);
            crate::ui::link_card::card(
                row_id,
                message_id,
                preview,
                theme,
                copy.links.as_ref(),
                crate::ui::link_card::Place {
                    own: false,
                    removable: copy.moderates,
                },
            )
        }
        // Display math: typeset, a size up, centred on its own line (KaTeX's
        // display mode in Grok Bot).
        Block::Math { source } => {
            let (full, runs) = math_runs(source, color, theme);
            let runs = match veil {
                Some(veil) => apply_veil(runs, &veil.spans(now), 0),
                None => runs,
            };
            base.flex()
                .justify_center()
                .text_size(px(CHAT_BODY * DISPLAY_MATH_SCALE))
                .line_height(px(CHAT_LINE_HEIGHT * DISPLAY_MATH_SCALE))
                .py(px(4.0))
                .debug_selector(|| "display-math".into())
                .child(bubble.text(starts[0], full, runs))
                .into_any_element()
        }
    }
}

/// Display math is this much larger than body text.
const DISPLAY_MATH_SCALE: f32 = 1.15;

/// The math face for an inline or display run: the serif family with its
/// glyph fallbacks, italic for variables, bold for `\mathbf`.
fn math_font(theme: &Theme, italic: bool, bold: bool) -> gpui::Font {
    gpui::Font {
        family: theme.font_math.into(),
        features: Default::default(),
        fallbacks: Some(gpui::FontFallbacks::from_fonts(Theme::math_fallbacks())),
        weight: if bold {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        },
        style: if italic {
            gpui::FontStyle::Italic
        } else {
            gpui::FontStyle::Normal
        },
    }
}

/// A display-math block's text and runs.
fn math_runs(source: &str, color: gpui::Hsla, theme: &Theme) -> (String, Vec<TextRun>) {
    let mut full = String::new();
    let mut runs = Vec::new();
    for run in math::typeset(source) {
        runs.push(TextRun {
            len: run.text.len(),
            font: math_font(
                theme,
                run.style == math::MathStyle::Italic,
                run.style == math::MathStyle::Bold,
            ),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
        full.push_str(&run.text);
    }
    (full, runs)
}

/// A frame whose content scrolls sideways only when the pointer's wheel or
/// trackpad gesture is horizontal. GPUI otherwise turns a vertical wheel over a
/// horizontal-only scroll area into sideways motion and the transcript stops
/// scrolling under the pointer (`Style::restrict_scroll_to_axis`).
fn sideways_scroll(frame: Stateful<Div>) -> Stateful<Div> {
    // A flex row: its one child sizes to its content (max-content) and, with
    // `flex_shrink_0` + `min_w_full`, is never narrower than the frame nor
    // squeezed below its content — so the frame has something to scroll.
    let mut frame = frame.flex().overflow_x_scroll();
    frame.style().restrict_scroll_to_axis = Some(true);
    frame
}

/// The narrowest a table column is laid out at, in points.
pub const TABLE_COLUMN_MIN: f32 = 72.0;
/// The widest a table column is laid out at before its cells wrap.
pub const TABLE_COLUMN_MAX: f32 = 280.0;
/// Average glyph advance of the 12.5 pt table text, used to size columns
/// before layout (the exact width is not known until the text is shaped).
const TABLE_GLYPH_ADVANCE: f32 = 7.0;

/// One flex basis per column: wide enough for the longest cell's rendered
/// text (markers removed) up to [`TABLE_COLUMN_MAX`], never under
/// [`TABLE_COLUMN_MIN`]. Every row uses the same bases, so the columns line
/// up, and their sum is what the frame scrolls when the bubble is narrower.
pub fn table_column_widths(header: &[String], rows: &[Vec<String>]) -> Vec<f32> {
    let cols = header.len().max(1);
    (0..cols)
        .map(|ci| {
            let longest = std::iter::once(header)
                .chain(rows.iter().map(Vec::as_slice))
                .filter_map(|row| row.get(ci))
                .map(|cell| inline_plain(cell).chars().count())
                .max()
                .unwrap_or(0);
            // Padding on both sides plus a little slack so "10:30am–7:30pm" never breaks
            // inside the word (the Lima shot of 2026-10-25 23:2x).
            (longest as f32 * TABLE_GLYPH_ADVANCE + 28.0).clamp(TABLE_COLUMN_MIN, TABLE_COLUMN_MAX)
        })
        .collect()
}

/// The code card's Copy pill, wired to its owner (Jacob 2026-10-24 22:22:
/// the pill showed no click effect). `on_copy` puts the code on the
/// clipboard and starts the "Copied" feedback for pill `id`; `copied` is the
/// pill showing it right now.
#[derive(Clone, Default)]
pub struct CodeCopy {
    pub copied: Option<SharedString>,
    #[allow(clippy::type_complexity)]
    pub on_copy: Option<Rc<dyn Fn(SharedString, String, &mut Window, &mut App)>>,
    /// The chart cards' actions (hover repaint, Save PNG, Expand); Copy data goes through
    /// `on_copy` with the card's own square id ([feedback chart-bubble-mac-2026-10-26]).
    pub chart: Option<Rc<crate::ui::chart_card::ChartUi>>,
    /// The link preview cards' wiring (pictures, open, Remove preview)
    /// ([feedback link-embeds-mac-2026-10-27]).
    pub links: Option<Rc<crate::ui::link_card::LinkUi>>,
    /// I moderate this chat (my own project, or a group I own): Remove preview shows on
    /// others' cards too.
    pub moderates: bool,
}

/// How long the Copy pill reads "Copied" (Jacob: ~1.5 s).
pub const CODE_COPY_FEEDBACK_MS: u64 = 1_500;

/// The block's text to the clipboard; `slot` remembers which pill to show
/// "Copied" on and since when.
fn copy_code_to_clipboard(
    slot: &mut Option<(SharedString, Instant)>,
    pill: SharedString,
    code: String,
    cx: &mut App,
) {
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(code));
    *slot = Some((pill, Instant::now()));
}

/// The pill still inside its "Copied" window at `now`, if any.
fn copied_pill(slot: &Option<(SharedString, Instant)>, now: Instant) -> Option<SharedString> {
    slot.as_ref()
        .filter(|(_, at)| {
            now.saturating_duration_since(*at) < Duration::from_millis(CODE_COPY_FEEDBACK_MS)
        })
        .map(|(id, _)| id.clone())
}

/// The Copy pill's (background, text) at rest, under the pointer, while the
/// button is down (the chips' pressed tint, #1636) and while it says
/// "Copied" (the rail's check colour).
pub fn copy_pill_colors(theme: &Theme, state: CopyPillState) -> (gpui::Hsla, gpui::Hsla) {
    match state {
        CopyPillState::Rest => (gpui::transparent_black(), theme.text_faint),
        CopyPillState::Hover => (theme.surface_2, theme.text),
        CopyPillState::Pressed => (
            mention::pill_color(theme.accent, mention::PILL_ALPHA_PRESSED),
            theme.accent,
        ),
        CopyPillState::Copied => (
            Theme::with_alpha(theme.online, mention::PILL_ALPHA),
            theme.online,
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyPillState {
    Rest,
    Hover,
    Pressed,
    Copied,
}

/// The code card's mono text size, line height and glyph advance (JetBrains
/// Mono / Menlo at 12.5 pt: 0.6 em). The advance places the indent guides
/// (`docs/mac-design-rules.md` § Code cards).
pub const CODE_TEXT: f32 = 12.5;
pub const CODE_LINE_HEIGHT: f32 = 18.0;
pub const CODE_GLYPH_ADVANCE: f32 = 7.5;
/// Body insets; the right inset leaves room for the copy button.
pub const CODE_PAD_X: f32 = 10.0;
pub const CODE_PAD_Y: f32 = 10.0;
pub const CODE_PAD_RIGHT: f32 = 40.0;
/// The copy button: a bordered square in the card's top-right corner.
pub const CODE_COPY_SIZE: f32 = 24.0;
pub const CODE_COPY_INSET: f32 = 8.0;

/// Fenced ``` block as Grok Bot's code card ([feedback
/// code-highlighting-mac-2026-10-26]): no header row, a copy icon floating
/// top-right, the mono body coloured by `arbos_core::highlight` (keywords,
/// function names, strings, numbers, comments in italic), 1 px indent guides,
/// and long lines scrolling sideways inside the card.
#[allow(clippy::too_many_arguments)]
fn code_fence_card(
    row_id: &str,
    lang: &str,
    code: &str,
    color: gpui::Hsla,
    theme: &Theme,
    bubble: &BubbleSelectable,
    at: usize,
    veil: Option<&Veil>,
    now: Instant,
    copy: &CodeCopy,
) -> AnyElement {
    let mono = gpui::Font {
        family: theme.font_mono.into(),
        features: Default::default(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: gpui::FontStyle::Normal,
    };
    let tokens = arbos_core::highlight::highlight(lang, code);
    let runs = code_runs(code, &tokens, &mono, color, theme);
    let runs = match veil {
        Some(v) => apply_veil(runs, &v.spans(now), 0),
        None => runs,
    };
    let guides = indent_guides(code);
    let body = div()
        .flex_shrink_0()
        .min_w_full()
        .relative()
        .debug_selector(|| "code-body".into())
        .pl(px(CODE_PAD_X))
        .pr(px(CODE_PAD_RIGHT))
        .py(px(CODE_PAD_Y))
        .text_size(px(CODE_TEXT))
        .line_height(px(CODE_LINE_HEIGHT))
        .whitespace_nowrap()
        .children(guides.into_iter().map(|(line, x)| {
            div()
                .absolute()
                .left(px(CODE_PAD_X + x))
                .top(px(CODE_PAD_Y + line as f32 * CODE_LINE_HEIGHT))
                .w(px(1.0))
                .h(px(CODE_LINE_HEIGHT))
                .bg(theme.code_guide)
                .debug_selector(move || format!("code-guide-{line}"))
        }))
        .child(bubble.text(at, code.to_string(), runs));
    // Long lines (a fenced table, a wide log line) keep their columns and
    // scroll sideways instead of wrapping into an unreadable stack.
    div()
        .flex()
        .flex_col()
        .w_full()
        .relative()
        .rounded(px(10.0))
        .bg(theme.code_bg)
        .border_1()
        .border_color(theme.border)
        .child(
            sideways_scroll(
                div()
                    .id(SharedString::from(format!("{row_id}#code")))
                    .w_full(),
            )
            .child(body),
        )
        .child(
            div()
                .absolute()
                .top(px(CODE_COPY_INSET))
                .right(px(CODE_COPY_INSET))
                .child(copy_pill(row_id, code, theme, copy)),
        )
        .into_any_element()
}

/// The code's styled runs: one per highlight token, mono, the theme's colour
/// for the token's kind, comments italic. Plain (no tokens) → one run in `color`.
fn code_runs(
    code: &str,
    tokens: &[arbos_core::highlight::Token],
    mono: &gpui::Font,
    color: gpui::Hsla,
    theme: &Theme,
) -> Vec<TextRun> {
    use arbos_core::highlight::TokenKind;
    let run = |len: usize, color: gpui::Hsla, italic: bool| TextRun {
        len,
        font: gpui::Font {
            style: if italic {
                gpui::FontStyle::Italic
            } else {
                gpui::FontStyle::Normal
            },
            ..mono.clone()
        },
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    if tokens.is_empty() {
        return vec![run(code.len(), color, false)];
    }
    let mut runs = Vec::with_capacity(tokens.len());
    let mut pos = 0usize;
    for t in tokens {
        if t.start > pos {
            runs.push(run(t.start - pos, color, false));
        }
        if t.end > t.start {
            let tone = if t.kind == TokenKind::Plain {
                color
            } else {
                theme.code_color(t.kind)
            };
            runs.push(run(t.end - t.start, tone, t.kind == TokenKind::Comment));
        }
        pos = pos.max(t.end);
    }
    if pos < code.len() {
        runs.push(run(code.len() - pos, color, false));
    }
    runs
}

/// Where the indent guides go: `(line, x)` in points from the body's left
/// inset, one per completed indent step on every non-blank line, a little past
/// the middle of the step (Grok: column 2.6 of a 4-space step). The step is the
/// block's smallest indent (tabs count 4), at least 2.
pub fn indent_guides(code: &str) -> Vec<(usize, f32)> {
    let indent_of = |line: &str| -> usize {
        line.chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .map(|c| if c == '\t' { 4 } else { 1 })
            .sum()
    };
    let unit = code
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(indent_of)
        .filter(|n| *n > 0)
        .min()
        .unwrap_or(4)
        .max(2);
    let mut out = Vec::new();
    for (li, line) in code.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let indent = indent_of(line);
        for k in 0..indent / unit {
            out.push((
                li,
                (k * unit) as f32 * CODE_GLYPH_ADVANCE + 0.65 * unit as f32 * CODE_GLYPH_ADVANCE,
            ));
        }
    }
    out
}

/// The code card's copy button: Grok's bordered square with the copy glyph,
/// top-right in the card. It answers the pointer — hover, a pressed tint while
/// the button is down — and shows a check in the online colour for
/// [`CODE_COPY_FEEDBACK_MS`] after a click (`#copy-check`).
fn copy_pill(row_id: &str, code: &str, theme: &Theme, copy: &CodeCopy) -> AnyElement {
    let id = SharedString::from(format!("{row_id}#copy"));
    let rig = rig_copy_pill();
    let copied = copy.copied.as_ref() == Some(&id) || rig == Some(CopyPillState::Copied);
    let state = match rig {
        Some(state) if !copied => state,
        _ if copied => CopyPillState::Copied,
        _ => CopyPillState::Rest,
    };
    let (bg, fg) = copy_pill_colors(theme, state);
    let (hover_bg, hover_fg) = copy_pill_colors(theme, CopyPillState::Hover);
    let (down_bg, down_fg) = copy_pill_colors(theme, CopyPillState::Pressed);
    let source = code.to_string();
    let on_copy = copy.on_copy.clone();
    let pill_id = id.clone();
    // The rest tint is transparent: the card's own colour shows through, so the
    // square reads as part of the card and still covers the code under it.
    let base = if bg.a == 0.0 { theme.code_bg } else { bg };
    div()
        .id(id)
        .debug_selector(move || format!("{row_id}#copy-pill"))
        .size(px(CODE_COPY_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .border_1()
        .border_color(theme.border)
        .cursor_pointer()
        .bg(base)
        .text_color(fg)
        .when(!copied, |el| {
            // hover-fill: exception code-card
            el.hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                // hover-fill: exception code-card
                .active(move |s| s.bg(down_bg).text_color(down_fg))
        })
        .child(if copied {
            div()
                .debug_selector(|| "copy-check".into())
                .child(sf_symbols::glyph("checkmark", 13.0, fg))
        } else {
            div().child(sf_symbols::glyph("doc.on.doc", 13.0, fg))
        })
        .on_click(move |_, window, cx| {
            if let Some(on_copy) = &on_copy {
                on_copy(pill_id.clone(), source.clone(), window, cx);
            }
        })
        .into_any_element()
}

/// Screenshot rig (`ARBOS_RIG_CODE_COPY=hover|pressed|copied`): every Copy
/// pill paints in that state — the EC2 Mac's VNC session cannot hold a
/// button through a capture, and "Copied" lasts 1.5 s.
fn rig_copy_pill() -> Option<CopyPillState> {
    static RIG: std::sync::OnceLock<Option<CopyPillState>> = std::sync::OnceLock::new();
    *RIG.get_or_init(|| match std::env::var("ARBOS_RIG_CODE_COPY").ok()?.trim() {
        "hover" => Some(CopyPillState::Hover),
        "pressed" => Some(CopyPillState::Pressed),
        "copied" => Some(CopyPillState::Copied),
        _ => None,
    })
}

/// Gap from the sender's name to the text (Jacob's reference: tight) and from
/// the reply quote header to the text (#1393).
const NAME_TO_TEXT_GAP: f32 = 2.0;
const QUOTE_TO_TEXT_GAP: f32 = 6.0;

/// The inside of a bubble, in this order: sender name (group / channel,
/// someone else) → reply quote header → the text. The time is never part of
/// the bubble (hover-only, see [`hover_time`]; timestamps-hover-only-2026-10-24).
///
/// Width rules:
/// - The column and the text row are `w_full` so they take the bubble's
///   content box when the bubble is capped — the body can wrap at that edge.
/// - No `min_w_0` on the column or the text row: taffy sizes a flex
///   column from its items, and an item with `min-width: 0` contributes
///   nothing, so the bubble would take the name's or the quote's width and
///   wrap the body one word per line (bubble-quote-width-2026-10-23).
/// - The text wrapper alone keeps `min_w_0`. The text line is a flex *row*,
///   and there the rule is the opposite of the column: GPUI measures a
///   text's min-content width as its unwrapped width
///   (`AvailableSpace::MinContent` → no wrap width), so without `min_w_0`
///   the item's automatic minimum is the whole line, it cannot shrink to
///   the bubble, and the text runs out of the bubble's right edge (Jacob,
///   2026-10-23, v0.6.184 / 0.6.185: bubble-overflow-hotfix-2026-10-23,
///   app-01M5N8R9VYF64GWW8TVXW8HFX9). With `min_w_0` the row still reports
///   the unwrapped width as its max-content size (a flex item's basis is its
///   content; the minimum only matters when shrinking), so a short bubble is
///   still content-sized, and a long one wraps at the bubble's width.
fn sender_bubble_column(
    name: Option<AnyElement>,
    quote: Option<AnyElement>,
    body: AnyElement,
) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .children(name.map(|n| {
            div()
                .pb(px(NAME_TO_TEXT_GAP))
                .debug_selector(|| "bubble-name".into())
                .child(n)
        }))
        .children(quote.map(|q| {
            div()
                .pb(px(QUOTE_TO_TEXT_GAP))
                .debug_selector(|| "bubble-quote".into())
                .child(q)
        }))
        .child(
            div().flex().flex_wrap().items_end().w_full().child(
                div()
                    .min_w_0()
                    .debug_selector(|| "bubble-text".into())
                    .child(body),
            ),
        )
}

/// Whether the open chat shows who sent each bubble (Jacob, 2026-10-23):
/// group chats and server channels do, people and agents alike; a 1:1 chat —
/// me and a person, or me and an agent — never labels the other side. A DM
/// with an agent added, an agent group, or a project with other humans in it
/// has three parties and counts as a group.
pub fn names_senders(space: Option<&Project>, chat: Option<&Chat>, channel: bool) -> bool {
    if channel {
        return true;
    }
    let Some(space) = space else {
        return false;
    };
    match space.kind {
        SpaceKind::Group => true,
        SpaceKind::Dm => !space.agents.is_empty(),
        SpaceKind::Project => {
            chat.is_some_and(Chat::is_group) || space.shared || !space.people.is_empty()
        }
    }
}

/// Whether the open chat is a people chat — one whose bubbles carry a sender
/// disc on the avatar column: a DM (with or without agents), a group, a
/// server channel, or a project with other people in it. A 1:1 agent chat is
/// not; its bubbles keep Grok's agent-chat widths ([`assistant_cap`],
/// [`OWN_BUBBLE_FRACTION`]).
pub fn people_chat(space: Option<&Project>, chat: Option<&Chat>, channel: bool) -> bool {
    if channel {
        return true;
    }
    let Some(space) = space else {
        return false;
    };
    match space.kind {
        SpaceKind::Group | SpaceKind::Dm => true,
        SpaceKind::Project => names_senders(Some(space), chat, false),
    }
}

#[cfg(test)]
mod selection_tests {
    //! The bubble's plain text is one string through every block (#1389):
    //! a drag from the first paragraph to the last selects it end to end,
    //! and what ⌘C copies is exactly the text the bubble shows.
    use super::*;

    const REPLY: &str = "I looked at the records.\n\nThe **likeliest** cause is the prompt, not an app bug:\n\n- agents answered at once\n- nobody waited for `everyone`\n\n```sh\narbos replay 12\n```\n\n| who | rolled |\n| --- | --- |\n| Bob | 4 |\n| Sam | 6 |\n\nWant a rule summary before the next round?";

    fn blocks() -> Vec<Block> {
        split_blocks(REPLY)
    }

    #[test]
    fn bubble_plain_is_what_the_bubble_shows() {
        let text = bubble_plain(&blocks(), false);
        assert_eq!(
            text,
            "I looked at the records.\n\
             The likeliest cause is the prompt, not an app bug:\n\
             • agents answered at once\n\
             • nobody waited for everyone\n\
             arbos replay 12\n\
             who | rolled\nBob | 4\nSam | 6\n\
             Want a rule summary before the next round?"
        );
    }

    #[test]
    fn every_block_starts_where_its_offset_says() {
        let blocks = blocks();
        let text = bubble_plain(&blocks, false);
        let offsets = block_offsets(&blocks, false);
        assert_eq!(offsets.len(), blocks.len());
        for (block, at) in blocks.iter().zip(&offsets) {
            let plain = block_plain(block, false);
            assert_eq!(&text[*at..*at + plain.len()], plain, "block at {at}");
        }
        // The segments the elements are built from tile each block exactly.
        for block in &blocks {
            let segs = block_segments(block, false);
            let joined: String = segs
                .iter()
                .map(|s| format!("{}{}", s.lead, s.text))
                .collect();
            assert_eq!(joined, block_plain(block, false));
        }
    }

    #[test]
    fn user_bubble_plain_text_is_literal() {
        let typed = "see **this**\n\n```py\nprint(1)\n```\n\n- a\n- b";
        let blocks = split_blocks(typed);
        assert_eq!(
            bubble_plain(&blocks, true),
            "see **this**\nprint(1)\n• a\n• b"
        );
        let one = split_blocks("# not a heading");
        assert_eq!(bubble_plain(&one, true), "not a heading");
    }

    /// A typed bubble's plain text — what its selection reads — is the text
    /// its elements draw: bare URLs carry their wrap hints in both, with and
    /// without a code fence, and every block still starts at its offset.
    #[test]
    fn user_bubble_plain_text_hints_its_urls_like_the_elements_draw_them() {
        let typed = "see https://a.io/x/y now";
        assert_eq!(user_bubble_plain(typed), urlwrap::hint_bare_urls(typed));
        assert_ne!(user_bubble_plain(typed), typed, "the hints are in");
        assert_eq!(urlwrap::plain(&user_bubble_plain(typed)), typed);

        let fenced = "see https://a.io/x/y\n\n```sh\ncurl https://a.io/x/y\n```\n\nhttps://b.io/z";
        let blocks = split_blocks(fenced);
        let text = bubble_plain(&blocks, true);
        assert_eq!(user_bubble_plain(fenced), text);
        assert!(
            text.contains(&urlwrap::breakable("https://b.io/z")),
            "prose URLs are hinted"
        );
        assert!(
            text.contains("curl https://a.io/x/y"),
            "code is as typed: {text:?}"
        );
        for (block, at) in blocks.iter().zip(block_offsets(&blocks, true)) {
            let plain = block_plain(block, true);
            assert_eq!(&text[at..at + plain.len()], plain, "block at {at}");
        }
    }

    #[test]
    fn table_cells_pad_to_the_header() {
        let block = Block::Table {
            header: vec!["a".into(), "b".into(), "c".into()],
            rows: vec![
                vec!["1".into()],
                vec!["2".into(), "3".into(), "4".into(), "5".into()],
            ],
        };
        assert_eq!(block_plain(&block, false), "a | b | c\n1 |  | \n2 | 3 | 4");
        assert_eq!(block_segments(&block, false).len(), 9);
    }

    /// The reply to Jacob's "Find me a book store in lima" (Bubble, 2026-10-25 21:31 UTC),
    /// verbatim: a plain GFM table whose first column is inline links.
    const LIMA_TABLE: &str = "| Store | Area | Sunday | What they sell |\n| --- | --- | --- | --- |\n| [Librería El Virrey](https://www.elvirrey.com/) | Miraflores, Calle Bolognesi 510 | 11am–7pm | Spanish literature, essays, history, art; some English at a higher price |\n| [Communitas](https://www.communitas.pe/) | San Isidro, Av. 2 de Mayo 1690 | 10:30am–7:30pm | Humanities, literature, law, history, science, in Spanish |\n| [Vallejo Librería Café](https://www.tangol.com/blog/en/the-3-best-bookstores-in-lima/n-15210) | San Isidro, Av. Camino Real 1119 | not listed | Books plus coffee or tea |\n| [Assouline Lima](https://www.assouline.com/pages/store/lima) | Miraflores, street not listed | not listed | Luxury and design books |";

    /// [feedback markdown-tables-mac-2026-10-25]: the Lima table is one real table whose
    /// cells are inline markdown — the store names are links (text = name, href = page),
    /// never raw `[…](…)`; a selection copies the names, not the markup.
    #[test]
    fn the_lima_table_is_a_table_of_links() {
        let blocks = split_blocks(LIMA_TABLE);
        let [Block::Table { header, rows }] = blocks.as_slice() else {
            panic!("one table, got {blocks:?}");
        };
        assert_eq!(header.len(), 4);
        assert_eq!(rows.len(), 4);
        let plain = bubble_plain(&blocks, false);
        assert!(
            !plain.contains("](") && !plain.contains("https://"),
            "{plain}"
        );
        assert!(plain.starts_with(&format!(
            "Store | Area | Sunday | What they sell\n{}Librería El Virrey | Miraflores",
            markdown::LINK_ICON_CELL
        )));
        let theme = Theme::dark(crate::ui::theme::Surface::Matte);
        let (text, runs, links, icons, _) =
            inline_runs(&rows[0][0], theme.text_muted, &theme, 0, &|_| false);
        let cell = markdown::LINK_ICON_CELL;
        assert_eq!(text, format!("{cell}Librería El Virrey"));
        assert_eq!(
            links,
            vec![(0..text.len(), "https://www.elvirrey.com/".to_string())]
        );
        assert_eq!(icons, vec![(0..cell.len(), "www.elvirrey.com".to_string())]);
        assert_eq!(
            runs[0].color,
            gpui::transparent_black(),
            "the icon cell is unpainted text"
        );
        assert_eq!(
            runs[1].color, theme.accent,
            "a link is painted in the accent"
        );
        assert!(runs[1].underline.is_some());
        // Columns: the long "What they sell" column caps out; "Sunday" stays narrow.
        let widths = table_column_widths(header, rows);
        assert_eq!(widths.len(), 4);
        assert_eq!(widths[3], TABLE_COLUMN_MAX);
        assert!(widths[2] < widths[1] && widths[1] < widths[3], "{widths:?}");
        assert!(widths.iter().all(|w| *w >= TABLE_COLUMN_MIN));
    }

    /// Pipe rows without the `| --- |` row are not a table: they degrade to one monospace
    /// block (the code card, which scrolls sideways) instead of prose with raw pipes. An
    /// indented table still parses as a table.
    #[test]
    fn pipe_rows_without_a_separator_degrade_to_a_mono_block() {
        let mut lines: Vec<&str> = LIMA_TABLE.lines().collect();
        lines.remove(1);
        let headless = lines.join("\n");
        let blocks = split_blocks(&headless);
        let [Block::Code { lang, code }] = blocks.as_slice() else {
            panic!("one mono block, got {blocks:?}");
        };
        assert!(lang.is_empty());
        assert_eq!(code, &headless);
        let indented = LIMA_TABLE
            .lines()
            .map(|l| format!("    {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(matches!(
            split_blocks(&indented).as_slice(),
            [Block::Table { .. }]
        ));
        assert!(matches!(
            split_blocks("one | pipe in prose\nnext line").as_slice(),
            [Block::Paragraph(_)]
        ));
    }

    struct TableHost {
        selection: Entity<TextSelection>,
        width: f32,
        text: &'static str,
    }

    impl gpui::Render for TableHost {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            let blocks = split_blocks(self.text);
            let texts = BubbleText::for_blocks(&blocks, false);
            let selectable = Selectable::new(&self.selection, theme.accent_soft, cx);
            let bubble = selectable.bubble("m1#text", texts[0].text.clone());
            div()
                .w(px(self.width))
                .debug_selector(|| "test-host".into())
                .child(render_block(
                    "m1#t.0",
                    &blocks[0],
                    texts[0].at,
                    None,
                    Instant::now(),
                    &theme,
                    &bubble,
                    &CodeCopy::default(),
                ))
        }
    }

    fn render_lima_table(
        cx: &mut gpui::TestAppContext,
        width: f32,
    ) -> (
        gpui::Bounds<Pixels>,
        gpui::Bounds<Pixels>,
        gpui::Bounds<Pixels>,
        gpui::Bounds<Pixels>,
    ) {
        cx.update(|cx| {
            let theme = crate::ui::theme::Theme::for_pref(
                crate::ui::theme::AppearancePref::Dark,
                crate::ui::theme::Surface::Matte,
                cx,
            );
            cx.set_global(theme);
        });
        let (_view, vcx) = cx.add_window_view(|_, cx| TableHost {
            selection: cx.new(TextSelection::new),
            width,
            text: LIMA_TABLE,
        });
        vcx.run_until_parked();
        let host = vcx.debug_bounds("test-host").expect("host drawn");
        let table = vcx.debug_bounds("table").expect("table drawn");
        let first_link_cell = vcx
            .debug_bounds("table-cell-r1-c0")
            .expect("first body cell drawn");
        let last_cell = vcx
            .debug_bounds("table-cell-r0-c3")
            .expect("last header-row cell drawn");
        (host, table, first_link_cell, last_cell)
    }

    /// Headless render of the Lima table in a wide bubble: every cell is drawn, the columns
    /// fill the bubble, nothing is squeezed under its minimum.
    #[gpui::test]
    fn the_lima_table_renders_as_cells_that_fill_a_wide_bubble(cx: &mut gpui::TestAppContext) {
        let (host, table, first, last) = render_lima_table(cx, 900.0);
        assert!(
            (f32::from(table.size.width) - 900.0).abs() < 3.0,
            "{:?}",
            table.size
        );
        assert!(f32::from(first.size.width) >= TABLE_COLUMN_MIN);
        assert!(
            f32::from(last.size.width) >= TABLE_COLUMN_MAX,
            "{:?}",
            last.size
        );
        assert!(last.right() <= host.right() + px(1.0));
    }

    /// The same table in a narrow bubble (the iPhone-width case) overflows its frame — the
    /// frame scrolls sideways — instead of shrinking the columns under their bases.
    #[gpui::test]
    fn the_lima_table_scrolls_sideways_in_a_narrow_bubble(cx: &mut gpui::TestAppContext) {
        let blocks = split_blocks(LIMA_TABLE);
        let Block::Table { header, rows } = &blocks[0] else {
            unreachable!()
        };
        let bases: f32 = table_column_widths(header, rows).iter().sum();
        let (host, table, first, last) = render_lima_table(cx, 360.0);
        assert!((f32::from(host.size.width) - 360.0).abs() < 1.0);
        assert!(
            f32::from(table.size.width) >= bases - 1.0,
            "{:?} vs {bases}",
            table.size
        );
        assert!(f32::from(first.size.width) >= TABLE_COLUMN_MIN);
        assert!(
            last.right() > host.right(),
            "the last column lies past the frame: scrollable"
        );
    }

    /// A fenced (or separator-less) table that still arrives is a code card: its long lines
    /// keep their columns and the card scrolls sideways instead of wrapping or cutting off.
    #[gpui::test]
    fn a_code_card_with_long_lines_scrolls_sideways(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let theme = crate::ui::theme::Theme::for_pref(
                crate::ui::theme::AppearancePref::Dark,
                crate::ui::theme::Surface::Matte,
                cx,
            );
            cx.set_global(theme);
        });
        const FENCED: &str = "```\n| Store | Area | Sunday | What they sell | Phone | Source | Checked |\n| [Librería El Virrey](https://www.elvirrey.com/) | Miraflores, Calle Bolognesi 510 | 11am–7pm | Spanish literature | (511) 713 0505 | own site | yes |\n```";
        let (_view, vcx) = cx.add_window_view(|_, cx| TableHost {
            selection: cx.new(TextSelection::new),
            width: 300.0,
            text: FENCED,
        });
        vcx.run_until_parked();
        let host = vcx.debug_bounds("test-host").expect("host drawn");
        let body = vcx.debug_bounds("code-body").expect("code body drawn");
        assert!(
            body.size.width > host.size.width + px(200.0),
            "{:?} vs {:?}: the line is laid out unwrapped, past the frame",
            body.size,
            host.size
        );
        assert!(
            body.size.height < px(80.0),
            "{:?}: two lines, not a wrapped stack",
            body.size
        );
    }

    /// Drag selection across the whole reply and copy: the clipboard text
    /// (what ⌘C writes, `TextSelection::selected_text`) equals the bubble's
    /// plain text.
    #[test]
    fn copy_of_a_full_drag_equals_the_bubble_plain_text() {
        let blocks = blocks();
        let text = bubble_plain(&blocks, false);
        let shared: SharedString = text.clone().into();
        let mut sel = crate::ui::select::Model::default();
        sel.begin("m1#text".into(), shared.clone(), 0, 1, false);
        sel.extend_to(shared.len());
        assert!(!sel.finish_drag());
        assert_eq!(sel.selected_text(), Some(text));
    }

    /// A drag from inside paragraph one into the list: the paragraph tail, the
    /// second paragraph and the first bullet (with its marker) come along.
    #[test]
    fn drag_across_three_blocks_selects_them_contiguously() {
        let blocks = blocks();
        let text = bubble_plain(&blocks, false);
        let offsets = block_offsets(&blocks, false);
        let shared: SharedString = text.into();
        let from = offsets[0] + "I looked ".len();
        let to = offsets[2] + "• agents".len();
        let mut sel = crate::ui::select::Model::default();
        sel.begin("m1#text".into(), shared, from, 1, false);
        sel.extend_to(to);
        assert_eq!(
            sel.selected_text().as_deref(),
            Some("at the records.\nThe likeliest cause is the prompt, not an app bug:\n• agents")
        );
    }

    /// Every block of the reply drawn through `render_block` with the whole
    /// bubble selected: each element's text must sit at its offset in the
    /// bubble's plain text (the `debug_assert` in `BubbleSelectable::text`),
    /// and the elements' heights add up (nothing collapsed or doubled).
    struct Bubble {
        selection: Entity<TextSelection>,
    }

    impl gpui::Render for Bubble {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            let blocks = blocks();
            let texts = BubbleText::for_blocks(&blocks, false);
            let selectable = Selectable::new(&self.selection, theme.accent_soft, cx);
            let bubble = selectable.bubble("m1#text", texts[0].text.clone());
            let mut col = div().w(px(600.0)).flex().flex_col();
            for (i, block) in blocks.iter().enumerate() {
                col = col.child(
                    div()
                        .debug_selector(move || format!("test-block-{i}"))
                        .child(render_block(
                            &format!("m1#t.{i}"),
                            block,
                            texts[i].at,
                            None,
                            Instant::now(),
                            &theme,
                            &bubble,
                            &CodeCopy::default(),
                        )),
                );
            }
            col
        }
    }

    #[gpui::test]
    fn every_block_renders_at_its_offset_with_the_bubble_selected(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let theme = crate::ui::theme::Theme::for_pref(
                crate::ui::theme::AppearancePref::Dark,
                crate::ui::theme::Surface::Matte,
                cx,
            );
            cx.set_global(theme);
        });
        let text: SharedString = bubble_plain(&blocks(), false).into();
        let (_view, vcx) = cx.add_window_view(|_, cx| {
            let selection = cx.new(TextSelection::new);
            selection.update(cx, |s, _| {
                let m = s.model_for_tests();
                m.begin("m1#text".into(), text.clone(), 0, 1, false);
                m.extend_to(text.len());
                m.finish_drag();
            });
            Bubble { selection }
        });
        vcx.run_until_parked();
        let n = blocks().len();
        let mut bottom = 0.0f32;
        for i in 0..n {
            let selector: &'static str = Box::leak(format!("test-block-{i}").into_boxed_str());
            let b = vcx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("block {i} drawn"));
            assert!(f32::from(b.origin.y) >= bottom - 0.5, "block {i} overlaps");
            assert!(f32::from(b.size.height) > 0.0, "block {i} has height");
            bottom = f32::from(b.origin.y + b.size.height);
        }
    }

    /// A code card on its own, with the Copy pill wired the way the
    /// transcript wires it (Jacob 2026-10-24 22:22).
    struct CodeCard {
        selection: Entity<TextSelection>,
        code_copied: Option<(SharedString, Instant)>,
    }

    const SNIPPET: &str = "fn main() {\n    println!(\"hi\");\n}";

    impl gpui::Render for CodeCard {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            let block = Block::Code {
                lang: "rust".into(),
                code: SNIPPET.into(),
            };
            let texts = BubbleText::for_blocks(std::slice::from_ref(&block), false);
            let selectable = Selectable::new(&self.selection, theme.accent_soft, cx);
            let bubble = selectable.bubble("m1#text", texts[0].text.clone());
            let this = cx.entity().downgrade();
            let copy = CodeCopy {
                copied: copied_pill(&self.code_copied, Instant::now()),
                on_copy: Some(Rc::new(move |id, code, _, cx| {
                    let _ = this.update(cx, |v, cx| {
                        copy_code_to_clipboard(&mut v.code_copied, id, code, cx);
                        cx.notify();
                    });
                })),
                chart: None,
                ..Default::default()
            };
            div().w(px(600.0)).child(render_block(
                "m1#t.0",
                &block,
                texts[0].at,
                None,
                Instant::now(),
                &theme,
                &bubble,
                &copy,
            ))
        }
    }

    #[gpui::test]
    fn the_copy_pill_puts_the_code_on_the_clipboard_and_says_copied(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let theme = crate::ui::theme::Theme::for_pref(
                crate::ui::theme::AppearancePref::Dark,
                crate::ui::theme::Surface::Matte,
                cx,
            );
            cx.set_global(theme);
        });
        let (view, vcx) = cx.add_window_view(|_, cx| CodeCard {
            selection: cx.new(TextSelection::new),
            code_copied: None,
        });
        vcx.run_until_parked();
        let pill = vcx
            .debug_bounds("m1#t.0#copy-pill")
            .expect("Copy pill drawn");
        let before = pill.size.width;
        vcx.simulate_click(pill.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let clipboard = vcx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
        assert_eq!(
            clipboard.as_deref(),
            Some(SNIPPET),
            "the block's text, verbatim"
        );
        view.read_with(vcx, |v, _| {
            assert_eq!(
                copied_pill(&v.code_copied, Instant::now()).as_deref(),
                Some("m1#t.0#copy"),
                "the pill says Copied"
            );
            assert_eq!(
                copied_pill(
                    &v.code_copied,
                    Instant::now() + Duration::from_millis(CODE_COPY_FEEDBACK_MS)
                ),
                None,
                "and reads Copy again after the window"
            );
        });
        // The square stays a square; the glyph swapped to the check.
        let pill = vcx
            .debug_bounds("m1#t.0#copy-pill")
            .expect("pill still drawn");
        assert_eq!(pill.size.width, before);
        assert_eq!(pill.size.width, px(CODE_COPY_SIZE));
        assert_eq!(pill.size.height, px(CODE_COPY_SIZE));
        assert!(
            vcx.debug_bounds("copy-check").is_some(),
            "the check glyph painted after the click"
        );
    }

    /// Jacob's bubble-sort card ([feedback code-highlighting-mac-2026-10-26]):
    /// the runs carry the theme's six colours, comments italic, and the guides
    /// sit one per indent step on the indented lines only.
    #[test]
    fn the_code_card_paints_grok_colours_and_indent_guides() {
        use arbos_core::highlight::{highlight, TokenKind};
        let code = "def bubble_sort(arr):\n    n = len(arr)\n    for i in range(n):\n        swapped = False\n\n        # If no two elements swapped\n        if not swapped:\n            break\n    return arr";
        let theme = Theme::dark(crate::ui::theme::Surface::Matte);
        let mono = sans_font(&theme);
        let tokens = highlight("python", code);
        let runs = code_runs(code, &tokens, &mono, theme.text, &theme);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), code.len());
        let run_at = |needle: &str| {
            let at = code.find(needle).unwrap();
            let mut pos = 0;
            runs.iter()
                .find(|r| {
                    let here = pos;
                    pos += r.len;
                    here <= at && at < pos
                })
                .unwrap()
                .clone()
        };
        assert_eq!(run_at("def").color, theme.code_keyword);
        assert_eq!(run_at("bubble_sort").color, theme.code_function);
        assert_eq!(run_at("len").color, theme.code_function);
        assert_eq!(run_at("False").color, theme.code_keyword);
        assert_eq!(run_at("# If").color, theme.code_comment);
        assert_eq!(run_at("# If").font.style, gpui::FontStyle::Italic);
        assert_eq!(
            run_at("arr)").color,
            theme.text,
            "plain text keeps the body colour"
        );
        assert_eq!(run_at("def").font.style, gpui::FontStyle::Normal);
        assert_eq!(theme.code_color(TokenKind::Number), theme.code_number);
        // Six distinct colours in both themes, and the themes differ.
        for theme in [
            Theme::dark(crate::ui::theme::Surface::Matte),
            Theme::light(crate::ui::theme::Surface::Matte),
        ] {
            let palette = [
                theme.code_plain,
                theme.code_keyword,
                theme.code_function,
                theme.code_string,
                theme.code_number,
                theme.code_comment,
            ];
            for (i, a) in palette.iter().enumerate() {
                for b in &palette[i + 1..] {
                    assert_ne!(a, b);
                }
            }
            assert!(
                theme.code_guide.a > 0.05 && theme.code_guide.a < 0.1,
                "faint guide"
            );
        }
        assert_ne!(
            Theme::dark(crate::ui::theme::Surface::Matte).code_keyword,
            Theme::light(crate::ui::theme::Surface::Matte).code_keyword
        );
        // Guides: none on `def` (indent 0) and the blank line; one on the 4-space
        // lines, two on the 8-space ones, three at 12; x a little past mid-step.
        let guides = indent_guides(code);
        let per_line = |line: usize| guides.iter().filter(|(l, _)| *l == line).count();
        assert_eq!(per_line(0), 0);
        assert_eq!(per_line(1), 1);
        assert_eq!(per_line(3), 2);
        assert_eq!(per_line(4), 0, "blank line");
        assert_eq!(per_line(7), 3);
        assert_eq!(per_line(8), 1);
        let first = guides.iter().find(|(l, _)| *l == 1).unwrap().1;
        assert!((first - 2.6 * CODE_GLYPH_ADVANCE).abs() < 0.01, "{first}");
        let second = guides.iter().filter(|(l, _)| *l == 3).nth(1).unwrap().1;
        assert!((second - 6.6 * CODE_GLYPH_ADVANCE).abs() < 0.01, "{second}");
        // Two-space code steps every two columns.
        let two = indent_guides("a:\n  b:\n    c");
        assert_eq!(two.iter().filter(|(l, _)| *l == 2).count(), 2);
        // Plain prose block: no tokens, one run.
        let plain = code_runs("hello", &[], &mono, theme.text, &theme);
        assert_eq!(plain.len(), 1);
    }

    #[test]
    fn the_copy_pill_has_a_pressed_and_a_copied_look() {
        let theme = Theme::dark(crate::ui::theme::Surface::Matte);
        let (rest_bg, rest_fg) = copy_pill_colors(&theme, CopyPillState::Rest);
        let (hover_bg, _) = copy_pill_colors(&theme, CopyPillState::Hover);
        let (down_bg, down_fg) = copy_pill_colors(&theme, CopyPillState::Pressed);
        let (done_bg, done_fg) = copy_pill_colors(&theme, CopyPillState::Copied);
        assert_eq!(rest_bg.a, 0.0, "nothing behind the label at rest");
        assert_ne!(hover_bg, rest_bg);
        assert_eq!(
            down_bg.a,
            mention::PILL_ALPHA_PRESSED,
            "the chips' pressed tint"
        );
        assert_eq!(down_fg, theme.accent);
        assert_ne!(down_bg, hover_bg, "pressed is not just hover");
        assert_eq!(done_fg, theme.online);
        assert_eq!(done_bg.a, mention::PILL_ALPHA);
        assert_ne!(rest_fg, down_fg);
    }

    /// Triple-click on the second paragraph takes that paragraph only; a
    /// double-click on a bullet takes its word.
    #[test]
    fn triple_and_double_click_in_a_multi_block_bubble() {
        let blocks = blocks();
        let text = bubble_plain(&blocks, false);
        let offsets = block_offsets(&blocks, false);
        let shared: SharedString = text.into();
        let mut sel = crate::ui::select::Model::default();
        sel.begin("m1#text".into(), shared.clone(), offsets[1] + 4, 3, false);
        assert_eq!(
            sel.selected_text().as_deref(),
            Some("The likeliest cause is the prompt, not an app bug:")
        );
        sel.finish_drag();
        sel.begin(
            "m1#text".into(),
            shared,
            offsets[2] + "• agents ".len() + 2,
            2,
            false,
        );
        assert_eq!(sel.selected_text().as_deref(), Some("answered"));
    }
}

#[cfg(test)]
mod layout_tests {
    //! Pixel parity with Grok Bot's Mac chat (refs 57/58; Jacob's crops of
    //! 2026-10-22): the agent bubble's left edge is the composer's left edge,
    //! and the bubble ends 84 pt before the column's right edge. Rendered
    //! headless through the same frames the shell and the transcript use.
    use gpui::TestAppContext;

    use gpui::Pixels;

    use super::*;
    use crate::ui::presence;
    use crate::ui::theme::COMPOSER_MARGIN;

    /// The reference window: 1180 pt wide with the 296 pt sidebar open.
    const REFERENCE_PANE: f32 = 1180.0 - crate::ui::theme::SIDEBAR_WIDTH;
    /// Grok's own pane width in ref 57 (the composer is bounded by its
    /// margins, not the cap).
    const GROK_REF_57_PANE: f32 = 728.0;

    struct Column {
        pane: f32,
        /// How many people / agents the presence row over the pill carries.
        participants: usize,
    }

    impl gpui::Render for Column {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            // A long agent reply: the bubble wants more than any pane has.
            let bubble = div()
                .debug_selector(|| "test-bubble".into())
                .max_w_full()
                .min_w_0()
                .px(px(BUBBLE_PAD_X))
                .child(div().w(px(4000.0)).h(px(CHAT_LINE_HEIGHT)));
            let row = div()
                .w_full()
                .flex()
                .items_end()
                .child(assistant_slot(bubble))
                .into_any_element();
            // A people-chat bubble row: the sender's disc at the column edge,
            // then the gap, then the bubble (`RowKind::Markdown` with a lane).
            let people_row = div()
                .w_full()
                .flex()
                .items_end()
                .pl(px(AVATAR_COLUMN_INSET))
                .gap(px(SENDER_DISC_GAP))
                .child(
                    div()
                        .size(px(SENDER_DISC))
                        .flex_shrink_0()
                        .rounded_full()
                        .debug_selector(|| "test-disc".into()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(CHAT_LINE_HEIGHT))
                        .debug_selector(|| "test-people-bubble".into()),
                )
                .into_any_element();
            // The composer pill as the shell frames it: the `+` circle 8 pt
            // inside the pill's left edge (7 pt pad + the 1 pt rim), then the
            // field body (gap 4, pad 10) where the placeholder's ink starts.
            let pill = div()
                .w_full()
                .h(px(COMPOSER_HEIGHT))
                .border_1()
                .pl(px((COMPOSER_HEIGHT - 2.0 - PLUS_BUTTON) / 2.0))
                .flex()
                .items_center()
                .gap(px(4.0))
                .debug_selector(|| "test-composer".into())
                .child(
                    div()
                        .size(px(PLUS_BUTTON))
                        .debug_selector(|| "test-plus".into()),
                )
                .child(
                    div().flex_1().pl(px(10.0)).child(
                        div()
                            .h(px(4.0))
                            .debug_selector(|| "test-composer-text".into()),
                    ),
                )
                .into_any_element();
            // The presence row over the pill, framed exactly as the shell
            // frames it (`render_center`: `column_row(presence::row(..))`).
            let theme = Theme::dark(crate::ui::theme::Surface::Matte);
            let names = ["scouttao", "Lium", "Home", "Doug", "Sam"];
            let participants = (0..self.participants)
                .map(|i| presence::Participant {
                    name: names[i % names.len()].to_string(),
                    activity: if i % 2 == 0 {
                        presence::Activity::Typing
                    } else {
                        presence::Activity::Working
                    },
                    picture: div()
                        .size(px(presence::AVATAR))
                        .rounded_full()
                        .into_any_element(),
                    page: None,
                })
                .collect();
            let presence = presence::Presence {
                participants,
                detail: None,
                queued: false,
            };
            let row_over_pill = presence::row(presence, None, &theme, cx).into_any_element();
            // A question / approval card, framed by the real `card_row`: in an agent chat (no
            // lane) and in a people chat (lane) — a card that wants more than any pane has.
            let card = |sel: &'static str| {
                div()
                    .debug_selector(move || sel.into())
                    .max_w_full()
                    .min_w_0()
                    .child(div().w(px(4000.0)).h(px(CHAT_LINE_HEIGHT)))
            };
            let card_plain = card_row(card("test-card"), false).into_any_element();
            let card_people = card_row(card("test-people-card"), true).into_any_element();
            div()
                .w(px(self.pane))
                .h(px(400.0))
                .flex()
                .flex_col()
                .child(column_row(row))
                .child(column_row(people_row))
                .child(column_row(card_plain))
                .child(column_row(card_people))
                .child(column_row(row_over_pill))
                .child(column_row(pill))
        }
    }

    struct Edges {
        bubble_left: f32,
        bubble_width: f32,
        composer_left: f32,
        composer_width: f32,
        plus_left: f32,
        plus: gpui::Bounds<Pixels>,
        column_left: f32,
        column_width: f32,
        presence_left: f32,
        presence_width: f32,
        pictures: usize,
        more: bool,
        text_left: f32,
        picture: gpui::Bounds<Pixels>,
        disc: gpui::Bounds<Pixels>,
        people_bubble_left: f32,
        composer_text_left: f32,
        card: gpui::Bounds<gpui::Pixels>,
        people_card: gpui::Bounds<gpui::Pixels>,
    }

    fn measure(cx: &mut TestAppContext, pane: f32) -> Edges {
        measure_with(cx, pane, 1)
    }

    fn measure_with(cx: &mut TestAppContext, pane: f32, participants: usize) -> Edges {
        // The presence row breathes with the pulse clock (a global the app installs).
        cx.update(|cx| {
            if !cx.has_global::<crate::ui::motion::PulseClock>() {
                crate::ui::motion::PulseClock::init(cx);
            }
        });
        let (_view, vcx) = cx.add_window_view(|_, _| Column { pane, participants });
        vcx.run_until_parked();
        let bubble = vcx.debug_bounds("test-bubble").expect("bubble drawn");
        let composer = vcx.debug_bounds("test-composer").expect("composer drawn");
        let plus = vcx.debug_bounds("test-plus").expect("plus drawn");
        let column = vcx.debug_bounds("chat-column").expect("column drawn");
        let presence = vcx
            .debug_bounds("presence-row")
            .expect("presence row drawn");
        let text = vcx
            .debug_bounds("presence-text")
            .expect("presence text drawn");
        let pictures = [
            "presence-picture-0",
            "presence-picture-1",
            "presence-picture-2",
        ]
        .into_iter()
        .filter(|sel| vcx.debug_bounds(sel).is_some())
        .count();
        let more = vcx.debug_bounds("presence-more").is_some();
        let picture = vcx
            .debug_bounds("presence-picture-0")
            .expect("first picture drawn");
        let disc = vcx.debug_bounds("test-disc").expect("sender disc drawn");
        let people_bubble = vcx
            .debug_bounds("test-people-bubble")
            .expect("people bubble drawn");
        let composer_text = vcx
            .debug_bounds("test-composer-text")
            .expect("composer text drawn");
        let card = vcx.debug_bounds("test-card").expect("card drawn");
        let people_card = vcx
            .debug_bounds("test-people-card")
            .expect("people card drawn");
        Edges {
            bubble_left: bubble.origin.x.into(),
            bubble_width: bubble.size.width.into(),
            composer_left: composer.origin.x.into(),
            composer_width: composer.size.width.into(),
            plus_left: plus.origin.x.into(),
            plus,
            column_left: column.origin.x.into(),
            column_width: column.size.width.into(),
            presence_left: presence.origin.x.into(),
            presence_width: presence.size.width.into(),
            pictures,
            more,
            text_left: text.origin.x.into(),
            picture,
            disc,
            people_bubble_left: people_bubble.origin.x.into(),
            composer_text_left: composer_text.origin.x.into(),
            card,
            people_card,
        }
    }

    /// #1440 follow-up (Jacob, 2026-10-23 17:59): a question / approval card lines up with the
    /// bubbles — card left == bubble left, card width ≤ the bubble cap — in an agent chat and,
    /// after the avatar lane, in a people chat.
    #[gpui::test]
    fn cards_share_the_bubbles_left_edge_and_cap(cx: &mut TestAppContext) {
        for pane in [REFERENCE_PANE, GROK_REF_57_PANE, 1400.0] {
            let e = measure(cx, pane);
            let (cx0, cw): (f32, f32) = (e.card.origin.x.into(), e.card.size.width.into());
            assert!(
                (cx0 - e.bubble_left).abs() <= 0.5,
                "pane {pane}: card left {cx0} vs bubble left {}",
                e.bubble_left
            );
            assert!(
                cw <= e.bubble_width + 0.5,
                "pane {pane}: card width {cw} over the bubble cap {}",
                e.bubble_width
            );
            let (pcx, pcw): (f32, f32) = (
                e.people_card.origin.x.into(),
                e.people_card.size.width.into(),
            );
            assert!(
                (pcx - e.people_bubble_left).abs() <= 0.5,
                "pane {pane}: people card left {pcx} vs people bubble left {}",
                e.people_bubble_left
            );
            // The lane's slot is the column minus the avatar lane; a card there
            // takes the people share of it, like an incoming bubble
            // ([feedback chat-density-mac-2026-10-26]).
            let cap = people_cap(e.column_width - SENDER_LANE);
            assert!(
                (pcw - cap).abs() <= 1.0,
                "pane {pane}: people card width {pcw} vs the lane cap {cap}"
            );
        }
    }

    /// Jacob, 2026-10-26 ([feedback chat-density-mac-2026-10-26]): in people
    /// chats the bubbles use more of the width on both sides. An incoming
    /// bubble (person or agent) fills 80 % of the slot after the sender lane
    /// and leaves 20 % free on the right; my own bubble fills 80 % of the
    /// column from the right and leaves 20 % free on the left — so both still
    /// read as left / right. The agent chat keeps Grok's numbers: the 560 pt
    /// ceiling for the agent, 72 % for me.
    #[gpui::test]
    fn people_chat_bubbles_take_four_fifths_of_their_side(cx: &mut TestAppContext) {
        assert_eq!(PEOPLE_BUBBLE_FRACTION, 0.80);
        assert_eq!(OWN_BUBBLE_FRACTION, 0.72);
        assert_eq!(own_bubble_fraction(true), PEOPLE_BUBBLE_FRACTION);
        assert_eq!(own_bubble_fraction(false), OWN_BUBBLE_FRACTION);
        for pane in [REFERENCE_PANE, GROK_REF_57_PANE, 1400.0] {
            let e = measure(cx, pane);
            let column = e.column_width;
            let slot = column - SENDER_LANE;
            // The people card is the incoming bubble's frame (same slot, same cap).
            let (pcx, pcw): (f32, f32) = (
                e.people_card.origin.x.into(),
                e.people_card.size.width.into(),
            );
            let right_free = e.column_left + column - (pcx + pcw);
            assert!(
                (pcw - 0.80 * slot).abs() <= 1.0,
                "pane {pane}: incoming bubble {pcw} vs 80 % of the slot {}",
                0.80 * slot
            );
            assert!(
                (right_free - 0.20 * slot).abs() <= 1.0,
                "pane {pane}: {right_free} free on the right of an incoming bubble (20 % of the slot = {})",
                0.20 * slot
            );
            // The agent chat's bubble (no lane) is untouched: Grok's cap.
            assert!(
                (e.bubble_width - assistant_cap(column)).abs() <= 1.0,
                "pane {pane}: agent-chat bubble {} vs Grok cap {}",
                e.bubble_width,
                assistant_cap(column)
            );
        }
        // Reference window (852 pt column): incoming 641.6 wide from 66, ending
        // 160 before the column's right edge (was 560 → 242 free); mine 681.6
        // from the right, 170 free on the left (was 613 → 239). One share at
        // every width: below a 750 pt column an agent's group bubble is
        // narrower than Grok's 560 agent-chat ceiling (517 at ref 57's 696),
        // a person's beside it is wider than its old 72 % everywhere.
        let column = REFERENCE_PANE - 2.0 * COMPOSER_MARGIN;
        assert!((people_cap(column - SENDER_LANE) - 641.6).abs() < 0.01);
        assert!((column * own_bubble_fraction(true) - 681.6).abs() < 0.01);
        assert!((column * own_bubble_fraction(false) - 613.44).abs() < 0.01);
    }

    #[gpui::test]
    fn the_presence_picture_is_the_bubble_disc_on_the_plus_column(cx: &mut TestAppContext) {
        // Jacob, 15:15 / 15:20: the row's picture and the bubble's picture are
        // one size at one x, and that x is the composer's `+` disc — centre on
        // centre, the same diameter (`transcript::identity`). The row's text
        // then starts where the bubble body starts.
        for pane in [REFERENCE_PANE, GROK_REF_57_PANE, 1400.0] {
            let e = measure(cx, pane);
            let (px0, pw, ph): (f32, f32, f32) = (
                e.picture.origin.x.into(),
                e.picture.size.width.into(),
                e.picture.size.height.into(),
            );
            let (dx, dw, dh): (f32, f32, f32) = (
                e.disc.origin.x.into(),
                e.disc.size.width.into(),
                e.disc.size.height.into(),
            );
            let plus_w: f32 = e.plus.size.width.into();
            let plus_center: f32 = (e.plus.origin.x + e.plus.size.width / 2.0).into();
            assert_eq!((pw, ph), (dw, dh), "pane {pane}: picture size vs disc size");
            assert_eq!(pw, SENDER_DISC);
            assert_eq!(
                pw, plus_w,
                "pane {pane}: the picture is the `+` disc's size"
            );
            assert!(
                (px0 - dx).abs() <= 0.5,
                "pane {pane}: picture x {px0} vs disc x {dx}"
            );
            assert!(
                ((px0 + pw / 2.0) - plus_center).abs() <= 1.0,
                "pane {pane}: picture centre {} vs `+` centre {plus_center}",
                px0 + pw / 2.0
            );
            assert!(
                ((dx + dw / 2.0) - plus_center).abs() <= 1.0,
                "pane {pane}: disc centre {} vs `+` centre {plus_center}",
                dx + dw / 2.0
            );
            assert!(
                (px0 - (e.column_left + AVATAR_COLUMN_INSET)).abs() <= 0.5,
                "pane {pane}: picture x {px0} vs column {} + X",
                e.column_left
            );
            assert!(
                (e.text_left - e.people_bubble_left).abs() <= 0.5,
                "pane {pane}: text x {} vs people bubble x {}",
                e.text_left,
                e.people_bubble_left
            );
            assert!(
                (e.people_bubble_left - (e.column_left + SENDER_LANE)).abs() <= 0.5,
                "pane {pane}: bubble body at {} (lane {})",
                e.people_bubble_left,
                SENDER_LANE
            );
            // The body (and the row's text) start where the composer's words start.
            assert!(
                (e.people_bubble_left - e.composer_text_left).abs() <= 0.5,
                "pane {pane}: bubble body {} vs composer text {}",
                e.people_bubble_left,
                e.composer_text_left
            );
        }
    }

    #[gpui::test]
    fn the_presence_row_starts_at_the_composers_left_edge(cx: &mut TestAppContext) {
        // Jacob, 2026-10-23: "@scouttao is typing…" sat over the middle of
        // the field. The row's first picture now starts where the bubbles and
        // the pill start, at every pane width, and the text follows the
        // pictures — never a full-width block whose text floats.
        for pane in [REFERENCE_PANE, GROK_REF_57_PANE, 1400.0] {
            let e = measure(cx, pane);
            assert!(
                (e.presence_left - e.composer_left).abs() <= 1.0,
                "pane {pane}: presence row left {} vs composer left {}",
                e.presence_left,
                e.composer_left
            );
            assert!(
                (e.presence_left - e.bubble_left).abs() <= 1.0,
                "pane {pane}: presence row left {} vs bubble left {}",
                e.presence_left,
                e.bubble_left
            );
            assert!(
                (e.presence_width - e.composer_width).abs() <= 1.0,
                "pane {pane}: the row spans the column ({} vs {})",
                e.presence_width,
                e.composer_width
            );
            let expected_text =
                e.presence_left + AVATAR_COLUMN_INSET + presence::AVATAR + presence::GAP;
            assert!(
                (e.text_left - expected_text).abs() <= 1.0,
                "pane {pane}: text at {} (expected {expected_text})",
                e.text_left
            );
        }
    }

    #[gpui::test]
    fn one_presence_row_whatever_the_count_with_one_picture_each(cx: &mut TestAppContext) {
        // Several people typing / agents working: ONE row, N pictures side by
        // side (capped at MAX_PICTURES plus a "+N" disc), one line of text.
        for n in 1..=5 {
            let e = measure_with(cx, REFERENCE_PANE, n);
            let rows = n; // the test column renders exactly one presence row
            assert_eq!(
                e.pictures,
                n.min(presence::MAX_PICTURES),
                "{n} participants"
            );
            assert_eq!(e.more, n > presence::MAX_PICTURES, "{n} participants");
            assert!(
                (e.presence_left - e.composer_left).abs() <= 1.0,
                "{n} participants: row left {} vs composer {}",
                e.presence_left,
                e.composer_left
            );
            // The stack grows by AVATAR − OVERLAP per extra picture; the text
            // stays on the same line, after the stack.
            let shown = n.min(presence::MAX_PICTURES) + usize::from(n > presence::MAX_PICTURES);
            let stack =
                presence::AVATAR + (shown as f32 - 1.0) * (presence::AVATAR - presence::OVERLAP);
            let expected_text = e.presence_left + AVATAR_COLUMN_INSET + stack + presence::GAP;
            assert!(
                (e.text_left - expected_text).abs() <= 1.0,
                "{n} participants: text at {} (expected {expected_text}, rows {rows})",
                e.text_left
            );
        }
    }

    #[gpui::test]
    fn the_agent_bubble_starts_at_the_composers_left_edge(cx: &mut TestAppContext) {
        for pane in [REFERENCE_PANE, GROK_REF_57_PANE, 1400.0] {
            let e = measure(cx, pane);
            assert!(
                (e.bubble_left - e.composer_left).abs() <= 1.0,
                "pane {pane}: bubble left {} vs composer left {}",
                e.bubble_left,
                e.composer_left
            );
            // Grok: the `+` circle is 8 pt inside the pill, so the bubble
            // edge sits 8 pt left of the circle.
            assert!(
                (e.plus_left - e.bubble_left - 8.0).abs() <= 1.0,
                "pane {pane}: plus left {} vs bubble left {}",
                e.plus_left,
                e.bubble_left
            );
            let expected_left = ((pane - COLUMN_MAX_WIDTH) / 2.0).max(COMPOSER_MARGIN);
            assert!(
                (e.bubble_left - expected_left).abs() <= 1.0,
                "pane {pane}: bubble left {} (expected {expected_left})",
                e.bubble_left
            );
        }
    }

    #[gpui::test]
    fn the_agent_bubble_cap_is_the_column_minus_the_grok_gap_at_most_grok_width(
        cx: &mut TestAppContext,
    ) {
        // Reference window (884 pt pane): the margins bound the column, so
        // the composer is 852 wide at 16; the bubble starts at 16 (Grok's
        // left inset) and stops at Grok's 560 pt ceiling (Jacob's shot of
        // 2026-10-24: 1120 px at 2 px/pt), not at the column − 84 = 768 it
        // filled before [feedback bubble-width-2026-10-24].
        let e = measure(cx, REFERENCE_PANE);
        let column = REFERENCE_PANE - 2.0 * COMPOSER_MARGIN;
        assert!((e.column_width - column).abs() <= 1.0, "{}", e.column_width);
        assert!(
            (e.composer_width - column).abs() <= 1.0,
            "{}",
            e.composer_width
        );
        assert!((e.composer_left - 16.0).abs() <= 1.0, "{}", e.composer_left);
        assert!((e.bubble_left - 16.0).abs() <= 1.0, "{}", e.bubble_left);
        assert!(
            (e.bubble_width - ASSISTANT_MAX_WIDTH).abs() <= 1.0,
            "bubble width {} (cap {})",
            e.bubble_width,
            assistant_cap(column)
        );
        assert!(column - ASSISTANT_RIGHT_GAP > ASSISTANT_MAX_WIDTH);
        assert!((e.bubble_left - e.column_left).abs() <= 1.0);
        // A wide pane: the column caps at the composer's 960 (Grok's); the
        // bubble still stops at 560 — 58 % of the column, as in Grok.
        let e = measure(cx, 1400.0);
        assert_eq!(e.column_width, COLUMN_MAX_WIDTH);
        assert_eq!(e.composer_width, COLUMN_MAX_WIDTH);
        assert!(
            (e.bubble_width - ASSISTANT_MAX_WIDTH).abs() <= 1.0,
            "bubble width {}",
            e.bubble_width
        );
        // Grok's narrow pane (ref 57, 2 px/pt): composer 696 pt wide at 16
        // from the pane; the gap rule is the smaller one there (696 − 84 = 612,
        // above the 560 ceiling), so the bubble is 560 wide at 16.
        let e = measure(cx, GROK_REF_57_PANE);
        assert!((e.composer_left - 16.0).abs() <= 1.0, "{}", e.composer_left);
        assert!(
            (e.composer_width - 696.0).abs() <= 1.0,
            "{}",
            e.composer_width
        );
        assert!((e.bubble_left - 16.0).abs() <= 1.0, "{}", e.bubble_left);
        assert!(
            (e.bubble_width - assistant_cap(696.0)).abs() <= 1.0,
            "{}",
            e.bubble_width
        );
        // Narrower than the ceiling allows (ref 58's 424 pt pane): the fixed
        // 84 pt gap rules, as before — 392 − 84 = 310, Grok's measured bubble.
        let e = measure(cx, 424.0);
        assert!((e.bubble_width - 308.0).abs() <= 2.0, "{}", e.bubble_width);
        assert_eq!(assistant_cap(392.0), 308.0);
        assert_eq!(
            assistant_cap(644.0),
            560.0,
            "the crossover is a 644 pt slot"
        );
    }

    struct NamedBubble;

    impl gpui::Render for NamedBubble {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let line = |h: f32| div().w(px(120.0)).h(px(h));
            div().w(px(400.0)).h(px(300.0)).child(
                div()
                    .px(px(BUBBLE_PAD_X))
                    .py(px(BUBBLE_PAD_Y))
                    .child(sender_bubble_column(
                        Some(line(CHAT_LINE_HEIGHT).into_any_element()),
                        Some(line(32.0).into_any_element()),
                        line(CHAT_LINE_HEIGHT).into_any_element(),
                    )),
            )
        }
    }

    /// Jacob, 2026-10-23 (Discord-style reference): inside a group / channel
    /// bubble the order is name → reply quote → text, 2 pt from the name to
    /// what follows, 6 from the quote. No time inside the bubble (Jacob,
    /// 2026-10-24: timestamps are hover-only).
    #[gpui::test]
    fn a_named_bubble_stacks_name_quote_text_without_a_time(cx: &mut TestAppContext) {
        let (_view, vcx) = cx.add_window_view(|_, _| NamedBubble);
        vcx.run_until_parked();
        let name = vcx.debug_bounds("bubble-name").expect("name drawn");
        let quote = vcx.debug_bounds("bubble-quote").expect("quote drawn");
        let text = vcx.debug_bounds("bubble-text").expect("text drawn");
        assert!(
            vcx.debug_bounds("bubble-time").is_none(),
            "a bubble never carries its time"
        );
        let bottom = |b: gpui::Bounds<gpui::Pixels>| f32::from(b.origin.y + b.size.height);
        let top = |b: gpui::Bounds<gpui::Pixels>| f32::from(b.origin.y);
        assert_eq!(
            bottom(name),
            top(quote),
            "name, then the quote (2 pt inside the name box)"
        );
        assert_eq!(
            bottom(name) - top(name),
            CHAT_LINE_HEIGHT + NAME_TO_TEXT_GAP,
            "name box = one line + the 2 pt gap"
        );
        assert_eq!(bottom(quote), top(text), "quote, then the text");
        assert_eq!(
            bottom(quote) - top(quote),
            32.0 + QUOTE_TO_TEXT_GAP,
            "quote box = its height + the 6 pt gap"
        );
    }

    /// A quoted agent bubble as `markdown_bubble` builds it: `sender_bubble_column`
    /// (name → quote → body) in the slot, with and without the avatar lane.
    struct QuotedBubble {
        lane: bool,
        selection: Entity<TextSelection>,
    }

    const LONG_BODY: &str = "Here, Arbos. Good to hear the avatar alignment is fixed. Nothing is \
        running on my side; the ten reports are in. What do you need?";

    impl gpui::Render for QuotedBubble {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::dark(crate::ui::theme::Surface::Matte);
            let selectable = Selectable::new(&self.selection, theme.accent_soft, cx);
            let bubble_sel = selectable.bubble("q#text".to_string(), SharedString::from(LONG_BODY));
            let quote = div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .min_w_0()
                .child(div().size(px(12.0)))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .debug_selector(|| "test-quote".into())
                        .child("“@doug”"),
                );
            let bubble = div()
                .debug_selector(|| "test-bubble".into())
                .max_w_full()
                .min_w_0()
                .relative()
                .px(px(BUBBLE_PAD_X))
                .py(px(BUBBLE_PAD_Y))
                .child(sender_bubble_column(
                    None,
                    Some(quote.into_any_element()),
                    render_block(
                        "q",
                        &Block::Paragraph(LONG_BODY.to_string()),
                        0,
                        None,
                        Instant::now(),
                        &theme,
                        &bubble_sel,
                        &CodeCopy::default(),
                    ),
                ));
            let row = div()
                .w_full()
                .flex()
                .items_end()
                .when(self.lane, |el| {
                    el.pl(px(AVATAR_COLUMN_INSET))
                        .gap(px(SENDER_DISC_GAP))
                        .child(div().size(px(SENDER_DISC)).flex_shrink_0())
                })
                .child(incoming_slot(bubble, self.lane))
                .into_any_element();
            div()
                .w(px(REFERENCE_PANE))
                .h(px(600.0))
                .text_size(px(CHAT_BODY))
                .line_height(px(CHAT_LINE_HEIGHT))
                .child(column_row(row))
        }
    }

    /// Jacob, 2026-10-23 17:09 (v0.6.175, "Doug" in the Arbonites group): an
    /// incoming bubble with a one-word reply quote collapsed to the quote's
    /// width and wrapped its body one word per line. The bubble's width is the
    /// body's (capped by the column), never the quote header's.
    ///
    /// 2026-10-23 20:55 (v0.6.185): after sender_bubble_column dropped
    /// `min_w_0` on the text wrapper, a long quoted body ran past the bubble's
    /// right edge instead of wrapping — assert the text stays inside.
    #[gpui::test]
    fn a_quoted_bubble_is_as_wide_as_its_body_not_its_quote(cx: &mut TestAppContext) {
        for lane in [false, true] {
            let (_view, vcx) = cx.add_window_view(|_, cx| QuotedBubble {
                lane,
                selection: cx.new(TextSelection::new),
            });
            vcx.run_until_parked();
            let bubble = vcx.debug_bounds("test-bubble").expect("bubble drawn");
            let quote = vcx.debug_bounds("test-quote").expect("quote drawn");
            let text = vcx.debug_bounds("bubble-text").expect("body drawn");
            let slot = vcx.debug_bounds("chat-bubble-slot").expect("slot drawn");
            let bw: f32 = bubble.size.width.into();
            let qw: f32 = quote.size.width.into();
            let tw: f32 = text.size.width.into();
            let tx: f32 = text.origin.x.into();
            let bx: f32 = bubble.origin.x.into();
            let cap: f32 = if lane {
                people_cap(slot.size.width.into())
            } else {
                assistant_cap(slot.size.width.into())
            };
            assert!(
                qw < 120.0 && bw > qw + 200.0,
                "lane={lane}: bubble {bw} pt is as narrow as its quote ({qw} pt)"
            );
            assert!(
                (bw - cap).abs() <= 1.0,
                "lane={lane}: a long body fills the cap: bubble {bw}, cap {cap}"
            );
            let bh: f32 = bubble.size.height.into();
            assert!(
                bh < 6.0 * CHAT_LINE_HEIGHT,
                "lane={lane}: the body wraps to a few lines, not one word per line ({bh} pt tall)"
            );
            let text_right = tx + tw;
            let bubble_inner_right = bx + bw - BUBBLE_PAD_X;
            assert!(
                text_right <= bubble_inner_right + 1.0,
                "lane={lane}: body runs past the bubble (text right {text_right}, inner {bubble_inner_right})"
            );
            assert!(
                bh > 2.0 * CHAT_LINE_HEIGHT,
                "lane={lane}: a long body must wrap to more than one line ({bh} pt tall)"
            );
        }
    }

    /// Which bubble a [`BubbleRow`] draws: my own (right, `RowKind::User`
    /// without `from`), another person's in a group (left, with the name,
    /// `RowKind::User` with `from`), or an agent's (`markdown_bubble` with a
    /// lane). The time is hover-only, so it is drawn at `hover` opacity beside
    /// the bubble through the real [`hover_time`], never inside it.
    #[derive(Clone, Copy, Debug)]
    enum Who {
        Mine,
        Other,
        Agent,
    }

    /// One transcript row built from the same pieces the real rows use
    /// (`sender_bubble_column`, `user_message_body` / `render_block`,
    /// `hover_time`, `assistant_slot`, `column_row`) in a pane of `pane` pt.
    struct BubbleRow {
        pane: f32,
        who: Who,
        text: &'static str,
        /// The row's hover opacity this frame (0 = at rest, 1 = hovered).
        hover: f32,
        selection: Entity<TextSelection>,
    }

    /// Longer than any bubble cap at a 1200 pt window (≈ 1,350 pt unwrapped).
    const OVERFLOWING_LINE: &str = "When you think about it, most people are going to be \
        blocked by how many tokens they can access pretty soon, and the ones who are not \
        blocked will be the ones who already own the pipes, the chips and the power lines \
        that the rest of us have to rent by the hour.";
    const SHORT_LINE: &str = "Most people cant do what I am doing right now";

    impl gpui::Render for BubbleRow {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::dark(crate::ui::theme::Surface::Matte);
            let selectable = Selectable::new(&self.selection, theme.accent_soft, cx);
            let time = || hover_time(Some("1:52 PM".to_string()), self.hover, &theme);
            let name = |name: &'static str| {
                div()
                    .text_size(px(CHAT_BODY))
                    .line_height(px(CHAT_LINE_HEIGHT))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(name)
                    .into_any_element()
            };
            let row = match self.who {
                Who::Agent => {
                    let bubble_sel =
                        selectable.bubble("a#text".to_string(), SharedString::from(self.text));
                    let bubble = div()
                        .debug_selector(|| "test-bubble".into())
                        .max_w_full()
                        .min_w_0()
                        .relative()
                        .px(px(BUBBLE_PAD_X))
                        .py(px(BUBBLE_PAD_Y))
                        .child(sender_bubble_column(
                            Some(name("Doug")),
                            None,
                            render_block(
                                "a",
                                &Block::Paragraph(self.text.to_string()),
                                0,
                                None,
                                Instant::now(),
                                &theme,
                                &bubble_sel,
                                &CodeCopy::default(),
                            ),
                        ))
                        // The rail (glyphs · time) out of the flow, as `markdown_bubble` does.
                        .child(
                            hover_rail_frame(None)
                                .child(div().size(px(16.0)))
                                .children(time().map(|t| t.ml(px(8.0)))),
                        );
                    div()
                        .w_full()
                        .flex()
                        .items_end()
                        .pl(px(AVATAR_COLUMN_INSET))
                        .gap(px(SENDER_DISC_GAP))
                        .child(div().size(px(SENDER_DISC)).flex_shrink_0())
                        .child(incoming_slot(bubble, true))
                }
                Who::Mine | Who::Other => {
                    let body = user_message_body(
                        "u",
                        self.text,
                        theme.bubble_text(),
                        &theme,
                        &selectable,
                        &CodeCopy::default(),
                    );
                    let other = matches!(self.who, Who::Other);
                    // Another person's bubble: a people chat; mine: an agent chat here.
                    let bubble = div()
                        .debug_selector(|| "test-bubble".into())
                        .max_w(relative_width(if other {
                            PEOPLE_BUBBLE_FRACTION
                        } else {
                            own_bubble_fraction(false)
                        }))
                        .px(px(BUBBLE_PAD_X))
                        .py(px(BUBBLE_PAD_Y))
                        .child(sender_bubble_column(
                            other.then(|| name("Samir Haddad")),
                            None,
                            body,
                        ));
                    if other {
                        div()
                            .w_full()
                            .flex()
                            .items_end()
                            .pl(px(AVATAR_COLUMN_INSET))
                            .gap(px(SENDER_DISC_GAP))
                            .child(div().size(px(SENDER_DISC)).flex_shrink_0())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .debug_selector(|| "test-bubble-parent".into())
                                    .child(bubble)
                                    .child(div().size(px(16.0)).ml(px(6.0)))
                                    .children(time().map(|t| t.ml(px(8.0)))),
                            )
                    } else {
                        div()
                            .w_full()
                            .flex()
                            .items_end()
                            .justify_end()
                            .debug_selector(|| "test-bubble-parent".into())
                            .children(time().map(|t| t.mr(px(8.0))))
                            .child(div().size(px(16.0)).mr(px(6.0)))
                            // Through the real delivery wrapper: a sent bubble must
                            // keep the same cap as a seen one (v0.6.223 regression).
                            .child(own_bubble(bubble, Delivery::Delivered))
                    }
                }
            };
            div()
                .w(px(self.pane))
                .h(px(600.0))
                .text_size(px(CHAT_BODY))
                .line_height(px(CHAT_LINE_HEIGHT))
                .child(column_row(row.into_any_element()))
        }
    }

    /// The cap a [`BubbleRow`] bubble may reach: the people share of the slot
    /// for an agent in a group (it has a lane); 72 % of the row for my own
    /// bubble in an agent chat; the people share of the lane beside the disc
    /// for another person — `relative_width` is relative to the parent.
    fn bubble_cap(vcx: &mut gpui::VisualTestContext, who: Who) -> f32 {
        match who {
            Who::Agent => {
                let slot = vcx.debug_bounds("chat-bubble-slot").expect("slot drawn");
                people_cap(slot.size.width.into())
            }
            Who::Mine | Who::Other => {
                let parent = vcx
                    .debug_bounds("test-bubble-parent")
                    .expect("bubble parent drawn");
                let share = if matches!(who, Who::Other) {
                    PEOPLE_BUBBLE_FRACTION
                } else {
                    own_bubble_fraction(false)
                };
                f32::from(parent.size.width) * share
            }
        }
    }

    /// Jacob, 2026-10-23 20:18 (Mac v0.6.184, the Arbonites group): his own
    /// bubbles ran off the right edge of the chat column, and another person's
    /// bubble was wide with the text spilling past it and the time on a line
    /// of its own. Cause: `sender_bubble_column` (v0.6.183, #1437) put the
    /// text in a flex row with the time, and a text's automatic minimum width
    /// in a row is its unwrapped width. Every bubble kind, at a 400 pt and a
    /// 1200 pt window: the bubble stays inside the column and under its cap,
    /// and the text wraps inside the bubble instead of running out of it.
    #[gpui::test]
    fn bubbles_stay_inside_the_column_and_wrap_their_text(cx: &mut TestAppContext) {
        // 400 pt: the sidebar is hidden, the pane is the window; 1200 pt:
        // the 296 pt sidebar is open.
        for pane in [400.0_f32, 1200.0 - crate::ui::theme::SIDEBAR_WIDTH] {
            for who in [Who::Mine, Who::Other, Who::Agent] {
                let (_view, vcx) = cx.add_window_view(|_, cx| BubbleRow {
                    pane,
                    who,
                    text: OVERFLOWING_LINE,
                    hover: 0.0,
                    selection: cx.new(TextSelection::new),
                });
                vcx.run_until_parked();
                let column = vcx.debug_bounds("chat-column").expect("column drawn");
                let bubble = vcx.debug_bounds("test-bubble").expect("bubble drawn");
                let text = vcx.debug_bounds("bubble-text").expect("text drawn");
                let right = |b: gpui::Bounds<Pixels>| f32::from(b.origin.x + b.size.width);
                let left = |b: gpui::Bounds<Pixels>| f32::from(b.origin.x);
                let cap = bubble_cap(vcx, who);
                let bw = f32::from(bubble.size.width);
                assert!(
                    right(bubble) <= right(column) + 0.5 && left(bubble) >= left(column) - 0.5,
                    "pane {pane} {who:?}: bubble {}..{} runs out of the column {}..{}",
                    left(bubble),
                    right(bubble),
                    left(column),
                    right(column)
                );
                assert!(
                    bw <= cap + 1.0,
                    "pane {pane} {who:?}: bubble {bw} pt is wider than its cap {cap}"
                );
                assert!(
                    right(text) <= right(bubble) - BUBBLE_PAD_X + 1.0,
                    "pane {pane} {who:?}: the text box ends at {} but the bubble's inner edge is {}",
                    right(text),
                    right(bubble) - BUBBLE_PAD_X
                );
                // A 1,350 pt line never fits a bubble this size: it must have wrapped.
                let th = f32::from(text.size.height);
                assert!(
                    th >= 2.0 * CHAT_LINE_HEIGHT,
                    "pane {pane} {who:?}: the text is one {th} pt line — it did not wrap inside the bubble"
                );
                // … and a long bubble fills its cap (the text is what sizes it).
                assert!(
                    (bw - cap).abs() <= 1.0,
                    "pane {pane} {who:?}: a long line fills the cap: bubble {bw}, cap {cap}"
                );
            }
        }
    }

    /// A one-line bubble stays content-sized: no slack after a short text
    /// (the row's max-content width is the text, never the cap).
    #[gpui::test]
    fn a_short_bubble_is_as_wide_as_its_text(cx: &mut TestAppContext) {
        for who in [Who::Mine, Who::Other, Who::Agent] {
            let (_view, vcx) = cx.add_window_view(|_, cx| BubbleRow {
                pane: REFERENCE_PANE,
                who,
                text: SHORT_LINE,
                hover: 0.0,
                selection: cx.new(TextSelection::new),
            });
            vcx.run_until_parked();
            let bubble = vcx.debug_bounds("test-bubble").expect("bubble drawn");
            let text = vcx.debug_bounds("bubble-text").expect("text drawn");
            let bw = f32::from(bubble.size.width);
            let tw = f32::from(text.size.width);
            let th = f32::from(text.size.height);
            let cap = bubble_cap(vcx, who);
            assert!(
                th <= CHAT_LINE_HEIGHT + 0.5,
                "{who:?}: a short line stays on one line ({th} pt tall)"
            );
            assert!(
                bw < cap - 60.0,
                "{who:?}: short bubble {bw} pt is at its cap ({cap})"
            );
            // The bubble is the text and the pads; the time is never inside it.
            assert!(
                (bw - (tw + 2.0 * BUBBLE_PAD_X)).abs() <= 2.0,
                "{who:?}: bubble {bw} ≠ text {tw} + pads"
            );
        }
    }

    /// Jacob, 2026-10-24 ("remove the always-on message timer. Only have that
    /// on hover"): at rest no bubble — mine, another person's, an agent's —
    /// draws a time anywhere; while the row is hovered the time appears beside
    /// the bubble, outside it, on the bubble's line. Reverts v0.6.203
    /// (#1496, always-visible footer time) and v0.6.183's group inline time
    /// (#1437) on the Mac. Rule: docs/mac-design-rules.md.
    #[gpui::test]
    fn the_time_is_hidden_at_rest_and_beside_the_bubble_on_hover(cx: &mut TestAppContext) {
        for who in [Who::Mine, Who::Other, Who::Agent] {
            let (_view, vcx) = cx.add_window_view(|_, cx| BubbleRow {
                pane: REFERENCE_PANE,
                who,
                text: SHORT_LINE,
                hover: 0.0,
                selection: cx.new(TextSelection::new),
            });
            vcx.run_until_parked();
            vcx.debug_bounds("test-bubble").expect("bubble drawn");
            assert!(
                vcx.debug_bounds("bubble-time").is_none(),
                "{who:?}: a time is drawn at rest"
            );

            let (_view, vcx) = cx.add_window_view(|_, cx| BubbleRow {
                pane: REFERENCE_PANE,
                who,
                text: SHORT_LINE,
                hover: 1.0,
                selection: cx.new(TextSelection::new),
            });
            vcx.run_until_parked();
            let bubble = vcx.debug_bounds("test-bubble").expect("bubble drawn");
            let time = vcx
                .debug_bounds("bubble-time")
                .unwrap_or_else(|| panic!("{who:?}: no time on hover"));
            let left = |b: gpui::Bounds<Pixels>| f32::from(b.origin.x);
            let right = |b: gpui::Bounds<Pixels>| f32::from(b.origin.x + b.size.width);
            let outside = match who {
                // My bubble sits right: the time is on its outer (left) edge.
                Who::Mine => right(time) <= left(bubble),
                // Others' bubbles sit left: the time follows the rail on the right.
                Who::Other | Who::Agent => left(time) >= right(bubble),
            };
            assert!(
                outside,
                "{who:?}: the time {}..{} overlaps the bubble {}..{}",
                left(time),
                right(time),
                left(bubble),
                right(bubble)
            );
            assert!(
                f32::from(time.origin.y) >= f32::from(bubble.origin.y)
                    && f32::from(time.origin.y + time.size.height)
                        <= f32::from(bubble.origin.y + bubble.size.height),
                "{who:?}: the time is not on the bubble's line"
            );
        }
    }

    /// The fade is the only gate: `hover_time` yields nothing at opacity 0 and
    /// an element at any positive opacity, so a row that was never hovered
    /// (`HoverFade::default()`) draws no time at all.
    #[test]
    fn hover_time_renders_nothing_at_rest() {
        let theme = Theme::dark(crate::ui::theme::Surface::Matte);
        let fade = HoverFade::default();
        let (opacity, _) = fade.opacity("m1", Instant::now());
        assert_eq!(opacity, 0.0);
        assert!(hover_time(Some("1:52 PM".into()), opacity, &theme).is_none());
        assert!(hover_time(Some("1:52 PM".into()), 0.3, &theme).is_some());
        assert!(hover_time(None, 1.0, &theme).is_none());
    }

    #[test]
    fn the_column_is_the_composers_geometry() {
        let m = [
            COLUMN_MARGIN,
            COMPOSER_MARGIN,
            COLUMN_MAX_WIDTH,
            COMPOSER_WIDTH,
            PLUS_BUTTON,
        ];
        let [margin, composer_margin, cap, input, circle] = std::hint::black_box(m);
        assert_eq!(margin, composer_margin, "one side margin");
        assert_eq!(margin, 16.0, "Grok: 16 pt from the pane");
        assert_eq!(cap, input + 2.0 * circle, "the pill at its widest");
    }
}

#[cfg(test)]
mod transcript_smoke_tests {
    //! The real `Transcript` — the entity the shell mounts, with a real
    //! `AppState` — rendered in a headless GPUI window, with a multi-block
    //! agent reply, hovered. The guard that would have caught v0.6.224's
    //! launch crash ([feedback bubble-rail-crash-hotfix-2026-10-25]:
    //! `ListState::bounds_for_item` called from inside the list's render →
    //! "RefCell already mutably borrowed"): every other transcript test
    //! renders rows or blocks on their own, never the list.
    use futures::future::BoxFuture;
    use gpui::{point, Modifiers, TestAppContext};
    use tokio::sync::mpsc;

    use super::*;
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};

    /// A source that never speaks: the test seeds the state by hand.
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

    pub(super) struct Host {
        pub(super) transcript: Entity<Transcript>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .bg(gpui::black())
                .child(self.transcript.clone())
        }
    }

    const LONG_REPLY: &str = "Arbos, from where I sat:\n\n**What worked**\n- Authority held: three guest agents deferred to the owner's call every time.\n- The handoff lines read cleanly; nobody asked who was speaking.\n\n**What did not**\n- The group lost the thread twice when two agents answered at once.\n- One attachment never rendered on the phone.\n\nTell me which of these you want first.";

    pub(super) fn message(id: &str, role: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "chatId": "c1",
            "role": role,
            "sender": if role == "user" { "user" } else { "box:1" },
            "status": "done",
            "createdAt": at,
            "completedAt": at + 1_000,
            "parts": [{ "id": format!("{id}-t"), "kind": "text", "text": text }],
        }))
        .expect("message")
    }

    /// A transcript over a seeded state: one agent chat with a user line and
    /// a five-block reply. Returns the runtime (kept alive for the state's
    /// tokio workers), the state and the transcript; the caller opens the window.
    fn mount(
        cx: &mut TestAppContext,
    ) -> (
        tokio::runtime::Runtime,
        Entity<AppState>,
        Entity<Transcript>,
    ) {
        let t = 1_700_000_000_000u64;
        mount_with(
            cx,
            vec![
                message("m1", "user", "How did the review go?", t),
                message("m2", "assistant", LONG_REPLY, t + 2_000),
            ],
        )
    }

    /// `mount` over any message list (the long-chat tests).
    pub(super) fn mount_with(
        cx: &mut TestAppContext,
        messages: Vec<Message>,
    ) -> (
        tokio::runtime::Runtime,
        Entity<AppState>,
        Entity<Transcript>,
    ) {
        // A runtime nobody drives: the state's tokio workers (source, updater,
        // calls) are parked for good, so no other thread ever touches gpui's
        // deterministic test scheduler.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        let t = 1_700_000_000_000u64;
        state.update(cx, |s, cx| {
            s.projects.push(Project {
                id: "p1".into(),
                name: "Arbos".into(),
                kind: SpaceKind::Project,
                ..Project::default()
            });
            let chat: Chat = serde_json::from_value(serde_json::json!({
                "id": "c1", "project_id": "p1", "title": "Main", "is_pinned": true,
                "is_system": false, "unread": false, "last_message_at": t,
            }))
            .expect("chat");
            s.chats.insert("p1".into(), vec![chat]);
            s.messages.insert("c1".into(), messages);
            s.selected_project = Some("p1".into());
            s.active_tab.insert("p1".into(), "c1".into());
            cx.notify();
        });
        let transcript = cx.new(|cx| Transcript::new(state.clone(), cx));
        transcript.update(cx, |t, cx| t.sync(cx));
        (rt, state, transcript)
    }

    /// Secret hand-off (2026-10-26): the request card (`secret_offer`, a stateful card with
    /// two inputs) and the ghost row both draw headless, and the card carries the suggested
    /// name into its Name field with nothing in the Value field.
    #[gpui::test]
    fn secret_offer_card_and_ghost_row_draw(cx: &mut TestAppContext) {
        let t = 1_700_000_000_000u64;
        let card: Message = serde_json::from_value(serde_json::json!({
            "id": "m2", "chatId": "c1", "role": "assistant", "sender": "agent", "status": "done",
            "createdAt": t + 2_000,
            "parts": [
                { "id": "p1", "kind": "text", "text": "Yes. Use the box below." },
                { "id": "p2", "kind": "approval", "approvalId": "ap-offer", "title": "Give Arbos a secret",
                  "summary": "the refunds report", "risk": "credential", "state": "pending",
                  "payload": { "kind": "secret_offer", "name": "STRIPE_KEY", "purpose": "the refunds report",
                               "agent": "Arbos", "projectId": "p1", "chatId": "c1", "answers": [] } }
            ],
        }))
        .expect("card row");
        let ghost: Message = serde_json::from_value(serde_json::json!({
            "id": "m3", "chatId": "c1", "role": "user", "sender": "user", "status": "done",
            "createdAt": t + 4_000, "redactedAt": t + 5_000,
            "parts": [{ "id": "p3", "kind": "secret_redacted", "name": "STRIPE_KEY", "env": "STRIPE_KEY",
                        "secretId": "s1", "savedBy": "user:me" }],
        }))
        .expect("ghost row");
        let (_rt, _state, transcript) = mount_with(
            cx,
            vec![
                message("m1", "user", "Can I give you a secret?", t),
                card,
                ghost,
            ],
        );
        let (_view, vcx) = cx.add_window_view(|_, _| Host {
            transcript: transcript.clone(),
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("chat-column").is_some(),
            "the transcript draws"
        );
        assert!(
            vcx.debug_bounds("secret-offer-card").is_some(),
            "the request card is on screen"
        );
        assert!(
            vcx.debug_bounds("secret-offer-save").is_some(),
            "the Save to vault row is on screen"
        );
        assert!(
            vcx.debug_bounds("secret-ghost").is_some(),
            "the ghost row draws in the user's slot"
        );
        transcript.read_with(cx, |t, cx| {
            let card = t.secret_cards.get("ap-offer").expect("card entity cached");
            card.read_with(cx, |c, cx| {
                assert_eq!(c.name_input.read(cx).text(), "STRIPE_KEY");
                assert!(c.value_input.read(cx).text().is_empty());
            });
        });
    }

    /// An expanded link card under a long line (v0.6.310 proof shot, the YouTube row): the
    /// bubble is as wide as the sentence, the card stops at 400 pt, and the card's title
    /// wraps there — the card must still be tall enough for its own footer.
    #[gpui::test]
    fn expanded_link_card_under_a_long_line_keeps_its_footer(cx: &mut TestAppContext) {
        let t = 1_700_000_000_000u64;
        let mut m = message(
            "m2",
            "user",
            "this one explains the render pipeline better than the docs, worth the ten minutes https://www.youtube.com/watch?v=aqz-KE-bpKQ",
            t + 2_000,
        );
        m.parts.push(
            serde_json::from_value(serde_json::json!({
                "id": "lp", "kind": "link_preview", "specVersion": 1,
                "url": "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
                "canonicalUrl": "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
                "provider": "youtube", "providerName": "YouTube", "type": "video",
                "title": "Big Buck Bunny 60fps 4K - Official Blender Foundation Short Film",
                "description": "Blender", "facts": ["Blender"], "siteHost": "www.youtube.com",
                "image": { "ref": "lp-yt.jpg", "width": 1280, "height": 720, "mime": "image/jpeg", "art": false },
                "layout": "expanded", "fetchedAt": t
            }))
            .expect("preview part"),
        );
        let (_rt, _state, transcript) = mount_with(
            cx,
            vec![message("m1", "user", "How did the review go?", t), m],
        );
        let (_view, vcx) = cx.add_window_view(|_, _| Host {
            transcript: transcript.clone(),
        });
        vcx.run_until_parked();
        let card = vcx.debug_bounds("link-card").expect("the card draws");
        let text = vcx
            .debug_bounds("link-text")
            .expect("the card's text block draws");
        assert!(
            card.size.width <= px(crate::ui::link_card::CARD_MAX_WIDTH + 0.5),
            "the card stops at its width: {card:?}"
        );
        assert!(
            text.bottom() + px(crate::ui::link_card::CARD_PAD) <= card.bottom() + px(0.5),
            "the text block (title, line, footer) fits inside the card: text {text:?} card {card:?}"
        );
    }

    /// 2026-10-26, Mac 0.6.268: a row the code did not expect aborted the app at every
    /// launch (a panic on the main thread unwinds into AppKit → `abort()`). A sync event whose
    /// apply panics is skipped with a notice, the state keeps every other row, and the
    /// transcript still renders; the panic hook records the message to `panic.log`.
    #[gpui::test]
    fn a_panicking_sync_event_is_skipped_not_fatal(cx: &mut TestAppContext) {
        crate::panic_log::install();
        let before = std::fs::read_to_string(crate::panic_log::log_path()).unwrap_or_default();
        let (_rt, state, transcript) = mount(cx);
        // (The hook also prints the poison to stderr; that line is expected in this test.)
        state.update(cx, |s, cx| {
            s.apply(SyncEvent::Notice("panic!".into()), cx);
        });
        state.read_with(cx, |s, _| {
            assert_eq!(s.messages.get("c1").map(Vec::len), Some(2), "rows kept");
            assert!(
                s.notices.iter().any(|n| n.text.contains("skipped")),
                "the user is told one update was skipped: {:?}",
                s.notices.iter().map(|n| n.text.clone()).collect::<Vec<_>>()
            );
        });
        transcript.update(cx, |t, cx| t.sync(cx));
        let (_view, vcx) = cx.add_window_view(|_, _| Host {
            transcript: transcript.clone(),
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("chat-column").is_some(),
            "the transcript still draws"
        );
        let after = std::fs::read_to_string(crate::panic_log::log_path()).unwrap_or_default();
        assert!(
            after.len() > before.len() && after[before.len()..].contains("test poison event"),
            "panic.log holds the message"
        );
    }

    /// The list renders the reply as block rows (no panic, every block drawn),
    /// and hovering the middle of the bubble shows the rail at the bubble's
    /// vertical centre — the whole of #1674 on the real transcript.
    #[gpui::test]
    fn a_multi_block_reply_renders_and_its_rail_sits_at_the_bubbles_centre(
        cx: &mut TestAppContext,
    ) {
        let (_rt, _state, transcript) = mount(cx);
        let (_host, vcx) = cx.add_window_view(|_, _| Host {
            transcript: transcript.clone(),
        });
        vcx.run_until_parked();
        let rows: Vec<(String, bool)> = transcript.read_with(&*vcx, |t, _| {
            t.rows
                .iter()
                .filter(|r| r.message_id == "m2")
                .map(|r| (r.id.clone(), matches!(r.kind, RowKind::Markdown { .. })))
                .collect()
        });
        let block_rows: Vec<String> = rows
            .iter()
            .filter(|(_, md)| *md)
            .map(|(id, _)| id.clone())
            .collect();
        assert_eq!(block_rows.len(), 6, "six markdown blocks: {rows:?}");
        let bubble = vcx
            .debug_bounds("chat-bubble-assistant")
            .expect("the reply's bubble is drawn");
        // Every block row was measured by the frame.
        let measured: Vec<Option<Bounds<Pixels>>> = transcript.read_with(&*vcx, |t, _| {
            let m = t.measured.borrow();
            block_rows
                .iter()
                .map(|id| m.rows.get(id).copied())
                .collect()
        });
        assert!(measured.iter().all(Option::is_some), "{measured:?}");
        let top = measured.iter().flatten().map(|b| b.top()).min().unwrap();
        let bottom = measured.iter().flatten().map(|b| b.bottom()).max().unwrap();
        assert!(
            bottom - top > px(150.0),
            "the bubble is tall: {top:?}..{bottom:?}"
        );
        // Hover the middle block: the rail shows, centred on the whole bubble.
        let middle = point(bubble.left() + px(40.0), (top + bottom) / 2.0);
        vcx.simulate_mouse_move(middle, None, Modifiers::default());
        vcx.run_until_parked();
        for _ in 0..3 {
            transcript.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
        let rail = vcx.debug_bounds("bubble-rail").expect("rail frame drawn");
        let rail_centre = f32::from(rail.top() + rail.bottom()) / 2.0;
        let bubble_centre = f32::from(top + bottom) / 2.0;
        assert!(
            (rail_centre - bubble_centre).abs() <= RAIL_BUTTON,
            "rail centre {rail_centre} vs bubble centre {bubble_centre} (rows {top:?}..{bottom:?})"
        );
        assert!(
            f32::from(rail.left()) >= f32::from(bubble.right()) - 1.0,
            "the rail hangs off the bubble's right edge"
        );
        let hovered = transcript.read_with(&*vcx, |t, _| t.hover.over.clone());
        assert!(
            hovered.as_ref().is_some_and(|(k, _)| k == "m2#bubble"),
            "the bubble is the hovered key: {hovered:?}"
        );
    }

    /// Jacob, 2026-10-25 ([feedback jump-to-latest-instant-mac-2026-10-25]):
    /// from the top of a 1 500-row chat, ↓ lands on the newest page in a
    /// frame, and the history in between is never laid out — before, the
    /// stick spring crawled through every row (11.8 s on 1 682 rows).
    #[gpui::test]
    fn jump_to_latest_from_deep_history_lays_out_only_the_newest_page(cx: &mut TestAppContext) {
        const ROWS: usize = 1_500;
        let t = 1_700_000_000_000u64;
        let messages: Vec<Message> = (0..ROWS)
            .map(|i| {
                let role = if i % 3 == 0 { "user" } else { "assistant" };
                let text =
                    format!("synthetic history line {i} — lorem ipsum dolor sit amet, consectetur");
                message(&format!("m{i}"), role, &text, t + i as u64 * 1_000)
            })
            .collect();
        let (_rt, _state, transcript) = mount_with(cx, messages);
        let (_host, vcx) = cx.add_window_view(|_, _| Host {
            transcript: transcript.clone(),
        });
        vcx.run_until_parked();
        // `measured` keeps every row it ever saw: forget them before a frame so
        // `painted` speaks for that frame alone.
        let frame = |vcx: &mut gpui::VisualTestContext| {
            transcript.update(vcx, |t, cx| {
                t.measured.borrow_mut().rows.clear();
                cx.notify();
            });
            vcx.run_until_parked();
        };
        // Whether any row of the message with this id was painted last frame.
        let painted = |vcx: &gpui::VisualTestContext, message_id: &str| {
            transcript.read_with(vcx, |t, _| {
                let m = t.measured.borrow();
                t.rows
                    .iter()
                    .any(|r| r.message_id == message_id && m.rows.contains_key(&r.id))
            })
        };
        // Opened at the bottom: the newest row is on screen and the open laid
        // out one page, not the chat (the open shares the jump's landing).
        assert!(
            painted(vcx, &format!("m{}", ROWS - 1)),
            "opens on the newest row"
        );
        let opened = transcript.read_with(&*vcx, |t, _| t.rows_laid_out);
        assert!(
            opened < 120,
            "opening a {ROWS}-row chat laid out {opened} rows"
        );

        // Up to the very top, the way a search hit or scrollbar drag lands
        // there: the rows between are unmeasured, as in a real long chat.
        transcript.update(vcx, |t, _| {
            t.pinned_to_bottom = false;
            t.list.scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: px(0.0),
            });
        });
        frame(vcx);
        assert!(painted(vcx, "m0"), "at the top");
        assert!(!painted(vcx, &format!("m{}", ROWS - 1)));
        let middle = transcript.read_with(&*vcx, |t, _| t.list.bounds_for_item(ROWS / 2));
        assert!(
            middle.is_none(),
            "the middle of the chat is not laid out yet"
        );
        let before_jump = transcript.read_with(&*vcx, |t, _| t.rows_laid_out);

        // ↓ — one frame later the newest row is on screen.
        transcript.update(vcx, |t, cx| t.jump_to_latest(cx));
        frame(vcx);
        assert!(
            painted(vcx, &format!("m{}", ROWS - 1)),
            "the newest row is on screen after one frame"
        );
        assert!(!painted(vcx, "m0"));
        let landed = transcript.read_with(&*vcx, |t, _| t.rows_laid_out) - before_jump;
        assert!(
            landed < 120,
            "the jump laid out {landed} rows, not one page"
        );
        // The landing cue runs its course without the position moving, and
        // the history between was never laid out — no page-by-page crawl.
        for _ in 0..12 {
            std::thread::sleep(Duration::from_millis(20));
            frame(vcx);
            assert!(
                painted(vcx, &format!("m{}", ROWS - 1)),
                "stays on the newest row"
            );
        }
        let settled = transcript.read_with(&*vcx, |t, _| {
            (
                t.arrived_at.is_none(),
                t.pinned_to_bottom,
                t.list.bounds_for_item(ROWS / 2).is_none(),
                t.list.bounds_for_item(ROWS / 4).is_none(),
                t.rows_laid_out - before_jump,
            )
        });
        assert_eq!(
            (settled.0, settled.1, settled.2, settled.3),
            (true, true, true, true),
            "cue over, pinned, middle rows still unmeasured: {settled:?}"
        );
        assert!(
            settled.4 < ROWS as u64 / 4,
            "the whole jump laid out {} row frames, a crawl would be ≥ {ROWS}",
            settled.4
        );
    }

    /// Close to the end with every row between already laid out, ↓ still
    /// glides on the spring (no snap, no cue) — the short hop stays smooth.
    #[gpui::test]
    fn jump_to_latest_from_nearby_glides_instead_of_snapping(cx: &mut TestAppContext) {
        let t = 1_700_000_000_000u64;
        let messages: Vec<Message> = (0..12)
            .map(|i| {
                message(
                    &format!("m{i}"),
                    "assistant",
                    &format!("line {i}"),
                    t + i as u64 * 1_000,
                )
            })
            .collect();
        let (_rt, _state, transcript) = mount_with(cx, messages);
        let (_host, vcx) = cx.add_window_view(|_, _| Host {
            transcript: transcript.clone(),
        });
        vcx.run_until_parked();
        // Every row is on screen (12 short lines fit a test window), so the end is near and measured.
        transcript.update(vcx, |t, cx| {
            t.pinned_to_bottom = false;
            t.list.scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: px(0.0),
            });
            cx.notify();
        });
        vcx.run_until_parked();
        transcript.update(vcx, |t, cx| t.jump_to_latest(cx));
        let (pinned, cue) =
            transcript.read_with(&*vcx, |t, _| (t.pinned_to_bottom, t.arrived_at.is_some()));
        assert!(pinned);
        assert!(
            !cue,
            "a nearby jump glides; the landing cue is for the long way"
        );
    }

    /// Grouped messages ([feedback grouped-messages-mac-2026-10-28]), drawn by the real
    /// transcript in a people group: Bob's second line 30 s later has no disc and no name
    /// but keeps the lane (its bubble's left edge is the first bubble's), and sits in the
    /// 6 pt in-run gap; Sam's line opens the 20 pt turn gap with a disc and a name.
    #[gpui::test]
    fn a_run_shows_one_disc_and_opens_no_turn_gap(cx: &mut TestAppContext) {
        let t = 1_700_000_000_000u64;
        let person = |id: &str, who: &str, text: &str, at: u64| -> Message {
            serde_json::from_value(serde_json::json!({
                "id": id, "chatId": "c1", "role": "user", "sender": format!("user:gh_{who}"),
                "senderHandle": who, "senderName": who, "status": "done",
                "createdAt": at, "completedAt": at,
                "parts": [{ "id": format!("{id}-t"), "kind": "text", "text": text }],
            }))
            .expect("message")
        };
        let (_rt, state, transcript) = mount_with(
            cx,
            vec![
                person("m1", "bob", "hi all", t),
                person("m2", "bob", "two", t + 30_000),
                person("m3", "sam", "hey", t + 60_000),
            ],
        );
        // A people group: the chat names its senders and draws their discs.
        state.update(cx, |s, cx| {
            s.projects[0].kind = SpaceKind::Group;
            cx.notify();
        });
        transcript.update(cx, |t, cx| t.sync(cx));
        let (_view, vcx) = cx.add_window_view(|_, _| Host {
            transcript: transcript.clone(),
        });
        vcx.run_until_parked();
        let disc1 = vcx
            .debug_bounds("disc#m1#user")
            .expect("Bob's first line has his disc");
        assert!(
            vcx.debug_bounds("disc#m2#user").is_none(),
            "Bob's second line, inside the run, draws no disc"
        );
        let disc3 = vcx
            .debug_bounds("disc#m3#user")
            .expect("Sam starts a run: disc");
        assert_eq!(
            disc1.origin.x, disc3.origin.x,
            "discs share the avatar column"
        );
        let mut bubble = |id: &str| {
            let sel: &'static str = Box::leak(format!("person-bubble#{id}#user").into_boxed_str());
            vcx.debug_bounds(sel)
                .unwrap_or_else(|| panic!("bubble {id} drawn"))
        };
        let b1 = bubble("m1");
        let b2 = bubble("m2");
        let b3 = bubble("m3");
        assert_eq!(b1.origin.x, b2.origin.x, "the continuation keeps the lane");
        assert_eq!(b2.origin.x, b3.origin.x);
        let gap = |a: Bounds<Pixels>, b: Bounds<Pixels>| {
            f32::from(b.origin.y - (a.origin.y + a.size.height))
        };
        assert_eq!(gap(b1, b2), 2.0 * ROW_GAP_HALF, "in-run gap: 6 pt");
        assert_eq!(
            gap(b2, b3),
            2.0 * ROW_GAP_HALF + TURN_GAP_EXTRA,
            "a new run opens the 20 pt turn gap"
        );
        // One name line taller: the run's first bubble names Bob, the second does not.
        assert_eq!(
            f32::from(b1.size.height - b2.size.height),
            CHAT_LINE_HEIGHT + NAME_TO_TEXT_GAP,
            "the continuation carries no name"
        );
        assert_eq!(
            b3.size.height, b1.size.height,
            "Sam's first bubble is named like Bob's"
        );
    }
}

#[cfg(test)]
mod reaction_chip_tests {
    //! Hover on a reaction chip, on the hover rail's smiley and on a capsule
    //! cell changes paint, never geometry: the same box at rest, under the
    //! pointer and after it left ([feedback reaction-pill-hover-jitter-2026-10-28];
    //! Scout, 2026-10-28: "the emoji reaction pill still twitches on hover").
    use gpui::{point, Bounds, Modifiers, Pixels, TestAppContext};

    use super::transcript_smoke_tests::{message, mount_with, Host};
    use super::*;

    fn same_box(rest: Bounds<Pixels>, now: Bounds<Pixels>, what: &str) {
        let d = |a: Pixels, b: Pixels| (f32::from(a) - f32::from(b)).abs();
        assert!(
            d(rest.origin.x, now.origin.x) < 0.01
                && d(rest.origin.y, now.origin.y) < 0.01
                && d(rest.size.width, now.size.width) < 0.01
                && d(rest.size.height, now.size.height) < 0.01,
            "{what} moved: at rest {rest:?}, now {now:?}"
        );
    }

    fn away() -> Point<Pixels> {
        point(px(2.0), px(2.0))
    }

    /// A user line with Sam's 👍 and an agent reply with a ❤️ held by two
    /// people, me among them (the count drawn, the accent fill on).
    fn seed(cx: &mut TestAppContext) -> (tokio::runtime::Runtime, Entity<Transcript>) {
        let t = 1_700_000_000_000u64;
        let mut mine = message(
            "m1",
            "user",
            "Here now. Paste what you have and I'll read it.",
            t,
        );
        mine.reactions
            .entry("👍".into())
            .or_default()
            .insert("user:sam".into(), true);
        let mut reply = message("m2", "assistant", "Sending in a sec.", t + 2_000);
        let (rt, state, transcript) = mount_with(cx, vec![mine, reply.clone()]);
        let me = state.read_with(cx, |s, _| format!("user:{}", s.me_id()));
        let held = reply.reactions.entry("❤️".into()).or_default();
        held.insert(me, true);
        held.insert("user:sam".into(), true);
        state.update(cx, |s, cx| {
            let rows = s.messages.get_mut("c1").expect("chat");
            rows.pop();
            rows.push(reply);
            cx.notify();
        });
        transcript.update(cx, |t, cx| t.sync(cx));
        (rt, transcript)
    }

    #[gpui::test]
    fn a_reaction_chip_keeps_its_box_under_the_pointer(cx: &mut TestAppContext) {
        let (_rt, transcript) = seed(cx);
        let (_view, vcx) = cx.add_window_view(|_, _| Host { transcript });
        vcx.run_until_parked();
        for sel in ["reaction-chip#👍", "reaction-chip#❤️"] {
            vcx.simulate_mouse_move(away(), None, Modifiers::default());
            vcx.run_until_parked();
            let rest = vcx
                .debug_bounds(sel)
                .unwrap_or_else(|| panic!("{sel} drawn at rest"));
            let row = vcx
                .debug_bounds("reaction-row")
                .expect("the chips row draws");
            assert!(
                (f32::from(rest.size.height) - REACTION_CHIP_HEIGHT).abs() < 0.01,
                "{sel}: the chip is {:?} tall, not the fixed {REACTION_CHIP_HEIGHT}",
                rest.size.height
            );
            vcx.simulate_mouse_move(rest.center(), None, Modifiers::default());
            vcx.run_until_parked();
            same_box(
                rest,
                vcx.debug_bounds(sel).expect("chip under the pointer"),
                sel,
            );
            same_box(
                row,
                vcx.debug_bounds("reaction-row")
                    .expect("row under the pointer"),
                "the chips row",
            );
            vcx.simulate_mouse_move(away(), None, Modifiers::default());
            vcx.run_until_parked();
            same_box(
                rest,
                vcx.debug_bounds(sel).expect("chip after the pointer left"),
                sel,
            );
        }
    }

    struct SizedHost {
        transcript: Entity<Transcript>,
        width: f32,
    }

    impl Render for SizedHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(self.width))
                .h(px(900.0))
                .bg(gpui::black())
                .child(self.transcript.clone())
        }
    }

    /// Chips sit right under their own bubble, on its leading edge, in a people group:
    /// the first line of Bob's run (disc), its grouped continuation (empty lane — Scout's
    /// "let me add him lol" 🥹 drew at the column's far left on v0.6.350), my own line
    /// (right edge) and an agent in the group (its lane), at a narrow and a wide window
    /// ([feedback reaction-chip-anchor-mac-2026-10-29]).
    #[gpui::test]
    fn chips_sit_under_their_own_bubble_on_its_leading_edge(cx: &mut TestAppContext) {
        let t = 1_700_000_000_000u64;
        let msg = |id: &str, role: &str, sender: &str, handle: &str, text: &str, at: u64| {
            let mut m: Message = serde_json::from_value(serde_json::json!({
                "id": id, "chatId": "c1", "role": role, "sender": sender,
                "senderHandle": handle, "senderName": handle, "status": "done",
                "createdAt": at, "completedAt": at,
                "parts": [{ "id": format!("{id}-t"), "kind": "text", "text": text }],
            }))
            .expect("message");
            m.reactions
                .entry("🥹".into())
                .or_default()
                .insert("user:gh_sam".into(), true);
            m
        };
        let (_rt, state, transcript) = mount_with(cx, Vec::new());
        let me = state.read_with(cx, |s, _| format!("user:{}", s.me_id()));
        state.update(cx, |s, cx| {
            s.projects[0].kind = SpaceKind::Group;
            let rows = s.messages.entry("c1".into()).or_default();
            rows.clear();
            rows.extend([
                msg(
                    "m1",
                    "user",
                    "user:gh_bob",
                    "bob",
                    "hes already on the app",
                    t,
                ),
                msg(
                    "m2",
                    "user",
                    "user:gh_bob",
                    "bob",
                    "let me add him lol",
                    t + 30_000,
                ),
                msg("m3", "user", &me, "me", "nice", t + 60_000),
                msg(
                    "m4",
                    "assistant",
                    "agent:a1",
                    "helper",
                    "Added.",
                    t + 90_000,
                ),
            ]);
            cx.notify();
        });
        transcript.update(cx, |t, cx| t.sync(cx));
        for width in [520.0_f32, 1400.0] {
            let (_view, vcx) = cx.add_window_view(|_, _| SizedHost {
                transcript: transcript.clone(),
                width,
            });
            vcx.run_until_parked();
            let mut bounds = |sel: String| {
                let sel: &'static str = Box::leak(sel.into_boxed_str());
                vcx.debug_bounds(sel)
                    .unwrap_or_else(|| panic!("{width} pt: {sel} drawn"))
            };
            let column = bounds("chat-column".into());
            assert!(
                f32::from(column.size.width) <= width,
                "{width} pt: the column follows the window"
            );
            let bubbles = [
                ("m1", bounds("person-bubble#m1#user".into()), false),
                ("m2", bounds("person-bubble#m2#user".into()), false),
                ("m3", bounds("own-bubble#m3#user".into()), true),
                ("m4", bounds("chat-bubble-assistant".into()), false),
            ];
            for (id, bubble, mine) in bubbles {
                let chips = bounds(format!("reactions#{id}"));
                let below = f32::from(chips.origin.y - (bubble.origin.y + bubble.size.height));
                assert!(
                    (0.0..=8.0).contains(&below),
                    "{width} pt {id}: chips start {below} pt below their bubble"
                );
                if mine {
                    let right = |b: Bounds<Pixels>| f32::from(b.origin.x + b.size.width);
                    assert!(
                        (right(chips) - right(bubble)).abs() < 0.5,
                        "{width} pt {id}: my chips end at {} but my bubble at {}",
                        right(chips),
                        right(bubble)
                    );
                } else {
                    assert!(
                        (f32::from(chips.origin.x) - f32::from(bubble.origin.x)).abs() < 0.5,
                        "{width} pt {id}: chips start at x {:?} but the bubble at x {:?}",
                        chips.origin.x,
                        bubble.origin.x
                    );
                }
            }
        }
    }

    /// The count and the emoji come from different fonts; neither sizes the chip.
    #[gpui::test]
    fn every_chip_is_the_same_height(cx: &mut TestAppContext) {
        let (_rt, transcript) = seed(cx);
        let (_view, vcx) = cx.add_window_view(|_, _| Host { transcript });
        vcx.run_until_parked();
        let one = vcx.debug_bounds("reaction-chip#👍").expect("👍 drawn");
        let two = vcx.debug_bounds("reaction-chip#❤️").expect("❤️ drawn");
        assert!(
            (f32::from(one.size.height) - f32::from(two.size.height)).abs() < 0.01,
            "a chip with a count ({:?}) is not as tall as one without ({:?})",
            two.size.height,
            one.size.height
        );
        assert!(
            two.size.width > one.size.width,
            "the count widens the chip, nothing else changes"
        );
    }

    /// The rail's smiley beside the agent reply: a fixed 24 pt box whether the
    /// pointer is on the bubble or on the button, and the bubble beside it
    /// does not move either.
    #[gpui::test]
    fn the_rail_smiley_keeps_its_box_under_the_pointer(cx: &mut TestAppContext) {
        let (_rt, transcript) = seed(cx);
        let (_view, vcx) = cx.add_window_view(|_, _| Host { transcript });
        vcx.run_until_parked();
        let bubble = vcx
            .debug_bounds("chat-bubble-assistant")
            .expect("the agent bubble draws");
        vcx.simulate_mouse_move(bubble.center(), None, Modifiers::default());
        vcx.run_until_parked();
        let smiley = vcx
            .debug_bounds("rail-button#face.smiling")
            .expect("the rail's smiley draws beside the hovered bubble");
        assert!(
            (f32::from(smiley.size.width) - RAIL_BUTTON).abs() < 0.01
                && (f32::from(smiley.size.height) - RAIL_BUTTON).abs() < 0.01,
            "the smiley is {:?}, not {RAIL_BUTTON} square",
            smiley.size
        );
        vcx.simulate_mouse_move(smiley.center(), None, Modifiers::default());
        vcx.run_until_parked();
        same_box(
            smiley,
            vcx.debug_bounds("rail-button#face.smiling")
                .expect("smiley under the pointer"),
            "the rail's smiley",
        );
        same_box(
            bubble,
            vcx.debug_bounds("chat-bubble-assistant")
                .expect("bubble while its rail is hovered"),
            "the bubble beside the hovered rail",
        );
    }

    struct Cells;

    impl Render for Cells {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .child(crate::ui::reactions::cell(
                    "react-👍",
                    div().text_size(px(22.0)).child("👍"),
                    true,
                    &theme,
                    |_, _, _| {},
                ))
                .child(crate::ui::reactions::cell(
                    "react-more",
                    div().size(px(20.0)),
                    false,
                    &theme,
                    |_, _, _| {},
                ))
        }
    }

    /// The capsule's cells — a held emoji and the add-emoji ☺+ — are fixed boxes too.
    #[gpui::test]
    fn capsule_cells_keep_their_box_under_the_pointer(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte)));
        let (_view, vcx) = cx.add_window_view(|_, _| Cells);
        vcx.run_until_parked();
        for sel in ["reaction-cell#react-👍", "reaction-cell#react-more"] {
            let rest = vcx
                .debug_bounds(sel)
                .unwrap_or_else(|| panic!("{sel} drawn"));
            vcx.simulate_mouse_move(rest.center(), None, Modifiers::default());
            vcx.run_until_parked();
            same_box(
                rest,
                vcx.debug_bounds(sel).expect("cell under the pointer"),
                sel,
            );
            vcx.simulate_mouse_move(away(), None, Modifiers::default());
            vcx.run_until_parked();
            same_box(
                rest,
                vcx.debug_bounds(sel).expect("cell after the pointer left"),
                sel,
            );
        }
    }
}

#[cfg(test)]
mod cross_bubble_copy_tests {
    //! Jacob, 2026-10-24 ([feedback bubble-selection-2026-10-24]): a selection
    //! across messages copies cleanly, like Grok's — the first message from
    //! the pressed word, every message between whole, the last up to the
    //! pointer, in transcript order whichever way the drag went.
    use super::*;

    fn at(order: usize, key: &str, text: &str) -> BubbleAt {
        BubbleAt {
            order,
            key: key.to_string().into(),
            text: text.to_string().into(),
        }
    }

    fn transcript() -> Vec<(usize, String, String)> {
        vec![
            (0, "m1.u".into(), "What's the time difference?".into()),
            (
                1,
                "m2#text".into(),
                "London is 7 hours ahead of you.".into(),
            ),
            (2, "m3.u".into(), "Make me a packing list.".into()),
            (
                3,
                "m4#text".into(),
                "Here's a list:\n• Boards\n• Leash".into(),
            ),
        ]
    }

    #[test]
    fn copy_across_is_top_to_bottom_with_the_middle_whole() {
        let rows = transcript();
        let span = Span {
            start: at(0, "m1.u", "What's the time difference?"),
            start_index: 11,
            end: at(3, "m4#text", "Here's a list:\n• Boards\n• Leash"),
            end_index: 14,
        };
        assert_eq!(
            copy_across(&span, &rows).as_deref(),
            Some(
                "time difference?\n\nLondon is 7 hours ahead of you.\n\nMake me a packing list.\n\nHere's a list:"
            )
        );
    }

    /// The model orders the ends; a drag that started at the bottom gives
    /// the same span and so the same copy.
    #[test]
    fn a_bottom_up_drag_copies_the_same_text() {
        let rows = transcript();
        let mut model = crate::ui::select::Model::default();
        let (top, bottom) = (
            at(1, "m2#text", "London is 7 hours ahead of you."),
            at(3, "m4#text", "Here's a list:\n• Boards\n• Leash"),
        );
        model.begin_at(bottom.clone(), 6, 1, false);
        model.extend_in(&top, 10);
        let up = model.span().expect("span");
        model.begin_at(top, 10, 1, false);
        model.extend_in(&bottom, 6);
        let down = model.span().expect("span");
        assert_eq!(up, down);
        assert_eq!(
            copy_across(&up, &rows).as_deref(),
            Some("7 hours ahead of you.\n\nMake me a packing list.\n\nHere's")
        );
    }

    #[test]
    fn two_adjacent_bubbles_have_nothing_between() {
        let rows = transcript();
        let span = Span {
            start: at(2, "m3.u", "Make me a packing list."),
            start_index: 10,
            end: at(3, "m4#text", "Here's a list:\n• Boards\n• Leash"),
            end_index: 6,
        };
        assert_eq!(
            copy_across(&span, &rows).as_deref(),
            Some("packing list.\n\nHere's")
        );
    }

    /// A typed bubble's selectable text is what it renders: a heading reads
    /// without its `#` ([feedback chat-markdown-mac-2026-10-27]), `#hashtag`
    /// and plain lines as typed, a fenced block as its code.
    #[test]
    fn a_user_bubble_is_selectable_as_rendered() {
        assert_eq!(user_bubble_plain("# a heading"), "a heading");
        assert_eq!(user_bubble_plain("#hashtag stays"), "#hashtag stays");
        assert_eq!(
            user_bubble_plain("see\n```rs\nlet x = 1;\n```"),
            "see\nlet x = 1;"
        );
    }
}

#[cfg(test)]
mod link_icon_tests {
    //! Jacob, 2026-10-25 22:34 UTC ([feedback link-icons-mac-2026-10-25]): every named
    //! link in a reply — bullets, prose, table cells — carries its site's small rounded
    //! icon before the name, as in Grok Bot; a site without one shows a neutral square.
    //! His example: Bubble's New York Post headline list.
    use std::sync::Arc;

    use gpui::TestAppContext;

    use super::*;
    use crate::ui::select::{LinkIcons, PAINTED_LINK_ICONS};

    /// The reply to "Read the latest from the New York post" (Jacob's iPhone screenshot),
    /// seven linked headlines as bullets, then one line of prose.
    const NYPOST: &str = "- [Syringe attack: lawyer claims mistaken identity](https://nypost.com/2026/10/25/us-news/syringe-attack/)\n- [Arrest story: a second Jewish victim minutes earlier](https://nypost.com/2026/10/25/us-news/arrest-story/)\n- [Ex-NYPD cop charged in a George Washington Bridge killing, free on $1M bail](https://nypost.com/2026/10/25/us-news/ex-nypd-cop/)\n- [Thomas Markle home search, child pornography, Philippine official’s comment](https://nypost.com/2026/10/25/world-news/thomas-markle/)\n- [Musk says SpaceX cut Ukraine’s Starlink access](https://nypost.com/2026/10/25/business/musk-starlink/)\n- [Giants beat the Texans in overtime in J. J. McCarthy’s first start](https://nypost.com/2026/10/25/sports/giants-texans/)\n- [McCarthy’s wife after that win](https://nypost.com/2026/10/25/sports/mccarthy-wife/)\n\nNo times or bylines were on the page text I read. Want one of these opened?";

    #[test]
    fn named_links_get_an_icon_cell_and_name_their_site() {
        let cell = markdown::LINK_ICON_CELL;
        let spans = inline_spans("See [El Virrey](https://www.elvirrey.com/shop) or https://x.com/a and [https://y.com](https://y.com).");
        let links: Vec<_> = spans.iter().filter(|s| s.style.link).collect();
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].text, format!("{cell}El Virrey"));
        assert_eq!(links[0].icon.as_deref(), Some("www.elvirrey.com"));
        // Bare URLs (and a URL as its own label) have no cell; their text
        // carries the wrap hints (`urlwrap`), their target never does.
        assert_eq!(urlwrap::plain(&links[1].text), "https://x.com/a");
        assert_eq!(links[1].text, urlwrap::breakable("https://x.com/a"));
        assert_eq!(links[1].href.as_deref(), Some("https://x.com/a"));
        assert_eq!(links[1].icon, None);
        assert_eq!(urlwrap::plain(&links[2].text), "https://y.com");
        assert_eq!(links[2].text, urlwrap::breakable("https://y.com"));
        assert_eq!(links[2].href.as_deref(), Some("https://y.com"));
        assert_eq!(links[2].icon, None);
        assert_eq!(
            markdown::link_host("https://user@Sub.Example.com:8443/p?q#f").as_deref(),
            Some("sub.example.com")
        );
        assert_eq!(markdown::link_host("http://10.0.0.1/x"), None);
        assert_eq!(markdown::link_host("mailto:a@b.com"), None);
        assert_eq!(markdown::link_host("https://localhost/"), None);
        // Every block kind but code yields its hosts, each once, in reading order.
        let hosts = markdown::link_hosts(&format!(
            "[A](https://a.com) and [B](https://b.org/x)\n\n{NYPOST}\n\n| k | v |\n| --- | --- |\n| [C](https://c.net) | [A again](https://a.com/2) |\n\n```\n[D](https://d.io)\n```"
        ));
        assert_eq!(hosts, vec!["a.com", "b.org", "nypost.com", "c.net"]);
    }

    struct ListHost {
        selection: Entity<TextSelection>,
        icons: Rc<LinkIcons>,
    }

    impl gpui::Render for ListHost {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            let blocks = split_blocks(NYPOST);
            let texts = BubbleText::for_blocks(&blocks, false);
            let selectable = Selectable::new(&self.selection, theme.accent_soft, cx)
                .with_link_icons(self.icons.clone(), link_icon_placeholder(&theme));
            let bubble = selectable.bubble("m1#text", texts[0].text.clone());
            let mut col = div().w(px(520.0)).flex().flex_col();
            for (i, block) in blocks.iter().enumerate() {
                col = col.child(
                    div()
                        .debug_selector(move || format!("test-block-{i}"))
                        .child(render_block(
                            &format!("m1#t.{i}"),
                            block,
                            texts[i].at,
                            None,
                            Instant::now(),
                            &theme,
                            &bubble,
                            &CodeCopy::default(),
                        )),
                );
            }
            col
        }
    }

    /// Headless render of the list: seven bullets, each a link whose text starts with the
    /// icon cell, and seven icons painted — the site's picture when the bubble has one, the
    /// placeholder square while it loads or when there is none.
    #[gpui::test]
    fn the_nypost_list_paints_an_icon_before_every_headline(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let theme = crate::ui::theme::Theme::for_pref(
                crate::ui::theme::AppearancePref::Dark,
                crate::ui::theme::Surface::Matte,
                cx,
            );
            cx.set_global(theme);
        });
        let blocks = split_blocks(NYPOST);
        let [Block::Bullets(items), Block::Paragraph(_)] = blocks.as_slice() else {
            panic!("a bullet list then prose, got {blocks:?}");
        };
        assert_eq!(items.len(), 7);
        let theme = Theme::dark(crate::ui::theme::Surface::Matte);
        for item in items {
            let (text, runs, links, icons, _) =
                inline_runs(item, theme.text, &theme, 0, &|_| false);
            assert!(text.starts_with(markdown::LINK_ICON_CELL), "{text}");
            assert_eq!(icons.len(), 1);
            assert_eq!(icons[0].1, "nypost.com");
            assert_eq!(links.len(), 1);
            assert_eq!(
                links[0].0,
                0..text.len(),
                "the click target covers cell and name"
            );
            assert_eq!(runs[0].color, gpui::transparent_black());
            assert_eq!(runs[0].len, markdown::LINK_ICON_CELL.len());
        }
        // One real picture for nypost.com; render twice: with it, and still loading.
        let png = {
            let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 30, 30, 255]));
            let mut out = std::io::Cursor::new(Vec::new());
            img.write_to(&mut out, image::ImageFormat::Png).unwrap();
            out.into_inner()
        };
        let picture = Arc::new(crate::app::decode_image(&png).expect("png decodes"));
        for (icons, label) in [
            (
                Rc::new(LinkIcons {
                    by_host: HashMap::from([("nypost.com".to_string(), Some(picture.clone()))]),
                }),
                "pictures",
            ),
            (LinkIcons::empty(), "placeholders"),
        ] {
            PAINTED_LINK_ICONS.with(|n| n.set(0));
            let (_view, vcx) = cx.add_window_view(|_, cx| ListHost {
                selection: cx.new(TextSelection::new),
                icons,
            });
            vcx.run_until_parked();
            let painted = PAINTED_LINK_ICONS.with(|n| n.get());
            assert_eq!(painted, 7, "{label}: one icon per headline");
            for i in 0..7 {
                let text = vcx
                    .debug_bounds(match i {
                        0 => "list-text-0",
                        1 => "list-text-1",
                        2 => "list-text-2",
                        3 => "list-text-3",
                        4 => "list-text-4",
                        5 => "list-text-5",
                        _ => "list-text-6",
                    })
                    .expect("bullet text drawn");
                assert!(text.size.height >= px(CHAT_LINE_HEIGHT));
            }
        }
    }
}

#[cfg(test)]
mod markdown_tests {
    //! Jacob, 2026-10-24 (Grok side by side, [feedback bubble-markdown-2026-10-24]):
    //! "Arbos list bullets are small dots with tighter heading weight; Grok
    //! headings are bolder and bullets render more like markdown." Measured
    //! on `grok-chat-1.png` at 2 px/pt: 22 pt text line, items 26 pt apart,
    //! blocks 32 pt apart, a 4.5 pt disc centred 8.5 pt in, text at 20.
    use gpui::TestAppContext;

    use super::*;

    /// The pitches Grok's list reads at, over our line height.
    #[test]
    fn the_rhythm_is_groks() {
        let [line, item, block] =
            std::hint::black_box([CHAT_LINE_HEIGHT, LIST_ITEM_GAP, BLOCK_GAP_HALF]);
        assert_eq!(
            line, 22.0,
            "Grok's wrapped lines are 44 px apart at 2 px/pt"
        );
        assert_eq!(line + item, 26.0, "item pitch");
        assert_eq!(line + 2.0 * block, 32.0, "block pitch");
        assert_eq!(STRONG_WEIGHT, FontWeight::BOLD);
    }

    /// `**bold**` is bold (700), not semibold; the rest of the line keeps the
    /// body weight.
    #[test]
    fn strong_runs_are_bold() {
        let theme = Theme::dark(crate::ui::theme::Surface::Matte);
        let (text, runs, _, _, _) =
            inline_runs("Surf **gear** list", theme.text, &theme, 0, &|_| false);
        assert_eq!(text, "Surf gear list");
        let weights: Vec<(usize, FontWeight)> =
            runs.iter().map(|r| (r.len, r.font.weight)).collect();
        assert_eq!(
            weights,
            vec![
                (5, FontWeight::NORMAL),
                (4, FontWeight::BOLD),
                (5, FontWeight::NORMAL)
            ]
        );
    }

    struct Bullets {
        selection: Entity<TextSelection>,
    }

    impl gpui::Render for Bullets {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::dark(crate::ui::theme::Surface::Matte);
            let selectable = Selectable::new(&self.selection, theme.accent_soft, cx);
            let block = Block::Bullets(vec![
                "Leash, with a spare".into(),
                "Wax and a wax comb".into(),
                "Ding repair kit".into(),
            ]);
            let text = bubble_plain(std::slice::from_ref(&block), false);
            let bubble = selectable.bubble("b#text", text);
            div()
                .w(px(500.0))
                .h(px(300.0))
                .pl(px(40.0))
                .pt(px(10.0))
                .child(
                    div()
                        .w(px(400.0))
                        .debug_selector(|| "test-block".into())
                        .child(render_block(
                            "b",
                            &block,
                            0,
                            None,
                            Instant::now(),
                            &theme,
                            &bubble,
                            &CodeCopy::default(),
                        )),
                )
        }
    }

    /// Rendered: a real disc of Grok's size at Grok's place, the text at the
    /// 20 pt indent, one item every 26 pt.
    #[gpui::test]
    fn bullets_are_discs_at_groks_indent_and_pitch(cx: &mut TestAppContext) {
        let (_view, vcx) = cx.add_window_view(|_, cx| Bullets {
            selection: cx.new(TextSelection::new),
        });
        vcx.run_until_parked();
        let block = vcx.debug_bounds("test-block").expect("block drawn");
        let left = f32::from(block.left());
        let dot = vcx.debug_bounds("list-bullet-0").expect("bullet drawn");
        assert_eq!(f32::from(dot.size.width), BULLET_DOT);
        assert_eq!(f32::from(dot.size.height), BULLET_DOT);
        let dot_centre = f32::from(dot.left() + dot.right()) / 2.0 - left;
        assert!(
            (dot_centre - BULLET_CENTRE).abs() <= 0.5,
            "dot centre {dot_centre} pt from the block's left"
        );
        let text = vcx.debug_bounds("list-text-0").expect("item text drawn");
        assert!(
            (f32::from(text.left()) - left - LIST_INDENT).abs() <= 0.5,
            "text starts {} pt in",
            f32::from(text.left()) - left
        );
        let item0 = vcx.debug_bounds("list-item-0").expect("item 0 drawn");
        let item1 = vcx.debug_bounds("list-item-1").expect("item 1 drawn");
        let item2 = vcx.debug_bounds("list-item-2").expect("item 2 drawn");
        let pitch = f32::from(item1.top() - item0.top());
        assert!((pitch - 26.0).abs() <= 0.5, "item pitch {pitch}");
        assert!((f32::from(item2.top() - item1.top()) - 26.0).abs() <= 0.5);
        // The dot sits on the first line's centre.
        let line_centre = f32::from(item0.top()) + CHAT_LINE_HEIGHT / 2.0;
        let dot_mid = f32::from(dot.top() + dot.bottom()) / 2.0;
        assert!(
            (dot_mid - line_centre).abs() <= 0.5,
            "dot mid {dot_mid} vs line {line_centre}"
        );
    }

    /// The copy of a list is unchanged: "• " markers, one item per line.
    #[test]
    fn list_plain_text_keeps_its_markers() {
        let block = Block::Bullets(vec!["a".into(), "b".into()]);
        assert_eq!(block_plain(&block, false), "• a\n• b");
    }
}

#[cfg(test)]
mod chat_markdown_tests {
    //! Jacob, 2026-10-27 ([feedback chat-markdown-mac-2026-10-27]): "allow these
    //! common chat highlighting stuff for users to use > etc." Grok Bot renders a
    //! typed `> hello` as a quote block; the Mac composer sent it as raw text.
    //! Typed bubbles now go through the shared grammar (quotes, headings, lists,
    //! code cards, bold / italic / strike / spoiler / code / links), and plain
    //! text still renders as typed.
    use gpui::{point, TestAppContext};

    use super::*;

    /// A typed `> hello` is a quote; `>5` and a lone `>` are text.
    #[test]
    fn a_typed_quote_is_a_quote_block_and_a_glued_angle_is_text() {
        assert_eq!(
            split_blocks("> hello\nthat is what the quote looks like now"),
            vec![
                Block::Quote(vec![Block::Paragraph("hello".into())]),
                Block::Paragraph("that is what the quote looks like now".into()),
            ]
        );
        assert_eq!(
            split_blocks("> a\n>> b"),
            vec![Block::Quote(vec![
                Block::Paragraph("a".into()),
                Block::Quote(vec![Block::Paragraph("b".into())]),
            ])]
        );
        for text in [">5 means more than five", ">", "a > b"] {
            assert_eq!(split_blocks(text), vec![Block::Paragraph(text.into())]);
        }
    }

    /// The plain text of a typed bubble: as typed when nothing formats, the
    /// rendered words (markers gone, list markers kept) otherwise.
    #[test]
    fn typed_bubble_plain_text() {
        assert_eq!(
            user_bubble_plain("snake_case 2*3*4 jacob_s@example.com $5"),
            "snake_case 2*3*4 jacob_s@example.com $5"
        );
        assert!(is_plain_paragraph(&split_blocks("hello https://a.com/x_y")));
        assert!(!is_plain_paragraph(&split_blocks("> hello")));
        assert!(!is_plain_paragraph(&split_blocks("**hello**")));
        assert_eq!(user_bubble_plain("> **hi**\n- a\n- b"), "hi\n• a\n• b");
        assert_eq!(
            block_plain(
                &Block::Quote(vec![
                    Block::Paragraph("q".into()),
                    Block::Bullets(vec!["a".into()])
                ]),
                false
            ),
            "q\n• a"
        );
        // Every segment's start is where its text sits in the bubble's text.
        let blocks = split_blocks("> one\n> two\n\nafter ~~x~~");
        let plain = bubble_plain(&blocks, false);
        let offsets = block_offsets(&blocks, false);
        assert_eq!(plain, "one\ntwo\nafter x");
        assert_eq!(&plain[offsets[1]..], "after x");
    }

    /// `~~strike~~` strikes through; a `||spoiler||` paints as a pill with the
    /// glyphs invisible until the viewer opens it, and its range is clickable.
    #[test]
    fn strike_and_spoiler_runs() {
        let theme = Theme::dark(crate::ui::theme::Surface::Matte);
        let (text, runs, _, _, spoilers) =
            inline_runs("a ~~gone~~ ||secret|| b", theme.text, &theme, 10, &|_| {
                false
            });
        assert_eq!(text, "a gone secret b");
        assert!(runs[1].strikethrough.is_some(), "{runs:?}");
        assert!(runs[0].strikethrough.is_none());
        let pill = &runs[3];
        assert_eq!(pill.len, "secret".len());
        assert_eq!(pill.color.a, 0.0, "hidden glyphs");
        assert_eq!(
            pill.background_color,
            Some(spoiler_fill(theme.text, &theme, false))
        );
        assert_eq!(
            spoilers,
            vec![(7..13, 17)],
            "range in the element, start in the bubble"
        );
        let (_, runs, _, _, _) =
            inline_runs("a ~~gone~~ ||secret|| b", theme.text, &theme, 10, &|at| {
                at == 17
            });
        assert_eq!(runs[3].color, theme.text, "open: readable");
        assert_eq!(
            runs[3].background_color,
            Some(spoiler_fill(theme.text, &theme, true))
        );
        // A spoiler with styled parts is one spoiler: every run shares its start.
        let (_, _, _, _, spoilers) = inline_runs("||**a** b||", theme.text, &theme, 0, &|_| false);
        assert_eq!(spoilers, vec![(0..1, 0), (1..3, 0)]);
    }

    struct QuoteBubble {
        selection: Entity<TextSelection>,
    }

    const QUOTED: &str = "> hello there\n\nthe ||butler|| did it";

    impl gpui::Render for QuoteBubble {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::dark(crate::ui::theme::Surface::Matte);
            let selectable = Selectable::new(&self.selection, theme.accent_soft, cx);
            let blocks = split_blocks(QUOTED);
            let bubble = selectable.bubble("u#text", bubble_plain(&blocks, false));
            let offsets = block_offsets(&blocks, false);
            let mut col = div()
                .w(px(400.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .debug_selector(|| "test-block".into());
            for (i, block) in blocks.iter().enumerate() {
                col = col.child(render_block_in(
                    &format!("u{i}"),
                    block,
                    theme.text,
                    offsets[i],
                    None,
                    Instant::now(),
                    &theme,
                    &bubble,
                    &CodeCopy::default(),
                ));
            }
            div().w(px(500.0)).h(px(300.0)).p(px(20.0)).child(col)
        }
    }

    /// Rendered: the quote bar is 3 pt wide at the block's left, the quoted
    /// text starts `QUOTE_GAP` after it; a click on the spoiler opens it for
    /// this viewer, a second click hides it again.
    #[gpui::test]
    fn a_quote_draws_its_bar_and_a_click_opens_a_spoiler(cx: &mut TestAppContext) {
        let selection = cx.update(|cx| cx.new(TextSelection::new));
        let (_view, vcx) = cx.add_window_view(|_, _| QuoteBubble {
            selection: selection.clone(),
        });
        vcx.run_until_parked();
        let block = vcx.debug_bounds("test-block").expect("block drawn");
        let bar = vcx.debug_bounds("quote-bar").expect("quote bar drawn");
        assert_eq!(f32::from(bar.size.width), QUOTE_BAR);
        assert!(
            (f32::from(bar.left() - block.left())).abs() <= 0.5,
            "bar at the left"
        );
        assert!(
            f32::from(bar.size.height) >= CHAT_LINE_HEIGHT - 1.0,
            "the bar spans the quoted line: {bar:?}"
        );
        let quote = vcx.debug_bounds("quote-block").expect("quote drawn");
        assert!(f32::from(quote.size.height) >= CHAT_LINE_HEIGHT - 1.0);

        // The second paragraph: "the ||butler|| did it" — click inside "butler".
        let plain = bubble_plain(&split_blocks(QUOTED), false);
        let start = plain.find("butler").expect("spoiler text in the bubble");
        let revealed = |vcx: &gpui::VisualTestContext| {
            selection.read_with(vcx, |s, _| s.spoiler_revealed(&("u#text".into(), start)))
        };
        assert!(!revealed(vcx), "hidden at first");
        let para = quote.bottom() + px(6.0);
        let line_h = vcx.update(|window, _| window.line_height());
        // "the " is four glyphs in; the pill starts after them.
        let x = block.left() + px(40.0);
        vcx.simulate_click(point(x, para + line_h / 2.0), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(revealed(vcx), "one click opens the spoiler");
        vcx.simulate_click(point(x, para + line_h / 2.0), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(!revealed(vcx), "a second click hides it again");
        assert!(cx.opened_url().is_none(), "nothing opened in the browser");
    }

    /// Headings, lists, fences and links a user types render as blocks; the
    /// one-line preview and the link hosts see inside quotes.
    #[test]
    fn typed_formats_split_into_blocks() {
        let blocks = split_blocks(
            "# Title\n> [Docs](https://docs.arbos.life/x)\n- a\n1. b\n```py\nx = 1\n```\n**done**",
        );
        assert_eq!(blocks.len(), 6, "{blocks:?}");
        assert!(matches!(blocks[0], Block::Heading { level: 1, .. }));
        assert!(matches!(blocks[1], Block::Quote(_)));
        assert!(matches!(blocks[2], Block::Bullets(_)));
        assert!(matches!(blocks[3], Block::Numbered(_)));
        assert!(matches!(blocks[4], Block::Code { .. }));
        assert_eq!(
            one_line("> **hello** there\n\n||secret||"),
            "hello there secret"
        );
        assert_eq!(
            markdown::link_hosts("> see [Docs](https://docs.arbos.life/x)"),
            vec!["docs.arbos.life".to_string()]
        );
        // A named link's label keeps its styles and its one icon cell.
        let spans = inline_spans("[**Bold** link](https://arbos.life/a)");
        assert_eq!(spans.len(), 2);
        assert!(spans[0].text.starts_with(markdown::LINK_ICON_CELL) && spans[0].style.bold);
        assert_eq!(spans[0].icon.as_deref(), Some("arbos.life"));
        assert!(spans[1].icon.is_none() && spans[1].style.link);
    }
}

#[cfg(test)]
mod rail_tests {
    //! Jacob, 2026-10-24 (Grok side by side, [feedback bubble-hover-rail-2026-10-24]):
    //! the ☺ ↩ ⋯ rail sits vertically centred beside the bubble, and it shows
    //! while the pointer is anywhere on the bubble — not only on its last
    //! block. An agent reply is one transcript row per markdown block, so both
    //! are about the rows of one message acting as one bubble.
    use gpui::{point, size, Bounds, TestAppContext};

    use super::*;

    fn b(top: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(0.0), px(top)), size(px(800.0), px(height)))
    }

    const NO_INSET: (Pixels, Pixels) = (px(0.0), px(0.0));

    /// Hovering any block row of a reply hovers the bubble; moving from one
    /// block to the next — leave-then-enter or enter-then-leave — keeps it
    /// hovered; leaving the last row the pointer was on fades it out.
    #[test]
    fn the_bubble_stays_hovered_across_its_block_rows() {
        let mut fade = HoverFade::default();
        let now = Instant::now();
        let later = now + Duration::from_millis(HOVER_FADE_MS as u64 + 50);
        assert!(fade.set("m1#bubble", "m1.0", true, now), "enter block 0");
        assert_eq!(fade.opacity("m1#bubble", later).0, 1.0);
        // Block 0 → block 1, leave reported first: the leave starts a fade
        // out, the enter in the same instant resumes from full — no dip.
        fade.set("m1#bubble", "m1.0", false, later);
        fade.set("m1#bubble", "m1.1", true, later);
        assert_eq!(fade.opacity("m1#bubble", later).0, 1.0, "still hovered");
        // Block 1 → block 2, enter reported first: nothing changes at all.
        assert!(!fade.set("m1#bubble", "m1.2", true, later));
        assert!(!fade.set("m1#bubble", "m1.1", false, later));
        assert_eq!(fade.opacity("m1#bubble", later).0, 1.0, "still hovered");
        // Off the bubble: fades out.
        assert!(fade.set("m1#bubble", "m1.2", false, later));
        let (opacity, moving) = fade.opacity(
            "m1#bubble",
            later + Duration::from_millis(HOVER_FADE_MS as u64 / 2),
        );
        assert!(opacity < 1.0 && moving, "fading out: {opacity}");
        assert_eq!(
            fade.opacity("m1#bubble", later + Duration::from_secs(1)).0,
            0.0
        );
    }

    /// Two bubbles: entering the second's row leaves the first.
    #[test]
    fn hovering_another_bubble_fades_the_first() {
        let mut fade = HoverFade::default();
        let now = Instant::now();
        let later = now + Duration::from_secs(1);
        fade.set("m1#bubble", "m1.0", true, now);
        assert!(fade.set("m2#bubble", "m2.0", true, later));
        let settled = later + Duration::from_secs(1);
        assert_eq!(fade.opacity("m2#bubble", settled).0, 1.0);
        assert_eq!(fade.opacity("m1#bubble", settled).0, 0.0);
    }

    /// Mac 0.6.268, 2026-10-26 (Jacob: SIGABRT at 14:55:17, 15:21:53 and then at every
    /// launch): the list's measured rows were of two layout generations — the rows above
    /// still at their old place, the last row at its new one — so the bubble's span came out
    /// inverted or shorter than the rail button, and `clamp` panicked on the main thread
    /// (`assertion failed: min <= max`). Both shapes now give a centre, never a panic.
    #[test]
    fn stale_or_tiny_measurements_never_panic_the_rail() {
        let viewport = b(0.0, 1000.0);
        // Inverted: the last row was measured above the rows "above" it.
        let rows = [Some(b(600.0, 100.0)), Some(b(700.0, 60.0))];
        let last = b(300.0, 20.0);
        let centre = rail_centre(&rows, last, viewport, (px(17.0), px(3.0)));
        assert!(f32::from(centre).is_finite());
        // Shorter than the button: a 10 pt bubble.
        let centre = rail_centre(&[], b(500.0, 10.0), viewport, (px(3.0), px(3.0)));
        assert!(f32::from(centre).is_finite());
        // A viewport too short for the rail at all.
        let centre = rail_centre(&[], b(10.0, 40.0), b(0.0, 20.0), NO_INSET);
        assert!(f32::from(centre).is_finite());
        assert_eq!(clamp_or_keep(px(5.0), px(10.0), px(0.0)), px(5.0));
        assert_eq!(clamp_or_keep(px(5.0), px(0.0), px(10.0)), px(5.0));
        assert_eq!(clamp_or_keep(px(50.0), px(0.0), px(10.0)), px(10.0));
    }

    /// The rail's centre is the middle of the whole bubble — the rows above
    /// plus the last — measured from the last row's top, less the row padding
    /// around the bubble.
    #[test]
    fn the_rail_is_centred_on_the_whole_bubble() {
        let viewport = b(0.0, 1000.0);
        // Three block rows: 100 + 60 + 40 from y = 200; the bubble spans 200..400.
        let rows = [Some(b(200.0, 100.0)), Some(b(300.0, 60.0))];
        let last = b(360.0, 40.0);
        let centre = rail_centre(&rows, last, viewport, NO_INSET);
        assert_eq!(
            f32::from(centre),
            300.0 - 360.0,
            "mid-bubble is 60 above the last row"
        );
        // The half gap above the first row and below the last is not bubble.
        let centre = rail_centre(&rows, last, viewport, (px(3.0), px(3.0)));
        assert_eq!(
            f32::from(centre),
            300.0 - 360.0,
            "symmetric insets keep the middle"
        );
        let centre = rail_centre(&rows, last, viewport, (px(17.0), px(3.0)));
        assert_eq!(
            f32::from(centre),
            307.0 - 360.0,
            "a turn gap above shifts it down"
        );
    }

    /// A bubble taller than the pane keeps its rail on screen: the centre is
    /// clamped inside the viewport, under the header fade band.
    #[test]
    fn a_tall_bubble_keeps_its_rail_inside_the_viewport() {
        let viewport = b(100.0, 600.0);
        // The bubble runs from far above the viewport (unmeasured rows) to below it.
        let rows = [None, Some(b(-900.0, 500.0)), Some(b(-400.0, 500.0))];
        let last = b(100.0, 900.0);
        let centre = f32::from(rail_centre(&rows, last, viewport, NO_INSET)) + 100.0;
        let low = 100.0 + HEADER_FADE_BAND + RAIL_BUTTON / 2.0 + RAIL_EDGE_INSET;
        let high = 700.0 - RAIL_BUTTON / 2.0 - RAIL_EDGE_INSET;
        assert!(
            (low..=high).contains(&centre),
            "centre {centre} outside the pane's {low}..{high}"
        );
        // A bubble that ends above the viewport's bottom: clamped to the bubble, not past it.
        let rows = [Some(b(-200.0, 100.0))];
        let last = b(-100.0, 150.0);
        let centre = f32::from(rail_centre(&rows, last, viewport, NO_INSET)) - 100.0;
        assert!(
            centre <= 50.0 - RAIL_BUTTON / 2.0 + 0.01,
            "rail {centre} below the bubble's bottom (50)"
        );
    }

    struct Stack;

    impl gpui::Render for Stack {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            // Three stacked "block rows" of one bubble, 60 + 40 + 20 tall; the
            // last carries the rail, centred on all three (= 60 above its own top).
            let block = |h: f32| div().w(px(300.0)).h(px(h)).relative();
            div()
                .w(px(500.0))
                .h(px(400.0))
                .flex()
                .flex_col()
                .child(block(60.0).debug_selector(|| "test-first".into()))
                .child(block(40.0))
                .child(
                    block(20.0).debug_selector(|| "test-last".into()).child(
                        hover_rail_frame(Some(px(-40.0))).child(div().size(px(RAIL_BUTTON))),
                    ),
                )
        }
    }

    /// Rendered: the frame puts the rail's centre at the bubble's centre and
    /// hangs it 6 pt off the bubble's right edge.
    #[gpui::test]
    fn the_rail_frame_hangs_at_the_bubbles_centre(cx: &mut TestAppContext) {
        let (_view, vcx) = cx.add_window_view(|_, _| Stack);
        vcx.run_until_parked();
        let first = vcx.debug_bounds("test-first").expect("first row drawn");
        let last = vcx.debug_bounds("test-last").expect("last row drawn");
        let rail = vcx.debug_bounds("bubble-rail").expect("rail drawn");
        let bubble_centre = f32::from(first.top() + last.bottom()) / 2.0;
        let rail_centre = f32::from(rail.top() + rail.bottom()) / 2.0;
        assert!(
            (rail_centre - bubble_centre).abs() <= 0.5,
            "rail centre {rail_centre}, bubble centre {bubble_centre}"
        );
        assert!(
            (f32::from(rail.left() - last.right()) - 0.0).abs() <= 0.5,
            "rail frame starts at the bubble's right edge: {} vs {}",
            f32::from(rail.left()),
            f32::from(last.right())
        );
    }
}

#[cfg(test)]
mod system_line_tests {
    //! #1349: a relay-written membership notice (`role: system`, `sender: system`) is a
    //! centred stamp between the bubbles — never a message from the adder — and the
    //! bubble after it names its sender again.
    use std::collections::{HashMap, HashSet};

    use super::*;

    fn message(role: &str, sender: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": format!("m-{at}"),
            "chatId": "chat_testing",
            "role": role,
            "sender": sender,
            "senderHandle": if sender.starts_with("user:") { "bob" } else { "" },
            "status": "done",
            "createdAt": at,
            "parts": [{ "id": format!("p-{at}"), "kind": "text", "text": text }],
        }))
        .expect("message")
    }

    fn agent_reply(id: &str, text: &str, in_reply_to: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "chatId": "chat_group",
            "role": "assistant",
            "sender": "agent:doug",
            "senderName": "Doug",
            "senderHandle": "doug",
            "askedByHandle": "jacob",
            "inReplyTo": in_reply_to,
            "status": "done",
            "createdAt": at,
            "parts": [{ "id": format!("p-{id}"), "kind": "text", "text": text }],
        }))
        .expect("agent reply")
    }

    fn spawn_row(id: &str, name: &str, worker: bool, at: u64) -> Message {
        let mut row = serde_json::json!({
            "id": id,
            "chatId": "chat_testing",
            "role": "assistant",
            "sender": "box:1",
            "status": "done",
            "turn": "spawn",
            "subagentId": format!("sub-{id}"),
            "subagentName": name,
            "handoffMessages": 2,
            "activity": "Searching for surf schools Santa Teresa",
            "createdAt": at,
            "parts": [],
        });
        if worker {
            row["worker"] = serde_json::json!(true);
        }
        serde_json::from_value(row).expect("spawn row")
    }

    /// Grok parity trait 15 ([feedback hide-worker-rows-and-tz-2026-10-24]): a
    /// background worker's spawn row (`worker: true`) draws no "N messages
    /// with ◉ X" line; an agent-to-agent send (no flag) keeps its line; the
    /// worker row's `activity` still feeds the working line.
    #[test]
    fn worker_spawn_rows_are_hidden_but_their_activity_is_read() {
        let t = 1_700_000_000_000u64;
        let worker = spawn_row("w1", "santa-teresa-surf-schools", true, t + 1_000);
        let teammate = spawn_row("t1", "Comms", false, t + 2_000);
        assert!(worker.worker && !teammate.worker);
        let messages = [
            message("user", "user", "find me the 3 best surf schools", t),
            worker.clone(),
            teammate,
        ];
        let spoken = HashSet::new();
        let failed = HashMap::new();
        let speech = SpeechRows {
            enabled: false,
            spoken: &spoken,
            failed: &failed,
            voice_mode: false,
        };
        let rows = build_rows(
            &messages,
            false,
            speech,
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            false,
        );
        let handoffs: Vec<&Vec<String>> = rows
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::Handoff { names, .. } => Some(names),
                _ => None,
            })
            .collect();
        assert_eq!(handoffs, [&vec!["Comms".to_string()]], "{handoffs:?}");
        assert!(
            rows.iter().all(|r| r.message_id != "w1"),
            "no row at all for the worker"
        );
        assert_eq!(
            worker.current_activity().as_deref(),
            Some("Searching for surf schools Santa Teresa")
        );
    }

    #[test]
    fn membership_notice_is_a_centred_stamp_that_breaks_the_sender_run() {
        let t = 1_700_000_000_000u64;
        let messages = [
            message("user", "user:gh_bob", "hi all", t),
            message(
                "system",
                "system",
                "Jacob added Home, hjkb and Doug",
                t + 1_000,
            ),
            message("user", "user:gh_bob", "welcome", t + 2_000),
        ];
        let spoken = HashSet::new();
        let failed = HashMap::new();
        let speech = SpeechRows {
            enabled: false,
            spoken: &spoken,
            failed: &failed,
            voice_mode: false,
        };
        let rows = build_rows(
            &messages,
            false,
            speech,
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            true,
        );
        let kinds: Vec<&RowKind> = rows.iter().map(|r| &r.kind).collect();
        // Date stamp, Bob (named), the notice, Bob again (named again).
        let bubbles: Vec<(bool, &str)> = kinds
            .iter()
            .filter_map(|k| match k {
                RowKind::User { named, text, .. } => Some((*named, text.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(bubbles, [(true, "hi all"), (true, "welcome")]);
        let stamps: Vec<&str> = kinds
            .iter()
            .filter_map(|k| match k {
                RowKind::Stamp { label } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            stamps.contains(&"Jacob added Home, hjkb and Doug"),
            "the notice is a stamp row: {stamps:?}"
        );
        assert!(
            rows.iter().all(|r| r.id != "m-1700000001000#user"),
            "no bubble for the system row"
        );
    }

    /// #1447, as grouped messages keep it: consecutive people-chat replies from the same
    /// agent each quote the message they answer, and a quoted bubble starts its own run —
    /// so each is named. (Unquoted follow-ups group; see `grouping_tests`.)
    #[test]
    fn consecutive_agent_replies_each_name_and_quote() {
        let t = 1_700_000_000_000u64;
        let user_a = message("user", "user:gh_jacob", "first ask", t);
        let user_b = message("user", "user:gh_jacob", "second ask", t + 1_000);
        let messages = [
            user_a,
            agent_reply("a1", "quick ack", "m-1700000000000", t + 2_000),
            agent_reply("a2", "full answer", "m-1700000000000", t + 3_000),
            user_b,
            agent_reply("a3", "answer two", "m-1700000001000", t + 4_000),
        ];
        let spoken = HashSet::new();
        let failed = HashMap::new();
        let speech = SpeechRows {
            enabled: false,
            spoken: &spoken,
            failed: &failed,
            voice_mode: false,
        };
        let rows = build_rows(
            &messages,
            false,
            speech,
            "gh_jacob",
            &HashSet::new(),
            &Mentions::empty(),
            true,
        );
        // The name sits inside each reply's first bubble (sender-name-in-bubble-2026-10-23).
        let named_bubbles: Vec<bool> = rows
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::Markdown {
                    first: true, named, ..
                } => Some(*named),
                _ => None,
            })
            .collect();
        assert_eq!(
            named_bubbles,
            [true, true, true],
            "each agent reply names the sender: {named_bubbles:?}"
        );
        let quotes: Vec<(&str, &str)> = rows
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::Markdown {
                    quote: Some(q),
                    first: true,
                    ..
                } => Some((q.message_id.as_str(), q.text.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            quotes,
            [
                ("m-1700000000000", "first ask"),
                ("m-1700000000000", "first ask"),
                ("m-1700000001000", "second ask"),
            ],
            "each reply quotes the message it answers: {quotes:?}"
        );
    }

    fn speech<'a>(spoken: &'a HashSet<Id>, failed: &'a HashMap<Id, String>) -> SpeechRows<'a> {
        SpeechRows {
            enabled: false,
            spoken,
            failed,
            voice_mode: false,
        }
    }

    /// Which bubbles carry a sender name: `(text, named)` for people, `("agent", named)`
    /// for an agent message's first block.
    fn names(rows: &[Row]) -> Vec<(&str, bool)> {
        rows.iter()
            .filter_map(|r| match &r.kind {
                RowKind::User {
                    named, text, from, ..
                } => Some((text.as_str(), *named && from.is_some())),
                RowKind::Markdown { named, first, .. } if *first => Some(("agent", *named)),
                _ => None,
            })
            .collect()
    }

    /// Jacob, 2026-10-23: the sender's name sits inside the bubble in group chats and
    /// channels — on the first bubble of a run (people and agents alike; these agent
    /// replies each quote the ask, so each starts a run) — and never on my own bubble.
    #[test]
    fn group_and_channel_bubbles_name_their_sender_but_not_mine() {
        let t = 1_700_000_000_000u64;
        let messages = [
            message("user", "user:gh_bob", "hi all", t),
            message("user", "user:gh_bob", "second", t + 1_000),
            message("user", "user:gh_me", "mine", t + 2_000),
            agent_reply("a1", "on it", "m-1700000002000", t + 3_000),
            agent_reply("a2", "done", "m-1700000002000", t + 4_000),
        ];
        let (spoken, failed) = (HashSet::new(), HashMap::new());
        let rows = build_rows(
            &messages,
            false,
            speech(&spoken, &failed),
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            true,
        );
        assert_eq!(
            names(&rows),
            [
                ("hi all", true),
                ("second", false),
                ("mine", false),
                ("agent", true),
                ("agent", true),
            ]
        );
    }

    /// Bubble menu → Forward (feedback app-01M5MXRZ7609S70P9S9P2RZDDF): a row with
    /// `forwardedFrom` draws the "Forwarded from <name>" header instead of a reply
    /// quote; a plain message carries neither.
    #[test]
    fn forwarded_bubble_carries_its_sender_header() {
        let t = 1_700_000_000_000u64;
        let mut forwarded = message("user", "user:gh_me", "see this", t + 1_000);
        forwarded.forwarded_from = Some(" Sam Rivera ".into());
        let messages = [message("user", "user:gh_bob", "hi", t), forwarded];
        let (spoken, failed) = (HashSet::new(), HashMap::new());
        let rows = build_rows(
            &messages,
            false,
            speech(&spoken, &failed),
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            false,
        );
        let headers: Vec<(Option<String>, bool)> = rows
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::User {
                    forwarded_from,
                    quote,
                    ..
                } => Some((forwarded_from.clone(), quote.is_some())),
                _ => None,
            })
            .collect();
        assert_eq!(
            headers,
            [(None, false), (Some("Sam Rivera".into()), false)],
            "the forward header is trimmed and a forward never quotes"
        );
    }

    /// A user message whose parts are a picture then `text` (the send shape), replying
    /// to `reply_to`.
    fn picture_reply(sender: &str, text: &str, reply_to: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": format!("m-{at}"),
            "chatId": "chat_testing",
            "role": "user",
            "sender": sender,
            "senderHandle": "bob",
            "status": "done",
            "createdAt": at,
            "replyTo": reply_to,
            "parts": [
                { "id": format!("a-{at}"), "kind": "attachment",
                  "attachment": { "name": "IMG_0042.jpeg", "mime": "image/jpeg", "ref": "blob-42" } },
                { "id": format!("p-{at}"), "kind": "text", "text": text },
            ],
        }))
        .expect("picture reply")
    }

    /// Per row: `("attachment" | "user", quote text)`.
    fn quotes(rows: &[Row]) -> Vec<(&str, Option<String>)> {
        rows.iter()
            .filter_map(|r| match &r.kind {
                RowKind::Attachment { quote, .. } => {
                    Some(("attachment", quote.as_ref().map(|q| q.text.clone())))
                }
                RowKind::User { quote, .. } => {
                    Some(("user", quote.as_ref().map(|q| q.text.clone())))
                }
                _ => None,
            })
            .collect()
    }

    /// Tester bug ([feedback picture-reply-quote-mac-2026-10-28]): a reply that carries a
    /// picture lost its quote — a wordless picture had no bubble to hold it, and a captioned
    /// picture drew it under the picture. The quote now sits above the first card, once;
    /// the caption bubble stays plain; a text reply to a picture quotes the file's name.
    #[test]
    fn picture_replies_quote_above_the_first_card() {
        let t = 1_700_000_000_000u64;
        let original = message("user", "user:gh_bob", "look at this", t);
        let wordless = picture_reply("user:gh_me", "", &original.id, t + 1_000);
        let captioned = picture_reply("user:gh_me", "same here", &original.id, t + 2_000);
        let mut to_picture = message("user", "user:gh_bob", "nice shot", t + 3_000);
        to_picture.reply_to = Some(wordless.id.clone());
        let messages = [original, wordless, captioned, to_picture];
        let (spoken, failed) = (HashSet::new(), HashMap::new());
        let rows = build_rows(
            &messages,
            false,
            speech(&spoken, &failed),
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            false,
        );
        assert_eq!(
            quotes(&rows),
            [
                ("user", None),
                ("attachment", Some("look at this".into())),
                ("attachment", Some("look at this".into())),
                ("user", None),
                ("user", Some("IMG_0042.jpeg".into())),
            ],
            "{rows:?}"
        );
    }

    /// A forwarded picture keeps its forward header and never quotes, on the card or
    /// the caption.
    #[test]
    fn forwarded_picture_never_quotes() {
        let t = 1_700_000_000_000u64;
        let original = message("user", "user:gh_bob", "look at this", t);
        let mut forwarded = picture_reply("user:gh_me", "caption", &original.id, t + 1_000);
        forwarded.forwarded_from = Some("Sam Rivera".into());
        let messages = [original, forwarded];
        let (spoken, failed) = (HashSet::new(), HashMap::new());
        let rows = build_rows(
            &messages,
            false,
            speech(&spoken, &failed),
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            false,
        );
        assert_eq!(
            quotes(&rows),
            [("user", None), ("attachment", None), ("user", None)]
        );
        assert!(rows.iter().any(|r| matches!(
            &r.kind,
            RowKind::Attachment { forwarded_from: Some(f), .. } if f.ends_with("Sam Rivera")
        )));
    }

    /// … and a 1:1 chat (me ↔ person, me ↔ agent) labels nobody, and carries no
    /// `#sender` caption row either.
    #[test]
    fn dm_and_agent_chat_bubbles_carry_no_name() {
        let t = 1_700_000_000_000u64;
        let messages = [
            message("user", "user:gh_bob", "hi", t),
            message("user", "user:gh_me", "hey", t + 1_000),
            agent_reply("a1", "hello", "m-1700000001000", t + 2_000),
        ];
        let (spoken, failed) = (HashSet::new(), HashMap::new());
        let rows = build_rows(
            &messages,
            false,
            speech(&spoken, &failed),
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            false,
        );
        assert_eq!(
            names(&rows),
            [("hi", false), ("hey", false), ("agent", false)]
        );
        assert!(
            rows.iter().all(|r| !r.id.ends_with("#sender")),
            "no sender header row above a bubble any more"
        );
    }

    /// #1604: in a multi-party chat an agent row with `agent:<id>` (and no
    /// `askedByHandle` — the shape a subagent-result turn used to ship) still
    /// sits on the avatar column and names its sender, like every other
    /// participant. A 1:1 agent chat (names_senders off) stays flush-left.
    #[test]
    fn multi_party_agent_bubble_keeps_lane_without_asked_by() {
        let t = 1_700_000_000_000u64;
        let mut agent = message("assistant", "agent:doug", "subagent result prose", t);
        agent.sender_name = Some("Doug".into());
        agent.sender_handle = Some("doug".into());
        // No asked_by_handle — the pre-#1604 SubagentResult shape.
        let (spoken, failed) = (HashSet::new(), HashMap::new());
        let group = build_rows(
            &[agent.clone()],
            false,
            speech(&spoken, &failed),
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            true,
        );
        let solo = build_rows(
            &[agent],
            false,
            speech(&spoken, &failed),
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            false,
        );
        let group_lane: Vec<Option<&str>> = group
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::Markdown {
                    first: true,
                    lane,
                    named,
                    ..
                } => Some((lane.as_deref(), *named)),
                _ => None,
            })
            .map(|(lane, named)| {
                assert!(named, "group names the agent");
                lane
            })
            .collect();
        assert_eq!(
            group_lane,
            [Some("agent:doug")],
            "group: avatar lane + disc seed"
        );
        let solo_lane: Vec<Option<&str>> = solo
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::Markdown {
                    first: true,
                    lane,
                    named,
                    ..
                } => {
                    assert!(!named, "1:1 never labels the agent");
                    Some(lane.as_deref())
                }
                _ => None,
            })
            .collect();
        assert_eq!(solo_lane, [None], "1:1: flush-left, no disc");
    }

    /// The chat-kind rule behind `names_senders`.
    #[test]
    fn names_senders_follows_the_chat_kind() {
        let dm = Project {
            kind: SpaceKind::Dm,
            ..Default::default()
        };
        let dm_with_agent = Project {
            kind: SpaceKind::Dm,
            agents: vec![Default::default()],
            ..Default::default()
        };
        let group = Project {
            kind: SpaceKind::Group,
            ..Default::default()
        };
        let agent = Project::default();
        let shared = Project {
            shared: true,
            ..Default::default()
        };
        let agent_group = Chat {
            id: "c".into(),
            project_id: "p".into(),
            title: "Launch sync".into(),
            is_pinned: false,
            is_system: false,
            kind: crate::model::ChatKind::Group,
            participants: Vec::new(),
            unread: false,
            unread_count: 0,
            last_message_at: 0,
            archived_at: None,
        };
        assert!(!names_senders(Some(&dm), None, false), "1:1 DM");
        assert!(!names_senders(Some(&agent), None, false), "me ↔ agent");
        assert!(names_senders(Some(&group), None, false), "people group");
        assert!(
            names_senders(Some(&dm_with_agent), None, false),
            "DM + agent"
        );
        assert!(names_senders(Some(&shared), None, false), "shared project");
        assert!(
            names_senders(Some(&agent), Some(&agent_group), false),
            "agent group"
        );
        assert!(names_senders(None, None, true), "channel");
        assert!(!names_senders(None, None, false), "nothing open");
        // People chats take the wider bubble share ([feedback
        // chat-density-mac-2026-10-26]): every chat with a sender disc, the
        // 1:1 DM included; never the 1:1 agent chat.
        assert!(
            people_chat(Some(&dm), None, false),
            "1:1 DM is a people chat"
        );
        assert!(people_chat(Some(&dm_with_agent), None, false));
        assert!(people_chat(Some(&group), None, false));
        assert!(people_chat(Some(&shared), None, false));
        assert!(people_chat(Some(&agent), Some(&agent_group), false));
        assert!(people_chat(None, None, true), "channel");
        assert!(
            !people_chat(Some(&agent), None, false),
            "me ↔ agent keeps Grok's widths"
        );
        assert!(!people_chat(None, None, false));
    }
}

#[cfg(test)]
mod grouping_tests {
    //! Grouped messages (Jacob, 2026-10-28, [feedback grouped-messages-mac-2026-10-28]):
    //! consecutive messages from one sender within `RUN_WINDOW_MS` form a run — the name
    //! and the disc once, on the first message; the later ones under its text with the
    //! avatar column empty and the 6 pt in-run gap. The run breaks on a sender change,
    //! on the window passing, at a system line or a date stamp, and at a reply quote.
    use std::collections::{HashMap, HashSet};

    use super::*;

    fn message(id: &str, role: &str, sender: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "chatId": "chat_group",
            "role": role,
            "sender": sender,
            "senderHandle": if sender.starts_with("user:") { sender.trim_start_matches("user:gh_") } else { "" },
            "status": "done",
            "createdAt": at,
            "parts": [{ "id": format!("p-{id}"), "kind": "text", "text": text }],
        }))
        .expect("message")
    }

    fn person(id: &str, who: &str, text: &str, at: u64) -> Message {
        message(id, "user", &format!("user:gh_{who}"), text, at)
    }

    /// A people-chat agent reply with no `inReplyTo` (a follow-up bubble of a
    /// multi-message answer): quotes nothing.
    fn agent(id: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "chatId": "chat_group",
            "role": "assistant",
            "sender": "agent:doug",
            "senderName": "Doug",
            "senderHandle": "doug",
            "askedByHandle": "jacob",
            "status": "done",
            "createdAt": at,
            "parts": [{ "id": format!("p-{id}"), "kind": "text", "text": text }],
        }))
        .expect("agent reply")
    }

    fn rows_of(messages: &[Message], names_senders: bool) -> (Vec<Row>, HashSet<Id>) {
        let (spoken, failed) = (HashSet::new(), HashMap::new());
        let speech = SpeechRows {
            enabled: false,
            spoken: &spoken,
            failed: &failed,
            voice_mode: false,
        };
        let mut cache = MessageRowCache::default();
        let rows = build_rows_with_cache(
            messages,
            false,
            speech,
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            names_senders,
            names_senders,
            true,
            &mut cache,
            &HashSet::new(),
        );
        (rows, cache.run_starts)
    }

    /// `(text, named, disc)` for every bubble from someone else, `("agent", named, disc)`
    /// for an agent's first block, `("mine", false, false)` for mine.
    fn shape(rows: &[Row]) -> Vec<(String, bool, bool)> {
        rows.iter()
            .filter_map(|r| match &r.kind {
                RowKind::User {
                    text,
                    named,
                    disc,
                    from: Some(_),
                    ..
                } => Some((text.clone(), *named, *disc)),
                RowKind::User { from: None, .. } => Some(("mine".into(), false, false)),
                RowKind::Markdown {
                    first: true,
                    named,
                    disc,
                    lane,
                    ..
                } => Some((
                    if lane.is_some() { "agent" } else { "agent-1:1" }.into(),
                    *named,
                    *disc,
                )),
                _ => None,
            })
            .collect()
    }

    const T: u64 = 1_700_000_000_000;

    /// Arbonites, 2026-10-28 (01M627S7…, 01M627VG…): a box wrote a hand-off row and a job
    /// row into a group with bare `sender: "agent"` and no words. A group draws neither — no
    /// "Messaged ◉ Bob" line from no one, no empty bubble — and Bob's run is not broken; a
    /// 1:1 agent chat keeps its hand-off line, and an attributed one still draws in a group.
    #[test]
    fn senderless_blank_rows_draw_nothing_in_a_group() {
        let blank = |id: &str, extra: serde_json::Value, at: u64| -> Message {
            let mut v = serde_json::json!({
                "id": id,
                "chatId": "chat_group",
                "role": "assistant",
                "sender": "agent",
                "status": "done",
                "createdAt": at,
                "parts": [{ "id": format!("p-{id}"), "kind": "text", "text": "" }],
            });
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            serde_json::from_value(v).expect("blank row")
        };
        let handoff = serde_json::json!({
            "turn": { "kind": "spawn" },
            "subagentName": "Bob",
            "handoffMessages": 1,
        });
        let messages = vec![
            person("m1", "bob", "morning", T),
            blank("h1", handoff.clone(), T + 1_000),
            blank("u1", serde_json::json!({ "role": "user" }), T + 1_500),
            person("m2", "bob", "anyone around?", T + 2_000),
            agent("m3", "Here.", T + 3_000),
        ];
        let (rows, _) = rows_of(&messages, true);
        assert!(
            rows.iter()
                .all(|r| !matches!(r.kind, RowKind::Handoff { .. })
                    && r.message_id != "h1"
                    && r.message_id != "u1"),
            "{:?}",
            rows.iter().map(|r| &r.id).collect::<Vec<_>>()
        );
        assert_eq!(
            shape(&rows),
            vec![
                ("morning".into(), true, true),
                ("anyone around?".into(), false, false),
                ("agent".into(), true, true),
            ]
        );

        let (rows, _) = rows_of(&[blank("h1", handoff.clone(), T)], false);
        assert!(rows
            .iter()
            .any(|r| matches!(r.kind, RowKind::Handoff { .. })));
        let mut named = blank("h2", handoff, T);
        named.sender = "agent:doug".into();
        let (rows, _) = rows_of(&[named], true);
        assert!(rows
            .iter()
            .any(|r| matches!(r.kind, RowKind::Handoff { .. })));
    }

    #[test]
    fn the_window_is_two_minutes_and_the_gaps_are_groks() {
        assert_eq!(RUN_WINDOW_MS, 120_000, "a couple of minutes = 2 min");
        assert_eq!(RUN_GAP, 6.0, "bubbles of one run sit 6 pt apart (Grok Mac)");
        assert_eq!(
            TURN_GAP, 20.0,
            "a new run opens 20 pt (Grok Mac), unchanged"
        );
    }

    #[test]
    fn continues_run_rule() {
        let tail = RunTail {
            sender: "user:gh_bob".into(),
            at: T,
        };
        assert!(continues_run(Some(&tail), "user:gh_bob", T + 1_000, false));
        assert!(
            continues_run(Some(&tail), "user:gh_bob", T + RUN_WINDOW_MS, false),
            "exactly the window still groups"
        );
        assert!(
            !continues_run(Some(&tail), "user:gh_bob", T + RUN_WINDOW_MS + 1, false),
            "one ms past the window breaks"
        );
        assert!(
            !continues_run(Some(&tail), "user:gh_sam", T + 1_000, false),
            "another sender breaks"
        );
        assert!(
            !continues_run(Some(&tail), "user:gh_bob", T + 1_000, true),
            "a reply quote breaks"
        );
        assert!(
            !continues_run(None, "user:gh_bob", T + 1_000, false),
            "no run yet"
        );
        assert!(
            !continues_run(Some(&tail), "user:gh_bob", 0, false),
            "a row without a clock never groups"
        );
        assert!(
            continues_run(Some(&tail), "user:gh_bob", T - 30_000, false),
            "a clock a little behind the run's (another device) still groups"
        );
    }

    #[test]
    fn same_sender_within_the_window_continues_the_run() {
        let messages = [
            person("m1", "bob", "hi all", T),
            person("m2", "bob", "two", T + 30_000),
            person("m3", "bob", "three", T + 90_000),
        ];
        let (rows, starts) = rows_of(&messages, true);
        assert_eq!(
            shape(&rows),
            [
                ("hi all".into(), true, true),
                ("two".into(), false, false),
                ("three".into(), false, false),
            ],
            "name and disc once, on the run's first message"
        );
        assert_eq!(
            starts,
            HashSet::from(["m1".to_string()]),
            "one turn gap, at the top"
        );
        // The later rows still keep the lane (the `from` person) so the bubble aligns
        // under the first one's text.
        assert!(rows
            .iter()
            .all(|r| !matches!(r.kind, RowKind::User { from: None, .. })));
    }

    #[test]
    fn a_sender_change_or_the_window_passing_starts_a_new_run() {
        let messages = [
            person("m1", "bob", "a", T),
            person("m2", "sam", "b", T + 10_000),
            person("m3", "sam", "c", T + 10_000 + RUN_WINDOW_MS + 1),
            person("m4", "bob", "d", T + 10_000 + RUN_WINDOW_MS + 2_000),
        ];
        let (rows, starts) = rows_of(&messages, true);
        assert_eq!(
            shape(&rows),
            [
                ("a".into(), true, true),
                ("b".into(), true, true),
                ("c".into(), true, true),
                ("d".into(), true, true),
            ]
        );
        assert_eq!(starts.len(), 4);
    }

    #[test]
    fn a_system_line_a_date_stamp_or_a_quote_breaks_the_run() {
        let mut quoted = person("m5", "bob", "re: that", T + 3_000);
        quoted.reply_to = Some("m1".into());
        let messages = [
            person("m1", "bob", "a", T),
            message("s1", "system", "system", "Jacob added Doug", T + 1_000),
            person("m2", "bob", "b", T + 2_000),
            quoted,
            person("m6", "bob", "after the quote", T + 4_000),
            // An hour later: the date stamp lands and breaks the run (the window has
            // passed anyway).
            person("m7", "bob", "next hour", T + 2 * STAMP_GAP_MS),
        ];
        let (rows, starts) = rows_of(&messages, true);
        assert_eq!(
            shape(&rows),
            [
                ("a".into(), true, true),
                ("b".into(), true, true),
                ("re: that".into(), true, true),
                ("after the quote".into(), false, false),
                ("next hour".into(), true, true),
            ],
            "the quoted bubble starts a run of its own; the next line joins it"
        );
        // The system line keeps its own turn gap above (as before), so it counts as a start.
        assert_eq!(
            starts,
            HashSet::from([
                "m1".to_string(),
                "s1".to_string(),
                "m2".to_string(),
                "m5".to_string(),
                "m7".to_string()
            ])
        );
    }

    #[test]
    fn agents_group_like_people_unless_they_quote() {
        let mut quoting = agent("a3", "answering the ask", T + 40_000);
        quoting.in_reply_to = Some("m1".into());
        let messages = [
            person("m1", "jacob", "ask", T),
            agent("a1", "first part", T + 10_000),
            agent("a2", "second part", T + 20_000),
            quoting,
            agent("a4", "and a note", T + 50_000),
        ];
        let (rows, starts) = rows_of(&messages, true);
        assert_eq!(
            shape(&rows),
            [
                ("ask".into(), true, true),
                ("agent".into(), true, true),
                ("agent".into(), false, false),
                ("agent".into(), true, true),
                ("agent".into(), false, false),
            ],
            "an unquoted follow-up continues the agent's run; a quoted reply starts one"
        );
        assert_eq!(
            starts,
            HashSet::from(["m1".to_string(), "a1".to_string(), "a3".to_string()])
        );
    }

    #[test]
    fn my_own_messages_group_too_and_a_dm_groups_its_discs() {
        let messages = [
            person("m1", "me", "one", T),
            person("m2", "me", "two", T + 5_000),
            person("m3", "bob", "hey", T + 10_000),
            person("m4", "bob", "you there?", T + 20_000),
        ];
        // A DM names nobody but still shows the disc once per run.
        let (rows, starts) = rows_of(&messages, false);
        assert_eq!(
            shape(&rows),
            [
                ("mine".into(), false, false),
                ("mine".into(), false, false),
                ("hey".into(), false, true),
                ("you there?".into(), false, false),
            ]
        );
        assert_eq!(
            starts,
            HashSet::from(["m1".to_string(), "m3".to_string()]),
            "my second line sits in the in-run gap; Bob's run opens the turn gap once"
        );
    }

    #[test]
    fn an_agent_in_its_own_chat_groups_its_bubbles() {
        // 1:1 agent chat (no lane): a quick ack and the answer within the window are one
        // run — one turn gap, then 6 pt (supersedes #1447's gap on every agent message).
        let messages = [
            message("m1", "user", "user", "chart the weather", T),
            message("a1", "assistant", "box:1", "On it.", T + 5_000),
            message("a2", "assistant", "box:1", "Here is the chart.", T + 60_000),
            message(
                "a3",
                "assistant",
                "box:1",
                "Late addendum.",
                T + 60_000 + RUN_WINDOW_MS + 1,
            ),
        ];
        let (rows, starts) = rows_of(&messages, false);
        assert_eq!(
            shape(&rows),
            [
                ("mine".into(), false, false),
                ("agent-1:1".into(), false, true),
                ("agent-1:1".into(), false, false),
                ("agent-1:1".into(), false, true),
            ]
        );
        assert_eq!(
            starts,
            HashSet::from(["m1".to_string(), "a1".to_string(), "a3".to_string()])
        );
    }

    #[test]
    fn the_row_cache_keeps_run_starts_on_a_hit() {
        let messages = [
            person("m1", "bob", "a", T),
            person("m2", "bob", "b", T + 1_000),
            person("m3", "sam", "c", T + 2_000),
        ];
        let (spoken, failed) = (HashSet::new(), HashMap::new());
        let speech = SpeechRows {
            enabled: false,
            spoken: &spoken,
            failed: &failed,
            voice_mode: false,
        };
        let mut cache = MessageRowCache::default();
        let build = |cache: &mut MessageRowCache| {
            build_rows_with_cache(
                &messages,
                false,
                speech,
                "gh_me",
                &HashSet::new(),
                &Mentions::empty(),
                true,
                true,
                true,
                cache,
                &HashSet::new(),
            )
        };
        let first = build(&mut cache);
        assert_eq!(cache.built, 3);
        let second = build(&mut cache);
        assert_eq!(cache.built, 0, "every message served from the cache");
        assert_eq!(shape(&first), shape(&second));
        assert_eq!(
            cache.run_starts,
            HashSet::from(["m1".to_string(), "m3".to_string()]),
            "run starts are rebuilt from the cached entries"
        );
    }
}

#[cfg(test)]
mod secret_handoff_rows_tests {
    //! Secret hand-off (Jacob 2026-10-26): a redacted user message draws as the ghost row
    //! "Secret saved as NAME" — mine on the right, another person's on the left under their
    //! name — and never as a text bubble; the row carries no value.
    use std::collections::{HashMap, HashSet};

    use super::*;
    use crate::model::Message;

    fn redacted(sender: &str, name: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": format!("m-{at}"),
            "chatId": "chat_testing",
            "role": "user",
            "sender": sender,
            "senderHandle": if sender.starts_with("user:") { "jacob" } else { "" },
            "senderName": if sender.starts_with("user:") { "Jacob Steeves" } else { "" },
            "status": "done",
            "createdAt": at,
            "redactedAt": at + 5_000,
            "parts": [{ "id": format!("p-{at}"), "kind": "secret_redacted", "name": name, "env": name,
                "secretId": "s1", "savedBy": sender, "agent": "p1" }],
        }))
        .expect("message")
    }

    fn rows_of(messages: &[Message], me: &str) -> Vec<Row> {
        let spoken = HashSet::new();
        let failed = HashMap::new();
        build_rows(
            messages,
            false,
            SpeechRows {
                enabled: false,
                spoken: &spoken,
                failed: &failed,
                voice_mode: false,
            },
            me,
            &HashSet::new(),
            &Mentions::empty(),
            false,
        )
    }

    #[test]
    fn my_redacted_message_is_the_ghost_row_on_my_side() {
        let t = 1_700_000_000_000u64;
        let rows = rows_of(&[redacted("user:gh_me", "STRIPE_KEY", t)], "gh_me");
        let user: Vec<_> = rows
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::User {
                    text, ghost, from, ..
                } => Some((text.clone(), ghost.clone(), from.is_some())),
                _ => None,
            })
            .collect();
        assert_eq!(
            user,
            [(
                "Secret saved as STRIPE_KEY".to_string(),
                Some("STRIPE_KEY".to_string()),
                false
            )]
        );
    }

    #[test]
    fn another_members_redacted_message_keeps_their_lane_and_names_them() {
        let t = 1_700_000_000_000u64;
        let rows = rows_of(&[redacted("user:gh_jacob", "STRIPE_KEY", t)], "gh_me");
        let row = rows
            .iter()
            .find_map(|r| match &r.kind {
                RowKind::User { ghost, from, .. } => Some((ghost.clone(), from.clone())),
                _ => None,
            })
            .expect("a user row");
        assert_eq!(row.0.as_deref(), Some("STRIPE_KEY"));
        let from = row.1.expect("the other person's lane");
        assert_eq!(from.display_name(), "Jacob Steeves");
    }

    #[test]
    fn the_part_decodes_with_names_only_and_an_older_build_ignores_it() {
        let m = redacted("user:gh_me", "STRIPE_KEY", 1);
        assert!(m.plain_text().is_empty(), "nothing to copy or search");
        match &m.parts[0] {
            Part::SecretRedacted {
                name,
                env,
                secret_id,
                saved_by,
                agent,
                ..
            } => {
                assert_eq!((name.as_str(), env.as_str()), ("STRIPE_KEY", "STRIPE_KEY"));
                assert_eq!(secret_id, "s1");
                assert_eq!(saved_by, "user:gh_me");
                assert_eq!(agent.as_deref(), Some("p1"));
            }
            other => panic!("{other:?}"),
        }
        let v = serde_json::to_value(&m.parts[0]).unwrap();
        assert!(v.get("value").is_none());
    }
}

#[cfg(test)]
mod presence_rows_tests {
    //! [feedback typing-indicator-left-2026-10-23]: who is working lives in the
    //! presence row over the composer, so the transcript never doubles it — no
    //! "<name> is working" row, and no sender caption over a stream that has no
    //! words yet (Jacob's "GigChadAgent" + "gigchadagent is working" pair).
    use std::collections::{HashMap, HashSet};

    use super::*;

    fn agent_message(status: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": format!("m-{at}"),
            "chatId": "chat_testing",
            "role": "assistant",
            "sender": "agent:01PROJGIGCHAD",
            "senderName": "GigChadAgent",
            "senderHandle": "gigchadagent",
            "askedByHandle": "jacob",
            "status": status,
            "createdAt": at,
            "parts": if text.is_empty() {
                serde_json::json!([{ "id": format!("s-{at}"), "kind": "status", "code": "working", "message": "Working…" }])
            } else {
                serde_json::json!([{ "id": format!("p-{at}"), "kind": "text", "text": text }])
            },
        }))
        .expect("message")
    }

    fn rows_of(messages: &[Message]) -> Vec<Row> {
        let spoken = HashSet::new();
        let failed = HashMap::new();
        let speech = SpeechRows {
            enabled: false,
            spoken: &spoken,
            failed: &failed,
            voice_mode: false,
        };
        build_rows(
            messages,
            false,
            speech,
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            true,
        )
    }

    fn reply_row(id: &str, ask: &str, status: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "chatId": "chat_testing",
            "role": "assistant",
            "sender": "agent:01PROJGIGCHAD",
            "senderName": "GigChadAgent",
            "inReplyTo": ask,
            "status": status,
            "createdAt": at,
            "parts": if text.is_empty() {
                serde_json::json!([
                    { "id": format!("t-{id}"), "kind": "text", "text": "" },
                    { "id": format!("s-{id}"), "kind": "status", "code": "working", "message": "Working…" }
                ])
            } else {
                serde_json::json!([{ "id": format!("p-{id}"), "kind": "text", "text": text }])
            },
        }))
        .expect("message")
    }

    /// Jacob, "run bubble sort" (2026-10-25 18:08 UTC): the answer went out as a `send_message`
    /// bubble, the fresh result row it opened ended wordless and Done, and the transcript drew
    /// "The agent didn't answer. Ask again." under the answer. A turn that answered elsewhere
    /// never gets the stock line; a turn that said nothing still does.
    #[test]
    fn no_stock_line_when_the_turn_answered_in_another_row() {
        let now = now_ms();
        let answered = [
            reply_row("r1", "u1", "done", "Sorted: 1, 2, 3, 5, 8.", now),
            reply_row("r2", "u1", "done", "", now + 1),
        ];
        let rows = rows_of(&answered);
        assert!(
            rows.iter()
                .all(|r| !matches!(r.kind, RowKind::NoAnswer { .. })),
            "no 'didn't answer' under an answer: {:?}",
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>()
        );
        let silent = [reply_row("r3", "u2", "done", "", now + 2)];
        let rows = rows_of(&silent);
        assert!(
            rows.iter()
                .any(|r| matches!(r.kind, RowKind::NoAnswer { .. })),
            "a turn that said nothing keeps the stock line"
        );
    }

    #[test]
    fn a_wordless_stream_adds_no_caption_and_no_working_row() {
        let now = now_ms();
        let rows = rows_of(&[agent_message("streaming", "", now)]);
        assert!(
            rows.iter().all(|r| !matches!(
                r.kind,
                RowKind::Markdown { named: true, .. } | RowKind::NoAnswer { named: true, .. }
            )),
            "no sender name without a bubble: {:?}",
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>()
        );
        assert!(
            rows.iter().all(|r| r.id != "chat#indicator"),
            "the working line is the presence row, not a transcript row"
        );
    }

    #[test]
    fn the_caption_returns_with_the_first_words() {
        let now = now_ms();
        let rows = rows_of(&[agent_message(
            "streaming",
            "On it — pulling the numbers.",
            now,
        )]);
        assert_eq!(
            rows.iter()
                .filter(|r| matches!(r.kind, RowKind::Markdown { named: true, .. }))
                .count(),
            1,
            "one name, inside the first bubble"
        );
    }

    /// What agentd writes for a background job (duplex §2.3): wordless, hidden.
    fn hidden_job_row(id: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "chatId": "chat_testing", "role": "assistant", "sender": "agent:01PROJGIGCHAD",
            "status": "streaming", "createdAt": at, "hidden": true, "worker": true,
            "job": { "id": format!("j-{id}"), "title": "Searching", "kind": "research", "touches": [] },
            "parts": [{ "id": format!("s-{id}"), "kind": "status", "code": "working", "message": "Working…" }],
        }))
        .expect("message")
    }

    fn stamps(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .filter(|r| matches!(r.kind, RowKind::Stamp { .. }))
            .map(|r| r.id.clone())
            .collect()
    }

    /// [feedback status-row-gap-mac-2026-10-26]: a row that draws nothing — a stream
    /// with no words yet, a hidden job row — an hour after the last bubble used to
    /// leave its "Today 5:32 PM" stamp hanging over empty space above the status
    /// row. The stamp comes with the first words, as it would have anyway.
    #[test]
    fn a_wordless_row_an_hour_later_brings_no_dangling_stamp() {
        let now = now_ms();
        let earlier = now - 2 * STAMP_GAP_MS;
        let settled = [
            agent_message(
                "done",
                "Three decisions: keep the canvas infinite.",
                earlier,
            ),
            agent_message("streaming", "", now),
        ];
        let rows = rows_of(&settled);
        assert_eq!(
            stamps(&rows),
            vec![format!("m-{earlier}#stamp")],
            "only the bubble's own stamp: {:?}",
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>()
        );
        assert!(
            matches!(rows[rows.len() - 1].kind, RowKind::BottomInset)
                && !matches!(rows[rows.len() - 2].kind, RowKind::Stamp { .. }),
            "nothing but the clearance after the last bubble"
        );

        let with_job = [
            agent_message(
                "done",
                "Three decisions: keep the canvas infinite.",
                earlier,
            ),
            hidden_job_row("j1", now),
        ];
        assert_eq!(
            stamps(&rows_of(&with_job)),
            vec![format!("m-{earlier}#stamp")],
            "a hidden job row leaves no stamp either"
        );

        // The reply lands: its stamp arrives with its bubble, and the sender run
        // the retracted stamp would have broken is unchanged for later rows.
        let landed = [
            agent_message(
                "done",
                "Three decisions: keep the canvas infinite.",
                earlier,
            ),
            agent_message("done", "Here is the chart.", now),
        ];
        let rows = rows_of(&landed);
        assert_eq!(
            stamps(&rows),
            vec![format!("m-{earlier}#stamp"), format!("m-{now}#stamp")],
            "the reply's stamp comes with the reply"
        );
        let reply_ix = rows
            .iter()
            .position(|r| r.id == format!("m-{now}#stamp"))
            .expect("stamp");
        assert!(
            matches!(rows[reply_ix + 1].kind, RowKind::Markdown { .. }),
            "the bubble sits right under its stamp"
        );
    }
}

#[cfg(test)]
mod delivery_tests {
    //! Jacob, 2026-10-24 ("remove these 'sent' items, just use coloring") and
    //! 2026-10-25 ("my messages show up too dim, I think they have not been
    //! sent"): no caption on my bubbles; a delivered bubble is the full style
    //! at once, whatever read receipts say; only pending is dimmed; a failed
    //! one is full with a red "! Retry". Rule: docs/mac-design-rules.md.
    use std::collections::{HashMap, HashSet};

    use gpui::TestAppContext;

    use super::*;

    fn message(
        id: &str,
        role: &str,
        sender: &str,
        status: &str,
        pending: bool,
        at: u64,
    ) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "chatId": "chat_group",
            "role": role,
            "sender": sender,
            "senderHandle": if sender.starts_with("user:") { "sam" } else { "" },
            "status": status,
            "pending": pending,
            "createdAt": at,
            "parts": [{ "id": format!("p-{id}"), "kind": "text", "text": "I did lol" }],
        }))
        .expect("message")
    }

    fn mine(id: &str, at: u64) -> Message {
        message(id, "user", "user:gh_me", "done", false, at)
    }

    #[test]
    fn only_pending_is_dimmed() {
        assert_eq!(Delivery::Delivered.opacity(), 1.0);
        assert_eq!(Delivery::Failed.opacity(), 1.0);
        assert_eq!(Delivery::Pending.opacity(), BUBBLE_ALPHA_PENDING);
        assert!(Delivery::Pending.opacity() < Delivery::Delivered.opacity());
    }

    #[test]
    fn an_acked_bubble_is_delivered_with_or_without_receipts_or_replies() {
        let t = 1_700_000_000_000u64;
        // Jacob's Arbonites shape (2026-10-25 22:11–22:17): two of mine, an
        // agent reply after the second, nobody's read receipt anywhere.
        let messages = vec![
            mine("a", t),
            mine("b", t + 1_000),
            message("r", "assistant", "agent:hezbola", "done", false, t + 2_000),
            mine("c", t + 3_000),
        ];
        for m in &messages {
            if m.role == Role::User {
                assert_eq!(
                    delivery_state(m, true),
                    Some(Delivery::Delivered),
                    "{}: delivered is the full style at once",
                    m.id
                );
            }
        }
    }

    #[test]
    fn pending_and_failed_are_the_only_other_states_and_others_have_none() {
        let t = 1_700_000_000_000u64;
        let pending = message("p", "user", "user:gh_me", "queued", true, t);
        let failed = message("f", "user", "user:gh_me", "error", false, t + 1);
        let sent = mine("s", t + 2);
        assert_eq!(delivery_state(&pending, true), Some(Delivery::Pending));
        assert_eq!(delivery_state(&failed, true), Some(Delivery::Failed));
        assert_eq!(delivery_state(&sent, true), Some(Delivery::Delivered));
        assert_eq!(
            delivery_state(&sent, false),
            None,
            "someone else's bubble: no state"
        );
    }

    #[test]
    fn build_rows_puts_the_state_on_my_bubbles_only() {
        let t = 1_700_000_000_000u64;
        let messages = vec![
            mine("a", t),
            message("o", "user", "user:gh_sam", "done", false, t + 1_000),
            message("p", "user", "user:gh_me", "queued", true, t + 2_000),
        ];
        let spoken = HashSet::new();
        let failed = HashMap::new();
        let speech = SpeechRows {
            enabled: false,
            spoken: &spoken,
            failed: &failed,
            voice_mode: false,
        };
        let rows = build_rows(
            &messages,
            false,
            speech,
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            false,
        );
        let states: Vec<Option<Delivery>> = rows
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::User { delivery, .. } => Some(*delivery),
                _ => None,
            })
            .collect();
        assert_eq!(
            states,
            vec![Some(Delivery::Delivered), None, Some(Delivery::Pending)],
            "mine: delivered / pending; Sam's bubble: no state"
        );
    }

    #[test]
    fn row_cache_returns_cached_rows_and_rebuilds_only_the_changed_message() {
        let t = 1_700_000_000_000u64;
        let mut messages = vec![
            mine("a", t),
            mine("b", t + 1_000),
            message("c", "assistant", "agent:x", "streaming", false, t + 2_000),
        ];
        // Give the streaming reply some text so it draws a bubble.
        if let Some(Part::Text { text, .. }) = messages[2].parts.get_mut(0) {
            *text = "hel".into();
        }
        let spoken = HashSet::new();
        let failed = HashMap::new();
        let speech = SpeechRows {
            enabled: false,
            spoken: &spoken,
            failed: &failed,
            voice_mode: false,
        };
        let mut cache = MessageRowCache::default();
        let first = build_rows_with_cache(
            &messages,
            false,
            speech,
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            false,
            false,
            true,
            &mut cache,
            &HashSet::new(),
        );
        assert_eq!(cache.built, 3, "cold build walks every message");

        let speech = SpeechRows {
            enabled: false,
            spoken: &spoken,
            failed: &failed,
            voice_mode: false,
        };
        let again = build_rows_with_cache(
            &messages,
            false,
            speech,
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            false,
            false,
            true,
            &mut cache,
            &HashSet::new(),
        );
        assert_eq!(
            cache.built, 0,
            "unchanged messages are served from the cache"
        );
        let ids = |rows: &[Row]| -> Vec<(String, u64)> {
            rows.iter().map(|r| (r.id.clone(), r.version)).collect()
        };
        assert_eq!(ids(&first), ids(&again));

        // Streaming upsert: only the last message's text changes.
        if let Some(Part::Text { text, .. }) = messages[2].parts.get_mut(0) {
            *text = "hello".into();
        }
        let speech = SpeechRows {
            enabled: false,
            spoken: &spoken,
            failed: &failed,
            voice_mode: false,
        };
        let updated = build_rows_with_cache(
            &messages,
            false,
            speech,
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            false,
            false,
            true,
            &mut cache,
            &HashSet::new(),
        );
        assert_eq!(
            cache.built, 1,
            "a streaming upsert rebuilds only the changed message"
        );
        assert_ne!(ids(&again), ids(&updated));
    }

    /// The three states as the row draws them: one bubble per state, no
    /// caption anywhere, and the red "! Retry" only beside a failed bubble.
    struct OwnBubbles;

    impl gpui::Render for OwnBubbles {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::dark(crate::ui::theme::Surface::Matte);
            let bubble = |text: &'static str, d: Delivery| {
                div()
                    .debug_selector(move || {
                        match d {
                            Delivery::Pending => "own-bubble-pending",
                            Delivery::Failed => "own-bubble-failed",
                            Delivery::Delivered => "own-bubble-delivered",
                        }
                        .into()
                    })
                    .px(px(BUBBLE_PAD_X))
                    .py(px(BUBBLE_PAD_Y))
                    .rounded(px(BUBBLE_RADIUS))
                    .bg(theme.bubble)
                    .text_color(theme.bubble_text())
                    .child(text)
            };
            div()
                .w(px(600.0))
                .h(px(400.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .children(
                    [Delivery::Pending, Delivery::Failed, Delivery::Delivered]
                        .into_iter()
                        .map(|d| {
                            div()
                                .flex()
                                .justify_end()
                                .items_center()
                                .children(
                                    (d == Delivery::Failed)
                                        .then(|| failed_glyph(&theme).mr(px(8.0))),
                                )
                                .children(
                                    (d == Delivery::Pending)
                                        .then(|| pending_glyph(&theme).mr(px(8.0))),
                                )
                                .child(own_bubble(bubble("fish?", d), d))
                        }),
                )
        }
    }

    #[gpui::test]
    fn every_state_draws_a_bubble_and_only_failed_gets_the_retry(cx: &mut TestAppContext) {
        let (_view, vcx) = cx.add_window_view(|_, _| OwnBubbles);
        vcx.run_until_parked();
        for sel in [
            "own-bubble-pending",
            "own-bubble-failed",
            "own-bubble-delivered",
        ] {
            vcx.debug_bounds(sel)
                .unwrap_or_else(|| panic!("{sel} drawn"));
        }
        assert!(
            vcx.debug_bounds("bubble-receipt").is_none(),
            "no sent/seen caption anywhere"
        );
        let glyph = vcx
            .debug_bounds("own-bubble-failed-glyph")
            .expect("red mark + Retry beside the failed bubble");
        let failed = vcx
            .debug_bounds("own-bubble-failed")
            .expect("failed bubble");
        assert!(
            f32::from(glyph.origin.x + glyph.size.width) <= f32::from(failed.origin.x),
            "the mark sits outside the bubble, on its outer edge"
        );
        let clock = vcx
            .debug_bounds("own-bubble-pending-glyph")
            .expect("clock beside the pending bubble");
        let pending = vcx
            .debug_bounds("own-bubble-pending")
            .expect("pending bubble");
        assert!(
            f32::from(clock.origin.x + clock.size.width) <= f32::from(pending.origin.x),
            "the clock sits outside the pending bubble"
        );
    }
}

#[cfg(test)]
mod reply_in_dm_tests {
    //! Reply in DM (#1835, Jacob 2026-10-25): a DM bubble that answers a message in
    //! another chat draws the quote header plus "Replying to <who> in <chat>", and the
    //! click opens that chat on the original — for a member; a non-member sees the
    //! words and nothing to follow.
    use std::collections::{HashMap, HashSet};

    use futures::future::BoxFuture;
    use gpui::{point, Bounds, Entity, Modifiers, Pixels, TestAppContext};
    use tokio::sync::mpsc;

    use super::*;
    use crate::app::AppState;
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::motion::PulseClock;
    use crate::ui::shell::Shell;

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

    const GROUP: &str = "01GROUPARBONITES0000000000";
    const DM: &str = "01DMWITHSAM00000000000000";
    const ORIGINAL: &str = "01ORIGINALMSG000000000000";

    fn row(chat: &str, id: &str, sender: &str, handle: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "chatId": chat, "role": "user", "sender": sender,
            "senderHandle": handle, "status": "done", "createdAt": at, "completedAt": at,
            "parts": [{ "id": format!("{id}-t"), "kind": "text", "text": text }],
        }))
        .expect("message")
    }

    fn reply_ref(chat_id: &str, chat_title: &str, message_id: &str, text: &str) -> ReplyRef {
        ReplyRef {
            chat_id: chat_id.into(),
            project_id: "people".into(),
            message_id: message_id.into(),
            chat_title: chat_title.into(),
            sender: "user:gh_1".into(),
            sender_name: "Jacob".into(),
            sender_handle: "unconst".into(),
            text: text.into(),
        }
    }

    /// A server channel's member row names its sender the way a group's does — by display
    /// name, from the roster-filled `senderName` — and only falls back to the handle when
    /// no name is known ([feedback server-member-names-2026-10-28]; Affinity: "referred to
    /// as our handles instead of our usernames in server").
    #[test]
    fn a_channel_member_row_renders_the_display_name_not_the_handle() {
        let t = 1_700_000_000_000u64;
        let channel = "!general:arbos.life";
        let mut named = row(channel, "c1", "user:test_01scout", "A", "yo", t);
        named.sender_name = Some("Scout".into());
        let nameless = row(channel, "c2", "user:test_01z", "Z", "me", t + 1_000);
        let (spoken, failed) = (HashSet::new(), HashMap::new());
        let rows = build_rows(
            &[named, nameless],
            false,
            SpeechRows {
                enabled: false,
                spoken: &spoken,
                failed: &failed,
                voice_mode: false,
            },
            "test_01ME",
            &HashSet::new(),
            &Mentions::empty(),
            true,
        );
        let labels: Vec<(String, bool)> = rows
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::User {
                    from: Some(p),
                    named,
                    ..
                } => Some((p.display_name(), *named)),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels,
            vec![("Scout".to_string(), true), ("Z".to_string(), true)],
            "Scout's bubble is labelled Scout; a member with no known name keeps the handle"
        );
    }

    /// `build_rows` alone: a `replyRef` row quotes its snapshot and names the chat; a
    /// `replyTo` row quotes the in-chat original and names nothing.
    #[test]
    fn build_rows_carries_the_quote_and_the_chat_name() {
        let t = 1_700_000_000_000u64;
        let mut dm_reply = row(DM, "d1", "user:gh_2", "sam", "Works", t + 1_000);
        dm_reply.reply_ref = Some(reply_ref(
            GROUP,
            "Arbonites",
            ORIGINAL,
            "Testing reply in dm — does the quote survive?",
        ));
        let mut in_chat = row(DM, "d2", "user:gh_me", "me", "and this?", t + 2_000);
        in_chat.reply_to = Some("d1".into());
        let messages = [
            row(DM, "d0", "user:gh_me", "me", "hi", t),
            dm_reply,
            in_chat,
        ];
        let (spoken, failed) = (HashSet::new(), HashMap::new());
        let rows = build_rows(
            &messages,
            false,
            SpeechRows {
                enabled: false,
                spoken: &spoken,
                failed: &failed,
                voice_mode: false,
            },
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            false,
        );
        let quotes: Vec<Option<Quote>> = rows
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::User { quote, .. } => Some(quote.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(quotes.len(), 3);
        assert!(quotes[0].is_none());
        let cross = quotes[1].clone().expect("the DM reply quotes");
        assert_eq!(cross.message_id, ORIGINAL);
        assert_eq!(cross.text, "Testing reply in dm — does the quote survive?");
        assert_eq!(
            cross.source.as_ref().map(|s| s.label.as_str()),
            Some("Replying to Jacob in Arbonites")
        );
        assert_eq!(
            cross.source.as_ref().map(|s| s.chat_id.as_str()),
            Some(GROUP)
        );
        let local = quotes[2].clone().expect("the in-chat reply quotes");
        assert_eq!(local.message_id, "d1");
        assert_eq!(local.text, "Works");
        assert!(local.source.is_none(), "an in-chat reply names no chat");
    }

    /// Me (`gh_me`) in the Arbonites group and in a DM with Sam. Sam's DM row
    /// answers `ORIGINAL` in the group; a second row answers a chat I am not in.
    fn mount(
        cx: &mut TestAppContext,
    ) -> (
        tokio::runtime::Runtime,
        Entity<AppState>,
        Entity<Shell>,
        &mut gpui::VisualTestContext,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        let now = now_ms();
        state.update(cx, |s, cx| {
            let sam = Person {
                id: "gh_2".into(),
                handle: "sam".into(),
                name: Some("Sam Rivera".into()),
                ..Person::default()
            };
            s.projects.push(Project {
                id: GROUP.into(),
                name: "Arbonites".into(),
                kind: SpaceKind::Group,
                people: vec![sam.clone()],
                ..Project::default()
            });
            s.projects.push(Project {
                id: DM.into(),
                name: "sam".into(),
                kind: SpaceKind::Dm,
                people: vec![sam],
                ..Project::default()
            });
            let chat = |id: &str, title: &str| -> Chat {
                serde_json::from_value(serde_json::json!({
                    "id": id, "project_id": id, "title": title, "is_pinned": true,
                    "is_system": false, "unread": false, "last_message_at": now,
                }))
                .expect("chat")
            };
            s.chats.insert(GROUP.into(), vec![chat(GROUP, "Arbonites")]);
            s.chats.insert(DM.into(), vec![chat(DM, "sam")]);
            s.messages.insert(
                GROUP.into(),
                vec![
                    row(
                        GROUP,
                        "g0",
                        "user:gh_2",
                        "sam",
                        "morning all",
                        now - 120_000,
                    ),
                    row(
                        GROUP,
                        ORIGINAL,
                        "user:gh_me",
                        "unconst",
                        "Testing reply in dm",
                        now - 90_000,
                    ),
                ],
            );
            let mut sams = row(DM, "d1", "user:gh_2", "sam", "Works", now - 60_000);
            sams.reply_ref = Some(reply_ref(
                GROUP,
                "Arbonites",
                ORIGINAL,
                "Testing reply in dm",
            ));
            let mut outside = row(DM, "d2", "user:gh_2", "sam", "Same in ours", now - 30_000);
            outside.reply_ref = Some(reply_ref(
                "01NOTMYCHAT0000000000000000",
                "Design crit",
                "01NOTMYMSG00000000000000000",
                "The tiers are stale",
            ));
            s.messages.insert(DM.into(), vec![sams, outside]);
            s.selected_project = Some(DM.into());
            s.active_tab.insert(GROUP.into(), GROUP.into());
            s.active_tab.insert(DM.into(), DM.into());
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.run_until_parked();
        settle(&state, &shell, vcx);
        (rt, state, shell, vcx)
    }

    /// The transcript syncs on state notifications; give it (and the shell) a few frames.
    fn settle(state: &Entity<AppState>, shell: &Entity<Shell>, vcx: &mut gpui::VisualTestContext) {
        for _ in 0..3 {
            state.update(vcx, |_, cx| cx.notify());
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    fn centre(b: Bounds<Pixels>) -> gpui::Point<Pixels> {
        point(b.left() + b.size.width / 2.0, b.top() + b.size.height / 2.0)
    }

    #[gpui::test]
    fn dm_bubble_draws_the_quote_with_its_chat_and_the_click_opens_the_original(
        cx: &mut TestAppContext,
    ) {
        let (_rt, state, shell, vcx) = mount(cx);
        // Both DM rows draw a quote with a source line; the transcript is the DM.
        let drawn: Vec<(String, Option<String>, Option<String>)> =
            shell.read_with(&*vcx, |sh, cx| {
                sh.transcript
                    .read(cx)
                    .rows
                    .iter()
                    .filter_map(|r| match &r.kind {
                        RowKind::User { quote: Some(q), .. } => Some((
                            r.message_id.clone(),
                            q.source.as_ref().map(|s| s.label.clone()),
                            Some(q.text.clone()),
                        )),
                        _ => None,
                    })
                    .collect()
            });
        assert_eq!(
            drawn,
            vec![
                (
                    "d1".into(),
                    Some("Replying to Jacob in Arbonites".into()),
                    Some("Testing reply in dm".into())
                ),
                (
                    "d2".into(),
                    Some("Replying to Jacob in Design crit".into()),
                    Some("The tiers are stale".into())
                ),
            ]
        );
        let first = vcx
            .debug_bounds("reply-quote-d1#user")
            .expect("Sam's first quote header is on screen");
        assert!(vcx.debug_bounds("reply-quote-source-d1#user").is_some());
        assert!(vcx.debug_bounds("reply-quote-d2#user").is_some());
        assert!(vcx.debug_bounds("reply-quote-source-d2#user").is_some());
        // Membership decides the link: the group is mine, Design crit is not.
        let (member, outsider) = state.read_with(&*vcx, |s, _| {
            (
                s.is_member_of_chat(GROUP),
                s.is_member_of_chat("01NOTMYCHAT0000000000000000"),
            )
        });
        assert!(member && !outsider);

        // Click the first quote: the group opens on the original, highlighted.
        vcx.simulate_click(centre(first), Modifiers::default());
        vcx.run_until_parked();
        let (space, tab, reveal, highlight) = state.read_with(&*vcx, |s, _| {
            (
                s.selected_project.clone(),
                s.active_tab.get(GROUP).cloned(),
                s.reveal_message.clone(),
                s.highlight_message.clone(),
            )
        });
        assert_eq!(space.as_deref(), Some(GROUP), "the source chat is open");
        assert_eq!(tab.as_deref(), Some(GROUP));
        assert_eq!(reveal, Some((GROUP.to_owned(), ORIGINAL.to_owned())));
        assert_eq!(highlight.as_deref(), Some(ORIGINAL));

        // Back in the DM, the outsider quote is words only: the click goes nowhere.
        state.update(vcx, |s, cx| s.select_project(DM.into(), cx));
        settle(&state, &shell, vcx);
        let second = vcx
            .debug_bounds("reply-quote-d2#user")
            .expect("the outsider quote is on screen");
        vcx.simulate_click(centre(second), Modifiers::default());
        vcx.run_until_parked();
        let (space, reveal) = state.read_with(&*vcx, |s, _| {
            (s.selected_project.clone(), s.reveal_message.clone())
        });
        assert_eq!(
            space.as_deref(),
            Some(DM),
            "a non-member stays where they are"
        );
        assert_eq!(reveal, None);
        // And the model agrees without a click.
        let moved = state.update(vcx, |s, cx| {
            s.reveal_reply_ref(
                &reply_ref("01NOTMYCHAT0000000000000000", "Design crit", "x", ""),
                cx,
            )
        });
        assert!(!moved);
    }
}
#[cfg(test)]
mod crash_probe_tests {
    //! Mac 0.6.268, 2026-10-26: the rows of Jacob's two crash windows (the Affine chat's empty-sender
    //! SVG-attachment rows, the Arbonites table / markdown sample / roleplay replies) in a group
    //! transcript, then the local actions he could have taken — scroll stops, a pointer sweep, clicks
    //! everywhere. This is what found `rail_centre`'s clamp panic; it stays as the guard.
    use futures::future::BoxFuture;
    use gpui::TestAppContext;
    use tokio::sync::mpsc;

    use super::*;
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use gpui::{point, Modifiers};

    const ARBONITES: &[(&str, &str)] = &[
        ("user", "@hezbola give me a random markdown table"),
        ("assistant", "| Fruit | Colour | Weight (g) |\n| --- | --- | --- |\n| Apple | Red | 180 |\n| Banana | Yellow | 120 |\n| Kiwi | Green | 75 |"),
        ("user", "show me a couple things"),
        ("assistant", "Here's a sample, Arbos.\n\n**Bold**, *italic*, and `inline code`.\n\n- A bullet list\n- With two items\n\n> A block quote.\n\n[Eater Austin's Austin guide](https://austin.eater.com/maps/best-restaurants-austin-eater-38)"),
        ("user", "@doug-agent @hezbola  can you two engage in a 10 turn roleplay dialoge. doug is a product manager, and hezbola is a senior engineer."),
        ("assistant", "Yes, Arbos, with some limits. I don't have matplotlib installed, but Pillow is there, so I can draw bar, line and pie charts as PNG images and hand them over as files."),
        ("user", "Happy to, Affinity. Turn 1, PM hat on.\n\n@hezbola — kicking off a new prototype: a Sakata cracker stacking game, built in Blender. Scope for v0: a tower of rice crackers you drop one at a time, physics on, game over when one slides off."),
        ("assistant", "Turn 2, engineer hat on. Doug, here's the cracker asset plan.\n\nGeometry: a 60 mm diameter disc, about 6 mm thick. The ridged edge comes from a displaced ring on the rim, and the sesame speckle is a small-scale bump texture, not extra geometry.\n\nPhysics: mass about 8 g, friction 0.6, bounce 0.05, and the collision margin set small so stacks settle.\n\nDrop: spawn the cracker 20 cm above the stack, with a small random tilt and offset so it doesn't land perfectly flat every time.\n\nI haven't built this in Blender yet. Should I start the scene script, or do you want the asset spec changed first?"),
    ];

    struct Idle;
    impl SyncSource for Idle {
        fn name(&self) -> &'static str {
            "idle"
        }
        fn run(
            self: Box<Self>,
            _e: mpsc::UnboundedSender<SyncEvent>,
            _c: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            Box::pin(std::future::pending())
        }
    }
    struct Host {
        transcript: Entity<Transcript>,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.transcript.clone())
        }
    }

    #[gpui::test]
    fn crash_window_rows_survive_scroll_hover_and_clicks(cx: &mut TestAppContext) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        let t = 1_700_000_000_000u64;
        let rows: Vec<(String, String, String)> = ARBONITES
            .iter()
            .map(|(k, t)| (String::new(), (*k).to_owned(), (*t).to_owned()))
            .collect();
        let mut messages: Vec<Message> = Vec::new();
        let mut at = t;
        for (i, (_, kind, text)) in rows.iter().enumerate() {
            at += 10_000;
            let role = if kind == "user" { "user" } else { "assistant" };
            messages.push(serde_json::from_value(serde_json::json!({
                "id": format!("m{i}"), "chatId": "g1", "role": role,
                "sender": if role == "user" { "user:gh_1" } else { "agent:01M5NNT5FNTZNCT3THWNVEW8WM" },
                "senderName": if role == "user" { "Affinity" } else { "Hezbola" },
                "senderHandle": if role == "user" { "affinity" } else { "hezbola" },
                "status": "done", "createdAt": at, "completedAt": at + 1_000,
                "parts": [{ "id": format!("m{i}-t"), "kind": "text", "text": text }],
            })).unwrap());
        }
        // (1) a hidden job-worker row: empty agent sender carrying an SVG attachment of kind image
        at += 10_000;
        messages.push(serde_json::from_value(serde_json::json!({
            "id": "w1", "chatId": "g1", "role": "assistant", "sender": "agent:01M5N1WSR91G8CJKNYXKJESK7X",
            "senderName": "", "senderHandle": "", "worker": true, "hidden": true,
            "status": "done", "createdAt": at, "completedAt": at + 1_000,
            "parts": [{ "id": "w1-a", "kind": "attachment", "attachment": { "name": "affine-favicon.svg", "mime": "image/svg+xml", "ref": "blob-svg-1", "bytes": 4031, "kind": "image", "title": "affine.io logo" } }],
        })).unwrap());
        // (2) the same shape as a visible row with sender {kind:"agent", name:""} and an SVG image
        at += 10_000;
        messages.push(serde_json::from_value(serde_json::json!({
            "id": "w2", "chatId": "g1", "role": "assistant", "sender": "agent:01M5N1WSR91G8CJKNYXKJESK7X",
            "senderName": "", "senderHandle": "", "senderAvatar": "", "senderAvatarUrl": "",
            "status": "done", "createdAt": at, "completedAt": at + 1_000,
            "parts": [
                { "id": "w2-t", "kind": "text", "text": "I found the AFFiNE logo from affine.io. You can get the official SVG directly at [affine.io/favicon.svg](https://affine.io/favicon.svg) (a 1000×1000 vector mark), and I have also saved and attached a copy for you." },
                { "id": "w2-a", "kind": "attachment", "attachment": { "name": "affine-favicon.svg", "mime": "image/svg+xml", "ref": "blob-svg-2", "kind": "image" } }
            ],
        })).unwrap());
        // Mac-only shapes bc-3790a460 listed (`internal/mac-launch-crash-recovery-2026-10-26.md`
        // §2): a user attachment whose upload failed (part without `ref`, still pending), a
        // voice memo, and a Reply-in-DM quote (`replyRef`).
        at += 10_000;
        messages.push(serde_json::from_value(serde_json::json!({
            "id": "u-noref", "chatId": "g1", "role": "user", "sender": "user:gh_1", "senderName": "Affinity",
            "status": "done", "createdAt": at, "completedAt": at, "pending": true,
            "parts": [{ "id": "u-noref-a", "kind": "attachment", "attachment": { "name": "Screenshot.png", "mime": "image/png", "kind": "image" } }],
        })).unwrap());
        at += 10_000;
        messages.push(serde_json::from_value(serde_json::json!({
            "id": "u-memo", "chatId": "g1", "role": "user", "sender": "user:gh_1", "senderName": "Affinity",
            "status": "done", "createdAt": at, "completedAt": at,
            "parts": [{ "id": "u-memo-a", "kind": "attachment", "attachment": { "name": "memo.m4a", "mime": "audio/mp4", "ref": "blob-memo", "kind": "audio", "bytes": 48213 } }],
        })).unwrap());
        at += 10_000;
        messages.push(serde_json::from_value(serde_json::json!({
            "id": "u-ref", "chatId": "g1", "role": "user", "sender": "user:gh_1", "senderName": "Affinity",
            "status": "done", "createdAt": at, "completedAt": at,
            "replyRef": { "chatId": "other", "projectId": "", "messageId": "m-far", "chatTitle": "Design", "sender": "user:gh_2", "senderName": "Sam", "senderHandle": "sam", "text": "" },
            "parts": [{ "id": "u-ref-t", "kind": "text", "text": "replying across chats" }],
        })).unwrap());
        state.update(cx, |s, cx| {
            s.projects.push(Project {
                id: "g1".into(),
                name: "Arbonites".into(),
                kind: SpaceKind::Group,
                ..Project::default()
            });
            let chat: Chat = serde_json::from_value(serde_json::json!({
                "id": "g1", "project_id": "g1", "title": "Arbonites", "is_pinned": true,
                "is_system": false, "unread": false, "last_message_at": at, "kind": "group",
            }))
            .unwrap();
            s.chats.insert("g1".into(), vec![chat]);
            s.messages.insert("g1".into(), messages);
            s.selected_project = Some("g1".into());
            s.active_tab.insert("g1".into(), "g1".into());
            cx.notify();
        });
        // The SVG bytes arrive as a thumbnail for both refs (what FetchThumbnail returns).
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1000 1000"><circle cx="500" cy="500" r="400"/></svg>"#.to_vec();
        state.update(cx, |s, cx| {
            for key in ["blob-svg-1", "blob-svg-2"] {
                s.apply(
                    SyncEvent::Thumbnail {
                        key: key.into(),
                        bytes: std::sync::Arc::new(svg.clone()),
                    },
                    cx,
                );
            }
        });
        // Real link icons for the sites in the texts (what the relay would answer).
        let png = {
            let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 30, 30, 255]));
            let mut out = std::io::Cursor::new(Vec::new());
            img.write_to(&mut out, image::ImageFormat::Png).unwrap();
            out.into_inner()
        };
        state.update(cx, |s, cx| {
            for host in ["austin.eater.com", "affine.io", "nypost.com"] {
                s.apply(
                    SyncEvent::LinkIcon {
                        host: host.into(),
                        png: Some(std::sync::Arc::new(png.clone())),
                    },
                    cx,
                );
            }
        });
        let transcript = cx.new(|cx| Transcript::new(state.clone(), cx));
        transcript.update(cx, |t, cx| t.sync(cx));
        let (_view, vcx) = cx.add_window_view(|_, _| Host {
            transcript: transcript.clone(),
        });
        vcx.run_until_parked();
        // Local actions: scroll through the whole list, sweep the pointer over every 24 pt
        // band, click every link and the bubbles, open the hover rail — at each scroll stop.
        let column = vcx.debug_bounds("chat-column").expect("column");
        for stop in 0..12 {
            transcript.update(vcx, |t, cx| {
                t.list.scroll_to(gpui::ListOffset {
                    item_ix: stop * 3,
                    offset_in_item: px(0.0),
                });
                cx.notify();
            });
            vcx.run_until_parked();
            let mut y = column.top() + px(8.0);
            while y < column.bottom() {
                for x in [
                    column.left() + px(60.0),
                    column.left() + px(200.0),
                    column.left() + px(420.0),
                    column.right() - px(40.0),
                ] {
                    vcx.simulate_mouse_move(point(x, y), None, Modifiers::default());
                    vcx.run_until_parked();
                    vcx.simulate_click(point(x, y), Modifiers::default());
                    vcx.run_until_parked();
                }
                y += px(24.0);
            }
        }
        assert!(vcx.debug_bounds("chat-column").is_some());
        drop(rt);
    }
}

/// Chart bubbles ([feedback chart-bubble-mac-2026-10-26]): the `chart` part is one block of
/// the agent's bubble (the code card's frame, title, legend, plot, the three icon squares);
/// its fallback picture is hidden; Copy data puts the TSV on the clipboard.
#[cfg(test)]
mod chart_tests {
    use super::*;
    use crate::ui::chart_card::{ICON_SQUARE, PLOT_HEIGHT};

    const SPEC: &str = r#"{"kind":"line","title":"Eval score over runs","subtitle":"memory-pack eval","series":[{"name":"Affine 2","points":[[1,66.2],[2,70.4],[3,79.6],[4,83.4]]},{"name":"Baseline","points":[[1,70.1],[2,70.6],[3,71.1],[4,71.2]]}],"x":{"label":"Run"},"y":{"label":"Pass rate","unit":"%","min":60,"max":90}}"#;

    fn chart_message(fallback_points_at: &str) -> Message {
        let spec: serde_json::Value = serde_json::from_str(SPEC).unwrap();
        serde_json::from_value(serde_json::json!({
            "id": "m2",
            "chatId": "c1",
            "role": "assistant",
            "sender": "box:1",
            "status": "done",
            "createdAt": 1_700_000_002_000u64,
            "inReplyTo": "m1",
            "parts": [
                { "id": "t", "kind": "text", "text": "Affine 2 passed the baseline at run 2." },
                { "id": "a", "kind": "attachment", "attachment": { "name": "eval.png", "mime": "image/png", "ref": "blob-1", "bytes": 55_000, "kind": "image", "title": "Eval score over runs", "chartPart": fallback_points_at } },
                { "id": "c", "kind": "chart", "specVersion": 1, "chart": spec, "table": "| Run | Affine 2 (%) | Baseline (%) |\n| --- | --- | --- |\n| 1 | 66.2 | 70.1 |", "fallback": "a" }
            ],
        }))
        .expect("message")
    }

    fn rows_of(m: Message) -> Vec<Row> {
        let spoken = HashSet::new();
        let failed = HashMap::new();
        let speech = SpeechRows {
            enabled: false,
            spoken: &spoken,
            failed: &failed,
            voice_mode: false,
        };
        build_rows(
            &[m],
            false,
            speech,
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            false,
        )
    }

    #[test]
    fn the_part_parses_with_its_camel_case_fields() {
        let m = chart_message("c");
        let Part::Chart {
            id,
            spec_version,
            chart,
            table,
            fallback,
        } = &m.parts[2]
        else {
            panic!("chart part")
        };
        assert_eq!(id, "c");
        assert_eq!(*spec_version, 1);
        assert_eq!(chart["kind"], "line");
        assert!(table.starts_with("| Run |"));
        assert_eq!(fallback.as_deref(), Some("a"));
        let Part::Attachment { attachment, .. } = &m.parts[1] else {
            panic!("attachment part")
        };
        assert_eq!(attachment.chart_part.as_deref(), Some("c"));
        assert!(m.has_chart_part("c") && !m.has_chart_part("zz"));
    }

    #[test]
    fn a_chart_part_is_the_bubbles_last_block_and_hides_its_picture() {
        let rows = rows_of(chart_message("c"));
        let kinds: Vec<String> = rows
            .iter()
            .map(|r| match &r.kind {
                RowKind::Markdown {
                    block, first, last, ..
                } => format!(
                    "{}:{}{}",
                    match block {
                        Block::Chart { .. } => "chart",
                        Block::Paragraph(_) => "para",
                        _ => "other",
                    },
                    if *first { "F" } else { "" },
                    if *last { "L" } else { "" }
                ),
                RowKind::Attachment { .. } => "attachment".into(),
                _ => "misc".into(),
            })
            .filter(|k| k != "misc")
            .collect();
        assert_eq!(kinds, ["para:F", "chart:L"], "{kinds:?}");
        let chart_row = rows
            .iter()
            .find(|r| {
                matches!(
                    r.kind,
                    RowKind::Markdown {
                        block: Block::Chart { .. },
                        ..
                    }
                )
            })
            .unwrap();
        assert_eq!(chart_row.id, "m2#c");
        if let RowKind::Markdown {
            block: Block::Chart { table, spec, .. },
            bubble,
            ..
        } = &chart_row.kind
        {
            assert!(spec.contains("\"kind\":\"line\""));
            assert_eq!(
                bubble.text.as_ref(),
                table.as_str(),
                "a copy carries the table"
            );
        }

        // The picture stands in for a chart that is not in this message: it stays a picture.
        let rows = rows_of(chart_message("elsewhere"));
        assert!(rows
            .iter()
            .any(|r| matches!(r.kind, RowKind::Attachment { .. })));
        assert_eq!(
            rows.iter()
                .filter(|r| matches!(
                    r.kind,
                    RowKind::Markdown {
                        block: Block::Chart { .. },
                        ..
                    }
                ))
                .count(),
            1
        );
    }

    #[test]
    fn one_line_and_plain_text_name_the_chart() {
        let block = Block::Chart {
            part_id: "c".into(),
            spec: SPEC.into(),
            table: "| a |\n| --- |\n| 1 |".into(),
        };
        assert_eq!(block_plain(&block, false), "| a |\n| --- |\n| 1 |");
        assert!(!is_json_block(&block));
        assert_eq!(block_segments(&block, false).len(), 1);
    }

    struct ChartHost {
        selection: Entity<TextSelection>,
        code_copied: Option<(SharedString, Instant)>,
        saved: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        expanded: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    }

    impl gpui::Render for ChartHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::dark(crate::ui::theme::Surface::Matte);
            let block = Block::Chart {
                part_id: "c".into(),
                spec: SPEC.into(),
                table: String::new(),
            };
            let selectable = Selectable::new(&self.selection, theme.accent_soft, cx);
            let bubble = selectable.bubble("m2#text", String::new());
            let this = cx.entity().downgrade();
            let saved = self.saved.clone();
            let expanded = self.expanded.clone();
            let copy = CodeCopy {
                copied: copied_pill(&self.code_copied, Instant::now()),
                on_copy: Some(Rc::new(move |id, text, _, cx| {
                    let _ = this.update(cx, |v, cx| {
                        copy_code_to_clipboard(&mut v.code_copied, id, text, cx);
                        cx.notify();
                    });
                })),
                chart: Some(Rc::new(crate::ui::chart_card::ChartUi {
                    on_hover: Rc::new(|_, _| {}),
                    on_save: Rc::new(move |spec, title, _, _| {
                        saved.borrow_mut().push(format!("{title}:{}", spec.len()))
                    }),
                    on_expand: Rc::new(move |part_id, _, _, _| expanded.borrow_mut().push(part_id)),
                })),
                ..Default::default()
            };
            div()
                .w(px(536.0))
                .debug_selector(|| "chart-host".into())
                .child(render_block(
                    "m2#c",
                    &block,
                    0,
                    None,
                    Instant::now(),
                    &theme,
                    &bubble,
                    &copy,
                ))
        }
    }

    #[gpui::test]
    fn the_card_draws_its_parts_and_the_squares_act(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let theme = crate::ui::theme::Theme::for_pref(
                crate::ui::theme::AppearancePref::Dark,
                crate::ui::theme::Surface::Matte,
                cx,
            );
            cx.set_global(theme);
        });
        let saved = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let expanded = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let (view, vcx) = cx.add_window_view(|_, cx| ChartHost {
            selection: cx.new(TextSelection::new),
            code_copied: None,
            saved: saved.clone(),
            expanded: expanded.clone(),
        });
        vcx.run_until_parked();
        let host = vcx.debug_bounds("chart-host").expect("host");
        let card = vcx.debug_bounds("chart-card").expect("card drawn");
        assert_eq!(
            card.size.width, host.size.width,
            "the card fills the bubble"
        );
        vcx.debug_bounds("chart-title").expect("title");
        vcx.debug_bounds("chart-legend")
            .expect("legend: two series");
        let plot = vcx.debug_bounds("m2#c#chart-plot").expect("plot");
        assert_eq!(plot.size.height, px(PLOT_HEIGHT));
        assert!(
            plot.origin.y > card.origin.y + px(40.0),
            "under the title and legend"
        );
        let copy = vcx.debug_bounds("m2#c#chart-copy").expect("copy square");
        let save = vcx.debug_bounds("m2#c#chart-save").expect("save square");
        let expand = vcx
            .debug_bounds("m2#c#chart-expand")
            .expect("expand square");
        for b in [&copy, &save, &expand] {
            assert_eq!(b.size.width, px(ICON_SQUARE));
            assert_eq!(b.size.height, px(ICON_SQUARE));
        }
        assert!(copy.origin.x < save.origin.x && save.origin.x < expand.origin.x);
        assert!(
            (expand.origin.x + expand.size.width - (card.origin.x + card.size.width)).abs()
                <= px(9.0),
            "inset 8 pt from the card's right edge: {:?} vs {:?}",
            expand,
            card
        );
        assert!(
            (expand.origin.y - card.origin.y).abs() <= px(9.0),
            "and from its top"
        );

        vcx.simulate_click(copy.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let clipboard = vcx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
        let text = clipboard.expect("TSV on the clipboard");
        assert_eq!(
            text.lines().next().unwrap(),
            "Run\tAffine 2 (%)\tBaseline (%)"
        );
        assert_eq!(text.lines().nth(4).unwrap(), "4\t83.4\t71.2");
        view.read_with(vcx, |v, _| {
            assert_eq!(
                copied_pill(&v.code_copied, Instant::now()).as_deref(),
                Some("m2#c#chart-copy"),
                "the square shows the check"
            );
        });
        vcx.simulate_click(save.center(), gpui::Modifiers::default());
        vcx.simulate_click(expand.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert_eq!(
            saved.borrow().as_slice(),
            [format!("Eval score over runs:{}", SPEC.len())]
        );
        assert_eq!(expanded.borrow().as_slice(), ["c"]);
    }

    #[test]
    fn the_viewer_state_opens_switches_and_closes() {
        let v = crate::ui::chart_viewer::ChartViewer {
            chat_id: Some("c1".into()),
            part_id: "c".into(),
            spec: SPEC.into(),
            data_view: false,
            copied_at: None,
        };
        assert!(!v.copied_now());
        let mut v2 = v.clone();
        v2.copied_at = Some(now_ms());
        assert!(v2.copied_now());
        v2.copied_at = Some(now_ms() - crate::ui::chart_viewer::COPIED_MS - 1);
        assert!(!v2.copied_now());
    }
}

/// Link previews ([feedback link-embeds-mac-2026-10-27]): the `link_preview` part parses,
/// becomes a card under the text (or the whole bubble for a bare link), is hidden by
/// `preview: "off"` and the global switch, and the card draws its parts.
#[cfg(test)]
mod link_preview_tests {
    use super::*;

    fn part(extra: serde_json::Value) -> serde_json::Value {
        let mut v = serde_json::json!({
            "id": "lp", "kind": "link_preview", "specVersion": 1,
            "url": "https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH?si=163bbf4a6caf4b39",
            "canonicalUrl": "https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH",
            "provider": "spotify", "providerName": "Spotify", "type": "song",
            "title": "Midnight City", "subtitle": "M83 · Hurry Up, We're Dreaming",
            "facts": ["Song", "4:03"], "siteHost": "open.spotify.com",
            "image": { "ref": "lp-abc.jpg", "width": 300, "height": 300, "mime": "image/jpeg", "art": true },
            "layout": "compact", "fetchedAt": 1_700_000_000_000u64
        });
        if let Some(o) = extra.as_object() {
            for (k, val) in o {
                v[k] = val.clone();
            }
        }
        v
    }

    fn user_message(text: &str, preview_field: Option<&str>) -> Message {
        let mut m = serde_json::json!({
            "id": "m1", "chatId": "c1", "role": "user", "sender": "user:gh_me", "status": "done",
            "createdAt": 1_700_000_001_000u64,
            "parts": [ { "id": "t", "kind": "text", "text": text }, part(serde_json::json!({})) ],
        });
        if let Some(p) = preview_field {
            m["preview"] = serde_json::json!(p);
        }
        serde_json::from_value(m).expect("message")
    }

    fn rows_of(m: Message) -> Vec<Row> {
        rows_with_previews(m, true)
    }

    fn rows_with_previews(m: Message, link_previews: bool) -> Vec<Row> {
        let spoken = HashSet::new();
        let failed = HashMap::new();
        let speech = SpeechRows {
            enabled: false,
            spoken: &spoken,
            failed: &failed,
            voice_mode: false,
        };
        build_rows_with_cache(
            &[m],
            false,
            speech,
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            false,
            false,
            link_previews,
            &mut MessageRowCache::default(),
            &HashSet::new(),
        )
    }

    fn user_row(rows: &[Row]) -> (String, Option<Box<(String, LinkPreview)>>) {
        rows.iter()
            .find_map(|r| match &r.kind {
                RowKind::User { text, preview, .. } => Some((text.clone(), preview.clone())),
                _ => None,
            })
            .expect("a user row")
    }

    #[test]
    fn the_part_parses_and_unknown_fields_are_ignored() {
        let m = user_message("x", None);
        let Part::LinkPreview { id, preview } = &m.parts[1] else {
            panic!("link preview part")
        };
        assert_eq!(id, "lp");
        assert_eq!(preview.provider, "spotify");
        assert_eq!(preview.kind, "song");
        assert_eq!(preview.title, "Midnight City");
        assert_eq!(preview.facts, ["Song", "4:03"]);
        assert_eq!(
            preview.image.as_ref().map(|i| i.blob_ref.as_str()),
            Some("lp-abc.jpg")
        );
        assert!(preview.image.as_ref().is_some_and(|i| i.art));
        assert_eq!(preview.site_host.as_deref(), Some("open.spotify.com"));
        assert!(preview.site_icon.is_none(), "reserved for the shared tile");
        assert!(!preview.expanded());
        assert_eq!(preview.one_line(), "Midnight City — Spotify");
        let m: Message = serde_json::from_value(serde_json::json!({
            "id": "m9", "role": "user", "status": "done",
            "parts": [ { "id": "z", "kind": "link_preview", "title": "T", "newerField": { "a": 1 } } ],
        }))
        .unwrap();
        assert!(matches!(&m.parts[0], Part::LinkPreview { preview, .. } if preview.title == "T"));
    }

    #[test]
    fn text_plus_link_keeps_the_text_and_a_bare_link_becomes_the_card() {
        let (text, preview) = user_row(&rows_of(user_message(
            "listen to this https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH?si=163bbf4a6caf4b39",
            None,
        )));
        assert!(text.starts_with("listen to this https://"), "{text}");
        assert!(preview.is_some());

        let (text, preview) = user_row(&rows_of(user_message(
            "https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH?si=163bbf4a6caf4b39",
            None,
        )));
        assert_eq!(text, "", "a bare link shows the card alone");
        assert_eq!(preview.as_ref().map(|p| p.0.as_str()), Some("lp"));

        // The canonical form of the same link counts as bare too.
        let (text, _) = user_row(&rows_of(user_message(
            "https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH",
            None,
        )));
        assert_eq!(text, "");
    }

    #[test]
    fn preview_off_and_the_settings_switch_hide_the_card() {
        let (text, preview) = user_row(&rows_of(user_message(
            "https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH",
            Some("off"),
        )));
        assert!(preview.is_none());
        assert!(text.starts_with("https://"), "the plain link is back");

        let (_, preview) = user_row(&rows_with_previews(
            user_message(
                "https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH",
                None,
            ),
            false,
        ));
        assert!(preview.is_none());
    }

    #[test]
    fn an_agents_link_preview_is_the_bubbles_last_block() {
        let m: Message = serde_json::from_value(serde_json::json!({
            "id": "m2", "chatId": "c1", "role": "assistant", "sender": "box:1", "status": "done",
            "createdAt": 1_700_000_002_000u64,
            "parts": [
                { "id": "t", "kind": "text", "text": "Mira posted the count this morning: https://x.com/mira/status/1" },
                part(serde_json::json!({ "provider": "x", "providerName": "X", "type": "post", "title": "Mira Kaplan", "subtitle": "@mirakaplan", "description": "Subnet 120 just crossed 1,000 validators.", "facts": ["Oct 27, 2026"], "image": serde_json::Value::Null, "layout": "compact" })),
            ],
        }))
        .unwrap();
        let rows = rows_of(m);
        let kinds: Vec<String> = rows
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::Markdown {
                    block, first, last, ..
                } => Some(format!(
                    "{}:{}{}",
                    match block {
                        Block::LinkPreview { .. } => "link",
                        Block::Paragraph(_) => "para",
                        _ => "other",
                    },
                    if *first { "F" } else { "" },
                    if *last { "L" } else { "" }
                )),
                _ => None,
            })
            .collect();
        assert_eq!(kinds, ["para:F", "link:L"], "{kinds:?}");
        let row = rows
            .iter()
            .find(|r| r.id == "m2#lp")
            .expect("the card's row");
        if let RowKind::Markdown {
            block: Block::LinkPreview { preview, .. },
            bubble,
            ..
        } = &row.kind
        {
            assert_eq!(preview.kind, "post");
            assert_eq!(
                bubble.text.as_ref(),
                "Mira Kaplan — X",
                "a copy carries title and site"
            );
        } else {
            panic!("link block");
        }
    }

    #[test]
    fn is_link_only_matches_the_typed_or_canonical_url_alone() {
        let p = LinkPreview {
            url: "https://a.test/x?si=1".into(),
            canonical_url: "https://a.test/x".into(),
            ..Default::default()
        };
        assert!(is_link_only("https://a.test/x?si=1", &p));
        assert!(is_link_only("  https://a.test/x  ", &p));
        assert!(is_link_only("https://a.test/x?si=1/", &p));
        assert!(!is_link_only("see https://a.test/x?si=1", &p));
        assert!(!is_link_only("", &p));
    }

    struct Host {
        selection: Entity<TextSelection>,
        removable: bool,
        own: bool,
        expanded: bool,
        opened: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        removed: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    }

    impl gpui::Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::dark(crate::ui::theme::Surface::Matte);
            let mut v = part(serde_json::json!({}));
            if self.expanded {
                v["layout"] = serde_json::json!("expanded");
                v["type"] = serde_json::json!("video");
                v["duration"] = serde_json::json!("18:24");
                v["image"] = serde_json::json!({ "ref": "lp-yt.jpg", "width": 1280, "height": 720, "mime": "image/jpeg", "art": false });
            }
            let preview: LinkPreview = serde_json::from_value(v).unwrap();
            let _ = Selectable::new(&self.selection, theme.accent_soft, cx);
            let opened = self.opened.clone();
            let removed = self.removed.clone();
            let ui = Rc::new(crate::ui::link_card::LinkUi {
                thumbnail: Rc::new(|_| None),
                host_icon: Rc::new(|_| None),
                on_open: Rc::new(move |url, _, _| opened.borrow_mut().push(url)),
                on_remove: Rc::new(move |mid, _, _| removed.borrow_mut().push(mid)),
            });
            div()
                .w(px(536.0))
                .debug_selector(|| "link-host".into())
                .child(crate::ui::link_card::card(
                    "m1#user",
                    "m1",
                    &preview,
                    &theme,
                    Some(&ui),
                    crate::ui::link_card::Place {
                        own: self.own,
                        removable: self.removable,
                    },
                ))
        }
    }

    #[allow(clippy::type_complexity)]
    fn host(
        cx: &mut gpui::TestAppContext,
        removable: bool,
        expanded: bool,
    ) -> (
        Entity<Host>,
        &mut gpui::VisualTestContext,
        std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    ) {
        cx.update(|cx| {
            let theme = crate::ui::theme::Theme::for_pref(
                crate::ui::theme::AppearancePref::Dark,
                crate::ui::theme::Surface::Matte,
                cx,
            );
            cx.set_global(theme);
        });
        let opened = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let removed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let (o, r) = (opened.clone(), removed.clone());
        let (view, vcx) = cx.add_window_view(move |_, cx| Host {
            selection: cx.new(TextSelection::new),
            removable,
            own: true,
            expanded,
            opened: o,
            removed: r,
        });
        vcx.run_until_parked();
        (view, vcx, opened, removed)
    }

    #[gpui::test]
    fn the_compact_card_is_capped_at_400_with_56pt_art_and_opens_on_click(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_view, vcx, opened, removed) = host(cx, true, false);
        let card = vcx.debug_bounds("link-card").expect("card drawn");
        assert_eq!(
            f32::from(card.size.width),
            crate::ui::link_card::CARD_MAX_WIDTH,
            "capped, not the 536 pt bubble"
        );
        let art = vcx.debug_bounds("link-art").expect("art");
        assert_eq!(f32::from(art.size.width), crate::ui::link_card::ART);
        assert_eq!(f32::from(art.size.height), crate::ui::link_card::ART);
        vcx.debug_bounds("link-footer").expect("footer");
        vcx.debug_bounds("link-tile")
            .expect("the site-icon tile slot");
        assert!(
            vcx.debug_bounds("link-media").is_none(),
            "compact: no picture on top"
        );
        let remove = vcx
            .debug_bounds("link-remove")
            .expect("× present on a removable card");
        assert!(f32::from(remove.size.width) == crate::ui::link_card::REMOVE_DISC);
        // A click on the text opens the canonical link.
        let text = vcx.debug_bounds("link-text").unwrap();
        vcx.simulate_click(text.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert_eq!(
            opened.borrow().as_slice(),
            ["https://open.spotify.com/track/7eiYNCojnoUVnz14kjBcQH"]
        );
        assert!(removed.borrow().is_empty());
        // A click on the × removes and does not open.
        vcx.simulate_click(remove.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert_eq!(removed.borrow().as_slice(), ["m1"]);
        assert_eq!(opened.borrow().len(), 1, "the × swallows its click");
    }

    #[gpui::test]
    fn the_expanded_card_puts_the_picture_on_top_with_play_and_duration(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_view, vcx, _, _) = host(cx, false, true);
        let card = vcx.debug_bounds("link-card").expect("card");
        let media = vcx.debug_bounds("link-media").expect("picture on top");
        assert_eq!(
            f32::from(media.size.height),
            crate::ui::link_card::MEDIA_HEIGHT
        );
        assert!(
            (f32::from(card.size.width) - f32::from(media.size.width)).abs() <= 2.0,
            "the picture spans the card inside its hairline"
        );
        let text = vcx.debug_bounds("link-text").unwrap();
        assert!(text.top() >= media.bottom(), "text under the picture");
        vcx.debug_bounds("link-play").expect("▶ on a video");
        vcx.debug_bounds("link-duration").expect("duration chip");
        assert!(vcx.debug_bounds("link-art").is_none());
        assert!(
            vcx.debug_bounds("link-remove").is_none(),
            "no × when not removable"
        );
    }
}
