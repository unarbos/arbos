//! The chat column body: a bottom-aligned `list()` with one row per markdown
//! block (zeron-study §4.1–§4.4). Rows are diffed by `(id, version)` and
//! spliced; streaming text fades in through the veil.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use gpui::{
    actions, div, list, prelude::*, px, Animation, AnimationExt, AnyElement, App, Context, Div,
    Entity, FontWeight, ListAlignment, ListScrollEvent, ListState, Render, SharedString, Stateful,
    TextRun, Window,
};

use crate::app::{AppEvent, AppState, DetailsTarget};
use crate::model::{
    Attachment, AttachmentKind, Id, Message, MessageStatus, Part, Person, Role, StatusKind,
    TurnKind,
};
use crate::ui::approve::{approval_card, ApprovalView};
use crate::ui::attachments;
use crate::ui::cards;
use crate::ui::frost::edge_faded;
use crate::ui::groups;
use crate::ui::icons::icon;
use crate::ui::markdown::{inline_spans, polish_assistant_prose, split_blocks, Block};
use crate::ui::motion::PulseClock;
use crate::ui::people;
use crate::ui::select::{Selectable, TextSelection};
use crate::ui::theme::{
    Theme, BUBBLE_RADIUS, COLUMN_MARGIN, LINE_HEIGHT, TEXT_BODY, TEXT_SMALL, TEXT_TINY,
};
use crate::ui::veil::Veil;
use crate::ui::widgets::QuestionCard;
use serde_json::Value;

actions!(transcript, [Copy]);

const OVERDRAW_PX: f32 = 320.0;
pub const COLUMN_MAX_WIDTH: f32 = 820.0;
/// Assistant bubble cap (Grok: the bubble never spans the whole pane).
pub const ASSISTANT_MAX_WIDTH: f32 = 620.0;
/// A new time stamp line whenever messages are this far apart.
const STAMP_GAP_MS: u64 = 60 * 60 * 1000;
/// Messages for Mac fades a row's hover-only affordances (time, react glyph)
/// in and out rather than snapping them.
const HOVER_FADE_MS: f32 = 150.0;

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

#[derive(Clone, Debug)]
pub enum RowKind {
    User {
        text: String,
        pending: bool,
        /// Caption for prompts not typed by the user (a routine, another project).
        sender_label: Option<String>,
        /// Another person in a people chat (D72): left bubble with their
        /// avatar and handle instead of my right bubble.
        from: Option<Person>,
        /// "9:41 PM", revealed at the bubble's outer edge on hover.
        time: Option<String>,
    },
    Markdown {
        block: Block,
        live: bool,
        /// Where this block sits in its message's bubble (first/last round
        /// the corners; `fill` makes a multi-block bubble span the cap width
        /// so every block has the same edge).
        first: bool,
        last: bool,
        fill: bool,
        /// "9:41 PM", revealed next to the react glyph on hover.
        time: Option<String>,
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
    /// An image or file in the chat (agent or user). `mine` right-aligns my sends.
    Attachment {
        attachment: Attachment,
        mine: bool,
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
    /// The one working indicator for the chat: a pulsing dot under the last
    /// message while anything runs here. `queued` tints it amber.
    Indicator {
        queued: bool,
        /// "Cloning RaoFoundation/subtensor" — else "<name> is working".
        activity: Option<String>,
    },
    /// Debug-only "Show details" disclosure under a finished reply.
    Details,
    /// F6: who speaks next — another Project's glyph and name above its message.
    Sender {
        name: String,
        icon: Option<String>,
        /// Stable seed for the chip colour (the sender id).
        seed: String,
        /// "answering @sam · owned by @jacob" for a shared agent (D72).
        note: Option<String>,
        /// Tree-character slug the relay put on the reply (`senderAvatar`).
        avatar: Option<String>,
        /// Its colour (`senderAvatarColor`); `None` = look it up / mono.
        avatar_color: Option<String>,
    },
    /// F11: small speaker under a reply; click speaks it (again).
    Speaker {
        /// The speech is already cached (instant replay) vs. will be synthesised.
        cached: bool,
    },
}

/// Which replies get a speaker row (F11).
#[derive(Clone, Copy)]
pub struct SpeechRows<'a> {
    pub spoken: &'a HashSet<Id>,
    pub voice_mode: bool,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub id: String,
    pub version: u64,
    pub message_id: Id,
    pub kind: RowKind,
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

/// Right-click on a bubble: the shell opens the react menu there.
/// Hover reply: the shell sets the composer's reply target.
pub enum TranscriptEvent {
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
    veils: HashMap<String, Veil>,
    /// One stateful card per typed question, keyed by question id; dropped with its row.
    cards: HashMap<String, Entity<QuestionCard>>,
    pinned_to_bottom: bool,
    /// Message id we already scrolled to for the current reveal request.
    revealed: Option<Id>,
    spring: StickSpring,
    /// Set by the scroll callback; the next frame re-pins when the user
    /// stopped within `STICK_THRESHOLD_PX` of the end.
    repin_check_pending: bool,
    /// The working line's current text and when it changed (fade on swap).
    activity_shown: Option<(String, Instant)>,
    hover: HoverFade,
    /// Text selected in a bubble (drag / double-click / triple-click); ⌘C copies it.
    selection: Entity<TextSelection>,
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
            veils: HashMap::new(),
            cards: HashMap::new(),
            pinned_to_bottom: true,
            revealed: None,
            spring: StickSpring::default(),
            repin_check_pending: false,
            activity_shown: None,
            hover: HoverFade::default(),
            selection,
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
        let state = self.state.read(cx);
        let chat_id = state.active_chat_id();
        let chat_changed = chat_id != self.chat_id;
        let messages: Vec<Message> = chat_id
            .as_deref()
            .map(|id| state.messages_of(id).to_vec())
            .unwrap_or_default();
        self.chat_id = chat_id;

        let show_details = state.show_details;
        let speech = SpeechRows {
            spoken: &state.spoken,
            voice_mode: state.voice_mode,
        };
        let me_id = state.me_id();
        let thumb_keys: HashSet<&str> = state.thumbnails.keys().map(|k| k.as_str()).collect();
        let new_rows = build_rows(&messages, show_details, speech, &me_id, &thumb_keys);

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

    /// A home-search hit asked to land on a message. Scroll once, when its row exists.
    fn reveal_hit(&mut self, cx: &mut Context<Self>) {
        let Some((chat, msg)) = self.state.read(cx).reveal_message.clone() else {
            self.revealed = None;
            return;
        };
        if self.chat_id.as_deref() != Some(chat.as_str()) {
            return;
        }
        if self.revealed.as_deref() == Some(msg.as_str()) {
            return;
        }
        let Some(ix) = self.rows.iter().position(|r| {
            r.message_id == msg
                && !matches!(r.kind, RowKind::Stamp { .. } | RowKind::Indicator { .. })
        }) else {
            return;
        };
        self.pinned_to_bottom = false;
        self.list.scroll_to(gpui::ListOffset {
            item_ix: ix,
            offset_in_item: px(0.0),
        });
        self.revealed = Some(msg);
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

    fn copy_message(&self, message_id: &Id, cx: &mut Context<Self>) {
        if let Some(text) = self.message_plain(message_id, cx) {
            if !text.is_empty() {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            }
        }
    }

    /// Right-click → react menu. Control-click → copy the message text.
    fn react_target(
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
                cx.emit(TranscriptEvent::ReactMenu {
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

    /// Hover-only reply + copy glyphs beside a bubble (fade with the row).
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
        let btn = |id: SharedString, glyph: &'static str| {
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
                .child(icon(glyph, 16.0, theme.text_muted))
        };
        div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .child(
                btn(SharedString::from(format!("{row_id}#reply")), "reply").on_click(cx.listener(
                    move |_, _, _, cx| {
                        if let Some(chat_id) = chat_id.clone() {
                            cx.emit(TranscriptEvent::Reply {
                                chat_id,
                                message_id: reply_msg.clone(),
                            });
                        }
                    },
                )),
            )
            .child(
                btn(SharedString::from(format!("{row_id}#copy")), "copy")
                    .on_click(cx.listener(move |this, _, _, cx| this.copy_message(&copy_msg, cx))),
            )
    }

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        let theme = Theme::of(cx).clone();
        let now = Instant::now();
        let selectable = Selectable::new(&self.selection, theme.accent_soft, cx);
        let inner: AnyElement = match row.kind {
            RowKind::User {
                text,
                pending,
                sender_label,
                from,
                time,
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
                let text = plain_text(
                    &format!("{}.u", row.id),
                    &text,
                    text_color,
                    &theme,
                    &bubble_selectable,
                );
                let bubble = self
                    .react_target(
                        div().id(SharedString::from(format!("{}#bubble", row.id))),
                        &row.message_id,
                        cx,
                    )
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .max_w(relative_width(0.72))
                    .px(px(14.0))
                    .py(px(11.0))
                    .rounded(px(BUBBLE_RADIUS))
                    .bg(theme.bubble)
                    .text_color(theme.bubble_text())
                    .text_size(px(TEXT_BODY))
                    .line_height(px(LINE_HEIGHT))
                    .child(text);
                if let Some(person) = from {
                    // Someone else: left-aligned, their avatar and handle above.
                    // Must not early-return: the match arm still needs the
                    // COLUMN_MARGIN wrapper (same padding as agent bubbles).
                    let image = self.state.read(cx).avatar(&person.avatar_url);
                    let color = groups::sender_color(&person.handle, &theme);
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(px(3.0))
                        .py(px(6.0))
                        .child({
                            // Click the caption → the person's profile page.
                            let open_state = self.state.clone();
                            let handle = person.handle.clone();
                            let sid = self.chat_id.as_ref().and_then(|c| {
                                self.state.read(cx).chats.iter().find_map(|(pid, tabs)| {
                                    tabs.iter().any(|t| &t.id == c).then(|| pid.clone())
                                })
                            });
                            div()
                                .id(SharedString::from(format!("{}#who", row.id)))
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .pl(px(2.0))
                                .cursor_pointer()
                                .child(people::avatar(image, &person, 20.0, &theme))
                                .child(
                                    div()
                                        .text_size(px(TEXT_TINY))
                                        .text_color(theme.text_muted)
                                        .child(format!("@{}", person.handle)),
                                )
                                .on_click(move |_, _, cx| {
                                    let (handle, sid) = (handle.clone(), sid.clone());
                                    open_state.update(cx, |s, cx| {
                                        s.open_profile(
                                            crate::app::ProfilePage::User {
                                                handle,
                                                space_id: sid,
                                            },
                                            cx,
                                        )
                                    });
                                })
                        })
                        .child({
                            let (row_el, reveal) = self.hover_row(&row.id, now, window, cx);
                            let actions =
                                self.hover_reply_copy(&row.id, &row.message_id, reveal, &theme, cx);
                            row_el
                                .justify_start()
                                .child(
                                    bubble
                                        .bg(theme.bubble_assistant)
                                        .text_color(theme.text)
                                        .border_l_2()
                                        .border_color(Theme::with_alpha(color, 0.7))
                                        .when(pending, |el| el.opacity(0.6)),
                                )
                                .child(actions.ml(px(6.0)))
                                .children(hover_time(time, reveal, &theme).map(|t| t.ml(px(8.0))))
                        })
                        .into_any_element()
                } else {
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(3.0))
                        .py(px(6.0))
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
                first,
                last,
                fill,
                time,
            } => {
                let veil = if live {
                    self.veils.get(&row.id).cloned()
                } else {
                    None
                };
                let fading = veil.as_ref().map(|v| v.is_active(now)).unwrap_or(false);
                if live || fading {
                    PulseClock::request(cx);
                }
                // One assistant bubble, drawn block by block: the first block
                // rounds the top, the last the bottom, the rest run straight.
                let r = px(BUBBLE_RADIUS);
                let bubble = self
                    .react_target(
                        div().id(SharedString::from(format!("{}#bubble", row.id))),
                        &row.message_id,
                        cx,
                    )
                    .max_w(px(ASSISTANT_MAX_WIDTH))
                    .when(fill, |el| el.w_full())
                    .px(px(14.0))
                    .when(first, |el| el.pt(px(11.0)).rounded_t(r))
                    .when(!first, |el| el.pt(px(3.0)))
                    .when(last, |el| el.pb(px(11.0)).rounded_b(r))
                    .when(!last, |el| el.pb(px(3.0)))
                    .bg(theme.bubble_assistant)
                    .child(render_block(
                        &row.id,
                        &block,
                        veil.as_ref(),
                        now,
                        &theme,
                        &selectable,
                    ));
                let (row_el, reveal) = self.hover_row(&row.id, now, window, cx);
                // Hover: reply + copy, then Grok Mac's muted react glyph (opens
                // the same bar as a right-click).
                let hover_actions = (last && !live).then(|| {
                    let actions =
                        self.hover_reply_copy(&row.id, &row.message_id, reveal, &theme, cx);
                    let chat_id = self.chat_id.clone();
                    let message_id = row.message_id.clone();
                    let react = div()
                        .id(SharedString::from(format!("{}#react", row.id)))
                        .size(px(24.0))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .opacity(reveal)
                        .hover(|s| s.bg(theme.surface_2))
                        .child(icon("face-smile", 16.0, theme.text_muted))
                        .on_click(cx.listener(move |_, event: &gpui::ClickEvent, _, cx| {
                            if let Some(chat_id) = chat_id.clone() {
                                cx.emit(TranscriptEvent::ReactMenu {
                                    chat_id,
                                    message_id: message_id.clone(),
                                    position: event.position(),
                                });
                            }
                        }));
                    div()
                        .ml(px(6.0))
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(actions)
                        .child(react)
                });
                // Messages for Mac: the bubble's time shows beside the glyph on hover.
                let hover_time = (last && !live)
                    .then(|| hover_time(time, reveal, &theme).map(|t| t.ml(px(8.0))))
                    .flatten();
                row_el
                    .justify_start()
                    .when(first, |el| el.pt(px(6.0)))
                    .when(last, |el| el.pb(px(6.0)))
                    .child(bubble)
                    .children(hover_actions)
                    .children(hover_time)
                    .into_any_element()
            }
            RowKind::Stamp { label } => div()
                .w_full()
                .flex()
                .justify_center()
                .pt(px(14.0))
                .pb(px(8.0))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(label)
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
                    .pb(px(6.0))
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
            RowKind::Attachment { attachment, mine } => {
                let thumbnail = attachment
                    .blob_ref
                    .as_ref()
                    .and_then(|k| self.state.read(cx).thumbnails.get(k).cloned());
                div()
                    .w_full()
                    .py(px(4.0))
                    .flex()
                    .when(mine, |el| el.justify_end())
                    .child(attachments::card(
                        &row.id,
                        &attachment,
                        thumbnail,
                        self.state.clone(),
                        &theme,
                    ))
                    .into_any_element()
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
                div()
                    .w_full()
                    .flex()
                    .justify_start()
                    .child(card)
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
                approval_card(
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
                )
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
            RowKind::Indicator { queued, activity } => {
                PulseClock::request(cx);
                // A new activity fades in over 260 ms.
                let key = activity.clone().unwrap_or_default();
                let since = match &self.activity_shown {
                    Some((shown, at)) if *shown == key => *at,
                    _ => {
                        self.activity_shown = Some((key, now));
                        now
                    }
                };
                let fade = (since.elapsed().as_millis() as f32 / 260.0).clamp(0.0, 1.0);
                // "<name> is working" with the worker's picture (Grok's one line).
                let state = self.state.read(cx);
                let (name, picture): (String, Option<gpui::Div>) = match state.selected() {
                    Some(space) if space.is_people() => {
                        let chat_id = self.chat_id.clone().unwrap_or_default();
                        let agent = state
                            .messages_of(&chat_id)
                            .iter()
                            .rev()
                            .find(|m| m.role == Role::Assistant && m.is_streaming())
                            .and_then(|m| m.sender_handle.clone().or_else(|| m.sender_name.clone()))
                            .unwrap_or_else(|| "Agent".into());
                        let glyph = space
                            .agents
                            .iter()
                            .find(|a| a.handle == agent || a.name == agent)
                            .map(|a| people::agent_glyph(a, 20.0, &theme));
                        (agent, glyph)
                    }
                    Some(space) => (
                        space.name.clone(),
                        Some(crate::ui::avatars::project(space, 20.0, &theme)),
                    ),
                    None => ("Agent".into(), None),
                };
                cards::working_line(&name, picture, queued, activity.as_deref(), &theme, cx)
                    .opacity(fade)
                    .into_any_element()
            }
            RowKind::Sender {
                name,
                icon,
                seed,
                note,
                avatar,
                avatar_color,
            } => {
                let state = self.state.read(cx);
                // A shared agent's reply may carry only its handle: name, glyph
                // and colour come from the chat's member list or my projects.
                let agent = seed.strip_prefix("agent:").and_then(|pid| {
                    state
                        .selected()
                        .and_then(|space| space.agents.iter().find(|a| a.project_id == pid))
                        .map(|a| (a.name.clone(), a.icon.clone(), a.color.clone()))
                        .or_else(|| {
                            state
                                .projects
                                .iter()
                                .find(|p| p.id == pid)
                                .map(|p| (p.name.clone(), p.icon.clone(), p.color.clone()))
                        })
                });
                let (name, icon) = match &agent {
                    Some((n, i, _)) if name == "Agent" || icon.is_none() => {
                        (n.clone(), Some(i.clone()))
                    }
                    _ => (name, icon),
                };
                let color = agent
                    .as_ref()
                    .map(|(_, _, c)| groups::project_color(c, theme.accent))
                    .or_else(|| {
                        state
                            .projects
                            .iter()
                            .find(|p| format!("project:{}", p.id) == seed)
                            .map(|p| groups::project_color(&p.color, theme.accent))
                    })
                    .unwrap_or_else(|| groups::sender_color(&seed, &theme));
                // The sender's disc: a shared agent's own gradient, else my project's.
                let agent_pid_seed = seed
                    .strip_prefix("agent:")
                    .or_else(|| seed.strip_prefix("project:"))
                    .map(str::to_string);
                let avatar_color_index = agent_pid_seed.as_deref().and_then(|pid| {
                    state
                        .selected()
                        .and_then(|space| space.agents.iter().find(|a| a.project_id == pid))
                        .and_then(|a| a.avatar_color_index)
                        .or_else(|| {
                            state
                                .projects
                                .iter()
                                .find(|p| p.id == pid)
                                .and_then(|p| p.avatar_color_index)
                        })
                });
                let disc_seed = agent_pid_seed.clone().unwrap_or_else(|| seed.clone());
                let _ = (&avatar, &avatar_color, &color, &icon);
                // Click the chip → the agent's profile page.
                let agent_pid = agent_pid_seed;
                let open_state = self.state.clone();
                let chip = groups::sender_chip_with(&name, &disc_seed, avatar_color_index, &theme)
                    .id(SharedString::from(format!("{}#chip", row.id)))
                    .when(agent_pid.is_some(), |el| {
                        el.cursor_pointer().on_click(move |_, _, cx| {
                            if let Some(pid) = agent_pid.clone() {
                                open_state.update(cx, |s, cx| {
                                    s.open_profile(
                                        crate::app::ProfilePage::Agent { project_id: pid },
                                        cx,
                                    )
                                });
                            }
                        })
                    });
                match note {
                    Some(note) => chip
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(theme.text_faint)
                                .child(format!("· {note}")),
                        )
                        .into_any_element(),
                    None => chip.into_any_element(),
                }
            }
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
            RowKind::Speaker { cached } => {
                let state = self.state.clone();
                let message_id = row.message_id.clone();
                div()
                    .id(SharedString::from(row.id.clone()))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .w(px(96.0))
                    .pb(px(6.0))
                    .text_size(px(11.0))
                    .text_color(theme.text_faint)
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.text_muted))
                    .child(div().text_size(px(12.0)).child("▶"))
                    .child(if cached { "Replay" } else { "Speak" })
                    .on_click(move |_, _, cx| {
                        state.update(cx, |s, cx| s.replay_speech(message_id.clone(), cx));
                    })
                    .into_any_element()
            }
        };
        let highlighted =
            self.state.read(cx).highlight_message.as_deref() == Some(row.message_id.as_str());
        div()
            .w_full()
            .flex()
            .justify_center()
            .px(px(COLUMN_MARGIN))
            .when(highlighted, |el| {
                el.bg(Theme::with_alpha(theme.accent, 0.14))
            })
            .child(div().w_full().max_w(px(COLUMN_MAX_WIDTH)).child(inner))
            .into_any_element()
    }
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
            .child(edge_faded(
                24.0,
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

fn hash_version<T: Hash>(value: &T, live: bool) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h);
    (h.finish() << 1) | (live as u64)
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
) -> Vec<Row> {
    let mine = |m: &Message| {
        m.sender == "user" || m.sender.is_empty() || m.sender == format!("user:{me_id}")
    };
    let mut rows = Vec::new();
    let mut running = false;
    let mut queued = false;
    let last_reply = messages
        .iter()
        .rposition(|m| m.role == Role::Assistant && !m.is_streaming());
    let mut last_stamp_at: u64 = 0;
    for (mix, message) in messages.iter().enumerate() {
        let streaming = message.is_streaming();
        if message.created_at > 0 && message.created_at.saturating_sub(last_stamp_at) > STAMP_GAP_MS
        {
            last_stamp_at = message.created_at;
            let label = crate::ui::sidebar::stamp_label(message.created_at);
            rows.push(Row {
                version: hash_version(&label, false),
                id: format!("{}#stamp", message.id),
                message_id: message.id.clone(),
                kind: RowKind::Stamp { label },
            });
        }
        match message.role {
            Role::User => {
                let text = message.plain_text();
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
                // Image preview / file card rows (same cards as agent attachments).
                let is_mine = from.is_none();
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
                            ),
                            false,
                        ),
                        id: format!("{}#{}", message.id, part_id),
                        message_id: message.id.clone(),
                        kind: RowKind::Attachment {
                            attachment: attachment.clone(),
                            mine: is_mine,
                        },
                    });
                }
                // Skip a bare "Image"/"Photo" bubble when the attachment already shows it.
                let show_text = !text.is_empty() && !(has_image && is_generic_media_label(&text));
                if show_text || attachment_parts.is_empty() {
                    rows.push(Row {
                        version: hash_version(
                            &(
                                &text,
                                message.pending,
                                &sender_label,
                                from.as_ref().map(|p| &p.handle),
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
                            time: clock_label(message.created_at),
                        },
                    });
                }
                if message.status == MessageStatus::Queued {
                    queued = true;
                }
                push_reactions(&mut rows, message, mine(message));
            }
            Role::System => {
                // Call lines and other system notes: one centred muted line.
                let text = message.plain_text();
                let line = text.lines().next().unwrap_or_default().trim().to_string();
                if !line.is_empty() {
                    rows.push(Row {
                        version: hash_version(&line, false),
                        id: format!("{}#system", message.id),
                        message_id: message.id.clone(),
                        kind: RowKind::Stamp { label: line },
                    });
                }
            }
            Role::Assistant | Role::Project => {
                if streaming {
                    running = true;
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
                if message.role == Role::Project {
                    let name = message
                        .sender_name
                        .clone()
                        .or_else(|| message.sender.strip_prefix("project:").map(str::to_owned))
                        .unwrap_or_else(|| "Project".to_owned());
                    rows.push(Row {
                        version: hash_version(&(&name, &message.sender_icon), false),
                        id: format!("{}#sender", message.id),
                        message_id: message.id.clone(),
                        kind: RowKind::Sender {
                            name,
                            icon: message.sender_icon.clone(),
                            seed: message.sender.clone(),
                            note: None,
                            avatar: None,
                            avatar_color: None,
                        },
                    });
                } else if message.sender.starts_with("agent:") && message.asked_by_handle.is_some()
                {
                    // A shared agent in a people chat: name it and whom it answers.
                    let name = message
                        .sender_name
                        .clone()
                        .unwrap_or_else(|| "Agent".to_owned());
                    let mut note = format!(
                        "answering @{}",
                        message.asked_by_handle.clone().unwrap_or_default()
                    );
                    if let Some(owner) = &message.owner_handle {
                        note.push_str(&format!(" · owned by @{owner}"));
                    }
                    rows.push(Row {
                        version: hash_version(
                            &(
                                &name,
                                &message.sender_icon,
                                &note,
                                &message.sender_avatar_color,
                            ),
                            false,
                        ),
                        id: format!("{}#sender", message.id),
                        message_id: message.id.clone(),
                        kind: RowKind::Sender {
                            name,
                            icon: message.sender_icon.clone(),
                            seed: message.sender.clone(),
                            note: Some(note),
                            avatar: message
                                .sender_avatar
                                .clone()
                                .filter(|a| !a.is_empty() && !a.contains("://")),
                            avatar_color: message
                                .sender_avatar_color
                                .clone()
                                .filter(|a| !a.is_empty()),
                        },
                    });
                }
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
                        Part::Text { id, text } => Some((id.clone(), polish_assistant_prose(text))),
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
                let mut block_ix = 0usize;
                let mut had_prose = false;
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
                                rows.push(Row {
                                    version: hash_version(&(&block, first, last, fill), live),
                                    id: format!("{}#{}.{}", message.id, id, bix),
                                    message_id: message.id.clone(),
                                    kind: RowKind::Markdown {
                                        block,
                                        live,
                                        first,
                                        last,
                                        fill,
                                        time: clock_label(message.created_at),
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
                                    mine: false,
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
                        } => rows.push(Row {
                            version: hash_version(&(code, text), false),
                            id: format!("{}#{}", message.id, id),
                            message_id: message.id.clone(),
                            kind: RowKind::Error {
                                code: code.clone(),
                                message: text.clone(),
                            },
                        }),
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
                // Stopped / failed turns leave one tiny glyph, no words.
                let glyph = match message.status {
                    MessageStatus::Cancelled => Some("■"),
                    MessageStatus::Error
                        if !message
                            .parts
                            .iter()
                            .any(|p| matches!(p, Part::Error { .. })) =>
                    {
                        Some("✗")
                    }
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
                // Speaker (F11): every reply whose speech is cached, plus the
                // newest finished reply while voice mode is on.
                let cached = speech.spoken.contains(&message.id);
                if !streaming
                    && message.role == Role::Assistant
                    && had_prose
                    && (cached || (speech.voice_mode && last_reply == Some(mix)))
                {
                    rows.push(Row {
                        version: hash_version(&cached, false),
                        id: format!("{}#speaker", message.id),
                        message_id: message.id.clone(),
                        kind: RowKind::Speaker { cached },
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
    }
    if running || queued {
        let last = messages.last().map(|m| m.id.clone()).unwrap_or_default();
        let activity = messages
            .iter()
            .rev()
            .find(|m| m.is_streaming())
            .and_then(Message::current_activity);
        rows.push(Row {
            version: hash_version(&(queued, &activity), true),
            id: "chat#indicator".into(),
            message_id: last,
            kind: RowKind::Indicator { queued, activity },
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

/// Unstyled selectable text (a user bubble): one run in the body font.
fn plain_text(
    id: &str,
    text: &str,
    color: gpui::Hsla,
    theme: &Theme,
    selectable: &Selectable,
) -> AnyElement {
    let runs = vec![TextRun {
        len: text.len(),
        font: sans_font(theme),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }];
    selectable
        .text(id.to_string(), text.to_string(), runs)
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn styled_text(
    id: &str,
    text: &str,
    base_color: gpui::Hsla,
    theme: &Theme,
    veil: Option<&Veil>,
    now: Instant,
    offset: usize,
    selectable: &Selectable,
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
        Some(veil) => apply_veil(runs, &veil.spans(now), offset),
        None => runs,
    };
    selectable
        .text(id.to_string(), full, runs)
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
    veil: Option<&Veil>,
    now: Instant,
    theme: &Theme,
    selectable: &Selectable,
) -> AnyElement {
    let base = div()
        .w_full()
        .min_w_0()
        .text_color(theme.text)
        .text_size(px(TEXT_BODY))
        .line_height(px(LINE_HEIGHT));
    match block {
        Block::Paragraph(text) => base
            .child(styled_text(
                &format!("{row_id}.p"),
                text,
                theme.text,
                theme,
                veil,
                now,
                0,
                selectable,
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
                    &format!("{row_id}.h"),
                    text,
                    theme.text,
                    theme,
                    veil,
                    now,
                    0,
                    selectable,
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
                            &format!("{row_id}.li{i}"),
                            item,
                            theme.text,
                            theme,
                            veil,
                            now,
                            offset,
                            selectable,
                        ))),
                );
                offset += item.len() + 1;
            }
            col.into_any_element()
        }
        Block::Table { header, rows } => {
            let cols = header.len().max(1);
            let cell = |text: &str, head: bool, i: usize| {
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(10.0))
                    .py(px(6.0))
                    .when(i > 0, |el| el.border_l_1().border_color(theme.border))
                    .text_size(px(12.5))
                    .when(head, |el| el.font_weight(FontWeight::SEMIBOLD))
                    .text_color(if head { theme.text } else { theme.text_muted })
                    .child(text.to_string())
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
                head_row = head_row.child(cell(h, true, i));
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
                    r = r.child(cell(row.get(i).map(String::as_str).unwrap_or(""), false, i));
                }
                table = table.child(r);
            }
            table.into_any_element()
        }
        Block::Code { lang, code } => {
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
                        lang.clone()
                    })
                    .child({
                        // Copies the block (Grok's code header); the label is the hit target.
                        let source = code.clone();
                        div()
                            .id(SharedString::from(format!("{row_id}#copy")))
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.surface_2).text_color(theme.text))
                            .child("Copy")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    source.clone(),
                                ));
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
                color: theme.text,
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
                    .child(selectable.text(format!("{row_id}.code"), code.clone(), runs)),
            );
            card.into_any_element()
        }
    }
}
