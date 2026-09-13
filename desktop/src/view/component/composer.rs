//! The composer: a growing field on a glass card, and a compact `/` menu
//! of app actions plus whatever the live agent advertises.

use crate::{
    model::{
        attachment::{Attachment, AttachmentDrafts, Prompt},
        session::{Command, Usage},
    },
    view::root,
};
use bezel::{
    gpui::{
        self, AnyElement, App, Context, Entity, EventEmitter, ExternalPaths, FocusHandle,
        Focusable, Hsla, KeyBinding, MouseDownEvent, MouseMoveEvent, PathPromptOptions, Pixels,
        Point, Render, ScrollHandle, SharedString, StyledText, TextRun, Window, actions, div, font,
        prelude::*, px,
    },
    theme::{Glass, SurfaceStyle, TextStyle, Theme, Typeset},
    ui::{
        icons,
        input::{self, FieldEvent, Shape, TextField},
        menu::Cursor,
        popover,
        tooltip::Tooltip,
        widgets::Controls as _,
    },
};
use std::path::PathBuf;

actions!(
    arbos_composer,
    [
        Send,
        CommandNext,
        CommandPrevious,
        CommandDismiss,
        CommandBackspace,
        CommandDelete
    ]
);

/// Claimed on top of `TextField`/`TextArea`, so `enter` sends here and stays a
/// newline in every other multi-line field.
const KEY_CONTEXT: &str = "ArbosComposer";
const MODEL_SEARCH_CONTEXT: &str = "ArbosModelSearch";

/// What the pill and the agent mark are cut from — and every card that floats
/// in the same stack over the transcript, which is why it is not private.
pub(crate) const SURFACE: SurfaceStyle = SurfaceStyle::Glass(Glass::Regular);

/// How full the context has to be before the meter says so in amber. Late
/// enough that it is not shouting through a normal conversation, early enough
/// to leave room to compact before a turn is refused.
const WARN_AT: f32 = 0.8;

/// How tall the `/` picker gets before it scrolls rather than grows. Rows are
/// one line, so this is a short stack, not a palette.
const PICKER_HEIGHT: f32 = 220.;

/// How wide the `/` picker sits. Compact: it floats over the field, it does
/// not stretch to the card.
const PICKER_WIDTH: f32 = 280.;

/// Model names are `Provider: Name (variant)` — wider than a slash slug.
const MODEL_PICKER_WIDTH: f32 = 320.;

pub fn init(cx: &mut App) {
    crate::view::bind_field_editing(cx, KEY_CONTEXT, true);
    crate::view::bind_field_editing(cx, MODEL_SEARCH_CONTEXT, false);
    let ctx = Some(KEY_CONTEXT);
    let search = Some(MODEL_SEARCH_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("enter", Send, ctx),
        // Bound explicitly: the field's own `enter` is what usually inserts a
        // newline, and the composer has just taken it.
        KeyBinding::new("shift-enter", input::InsertNewline, ctx),
        KeyBinding::new("down", CommandNext, ctx),
        KeyBinding::new("up", CommandPrevious, ctx),
        KeyBinding::new("escape", CommandDismiss, ctx),
        // After the field's own erase bindings, so an empty composer can
        // hand the key to the sidebar instead of swallowing it.
        KeyBinding::new("backspace", CommandBackspace, ctx),
        KeyBinding::new("delete", CommandDelete, ctx),
        KeyBinding::new("enter", Send, search),
        KeyBinding::new("down", CommandNext, search),
        KeyBinding::new("up", CommandPrevious, search),
        KeyBinding::new("escape", CommandDismiss, search),
    ]);
}

/// One agent on offer: what to call it, and the registry's mark for it when
/// the catalog knows it.
#[derive(Clone, PartialEq)]
pub struct Agent {
    pub name: SharedString,
    pub icon: Option<SharedString>,
}

/// Which request a [`Switch`] is, since the agent offers two shapes of the
/// same idea.
#[derive(Clone, PartialEq, Eq)]
pub enum SwitchId {
    /// The kernel's permission mode (auto / ask / plan), or an ACP agent's
    /// session mode.
    Mode,
    /// Leftover ACP config switch. Unused: the model is [`SwitchId::Model`].
    Config(SharedString),
    /// Kernel `set_model` — the provider catalog.
    Model,
}

/// One value a switch can be set to.
#[derive(Clone, PartialEq)]
pub struct SwitchOption {
    pub id: SharedString,
    pub name: SharedString,
}

/// One switchable thing the session offers: the agent's mode, or a config
/// option. One shape for both — the composer shows a value and reports a pick,
/// and which request that is stays the session's business.
#[derive(Clone, PartialEq)]
pub struct Switch {
    pub id: SwitchId,
    /// What the thing is called, shown when it is on a value the agent no
    /// longer offers — which happens when an update lands mid-pick.
    pub name: SharedString,
    pub current: Option<SharedString>,
    pub options: Vec<SwitchOption>,
}

#[derive(Clone)]
pub enum ComposerEvent {
    Submit(Prompt),
    /// Stop the in-flight turn and send this plus the queue, now.
    Force(Prompt),
    Cancel,
    /// Leftover. The chip is a model picker; this is never emitted.
    Agent(usize),
    /// Leftover. Catalog ACP agents are not installed as chat runtimes.
    /// Set a switch to one of its values, by id.
    Switch(SwitchId, SharedString),
    Voice,
    Attach,
    /// No slash or model menu: the arrow keys step the sidebar.
    Step(isize),
    /// Field empty, no slash or model menu: Delete/Backspace archives or
    /// kernel-deletes the highlighted chat.
    Delete,
}

/// One skill or slash template the `/` menu can offer.
#[derive(Clone)]
struct SlashEntry {
    slug: SharedString,
    description: SharedString,
}

/// A pasted chat link, shown as a chip. `markdown` is what send writes.
#[derive(Clone)]
struct ChatChip {
    title: SharedString,
    markdown: String,
}

/// A sidebar session on its way to another row or the composer. Dropped on
/// a sibling it reorders; dropped on the composer it becomes a chip.
#[derive(Clone)]
pub(crate) struct SessionDrag {
    pub id: u64,
    pub markdown: SharedString,
}

fn join_chat_links(chips: &[ChatChip], text: &str) -> String {
    let mut parts: Vec<String> = chips.iter().map(|chip| chip.markdown.clone()).collect();
    let text = text.trim();
    if !text.is_empty() {
        parts.push(text.to_string());
    }
    parts.join("\n\n")
}

/// Pull `[title](arbos://chat/…)` and bare `arbos://chat/…` out of `text`.
fn take_chat_links(text: &str) -> (String, Vec<ChatChip>) {
    let mut rest = String::new();
    let mut chips = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with('[')
            && let Some((chip, used)) = parse_md_chat_link(&text[i..])
        {
            chips.push(chip);
            i += used;
            continue;
        }
        if text[i..].starts_with("arbos://chat/")
            && let Some((chip, used)) = parse_bare_chat_link(&text[i..])
        {
            chips.push(chip);
            i += used;
            continue;
        }
        let next = text[i..]
            .chars()
            .next()
            .map(|ch| ch.len_utf8())
            .unwrap_or(1);
        rest.push_str(&text[i..i + next]);
        i += next;
    }
    (collapse_spaces(&rest), chips)
}

fn parse_md_chat_link(s: &str) -> Option<(ChatChip, usize)> {
    let close = s.find("](")?;
    let title = s.get(1..close)?.replace("\\]", "]").replace("\\\\", "\\");
    let after = s.get(close + 2..)?;
    if !after.starts_with("arbos://chat/") {
        return None;
    }
    let end = after.find(')')?;
    let url = after[..end].split_whitespace().next()?;
    if !url.starts_with("arbos://chat/") {
        return None;
    }
    let used = close + 2 + end + 1;
    let markdown = s.get(..used)?.to_string();
    let title = title.trim();
    let title = if title.is_empty() { "chat" } else { title };
    Some((
        ChatChip {
            title: title.to_string().into(),
            markdown,
        },
        used,
    ))
}

fn parse_bare_chat_link(s: &str) -> Option<(ChatChip, usize)> {
    let rest = s.strip_prefix("arbos://chat/")?;
    let used = rest
        .find(|ch: char| ch.is_whitespace() || ch == ')')
        .unwrap_or(rest.len());
    let url = &s[.."arbos://chat/".len() + used];
    if url.len() <= "arbos://chat/".len() {
        return None;
    }
    Some((
        ChatChip {
            title: "chat".into(),
            markdown: format!("[{}]({url} \"chip\")", "chat"),
        },
        url.len(),
    ))
}

fn floor_char(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Transcript to drop after `before`. A leading space when the field already
/// has a word, so "hello" + "world" becomes "hello world".
fn voice_piece(before: &str, transcript: &str) -> String {
    let text = transcript.trim();
    if text.is_empty() {
        return String::new();
    }
    if before.is_empty() || before.ends_with(char::is_whitespace) {
        text.to_string()
    } else {
        format!(" {text}")
    }
}

fn collapse_spaces(s: &str) -> String {
    let t = s
        .lines()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join("\n");
    t.trim().to_string()
}

fn slash_entries(commands: &[Command]) -> Vec<SlashEntry> {
    commands
        .iter()
        .map(|command| SlashEntry {
            slug: command.name.clone().into(),
            description: command.description.clone().into(),
        })
        .collect()
}

/// The `/` token the caret is in: its start, and the text after the slash.
/// Opens at the start of the field or after whitespace — not mid-word.
/// Rank a model against a search query. Every whitespace token must appear
/// in the visible name or the catalog id (case-insensitive). Prefix of the
/// name ranks first, then any other hit.
fn model_rank(query: &str, option: &SwitchOption) -> Option<usize> {
    let query = query.trim().to_lowercase();
    let haystack = format!("{} {}", option.name, option.id).to_lowercase();
    if query.is_empty() {
        return Some(1);
    }
    let tokens: Vec<&str> = query.split_whitespace().collect();
    if !tokens.iter().all(|token| haystack.contains(token)) {
        return None;
    }
    if haystack.starts_with(tokens[0]) {
        Some(0)
    } else {
        Some(1)
    }
}

fn slash_query(content: &str, caret: usize) -> Option<(usize, &str)> {
    let caret = caret.min(content.len());
    let before = content.get(..caret)?;
    let start = before
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
        .map(|(i, ch)| i + ch.len_utf8())
        .unwrap_or(0);
    let token = content.get(start..caret)?;
    token.strip_prefix('/').map(|query| (start, query))
}

/// What the mic is doing. The kernel owns the capture; this is only paint
/// and whether a second click stops it.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum VoiceState {
    #[default]
    Idle,
    Recording,
    Busy,
}

pub struct Composer {
    field: Entity<TextField>,
    /// Byte offset of the `/` being typed, or `None` when no picker is open.
    /// Derived from the text on every change rather than stored as a flag: a
    /// backspace over the `/` has to close the picker, and a flag would have to
    /// be told.
    command: Option<usize>,
    /// Escape dismissed this `/` token. Cleared when the token is gone, so a
    /// later `/` can open the menu again.
    slash_dismissed: bool,
    /// Last query handed to [`Filter::refilter`]. The field notifies on
    /// caret blink and on keys the picker already ate; re-ranking then
    /// would put the highlight back on the first row.
    slash_query: String,
    /// Last pointer we saw over a row. A layout pass repeats the same
    /// point and must not steal a keyboard highlight.
    slash_pointer: Option<Point<Pixels>>,
    filter: popover::Filter,
    /// Agent-advertised commands, kept so a no-op update does not rebuild.
    commands: Vec<Command>,
    /// App actions first, then the agent's, in the filter's own order.
    entries: Vec<SlashEntry>,
    /// Where the picker's list sits, since the card clamps at
    /// [`PICKER_HEIGHT`]: arrowing past the last visible row has to bring
    /// the row it landed on back into view.
    scroll: ScrollHandle,
    /// Whether a turn is in flight — what the button does when pressed.
    streaming: bool,
    /// A follow-up is sitting above the card, waiting for this turn.
    queued: bool,
    /// The configured agents, and which one the session runs on.
    agents: Vec<Agent>,
    agent: Option<usize>,
    /// What the live session can be switched between — its mode, its model.
    /// Empty for a session with no agent behind it.
    switches: Vec<Switch>,
    /// Empty-state line when the catalog did not arrive (e.g. gateway down).
    model_note: String,
    /// Context spent, when the agent counts it.
    usage: Option<Usage>,
    /// Whether the agent mark's menu is up.
    menu: bool,
    /// Where that menu is being worked.
    cursor: Cursor,
    /// Search field inside the model picker. Focused while that menu is up.
    model_search: Entity<TextField>,
    /// Last query we ranked. Same caret-blink guard as the slash list.
    model_query: String,
    /// Original option indices that match [`Self::model_query`], ranked.
    model_hits: Vec<usize>,
    /// Highlighted row in [`Self::model_hits`], not in the full catalog.
    model_active: usize,
    /// Last pointer over a model row. Same guard as the slash list.
    model_pointer: Option<Point<Pixels>>,
    /// Files sitting on the card, waiting to go out with the next send.
    attachments: AttachmentDrafts,
    /// Chat links lifted out of the field so a paste shows a chip, not
    /// the markdown. The raw string goes back on send.
    chat_links: Vec<ChatChip>,
    voice: VoiceState,
    /// Partial transcript shown muted after the caret while the mic is down.
    voice_preview: String,
    /// Byte offset in the field where this take should land. Snapshotted
    /// when recording starts so later peek updates stay at the caret.
    voice_at: usize,
    /// A lost connection can be woken by an empty send.
    reconnect: bool,
    /// Hint shown when the field is empty. Cursor keeps "Send follow-up"
    /// visible on an empty focused composer; the caret sits at the start.
    hint: SharedString,
    /// Last string written into the field, so a paint does not notify
    /// on every frame and loop.
    painted_hint: SharedString,
    /// Focus listeners are attached once we have a window.
    watching_focus: bool,
    /// Session whose draft is in the field. None before the first bind.
    bound: Option<u64>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| {
            TextField::new(cx)
                .with_shape(Shape::Grow {
                    min: root::COMPOSER_FIELD_MIN as usize,
                    max: 10,
                })
                .with_frame(false)
                .with_key_context(KEY_CONTEXT)
                .with_placeholder("")
        });
        cx.observe(&field, |composer: &mut Self, _, cx| composer.reread(cx))
            .detach();
        let model_search = cx.new(|cx| {
            TextField::new(cx)
                .with_frame(false)
                .with_key_context(MODEL_SEARCH_CONTEXT)
                .with_placeholder("Search models")
        });
        cx.subscribe(
            &model_search,
            |this: &mut Self, _, event: &FieldEvent, cx| {
                if *event != FieldEvent::Changed {
                    return;
                }
                this.on_model_query(cx);
            },
        )
        .detach();
        let entries = slash_entries(&[]);
        let filter = popover::Filter::new(entries.iter().map(|entry| entry.slug.clone()).collect());
        Self {
            field,
            model_search,
            command: None,
            slash_dismissed: false,
            slash_query: String::new(),
            slash_pointer: None,
            filter,
            commands: Vec::new(),
            entries,
            scroll: ScrollHandle::new(),
            streaming: false,
            queued: false,
            agents: Vec::new(),
            agent: None,
            switches: Vec::new(),
            model_note: String::new(),
            usage: None,
            menu: false,
            cursor: Cursor::default(),
            model_query: String::new(),
            model_hits: Vec::new(),
            model_active: 0,
            model_pointer: None,
            attachments: AttachmentDrafts::default(),
            chat_links: Vec::new(),
            voice: VoiceState::Idle,
            voice_preview: String::new(),
            voice_at: 0,
            reconnect: false,
            hint: "Send follow-up".into(),
            painted_hint: "".into(),
            watching_focus: false,
            bound: None,
        }
    }

    pub fn content(&self, cx: &App) -> String {
        join_chat_links(&self.chat_links, &self.field.read(cx).content())
    }

    pub fn bound(&self) -> Option<u64> {
        self.bound
    }

    /// ChatView's pencil: put a past prompt in the field so the user can
    /// edit it, then send. Does not submit.
    pub fn replace_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.field
            .update(cx, |field, cx| field.set_content(text.to_string(), cx));
        cx.notify();
    }

    pub fn text_field(&self) -> Entity<TextField> {
        self.field.clone()
    }

    /// Point the field at another session's draft. The caller saved the
    /// previous text first.
    pub fn bind_session(&mut self, id: Option<u64>, draft: &str, cx: &mut Context<Self>) {
        if self.bound == id {
            return;
        }
        self.bound = id;
        self.chat_links.clear();
        self.voice_preview.clear();
        self.field
            .update(cx, |field, cx| field.set_content(draft.to_string(), cx));
        self.command = None;
        self.slash_dismissed = false;
        self.slash_query.clear();
        self.slash_pointer = None;
        self.close_menu(cx);
        cx.notify();
    }

    /// Replace the field's text for the bound session (a follow-up taken
    /// back, a rewind handing the prompt back).
    pub fn take_draft(&mut self, draft: &str, cx: &mut Context<Self>) {
        self.voice_preview.clear();
        self.field
            .update(cx, |field, cx| field.set_content(draft.to_string(), cx));
        self.command = None;
        self.close_menu(cx);
        cx.notify();
    }

    pub fn set_placeholder(&mut self, placeholder: &str, cx: &mut Context<Self>) {
        if self.hint == placeholder {
            return;
        }
        self.hint = placeholder.into();
        cx.notify();
    }

    /// Keep Bezel's own placeholder empty. The visible hint is painted
    /// beside the field so a focused caret never sits on the first letter.
    fn paint_placeholder(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.painted_hint.is_empty() {
            return;
        }
        self.painted_hint = "".into();
        self.field
            .update(cx, |field, cx| field.set_placeholder("", cx));
    }

    /// Skills and slash templates for the open project. The picker stays
    /// up across a catalog change: closing it here would swallow the menu
    /// the moment the list first landed.
    pub fn set_commands(&mut self, commands: &[Command], cx: &mut Context<Self>) {
        if self.commands == commands {
            return;
        }
        self.commands = commands.to_vec();
        self.entries = slash_entries(commands);
        self.filter = popover::Filter::new(
            self.entries
                .iter()
                .map(|entry| entry.slug.clone())
                .collect(),
        );
        if self.command.is_some() {
            let content = self.field.read(cx).content().clone();
            let caret = self.field.read(cx).cursor().min(content.len());
            if let Some((_, query)) = slash_query(&content, caret) {
                self.filter.refilter(query);
                self.reveal();
            }
        }
        cx.notify();
    }

    pub fn set_streaming(&mut self, streaming: bool, cx: &mut Context<Self>) {
        if self.streaming != streaming {
            self.streaming = streaming;
            cx.notify();
        }
    }

    pub fn set_queued(&mut self, queued: bool, cx: &mut Context<Self>) {
        if self.queued != queued {
            self.queued = queued;
            cx.notify();
        }
    }

    pub fn set_reconnect(&mut self, reconnect: bool, cx: &mut Context<Self>) {
        if self.reconnect != reconnect {
            self.reconnect = reconnect;
            cx.notify();
        }
    }

    /// The agents on offer, and the one the session is talking to.
    pub fn set_agents(&mut self, agents: &[Agent], current: Option<usize>, cx: &mut Context<Self>) {
        if self.agents == agents && self.agent == current {
            return;
        }
        self.agents = agents.to_vec();
        self.agent = current;
        cx.notify();
    }

    /// Why the model list is empty, when the gateway said so.
    pub fn set_model_note(&mut self, note: &str, cx: &mut Context<Self>) {
        if self.model_note == note {
            return;
        }
        self.model_note = note.to_string();
        cx.notify();
    }

    /// What the session can be switched between, and how much context it has
    /// spent. Both belong to a live connection, so both go empty with one.
    pub fn set_switches(&mut self, switches: &[Switch], cx: &mut Context<Self>) {
        if self.switches == switches {
            return;
        }
        self.switches = switches.to_vec();
        if self.menu {
            let query = self.model_query.clone();
            self.rebuild_model_hits(&query);
            if self.model_hits.is_empty() {
                self.model_active = 0;
            } else if self.model_active >= self.model_hits.len() {
                self.model_active = self.model_hits.len() - 1;
            }
        }
        cx.notify();
    }

    pub fn set_usage(&mut self, usage: Option<Usage>, cx: &mut Context<Self>) {
        let same = match (self.usage, usage) {
            (Some(held), Some(next)) => {
                held.used == next.used && held.size == next.size && held.spent == next.spent
            }
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.usage = usage;
        cx.notify();
    }

    pub fn is_empty(&self, cx: &App) -> bool {
        self.field.read(cx).content().trim().is_empty()
            && self
                .attachments
                .get(self.bound)
                .is_none_or(|tray| tray.items.is_empty() && tray.loading == 0)
            && self.chat_links.is_empty()
    }

    pub fn is_recording(&self) -> bool {
        self.voice == VoiceState::Recording
    }

    pub fn voice_busy(&self) -> bool {
        self.voice == VoiceState::Busy
    }

    pub fn set_voice(&mut self, voice: VoiceState, cx: &mut Context<Self>) {
        if self.voice == voice {
            return;
        }
        if voice == VoiceState::Recording {
            let content = self.field.read(cx).content();
            self.voice_at = floor_char(&content, self.field.read(cx).cursor());
            self.voice_preview.clear();
        }
        if voice == VoiceState::Idle {
            self.voice_preview.clear();
        }
        self.voice = voice;
        cx.notify();
    }

    /// Live words from the kernel, shown muted after the caret until release.
    pub fn set_voice_preview(&mut self, text: &str, cx: &mut Context<Self>) {
        let next = text.trim().to_string();
        if self.voice_preview == next {
            return;
        }
        self.voice_preview = next;
        cx.notify();
    }

    /// Put a chat-link on the card the same way a paste does: the field
    /// lifts `[title](arbos://chat/…)` into a chip.
    pub fn accept_chat_link(
        &mut self,
        markdown: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.insert_text(markdown, cx);
        window.focus(&self.field.read(cx).focus_handle(cx), cx);
    }

    /// Hold-Fn (and the mic) land here: put the words in the field and send
    /// them, the same two-step as the web composer's dictation final.
    /// Dictation that stays in the field: the words are shown, not sent
    /// (a duplex voice server already answered them).
    pub fn dictation_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.close_menu(cx);
        self.voice_preview.clear();
        self.insert_text_at(self.voice_at, text, cx);
    }

    pub fn dictation_final(&mut self, text: &str, cx: &mut Context<Self>) {
        self.close_menu(cx);
        self.command = None;
        self.voice_preview.clear();
        self.insert_text_at(self.voice_at, text, cx);
        if self.is_empty(cx) {
            return;
        }
        self.submit(cx);
    }

    /// Drop dictation at the caret. A space separates it from whatever was
    /// already typed, so two takes do not glue into one word.
    pub fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let at = floor_char(&self.field.read(cx).content(), self.field.read(cx).cursor());
        self.insert_text_at(at, text, cx);
    }

    /// Double-click the send field — or an overlay sitting on it — to take
    /// the whole buffer. `TextField`'s own press parks a caret in the word
    /// on this tick, so SelectAll waits a frame, same as heading rename.
    fn select_all_text(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.field.read(cx).content().is_empty() {
            return;
        }
        window.focus(&self.field.read(cx).focus_handle(cx), cx);
        cx.spawn_in(window, async move |this, cx| {
            let _ = this.update_in(cx, |this, window, cx| {
                this.dispatch_select_all(window, cx);
            });
        })
        .detach();
        cx.on_next_frame(window, |this, window, cx| {
            this.dispatch_select_all(window, cx);
        });
    }

    fn dispatch_select_all(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.field.read(cx).content().is_empty() {
            return;
        }
        window.focus(&self.field.read(cx).focus_handle(cx), cx);
        window.dispatch_action(Box::new(input::SelectAll), cx);
    }

    fn on_double_click_select_all(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.click_count >= 2 {
            self.select_all_text(window, cx);
        }
    }

    fn insert_text_at(&mut self, at: usize, text: &str, cx: &mut Context<Self>) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let held = self.field.read(cx).content().to_string();
        let at = floor_char(&held, at);
        let before = &held[..at];
        let after = &held[at..];
        let piece = voice_piece(before, text);
        let next = format!("{before}{piece}{after}");
        self.field
            .update(cx, |field, cx| field.set_content(next, cx));
        cx.notify();
    }

    pub fn accept_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_paths(paths, cx);
        window.focus(&self.field.read(cx).focus_handle(cx), cx);
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(id) = self.bound else {
            return;
        };
        self.load_paths(id, paths, cx);
    }

    fn load_paths(&mut self, id: u64, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let tray = self.attachments.get_mut(id);
        if tray.loading > 0 {
            tray.error = Some("Wait for the current attachments to finish loading".into());
            cx.notify();
            return;
        }
        if paths.len() > 16 {
            tray.error = Some("Drop at most 16 files at a time".into());
            cx.notify();
            return;
        }
        tray.loading += 1;
        tray.error = None;
        let mut staging = tray.clone();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .into_iter()
                        .map(|path| {
                            Attachment::load(path).and_then(|attachment| {
                                staging.insert(attachment.clone())?;
                                Ok(attachment)
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let tray = this.attachments.get_mut(id);
                tray.loading = tray.loading.saturating_sub(1);
                for attachment in loaded {
                    if let Err(error) = attachment.and_then(|a| tray.insert(a)) {
                        tray.error = Some(format!("{error:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn remove_attachment(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(id) = self.bound else {
            return;
        };
        let tray = self.attachments.get_mut(id);
        if ix < tray.items.len() {
            tray.items.remove(ix);
            tray.error = None;
            cx.notify();
        }
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.bound else {
            return;
        };
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| this.load_paths(id, paths, cx));
        })
        .detach();
    }

    fn lift_chat_links(&mut self, cx: &mut Context<Self>) {
        let content = self.field.read(cx).content().to_string();
        let (rest, chips) = take_chat_links(&content);
        if chips.is_empty() {
            return;
        }
        self.chat_links.extend(chips);
        self.field
            .update(cx, |field, cx| field.set_content(rest, cx));
        cx.notify();
    }

    /// The picker trigger, and it is a *read* of the text rather than a key
    /// handler: a `/` at the start of the field or after whitespace. Typing,
    /// pasting, arrowing back into the word and deleting the `/` all agree
    /// without any of them being special-cased.
    fn reread(&mut self, cx: &mut Context<Self>) {
        self.lift_chat_links(cx);
        let content = self.field.read(cx).content().clone();
        let caret = self.field.read(cx).cursor().min(content.len());
        match slash_query(&content, caret) {
            Some((start, query)) if !self.slash_dismissed => {
                let opened = self.command != Some(start);
                let query_changed = self.slash_query != query;
                self.command = Some(start);
                self.close_menu(cx);
                if query_changed {
                    self.slash_query = query.to_string();
                    self.filter.refilter(query);
                    self.reveal();
                }
                if opened || query_changed {
                    cx.notify();
                }
            }
            Some(_) => {
                self.command = None;
                self.slash_query.clear();
                self.slash_pointer = None;
            }
            None => {
                self.command = None;
                self.slash_dismissed = false;
                self.slash_query.clear();
                self.slash_pointer = None;
            }
        }
    }

    /// Take the highlighted row: an app action fires and the `/token` goes,
    /// an agent command replaces the token with `/name `.
    fn accept(&mut self, item: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(item).cloned() else {
            return;
        };
        let content = self.field.read(cx).content().clone();
        let caret = self.field.read(cx).cursor().min(content.len());
        let start = self.command.unwrap_or(0);
        let start = start.min(caret);
        let prefix = content.get(..start).unwrap_or("");
        let rest = content.get(caret..).unwrap_or("");
        let next = format!("{prefix}/{} {rest}", entry.slug);
        self.field
            .update(cx, |field, cx| field.set_content(next, cx));
        self.command = None;
        cx.notify();
    }

    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.bound else {
            return;
        };
        if self
            .attachments
            .get(Some(id))
            .is_some_and(|tray| tray.loading > 0)
        {
            return;
        }
        // `enter` is one key doing two jobs: while the picker is up it takes
        // the highlighted row, exactly as the combobox's does.
        if !self.menu
            && self.command.is_some()
            && let Some(item) = self.filter.active_item()
        {
            self.accept(item, cx);
            return;
        }
        let content = self.field.read(cx).content().clone();
        if content.trim().is_empty()
            && self
                .attachments
                .get(self.bound)
                .is_none_or(|tray| tray.items.is_empty() && tray.loading == 0)
            && self.chat_links.is_empty()
        {
            // Empty send still reaches the session: a lost connection
            // reconnects on it. A live session ignores the blank.
            cx.emit(ComposerEvent::Submit(Prompt::default()));
            return;
        }
        let prompt = Prompt::compose(
            &join_chat_links(&self.chat_links, &content),
            std::mem::take(&mut self.attachments.get_mut(id).items),
        );
        self.field.update(cx, |field, cx| field.clear(cx));
        self.attachments.get_mut(id).error = None;
        self.chat_links.clear();
        self.command = None;
        cx.emit(ComposerEvent::Submit(prompt));
        cx.notify();
    }

    /// Stop the running turn and send the queue plus whatever is in the
    /// field. Empty field is fine: the queue alone is enough.
    pub(crate) fn force(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.bound else {
            return;
        };
        if self
            .attachments
            .get(Some(id))
            .is_some_and(|tray| tray.loading > 0)
        {
            return;
        }
        let content = self.field.read(cx).content().clone();
        let empty = content.trim().is_empty()
            && self
                .attachments
                .get(self.bound)
                .is_none_or(|tray| tray.items.is_empty() && tray.loading == 0)
            && self.chat_links.is_empty();
        let prompt = if empty {
            Prompt::default()
        } else {
            let prompt = Prompt::compose(
                &join_chat_links(&self.chat_links, &content),
                std::mem::take(&mut self.attachments.get_mut(id).items),
            );
            self.field.update(cx, |field, cx| field.clear(cx));
            self.attachments.get_mut(id).error = None;
            self.chat_links.clear();
            self.command = None;
            prompt
        };
        cx.emit(ComposerEvent::Force(prompt));
        cx.notify();
    }

    fn send(&mut self, _: &Send, window: &mut Window, cx: &mut Context<Self>) {
        if self.pick_model(window, cx) {
            return;
        }
        self.submit(cx);
    }

    fn command_next(&mut self, _: &CommandNext, _: &mut Window, cx: &mut Context<Self>) {
        if self.step_model(1, cx) {
            return;
        }
        if self.command.is_none() {
            cx.emit(ComposerEvent::Step(1));
            return;
        }
        self.filter.step(1);
        self.reveal();
        cx.notify();
    }

    fn command_previous(&mut self, _: &CommandPrevious, _: &mut Window, cx: &mut Context<Self>) {
        if self.step_model(-1, cx) {
            return;
        }
        if self.command.is_none() {
            cx.emit(ComposerEvent::Step(-1));
            return;
        }
        self.filter.step(-1);
        self.reveal();
        cx.notify();
    }

    fn step_model(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        if !self.menu {
            return false;
        }
        let count = self.model_hits.len();
        if count == 0 {
            return true;
        }
        let next = self.model_active as isize + delta;
        self.model_active = next.rem_euclid(count as isize) as usize;
        self.scroll.scroll_to_item(self.model_active);
        cx.notify();
        true
    }

    fn pick_model(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.menu {
            return false;
        }
        let Some(switch) = self.model_switch() else {
            self.close_menu(cx);
            window.focus(&self.field.read(cx).focus_handle(cx), cx);
            cx.notify();
            return true;
        };
        let Some(&ix) = self.model_hits.get(self.model_active) else {
            self.close_menu(cx);
            window.focus(&self.field.read(cx).focus_handle(cx), cx);
            cx.notify();
            return true;
        };
        let Some(option) = switch.options.get(ix) else {
            self.close_menu(cx);
            window.focus(&self.field.read(cx).focus_handle(cx), cx);
            cx.notify();
            return true;
        };
        let id = switch.id.clone();
        let value = option.id.clone();
        self.close_menu(cx);
        window.focus(&self.field.read(cx).focus_handle(cx), cx);
        cx.emit(ComposerEvent::Switch(id, value));
        cx.notify();
        true
    }

    /// Scroll the highlighted row back inside the clamped card. The card's
    /// children are the filtered rows one for one, so the position the filter
    /// reports is the child gpui indexes.
    fn reveal(&self) {
        if let Some(active) = self.filter.active() {
            self.scroll.scroll_to_item(active);
        }
    }

    fn command_backspace(
        &mut self,
        _: &CommandBackspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.erase(true, window, cx);
    }

    fn command_delete(&mut self, _: &CommandDelete, window: &mut Window, cx: &mut Context<Self>) {
        self.erase(false, window, cx);
    }

    /// Empty field, no picker: the key belongs to the highlighted chat.
    /// Otherwise it is still typing.
    fn erase(&mut self, backspace: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.command.is_none() && !self.menu && self.is_empty(cx) {
            cx.emit(ComposerEvent::Delete);
            return;
        }
        if backspace {
            window.dispatch_action(Box::new(input::Backspace), cx);
        } else {
            window.dispatch_action(Box::new(input::Delete), cx);
        }
    }

    /// Escape backs out of whatever is happening, outermost first: the agent
    /// menu, then the command picker, and the turn in flight once there is
    /// nothing left to close.
    fn command_dismiss(&mut self, _: &CommandDismiss, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu {
            self.close_menu(cx);
            window.focus(&self.field.read(cx).focus_handle(cx), cx);
        } else if self.command.take().is_some() {
            self.slash_dismissed = true;
        } else {
            cx.emit(ComposerEvent::Cancel);
        }
        cx.notify();
    }

    /// The `/` menu: a compact floating list, pinned to the field's left —
    /// where the slash sits. TextField does not expose a caret x, so this is
    /// field-left rather than glyph-accurate.
    fn picker(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.menu {
            return None;
        }
        self.command?;
        let active = self.filter.active();
        let filtered = self.filter.filtered().to_vec();
        let rows: Vec<AnyElement> = filtered
            .iter()
            .enumerate()
            .filter_map(|(position, &item)| {
                let entry = self.entries.get(item)?;
                let description = entry.description.trim();
                Some(
                    popover::menu_row(theme, Some(position) == active, None)
                        .id(SharedString::from(format!("composer-slash-{item}")))
                        .gap(px(8.))
                        .py(px(4.))
                        .on_mouse_move(cx.listener(
                            move |composer: &mut Self, event: &MouseMoveEvent, _, cx| {
                                let pos = event.position;
                                if composer.slash_pointer == Some(pos) {
                                    return;
                                }
                                let first = composer.slash_pointer.is_none();
                                composer.slash_pointer = Some(pos);
                                // First report is often a layout pass with
                                // the pointer still over the top row. Keep
                                // the keyboard highlight until the mouse
                                // actually moves.
                                if first {
                                    return;
                                }
                                if composer.filter.active() != Some(position) {
                                    composer.filter.set_active(position);
                                    cx.notify();
                                }
                            },
                        ))
                        .on_click(cx.listener(move |composer, _, _, cx| {
                            composer.accept(item, cx);
                        }))
                        .child(
                            icons::icon(icons::devices::COMMAND)
                                .size(px(12.))
                                .text_color(theme.text_faint),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_color(theme.text)
                                .child(entry.slug.clone()),
                        )
                        .when(!description.is_empty(), |row| {
                            row.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_style(TextStyle::Caption)
                                    .text_color(theme.text_muted)
                                    .child(description.to_string()),
                            )
                        })
                        .into_any_element(),
                )
            })
            .collect();
        if rows.is_empty() {
            return None;
        }
        let card = popover::popover_card(theme)
            .id("composer-commands-list")
            .w(px(PICKER_WIDTH))
            .max_h(px(PICKER_HEIGHT))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(Self::on_double_click_select_all),
            )
            .children(rows);
        Some(popover::anchored_menu_above(
            "composer-commands",
            card.into_any_element(),
            None,
        ))
    }

    /// Shut the menu, and whatever it had a panel down over.
    fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = false;
        self.cursor.clear();
        self.model_query.clear();
        self.model_hits.clear();
        self.model_pointer = None;
        self.model_search.update(cx, |field, cx| field.clear(cx));
    }

    fn open_model_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = true;
        self.cursor.clear();
        self.model_pointer = None;
        self.model_query.clear();
        self.model_search.update(cx, |field, cx| field.clear(cx));
        self.rebuild_model_hits("");
        self.highlight_current_model();
        self.scroll.scroll_to_item(self.model_active);
        window.focus(&self.model_search.read(cx).focus_handle(cx), cx);
    }

    fn on_model_query(&mut self, cx: &mut Context<Self>) {
        if !self.menu {
            return;
        }
        let query = self.model_search.read(cx).content().to_string();
        if query == self.model_query {
            return;
        }
        self.model_query = query.clone();
        self.rebuild_model_hits(&query);
        self.model_active = 0;
        self.model_pointer = None;
        self.scroll.scroll_to_item(0);
        cx.notify();
    }

    fn rebuild_model_hits(&mut self, query: &str) {
        let Some(switch) = self.model_switch() else {
            self.model_hits.clear();
            self.model_active = 0;
            return;
        };
        let mut ranked: Vec<(usize, usize)> = switch
            .options
            .iter()
            .enumerate()
            .filter_map(|(ix, option)| model_rank(query, option).map(|rank| (rank, ix)))
            .collect();
        ranked.sort_by_key(|&(rank, ix)| (rank, ix));
        self.model_hits = ranked.into_iter().map(|(_, ix)| ix).collect();
    }

    fn highlight_current_model(&mut self) {
        let Some(switch) = self.model_switch() else {
            self.model_active = 0;
            return;
        };
        let current = switch.current.as_ref();
        self.model_active = self
            .model_hits
            .iter()
            .position(|&ix| switch.options.get(ix).map(|option| &option.id) == current)
            .unwrap_or(0);
    }

    /// Ghost mark inside the pill. No second circle, no glass of its own.
    fn icon_btn(
        &self,
        id: &'static str,
        icon: AnyElement,
        tip: &'static str,
        theme: &Theme,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .flex_none()
            .size(px(root::COMPOSER_HIT))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|button| button.bg(theme.element_hover))
            .tooltip(move |window, cx| Tooltip::text(tip, window, cx))
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .child(icon)
            .into_any_element()
    }

    fn glyph(icon: &'static str, theme: &Theme) -> AnyElement {
        icons::icon(icon)
            .size(px(15.))
            .text_color(theme.text)
            .into_any_element()
    }

    /// The model picker, as Cursor draws it: the model's name in muted text
    /// with a chevron, at the right end of the pill. Falls back to the agent
    /// mark when no model catalog has landed.
    fn chip(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let label: Option<SharedString> = self.model_switch().and_then(|switch| {
            let current = switch.current.as_ref()?;
            switch
                .options
                .iter()
                .find(|option| &option.id == current)
                .map(|option| option.name.clone())
                .or_else(|| Some(current.clone()))
        });
        let tip = if self.reconnect { "Reconnect" } else { "Model" };
        div()
            .id("composer-model")
            .flex_none()
            .h(px(root::COMPOSER_HIT))
            .px(px(6.))
            .rounded(px(6.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(3.))
            .cursor_pointer()
            .text_style(TextStyle::Body)
            .text_color(theme.text_muted)
            .hover(|button| button.bg(theme.element_hover))
            .tooltip(move |window, cx| Tooltip::text(tip, window, cx))
            .on_click(cx.listener(|composer, _, window, cx| {
                if composer.reconnect {
                    composer.submit(cx);
                    return;
                }
                if composer.menu {
                    composer.close_menu(cx);
                    window.focus(&composer.field.read(cx).focus_handle(cx), cx);
                } else {
                    composer.open_model_menu(window, cx);
                }
                cx.notify();
            }))
            .child(match label {
                Some(name) => div()
                    .max_w(px(220.))
                    .truncate()
                    .child(name)
                    .into_any_element(),
                None => Self::glyph(icons::system::WIDGET, theme),
            })
            .child(
                icons::icon(icons::arrows::ALT_ARROW_DOWN)
                    .size(px(12.))
                    .text_color(theme.text_faint)
                    .into_any_element(),
            )
            .into_any_element()
    }

    fn model_switch(&self) -> Option<&Switch> {
        self.switches
            .iter()
            .find(|switch| matches!(switch.id, SwitchId::Model))
            .or_else(|| self.switches.first())
    }

    /// Compact list above the 4-box — not a flyout beside the window.
    /// Search at the top, same shape as Cursor's model menu.
    fn menu_card(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.menu {
            return None;
        }
        let switch = self.model_switch();
        let options = switch
            .map(|switch| switch.options.as_slice())
            .unwrap_or(&[]);
        let current = switch.and_then(|switch| switch.current.clone());
        let switch_id = switch.map(|switch| switch.id.clone());
        let hits = &self.model_hits;
        let active = if hits.is_empty() {
            0
        } else {
            self.model_active.min(hits.len() - 1)
        };
        let rows: Vec<AnyElement> = hits
            .iter()
            .enumerate()
            .filter_map(|(position, &ix)| {
                let option = options.get(ix)?;
                let picked = current.as_ref() == Some(&option.id);
                let name = option.name.clone();
                let id = option.id.clone();
                let switch_id = switch_id.clone();
                Some(
                    popover::menu_row(theme, position == active, None)
                        .id(SharedString::from(format!("composer-model-{ix}")))
                        .gap(px(8.))
                        .py(px(4.))
                        .on_mouse_move(cx.listener(
                            move |composer: &mut Self, event: &MouseMoveEvent, _, cx| {
                                let pos = event.position;
                                if composer.model_pointer == Some(pos) {
                                    return;
                                }
                                let first = composer.model_pointer.is_none();
                                composer.model_pointer = Some(pos);
                                if first {
                                    return;
                                }
                                if composer.model_active != position {
                                    composer.model_active = position;
                                    cx.notify();
                                }
                            },
                        ))
                        .on_click(cx.listener(move |composer, _, window, cx| {
                            composer.close_menu(cx);
                            window.focus(&composer.field.read(cx).focus_handle(cx), cx);
                            if let Some(switch_id) = switch_id.clone() {
                                cx.emit(ComposerEvent::Switch(switch_id, id.clone()));
                            }
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.text)
                                .child(name),
                        )
                        .when(picked, |row| {
                            row.child(div().flex_none().text_color(theme.text_muted).child("✓"))
                        })
                        .into_any_element(),
                )
            })
            .collect();
        let empty = if options.is_empty() {
            if self.model_note.is_empty() {
                Some("No models".to_string())
            } else {
                Some(self.model_note.clone())
            }
        } else if rows.is_empty() {
            Some("No matches".to_string())
        } else {
            None
        };
        let list = div()
            .id("composer-model-list")
            .w(px(MODEL_PICKER_WIDTH))
            .max_h(px(PICKER_HEIGHT))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(rows)
            .children(empty.map(|note| {
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child(note)
            }));
        let card = popover::popover_card(theme)
            .id("composer-model-card")
            .w(px(MODEL_PICKER_WIDTH))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    // Search is its own field. Only take the send buffer
                    // when that field still holds focus.
                    if event.click_count >= 2
                        && this.field.read(cx).focus_handle(cx).is_focused(window)
                    {
                        this.select_all_text(window, cx);
                    }
                }),
            )
            .child(popover::search_line(
                theme,
                self.model_search.clone().into_any_element(),
            ))
            .child(list)
            .child(popover::divider())
            .child(self.usage_row(theme))
            .child(self.cost_row(theme));
        // `anchored_menu_above` puts 6px between the card and the 4-box.
        // The shield covers that button too, so this hit sits on the
        // floating layer over it — a second press still toggles shut.
        let layer = div().relative().child(card).child(
            div()
                .id("composer-model-toggle")
                .absolute()
                .left_0()
                .bottom(px(-(6. + root::COMPOSER_HIT)))
                .size(px(root::COMPOSER_HIT))
                .occlude()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_menu(cx);
                    window.focus(&this.field.read(cx).focus_handle(cx), cx);
                    cx.notify();
                })),
        );
        Some(
            div()
                .child(self.model_occluder(window, cx))
                .child(popover::anchored_menu_above(
                    "composer-models",
                    layer.into_any_element(),
                    None,
                ))
                .into_any_element(),
        )
    }

    /// Full-window catcher while the model menu is up. A press off the card
    /// would otherwise land on the transcript or the send field — move the
    /// caret, or send. This eats that press and shuts the menu. Escape, a
    /// pick, and the 4-box still shut it too. The card sits on a higher
    /// deferred layer, so a press on search, the list, or Context never
    /// reaches this.
    fn model_occluder(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let viewport = window.viewport_size();
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_0()
            .child(
                gpui::deferred(
                    gpui::anchored()
                        .position(gpui::point(px(0.0), px(0.0)))
                        .child(
                            div()
                                .id("composer-model-occluder")
                                .occlude()
                                .w(viewport.width)
                                .h(viewport.height)
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                    }),
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close_menu(cx);
                                    window.focus(&this.field.read(cx).focus_handle(cx), cx);
                                    cx.notify();
                                })),
                        ),
                )
                .priority(0),
            )
            .into_any_element()
    }

    /// Context spent, as the menu carries it: a name, and whatever the agent
    /// has counted. The number carries the warning rather than the track,
    /// because bezel's bar paints its fill from the theme and recolouring it
    /// would mean reimplementing it.
    ///
    /// A dash until it counts, and for an agent that never does — the two are
    /// one state here, since nothing in ACP asks and only an update tells. Not
    /// a 0%: a session with nothing said is not empty, its prompt and its
    /// tools being in the window before you type.
    fn usage_row(&self, theme: &Theme) -> AnyElement {
        let row = div()
            .id("composer-usage")
            // The metrics `popover::menu_row` gives a row, without the hover
            // and the pointer: nothing here is pressable.
            .px(px(8.))
            .py(px(6.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .text_style(TextStyle::Body)
            .text_color(theme.text_muted)
            .child(div().flex_1().min_w_0().child("Context"));
        let Some((usage, fraction)) = self
            .usage
            .and_then(|usage| Some((usage, usage.fraction()?)))
        else {
            return row
                .child(
                    div()
                        .text_style(TextStyle::Caption)
                        .text_color(theme.text_faint)
                        .child("—"),
                )
                .into_any_element();
        };
        let percent = (fraction * 100.).round() as u32;
        let (used, size) = (usage.used, usage.size);
        row
            // The raw counts would be noise in a row of words, and the tooltip
            // has them for whoever wants them.
            .tooltip(move |window, cx| {
                Tooltip::text(format!("{used} of {size} tokens"), window, cx)
            })
            .child(
                div()
                    .w(px(44.))
                    .flex_none()
                    .child(theme.progress_bar(fraction)),
            )
            .child(
                div()
                    .text_style(TextStyle::Caption)
                    .text_color(match fraction >= WARN_AT {
                        true => theme.warning,
                        false => theme.text_faint,
                    })
                    .child(format!("{percent}%")),
            )
            .into_any_element()
    }

    /// "Cost  $0.0123" under the context row, when the provider prices
    /// turns. The tooltip has the last turn's price.
    fn cost_row(&self, theme: &Theme) -> AnyElement {
        let Some(usage) = self.usage else {
            return div().into_any_element();
        };
        let Some(spent) = usage.spent else {
            return div().into_any_element();
        };
        let last = usage.last_cost.map(dollars).unwrap_or_else(|| "—".into());
        div()
            .id("composer-cost")
            .px(px(8.))
            .py(px(6.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .text_style(TextStyle::Body)
            .text_color(theme.text_muted)
            .tooltip(move |window, cx| {
                Tooltip::text(format!("last turn {last}; this chat since it was opened"), window, cx)
            })
            .child(div().flex_1().min_w_0().child("Cost"))
            .child(
                div()
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child(dollars(spent)),
            )
            .into_any_element()
    }

    fn tray_chip(
        &self,
        id: impl Into<gpui::ElementId>,
        x_id: impl Into<gpui::ElementId>,
        glyph: &'static str,
        label: SharedString,
        theme: &Theme,
        cx: &Context<Self>,
        remove: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.))
            .h(px(22.))
            .px(px(7.))
            .rounded_full()
            .bg(theme.element_hover)
            .child(
                icons::icon(glyph)
                    .size(px(11.))
                    .text_color(theme.text_faint),
            )
            .child(
                div()
                    .max_w(px(140.))
                    .truncate()
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text)
                    .child(label),
            )
            .child(
                div()
                    .id(x_id)
                    .size(px(14.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|hit| hit.bg(theme.element_active))
                    .child(
                        icons::icon(icons::system::CLOSE)
                            .size(px(10.))
                            .text_color(theme.text_faint),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| remove(this, cx))),
            )
            .into_any_element()
    }

    /// Chat links and files waiting on the card. One chip each, with an ✕.
    /// Nothing when the tray is empty — the row must not occupy space as a
    /// blank band.
    fn chips(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if self
            .attachments
            .get(self.bound)
            .is_none_or(|tray| tray.items.is_empty() && tray.loading == 0)
            && self.chat_links.is_empty()
        {
            return div().into_any_element();
        }
        div()
            .w_full()
            .mb(px(6.))
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(6.))
            .children(self.chat_links.iter().enumerate().map(|(ix, chip)| {
                let title = chip.title.clone();
                self.tray_chip(
                    ("composer-chat", ix),
                    ("composer-chat-x", ix),
                    icons::system::CHAT_ROUND_LINE,
                    title,
                    theme,
                    cx,
                    move |this, cx| {
                        if ix < this.chat_links.len() {
                            this.chat_links.remove(ix);
                            cx.notify();
                        }
                    },
                )
            }))
            .children(
                self.attachments
                    .get(self.bound)
                    .into_iter()
                    .flat_map(|tray| tray.items.iter())
                    .enumerate()
                    .map(|(ix, attachment)| {
                        let path = &attachment.path;
                        let name = path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("file")
                            .to_string();
                        super::attachment::chip(
                            ("composer-file", ix),
                            name,
                            attachment.preview.clone(),
                            theme,
                        )
                        .child(
                            div()
                                .id(("composer-file-x", ix))
                                .size(px(14.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|hit| hit.bg(theme.element_active))
                                .child(
                                    icons::icon(icons::system::CLOSE)
                                        .size(px(10.))
                                        .text_color(theme.text_faint),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_attachment(ix, cx);
                                })),
                        )
                        .into_any_element()
                    }),
            )
            .into_any_element()
    }

    /// Mic: ghost when idle, filled disc while the kernel is listening, faint
    /// while a transcript is coming back.
    fn voice_btn(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let recording = self.voice == VoiceState::Recording;
        let busy = self.voice == VoiceState::Busy;
        let tip = if recording {
            "Stop dictation"
        } else {
            "Hold Fn to talk"
        };
        let disc = div()
            .id("composer-voice")
            .flex_none()
            .size(px(root::COMPOSER_HIT))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .tooltip(move |window, cx| Tooltip::text(tip, window, cx));
        let disc = if recording {
            disc.bg(theme.solid)
                .cursor_pointer()
                .hover(|s| s.opacity(0.88))
                .child(
                    icons::icon(icons::media::MICROPHONE)
                        .size(px(13.))
                        .text_color(theme.on_solid),
                )
        } else if busy {
            disc.child(
                icons::icon(icons::media::MICROPHONE)
                    .size(px(15.))
                    .text_color(theme.text_faint),
            )
        } else {
            // Idle: a filled disc with a dark glyph, the same plate as Send;
            // Cursor's follow-up pill ends in exactly this.
            disc.bg(theme.solid)
                .cursor_pointer()
                .hover(|s| s.opacity(0.88))
                .child(
                    icons::icon(icons::media::MICROPHONE)
                        .size(px(13.))
                        .text_color(theme.on_solid),
                )
        };
        disc.on_click(cx.listener(|composer, _, _, cx| {
            if composer.voice != VoiceState::Busy {
                cx.emit(ComposerEvent::Voice);
            }
        }))
        .into_any_element()
    }

    /// Send sits at the end of the tool row. Empty: a faint arrow. Ready or
    /// stopping: one filled disc, never a second colour.
    ///
    /// A turn in flight used to steal this disc for Stop, so a follow-up
    /// had no button. Stop stays while it is running; Send comes back the
    /// moment there is text, and queues. Force sits beside Stop when there
    /// is something to push now — a queued line, or text in the field.
    fn button(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let streaming = self.streaming;
        let empty = self.is_empty(cx);
        let can_force = streaming && (self.queued || !empty);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .when(streaming, |row| {
                row.child(self.disc(
                    "composer-stop",
                    icons::media::STOP,
                    true,
                    "Stop",
                    theme,
                    cx,
                    |_, cx| cx.emit(ComposerEvent::Cancel),
                ))
            })
            .when(can_force, |row| {
                row.child(self.disc(
                    "composer-force",
                    icons::media::SKIP_NEXT,
                    true,
                    "Interrupt now: stop the running turn and send this",
                    theme,
                    cx,
                    |composer, cx| composer.force(cx),
                ))
            })
            .when(!streaming || !empty, |row| {
                row.child(self.disc(
                    "composer-send",
                    icons::arrows::ARROW_UP,
                    !empty || self.reconnect,
                    if streaming {
                        "Queue"
                    } else if self.reconnect {
                        "Reconnect"
                    } else {
                        "Send"
                    },
                    theme,
                    cx,
                    |composer, cx| composer.submit(cx),
                ))
            })
            .into_any_element()
    }

    fn disc(
        &self,
        id: &'static str,
        glyph: &'static str,
        ready: bool,
        tip: &'static str,
        theme: &Theme,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let disc = div()
            .id(id)
            .flex_none()
            .size(px(root::COMPOSER_HIT))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .tooltip(move |window, cx| Tooltip::text(tip, window, cx));
        let disc = if ready {
            disc.bg(theme.solid)
                .cursor_pointer()
                .hover(|s| s.opacity(0.88))
                .child(icons::icon(glyph).size(px(13.)).text_color(theme.on_solid))
        } else {
            disc.child(
                icons::icon(glyph)
                    .size(px(15.))
                    .text_color(theme.text_faint),
            )
        };
        disc.on_click(cx.listener(move |composer, _, _, cx| on_click(composer, cx)))
            .into_any_element()
    }

    /// Muted live transcript after the caret. Painted as its own line so a
    /// `StyledText` run-length mismatch cannot take down the whole card.
    fn voice_ghost(&self, theme: &Theme, cx: &App) -> Option<AnyElement> {
        if self.voice_preview.is_empty() {
            return None;
        }
        let content = self.field.read(cx).content();
        let at = floor_char(&content, self.voice_at.min(content.len()));
        let before = content.get(..at).unwrap_or("");
        let ghost = voice_piece(before, &self.voice_preview);
        if ghost.is_empty() {
            return None;
        }
        Some(
            div()
                .id("composer-voice-ghost")
                .w_full()
                .text_style(TextStyle::Caption)
                .text_color(theme.text_muted)
                .child(ghost)
                .into_any_element(),
        )
    }

    /// The field, and the hint sitting under it when the field is empty.
    /// Bezel paints its own placeholder as the text, so we draw the hint
    /// ourselves. It stays on an empty focused field, same as Cursor.
    fn field_stack(
        &self,
        theme: &Theme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let empty = self.field.read(cx).content().is_empty();
        let reconnect = self.reconnect;
        let picker = self.picker(theme, cx);
        let ghost = self.voice_ghost(theme, cx);
        let leading = px(TextStyle::Body.painted_line_height());
        div()
            .id("composer-field")
            .w_full()
            .min_w_0()
            .relative()
            .text_style(TextStyle::Body)
            .line_height(leading)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(Self::on_double_click_select_all),
            )
            .children(picker)
            .when(empty && ghost.is_none() && !self.hint.is_empty(), |el| {
                el.child(
                    div()
                        .id("composer-hint")
                        .absolute()
                        .left_0()
                        .top_0()
                        .cursor_pointer()
                        .text_style(TextStyle::Body)
                        .line_height(leading)
                        .text_color(theme.text_faint)
                        .child(hint_label(&self.hint, theme.text_faint, &theme))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                if reconnect {
                                    this.submit(cx);
                                } else {
                                    window.focus(&this.field.read(cx).focus_handle(cx), cx);
                                }
                            }),
                        ),
                )
            })
            .children(ghost)
            .child(self.field.clone())
            .into_any_element()
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let empty = self.is_empty(cx);
        let streaming = self.streaming;

        div()
            .on_action(cx.listener(Self::send))
            .on_action(cx.listener(Self::command_next))
            .on_action(cx.listener(Self::command_previous))
            .on_action(cx.listener(Self::command_dismiss))
            .on_action(cx.listener(Self::command_backspace))
            .on_action(cx.listener(Self::command_delete))
            .flex()
            .flex_none()
            .flex_col()
            .min_h(px(root::composer_height()))
            .gap(px(6.))
            .child(
                // Cursor's follow-up pill: one row, `+` at the left, the
                // field, the model's name, then the mic — which becomes the
                // send arrow the moment there is something to send.
                div()
                    .id("composer-card")
                    // No explicit width: the column stretches it, and the
                    // negative margins then bleed both edges like the
                    // strips above it do. `w_full` pinned it to the column
                    // width and only shifted it left.
                    .flex_none()
                    .min_h(px(root::composer_height()))
                    .rounded(px(root::COMPOSER_RADIUS))
                    .ml(px(-root::COMPOSER_PAD_X))
                    .mr(px(-root::COMPOSER_PAD_X))
                    .px(px(root::COMPOSER_PAD_X))
                    .pt(px(root::COMPOSER_PAD_TOP))
                    .pb(px(root::COMPOSER_PAD_BOTTOM))
                    .bg(theme.input_bg)
                    .border_1()
                    .border_color(theme.border)
                    .flex()
                    .flex_col()
                    .drag_over::<ExternalPaths>(|style, _, _, cx| {
                        style.bg(Theme::of(cx).element_hover)
                    })
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                        this.accept_paths(paths.paths().to_vec(), window, cx);
                        cx.stop_propagation();
                    }))
                    .drag_over::<SessionDrag>(|style, _, _, cx| {
                        style.bg(Theme::of(cx).element_hover)
                    })
                    .on_drop(cx.listener(|this, drag: &SessionDrag, window, cx| {
                        this.accept_chat_link(&drag.markdown, window, cx);
                    }))
                    .child(self.chips(&theme, cx))
                    .children(
                        self.attachments
                            .get(self.bound)
                            .and_then(|tray| {
                                if tray.loading > 0 {
                                    Some("Loading attachments…".to_string())
                                } else {
                                    tray.error.clone()
                                }
                            })
                            .map(|message| {
                                div()
                                    .text_style(TextStyle::Caption)
                                    .text_color(theme.text_muted)
                                    .child(message)
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_end()
                            .gap(px(8.))
                            .child(
                                // `+` in its own small disc, a step above the pill.
                                div()
                                    .id("composer-attach")
                                    .flex_none()
                                    .size(px(24.))
                                    .my(px((root::COMPOSER_HIT - 24.) / 2.))
                                    .rounded_full()
                                    .bg(theme.surface_raised_hover)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|button| button.bg(theme.element_active))
                                    .tooltip(|window, cx| Tooltip::text("Add files", window, cx))
                                    .on_click(
                                        cx.listener(|composer, _, _, cx| composer.pick_files(cx)),
                                    )
                                    .child(
                                        icons::icon(icons::system::PLUS)
                                            .size(px(13.))
                                            .text_color(theme.text_muted),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    // Centre a one-line field on the 28px
                                    // controls; a taller field keeps them
                                    // on its last line.
                                    .py(px((root::COMPOSER_HIT
                                        - TextStyle::Body.painted_line_height())
                                        / 2.))
                                    .child(self.field_stack(&theme, window, cx)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .relative()
                                            .children(self.menu_card(&theme, window, cx))
                                            .child(self.chip(&theme, cx)),
                                    )
                                    // One round button: mic while the field
                                    // is empty, send once there is text,
                                    // stop while a turn streams.
                                    .when(empty && !streaming, |row| {
                                        row.child(self.voice_btn(&theme, cx))
                                    })
                                    .when(!empty || streaming, |row| {
                                        row.child(self.button(&theme, cx))
                                    }),
                            ),
                    ),
            )
    }
}

/// The hint as one shaped run on the UI face, in `color`.
fn hint_label(text: &str, color: Hsla, theme: &Theme) -> StyledText {
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

impl Focusable for Composer {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.field.read(cx).focus_handle(cx)
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.watching_focus {
            self.watching_focus = true;
            let handle = self.field.read(cx).focus_handle(cx);
            cx.on_focus(&handle, window, |_, _, cx| cx.notify())
                .detach();
            cx.on_blur(&handle, window, |_, _, cx| cx.notify()).detach();
        }
        self.paint_placeholder(window, cx);
        self.body(window, cx)
    }
}

/// `$0.0041` under a cent, `$0.12` above, `$3.40` at dollars.
pub fn dollars(amount: f64) -> String {
    if amount < 0.01 {
        format!("${amount:.4}")
    } else if amount < 1.0 {
        format!("${amount:.3}")
    } else {
        format!("${amount:.2}")
    }
}
