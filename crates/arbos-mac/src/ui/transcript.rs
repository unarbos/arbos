//! The chat column body: a bottom-aligned `list()` with one row per markdown
//! block (zeron-study §4.1–§4.4). Rows are diffed by `(id, version)` and
//! spliced; streaming text fades in through the veil.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    actions, div, list, prelude::*, px, Animation, AnimationExt, AnyElement, App, Context, Div,
    Entity, FontWeight, ListAlignment, ListScrollEvent, ListState, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, SharedString, Stateful, TextRun, Window,
};

use crate::app::{AppEvent, AppState, DetailsTarget};
use crate::model::{
    now_ms, Attachment, AttachmentKind, Chat, Id, Message, MessageStatus, Part, Person, Project,
    Role, SpaceKind, StatusKind, TurnKind,
};
use crate::ui::approve::{approval_card, ApprovalView};
use crate::ui::attachments;
use crate::ui::avatars;
use crate::ui::cards;
use crate::ui::frost::edge_faded;
use crate::ui::icons::icon;
use crate::ui::markdown::{
    has_fenced_code, inline_plain, inline_spans, polish_assistant_prose, split_blocks, Block,
};
use crate::ui::mention::{self, Mentions};
use crate::ui::motion::PulseClock;
use crate::ui::people;
use crate::ui::select::{BubbleSelectable, Selectable, TextSelection};
use crate::ui::theme::{
    Theme, BUBBLE_PAD_X, BUBBLE_PAD_Y, BUBBLE_RADIUS, CHAT_BODY, CHAT_LINE_HEIGHT, COLUMN_MARGIN,
    COMPOSER_HEIGHT, COMPOSER_WIDTH, LINE_HEIGHT, PLUS_BUTTON, TEXT_SMALL, TEXT_TINY,
};
use crate::ui::veil::Veil;
use crate::ui::widgets::QuestionCard;
use serde_json::Value;

actions!(transcript, [Copy]);

/// Where one of my own bubbles is on its way. No caption says it (Jacob,
/// 2026-10-24: "remove these 'sent' items, just use coloring"): the bubble's
/// opacity does — see [`Delivery::opacity`] and docs/mac-design-rules.md.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Delivery {
    /// Not yet acknowledged by the relay.
    Pending,
    /// The relay refused it; a red glyph marks it beside the bubble.
    Failed,
    /// Acknowledged, nobody has opened past it yet.
    Sent,
    /// A peer's read watermark covers it, an agent answered after it, or read
    /// receipts are off (nothing to wait for).
    Seen,
}

/// Whole-bubble opacity of a sent-but-unseen bubble. Measured on both themes
/// (white text, bubble `#5c5c5c` on `#141414` / `#111111` on `#fcfcfc`): the
/// composited text keeps a 4.9 : 1 (dark) and 7.7 : 1 (light) contrast on its
/// fill, while the fill drops to L* 30 from 39 (dark) and to `#535353` from
/// `#111111` (light) — visibly lighter next to a seen bubble, still readable.
pub const BUBBLE_ALPHA_SENT: f32 = 0.72;
/// A pending bubble sits below a sent one (3.4 : 1 / 3.6 : 1 text contrast —
/// transient, it lasts until the relay's ACK).
pub const BUBBLE_ALPHA_PENDING: f32 = 0.5;

impl Delivery {
    pub fn opacity(self) -> f32 {
        match self {
            Delivery::Pending => BUBBLE_ALPHA_PENDING,
            Delivery::Sent => BUBBLE_ALPHA_SENT,
            Delivery::Failed | Delivery::Seen => 1.0,
        }
    }
}

const OVERDRAW_PX: f32 = 320.0;
/// One column for every chat kind: the rows and the composer are centred
/// in the pane with the same side margin ([`COLUMN_MARGIN`]) and the same
/// width cap, so the agent bubble's left edge is the composer's left edge
/// and my bubble's right edge is the composer's right edge, at any pane
/// width. Grok Mac (refs 57/58, 2 px/pt): both at 16 pt from the pane. The
/// cap is the composer pill at its widest as it has always shipped — the
/// 820 pt input plus the two 28 pt circles — so the pill itself does not
/// move; up to a 908 pt pane the margins bound both.
pub const COLUMN_MAX_WIDTH: f32 = COMPOSER_WIDTH + 2.0 * PLUS_BUTTON;
/// Agent bubble cap, measured from Grok Mac on two pane widths (ref 57:
/// 728 pt pane, 612 pt bubble; ref 58: 424 pt pane, 310 pt bubble): the
/// bubble ends a fixed 84 pt before the column's right edge (100 pt from
/// the pane), not at a fixed width. The hover rail (reply · copy · react ·
/// time) lives in that gap, so hovering never re-wraps the text. Only the
/// sent/seen label sits inside the bubble; the time is hover-only
/// (docs/mac-design-rules.md).
pub const ASSISTANT_RIGHT_GAP: f32 = 84.0;
/// A new time stamp line whenever messages are this far apart.
const STAMP_GAP_MS: u64 = 60 * 60 * 1000;
/// Vertical rhythm measured from Grok Bot's Mac app (parity it.36, 2 px/pt):
/// consecutive bubbles of one sender sit 6 pt apart, a change of sender opens
/// 20 pt, a time stamp has 20 pt above and 24 below, and the last message keeps
/// 54 pt clear of the composer.
const ROW_GAP_HALF: f32 = 3.0;
const TURN_GAP_EXTRA: f32 = 20.0 - 2.0 * ROW_GAP_HALF;
/// Measured glyph-to-bubble, like the reference: the stamp's 13 pt line box
/// carries ~4 pt of its own above and ~7 below the glyphs (measured).
const STAMP_PAD_TOP: f32 = 20.0 - ROW_GAP_HALF - 4.0;
const STAMP_PAD_BOTTOM: f32 = 24.0 - ROW_GAP_HALF - 7.0;
/// Clearance above the composer when pinned (54 pt glyph-to-pill, minus the
/// composer row's own 6 pt top padding and the last row's half-gap). Kept as a
/// trailing list row — not as container padding — so scrolling bubbles reach
/// the real edge of the input instead of hitting an opaque black strip.
const BOTTOM_INSET: f32 = 54.0 - ROW_GAP_HALF - 6.0;
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
/// How far a right swipe pulls the bubble before reply arms (iPhone
/// `replySwipeWidth` / Telegram-style).
const REPLY_SWIPE_WIDTH: f32 = 56.0;
/// Release past this fraction of [`REPLY_SWIPE_WIDTH`] starts a reply.
const REPLY_SWIPE_ARM: f32 = REPLY_SWIPE_WIDTH * 0.7;
/// Ignore pointer noise until the drag travels this far (iPhone minimumDistance).
const REPLY_SWIPE_SLOP: f32 = 12.0;

/// Which bubble row the pointer is over, and the one it just left, with the
/// moment each happened: the fade is computed from those at render time.
#[derive(Default)]
struct HoverFade {
    over: Option<(String, Instant)>,
    left: Option<(String, Instant)>,
}

/// In-progress swipe-right-to-reply on a message row (iPhone `replySwipeGesture`).
struct ReplySwipe {
    /// The transcript row the press started on (visual offset + reply glyph).
    row_id: String,
    /// The message to reply to when the swipe arms.
    message_id: Id,
    /// Press origin in window coordinates.
    start: Point<Pixels>,
    /// How far the row has been pulled right (px).
    drag: f32,
    /// True once the pan is clearly horizontal-right (scroll / select stay free until then).
    locked: bool,
}

/// Click-drag vertical pan that scrolls the list (iPhone ScrollView parity).
/// Armed on press; locks once the pan is clearly vertical so text selection
/// and reply-swipe stay free for horizontal / select gestures.
struct DragScroll {
    start: Point<Pixels>,
    last: Point<Pixels>,
    locked: bool,
}

impl HoverFade {
    /// Record a hover change for `row_id`; `true` when something changed.
    fn set(&mut self, row_id: &str, hovered: bool, now: Instant) -> bool {
        let is_over = self.over.as_ref().is_some_and(|(id, _)| id == row_id);
        if hovered == is_over {
            return false;
        }
        if hovered {
            if let Some((prev, _)) = self.over.take() {
                self.left = Some((prev, now));
            }
            // Re-entering mid-fade starts the fade-in from the current level.
            let head_start = match &self.left {
                Some((id, at)) if id == row_id => {
                    let remaining = 1.0 - Self::progress(*at, now);
                    Duration::from_secs_f32(remaining * HOVER_FADE_MS / 1000.0)
                }
                _ => Duration::ZERO,
            };
            self.left = None;
            self.over = Some((row_id.to_string(), now - head_start));
        } else {
            self.over = None;
            self.left = Some((row_id.to_string(), now));
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
    /// The message replied to (`replyTo` on the row).
    pub message_id: Id,
    /// Its first line, cut to [`QUOTE_CHARS`] characters with an ellipsis.
    pub text: String,
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
        }
    }
}

/// A whole message on one line, for quote headers and the composer's reply
/// preview: markdown blocks (headings, bullets, fences, tables) and inline
/// markers (`**`, `` ` ``, links) are dropped, every newline becomes a space,
/// whitespace is collapsed. Attachments are not part of it — callers name the
/// file when there is no prose.
pub fn one_line(text: &str) -> String {
    let mut out = String::new();
    for block in split_blocks(text) {
        let pieces: Vec<String> = match block {
            Block::Paragraph(text) | Block::Heading { text, .. } => vec![text],
            Block::Bullets(items) | Block::Numbered(items) => items,
            Block::Code { code, .. } => vec![code],
            Block::Table { header, rows } => header
                .into_iter()
                .chain(rows.into_iter().flatten())
                .collect(),
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
    out
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
fn forwarded_header(by: String, text_color: gpui::Hsla) -> Stateful<Div> {
    let muted = Theme::with_alpha(text_color, 0.72);
    div()
        .id(SharedString::from(format!("forwarded-from-{by}")))
        .flex()
        .items_center()
        .gap(px(6.0))
        .min_w_0()
        .child(icon("forward", 12.0, muted))
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

#[derive(Clone, Debug)]
pub enum RowKind {
    User {
        text: String,
        /// Caption for prompts not typed by the user (a routine, another project).
        sender_label: Option<String>,
        /// Another person's bubble carries their name inside it, at the top, when
        /// the chat names its senders (group / channel, see [`names_senders`]) and
        /// the sender changed (Grok Bot's group style for the run).
        named: bool,
        /// Another person in a people chat (D72): left bubble with their
        /// avatar and handle instead of my right bubble.
        from: Option<Person>,
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
    },
    /// Centered muted time line ("Today 9:39 PM").
    Stamp {
        label: String,
    },
    /// Emoji reaction chips under a bubble (iPhone parity): `(emoji, senders)`.
    Reactions {
        reactions: Vec<(String, Vec<String>)>,
        /// Under my right-hand bubble (chips align right).
        mine: bool,
        /// Under another person's bubble (chips clear the avatar column).
        indent: bool,
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
    Attachment {
        attachment: Attachment,
        /// The `parts[]` id: the viewer opens on this picture of the album.
        part_id: Id,
        mine: bool,
        /// Someone else's (person or agent) in a people chat: keep the
        /// sender's avatar lane free, like the message's bubble.
        lane: bool,
        /// The message's "Forwarded from …" line, drawn on the card.
        forwarded_from: Option<String>,
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

/// The hover rail's buttons: Grok Mac's 24 pt hit target, 16 pt glyph, 2 pt apart.
pub const RAIL_BUTTON: f32 = 24.0;
pub const RAIL_GLYPH: f32 = 16.0;
pub const RAIL_GAP: f32 = 2.0;

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
}

impl gpui::EventEmitter<TranscriptEvent> for Transcript {}

pub struct Transcript {
    state: Entity<AppState>,
    chat_id: Option<Id>,
    list: ListState,
    rows: Vec<Row>,
    /// Who `@handle` resolves to in this chat's bubbles (mention chips),
    /// rebuilt with the rows.
    mentions: Rc<Mentions>,
    /// Whether the right pane was open at the last sync: its opening or
    /// closing changes the column width, and the cached row layouts must go.
    pane_open: bool,
    veils: HashMap<String, Veil>,
    /// One stateful card per typed question, keyed by question id; dropped with its row.
    cards: HashMap<String, Entity<QuestionCard>>,
    pinned_to_bottom: bool,
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
    /// Swipe-right-to-reply in progress (Telegram / iPhone parity).
    reply_swipe: Option<ReplySwipe>,
    /// Vertical click-drag scroll over bubbles / empty rows.
    drag_scroll: Option<DragScroll>,
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
            mentions: Mentions::empty(),
            pane_open: false,
            veils: HashMap::new(),
            cards: HashMap::new(),
            pinned_to_bottom: true,
            revealed: None,
            spring: StickSpring::default(),
            repin_check_pending: false,
            hover: HoverFade::default(),
            selection,
            turn_starts: HashSet::new(),
            multi_party: false,
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
            reply_swipe: None,
            drag_scroll: None,
        }
    }

    /// Rows that can be replied to (same set as the message menu's Reply item
    /// on the iPhone: text, media, and empty agent bubbles — not stamps,
    /// cards, errors, or chrome).
    fn can_reply_row(kind: &RowKind) -> bool {
        matches!(
            kind,
            RowKind::User { .. }
                | RowKind::Markdown { .. }
                | RowKind::NoAnswer { .. }
                | RowKind::Attachment { .. }
        )
    }

    /// Mouse down on a replyable row: arm a possible swipe-right-to-reply.
    /// Vertical scroll and text selection stay free until the pan locks.
    fn begin_reply_swipe(&mut self, row_id: String, message_id: Id, start: Point<Pixels>) {
        self.reply_swipe = Some(ReplySwipe {
            row_id,
            message_id,
            start,
            drag: 0.0,
            locked: false,
        });
    }

    /// Trackpad / mouse move while a reply swipe may be in progress.
    fn on_reply_swipe_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.reply_swipe.is_none() {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.reply_swipe = None;
            cx.notify();
            return;
        }
        let (dx, dy, was_locked, prev_drag) = {
            let swipe = self.reply_swipe.as_ref().expect("checked above");
            let dx: f32 = (event.position.x - swipe.start.x).into();
            let dy: f32 = (event.position.y - swipe.start.y).into();
            (dx, dy, swipe.locked, swipe.drag)
        };
        if !was_locked {
            if dx.abs() + dy.abs() < REPLY_SWIPE_SLOP {
                return;
            }
            // Vertical scroll or leftward peek: do not steal the gesture.
            if dx <= 0.0 || dx.abs() <= dy.abs() * 1.15 {
                self.reply_swipe = None;
                return;
            }
        }
        let raw = dx.max(0.0);
        let w = REPLY_SWIPE_WIDTH;
        let reveal = if raw <= w {
            raw
        } else {
            w + ((raw - w) * 0.2).min(24.0)
        };
        let just_locked = !was_locked;
        let moved = (prev_drag - reveal).abs() > 0.5;
        if let Some(swipe) = self.reply_swipe.as_mut() {
            swipe.locked = true;
            if moved {
                swipe.drag = reveal;
            }
        }
        if just_locked {
            self.selection.update(cx, |s, cx| {
                s.clear();
                cx.notify();
            });
        }
        if just_locked || moved {
            cx.notify();
        }
    }

    /// Mouse up: if the swipe armed far enough, start a reply (same path as
    /// the hover reply glyph and the message-menu Reply item).
    fn on_reply_swipe_up(&mut self, cx: &mut Context<Self>) {
        let Some(swipe) = self.reply_swipe.take() else {
            return;
        };
        let triggered = swipe.locked && swipe.drag >= REPLY_SWIPE_ARM;
        if triggered {
            if let Some(chat_id) = self.chat_id.clone() {
                cx.emit(TranscriptEvent::Reply {
                    chat_id,
                    message_id: swipe.message_id,
                });
            }
        }
        cx.notify();
    }

    /// Apply `ARBOS_RIG_SELECT` to the newest settled agent bubble, once.
    fn apply_rig_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((spec, not_before)) = self.rig_select.clone() else {
            return;
        };
        if Instant::now() < not_before {
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

    /// ⌘C with text selected in a bubble.
    fn copy_selection(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selection.read(cx).selected_text() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    /// The row around a bubble: tracks the pointer so the time and react glyph
    /// can fade, and reports their opacity for this frame. With Reduce Motion
    /// the fade collapses to the snap it replaced.
    fn hover_row(
        &self,
        row_id: &str,
        now: Instant,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> (Stateful<Div>, f32) {
        let (opacity, moving) = self.hover.opacity(row_id, now);
        let opacity = if cx.reduce_motion() {
            opacity.round()
        } else {
            if moving {
                window.request_animation_frame();
            }
            opacity
        };
        let id = row_id.to_string();
        let row = div()
            .id(SharedString::from(format!("{row_id}#row")))
            .w_full()
            .flex()
            .items_center()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if this.hover.set(&id, *hovered, Instant::now()) {
                    cx.notify();
                }
            }));
        (row, opacity)
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
    fn on_wheel(&mut self, event: &gpui::ScrollWheelEvent, cx: &mut Context<Self>) {
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
        // Reply swipe has locked horizontal: leave it alone.
        if self.reply_swipe.as_ref().is_some_and(|s| s.locked) {
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
            if dx.abs() + dy_from_start.abs() < REPLY_SWIPE_SLOP {
                return;
            }
            // Horizontal-dominant: text selection / reply swipe own the gesture.
            if dx.abs() > dy_from_start.abs() * 1.15 {
                self.drag_scroll = None;
                return;
            }
            // Vertical: take the gesture.
            self.reply_swipe = None;
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
        if remaining.abs() < 0.5 && self.spring.velocity.abs() < 0.5 {
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
        let state = self.state.read(cx);
        let chat_id = state.active_chat_id();
        let chat_changed = chat_id != self.chat_id;
        let pane_open = state.profile.is_some();
        let pane_changed = pane_open != self.pane_open;
        self.pane_open = pane_open;
        if chat_changed {
            self.reply_swipe = None;
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
        // Per-handle read watermarks for this chat (Settings → Notifications).
        let receipts = chat_id
            .as_deref()
            .and_then(|id| state.message_receipts.get(id));
        let read_receipts = state.read_receipts;
        self.chat_id = chat_id;
        let new_rows = build_rows(
            &messages,
            show_details,
            speech,
            &me_id,
            &thumb_keys,
            &self.mentions,
            multi_party,
            receipts,
            read_receipts,
        );
        self.multi_party = multi_party;
        // A sender change opens the turn gap. So does every new assistant /
        // project message — consecutive replies from the same agent (quick ack
        // then the answer, or two answers in a row) must not read as one bubble
        // (#1447 / doug-merged-reply-bubbles).
        self.turn_starts = messages
            .windows(2)
            .filter(|w| {
                (w[0].role, &w[0].sender) != (w[1].role, &w[1].sender)
                    || matches!(w[1].role, Role::Assistant | Role::Project)
            })
            .map(|w| w[1].id.clone())
            .collect();

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
            self.list.scroll_to_end();
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

    /// Plain text of a message in this chat, for copy / control-click.
    fn message_plain(&self, message_id: &Id, cx: &App) -> Option<String> {
        let chat_id = self.chat_id.as_ref()?;
        self.state
            .read(cx)
            .messages
            .get(chat_id)
            .and_then(|list| list.iter().find(|m| m.id == *message_id))
            .map(|m| m.plain_text())
    }

    pub(crate) fn copy_message(&mut self, message_id: &Id, cx: &mut Context<Self>) {
        if let Some(text) = self.message_plain(message_id, cx) {
            if !text.is_empty() {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                self.copied_feedback = Some((message_id.clone(), Instant::now()));
                cx.notify();
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(COPY_FEEDBACK_MS))
                        .await;
                    let _ = this.update(cx, |this, cx| {
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
        }
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
    fn code_copy(&self, cx: &Context<Self>) -> CodeCopy {
        let this = cx.entity().downgrade();
        CodeCopy {
            copied: copied_pill(&self.code_copied, Instant::now()),
            on_copy: Some(Rc::new(move |id, code, _, cx| {
                let _ = this.update(cx, |t, cx| t.copy_code(id, code, cx));
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
    fn quote_header(&self, row_id: &str, quote: Quote, text_color: gpui::Hsla) -> Stateful<Div> {
        let state = self.state.clone();
        let chat_id = self.chat_id.clone();
        let target = quote.message_id.clone();
        let muted = Theme::with_alpha(text_color, 0.72);
        div()
            .id(SharedString::from(format!("{row_id}#quote")))
            .flex()
            .items_center()
            .gap(px(6.0))
            .min_w_0()
            .cursor_pointer()
            .hover(|s| s.opacity(0.8))
            .child(icon("bubble", 12.0, muted))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(TEXT_SMALL))
                    .line_height(px(LINE_HEIGHT - 4.0))
                    .text_color(muted)
                    .child(format!("“{}”", quote.text)),
            )
            .on_click(move |_, _, cx| {
                if let Some(chat_id) = chat_id.clone() {
                    let target = target.clone();
                    state.update(cx, |s, cx| s.reveal_reply(chat_id, target, cx));
                }
            })
    }

    /// The hover rail beside a bubble (fades with the row), in Grok's order:
    /// smiley (the reaction capsule) · reply · ⋯ (the message menu: Copy,
    /// Reply, Delete). After a copy from the menu the ⋯ shows a checkmark
    /// for a moment.
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
        let more_glyph = if just_copied { "check" } else { "ellipsis" };
        let more_color = if just_copied {
            theme.online
        } else {
            theme.text_muted
        };
        let btn = |id: SharedString, glyph: &'static str, opacity: f32, color: gpui::Hsla| {
            div()
                .id(id)
                .size(px(RAIL_BUTTON))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .opacity(opacity)
                .hover(|s| s.bg(theme.surface_2))
                .child(icon(glyph, RAIL_GLYPH, color))
        };
        div()
            .flex()
            .items_center()
            .gap(px(RAIL_GAP))
            .child(
                btn(
                    SharedString::from(format!("{row_id}#react")),
                    "face-smile",
                    opacity,
                    theme.text_muted,
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
                    "reply",
                    opacity,
                    theme.text_muted,
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
        AgentIdentity {
            name: a.name,
            seed: a.id.clone(),
            color_index: Some(a.color_index),
            leaf: (Some(a.leaf.leaf), Some(a.leaf.color)),
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
                Some(space) => crate::ui::avatars::project(space, 16.0, theme).into_any_element(),
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
                    gpui::img(image)
                        .size(px(SENDER_DISC))
                        .rounded_full()
                        .object_fit(gpui::ObjectFit::Cover),
                );
        }
        avatars::agent(
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
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(CHAT_BODY))
                    .line_height(px(CHAT_LINE_HEIGHT))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color)
                    .child(name),
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
        let name_el = match (&identity, agent_accent) {
            (Some(a), Some(color)) if named => {
                let open_state = self.state.clone();
                let pid = a.project_id.clone();
                Some(
                    self.sender_name_label(row_id, a.name.clone(), color, true)
                        .when_some(pid, |el, pid| {
                            el.cursor_pointer().on_click(move |_, _, cx| {
                                let pid = pid.clone();
                                open_state.update(cx, |s, cx| {
                                    s.push_profile(
                                        crate::app::ProfilePage::Agent { project_id: pid },
                                        cx,
                                    )
                                });
                            })
                        })
                        .into_any_element(),
                )
            }
            _ => None,
        };
        let r = px(BUBBLE_RADIUS);
        let quote_header = quote
            .filter(|_| first)
            .map(|q| self.quote_header(row_id, q, theme.text).into_any_element());
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
            .when(!first, |el| el.pt(px(3.0)))
            .when(last, |el| el.pb(px(BUBBLE_PAD_Y)).rounded_b(r))
            .when(!last, |el| el.pb(px(3.0)))
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
                    &self.code_copy(cx),
                ),
            ));
        let (row_el, reveal) = self.hover_row(row_id, now, window, cx);
        // Hover: the rail (smiley · reply · ⋯) and the bubble's time on the last
        // block of a finished reply. Nothing of it exists at rest.
        let hover_actions =
            (last && !live).then(|| self.hover_rail(row_id, message_id, reveal, theme, cx));
        let hover_time = (last && !live)
            .then(|| hover_time(time, reveal, theme).map(|t| t.ml(px(8.0))))
            .flatten();
        // The rail hangs off the bubble's right edge, out of the flow: it
        // lives in the column's 84 pt gap and never narrows the bubble.
        let hover_rail = (hover_actions.is_some() || hover_time.is_some()).then(|| {
            div()
                .absolute()
                .left(relative_width(1.0))
                .top_0()
                .bottom_0()
                .pl(px(6.0))
                .flex()
                .items_center()
                .children(hover_actions)
                .children(hover_time)
        });
        let bubble = bubble.children(hover_rail);
        // People chats: the sender's disc at the bubble's bottom-left on
        // the last block; earlier blocks keep the lane free (mocks a/f).
        let disc = lane.as_deref().map(|seed| {
            if last {
                // The disc opens the agent's profile, pushed over the group's pane.
                let (_, _, _, agent_pid) = self.agent_identity(seed, "Agent", cx);
                let open_state = self.state.clone();
                div()
                    .id(SharedString::from(format!("{row_id}#disc")))
                    .flex_shrink_0()
                    .child(self.agent_disc(seed, cx))
                    .when_some(agent_pid, |el, pid| {
                        el.cursor_pointer().on_click(move |_, _, cx| {
                            let pid = pid.clone();
                            open_state.update(cx, |s, cx| {
                                s.push_profile(
                                    crate::app::ProfilePage::Agent { project_id: pid },
                                    cx,
                                )
                            });
                        })
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
            .child(assistant_slot(bubble))
            .into_any_element()
    }

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        let theme = Theme::of(cx).clone();
        let now = Instant::now();
        // Before the match moves `row.kind`: which rows accept swipe-right-to-reply.
        let can_reply = Self::can_reply_row(&row.kind) && !row.message_id.is_empty();
        let swipe_row_id = row.id.clone();
        let swipe_message_id = row.message_id.clone();
        let selectable = Selectable::new(&self.selection, theme.accent_soft, cx)
            .with_mentions(self.mentions.clone(), theme.accent);
        let inner: AnyElement = match row.kind {
            RowKind::User {
                text,
                sender_label,
                from,
                named,
                time,
                delivery,
                quote,
                forwarded_from,
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
                    None => quote.map(|q| self.quote_header(&row.id, q, text_color)),
                };
                // Triple-backtick fences render as code cards (same as agent
                // replies); other text stays plain so "# heading" is still literal.
                // Bare `http(s)://…` URLs are still accented and clickable.
                let body = user_message_body(
                    &row.id,
                    &text,
                    text_color,
                    &theme,
                    &bubble_selectable,
                    &self.code_copy(cx),
                );
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
                let open_person = |this: &Self, person: &Person, cx: &Context<Self>| {
                    let open_state = this.state.clone();
                    let handle = person.handle.clone();
                    let sid = this.chat_id.as_ref().and_then(|c| {
                        this.state.read(cx).chats.iter().find_map(|(pid, tabs)| {
                            tabs.iter().any(|t| &t.id == c).then(|| pid.clone())
                        })
                    });
                    move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                        let (handle, sid) = (handle.clone(), sid.clone());
                        open_state.update(cx, |s, cx| {
                            s.push_profile(
                                crate::app::ProfilePage::User {
                                    handle,
                                    space_id: sid,
                                },
                                cx,
                            )
                        });
                    }
                };
                let name_el = match (&from, color) {
                    (Some(person), Some(color)) if named => Some(
                        self.sender_name_label(&row.id, person.display_name(), color, false)
                            .cursor_pointer()
                            .on_click(open_person(self, person, cx))
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
                    .max_w(relative_width(0.72))
                    .px(px(BUBBLE_PAD_X))
                    .py(px(BUBBLE_PAD_Y))
                    .rounded(px(BUBBLE_RADIUS))
                    .bg(theme.bubble)
                    .text_color(theme.bubble_text())
                    .text_size(px(CHAT_BODY))
                    .line_height(px(CHAT_LINE_HEIGHT))
                    .child(sender_bubble_column(
                        name_el,
                        quote_header.map(|q| q.into_any_element()),
                        body,
                    ));
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
                            let (row_el, reveal) = self.hover_row(&row.id, now, window, cx);
                            let actions =
                                self.hover_rail(&row.id, &row.message_id, reveal, &theme, cx);
                            row_el
                                .justify_start()
                                .items_end()
                                .pl(px(AVATAR_COLUMN_INSET))
                                .gap(px(SENDER_DISC_GAP))
                                .child(
                                    // The disc opens the same profile as the name.
                                    div()
                                        .id(SharedString::from(format!("{}#disc", row.id)))
                                        .flex_shrink_0()
                                        .cursor_pointer()
                                        .child(people::avatar(image, &person, SENDER_DISC, &theme))
                                        .on_click(open_person(self, &person, cx)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .child(
                                            bubble
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
                            let (row_el, reveal) = self.hover_row(&row.id, now, window, cx);
                            let actions =
                                self.hover_rail(&row.id, &row.message_id, reveal, &theme, cx);
                            let delivery = delivery.unwrap_or(Delivery::Seen);
                            row_el
                                .justify_end()
                                .children(hover_time(time, reveal, &theme).map(|t| t.mr(px(8.0))))
                                .child(actions.mr(px(6.0)))
                                .children(failed_glyph(delivery, &theme).map(|g| g.mr(px(8.0))))
                                .child(own_bubble(bubble, delivery))
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
            } => self.markdown_bubble(
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
                now,
                &theme,
                &selectable,
                window,
                cx,
            ),
            RowKind::NoAnswer {
                seed,
                name,
                time,
                lane,
                quote,
                named,
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
                    now,
                    &theme,
                    &selectable,
                    window,
                    cx,
                )
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
                indent,
            } => {
                // iPhone parity: small capsules under the bubble, mine tinted;
                // click toggles my reaction on that emoji.
                let me = format!("user:{}", self.state.read(cx).me_id());
                let chat_id = self.chat_id.clone().unwrap_or_default();
                let mut chips = div().flex().flex_wrap().gap(px(4.0));
                for (emoji, senders) in reactions {
                    let on = senders.contains(&me);
                    let (state, chat, message_id, e) = (
                        self.state.clone(),
                        chat_id.clone(),
                        row.message_id.clone(),
                        emoji.clone(),
                    );
                    chips = chips.child(
                        div()
                            .id(SharedString::from(format!("{}#{}", row.id, emoji)))
                            .flex()
                            .items_center()
                            .gap(px(3.0))
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded_full()
                            .bg(if on {
                                Theme::with_alpha(theme.accent, 0.22)
                            } else {
                                theme.bubble_assistant
                            })
                            .border_1()
                            .border_color(if on {
                                Theme::with_alpha(theme.accent, 0.55)
                            } else {
                                theme.border
                            })
                            .cursor_pointer()
                            .child(div().text_size(px(14.0)).child(emoji.clone()))
                            .when(senders.len() > 1, |el| {
                                el.child(
                                    div()
                                        .text_size(px(12.0))
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
                    .w_full()
                    .flex()
                    .when(mine, |el| el.justify_end())
                    .when(!mine, |el| el.justify_start())
                    .when(indent, |el| el.pl(px(2.0)))
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
                lane,
                forwarded_from,
            } => {
                let preview = match attachment.blob_ref.as_ref() {
                    None => attachments::Preview::Unavailable,
                    Some(k) => {
                        let state = self.state.read(cx);
                        match state.card_thumbnail(k) {
                            Some(image) => attachments::Preview::Ready(image),
                            None if state.thumbnails_failed.contains(k) => {
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
                        open,
                        self.state.clone(),
                        &theme,
                    ));
                let (row_el, reveal) = self.hover_row(&row.id, now, window, cx);
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
                                .child(actions.mr(px(6.0)))
                                .child(card),
                        )
                        .into_any_element()
                } else {
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
                                .when(lane, |el| el.pl(px(SENDER_LANE)))
                                .child(card)
                                .child(actions.ml(px(6.0))),
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
            RowKind::Approval { view, lane } => {
                let chat_id = self.chat_id.clone().unwrap_or_default();
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
        // The first row of a message from a new sender opens the turn gap; a
        // stamp between them already carries its own spacing.
        let turn_start = ix
            .checked_sub(1)
            .and_then(|i| self.rows.get(i))
            .is_some_and(|prev| {
                prev.message_id != row.message_id
                    && !matches!(prev.kind, RowKind::Stamp { .. } | RowKind::BottomInset)
                    && self.turn_starts.contains(&row.message_id)
            });
        let swipe_drag = self
            .reply_swipe
            .as_ref()
            .filter(|s| s.row_id == swipe_row_id)
            .map(|s| s.drag)
            .unwrap_or(0.0);
        let accent = theme.accent;
        div()
            .id(SharedString::from(format!("{swipe_row_id}#swipe")))
            .relative()
            .w_full()
            .when(can_reply, |el| {
                let (row_id, message_id) = (swipe_row_id.clone(), swipe_message_id.clone());
                el.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, _cx| {
                        if event.modifiers.control || event.modifiers.shift {
                            return;
                        }
                        this.begin_reply_swipe(row_id.clone(), message_id.clone(), event.position);
                    }),
                )
            })
            .when(swipe_drag > 0.5, |el| {
                el.child(
                    div()
                        .absolute()
                        .left(px(COLUMN_MARGIN + swipe_drag - REPLY_SWIPE_WIDTH))
                        .top_0()
                        .bottom_0()
                        .w(px(REPLY_SWIPE_WIDTH))
                        .flex()
                        .items_center()
                        .justify_start()
                        .pl(px(4.0))
                        .opacity((swipe_drag / (REPLY_SWIPE_WIDTH * 0.55)).min(1.0))
                        .child(icon("reply", 16.0, accent)),
                )
            })
            .child(
                column_row(inner)
                    .when(swipe_drag > 0.0, |el| el.ml(px(swipe_drag)))
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
/// sender lane, minus Grok's fixed [`ASSISTANT_RIGHT_GAP`]. The bubble's
/// `max_w_full` is this slot's width.
fn assistant_slot(bubble: impl IntoElement) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .pr(px(ASSISTANT_RIGHT_GAP))
        .flex()
        .items_center()
        .debug_selector(|| "chat-bubble-slot".into())
        .child(bubble)
}

/// A question / approval card is a transcript row like a bubble (#1374, #1440 follow-up): the
/// same left edge as an incoming bubble — after the sender's avatar lane in a people chat — and
/// the same cap width (`assistant_slot`). The lane is left empty: the card belongs to the
/// agent's run and the disc already sits on its bubble.
pub fn card_row(card: impl IntoElement, lane: bool) -> Div {
    div()
        .w_full()
        .flex()
        .when(lane, |el| el.pl(px(SENDER_LANE)))
        .debug_selector(|| "chat-card-row".into())
        .child(assistant_slot(card))
}

fn relative_width(fraction: f32) -> gpui::DefiniteLength {
    gpui::relative(fraction)
}

impl Render for Transcript {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.step_spring(window, cx);
        let theme = Theme::of(cx).clone();
        // Grok Mac (ref 58): a 40 pt ↓ circle centred above the composer once
        // the user has scrolled up; clicking re-pins and the spring glides down.
        let max = f32::from(self.list.max_offset_for_scrollbar().y);
        let cur = -f32::from(self.list.scroll_px_offset_for_scrollbar().y);
        let show_jump = !self.pinned_to_bottom && max - cur > JUMP_BUTTON_AFTER_PX;
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
            .on_action(cx.listener(Self::copy_selection))
            .on_scroll_wheel(cx.listener(|this, event, _, cx| this.on_wheel(event, cx)))
            // Drag-scroll + swipe-right-to-reply: move/up live on the transcript
            // so a drag that leaves the row still finishes (iPhone parity).
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _cx| {
                    if event.modifiers.control || event.modifiers.shift {
                        return;
                    }
                    this.begin_drag_scroll(event.position);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                this.on_reply_swipe_move(event, cx);
                this.on_drag_scroll_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.on_reply_swipe_up(cx);
                    this.end_drag_scroll();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.on_reply_swipe_up(cx);
                    this.end_drag_scroll();
                }),
            )
            // Grok's header band (Jacob, Oct 22 12:49 CR, superseding the opaque
            // band): the list runs up under the floating pill and fades out
            // over `HEADER_FADE_BAND` — Grok's text is gone at the window edge
            // and fully readable ~50 pt down ([feedback mac-header-fade-grok]).
            .child(edge_faded(
                HEADER_FADE_BAND,
                true,
                false,
                list(self.list.clone(), cx.processor(Self::render_row)).size_full(),
            ))
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
                                .bg(theme.card())
                                .border_1()
                                .border_color(theme.border_strong)
                                .shadow_md()
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.surface_2))
                                .child(icon("arrow-down", 15.0, theme.text))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.pinned_to_bottom = true;
                                    this.repin_check_pending = false;
                                    cx.notify();
                                }))
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
        Block::Bullets(items) | Block::Numbered(items) => items.iter().map(|s| s.len() + 1).sum(),
        Block::Code { code, .. } => code.len(),
        Block::Table { header, rows } => {
            header.iter().map(|s| s.len() + 1).sum::<usize>()
                + rows.iter().flatten().map(|s| s.len() + 1).sum::<usize>()
        }
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
/// refused, then sent until a peer opened past it (or an agent answered after
/// it). With read receipts off there is nothing to wait for, so an ACKed
/// bubble is `Seen` (full opacity) at once. `None` for someone else's bubble.
pub fn delivery_state(
    message: &Message,
    messages: &[Message],
    mix: usize,
    is_mine: bool,
    receipts: Option<&HashMap<String, Id>>,
    read_receipts: bool,
) -> Option<Delivery> {
    if !is_mine {
        return None;
    }
    if message.pending {
        return Some(Delivery::Pending);
    }
    if message.status == MessageStatus::Error {
        return Some(Delivery::Failed);
    }
    if !read_receipts || message_is_seen(message, messages, mix, receipts) {
        return Some(Delivery::Seen);
    }
    Some(Delivery::Sent)
}

/// True when a peer's read watermark covers this message, or an agent/project
/// replied after it (the reply is the "opened" signal in agent chats).
fn message_is_seen(
    message: &Message,
    messages: &[Message],
    mix: usize,
    receipts: Option<&HashMap<String, Id>>,
) -> bool {
    if let Some(receipts) = receipts {
        if !receipts.is_empty() {
            let any = receipts.values().any(|seen_id| {
                messages
                    .iter()
                    .position(|m| &m.id == seen_id)
                    .is_some_and(|ix| ix >= mix)
                    || seen_id == &message.id
            });
            if any {
                return true;
            }
        }
    }
    messages[mix + 1..]
        .iter()
        .any(|m| matches!(m.role, Role::Assistant | Role::Project))
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

/// My own bubble at its delivery opacity (see [`Delivery::opacity`]). One
/// wrapper for every own bubble so the layout tests can read the state back.
fn own_bubble(bubble: impl IntoElement, delivery: Delivery) -> Div {
    div()
        .max_w_full()
        .min_w_0()
        .debug_selector(|| {
            match delivery {
                Delivery::Pending => "own-bubble-pending",
                Delivery::Failed => "own-bubble-failed",
                Delivery::Sent => "own-bubble-sent",
                Delivery::Seen => "own-bubble-seen",
            }
            .into()
        })
        .opacity(delivery.opacity())
        .child(bubble)
}

/// The red mark beside a bubble the relay refused: always visible, outside
/// the bubble on its outer edge. (There is no resend yet — the composer keeps
/// the text, the mark says the row is not on the relay.)
fn failed_glyph(delivery: Delivery, theme: &Theme) -> Option<Div> {
    (delivery == Delivery::Failed).then(|| {
        div()
            .flex_shrink_0()
            .size(px(RAIL_GLYPH))
            .flex()
            .items_center()
            .justify_center()
            .debug_selector(|| "own-bubble-failed-glyph".into())
            .child(icon("exclamation", RAIL_GLYPH, theme.danger))
    })
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
        | Block::Bullets(_)
        | Block::Numbered(_)
        | Block::Table { .. } => return false,
    };
    let t = text.trim();
    (t.starts_with('{') || t.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(t).is_ok()
}

/// The transcript shows only what the agent said and made: prose, files,
/// questions, errors. Everything the agent did (tool calls, status events,
/// JSON, usage) is hidden; the Details drawer has it. One indicator per chat
/// pulses under the last message while anything runs.
///
/// `thumb_keys` are blob refs that already have a decoded preview; they are
/// mixed into row versions so a late thumbnail reloads the picture row.
/// `names_senders`, `receipts`, and `read_receipts` landed from parallel
/// feedbacks; keep them as plain args rather than a one-off options struct.
#[allow(clippy::too_many_arguments)]
pub fn build_rows(
    messages: &[Message],
    show_details: bool,
    speech: SpeechRows<'_>,
    me_id: &str,
    thumb_keys: &HashSet<&str>,
    mentions: &Mentions,
    names_senders: bool,
    receipts: Option<&HashMap<String, Id>>,
    read_receipts: bool,
) -> Vec<Row> {
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
    // Who spoke last in a people chat: a bubble carries its sender's name only when
    // the sender changed or a date stamp broke the run (Grok Bot's group style), and
    // only in chats that name their senders at all (`names_senders`).
    let mut prev_sender: Option<String> = None;
    for (mix, message) in messages.iter().enumerate() {
        let streaming = message.is_streaming();
        if message.created_at > 0 && message.created_at.saturating_sub(last_stamp_at) > STAMP_GAP_MS
        {
            last_stamp_at = message.created_at;
            prev_sender = None;
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
                // `@handle` → `@Display Name` so the chip shows the name and a copy
                // carries it (mention chips, Oct 23).
                let text = mentions.display(&message.plain_text());
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
                let named = names_senders
                    && from.is_some()
                    && prev_sender.as_deref() != Some(message.sender.as_str());
                // Image preview / file card rows (same cards as agent attachments).
                let is_mine = from.is_none();
                // A forwarded attachment's "Forwarded from …" line is the card's
                // hint, not a bubble of its own.
                let forwarded = (!attachment_parts.is_empty())
                    .then(|| attachments::forwarded_from(&text).map(str::to_owned))
                    .flatten();
                for (part_id, attachment) in &attachment_parts {
                    let has_thumb = attachment
                        .blob_ref
                        .as_deref()
                        .is_some_and(|k| thumb_keys.contains(k));
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
                            ),
                            false,
                        ),
                        id: format!("{}#{}", message.id, part_id),
                        message_id: message.id.clone(),
                        kind: RowKind::Attachment {
                            attachment: attachment.clone(),
                            part_id: part_id.clone(),
                            mine: is_mine,
                            lane: !is_mine,
                            forwarded_from: forwarded.clone(),
                        },
                    });
                }
                // Skip a bare "Image"/"Photo" bubble when the attachment already shows it.
                let show_text = !text.is_empty()
                    && !(has_image && is_generic_media_label(&text))
                    && forwarded.is_none();
                if show_text || attachment_parts.is_empty() {
                    let quote = message
                        .reply_to
                        .as_deref()
                        .filter(|id| !id.is_empty())
                        .map(|id| Quote::for_reply(id, messages));
                    let delivery =
                        delivery_state(message, messages, mix, is_mine, receipts, read_receipts);
                    let forwarded_from = message
                        .forwarded_from
                        .as_deref()
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .map(str::to_owned);
                    rows.push(Row {
                        version: hash_version(
                            &(
                                &text,
                                &sender_label,
                                from.as_ref().map(|p| &p.handle),
                                &quote,
                                delivery,
                                &forwarded_from,
                            ),
                            false,
                        ),
                        id: format!("{}#user", message.id),
                        message_id: message.id.clone(),
                        kind: RowKind::User {
                            text,
                            sender_label,
                            from,
                            named,
                            time: clock_label(message.created_at),
                            delivery,
                            quote,
                            forwarded_from,
                        },
                    });
                }
                if message.status == MessageStatus::Queued {
                    queued = true;
                }
                push_reactions(&mut rows, message, mine(message));
            }
            Role::System => {
                // Call lines and membership notices ("Jacob added Home, hjkb and
                // Doug", #1349): one centred muted line, never a bubble. Like a
                // date stamp it breaks the sender run — the next bubble names its
                // sender again (Telegram / Grok Bot).
                let text = message.plain_text();
                let line = text.lines().next().unwrap_or_default().trim().to_string();
                if !line.is_empty() {
                    rows.push(Row {
                        version: hash_version(&line, false),
                        id: format!("{}#system", message.id),
                        message_id: message.id.clone(),
                        kind: RowKind::Stamp { label: line },
                    });
                    prev_sender = None;
                }
            }
            Role::Assistant | Role::Project => {
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
                    if let Some(name) = named {
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
                // name resolves at render time from the seed, like the disc). Each
                // people-chat reply names its sender (#1447): consecutive answers from
                // the same agent must not merge into one unnamed run. A stream with no
                // words yet has no bubble, so it shows no name either (the presence row
                // says who is working).
                let sender_named = names_senders
                    && lane.is_some()
                    && (agent_speaker || prev_sender.as_deref() != Some(message.sender.as_str()));
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
                let total_blocks: usize = polished
                    .iter()
                    .map(|(_, text)| {
                        split_blocks(text)
                            .into_iter()
                            .filter(|b| !is_json_block(b))
                            .count()
                    })
                    .sum();
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
                                let named = sender_named && first;
                                rows.push(Row {
                                    version: hash_version(
                                        &(&block, first, last, fill, &reply_quote, named),
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
                                    },
                                });
                                block_ix += 1;
                            }
                        }
                        Part::Attachment { id, attachment } => {
                            let has_thumb = attachment
                                .blob_ref
                                .as_deref()
                                .is_some_and(|k| thumb_keys.contains(k));
                            rows.push(Row {
                                version: hash_version(
                                    &(
                                        &attachment.name,
                                        &attachment.title,
                                        attachment.bytes,
                                        &attachment.blob_ref,
                                        has_thumb,
                                    ),
                                    false,
                                ),
                                id: format!("{}#{}", message.id, id),
                                message_id: message.id.clone(),
                                kind: RowKind::Attachment {
                                    attachment: attachment.clone(),
                                    part_id: id.clone(),
                                    mine: false,
                                    lane: lane.is_some(),
                                    forwarded_from: None,
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
                            };
                            rows.push(Row {
                                version: hash_version(
                                    &(approval_id, title, state.label(), decided_by, reason),
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
                if shows_nothing && finished && !streaming && !handoff_row && (failed || only_empty)
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
                        },
                        None => RowKind::NoAnswer {
                            seed: message.sender.clone(),
                            name: message.sender_name.clone().filter(|n| !n.trim().is_empty()),
                            time: clock_label(message.created_at),
                            lane: lane.clone(),
                            quote: reply_quote.clone(),
                            named: sender_named,
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
                    push_reactions(&mut rows, message, false);
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
        if rows.len() > rows_before_message {
            prev_sender = Some(message.sender.clone());
        }
    }
    // Who is working is the presence row over the composer (`ui::presence`),
    // not a transcript row: one notice per agent, never a caption plus a line.
    let _ = (running, queued);
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

/// One chips row under a message that carries reactions.
fn push_reactions(rows: &mut Vec<Row>, message: &Message, mine: bool) {
    let reactions = message.reaction_list();
    if reactions.is_empty() {
        return;
    }
    let indent = message.role == Role::User && !mine;
    rows.push(Row {
        version: hash_version(&(&reactions, mine, indent), false),
        id: format!("{}#reactions", message.id),
        message_id: message.id.clone(),
        kind: RowKind::Reactions {
            reactions,
            mine,
            indent,
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
            t.to_string()
        } else {
            inline_plain(t)
        }
    };
    let seg = |lead: String, text: String| Segment { lead, text };
    match block {
        Block::Paragraph(text) | Block::Heading { text, .. } => {
            vec![seg(String::new(), inline(text))]
        }
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
                    out.push(seg(lead.to_string(), cell.to_string()));
                }
            }
            out
        }
        Block::Code { code, .. } => vec![seg(String::new(), code.clone())],
    }
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
            links.push((start..start + url.len(), url.clone()));
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
fn user_message_body(
    row_id: &str,
    text: &str,
    color: gpui::Hsla,
    theme: &Theme,
    selectable: &Selectable,
    copy: &CodeCopy,
) -> AnyElement {
    let key = format!("{row_id}.u");
    if !has_fenced_code(text) {
        let bubble = selectable.bubble(key, text.to_string());
        return plain_text(text, color, theme, &bubble, 0);
    }
    let blocks = split_blocks(text);
    let bubble = selectable.bubble(key, bubble_plain(&blocks, true));
    let offsets = block_offsets(&blocks, true);
    // Same as `render_block`: no `min_w_0`, so the quote header above never
    // sets this column's width.
    let mut col = div().flex().flex_col().gap(px(6.0)).w_full();
    for (i, block) in blocks.iter().enumerate() {
        let id = format!("{row_id}.u{i}");
        let at = offsets[i];
        col = match block {
            Block::Code { lang, code } => col.child(code_fence_card(
                &id,
                lang,
                code,
                color,
                theme,
                &bubble,
                at,
                None,
                Instant::now(),
                copy,
            )),
            // Prose, lists and tables stay literal in a typed bubble (a
            // "# heading" is still a "# heading"); one element per block.
            Block::Paragraph(_)
            | Block::Heading { .. }
            | Block::Bullets(_)
            | Block::Numbered(_)
            | Block::Table { .. } => col.child(plain_text(
                &block_plain(block, true),
                color,
                theme,
                &bubble,
                at,
            )),
        };
    }
    col.into_any_element()
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
    let spans = inline_spans(text);
    let mut full = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut links: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let sans = sans_font(theme);
    let mono = gpui::Font {
        family: theme.font_mono.into(),
        ..sans.clone()
    };
    for span in spans {
        let style = span.style;
        let mut font = if style.code {
            mono.clone()
        } else {
            sans.clone()
        };
        if style.bold {
            font.weight = FontWeight::SEMIBOLD;
        }
        if style.italic {
            font.style = gpui::FontStyle::Italic;
        }
        let start = full.len();
        if let Some(href) = span.href.clone() {
            links.push((start..start + span.text.len(), href));
        }
        runs.push(TextRun {
            len: span.text.len(),
            font,
            color: if style.link { theme.accent } else { base_color },
            background_color: if style.code {
                Some(theme.code_bg)
            } else {
                None
            },
            underline: if style.link {
                Some(gpui::UnderlineStyle {
                    color: Some(Theme::with_alpha(theme.accent, 0.6)),
                    thickness: px(1.0),
                    wavy: false,
                })
            } else {
                None
            },
            strikethrough: None,
        });
        full.push_str(&span.text);
    }
    let runs = match veil {
        Some(veil) => apply_veil(runs, &veil.spans(now), veil_offset),
        None => runs,
    };
    bubble
        .text(at, full, runs)
        .with_links(links)
        .into_any_element()
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
        .text_color(theme.text)
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
    match block {
        Block::Paragraph(text) => base
            .child(styled_text(
                text, theme.text, theme, veil, now, 0, bubble, starts[0],
            ))
            .into_any_element(),
        Block::Heading { level, text } => {
            let size = match level {
                1 => 20.0,
                2 => 17.0,
                _ => 15.0,
            };
            base.text_size(px(size))
                .font_weight(FontWeight::SEMIBOLD)
                .pt(px(6.0))
                .child(styled_text(
                    text, theme.text, theme, veil, now, 0, bubble, starts[0],
                ))
                .into_any_element()
        }
        Block::Bullets(items) | Block::Numbered(items) => {
            let numbered = matches!(block, Block::Numbered(_));
            let mut col = base.flex().flex_col().gap(px(3.0));
            let mut offset = 0usize;
            // Two-digit markers ("10.") need a wider gutter than "•".
            let marker_w = if numbered && items.len() >= 10 {
                24.0
            } else {
                16.0
            };
            for (i, item) in items.iter().enumerate() {
                let marker = if numbered {
                    format!("{}.", i + 1)
                } else {
                    "•".to_string()
                };
                col = col.child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(
                            div()
                                .w(px(marker_w))
                                .flex_shrink_0()
                                .text_color(theme.text_muted)
                                .child(marker),
                        )
                        .child(div().flex_1().min_w_0().child(styled_text(
                            item, theme.text, theme, veil, now, offset, bubble, starts[i],
                        ))),
                );
                offset += item.len() + 1;
            }
            col.into_any_element()
        }
        Block::Table { header, rows } => {
            let cols = header.len().max(1);
            let font = sans_font(theme);
            let cell = |text: &str, head: bool, i: usize, start: usize| {
                let mut font = font.clone();
                if head {
                    font.weight = FontWeight::SEMIBOLD;
                }
                let color = if head { theme.text } else { theme.text_muted };
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(10.0))
                    .py(px(6.0))
                    .when(i > 0, |el| el.border_l_1().border_color(theme.border))
                    .text_size(px(12.5))
                    .when(head, |el| el.font_weight(FontWeight::SEMIBOLD))
                    .text_color(color)
                    .child(bubble.text(
                        start,
                        text.to_string(),
                        vec![TextRun {
                            len: text.len(),
                            font,
                            color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        }],
                    ))
            };
            let mut table = div()
                .flex()
                .flex_col()
                .w_full()
                .rounded(px(8.0))
                .border_1()
                .border_color(theme.border)
                .overflow_hidden();
            let mut head_row = div().flex().bg(theme.code_bg);
            for (i, h) in header.iter().enumerate() {
                head_row = head_row.child(cell(h, true, i, starts[i]));
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
                        i,
                        starts[(ri + 1) * cols + i],
                    ));
                }
                table = table.child(r);
            }
            table.into_any_element()
        }
        Block::Code { lang, code } => code_fence_card(
            row_id, lang, code, theme.text, theme, bubble, starts[0], veil, now, copy,
        ),
    }
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

/// Fenced ``` block: lang label, Copy, mono body on `code_bg` (Grok's code card).
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
    let mut card = div()
        .flex()
        .flex_col()
        .w_full()
        .rounded(px(8.0))
        .bg(theme.code_bg)
        .border_1()
        .border_color(theme.border);
    card = card.child(
        div()
            .flex()
            .justify_between()
            .items_center()
            .px(px(10.0))
            .py(px(4.0))
            .border_b_1()
            .border_color(theme.border)
            .text_size(px(11.0))
            .text_color(theme.text_faint)
            .child(if lang.is_empty() {
                "code".to_string()
            } else {
                lang.to_string()
            })
            .child(copy_pill(row_id, code, theme, copy)),
    );
    let mono = gpui::Font {
        family: theme.font_mono.into(),
        features: Default::default(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: gpui::FontStyle::Normal,
    };
    let runs = vec![TextRun {
        len: code.len(),
        font: mono,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }];
    let runs = match veil {
        Some(v) => apply_veil(runs, &v.spans(now), 0),
        None => runs,
    };
    card = card.child(
        div()
            .px(px(10.0))
            .py(px(8.0))
            .text_size(px(12.5))
            .line_height(px(18.0))
            .child(bubble.text(at, code.to_string(), runs)),
    );
    card.into_any_element()
}

/// The code card's Copy pill (Grok's code header): the label is the hit
/// target. It answers the pointer — hover, a pressed tint while the button is
/// down — and reads "✓ Copied" for [`CODE_COPY_FEEDBACK_MS`] after a click.
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
    div()
        .id(id)
        .debug_selector(move || format!("{row_id}#copy-pill"))
        .flex()
        .items_center()
        .gap(px(3.0))
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(5.0))
        .cursor_pointer()
        .bg(bg)
        .text_color(fg)
        .when(!copied, |el| {
            el.hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                .active(move |s| s.bg(down_bg).text_color(down_fg))
        })
        .when(copied, |el| el.child(icon("check", 11.0, fg)))
        .child(if copied { "Copied" } else { "Copy" })
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
        // "✓ Copied" is wider than "Copy": the swap really painted.
        let pill = vcx
            .debug_bounds("m1#t.0#copy-pill")
            .expect("pill still drawn");
        assert!(
            pill.size.width > before + px(10.0),
            "{} vs {before}",
            pill.size.width
        );
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
                })
                .collect();
            let presence = presence::Presence {
                participants,
                detail: None,
                queued: false,
            };
            let row_over_pill = presence::row(presence, &theme, cx).into_any_element();
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
            let people_cap = e.bubble_width - SENDER_LANE;
            assert!(
                pcw <= people_cap + 0.5,
                "pane {pane}: people card width {pcw} over the lane cap {people_cap}"
            );
        }
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
    fn the_agent_bubble_cap_is_the_column_minus_the_grok_gap(cx: &mut TestAppContext) {
        // Reference window (884 pt pane): the margins bound the column, so
        // the composer is 852 wide at 16 and the bubble 768 wide at 16 —
        // Grok's 16 pt left inset and 100 pt right inset.
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
            (e.bubble_width - (column - ASSISTANT_RIGHT_GAP)).abs() <= 1.0,
            "bubble width {}",
            e.bubble_width
        );
        assert!((e.bubble_left - e.column_left).abs() <= 1.0);
        // A wide pane: the cap holds both, centred together.
        let e = measure(cx, 1400.0);
        assert_eq!(e.column_width, COLUMN_MAX_WIDTH);
        assert_eq!(e.composer_width, COLUMN_MAX_WIDTH);
        assert!(
            (e.bubble_width - (COLUMN_MAX_WIDTH - ASSISTANT_RIGHT_GAP)).abs() <= 1.0,
            "bubble width {}",
            e.bubble_width
        );
        // Grok's own pane (ref 57, 2 px/pt): composer 696 pt wide at 16 from
        // the pane, bubble 612 pt wide at 16 — the measured reference.
        let e = measure(cx, GROK_REF_57_PANE);
        assert!((e.composer_left - 16.0).abs() <= 1.0, "{}", e.composer_left);
        assert!(
            (e.composer_width - 696.0).abs() <= 1.0,
            "{}",
            e.composer_width
        );
        assert!((e.bubble_left - 16.0).abs() <= 1.0, "{}", e.bubble_left);
        assert!((e.bubble_width - 612.0).abs() <= 1.0, "{}", e.bubble_width);
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
                .child(assistant_slot(bubble))
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
            let cap: f32 = f32::from(slot.size.width) - ASSISTANT_RIGHT_GAP;
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
                            div()
                                .absolute()
                                .left(relative_width(1.0))
                                .top_0()
                                .bottom_0()
                                .pl(px(6.0))
                                .flex()
                                .items_center()
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
                        .child(assistant_slot(bubble))
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
                    let bubble = div()
                        .debug_selector(|| "test-bubble".into())
                        .max_w(relative_width(0.72))
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
                            .child(bubble)
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

    /// The cap a [`BubbleRow`] bubble may reach: the slot minus Grok's gap
    /// for an agent; 72 % of the bubble's parent (the row, or the lane beside
    /// the disc) for people — `relative_width(0.72)` is relative to the parent.
    fn bubble_cap(vcx: &mut gpui::VisualTestContext, who: Who) -> f32 {
        match who {
            Who::Agent => {
                let slot = vcx.debug_bounds("chat-bubble-slot").expect("slot drawn");
                f32::from(slot.size.width) - ASSISTANT_RIGHT_GAP
            }
            Who::Mine | Who::Other => {
                let parent = vcx
                    .debug_bounds("test-bubble-parent")
                    .expect("bubble parent drawn");
                f32::from(parent.size.width) * 0.72
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
            None,
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

    /// #1447: consecutive people-chat replies from the same agent each get a sender
    /// caption and a quote of the message they answer — not one merged run.
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
            None,
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
    /// channels — people when the sender changed, agents on every reply (#1447) — and
    /// never on my own bubble.
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
            None,
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
            None,
            true,
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
            None,
            true,
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
            None,
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
            None,
            true,
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
            None,
            true,
        )
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
}

#[cfg(test)]
mod delivery_tests {
    //! Jacob, 2026-10-24 ("remove these 'sent' items, just use coloring, make the
    //! bubble less opaque"): no caption on my bubbles; a sent-but-unseen bubble is
    //! lighter, a seen one is full; pending sits lower still and a failed one is
    //! full with a red mark. Rule: docs/mac-design-rules.md.
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
            "chatId": "chat_dm",
            "role": role,
            "sender": sender,
            "senderHandle": if sender.starts_with("user:") { "sam" } else { "" },
            "status": status,
            "pending": pending,
            "createdAt": at,
            "parts": [{ "id": format!("p-{id}"), "kind": "text", "text": "fish?" }],
        }))
        .expect("message")
    }

    fn mine(id: &str, at: u64) -> Message {
        message(id, "user", "user:gh_me", "done", false, at)
    }

    #[test]
    fn opacity_is_the_state() {
        assert_eq!(Delivery::Seen.opacity(), 1.0);
        assert_eq!(Delivery::Failed.opacity(), 1.0);
        assert_eq!(Delivery::Sent.opacity(), BUBBLE_ALPHA_SENT);
        assert_eq!(Delivery::Pending.opacity(), BUBBLE_ALPHA_PENDING);
        let order = [Delivery::Pending, Delivery::Sent, Delivery::Seen].map(Delivery::opacity);
        assert!(
            order.windows(2).all(|w| w[0] < w[1]),
            "pending < sent < seen: {order:?}"
        );
    }

    #[test]
    fn unseen_is_sent_and_a_watermark_or_a_reply_makes_it_seen() {
        let t = 1_700_000_000_000u64;
        let messages = vec![mine("a", t), mine("b", t + 1_000), mine("c", t + 2_000)];
        let no_receipts: HashMap<String, Id> = HashMap::new();
        for ix in 0..3 {
            assert_eq!(
                delivery_state(&messages[ix], &messages, ix, true, Some(&no_receipts), true),
                Some(Delivery::Sent),
                "nobody opened the chat: lighter"
            );
        }
        // Sam's watermark is "b": a and b are seen, c is still only sent.
        let receipts: HashMap<String, Id> = HashMap::from([("sam".to_owned(), "b".to_owned())]);
        let got: Vec<_> = (0..3)
            .map(|ix| delivery_state(&messages[ix], &messages, ix, true, Some(&receipts), true))
            .collect();
        assert_eq!(
            got,
            vec![
                Some(Delivery::Seen),
                Some(Delivery::Seen),
                Some(Delivery::Sent)
            ]
        );
        // An agent's answer after my message is the "opened" signal in agent chats.
        let with_reply = vec![
            mine("a", t),
            message("r", "assistant", "agent:arbos", "done", false, t + 500),
            mine("b", t + 1_000),
        ];
        assert_eq!(
            delivery_state(&with_reply[0], &with_reply, 0, true, None, true),
            Some(Delivery::Seen)
        );
        assert_eq!(
            delivery_state(&with_reply[2], &with_reply, 2, true, None, true),
            Some(Delivery::Sent)
        );
    }

    #[test]
    fn pending_failed_and_receipts_off_are_unchanged_states() {
        let t = 1_700_000_000_000u64;
        let pending = message("p", "user", "user:gh_me", "queued", true, t);
        let failed = message("f", "user", "user:gh_me", "error", false, t + 1);
        let sent = mine("s", t + 2);
        let messages = vec![pending.clone(), failed.clone(), sent.clone()];
        assert_eq!(
            delivery_state(&pending, &messages, 0, true, None, true),
            Some(Delivery::Pending)
        );
        assert_eq!(
            delivery_state(&failed, &messages, 1, true, None, true),
            Some(Delivery::Failed)
        );
        // Read receipts off: nothing to wait for, an ACKed bubble is full at once.
        assert_eq!(
            delivery_state(&sent, &messages, 2, true, None, false),
            Some(Delivery::Seen)
        );
        // Someone else's bubble carries no delivery state at all.
        assert_eq!(delivery_state(&sent, &messages, 2, false, None, true), None);
    }

    #[test]
    fn build_rows_puts_the_state_on_my_bubbles_only() {
        let t = 1_700_000_000_000u64;
        let messages = vec![
            mine("a", t),
            message("o", "user", "user:gh_sam", "done", false, t + 1_000),
            mine("b", t + 2_000),
        ];
        let spoken = HashSet::new();
        let failed = HashMap::new();
        let speech = SpeechRows {
            enabled: false,
            spoken: &spoken,
            failed: &failed,
            voice_mode: false,
        };
        let receipts: HashMap<String, Id> = HashMap::from([("sam".to_owned(), "a".to_owned())]);
        let rows = build_rows(
            &messages,
            false,
            speech,
            "gh_me",
            &HashSet::new(),
            &Mentions::empty(),
            false,
            Some(&receipts),
            true,
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
            vec![Some(Delivery::Seen), None, Some(Delivery::Sent)],
            "mine: seen up to Sam's watermark, then sent; Sam's bubble: no state"
        );
    }

    /// The four states as the row draws them: one wrapper per own bubble named by
    /// state, no caption anywhere, and the red mark only beside a failed bubble.
    struct OwnBubbles;

    impl gpui::Render for OwnBubbles {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::dark(crate::ui::theme::Surface::Matte);
            let bubble = |text: &'static str| {
                div()
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
                    [
                        Delivery::Pending,
                        Delivery::Failed,
                        Delivery::Sent,
                        Delivery::Seen,
                    ]
                    .into_iter()
                    .map(|d| {
                        div()
                            .flex()
                            .justify_end()
                            .items_center()
                            .children(failed_glyph(d, &theme).map(|g| g.mr(px(8.0))))
                            .child(own_bubble(bubble("fish?"), d))
                    }),
                )
        }
    }

    #[gpui::test]
    fn every_state_draws_a_bubble_and_only_failed_gets_the_mark(cx: &mut TestAppContext) {
        let (_view, vcx) = cx.add_window_view(|_, _| OwnBubbles);
        vcx.run_until_parked();
        for sel in [
            "own-bubble-pending",
            "own-bubble-failed",
            "own-bubble-sent",
            "own-bubble-seen",
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
            .expect("red mark beside the failed bubble");
        let failed = vcx
            .debug_bounds("own-bubble-failed")
            .expect("failed bubble");
        assert!(
            f32::from(glyph.origin.x + glyph.size.width) <= f32::from(failed.origin.x),
            "the mark sits outside the bubble, on its outer edge"
        );
    }
}
