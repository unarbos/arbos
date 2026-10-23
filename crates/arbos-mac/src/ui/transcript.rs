//! The chat column body: a bottom-aligned `list()` with one row per markdown
//! block (zeron-study §4.1–§4.4). Rows are diffed by `(id, version)` and
//! spliced; streaming text fades in through the veil.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    actions, div, list, prelude::*, px, Animation, AnimationExt, AnyElement, App, Context, Div,
    Entity, FontWeight, ListAlignment, ListScrollEvent, ListState, Render, SharedString, Stateful,
    TextRun, Window,
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
use crate::ui::mention::Mentions;
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
/// time) lives in that gap, so hovering never re-wraps the text.
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
/// Where another sender's bubble (or picture / file card) starts: the column
/// inset, the disc and its gap. Media rows pad the same way (`media_row`).
#[cfg(test)]
pub const SENDER_LANE: f32 = AVATAR_COLUMN_INSET + SENDER_DISC + SENDER_DISC_GAP;
/// Messages for Mac fades a row's hover-only affordances (time, react glyph)
/// in and out rather than snapping them.
const HOVER_FADE_MS: f32 = 150.0;
/// The fade band under the floating header (one number for every chat kind).
pub const HEADER_FADE_BAND: f32 = crate::ui::header::metrics::FADE_BAND;
/// How long the hover copy glyph stays a checkmark after a successful copy.
const COPY_FEEDBACK_MS: u64 = 2_000;

/// Which bubble row the pointer is over, and the one it just left, with the
/// moment each happened: the fade is computed from those at render time.
#[derive(Default)]
struct HoverFade {
    over: Option<(String, Instant)>,
    left: Option<(String, Instant)>,
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

/// First words of a message for its quote header: the whole message on one
/// line (newlines and markdown flattened), cut on a word boundary; attachments
/// name their file.
pub fn quote_text(message: &Message) -> String {
    message_line(message, QUOTE_CHARS)
}

/// Who sent a media row, mirroring the text rows' decoration.
#[derive(Clone, Debug, PartialEq)]
pub enum MediaSender {
    /// My send: right-aligned, no chrome (like my bubble).
    Mine,
    /// Another person in a people chat: their disc on the avatar column,
    /// their name when the chat names its senders (like `RowKind::User.from`).
    Person(Person),
    /// An agent in its own lane (people chat / Project): disc and name from the
    /// seed, like `RowKind::Markdown.lane`.
    Agent(String),
    /// The agent of a 1:1 agent chat: left-aligned, no disc, no name.
    Plain,
}

impl MediaSender {
    /// The row leaves the avatar column free (and the last row draws a disc).
    #[cfg(test)]
    pub fn has_lane(&self) -> bool {
        matches!(self, MediaSender::Person(_) | MediaSender::Agent(_))
    }
}

/// Who draws a media row's chrome: avatar, name, identity colour, `agent` tag, click → profile.
type MediaIdentity = (AnyElement, String, gpui::Hsla, bool, Box<dyn Fn(&mut App)>);

#[derive(Clone, Debug)]
pub enum RowKind {
    User {
        text: String,
        pending: bool,
        /// Caption for prompts not typed by the user (a routine, another project).
        sender_label: Option<String>,
        /// Another person's bubble carries their name inside it, at the top, when
        /// the chat names its senders (group / channel, see [`names_senders`]) and
        /// the sender changed (Grok Bot's group style for the run).
        named: bool,
        /// Another person in a people chat (D72): left bubble with their
        /// avatar and handle instead of my right bubble.
        from: Option<Person>,
        /// "9:41 PM", revealed at the bubble's outer edge on hover.
        time: Option<String>,
        /// Set when this message replies to another: Grok's quote header
        /// sits inside the bubble above the text.
        quote: Option<Quote>,
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
        /// "9:41 PM", revealed next to the react glyph on hover.
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
        /// The message ends with this block (no attachment follows): the
        /// sender's disc and the hover rail sit here. `last` alone only rounds
        /// the corners — a picture after the text takes the disc instead.
        tail: bool,
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
    /// An image or file in the chat (agent or user). Drawn with the same
    /// sender decoration as the message's text rows (feedback
    /// media-sender-attribution-2026-10-23): the avatar column, the sender's
    /// disc on the message's last row, the name above the first row in chats
    /// that name their senders, the hover rail on the last row.
    Attachment {
        attachment: Attachment,
        /// The `parts[]` id: the viewer opens on this picture of the album.
        part_id: Id,
        /// Who sent it — decides the side, the lane and the disc.
        sender: MediaSender,
        /// First row of a message whose sender is named (`names_senders`):
        /// the name sits above this picture / card, in the sender's colour.
        named: bool,
        /// Last row of its message: draws the disc and carries the hover rail.
        last: bool,
        /// "9:41 PM" for the hover rail / inline time.
        time: Option<String>,
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
    },
    /// Error: one line, red glyph, Retry.
    /// A typed question widget (design §10): rendered by a [`QuestionCard`] view.
    Question {
        question_id: String,
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

/// Right-click on a bubble: the shell opens the message menu there.
/// Hover face-smile: the shell opens the quick-react emoji bar there.
/// Hover reply: the shell sets the composer's reply target.
pub enum TranscriptEvent {
    MessageMenu {
        chat_id: Id,
        message_id: Id,
        position: gpui::Point<gpui::Pixels>,
    },
    /// Hover react glyph → the six-emoji bar (`react_menu`).
    ReactMenu {
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
    /// `ARBOS_RIG_SELECT=all|<from>..<to>|word@<i>|para@<i>` (screenshot rig):
    /// once the open chat shows a settled agent bubble, select that much of
    /// the newest one — the drag / double-click / triple-click the EC2 Mac's
    /// GUI session cannot receive from posted events. Not before
    /// `ARBOS_RIG_SELECT_AFTER_MS` (so a reply typed over VNC can land
    /// first). Cleared when applied.
    rig_select: Option<(String, Instant)>,
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
        }
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
        // end: the user is back at the tail. Direction is handled by the
        // wheel listener; this callback never re-pins on its own otherwise.
        if !event.is_scrolled {
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
        let messages: Vec<Message> = chat_id
            .as_deref()
            .map(|id| state.messages_of(id).to_vec())
            .unwrap_or_default();
        self.chat_id = chat_id;

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
        let new_rows = build_rows(
            &messages,
            show_details,
            speech,
            &me_id,
            &thumb_keys,
            &self.mentions,
            multi_party,
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
            new_rows
                .iter()
                .any(|r| matches!(&r.kind, RowKind::Question { question_id } if question_id == qid))
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

    /// Hover-only reply · copy · react glyphs beside a bubble (fade with the
    /// row). After a successful copy the glyph becomes a checkmark for a
    /// moment. The face-smile opens the six-emoji bar (`react_menu`).
    fn hover_reply_copy(
        &self,
        row_id: &str,
        message_id: &Id,
        opacity: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let chat_id = self.chat_id.clone();
        let reply_msg = message_id.clone();
        let copy_msg = message_id.clone();
        let react_chat = self.chat_id.clone();
        let react_msg = message_id.clone();
        let just_copied = self.copied_feedback.as_ref().is_some_and(|(id, at)| {
            id == message_id && at.elapsed() < Duration::from_millis(COPY_FEEDBACK_MS)
        });
        // Keep the checkmark readable even if the pointer leaves mid-feedback.
        let copy_opacity = if just_copied { 1.0 } else { opacity };
        let copy_glyph = if just_copied { "check" } else { "copy" };
        let copy_color = if just_copied {
            theme.online
        } else {
            theme.text_muted
        };
        let btn = |id: SharedString, glyph: &'static str, opacity: f32, color: gpui::Hsla| {
            div()
                .id(id)
                .size(px(24.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .opacity(opacity)
                .hover(|s| s.bg(theme.surface_2))
                .child(icon(glyph, 16.0, color))
        };
        div()
            .flex()
            .items_center()
            .gap(px(2.0))
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
                    SharedString::from(format!("{row_id}#copy")),
                    copy_glyph,
                    copy_opacity,
                    copy_color,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.copy_message(&copy_msg, cx))),
            )
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
                            cx.emit(TranscriptEvent::ReactMenu {
                                chat_id,
                                message_id: react_msg.clone(),
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

    /// `agent_identity` plus the D92 leaf pair in effect.
    fn agent_identity_full(&self, seed: &str, name: &str, cx: &Context<Self>) -> AgentIdentity {
        let state = self.state.read(cx);
        let agent_pid = seed
            .strip_prefix("agent:")
            .or_else(|| seed.strip_prefix("project:"))
            .map(str::to_string);
        let member = agent_pid.as_deref().and_then(|pid| {
            state
                .selected()
                .and_then(|space| space.agents.iter().find(|a| a.project_id == pid))
                .map(|a| {
                    (
                        a.name.clone(),
                        a.avatar_color_index,
                        (a.avatar_leaf, a.avatar_leaf_color),
                        a.image_url.clone(),
                    )
                })
                .or_else(|| {
                    state.projects.iter().find(|p| p.id == pid).map(|p| {
                        (
                            p.name.clone(),
                            p.avatar_color_index,
                            (p.avatar_leaf, p.avatar_leaf_color),
                            p.image_url.clone(),
                        )
                    })
                })
        });
        let resolved_name = match &member {
            Some((n, _, _, _)) if name == "Agent" || name == "Project" => n.clone(),
            _ => name.to_string(),
        };
        let color_index = member.as_ref().and_then(|(_, c, _, _)| *c);
        let leaf = member
            .as_ref()
            .map(|(_, _, l, _)| *l)
            .unwrap_or((None, None));
        // D92 amendment: the agent's uploaded / generated picture, once the loader has it.
        let image = member
            .as_ref()
            .and_then(|(_, _, _, url)| url.as_deref())
            .and_then(|url| state.avatar(url));
        let disc_seed = agent_pid.clone().unwrap_or_else(|| seed.to_string());
        AgentIdentity {
            name: resolved_name,
            seed: disc_seed,
            color_index,
            leaf,
            image,
            project_id: agent_pid,
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
        tail: bool,
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
        // colour, with the `agent` tag; the time after the last block's text.
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
        let inline_time = self.multi_party && lane.is_some() && tail && !live;
        let time_el = inline_time
            .then(|| {
                time.clone()
                    .map(|t| inline_time_label(t, theme).into_any_element())
            })
            .flatten();
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
                ),
                time_el,
            ));
        let (row_el, reveal) = self.hover_row(row_id, now, window, cx);
        // Hover: reply · copy · react (face-smile opens the emoji bar).
        let hover_actions = (tail && !live).then(|| {
            let actions = self.hover_reply_copy(row_id, message_id, reveal, theme, cx);
            div().flex().items_center().gap(px(2.0)).child(actions)
        });
        // Messages for Mac: the bubble's time shows beside the glyph on hover
        // (1:1 chats; a group / channel bubble already carries it after the text).
        let hover_time = (tail && !live && !inline_time)
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
            if tail {
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

    /// A picture / file card dressed like the message's text rows (feedback
    /// media-sender-attribution-2026-10-23): my sends sit right with the hover
    /// rail on the left; another sender's card sits on the avatar column with
    /// the sender's disc on the message's last row (earlier rows keep the lane
    /// free, like text blocks), the name above the first row in chats that
    /// name their senders (`sender_name_label` — the same label the bubbles
    /// use), the reply · copy rail and the time on the last row.
    #[allow(clippy::too_many_arguments)]
    fn media_row(
        &mut self,
        row_id: &str,
        message_id: &Id,
        card: AnyElement,
        sender: MediaSender,
        named: bool,
        last: bool,
        time: Option<String>,
        now: Instant,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (row_el, reveal) = self.hover_row(row_id, now, window, cx);
        let rail = last.then(|| {
            let actions = self.hover_reply_copy(row_id, message_id, reveal, theme, cx);
            let time = hover_time(time.clone(), reveal, theme);
            (actions, time)
        });
        if sender == MediaSender::Mine {
            // My bubble sits right, so its outer edge (and time) is on the left.
            return div()
                .w_full()
                .flex()
                .flex_col()
                .items_end()
                .py(px(ROW_GAP_HALF))
                .child(match rail {
                    Some((actions, time)) => row_el
                        .justify_end()
                        .items_center()
                        .children(time.map(|t| t.mr(px(8.0))))
                        .child(actions.mr(px(6.0)))
                        .child(card),
                    None => row_el.justify_end().child(card),
                })
                .into_any_element();
        }
        // Who: disc + name + colour from the person or the agent's seed.
        let identity: Option<MediaIdentity> = match &sender {
            MediaSender::Person(person) => {
                let state = self.state.read(cx);
                let image = state.avatar(&person.avatar_url);
                let color = state.sender_accent(person, theme);
                let open_state = self.state.clone();
                let handle = person.handle.clone();
                let sid = self.chat_id.as_ref().and_then(|c| {
                    state.chats.iter().find_map(|(pid, tabs)| {
                        tabs.iter().any(|t| &t.id == c).then(|| pid.clone())
                    })
                });
                let open = move |cx: &mut App| {
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
                };
                Some((
                    people::avatar(image, person, SENDER_DISC, theme).into_any_element(),
                    person.display_name(),
                    color,
                    false,
                    Box::new(open),
                ))
            }
            MediaSender::Agent(seed) => {
                let a = self.agent_identity_full(seed, "Agent", cx);
                let color =
                    avatars::agent_accent(&a.seed, a.color_index, a.leaf.0, a.leaf.1, theme);
                let open_state = self.state.clone();
                let pid = a.project_id.clone();
                let open = move |cx: &mut App| {
                    if let Some(pid) = pid.clone() {
                        open_state.update(cx, |s, cx| {
                            s.push_profile(crate::app::ProfilePage::Agent { project_id: pid }, cx)
                        });
                    }
                };
                Some((
                    self.agent_disc(seed, cx).into_any_element(),
                    a.name,
                    color,
                    true,
                    Box::new(open),
                ))
            }
            MediaSender::Mine | MediaSender::Plain => None,
        };
        let has_lane = identity.is_some();
        let (disc, name_el) = match identity {
            Some((avatar, name, color, agent, open)) => {
                let open = Rc::new(open);
                let disc: AnyElement = if last {
                    let open = open.clone();
                    div()
                        .id(SharedString::from(format!("{row_id}#disc")))
                        .flex_shrink_0()
                        .cursor_pointer()
                        .child(avatar)
                        .on_click(move |_, _, cx| open(cx))
                        .into_any_element()
                } else {
                    div().w(px(SENDER_DISC)).flex_shrink_0().into_any_element()
                };
                let name_el = named.then(|| {
                    self.sender_name_label(row_id, name, color, agent)
                        .cursor_pointer()
                        .on_click(move |_, _, cx| open(cx))
                });
                (Some(disc), name_el)
            }
            None => (None, None),
        };
        // The card, the name above it, and the rail hanging off its right edge.
        let stack = div()
            .relative()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(2.0))
            .children(name_el)
            .child(card)
            .children(rail.map(|(actions, time)| {
                div()
                    .absolute()
                    .left(relative_width(1.0))
                    .top_0()
                    .bottom_0()
                    .pl(px(6.0))
                    .flex()
                    .items_center()
                    .child(actions)
                    .children(time.map(|t| t.ml(px(8.0))))
            }));
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_start()
            .py(px(ROW_GAP_HALF))
            .child(
                row_el
                    .justify_start()
                    .items_end()
                    .when(has_lane, |el| {
                        el.pl(px(AVATAR_COLUMN_INSET)).gap(px(SENDER_DISC_GAP))
                    })
                    .children(disc)
                    .child(assistant_slot(stack)),
            )
            .into_any_element()
    }

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        let theme = Theme::of(cx).clone();
        let now = Instant::now();
        let selectable = Selectable::new(&self.selection, theme.accent_soft, cx).with_mentions(
            self.mentions.clone(),
            theme.accent,
            Theme::with_alpha(theme.accent, 0.14),
        );
        let inner: AnyElement = match row.kind {
            RowKind::User {
                text,
                pending,
                sender_label,
                from,
                named,
                time,
                quote,
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
                let quote_header = quote.map(|q| self.quote_header(&row.id, q, text_color));
                // Triple-backtick fences render as code cards (same as agent
                // replies); other text stays plain so "# heading" is still literal.
                // Bare `http(s)://…` URLs are still accented and clickable.
                let body =
                    user_message_body(&row.id, &text, text_color, &theme, &bubble_selectable);
                // Someone else's bubble in a group / channel: their name inside
                // it (their colour), the time after the text (`multi_party`).
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
                let inline_time = self.multi_party && from.is_some();
                let time_el = inline_time
                    .then(|| {
                        time.clone()
                            .map(|t| inline_time_label(t, &theme).into_any_element())
                    })
                    .flatten();
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
                        time_el,
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
                                self.hover_reply_copy(&row.id, &row.message_id, reveal, &theme, cx);
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
                                                .border_color(Theme::with_alpha(color, 0.7))
                                                .when(pending, |el| el.opacity(0.6)),
                                        )
                                        .child(actions.ml(px(6.0)))
                                        .children(
                                            (!inline_time)
                                                .then(|| {
                                                    hover_time(time, reveal, &theme)
                                                        .map(|t| t.ml(px(8.0)))
                                                })
                                                .flatten(),
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
                            // My bubble sits right, so its outer edge (and time) is on the left.
                            let (row_el, reveal) = self.hover_row(&row.id, now, window, cx);
                            let actions =
                                self.hover_reply_copy(&row.id, &row.message_id, reveal, &theme, cx);
                            row_el
                                .justify_end()
                                .children(hover_time(time, reveal, &theme).map(|t| t.mr(px(8.0))))
                                .child(actions.mr(px(6.0)))
                                .child(bubble.when(pending, |el| el.opacity(0.6)))
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
                tail,
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
                tail,
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
                    true,
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
                let state = self.state.read(cx);
                let lead = if total <= names.len() as u32 {
                    "Messaged".to_string()
                } else {
                    format!("{total} messages with")
                };
                let mut line = div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_muted)
                    .child(lead);
                let n = names.len();
                for (i, name) in names.iter().enumerate() {
                    let glyph: AnyElement = match state
                        .projects
                        .iter()
                        .find(|p| p.name.eq_ignore_ascii_case(name))
                    {
                        Some(space) => {
                            crate::ui::avatars::project(space, 16.0, &theme).into_any_element()
                        }
                        // Not one of my projects (a subagent): initials on the name's gradient.
                        None => crate::ui::avatars::element(name, name, None, 16.0, &theme)
                            .into_any_element(),
                    };
                    line = line.child(glyph).child(div().child(name.clone()));
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
            RowKind::Attachment {
                attachment,
                part_id,
                sender,
                named,
                last,
                time,
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
                let card = attachments::card(
                    &row.id,
                    &attachment,
                    preview,
                    forwarded_from.as_deref(),
                    open,
                    self.state.clone(),
                    &theme,
                );
                self.media_row(
                    &row.id,
                    &row.message_id,
                    card,
                    sender,
                    named,
                    last,
                    time,
                    now,
                    &theme,
                    window,
                    cx,
                )
            }
            RowKind::AskUser { text } => {
                cards::status_row(StatusKind::AskUser, &text, false, &theme, cx).into_any_element()
            }
            RowKind::Question { question_id } => {
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
                // Grok: the card is as wide as the assistant block.
                div()
                    .w_full()
                    .flex()
                    .child(assistant_slot(card))
                    .into_any_element()
            }
            RowKind::Approval { view } => {
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
                div()
                    .w_full()
                    .flex()
                    .child(assistant_slot(card))
                    .into_any_element()
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
        column_row(inner)
            .when(turn_start, |el| el.pt(px(TURN_GAP_EXTRA)))
            .when(highlighted, |el| {
                el.bg(Theme::with_alpha(theme.accent, 0.14))
            })
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

/// The 11 pt muted time beside a bubble, at the row's current hover opacity.
fn hover_time(time: Option<String>, opacity: f32, theme: &Theme) -> Option<Div> {
    time.map(|label| {
        div()
            .flex_shrink_0()
            .text_size(px(11.0))
            .text_color(theme.text_muted)
            .opacity(opacity)
            .child(label)
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
pub fn build_rows(
    messages: &[Message],
    show_details: bool,
    speech: SpeechRows<'_>,
    me_id: &str,
    thumb_keys: &HashSet<&str>,
    mentions: &Mentions,
    names_senders: bool,
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
                // Image preview / file card rows (same cards as agent attachments),
                // decorated like the text bubble: the sender's disc on the message's
                // last row, the name above its first row, my sends on the right.
                let is_mine = from.is_none();
                // A forwarded attachment's "Forwarded from …" line is the card's
                // hint, not a bubble of its own.
                let forwarded = (!attachment_parts.is_empty())
                    .then(|| attachments::forwarded_from(&text).map(str::to_owned))
                    .flatten();
                // Skip a bare "Image"/"Photo" bubble when the attachment already shows it.
                let show_text = !text.is_empty()
                    && !(has_image && is_generic_media_label(&text))
                    && forwarded.is_none();
                let text_row = show_text || attachment_parts.is_empty();
                let media_sender = match &from {
                    Some(person) => MediaSender::Person(person.clone()),
                    None => MediaSender::Mine,
                };
                let n_att = attachment_parts.len();
                for (i, (part_id, attachment)) in attachment_parts.iter().enumerate() {
                    let has_thumb = attachment
                        .blob_ref
                        .as_deref()
                        .is_some_and(|k| thumb_keys.contains(k));
                    let first_row = i == 0;
                    let last_row = i + 1 == n_att && !text_row;
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
                                named && first_row,
                                last_row,
                            ),
                            false,
                        ),
                        id: format!("{}#{}", message.id, part_id),
                        message_id: message.id.clone(),
                        kind: RowKind::Attachment {
                            attachment: attachment.clone(),
                            part_id: part_id.clone(),
                            sender: media_sender.clone(),
                            named: named && first_row,
                            last: last_row,
                            time: clock_label(message.created_at),
                            forwarded_from: forwarded.clone(),
                        },
                    });
                }
                // The name went above the first picture; the text bubble below it
                // does not repeat it.
                let named = named && attachment_parts.is_empty();
                if text_row {
                    let quote = message
                        .reply_to
                        .as_deref()
                        .filter(|id| !id.is_empty())
                        .map(|id| Quote::for_reply(id, messages));
                    rows.push(Row {
                        version: hash_version(
                            &(
                                &text,
                                message.pending,
                                &sender_label,
                                from.as_ref().map(|p| &p.handle),
                                &quote,
                            ),
                            false,
                        ),
                        id: format!("{}#user", message.id),
                        message_id: message.id.clone(),
                        kind: RowKind::User {
                            text,
                            pending: message.pending,
                            sender_label,
                            from,
                            named,
                            time: clock_label(message.created_at),
                            quote,
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
                // Quote the user message this answers: prefer `replyTo` (platform
                // reply feature); fall back to `inReplyTo` (turn link / older rows)
                // so consecutive replies do not look like one run (#1447).
                let reply_quote = message
                    .reply_to
                    .as_deref()
                    .filter(|id| !id.is_empty())
                    .or_else(|| message.in_reply_to.as_deref().filter(|id| !id.is_empty()))
                    .map(|id| Quote::for_reply(id, messages));
                if message.role == Role::Project || in_people {
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
                    && (in_people
                        || message.role == Role::Project
                        || prev_sender.as_deref() != Some(message.sender.as_str()));
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
                // The message's last visible part: a text part with words, or an
                // attachment. The disc and the hover rail go on that row.
                let last_visible_part =
                    message
                        .parts
                        .iter()
                        .enumerate()
                        .rev()
                        .find_map(|(i, p)| match p {
                            Part::Attachment { .. } => Some(i),
                            Part::Text { id, .. } => polished
                                .iter()
                                .find(|(pid, _)| pid == id)
                                .filter(|(_, t)| {
                                    split_blocks(t).into_iter().any(|b| !is_json_block(&b))
                                })
                                .map(|_| i),
                            _ => None,
                        });
                let ends_with_text = last_visible_part
                    .is_some_and(|i| matches!(message.parts[i], Part::Text { .. }));
                let mut block_ix = 0usize;
                let mut had_prose = false;
                let mut had_attachment = false;
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
                                let named = sender_named && first && !had_attachment;
                                let tail = last && ends_with_text;
                                rows.push(Row {
                                    version: hash_version(
                                        &(&block, first, last, fill, &reply_quote, named, tail),
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
                                        tail,
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
                            // Named when it is the message's first visible row
                            // (no text block before it); last when no text block
                            // or attachment follows — the disc sits there.
                            let first_row = block_ix == 0 && !had_attachment;
                            let last_row = Some(pix) == last_visible_part;
                            had_attachment = true;
                            rows.push(Row {
                                version: hash_version(
                                    &(
                                        &attachment.name,
                                        &attachment.title,
                                        attachment.bytes,
                                        &attachment.blob_ref,
                                        has_thumb,
                                        sender_named && first_row,
                                        last_row,
                                    ),
                                    false,
                                ),
                                id: format!("{}#{}", message.id, id),
                                message_id: message.id.clone(),
                                kind: RowKind::Attachment {
                                    attachment: attachment.clone(),
                                    part_id: id.clone(),
                                    sender: match &lane {
                                        Some(seed) => MediaSender::Agent(seed.clone()),
                                        None => MediaSender::Plain,
                                    },
                                    named: sender_named && first_row,
                                    last: last_row,
                                    time: clock_label(message.created_at),
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
                                kind: RowKind::Approval { view },
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
                if shows_nothing && finished && !streaming && (failed || only_empty) {
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
                            tail: true,
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

fn render_block(
    row_id: &str,
    block: &Block,
    at: usize,
    veil: Option<&Veil>,
    now: Instant,
    theme: &Theme,
    bubble: &BubbleSelectable,
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
            row_id, lang, code, theme.text, theme, bubble, starts[0], veil, now,
        ),
    }
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
            .child({
                // Copies the block (Grok's code header); the label is the hit target.
                let source = code.to_string();
                div()
                    .id(SharedString::from(format!("{row_id}#copy")))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_2).text_color(theme.text))
                    .child("Copy")
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(source.clone()));
                    })
            }),
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

/// Gap from the sender's name to the text (Jacob's reference: tight) and from
/// the reply quote header to the text (#1393).
const NAME_TO_TEXT_GAP: f32 = 2.0;
const QUOTE_TO_TEXT_GAP: f32 = 6.0;

/// The inside of a bubble, in this order: sender name (group / channel,
/// someone else) → reply quote header → the text, with the time after it.
/// A short text keeps the time on its line; a wrapped one pushes it to the
/// right of the next line. No `min_w_0` on the column or its items: taffy
/// sizes a flex column from its items, and an item with `min-width: 0`
/// contributes nothing, so the bubble would take the name's or the quote's
/// width and wrap the body one word per line (bubble-quote-width-2026-10-23).
///
/// The text + time line is a flex *row*, and there the rule is the opposite:
/// the text item must be `min_w_0`. GPUI measures a text's min-content width
/// as its unwrapped width (`AvailableSpace::MinContent` → no wrap width), so
/// without it the item's automatic minimum is the whole line, it cannot
/// shrink to the bubble, and the text runs out of the bubble's right edge
/// while the time drops to a line of its own (Jacob, 2026-10-23, v0.6.184:
/// bubble-overflow-hotfix-2026-10-23). With `min_w_0` the row still reports
/// the unwrapped width as its max-content size (a flex item's basis is its
/// content; the minimum only matters when shrinking), so a short bubble is
/// still content-sized, and a long one wraps at the bubble's width.
fn sender_bubble_column(
    name: Option<AnyElement>,
    quote: Option<AnyElement>,
    body: AnyElement,
    time: Option<AnyElement>,
) -> Div {
    div()
        .flex()
        .flex_col()
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
            div()
                .flex()
                .flex_wrap()
                .items_end()
                .child(
                    div()
                        .min_w_0()
                        .debug_selector(|| "bubble-text".into())
                        .child(body),
                )
                .children(time.map(|t| {
                    div()
                        .ml_auto()
                        .pl(px(8.0))
                        .debug_selector(|| "bubble-time".into())
                        .child(t)
                })),
        )
}

/// "8:05 AM" after a group / channel bubble's text.
fn inline_time_label(time: String, theme: &Theme) -> Div {
    div()
        .text_size(px(TEXT_TINY))
        .line_height(px(CHAT_LINE_HEIGHT))
        .text_color(theme.text_faint)
        .whitespace_nowrap()
        .child(time)
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
            div()
                .w(px(self.pane))
                .h(px(400.0))
                .flex()
                .flex_col()
                .child(column_row(row))
                .child(column_row(people_row))
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
                        Some(div().w(px(40.0)).h(px(CHAT_LINE_HEIGHT)).into_any_element()),
                    )),
            )
        }
    }

    /// Jacob, 2026-10-23 (Discord-style reference): inside a group / channel
    /// bubble the order is name → reply quote → text, 2 pt from the name to
    /// what follows, 6 from the quote; the time sits after the text.
    #[gpui::test]
    fn a_named_bubble_stacks_name_quote_text_and_puts_the_time_after(cx: &mut TestAppContext) {
        let (_view, vcx) = cx.add_window_view(|_, _| NamedBubble);
        vcx.run_until_parked();
        let name = vcx.debug_bounds("bubble-name").expect("name drawn");
        let quote = vcx.debug_bounds("bubble-quote").expect("quote drawn");
        let text = vcx.debug_bounds("bubble-text").expect("text drawn");
        let time = vcx.debug_bounds("bubble-time").expect("time drawn");
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
        assert_eq!(top(time), top(text), "the time shares the text's line");
        assert!(
            f32::from(time.origin.x) >= f32::from(text.origin.x + text.size.width) + 8.0,
            "the time follows the text"
        );
    }

    /// A quoted agent bubble as `markdown_bubble` builds it: the bubble is a
    /// flex column (quote header over the text) in the slot, with and without
    /// the avatar lane.
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
                .flex()
                .flex_col()
                .gap(px(6.0))
                .px(px(BUBBLE_PAD_X))
                .py(px(BUBBLE_PAD_Y))
                .child(quote)
                .child(render_block(
                    "q",
                    &Block::Paragraph(LONG_BODY.to_string()),
                    0,
                    None,
                    Instant::now(),
                    &theme,
                    &bubble_sel,
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
            let slot = vcx.debug_bounds("chat-bubble-slot").expect("slot drawn");
            let bw: f32 = bubble.size.width.into();
            let qw: f32 = quote.size.width.into();
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
        }
    }

    /// Which bubble a [`BubbleRow`] draws: my own (right, `RowKind::User`
    /// without `from`), another person's in a group (left, name + inline
    /// time, `RowKind::User` with `from`), or an agent's (`markdown_bubble`
    /// with a lane and the inline time).
    #[derive(Clone, Copy, Debug)]
    enum Who {
        Mine,
        Other,
        Agent,
    }

    /// One transcript row built from the same pieces the real rows use
    /// (`sender_bubble_column`, `user_message_body` / `render_block`,
    /// `inline_time_label`, `assistant_slot`, `column_row`) in a pane of
    /// `pane` pt.
    struct BubbleRow {
        pane: f32,
        who: Who,
        text: &'static str,
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
            let time = Some(inline_time_label("1:52 PM".to_string(), &theme).into_any_element());
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
                            ),
                            time,
                        ));
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
                    let body =
                        user_message_body("u", self.text, theme.bubble_text(), &theme, &selectable);
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
                            other.then_some(time).flatten(),
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
                                    .child(div().size(px(16.0)).ml(px(6.0))),
                            )
                    } else {
                        div()
                            .w_full()
                            .flex()
                            .items_end()
                            .justify_end()
                            .debug_selector(|| "test-bubble-parent".into())
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
    /// (the row's max-content width is the text plus the time, never the cap).
    #[gpui::test]
    fn a_short_bubble_is_as_wide_as_its_text(cx: &mut TestAppContext) {
        for who in [Who::Mine, Who::Other, Who::Agent] {
            let (_view, vcx) = cx.add_window_view(|_, cx| BubbleRow {
                pane: REFERENCE_PANE,
                who,
                text: SHORT_LINE,
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
            // The bubble is the text, the pads, and (groups) the time after it.
            let time_w = match who {
                Who::Mine => 0.0,
                Who::Other | Who::Agent => f32::from(
                    vcx.debug_bounds("bubble-time")
                        .expect("time drawn")
                        .size
                        .width,
                ),
            };
            assert!(
                (bw - (tw + time_w + 2.0 * BUBBLE_PAD_X)).abs() <= 2.0,
                "{who:?}: bubble {bw} ≠ text {tw} + time {time_w} + pads"
            );
        }
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
mod media_attribution_tests {
    //! feedback media-sender-attribution-2026-10-23 (#1426 follow-up): a picture or
    //! file row carries the same sender decoration as a text row — avatar column
    //! and disc, the name in chats that name their senders, none in 1:1 — and
    //! consecutive media from one sender group like text.
    use std::collections::{HashMap, HashSet};

    use super::*;

    fn with_photo(role: &str, sender: &str, text: &str, at: u64, photos: usize) -> Message {
        let mut parts: Vec<serde_json::Value> = (0..photos)
            .map(|i| {
                serde_json::json!({
                    "id": format!("a-{at}-{i}"),
                    "kind": "attachment",
                    "attachment": { "name": format!("scout-{i}.jpg"), "mime": "image/jpeg", "ref": format!("blob-{at}-{i}") }
                })
            })
            .collect();
        if !text.is_empty() {
            parts
                .push(serde_json::json!({ "id": format!("p-{at}"), "kind": "text", "text": text }));
        }
        let mut v = serde_json::json!({
            "id": format!("m-{at}"),
            "chatId": "chat_group",
            "role": role,
            "sender": sender,
            "status": "done",
            "createdAt": at,
            "parts": parts,
        });
        if sender.starts_with("user:") {
            v["senderHandle"] = "scout".into();
            v["senderName"] = "Scout".into();
        } else if sender.starts_with("agent:") {
            v["senderHandle"] = "scout".into();
            v["senderName"] = "Scout".into();
            v["askedByHandle"] = "jacob".into();
        }
        serde_json::from_value(v).expect("message")
    }

    fn rows(messages: &[Message], names_senders: bool) -> Vec<Row> {
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
            names_senders,
        )
    }

    fn media(rows: &[Row]) -> Vec<(&MediaSender, bool, bool)> {
        rows.iter()
            .filter_map(|r| match &r.kind {
                RowKind::Attachment {
                    sender,
                    named,
                    last,
                    ..
                } => Some((sender, *named, *last)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_persons_photo_in_a_group_is_named_and_sits_on_the_avatar_column() {
        let t = 1_700_000_000_000u64;
        let rows = rows(&[with_photo("user", "user:gh_scout", "", t, 1)], true);
        let m = media(&rows);
        assert_eq!(m.len(), 1);
        let (sender, named, last) = m[0];
        assert!(matches!(sender, MediaSender::Person(p) if p.handle == "scout"));
        assert!(sender.has_lane(), "the disc column, like a text bubble");
        assert!(
            named,
            "the name above the picture (group names its senders)"
        );
        assert!(last, "the only row draws the disc and carries the rail");
    }

    #[test]
    fn an_agents_photo_in_a_group_is_named_from_its_lane() {
        let t = 1_700_000_000_000u64;
        let rows = rows(&[with_photo("assistant", "agent:scout", "", t, 1)], true);
        let m = media(&rows);
        assert_eq!(m.len(), 1);
        assert!(matches!(m[0].0, MediaSender::Agent(seed) if seed == "agent:scout"));
        assert!(m[0].1 && m[0].2);
    }

    #[test]
    fn a_photo_in_a_one_to_one_chat_has_no_name_but_keeps_the_avatar() {
        let t = 1_700_000_000_000u64;
        // DM with a person: avatar column, no name (names_senders = false).
        let rows = rows(&[with_photo("user", "user:gh_scout", "", t, 1)], false);
        let m = media(&rows);
        assert!(matches!(m[0].0, MediaSender::Person(_)));
        assert!(!m[0].1, "1:1 chats show no name label");
        // Me ↔ agent: the agent's picture is plain — no lane, no name, like its bubble.
        let plain = rows_plain();
        assert_eq!(media(&plain)[0].0, &MediaSender::Plain);
        assert!(!media(&plain)[0].1);
    }

    fn rows_plain() -> Vec<Row> {
        let t = 1_700_000_000_000u64;
        let mut m = with_photo("assistant", "agent", "", t, 1);
        m.asked_by_handle = None;
        rows(&[m], false)
    }

    #[test]
    fn consecutive_photos_from_one_sender_group_like_text() {
        let t = 1_700_000_000_000u64;
        // Two photos in one message, then a text-only message from the same sender:
        // the name once (above the first picture), the disc on each message's last row.
        let rows = rows(
            &[
                with_photo("user", "user:gh_scout", "", t, 2),
                with_photo("user", "user:gh_scout", "and a caption", t + 1_000, 0),
            ],
            true,
        );
        let m = media(&rows);
        assert_eq!(
            m.iter().map(|(_, n, l)| (*n, *l)).collect::<Vec<_>>(),
            [(true, false), (false, true)]
        );
        let named_text: Vec<bool> = rows
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::User { named, .. } => Some(*named),
                _ => None,
            })
            .collect();
        assert_eq!(
            named_text,
            [false],
            "same sender run: the caption is not named again"
        );
    }

    #[test]
    fn a_photo_with_a_caption_puts_the_name_on_the_photo_and_the_disc_on_the_text() {
        let t = 1_700_000_000_000u64;
        let rows = rows(
            &[with_photo("user", "user:gh_scout", "look at this", t, 1)],
            true,
        );
        let m = media(&rows);
        assert_eq!((m[0].1, m[0].2), (true, false));
        let text_named: Vec<bool> = rows
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::User { named, .. } => Some(*named),
                _ => None,
            })
            .collect();
        assert_eq!(
            text_named,
            [false],
            "the name is above the picture, not repeated in the bubble"
        );
    }

    #[test]
    fn an_agents_text_then_photo_moves_the_disc_to_the_photo() {
        let t = 1_700_000_000_000u64;
        let mut m = with_photo("assistant", "agent:scout", "here it is", t, 1);
        // Text first, then the picture (the agent's usual order).
        m.parts.reverse();
        let rows = rows(&[m], true);
        let tails: Vec<bool> = rows
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::Markdown { tail, named, .. } => Some(*tail && !*named),
                _ => None,
            })
            .collect();
        assert_eq!(
            tails,
            [false],
            "the text block rounds its corners but the disc moves on"
        );
        let named_text = rows
            .iter()
            .any(|r| matches!(&r.kind, RowKind::Markdown { named: true, .. }));
        assert!(
            named_text,
            "the name stays on the first row — the text bubble"
        );
        let m = media(&rows);
        assert_eq!(
            (m[0].1, m[0].2),
            (false, true),
            "the picture is the message's tail"
        );
    }
}
